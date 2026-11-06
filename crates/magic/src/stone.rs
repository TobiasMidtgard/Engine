//! Shaping: stone that is really there (round-3-plan.md). A spell with the `stone` effect
//! raises rough rock out of the ground: a block per piece of a wall, a pillar under an area, a
//! step where a bolt lands. Each is a kinematic physics body (the engine_world physics
//! contract), so bolts burst on it, crates and people stand on it, and a rising pillar lifts
//! whatever is on top. Stone comes up heavier than light (a wall in 0.35 s with a small
//! overshoot and a puff of dust; a pillar slowly enough to ride), stands for the spell's time or
//! its wall's, and grinds back down. A stone grip is a ring of rocks around a body's feet that
//! holds it (`Statuses::rooted`) until it wears off or fire cracks it.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialPreset, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::SmokeEmission;
use engine_world::physics::{
    BodyKind, CharacterController, Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody,
};

use crate::runtime::{LookPiece, MagicQueue, roll};
use crate::status::Afflictions;

/// Seconds a wall's stone takes to rise.
pub const RISE_SECS: f32 = 0.35;
/// How far past its height it heaves (1.06 = 6 %): stone is heavy, so less than ice.
pub const OVERSHOOT: f32 = 1.06;
/// Seconds it takes to grind back down.
pub const SINK_SECS: f32 = 1.2;

/// What the stone is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoneKind {
    /// A block of a wall, one per piece.
    Wall,
    /// A column under an area: it lifts what stands there.
    Pillar,
    /// A step where a bolt struck the ground.
    Block,
    /// One of the rocks closing around a body's feet (no collider: it would shove the body).
    Grip,
}

impl StoneKind {
    /// Seconds it takes to rise `height` m: walls and steps heave up, a pillar climbs slowly
    /// enough that what stands on it rides up instead of being thrown.
    pub fn rise_secs(self, height: f32) -> f32 {
        match self {
            StoneKind::Wall | StoneKind::Block => RISE_SECS,
            StoneKind::Pillar => (0.6 + 0.3 * height).clamp(1.0, 2.4),
            StoneKind::Grip => 0.25,
        }
    }
}

/// Stone to raise, queued for the flush.
#[derive(Debug, Clone)]
pub struct NewStone {
    pub kind: StoneKind,
    /// The ground under its center.
    pub at: DVec3,
    /// The direction its width runs (a wall's line).
    pub along: Vec3,
    /// Width (along), height, depth, m.
    pub size: Vec3,
    /// The wall piece it stands for: it sinks with it. `None`: it stands `life` seconds.
    pub owner: Option<Entity>,
    pub life: f32,
    pub seed: u64,
}

/// A raised stone.
#[derive(Component, Debug, Clone)]
pub struct StoneBody {
    pub kind: StoneKind,
    pub owner: Option<Entity>,
    /// The ground under it: the stone's base when it stands.
    pub base: DVec3,
    pub height: f32,
    pub rise_secs: f32,
    pub age: f32,
    pub life: f32,
    /// Seconds since it began to sink.
    pub sink: Option<f32>,
    pub dusted: bool,
    /// Its base is on solid ground (a pillar looks again under loose bodies on its first step).
    pub grounded: bool,
}

/// How much of its height stands above the ground `age` s into a rise of `secs`: up past it,
/// then back. The ease-out decelerates at 2·1.06·h/(0.7·secs)², under gravity for a pillar,
/// so a rider stays on it.
pub fn rise(age: f32, secs: f32) -> f32 {
    let up = secs * 0.7;
    if age <= 0.0 {
        0.0
    } else if age < up {
        // Ease out: it bursts up and slows as it nears the top.
        let t = age / up;
        OVERSHOOT * (1.0 - (1.0 - t) * (1.0 - t))
    } else if age < secs {
        OVERSHOOT + (1.0 - OVERSHOOT) * (age - up) / (secs - up)
    } else {
        1.0
    }
}

/// The size of the stone a spell raises: `stone_m` is its height, `width` a wall piece's.
pub fn stone_size(kind: StoneKind, stone_m: f32, width: f32) -> Vec3 {
    match kind {
        StoneKind::Wall => Vec3::new(width * 1.08, stone_m, 0.75),
        // A pillar stands a third taller than a wall and as thick as a man is tall.
        StoneKind::Pillar => {
            let d = (0.6 + 0.25 * stone_m).clamp(1.0, 2.2);
            Vec3::new(d, stone_m * 1.35, d)
        }
        StoneKind::Block => Vec3::new(1.2, (stone_m * 0.45).clamp(0.5, 1.3), 1.2),
        StoneKind::Grip => Vec3::new(0.5, 0.7, 0.42),
    }
}

/// Raise a stone (from the flush). It starts below the ground and comes up.
pub fn spawn_stone(commands: &mut Commands, s: NewStone) -> Entity {
    let lean = (roll(s.seed, 11) - 0.5) * 0.05;
    let twist = (roll(s.seed, 12) - 0.5) * 0.12;
    // A grip's rocks lean in over the feet (their local +Z faces the body).
    let close = if s.kind == StoneKind::Grip { 0.38 } else { 0.0 };
    let rest = Quat::from_rotation_arc(Vec3::X, s.along.normalize_or(Vec3::X))
        * Quat::from_rotation_y(twist)
        * Quat::from_rotation_z(lean)
        * Quat::from_rotation_x(close);
    let size = s.size;
    let material = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
    let mut ec = commands.spawn((
        Name::new("Spell stone"),
        Transform::from_translation(s.at - DVec3::Y * size.y as f64).with_rotation(rest),
        MeshRenderer::new(
            MeshRef::Shape(Shape::Rock {
                size: size.to_array(),
                seed: (s.seed % 100_000) as u32,
                detail: 3,
            }),
            MaterialRef::Inline(material),
        ),
        StoneBody {
            kind: s.kind,
            owner: s.owner,
            base: s.at,
            height: size.y,
            rise_secs: s.kind.rise_secs(size.y),
            age: 0.0,
            life: s.life,
            sink: None,
            dusted: false,
            grounded: s.kind != StoneKind::Pillar,
        },
    ));
    if s.kind != StoneKind::Grip {
        ec.insert((
            // A box a little inside the rock, so what hits it meets the rock's face.
            Collider {
                shape: ColliderShape::Compound(vec![(
                    Vec3::new(0.0, size.y * 0.5, 0.0),
                    Quat::IDENTITY,
                    ColliderShape::Cuboid {
                        half_extents: Vec3::new(size.x * 0.46, size.y * 0.5, size.z * 0.44),
                    },
                )]),
                density: 2600.0,
                ..Default::default()
            },
            RigidBody {
                kind: BodyKind::Kinematic,
                ..Default::default()
            },
        ));
    }
    ec.id()
}

/// The rocks of a stone grip around feet at `at`, leaning in (from the hit that roots).
pub fn grip_stones(at: DVec3, holder: Entity, secs: f32, seed: u64) -> Vec<NewStone> {
    (0..4)
        .map(|i| {
            let a = std::f32::consts::TAU * (i as f32 + 0.25 * roll(seed, i)) / 4.0;
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            NewStone {
                kind: StoneKind::Grip,
                at: at + (out * 0.38).as_dvec3(),
                // Its width runs around the feet.
                along: Vec3::new(-out.z, 0.0, out.x),
                size: stone_size(StoneKind::Grip, 0.0, 0.0) * (0.85 + 0.3 * roll(seed, 20 + i)),
                owner: Some(holder),
                life: secs,
                seed: seed.wrapping_add(i * 977),
            }
        })
        .collect()
}

/// The solid ground under `p`: the first surface that isn't a loose body or a character, so a
/// pillar rises under a crate stack (and lifts it), not on top of it.
pub fn solid_ground_below(
    queries: &PhysicsQueries,
    p: DVec3,
    loose: impl Fn(Entity) -> bool,
) -> Option<DVec3> {
    let mut filter = QueryFilter::default();
    for _ in 0..8 {
        let h = queries.raycast(p + DVec3::Y * 2.5, Vec3::NEG_Y, 40.0, &filter)?;
        if !loose(h.entity) {
            return Some(h.point);
        }
        filter.exclude.push(h.entity);
    }
    None
}

/// Raise stones, hold them while their wall or time lasts, then grind them back down.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn raise_stone(
    time: Res<Time>,
    mut stones: Query<(Entity, &mut StoneBody, &mut Transform)>,
    pieces: Query<&LookPiece>,
    held: Query<&Afflictions>,
    queries: Option<Res<PhysicsQueries>>,
    bodies: Query<(
        Option<&RigidBody>,
        Has<CharacterController>,
        Option<&ChildOf>,
    )>,
    mut queue: ResMut<MagicQueue>,
    mut dust: MessageWriter<SmokeEmission>,
) {
    let dt = time.delta_secs();
    // A body that moves on its own (or a part of one): stone rises under it.
    let loose = |mut e: Entity| {
        for _ in 0..4 {
            let Ok((rb, character, parent)) = bodies.get(e) else {
                return false;
            };
            if character || rb.is_some_and(|b| b.kind == BodyKind::Dynamic) {
                return true;
            }
            match (rb, parent) {
                (None, Some(p)) => e = p.parent(),
                _ => return false,
            }
        }
        false
    };
    for (e, mut s, mut tf) in &mut stones {
        if !s.grounded {
            s.grounded = true;
            if let Some(q) = queries.as_deref()
                && let Some(g) = solid_ground_below(q, s.base, loose)
            {
                s.base = g;
            }
        }
        s.age += dt;
        if !s.dusted {
            // Dust and grit thrown up where it breaks the ground.
            s.dusted = true;
            dust.write(SmokeEmission {
                position: s.base + DVec3::Y * 0.3,
                velocity: Vec3::Y * 1.4,
                radius: (s.height * 0.4).clamp(0.6, 2.0),
                mass_kg: 0.05 * s.height,
                temperature_c: 25.0,
                darkness: 0.3,
            });
        }
        let ended = match (s.kind, s.owner) {
            // A grip holds while its body is rooted (fire cracks it early).
            (StoneKind::Grip, Some(b)) => {
                s.age > 0.5 && held.get(b).ok().is_none_or(|a| a.rooted <= 0.0) || s.age > s.life
            }
            (_, Some(p)) => pieces.get(p).ok().is_none_or(|p| p.fading.is_some()),
            (_, None) => s.age > s.life,
        };
        if ended && s.sink.is_none() {
            s.sink = Some(0.0);
        }
        let sink_secs = if s.kind == StoneKind::Grip {
            0.5
        } else {
            SINK_SECS
        };
        let mut up = rise(s.age, s.rise_secs);
        if let Some(t) = s.sink.as_mut() {
            *t += dt;
            let k = (*t / sink_secs).min(1.0);
            up *= 1.0 - k * k;
            if *t >= sink_secs {
                queue.despawn.push(e);
            }
        }
        tf.translation = s.base - DVec3::Y * (s.height * (1.0 - up)) as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stone_heaves_up_past_its_height_and_settles() {
        assert_eq!(rise(0.0, RISE_SECS), 0.0);
        let peak = (0..200)
            .map(|i| rise(i as f32 * RISE_SECS / 200.0, RISE_SECS))
            .fold(0.0, f32::max);
        assert!((peak - OVERSHOOT).abs() < 0.01, "{peak}");
        assert_eq!(rise(RISE_SECS, RISE_SECS), 1.0);
        assert_eq!(rise(30.0, RISE_SECS), 1.0);
    }

    #[test]
    fn a_pillar_climbs_gently_enough_to_ride() {
        // Its ease-out decelerates slower than gravity, so a body on top stays on it.
        for stone_m in [1.2, 2.1, 3.3, 5.0] {
            let h = stone_size(StoneKind::Pillar, stone_m, 0.0).y * 1.4;
            let secs = StoneKind::Pillar.rise_secs(h);
            let up = 0.7 * secs;
            let decel = 2.0 * OVERSHOOT * h / (up * up);
            assert!(decel < 9.81, "{stone_m} m: {decel} m/s²");
        }
    }

    #[test]
    fn a_grip_closes_around_the_feet() {
        let stones = grip_stones(DVec3::ZERO, Entity::from_bits(1 << 32 | 7), 4.0, 99);
        assert_eq!(stones.len(), 4);
        for s in &stones {
            let r = s.at.length();
            assert!((0.3..0.5).contains(&r), "{r}");
            assert!(s.size.y < 1.0 && s.kind == StoneKind::Grip);
        }
    }

    #[test]
    fn pillars_stand_taller_than_walls_and_steps_are_low() {
        let wall = stone_size(StoneKind::Wall, 2.2, 1.4);
        let pillar = stone_size(StoneKind::Pillar, 2.2, 0.0);
        let step = stone_size(StoneKind::Block, 2.2, 0.0);
        assert!(pillar.y > wall.y && wall.y > step.y);
        assert!(step.y <= 1.3 && step.x >= 1.0);
    }
}
