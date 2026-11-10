//! `spells_bones` (round-18-plan.md, "The hall stop"): bone. Necromancers on the dead's side of
//! the floor (lanes 3 to 5) against the students (lanes 0 to 2), as `spells_pacts` sets two
//! sides:
//!
//! * The students raise a stone golem, a storm elemental and a fire elemental mid-floor (0.1 to
//!   0.5 s), and a second golem and storm elemental after the dragon's first pass (9.6 and
//!   10.8 s).
//! * Lane 3 puts on Bone armour (0.3 s): shards of bone swirl up round it and lock on as plates.
//! * Lane 4 raises a Bone dragon on its side of the floor (0.6 s): its bones fly up out of the
//!   floor and lock together over 2.6 s, its heart lights in its ribs, it rears and takes off,
//!   flies a circle over the students' creatures breathing grave frost down on them, lands among
//!   them to bite and sweep its tail, and flies again.
//! * Lane 5 closes a Bone prison over the golem (2.4 s, and again at 13.6 s): eight ribs break
//!   out of the floor round it and close over its head like a hand. Lane 0 shatters it with a
//!   push bolt (5.4 s): a heavy blow from the students' side breaks a prison into tumbling shards.
//! * Lane 3 raises a Bone wall across the floor before the necromancers' plinths (3.6 s): a
//!   ribcage 2.2 m high and 6 m long, pairs of ribs arching up to a spine, that the students'
//!   creatures push through slowly.
//! * Lanes 5 and 3 throw Bone spears through the students' line (5 s on): a barbed spear that
//!   goes through every body in its way and stands quivering in the floor where it lands. Lane
//!   4 cages the storm elemental (11 s).
//! * The students answer with fire and storm: lane 1 throws fire bolts at the dragon (skeletal
//!   dragons fear fire), lane 2 lightning bolts, lane 0 fire at the armoured necromancer (bone
//!   armour turns weapons, not magic).
//! * The necromancers' spells spare their own (Safeguard): the dragon is never caged or snagged
//!   by them, and their spears fly past it. The masters on the plinths have 4000 health this
//!   stop, so none falls.

use super::{AimAt, Cast, Lane, d};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;
use crate::summon::MinionKind;

/// When the program starts in its period, s.
pub const START: f32 = 0.2;
/// Seconds between periods: the dragon and the students' creatures last 21.6 s.
pub const PERIOD: f32 = 24.0;
/// Where lane 4 raises its dragon (x, z): on the necromancers' side of the floor, before the
/// students' creatures.
pub const DRAGON: [f64; 2] = [3.2, 4.6];
/// Where lane 3 raises its bone wall (x, z): before the necromancers' plinths, across the
/// floor (lane 3 casts it straight down its lane).
pub const RIBCAGE: [f64; 2] = [3.4, 7.0];

/// Lane 4's bone dragon, lasting the period.
pub fn bone_dragon() -> SpellDesign {
    d(&["bone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Bone dragon")
}

/// Lane 5's bone prison (and lane 4's).
pub fn prison() -> SpellDesign {
    d(&["bone"], CastType::Area)
        .with(Modifier::Safeguard)
        .named("Bone prison")
}

/// Lanes 3 and 5's bone spear, past the necromancers' own dragon.
pub fn spear() -> SpellDesign {
    d(&["bone"], CastType::Bolt)
        .with(Modifier::Safeguard)
        .named("Bone spear")
}

/// Lane 3's bone wall, 6 m long, standing most of the period.
pub fn bone_wall() -> SpellDesign {
    d(&["bone"], CastType::Wall)
        .with(Modifier::Size { factor: 0.75 })
        .with(Modifier::Duration { factor: 1.6 })
        .with(Modifier::Safeguard)
        .named("Bone wall")
}

/// Lane 3's bone armour.
pub fn bone_armour() -> SpellDesign {
    d(&["bone"], CastType::OnSelf).named("Bone armour")
}

/// Lane 0's push bolt at its own golem's prison: no Safeguard, so it strikes the prison on its
/// own side's golem (a shove that frees it rather than hurts it).
pub fn jailbreak() -> SpellDesign {
    d(&["push"], CastType::Bolt).named("Shatter")
}

fn fire_bolt() -> SpellDesign {
    d(&["fire"], CastType::Bolt)
        .with(Modifier::Size { factor: 1.2 })
        .with(Modifier::Safeguard)
        .named("Fire bolt")
}

fn lightning_bolt() -> SpellDesign {
    d(&["storm"], CastType::Bolt)
        .with(Modifier::Size { factor: 1.2 })
        .with(Modifier::Safeguard)
        .named("Lightning bolt")
}

fn golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Stone golem")
}

fn storm_elemental() -> SpellDesign {
    d(&["storm"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Storm elemental")
}

fn fire_elemental() -> SpellDesign {
    d(&["fire"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.36 })
        .named("Fire elemental")
}

/// The students' second golem and storm elemental, risen after the dragon's first pass (9.6 and
/// 10.8 s) for it to land on, gone before the next period.
fn late_golem() -> SpellDesign {
    d(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.2 })
        .named("Stone golem")
}

fn late_elemental() -> SpellDesign {
    d(&["storm"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.2 })
        .named("Storm elemental")
}

/// The program's lanes: every lane Ascended (no fizzles to spoil the show), students on lanes 0
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
    let dragon = AimAt::Kind(MinionKind::BoneDragon);
    vec![
        // The golem stands mid-floor before the students' plinths; lane 0 breaks it out of its
        // prison, then throws fire at the armoured necromancer.
        of(vec![
            cast(0.1, golem(), AimAt::Floor(-0.6, 6.0)),
            cast(5.4, jailbreak(), AimAt::Own(MinionKind::Golem)),
            cast(8.0, fire_bolt(), AimAt::Mage(3)),
            cast(9.6, late_golem(), AimAt::Floor(-0.8, 5.8)),
            cast(16.4, jailbreak(), AimAt::Own(MinionKind::Golem)),
            cast(18.6, fire_bolt(), AimAt::Mage(3)),
        ]),
        // Fire at the dragon, in the air and on the floor.
        of(vec![
            cast(0.5, fire_elemental(), AimAt::Floor(-2.0, 3.8)),
            cast(6.4, fire_bolt(), dragon),
            cast(8.8, fire_bolt(), dragon),
            cast(11.2, fire_bolt(), dragon),
            cast(14.0, fire_bolt(), dragon),
            cast(17.0, fire_bolt(), dragon),
        ]),
        of(vec![
            cast(0.3, storm_elemental(), AimAt::Floor(-1.2, 5.2)),
            cast(7.4, lightning_bolt(), dragon),
            cast(10.0, lightning_bolt(), dragon),
            cast(10.8, late_elemental(), AimAt::Floor(-1.4, 4.6)),
            cast(12.6, lightning_bolt(), dragon),
            cast(15.4, lightning_bolt(), dragon),
        ]),
        // Armour on, then the ribcage before the necromancers' plinths (straight down its lane,
        // so it runs across the floor), then spears.
        of(vec![
            cast(0.3, bone_armour(), AimAt::Foe),
            cast(3.6, bone_wall(), AimAt::Floor(RIBCAGE[0], RIBCAGE[1])),
            cast(8.0, spear(), AimAt::Kind(MinionKind::Elemental)),
            cast(14.6, spear(), AimAt::Golem),
        ]),
        of(vec![
            cast(0.6, bone_dragon(), AimAt::Floor(DRAGON[0], DRAGON[1])),
            cast(11.0, prison(), AimAt::Kind(MinionKind::Elemental)),
        ]),
        // The prisons, and spears from the far end of the dais across the students' line.
        of(vec![
            cast(2.4, prison(), AimAt::Golem),
            cast(5.0, spear(), AimAt::Kind(MinionKind::Elemental)),
            cast(10.0, spear(), AimAt::Golem),
            cast(13.6, prison(), AimAt::Golem),
            cast(18.0, spear(), AimAt::Kind(MinionKind::Elemental)),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;
    use crate::summon::kind_of;

    #[test]
    fn the_spells_are_bone_and_last_their_parts() {
        let ctx = CastContext::new(Mastery::Ascended1, Power::at_level(60.0));
        let dragon = compile(&bone_dragon(), ctx).unwrap();
        assert_eq!(kind_of(&dragon), Some(MinionKind::BoneDragon));
        assert!(
            dragon.carrier.duration > 18.0 && 1.4 + dragon.carrier.duration < PERIOD,
            "{}",
            dragon.carrier.duration
        );
        let p = compile(&prison(), ctx).unwrap();
        assert!(p.payload.prison_secs >= 3.0, "{}", p.payload.prison_secs);
        // The golem is still caged when lane 0 breaks it out.
        assert!(2.4 + p.payload.prison_secs > 5.4 + 0.6);
        let s = compile(&spear(), ctx).unwrap();
        assert!(s.carrier.pierce >= crate::bones::SPEAR_PIERCE);
        let w = compile(&bone_wall(), ctx).unwrap();
        assert!(w.payload.ribcage);
        assert!(
            w.carrier.duration > 12.0 && 3.8 + w.carrier.duration < PERIOD,
            "{}",
            w.carrier.duration
        );
        let a = compile(&bone_armour(), ctx).unwrap();
        assert!(a.payload.armour_secs >= 15.0, "{}", a.payload.armour_secs);
        let j = compile(&jailbreak(), ctx).unwrap();
        assert!(
            j.payload
                .force
                .is_some_and(|(_, i)| i >= crate::bones::SHATTER_IMPULSE),
            "{:?}",
            j.payload.force
        );
    }
}
