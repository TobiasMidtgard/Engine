//! `--loops plaza,fortress[:rounds]`: jump the camera round places for a few rounds and log,
//! at each visit, the frame time, the profile's slowest systems and which components grew
//! since the last visit there. For finding what builds up over a session (the lead's PC loops
//! slowed at the plaza while the fortress didn't). The last round warns about what still grew
//! since the round before: a revisit should leave the world as it found it.

use crate::{camera, places};
use engine::prelude::*;
use std::collections::HashMap;
use std::time::Instant;

/// Seconds at each place before it is measured (the camera's move and the builds in view
/// settle first).
const DWELL_S: f32 = 25.0;

#[derive(Resource)]
struct Loops {
    places: Vec<places::Place>,
    rounds: u32,
    round: u32,
    at: usize,
    arrived: Option<Instant>,
    /// Each place's component counts at its last visit.
    seen: HashMap<&'static str, HashMap<String, usize>>,
}

/// Install the loop for `spec` (`a,b,c` or `a,b,c:rounds`, 5 rounds by default).
pub fn install(app: &mut App, spec: &str) {
    let (keys, rounds) = match spec.split_once(':') {
        Some((keys, n)) => (keys, n.parse().unwrap_or(5)),
        None => (spec, 5),
    };
    let places: Vec<_> = keys
        .split(',')
        .map(|k| places::find(k.trim()).unwrap_or_else(|| panic!("--loops: no place {k}")))
        .collect();
    app.insert_resource(Loops {
        places,
        rounds,
        round: 1,
        at: 0,
        arrived: None,
        seen: HashMap::new(),
    });
    app.add_systems(Update, run.after(camera::fly).run_if(due));
}

/// Whether [`run`] has anything to do this frame: the world is built and the camera is due to
/// move on (or hasn't gone to the first place yet).
fn due(loops: Res<Loops>, build: Option<Res<crate::build::WorldBuild>>) -> bool {
    build.is_none()
        && loops
            .arrived
            .is_none_or(|at| at.elapsed().as_secs_f32() >= DWELL_S)
}

fn run(world: &mut World) {
    let (place, arrived) = {
        let l = world.resource::<Loops>();
        (l.places[l.at].clone(), l.arrived)
    };
    let Some(arrived) = arrived else {
        camera::go_to(world, &place);
        places::visit_soon(world, &place);
        world.resource_mut::<Loops>().arrived = Some(Instant::now());
        return;
    };
    if arrived.elapsed().as_secs_f32() < DWELL_S {
        return;
    }
    let counts = census(world);
    let entities = world.entities().count_spawned();
    let profile = world
        .get_resource::<engine::core::profile::ProfileSnapshot>()
        .filter(|p| !p.systems.is_empty())
        .map(|p| {
            let top: Vec<String> = p
                .systems
                .iter()
                .take(8)
                .map(|(name, ms)| format!("{name} {ms:.1}"))
                .collect();
            format!(
                "{:.0} ms a frame, systems {:.1} ms: {}",
                p.frame_ms,
                p.busy_ms,
                top.join(", ")
            )
        })
        .unwrap_or_else(|| "no profile (run with --profile)".into());
    let mut l = world.resource_mut::<Loops>();
    let grown = l.seen.get(place.key).map(|before| {
        let mut diff: Vec<(i64, &String, usize)> = counts
            .iter()
            .map(|(name, &n)| (n as i64 - *before.get(name).unwrap_or(&0) as i64, name, n))
            .filter(|(d, ..)| *d != 0)
            .collect();
        diff.sort_by_key(|(d, ..)| -d.abs());
        diff.iter()
            .take(12)
            .map(|(d, name, n)| format!("{name} {d:+} (now {n})"))
            .collect::<Vec<_>>()
            .join(", ")
    });
    let totals: Vec<String> = WATCHED
        .iter()
        .map(|w| format!("{w} {}", total(&counts, w)))
        .collect();
    engine::core::log::info!(
        "loop {}/{} at {}: {entities} entities ({}); {profile}; since the last visit: {}",
        l.round,
        l.rounds,
        place.key,
        totals.join(", "),
        grown.as_deref().unwrap_or("(first visit)")
    );
    // The last round: what still grew since the one before. Trees and plants that grew are
    // a leak; parts that grew while some still wait for theirs are Foliage catching up (a few
    // ms a frame, so over visits at headless frame times).
    if l.round == l.rounds
        && l.rounds > 1
        && let Some(before) = l.seen.get(place.key)
    {
        let grew = |w: &str| total(&counts, w) > total(before, w);
        let still: Vec<String> = WATCHED
            .iter()
            .filter(|w| grew(w))
            .map(|w| format!("{w} {} -> {}", total(before, w), total(&counts, w)))
            .collect();
        let owners_grew = OWNERS.iter().any(|w| grew(w));
        let waiting = total(&counts, "SceneryTree") + total(&counts, "Plant")
            - (total(&counts, "SceneryTreeVisual") + total(&counts, "PlantVisual"))
                .min(total(&counts, "SceneryTree") + total(&counts, "Plant"));
        if still.is_empty() {
            engine::core::log::info!("loop: {} is flat on the last revisit", place.key);
        } else if !owners_grew && waiting > 0 {
            engine::core::log::info!(
                "loop: {} has as many trees and plants as last time; {waiting} still wait for \
                 their parts ({})",
                place.key,
                still.join(", ")
            );
        } else {
            engine::core::log::warn!(
                "loop: {} still grows on the last revisit: {}",
                place.key,
                still.join(", ")
            );
        }
    }
    l.seen.insert(place.key, counts);
    l.arrived = None;
    l.at += 1;
    if l.at == l.places.len() {
        l.at = 0;
        l.round += 1;
        if l.round > l.rounds {
            world.write_message(AppExit::Success);
        }
    }
}

/// Components whose totals every visit logs: the trees and plants and what draws them.
const WATCHED: [&str; 7] = [
    "SceneryTree",
    "SceneryTreeVisual",
    "NearWoodsTree",
    "Plant",
    "PlantVisual",
    "PlantPart",
    "MeshRenderer",
];

/// What the drawn parts belong to: more of these on a revisit is a leak.
const OWNERS: [&str; 3] = ["SceneryTree", "NearWoodsTree", "Plant"];

/// Entities with the component named `short` (the type's name without its path).
fn total(counts: &HashMap<String, usize>, short: &str) -> usize {
    counts
        .iter()
        .filter(|(name, _)| name.rsplit("::").next() == Some(short))
        .map(|(_, n)| n)
        .sum()
}

/// Entities per component type.
fn census(world: &World) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for arch in world.archetypes().iter().filter(|a| !a.is_empty()) {
        for id in arch.iter_components() {
            if let Some(name) = world.components().get_name(id) {
                *out.entry(format!("{name}")).or_default() += arch.len() as usize;
            }
        }
    }
    out
}
