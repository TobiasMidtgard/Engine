//! Which buildings a company may put up yet, and what opens the rest.
//!
//! A public building opens with the need it serves: a Chapel once 100 Fisherfolk or 50
//! Labourers ask for one, a School at 100 Burghers ([`crate::defs::Want::unlock`]). A works
//! staffed by Labourers or Burghers opens once the company has people of that tier: a house
//! has to be moved up first. Everything else (harbours, houses, markets, timber, fish, wool
//! and the farms of the first tier, ships, the watch) is open from the start. People are
//! counted over every island the company holds.
//!
//! The interface greys out and padlocks what is locked with [`Lock::why`]
//! (`isle.build.locks`), and [`refusal`] keeps `build::place` from putting it up.

use engine::prelude::World;
use serde::Serialize;

use crate::defs::{Kind, Need, Tier};
use crate::sim::{Company, Economy};

/// Whether one kind of building is open to a company, and why not.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Lock {
    pub kind: Kind,
    pub name: &'static str,
    pub locked: bool,
    /// What opens it, in a sentence (empty when it is open).
    pub why: String,
    /// How far along the nearest way to open it is, 0 to 1 (1 when open).
    pub progress: f32,
}

/// People of each tier on the islands company `c` holds.
pub fn people_of(econ: &Economy, c: Company) -> [f32; Tier::COUNT] {
    let mut p = [0.0; Tier::COUNT];
    for (i, b) in econ.islands.iter().enumerate() {
        if econ.owns(c, i) {
            for (t, n) in p.iter_mut().zip(b.population) {
                *t += n;
            }
        }
    }
    p
}

/// What opens `kind`: people of a tier, any one of which will do (empty: open from the
/// start).
pub fn opens_at(kind: Kind) -> Vec<(Tier, f32)> {
    let mut ways: Vec<(Tier, f32)> = Tier::ALL
        .iter()
        .flat_map(|&t| {
            t.wants()
                .iter()
                .filter(move |w| matches!(w.need, Need::Service(k) if k == kind))
                // (A higher tier's need asks for at least one of its people.)
                .map(move |w| {
                    (
                        t,
                        if t == Tier::Fisherfolk {
                            w.unlock
                        } else {
                            w.unlock.max(1.0)
                        },
                    )
                })
        })
        .collect();
    if ways.is_empty()
        && let Some((t, _)) = kind.def().workers
        && t != Tier::Fisherfolk
    {
        ways.push((t, 1.0));
    }
    if ways.iter().any(|(_, n)| *n <= 0.0) {
        ways.clear();
    }
    ways
}

/// Whether `kind` is open to company `c`, and what opens it if not.
pub fn lock_of(econ: &Economy, c: Company, kind: Kind) -> Lock {
    let ways = opens_at(kind);
    let have = people_of(econ, c);
    let progress = ways
        .iter()
        .map(|(t, n)| (have[t.index()] / n).min(1.0))
        .fold(if ways.is_empty() { 1.0 } else { 0.0 }, f32::max);
    let locked = progress < 1.0;
    let why = if !locked {
        String::new()
    } else if kind.def().workers.is_some_and(|(t, _)| ways == [(t, 1.0)]) {
        let t = ways[0].0;
        format!("Worked by {0}: move a house up to {0} first.", t.name())
    } else {
        let need: Vec<String> = ways
            .iter()
            .map(|(t, n)| format!("{n:.0} {}", t.name()))
            .collect();
        let got: Vec<String> = ways
            .iter()
            .map(|(t, _)| format!("{:.0} {}", have[t.index()].floor(), t.name()))
            .collect();
        format!(
            "Opens at {} (you have {}).",
            need.join(" or "),
            got.join(", ")
        )
    };
    Lock {
        kind,
        name: kind.def().name,
        locked,
        why,
        progress,
    }
}

/// Every kind of building the build bar offers, open or not, for the company giving orders.
pub fn locks(world: &World) -> Vec<Lock> {
    let econ = world.resource::<Economy>();
    let me = crate::sim::acting(world);
    crate::defs::DEFS
        .iter()
        .map(|d| lock_of(econ, me, d.kind))
        .collect()
}

/// Why the company giving orders may not put up `kind` yet (None when it may).
pub fn refusal(world: &World, kind: Kind) -> Option<String> {
    let l = lock_of(world.resource::<Economy>(), crate::sim::acting(world), kind);
    l.locked
        .then(|| format!("{} is not open yet. {}", l.name, l.why))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tier_starts_with_what_it_needs_and_the_rest_opens_with_people() {
        let mut econ = Economy::default();
        econ.islands[0].harbours = 1;
        econ.islands[0].owner = 0;
        econ.islands[0].population[Tier::Fisherfolk.index()] = 40.0;
        for k in [
            Kind::Harbour,
            Kind::House,
            Kind::Market,
            Kind::Fishery,
            Kind::Woodcutter,
            Kind::Sawmill,
            Kind::SheepFarm,
            Kind::Weaver,
            Kind::Shipyard,
        ] {
            assert!(!lock_of(&econ, 0, k).locked, "{k:?}");
        }
        let chapel = lock_of(&econ, 0, Kind::Chapel);
        assert!(chapel.locked);
        assert!(
            chapel.why.contains("100 Fisherfolk or 50 Labourers"),
            "{}",
            chapel.why
        );
        assert!((chapel.progress - 0.4).abs() < 1e-6);
        let mill = lock_of(&econ, 0, Kind::Mill);
        assert!(
            mill.locked && mill.why.contains("Labourers"),
            "{}",
            mill.why
        );
        // One house moved up opens the Labourers' works; their Chapel at 50.
        econ.islands[0].population[Tier::Labourers.index()] = 12.0;
        assert!(!lock_of(&econ, 0, Kind::Mill).locked);
        assert!(lock_of(&econ, 0, Kind::Chapel).locked);
        econ.islands[0].population[Tier::Labourers.index()] = 50.0;
        assert!(!lock_of(&econ, 0, Kind::Chapel).locked);
        // Another company's people open nothing for this one.
        assert!(lock_of(&econ, 1, Kind::Mill).locked);
    }
}
