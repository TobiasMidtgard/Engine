//! Where on a foe a strike aims (the look-to-aim combat brief): the brain picks a
//! `BodyZone` for each melee strike from what it sees of the foe, as a fighter would.
//!
//! * **Turned away** (its back half toward the attacker): the belly from behind, the kidneys,
//!   where the animation module stabs.
//! * **Open** (recovering from a swing, staggered): a skilled fighter goes for the neck or the
//!   head, the vitals it can't guard now; a poor one swings at the chest.
//! * **Guarding** (its guard up): the guard covers head and chest, so a fighter who knows to
//!   cuts under it at the near knee or thigh; a rookie hits the guard.
//! * **Winding up its own strike**: a master cuts the arm that swings (the forearm, nearer
//!   than the body).
//! * **A heavy blow** comes down on the head; **on the run** it drives into the body.
//! * **Otherwise** it varies its aim so the foe can't read it: a rookie keeps to the chest and
//!   belly, a skilled fighter mixes in the arms and legs, a master the head too, and none aims
//!   at the same zone twice running when it has others.
//!
//! Left and right are the foe's own: the near side, the one turned toward the attacker.

use engine_core::prelude::*;
use engine_world::combat::BodyZone;

use crate::perception::Snapshot;
use crate::skills::NpcSkills;

/// What the attacker knows when it picks where to strike.
#[derive(Debug, Clone, Copy)]
pub struct AimInput {
    /// Where it stands (feet).
    pub from: DVec3,
    pub heavy: bool,
    /// Striking on the run (a dash in).
    pub moving: bool,
    /// The zone its last strike aimed at.
    pub last: Option<BodyZone>,
    /// 0..1, a different number each strike (its brain's dice).
    pub roll: f32,
}

/// Pick the zone a strike at `foe` aims at (see the module docs).
pub fn pick_zone(foe: &Snapshot, skills: &NpcSkills, a: AimInput) -> BodyZone {
    let level = skills.level().clamp(0.0, 1.0);
    let to_me = Vec3::new(
        (a.from.x - foe.position.x) as f32,
        0.0,
        (a.from.z - foe.position.z) as f32,
    )
    .normalize_or_zero();
    let ahead = foe.forward.with_y(0.0).normalize_or(Vec3::NEG_Z);
    // The foe's own right (it faces `ahead`).
    let right = Vec3::new(-ahead.z, 0.0, ahead.x);
    let near_right = to_me.dot(right) >= 0.0;
    let side = |l: BodyZone, r: BodyZone| if near_right { r } else { l };
    if ahead.dot(to_me) < -0.25 {
        return BodyZone::Belly;
    }
    if foe.vulnerable {
        return if level >= 0.6 {
            if a.roll < 0.55 {
                BodyZone::Neck
            } else {
                BodyZone::Head
            }
        } else {
            BodyZone::Chest
        };
    }
    if a.heavy {
        return BodyZone::Head;
    }
    if foe.blocking && level >= 0.4 {
        return if a.roll < 0.3 + 0.5 * level {
            side(BodyZone::KneeL, BodyZone::KneeR)
        } else {
            side(BodyZone::ThighL, BodyZone::ThighR)
        };
    }
    // Its sword arm is its right.
    if foe.attacking && level >= 0.75 && a.roll < 0.6 {
        return BodyZone::ForearmR;
    }
    if a.moving {
        return if a.roll < 0.5 {
            BodyZone::Chest
        } else {
            BodyZone::Belly
        };
    }
    // Varied by skill: what it knows to aim at, picked by the roll, never the last one.
    let mut known: Vec<BodyZone> = vec![BodyZone::Chest, BodyZone::Belly];
    if level >= 0.35 {
        known.push(side(BodyZone::UpperArmL, BodyZone::UpperArmR));
        known.push(side(BodyZone::ThighL, BodyZone::ThighR));
    }
    if level >= 0.6 {
        known.push(BodyZone::Neck);
        known.push(side(BodyZone::KneeL, BodyZone::KneeR));
    }
    if level >= 0.8 {
        known.push(BodyZone::Head);
        known.push(side(BodyZone::ForearmL, BodyZone::ForearmR));
    }
    known.retain(|&z| Some(z) != a.last);
    known[((a.roll * known.len() as f32) as usize).min(known.len() - 1)]
}

/// How high on a standing body a zone is, as a share of its height (where the stand-in's
/// own strikes land, and where a defender's guard goes).
pub fn zone_height(zone: BodyZone) -> f32 {
    match zone {
        BodyZone::Head => 0.92,
        BodyZone::Neck => 0.84,
        BodyZone::Chest => 0.7,
        BodyZone::UpperArmL | BodyZone::UpperArmR => 0.74,
        BodyZone::Belly => 0.58,
        BodyZone::ForearmL | BodyZone::ForearmR => 0.6,
        BodyZone::HandL | BodyZone::HandR => 0.52,
        BodyZone::ThighL | BodyZone::ThighR => 0.42,
        BodyZone::KneeL | BodyZone::KneeR => 0.28,
        BodyZone::ShinL | BodyZone::ShinR => 0.16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foe() -> Snapshot {
        Snapshot {
            time: 0.0,
            position: DVec3::ZERO,
            velocity: Vec3::ZERO,
            // Facing +X: its right is +Z.
            forward: Vec3::X,
            mode: Default::default(),
            blocking: false,
            attacking: false,
            vulnerable: false,
            stamina: 1.0,
            reloading: false,
            shooter: false,
        }
    }

    fn aim(from: DVec3, roll: f32) -> AimInput {
        AimInput {
            from,
            heavy: false,
            moving: false,
            last: None,
            roll,
        }
    }

    #[test]
    fn a_fighter_reads_the_foe() {
        let ahead = DVec3::new(1.5, 0.0, 0.3);
        let master = NpcSkills::master();
        // Its back turned: the kidneys.
        let behind = DVec3::new(-1.5, 0.0, 0.0);
        assert_eq!(
            pick_zone(&foe(), &master, aim(behind, 0.5)),
            BodyZone::Belly
        );
        // Open: the vitals for a master, the chest for a rookie.
        let open = Snapshot {
            vulnerable: true,
            ..foe()
        };
        for r in [0.1, 0.9] {
            let z = pick_zone(&open, &master, aim(ahead, r));
            assert!(matches!(z, BodyZone::Neck | BodyZone::Head), "{z:?}");
        }
        let rookie = NpcSkills::rookie();
        assert_eq!(pick_zone(&open, &rookie, aim(ahead, 0.1)), BodyZone::Chest);
        // Guarding: under the guard, on the near side (it stands to the foe's right).
        let guard = Snapshot {
            blocking: true,
            ..foe()
        };
        for r in [0.1, 0.9] {
            let z = pick_zone(&guard, &master, aim(ahead, r));
            assert!(matches!(z, BodyZone::KneeR | BodyZone::ThighR), "{z:?}");
        }
        let left = DVec3::new(1.5, 0.0, -0.3);
        let z = pick_zone(&guard, &master, aim(left, 0.1));
        assert_eq!(z, BodyZone::KneeL);
        // A master cuts the arm that swings at it.
        let swinging = Snapshot {
            attacking: true,
            ..foe()
        };
        assert_eq!(
            pick_zone(&swinging, &master, aim(ahead, 0.2)),
            BodyZone::ForearmR
        );
    }

    #[test]
    fn aims_vary_with_skill_and_never_repeat() {
        let ahead = DVec3::new(1.5, 0.0, 0.3);
        let seen = |skills: NpcSkills| {
            let mut zones = std::collections::BTreeSet::new();
            let mut last = None;
            for i in 0..200 {
                let roll = (i as f32 * 0.618_034).fract();
                let z = pick_zone(
                    &foe(),
                    &skills,
                    AimInput {
                        last,
                        ..aim(ahead, roll)
                    },
                );
                assert_ne!(Some(z), last, "the same zone twice running");
                last = Some(z);
                zones.insert(z);
            }
            zones
        };
        let rookie = seen(NpcSkills::rookie());
        assert_eq!(
            rookie,
            [BodyZone::Chest, BodyZone::Belly].into_iter().collect()
        );
        let master = seen(NpcSkills::master());
        assert!(master.len() >= 7, "{master:?}");
        assert!(master.contains(&BodyZone::Head) && master.contains(&BodyZone::KneeR));
    }
}
