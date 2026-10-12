//! Showcase content: the living forest and the glowing-mushroom cave.
//!
//! Both only spawn data (ecology regions, resource sources, ground cover); the plugin grows
//! every plant from it. Call them from a game's setup with a zone center:
//!
//! ```ignore
//! engine_foliage::demo::spawn_demo(&mut commands, forest_center);
//! engine_foliage::demo::spawn_cave_demo(&mut commands, cave_center);
//! ```
//!
//! **Forest** (radius ≈ 90 m, entered from +X): an old mixed forest grown for 110 simulated
//! years, then partly cut and partly burnt 5 years ago. A dirt trail ([`TRAIL`]) leads from the
//! +X edge through every feature below; [`forest_viewpoints`] stand on it.
//! - A clear-cut opening on the +X/+Z side: stumps (oak, birch and hazel resprouting),
//!   grey logs, young trees and grass coming back, and every 25 s a tree at its edge is felled
//!   so you can watch one fall.
//! - A burn scar on the −X/+Z side: charred snags over black ash, birch and grass recolonizing.
//! - A spring-fed pond (−10, +10): lush ferns, bluebells and birches around it.
//! - A dry, rocky ridge (−55, −40): pines win there; oaks and birches struggle.
//! - Growth runs 30× faster than the world default: a year every 20 seconds.
//!
//! **Cave** (≈ 28 × 32 m inside): no light, so only fungi grow. A mineral pool and a seep along
//! the back-left wall feed glowcaps and tall lantern caps; the rest of the floor stays bare.
//! The mushrooms glow brighter the better fed they are and light the cave around them.

use engine_assets::Assets;
use engine_core::math::noise::fbm2;
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRenderer, Shape, StandardMaterial, Vertex};
use engine_world::Ground;

use crate::ecology::RegionState;
use crate::felling::fell_tree;
use crate::plant::{Disturbance, EcologyRegion, Plant, PlantStage, RegionHistory, ResourceSource};
use crate::species::{PlantLibrary, srgb};

/// Forest clear-cut, local to the zone center (x, z, radius).
pub const CLEARING: [f32; 3] = [40.0, 28.0, 18.0];
/// Forest burn scar (x, z, radius).
pub const BURN: [f32; 3] = [-34.0, 52.0, 17.0];
/// Spring-fed pond (x, z, radius of open water).
pub const POND: [f32; 3] = [-10.0, 10.0, 4.5];
/// Dry ridge (x, z, radius).
pub const RIDGE: [f32; 3] = [-55.0, -40.0, 26.0];
/// Years since the forest was cut and burnt.
pub const YEARS_SINCE: f32 = 5.0;
/// The forest trail (local x, z): in from the +X edge, across the clearing, past the pond and
/// the burn scar, up onto the dry ridge. Nothing grows on it.
pub const TRAIL: [[f32; 2]; 13] = [
    [96.0, 41.0],
    [74.0, 36.0],
    [57.0, 31.0],
    [40.0, 27.0],
    [24.0, 22.0],
    [8.0, 16.0],
    [-3.0, 15.5],
    [-14.0, 24.0],
    [-25.0, 35.0],
    [-38.0, 22.0],
    [-46.0, 2.0],
    [-52.0, -18.0],
    [-56.0, -34.0],
];
/// Half the trail's width, meters.
pub const TRAIL_HALF_WIDTH: f32 = 1.6;

/// Distance from `p` to a polyline.
pub fn polyline_distance(p: Vec2, line: &[[f32; 2]]) -> f32 {
    line.windows(2)
        .map(|w| {
            let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t).distance(p)
        })
        .fold(f32::INFINITY, f32::min)
}

/// Circles along the trail for [`EcologyRegion::keep_clear`].
fn trail_clearance() -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for w in TRAIL.windows(2) {
        let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
        let n = (a.distance(b) / 1.5).ceil() as usize;
        for i in 0..n {
            let p = a.lerp(b, i as f32 / n as f32);
            out.push([p.x, p.y, TRAIL_HALF_WIDTH + 0.6]);
        }
    }
    out
}

/// A camera viewpoint of a showcase: world-space eye and target, and a good hour of day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewpoint {
    /// Short unique key, like `forest_trail`.
    pub name: &'static str,
    /// What it shows, for a tour or a menu.
    pub title: &'static str,
    pub eye: DVec3,
    pub target: DVec3,
    pub hour: f32,
}

fn view(
    center: DVec3,
    (name, title): (&'static str, &'static str),
    eye: [f32; 3],
    target: [f32; 3],
    hour: f32,
) -> Viewpoint {
    let at = |v: [f32; 3]| center + DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
    Viewpoint {
        name,
        title,
        eye: at(eye),
        target: at(target),
        hour,
    }
}

/// Where to look at the forest from (heights above the zone center; on uneven ground, add the
/// ground height). All but the first stand on the trail.
pub fn forest_viewpoints(center: DVec3) -> Vec<Viewpoint> {
    vec![
        view(
            center,
            ("forest_overview", "The living forest from above"),
            [112.0, 32.0, 92.0],
            [8.0, 0.0, 12.0],
            9.0,
        ),
        view(
            center,
            ("forest_trail", "The trail into the forest"),
            [76.0, 1.7, 36.5],
            [56.0, 2.5, 31.0],
            9.5,
        ),
        view(
            center,
            (
                "forest_clearing",
                "A clear-cut growing back; a tree falls every 25 s",
            ),
            [58.0, 2.0, 31.5],
            [36.0, 1.5, 25.0],
            9.5,
        ),
        view(
            center,
            ("forest_spring", "Ferns and birches by the spring-fed pond"),
            [-1.0, 1.7, 15.5],
            [-11.0, 0.3, 9.0],
            10.0,
        ),
        view(
            center,
            ("forest_burn_scar", "Charred snags and grass after a fire"),
            [-26.0, 2.0, 37.5],
            [-36.0, 3.0, 54.0],
            10.5,
        ),
        view(
            center,
            ("forest_dry_ridge", "Pines win on the dry, rocky ridge"),
            [-51.5, 1.9, -16.0],
            [-57.0, 2.0, -42.0],
            16.0,
        ),
    ]
}

/// Where to look at the mushroom cave from.
pub fn cave_viewpoints(center: DVec3) -> Vec<Viewpoint> {
    vec![
        view(
            center,
            ("cave_fungi", "Glowing fungi where mineral water reaches"),
            [0.0, 3.0, -6.0],
            [0.0, 0.5, 10.0],
            14.0,
        ),
        view(
            center,
            ("cave_pool", "Glowcaps around the mineral pool"),
            [CAVE_POOL[0] + 6.0, 1.4, CAVE_POOL[1] - 5.0],
            [CAVE_POOL[0] - 1.0, 0.4, CAVE_POOL[1] + 2.0],
            14.0,
        ),
    ]
}

fn local(center: DVec3, x: f32, z: f32) -> DVec3 {
    center + DVec3::new(x as f64, 0.0, z as f64)
}

/// The forest's ecology region (centered on the zone).
pub fn forest_region() -> EcologyRegion {
    EcologyRegion {
        half_extents: [86.0, 86.0],
        round: true,
        cell_m: 2.0,
        species: [
            "oak",
            "birch",
            "pine",
            "spruce",
            "hazel",
            "fern",
            "grass",
            "wildflowers",
            "bluebell",
        ]
        .map(String::from)
        .to_vec(),
        seed_rain: 150.0,
        sky_light: 1.0,
        moisture: 0.42,
        nutrients: 0.45,
        patchiness: 0.75,
        max_plants: [700, 600, 14000],
        step_years: 0.25,
        time_scale: 30.0,
        keep_clear: [vec![POND], trail_clearance()].concat(),
        history: RegionHistory {
            grow_years: 110.0,
            initial_plants: [260, 120, 1500],
            disturbance: Disturbance::Many(vec![
                Disturbance::ClearCut {
                    center: [CLEARING[0], CLEARING[1]],
                    radius: CLEARING[2],
                },
                Disturbance::Burn {
                    center: [BURN[0], BURN[1]],
                    radius: BURN[2],
                },
            ]),
            years_since: YEARS_SINCE,
        },
        seed: 7,
        ..Default::default()
    }
}

/// The forest's resource sources, local to the zone center: the spring and the dry ridge.
pub fn forest_sources() -> Vec<(Vec2, ResourceSource)> {
    vec![
        (
            Vec2::new(POND[0], POND[1]),
            ResourceSource {
                radius_m: 18.0,
                water: 0.45,
                nutrients: 0.12,
                minerals: 0.0,
            },
        ),
        (
            Vec2::new(RIDGE[0], RIDGE[1]),
            ResourceSource {
                radius_m: RIDGE[2],
                water: -0.4,
                nutrients: -0.15,
                minerals: 0.0,
            },
        ),
    ]
}

/// The living forest at `center` (flat ground or whatever `engine_world::Ground` says).
pub fn spawn_demo(commands: &mut Commands, center: DVec3) {
    commands.spawn((Transform::from_translation(center), forest_region()));
    for (at, source) in forest_sources() {
        commands.spawn((
            Transform::from_translation(local(center, at.x, at.y)),
            source,
        ));
    }
    // The pond: dark still water inside a ring of wet mud.
    let mut water = StandardMaterial::hex(0x0d1f1d).with_roughness(0.02);
    water.reflectance = 0.6;
    water.receives_wetness = false;
    let mut mud = StandardMaterial::hex(0x2a2116).with_roughness(0.35);
    mud.reflectance = 0.3;
    mud.receives_wetness = false;
    for (radius, height, material) in [(POND[2] + 0.9, 0.035, mud), (POND[2], 0.05, water)] {
        commands.spawn((
            Transform::from_translation(
                local(center, POND[0], POND[1]) + DVec3::Y * (height * 0.5),
            ),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius,
                    height: height as f32,
                    segments: 48,
                },
                material,
            ),
        ));
    }
    // Rocks on the dry ridge.
    let rock = StandardMaterial::hex(0x6d6a62).with_roughness(0.9);
    let mut rng = Rng::new(0x51d6e);
    for _ in 0..26 {
        let p = rng.in_unit_disk() * RIDGE[2] * 0.8;
        let s = rng.range(0.5, 1.9);
        commands.spawn((
            Transform::from_translation(
                local(center, RIDGE[0] + p.x, RIDGE[1] + p.y) + DVec3::Y * (0.15 * s) as f64,
            )
            .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
            .with_scale(Vec3::new(s * 1.5, s * 0.65, s)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.0,
                    segments: 10,
                },
                rock,
            ),
        ));
    }
    // Leaf litter, moss and ash under the trees.
    commands.spawn((
        Transform::from_translation(center),
        GroundCover {
            half_extents: [92.0, 92.0],
            round: true,
            style: CoverStyle::ForestFloor,
            scorched: vec![[BURN[0], BURN[1], BURN[2] + 2.0, YEARS_SINCE]],
            trail: TRAIL.to_vec(),
            trail_half_width: TRAIL_HALF_WIDTH,
            ..Default::default()
        },
    ));
    // A tree at the clearing's edge falls every so often.
    commands.spawn((
        Transform::from_translation(local(center, CLEARING[0], CLEARING[1])),
        DemoFelling {
            every_secs: 25.0,
            inner_m: CLEARING[2] - 2.0,
            outer_m: CLEARING[2] + 9.0,
            timer: 0.5,
            max_trees: 60,
            felled: 0,
        },
    ));
}

/// Cave pool, local to the cave center (x, z, radius).
pub const CAVE_POOL: [f32; 3] = [0.0, 4.0, 5.0];
/// Mineral seep along the back-left wall (x, z, radius).
pub const CAVE_SEEP: [f32; 3] = [-9.5, 11.5, 5.0];

/// The cave's ecology region (centered on the zone): only fungi, fed by minerals.
pub fn cave_region() -> EcologyRegion {
    EcologyRegion {
        half_extents: [13.5, 15.5],
        cell_m: 1.0,
        species: vec!["glowcap".into(), "lantern cap".into()],
        seed_rain: 30.0,
        sky_light: 0.03,
        moisture: 0.5,
        nutrients: 0.3,
        minerals: 0.0,
        patchiness: 0.6,
        max_plants: [0, 80, 1200],
        step_years: 0.1,
        time_scale: 20.0,
        keep_clear: vec![[CAVE_POOL[0], CAVE_POOL[1], CAVE_POOL[2] + 0.3]],
        history: RegionHistory {
            grow_years: 10.0,
            initial_plants: [0, 40, 120],
            ..Default::default()
        },
        seed: 11,
        ..Default::default()
    }
}

/// The cave's resource sources, local to the zone center: the mineral pool and the seep.
pub fn cave_sources() -> Vec<(Vec2, ResourceSource)> {
    vec![
        (
            Vec2::new(CAVE_POOL[0], CAVE_POOL[1]),
            ResourceSource {
                radius_m: 11.0,
                water: 0.35,
                nutrients: 0.05,
                minerals: 0.95,
            },
        ),
        (
            Vec2::new(CAVE_SEEP[0], CAVE_SEEP[1]),
            ResourceSource {
                radius_m: CAVE_SEEP[2],
                water: 0.25,
                nutrients: 0.0,
                minerals: 0.8,
            },
        ),
    ]
}

/// The mushroom cave's living part at `center` (inside a cave of about 28 × 32 m whose mouth
/// faces −Z): mineral pool, seep, cave floor and the fungi that grow where minerals reach.
pub fn spawn_cave_demo(commands: &mut Commands, center: DVec3) {
    commands.spawn((Transform::from_translation(center), cave_region()));
    for (at, source) in cave_sources() {
        commands.spawn((
            Transform::from_translation(local(center, at.x, at.y)),
            source,
        ));
    }
    // The pool: dark mineral water with a faint glow of its own.
    let mut pool = StandardMaterial::hex(0x061a1e)
        .with_roughness(0.03)
        .with_emissive([0.1, 0.9, 0.75], 0.12);
    pool.reflectance = 0.5;
    pool.receives_wetness = false;
    commands.spawn((
        Transform::from_translation(local(center, CAVE_POOL[0], CAVE_POOL[1]) + DVec3::Y * 0.05),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: CAVE_POOL[2],
                height: 0.02,
                segments: 64,
            },
            pool,
        ),
    ));
    commands.spawn((
        Transform::from_translation(center),
        GroundCover {
            half_extents: [14.2, 16.2],
            round: false,
            cell_m: 0.5,
            style: CoverStyle::CaveFloor,
            ..Default::default()
        },
    ));
}

// ---------------------------------------------------------------- ground cover

/// Look of a [`GroundCover`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CoverStyle {
    /// Leaf litter, humus and moss (mossier where the soil is wet), fading to grass at the edge.
    #[default]
    ForestFloor,
    /// Damp rock and mud with pale mineral crust where mineral water soaks the floor.
    CaveFloor,
}

/// A ground layer (forest floor, cave floor) laid over the ground and colored from the soil
/// of the ecology region beneath it: wetter is mossier, minerals leave crust, fire leaves ash.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundCover {
    /// Half size along X and Z, meters.
    pub half_extents: [f32; 2],
    /// Elliptical, fading out at the edge.
    pub round: bool,
    /// Grid spacing, meters.
    pub cell_m: f32,
    pub style: CoverStyle,
    /// Burnt patches: local `[x, z, radius m, years ago]`.
    pub scorched: Vec<[f32; 4]>,
    /// A dirt trail through it: local `[x, z]` points.
    pub trail: Vec<[f32; 2]>,
    pub trail_half_width: f32,
    pub seed: u64,
}

impl Default for GroundCover {
    fn default() -> Self {
        Self {
            half_extents: [20.0, 20.0],
            round: true,
            cell_m: 1.0,
            style: CoverStyle::ForestFloor,
            scorched: Vec::new(),
            trail: Vec::new(),
            trail_half_width: 1.5,
            seed: 3,
        }
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Color of the cover at a world point. `soil` is `[water, nutrients, minerals]`.
pub fn cover_color(cover: &GroundCover, local: Vec2, world: DVec3, soil: [f32; 3]) -> [f32; 3] {
    let n = fbm2(world.x * 0.07, world.z * 0.07, cover.seed, 3, 2.0, 0.5);
    let n2 = fbm2(
        world.x * 0.35,
        world.z * 0.35,
        cover.seed ^ 0x77,
        2,
        2.0,
        0.5,
    );
    let [water, nutrients, minerals] = soil;
    match cover.style {
        CoverStyle::ForestFloor => {
            let litter = srgb(0x4b3c2a);
            let moss = srgb(0x3e5a27);
            let humus = srgb(0x2b231a);
            let needles = srgb(0x6a4b30);
            let ash = srgb(0x1b1917);
            let lawn = srgb(0x55743a);
            let mut c = mix(litter, moss, smooth(0.38, 0.72, water + 0.18 * n));
            c = mix(
                c,
                humus,
                0.45 * smooth(-0.1, 0.7, n2) + 0.2 * smooth(0.5, 0.9, nutrients),
            );
            c = mix(c, needles, smooth(0.28, 0.12, water) * 0.7);
            for s in &cover.scorched {
                let d = (local - Vec2::new(s[0], s[1])).length();
                let f = (1.0 - d / s[2].max(0.1)).clamp(0.0, 1.0).powf(0.5) / (1.0 + s[3] * 0.18);
                c = mix(c, ash, f * (0.8 + 0.2 * n2));
            }
            if cover.round {
                let e = Vec2::new(
                    local.x / cover.half_extents[0],
                    local.y / cover.half_extents[1],
                )
                .length();
                c = mix(c, lawn, smooth(0.88 + 0.05 * n, 1.0, e));
            }
            if cover.trail.len() >= 2 {
                // Packed earth in the middle, trampled litter at the sides.
                let d = polyline_distance(local, &cover.trail) + 0.35 * n2;
                let hw = cover.trail_half_width;
                c = mix(c, srgb(0x5d4c3a), smooth(hw * 1.6, hw * 0.9, d) * 0.5);
                c = mix(
                    c,
                    srgb(0x75624b),
                    smooth(hw * 0.9, hw * 0.3, d) * (0.8 + 0.2 * n),
                );
            }
            c
        }
        CoverStyle::CaveFloor => {
            let rock = srgb(0x2a2826);
            let mud = srgb(0x3a3027);
            let crust = srgb(0xa3b8ae);
            let mut c = mix(rock, mud, smooth(0.45, 0.75, water + 0.2 * n));
            c = mix(
                c,
                [rock[0] * 0.6, rock[1] * 0.6, rock[2] * 0.6],
                0.5 * smooth(0.0, 0.8, n2),
            );
            let crusty = smooth(0.5, 0.95, minerals) * (0.55 + 0.45 * smooth(-0.3, 0.6, n2));
            mix(c, crust, crusty * 0.85)
        }
    }
}

/// Build the mesh of every new ground cover (after the regions beneath have grown their soil).
pub fn build_ground_cover(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    ground: Option<Res<Ground>>,
    covers: Query<(Entity, &GroundCover, &Transform), Without<MeshRenderer>>,
    regions: Query<&RegionState>,
) {
    for (e, cover, t) in &covers {
        let center = t.translation;
        let cell = cover.cell_m.max(0.25);
        let nx = (cover.half_extents[0] * 2.0 / cell).ceil() as u32 + 1;
        let nz = (cover.half_extents[1] * 2.0 / cell).ceil() as u32 + 1;
        let mut vertices = Vec::with_capacity((nx * nz) as usize);
        let mut inside = Vec::with_capacity((nx * nz) as usize);
        for j in 0..nz {
            for i in 0..nx {
                let lx = -cover.half_extents[0] + i as f32 * cell;
                let lz = -cover.half_extents[1] + j as f32 * cell;
                let world = center + DVec3::new(lx as f64, 0.0, lz as f64);
                let y = ground
                    .as_deref()
                    .and_then(|g| g.height(world + DVec3::Y * 4.0))
                    .unwrap_or(center.y);
                let soil = regions
                    .iter()
                    .find(|r| r.sim.soil.contains(world))
                    .map(|r| r.sim.soil.sample(world))
                    .unwrap_or([0.4, 0.4, 0.0]);
                let local = Vec2::new(lx, lz);
                let c = cover_color(cover, local, world, soil);
                let e = Vec2::new(lx / cover.half_extents[0], lz / cover.half_extents[1]).length();
                inside.push(!cover.round || e <= 1.0);
                vertices.push(
                    Vertex::new(
                        Vec3::new(lx, (y - center.y) as f32 + 0.025, lz),
                        Vec3::Y,
                        Vec2::new(lx * 0.25, lz * 0.25),
                    )
                    .with_color(Vec4::new(c[0], c[1], c[2], 1.0)),
                );
            }
        }
        let mut indices = Vec::new();
        for j in 0..nz - 1 {
            for i in 0..nx - 1 {
                let a = j * nx + i;
                let (b, c, d) = (a + 1, a + nx + 1, a + nx);
                if [a, b, c, d].iter().any(|&k| !inside[k as usize]) {
                    continue;
                }
                indices.extend_from_slice(&[a, c, b, a, d, c]);
            }
        }
        let mut mesh = Mesh::new(vertices, indices);
        mesh.compute_normals();
        let mut material = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.93);
        material.receives_wetness = true;
        let mut r = MeshRenderer::new(meshes.add(mesh), material);
        r.cast_shadows = false;
        commands.entity(e).insert(r);
    }
}

// ---------------------------------------------------------------- live felling

/// Fells the nearest mature tree in a ring around this entity every `every_secs` (world time),
/// toward the center: a showcase of falling trees and the regrowth that follows.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoFelling {
    pub every_secs: f32,
    /// Ring of trees to fell, meters from this entity.
    pub inner_m: f32,
    pub outer_m: f32,
    /// Seconds until the next tree falls.
    pub timer: f32,
    /// Stop after this many.
    pub max_trees: u32,
    pub felled: u32,
}

impl Default for DemoFelling {
    fn default() -> Self {
        Self {
            every_secs: 30.0,
            inner_m: 10.0,
            outer_m: 20.0,
            timer: 5.0,
            max_trees: 20,
            felled: 0,
        }
    }
}

/// Run [`DemoFelling`] entities.
pub fn run_demo_felling(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut q = world.query::<(Entity, &mut DemoFelling, &Transform)>();
    let mut due = Vec::new();
    for (e, mut d, t) in q.iter_mut(world) {
        if d.felled >= d.max_trees {
            continue;
        }
        d.timer -= dt;
        if d.timer <= 0.0 {
            d.timer = d.every_secs;
            due.push((e, t.translation, d.inner_m as f64, d.outer_m as f64));
        }
    }
    if due.is_empty() {
        return;
    }
    let Some(library) = world.get_resource::<PlantLibrary>().cloned() else {
        return;
    };
    for (e, center, inner, outer) in due {
        let mut pq = world.query::<(Entity, &Plant, &Transform)>();
        let pick = pq
            .iter(world)
            .filter(|(_, p, _)| {
                p.stage == PlantStage::Alive && p.size > 0.5 && library.get(p.species).is_tree()
            })
            .map(|(pe, _, t)| {
                (
                    pe,
                    t.translation,
                    DVec2::new(t.translation.x - center.x, t.translation.z - center.z).length(),
                )
            })
            .filter(|(_, _, d)| *d >= inner && *d <= outer)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((tree, p, _)) = pick {
            let dir = Vec3::new((center.x - p.x) as f32, 0.0, (center.z - p.z) as f32)
                .normalize_or(Vec3::X);
            if fell_tree(world, tree, dir).is_some()
                && let Some(mut d) = world.get_mut::<DemoFelling>(e)
            {
                d.felled += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ecology::play_history;
    use crate::sim::{FlatEnv, RegionSim, SimPlant, SimSource, Soil};

    fn grown_forest() -> (Vec<SimPlant>, usize) {
        grow(forest_region(), forest_sources())
    }

    fn grow(region: EcologyRegion, sources: Vec<(Vec2, ResourceSource)>) -> (Vec<SimPlant>, usize) {
        let library = PlantLibrary::default();
        let env = FlatEnv::default();
        let sources: Vec<SimSource> = sources
            .into_iter()
            .map(|(at, s)| SimSource {
                pos: DVec3::new(at.x as f64, 0.0, at.y as f64),
                radius: s.radius_m,
                water: s.water,
                nutrients: s.nutrients,
                minerals: s.minerals,
            })
            .collect();
        let soil = Soil::build(&region, DVec3::ZERO, &sources, &env);
        let mut sim = RegionSim::new(soil, region.seed);
        let rain: Vec<_> = region
            .species
            .iter()
            .filter_map(|n| library.id(n))
            .collect();
        let (plants, logs) = play_history(&mut sim, &region, DVec3::ZERO, &library, &rain, &env);
        (plants, logs.len())
    }

    fn census(plants: &[SimPlant], within: Option<[f32; 3]>) -> BTreeMap<String, usize> {
        let library = PlantLibrary::default();
        let mut out = BTreeMap::new();
        for p in plants {
            if let Some([x, z, r]) = within
                && DVec2::new(p.pos.x - x as f64, p.pos.z - z as f64).length() > r as f64
            {
                continue;
            }
            let key = format!(
                "{} {:?}",
                library.get(p.plant.species).name,
                std::mem::discriminant(&p.plant.stage)
            );
            let key = key.split(' ').next().unwrap().to_string()
                + match p.plant.stage {
                    PlantStage::Alive if p.plant.size < 0.25 => " young",
                    PlantStage::Alive => "",
                    PlantStage::Stump { .. } => " stump",
                    PlantStage::Dead { .. } => " dead",
                    PlantStage::Burnt { .. } => " burnt",
                };
            *out.entry(key).or_insert(0) += 1;
        }
        out
    }

    #[test]
    fn showcase_forest_is_mixed_and_regrowing() {
        let (plants, logs) = grown_forest();
        let all = census(&plants, None);
        let clearing = census(&plants, Some(CLEARING));
        let burn = census(&plants, Some(BURN));
        let spring = census(&plants, Some([POND[0], POND[1], 14.0]));
        let ridge = census(&plants, Some(RIDGE));
        eprintln!(
            "all: {all:?}\nclearing: {clearing:?}\nburn: {burn:?}\nspring: {spring:?}\nridge: {ridge:?}\nlogs: {logs}"
        );
        let n = |c: &BTreeMap<String, usize>, k: &str| c.get(k).copied().unwrap_or(0);
        let sum = |c: &BTreeMap<String, usize>, keys: &[&str]| {
            keys.iter().map(|k| n(c, k)).sum::<usize>()
        };

        // A mixed forest: no tree species takes over.
        let canopy = sum(&all, &["oak", "birch", "pine", "spruce"]);
        for tree in ["oak", "pine", "spruce"] {
            assert!(n(&all, tree) * 10 > canopy, "too few {tree}: {all:?}");
            assert!(n(&all, tree) * 2 < canopy, "{tree} took over: {all:?}");
        }
        assert!(sum(&all, &["fern", "grass"]) > 2000, "{all:?}");
        // The clear-cut: stumps of pines and spruces (oaks, birches and hazels resprouted),
        // young trees and plenty of grass.
        assert!(
            sum(&clearing, &["pine stump", "spruce stump"]) >= 3,
            "{clearing:?}"
        );
        assert!(
            sum(
                &clearing,
                &[
                    "oak young",
                    "birch young",
                    "pine young",
                    "spruce young",
                    "birch",
                    "hazel"
                ]
            ) >= 5,
            "{clearing:?}"
        );
        assert!(n(&clearing, "grass") > 100, "{clearing:?}");
        assert!(logs >= 10);
        // The burn: charred snags, grass and ferns coming back.
        let snags = burn
            .iter()
            .filter(|(k, _)| k.ends_with("burnt"))
            .map(|(_, v)| v)
            .sum::<usize>();
        assert!(snags >= 10, "{burn:?}");
        assert!(n(&burn, "grass") > 50, "{burn:?}");
        // Ferns love the spring; pines win on the dry ridge.
        assert!(n(&spring, "fern") > 50, "{spring:?}");
        assert!(
            n(&ridge, "pine") > n(&ridge, "birch") + n(&ridge, "oak"),
            "{ridge:?}"
        );
    }

    #[test]
    fn showcase_cave_glows_where_minerals_reach() {
        let (plants, _) = grow(cave_region(), cave_sources());
        let all = census(&plants, None);
        eprintln!("cave: {all:?}");
        let n = |k: &str| all.get(k).copied().unwrap_or(0);
        assert!(n("glowcap") > 80, "{all:?}");
        assert!(n("lantern") >= 4, "{all:?}");
        // Fungi only where the pool or the seep feeds the floor.
        let fed = |p: &SimPlant| {
            let d = |c: [f32; 3]| DVec2::new(p.pos.x - c[0] as f64, p.pos.z - c[1] as f64).length();
            d(CAVE_POOL) < 11.0 || d(CAVE_SEEP) < CAVE_SEEP[2] as f64
        };
        let alive: Vec<_> = plants.iter().filter(|p| p.plant.stage.is_alive()).collect();
        let stray = alive.iter().filter(|p| !fed(p)).count();
        assert!(
            stray * 20 < alive.len(),
            "{stray} of {} fungi grow on bare rock",
            alive.len()
        );
    }
}
