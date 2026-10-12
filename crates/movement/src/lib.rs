#![doc = include_str!("../README.md")]

pub mod afterimage;
mod api;
pub mod combat;
pub mod demo;
mod fallback;
pub mod fx;
mod locomotion;
pub mod mover;
pub mod player;
pub mod profile;
pub mod tempo;

use engine_core::prelude::*;
use engine_render::RenderAppExt;

pub use afterimage::{Afterimage, Afterimages};
pub use combat::{
    AttackPhase, AttackState, CombatLog, CombatRecord, CombatState, Fighter, Outcome,
};
pub use fx::{CameraFx, SlowMotionFx};
pub use mover::{Dash, DashRequest, Mantle, Mover, WallContact};
pub use player::PlayerControlled;
pub use profile::MovementProfile;
pub use tempo::{
    CINEMATIC_KEY, Cinematic, FOCUS_KEY, Perceiver, PerceptionStatus, SlowMoPulse, Tempo,
    TimePerception,
};

/// Everything a game needs to use movement, in one import.
pub mod prelude {
    pub use crate::{
        Afterimages, CameraFx, CombatState, Fighter, MovementPlugin, MovementProfile, Mover,
        Perceiver, PlayerControlled, SlowMotionFx, Tempo, TimePerception,
    };
}

/// System sets of the movement module inside `FixedUpdate`, in order. Other modules can order
/// against them (an NPC brain writing `MoveIntent` should run before [`MovementSet::Intent`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementSet {
    /// Player input → `MoveIntent`, `Tempo` → `LocalTimeScale` (in `SimSet::Input`).
    Intent,
    /// Attacks start, characters move, strikes land, damage applies (in `SimSet::Gameplay`).
    Act,
    /// Reflex slow motion and cinematic beats react to what happened (in `SimSet::Gameplay`).
    Perceive,
}

/// Advanced locomotion (sprint, slide, wall-run, wall-jump, mantle, double jump, dash),
/// combat movement (dash-strikes, blocks, dodges, counters) and time-dilation gameplay
/// (reflex slow motion, cinematic beats, afterimages). See the crate docs.
pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        engine_world::locomotion::claim_backend(app, "movement");
        app.init_resource::<TimePerception>()
            .init_resource::<PerceptionStatus>()
            .init_resource::<CombatLog>()
            .init_resource::<SlowMotionFx>()
            .register_component::<MovementProfile>(
                "How a character moves: run/sprint speeds (m/s), jumps (m), slide, wall-run, \
                 mantle and dash tuning. Presets: pilot, human, elite (movement.preset).",
            )
            .register_component::<Mover>(
                "Movement state written every step: mode, velocity (m/s, own time), facing, \
                 grounded, wall being run on, dash/mantle in progress, dash charges.",
            )
            .register_component::<Fighter>(
                "Combat stats: team, health, damage, reach (m), swing arc (deg), windup/strike/\
                 recovery (s, own time), dash-strike range (m), block and counter multipliers.",
            )
            .register_component::<CombatState>(
                "Combat state: attack phase (DashIn, Windup, Strike, Recovery), stagger, \
                 blocking, defeated, hit/dodge/block counts.",
            )
            .register_component::<Tempo>(
                "How fast a character acts (speed → its LocalTimeScale) and perceives \
                 (reflexes: on the Perceiver, attacks slow the world to 1/reflexes).",
            )
            .register_component::<Perceiver>(
                "Marks the player whose reflexes drive world slow motion.",
            )
            .register_component::<Afterimages>(
                "Leave glowing afterimages while dashing, wall-running, sliding or striking: \
                 color, glow, alpha, interval and lifetime (real seconds).",
            )
            .register_component::<CameraFx>(
                "Camera cues for the followed character: FOV kick on sprint/dash, narrower in \
                 focus, roll while wall-running (degrees).",
            )
            .register_component::<PlayerControlled>(
                "Drive this character from input actions (move, jump, sprint, crouch, dash, \
                 attack, block), relative to the active camera.",
            )
            .register_resource::<TimePerception>(
                "Reflex slow motion (min scale, blend in, linger, threat radius) and cinematic \
                 slow-motion beats on hits, kills and enemy attacks.",
            )
            .register_resource::<PerceptionStatus>(
                "Reflex slow motion right now: focused, focus scale, threats, counts.",
            )
            .register_resource::<SlowMotionFx>(
                "Slow-motion screen effect: vignette, tint, grade, speed lines.",
            )
            .configure_sets(
                FixedUpdate,
                (
                    MovementSet::Intent.in_set(SimSet::Input),
                    (MovementSet::Act, MovementSet::Perceive)
                        .chain()
                        .in_set(SimSet::Gameplay),
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    (
                        locomotion::adopt_characters,
                        player::player_intent,
                        tempo::apply_tempo,
                    )
                        .in_set(MovementSet::Intent),
                    (
                        combat::begin_attacks,
                        locomotion::step_movers,
                        combat::resolve_attacks,
                        combat::apply_damage,
                    )
                        .chain()
                        .in_set(MovementSet::Act),
                    tempo::perceive.in_set(MovementSet::Perceive),
                    // Stand-in character physics while no physics module is installed.
                    fallback::publish
                        .in_set(SimSet::Input)
                        .run_if(fallback::no_physics),
                    fallback::step_characters
                        .in_set(SimSet::Physics)
                        .run_if(fallback::no_physics),
                ),
            )
            .add_systems(
                Update,
                (
                    fx::update_slow_motion_fx,
                    (afterimage::spawn_afterimages, afterimage::fade_afterimages).chain(),
                    fx::camera_fx.after(UpdateSet::Camera),
                ),
            )
            .add_render_feature(fx::SlowMotionOverlay::default());
        demo::register(app);
        api::register(app);
    }
}
