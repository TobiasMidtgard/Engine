//! Frame profiler: where each frame's time goes, per schedule and per system.
//!
//! Off unless the game is started with `--profile` or `ENGINE_PROFILE=1` (it must be on
//! before plugins add their systems, so [`crate::log::init`] decides). Then every few seconds
//! the engine logs, and writes to `engine-profile.txt` in the working directory, a table of
//! the schedules and the slowest systems in milliseconds per frame, plus how many cores the
//! frame kept busy. Timing comes from the ECS's own `system` and `schedule` tracing spans;
//! modules can add their own spans inside a system (any `tracing` span whose target starts
//! with `engine_`, e.g. `info_span!("shadow pass")` or `info_span!("render_step", name = "submit")`
//! in `engine_render`; a `name` field, when there is one, names the row), listed separately.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use parking_lot::Mutex;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Filter, Layer};
use tracing_subscriber::registry::LookupSpan;

static ENABLED: AtomicBool = AtomicBool::new(false);
static TOTALS: OnceLock<Mutex<HashMap<String, Stat>>> = OnceLock::new();

/// Seconds between reports.
const REPORT_SECS: f64 = 5.0;
/// Systems listed per report.
const TOP_SYSTEMS: usize = 40;

#[derive(Clone, Copy, Default)]
struct Stat {
    total: Duration,
    calls: u64,
    max: Duration,
}

/// Whether the profiler is recording.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `--profile` on the command line or `ENGINE_PROFILE` set (to anything but `0`).
pub(crate) fn requested() -> bool {
    std::env::args().any(|a| a == "--profile")
        || std::env::var("ENGINE_PROFILE").is_ok_and(|v| !v.is_empty() && v != "0")
}

pub(crate) fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

fn totals() -> &'static Mutex<HashMap<String, Stat>> {
    TOTALS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Which spans the profiler times: the ECS's per-system and per-schedule spans.
fn timed(meta: &tracing::Metadata<'_>) -> bool {
    if !meta.is_span() {
        return false;
    }
    if meta.target().starts_with("bevy_ecs") {
        matches!(meta.name(), "system" | "system_commands" | "schedule")
    } else {
        // A module's own spans inside its systems (render passes, sim stages).
        meta.target().starts_with("engine_")
    }
}

/// Keys of module spans, which run inside systems and so are not added to the busy time.
const INNER: &str = "  in ";

/// Per-layer filter: only the timed spans, and only while profiling.
pub(crate) struct ProfileFilter;

impl<S> Filter<S> for ProfileFilter {
    fn enabled(&self, meta: &tracing::Metadata<'_>, _: &Context<'_, S>) -> bool {
        enabled() && timed(meta)
    }

    /// Every timed span is info level (bevy's system and schedule spans, the engine's own), so
    /// say so: without a hint the global max level stays TRACE and every `log::trace!` (wgpu logs
    /// each render pass command) goes through the dispatcher before a filter drops it.
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::INFO)
    }
}

/// The key a span's time is filed under, kept in the span's extensions.
struct Key(String);
/// When the span was entered.
struct Entered(Instant);

struct NameVisitor(Option<String>);

impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

pub(crate) struct ProfileLayer;

impl<S> Layer<S> for ProfileLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = NameVisitor(None);
        attrs.record(&mut visitor);
        let name = visitor.0.unwrap_or_default();
        let meta = attrs.metadata();
        let key = if !meta.target().starts_with("bevy_ecs") {
            let module = meta.target().split("::").next().unwrap_or("");
            // A `name` field tells apart spans that share one span name ("render_step").
            if name.is_empty() {
                format!("{INNER}{module}: {}", meta.name())
            } else {
                format!("{INNER}{module}: {name}")
            }
        } else {
            match meta.name() {
                "schedule" => format!("schedule {name}"),
                "system_commands" => format!("{} (commands)", short_name(&name)),
                _ => short_name(&name),
            }
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(Key(key));
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().replace(Entered(Instant::now()));
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut ext = span.extensions_mut();
        let Some(Entered(start)) = ext.remove::<Entered>() else {
            return;
        };
        let elapsed = start.elapsed();
        let Some(Key(key)) = ext.get_mut::<Key>() else {
            return;
        };
        let mut totals = totals().lock();
        let stat = match totals.get_mut(key.as_str()) {
            Some(stat) => stat,
            None => totals.entry(key.clone()).or_default(),
        };
        stat.total += elapsed;
        stat.calls += 1;
        stat.max = stat.max.max(elapsed);
    }
}

/// `crate_a::module::system_fn` → `crate_a::system_fn`, keeping generic arguments readable.
fn short_name(name: &str) -> String {
    let base = name.split('<').next().unwrap_or(name);
    let mut parts = base.rsplit("::");
    let last = parts.next().unwrap_or(base);
    let first = base.split("::").next().unwrap_or("");
    let rest = &name[base.len()..];
    if first.is_empty() || first == last {
        format!("{last}{rest}")
    } else {
        format!("{first}::{last}{rest}")
    }
}

/// The last second's profile, for live views such as the debug overlay. Updated about once a
/// second while profiling; empty otherwise.
#[derive(Resource, Debug, Clone, Default, serde::Serialize)]
pub struct ProfileSnapshot {
    /// Real milliseconds per frame over the window.
    pub frame_ms: f64,
    /// Milliseconds per frame of all systems together, across threads.
    pub busy_ms: f64,
    /// `(schedule, ms per frame)`, slowest first.
    pub schedules: Vec<(String, f64)>,
    /// `(system, ms per frame)`, slowest first (at most [`SNAPSHOT_SYSTEMS`]).
    pub systems: Vec<(String, f64)>,
}

/// Systems kept in a [`ProfileSnapshot`].
pub const SNAPSHOT_SYSTEMS: usize = 16;
/// Seconds per [`ProfileSnapshot`].
const SNAPSHOT_SECS: f64 = 1.0;

/// Frames and wall time since the last report, and since the last snapshot.
#[derive(Resource)]
pub(crate) struct ProfileWindow {
    started: Instant,
    frames: u64,
    frame_max: Duration,
    last_frame: Instant,
    /// Totals of the snapshots since the last report.
    report: HashMap<String, Stat>,
    snapshot_started: Instant,
    snapshot_frames: u64,
}

impl Default for ProfileWindow {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            frames: 0,
            frame_max: Duration::ZERO,
            last_frame: now,
            report: HashMap::new(),
            snapshot_started: now,
            snapshot_frames: 0,
        }
    }
}

/// Engine system (`Last`): count frames, publish a [`ProfileSnapshot`] every second and write
/// a report every few seconds.
pub(crate) fn profile_report_system(
    mut window: ResMut<ProfileWindow>,
    mut snapshot: ResMut<ProfileSnapshot>,
) {
    let now = Instant::now();
    let frame = now - window.last_frame;
    window.last_frame = now;
    window.frames += 1;
    window.snapshot_frames += 1;
    window.frame_max = window.frame_max.max(frame);
    let since_snapshot = now - window.snapshot_started;
    if since_snapshot.as_secs_f64() >= SNAPSHOT_SECS {
        let stats = std::mem::take(&mut *totals().lock());
        *snapshot = make_snapshot(&stats, window.snapshot_frames, since_snapshot);
        for (key, stat) in stats {
            let total = window.report.entry(key).or_default();
            total.total += stat.total;
            total.calls += stat.calls;
            total.max = total.max.max(stat.max);
        }
        window.snapshot_started = now;
        window.snapshot_frames = 0;
    }
    let wall = now - window.started;
    if wall.as_secs_f64() < REPORT_SECS {
        return;
    }
    let report = format_report(&window.report, window.frames, wall, window.frame_max);
    crate::log::info!("frame profile\n{report}");
    if let Err(e) = std::fs::write("engine-profile.txt", &report) {
        crate::log::warn!("could not write engine-profile.txt: {e}");
    }
    *window = ProfileWindow::default();
}

fn make_snapshot(stats: &HashMap<String, Stat>, frames: u64, wall: Duration) -> ProfileSnapshot {
    let frames = frames.max(1) as f64;
    let per_frame = |d: Duration| d.as_secs_f64() * 1000.0 / frames;
    let mut schedules = Vec::new();
    let mut systems = Vec::new();
    let mut busy = Duration::ZERO;
    for (key, stat) in stats {
        if let Some(name) = key.strip_prefix("schedule ") {
            schedules.push((name.to_string(), per_frame(stat.total)));
        } else if !key.starts_with(INNER) {
            busy += stat.total;
            systems.push((key.clone(), per_frame(stat.total)));
        }
    }
    let slowest_first = |a: &(String, f64), b: &(String, f64)| b.1.total_cmp(&a.1);
    schedules.sort_by(slowest_first);
    systems.sort_by(slowest_first);
    systems.truncate(SNAPSHOT_SYSTEMS);
    ProfileSnapshot {
        frame_ms: per_frame(wall),
        busy_ms: per_frame(busy),
        schedules,
        systems,
    }
}

fn format_report(
    stats: &HashMap<String, Stat>,
    frames: u64,
    wall: Duration,
    frame_max: Duration,
) -> String {
    let frames = frames.max(1) as f64;
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let frame_ms = ms(wall) / frames;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{frames:.0} frames in {:.1} s: {frame_ms:.1} ms per frame ({:.1} fps), worst {:.1} ms",
        wall.as_secs_f64(),
        1000.0 / frame_ms.max(1e-6),
        ms(frame_max),
    );

    let mut schedules: Vec<_> = stats
        .iter()
        .filter(|(k, _)| k.starts_with("schedule "))
        .collect();
    schedules.sort_by_key(|a| std::cmp::Reverse(a.1.total));
    let mut systems: Vec<_> = stats
        .iter()
        .filter(|(k, _)| !k.starts_with("schedule ") && !k.starts_with(INNER))
        .collect();
    let mut inner: Vec<_> = stats.iter().filter(|(k, _)| k.starts_with(INNER)).collect();
    inner.sort_by_key(|a| std::cmp::Reverse(a.1.total));
    systems.sort_by_key(|a| std::cmp::Reverse(a.1.total));

    let busy: Duration = systems.iter().map(|(_, s)| s.total).sum();
    let busy_ms = ms(busy) / frames;
    let _ = writeln!(
        out,
        "systems busy {busy_ms:.1} ms per frame: {:.2} of {threads} cores on average",
        busy_ms / frame_ms.max(1e-6),
    );

    let _ = writeln!(out, "\nschedule                       ms/frame  runs/frame");
    for (name, s) in &schedules {
        let _ = writeln!(
            out,
            "{:<30} {:>8.2}  {:>10.1}",
            name.trim_start_matches("schedule "),
            ms(s.total) / frames,
            s.calls as f64 / frames,
        );
    }

    let _ = writeln!(
        out,
        "\nslowest systems                                                   ms/frame   max ms  runs/frame"
    );
    for (name, s) in systems.iter().take(TOP_SYSTEMS) {
        let _ = writeln!(
            out,
            "{:<64} {:>9.2} {:>8.2} {:>11.1}",
            truncate(name, 64),
            ms(s.total) / frames,
            ms(s.max),
            s.calls as f64 / frames,
        );
    }
    if !inner.is_empty() {
        let _ = writeln!(
            out,
            "\ninside systems                                                    ms/frame   max ms  runs/frame"
        );
        for (name, s) in inner.iter().take(TOP_SYSTEMS) {
            let _ = writeln!(
                out,
                "{:<64} {:>9.2} {:>8.2} {:>11.1}",
                truncate(name, 64),
                ms(s.total) / frames,
                ms(s.max),
                s.calls as f64 / frames,
            );
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_keep_crate_and_function() {
        assert_eq!(
            short_name("engine_water::flow::step_flow_system"),
            "engine_water::step_flow_system"
        );
        assert_eq!(short_name("my_system"), "my_system");
        assert_eq!(
            short_name("engine_core::time::tick<engine_core::time::Real>"),
            "engine_core::tick<engine_core::time::Real>"
        );
    }

    #[test]
    fn snapshots_list_the_slowest_systems_per_frame() {
        let mut stats = HashMap::new();
        for (key, ms) in [
            ("schedule Update", 30),
            ("game::fast", 2),
            ("game::slow", 20),
            ("  in engine_render: submit", 9),
        ] {
            stats.insert(
                key.to_string(),
                Stat {
                    total: Duration::from_millis(ms * 10),
                    calls: 10,
                    max: Duration::from_millis(ms),
                },
            );
        }
        let snap = make_snapshot(&stats, 10, Duration::from_millis(400));
        assert!((snap.frame_ms - 40.0).abs() < 1e-9);
        assert!((snap.busy_ms - 22.0).abs() < 1e-9);
        assert_eq!(snap.schedules, vec![("Update".to_string(), 30.0)]);
        assert_eq!(snap.systems[0], ("game::slow".to_string(), 20.0));
        assert_eq!(snap.systems.len(), 2);
    }

    #[test]
    fn report_lists_schedules_and_systems_per_frame() {
        let mut stats = HashMap::new();
        stats.insert(
            "schedule Update".to_string(),
            Stat {
                total: Duration::from_millis(100),
                calls: 10,
                max: Duration::from_millis(12),
            },
        );
        stats.insert(
            "game::slow".to_string(),
            Stat {
                total: Duration::from_millis(80),
                calls: 10,
                max: Duration::from_millis(9),
            },
        );
        stats.insert(
            format!("{INNER}engine_render: shadow pass"),
            Stat {
                total: Duration::from_millis(50),
                calls: 10,
                max: Duration::from_millis(6),
            },
        );
        let report = format_report(
            &stats,
            10,
            Duration::from_secs(1),
            Duration::from_millis(150),
        );
        assert!(report.contains("100.0 ms per frame"), "{report}");
        assert!(report.contains("Update"), "{report}");
        assert!(report.contains("game::slow"), "{report}");
        assert!(report.contains("8.00"), "{report}");
        assert!(report.contains("systems busy 8.0 ms"), "{report}");
        assert!(report.contains("engine_render: shadow pass"), "{report}");
    }
}
