//! Tether (round-5-plan.md section 4): a link from the caster's hand to one body that works on
//! it every tick and snaps when stretched too far.
//!
//! A tether is a [`BeamCast`] with a [`TetherLink`], so the spell bar's channel (hold to keep
//! it, mana drains) works for it unchanged. Unlike a beam it doesn't stop at the first thing
//! in its way: it stays on its body wherever that goes. Anything alive that crosses the line
//! is hit too (Io's tether in Dota 2 slows enemies that cross it). Past 1.3 times its range it
//! snaps and fires its Expire stages; held to the end it lands an Impact hit.
//!
//! Magic draws the line itself ([`draw_tethers`]): mesh segments from the hand to the body,
//! sagging while slack, pulling straight and thin as it nears the break, trembling at the
//! edge, and whipping back in two halves when it snaps. Pull and chains tethers are iron
//! chain links; the rest glowing rope in the weave's colour.

use engine_core::math::{DVec3, Mat3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialPreset, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::physics::{ColliderShape, PhysicsQueries, QueryFilter};

use crate::caster::MagicCaster;
use crate::looks::{LookCmd, Role};
use crate::runtime::{BeamCast, HitQueue, MagicQueue, PendingHit, hand_of};

/// A tether breaks this many times its range away.
pub const BREAK_FACTOR: f32 = 1.3;
/// The sweep that finds what a tether aimed at a point links to, m.
pub const LINK_RADIUS: f32 = 0.6;
/// Segments of a glowing rope.
pub const ROPE_SEGMENTS: usize = 16;
/// Links of a chain.
pub const CHAIN_LINKS: usize = 36;
/// Seconds a snapped line takes to whip back (a released one reels in faster).
pub const RECOIL_SECS: f32 = 0.4;

/// What a beam needs to be a tether.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TetherLink {
    /// The body it holds.
    pub target: Entity,
    /// It snaps past this distance, m.
    pub break_m: f32,
    /// How taut it is: 0 slack, 1 at the break (for the look).
    pub strain: f32,
    /// Ticks so far.
    pub ticks: u32,
}

impl TetherLink {
    pub fn new(target: Entity, range: f32) -> Self {
        Self {
            target,
            break_m: range * BREAK_FACTOR,
            strain: 0.0,
            ticks: 0,
        }
    }
}

/// How taut a tether `d` m long is when it breaks at `break_m`: slack up to 40 % of the break,
/// taut at it.
pub fn strain(d: f32, break_m: f32) -> f32 {
    ((d - 0.4 * break_m) / (0.6 * break_m).max(1e-3)).clamp(0.0, 1.0)
}

/// What a tether cast from `hand` links to: the aimed body, else the first body a sphere
/// sweep meets toward the aim within `range` (never the ground under it).
pub fn link_target(
    queries: &PhysicsQueries,
    hand: DVec3,
    aim_point: DVec3,
    aimed: Option<Entity>,
    range: f32,
    exclude: &QueryFilter,
    alive: impl Fn(Entity) -> bool,
) -> Option<Entity> {
    if let Some(t) = aimed {
        return Some(t);
    }
    let dir = (aim_point - hand).as_vec3().normalize_or(Vec3::NEG_Z);
    let mut filter = exclude.clone();
    for _ in 0..4 {
        let h = queries.shapecast(
            &ColliderShape::Sphere {
                radius: LINK_RADIUS,
            },
            hand,
            Quat::IDENTITY,
            dir,
            range,
            &filter,
        )?;
        if alive(h.entity) {
            return Some(h.entity);
        }
        filter.exclude.push(h.entity);
    }
    None
}

/// One step of a tether (from `runtime::tick_beams`): hold on, tick, snap or finish. Returns
/// true when it is over.
#[allow(clippy::too_many_arguments)]
pub fn tick_tether(
    b: &mut BeamCast,
    from: DVec3,
    to: Option<DVec3>,
    dt: f32,
    queries: &PhysicsQueries,
    alive: impl Fn(Entity) -> bool,
    hits: &mut HitQueue,
    queue: &mut MagicQueue,
) -> bool {
    let Some(mut link) = b.tether else {
        return true;
    };
    let spell = b.cast.spell.clone();
    // The body is gone (despawned): the line drops.
    let Some(to) = to else {
        return true;
    };
    b.end = to;
    let span = to - from;
    let d = span.length() as f32;
    let dir = span.as_vec3().normalize_or(Vec3::NEG_Z);
    link.strain = strain(d, link.break_m);
    let hit = |target: Entity, point: DVec3, multi: bool, impact: bool, expire: bool| PendingHit {
        cast: b.cast.clone(),
        target: Some(target),
        point,
        normal: -dir,
        dir,
        multi,
        impact,
        reacts: !expire && multi,
        expire,
    };
    if d > link.break_m {
        // Snapped: sparks at both ends, and its Expire stages where the body is.
        link.strain = 1.0;
        b.tether = Some(link);
        for (at, up) in [(to, -dir), (from, dir)] {
            queue.looks.push(LookCmd::Burst {
                at,
                up,
                look: spell.weave.key.clone(),
                role: Role::Impact,
                scale: 0.5,
                color: spell.weave.color,
            });
        }
        hits.0.push(hit(link.target, to, false, false, true));
        return true;
    }
    b.next_tick -= dt;
    if b.next_tick <= 0.0 {
        b.next_tick += spell.carrier.tick.max(0.05);
        link.ticks += 1;
        hits.0.push(hit(link.target, to, true, false, false));
        // Whatever alive crosses the line takes a tick as well.
        let mut filter = b
            .cast
            .caster
            .map(QueryFilter::excluding)
            .unwrap_or_default();
        filter.exclude.push(link.target);
        for _ in 0..4 {
            let Some(h) = queries.raycast(from, dir, (d - 0.4).max(0.0), &filter) else {
                break;
            };
            if alive(h.entity) {
                hits.0.push(hit(h.entity, h.point, true, false, false));
            }
            filter.exclude.push(h.entity);
        }
        // Sparks where it bites, every other tick.
        if link.ticks % 2 == 1 {
            queue.looks.push(LookCmd::Burst {
                at: to,
                up: -dir,
                look: spell.weave.key.clone(),
                role: Role::Impact,
                scale: 0.3,
                color: spell.weave.color,
            });
        }
    }
    b.tether = Some(link);
    if b.age >= b.duration {
        // Held to the end: it lands.
        hits.0.push(hit(link.target, to, false, true, false));
        return true;
    }
    false
}

/// A tether's drawn line.
#[derive(Component, Debug, Clone)]
pub struct Rope {
    /// The tether it draws.
    pub beam: Entity,
    pub segments: Vec<Entity>,
    pub chain: bool,
    /// Hand and body ends, last seen.
    pub from: DVec3,
    pub to: DVec3,
    pub strain: f32,
    /// Seconds since its tether ended, and whether it snapped.
    pub gone: Option<(f32, bool)>,
    pub radius: f32,
    pub age: f32,
}

/// One piece of a drawn tether.
#[derive(Component, Debug, Clone, Copy)]
pub struct RopeSegment;

/// Does a tether of this weave draw as chain links?
pub fn draws_as_chain(ingredients: &[String]) -> bool {
    ingredients.iter().any(|k| k == "chains" || k == "pull")
}

/// Points along a tether `u` in 0..=1 of the way from `from` to `to`: sagging while slack,
/// trembling near the break, whipping back to both ends over `recoil` (0..1) once snapped,
/// or reeling in toward the hand once let go.
pub fn rope_point(
    from: DVec3,
    to: DVec3,
    u: f32,
    strain: f32,
    age: f32,
    gone: Option<(f32, bool)>,
) -> DVec3 {
    let span = to - from;
    let len = span.length() as f32;
    let dir = span.as_vec3().normalize_or(Vec3::NEG_Z);
    let side = dir.cross(Vec3::Y).normalize_or(Vec3::X);
    let sag = (1.0 - strain).powf(1.5) * 0.12 * len;
    let tremble = if strain > 0.75 {
        0.05 * (strain - 0.75) / 0.25 * (55.0 * age + 9.0 * u).sin()
    } else {
        0.0
    };
    let bow = 4.0 * u * (1.0 - u);
    let straight = |u: f32| from + span * u as f64;
    match gone {
        None => {
            straight(u) - DVec3::Y * (sag * bow) as f64
                + (side * tremble * (std::f32::consts::PI * u).sin()).as_dvec3()
        }
        Some((t, true)) => {
            // Each half springs back to its end, curling as it goes.
            let k = (t / RECOIL_SECS).clamp(0.0, 1.0);
            let (end, v) = if u < 0.5 {
                (from, u / 0.5)
            } else {
                (to, (1.0 - u) / 0.5)
            };
            let reach = 0.5 * (1.0 - k) * (1.0 - k);
            let along = if u < 0.5 {
                end + span * (v * reach) as f64
            } else {
                end - span * (v * reach) as f64
            };
            let curl = (v * 6.0 + 14.0 * t).sin() * 0.35 * v * k.min(1.0 - k) * 2.0;
            along + (side * curl).as_dvec3() - DVec3::Y * (0.6 * v * k) as f64
        }
        Some((t, false)) => {
            let k = (t / (RECOIL_SECS * 0.6)).clamp(0.0, 1.0);
            from + span * (u * (1.0 - k)) as f64 - DVec3::Y * (sag * bow * (1.0 - k)) as f64
        }
    }
}

/// Draw each tether as a line of meshes from the hand to its body; let it whip back and go
/// when the tether ends.
#[allow(clippy::type_complexity)]
pub fn draw_tethers(
    time: Res<Time>,
    beams: Query<(Entity, &BeamCast)>,
    casters: Query<(&GlobalTransform, &MagicCaster)>,
    anchors: Query<&GlobalTransform>,
    mut ropes: Query<(Entity, &mut Rope)>,
    mut segments: Query<&mut Transform, With<RopeSegment>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    // Ropes to start: tethers without one.
    for (e, b) in &beams {
        let Some(link) = b.tether else { continue };
        if ropes.iter().any(|(_, r)| r.beam == e) {
            continue;
        }
        let spell = &b.cast.spell;
        let chain = draws_as_chain(&spell.weave.ingredients);
        let c = spell.weave.color;
        let (shape, material, n) = if chain {
            (
                Shape::Torus {
                    radius: 1.0,
                    tube: 0.22,
                    segments: 12,
                },
                StandardMaterial::preset(MaterialPreset::RustySteel)
                    .with_tint(0.5, 0.48, 0.46)
                    .with_metallic(0.85)
                    .with_roughness(0.45)
                    .with_emissive(c, 1.2),
                CHAIN_LINKS,
            )
        } else {
            (
                Shape::Cylinder {
                    radius: 1.0,
                    height: 1.0,
                    segments: 10,
                },
                StandardMaterial::default()
                    .with_tint(c[0], c[1], c[2])
                    .with_emissive(c, 9.0),
                ROPE_SEGMENTS,
            )
        };
        let from = b
            .cast
            .caster
            .and_then(|c| casters.get(c).ok())
            .map(|(g, c)| hand_of(g, c))
            .unwrap_or(b.end);
        let pieces: Vec<Entity> = (0..n)
            .map(|_| {
                commands
                    .spawn((
                        Name::new("Tether piece"),
                        Transform::from_translation(from).with_scale(Vec3::ZERO),
                        MeshRenderer::new(MeshRef::Shape(shape), MaterialRef::Inline(material)),
                        RopeSegment,
                    ))
                    .id()
            })
            .collect();
        commands.spawn((
            Name::new("Tether"),
            Transform::default(),
            Rope {
                beam: e,
                segments: pieces,
                chain,
                from,
                to: b.end,
                strain: link.strain,
                gone: None,
                radius: (spell.carrier.radius * 0.4).clamp(0.03, 0.08),
                age: 0.0,
            },
        ));
    }
    for (rope_e, mut rope) in &mut ropes {
        rope.age += dt;
        match beams.get(rope.beam) {
            Ok((_, b)) => {
                if let Some((g, c)) = b.cast.caster.and_then(|c| casters.get(c).ok()) {
                    rope.from = hand_of(g, c);
                }
                if let Some(link) = b.tether {
                    rope.to = anchors
                        .get(link.target)
                        .map(|g| g.translation() + DVec3::Y * 1.1)
                        .unwrap_or(b.end);
                    rope.strain = link.strain;
                }
            }
            Err(_) => {
                let snapped = rope.strain >= 0.99;
                let t = rope.gone.map_or(0.0, |g| g.0 + dt);
                rope.gone = Some((t, rope.gone.map_or(snapped, |g| g.1)));
            }
        }
        let over = rope.gone.is_some_and(|(t, snapped)| {
            t > if snapped {
                RECOIL_SECS
            } else {
                RECOIL_SECS * 0.6
            }
        });
        if over {
            for &s in &rope.segments {
                commands.entity(s).despawn();
            }
            commands.entity(rope_e).despawn();
            continue;
        }
        let n = rope.segments.len();
        let len = rope.from.distance(rope.to) as f32;
        // Thinner as it pulls taut; it fades to nothing as it goes.
        let fade = rope
            .gone
            .map_or(1.0, |(t, _)| 1.0 - (t / RECOIL_SECS).min(1.0));
        let thick = rope.radius * (1.0 - 0.45 * rope.strain) * fade;
        let points: Vec<DVec3> = (0..=n)
            .map(|i| {
                rope_point(
                    rope.from,
                    rope.to,
                    i as f32 / n as f32,
                    rope.strain,
                    rope.age,
                    rope.gone,
                )
            })
            .collect();
        let span_dir = (rope.to - rope.from).as_vec3().normalize_or(Vec3::NEG_Z);
        let up = span_dir.cross(Vec3::Y).normalize_or(Vec3::X);
        for (i, &s) in rope.segments.iter().enumerate() {
            let Ok(mut tf) = segments.get_mut(s) else {
                continue;
            };
            let (a, b) = (points[i], points[i + 1]);
            let seg = (b - a).as_vec3();
            let l = seg.length();
            let dir = seg.normalize_or(span_dir);
            tf.translation = a.lerp(b, 0.5);
            if rope.chain {
                // Links alternate a quarter turn about the chain, each a little longer than
                // its share so they overlap; a slack chain bunches them.
                let normal = if i % 2 == 0 {
                    up
                } else {
                    dir.cross(up).normalize_or(Vec3::Y)
                };
                let normal = (normal - dir * normal.dot(dir)).normalize_or(Vec3::Y);
                let z = dir.cross(normal);
                tf.rotation = Quat::from_mat3(&Mat3::from_cols(dir, normal, z));
                let link = (len / n as f32 * 0.75).clamp(0.09, 0.22) * fade;
                tf.scale = Vec3::new(link * 1.35, link, link);
            } else {
                tf.rotation = Quat::from_rotation_arc(Vec3::Y, dir);
                tf.scale = Vec3::new(thick, l * 1.04, thick);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tether_is_slack_near_and_taut_at_the_break() {
        assert_eq!(strain(2.0, 18.0), 0.0);
        assert!((strain(18.0, 18.0) - 1.0).abs() < 1e-6);
        assert!(strain(12.0, 18.0) > 0.3 && strain(12.0, 18.0) < 0.7);
    }

    #[test]
    fn the_line_sags_while_slack_and_runs_straight_when_taut() {
        let (a, b) = (DVec3::new(0.0, 2.0, 0.0), DVec3::new(10.0, 2.0, 0.0));
        let slack = rope_point(a, b, 0.5, 0.0, 0.0, None);
        let taut = rope_point(a, b, 0.5, 1.0, 0.0, None);
        assert!(slack.y < 1.2, "{slack}");
        assert!((taut.y - 2.0).abs() < 0.06, "{taut}");
        // Its ends stay on the hand and the body.
        assert!(rope_point(a, b, 0.0, 0.3, 1.0, None).distance(a) < 1e-6);
        assert!(rope_point(a, b, 1.0, 0.3, 1.0, None).distance(b) < 1e-6);
    }

    #[test]
    fn a_snapped_line_springs_back_to_both_ends() {
        let (a, b) = (DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0));
        let done = Some((RECOIL_SECS, true));
        assert!(rope_point(a, b, 0.4, 1.0, 0.0, done).distance(a) < 0.7);
        assert!(rope_point(a, b, 0.6, 1.0, 0.0, done).distance(b) < 0.7);
        // A let-go line reels in to the hand.
        let reeled = Some((RECOIL_SECS, false));
        assert!(rope_point(a, b, 1.0, 0.0, 0.0, reeled).distance(a) < 1e-6);
    }

    #[test]
    fn pull_and_chains_draw_as_chain_links() {
        assert!(draws_as_chain(&["pull".to_string()]));
        assert!(draws_as_chain(&["frost".to_string(), "chains".to_string()]));
        assert!(!draws_as_chain(&["storm".to_string()]));
    }
}
