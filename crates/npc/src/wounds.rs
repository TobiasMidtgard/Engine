//! What a fighter's wounds leave it able to do, read from the melee contracts the combat
//! module keeps ([`BodyInjuries`], the [`Loadout`] it empties when a weapon arm is ruined):
//!
//! * **Legs.** A knee wounded to [`LEG_GONE_AT`] (or a leg cut off at the hip, knee or ankle)
//!   is a leg it can't stand on: it can't run, sprint or dash, it fights from where it is down
//!   on one knee (`MoveIntent::crouch`) and doesn't chase, and unless it is brave it crawls
//!   back to its mates ([`NpcAction::Retreat`](crate::NpcAction::Retreat)). With both legs
//!   gone it only crawls. A locked knee, or a thigh or shin ruined to the same mark, is a limp:
//!   no sprint, slower.
//! * **Disarmed.** Its kit's main hand holds a weapon but the `Loadout`'s main hand is empty
//!   (a ruined sword arm): it fights with its fists ([`NpcWeapon::fists`]), and backs off to its
//!   mates unless cornered or brave. A lost shield is a fighter without one.
//! * **Dead.** A severed neck (or head) is a killing blow: the body is not revived.
//! * **Guard.** Which part a strike comes at weighs how hard it guards (head and neck most,
//!   the combat module's cover priorities), what that foe keeps striking (learned from
//!   `BodyHit`s, [`OpponentModel::aim_share`](crate::OpponentModel::aim_share)) and its own
//!   wounds (a wounded knee is guarded harder): [`guard_weight`].

use engine_world::combat::{BodyInjuries, BodyZone, ItemClass, Loadout};

use crate::agent::NpcWeapon;
use crate::intent_body::loadout_of;

/// A wound at least this deep on a knee (0..1) takes that leg (the combat module's mark).
pub const LEG_GONE_AT: f32 = 0.6;

/// Top speed with one leg gone (dragging it, on one knee), m/s.
pub const ONE_LEG_SPEED: f32 = 0.5;
/// Top speed crawling with both legs gone, m/s.
pub const CRAWL_SPEED: f32 = 0.3;
/// Speed factor of a limp.
pub const LIMP: f32 = 0.6;

/// What a fighter's wounds leave it able to do (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wounds {
    /// Legs it can't stand on (0..2).
    pub legs_out: u8,
    /// Limping (a locked knee, or a thigh or shin ruined), but still on its feet.
    pub limping: bool,
    /// Its weapon was knocked from its hand (the kit has one, the loadout's hand is empty).
    pub disarmed: bool,
    /// Its shield arm is gone (the kit has a shield, the loadout's off hand is empty).
    pub shield_lost: bool,
    /// The neck or head is cut through.
    pub dead: bool,
}

impl Wounds {
    /// Read from a body's injuries and loadout, against the weapon its kit has.
    pub fn of(
        injuries: Option<&BodyInjuries>,
        loadout: Option<&Loadout>,
        weapon: &NpcWeapon,
    ) -> Self {
        let mut w = Wounds::default();
        if let Some(i) = injuries {
            use BodyZone::*;
            for (thigh, knee, shin) in [(ThighL, KneeL, ShinL), (ThighR, KneeR, ShinR)] {
                let (t, k, s) = (i.get(thigh), i.get(knee), i.get(shin));
                let gone = k.wound >= LEG_GONE_AT || t.severed || k.severed || s.severed;
                if gone {
                    w.legs_out += 1;
                } else if k.joint_locked || t.wound >= LEG_GONE_AT || s.wound >= LEG_GONE_AT {
                    w.limping = true;
                }
            }
            w.dead = i.get(Neck).severed || i.get(Head).severed;
        }
        if let (Some(have), Some(kit)) = (loadout, loadout_of(weapon)) {
            w.disarmed = kit.main != ItemClass::None && have.main == ItemClass::None;
            w.shield_lost =
                weapon.shield && kit.off != ItemClass::None && have.off == ItemClass::None;
        }
        w
    }

    /// It can't stand on a leg.
    pub fn lame(&self) -> bool {
        self.legs_out > 0
    }

    /// What it fights with now: its fists once disarmed, no shield once that arm is gone.
    pub fn weapon(&self, w: &NpcWeapon) -> NpcWeapon {
        if self.disarmed {
            NpcWeapon::fists()
        } else if self.shield_lost {
            NpcWeapon {
                shield: false,
                ..*w
            }
        } else {
            *w
        }
    }

    /// The fastest it can move, m/s, given a top speed `speed` it would have unhurt.
    pub fn top_speed(&self, speed: f32) -> f32 {
        match self.legs_out {
            0 if self.limping => speed * LIMP,
            0 => speed,
            1 => speed.min(ONE_LEG_SPEED),
            _ => speed.min(CRAWL_SPEED),
        }
    }
}

/// How hard a defender guards the part a strike comes at (the combat module's cover
/// priority: head and neck 1, chest 0.85, belly 0.6, arms 0.45, thighs and knees 0.35, shins
/// 0.2), as a factor on its defense: 1 at the chest, more at the head, less down the legs.
pub fn cover_priority(zone: BodyZone) -> f32 {
    use BodyZone::*;
    match zone {
        Head | Neck => 1.0,
        Chest => 0.85,
        Belly => 0.6,
        UpperArmL | UpperArmR | ForearmL | ForearmR | HandL | HandR => 0.45,
        ThighL | ThighR | KneeL | KneeR => 0.35,
        ShinL | ShinR => 0.2,
    }
}

/// The factor on a defender's chance to guard a strike aimed at `zone` (`None`: not known,
/// 1): by the part's cover priority, plus `habit` (0..1: how much of what this foe lands goes
/// there, as far as the defender has learned it) and the defender's own `wound` on it (0..1).
pub fn guard_weight(zone: Option<BodyZone>, habit: f32, wound: f32) -> f32 {
    let Some(zone) = zone else {
        return 1.0;
    };
    0.55 + 0.45 * cover_priority(zone) / cover_priority(BodyZone::Chest)
        + 0.3 * habit.clamp(0.0, 1.0)
        + 0.2 * wound.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_neck_are_guarded_hardest() {
        let w = |z| guard_weight(Some(z), 0.0, 0.0);
        assert!(w(BodyZone::Head) > w(BodyZone::Chest));
        assert!((w(BodyZone::Chest) - 1.0).abs() < 1e-6);
        assert!(w(BodyZone::Chest) > w(BodyZone::KneeL));
        assert!(w(BodyZone::KneeL) > w(BodyZone::ShinR));
        assert_eq!(guard_weight(None, 1.0, 1.0), 1.0);
        // What a foe keeps going for, and its own wound, are guarded harder.
        assert!(guard_weight(Some(BodyZone::KneeL), 0.8, 0.0) > w(BodyZone::KneeL));
        assert!(guard_weight(Some(BodyZone::KneeL), 0.0, 0.7) > w(BodyZone::KneeL));
    }

    #[test]
    fn a_ruined_knee_takes_the_leg_and_a_cut_neck_kills() {
        let sword = NpcWeapon::sword();
        let mut i = BodyInjuries::default();
        i.get_mut(BodyZone::KneeL).wound = 0.65;
        let w = Wounds::of(Some(&i), None, &sword);
        assert_eq!((w.legs_out, w.dead), (1, false));
        i.get_mut(BodyZone::ShinR).severed = true;
        assert_eq!(Wounds::of(Some(&i), None, &sword).legs_out, 2);
        let mut j = BodyInjuries::default();
        j.get_mut(BodyZone::KneeR).joint_locked = true;
        let w = Wounds::of(Some(&j), None, &sword);
        assert!(w.limping && !w.lame());
        j.get_mut(BodyZone::Neck).severed = true;
        assert!(Wounds::of(Some(&j), None, &sword).dead);
    }

    #[test]
    fn an_empty_sword_hand_fights_with_fists() {
        let sword = NpcWeapon::sword_and_shield();
        let kit = loadout_of(&sword).unwrap();
        let full = Wounds::of(None, Some(&kit), &sword);
        assert!(!full.disarmed && !full.shield_lost);
        let empty = Loadout {
            main: ItemClass::None,
            ..kit
        };
        let w = Wounds::of(None, Some(&empty), &sword);
        assert!(w.disarmed);
        assert_eq!(w.weapon(&sword), NpcWeapon::fists());
        let no_shield = Loadout {
            off: ItemClass::None,
            ..kit
        };
        let w = Wounds::of(None, Some(&no_shield), &sword);
        assert!(w.shield_lost && !w.weapon(&sword).shield);
        // Fists were never a weapon to lose.
        let fists = NpcWeapon::fists();
        let w = Wounds::of(None, Some(&Loadout::default()), &fists);
        assert!(!w.disarmed);
    }
}
