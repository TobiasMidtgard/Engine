//! Fighters on intent: a body this module built for a [`BodyRequest`] that carries a
//! [`Loadout`] fights with what it holds, driven only by what gameplay or AI writes.
//!
//! * [`Loadout`] picks the move set and the held props: a one-handed weapon (with or without
//!   a shield) the sword-and-shield set, a two-handed blade or axe the great sword's, a bow
//!   the bow's, empty hands boxing. The props are spawned here and re-made when it changes.
//! * [`MoveIntent`]`.attack` presses a strike as it turns on (`Melee`, `Spell`, `Ranged`: a
//!   light one; `Dash`: a heavy one), and `.block` holds the guard up. An
//!   [`AnimationTrigger`](engine_world::animation::AnimationTrigger) `attack_light` or
//!   `attack_heavy` works too.
//! * [`StrikeTargets`]`.target` is who the strikes close on and land on, and where the head
//!   looks.
//! * [`AnimationParams`] `strike_speed` and `windup_speed` (1 when unset) scale the strike's
//!   tempo, for skill (a rookie's slow telegraph, a master's fast one); an
//!   [`AnimationTrigger`](engine_world::animation::AnimationTrigger) `feint` pulls a strike
//!   back while it winds up. `MoveIntent::crouch` sets `crouch`.
//! * Strikes that land send `DamageEvent`s; [`Statuses`] `shielding` follows the guard and
//!   `staggered` an opening after a deflect, where the body has `Statuses`.
//! * [`AttackWindow`] says each frame which phase the strike being played is in (wind-up,
//!   active, recovery; cancel while a blocked or deflected blow rebounds), for the rules and
//!   the AI to read instead of keeping timelines of their own.
//!
//! The requester keeps moving the entity (the contract says the animation module never does),
//! so the [`Attacker`] it gets doesn't warp its transform: [`Attacker::warp_delta`] says
//! where a strike's step would go, and [`HitReaction::stagger_velocity`](crate::hit::HitReaction)
//! how fast a blow pushes it.

use engine_core::prelude::*;
use engine_world::animation::{AnimationParams, BodyRequest, StrikeTargets};
use engine_world::combat::{AttackPhase as WindowPhase, AttackWindow, ItemClass, Loadout};
use engine_world::events::AttackKind;
use engine_world::locomotion::MoveIntent;
use engine_world::magic::Statuses;

use crate::animator::{AnimationLibrary, Animator};
use crate::combat::Tempo;
use crate::combat::{AttackPhase, AttackSets, Attacker, WindowKind};
use crate::grip::HeldItems;
use crate::ik::{LookAt, ReachIk};

/// What a fighter was armed with, and what of it this module put on the body.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Armed {
    pub loadout: Loadout,
    /// The prop entities (blade, guard, shield board, bow limbs, ...).
    pub props: Vec<Entity>,
    /// The intent's strike last frame, to press once as it turns on.
    attack_was: Option<AttackKind>,
}

/// How a [`Loadout`] fights: the move set, the stance it stands in, the props it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arms {
    pub set: &'static str,
    /// The [`AnimationParams`] stance layer turned on (`None`: the plain guard of boxing).
    pub stance: Option<&'static str>,
    pub sword: bool,
    pub shield: bool,
    pub greatsword: bool,
    pub bow: bool,
}

/// Every stance and carry layer a fighter might have on, so a new loadout turns the old off.
const LAYERS: [&str; 5] = [
    "sword_stance",
    "greatsword_stance",
    "bow_carry",
    "sword_carry",
    "greatsword_carry",
];

impl Arms {
    pub fn of(loadout: &Loadout) -> Self {
        let none = Self {
            set: "boxing",
            stance: None,
            sword: false,
            shield: false,
            greatsword: false,
            bow: false,
        };
        let one_handed = |c: ItemClass| {
            matches!(
                c,
                ItemClass::OneHandedBlade
                    | ItemClass::Axe
                    | ItemClass::Mace
                    | ItemClass::Dagger
                    | ItemClass::Spear
            )
        };
        match (loadout.main, loadout.off) {
            (ItemClass::Bow, _) | (_, ItemClass::Bow) => Self {
                set: "bow",
                stance: Some("bow_carry"),
                bow: true,
                ..none
            },
            (ItemClass::TwoHandedBlade | ItemClass::TwoHandedAxe, _) => Self {
                set: "greatsword",
                stance: Some("greatsword_stance"),
                greatsword: true,
                ..none
            },
            (main, off) if one_handed(main) => Self {
                set: "sword_shield",
                stance: Some("sword_stance"),
                sword: true,
                shield: off == ItemClass::Shield,
                ..none
            },
            (_, ItemClass::Shield) => Self {
                set: "sword_shield",
                stance: Some("sword_stance"),
                shield: true,
                ..none
            },
            _ => none,
        }
    }
}

/// Arm requested bodies with what their [`Loadout`] holds (once the body is built, and again
/// when the loadout changes): the move set, the stance, the props.
#[allow(clippy::type_complexity)]
pub(crate) fn arm_fighters(
    mut commands: Commands,
    q: Query<
        (Entity, &Loadout, Option<&Armed>),
        (
            With<BodyRequest>,
            With<Animator>,
            Or<(Changed<Loadout>, Added<Animator>)>,
        ),
    >,
) {
    for (entity, loadout, armed) in &q {
        if armed.is_some_and(|a| a.loadout == *loadout) {
            continue;
        }
        let loadout = *loadout;
        let old = armed.map(|a| a.props.clone()).unwrap_or_default();
        commands.queue(move |world: &mut World| arm(world, entity, loadout, &old));
    }
}

fn arm(world: &mut World, entity: Entity, loadout: Loadout, old: &[Entity]) {
    for &p in old {
        if let Ok(e) = world.get_entity_mut(p) {
            e.despawn();
        }
    }
    let Ok(mut e) = world.get_entity_mut(entity) else {
        return;
    };
    // What the last loadout held goes with its props.
    e.remove::<(HeldItems, crate::combat::Shield, ReachIk)>();
    let arms = Arms::of(&loadout);
    let mut props = Vec::new();
    if arms.sword {
        props.extend(crate::props::spawn_sword(world, entity));
    }
    if arms.shield {
        props.extend(crate::props::spawn_shield(world, entity));
    }
    if arms.greatsword {
        props.extend(crate::props::spawn_greatsword(world, entity));
    }
    if arms.bow {
        props.extend(crate::props::spawn_bow(world, entity));
    }
    let mut e = world.entity_mut(entity);
    let target = e.get::<Attacker>().and_then(|a| a.target);
    // Blows knock it back through the requester too: the push is published
    // (`HitReaction::stagger_velocity`), not walked.
    if !e.contains::<crate::hit::HitReaction>() {
        e.insert(crate::hit::HitReaction {
            stagger_move: false,
            ..Default::default()
        });
    }
    // Head and eyes on the foe (the brain's `StrikeTargets.target`).
    if !e.contains::<LookAt>() {
        e.insert(LookAt {
            enabled: false,
            chain: vec![("neck".into(), 0.4), ("head".into(), 0.6)],
            max_angle_deg: 70.0,
            weight: 0.85,
            ..Default::default()
        });
    }
    let mut attacker = Attacker::new(arms.set, target);
    // The requester moves the entity; a strike's step is published, not taken.
    attacker.warp = false;
    e.insert((
        attacker,
        Armed {
            loadout,
            props,
            attack_was: None,
        },
    ));
    let mut params = e.get::<AnimationParams>().cloned().unwrap_or_default();
    for layer in LAYERS {
        params.set(layer, if Some(layer) == arms.stance { 1.0 } else { 0.0 });
    }
    e.insert(params);
}

/// The guard parameter a set raises: the shield's, or a plain one.
fn guard_param(arms: &Arms) -> &'static str {
    if arms.shield {
        "shield_guard"
    } else {
        "guarding"
    }
}

/// How a fighter's strikes play unless gameplay says otherwise: a slower wind-up that reads,
/// a sharp active phase, a held follow-through (Sifu and God of War time their strikes this
/// way rather than as captured).
pub const FIGHTER_TEMPO: Tempo = Tempo {
    speed: 1.0,
    windup: 0.9,
    active: 1.2,
    recovery: 0.85,
};

/// Where on a foe the head looks: about eye height above its feet, m.
const LOOK_HEIGHT: f64 = 1.55;

/// A parameter that defaults to 1 when unset (`strike_speed`, `windup_speed`).
fn scale(params: &AnimationParams, name: &str) -> f32 {
    params
        .values
        .get(name)
        .copied()
        .unwrap_or(1.0)
        .clamp(0.25, 3.0)
}

type Fighter<'a> = (
    &'a mut Armed,
    &'a mut Attacker,
    &'a mut Animator,
    &'a mut AnimationParams,
    Option<&'a MoveIntent>,
    Option<&'a StrikeTargets>,
    Option<&'a mut LookAt>,
    Option<&'a mut Statuses>,
);

/// Turn intent into presses, guard, target, tempo and gaze.
pub(crate) fn drive_fighters(mut q: Query<Fighter>, places: Query<&GlobalTransform>) {
    for (mut armed, mut attacker, mut animator, mut params, intent, targets, look, statuses) in
        &mut q
    {
        if let Some(t) = targets
            && t.target.is_some()
        {
            attacker.target = t.target;
        }
        // Skill sets the tempo: `strike_speed` all of it, `windup_speed` the telegraph.
        let tempo = Tempo {
            speed: scale(&params, "strike_speed"),
            windup: FIGHTER_TEMPO.windup * scale(&params, "windup_speed"),
            ..FIGHTER_TEMPO
        };
        if attacker.tempo != tempo {
            attacker.tempo = tempo;
        }
        if let Some(mut look) = look {
            let foe = attacker.target.and_then(|t| places.get(t).ok());
            look.enabled = foe.is_some();
            if let Some(foe) = foe {
                look.target = foe.translation() + DVec3::Y * LOOK_HEIGHT;
            }
        }
        let arms = Arms::of(&armed.loadout);
        let guard = guard_param(&arms);
        let up = params.get(guard) > 0.5;
        if let Some(mut st) = statuses {
            let open = attacker.open > 0.0;
            if st.shielding != up || st.staggered != open {
                st.shielding = up;
                st.staggered = open;
            }
        }
        let Some(intent) = intent else {
            continue;
        };
        if intent.attack.is_some() && armed.attack_was.is_none() {
            animator.trigger(match intent.attack {
                Some(AttackKind::Dash) => "attack_heavy",
                _ => "attack_light",
            });
        }
        armed.attack_was = intent.attack;
        let up = if intent.block { 1.0 } else { 0.0 };
        if params.get(guard) != up {
            params.set(guard, up);
        }
        let crouch = if intent.crouch { 1.0 } else { 0.0 };
        if params.get("crouch") != crouch {
            params.set("crouch", crouch);
        }
    }
}

/// Publish the phase of the strike each fighter plays.
pub(crate) fn publish_windows(
    mut commands: Commands,
    sets: Res<AttackSets>,
    library: Res<AnimationLibrary>,
    mut q: Query<(Entity, &Attacker, Option<&mut AttackWindow>), With<Armed>>,
) {
    for (entity, attacker, window) in &mut q {
        let now = window_of(attacker, &sets, &library);
        match window {
            Some(mut w) => {
                if *w != now {
                    *w = now;
                }
            }
            None => {
                commands.entity(entity).insert(now);
            }
        }
    }
}

/// The [`AttackWindow`] for where `attacker` is in its move.
pub fn window_of(
    attacker: &Attacker,
    sets: &AttackSets,
    library: &AnimationLibrary,
) -> AttackWindow {
    let Some(m) = attacker
        .current
        .as_ref()
        .and_then(|c| sets.0.get(&attacker.set).and_then(|s| s.get(c)))
    else {
        return AttackWindow::default();
    };
    let hits: Vec<(f32, f32)> = m
        .windows
        .iter()
        .filter(|w| matches!(w.kind, WindowKind::Hit { .. }))
        .map(|w| (w.start, w.end))
        .collect();
    let end = library
        .clip(&m.clip)
        .map(|c| c.duration)
        .unwrap_or(attacker.time);
    let first = hits.iter().map(|h| h.0).fold(f32::INFINITY, f32::min);
    let last = hits.iter().map(|h| h.1).fold(0.0, f32::max);
    let t = attacker.time;
    let (phase, from, to) = if attacker.recoil > 0.0 {
        (WindowPhase::Cancel, t, t + attacker.recoil)
    } else {
        match attacker.phase {
            AttackPhase::Startup | AttackPhase::Idle if hits.is_empty() => {
                (WindowPhase::Recovery, 0.0, end)
            }
            AttackPhase::Startup | AttackPhase::Idle => (WindowPhase::Windup, 0.0, first),
            AttackPhase::Active => (WindowPhase::Active, first, last),
            AttackPhase::Recovery => (WindowPhase::Recovery, last, end.max(last)),
        }
    };
    // Clip seconds to world seconds, at the rate this phase plays (a cancel's recoil is
    // world time already).
    let rate = match phase {
        WindowPhase::Cancel => 1.0,
        _ => attacker.tempo.rate(attacker.phase),
    };
    AttackWindow {
        phase: Some(phase),
        name: m.name.clone(),
        phase_secs: (t - from).max(0.0) / rate,
        phase_length_secs: (to - from).max(0.0) / rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadouts_pick_their_sets() {
        let l = |main, off| Loadout {
            main,
            off,
            two_handed: false,
        };
        assert_eq!(
            Arms::of(&l(ItemClass::OneHandedBlade, ItemClass::Shield)).set,
            "sword_shield"
        );
        assert!(Arms::of(&l(ItemClass::OneHandedBlade, ItemClass::Shield)).shield);
        assert!(!Arms::of(&l(ItemClass::Axe, ItemClass::None)).shield);
        assert_eq!(
            Arms::of(&l(ItemClass::TwoHandedBlade, ItemClass::None)).set,
            "greatsword"
        );
        assert_eq!(
            Arms::of(&l(ItemClass::TwoHandedAxe, ItemClass::None)).set,
            "greatsword"
        );
        assert_eq!(Arms::of(&l(ItemClass::Bow, ItemClass::None)).set, "bow");
        assert_eq!(Arms::of(&l(ItemClass::None, ItemClass::None)).set, "boxing");
    }
}
