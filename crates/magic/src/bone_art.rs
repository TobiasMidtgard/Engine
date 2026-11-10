//! The shapes bone is drawn with (round-18-plan.md), made in code: the paths ribs take as they
//! break out of the floor (closing over a foe like fingers, arching over a ribcage wall, a spine
//! along its ridge, spikes across an area), the torn membrane of a bone dragon's wings, and the
//! old bone everything is made of.
//!
//! A rib is drawn as tapered bone segments laid along one of these paths by the Wilds' root
//! machinery (wilds.rs), so the paths here are plain points: the first under the floor where the
//! rib breaks out, the last at its tip.

use std::f32::consts::PI;

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::{MaterialPreset, Mesh, StandardMaterial, Vertex};

use crate::pact_art::hash2;
use crate::wilds_art::UNDER_M;

// ------------------------------------------------------------------------------- materials

/// Old bone: plaster's warm pale ground, its cracks and chips at a bone's scale, a little
/// yellowed, mapped in the world so thin pieces keep it.
pub fn bone() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::Plaster)
        .with_triplanar_world()
        .with_texture_scale(0.35)
        .with_tint(1.0, 0.92, 0.76)
}

/// Bone gone dark with age where it meets the ground and in its hollows (sockets, the spurs).
pub fn dark_bone() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::Plaster)
        .with_triplanar_world()
        .with_texture_scale(0.3)
        .with_tint(0.5, 0.43, 0.33)
}

/// A dragon's wing membrane: dried, torn hide, dark and leathery, a little light through it.
pub fn membrane() -> StandardMaterial {
    StandardMaterial::color(0.13, 0.115, 0.095)
        .with_roughness(0.72)
        .with_subsurface(0.3)
}

/// Cracked floor stone heaved up where ribs break out.
pub fn rubble() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::Rock)
        .with_triplanar_world()
        .with_texture_scale(0.6)
        .with_tint(0.7, 0.66, 0.6)
}

// ------------------------------------------------------------------------------- paths

fn cubic(a: Vec3, b: Vec3, c: Vec3, d: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u * u) + b * (3.0 * u * u * t) + c * (3.0 * u * t * t) + d * (t * t * t)
}

/// A prison's rib round a body standing at the origin: it breaks out of the floor `base_r` out at
/// `angle` (radians, from +X toward +Z), bows out to `bulge_r` as it climbs, then curls in over
/// the head to its tip `tip_r` from the axis at `height`, twisting `twist` radians round the body
/// on the way, as a finger of a hand closing.
pub fn prison_rib(
    angle: f32,
    base_r: f32,
    bulge_r: f32,
    height: f32,
    tip_r: f32,
    twist: f32,
) -> Vec<Vec3> {
    // In the rib's own plane: (out from the axis, up).
    let p0 = Vec2::new(base_r, -UNDER_M - 0.1);
    let p1 = Vec2::new(bulge_r * 1.18, height * 0.42);
    let p2 = Vec2::new(bulge_r * 0.95, height * 1.08);
    let p3 = Vec2::new(tip_r, height);
    let n = 44;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let q = cubic(
                p0.extend(0.0),
                p1.extend(0.0),
                p2.extend(0.0),
                p3.extend(0.0),
                t,
            );
            let a = angle + twist * t * t;
            Vec3::new(a.cos() * q.x, q.y, a.sin() * q.x)
        })
        .collect()
}

/// A ribcage wall's rib at `u` along the wall: it breaks out of the floor `half_w` to `side`
/// (+1 or -1) of the line, rises and arches over to meet its partner from the other side on
/// the spine `height` up, its tip just past the line. `along` and `across` are the wall's
/// level axes.
pub fn wall_rib(
    along: Vec3,
    across: Vec3,
    u: f32,
    side: f32,
    half_w: f32,
    height: f32,
    lean: f32,
) -> Vec<Vec3> {
    let at = |x: f32, y: f32, du: f32| along * (u + du) + across * (side * x) + Vec3::Y * y;
    let p0 = at(half_w, -UNDER_M - 0.1, 0.0);
    let p1 = at(half_w * 1.22, height * 0.62, lean * 0.4);
    let p2 = at(half_w * 0.55, height * 1.14, lean * 0.8);
    let p3 = at(-0.14, height * 1.02, lean);
    let n = 40;
    (0..=n)
        .map(|i| cubic(p0, p1, p2, p3, i as f32 / n as f32))
        .collect()
}

/// A ribcage wall's spine: up out of the floor past one end of the wall, along its ridge
/// `height` up, and down into the floor past the other end, so it slides along the top as it
/// grows.
pub fn spine(along: Vec3, half: f32, height: f32) -> Vec<Vec3> {
    let end = half + 0.5;
    let mut pts = Vec::new();
    let n = 10;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = (t * PI * 0.5).sin();
        pts.push(along * (-end + 0.5 * a) + Vec3::Y * (-UNDER_M - 0.2 + (height + 0.2) * a));
    }
    let m = ((2.0 * half) / 0.1).ceil() as usize;
    for i in 1..m {
        let u = -half + 2.0 * half * i as f32 / m as f32;
        // The ridge sags a little between its ends.
        let sag = 0.08 * (PI * (u + half) / (2.0 * half)).sin();
        pts.push(along * u + Vec3::Y * (height - sag));
    }
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = (t * PI * 0.5).cos();
        pts.push(along * (end - 0.5 * a) + Vec3::Y * (-UNDER_M - 0.2 + (height + 0.2) * a));
    }
    pts
}

/// A bone spike thrust up out of the floor at `base`: `length` long, leaning toward `lean`
/// (level) and hooked a little at its tip.
pub fn spike(base: Vec3, length: f32, lean: Vec3, seed: u32) -> Vec<Vec3> {
    let lean = Vec3::new(lean.x, 0.0, lean.z).normalize_or(Vec3::X);
    let tilt = 0.25 + 0.3 * hash2(5, 6, seed);
    let n = 16;
    (0..=n)
        .map(|i| {
            let s = i as f32 / n as f32;
            let a = tilt + 0.5 * s * s;
            base + (Vec3::Y * a.cos() + lean * a.sin()) * (s * (length + UNDER_M))
                - Vec3::Y * UNDER_M
        })
        .collect()
}

// ------------------------------------------------------------------------------- the wing

/// A right wing's arm, at size 1, in the arm's frame (span along +X, back along +Z, up +Y): the
/// wrist where the hand's fingers spread.
pub const WRIST: Vec3 = Vec3::new(1.3, 0.04, -0.05);
/// The inner membrane's trailing corners: where the hand's last panel meets it, and its root on
/// the body's flank toward the hip.
pub const INNER_TIP: Vec3 = Vec3::new(1.05, -0.02, 0.95);
pub const INNER_ROOT: Vec3 = Vec3::new(0.08, -0.06, 1.3);
/// A right hand's four fingers, in the hand's frame (its origin the wrist): each one's tip.
pub const FINGERS: [Vec3; 4] = [
    Vec3::new(1.98, 0.12, -0.32),
    Vec3::new(1.58, 0.0, 0.9),
    Vec3::new(0.78, -0.04, 1.42),
    Vec3::new(0.1, -0.05, 1.15),
];

/// A panel of membrane stretched from `o` between `a` and `b`, its trailing edge (from `a` to
/// `b`) scalloped in by `scallop` and sagging, torn through in places: `torn` is the share of it
/// gone near its edge (less near its root). Both faces.
fn panel(m: &mut Mesh, o: Vec3, a: Vec3, b: Vec3, scallop: f32, torn: f32, seed: u32) {
    let (nu, nv) = (8usize, 6usize);
    let n = (a - o).cross(b - o).normalize_or(Vec3::Y);
    let at = |iu: usize, iv: usize| {
        let u = iu as f32 / nu as f32;
        let v = iv as f32 / nv as f32;
        let edge = a.lerp(b, v);
        let pull = 1.0 - scallop * (PI * v).sin() * u * u * u;
        // The hide sags between its bones in the wind.
        let sag = -0.07 * (PI * v).sin() * (PI * u).sin() * a.distance(b).min(1.5);
        o + (edge - o) * (u * pull) + n * sag
    };
    // Thin hide between the bones lets more light through; it is darker and thicker along them.
    let shade = |iu: usize, iv: usize| {
        let u = iu as f32 / nu as f32;
        let v = iv as f32 / nv as f32;
        let s = 0.62 + 0.5 * (PI * v).sin() * (0.35 + 0.65 * u);
        Vec4::new(s, s * 0.95, s * 0.88, 1.0)
    };
    for iu in 0..nu {
        for iv in 0..nv {
            let u = (iu as f32 + 0.5) / nu as f32;
            // Rents and holes, most toward the trailing edge, none by the bones.
            let near_bone = iv == 0 || iv == nv - 1;
            let p = torn * (0.15 + 1.1 * u * u * u);
            let h = hash2(iu as i32 * 7 + 3, iv as i32 * 13 + 5, seed);
            if !near_bone && iu > 1 && h < p {
                continue;
            }
            let (p00, p10, p01, p11) = (
                (at(iu, iv), shade(iu, iv)),
                (at(iu + 1, iv), shade(iu + 1, iv)),
                (at(iu, iv + 1), shade(iu, iv + 1)),
                (at(iu + 1, iv + 1), shade(iu + 1, iv + 1)),
            );
            // The ragged edge: the outer row's cells come off short now and then.
            let (p10, p11) = if iu == nu - 1 && hash2(iv as i32, 41, seed) < 0.45 {
                let s = 0.35 + 0.4 * hash2(iv as i32, 43, seed);
                ((p00.0.lerp(p10.0, s), p10.1), (p01.0.lerp(p11.0, s), p11.1))
            } else {
                (p10, p11)
            };
            for tri in [[p00, p10, p11], [p00, p11, p01]] {
                let fn_ = (tri[1].0 - tri[0].0)
                    .cross(tri[2].0 - tri[0].0)
                    .normalize_or(n);
                for (t, nn) in [(tri, fn_), ([tri[0], tri[2], tri[1]], -fn_)] {
                    let base = m.vertices.len() as u32;
                    for (p, c) in t {
                        m.vertices
                            .push(Vertex::new(p, nn, Vec2::new(p.x, p.z)).with_color(c));
                    }
                    m.indices.extend([base, base + 1, base + 2]);
                }
            }
        }
    }
}

fn mirror(mut m: Mesh, side: f32) -> Mesh {
    if side < 0.0 {
        for v in m.vertices.iter_mut() {
            v.position[0] = -v.position[0];
            v.normal[0] = -v.normal[0];
        }
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
    m
}

/// The membrane between a wing's arm and its body (the arm's frame, at size 1; `side` -1 for the
/// left wing).
pub fn arm_membrane(side: f32, seed: u32) -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let root = Vec3::new(0.02, 0.0, 0.04);
    panel(&mut m, root, WRIST, INNER_TIP, 0.0, 0.0, seed);
    panel(&mut m, root, INNER_TIP, INNER_ROOT, 0.16, 0.3, seed ^ 0x51);
    mirror(m, side)
}

/// The membrane between a wing's fingers (the hand's frame, at size 1; `side` -1 for the left
/// wing), torn through toward its trailing edge.
pub fn hand_membrane(side: f32, seed: u32) -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let o = Vec3::ZERO;
    for (i, w) in FINGERS.windows(2).enumerate() {
        panel(
            &mut m,
            o,
            w[0],
            w[1],
            0.22,
            0.32 + 0.08 * i as f32,
            seed ^ (i as u32 * 0x9e37),
        );
    }
    // The last finger back to the inner membrane's tip.
    panel(
        &mut m,
        o,
        FINGERS[3],
        INNER_TIP - WRIST,
        0.12,
        0.25,
        seed ^ 0x77,
    );
    mirror(m, side)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prison_rib_breaks_out_beside_the_body_and_closes_over_its_head() {
        let r = prison_rib(0.0, 0.7, 0.8, 2.2, 0.1, 0.3);
        let first = r[0];
        let last = *r.last().unwrap();
        assert!(first.y < 0.0, "it starts under the floor");
        assert!((first.length() - 0.7).abs() < 0.05, "{first}");
        assert!((last.y - 2.2).abs() < 1e-3);
        assert!(
            Vec3::new(last.x, 0.0, last.z).length() < 0.15,
            "tip over the head"
        );
        // It bows out past its base as it climbs.
        let widest = r
            .iter()
            .map(|p| Vec3::new(p.x, 0.0, p.z).length())
            .fold(0.0, f32::max);
        assert!(widest > 0.8, "{widest}");
    }

    #[test]
    fn a_wall_rib_arches_from_its_side_over_the_line() {
        let r = wall_rib(Vec3::X, Vec3::Z, 0.5, 1.0, 1.0, 2.2, 0.0);
        assert!(r[0].z > 0.9 && r[0].y < 0.0);
        let last = *r.last().unwrap();
        assert!(last.z < 0.0 && last.z > -0.3, "{last}");
        let top = r.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!(top > 2.2 && top < 2.6, "{top}");
        assert!(r.iter().all(|p| (p.x - 0.5).abs() < 1e-4));
    }

    #[test]
    fn the_membranes_are_torn_but_whole_at_their_bones() {
        let full = {
            let mut m = Mesh::new(Vec::new(), Vec::new());
            panel(&mut m, Vec3::ZERO, Vec3::X, Vec3::Z, 0.2, 0.0, 1);
            m.indices.len()
        };
        let hand = hand_membrane(1.0, 7);
        assert!(!hand.indices.is_empty());
        let mut torn = Mesh::new(Vec::new(), Vec::new());
        panel(&mut torn, Vec3::ZERO, Vec3::X, Vec3::Z, 0.2, 0.5, 1);
        assert!(torn.indices.len() < full, "holes in it");
        assert!(torn.indices.len() > full / 2, "but most of it there");
        // The left wing mirrors the right.
        let left = hand_membrane(-1.0, 7);
        assert_eq!(left.vertices.len(), hand.vertices.len());
        assert!((left.vertices[5].position[0] + hand.vertices[5].position[0]).abs() < 1e-5);
    }
}
