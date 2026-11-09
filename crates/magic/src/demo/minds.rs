//! `spells_minds` (round-13-plan.md, "The hall stop"): the necromancers' dead against students
//! who answer with minds alone, as `spells_dead` sets two sides on the floor:
//!
//! * The necromancers (lanes 3 to 5) raise a stone golem and five skeleton warriors on the
//!   right of the floor; with no creature of the students' to fight they make for the dais.
//! * Lane 1 sets three Decoys (copies of itself) on the floor in their way (2.4 s): the
//!   skeletons turn on the copies and cut them down one by one, each breaking into violet glass.
//! * Lane 2 takes a Mirror image (3.6 s): four copies step out of it on the dais to its sides.
//! * Lane 1 throws Frenzy where the skeletons crowd (6.8 s): they hack at each other.
//! * Lane 2 Commands the golem (9.2 s): it glows magenta on a thread of light from the
//!   student's hand and smashes the skeletons it rose with, then goes for its own necromancer.
//! * Four more skeletons climb out mid-floor, around the golem, to fight it (10 to 11 s),
//!   and lane 0 rolls a Terror wave across the floor (11.1 s): the dead it passes run, dread
//!   over their heads, then come back to the golem's fists. The wave spares the golem, which is
//!   the students' while it serves.

use super::{AimAt, Cast, Lane, d};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;

/// When the program starts in its period, s.
pub const START: f32 = 0.2;
/// Seconds between periods: the first dead last 21.6 s, the second 12.6 s and the command
/// 12 s, all gone by the end.
pub const PERIOD: f32 = 24.0;

/// Lane 1's decoys: three copies on the floor (#38, Split 3).
pub fn decoy() -> SpellDesign {
    d(&["phantasm"], CastType::Summon)
        .with(Modifier::Split { count: 3 })
        .named("Decoy")
}

/// Lane 2's mirror image (#28).
pub fn mirror() -> SpellDesign {
    d(&["phantasm"], CastType::OnSelf).named("Mirror image")
}

/// Lane 0's terror wave (#54): broad and long enough to cross the floor, with a short fear so
/// the dead come back in view; it spares the students' side (and a commanded golem).
pub fn terror() -> SpellDesign {
    d(&["fear"], CastType::Wave)
        .with(Modifier::Size { factor: 2.0 })
        .with(Modifier::Range { factor: 1.3 })
        .with(Modifier::Duration { factor: 0.35 })
        .with(Modifier::Safeguard)
        .named("Terror wave")
}

/// Lane 2's command (#17).
pub fn command() -> SpellDesign {
    d(&["command"], CastType::Bolt).named("Command")
}

/// Lane 1's frenzy (#31); it spares the students' side (and the commanded golem).
pub fn frenzy() -> SpellDesign {
    d(&["wrath"], CastType::Area)
        .with(Modifier::Safeguard)
        .named("Frenzy")
}

/// The necromancers' dead, lasting the period.
fn raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Raise skeleton")
}

/// The second wave of dead, raised at 10 to 11 s, gone before the next period.
fn late_raise() -> SpellDesign {
    d(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.21 })
        .named("Raise skeleton")
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
    // The dead go for the decoys (3 to 7 s); the frenzy takes them where they crowd, the golem
    // is turned on the last of them, and when four more climb out to fight it the terror wave
    // (which spares it, now the students') sends them running down the floor until the fear
    // wears off (1.4 s) and they come back to it.
    vec![
        of(vec![cast(10.9, terror(), AimAt::Floor(5.0, 3.5))]),
        of(vec![
            cast(2.2, decoy(), AimAt::Floor(0.5, 3.0)),
            cast(6.6, frenzy(), AimAt::Foe),
        ]),
        of(vec![
            cast(3.4, mirror(), AimAt::Foe),
            cast(9.0, command(), AimAt::Golem),
        ]),
        of(vec![cast(0.0, golem(), AimAt::Floor(6.5, 5.0))]),
        of(vec![
            cast(0.1, raise(), AimAt::Floor(5.5, 2.5)),
            cast(0.7, raise(), AimAt::Floor(5.0, 0.2)),
            cast(1.3, raise(), AimAt::Floor(6.8, 1.2)),
            cast(10.0, late_raise(), AimAt::Floor(7.2, 4.4)),
            cast(10.6, late_raise(), AimAt::Floor(6.6, 5.6)),
        ]),
        of(vec![
            cast(0.2, raise(), AimAt::Floor(7.8, 3.2)),
            cast(0.8, raise(), AimAt::Floor(7.4, -0.4)),
            cast(9.4, late_raise(), AimAt::Floor(6.5, 2.6)),
            cast(10.2, late_raise(), AimAt::Floor(7.6, 1.6)),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn the_spells_are_copies_terror_command_and_frenzy() {
        let ctx = CastContext::new(Mastery::Ascended1, Power::at_level(60.0));
        let dec = compile(&decoy(), ctx).unwrap();
        assert_eq!(dec.payload.phantasms, 3);
        let mir = compile(&mirror(), ctx).unwrap();
        assert_eq!(mir.payload.phantasms, 4);
        let t = compile(&terror(), ctx).unwrap();
        assert!((1.2..2.0).contains(&t.payload.fear_secs) && t.safeguard && t.carrier.range > 22.0);
        let c = compile(&command(), ctx).unwrap();
        assert!(c.payload.command_secs >= 12.0);
        let f = compile(&frenzy(), ctx).unwrap();
        assert!(f.payload.wrath_secs > 0.0 && f.payload.command_secs == 0.0);
        for s in [raise(), golem()] {
            let s = compile(&s, ctx).unwrap();
            assert!(s.carrier.duration >= 18.0 && s.carrier.duration < PERIOD - 2.0);
        }
        let late = compile(&late_raise(), ctx).unwrap();
        assert!(late.carrier.duration > 8.0 && 11.0 + late.carrier.duration < PERIOD);
    }
}
