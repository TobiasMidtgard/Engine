//! Buoyancy: things float, bob on waves, rock on wakes, drift with currents, and sink if dense.
//!
//! A [`Buoyancy`] component samples the water at a grid of probes over the object's box. Each
//! probe pushes up with the weight of the water it displaces (Archimedes) and drags against
//! the water's motion, at its own position, so a boat pitches over a swell and a log rolls in
//! a current.
//!
//! * Physics bodies (`engine_world::physics::RigidBody`) are left to the physics module, which
//!   floats them from their real collider shapes through the same `Water` query. A `Buoyancy`
//!   on one does nothing (it would count the lift twice).
//! * Everything else the water module moves itself with a small rigid-body integrator
//!   ([`Floating`]): cheap floating props (buoys, crates, ducks, debris), and floating that
//!   works without a physics module.

use std::sync::Mutex;

use engine_core::math::{DVec3, Mat3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::RigidBody;
use engine_world::{FocusPoints, Gravity, SimulationRange, WaterSample, WaterSplash, WorldImpulse};

use crate::interact::splash;
use crate::state::{SharedWater, WaterState};

/// Water density, kg/m³.
pub const WATER_DENSITY: f32 = 1000.0;

/// Makes an entity float (see the [module docs](self)).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Buoyancy {
    /// The object's box, half extents in meters (local axes).
    pub half_extents: Vec3,
    /// How much of the box is solid, 0..1 (a hull or a sphere fills less than its box: a
    /// sphere ≈ 0.52, a rowboat hull ≈ 0.45). Displaced volume = box volume × fill.
    pub fill: f32,
    /// Average density of the object, kg/m³: below 1000 floats (wood 600, ice 917), above
    /// sinks (steel 7800).
    pub density: f32,
    /// Probes along local x and z. More probes = smoother rocking.
    pub probes: [u32; 2],
    /// Water drag on movement through it (0 none .. 1 heavy).
    pub drag: f32,
    /// Drag on rotation (0..1).
    pub angular_drag: f32,
}

impl Default for Buoyancy {
    fn default() -> Self {
        Self {
            half_extents: Vec3::splat(0.5),
            fill: 1.0,
            density: 600.0,
            probes: [2, 2],
            drag: 0.5,
            angular_drag: 0.5,
        }
    }
}

impl Buoyancy {
    /// A box of `half_extents` with average `density`.
    pub fn new(half_extents: Vec3, density: f32) -> Self {
        Self {
            half_extents,
            density,
            ..Self::default()
        }
    }

    /// A ball of `radius` meters with average `density`.
    pub fn sphere(radius: f32, density: f32) -> Self {
        Self {
            half_extents: Vec3::splat(radius),
            fill: std::f32::consts::PI / 6.0,
            density,
            probes: [2, 2],
            ..Self::default()
        }
    }

    pub fn with_fill(mut self, fill: f32) -> Self {
        self.fill = fill;
        self
    }

    pub fn with_probes(mut self, x: u32, z: u32) -> Self {
        self.probes = [x.max(1), z.max(1)];
        self
    }

    /// Displaced volume when fully under, m³.
    pub fn volume(&self) -> f32 {
        8.0 * self.half_extents.x * self.half_extents.y * self.half_extents.z * self.fill.max(0.0)
    }

    /// Fraction of its height a free-floating object sits under water at rest (≥ 1 sinks).
    pub fn draft_fraction(&self) -> f32 {
        self.density / WATER_DENSITY
    }
}

/// Motion state of an entity the water module floats itself (no physics body). Gameplay can
/// read or set it (push a boat, spin a barrel).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Floating {
    /// m/s.
    pub velocity: Vec3,
    /// rad/s, world axes.
    pub angular_velocity: Vec3,
    /// Extra force this step (N, world), e.g. a motor or a paddle; cleared after use.
    pub force: Vec3,
    /// Extra torque this step (N·m, world); cleared after use.
    pub torque: Vec3,
    /// Fraction of the volume under water last step, 0..1.
    pub submerged: f32,
    /// Out of the `SimulationRange` of every focus: it stays where it is until one comes near
    /// (managed by the water module).
    #[serde(skip)]
    pub resting: bool,
}

/// Keeps a floating entity near an anchor (a moored buoy or boat). Only for entities the
/// water module floats itself; physics bodies use a `Joint`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tether {
    /// World anchor point.
    pub anchor: DVec3,
    /// Slack rope length, meters.
    pub length_m: f32,
    /// Rope stiffness, N/m.
    pub stiffness: f32,
}

impl Default for Tether {
    fn default() -> Self {
        Self {
            anchor: DVec3::ZERO,
            length_m: 3.0,
            stiffness: 400.0,
        }
    }
}

/// Forces on one object this step.
pub struct BuoyancyForces {
    /// (force N, world application point) per probe.
    pub probes: Vec<(Vec3, DVec3)>,
    /// Fraction of the volume under water.
    pub submerged: f32,
}

/// Buoyancy and drag on an object at `pos`/`rot` moving with `lin`/`ang` velocity.
#[allow(clippy::too_many_arguments)]
pub fn compute_forces(
    state: &WaterState,
    b: &Buoyancy,
    pos: DVec3,
    rot: Quat,
    lin: Vec3,
    ang: Vec3,
    up: Vec3,
    g: f32,
    scratch: &mut Vec<(Vec3, DVec3)>,
) -> f32 {
    scratch.clear();
    let he = b.half_extents.max(Vec3::splat(0.01));
    let [px, pz] = [b.probes[0].max(1), b.probes[1].max(1)];
    let n = (px * pz) as f32;
    let probe_volume = b.volume() / n;
    let probe_area = 4.0 * he.x * he.z / n;
    let body_up = rot * Vec3::Y;
    let mut submerged = 0.0;
    for iz in 0..pz {
        for ix in 0..px {
            let lx = if px == 1 {
                0.0
            } else {
                (ix as f32 / (px - 1) as f32 * 2.0 - 1.0) * he.x * 0.7
            };
            let lz = if pz == 1 {
                0.0
            } else {
                (iz as f32 / (pz - 1) as f32 * 2.0 - 1.0) * he.z * 0.7
            };
            let center = pos + (rot * Vec3::new(lx, 0.0, lz)).as_dvec3();
            let Some(water): Option<WaterSample> = state.sample(center) else {
                continue;
            };
            // The probe is a column along the body's up axis; how much of it is under water?
            let span = (body_up * he.y).dot(up).abs().max(he.y * 0.3);
            let bottom = center.y - span as f64;
            let frac = (((water.surface_height - bottom) as f32) / (2.0 * span)).clamp(0.0, 1.0);
            if frac <= 0.0 {
                continue;
            }
            submerged += frac / n;
            let lift = up * (WATER_DENSITY * g * probe_volume * frac);
            let at_y = bottom + (frac * span) as f64;
            let at = DVec3::new(center.x, at_y, center.z);
            // Drag against the water (quadratic + a little linear so slow drift settles).
            let r = (at - pos).as_vec3();
            let v_point = lin + ang.cross(r);
            let rel = v_point - water.velocity;
            let speed = rel.length();
            let drag = -rel
                * (b.drag
                    * frac
                    * (0.5 * WATER_DENSITY * probe_area * speed
                        + 60.0 * probe_volume * WATER_DENSITY * 0.01));
            scratch.push((lift + drag, at));
        }
    }
    submerged
}

/// Integrate [`Floating`] entities (physics bodies float in the physics module). Those out of
/// the `SimulationRange` of every focus rest where they are; the rest step in parallel.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn buoyancy_system(
    time: Res<Time>,
    shared: Res<SharedWater>,
    gravity: Res<Gravity>,
    focus: Res<FocusPoints>,
    range: Res<SimulationRange>,
    new: Query<Entity, (With<Buoyancy>, Without<Floating>, Without<RigidBody>)>,
    mut floats: Query<
        (
            Entity,
            &mut Transform,
            &Buoyancy,
            &mut Floating,
            Option<&Tether>,
            Option<&LocalTimeScale>,
        ),
        Without<RigidBody>,
    >,
    mut blasts: MessageReader<WorldImpulse>,
    mut splash_out: MessageWriter<WaterSplash>,
    mut commands: Commands,
) {
    for entity in &new {
        commands.entity(entity).insert(Floating::default());
    }
    let blasts: Vec<WorldImpulse> = blasts.read().copied().collect();
    let hits = Mutex::new(Vec::new());
    // Held across the parallel step: everything that writes the water runs in
    // `SimSet::Environment`, after this set, so no writer can be waiting on it meanwhile.
    let state = shared.0.read();
    floats
        .par_iter_mut()
        .for_each(|(entity, mut transform, b, mut fl, tether, local)| {
            let dt = time.delta_for(local);
            if dt <= 0.0 {
                return;
            }
            let pos = transform.translation;
            let reach = b.half_extents.length() as f64;
            let mut resting = fl.resting;
            let rests = crate::rests(&range, &focus, pos, reach, &mut resting);
            if resting != fl.resting {
                fl.resting = resting;
            }
            if rests {
                return;
            }
            let (p, q, hit) = step_float(
                &state,
                &gravity,
                &blasts,
                entity,
                b,
                &mut fl,
                tether,
                pos,
                transform.rotation,
                dt,
            );
            transform.translation = p;
            transform.rotation = q;
            if let Some(hit) = hit {
                hits.lock().unwrap().push(hit);
            }
        });
    drop(state);
    let hits = hits.into_inner().unwrap();
    if !hits.is_empty() {
        let mut state = shared.0.write();
        for (s, d) in hits {
            state.ripples.disturb(d);
            splash_out.write(s);
        }
    }
}

/// One step of one floating thing: its new place and turn, and the splash it makes if it
/// just hit the water.
#[allow(clippy::too_many_arguments)]
fn step_float(
    state: &WaterState,
    gravity: &Gravity,
    blasts: &[WorldImpulse],
    entity: Entity,
    b: &Buoyancy,
    fl: &mut Floating,
    tether: Option<&Tether>,
    pos: DVec3,
    rot: Quat,
    dt: f32,
) -> (
    DVec3,
    Quat,
    Option<(WaterSplash, crate::ripples::Disturbance)>,
) {
    let gvec = gravity.at(pos);
    let g = gvec.length();
    let up = if g > 1e-6 { -gvec / g } else { Vec3::Y };
    let he = b.half_extents.max(Vec3::splat(0.01));
    let m = (b.density * b.volume()).max(0.01);
    // Box inertia (fill scales mass, not shape).
    let inertia = Vec3::new(
        he.y * he.y + he.z * he.z,
        he.x * he.x + he.z * he.z,
        he.x * he.x + he.y * he.y,
    ) * (m / 3.0);
    let mut scratch = Vec::with_capacity((b.probes[0] * b.probes[1]) as usize);
    let submerged = compute_forces(
        state,
        b,
        pos,
        rot,
        fl.velocity,
        fl.angular_velocity,
        up,
        g,
        &mut scratch,
    );
    let mut force = gvec * m + fl.force;
    let mut torque = fl.torque;
    for (f, at) in scratch.iter() {
        force += *f;
        torque += (*at - pos).as_vec3().cross(*f);
    }
    // Rotational drag in the water.
    torque -=
        fl.angular_velocity * (b.angular_drag * submerged * m * (he.x * he.x + he.z * he.z) * 4.0);
    if let Some(t) = tether {
        let d = (t.anchor - pos).as_vec3();
        let horizontal = d - up * d.dot(up);
        let stretch = horizontal.length() - t.length_m;
        if stretch > 0.0 {
            force += horizontal.normalize() * stretch * t.stiffness;
        }
    }
    // Blasts throw floating things (physics bodies get them from the physics module).
    for i in blasts {
        let d = (pos - i.center).as_vec3();
        let dist = d.length();
        if dist < i.radius {
            let k = 1.0 - dist / i.radius.max(0.01);
            let dir = (d / dist.max(0.1) + up * 1.5).normalize();
            fl.velocity += dir * i.strength * k * k * 0.35;
            fl.angular_velocity += Vec3::new(d.z, 0.0, -d.x).normalize_or_zero() * k * 2.0;
        }
    }
    // Hitting the water: a splash.
    let down = -fl.velocity.dot(up);
    let hit = (fl.submerged < 0.02 && submerged > 0.0 && down > 2.0).then(|| {
        let r = he.x.max(he.z);
        (
            WaterSplash {
                position: pos,
                velocity: fl.velocity,
                radius: r,
                mass_kg: (m * down * 0.02).min(5000.0),
                source: Some(entity),
            },
            splash(pos, r, down),
        )
    });
    fl.force = Vec3::ZERO;
    fl.torque = Vec3::ZERO;
    fl.submerged = submerged;

    // Semi-implicit Euler.
    fl.velocity += force / m * dt;
    let r = Mat3::from_quat(rot);
    let inv_world = r * Mat3::from_diagonal(inertia.recip()) * r.transpose();
    fl.angular_velocity += inv_world * torque * dt;
    // Air drag so things in the air don't spin forever.
    fl.angular_velocity *= (1.0 - 0.05 * dt).max(0.0);
    let mut p = pos + (fl.velocity * dt).as_dvec3();
    let w = fl.angular_velocity;
    let angle = w.length() * dt;
    let mut q = if angle > 1e-7 {
        Quat::from_axis_angle(w / w.length(), angle) * rot
    } else {
        rot
    };
    q = q.normalize();

    // Sinkers come to rest on the bed.
    if let Some(bed) = state.bed_height(p) {
        let reach = (he * Vec3::new(1.0, 1.0, 1.0)).dot((q.inverse() * up).abs());
        let floor = bed + reach as f64;
        if p.y < floor {
            p.y = floor;
            let down = fl.velocity.dot(up);
            if down < 0.0 {
                fl.velocity -= up * down;
            }
            fl.velocity *= (1.0 - 4.0 * dt).max(0.0);
            fl.angular_velocity *= (1.0 - 4.0 * dt).max(0.0);
            // Settle flat on the bottom.
            let tilt = Quat::from_rotation_arc(q * Vec3::Y, up);
            q = Quat::IDENTITY.slerp(tilt, (2.0 * dt).min(1.0)) * q;
        }
    }
    (p, q, hit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{WaterBed, WaterBody, WaterExtent, WaveSettings};
    use crate::state::BodyState;
    use crate::waves::WaveSet;

    fn calm_lake() -> WaterState {
        let mut s = WaterState::new(16, 1.0);
        s.bodies.push(BodyState {
            entity: Entity::PLACEHOLDER,
            center: DVec3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            body: WaterBody {
                extent: WaterExtent::Infinite,
                bed: WaterBed::Flat { depth_m: 10.0 },
                waves: WaveSettings {
                    wind_waves: false,
                    ..Default::default()
                },
                interactive: false,
                ..Default::default()
            },
            waves: WaveSet::default(),
            mean_depth: 10.0,
            wind: Vec3::ZERO,
        });
        s
    }

    /// Settle a floating box and return its center height.
    fn settle(b: &Buoyancy) -> f64 {
        let state = calm_lake();
        let he = b.half_extents;
        let m = b.density * b.volume();
        let mut y = 1.0f64;
        let mut v = 0.0f32;
        let mut scratch = Vec::new();
        for _ in 0..60 * 30 {
            compute_forces(
                &state,
                b,
                DVec3::new(0.0, y, 0.0),
                Quat::IDENTITY,
                Vec3::new(0.0, v, 0.0),
                Vec3::ZERO,
                Vec3::Y,
                9.81,
                &mut scratch,
            );
            let f: f32 = scratch.iter().map(|(f, _)| f.y).sum::<f32>() - m * 9.81;
            v += f / m / 60.0;
            y += (v / 60.0) as f64;
            if y < -10.0 + he.y as f64 {
                y = -10.0 + he.y as f64;
                v = 0.0;
            }
        }
        y
    }

    #[test]
    fn wood_floats_with_sixty_percent_under() {
        let b = Buoyancy::new(Vec3::new(0.5, 0.25, 0.5), 600.0);
        let y = settle(&b);
        // Draft = 0.6 × 0.5 m = 0.3 m: center sits 0.25 - 0.3 = -0.05 m.
        assert!((y + 0.05).abs() < 0.01, "{y}");
    }

    #[test]
    fn ice_floats_low_and_steel_sinks() {
        let ice = settle(&Buoyancy::new(Vec3::splat(0.5), 917.0));
        assert!((ice - (0.5 - 0.917)).abs() < 0.02, "{ice}");
        let steel = settle(&Buoyancy::new(Vec3::splat(0.2), 7800.0));
        assert!(steel < -9.0, "{steel}");
    }

    #[test]
    fn current_drags_floating_things_along() {
        let mut state = calm_lake();
        state.bodies[0].body.current = [1.0, 0.0];
        let b = Buoyancy::new(Vec3::new(0.5, 0.2, 0.5), 500.0);
        let mut scratch = Vec::new();
        compute_forces(
            &state,
            &b,
            DVec3::new(0.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::Y,
            9.81,
            &mut scratch,
        );
        let fx: f32 = scratch.iter().map(|(f, _)| f.x).sum();
        assert!(fx > 1.0, "{fx}");
    }
}
