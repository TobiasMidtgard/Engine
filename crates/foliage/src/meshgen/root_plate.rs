//! The root plate of a tree the wind threw over roots and all: the disc of soil its roots held,
//! tipped up on edge at the butt of the fallen trunk, raw earth and torn roots on the side away
//! from the trunk, the old forest floor (humus, moss) on the side the trunk grows from.
//!
//! Real plates of temperate trees are shallow and wide: 0.5-1 m deep and 2-5 m across, several
//! times the trunk's diameter, their rims ragged where roots tore out of the ground (Schaetzl et
//! al. 1989, "Tree uprooting: review of impacts on forest ecology", Vegetatio 79; Nicoll & Ray
//! 1996, "Adaptive growth of tree root systems in response to wind action and site
//! conditions").
//!
//! The mesh is a plate of radius about 1 m (scale it to the tree): its axis is +Y, the trunk's
//! direction, so a log's own frame places it. The forest-floor face is at +Y, the raw earth
//! bulges out toward −Y, roots stick out of the earth face and the rim.

use engine_core::math::{Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::{MeshBuilder, rgb};

/// Points around the rim.
const AROUND: usize = 64;
/// Rings from the center out (relative radius).
const RINGS: [f32; 6] = [0.0, 0.3, 0.55, 0.75, 0.9, 1.0];

/// Forest floor on the plate's top: dark humus, mossy in patches.
const HUMUS: [f32; 3] = [0.075, 0.055, 0.035];
const MOSS: [f32; 3] = [0.06, 0.085, 0.03];
/// Raw earth on the torn side: brown mineral soil, darker in clods, paler where sandy.
const EARTH: [f32; 3] = [0.1, 0.068, 0.042];
const CLOD: [f32; 3] = [0.055, 0.04, 0.028];
const CLAY: [f32; 3] = [0.17, 0.12, 0.07];
const STONE: [f32; 3] = [0.2, 0.19, 0.17];
/// Torn roots: pale wood where the bark scraped off, grey-brown where it stayed.
const ROOT: [f32; 3] = [0.16, 0.115, 0.075];
const ROOT_BARK: [f32; 3] = [0.06, 0.045, 0.034];

/// A root plate about 1 m in radius, different for every `seed`.
pub fn root_plate(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed ^ 0x700f_91a7);
    let mut b = MeshBuilder::new();
    // Ragged rim: a few lobes where big roots tore out further.
    let phase: [f32; 3] = std::array::from_fn(|_| rng.range(0.0, std::f32::consts::TAU));
    let jag: Vec<f32> = (0..AROUND).map(|_| rng.range(-0.13, 0.13)).collect();
    let rim = |a: f32| {
        let k = ((a / std::f32::consts::TAU * AROUND as f32).round() as usize) % AROUND;
        1.0 + 0.16 * (2.0 * a + phase[0]).sin()
            + 0.1 * (5.0 * a + phase[1]).sin()
            + 0.06 * (11.0 * a + phase[2]).sin()
            + jag[k]
    };
    let bump = |x: f32, z: f32, salt: f32| {
        (x * 7.1 + salt).sin() * (z * 6.3 - salt * 0.7).cos() * 0.5
            + (x * 13.0 - z * 11.0 + salt * 2.0).sin() * 0.25
    };
    let angle = |k: usize| k as f32 / AROUND as f32 * std::f32::consts::TAU;
    // The forest floor face (+Y): nearly flat, its edge rolled over a little.
    let top_y = |rho: f32, x: f32, z: f32| 0.13 - 0.06 * rho * rho + 0.02 * bump(x, z, 1.0);
    // The raw earth face (−Y): bulging out in the middle where the roots were densest.
    let bottom_y =
        |rho: f32, x: f32, z: f32| -0.12 - 0.26 * (1.0 - rho * rho) + 0.08 * bump(x, z, 4.0);
    let face = |b: &mut MeshBuilder, top: bool, rng: &mut Rng| {
        let first = b.vertices.len() as u32;
        let ny = if top { 1.0 } else { -1.0 };
        for (ri, &rho) in RINGS.iter().enumerate() {
            let count = if ri == 0 { 1 } else { AROUND };
            for k in 0..count {
                let a = angle(k);
                let r = rho * rim(a);
                let (x, z) = (r * a.cos(), r * a.sin());
                let clump = if top || ri == 0 {
                    0.0
                } else {
                    rng.range(-0.03, 0.03)
                };
                let y = if top {
                    top_y(rho, x, z)
                } else {
                    bottom_y(rho, x, z) + clump
                };
                // Tilted outward toward the rim, so the plate's edge rounds off in the light;
                // the torn earth lumpy.
                let tilt = if top { 0.0 } else { 0.5 };
                let n = Vec3::new(
                    x * 0.35 * rho + rng.range(-tilt, tilt),
                    ny,
                    z * 0.35 * rho + rng.range(-tilt, tilt),
                );
                let color = if top {
                    let m = (0.5 + 0.5 * bump(x * 0.6, z * 0.6, 7.0)).clamp(0.0, 1.0);
                    rgb(HUMUS, 1.0).lerp(rgb(MOSS, 1.0), m * m)
                } else {
                    let c = (0.5 + bump(x * 1.4, z * 1.4, 2.0)).clamp(0.0, 1.0);
                    let earth = rgb(EARTH, rng.range(0.85, 1.12)).lerp(rgb(CLOD, 1.0), c * 0.7);
                    // Paler subsoil clay and stones the roots lifted out with it.
                    let clay = (bump(x * 2.3, z * 2.1, 5.0) - 0.25).clamp(0.0, 0.5) * 2.0;
                    let stone = rng.chance(0.06);
                    if stone {
                        rgb(STONE, rng.range(0.8, 1.2))
                    } else {
                        earth.lerp(rgb(CLAY, 1.0), clay * 0.8)
                    }
                };
                b.vertex(Vec3::new(x, y, z), n, Vec2::new(x, z), color);
            }
        }
        // Fan to the first ring, then quads between rings. Seen from +Y, angles run
        // counter-clockwise from +X toward +Z, which is clockwise: wind each triangle so its
        // normal points out of its face.
        let ring = |ri: usize, k: usize| -> u32 {
            if ri == 0 {
                first
            } else {
                first + 1 + ((ri - 1) * AROUND + k % AROUND) as u32
            }
        };
        let tri = |b: &mut MeshBuilder, p: u32, q: u32, r: u32| {
            if top {
                b.tri(p, r, q);
            } else {
                b.tri(p, q, r);
            }
        };
        for k in 0..AROUND {
            tri(b, ring(0, 0), ring(1, k), ring(1, k + 1));
        }
        for ri in 1..RINGS.len() - 1 {
            for k in 0..AROUND {
                let (p, q) = (ring(ri, k), ring(ri, k + 1));
                let (r, s) = (ring(ri + 1, k), ring(ri + 1, k + 1));
                tri(b, p, r, s);
                tri(b, p, s, q);
            }
        }
    };
    face(&mut b, true, &mut rng);
    face(&mut b, false, &mut rng);
    // The edge between them: earth, bulging out a little at mid-depth.
    let first = b.vertices.len() as u32;
    for k in 0..=AROUND {
        let a = angle(k);
        let r = rim(a);
        let (c, s) = (a.cos(), a.sin());
        for (y_of, out, t) in [
            (top_y(1.0, r * c, r * s), 1.0, 0.0),
            (
                0.5 * (top_y(1.0, r * c, r * s) + bottom_y(1.0, r * c, r * s)),
                1.06,
                0.5,
            ),
            (bottom_y(1.0, r * c, r * s), 1.0, 1.0),
        ] {
            let p = Vec3::new(r * out * c, y_of, r * out * s);
            let color = rgb(HUMUS, 1.0).lerp(rgb(EARTH, 0.9), t);
            b.vertex(p, Vec3::new(c, 0.0, s), Vec2::new(a, y_of), color);
        }
    }
    for k in 0..AROUND as u32 {
        for j in 0..2u32 {
            let p = first + k * 3 + j;
            let (q, r, s) = (p + 1, p + 3, p + 4);
            b.tri(p, r, s);
            b.tri(p, s, q);
        }
    }
    // Torn roots: out of the earth face, curving down and out, and out through the rim.
    let root_color = |_: usize, around: f32| {
        let bark = (around * 3.0).fract() < 0.55;
        rgb(if bark { ROOT_BARK } else { ROOT }, 1.0)
    };
    let tip_color = |_: f32| rgb(ROOT, 1.15);
    for i in 0..110 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let (c, s) = (a.cos(), a.sin());
        let r0;
        let path: Vec<Vec3> = if i < 40 {
            // Out of the earth face: where a sinker or a big lateral snapped off.
            let rho = rng.range(0.15, 0.85);
            let start = Vec3::new(rho * c, bottom_y(rho, rho * c, rho * s) + 0.04, rho * s);
            let reach = rng.range(0.15, 0.55);
            let out = rng.range(0.1, 0.6);
            r0 = rng.range(0.004, 0.016);
            (0..5)
                .map(|j| {
                    let t = j as f32 / 4.0;
                    start
                        + Vec3::new(c * out * t, -reach * t, s * out * t)
                        + Vec3::new(0.0, 0.04 * (t * 3.0).sin(), 0.0)
                })
                .collect()
        } else {
            // Out through the rim: the long laterals, torn off a way out, drooping.
            let r = rim(a);
            let y = rng.range(-0.25, 0.1);
            // Mostly short torn stubs and fine roots, a few long thick laterals.
            let long = rng.chance(0.2);
            let reach = if long {
                rng.range(0.35, 0.7)
            } else {
                rng.range(0.08, 0.35)
            };
            let droop = rng.range(0.0, 0.5) * reach;
            let wander = rng.range(-0.4, 0.4) * reach;
            let kink = rng.range(-0.3, 0.3) * reach;
            // Thin for a unit plate: it is scaled up to 3 m in radius.
            r0 = if long {
                rng.range(0.012, 0.024)
            } else {
                rng.range(0.003, 0.01)
            };
            (0..5)
                .map(|j| {
                    let t = j as f32 / 4.0;
                    let d = r * 0.8 + reach * t;
                    let side = Vec3::new(-s, 0.0, c) * (wander * t * t);
                    let bend = Vec3::Y * (kink * (t * 6.0).sin() * t);
                    Vec3::new(c * d, y - droop * t * t, s * d) + side + bend
                })
                .collect()
        };
        let radius: Vec<f32> = (0..5).map(|j| r0 * (1.0 - 0.45 * j as f32 / 4.0)).collect();
        b.tube(&path, &radius, 5, 1.0, &root_color, None, Some(&tip_color));
    }
    b.finish()
}

/// The plate's radius and depth for a trunk of `trunk_radius_m`: about nine trunk radii (a
/// 40 cm trunk tears up a plate some 3.5 m across, Schaetzl et al. 1989), shallow (meters).
pub fn plate_size(trunk_radius_m: f32) -> (f32, f32) {
    let radius = (9.0 * trunk_radius_m).clamp(0.8, 3.0);
    (radius, radius * 0.3)
}

/// Vertex color as linear RGB with alpha 1 (for tests).
#[cfg(test)]
fn mean_color(m: &Mesh) -> engine_core::math::Vec4 {
    use engine_core::math::Vec4;
    let n = m.vertices.len().max(1) as f32;
    m.vertices
        .iter()
        .map(|v| Vec4::from(v.color))
        .fold(Vec4::ZERO, |a, c| a + c / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_plate_is_a_wide_shallow_disc_with_roots_and_faces_outward() {
        let m = root_plate(3);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &m.vertices {
            let p = Vec3::from(v.position);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let across = (hi.x - lo.x).max(hi.z - lo.z);
        // The roots reach past the earth (−Y): deeper than the plate itself.
        assert!((1.8..4.6).contains(&across), "{across} m across");
        assert!(hi.y < 0.2 && lo.y < -0.45, "{lo} .. {hi}");
        assert!(m.vertices.len() > 300, "{} vertices", m.vertices.len());
        // Front faces wind counter-clockwise (as the renderer culls): normals out.
        let mut inward = 0;
        for tri in m.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let pa = Vec3::from(a.position);
            let geo = (Vec3::from(b.position) - pa).cross(Vec3::from(c.position) - pa);
            if geo.length() < 1e-8 {
                continue;
            }
            let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            if geo.dot(n) <= 0.0 {
                inward += 1;
            }
        }
        assert_eq!(inward, 0, "inward-facing triangles");
        // Earth brown, not black or bright.
        let c = mean_color(&m);
        assert!(c.x > c.z && c.x < 0.3, "{c}");
        assert_ne!(root_plate(3).vertices.len(), 0);
        assert_ne!(
            root_plate(3).vertices[5].position,
            root_plate(4).vertices[5].position
        );
    }

    #[test]
    fn a_plate_is_several_trunks_wide() {
        let (r, depth) = plate_size(0.25);
        assert!((1.8..2.6).contains(&r) && depth < r * 0.5);
        assert_eq!(plate_size(0.01).0, 0.8);
    }
}
