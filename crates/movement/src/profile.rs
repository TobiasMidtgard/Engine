//! Tuning: how a character moves. One [`MovementProfile`] per character; every field is
//! JSON-patchable (`entity.set`), so designers and AI agents can tune movement live.

use engine_core::prelude::*;

/// How a character moves: speeds, jumps, slides, wall-runs, mantles, ledge hangs, dashes and
/// the grapple hook.
///
/// All speeds are in meters per second of the character's **own** time (see
/// [`Tempo`](crate::tempo::Tempo)): a character with `Tempo::speed = 2` does everything twice
/// as fast without changing its profile. Heights are meters, times are seconds.
///
/// [`MovementProfile::default`] is the Titanfall-style pilot. [`MovementProfile::human`] is a
/// grounded soldier (no wall-running), [`MovementProfile::elite`] a blade master whose dashes
/// reach further.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(crate::Mover)]
#[serde(default)]
pub struct MovementProfile {
    // ------------------------------------------------------------ ground
    /// Jog speed at full stick, m/s.
    pub run_speed: f32,
    /// Speed while `MoveIntent::sprint` is held, m/s.
    pub sprint_speed: f32,
    /// Speed while crouched, m/s.
    pub crouch_speed: f32,
    /// Acceleration toward the wanted speed on the ground, m/s².
    pub ground_accel: f32,
    /// Braking when there is no input, m/s².
    pub ground_brake: f32,
    /// Braking when moving faster than the wanted speed (after slides, dashes and hops), m/s².
    /// Low values keep momentum, which is what makes chaining moves feel fast.
    pub overspeed_brake: f32,
    /// Turn rate of the body toward where it faces, rad/s.
    pub turn_rate: f32,

    // ------------------------------------------------------------ air
    /// Air acceleration toward the stick direction, m/s².
    pub air_accel: f32,
    /// Air control only accelerates up to this speed along the stick direction, m/s; faster
    /// momentum is kept, never killed.
    pub air_speed: f32,
    /// Height of a jump from the ground, m.
    pub jump_height: f32,
    /// Extra jumps in the air (double jump = 1). Refilled on the ground and on walls.
    pub air_jumps: u32,
    /// Height of an air jump, m.
    pub air_jump_height: f32,
    /// How much an air jump turns momentum toward the stick direction, 0..1.
    pub air_jump_redirect: f32,
    /// Jumping still works this long after running off a ledge, s.
    pub coyote_secs: f32,
    /// A jump pressed this long before landing still happens, s.
    pub jump_buffer_secs: f32,
    /// Fastest fall, m/s.
    pub max_fall_speed: f32,

    // ------------------------------------------------------------ slide
    /// Crouching faster than this on the ground starts a slide, m/s.
    pub slide_min_speed: f32,
    /// Speed added when a slide starts, m/s (at most once per `slide_boost_cooldown`).
    pub slide_boost: f32,
    pub slide_boost_cooldown: f32,
    /// Slide friction on flat ground, m/s². Slopes speed slides up or slow them down.
    pub slide_friction: f32,
    /// A slide ends below this speed, m/s.
    pub slide_end_speed: f32,
    /// Slides never go faster than this, m/s.
    pub slide_max_speed: f32,
    /// How fast a slide can be steered, rad/s.
    pub slide_steer: f32,
    /// Capsule height while crouching or sliding, m (0 = keep the capsule).
    pub crouch_height: f32,

    // ------------------------------------------------------------ wall-run
    pub wallrun: bool,
    /// Wall-runs start above this speed along the wall, m/s.
    pub wallrun_min_speed: f32,
    /// Running speed on a wall, m/s.
    pub wallrun_speed: f32,
    pub wallrun_accel: f32,
    /// Longest wall-run, s. Gravity ramps back in over the second half.
    pub wallrun_max_secs: f32,
    /// Fraction of gravity at the start of a wall-run.
    pub wallrun_gravity: f32,
    /// Upward speed when a wall-run starts, m/s.
    pub wallrun_lift: f32,
    /// Walls are only run on with the feet at least this high above the ground, m.
    pub wallrun_min_height: f32,
    /// How far beyond the capsule radius walls are found, m.
    pub wall_reach: f32,
    /// Push away from the wall on a wall-jump, m/s.
    pub wall_jump_push: f32,
    /// Height of a wall-jump, m.
    pub wall_jump_height: f32,

    // ------------------------------------------------------------ mantle
    pub mantle: bool,
    /// Ledges between these heights above the feet are climbed onto, m.
    pub mantle_min_height: f32,
    pub mantle_max_height: f32,
    /// Climbing speed, m/s.
    pub mantle_speed: f32,
    /// Catch ledges too high to climb straight onto and hang from them: shimmy along, climb
    /// up (jump, or hold toward the wall), kick off (jump away) or let go (crouch).
    pub ledge_grab: bool,
    /// Highest ledge above the feet the hands catch, m (from `mantle_max_height` up).
    pub ledge_grab_max_height: f32,
    /// Sideways speed while hanging, m/s.
    pub shimmy_speed: f32,

    // ------------------------------------------------------------ dash
    /// Longest dash, m.
    pub dash_distance: f32,
    /// Time a full-length dash takes, s (shorter dashes are proportionally quicker).
    pub dash_secs: f32,
    /// Dashes available before recharging.
    pub dash_charges: u32,
    /// Seconds to recharge one dash.
    pub dash_recharge_secs: f32,
    /// Attacks miss during the first part of a dash (dodging), s.
    pub dash_iframe_secs: f32,
    /// Speed carried out of a dash, m/s.
    pub dash_exit_speed: f32,
    /// A dash toward a point (`MoveIntent::dash_to`) stops this far short of it, m.
    pub dash_stop_distance: f32,

    // ------------------------------------------------------------ grapple
    /// Fire a grapple hook at a surface ([`Mover::pending_grapple`](crate::Mover)) and get
    /// reeled in: jump or fire again to let go, crouch drops, and a ledge at the end is
    /// climbed onto.
    pub grapple: bool,
    /// Longest shot, m.
    pub grapple_range: f32,
    /// Pull along the rope, m/s².
    pub grapple_pull: f32,
    /// Fastest reel-in along the rope, m/s.
    pub grapple_speed: f32,
    /// Seconds after letting go before the next shot.
    pub grapple_cooldown_secs: f32,

    // ------------------------------------------------------------ feedback
    /// Body mass for footsteps and landings, kg.
    pub mass_kg: f32,
    /// Stride footsteps are emitted by movement unless the entity has a `SkeletonPose`
    /// (then the animation module emits them from its clips). Set false to never emit them.
    pub stride_footsteps: bool,
}

impl Default for MovementProfile {
    fn default() -> Self {
        Self::pilot()
    }
}

impl MovementProfile {
    /// Titanfall-style pilot: fast, momentum-keeping, wall-runs, double jump, two dashes, a
    /// grapple hook.
    pub fn pilot() -> Self {
        Self {
            run_speed: 5.5,
            sprint_speed: 8.5,
            crouch_speed: 2.5,
            ground_accel: 55.0,
            ground_brake: 40.0,
            overspeed_brake: 7.0,
            turn_rate: 14.0,
            air_accel: 14.0,
            air_speed: 4.0,
            jump_height: 1.25,
            air_jumps: 1,
            air_jump_height: 1.0,
            air_jump_redirect: 0.75,
            coyote_secs: 0.12,
            jump_buffer_secs: 0.15,
            max_fall_speed: 60.0,
            slide_min_speed: 6.0,
            slide_boost: 2.5,
            slide_boost_cooldown: 1.5,
            slide_friction: 2.2,
            slide_end_speed: 3.0,
            slide_max_speed: 16.0,
            slide_steer: 1.2,
            crouch_height: 1.0,
            wallrun: true,
            wallrun_min_speed: 4.5,
            wallrun_speed: 9.5,
            wallrun_accel: 18.0,
            wallrun_max_secs: 1.75,
            wallrun_gravity: 0.12,
            wallrun_lift: 1.8,
            wallrun_min_height: 0.5,
            wall_reach: 0.45,
            wall_jump_push: 5.5,
            wall_jump_height: 1.1,
            mantle: true,
            mantle_min_height: 0.4,
            mantle_max_height: 2.1,
            mantle_speed: 6.5,
            ledge_grab: true,
            ledge_grab_max_height: 2.7,
            shimmy_speed: 1.8,
            dash_distance: 6.0,
            dash_secs: 0.18,
            dash_charges: 2,
            dash_recharge_secs: 1.2,
            dash_iframe_secs: 0.15,
            dash_exit_speed: 9.0,
            dash_stop_distance: 1.3,
            grapple: true,
            grapple_range: 40.0,
            grapple_pull: 40.0,
            grapple_speed: 20.0,
            grapple_cooldown_secs: 2.5,
            mass_kg: 80.0,
            stride_footsteps: true,
        }
    }

    /// A grounded soldier: no wall-running, double jump or grapple, one short dash.
    pub fn human() -> Self {
        Self {
            run_speed: 4.5,
            sprint_speed: 6.5,
            overspeed_brake: 20.0,
            air_jumps: 0,
            wallrun: false,
            mantle_max_height: 1.6,
            ledge_grab_max_height: 2.2,
            shimmy_speed: 1.2,
            dash_distance: 3.5,
            dash_charges: 1,
            dash_recharge_secs: 2.0,
            dash_exit_speed: 5.0,
            grapple: false,
            ..Self::pilot()
        }
    }

    /// An elite blade master: long gap-closing dashes. Pair with a high
    /// [`Tempo::speed`](crate::tempo::Tempo) for the enemy that strikes "in a blink".
    pub fn elite() -> Self {
        Self {
            dash_distance: 12.0,
            dash_secs: 0.3,
            dash_charges: 3,
            dash_recharge_secs: 0.8,
            ..Self::pilot()
        }
    }

    /// Look up a preset by name (`pilot`, `human`, `elite`).
    pub fn preset(name: &str) -> Option<Self> {
        match name {
            "pilot" => Some(Self::pilot()),
            "human" => Some(Self::human()),
            "elite" => Some(Self::elite()),
            _ => None,
        }
    }

    /// Launch speed for a jump of `height` meters under gravity `g` (m/s²).
    pub fn launch_speed(height: f32, g: f32) -> f32 {
        (2.0 * g.max(0.0) * height.max(0.0)).sqrt()
    }

    /// Dash speed in m/s (own time).
    pub fn dash_speed(&self) -> f32 {
        self.dash_distance / self.dash_secs.max(1e-3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_through_json() {
        for name in ["pilot", "human", "elite"] {
            let p = MovementProfile::preset(name).unwrap();
            let json = serde_json::to_value(p).unwrap();
            let back: MovementProfile = serde_json::from_value(json).unwrap();
            assert_eq!(p, back);
        }
        // Missing fields fall back to the pilot defaults.
        let partial: MovementProfile =
            serde_json::from_value(serde_json::json!({"sprint_speed": 12.0})).unwrap();
        assert_eq!(partial.sprint_speed, 12.0);
        assert_eq!(partial.run_speed, MovementProfile::pilot().run_speed);
    }

    #[test]
    fn jump_heights_match_launch_speed() {
        let v = MovementProfile::launch_speed(1.25, 9.81);
        // Apex of a ballistic jump: v² / 2g.
        assert!((v * v / (2.0 * 9.81) - 1.25).abs() < 1e-4);
    }
}
