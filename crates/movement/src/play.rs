//! Play mode: take control of a pilot and try the movement and combat by hand.
//!
//! [`start`] puts a level 10 pilot (the teal pilot-suit sword fighter) on the ground in
//! front of the active camera and a [`PilotCamera`] behind it; [`stop`] parks the pilot and
//! gives the view back.
//!
//! There is one pilot. Stopping parks it where it stands, the way a vehicle seats its driver:
//! hidden, its character capsule put away (out of physics, so nothing bumps into it and it
//! can't fall when the ground there streams out) and deaf to the controls. Starting again
//! brings the same pilot, health and kit and all, to the new spot (playtest 2026-10-29 #10).
//! The spot is the highest surface under a point in front of the camera from any height: a
//! roof, the ground or a lake's surface (not its bed). Where there is nothing under it (deep
//! space, ground not streamed in) the pilot drops from the air there. While the pilot sits in
//! a vehicle it can't be parked: getting out (E) comes first. `movement.play` does the same from the console or an agent, and where
//! [`PlayToggle`] is present (the demo adds it) the `pilot` action (P, gamepad d-pad down)
//! switches play mode on and off.
//!
//! Controls are the default action map: move, look (mouse or right stick), jump (double jump
//! in the air), sprint, crouch (slide while running), dash, attack (a dash-strike when the
//! enemy is far), block, and `lock_on` (middle mouse, right stick click). Escape frees the
//! mouse; a click captures it again.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::window::{CursorMode, Window};
use engine_input::{ActionState, ButtonInput, MouseButton};
use engine_render::{Camera, MeshRenderer, Projection};
use engine_world::interact::{Interactor, Using};
use engine_world::physics::{CharacterController, PhysicsQueries, QueryFilter};
use engine_world::{Gravity, Ground, SimulationFocus, Water};

use crate::camera::PilotCamera;
use crate::combat::Fighter;
use crate::demo;
use crate::fx::CameraFx;
use crate::lockon::LockOn;
use crate::look::fighter::Style;
use crate::mover::{Mover, flat};
use crate::player::PlayerControlled;
use crate::profile::MovementProfile;
use crate::tempo::{Perceiver, Tempo};

/// The pilot's name (for `movement.status`, `--follow` and the API).
pub const PILOT_NAME: &str = "Pilot";

/// Play mode right now.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct Play {
    pub pilot: Option<Entity>,
    pub camera: Option<Entity>,
    /// The mouse is captured for looking around.
    pub captured: bool,
    /// The pilot, put away while play mode is off (see the module docs).
    pub parked: Option<Entity>,
}

/// What a parked pilot gave up and gets back when play starts again.
#[derive(Component, Debug, Clone)]
pub struct Parked {
    controller: CharacterController,
    player: PlayerControlled,
    /// Its meshes that were drawn.
    hidden: Vec<Entity>,
}

/// Highest a pilot is put above the camera's point (a roof just over the view), m.
const LOOK_UP: f64 = 60.0;
/// Farthest below the camera's point that ground is looked for, m (a flight's altitude).
const LOOK_DOWN: f32 = 20_000.0;

/// Lets the `pilot` action start and stop play mode.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct PlayToggle;

/// Starts play mode: the pilot standing at `at` (feet, world meters), or on whatever is under
/// a point about 6 m in front of the active camera. Moves it there if play is already on.
pub fn start(world: &mut World, at: Option<DVec3>) -> Result<Entity, String> {
    if in_vehicle(world) {
        return Err("the pilot is in a vehicle: get out first (E)".into());
    }
    stop(world);
    let view = active_camera(world);
    let (from, forward) = view
        .as_ref()
        .map(|(_, t)| (t.translation(), t.forward()))
        .unwrap_or((DVec3::new(0.0, 2.0, 0.0), Vec3::NEG_Z));
    let up = world.resource::<Gravity>().up_at(from);
    let facing = flat(forward, up).try_normalize().unwrap_or(Vec3::NEG_Z);
    let feet = at.unwrap_or_else(|| {
        let ahead = from + (facing * 6.0).as_dvec3();
        // Nothing under it: drop from the air.
        landing(world, ahead, up).unwrap_or(ahead)
    });
    let center = feet + (up * 0.95).as_dvec3();

    let parked = world.resource::<Play>().parked;
    let pilot = match parked.filter(|&p| world.get_entity(p).is_ok()) {
        Some(p) => {
            unpark(world, p, center, facing);
            p
        }
        None => spawn_pilot(world, center, facing),
    };

    // The camera: like the one in use (exposure, order), drawn on top of it.
    let mut camera = view.map_or_else(|| Camera::perspective(65.0), |(c, _)| c);
    camera.order += 1;
    camera.active = true;
    if let Projection::Perspective { fov_y_deg, near } = &mut camera.projection {
        *fov_y_deg = 65.0;
        *near = near.min(0.05);
    }
    let behind = center - (facing * 4.0).as_dvec3() + (up * 1.5).as_dvec3();
    let cam = world
        .spawn((
            Name::new("Pilot camera"),
            camera,
            Transform::from_translation(behind),
            PilotCamera::following(pilot),
            CameraFx {
                target: Some(pilot),
                ..Default::default()
            },
            SimulationFocus { priority: 10 },
        ))
        .id();
    *world.resource_mut::<Play>() = Play {
        pilot: Some(pilot),
        camera: Some(cam),
        captured: true,
        parked: None,
    };
    set_cursor(world, CursorMode::Locked);
    Ok(pilot)
}

fn spawn_pilot(world: &mut World, center: DVec3, facing: Vec3) -> Entity {
    let mut queue = engine_core::ecs::world::CommandQueue::default();
    let pilot = {
        let mut commands = Commands::new(&mut queue, world);
        demo::spawn_character(
            &mut commands,
            PILOT_NAME,
            center,
            facing,
            Style::Pilot,
            MovementProfile::pilot(),
            (
                PlayerControlled::default(),
                Fighter::new(1, 100.0),
                // A trained fighter's reflexes (the world slows when it fights), but its own
                // clock at normal speed: at level 10's 1.45 everything it did ran 45% fast,
                // which played as running far too fast (playtest 2026-10-27 #25).
                Tempo {
                    speed: 1.0,
                    ..Tempo::level(10)
                },
                Perceiver,
                LockOn::default(),
                // Its body, not the camera, reaches what it can use (E).
                Interactor,
            ),
        )
    };
    queue.apply(world);
    pilot
}

/// Ends play mode, parking the pilot where it stands. Returns whether it was on. A pilot in
/// a vehicle stays in it, playing (see [`in_vehicle`]).
pub fn stop(world: &mut World) -> bool {
    if in_vehicle(world) {
        return false;
    }
    let play = *world.resource::<Play>();
    if let Some(cam) = play.camera
        && world.get_entity(cam).is_ok()
    {
        world.despawn(cam);
    }
    let parked = match play.pilot.filter(|&p| world.get_entity(p).is_ok()) {
        Some(p) => {
            park(world, p);
            Some(p)
        }
        None => play.parked,
    };
    *world.resource_mut::<Play>() = Play {
        parked,
        ..Play::default()
    };
    let was_on = play.pilot.is_some() || play.camera.is_some();
    if was_on {
        set_cursor(world, CursorMode::Normal);
    }
    was_on
}

/// The playing pilot sits in a vehicle (or mans something else that holds it): play mode
/// can't park it until it gets out.
pub fn in_vehicle(world: &World) -> bool {
    world
        .resource::<Play>()
        .pilot
        .is_some_and(|p| world.get::<Using>(p).is_some())
}

/// Puts the pilot away (see the module docs).
fn park(world: &mut World, pilot: Entity) {
    if world.get::<Parked>(pilot).is_some() {
        return;
    }
    let mut hidden = Vec::new();
    let mut stack = vec![pilot];
    while let Some(e) = stack.pop() {
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
        if let Some(mut m) = world.get_mut::<MeshRenderer>(e)
            && m.visible
        {
            m.visible = false;
            hidden.push(e);
        }
    }
    if let Some(mut l) = world.get_mut::<LockOn>(pilot) {
        l.target = None;
    }
    let mut e = world.entity_mut(pilot);
    let controller = e.take::<CharacterController>().unwrap_or_default();
    let player = e.take::<PlayerControlled>().unwrap_or_default();
    e.remove::<(Interactor, Perceiver)>();
    e.insert(Parked {
        controller,
        player,
        hidden,
    });
}

/// Brings the parked pilot back, standing with its center at `center` facing `facing`.
fn unpark(world: &mut World, pilot: Entity, center: DVec3, facing: Vec3) {
    let up = world.resource::<Gravity>().up_at(center);
    let rotation = engine_core::transform::look_rotation(facing, up).unwrap_or_default();
    let mut e = world.entity_mut(pilot);
    if let Some(mut t) = e.get_mut::<Transform>() {
        t.translation = center;
        t.rotation = rotation;
    }
    // No speed, fall or wall run carried over from where it was parked. (Removing the mover
    // would have it adopted again, with the human profile.)
    if let Some(mut m) = e.get_mut::<Mover>() {
        m.reset_motion();
        m.facing = facing;
        m.grounded = false;
        m.fall_speed = 0.0;
        m.air_secs = 0.0;
    }
    if let Some(parked) = e.take::<Parked>() {
        e.insert((parked.controller, parked.player, Interactor, Perceiver));
        for m in parked.hidden {
            if let Some(mut m) = world.get_mut::<MeshRenderer>(m) {
                m.visible = true;
            }
        }
    }
    if let Some(mut g) = world.get_mut::<GlobalTransform>(pilot) {
        *g = GlobalTransform::from(Transform::from_translation(center).with_rotation(rotation));
    }
}

fn set_cursor(world: &mut World, mode: CursorMode) {
    if let Some(mut w) = world.get_resource_mut::<Window>()
        && w.cursor_mode != mode
    {
        w.cursor_mode = mode;
    }
}

/// The active camera with the highest order, and where it is. A camera without a parent is
/// where its `Transform` says even before transforms propagate (moved this frame).
pub(crate) fn active_camera(world: &mut World) -> Option<(Camera, GlobalTransform)> {
    let mut q = world.query::<(&Camera, &GlobalTransform, &Transform, Has<ChildOf>)>();
    q.iter(world)
        .filter(|(c, ..)| c.active)
        .max_by_key(|(c, ..)| c.order)
        .map(|(c, g, t, child)| (*c, if child { *g } else { GlobalTransform::from(*t) }))
}

/// Where to stand under `p`: the highest of a collider, the ground and a water surface, from
/// [`LOOK_UP`] above it down to [`LOOK_DOWN`] below (any flight's altitude). Feet on a
/// lake's surface sink in and swim.
fn landing(world: &World, p: DVec3, up: Vec3) -> Option<DVec3> {
    let upd = up.as_dvec3();
    let top = p + upd * LOOK_UP;
    let hit = world
        .resource::<PhysicsQueries>()
        .0
        .raycast(
            top,
            -up,
            LOOK_DOWN + LOOK_UP as f32,
            &QueryFilter::default(),
        )
        .map(|h| h.point);
    let surface = world
        .get_resource::<Ground>()
        .and_then(|g| g.height(top))
        .filter(|_| up.y > 0.9)
        .map(|h| DVec3::new(p.x, h, p.z));
    let below = |q: &DVec3| (top - *q).dot(upd) >= 0.0;
    let ground = [hit, surface]
        .into_iter()
        .flatten()
        .filter(below)
        .max_by(|a, b| a.dot(upd).total_cmp(&b.dot(upd)))?;
    let water = world
        .get_resource::<Water>()
        .and_then(|w| w.sample(ground))
        .map(|w| DVec3::new(ground.x, w.surface_height, ground.z))
        .filter(|w| w.dot(upd) > ground.dot(upd) && below(w));
    Some(water.unwrap_or(ground))
}

/// The `pilot` action switches play mode where [`PlayToggle`] is present (`Update`).
pub(crate) fn toggle(world: &mut World) {
    if !world.contains_resource::<PlayToggle>() {
        return;
    }
    let pressed = world
        .get_resource::<ActionState>()
        .is_some_and(|a| a.just_pressed("pilot"));
    if !pressed {
        return;
    }
    if in_vehicle(world) {
        engine_core::log::info!("play mode: get out of the vehicle first (E)");
        return;
    }
    if !stop(world)
        && let Err(e) = start(world, None)
    {
        engine_core::log::warn!("play mode: {e}");
    }
}

/// Keep the mouse captured while playing: Escape frees it, a click captures it again
/// (`PostUpdate`, so it wins over other cursor handling that frame).
pub(crate) fn capture_mouse(
    mut play: ResMut<Play>,
    actions: Option<Res<ActionState>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    window: Option<ResMut<Window>>,
    cameras: Query<(), With<PilotCamera>>,
) {
    let Some(cam) = play.camera else {
        return;
    };
    if cameras.get(cam).is_err() {
        // Despawned by someone else: play mode is over.
        *play = Play::default();
        return;
    }
    if actions.as_ref().is_some_and(|a| a.just_pressed("pause")) {
        play.captured = false;
    } else if !play.captured
        && buttons.as_ref().is_some_and(|b| {
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
                .into_iter()
                .any(|m| b.just_pressed(m))
        })
    {
        play.captured = true;
    }
    if let Some(mut w) = window {
        let want = if play.captured {
            CursorMode::Locked
        } else {
            CursorMode::Normal
        };
        if w.cursor_mode != want {
            w.cursor_mode = want;
        }
    }
}
