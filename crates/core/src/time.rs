//! Time: real time, world time, fixed steps and time dilation.
//!
//! # Three clocks
//!
//! * **Real time** — wall-clock seconds. Never slowed.
//! * **World time** — real time × [`Time::scale`]. Everything that should slow down during
//!   bullet time (physics, enemies, particles, weather) uses this. `Time::delta_secs()`.
//! * **Entity time** — world time × that entity's [`LocalTimeScale`]. An entity with
//!   `ignore_world_dilation: true` keeps moving at real speed while the world is slowed, which
//!   is how a high-reflex player sees enemies in slow motion.
//!
//! # Fixed steps
//!
//! `FixedUpdate` runs at a fixed **real-time** rate ([`FixedTime::hz`], default 60). Each step
//! advances the world by `1/hz × scale` seconds, so slow motion makes each step smaller instead
//! of making steps rarer. That keeps player input and cameras smooth at any dilation.
//! Inside fixed schedules, [`Time`] reports the fixed step's deltas automatically, so a system
//! can use `time.delta_secs()` in either `Update` or `FixedUpdate`.
//!
//! # Time dilation
//!
//! [`TimeDilation`] collects slow-motion requests from any system ("parry", "focus",
//! "finisher"…), blends them in and out, and the strongest one wins:
//!
//! ```
//! # use engine_core::prelude::*;
//! fn on_enemy_attack(mut dilation: ResMut<TimeDilation>) {
//!     // Drop to 15% speed for 0.4 real seconds, easing in over 0.05 s and out over 0.3 s.
//!     dilation.pulse("parry_window", 0.15, 0.4);
//! }
//! ```

use bevy_ecs::prelude::*;
use bevy_platform::time::Instant;
use serde::{Deserialize, Serialize};

use crate::app::App;

/// Where frame deltas come from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TimeSource {
    /// Measure the wall clock (normal play).
    Realtime,
    /// Every frame advances by exactly this many real seconds (tests, headless capture,
    /// AI-driven stepping). Deterministic.
    Manual { step_secs: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Context {
    Frame,
    Fixed,
}

/// The clock resource. See the [module docs](self).
#[derive(Resource, Debug, Clone)]
pub struct Time {
    source: TimeSource,
    last_instant: Option<Instant>,
    max_delta: f64,
    base_scale: f64,
    scale: f64,
    frame: u64,
    real_delta: f64,
    real_elapsed: f64,
    delta: f64,
    elapsed: f64,
    context: Context,
    fixed_real_delta: f64,
    fixed_delta: f64,
}

impl Default for Time {
    fn default() -> Self {
        Self {
            source: TimeSource::Realtime,
            last_instant: None,
            max_delta: 0.25,
            base_scale: 1.0,
            scale: 1.0,
            frame: 0,
            real_delta: 0.0,
            real_elapsed: 0.0,
            delta: 0.0,
            elapsed: 0.0,
            context: Context::Frame,
            fixed_real_delta: 0.0,
            fixed_delta: 0.0,
        }
    }
}

impl Time {
    /// World-time seconds since the last frame (or the current fixed step's world delta when
    /// called from a fixed schedule).
    pub fn delta_secs(&self) -> f32 {
        self.delta_secs_f64() as f32
    }

    pub fn delta_secs_f64(&self) -> f64 {
        match self.context {
            Context::Frame => self.delta,
            Context::Fixed => self.fixed_delta,
        }
    }

    /// Real seconds since the last frame (or the fixed step's real length in fixed schedules).
    /// Unaffected by dilation; still respects the user pause ([`Time::base_scale`] = 0).
    pub fn real_delta_secs(&self) -> f32 {
        (match self.context {
            Context::Frame => self.real_delta,
            Context::Fixed => self.fixed_real_delta,
        }) as f32
    }

    /// Delta for one entity, honoring its [`LocalTimeScale`].
    pub fn delta_for(&self, local: Option<&LocalTimeScale>) -> f32 {
        match local {
            None => self.delta_secs(),
            Some(l) if l.ignore_world_dilation => {
                self.real_delta_secs() * self.base_scale as f32 * l.factor
            }
            Some(l) => self.delta_secs() * l.factor,
        }
    }

    /// World seconds since start.
    pub fn elapsed_secs(&self) -> f32 {
        self.elapsed as f32
    }

    pub fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed
    }

    /// Real seconds since start.
    pub fn real_elapsed_secs_f64(&self) -> f64 {
        self.real_elapsed
    }

    /// Effective world time scale this frame (`base_scale × dilation`).
    pub fn scale(&self) -> f32 {
        self.scale as f32
    }

    /// User-controlled speed (1 = normal, 0 = paused, 2 = fast-forward).
    pub fn base_scale(&self) -> f32 {
        self.base_scale as f32
    }

    pub fn set_base_scale(&mut self, scale: f32) {
        self.base_scale = scale.max(0.0) as f64;
    }

    pub fn pause(&mut self) {
        self.base_scale = 0.0;
    }

    pub fn resume(&mut self) {
        if self.base_scale == 0.0 {
            self.base_scale = 1.0;
        }
    }

    pub fn is_paused(&self) -> bool {
        self.base_scale == 0.0
    }

    /// Frames rendered since start.
    pub fn frame_count(&self) -> u64 {
        self.frame
    }

    pub fn source(&self) -> TimeSource {
        self.source
    }

    /// Switch between wall-clock and fixed manual stepping.
    pub fn set_source(&mut self, source: TimeSource) {
        self.source = source;
        self.last_instant = None;
    }

    /// Clamp for a single frame's real delta (prevents huge jumps after a hitch).
    pub fn set_max_delta(&mut self, secs: f64) {
        self.max_delta = secs.max(0.001);
    }

    /// Advance the frame clock by `real_delta` seconds with effective `scale`.
    fn advance(&mut self, real_delta: f64, dilation: f64) {
        self.real_delta = real_delta;
        self.real_elapsed += real_delta;
        self.scale = self.base_scale * dilation;
        self.delta = real_delta * self.scale;
        self.elapsed += self.delta;
        self.frame += 1;
    }
}

/// Fixed-step bookkeeping. See the [module docs](self).
#[derive(Resource, Debug, Clone)]
pub struct FixedTime {
    hz: f64,
    accumulator: f64,
    max_steps: u32,
    budget_secs: f64,
    steps: u32,
    current_step: u32,
    world_step: f64,
    alpha: f32,
    tick: u64,
}

impl Default for FixedTime {
    fn default() -> Self {
        Self {
            hz: 60.0,
            accumulator: 0.0,
            max_steps: 8,
            budget_secs: 1.0 / 30.0,
            steps: 0,
            current_step: 0,
            world_step: 1.0 / 60.0,
            alpha: 0.0,
            tick: 0,
        }
    }
}

impl FixedTime {
    /// Fixed steps per real second.
    pub fn hz(&self) -> f64 {
        self.hz
    }

    pub fn set_hz(&mut self, hz: f64) {
        self.hz = hz.clamp(1.0, 1000.0);
    }

    /// Real seconds per step.
    pub fn step_real_secs(&self) -> f64 {
        1.0 / self.hz
    }

    /// World seconds per step this frame (`step_real × Time::scale`).
    pub fn world_step_secs(&self) -> f64 {
        self.world_step
    }

    /// Fraction of a step left in the accumulator after this frame's steps (0..1).
    /// Used to interpolate rendering between the last two simulated states.
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Total fixed steps run since start.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn steps_this_frame(&self) -> u32 {
        self.steps
    }

    /// Index of the step currently running this frame.
    pub fn current_step(&self) -> u32 {
        self.current_step
    }

    /// Cap on steps per frame (protects against the "spiral of death" after a long hitch).
    pub fn set_max_steps(&mut self, max: u32) {
        self.max_steps = max.max(1);
    }

    /// Wall-clock seconds the fixed steps of one real-time frame may take (default 1/30 s).
    /// When the steps cost more than that, the frame stops stepping and drops the backlog, so
    /// the world runs slower than real time instead of the frame rate collapsing: each slow
    /// frame would otherwise run more catch-up steps, making the next frame slower still.
    /// At least one step always runs; [`TimeSource::Manual`] clocks ignore the budget so
    /// tests and captures stay deterministic.
    pub fn step_budget_secs(&self) -> f64 {
        self.budget_secs
    }

    pub fn set_step_budget_secs(&mut self, secs: f64) {
        self.budget_secs = secs.max(0.0);
    }

    /// The frame loop ran only `ran` of this frame's steps: drop the rest and the backlog.
    pub(crate) fn stop_early(&mut self, ran: u32) {
        if ran >= self.steps {
            return;
        }
        self.steps = ran;
        let step = self.step_real_secs();
        self.accumulator = self.accumulator.min(step);
        self.alpha = (self.accumulator / step).clamp(0.0, 1.0) as f32;
    }

    fn accumulate(&mut self, real_delta: f64, scale: f64) {
        let step = self.step_real_secs();
        self.accumulator += real_delta;
        let due = (self.accumulator / step).floor() as u32;
        self.steps = due.min(self.max_steps);
        self.accumulator -= self.steps as f64 * step;
        if due > self.max_steps {
            // Drop the backlog rather than trying to catch up forever.
            self.accumulator = self.accumulator.min(step);
        }
        self.world_step = step * scale;
        self.alpha = (self.accumulator / step).clamp(0.0, 1.0) as f32;
    }
}

/// Collects slow-motion (or fast-forward) requests and blends them. See the [module docs](self).
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimeDilation {
    requests: Vec<DilationRequest>,
    #[serde(skip)]
    current: f64,
}

/// One dilation request. Created through [`TimeDilation`] helpers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DilationRequest {
    pub key: String,
    /// World time scale while fully active (0.15 = 15% speed).
    pub scale: f32,
    /// Real seconds at full strength before it releases itself. `None` = until released.
    pub hold: Option<f32>,
    /// Real seconds to ease in.
    pub blend_in: f32,
    /// Real seconds to ease out after release.
    pub blend_out: f32,
    #[serde(default)]
    age: f32,
    #[serde(default)]
    released_at: Option<f32>,
}

impl DilationRequest {
    fn weight(&self) -> f32 {
        let fade_in = if self.blend_in > 0.0 {
            (self.age / self.blend_in).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let fade_out = match self.released_at {
            Some(t) if self.blend_out > 0.0 => {
                1.0 - ((self.age - t) / self.blend_out).clamp(0.0, 1.0)
            }
            Some(_) => 0.0,
            None => 1.0,
        };
        smoothstep(fade_in.min(fade_out))
    }

    fn finished(&self) -> bool {
        matches!(self.released_at, Some(t) if self.age - t >= self.blend_out)
    }
}

fn smoothstep(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

impl TimeDilation {
    /// Add or replace a request.
    pub fn request(&mut self, request: DilationRequest) {
        self.requests.retain(|r| r.key != request.key);
        self.requests.push(request);
    }

    /// A short slow-motion burst: ease in quickly, hold for `hold_secs`, ease out.
    /// Ideal for "every attack looks like slow motion" moments.
    pub fn pulse(&mut self, key: impl Into<String>, scale: f32, hold_secs: f32) {
        self.request(DilationRequest {
            key: key.into(),
            scale,
            hold: Some(hold_secs),
            blend_in: 0.05,
            blend_out: 0.25,
            age: 0.0,
            released_at: None,
        });
    }

    /// Slow the world until [`TimeDilation::release`] is called (e.g. a "focus" ability).
    pub fn hold(&mut self, key: impl Into<String>, scale: f32, blend_in: f32) {
        let key = key.into();
        if let Some(existing) = self.requests.iter_mut().find(|r| r.key == key) {
            existing.scale = scale;
            if existing.released_at.is_some() {
                existing.released_at = None;
                existing.age = 0.0;
            }
            return;
        }
        self.request(DilationRequest {
            key,
            scale,
            hold: None,
            blend_in,
            blend_out: 0.3,
            age: 0.0,
            released_at: None,
        });
    }

    /// Begin easing a request out.
    pub fn release(&mut self, key: &str) {
        for r in &mut self.requests {
            if r.key == key && r.released_at.is_none() {
                r.released_at = Some(r.age);
            }
        }
    }

    pub fn clear(&mut self) {
        self.requests.clear();
    }

    pub fn is_active(&self, key: &str) -> bool {
        self.requests.iter().any(|r| r.key == key)
    }

    pub fn requests(&self) -> &[DilationRequest] {
        &self.requests
    }

    /// Current combined factor (1 = normal speed).
    pub fn factor(&self) -> f32 {
        if self.requests.is_empty() {
            1.0
        } else {
            self.current as f32
        }
    }

    /// Advance requests by `real_dt` and compute the combined factor.
    /// Slowdowns combine by taking the strongest; speed-ups by taking the fastest.
    pub fn advance(&mut self, real_dt: f32) -> f64 {
        for r in &mut self.requests {
            r.age += real_dt;
            if let (Some(hold), None) = (r.hold, r.released_at)
                && r.age >= r.blend_in + hold
            {
                r.released_at = Some(r.age);
            }
        }
        self.requests.retain(|r| !r.finished());
        let mut slow = 1.0f32;
        let mut fast = 1.0f32;
        for r in &self.requests {
            let s = 1.0 + (r.scale - 1.0) * r.weight();
            if s < 1.0 {
                slow = slow.min(s);
            } else {
                fast = fast.max(s);
            }
        }
        self.current = (slow * fast).max(0.0) as f64;
        self.current
    }
}

/// Per-entity time scale. See the [module docs](self).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalTimeScale {
    /// Multiplier on this entity's delta (0.5 = half speed, 2 = double).
    pub factor: f32,
    /// If true, this entity runs on real time and ignores world slow motion.
    pub ignore_world_dilation: bool,
}

impl Default for LocalTimeScale {
    fn default() -> Self {
        Self {
            factor: 1.0,
            ignore_world_dilation: false,
        }
    }
}

impl LocalTimeScale {
    /// An entity that moves at full real speed through slowed time (times `factor`).
    pub fn unaffected_by_dilation(factor: f32) -> Self {
        Self {
            factor,
            ignore_world_dilation: true,
        }
    }
}

/// Engine system (runs first each frame): advances clocks and schedules fixed steps.
pub fn time_system(
    mut time: ResMut<Time>,
    mut fixed: ResMut<FixedTime>,
    mut dilation: ResMut<TimeDilation>,
) {
    let real_delta = match time.source {
        TimeSource::Manual { step_secs } => step_secs,
        TimeSource::Realtime => {
            let now = Instant::now();
            let d = time
                .last_instant
                .map(|last| now.duration_since(last).as_secs_f64())
                .unwrap_or(0.0);
            time.last_instant = Some(now);
            d
        }
    }
    .clamp(0.0, time.max_delta);

    let factor = dilation.advance(real_delta as f32);
    time.context = Context::Frame;
    time.advance(real_delta, factor);
    let scale = time.scale;
    // A user pause stops the fixed loop entirely; dilation only shrinks each step.
    let fixed_real = if time.is_paused() { 0.0 } else { real_delta };
    fixed.accumulate(fixed_real, scale);
}

/// Switch [`Time`] into fixed-step context. Called by the frame loop.
pub(crate) fn enter_fixed_step(world: &mut World, index: u32) {
    let (real, world_step) = {
        let mut fixed = world.resource_mut::<FixedTime>();
        fixed.current_step = index;
        fixed.tick += 1;
        (fixed.step_real_secs(), fixed.world_step)
    };
    let mut time = world.resource_mut::<Time>();
    time.context = Context::Fixed;
    time.fixed_real_delta = real;
    time.fixed_delta = world_step;
}

/// Switch [`Time`] back to per-frame context. Called by the frame loop.
pub(crate) fn exit_fixed_steps(world: &mut World) {
    world.resource_mut::<Time>().context = Context::Frame;
}

pub(crate) fn register_api(app: &mut App) {
    app.register_api(
        "time.status",
        "Clock state: scale, pause, frame count, fixed rate, active dilation requests.",
        "{}",
        |world, _| {
            let time = world.resource::<Time>();
            let fixed = world.resource::<FixedTime>();
            let dil = world.resource::<TimeDilation>();
            Ok(serde_json::json!({
                "frame": time.frame_count(),
                "elapsed_world_secs": time.elapsed_secs_f64(),
                "elapsed_real_secs": time.real_elapsed_secs_f64(),
                "scale": time.scale(),
                "base_scale": time.base_scale(),
                "paused": time.is_paused(),
                "fixed_hz": fixed.hz(),
                "dilation": dil.requests(),
            }))
        },
    )
    .register_api(
        "time.set_speed",
        "Set the user time scale (1 normal, 0 pause, 0.25 slow, 2 fast-forward).",
        r#"{"scale": 0.5}"#,
        |world, args| {
            let scale = args["scale"].as_f64().ok_or("missing number 'scale'")? as f32;
            world.resource_mut::<Time>().set_base_scale(scale);
            Ok(serde_json::json!({"ok": true}))
        },
    )
    .register_api(
        "time.dilate",
        "Start a slow-motion pulse. hold_secs is real time at full strength.",
        r#"{"key": "test", "scale": 0.2, "hold_secs": 1.0}"#,
        |world, args| {
            let key = args["key"].as_str().unwrap_or("api").to_string();
            let scale = args["scale"].as_f64().unwrap_or(0.2) as f32;
            let hold = args["hold_secs"].as_f64().unwrap_or(1.0) as f32;
            world.resource_mut::<TimeDilation>().pulse(key, scale, hold);
            Ok(serde_json::json!({"ok": true}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    fn manual_app(step: f64) -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs: step });
        app
    }

    #[test]
    fn fixed_steps_follow_real_time() {
        let mut app = manual_app(1.0 / 30.0);
        #[derive(Resource, Default)]
        struct Steps(u32);
        app.init_resource::<Steps>()
            .add_systems(FixedUpdate, |mut s: ResMut<Steps>| s.0 += 1);
        app.update_n(10);
        // 10 frames at 30 fps = 1/3 s = 20 steps at 60 Hz.
        assert_eq!(app.world.resource::<Steps>().0, 20);
    }

    #[test]
    fn slow_steps_stop_at_the_budget_in_real_time() {
        #[derive(Resource, Default)]
        struct Steps(u32);
        let mut app = App::new();
        app.world
            .resource_mut::<FixedTime>()
            .set_step_budget_secs(0.0);
        app.init_resource::<Steps>()
            .add_systems(FixedUpdate, |mut s: ResMut<Steps>| s.0 += 1);
        app.update();
        let before = app.world.resource::<Steps>().0;
        // 200 ms behind: 12 steps are due, but the budget lets only the first one run.
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.update();
        assert_eq!(app.world.resource::<Steps>().0 - before, 1);
        let fixed = app.world.resource::<FixedTime>();
        assert_eq!(fixed.steps_this_frame(), 1);
        assert!(fixed.alpha() <= 1.0);
    }

    #[test]
    fn manual_clocks_ignore_the_step_budget() {
        #[derive(Resource, Default)]
        struct Steps(u32);
        let mut app = manual_app(0.1);
        app.world
            .resource_mut::<FixedTime>()
            .set_step_budget_secs(0.0);
        app.init_resource::<Steps>()
            .add_systems(FixedUpdate, |mut s: ResMut<Steps>| s.0 += 1);
        app.update();
        assert_eq!(app.world.resource::<Steps>().0, 6);
    }

    #[test]
    fn dilation_shrinks_world_delta_not_step_count() {
        let mut app = manual_app(1.0 / 60.0);
        app.world
            .resource_mut::<TimeDilation>()
            .hold("t", 0.25, 0.0);
        #[derive(Resource, Default)]
        struct Acc {
            world: f64,
            steps: u32,
        }
        app.init_resource::<Acc>()
            .add_systems(FixedUpdate, |t: Res<Time>, mut a: ResMut<Acc>| {
                a.world += t.delta_secs_f64();
                a.steps += 1;
            });
        app.update_n(60);
        let acc = app.world.resource::<Acc>();
        assert_eq!(acc.steps, 60);
        assert!(
            (acc.world - 0.25).abs() < 0.02,
            "world advanced {}",
            acc.world
        );
    }

    #[test]
    fn local_scale_ignores_dilation() {
        let mut time = Time::default();
        time.advance(0.1, 0.2);
        let player = LocalTimeScale::unaffected_by_dilation(1.0);
        assert!((time.delta_for(Some(&player)) - 0.1).abs() < 1e-6);
        assert!((time.delta_for(None) - 0.02).abs() < 1e-6);
    }

    #[test]
    fn pulse_releases_itself() {
        let mut d = TimeDilation::default();
        d.pulse("hit", 0.1, 0.2);
        let mut min = 1.0f64;
        for _ in 0..100 {
            min = min.min(d.advance(0.01));
        }
        assert!(min < 0.2);
        assert!(d.requests().is_empty());
        assert_eq!(d.factor(), 1.0);
    }

    #[test]
    fn pause_stops_fixed_steps() {
        let mut app = manual_app(1.0 / 60.0);
        app.world.resource_mut::<Time>().pause();
        app.update_n(5);
        assert_eq!(app.world.resource::<FixedTime>().tick(), 0);
    }
}
