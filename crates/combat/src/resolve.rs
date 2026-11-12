//! Judging blows: every melee `DamageEvent` from an armed body is found on the struck body's
//! parts, judged by [`rules::judge`], its amount replaced by the verdict's, a [`BodyHit`] sent
//! and the wound kept in [`BodyInjuries`]. Then [`disarm`] empties a ruined weapon hand.

use std::collections::{HashMap, VecDeque};

use engine_core::prelude::*;
use engine_world::animation::{Ragdoll, SkeletonPose, StrikeTargets};
use engine_world::combat::{
    Approach, AttackPhase, AttackWindow, BodyHit, BodyInjuries, BodyZone, CombatSkills, ItemClass,
    Loadout, MeleeContact,
};
use engine_world::{DamageEvent, DamageKind, Statuses};

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
    Option<&'a Statuses>,
);

fn salt(p: DVec3, amount: f32) -> u64 {
    (p.x.to_bits() ^ p.y.to_bits().rotate_left(21) ^ p.z.to_bits().rotate_left(42))
        ^ amount.to_bits() as u64
}

/// What judging a blow needs: the rules' settings and dice, the bodies, their wounds, and
/// where the verdicts go.
#[derive(SystemParam)]
pub(crate) struct Judge<'w, 's> {
    time: Res<'w, Time>,
    settings: Res<'w, CombatSettings>,
    dice: ResMut<'w, Dice>,
    log: ResMut<'w, CombatLog>,
    hits: MessageWriter<'w, BodyHit>,
    bodies: Query<'w, 's, BodyItem<'static>>,
    injuries: Query<'w, 's, &'static mut BodyInjuries>,
    fresh: Local<'s, HashMap<Entity, BodyInjuries>>,
    own: ResMut<'w, OwnDamage>,
    commands: Commands<'w, 's>,
    metrics: Res<'w, Metrics>,
}

/// What became of a blow put to the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Judged {
    /// Not a blow the rules judge (not melee, no armed attacker, not a person): as it was.
    Untouched,
    /// The blade met a shield or a weapon: logged, the amount left to the guard.
    Blocked,
    /// Judged on a body part: the amount is the verdict's.
    Landed,
}

impl Judge<'_, '_> {
    /// Judge one melee blow in place. `item` is the striking item's class when the blow says
    /// it (a `MeleeContact`), otherwise the attacker's main hand.
    /// `entry` is where the edge went in (world space) and the bone there, when the
    /// animation module tells (a `MeleeContact`): the struck part is then found from it.
    pub(crate) fn judge(
        &mut self,
        blow: &mut DamageEvent,
        item: Option<ItemClass>,
        entry: Option<(DVec3, Option<&str>)>,
    ) -> Judged {
        if !matches!(
            blow.kind,
            DamageKind::Slash | DamageKind::Pierce | DamageKind::Blunt
        ) || blow.amount <= 0.0
        {
            return Judged::Untouched;
        }
        let Some(by) = blow.source else {
            return Judged::Untouched;
        };
        let Ok((agt, _, aload, askills, aim, awindow, _, _, _)) = self.bodies.get(by) else {
            return Judged::Untouched;
        };
        // Armed bodies (and their fists once disarmed), or any blow that names its weapon.
        let class = match (item, aload) {
            (Some(c), _) if c != ItemClass::None || aload.is_some() => c,
            (None, Some(l)) => l.main,
            _ => return Judged::Untouched,
        };
        let Ok((tgt, pose, tload, tskills, _, twindow, armor, ragdoll, tstatus)) =
            self.bodies.get(blow.target)
        else {
            return Judged::Untouched;
        };
        // Only people: something that fights or is posed as a humanoid.
        let humanoid = pose.is_some_and(|p| p.index_of("upper_arm_r").is_some());
        if !humanoid && tload.is_none() && tskills.is_none() {
            return Judged::Untouched;
        }
        let caps = capsules_of(pose.filter(|_| humanoid));
        let local = to_model(tgt, blow.point);
        let Some(nearest) = caps.zone_at(local) else {
            return Judged::Untouched;
        };
        let gap = caps
            .capsules
            .iter()
            .map(|c| c.distance(local))
            .fold(f32::INFINITY, f32::min);
        let forward = tgt.rotation() * Vec3::NEG_Z;
        let approach = Approach::of(forward, (agt.translation() - tgt.translation()).as_vec3());
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
        // An opening: the defender reeling (a broken shield), stunned or frozen, or a riposte
        // into a guard the attacker just turned aside.
        let opening = tstatus.is_some_and(|s| s.staggered || s.shocked || s.frozen)
            || awindow.is_some_and(|w| w.name.contains("riposte"));
        let aimed = aim
            .filter(|a| a.target == Some(blow.target))
            .and_then(|a| a.zone);
        // Where the edge went in, when told: that part was struck, and the blade is in the
        // body, not on a guard.
        let entered = entry.and_then(|(p, bone)| {
            let at = to_model(tgt, p);
            caps.zone_at(at)
                .or_else(|| bone.and_then(BodyZone::of_bone))
                .map(|z| (z, at))
        });
        let blocked = entered.is_none() && gap > HELD_ITEM_GAP_M * caps.scale;
        // Without it, the blade's point nearest the body's middle is all there is: an aimed
        // blow struck its part (skill then decides whether it stays there), an unaimed one the
        // part nearest its point.
        let (struck, point) = match (entered, aimed) {
            (Some((z, at)), _) => (z, at),
            (None, Some(z)) => (z, caps.center(z).unwrap_or(local)),
            (None, None) => (nearest, local),
        };
        let s = salt(blow.point, blow.amount);
        let rolls = [
            self.dice.roll(s),
            self.dice.roll(s ^ 1),
            self.dice.roll(s ^ 2),
        ];
        let target = blow.target;
        let fresh = &self.fresh;
        let injuries = &self.injuries;
        let injury_now = |zone: BodyZone| {
            fresh
                .get(&target)
                .or(injuries.get(target).ok())
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
            opening,
            from_right: struck.side().map_or(local.x > 0.0, |s| s == 1),
            armor: armor.copied().unwrap_or_default(),
            gore: self.settings.gore,
            rolls,
        };
        let verdict = rules::judge(&strike, injury_now);
        let record = CombatRecord {
            target: Some(target),
            world_point: to_world(tgt, caps.center(verdict.zone).unwrap_or(point)),
            at: self.time.elapsed_secs_f64(),
            aimed,
            struck,
            verdict,
            approach,
            kind: blow.kind,
            class,
            swing: blow.amount,
            blocked,
        };
        if blocked {
            // The blade met a shield or a weapon: that share already went through the guard.
            self.log.push(record);
            return Judged::Blocked;
        }
        if self.settings.rules {
            blow.amount = verdict.amount;
        }
        self.hits.write(BodyHit {
            target,
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
        if self.settings.rules {
            let entry = match self.injuries.get_mut(target) {
                Ok(i) => i.into_inner(),
                Err(_) => self.fresh.entry(target).or_default(),
            };
            rules::wound(entry.get_mut(verdict.zone), &verdict, blow.kind);
        }
        self.log.push(record);
        Judged::Landed
    }

    /// Insert the first `BodyInjuries` of bodies wounded this frame, and count the blows.
    fn finish(&mut self, judged: u32) {
        for (e, i) in self.fresh.drain() {
            self.commands.entity(e).try_insert(i);
        }
        if judged > 0 {
            self.metrics
                .set("combat.blows_judged", self.log.total as f64);
        }
    }
}

/// The `DamageEvent`s [`judge_contacts`] wrote this frame, already judged: the in-place path
/// must not judge them again.
#[derive(Resource, Debug, Default)]
pub(crate) struct OwnDamage {
    ids: Vec<usize>,
}

/// Judge this frame's melee `DamageEvent`s in place (Update, after the animation module has
/// landed them). The path for blows sent straight as damage, while the animation module
/// still does that; [`judge_contacts`] is the one for `MeleeContact`s.
pub(crate) fn judge_blows(mut judge: Judge, mut blows: MessageMutator<DamageEvent>) {
    let mut judged = 0u32;
    let own = std::mem::take(&mut judge.own.ids);
    for (blow, id) in blows.read_with_id() {
        if own.contains(&id.id) {
            continue;
        }
        if judge.judge(blow, None, None) != Judged::Untouched {
            judged += 1;
        }
    }
    judge.finish(judged);
}

/// Turn this frame's melee contacts into damage: each is judged and goes out as exactly one
/// `DamageEvent` (the judged amount; a blade caught on a guard keeps the swing's own, a
/// contact the rules don't judge goes out as it came).
pub(crate) fn judge_contacts(
    mut judge: Judge,
    mut contacts: MessageReader<MeleeContact>,
    mut damage: MessageWriter<DamageEvent>,
) {
    let mut judged = 0u32;
    for c in contacts.read() {
        let mut blow = c.blow.clone();
        let entry = c.entry.map(|p| (p, c.bone.as_deref()));
        if judge.judge(&mut blow, Some(c.item), entry) != Judged::Untouched {
            judged += 1;
        }
        let id = damage.write(blow);
        judge.own.ids.push(id.id);
    }
    judge.finish(judged);
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
