//! Streaming audit (`ENGINE_STREAM_AUDIT`): loads, unloads and reloads every streamed cell
//! and reports what each leaves behind or duplicates.
//!
//! A cell's unload takes the entities its spawn created ([`engine_world::CellMember`]) and
//! whatever its unload hook drops. Anything a demo spawns later from its own systems, or keeps
//! in a resource, survives an unload and comes back twice on the next load (playtest #26,
//! "duplicated assets"). For each cell the audit pins it out, in, out and in again
//! ([`StreamedCells::pins`]), lets each state settle, and logs `stream audit:` lines: what
//! loading added (by module, as the shift audit groups it), what stayed after the unload, and
//! how the reload differs from the first load, with the process RAM at each step.
//!
//! Other streamers keep working meanwhile (foliage cells around the camera, other cells), so
//! the counts take only entities within the cell's reach (its radius plus [`MARGIN`]; entities
//! without a place count everywhere), and the baseline is taken twice: kinds whose count changes
//! between the two with the cell out change on their own, and are listed apart, not blamed on
//! the cell.
//!
//! `ENGINE_STREAM_AUDIT=<settle frames>` audits every cell; `<settle>:<key>,<key>` only those.

use std::collections::{BTreeMap, HashMap};

use engine_core::diagnostics::{Diagnostics, PROCESS_RAM_MB};
use engine_core::ecs::resource::IsResource;
use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::{CellState, StreamedCells};

use crate::shift_audit::kind;

/// Kinds listed per line at most.
const ROWS: usize = 12;

/// Meters past a cell's radius that still count as its own.
pub const MARGIN: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Pinned out: the baseline without the cell.
    Out,
    /// Still out, a settle later: what changes on its own.
    Still,
    /// Pinned in: the first load.
    In,
    /// Pinned out again: what the unload left.
    OutAgain,
    /// Pinned in again: the reload.
    InAgain,
}

type Census = (HashMap<String, usize>, Option<f64>);

#[derive(Resource)]
struct StreamAudit {
    settle: u32,
    only: Option<Vec<String>>,
    /// Cells to audit, by key, and the one in progress.
    queue: Vec<String>,
    step: Step,
    waited: u32,
    census: Vec<Census>,
    started: bool,
}

/// Install the audit when `ENGINE_STREAM_AUDIT` asks for it.
pub(crate) fn install(app: &mut App) {
    let Ok(spec) = std::env::var("ENGINE_STREAM_AUDIT") else {
        return;
    };
    let (settle, only) = match spec.split_once(':') {
        Some((n, keys)) => (
            n,
            Some(keys.split(',').map(|k| k.trim().to_string()).collect()),
        ),
        None => (spec.as_str(), None),
    };
    let Ok(settle) = settle.trim().parse::<u32>() else {
        engine_core::log::warn!(
            "ENGINE_STREAM_AUDIT={spec:?} not understood; use <settle frames> (e.g. 120) or \
             <settle frames>:<key>,<key>"
        );
        return;
    };
    app.add_plugins(StreamAuditPlugin { settle, only });
}

/// Runs the streaming audit (what `ENGINE_STREAM_AUDIT` installs): each cell, or only those
/// in `only`, goes out, in, out and in again, each step held `settle` frames.
pub struct StreamAuditPlugin {
    pub settle: u32,
    pub only: Option<Vec<String>>,
}

impl Plugin for StreamAuditPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(StreamAudit {
            settle: self.settle.max(1),
            only: self.only.clone(),
            queue: Vec::new(),
            step: Step::Out,
            waited: 0,
            census: Vec::new(),
            started: false,
        })
        .init_resource::<StreamAuditReport>()
        .add_systems(First, audit_system);
    }
}

/// The audit's verdicts so far: each cell's key, and what its unload left plus how its reload
/// differed from its first load (entities by kind; both empty when clean).
#[derive(Resource, Debug, Clone, Default)]
pub struct StreamAuditReport {
    pub cells: Vec<AuditedCell>,
    pub done: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AuditedCell {
    pub key: String,
    pub loaded: i64,
    pub left: Vec<(String, i64)>,
    pub reload: Vec<(String, i64)>,
}

impl AuditedCell {
    pub fn clean(&self) -> bool {
        self.left.is_empty() && self.reload.is_empty()
    }
}

/// Entities by kind within `reach` of `center` (or with no place), and the process RAM.
fn census(world: &mut World, center: DVec3, reach: f64) -> Census {
    let entities: Vec<Entity> = world
        .query_filtered::<(Entity, Option<&GlobalTransform>), Without<IsResource>>()
        .iter(world)
        .filter(|(_, g)| g.is_none_or(|g| g.translation().distance(center) <= reach))
        .map(|(e, _)| e)
        .collect();
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for e in entities {
        *kinds.entry(kind(world, e)).or_default() += 1;
    }
    let ram = world.resource::<Diagnostics>().get(PROCESS_RAM_MB);
    (kinds, ram)
}

/// `after - before` per kind, largest changes first, without zeros.
fn diff(before: &HashMap<String, usize>, after: &HashMap<String, usize>) -> Vec<(String, i64)> {
    let mut all: BTreeMap<&String, i64> = BTreeMap::new();
    for (k, n) in after {
        *all.entry(k).or_default() += *n as i64;
    }
    for (k, n) in before {
        *all.entry(k).or_default() -= *n as i64;
    }
    let mut out: Vec<(String, i64)> = all
        .into_iter()
        .filter(|(_, d)| *d != 0)
        .map(|(k, d)| (k.clone(), d))
        .collect();
    out.sort_by_key(|(_, d)| std::cmp::Reverse(d.abs()));
    out
}

fn rows(d: &[(String, i64)]) -> String {
    let total: i64 = d.iter().map(|(_, n)| n).sum();
    let mut text = format!("{total:+}");
    for (k, n) in d.iter().take(ROWS) {
        text.push_str(&format!("\n    {n:+6}  {k}"));
    }
    if d.len() > ROWS {
        text.push_str(&format!("\n    ... {} more kinds", d.len() - ROWS));
    }
    text
}

fn ram(from: Option<f64>, to: Option<f64>) -> String {
    match (from, to) {
        (Some(a), Some(b)) => format!("RAM {:+.0} MB", b - a),
        _ => "RAM n/a".into(),
    }
}

fn audit_system(world: &mut World) {
    let mut audit = world.remove_resource::<StreamAudit>().expect("installed");
    run(world, &mut audit);
    world.insert_resource(audit);
}

fn run(world: &mut World, audit: &mut StreamAudit) {
    let Some(cells) = world.get_resource::<StreamedCells>() else {
        return;
    };
    if !audit.started {
        if cells.cells.is_empty() {
            return;
        }
        audit.queue = cells
            .cells
            .iter()
            .map(|c| c.key.clone())
            .filter(|k| audit.only.as_ref().is_none_or(|o| o.contains(k)))
            .rev()
            .collect();
        audit.started = true;
        engine_core::log::info!(
            "stream audit: {} cells, {} frames each step",
            audit.queue.len(),
            audit.settle
        );
    }
    let Some(key) = audit.queue.last().cloned() else {
        return;
    };
    let cell = cells.find(&key).and_then(|id| cells.get(id));
    let state = cell.map(|c| c.state);
    let (center, reach) = cell.map_or((DVec3::ZERO, f64::INFINITY), |c| {
        (c.center, c.radius + MARGIN)
    });
    let want_in = matches!(audit.step, Step::In | Step::InAgain);
    world
        .resource_mut::<StreamedCells>()
        .pins
        .insert(key.clone(), want_in);
    let settled = state
        == Some(if want_in {
            CellState::Loaded
        } else {
            CellState::Unloaded
        });
    if !settled {
        audit.waited = 0;
        return;
    }
    audit.waited += 1;
    if audit.waited < audit.settle {
        return;
    }
    audit.waited = 0;
    audit.census.push(census(world, center, reach));
    audit.step = match audit.step {
        Step::Out => Step::Still,
        Step::Still => Step::In,
        Step::In => Step::OutAgain,
        Step::OutAgain => Step::InAgain,
        Step::InAgain => {
            let verdict = report(&key, &audit.census);
            world
                .resource_mut::<StreamAuditReport>()
                .cells
                .extend(verdict);
            audit.census.clear();
            world.resource_mut::<StreamedCells>().pins.remove(&key);
            audit.queue.pop();
            if audit.queue.is_empty() {
                world.resource_mut::<StreamAuditReport>().done = true;
                engine_core::log::info!("stream audit: done");
            }
            Step::Out
        }
    };
}

fn report(key: &str, census: &[Census]) -> Option<AuditedCell> {
    let [
        (base, r0),
        (still, _),
        (loaded, r1),
        (after, r2),
        (reloaded, r3),
    ] = census
    else {
        return None;
    };
    // Kinds that changed with the cell out change on their own: not the cell's.
    let churn: Vec<(String, i64)> = diff(base, still);
    let own = |d: Vec<(String, i64)>| -> Vec<(String, i64)> {
        d.into_iter()
            .filter(|(k, _)| !churn.iter().any(|(c, _)| c == k))
            .collect()
    };
    let load = own(diff(base, loaded));
    let left = own(diff(base, after));
    let again = own(diff(loaded, reloaded));
    let verdict = if left.is_empty() && again.is_empty() {
        "clean"
    } else {
        "LEAKS"
    };
    engine_core::log::info!(
        "stream audit: {key}: {verdict}\n  load {} ({})\n  left after unload {} ({})\n  \
         reload vs first load {} ({}){}",
        rows(&load),
        ram(*r0, *r1),
        rows(&left),
        ram(*r0, *r2),
        rows(&again),
        ram(*r1, *r3),
        if churn.is_empty() {
            String::new()
        } else {
            let kinds: Vec<&str> = churn.iter().map(|(k, _)| k.as_str()).collect();
            format!("\n  left out (change on their own): {}", kinds.join(", "))
        },
    );
    Some(AuditedCell {
        key: key.to_string(),
        loaded: load.iter().map(|(_, n)| n).sum(),
        left,
        reload: again,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lists_changes_largest_first() {
        let before = HashMap::from([("a".to_string(), 3), ("b".to_string(), 1)]);
        let after = HashMap::from([("a".to_string(), 3), ("b".to_string(), 4), ("c".into(), 1)]);
        let d = diff(&before, &after);
        assert_eq!(d, vec![("b".to_string(), 3), ("c".to_string(), 1)]);
        assert!(rows(&d).starts_with("+4"));
        assert!(diff(&after, &after).is_empty());
    }
}
