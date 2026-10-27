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
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// Rows of the "ran alone" list per report.
const ALONE_ROWS: usize = 20;

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
    start_watchdog();
}

/// Frames finished so far (bumped at the end of every `run_frame`), for the hang watchdog.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// A frame finished.
pub(crate) fn frame_done() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// A frame stuck this long is reported as a hang.
const HANG_SECS: u64 = 20;
/// While it stays stuck, the report repeats this often.
const HANG_REPEAT_SECS: u64 = 60;

/// One thread's open (entered, not yet exited) timed spans, innermost last.
type OpenSpans = Arc<Mutex<Vec<(String, Instant)>>>;

/// Every thread that has entered a timed span, with its open spans.
fn threads() -> &'static Mutex<Vec<(String, OpenSpans)>> {
    static THREADS: OnceLock<Mutex<Vec<(String, OpenSpans)>>> = OnceLock::new();
    THREADS.get_or_init(|| Mutex::new(Vec::new()))
}

thread_local! {
    static OPEN: OpenSpans = {
        let open = OpenSpans::default();
        let thread = std::thread::current();
        let name = thread.name().map_or_else(|| format!("{:?}", thread.id()), str::to_string);
        threads().lock().push((name, open.clone()));
        open
    };
}

/// Watch for frames that never finish (a deadlock or an endless loop) and log what every
/// thread was inside when it happened, so a hang on a player's machine names its system
/// without a debugger. Profiling only: open spans are tracked only while it records.
fn start_watchdog() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("engine hang watchdog".into())
        .spawn(|| {
            let mut last = (FRAMES.load(Ordering::Relaxed), Instant::now());
            let mut reported: Option<Instant> = None;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let frames = FRAMES.load(Ordering::Relaxed);
                if frames != last.0 {
                    last = (frames, Instant::now());
                    reported = None;
                    continue;
                }
                let stuck = last.1.elapsed();
                let due = match reported {
                    None => stuck.as_secs() >= HANG_SECS,
                    Some(at) => at.elapsed().as_secs() >= HANG_REPEAT_SECS,
                };
                if due {
                    tracing::error!("{}", hang_report(frames, stuck));
                    reported = Some(Instant::now());
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("no hang watchdog ({e})");
    }
}

/// "Frame N has not finished for S s" and each busy thread's open spans, outermost first.
fn hang_report(frames: u64, stuck: Duration) -> String {
    let mut out = format!(
        "frame {} has not finished for {:.0} s (a hang); open spans per thread:",
        frames + 1,
        stuck.as_secs_f64()
    );
    let now = Instant::now();
    for (name, open) in threads().lock().iter() {
        let open = open.lock();
        if open.is_empty() {
            continue;
        }
        let spans: Vec<String> = open
            .iter()
            .map(|(key, at)| {
                format!(
                    "{} ({:.1} s)",
                    key.trim_start_matches(INNER),
                    (now - *at).as_secs_f64()
                )
            })
            .collect();
        let _ = write!(out, "\n  {name}: {}", spans.join(" > "));
    }
    out
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
/// Keys of the time a system ran with no other system running at the same moment: the serial
/// part of the frame, where the other cores waited (Amdahl's law).
const ALONE: &str = "  alone ";
/// Key of the time no system ran at all (executor hand-offs, frame pacing, work outside systems).
const IDLE: &str = "  idle";

/// Systems running right now, across threads, to file the time each one ran alone.
struct Running {
    keys: Vec<String>,
    /// When the set last changed.
    since: Instant,
}

fn running() -> &'static Mutex<Running> {
    static RUNNING: OnceLock<Mutex<Running>> = OnceLock::new();
    RUNNING.get_or_init(|| {
        Mutex::new(Running {
            keys: Vec::new(),
            since: Instant::now(),
        })
    })
}

/// A system span's key (not a schedule's or a module's span inside a system).
fn is_system(key: &str) -> bool {
    !key.starts_with(INNER) && !key.starts_with("schedule ")
}

/// A system span `key` starts (`enter`) or ends: file the time since the running set last
/// changed as one system's time alone, or as idle, then update the set.
fn system_moved(key: &str, enter: bool, now: Instant) {
    let mut run = running().lock();
    let span = now.saturating_duration_since(run.since);
    let credit = match run.keys.as_slice() {
        [] => Some(IDLE.to_string()),
        [only] => Some(format!("{ALONE}{only}")),
        _ => None,
    };
    if let Some(credit) = credit {
        let mut totals = totals().lock();
        totals.entry(credit).or_default().total += span;
    }
    if enter {
        run.keys.push(key.to_string());
    } else if let Some(i) = run.keys.iter().rposition(|k| k == key) {
        run.keys.swap_remove(i);
    }
    run.since = now;
}

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
            let now = Instant::now();
            let mut ext = span.extensions_mut();
            if let Some(Key(key)) = ext.get_mut::<Key>() {
                let key = key.clone();
                if is_system(&key) {
                    system_moved(&key, true, now);
                }
                OPEN.with(|open| open.lock().push((key, now)));
            }
            ext.replace(Entered(now));
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
        OPEN.with(|open| {
            let mut open = open.lock();
            if let Some(i) = open.iter().rposition(|(k, _)| k == key) {
                open.remove(i);
            }
        });
        if is_system(key) {
            system_moved(key, false, start + elapsed);
        }
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
        } else if is_system(key) && !key.starts_with(ALONE) && key != IDLE {
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
        .filter(|(k, _)| is_system(k) && !k.starts_with(ALONE) && k.as_str() != IDLE)
        .collect();
    let mut alone: Vec<_> = stats
        .iter()
        .filter_map(|(k, s)| k.strip_prefix(ALONE).map(|name| (name, s.total)))
        .collect();
    alone.sort_by_key(|a| std::cmp::Reverse(a.1));
    let alone_ms = ms(alone.iter().map(|(_, d)| *d).sum()) / frames;
    let idle_ms = stats.get(IDLE).map_or(0.0, |s| ms(s.total) / frames);
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
    let _ = writeln!(
        out,
        "one system at a time {alone_ms:.1} ms per frame, no system {idle_ms:.1} ms, \
         several at once {:.1} ms",
        (frame_ms - alone_ms - idle_ms).max(0.0),
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
    if !alone.is_empty() {
        let _ = writeln!(
            out,
            "\nran alone (nothing else running)                                  ms/frame"
        );
        for (name, total) in alone.iter().take(ALONE_ROWS) {
            let _ = writeln!(
                out,
                "{:<64} {:>9.2}",
                truncate(name, 64),
                ms(*total) / frames
            );
        }
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

/// Frames before [`parallelism_report_system`] reads the schedules (they are built on first run).
const PARALLELISM_REPORT_FRAME: u32 = 30;

/// Engine system (`Last`, profiling only): once, log why systems of each schedule can't run
/// side by side: the exclusive (`&mut World`) systems, which run alone by construction, and the
/// components and resources most system pairs both touch with one of them writing, which the
/// executor must run one after the other (Bevy's access-conflict rule, as in its ambiguity
/// checker). Ordering constraints serialize too and are not counted.
pub(crate) fn parallelism_report_system(world: &mut World, mut frames: Local<u32>) {
    *frames += 1;
    if *frames != PARALLELISM_REPORT_FRAME {
        return;
    }
    let report = parallelism_report(world);
    crate::log::info!("schedule parallelism\n{report}");
}

/// Rows of the conflict list per schedule.
const CONFLICT_ROWS: usize = 8;

pub(crate) fn parallelism_report(world: &World) -> String {
    use bevy_ecs::query::AccessConflicts;
    use bevy_ecs::schedule::Schedules;
    let mut out = String::new();
    let Some(schedules) = world.get_resource::<Schedules>() else {
        return out;
    };
    let mut labels: Vec<_> = schedules.iter().collect();
    labels.sort_by_key(|(label, _)| format!("{label:?}"));
    for (label, schedule) in labels {
        let Ok(systems) = schedule.systems_with_access() else {
            continue;
        };
        let systems: Vec<_> = systems
            .map(|(_, s)| (short_name(&s.system().name().to_string()), s.access()))
            .collect();
        if systems.len() < 2 {
            continue;
        }
        let exclusive: Vec<&str> = systems
            .iter()
            .filter(|(_, a)| a.is_exclusive())
            .map(|(n, _)| n.as_str())
            .collect();
        // Per contested component or resource: the pairs it serializes and the systems in them.
        let mut contested: HashMap<bevy_ecs::component::ComponentId, (usize, Vec<usize>)> =
            HashMap::new();
        let (mut pairs, mut conflicting) = (0usize, 0usize);
        for i in 0..systems.len() {
            for j in i + 1..systems.len() {
                let (a, b) = (systems[i].1, systems[j].1);
                if a.is_exclusive() || b.is_exclusive() {
                    continue;
                }
                pairs += 1;
                match a.get_conflicts(b) {
                    AccessConflicts::All => conflicting += 1,
                    AccessConflicts::Individual(ids) => {
                        if ids.is_clear() {
                            continue;
                        }
                        conflicting += 1;
                        for id in ids.iter() {
                            let entry = contested.entry(id).or_default();
                            entry.0 += 1;
                            for k in [i, j] {
                                if !entry.1.contains(&k) {
                                    entry.1.push(k);
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = writeln!(
            out,
            "{label:?}: {} systems, {} exclusive; {conflicting} of {pairs} other pairs can't run together",
            systems.len(),
            exclusive.len(),
        );
        if !exclusive.is_empty() {
            let _ = writeln!(out, "  exclusive: {}", exclusive.join(", "));
        }
        let mut contested: Vec<_> = contested.into_iter().collect();
        contested.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
        for (id, (n, users)) in contested.into_iter().take(CONFLICT_ROWS) {
            let name = world
                .components()
                .get_name(id)
                .map_or_else(|| format!("{id:?}"), |n| n.shortname().to_string());
            let mut names: Vec<&str> = users.iter().map(|&k| systems[k].0.as_str()).collect();
            names.sort_unstable();
            let shown = names.len().min(6);
            let more = names.len() - shown;
            let _ = writeln!(
                out,
                "  {name}: {n} pairs, {} systems ({}{})",
                names.len(),
                names[..shown].join(", "),
                if more > 0 {
                    format!(", +{more}")
                } else {
                    String::new()
                },
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
    fn hang_reports_name_each_threads_open_spans() {
        use tracing_subscriber::layer::SubscriberExt;
        let (entered, wait) = std::sync::mpsc::channel();
        let (release, done) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::Builder::new()
            .name("stuck worker".into())
            .spawn(move || {
                let subscriber = tracing_subscriber::registry().with(ProfileLayer);
                tracing::subscriber::with_default(subscriber, || {
                    let _outer = tracing::info_span!("render_step", name = "upload").entered();
                    let _inner = tracing::info_span!("wait for slot").entered();
                    entered.send(()).unwrap();
                    done.recv().unwrap();
                });
            })
            .unwrap();
        wait.recv().unwrap();
        let report = hang_report(41, Duration::from_secs(25));
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            report.starts_with("frame 42 has not finished for 25 s"),
            "{report}"
        );
        let line = report
            .lines()
            .find(|l| l.trim_start().starts_with("stuck worker:"))
            .unwrap_or_else(|| panic!("no worker line in {report}"));
        assert!(
            line.contains("engine_core: upload (")
                && line.contains(" > engine_core: wait for slot ("),
            "{line}"
        );
        // Exited spans leave the thread's list.
        assert!(!hang_report(0, Duration::ZERO).contains("wait for slot"));
    }

    #[test]
    fn systems_running_alone_are_told_apart_from_systems_running_together() {
        use std::sync::Barrier;
        use tracing_subscriber::layer::SubscriberExt;
        fn system(name: &str, hold: impl FnOnce()) {
            let subscriber = tracing_subscriber::registry().with(ProfileLayer);
            tracing::subscriber::with_default(subscriber, || {
                let _span =
                    tracing::info_span!(target: "bevy_ecs::system", "system", name).entered();
                hold();
            });
        }
        let pause = Duration::from_millis(60);
        system("alone_test::serial", || std::thread::sleep(pause));
        let both_in = Arc::new(Barrier::new(2));
        let both_done = Arc::new(Barrier::new(2));
        let workers: Vec<_> = ["alone_test::left", "alone_test::right"]
            .into_iter()
            .map(|name| {
                let (both_in, both_done) = (both_in.clone(), both_done.clone());
                std::thread::spawn(move || {
                    system(name, || {
                        both_in.wait();
                        std::thread::sleep(pause);
                        both_done.wait();
                    })
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        let totals = totals().lock();
        let alone = |name: &str| {
            totals
                .get(&format!("{ALONE}{name}"))
                .map_or(Duration::ZERO, |s| s.total)
        };
        assert!(
            alone("alone_test::serial") >= pause,
            "{:?}",
            alone("alone_test::serial")
        );
        let together = alone("alone_test::left") + alone("alone_test::right");
        assert!(
            together < pause / 2,
            "left and right ran alone {together:?}"
        );
        // Both still count as busy systems.
        assert!(totals["alone_test::left"].total >= pause);
    }

    #[test]
    fn parallelism_report_names_exclusive_systems_and_contested_data() {
        use bevy_ecs::schedule::{Schedule, ScheduleLabel, Schedules};
        #[derive(Resource, Default)]
        struct Shared(u32);
        #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
        struct Probe;
        fn writer_a(mut s: ResMut<Shared>) {
            s.0 += 1;
        }
        fn writer_b(mut s: ResMut<Shared>) {
            s.0 += 2;
        }
        fn reader(_s: Res<Shared>) {}
        fn alone(_world: &mut World) {}
        let mut world = World::new();
        world.init_resource::<Shared>();
        let mut schedule = Schedule::new(Probe);
        schedule.add_systems((writer_a, writer_b, reader, alone));
        schedule.run(&mut world);
        let mut schedules = Schedules::default();
        schedules.insert(schedule);
        world.insert_resource(schedules);
        let report = parallelism_report(&world);
        assert!(
            report.contains("Probe: 4 systems, 1 exclusive; 3 of 3 other pairs"),
            "{report}"
        );
        assert!(report.contains("exclusive: engine_core::alone"), "{report}");
        assert!(
            report
                .contains("Shared: 3 pairs, 3 systems (engine_core::reader, engine_core::writer_a"),
            "{report}"
        );
    }

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

    #[test]
    fn report_splits_the_frame_into_serial_idle_and_parallel_time() {
        let mut stats = HashMap::new();
        for (key, ms) in [
            ("game::slow".to_string(), 80),
            (format!("{ALONE}game::slow"), 50),
            (IDLE.to_string(), 10),
        ] {
            stats.insert(
                key,
                Stat {
                    total: Duration::from_millis(ms),
                    calls: 10,
                    max: Duration::from_millis(9),
                },
            );
        }
        let report = format_report(&stats, 10, Duration::from_secs(1), Duration::ZERO);
        assert!(
            report.contains(
                "one system at a time 5.0 ms per frame, no system 1.0 ms, several at once 94.0 ms"
            ),
            "{report}"
        );
        assert!(report.contains("ran alone"), "{report}");
        // Alone and idle time is not counted again as busy.
        assert!(report.contains("systems busy 8.0 ms"), "{report}");
    }
}
