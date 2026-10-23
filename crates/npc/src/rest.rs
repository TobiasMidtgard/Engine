//! NPCs and animals far from every focus rest: they stop sensing, thinking and moving, and
//! cost next to nothing until a focus comes near again (the `SimulationRange` contract).

use engine_core::prelude::*;
use engine_world::{FocusPoints, MoveIntent, SimulationRange};

use crate::agent::Npc;
use crate::wildlife::Animal;

/// On an NPC or animal beyond the simulation range of every focus. It stands where it was,
/// skipped by perception, the brain, wildlife thinking, the fallback mover and its body's
/// posing; it wakes once a focus is back within range.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Resting;

type AgentItem<'a> = (Entity, &'a Transform, Has<Resting>, Has<MoveIntent>);
type Agents = Or<(With<Npc>, With<Animal>)>;

/// Puts far agents to rest and wakes near ones (with the range's hysteresis, so agents at the
/// edge don't flicker).
pub(crate) fn rest_far_agents(
    mut commands: Commands,
    range: Option<Res<SimulationRange>>,
    focus: Res<FocusPoints>,
    agents: Query<AgentItem, Agents>,
) {
    let range = range.map(|r| *r).unwrap_or_default();
    for (e, t, resting, moves) in &agents {
        let p = t.translation;
        if resting && range.is_active(&focus, p) {
            commands.entity(e).remove::<Resting>();
        } else if !resting && range.is_resting(&focus, p) {
            let mut e = commands.entity(e);
            e.insert(Resting);
            if moves {
                e.insert(MoveIntent::default());
            }
        }
    }
}
