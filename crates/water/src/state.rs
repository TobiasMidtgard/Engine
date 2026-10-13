//! The live water state shared by the simulation, the `Water` query and the renderer.

use std::sync::Arc;

use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_world::{Ground, WaterQuery, WaterSample};
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
                let probe = DVec3::new(p.x, self.center.y + 0.05, p.z);
                match ground.height(probe) {
                    Some(h) => (self.center.y - h) as f32,
                    None => self.mean_depth,
                }
            }
        })
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
}

impl WaterState {
    pub fn new(ripple_cells: usize, ripple_cell: f32) -> Self {
        Self {
            bodies: Vec::new(),
            ripples: RippleField::new(ripple_cells, ripple_cell),
            flows: Vec::new(),
            ground: Ground::default(),
            time: 0.0,
        }
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
        if b.body.interactive
            && let Some((h, grad, f)) = self.ripples.sample(DVec2::new(p.x, p.z))
        {
            height += h as f64;
            normal = (normal + Vec3::new(-grad.x, 0.0, -grad.y)).normalize_or(Vec3::Y);
            foam = f;
        }
        let velocity = Vec3::new(b.body.current[0], 0.0, b.body.current[1]) + w.velocity;
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
            let local = p - f.origin;
            let (i, j) = (
                (local.x / f.cell as f64).floor(),
                (local.z / f.cell as f64).floor(),
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
