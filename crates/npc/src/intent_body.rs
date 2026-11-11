//! Brain drives, animation animates (characters week N2): fighters whose bodies the animation
//! module poses from intent, on `engine_world` contracts only.
//!
//! * **Who.** With [`NpcSettings::anim_bodies`] (and `character_looks`) on, while a character
//!   module draws appearances (`AppearanceBodies`), a fighter that is a person (it has an
//!   `Appearance` or asks for one) and fights hand to hand, with a bow or with a gun asks the
//!   animation module for its body: a `BodyRequest::human` with its [`Seed`], a `Loadout`
//!   from its weapon ([`loadout_of`]) and `StrikeTargets`. Until the animation module answers
//!   (arms the body: an `AttackWindow` appears) the stand-in's pose stays, so without one
//!   nothing changes.
//! * **Props.** `hide_props` 1 hides the animation module's own sword and shield (their grips
//!   stay): the fighter's team sword or club and its painted shield are drawn in the hands it
//!   poses instead (`visuals`). Archers and riflemen shoot the animation module's bow (it
//!   bends and strings as the body draws) and carbine.
//! * **Bows.** As the brain winds a shot up the archer gets `draw` (it draws and holds at
//!   full draw), and `loose` as the arrow flies (this crate's arrow, on this crate's time), or
//!   `lower` when the shot is called off; it isn't pressed as a quick shot.
//! * **Guns.** A rifleman's `aim` is up while the brain winds a shot up and for a moment
//!   after; `fire` goes as the bullet flies, and after every [`MAGAZINE_SHOTS`] shots `reload`
//!   changes the magazine ([`RELOAD_SECS`]): the brain holds its next shot until it's done,
//!   and `reloading` shows foes the opening.
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
    /// An archer's bow is drawn (`draw` sent, no `loose` or `lower` yet), or a rifleman's
    /// shot is winding up.
    drawing: bool,
    /// A rifleman's shots since its last magazine change, how long it keeps the gun shouldered
    /// after a shot and when its magazine change is done (NPC clock).
    shots: u32,
    aimed_until: f64,
    reloaded_at: f64,
}

/// How many shots a rifleman's magazine holds: it changes it after every that many.
pub const MAGAZINE_SHOTS: u32 = 6;

/// How long the animation module's magazine change takes (world seconds).
pub const RELOAD_SECS: f32 = 2.3;

/// How long a rifleman keeps its gun shouldered after a shot (own seconds).
const AIMED_SECS: f64 = 1.5;

/// Whether a weapon is a bow (arrows fly slowly; bullets do not).
pub(crate) fn is_bow(w: &NpcWeapon) -> bool {
    w.style == WeaponStyle::Ranged && w.projectile_speed < 200.0
}

/// What a fighter holds, for the animation module's move sets.
pub fn loadout_of(w: &NpcWeapon) -> Option<Loadout> {
    match w.style {
        WeaponStyle::Ranged if is_bow(w) => Some(Loadout {
            main: ItemClass::Bow,
            off: ItemClass::None,
            two_handed: true,
        }),
        WeaponStyle::Ranged => Some(Loadout {
            main: ItemClass::Firearm,
            off: ItemClass::None,
            two_handed: true,
        }),
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
    Entity,
    (
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
    ),
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
pub(crate) fn drive_bodies(mut q: Query<Driving>, mut triggers: MessageWriter<AnimationTrigger>) {
    for (
        me,
        (mut body, brain, clock, skills, weapon, mut combat, window, targets, params, asked),
    ) in &mut q
    {
        let now = clock.now;
        let plays = body.asked && asked && window.is_some();
        if combat.anim_strikes != plays {
            combat.anim_strikes = plays;
        }
        if !body.asked || !asked {
            body.drawing = false;
            continue;
        }
        // An archer draws as its brain winds a shot up, holds, and looses as the arrow flies
        // (or lets the string down when the shot is called off).
        if is_bow(weapon) && plays {
            let winding = matches!(combat.phase, AttackPhase::Windup { .. });
            let cue = match (winding, body.drawing) {
                (true, false) => Some("draw"),
                (false, true) if matches!(combat.phase, AttackPhase::Recovery { .. }) => {
                    Some("loose")
                }
                (false, true) => Some("lower"),
                _ => None,
            };
            if let Some(name) = cue {
                body.drawing = winding;
                triggers.write(AnimationTrigger {
                    entity: me,
                    name: name.into(),
                });
            }
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
        // A rifleman shoulders its gun as its brain winds a shot up (and for a moment after),
        // fires as the shot goes, and changes the magazine after every few: a real pause,
        // which its foes see (`reloading`).
        if weapon.style == WeaponStyle::Ranged && !is_bow(weapon) && plays {
            let winding = matches!(combat.phase, AttackPhase::Windup { .. });
            if body.drawing && !winding && matches!(combat.phase, AttackPhase::Recovery { .. }) {
                triggers.write(AnimationTrigger {
                    entity: me,
                    name: "fire".into(),
                });
                body.shots += 1;
                body.aimed_until = now + AIMED_SECS;
                if body.shots >= MAGAZINE_SHOTS {
                    body.shots = 0;
                    triggers.write(AnimationTrigger {
                        entity: me,
                        name: "reload".into(),
                    });
                    body.reloaded_at = now + (RELOAD_SECS * rate) as f64;
                    combat.ready_at = combat.ready_at.max(body.reloaded_at);
                }
            }
            body.drawing = winding;
            let aim = if winding || now < body.aimed_until {
                1.0
            } else {
                0.0
            };
            if params.values.get("aim") != Some(&aim) {
                params.set("aim", aim);
            }
            let reloading = if now < body.reloaded_at { 1.0 } else { 0.0 };
            if params.values.get("reloading") != Some(&reloading) {
                params.set("reloading", reloading);
            }
        }
        // Its own sword and shield are drawn in the hands the animation module poses
        // (`visuals`), so the animation module's props are hidden (their grips stay); an
        // archer shoots the animation module's bow, which bends and strings as it draws.
        let ranged = weapon.style == WeaponStyle::Ranged;
        let hide = if ranged { 0.0 } else { 1.0 };
        if params.values.get("hide_props") != Some(&hide) {
            params.set("hide_props", hide);
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
