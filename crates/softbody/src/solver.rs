//! The XPBD particle solver that cloth, ropes, soft volumes and hair all run on.
//!
//! Extended Position-Based Dynamics (Macklin, Müller and Chentanez 2016) with "small steps"
//! (Macklin et al. 2019): each fixed step is split into substeps, and every substep predicts
//! positions, projects each constraint once, resolves collisions and derives velocities. Many
//! cheap substeps converge better than many iterations, keep stiff constraints stiff and make
//! the result independent of the frame rate. Compliance (the inverse of stiffness, m/N) is a
//! physical quantity, so a rope is as stiff at 30 Hz as at 240 Hz.
//!
//! Positions are `f32` relative to [`Sim::origin`] (an `f64` world point), so a cape on a
//! spaceship 10⁸ m from the world origin simulates as precisely as one in the plaza.
//!
//! The solver knows nothing about the ECS. The plugin feeds it frames (bones, entities),
//! colliders and wind each step and reads back positions and reaction impulses.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::ops::Range;

use engine_core::math::{DVec3, IVec3, Mat3, Quat, Vec2, Vec3};
use engine_world::WorldShift;

use crate::collide::{LocalCollider, PosedCollider};
use engine_core::tasks::{TaskPool, TaskPoolBuilder};

/// Air density at sea level, kg/m³.
pub const AIR_DENSITY: f32 = 1.225;

/// Keeps two particles `rest` meters apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    pub a: u32,
    pub b: u32,
    pub rest: f32,
    /// Inverse stiffness, m/N (0 = rigid).
    pub compliance: f32,
    /// Owner-defined tag (the cloth edge it belongs to, for tearing). `u32::MAX` = none.
    pub tag: u32,
}

/// Keeps particle `p` within `max` meters of particle `anchor`: a long-range attachment
/// (Kim, Chentanez and Müller 2012) that stops cloth and hair stretching like rubber when
/// their anchor moves fast, at almost no cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tether {
    pub p: u32,
    pub anchor: u32,
    pub max: f32,
}

/// A kinematic reference frame (a bone, an entity, the world) that pins and goals follow.
/// Interpolated from `from` to `to` across the substeps of one fixed step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub from_pos: Vec3,
    pub from_rot: Quat,
    pub to_pos: Vec3,
    pub to_rot: Quat,
}

impl Frame {
    pub fn at(pos: Vec3, rot: Quat) -> Self {
        Self {
            from_pos: pos,
            from_rot: rot,
            to_pos: pos,
            to_rot: rot,
        }
    }

    /// Pose at fraction `t` (0..1) of the current step.
    #[inline]
    pub fn pose(&self, t: f32) -> (Vec3, Quat) {
        (
            self.from_pos.lerp(self.to_pos, t),
            self.from_rot.slerp(self.to_rot, t).normalize(),
        )
    }

    /// Point `local` (frame space) at fraction `t` of the step.
    #[inline]
    pub fn point(&self, local: Vec3, t: f32) -> Vec3 {
        let (p, r) = self.pose(t);
        p + r * local
    }
}

/// Hard attachment: particle `p` follows `frame` exactly, at `local` in frame space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pin {
    pub p: u32,
    pub frame: u16,
    pub local: Vec3,
}

/// Soft attachment: particle `p` is pulled toward `frame * local` with `compliance` (m/N).
/// The reaction lands in [`Sim::frame_impulses`], so ropes tied to rigid bodies pull them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Goal {
    pub p: u32,
    pub frame: u16,
    pub local: Vec3,
    pub compliance: f32,
}

/// One-sided contact with the skin a strand grows from: particle `p` stays `margin` meters
/// on the outer side of the plane through `point` with unit `normal` (frame space), so fur
/// pressed by wind or inertia lies down on the body instead of sinking into it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinPlane {
    pub p: u32,
    pub frame: u16,
    pub point: Vec3,
    pub normal: Vec3,
    pub margin: f32,
}

/// One-sided contact with a capsule riding a frame, for hair lying on the body it was grown
/// over: particle `p` stays `radius` meters or more from the segment `a`–`b` (frame space), so
/// it may lift off a shoulder but not sink into it. A particle more than `reach` inside is left
/// alone (it came round from elsewhere, through the arm, say).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinCapsule {
    pub p: u32,
    pub frame: u16,
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
    pub reach: f32,
}

/// Shape matching (Müller et al. 2005): particles are pulled toward their rest shape, rotated
/// and translated to best fit where they are now. Gives jelly its wobble and its spring back.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeMatch {
    pub particles: Vec<u32>,
    /// Rest positions relative to the rest center of mass.
    pub rest: Vec<Vec3>,
    /// Inverse stiffness (m/N); 0 = rigid.
    pub compliance: f32,
    /// Deformation beyond `yield_m` creeps into the rest shape at this rate (1/s): dents stay.
    pub plasticity: f32,
    pub yield_m: f32,
    /// Best-fit rotation, warm-started each substep.
    pub rotation: Quat,
}

/// Aerodynamic coefficients for cloth triangles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aero {
    /// Pressure drag coefficient across the cloth (flat plate ≈ 1.2).
    pub drag: f32,
    /// Skin friction along the cloth (streams flags out).
    pub friction: f32,
    /// Lift at an angle to the wind, across it (a sail drawing on a reach, a kite): peaks
    /// with the wind 45° to the cloth; a little on a flag keeps its ripples running. 0 for
    /// curtains.
    pub lift: f32,
}

/// Per-step inputs and tuning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimParams {
    /// Substeps per 1/60 s of simulated time (scaled with the step length, so slow motion
    /// costs less, not more).
    pub substeps: u32,
    /// Gravity, m/s².
    pub gravity: Vec3,
    /// Velocity damping, 1/s.
    pub damping: f32,
    /// Collision thickness of each particle, meters.
    pub radius: f32,
    /// Friction against colliders and the ground.
    pub friction: f32,
    /// Wind at the body this step, m/s.
    pub wind: Vec3,
    /// Linear drag toward the wind velocity, 1/s (ropes, hair).
    pub drag: f32,
    /// Gusty flutter added to the wind per particle and triangle (0..1).
    pub turbulence: f32,
    /// Seconds of simulated time, for turbulence.
    pub time: f32,
    /// Triangle aerodynamics (cloth).
    pub aero: Option<Aero>,
    /// Speed limit per particle, m/s (safety net against explosions).
    pub max_speed: f32,
    /// Frame whose motion the body partly follows rigidly, and how much (0..1). Takes the
    /// edge off extreme accelerations (a dashing character's cape and hair) without
    /// changing how they move at normal speeds.
    pub follow: Option<(u16, f32)>,
    /// Cloth against itself: particles stay this far (m) from the body's own triangles, on
    /// the side they came from. 0 = off.
    pub self_collide: f32,
    /// Under water: the lift on a fully submerged particle as a fraction of its dry weight
    /// (above 1 floats while dry; soaking up water makes it heavier, so it may sink).
    pub buoyancy: f32,
    /// Under water: how fast a particle takes on the current's velocity, 1/s.
    pub water_drag: f32,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            substeps: 10,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            damping: 0.1,
            radius: 0.02,
            friction: 0.5,
            wind: Vec3::ZERO,
            drag: 0.0,
            turbulence: 0.25,
            time: 0.0,
            aero: None,
            max_speed: 60.0,
            follow: None,
            self_collide: 0.0,
            buoyancy: 1.0,
            water_drag: 4.0,
        }
    }
}

/// The water around a body this step, in the body's frame: the surface height sampled on a
/// 3 × 3 grid over the body (bilinear in between) and the current.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WaterSurface {
    /// Local x and z of the grid's first sample, and the spacing between samples.
    pub min: Vec2,
    pub cell: Vec2,
    /// Local y of the surface, rows along z, columns along x.
    pub heights: [f32; 9],
    /// Current, m/s.
    pub current: Vec3,
}

impl WaterSurface {
    /// Calm water at local height `y` everywhere.
    pub fn flat(y: f32, current: Vec3) -> Self {
        Self {
            min: Vec2::ZERO,
            cell: Vec2::ONE,
            heights: [y; 9],
            current,
        }
    }

    /// Local height of the surface over `p`.
    pub fn height(&self, p: Vec3) -> f32 {
        let g = ((Vec2::new(p.x, p.z) - self.min) / self.cell).clamp(Vec2::ZERO, Vec2::splat(2.0));
        let (c, r) = ((g.x as usize).min(1), (g.y as usize).min(1));
        let (fx, fz) = (g.x - c as f32, g.y - r as f32);
        let h = |r: usize, c: usize| self.heights[r * 3 + c];
        let top = h(r, c) + (h(r, c + 1) - h(r, c)) * fx;
        let bottom = h(r + 1, c) + (h(r + 1, c + 1) - h(r + 1, c)) * fx;
        top + (bottom - top) * fz
    }

    /// How far under the surface `p` is (negative above it).
    pub fn depth(&self, p: Vec3) -> f32 {
        self.height(p) - p.y
    }
}

/// Reaction impulse on one collider this step (N·s), and its moment about the collider's
/// position at the end of the step (N·m·s).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ColliderImpulse {
    pub linear: Vec3,
    pub angular: Vec3,
}

/// One soft body's particles and constraints.
#[derive(Debug, Clone, Default)]
pub struct Sim {
    /// World position the particle coordinates are relative to.
    pub origin: DVec3,
    pub pos: Vec<Vec3>,
    pub vel: Vec<Vec3>,
    /// Current inverse masses (1/kg); 0 = kinematic (pinned).
    pub inv_mass: Vec<f32>,
    /// Inverse masses before pins and wetness.
    pub base_inv_mass: Vec<f32>,
    /// Positions at the start of the last step (render interpolation).
    pub step_start: Vec<Vec3>,
    /// Positions at the start of the current substep.
    prev: Vec<Vec3>,
    /// Which particles are pinned (index into `pins`), `u32::MAX` = free.
    pin_of: Vec<u32>,
    pub distances: Vec<Distance>,
    pub tethers: Vec<Tether>,
    pub pins: Vec<Pin>,
    pub goals: Vec<Goal>,
    pub skin_planes: Vec<SkinPlane>,
    pub skin_capsules: Vec<SkinCapsule>,
    pub shapes: Vec<ShapeMatch>,
    pub frames: Vec<Frame>,
    /// Triangles that feel the air (cloth).
    pub aero_triangles: Vec<[u32; 3]>,
    /// Ground plane per particle (`n.xyz`, offset: inside when `n·x + w < 0`), or one for all.
    pub ground: Vec<[f32; 4]>,
    /// Extra forces this step (N), cleared after each step.
    pub loads: Vec<(u32, Vec3)>,
    pub params: SimParams,
    /// Reaction impulses on frames from goals this step (linear, angular about the frame).
    pub frame_impulses: Vec<ColliderImpulse>,
    /// Reaction impulses on colliders this step, indexed like the collider slice.
    pub collider_impulses: Vec<ColliderImpulse>,
    /// Fastest approach speed into the ground this step, m/s, and where.
    pub ground_impact: (f32, Vec3),
    /// Particle-collider contacts in the last substep.
    pub contacts: u32,
    /// The water around the body this step, if it is near any.
    pub water: Option<WaterSurface>,
    accel: Vec<Vec3>,
    /// The gusty wind at each particle this step (empty in steady wind).
    gusts: Vec<Vec3>,
    /// How far each particle is in the water this substep (0..1).
    submerged: Vec<f32>,
    /// Each particle's surface normal (area-weighted), for drag in water.
    normals: Vec<Vec3>,
    posed: Vec<PosedCollider>,
    /// The particles each collider can reach this step: `near[near_starts[c]..near_starts[c + 1]]`.
    near_starts: Vec<u32>,
    near: Vec<u32>,
    /// Each frame's pose at the end of this substep.
    poses: Vec<(Vec3, Quat)>,
    /// The air's force on each particle (N), sampled a few times a step.
    aero_force: Vec<Vec3>,
    split: Split,
    self_contacts: SelfContacts,
}

impl Sim {
    pub fn new(origin: DVec3) -> Self {
        Self {
            origin,
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    /// Add a particle at body-local `pos` with `mass` kg; returns its index.
    pub fn add_particle(&mut self, pos: Vec3, mass: f32) -> u32 {
        let w = if mass > 0.0 { 1.0 / mass } else { 0.0 };
        self.pos.push(pos);
        self.vel.push(Vec3::ZERO);
        self.inv_mass.push(w);
        self.base_inv_mass.push(w);
        self.step_start.push(pos);
        self.prev.push(pos);
        self.pin_of.push(u32::MAX);
        (self.pos.len() - 1) as u32
    }

    /// Connect `a` and `b` at their current distance.
    pub fn add_distance(&mut self, a: u32, b: u32, compliance: f32, tag: u32) {
        let rest = (self.pos[a as usize] - self.pos[b as usize]).length();
        self.distances.push(Distance {
            a,
            b,
            rest,
            compliance,
            tag,
        });
    }

    /// Add a kinematic frame; returns its index.
    pub fn add_frame(&mut self, pos: Vec3, rot: Quat) -> u16 {
        self.frames.push(Frame::at(pos, rot));
        self.frame_impulses.push(ColliderImpulse::default());
        (self.frames.len() - 1) as u16
    }

    /// Where `frame` will be at the end of the next step. Call once per step for every frame
    /// that moves; frames left alone stay put.
    pub fn set_frame(&mut self, frame: u16, pos: Vec3, rot: Quat) {
        let f = &mut self.frames[frame as usize];
        f.to_pos = pos;
        f.to_rot = rot;
    }

    /// Jump a frame without sweeping (teleports, first frame).
    pub fn snap_frame(&mut self, frame: u16, pos: Vec3, rot: Quat) {
        self.frames[frame as usize] = Frame::at(pos, rot);
    }

    /// Pin particle `p` to `frame` where it is now.
    pub fn pin_here(&mut self, p: u32, frame: u16) {
        let f = self.frames[frame as usize];
        let local = f.to_rot.conjugate() * (self.pos[p as usize] - f.to_pos);
        self.pin(p, frame, local);
    }

    /// Pin particle `p` to `frame` at `local`.
    pub fn pin(&mut self, p: u32, frame: u16, local: Vec3) {
        let i = p as usize;
        if self.pin_of[i] != u32::MAX {
            self.pins[self.pin_of[i] as usize] = Pin { p, frame, local };
        } else {
            self.pin_of[i] = self.pins.len() as u32;
            self.pins.push(Pin { p, frame, local });
        }
        self.inv_mass[i] = 0.0;
    }

    /// Release particle `p`'s pin and the goals holding it, if any.
    pub fn unpin(&mut self, p: u32) {
        let i = p as usize;
        self.goals.retain(|g| g.p != p);
        let slot = self.pin_of[i];
        if slot == u32::MAX {
            return;
        }
        self.pins.swap_remove(slot as usize);
        if let Some(moved) = self.pins.get(slot as usize) {
            self.pin_of[moved.p as usize] = slot;
        }
        self.pin_of[i] = u32::MAX;
        self.inv_mass[i] = self.base_inv_mass[i];
    }

    pub fn is_pinned(&self, p: u32) -> bool {
        self.pin_of[p as usize] != u32::MAX
    }

    /// Scale every free particle's mass (wet cloth is heavier).
    pub fn set_mass_scale(&mut self, scale: f32) {
        let s = 1.0 / scale.max(1e-3);
        for i in 0..self.pos.len() {
            if self.pin_of[i] == u32::MAX {
                self.inv_mass[i] = self.base_inv_mass[i] * s;
            }
        }
    }

    /// Multiply each free particle's mass by `scale(i)` (wetness), from its base mass.
    pub fn set_mass_scales(&mut self, scale: impl Fn(usize) -> f32) {
        for i in 0..self.pos.len() {
            if self.pin_of[i] == u32::MAX {
                self.inv_mass[i] = self.base_inv_mass[i] / scale(i).max(1e-3);
            }
        }
    }

    /// Move the coordinate origin to `origin` without moving anything in the world.
    pub fn rebase(&mut self, origin: DVec3) {
        let shift = (self.origin - origin).as_vec3();
        if shift == Vec3::ZERO {
            return;
        }
        self.origin = origin;
        for v in self
            .pos
            .iter_mut()
            .chain(self.step_start.iter_mut())
            .chain(self.prev.iter_mut())
        {
            *v += shift;
        }
        for f in &mut self.frames {
            f.from_pos += shift;
            f.to_pos += shift;
        }
        for g in &mut self.ground {
            // n·(x - shift) + w = n·x + (w - n·shift)
            let n = Vec3::new(g[0], g[1], g[2]);
            g[3] -= n.dot(shift);
        }
    }

    /// Follow the world into a new reference frame (floating origin, a new planet's frame):
    /// every particle keeps its place in the world, and its velocity is seen from the new
    /// frame. A plain floating-origin move only moves [`Sim::origin`].
    pub fn shift(&mut self, s: &WorldShift) {
        self.origin = s.point(self.origin);
        if !s.is_translation_only() {
            let r = s.rotation.as_quat();
            for v in self
                .pos
                .iter_mut()
                .chain(self.step_start.iter_mut())
                .chain(self.prev.iter_mut())
            {
                *v = r * *v;
            }
            for f in &mut self.frames {
                f.from_pos = r * f.from_pos;
                f.to_pos = r * f.to_pos;
                f.from_rot = (r * f.from_rot).normalize();
                f.to_rot = (r * f.to_rot).normalize();
            }
            // n·x + w with both n and x turned keeps w.
            for g in &mut self.ground {
                let n = r * Vec3::new(g[0], g[1], g[2]);
                g[..3].copy_from_slice(&n.to_array());
            }
            for (_, f) in &mut self.loads {
                *f = r * *f;
            }
            self.params.gravity = r * self.params.gravity;
        }
        if !s.is_translation_only()
            || s.velocity != DVec3::ZERO
            || s.angular_velocity != DVec3::ZERO
        {
            for (v, x) in self.vel.iter_mut().zip(&self.pos) {
                *v = s.velocity_at(self.origin + x.as_dvec3(), *v);
            }
        }
    }

    /// A teleport: `frame` jumps to `pos`, `rot`, and every particle (with its history and
    /// velocity) goes along rigidly, so nothing is dragged across the jump and torn. Snap the
    /// other frames to their new poses afterwards.
    pub fn carry(&mut self, frame: u16, pos: Vec3, rot: Quat) {
        let f = self.frames[frame as usize];
        let turn = (rot * f.to_rot.conjugate()).normalize();
        let moved = |x: Vec3| pos + turn * (x - f.to_pos);
        for v in self
            .pos
            .iter_mut()
            .chain(self.step_start.iter_mut())
            .chain(self.prev.iter_mut())
        {
            *v = moved(*v);
        }
        for v in &mut self.vel {
            *v = turn * *v;
        }
        self.snap_frame(frame, pos, rot);
    }

    /// Translate every particle (and its history) by `delta`, keeping velocities.
    pub fn translate(&mut self, delta: Vec3) {
        for v in self
            .pos
            .iter_mut()
            .chain(self.step_start.iter_mut())
            .chain(self.prev.iter_mut())
        {
            *v += delta;
        }
    }

    /// Axis-aligned bounds of the particles (body-local).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.pos {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    }

    /// Position of particle `i` between the last two steps (`alpha` 0..1).
    #[inline]
    pub fn interpolated(&self, i: usize, alpha: f32) -> Vec3 {
        self.step_start[i].lerp(self.pos[i], alpha)
    }

    /// Move particle `i` to `p` (body-local) at rest, without changing any constraint.
    pub fn place(&mut self, i: u32, p: Vec3) {
        let i = i as usize;
        self.pos[i] = p;
        self.step_start[i] = p;
        self.prev[i] = p;
        self.vel[i] = Vec3::ZERO;
    }

    /// Total kinetic energy, J.
    pub fn kinetic_energy(&self) -> f32 {
        self.vel
            .iter()
            .zip(&self.base_inv_mass)
            .map(|(v, w)| {
                if *w > 0.0 {
                    0.5 * v.length_squared() / w
                } else {
                    0.0
                }
            })
            .sum()
    }

    /// Change every free particle's velocity by `dv(position)` (explosions, hits).
    pub fn add_velocity(&mut self, mut dv: impl FnMut(Vec3) -> Vec3) {
        for i in 0..self.pos.len() {
            if self.inv_mass[i] > 0.0 {
                self.vel[i] += dv(self.pos[i]);
            }
        }
    }

    /// Number of substeps for a step of `dt` seconds.
    /// Particle-triangle pairs the self-collision is watching this step.
    pub fn self_pairs(&self) -> usize {
        self.self_contacts.pairs.len()
    }

    /// Of those, the ones close enough to be solved every substep.
    pub fn self_pairs_solved(&self) -> usize {
        self.self_contacts.close.len()
    }

    pub fn substeps_for(&self, dt: f32) -> u32 {
        let n = (self.params.substeps.max(1) as f32 * dt * 60.0).ceil() as u32;
        n.clamp(1, self.params.substeps.max(1) * 4)
    }

    /// Advance by `dt` seconds against `colliders` (body-local, posed for this step).
    pub fn step(&mut self, dt: f32, colliders: &[LocalCollider]) {
        self.step_start.clone_from(&self.pos);
        for f in &mut self.frame_impulses {
            *f = ColliderImpulse::default();
        }
        self.collider_impulses.clear();
        self.collider_impulses
            .resize(colliders.len(), ColliderImpulse::default());
        self.ground_impact = (0.0, Vec3::ZERO);
        if dt <= 0.0 || self.pos.is_empty() {
            self.finish_step();
            return;
        }
        if self.params.self_collide > 0.0 && !self.aero_triangles.is_empty() {
            self.self_contacts.update(
                &self.pos,
                &self.inv_mass,
                &self.aero_triangles,
                self.params.self_collide,
            );
        } else {
            self.self_contacts = SelfContacts::default();
        }
        // The gusts change little within a step, so sample them once per step rather than in
        // every substep.
        let p = self.params;
        self.gusts.clear();
        if p.turbulence > 0.0 && (p.drag > 0.0 || p.aero.is_some()) {
            let side = p.wind.cross(Vec3::Y);
            self.gusts.extend(self.pos.iter().map(|x| {
                let (f, g) = flutter(*x, p.time);
                p.wind * (1.0 + p.turbulence * f) + side * (0.5 * p.turbulence * g)
            }));
        }
        self.find_near(colliders, dt);
        let mut split = std::mem::take(&mut self.split);
        split.update(self);
        self.split = split;
        let n = self.substeps_for(dt);
        let h = dt / n as f32;
        // The air's force changes slowly next to the substeps (Chaos Cloth applies it once a
        // frame at 60 Hz), so it is sampled at least every 1/240 s, not every substep.
        let aero_every = (n / 4).max(1);
        if self.split.starts.len() > 2
            && self.shapes.is_empty()
            && self.aero_triangles.is_empty()
            && self.water.is_none()
        {
            self.step_runs(n, dt, colliders);
        } else {
            for k in 0..n {
                let t0 = k as f32 / n as f32;
                let t1 = (k + 1) as f32 / n as f32;
                self.substep(h, t0, t1, dt, colliders, k % aero_every == 0);
            }
        }
        self.params.time += dt;
        self.finish_step();
    }

    /// Step each run on its own thread through every substep: a body of separate pieces
    /// (hair's strands) whose pieces feel only gravity, the wind and the colliders, so they
    /// share nothing but what they read.
    fn step_runs(&mut self, n: u32, dt: f32, colliders: &[LocalCollider]) {
        let p = self.params;
        let h = dt / n as f32;
        let frames = self.frames.len();
        self.poses.clear();
        for k in 0..=n {
            let t = k as f32 / n as f32;
            self.poses.extend(self.frames.iter().map(|f| f.pose(t)));
        }
        let margin = p.radius + p.max_speed * h;
        self.posed.clear();
        for k in 1..=n {
            let t = k as f32 / n as f32;
            self.posed.extend(
                colliders
                    .iter()
                    .map(|c| PosedCollider::at(c, t, dt, margin)),
            );
        }
        let still = Flow {
            flow: Vec3::ZERO,
            world: Vec3::ZERO,
            time: 0.0,
        };
        let (poses, posed, gusts) = (&self.poses, &self.posed, &self.gusts);
        let (w, loads, split) = (&self.inv_mass, &self.loads, &self.split);
        let shared = |k: usize| Shared {
            p,
            h,
            inv_h: 1.0 / h,
            inv_h2: 1.0 / (h * h),
            w,
            submerged: &[],
            normals: &[],
            flow: &still,
            frames: &self.frames,
            poses: &poses[(k + 1) * frames..(k + 2) * frames],
            distances: &self.distances,
            tethers: &self.tethers,
            goals: &self.goals,
            skin_planes: &self.skin_planes,
            skin_capsules: &self.skin_capsules,
            pins: &self.pins,
        };
        let touch = |k: usize| Touch {
            p,
            h,
            inv_h: 1.0 / h,
            w,
            ground: &self.ground,
            colliders,
            posed: &posed[k * colliders.len()..(k + 1) * colliders.len()],
            near_starts: &self.near_starts,
            near: &self.near,
        };
        let results = for_runs(
            &split.starts,
            true,
            (&mut self.pos, &mut self.vel, &mut self.prev),
            |r, run| {
                let Run {
                    base,
                    pos,
                    vel,
                    prev,
                } = run;
                let mut accel = vec![Vec3::ZERO; pos.len()];
                let mut frame_impulses = vec![ColliderImpulse::default(); frames];
                let mut touched = Touched {
                    contacts: 0,
                    ground_impact: (0.0, Vec3::ZERO),
                    impulses: vec![ColliderImpulse::default(); colliders.len()],
                };
                let end = base + pos.len();
                for k in 0..n as usize {
                    // Follow part of the anchor frame's motion rigidly.
                    if let Some((frame, amount)) = p.follow
                        && amount > 0.0
                        && (frame as usize) < frames
                    {
                        let (p0, r0) = poses[k * frames + frame as usize];
                        let (p1, r1) = poses[(k + 1) * frames + frame as usize];
                        let rot = Quat::IDENTITY.slerp(r1 * r0.conjugate(), amount);
                        let shift = (p1 - p0) * amount;
                        for (j, (x, v)) in pos.iter_mut().zip(vel.iter_mut()).enumerate() {
                            if w[base + j] > 0.0 {
                                *x = p0 + rot * (*x - p0) + shift;
                                *v = rot * *v;
                            }
                        }
                    }
                    // Gravity, the wind's drag and loads.
                    for (j, (a, v)) in accel.iter_mut().zip(vel.iter()).enumerate() {
                        *a = p.gravity;
                        if p.drag > 0.0 {
                            let gust = gusts.get(base + j).copied().unwrap_or(p.wind);
                            *a += (gust - *v) * p.drag;
                        }
                    }
                    for &(i, f) in loads {
                        let i = i as usize;
                        if (base..end).contains(&i) {
                            accel[i - base] += f * w[i];
                        }
                    }
                    let run = Run {
                        base,
                        pos: &mut *pos,
                        vel: &mut *vel,
                        prev: &mut *prev,
                    };
                    let fi = constrain_run(&shared(k), &split.lists[r], &accel, run);
                    for (sum, add) in frame_impulses.iter_mut().zip(fi) {
                        sum.linear += add.linear;
                        sum.angular += add.angular;
                    }
                    let run = Run {
                        base,
                        pos: &mut *pos,
                        vel: &mut *vel,
                        prev: &mut *prev,
                    };
                    let t = collide_run(&touch(k), run);
                    touched.contacts = t.contacts;
                    if t.ground_impact.0 > touched.ground_impact.0 {
                        touched.ground_impact = t.ground_impact;
                    }
                    for (sum, add) in touched.impulses.iter_mut().zip(t.impulses) {
                        sum.linear += add.linear;
                        sum.angular += add.angular;
                    }
                }
                (frame_impulses, touched)
            },
        );
        self.contacts = 0;
        for (fi, t) in results {
            for (sum, add) in self.frame_impulses.iter_mut().zip(fi) {
                sum.linear += add.linear;
                sum.angular += add.angular;
            }
            self.contacts += t.contacts;
            if t.ground_impact.0 > self.ground_impact.0 {
                self.ground_impact = t.ground_impact;
            }
            for (sum, add) in self.collider_impulses.iter_mut().zip(t.impulses) {
                sum.linear += add.linear;
                sum.angular += add.angular;
            }
        }
    }

    /// Which particles each collider can reach this step: those within its bounding box
    /// swept along its path, plus as far as the particle may travel in the step (twice its
    /// speed and half the body's fastest, for a strand whipped up to speed within the step,
    /// plus how far the anchor frames carry it), so the substeps test each particle against
    /// a few colliders, not all of them.
    fn find_near(&mut self, colliders: &[LocalCollider], dt: f32) {
        self.near_starts.clear();
        self.near.clear();
        if colliders.is_empty() {
            return;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut fastest = 0.0f32;
        for (x, v) in self.pos.iter().zip(&self.vel) {
            lo = lo.min(*x);
            hi = hi.max(*x);
            fastest = fastest.max(v.length_squared());
        }
        let (center, extent) = ((lo + hi) * 0.5, (hi - lo).length() * 0.5);
        let carried = self.frames.iter().fold(0.0f32, |m, f| {
            let (_, mut angle) = (f.to_rot * f.from_rot.conjugate()).to_axis_angle();
            if angle > PI {
                angle -= TAU;
            }
            let arm = (center - f.from_pos).length() + extent;
            m.max((f.to_pos - f.from_pos).length() + angle.abs() * arm)
        });
        let travel = 0.5 * fastest.sqrt() * dt + carried + self.params.radius + NEAR_SLACK;
        for c in colliders {
            self.near_starts.push(self.near.len() as u32);
            // In the collider's box, from the nearest point of its path; turning in the step
            // sweeps its far end by the angle times its size.
            let (_, mut angle) = (c.to_rot * c.from_rot.conjugate()).to_axis_angle();
            if angle > PI {
                angle -= TAU;
            }
            let reach = travel + angle.abs() * c.shape.bounding_radius();
            let half = c.shape.half_extents() + reach;
            let vel = &self.vel;
            let inv_rot = c.to_rot.conjugate();
            let (a, ab) = (c.from_pos, c.to_pos - c.from_pos);
            let l2 = ab.length_squared().max(1e-12);
            for (i, x) in self.pos.iter().enumerate() {
                if self.inv_mass[i] == 0.0 {
                    continue;
                }
                let t = ((*x - a).dot(ab) / l2).clamp(0.0, 1.0);
                let own = Vec3::splat(2.0 * dt) * vel[i].length();
                if (inv_rot * (*x - a - ab * t)).abs().cmple(half + own).all() {
                    self.near.push(i as u32);
                }
            }
        }
        self.near_starts.push(self.near.len() as u32);
    }

    fn finish_step(&mut self) {
        for f in &mut self.frames {
            f.from_pos = f.to_pos;
            f.from_rot = f.to_rot;
        }
        self.loads.clear();
    }

    fn substep(
        &mut self,
        h: f32,
        t0: f32,
        t1: f32,
        dt: f32,
        colliders: &[LocalCollider],
        sample_air: bool,
    ) {
        let inv_h = 1.0 / h;
        let inv_h2 = inv_h * inv_h;
        let p = self.params;

        // Follow part of the anchor frame's motion rigidly.
        if let Some((frame, amount)) = p.follow
            && amount > 0.0
            && let Some(f) = self.frames.get(frame as usize)
        {
            let (p0, r0) = f.pose(t0);
            let (p1, r1) = f.pose(t1);
            let rot = Quat::IDENTITY.slerp(r1 * r0.conjugate(), amount);
            let shift = (p1 - p0) * amount;
            for i in 0..self.pos.len() {
                if self.inv_mass[i] > 0.0 {
                    let rel = self.pos[i] - p0;
                    self.pos[i] = p0 + rot * rel + shift;
                    self.vel[i] = rot * self.vel[i];
                }
            }
        }

        self.poses.clear();
        self.poses.extend(self.frames.iter().map(|f| f.pose(t1)));

        // External accelerations: gravity, water, air, loads.
        self.accel.clear();
        self.accel.resize(self.pos.len(), p.gravity);
        // Water: lift against gravity on what's under the surface (the same force on a soaked,
        // heavier particle lifts it less), eased in across the particle's thickness.
        self.submerged.clear();
        if let Some(water) = &self.water {
            self.submerged.resize(self.pos.len(), 0.0);
            let r = p.radius.max(0.005);
            for i in 0..self.pos.len() {
                if self.inv_mass[i] == 0.0 {
                    continue;
                }
                let s = ((water.depth(self.pos[i]) + r) / (2.0 * r)).clamp(0.0, 1.0);
                if s > 0.0 {
                    self.submerged[i] = s;
                    let dry = self.base_inv_mass[i].max(1e-9);
                    self.accel[i] -= p.gravity * (p.buoyancy * s * self.inv_mass[i] / dry);
                }
            }
        }
        // The wind only reaches what is out of the water.
        let submerged = &self.submerged;
        let in_air = |i: usize| 1.0 - submerged.get(i).copied().unwrap_or(0.0);
        let time = p.time + t0 * dt;
        let gusts = &self.gusts;
        let gust = |i: usize| gusts.get(i).copied().unwrap_or(p.wind);
        if p.drag > 0.0 {
            for (i, (a, v)) in self.accel.iter_mut().zip(&self.vel).enumerate() {
                *a += (gust(i) - *v) * (p.drag * in_air(i));
            }
        }
        if let Some(aero) = p.aero
            && (sample_air || self.aero_force.len() != self.pos.len())
        {
            self.aero_force.clear();
            self.aero_force.resize(self.pos.len(), Vec3::ZERO);
            for tri in &self.aero_triangles {
                let [a, b, c] = tri.map(|i| i as usize);
                let exposed = (in_air(a) + in_air(b) + in_air(c)) * (1.0 / 3.0);
                let n2 = (self.pos[b] - self.pos[a]).cross(self.pos[c] - self.pos[a]);
                let len = n2.length();
                if len < 1e-12 || exposed <= 0.0 {
                    continue;
                }
                let area = 0.5 * len * exposed;
                let n = n2 / len;
                let wind = (gust(a) + gust(b) + gust(c)) * (1.0 / 3.0);
                let v_rel = wind - (self.vel[a] + self.vel[b] + self.vel[c]) * (1.0 / 3.0);
                let vn = v_rel.dot(n);
                let vt = v_rel - n * vn;
                let mut force =
                    n * (aero.drag * vn * vn.abs()) + vt * (aero.friction * vt.length());
                if aero.lift > 0.0 {
                    // Across the wind, toward the side the wind presses: ½ρA·C·v²·sin 2α.
                    let speed = v_rel.length();
                    let pressed = n * vn.signum();
                    let across = pressed - v_rel * (pressed.dot(v_rel) / (speed * speed).max(1e-9));
                    if let Some(dir) = across.try_normalize() {
                        force += dir * (aero.lift * 2.0 * vn.abs() * vt.length());
                    }
                }
                let force = force * (0.5 * AIR_DENSITY * area / 3.0);
                for i in [a, b, c] {
                    self.aero_force[i] += force;
                }
            }
        }
        if p.aero.is_some() {
            for ((a, f), w) in self
                .accel
                .iter_mut()
                .zip(&self.aero_force)
                .zip(&self.inv_mass)
            {
                *a += *f * *w;
            }
        } else {
            self.aero_force.clear();
        }
        for &(i, f) in &self.loads {
            let i = i as usize;
            self.accel[i] += f * self.inv_mass[i];
        }

        // Water drag on what's in it: cloth resists moving across its face far more than
        // sliding along it (so a strip in a stream lies along the flow and ripples, and a
        // sinking sheet glides); strands and ropes drag the same every way.
        self.normals.clear();
        if !self.submerged.is_empty() && !self.aero_triangles.is_empty() {
            self.normals.resize(self.pos.len(), Vec3::ZERO);
            for tri in &self.aero_triangles {
                let [a, b, c] = tri.map(|i| i as usize);
                if self.submerged[a] + self.submerged[b] + self.submerged[c] == 0.0 {
                    continue;
                }
                let n2 = (self.pos[b] - self.pos[a]).cross(self.pos[c] - self.pos[a]);
                for i in [a, b, c] {
                    self.normals[i] += n2;
                }
            }
        }
        let flow = Flow {
            flow: self.water.map_or(Vec3::ZERO, |w| w.current),
            // The wrap's seams sit 2 km out, not at the origin (the lake's surface is at y = 0).
            world: (self.origin + DVec3::splat(2048.0))
                .rem_euclid(DVec3::splat(4096.0))
                .as_vec3(),
            time,
        };

        // Each run of particles predicts and solves the constraints inside it, side by side
        // with the others; then the constraints across runs; then each run collides.
        let shared = Shared {
            p,
            h,
            inv_h,
            inv_h2,
            w: &self.inv_mass,
            submerged: &self.submerged,
            normals: &self.normals,
            flow: &flow,
            frames: &self.frames,
            poses: &self.poses,
            distances: &self.distances,
            tethers: &self.tethers,
            goals: &self.goals,
            skin_planes: &self.skin_planes,
            skin_capsules: &self.skin_capsules,
            pins: &self.pins,
        };
        let split = &self.split;
        let accel = &self.accel;
        let frame_impulses = for_runs(
            &split.starts,
            false,
            (&mut self.pos, &mut self.vel, &mut self.prev),
            |k, run| {
                let span = run.base..run.base + run.pos.len();
                constrain_run(&shared, &split.lists[k], &accel[span], run)
            },
        );
        for fi in frame_impulses {
            for (sum, add) in self.frame_impulses.iter_mut().zip(fi) {
                sum.linear += add.linear;
                sum.angular += add.angular;
            }
        }
        let pos = &mut self.pos;
        let w = &self.inv_mass;
        for s in &mut self.shapes {
            solve_shape(s, pos, w, h, inv_h2);
        }
        if !self.self_contacts.close.is_empty() {
            collide_self(
                pos,
                &self.prev,
                w,
                &self.aero_triangles,
                p.self_collide,
                p.friction,
                &self.self_contacts.close,
            );
        }

        // Collisions, then velocities.
        self.posed.clear();
        let margin = p.radius + p.max_speed * h;
        self.posed.extend(
            colliders
                .iter()
                .map(|c| PosedCollider::at(c, t1, dt, margin)),
        );
        let touch = Touch {
            p,
            h,
            inv_h,
            w: &self.inv_mass,
            ground: &self.ground,
            colliders,
            posed: &self.posed,
            near_starts: &self.near_starts,
            near: &self.near,
        };
        let touched = for_runs(
            &split.starts,
            false,
            (&mut self.pos, &mut self.vel, &mut self.prev),
            |_, run| collide_run(&touch, run),
        );
        self.contacts = 0;
        for t in touched {
            self.contacts += t.contacts;
            if t.ground_impact.0 > self.ground_impact.0 {
                self.ground_impact = t.ground_impact;
            }
            for (sum, add) in self.collider_impulses.iter_mut().zip(t.impulses) {
                sum.linear += add.linear;
                sum.angular += add.angular;
            }
        }
    }
}

/// Moving water's velocity at a point, with eddies that, like gusts in the wind, turn it
/// every way and are carried along by the current, so what streams in it undulates (none in
/// still water). In world space (wrapped), so bodies side by side feel the same eddies.
struct Flow {
    flow: Vec3,
    world: Vec3,
    time: f32,
}

impl Flow {
    fn at(&self, x: Vec3) -> Vec3 {
        let (flow, time) = (self.flow, self.time);
        let across = flow.cross(Vec3::Y);
        if across == Vec3::ZERO {
            return flow;
        }
        let drift = x + self.world - flow * time;
        let (f, g) = flutter(drift * 0.6, time * 0.35);
        let (e, _) = flutter(Vec3::new(drift.z, drift.x, drift.y) * 0.8, time * 0.5 + 3.0);
        let speed = flow.length();
        flow * (1.0 + 0.5 * WATER_EDDIES * f)
            + across * (0.8 * WATER_EDDIES * g)
            + Vec3::Y * (WATER_EDDIES * speed * e)
    }
}

/// Threads for the work one body splits off (hair's runs, a big self-collision search), a
/// pool of their own. A thread waiting in a scope runs other tasks of that pool meanwhile:
/// on the compute pool that could be a whole other body, holding this body's step up until
/// that one's step ends too (steps of 200 ms on 32 threads); here it is only another piece.
/// Pieces never wait on anything, so the pool can't deadlock.
fn leaf_pool() -> &'static TaskPool {
    static POOL: std::sync::OnceLock<TaskPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
        TaskPoolBuilder::new()
            .num_threads((threads / 2).max(2))
            .thread_name("softbody".into())
            .build()
    })
}

/// Bodies with this many particles or more split into runs solved side by side.
const SPLIT_MIN: usize = 1024;
/// Fewest particles in a run: smaller ones cost more to hand out than they save.
const RUN_MIN: usize = 512;

/// How a big body of separate pieces (hair's strands, each a chain of consecutive ids)
/// splits into runs of whole pieces, each stepped on its own thread (Jolt groups a soft
/// body's vertices the same way; a cut never splits a piece, so no constraint joins two
/// runs, and a body all of one piece, like a cloth, stays one run). Each run's constraints.
#[derive(Debug, Clone, Default)]
struct Split {
    key: u64,
    /// Run `k` holds particles `starts[k]..starts[k + 1]`.
    starts: Vec<usize>,
    lists: Vec<RunLists>,
}

/// The constraints whose particles all lie in one run, by index.
#[derive(Debug, Clone, Default)]
struct RunLists {
    distances: Vec<u32>,
    tethers: Vec<u32>,
    goals: Vec<u32>,
    skin_planes: Vec<u32>,
    skin_capsules: Vec<u32>,
    pins: Vec<u32>,
}

impl Split {
    fn update(&mut self, sim: &Sim) {
        let n = sim.pos.len();
        let threads = leaf_pool().thread_num();
        let runs = if n >= SPLIT_MIN && threads > 1 {
            (n / RUN_MIN).clamp(1, threads)
        } else {
            1
        };
        let mix = |h: u64, v: u64| (h ^ v).wrapping_mul(0x100_0000_01b3);
        let pairs = sim.distances.iter().map(|c| (c.a, c.b));
        let pairs = pairs.chain(sim.tethers.iter().map(|c| (c.p, c.anchor)));
        let mut key = pairs.clone().fold(mix(n as u64, runs as u64), |h, (a, b)| {
            mix(h, a as u64 | (b as u64) << 32)
        });
        let singles = sim.goals.iter().map(|g| g.p);
        let singles = singles.chain(sim.skin_planes.iter().map(|c| c.p));
        let singles = singles.chain(sim.skin_capsules.iter().map(|c| c.p));
        let singles = singles.chain(sim.pins.iter().map(|c| c.p));
        key = singles.clone().fold(key, |h, p| mix(h, p as u64));
        if key == self.key && !self.lists.is_empty() {
            return;
        }
        self.key = key;
        // Where a cut can go: before particle `i` when nothing before it is joined to
        // anything from `i` on.
        let mut reach: Vec<u32> = (0..n as u32).collect();
        for (a, b) in pairs {
            let (lo, hi) = (a.min(b) as usize, a.max(b));
            if lo < n {
                reach[lo] = reach[lo].max(hi);
            }
        }
        self.starts.clear();
        self.starts.push(0);
        let size = n.div_ceil(runs);
        let mut far = 0;
        for (i, &r) in reach.iter().enumerate() {
            if i > 0 && far < i && i - self.starts[self.starts.len() - 1] >= size {
                self.starts.push(i);
            }
            far = far.max(r as usize);
        }
        self.starts.push(n);
        let runs = self.starts.len() - 1;
        self.lists.clear();
        self.lists.resize(runs, RunLists::default());
        let starts = &self.starts;
        let of = |i: u32| starts.partition_point(|&s| s <= i as usize) - 1;
        for (k, c) in sim.distances.iter().enumerate() {
            self.lists[of(c.a)].distances.push(k as u32);
        }
        for (k, c) in sim.tethers.iter().enumerate() {
            self.lists[of(c.p)].tethers.push(k as u32);
        }
        for (k, g) in sim.goals.iter().enumerate() {
            self.lists[of(g.p)].goals.push(k as u32);
        }
        for (k, c) in sim.skin_planes.iter().enumerate() {
            self.lists[of(c.p)].skin_planes.push(k as u32);
        }
        for (k, c) in sim.skin_capsules.iter().enumerate() {
            self.lists[of(c.p)].skin_capsules.push(k as u32);
        }
        for (k, c) in sim.pins.iter().enumerate() {
            self.lists[of(c.p)].pins.push(k as u32);
        }
    }
}

/// A run's share of the body: particles `base..base + pos.len()`.
struct Run<'a> {
    base: usize,
    pos: &'a mut [Vec3],
    vel: &'a mut [Vec3],
    prev: &'a mut [Vec3],
}

/// Run `f` on each run (`starts` as in [`Split`]), side by side on the compute pool when
/// `parallel` and there are several, and return what each gives in order.
fn for_runs<R: Send + 'static>(
    starts: &[usize],
    parallel: bool,
    (mut pos, mut vel, mut prev): (&mut [Vec3], &mut [Vec3], &mut [Vec3]),
    f: impl Fn(usize, Run) -> R + Sync,
) -> Vec<R> {
    let mut runs = Vec::with_capacity(starts.len().saturating_sub(1));
    for (k, span) in starts.windows(2).enumerate() {
        let len = span[1] - span[0];
        let (p, rest_p) = std::mem::take(&mut pos).split_at_mut(len);
        let (v, rest_v) = std::mem::take(&mut vel).split_at_mut(len);
        let (q, rest_q) = std::mem::take(&mut prev).split_at_mut(len);
        (pos, vel, prev) = (rest_p, rest_v, rest_q);
        runs.push((
            k,
            Run {
                base: span[0],
                pos: p,
                vel: v,
                prev: q,
            },
        ));
    }
    if parallel && runs.len() > 1 {
        let f = &f;
        leaf_pool().scope(|scope| {
            for (k, r) in runs {
                scope.spawn(async move { f(k, r) });
            }
        })
    } else {
        runs.into_iter().map(|(k, r)| f(k, r)).collect()
    }
}

/// What every run reads while it predicts and solves its constraints.
struct Shared<'a> {
    p: SimParams,
    h: f32,
    inv_h: f32,
    inv_h2: f32,
    w: &'a [f32],
    submerged: &'a [f32],
    normals: &'a [Vec3],
    flow: &'a Flow,
    frames: &'a [Frame],
    poses: &'a [(Vec3, Quat)],
    distances: &'a [Distance],
    tethers: &'a [Tether],
    goals: &'a [Goal],
    skin_planes: &'a [SkinPlane],
    skin_capsules: &'a [SkinCapsule],
    pins: &'a [Pin],
}

/// Predict a run's particles and solve the constraints inside it; gives the goals'
/// reaction impulses on the frames.
/// `accel` holds the run's particles' accelerations.
fn constrain_run(s: &Shared, lists: &RunLists, accel: &[Vec3], run: Run) -> Vec<ColliderImpulse> {
    let Run {
        base,
        pos,
        vel,
        prev,
    } = run;
    let (p, h, w) = (s.p, s.h, s.w);
    // Predict. Water drag pulls the velocity toward the current, implicitly (stable
    // however strong).
    for (k, ((x, v), before)) in pos.iter_mut().zip(vel.iter_mut()).zip(prev).enumerate() {
        let i = base + k;
        *before = *x;
        if w[i] > 0.0 {
            *v += accel[k] * h;
            if let Some(&sub) = s.submerged.get(i)
                && sub > 0.0
            {
                let k = p.water_drag * sub * h;
                let c = s.flow.at(*x);
                let rel = *v - c;
                *v = match s.normals.get(i).and_then(|n| n.try_normalize()) {
                    Some(n) => {
                        let along = rel.dot(n);
                        let slide = rel - n * along;
                        c + n * (along / (1.0 + k)) + slide / (1.0 + k * SLIDE_DRAG)
                    }
                    None => c + rel / (1.0 + k),
                };
            }
            *x += *v * h;
        }
    }
    let point = |frame: u16, local: Vec3| {
        let (fp, fr) = s.poses[frame as usize];
        fp + fr * local
    };
    for &k in &lists.pins {
        let pin = &s.pins[k as usize];
        pos[pin.p as usize - base] = point(pin.frame, pin.local);
    }

    // Constraints.
    solve_distances(pos, base, w, s.inv_h2, s.distances, &lists.distances);
    solve_tethers(pos, base, w, s.tethers, &lists.tethers);
    let mut impulses = vec![ColliderImpulse::default(); s.frames.len()];
    for &k in &lists.goals {
        let g = &s.goals[k as usize];
        let i = g.p as usize;
        if w[i] == 0.0 {
            continue;
        }
        let x = &mut pos[i - base];
        let target = point(g.frame, g.local);
        let dx = (target - *x) * (w[i] / (w[i] + g.compliance * s.inv_h2));
        *x += dx;
        // Reaction on the frame: equal and opposite impulse.
        let j = dx * (s.inv_h / w[i]);
        let fi = &mut impulses[g.frame as usize];
        fi.linear -= j;
        fi.angular -= (*x - s.frames[g.frame as usize].to_pos).cross(j);
    }
    for &k in &lists.skin_planes {
        let c = &s.skin_planes[k as usize];
        let i = c.p as usize;
        if w[i] == 0.0 {
            continue;
        }
        let x = &mut pos[i - base];
        let (fp, fr) = s.poses[c.frame as usize];
        let n = fr * c.normal;
        let d = (*x - fp - fr * c.point).dot(n) - c.margin;
        if d < 0.0 {
            *x -= n * d;
        }
    }
    for &k in &lists.skin_capsules {
        let c = &s.skin_capsules[k as usize];
        let i = c.p as usize;
        if w[i] == 0.0 {
            continue;
        }
        let x = &mut pos[i - base];
        let (fp, fr) = s.poses[c.frame as usize];
        let (a, ab) = (fp + fr * c.a, fr * (c.b - c.a));
        let t = ((*x - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
        let off = *x - a - ab * t;
        let dist = off.length();
        let d = dist - c.radius;
        if d < 0.0 && d > -c.reach && dist > 1e-6 {
            *x -= off * (d / dist);
        }
    }
    impulses
}

/// Distance constraints `list` (indices into `distances`) on particles `base..`.
fn solve_distances(
    pos: &mut [Vec3],
    base: usize,
    w: &[f32],
    inv_h2: f32,
    distances: &[Distance],
    list: &[u32],
) {
    for &k in list {
        let c = &distances[k as usize];
        let (a, b) = (c.a as usize, c.b as usize);
        let wsum = w[a] + w[b];
        if wsum == 0.0 {
            continue;
        }
        let d = pos[a - base] - pos[b - base];
        let len = d.length();
        if len < 1e-9 {
            continue;
        }
        let dl = -(len - c.rest) / (wsum + c.compliance * inv_h2);
        let corr = d * (dl / len);
        pos[a - base] += corr * w[a];
        pos[b - base] -= corr * w[b];
    }
}

/// Tethers `list` (indices into `tethers`) on particles `base..`.
fn solve_tethers(pos: &mut [Vec3], base: usize, w: &[f32], tethers: &[Tether], list: &[u32]) {
    for &k in list {
        let c = &tethers[k as usize];
        let (i, a) = (c.p as usize, c.anchor as usize);
        if w[i] == 0.0 {
            continue;
        }
        let d = pos[i - base] - pos[a - base];
        let len = d.length();
        if len > c.max {
            // A pinned anchor doesn't move; one held by a goal (cloth on a moving body)
            // takes its share, so the goal passes the pull on to the body.
            let corr = d * ((len - c.max) / (len * (w[i] + w[a])));
            pos[i - base] -= corr * w[i];
            pos[a - base] += corr * w[a];
        }
    }
}

/// What every run reads while it collides.
struct Touch<'a> {
    p: SimParams,
    h: f32,
    inv_h: f32,
    w: &'a [f32],
    ground: &'a [[f32; 4]],
    colliders: &'a [LocalCollider],
    posed: &'a [PosedCollider],
    near_starts: &'a [u32],
    near: &'a [u32],
}

/// What a run's collisions gave.
struct Touched {
    contacts: u32,
    ground_impact: (f32, Vec3),
    impulses: Vec<ColliderImpulse>,
}

/// Collide a run's particles with the ground and the colliders, then update their
/// velocities.
fn collide_run(t: &Touch, run: Run) -> Touched {
    let Run {
        base,
        pos,
        vel,
        prev,
    } = run;
    let (p, h, w) = (t.p, t.h, t.w);
    let end = base + pos.len();
    let mut out = Touched {
        contacts: 0,
        ground_impact: (0.0, Vec3::ZERO),
        impulses: vec![ColliderImpulse::default(); t.colliders.len()],
    };
    let shared_ground = (t.ground.len() == 1).then(|| t.ground[0]);
    for k in 0..pos.len() {
        let i = base + k;
        if w[i] == 0.0 {
            continue;
        }
        let g = shared_ground.or_else(|| t.ground.get(i).copied());
        if let Some(g) = g {
            let x = pos[k];
            let n = Vec3::new(g[0], g[1], g[2]);
            let d = n.dot(x) + g[3] - p.radius;
            if d < 0.0 {
                let approach = -n.dot(vel[k]);
                if approach > out.ground_impact.0 {
                    out.ground_impact = (approach, x);
                }
                pos[k] = friction(x - n * d, prev[k], n, -d, Vec3::ZERO, h, p.friction);
            }
        }
    }
    for (ci, c) in t.posed.iter().enumerate() {
        let near = &t.near[t.near_starts[ci] as usize..t.near_starts[ci + 1] as usize];
        // Each collider's particles are in id order.
        let from = near.partition_point(|&i| (i as usize) < base);
        let to = near.partition_point(|&i| (i as usize) < end);
        for &i in &near[from..to] {
            let i = i as usize;
            if w[i] == 0.0 {
                continue;
            }
            let k = i - base;
            let mut x = pos[k];
            let Some((d, n)) = c.query(&t.colliders[ci].shape, x) else {
                continue;
            };
            if d >= p.radius {
                continue;
            }
            out.contacts += 1;
            let push = p.radius - d;
            let before = x;
            x += n * push;
            let mu = (p.friction * c.friction).sqrt();
            x = friction(x, prev[k], n, push, c.velocity_at(x), h, mu);
            if t.colliders[ci].dynamic {
                let j = (x - before) * (t.inv_h / w[i]);
                let imp = &mut out.impulses[ci];
                imp.linear -= j;
                imp.angular -= (x - t.colliders[ci].to_pos).cross(j);
            }
            pos[k] = x;
        }
    }

    // Velocities.
    let damp = (1.0 - p.damping * h).max(0.0);
    let max2 = p.max_speed * p.max_speed;
    for k in 0..pos.len() {
        let mut v = (pos[k] - prev[k]) * t.inv_h;
        if w[base + k] > 0.0 {
            v *= damp;
            let s2 = v.length_squared();
            if s2 > max2 {
                v *= p.max_speed / s2.sqrt();
            }
        }
        vel[k] = v;
    }
    out
}

/// Candidate pairs for cloth self-collision (a Verlet list): each particle with the
/// triangles it is within `gap` plus a skin of, found again only once the cloth has bent
/// by half the skin. Swinging and turning as a whole (a curtain swaying, a cape on a
/// running character) doesn't count, since it changes no distances.
#[derive(Debug, Clone, Default)]
struct SelfContacts {
    pairs: Vec<(u32, u32)>,
    /// The pairs that can touch during this step (within the gap plus how far they can
    /// close at the speed the cloth last moved), solved every substep.
    close: Vec<(u32, u32)>,
    /// Positions at the start of the last step, and how far each particle moved in it
    /// (besides the whole cloth's drift), m.
    last: Vec<Vec3>,
    motion: Vec<f32>,
    /// Particles that bent too far from where the pairs were last found, this step.
    moved: Vec<bool>,
    /// Positions at the last search, about their center.
    at_search: Vec<Vec3>,
    /// How the cloth has turned since the last search.
    turned: Quat,
    skin: f32,
    /// Free particles bucketed by hashed cell: bucket starts, then the particles (position
    /// and id), in bucket order so a bucket's are read together.
    starts: Vec<u32>,
    bucketed: Vec<(Vec3, u32)>,
    cells: Vec<u32>,
    /// The same for just the moved particles, when only they are searched again.
    moved_starts: Vec<u32>,
    moved_bucketed: Vec<(Vec3, u32)>,
    /// The hot patches' triangles, this search.
    hot: Vec<u32>,
    patches: Patches,
}

/// Extra reach when finding the particles near each collider, m.
const NEAR_SLACK: f32 = 0.05;

/// Normals of a patch further than this from its mean (radians) can fold it onto itself.
const PATCH_CONE: f32 = 1.25;
/// Triangles per patch.
const PATCH_SIZE: usize = 48;
/// Fewest triangles a thread searches when the self-collision search splits.
const SEARCH_CHUNK: usize = 256;

/// The cloth's triangles grown into small connected patches, to skip most of the
/// self-collision search. Each search joins neighbouring patches into regions whose normals
/// all lie within a cone narrower than [`PATCH_CONE`]: such a region is a gentle height field
/// over the plane across its axis, so it can't fold onto itself, and only where two regions'
/// bounds meet can cloth touch cloth. A hanging curtain is one region and costs nothing; only
/// triangles in "hot" patches (folded sharply, or near another region) are searched.
#[derive(Debug, Clone, Default)]
struct Patches {
    /// Fingerprint of the triangles the patches were grown for (cloth tears change them).
    key: u64,
    /// Patch `k` holds `tris[starts[k]..starts[k + 1]]`.
    starts: Vec<u32>,
    tris: Vec<u32>,
    /// Patches sharing a vertex with each patch.
    neighbours: Vec<Vec<u32>>,
    // Per search.
    bounds: Vec<(Vec3, Vec3)>,
    cones: Vec<(Vec3, f32)>,
    hot: Vec<bool>,
    region_of: Vec<u32>,
    /// Each region's bounds and patches (`members[first..]`, contiguous).
    regions: Vec<(Vec3, Vec3, Range<u32>)>,
    members: Vec<u32>,
    queue: Vec<u32>,
    /// The particles sharing a triangle with each particle (`ring[ring_starts[i]..]`).
    ring_starts: Vec<u32>,
    ring: Vec<u32>,
}

/// The narrowest cone (axis, half-angle) holding both cones.
fn merge_cones((na, ha): (Vec3, f32), (nb, hb): (Vec3, f32)) -> (Vec3, f32) {
    let theta = na.dot(nb).clamp(-1.0, 1.0).acos();
    if theta + hb <= ha {
        return (na, ha);
    }
    if theta + ha <= hb {
        return (nb, hb);
    }
    let half = (theta + ha + hb) * 0.5;
    if theta < 1e-4 || theta > PI - 1e-4 {
        return (na, half);
    }
    // Turn the first axis toward the second by the half-angle it grows.
    let phi = half - ha;
    let axis = (na * (theta - phi).sin() + nb * phi.sin()) / theta.sin();
    (axis.normalize_or(na), half)
}

fn overlap((la, ha): (Vec3, Vec3), (lb, hb): (Vec3, Vec3)) -> bool {
    la.cmple(hb).all() && lb.cmple(ha).all()
}

impl Patches {
    fn count(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    fn fingerprint(triangles: &[[u32; 3]]) -> u64 {
        triangles.iter().fold(triangles.len() as u64, |h, t| {
            let v = (t[0] as u64) | (t[1] as u64) << 21 | (t[2] as u64) << 42;
            (h ^ v).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Grow patches breadth-first across shared edges.
    fn grow(&mut self, triangles: &[[u32; 3]], particles: usize) {
        let mut by_edge: HashMap<(u32, u32), [u32; 2]> = HashMap::new();
        for (t, tri) in triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let e = by_edge.entry((a.min(b), a.max(b))).or_insert([u32::MAX; 2]);
                e[usize::from(e[0] != u32::MAX)] = t as u32;
            }
        }
        let mut patch_of = vec![u32::MAX; triangles.len()];
        self.starts.clear();
        self.tris.clear();
        let mut queue = std::collections::VecDeque::new();
        for seed in 0..triangles.len() {
            if patch_of[seed] != u32::MAX {
                continue;
            }
            let patch = self.starts.len() as u32;
            self.starts.push(self.tris.len() as u32);
            let first = self.tris.len();
            queue.clear();
            queue.push_back(seed as u32);
            patch_of[seed] = patch;
            while let Some(t) = queue.pop_front() {
                self.tris.push(t);
                if self.tris.len() - first + queue.len() >= PATCH_SIZE {
                    continue;
                }
                let tri = triangles[t as usize];
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    for n in by_edge[&(a.min(b), a.max(b))] {
                        if n != u32::MAX && patch_of[n as usize] == u32::MAX {
                            patch_of[n as usize] = patch;
                            queue.push_back(n);
                        }
                    }
                }
            }
        }
        self.starts.push(self.tris.len() as u32);
        let mut ring: Vec<Vec<u32>> = vec![Vec::new(); particles];
        for tri in triangles {
            for &v in tri {
                for &u in tri {
                    if u != v && !ring[v as usize].contains(&u) {
                        ring[v as usize].push(u);
                    }
                }
            }
        }
        self.ring_starts.clear();
        self.ring.clear();
        for list in &ring {
            self.ring_starts.push(self.ring.len() as u32);
            self.ring.extend_from_slice(list);
        }
        self.ring_starts.push(self.ring.len() as u32);
        let count = self.count();
        let mut patches_of: Vec<Vec<u32>> = vec![Vec::new(); particles];
        for (t, tri) in triangles.iter().enumerate() {
            for &v in tri {
                let list = &mut patches_of[v as usize];
                if !list.contains(&patch_of[t]) {
                    list.push(patch_of[t]);
                }
            }
        }
        self.neighbours.clear();
        self.neighbours.resize(count, Vec::new());
        for list in &patches_of {
            for &a in list {
                for &b in list {
                    let n = &mut self.neighbours[a as usize];
                    if a != b && !n.contains(&b) {
                        n.push(b);
                    }
                }
            }
        }
        self.key = Self::fingerprint(triangles);
    }

    /// Mark the patches that may touch themselves or each other within `reach`.
    fn heat(&mut self, pos: &[Vec3], triangles: &[[u32; 3]], reach: f32) {
        let key = Self::fingerprint(triangles);
        if key != self.key || self.starts.is_empty() {
            self.grow(triangles, pos.len());
        }
        let count = self.count();
        self.bounds.clear();
        self.cones.clear();
        self.hot.clear();
        self.hot.resize(count, false);
        for k in 0..count {
            let tris = &self.tris[self.starts[k] as usize..self.starts[k + 1] as usize];
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let mut sum = Vec3::ZERO;
            for &t in tris {
                let [a, b, c] = triangles[t as usize].map(|i| pos[i as usize]);
                lo = lo.min(a).min(b).min(c);
                hi = hi.max(a).max(b).max(c);
                sum += (b - a).cross(c - a);
            }
            let axis = sum.normalize_or_zero();
            let mut cos = 1.0f32;
            for &t in tris {
                let [a, b, c] = triangles[t as usize].map(|i| pos[i as usize]);
                if let Some(n) = (b - a).cross(c - a).try_normalize() {
                    cos = cos.min(n.dot(axis));
                }
            }
            let half = cos.clamp(-1.0, 1.0).acos();
            self.hot[k] = axis == Vec3::ZERO || half > PATCH_CONE;
            self.bounds.push((lo - reach, hi + reach));
            self.cones.push((axis, half));
        }
        // Regions: neighbouring patches joined while their normals stay within the cone.
        self.region_of.clear();
        self.region_of.resize(count, u32::MAX);
        self.regions.clear();
        self.members.clear();
        for seed in 0..count {
            if self.region_of[seed] != u32::MAX {
                continue;
            }
            let region = self.regions.len() as u32;
            let first = self.members.len() as u32;
            let (mut cone, mut bounds) = (self.cones[seed], self.bounds[seed]);
            self.region_of[seed] = region;
            self.queue.clear();
            self.queue.push(seed as u32);
            // A patch folded on its own stays alone.
            let grows = !self.hot[seed];
            while let Some(k) = self.queue.pop() {
                self.members.push(k);
                let (lo, hi) = self.bounds[k as usize];
                bounds = (bounds.0.min(lo), bounds.1.max(hi));
                if !grows {
                    continue;
                }
                for &m in &self.neighbours[k as usize] {
                    let m = m as usize;
                    if self.region_of[m] != u32::MAX || self.hot[m] {
                        continue;
                    }
                    let merged = merge_cones(cone, self.cones[m]);
                    if merged.1 <= PATCH_CONE {
                        cone = merged;
                        self.region_of[m] = region;
                        self.queue.push(m as u32);
                    }
                }
            }
            self.regions
                .push((bounds.0, bounds.1, first..self.members.len() as u32));
        }
        // Where two regions' bounds meet, the patches of each inside the other's bounds.
        for a in 0..self.regions.len() {
            for b in a + 1..self.regions.len() {
                let (la, ha, ra) = self.regions[a].clone();
                let (lb, hb, rb) = self.regions[b].clone();
                if !overlap((la, ha), (lb, hb)) {
                    continue;
                }
                for &pa in &self.members[ra.start as usize..ra.end as usize] {
                    for &pb in &self.members[rb.start as usize..rb.end as usize] {
                        let (pa, pb) = (pa as usize, pb as usize);
                        if overlap(self.bounds[pa], self.bounds[pb]) {
                            self.hot[pa] = true;
                            self.hot[pb] = true;
                        }
                    }
                }
            }
        }
    }

    /// The triangles of hot patches.
    fn hot_triangles(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.count())
            .filter(|&k| self.hot[k])
            .flat_map(|k| &self.tris[self.starts[k] as usize..self.starts[k + 1] as usize])
            .copied()
    }
}

impl SelfContacts {
    fn update(&mut self, pos: &[Vec3], w: &[f32], triangles: &[[u32; 3]], gap: f32) {
        let center = pos.iter().sum::<Vec3>() / pos.len().max(1) as f32;
        // How far each particle moved last step; it moves about as far in this one.
        self.motion.clear();
        if self.last.len() == pos.len() {
            let shift = center - self.last.iter().sum::<Vec3>() / pos.len() as f32;
            self.motion.extend(
                pos.iter()
                    .zip(&self.last)
                    .map(|(p, l)| (*p - *l - shift).length()),
            );
        } else {
            self.motion.resize(pos.len(), f32::INFINITY);
        }
        self.last.clear();
        self.last.extend_from_slice(pos);
        self.refresh(pos, w, triangles, gap, center);
        // Of those, the ones that can touch this step: a pair closes by at most what its
        // particle and its triangle's corners move, twice that for headroom. Cloth at rest
        // keeps only the layers lying on each other.
        self.close.clear();
        let motion = &self.motion;
        // Pairs come mostly grouped by triangle, so each triangle's normal is found once.
        let mut group = (u32::MAX, Vec3::ZERO, Vec3::ZERO, 0.0);
        for &(i, t) in &self.pairs {
            if group.0 != t {
                let tri = triangles[t as usize];
                let [a, b, c] = tri.map(|j| pos[j as usize]);
                let moves = tri.map(|j| motion[j as usize]);
                group = (
                    t,
                    a,
                    (b - a).cross(c - a).normalize_or_zero(),
                    moves[0].max(moves[1]).max(moves[2]),
                );
            }
            let reach = gap + 2.0 * (motion[i as usize] + group.3);
            if (pos[i as usize] - group.1).dot(group.2).abs() < reach {
                self.close.push((i, t));
            }
        }
    }

    /// Find the candidate pairs again where the cloth has bent by half the skin since they
    /// were found (a Verlet list kept per particle). Swinging and turning as a whole (a
    /// curtain swaying, a cape on a running character) doesn't count, since it changes no
    /// distances.
    fn refresh(&mut self, pos: &[Vec3], w: &[f32], triangles: &[[u32; 3]], gap: f32, center: Vec3) {
        let sample = triangles.len().min(64);
        let edge = triangles[..sample]
            .iter()
            .map(|t| (pos[t[1] as usize] - pos[t[0] as usize]).length())
            .sum::<f32>()
            / sample as f32;
        if self.at_search.len() == pos.len() {
            let mut a = Mat3::ZERO;
            for (p, s) in pos.iter().zip(&self.at_search) {
                let r = *p - center;
                a += Mat3::from_cols(r * s.x, r * s.y, r * s.z);
            }
            extract_rotation(&a, &mut self.turned, 3);
            let rot = Mat3::from_quat(self.turned);
            // Safe for one more step at each particle's last pace: no pair between particles
            // that stay under half the skin can have closed it.
            let half = 0.5 * self.skin;
            self.moved.clear();
            self.moved.extend(
                pos.iter()
                    .zip(&self.at_search)
                    .zip(&self.motion)
                    .map(|((p, s), m)| (*p - center - rot * *s).length() + m > half),
            );
            let count = self.moved.iter().filter(|&&m| m).count();
            if count == 0 {
                return;
            }
            if 3 * count < pos.len() {
                // Keep the pairs between particles that stayed, and search again only for
                // the moved ones.
                let moved = &self.moved;
                self.pairs.retain(|&(i, t)| {
                    !moved[i as usize] && !triangles[t as usize].iter().any(|&v| moved[v as usize])
                });
                self.search(pos, w, triangles, gap + self.skin, edge, true);
                let back = rot.transpose();
                for (i, p) in pos.iter().enumerate() {
                    if self.moved[i] {
                        self.at_search[i] = back * (*p - center);
                    }
                }
                return;
            }
        }
        // Under half a triangle, so neighbours in the same sheet (a triangle away) never
        // become candidates; only the pairs that can touch each step are solved, so a wide
        // skin costs little.
        self.skin = (0.5 * edge - gap).max(gap);
        self.at_search.clear();
        self.at_search.extend(pos.iter().map(|p| *p - center));
        self.turned = Quat::IDENTITY;
        self.pairs.clear();
        self.search(pos, w, triangles, gap + self.skin, edge, false);
    }

    /// Every free particle within `reach` of a triangle (not its own), over or beside it.
    /// With `moved_only`, only pairs with a moved particle (as the point, or a corner of the
    /// triangle) are added.
    fn search(
        &mut self,
        pos: &[Vec3],
        w: &[f32],
        triangles: &[[u32; 3]],
        reach: f32,
        edge: f32,
        moved_only: bool,
    ) {
        self.patches.heat(pos, triangles, reach);
        if !self.patches.hot.contains(&true) {
            return;
        }
        // Cells as wide as a triangle with its reach around it, so each triangle looks in at
        // most two cells along each axis.
        let inv = 1.0 / (1.5 * edge + 2.0 * reach).max(1e-3);
        // Dense hashing: cells hash into twice as many buckets as particles, filled by
        // counting.
        let mask = (2 * pos.len()).next_power_of_two() as u32 - 1;
        let bucket = |c: IVec3| -> usize {
            let h = (c.x.wrapping_mul(92_837_111))
                ^ (c.y.wrapping_mul(689_287_499))
                ^ (c.z.wrapping_mul(283_923_481));
            (h as u32 & mask) as usize
        };
        let cell_of = |p: Vec3| (p * inv).floor().as_ivec3();
        let moved = &self.moved;
        let fill_grid = |starts: &mut Vec<u32>,
                         bucketed: &mut Vec<(Vec3, u32)>,
                         cells: &mut Vec<u32>,
                         only_moved: bool| {
            starts.clear();
            starts.resize(mask as usize + 2, 0);
            cells.clear();
            cells.extend(pos.iter().zip(w).enumerate().map(|(i, (p, &w))| {
                if w == 0.0 || (only_moved && !moved[i]) {
                    return u32::MAX;
                }
                let b = bucket(cell_of(*p)) as u32;
                starts[b as usize + 1] += 1;
                b
            }));
            for b in 1..starts.len() {
                starts[b] += starts[b - 1];
            }
            bucketed.clear();
            bucketed.resize(starts[starts.len() - 1] as usize, (Vec3::ZERO, 0));
            let mut fill = starts.clone();
            for (i, &b) in cells.iter().enumerate() {
                if b != u32::MAX {
                    bucketed[fill[b as usize] as usize] = (pos[i], i as u32);
                    fill[b as usize] += 1;
                }
            }
        };
        fill_grid(&mut self.starts, &mut self.bucketed, &mut self.cells, false);
        if moved_only {
            fill_grid(
                &mut self.moved_starts,
                &mut self.moved_bucketed,
                &mut self.cells,
                true,
            );
        }
        let beside = -reach / edge.max(1e-3) - 0.05;
        let (all, few) = (
            (&*self.starts, &*self.bucketed),
            (&*self.moved_starts, &*self.moved_bucketed),
        );
        let (ring_starts, ring) = (&self.patches.ring_starts, &self.patches.ring);
        // Particles sharing a triangle with a corner: the two triangles share an edge or a
        // corner, so only a fold flat across it could bring them together, and the bending
        // stops that (Chaos and PhysX skip near neighbours the same way).
        let neighbour = |i: u32, tri: &[u32; 3]| {
            tri.iter().any(|&v| {
                ring[ring_starts[v as usize] as usize..ring_starts[v as usize + 1] as usize]
                    .contains(&i)
            })
        };
        let near = |t: u32, pairs: &mut Vec<(u32, u32)>| {
            let tri = &triangles[t as usize];
            // Only the moved particles over a triangle that stayed.
            let (starts, bucketed) = if moved_only && !tri.iter().any(|&v| moved[v as usize]) {
                few
            } else {
                all
            };
            let [a, b, c] = tri.map(|i| pos[i as usize]);
            let Some(n) = (b - a).cross(c - a).try_normalize() else {
                return;
            };
            let (lo, hi) = (a.min(b).min(c) - reach, a.max(b).max(c) + reach);
            let (clo, chi) = (cell_of(lo), cell_of(hi));
            if (chi - clo).max_element() > 4 {
                return; // torn loose and stretched across the scene
            }
            for z in clo.z..=chi.z {
                for y in clo.y..=chi.y {
                    for x in clo.x..=chi.x {
                        let slot = bucket(IVec3::new(x, y, z));
                        for &(p, i) in &bucketed[starts[slot] as usize..starts[slot + 1] as usize] {
                            if p.cmplt(lo).any()
                                || p.cmpgt(hi).any()
                                || tri.contains(&i)
                                || neighbour(i, tri)
                            {
                                continue;
                            }
                            let d = (p - a).dot(n);
                            if d.abs() < reach
                                && barycentric(p - n * d, a, b, c).iter().all(|&u| u > beside)
                            {
                                pairs.push((i, t));
                            }
                        }
                    }
                }
            }
        };
        // A big search splits over the compute pool, by triangles.
        self.hot.clear();
        self.hot.extend(self.patches.hot_triangles());
        let threads = leaf_pool().thread_num();
        let tasks = (self.hot.len() / SEARCH_CHUNK).clamp(1, threads);
        if tasks > 1 {
            let near = &near;
            let found = leaf_pool().scope(|scope| {
                for chunk in self.hot.chunks(self.hot.len().div_ceil(tasks)) {
                    scope.spawn(async move {
                        let mut pairs = Vec::new();
                        for &t in chunk {
                            near(t, &mut pairs);
                        }
                        pairs
                    });
                }
            });
            for f in found {
                self.pairs.extend(f);
            }
        } else {
            for &t in &self.hot {
                near(t, &mut self.pairs);
            }
        }
    }
}

/// Keep each candidate particle at least `gap` from its triangle, on the side it was on at
/// the start of the substep, with friction, sharing the push between them by inverse mass.
fn collide_self(
    pos: &mut [Vec3],
    prev: &[Vec3],
    w: &[f32],
    triangles: &[[u32; 3]],
    gap: f32,
    friction: f32,
    pairs: &[(u32, u32)],
) {
    for &(i, t) in pairs {
        let i = i as usize;
        let [ia, ib, ic] = triangles[t as usize].map(|j| j as usize);
        let (a, b, c) = (pos[ia], pos[ib], pos[ic]);
        let Some(n) = (b - a).cross(c - a).try_normalize() else {
            continue;
        };
        let d = (pos[i] - a).dot(n);
        if d.abs() >= gap {
            continue;
        }
        let bary = barycentric(pos[i] - n * d, a, b, c);
        if bary.iter().any(|&x| x < -0.02) {
            continue;
        }
        // The side it was on before this substep moved anything.
        let was = (prev[i] - prev[ia]).dot((prev[ib] - prev[ia]).cross(prev[ic] - prev[ia]));
        let side = if was.abs() > 1e-12 {
            was.signum()
        } else {
            d.signum()
        };
        let short = gap - side * d;
        if short <= 0.0 {
            continue;
        }
        let wt: f32 = [ia, ib, ic]
            .iter()
            .zip(bary)
            .map(|(&j, b)| w[j] * b * b)
            .sum();
        let total = w[i] + wt;
        if total <= 0.0 {
            continue;
        }
        // Friction: cancel up to `friction × push` of their sliding past each other.
        let moved = |j: usize| pos[j] - prev[j];
        let rel = moved(i) - (moved(ia) * bary[0] + moved(ib) * bary[1] + moved(ic) * bary[2]);
        let slide = rel - n * rel.dot(n);
        let limit = friction * short;
        let slide = if slide.length_squared() > limit * limit {
            slide * (limit / slide.length())
        } else {
            slide
        };
        let push = (n * (side * short) - slide) / total;
        pos[i] += push * w[i];
        for (&j, b) in [ia, ib, ic].iter().zip(bary) {
            pos[j] -= push * (w[j] * b);
        }
    }
}

/// Barycentric weights of `p` (in the triangle's plane) in triangle `a b c`.
fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
    let (d20, d21) = (v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-20 {
        return [1.0, 0.0, 0.0];
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    [1.0 - v - w, v, w]
}

/// How much a current swirls about its mean (0..1), in speed along it and sway across it.
const WATER_EDDIES: f32 = 0.5;
/// Water drag on cloth sliding along its own plane, relative to moving across it.
const SLIDE_DRAG: f32 = 0.3;

/// Two cheap, smooth pseudo-random signals in -1..1 that vary over space and time (gusts).
#[inline]
fn flutter(x: Vec3, t: f32) -> (f32, f32) {
    let a = (1.9 * x.x + 2.3 * x.z + 4.1 * t).sin() * (1.3 * x.y - 2.9 * t + 0.7 * x.x).sin();
    let b = (2.7 * x.z - 1.7 * x.y + 3.3 * t).sin() * (1.1 * x.x + 5.3 * t).sin();
    (a, b)
}

/// Coulomb friction for a particle pushed `push` meters out along `n`, relative to a surface
/// moving at `surface_vel`.
#[inline]
fn friction(x: Vec3, prev: Vec3, n: Vec3, push: f32, surface_vel: Vec3, h: f32, mu: f32) -> Vec3 {
    let dp = (x - prev) - surface_vel * h;
    let dt = dp - n * dp.dot(n);
    let lt = dt.length();
    if lt < 1e-12 {
        return x;
    }
    // Static friction holds when the slip is within the cone; otherwise kinetic (80%).
    if lt < mu * push {
        x - dt
    } else {
        x - dt * ((0.8 * mu * push) / lt).min(1.0)
    }
}

fn solve_shape(s: &mut ShapeMatch, pos: &mut [Vec3], w: &[f32], h: f32, inv_h2: f32) {
    let mut mass = 0.0;
    let mut com = Vec3::ZERO;
    for &i in &s.particles {
        let m = mass_of(w[i as usize]);
        mass += m;
        com += pos[i as usize] * m;
    }
    if mass <= 0.0 {
        return;
    }
    com /= mass;
    let mut a = Mat3::ZERO;
    for (k, &i) in s.particles.iter().enumerate() {
        let m = mass_of(w[i as usize]);
        let d = (pos[i as usize] - com) * m;
        let q = s.rest[k];
        a.x_axis += d * q.x;
        a.y_axis += d * q.y;
        a.z_axis += d * q.z;
    }
    extract_rotation(&a, &mut s.rotation, 4);
    let r = s.rotation;
    let creep = (s.plasticity * h).min(1.0);
    for (k, &i) in s.particles.iter().enumerate() {
        let i = i as usize;
        let goal = com + r * s.rest[k];
        let diff = goal - pos[i];
        if creep > 0.0 && diff.length() > s.yield_m {
            // Deformation past the yield point becomes part of the rest shape.
            s.rest[k] -= r.conjugate() * diff * creep;
        }
        let wi = w[i];
        if wi > 0.0 {
            pos[i] += diff * (wi / (wi + s.compliance * inv_h2));
        }
    }
    if creep > 0.0 {
        let mean = s.rest.iter().copied().sum::<Vec3>() / s.rest.len().max(1) as f32;
        for q in &mut s.rest {
            *q -= mean;
        }
    }
}

/// Mass for shape matching; pinned particles count as heavy.
#[inline]
fn mass_of(w: f32) -> f32 {
    if w > 0.0 { 1.0 / w } else { 1e3 }
}

/// Rotation part of `a`, refined in place from the warm start `q` (Müller, Bender, Chentanez
/// and Macklin 2016, "A Robust Method to Extract the Rotational Part of Deformations").
pub fn extract_rotation(a: &Mat3, q: &mut Quat, iterations: u32) {
    for _ in 0..iterations {
        let r = Mat3::from_quat(*q);
        let num = r.x_axis.cross(a.x_axis) + r.y_axis.cross(a.y_axis) + r.z_axis.cross(a.z_axis);
        let den =
            (r.x_axis.dot(a.x_axis) + r.y_axis.dot(a.y_axis) + r.z_axis.dot(a.z_axis)).abs() + 1e-9;
        let omega = num / den;
        let angle = omega.length();
        if angle < 1e-9 {
            break;
        }
        *q = (Quat::from_axis_angle(omega / angle, angle) * *q).normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collide::LocalShape;

    /// A 40 x 40 sheet of 5 cm cells in the XZ plane, folded across x = 1 m by `fold`
    /// (radians, 0 flat, PI flat on itself) with the halves `gap` m apart.
    fn sheet(fold: f32, gap: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let n = 41;
        let pos = (0..n * n)
            .map(|k| {
                let (x, z) = ((k % n) as f32 * 0.05, (k / n) as f32 * 0.05);
                if x <= 1.0 {
                    return Vec3::new(x, 0.0, z);
                }
                let r = Quat::from_rotation_z(fold);
                Vec3::new(1.0, gap * fold / PI, z) + r * Vec3::new(x - 1.0, 0.0, 0.0)
            })
            .collect();
        let mut tris = Vec::new();
        for j in 0..n as u32 - 1 {
            for i in 0..n as u32 - 1 {
                let a = j * n as u32 + i;
                tris.push([a, a + 1, a + n as u32]);
                tris.push([a + 1, a + n as u32 + 1, a + n as u32]);
            }
        }
        (pos, tris)
    }

    /// The search over a sheet crumpled everywhere (every patch hot). Run with
    /// `cargo test --release -p engine_softbody --lib self_collision_search_speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn self_collision_search_speed() {
        let (mut pos, tris) = sheet(0.0, 0.0);
        for p in &mut pos {
            p.y = 0.25 * (9.0 * p.x).sin() * (7.0 * p.z).sin();
        }
        let w = vec![1.0; pos.len()];
        let mut c = SelfContacts::default();
        c.search(&pos, &w, &tris, 0.026, 0.05, false);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            c.pairs.clear();
            c.search(&pos, &w, &tris, 0.026, 0.05, false);
        }
        println!(
            "search {:.3} ms, {} pairs, {} of {} hot",
            t.elapsed().as_secs_f64() * 50.0,
            c.pairs.len(),
            c.patches.hot.iter().filter(|h| **h).count(),
            c.patches.count()
        );
    }

    #[test]
    fn self_collision_searches_only_where_cloth_folds() {
        let mut patches = Patches::default();
        let (flat, tris) = sheet(0.0, 0.0);
        patches.heat(&flat, &tris, 0.03);
        assert!(patches.count() > 30);
        assert!(
            !patches.hot.contains(&true),
            "a flat sheet can't touch itself"
        );
        let (bent, _) = sheet(0.6, 0.0);
        patches.heat(&bent, &tris, 0.03);
        assert!(!patches.hot.contains(&true), "nor a gently bent one");
        // Folded over onto itself: the patches along both layers are searched.
        let (folded, _) = sheet(PI, 0.02);
        patches.heat(&folded, &tris, 0.03);
        let hot = patches.hot_triangles().count();
        assert!(
            hot > tris.len() / 2,
            "{hot} of {} triangles searched",
            tris.len()
        );
    }

    fn chain(n: usize, spacing: f32, mass: f32) -> Sim {
        let mut sim = Sim::new(DVec3::ZERO);
        let world = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
        for i in 0..n {
            sim.add_particle(Vec3::new(i as f32 * spacing, 0.0, 0.0), mass);
        }
        for i in 1..n {
            sim.add_distance(i as u32 - 1, i as u32, 0.0, u32::MAX);
        }
        sim.pin_here(0, world);
        sim
    }

    #[test]
    fn strands_stepped_side_by_side_match_strands_stepped_alone() {
        // 64 hanging strands of 20 in one body (split into runs on the pool), and each in a
        // body of its own (too small to split), in gusty wind over a sphere.
        let strand = |k: usize, sim: &mut Sim, frame: u16| {
            let first = sim.len() as u32;
            let root = Vec3::new(
                (k % 8) as f32 * 0.1 - 0.35,
                1.2,
                (k / 8) as f32 * 0.1 - 0.35,
            );
            for i in 0..20 {
                sim.add_particle(root + Vec3::new(0.03 * i as f32, 0.0, 0.0), 0.001);
            }
            for i in 1..20 {
                sim.add_distance(first + i - 1, first + i, 0.0, u32::MAX);
            }
            sim.pin_here(first, frame);
        };
        let setup = |sim: &mut Sim| {
            sim.params.drag = 2.5;
            sim.params.turbulence = 0.6;
            sim.params.wind = Vec3::new(2.0, 0.0, 1.0);
            sim.params.radius = 0.005;
        };
        let mut all = Sim::new(DVec3::ZERO);
        let frame = all.add_frame(Vec3::ZERO, Quat::IDENTITY);
        setup(&mut all);
        let mut alone: Vec<Sim> = (0..64)
            .map(|k| {
                let mut sim = Sim::new(DVec3::ZERO);
                let frame = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
                strand(k, &mut sim, frame);
                setup(&mut sim);
                sim
            })
            .collect();
        for k in 0..64 {
            strand(k, &mut all, frame);
        }
        let colliders = [LocalCollider::fixed(
            LocalShape::Sphere { radius: 0.4 },
            Vec3::new(0.0, 0.5, 0.0),
            Quat::IDENTITY,
        )];
        for _ in 0..60 {
            all.step(1.0 / 60.0, &colliders);
            for sim in &mut alone {
                sim.step(1.0 / 60.0, &colliders);
            }
        }
        assert!(all.split.starts.len() > 2, "the body split into runs");
        for (k, sim) in alone.iter().enumerate() {
            assert_eq!(sim.pos, all.pos[k * 20..(k + 1) * 20], "strand {k}");
        }
    }

    #[test]
    fn hanging_chain_keeps_its_length_and_comes_to_rest() {
        let mut sim = chain(20, 0.1, 0.1);
        sim.params.damping = 1.0;
        for _ in 0..600 {
            sim.step(1.0 / 60.0, &[]);
        }
        let len: f32 = sim.pos.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        assert!((len - 1.9).abs() < 0.02, "length {len}");
        let tip = sim.pos[19];
        assert!(tip.y < -1.85 && tip.x.abs() < 0.1, "tip {tip}");
        assert!(
            sim.kinetic_energy() < 1e-3,
            "energy {}",
            sim.kinetic_energy()
        );
    }

    #[test]
    fn heavy_load_does_not_stretch_a_rigid_rope() {
        let mut sim = chain(10, 0.2, 0.05);
        sim.params.substeps = 20;
        // 40 kg hanging from the tip.
        let tip = sim.add_particle(Vec3::new(2.0, 0.0, 0.0), 40.0);
        sim.add_distance(9, tip, 0.0, u32::MAX);
        sim.params.damping = 2.0;
        for _ in 0..600 {
            sim.step(1.0 / 60.0, &[]);
        }
        let len: f32 = sim.pos.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        assert!(len < 2.0 * 1.03, "rope stretched to {len} m");
    }

    #[test]
    fn particles_stay_out_of_colliders() {
        // A cloth-like row falling onto a sphere and a box.
        let mut sim = Sim::new(DVec3::ZERO);
        for i in 0..50 {
            let x = -1.25 + i as f32 * 0.05;
            sim.add_particle(Vec3::new(x, 2.0, 0.0), 0.01);
        }
        for i in 1..50 {
            sim.add_distance(i - 1, i, 0.0, u32::MAX);
        }
        sim.params.radius = 0.01;
        let colliders = [
            LocalCollider::fixed(
                LocalShape::Sphere { radius: 0.5 },
                Vec3::new(-0.6, 0.5, 0.0),
                Quat::IDENTITY,
            ),
            LocalCollider::fixed(
                LocalShape::Cuboid {
                    half: Vec3::splat(0.4),
                },
                Vec3::new(0.6, 0.4, 0.0),
                Quat::from_rotation_z(0.3),
            ),
        ];
        sim.ground = vec![[0.0, 1.0, 0.0, 0.0]];
        for _ in 0..240 {
            sim.step(1.0 / 60.0, &colliders);
            for p in &sim.pos {
                for c in &colliders {
                    let local = c.to_rot.conjugate() * (*p - c.to_pos);
                    let (d, _) = c.shape.sdf(local);
                    assert!(d > -0.005, "particle {p} inside {:?} by {d}", c.shape);
                }
                assert!(p.y > -0.001, "particle {p} under the ground");
            }
        }
    }

    #[test]
    fn fast_collider_sweeps_instead_of_tunneling() {
        // A sphere moves 1 m in one step through a hanging particle: interpolated poses push
        // the particle ahead of it instead of letting the particle end up behind.
        let mut sim = Sim::new(DVec3::ZERO);
        sim.add_particle(Vec3::ZERO, 0.01);
        sim.params.gravity = Vec3::ZERO;
        sim.params.substeps = 20;
        let c = LocalCollider {
            shape: LocalShape::Sphere { radius: 0.2 },
            from_pos: Vec3::new(-0.5, 0.0, 0.0),
            from_rot: Quat::IDENTITY,
            to_pos: Vec3::new(0.5, 0.0, 0.0),
            to_rot: Quat::IDENTITY,
            friction: 0.5,
            id: 0,
            dynamic: false,
        };
        sim.step(1.0 / 60.0, &[c]);
        assert!(sim.pos[0].x > 0.65, "particle at {}", sim.pos[0]);
    }

    #[test]
    fn dynamic_collider_receives_the_weight_resting_on_it() {
        // 1 kg of particles resting on a dynamic box: the box is pushed down by m·g·dt.
        let mut sim = Sim::new(DVec3::ZERO);
        for i in 0..10 {
            sim.add_particle(Vec3::new(i as f32 * 0.05 - 0.25, 0.52, 0.0), 0.1);
        }
        sim.params.radius = 0.02;
        sim.params.friction = 1.0;
        let mut c = LocalCollider::fixed(
            LocalShape::Cuboid {
                half: Vec3::splat(0.5),
            },
            Vec3::ZERO,
            Quat::IDENTITY,
        );
        c.dynamic = true;
        c.id = 0;
        let dt = 1.0 / 60.0;
        for _ in 0..120 {
            sim.step(dt, std::slice::from_ref(&c));
        }
        let j = sim.collider_impulses[0].linear;
        let expect = -9.81 * 1.0 * dt;
        assert!(
            (j.y - expect).abs() < 0.1 * expect.abs(),
            "impulse {j}, expected {expect}"
        );
    }

    #[test]
    fn goals_pull_frames_back() {
        // A 2 kg particle held by a stiff goal: the frame carries its weight.
        let mut sim = Sim::new(DVec3::ZERO);
        let f = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
        sim.add_particle(Vec3::ZERO, 2.0);
        sim.goals.push(Goal {
            p: 0,
            frame: f,
            local: Vec3::ZERO,
            compliance: 0.0,
        });
        let dt = 1.0 / 60.0;
        sim.step(dt, &[]);
        let j = sim.frame_impulses[f as usize].linear;
        assert!((j.y + 2.0 * 9.81 * dt).abs() < 1e-3, "{j}");
    }

    #[test]
    fn skin_planes_hold_particles_on_the_outer_side() {
        // A particle dropped on a tilted skin plane comes to lie on it, `margin` above.
        let mut sim = Sim::new(DVec3::ZERO);
        let tilt = Quat::from_rotation_z(0.3);
        let f = sim.add_frame(Vec3::new(0.0, 1.0, 0.0), tilt);
        let a = sim.add_particle(Vec3::new(0.0, 1.05, 0.0), 0.001);
        sim.skin_planes.push(SkinPlane {
            p: a,
            frame: f,
            point: Vec3::ZERO,
            normal: Vec3::Y,
            margin: 0.002,
        });
        for _ in 0..120 {
            sim.step(1.0 / 60.0, &[]);
            let height = (sim.pos[a as usize] - Vec3::Y).dot(tilt * Vec3::Y);
            assert!(height > 0.002 - 1e-4, "{height} m above the skin");
        }
        let height = (sim.pos[a as usize] - Vec3::Y).dot(tilt * Vec3::Y);
        assert!(height < 0.003, "rests on it: {height}");
    }

    #[test]
    fn skin_capsules_keep_particles_off_the_body_but_let_them_lift() {
        // A strand-end particle dropped onto a turned arm comes to rest on it, sliding round
        // without being popped off; one already deep inside is left where it is.
        let mut sim = Sim::new(DVec3::ZERO);
        let turn = Quat::from_rotation_y(0.7);
        let f = sim.add_frame(Vec3::new(0.0, 1.0, 0.0), turn);
        let a = sim.add_particle(Vec3::new(0.01, 1.08, 0.0), 0.001);
        let deep = sim.add_particle(Vec3::new(0.0, 1.0, 0.0), 0.001);
        for p in [a, deep] {
            sim.skin_capsules.push(SkinCapsule {
                p,
                frame: f,
                a: Vec3::new(0.0, 0.0, -0.2),
                b: Vec3::new(0.0, 0.0, 0.2),
                radius: 0.05,
                reach: 0.03,
            });
        }
        for step in 1..=120 {
            sim.step(1.0 / 60.0, &[]);
            let off = sim.pos[a as usize] - Vec3::Y;
            let r = off.reject_from(turn * Vec3::Z).length();
            assert!(r > 0.05 - 1e-4, "{r} m from the axis");
            // No faster than falling freely for as long.
            let speed = sim.vel[a as usize].length();
            let falling = 9.81 * step as f32 / 60.0;
            assert!(speed < falling + 0.3, "thrown at {speed} m/s");
        }
        assert!(sim.pos[deep as usize].y < 1.0, "the deep one falls through");
    }

    #[test]
    fn shape_matching_springs_back_and_plasticity_keeps_dents() {
        let build = |plasticity: f32| {
            let mut sim = Sim::new(DVec3::ZERO);
            let mut rest = Vec::new();
            let mut ids = Vec::new();
            for z in 0..3 {
                for y in 0..3 {
                    for x in 0..3 {
                        let p = Vec3::new(x as f32, y as f32, z as f32) * 0.25;
                        ids.push(sim.add_particle(p, 0.1));
                        rest.push(p - Vec3::splat(0.25));
                    }
                }
            }
            sim.shapes.push(ShapeMatch {
                particles: ids,
                rest,
                compliance: 1e-4,
                plasticity,
                yield_m: 0.02,
                rotation: Quat::IDENTITY,
            });
            sim.params.gravity = Vec3::ZERO;
            sim.params.damping = 3.0;
            sim
        };
        for (plasticity, expect_dent) in [(0.0, false), (20.0, true)] {
            let mut sim = build(plasticity);
            // Hold one corner squashed in for half a second, then let go.
            let world = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
            sim.pin(26, world, Vec3::splat(0.3));
            for _ in 0..30 {
                sim.step(1.0 / 60.0, &[]);
            }
            sim.unpin(26);
            for _ in 0..300 {
                sim.step(1.0 / 60.0, &[]);
            }
            // Distance from corner 26 to corner 0 at rest is 0.5·√3 ≈ 0.866.
            let diag = (sim.pos[26] - sim.pos[0]).length();
            let dented = diag < 0.8;
            assert_eq!(
                dented, expect_dent,
                "plasticity {plasticity}: diagonal {diag}"
            );
        }
    }

    #[test]
    fn extract_rotation_recovers_a_rotation() {
        let r = Quat::from_euler(engine_core::math::EulerRot::XYZ, 0.4, -1.1, 0.7);
        let a = Mat3::from_quat(r) * Mat3::from_diagonal(Vec3::new(1.3, 0.8, 1.1));
        let mut q = Quat::IDENTITY;
        extract_rotation(&a, &mut q, 40);
        assert!(q.dot(r).abs() > 0.9999, "{q} vs {r}");
    }

    #[test]
    fn wind_lifts_a_flag_toward_the_wind_direction() {
        let mut sim = Sim::new(DVec3::ZERO);
        let world = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
        let (w, h) = (12, 8);
        for j in 0..h {
            for i in 0..w {
                sim.add_particle(Vec3::new(i as f32 * 0.1, -(j as f32) * 0.1, 0.0), 0.002);
            }
        }
        let id = |i: usize, j: usize| (j * w + i) as u32;
        for j in 0..h {
            for i in 0..w {
                if i + 1 < w {
                    sim.add_distance(id(i, j), id(i + 1, j), 0.0, u32::MAX);
                }
                if j + 1 < h {
                    sim.add_distance(id(i, j), id(i, j + 1), 0.0, u32::MAX);
                }
                if i + 1 < w && j + 1 < h {
                    sim.aero_triangles
                        .push([id(i, j), id(i, j + 1), id(i + 1, j)]);
                    sim.aero_triangles
                        .push([id(i + 1, j), id(i, j + 1), id(i + 1, j + 1)]);
                }
            }
        }
        for j in 0..h {
            sim.pin_here(id(0, j), world);
        }
        sim.params.aero = Some(Aero {
            drag: 1.2,
            friction: 0.05,
            lift: 0.0,
        });
        sim.params.wind = Vec3::new(0.0, 0.0, 8.0);
        for _ in 0..300 {
            sim.step(1.0 / 60.0, &[]);
        }
        // The free edge is blown downwind (+Z) instead of hanging straight down.
        let tip = sim.pos[id(w - 1, 0) as usize];
        assert!(tip.z > 0.5, "tip {tip}");
        assert!(sim.pos.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn slow_motion_uses_fewer_substeps_and_moves_proportionally_less() {
        let mut fast = chain(2, 0.0, 1.0);
        fast.distances.clear();
        fast.unpin(0);
        let mut slow = fast.clone();
        for _ in 0..10 {
            fast.step(1.0 / 60.0, &[]);
        }
        for _ in 0..40 {
            slow.step(0.25 / 60.0, &[]);
        }
        assert!(slow.substeps_for(0.25 / 60.0) < fast.substeps_for(1.0 / 60.0));
        assert!(
            (fast.pos[0].y - slow.pos[0].y).abs() < 0.01,
            "{} vs {}",
            fast.pos[0].y,
            slow.pos[0].y
        );
    }

    #[test]
    fn rebase_keeps_world_positions() {
        // A rope far from the world origin, moved to a nearby coordinate origin.
        let mut sim = chain(3, 1.0, 1.0);
        sim.origin = DVec3::new(1e8, 0.0, -1e8);
        sim.step(1.0 / 60.0, &[]);
        let world: Vec<DVec3> = sim.pos.iter().map(|p| sim.origin + p.as_dvec3()).collect();
        sim.rebase(DVec3::new(1e8 + 5.0, 2.0, -1e8 - 3.0));
        for (p, w) in sim.pos.iter().zip(&world) {
            assert!((sim.origin + p.as_dvec3() - *w).length() < 1e-3);
        }
        sim.step(1.0 / 60.0, &[]);
        assert!(sim.pos.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn world_shift_keeps_places_and_turns_motion() {
        let mut sim = chain(4, 0.5, 1.0);
        sim.origin = DVec3::new(20.0, 3.0, -7.0);
        for _ in 0..10 {
            sim.step(1.0 / 60.0, &[]);
        }
        let world: Vec<DVec3> = sim.pos.iter().map(|p| sim.origin + p.as_dvec3()).collect();
        let vel = sim.vel.clone();
        let s = WorldShift {
            rotation: engine_core::math::DQuat::from_rotation_y(1.0),
            translation: DVec3::new(-5000.0, 0.0, 120.0),
            velocity: DVec3::new(0.0, 0.0, 3.0),
            angular_velocity: DVec3::ZERO,
        };
        sim.shift(&s);
        for (i, w) in world.iter().enumerate() {
            let now = sim.origin + sim.pos[i].as_dvec3();
            assert!(
                (now - s.point(*w)).length() < 1e-3,
                "{i}: {now} vs {}",
                s.point(*w)
            );
            assert!((sim.vel[i] - s.velocity_at(now, vel[i])).length() < 1e-4);
        }
        // The pinned end follows its frame, which turned with everything else.
        let f = sim.frames[0];
        assert!((f.to_pos - sim.pos[0]).length() < 1e-4);
        assert!((sim.params.gravity - s.direction(Vec3::new(0.0, -9.81, 0.0))).length() < 1e-4);
        sim.step(1.0 / 60.0, &[]);
        assert!(sim.pos.iter().all(|p| p.is_finite()));
        assert!((sim.pos[1] - sim.pos[0]).length() < 0.51);
    }
}
