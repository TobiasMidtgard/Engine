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

use engine_core::math::{DVec3, Mat3, Quat, Vec3};

use crate::collide::{LocalCollider, PosedCollider};

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
        }
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
    accel: Vec<Vec3>,
    posed: Vec<PosedCollider>,
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

    /// Release particle `p`'s pin, if any.
    pub fn unpin(&mut self, p: u32) {
        let i = p as usize;
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
        let n = self.substeps_for(dt);
        let h = dt / n as f32;
        for k in 0..n {
            let t0 = k as f32 / n as f32;
            let t1 = (k + 1) as f32 / n as f32;
            self.substep(h, t0, t1, dt, colliders);
        }
        self.params.time += dt;
        self.finish_step();
    }

    fn finish_step(&mut self) {
        for f in &mut self.frames {
            f.from_pos = f.to_pos;
            f.from_rot = f.to_rot;
        }
        self.loads.clear();
    }

    fn substep(&mut self, h: f32, t0: f32, t1: f32, dt: f32, colliders: &[LocalCollider]) {
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

        // External accelerations: gravity, air, loads.
        self.accel.clear();
        self.accel.resize(self.pos.len(), p.gravity);
        let time = p.time + t0 * dt;
        let side = p.wind.cross(Vec3::Y);
        let gust = |x: Vec3| -> Vec3 {
            if p.turbulence <= 0.0 {
                return p.wind;
            }
            let (f, g) = flutter(x, time);
            p.wind * (1.0 + p.turbulence * f) + side * (0.5 * p.turbulence * g)
        };
        if p.drag > 0.0 {
            for ((a, v), x) in self.accel.iter_mut().zip(&self.vel).zip(&self.pos) {
                *a += (gust(*x) - *v) * p.drag;
            }
        }
        if let Some(aero) = p.aero {
            for tri in &self.aero_triangles {
                let [a, b, c] = tri.map(|i| i as usize);
                let n2 = (self.pos[b] - self.pos[a]).cross(self.pos[c] - self.pos[a]);
                let len = n2.length();
                if len < 1e-12 {
                    continue;
                }
                let area = 0.5 * len;
                let n = n2 / len;
                let center = (self.pos[a] + self.pos[b] + self.pos[c]) * (1.0 / 3.0);
                let v_rel = gust(center) - (self.vel[a] + self.vel[b] + self.vel[c]) * (1.0 / 3.0);
                let vn = v_rel.dot(n);
                let vt = v_rel - n * vn;
                let force = (n * (aero.drag * vn * vn.abs()) + vt * (aero.friction * vt.length()))
                    * (0.5 * AIR_DENSITY * area / 3.0);
                for i in [a, b, c] {
                    self.accel[i] += force * self.inv_mass[i];
                }
            }
        }
        for &(i, f) in &self.loads {
            let i = i as usize;
            self.accel[i] += f * self.inv_mass[i];
        }

        // Predict.
        for i in 0..self.pos.len() {
            self.prev[i] = self.pos[i];
            if self.inv_mass[i] > 0.0 {
                self.vel[i] += self.accel[i] * h;
                self.pos[i] += self.vel[i] * h;
            }
        }
        for pin in &self.pins {
            self.pos[pin.p as usize] = self.frames[pin.frame as usize].point(pin.local, t1);
        }

        // Constraints.
        let pos = &mut self.pos;
        let w = &self.inv_mass;
        for c in &self.distances {
            let (a, b) = (c.a as usize, c.b as usize);
            let wsum = w[a] + w[b];
            if wsum == 0.0 {
                continue;
            }
            let d = pos[a] - pos[b];
            let len = d.length();
            if len < 1e-9 {
                continue;
            }
            let dl = -(len - c.rest) / (wsum + c.compliance * inv_h2);
            let corr = d * (dl / len);
            pos[a] += corr * w[a];
            pos[b] -= corr * w[b];
        }
        for c in &self.tethers {
            let (i, a) = (c.p as usize, c.anchor as usize);
            if w[i] == 0.0 {
                continue;
            }
            let d = pos[i] - pos[a];
            let len = d.length();
            if len > c.max {
                pos[i] -= d * ((len - c.max) / len);
            }
        }
        for g in &self.goals {
            let i = g.p as usize;
            if w[i] == 0.0 {
                continue;
            }
            let target = self.frames[g.frame as usize].point(g.local, t1);
            let dx = (target - pos[i]) * (w[i] / (w[i] + g.compliance * inv_h2));
            pos[i] += dx;
            // Reaction on the frame: equal and opposite impulse.
            let j = dx * (inv_h / w[i]);
            let fi = &mut self.frame_impulses[g.frame as usize];
            fi.linear -= j;
            fi.angular -= (pos[i] - self.frames[g.frame as usize].to_pos).cross(j);
        }
        for s in &mut self.shapes {
            solve_shape(s, pos, w, h, inv_h2);
        }

        // Collisions.
        self.posed.clear();
        let margin = p.radius + p.max_speed * h;
        self.posed.extend(
            colliders
                .iter()
                .map(|c| PosedCollider::at(c, t1, dt, margin)),
        );
        let mut contacts = 0;
        let shared_ground = (self.ground.len() == 1).then(|| self.ground[0]);
        for i in 0..pos.len() {
            if w[i] == 0.0 {
                continue;
            }
            let x0 = pos[i];
            let mut x = x0;
            // Ground.
            let g = shared_ground.or_else(|| self.ground.get(i).copied());
            if let Some(g) = g {
                let n = Vec3::new(g[0], g[1], g[2]);
                let d = n.dot(x) + g[3] - p.radius;
                if d < 0.0 {
                    let approach = -n.dot(self.vel[i]);
                    if approach > self.ground_impact.0 {
                        self.ground_impact = (approach, x);
                    }
                    x -= n * d;
                    x = friction(x, self.prev[i], n, -d, Vec3::ZERO, h, p.friction);
                }
            }
            // Colliders.
            for (ci, c) in self.posed.iter().enumerate() {
                let Some((d, n)) = c.query(x) else {
                    continue;
                };
                if d >= p.radius {
                    continue;
                }
                contacts += 1;
                let push = p.radius - d;
                let before = x;
                x += n * push;
                let mu = (p.friction * c.friction).sqrt();
                x = friction(x, self.prev[i], n, push, c.velocity_at(x), h, mu);
                if colliders[ci].dynamic {
                    let j = (x - before) * (inv_h / w[i]);
                    let imp = &mut self.collider_impulses[ci];
                    imp.linear -= j;
                    imp.angular -= (x - colliders[ci].to_pos).cross(j);
                }
            }
            pos[i] = x;
        }
        self.contacts = contacts;

        // Velocities.
        let damp = (1.0 - p.damping * h).max(0.0);
        let max2 = p.max_speed * p.max_speed;
        for i in 0..pos.len() {
            let mut v = (pos[i] - self.prev[i]) * inv_h;
            if w[i] > 0.0 {
                v *= damp;
                let s2 = v.length_squared();
                if s2 > max2 {
                    v *= p.max_speed / s2.sqrt();
                }
            }
            self.vel[i] = v;
        }
    }
}

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
            sim.step(dt, &[c]);
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
}
