//! The [`DebugDraw`] resource: storage, lifetimes and depth modes of debug lines.
//!
//! Every shape is tessellated into line segments with `f64` world-space endpoints when it is
//! drawn. The render feature converts them to camera-relative `f32` once per frame, so lines
//! stay exact anywhere in a large world.
//!
//! # Lifetimes
//!
//! A shape lives for **this frame** by default: it is drawn in the frame it was added and
//! removed at the start of the next frame (in `First`). Give it a duration with
//! [`ShapeHandle::duration`] to keep it for that many **real** seconds: durations keep
//! counting while the world is paused or in slow motion, so a marker left during bullet time
//! does not hang around forever. [`ShapeHandle::forever`] keeps a shape until
//! [`DebugDraw::clear`].
//!
//! Shapes added after `Render` (in `Last`) with no duration are removed before they are ever
//! drawn. Systems in `FixedUpdate` should give their shapes a duration of about one fixed step,
//! because the fixed loop can run zero or several times per frame.

use bevy_ecs::prelude::*;
use engine_core::math::DVec3;
use serde::{Deserialize, Serialize};

use crate::color::Color;

/// Whether a shape is hidden by geometry in front of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepthMode {
    /// Depth-tested against the scene (hidden behind closer geometry). Lines never write
    /// depth, so they do not hide each other or anything drawn later.
    #[default]
    Tested,
    /// Always visible, drawn over everything else in the scene.
    #[serde(alias = "always", alias = "overlay")]
    OnTop,
}

impl DepthMode {
    /// Both modes, in the order they are rendered.
    pub const ALL: [DepthMode; 2] = [DepthMode::Tested, DepthMode::OnTop];

    pub(crate) fn index(self) -> usize {
        match self {
            DepthMode::Tested => 0,
            DepthMode::OnTop => 1,
        }
    }
}

/// One line segment with `f64` world-space endpoints (meters) and a linear RGBA color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugLine {
    pub a: DVec3,
    pub b: DVec3,
    pub color: Color,
}

#[derive(Debug, Clone, Copy)]
struct TimedLine {
    line: DebugLine,
    /// Real time (seconds since start) at which the line disappears. `INFINITY` = never.
    expires: f64,
    depth: DepthMode,
}

/// Immediate-mode debug drawing. Take it as `ResMut<DebugDraw>` in any system and add shapes;
/// they are drawn as lines over the scene this frame (or longer, see the [module docs](self)).
///
/// ```ignore
/// fn show_targets(mut debug: ResMut<DebugDraw>, q: Query<&GlobalTransform, With<Target>>) {
///     for t in &q {
///         debug.sphere(t.translation(), 0.5, RED);
///         debug.arrow(t.translation(), t.translation() + DVec3::Y * 2.0, YELLOW).on_top();
///     }
///     debug.line(DVec3::ZERO, DVec3::X * 10.0, CYAN).duration(3.0);
/// }
/// ```
#[derive(Resource, Debug)]
pub struct DebugDraw {
    /// When false, new shapes are ignored (and nothing is drawn). Mirrors
    /// `DebugViews::enabled`, which is the switch to use from data or the AI bridge.
    pub enabled: bool,
    /// Line segments per full circle for circles, spheres, capsules and cylinders (4..=256).
    pub segments: u32,
    /// Upper bound on stored lines (all lifetimes together). Shapes beyond it are dropped
    /// and counted in [`DebugDraw::dropped`].
    pub max_lines: usize,
    now: f64,
    frame: [Vec<DebugLine>; 2],
    timed: Vec<TimedLine>,
    dropped: u64,
}

impl Default for DebugDraw {
    fn default() -> Self {
        Self {
            enabled: true,
            segments: 32,
            max_lines: 1 << 18,
            now: 0.0,
            frame: [Vec::new(), Vec::new()],
            timed: Vec::new(),
            dropped: 0,
        }
    }
}

impl DebugDraw {
    /// Pass as a duration to keep a shape until [`DebugDraw::clear`].
    pub const FOREVER: f32 = f32::INFINITY;

    /// Start a new frame at `now_real_secs` (real seconds since start): drop this-frame shapes
    /// and expired timed shapes. The plugin calls this in `First`; call it yourself only when
    /// driving a `DebugDraw` without the plugin.
    pub fn begin_frame(&mut self, now_real_secs: f64) {
        self.now = now_real_secs;
        for list in &mut self.frame {
            list.clear();
        }
        self.timed.retain(|t| t.expires > now_real_secs);
    }

    /// Real time (seconds since start) of the current frame, as given to `begin_frame`.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Remove every shape, including timed and permanent ones.
    pub fn clear(&mut self) {
        for list in &mut self.frame {
            list.clear();
        }
        self.timed.clear();
    }

    /// Number of stored line segments (every lifetime and depth mode).
    pub fn line_count(&self) -> usize {
        self.frame[0].len() + self.frame[1].len() + self.timed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.line_count() == 0
    }

    /// Number of stored segments that outlive this frame (timed or permanent).
    pub fn timed_line_count(&self) -> usize {
        self.timed.len()
    }

    /// Total segments rejected because [`DebugDraw::max_lines`] was reached.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Every segment that should be drawn now with the given depth mode.
    pub fn lines(&self, depth: DepthMode) -> impl Iterator<Item = &DebugLine> + '_ {
        self.frame[depth.index()].iter().chain(
            self.timed
                .iter()
                .filter(move |t| t.depth == depth)
                .map(|t| &t.line),
        )
    }

    /// Add one raw segment. Shapes call this; it is the only place lines are stored.
    #[inline]
    pub(crate) fn push(&mut self, a: DVec3, b: DVec3, color: Color) {
        if !self.enabled {
            return;
        }
        if self.line_count() >= self.max_lines {
            self.dropped += 1;
            return;
        }
        self.frame[0].push(DebugLine { a, b, color });
    }

    /// Index where the next shape's segments start (shapes are first stored as
    /// this-frame, depth-tested lines).
    #[inline]
    pub(crate) fn begin_shape(&self) -> usize {
        self.frame[0].len()
    }

    /// Handle for the segments pushed since `start`.
    #[inline]
    pub(crate) fn finish_shape(&mut self, start: usize) -> ShapeHandle<'_> {
        ShapeHandle {
            draw: self,
            start,
            depth: DepthMode::Tested,
            timed: false,
        }
    }

    /// Real-time expiry for `secs`, or `None` for "this frame only".
    fn expiry(&self, secs: f32) -> Option<f64> {
        if secs.is_nan() || secs <= 0.0 {
            None
        } else if secs.is_infinite() {
            Some(f64::INFINITY)
        } else {
            Some(self.now + secs as f64)
        }
    }
}

/// Returned by every shape method; changes how long the shape lives and whether it is
/// depth-tested. Ignore it for a depth-tested shape that lasts this frame.
///
/// ```ignore
/// debug.sphere(p, 1.0, RED).duration(2.0).on_top();
/// ```
pub struct ShapeHandle<'a> {
    draw: &'a mut DebugDraw,
    /// Index of the shape's first segment in the list it currently lives in. The shape's
    /// segments are always the tail of that list (the handle borrows the storage).
    start: usize,
    depth: DepthMode,
    timed: bool,
}

impl ShapeHandle<'_> {
    /// Keep the shape for `secs` real seconds (still counts while the world is paused or
    /// slowed). `0` (or negative) = this frame only, [`DebugDraw::FOREVER`] = until cleared.
    pub fn duration(mut self, secs: f32) -> Self {
        let expires = self.draw.expiry(secs);
        let draw = &mut *self.draw;
        match (self.timed, expires) {
            (false, None) => {}
            (false, Some(expires)) => {
                let depth = self.depth;
                let start = draw.timed.len();
                draw.timed
                    .extend(
                        draw.frame[depth.index()]
                            .drain(self.start..)
                            .map(|line| TimedLine {
                                line,
                                expires,
                                depth,
                            }),
                    );
                self.start = start;
                self.timed = true;
            }
            (true, Some(expires)) => {
                for t in &mut draw.timed[self.start..] {
                    t.expires = expires;
                }
            }
            (true, None) => {
                let list = &mut draw.frame[self.depth.index()];
                let start = list.len();
                list.extend(draw.timed.drain(self.start..).map(|t| t.line));
                self.start = start;
                self.timed = false;
            }
        }
        self
    }

    /// Keep the shape until [`DebugDraw::clear`] (or `debug.clear`).
    pub fn forever(self) -> Self {
        self.duration(DebugDraw::FOREVER)
    }

    /// Choose whether closer geometry hides the shape.
    pub fn depth(mut self, mode: DepthMode) -> Self {
        if mode == self.depth {
            return self;
        }
        let draw = &mut *self.draw;
        if self.timed {
            for t in &mut draw.timed[self.start..] {
                t.depth = mode;
            }
        } else {
            let [tested, on_top] = &mut draw.frame;
            let (from, to) = match mode {
                DepthMode::OnTop => (tested, on_top),
                DepthMode::Tested => (on_top, tested),
            };
            let start = to.len();
            to.extend(from.drain(self.start..));
            self.start = start;
        }
        self.depth = mode;
        self
    }

    /// Draw the shape over everything (shorthand for `.depth(DepthMode::OnTop)`).
    pub fn on_top(self) -> Self {
        self.depth(DepthMode::OnTop)
    }

    /// Number of line segments the shape was tessellated into.
    pub fn line_count(&self) -> usize {
        let list_len = if self.timed {
            self.draw.timed.len()
        } else {
            self.draw.frame[self.depth.index()].len()
        };
        list_len - self.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::*;

    fn count(d: &DebugDraw, depth: DepthMode) -> usize {
        d.lines(depth).count()
    }

    #[test]
    fn frame_shapes_last_until_next_frame() {
        let mut d = DebugDraw::default();
        d.begin_frame(1.0);
        d.line(DVec3::ZERO, DVec3::X, RED);
        assert_eq!(count(&d, DepthMode::Tested), 1);
        d.begin_frame(1.0); // even if no real time passed (paused, manual step 0)
        assert!(d.is_empty());
    }

    #[test]
    fn timed_shapes_expire_in_real_time() {
        let mut d = DebugDraw::default();
        d.begin_frame(10.0);
        d.line(DVec3::ZERO, DVec3::X, RED).duration(0.5);
        d.line(DVec3::ZERO, DVec3::Y, GREEN).forever();
        d.line(DVec3::ZERO, DVec3::Z, BLUE).duration(0.0);
        assert_eq!(d.line_count(), 3);
        d.begin_frame(10.25);
        assert_eq!(d.line_count(), 2);
        d.begin_frame(10.5);
        assert_eq!(d.line_count(), 1, "expires exactly at its end time");
        d.begin_frame(1.0e6);
        assert_eq!(d.line_count(), 1, "forever survives");
        d.clear();
        assert!(d.is_empty());
    }

    #[test]
    fn handle_moves_between_lists_in_any_order() {
        let mut d = DebugDraw::default();
        d.begin_frame(0.0);
        d.line(DVec3::ZERO, DVec3::X, RED); // stays: tested, this frame
        let n = d
            .sphere(DVec3::ZERO, 1.0, GREEN)
            .on_top()
            .duration(2.0)
            .line_count();
        assert!(n > 0);
        d.line(DVec3::ZERO, DVec3::Y, BLUE).duration(2.0).on_top();
        d.line(DVec3::ZERO, DVec3::Z, BLUE)
            .duration(2.0)
            .duration(0.0)
            .on_top();
        assert_eq!(count(&d, DepthMode::Tested), 1);
        assert_eq!(count(&d, DepthMode::OnTop), n + 2);
        assert_eq!(d.timed_line_count(), n + 1);
        // Back to tested.
        d.line(DVec3::ZERO, DVec3::NEG_X, RED)
            .on_top()
            .depth(DepthMode::Tested);
        assert_eq!(count(&d, DepthMode::Tested), 2);
        d.begin_frame(1.0);
        assert_eq!(count(&d, DepthMode::OnTop), n + 1);
        assert_eq!(count(&d, DepthMode::Tested), 0);
        d.begin_frame(2.0);
        assert!(d.is_empty());
    }

    #[test]
    fn negative_and_nan_durations_mean_this_frame() {
        let mut d = DebugDraw::default();
        d.begin_frame(0.0);
        d.line(DVec3::ZERO, DVec3::X, RED).duration(-1.0);
        d.line(DVec3::ZERO, DVec3::X, RED).duration(f32::NAN);
        assert_eq!(d.timed_line_count(), 0);
        assert_eq!(d.line_count(), 2);
    }

    #[test]
    fn disabled_and_capped() {
        let mut d = DebugDraw {
            enabled: false,
            ..Default::default()
        };
        d.sphere(DVec3::ZERO, 1.0, RED).duration(1.0);
        assert!(d.is_empty());
        d.enabled = true;
        d.max_lines = 10;
        for _ in 0..15 {
            d.line(DVec3::ZERO, DVec3::X, RED);
        }
        assert_eq!(d.line_count(), 10);
        assert_eq!(d.dropped(), 5);
    }
}
