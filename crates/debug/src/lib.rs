//! # engine_debug
//!
//! Debug drawing for every module, game and AI agent: lines and wireframe shapes in the
//! world, drawn over the scene with the renderer's camera-relative precision.
//!
//! * [`DebugDraw`]: immediate-mode resource. Any system adds shapes; they are drawn this
//!   frame, or for a [duration](ShapeHandle::duration) in real seconds (still counting while
//!   the world is paused or slowed), depth-tested or [on top](ShapeHandle::on_top).
//! * Shapes: line, ray, arrow, aabb, oriented box, sphere, circle, cross/point, axes, capsule,
//!   cylinder, grid, frustum, polyline/polygon. Positions are `f64` world meters.
//! * [`DebugViews`]: built-in views of the `engine_world` contracts (wind, ground wetness and
//!   snow, focus points, gravity, world events). Off by default.
//! * [`PerfOverlay`]: an on-screen performance panel (F3): fps, a frame-time graph, GPU time
//!   per render pass, render CPU per step, VRAM and, with `--profile`, the slowest systems.
//! * API: `debug.draw`, `debug.clear`, `debug.views` (JSON in/out), so AI agents can
//!   annotate the world and then look at it with `render.screenshot`.
//!
//! ```ignore
//! use engine_debug::prelude::*;
//!
//! app.add_plugins(DebugPlugin);
//!
//! fn show_path(mut debug: ResMut<DebugDraw>, q: Query<(&GlobalTransform, &Path)>) {
//!     for (t, path) in &q {
//!         debug.polyline(path.points.iter().copied(), CYAN);
//!         debug.axes_global(t, 0.5);
//!     }
//!     debug.sphere(DVec3::new(0.0, 1.0, 0.0), 0.5, RED).duration(2.0).on_top();
//! }
//! ```
//!
//! Colors are linear RGBA written as display values (not exposed like physical light), so
//! a debug line looks the same in daylight and at night.

pub mod api;
pub mod color;
pub mod draw;
pub mod feature;
pub mod overlay;
pub mod shapes;
mod shift_audit;
mod stream_audit;
pub use stream_audit::{AuditedCell, StreamAuditPlugin, StreamAuditReport};
pub mod views;

pub use color::Color;
pub use draw::{DebugDraw, DebugLine, DepthMode, ShapeHandle};
pub use feature::DebugLinesFeature;
pub use overlay::{FrameHistory, PerfOverlay, PerfOverlayFeature};
pub use shapes::{DebugShape, POINT_SIZE, ShapeStyle};
pub use views::{DebugViews, EventViews, SurfaceView, WindView};

use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_world::{GroundContact, NoiseEvent, ProjectileTrace, WorldImpulse};

/// Everything needed to draw debug shapes, including the color constants.
pub mod prelude {
    pub use crate::color::{
        BLACK, BLUE, CYAN, Color, GRAY, GREEN, LIME, MAGENTA, ORANGE, PINK, PURPLE, RED, WHITE,
        YELLOW,
    };
    pub use crate::{
        DebugDraw, DebugPlugin, DebugShape, DebugViews, DepthMode, PerfOverlay, ShapeStyle,
    };
}

/// Adds [`DebugDraw`], [`DebugViews`], the line render feature and the `debug.*` API.
///
/// Works with or without `WorldPlugin` (views of missing world resources are skipped) and
/// without a GPU (shapes are recorded and expire; nothing is drawn).
pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugDraw>()
            .init_resource::<DebugViews>()
            .init_resource::<PerfOverlay>()
            .init_resource::<FrameHistory>()
            .register_resource::<PerfOverlay>(
                "Performance panel (F3): enabled, scale (pixels per font pixel, 0 = auto).",
            )
            .register_resource::<DebugViews>(
                "Debug drawing switches: enabled (master switch for all debug lines), wind \
                 {enabled, radius, spacing, height, scale (m per m/s), max_speed}, surface \
                 {enabled, radius, spacing} (wetness/puddles/snow/scorch points), focus, gravity, \
                 events {projectiles, impulses, ground_contacts, noise, duration (real s)}.",
            )
            // The event views read these; registering is idempotent with WorldPlugin.
            .add_message::<ProjectileTrace>()
            .add_message::<WorldImpulse>()
            .add_message::<GroundContact>()
            .add_message::<NoiseEvent>()
            .add_systems(
                First,
                (begin_frame_system, overlay::perf_overlay_system)
                    .after(engine_core::time::time_system),
            )
            .add_systems(
                Extract,
                (views::environment_views_system, views::event_views_system).chain(),
            )
            .add_systems(
                Render,
                end_frame_system.after(engine_render::renderer::render_system),
            )
            .add_render_feature(DebugLinesFeature::default())
            .add_render_feature(PerfOverlayFeature::default());
        api::register(app);
        shift_audit::install(app);
        stream_audit::install(app);
    }
}

/// `First`: start the debug frame on the real clock (drops the one-frame shapes the last
/// render drew and expired timed shapes) and apply the master switch.
pub fn begin_frame_system(time: Res<Time>, views: Res<DebugViews>, mut draw: ResMut<DebugDraw>) {
    draw.enabled = views.enabled;
    draw.begin_frame(time.real_elapsed_secs_f64());
}

/// `Render`, after the renderer: mark this frame's one-frame shapes as drawn, so shapes added
/// later (in `Last` or between frames) are kept for the next frame. Runs with or without a
/// GPU.
pub fn end_frame_system(mut draw: ResMut<DebugDraw>) {
    draw.end_frame();
}

#[cfg(test)]
mod tests {
    use super::prelude::*;
    use engine_core::math::{DVec3, Vec3};
    use engine_core::prelude::*;
    use engine_core::time::TimeSource;
    use engine_world::{FocusPoints, SimulationFocus, SurfaceLayers, WorldImpulse, WorldPlugin};

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs: 0.1 });
        app.add_plugins((WorldPlugin, DebugPlugin));
        app
    }

    #[derive(Resource, Default)]
    struct Seen(usize);

    #[test]
    fn shapes_from_update_are_visible_until_the_next_frame() {
        let mut app = app();
        app.add_systems(Update, |mut d: ResMut<DebugDraw>| {
            d.sphere(DVec3::ZERO, 1.0, RED);
        });
        app.init_resource::<Seen>();
        // Render runs after Update: it sees this frame's shapes.
        app.add_systems(Render, |d: Res<DebugDraw>, mut seen: ResMut<Seen>| {
            seen.0 = d.line_count();
        });
        app.update_n(3);
        assert_eq!(
            app.world.resource::<Seen>().0,
            96,
            "drawn once per frame, not accumulated"
        );
        // Still there between frames (screenshots taken after update see them).
        assert_eq!(app.world.resource::<DebugDraw>().line_count(), 96);
    }

    #[test]
    fn frame_shapes_added_after_render_show_in_the_next_frame() {
        let mut app = app();
        app.init_resource::<Seen>();
        app.add_systems(Render, |d: Res<DebugDraw>, mut seen: ResMut<Seen>| {
            seen.0 = d.line_count();
        });
        // A system in `Last` (after Render) draws every frame: its shapes show one frame
        // later and do not pile up.
        app.add_systems(Last, |mut d: ResMut<DebugDraw>| {
            d.cross(DVec3::ZERO, 1.0, RED);
        });
        app.update();
        assert_eq!(app.world.resource::<Seen>().0, 0);
        app.update_n(3);
        assert_eq!(app.world.resource::<Seen>().0, 3, "drawn, not accumulated");
    }

    #[test]
    fn one_frame_api_shapes_drawn_between_frames_are_rendered_once() {
        let mut app = app();
        app.init_resource::<Seen>();
        app.add_systems(Render, |d: Res<DebugDraw>, mut seen: ResMut<Seen>| {
            seen.0 = d.line_count();
        });
        app.update();
        // An AI agent calls the API between frames.
        engine_core::registry::call_api(
            &mut app.world,
            "debug.draw",
            serde_json::json!({"line": {"a": [0, 0, 0], "b": [1, 0, 0], "duration": 0}}),
        )
        .unwrap();
        app.update();
        assert_eq!(
            app.world.resource::<Seen>().0,
            1,
            "rendered in the next frame"
        );
        app.update();
        assert_eq!(app.world.resource::<Seen>().0, 0, "and only once");
    }

    #[test]
    fn durations_use_real_time_even_when_paused_or_dilated() {
        let mut app = app();
        app.update();
        let world_time = app.world.resource::<Time>().elapsed_secs();
        app.world.resource_mut::<Time>().pause();
        app.world
            .resource_mut::<TimeDilation>()
            .hold("slowmo", 0.05, 0.0);
        app.world
            .resource_mut::<DebugDraw>()
            .line(DVec3::ZERO, DVec3::X, RED)
            .duration(1.0);
        app.update_n(5); // 0.5 s real
        assert_eq!(app.world.resource::<DebugDraw>().line_count(), 1);
        app.update_n(6); // 1.1 s real
        assert!(app.world.resource::<DebugDraw>().is_empty());
        assert_eq!(
            app.world.resource::<Time>().elapsed_secs(),
            world_time,
            "world time stood still"
        );
    }

    #[test]
    fn views_draw_world_state_when_enabled() {
        let mut app = app();
        app.world.spawn((
            Transform::from_xyz(3.0, 0.0, 4.0),
            SimulationFocus::default(),
        ));
        app.update();
        assert!(
            app.world.resource::<DebugDraw>().is_empty(),
            "views are off by default"
        );

        {
            let mut v = app.world.resource_mut::<DebugViews>();
            v.wind.enabled = true;
            v.focus = true;
            v.gravity = true;
        }
        app.update();
        let draw = app.world.resource::<DebugDraw>();
        assert!(draw.line_count() > 100, "{}", draw.line_count());
        // Focus + gravity markers are drawn on top.
        assert_eq!(draw.lines(DepthMode::OnTop).count(), 32 + 1 + 3 + 5);
        let focus = app.world.resource::<FocusPoints>().primary();
        assert_eq!(focus, DVec3::new(3.0, 0.0, 4.0));

        // Surface: wet cells near the focus show up blue.
        app.world.resource_mut::<DebugViews>().set_all(false);
        app.world.resource_mut::<DebugViews>().surface.enabled = true;
        app.world
            .resource_mut::<SurfaceLayers>()
            .modify(DVec3::new(4.0, 0.0, 4.0), |c| c.wetness = 1.0);
        app.update();
        let draw = app.world.resource::<DebugDraw>();
        assert!(draw.line_count() > 50);
        let wet = draw
            .lines(DepthMode::Tested)
            .filter(|l| l.color[2] > 0.8 && l.color[0] < 0.2)
            .count();
        assert!(wet >= 2, "the wet cell is drawn blue ({wet})");
    }

    #[test]
    fn world_events_leave_short_lived_markers() {
        let mut app = app();
        app.world.resource_mut::<DebugViews>().events.impulses = true;
        app.update();
        app.world.write_message(WorldImpulse {
            center: DVec3::new(0.0, 1.0, 0.0),
            radius: 3.0,
            strength: 10.0,
            energy_j: 0.0,
            heat_c: 0.0,
            source: None,
        });
        app.update();
        let n = app.world.resource::<DebugDraw>().line_count();
        assert_eq!(n, 96 + 3, "sphere + cross");
        assert_eq!(app.world.resource::<DebugDraw>().timed_line_count(), n);
        app.update_n(5);
        assert_eq!(
            app.world.resource::<DebugDraw>().line_count(),
            n,
            "event read once, kept 1.5 s"
        );
        app.update_n(12);
        assert!(app.world.resource::<DebugDraw>().is_empty());
    }

    #[test]
    fn master_switch_hides_everything() {
        let mut app = app();
        app.world.resource_mut::<DebugViews>().enabled = false;
        app.add_systems(Update, |mut d: ResMut<DebugDraw>| {
            d.line(DVec3::ZERO, DVec3::Y, RED).duration(5.0);
            d.ray(DVec3::ZERO, Vec3::X, BLUE);
        });
        app.update_n(2);
        assert!(app.world.resource::<DebugDraw>().is_empty());
    }
}
