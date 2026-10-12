//! GPU data layouts. Each struct here matches one in `shaders/particles_common.wgsl`.

use bytemuck::{Pod, Zeroable};

use crate::effect::EmitterDesc;
use engine_core::math::{Quat, Vec3};

/// Air density at sea level on Earth, kg/m³: the air effects are authored for.
pub const SEA_LEVEL_AIR: f32 = 1.225;
/// Air denser than this many times [`SEA_LEVEL_AIR`] (Venus) acts like it, so effects stay sane.
const MAX_AIR_THICKNESS: f32 = 4.0;

/// Samples of the color gradient the GPU interpolates between.
pub const COLOR_SAMPLES: usize = 16;
/// Samples of the size curve (packed four per `vec4`).
pub const SIZE_SAMPLES: usize = 16;
/// Particles per update/spawn work item (one compute workgroup).
pub const CHUNK: u32 = 256;

/// One particle (64 bytes). Positions are camera-relative for world-space emitters and
/// emitter-relative for local-space ones.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuParticle {
    pub pos: [f32; 3],
    /// Seconds since birth. Dead when `age >= life`, so zeroed memory is dead.
    pub age: f32,
    pub vel: [f32; 3],
    pub life: f32,
    /// Scene light falling on the particle per unit albedo (cd/m²), refreshed every few frames.
    pub light: [f32; 3],
    /// Diameter at birth, meters.
    pub size: f32,
    pub rot: f32,
    pub spin: f32,
    /// Random 0..1 per particle (sprite variation).
    pub seed: f32,
    /// Index into this frame's emitter table.
    pub emitter: u32,
}

/// Per-emitter parameters for one frame (608 bytes). The static part is baked from an
/// [`EmitterDesc`] once; positions, wind, gravity and the collision plane change per frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GpuEmitter {
    /// xyz camera-relative position now, w this emitter's time step (seconds).
    pub origin: [f32; 4],
    /// xyz camera-relative position last frame, w 1 for local-space particles.
    pub prev: [f32; 4],
    /// Rotation quaternion (local → world).
    pub rot: [f32; 4],
    /// Emitter velocity (m/s), w fraction inherited.
    pub velocity: [f32; 4],
    /// Gravity × multiplier + acceleration (m/s²), w drag (1/s).
    pub forces: [f32; 4],
    /// Air velocity the particles drift toward (m/s), w turbulence (m/s²).
    pub wind: [f32; 4],
    /// Turbulence scale (m), vortex (m/s²), attraction (m/s²), orbit speed (m/s).
    pub motion: [f32; 4],
    /// Collision plane: unit normal xyz and d (camera-relative, `dot(n, p) + d = 0`).
    pub plane: [f32; 4],
    /// Collision mode (0 none, 1 bounce, 2 die, 3 stick), restitution, friction, plane valid.
    pub collide: [f32; 4],
    /// Shape code and three parameters (see `EmitShape::gpu`).
    pub shape: [f32; 4],
    /// Shape offset (local, m), w cosine of the spread angle.
    pub offset: [f32; 4],
    /// Main direction (local, unit), w unused.
    pub direction: [f32; 4],
    /// Extra birth velocity (local, m/s), w unused.
    pub extra_vel: [f32; 4],
    /// Speed min/max (m/s), lifetime min/max (s).
    pub speed_life: [f32; 4],
    /// Size min/max (m), rotation min/max (rad).
    pub size_rot: [f32; 4],
    /// Spin min/max (rad/s), sprite code, facing code.
    pub spin_look: [f32; 4],
    /// Emissive (cd/m²), lit (0..1), additive (0..1), soft distance (m).
    pub shade: [f32; 4],
    /// Stretch (s), translucency (0..1), size multiplier, opacity multiplier.
    pub look: [f32; 4],
    /// Color over life (linear RGB, coverage).
    pub color: [[f32; 4]; COLOR_SAMPLES],
    /// Size multiplier over life, four samples per element.
    pub size_curve: [[f32; 4]; SIZE_SAMPLES / 4],
}

impl GpuEmitter {
    /// Bake the parts that only depend on the description and the effect's `scale`.
    pub fn bake(desc: &EmitterDesc, scale: f32) -> Self {
        let s = scale.max(0.0);
        let mut shape = desc.shape.gpu();
        // Scale lengths (not the cone angle's cosine).
        shape[1] *= s;
        if shape[0] != 3.0 {
            shape[2] *= s;
        }
        if shape[0] == 6.0 {
            shape[3] *= s;
        }
        let dir = normalize3(desc.direction);
        let spread_cos = desc.spread_deg.clamp(0.0, 180.0).to_radians().cos();
        let [collide_mode, restitution, friction] = desc.collide.gpu();
        let size_samples: [f32; SIZE_SAMPLES] = desc.size_over_life.bake();
        let mut size_curve = [[0.0; 4]; SIZE_SAMPLES / 4];
        for (i, v) in size_samples.iter().enumerate() {
            size_curve[i / 4][i % 4] = *v;
        }
        let [lo, hi] = desc.lifetime;
        Self {
            origin: [0.0; 4],
            prev: [0.0, 0.0, 0.0, if desc.local_space { 1.0 } else { 0.0 }],
            rot: [0.0, 0.0, 0.0, 1.0],
            velocity: [0.0, 0.0, 0.0, desc.inherit_velocity],
            forces: [0.0, 0.0, 0.0, desc.drag.max(0.0)],
            wind: [0.0, 0.0, 0.0, desc.turbulence * s],
            motion: [
                (desc.turbulence_scale * s).max(0.01),
                desc.vortex * s,
                desc.attract * s,
                desc.orbit_speed * s,
            ],
            plane: [0.0, 1.0, 0.0, 0.0],
            collide: [collide_mode, restitution, friction, 0.0],
            shape,
            offset: [
                desc.offset[0] * s,
                desc.offset[1] * s,
                desc.offset[2] * s,
                spread_cos,
            ],
            direction: [dir[0], dir[1], dir[2], 0.0],
            extra_vel: [
                desc.velocity[0] * s,
                desc.velocity[1] * s,
                desc.velocity[2] * s,
                0.0,
            ],
            speed_life: [
                desc.speed[0] * s,
                desc.speed[1] * s,
                lo.min(hi).max(1e-3),
                hi.max(lo).max(1e-3),
            ],
            size_rot: [
                desc.size[0] * s,
                desc.size[1] * s,
                desc.rotation_deg[0].to_radians(),
                desc.rotation_deg[1].to_radians(),
            ],
            spin_look: [
                desc.spin_deg[0].to_radians(),
                desc.spin_deg[1].to_radians(),
                desc.sprite.code(),
                desc.facing.code(),
            ],
            shade: [
                desc.emissive.max(0.0),
                desc.lit.clamp(0.0, 1.0),
                desc.additive.clamp(0.0, 1.0),
                desc.soft.max(0.0),
            ],
            look: [desc.stretch.max(0.0), desc.sprite.translucency(), 1.0, 1.0],
            color: desc.color.bake(),
            size_curve,
        }
    }

    /// Forces from the local gravity and air. Effects are authored for sea-level air on a flat
    /// world: their constant acceleration (the buoyancy of flames and smoke) points away from
    /// the planet wherever "up" is, and buoyancy, drag, turbulence and the air's support of
    /// floating dust scale with the air's density, so smoke barely rises on Mars and dust
    /// flies in clean ballistic arcs on the Moon.
    pub fn set_forces(&mut self, gravity: Vec3, air_density: f32, desc: &EmitterDesc, scale: f32) {
        let thickness = (air_density / SEA_LEVEL_AIR).clamp(0.0, MAX_AIR_THICKNESS);
        let up = (-gravity).try_normalize().unwrap_or(Vec3::Y);
        let lift = Quat::from_rotation_arc(Vec3::Y, up)
            * Vec3::from(desc.acceleration)
            * (scale * thickness);
        // A gravity multiplier under 1 stands for the air holding dust, smoke and spray up;
        // without air they fall like stones. Light (glows, sparks, magic) keeps its own.
        let held = if desc.additive < 0.5 && desc.gravity < 1.0 {
            1.0 - (1.0 - desc.gravity) * thickness.min(1.0)
        } else {
            desc.gravity
        };
        let f = gravity * held + lift;
        self.forces = [f.x, f.y, f.z, self.forces[3] * thickness];
        self.wind[3] *= thickness;
    }
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// One run of new particles for a compute workgroup (64 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuSpawn {
    /// Emitter table index.
    pub emitter: u32,
    /// First slot of the emitter's ring in the particle pool.
    pub base: u32,
    /// Ring size.
    pub capacity: u32,
    /// Ring position of this chunk's first particle.
    pub ring_start: u32,
    /// Particles in this chunk (≤ [`CHUNK`]).
    pub count: u32,
    /// Index of this chunk's first particle within its run.
    pub index: u32,
    /// Particles in the whole run (spread evenly over the step).
    pub run_count: u32,
    pub seed: u32,
    /// xyz emitter position at the start of the step (camera-relative), w step length (s).
    pub from: [f32; 4],
    /// xyz emitter position at the end of the step, w extra seconds to advance (prewarm).
    pub to: [f32; 4],
}

/// A range of particle slots to update (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuUpdate {
    pub emitter: u32,
    pub first: u32,
    pub count: u32,
    pub _pad: u32,
}

/// Per-frame simulation constants (128 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuSim {
    /// xyz: how far the camera origin moved since last frame (added to world-space particles),
    /// w: world time for turbulence (wrapped), seconds.
    pub shift: [f32; 4],
    /// Update chunks, spawn chunks, frame index, unused.
    pub counts: [u32; 4],
    /// Scene collision: enabled (1/0), near plane (m), surface thickness (m), unused.
    pub depth: [f32; 4],
    /// The world re-anchored since the last step (see `engine_world::WorldShift`): columns of
    /// the rotation turning particle positions and velocities, `turn[0][3]` = 1 when set.
    pub turn: [[f32; 4]; 3],
    /// xyz: velocity the old frame had at the last camera origin (added to particles), m/s.
    pub frame_velocity: [f32; 4],
    /// xyz: angular velocity of the old frame, rad/s.
    pub frame_spin: [f32; 4],
}

/// One ribbon vertex (48 bytes): trails and beams, built on the CPU each frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct RibbonVertex {
    /// Camera-relative position, meters.
    pub pos: [f32; 3],
    /// Meters from the head of the ribbon (stable noise along it).
    pub dist: f32,
    /// Linear RGB radiance (cd/m²) and coverage.
    pub color: [f32; 4],
    /// -1..1 across the ribbon.
    pub across: f32,
    /// Style code (see `RibbonStyle`).
    pub style: f32,
    /// 0 alpha blended .. 1 additive.
    pub additive: f32,
    /// 0..1 from head to tail.
    pub along: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_the_shader() {
        assert_eq!(std::mem::size_of::<GpuParticle>(), 64);
        assert_eq!(
            std::mem::size_of::<GpuEmitter>(),
            18 * 16 + 16 * 16 + 4 * 16
        );
        assert_eq!(std::mem::size_of::<GpuSpawn>(), 64);
        assert_eq!(std::mem::size_of::<GpuUpdate>(), 16);
        assert_eq!(std::mem::size_of::<GpuSim>(), 128);
        assert_eq!(std::mem::size_of::<RibbonVertex>(), 48);
    }

    #[test]
    fn zeroed_particles_are_dead() {
        let p = GpuParticle::zeroed();
        assert!(p.age >= p.life);
    }

    #[test]
    fn bake_scales_lengths_but_not_angles() {
        let desc = EmitterDesc {
            shape: crate::effect::EmitShape::Cone {
                angle_deg: 60.0,
                radius: 2.0,
            },
            size: [0.5, 1.0],
            speed: [2.0, 4.0],
            ..Default::default()
        };
        let a = GpuEmitter::bake(&desc, 1.0);
        let b = GpuEmitter::bake(&desc, 2.0);
        assert_eq!(b.shape[1], 4.0);
        assert!((a.shape[2] - 0.5).abs() < 1e-6 && a.shape[2] == b.shape[2]);
        assert_eq!(b.size_rot[0], 1.0);
        assert_eq!(b.speed_life[1], 8.0);
        assert_eq!(a.speed_life[2], 1.0);
    }

    #[test]
    fn smoke_rises_away_from_the_planet_and_only_as_thick_air_lets_it() {
        let desc = EmitterDesc {
            acceleration: [0.0, 3.0, 0.0],
            drag: 2.0,
            turbulence: 1.0,
            ..Default::default()
        };
        let forces = |gravity: Vec3, density: f32| {
            let mut gpu = GpuEmitter::bake(&desc, 1.0);
            gpu.set_forces(gravity, density, &desc, 1.0);
            (
                Vec3::from_slice(&gpu.forces[..3]),
                gpu.forces[3],
                gpu.wind[3],
            )
        };
        let near = |a: Vec3, b: Vec3| a.distance(b) < 1e-4;

        // On Earth at sea level: rises 3 m/s² against 9.81 m/s² of gravity, full drag.
        let (f, drag, swirl) = forces(Vec3::new(0.0, -9.81, 0.0), SEA_LEVEL_AIR);
        assert!(
            near(f, Vec3::new(0.0, -9.81 * desc.gravity + 3.0, 0.0)),
            "{f}"
        );
        assert_eq!((drag, swirl), (2.0, 1.0));

        // On a planet's side, where down is -X: it rises along +X, not world Y.
        let (f, ..) = forces(Vec3::new(-9.81, 0.0, 0.0), SEA_LEVEL_AIR);
        assert!(
            near(f, Vec3::new(-9.81 * desc.gravity + 3.0, 0.0, 0.0)),
            "{f}"
        );

        // Mars (1.6 % of Earth's air): buoyancy, drag and turbulence nearly gone, and the
        // air barely holds the smoke up.
        let thin = 0.020 / SEA_LEVEL_AIR;
        let (f, drag, swirl) = forces(Vec3::new(0.0, -3.71, 0.0), 0.020);
        assert!(
            (f.y - (-3.71 * (1.0 - thin) + 3.0 * thin)).abs() < 1e-4,
            "{f}"
        );
        assert!(drag < 0.04 && swirl < 0.02, "{drag} {swirl}");

        // The Moon: no air at all, so smoke falls like a stone and nothing else acts.
        let (f, drag, swirl) = forces(Vec3::new(0.0, -1.62, 0.0), 0.0);
        assert!(near(f, Vec3::new(0.0, -1.62, 0.0)), "{f}");
        assert_eq!((drag, swirl), (0.0, 0.0));

        // A glowing (additive) mote keeps floating: magic is not held up by air.
        let glow = EmitterDesc {
            additive: 1.0,
            ..desc.clone()
        };
        let mut gpu = GpuEmitter::bake(&glow, 1.0);
        gpu.set_forces(Vec3::new(0.0, -1.62, 0.0), 0.0, &glow, 1.0);
        assert_eq!(gpu.forces[1], -1.62 * glow.gravity);
    }
}
