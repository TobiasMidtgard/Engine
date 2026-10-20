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
            iterations: 14,
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
    pub fn set_floor(&mut self, ground: impl Fn(f64, f64) -> Option<f64>) {
        for z in 0..self.dims.z {
            for x in 0..self.dims.x {
                let c = self.voxel_center(x, 0, z);
                let layers = ground(c.x, c.z)
                    .map(|h| ((h - self.origin.y) / self.cell as f64).round().max(0.0))
                    .unwrap_or(0.0)
                    .min(self.dims.y as f64) as u16;
                self.floor[(x + z * self.dims.x) as usize] = layers;
            }
        }
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
        // Only the air around the smoke is worth solving; the rest just follows the wind.
        self.active = self.active_region();
        let (lo, hi) = self.active;
        self.sample_wind(wind);
        self.apply_forces(p);
        if p.vorticity > 0.0 {
            self.confine_vorticity(p.vorticity, dt, lo, hi);
        }
        self.project(p.iterations, lo, hi);
        self.advect(p, lo, hi);
        // Explicit diffusion is stable up to 1/6 per step.
        let mix = (p.diffusion_m2_s * dt / (self.cell * self.cell)).min(0.16);
        if mix > 1e-4 {
            self.diffuse(mix, lo, hi);
        }
        for e in &mut self.expansions {
            e.life -= dt;
        }
        self.expansions.retain(|e| e.life > 0.0);
        self.version += 1;
    }

    /// The box around everything with smoke, heat or expanding air, plus room for it to
    /// move and for the flow around it.
    fn active_region(&self) -> (UVec3, UVec3) {
        let d = self.dims;
        let mut lo = d;
        let mut hi = UVec3::ZERO;
        for z in 0..d.z {
            for y in 0..d.y {
                let row = ((d.x * (y + d.y * z)) as usize)..((d.x * (y + 1 + d.y * z)) as usize);
                let mut first = None;
                let mut last = 0;
                for (x, v) in self.voxels[row].iter().enumerate() {
                    if v.density > 1e-7 || v.heat.abs() > 0.05 {
                        first.get_or_insert(x as u32);
                        last = x as u32;
                    }
                }
                if let Some(first) = first {
                    lo = lo.min(UVec3::new(first, y, z));
                    hi = hi.max(UVec3::new(last + 1, y + 1, z + 1));
                }
            }
        }
        for e in &self.expansions {
            let (a, b) = (self.local(e.from), self.local(e.to));
            let r = e.radius / self.cell;
            let emin = (a.min(b) - r).floor().max(Vec3::ZERO).as_uvec3();
            let emax = (a.max(b) + r).ceil().max(Vec3::ZERO).as_uvec3();
            lo = lo.min(emin);
            hi = hi.max(emax);
        }
        if hi.x <= lo.x {
            return (UVec3::ZERO, UVec3::ZERO);
        }
        let m = ((3.0 / self.cell).ceil() as u32).max(4);
        let lo = UVec3::new(
            lo.x.saturating_sub(m),
            lo.y.saturating_sub(m),
            lo.z.saturating_sub(m),
        );
        (lo, (hi + m).min(d))
    }

    fn sample_wind(&mut self, wind: &(dyn Fn(DVec3) -> Vec3 + Sync)) {
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

    fn apply_forces(&mut self, p: &StepParams) {
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
            for y in 0..dims.y {
                for x in 0..dims.x {
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

    fn confine_vorticity(&mut self, strength: f32, dt: f32, lo: UVec3, hi: UVec3) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let h = self.cell;
        // Curl one voxel beyond the region, so the confinement inside it has neighbors.
        let clo = UVec3::new(
            lo.x.saturating_sub(1),
            lo.y.saturating_sub(1),
            lo.z.saturating_sub(1),
        );
        let chi = (hi + 1).min(dims);
        {
            let voxels = &self.voxels;
            par_chunks_mut(&mut self.curl, slice, |z, chunk| {
                let z = z as u32;
                if z < clo.z || z >= chi.z {
                    return;
                }
                for y in clo.y..chi.y {
                    for x in clo.x..chi.x {
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
            if z < lo.z || z >= hi.z {
                return;
            }
            let inside = |x: i32, y: i32, z: i32| {
                let c = UVec3::new(
                    x.clamp(clo.x as i32, chi.x as i32 - 1) as u32,
                    y.clamp(clo.y as i32, chi.y as i32 - 1) as u32,
                    z.clamp(clo.z as i32, chi.z as i32 - 1) as u32,
                );
                (c.x + dims.x * (c.y + dims.y * c.z)) as usize
            };
            for y in lo.y..hi.y {
                for x in lo.x..hi.x {
                    if y < floor[(x + z * dims.x) as usize] as u32 {
                        continue;
                    }
                    let m = |dx: i32, dy: i32, dz: i32| {
                        curl[inside(x as i32 + dx, y as i32 + dy, z as i32 + dz)].length()
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

    fn project(&mut self, iterations: u32, lo: UVec3, hi: UVec3) {
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
                if z < lo.z || z >= hi.z {
                    return;
                }
                for y in lo.y..hi.y {
                    for x in lo.x..hi.x {
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
        self.pressure.iter_mut().for_each(|p| *p = 0.0);
        self.pressure_next.iter_mut().for_each(|p| *p = 0.0);
        for _ in 0..iterations {
            {
                let p = &self.pressure;
                let div = &self.divergence;
                let floor = &self.floor;
                par_chunks_mut(&mut self.pressure_next, slice, |z, chunk| {
                    let z = z as u32;
                    if z < lo.z || z >= hi.z {
                        return;
                    }
                    for y in lo.y..hi.y {
                        for x in lo.x..hi.x {
                            let i = (x + dims.x * (y + dims.y * z)) as usize;
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
            if z < lo.z || z >= hi.z {
                return;
            }
            for y in lo.y..hi.y {
                for x in lo.x..hi.x {
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

    fn advect(&mut self, p: &StepParams, lo: UVec3, hi: UVec3) {
        let sources = self.sources();
        let dims = self.dims;
        // Semi-Lagrangian advection doesn't conserve mass; measure what should be there after
        // the step (what's here minus what blows out of the open faces) and correct to it.
        let before: f64 = self.voxels.iter().map(|v| v.density as f64).sum();
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
            let base = (z * dims.x * dims.y) as usize;
            if z < lo.z || z >= hi.z {
                chunk.copy_from_slice(&src[base..base + chunk.len()]);
                return;
            }
            for y in 0..dims.y {
                for x in 0..dims.x {
                    let out = &mut chunk[(x + y * dims.x) as usize];
                    if y < lo.y || y >= hi.y || x < lo.x || x >= hi.x {
                        *out = src[base + (x + y * dims.x) as usize];
                        continue;
                    }
                    if y < floor[(x + z * dims.x) as usize] as u32 {
                        *out = Voxel::default();
                        continue;
                    }
                    let i = (x + dims.x * (y + dims.y * z)) as usize;
                    let c = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                    // Midpoint (RK2) backtrace: follows curved flow better than one step.
                    let mid = c - src[i].vel * (k * 0.5);
                    let v_mid = sample(src, dims, mid, &outside).vel;
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
        std::mem::swap(&mut self.voxels, &mut self.scratch);
        let mut after: f64 = self.voxels.iter().map(|v| v.density as f64).sum();
        if after > 1e-12 {
            let fade = (-p.dt / p.dissipation_secs.max(0.1)).exp() as f64;
            let expected = ((before - outflow).max(0.0)) * fade;
            let k = (expected / after).clamp(0.7, 1.4) as f32;
            if (k - 1.0).abs() > 1e-4 {
                par_chunks_mut(&mut self.voxels, slice, |_, chunk| {
                    for v in chunk {
                        v.density *= k;
                        v.tint *= k;
                    }
                });
                after *= k as f64;
            }
        }
        self.max_density = self.voxels.iter().map(|v| v.density).fold(0.0, f32::max);
        self.mass_kg = (after * (self.cell as f64).powi(3)) as f32;
    }

    /// Mix smoke and heat with the neighboring voxels (`k` of the difference per step).
    fn diffuse(&mut self, k: f32, lo: UVec3, hi: UVec3) {
        let dims = self.dims;
        let slice = (dims.x * dims.y) as usize;
        let src = &self.voxels;
        let floor = &self.floor;
        par_chunks_mut(&mut self.scratch, slice, |z, chunk| {
            let z = z as u32;
            let base = (z * dims.x * dims.y) as usize;
            if z < lo.z || z >= hi.z {
                chunk.copy_from_slice(&src[base..base + chunk.len()]);
                return;
            }
            for y in 0..dims.y {
                for x in 0..dims.x {
                    let i = base + (x + y * dims.x) as usize;
                    let out = &mut chunk[(x + y * dims.x) as usize];
                    *out = src[i];
                    if y < lo.y || y >= hi.y || x < lo.x || x >= hi.x {
                        continue;
                    }
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
        std::mem::swap(&mut self.voxels, &mut self.scratch);
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
        self.active = self.active_region();
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

#[cfg(test)]
mod tests {
    use super::*;

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
