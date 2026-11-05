//! Hanging ropes ([`ClimbRope`]): a three-strand hemp rope from its anchor, straight down at
//! rest, swaying a little; held, it runs taut from the anchor to the climber's hands and its
//! tail hangs from below the hands, trailing the swing. A knot at the end. Nothing here moves
//! anyone (see [`crate::climbrope`]).
//!
//! The rope is a chain of [`SEGMENTS`] stretched pieces re-laid every frame.

use std::f32::consts::TAU;

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_core::transform::TransformInterpolation;
use engine_render::{MeshRenderer, StandardMaterial};

use super::{Library, MeshBuilder};
use crate::climbrope::ClimbRope;
use crate::mover::Mover;

/// Pieces of rope per rope.
pub const SEGMENTS: usize = 40;

const PIECE_MESH: &str = "climbrope/piece";
const KNOT_MESH: &str = "climbrope/knot";
const MATERIAL: &str = "climbrope/hemp";

/// Registers the rope piece, the knot and the hemp.
pub(crate) fn generate(lib: &mut Library) {
    // One meter along +Y at unit radius (scaled per rope); three strands twisted around.
    let mut piece = MeshBuilder::tube(&[Vec3::ZERO, Vec3::Y], 1.0, 10);
    piece.shade(|p, n| {
        let a = n.x.atan2(n.z);
        let strand = (3.0 * a + p.y * TAU * 9.0).cos();
        0.7 + 0.3 * strand * strand
    });
    lib.mesh(PIECE_MESH, piece);
    let knot = MeshBuilder::lathe(
        &[
            Vec2::new(0.0, -1.6),
            Vec2::new(1.2, -1.3),
            Vec2::new(1.9, 0.0),
            Vec2::new(1.2, 1.3),
            Vec2::new(0.0, 1.6),
        ],
        12,
    );
    lib.mesh(KNOT_MESH, knot);
    lib.material(
        MATERIAL,
        StandardMaterial::hex(0xa48a5e).with_roughness(0.92),
    );
}

/// The pieces of one rope (on the rope's entity).
#[derive(Component, Debug, Clone)]
pub struct RopePieces {
    pieces: Vec<Entity>,
    knot: Entity,
}

/// A piece of a rope (despawned with it).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct RopePart {
    rope: Entity,
}

fn visual(t: &Transform, interp: Option<&TransformInterpolation>, alpha: f32) -> Transform {
    interp
        .and_then(|i| i.previous)
        .map_or(*t, |p| p.lerp(t, alpha))
}

/// Points along a rope from `anchor`, `length` long: through `hands` (if held, `held` meters
/// down it) and hanging on below, its tail swinging out along `trail` (the hands' velocity's
/// opposite, m/s) and bending down under its weight. Unheld, it hangs straight with a sway.
pub fn rope_points(
    anchor: DVec3,
    length: f32,
    held: Option<(DVec3, f32, Vec3)>,
    sway: Vec3,
) -> [DVec3; SEGMENTS + 1] {
    std::array::from_fn(|i| {
        let s = length * i as f32 / SEGMENTS as f32;
        match held {
            Some((hands, at, _)) if s <= at => {
                anchor + (hands - anchor) * (s / at.max(1e-3)) as f64
            }
            Some((hands, at, trail)) => {
                let below = s - at;
                // The tail leans back against the swing, less and less down its length.
                let lean = trail * (0.08 * below).min(0.6) * (below / (below + 1.0));
                hands + DVec3::new(0.0, -below as f64, 0.0) + (lean * below.min(2.0)).as_dvec3()
            }
            None => {
                let k = s / length.max(1e-3);
                anchor + DVec3::new(0.0, -s as f64, 0.0) + (sway * k * k).as_dvec3()
            }
        }
    })
}

/// Gives every new rope its pieces and lays each one along its anchor and climber
/// (`Update`, after the puppets are posed).
#[allow(clippy::type_complexity)]
pub(crate) fn draw_climb_ropes(
    mut commands: Commands,
    time: Res<Time>,
    fixed: Res<FixedTime>,
    ropes: Query<(Entity, &Transform, &ClimbRope, Option<&RopePieces>)>,
    climbers: Query<(&Mover, &Transform, Option<&TransformInterpolation>)>,
    mut parts: Query<&mut Transform, (With<RopePart>, Without<ClimbRope>, Without<Mover>)>,
) {
    let alpha = fixed.alpha();
    let t = time.elapsed_secs();
    for (rope, rt, r, pieces) in &ropes {
        let Some(pieces) = pieces else {
            let piece = |commands: &mut Commands| {
                commands
                    .spawn((
                        Name::new("Rope"),
                        Transform::from_translation(rt.translation),
                        MeshRenderer::new(super::mesh(PIECE_MESH), super::material(MATERIAL)),
                        RopePart { rope },
                    ))
                    .id()
            };
            let list = (0..SEGMENTS).map(|_| piece(&mut commands)).collect();
            let knot = commands
                .spawn((
                    Name::new("Rope knot"),
                    Transform::from_translation(rt.translation),
                    MeshRenderer::new(super::mesh(KNOT_MESH), super::material(MATERIAL)),
                    RopePart { rope },
                ))
                .id();
            commands
                .entity(rope)
                .insert(RopePieces { pieces: list, knot });
            continue;
        };
        let anchor = rt.translation;
        let held = climbers.iter().find_map(|(m, ct, interp)| {
            let hold = m.rope.filter(|h| h.rope == Some(rope))?;
            // The hands where the body is drawn: the physics' hands, moved with the body.
            let body = visual(ct, interp, alpha).translation;
            let hands_phys = anchor + (hold.dir * hold.s).as_dvec3();
            let hands = body + (hands_phys - ct.translation);
            Some((hands, hold.s, -hold.swing * 0.3))
        });
        // A gentle sway when nobody holds it.
        let phase = (anchor.x * 0.37 + anchor.z * 0.21) as f32;
        let sway = Vec3::new((t * 0.9 + phase).sin(), 0.0, (t * 0.7 + phase * 1.3).cos()) * 0.12;
        let points = rope_points(anchor, r.length, held, sway);
        let radius = r.radius.max(0.005);
        for (i, &e) in pieces.pieces.iter().enumerate() {
            let (a, b) = (points[i], points[i + 1]);
            let d = (b - a).as_vec3();
            let len = d.length().max(1e-4);
            if let Ok(mut pt) = parts.get_mut(e) {
                pt.set_if_neq(Transform {
                    translation: a,
                    rotation: Quat::from_rotation_arc(Vec3::Y, d / len),
                    scale: Vec3::new(radius, len, radius),
                });
            }
        }
        if let Ok(mut kt) = parts.get_mut(pieces.knot) {
            kt.set_if_neq(Transform {
                translation: points[SEGMENTS],
                rotation: Quat::IDENTITY,
                scale: Vec3::splat(radius),
            });
        }
    }
}

/// Despawns the pieces of ropes that are gone (`Update`).
pub(crate) fn drop_orphan_climb_ropes(
    mut commands: Commands,
    parts: Query<(Entity, &RopePart)>,
    ropes: Query<(), With<ClimbRope>>,
) {
    for (e, part) in &parts {
        if !ropes.contains(part.rope) {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_rope_runs_through_the_hands_and_hangs_on_below() {
        let anchor = DVec3::new(0.0, 10.0, 0.0);
        let hands = DVec3::new(1.0, 6.0, 0.0);
        let p = rope_points(anchor, 8.0, Some((hands, 4.1, Vec3::ZERO)), Vec3::ZERO);
        assert_eq!(p[0], anchor);
        // The point nearest 4.1 m down is on the line to the hands.
        let i = (4.0 / 8.0 * SEGMENTS as f32) as usize;
        let on = anchor + (hands - anchor) * (4.0 / 4.1);
        assert!(p[i].distance(on) < 1e-6, "{:?} vs {on:?}", p[i]);
        // The end hangs straight below the hands.
        let end = p[SEGMENTS];
        assert!((end.x - 1.0).abs() < 1e-6 && (end.y - (6.0 - 3.9)).abs() < 1e-4);
    }
}
