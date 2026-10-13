//! Combat and movement effects, and the render feature that draws them.
//!
//! * **Blade trails**: every [`Blade`] leaves a glowing crescent behind its tip while its
//!   owner strikes (faintly from the very end of the windup, fully through the strike, fading
//!   over the start of the recovery), or, outside attacks, while the blade keeps sweeping
//!   fast. Samples are smoothed with Catmull-Rom splines and the blade's direction is
//!   interpolated around its base, so even a swing that crosses 120° between two frames
//!   draws a clean arc instead of a chord. The ribbon is emissive in the blade color with a
//!   thin white-hot line along the cutting edge; it is widest right behind the blade and
//!   tapers to a point along the tip's path, by age and by distance along the cut, so a cut
//!   reads as a crescent however fast it was made. Trails follow most of their owner's own
//!   motion ([`EffectSettings::trail_follow`]): a dash-strike leaves a crescent, not a smear
//!   across the arena (the afterimages show the dash).
//! * **Strikes** (from the [`CombatLog`]): a blocked strike throws a burst of sparks from
//!   where the blades meet (they arc under gravity, bounce off the floor and cool from white
//!   to red); a hit leaves a crimson-to-white slash flash across the target; counters and
//!   kills get a bigger flash (a kill crosses two cuts) and a short-lived [`PointLight`].
//! * **Dust**: landings, sprint and wall-run steps (from `GroundContact`), dash starts, slide
//!   starts and wall-jump kick-offs (from animation triggers) kick up soft dust puffs, and a
//!   slide sprays them all the way. Puffs are textured with baked noise and lit per pixel
//!   like lumpy balls (wrapped sun, forward scattering, sky); they expand, drift and fade, and
//!   fade softly where they meet the ground.
//! * The afterimages of [`crate::afterimage`] are drawn here too: a translucent shadow of
//!   every body part in a deep tint of the trail color, edged with light. A ghost stays
//!   hidden while the body it copies still overlaps it.
//!
//! ## Clocks
//!
//! Effects are light in the world, so they live on the **world** clock: sparks, flashes and
//! dust move with `Time::delta_secs()`, and in slow motion they hang in the air. Trails are
//! sampled once per rendered frame and stamped with world time, and a trail shows the last
//! [`EffectSettings::trail_secs`] world seconds of the blade's path. At full speed that is a
//! single sharp crescent that is gone a moment later; in slow motion the same arc is drawn
//! from many more samples (silky) and lingers for real seconds; and a perceiver who moves at
//! full speed through a slowed world leaves long cuts hanging in the air behind its blade.
//! Spark streaks are motion blur over a fixed real-time shutter, so they shorten to points
//! in slow motion. Afterimages fade in real time (see [`crate::afterimage`]).
//!
//! ## Brightness
//!
//! Emission is in cd/m² at [`EffectSettings::reference_ev100`] (a sunny day). At other
//! exposures it follows the camera by [`EffectSettings::exposure_adapt`], so a trail that
//! reads in daylight doesn't turn into a blinding blob at night.
//!
//! ## Cost
//!
//! Nothing here spawns entities except a pool of at most [`MAX_LIGHTS`] flash lights.
//! Particles live in one capped list (the oldest are recycled), trails in a ring buffer per
//! blade. The feature draws everything in at most four calls plus one per ghost mesh, from
//! buffers that are rewritten in place.

use std::collections::{HashMap, VecDeque};

use bytemuck::{Pod, Zeroable};
use engine_assets::Assets;
use engine_core::math::{DVec3, Mat4, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, Mesh, PointLight, RenderFeature,
    RenderPhase, RenderSettings, ResolvedMesh, Vertex, ViewData, wgpu,
};
use engine_world::animation::AnimationTrigger;
use engine_world::physics::CharacterController;
use engine_world::{ContactKind, Gravity, GroundContact, LocomotionMode, WorldShift};
use wgpu::util::DeviceExt;

use super::{Blade, texgen};
use crate::afterimage::Afterimage;
use crate::combat::{AttackPhase, CombatLog, CombatRecord, CombatState, Fighter, Outcome};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;

/// The effects shader (ribbons, particles, ghosts).
pub const SHADER: &str = include_str!("effects.wgsl");

/// Flash lights alive at once.
pub const MAX_LIGHTS: usize = 4;
/// Samples kept per trail (a trail never needs more: at 60 fps this is two real seconds).
const MAX_TRAIL_SAMPLES: usize = 128;
/// Trail samples older than this many real seconds are dropped whatever the world clock does.
const MAX_TRAIL_REAL_SECS: f64 = 2.0;
/// A blade that jumps farther than this (relative to its owner, or with its owner) between two
/// frames was teleported or re-attached: its trail restarts.
const TELEPORT_M: f64 = 3.0;
const OWNER_TELEPORT_M: f64 = 25.0;
/// Ribbon subdivision: at most this angle (radians) and tip travel (m) per step.
const MAX_STEP_RAD: f32 = 0.07;
const MAX_STEP_M: f32 = 0.05;
const MAX_SUBDIVISIONS: usize = 32;
/// Motion blur of spark streaks, real seconds of exposure.
const SHUTTER_SECS: f32 = 1.0 / 40.0;
/// A sliding character kicks up a dust puff every this many meters.
const SLIDE_DUST_M: f32 = 0.3;

/// Effect tuning. Patch it live (`resource.set EffectSettings`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectSettings {
    pub enabled: bool,
    /// World seconds of blade path a trail shows. Short, so a full-speed swing leaves one
    /// crescent; on the world clock, so the same arc lingers in slow motion.
    pub trail_secs: f32,
    /// The last part of a windup that already trails, as a fraction of the windup.
    pub trail_windup_fraction: f32,
    /// Own seconds into the recovery that still trail (the follow-through).
    pub trail_recovery_secs: f32,
    /// Blades also trail (faintly) while the tip keeps sweeping around the base faster than
    /// this, m/s (real).
    pub trail_sweep_speed: f32,
    /// How much of its owner's own motion a trail follows: 0 = it hangs where it was cut,
    /// 1 = it travels with the owner (a dash-strike leaves a crescent, not a 10 m smear).
    pub trail_follow: f32,
    /// Luminance of a trail's colored body, cd/m².
    pub trail_nits: f32,
    /// Luminance of the white-hot core along the cutting edge, cd/m².
    pub core_nits: f32,
    /// Sparks thrown by a blocked strike.
    pub parry_sparks: u32,
    /// Luminance of a fresh spark, cd/m².
    pub spark_nits: f32,
    /// Luminance of the white core of a slash flash, cd/m².
    pub slash_nits: f32,
    /// Peak luminous power of the flash light on a kill, lumens (counters get 60%, parries
    /// 25%).
    pub impact_light_lm: f32,
    /// Kick up dust from feet, slides, dashes and wall kicks.
    pub dust: bool,
    /// Dust albedo, linear RGB.
    pub dust_albedo: [f32; 3],
    /// Peak opacity of a dust puff, 0..1.
    pub dust_opacity: f32,
    /// Most particles alive at once (sparks, motes, flares, dust); the oldest are recycled.
    pub max_particles: u32,
    /// How much effect brightness follows the camera exposure: 0 = fixed cd/m², 1 = always
    /// the same on screen.
    pub exposure_adapt: f32,
    /// Exposure (EV100) at which the luminances above apply as given.
    pub reference_ev100: f32,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            trail_secs: 0.1,
            trail_windup_fraction: 0.15,
            trail_recovery_secs: 0.12,
            trail_sweep_speed: 14.0,
            trail_follow: 0.8,
            trail_nits: 14_000.0,
            core_nits: 40_000.0,
            parry_sparks: 80,
            spark_nits: 90_000.0,
            slash_nits: 150_000.0,
            impact_light_lm: 120_000.0,
            dust: true,
            dust_albedo: [0.46, 0.42, 0.37],
            dust_opacity: 0.55,
            max_particles: 3000,
            exposure_adapt: 0.5,
            reference_ev100: 14.0,
        }
    }
}

impl EffectSettings {
    /// Multiplier on effect luminance for a camera at `ev100` (see `exposure_adapt`).
    pub fn exposure_scale(&self, ev100: f32) -> f32 {
        2f32.powf(self.exposure_adapt.clamp(0.0, 1.0) * (ev100 - self.reference_ev100))
            .clamp(1e-4, 8.0)
    }
}

// ============================================================================ trails

/// One sample of a blade: where its base and tip were, and when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailSample {
    pub base: DVec3,
    pub tip: DVec3,
    /// Where the blade's owner was (world), so the trail can travel with it.
    pub anchor: DVec3,
    /// World seconds (`Time::elapsed_secs_f64`).
    pub world_secs: f64,
    /// Real seconds.
    pub real_secs: f64,
    /// How strongly this part of the trail glows, 0..1 (1 while swinging).
    pub glow: f32,
}

/// The recent path of one blade.
#[derive(Debug, Clone)]
pub struct Trail {
    pub blade: Entity,
    /// Linear RGB of the blade's glow.
    pub color: [f32; 3],
    /// Oldest first.
    pub samples: VecDeque<TrailSample>,
    /// Where the blade's owner is now (world).
    pub anchor: DVec3,
    /// Frames in a row the tip has swept fast.
    fast_frames: u8,
    seen: bool,
}

/// How a trail ribbon looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailStyle {
    /// World seconds of path shown.
    pub window: f32,
    /// Where the owner is now, and how much of the owner's motion since each sample the trail
    /// follows (0 = it stays where it was cut, 1 = it moves with the owner).
    pub anchor: DVec3,
    pub follow: f32,
    /// Radiance of the colored body and of the white-hot core, cd/m² (linear RGB).
    pub body: Vec3,
    pub core: Vec3,
    /// Varies the silk pattern between blades.
    pub seed: f32,
}

/// One vertex of an effect ribbon (trails, slash flashes, shock rings).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, Default)]
pub struct RibbonVertex {
    /// Camera-relative position (m) and the style: 0 trail, 1 slash flash, 2 ring.
    pub pos_mode: [f32; 4],
    /// Trails: age along the trail (0 at the blade, 1 at the tail) and across (0 at the base,
    /// 1 at the tip). Slashes: along (0..1) and across (−1..1). Rings: around and across
    /// (0 inside, 1 outside). Then fade (0..1) and a seed.
    pub uv_fade_seed: [f32; 4],
    /// Body radiance (cd/m²); w: a world-anchored texture coordinate.
    pub body: [f32; 4],
    /// Core radiance (cd/m²); w: trails: how far back along the lit cut this point is
    /// (0 at the blade, 1 at the oldest lit point), by the tip's path length.
    pub core: [f32; 4],
}

impl RibbonVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// Camera-relative position.
    pub fn position(&self) -> Vec3 {
        Vec3::new(self.pos_mode[0], self.pos_mode[1], self.pos_mode[2])
    }
}

/// Ribbon geometry, rebuilt every frame into the same buffers.
#[derive(Debug, Clone, Default)]
pub struct RibbonMesh {
    pub vertices: Vec<RibbonVertex>,
    pub indices: Vec<u32>,
}

impl RibbonMesh {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }
}

fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

fn catmull_rom1(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    catmull_rom(
        Vec3::splat(p0),
        Vec3::splat(p1),
        Vec3::splat(p2),
        Vec3::splat(p3),
        t,
    )
    .x
}

/// Spherical interpolation between unit directions (a blade turning around its base).
fn slerp_dir(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let d = a.dot(b).clamp(-1.0, 1.0);
    if d > 0.9995 {
        return a.lerp(b, t).normalize_or(a);
    }
    if d < -0.9995 {
        let axis = a.any_orthonormal_vector();
        return Quat::from_axis_angle(axis, std::f32::consts::PI * t) * a;
    }
    let theta = d.acos();
    (a * ((1.0 - t) * theta).sin() + b * (t * theta).sin()) / theta.sin()
}

/// Builds the ribbon of one blade trail into `out` (appending), camera-relative to `origin`.
/// `samples` are oldest first; `now` is the current world time. Returns the number of quads.
///
/// Between two samples the base follows a Catmull-Rom spline and the blade's direction turns
/// around the base (great-circle interpolation plus the spline's curvature), so the tip
/// sweeps an arc. Segments are subdivided by angle and tip travel. Vertices carry the age
/// along the trail (`u`, 0 at the blade, 1 at the end of the window) and the position across
/// it (`v`, 0 at the base, 1 at the tip); quads that have faded out are left out.
pub fn build_trail(
    samples: &VecDeque<TrailSample>,
    now: f64,
    origin: DVec3,
    style: &TrailStyle,
    out: &mut RibbonMesh,
) -> usize {
    let n = samples.len();
    if n < 2 || style.window <= 0.0 {
        return 0;
    }
    let age = |i: usize| (now - samples[i].world_secs).max(0.0) as f32;
    // Skip samples whose successor is already out of the window (keep one for the fade).
    let mut start = 0;
    while start + 1 < n && age(start + 1) >= style.window {
        start += 1;
    }
    if start + 1 >= n || samples.range(start..).all(|s| s.glow <= 1e-3) {
        return 0;
    }
    struct Frame {
        base: Vec3,
        dir: Vec3,
        len: f32,
        age: f32,
        glow: f32,
        secs: f32,
    }
    let follow = style.follow.clamp(0.0, 1.0) as f64;
    let frame = |i: usize| {
        let s = samples[i];
        let shift = (style.anchor - s.anchor) * follow - origin;
        let base = (s.base + shift).as_vec3();
        let d = (s.tip + shift).as_vec3() - base;
        let len = d.length();
        Frame {
            base,
            dir: if len > 1e-6 { d / len } else { Vec3::Y },
            len,
            age: age(i),
            glow: s.glow,
            secs: (s.world_secs % 4096.0) as f32,
        }
    };
    let first = out.vertices.len() as u32;
    let window = style.window;
    let emit = |out: &mut RibbonMesh, base: Vec3, tip: Vec3, age: f32, glow: f32, secs: f32| {
        let u = (age / window).clamp(0.0, 1.0);
        let body = [style.body.x, style.body.y, style.body.z, secs];
        let core = [style.core.x, style.core.y, style.core.z, 0.0];
        for (p, v) in [(base, 0.0), (tip, 1.0)] {
            out.vertices.push(RibbonVertex {
                pos_mode: [p.x, p.y, p.z, 0.0],
                uv_fade_seed: [u, v, glow.clamp(0.0, 1.0), style.seed],
                body,
                core,
            });
        }
    };
    for i in start..n - 1 {
        let f0 = frame(i.saturating_sub(1));
        let f1 = frame(i);
        let f2 = frame(i + 1);
        let f3 = frame((i + 2).min(n - 1));
        let angle = f1.dir.dot(f2.dir).clamp(-1.0, 1.0).acos();
        let travel = (f2.base + f2.dir * f2.len - f1.base - f1.dir * f1.len).length();
        let steps = ((angle / MAX_STEP_RAD).max(travel / MAX_STEP_M).ceil() as usize)
            .clamp(1, MAX_SUBDIVISIONS);
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            let base = catmull_rom(f0.base, f1.base, f2.base, f3.base, t);
            let curve = catmull_rom(f0.dir, f1.dir, f2.dir, f3.dir, t) - f1.dir.lerp(f2.dir, t);
            let arc = slerp_dir(f1.dir, f2.dir, t);
            let dir = (arc + curve).normalize_or(arc);
            let len = catmull_rom1(f0.len, f1.len, f2.len, f3.len, t)
                .clamp(f1.len.min(f2.len) * 0.9, f1.len.max(f2.len) * 1.1);
            let a = f1.age + (f2.age - f1.age) * t;
            let g = f1.glow + (f2.glow - f1.glow) * t;
            let s = f1.secs + (f2.secs - f1.secs) * t;
            emit(out, base, base + dir * len, a, g, s);
        }
    }
    let last = frame(n - 1);
    emit(
        out,
        last.base,
        last.base + last.dir * last.len,
        last.age,
        last.glow,
        last.secs,
    );
    let points = (out.vertices.len() as u32 - first) / 2;
    // How far back along the lit cut each point is, by the length of the tip's path: the
    // ribbon tapers along it, so a cut reads as a crescent however fast it was made.
    let tip_at = |out: &RibbonMesh, p: u32| out.vertices[(first + p * 2 + 1) as usize].position();
    let lit_at = |out: &RibbonMesh, p: u32| {
        let uv = out.vertices[(first + p * 2) as usize].uv_fade_seed;
        uv[2] > 1e-3 && uv[0] < 1.0
    };
    let mut total = 0.0f32;
    let mut along = 0.0f32;
    for p in (0..points).rev() {
        if p + 1 < points {
            along += (tip_at(out, p + 1) - tip_at(out, p)).length();
        }
        if lit_at(out, p) {
            total = along;
        }
    }
    let mut along = 0.0f32;
    for p in (0..points).rev() {
        if p + 1 < points {
            along += (tip_at(out, p + 1) - tip_at(out, p)).length();
        }
        let s = if total > 1e-4 {
            (along / total).min(1.0)
        } else {
            0.0
        };
        for k in 0..2 {
            out.vertices[(first + p * 2 + k) as usize].core[3] = s;
        }
    }
    let mut quads = 0;
    for p in 0..points.saturating_sub(1) {
        let a = first + p * 2;
        let (va, vb) = (out.vertices[a as usize], out.vertices[a as usize + 2]);
        let lit = va.uv_fade_seed[2] + vb.uv_fade_seed[2] > 1e-3;
        let young = va.uv_fade_seed[0].min(vb.uv_fade_seed[0]) < 1.0;
        if lit && young {
            out.quad(a, a + 2, a + 3, a + 1);
            quads += 1;
        }
    }
    quads
}

// ============================================================================ slashes & rings

/// A slash flash across a struck fighter: a crescent that draws across in a blink, then
/// glows crimson and fades. It always faces the camera; its curve follows the swing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slash {
    /// Middle of the cut (world).
    pub center: DVec3,
    /// Angle of the cut on screen, radians from the camera's horizontal (counterclockwise;
    /// adding π draws the same cut the other way). Hit flashes are a graphic accent, so they
    /// are laid out as the camera sees them and read from any angle.
    pub tilt: f32,
    /// Toward the swing's pivot (the attacker), unit: the crescent bends around it.
    pub pivot: Vec3,
    /// Radius of the arc, m.
    pub radius: f32,
    /// Angle the arc covers, radians.
    pub arc: f32,
    /// Widest point of the crescent, m.
    pub width: f32,
    /// Meters it is pulled toward the camera, so the body it cuts never hides it.
    pub lift: f32,
    /// World seconds since it appeared, and how long it lasts.
    pub age: f32,
    pub life: f32,
    /// World seconds the cut takes to draw across.
    pub draw_secs: f32,
    /// Crimson glow and white core radiance, cd/m².
    pub body: Vec3,
    pub core: Vec3,
    pub seed: f32,
}

/// Builds a slash flash's crescent into `out` (camera-relative to `origin`, facing it).
pub fn build_slash(s: &Slash, origin: DVec3, out: &mut RibbonMesh) {
    let t = (s.age / s.life.max(1e-3)).clamp(0.0, 1.0);
    if t >= 1.0 {
        return;
    }
    let center = (s.center - origin).as_vec3();
    let to_cam = (-center).normalize_or(Vec3::Z);
    let center = center + to_cam * s.lift.min(center.length() * 0.5);
    let right = Vec3::Y
        .cross(to_cam)
        .normalize_or(to_cam.any_orthonormal_vector());
    let up = to_cam.cross(right);
    let sweep = right * s.tilt.cos() + up * s.tilt.sin();
    let mut inward = to_cam.cross(sweep);
    if inward.dot(s.pivot) < 0.0 {
        inward = -inward;
    }
    let pivot = center + inward * s.radius;
    let reveal = (s.age / s.draw_secs.max(1e-3)).clamp(0.0, 1.0);
    let reveal = 1.0 - (1.0 - reveal) * (1.0 - reveal);
    let fade = (1.0 - t).powf(1.3);
    // The white core burns off first, leaving the crimson afterglow.
    let core = s.core * (1.0 - t).powi(3);
    let width = s.width * (1.0 + 0.8 * t);
    let segments = 28;
    let first = out.vertices.len() as u32;
    for k in 0..=segments {
        let along = k as f32 / segments as f32 * reveal;
        let phi = (along - 0.5) * s.arc;
        let radial = -inward * phi.cos() + sweep * phi.sin();
        let p = pivot + radial * s.radius;
        let w = width * (std::f32::consts::PI * along).sin().max(0.0).powf(0.75) * 0.5;
        for (side, v) in [(-1.0, -1.0), (1.0, 1.0)] {
            let q = p + radial * (w * side);
            out.vertices.push(RibbonVertex {
                pos_mode: [q.x, q.y, q.z, 1.0],
                uv_fade_seed: [along, v, fade, s.seed],
                body: [s.body.x, s.body.y, s.body.z, reveal],
                core: [core.x, core.y, core.z, 0.0],
            });
        }
    }
    for k in 0..segments as u32 {
        let a = first + k * 2;
        out.quad(a, a + 2, a + 3, a + 1);
    }
}

/// A shock ring: a thin bright ring that races outward from an impact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    pub center: DVec3,
    /// Plane normal (it is turned halfway toward the camera so it never vanishes edge-on;
    /// zero = facing the camera).
    pub normal: Vec3,
    /// Radius at the start and the end, m.
    pub from: f32,
    pub to: f32,
    /// Band width at the start, m.
    pub width: f32,
    pub age: f32,
    pub life: f32,
    pub body: Vec3,
    pub core: Vec3,
}

/// Builds a shock ring into `out` (camera-relative to `origin`).
pub fn build_ring(r: &Ring, origin: DVec3, out: &mut RibbonMesh) {
    let t = (r.age / r.life.max(1e-3)).clamp(0.0, 1.0);
    if t >= 1.0 {
        return;
    }
    let center = (r.center - origin).as_vec3();
    let to_cam = (-center).normalize_or(Vec3::Z);
    let n = (r.normal.normalize_or(to_cam) + to_cam * 0.6).normalize_or(to_cam);
    let a = n.any_orthonormal_vector();
    let b = n.cross(a);
    let ease = 1.0 - (1.0 - t).powi(3);
    let radius = r.from + (r.to - r.from) * ease;
    let width = r.width * (1.0 - 0.6 * t);
    let fade = (1.0 - t).powf(1.5);
    let segments = 48;
    let first = out.vertices.len() as u32;
    for k in 0..=segments {
        let phi = k as f32 / segments as f32 * std::f32::consts::TAU;
        let radial = a * phi.cos() + b * phi.sin();
        for v in [0.0f32, 1.0] {
            let p = center + radial * (radius - width * (1.0 - v));
            out.vertices.push(RibbonVertex {
                pos_mode: [p.x, p.y, p.z, 2.0],
                uv_fade_seed: [k as f32 / segments as f32, v, fade, 0.0],
                body: [r.body.x, r.body.y, r.body.z, 0.0],
                core: [r.core.x, r.core.y, r.core.z, 0.0],
            });
        }
    }
    for k in 0..segments as u32 {
        let i = first + k * 2;
        out.quad(i, i + 2, i + 3, i + 1);
    }
}

// ============================================================================ particles

/// What a particle draws as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleKind {
    /// A hot metal spark: a streak along its velocity that cools from white to red.
    Spark = 0,
    /// A short glow with a four-point star (the instant of contact).
    Flare = 1,
    /// A small soft glowing mote (the crimson spray of a hit).
    Mote = 2,
    /// A lit, translucent dust puff.
    Dust = 3,
}

/// One particle. Positions in world meters, velocities in m/s of world time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub kind: ParticleKind,
    pub pos: DVec3,
    pub vel: Vec3,
    /// World seconds alive, and the lifetime.
    pub age: f32,
    pub life: f32,
    /// Size (radius, m) at birth and at the end.
    pub size: f32,
    pub end_size: f32,
    /// Emission color (unit luminance), or albedo for dust.
    pub color: Vec3,
    /// Luminance at birth (cd/m²), or peak opacity for dust.
    pub nits: f32,
    /// Velocity damping, 1/s.
    pub drag: f32,
    /// Multiplier on gravity (dust drifts up a little instead: negative).
    pub gravity: f32,
    /// Sparks: temperature at birth, kelvin (0 = fixed color).
    pub heat: f32,
    /// Height of the floor sparks bounce on (world y).
    pub floor: f64,
    pub seed: f32,
}

/// GPU layout of `FxParticle` in `effects.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, Default)]
struct ParticleInstance {
    /// Camera-relative center (m), size (m).
    pos_size: [f32; 4],
    /// Radiance (cd/m²) or dust albedo; w: dust opacity.
    color: [f32; 4],
    /// Streak direction (unit) and length (m).
    axis: [f32; 4],
    /// Kind, seed, age 0..1, spare.
    params: [f32; 4],
}

/// Linear RGB of a blackbody at `kelvin`, normalized to luminance 1.
pub fn blackbody(kelvin: f32) -> Vec3 {
    let t = kelvin.clamp(800.0, 12_000.0) / 100.0;
    let r = if t <= 66.0 {
        1.0
    } else {
        (1.292_936_2 * (t - 60.0).powf(-0.133_204_76)).clamp(0.0, 1.0)
    };
    let g = if t <= 66.0 {
        (0.390_081_58 * t.ln() - 0.631_841_4).clamp(0.0, 1.0)
    } else {
        (1.129_890_9 * (t - 60.0).powf(-0.075_514_85)).clamp(0.0, 1.0)
    };
    let b = if t >= 66.0 {
        1.0
    } else if t <= 19.0 {
        0.0
    } else {
        (0.543_206_8 * (t - 10.0).ln() - 1.196_254_1).clamp(0.0, 1.0)
    };
    let c = Vec3::new(r, g, b).powf(2.2);
    c / c.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-4)
}

/// A flash light from the pool.
#[derive(Debug, Clone, Copy)]
struct FlashLight {
    entity: Option<Entity>,
    pos: DVec3,
    color: [f32; 3],
    peak_lm: f32,
    age: f32,
    life: f32,
}

// ============================================================================ state

/// Everything the effects are showing right now. Written by the effect systems
/// (`PostUpdate`), drawn by the effects render feature. Other code can add effects through
/// its methods (`parry`, `hit`, `dust_burst`, …).
#[derive(Resource, Debug, Default)]
pub struct Effects {
    pub trails: Vec<Trail>,
    pub particles: Vec<Particle>,
    pub slashes: Vec<Slash>,
    pub rings: Vec<Ring>,
    lights: Vec<FlashLight>,
    /// World and real seconds at the last update.
    pub now: f64,
    pub real_now: f64,
    rng: u32,
    recycle: usize,
    last_record: Option<CombatRecord>,
    max_particles: usize,
}

/// How big an impact effect is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    Hit,
    Counter,
    Kill,
}

impl Effects {
    /// Move everything showing into the world's new frame (see [`WorldShift`]).
    pub fn shift(&mut self, s: &WorldShift) {
        for t in &mut self.trails {
            t.anchor = s.point(t.anchor);
            for sample in &mut t.samples {
                sample.base = s.point(sample.base);
                sample.tip = s.point(sample.tip);
                sample.anchor = s.point(sample.anchor);
            }
        }
        for p in &mut self.particles {
            p.floor = s.point(DVec3::new(p.pos.x, p.floor, p.pos.z)).y;
            p.pos = s.point(p.pos);
            p.vel = s.velocity_at(p.pos, p.vel);
        }
        for slash in &mut self.slashes {
            slash.center = s.point(slash.center);
            slash.pivot = s.direction(slash.pivot);
        }
        for r in &mut self.rings {
            r.center = s.point(r.center);
            r.normal = s.direction(r.normal);
        }
        for l in &mut self.lights {
            l.pos = s.point(l.pos);
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rand()
    }

    fn unit(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), z, r * a.sin())
    }

    /// A direction within `spread` radians of `dir`.
    fn cone(&mut self, dir: Vec3, spread: f32) -> Vec3 {
        let d = dir.normalize_or(Vec3::Y);
        let a = d.any_orthonormal_vector();
        let b = d.cross(a);
        let phi = self.range(0.0, std::f32::consts::TAU);
        let theta = spread * self.rand().sqrt();
        (d * theta.cos() + (a * phi.cos() + b * phi.sin()) * theta.sin()).normalize_or(d)
    }

    /// Adds a particle, recycling the oldest slots when the cap is reached.
    pub fn push(&mut self, p: Particle) {
        let cap = if self.max_particles == 0 {
            EffectSettings::default().max_particles as usize
        } else {
            self.max_particles
        };
        if self.particles.len() < cap {
            self.particles.push(p);
        } else if !self.particles.is_empty() {
            self.recycle = (self.recycle + 1) % self.particles.len();
            let i = self.recycle;
            self.particles[i] = p;
        }
    }

    fn light(&mut self, pos: DVec3, color: [f32; 3], peak_lm: f32, life: f32) {
        if peak_lm <= 0.0 {
            return;
        }
        let slot = self
            .lights
            .iter()
            .position(|l| l.age >= l.life)
            .or_else(|| (self.lights.len() < MAX_LIGHTS).then_some(self.lights.len()))
            .unwrap_or_else(|| {
                // All busy: take the one closest to done.
                let mut best = 0;
                for (i, l) in self.lights.iter().enumerate() {
                    if l.age / l.life > self.lights[best].age / self.lights[best].life {
                        best = i;
                    }
                }
                best
            });
        let light = FlashLight {
            entity: self.lights.get(slot).and_then(|l| l.entity),
            pos,
            color,
            peak_lm,
            age: 0.0,
            life,
        };
        if slot == self.lights.len() {
            self.lights.push(light);
        } else {
            self.lights[slot] = light;
        }
    }

    /// Flash lights currently lit.
    pub fn lit_lights(&self) -> usize {
        self.lights.iter().filter(|l| l.age < l.life).count()
    }

    fn flare(&mut self, at: DVec3, color: Vec3, nits: f32, size: f32, life: f32) {
        let seed = self.rand();
        self.push(Particle {
            kind: ParticleKind::Flare,
            pos: at,
            vel: Vec3::ZERO,
            age: 0.0,
            life,
            size,
            end_size: size * 1.4,
            color,
            nits,
            drag: 0.0,
            gravity: 0.0,
            heat: 0.0,
            floor: f64::MIN,
            seed,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn sparks(
        &mut self,
        at: DVec3,
        fans: &[Vec3],
        count: u32,
        spread: f32,
        speed: (f32, f32),
        nits: f32,
        floor: f64,
    ) {
        if fans.is_empty() {
            return;
        }
        for i in 0..count {
            let fan = fans[i as usize % fans.len()];
            let dir = self.cone(fan, spread);
            // Most sparks are slow; a few shoot far.
            let v = speed.0 + (speed.1 - speed.0) * self.rand().powf(1.6);
            let life = self.range(0.25, 0.8) * (0.6 + 0.4 * v / speed.1);
            let heat = self.range(4200.0, 6500.0);
            let seed = self.rand();
            let size = self.range(0.008, 0.016);
            let jitter = self.unit() * 0.02;
            let lum = nits * self.range(0.6, 1.2);
            self.push(Particle {
                kind: ParticleKind::Spark,
                pos: at + jitter.as_dvec3(),
                vel: dir * v,
                age: 0.0,
                life,
                size,
                end_size: size * 0.6,
                color: Vec3::ONE,
                nits: lum,
                drag: 0.9,
                gravity: 1.0,
                heat,
                floor,
                seed,
            });
        }
    }

    fn motes(&mut self, at: DVec3, dir: Vec3, sweep: Vec3, count: u32, color: Vec3, nits: f32) {
        for _ in 0..count {
            let v = sweep * self.range(1.5, 6.0)
                + dir * self.range(0.5, 4.0)
                + Vec3::Y * self.range(-0.5, 2.0)
                + self.unit() * 1.2;
            let along = sweep * self.range(-0.35, 0.35);
            let life = self.range(0.35, 0.9);
            let size = self.range(0.012, 0.035);
            let seed = self.rand();
            let lum = nits * self.range(0.5, 1.2);
            self.push(Particle {
                kind: ParticleKind::Mote,
                pos: at + along.as_dvec3(),
                vel: v,
                age: 0.0,
                life,
                size,
                end_size: size * 0.3,
                color,
                nits: lum,
                drag: 2.2,
                gravity: 0.5,
                heat: 0.0,
                floor: f64::MIN,
                seed,
            });
        }
    }

    /// Sparks, a contact flare and a warm light where a blocked strike meets
    /// the guard. `dir` is the attack direction (attacker → defender), `floor` the world
    /// height sparks bounce on.
    pub fn parry(&mut self, at: DVec3, dir: Vec3, floor: f64, s: &EffectSettings) {
        let up = Vec3::Y;
        let dir = flat(dir, up).normalize_or(Vec3::Z);
        let side = up.cross(dir);
        // Two fans to the sides of the blades, up and back toward the attacker.
        let fans = [
            (side * 0.85 + up * 0.55 - dir * 0.35).normalize(),
            (-side * 0.85 + up * 0.55 - dir * 0.35).normalize(),
            (up * 0.9 - dir * 0.4).normalize(),
        ];
        self.sparks(
            at,
            &fans,
            s.parry_sparks,
            0.6,
            (2.5, 11.0),
            s.spark_nits,
            floor,
        );
        self.flare(at, blackbody(6500.0), s.spark_nits * 1.5, 0.2, 0.07);
        self.flare(at, blackbody(4200.0), s.spark_nits * 0.12, 0.5, 0.14);
        self.light(at, [1.0, 0.78, 0.5], s.impact_light_lm * 0.3, 0.14);
    }

    /// A slash flash across a struck fighter (`chest`), with crimson motes and a flare;
    /// counters and kills are bigger and add a light. `dir` is the attack direction
    /// and `pivot_m` the distance to the attacker (the swing's radius).
    pub fn hit(
        &mut self,
        chest: DVec3,
        dir: Vec3,
        pivot_m: f32,
        impact: Impact,
        s: &EffectSettings,
    ) {
        let up = Vec3::Y;
        let dir = flat(dir, up).normalize_or(Vec3::Z);
        let side = up.cross(dir);
        let crimson = Vec3::new(1.0, 0.035, 0.06) / 0.0828;
        let (scale, motes, sparks): (f32, u32, u32) = match impact {
            Impact::Hit => (1.0, 22, 10),
            Impact::Counter => (1.35, 36, 22),
            Impact::Kill => (1.7, 60, 36),
        };
        let crosses = if impact == Impact::Kill { 2 } else { 1 };
        for c in 0..crosses {
            // Diagonal cuts, like a blade drawn across from shoulder to hip; a kill crosses a
            // second one over the first.
            let lean = self.range(0.45, 0.85);
            let mut tilt = if c == 1 {
                std::f32::consts::PI - lean - self.range(0.05, 0.3)
            } else {
                lean
            };
            if self.rand() < 0.5 {
                tilt += std::f32::consts::PI;
            }
            // Roughly the same cut in the world, for the spray.
            let sweep = (side * tilt.cos() + up * tilt.sin()).normalize();
            let seed = self.rand();
            let shift = sweep * self.range(-0.08, 0.08);
            let radius = pivot_m.clamp(0.9, 2.4) * if c == 1 { 1.2 } else { 1.0 };
            self.slashes.push(Slash {
                center: chest + shift.as_dvec3(),
                tilt,
                pivot: -dir,
                radius,
                // About a body's width across, whatever the swing's radius.
                arc: (1.3 * scale.sqrt() / radius).min(1.6),
                width: 0.13 * scale,
                lift: 0.35,
                age: -0.03 * c as f32,
                life: 0.3 * scale,
                draw_secs: 0.035,
                body: crimson * s.slash_nits * 0.07,
                core: Vec3::new(1.0, 0.92, 0.94) * s.slash_nits,
                seed,
            });
            let spray = sweep;
            self.motes(
                chest,
                dir,
                spray,
                motes / crosses,
                crimson,
                s.slash_nits * 0.05,
            );
        }
        self.flare(
            chest,
            Vec3::new(1.0, 0.85, 0.9),
            s.slash_nits * 0.6,
            0.16 * scale,
            0.08,
        );
        self.flare(
            chest,
            crimson,
            s.slash_nits * 0.02,
            0.45 * scale,
            0.2 * scale,
        );
        let floor = chest.y - 1.2;
        self.sparks(
            chest,
            &[up * 0.4 + dir, up * 0.6 - side, up * 0.6 + side],
            sparks,
            0.7,
            (2.0, 8.0),
            s.spark_nits * 0.8,
            floor,
        );
        if impact != Impact::Hit {
            let lm = s.impact_light_lm * if impact == Impact::Kill { 1.0 } else { 0.6 };
            self.light(chest, [1.0, 0.32, 0.3], lm, 0.22 * scale);
        }
    }

    /// Dust puffs: `count` puffs around `at`, each moving with `vel(i)` (m/s).
    #[allow(clippy::too_many_arguments)]
    fn puffs(
        &mut self,
        at: DVec3,
        count: u32,
        spread_m: f32,
        size: (f32, f32),
        life: (f32, f32),
        opacity: f32,
        s: &EffectSettings,
        mut vel: impl FnMut(&mut Self, u32) -> Vec3,
    ) {
        if !s.dust {
            return;
        }
        let albedo = Vec3::from(s.dust_albedo);
        for i in 0..count {
            let v = vel(self, i);
            let off = Vec3::new(self.range(-1.0, 1.0), 0.0, self.range(-1.0, 1.0)) * spread_m;
            let shade = self.range(0.85, 1.12);
            let seed = self.rand();
            let start = size.0 * self.range(0.7, 1.2);
            let end = size.1 * self.range(0.75, 1.25);
            let life = self.range(life.0, life.1);
            self.push(Particle {
                kind: ParticleKind::Dust,
                pos: at + off.as_dvec3() + DVec3::Y * (start as f64 * 0.5),
                vel: v,
                age: 0.0,
                life,
                size: start,
                end_size: end,
                color: albedo * shade,
                nits: (opacity * s.dust_opacity).clamp(0.0, 1.0),
                drag: 2.6,
                gravity: -0.035,
                heat: 0.0,
                floor: f64::MIN,
                seed,
            });
        }
    }

    /// A ring of dust around `feet` (a landing or a stomp). `strength` 0..1.
    pub fn dust_ring(&mut self, feet: DVec3, strength: f32, s: &EffectSettings) {
        let k = strength.clamp(0.0, 1.0);
        let count = 8 + (12.0 * k) as u32;
        let phase = self.rand() * std::f32::consts::TAU;
        self.puffs(
            feet,
            count,
            0.12,
            (0.16, 0.55 + 0.6 * k),
            (0.9, 1.7),
            0.55 + 0.45 * k,
            s,
            |fx, i| {
                let a =
                    phase + i as f32 / count as f32 * std::f32::consts::TAU + fx.range(-0.2, 0.2);
                Vec3::new(a.cos(), 0.0, a.sin()) * (1.2 + 3.0 * k) * fx.range(0.6, 1.0)
                    + Vec3::Y * fx.range(0.05, 0.45)
            },
        );
    }

    /// Dust thrown back from `feet` by a move toward `forward` (dash starts, slides).
    pub fn dust_burst(
        &mut self,
        feet: DVec3,
        forward: Vec3,
        count: u32,
        strength: f32,
        s: &EffectSettings,
    ) {
        let fwd = flat(forward, Vec3::Y).normalize_or(Vec3::Z);
        let side = Vec3::Y.cross(fwd);
        let k = strength.clamp(0.0, 1.5);
        self.puffs(
            feet - (fwd * 0.25).as_dvec3(),
            count,
            0.2,
            (0.18, 0.6 + 0.35 * k),
            (0.8, 1.5),
            0.5 + 0.35 * k,
            s,
            |fx, _| {
                -fwd * fx.range(1.0, 3.5) * k.max(0.4)
                    + side * fx.range(-1.4, 1.4)
                    + Vec3::Y * fx.range(0.1, 0.7)
            },
        );
    }

    /// Dust knocked off a wall at `at` (wall-run steps and wall-jump kick-offs).
    pub fn wall_dust(
        &mut self,
        at: DVec3,
        normal: Vec3,
        count: u32,
        strength: f32,
        s: &EffectSettings,
    ) {
        let n = normal.normalize_or(Vec3::X);
        let k = strength.clamp(0.0, 1.5);
        self.puffs(
            at,
            count,
            0.1,
            (0.12, 0.4 + 0.3 * k),
            (0.6, 1.2),
            0.35 + 0.3 * k,
            s,
            |fx, _| n * fx.range(0.5, 2.5) * k.max(0.3) + fx.unit() * 0.6 + Vec3::Y * 0.2,
        );
    }

    /// Sparks streaming off a glove gripping a zip line at `at`, riding along `heading` at
    /// `speed` m/s, for `dt` seconds: none below 6 m/s, a steady stream at full speed. They
    /// leave with part of the hand's speed and fall away behind it.
    pub fn zip_sparks(
        &mut self,
        at: DVec3,
        heading: Vec3,
        speed: f32,
        dt: f32,
        s: &EffectSettings,
    ) {
        let rate = 110.0 * ((speed - 6.0) / 9.0).clamp(0.0, 1.0);
        if rate <= 0.0 || dt <= 0.0 {
            return;
        }
        let n = (rate * dt + self.rand()).floor() as u32;
        let ahead = heading.normalize_or(Vec3::X);
        let back = (-ahead * 0.6 - Vec3::Y * 0.5).normalize_or(-ahead);
        for _ in 0..n.min(10) {
            let dir = self.cone(back, 0.9);
            let v = ahead * speed * self.range(0.55, 0.85) + dir * self.range(1.0, 4.5);
            let size = self.range(0.007, 0.013);
            let seed = self.rand();
            let heat = self.range(3800.0, 5600.0);
            let lum = s.spark_nits * self.range(0.5, 1.0);
            let life = self.range(0.12, 0.35);
            let jitter = self.unit() * 0.01;
            self.push(Particle {
                kind: ParticleKind::Spark,
                pos: at + jitter.as_dvec3(),
                vel: v,
                age: 0.0,
                life,
                size,
                end_size: size * 0.5,
                color: Vec3::ONE,
                nits: lum,
                drag: 2.5,
                gravity: 1.0,
                heat,
                floor: at.y - 50.0,
                seed,
            });
        }
    }

    /// Advance everything by `dt` world seconds (and `real_dt` real seconds).
    pub fn step(&mut self, dt: f32, gravity: Vec3) {
        if dt <= 0.0 {
            return;
        }
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.age += dt;
            if p.age >= p.life {
                self.particles.swap_remove(i);
                continue;
            }
            p.vel += gravity * (p.gravity * dt);
            p.vel *= (-p.drag * dt).exp();
            p.pos += (p.vel * dt).as_dvec3();
            if p.pos.y < p.floor && p.vel.y < 0.0 {
                // Bounce and skid.
                p.pos.y = p.floor;
                p.vel.y = -p.vel.y * 0.3;
                p.vel.x *= 0.55;
                p.vel.z *= 0.55;
            }
            i += 1;
        }
        self.slashes.retain_mut(|s| {
            s.age += dt;
            s.age < s.life
        });
        self.rings.retain_mut(|r| {
            r.age += dt;
            r.age < r.life
        });
        for l in &mut self.lights {
            l.age = (l.age + dt).min(l.life);
        }
    }
}

// ============================================================================ systems

/// Advance effects on the world clock and drive the flash lights (PostUpdate).
pub(crate) fn step_effects(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<EffectSettings>,
    gravity: Option<Res<Gravity>>,
    mut fx: ResMut<Effects>,
    mut lights: Query<(&mut PointLight, &mut Transform, &mut GlobalTransform)>,
    mut diagnostics: Option<ResMut<Diagnostics>>,
) {
    fx.now = time.elapsed_secs_f64();
    fx.real_now = time.real_elapsed_secs_f64();
    fx.max_particles = settings.max_particles.max(16) as usize;
    if fx.rng == 0 {
        fx.rng = 0x9e37_79b9;
    }
    if !settings.enabled {
        fx.particles.clear();
        fx.slashes.clear();
        fx.rings.clear();
        fx.trails.clear();
        for l in &mut fx.lights {
            l.age = l.life;
        }
    }
    let g = gravity.map_or(Vec3::new(0.0, -9.81, 0.0), |g| g.uniform);
    fx.step(time.delta_secs(), g);
    let max = fx.max_particles;
    fx.particles.truncate(max);
    for l in &mut fx.lights {
        let t = (l.age / l.life.max(1e-3)).clamp(0.0, 1.0);
        let on = t < 1.0;
        let lm = if on {
            l.peak_lm * (1.0 - t) * (1.0 - t)
        } else {
            0.0
        };
        let light = PointLight {
            color: l.color,
            intensity_lm: lm,
            range: if on { 6.0 } else { 0.01 },
            radius: 0.15,
        };
        let pos = if on { l.pos } else { l.pos - DVec3::Y * 1000.0 };
        match l.entity {
            Some(e) => {
                if let Ok((mut pl, mut t, mut gt)) = lights.get_mut(e) {
                    if *pl != light {
                        *pl = light;
                    }
                    if t.translation != pos {
                        t.translation = pos;
                        *gt = GlobalTransform::from(*t);
                    }
                }
            }
            None if on => {
                let t = Transform::from_translation(pos);
                l.entity = Some(
                    commands
                        .spawn((
                            Name::new("Effect flash light"),
                            light,
                            t,
                            GlobalTransform::from(t),
                        ))
                        .id(),
                );
            }
            None => {}
        }
    }
    if let Some(d) = diagnostics.as_deref_mut() {
        d.set("movement.effects.particles", fx.particles.len() as f64);
        d.set("movement.effects.trails", fx.trails.len() as f64);
    }
}

/// How strongly a fighter's blade trails right now (0..1), from its attack.
fn swing_glow(cs: Option<&CombatState>, fighter: Option<&Fighter>, s: &EffectSettings) -> f32 {
    let Some(cs) = cs else { return 0.0 };
    if cs.defeated {
        return 0.0;
    }
    let Some(a) = cs.attack else { return 0.0 };
    let f = fighter.copied().unwrap_or_default();
    match a.phase {
        // The blade is carried, not swung: the afterimages show the dash.
        AttackPhase::DashIn => 0.0,
        AttackPhase::Windup => {
            let total = if a.kind == engine_world::AttackKind::Dash {
                f.dash_windup_secs
            } else {
                f.windup_secs
            };
            // Only the very end of the backswing, faintly: the cut is the strike.
            let end = total * s.trail_windup_fraction;
            if a.secs_left <= end {
                0.5 * (1.0 - a.secs_left / end.max(1e-4)).clamp(0.0, 1.0)
            } else {
                0.0
            }
        }
        AttackPhase::Strike => 1.0,
        AttackPhase::Recovery => {
            let into = f.recovery_secs - a.secs_left;
            (1.0 - into / s.trail_recovery_secs.max(1e-4)).clamp(0.0, 1.0)
        }
    }
}

/// Sample every blade after transforms settle (PostUpdate) and keep each trail's history.
pub(crate) fn sample_blades(
    time: Res<Time>,
    settings: Res<EffectSettings>,
    mut fx: ResMut<Effects>,
    blades: Query<(Entity, &Blade, &GlobalTransform)>,
    transforms: Query<&GlobalTransform>,
    owners: Query<(Option<&CombatState>, Option<&Fighter>)>,
) {
    if !settings.enabled {
        return;
    }
    let now = time.elapsed_secs_f64();
    let real_now = time.real_elapsed_secs_f64();
    let real_dt = time.real_delta_secs();
    let advanced = time.delta_secs() > 0.0;
    let window = settings.trail_secs.max(0.01) as f64;
    for t in &mut fx.trails {
        t.seen = false;
    }
    for (entity, blade, gt) in &blades {
        let Ok(tip) = transforms.get(blade.tip) else {
            continue;
        };
        let (base, tip) = (gt.translation(), tip.translation());
        let anchor = transforms
            .get(blade.owner)
            .map_or(base, |o| o.translation());
        let (cs, fighter) = owners.get(blade.owner).unwrap_or((None, None));
        let idx = match fx.trails.iter().position(|t| t.blade == entity) {
            Some(i) => i,
            None => {
                fx.trails.push(Trail {
                    blade: entity,
                    color: blade.color,
                    samples: VecDeque::with_capacity(MAX_TRAIL_SAMPLES),
                    anchor,
                    fast_frames: 0,
                    seen: true,
                });
                fx.trails.len() - 1
            }
        };
        let trail = &mut fx.trails[idx];
        trail.seen = true;
        trail.color = blade.color;
        trail.anchor = anchor;
        if let Some(last) = trail.samples.back().copied() {
            let moved = (base - anchor).distance(last.base - last.anchor);
            if moved > TELEPORT_M || last.anchor.distance(anchor) > OWNER_TELEPORT_M {
                trail.samples.clear();
                trail.fast_frames = 0;
            } else if !advanced {
                // Paused (or time stopped): keep the trail as it is.
                continue;
            }
        }
        let mut glow = swing_glow(cs, fighter, &settings);
        let attacking = cs.is_some_and(|c| c.attack.is_some() || c.defeated);
        if let Some(last) = trail.samples.back()
            && real_dt > 0.0
            && !attacking
        {
            // Outside attacks (which decide for themselves): sweeping fast around the base, not
            // just carried along, for more than a frame (a pose that snaps is not a swing).
            let sweep = ((tip - base) - (last.tip - last.base)).length() as f32 / real_dt;
            let fast = ((sweep - settings.trail_sweep_speed) / settings.trail_sweep_speed.max(0.1))
                .clamp(0.0, 1.0);
            trail.fast_frames = if fast > 0.0 {
                trail.fast_frames.saturating_add(1)
            } else {
                0
            };
            if trail.fast_frames >= 2 {
                glow = glow.max(fast * 0.6);
            }
        }
        if trail.samples.len() == MAX_TRAIL_SAMPLES {
            trail.samples.pop_front();
        }
        trail.samples.push_back(TrailSample {
            base,
            tip,
            anchor,
            world_secs: now,
            real_secs: real_now,
            glow,
        });
        while trail.samples.len() > 2
            && (now - trail.samples[1].world_secs > window
                || real_now - trail.samples[1].real_secs > MAX_TRAIL_REAL_SECS)
        {
            trail.samples.pop_front();
        }
    }
    fx.trails.retain(|t| t.seen);
}

/// Sparks, slash flashes, rings and lights for new strikes in the combat log (PostUpdate).
#[allow(clippy::type_complexity)]
pub(crate) fn combat_effects(
    time: Res<Time>,
    settings: Res<EffectSettings>,
    log: Res<CombatLog>,
    mut fx: ResMut<Effects>,
    bodies: Query<(
        &GlobalTransform,
        Option<&CombatState>,
        Option<&CharacterController>,
    )>,
    blades: Query<(&Blade, &GlobalTransform)>,
    transforms: Query<&GlobalTransform>,
) {
    let records = &log.records;
    let start = match fx.last_record {
        Some(last) => records
            .iter()
            .rposition(|r| *r == last)
            .map_or(0, |i| i + 1),
        None => 0,
    };
    if let Some(last) = records.back() {
        fx.last_record = Some(*last);
    }
    if !settings.enabled {
        return;
    }
    let real_now = time.real_elapsed_secs_f64();
    for r in records.range(start..) {
        // Never replay old history (a late start, an evicted marker).
        if real_now - r.real_secs > 0.5 {
            continue;
        }
        let (Some(attacker), Some(target)) = (r.attacker, r.target) else {
            continue;
        };
        let Ok((tgt, tgt_cs, tgt_cc)) = bodies.get(target) else {
            continue;
        };
        let tp = tgt.translation();
        let ap = bodies
            .get(attacker)
            .map_or(tp - DVec3::Z, |b| b.0.translation());
        let dir = flat((tp - ap).as_vec3(), Vec3::Y).normalize_or(Vec3::Z);
        let half_height = tgt_cc.map_or(0.9, |c| c.height * 0.5);
        let radius = tgt_cc.map_or(0.35, |c| c.radius);
        let floor = tp.y - half_height as f64;
        let chest = tp + DVec3::Y * (half_height as f64 * 0.35);
        let reach = flat((tp - ap).as_vec3(), Vec3::Y).length();
        match r.outcome {
            Outcome::Blocked => {
                // Where the blades meet: along the defender's blade if it has one.
                let contact = blades
                    .iter()
                    .find(|(b, _)| b.owner == target)
                    .and_then(|(b, base)| {
                        let tip = transforms.get(b.tip).ok()?.translation();
                        Some(base.translation().lerp(tip, 0.62))
                    })
                    .unwrap_or(chest - (dir * (radius + 0.25)).as_dvec3());
                fx.parry(contact, dir, floor, &settings);
            }
            Outcome::Hit | Outcome::Counter => {
                let killed = tgt_cs.is_some_and(|c| c.defeated);
                let impact = if killed {
                    Impact::Kill
                } else if r.outcome == Outcome::Counter {
                    Impact::Counter
                } else {
                    Impact::Hit
                };
                fx.hit(chest, dir, reach, impact, &settings);
            }
            Outcome::Dodged | Outcome::Whiff => {}
        }
    }
}

/// Dust from ground contacts, kick-offs and slides of characters the movement module moves
/// (PostUpdate).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn dust_effects(
    time: Res<Time>,
    settings: Res<EffectSettings>,
    mut fx: ResMut<Effects>,
    mut contacts: MessageReader<GroundContact>,
    mut triggers: MessageReader<AnimationTrigger>,
    movers: Query<(
        Entity,
        &Mover,
        &GlobalTransform,
        &CharacterController,
        Option<&MovementProfile>,
    )>,
    mut scratch: Local<Vec<(Entity, GroundContact)>>,
    mut slides: Local<Vec<(Entity, f32)>>,
) {
    if !settings.enabled || !settings.dust {
        contacts.clear();
        triggers.clear();
        slides.clear();
        return;
    }
    // A slide sprays dust along the ground the whole way: a puff every SLIDE_DUST_M.
    let dt = time.delta_secs();
    let sliding = |m: &Mover| m.mode == LocomotionMode::Slide && m.grounded;
    slides.retain(|(e, _)| movers.get(*e).is_ok_and(|(_, m, ..)| sliding(m)));
    for (e, m, gt, cc, _) in &movers {
        if !sliding(m) {
            continue;
        }
        let i = match slides.iter().position(|(s, _)| *s == e) {
            Some(i) => i,
            None => {
                slides.push((e, 0.0));
                slides.len() - 1
            }
        };
        let v = flat(m.velocity, m.up);
        let speed = v.length();
        slides[i].1 += speed * dt;
        let feet = gt.translation() - (m.up * cc.height * 0.5).as_dvec3();
        let mut puffs = 0;
        while slides[i].1 >= SLIDE_DUST_M && puffs < 3 {
            slides[i].1 -= SLIDE_DUST_M;
            puffs += 1;
            fx.dust_burst(
                feet,
                v.normalize_or(m.facing),
                1,
                0.45 + speed / 24.0,
                &settings,
            );
        }
        slides[i].1 = slides[i].1.min(SLIDE_DUST_M);
    }

    scratch.clear();
    scratch.extend(
        contacts
            .read()
            .filter_map(|c| c.entity.filter(|e| movers.contains(*e)).map(|e| (e, *c))),
    );
    // Landings write both feet in the same step; footsteps one at a time.
    scratch.sort_by_key(|(e, _)| *e);
    let mut i = 0;
    while i < scratch.len() {
        let (e, c) = scratch[i];
        let mut j = i + 1;
        while j < scratch.len() && scratch[j].0 == e {
            j += 1;
        }
        let group = j - i;
        let Ok((_, m, gt, cc, profile)) = movers.get(e) else {
            i = j;
            continue;
        };
        let mass = profile.map_or(80.0, |p| p.mass_kg).max(1.0);
        let forward = flat(m.velocity, m.up).normalize_or(m.facing);
        let wall = c.normal.dot(m.up).abs() < 0.5;
        match c.kind {
            ContactKind::Body => {
                fx.dust_burst(c.position, forward, 2, 0.6 + c.speed / 20.0, &settings);
            }
            ContactKind::Foot if wall => {
                fx.wall_dust(c.position, c.normal, 1, 0.5, &settings);
            }
            ContactKind::Foot if group >= 2 => {
                // load = half the weight × (1 + impact / 4 m/s) per foot.
                let impact = ((c.load_kg / (0.5 * mass) - 1.0) * 4.0).max(0.0);
                if impact > 1.5 {
                    let feet = gt.translation() - (m.up * cc.height * 0.5).as_dvec3();
                    fx.dust_ring(feet, (impact - 1.5) / 8.0, &settings);
                }
            }
            ContactKind::Foot if c.speed > 7.5 => {
                fx.dust_burst(c.position, forward, 1, 0.35, &settings);
            }
            _ => {}
        }
        i = j;
    }
    for t in triggers.read() {
        let Ok((_, m, gt, cc, _)) = movers.get(t.entity) else {
            continue;
        };
        let feet = gt.translation() - (m.up * cc.height * 0.5).as_dvec3();
        match t.name.as_str() {
            "dash" => {
                let dir = m
                    .dash
                    .map(|d| d.dir)
                    .unwrap_or_else(|| m.velocity.normalize_or(m.facing));
                if m.grounded {
                    fx.dust_burst(feet, dir, 9, 1.0, &settings);
                    fx.dust_ring(feet, 0.25, &settings);
                } else {
                    // An air dash pushes off the air itself: a faint shock ring.
                    fx.rings.push(Ring {
                        center: gt.translation() - (dir * 0.3).as_dvec3(),
                        normal: dir,
                        from: 0.2,
                        to: 1.3,
                        width: 0.08,
                        age: 0.0,
                        life: 0.2,
                        body: Vec3::splat(settings.trail_nits * 0.12),
                        core: Vec3::splat(settings.trail_nits * 0.35),
                    });
                }
            }
            "slide" => {
                let dir = flat(m.velocity, m.up).normalize_or(m.facing);
                fx.dust_burst(feet, dir, 10, 1.1, &settings);
            }
            "wall_jump" => {
                // The kick pushes away from the wall: the wall is behind the new velocity.
                let away = flat(m.velocity, m.up).normalize_or(-m.facing);
                let at = gt.translation() - (away * cc.radius).as_dvec3();
                fx.wall_dust(at, away, 8, 1.2, &settings);
            }
            _ => {}
        }
    }
}

// ============================================================================ render feature

/// GPU layout of a ghost instance (locations 4..9 in `effects.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, Default)]
struct GhostInstance {
    /// Camera-relative model matrix rows.
    m: [[f32; 4]; 3],
    /// Rim radiance (cd/m²) and body opacity.
    rim: [f32; 4],
    /// Body emission (cd/m²) and age 0..1.
    body: [f32; 4],
    /// Lit tint (albedo) and seed.
    albedo: [f32; 4],
}

impl GhostInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
        7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

struct GhostMesh {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    count: u32,
    version: u64,
}

/// One instanced ghost draw: a mesh, mirrored or not, a range of instances.
#[derive(Debug, Clone, Copy)]
struct GhostBatch {
    mesh: u32,
    mirrored: bool,
    first: u32,
    count: u32,
}

/// Draws trails, flashes, rings, particles and afterimage ghosts (added by the plugin).
#[derive(Default)]
pub(crate) struct EffectsFeature {
    ribbon_pipeline: Option<wgpu::RenderPipeline>,
    particle_pipeline: Option<wgpu::RenderPipeline>,
    /// Counter-clockwise and clockwise (mirrored) front faces.
    ghost_pipelines: [Option<wgpu::RenderPipeline>; 2],
    layout: Option<wgpu::BindGroupLayout>,
    group: Option<wgpu::BindGroup>,
    noise: Option<(wgpu::Texture, wgpu::TextureView)>,
    ribbon_vertices: Option<GrowBuffer>,
    ribbon_indices: Option<GrowBuffer>,
    particles: Option<GrowBuffer>,
    ghost_instances: Option<GrowBuffer>,
    ghost_meshes: HashMap<u32, GhostMesh>,
    ghost_query: Option<QueryState<&'static Afterimage>>,
    ribbon: RibbonMesh,
    particle_data: Vec<ParticleInstance>,
    dust_sort: Vec<(f32, ParticleInstance)>,
    ghost_items: Vec<(u32, bool, GhostInstance)>,
    ghost_data: Vec<GhostInstance>,
    ghost_batches: Vec<GhostBatch>,
    index_count: u32,
    particle_count: u32,
    dust_count: u32,
    stats: FeatureStats,
}

/// Size of the baked noise texture, texels per side.
const NOISE_SIZE: u32 = 128;

/// Tiling noise for dust and trails: R cloudy fBm, G billows (inverted cells with detail),
/// B fine fBm, A ridges.
pub fn bake_noise(size: u32) -> Vec<u8> {
    let n = size.max(8);
    let mut out = vec![0u8; (n * n * 4) as usize];
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for y in 0..n {
        for x in 0..n {
            let uv = engine_core::math::Vec2::new(
                (x as f32 + 0.5) / n as f32,
                (y as f32 + 0.5) / n as f32,
            );
            let cloud = texgen::fbm(uv * 4.0, 4, 5, 0.55, 11) * 0.5 + 0.5;
            let (d1, d2, _) = texgen::worley(uv * 5.0, 5, 3);
            let cells = (1.0 - d1 * 1.25).clamp(0.0, 1.0) * 0.65 + (d2 - d1).clamp(0.0, 1.0) * 0.35;
            let billow = (cells * 0.7 + cloud * 0.5).clamp(0.0, 1.0);
            let fine = texgen::fbm(uv * 8.0, 8, 4, 0.5, 29) * 0.5 + 0.5;
            let ridge = texgen::ridged(uv * 6.0, 6, 4, 7);
            let i = ((y * n + x) * 4) as usize;
            out[i] = to8(cloud);
            out[i + 1] = to8(billow);
            out[i + 2] = to8(fine);
            out[i + 3] = to8(ridge);
        }
    }
    out
}

impl EffectsFeature {
    fn upload_ghost_mesh(&mut self, device: &wgpu::Device, index: u32, mesh: &Mesh, version: u64) {
        if mesh.indices.is_empty() || mesh.vertices.is_empty() {
            self.ghost_meshes.remove(&index);
            return;
        }
        let vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("movement ghost vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("movement ghost indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.ghost_meshes.insert(
            index,
            GhostMesh {
                vertex,
                index: index_buffer,
                count: mesh.indices.len() as u32,
                version,
            },
        );
    }

    fn prepare_particles(&mut self, fx: &Effects, view: &ViewData, k: f32, time_scale: f32) {
        self.particle_data.clear();
        self.dust_sort.clear();
        for p in &fx.particles {
            let t = (p.age / p.life.max(1e-4)).clamp(0.0, 1.0);
            let rel = (p.pos - view.origin).as_vec3();
            if !view
                .frustum
                .intersects_sphere(rel, p.size.max(p.end_size) * 2.0 + 0.5)
            {
                continue;
            }
            let mut inst = ParticleInstance {
                params: [p.kind as u32 as f32, p.seed, t, 0.0],
                ..Default::default()
            };
            match p.kind {
                ParticleKind::Spark => {
                    let cool = 1.0 - 0.72 * t.powf(0.6);
                    let temp = p.heat * cool;
                    let lum = p.nits * cool.powi(3) * (1.0 - t).sqrt() * k;
                    let c = blackbody(temp) * lum;
                    let speed = p.vel.length();
                    let dir = if speed > 1e-3 { p.vel / speed } else { Vec3::Y };
                    let size = p.size + (p.end_size - p.size) * t;
                    let len = speed * time_scale * SHUTTER_SECS;
                    inst.pos_size = [rel.x, rel.y, rel.z, size];
                    inst.color = [c.x, c.y, c.z, 0.0];
                    inst.axis = [dir.x, dir.y, dir.z, len];
                }
                ParticleKind::Flare | ParticleKind::Mote => {
                    let fade = if p.kind == ParticleKind::Flare {
                        (1.0 - t) * (1.0 - t)
                    } else {
                        (1.0 - t).powf(1.5)
                    };
                    let c = p.color * (p.nits * fade * k);
                    let size = p.size + (p.end_size - p.size) * t;
                    inst.pos_size = [rel.x, rel.y, rel.z, size];
                    inst.color = [c.x, c.y, c.z, 0.0];
                }
                ParticleKind::Dust => {
                    let grow = 1.0 - (1.0 - t).powf(2.5);
                    let size = p.size + (p.end_size - p.size) * grow;
                    let alpha = p.nits * smooth(0.0, 0.08, t) * (1.0 - t).powf(1.6);
                    inst.pos_size = [rel.x, rel.y, rel.z, size];
                    inst.color = [p.color.x, p.color.y, p.color.z, alpha];
                    self.dust_sort.push((rel.length_squared(), inst));
                    continue;
                }
            }
            self.particle_data.push(inst);
        }
        // Dust is blended over what is behind it: farthest first, before the glowing kinds.
        self.dust_sort.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.dust_count = self.dust_sort.len() as u32;
        let glowing = self.particle_data.len();
        self.particle_data
            .extend(self.dust_sort.iter().map(|(_, p)| *p));
        self.particle_data.rotate_right(self.dust_sort.len());
        debug_assert_eq!(self.particle_data.len(), glowing + self.dust_sort.len());
    }

    fn prepare_ghosts(&mut self, world: &World, gpu: &GpuShared, view: &ViewData, k: f32) {
        self.ghost_items.clear();
        self.ghost_data.clear();
        let Some(meshes) = world.get_resource::<Assets<Mesh>>() else {
            return;
        };
        let Some(mut query) = self.ghost_query.take() else {
            return;
        };
        for ghost in query.iter(world) {
            let Some(style) = ghost.draw_style() else {
                continue;
            };
            let anchor = (ghost.anchor - view.origin).as_vec3();
            let t = style.age01;
            // Fade in over the first frames (a fresh ghost sits right on the body it copies)
            // and out over the rest.
            // Hidden while the body it copies still overlaps it.
            let apart = ghost
                .source
                .and_then(|s| world.get::<GlobalTransform>(s))
                .map_or(1.0, |g| {
                    smooth(0.2, 0.65, g.translation().distance(ghost.anchor) as f32)
                });
            let fade = smooth(0.0, 0.07, t) * (1.0 - t).powi(2) * apart;
            if fade <= 1e-3 {
                continue;
            }
            let color = vivid(style.color);
            let rim = color * (style.rim_nits * fade * k);
            let body = color * ((style.glow + style.rim_nits * 0.07) * fade * k);
            // Ghosts swell a little as they dissipate.
            let swell = Mat4::from_scale(Vec3::splat(1.0 + 0.05 * t));
            for part in &ghost.parts {
                let Some(handle) = part
                    .mesh
                    .or_else(|| world.get::<ResolvedMesh>(part.source).map(|r| r.mesh))
                else {
                    continue;
                };
                let handle = meshes.canonical(handle);
                let index = handle.index();
                let version = meshes.version(handle);
                if version == 0 {
                    continue;
                }
                if self
                    .ghost_meshes
                    .get(&index)
                    .is_none_or(|m| m.version != version)
                    && let Some(mesh) = meshes.get(handle)
                {
                    self.upload_ghost_mesh(&gpu.device, index, mesh, version);
                }
                if !self.ghost_meshes.contains_key(&index) {
                    continue;
                }
                let model = Mat4::from_translation(anchor) * swell * part.transform;
                let mirrored = engine_core::math::Mat3::from_mat4(model).determinant() < 0.0;
                self.ghost_items.push((
                    index,
                    mirrored,
                    GhostInstance {
                        m: [
                            model.row(0).to_array(),
                            model.row(1).to_array(),
                            model.row(2).to_array(),
                        ],
                        rim: [rim.x, rim.y, rim.z, style.alpha * fade],
                        body: [body.x, body.y, body.z, t],
                        albedo: [color.x, color.y, color.z, style.seed],
                    },
                ));
            }
        }
        self.ghost_query = Some(query);
        self.ghost_items
            .sort_by_key(|(mesh, mirrored, _)| (*mirrored, *mesh));
        self.ghost_batches.clear();
        for (mesh, mirrored, inst) in &self.ghost_items {
            let first = self.ghost_data.len() as u32;
            self.ghost_data.push(*inst);
            match self.ghost_batches.last_mut() {
                Some(b) if b.mesh == *mesh && b.mirrored == *mirrored => b.count += 1,
                _ => self.ghost_batches.push(GhostBatch {
                    mesh: *mesh,
                    mirrored: *mirrored,
                    first,
                    count: 1,
                }),
            }
        }
    }
}

/// An effect color from a linear RGB tint: saturated (brightest channel at 1), then evened
/// out so pale colors (cyan, white) don't glow much brighter than deep ones (crimson).
fn vivid(rgb: [f32; 3]) -> Vec3 {
    let c = Vec3::from(rgb).max(Vec3::ZERO);
    let c = c / c.max_element().max(1e-4);
    let luminance = c.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-3);
    c * (0.35 / luminance).clamp(0.45, 1.4)
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl RenderFeature for EffectsFeature {
    fn name(&self) -> &str {
        "movement_effects"
    }

    /// After meshes and fire, so effects glow over them.
    fn order(&self) -> i32 {
        60
    }

    fn needs_scene_copy(&self) -> bool {
        self.dust_count > 0
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("movement effects"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let size = wgpu::Extent3d {
            width: NOISE_SIZE,
            height: NOISE_SIZE,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("movement effects noise"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &bake_noise(NOISE_SIZE),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(NOISE_SIZE * 4),
                rows_per_image: Some(NOISE_SIZE),
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        self.noise = Some((texture, view));

        let shader = gpu.shader("movement effects", SHADER, true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("movement effects"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        primitive: wgpu::PrimitiveState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(false)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: Some(premultiplied),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let no_cull = wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        };
        self.ribbon_pipeline = Some(pipeline(
            "movement ribbons",
            "vs_ribbon",
            "fs_ribbon",
            &[Some(RibbonVertex::layout())],
            no_cull,
        ));
        self.particle_pipeline = Some(pipeline(
            "movement particles",
            "vs_particle",
            "fs_particle",
            &[],
            no_cull,
        ));
        for (i, front) in [wgpu::FrontFace::Ccw, wgpu::FrontFace::Cw]
            .into_iter()
            .enumerate()
        {
            self.ghost_pipelines[i] = Some(pipeline(
                "movement ghosts",
                "vs_ghost",
                "fs_ghost",
                &[Some(Vertex::layout()), Some(GhostInstance::layout())],
                wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    front_face: front,
                    ..Default::default()
                },
            ));
        }
        self.ribbon_vertices = Some(GrowBuffer::new(
            device,
            "movement ribbon vertices",
            wgpu::BufferUsages::VERTEX,
            4096 * std::mem::size_of::<RibbonVertex>() as u64,
        ));
        self.ribbon_indices = Some(GrowBuffer::new(
            device,
            "movement ribbon indices",
            wgpu::BufferUsages::INDEX,
            8192 * 4,
        ));
        self.particles = Some(GrowBuffer::new(
            device,
            "movement particles",
            wgpu::BufferUsages::STORAGE,
            1024 * std::mem::size_of::<ParticleInstance>() as u64,
        ));
        self.ghost_instances = Some(GrowBuffer::new(
            device,
            "movement ghost instances",
            wgpu::BufferUsages::VERTEX,
            512 * std::mem::size_of::<GhostInstance>() as u64,
        ));
        self.layout = Some(layout);
        self.group = None;
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        // The scene depth copy is recreated with the other size-dependent targets.
        self.group = None;
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.index_count = 0;
        self.particle_count = 0;
        self.dust_count = 0;
        self.ghost_batches.clear();
        self.stats = FeatureStats::default();
        if self.ribbon_pipeline.is_none() {
            return;
        }
        let settings = world
            .get_resource::<EffectSettings>()
            .copied()
            .unwrap_or_default();
        let k = settings.exposure_scale(view.ev100);
        let time_scale = world.get_resource::<Time>().map_or(1.0, |t| t.scale());
        if self.ghost_query.is_none() {
            self.ghost_query = Some(world.query::<&Afterimage>());
        }
        let world_ref: &World = world;

        // Ribbons and particles.
        self.ribbon.clear();
        self.particle_data.clear();
        if settings.enabled
            && let Some(fx) = world_ref.get_resource::<Effects>()
        {
            for (i, trail) in fx.trails.iter().enumerate() {
                let c = vivid(trail.color);
                let style = TrailStyle {
                    window: settings.trail_secs,
                    anchor: trail.anchor,
                    follow: settings.trail_follow,
                    body: c * (settings.trail_nits * k),
                    core: c.lerp(Vec3::ONE, 0.82) * (settings.core_nits * k),
                    seed: (i as f32 * 0.618_034).fract(),
                };
                build_trail(
                    &trail.samples,
                    fx.now,
                    view.origin,
                    &style,
                    &mut self.ribbon,
                );
            }
            let scaled = |c: Vec3| c * k;
            for s in &fx.slashes {
                if s.age < 0.0 {
                    continue;
                }
                let s = Slash {
                    body: scaled(s.body),
                    core: scaled(s.core),
                    ..*s
                };
                build_slash(&s, view.origin, &mut self.ribbon);
            }
            for r in &fx.rings {
                let r = Ring {
                    body: scaled(r.body),
                    core: scaled(r.core),
                    ..*r
                };
                build_ring(&r, view.origin, &mut self.ribbon);
            }
            self.prepare_particles(fx, view, k, time_scale);
        }

        // Afterimage ghosts.
        self.prepare_ghosts(world_ref, gpu, view, settings.exposure_scale(view.ev100));

        // Upload.
        let device = &gpu.device;
        let queue = &gpu.queue;
        if !self.ribbon.indices.is_empty()
            && let (Some(v), Some(i)) = (&mut self.ribbon_vertices, &mut self.ribbon_indices)
        {
            v.write(device, queue, bytemuck::cast_slice(&self.ribbon.vertices));
            i.write(device, queue, bytemuck::cast_slice(&self.ribbon.indices));
            self.index_count = self.ribbon.indices.len() as u32;
        }
        if !self.particle_data.is_empty()
            && let Some(p) = &mut self.particles
        {
            if p.write(device, queue, bytemuck::cast_slice(&self.particle_data)) {
                self.group = None;
            }
            self.particle_count = self.particle_data.len() as u32;
        }
        if !self.ghost_data.is_empty()
            && let Some(g) = &mut self.ghost_instances
        {
            g.write(device, queue, bytemuck::cast_slice(&self.ghost_data));
        }
        let draws = (self.index_count > 0) as u32
            + (self.particle_count > 0) as u32
            + self.ghost_batches.len() as u32;
        self.stats = FeatureStats {
            draw_calls: draws,
            instances: self.particle_count + self.ghost_data.len() as u32,
            triangles: (self.index_count / 3) as u64 + self.particle_count as u64 * 2,
            culled: 0,
        };
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if self.group.is_none()
            && let (Some(layout), Some(noise), Some(particles)) =
                (&self.layout, &self.noise, &self.particles)
        {
            self.group = Some(
                ctx.gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("movement effects"),
                        layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&noise.1),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: particles.buffer.as_entire_binding(),
                            },
                        ],
                    }),
            );
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let Some(group) = &self.group else {
            return;
        };
        pass.set_bind_group(1, group, &[]);
        if !self.ghost_batches.is_empty()
            && let Some(instances) = &self.ghost_instances
        {
            pass.set_vertex_buffer(1, instances.buffer.slice(..));
            for b in &self.ghost_batches {
                let (Some(pipeline), Some(mesh)) = (
                    &self.ghost_pipelines[b.mirrored as usize],
                    self.ghost_meshes.get(&b.mesh),
                ) else {
                    continue;
                };
                pass.set_pipeline(pipeline);
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, b.first..b.first + b.count);
            }
        }
        if self.particle_count > 0
            && let Some(pipeline) = &self.particle_pipeline
        {
            pass.set_pipeline(pipeline);
            pass.draw(0..6, 0..self.particle_count);
        }
        if self.index_count > 0
            && let (Some(pipeline), Some(v), Some(i)) = (
                &self.ribbon_pipeline,
                &self.ribbon_vertices,
                &self.ribbon_indices,
            )
        {
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, v.buffer.slice(..));
            pass.set_index_buffer(i.buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "ribbon_triangles": self.index_count / 3,
            "particles": self.particle_count,
            "dust": self.dust_count,
            "ghost_batches": self.ghost_batches.len(),
            "ghost_meshes": self.ghost_meshes.len(),
        })
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<EffectSettings>()
        .init_resource::<Effects>()
        .register_resource::<EffectSettings>(
            "Combat and movement effects: blade trails (world seconds shown, cd/m²), parry \
             sparks, slash flashes, impact lights (lm), dust, particle cap, exposure adapt.",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(base: Vec3, tip: Vec3, secs: f64, glow: f32) -> TrailSample {
        TrailSample {
            base: base.as_dvec3(),
            tip: tip.as_dvec3(),
            anchor: DVec3::ZERO,
            world_secs: secs,
            real_secs: secs,
            glow,
        }
    }

    fn style() -> TrailStyle {
        TrailStyle {
            window: 0.14,
            anchor: DVec3::ZERO,
            follow: 0.8,
            body: Vec3::new(1.0, 0.1, 0.1),
            core: Vec3::ONE,
            seed: 0.0,
        }
    }

    #[test]
    fn a_blink_fast_swing_draws_an_arc_not_a_chord() {
        // The blade turns 120° around its base between two frames.
        let base = Vec3::new(0.0, 1.0, 0.0);
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Quat::from_rotation_y(120f32.to_radians()) * a;
        let samples: VecDeque<_> = [
            sample(base, base + a, 0.0, 1.0),
            sample(base, base + b, 1.0 / 60.0, 1.0),
        ]
        .into();
        let mut out = RibbonMesh::default();
        let quads = build_trail(&samples, 1.0 / 60.0, DVec3::ZERO, &style(), &mut out);
        assert!(quads >= 20, "subdivided by angle: {quads} quads");
        // Every tip vertex sits on the circle the tip sweeps, so the middle bulges out.
        let mut farthest_from_chord: f32 = 0.0;
        let chord_mid = base + (a + b) * 0.5;
        for v in out.vertices.iter().filter(|v| v.uv_fade_seed[1] == 1.0) {
            let r = (v.position() - base).length();
            assert!((r - 1.0).abs() < 0.03, "tip at radius {r}");
            farthest_from_chord = farthest_from_chord.max((v.position() - chord_mid).length());
        }
        assert!(farthest_from_chord > 0.45, "{farthest_from_chord}");
        // The newest point is the blade (age 0) and the oldest is older.
        let head = out.vertices.last().unwrap();
        assert_eq!(head.uv_fade_seed[0], 0.0);
        assert!(out.vertices[0].uv_fade_seed[0] > 0.1);
        assert_eq!(out.indices.len(), quads * 6);
        assert!(
            out.indices
                .iter()
                .all(|&i| (i as usize) < out.vertices.len())
        );
    }

    #[test]
    fn a_cut_tapers_along_its_length_and_follows_its_owner() {
        // A quarter swing while the owner dashes 6 m along +X in the same moment.
        let samples: VecDeque<_> = (0..=6)
            .map(|i| {
                let t = i as f32 / 6.0;
                let owner = Vec3::X * 6.0 * t;
                let base = owner + Vec3::Y;
                let dir = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2 * t) * Vec3::Y;
                TrailSample {
                    anchor: owner.as_dvec3(),
                    ..sample(base, base + dir, i as f64 / 600.0, 1.0)
                }
            })
            .collect();
        let now = 6.0 / 600.0;
        let anchor = DVec3::X * 6.0;
        let build = |follow: f32| {
            let mut out = RibbonMesh::default();
            let style = TrailStyle {
                anchor,
                follow,
                ..style()
            };
            assert!(build_trail(&samples, now, DVec3::ZERO, &style, &mut out) > 0);
            out
        };
        // Following the owner fully, the whole cut stays around it: an arc, not a 6 m smear.
        let follow = build(1.0);
        let near = follow
            .vertices
            .iter()
            .all(|v| (v.position() - Vec3::new(6.0, 1.0, 0.0)).length() < 1.05);
        assert!(near, "the cut travels with its owner");
        // Left where it was cut, it spans the dash.
        let left = build(0.0);
        let min_x = left
            .vertices
            .iter()
            .map(|v| v.position().x)
            .fold(f32::MAX, f32::min);
        assert!(min_x < 0.5, "{min_x}");
        // Along the cut: 0 at the blade, 1 at the oldest lit point, increasing backward.
        let back: Vec<f32> = follow
            .vertices
            .iter()
            .step_by(2)
            .map(|v| v.core[3])
            .collect();
        assert_eq!(*back.last().unwrap(), 0.0);
        assert!((back[0] - 1.0).abs() < 1e-4, "{back:?}");
        assert!(back.windows(2).all(|w| w[0] >= w[1]), "{back:?}");
    }

    #[test]
    fn faded_or_idle_parts_of_a_trail_draw_nothing() {
        let base = Vec3::ZERO;
        let tip = Vec3::X;
        let mut samples: VecDeque<_> = (0..10)
            .map(|i| {
                sample(
                    base + Vec3::Z * i as f32 * 0.1,
                    tip + Vec3::Z * i as f32 * 0.1,
                    i as f64 / 60.0,
                    0.0,
                )
            })
            .collect();
        let mut out = RibbonMesh::default();
        assert_eq!(
            build_trail(&samples, 10.0 / 60.0, DVec3::ZERO, &style(), &mut out),
            0
        );
        // Glowing, but long past the window.
        for s in &mut samples {
            s.glow = 1.0;
        }
        out.clear();
        assert_eq!(
            build_trail(&samples, 5.0, DVec3::ZERO, &style(), &mut out),
            0
        );
        // Glowing and fresh: drawn, and fading toward the tail.
        out.clear();
        let quads = build_trail(&samples, 9.0 / 60.0, DVec3::ZERO, &style(), &mut out);
        assert!(quads > 0);
        let ages: Vec<f32> = out
            .vertices
            .iter()
            .step_by(2)
            .map(|v| v.uv_fade_seed[0])
            .collect();
        assert!(ages.windows(2).all(|w| w[0] >= w[1]), "{ages:?}");
    }

    #[test]
    fn ribbons_are_camera_relative() {
        let samples: VecDeque<_> = [
            sample(
                Vec3::new(1000.0, 1.0, 0.0),
                Vec3::new(1001.0, 1.0, 0.0),
                0.0,
                1.0,
            ),
            sample(
                Vec3::new(1000.0, 1.0, 0.0),
                Vec3::new(1000.0, 2.0, 0.0),
                0.01,
                1.0,
            ),
        ]
        .into();
        let mut out = RibbonMesh::default();
        build_trail(
            &samples,
            0.01,
            DVec3::new(1000.0, 0.0, 0.0),
            &style(),
            &mut out,
        );
        assert!(out.vertices.iter().all(|v| v.position().length() < 3.0));
    }

    #[test]
    fn slashes_and_rings_build_closed_strips() {
        let mut out = RibbonMesh::default();
        let s = Slash {
            center: DVec3::new(0.0, 1.2, -5.0),
            tilt: 0.5,
            pivot: Vec3::Z,
            radius: 1.5,
            arc: 1.4,
            width: 0.12,
            lift: 0.3,
            age: 0.1,
            life: 0.3,
            draw_secs: 0.03,
            body: Vec3::X,
            core: Vec3::ONE,
            seed: 0.0,
        };
        build_slash(&s, DVec3::ZERO, &mut out);
        assert!(out.triangle_count() > 20);
        let r = Ring {
            center: DVec3::new(0.0, 1.0, -4.0),
            normal: Vec3::Z,
            from: 0.1,
            to: 1.0,
            width: 0.1,
            age: 0.05,
            life: 0.2,
            body: Vec3::ONE,
            core: Vec3::ONE,
        };
        let before = out.vertices.len();
        build_ring(&r, DVec3::ZERO, &mut out);
        assert!(out.vertices.len() > before);
        assert!(
            out.indices
                .iter()
                .all(|&i| (i as usize) < out.vertices.len())
        );
        // Expired: nothing.
        let n = out.vertices.len();
        build_slash(&Slash { age: 1.0, ..s }, DVec3::ZERO, &mut out);
        assert_eq!(out.vertices.len(), n);
    }

    #[test]
    fn particles_are_capped_and_sparks_fall_and_bounce() {
        let s = EffectSettings::default();
        let mut fx = Effects {
            max_particles: 50,
            ..Default::default()
        };
        fx.parry(DVec3::new(0.0, 1.3, 0.0), Vec3::Z, 0.0, &s);
        assert_eq!(fx.particles.len(), 50);
        assert_eq!(fx.lit_lights(), 1);
        for _ in 0..60 {
            fx.step(1.0 / 60.0, Vec3::new(0.0, -9.81, 0.0));
        }
        assert!(fx.particles.iter().all(|p| p.pos.y >= -1e-6));
        // Everything has burnt out after two seconds.
        for _ in 0..60 {
            fx.step(1.0 / 60.0, Vec3::new(0.0, -9.81, 0.0));
        }
        assert!(fx.particles.is_empty() && fx.slashes.is_empty() && fx.rings.is_empty());
        assert_eq!(fx.lit_lights(), 0);
    }

    #[test]
    fn hot_sparks_are_white_and_cool_ones_red() {
        let hot = blackbody(6500.0);
        let cool = blackbody(1400.0);
        assert!((hot.x - hot.z).abs() < 0.25, "{hot}");
        assert!(cool.x > cool.y * 2.0 && cool.z < 0.05, "{cool}");
    }

    #[test]
    fn noise_tiles_and_varies() {
        let n = bake_noise(32);
        assert_eq!(n.len(), 32 * 32 * 4);
        let min = n.iter().step_by(4).min().unwrap();
        let max = n.iter().step_by(4).max().unwrap();
        assert!(max - min > 60, "{min}..{max}");
    }

    #[test]
    fn shader_validates() {
        let code = engine_render::compose_shader(SHADER, true);
        engine_render::validate_wgsl("movement_effects", &code).unwrap();
    }

    #[test]
    fn layouts() {
        assert_eq!(std::mem::size_of::<RibbonVertex>(), 64);
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 64);
        assert_eq!(std::mem::size_of::<GhostInstance>(), 96);
    }
}
