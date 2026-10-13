//! The showcase's market street, where the UI test cases play: five stalls of different
//! trades around a cobbled square (Old Brannoc's in the middle), bunting from a central mast,
//! iron lamp posts, a handcart, and a map table with Veyra's northern highlands drawn on
//! parchment and a brass armillary sphere. All of it is generated in code.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_assets::Handle;
use engine_core::prelude::*;
use engine_render::Mesh;
use engine_render::prelude::*;
use engine_world::Ground;

use super::art::mesh::{beam, bevel_box, icosphere, lathe, tube};
use super::art::props::{barrel_mesh, sack_mesh};
use super::art::stall::{Goods, StallMats, spawn_stall};
use super::art::textures::{
    CREAM, HeightGrid, canvas_texel, cobble_texel, linen_texel, parchment_map_texel, rock_texel,
    wicker_texel, wood_texel,
};
use super::art::{
    TexSet, add_mat, add_mesh, bake_set, fix_winding, hex, lamp, moved, noise, rgba, sat, sstep,
    textured, tint, vtx,
};

/// Stalls stand on a ring this far from the square's middle, facing it.
pub const STALL_RING_M: f64 = 12.0;
/// The paved square's radius (its edge frays out over the next few meters).
pub const SQUARE_M: f32 = 14.5;

/// Marks everything the market spawned.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct MarketArt;

/// The five stalls: trade and awning stripe color, in ring order. Old Brannoc's is the
/// middle one.
const STALLS: [(Goods, u32); 5] = [
    (Goods::Produce, 0x2f5a2a),
    (Goods::Cloth, 0x5a2a5e),
    (Goods::Trader, 0x6a1a0e),
    (Goods::Potter, 0x1f3f66),
    (Goods::Smith, 0x8a6418),
];

/// Where stall `i` stands (relative to the square's middle, ground level) and its yaw (its
/// front, local +Z, faces the middle).
pub fn stall_place(i: usize) -> (DVec3, f32) {
    let a = i as f64 / STALLS.len() as f64 * std::f64::consts::TAU * 0.75 + 0.6;
    let p = DVec3::new(a.cos() * STALL_RING_M, 0.0, a.sin() * STALL_RING_M);
    let yaw = (-(a.cos()) as f32).atan2(-(a.sin()) as f32);
    (p, yaw)
}

/// The camera for the UI tour stops (relative to the square's middle): 6.5 m in front of Old
/// Brannoc's stall, looking at its counter, so the HUD's world markers and the screens have
/// the stall behind them. Returns (eye, target).
pub fn ui_view() -> (DVec3, DVec3) {
    let (p, yaw) = stall_place(2);
    let front = DVec3::new(yaw.sin() as f64, 0.0, yaw.cos() as f64);
    (p + front * 6.5 + DVec3::Y * 1.7, p + DVec3::Y * 1.5)
}

/// Materials shared by everything in the market.
struct Mats {
    wood: Handle<StandardMaterial>,
    iron: Handle<StandardMaterial>,
    crystal: Handle<StandardMaterial>,
    pottery: Handle<StandardMaterial>,
    burlap: Handle<StandardMaterial>,
    lamp_glass: Handle<StandardMaterial>,
    produce: Handle<StandardMaterial>,
    cloth: Handle<StandardMaterial>,
    wicker: Handle<StandardMaterial>,
    steel: Handle<StandardMaterial>,
    brass: Handle<StandardMaterial>,
    cobble: Handle<StandardMaterial>,
    stone: Handle<StandardMaterial>,
    parchment: Handle<StandardMaterial>,
}

impl Mats {
    fn new(world: &mut World) -> Self {
        let wood = bake_set(world, 512, 1024, 0.5, true, 1.0, wood_texel);
        let rock = bake_set(world, 512, 512, 1.6, true, 1.6, rock_texel);
        let linen = bake_set(world, 512, 512, 0.5, true, 1.0, linen_texel);
        let wicker = bake_set(world, 512, 512, 0.4, true, 1.0, wicker_texel);
        let cobble = bake_set(world, 1024, 1024, 2.0, true, 1.0, cobble_texel);
        let parchment = parchment(world);
        let burlap_canvas = bake_set(world, 512, 512, 1.0, true, 1.0, |u, v| {
            canvas_texel(u * 0.5 + 0.25, v * 0.5 + 0.2, CREAM, CREAM, 1.0, 9)
        });
        Self {
            wood: add_mat(world, textured(&wood, 1.0)),
            iron: add_mat(
                world,
                StandardMaterial::color(0.09, 0.085, 0.08)
                    .with_metallic(0.85)
                    .with_roughness(0.55),
            ),
            crystal: add_mat(
                world,
                StandardMaterial {
                    base_color: [0.25, 0.75, 0.8, 1.0],
                    perceptual_roughness: 0.12,
                    reflectance: 0.7,
                    subsurface: 0.6,
                    emissive: [0.2, 0.85, 0.9],
                    emissive_strength: 25.0,
                    ..Default::default()
                },
            ),
            pottery: add_mat(
                world,
                StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.42),
            ),
            burlap: add_mat(
                world,
                StandardMaterial {
                    base_color: [0.42, 0.32, 0.21, 1.0],
                    subsurface: 0.2,
                    ..textured(&burlap_canvas, 3.0)
                },
            ),
            lamp_glass: add_mat(world, lamp([1.0, 0.62, 0.3], 900.0)),
            produce: add_mat(
                world,
                StandardMaterial {
                    base_color: [1.0; 4],
                    perceptual_roughness: 0.45,
                    subsurface: 0.35,
                    ..Default::default()
                },
            ),
            cloth: add_mat(
                world,
                StandardMaterial {
                    double_sided: true,
                    subsurface: 0.45,
                    ..textured(&linen, 2.0)
                },
            ),
            wicker: add_mat(world, textured(&wicker, 1.0 / 0.4)),
            steel: add_mat(
                world,
                StandardMaterial::color(0.62, 0.62, 0.64)
                    .with_metallic(1.0)
                    .with_roughness(0.22),
            ),
            brass: add_mat(
                world,
                StandardMaterial {
                    base_color: rgba(hex(0xb08d3c)).to_array(),
                    metallic: 1.0,
                    perceptual_roughness: 0.32,
                    ..Default::default()
                },
            ),
            cobble: add_mat(
                world,
                StandardMaterial {
                    alpha_mode: AlphaMode::Mask(0.5),
                    reflectance: 0.4,
                    ..textured(&cobble, 0.5)
                },
            ),
            stone: add_mat(world, textured(&rock, 1.0 / 1.6)),
            parchment: add_mat(
                world,
                StandardMaterial {
                    double_sided: true,
                    reflectance: 0.3,
                    ..textured(&parchment, 1.0)
                },
            ),
        }
    }

    fn stall(&self, canvas: Handle<StandardMaterial>) -> StallMats {
        StallMats {
            wood: self.wood,
            canvas,
            iron: self.iron,
            crystal: self.crystal,
            pottery: self.pottery,
            burlap: self.burlap,
            lamp_glass: self.lamp_glass,
            produce: self.produce,
            cloth: self.cloth,
            wicker: self.wicker,
            steel: self.steel,
            brass: self.brass,
        }
    }
}

/// The map on the table: Veyra's northern highlands around Hollow Reach, from the same
/// terrain the star map draws.
fn parchment(world: &mut World) -> TexSet {
    let map = crate::map::demo_map();
    let Some((_, veyra)) = map.planet("veyra") else {
        return bake_set(world, 8, 8, 1.0, false, 0.0, |u, v| {
            parchment_map_texel(u, v, &HeightGrid::sample(2, 2, |_, _| 0.0), 0.5, 0.1, &[])
        });
    };
    let look = veyra.look();
    let (lon0, lon1, lat0, lat1) = (-2.0f64, 30.0, 34.0, 60.0);
    let land = HeightGrid::sample(320, 260, |u, v| {
        let lon = lon0 + (lon1 - lon0) * u as f64;
        let lat = lat1 - (lat1 - lat0) * v as f64;
        crate::planet_noise::height(crate::planet_noise::direction(lon, lat), &look)
    });
    let marks: Vec<Vec2> = veyra
        .pois
        .iter()
        .filter(|p| (lon0..lon1).contains(&p.lon) && (lat0..lat1).contains(&p.lat))
        .map(|p| {
            Vec2::new(
                ((p.lon - lon0) / (lon1 - lon0)) as f32,
                ((lat1 - p.lat) / (lat1 - lat0)) as f32,
            )
        })
        .collect();
    let sea = look.shape[1];
    bake_set(world, 1024, 832, 1.0, false, 1.0, move |u, v| {
        parchment_map_texel(u, v, &land, sea, 0.035, &marks)
    })
}

/// Spawns the market around `origin` (the square's middle, at ground level).
pub fn spawn_market(world: &mut World, origin: DVec3) {
    let ground = world.get_resource::<Ground>().cloned().unwrap_or_default();
    let ground_y = |p: DVec3| ground.height(p).unwrap_or(origin.y);
    let mats = Mats::new(world);
    square(world, &mats, origin, &ground_y);
    let mut fronts = Vec::new();
    for (i, (goods, stripe)) in STALLS.into_iter().enumerate() {
        let (p, yaw) = stall_place(i);
        let at = origin + p;
        let y = ground_y(at);
        let set = bake_set(world, 1024, 1024, 3.4, false, 1.0, move |u, v| {
            canvas_texel(u, v, CREAM, hex(stripe) * 0.85, 9.0, i as u32)
        });
        let canvas = add_mat(
            world,
            StandardMaterial {
                double_sided: true,
                subsurface: 0.55,
                alpha_mode: AlphaMode::Mask(0.5),
                ..textured(&set, 1.0)
            },
        );
        let t = Transform::from_translation(DVec3::new(at.x, y, at.z))
            .with_rotation(Quat::from_rotation_y(yaw));
        for e in spawn_stall(world, &mats.stall(canvas), t, goods, 40 + i as u64) {
            world.entity_mut(e).insert(MarketArt);
        }
        // The front beam's top above each corner post, for the bunting.
        for x in [-1.6, 1.6] {
            fronts.push(t.transform_point(DVec3::new(x, 2.38, 1.0)));
        }
    }
    mast_and_bunting(world, &mats, origin, ground_y(origin), &fronts);
    for k in 0..4 {
        let a = k as f64 / 4.0 * std::f64::consts::TAU + 0.35;
        let p = origin + DVec3::new(a.cos() * 7.5, 0.0, a.sin() * 7.5);
        lamp_post(world, &mats, DVec3::new(p.x, ground_y(p), p.z), -(a as f32));
    }
    let table = origin + DVec3::new(0.5, 0.0, 4.5);
    map_table(
        world,
        &mats,
        DVec3::new(table.x, ground_y(table), table.z),
        0.0,
    );
    let cart = origin + DVec3::new(10.5, 0.0, -3.5);
    handcart(
        world,
        &mats,
        DVec3::new(cart.x, ground_y(cart), cart.z),
        -0.6,
    );
    let well_at = origin + DVec3::new(-4.5, 0.0, -4.5);
    well(
        world,
        &mats,
        DVec3::new(well_at.x, ground_y(well_at), well_at.z),
        0.5,
    );
    for (x, z, yaw) in [(4.0f64, -8.0f64, 0.9f32), (-7.5, 1.5, 1.4)] {
        let p = origin + DVec3::new(x, 0.0, z);
        bench(world, &mats, DVec3::new(p.x, ground_y(p), p.z), yaw);
    }
    // Stock waiting to be shelved, between the stalls.
    let mut rng = Rng::new(77);
    for i in 0..STALLS.len() - 1 {
        let (a, _) = stall_place(i);
        let (b, _) = stall_place(i + 1);
        let mid = (a + b) * 0.5 * 0.97;
        let p = origin + mid;
        clutter(world, &mats, DVec3::new(p.x, ground_y(p), p.z), &mut rng);
    }
    // Straw dropped where stock is carried: in front of every stall and round the handcart.
    let mut spots: Vec<(DVec3, f32, Vec2, u32)> = (0..STALLS.len())
        .map(|i| {
            let (p, yaw) = stall_place(i);
            // Centred a little in front of the counter.
            let front = DVec3::new(yaw.sin() as f64, 0.0, yaw.cos() as f64);
            (origin + p + front * 1.0, yaw, Vec2::new(2.6, 1.6), 60)
        })
        .collect();
    spots.push((cart, -0.6, Vec2::new(1.6, 1.4), 45));
    straw(world, &mats, origin, &ground_y, &spots, &mut rng);
}

/// Straw lying on the cobbles: `count` clumps of loosely aligned blades in an ellipse of half
/// extents `extent` (local x, z) around each spot, thinning toward its edge.
fn straw(
    world: &mut World,
    mats: &Mats,
    origin: DVec3,
    ground_y: &dyn Fn(DVec3) -> f64,
    spots: &[(DVec3, f32, Vec2, u32)],
    rng: &mut Rng,
) {
    let colors = [0xc9a85au32, 0xb8944a, 0xd8bf7a, 0x9a7a40, 0x8a6a3a];
    let mut m = Mesh::default();
    for &(center, yaw, extent, count) in spots {
        let rot = Quat::from_rotation_y(yaw);
        for _ in 0..count {
            // Denser in the middle.
            let (a, r) = (rng.range(0.0, TAU), rng.range(0.0, 1.0f32).powf(1.6));
            let local = Vec3::new(a.cos() * r * extent.x, 0.0, a.sin() * r * extent.y);
            let clump = center + (rot * local).as_dvec3();
            let heading = rng.range(0.0, TAU);
            let blades = rng.range_i32(5, 15);
            for k in 0..blades {
                let off = Vec2::new(rng.range(-0.1, 0.1), rng.range(-0.1, 0.1));
                let at = clump + DVec3::new(off.x as f64, 0.0, off.y as f64);
                // Later blades lie on earlier ones.
                let y = (ground_y(at) - origin.y) as f32 + 0.024 + k as f32 * 0.0012;
                let base = Vec3::new((at.x - origin.x) as f32, y, (at.z - origin.z) as f32);
                let len = rng.range(0.07, 0.2);
                let dir = Quat::from_rotation_y(heading + rng.range(-0.6, 0.6)) * Vec3::X;
                let side = Vec3::new(-dir.z, 0.0, dir.x) * rng.range(0.0025, 0.0045);
                // Blades lie flat but curl up a little at one end.
                let tip = base + dir * len + Vec3::Y * rng.range(0.0, 0.02);
                let c = rgba(hex(colors[rng.range_i32(0, colors.len() as i32) as usize]))
                    * rng.range(0.8, 1.1);
                let i = m.vertices.len() as u32;
                for (p, u) in [
                    (base - side, 0.0),
                    (base + side, 0.0),
                    (tip + side * 0.5, len),
                    (tip - side * 0.5, len),
                ] {
                    m.vertices.push(vtx(p, Vec3::Y, Vec2::new(u, 0.0), c));
                }
                m.indices
                    .extend_from_slice(&[i, i + 2, i + 1, i, i + 3, i + 2]);
            }
        }
    }
    fix_winding(&mut m);
    let h = add_mesh(world, m);
    let mut r = MeshRenderer::new(h, mats.cloth);
    r.cast_shadows = false;
    world.spawn((Transform::from_translation(origin), r, MarketArt));
}

fn spawn(world: &mut World, mesh: Mesh, mat: Handle<StandardMaterial>, t: Transform) {
    if mesh.triangle_count() == 0 {
        return;
    }
    let h = add_mesh(world, mesh);
    world.spawn((t, MeshRenderer::new(h, mat), MarketArt));
}

/// The cobbled square: a disc of setts following the ground, fraying out stone by stone into
/// the grass at its edge.
fn square(world: &mut World, mats: &Mats, origin: DVec3, ground_y: &dyn Fn(DVec3) -> f64) {
    let cell = 0.5f32;
    let r_out = SQUARE_M + 3.5;
    let n = (2.0 * r_out / cell).ceil() as i32;
    let base_y = origin.y;
    let mut m = Mesh::default();
    let row = (n + 1) as u32;
    for k in 0..=n {
        for i in 0..=n {
            let x = -r_out + i as f32 * cell;
            let z = -r_out + k as f32 * cell;
            let d = (x * x + z * z).sqrt();
            let ragged = SQUARE_M + 2.2 * noise::fbm(x * 0.18, z * 0.18, 4, 351);
            let a = sat((ragged - d) / 2.5 + 0.55);
            let w = DVec3::new(origin.x + x as f64, base_y, origin.z + z as f64);
            let y = (ground_y(w) - base_y) as f32 + 0.02;
            m.vertices.push(vtx(
                Vec3::new(x, y, z),
                Vec3::Y,
                Vec2::new(x, z),
                Vec4::new(1.0, 1.0, 1.0, a),
            ));
        }
    }
    for k in 0..n as u32 {
        for i in 0..n as u32 {
            let a = k * row + i;
            let quad = [a, a + 1, a + row, a + row + 1];
            if quad.iter().all(|&q| m.vertices[q as usize].color[3] <= 0.0) {
                continue;
            }
            m.indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    m.compute_normals();
    fix_winding(&mut m);
    let t = Transform::from_translation(DVec3::new(origin.x, base_y, origin.z));
    let h = add_mesh(world, m);
    let mut r = MeshRenderer::new(h, mats.cobble);
    r.cast_shadows = false;
    world.spawn((t, r, MarketArt));
}

/// A tall mast in the middle with strings of pennants to both front corners of every stall.
fn mast_and_bunting(world: &mut World, mats: &Mats, origin: DVec3, ground: f64, fronts: &[DVec3]) {
    let base = DVec3::new(origin.x, ground, origin.z);
    let mast_h = 7.5f32;
    let mut wood = Mesh::default();
    wood.merge(&tint(
        lathe(
            &[
                (0.0, -0.2),
                (0.14, -0.2),
                (0.12, mast_h * 0.5),
                (0.08, mast_h),
                (0.0, mast_h + 0.02),
            ],
            20,
            0.12,
        ),
        Vec4::new(0.9, 0.85, 0.8, 1.0),
    ));
    let mut iron = Mesh::default();
    // A finial and a ring the strings tie to.
    iron.merge(&moved(
        lathe(
            &[(0.0, 0.0), (0.1, 0.04), (0.06, 0.2), (0.0, 0.36)],
            16,
            0.08,
        ),
        Mat4::from_translation(Vec3::Y * mast_h),
    ));
    let ring_y = mast_h - 0.6;
    iron.merge(&moved(
        lathe(
            &[(0.1, -0.03), (0.16, -0.03), (0.16, 0.03), (0.1, 0.03)],
            20,
            0.15,
        ),
        Mat4::from_translation(Vec3::Y * ring_y),
    ));
    // Stone footing.
    let mut stone = Mesh::default();
    stone.merge(&tint(
        lathe(
            &[
                (0.0, 0.0),
                (0.55, 0.0),
                (0.55, 0.18),
                (0.4, 0.3),
                (0.0, 0.3),
            ],
            24,
            0.5,
        ),
        Vec4::new(0.75, 0.72, 0.7, 1.0),
    ));
    let t = Transform::from_translation(base);
    spawn(world, wood, mats.wood, t);
    spawn(world, iron, mats.iron, t);
    spawn(world, stone, mats.stone, t);
    // Bunting: each string sags from the mast's ring to a stall, pennants every 0.4 m.
    let colors = [0x9c1a14u32, 0xd8cdb0, 0x264d7a, 0xc9a23a, 0x3f6b3a];
    let mut flags = Mesh::default();
    let mut line = Mesh::default();
    let top = Vec3::new(0.0, ring_y, 0.0);
    for (s, f) in fronts.iter().enumerate() {
        let end = (*f - base).as_vec3();
        let span = top.distance(end);
        let sag = 0.09 * span;
        let n = (span / 0.4) as usize;
        let at = |t: f32| top + (end - top) * t - Vec3::Y * (sag * 4.0 * t * (1.0 - t));
        let steps = 24;
        for k in 0..steps {
            let (a, b) = (
                at(k as f32 / steps as f32),
                at((k + 1) as f32 / steps as f32),
            );
            line.merge(&tube(a, b, 0.006, 4));
        }
        for k in 1..n {
            let t = k as f32 / n as f32;
            let a = at(t - 0.4 / span * 0.45);
            let b = at(t + 0.4 / span * 0.45);
            let mid = (a + b) * 0.5;
            // Hang in a little breeze: the tip swings out of plane.
            let swing = 0.06 * noise::n2(t * 7.0, s as f32, 361);
            let tip = mid - Vec3::Y * 0.26 + Vec3::new(swing, 0.0, -swing);
            let c = rgba(hex(colors[(k + s) % colors.len()]));
            let base_i = flags.vertices.len() as u32;
            let nrm = (b - a).cross(tip - a).normalize_or(Vec3::Z);
            flags.vertices.push(vtx(a, nrm, Vec2::new(0.0, 0.0), c));
            flags.vertices.push(vtx(b, nrm, Vec2::new(0.25, 0.0), c));
            flags
                .vertices
                .push(vtx(tip, nrm, Vec2::new(0.125, 0.26), c * 0.92));
            flags
                .indices
                .extend_from_slice(&[base_i, base_i + 1, base_i + 2]);
        }
    }
    spawn(world, flags, mats.cloth, t);
    // Hemp twine, dyed linen.
    spawn(world, tint(line, rgba(hex(0x7a6448))), mats.cloth, t);
}

/// An iron lamp post with a glowing lantern on a bracket, turned by `yaw`.
fn lamp_post(world: &mut World, mats: &Mats, at: DVec3, yaw: f32) {
    let mut iron = Mesh::default();
    let h = 3.2f32;
    iron.merge(&lathe(
        &[
            (0.0, 0.0),
            (0.16, 0.0),
            (0.16, 0.1),
            (0.09, 0.22),
            (0.06, 0.6),
            (0.045, h),
            (0.0, h + 0.05),
        ],
        16,
        0.06,
    ));
    // A curled bracket out to the lantern.
    let arm = 0.55f32;
    let steps = 10;
    for k in 0..steps {
        let t0 = k as f32 / steps as f32;
        let t1 = (k + 1) as f32 / steps as f32;
        let p = |t: f32| Vec3::new(t * arm, h - 0.15 + 0.18 * (PI * t).sin(), 0.0);
        iron.merge(&tube(p(t0), p(t1), 0.018, 8));
    }
    let lantern_at = Vec3::new(arm, h - 0.62, 0.0);
    iron.merge(&tube(
        lantern_at + Vec3::Y * 0.44,
        Vec3::new(arm, h - 0.12, 0.0),
        0.008,
        6,
    ));
    // A four-sided lantern: cap, base and corner posts.
    iron.merge(&moved(
        lathe(&[(0.0, 0.44), (0.17, 0.34), (0.15, 0.3)], 4, 0.15),
        Mat4::from_translation(lantern_at) * Mat4::from_rotation_y(PI * 0.25),
    ));
    iron.merge(&moved(
        lathe(
            &[(0.0, -0.02), (0.12, -0.02), (0.12, 0.02), (0.0, 0.02)],
            4,
            0.12,
        ),
        Mat4::from_translation(lantern_at) * Mat4::from_rotation_y(PI * 0.25),
    ));
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2;
        let o = Vec3::new(a.cos() * 0.11, 0.0, a.sin() * 0.11);
        iron.merge(&tube(
            lantern_at + o,
            lantern_at + o + Vec3::Y * 0.3,
            0.008,
            5,
        ));
    }
    let glass = moved(
        lathe(
            &[(0.0, 0.02), (0.09, 0.03), (0.095, 0.28), (0.0, 0.29)],
            4,
            0.09,
        ),
        Mat4::from_translation(lantern_at) * Mat4::from_rotation_y(PI * 0.25),
    );
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    spawn(world, iron, mats.iron, t);
    let h_glass = add_mesh(world, glass);
    let mut r = MeshRenderer::new(h_glass, mats.lamp_glass);
    r.cast_shadows = false;
    world.spawn((t, r, MarketArt));
    world.spawn((
        PointLight {
            color: [1.0, 0.62, 0.32],
            intensity_lm: 600.0,
            range: 12.0,
            radius: 0.06,
        },
        Transform::from_translation(t.transform_point((lantern_at + Vec3::Y * 0.15).as_dvec3())),
        MarketArt,
    ));
}

/// The map table: a heavy trestle table with the parchment map, an armillary sphere, rolled
/// charts, a compass and an inkwell.
fn map_table(world: &mut World, mats: &Mats, at: DVec3, yaw: f32) {
    let (w, d, top_y) = (2.2f32, 1.3f32, 0.92f32);
    let mut wood = Mesh::default();
    wood.merge(&moved(
        bevel_box(Vec3::new(w, 0.07, d), 0.02, 0),
        Mat4::from_translation(Vec3::Y * top_y),
    ));
    for sx in [-1.0f32, 1.0] {
        // Trestle: two splayed legs, a foot and a cross rail.
        let x = sx * (w * 0.5 - 0.3);
        for sz in [-1.0f32, 1.0] {
            wood.merge(&beam(
                Vec3::new(x, 0.08, sz * (d * 0.5 - 0.12)),
                Vec3::new(x, top_y - 0.04, sz * (d * 0.5 - 0.3)),
                0.09,
                0.09,
                0.015,
            ));
        }
        wood.merge(&moved(
            bevel_box(Vec3::new(0.12, 0.1, d - 0.1), 0.015, 2),
            Mat4::from_translation(Vec3::new(x, 0.05, 0.0)),
        ));
        wood.merge(&moved(
            bevel_box(Vec3::new(0.08, 0.08, d - 0.5), 0.012, 2),
            Mat4::from_translation(Vec3::new(x, top_y - 0.1, 0.0)),
        ));
    }
    wood.merge(&moved(
        bevel_box(Vec3::new(w - 0.6, 0.08, 0.08), 0.012, 0),
        Mat4::from_translation(Vec3::new(0.0, 0.35, 0.0)),
    ));
    // The map: a sheet with slightly lifted corners, pinned by the props.
    let (mw, md) = (1.55f32, 1.05f32);
    let mut sheet = Mesh::default();
    let (nu, nv) = (24usize, 16usize);
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f32 / nu as f32, j as f32 / nv as f32);
            let (cu, cv) = ((2.0 * u - 1.0).abs(), (2.0 * v - 1.0).abs());
            let lift = 0.012 * sstep(0.7, 1.0, cu) * sstep(0.6, 1.0, cv)
                + 0.004 * noise::n2(u * 5.0, v * 5.0, 371);
            let p = Vec3::new((u - 0.5) * mw - 0.12, top_y + 0.038 + lift, (v - 0.5) * md);
            sheet
                .vertices
                .push(vtx(p, Vec3::Y, Vec2::new(u, v), Vec4::ONE));
        }
    }
    let row = (nu + 1) as u32;
    for j in 0..nv as u32 {
        for i in 0..nu as u32 {
            let a = j * row + i;
            sheet
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    sheet.compute_normals();
    fix_winding(&mut sheet);
    // Rolled charts.
    let mut paper = Mesh::default();
    for (k, (x, z)) in [(0.88f32, -0.35f32), (0.95, -0.15), (0.8, 0.45)]
        .into_iter()
        .enumerate()
    {
        let r = 0.035 + 0.008 * k as f32;
        let a = Vec3::new(x, top_y + 0.035 + r, z);
        paper.merge(&tube(
            a,
            a + Vec3::new(0.08, 0.0, 0.42 - k as f32 * 0.08),
            r,
            14,
        ));
    }
    let mut brass = Mesh::default();
    let mut iron = Mesh::default();
    // Armillary sphere on a turned stand: three rings around a small globe.
    let c = Vec3::new(0.82, top_y + 0.45, 0.12);
    brass.merge(&moved(
        lathe(
            &[
                (0.0, 0.0),
                (0.12, 0.0),
                (0.12, 0.025),
                (0.05, 0.06),
                (0.03, 0.2),
                (0.05, 0.24),
                (0.02, 0.3),
                (0.0, 0.3),
            ],
            24,
            0.08,
        ),
        Mat4::from_translation(Vec3::new(c.x, top_y + 0.035, c.z)),
    ));
    let ring = |r: f32| {
        lathe(
            &[
                (r - 0.008, -0.006),
                (r + 0.008, -0.006),
                (r + 0.008, 0.006),
                (r - 0.008, 0.006),
            ],
            48,
            r,
        )
    };
    for (tilt, r) in [(0.0f32, 0.17f32), (FRAC_PI_2, 0.165), (0.41, 0.175)] {
        brass.merge(&moved(
            ring(r),
            Mat4::from_translation(c) * Mat4::from_rotation_z(0.4) * Mat4::from_rotation_x(tilt),
        ));
    }
    brass.merge(&tube(
        c - Vec3::new(0.0, 0.2, 0.0),
        c + Vec3::new(0.0, 0.2, 0.0),
        0.005,
        6,
    ));
    let (dirs, faces) = icosphere(3);
    let mut globe = Mesh {
        vertices: dirs
            .iter()
            .map(|&dd| {
                let land = noise::fbm3(dd.x * 2.0, dd.y * 2.0, dd.z * 2.0, 4, 381) > 0.05;
                let col = if land { hex(0x8a7a4a) } else { hex(0x2a4a5a) };
                vtx(c + dd * 0.08, dd, Vec2::new(dd.x, dd.y), rgba(col))
            })
            .collect(),
        indices: faces.iter().flat_map(|f| f.iter().copied()).collect(),
    };
    fix_winding(&mut globe);
    // A brass compass and an inkwell with a quill.
    brass.merge(&moved(
        lathe(
            &[(0.0, 0.0), (0.05, 0.0), (0.055, 0.018), (0.0, 0.02)],
            24,
            0.05,
        ),
        Mat4::from_translation(Vec3::new(-0.65, top_y + 0.04, 0.35)),
    ));
    iron.merge(&moved(
        lathe(
            &[
                (0.0, 0.0),
                (0.04, 0.0),
                (0.045, 0.05),
                (0.02, 0.06),
                (0.02, 0.07),
                (0.0, 0.07),
            ],
            16,
            0.04,
        ),
        Mat4::from_translation(Vec3::new(-0.9, top_y + 0.035, -0.4)),
    ));
    let mut quill = Mesh::default();
    let qa = Vec3::new(-0.9, top_y + 0.09, -0.4);
    let qb = qa + Vec3::new(0.08, 0.26, 0.06);
    quill.merge(&tint(tube(qa, qb, 0.003, 5), rgba(hex(0xe8e0d0))));
    for k in 0..14 {
        let t = 0.3 + 0.7 * k as f32 / 14.0;
        let p = qa + (qb - qa) * t;
        let wv = 0.018 * (PI * (t - 0.3) / 0.7).sin();
        let i = quill.vertices.len() as u32;
        let n = Vec3::X;
        let c4 = rgba(hex(0xf0ece4));
        quill.vertices.push(vtx(p, n, Vec2::ZERO, c4));
        quill
            .vertices
            .push(vtx(p + Vec3::new(0.0, 0.0, wv), n, Vec2::ZERO, c4));
        quill
            .vertices
            .push(vtx(p + (qb - qa) * 0.07, n, Vec2::ZERO, c4));
        quill.indices.extend_from_slice(&[i, i + 1, i + 2]);
    }
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    spawn(world, wood, mats.wood, t);
    spawn(world, sheet, mats.parchment, t);
    spawn(world, paper, mats.cloth, t);
    spawn(world, brass, mats.brass, t);
    spawn(world, iron, mats.iron, t);
    spawn(world, globe, mats.pottery, t);
    spawn(world, quill, mats.cloth, t);
}

/// A two-wheeled handcart with wicker sides, loaded with sacks and a barrel, resting on its
/// legs; `yaw` turns it.
fn handcart(world: &mut World, mats: &Mats, at: DVec3, yaw: f32) {
    let mut wood = Mesh::default();
    let mut iron = Mesh::default();
    let mut wicker = Mesh::default();
    let mut burlap = Mesh::default();
    let wheel_r = 0.55f32;
    let (bw, bl) = (1.1f32, 1.6f32);
    // The bed tilts forward onto its legs.
    let tilt = Quat::from_rotation_x(-0.08);
    let bed = Mat4::from_rotation_translation(tilt, Vec3::new(0.0, wheel_r + 0.05, 0.0));
    wood.merge(&moved(bevel_box(Vec3::new(bw, 0.05, bl), 0.01, 2), bed));
    for sx in [-1.0f32, 1.0] {
        // Shafts running forward into handles.
        wood.merge(&moved(
            beam(
                Vec3::new(sx * (bw * 0.5 - 0.05), -0.04, -bl * 0.5 - 0.1),
                Vec3::new(sx * 0.3, -0.12, bl * 0.5 + 1.1),
                0.07,
                0.07,
                0.012,
            ),
            bed,
        ));
        // Wicker side panels.
        wicker.merge(&moved(
            bevel_box(Vec3::new(0.04, 0.4, bl), 0.01, 2),
            bed * Mat4::from_translation(Vec3::new(sx * (bw * 0.5 - 0.02), 0.22, 0.0)),
        ));
        // A spoked wheel.
        let c = Vec3::new(sx * (bw * 0.5 + 0.12), wheel_r, -0.1);
        let face = Mat4::from_translation(c) * Mat4::from_rotation_z(FRAC_PI_2);
        wood.merge(&moved(
            lathe(
                &[
                    (wheel_r - 0.07, -0.04),
                    (wheel_r, -0.04),
                    (wheel_r, 0.04),
                    (wheel_r - 0.07, 0.04),
                    (wheel_r - 0.07, -0.04),
                ],
                40,
                wheel_r,
            ),
            face,
        ));
        iron.merge(&moved(
            lathe(
                &[
                    (wheel_r, -0.045),
                    (wheel_r + 0.012, -0.045),
                    (wheel_r + 0.012, 0.045),
                    (wheel_r, 0.045),
                ],
                40,
                wheel_r,
            ),
            face,
        ));
        wood.merge(&moved(
            lathe(
                &[(0.0, -0.09), (0.09, -0.09), (0.09, 0.09), (0.0, 0.09)],
                16,
                0.08,
            ),
            face,
        ));
        for k in 0..10 {
            let a = k as f32 / 10.0 * TAU;
            let dir = Vec3::new(0.0, a.cos(), a.sin());
            wood.merge(&beam(
                c + dir * 0.08,
                c + dir * (wheel_r - 0.06),
                0.03,
                0.03,
                0.006,
            ));
        }
    }
    // Axle and a resting leg.
    iron.merge(&tube(
        Vec3::new(-bw * 0.5 - 0.2, wheel_r, -0.1),
        Vec3::new(bw * 0.5 + 0.2, wheel_r, -0.1),
        0.025,
        10,
    ));
    wood.merge(&beam(
        Vec3::new(0.0, 0.0, bl * 0.5 + 0.45),
        Vec3::new(0.0, wheel_r - 0.05, bl * 0.5 + 0.4),
        0.06,
        0.06,
        0.01,
    ));
    // Load: sacks and a small barrel.
    for (k, (x, z, s)) in [
        (-0.25f32, -0.4f32, 0.2f32),
        (0.25, -0.35, 0.19),
        (0.0, 0.2, 0.2),
    ]
    .into_iter()
    .enumerate()
    {
        burlap.merge(&moved(
            sack_mesh(Vec3::new(s, s * 1.1, s), 900 + k as u32),
            bed * Mat4::from_translation(Vec3::new(x, 0.025 + s * 0.6, z)),
        ));
    }
    let (bw_mesh, bi) = barrel_mesh(0.6, 0.2);
    let lay = bed
        * Mat4::from_rotation_translation(
            Quat::from_rotation_z(FRAC_PI_2),
            Vec3::new(0.3, 0.23, 0.55),
        );
    wood.merge(&tint(moved(bw_mesh, lay), Vec4::new(1.0, 0.82, 0.68, 1.0)));
    iron.merge(&moved(bi, lay));
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    spawn(world, wood, mats.wood, t);
    spawn(world, iron, mats.iron, t);
    spawn(world, wicker, mats.wicker, t);
    spawn(world, burlap, mats.burlap, t);
}

/// A stone well with a timber roof, a windlass and a bucket.
fn well(world: &mut World, mats: &Mats, at: DVec3, yaw: f32) {
    let mut stone = Mesh::default();
    let (r_out, r_in, h) = (0.85f32, 0.62f32, 0.75f32);
    // The wall in courses of rough blocks.
    let mut rng = Rng::new(91);
    let courses = 4;
    for c in 0..courses {
        let n = 14;
        let y0 = c as f32 * h / courses as f32;
        let ch = h / courses as f32;
        for k in 0..n {
            let a0 = (k as f32 + 0.5 * (c % 2) as f32) / n as f32 * TAU;
            let a = a0 + 0.5 / n as f32 * TAU;
            let rr = (r_out + r_in) * 0.5;
            let len = rr * TAU / n as f32 - 0.015;
            let m = Mat4::from_rotation_translation(
                Quat::from_rotation_y(-a),
                Vec3::new(a.cos() * rr, y0 + ch * 0.5, a.sin() * rr),
            );
            stone.merge(&tint(
                moved(
                    bevel_box(
                        Vec3::new((r_out - r_in) * rng.range(0.9, 1.0), ch - 0.012, len),
                        0.025,
                        2,
                    ),
                    m,
                ),
                Vec4::splat(rng.range(0.8, 1.05)).with_w(1.0),
            ));
        }
    }
    // A capstone ring.
    stone.merge(&lathe(
        &[
            (r_in - 0.02, h),
            (r_out + 0.04, h),
            (r_out + 0.05, h + 0.06),
            (r_out + 0.02, h + 0.1),
            (r_in - 0.02, h + 0.1),
            (r_in - 0.02, h),
        ],
        36,
        r_out,
    ));
    // Dark water far down.
    let mut water = Mesh::default();
    water.merge(&lathe(&[(0.0, h - 0.6), (r_in, h - 0.6)], 24, r_in));
    let mut wood = Mesh::default();
    let mut iron = Mesh::default();
    let post_h = 2.3f32;
    for sx in [-1.0f32, 1.0] {
        wood.merge(&beam(
            Vec3::new(sx * (r_out - 0.05), h, 0.0),
            Vec3::new(sx * (r_out - 0.05), post_h, 0.0),
            0.1,
            0.1,
            0.015,
        ));
    }
    // A small pitched roof.
    for sz in [-1.0f32, 1.0] {
        let roof = Mat4::from_rotation_translation(
            Quat::from_rotation_x(sz * 0.62),
            Vec3::new(0.0, post_h + 0.22, sz * 0.36),
        );
        for k in 0..7 {
            let x = -0.95 + k as f32 * 0.3166;
            wood.merge(&tint(
                moved(
                    bevel_box(Vec3::new(0.3, 0.025, 0.92), 0.006, 2),
                    roof * Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
                ),
                Vec4::splat(rng.range(0.7, 0.95)).with_w(1.0),
            ));
        }
    }
    wood.merge(&moved(
        bevel_box(Vec3::new(2.0, 0.08, 0.08), 0.012, 0),
        Mat4::from_translation(Vec3::new(0.0, post_h + 0.5, 0.0)),
    ));
    // Windlass, rope and bucket.
    wood.merge(&tube(
        Vec3::new(-r_out + 0.05, post_h - 0.45, 0.0),
        Vec3::new(r_out - 0.05, post_h - 0.45, 0.0),
        0.06,
        14,
    ));
    iron.merge(&tube(
        Vec3::new(r_out - 0.05, post_h - 0.45, 0.0),
        Vec3::new(r_out + 0.15, post_h - 0.45, 0.0),
        0.012,
        6,
    ));
    iron.merge(&tube(
        Vec3::new(r_out + 0.15, post_h - 0.45, 0.0),
        Vec3::new(r_out + 0.15, post_h - 0.7, 0.0),
        0.012,
        6,
    ));
    let mut rope = Mesh::default();
    rope.merge(&tube(
        Vec3::new(0.1, post_h - 0.45, 0.06),
        Vec3::new(0.1, h + 0.55, 0.06),
        0.01,
        6,
    ));
    let bucket_at = Vec3::new(r_out + 0.25, h + 0.1, 0.35);
    wood.merge(&moved(
        lathe(
            &[
                (0.0, 0.0),
                (0.13, 0.0),
                (0.16, 0.28),
                (0.145, 0.28),
                (0.12, 0.02),
                (0.0, 0.02),
            ],
            20,
            0.14,
        ),
        Mat4::from_translation(bucket_at),
    ));
    for y in [0.06f32, 0.22] {
        iron.merge(&moved(
            lathe(
                &[
                    (0.135 + y * 0.1, y - 0.012),
                    (0.14 + y * 0.1, y),
                    (0.135 + y * 0.1, y + 0.012),
                ],
                20,
                0.14,
            ),
            Mat4::from_translation(bucket_at),
        ));
    }
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    spawn(world, stone, mats.stone, t);
    spawn(world, water, mats.iron, t);
    spawn(world, wood, mats.wood, t);
    spawn(world, iron, mats.iron, t);
    spawn(world, rope, mats.burlap, t);
}

/// A plank bench on two splayed legs.
fn bench(world: &mut World, mats: &Mats, at: DVec3, yaw: f32) {
    let mut wood = Mesh::default();
    let (len, seat_y) = (1.9f32, 0.46f32);
    wood.merge(&moved(
        bevel_box(Vec3::new(len, 0.06, 0.34), 0.012, 0),
        Mat4::from_translation(Vec3::Y * seat_y),
    ));
    for sx in [-1.0f32, 1.0] {
        for sz in [-1.0f32, 1.0] {
            wood.merge(&beam(
                Vec3::new(sx * (len * 0.5 - 0.25), 0.0, sz * 0.2),
                Vec3::new(sx * (len * 0.5 - 0.3), seat_y - 0.02, sz * 0.1),
                0.06,
                0.06,
                0.01,
            ));
        }
    }
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    spawn(world, wood, mats.wood, t);
}

/// A few crates, a barrel and sacks stacked on the ground.
fn clutter(world: &mut World, mats: &Mats, at: DVec3, rng: &mut Rng) {
    let mut wood = Mesh::default();
    let mut iron = Mesh::default();
    let mut burlap = Mesh::default();
    let yaw0 = rng.range(0.0, TAU);
    let s0 = Vec3::new(0.65, 0.5, 0.55);
    wood.merge(&moved(
        super::art::props::crate_mesh(s0, rng),
        Mat4::from_rotation_translation(Quat::from_rotation_y(yaw0), Vec3::Y * (s0.y * 0.5)),
    ));
    if rng.chance(0.6) {
        let s1 = Vec3::new(0.5, 0.4, 0.45);
        wood.merge(&moved(
            super::art::props::crate_mesh(s1, rng),
            Mat4::from_rotation_translation(
                Quat::from_rotation_y(yaw0 + rng.range(-0.4, 0.4)),
                Vec3::new(0.0, s0.y + s1.y * 0.5, 0.0),
            ),
        ));
    }
    let (bw, bi) = barrel_mesh(0.9, 0.3);
    let bp = Vec3::new(0.75, 0.0, 0.2);
    wood.merge(&tint(
        moved(bw, Mat4::from_translation(bp)),
        Vec4::new(1.0, 0.82, 0.68, 1.0),
    ));
    iron.merge(&moved(bi, Mat4::from_translation(bp)));
    for k in 0..rng.range_i32(1, 4) {
        let s = rng.range(0.22, 0.3);
        let a = rng.range(0.0, TAU);
        burlap.merge(&moved(
            sack_mesh(Vec3::new(s, s * 1.15, s), rng.next_u32() % 1000 + k as u32),
            Mat4::from_translation(Vec3::new(
                -0.7 + a.cos() * 0.25,
                s * 0.7,
                0.3 + a.sin() * 0.25,
            )),
        ));
    }
    let t = Transform::from_translation(at);
    spawn(world, wood, mats.wood, t);
    spawn(world, iron, mats.iron, t);
    spawn(world, burlap, mats.burlap, t);
}
