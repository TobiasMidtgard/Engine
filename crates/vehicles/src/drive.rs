//! Driving and flying by hand: walk up to a car, plane or spaceship, press E (the `interact`
//! action) to get in, drive or fly it with the keyboard and mouse, press E again to get out.
//!
//! Every vehicle carries an [`Interactable`] ("Drive", "Fly") whose reach covers the vehicle,
//! enabled while it is nearly at rest. Getting in turns its autopilot off, hides the actor
//! (the player's body, or the free camera when nobody walks) and keeps it where it got in,
//! with its [`MoveIntent`] cleared every step so on-foot controls do nothing, and puts a chase
//! camera above every other camera. The actor keeps [`Using`] the vehicle, so the next E gets
//! out wherever it is: beside the vehicle, on the ground, once it has (nearly) stopped.
//!
//! Controls ([`Driven`] holds the state):
//!
//! - **Car**: W throttle, S brake (held at a stop: reverse), A/D steer, Space handbrake. The
//!   mouse swings the camera round; it settles back behind the car when the mouse rests.
//! - **Plane**: mouse aim. The camera looks where the mouse points and the flight computer
//!   flies the nose there, banking into turns (the scheme of War Thunder's mouse aim and of
//!   Brian Hernandez's MouseFlight). W/S move the throttle lever, A/D add roll and steer on
//!   the ground, Space brakes, G raises or lowers the gear, hold C to look around without
//!   steering.
//! - **Spaceship**: the same mouse aim; in thin air or space the ship turns on its thrusters
//!   (yaw and pitch, no banking), A/D roll. W/S move the main engine's lever (S at zero
//!   brakes), Shift is full thrust, Space/Ctrl lift on the hover engines, H holds the ship up
//!   against gravity (hover assist), G gear.
//!
//! The wheel zooms the camera in and out. The camera follows the vehicle's interpolated pose
//! (the one that is drawn), so it doesn't judder between physics steps, and as a root entity
//! it moves with world shifts like everything else.

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_core::transform::{TransformInterpolation, look_rotation};
use engine_core::window::{CursorMode, Window};
use engine_input::{ActionState, ButtonInput, KeyCode, Mouse};
use engine_render::{Camera, MeshRenderer, Projection};
use engine_world::physics::{Collider, ColliderShape, PhysicsQueries, QueryFilter, Velocity};
use engine_world::{
    Gravity, Interact, Interactable, Interactor, MoveIntent, SimulationFocus, Using,
    VehicleControls,
};

use crate::aero::Aircraft;
use crate::autopilot::{Autopilot, AutopilotMode};
use crate::chassis::{Vehicle, VehicleTelemetry};
use crate::cruise::Cruise;
use crate::ship::{AssistMode, Spacecraft};

/// Fastest a vehicle may be going to get in or out, m/s.
pub const BOARD_SPEED: f32 = 2.5;
/// Mouse sensitivity, radians per pixel.
const LOOK_RAD_PER_PX: f32 = 0.0022;
/// Seconds without mouse motion before a car's camera swings back behind it.
const RECENTER_AFTER_S: f32 = 1.2;

/// How a vehicle is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Style {
    #[default]
    Car,
    Plane,
    Ship,
}

/// What can be driven, and how big it is (for the camera, the prompt's reach and where you
/// step out). Added to every [`Vehicle`] with its [`Interactable`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Boardable {
    pub style: Style,
    /// The collider's box in the vehicle's own frame, meters.
    pub min: Vec3,
    pub max: Vec3,
}

impl Boardable {
    /// Half the longest side, meters.
    pub fn radius(&self) -> f32 {
        ((self.max - self.min) * 0.5).max_element()
    }
}

/// On a vehicle someone is driving.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Driven {
    /// Who drives (the player's body, or a camera).
    pub actor: Entity,
    /// The chase camera.
    pub camera: Entity,
    pub style: Style,
    /// The throttle lever of a plane or ship, 0..1.
    pub lever: f32,
    /// Steering as the wheel has turned so far, −1..1 (a car's steering eases in and out).
    pub steer: f32,
    /// A car camera's swing off straight behind (yaw, pitch), radians.
    pub orbit: Vec2,
    /// Seconds since the mouse last moved.
    pub still_s: f32,
    /// Camera distance scale (the wheel).
    pub zoom: f32,
    /// Meshes of the actor hidden while it sits in the vehicle.
    pub hidden: Vec<Entity>,
    /// The cursor before getting in.
    pub cursor: CursorMode,
}

/// On the actor sitting in `vehicle`, with what to put back if the vehicle goes away.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Seated {
    pub vehicle: Entity,
    pub camera: Entity,
    pub hidden: Vec<Entity>,
    pub cursor: CursorMode,
}

/// The camera that follows a driven vehicle.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ChaseCamera {
    pub vehicle: Entity,
}

pub(crate) fn register(app: &mut App) {
    app.add_systems(
        Update,
        (make_boardable, board, prompts, drive_controls, readouts)
            .chain()
            .in_set(UpdateSet::Logic),
    )
    .add_systems(Update, chase_camera.in_set(UpdateSet::Camera))
    .add_systems(
        FixedUpdate,
        hold_seated.after(SimSet::Input).before(SimSet::Gameplay),
    )
    .register_component::<Boardable>(
        "A vehicle you can get into (E): style (Car, Plane, Ship) and its local box (m).",
    );
}

/// Local box of a collider shape, meters.
fn shape_bounds(shape: &ColliderShape, at: Vec3, rot: Quat, min: &mut Vec3, max: &mut Vec3) {
    let mut corners = |half: Vec3| {
        for i in 0..8 {
            let s = Vec3::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { -1.0 } else { 1.0 },
            );
            let p = at + rot * (half * s);
            *min = min.min(p);
            *max = max.max(p);
        }
    };
    match shape {
        ColliderShape::Sphere { radius } => corners(Vec3::splat(*radius)),
        ColliderShape::Cuboid { half_extents } => corners(*half_extents),
        ColliderShape::Capsule {
            radius,
            half_height,
        }
        | ColliderShape::Cylinder {
            radius,
            half_height,
        } => corners(Vec3::new(*radius, half_height + radius, *radius)),
        ColliderShape::ConvexHull { points } => {
            for p in points {
                let p = at + rot * *p;
                *min = min.min(p);
                *max = max.max(p);
            }
        }
        ColliderShape::Compound(parts) => {
            for (offset, r, s) in parts {
                shape_bounds(s, at + rot * *offset, rot * *r, min, max);
            }
        }
        ColliderShape::TriMesh { .. } | ColliderShape::Heightfield { .. } => {}
    }
}

type NewVehicle<'a> = (
    Entity,
    &'a Vehicle,
    Option<&'a Collider>,
    Has<Aircraft>,
    Has<Spacecraft>,
);

/// Every vehicle gets a prompt and a [`Boardable`] once.
fn make_boardable(mut commands: Commands, q: Query<NewVehicle, Without<Boardable>>) {
    for (e, _, collider, plane, ship) in &q {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        if let Some(c) = collider {
            shape_bounds(&c.shape, Vec3::ZERO, Quat::IDENTITY, &mut min, &mut max);
        }
        if min.x > max.x {
            // No collider: a car-sized box.
            (min, max) = (Vec3::new(-0.9, -0.4, -2.2), Vec3::new(0.9, 0.9, 2.2));
        }
        let style = if ship {
            Style::Ship
        } else if plane {
            Style::Plane
        } else {
            Style::Car
        };
        let b = Boardable { style, min, max };
        // Used from anywhere beside it: the use point under its middle, near the bottom of
        // its body, reached from up to a meter and a half past its longest half.
        let center = Vec3::new((min.x + max.x) * 0.5, min.y + 0.3, (min.z + max.z) * 0.5);
        let verb = match style {
            Style::Car => "Drive",
            Style::Plane | Style::Ship => "Fly",
        };
        commands.entity(e).insert((
            b,
            Interactable::new(verb).reach(b.radius() + 1.5).at(center),
        ));
    }
}

/// Gets in and out on E.
fn board(world: &mut World, mut cursor: Local<bevy_ecs::message::MessageCursor<Interact>>) {
    let presses: Vec<Interact> = {
        let messages = world.resource::<bevy_ecs::message::Messages<Interact>>();
        cursor.read(messages).copied().collect()
    };
    for press in presses {
        let Some(b) = world.get::<Boardable>(press.target).copied() else {
            continue;
        };
        if let Some(driven) = world.get::<Driven>(press.target) {
            // The driver, or (driving from a free camera) the chase camera that stands in.
            if driven.actor == press.actor || driven.camera == press.actor {
                get_out(world, press.target, false, false);
            }
            continue;
        }
        if world.get::<Seated>(press.actor).is_some() || speed(world, press.target) > BOARD_SPEED {
            continue;
        }
        get_in(world, press.actor, press.target, b);
    }
    // Whoever drove may be gone (play mode ended), or the vehicle (despawned): end the drive.
    let mut ended = Vec::new();
    let mut q = world.query::<(Entity, &Driven)>();
    for (e, d) in q.iter(world) {
        if world.get_entity(d.actor).is_err() || world.get_entity(d.camera).is_err() {
            ended.push(e);
        }
    }
    for e in ended {
        get_out(world, e, true, false);
    }
    // The vehicle went away under its driver: show the body again, drop the camera.
    let mut orphans = Vec::new();
    let mut q = world.query::<(Entity, &Seated)>();
    for (e, s) in q.iter(world) {
        if world.get::<Driven>(s.vehicle).is_none() {
            orphans.push((e, s.clone()));
        }
    }
    for (e, s) in orphans {
        restore(world, &s.hidden, s.camera, s.cursor);
        world.entity_mut(e).remove::<(Seated, Using)>();
    }
}

fn speed(world: &World, e: Entity) -> f32 {
    if let Some(c) = world.get::<Cruise>(e) {
        return c.speed as f32;
    }
    world.get::<Velocity>(e).map_or(0.0, |v| v.linear.length())
}

pub(crate) fn get_in(world: &mut World, actor: Entity, vehicle: Entity, b: Boardable) {
    if let Some(mut ap) = world.get_mut::<Autopilot>(vehicle) {
        ap.mode = AutopilotMode::Off;
        ap.phase.clear();
    }
    if let Some(mut c) = world.get_mut::<VehicleControls>(vehicle) {
        *c = VehicleControls::default();
    }
    // Hide the actor's body (its meshes and theirs below).
    let mut hidden = Vec::new();
    let mut stack = vec![actor];
    while let Some(e) = stack.pop() {
        if let Some(mut m) = world.get_mut::<MeshRenderer>(e)
            && m.visible
        {
            m.visible = false;
            hidden.push(e);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
    // The chase camera: like the one in use, drawn over it, starting behind the vehicle and
    // looking where it points (a plane flies where the camera looks).
    let mut camera =
        engine_render::frame::active_camera(world).map_or_else(Camera::default, |(_, c, _)| c);
    let pose = world
        .get::<GlobalTransform>(vehicle)
        .map_or_else(Transform::default, |g| g.to_transform());
    let up = world.resource::<Gravity>().up_at(pose.translation);
    let fwd = pose.forward();
    let flat = (fwd - up * fwd.dot(up)).normalize_or(fwd);
    let from = Transform::from_translation(
        pose.translation - (flat * (2.3 * b.radius() + 2.0)).as_dvec3() + (up * 2.0).as_dvec3(),
    )
    .with_rotation(look_rotation(flat, up).unwrap_or(pose.rotation));
    camera.order += 1;
    camera.active = true;
    if let Projection::Perspective { fov_y_deg, near } = &mut camera.projection {
        *fov_y_deg = 65.0;
        *near = near.min(0.1);
    }
    let cam = world
        .spawn((
            Name::new("Vehicle camera"),
            camera,
            from,
            ChaseCamera { vehicle },
            SimulationFocus { priority: 20 },
        ))
        .id();
    let cursor = world
        .get_resource::<Window>()
        .map_or(CursorMode::Normal, |w| w.cursor_mode);
    if let Some(mut w) = world.get_resource_mut::<Window>() {
        w.cursor_mode = CursorMode::Locked;
    }
    let lever = world
        .get::<VehicleTelemetry>(vehicle)
        .map_or(0.0, |t| t.throttle);
    world.entity_mut(actor).insert((
        Seated {
            vehicle,
            camera: cam,
            hidden: hidden.clone(),
            cursor,
        },
        Using { target: vehicle },
    ));
    world.entity_mut(vehicle).insert(Driven {
        actor,
        camera: cam,
        style: b.style,
        lever,
        steer: 0.0,
        orbit: Vec2::ZERO,
        still_s: 0.0,
        zoom: 1.0,
        hidden,
        cursor,
    });
    // A free camera as the driver: the chase camera is now the view that picks what E
    // uses, so it stands in, busy with the vehicle.
    if world.get::<Interactor>(actor).is_none() {
        world
            .entity_mut(cam)
            .insert((Interactor, Using { target: vehicle }));
    }
    if let Some(mut i) = world.get_mut::<Interactable>(vehicle) {
        i.verb = "Get out".into();
    }
}

/// Where to step out: beside the vehicle's left side, on whatever is below.
fn exit_point(world: &World, vehicle: Entity, b: &Boardable) -> Option<DVec3> {
    let t = world.get::<GlobalTransform>(vehicle)?.to_transform();
    let up = world.resource::<Gravity>().up_at(t.translation);
    let side = Vec3::new(b.min.x - 1.0, 0.0, (b.min.z + b.max.z) * 0.5);
    let beside = t.transform_point(side.as_dvec3());
    let top = beside + (up * (b.max.y + 3.0)).as_dvec3();
    let reach = b.max.y - b.min.y + 60.0;
    let floor = world.get_resource::<PhysicsQueries>().and_then(|q| {
        q.0.raycast(top, -up, reach, &QueryFilter::excluding(vehicle))
            .map(|h| h.point)
    });
    Some(floor.unwrap_or(beside - (up * 0.5).as_dvec3()))
}

/// Gets out of `vehicle`: beside it, once it has (nearly) stopped, or at once with `force`.
/// With `stay` the actor stays where it is (a tour or script moved it on already).
pub(crate) fn get_out(world: &mut World, vehicle: Entity, force: bool, stay: bool) {
    let Some(driven) = world.get::<Driven>(vehicle).cloned() else {
        return;
    };
    let b = world.get::<Boardable>(vehicle).copied();
    // Cruising: only a forced exit (a script, the tour), and it drops back into physics first.
    if world.get::<Cruise>(vehicle).is_some() {
        if !force {
            return;
        }
        crate::cruise::drop_out(world, vehicle);
    }
    if world.get_entity(driven.actor).is_ok() {
        // Moving: stay in.
        if !force && speed(world, vehicle) > BOARD_SPEED {
            return;
        }
        if !stay && let Some(feet) = b.and_then(|b| exit_point(world, vehicle, &b)) {
            let up = world.resource::<Gravity>().up_at(feet);
            // A body stands with its center about a meter up; a camera at eye height.
            let lift = if world.get::<Camera>(driven.actor).is_some() {
                1.7
            } else {
                0.95
            };
            let at = feet + (up * lift).as_dvec3();
            if let Some(mut t) = world.get_mut::<Transform>(driven.actor) {
                t.translation = at;
            }
            if let Some(mut i) = world.get_mut::<TransformInterpolation>(driven.actor) {
                i.reset();
            }
        }
        world.entity_mut(driven.actor).remove::<(Seated, Using)>();
    }
    restore(world, &driven.hidden, driven.camera, driven.cursor);
    if world.get_entity(vehicle).is_err() {
        return;
    }
    if let Some(mut c) = world.get_mut::<VehicleControls>(vehicle) {
        // Left parked: the handbrake on, a plane's wheel brakes held.
        *c = VehicleControls {
            handbrake: driven.style == Style::Car,
            brake: if driven.style == Style::Plane {
                1.0
            } else {
                0.0
            },
            ..Default::default()
        };
    }
    world.entity_mut(vehicle).remove::<Driven>();
    if let Some(mut i) = world.get_mut::<Interactable>(vehicle) {
        i.verb = match driven.style {
            Style::Car => "Drive",
            Style::Plane | Style::Ship => "Fly",
        }
        .into();
    }
}

/// Shows the hidden body again, drops the chase camera and gives the cursor back.
fn restore(world: &mut World, hidden: &[Entity], camera: Entity, cursor: CursorMode) {
    for e in hidden {
        if let Some(mut m) = world.get_mut::<MeshRenderer>(*e) {
            m.visible = true;
        }
    }
    if world.get_entity(camera).is_ok() {
        world.despawn(camera);
    }
    if let Some(mut w) = world.get_resource_mut::<Window>() {
        w.cursor_mode = cursor;
    }
}

/// Prompts: a moving vehicle can't be boarded, and one being driven says how to get out.
type Prompt<'a> = (
    &'a mut Interactable,
    &'a Boardable,
    Option<&'a Velocity>,
    Has<Driven>,
    Has<Cruise>,
);

fn prompts(mut q: Query<Prompt>) {
    for (mut i, b, v, driven, cruising) in &mut q {
        let moving = cruising || v.is_some_and(|v| v.linear.length() > BOARD_SPEED);
        let (verb, enabled) = match (driven, moving) {
            (true, false) => ("Get out", true),
            (true, true) => ("Stop to get out", true),
            (false, _) => (
                match b.style {
                    Style::Car => "Drive",
                    Style::Plane | Style::Ship => "Fly",
                },
                !moving,
            ),
        };
        if i.verb != verb {
            i.verb = verb.into();
        }
        if i.enabled != enabled {
            i.enabled = enabled;
        }
    }
}

/// The seated actor's legs do nothing (after input wrote its intent, before movement reads
/// it).
fn hold_seated(mut q: Query<&mut MoveIntent, With<Seated>>) {
    for mut intent in &mut q {
        if *intent != MoveIntent::default() {
            *intent = MoveIntent::default();
        }
    }
}

/// Stick commands that fly the nose toward `aim` (world direction): pitch and yaw toward it,
/// and roll that banks into a turn when the aim is well off the nose and levels the wings
/// when it is close (`bank` false: no banking, roll left to the keys). Returns
/// (pitch, yaw, roll), each −1..1, in the `VehicleControls` sense (+ = nose up, nose right,
/// roll right).
pub fn mouse_aim(rotation: Quat, aim: Vec3, up: Vec3, bank: bool, gain: f32) -> (f32, f32, f32) {
    let local = rotation.inverse() * aim.normalize_or(rotation * Vec3::NEG_Z);
    let ahead = -local.z;
    let (mut x, y) = (local.x * gain, local.y * gain);
    // Behind the nose: turn hard toward the side it is on.
    if ahead < 0.0 {
        x = if local.x >= 0.0 { 1.0 } else { -1.0 } * gain.max(1.0);
    }
    let pitch = y.clamp(-1.0, 1.0);
    let yaw = x.clamp(-1.0, 1.0);
    if !bank {
        return (pitch, yaw, 0.0);
    }
    let off = ahead.clamp(-1.0, 1.0).acos();
    let into_turn = x.clamp(-1.0, 1.0);
    let right = rotation * Vec3::X;
    let level = (right.dot(up) * 2.0).clamp(-1.0, 1.0);
    let k = (off / 10f32.to_radians()).clamp(0.0, 1.0);
    (pitch, yaw, level + (into_turn - level) * k)
}

/// Approaches `target` at `rate` per second.
fn toward(value: f32, target: f32, rate: f32, dt: f32) -> f32 {
    value + (target - value).clamp(-rate * dt, rate * dt)
}

type DrivenParts<'a> = (
    Entity,
    &'a mut Driven,
    &'a mut VehicleControls,
    &'a Transform,
    &'a VehicleTelemetry,
    Option<&'a mut Aircraft>,
    Option<&'a mut Spacecraft>,
);

/// Keys and mouse to [`VehicleControls`].
#[allow(clippy::too_many_arguments)]
fn drive_controls(
    time: Res<Time>,
    gravity: Res<Gravity>,
    actions: Option<Res<ActionState>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    cameras: Query<&Transform, (With<ChaseCamera>, Without<Driven>)>,
    mut q: Query<DrivenParts>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let Some(actions) = actions else { return };
    let held = |k: KeyCode| keys.as_ref().is_some_and(|keys| keys.pressed(k));
    let tapped = |k: KeyCode| keys.as_ref().is_some_and(|keys| keys.just_pressed(k));
    let mv = actions.axis("move");
    for (e, mut d, mut c, t, tel, aircraft, ship) in &mut q {
        let up = gravity.up_at(t.translation);
        let aim = cameras
            .get(d.camera)
            .map_or(t.forward(), |cam| cam.forward());
        let looking = held(KeyCode::C);
        match d.style {
            Style::Car => {
                // Steering eases in, and the faster the car the less lock a key asks for.
                let lock = 1.0 / (1.0 + tel.speed.max(0.0) / 30.0);
                let rate = if mv.x.abs() > 0.05 { 3.0 } else { 5.0 };
                d.steer = toward(d.steer, mv.x * lock, rate, dt);
                *c = VehicleControls {
                    throttle: mv.y.max(0.0),
                    brake: (-mv.y).max(0.0),
                    steer: d.steer,
                    handbrake: actions.pressed("jump"),
                    boost: actions.pressed("sprint"),
                    ..Default::default()
                };
            }
            Style::Plane => {
                d.lever = (d.lever + mv.y * 0.6 * dt).clamp(0.0, 1.0);
                let (pitch, yaw, roll) = if looking {
                    let (_, _, level) = mouse_aim(t.rotation, t.forward(), up, true, 2.5);
                    (0.0, 0.0, level)
                } else {
                    mouse_aim(t.rotation, aim, up, true, 2.5)
                };
                let grounded = tel.wheels_grounded > 0;
                c.throttle = if actions.pressed("sprint") {
                    1.0
                } else {
                    d.lever
                };
                c.pitch = pitch;
                c.steer = (roll + mv.x).clamp(-1.0, 1.0);
                // The rudder (and nose wheel) from the aim, or the keys on the ground.
                c.yaw = if grounded {
                    (mv.x + yaw * 0.5).clamp(-1.0, 1.0)
                } else {
                    yaw * 0.3
                };
                c.brake = if actions.pressed("jump") { 1.0 } else { 0.0 };
                c.handbrake = false;
                if let Some(mut a) = aircraft {
                    if tapped(KeyCode::G) {
                        a.gear_down = !a.gear_down;
                    }
                    a.flaps = if a.gear_down { 0.4 } else { 0.0 };
                    // Wind the steady elevator into the trim, as a pilot does.
                    if !grounded && !tel.stalled {
                        a.pitch_trim = (a.pitch_trim + c.pitch * dt * 0.4).clamp(-0.6, 0.6);
                    }
                }
            }
            Style::Ship => {
                d.lever = (d.lever + mv.y * 0.5 * dt).clamp(0.0, 1.0);
                // Banking turns where the wings bite; thrusters turn it in thin air.
                let winged = tel.dynamic_pressure > 3_000.0;
                let lift = actions.value("ascend").clamp(-1.0, 1.0);
                // On its gear with nothing asked of the engines, it rests (no puffing RCS).
                let resting = tel.wheels_grounded > 0 && d.lever <= 0.0 && lift <= 0.0;
                let (pitch, yaw, roll) = if looking || resting {
                    (0.0, 0.0, 0.0)
                } else {
                    mouse_aim(t.rotation, aim, up, winged, 2.0)
                };
                let s_held = mv.y < -0.5;
                c.throttle = if actions.pressed("sprint") {
                    1.0
                } else {
                    d.lever
                };
                c.brake = if d.lever <= 0.0 && s_held { 1.0 } else { 0.0 };
                c.boost = actions.pressed("sprint");
                c.pitch = pitch;
                c.yaw = if winged { yaw * 0.3 } else { yaw };
                c.steer = (roll + mv.x).clamp(-1.0, 1.0);
                c.lift = lift;
                c.strafe = 0.0;
                c.handbrake = false;
                if let Some(mut a) = aircraft
                    && tapped(KeyCode::G)
                {
                    a.gear_down = !a.gear_down;
                }
                // J: into cruise (out of the air) or back out of it.
                if tapped(KeyCode::J) {
                    commands.queue(move |world: &mut World| {
                        if world.get::<Cruise>(e).is_some() {
                            crate::cruise::drop_out(world, e);
                        } else if let Err(why) = crate::cruise::engage(world, e) {
                            engine_core::log::info!("cruise: {why}");
                        }
                    });
                }
                if let Some(mut s) = ship
                    && tapped(KeyCode::H)
                {
                    s.assist.mode = match s.assist.mode {
                        AssistMode::Hover => AssistMode::Full,
                        _ => AssistMode::Hover,
                    };
                }
            }
        }
    }
}

type ChaseView<'a> = (&'a mut Transform, &'a ChaseCamera);
type ChaseTarget<'a> = (
    &'a Transform,
    Option<&'a TransformInterpolation>,
    &'a mut Driven,
    &'a Boardable,
    Option<&'a VehicleTelemetry>,
);

type Gauges<'a> = (
    Entity,
    &'a Driven,
    &'a Transform,
    &'a VehicleTelemetry,
    Option<&'a Aircraft>,
    Option<&'a Spacecraft>,
    Option<&'a Cruise>,
);

/// What the driver reads under the prompt: speed, height, throttle, gear, cruise.
fn readouts(
    gravity: Res<Gravity>,
    mut out: Option<ResMut<engine_world::Readouts>>,
    q: Query<Gauges>,
) {
    use crate::cruise::{ENGAGE_ALTITUDE_M, distance_text, speed_text};
    let Some(out) = out.as_mut() else { return };
    for (_, d, t, tel, aircraft, ship, cruise) in &q {
        match cruise {
            Some(c) => {
                out.set("Speed", speed_text(c.speed));
                out.set("Altitude", distance_text(c.altitude_m));
                out.set(
                    "Cruise",
                    match c.target {
                        Some(_) => "homing on the body ahead (J drops out)".to_string(),
                        None => "aim at a planet or star to home on it (J drops out)".to_string(),
                    },
                );
                out.set("Lever", format!("{:.0}%  W/S", d.lever * 100.0));
                continue;
            }
            None => out.set("Speed", speed_text(tel.speed as f64)),
        }
        match d.style {
            Style::Car => {
                let gear = match tel.gear {
                    g if g < 0 => "R".to_string(),
                    0 => "N".to_string(),
                    g => g.to_string(),
                };
                out.set("Gear", gear);
                out.set("Engine", format!("{:.0} rpm", tel.rpm));
            }
            Style::Plane | Style::Ship => {
                let h = crate::cruise::altitude(&gravity, t.translation).0;
                out.set("Altitude", distance_text(h.max(0.0)));
                out.set(
                    "Throttle",
                    format!("{:.0}%  W/S, Shift full", d.lever * 100.0),
                );
                if let Some(a) = aircraft {
                    out.set("Gear", if a.gear_down { "down  G" } else { "up  G" });
                }
                if d.style == Style::Plane && tel.stalled {
                    out.set("Stall", "nose down");
                }
                if let Some(s) = ship {
                    let assist = match s.assist.mode {
                        AssistMode::Hover => "hover  H",
                        AssistMode::Full => "full  H for hover",
                        AssistMode::Rotation => "rotation",
                        AssistMode::Off => "off",
                    };
                    out.set("Assist", assist);
                    out.set(
                        "Cruise",
                        if h >= ENGAGE_ALTITUDE_M {
                            "J".to_string()
                        } else {
                            format!("over {:.0} km (J)", ENGAGE_ALTITUDE_M / 1000.0)
                        },
                    );
                }
            }
        }
    }
}

/// Puts each chase camera behind its vehicle.
#[allow(clippy::too_many_arguments)]
fn chase_camera(
    time: Res<Time>,
    fixed: Res<FixedTime>,
    gravity: Res<Gravity>,
    mouse: Option<Res<Mouse>>,
    window: Option<Res<Window>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    queries: Option<Res<PhysicsQueries>>,
    mut cams: Query<ChaseView, Without<Driven>>,
    mut vehicles: Query<(Entity, ChaseTarget)>,
) {
    let dt = time.delta_secs().max(1e-4);
    let locked = window.is_none_or(|w| w.cursor_mode == CursorMode::Locked);
    let (delta, scroll) = mouse
        .as_ref()
        .filter(|_| locked)
        .map_or((Vec2::ZERO, 0.0), |m| (m.delta, m.scroll.y));
    let looked_back = keys.as_ref().is_some_and(|k| k.just_released(KeyCode::C));
    for (vehicle, (t, interp, mut d, b, tel)) in &mut vehicles {
        let Ok((mut cam, chase)) = cams.get_mut(d.camera) else {
            continue;
        };
        if chase.vehicle != vehicle {
            continue;
        }
        // The pose that is drawn this frame.
        let pose = interp
            .and_then(|i| i.previous)
            .map_or(*t, |prev| prev.lerp(t, fixed.alpha()));
        let up = gravity.up_at(pose.translation);
        if scroll != 0.0 {
            d.zoom = (d.zoom * 0.88f32.powf(scroll)).clamp(0.4, 4.0);
        }
        let r = b.radius();
        match d.style {
            Style::Car => {
                if delta != Vec2::ZERO {
                    d.still_s = 0.0;
                    d.orbit.x -= delta.x * LOOK_RAD_PER_PX;
                    d.orbit.y = (d.orbit.y - delta.y * LOOK_RAD_PER_PX).clamp(-0.5, 1.2);
                } else {
                    d.still_s += dt;
                    if d.still_s > RECENTER_AFTER_S {
                        let k = 1.0 - (-dt * 2.5).exp();
                        d.orbit *= 1.0 - k;
                    }
                }
                let target = pose.translation + (up * (b.max.y + 0.6)).as_dvec3();
                let fwd = pose.forward();
                let flat = (fwd - up * fwd.dot(up)).normalize_or(cam.forward());
                // Behind the car, swung by the mouse, a little above.
                let back = Quat::from_axis_angle(up, d.orbit.x) * -flat;
                let side = back.cross(up).normalize_or(Vec3::X);
                let wanted = Quat::from_axis_angle(side, 0.22 + d.orbit.y) * back;
                // Trails the car's turns a little.
                let now = (cam.translation - target).as_vec3().normalize_or(wanted);
                let k = 1.0 - (-dt * 7.0).exp();
                let dir = now.lerp(wanted, k).normalize_or(wanted);
                let dist = (2.3 * r + 2.0) * d.zoom;
                let dist = clear_distance(queries.as_deref(), target, dir, dist, vehicle);
                cam.translation = target + (dir * dist).as_dvec3();
                cam.look_at(target, up);
            }
            Style::Plane | Style::Ship => {
                // The camera is the aim: the mouse turns it, the flight computer follows.
                let thin = tel.is_some_and(|t| t.dynamic_pressure < 3_000.0);
                let free = d.style == Style::Ship && thin;
                let mut rot = cam.rotation;
                if looked_back {
                    rot = pose.rotation;
                }
                if delta != Vec2::ZERO {
                    let yaw_axis = if free { rot * Vec3::Y } else { up };
                    let yaw = Quat::from_axis_angle(yaw_axis, -delta.x * LOOK_RAD_PER_PX);
                    let pitch = Quat::from_axis_angle(rot * Vec3::X, -delta.y * LOOK_RAD_PER_PX);
                    rot = (yaw * pitch * rot).normalize();
                }
                let mut fwd = rot * Vec3::NEG_Z;
                if !free {
                    // Keep the horizon level, short of straight up or down.
                    let s = fwd.dot(up).clamp(-0.97, 0.97);
                    let flat = (fwd - up * fwd.dot(up)).normalize_or(pose.forward());
                    fwd = (flat * (1.0 - s * s).sqrt() + up * s).normalize();
                    rot = look_rotation(fwd, up).unwrap_or(rot);
                } else {
                    // In space the camera's up eases toward the ship's.
                    let ship_up = pose.rotation * Vec3::Y;
                    let cur_up = rot * Vec3::Y;
                    let k = 1.0 - (-dt * 1.5).exp();
                    let new_up = cur_up.lerp(ship_up, k).normalize_or(cur_up);
                    rot = look_rotation(fwd, new_up).unwrap_or(rot);
                }
                let cam_up = rot * Vec3::Y;
                let dist = (2.2 * r + 4.0) * d.zoom;
                let target = pose.translation + (cam_up * (0.25 * r)).as_dvec3();
                let back = -fwd;
                let dist = clear_distance(queries.as_deref(), target, back, dist, vehicle);
                cam.translation = target + (back * dist).as_dvec3();
                cam.rotation = rot;
            }
        }
    }
}

/// How far the camera can sit from `target` along `dir` without going into the ground or a
/// wall.
fn clear_distance(
    queries: Option<&PhysicsQueries>,
    target: DVec3,
    dir: Vec3,
    dist: f32,
    vehicle: Entity,
) -> f32 {
    let Some(q) = queries else { return dist };
    match q
        .0
        .raycast(target, dir, dist, &QueryFilter::excluding(vehicle))
    {
        Some(hit) => ((hit.point - target).length() as f32 - 0.3).max(0.5),
        None => dist,
    }
}
