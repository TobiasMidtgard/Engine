//! The live water state shared by the simulation, the `Water` query and the renderer.

use std::sync::Arc;

use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_world::{Ground, WaterQuery, WaterSample, WorldShift};
use parking_lot::RwLock;

use crate::body::{WaterBed, WaterBody};
use crate::flow::FlowSim;
use crate::ripples::RippleField;
use crate::waves::{WavePoint, WaveSet};

/// One water body as simulated this step.
#[derive(Debug, Clone)]
pub struct BodyState {
    pub entity: Entity,
    /// World center; `y` is the still surface level.
    pub center: DVec3,
    /// Yaw of the extent, radians.
    pub yaw: f32,
    pub body: WaterBody,
    pub waves: WaveSet,
    /// Typical depth used for wave dispersion, meters.
    pub mean_depth: f32,
    /// Wind over the body at 10 m, m/s.
    pub wind: Vec3,
    /// Its ground bed read once into a grid ([`BedCache`]), for `WaterBed::Ground` bodies:
    /// `None` until it is made (the ground is asked directly meanwhile).
    pub bed: Option<BedRef>,
}

/// A body's [`BedCache`] and the place it was read for (which follows world shifts).
#[derive(Debug, Clone)]
pub struct BedRef {
    pub center: DVec3,
    pub yaw: f32,
    pub grid: Arc<BedCache>,
    /// Counts the reads of it that came back different (a read again after the ground provider
    /// changed that found the same bed keeps it, so what depends on the bed isn't rebuilt).
    pub rev: u32,
}

impl BodyState {
    /// Body-local horizontal offset of a world point (rotated into the extent's frame).
    pub fn local(&self, p: DVec3) -> Vec2 {
        let d = DVec2::new(p.x - self.center.x, p.z - self.center.z).as_vec2();
        let (s, c) = self.yaw.sin_cos();
        Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y)
    }

    /// Offset from the wave reference point (the center), unrotated (waves live in world axes).
    pub fn wave_coords(&self, p: DVec3) -> Vec2 {
        DVec2::new(p.x - self.center.x, p.z - self.center.z).as_vec2()
    }

    /// Follow a world shift: the body moves and turns with the world, its waves keep their
    /// phase and turn with it, so the surface is the same where it was.
    pub fn shift(&mut self, s: &WorldShift) {
        // The bed grid lies in the body's own frame, which a move or a turn about the vertical
        // carries along; a tilt of the world's frame puts the ground at other heights.
        if !keeps_level(s) {
            self.bed = None;
        }
        if let Some(b) = &mut self.bed {
            b.center = s.point(b.center);
            b.yaw = crate::flow::turned_yaw(s, b.yaw);
        }
        self.center = s.point(self.center);
        self.yaw = crate::flow::turned_yaw(s, self.yaw);
        self.wind = s.direction(self.wind);
        for w in &mut self.waves.waves {
            let d = s.direction(Vec3::new(w.dir.x, 0.0, w.dir.y));
            w.dir = Vec2::new(d.x, d.z).normalize_or(w.dir);
        }
    }

    /// A ground-bedded lake whose bed grid ([`BedCache`]) is still being read.
    pub fn bed_pending(&self) -> bool {
        matches!(self.body.bed, WaterBed::Ground)
            && self.body.extent.bounding_radius().is_finite()
            && self.bed.is_none()
    }

    pub fn contains(&self, p: DVec3) -> bool {
        self.body.extent.contains_local(self.local(p))
    }

    /// Still-water depth at world x/z, meters (≤ 0 = dry). `None` outside the extent.
    pub fn depth_at(&self, p: DVec3, ground: &Ground) -> Option<f32> {
        let local = self.local(p);
        if !self.body.extent.contains_local(local) {
            return None;
        }
        Some(match self.body.bed {
            WaterBed::Flat { depth_m } => depth_m,
            WaterBed::Bowl { depth_m, shore_m } => {
                let inset = self.body.extent.inset(local);
                let t = (inset / shore_m.max(0.01)).clamp(0.0, 1.0);
                depth_m * t * t * (3.0 - 2.0 * t)
            }
            WaterBed::Ground => {
                // The grid holds the ground over the surface it was read for: a lake risen
                // since (a flood, `Freshet::lake_rise_m`) stands that much deeper on it.
                if let Some(b) = self.bed.as_ref()
                    && let Some(rel) = b.grid.at(local)
                {
                    return Some((self.center.y - b.center.y) as f32 - rel);
                }
                let probe = DVec3::new(p.x, self.center.y + 0.05, p.z);
                match ground.height(probe) {
                    Some(h) => (self.center.y - h) as f32,
                    None => self.mean_depth,
                }
            }
        })
    }
}

/// A lake's ground bed read once into a grid in the body's own frame, as Crest bakes its sea
/// floor depth cache once from the scene instead of asking the terrain every frame: the
/// depth under a point is then a lookup (about 0.05 µs) instead of a terrain sample (1-14 µs
/// with the showcase's landscape), and every reader of the water asks for it many times a
/// step (ripple grid reads, buoyancy probes, boat lift, shore wetness, reservoirs).
#[derive(Debug, Clone)]
pub struct BedCache {
    /// Half the side of the square it covers round the body's center, m (local frame).
    pub half: f32,
    /// Spacing of its points, m.
    pub cell: f32,
    /// Points per side.
    pub n: usize,
    /// Ground height over the still surface at each point, m (negative under water); NaN where
    /// the ground didn't answer.
    pub rel: Vec<f32>,
}

/// Whether a world shift keeps the vertical where it was (a move, or a turn about it).
pub fn keeps_level(s: &WorldShift) -> bool {
    (s.rotation * DVec3::Y).y > 1.0 - 1e-9
}

/// Bed grids are never finer than this, m.
pub const BED_CELL_M: f32 = 0.5;
/// Nor more points per side than this (a 512² grid is 1 MB).
pub const BED_POINTS: usize = 512;

impl BedCache {
    /// Read the ground under a body with its still surface at `center`, turned by `yaw`, over
    /// `half` m round it (goes wide on the pool it is called from).
    pub fn build(center: DVec3, yaw: f32, half: f32, ground: &Ground) -> Self {
        let cell = (2.0 * half / (BED_POINTS - 1) as f32).max(BED_CELL_M);
        let n = ((2.0 * half / cell).ceil() as usize + 1).max(2);
        let mut rel = vec![f32::NAN; n * n];
        let (s, c) = yaw.sin_cos();
        engine_core::tasks::init_task_pools();
        engine_core::tasks::par_chunks_mut(&mut rel, n * 8, |k, chunk| {
            for (o, r) in chunk.iter_mut().enumerate() {
                let idx = k * n * 8 + o;
                let (lx, lz) = (
                    -half + (idx % n) as f32 * cell,
                    -half + (idx / n) as f32 * cell,
                );
                // The inverse of `BodyState::local`.
                let (dx, dz) = (c * lx + s * lz, -s * lx + c * lz);
                let probe = DVec3::new(center.x + dx as f64, center.y + 0.05, center.z + dz as f64);
                *r = ground
                    .height(probe)
                    .map_or(f32::NAN, |h| (h - center.y) as f32);
            }
        });
        Self { half, cell, n, rel }
    }

    /// Ground over the still surface at body-local `local`, bilinear; `None` outside the grid
    /// or next to a point the ground didn't answer.
    pub fn at(&self, local: Vec2) -> Option<f32> {
        let g = (local + Vec2::splat(self.half)) / self.cell;
        let last = (self.n - 1) as f32;
        if !(g.x >= 0.0 && g.y >= 0.0 && g.x <= last && g.y <= last) {
            return None;
        }
        let (i, j) = (
            (g.x as usize).min(self.n - 2),
            (g.y as usize).min(self.n - 2),
        );
        let (fx, fz) = (g.x - i as f32, g.y - j as f32);
        let k = j * self.n + i;
        let r = &self.rel;
        let a = r[k] + (r[k + 1] - r[k]) * fx;
        let b = r[k + self.n] + (r[k + self.n + 1] - r[k + self.n]) * fx;
        let v = a + (b - a) * fz;
        (!v.is_nan()).then_some(v)
    }

    /// Read the ground again at the points within `radius` m of body-local `local` (a terrain
    /// edit).
    pub fn reread(&mut self, center: DVec3, yaw: f32, local: Vec2, radius: f32, ground: &Ground) {
        let (s, c) = yaw.sin_cos();
        let lo = ((local - Vec2::splat(radius) + Vec2::splat(self.half)) / self.cell)
            .floor()
            .max(Vec2::ZERO);
        let hi = ((local + Vec2::splat(radius) + Vec2::splat(self.half)) / self.cell)
            .ceil()
            .min(Vec2::splat((self.n - 1) as f32));
        for j in lo.y as usize..=hi.y as usize {
            for i in lo.x as usize..=hi.x as usize {
                let (lx, lz) = (
                    -self.half + i as f32 * self.cell,
                    -self.half + j as f32 * self.cell,
                );
                let (dx, dz) = (c * lx + s * lz, -s * lx + c * lz);
                let probe = DVec3::new(center.x + dx as f64, center.y + 0.05, center.z + dz as f64);
                self.rel[j * self.n + i] = ground
                    .height(probe)
                    .map_or(f32::NAN, |h| (h - center.y) as f32);
            }
        }
    }
}

/// Everything the water module knows this step.
pub struct WaterState {
    pub bodies: Vec<BodyState>,
    pub ripples: RippleField,
    pub flows: Vec<FlowSim>,
    /// The terrain provider at the last step (for `WaterBed::Ground`).
    pub ground: Ground,
    /// World seconds simulated.
    pub time: f64,
    /// World shifts followed so far (anything caching world positions rebuilds when it changes).
    pub shifts: u64,
    /// Moving interactors pushing the surface this step (x/z, radius). The dip a hull digs
    /// under itself drives its wake but isn't what it floats on: a boat would settle into its
    /// own trough, so the ripples are left out of [`WaterState::sample`] under each one.
    pub hulls: Vec<Hull>,
    /// Ground drawn by meshes the world's ground query doesn't know (`swash::BeachGround`),
    /// at their places.
    pub beaches: Vec<(DVec3, crate::swash::BeachGround)>,
}

/// A moving interactor's own dip ([`WaterState::hulls`]): round, or a capsule along a long
/// hull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hull {
    /// World x/z.
    pub center: DVec2,
    /// From the center to the end of the capsule's spine, meters (zero when round).
    pub half: DVec2,
    /// Meters.
    pub radius: f32,
}

impl WaterState {
    pub fn new(ripple_cells: usize, ripple_cell: f32) -> Self {
        Self {
            bodies: Vec::new(),
            ripples: RippleField::new(ripple_cells, ripple_cell),
            flows: Vec::new(),
            ground: Ground::default(),
            time: 0.0,
            shifts: 0,
            hulls: Vec::new(),
            beaches: Vec::new(),
        }
    }

    /// Move everything into the frame after a world shift, so queries answer in the new frame
    /// at once.
    pub fn shift(&mut self, s: &WorldShift) {
        for b in &mut self.bodies {
            b.shift(s);
        }
        self.ripples.shift(s);
        for f in &mut self.flows {
            f.shift(s);
        }
        for h in &mut self.hulls {
            let p = s.point(DVec3::new(h.center.x, 0.0, h.center.y));
            h.center = DVec2::new(p.x, p.z);
            let d = s.direction(engine_core::math::Vec3::new(
                h.half.x as f32,
                0.0,
                h.half.y as f32,
            ));
            h.half = DVec2::new(d.x as f64, d.z as f64);
        }
        self.shifts += 1;
    }

    /// Still-water depth of the deepest body covering `p` (for ripple wave speeds).
    pub fn still_depth(&self, p: DVec3) -> Option<f32> {
        self.bodies
            .iter()
            .filter(|b| b.body.interactive)
            .filter_map(|b| b.depth_at(p, &self.ground))
            .fold(None, |acc: Option<f32>, d| {
                Some(acc.map_or(d, |a| a.max(d)))
            })
    }

    /// How much of the ripples count at `p`: 0 under a moving interactor (its own push), 1
    /// clear of them.
    pub fn hull_weight(&self, p: DVec2) -> f32 {
        self.hulls.iter().fold(1.0, |w, h| {
            let r = h.radius;
            let len2 = h.half.length_squared();
            let t = if len2 > 0.0 {
                ((p - h.center).dot(h.half) / len2).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let d = (p - (h.center + h.half * t)).length() as f32;
            let t = ((d - r * 0.9) / (r * 0.9)).clamp(0.0, 1.0);
            w.min(t * t * (3.0 - 2.0 * t))
        })
    }

    /// Sample one body at `p`: (surface height, depth, velocity, normal, foam).
    pub fn sample_body(&self, b: &BodyState, p: DVec3) -> Option<(f64, f32, Vec3, Vec3, f32)> {
        let depth = b.depth_at(p, &self.ground)?;
        if depth <= 0.0 {
            return None;
        }
        let w: WavePoint = b.waves.sample(b.wave_coords(p), depth);
        let mut height = b.center.y + w.offset.y as f64;
        let mut normal = w.normal;
        let mut foam = 0.0;
        let mut velocity = Vec3::new(b.body.current[0], 0.0, b.body.current[1]) + w.velocity;
        if b.body.interactive
            && let Some(r) = self.ripples.sample(DVec2::new(p.x, p.z))
        {
            // A moving interactor's own push (its trough, the water it drags along) doesn't
            // act back on it.
            let own = self.hull_weight(DVec2::new(p.x, p.z));
            height += (r.height * own) as f64;
            let g = r.gradient * own;
            normal = (normal + Vec3::new(-g.x, 0.0, -g.y)).normalize_or(Vec3::Y);
            velocity += Vec3::new(r.current.x, 0.0, r.current.y) * own;
            foam = r.foam;
        }
        Some((height, depth + w.offset.y, velocity, normal, foam))
    }

    /// The water at `p`, from bodies and flowing regions (the highest surface wins).
    pub fn sample(&self, p: DVec3) -> Option<WaterSample> {
        let mut best: Option<WaterSample> = None;
        for b in &self.bodies {
            if let Some((h, depth, velocity, normal, _)) = self.sample_body(b, p)
                && best.is_none_or(|w| h > w.surface_height)
            {
                best = Some(WaterSample {
                    surface_height: h,
                    depth,
                    velocity,
                    normal,
                });
            }
        }
        for f in &self.flows {
            if let Some((h, depth, v)) = f.sample(p)
                && best.is_none_or(|w| h > w.surface_height)
            {
                best = Some(WaterSample {
                    surface_height: h,
                    depth,
                    velocity: Vec3::new(v.x, 0.0, v.y),
                    normal: Vec3::Y,
                });
            }
        }
        best
    }

    /// Ground or bed height under `p` as water sees it (for things that sink).
    pub fn bed_height(&self, p: DVec3) -> Option<f64> {
        for b in &self.bodies {
            if let Some(d) = b.depth_at(p, &self.ground)
                && d > 0.0
            {
                return Some(b.center.y - d as f64);
            }
        }
        for f in &self.flows {
            let local = f.local(p);
            let (i, j) = (
                (local.x / f.cell as f64).floor(),
                (local.y / f.cell as f64).floor(),
            );
            if i >= 0.0 && j >= 0.0 && (i as usize) < f.nx && (j as usize) < f.nz {
                return Some(f.origin.y + f.bed[j as usize * f.nx + i as usize] as f64);
            }
        }
        self.ground.height(p)
    }
}

/// Shared handle to the water state. A resource; the `Water` query and the renderer read it.
#[derive(Resource, Clone)]
pub struct SharedWater(pub Arc<RwLock<WaterState>>);

impl SharedWater {
    pub fn new(state: WaterState) -> Self {
        Self(Arc::new(RwLock::new(state)))
    }
}

/// The `engine_world::WaterQuery` implementation installed by `WaterPlugin`.
pub struct WaterProvider(pub Arc<RwLock<WaterState>>);

impl WaterQuery for WaterProvider {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        self.0.read().sample(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::{GroundQuery, GroundSample, SurfaceId};

    /// A bowl with ripples in its floor, raised by `bump` round (20, 0, 5) within 3 m.
    struct Basin(f64);

    impl GroundQuery for Basin {
        fn sample(&self, p: DVec3) -> Option<GroundSample> {
            let (x, z) = (p.x - 100.0, p.z + 40.0);
            let mut h = -8.0 + 0.004 * (x * x + z * z) + 0.3 * (x * 0.7).sin() * (z * 0.4).cos();
            if (x - 20.0).hypot(z - 5.0) < 3.0 {
                h += self.0;
            }
            Some(GroundSample {
                height: h + 2.0,
                normal: Vec3::Y,
                surface: SurfaceId(3),
            })
        }
    }

    fn lake(ground: &Ground, yaw: f32) -> BodyState {
        BodyState {
            entity: Entity::from_raw_u32(7).unwrap(),
            center: DVec3::new(100.0, 2.0, -40.0),
            yaw,
            body: WaterBody::lake(45.0),
            waves: WaveSet::default(),
            mean_depth: 5.0,
            wind: Vec3::ZERO,
            bed: Some(BedRef {
                center: DVec3::new(100.0, 2.0, -40.0),
                yaw,
                grid: Arc::new(BedCache::build(
                    DVec3::new(100.0, 2.0, -40.0),
                    yaw,
                    45.0,
                    ground,
                )),
                rev: 0,
            }),
        }
    }

    #[test]
    fn a_lakes_bed_grid_reads_as_the_ground_does_and_takes_edits() {
        let ground = Ground(Arc::new(Basin(0.0)));
        for yaw in [0.0, 0.7] {
            let mut b = lake(&ground, yaw);
            let mut bare = b.clone();
            bare.bed = None;
            let mut worst = 0.0f32;
            for k in 0..2000 {
                let a = k as f64 * 2.399;
                let r = 44.0 * ((k as f64 + 0.5) / 2000.0).sqrt();
                let p = b.center + DVec3::new(a.cos() * r, 0.0, a.sin() * r);
                let (cached, direct) = (b.depth_at(p, &ground), bare.depth_at(p, &ground));
                worst = worst.max((cached.unwrap() - direct.unwrap()).abs());
            }
            // 0.5 m points over a floor that rises 0.3 m in 2 m: within a couple of cm.
            assert!(worst < 0.03, "yaw {yaw}: {worst} m off");
            // Outside the lake: no water, as before.
            assert_eq!(
                b.depth_at(b.center + DVec3::new(60.0, 0.0, 0.0), &ground),
                None
            );
            // A mound raised in the bed shows once the edit is read in.
            let edited = Ground(Arc::new(Basin(1.5)));
            let at = DVec3::new(120.0, 0.0, -35.0);
            let before = b.depth_at(at, &ground).unwrap();
            let r = b.bed.as_mut().unwrap();
            let local = Vec2::new(
                yaw.cos() * 20.0 - yaw.sin() * 5.0,
                yaw.sin() * 20.0 + yaw.cos() * 5.0,
            );
            let (c, y) = (r.center, r.yaw);
            Arc::make_mut(&mut r.grid).reread(c, y, local, 4.0, &edited);
            let after = b.depth_at(at, &edited).unwrap();
            assert!((before - after - 1.5).abs() < 0.05, "{before} -> {after}");
        }
    }

    #[test]
    fn a_bed_grid_follows_a_world_shift() {
        let ground = Ground(Arc::new(Basin(0.0)));
        let mut b = lake(&ground, 0.3);
        let p = DVec3::new(112.0, 0.0, -31.0);
        let before = b.depth_at(p, &ground).unwrap();
        let s = WorldShift {
            rotation: engine_core::math::DQuat::from_rotation_y(0.4),
            translation: DVec3::new(-80.0, -2.0, 30.0),
            ..WorldShift::IDENTITY
        };
        b.shift(&s);
        assert!(b.bed.is_some());
        // The same spot, in the new frame (the ground there is not asked: the grid answers).
        let after = b.depth_at(s.point(p), &Ground::default()).unwrap();
        assert!((before - after).abs() < 1e-3, "{before} -> {after}");
        // A tilt drops it.
        b.shift(&WorldShift {
            rotation: engine_core::math::DQuat::from_rotation_x(0.01),
            ..WorldShift::IDENTITY
        });
        assert!(b.bed.is_none());
    }
}
