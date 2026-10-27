//! A smoke domain: a box of air on a regular grid, simulated as an incompressible gas.
//!
//! Each voxel holds air velocity, smoke density (kg/m³), heat (°C above the surrounding air;
//! below zero for chilled, foggy air) and tint (albedo × density, so colors mix by mass). A
//! step:
//!
//! 1. **Forces**: hot smoke rises (buoyancy ∝ ΔT / T), cold air and dense smoke sink, and the
//!    air relaxes toward the wind (`WindField`, sampled on a coarse grid), so smoke drifts
//!    and bends with wind currents and fire updrafts.
//! 2. **Vorticity confinement** puts back the small swirls the coarse grid smears out.
//! 3. **Projection** makes the flow incompressible (Jacobi pressure solve). Air goes around
//!    things instead of through them, so a pushed patch of smoke rolls. Explosions and bullet
//!    wakes are *expansion sources* in that solve: air streams out of them, carving holes and
//!    rings, and the rest of the flow stays consistent.
//! 4. **Advection** (semi-Lagrangian, trilinear) moves everything along the flow; smoke slowly
//!    thins out and heat cools.
//!
//! The ground is a floor: voxels under it are solid (no smoke, no flow into it). Other faces
//! are open: smoke leaves, wind blows clean air in.

use engine_core::math::{DVec3, UVec3, Vec3};
use engine_core::tasks::par_chunks_mut;
use engine_world::WorldShift;

/// One voxel of air.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Voxel {
    /// Air velocity, m/s.
    pub vel: Vec3,
    /// Smoke, kg/m³.
    pub density: f32,
    /// Temperature above the surrounding air, °C (below zero: colder, so it sinks).
    pub heat: f32,
    /// Albedo × density (linear RGB): the smoke's color, weighted by mass so colors mix.
    pub tint: Vec3,
}

impl Voxel {
    /// Single-scattering albedo of the smoke here.
    pub fn albedo(&self) -> Vec3 {
        if self.density > 1e-9 {
            (self.tint / self.density).clamp(Vec3::ZERO, Vec3::ONE)
        } else {
            Vec3::splat(0.9)
        }
    }

    fn scaled(self, k: f32) -> Self {
        Self {
            vel: self.vel * k,
            density: self.density * k,
            heat: self.heat * k,
            tint: self.tint * k,
        }
    }

    fn add(self, o: Self) -> Self {
        Self {
            vel: self.vel + o.vel,
            density: self.density + o.density,
            heat: self.heat + o.heat,
            tint: self.tint + o.tint,
        }
    }
}

/// Shares of the difference [`Domain::settle_fog`] closes in one call (0..1 each).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogRates {
    /// Fog forming where there is less than its share.
    pub form: f32,
    /// Fog burning off where there is more.
    pub burn: f32,
    /// Foggy air chilling toward its temperature.
    pub chill: f32,
}

impl FogRates {
    /// Rates for `dt` seconds with these 1/e times, seconds.
    pub fn new(dt: f32, form_secs: f32, burn_secs: f32, chill_secs: f32) -> Self {
        let k = |secs: f32| 1.0 - (-dt / secs.max(1e-3)).exp();
        Self {
            form: k(form_secs),
            burn: k(burn_secs),
            chill: k(chill_secs),
        }
    }
}

/// A source of expanding air in the pressure solve (explosion, bullet wake).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expansion {
    pub from: DVec3,
    /// Same as `from` for a point source; a segment for a line source.
    pub to: DVec3,
    pub radius: f32,
    /// Volume growth rate at the core, 1/s.
    pub rate: f32,
    /// Seconds left.
    pub life: f32,
}

/// Simulation constants for one step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepParams {
    pub dt: f32,
    /// Surrounding air temperature, °C.
    pub ambient_c: f32,
    /// Multiplier on physical buoyancy (g ΔT / T).
    pub buoyancy: f32,
    /// Downward pull per kg/m³ of smoke, m/s² (dense, cold smoke sinks).
    pub weight: f32,
    /// How fast the air follows the wind, 1/s.
    pub wind_coupling: f32,
    /// Share of that coupling on vertical motion (updrafts); low so plumes keep rising.
    pub vertical_coupling: f32,
    /// Vorticity confinement strength (0 = off, ~0.3 lively).
    pub vorticity: f32,
    /// Pressure solver iterations.
    pub iterations: u32,
    /// Seconds for smoke to thin to 1/e by itself (mixing below the grid).
    pub dissipation_secs: f32,
    /// Seconds for hot smoke to cool to 1/e of its excess temperature.
    pub cool_secs: f32,
    /// Turbulent mixing too small for the grid, m²/s: softens and widens clouds.
    pub diffusion_m2_s: f32,
}

impl Default for StepParams {
    fn default() -> Self {
        Self {
            dt: 0.05,
            ambient_c: 15.0,
            // Twice physical: the grid smears thin plumes, which would rob them of their heat.
            buoyancy: 2.0,
            weight: 2.0,
            wind_coupling: 0.6,
            vertical_coupling: 0.1,
            vorticity: 0.35,
            iterations: 8,
            dissipation_secs: 120.0,
            cool_secs: 5.0,
            diffusion_m2_s: 0.2,
        }
    }
}

/// Coarse wind samples every this many voxels.
const WIND_STEP: u32 = 4;

/// A box of simulated air.
#[derive(Debug, Clone)]
pub struct Domain {
    /// World position of the min corner.
    pub origin: DVec3,
    /// Voxel size, meters.
    pub cell: f32,
    pub dims: UVec3,
    pub voxels: Vec<Voxel>,
    scratch: Vec<Voxel>,
    pressure: Vec<f32>,
    pressure_next: Vec<f32>,
    divergence: Vec<f32>,
    curl: Vec<Vec3>,
    /// Per (x, z) column: voxel layers under the ground (solid).
    pub floor: Vec<u16>,
    /// The floor no longer matches the ground (the world turned under the box): measure it
    /// again with [`Domain::set_floor`] before the next step.
    pub floor_stale: bool,
    wind: Vec<Vec3>,
    wind_dims: UVec3,
    pub expansions: Vec<Expansion>,
    /// Gas released since the last step: (center, radius, m³).
    inflow: Vec<(DVec3, f32, f32)>,
    /// Bumped every step that changed anything visible.
    pub version: u64,
    /// Bumped when the contents jumped instead of flowing (resampled into a turned world), so
    /// the renderer doesn't blend from what was there before (also on [`Domain::clear`]).
    pub resampled: u64,
    /// Smoke in the box, kg.
    pub mass_kg: f32,
    /// Densest voxel, kg/m³.
    pub max_density: f32,
    /// Seconds simulated.
    pub time: f32,
    /// Voxels simulated last step (min, max exclusive): where there is smoke, plus a margin.
    pub active: (UVec3, UVec3),
    /// How many voxels were simulated last step: within `active`, only the stretch of each
    /// row near the smoke (a slanted plume fills little of its box).
    pub active_voxels: u32,
    /// Holds a fog bank: drawn as soft, wide fog rather than billowing smoke.
    pub fog: bool,
}

impl Domain {
    pub fn new(origin: DVec3, cell: f32, dims: UVec3) -> Self {
        let dims = dims.max(UVec3::splat(4));
        let n = (dims.x * dims.y * dims.z) as usize;
        let wind_dims = (dims - 1) / WIND_STEP + 2;
        Self {
            origin,
            cell,
            dims,
            voxels: vec![Voxel::default(); n],
            scratch: vec![Voxel::default(); n],
            pressure: vec![0.0; n],
            pressure_next: vec![0.0; n],
            divergence: vec![0.0; n],
            curl: vec![Vec3::ZERO; n],
            floor: vec![0; (dims.x * dims.z) as usize],
            floor_stale: false,
            wind: vec![Vec3::ZERO; (wind_dims.x * wind_dims.y * wind_dims.z) as usize],
            wind_dims,
            expansions: Vec::new(),
            inflow: Vec::new(),
            version: 1,
            resampled: 0,
            mass_kg: 0.0,
            max_density: 0.0,
            time: 0.0,
            active: (UVec3::ZERO, UVec3::ZERO),
            active_voxels: 0,
            fog: false,
        }
    }

    /// World size of the box, meters.
    pub fn size(&self) -> DVec3 {
        self.dims.as_dvec3() * self.cell as f64
    }

    pub fn center(&self) -> DVec3 {
        self.origin + self.size() * 0.5
    }

    pub fn contains(&self, p: DVec3) -> bool {
        let q = p - self.origin;
        let s = self.size();
        q.x >= 0.0 && q.y >= 0.0 && q.z >= 0.0 && q.x < s.x && q.y < s.y && q.z < s.z
    }

    #[inline]
    pub fn index(&self, x: u32, y: u32, z: u32) -> usize {
        (x + self.dims.x * (y + self.dims.y * z)) as usize
    }

    /// Continuous voxel coordinates of a world point (voxel centers at i + 0.5).
    pub fn local(&self, p: DVec3) -> Vec3 {
        ((p - self.origin) / self.cell as f64).as_vec3()
    }

    /// World position of a voxel's center.
    pub fn voxel_center(&self, x: u32, y: u32, z: u32) -> DVec3 {
        self.origin + (UVec3::new(x, y, z).as_dvec3() + 0.5) * self.cell as f64
    }

    pub fn is_solid(&self, x: u32, y: u32, z: u32) -> bool {
        y < self.floor[(x + z * self.dims.x) as usize] as u32
    }

    /// Set the ground under the box: `ground(x, z)` gives the ground height at a world
    /// position, or `None` for no ground.
    pub fn set_floor(&mut self, ground: impl Fn(f64, f64) -> Option<f64> + Sync) {
        // Ground queries are dear (a planet's terrain): a row of columns per task.
        let (origin, cell, dims) = (self.origin, self.cell as f64, self.dims);
        par_chunks_mut(&mut self.floor, dims.x as usize, |z, row| {
            for (x, layers) in row.iter_mut().enumerate() {
                let c = origin + DVec3::new(x as f64 + 0.5, 0.5, z as f64 + 0.5) * cell;
                *layers = ground(c.x, c.z)
                    .map(|h| ((h - origin.y) / cell).round().max(0.0))
                    .unwrap_or(0.0)
                    .min(dims.y as f64) as u16;
            }
        });
    }

    /// Trilinear sample at continuous voxel coordinates; outside the box, still air.
    pub fn sample_local(&self, q: Vec3) -> Voxel {
        sample(&self.voxels, self.dims, q, &|_| Voxel::default())
    }

    /// The air at a world point (still, clean air outside the box).
    pub fn sample(&self, p: DVec3) -> Voxel {
        self.sample_local(self.local(p))
    }

    /// Release smoke: `mass_kg` spread over a ball of `radius`, at `heat_c` above the air,
    /// moving at `vel`, colored `albedo`.
    pub fn splat(
        &mut self,
        p: DVec3,
        radius: f32,
        mass_kg: f32,
        heat_c: f32,
        vel: Vec3,
        albedo: Vec3,
    ) {
        // At least a voxel wide, so a point anywhere in the box reaches a voxel center.
        let r = radius.max(self.cell);
        let mut cells: Vec<(usize, f32)> = Vec::new();
        let mut total = 0.0;
        self.visit_ball(p, r, |i, d| {
            let w = (1.0 - (d / r).powi(2)).max(0.0).powi(2);
            if w > 0.0 {
                cells.push((i, w));
                total += w;
            }
        });
        if total <= 0.0 {
            return;
        }
        let vol = self.cell.powi(3);
        self.mass_kg += mass_kg;
        for (i, w) in cells {
            let v = &mut self.voxels[i];
            let d = mass_kg * w / total / vol;
            v.density += d;
            v.tint += albedo * d;
            v.heat = v.heat.max(heat_c * w);
            v.vel += (vel - v.vel) * w;
        }
    }

    /// Fog forming and burning off. Every open voxel whose fog share `level(y)` (0..1, from
    /// the voxel center's world height) calls for more than it holds gains `rate.form` of the
    /// difference toward `level × density` kg/m³, colored `albedo`; its air is chilled toward
    /// `level × chill_c` below the surroundings (`rate.chill` of the difference) so it sinks
    /// and pools. Chilled (foggy) voxels holding more than their share lose `rate.burn` of the
    /// excess. Smoke that isn't fog (warm or at air temperature) is left alone.
    pub fn settle_fog(
        &mut self,
        level: impl Fn(f64) -> f32 + Sync,
        density: f32,
        chill_c: f32,
        albedo: Vec3,
        rate: FogRates,
    ) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let (origin_y, cell) = (self.origin.y, self.cell as f64);
        let levels: Vec<f32> = (0..dims.y)
            .map(|y| level(origin_y + (y as f64 + 0.5) * cell).clamp(0.0, 1.0))
            .collect();
        let floor = &self.floor;
        let k = rate.form.clamp(0.0, 1.0);
        let burn = rate.burn.clamp(0.0, 1.0);
        let chill = rate.chill.clamp(0.0, 1.0);
        par_chunks_mut(&mut self.voxels, slice, |z, chunk| {
            let z = z as u32;
            for x in 0..dims.x {
                let f = floor[(x + z * dims.x) as usize] as u32;
                for (y, &l) in levels.iter().enumerate().skip(f as usize) {
                    let v = &mut chunk[(x + y as u32 * dims.x) as usize];
                    let want = l * density;
                    let cold = -chill_c * l;
                    if l > 0.0 && v.heat > cold {
                        v.heat += (cold - v.heat) * chill;
                    }
                    if want > v.density {
                        let add = (want - v.density) * k;
                        v.density += add;
                        v.tint += albedo * add;
                    } else if v.heat < -0.01 && v.density > 0.0 {
                        let keep = 1.0 - (v.density - want) / v.density * burn;
                        v.density *= keep;
                        v.tint *= keep;
                        if v.density < 1e-8 {
                            *v = Voxel {
                                vel: v.vel,
                                ..Voxel::default()
                            };
                        }
                    }
                }
            }
        });
        let total: f64 = self.voxels.iter().map(|v| v.density as f64).sum();
        self.mass_kg = (total * cell.powi(3)) as f32;
        self.max_density = self.voxels.iter().map(|v| v.density).fold(0.0, f32::max);
    }

    /// Gas released at a point (a grenade's smoke, a vent's steam): `volume_m3` of new gas
    /// that pushes the air around it outward over the next step, so the cloud billows out.
    pub fn inflate(&mut self, center: DVec3, radius: f32, volume_m3: f32) {
        if volume_m3 <= 0.0 {
            return;
        }
        let r = radius.max(self.cell * 2.0);
        match self
            .inflow
            .iter_mut()
            .find(|(c, _, _)| c.distance_squared(center) < 1e-4)
        {
            Some((_, radius, volume)) => {
                *radius = radius.max(r);
                *volume += volume_m3;
            }
            None => self.inflow.push((center, r, volume_m3)),
        }
    }

    /// A bullet: clear smoke along the segment within `radius` (`clear` 0..1 of it at the
    /// core), drag the air along, and blow it out of the wake.
    pub fn carve(&mut self, from: DVec3, to: DVec3, radius: f32, clear: f32, speed: f32) {
        // The grid can't hold a centimeter-wide hole: clear the voxels the path runs through
        // (the renderer draws the sharp tunnel on top).
        let r = (radius * 4.0).max(self.cell * 1.5);
        let dir = (to - from).normalize_or_zero().as_vec3();
        let push = (speed * 0.004).min(4.0);
        self.visit_capsule(from, to, r, |v, d, _| {
            let t = ((d / r - 0.5) * 2.0).clamp(0.0, 1.0);
            let f = 1.0 - t * t * (3.0 - 2.0 * t);
            let k = 1.0 - clear * f;
            v.density *= k;
            v.tint *= k;
            v.vel += dir * push * f;
        });
        self.expansions.push(Expansion {
            from,
            to,
            radius: r * 1.5,
            rate: 6.0,
            life: 0.15,
        });
    }

    /// An explosion or shockwave: air streams out of a ball of `radius`, peaking at
    /// `strength` m/s at its edge, for a short moment.
    pub fn blast(&mut self, center: DVec3, radius: f32, strength: f32) {
        let r = radius.max(self.cell);
        self.expansions.push(Expansion {
            from: center,
            to: center,
            radius: r,
            rate: 3.0 * strength / r,
            life: 0.25,
        });
    }

    /// Something moving through the air (a car, a character): inside its ball the air moves
    /// with it and the smoke is pushed out of the way.
    pub fn obstacle(&mut self, center: DVec3, radius: f32, vel: Vec3) {
        let r = radius.max(self.cell * 0.5);
        self.visit_capsule(center, center, r, |v, d, _| {
            let f = (1.0 - d / r).clamp(0.0, 1.0).sqrt();
            v.vel += (vel - v.vel) * f;
            v.density *= 1.0 - 0.5 * f;
            v.tint *= 1.0 - 0.5 * f;
        });
    }

    /// A fan, vent or rotor: air within `radius` of the line from `from` along `dir` for
    /// `length` meters is driven toward `speed` m/s, weaker toward the end of the jet.
    pub fn jet(&mut self, from: DVec3, dir: Vec3, length: f32, radius: f32, speed: f32, dt: f32) {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO || length <= 0.0 {
            return;
        }
        let to = from + dir.as_dvec3() * length as f64;
        let r = radius.max(self.cell);
        let k = (dt * 6.0).min(1.0);
        self.visit_capsule(from, to, r, |v, d, along| {
            let f = (1.0 - d / r).clamp(0.0, 1.0).sqrt() * (1.0 - 0.6 * along);
            v.vel += (dir * speed - v.vel) * (k * f);
        });
    }

    fn visit_ball(&self, p: DVec3, r: f32, mut f: impl FnMut(usize, f32)) {
        let q = self.local(p);
        let rv = r / self.cell;
        let lo = (q - rv).floor().max(Vec3::ZERO).as_uvec3();
        let hi = (q + rv).ceil().min(self.dims.as_vec3()).as_uvec3();
        for z in lo.z..hi.z {
            for y in lo.y..hi.y {
                for x in lo.x..hi.x {
                    if self.is_solid(x, y, z) {
                        continue;
                    }
                    let c = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                    let d = (c - q).length() * self.cell;
                    if d < r {
                        f(self.index(x, y, z), d);
                    }
                }
            }
        }
    }

    /// Visit voxels within `r` of the segment a–b: `f(voxel, distance, along 0..1)`.
    fn visit_capsule(
        &mut self,
        a: DVec3,
        b: DVec3,
        r: f32,
        mut f: impl FnMut(&mut Voxel, f32, f32),
    ) {
        let qa = self.local(a);
        let qb = self.local(b);
        let rv = r / self.cell;
        let lo = (qa.min(qb) - rv).floor().max(Vec3::ZERO).as_uvec3();
        let hi = (qa.max(qb) + rv).ceil().min(self.dims.as_vec3()).as_uvec3();
        let ab = qb - qa;
        let len2 = ab.length_squared().max(1e-9);
        for z in lo.z..hi.z {
            for y in lo.y..hi.y {
                for x in lo.x..hi.x {
                    if self.is_solid(x, y, z) {
                        continue;
                    }
                    let c = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                    let t = ((c - qa).dot(ab) / len2).clamp(0.0, 1.0);
                    let d = (c - (qa + ab * t)).length() * self.cell;
                    if d < r {
                        let i = self.index(x, y, z);
                        f(&mut self.voxels[i], d, t);
                    }
                }
            }
        }
    }

    /// Fraction of light that gets through the smoke from `a` to `b` (1 = clear), for
    /// `extinction` m²/kg.
    pub fn transmittance(&self, a: DVec3, b: DVec3, extinction: f32) -> f32 {
        let Some((t0, t1)) = self.clip(a, b) else {
            return 1.0;
        };
        let len = (b - a).length() * (t1 - t0);
        let steps = ((len / (self.cell as f64 * 0.5)).ceil() as usize).clamp(1, 4096);
        let ds = (len / steps as f64) as f32;
        let mut tau = 0.0;
        for i in 0..steps {
            let t = t0 + (t1 - t0) * (i as f64 + 0.5) / steps as f64;
            tau += self.sample(a.lerp(b, t)).density * extinction * ds;
        }
        (-tau).exp()
    }

    /// The part of segment a–b inside the box, as parameters 0..1.
    pub fn clip(&self, a: DVec3, b: DVec3) -> Option<(f64, f64)> {
        clip_box(self.origin, self.origin + self.size(), a, b)
    }

    /// Whether there's anything to simulate.
    pub fn is_idle(&self) -> bool {
        self.mass_kg <= 1e-6 && self.expansions.is_empty() && self.inflow.is_empty()
    }

    /// Advance the air by `p.dt`. `wind(p)` is the wind at a world point.
    pub fn step(&mut self, p: &StepParams, wind: &(dyn Fn(DVec3) -> Vec3 + Sync)) {
        if self.is_idle() {
            return;
        }
        self.sample_wind(wind);
        self.step_sampled(p);
    }

    /// [`step`](Self::step) in the wind last sampled with [`sample_wind`](Self::sample_wind):
    /// it needs nothing from the world, so it can run on a background task.
    pub fn step_sampled(&mut self, p: &StepParams) {
        if self.is_idle() {
            return;
        }
        let dt = p.dt;
        self.time += dt;
        // Released gas expands over this step: the expansion rate falls off linearly to the
        // edge of its ball, which holds π r³ / 3 of volume per unit rate.
        for (c, r, volume) in std::mem::take(&mut self.inflow) {
            self.expansions.push(Expansion {
                from: c,
                to: c,
                radius: r,
                rate: 3.0 * volume / (dt.max(1e-4) * std::f32::consts::PI * r.powi(3)),
                life: dt * 0.999,
            });
        }
        // Spans show each phase's share in the engine profile (`--profile`).
        let span = tracing::info_span!("smoke forces").entered();
        // Only the air around the smoke is worth solving; the rest just follows the wind.
        let r = self.measure();
        self.apply_forces(p, &r);
        if p.vorticity > 0.0 {
            self.confine_vorticity(p.vorticity, dt, &r);
        }
        drop(span);
        let span = tracing::info_span!("smoke pressure").entered();
        self.project(p.iterations, &r);
        drop(span);
        let span = tracing::info_span!("smoke advect").entered();
        self.advect(p, &r);
        // Explicit diffusion is stable up to 1/6 per step.
        let mix = (p.diffusion_m2_s * dt / (self.cell * self.cell)).min(0.16);
        if mix > 1e-4 {
            self.diffuse(mix, &r);
        }
        drop(span);
        for e in &mut self.expansions {
            e.life -= dt;
        }
        self.expansions.retain(|e| e.life > 0.0);
        self.version += 1;
    }

    /// Where the next step works ([`Region`]); it also sets `active` and `active_voxels`.
    fn measure(&mut self) -> Region {
        let r = self.region();
        self.active = (r.lo, r.hi);
        self.active_voxels = r.voxels(self.dims);
        r
    }

    /// Each row's stretch of voxels with smoke, heat or expanding air in it, widened by room
    /// for it to move and for the flow around it (the sparse-brick idea of NVIDIA's GDC 2015
    /// sparse fluid talk, along rows: a plume the wind lays over fills little of its box).
    fn region(&self) -> Region {
        let d = self.dims;
        let (nx, ny) = (d.x as usize, d.y as usize);
        let mut raw = vec![(0u32, 0u32); (d.y * d.z) as usize];
        let voxels = &self.voxels;
        // Rows are `y + z * dims.y`, so row r's voxels start at r * dims.x.
        par_chunks_mut(&mut raw, ny * 4, |c, chunk| {
            for (k, span) in chunk.iter_mut().enumerate() {
                let start = (c * ny * 4 + k) * nx;
                let row = &voxels[start..start + nx];
                let busy = |v: &Voxel| v.density > 1e-7 || v.heat.abs() > 0.05;
                if let (Some(first), Some(last)) =
                    (row.iter().position(busy), row.iter().rposition(busy))
                {
                    *span = (first as u32, last as u32 + 1);
                }
            }
        });
        for e in &self.expansions {
            let (a, b) = (self.local(e.from), self.local(e.to));
            let r = e.radius / self.cell;
            let emin = (a.min(b) - r).floor().max(Vec3::ZERO).as_uvec3().min(d);
            let emax = (a.max(b) + r).ceil().max(Vec3::ZERO).as_uvec3().min(d);
            for z in emin.z..emax.z {
                for y in emin.y..emax.y {
                    let i = (y + z * d.y) as usize;
                    raw[i] = join(raw[i], (emin.x, emax.x));
                }
            }
        }
        let m = ((2.0 / self.cell).ceil() as u32).max(3);
        let act = dilate(&raw, d, m);
        let wide = dilate(&raw, d, m + 1);
        let (mut lo, mut hi) = (d, UVec3::ZERO);
        for z in 0..d.z {
            for y in 0..d.y {
                let (a, b) = wide[(y + z * d.y) as usize];
                if a < b {
                    lo = lo.min(UVec3::new(a, y, z));
                    hi = hi.max(UVec3::new(b, y + 1, z + 1));
                }
            }
        }
        if hi.x <= lo.x {
            (lo, hi) = (UVec3::ZERO, UVec3::ZERO);
        }
        Region { act, wide, lo, hi }
    }

    /// Sample the wind the box feels (from the world, before a [`step_sampled`]).
    ///
    /// [`step_sampled`]: Self::step_sampled
    pub fn sample_wind(&mut self, wind: &(dyn Fn(DVec3) -> Vec3 + Sync)) {
        let wd = self.wind_dims;
        for z in 0..wd.z {
            for y in 0..wd.y {
                for x in 0..wd.x {
                    let p = self.origin
                        + UVec3::new(x, y, z).as_dvec3() * (WIND_STEP as f64 * self.cell as f64);
                    self.wind[(x + wd.x * (y + wd.y * z)) as usize] = wind(p);
                }
            }
        }
    }

    /// Wind, buoyancy and weight, in the active region and a layer of voxels around it (all
    /// the solve and the advection read; the still air beyond is left as it was).
    fn apply_forces(&mut self, p: &StepParams, r: &Region) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let floor = &self.floor;
        let wind = &self.wind;
        let wd = self.wind_dims;
        let k_wind = 1.0 - (-p.wind_coupling * p.dt).exp();
        let vertical_coupling = p.vertical_coupling;
        let lift = 9.81 * p.buoyancy / (p.ambient_c + 273.15).max(50.0);
        let (dt, weight) = (p.dt, p.weight);
        par_chunks_mut(&mut self.voxels, slice, |z, chunk| {
            let z = z as u32;
            if z < r.lo.z || z >= r.hi.z {
                return;
            }
            for y in r.lo.y..r.hi.y {
                for x in r.wide(dims, y, z) {
                    let v = &mut chunk[(x + y * dims.x) as usize];
                    if y < floor[(x + z * dims.x) as usize] as u32 {
                        *v = Voxel::default();
                        continue;
                    }
                    let w = wind_at(wind, wd, Vec3::new(x as f32, y as f32, z as f32));
                    v.vel.x += (w.x - v.vel.x) * k_wind;
                    v.vel.z += (w.z - v.vel.z) * k_wind;
                    v.vel.y += (w.y - v.vel.y) * k_wind * vertical_coupling;
                    v.vel.y += (lift * v.heat - weight * v.density) * dt;
                }
            }
        });
    }

    fn confine_vorticity(&mut self, strength: f32, dt: f32, r: &Region) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let h = self.cell;
        // Curl one voxel beyond the region (its wide spans), so the confinement inside it has
        // neighbors.
        {
            let voxels = &self.voxels;
            par_chunks_mut(&mut self.curl, slice, |z, chunk| {
                let z = z as u32;
                if z < r.lo.z || z >= r.hi.z {
                    return;
                }
                for y in r.lo.y..r.hi.y {
                    for x in r.wide(dims, y, z) {
                        let g = |dx: i32, dy: i32, dz: i32| {
                            voxels[clamped_index(dims, x as i32 + dx, y as i32 + dy, z as i32 + dz)]
                                .vel
                        };
                        let (xp, xm) = (g(1, 0, 0), g(-1, 0, 0));
                        let (yp, ym) = (g(0, 1, 0), g(0, -1, 0));
                        let (zp, zm) = (g(0, 0, 1), g(0, 0, -1));
                        chunk[(x + y * dims.x) as usize] = Vec3::new(
                            (yp.z - ym.z) - (zp.y - zm.y),
                            (zp.x - zm.x) - (xp.z - xm.z),
                            (xp.y - xm.y) - (yp.x - ym.x),
                        ) / (2.0 * h);
                    }
                }
            });
        }
        let curl = &self.curl;
        let floor = &self.floor;
        par_chunks_mut(&mut self.voxels, slice, |z, chunk| {
            let z = z as u32;
            if z < r.lo.z || z >= r.hi.z {
                return;
            }
            for y in r.lo.y..r.hi.y {
                for x in r.act(dims, y, z) {
                    if y < floor[(x + z * dims.x) as usize] as u32 {
                        continue;
                    }
                    let m = |dx: i32, dy: i32, dz: i32| {
                        curl[clamped_index(dims, x as i32 + dx, y as i32 + dy, z as i32 + dz)]
                            .length()
                    };
                    let grad = Vec3::new(
                        m(1, 0, 0) - m(-1, 0, 0),
                        m(0, 1, 0) - m(0, -1, 0),
                        m(0, 0, 1) - m(0, 0, -1),
                    );
                    let len = grad.length();
                    if len < 1e-6 {
                        continue;
                    }
                    let n = grad / len;
                    let w = curl[(x + dims.x * (y + dims.y * z)) as usize];
                    chunk[(x + y * dims.x) as usize].vel += n.cross(w) * (strength * h * dt);
                }
            }
        });
    }

    fn project(&mut self, iterations: u32, r: &Region) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let h = self.cell;
        // Divergence, minus expansion sources (air that appears there pushes the rest away).
        let sources = self.sources();
        {
            let voxels = &self.voxels;
            let floor = &self.floor;
            par_chunks_mut(&mut self.divergence, slice, |z, chunk| {
                let z = z as u32;
                if z < r.lo.z || z >= r.hi.z {
                    return;
                }
                for y in r.lo.y..r.hi.y {
                    for x in r.act(dims, y, z) {
                        let out = &mut chunk[(x + y * dims.x) as usize];
                        if y < floor[(x + z * dims.x) as usize] as u32 {
                            *out = 0.0;
                            continue;
                        }
                        let vel = |dx: i32, dy: i32, dz: i32| {
                            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                            // Below the floor: no flow through it.
                            if ny < 0
                                || (nx >= 0
                                    && nz >= 0
                                    && (nx as u32) < dims.x
                                    && (nz as u32) < dims.z
                                    && (ny as u32)
                                        < floor[(nx as u32 + nz as u32 * dims.x) as usize] as u32)
                            {
                                return Vec3::ZERO;
                            }
                            voxels[clamped_index(dims, nx, ny, nz)].vel
                        };
                        let mut d = (vel(1, 0, 0).x - vel(-1, 0, 0).x + vel(0, 1, 0).y
                            - vel(0, -1, 0).y
                            + vel(0, 0, 1).z
                            - vel(0, 0, -1).z)
                            / (2.0 * h);
                        let c = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                        d -= expansion_at(&sources, c);
                        *out = d;
                    }
                }
            });
        }
        // Jacobi: open faces (and the air around the region) hold p = 0, the floor reflects.
        // Inside the region the solve starts from the last step's pressure: the flow changes
        // little from one step to the next, so a few iterations get close.
        for buf in [&mut self.pressure, &mut self.pressure_next] {
            par_chunks_mut(buf, slice, |z, chunk| {
                let z = z as u32;
                for y in 0..dims.y {
                    let row = &mut chunk[(y * dims.x) as usize..((y + 1) * dims.x) as usize];
                    let span = r.act(dims, y, z);
                    if span.is_empty() {
                        row.fill(0.0);
                    } else {
                        row[..span.start as usize].fill(0.0);
                        row[span.end as usize..].fill(0.0);
                    }
                }
            });
        }
        // Above this layer a column and its four neighbors are all open air: there the six
        // neighbors are read straight, without the edge and floor checks (most of the work).
        let open_above: Vec<u32> = (0..dims.z)
            .flat_map(|z| (0..dims.x).map(move |x| (x, z)))
            .map(|(x, z)| {
                let f = |x: u32, z: u32| self.floor[(x + z * dims.x) as usize] as u32;
                if x == 0 || z == 0 || x + 1 >= dims.x || z + 1 >= dims.z {
                    return u32::MAX;
                }
                f(x, z)
                    .max(f(x - 1, z))
                    .max(f(x + 1, z))
                    .max(f(x, z - 1))
                    .max(f(x, z + 1))
                    + 1
            })
            .collect();
        let row = dims.x as usize;
        for _ in 0..iterations {
            {
                let p = &self.pressure;
                let div = &self.divergence;
                let floor = &self.floor;
                let open_above = &open_above;
                par_chunks_mut(&mut self.pressure_next, slice, |z, chunk| {
                    let z = z as u32;
                    if z < r.lo.z || z >= r.hi.z {
                        return;
                    }
                    for y in r.lo.y..r.hi.y {
                        for x in r.act(dims, y, z) {
                            let i = (x + dims.x * (y + dims.y * z)) as usize;
                            if y >= open_above[(x + z * dims.x) as usize] && y + 1 < dims.y {
                                let sum = p[i + 1]
                                    + p[i - 1]
                                    + p[i + row]
                                    + p[i - row]
                                    + p[i + slice]
                                    + p[i - slice];
                                chunk[(x + y * dims.x) as usize] = (sum - div[i] * h * h) / 6.0;
                                continue;
                            }
                            let fl = floor[(x + z * dims.x) as usize] as u32;
                            if y < fl {
                                chunk[(x + y * dims.x) as usize] = 0.0;
                                continue;
                            }
                            let here = p[i];
                            let at = |nx: i32, ny: i32, nz: i32| {
                                if nx < 0
                                    || nz < 0
                                    || nx >= dims.x as i32
                                    || nz >= dims.z as i32
                                    || ny >= dims.y as i32
                                {
                                    return 0.0;
                                }
                                if ny < 0
                                    || (ny as u32)
                                        < floor[(nx as u32 + nz as u32 * dims.x) as usize] as u32
                                {
                                    return here;
                                }
                                p[(nx as u32 + dims.x * (ny as u32 + dims.y * nz as u32)) as usize]
                            };
                            let (xi, yi, zi) = (x as i32, y as i32, z as i32);
                            let sum = at(xi + 1, yi, zi)
                                + at(xi - 1, yi, zi)
                                + at(xi, yi + 1, zi)
                                + at(xi, yi - 1, zi)
                                + at(xi, yi, zi + 1)
                                + at(xi, yi, zi - 1);
                            chunk[(x + y * dims.x) as usize] = (sum - div[i] * h * h) / 6.0;
                        }
                    }
                });
            }
            std::mem::swap(&mut self.pressure, &mut self.pressure_next);
        }
        // Subtract the pressure gradient.
        let p = &self.pressure;
        let floor = &self.floor;
        par_chunks_mut(&mut self.voxels, slice, |z, chunk| {
            let z = z as u32;
            if z < r.lo.z || z >= r.hi.z {
                return;
            }
            for y in r.lo.y..r.hi.y {
                for x in r.act(dims, y, z) {
                    let fl = floor[(x + z * dims.x) as usize] as u32;
                    if y < fl {
                        continue;
                    }
                    let i = (x + dims.x * (y + dims.y * z)) as usize;
                    let here = p[i];
                    let at = |nx: i32, ny: i32, nz: i32| {
                        if nx < 0
                            || nz < 0
                            || nx >= dims.x as i32
                            || nz >= dims.z as i32
                            || ny >= dims.y as i32
                        {
                            return 0.0;
                        }
                        if ny < 0
                            || (ny as u32) < floor[(nx as u32 + nz as u32 * dims.x) as usize] as u32
                        {
                            return here;
                        }
                        p[(nx as u32 + dims.x * (ny as u32 + dims.y * nz as u32)) as usize]
                    };
                    let (xi, yi, zi) = (x as i32, y as i32, z as i32);
                    let grad = Vec3::new(
                        at(xi + 1, yi, zi) - at(xi - 1, yi, zi),
                        at(xi, yi + 1, zi) - at(xi, yi - 1, zi),
                        at(xi, yi, zi + 1) - at(xi, yi, zi - 1),
                    ) / (2.0 * h);
                    let v = &mut chunk[(x + y * dims.x) as usize];
                    v.vel -= grad;
                    if y == fl && v.vel.y < 0.0 {
                        v.vel.y = 0.0;
                    }
                }
            }
        });
    }

    /// Expansion sources in voxel coordinates: (from, to, radius, rate).
    fn sources(&self) -> Vec<(Vec3, Vec3, f32, f32)> {
        self.expansions
            .iter()
            .map(|e| {
                (
                    self.local(e.from),
                    self.local(e.to),
                    e.radius / self.cell,
                    e.rate,
                )
            })
            .collect()
    }

    fn advect(&mut self, p: &StepParams, r: &Region) {
        let sources = self.sources();
        let dims = self.dims;
        // Semi-Lagrangian advection doesn't conserve mass; measure what should be there after
        // the step (what's here minus what blows out of the open faces) and correct to it.
        // Everything with smoke is inside the active region (it is drawn around it), so only
        // the region is summed, advected and copied back.
        let before = region_density(&self.voxels, dims, r);
        let outflow = self.outflow(p.dt);
        let slice = (dims.x * dims.y) as usize;
        let k = p.dt / self.cell;
        let fade = (-p.dt / p.dissipation_secs.max(0.1)).exp();
        let cool = (-p.dt / p.cool_secs.max(0.05)).exp();
        let src = &self.voxels;
        let floor = &self.floor;
        let wind = &self.wind;
        let wd = self.wind_dims;
        let outside = |q: Vec3| Voxel {
            vel: wind_at(wind, wd, q - 0.5),
            ..Default::default()
        };
        par_chunks_mut(&mut self.scratch, slice, |z, chunk| {
            let z = z as u32;
            if z < r.lo.z || z >= r.hi.z {
                return;
            }
            for y in r.lo.y..r.hi.y {
                for x in r.act(dims, y, z) {
                    let out = &mut chunk[(x + y * dims.x) as usize];
                    if y < floor[(x + z * dims.x) as usize] as u32 {
                        *out = Voxel::default();
                        continue;
                    }
                    let i = (x + dims.x * (y + dims.y * z)) as usize;
                    let c = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                    // Midpoint (RK2) backtrace: follows curved flow better than one step.
                    let mid = c - src[i].vel * (k * 0.5);
                    let v_mid = sample_vel(src, dims, mid, &outside);
                    let back = c - v_mid * k;
                    let mut v = sample(src, dims, back, &outside);
                    // Expanding air thins the smoke in it (mass is conserved, volume grows).
                    let thin = if sources.is_empty() {
                        fade
                    } else {
                        fade * (-expansion_at(&sources, c) * p.dt).exp()
                    };
                    v.density *= thin;
                    v.tint *= thin;
                    // The new volume is mostly air drawn in from around: it cools the gas too.
                    v.heat *= cool * thin / fade;
                    if y == floor[(x + z * dims.x) as usize] as u32 {
                        v.vel.y = v.vel.y.max(0.0);
                    }
                    if v.density < 1e-8 {
                        v.density = 0.0;
                        v.tint = Vec3::ZERO;
                    }
                    *out = v;
                }
            }
        });
        copy_back(&mut self.voxels, &self.scratch, dims, r);
        let mut after = region_density(&self.voxels, dims, r);
        if after > 1e-12 {
            let fade = (-p.dt / p.dissipation_secs.max(0.1)).exp() as f64;
            let expected = ((before - outflow).max(0.0)) * fade;
            let k = (expected / after).clamp(0.7, 1.4) as f32;
            if (k - 1.0).abs() > 1e-4 {
                par_chunks_mut(&mut self.voxels, slice, |z, chunk| {
                    let z = z as u32;
                    if z < r.lo.z || z >= r.hi.z {
                        return;
                    }
                    for y in r.lo.y..r.hi.y {
                        let span = r.act(dims, y, z);
                        let row =
                            (span.start + y * dims.x) as usize..(span.end + y * dims.x) as usize;
                        for v in &mut chunk[row] {
                            v.density *= k;
                            v.tint *= k;
                        }
                    }
                });
                after *= k as f64;
            }
        }
        self.max_density = region_max_density(&self.voxels, dims, r);
        self.mass_kg = (after * (self.cell as f64).powi(3)) as f32;
    }

    /// Mix smoke and heat with the neighboring voxels (`k` of the difference per step).
    fn diffuse(&mut self, k: f32, r: &Region) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let src = &self.voxels;
        let floor = &self.floor;
        par_chunks_mut(&mut self.scratch, slice, |z, chunk| {
            let z = z as u32;
            if z < r.lo.z || z >= r.hi.z {
                return;
            }
            let base = (z * dims.x * dims.y) as usize;
            for y in r.lo.y..r.hi.y {
                for x in r.act(dims, y, z) {
                    let i = base + (x + y * dims.x) as usize;
                    let out = &mut chunk[(x + y * dims.x) as usize];
                    *out = src[i];
                    if y < floor[(x + z * dims.x) as usize] as u32 {
                        continue;
                    }
                    let here = src[i];
                    let mut sum_d = 0.0;
                    let mut sum_h = 0.0;
                    let mut sum_t = Vec3::ZERO;
                    let mut n = 0.0;
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                        // Nothing mixes into the ground; open faces see clean air.
                        if ny < 0
                            || (nx >= 0
                                && nz >= 0
                                && (nx as u32) < dims.x
                                && (nz as u32) < dims.z
                                && (ny as u32)
                                    < floor[(nx as u32 + nz as u32 * dims.x) as usize] as u32)
                        {
                            continue;
                        }
                        let v = if nx < 0
                            || nz < 0
                            || nx >= dims.x as i32
                            || nz >= dims.z as i32
                            || ny >= dims.y as i32
                        {
                            Voxel::default()
                        } else {
                            src[(nx as u32 + dims.x * (ny as u32 + dims.y * nz as u32)) as usize]
                        };
                        sum_d += v.density - here.density;
                        sum_h += v.heat - here.heat;
                        sum_t += v.tint - here.tint;
                        n += 1.0;
                    }
                    if n > 0.0 {
                        out.density = (here.density + k * sum_d).max(0.0);
                        out.heat = here.heat + k * sum_h;
                        out.tint = (here.tint + k * sum_t).max(Vec3::ZERO);
                    }
                }
            }
        });
        copy_back(&mut self.voxels, &self.scratch, dims, r);
    }

    /// Smoke (in voxel-density units) leaving through the open faces in `dt`.
    fn outflow(&self, dt: f32) -> f64 {
        let d = self.dims;
        let k = (dt / self.cell) as f64;
        let mut out = 0.0f64;
        let mut face = |x: u32, y: u32, z: u32, n: Vec3| {
            let v = self.voxels[self.index(x, y, z)];
            if v.density > 0.0 {
                out += v.density as f64 * (v.vel.dot(n).max(0.0) as f64 * k).min(1.0);
            }
        };
        for z in 0..d.z {
            for y in 0..d.y {
                face(0, y, z, Vec3::NEG_X);
                face(d.x - 1, y, z, Vec3::X);
            }
        }
        for y in 0..d.y {
            for x in 0..d.x {
                face(x, y, 0, Vec3::NEG_Z);
                face(x, y, d.z - 1, Vec3::Z);
            }
        }
        for z in 0..d.z {
            for x in 0..d.x {
                face(x, d.y - 1, z, Vec3::Y);
            }
        }
        out
    }

    /// Move the box into a new reference frame (see [`WorldShift`]).
    ///
    /// A translation moves `origin` (exactly; the voxels stay as they are). A rotation
    /// resamples the air (trilinear, velocities turned) into a box of the same size, still
    /// along the world axes, placed so that `anchor` (meters from the old min corner) is where
    /// the world took it, and marks the floor to be measured again. Either way `version` is
    /// bumped, so the renderer and the obscurance snapshot pick it up.
    pub fn shift(&mut self, s: &WorldShift, anchor: DVec3) {
        for e in &mut self.expansions {
            e.from = s.point(e.from);
            e.to = s.point(e.to);
        }
        for (c, _, _) in &mut self.inflow {
            *c = s.point(*c);
        }
        self.version += 1;
        if s.is_translation_only() {
            self.origin += s.translation;
            return;
        }
        let old_origin = self.origin;
        self.origin = s.point(old_origin + anchor) - anchor;
        let old = std::mem::take(&mut self.voxels);
        let old_floor = std::mem::take(&mut self.floor);
        let dims = self.dims;
        let cell = self.cell as f64;
        let new_origin = self.origin;
        // Continuous voxel coordinates in the old box of a voxel center of the new one.
        let back = |x: u32, y: u32, z: u32| {
            let p = new_origin + (UVec3::new(x, y, z).as_dvec3() + 0.5) * cell;
            ((s.inverse_point(p) - old_origin) / cell).as_vec3()
        };
        let mut voxels = vec![Voxel::default(); old.len()];
        let slice = (dims.x * dims.y) as usize;
        par_chunks_mut(&mut voxels, slice, |z, chunk| {
            for y in 0..dims.y {
                for x in 0..dims.x {
                    let mut v = sample(&old, dims, back(x, y, z as u32), &|_| Voxel::default());
                    v.vel = s.direction(v.vel);
                    chunk[(x + y * dims.x) as usize] = v;
                }
            }
        });
        self.voxels = voxels;
        // Until the ground is measured again: the old floor under each new column.
        self.floor = (0..dims.x * dims.z)
            .map(|i| {
                let q = back(i % dims.x, 0, i / dims.x);
                let c = q.floor().as_ivec3();
                let x = c.x.clamp(0, dims.x as i32 - 1) as u32;
                let z = c.z.clamp(0, dims.z as i32 - 1) as u32;
                old_floor[(x + z * dims.x) as usize]
            })
            .collect();
        self.floor_stale = true;
        self.resampled += 1;
        self.max_density = self.voxels.iter().map(|v| v.density).fold(0.0, f32::max);
        let total: f64 = self.voxels.iter().map(|v| v.density as f64).sum();
        self.mass_kg = (total * cell.powi(3)) as f32;
        self.measure();
    }

    /// Make this a copy of `other`'s air (same box), reusing this one's memory: what a step
    /// reads, not its scratch space.
    pub fn copy_from(&mut self, other: &Domain) {
        if self.dims != other.dims {
            *self = other.clone();
            return;
        }
        self.origin = other.origin;
        self.cell = other.cell;
        self.voxels.clone_from(&other.voxels);
        self.pressure.clone_from(&other.pressure);
        self.floor.clone_from(&other.floor);
        self.floor_stale = other.floor_stale;
        self.expansions.clone_from(&other.expansions);
        self.inflow.clone_from(&other.inflow);
        self.version = other.version;
        self.resampled = other.resampled;
        self.mass_kg = other.mass_kg;
        self.max_density = other.max_density;
        self.time = other.time;
        self.active = other.active;
        self.active_voxels = other.active_voxels;
        self.fog = other.fog;
    }

    /// Make a [`Change`].
    pub fn apply(&mut self, c: &Change) {
        match *c {
            Change::Splat {
                at,
                radius,
                mass_kg,
                heat_c,
                vel,
                albedo,
            } => self.splat(at, radius, mass_kg, heat_c, vel, albedo),
            Change::Inflate { at, radius, gas_m3 } => self.inflate(at, radius, gas_m3),
            Change::Carve {
                from,
                to,
                radius,
                clear,
                speed,
            } => self.carve(from, to, radius, clear, speed),
            Change::Blast {
                at,
                radius,
                strength,
            } => self.blast(at, radius, strength),
            Change::Obstacle { at, radius, vel } => self.obstacle(at, radius, vel),
            Change::Jet {
                from,
                dir,
                length,
                radius,
                speed,
                dt,
            } => self.jet(from, dir, length, radius, speed, dt),
            Change::Clear => self.clear(),
        }
    }

    /// Remove all smoke and stop the air.
    pub fn clear(&mut self) {
        self.voxels.iter_mut().for_each(|v| *v = Voxel::default());
        self.expansions.clear();
        self.inflow.clear();
        self.mass_kg = 0.0;
        self.max_density = 0.0;
        self.version += 1;
        // The contents jumped: the renderer must not blend from the old smoke.
        self.resampled += 1;
    }
}

/// Something done to a volume's air from outside its step (released smoke, a bullet, a
/// blast, a fan), kept so it can be done again to a step that was running meanwhile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    Splat {
        at: DVec3,
        radius: f32,
        mass_kg: f32,
        heat_c: f32,
        vel: Vec3,
        albedo: Vec3,
    },
    Inflate {
        at: DVec3,
        radius: f32,
        gas_m3: f32,
    },
    Carve {
        from: DVec3,
        to: DVec3,
        radius: f32,
        clear: f32,
        speed: f32,
    },
    Blast {
        at: DVec3,
        radius: f32,
        strength: f32,
    },
    Obstacle {
        at: DVec3,
        radius: f32,
        vel: Vec3,
    },
    Jet {
        from: DVec3,
        dir: Vec3,
        length: f32,
        radius: f32,
        speed: f32,
        dt: f32,
    },
    Clear,
}

/// Volume growth rate (1/s) of the expansion sources at voxel coordinates `c`.
fn expansion_at(sources: &[(Vec3, Vec3, f32, f32)], c: Vec3) -> f32 {
    let mut s = 0.0;
    for (a, b, r, rate) in sources {
        let ab = *b - *a;
        let t = ((c - *a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        let dist = (c - (*a + ab * t)).length();
        if dist < *r {
            s += rate * (1.0 - dist / r);
        }
    }
    s
}

#[inline]
fn clamped_index(dims: UVec3, x: i32, y: i32, z: i32) -> usize {
    let x = x.clamp(0, dims.x as i32 - 1) as u32;
    let y = y.clamp(0, dims.y as i32 - 1) as u32;
    let z = z.clamp(0, dims.z as i32 - 1) as u32;
    (x + dims.x * (y + dims.y * z)) as usize
}

/// Coarse wind at continuous voxel coordinates (voxel corners, not centers).
fn wind_at(wind: &[Vec3], wd: UVec3, q: Vec3) -> Vec3 {
    let g = (q / WIND_STEP as f32).clamp(Vec3::ZERO, (wd - 1).as_vec3());
    let i = g.floor().as_uvec3().min(wd - 2);
    let f = g - i.as_vec3();
    let at = |x: u32, y: u32, z: u32| wind[(x + wd.x * (y + wd.y * z)) as usize];
    let x00 = at(i.x, i.y, i.z).lerp(at(i.x + 1, i.y, i.z), f.x);
    let x10 = at(i.x, i.y + 1, i.z).lerp(at(i.x + 1, i.y + 1, i.z), f.x);
    let x01 = at(i.x, i.y, i.z + 1).lerp(at(i.x + 1, i.y, i.z + 1), f.x);
    let x11 = at(i.x, i.y + 1, i.z + 1).lerp(at(i.x + 1, i.y + 1, i.z + 1), f.x);
    x00.lerp(x10, f.y).lerp(x01.lerp(x11, f.y), f.z)
}

/// Trilinear sample of voxel data at continuous coordinates (centers at i + 0.5). Points
/// outside the box blend toward `outside(q)`.
/// The part of segment a–b inside the box lo–hi, as parameters 0..1.
pub(crate) fn clip_box(lo: DVec3, hi: DVec3, a: DVec3, b: DVec3) -> Option<(f64, f64)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..3 {
        if d[k].abs() < 1e-12 {
            if a[k] < lo[k] || a[k] > hi[k] {
                return None;
            }
            continue;
        }
        let ta = (lo[k] - a[k]) / d[k];
        let tb = (hi[k] - a[k]) / d[k];
        t0 = t0.max(ta.min(tb));
        t1 = t1.min(ta.max(tb));
    }
    (t0 < t1).then_some((t0, t1))
}

/// [`sample`]'s velocity alone (the advection's midpoint needs only that).
fn sample_vel(data: &[Voxel], dims: UVec3, q: Vec3, outside: &dyn Fn(Vec3) -> Voxel) -> Vec3 {
    let g = q - 0.5;
    let i = g.floor();
    let f = g - i;
    let i = i.as_ivec3();
    let mut acc = Vec3::ZERO;
    for corner in 0..8 {
        let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
        let w = (if dx == 1 { f.x } else { 1.0 - f.x })
            * (if dy == 1 { f.y } else { 1.0 - f.y })
            * (if dz == 1 { f.z } else { 1.0 - f.z });
        if w <= 0.0 {
            continue;
        }
        let (x, y, z) = (i.x + dx, i.y + dy, i.z + dz);
        let v = if x < 0 || z < 0 || x >= dims.x as i32 || y >= dims.y as i32 || z >= dims.z as i32
        {
            outside(q).vel
        } else if y < 0 {
            let v = data[clamped_index(dims, x, 0, z)].vel;
            Vec3::new(v.x, 0.0, v.z)
        } else {
            data[(x as u32 + dims.x * (y as u32 + dims.y * z as u32)) as usize].vel
        };
        acc += v * w;
    }
    acc
}

fn sample(data: &[Voxel], dims: UVec3, q: Vec3, outside: &dyn Fn(Vec3) -> Voxel) -> Voxel {
    let g = q - 0.5;
    let i = g.floor();
    let f = g - i;
    let i = i.as_ivec3();
    let mut acc = Voxel::default();
    for corner in 0..8 {
        let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
        let w = (if dx == 1 { f.x } else { 1.0 - f.x })
            * (if dy == 1 { f.y } else { 1.0 - f.y })
            * (if dz == 1 { f.z } else { 1.0 - f.z });
        if w <= 0.0 {
            continue;
        }
        let (x, y, z) = (i.x + dx, i.y + dy, i.z + dz);
        let v = if x < 0 || z < 0 || x >= dims.x as i32 || y >= dims.y as i32 || z >= dims.z as i32
        {
            outside(q)
        } else if y < 0 {
            // Under the box: like the bottom layer, but with no flow into the ground.
            let mut v = data[clamped_index(dims, x, 0, z)];
            v.vel.y = 0.0;
            v
        } else {
            data[(x as u32 + dims.x * (y as u32 + dims.y * z as u32)) as usize]
        };
        acc = acc.add(v.scaled(w));
    }
    acc
}

/// Where a step works: each row's stretch of x (rows by `y + z * dims.y`; an empty row holds
/// (0, 0)) and the box around the wide stretches.
struct Region {
    /// Room for the smoke to move and for the flow around it.
    act: Vec<(u32, u32)>,
    /// `act` and a voxel more each way: forces and curl, which the rest reads around it.
    wide: Vec<(u32, u32)>,
    lo: UVec3,
    hi: UVec3,
}

impl Region {
    fn act(&self, dims: UVec3, y: u32, z: u32) -> std::ops::Range<u32> {
        let (a, b) = self.act[(y + z * dims.y) as usize];
        a..b
    }

    fn wide(&self, dims: UVec3, y: u32, z: u32) -> std::ops::Range<u32> {
        let (a, b) = self.wide[(y + z * dims.y) as usize];
        a..b
    }

    fn voxels(&self, dims: UVec3) -> u32 {
        (self.act.iter().map(|(a, b)| b - a).sum::<u32>()) * (dims.x > 0) as u32
    }
}

/// Both stretches as one (empty ones don't count).
fn join(a: (u32, u32), b: (u32, u32)) -> (u32, u32) {
    if a.0 >= a.1 {
        b
    } else if b.0 >= b.1 {
        a
    } else {
        (a.0.min(b.0), a.1.max(b.1))
    }
}

/// Each row's stretch, grown by `m` voxels every way: over the rows within `m` (up and down,
/// then along z), then out along the row.
fn dilate(raw: &[(u32, u32)], d: UVec3, m: u32) -> Vec<(u32, u32)> {
    let (ny, nz, m) = (d.y as usize, d.z as usize, m as usize);
    let mut along_y = vec![(0, 0); raw.len()];
    for z in 0..nz {
        for y in 0..ny {
            along_y[y + z * ny] = (y.saturating_sub(m)..(y + m + 1).min(ny))
                .fold((0, 0), |s, yy| join(s, raw[yy + z * ny]));
        }
    }
    let mut out = vec![(0, 0); raw.len()];
    for z in 0..nz {
        for y in 0..ny {
            let (a, b) = (z.saturating_sub(m)..(z + m + 1).min(nz))
                .fold((0, 0), |s, zz| join(s, along_y[y + zz * ny]));
            if a < b {
                out[y + z * ny] = (a.saturating_sub(m as u32), (b + m as u32).min(d.x));
            }
        }
    }
    out
}

/// Copy the region's stretches from `from` to `to`.
fn copy_back(to: &mut [Voxel], from: &[Voxel], dims: UVec3, r: &Region) {
    let slice = (dims.x * dims.y) as usize;
    par_chunks_mut(to, slice, |z, chunk| {
        let z = z as u32;
        if z < r.lo.z || z >= r.hi.z {
            return;
        }
        let base = (z * dims.x * dims.y) as usize;
        for y in r.lo.y..r.hi.y {
            let span = r.act(dims, y, z);
            let row = (span.start + y * dims.x) as usize..(span.end + y * dims.x) as usize;
            chunk[row.clone()].copy_from_slice(&from[base + row.start..base + row.end]);
        }
    });
}

/// Smoke (voxel-density units) in the region.
fn region_density(voxels: &[Voxel], dims: UVec3, r: &Region) -> f64 {
    let mut sum = 0.0;
    for z in r.lo.z..r.hi.z {
        for y in r.lo.y..r.hi.y {
            let span = r.act(dims, y, z);
            let start = (span.start + dims.x * (y + dims.y * z)) as usize;
            let end = start + span.len();
            sum += voxels[start..end]
                .iter()
                .map(|v| v.density as f64)
                .sum::<f64>();
        }
    }
    sum
}

/// Thickest density in the region.
fn region_max_density(voxels: &[Voxel], dims: UVec3, r: &Region) -> f32 {
    let mut max = 0.0f32;
    for z in r.lo.z..r.hi.z {
        for y in r.lo.y..r.hi.y {
            let span = r.act(dims, y, z);
            let start = (span.start + dims.x * (y + dims.y * z)) as usize;
            let end = start + span.len();
            max = voxels[start..end]
                .iter()
                .map(|v| v.density)
                .fold(max, f32::max);
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a step of the meadow's volume takes with a plume in it (run by hand:
    /// `cargo test --release -p engine_smoke --lib -- --ignored --nocapture step_cost`).
    #[test]
    #[ignore]
    fn step_cost() {
        engine_core::tasks::init_task_pools();
        let mut d = Domain::new(DVec3::new(-50.0, -1.0, -30.0), 1.25, UVec3::new(80, 32, 48));
        d.set_floor(|x, _| Some(x * 0.05));
        let p = StepParams {
            dt: 0.05,
            ..Default::default()
        };
        // A breeze across the box's diagonal draws the plume out long and slanted, as the
        // meadow's wind does.
        let calm = |_: DVec3| Vec3::new(4.0, 0.0, 2.5);
        for i in 0..150 {
            let x = -20.0 + (i % 8) as f64 * 3.0;
            d.splat(
                DVec3::new(x, 2.0, 0.0),
                2.0,
                0.3,
                300.0,
                Vec3::Y * 3.0,
                Vec3::ONE,
            );
            d.step(&p, &calm);
        }
        // Each phase's share, then whole steps.
        let mut t = [0.0f64; 5];
        for _ in 0..20 {
            let c = std::time::Instant::now();
            let r2 = d.measure();
            t[0] += c.elapsed().as_secs_f64();
            let c = std::time::Instant::now();
            d.apply_forces(&p, &r2);
            d.confine_vorticity(p.vorticity, 0.05, &r2);
            t[1] += c.elapsed().as_secs_f64();
            let c = std::time::Instant::now();
            d.project(p.iterations, &r2);
            t[2] += c.elapsed().as_secs_f64();
            let c = std::time::Instant::now();
            d.advect(&p, &r2);
            t[3] += c.elapsed().as_secs_f64();
            let c = std::time::Instant::now();
            d.diffuse(0.05, &r2);
            t[4] += c.elapsed().as_secs_f64();
        }
        println!(
            "phases ms (region, forces, pressure, advect, diffuse): {:?}",
            t.map(|x| (x * 50.0 * 100.0).round() / 100.0)
        );
        let started = std::time::Instant::now();
        for _ in 0..20 {
            d.splat(
                DVec3::new(-10.0, 2.0, 0.0),
                2.0,
                0.3,
                300.0,
                Vec3::Y * 3.0,
                Vec3::ONE,
            );
            d.step(&p, &calm);
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0 / 20.0;
        let (lo, hi) = d.active;
        println!(
            "step: {ms:.2} ms, active {:?} ({} voxels, {} in the box), mass {:.2} kg",
            d.active,
            d.active_voxels,
            (hi - lo).element_product(),
            d.mass_kg
        );
    }

    fn pools() {
        engine_core::tasks::init_task_pools();
    }

    fn domain() -> Domain {
        let mut d = Domain::new(DVec3::new(-8.0, 0.0, -8.0), 0.5, UVec3::new(32, 32, 32));
        d.set_floor(|_, _| Some(0.0));
        d
    }

    fn params() -> StepParams {
        StepParams {
            dt: 0.05,
            ..Default::default()
        }
    }

    fn centroid(d: &Domain) -> DVec3 {
        let mut c = DVec3::ZERO;
        let mut m = 0.0;
        for z in 0..d.dims.z {
            for y in 0..d.dims.y {
                for x in 0..d.dims.x {
                    let v = d.voxels[d.index(x, y, z)];
                    c += d.voxel_center(x, y, z) * v.density as f64;
                    m += v.density as f64;
                }
            }
        }
        c / m.max(1e-12)
    }

    #[test]
    fn splat_conserves_mass() {
        pools();
        let mut d = domain();
        d.splat(
            DVec3::new(0.0, 3.0, 0.0),
            1.0,
            0.5,
            0.0,
            Vec3::ZERO,
            Vec3::splat(0.8),
        );
        d.step(&params(), &|_| Vec3::ZERO);
        assert!((d.mass_kg - 0.5).abs() < 0.02, "{}", d.mass_kg);
        assert!((d.sample(DVec3::new(0.0, 3.0, 0.0)).albedo() - Vec3::splat(0.8)).length() < 0.01);
    }

    #[test]
    fn hot_smoke_rises_and_wind_carries_it() {
        pools();
        let mut d = domain();
        let source = DVec3::new(-4.0, 1.0, 0.0);
        for _ in 0..60 {
            d.splat(source, 0.8, 0.01, 400.0, Vec3::Y * 2.0, Vec3::splat(0.5));
            d.step(&params(), &|_| Vec3::new(2.0, 0.0, 0.0));
        }
        let c = centroid(&d);
        assert!(c.y > source.y + 2.0, "a hot plume should rise: {c}");
        assert!(c.x > source.x + 1.5, "wind should bend it downwind: {c}");
        // Smoke reached well above the source.
        let high = (0..d.dims.x).any(|x| d.voxels[d.index(x, 12, d.dims.z / 2)].density > 1e-4);
        assert!(high, "no smoke 6 m up");
    }

    #[test]
    fn smoke_stays_above_the_ground() {
        pools();
        let mut d = domain();
        d.splat(
            DVec3::new(0.0, 1.0, 0.0),
            1.0,
            0.5,
            0.0,
            Vec3::new(0.0, -5.0, 0.0),
            Vec3::ONE,
        );
        for _ in 0..40 {
            d.step(&params(), &|_| Vec3::ZERO);
        }
        for z in 0..d.dims.z {
            for x in 0..d.dims.x {
                // Floor at y = 0: no layer is solid, but no velocity points into the ground
                // in the bottom layer and the smoke is all still there.
                assert!(d.voxels[d.index(x, 0, z)].vel.y >= -1e-3);
            }
        }
        assert!(d.mass_kg > 0.4, "{}", d.mass_kg);
    }

    #[test]
    fn a_bullet_clears_a_tunnel_and_a_blast_a_hole() {
        pools();
        let mut d = domain();
        // A cloud of cold smoke.
        for i in 0..5 {
            d.splat(
                DVec3::new(-2.0 + i as f64, 3.0, 0.0),
                2.5,
                0.3,
                0.0,
                Vec3::ZERO,
                Vec3::ONE,
            );
        }
        let on_path = DVec3::new(0.0, 3.0, 0.0);
        let beside = DVec3::new(0.0, 3.0, 2.0);
        let (before, side_before) = (d.sample(on_path).density, d.sample(beside).density);
        let ray = (DVec3::new(-8.0, 3.0, 0.0), DVec3::new(8.0, 3.0, 0.0));
        let t_before = d.transmittance(ray.0, ray.1, 3000.0);
        d.carve(ray.0, ray.1, 0.02, 0.95, 800.0);
        let after = d.sample(on_path).density;
        assert!(
            after < before * 0.1,
            "bullet path should clear: {before} -> {after}"
        );
        assert!(
            d.sample(beside).density > side_before * 0.9,
            "smoke beside the path stays"
        );
        assert!(d.transmittance(ray.0, ray.1, 3000.0) > t_before);
        // The wake keeps pushing smoke out of the tunnel for a moment.
        for _ in 0..3 {
            d.step(&params(), &|_| Vec3::ZERO);
        }
        assert!(d.sample(on_path).density < before * 0.3);

        let dense = d.sample(beside).density;
        d.blast(beside, 2.0, 30.0);
        for _ in 0..6 {
            d.step(&params(), &|_| Vec3::ZERO);
        }
        let hole = d.sample(beside).density;
        assert!(
            hole < dense * 0.5,
            "blast should clear its center: {dense} -> {hole}"
        );
    }

    #[test]
    fn transmittance_through_clear_air_is_one() {
        let d = domain();
        assert_eq!(
            d.transmittance(
                DVec3::new(-20.0, 1.0, 0.0),
                DVec3::new(20.0, 1.0, 0.0),
                3000.0
            ),
            1.0
        );
        assert_eq!(
            d.transmittance(
                DVec3::new(-20.0, 50.0, 0.0),
                DVec3::new(20.0, 50.0, 0.0),
                3000.0
            ),
            1.0
        );
    }
}
