//! The runtime side of a soft body: the solver state plus what to draw and what the frames
//! follow. Built by the plugin from a [`Cloth`](crate::Cloth), [`Rope`](crate::Rope),
//! [`RopeBridge`](crate::RopeBridge), [`SoftVolume`](crate::SoftVolume) or
//! [`HairGroom`](crate::HairGroom) component.

use engine_core::prelude::*;

use crate::cloth::ClothState;
use crate::collide::LocalCollider;
use crate::hair::Strands;
use crate::rope::BridgeState;
use crate::settings::{SimSettings, SoftMaterial};
use crate::solver::Sim;
use crate::systems::StepOutput;

/// What kind of thing a soft body is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoftKind {
    Cloth,
    Rope,
    Bridge,
    Volume,
    Hair,
}

/// What a solver frame follows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameSource {
    /// Fixed in the world.
    World,
    /// An entity's `GlobalTransform`.
    Entity(Entity),
    /// Bone `bone` of `owner`'s `SkeletonPose`.
    Bone { owner: Entity, bone: usize },
}

/// Triangles to draw, following the particles.
#[derive(Debug, Clone, Default)]
pub struct Surface {
    /// Triangles currently drawn (indices into render vertices).
    pub triangles: Vec<[u32; 3]>,
    /// Texture coordinates per render vertex.
    pub uv: Vec<Vec2>,
    /// How render vertices follow particles. `None` = one render vertex per particle.
    pub embed: Option<Embedding>,
    /// Per triangle (same order as `triangles`): inverse of the rest-shape edge matrix in
    /// pattern space (meters), for tension. Empty = no tension shading.
    pub rest_inv: Vec<Mat2>,
    /// Meters per texture coordinate unit (u, v); sizes printed patterns and wrinkles.
    pub uv_scale: Vec2,
}

impl Surface {
    pub fn vertex_count(&self) -> usize {
        self.uv.len()
    }
}

/// Render vertices embedded in a particle lattice: each is a weighted sum of 8 particles
/// plus a rest offset that turns with the body (the body's shape-matching rotation).
#[derive(Debug, Clone, Default)]
pub struct Embedding {
    pub particles: Vec<[u32; 8]>,
    pub weights: Vec<[f32; 8]>,
    pub offsets: Vec<Vec3>,
}

/// A visible rope: a chain of particles drawn as a tube.
#[derive(Debug, Clone, PartialEq)]
pub struct Tube {
    pub particles: Vec<u32>,
    pub radius: f32,
    pub beads: Option<crate::rope::Beads>,
}

/// Runtime state of one soft body. Added by the plugin; read it for positions
/// ([`SoftBodySim::world_position`]) and stats.
#[derive(Component)]
pub struct SoftBodySim {
    pub kind: SoftKind,
    pub sim: Sim,
    /// What each solver frame follows (same order as `sim.frames`).
    pub frames: Vec<FrameSource>,
    pub settings: SimSettings,
    pub material: SoftMaterial,
    pub surface: Option<Surface>,
    pub tubes: Vec<Tube>,
    pub strands: Option<Strands>,
    pub cloth: Option<ClothState>,
    pub bridge: Option<BridgeState>,
    /// Colliders this body never touches (its own bridge planks).
    pub ignore: Vec<Entity>,
    /// Entity whose `LocalTimeScale` this body runs on (the character wearing it).
    pub time_owner: Option<Entity>,
    /// Fixed steps simulated so far.
    pub steps: u64,
    /// Steps to simulate at once on the next update (prewarm).
    pub pending_prewarm: u32,
    /// Bumped whenever `surface.triangles` or `tubes` change (tearing, cutting, burning).
    pub topology_version: u64,
    /// World-space bounding sphere at the end of the last step.
    pub center: DVec3,
    pub radius: f32,
    /// Skipped this step (level of detail).
    pub frozen: bool,
    /// Wind felt at the body last step, m/s.
    pub wind: Vec3,
    /// Milliseconds the last step took.
    pub step_ms: f32,
    /// Results of the last step for the world (impulses, smoke, landings).
    pub out: StepOutput,
    /// Simulated time owed by skipped level-of-detail steps, seconds.
    pub lod_debt: f32,
    pub(crate) scratch_colliders: Vec<LocalCollider>,
}

impl SoftBodySim {
    pub fn new(
        kind: SoftKind,
        mut sim: Sim,
        settings: SimSettings,
        material: SoftMaterial,
    ) -> Self {
        // Start with the body's own settings (the plugin refreshes them every step).
        let p = &mut sim.params;
        p.substeps = settings.substeps.max(1);
        p.damping = settings.damping;
        p.friction = settings.friction;
        p.max_speed = settings.max_speed;
        p.gravity *= settings.gravity_scale;
        let pending_prewarm = (settings.prewarm_secs.max(0.0) * 60.0).round() as u32;
        let mut body = Self {
            kind,
            sim,
            frames: Vec::new(),
            settings,
            material,
            surface: None,
            tubes: Vec::new(),
            strands: None,
            cloth: None,
            bridge: None,
            ignore: Vec::new(),
            time_owner: None,
            steps: 0,
            pending_prewarm,
            topology_version: 1,
            center: DVec3::ZERO,
            radius: 0.0,
            frozen: false,
            wind: Vec3::ZERO,
            step_ms: 0.0,
            out: StepOutput::default(),
            lod_debt: 0.0,
            scratch_colliders: Vec::new(),
        };
        body.update_bounds();
        body
    }

    /// Add a solver frame that follows `source`, placed at its current pose (body-local).
    pub fn add_frame(&mut self, source: FrameSource, pos: Vec3, rot: Quat) -> u16 {
        self.frames.push(source);
        self.sim.add_frame(pos, rot)
    }

    /// World position of particle `i` at the end of the last step.
    pub fn world_position(&self, i: usize) -> DVec3 {
        self.sim.origin + self.sim.pos[i].as_dvec3()
    }

    /// Body-local coordinates of a world point.
    pub fn local(&self, p: DVec3) -> Vec3 {
        (p - self.sim.origin).as_vec3()
    }

    /// Recompute the bounding sphere from the particles.
    pub fn update_bounds(&mut self) {
        if self.sim.is_empty() {
            self.center = self.sim.origin;
            self.radius = 0.0;
            return;
        }
        let (lo, hi) = self.sim.bounds();
        let c = (lo + hi) * 0.5;
        self.center = self.sim.origin + c.as_dvec3();
        self.radius = (hi - lo).length() * 0.5 + self.settings.thickness;
    }

    /// Particles currently simulated.
    pub fn particle_count(&self) -> usize {
        self.sim.len()
    }

    /// Short description for `softbody.list`.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind,
            "particles": self.sim.len(),
            "constraints": self.sim.distances.len() + self.sim.goals.len() + self.sim.tethers.len(),
            "pins": self.sim.pins.len(),
            "center": self.center.to_array(),
            "radius": self.radius,
            "frozen": self.frozen,
            "step_ms": self.step_ms,
            "triangles": self.surface.as_ref().map(|s| s.triangles.len()).unwrap_or(0),
            "guides": self.strands.as_ref().map(|s| s.guides.len()).unwrap_or(0),
        })
    }
}
