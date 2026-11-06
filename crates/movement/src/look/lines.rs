//! Lines that give under the body: tightropes and rope rails, washing lines and the oak's
//! branches ([`TraversalElement`]s of kind tightrope, clothesline and branch). Each is a
//! chain of stretched pieces re-laid every frame, pressed down under every walker's feet and
//! every hanger's hands exactly as far as [`crate::balance`] and [`crate::bar`] press them,
//! so the rope meets the feet and the branch bends where the hunter crouches.
//!
//! A rope or a washing line sags between its ends and dips into a shallow V under each load;
//! a branch is a cantilever: straight unloaded, bending more the farther out the weight is,
//! its leafy tip following.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::MeshRenderer;

use super::materials as mat;
use super::{Library, MeshBuilder};
use crate::mover::Mover;
use crate::traverse::{ElementKind, TraversalElement};

const PIECE_MESH: &str = "lines/piece";
const TUFT_MESH: &str = "lines/tuft";

/// Registers the unit piece (radius 1, one meter along +Y) and the leafy tuft on a branch's
/// tip.
pub(crate) fn generate(lib: &mut Library) {
    let mut piece = MeshBuilder::tube(&[Vec3::ZERO, Vec3::Y], 1.0, 10);
    // The lay of a rope or the ridges of bark, read in the shading.
    piece.shade(|p, n| 0.85 + 0.15 * (n.x.atan2(n.z) * 3.0 + p.y * 9.0).sin());
    lib.mesh(PIECE_MESH, piece);
    lib.mesh(
        TUFT_MESH,
        super::city::leaf_cluster(Vec3::ZERO, Vec3::new(0.9, 0.6, 0.9), 900, 77),
    );
}

/// How many pieces a line of this kind is drawn with.
fn pieces(kind: ElementKind) -> usize {
    match kind {
        ElementKind::Branch => 14,
        ElementKind::Clothesline => 20,
        _ => 36,
    }
}

/// The radius at `u` (0..1 from `a`) of a line of `kind`, `width` wide.
fn radius(kind: ElementKind, width: f32, u: f32) -> f32 {
    match kind {
        ElementKind::Branch => width * 0.5 * (1.0 - 0.7 * u),
        ElementKind::Clothesline => 0.006,
        _ => (width * 0.5).clamp(0.012, 0.022),
    }
}

/// How far down the line is pressed at `u` by a load pressing it `d` m down at `at`.
pub fn deflection(kind: ElementKind, u: f32, at: f32, d: f32) -> f32 {
    if kind == ElementKind::Branch {
        // A cantilever from `a`: x²(3a − x) up to the load, straight on beyond it.
        let a = at.max(1e-3);
        let k = if u <= a {
            (u / a).powi(2) * (3.0 - u / a) * 0.5
        } else {
            1.0 + 1.5 * (u - a) / a
        };
        d * k
    } else {
        // A tent from the two anchors to the load.
        let k = if u <= at {
            u / at.max(1e-4)
        } else {
            (1.0 - u) / (1.0 - at).max(1e-4)
        };
        d * k
    }
}

/// The pieces of one line (on the line's entity).
#[derive(Component, Debug, Clone)]
pub struct LinePieces {
    pieces: Vec<Entity>,
    tuft: Option<Entity>,
}

/// A piece of a line (despawned with it).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct LinePart {
    line: Entity,
}

fn drawn(kind: ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Tightrope | ElementKind::Clothesline | ElementKind::Branch
    )
}

/// Gives every new sagging line its pieces and lays them along it, pressed down under each
/// walker and hanger (`Update`).
#[allow(clippy::type_complexity)]
pub(crate) fn draw_lines(
    mut commands: Commands,
    lines: Query<(
        Entity,
        &GlobalTransform,
        &TraversalElement,
        Option<&LinePieces>,
    )>,
    movers: Query<&Mover>,
    mut parts: Query<&mut Transform, (With<LinePart>, Without<TraversalElement>)>,
    mut loads: Local<Vec<(Entity, f32, f32)>>,
) {
    loads.clear();
    for m in &movers {
        if let Some(w) = m.line {
            loads.push((w.element, w.s, w.load));
        }
        if let Some(h) = m.bar {
            loads.push((h.element, h.s, h.load));
        }
    }
    for (line, g, el, has) in &lines {
        if !drawn(el.kind) {
            continue;
        }
        let Some(has) = has else {
            let (material, n) = match el.kind {
                ElementKind::Branch => (mat::BARK, pieces(el.kind)),
                _ => (mat::CLOTH, pieces(el.kind)),
            };
            let spawn = |commands: &mut Commands, mesh: &str, material: &str| {
                commands
                    .spawn((
                        Name::new("Line piece"),
                        Transform::from_translation(g.translation()),
                        MeshRenderer::new(super::mesh(mesh), mat::material(material)),
                        LinePart { line },
                    ))
                    .id()
            };
            let pieces = (0..n)
                .map(|_| spawn(&mut commands, PIECE_MESH, material))
                .collect();
            let tuft = (el.kind == ElementKind::Branch)
                .then(|| spawn(&mut commands, TUFT_MESH, mat::LEAVES));
            commands.entity(line).insert(LinePieces { pieces, tuft });
            continue;
        };
        let t = g.to_transform();
        let a = t.translation + (t.rotation * el.a).as_dvec3();
        let b = t.translation + (t.rotation * el.b).as_dvec3();
        let up = Vec3::Y;
        let len = (b - a).length() as f32;
        let n = has.pieces.len();
        let on: Vec<(f32, f32)> = loads
            .iter()
            .filter(|l| l.0 == line)
            .map(|l| ((l.1 / len.max(1e-3)).clamp(0.0, 1.0), l.2))
            .collect();
        let point = |u: f32| -> DVec3 {
            let mut drop = if el.kind == ElementKind::Branch {
                0.0
            } else {
                4.0 * el.sag * u * (1.0 - u)
            };
            for &(at, d) in &on {
                drop += deflection(el.kind, u, at, d);
            }
            a + (b - a) * u as f64 - (up * drop).as_dvec3()
        };
        for (i, &e) in has.pieces.iter().enumerate() {
            let (u0, u1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            let (p0, p1) = (point(u0), point(u1));
            let d = (p1 - p0).as_vec3();
            let l = d.length().max(1e-4);
            let r = radius(el.kind, el.width, (u0 + u1) * 0.5);
            // Its top on the line walked (a hanger's hands close round it: near enough).
            let lift = r;
            if let Ok(mut pt) = parts.get_mut(e) {
                pt.set_if_neq(Transform {
                    translation: p0 - (up * lift).as_dvec3(),
                    rotation: Quat::from_rotation_arc(Vec3::Y, d / l),
                    scale: Vec3::new(r, l + r * 0.5, r),
                });
            }
        }
        if let Some(tuft) = has.tuft
            && let Ok(mut pt) = parts.get_mut(tuft)
        {
            let tip = point(1.0);
            let bend = (point(1.0) - point(0.9)).as_vec3().normalize_or(Vec3::X);
            pt.set_if_neq(Transform {
                translation: tip + (bend * 0.5 + up * 0.25).as_dvec3(),
                rotation: Quat::from_rotation_arc(
                    Vec3::X,
                    Vec3::new(bend.x, 0.0, bend.z).normalize_or(Vec3::X),
                ),
                scale: Vec3::ONE,
            });
        }
    }
}

/// Despawns the pieces of lines that are gone (`Update`).
pub(crate) fn drop_orphan_lines(
    mut commands: Commands,
    parts: Query<(Entity, &LinePart)>,
    lines: Query<(), With<TraversalElement>>,
) {
    for (e, part) in &parts {
        if !lines.contains(part.line) {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_pressed_down_exactly_under_the_load() {
        for kind in [ElementKind::Tightrope, ElementKind::Branch] {
            let d = deflection(kind, 0.4, 0.4, 0.2);
            assert!((d - 0.2).abs() < 1e-5, "{kind:?}: {d}");
        }
        // The anchors of a rope don't move; a branch's root doesn't.
        assert_eq!(deflection(ElementKind::Tightrope, 0.0, 0.5, 0.2), 0.0);
        assert_eq!(deflection(ElementKind::Branch, 0.0, 0.5, 0.2), 0.0);
        // A branch's tip beyond the load dips further.
        assert!(deflection(ElementKind::Branch, 1.0, 0.5, 0.2) > 0.2);
    }
}
