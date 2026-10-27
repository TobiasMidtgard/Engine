//! Play mode: take control of a pilot and try the movement and combat by hand.
//!
//! [`start`] puts a level 10 pilot (the teal pilot-suit sword fighter) on the ground in
//! front of the active camera and a [`PilotCamera`] behind it; [`stop`] removes both and gives
//! the view back. `movement.play` does the same from the console or an agent, and where
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
use engine_render::{Camera, Projection};
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{Gravity, Ground, SimulationFocus};

use crate::camera::PilotCamera;
use crate::combat::Fighter;
use crate::demo;
use crate::fx::CameraFx;
use crate::lockon::LockOn;
use crate::look::fighter::Style;
use crate::mover::flat;
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
}

/// Lets the `pilot` action start and stop play mode.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct PlayToggle;

/// Starts play mode: a pilot standing at `at` (feet, world meters), or on the ground about
/// 6 m in front of the active camera. Restarts it if it is already on.
pub fn start(world: &mut World, at: Option<DVec3>) -> Result<Entity, String> {
    stop(world);
    let view = active_camera(world);
    let (from, forward) = view
        .as_ref()
        .map(|(_, t)| (t.translation(), t.forward()))
        .unwrap_or((DVec3::new(0.0, 2.0, 0.0), Vec3::NEG_Z));
    let up = world.resource::<Gravity>().up_at(from);
    let facing = flat(forward, up).try_normalize().unwrap_or(Vec3::NEG_Z);
    let feet = match at {
        Some(p) => p,
        None => ground_below(world, from + (facing * 6.0).as_dvec3(), up)
            .ok_or("no ground in front of the camera to stand on")?,
    };
    let center = feet + (up * 0.95).as_dvec3();

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
            ),
        )
    };
    queue.apply(world);

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
    };
    set_cursor(world, CursorMode::Locked);
    Ok(pilot)
}

/// Ends play mode. Returns whether it was on.
pub fn stop(world: &mut World) -> bool {
    let play = *world.resource::<Play>();
    for e in [play.pilot, play.camera].into_iter().flatten() {
        if world.get_entity(e).is_ok() {
            world.despawn(e);
        }
    }
    *world.resource_mut::<Play>() = Play::default();
    let was_on = play.pilot.is_some() || play.camera.is_some();
    if was_on {
        set_cursor(world, CursorMode::Normal);
    }
    was_on
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

/// The highest surface under `p`: a collider or the ground.
fn ground_below(world: &World, p: DVec3, up: Vec3) -> Option<DVec3> {
    let top = p + (up * 60.0).as_dvec3();
    let hit = world
        .resource::<PhysicsQueries>()
        .0
        .raycast(top, -up, 400.0, &QueryFilter::default())
        .map(|h| h.point);
    let surface = world
        .resource::<Ground>()
        .height(top)
        .filter(|_| up.y > 0.9)
        .map(|h| DVec3::new(p.x, h, p.z));
    match (hit, surface) {
        (Some(a), Some(b)) => Some(if a.dot(up.as_dvec3()) >= b.dot(up.as_dvec3()) {
            a
        } else {
            b
        }),
        (a, b) => a.or(b),
    }
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
