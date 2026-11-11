//! Brain drives, animation animates (characters week N2): fighters whose bodies the animation
//! module poses from intent, on `engine_world` contracts only.
//!
//! * **Who.** With [`NpcSettings::anim_bodies`] (and `character_looks`) on, while a character
//!   module draws appearances (`AppearanceBodies`), a fighter that is a person (it has an
//!   `Appearance` or asks for one) and fights hand to hand with a longsword or bare fists asks
//!   the animation module for its body: a `BodyRequest::human` with its [`Seed`], a `Loadout`
//!   from its weapon ([`loadout_of`]) and `StrikeTargets`. Archers and riflemen keep the
//!   stand-in's pose (the animation module has no carbine and doesn't shoot this crate's
//!   arrows). Until the animation module answers (arms the body: an `AttackWindow` appears)
//!   the stand-in's pose stays, so without one nothing changes.
//! * **Props.** `hide_props` 1 hides the animation module's own sword and shield (their grips
//!   stay): the fighter's team sword or club and its painted shield are drawn in the hands it
//!   poses instead (`visuals`).
//! * **Falls.** A fighter the animation module poses keeps its body when it goes down: the
//!   `Ragdoll` weight 1 this crate gives the downed lets it fall limp, and weight 0 on a revive
//!   gets it up. Mates kneel by where that body lies ([`drawn_lying`]), not by where the
//!   stand-in would have fallen. A fighter sent home gives its body back for a step, so it is
//!   built afresh where it stands.
//! * **Revives.** A reviver the animation module poses kneels in its body (`kneel` 1 while the
//!   brain has it down by a mate: both hands on a chest about 0.45 m ahead), so it walks right
//!   up to its spot before getting down; what it held is set down where the stand-in sets it.
//! * **What stays with the stand-in.** A fighter on the stand-in's pose that goes down or
//!   kneels stays on it until it stands again ([`NpcBrain::GET_UP_SECS`] after).
//! * **Intent.** The brain already writes `MoveIntent` (`attack` pressed for a strike, `heavy`
//!   ones as `Dash`; `block` while it guards; `crouch`). Here each step writes
//!   `StrikeTargets.target` (its foe) and `.zone` (where its strike aims, `aim.rs`: the
//!   animation module throws an aimed cut or stab there, from as many lines as the
//!   `CombatSkills` given with the body say it knows), and the strike's tempo in `AnimationParams`: `windup_speed`
//!   so the clip's telegraph lasts as long as the brain's wind-up (learned per light and heavy
//!   strike from the first `AttackWindow` it plays), `strike_speed` by skill (a rookie's strikes
//!   0.82 times as fast, a master's 1.19), and `evasive` only while the brain dodges (the
//!   animation module slips or rolls under the strike it reads; the mover carries the body).
//! * **Strikes.** Once the animation module plays the strikes (an `AttackWindow` appears),
//!   [`NpcCombat::anim_strikes`] is set: the brain's strikes land through the animation module's
//!   `DamageEvent`s instead of this crate's, a feint is an `AnimationTrigger` `feint`, and a strike
//!   the animation module cancels (blocked or deflected) staggers the brain's attacker for the
//!   rest of its window.

use engine_core::prelude::*;
use engine_world::animation::{
    AnimationParams, AnimationTrigger, BodyRequest, SkeletonPose, StrikeTargets,
};
use engine_world::character::{Appearance, AppearanceRequest};
use engine_world::combat::{
    AttackPhase as WindowPhase, AttackWindow, CombatSkills, ItemClass, Loadout,
};

use crate::agent::{
    AttackPhase, Npc, NpcBrain, NpcClock, NpcCombat, NpcSettings, NpcVitals, NpcWeapon, WeaponStyle,
};
use crate::seed::Seed;
use crate::skills::NpcSkills;
use crate::visuals::StandIn;
use crate::wildlife::Animal;

/// A fighter whose body the animation module may pose (see the module docs).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct IntentBody {
    /// This crate gave the fighter its `BodyRequest` (and takes it away again).
    pub asked: bool,
    /// The stand-in keeps the pose until this time (NPC clock): getting up.
    pub hold_until: f64,
    /// How long the light and the heavy strike's telegraph lasts at `windup_speed` and
    /// `strike_speed` 1, own-clock seconds (0 until one has been played).
    pub natural_windup: [f32; 2],
    /// The strike being learned from (heavy) and whether its telegraph has been measured.
    learning: Option<(bool, bool)>,
}

/// What a fighter holds, for the animation module's move sets; `None` for those who fight
/// from range (they keep the stand-in's pose).
pub fn loadout_of(w: &NpcWeapon) -> Option<Loadout> {
    match w.style {
        WeaponStyle::Ranged => None,
        WeaponStyle::Melee if !w.can_block => Some(Loadout::default()),
        WeaponStyle::Melee if w.shield => Some(Loadout {
            main: ItemClass::OneHandedBlade,
            off: ItemClass::Shield,
            two_handed: false,
        }),
        WeaponStyle::Melee => Some(Loadout {
            main: ItemClass::TwoHandedBlade,
            off: ItemClass::None,
            two_handed: true,
        }),
    }
}

pub(crate) type Asking<'a> = (
    Entity,
    &'a Seed,
    &'a NpcWeapon,
    &'a NpcVitals,
    &'a NpcBrain,
    &'a NpcClock,
    Option<&'a mut IntentBody>,
    Has<BodyRequest>,
    Has<StrikeTargets>,
    Has<Loadout>,
    Has<Appearance>,
    Has<AppearanceRequest>,
    (&'a NpcSkills, Has<CombatSkills>),
);

/// Which fighters [`give_bodies`] looks at.
pub(crate) type AskingFilter = (With<Npc>, With<StandIn>, Without<Animal>);

/// Give and take away fighters' `BodyRequest`s (see the module docs). Run by the system that
/// asks for their looks, so its commands add no sync point of their own.
pub(crate) fn give_bodies(
    commands: &mut Commands,
    settings: &NpcSettings,
    drawn: bool,
    q: &mut Query<Asking, AskingFilter>,
) {
    for (
        e,
        seed,
        weapon,
        vitals,
        brain,
        clock,
        body,
        asked,
        targets,
        loadout,
        looks,
        asks,
        (skills, skilled),
    ) in q.iter_mut()
    {
        let now = clock.now;
        let mut fresh = IntentBody::default();
        let (body, new) = match body {
            Some(b) => (b.into_inner(), false),
            None => (&mut fresh, true),
        };
        if (vitals.downed || brain.kneeling()) && !(body.asked && asked) {
            body.hold_until = body.hold_until.max(now + NpcBrain::GET_UP_SECS);
        }
        let kit = loadout_of(weapon);
        let want = settings.anim_bodies
            && settings.character_looks
            && drawn
            && (looks || asks)
            && kit.is_some()
            && now >= body.hold_until
            // Falls, kneeling by a mate and getting up from either are the animation
            // module's, once it has the body.
            && ((body.asked && asked)
                || (!vitals.downed && !brain.kneeling() && now >= brain.revive_rise_until));
        let mut cmd = commands.entity(e);
        if want && !asked {
            body.asked = true;
            cmd.try_insert(BodyRequest::human(seed.0));
            if !loadout && let Some(kit) = kit {
                cmd.try_insert(kit);
            }
            if !targets {
                cmd.try_insert(StrikeTargets::default());
            }
            if !skilled && let Some(kit) = kit {
                cmd.try_insert(combat_skills(&kit, skills));
            }
        } else if !want && asked && body.asked {
            body.asked = false;
            cmd.remove::<BodyRequest>();
        }
        if new {
            cmd.try_insert(*body);
        }
    }
}

type Driving<'a> = (
    &'a mut IntentBody,
    &'a NpcBrain,
    &'a NpcClock,
    &'a NpcSkills,
    &'a NpcWeapon,
    &'a mut NpcCombat,
    Option<&'a AttackWindow>,
    Option<&'a mut StrikeTargets>,
    Option<&'a mut AnimationParams>,
    Has<BodyRequest>,
);

/// How good a fighter is with what it holds, for the animation module's aimed strikes (how
/// many lines of cut it knows): its skill level with its main weapon, its defense with the off
/// hand.
pub fn combat_skills(kit: &Loadout, skills: &NpcSkills) -> CombatSkills {
    let level = skills.level().clamp(0.0, 1.0);
    let mut weapons = std::collections::BTreeMap::new();
    if kit.main != ItemClass::None {
        weapons.insert(kit.main, level);
    }
    if kit.off != ItemClass::None {
        weapons.insert(kit.off, skills.defense.clamp(0.0, 1.0));
    }
    CombatSkills {
        weapons,
        off_hand: (0.5 * (level + skills.defense)).clamp(0.0, 1.0),
    }
}

/// The windup speed the animation module plays at, clamped as it clamps it.
const SPEED_RANGE: (f32, f32) = (0.25, 3.0);

/// How fast a fighter's strikes play by its skill (`strike_speed`).
pub(crate) fn strike_speed(skills: &NpcSkills) -> f32 {
    0.82 + 0.37 * skills.level().clamp(0.0, 1.0)
}

/// Write each intent body's foe and strike tempo, learn its strikes' telegraphs, and say
/// whether the animation module plays its strikes (see the module docs).
pub(crate) fn drive_bodies(mut q: Query<Driving>) {
    for (mut body, brain, clock, skills, weapon, mut combat, window, targets, params, asked) in
        &mut q
    {
        let now = clock.now;
        let plays = body.asked && asked && window.is_some();
        if combat.anim_strikes != plays {
            combat.anim_strikes = plays;
        }
        if !body.asked || !asked {
            continue;
        }
        if let Some(mut t) = targets
            && (t.target != brain.target || t.zone != combat.zone)
        {
            t.target = brain.target;
            t.zone = combat.zone;
        }
        let Some(mut params) = params else {
            continue;
        };
        let speed = strike_speed(skills);
        let windup = |heavy: bool| weapon.windup * if heavy { 1.5 } else { 1.0 };
        let rate = clock.rate.max(1e-3);
        // Learn the telegraph from the first wind-up of each kind as it plays.
        let heavy = match combat.phase {
            AttackPhase::Windup { heavy, .. } => Some(heavy),
            _ => None,
        };
        match (heavy, window.and_then(|w| w.phase.map(|p| (p, w)))) {
            (Some(h), Some((WindowPhase::Windup, w))) if w.phase_length_secs > 0.0 => {
                if body.learning != Some((h, true)) {
                    let at = params.values.get("windup_speed").copied().unwrap_or(1.0);
                    let at = at.clamp(SPEED_RANGE.0, SPEED_RANGE.1);
                    let natural = w.phase_length_secs * rate * at * speed;
                    if natural.is_finite() && natural > 0.0 {
                        body.natural_windup[usize::from(h)] = natural;
                    }
                    body.learning = Some((h, true));
                }
            }
            (Some(h), _) => {
                if body.learning.is_none_or(|(lh, _)| lh != h) {
                    body.learning = Some((h, false));
                }
            }
            (None, _) => body.learning = None,
        }
        // The tempo for the next strike (or this one, which takes it from its next frame).
        let h = heavy.unwrap_or(false);
        let natural = body.natural_windup[usize::from(h)];
        let windup_speed = if natural > 0.0 {
            (natural / (windup(h) * speed).max(1e-3)).clamp(SPEED_RANGE.0, SPEED_RANGE.1)
        } else {
            1.0
        };
        if params.values.get("windup_speed") != Some(&windup_speed) {
            params.set("windup_speed", windup_speed);
        }
        if params.values.get("strike_speed") != Some(&speed) {
            params.set("strike_speed", speed);
        }
        // The brain decides to dodge (and its mover carries the body): the animation module
        // slips or rolls under the strike only then.
        let evasive = if combat.is_dodging(now) { 1.0 } else { 0.0 };
        if params.values.get("evasive") != Some(&evasive) {
            params.set("evasive", evasive);
        }
        // Down on its knees by a mate, its hands on the chest.
        let kneel = if brain.kneeling() { 1.0 } else { 0.0 };
        if params.values.get("kneel") != Some(&kneel) {
            params.set("kneel", kneel);
        }
        // Its own sword and shield are drawn in the hands the animation module poses
        // (`visuals`), so the animation module's props are hidden (their grips stay).
        if params.values.get("hide_props") != Some(&1.0) {
            params.set("hide_props", 1.0);
        }
        // A strike the animation module cancelled (blocked, deflected) rebounds: the brain's
        // attacker reels for the rest of it.
        if let Some(w) = window
            && w.phase == Some(WindowPhase::Cancel)
            && matches!(combat.phase, AttackPhase::Windup { .. })
        {
            let left = (w.phase_length_secs - w.phase_secs).max(0.1);
            combat.phase = AttackPhase::Staggered {
                until: now + (left * rate) as f64,
            };
        }
    }
}

/// Tell the animation module a strike is pulled back (a feint), for a fighter whose strikes it
/// plays.
pub(crate) fn feint(
    me: Entity,
    combat: &NpcCombat,
    triggers: &mut MessageWriter<AnimationTrigger>,
) {
    if combat.anim_strikes {
        triggers.write(AnimationTrigger {
            entity: me,
            name: "feint".into(),
        });
    }
}

/// Where a body the animation module poses lies, in the world: about its breastbone (halfway
/// from the chest joint to the neck), its hips and its head. `None` without those bones.
pub(crate) fn drawn_lying(t: &Transform, pose: &SkeletonPose) -> Option<[DVec3; 3]> {
    let g = GlobalTransform::from(*t);
    let at = |name: &str| pose.bone_world(&g, pose.index_of(name)?).map(|(p, _)| p);
    let (chest, neck) = (at("chest")?, at("neck")?);
    Some([chest.lerp(neck, 0.5), at("hips")?, at("head")?])
}

/// Where a mate's hands and knees go to revive a body lying as [`drawn_lying`] says, on the
/// ground at height `ground`: the breastbone, and beside it from the body's left and right
/// (half a metre out, a little toward the head).
pub(crate) fn kneel_by(ground: f64, [chest, hips, head]: [DVec3; 3]) -> (DVec3, [DVec3; 2]) {
    let along = DVec3::new(head.x - hips.x, 0.0, head.z - hips.z).normalize_or(DVec3::Z);
    let side = DVec3::new(along.z, 0.0, -along.x);
    let flat = DVec3::new(chest.x, ground, chest.z);
    let knee = |s: f64| flat + side * (0.5 * s) + along * 0.08;
    (chest, [knee(-1.0), knee(1.0)])
}
