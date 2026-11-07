//! `spells_kinesis` (round-10-plan.md, "The hall stop"): telekinesis down three lanes.
//!
//! * Lane 2 (the gravity gun): lifts the barrel and then the crate lying in its line and throws
//!   them at its dummy; in the second half it tears a ring of stones round itself that stops
//!   the bolts lanes 1 and 3 fire at it, then flings the ring down the hall.
//! * Lane 3 (Control's Launch, BioShock's catch): aims at its dummy, out of reach, so a stone
//!   tears out of the floor in front of it, comes to its hand and flies at the dummy; then lane
//!   4 fires a bolt across its line, and lane 3 snatches it out of the air, holds it spinning
//!   before its hand and throws it at its own dummy; then a fire grip's burning stone.
//! * Lane 4 (The Force Unleashed): lifts the loose dummy standing in front of it, tumbling,
//!   and hurls it down the hall into its dummy.
//!
//! Lanes are counted from 0 as in the code. Things to take lie on each lane's line to its dummy
//! ([`in_reach`]), so a grip aimed down the lane takes them.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use super::art::*;
use super::{AimAt, Cast, Lane, d, lane};
use crate::design::{CastType, Modifier, SpellDesign};

/// When the program starts in its period, s (the last program's smoke settles first).
pub const START: f32 = 1.2;
/// The barrel and the crate on lane 2's line (z from the hall's centre).
pub const BARREL_Z: f64 = 2.5;
pub const CRATE_Z: f64 = 4.6;
/// The loose dummy lane 4 lifts.
pub const LOOSE_Z: f64 = 4.0;
/// Where lane 4's bolt crosses lane 3's line (x, y, z from the hall's centre).
pub const CROSSING: [f64; 3] = [3.0, 1.75, 4.2];
/// Lane 4 fires; lane 3 reaches out this long after, so the bolt crosses its line while it
/// looks (both gather 0.32 s; the bolt flies 0.3 s to the crossing).
pub const CATCH_LAG: f32 = 0.16;

/// A lane's hand (on its plinth, facing down the hall).
pub fn hand(lane: usize) -> DVec3 {
    DVec3::new(LANES[lane], PLINTH_TOP, MAGE_Z) + Vec3::new(0.25, 1.35, -0.45).as_dvec3()
}

/// A lane's dummy's middle.
pub fn dummy_middle(lane: usize) -> DVec3 {
    DVec3::new(LANES[lane], FLOOR + 1.1, DUMMY_Z)
}

/// Whether a thing of `radius` at `at` lies in the cone of a grip from `lane` aimed at its
/// dummy, and how far off the aim it is (degrees), as kinesis picks.
pub fn in_reach(lane: usize, at: DVec3, radius: f32) -> Option<f32> {
    let h = hand(lane);
    let aim = (dummy_middle(lane) - h).as_vec3().normalize();
    let to = (at - h).as_vec3();
    let dist = to.length();
    let a = to.angle_between(aim);
    let limit = crate::kinesis::CONE_DEG.to_radians() + radius / dist.max(0.5);
    (dist <= 12.0 && a <= limit).then_some(a.to_degrees())
}

fn grip(hold: f32) -> SpellDesign {
    d(&["grip"], CastType::Bolt)
        .with(Modifier::Duration { factor: hold })
        .named("Telekinesis")
}

/// The program's lanes.
pub(super) fn lanes() -> Vec<Lane> {
    let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
        at: at + START,
        design: Some(design),
        aim,
    };
    let casts = |casts: Vec<Cast>| Lane {
        casts,
        ..lane(SpellDesign::default())
    };
    let fire = || d(&["fire"], CastType::Bolt).named("Fire bolt");
    let [cx, cy, cz] = CROSSING;
    let ring = d(&["grip"], CastType::Orbit)
        .with(Modifier::Duration { factor: 0.6 })
        .named("Stone ring");
    vec![
        casts(Vec::new()),
        // Bolts at lane 2's ring.
        casts(vec![
            cast(9.6, fire(), AimAt::Mage(2)),
            cast(10.6, fire(), AimAt::Mage(2)),
        ]),
        // The gravity gun, then the ring.
        casts(vec![
            cast(0.9, grip(0.7), AimAt::Focus),
            cast(4.0, grip(0.7), AimAt::Focus),
            cast(8.4, ring, AimAt::Focus),
        ]),
        // Launch (a stone torn from the floor), the catch, a burning stone, then bolts at the
        // ring.
        casts(vec![
            cast(0.2, grip(0.6), AimAt::Focus),
            cast(2.4 + CATCH_LAG, grip(0.6), AimAt::Focus),
            cast(
                5.6,
                d(&["grip", "fire"], CastType::Bolt)
                    .with(Modifier::Duration { factor: 0.6 })
                    .named("Burning stone"),
                AimAt::Focus,
            ),
            cast(10.1, fire(), AimAt::Mage(2)),
            cast(11.1, fire(), AimAt::Mage(2)),
        ]),
        // The bolt to be caught, then the loose dummy lifted and hurled.
        casts(vec![
            cast(2.4, fire(), AimAt::At(cx, cy, cz)),
            cast(6.4, grip(0.8), AimAt::Focus),
        ]),
        casts(Vec::new()),
    ]
}

/// The program's props: lane 2's barrel and crate, lane 4's loose dummy (none of them the
/// lane's focus, so grips aim past them at the lane's dummy).
pub fn spawn_props(commands: &mut Commands, origin: DVec3) {
    spawn_crate(
        commands,
        origin + DVec3::new(LANES[2], FLOOR + 0.45, BARREL_Z),
        2,
        false,
        true,
    );
    spawn_crate(
        commands,
        origin + DVec3::new(LANES[2], FLOOR + 0.38, CRATE_Z),
        2,
        false,
        false,
    );
    spawn_dummy(
        commands,
        origin + DVec3::new(LANES[4], FLOOR, LOOSE_Z),
        4,
        false,
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_each_lane_takes_lies_in_its_grips_cone() {
        let barrel = DVec3::new(LANES[2], FLOOR + 0.45, BARREL_Z);
        let crate_at = DVec3::new(LANES[2], FLOOR + 0.38, CRATE_Z);
        assert!(in_reach(2, barrel, 0.55).is_some(), "the barrel");
        assert!(in_reach(2, crate_at, 0.65).is_some(), "the crate");
        let loose = DVec3::new(LANES[4], FLOOR + 1.1, LOOSE_Z);
        assert!(in_reach(4, loose, 0.89).is_some(), "the loose dummy");
        // Lane 3 has nothing in reach (its dummy is too far): it tears the floor.
        assert!(in_reach(3, DVec3::new(LANES[3], FLOOR + 1.1, DUMMY_Z), 0.9).is_none());
        for (at, r) in [(barrel, 0.55), (crate_at, 0.65), (loose, 0.89)] {
            assert!(in_reach(3, at, r).is_none(), "lane 3 reaches {at}");
        }
        // Lane 4's bolt crosses lane 3's cone.
        let crossing = DVec3::from_array(CROSSING);
        assert!(in_reach(3, crossing, 0.6).is_some());
    }

    #[test]
    fn the_bolt_crosses_lane_3_while_its_grip_looks() {
        // Both gather for the same time; the bolt then flies to the crossing.
        let fly = (DVec3::from_array(CROSSING) - hand(4)).length() as f32 / 26.0;
        let looks_from = CATCH_LAG;
        let looks_to = CATCH_LAG + crate::kinesis::SEARCH_SECS;
        assert!(
            (looks_from + 0.03..looks_to - 0.05).contains(&fly),
            "crossing at {fly} s, looking {looks_from}..{looks_to}"
        );
    }
}
