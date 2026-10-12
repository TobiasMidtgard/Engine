//! Following the world when it is re-anchored: a floating-origin move, or a new reference frame
//! (`engine_world::WorldShift`). engine_world moves the bodies (root transforms, move intents,
//! locomotion velocities); this moves what NPCs and animals keep in world space of their own,
//! in `WorldShiftSet::Apply`, so after a shift they still head for, aim at, remember and return
//! to the same places in the new frame.
//!
//! * Points move with `WorldShift::point`. Directions turn with `WorldShift::direction` and stay
//!   level, like every heading the AI keeps in XZ. Remembered velocities of bodies pick up the
//!   old frame's own motion with `WorldShift::velocity_at`, as `LocomotionState` does, so old
//!   observations and new ones agree.
//! * The tactical world moves in place (a turned world is also rebuilt from the colliders on the
//!   next step); the navigation grid drops its chunks and rebuilds them where paths are asked for.
//! * `Perceivables` are not shifted: they are gathered again at the start of every step, before
//!   anything reads them.

use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::agent::{AttackPhase, Npc, NpcBrain, NpcCombat, NpcOrder};
use crate::locomotion::MoverState;
use crate::nav::{NavAgent, NavGrid};
use crate::perception::NpcMemory;
use crate::wildlife::{Animal, AnimalMind, BirdFlight};
use crate::world_model::TacticalWorld;

/// All shifts written this frame, combined (they apply in order).
fn combined(shifts: &mut MessageReader<WorldShift>) -> Option<WorldShift> {
    shifts.read().copied().reduce(|acc, next| acc.then(&next))
}

/// A level heading (XZ, same length) in the new frame.
fn level(s: &WorldShift, v: Vec3) -> Vec3 {
    let d = s.direction(v);
    Vec3::new(d.x, 0.0, d.z).normalize_or_zero() * v.length()
}

/// Fighters' homes, orders, destinations, chosen spots, memories, aim points, paths and dashes
/// (paths and dashes of animals too).
pub(crate) fn shift_agents(
    mut shifts: MessageReader<WorldShift>,
    mut npcs: Query<&mut Npc>,
    mut brains: Query<&mut NpcBrain>,
    mut memories: Query<&mut NpcMemory>,
    mut combats: Query<&mut NpcCombat>,
    mut agents: Query<&mut NavAgent>,
    mut movers: Query<&mut MoverState>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    let moved = |p: DVec3| s.point(p);
    for mut npc in &mut npcs {
        npc.home = npc.home.map(moved);
    }
    for mut brain in &mut brains {
        let b = &mut *brain;
        b.home = b.home.map(moved);
        b.destination = b.destination.map(moved);
        b.high_ground = b.high_ground.map(moved);
        if let Some(NpcOrder::Hold { at } | NpcOrder::MoveTo { at }) = &mut b.order {
            *at = moved(*at);
        }
        for spot in &mut b.spots {
            spot.position = moved(spot.position);
        }
    }
    for mut memory in &mut memories {
        let m = &mut *memory;
        for k in &mut m.known {
            k.last_position = moved(k.last_position);
            k.last_velocity = s.velocity_at(k.last_position, k.last_velocity);
            for h in &mut k.history {
                h.position = moved(h.position);
                h.velocity = s.velocity_at(h.position, h.velocity);
                h.forward = level(&s, h.forward);
            }
        }
        if let Some(st) = &mut m.stimulus {
            st.position = moved(st.position);
        }
    }
    for mut combat in &mut combats {
        if let AttackPhase::Windup { aim, .. } = &mut combat.phase {
            *aim = moved(*aim);
        }
    }
    for mut nav in &mut agents {
        let n = &mut *nav;
        for p in &mut n.path {
            *p = moved(*p);
        }
        n.goal = n.goal.map(moved);
    }
    for mut mover in &mut movers {
        mover.shift(&s);
    }
}

/// Animals' homes, threats, destinations, noises heard, water spots, flight and landing spots.
pub(crate) fn shift_wildlife(
    mut shifts: MessageReader<WorldShift>,
    mut animals: Query<&mut Animal>,
    mut minds: Query<&mut AnimalMind>,
    mut birds: Query<(&mut BirdFlight, Option<&GlobalTransform>)>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    let moved = |p: DVec3| s.point(p);
    for mut animal in &mut animals {
        animal.home = animal.home.map(moved);
    }
    for mut mind in &mut minds {
        let m = &mut *mind;
        m.home = m.home.map(moved);
        m.destination = m.destination.map(moved);
        m.threat_pos = m.threat_pos.map(moved);
        m.hurt_from = m.hurt_from.map(moved);
        m.heard = m.heard.map(|(at, loud)| (moved(at), loud));
        m.water = m.water.map(|w| w.map(moved));
        m.flee_dir = level(&s, m.flee_dir);
    }
    for (mut bird, g) in &mut birds {
        // `GlobalTransform` still holds the old frame until transforms propagate after `Apply`.
        let at = moved(g.map_or(DVec3::ZERO, |g| g.translation()));
        bird.velocity = s.velocity_at(at, bird.velocity);
        bird.landing = bird.landing.map(moved);
    }
}

/// The tactical world moves with the world; navigation chunks cache heights at old cells.
pub(crate) fn shift_world_model(
    mut shifts: MessageReader<WorldShift>,
    mut world: ResMut<TacticalWorld>,
    mut nav: ResMut<NavGrid>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    world.shift(&s);
    nav.clear();
}
