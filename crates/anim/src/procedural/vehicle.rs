//! Vehicle animation: wheels that spin with the ground speed and steer into turns, springy
//! suspension that follows bumps, and a body that rolls in corners, squats under throttle and
//! dives under braking. It reads how the vehicle moves (and `VehicleControls` when present),
//! so it works on top of any vehicle simulation, or on a vehicle moved along a path.

use engine_core::prelude::*;
use engine_world::locomotion::VehicleControls;
use engine_world::physics::PhysicsQueries;
use engine_world::{ContactKind, Ground, SurfaceId};

use crate::animator::Animator;
use crate::ik::{GroundProbe, IkSurfaces, to_world};
use crate::xform::smoothing;

/// One wheel of a [`VehicleRig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelRig {
    /// Bone that spins (about its local X axis).
    pub bone: String,
    /// Bone that steers (about model up) and moves with the suspension. Defaults to `bone`.
    pub mount: Option<String>,
    pub radius: f32,
    pub steers: bool,
    #[serde(skip)]
    pub(crate) spin: f32,
    #[serde(skip)]
    pub(crate) compression: f32,
    #[serde(skip)]
    pub(crate) velocity: f32,
    #[serde(skip)]
    pub(crate) distance: f32,
}

impl Default for WheelRig {
    fn default() -> Self {
        Self {
            bone: String::new(),
            mount: None,
            radius: 0.33,
            steers: false,
            spin: 0.0,
            compression: 0.0,
            velocity: 0.0,
            distance: 0.0,
        }
    }
}

/// Procedural vehicle animation. See the module docs.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VehicleRig {
    pub enabled: bool,
    /// Body bone that rolls and pitches.
    pub body: String,
    pub wheels: Vec<WheelRig>,
    /// Suspension travel up and down from rest, meters.
    pub travel: f32,
    /// Suspension natural frequency, Hz (1.2 soft car .. 3 race car).
    pub frequency: f32,
    /// Damping ratio (0.3 bouncy .. 1 no overshoot).
    pub damping: f32,
    /// Body roll per m/s² of sideways acceleration, degrees.
    pub roll_deg_per_g: f32,
    /// Body pitch per m/s² of forward acceleration, degrees.
    pub pitch_deg_per_g: f32,
    /// Largest steering angle, degrees.
    pub max_steer_deg: f32,
    /// Distance between front and rear axles, meters (steering from the turn rate).
    pub wheelbase: f32,
    /// Emit a ground contact (tire track) every this many meters per wheel (0 = never).
    pub track_spacing: f32,
    pub mass_kg: f32,
    #[serde(skip)]
    pub(crate) rt: VehicleRuntime,
}

impl Default for VehicleRig {
    fn default() -> Self {
        Self {
            enabled: true,
            body: "body".into(),
            wheels: Vec::new(),
            travel: 0.12,
            frequency: 1.6,
            damping: 0.45,
            roll_deg_per_g: 4.0,
            pitch_deg_per_g: 2.5,
            max_steer_deg: 32.0,
            wheelbase: 2.6,
            track_spacing: 0.5,
            mass_kg: 1300.0,
            rt: VehicleRuntime::default(),
        }
    }
}

/// A wheel rolled over a spot (for tire tracks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelContact {
    pub position: DVec3,
    pub normal: Vec3,
    pub surface: SurfaceId,
    pub yaw: f32,
    pub speed: f32,
    pub load_kg: f32,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct VehicleRuntime {
    prev_origin: Option<DVec3>,
    prev_forward: Option<Vec3>,
    velocity: Vec3,
    accel: Vec3,
    yaw_rate: f32,
    steer: f32,
    roll: f32,
    pitch: f32,
    pub(crate) contacts: Vec<WheelContact>,
}

impl VehicleRig {
    /// Forward speed, m/s (negative when reversing).
    pub fn forward_speed(&self, forward: Vec3) -> f32 {
        self.rt.velocity.dot(forward)
    }

    pub fn steer_angle(&self) -> f32 {
        self.rt.steer
    }

    pub fn take_contacts(&mut self) -> Vec<WheelContact> {
        std::mem::take(&mut self.rt.contacts)
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn vehicle_rig_system(
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(
        Entity,
        &mut Animator,
        &mut VehicleRig,
        &GlobalTransform,
        Option<&VehicleControls>,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(entity, mut animator, mut rig, gt, controls)| {
            if !rig.enabled {
                return;
            }
            let probe = GroundProbe {
                ground: &ground,
                physics: physics.as_deref(),
                surfaces: &surfaces.0,
                exclude: Some(entity),
            };
            step_vehicle(&mut animator, &mut rig, gt, controls, &probe);
        });
}

/// Animate one vehicle for this frame.
pub fn step_vehicle(
    animator: &mut Animator,
    rig: &mut VehicleRig,
    gt: &GlobalTransform,
    controls: Option<&VehicleControls>,
    probe: &GroundProbe,
) {
    let dt = animator.dt;
    let skeleton = animator.skeleton.clone();
    let Some(body) = skeleton.index_of(&rig.body) else {
        return;
    };
    let origin = gt.translation();
    let forward = gt.forward();
    let right = gt.right();
    let rt = &mut rig.rt;
    if dt > 0.0 {
        if let Some(prev) = rt.prev_origin {
            let v = ((origin - prev) / dt as f64).as_vec3();
            if v.length() < 150.0 {
                let a = (v - rt.velocity) / dt;
                rt.accel = rt.accel.lerp(a, smoothing(dt, 0.15));
                rt.velocity = v;
            }
        }
        if let Some(pf) = rt.prev_forward {
            let yaw = pf.cross(forward).y.clamp(-1.0, 1.0).asin();
            rt.yaw_rate += (yaw / dt - rt.yaw_rate) * smoothing(dt, 0.1);
        }
    }
    rt.prev_origin = Some(origin);
    rt.prev_forward = Some(forward);
    let v_fwd = rt.velocity.dot(forward);

    // Steering (positive = right, like `VehicleControls::steer`): the driver's input if there
    // is one, else the angle that makes this turn rate (a left turn has a positive yaw rate).
    let max = rig.max_steer_deg.to_radians();
    let steer_target = match controls {
        Some(c) => c.steer * max,
        None if v_fwd.abs() > 0.3 => {
            (-(rig.wheelbase * rt.yaw_rate / v_fwd).atan()).clamp(-max, max)
        }
        None => rt.steer,
    };
    rt.steer += (steer_target - rt.steer) * smoothing(dt, 0.12);

    // Body roll and pitch from acceleration, sprung: the body leans out of turns (a right turn
    // accelerates to the right and drops the left side), squats under throttle and dives
    // under braking.
    let lat = rt.accel.dot(right);
    let lon = rt.accel.dot(forward);
    let roll_t = (-lat * rig.roll_deg_per_g / 9.81)
        .to_radians()
        .clamp(-0.2, 0.2);
    let pitch_t = (lon * rig.pitch_deg_per_g / 9.81)
        .to_radians()
        .clamp(-0.12, 0.12);
    rt.roll += (roll_t - rt.roll) * smoothing(dt, 0.15);
    rt.pitch += (pitch_t - rt.pitch) * smoothing(dt, 0.15);

    animator.refresh_model();
    let up_model = (gt.rotation().conjugate() * Vec3::Y).normalize();
    let omega = std::f32::consts::TAU * rig.frequency;
    let wheel_count = rig.wheels.len().max(1) as f32;
    let mut comp_front = 0.0;
    let mut comp_back = 0.0;
    let mut comp_left = 0.0;
    let mut comp_right = 0.0;
    let mut front_n = 0.0f32;
    let mut back_n = 0.0f32;
    let mut left_n = 0.0f32;
    let mut right_n = 0.0f32;
    let mut wheel_bones = Vec::new();
    for w in rig.wheels.iter_mut() {
        let Some(bone) = skeleton.index_of(&w.bone) else {
            continue;
        };
        let mount = w
            .mount
            .as_deref()
            .and_then(|m| skeleton.index_of(m))
            .unwrap_or(bone);
        let rest = skeleton.rest_model[bone].t;
        // Suspension: where would the wheel sit on the ground under it?
        let center_world = to_world(gt, rest);
        let target = match probe.probe(
            center_world,
            rig.travel + w.radius,
            rig.travel + w.radius + 0.5,
        ) {
            Some(h) => {
                let want = h.height + w.radius as f64;
                if w.distance >= rig.track_spacing && rig.track_spacing > 0.0 {
                    w.distance = 0.0;
                    rt.contacts.push(WheelContact {
                        position: DVec3::new(center_world.x, h.height, center_world.z),
                        normal: h.normal,
                        surface: h.surface,
                        yaw: forward.x.atan2(-forward.z),
                        speed: v_fwd.abs(),
                        load_kg: rig.mass_kg / wheel_count,
                        width: w.radius * 0.6,
                    });
                }
                ((want - center_world.y) as f32).clamp(-rig.travel, rig.travel)
            }
            None => -rig.travel,
        };
        // Spring-damper toward the target.
        if dt > 0.0 {
            let x = w.compression;
            let a = omega * omega * (target - x) - 2.0 * rig.damping * omega * w.velocity;
            w.velocity += a * dt;
            w.compression = (x + w.velocity * dt).clamp(-rig.travel, rig.travel);
        }
        w.spin = (w.spin + v_fwd * dt / w.radius.max(0.05)).rem_euclid(std::f32::consts::TAU);
        w.distance += (v_fwd * dt).abs();
        if rest.z < 0.0 {
            comp_front += w.compression;
            front_n += 1.0;
        } else {
            comp_back += w.compression;
            back_n += 1.0;
        }
        if rest.x < 0.0 {
            comp_left += w.compression;
            left_n += 1.0;
        } else {
            comp_right += w.compression;
            right_n += 1.0;
        }
        wheel_bones.push((bone, mount, w.compression, w.spin, w.steers));
    }
    let avg = |s: f32, n: f32| if n > 0.0 { s / n } else { 0.0 };
    let track = rig
        .wheels
        .iter()
        .filter_map(|w| skeleton.index_of(&w.bone))
        .map(|b| skeleton.rest_model[b].t.x.abs())
        .fold(0.5f32, f32::max)
        * 2.0;
    let wheelbase = rig.wheelbase.max(0.5);
    // The body sits on the springs: average height, pitch and roll from the wheels.
    let heave = (comp_front + comp_back) / (front_n + back_n).max(1.0);
    let pitch_susp = ((avg(comp_front, front_n) - avg(comp_back, back_n)) / wheelbase).atan();
    let roll_susp = ((avg(comp_left, left_n) - avg(comp_right, right_n)) / track).atan();
    let side = Vec3::X;
    let mut edit = animator.edit();
    edit.translate_model(body, up_model * heave);
    edit.rotate_model(
        body,
        Quat::from_axis_angle(side, pitch_susp + rt.pitch)
            * Quat::from_axis_angle(Vec3::NEG_Z, roll_susp + rt.roll),
    );
    // Wheels hang from the body but follow the ground: undo the body motion at each wheel.
    for (bone, mount, compression, spin, steers) in wheel_bones {
        let rest_mount = skeleton.rest_model[mount].t;
        let now = edit.position(mount);
        let goal = rest_mount + up_model * compression;
        edit.translate_model(mount, goal - now);
        // Wheels stay level with the road rather than the body.
        let level = skeleton.rest_model[mount].r;
        let steer = if steers {
            Quat::from_axis_angle(up_model, -rt.steer)
        } else {
            Quat::IDENTITY
        };
        edit.set_model_rotation(mount, steer * level);
        if bone != mount {
            edit.local[bone].r = skeleton.rest[bone].r * Quat::from_rotation_x(-spin);
            edit.skeleton
                .update_model_subtree(edit.local, edit.model, bone);
        } else {
            edit.set_model_rotation(bone, steer * level * Quat::from_rotation_x(-spin));
        }
    }
}

/// Vehicle wheels as ground contacts.
pub(crate) fn wheel_contact(c: &WheelContact, entity: Entity) -> engine_world::GroundContact {
    engine_world::GroundContact {
        position: c.position,
        normal: c.normal,
        size: [c.width, 0.3],
        yaw: c.yaw,
        load_kg: c.load_kg,
        speed: c.speed,
        kind: ContactKind::Wheel,
        surface: Some(c.surface),
        entity: Some(entity),
    }
}
