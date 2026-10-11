//! Logging: `tracing` to stderr plus an in-memory ring buffer the AI bridge can read
//! (`log.tail`), so an agent can see warnings and errors from a running game.
//!
//! Use the `tracing` macros re-exported here: `info!`, `warn!`, `error!`, `debug!`.

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use bevy_ecs::prelude::*;
use parking_lot::Mutex;
use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

pub use tracing::{debug, error, info, trace, warn};

/// One captured log line.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub level: String,
    pub target: String,
    pub message: String,
    pub real_secs: f64,
}

/// Shared ring buffer of recent log lines.
#[derive(Resource, Clone)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
    capacity: usize,
}

impl LogBuffer {
    /// The most recent `n` lines, oldest first. `min_level` filters ("warn" keeps warn+error).
    pub fn tail(&self, n: usize, min_level: Option<&str>) -> Vec<LogLine> {
        let rank = |l: &str| match l {
            "ERROR" => 4,
            "WARN" => 3,
            "INFO" => 2,
            "DEBUG" => 1,
            _ => 0,
        };
        let min = min_level.map(|l| rank(&l.to_uppercase())).unwrap_or(0);
        let lines = self.lines.lock();
        let filtered: Vec<LogLine> = lines
            .iter()
            .filter(|l| rank(&l.level) >= min)
            .cloned()
            .collect();
        filtered[filtered.len().saturating_sub(n)..].to_vec()
    }

    fn push(&self, line: LogLine) {
        let mut lines = self.lines.lock();
        if lines.len() >= self.capacity {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

static BUFFER: OnceLock<LogBuffer> = OnceLock::new();
static START: OnceLock<std::time::Instant> = OnceLock::new();

/// The process-wide log buffer (created on first use).
pub fn log_buffer() -> LogBuffer {
    BUFFER
        .get_or_init(|| LogBuffer {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(2048))),
            capacity: 2048,
        })
        .clone()
}

struct BufferLayer(LogBuffer);

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        } else {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            self.0.push_str(&format!("{}={value:?}", field.name()));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let start = START.get_or_init(std::time::Instant::now);
        self.0.push(LogLine {
            level: event.metadata().level().to_string(),
            target: event.metadata().target().to_string(),
            message: visitor.0,
            real_secs: start.elapsed().as_secs_f64(),
        });
    }
}

/// Install the global logger once. `default_filter` is used unless `RUST_LOG` is set,
/// e.g. `"info,wgpu=warn,naga=warn"`.
pub fn init(default_filter: &str) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        START.get_or_init(std::time::Instant::now);
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(default_filter));
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(BufferLayer(log_buffer()))
            .try_init();
    });
}
