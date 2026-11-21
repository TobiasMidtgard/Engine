//! The sea: a round one, drawn by the game in three layers.
//!
//! Round every island lies a sheet of clear water over the terrain's sea bed, coloured by a
//! chart of its region's depths (pale and see-through over the sand of the shallows, deep
//! blue and opaque off the shelf, each climate with its own water, ice on the arctic one).
//! Everywhere else is the open ocean, a solid deep-blue shell round the planet that hides
//! the bed; the sheets thin out over it well off the shelf. Neither shines: the shine (waves,
//! the sun's glitter, the sky's reflection) is one clear glossy skin over the whole planet,
//! so it is the same everywhere and no layer's edge shows in it. All follow the planet's
//! curve exactly.
//!
//! (As the renderer stands, a blended surface only glitters in the sun where something solid
//! lies within a hundredth of its distance beneath it: seen from near, the skin glitters over
//! the ocean and not over the shallows, where only the bed is under it.)
//!
//! Why not glossy sheets over a glossy ocean: a smooth blended surface mirrors the sky however
//! thin it is, so a sheet thinning out showed its last quads at low angles. And why a solid
//! ocean: blended surfaces take their haze by another path than solid ones, which showed each
//! region as a darker disc from orbit.

use engine::core::math::noise::fbm2;
use engine::prelude::*;
use engine::render::{Image, Mesh, NoMeshLod, NoVirtualGeometry, Vertex};

use crate::globe::{self, Biome, Frame, Local};
use crate::island;

/// Side of a depth chart, texels.
const CHART: usize = 1024;
/// How far past a region's reach its sheet of water runs (as a share of the reach).
const MARGIN: f64 = 1.04;
/// Quads along a region's sheet, and along each of its pieces.
const SHEET_QUADS: usize = 192;
const SHEET_PIECES: usize = 8;
/// Pieces of ocean along a cube face, and quads along a piece.
const OCEAN_PIECES: usize = 12;
const OCEAN_QUADS: usize = 24;
/// The open ocean lies this far under the islands' sheets, meters (more than a sheet's flat
/// quads sag between their corners, so it never shows through one; less than the swell).
const OCEAN_UNDER_M: f64 = 0.3;
/// Water shallower than this is the sheets' alone (a sheet hides all beneath it from 14 m
/// down); deeper, the ocean lies under them.
const SHALLOW_M: f64 = 14.0;
/// The glossy skin lies this far over the sheets, meters (its flat quads sag half as much
/// again between their corners).
const SKIN_OVER_M: f64 = 0.08;
/// Blended things are drawn farthest first, by the middle of their bounds. The sheets have to
/// come before the skin over them, so each piece's bounds are stretched this far down into the
/// planet (by one loose corner), which puts its middle farther off than any skin near it.
const SHEET_BOUNDS_DOWN_M: f32 = 60_000.0;

/// Water colours of a climate (linear RGB): over the shallows, the shelf and the deep.
fn waters(biome: Biome) -> [[f32; 3]; 3] {
    match biome {
        Biome::Temperate => [[0.11, 0.45, 0.47], [0.03, 0.25, 0.36], DEEP],
        Biome::Tropical => [[0.20, 0.66, 0.60], [0.03, 0.34, 0.44], DEEP],
        Biome::Arid => [[0.22, 0.60, 0.56], [0.04, 0.30, 0.40], DEEP],
        Biome::Arctic => [[0.20, 0.42, 0.44], [0.04, 0.20, 0.27], DEEP],
    }
}

/// The open ocean's blue.
const DEEP: [f32; 3] = [0.012, 0.085, 0.19];

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let e = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (e * 255.0 + 0.5) as u8
}

/// The water's colour and how much it hides the bed, over a bed `depth` meters down. `keep`
/// (0..1) thins the water itself out (toward the open ocean beneath it) and `ice` the ice on it
/// (toward the edge of the arctic region).
fn water_at(biome: Biome, depth: f64, x: f64, z: f64, keep: f64, ice: f64) -> [u8; 4] {
    let [shallow, shelf, deep] = waters(biome);
    let a = smooth(0.0, 5.5, depth) as f32;
    let b = smooth(5.0, 30.0, depth) as f32;
    let mut c = [0.0f32; 3];
    for k in 0..3 {
        let mid = shallow[k] + (shelf[k] - shallow[k]) * a;
        c[k] = mid + (deep[k] - mid) * b;
    }
    // Thin and clear at the water's edge, opaque by the shelf's edge.
    let mut alpha = if depth <= 0.0 {
        0.0
    } else {
        0.50 * smooth(0.0, 0.5, depth)
            + 0.34 * smooth(0.5, 3.5, depth)
            + 0.16 * smooth(3.5, 14.0, depth)
    } as f32;
    alpha *= keep as f32;
    let ice_share = ice as f32;
    if biome == Biome::Arctic && depth > 2.5 {
        // Pack ice: pale floes with dark leads between, thicker to the north.
        let floes = fbm2(x / 420.0, z / 420.0, 911, 4, 2.1, 0.55) as f64
            + 0.35 * fbm2(x / 90.0, z / 90.0, 913, 2, 2.0, 0.5) as f64;
        let north = smooth(2_000.0, -7_000.0, z - globe::sheet_origin(3).y);
        let ice = smooth(0.34 - 0.3 * north, 0.40 - 0.3 * north, floes) as f32
            * smooth(2.5, 9.0, depth) as f32
            * ice_share;
        let white = [0.80, 0.86, 0.90];
        for k in 0..3 {
            c[k] += (white[k] - c[k]) * ice;
        }
        alpha += (1.0 - alpha) * ice;
    }
    [
        srgb(c[0]),
        srgb(c[1]),
        srgb(c[2]),
        (alpha * 255.0 + 0.5) as u8,
    ]
}

/// How far past an island's rim its own water starts to give way to the ocean's, and where it
/// has: in step with the island's size, but a continent's shelf is no wider than an isle's.
fn own_from(l: &island::Island) -> f64 {
    (l.radius * 0.9 + 200.0).min(2_600.0)
}

fn own_to(l: &island::Island) -> f64 {
    (l.radius * 1.3 + 400.0).min(3_600.0)
}

/// How much the water at `(x, z)` is the islands' own: all of it round an island, giving way
/// to the open ocean's well past the shelf, over deep water ([`own_from`] to [`own_to`]).
fn by_land<'a>(lands: impl IntoIterator<Item = &'a island::Island>, x: f64, z: f64) -> f64 {
    lands
        .into_iter()
        .map(|l| {
            let d = (DVec2::new(x, z) - l.center).length();
            1.0 - smooth(l.radius + own_from(l), l.radius + own_to(l), d)
        })
        .fold(0.0, f64::max)
}

/// The same for the pack ice, which lies over all of the arctic region's waters and gives out
/// toward their edge.
fn by_ice(x: f64, z: f64) -> f64 {
    let k = globe::regions()
        .iter()
        .position(|r| r.biome == Biome::Arctic)
        .expect("an arctic region");
    let far = (DVec2::new(x, z) - globe::sheet_origin(k)).length();
    let reach = globe::regions()[k].reach_m;
    1.0 - smooth(reach * 0.84, reach * 0.94, far)
}

/// Whether the sea at planet direction `d` is shallow enough to see the bed through (the
/// open ocean, which hides it, keeps out of there). Only where the sheet lies whole.
fn shallow(d: DVec3) -> bool {
    let m = globe::map_of(d);
    island::islands()
        .iter()
        .any(|l| (DVec2::new(m.x, m.z) - l.center).length() < l.radius + own_from(l))
        && island::height(m.x, m.z) > -SHALLOW_M
}

/// The depth chart of region `k`'s waters: straight-alpha sRGB RGBA8, `CHART` a side, over
/// the square of half-side `reach × MARGIN` round the sheet's middle (north up).
fn chart(k: usize) -> Vec<u8> {
    let region = &globe::regions()[k];
    let o = globe::sheet_origin(k);
    let half = region.reach_m * MARGIN;
    let step = 2.0 * half / CHART as f64;
    let lands: Vec<&island::Island> = island::islands()
        .iter()
        .filter(|i| i.region() == k)
        .collect();
    let rows: Vec<Vec<u8>> = engine_ui::demo::art::par_map(CHART, |j| {
        let z = o.y - half + (j as f64 + 0.5) * step;
        let mut row = Vec::with_capacity(CHART * 4);
        for i in 0..CHART {
            let x = o.x - half + (i as f64 + 0.5) * step;
            // Out from the islands the water thins away over the open ocean's, and deepens
            // to its blue on the way.
            let keep = by_land(lands.iter().copied(), x, z);
            let ice = if region.biome == Biome::Arctic {
                by_ice(x, z)
            } else {
                0.0
            };
            let depth = if keep > 0.0 {
                (-island::height(x, z)).max(60.0 * (1.0 - keep))
            } else {
                60.0
            };
            let px = water_at(region.biome, depth, x, z, keep, ice);
            row.extend_from_slice(&px);
        }
        row
    });
    rows.concat()
}

/// The water's body, a sheet's or the ocean's: colour only, without shine.
fn body_material(texture: (Handle<Image>, f32), alpha_mode: AlphaMode) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        base_color_texture: Some(texture.0),
        // One tile of the chart covers the whole sheet (UVs are meters).
        texture_scale: texture.1,
        alpha_mode,
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        // Broad lighter and darker patches, like wind on the water.
        detail: 0.5,
        detail_scale: 11.0,
        receives_wetness: false,
        ..Default::default()
    }
}

/// The water's skin: clear, so all it shows is what it mirrors.
fn skin_material() -> StandardMaterial {
    StandardMaterial {
        // (All but clear: nothing may take a wholly clear thing for one not worth drawing.)
        base_color: [DEEP[0], DEEP[1], DEEP[2], 0.004],
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.16,
        reflectance: 0.4,
        // Waves: bumps a dozen meters across (and finer ones seen from close by) that catch
        // the sun and the sky.
        detail: std::env::var("ARCH_SEA_SKIN_DETAIL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.7),
        detail_scale: 11.0,
        receives_wetness: false,
        ..Default::default()
    }
}

/// The sheet of water over region `k`, in pieces: each a mesh in its own flat space.
fn sheet_pieces(k: usize) -> Vec<(Local, Mesh)> {
    let region = &globe::regions()[k];
    let o = globe::sheet_origin(k);
    let half = region.reach_m * MARGIN;
    let side = 2.0 * half;
    let quad = side / SHEET_QUADS as f64;
    let per = SHEET_QUADS / SHEET_PIECES;
    let lands: Vec<&island::Island> = island::islands()
        .iter()
        .filter(|i| i.region() == k)
        .collect();
    let mut out = Vec::new();
    for pj in 0..SHEET_PIECES {
        for pi in 0..SHEET_PIECES {
            let corner = |i: usize, j: usize| {
                DVec2::new(
                    o.x - half + (pi * per + i) as f64 * quad,
                    o.y - half + (pj * per + j) as f64 * quad,
                )
            };
            let mid = corner(per / 2, per / 2);
            let space = Local::at(mid.x, mid.y);
            let n = per + 1;
            let mut vertices = Vec::with_capacity(n * n);
            for j in 0..n {
                for i in 0..n {
                    let p = corner(i, j);
                    let at = space.put(DVec3::new(p.x, 0.0, p.y));
                    let up = space.put(DVec3::new(p.x, 1.0, p.y)) - at;
                    vertices.push(Vertex {
                        position: at.to_array(),
                        normal: up.normalize_or(Vec3::Y).to_array(),
                        uv: [(p.x - (o.x - half)) as f32, (p.y - (o.y - half)) as f32],
                        color: [1.0; 4],
                    });
                }
            }
            let mut indices = Vec::new();
            for j in 0..per {
                for i in 0..per {
                    // Only inside the region's reach, and not where all is dry land.
                    let c = corner(i, j) + DVec2::splat(quad * 0.5);
                    if (c - o).length() > region.reach_m * 1.02 + quad {
                        continue;
                    }
                    let spots = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)]
                        .map(|(u, v)| corner(i, j) + DVec2::new(u, v) * quad);
                    if spots.iter().all(|p| island::height(p.x, p.y) > 6.0) {
                        continue;
                    }
                    // Nor far out, where the sheet has thinned to nothing (but for the ice,
                    // which lies over the whole region).
                    if region.biome != Biome::Arctic
                        && spots
                            .iter()
                            .all(|p| by_land(lands.iter().copied(), p.x, p.y) <= 0.0)
                    {
                        continue;
                    }
                    let a = (j * n + i) as u32;
                    let w = n as u32;
                    indices.extend_from_slice(&[a, a + w, a + w + 1, a, a + w + 1, a + 1]);
                }
            }
            if !indices.is_empty() {
                // (See `SHEET_BOUNDS_DOWN_M`: a corner no triangle uses.)
                vertices.push(Vertex {
                    position: [0.0, -SHEET_BOUNDS_DOWN_M, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    uv: [0.0; 2],
                    color: [1.0; 4],
                });
                out.push((space, Mesh::new(vertices, indices)));
            }
        }
    }
    out
}

/// The direction of a place on a cube face.
fn face_dir(face: usize, u: f64, v: f64) -> DVec3 {
    match face {
        0 => DVec3::new(1.0, u, v),
        1 => DVec3::new(-1.0, u, v),
        2 => DVec3::new(v, 1.0, u),
        3 => DVec3::new(v, -1.0, u),
        4 => DVec3::new(u, v, 1.0),
        _ => DVec3::new(u, v, -1.0),
    }
    .normalize()
}

/// One piece of the open ocean (planet pose and mesh), or `None` where all of it is land or
/// shallows; or, with `skin`, of the glossy skin over all the water there is.
fn ocean_piece(face: usize, pi: usize, pj: usize, skin: bool) -> Option<(DQuat, DVec3, Mesh)> {
    let total = (OCEAN_PIECES * OCEAN_QUADS) as f64;
    // A place on the face in half quads from the piece's corner (may lie a quad outside it).
    let dir2 = |a: i64, b: i64| {
        let coord = |piece: usize, a: i64| {
            ((piece * OCEAN_QUADS * 2) as f64 + a as f64) / (total * 2.0) * 2.0 - 1.0
        };
        face_dir(face, coord(pi, a), coord(pj, b))
    };
    let dir = |i: usize, j: usize| dir2(2 * i as i64, 2 * j as i64);
    let mid = dir(OCEAN_QUADS / 2, OCEAN_QUADS / 2);
    let side = mid.cross(face_dir(face, 0.3, 0.7)).normalize();
    let rot = DQuat::from_mat3(&DMat3::from_cols(side, mid, side.cross(mid))).normalize();
    let origin = mid * globe::RADIUS_M;
    let inv = rot.inverse();
    // What a piece leaves out: the ocean the shallows, the skin dry land. Sampled at every
    // half quad of the piece and of the quads round it.
    let w2 = 2 * (OCEAN_QUADS + 2) + 1;
    let outs: Vec<bool> = (0..w2 * w2)
        .map(|k| {
            let d = dir2((k % w2) as i64 - 2, (k / w2) as i64 - 2);
            if skin {
                island::ground(d) > 4.0
            } else {
                shallow(d)
            }
        })
        .collect();
    // Whether the quad at `(i, j)` (-1 and `OCEAN_QUADS`: the neighbours' nearest) is left
    // out, by its corners, middle and the middles of its sides: the ocean keeps off a quad
    // that touches shallows anywhere, the skin only off one that is all dry.
    let left_out = |i: i64, j: i64| {
        let (a0, b0) = ((2 * (i + 1)) as usize, (2 * (j + 1)) as usize);
        let mut spots = (0..3).flat_map(|b| (0..3).map(move |a| (a0 + a, b0 + b)));
        if skin {
            spots.all(|(a, b)| outs[b * w2 + a])
        } else {
            spots.any(|(a, b)| outs[b * w2 + a])
        }
    };
    let n = OCEAN_QUADS + 1;
    let mut vertices = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let d = dir(i, j);
            let (qi, qj) = (i as i64, j as i64);
            // The ocean does not end at a hole's rim, hanging over the bed far below (an
            // edge that the sun's contact shadows drew on the water as thin dark lines): the
            // quads next to a hole slope down into the bed. (By the quads round a corner
            // whichever piece they are in, so neighbours agree.)
            let rim = !skin
                && (left_out(qi - 1, qj - 1)
                    || left_out(qi, qj - 1)
                    || left_out(qi - 1, qj)
                    || left_out(qi, qj));
            let h = if skin {
                std::env::var("ARCH_SEA_SKIN_UP")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(SKIN_OVER_M)
            } else if rim {
                (island::ground(d) - 3.0).min(-OCEAN_UNDER_M)
            } else {
                -OCEAN_UNDER_M
            };
            let p = (inv * (d * (globe::RADIUS_M + h) - origin)).as_vec3();
            vertices.push(Vertex {
                position: p.to_array(),
                normal: (inv * d).as_vec3().to_array(),
                uv: [p.x, p.z],
                color: [1.0; 4],
            });
        }
    }
    let at = |i: usize, j: usize| (inv * (dir(i, j) * globe::RADIUS_M - origin)).as_vec3();
    let flip = (at(1, 0) - at(0, 0)).cross(at(0, 1) - at(0, 0)).y < 0.0;
    let mut indices = Vec::new();
    for j in 0..OCEAN_QUADS {
        for i in 0..OCEAN_QUADS {
            if left_out(i as i64, j as i64) {
                continue;
            }
            let v = |i: usize, j: usize| (j * n + i) as u32;
            let (a, b, c, d) = (v(i, j), v(i + 1, j), v(i + 1, j + 1), v(i, j + 1));
            if flip {
                indices.extend_from_slice(&[a, c, b, a, d, c]);
            } else {
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    (!indices.is_empty()).then(|| (rot, origin, Mesh::new(vertices, indices)))
}

/// Spawn the sea: every region's waters, the ocean between and the skin over it all.
pub fn spawn(world: &mut World) {
    // SCRATCH (not for commit): ARCH_SEA_HIDE=sheets,ocean,skin leaves those layers out;
    // ARCH_SEA_SKIN_DETAIL=<f32> sets the skin's detail; ARCH_SEA_SKIN_UP=<m> lifts the skin.
    let hide = std::env::var("ARCH_SEA_HIDE").unwrap_or_default();
    let started = std::time::Instant::now();
    let frame = *world.resource::<Frame>();
    let mut pieces = 0;
    for k in 0..globe::regions().len() {
        let side = (globe::regions()[k].reach_m * MARGIN * 2.0) as f32;
        let image = Image::new(CHART as u32, CHART as u32, chart(k), true);
        let texture = world.resource_mut::<Assets<Image>>().add(image);
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(body_material((texture, side), AlphaMode::Blend));
        for (space, mesh) in sheet_pieces(k) {
            if hide.contains("sheets") {
                continue;
            }
            let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
            let mut r = MeshRenderer::new(mesh, material);
            r.cast_shadows = false;
            world.spawn((
                Name::new(format!("Sea of {}", globe::regions()[k].name)),
                frame.local(&space),
                r,
                NoVirtualGeometry,
                NoMeshLod,
            ));
            pieces += 1;
        }
    }
    // The ocean takes its blue from a texture as the sheets do, so the two shade alike.
    let blue = water_at(Biome::Temperate, 60.0, 0.0, 0.0, 1.0, 0.0);
    let image = Image::new(4, 4, blue.repeat(16), true);
    let texture = world.resource_mut::<Assets<Image>>().add(image);
    let ocean = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(body_material((texture, 1_000.0), AlphaMode::Opaque));
    let skin = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(skin_material());
    let ids: Vec<(usize, usize, usize, bool)> = [false, true]
        .into_iter()
        .flat_map(|skin| {
            (0..6).flat_map(move |f| {
                (0..OCEAN_PIECES).flat_map(move |j| (0..OCEAN_PIECES).map(move |i| (f, i, j, skin)))
            })
        })
        .collect();
    let built = engine_ui::demo::art::par_map(ids.len(), |n| {
        let (f, i, j, skin) = ids[n];
        ocean_piece(f, i, j, skin).map(|p| (p, skin))
    });
    for ((rot, origin, mesh), is_skin) in built.into_iter().flatten() {
        if (is_skin && hide.contains("skin")) || (!is_skin && hide.contains("ocean")) {
            continue;
        }
        let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
        let mut r = MeshRenderer::new(mesh, if is_skin { skin } else { ocean });
        r.cast_shadows = false;
        world.spawn((
            Name::new(if is_skin { "Sea skin" } else { "Ocean" }),
            Transform::from_translation(frame.world(origin))
                .with_rotation((frame.rot * rot).normalize().as_quat()),
            r,
            NoVirtualGeometry,
            NoMeshLod,
        ));
        pieces += 1;
    }
    engine::core::log::info!(
        "archipelago sea: {pieces} pieces in {:.2} s",
        started.elapsed().as_secs_f32()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_is_clear_at_the_edge_and_deep_blue_off_the_shelf() {
        let edge = water_at(Biome::Temperate, 0.0, 0.0, 0.0, 1.0, 0.0);
        assert_eq!(edge[3], 0);
        let paddling = water_at(Biome::Tropical, 0.6, 0.0, 0.0, 1.0, 0.0);
        assert!((100..170).contains(&paddling[3]), "{paddling:?}");
        assert!(paddling[1] > paddling[2] / 2 && paddling[1] > paddling[0]);
        let deep = water_at(Biome::Temperate, 45.0, 0.0, 0.0, 1.0, 0.0);
        assert_eq!(deep[3], 255);
        assert!(deep[2] > deep[1] && deep[1] > deep[0], "{deep:?}");
        // Every region's deep water is the open ocean's.
        assert_eq!(
            &deep[..3],
            &water_at(Biome::Tropical, 80.0, 0.0, 0.0, 1.0, 0.0)[..3]
        );
    }

    #[test]
    fn the_ocean_keeps_out_of_the_shallows_and_the_sheets_keep_to_the_islands() {
        // The ocean's pieces over the home isles have holes for them; most pieces are whole.
        // The skin has none over water.
        let home = globe::regions()[0].anchor();
        let mut whole = [0, 0];
        let mut over_home = false;
        for f in 0..6 {
            for j in 0..OCEAN_PIECES {
                for i in 0..OCEAN_PIECES {
                    for skin in [false, true] {
                        if let Some((_, origin, m)) = ocean_piece(f, i, j, skin) {
                            let full = m.indices.len() == OCEAN_QUADS * OCEAN_QUADS * 6;
                            whole[skin as usize] += full as usize;
                            let far = origin.normalize().dot(home).acos() * globe::RADIUS_M;
                            // (Pieces are 5 km across: the nearest few share the home isles.)
                            over_home |= !skin && far < 4_500.0 && !full;
                        }
                    }
                }
            }
        }
        assert!(
            over_home && whole[0] > 700 && whole[1] > whole[0],
            "{whole:?} whole"
        );
        let c = island::islands()[0].center;
        assert!(shallow(globe::dir(c.x, c.y)));
        assert!(!shallow(globe::dir(c.x + 30_000.0, c.y)), "the open ocean");
        // A sheet's pieces lie round its islands and skip the dry middle of a continent.
        let all = SHEET_QUADS * SHEET_QUADS;
        for k in 0..3 {
            let quads: usize = sheet_pieces(k).iter().map(|p| p.1.indices.len() / 6).sum();
            assert!(
                quads > all / 50 && quads < all * 4 / 5,
                "region {k}: {quads} of {all}"
            );
        }
        // Every island's own water lies inside its region's sheet.
        for l in island::islands() {
            let k = l.region();
            let far = (l.center - globe::sheet_origin(k)).length();
            let out = l.radius + own_to(l);
            assert!(
                far + out < globe::regions()[k].reach_m * 1.02,
                "{}: {far:.0} + {out:.0} m of {:.0}",
                l.name,
                globe::regions()[k].reach_m
            );
        }
        // The water thins out between the shelf and the open ocean; the ice only at the
        // edge of the arctic region.
        let lands = &island::islands()[..1];
        assert_eq!(by_land(lands, c.x, c.y), 1.0);
        assert_eq!(
            by_land(lands, c.x + lands[0].radius + own_to(&lands[0]) + 1.0, c.y),
            0.0
        );
        assert_eq!(water_at(Biome::Temperate, 60.0, 0.0, 0.0, 0.0, 0.0)[3], 0);
        let north = globe::sheet_origin(3) + DVec2::new(0.0, -3_000.0);
        let floe = |ice: f64| {
            (0..40)
                .map(|i| {
                    water_at(
                        Biome::Arctic,
                        60.0,
                        north.x + i as f64 * 97.0,
                        north.y,
                        0.0,
                        ice,
                    )
                })
                .map(|px| px[3])
                .max()
                .unwrap()
        };
        assert!(floe(1.0) > 200, "ice lies on the arctic water");
        assert_eq!(floe(0.0), 0);
        assert_eq!(by_ice(north.x, north.y), 1.0);
    }
}
