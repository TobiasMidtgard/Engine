//! `spells_life` (round-19-plan.md, "Hall stop"): healing you can see. Healers on the students'
//! side of the floor (lanes 0 to 2) keep a stone golem and a treant standing against the dead
//! the necromancers raise (lanes 3 to 5):
//!
//! * Lane 0 summons a stone golem mid-floor (0.1 s) and lane 1 a treant beside it (0.4 s), and
//!   lane 1 a wisp that flies to whichever of them is most hurt (1.0 s).
//! * The necromancers raise eight skeleton warriors on their side (0 to 1.3 s, and three more at
//!   7 s) that charge the golem and treant. Under their blades the golem sheds a shoulder plate,
//!   the other, then its left arm from the elbow.
//! * Lane 2 lays a healing circle under the golem (4.4 s): a ring of light on the floor,
//!   blossoms opening across it from the middle out, a wave of light every second. It heals the
//!   golem, so its pieces fly back up off the floor into place, and it burns the skeletons that
//!   stand in it, gold fire off their bones.
//! * Lane 0 throws mending bolts at the golem (6.6 and 8.8 s): a ring of light runs up it.
//! * The necromancers throw bone spears at the golem (10.8 to 12.6 s) and it falls apart.
//!   Lane 0 revives it (14 s): a mending bolt on its pieces, which rise off the floor and fly
//!   together, and it stands again at half health; lane 2's second circle lands on it (15.4 s).
//! * Lane 1 lets healing rain fall over the treant and the dead (11.6 s): drops of light that
//!   mend the treant and burn the skeletons.
//! * The masters on the plinths have 4000 health this stop, so none falls.

use super::{AimAt, Cast, Lane, d};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;
use crate::summon::MinionKind;

/// When the program starts in its period, s.
pub const START: f32 = 0.2;
/// Seconds between periods.
pub const PERIOD: f32 = 24.0;
/// Where the golem stands (x, z).
pub const GOLEM: [f64; 2] = [-1.0, 5.4];
/// Where the treant stands (x, z).
pub const TREANT: [f64; 2] = [-3.4, 3.6];

fn golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Stone golem")
}

fn treant() -> SpellDesign {
    d(&["wilds"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.34 })
        .named("Treant")
}

/// Lane 1's wisp, lasting the period.
pub fn wisp() -> SpellDesign {
    d(&["heal"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Wisp")
}

/// Lane 2's healing circle: 3.2 m across the golem's ground for 7 s.
pub fn circle() -> SpellDesign {
    d(&["heal"], CastType::Area)
        .with(Modifier::Linger {
            secs: 7.0,
            radius_m: 3.2,
        })
        .named("Healing circle")
}

/// Lane 0's mending bolt (and its revive).
pub fn mending() -> SpellDesign {
    d(&["heal"], CastType::Bolt).named("Mending bolt")
}

/// Lane 1's healing rain.
pub fn rain() -> SpellDesign {
    d(&["heal"], CastType::Rain)
        .with(Modifier::Size { factor: 1.2 })
        .named("Healing rain")
}

/// The necromancers' dead, lasting the period.
fn raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Raise skeleton")
}

/// The late dead, gone before the next period.
fn late_raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.24 })
        .named("Raise skeleton")
}

/// The necromancers' spears to bring the golem down.
pub fn spear() -> SpellDesign {
    d(&["bone"], CastType::Bolt)
        .with(Modifier::Size { factor: 1.3 })
        .with(Modifier::Safeguard)
        .named("Bone spear")
}

/// The program's lanes: every lane Ascended (no fizzles to spoil the show), healers on lanes 0
/// to 2 and necromancers on 3 to 5.
pub(super) fn lanes() -> Vec<Lane> {
    let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
        at: at + START,
        design: Some(design),
        aim,
    };
    let of = |casts: Vec<Cast>| Lane {
        mastery: Mastery::Ascended1,
        level: 60.0,
        known: None,
        casts,
    };
    let own_golem = AimAt::Own(MinionKind::Golem);
    vec![
        of(vec![
            cast(0.1, golem(), AimAt::Floor(GOLEM[0], GOLEM[1])),
            cast(6.6, mending(), own_golem),
            cast(8.8, mending(), own_golem),
            cast(14.0, mending(), AimAt::Fallen),
            cast(18.4, mending(), own_golem),
        ]),
        of(vec![
            cast(0.4, treant(), AimAt::Floor(TREANT[0], TREANT[1])),
            cast(1.0, wisp(), AimAt::Floor(-2.2, 2.4)),
            cast(11.6, rain(), AimAt::Own(MinionKind::Treant)),
        ]),
        of(vec![
            cast(4.4, circle(), own_golem),
            cast(15.4, circle(), own_golem),
        ]),
        // The dead rise in the middle of the necromancers' side of the floor and charge.
        of(vec![
            cast(0.0, raise(), AimAt::Floor(3.6, 4.0)),
            cast(0.6, raise(), AimAt::Floor(4.4, 6.0)),
            cast(7.0, late_raise(), AimAt::Floor(3.8, 5.0)),
            cast(11.4, spear(), AimAt::Golem),
        ]),
        of(vec![
            cast(0.1, raise(), AimAt::Floor(4.8, 3.0)),
            cast(0.7, raise(), AimAt::Floor(5.2, 5.2)),
            cast(7.2, late_raise(), AimAt::Floor(4.6, 3.6)),
            cast(10.8, spear(), AimAt::Golem),
            cast(12.0, spear(), AimAt::Golem),
        ]),
        of(vec![
            cast(0.2, raise(), AimAt::Floor(4.0, 2.2)),
            cast(0.9, raise(), AimAt::Floor(5.6, 4.2)),
            cast(1.3, raise(), AimAt::Floor(3.4, 6.4)),
            cast(7.4, late_raise(), AimAt::Floor(4.2, 6.6)),
            cast(11.2, spear(), AimAt::Golem),
            cast(12.6, spear(), AimAt::Golem),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn the_spells_heal_and_last_their_parts() {
        let ctx = CastContext::new(Mastery::Ascended1, Power::at_level(60.0));
        for s in [circle(), mending(), rain()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.payload.heal > 0.0 && !s.payload.harmful());
        }
        let c = compile(&circle(), ctx).unwrap();
        assert!(
            c.stages_on(crate::design::Trigger::Impact)
                .any(|st| st.payload.heal > 0.0 && st.carrier.duration >= 6.0),
            "it lingers 7 s"
        );
        for s in [golem(), treant(), wisp(), raise()] {
            let s = compile(&s, ctx).unwrap();
            assert!(
                s.carrier.duration > 18.0 && 1.4 + s.carrier.duration < PERIOD,
                "{}",
                s.carrier.duration
            );
        }
        let late = compile(&late_raise(), ctx).unwrap();
        assert!(
            7.6 + late.carrier.duration < PERIOD,
            "{}",
            late.carrier.duration
        );
    }
}
