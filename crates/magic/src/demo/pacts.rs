//! `spells_pacts` (round-15-plan.md, "The hall stop"): pacts. Warlocks on the dead's side of the
//! floor (lanes 3 to 5) against the students (lanes 0 to 2), as `spells_blades` sets two sides:
//!
//! * The students raise a stone golem and a frost elemental mid-floor (0.3 and 0.5 s).
//! * Lane 3 opens a Hellgate on the floor before its plinth (0.6 s), facing down the hall: a ring
//!   of black horned stone 3 m tall round a churning red-black membrane. Two hellhounds leap out
//!   of it (about 2.2 and 3.0 s), run at the students' creatures and pounce on them, then an imp
//!   comes every 2.5 s that flies up and throws firebolts.
//! * Lane 3 binds the students' creatures together with Fatal bonds (3.5 s; short hops, so the
//!   chain stays on the floor and perhaps one master on the dais): every bite on one hurts them
//!   all. Then it drags the elemental in on a Chain hook (5.3 s, and the second one at 14 s).
//! * Lane 4 binds the golem in chains out of a burning sigil on the floor (4.7 s) while the
//!   hounds tear at it, and the second golem too (13.5 s).
//! * Lane 5 lobs a Demonic meteor onto the golem (7.4 s) and another before the students'
//!   plinths (15.8 s): a burning black rock that bursts where it lands and lets three imps out
//!   of the crater.
//! * The students turn the fight: lane 1 calls a spirit blade (7.8 s) and raises a second golem
//!   (9.4 s), lane 2 a phalanx over its head (9.0 and 13.8 s) and a second elemental (11 s), lane
//!   1 drops frost bladefalls on the demons (11 and 15.8 s), and lane 0 throws frost bolts and
//!   glintblades at the gate until it breaks (about 14 s). The last imps are hunted down by 20 s.
//! * The meteors, the hook, the bonds and the binding spare the warlocks' own (Safeguard: they fly
//!   past their imps). At the end of each period what is left of the pact falls in fire
//!   (`demons::dismiss`); the masters on the plinths have 4000 health this stop, so none falls.

use super::{AimAt, Cast, Lane, d};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;
use crate::summon::MinionKind;

/// When the program starts in its period, s.
pub const START: f32 = 0.2;
/// Seconds between periods: the students' creatures last 21.6 s, the gate stands until it is
/// broken, and what is left of the pact falls at the end.
pub const PERIOD: f32 = 24.0;
/// Where lane 3 opens its gate (x, z), on the floor before its plinth, facing down the hall.
pub const GATE: [f64; 2] = [3.4, 5.6];

/// Lane 3's hellgate: it stands most of the period unless it is broken; its demons stay until
/// the period ends.
pub fn hellgate() -> SpellDesign {
    d(&["demon"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.95 })
        .named("Hellgate")
}

/// Lane 4's binding: chains out of a sigil under the golem.
pub fn binding() -> SpellDesign {
    d(&["chains"], CastType::Area)
        .with(Modifier::Safeguard)
        .named("Binding chains")
}

/// Lane 3's chain hook.
pub fn hook() -> SpellDesign {
    // Through the warlocks' own imps on the way.
    d(&["chains"], CastType::Bolt)
        .with(Modifier::Safeguard)
        .named("Chain hook")
}

/// Lane 3's fatal bonds: a chain that hops from body to body and binds their fates; short hops,
/// so it binds the students' creatures on the floor and not their masters up on the dais.
pub fn bonds() -> SpellDesign {
    d(&["chains"], CastType::Chain)
        .with(Modifier::Range { factor: 0.45 })
        .with(Modifier::Safeguard)
        .named("Fatal bonds")
}

/// Lane 5's demonic meteor, lobbed in over the students' creatures; its imps stay until the
/// period ends.
pub fn meteor() -> SpellDesign {
    // Over the warlocks' own imps on the way.
    d(&["demon"], CastType::Lob)
        .with(Modifier::Safeguard)
        .named("Demonic meteor")
}

/// The students' answer: lane 2's phalanx (as in `spells_blades`).
fn phalanx() -> SpellDesign {
    super::blades::phalanx()
}

/// Lane 1's frost bladefall, sparing the students' creatures.
fn bladefall() -> SpellDesign {
    super::blades::bladefall()
}

/// Lane 0's frost bolt at the gate.
fn frost_bolt() -> SpellDesign {
    d(&["frost"], CastType::Bolt)
        .with(Modifier::Size { factor: 1.3 })
        .named("Frost bolt")
}

/// Lane 2's spirit blade, fighting beside the golem.
fn spirit_blade() -> SpellDesign {
    d(&["blade"], CastType::Summon)
        .with(Modifier::Size { factor: 1.4 })
        .named("Spirit blade")
}

fn golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Stone golem")
}

fn elemental() -> SpellDesign {
    d(&["frost"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Frost elemental")
}

/// The students' second golem and elemental, risen as they turn the fight (9.4 to 11 s), gone
/// before the next period.
fn late_golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.2 })
        .named("Stone golem")
}

fn late_elemental() -> SpellDesign {
    d(&["frost"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.2 })
        .named("Frost elemental")
}

/// The program's lanes: every lane Ascended (no fizzles to spoil the show), students on lanes 0
/// to 2 and warlocks on 3 to 5.
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
    vec![
        // The golem stands mid-floor before the students' plinths; then lane 0 goes for the
        // gate, with frost and then blades.
        of(vec![
            cast(0.1, golem(), AimAt::Floor(-0.4, 6.2)),
            cast(9.6, frost_bolt(), AimAt::Gate),
            cast(10.8, frost_bolt(), AimAt::Gate),
            cast(12.0, frost_bolt(), AimAt::Gate),
            cast(13.2, super::blades::glintblades(), AimAt::Gate),
            cast(14.4, frost_bolt(), AimAt::Gate),
            cast(15.6, super::blades::glintblades(), AimAt::Gate),
            cast(16.8, frost_bolt(), AimAt::Gate),
            cast(18.0, super::blades::glintblades(), AimAt::Gate),
        ]),
        // The spirit blade comes as the students turn the fight.
        of(vec![
            cast(7.6, spirit_blade(), AimAt::Floor(-0.6, 4.4)),
            cast(9.2, late_golem(), AimAt::Floor(-0.6, 6.0)),
            cast(10.8, bladefall(), AimAt::Foe),
            cast(15.6, bladefall(), AimAt::Foe),
        ]),
        // The elemental keeps near its summoner: lane 2's, nearest the gate.
        of(vec![
            cast(0.3, elemental(), AimAt::Floor(-1.2, 5.0)),
            cast(8.8, phalanx(), AimAt::Foe),
            cast(10.8, late_elemental(), AimAt::Floor(-1.0, 4.8)),
            cast(13.6, phalanx(), AimAt::Foe),
        ]),
        // The gate opens before lane 3's plinth; the hook drags the elemental in.
        of(vec![
            cast(0.2, hellgate(), AimAt::Floor(GATE[0], GATE[1])),
            // The bonds first, while the students' three stand together.
            cast(2.6, bonds(), AimAt::Kind(MinionKind::Golem)),
            cast(4.2, hook(), AimAt::Kind(MinionKind::Elemental)),
            cast(13.0, hook(), AimAt::Kind(MinionKind::Elemental)),
        ]),
        of(vec![
            cast(3.6, binding(), AimAt::Golem),
            cast(12.4, binding(), AimAt::Golem),
        ]),
        of(vec![
            cast(5.2, meteor(), AimAt::Golem),
            // The second comes down before the students' plinths, whoever stands there.
            cast(13.6, meteor(), AimAt::Floor(-0.8, 6.0)),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn the_spells_are_pacts_and_the_students_answers() {
        let ctx = CastContext::new(Mastery::Ascended1, Power::at_level(60.0));
        let gate = compile(&hellgate(), ctx).unwrap();
        assert!(gate.payload.demons >= 2, "{}", gate.payload.demons);
        // The gate and its demons stand most of the period, and are gone at its end.
        assert!(
            gate.payload.demon_secs > 15.0 && gate.payload.demon_secs < PERIOD - 1.0,
            "{}",
            gate.payload.demon_secs
        );
        assert!(gate.carrier.duration > 12.0, "{}", gate.carrier.duration);
        let bind = compile(&binding(), ctx).unwrap();
        assert!(bind.payload.bind_secs >= 3.0, "{}", bind.payload.bind_secs);
        let hook = compile(&hook(), ctx).unwrap();
        assert!(hook.payload.hook);
        let bonds = compile(&bonds(), ctx).unwrap();
        assert!(
            bonds.payload.bond_secs >= 10.0,
            "{}",
            bonds.payload.bond_secs
        );
        assert!(
            bonds.carrier.chain_radius < 5.5,
            "{}",
            bonds.carrier.chain_radius
        );
        // Lane 3's plinth to the golem's spot.
        assert!(
            bonds.carrier.range.max(bonds.carrier.chain_radius) > 7.0,
            "{}",
            bonds.carrier.range
        );
        let meteor = compile(&meteor(), ctx).unwrap();
        assert_eq!(meteor.cast, CastType::Lob);
        assert!(meteor.payload.demons >= 3, "{}", meteor.payload.demons);
        assert!(
            meteor.payload.demon_secs > 10.0,
            "{}",
            meteor.payload.demon_secs
        );
        for s in [golem(), elemental()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.carrier.duration >= 18.0 && s.carrier.duration < PERIOD - 2.0);
        }
        for s in [late_golem(), late_elemental()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.carrier.duration > 8.0 && 11.8 + s.carrier.duration < PERIOD);
        }
        assert!(compile(&frost_bolt(), ctx).unwrap().payload.damage > 0.0);
    }
}
