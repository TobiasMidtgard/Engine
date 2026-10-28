//! The market square in the middle of the village, where the exe starts: granite setts round a
//! marble middle and Particles' carved fountain, market stalls under striped awnings round the
//! square (potter, coppersmith, greengrocer, cloth merchant, baker), a Bavarian maypole with
//! guild signs, a village oak with a round bench, benches, and bollards at the road gaps.
//!
//! Laid out after a real market square (Rothenburg ob der Tauber's Marktplatz: a fountain in the
//! middle, stalls round it, cobbles, half-timbered houses on every side) and the usual level-art
//! rules: one tall focal silhouette (the maypole) the eye finds first, props turned off the
//! grid so nothing lines up on a tangent, a different color for every awning, and props that
//! tell what happens here (wares, crates, barrels) at human scale.
//!
//! Rendering's material checks (test case 47) live on real props: the potter's red-glazed jugs
//! go from glossy to matte left to right and the coppersmith's pots do the same in metal (the
//! `plaza_stalls` stop looks at both).
//!
//! The meshes are built the first time the square loads ([`PlazaMeshes`]), so every stall of a
//! kind shares them.

use crate::scenery::{self, Library, Lod, NearSolid};
use crate::zones::Zone;
use engine::prelude::*;
use engine::render::{MaterialRef, Vertex};
use engine::world::physics::ColliderShape;
use engine_vfx::demo::art::meshes::{self as m, Profile};
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// Top of the paving (m above the zone's center).
const FLOOR: f64 = 0.2;
/// The stalls stand this far from the fountain (m), on the inner band of setts, in front of
/// the corner trees.
const STALL_R: f64 = 19.5;
/// Wares and small props hide beyond this (m, before the showcase's LOD scale).
const WARES_HIDE: f32 = 70.0;
/// Stall frames and awnings hide beyond this (m).
const STALL_HIDE: f32 = 300.0;

/// Stall frame: posts, counter, shelves, the awning's rails (local: front toward +Z).
const STALL_HALF: Vec2 = Vec2::new(1.75, 0.95);
const FRONT_H: f32 = 2.3;
const BACK_H: f32 = 2.75;
/// Top of the counter.
const COUNTER_Y: f32 = 0.945;

/// What a stall sells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trade {
    Potter,
    Coppersmith,
    Greengrocer,
    Cloth,
    Baker,
}

/// The stalls: trade, angle round the square (degrees; −90 is north, toward the plaza stop's
/// view) and awning color. Clear of the four roads (0, ±90, 180) and the corner trees (±45,
/// ±135).
const STALLS: [(Trade, f64, u32); 5] = [
    (Trade::Cloth, -160.0, 0x2b4f86),
    (Trade::Potter, -113.0, 0x9c2a22),
    (Trade::Coppersmith, -67.0, 0x2f5d3a),
    (Trade::Greengrocer, -20.0, 0xc99a2e),
    (Trade::Baker, 160.0, 0xb5562a),
];

/// Copper as it reads on a market stall: warmer and deeper than measured polished copper, which
/// looks pale under an overcast sky.
const COPPER: u32 = 0xeea27a;

/// The maypole: angle (degrees) and distance from the fountain, height (m).
const MAYPOLE: (f64, f64) = (-122.0, 12.0);
const MAYPOLE_H: f32 = 24.0;

/// Meshes built once and shared by every copy (and every load of the square while they last:
/// a cell that releases its assets on unload frees them, and the next load builds them anew).
#[derive(Resource, Default)]
struct PlazaMeshes(HashMap<&'static str, Handle<Mesh>>);

/// A mesh to place: built by `build` the first time `key` is used.
struct Piece {
    key: &'static str,
    build: fn() -> Mesh,
    at: Transform,
    material: MaterialRef,
    hide: f32,
}

#[derive(Default)]
struct Pieces(Vec<Piece>);

impl Pieces {
    fn add(
        &mut self,
        key: &'static str,
        build: fn() -> Mesh,
        at: Transform,
        material: impl Into<MaterialRef>,
        hide: f32,
    ) {
        self.0.push(Piece {
            key,
            build,
            at,
            material: material.into(),
            hide,
        });
    }

    /// Build what isn't built yet and spawn every piece (nothing without a renderer).
    fn spawn(self, commands: &mut Commands) {
        commands.queue(move |world: &mut World| {
            if !world.contains_resource::<Assets<Mesh>>() {
                return;
            }
            world.init_resource::<PlazaMeshes>();
            for p in self.0 {
                // A square that streamed out with its assets released built its meshes again.
                let known = world
                    .resource::<PlazaMeshes>()
                    .0
                    .get(p.key)
                    .copied()
                    .filter(|h| world.resource::<Assets<Mesh>>().contains(*h));
                let mesh = known.unwrap_or_else(|| {
                    let h = world.resource_mut::<Assets<Mesh>>().add((p.build)());
                    world.resource_mut::<PlazaMeshes>().0.insert(p.key, h);
                    h
                });
                let mut e = world.spawn((p.at, MeshRenderer::new(mesh, p.material)));
                if p.hide.is_finite() {
                    e.insert(Lod::hide_beyond(mesh, p.hide));
                }
            }
        });
    }
}

/// Linear color from sRGB hex (for vertex colors, which are linear).
fn lin(hex: u32) -> Vec3 {
    let c = |s: u32| ((hex >> s) & 0xff) as f32 / 255.0;
    Vec3::new(c(16), c(8), c(0)).powf(2.2)
}

fn tinted(mut mesh: Mesh, c: Vec3) -> Mesh {
    for v in &mut mesh.vertices {
        v.color = c.extend(1.0).to_array();
    }
    mesh
}

/// A beveled plank from `a` to `b` (grain along it), `w` wide and `t` thick.
fn beam(a: Vec3, b: Vec3, w: f32, t: f32) -> Mesh {
    let d = b - a;
    let len = d.length();
    m::moved(
        m::block(Vec3::new(len, t, w), 0.008),
        (a + b) * 0.5,
        Quat::from_rotation_arc(Vec3::X, d / len),
    )
}

/// A board centered at `at`, `size` (x along the grain).
fn board(size: Vec3, at: Vec3) -> Mesh {
    m::moved(m::block(size, 0.006), at, Quat::IDENTITY)
}

/// A square post standing on `foot`, `h` tall (grain up).
fn post(foot: Vec3, h: f32, s: f32) -> Mesh {
    beam(foot, foot + Vec3::Y * h, s, s)
}

// ------------------------------------------------------------------------------------------
// Stalls

fn stall_frame() -> Mesh {
    let mut f = Mesh::default();
    let (hx, hz) = (STALL_HALF.x, STALL_HALF.y);
    for x in [-hx, hx] {
        f.merge(&post(Vec3::new(x, 0.0, hz), FRONT_H, 0.1));
        f.merge(&post(Vec3::new(x, 0.0, -hz), BACK_H, 0.1));
        // The awning's rail along each side, on the post tops.
        f.merge(&beam(
            Vec3::new(x, BACK_H + 0.045, -hz - 0.1),
            Vec3::new(x, FRONT_H + 0.045, hz + 0.1),
            0.07,
            0.09,
        ));
        // A brace from the back post to the counter's end.
        f.merge(&beam(
            Vec3::new(x, 0.1, -hz + 0.05),
            Vec3::new(x, COUNTER_Y - 0.05, 0.2),
            0.06,
            0.05,
        ));
    }
    for (y, z) in [(FRONT_H - 0.05, hz), (BACK_H - 0.05, -hz)] {
        f.merge(&beam(
            Vec3::new(-hx - 0.06, y, z),
            Vec3::new(hx + 0.06, y, z),
            0.07,
            0.08,
        ));
    }
    // Counter: a thick top on an apron of boards.
    f.merge(&board(
        Vec3::new(3.4, 0.05, 0.8),
        Vec3::new(0.0, COUNTER_Y - 0.025, 0.55),
    ));
    for k in 0..3 {
        f.merge(&board(
            Vec3::new(3.38, 0.19, 0.03),
            Vec3::new(0.0, 0.33 + k as f32 * 0.2, 0.93),
        ));
    }
    // Shelves at the back.
    for y in [0.55, 1.25] {
        f.merge(&board(
            Vec3::new(3.38, 0.04, 0.38),
            Vec3::new(0.0, y, -0.72),
        ));
    }
    f
}

/// The awning's stripes (every other one: `odd` picks which), a canvas sheet sagging between
/// the rails and running past the front, with a scalloped valance.
fn awning(odd: bool) -> Mesh {
    const STRIPES: usize = 8;
    const HALF_W: f32 = 1.95;
    let (z0, z1) = (-STALL_HALF.y - 0.12, STALL_HALF.y + 0.38);
    let y_at = |z: f32, x: f32| {
        let s = (z - z0) / (z1 - z0);
        let slope = (BACK_H - FRONT_H) / (2.0 * STALL_HALF.y);
        let rails = (x / STALL_HALF.x).clamp(-1.0, 1.0);
        BACK_H + 0.1 - slope * (z - z0) - 0.05 * (PI * s).sin() - 0.035 * (1.0 - rails * rails)
    };
    let w = 2.0 * HALF_W / STRIPES as f32;
    let mut mesh = Mesh::default();
    for k in (0..STRIPES).filter(|k| (k % 2 == 1) == odd) {
        let xs: Vec<f32> = (0..=4)
            .map(|i| -HALF_W + (k as f32 + i as f32 / 4.0) * w)
            .collect();
        // Sheet: rows from the back to the front (front faces up).
        let rows: Vec<Vec<Vec3>> = (0..=10)
            .map(|j| {
                let z = z0 + (z1 - z0) * j as f32 / 10.0;
                xs.iter().map(|&x| Vec3::new(x, y_at(z, x), z)).collect()
            })
            .collect();
        mesh.merge(&m::grid(&rows, false));
        // Valance: down from the front edge to a scallop (front faces out).
        let cols: Vec<f32> = (0..=8)
            .map(|i| -HALF_W + (k as f32 + i as f32 / 8.0) * w)
            .collect();
        let top: Vec<Vec3> = cols
            .iter()
            .map(|&x| Vec3::new(x, y_at(z1, x), z1))
            .collect();
        let bottom: Vec<Vec3> = cols
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let s = i as f32 / 8.0;
                Vec3::new(x, y_at(z1, x) - 0.17 - 0.09 * (PI * s).sin(), z1 + 0.01)
            })
            .collect();
        mesh.merge(&m::grid(&[top, bottom], false));
    }
    m::two_sided(mesh)
}

/// A glazed jug with a handle, 0.37 m tall.
fn jug() -> Mesh {
    let p = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.065, 0.0)
        .to(0.1, 0.04)
        .to(0.125, 0.12)
        .to(0.118, 0.2)
        .to(0.085, 0.265)
        .to(0.046, 0.305)
        .corner(0.052, 0.365)
        .corner(0.042, 0.37)
        .to(0.036, 0.32);
    let mut mesh = m::revolve(&p, 28);
    let handle: Vec<Vec3> = (0..=10)
        .map(|i| {
            let a = PI * (0.1 + 0.85 * i as f32 / 10.0);
            Vec3::new(0.07 + 0.075 * a.sin(), 0.215 + 0.1 * a.cos(), 0.0)
        })
        .collect();
    mesh.merge(&m::tube(&handle, |_| 0.013, 8, true));
    mesh
}

/// A copper cooking pot with a rolled rim and two loop handles, 0.19 m tall.
fn pot() -> Mesh {
    let p = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.12, 0.0)
        .to(0.14, 0.025)
        .to(0.145, 0.15)
        .to(0.152, 0.18)
        .corner(0.16, 0.19)
        .corner(0.148, 0.192)
        .to(0.138, 0.17)
        .to(0.136, 0.02)
        .corner(0.0, 0.015);
    let mut mesh = m::revolve(&p, 32);
    for side in [-1.0f32, 1.0] {
        let path: Vec<Vec3> = (0..=8)
            .map(|i| {
                let a = PI * i as f32 / 8.0;
                Vec3::new(side * (0.145 + 0.035 * a.sin()), 0.15, 0.04 * a.cos())
            })
            .collect();
        mesh.merge(&m::tube(&path, |_| 0.008, 6, true));
    }
    mesh
}

/// A copper pan with a long handle, hanging face out (local: the pan in the XY plane).
fn pan() -> Mesh {
    let p = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.12, 0.0)
        .to(0.14, 0.02)
        .corner(0.15, 0.045)
        .corner(0.142, 0.047)
        .to(0.13, 0.02)
        .corner(0.0, 0.012);
    let mut mesh = m::revolve(&p, 28);
    mesh.merge(&beam(
        Vec3::new(0.14, 0.035, 0.0),
        Vec3::new(0.42, 0.06, 0.0),
        0.025,
        0.01,
    ));
    // Turned to face +Z with the handle up, then lowered so it hangs from the handle's end.
    m::moved(
        mesh,
        Vec3::Y * -0.42,
        Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_x(FRAC_PI_2),
    )
}

/// The potter's shelves: unglazed bowls and stacked plates.
fn earthenware() -> Mesh {
    let bowl = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.05, 0.0)
        .to(0.1, 0.04)
        .corner(0.125, 0.075)
        .corner(0.115, 0.078)
        .to(0.09, 0.04)
        .corner(0.0, 0.022);
    let bowl = m::revolve(&bowl, 24);
    let mut mesh = Mesh::default();
    for (i, x) in [-1.35f32, -0.9, -0.45, 0.4, 0.85, 1.3]
        .into_iter()
        .enumerate()
    {
        for s in 0..(1 + i % 3) {
            mesh.merge(&m::moved(
                bowl.clone(),
                Vec3::new(x, 1.27 + s as f32 * 0.045, -0.72),
                Quat::from_rotation_y(i as f32),
            ));
        }
    }
    // Big storage crocks on the lower shelf.
    let crock = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.1, 0.0)
        .to(0.15, 0.12)
        .to(0.14, 0.26)
        .corner(0.11, 0.3)
        .corner(0.1, 0.3)
        .to(0.09, 0.27);
    let crock = m::revolve(&crock, 24);
    for x in [-1.2f32, -0.6, 0.7, 1.25] {
        mesh.merge(&m::moved(
            crock.clone(),
            Vec3::new(x, 0.57, -0.72),
            Quat::IDENTITY,
        ));
    }
    mesh
}

/// A slatted produce crate, 0.5 × 0.2 × 0.36 m, origin under its middle.
fn crate_box() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.merge(&board(
        Vec3::new(0.5, 0.015, 0.36),
        Vec3::new(0.0, 0.008, 0.0),
    ));
    for z in [-0.17f32, 0.17] {
        for y in [0.05f32, 0.15] {
            mesh.merge(&board(Vec3::new(0.5, 0.06, 0.015), Vec3::new(0.0, y, z)));
        }
    }
    for x in [-0.242f32, 0.242] {
        mesh.merge(&m::moved(
            m::block(Vec3::new(0.36, 0.2, 0.016), 0.004),
            Vec3::new(x, 0.1, 0.0),
            Quat::from_rotation_y(FRAC_PI_2),
        ));
    }
    tinted(mesh, lin(0xd2b48a))
}

/// A heap of round produce in a crate (`r` each, `squash` the height), seeded.
fn heap(seed: u64, r: f32, squash: f32, colors: &[u32]) -> Mesh {
    let ball = m::icosphere(2);
    let mut rng = Rng::new(seed);
    let mut mesh = Mesh::default();
    let (hx, hz) = (0.23 - r, 0.16 - r);
    let nx = ((2.0 * hx) / (2.0 * r * 0.95)).floor().max(1.0) as i32;
    let nz = ((2.0 * hz) / (2.0 * r * 0.95)).floor().max(1.0) as i32;
    for layer in 0..2 {
        for i in 0..=nx {
            for k in 0..=nz {
                let (u, w) = (i as f32 / nx as f32, k as f32 / nz as f32);
                // The top layer mounds toward the middle.
                let mid = 1.0 - ((u - 0.5).abs().max((w - 0.5).abs()) * 2.0);
                if layer == 1 && mid < 0.25 {
                    continue;
                }
                let p = Vec3::new(
                    -hx + 2.0 * hx * u + rng.range(-0.3, 0.3) * r,
                    0.02 + r * squash * (1.0 + layer as f32 * 1.7) + mid * 0.02 * layer as f32,
                    -hz + 2.0 * hz * w + rng.range(-0.3, 0.3) * r,
                );
                let s = r * rng.range(0.88, 1.1);
                let c = colors[rng.range_i32(0, colors.len() as i32) as usize];
                let shade = rng.range(0.8, 1.1);
                mesh.merge(&tinted(
                    m::placed(
                        ball.clone(),
                        p,
                        Quat::from_rotation_y(rng.range(0.0, TAU)),
                        Vec3::new(s, s * squash, s),
                    ),
                    lin(c) * shade,
                ));
            }
        }
    }
    mesh
}

fn apples() -> Mesh {
    heap(
        0xa991e,
        0.038,
        0.92,
        &[0x8e1a16, 0xa52a1c, 0x7d1d12, 0xb5452a],
    )
}
fn pears() -> Mesh {
    heap(0x9ea25, 0.036, 1.15, &[0xb9a33a, 0xa6a13d, 0xc4a94a])
}
fn cabbages() -> Mesh {
    heap(0xcabba9e, 0.075, 0.85, &[0x5f8a35, 0x6f9a3c, 0x7da54a])
}
fn onions() -> Mesh {
    heap(0x0e1015, 0.03, 0.9, &[0xb57a3e, 0xa5652e, 0xc28a4c])
}

/// A burlap sack of potatoes, 0.55 m tall, its mouth rolled down.
fn sack() -> Mesh {
    let p = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.15, 0.0)
        .to(0.2, 0.08)
        .to(0.21, 0.3)
        .to(0.19, 0.45)
        .corner(0.2, 0.52)
        .corner(0.17, 0.53)
        .to(0.12, 0.48)
        .corner(0.0, 0.47);
    let mut mesh = m::revolve(&p, 20);
    m::noise_warp(&mut mesh, 0.018, 9.0, 0x5ac);
    mesh
}

/// The baker's wares: long loaves, round breads and pretzels in shallow baskets.
fn breads() -> Mesh {
    let ball = m::icosphere(2);
    let mut rng = Rng::new(0xb4ead);
    let mut mesh = Mesh::default();
    for (bx, kind) in [(-1.1f32, 0), (0.0, 1), (1.1, 2)] {
        for i in 0..6 {
            let a = i as f32 / 6.0 * TAU + rng.range(-0.2, 0.2);
            let (cx, cz) = (bx + a.cos() * 0.13, 0.55 + a.sin() * 0.11);
            let y = COUNTER_Y + 0.07;
            let shade = lin(0xa8642c) * rng.range(0.85, 1.1);
            let piece = match kind {
                0 => m::placed(
                    ball.clone(),
                    Vec3::new(cx, y + 0.03, cz),
                    Quat::from_rotation_y(a + rng.range(-0.3, 0.3)),
                    Vec3::new(0.14, 0.055, 0.065),
                ),
                1 => m::placed(
                    ball.clone(),
                    Vec3::new(cx, y + 0.025, cz),
                    Quat::from_rotation_y(a),
                    Vec3::new(0.085, 0.05, 0.085),
                ),
                _ => pretzel(Vec3::new(cx, y + 0.02, cz), a),
            };
            mesh.merge(&tinted(piece, shade));
        }
    }
    mesh
}

/// A pretzel lying flat: a loop whose ends cross into the middle.
fn pretzel(at: Vec3, yaw: f32) -> Mesh {
    let path: Vec<Vec3> = (0..=24)
        .map(|i| {
            let t = -PI * 0.92 + 1.84 * PI * i as f32 / 24.0;
            // A lemniscate-like outline: two lobes meeting in a twist.
            Vec3::new(
                0.07 * t.sin(),
                0.008 * (2.0 * t).cos(),
                0.055 * (t.cos() - 0.35 * (2.0 * t).cos()),
            )
        })
        .collect();
    let mesh = m::tube(&path, |s| 0.012 + 0.006 * (PI * s).sin(), 8, true);
    m::moved(mesh, at, Quat::from_rotation_y(yaw))
}

/// Three shallow wicker baskets for the baker's counter.
fn baskets() -> Mesh {
    let p = Profile::new()
        .corner(0.0, 0.0)
        .corner(0.18, 0.0)
        .to(0.23, 0.06)
        .corner(0.25, 0.085)
        .corner(0.235, 0.09)
        .to(0.21, 0.05)
        .corner(0.0, 0.012);
    let b = m::revolve(&p, 28);
    let mut mesh = Mesh::default();
    for x in [-1.1f32, 0.0, 1.1] {
        mesh.merge(&m::moved(
            b.clone(),
            Vec3::new(x, COUNTER_Y, 0.55),
            Quat::IDENTITY,
        ));
    }
    tinted(mesh, lin(0xa98454))
}

/// The cloth merchant's lengths of cloth hung from the back rail, in folds.
fn drapes() -> Mesh {
    let colors = [0x7a2440, 0xd9c79a, 0x2f6b62, 0xb0703a];
    let mut mesh = Mesh::default();
    for (k, c) in colors.iter().enumerate() {
        let x0 = -1.6 + k as f32 * 0.82;
        let len = 1.2 + (k % 2) as f32 * 0.3;
        let rows: Vec<Vec<Vec3>> = (0..=8)
            .map(|j| {
                let y = BACK_H - 0.12 - len * j as f32 / 8.0;
                (0..=12)
                    .map(|i| {
                        let u = i as f32 / 12.0;
                        let fold = 0.035 * (u * TAU * 3.0 + k as f32).sin();
                        Vec3::new(x0 + u * 0.7, y, -STALL_HALF.y + 0.08 + fold)
                    })
                    .collect()
            })
            .collect();
        mesh.merge(&tinted(m::grid(&rows, false), lin(*c)));
    }
    m::two_sided(mesh)
}

/// Lantern on a stall's front post: an iron cage (the glass is [`lantern_glass`]).
fn lantern_cage() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.merge(&board(
        Vec3::new(0.16, 0.02, 0.16),
        Vec3::new(0.0, 0.0, 0.0),
    ));
    mesh.merge(&board(
        Vec3::new(0.18, 0.02, 0.18),
        Vec3::new(0.0, 0.24, 0.0),
    ));
    for (x, z) in [
        (-0.07f32, -0.07f32),
        (0.07, -0.07),
        (-0.07, 0.07),
        (0.07, 0.07),
    ] {
        mesh.merge(&post(Vec3::new(x, 0.0, z), 0.24, 0.014));
    }
    // The bracket to the post.
    mesh.merge(&beam(
        Vec3::new(0.0, 0.32, 0.0),
        Vec3::new(0.0, 0.32, -0.3),
        0.02,
        0.02,
    ));
    mesh.merge(&post(Vec3::new(0.0, 0.24, 0.0), 0.09, 0.015));
    mesh
}

fn lantern_glass() -> Mesh {
    board(Vec3::new(0.12, 0.2, 0.12), Vec3::new(0.0, 0.12, 0.0))
}

// ------------------------------------------------------------------------------------------
// Maypole

/// The pole, white with a blue spiral (painted in vertex colors), tapering as it rises.
fn maypole_pole() -> Mesh {
    const SIDES: u32 = 40;
    let rows = (MAYPOLE_H / 0.05) as u32;
    let white = lin(0xeceae2);
    let blue = lin(0x1d5aa8);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let r = |y: f32| 0.21 - 0.11 * (y / MAYPOLE_H);
    for j in 0..=rows {
        let y = MAYPOLE_H * j as f32 / rows as f32;
        for i in 0..=SIDES {
            let a = i as f32 / SIDES as f32 * TAU;
            let (s, c) = a.sin_cos();
            let n = Vec3::new(c, 0.11 / MAYPOLE_H, s).normalize();
            // The spiral: one turn every 1.3 m; the bottom 1.6 m stays white.
            let phase = (a / TAU + y / 1.3).fract();
            let color = if y > 1.6 && phase < 0.5 { blue } else { white };
            vertices.push(
                Vertex::new(Vec3::new(c * r(y), y, s * r(y)), n, Vec2::new(a * 0.2, y))
                    .with_color(color.extend(1.0)),
            );
        }
    }
    let ring = SIDES + 1;
    for j in 0..rows {
        for i in 0..SIDES {
            let a = j * ring + i;
            let b = a + ring;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh::new(vertices, indices)
}

/// Spruce garlands round the pole: a wide wreath and a smaller one above it.
fn maypole_wreaths() -> Mesh {
    let mut mesh = Mesh::default();
    for (y, radius, seed) in [(16.0f32, 0.95f32, 1u64), (20.5, 0.7, 2)] {
        let path: Vec<Vec3> = (0..48)
            .map(|i| {
                let a = i as f32 / 48.0 * TAU;
                Vec3::new(a.cos() * radius, y, a.sin() * radius)
            })
            .collect();
        let mut w = m::tube_loop(&path, Vec2::splat(0.11), 10);
        m::noise_warp(&mut w, 0.05, 7.0, seed);
        mesh.merge(&w);
    }
    mesh
}

/// Iron: the wreaths' hangers and the guild signs' arms and struts.
fn maypole_iron() -> Mesh {
    let mut mesh = Mesh::default();
    for (y, radius) in [(16.0f32, 0.95f32), (20.5, 0.7)] {
        for k in 0..4 {
            let a = k as f32 / 4.0 * TAU + 0.4;
            let rim = Vec3::new(a.cos() * radius, y, a.sin() * radius);
            mesh.merge(&m::tube(
                &[rim, Vec3::new(a.cos() * 0.12, y + 1.3, a.sin() * 0.12)],
                |_| 0.008,
                5,
                false,
            ));
        }
    }
    for (y, a) in sign_arms() {
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let r0 = 0.2 - 0.11 * y / MAYPOLE_H;
        mesh.merge(&m::tube(
            &[d * r0 + Vec3::Y * y, d * 1.45 + Vec3::Y * y],
            |_| 0.022,
            6,
            true,
        ));
        mesh.merge(&m::tube(
            &[d * r0 + Vec3::Y * (y - 0.55), d * 0.9 + Vec3::Y * y],
            |_| 0.014,
            6,
            true,
        ));
        // Two short chains down to the sign.
        for out in [0.55f32, 1.3] {
            mesh.merge(&m::tube(
                &[d * out + Vec3::Y * y, d * out + Vec3::Y * (y - 0.18)],
                |_| 0.008,
                4,
                false,
            ));
        }
    }
    mesh
}

/// Height and direction (radians round the pole) of each guild sign's arm: four levels of two,
/// each level turned so the signs face every side of the square.
fn sign_arms() -> impl Iterator<Item = (f32, f32)> {
    [(7.0f32, 0.3f32), (9.4, 1.1), (11.8, 1.9), (14.2, 2.7)]
        .into_iter()
        .flat_map(|(y, a)| [(y, a), (y, a + PI)])
}

/// The guild signs: painted boards in frames (color in the vertex colors), each with its
/// trade's emblem raised on it.
fn maypole_signs() -> Mesh {
    // Field, frame and emblem colors per sign.
    let looks = [
        (0x1d5aa8, 0x3a2414, 0xe9c46a),
        (0xeceae2, 0x3a2414, 0x9c2a22),
        (0x2f5d3a, 0x3a2414, 0xeceae2),
        (0x9c2a22, 0x3a2414, 0xe9c46a),
        (0xe9c46a, 0x3a2414, 0x1d5aa8),
        (0xeceae2, 0x3a2414, 0x2f5d3a),
        (0x1d5aa8, 0x3a2414, 0xeceae2),
        (0x6b3a26, 0x3a2414, 0xe9c46a),
    ];
    let mut mesh = Mesh::default();
    for (k, (y, a)) in sign_arms().enumerate() {
        let (field, frame, emblem) = looks[k % looks.len()];
        let mut sign = Mesh::default();
        // In the sign's own space: X along the arm, the board's face toward ±Z.
        sign.merge(&tinted(
            board(Vec3::new(0.82, 0.58, 0.035), Vec3::ZERO),
            lin(field),
        ));
        for (size, at) in [
            (Vec3::new(0.9, 0.05, 0.05), Vec3::new(0.0, 0.29, 0.0)),
            (Vec3::new(0.9, 0.05, 0.05), Vec3::new(0.0, -0.29, 0.0)),
            (Vec3::new(0.05, 0.62, 0.05), Vec3::new(0.43, 0.0, 0.0)),
            (Vec3::new(0.05, 0.62, 0.05), Vec3::new(-0.43, 0.0, 0.0)),
        ] {
            sign.merge(&tinted(board(size, at), lin(frame)));
        }
        // The emblem: a round boss with a bar across, on both faces.
        for side in [-1.0f32, 1.0] {
            let boss = m::revolve(
                &Profile::new()
                    .corner(0.0, 0.0)
                    .corner(0.14, 0.0)
                    .corner(0.14, 0.02)
                    .corner(0.0, 0.025),
                20,
            );
            sign.merge(&tinted(
                m::moved(
                    boss,
                    Vec3::new(0.0, 0.0, side * 0.017),
                    Quat::from_rotation_x(side * FRAC_PI_2),
                ),
                lin(emblem),
            ));
            sign.merge(&tinted(
                board(
                    Vec3::new(0.5, 0.06, 0.02),
                    Vec3::new(0.0, -0.17, side * 0.027),
                ),
                lin(emblem),
            ));
        }
        let rot = Quat::from_rotation_y(-a);
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        mesh.merge(&m::moved(sign, d * 0.92 + Vec3::Y * (y - 0.48), rot));
    }
    mesh
}

/// The pole's foot: a stone collar.
fn maypole_foot() -> Mesh {
    m::revolve(
        &Profile::new()
            .corner(0.0, -0.1)
            .corner(0.5, -0.1)
            .corner(0.5, 0.25)
            .corner(0.42, 0.32)
            .to(0.3, 0.45)
            .corner(0.24, 0.6)
            .corner(0.0, 0.6),
        32,
    )
}

/// The tip: a little spruce tree on top, as on Bavarian maypoles.
fn maypole_crown() -> Mesh {
    let mut mesh = Mesh::default();
    for (k, (y, r, h)) in [
        (0.0f32, 0.55f32, 1.0f32),
        (0.6, 0.42, 0.9),
        (1.15, 0.28, 0.8),
    ]
    .into_iter()
    .enumerate()
    {
        let mut cone = m::revolve(
            &Profile::new()
                .corner(0.0, 0.0)
                .corner(r, 0.12)
                .to(r * 0.5, h * 0.55)
                .corner(0.0, h),
            14,
        );
        m::noise_warp(&mut cone, 0.04, 6.0, 10 + k as u64);
        mesh.merge(&m::moved(
            cone,
            Vec3::Y * (MAYPOLE_H - 0.3 + y),
            Quat::IDENTITY,
        ));
    }
    mesh
}

/// Ribbons hanging from the wreaths, blue and white (vertex colors).
fn maypole_ribbons() -> Mesh {
    let (white, blue) = (lin(0xeceae2), lin(0x1d5aa8));
    let mut mesh = Mesh::default();
    for (y, radius) in [(16.0f32, 0.95f32), (20.5, 0.7)] {
        for k in 0..12 {
            let a = (k as f32 + 0.5) / 12.0 * TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            let side = Vec3::new(-a.sin(), 0.0, a.cos());
            let len = 1.3 + 0.4 * ((k * 7 % 5) as f32 / 4.0);
            let rows: Vec<Vec<Vec3>> = (0..=6)
                .map(|j| {
                    let t = j as f32 / 6.0;
                    let c = d * (radius + 0.02 + 0.1 * t * t) + Vec3::Y * (y - 0.08 - len * t);
                    vec![c - side * 0.035, c + side * 0.035]
                })
                .collect();
            let color = if k % 2 == 0 { blue } else { white };
            mesh.merge(&tinted(m::grid(&rows, false), color));
        }
    }
    m::two_sided(mesh)
}

// ------------------------------------------------------------------------------------------
// The oak, bollards

/// A round bench round the oak: eight seat sections on legs, the seat 0.45 m up.
fn round_bench() -> Mesh {
    let mut mesh = Mesh::default();
    let (r_in, r_out) = (0.95f32, 1.4f32);
    for k in 0..8 {
        let a = k as f32 / 8.0 * TAU;
        let rot = Quat::from_rotation_y(-a);
        let along = 2.0 * r_out * (PI / 8.0).tan() - 0.04;
        for r in [r_in + 0.08, r_in + 0.23, r_in + 0.38] {
            let len = along * r / r_out;
            let seat = board(Vec3::new(0.035 + len, 0.035, 0.13), Vec3::ZERO);
            mesh.merge(&m::moved(
                m::moved(
                    seat,
                    Vec3::new(0.0, 0.45, 0.0),
                    Quat::from_rotation_y(FRAC_PI_2),
                ),
                rot * Vec3::new(r, 0.0, 0.0),
                rot,
            ));
        }
        for r in [r_in + 0.1, r_out - 0.08] {
            mesh.merge(&m::moved(
                post(Vec3::ZERO, 0.43, 0.07),
                rot * Vec3::new(r, 0.0, 0.0),
                rot,
            ));
        }
    }
    tinted(mesh, lin(0xc29a6b))
}

/// The low stone curb round the oak's bed of grass.
fn tree_curb() -> Mesh {
    m::revolve(
        &Profile::new()
            .corner(2.75, -0.05)
            .corner(3.1, -0.05)
            .corner(3.1, 0.13)
            .corner(2.75, 0.15)
            .corner(2.75, -0.05),
        64,
    )
}

/// A stone bollard with a domed top, 0.8 m.
fn bollard() -> Mesh {
    m::revolve(
        &Profile::new()
            .corner(0.0, -0.05)
            .corner(0.19, -0.05)
            .to(0.17, 0.1)
            .to(0.15, 0.55)
            .corner(0.165, 0.6)
            .to(0.12, 0.74)
            .corner(0.0, 0.8),
        24,
    )
}

// ------------------------------------------------------------------------------------------
// Layout

/// Yaw that turns a stall's front (+Z) toward the fountain from angle `a` (radians).
fn facing_center(a: f64) -> f32 {
    (-(a.cos()) as f32).atan2(-(a.sin()) as f32)
}

fn canvas(hex: u32) -> StandardMaterial {
    StandardMaterial::hex(hex)
        .with_roughness(0.85)
        .with_subsurface(0.6)
        .with_detail(0.3, 0.02)
}

/// The square: paving, wall and fountain (Particles'), then the market round it.
pub fn spawn(commands: &mut Commands, lib: &Library, z: &Zone) {
    let c = z.center;
    let vm = &lib.mats.village;
    // Paving: granite setts in concentric bands (a darker band of smaller setts between two
    // light ones) round a polished marble middle, laid in world meters so the joints line up.
    let cobbles = |tile: f32, tint: u32| {
        StandardMaterial::preset(MaterialPreset::Cobblestone)
            .with_texture_scale(tile)
            .with_tint_hex(tint)
    };
    let paving = [
        (31.0f32, cobbles(1.5, 0xf2eee8)),
        (24.0, cobbles(1.0, 0xc4beb6)),
        (16.0, cobbles(1.5, 0xfaf6f0)),
        (
            8.0,
            StandardMaterial::preset(MaterialPreset::Marble).with_texture_scale(1.6),
        ),
    ];
    for (i, (r, material)) in paving.into_iter().enumerate() {
        commands.spawn((
            Transform::from_translation(c + DVec3::new(0.0, 0.1 + i as f64 * 0.005, 0.0)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: r,
                    height: 0.2,
                    segments: 128,
                },
                material.with_triplanar_world(),
            ),
        ));
    }
    // Low wall round the edge, with gaps for the four roads.
    let wall = StandardMaterial::preset(MaterialPreset::Concrete)
        .with_tint_hex(0xe6ddd0)
        .with_triplanar();
    for i in 0..32 {
        let a = i as f64 / 32.0 * std::f64::consts::TAU;
        if i % 8 == 0 {
            continue;
        }
        commands.spawn((
            Transform::from_translation(c + DVec3::new(a.cos() * 31.0, 0.35, a.sin() * 31.0))
                .with_rotation(Quat::from_rotation_y(-(a as f32) + FRAC_PI_2)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [6.2, 0.55, 0.6],
                },
                wall,
            ),
        ));
    }
    // A carved fountain in the middle of the marble (Particles' court fountain, a third bigger),
    // its jets falling into the basin.
    let fountain = 1.3;
    engine_vfx::demo::art::spawn_prop(
        commands,
        "plaza.fountain",
        Transform::from_translation(c + DVec3::new(0.0, 0.2, 0.0))
            .with_scale(Vec3::splat(fountain)),
        engine_vfx::demo::art::props::water::fountain,
    );
    commands.spawn((
        Transform::from_translation(
            c + DVec3::new(
                0.0,
                0.2 + (engine_vfx::demo::art::props::water::NOZZLE * fountain) as f64,
                0.0,
            ),
        ),
        engine_vfx::ParticleEffect::new("water.fountain"),
    ));

    let mut pieces = Pieces::default();
    let floor = c + DVec3::Y * FLOOR;
    let round = |a_deg: f64, r: f64| {
        let a = a_deg.to_radians();
        floor + DVec3::new(a.cos() * r, 0.0, a.sin() * r)
    };
    let cream = canvas(0xece3cf);
    let painted = vm.paint;
    let fabric = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_subsurface(0.5)
        .with_detail(0.25, 0.015);
    // Red glaze (smooth to matte) and copper (polished to dull), left to right as the customer
    // sees them: Rendering's roughness ladders.
    let ladder = |k: usize| (k as f32 / 6.0).max(0.05);
    for (n, &(trade, a_deg, color)) in STALLS.iter().enumerate() {
        let a = a_deg.to_radians();
        let pos = round(a_deg, STALL_R);
        let rot = Quat::from_rotation_y(facing_center(a));
        let tf = Transform::from_translation(pos).with_rotation(rot);
        let local = |p: Vec3| pos + (rot * p).as_dvec3();
        let at = |p: Vec3, yaw: f32| {
            Transform::from_translation(local(p)).with_rotation(rot * Quat::from_rotation_y(yaw))
        };
        pieces.add("stall_frame", stall_frame, tf, vm.timber[2], STALL_HIDE);
        pieces.add("awning_a", || awning(false), tf, canvas(color), STALL_HIDE);
        pieces.add("awning_b", || awning(true), tf, cream, STALL_HIDE);
        commands.spawn((
            Transform::from_translation(pos + DVec3::Y * 1.4).with_rotation(rot),
            NearSolid {
                shape: ColliderShape::Cuboid {
                    half_extents: Vec3::new(STALL_HALF.x + 0.05, 1.4, STALL_HALF.y + 0.05),
                },
                reach: 70.0,
            },
        ));
        // A lantern on the front-left post, lit.
        let lamp = Vec3::new(-STALL_HALF.x, 1.85, STALL_HALF.y + 0.3);
        pieces.add(
            "lantern_cage",
            lantern_cage,
            at(lamp, 0.0),
            vm.iron,
            WARES_HIDE,
        );
        pieces.add(
            "lantern_glass",
            lantern_glass,
            at(lamp, 0.0),
            vm.lantern,
            WARES_HIDE,
        );
        commands.spawn((
            Transform::from_translation(local(lamp + Vec3::Y * 0.12)),
            PointLight {
                color: [1.0, 0.7, 0.42],
                intensity_lm: 350.0,
                range: 7.0,
                radius: 0.05,
            },
        ));
        let counter = |x: f32, z: f32| Vec3::new(x, COUNTER_Y, z);
        match trade {
            Trade::Potter => {
                for k in 0..7 {
                    // Left to right as the customer sees them (from the front, −X is left).
                    let x = -1.45 + k as f32 * 0.48;
                    pieces.add(
                        "jug",
                        jug,
                        at(counter(x, 0.6), 2.4),
                        StandardMaterial::hex(0xa3281c).with_roughness(ladder(k)),
                        WARES_HIDE,
                    );
                }
                pieces.add(
                    "earthenware",
                    earthenware,
                    tf,
                    StandardMaterial::hex(0xb5653a)
                        .with_roughness(0.85)
                        .with_detail(0.4, 0.01),
                    WARES_HIDE,
                );
            }
            Trade::Coppersmith => {
                for k in 0..7 {
                    let x = -1.45 + k as f32 * 0.48;
                    pieces.add(
                        "pot",
                        pot,
                        at(counter(x, 0.6), 0.3 + k as f32),
                        StandardMaterial::hex(COPPER)
                            .with_metallic(1.0)
                            .with_roughness(ladder(k)),
                        WARES_HIDE,
                    );
                }
                for x in [-1.5f32, 1.5] {
                    pieces.add(
                        "pan",
                        pan,
                        at(Vec3::new(x, FRONT_H - 0.1, STALL_HALF.y + 0.06), 0.15),
                        StandardMaterial::hex(COPPER)
                            .with_metallic(1.0)
                            .with_roughness(0.4),
                        WARES_HIDE,
                    );
                }
                // Brass kettles on the shelf.
                for x in [-1.1f32, -0.2, 0.9] {
                    pieces.add(
                        "pot",
                        pot,
                        at(Vec3::new(x, 1.27, -0.72), x),
                        StandardMaterial::hex(0xf2e8b0)
                            .with_metallic(1.0)
                            .with_roughness(0.35),
                        WARES_HIDE,
                    );
                }
            }
            Trade::Greengrocer => {
                // Crates tipped toward the customer.
                let tip = Quat::from_rotation_x(0.22);
                for (x, wares) in [
                    (1.1f32, ("apples", apples as fn() -> Mesh)),
                    (0.0, ("cabbages", cabbages as fn() -> Mesh)),
                    (-1.1, ("pears", pears as fn() -> Mesh)),
                ] {
                    let p = counter(x, 0.58) + Vec3::Y * 0.05;
                    let t = Transform::from_translation(local(p)).with_rotation(rot * tip);
                    pieces.add("crate", crate_box, t, painted, WARES_HIDE);
                    pieces.add(
                        wares.0,
                        wares.1,
                        t,
                        StandardMaterial::color(1.0, 1.0, 1.0)
                            .with_roughness(0.45)
                            .with_subsurface(0.3),
                        WARES_HIDE,
                    );
                }
                for (x, yaw) in [(-0.9f32, 0.3f32), (0.6, -0.2)] {
                    let t = at(Vec3::new(x, 0.57, -0.72), yaw);
                    pieces.add("crate", crate_box, t, painted, WARES_HIDE);
                    pieces.add(
                        "onions",
                        onions,
                        t,
                        StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.6),
                        WARES_HIDE,
                    );
                }
            }
            Trade::Cloth => {
                pieces.add("drapes", drapes, tf, fabric, WARES_HIDE);
                // Bolts of cloth lying on the counter.
                for (k, hex) in [
                    0x7a2440u32,
                    0x2f6b62,
                    0xd9c79a,
                    0x3b4e7a,
                    0xb0703a,
                    0x5d4a6e,
                ]
                .into_iter()
                .enumerate()
                {
                    let x = -1.35 + k as f32 * 0.54;
                    commands.spawn((
                        Transform::from_translation(local(counter(x, 0.55) + Vec3::Y * 0.085))
                            .with_rotation(
                                rot * Quat::from_rotation_y(0.08 * (k as f32 - 2.5))
                                    * Quat::from_rotation_x(FRAC_PI_2),
                            ),
                        MeshRenderer::new(
                            Shape::Cylinder {
                                radius: 0.085,
                                height: 0.62,
                                segments: 20,
                            },
                            canvas(hex),
                        ),
                    ));
                }
            }
            Trade::Baker => {
                pieces.add("baskets", baskets, tf, painted, WARES_HIDE);
                pieces.add(
                    "breads",
                    breads,
                    tf,
                    StandardMaterial::color(1.0, 1.0, 1.0)
                        .with_roughness(0.7)
                        .with_detail(0.5, 0.008),
                    WARES_HIDE,
                );
            }
        }
        // By its side, a barrel or sacks and a stack of crates, turned off the stall's lines.
        let side = if n % 2 == 0 { 1.0 } else { -1.0 };
        let barrels = &lib.props.barrels;
        scenery::spawn_prop(
            commands,
            lib,
            &barrels[n % barrels.len()],
            local(Vec3::new(side * (STALL_HALF.x + 0.7), 0.0, -0.3)),
            facing_center(a) + 0.4 * side,
            0,
        );
        for (k, (dx, dz, y, yaw)) in [
            (0.75f32, 0.55f32, 0.0f32, 0.25f32),
            (0.82, 0.5, 0.2, -0.1),
            (0.7, 1.05, 0.0, 0.55),
        ]
        .into_iter()
        .enumerate()
        {
            if trade == Trade::Greengrocer && k == 2 {
                continue;
            }
            pieces.add(
                "crate",
                crate_box,
                at(
                    Vec3::new(-side * (STALL_HALF.x + dx), y, dz - 0.4),
                    yaw * side,
                ),
                painted,
                WARES_HIDE,
            );
        }
        if trade == Trade::Greengrocer || trade == Trade::Baker {
            for (dx, dz, yaw) in [(0.3f32, 1.25f32, 0.3f32), (0.75, 1.4, 1.9)] {
                pieces.add(
                    "sack",
                    sack,
                    at(Vec3::new(side * (STALL_HALF.x - dx), 0.0, dz), yaw),
                    StandardMaterial::hex(0xa08a62)
                        .with_roughness(1.0)
                        .with_detail(0.5, 0.006),
                    WARES_HIDE,
                );
            }
        }
    }

    // The maypole: the square's landmark, seen from the village's streets.
    let (ma, mr) = MAYPOLE;
    let foot = round(ma, mr);
    let pole = Transform::from_translation(foot).with_rotation(Quat::from_rotation_y(0.35));
    pieces.add(
        "maypole_pole",
        maypole_pole,
        pole,
        StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.45),
        f32::INFINITY,
    );
    pieces.add("maypole_foot", maypole_foot, pole, vm.stone, STALL_HIDE);
    let spruce = StandardMaterial::hex(0x2c4a26)
        .with_roughness(0.9)
        .with_subsurface(0.4)
        .with_detail(0.8, 0.03);
    pieces.add("maypole_wreaths", maypole_wreaths, pole, spruce, STALL_HIDE);
    pieces.add("maypole_crown", maypole_crown, pole, spruce, f32::INFINITY);
    pieces.add("maypole_iron", maypole_iron, pole, vm.iron, STALL_HIDE);
    pieces.add("maypole_signs", maypole_signs, pole, painted, STALL_HIDE);
    pieces.add("maypole_ribbons", maypole_ribbons, pole, fabric, STALL_HIDE);
    commands.spawn((
        Transform::from_translation(foot + DVec3::Y * 1.5),
        NearSolid {
            shape: ColliderShape::Capsule {
                radius: 0.5,
                half_height: 1.0,
            },
            reach: 70.0,
        },
    ));

    // The village oak in its bed of grass, a round bench under it.
    let oak = round(135.0, 21.0);
    pieces.add(
        "tree_curb",
        tree_curb,
        Transform::from_translation(oak),
        vm.stone,
        STALL_HIDE,
    );
    commands.spawn((
        Transform::from_translation(oak + DVec3::Y * 0.03),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 2.8,
                height: 0.12,
                segments: 64,
            },
            StandardMaterial::preset(MaterialPreset::Grass).with_triplanar_world(),
        ),
    ));
    pieces.add(
        "round_bench",
        round_bench,
        Transform::from_translation(oak).with_rotation(Quat::from_rotation_y(0.2)),
        painted,
        WARES_HIDE,
    );
    let t = lib
        .trees_of(crate::meshgen::TreeKind::Oak)
        .nth(1)
        .expect("oak");
    scenery::spawn_tree(commands, t, oak, 0.9, 1.2);
    commands.spawn((
        Transform::from_translation(oak + DVec3::Y * 1.5),
        NearSolid {
            shape: ColliderShape::Capsule {
                radius: 1.45,
                half_height: 0.1,
            },
            reach: 70.0,
        },
    ));
    // Oaks in planters at the other three corners.
    for k in [0, 2, 3] {
        let a = (k as f64 + 0.5) * 90.0;
        let p = round(a, 22.0);
        commands.spawn((
            Transform::from_translation(p + DVec3::Y * 0.4),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 2.0,
                    height: 0.8,
                    segments: 24,
                },
                wall,
            ),
        ));
        let t = lib
            .trees_of(crate::meshgen::TreeKind::Oak)
            .nth(k)
            .expect("oak");
        scenery::spawn_tree(commands, t, p + DVec3::Y * 0.6, a.to_radians() as f32, 0.8);
    }
    // Benches facing the fountain, and bollards at each road's way in.
    for a_deg in [30.0f64, -150.0, 100.0] {
        let a = a_deg.to_radians();
        scenery::spawn_prop(
            commands,
            lib,
            &lib.props.bench,
            round(a_deg, 12.5),
            facing_center(a),
            2,
        );
    }
    for road in 0..4 {
        for side in [-1.0f64, 1.0] {
            let a = road as f64 * 90.0 + side * 4.5;
            pieces.add(
                "bollard",
                bollard,
                Transform::from_translation(round(a, 30.0)),
                vm.stone,
                WARES_HIDE,
            );
        }
    }
    pieces.spawn(commands);
}

/// A customer's view of stall `n` of [`STALLS`] for the tour (eye, target): a few steps in
/// front of it and a little to one side, looking at the wares on the counter.
pub fn stall_view(n: usize) -> (DVec3, DVec3) {
    let a = STALLS[n].1.to_radians();
    let out = DVec3::new(a.cos(), 0.0, a.sin());
    let side = DVec3::new(-a.sin(), 0.0, a.cos());
    let c = crate::zones::zone("plaza").center + DVec3::Y * FLOOR;
    let stall = c + out * STALL_R;
    (
        stall - out * 3.0 + side * 0.5 + DVec3::Y * 1.6,
        stall - out * 0.4 + DVec3::Y * 1.05,
    )
}

/// Index in [`STALLS`] of the potter's and the coppersmith's stalls.
pub const POTTER: usize = 1;
pub const COPPERSMITH: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stalls_keep_clear_of_the_roads_the_trees_and_the_spawn() {
        let spot = |a: f64, r: f64| DVec2::new(a.to_radians().cos(), a.to_radians().sin()) * r;
        let walk_spawn = DVec2::new(8.0, 18.0);
        assert_eq!(STALLS[POTTER].0, Trade::Potter);
        assert_eq!(STALLS[COPPERSMITH].0, Trade::Coppersmith);
        for &(trade, a, _) in &STALLS {
            for road in [-180.0, -90.0, 0.0, 90.0, 180.0] {
                assert!((a - road).abs() > 15.0, "{trade:?} on a road");
            }
            for tree in [-135.0, -45.0, 45.0, 135.0] {
                assert!(
                    spot(a, STALL_R).distance(spot(tree, 22.0)) > 5.0,
                    "{trade:?} under a tree"
                );
            }
            assert!(
                spot(a, STALL_R).distance(walk_spawn) > 6.0,
                "{trade:?} on the spawn"
            );
            let (ma, mr) = MAYPOLE;
            assert!(spot(a, STALL_R).distance(spot(ma, mr)) > 6.0);
        }
    }

    #[test]
    fn the_maypole_spiral_and_the_wares_are_real_meshes() {
        for (name, mesh) in [
            ("pole", maypole_pole()),
            ("signs", maypole_signs()),
            ("awning", awning(false)),
            ("jug", jug()),
            ("apples", apples()),
            ("breads", breads()),
            ("bench", round_bench()),
        ] {
            assert!(mesh.triangle_count() > 20, "{name} is empty");
            assert!(
                mesh.vertices.iter().all(|v| v
                    .position
                    .iter()
                    .chain(&v.normal)
                    .all(|x| x.is_finite())),
                "{name} has a bad vertex"
            );
        }
        // Both colors of the spiral show.
        let pole = maypole_pole();
        let blue = pole
            .vertices
            .iter()
            .filter(|v| v.color[2] > v.color[0] * 2.0)
            .count();
        let share = blue as f32 / pole.vertices.len() as f32;
        assert!((0.35..0.55).contains(&share), "blue share {share}");
    }
}
