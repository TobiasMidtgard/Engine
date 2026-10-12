//! Lock-on targeting for the player. Press `lock_on` (middle mouse or right stick click) to
//! lock onto the enemy closest to the center of the view; press it again to let go.
//!
//! While locked, the character faces the enemy (so the stick strafes around it), attacks and
//! dash-strikes go to it, a [`PilotCamera`](crate::camera::PilotCamera) keeps both in view,
//! and a marker hangs over its head. When it falls or gets away, the lock moves to the next
//! enemy in front, or lets go.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::transform::TransformInterpolation;
use engine_input::ActionState;
use engine_render::{Camera, MeshRenderer, StandardMaterial};

use crate::combat::{CombatState, Fighter};
use crate::look::{self, Library, MeshBuilder, NoGhost};
use crate::mover::{Mover, flat};

/// Lock onto enemies. Put it on a [`PlayerControlled`](crate::PlayerControlled) fighter.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockOn {
    /// The enemy locked onto.
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Farthest enemy to lock onto, m.
    pub range: f32,
    /// The lock lets go beyond this, m.
    pub break_range: f32,
    /// Widest angle from the view's center to pick an enemy, degrees.
    pub cone_deg: f32,
}

impl Default for LockOn {
    fn default() -> Self {
        Self {
            target: None,
            range: 30.0,
            break_range: 45.0,
            cone_deg: 55.0,
        }
    }
}

/// A fighter that could be locked onto.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Candidate {
    entity: Entity,
    pos: DVec3,
    team: u32,
}

/// The best enemy to lock onto from `from` looking along `view`: near the view's center and
/// close by. `None` when nothing is in range inside the cone.
fn pick(
    from: DVec3,
    view: Vec3,
    up: Vec3,
    team: u32,
    lock: &LockOn,
    candidates: &[Candidate],
    skip: Option<Entity>,
) -> Option<Entity> {
    let view = flat(view, up).normalize_or(Vec3::NEG_Z);
    let cone = lock.cone_deg.to_radians();
    candidates
        .iter()
        .filter(|c| c.team != team && Some(c.entity) != skip)
        .filter_map(|c| {
            let to = flat((c.pos - from).as_vec3(), up);
            let dist = to.length();
            let angle = view.angle_between(to.normalize_or(view));
            (dist <= lock.range && angle <= cone)
                .then_some((angle / cone + dist / lock.range, c.entity))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

/// The enemy `me` would lock onto now, looking the way it faces.
pub fn best_target(world: &mut World, me: Entity) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Transform, &Fighter, &CombatState)>();
    let all: Vec<Candidate> = q
        .iter(world)
        .filter(|(_, _, _, cs)| !cs.defeated)
        .map(|(entity, t, f, _)| Candidate {
            entity,
            pos: t.translation,
            team: f.team,
        })
        .collect();
    let (_, t, f, _) = q.get(world, me).ok()?;
    let m = world.get::<Mover>(me)?;
    let lock = world.get::<LockOn>(me).copied().unwrap_or_default();
    pick(t.translation, m.facing, m.up, f.team, &lock, &all, None)
}

/// Toggle the lock and keep it on a live enemy (`FixedUpdate`, before the player's intent).
#[allow(clippy::type_complexity)]
pub(crate) fn lock_on(
    actions: Option<Res<ActionState>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    fighters: Query<(Entity, &Transform, &Fighter, &CombatState)>,
    mut players: Query<(Entity, &Mover, &mut LockOn)>,
    mut candidates: Local<Vec<Candidate>>,
) {
    candidates.clear();
    candidates.extend(fighters.iter().filter(|(_, _, _, cs)| !cs.defeated).map(
        |(entity, t, f, _)| Candidate {
            entity,
            pos: t.translation,
            team: f.team,
        },
    ));
    let view = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.forward());
    let pressed = actions
        .as_ref()
        .is_some_and(|a| a.pressed_since_tick("lock_on"));
    for (me, m, mut lock) in &mut players {
        let Ok((_, t, f, _)) = fighters.get(me) else {
            continue;
        };
        let from = t.translation;
        let view = view.unwrap_or(m.facing);
        if pressed {
            lock.target = match lock.target {
                Some(_) => None,
                None => pick(from, view, m.up, f.team, &lock, &candidates, None),
            };
            continue;
        }
        // Fell or got away: move to the next enemy in front, or let go.
        if let Some(e) = lock.target {
            let alive = candidates.iter().find(|c| c.entity == e);
            let near = alive.is_some_and(|c| c.pos.distance(from) <= lock.break_range as f64);
            if !near {
                lock.target = pick(from, m.facing, m.up, f.team, &lock, &candidates, Some(e));
            }
        }
    }
}

/// The marker over a locked-on enemy.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct LockMarker {
    owner: Entity,
}

const MARKER_MESH: &str = "lockon/marker";
const MARKER_MATERIAL: &str = "lockon/marker";

/// Registers the marker's mesh and material (a glowing crimson diamond).
pub(crate) fn generate(lib: &mut Library) {
    let diamond = MeshBuilder::lathe(
        &[
            Vec2::new(0.0, -0.16),
            Vec2::new(0.085, 0.0),
            Vec2::new(0.0, 0.1),
        ],
        4,
    );
    lib.mesh(MARKER_MESH, diamond);
    let c = engine_render::material::srgb_hex_to_linear(0xff2d3c);
    lib.material(
        MARKER_MATERIAL,
        StandardMaterial::hex(0xff2d3c)
            .with_roughness(0.3)
            .with_emissive(c, 60.0),
    );
}

/// Keep one marker over each locked-on enemy, bobbing and spinning (`Update`).
#[allow(clippy::type_complexity)]
pub(crate) fn place_markers(
    mut commands: Commands,
    time: Res<Time>,
    fixed: Res<engine_core::time::FixedTime>,
    locks: Query<(Entity, &LockOn)>,
    targets: Query<
        (&Transform, Option<&TransformInterpolation>, Option<&Mover>),
        Without<LockMarker>,
    >,
    mut markers: Query<(Entity, &LockMarker, &mut Transform)>,
) {
    let alpha = fixed.alpha();
    let now = time.real_elapsed_secs_f64();
    let mut placed = Vec::new();
    for (entity, marker, mut t) in &mut markers {
        let target = locks.get(marker.owner).ok().and_then(|(_, l)| l.target);
        let Some((tt, interp, m)) = target.and_then(|e| targets.get(e).ok()) else {
            commands.entity(entity).despawn();
            continue;
        };
        let up = m.map_or(Vec3::Y, |m| m.up);
        let pos = interp
            .and_then(|i| i.previous)
            .map_or(tt.translation, |p| p.lerp(tt, alpha).translation);
        let bob = 0.06 * (now * 3.0).sin() as f32;
        t.translation = pos + (up * (1.45 + bob)).as_dvec3();
        t.rotation = Quat::from_rotation_arc(Vec3::Y, up) * Quat::from_rotation_y(now as f32 * 2.5);
        placed.push(marker.owner);
    }
    for (owner, lock) in &locks {
        if lock.target.is_some() && !placed.contains(&owner) {
            commands.spawn((
                Name::new("Lock-on marker"),
                Transform::default(),
                MeshRenderer::new(look::mesh(MARKER_MESH), look::material(MARKER_MATERIAL)),
                LockMarker { owner },
                NoGhost,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fighter(i: u32, x: f64, z: f64, team: u32) -> Candidate {
        Candidate {
            entity: Entity::from_raw_u32(i).unwrap(),
            pos: DVec3::new(x, 0.0, z),
            team,
        }
    }

    #[test]
    fn picks_the_enemy_nearest_the_center_of_view() {
        let lock = LockOn::default();
        let all = [
            fighter(1, 0.0, -2.0, 1),  // a teammate right in front
            fighter(2, 6.0, -6.0, 2),  // 45° off
            fighter(3, 0.5, -12.0, 2), // almost dead ahead, farther
            fighter(4, 0.0, 8.0, 2),   // behind
            fighter(5, 0.0, -40.0, 2), // out of range
        ];
        let got = pick(DVec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1, &lock, &all, None);
        assert_eq!(got, Some(all[2].entity));
        // Skipping that one (it fell): the next one in front.
        let next = pick(DVec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1, &lock, &all, got);
        assert_eq!(next, Some(all[1].entity));
        // Looking backward: the one behind.
        let back = pick(DVec3::ZERO, Vec3::Z, Vec3::Y, 1, &lock, &all, None);
        assert_eq!(back, Some(all[3].entity));
    }
}
