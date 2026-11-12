//! Judging blows: every melee `DamageEvent` from an armed body is found on the struck body's
//! parts, judged by [`rules::judge`], its amount replaced by the verdict's, a [`BodyHit`] sent
//! and the wound kept in [`BodyInjuries`]. Then [`disarm`] empties a ruined weapon hand.

use std::collections::{HashMap, VecDeque};

use engine_core::prelude::*;
use engine_world::animation::{Ragdoll, SkeletonPose, StrikeTargets};
use engine_world::combat::{
    Approach, AttackPhase, AttackWindow, BodyHit, BodyInjuries, BodyZone, CombatSkills, ItemClass,
    Loadout,
};
use engine_world::{DamageEvent, DamageKind};

use crate::CombatSettings;
use crate::body::BodyCapsules;
use crate::rules::{self, Armor, Strike};

/// The judged blows, newest last (`combat.log`).
#[derive(Resource, Debug, Default)]
pub struct CombatLog {
    pub records: VecDeque<CombatRecord>,
    /// Blows judged since the start.
    pub total: u64,
}

impl CombatLog {
    pub const CAPACITY: usize = 256;

    fn push(&mut self, r: CombatRecord) {
        if self.records.len() == Self::CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(r);
        self.total += 1;
    }
}

/// One judged blow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombatRecord {
    /// The struck body (not in `combat.log`'s JSON).
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Where it landed, world space (the middle of the part it landed on).
    pub world_point: DVec3,
    /// World time, s.
    pub at: f64,
    pub aimed: Option<BodyZone>,
    /// The part the blade crossed.
    pub struck: BodyZone,
    pub verdict: rules::Verdict,
    pub approach: Approach,
    pub kind: DamageKind,
    pub class: ItemClass,
    /// The swing's own damage, before the rules.
    pub swing: f32,
    /// The blade met something held (a shield, a weapon), not the body.
    pub blocked: bool,
}

/// Rolls for the rules: a counter run through a hash, so the same fight rolls the same way.
#[derive(Resource, Debug, Default)]
pub struct Dice {
    n: u64,
}

impl Dice {
    /// The next roll, 0..1, mixed with `salt` (something about the blow itself, such as its
    /// point; never an entity id).
    pub fn roll(&mut self, salt: u64) -> f32 {
        self.n = self.n.wrapping_add(1);
        let mut z = self
            .n
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(salt);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A blade this far (m, for a 1.75 m body) off every part met something held, not the body.
pub const HELD_ITEM_GAP_M: f32 = 0.12;

/// The humanoid bone a zone is named after (for `BodyHit::bone`).
pub fn bone_of(zone: BodyZone) -> &'static str {
    use BodyZone::*;
    match zone {
        Head => "head",
        Neck => "neck",
        Chest => "chest",
        Belly => "spine",
        UpperArmL => "upper_arm_l",
        UpperArmR => "upper_arm_r",
        ForearmL => "lower_arm_l",
        ForearmR => "lower_arm_r",
        HandL => "hand_l",
        HandR => "hand_r",
        ThighL => "upper_leg_l",
        ThighR => "upper_leg_r",
        KneeL | ShinL => "lower_leg_l",
        KneeR | ShinR => "lower_leg_r",
    }
}

/// A point in a body's model space (origin at its feet, facing −Z).
pub fn to_model(gt: &GlobalTransform, p: DVec3) -> Vec3 {
    let s = gt.scale();
    (gt.rotation().inverse() * (p - gt.translation()).as_vec3()) / s.max(Vec3::splat(1e-6))
}

/// A model-space point back in the world.
pub fn to_world(gt: &GlobalTransform, p: Vec3) -> DVec3 {
    gt.translation() + (gt.rotation() * (p * gt.scale())).as_dvec3()
}

/// The capsules of a body: from its pose, or a standing person.
pub fn capsules_of(pose: Option<&SkeletonPose>) -> BodyCapsules {
    match pose {
        Some(p) if p.index_of("head").is_some() => BodyCapsules::from_pose(p),
        _ => BodyCapsules::standing(1.75),
    }
}

type BodyItem<'a> = (
    &'a GlobalTransform,
    Option<&'a SkeletonPose>,
    Option<&'a Loadout>,
    Option<&'a CombatSkills>,
    Option<&'a StrikeTargets>,
    Option<&'a AttackWindow>,
    Option<&'a Armor>,
    Option<&'a Ragdoll>,
);

fn salt(p: DVec3, amount: f32) -> u64 {
    (p.x.to_bits() ^ p.y.to_bits().rotate_left(21) ^ p.z.to_bits().rotate_left(42))
        ^ amount.to_bits() as u64
}

/// Judge this frame's melee blows (Update, after the animation module has landed them).
#[allow(clippy::too_many_arguments)]
pub(crate) fn judge_blows(
    time: Res<Time>,
    settings: Res<CombatSettings>,
    mut dice: ResMut<Dice>,
    mut log: ResMut<CombatLog>,
    mut blows: MessageMutator<DamageEvent>,
    mut hits: MessageWriter<BodyHit>,
    bodies: Query<BodyItem>,
    mut injuries: Query<&mut BodyInjuries>,
    mut fresh: Local<HashMap<Entity, BodyInjuries>>,
    mut commands: Commands,
    metrics: Res<Metrics>,
) {
    let mut judged = 0u32;
    for blow in blows.read() {
        if !matches!(
            blow.kind,
            DamageKind::Slash | DamageKind::Pierce | DamageKind::Blunt
        ) || blow.amount <= 0.0
        {
            continue;
        }
        let Some(by) = blow.source else { continue };
        let Ok((agt, _, Some(aload), askills, aim, awindow, _, _)) = bodies.get(by) else {
            continue;
        };
        let Ok((tgt, pose, tload, tskills, _, twindow, armor, ragdoll)) = bodies.get(blow.target)
        else {
            continue;
        };
        // Only people: something that fights or is posed as a humanoid.
        let humanoid = pose.is_some_and(|p| p.index_of("upper_arm_r").is_some());
        if !humanoid && tload.is_none() && tskills.is_none() {
            continue;
        }
        let caps = capsules_of(pose.filter(|_| humanoid));
        let local = to_model(tgt, blow.point);
        let Some(nearest) = caps.zone_at(local) else {
            continue;
        };
        let gap = caps
            .capsules
            .iter()
            .map(|c| c.distance(local))
            .fold(f32::INFINITY, f32::min);
        let forward = tgt.rotation() * Vec3::NEG_Z;
        let approach = Approach::of(forward, (agt.translation() - tgt.translation()).as_vec3());
        let class = aload.main;
        let skill = askills.map_or(CombatSkills::DEFAULT_SKILL, |s| s.skill(class));
        let tclass = tload.map_or(ItemClass::None, |l| l.main);
        let defense = tskills.map_or(CombatSkills::DEFAULT_SKILL, |s| s.skill(tclass));
        let busy = twindow.is_some_and(|w| {
            matches!(
                w.phase,
                Some(AttackPhase::Windup) | Some(AttackPhase::Active)
            )
        });
        let down = ragdoll.is_some_and(|r| r.weight > 0.5);
        let aimed = aim
            .filter(|a| a.target == Some(blow.target))
            .and_then(|a| a.zone);
        let blocked = gap > HELD_ITEM_GAP_M * caps.scale;
        // The animation module swings an aimed blow's line at its part and reports the blade's
        // point nearest the body's middle, not where the edge went in: an aimed blow struck
        // its part (skill then decides whether it stays there), an unaimed one the part
        // nearest its point.
        let struck = aimed.unwrap_or(nearest);
        let point = match aimed {
            Some(z) => caps.center(z).unwrap_or(local),
            None => local,
        };
        let s = salt(blow.point, blow.amount);
        let rolls = [dice.roll(s), dice.roll(s ^ 1), dice.roll(s ^ 2)];
        let swing = blow.amount;
        let kind = blow.kind;
        let target = blow.target;
        let record_base = |verdict: rules::Verdict| CombatRecord {
            target: Some(target),
            world_point: to_world(tgt, caps.center(verdict.zone).unwrap_or(point)),
            at: time.elapsed_secs_f64(),
            aimed,
            struck,
            verdict,
            approach,
            kind,
            class,
            swing,
            blocked,
        };
        let injury_now = |zone: BodyZone| {
            fresh
                .get(&blow.target)
                .or(injuries.get(blow.target).ok())
                .map(|i| *i.get(zone))
                .unwrap_or_default()
        };
        let strike = Strike {
            struck,
            aimed,
            approach,
            kind: blow.kind,
            class,
            amount: blow.amount,
            skill,
            defense,
            can_cover: approach != Approach::Back && !busy && !down,
            shield: tload.is_some_and(|l| l.off == ItemClass::Shield),
            heavy: awindow.is_some_and(|w| w.name.contains("heavy")),
            from_right: struck.side().map_or(local.x > 0.0, |s| s == 1),
            armor: armor.copied().unwrap_or_default(),
            gore: settings.gore,
            rolls,
        };
        let verdict = rules::judge(&strike, injury_now);
        judged += 1;
        if blocked {
            // The blade met a shield or a weapon: that share already went through the guard.
            log.push(record_base(verdict));
            continue;
        }
        if settings.rules {
            blow.amount = verdict.amount;
        }
        hits.write(BodyHit {
            target: blow.target,
            attacker: Some(by),
            zone: verdict.zone,
            approach,
            bone: Some(bone_of(verdict.zone).to_string()),
            point: if verdict.zone == struck {
                point
            } else {
                caps.center(verdict.zone).unwrap_or(point)
            },
            kind: blow.kind,
            energy_j: 0.5 * blow.impulse.length_squared() / 1.5,
            cut_quality: verdict.quality,
            blocked_by: None,
        });
        if settings.rules {
            let entry = match injuries.get_mut(blow.target) {
                Ok(i) => i.into_inner(),
                Err(_) => fresh.entry(blow.target).or_default(),
            };
            rules::wound(entry.get_mut(verdict.zone), &verdict, blow.kind);
        }
        log.push(record_base(verdict));
    }
    for (e, i) in fresh.drain() {
        commands.entity(e).try_insert(i);
    }
    if judged > 0 {
        metrics.set("combat.blows_judged", log.total as f64);
    }
}

/// A ruined or severed weapon arm lets go: the main hand (the right) empties, or the off hand
/// (the left); a two-handed weapon falls from either.
pub(crate) fn disarm(
    settings: Res<CombatSettings>,
    mut q: Query<(&BodyInjuries, &mut Loadout), Changed<BodyInjuries>>,
) {
    if !settings.disarm {
        return;
    }
    let ruined = |i: &BodyInjuries, a: BodyZone, b: BodyZone| {
        [a, b].iter().any(|z| {
            let z = i.get(*z);
            z.severed || z.wound >= rules::DISARMED_AT
        })
    };
    for (i, mut l) in &mut q {
        let right = ruined(i, BodyZone::ForearmR, BodyZone::HandR);
        let left = ruined(i, BodyZone::ForearmL, BodyZone::HandL);
        let both = l.two_handed || l.main.needs_both_hands();
        let mut next = *l;
        if (right || (both && left)) && next.main != ItemClass::None {
            next.main = ItemClass::None;
            next.two_handed = false;
        }
        if left && next.off != ItemClass::None {
            next.off = ItemClass::None;
        }
        if next != *l {
            *l = next;
        }
    }
}
