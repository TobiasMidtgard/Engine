//! The melee rules as plain functions (aimed-melee.md 3.1, 3.4-3.6): one place that says what a
//! blow on a body part does, for the player and every NPC. [`judge`] takes what is known about
//! a blow ([`Strike`]) and the struck body's wounds and returns the [`Verdict`]; the systems in
//! [`resolve`](crate::resolve) feed it and act on it. Every number is here.
//!
//! * **Accuracy** ([`on_target`]): an aimed blow lands on its part with a chance that grows with
//!   weapon skill (45% for a beginner, 100% for a master), less on small parts (hands, knees,
//!   neck); a miss lands on the part beside it ([`beside`]: neck to shoulder, knee to thigh).
//! * **Cover** ([`cover`]): a defender facing the blow covers its vital parts more (head and
//!   neck 1, chest 0.85, belly 0.6, arms 0.45, thighs and knees 0.35, shins 0.2), better with
//!   skill and a shield, worse against a skilled attacker or a heavy blow; a covered blow is
//!   caught on the arm or thigh nearest it at 40%.
//! * **Edge** ([`cut_quality`]): how cleanly the edge meets the body, 0..1, from skill; a blade
//!   under [`GLANCE_BELOW`] glances (55%).
//! * **Where it hurts** ([`zone_damage`]): by part, kind and side (a cut throat 2.2, a blunt
//!   head 1.8, a stab in the chest 1.7, in the kidney from behind 1.9, limbs 0.45 to 0.8,
//!   from behind 1.25 more); armour ([`armor_factor`]) stops cuts most, stabs less, blunt least.
//! * **Wounds** ([`wound_capacity`]): the damage a part takes before it is ruined. A knee at
//!   [`LEG_GIVES_OUT`] can't hold its leg (a hamstring cut from behind goes there at once); a
//!   weapon arm at [`DISARMED_AT`] lets go. Edged blows bleed, blunt ones lock knees and elbows.
//! * **Severing** ([`sever_threshold`]): a clean, hard slash cuts through at the joint when
//!   `quality × (blow / capacity + old wound) × armour × heavy` passes the joint's threshold.

use engine_world::DamageKind;
use engine_world::combat::{Approach, BodyZone, ItemClass, ZoneInjury};

use engine_core::prelude::*;

/// How far dismemberment goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Gore {
    /// Nothing is ever cut off.
    Off,
    /// Arms and legs can be cut off; heads stay on.
    Limbs,
    /// Heads too.
    #[default]
    Full,
}

/// Armour per body zone, 0 (bare) .. 1 (plate), in [`BodyZone::ALL`] order.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Armor {
    pub zones: [f32; 16],
}

impl Armor {
    /// The same armour on every zone.
    pub fn all(level: f32) -> Self {
        Self {
            zones: [level.clamp(0.0, 1.0); 16],
        }
    }

    pub fn on(&self, zone: BodyZone) -> f32 {
        self.zones[zone.index()].clamp(0.0, 1.0)
    }
}

/// Edged blows with a cut quality under this glance off.
pub const GLANCE_BELOW: f32 = 0.35;
/// Damage kept by a glancing edge.
pub const GLANCE_SHARE: f32 = 0.55;
/// Damage kept by a blow the defender covered with an arm or a thigh.
pub const COVER_SHARE: f32 = 0.4;
/// Knee wound at which the leg gives out (the body drops to that knee).
pub const LEG_GIVES_OUT: f32 = 0.6;
/// Wound at which a weapon arm (forearm or hand) lets go of what it holds.
pub const DISARMED_AT: f32 = 0.6;
/// A hamstring cut (a clean edge at the knee from behind) leaves at least this wound.
pub const HAMSTRING_WOUND: f32 = 0.7;
/// Damage that kills outright (a severed neck): more than any body's health.
pub const KILLING_BLOW: f32 = 10_000.0;

/// Damage (in `DamageEvent` amounts; a person has about 100) a part takes before it is ruined.
pub fn wound_capacity(zone: BodyZone) -> f32 {
    use BodyZone::*;
    match zone {
        Head => 45.0,
        Neck => 35.0,
        Chest => 90.0,
        Belly => 80.0,
        UpperArmL | UpperArmR => 45.0,
        ForearmL | ForearmR => 35.0,
        HandL | HandR => 22.0,
        ThighL | ThighR => 60.0,
        KneeL | KneeR => 35.0,
        ShinL | ShinR => 40.0,
    }
}

/// How hard a defender guards a part (aimed-melee 3.4): head and neck most.
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

/// Chance an aimed blow lands on the part it was aimed at, by the attacker's weapon skill
/// 0..1: 45% for a beginner, every time for a master, less on small parts.
pub fn on_target(skill: f32, zone: BodyZone) -> f32 {
    use BodyZone::*;
    let small = match zone {
        HandL | HandR | KneeL | KneeR => 0.15,
        Neck => 0.1,
        ForearmL | ForearmR | ShinL | ShinR => 0.05,
        _ => 0.0,
    };
    (0.45 + 0.55 * skill.clamp(0.0, 1.0) - small * (1.0 - skill.clamp(0.0, 1.0))).clamp(0.05, 1.0)
}

/// Where an aimed blow that missed its part lands instead (`roll` 0..1 picks between two).
pub fn beside(zone: BodyZone, roll: f32) -> BodyZone {
    use BodyZone::*;
    let pick = |a, b| if roll < 0.5 { a } else { b };
    match zone {
        Head => pick(Neck, UpperArmR),
        Neck => pick(UpperArmL, UpperArmR),
        Chest => pick(Belly, UpperArmL),
        Belly => pick(Chest, ThighR),
        UpperArmL => pick(Chest, ForearmL),
        UpperArmR => pick(Chest, ForearmR),
        ForearmL => pick(UpperArmL, HandL),
        ForearmR => pick(UpperArmR, HandR),
        HandL => ForearmL,
        HandR => ForearmR,
        ThighL => pick(KneeL, Belly),
        ThighR => pick(KneeR, Belly),
        KneeL => pick(ThighL, ShinL),
        KneeR => pick(ThighR, ShinR),
        ShinL => KneeL,
        ShinR => KneeR,
    }
}

/// Chance a defender covers a blow on `zone` (aimed-melee 3.4): rookie against rookie covers
/// its head about half the time, a master nearly always against a rookie and 70% against a
/// master; a shield adds 15% up high and 25% off low, a heavy blow takes 10% off.
pub fn cover(zone: BodyZone, defense: f32, attack: f32, shield: bool, heavy: bool) -> f32 {
    let low = matches!(
        zone,
        BodyZone::KneeL | BodyZone::KneeR | BodyZone::ShinL | BodyZone::ShinR
    );
    let shield = match (shield, low) {
        (false, _) => 1.0,
        (true, false) => 1.15,
        (true, true) => 0.75,
    };
    let d = defense.clamp(0.0, 1.0);
    let a = attack.clamp(0.0, 1.0);
    (cover_priority(zone) * (0.35 + 0.65 * d) * shield + 0.1
        - 0.4 * a
        - if heavy { 0.1 } else { 0.0 })
    .clamp(0.0, 0.95)
}

/// Where a covered blow is caught: the arm on the side it comes from for the upper body, the
/// thigh for the belly and legs (`right` = the blow comes at the body's right side).
pub fn caught_on(zone: BodyZone, right: bool) -> BodyZone {
    use BodyZone::*;
    match zone {
        Head | Neck | Chest | UpperArmL | UpperArmR => {
            if right {
                ForearmR
            } else {
                ForearmL
            }
        }
        ForearmL | HandL => ForearmL,
        ForearmR | HandR => ForearmR,
        Belly | ThighL | ThighR | KneeL | KneeR | ShinL | ShinR => {
            if right {
                ThighR
            } else {
                ThighL
            }
        }
    }
}

/// How cleanly the edge meets the body, 0..1: about 0.5 for a beginner with a wide spread, 0.95
/// for a master (`roll` 0..1).
pub fn cut_quality(skill: f32, roll: f32) -> f32 {
    let s = skill.clamp(0.0, 1.0);
    (0.3 + 0.65 * s + (roll - 0.5) * 0.6 * (1.0 - s)).clamp(0.0, 1.0)
}

/// Damage multiplier for a blow of `kind` on `zone` from `approach`.
pub fn zone_damage(zone: BodyZone, kind: DamageKind, approach: Approach) -> f32 {
    use BodyZone::*;
    let edged = matches!(kind, DamageKind::Slash);
    let stab = matches!(kind, DamageKind::Pierce);
    let m = match zone {
        Head if stab => 1.9,
        Head if edged => 1.5,
        Head => 1.8,
        Neck if edged => 2.2,
        Neck if stab => 2.0,
        Neck => 1.4,
        Chest if stab => 1.7,
        Chest => 1.15,
        Belly if stab && approach == Approach::Back => 1.9,
        Belly if stab => 1.4,
        Belly => 1.0,
        UpperArmL | UpperArmR => 0.7,
        ForearmL | ForearmR => 0.6,
        HandL | HandR => 0.45,
        ThighL | ThighR if stab => 1.0,
        ThighL | ThighR => 0.8,
        KneeL | KneeR => 0.75,
        ShinL | ShinR => 0.55,
    };
    match approach {
        Approach::Back => m * 1.25,
        Approach::Side => m * 1.1,
        Approach::Front => m,
    }
}

/// Share of a blow armour lets through: cuts lose 80% to full plate, stabs 60%, blunt 30%.
pub fn armor_factor(armor: f32, kind: DamageKind) -> f32 {
    let stop = match kind {
        DamageKind::Slash => 0.8,
        DamageKind::Pierce => 0.6,
        _ => 0.3,
    };
    1.0 - armor.clamp(0.0, 1.0) * stop
}

/// A joint a limb is cut off at: everything past it (away from the body) goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Joint {
    Neck,
    Shoulder,
    Elbow,
    Wrist,
    Hip,
    Knee,
    Ankle,
}

/// The joint a severing cut on `zone` goes through, and how hard it is to cut (score needed).
/// `None`: a part that is never cut off (head, chest, belly).
pub fn sever_threshold(zone: BodyZone) -> Option<(Joint, f32)> {
    use BodyZone::*;
    match zone {
        Neck => Some((Joint::Neck, 0.95)),
        UpperArmL | UpperArmR => Some((Joint::Shoulder, 1.15)),
        ForearmL | ForearmR => Some((Joint::Elbow, 0.85)),
        HandL | HandR => Some((Joint::Wrist, 0.65)),
        ThighL | ThighR => Some((Joint::Hip, 1.35)),
        KneeL | KneeR => Some((Joint::Knee, 0.95)),
        ShinL | ShinR => Some((Joint::Ankle, 0.9)),
        Head | Chest | Belly => None,
    }
}

/// Whether a weapon class cuts with an edge (can sever, can hamstring).
pub fn edged(class: ItemClass) -> bool {
    matches!(
        class,
        ItemClass::OneHandedBlade
            | ItemClass::Axe
            | ItemClass::Dagger
            | ItemClass::TwoHandedBlade
            | ItemClass::TwoHandedAxe
            | ItemClass::Spear
    )
}

/// Everything known about one blow, for [`judge`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strike {
    /// The part the blade crossed (the animation's contact point on the zone capsules).
    pub struck: BodyZone,
    /// The part it was aimed at, if aimed (`StrikeTargets.zone`).
    pub aimed: Option<BodyZone>,
    pub approach: Approach,
    pub kind: DamageKind,
    /// What struck (the attacker's main hand).
    pub class: ItemClass,
    /// The swing's own damage (the `DamageEvent` amount).
    pub amount: f32,
    /// The attacker's skill with `class`, 0..1.
    pub skill: f32,
    /// The defender's skill with what it holds, 0..1, and whether it can cover at all (facing
    /// the blow, not winding up a strike of its own, on its feet).
    pub defense: f32,
    pub can_cover: bool,
    pub shield: bool,
    /// A heavy (committed) blow.
    pub heavy: bool,
    /// The blow comes at the body's right side (for where a covered blow is caught).
    pub from_right: bool,
    /// Armour on the struck part, 0..1 (taken by [`judge`] from the final zone).
    pub armor: Armor,
    pub gore: Gore,
    /// Three rolls 0..1: accuracy, cover, edge.
    pub rolls: [f32; 3],
}

/// What a blow does.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// The part it finally landed on.
    pub zone: BodyZone,
    /// Aimed at a part and landed elsewhere for want of skill.
    pub off_target: bool,
    /// The defender covered it (caught on an arm or a thigh).
    pub covered: bool,
    /// An edge that met the body badly.
    pub glanced: bool,
    pub quality: f32,
    /// The damage it does (replaces the swing's own).
    pub amount: f32,
    /// Wound added to the part, 0..1.
    pub wound: f32,
    pub bleeding: f32,
    /// A hamstring cut (clean edge at the knee from behind).
    pub hamstring: bool,
    /// Cut off at this joint.
    pub severed: Option<Joint>,
    /// Kills outright (a severed neck).
    pub fatal: bool,
}

/// Judge one blow against the struck body's wounds (`injury` is the part's wound before the
/// blow, read for the final zone through `wound_of`).
pub fn judge(s: &Strike, wound_of: impl Fn(BodyZone) -> ZoneInjury) -> Verdict {
    // 1. Skill: an aimed blow that misses its part lands beside it, if the blade crossed that
    //    part (the animation aimed it there); a blade elsewhere is where it is.
    let mut zone = s.struck;
    let mut off_target = false;
    if let Some(aimed) = s.aimed
        && aimed == s.struck
        && s.rolls[0] > on_target(s.skill, aimed)
    {
        zone = beside(aimed, (s.rolls[0] * 7.0).fract());
        off_target = true;
    }
    // 2. The defender covers vital parts more.
    let mut covered = false;
    if s.can_cover && s.rolls[1] < cover(zone, s.defense, s.skill, s.shield, s.heavy) {
        let caught = caught_on(zone, s.from_right);
        covered = caught != zone || cover_priority(zone) >= 0.85;
        zone = caught;
    }
    // 3. The edge.
    let edge = matches!(s.kind, DamageKind::Slash | DamageKind::Pierce);
    let quality = if edge {
        cut_quality(s.skill, s.rolls[2])
    } else {
        0.0
    };
    let glanced = edge && quality < GLANCE_BELOW;
    let armor = s.armor.on(zone);
    let mut amount = s.amount * zone_damage(zone, s.kind, s.approach) * armor_factor(armor, s.kind);
    if covered {
        amount *= COVER_SHARE;
    }
    if glanced {
        amount *= GLANCE_SHARE;
    }
    // 4. What stays.
    let before = wound_of(zone);
    let cap = wound_capacity(zone);
    let mut wound = (amount / cap).min(1.0 - before.wound).max(0.0);
    let hamstring = matches!(zone, BodyZone::KneeL | BodyZone::KneeR)
        && s.approach == Approach::Back
        && s.kind == DamageKind::Slash
        && edged(s.class)
        && !glanced
        && !covered;
    if hamstring {
        wound = wound.max(HAMSTRING_WOUND - before.wound);
    }
    let bleeding = if edge && !glanced {
        (0.1 + 0.6 * quality * (amount / cap)).min(1.0)
    } else {
        0.0
    };
    // 5. Severing: a clean, hard slash through the joint.
    let mut severed = None;
    if s.kind == DamageKind::Slash
        && edged(s.class)
        && !glanced
        && !before.severed
        && let Some((joint, need)) = sever_threshold(zone)
        && match s.gore {
            Gore::Off => false,
            Gore::Limbs => zone != BodyZone::Neck,
            Gore::Full => true,
        }
    {
        let heavy = if s.heavy { 1.25 } else { 1.0 };
        let score = quality * (amount / cap + before.wound) * (1.0 - 0.7 * armor) * heavy;
        if score >= need {
            severed = Some(joint);
            wound = 1.0 - before.wound;
        }
    }
    let fatal = severed.is_some() && zone == BodyZone::Neck;
    if fatal {
        amount = KILLING_BLOW;
    }
    Verdict {
        zone,
        off_target,
        covered,
        glanced,
        quality,
        amount,
        wound,
        bleeding,
        hamstring,
        severed,
        fatal,
    }
}

/// Add a verdict to a part's wounds. Blunt blows that ruin a knee or an elbow half way lock
/// it.
pub fn wound(injury: &mut ZoneInjury, v: &Verdict, kind: DamageKind) {
    injury.wound = (injury.wound + v.wound).clamp(0.0, 1.0);
    injury.bleeding = (injury.bleeding + v.bleeding).clamp(0.0, 1.0);
    if v.severed.is_some() {
        injury.severed = true;
        injury.bleeding = 1.0;
    }
    let joint = matches!(
        v.zone,
        BodyZone::KneeL | BodyZone::KneeR | BodyZone::ForearmL | BodyZone::ForearmR
    );
    if joint && kind == DamageKind::Blunt && injury.wound >= 0.5 {
        injury.joint_locked = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BodyZone::*;

    fn strike(zone: BodyZone) -> Strike {
        Strike {
            struck: zone,
            aimed: Some(zone),
            approach: Approach::Front,
            kind: DamageKind::Slash,
            class: ItemClass::OneHandedBlade,
            amount: 20.0,
            skill: 0.8,
            defense: 0.3,
            can_cover: false,
            shield: false,
            heavy: false,
            from_right: true,
            armor: Armor::default(),
            gore: Gore::Full,
            rolls: [0.0, 0.99, 0.5],
        }
    }

    fn fresh(_: BodyZone) -> ZoneInjury {
        ZoneInjury::default()
    }

    /// Aimed-melee 3.1, row by row: what a clean hit on each part does.
    #[test]
    fn every_part_hurts_as_the_table_says() {
        // Head: heavy damage; blunt more than a cut.
        let cut = judge(&strike(Head), fresh);
        let club = judge(
            &Strike {
                kind: DamageKind::Blunt,
                class: ItemClass::Mace,
                ..strike(Head)
            },
            fresh,
        );
        assert!(
            cut.amount > 25.0 && club.amount > cut.amount,
            "{cut:?} {club:?}"
        );
        assert!(
            cut.severed.is_none(),
            "heads come off at the neck, not the skull"
        );
        // Neck: the deadliest cut; a clean, hard one takes the head off and kills.
        let neck = judge(&strike(Neck), fresh);
        assert!(neck.amount > 40.0, "{neck:?}");
        let chop = judge(
            &Strike {
                amount: 30.0,
                heavy: true,
                skill: 1.0,
                class: ItemClass::TwoHandedBlade,
                ..strike(Neck)
            },
            fresh,
        );
        assert_eq!(chop.severed, Some(Joint::Neck));
        assert!(chop.fatal && chop.amount >= KILLING_BLOW);
        // Chest: a stab there is critical, more than a cut.
        let stab = judge(
            &Strike {
                kind: DamageKind::Pierce,
                ..strike(Chest)
            },
            fresh,
        );
        assert!(stab.amount > judge(&strike(Chest), fresh).amount * 1.3);
        assert!(stab.severed.is_none());
        // Belly: bleeds; a stab in the back (kidney) worse than one in front.
        let kidney = judge(
            &Strike {
                kind: DamageKind::Pierce,
                approach: Approach::Back,
                ..strike(Belly)
            },
            fresh,
        );
        let gut = judge(
            &Strike {
                kind: DamageKind::Pierce,
                ..strike(Belly)
            },
            fresh,
        );
        assert!(gut.bleeding > 0.1 && kidney.amount > gut.amount * 1.5);
        // Arms: less damage than the body; a ruined weapon arm lets go (DISARMED_AT).
        let arm = judge(&strike(ForearmR), fresh);
        assert!(arm.amount < 20.0);
        let mut forearm = ZoneInjury::default();
        for _ in 0..3 {
            let v = judge(&strike(ForearmR), |_| forearm);
            wound(&mut forearm, &v, DamageKind::Slash);
        }
        assert!(forearm.wound >= DISARMED_AT, "{forearm:?}");
        // Hands: little damage, cut off at the wrist by a second clean cut.
        let mut hand = ZoneInjury::default();
        let mut cut_off = None;
        for _ in 0..3 {
            let v = judge(
                &Strike {
                    skill: 1.0,
                    ..strike(HandL)
                },
                |_| hand,
            );
            wound(&mut hand, &v, DamageKind::Slash);
            cut_off = cut_off.or(v.severed);
        }
        assert_eq!(cut_off, Some(Joint::Wrist));
        assert!(hand.severed && hand.bleeding == 1.0);
        // Thighs: bleed, slower; very hard to cut through.
        let thigh = judge(&strike(ThighL), fresh);
        assert!(thigh.bleeding > 0.1 && thigh.severed.is_none());
        // Knees: from the front a wound; from behind a hamstring and the leg gives out.
        let front = judge(&strike(KneeR), fresh);
        assert!(!front.hamstring && front.wound < LEG_GIVES_OUT);
        let behind = judge(
            &Strike {
                approach: Approach::Back,
                ..strike(KneeR)
            },
            fresh,
        );
        assert!(
            behind.hamstring && behind.wound >= LEG_GIVES_OUT,
            "{behind:?}"
        );
        // Shins: the least damage.
        assert!(judge(&strike(ShinL), fresh).amount < judge(&strike(ThighL), fresh).amount);
        // Blunt blows on a knee lock it.
        let mut knee = ZoneInjury::default();
        for _ in 0..4 {
            let v = judge(
                &Strike {
                    kind: DamageKind::Blunt,
                    class: ItemClass::Mace,
                    ..strike(KneeL)
                },
                |_| knee,
            );
            wound(&mut knee, &v, DamageKind::Blunt);
        }
        assert!(knee.joint_locked && !knee.severed);
    }

    /// Aimed-melee 3.5: how skill changes hitting.
    #[test]
    fn skill_decides_landing_where_aimed_and_how_clean() {
        assert!((on_target(0.0, Chest) - 0.45).abs() < 1e-6);
        assert_eq!(on_target(1.0, KneeL), 1.0);
        assert!(on_target(0.2, HandR) < on_target(0.2, Chest));
        // A beginner who rolls badly hits beside the part; a master with the same roll doesn't.
        let rookie = judge(
            &Strike {
                skill: 0.1,
                rolls: [0.9, 0.99, 0.5],
                ..strike(Neck)
            },
            fresh,
        );
        assert!(rookie.off_target && rookie.zone != Neck);
        let master = judge(
            &Strike {
                skill: 1.0,
                rolls: [0.9, 0.99, 0.5],
                ..strike(Neck)
            },
            fresh,
        );
        assert!(!master.off_target && master.zone == Neck);
        // Edges: a beginner glances off sometimes, a master never.
        assert!(cut_quality(0.0, 0.0) < GLANCE_BELOW);
        assert!(cut_quality(1.0, 0.0) > 0.9);
        let glance = judge(
            &Strike {
                skill: 0.0,
                rolls: [0.0, 0.99, 0.0],
                ..strike(Chest)
            },
            fresh,
        );
        assert!(glance.glanced && glance.severed.is_none());
    }

    /// Aimed-melee 3.4: vital parts covered more, by skill.
    #[test]
    fn defenders_cover_vital_parts_more_and_better_with_skill() {
        let rookie_head = cover(Head, 0.3, 0.3, false, false);
        assert!((0.4..0.6).contains(&rookie_head), "{rookie_head}");
        assert!(cover(Head, 1.0, 0.3, false, false) > 0.9);
        assert!((0.6..0.8).contains(&cover(Head, 1.0, 1.0, false, false)));
        assert!(cover(Head, 0.5, 0.5, false, false) > cover(Belly, 0.5, 0.5, false, false));
        assert!(cover(Belly, 0.5, 0.5, false, false) > cover(KneeL, 0.5, 0.5, false, false));
        assert!(cover(KneeL, 0.5, 0.5, false, false) > cover(ShinL, 0.5, 0.5, false, false));
        // A shield guards high and gets in the way low.
        assert!(cover(Chest, 0.5, 0.5, true, false) > cover(Chest, 0.5, 0.5, false, false));
        assert!(cover(ShinR, 0.5, 0.5, true, false) < cover(ShinR, 0.5, 0.5, false, false));
        // A covered neck cut lands on the forearm at a fraction of the damage.
        let v = judge(
            &Strike {
                can_cover: true,
                defense: 0.9,
                rolls: [0.0, 0.1, 0.5],
                ..strike(Neck)
            },
            fresh,
        );
        assert!(
            v.covered && v.zone == ForearmR && v.severed.is_none() && v.amount < 10.0,
            "{v:?}"
        );
    }

    #[test]
    fn armour_and_gore_keep_limbs_on() {
        let s = Strike {
            amount: 30.0,
            heavy: true,
            skill: 1.0,
            class: ItemClass::TwoHandedBlade,
            ..strike(Neck)
        };
        assert!(judge(&s, fresh).severed.is_some());
        assert!(
            judge(
                &Strike {
                    armor: Armor::all(1.0),
                    ..s
                },
                fresh
            )
            .severed
            .is_none()
        );
        assert!(
            judge(
                &Strike {
                    gore: Gore::Limbs,
                    ..s
                },
                fresh
            )
            .severed
            .is_none()
        );
        assert!(
            judge(
                &Strike {
                    gore: Gore::Off,
                    ..s
                },
                fresh
            )
            .severed
            .is_none()
        );
        let wrist = Strike {
            gore: Gore::Limbs,
            ..strike(HandR)
        };
        let worn = ZoneInjury {
            wound: 0.6,
            ..Default::default()
        };
        assert_eq!(
            judge(
                &Strike {
                    skill: 1.0,
                    ..wrist
                },
                |_| worn
            )
            .severed,
            Some(Joint::Wrist)
        );
        // Maces and stabs never cut anything off.
        let mace = Strike {
            kind: DamageKind::Blunt,
            class: ItemClass::Mace,
            ..s
        };
        assert!(judge(&mace, fresh).severed.is_none());
        let thrust = Strike {
            kind: DamageKind::Pierce,
            ..s
        };
        assert!(judge(&thrust, fresh).severed.is_none());
    }
}
