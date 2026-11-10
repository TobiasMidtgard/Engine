//! `spells_blades` (round-14-plan.md, "The hall stop"): spellblades. The necromancers' dead
//! carry fire on their weapons; the students answer with blades of hard light, as
//! `spells_dead` sets two sides on the floor:
//!
//! * The necromancers (lanes 3 to 5) raise a stone golem and five skeleton warriors mid-floor,
//!   then each Imbues its creatures with fire (1.8 to 2.2 s): the skeletons' blades and the
//!   golem's fists burn orange, and every cut they land sets fire.
//! * Lane 2 raises a Greatblade phalanx (2.4 s): five glintstone-blue swords in an arch over its
//!   head that loose themselves, one after another, at whatever of the dead comes within 9 m.
//! * Lane 1 calls two Spirit blades (3.0 s) that fly out and fence with the skeletons from either
//!   flank, cutting from a new angle each time; the dead strike back and can shatter them. Then
//!   it drops a frost Bladefall down the line of the dead (4.4 s): twenty pale swords fall point
//!   first one after another across the floor and stand in it.
//! * Lane 0, at the far end of the students' line (off the right of the picture: the camera looks
//!   +z, so -x is right), throws fans of three Glintblades at the golem (3.6 to 8.2 s), each
//!   breaking into shards where it strikes.
//! * Lane 2 raises a phalanx for each wave (2.4, 7, 12.2 and 16.4 s). Four more skeletons climb
//!   out (9.4 to 10.6 s) and are imbued with fire again; lane 2 takes a Bound blade in its hand
//!   (10.4 s), lane 1 drops another bladefall on them (12.6 s) and lane 0 throws glintblades at
//!   them (11.4, 13 and 15 s).
//! * A third wave (a golem and three skeletons, 14.6 to 15.8 s) burns too; glintblades go at its
//!   golem (16.6 and 18.4 s) and a last bladefall comes down on it (19.6 s). It is gone before
//!   the period ends.
//! * The bladefalls spare the students (Safeguard), and so does the steam their frost makes on
//!   the burning dead. The masters on the plinths have 4000 health this stop, so none falls.
//!
//! The dead rise mid-floor between the two sides (x -0.6 to 2.8, z 5.8 to 7.4), so the whole
//! fight stays in the stop's one picture; the golems rise nearer the necromancers and meet the
//! spirit blades before they reach a plinth (a master a golem knocks off its plinth goes limp).

use super::{AimAt, Cast, Lane, d};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;

/// When the program starts in its period, s.
pub const START: f32 = 0.2;
/// Seconds between periods: the first dead last 21.6 s, the second 12.6 s, the third 7.8 s, and
/// the blades are gone (or cleared) by the end.
pub const PERIOD: f32 = 24.0;

/// The necromancers' fire on their creatures' weapons (and their own hands).
pub fn fire_weapon() -> SpellDesign {
    d(&["fire"], CastType::Imbue).named("Fire weapon")
}

/// Lane 2's phalanx: five glintstone greatblades over the head (an Ascended's; Elden Ring's
/// Greatblade Phalanx).
pub fn phalanx() -> SpellDesign {
    d(&["blade"], CastType::Orbit)
        .with(Modifier::Size { factor: 1.5 })
        .named("Greatblade phalanx")
}

/// Lane 1's spirit blades: two big ones, fighting beside it.
pub fn spirit_blades() -> SpellDesign {
    d(&["blade"], CastType::Summon)
        .with(Modifier::Split { count: 2 })
        .with(Modifier::Size { factor: 1.4 })
        .named("Spirit blades")
}

/// Lane 0's fan of three glintblades.
pub fn glintblades() -> SpellDesign {
    d(&["blade"], CastType::Bolt)
        .with(Modifier::Split { count: 3 })
        .named("Glintblades")
}

/// A frost bladefall: twenty pale blades down the line from the caster to its aim, sparing
/// friends.
pub fn bladefall() -> SpellDesign {
    d(&["blade", "frost"], CastType::Rain)
        .with(Modifier::Split { count: 2 })
        // It comes down where the dead reach the students' plinths: it spares them, and so
        // does the steam its frost makes on the burning dead.
        .with(Modifier::Safeguard)
        .named("Frost bladefall")
}

/// Lane 2's bound blade: a sword of light in its hand.
pub fn bound_blade() -> SpellDesign {
    d(&["blade"], CastType::OnSelf).named("Bound blade")
}

/// The necromancers' dead, lasting the period.
fn raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Raise skeleton")
}

/// The second wave of dead, raised at 9.4 to 10.6 s, gone before the next period.
fn late_raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.21 })
        .named("Raise skeleton")
}

/// The third wave, raised at 14.6 to 15.8 s, gone before the next period.
fn last_raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.13 })
        .named("Raise skeleton")
}

/// The third wave's golem.
fn last_golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.13 })
        .named("Stone golem")
}

fn golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Stone golem")
}

/// The program's lanes: every lane Ascended (no fizzles to spoil the show), as in
/// `spells_dead`, students on lanes 0 to 2 and necromancers on 3 to 5.
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
        // Lane 0 stands at the far end of the line, off the right of the picture: its
        // glintblades fly into it.
        of(vec![
            cast(3.6, glintblades(), AimAt::Golem),
            cast(5.0, glintblades(), AimAt::Golem),
            cast(6.6, glintblades(), AimAt::Golem),
            cast(8.2, glintblades(), AimAt::Golem),
            cast(11.4, glintblades(), AimAt::Foe),
            cast(13.0, glintblades(), AimAt::Foe),
            cast(15.0, glintblades(), AimAt::Foe),
            cast(16.6, glintblades(), AimAt::Golem),
            cast(18.4, glintblades(), AimAt::Golem),
        ]),
        of(vec![
            cast(3.0, spirit_blades(), AimAt::Floor(-6.0, 6.5)),
            cast(4.4, bladefall(), AimAt::Foe),
            cast(12.6, bladefall(), AimAt::Foe),
            cast(19.6, bladefall(), AimAt::Foe),
        ]),
        // A phalanx for each wave, and one more as the second wave comes on.
        of(vec![
            cast(2.4, phalanx(), AimAt::Foe),
            cast(7.0, phalanx(), AimAt::Foe),
            cast(10.4, bound_blade(), AimAt::Foe),
            cast(12.2, phalanx(), AimAt::Foe),
            cast(16.4, phalanx(), AimAt::Foe),
        ]),
        // The dead rise mid-floor before the plinths, between the two sides, near enough to
        // the camera that the whole fight stays in one picture.
        of(vec![
            cast(0.0, golem(), AimAt::Floor(3.4, 8.0)),
            cast(1.8, fire_weapon(), AimAt::Foe),
            cast(14.6, last_golem(), AimAt::Floor(3.2, 7.8)),
            cast(16.2, fire_weapon(), AimAt::Foe),
        ]),
        of(vec![
            cast(0.1, raise(), AimAt::Floor(0.4, 6.2)),
            cast(0.7, raise(), AimAt::Floor(-0.6, 6.0)),
            cast(1.3, raise(), AimAt::Floor(1.2, 5.8)),
            cast(2.0, fire_weapon(), AimAt::Foe),
            cast(10.0, late_raise(), AimAt::Floor(1.2, 6.6)),
            cast(10.6, late_raise(), AimAt::Floor(0.0, 7.2)),
            cast(11.6, fire_weapon(), AimAt::Foe),
            cast(15.0, last_raise(), AimAt::Floor(0.6, 6.4)),
            cast(16.4, fire_weapon(), AimAt::Foe),
        ]),
        of(vec![
            cast(0.2, raise(), AimAt::Floor(2.4, 6.6)),
            cast(0.8, raise(), AimAt::Floor(2.8, 7.4)),
            cast(2.2, fire_weapon(), AimAt::Foe),
            cast(9.4, late_raise(), AimAt::Floor(-0.4, 6.2)),
            cast(10.2, late_raise(), AimAt::Floor(2.0, 6.0)),
            cast(11.8, fire_weapon(), AimAt::Foe),
            cast(15.4, last_raise(), AimAt::Floor(2.4, 6.6)),
            cast(15.8, last_raise(), AimAt::Floor(-0.2, 6.6)),
            cast(16.6, fire_weapon(), AimAt::Foe),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn the_spells_are_blades_and_a_fire_imbue() {
        let ctx = CastContext::new(Mastery::Ascended1, Power::at_level(60.0));
        let ph = compile(&phalanx(), ctx).unwrap();
        assert_eq!(ph.payload.blades, 5);
        assert!(ph.payload.blade_secs >= 12.0 && ph.payload.blade_m > 1.2);
        let sp = compile(&spirit_blades(), ctx).unwrap();
        assert_eq!((sp.cast, sp.payload.blades), (CastType::Summon, 2));
        let gl = compile(&glintblades(), ctx).unwrap();
        assert_eq!((gl.cast, gl.payload.blades), (CastType::Bolt, 3));
        let bf = compile(&bladefall(), ctx).unwrap();
        assert_eq!(bf.payload.blades, 20);
        let bb = compile(&bound_blade(), ctx).unwrap();
        assert_eq!(bb.payload.blades, 1);
        let fw = compile(&fire_weapon(), ctx).unwrap();
        assert!(fw.payload.imbue_secs >= 20.0 && fw.payload.damage > 0.0);
        for s in [raise(), golem()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.carrier.duration >= 18.0 && s.carrier.duration < PERIOD - 2.0);
        }
        let late = compile(&late_raise(), ctx).unwrap();
        assert!(late.carrier.duration > 8.0 && 11.0 + late.carrier.duration < PERIOD);
        for s in [last_raise(), last_golem()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.carrier.duration > 6.0 && 16.0 + s.carrier.duration < PERIOD);
        }
    }
}
