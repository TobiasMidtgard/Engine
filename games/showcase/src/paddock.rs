//! The paddock's horse track (playtest 1 #24, "the paddock oval's missing parts"): Animation's
//! demo marks the oval with bare white posts, which read as a fence with its rails gone. This
//! adds the rails, two white boards between each pair of posts round both sides of the lane, as
//! a riding arena's post-and-rail fence, and a raked, damper band of sand along the lane so the
//! oval reads as a track rather than posts standing in a sand field.
//!
//! The posts are Animation's (crates/anim/src/demo.rs, "Horse track"): its lane waypoints
//! scaled about the oval's middle by [`RINGS`], one post at each waypoint and one halfway to
//! the next. [`LANE`] and [`MIDDLE`] copy that lane; if Animation moves it, move these too.

use engine::prelude::*;
use engine::render::Vertex;

use crate::scenery::Lod;

/// The horse's lane round the oval (x, z, meters from the paddock's origin), from Animation's
/// demo.
const LANE: [[f32; 2]; 10] = [
    [16.0, 7.0],
    [9.0, 5.6],
    [5.4, 0.0],
    [7.4, -5.6],
    [13.0, -8.0],
    [20.0, -8.0],
    [26.0, -5.0],
    [27.6, 0.5],
    [25.0, 5.5],
    [21.0, 7.4],
];
/// The oval's middle, which the post rings are scaled about.
const MIDDLE: [f32; 2] = [16.5, -0.3];
/// The outer and inner post rings, as scales of the lane about [`MIDDLE`].
const RINGS: [f32; 2] = [1.32, 0.7];
/// The rails' heights (middle of each board, m above the ground; the posts are 1 m tall) and
/// their section (thickness across, height), m.
const RAILS: [f32; 2] = [0.45, 0.85];
const BOARD: [f32; 2] = [0.045, 0.1];
/// The raked lane: how wide (m) and how far above the sand (m, enough not to flicker).
const BAND_W: f32 = 3.2;
const BAND_LIFT: f32 = 0.015;
/// Past this the band and the rails are too thin to see.
const HIDE_M: f32 = 260.0;

/// Where Animation's posts stand round one ring, in order round the oval.
fn posts(scale: f32) -> Vec<Vec2> {
    let mid = Vec2::from(MIDDLE);
    let mut out = Vec::with_capacity(LANE.len() * 2);
    for (k, w) in LANE.iter().enumerate() {
        let (a, b) = (Vec2::from(*w), Vec2::from(LANE[(k + 1) % LANE.len()]));
        for along in [0.0, 0.5] {
            out.push(mid + (a.lerp(b, along) - mid) * scale);
        }
    }
    out
}

/// A box `len` long (local X), `size` across (Z) and tall (Y), from `a` to `b` at height `y`.
fn board(mesh: &mut Mesh, a: Vec2, b: Vec2, y: f32, size: [f32; 2]) {
    let d = b - a;
    let len = d.length();
    let mut m = engine::render::mesh::cuboid(Vec3::new(len, size[1], size[0]));
    let mid = (a + b) * 0.5;
    let yaw = -d.y.atan2(d.x);
    m.transform(&Mat4::from_rotation_translation(
        Quat::from_rotation_y(yaw),
        Vec3::new(mid.x, y, mid.y),
    ));
    mesh.merge(&m);
}

/// The rails of both rings, one mesh.
fn rails() -> Mesh {
    let mut mesh = Mesh::default();
    for scale in RINGS {
        let p = posts(scale);
        for k in 0..p.len() {
            let (a, b) = (p[k], p[(k + 1) % p.len()]);
            for y in RAILS {
                board(&mut mesh, a, b, y, BOARD);
            }
        }
    }
    mesh
}

/// The raked band along the lane: a closed ribbon over the sand, darker and rougher in the
/// middle where the hooves churn it, fading to the sand at its edges (vertex alpha is not
/// blended, so the fade is in the color: the edges match the arena sand).
fn band() -> Mesh {
    // Sample the lane as a smooth closed curve (Catmull-Rom through the waypoints).
    let n = LANE.len();
    let at = |i: isize| Vec2::from(LANE[i.rem_euclid(n as isize) as usize]);
    let mut curve = Vec::new();
    for i in 0..n as isize {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        for s in 0..8 {
            let t = s as f32 / 8.0;
            let (t2, t3) = (t * t, t * t * t);
            curve.push(
                0.5 * ((2.0 * p1)
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    let m = curve.len();
    // Across the band: edge, churned middle, edge.
    let across = [-0.5f32, -0.3, 0.0, 0.3, 0.5];
    let churn = |u: f32| 1.0 - (u.abs() * 2.0).powi(2);
    let mut mesh = Mesh::default();
    for (k, p) in curve.iter().enumerate() {
        let d = (curve[(k + 1) % m] - curve[(k + m - 1) % m]).normalize_or(Vec2::X);
        let side = Vec2::new(-d.y, d.x);
        for u in across {
            let q = *p + side * (u * BAND_W);
            let c = churn(u).max(0.0);
            // The arena sand times 1 at the edges; damp, darker, cooler sand in the lane.
            let tint = Vec3::ONE.lerp(Vec3::new(0.62, 0.55, 0.5), c);
            mesh.vertices.push(
                Vertex::new(
                    Vec3::new(q.x, BAND_LIFT, q.y),
                    Vec3::Y,
                    Vec2::new(q.x, q.y) * 0.25,
                )
                .with_color(tint.extend(1.0)),
            );
        }
    }
    let w = across.len() as u32;
    for k in 0..m as u32 {
        let next = (k + 1) % m as u32;
        for j in 0..w - 1 {
            let (a, b) = (k * w + j, k * w + j + 1);
            let (c, d) = (next * w + j, next * w + j + 1);
            mesh.indices.extend([a, c, b, b, c, d]);
        }
    }
    // Face up whichever way round the lane runs.
    let p = |i: u32| Vec3::from(mesh.vertices[mesh.indices[i as usize] as usize].position);
    if (p(1) - p(0)).cross(p(2) - p(0)).y < 0.0 {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    mesh
}

/// Spawn the rails and the lane band at the paddock's `origin` (its pad's middle).
pub fn spawn(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| {
        let (rails, band) = {
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            (
                meshes.get_or_insert_keyed("showcase/paddock/rails", rails),
                meshes.get_or_insert_keyed("showcase/paddock/lane", band),
            )
        };
        let (paint, sand) = {
            let mut mats = world.resource_mut::<Assets<StandardMaterial>>();
            (
                // Animation's post paint, so posts and rails are one fence.
                mats.get_or_insert_keyed("showcase/paddock/paint", || {
                    StandardMaterial::hex(0xe8e4dc).with_roughness(0.6)
                }),
                // The arena's sand (its color times the band's vertex colors), matte.
                mats.get_or_insert_keyed("showcase/paddock/sand", || {
                    StandardMaterial::hex(0xc8a06a).with_roughness(0.97)
                }),
            )
        };
        let at = Transform::from_translation(origin);
        world.spawn((
            Name::new("Horse track rails"),
            at,
            MeshRenderer::new(rails, paint),
            Lod::hide_beyond(rails, HIDE_M),
        ));
        world.spawn((
            Name::new("Horse track lane"),
            at,
            MeshRenderer::new(band, sand).without_shadows(),
            Lod::hide_beyond(band, HIDE_M * 0.6),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rails_join_every_post_and_the_lane_runs_between_them() {
        for scale in RINGS {
            assert_eq!(posts(scale).len(), 20);
        }
        let r = rails();
        // Two boards between each of 20 post pairs on two rings, 12 triangles a board.
        assert_eq!(r.indices.len() / 3, 2 * 20 * 2 * 12);
        // At every waypoint the raked band clears both rings of rails.
        let mid = Vec2::from(MIDDLE);
        for w in LANE {
            let d = (Vec2::from(w) - mid).length();
            let gap = d * (RINGS[0] - 1.0).min(1.0 - RINGS[1]);
            assert!(BAND_W * 0.5 + 0.3 < gap, "{w:?}: {gap}");
        }
        let b = band();
        assert!(!b.indices.is_empty());
        for v in &b.vertices {
            assert!(v.position[1] > 0.0 && v.position[1] < 0.05);
        }
        // Every triangle faces up (seen from above, not culled).
        for t in b.indices.as_chunks::<3>().0 {
            let p = |i: u32| Vec3::from(b.vertices[i as usize].position);
            assert!((p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).y > 0.0);
        }
    }
}
