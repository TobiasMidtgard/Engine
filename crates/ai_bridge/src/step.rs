//! Frame stepping, pause and resume as seen by agents.
//!
//! `engine.step {frames: n}` answers after exactly `n` more frames have run. If the game was
//! paused (`Time::pause`, `engine.pause` or `time.set_speed 0`), the bridge resumes time for
//! those `n` frames and pauses it again before the next frame starts, so an agent can advance a
//! paused world frame by frame and look at it in between.
//!
//! The counter is decremented at the start of every frame (before `time_system`, so a resume
//! takes effect in the same frame) and finished steps are answered at the end of the frame
//! (in `Last`, after rendering), so the answer means "those frames are fully done".

use bevy_ecs::prelude::*;
use engine_core::time::Time;
use serde_json::{Value, json};

use crate::protocol::{ReplyTo, RpcError, codes};

/// Largest `frames` value accepted by `engine.step`.
pub const MAX_STEP_FRAMES: u64 = 1_000_000;

/// One `engine.step` in progress.
#[derive(Debug)]
pub(crate) struct PendingStep {
    /// Frames still to run (counted down at frame start).
    remaining: u64,
    frames: u64,
    start_frame: u64,
    reply: Option<ReplyTo>,
}

/// Steps in progress and the pause state the bridge restores afterwards.
#[derive(Resource, Debug, Default)]
pub struct StepControl {
    steps: Vec<PendingStep>,
    /// Pause again once every step in progress has finished.
    repause: bool,
    /// The user time scale before the bridge paused the game, restored by resume and steps.
    saved_scale: Option<f32>,
}

impl StepControl {
    /// True while at least one `engine.step` is running frames.
    pub fn is_stepping(&self) -> bool {
        !self.steps.is_empty()
    }

    /// The most frames any step in progress still has to run.
    pub fn frames_remaining(&self) -> u64 {
        self.steps.iter().map(|s| s.remaining).max().unwrap_or(0)
    }

    /// True if the game will be paused again when the steps in progress finish.
    pub fn pauses_after_steps(&self) -> bool {
        self.repause
    }

    fn resume_scale(&self) -> f32 {
        self.saved_scale.filter(|s| *s > 0.0).unwrap_or(1.0)
    }
}

/// Read `{"frames": n}` (default 1).
pub(crate) fn parse_frames(params: &Value) -> Result<u64, RpcError> {
    let frames = match params.get("frames") {
        None | Some(Value::Null) => 1,
        Some(v) => v.as_u64().ok_or_else(|| {
            RpcError::new(
                codes::INVALID_PARAMS,
                format!("\"frames\" must be a whole number from 0 to {MAX_STEP_FRAMES}, got {v}"),
            )
        })?,
    };
    if frames > MAX_STEP_FRAMES {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!("\"frames\" is {frames}; step at most {MAX_STEP_FRAMES} frames per call"),
        ));
    }
    Ok(frames)
}

fn step_result(world: &World, frames: u64, start_frame: u64) -> Value {
    let time = world.resource::<Time>();
    json!({
        "frames": frames,
        "start_frame": start_frame,
        "frame": time.frame_count(),
        "paused": time.is_paused(),
        "elapsed_world_secs": time.elapsed_secs_f64(),
    })
}

/// Start stepping `frames` frames. `reply` (if any) is answered when they have run.
pub(crate) fn begin_step(world: &mut World, frames: u64, reply: Option<ReplyTo>) {
    let start_frame = world.resource::<Time>().frame_count();
    if frames == 0 {
        if let Some(reply) = reply {
            reply.send(Ok(step_result(world, 0, start_frame)));
        }
        return;
    }
    let paused = world.resource::<Time>().is_paused();
    let scale = {
        let mut control = world.resource_mut::<StepControl>();
        if paused {
            control.repause = true;
        }
        control.steps.push(PendingStep {
            remaining: frames,
            frames,
            start_frame,
            reply,
        });
        control.resume_scale()
    };
    if paused {
        world.resource_mut::<Time>().set_base_scale(scale);
    }
}

/// Frame start: every step in progress counts this frame.
pub(crate) fn count_frame(world: &mut World) {
    for step in &mut world.resource_mut::<StepControl>().steps {
        step.remaining = step.remaining.saturating_sub(1);
    }
}

/// Frame end: answer finished steps and pause again if the game was paused before.
pub(crate) fn finish_steps(world: &mut World) {
    let (done, repause) = {
        let mut control = world.resource_mut::<StepControl>();
        if control.steps.is_empty() && !control.repause {
            return;
        }
        let done: Vec<PendingStep> = control.steps.extract_if(.., |s| s.remaining == 0).collect();
        let repause = control.steps.is_empty() && control.repause;
        if repause {
            control.repause = false;
        }
        (done, repause)
    };
    if repause {
        world.resource_mut::<Time>().pause();
    }
    for step in done {
        if let Some(reply) = step.reply {
            reply.send(Ok(step_result(world, step.frames, step.start_frame)));
        }
    }
}

/// `engine.pause`: pause now, or after the steps in progress if any.
pub(crate) fn pause(world: &mut World) -> Value {
    let stepping = world.resource::<StepControl>().is_stepping();
    if stepping {
        // Remember the speed the step runs at, so engine.resume restores it after the
        // deferred pause (Time::pause itself forgets it).
        let speed = world.resource::<Time>().base_scale();
        let mut control = world.resource_mut::<StepControl>();
        control.repause = true;
        if speed > 0.0 {
            control.saved_scale = Some(speed);
        }
        return json!({"paused": false, "pause_after_step": true});
    }
    let scale = {
        let time = world.resource::<Time>();
        (!time.is_paused()).then(|| time.base_scale())
    };
    if let Some(scale) = scale {
        world.resource_mut::<StepControl>().saved_scale = Some(scale);
        world.resource_mut::<Time>().pause();
    }
    json!({"paused": true, "frame": world.resource::<Time>().frame_count()})
}

/// `engine.resume`: undo a pause (restoring the speed from before it) and cancel a pending
/// re-pause after steps.
pub(crate) fn resume(world: &mut World) -> Value {
    let scale = {
        let mut control = world.resource_mut::<StepControl>();
        control.repause = false;
        let scale = control.resume_scale();
        control.saved_scale = None;
        scale
    };
    let mut time = world.resource_mut::<Time>();
    if time.is_paused() {
        time.set_base_scale(scale);
    }
    json!({"paused": false, "speed": time.base_scale(), "frame": time.frame_count()})
}
