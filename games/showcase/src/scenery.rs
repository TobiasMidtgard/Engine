//! The valley's scenery: terrain, woods, meadows of grass and wildflowers, boulders, a village
//! around the plaza with street lamps. Zones get their content from `zones.rs`; this is
//! everything between and around them.
//!
//! Far things switch to cheaper meshes or hide (see [`Lod`]), so the whole valley (tens of
//! thousands of plants) stays fast.

use crate::landscape::{Cover, Crop, Landscape, field_corners};
use crate::meshgen::{self, TreeKind};
use crate::texgen::VillageMats;
use crate::village::{self, Part, PropKind, PropSpot};
use crate::zones::{ZONES, zone};
use engine::prelude::*;

/// Materials shared by the scenery (vertex colors carry the variation).
pub struct Mats {
    pub terrain: Handle<StandardMaterial>,
    pub bark: Handle<StandardMaterial>,
    pub leaves: Handle<StandardMaterial>,
    pub rock: Handle<StandardMaterial>,
    pub grass: Handle<StandardMaterial>,
    pub flowers: Handle<StandardMaterial>,
    /// The village's procedural PBR materials (plaster, timber, tiles, glass, iron, ...).
    pub village: VillageMats,
}

pub struct TreeHandles {
    pub kind: TreeKind,
    pub trunk: Handle<Mesh>,
    pub crown: Handle<Mesh>,
    pub far: Handle<Mesh>,
}

/// One house archetype: a mesh per material, near and far versions.
pub struct HouseHandles {
    pub near: Vec<(Part, Handle<Mesh>)>,
    pub far: Vec<(Part, Handle<Mesh>)>,
    /// Footprint half extents (x, z) including the roof overhang.
    pub half: Vec2,
    pub props: Vec<PropSpot>,
}

/// A prop: one mesh per material.
pub type PropHandles = Vec<(Part, Handle<Mesh>)>;

/// Street lamp meshes: (iron, glass), near and far.
pub struct LampHandles {
    pub near: (Handle<Mesh>, Handle<Mesh>),
    pub far: (Handle<Mesh>, Handle<Mesh>),
}

/// Street furniture variants.
pub struct Props {
    pub bench: PropHandles,
    pub barrels: Vec<PropHandles>,
    pub wood_piles: Vec<PropHandles>,
    pub cart: PropHandles,
}

/// Every procedural mesh and material, built once at startup.
pub struct Library {
    pub mats: Mats,
    pub trees: Vec<TreeHandles>,
    pub rocks: Vec<Handle<Mesh>>,
    pub small_rocks: Vec<Handle<Mesh>>,
    /// Grass patches up close (curved blades) and their sparse single-triangle stand-ins for
    /// mid distance, by the same index.
    pub grass: Vec<Handle<Mesh>>,
    pub grass_mid: Vec<Handle<Mesh>>,
    pub dry_grass: Vec<Handle<Mesh>>,
    pub dry_grass_mid: Vec<Handle<Mesh>>,
    pub flowers: Vec<Handle<Mesh>>,
    pub houses: Vec<HouseHandles>,
    pub lamp: LampHandles,
    pub props: Props,
}

impl Library {
    pub fn trees_of(&self, kind: TreeKind) -> impl Iterator<Item = &TreeHandles> {
        self.trees.iter().filter(move |t| t.kind == kind)
    }
}

pub fn build_library(
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Library {
    let white = |r: f32| StandardMaterial::hex(0xffffff).with_roughness(r);
    let (detail, detail_normal) = meshgen::ground_detail(512);
    let mut terrain = white(0.95)
        .with_base_color_texture(images.add(detail))
        .with_normal_texture(images.add(detail_normal));
    // The detail map averages about 0.85; lift the base so the vertex colors keep their value.
    terrain.base_color = [1.18, 1.18, 1.18, 1.0];
    terrain.normal_scale = 0.7;
    let mut leaves = white(0.75).with_subsurface(0.5).with_wind_sway(0.12);
    leaves.reflectance = 0.35;
    let mut grass = white(0.8).with_subsurface(0.6).with_wind_sway(0.18);
    grass.double_sided = true;
    let mut flowers = white(0.6).with_subsurface(0.4).with_wind_sway(0.15);
    flowers.double_sided = true;
    // The village's meshes build on a worker while its textures generate.
    let t0 = std::time::Instant::now();
    let (village_meshes, village_mats) = std::thread::scope(|s| {
        let job = s.spawn(village::build_meshes);
        let mats = crate::texgen::village_materials(materials, images);
        (job.join().expect("village mesh thread"), mats)
    });
    let mats = Mats {
        terrain: materials.add(terrain),
        bark: materials.add(white(0.9)),
        leaves: materials.add(leaves),
        rock: materials.add(white(0.85)),
        grass: materials.add(grass),
        flowers: materials.add(flowers),
        village: village_mats,
    };

    let mut trees = Vec::new();
    for (kind, variants) in [
        (TreeKind::Oak, 5),
        (TreeKind::Spruce, 5),
        (TreeKind::Birch, 3),
        (TreeKind::Bush, 3),
    ] {
        for v in 0..variants {
            let seed = 1000 + kind as u64 * 100 + v;
            let t = meshgen::tree(kind, seed);
            trees.push(TreeHandles {
                kind,
                trunk: meshes.add(t.trunk),
                crown: meshes.add(t.crown),
                far: meshes.add(meshgen::tree_far(kind, seed)),
            });
        }
    }
    let rocks = (0..6)
        .map(|i| meshes.add(meshgen::rock(500 + i, 3)))
        .collect();
    let small_rocks = (0..6)
        .map(|i| meshes.add(meshgen::rock(600 + i, 1)))
        .collect();
    let mut grass_set = |seed: u64, dry: f32| {
        (
            meshes.add(meshgen::grass_patch(seed, 4.0, 1300, dry, 3)),
            meshes.add(meshgen::grass_patch(seed, 4.0, 420, dry, 1)),
        )
    };
    let (grass, grass_mid): (Vec<_>, Vec<_>) = (0..6)
        .map(|i| grass_set(700 + i, 0.15 + i as f32 * 0.05))
        .unzip();
    let (dry_grass, dry_grass_mid): (Vec<_>, Vec<_>) =
        (0..4).map(|i| grass_set(750 + i, 0.75)).unzip();
    let flower_colors = [
        Vec3::new(0.9, 0.75, 0.1),
        Vec3::new(0.85, 0.85, 0.85),
        Vec3::new(0.45, 0.2, 0.7),
        Vec3::new(0.8, 0.1, 0.06),
    ];
    let flowers = flower_colors
        .iter()
        .enumerate()
        .map(|(i, c)| meshes.add(meshgen::flower_patch(800 + i as u64, 4.0, 24, *c)))
        .collect();
    let village::VillageMeshes {
        houses,
        lamp,
        bench,
        barrels,
        wood_piles,
        cart,
    } = village_meshes;
    let mut add_parts = |parts: Vec<(Part, Mesh)>| -> PropHandles {
        parts.into_iter().map(|(p, m)| (p, meshes.add(m))).collect()
    };
    let tris = |far: bool| {
        houses
            .iter()
            .map(|h| if far { &h.1 } else { &h.0 }.triangles().to_string())
            .collect::<Vec<_>>()
            .join("/")
    };
    let (near_tris, far_tris) = (tris(false), tris(true));
    let houses: Vec<HouseHandles> = houses
        .into_iter()
        .map(|(near, far)| HouseHandles {
            half: near.half,
            props: near.props,
            near: add_parts(near.parts),
            far: add_parts(far.parts),
        })
        .collect();
    let props = Props {
        bench: add_parts(bench.parts),
        barrels: barrels.into_iter().map(|b| add_parts(b.parts)).collect(),
        wood_piles: wood_piles.into_iter().map(|b| add_parts(b.parts)).collect(),
        cart: add_parts(cart.parts),
    };
    let lamp = LampHandles {
        near: (meshes.add(lamp.0.0), meshes.add(lamp.0.1)),
        far: (meshes.add(lamp.1.0), meshes.add(lamp.1.1)),
    };
    engine::core::log::info!(
        "village: {} house types ({} triangles near, {} far), built in {:.2} s",
        houses.len(),
        near_tris,
        far_tris,
        t0.elapsed().as_secs_f32()
    );
    Library {
        mats,
        trees,
        rocks,
        small_rocks,
        grass,
        grass_mid,
        dry_grass,
        dry_grass_mid,
        flowers,
        houses,
        lamp,
        props,
    }
}

/// Distance-based detail: beyond `switch` meters from the camera the entity shows `far` (or
/// hides when there is none), and beyond `hide` it hides either way.
#[derive(Component, Debug, Clone, Copy)]
pub struct Lod {
    pub near: Handle<Mesh>,
    pub far: Option<Handle<Mesh>>,
    pub switch: f32,
    pub hide: f32,
    pub far_shown: bool,
}

impl Lod {
    pub fn hide_beyond(mesh: Handle<Mesh>, distance: f32) -> Self {
        Self {
            near: mesh,
            far: None,
            switch: distance,
            hide: f32::INFINITY,
            far_shown: false,
        }
    }

    /// `near` up close, `far` from `switch` meters, nothing beyond `hide`.
    pub fn swap_then_hide(near: Handle<Mesh>, far: Handle<Mesh>, switch: f32, hide: f32) -> Self {
        Self {
            near,
            far: Some(far),
            switch,
            hide,
            far_shown: false,
        }
    }
}

/// Swap meshes or hide entities by distance to the active camera.
pub fn update_lods(
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut q: Query<(&GlobalTransform, &mut Lod, &mut MeshRenderer)>,
) {
    let Some(eye) = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    else {
        return;
    };
    q.par_iter_mut().for_each(|(t, mut lod, mut renderer)| {
        let d2 = (t.translation() - eye).length_squared();
        let far = d2 > (lod.switch as f64).powi(2);
        let shown = d2 <= (lod.hide as f64).powi(2) && (!far || lod.far.is_some());
        if far == lod.far_shown && renderer.visible == shown {
            return;
        }
        lod.far_shown = far;
        renderer.visible = shown;
        match (far, lod.far) {
            (false, _) => renderer.mesh = MeshRef::Handle(lod.near),
            (true, Some(f)) => renderer.mesh = MeshRef::Handle(f),
            (true, None) => {}
        }
    });
}

fn place(land: &Landscape, x: f64, z: f64, sink: f64) -> DVec3 {
    DVec3::new(x, land.height(x, z) - sink, z)
}

/// The terrain mesh chunks.
pub fn spawn_terrain(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    lib: &Library,
    land: &Landscape,
) {
    for (origin, mesh) in crate::landscape::build_terrain(land) {
        commands.spawn((
            Transform::from_translation(origin),
            MeshRenderer::new(meshes.add(mesh), lib.mats.terrain),
        ));
    }
}

/// One tree with near/far detail.
pub fn spawn_tree(
    commands: &mut Commands,
    lib: &Library,
    t: &TreeHandles,
    pos: DVec3,
    yaw: f32,
    scale: f32,
) {
    let tf = Transform::from_translation(pos)
        .with_rotation(Quat::from_rotation_y(yaw))
        .with_scale(Vec3::splat(scale));
    let switch = 220.0 * scale;
    if t.kind != TreeKind::Bush {
        commands.spawn((
            tf,
            MeshRenderer::new(t.trunk, lib.mats.bark),
            Lod::hide_beyond(t.trunk, switch),
        ));
    }
    commands.spawn((
        tf,
        MeshRenderer::new(t.crown, lib.mats.leaves),
        Lod {
            near: t.crown,
            far: Some(t.far),
            switch,
            hide: f32::INFINITY,
            far_shown: false,
        },
    ));
}

/// Pick a tree variant of `kind` with the rng.
pub fn pick_tree<'a>(lib: &'a Library, kind: TreeKind, rng: &mut Rng) -> &'a TreeHandles {
    let options: Vec<&TreeHandles> = lib.trees_of(kind).collect();
    options[rng.range_i32(0, options.len() as i32) as usize]
}

/// Inside any zone's pad (zones build their own content there).
fn in_zone(x: f64, z: f64, margin: f64) -> bool {
    ZONES.iter().any(|zn| {
        let d = DVec2::new(x - zn.center.x, z - zn.center.z).length();
        d < zn.radius + margin
    })
}

/// Inside a zone whose ground is paved, built over or its own (no scenery grass there).
fn in_hard_zone(x: f64, z: f64) -> bool {
    ZONES.iter().any(|zn| {
        let d = DVec2::new(x - zn.center.x, z - zn.center.z).length();
        d < zn.radius - 2.0 && !crate::zones::GRASSY_ZONES.contains(&zn.key)
    }) || crate::zones::BARE_FOOTPRINTS.iter().any(|&(key, rx, rz)| {
        // A grass patch reaches about 2 m from its center.
        let c = crate::zones::zone(key).center;
        (rx[0] - 2.0..rx[1] + 2.0).contains(&(x - c.x))
            && (rz[0] - 2.0..rz[1] + 2.0).contains(&(z - c.z))
    })
}

/// Woods on the hills and lone trees in the meadows.
pub fn spawn_woods(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    let spacing = 7.5;
    let extent = 1300.0;
    let n = (extent * 2.0 / spacing) as i32;
    let mut count = 0;
    for i in 0..n {
        for j in 0..n {
            let x = -extent + (i as f64 + rng.range(0.1, 0.9) as f64) * spacing;
            let z = -extent + (j as f64 + rng.range(0.1, 0.9) as f64) * spacing;
            let r = (x * x + z * z).sqrt();
            if r > extent || in_zone(x, z, 12.0) {
                continue;
            }
            let wood = land.woodland(x, z);
            let p = wood * wood + 0.012;
            if rng.f32() as f64 > p {
                continue;
            }
            if land.road_weight(x, z) > 0.0 || land.pad_weight(x, z) > 0.3 {
                continue;
            }
            let h = land.height(x, z);
            let n = land.normal(x, z, 2.0);
            if n.y < 0.8 || land.lake_depth(x, z) > -1.5 || h > 700.0 {
                continue;
            }
            // Conifers higher up and deep in the woods, broadleaves and birches lower down.
            let high = ((h - 60.0) / 300.0).clamp(0.0, 1.0) as f32;
            let roll = rng.f32();
            let kind = if roll < 0.35 + 0.5 * high + 0.2 * wood as f32 {
                TreeKind::Spruce
            } else if roll < 0.85 {
                TreeKind::Oak
            } else {
                TreeKind::Birch
            };
            let t = pick_tree(lib, kind, rng);
            let scale = rng.range(0.75, 1.25);
            let at = place(land, x, z, 0.1);
            spawn_tree(commands, lib, t, at, rng.range(0.0, TAU), scale);
            if near_wildlife(x, z) {
                solid(commands, at, 0.35 * scale, 5.0 * scale);
            }
            count += 1;
            // Bushes along the wood edges.
            if wood < 0.6 && rng.chance(0.35) {
                let a = rng.range(0.0, TAU) as f64;
                let (bx, bz) = (x + a.cos() * 3.5, z + a.sin() * 3.5);
                if land.road_weight(bx, bz) == 0.0 && !in_zone(bx, bz, 4.0) {
                    let b = pick_tree(lib, TreeKind::Bush, rng);
                    spawn_tree(
                        commands,
                        lib,
                        b,
                        place(land, bx, bz, 0.2),
                        rng.range(0.0, TAU),
                        rng.range(0.7, 1.3),
                    );
                }
            }
        }
    }
    engine::core::log::info!("scenery: {count} trees");
}

/// Grass and wildflower patches near the middle of the valley (hidden beyond ~90 m).
pub fn spawn_meadows(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    let spacing = 3.6;
    let extent = 520.0;
    let n = (extent * 2.0 / spacing) as i32;
    for i in 0..n {
        for j in 0..n {
            let x = -extent + (i as f64 + rng.range(0.2, 0.8) as f64) * spacing;
            let z = -extent + (j as f64 + rng.range(0.2, 0.8) as f64) * spacing;
            if x * x + z * z > extent * extent || in_hard_zone(x, z) {
                continue;
            }
            if land.road_weight(x, z) > 0.05 || land.lake_depth(x, z) > -0.3 {
                continue;
            }
            let h = land.height(x, z);
            let nrm = land.normal(x, z, 1.5);
            let cover = land.cover(x, z, h, nrm);
            if !matches!(cover, Cover::Grass | Cover::Forest) {
                continue;
            }
            if land
                .field_at(x, z)
                .is_some_and(|(crop, edge)| crop != Crop::Pasture && edge > 2.0)
            {
                continue;
            }
            let forest = cover == Cover::Forest;
            if forest && rng.chance(0.6) {
                continue;
            }
            let tilt = Quat::from_rotation_arc(Vec3::Y, nrm);
            let tf = Transform::from_translation(DVec3::new(x, h - 0.03, z))
                .with_rotation(tilt * Quat::from_rotation_y(rng.range(0.0, TAU)))
                .with_scale(Vec3::new(1.0, rng.range(0.8, 1.3), 1.0));
            let k = rng.range_i32(0, lib.grass.len() as i32) as usize;
            commands.spawn((
                tf,
                MeshRenderer::new(lib.grass[k], lib.mats.grass).without_shadows(),
                // Full blades up close; thin out gradually instead of ending in a hard line.
                Lod::swap_then_hide(
                    lib.grass[k],
                    lib.grass_mid[k],
                    rng.range(20.0, 26.0),
                    rng.range(45.0, 95.0),
                ),
            ));
            // Wildflowers in drifts.
            let drift = engine::core::math::noise::perlin2(x / 40.0, z / 40.0, 77);
            if !forest && drift > 0.15 && rng.chance(0.6) {
                let k = ((drift * 7.0) as usize + i as usize / 40) % lib.flowers.len();
                let mesh = lib.flowers[k];
                commands.spawn((
                    tf,
                    MeshRenderer::new(mesh, lib.mats.flowers).without_shadows(),
                    Lod::hide_beyond(mesh, 70.0),
                ));
            }
        }
    }
}

/// Boulders on the hills and in the woods, smaller stones along the roads.
pub fn spawn_rocks(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    for _ in 0..2600 {
        let p = rng.in_unit_disk() * 1400.0;
        let (x, z) = (p.x as f64, p.y as f64);
        if in_zone(x, z, 5.0) || land.road_weight(x, z) > 0.0 {
            continue;
        }
        let h = land.height(x, z);
        let n = land.normal(x, z, 2.0);
        let rocky = (1.0 - n.y) * 6.0 + land.woodland(x, z) as f32 * 0.3;
        if rng.f32() > rocky + 0.08 {
            continue;
        }
        let big = rng.chance(0.4);
        let s = if big {
            rng.range(1.2, 3.5)
        } else {
            rng.range(0.3, 0.9)
        };
        let mesh = if big {
            lib.rocks[rng.range_i32(0, lib.rocks.len() as i32) as usize]
        } else {
            lib.small_rocks[rng.range_i32(0, lib.small_rocks.len() as i32) as usize]
        };
        let tf = Transform::from_translation(DVec3::new(x, h - s as f64 * 0.25, z))
            .with_rotation(
                Quat::from_rotation_arc(Vec3::Y, n) * Quat::from_rotation_y(rng.range(0.0, TAU)),
            )
            .with_scale(Vec3::splat(s));
        commands.spawn((
            tf,
            MeshRenderer::new(mesh, lib.mats.rock),
            Lod::hide_beyond(mesh, if big { 900.0 } else { 160.0 }),
        ));
        if big && near_wildlife(x, z) {
            solid(commands, DVec3::new(x, h, z), s * 0.8, s * 1.2);
        }
    }
}

/// Where the wildlife roams, trunks and boulders get colliders so animals path around them.
fn near_wildlife(x: f64, z: f64) -> bool {
    let m = crate::zones::zone("meadow");
    DVec2::new(x - m.center.x, z - m.center.z).length() < m.radius + 120.0
}

/// A static upright obstacle (a trunk or a boulder) for physics and pathfinding.
fn solid(commands: &mut Commands, base: DVec3, radius: f32, height: f32) {
    let half = (height * 0.5 - radius).max(0.0);
    commands.spawn((
        Transform::from_translation(base + DVec3::new(0.0, height as f64 * 0.5, 0.0)),
        engine::world::physics::Collider::capsule(radius, half),
    ));
}

/// The cliff south of the lake faced with boulders, and stones along the stream's banks.
pub fn spawn_cliffs_and_stream(
    commands: &mut Commands,
    lib: &Library,
    land: &Landscape,
    rng: &mut Rng,
) {
    let boulder = |commands: &mut Commands, rng: &mut Rng, p: DVec3, s: f32| {
        let mesh = lib.rocks[rng.range_i32(0, lib.rocks.len() as i32) as usize];
        commands.spawn((
            Transform::from_translation(p)
                .with_rotation(
                    Quat::from_rotation_y(rng.range(0.0, TAU))
                        * Quat::from_rotation_x(rng.range(-0.5, 0.5)),
                )
                .with_scale(Vec3::new(s, s * rng.range(0.7, 1.1), s)),
            MeshRenderer::new(mesh, lib.mats.rock),
            Lod::hide_beyond(mesh, 1200.0),
        ));
    };
    for sc in &land.scarps {
        let d = (sc.b - sc.a).normalize();
        let n = DVec2::new(-d.y, d.x);
        let len = (sc.b - sc.a).length();
        let mut along = 12.0;
        while along < len - 12.0 {
            // The face sits where the wobbling line puts it.
            let (_, side) = sc.frame(sc.a + d * along, land.seed);
            let foot = sc.a + d * along - n * (side + 1.0);
            let near_water = land
                .stream_at(foot.x, foot.y)
                .is_some_and(|(dist, _)| dist < 7.0);
            if !near_water {
                let low = land.height(foot.x, foot.y);
                let top = land.height(foot.x + n.x * 4.0, foot.y + n.y * 4.0);
                for k in 0..3 {
                    let y = low + (top - low) * (k as f64 * 0.38) - 0.5;
                    let jitter = n * rng.range(-0.8, 1.2) as f64;
                    let q = foot + jitter;
                    let size = rng.range(1.8, 3.4);
                    boulder(commands, rng, DVec3::new(q.x, y, q.y), size);
                }
            }
            along += rng.range(3.0, 4.5) as f64;
        }
    }
    for st in &land.streams {
        for (i, p) in st.path.iter().enumerate().step_by(5) {
            if !rng.chance(0.6) || i + 1 >= st.path.len() {
                continue;
            }
            let dir = (st.path[i + 1] - *p).normalize_or(DVec2::Y);
            let side = DVec2::new(-dir.y, dir.x) * if rng.chance(0.5) { 1.0 } else { -1.0 };
            let q = *p + side * (st.width * 0.5 + rng.range(0.3, 1.5) as f64);
            let s = rng.range(0.5, 1.4);
            let mesh = lib.small_rocks[rng.range_i32(0, lib.small_rocks.len() as i32) as usize];
            commands.spawn((
                Transform::from_translation(DVec3::new(q.x, land.height(q.x, q.y) - 0.2, q.y))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
                    .with_scale(Vec3::splat(s)),
                MeshRenderer::new(mesh, lib.mats.rock),
                Lod::hide_beyond(mesh, 220.0),
            ));
        }
        // Big boulders flanking the falls.
        if let Some((top, _)) = st.falls() {
            let lip = st.path[top];
            let dir = (st.path[top + 1] - lip).normalize_or(DVec2::Y);
            let side = DVec2::new(-dir.y, dir.x);
            for s in [-1.0, 1.0] {
                for k in 0..3 {
                    let q = lip
                        + side * s * (st.width * 0.5 + 1.8 + k as f64 * 1.6)
                        + dir * (k as f64 * 1.2 - 1.0);
                    let y = land.height(q.x, q.y) - 1.0;
                    let size = rng.range(2.0, 3.0);
                    boulder(commands, rng, DVec3::new(q.x, y, q.y), size);
                }
            }
        }
    }
}

/// Street furniture hides beyond this distance (meters).
const PROP_HIDE: f32 = 75.0;

/// Distance (meters) beyond which a house part switches to the archetype's far mesh, or hides
/// when it has none: single tiles and stones go first (their relief is under a pixel by then),
/// the framing and windows last.
fn house_lod_distance(part: Part) -> f32 {
    match part {
        Part::Roof | Part::Stone => 80.0,
        Part::Iron => 50.0,
        Part::Plants => 60.0,
        _ => 140.0,
    }
}

/// One house's looks: material variants (indices into the village materials) and mirroring.
#[derive(Clone, Copy, Debug)]
pub struct HouseLook {
    pub plaster: usize,
    pub timber: usize,
    pub slate: bool,
    pub roof: usize,
    pub mirror: bool,
}

impl HouseLook {
    pub fn random(rng: &mut Rng) -> Self {
        Self {
            plaster: rng.range_i32(0, 64) as usize,
            timber: rng.range_i32(0, 64) as usize,
            slate: rng.chance(0.3),
            roof: rng.range_i32(0, 64) as usize,
            mirror: rng.chance(0.5),
        }
    }
}

fn village_material(lib: &Library, part: Part, look: &HouseLook) -> Handle<StandardMaterial> {
    let vm = &lib.mats.village;
    let pick = |v: &Vec<Handle<StandardMaterial>>, i: usize| v[i % v.len()];
    match part {
        Part::Plaster => pick(&vm.plaster, look.plaster),
        Part::Timber => pick(&vm.timber, look.timber),
        Part::Paint => vm.paint,
        Part::Stone => vm.stone,
        Part::Brick => vm.brick,
        Part::Roof if look.slate => pick(&vm.roof_slate, look.roof),
        Part::Roof => pick(&vm.roof_clay, look.roof),
        Part::Glass => vm.glass,
        Part::Iron => vm.iron,
        Part::Plants => vm.plants,
    }
}

/// The transform of a house with its door facing `facing` (radians around Y; 0 = +Z).
fn house_transform(pos: DVec3, facing: f32, look: &HouseLook) -> Transform {
    let mirror = if look.mirror { -1.0 } else { 1.0 };
    Transform::from_translation(pos)
        .with_rotation(Quat::from_rotation_y(facing))
        .with_scale(Vec3::new(mirror, 1.0, 1.0))
}

/// A house with its door facing `facing` (radians around Y; 0 = +Z): one entity per material,
/// each switching to the archetype's far mesh (or hiding) with distance.
pub fn spawn_house(
    commands: &mut Commands,
    lib: &Library,
    h: &HouseHandles,
    pos: DVec3,
    facing: f32,
    look: &HouseLook,
) {
    let tf = house_transform(pos, facing, look);
    for (part, near) in &h.near {
        let far = h.far.iter().find(|(p, _)| p == part).map(|(_, m)| *m);
        let mut renderer = MeshRenderer::new(*near, village_material(lib, *part, look));
        if *part == Part::Plants {
            renderer = renderer.without_shadows();
        }
        commands.spawn((
            tf,
            renderer,
            Lod {
                near: *near,
                far,
                switch: house_lod_distance(*part),
                hide: f32::INFINITY,
                far_shown: false,
            },
        ));
    }
}

/// A prop (bench, barrels, wood pile, cart) whose front faces `yaw` (radians around Y).
fn spawn_prop(
    commands: &mut Commands,
    lib: &Library,
    prop: &PropHandles,
    pos: DVec3,
    yaw: f32,
    timber: usize,
) {
    let tf = Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw));
    let look = HouseLook {
        plaster: 0,
        timber,
        slate: false,
        roof: 0,
        mirror: false,
    };
    for (part, mesh) in prop {
        commands.spawn((
            tf,
            MeshRenderer::new(*mesh, village_material(lib, *part, &look)),
            Lod::hide_beyond(*mesh, PROP_HIDE),
        ));
    }
}

/// A street lamp with a warm point light (lumens).
pub fn spawn_lamp(commands: &mut Commands, lib: &Library, pos: DVec3) {
    let tf = Transform::from_translation(pos);
    let vm = &lib.mats.village;
    let (near, far) = (lib.lamp.near, lib.lamp.far);
    commands.spawn((
        tf,
        MeshRenderer::new(near.0, vm.iron),
        Lod {
            near: near.0,
            far: Some(far.0),
            switch: 60.0,
            hide: f32::INFINITY,
            far_shown: false,
        },
    ));
    commands.spawn((
        tf,
        MeshRenderer::new(near.1, vm.lantern).without_shadows(),
        Lod {
            near: near.1,
            far: Some(far.1),
            switch: 60.0,
            hide: f32::INFINITY,
            far_shown: false,
        },
    ));
    commands.spawn((
        Transform::from_translation(pos + DVec3::new(0.0, 3.86, 0.0)),
        PointLight {
            color: [1.0, 0.72, 0.45],
            intensity_lm: 1800.0,
            range: 16.0,
            radius: 0.15,
        },
    ));
}

/// A house's footprint on the ground: center, unit axes (local X and Z) and half extents.
#[derive(Clone, Copy, Debug)]
struct Footprint {
    c: DVec2,
    ax: [DVec2; 2],
    half: DVec2,
}

impl Footprint {
    fn new(c: DVec2, facing: f32, half: Vec2) -> Self {
        let (s, co) = (facing as f64).sin_cos();
        // Local +X and +Z after turning by `facing` around Y.
        Self {
            c,
            ax: [DVec2::new(co, -s), DVec2::new(s, co)],
            half: half.as_dvec2(),
        }
    }

    fn corners(&self, grow: f64) -> [DVec2; 4] {
        let (x, z) = (
            self.ax[0] * (self.half.x + grow),
            self.ax[1] * (self.half.y + grow),
        );
        [
            self.c + x + z,
            self.c - x + z,
            self.c - x - z,
            self.c + x - z,
        ]
    }

    /// Separating-axis test with a margin (meters).
    fn overlaps(&self, o: &Footprint, margin: f64) -> bool {
        let a = self.corners(margin * 0.5);
        let b = o.corners(margin * 0.5);
        for axis in self.ax.iter().chain(o.ax.iter()) {
            let proj = |pts: &[DVec2; 4]| {
                pts.iter()
                    .map(|p| p.dot(*axis))
                    .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
            };
            let ((a0, a1), (b0, b1)) = (proj(&a), proj(&b));
            if a1 < b0 || b1 < a0 {
                return false;
            }
        }
        true
    }

    /// Distance from `p` to the footprint (0 inside).
    fn distance(&self, p: DVec2) -> f64 {
        let d = p - self.c;
        let q = DVec2::new(d.dot(self.ax[0]).abs(), d.dot(self.ax[1]).abs()) - self.half;
        q.max(DVec2::ZERO).length()
    }
}

/// The village: rows of houses around the plaza (doors toward it) with benches, barrels and
/// wood piles by their walls, carts at the plaza's edge, and street lamps.
pub fn spawn_village(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    let plaza = zone("plaza").center;
    let center = DVec2::new(plaza.x, plaza.z);
    let mut placed: Vec<Footprint> = Vec::new();
    let mut count = 0;
    let mut try_house =
        |commands: &mut Commands, rng: &mut Rng, k: usize, at: DVec2, facing: f32| {
            let h = &lib.houses[k];
            let fp = Footprint::new(at, facing, h.half);
            // Keep clear of roads, zones and other houses.
            let mut probe = fp.corners(1.0).to_vec();
            probe.extend(fp.corners(1.0).iter().map(|q| (*q + fp.c) * 0.5));
            probe.push(fp.c);
            let clear = probe.iter().all(|q| land.road_weight(q.x, q.y) == 0.0)
                && !ZONES
                    .iter()
                    .any(|zn| fp.distance(DVec2::new(zn.center.x, zn.center.z)) < zn.radius + 2.0)
                && placed.iter().all(|o| !fp.overlaps(o, 1.6));
            if !clear {
                return false;
            }
            // Sit on the lowest corner so no corner floats (the plinth goes down into the ground).
            let base = fp
                .corners(-0.5)
                .iter()
                .map(|q| land.height(q.x, q.y))
                .fold(f64::MAX, f64::min);
            let look = HouseLook::random(rng);
            let pos = DVec3::new(at.x, base, at.y);
            spawn_house(commands, lib, h, pos, facing, &look);
            // Props at some of the archetype's spots.
            let tf = house_transform(pos, facing, &look);
            let mut barrels_done = false;
            for spot in &h.props {
                let (prop, chance, timber) = match spot.kind {
                    PropKind::Bench => (&lib.props.bench, 0.6, 2),
                    PropKind::Barrels if !barrels_done => (
                        &lib.props.barrels
                            [rng.range_i32(0, lib.props.barrels.len() as i32) as usize],
                        0.3,
                        0,
                    ),
                    PropKind::Barrels => continue,
                    PropKind::WoodPile => (
                        &lib.props.wood_piles
                            [rng.range_i32(0, lib.props.wood_piles.len() as i32) as usize],
                        0.35,
                        2,
                    ),
                };
                if !rng.chance(chance) {
                    continue;
                }
                barrels_done |= spot.kind == PropKind::Barrels;
                let p = tf.translation + (tf.rotation * (spot.pos * tf.scale)).as_dvec3();
                let f = tf.rotation * (spot.facing * tf.scale);
                if land.road_weight(p.x, p.z) > 0.0 {
                    continue;
                }
                let y = land.height(p.x, p.z);
                spawn_prop(
                    commands,
                    lib,
                    prop,
                    DVec3::new(p.x, y, p.z),
                    f.x.atan2(f.z),
                    timber,
                );
            }
            placed.push(fp);
            count += 1;
            true
        };
    // Rows of houses along rings around the plaza, frontages a few meters apart.
    for (ring, gap) in [(55.0f64, (1.5f32, 4.0f32)), (76.0, (2.5, 7.0))] {
        let mut a = rng.range(0.0, 0.3) as f64;
        let end = a + std::f64::consts::TAU;
        let mut k = rng.range_i32(0, lib.houses.len() as i32) as usize;
        while a < end {
            let h = &lib.houses[k];
            let half_w = h.half.x as f64;
            // Center the house's frontage on the ring, its back half outward.
            let r = ring + h.half.y as f64;
            let at = center + DVec2::new(a.cos(), a.sin()) * r;
            let facing = (-(a.cos()) as f32).atan2(-(a.sin()) as f32);
            let placed_one = try_house(commands, rng, k, at, facing);
            let step = if placed_one {
                half_w * 2.0 + rng.range(gap.0, gap.1) as f64
            } else {
                3.0
            };
            a += step / ring;
            if placed_one {
                k = (k + 1 + rng.range_i32(0, 3) as usize) % lib.houses.len();
            }
        }
    }
    engine::core::log::info!("village: {count} houses");
    // Lamps around the plaza, and along the street in front of the first row.
    for i in 0..12 {
        let a = i as f64 / 12.0 * std::f64::consts::TAU + 0.13;
        let p = DVec3::new(a.cos() * 34.0, 0.0, a.sin() * 34.0);
        spawn_lamp(commands, lib, place(land, p.x, p.z, 0.0));
    }
    for i in 0..10 {
        let a = i as f64 / 10.0 * std::f64::consts::TAU + 0.4;
        let q = center + DVec2::new(a.cos(), a.sin()) * 51.5;
        let clear = land.road_weight(q.x, q.y) == 0.0
            && placed.iter().all(|fp| fp.distance(q) > 1.0)
            && !ZONES.iter().any(|zn| {
                DVec2::new(q.x - zn.center.x, q.y - zn.center.z).length() < zn.radius + 0.5
            });
        if clear {
            spawn_lamp(commands, lib, place(land, q.x, q.y, 0.0));
        }
    }
    // Hand carts left at the plaza's edge.
    let mut carts = 0;
    for _ in 0..40 {
        if carts == 2 {
            break;
        }
        let a = rng.range(0.0, TAU) as f64;
        let q = center + DVec2::new(a.cos(), a.sin()) * rng.range(44.0, 48.0) as f64;
        let clear = land.road_weight(q.x, q.y) == 0.0
            && land.road_weight(q.x + 2.0, q.y) == 0.0
            && land.road_weight(q.x - 2.0, q.y) == 0.0
            && placed.iter().all(|fp| fp.distance(q) > 2.5)
            && !ZONES.iter().any(|zn| {
                DVec2::new(q.x - zn.center.x, q.y - zn.center.z).length() < zn.radius + 2.0
            });
        if clear {
            let yaw = a as f32 + rng.range(-0.6, 0.6) + std::f32::consts::FRAC_PI_2;
            spawn_prop(
                commands,
                lib,
                &lib.props.cart,
                place(land, q.x, q.y, 0.0),
                yaw,
                2,
            );
            carts += 1;
        }
    }
}

/// Hedgerows along the farm fields and ripe grain in the wheat and barley fields near the
/// village.
pub fn spawn_farms(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    for (cell, crop) in &land.fields {
        let corners = field_corners(*cell);
        // Hedges on some edges.
        for k in 0..4 {
            if rng.chance(0.4) {
                continue;
            }
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            let len = (b - a).length();
            let steps = (len / 4.0) as i32;
            for i in 0..steps {
                let p = a.lerp(b, (i as f64 + rng.range(0.2, 0.8) as f64) / steps as f64);
                if land.road_weight(p.x, p.y) > 0.0 || in_zone(p.x, p.y, 4.0) {
                    continue;
                }
                let tree = if rng.chance(0.06) {
                    pick_tree(
                        lib,
                        if rng.chance(0.5) {
                            TreeKind::Oak
                        } else {
                            TreeKind::Birch
                        },
                        rng,
                    )
                } else {
                    pick_tree(lib, TreeKind::Bush, rng)
                };
                spawn_tree(
                    commands,
                    lib,
                    tree,
                    place(land, p.x, p.y, 0.2),
                    rng.range(0.0, TAU),
                    rng.range(0.7, 1.2),
                );
            }
        }
        // Grain you can walk through (and burn).
        if !matches!(crop, Crop::Wheat | Crop::Barley) {
            continue;
        }
        let center = (corners[0] + corners[2]) * 0.5;
        if center.length() > 450.0 {
            continue;
        }
        let u = corners[1] - corners[0];
        let v = corners[3] - corners[0];
        let (nu, nv) = ((u.length() / 3.0) as i32, (v.length() / 3.0) as i32);
        for i in 1..nu {
            for j in 1..nv {
                let p = corners[0]
                    + u * ((i as f64 + rng.range(-0.3, 0.3) as f64) / nu as f64)
                    + v * ((j as f64 + rng.range(-0.3, 0.3) as f64) / nv as f64);
                let h = land.height(p.x, p.y);
                let k = rng.range_i32(0, lib.dry_grass.len() as i32) as usize;
                commands.spawn((
                    Transform::from_translation(DVec3::new(p.x, h - 0.03, p.y))
                        .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
                        .with_scale(Vec3::new(1.0, rng.range(1.5, 1.9), 1.0)),
                    MeshRenderer::new(lib.dry_grass[k], lib.mats.grass).without_shadows(),
                    Lod::swap_then_hide(
                        lib.dry_grass[k],
                        lib.dry_grass_mid[k],
                        26.0,
                        rng.range(70.0, 130.0),
                    ),
                ));
            }
        }
    }
}
