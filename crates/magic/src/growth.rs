//! Known spells grow on their own: as mastery rises, a spell spends its new budget along the
//! growth path the player picked (spell-combination.md section 7). The Fire bolt learnt as a
//! novice becomes, on the Volley path, five homing fireballs that explode and leave burning
//! ground at Master, a rain of fireballs when ascended, and a small sun at God; nobody remakes
//! it by hand. The spellcrafter can always spend the budget by hand instead.

use serde::{Deserialize, Serialize};

use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;

/// How a damage spell spends extra budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Growth {
    /// More projectiles, homing, exploding, leaving burning ground.
    Volley,
    /// One ever bigger explosion that throws people.
    Blast,
    /// Lingering fire that splits and spreads.
    Burn,
    /// A faster, longer, piercing line.
    Lance,
}

impl Growth {
    pub const ALL: [Growth; 4] = [Growth::Volley, Growth::Blast, Growth::Burn, Growth::Lance];

    pub fn name(self) -> &'static str {
        match self {
            Growth::Volley => "Volley",
            Growth::Blast => "Blast",
            Growth::Burn => "Burn",
            Growth::Lance => "Lance",
        }
    }
}

/// `base` grown along `path` to `mastery`. Keeps the base's effects; replaces its cast type and
/// modifiers with the path's step for that tier, always inside the tier's budget.
pub fn grow(base: &SpellDesign, path: Growth, mastery: Mastery) -> SpellDesign {
    use CastType::*;
    use Mastery::*;
    use Modifier::*;
    let (cast, mods): (CastType, Vec<Modifier>) = match (path, mastery) {
        (_, Novice) => (Bolt, vec![]),
        (Growth::Volley, Apprentice) => (Bolt, vec![Size { factor: 1.6 }, Speed { factor: 1.2 }]),
        (Growth::Volley, Adept) => (
            Bolt,
            vec![
                Size { factor: 1.8 },
                Speed { factor: 1.2 },
                Blast { radius_m: 2.5 },
            ],
        ),
        (Growth::Volley, Expert) => (
            Bolt,
            vec![Split { count: 3 }, Seek, Blast { radius_m: 2.5 }],
        ),
        (Growth::Volley, Master) => (
            Bolt,
            vec![
                Split { count: 5 },
                Seek,
                Blast { radius_m: 3.0 },
                Linger {
                    secs: 8.0,
                    radius_m: 2.0,
                },
            ],
        ),
        (Growth::Volley, Ascended1 | Ascended2 | Ascended3) => (
            Rain,
            vec![
                Size { factor: 1.5 },
                Seek,
                Blast { radius_m: 3.5 },
                Linger {
                    secs: 8.0,
                    radius_m: 2.5,
                },
            ],
        ),
        (Growth::Volley, God) | (Growth::Blast, God) => (
            Lob,
            vec![
                Size { factor: 6.0 },
                Blast { radius_m: 30.0 },
                Linger {
                    secs: 20.0,
                    radius_m: 20.0,
                },
                Charge { max_secs: 2.0 },
            ],
        ),
        (Growth::Blast, Apprentice) => (Bolt, vec![Size { factor: 2.0 }]),
        (Growth::Blast, Adept) => (Bolt, vec![Size { factor: 2.0 }, Blast { radius_m: 4.0 }]),
        (Growth::Blast, Expert) => (
            Lob,
            vec![
                Size { factor: 2.5 },
                Blast { radius_m: 7.0 },
                Charge { max_secs: 1.0 },
            ],
        ),
        (Growth::Blast, _) => (
            Lob,
            vec![
                Size { factor: 3.0 },
                Blast { radius_m: 10.0 },
                Charge { max_secs: 1.5 },
                Linger {
                    secs: 6.0,
                    radius_m: 5.0,
                },
            ],
        ),
        (Growth::Burn, Apprentice) => (Bolt, vec![Stacking { threshold: 3 }]),
        (Growth::Burn, Adept) => (
            Bolt,
            vec![
                Stacking { threshold: 3 },
                Linger {
                    secs: 6.0,
                    radius_m: 1.5,
                },
            ],
        ),
        (Growth::Burn, Expert) => (
            Bolt,
            vec![
                Fork { count: 3 },
                Linger {
                    secs: 8.0,
                    radius_m: 2.0,
                },
                Stacking { threshold: 3 },
            ],
        ),
        (Growth::Burn, _) => (
            Wave,
            vec![
                Fork { count: 3 },
                Linger {
                    secs: 10.0,
                    radius_m: 2.5,
                },
                Stacking { threshold: 3 },
                Duration { factor: 2.0 },
            ],
        ),
        (Growth::Lance, Apprentice) => (Bolt, vec![Speed { factor: 1.6 }, Range { factor: 1.4 }]),
        (Growth::Lance, Adept) => (
            Bolt,
            vec![
                Speed { factor: 2.0 },
                Range { factor: 1.6 },
                Pierce { count: 2 },
            ],
        ),
        (Growth::Lance, Expert) => (Beam, vec![Range { factor: 1.6 }, Pierce { count: 3 }]),
        (Growth::Lance, _) => (
            Beam,
            vec![
                Range { factor: 2.5 },
                Pierce { count: 6 },
                Size { factor: 2.0 },
                Linger {
                    secs: 5.0,
                    radius_m: 1.2,
                },
            ],
        ),
    };
    let mut grown = SpellDesign {
        name: base.name.clone(),
        effects: base.effects.clone(),
        cast,
        modifiers: mods,
        stages: base.stages.clone(),
    };
    // A weave of several effects costs more to begin with: drop the step's last parts until it
    // fits (then fall back to a plain bolt).
    while grown.complexity() > mastery.budget() && !grown.modifiers.is_empty() {
        grown.modifiers.pop();
    }
    if grown.complexity() > mastery.budget() {
        grown.cast = CastType::Bolt;
    }
    grown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_fits_its_budget() {
        for path in Growth::ALL {
            for m in Mastery::ALL {
                for fx in [&["fire"][..], &["frost"], &["blood"], &["fire", "storm"]] {
                    let base = SpellDesign::new(fx, CastType::Bolt);
                    let grown = grow(&base, path, m);
                    assert!(
                        grown.complexity() <= m.budget(),
                        "{path:?} {m:?} {fx:?}: {} > {}",
                        grown.complexity(),
                        m.budget()
                    );
                    assert!(grown.problems().is_empty());
                }
            }
        }
    }

    #[test]
    fn the_fire_bolt_grows_into_a_volley() {
        let bolt = SpellDesign::new(&["fire"], CastType::Bolt).named("Fire bolt");
        let master = grow(&bolt, Growth::Volley, Mastery::Master);
        assert_eq!(master.complexity(), 11);
        assert!(master.has(|m| matches!(m, Modifier::Split { count: 5 })));
        let expert = grow(&bolt, Growth::Volley, Mastery::Expert);
        assert!(expert.has(|m| matches!(m, Modifier::Split { count: 3 })));
        assert_eq!(grow(&bolt, Growth::Volley, Mastery::Novice).complexity(), 0);
        assert_eq!(
            grow(&bolt, Growth::Volley, Mastery::Ascended1).cast,
            CastType::Rain
        );
    }
}
