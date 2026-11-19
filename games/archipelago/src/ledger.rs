//! The economy's ledger: what each production chain and each tier is worth, worked out from
//! the tables alone, so the balance can be read (and tested) without running a game.

use crate::defs::{DEFS, Good, Kind, NEED_PER_PERSON_MIN, Need, Tier};

/// One production chain at the size of one works making the final good.
#[derive(Clone, Debug)]
pub struct Chain {
    pub good: Good,
    /// Works of each kind it takes (fractions: a mill can serve two bakeries).
    pub works: Vec<(Kind, f32)>,
    /// Goods a minute it makes.
    pub per_min: f32,
    /// Coins a minute it costs to run.
    pub upkeep: f32,
}

fn producer(g: Good) -> Option<&'static crate::defs::Def> {
    DEFS.iter().find(|d| d.output == Some(g))
}

/// The chain behind `g`, sized to one works of the good itself.
pub fn chain(g: Good) -> Option<Chain> {
    let top = producer(g)?;
    let mut works = Vec::new();
    let add = |works: &mut Vec<(Kind, f32)>, k: Kind, n: f32| match works
        .iter_mut()
        .find(|(w, _)| *w == k)
    {
        Some((_, m)) => *m += n,
        None => works.push((k, n)),
    };
    // Breadth first: (def, works of it).
    let mut todo = vec![(top, 1.0f32)];
    while let Some((d, n)) = todo.pop() {
        add(&mut works, d.kind, n);
        // One input good a batch: the upstream works it takes to keep up.
        for i in d.input.into_iter().chain(d.input2) {
            let up = producer(i)?;
            todo.push((up, n * up.cycle_s / d.cycle_s));
        }
    }
    let upkeep = works.iter().map(|(k, n)| k.def().upkeep * n).sum();
    Some(Chain {
        good: g,
        works,
        per_min: 60.0 / top.cycle_s,
        upkeep,
    })
}

/// Goods a minute one full house of `t` uses of `g`.
pub fn house_use(t: Tier, g: Good) -> f32 {
    t.capacity() * NEED_PER_PERSON_MIN * g.need_share()
}

/// What a chain of `g` earns from houses of `t` when it feeds only half the houses it could:
/// (full houses it feeds, coins a minute at half load).
pub fn half_load(t: Tier, g: Good) -> Option<(f32, f32)> {
    let w = t.wants().iter().find(|w| w.need == Need::Good(g))?;
    let c = chain(g)?;
    let houses = c.per_min / house_use(t, g);
    Some((houses, 0.5 * houses * w.coins))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn print_the_tables() {
        println!("kind | coins | cost | workers | in | in2 | out | cycle_s | upkeep | per min");
        for d in DEFS.iter() {
            let per_min = if d.cycle_s > 0.0 {
                60.0 / d.cycle_s
            } else {
                0.0
            };
            println!(
                "{:?} | {} | {:?} | {:?} | {:?} | {:?} | {:?} | {} | {} | {:.2}",
                d.kind,
                d.coins,
                d.cost,
                d.workers,
                d.input,
                d.input2,
                d.output,
                d.cycle_s,
                d.upkeep,
                per_min
            );
        }
        for t in Tier::ALL {
            println!(
                "\n{} house {} rent {} full income {}",
                t.name(),
                t.capacity(),
                t.rent(),
                t.full_income()
            );
            for w in t.wants() {
                let line = match w.need {
                    Need::Good(g) => match (chain(g), half_load(t, g)) {
                        (Some(c), Some((houses, pays))) => format!(
                            "upkeep {:.1}, feeds {houses:.0} houses, pays {pays:.1} at half load",
                            c.upkeep
                        ),
                        _ => String::new(),
                    },
                    Need::Service(k) => format!("{} in reach", k.def().name),
                };
                println!(
                    "  {:?} {:?}: points {} people {} coins {} joy {} from {} | {line}",
                    w.cat, w.need, w.points, w.people, w.coins, w.joy, w.unlock
                );
            }
        }
    }

    /// Every chain pays for itself when it feeds only half the houses it could: a player
    /// who builds a little ahead of demand never loses coins on it.
    #[test]
    fn every_chain_pays_at_half_load() {
        let mut bad = Vec::new();
        for t in Tier::ALL {
            for w in t.wants() {
                let Need::Good(g) = w.need else { continue };
                let (Some(c), Some((_, pays))) = (chain(g), half_load(t, g)) else {
                    continue;
                };
                if pays <= c.upkeep {
                    bad.push(format!(
                        "{t:?} {g:?}: upkeep {:.1}, pays {pays:.1}",
                        c.upkeep
                    ));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "chains that lose coins at half load: {bad:#?}"
        );
    }

    /// The chains are what the design table says, so the numbers above mean what they seem.
    #[test]
    fn chains_add_up() {
        let fish = chain(Good::Fish).unwrap();
        assert_eq!(fish.works, vec![(Kind::Fishery, 1.0)]);
        let bread = chain(Good::Bread).unwrap();
        let kinds: Vec<Kind> = bread.works.iter().map(|w| w.0).collect();
        assert!(kinds.contains(&Kind::GrainFarm) && kinds.contains(&Kind::Mill));
        let (houses, _) = half_load(Tier::Fisherfolk, Good::Fish).unwrap();
        assert!((houses - 48.0).abs() < 0.5, "{houses}");
    }

    /// A house that moves up always pays more than it did, so moving up is worth it.
    #[test]
    fn moving_up_pays() {
        for t in Tier::ALL {
            if let Some((next, _)) = t.next() {
                let plain = |t: Tier| -> f32 {
                    t.rent()
                        + t.wants()
                            .iter()
                            .filter(|w| w.basic() && w.unlock == 0.0)
                            .map(|w| w.coins)
                            .sum::<f32>()
                };
                assert!(plain(next) > plain(t) * 1.5, "{t:?}");
            }
        }
    }
}
