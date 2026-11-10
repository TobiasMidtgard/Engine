//! `spells_wilds` (round-17-plan.md, "The hall stop"): the Wilds. Druids on the students' side of
//! the floor (lanes 0 to 2) against the necromancers' dead (lanes 3 to 5), as `spells_dead` sets
//! two sides:
//!
//! * The necromancers raise seven skeleton warriors on their side of the floor (0 to 1.3 s), and
//!   four more mid-charge (9.4 to 10.6 s); they charge for the druids on the dais.
//! * Lane 2 puts on Bark skin (0.3 s): plates of bark grow over it from the feet up.
//! * Lane 2 grows a Thorn hedge across the floor in the charge's way (1.2 s): brambles 1.8 m
//!   high that the dead push through slowly, scratched and snagged.
//! * Lane 1 summons a Treant before the dais (2.0 s): a walking tree that wades into the dead and
//!   smashes them, its slam rooting what it strikes.
//! * Lane 0 Entangles the front of the charge (3.4 s, and the late dead at 13 s): roots break out
//!   of the floor under each skeleton and coil up its legs. Lanes 0 and 2 fire Thornvine seeds
//!   into the dead (5 to 14 s) that burst into roots where they land, and lane 1 entangles again
//!   (8.4 s). The roots and thorns hurt a little: the treant does the killing.
//! * Lane 3 throws a fire bolt through the hedge (10.8 s): fire runs along it both ways and it
//!   burns away, and the late dead come on.
//! * The druids' spells spare their side (Safeguard), so the treant is never rooted by them.

use super::{AimAt, Cast, Lane, d};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;
use crate::summon::MinionKind;

/// When the program starts in its period, s.
pub const START: f32 = 0.2;
/// Seconds between periods: the first dead and the treant last 21.6 s, the late dead 12.6 s.
pub const PERIOD: f32 = 23.0;
/// Where lane 2 grows its hedge (x, z): mid-floor, across the charge's way to the dais.
pub const HEDGE: [f64; 2] = [1.6, 5.4];

/// Lane 0's entangle (Vitalism's Entangle).
pub fn entangle() -> SpellDesign {
    d(&["wilds"], CastType::Area)
        .with(Modifier::Safeguard)
        .named("Entangle")
}

/// A Thornvine seed: it bursts into roots where it lands.
pub fn thornvine() -> SpellDesign {
    d(&["wilds"], CastType::Bolt)
        .with(Modifier::Safeguard)
        .named("Thornvine")
}

/// Lane 2's thorn hedge, standing most of the period unless it is burnt.
pub fn thorn_wall() -> SpellDesign {
    d(&["wilds"], CastType::Wall)
        .with(Modifier::Size { factor: 1.3 })
        .with(Modifier::Duration { factor: 1.6 })
        .with(Modifier::Safeguard)
        .named("Thorn wall")
}

/// Lane 2's bark skin.
pub fn bark_skin() -> SpellDesign {
    d(&["wilds"], CastType::OnSelf).named("Bark skin")
}

/// Lane 1's treant, lasting the period.
pub fn treant() -> SpellDesign {
    d(&["wilds"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.34 })
        .named("Treant")
}

/// Lane 3's fire bolt through the hedge: over its own dead on the way.
fn fire_bolt() -> SpellDesign {
    d(&["fire"], CastType::Bolt)
        .with(Modifier::Safeguard)
        .named("Fire bolt")
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
        .with(Modifier::Duration { factor: 0.21 })
        .named("Raise skeleton")
}

/// The program's lanes: every lane Ascended (no fizzles to spoil the show), druids on lanes 0
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
    // The druids aim at the dead, never the practice dummies down the lanes.
    let dead = AimAt::Kind(MinionKind::Skeleton);
    vec![
        of(vec![
            cast(3.4, entangle(), dead),
            cast(5.0, thornvine(), dead),
            cast(7.0, thornvine(), dead),
            cast(13.0, entangle(), dead),
        ]),
        of(vec![
            cast(2.0, treant(), AimAt::Floor(-1.2, 6.2)),
            cast(8.4, entangle(), dead),
        ]),
        of(vec![
            cast(0.3, bark_skin(), AimAt::Foe),
            cast(1.2, thorn_wall(), AimAt::Floor(HEDGE[0], HEDGE[1])),
            cast(6.0, thornvine(), dead),
            cast(11.8, thornvine(), dead),
            cast(14.2, thornvine(), dead),
        ]),
        // The dead rise in the middle of the necromancers' side of the floor, close enough to
        // the hedge that their charge stays in the picture.
        of(vec![
            cast(0.0, raise(), AimAt::Floor(5.0, 2.5)),
            cast(0.6, raise(), AimAt::Floor(4.4, 0.6)),
            cast(10.8, fire_bolt(), AimAt::At(HEDGE[0], 1.0, HEDGE[1])),
        ]),
        of(vec![
            cast(0.1, raise(), AimAt::Floor(6.2, 1.4)),
            cast(0.7, raise(), AimAt::Floor(6.6, 3.4)),
            cast(9.4, late_raise(), AimAt::Floor(5.8, 2.6)),
            cast(10.0, late_raise(), AimAt::Floor(6.6, 1.6)),
        ]),
        of(vec![
            cast(0.2, raise(), AimAt::Floor(5.6, -0.2)),
            cast(0.9, raise(), AimAt::Floor(7.0, 1.0)),
            cast(1.3, raise(), AimAt::Floor(5.4, 4.4)),
            cast(10.6, late_raise(), AimAt::Floor(6.2, 4.0)),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn the_spells_are_the_wilds_and_last_their_parts() {
        let ctx = CastContext::new(Mastery::Ascended1, Power::at_level(60.0));
        let e = compile(&entangle(), ctx).unwrap();
        assert!(e.payload.roots_secs >= 2.0, "{}", e.payload.roots_secs);
        assert_eq!(e.carrier.duration, 0.0, "an entangle bursts once");
        let seed = compile(&thornvine(), ctx).unwrap();
        assert!(seed.payload.roots_secs > 0.0);
        assert_eq!(seed.stages.len(), 1, "it bursts into roots");
        let wall = compile(&thorn_wall(), ctx).unwrap();
        assert!(wall.payload.hedge);
        // The hedge stands until the fire bolt comes, and is gone by the period's end.
        assert!(
            wall.carrier.duration > 10.8 - 1.2 && wall.carrier.duration < PERIOD - 1.4,
            "{}",
            wall.carrier.duration
        );
        let bark = compile(&bark_skin(), ctx).unwrap();
        assert!(bark.payload.bark_secs >= 15.0, "{}", bark.payload.bark_secs);
        let t = compile(&treant(), ctx).unwrap();
        assert!(t.payload.roots_secs > 0.0);
        assert!(t.carrier.duration > 18.0 && 2.2 + t.carrier.duration < PERIOD);
        for s in [raise(), late_raise()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.carrier.duration > 10.0 && 11.0 + s.carrier.duration < PERIOD + 10.0);
        }
    }
}
