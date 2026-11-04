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
//!
//! `ENGINE_TRACE=<file.json>` also turns it on and writes every span of a few frames to that
//! file in the Chrome trace format (open it in <https://ui.perfetto.dev>, or summarize it with
//! `scripts/trace-summary.py`): `ENGINE_TRACE_AT` seconds after the first frame (default 30),
//! for `ENGINE_TRACE_FRAMES` frames (default 60). It shows, frame by frame, which system each
//! thread ran and where every thread waited, which the 5 s averages can't.

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
        || trace::config().is_some()
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
    trace::frame_done();
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
/// Keys of the system time each thread ran (`"  thread compute (3)"`), to see whether the
/// executor really spreads systems over the pool.
const THREAD: &str = "  thread ";

/// This thread's key under [`THREAD`], made once per thread.
fn thread_key() -> String {
    thread_local! {
        static KEY: String = format!(
            "{THREAD}{}",
            std::thread::current().name().unwrap_or("unnamed")
        );
    }
    KEY.with(Clone::clone)
}

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
    !key.starts_with(INNER) && !key.starts_with("schedule ") && !key.starts_with(THREAD)
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
        let mut totals = if is_system(key) {
            system_moved(key, false, start + elapsed);
            let mut totals = totals().lock();
            totals.entry(thread_key()).or_default().total += elapsed;
            totals
        } else {
            totals().lock()
        };
        let stat = match totals.get_mut(key.as_str()) {
            Some(stat) => stat,
            None => totals.entry(key.clone()).or_default(),
        };
        stat.total += elapsed;
        stat.calls += 1;
        stat.max = stat.max.max(elapsed);
        drop(totals);
        trace::span(key, start, elapsed);
    }
}

/// Every span of a few frames, for `ENGINE_TRACE` (see the module docs).
mod trace {
    use super::*;

    pub(super) struct Config {
        path: std::path::PathBuf,
        after: Duration,
        frames: u32,
    }

    pub(super) fn config() -> Option<&'static Config> {
        static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
        CONFIG
            .get_or_init(|| {
                let path = std::env::var_os("ENGINE_TRACE").filter(|p| !p.is_empty())?;
                let number = |name: &str, default: f64| {
                    std::env::var(name)
                        .ok()
                        .and_then(|v| v.trim().parse::<f64>().ok())
                        .filter(|v| v.is_finite() && *v >= 0.0)
                        .unwrap_or(default)
                };
                Some(Config {
                    path: path.into(),
                    after: Duration::from_secs_f64(number("ENGINE_TRACE_AT", 30.0)),
                    frames: number("ENGINE_TRACE_FRAMES", 60.0).max(1.0) as u32,
                })
            })
            .as_ref()
    }

    /// Recording now (checked on every span exit, so an atomic, not the lock).
    static RECORDING: AtomicBool = AtomicBool::new(false);

    struct Event {
        name: String,
        thread: u32,
        start: Instant,
        dur: Duration,
    }

    #[derive(Default)]
    struct Capture {
        first_frame: Option<Instant>,
        /// When each recorded frame ended; the first is when recording started.
        frame_ends: Vec<Instant>,
        events: Vec<Event>,
        done: bool,
    }

    fn capture() -> &'static Mutex<Capture> {
        static CAPTURE: OnceLock<Mutex<Capture>> = OnceLock::new();
        CAPTURE.get_or_init(Default::default)
    }

    fn thread_names() -> &'static Mutex<Vec<String>> {
        static NAMES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        NAMES.get_or_init(Default::default)
    }

    /// This thread's number in the trace, given on its first span.
    fn thread_number() -> u32 {
        thread_local! {
            static NUMBER: u32 = {
                let thread = std::thread::current();
                let name = thread.name().map_or_else(|| format!("{:?}", thread.id()), str::to_string);
                let mut names = thread_names().lock();
                names.push(name);
                names.len() as u32
            };
        }
        NUMBER.with(|n| *n)
    }

    pub(super) fn span(key: &str, start: Instant, dur: Duration) {
        if !RECORDING.load(Ordering::Relaxed) {
            return;
        }
        let event = Event {
            name: key.to_string(),
            thread: thread_number(),
            start,
            dur,
        };
        capture().lock().events.push(event);
    }

    pub(super) fn frame_done() {
        let Some(config) = config() else {
            return;
        };
        let now = Instant::now();
        let mut capture = capture().lock();
        if capture.done {
            return;
        }
        let first = *capture.first_frame.get_or_insert(now);
        if !RECORDING.load(Ordering::Relaxed) {
            if now.duration_since(first) >= config.after {
                capture.frame_ends.push(now);
                RECORDING.store(true, Ordering::Relaxed);
            }
            return;
        }
        capture.frame_ends.push(now);
        if capture.frame_ends.len() as u32 > config.frames {
            RECORDING.store(false, Ordering::Relaxed);
            capture.done = true;
            let taken = std::mem::take(&mut *capture);
            drop(capture);
            let names = thread_names().lock().clone();
            let path = config.path.clone();
            // Off the frame: tens of thousands of events take a moment to format.
            let spawned = std::thread::Builder::new()
                .name("engine trace writer".into())
                .spawn(move || match write(&path, &taken, &names) {
                    Ok(()) => tracing::info!(
                        "trace of {} frames written to {}",
                        taken.frame_ends.len() - 1,
                        path.display()
                    ),
                    Err(e) => tracing::warn!("trace not written to {}: {e}", path.display()),
                });
            if let Err(e) = spawned {
                tracing::warn!("trace not written ({e})");
            }
        }
    }

    /// The Chrome trace event format: complete (`X`) events in microseconds, a `frame` mark at
    /// the end of each frame, and the threads' names.
    fn write(path: &std::path::Path, capture: &Capture, names: &[String]) -> std::io::Result<()> {
        use std::io::Write;
        let Some(&origin) = capture.frame_ends.first() else {
            return Ok(());
        };
        let us = |at: Instant| {
            if at >= origin {
                at.duration_since(origin).as_secs_f64() * 1e6
            } else {
                -(origin.duration_since(at).as_secs_f64() * 1e6)
            }
        };
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        write!(out, "{{\"traceEvents\":[")?;
        let mut first = true;
        let mut sep = |out: &mut std::io::BufWriter<std::fs::File>| -> std::io::Result<()> {
            if !std::mem::take(&mut first) {
                writeln!(out, ",")?;
            }
            Ok(())
        };
        for (i, name) in names.iter().enumerate() {
            sep(&mut out)?;
            write!(
                out,
                "{{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":{},\"args\":{{\"name\":{}}}}}",
                i + 1,
                serde_json::to_string(name).unwrap_or_default()
            )?;
        }
        for (i, &end) in capture.frame_ends.iter().enumerate().skip(1) {
            sep(&mut out)?;
            write!(
                out,
                "{{\"ph\":\"i\",\"s\":\"g\",\"name\":\"frame {i}\",\"cat\":\"frame\",\"pid\":1,\"tid\":0,\"ts\":{:.1}}}",
                us(end)
            )?;
        }
        for e in &capture.events {
            let (cat, name) = if let Some(rest) = e.name.strip_prefix(INNER) {
                ("span", rest)
            } else if let Some(rest) = e.name.strip_prefix("schedule ") {
                ("schedule", rest)
            } else {
                ("system", e.name.as_str())
            };
            sep(&mut out)?;
            write!(
                out,
                "{{\"ph\":\"X\",\"cat\":\"{cat}\",\"name\":{},\"pid\":1,\"tid\":{},\"ts\":{:.1},\"dur\":{:.1}}}",
                serde_json::to_string(name).unwrap_or_default(),
                e.thread,
                us(e.start),
                e.dur.as_secs_f64() * 1e6
            )?;
        }
        write!(out, "]}}")?;
        out.flush()
    }

    #[cfg(test)]
    pub(super) fn write_for_test(
        path: &std::path::Path,
        frame_ends: Vec<Instant>,
        events: Vec<(String, u32, Instant, Duration)>,
        names: &[String],
    ) -> std::io::Result<()> {
        let capture = Capture {
            first_frame: frame_ends.first().copied(),
            frame_ends,
            events: events
                .into_iter()
                .map(|(name, thread, start, dur)| Event {
                    name,
                    thread,
                    start,
                    dur,
                })
                .collect(),
            done: true,
        };
        write(path, &capture, names)
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
    order: Option<Res<ScheduleOrder>>,
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
    let mut report = format_report(&window.report, window.frames, wall, window.frame_max);
    if let Some(order) = order {
        report.push_str(&format_floors(&ordering_floors(
            &order,
            &window.report,
            window.frames.max(1) as f64,
        )));
    }
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

    // Pool threads are named "compute (3)": group them, counting the threads that ran systems.
    let mut by_thread: Vec<(String, f64, usize)> = Vec::new();
    for (key, stat) in stats {
        let Some(name) = key.strip_prefix(THREAD) else {
            continue;
        };
        let group = name.split(" (").next().unwrap_or(name).to_string();
        let ms_frame = ms(stat.total) / frames;
        match by_thread.iter_mut().find(|(g, _, _)| *g == group) {
            Some((_, t, n)) => {
                *t += ms_frame;
                *n += 1;
            }
            None => by_thread.push((group, ms_frame, 1)),
        }
    }
    by_thread.sort_by(|a, b| b.1.total_cmp(&a.1));
    if !by_thread.is_empty() {
        let parts: Vec<String> = by_thread
            .iter()
            .map(|(g, t, n)| {
                if *n > 1 {
                    format!("{g} {t:.1} ms on {n} threads")
                } else {
                    format!("{g} {t:.1} ms")
                }
            })
            .collect();
        let _ = writeln!(out, "systems ran on: {}", parts.join(", "));
    }

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
/// Each schedule's systems in run order with the systems ordered after each one (indices into
/// the list), recorded when the schedule is built, so a report can find the chain of
/// `before`/`after`/`chain` edges that no number of cores can shorten.
#[derive(Resource, Default)]
pub(crate) struct ScheduleOrder(HashMap<String, Vec<(String, Vec<usize>)>>);

/// Build pass that records a schedule's flattened ordering into [`ScheduleOrder`]. Added after
/// the ECS's own sync point pass, so the `apply_deferred` points it inserts are included.
#[derive(Debug)]
pub(crate) struct OrderPass {
    pub(crate) label: String,
}

impl bevy_ecs::schedule::ScheduleBuildPass for OrderPass {
    type EdgeOptions = ();

    fn add_dependency(
        &mut self,
        _from: bevy_ecs::schedule::NodeId,
        _to: bevy_ecs::schedule::NodeId,
        _options: Option<&()>,
    ) {
    }

    fn collapse_set(
        &mut self,
        _set: bevy_ecs::schedule::SystemSetKey,
        _systems: &indexmap::IndexSet<
            bevy_ecs::schedule::SystemKey,
            bevy_platform::hash::FixedHasher,
        >,
        _dependency_flattening: &bevy_ecs::schedule::graph::DiGraph<bevy_ecs::schedule::NodeId>,
    ) -> impl Iterator<Item = (bevy_ecs::schedule::NodeId, bevy_ecs::schedule::NodeId)> {
        std::iter::empty()
    }

    fn build(
        &mut self,
        world: &mut World,
        graph: &mut bevy_ecs::schedule::ScheduleGraph,
        mut dependency_flattened: bevy_ecs::schedule::FlattenedDependencies<'_>,
    ) -> Result<(), bevy_ecs::schedule::ScheduleBuildError> {
        // A cycle is reported by the schedule's own build right after; record nothing then.
        let Ok((order, edges)) = dependency_flattened.toposort_and_graph() else {
            return Ok(());
        };
        let index: HashMap<_, _> = order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let list = order
            .iter()
            .map(|&key| {
                let name = graph
                    .systems
                    .get(key)
                    .map_or_else(String::new, |s| short_name(&s.system().name().to_string()));
                let after = edges
                    .neighbors(key)
                    .filter_map(|k| index.get(&k).copied())
                    .collect();
                (name, after)
            })
            .collect();
        world
            .get_resource_or_init::<ScheduleOrder>()
            .0
            .insert(self.label.clone(), list);
        Ok(())
    }
}

/// A schedule's ordering floor: (schedule, chain ms, schedule ms, the chain's systems and ms).
type Floor = (String, f64, f64, Vec<(String, f64)>);

/// The longest chain of ordered systems in each schedule, weighted by each system's time in
/// `stats` (ms per frame): the part of the schedule that runs one after another even on
/// unlimited cores. Returns (schedule, chain ms, schedule ms, chain) slowest chain first.
fn ordering_floors(
    order: &ScheduleOrder,
    stats: &HashMap<String, Stat>,
    frames: f64,
) -> Vec<Floor> {
    let ms = |key: &str| {
        stats
            .get(key)
            .map_or(0.0, |s| s.total.as_secs_f64() * 1000.0 / frames)
    };
    let mut floors = Vec::new();
    for (label, systems) in &order.0 {
        let wall = ms(&format!("schedule {label}"));
        if systems.is_empty() || wall <= 0.0 {
            continue;
        }
        // Systems come in run order, so one pass forward finds each one's longest lead-in.
        let mut best: Vec<(f64, Option<usize>)> = vec![(0.0, None); systems.len()];
        for i in 0..systems.len() {
            best[i].0 += ms(&systems[i].0);
            let (through, _) = best[i];
            for &j in &systems[i].1 {
                if through > best[j].0 {
                    best[j] = (through, Some(i));
                }
            }
        }
        let Some((mut at, &(floor, _))) = best
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
        else {
            continue;
        };
        let mut chain = Vec::new();
        loop {
            chain.push((systems[at].0.clone(), ms(&systems[at].0)));
            match best[at].1 {
                Some(prev) => at = prev,
                None => break,
            }
        }
        chain.reverse();
        floors.push((label.clone(), floor, wall, chain));
    }
    floors.sort_by(|a, b| b.1.total_cmp(&a.1));
    floors
}

/// Rows of the ordering chain shown per schedule.
const CHAIN_ROWS: usize = 8;

fn format_floors(floors: &[Floor]) -> String {
    let mut out = String::new();
    if floors.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "\nordering floor (longest before/after chain, ms per frame, of the schedule's ms)"
    );
    for (label, floor, wall, chain) in floors.iter().filter(|f| f.1 >= 0.05) {
        let mut steps: Vec<&(String, f64)> = chain.iter().filter(|(_, t)| *t >= 0.05).collect();
        steps.sort_by(|a, b| b.1.total_cmp(&a.1));
        steps.truncate(CHAIN_ROWS);
        let heaviest: Vec<String> = steps.iter().map(|(n, t)| format!("{n} {t:.2}")).collect();
        let _ = writeln!(
            out,
            "{label:<16} {floor:>6.2} of {wall:>6.2} ({} systems; heaviest: {})",
            chain.len(),
            heaviest.join(", "),
        );
        // The chain itself, in run order (systems of 0.05 ms or more), to see which ordering
        // edge joins the heavy ones.
        if chain.len() > 1 {
            let path: Vec<String> = chain
                .iter()
                .filter(|(_, t)| *t >= 0.05)
                .map(|(n, t)| format!("{n} {t:.2}"))
                .collect();
            let _ = writeln!(out, "  chain: {}", path.join(" -> "));
        }
    }
    out
}

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

/// What the systems of one schedule may touch: for tests that keep the frame running side by
/// side (a new exclusive system or a new writer of a contested component shows up here).
#[derive(Debug, Default, Clone)]
pub struct ScheduleAccess {
    /// The schedule's label, as `{:?}` prints it.
    pub schedule: String,
    /// Systems that take the whole world (`&mut World`, `apply_deferred` included), short names.
    pub exclusive: Vec<String>,
    /// Per component or resource (short name), the systems that write it (short names, sorted).
    pub writers: std::collections::BTreeMap<String, Vec<String>>,
}

/// Builds every schedule as its first run would (without running anything) and lists, per
/// schedule, its exclusive systems and who writes what.
pub fn schedule_access(world: &mut World) -> Vec<ScheduleAccess> {
    use bevy_ecs::query::ComponentAccessKind;
    use bevy_ecs::schedule::Schedules;
    // One at a time out of `Schedules`: building one reads `Schedules` itself.
    let labels: Vec<_> = world
        .get_resource::<Schedules>()
        .map(|s| s.iter().map(|(_, schedule)| schedule.label()).collect())
        .unwrap_or_default();
    for label in labels {
        let Some(mut schedule) = world.resource_mut::<Schedules>().remove(label) else {
            continue;
        };
        if let Err(e) = schedule.initialize(world) {
            crate::log::warn!("schedule {label:?} doesn't build: {e}");
        }
        world.resource_mut::<Schedules>().insert(schedule);
    }
    let Some(schedules) = world.get_resource::<Schedules>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (label, schedule) in schedules.iter() {
        let Ok(systems) = schedule.systems_with_access() else {
            continue;
        };
        let mut access = ScheduleAccess {
            schedule: format!("{label:?}"),
            ..Default::default()
        };
        for (_, s) in systems {
            let name = short_name(&s.system().name().to_string());
            if s.access().is_exclusive() {
                access.exclusive.push(name);
                continue;
            }
            let set = s.access().to_filtered_access_set();
            let Ok(kinds) = set.combined_access().try_iter_access() else {
                continue;
            };
            for kind in kinds {
                if let ComponentAccessKind::Exclusive(id) = kind {
                    let written = world
                        .components()
                        .get_name(id)
                        .map_or_else(|| format!("{id:?}"), |n| n.shortname().to_string());
                    access
                        .writers
                        .entry(written)
                        .or_default()
                        .push(name.clone());
                }
            }
        }
        access.exclusive.sort();
        for w in access.writers.values_mut() {
            w.sort();
            w.dedup();
        }
        out.push(access);
    }
    out.sort_by(|a, b| a.schedule.cmp(&b.schedule));
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
    fn schedule_access_lists_exclusive_systems_and_writers() {
        #[derive(Component)]
        struct Spot(f32);
        fn mover(mut q: Query<&mut Spot>) {
            for mut s in &mut q {
                s.0 += 1.0;
            }
        }
        fn looker(_q: Query<&Spot>) {}
        fn whole(_world: &mut World) {}
        let mut app = crate::app::App::new();
        app.add_systems(crate::schedule::Update, (mover, looker, whole));
        let all = schedule_access(&mut app.world);
        let update = all.iter().find(|a| a.schedule == "Update").unwrap();
        assert_eq!(update.exclusive, ["engine_core::whole"]);
        let spot = update
            .writers
            .iter()
            .find(|(k, _)| k.ends_with("Spot"))
            .unwrap()
            .1;
        assert_eq!(spot, &["engine_core::mover"]);
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

    #[test]
    fn ordering_floor_follows_the_heaviest_chain() {
        use bevy_ecs::schedule::{Schedule, ScheduleLabel};
        #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
        struct Probe;
        fn a() {}
        fn b() {}
        fn c() {}
        fn d() {}
        let mut world = World::new();
        let mut schedule = Schedule::new(Probe);
        schedule.add_build_pass(OrderPass {
            label: "Probe".into(),
        });
        schedule.add_systems(((a, b, c).chain(), d));
        schedule.initialize(&mut world).unwrap();
        let order = world.remove_resource::<ScheduleOrder>().unwrap();
        assert_eq!(order.0["Probe"].len(), 4);

        let mut stats = HashMap::new();
        for (key, ms) in [
            ("schedule Probe", 100),
            ("engine_core::a", 20),
            ("engine_core::b", 30),
            ("engine_core::c", 10),
            ("engine_core::d", 50),
        ] {
            stats.insert(
                key.to_string(),
                Stat {
                    total: Duration::from_millis(ms),
                    calls: 1,
                    max: Duration::ZERO,
                },
            );
        }
        let floors = ordering_floors(&order, &stats, 1.0);
        let (label, floor, wall, chain) = &floors[0];
        assert_eq!(label, "Probe");
        assert!((floor - 60.0).abs() < 1e-6, "{floor}");
        assert!((wall - 100.0).abs() < 1e-6);
        let names: Vec<&str> = chain.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["engine_core::a", "engine_core::b", "engine_core::c"]
        );
        let text = format_floors(&floors);
        assert!(text.contains("Probe"), "{text}");
        // The chain in run order, so the edge between two heavy systems can be found.
        assert!(
            text.contains(
                "chain: engine_core::a 20.00 -> engine_core::b 30.00 -> engine_core::c 10.00"
            ),
            "{text}"
        );
    }

    #[test]
    fn traces_are_chrome_trace_json_with_frames_threads_and_spans() {
        let dir = std::env::temp_dir().join(format!("engine-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trace.json");
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let events = vec![
            ("schedule Update".to_string(), 1, t0, ms(10)),
            ("game::move_\"it\"".to_string(), 2, t0 + ms(1), ms(3)),
            (
                format!("{INNER}engine_render: submit"),
                2,
                t0 + ms(2),
                ms(1),
            ),
        ];
        super::trace::write_for_test(
            &path,
            vec![t0, t0 + ms(16)],
            events,
            &["main".to_string(), "compute (0)".to_string()],
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let events = json["traceEvents"].as_array().unwrap();
        let named = |ph: &'static str| events.iter().filter(move |e| e["ph"] == ph);
        assert_eq!(named("M").count(), 2, "both threads named");
        assert_eq!(named("i").count(), 1, "one frame mark");
        let spans: Vec<_> = named("X").collect();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0]["cat"], "schedule");
        assert_eq!(spans[0]["name"], "Update");
        assert_eq!(spans[1]["name"], "game::move_\"it\"");
        assert_eq!(spans[1]["tid"], 2);
        assert!((spans[1]["ts"].as_f64().unwrap() - 1000.0).abs() < 1.0);
        assert!((spans[1]["dur"].as_f64().unwrap() - 3000.0).abs() < 1.0);
        assert_eq!(spans[2]["cat"], "span");
        assert_eq!(spans[2]["name"], "engine_render: submit");
        std::fs::remove_dir_all(&dir).ok();
    }
}
