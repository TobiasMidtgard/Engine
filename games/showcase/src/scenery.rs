//! The valley's scenery: terrain, woods, meadows of grass and wildflowers, boulders, a village
//! around the plaza with street lamps. Zones get their content from `zones.rs`; this is
//! everything between and around them.
//!
//! Far things switch to cheaper meshes or hide (see [`Lod`]), so the whole valley (tens of
//! thousands of plants) stays fast.

use crate::landscape::{Cover, Crop, Landscape, field_corners};
use crate::meshgen::{self, TreeKind};
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
    pub plaster: Handle<StandardMaterial>,
    pub wood: Handle<StandardMaterial>,
    pub roof: Handle<StandardMaterial>,
    pub window: Handle<StandardMaterial>,
    pub iron: Handle<StandardMaterial>,
    pub lantern: Handle<StandardMaterial>,
}

pub struct TreeHandles {
    pub kind: TreeKind,
    pub trunk: Handle<Mesh>,
    pub crown: Handle<Mesh>,
    pub far: Handle<Mesh>,
}

pub struct HouseHandles {
    pub walls: Handle<Mesh>,
    pub wood: Handle<Mesh>,
    pub roof: Handle<Mesh>,
    pub windows: Handle<Mesh>,
    pub half: Vec2,
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
    pub lamp: (Handle<Mesh>, Handle<Mesh>),
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
    let mut window = StandardMaterial::hex(0x14161c)
        .with_roughness(0.08)
        .with_emissive([1.0, 0.62, 0.32], 40.0);
    window.reflectance = 0.7;
    window.receives_wetness = false;
    let mats = Mats {
        terrain: materials.add(terrain),
        bark: materials.add(white(0.9)),
        leaves: materials.add(leaves),
        rock: materials.add(white(0.85)),
        grass: materials.add(grass),
        flowers: materials.add(flowers),
        plaster: materials.add(white(0.9)),
        wood: materials.add(white(0.75)),
        roof: materials.add(white(0.7)),
        window: materials.add(window),
        iron: materials.add(white(0.45).with_metallic(0.8)),
        lantern: materials.add(
            StandardMaterial::hex(0x332211)
                .with_emissive([1.0, 0.7, 0.4], 3_000.0)
                .with_roughness(0.2),
        ),
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
    let houses = (0..6)
        .map(|i| {
            let h = meshgen::house(900 + i);
            HouseHandles {
                walls: meshes.add(h.walls),
                wood: meshes.add(h.wood),
                roof: meshes.add(h.roof),
                windows: meshes.add(h.windows),
                half: h.half,
            }
        })
        .collect();
    let (iron, glass) = meshgen::lamp_post();
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
        lamp: (meshes.add(iron), meshes.add(glass)),
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

/// A house with its door facing `facing` (radians around Y; 0 = +Z).
pub fn spawn_house(
    commands: &mut Commands,
    lib: &Library,
    h: &HouseHandles,
    pos: DVec3,
    facing: f32,
) {
    let tf = Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(facing));
    for (mesh, mat) in [
        (h.walls, lib.mats.plaster),
        (h.wood, lib.mats.wood),
        (h.roof, lib.mats.roof),
        (h.windows, lib.mats.window),
    ] {
        commands.spawn((tf, MeshRenderer::new(mesh, mat)));
    }
}

/// A street lamp with a warm point light (lumens).
pub fn spawn_lamp(commands: &mut Commands, lib: &Library, pos: DVec3) {
    let tf = Transform::from_translation(pos);
    commands.spawn((tf, MeshRenderer::new(lib.lamp.0, lib.mats.iron)));
    commands.spawn((
        tf,
        MeshRenderer::new(lib.lamp.1, lib.mats.lantern).without_shadows(),
    ));
    commands.spawn((
        Transform::from_translation(pos + DVec3::new(0.0, 3.9, 0.0)),
        PointLight {
            color: [1.0, 0.72, 0.45],
            intensity_lm: 1800.0,
            range: 16.0,
            radius: 0.15,
        },
    ));
}

/// The village: houses ringing the plaza (doors toward it), more along the roads out, and
/// street lamps.
pub fn spawn_village(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    let plaza = zone("plaza").center;
    let mut placed: Vec<(DVec2, f64)> = Vec::new();
    let mut try_house = |commands: &mut Commands, rng: &mut Rng, x: f64, z: f64, facing: f32| {
        let h = &lib.houses[rng.range_i32(0, lib.houses.len() as i32) as usize];
        let r = h.half.length() as f64;
        let c = DVec2::new(x, z);
        // Keep clear of roads, zones and other houses.
        let clear = (0..8).all(|k| {
            let a = k as f64 / 8.0 * std::f64::consts::TAU;
            let q = c + DVec2::new(a.cos(), a.sin()) * r;
            land.road_weight(q.x, q.y) == 0.0
        }) && land.road_weight(x, z) == 0.0
            && !ZONES.iter().any(|zn| {
                (DVec2::new(zn.center.x, zn.center.z) - c).length() < zn.radius + r + 2.0
            })
            && placed
                .iter()
                .all(|(p, pr)| (*p - c).length() > pr + r + 3.0);
        if !clear {
            return;
        }
        // Sit on the lowest corner so no corner floats.
        let base = (0..4)
            .map(|k| {
                let a = k as f64 / 4.0 * std::f64::consts::TAU + 0.78;
                land.height(x + a.cos() * r * 0.7, z + a.sin() * r * 0.7)
            })
            .fold(f64::MAX, f64::min);
        spawn_house(commands, lib, h, DVec3::new(x, base, z), facing);
        placed.push((c, r));
    };
    for ring in [58.0, 78.0] {
        let steps = if ring < 60.0 { 22 } else { 28 };
        for i in 0..steps {
            let a = i as f64 / steps as f64 * std::f64::consts::TAU + rng.range(-0.04, 0.04) as f64;
            let x = plaza.x + a.cos() * ring;
            let z = plaza.z + a.sin() * ring;
            // Face the plaza: door (+Z local) toward the center.
            let facing = (-(a.cos()) as f32).atan2(-(a.sin()) as f32);
            try_house(commands, rng, x, z, facing);
        }
    }
    // Lamps around the plaza and along the first stretch of each road.
    for i in 0..12 {
        let a = i as f64 / 12.0 * std::f64::consts::TAU + 0.13;
        let p = DVec3::new(a.cos() * 34.0, 0.0, a.sin() * 34.0);
        spawn_lamp(commands, lib, place(land, p.x, p.z, 0.0));
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
