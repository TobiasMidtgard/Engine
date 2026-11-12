//! Melee contracts shared by the combat rules, NPC AI, animation, characters and UI
//! (engine-design/characters-week/aimed-melee.md 3.1):
//!
//! * [`BodyZone`]: where on a body a strike aims or lands, from the humanoid bones, and how
//!   vital it is. [`Approach`]: from the front, the side or behind.
//! * [`MeleeContact`] and [`MeleeJudge`]: melee blows go to the rules module first, and it
//!   writes the `DamageEvent`, so nobody reads an unjudged amount.
//! * [`BodyHit`]: a strike landed on a zone, sent next to the [`DamageEvent`](crate::DamageEvent)
//!   that keeps working for everyone else.
//! * [`CombatSkills`] and [`Loadout`] (with [`ItemClass`]): how good someone is with what they
//!   hold, and what they hold in each hand. UI's off-hand slot and the inventory write the
//!   loadout; animation picks its moves from it.
//! * [`BodyInjuries`]: per zone wounds, bleeding, locked joints and severed limbs.
//! * [`AttackWindow`]: the phase of the strike a body is playing (windup, active, recovery,
//!   cancel), written by the animation module so gameplay reads windows instead of keeping its
//!   own timelines.
//!
//! * [`CombatDifficulty`] (with [`DifficultyTier`]): which tactics enemies may use and how well,
//!   set by the UI's difficulty slider and scripts, read by NPC AI.
//!
//! [`StrikeTargets`](crate::animation::StrikeTargets) carries the aimed zone and point.

use std::collections::BTreeMap;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::events::DamageKind;

/// A part of a body that a strike aims at or lands on. `L`/`R` are the body's own left and
/// right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BodyZone {
    Head,
    Neck,
    Chest,
    Belly,
    UpperArmL,
    UpperArmR,
    ForearmL,
    ForearmR,
    HandL,
    HandR,
    ThighL,
    ThighR,
    KneeL,
    KneeR,
    ShinL,
    ShinR,
}

impl BodyZone {
    /// Every zone, in declaration order (the index [`BodyInjuries`] keeps them by).
    pub const ALL: [BodyZone; 16] = [
        BodyZone::Head,
        BodyZone::Neck,
        BodyZone::Chest,
        BodyZone::Belly,
        BodyZone::UpperArmL,
        BodyZone::UpperArmR,
        BodyZone::ForearmL,
        BodyZone::ForearmR,
        BodyZone::HandL,
        BodyZone::HandR,
        BodyZone::ThighL,
        BodyZone::ThighR,
        BodyZone::KneeL,
        BodyZone::KneeR,
        BodyZone::ShinL,
        BodyZone::ShinR,
    ];

    /// Meters round the knee joint (along the leg) that count as [`BodyZone::KneeL`] /
    /// [`BodyZone::KneeR`] rather than thigh or shin: whoever resolves a hit point picks the
    /// knee by distance to the `lower_leg` bone's origin.
    pub const KNEE_RADIUS_M: f32 = 0.12;

    /// Index into [`BodyZone::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// How vital the zone is, 0..1 (head and neck 1, chest 0.85, belly 0.6, limbs 0.2-0.45):
    /// defenders guard vital zones harder, and damage scales with it.
    pub fn vital(self) -> f32 {
        use BodyZone::*;
        match self {
            Head | Neck => 1.0,
            Chest => 0.85,
            Belly => 0.6,
            UpperArmL | UpperArmR | ForearmL | ForearmR => 0.35,
            HandL | HandR => 0.3,
            ThighL | ThighR => 0.4,
            KneeL | KneeR => 0.45,
            ShinL | ShinR => 0.2,
        }
    }

    /// Which side the zone is on: `Some(0)` left, `Some(1)` right, `None` for the middle.
    pub fn side(self) -> Option<usize> {
        use BodyZone::*;
        match self {
            UpperArmL | ForearmL | HandL | ThighL | KneeL | ShinL => Some(0),
            UpperArmR | ForearmR | HandR | ThighR | KneeR | ShinR => Some(1),
            Head | Neck | Chest | Belly => None,
        }
    }

    /// The zone a humanoid bone ([`crate::animation::HUMANOID_BONES`]) belongs to; `None` for
    /// bones outside every zone (`root`, hair chains). Knees come from the hit point, not a
    /// bone (see [`BodyZone::KNEE_RADIUS_M`]); `lower_leg` and `foot` bones map to the shin.
    pub fn of_bone(name: &str) -> Option<BodyZone> {
        use BodyZone::*;
        let (stem, side) = match name.rsplit_once('_') {
            Some((stem, "l")) => (stem, Some(0)),
            Some((stem, "r")) => (stem, Some(1)),
            _ => (name, None),
        };
        let pick = |l: BodyZone, r: BodyZone| match side {
            Some(0) => Some(l),
            Some(_) => Some(r),
            None => None,
        };
        match stem {
            "head" => Some(Head),
            "neck" => Some(Neck),
            "chest" => Some(Chest),
            "clavicle" => side.map(|_| Chest),
            "hips" | "spine" => Some(Belly),
            "upper_arm" | "upper_arm_twist_01" | "upper_arm_twist_02" => pick(UpperArmL, UpperArmR),
            "lower_arm" | "lower_arm_twist_01" | "lower_arm_twist_02" => pick(ForearmL, ForearmR),
            "upper_leg" | "thigh_twist_01" => pick(ThighL, ThighR),
            "lower_leg" | "calf_twist_01" | "foot" | "toe" => pick(ShinL, ShinR),
            s if s == "hand" || is_finger(s) => pick(HandL, HandR),
            _ => None,
        }
    }
}

/// Whether `stem` (a bone name without its side) is a finger bone: `thumb_01` .. `pinky_03`.
fn is_finger(stem: &str) -> bool {
    stem.split_once('_').is_some_and(|(finger, joint)| {
        matches!(finger, "thumb" | "index" | "middle" | "ring" | "pinky")
            && matches!(joint, "01" | "02" | "03")
    })
}

/// Where a strike comes from, by the angle between the attacker and the target's facing:
/// front under 60°, back over 120°, side between. Knee + back is a hamstring cut, belly + back
/// a kidney stab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Approach {
    #[default]
    Front,
    Side,
    Back,
}

impl Approach {
    /// From the target's facing `forward` and the direction `to_attacker` (target to attacker),
    /// both on the ground plane or not (only the horizontal part counts).
    pub fn of(forward: Vec3, to_attacker: Vec3) -> Approach {
        let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let a = Vec3::new(to_attacker.x, 0.0, to_attacker.z).normalize_or_zero();
        if f == Vec3::ZERO || a == Vec3::ZERO {
            return Approach::Front;
        }
        let angle = f.dot(a).clamp(-1.0, 1.0).acos().to_degrees();
        if angle < 60.0 {
            Approach::Front
        } else if angle > 120.0 {
            Approach::Back
        } else {
            Approach::Side
        }
    }
}

/// A strike landed on a body zone. Sent by whoever resolves melee hits (the combat module),
/// next to the [`DamageEvent`](crate::DamageEvent) for the same hit. Animation plays the hit
/// reaction for the zone, the character module draws the wound, NPC AI learns where it was
/// hit, and [`BodyInjuries`] keeps what stays.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct BodyHit {
    pub target: Entity,
    pub attacker: Option<Entity>,
    pub zone: BodyZone,
    pub approach: Approach,
    /// The humanoid bone struck, when known.
    pub bone: Option<String>,
    /// Where it landed, in the target's model space (m).
    pub point: Vec3,
    pub kind: DamageKind,
    /// Energy the strike carried into the body, J.
    pub energy_j: f32,
    /// 0..1: how clean the cut or stab was (edge alignment, follow-through); a strong clean
    /// edged hit on the neck or a limb can sever.
    pub cut_quality: f32,
    /// What took the strike instead of the body (a shield, a weapon, an armor piece), if it
    /// was blocked or deflected.
    pub blocked_by: Option<Entity>,
}

/// What someone holds in a hand, for skills, animation sets and rules.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub enum ItemClass {
    #[default]
    None,
    OneHandedBlade,
    Axe,
    Mace,
    Dagger,
    Spear,
    TwoHandedBlade,
    /// Two-handed axe or hammer.
    TwoHandedAxe,
    Shield,
    SpellFocus,
    Torch,
    Bow,
    /// A long gun (carbine, rifle), held in both hands.
    Firearm,
}

impl ItemClass {
    /// Whether the class needs both hands (two-handed blade, axe or hammer, bow, long gun).
    pub fn needs_both_hands(self) -> bool {
        matches!(
            self,
            ItemClass::TwoHandedBlade
                | ItemClass::TwoHandedAxe
                | ItemClass::Bow
                | ItemClass::Firearm
        )
    }
}

/// What a body holds: main hand, off hand, and whether one item takes both hands. Written by
/// the inventory or UI (`EquipSlot::OffHand`) for the player and by NPC AI for its fighters;
/// the animation module picks its move sets from it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Loadout {
    pub main: ItemClass,
    pub off: ItemClass,
    pub two_handed: bool,
}

/// How good a body is with each class of item, 0..1, and with its off hand. For players and
/// NPCs alike: the chance to land an aimed strike, guard a zone and parry follows it. Classes
/// not listed read [`CombatSkills::DEFAULT_SKILL`].
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatSkills {
    pub weapons: BTreeMap<ItemClass, f32>,
    pub off_hand: f32,
}

impl CombatSkills {
    /// Skill with a class no entry names.
    pub const DEFAULT_SKILL: f32 = 0.3;

    /// Skill with `class`, clamped to 0..1.
    pub fn skill(&self, class: ItemClass) -> f32 {
        self.weapons
            .get(&class)
            .copied()
            .unwrap_or(Self::DEFAULT_SKILL)
            .clamp(0.0, 1.0)
    }

    /// Off-hand skill, clamped to 0..1.
    pub fn off_hand(&self) -> f32 {
        self.off_hand.clamp(0.0, 1.0)
    }
}

/// What stays of the hits on one zone.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneInjury {
    /// 0 unhurt .. 1 ruined.
    pub wound: f32,
    /// Bleeding, 0..1.
    pub bleeding: f32,
    /// A joint that no longer bends (a locked knee or elbow).
    pub joint_locked: bool,
    /// The limb is cut off at or above this zone.
    pub severed: bool,
}

/// Wounds per [`BodyZone`], in [`BodyZone::ALL`] order. Written by the combat module from
/// [`BodyHit`]s; read by animation (limps, a dropped arm), the character module (wounds,
/// missing limbs) and AI.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyInjuries {
    pub zones: [ZoneInjury; 16],
}

impl BodyInjuries {
    pub fn get(&self, zone: BodyZone) -> &ZoneInjury {
        &self.zones[zone.index()]
    }

    pub fn get_mut(&mut self, zone: BodyZone) -> &mut ZoneInjury {
        &mut self.zones[zone.index()]
    }

    /// Whether any zone is severed.
    pub fn any_severed(&self) -> bool {
        self.zones.iter().any(|z| z.severed)
    }
}

/// The phase of a strike, as the animation plays it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AttackPhase {
    /// Drawing back: the strike can still be cancelled, and defenders read the telegraph.
    #[default]
    Windup,
    /// The weapon can land.
    Active,
    /// Following through; open to a counter.
    Recovery,
    /// Cancelled (a feint, a stagger, a parry).
    Cancel,
}

/// The strike a body is playing now, written by the animation module from its clips' marks;
/// absent (or `phase: None`) between strikes. Gameplay (combat rules, NPC AI) reads windows
/// from it instead of keeping timelines of its own.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AttackWindow {
    pub phase: Option<AttackPhase>,
    /// The move being played (`attack_light`, `stab`, ...).
    pub name: String,
    /// Seconds (world time) into this phase.
    pub phase_secs: f32,
    /// Seconds (world time) this phase lasts in all, as the clip plays it now.
    pub phase_length_secs: f32,
    /// The strike's own step this frame (a lunge, a pivot): world meters, zero between
    /// strikes. The body's mover adds it to the body's position (anim moves only bodies it
    /// drives itself).
    pub warp_delta: Vec3,
}

/// A blow's push on a body, written by the animation module while a hit reaction shoves it;
/// absent (or zero) otherwise. The body's mover adds it to the body's velocity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Knockback {
    /// World m/s.
    pub velocity: Vec3,
}

/// A melee blow as it lands, before any rules judge it. While [`MeleeJudge`] is present the
/// animation module writes its armed and unarmed blows here instead of as `DamageEvent`, and
/// the judging module writes exactly one `DamageEvent` per contact (or none for a miss), so
/// no reader ever sees an unjudged amount. Without a judge, blows go out as `DamageEvent`
/// directly, as before.
#[derive(Message, Debug, Clone)]
pub struct MeleeContact {
    /// The blow as the swing would deal it: target, amount, kind, point, push, attacker.
    pub blow: crate::DamageEvent,
    /// The striking item's class (`ItemClass::None` for fists, kicks and bites).
    pub item: ItemClass,
    /// Which hand struck, when an item did: false main hand, true off hand.
    pub off_hand: bool,
    /// World point where the edge (or point, or fist) first crossed the target's body, when
    /// the animation module knows it. With it, the judge picks the struck zone from geometry
    /// instead of `StrikeTargets.zone`.
    pub entry: Option<DVec3>,
    /// World direction the striking edge moved at contact (unit length): tells a cut from a
    /// stab, and which side it came from.
    pub edge_dir: Option<Vec3>,
    /// The `HUMANOID_BONES` name nearest `entry`, on the target.
    pub bone: Option<String>,
}

/// Present while a module judges every [`MeleeContact`] into `DamageEvent`s (see
/// [`judges_melee`]). Check for it when systems run, not in `Plugin::build`.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct MeleeJudge {
    pub judged_by: String,
}

/// Call from a combat plugin's `build` once it turns every [`MeleeContact`] into damage.
pub fn judges_melee(app: &mut App, name: &str) {
    if app.world.get_resource::<MeleeJudge>().is_none() {
        app.insert_resource(MeleeJudge {
            judged_by: name.to_string(),
        });
    }
}

/// How hard enemies fight: the tier decides which tactics they may use at all
/// (engine-design/npc-difficulty.md 4.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub enum DifficultyTier {
    Easy,
    #[default]
    Medium,
    Hard,
    VeryHard,
    Extreme,
}

/// Which tactics enemies may use (the tier) and how well they use them (skill). Written by the
/// UI's difficulty setting and scripts; read by NPC AI.
#[derive(Resource, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatDifficulty {
    pub tier: DifficultyTier,
    /// 0..1, how advanced enemies are within the tier; `None` leaves each NPC its own skills.
    pub skill: Option<f32>,
    /// Per-faction overrides, keyed by `Faction.0`: tier and skill as above.
    pub factions: BTreeMap<u32, (DifficultyTier, Option<f32>)>,
}

impl CombatDifficulty {
    /// The tier and skill for one faction: its override, else the global setting.
    pub fn for_faction(&self, faction: u32) -> (DifficultyTier, Option<f32>) {
        self.factions
            .get(&faction)
            .copied()
            .unwrap_or((self.tier, self.skill))
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<CombatDifficulty>()
        .register_resource::<CombatDifficulty>(
            "Enemy difficulty: tier (Easy, Medium, Hard, VeryHard, Extreme: which tactics enemies \
             may use), skill (0..1 within the tier, null keeps each NPC's own skills), factions \
             (faction id -> [tier, skill] overrides).",
        );
    app.add_message::<BodyHit>()
        .add_message::<MeleeContact>()
        .register_component::<Loadout>(
            "What a body holds: main and off hand item class (one-handed blade, axe, mace, \
             dagger, spear, two-handed blade, two-handed axe, shield, spell focus, torch, bow, \
             firearm, none), and whether one item takes both hands.",
        )
        .register_component::<CombatSkills>(
            "Skill 0..1 per item class (weapons map) and with the off hand; unlisted classes \
             read 0.3.",
        )
        .register_component::<Knockback>(
            "A hit reaction's push on a body, world m/s, for its mover to add to its velocity \
             (written by the animation module; absent or zero otherwise).",
        )
        .register_component::<BodyInjuries>(
            "Per body zone (head, neck, chest, belly, arms, hands, thighs, knees, shins): wound \
             and bleeding 0..1, joint locked, severed.",
        )
        .register_component::<AttackWindow>(
            "The strike being played: phase (windup, active, recovery, cancel), move name, \
             seconds into the phase and its length.",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_melee_judge_stays() {
        let mut app = App::new();
        judges_melee(&mut app, "engine_combat");
        judges_melee(&mut app, "other");
        assert_eq!(
            app.world.resource::<MeleeJudge>().judged_by,
            "engine_combat"
        );
    }

    #[test]
    fn a_faction_override_wins_and_round_trips() {
        let mut d = CombatDifficulty {
            tier: DifficultyTier::Hard,
            skill: Some(0.4),
            ..Default::default()
        };
        d.factions.insert(7, (DifficultyTier::Easy, None));
        assert_eq!(d.for_faction(7), (DifficultyTier::Easy, None));
        assert_eq!(d.for_faction(1), (DifficultyTier::Hard, Some(0.4)));
        let back: CombatDifficulty =
            serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn bones_fall_in_their_zones() {
        use BodyZone::*;
        for (bone, zone) in [
            ("head", Some(Head)),
            ("neck", Some(Neck)),
            ("chest", Some(Chest)),
            ("clavicle_l", Some(Chest)),
            ("spine", Some(Belly)),
            ("hips", Some(Belly)),
            ("upper_arm_l", Some(UpperArmL)),
            ("upper_arm_twist_02_r", Some(UpperArmR)),
            ("lower_arm_twist_01_l", Some(ForearmL)),
            ("hand_r", Some(HandR)),
            ("thumb_02_l", Some(HandL)),
            ("pinky_03_r", Some(HandR)),
            ("upper_leg_r", Some(ThighR)),
            ("thigh_twist_01_l", Some(ThighL)),
            ("lower_leg_l", Some(ShinL)),
            ("foot_r", Some(ShinR)),
            ("root", None),
            ("ponytail_2", None),
        ] {
            assert_eq!(BodyZone::of_bone(bone), zone, "{bone}");
        }
        for b in crate::animation::HUMANOID_BONES {
            if !matches!(b.0, "root" | "ponytail_1" | "ponytail_2" | "ponytail_3") {
                assert!(BodyZone::of_bone(b.0).is_some(), "{} has no zone", b.0);
            }
        }
        for (i, z) in BodyZone::ALL.iter().enumerate() {
            assert_eq!(z.index(), i);
        }
    }

    #[test]
    fn approach_from_the_angle_to_the_attacker() {
        let ahead = Vec3::NEG_Z;
        assert_eq!(
            Approach::of(ahead, Vec3::new(0.2, 0.0, -1.0)),
            Approach::Front
        );
        assert_eq!(Approach::of(ahead, Vec3::X), Approach::Side);
        assert_eq!(
            Approach::of(ahead, Vec3::new(-0.3, 0.5, 1.0)),
            Approach::Back
        );
    }

    #[test]
    fn contracts_from_data_fill_in_the_rest() {
        let skills: CombatSkills =
            serde_json::from_str(r#"{"weapons": {"Spear": 0.9}, "off_hand": 0.4}"#).unwrap();
        assert_eq!(skills.skill(ItemClass::Spear), 0.9);
        assert_eq!(skills.skill(ItemClass::Axe), CombatSkills::DEFAULT_SKILL);
        let back: CombatSkills =
            serde_json::from_str(&serde_json::to_string(&skills).unwrap()).unwrap();
        assert_eq!(back, skills);
        let loadout: Loadout = serde_json::from_str(r#"{"main": "TwoHandedAxe"}"#).unwrap();
        assert_eq!(loadout.off, ItemClass::None);
        assert!(loadout.main.needs_both_hands());
        let mut hurt = BodyInjuries::default();
        hurt.get_mut(BodyZone::KneeR).joint_locked = true;
        let back: BodyInjuries =
            serde_json::from_str(&serde_json::to_string(&hurt).unwrap()).unwrap();
        assert!(back.get(BodyZone::KneeR).joint_locked && !back.any_severed());
    }
}
