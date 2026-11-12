//! The combat contract (`engine_world::combat`) on the movement module's fighters.
//!
//! * **Kit and skill.** Every [`Fighter`] carries a katana: a `Loadout` with a one-handed
//!   blade (the left hand joins it on the handle at times) and `CombatSkills` with it by style:
//!   pilots 0.75, the duelist 0.7, the elite 0.85, grunts 0.4, others 0.5. Ones the game gives
//!   (from an inventory) are kept. The combat module judges every blow from a body with a
//!   loadout by where it lands and how skilled both sides are.
//! * **Aim.** A fighter that locks on (the player, [`LockOn`]) names its target in
//!   `StrikeTargets.target`: the locked enemy, else the nearest one in front within reach. The
//!   combat module fills `zone` and `point` from where the camera looks; this module leaves
//!   them alone.
//! * **Legs.** A body's `BodyInjuries` slow it ([`leg_speed`]): a leg that gives out (knee
//!   wound 0.6 or more, or cut off at the thigh, knee or shin) limps at 40% of the run speed,
//!   a locked knee at 70%, both legs gone crawl at 20%; a hurt leg can't sprint, and two gone
//!   can't jump.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::animation::StrikeTargets;
use engine_world::combat::{BodyInjuries, BodyZone, CombatSkills, ItemClass, Loadout};
use engine_world::locomotion::MoveIntent;

use crate::combat::{CombatState, Fighter};
use crate::demo::Puppet;
use crate::lockon::LockOn;
use crate::look::fighter::Style;
use crate::mover::flat;

/// A leg that gave out: the share of the run speed left.
pub const LIMP: f32 = 0.4;
/// A knee that no longer bends.
pub const STIFF: f32 = 0.7;
/// Both legs gone.
pub const CRAWL: f32 = 0.2;
/// A knee wound from which the leg gives out (the combat module's).
pub const LEG_GIVES_OUT: f32 = 0.6;

/// Reach within which a fighter without a lock picks the enemy in front, m.
const NEAR_FOE: f32 = 3.0;

/// How fast a body's legs still carry it, as a share of its run speed, and how many legs are
/// gone (see the module docs).
pub fn leg_speed(inj: &BodyInjuries) -> (f32, u32) {
    let legs = [
        (BodyZone::ThighL, BodyZone::KneeL, BodyZone::ShinL),
        (BodyZone::ThighR, BodyZone::KneeR, BodyZone::ShinR),
    ];
    let (mut gone, mut stiff) = (0, 0);
    for (thigh, knee, shin) in legs {
        let severed = [thigh, knee, shin].iter().any(|z| inj.get(*z).severed);
        if severed || inj.get(knee).wound >= LEG_GIVES_OUT {
            gone += 1;
        } else if inj.get(knee).joint_locked {
            stiff += 1;
        }
    }
    let speed = match (gone, stiff) {
        (0, 0) => 1.0,
        (0, _) => STIFF,
        (1, _) => LIMP,
        _ => CRAWL,
    };
    (speed, gone)
}

/// `intent` for a body with these legs: slower, no sprint, no jump on no legs.
pub fn hobble(intent: &MoveIntent, inj: &BodyInjuries) -> Option<MoveIntent> {
    let (speed, gone) = leg_speed(inj);
    if speed >= 1.0 {
        return None;
    }
    let mut out = *intent;
    out.direction = out.direction.clamp_length_max(speed);
    out.sprint = false;
    if gone >= 2 {
        out.jump = false;
    }
    Some(out)
}

/// One-handed blade skill by fighter style.
pub fn blade_skill(style: Option<Style>) -> f32 {
    match style {
        Some(Style::Pilot) => 0.75,
        Some(Style::Duelist) => 0.7,
        Some(Style::Elite) => 0.85,
        Some(Style::Grunt) => 0.4,
        None => 0.5,
    }
}

type Kit = (
    Entity,
    &'static Fighter,
    Option<&'static Puppet>,
    Has<Loadout>,
    Has<CombatSkills>,
);

type Unarmed = Or<(Without<Loadout>, Without<CombatSkills>)>;

/// Give every fighter its katana and skill (see the module docs).
pub fn arm_fighters(mut commands: Commands, q: Query<Kit, Unarmed>) {
    for (e, _, puppet, has_kit, has_skill) in &q {
        let mut ec = commands.entity(e);
        if !has_kit {
            ec.insert(Loadout {
                main: ItemClass::OneHandedBlade,
                off: ItemClass::None,
                two_handed: false,
            });
        }
        if !has_skill {
            let mut skills = CombatSkills::default();
            skills.weapons.insert(
                ItemClass::OneHandedBlade,
                blade_skill(puppet.map(|p| p.style())),
            );
            ec.insert(skills);
        }
    }
}

type Aimer = (
    Entity,
    &'static Transform,
    &'static Fighter,
    &'static LockOn,
    Option<&'static mut StrikeTargets>,
);

/// Name each locking fighter's target in its `StrikeTargets` (see the module docs).
pub fn name_targets(
    mut commands: Commands,
    mut aimers: Query<Aimer>,
    foes: Query<(Entity, &Transform, &Fighter, &CombatState)>,
) {
    for (e, t, f, lock, strike) in &mut aimers {
        let target = lock.target.or_else(|| {
            let ahead = t.forward();
            foes.iter()
                .filter(|(o, _, of, cs)| *o != e && of.team != f.team && !cs.defeated)
                .filter_map(|(o, ot, ..)| {
                    let to = flat((ot.translation - t.translation).as_vec3(), Vec3::Y);
                    let d = to.length();
                    (d < NEAR_FOE && to.normalize_or_zero().dot(ahead) > 0.3).then_some((d, o))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, o)| o)
        });
        match strike {
            Some(mut s) => {
                if s.target != target {
                    s.target = target;
                }
            }
            None => {
                commands.entity(e).insert(StrikeTargets {
                    target,
                    ..Default::default()
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hurt(zone: BodyZone, wound: f32) -> BodyInjuries {
        let mut i = BodyInjuries::default();
        i.get_mut(zone).wound = wound;
        i
    }

    #[test]
    fn a_knee_that_gives_out_limps_and_cannot_sprint() {
        let intent = MoveIntent {
            direction: Vec3::NEG_Z,
            sprint: true,
            ..Default::default()
        };
        assert!(hobble(&intent, &hurt(BodyZone::KneeL, 0.3)).is_none());
        let limp = hobble(&intent, &hurt(BodyZone::KneeL, 0.6)).expect("limps");
        assert!((limp.direction.length() - LIMP).abs() < 1e-5);
        assert!(!limp.sprint);
    }

    #[test]
    fn a_locked_knee_is_stiff_and_two_lost_legs_crawl_without_jumping() {
        let mut locked = BodyInjuries::default();
        locked.get_mut(BodyZone::KneeR).joint_locked = true;
        assert_eq!(leg_speed(&locked).0, STIFF);
        let mut both = hurt(BodyZone::KneeL, 0.7);
        both.get_mut(BodyZone::ShinR).severed = true;
        assert_eq!(leg_speed(&both), (CRAWL, 2));
        let intent = MoveIntent {
            direction: Vec3::X,
            jump: true,
            ..Default::default()
        };
        assert!(!hobble(&intent, &both).unwrap().jump);
    }
}
