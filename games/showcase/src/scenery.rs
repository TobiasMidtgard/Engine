//! The valley's scenery: terrain, woods, meadows of grass and wildflowers, boulders, a village
//! around the plaza with street lamps. Zones get their content from `zones.rs`; this is
//! everything between and around them.
//!
//! Far things switch to cheaper meshes or hide (see [`Lod`]), so the whole valley (tens of
//! thousands of plants) stays fast.

use crate::landscape::{Cover, Crop, Landscape, field_corners};
use crate::meshgen::{self, TreeKind};
use crate::rocks::{RockKit, RockLook, RockSize};
use crate::texgen::VillageMats;
use crate::village::{self, Part, PropKind, PropSpot};
use crate::zones::{ZONES, zone};
use engine::prelude::*;
use engine::render::Displacement;
use std::sync::Arc;

/// Materials shared by the scenery (vertex colors carry the variation).
pub struct Mats {
    pub terrain: Handle<StandardMaterial>,
    pub grass: Handle<StandardMaterial>,
    pub flowers: Handle<StandardMaterial>,
    /// The village's procedural PBR materials (plaster, timber, tiles, glass, iron, ...).
    pub village: VillageMats,
}

/// A tree variant: Foliage's species for its kind and a seed for its genes.
pub struct TreeHandles {
    pub kind: TreeKind,
    pub species: engine_foliage::SpeciesId,
    pub seed: u64,
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
    /// Boulder variants and the rock, wet rock and moss materials.
    pub rocks: RockKit,
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
    /// Dry-stone wall segment variants and the heads that end a run.
    pub walls: Vec<WallHandles>,
    pub wall_heads: Vec<Handle<Mesh>>,
    pub wall_head_placed: Vec<Arc<[crate::walls::Placed]>>,
    /// The hero stones' palette, for the walls up close.
    pub wall_palette: WallPalette,
    /// The spaceport's 600 m tower.
    pub tower: crate::tower::TowerKit,
}

/// One dry-stone wall segment variant (see [`crate::walls`]).
pub struct WallHandles {
    pub near: Handle<Mesh>,
    /// For the last segment of a run (its stones leave room for the head).
    pub near_end: Handle<Mesh>,
    /// The core: dark behind the stones (the run's last segment's stops inside the head),
    /// stone-colored from afar.
    pub core: Handle<Mesh>,
    pub core_end: Handle<Mesh>,
    pub core_far: Handle<Mesh>,
    /// Where `near`'s and `near_end`'s stones are, for the hero stones.
    pub placed: Arc<[crate::walls::Placed]>,
    pub placed_end: Arc<[crate::walls::Placed]>,
}

/// The hero wall stones' meshes and materials ([`crate::walls::HERO_CLASSES`]).
#[derive(Resource, Clone)]
pub struct WallPalette {
    /// Per class, [`crate::walls::HERO_VARIANTS`] stones: a light base mesh and the
    /// displacement that finishes it into real relief.
    pub stones: Vec<(Handle<Mesh>, Displacement)>,
    /// A material per shade ([`crate::walls::HERO_SHADES`]).
    pub shades: Vec<Handle<StandardMaterial>>,
}

impl Library {
    pub fn trees_of(&self, kind: TreeKind) -> impl Iterator<Item = &TreeHandles> {
        self.trees.iter().filter(move |t| t.kind == kind)
    }
}

/// The library's CPU work as plain data (meshes, textures, the hero stones), built on a
/// worker so the frame thread only adds the results as assets (the split Unreal's async
/// loading thread and Unity's background loading make: load and build off the main thread,
/// integrate on it).
pub struct LibraryParts {
    detail: (Image, Image),
    textures: crate::texgen::VillageTextures,
    village: village::VillageMeshes,
    grass: Vec<(Mesh, Mesh)>,
    dry_grass: Vec<(Mesh, Mesh)>,
    flowers: Vec<Mesh>,
    segments: Vec<crate::walls::WallSegment>,
    heads: Vec<crate::walls::WallHead>,
    palette: Vec<(Mesh, Displacement)>,
}

/// The library being built: its parts on a worker, the rocks and the tower on theirs.
pub struct LibraryJob {
    parts: std::thread::JoinHandle<LibraryParts>,
    rocks: crate::rocks::KitJob,
    tower: crate::tower::TowerJob,
    started: std::time::Instant,
}

impl LibraryJob {
    /// Start every part of the library on worker threads.
    pub fn start() -> Self {
        Self {
            // Rock textures, boulders and the cliffs build on threads of their own...
            rocks: crate::rocks::start_kit(),
            // ...and so does the tower.
            tower: crate::tower::start(),
            parts: std::thread::spawn(build_parts),
            started: std::time::Instant::now(),
        }
    }

    /// True once every worker is done ([`LibraryJob::finish`] won't wait).
    pub fn is_ready(&self) -> bool {
        self.parts.is_finished() && self.rocks.is_ready() && self.tower.is_ready()
    }

    /// Add the parts as assets (waiting for any still building: tours and headless runs
    /// build the world at once).
    pub fn finish(
        self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        plants: &engine_foliage::PlantLibrary,
    ) -> Library {
        let parts = self.parts.join().expect("library thread");
        let t0 = std::time::Instant::now();
        let rocks = self.rocks.finish(meshes, materials, images);
        let tower = self.tower.finish(meshes, materials, images);
        let lib = add_parts(parts, rocks, tower, meshes, materials, images, plants);
        engine::core::log::info!(
            "library: built in {:.2} s, added as assets in {:.3} s",
            self.started.elapsed().as_secs_f32(),
            t0.elapsed().as_secs_f32()
        );
        lib
    }
}

/// Every mesh and texture of the library, side by side on worker threads.
fn build_parts() -> LibraryParts {
    let t0 = std::time::Instant::now();
    let grass_set = |seed: u64, dry: f32| {
        (
            meshgen::grass_patch(seed, 4.0, 1300, dry, 3),
            meshgen::grass_patch(seed, 4.0, 420, dry, 1),
        )
    };
    let flower_colors = [
        Vec3::new(0.9, 0.75, 0.1),
        Vec3::new(0.85, 0.85, 0.85),
        Vec3::new(0.45, 0.2, 0.7),
        Vec3::new(0.8, 0.1, 0.06),
    ];
    let parts = std::thread::scope(|scope| {
        // The village's meshes, its textures and the ground's detail map...
        let village = scope.spawn(village::build_meshes);
        let textures = scope.spawn(crate::texgen::village_textures);
        let detail = scope.spawn(|| meshgen::ground_detail(512));
        // ...hundreds of sculpted stones a wall variant...
        let segments: Vec<_> = (0..4)
            .map(|i| scope.spawn(move || crate::walls::segment(900 + i)))
            .collect();
        let heads: Vec<_> = (0..3)
            .map(|i| scope.spawn(move || crate::walls::head(950 + i)))
            .collect();
        let palette = scope.spawn(palette_stones);
        // ...and the grass and flowers here meanwhile.
        let grass = (0..6)
            .map(|i| grass_set(700 + i, 0.15 + i as f32 * 0.05))
            .collect();
        let dry_grass = (0..4).map(|i| grass_set(750 + i, 0.75)).collect();
        let flowers = flower_colors
            .iter()
            .enumerate()
            .map(|(i, c)| meshgen::flower_patch(800 + i as u64, 4.0, 24, *c))
            .collect();
        fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
            h.join().expect("library worker")
        }
        LibraryParts {
            detail: join(detail),
            textures: join(textures),
            village: join(village),
            grass,
            dry_grass,
            flowers,
            segments: segments.into_iter().map(join).collect(),
            heads: heads.into_iter().map(join).collect(),
            palette: join(palette),
        }
    });
    engine::core::log::info!("library parts built in {:.2} s", t0.elapsed().as_secs_f32());
    parts
}

/// Add the parts as assets and make the materials (the frame thread's share).
fn add_parts(
    parts: LibraryParts,
    rocks: RockKit,
    tower: crate::tower::TowerKit,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    plants: &engine_foliage::PlantLibrary,
) -> Library {
    let LibraryParts {
        detail: (detail, detail_normal),
        textures,
        village: village_meshes,
        grass,
        dry_grass,
        flowers,
        segments,
        heads,
        palette,
    } = parts;
    let white = |r: f32| StandardMaterial::hex(0xffffff).with_roughness(r);
    let mut terrain = white(0.95)
        .with_base_color_texture(images.insert_keyed("showcase/ground/detail", detail))
        .with_normal_texture(images.insert_keyed("showcase/ground/detail_normal", detail_normal));
    // The detail map averages about 0.85; lift the base so the vertex colors keep their value.
    terrain.base_color = [1.18, 1.18, 1.18, 1.0];
    terrain.normal_scale = 0.7;
    let mut grass_mat = white(0.8).with_subsurface(0.6).with_wind_sway(0.18);
    grass_mat.double_sided = true;
    let mut flowers_mat = white(0.6).with_subsurface(0.4).with_wind_sway(0.15);
    flowers_mat.double_sided = true;
    let mats = Mats {
        terrain: materials.add(terrain),
        grass: materials.add(grass_mat),
        flowers: materials.add(flowers_mat),
        village: crate::texgen::village_materials(textures, materials, images),
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
            // The valley's conifers are spruces with a few pines among them, the bushes hazel.
            let name = match kind {
                TreeKind::Oak => "oak",
                TreeKind::Spruce if v % 3 == 2 => "pine",
                TreeKind::Spruce => "spruce",
                TreeKind::Birch => "birch",
                TreeKind::Bush => "hazel",
            };
            let species = plants
                .id(name)
                .unwrap_or_else(|| panic!("Foliage has no {name}"));
            trees.push(TreeHandles {
                kind,
                species,
                seed,
            });
        }
    }
    let mut add_pairs = |set: Vec<(Mesh, Mesh)>| -> (Vec<_>, Vec<_>) {
        set.into_iter()
            .map(|(near, mid)| (meshes.add(near), meshes.add(mid)))
            .unzip()
    };
    let (grass, grass_mid) = add_pairs(grass);
    let (dry_grass, dry_grass_mid) = add_pairs(dry_grass);
    let flowers = flowers.into_iter().map(|m| meshes.add(m)).collect();
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
    let walls = segments
        .into_iter()
        .map(|w| WallHandles {
            near: meshes.add(w.near),
            near_end: meshes.add(w.near_end),
            core: meshes.add(w.core),
            core_end: meshes.add(w.core_end),
            core_far: meshes.add(w.core_far),
            placed: w.placed.into(),
            placed_end: w.placed_end.into(),
        })
        .collect();
    let (wall_heads, wall_head_placed) = heads
        .into_iter()
        .map(|h| (meshes.add(h.near), Arc::from(h.placed)))
        .unzip();
    let wall_palette = wall_palette(palette, meshes, materials);
    engine::core::log::info!(
        "village: {} house types ({} triangles near, {} far)",
        houses.len(),
        near_tris,
        far_tris,
    );
    Library {
        mats,
        trees,
        rocks,
        grass,
        grass_mid,
        dry_grass,
        dry_grass_mid,
        flowers,
        houses,
        lamp,
        props,
        walls,
        wall_heads,
        wall_head_placed,
        wall_palette,
        tower,
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
    mut q: Query<(
        &GlobalTransform,
        &mut Lod,
        &mut MeshRenderer,
        Option<&HideWithin>,
    )>,
) {
    let Some(eye) = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    else {
        return;
    };
    q.par_iter_mut()
        .for_each(|(t, mut lod, mut renderer, within)| {
            let d2 = (t.translation() - eye).length_squared();
            let far = d2 > (lod.switch as f64).powi(2);
            let shown = d2 <= (lod.hide as f64).powi(2)
                && (!far || lod.far.is_some())
                && within.is_none_or(|w| d2 >= (w.0 as f64).powi(2));
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

/// One of Foliage's scenery trees (it picks its own level of detail and follows the seasons).
/// `scale` sizes it: a grown tree is about 1, saplings and young trees less.
pub fn spawn_tree(commands: &mut Commands, t: &TreeHandles, pos: DVec3, yaw: f32, scale: f32) {
    // Every tree its own genes (lean, height, crown), from its variant and where it stands.
    let spot = (pos.x * 7.0).round() as i64 as u64 ^ ((pos.z * 13.0).round() as i64 as u64) << 21;
    let genes = engine_foliage::Genes::random(&mut Rng::new(t.seed ^ spot));
    // Foliage's level of detail counts the size, not the transform's scale. Mostly grown
    // trees, so the woods' crowns meet.
    let size = (0.75 + (scale - 0.75) * 0.6).clamp(0.6, 1.0);
    commands.spawn((
        Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
        engine_foliage::SceneryTree {
            species: t.species,
            size,
            genes,
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

/// On a zone's wooded rim (see [`crate::zones::WOODED_RIMS`]) and inside no other zone, clear
/// of the forest trail's way in: the valley's trees may grow here.
fn on_wooded_rim(x: f64, z: f64) -> bool {
    let rim = crate::zones::WOODED_RIMS.iter().any(|&(key, inner)| {
        let zn = crate::zones::zone(key);
        let p = DVec2::new(x - zn.center.x, z - zn.center.z);
        // The trail comes in from beyond the +X edge.
        let t = engine_foliage::demo::TRAIL;
        let (a, b) = (
            DVec2::new(t[1][0] as f64, t[1][1] as f64),
            DVec2::new(t[0][0] as f64, t[0][1] as f64),
        );
        let way_in = (b - a).normalize();
        let along = (p - b).dot(way_in);
        let off = (p - b - way_in * along).length();
        let on_trail = key == "forest" && along > -25.0 && off < 4.0;
        (inner..zn.radius + 20.0).contains(&p.length()) && !on_trail
    });
    rim && !ZONES.iter().any(|zn| {
        let d = DVec2::new(x - zn.center.x, z - zn.center.z).length();
        d < zn.radius + 12.0 && !crate::zones::WOODED_RIMS.iter().any(|(k, _)| *k == zn.key)
    })
}

/// Inside a zone whose ground is paved, built over or its own (no scenery grass there), or
/// within `reach` of a grassy zone's bare footprint (how far the grass placed here spreads).
fn in_hard_zone(x: f64, z: f64, reach: f64) -> bool {
    ZONES.iter().any(|zn| {
        let d = DVec2::new(x - zn.center.x, z - zn.center.z).length();
        d < zn.radius - 2.0 && !crate::zones::GRASSY_ZONES.contains(&zn.key)
    }) || crate::zones::BARE_FOOTPRINTS.iter().any(|&(key, rx, rz)| {
        let c = crate::zones::zone(key).center;
        (rx[0] - reach..rx[1] + reach).contains(&(x - c.x))
            && (rz[0] - reach..rz[1] + reach).contains(&(z - c.z))
    })
}

/// Woods on the hills and lone trees in the meadows.
/// Valley trees are planted within this of the walk and the tour's viewpoints (m, across the
/// ground); beyond it Foliage's far woods (from 500 m off the camera) draw the slopes, and its
/// near woods grow trees round a free camera that goes there. Per-tree instances near and
/// clustered cards far, as Unreal's foliage does: 36k trees with their parts were about 110k
/// drawn meshes. `--trees-within <m>` (or `all`) changes it.
#[derive(Resource, Clone, Copy)]
pub struct TreeReach(pub f64);

impl Default for TreeReach {
    fn default() -> Self {
        Self(600.0)
    }
}

/// Where trees are kept (see [`TreeReach`]): along the walk and round the tour's viewpoints
/// over the valley. Not round what they look at: past 500 m the far woods draw it anyway.
fn tree_anchors() -> Vec<DVec2> {
    let mut out = Vec::new();
    for i in 0..crate::walk::STOPS.len() {
        let (from, to) = crate::walk::leg(i);
        let steps = ((to - from).length() / 25.0).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let p = from.lerp(to, k as f64 / steps as f64);
            out.push(DVec2::new(p.x, p.z));
        }
    }
    for p in crate::places::places() {
        let at = DVec2::new(p.position.x, p.position.z);
        if at.length() < 1400.0 {
            out.push(at);
        }
    }
    out
}

pub fn spawn_woods(
    commands: &mut Commands,
    lib: &Library,
    land: &Landscape,
    rng: &mut Rng,
    reach: TreeReach,
) {
    let spacing = 7.5;
    let extent = 1300.0;
    let n = (extent * 2.0 / spacing) as i32;
    let mut count = 0;
    let mut left = 0;
    let anchors = tree_anchors();
    let kept = |x: f64, z: f64| {
        let p = DVec2::new(x, z);
        anchors
            .iter()
            .any(|a| a.distance_squared(p) < reach.0 * reach.0)
    };
    for i in 0..n {
        for j in 0..n {
            let x = -extent + (i as f64 + rng.range(0.1, 0.9) as f64) * spacing;
            let z = -extent + (j as f64 + rng.range(0.1, 0.9) as f64) * spacing;
            let r = (x * x + z * z).sqrt();
            let rim = on_wooded_rim(x, z);
            if r > extent || (in_zone(x, z, 12.0) && !rim) {
                continue;
            }
            let wood = land.woodland(x, z);
            let p = wood * wood + 0.012;
            if rng.f32() as f64 > p {
                continue;
            }
            if land.road_weight(x, z) > 0.0
                || (land.pad_weight(x, z) > 0.3 && !rim)
                || crate::places::near_lake_trail(x, z, 4.0)
            {
                continue;
            }
            let h = land.height(x, z);
            let n = land.normal(x, z, 2.0);
            // Off the lake's shore (the valley floor elsewhere is about as low as the water).
            let shore = land.lake_t(x, z) < 1.3 && land.lake_depth(x, z) > -1.5;
            if n.y < 0.8 || shore || h > 700.0 {
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
            // The random draws are made either way, so what is planted keeps its layout.
            let keep = kept(x, z);
            let t = pick_tree(lib, kind, rng);
            let scale = rng.range(0.75, 1.25);
            let at = place(land, x, z, 0.1);
            let yaw = rng.range(0.0, TAU);
            if !keep {
                left += 1;
            } else {
                spawn_tree(commands, t, at, yaw, scale);
                if near_wildlife(x, z) {
                    solid(commands, at, 0.35 * scale, 5.0 * scale);
                }
                count += 1;
            }
            // Bushes along the wood edges.
            if wood < 0.6 && rng.chance(0.35) {
                let a = rng.range(0.0, TAU) as f64;
                let (bx, bz) = (x + a.cos() * 3.5, z + a.sin() * 3.5);
                if land.road_weight(bx, bz) == 0.0 && !in_zone(bx, bz, 4.0) {
                    let b = pick_tree(lib, TreeKind::Bush, rng);
                    let (yaw, scale) = (rng.range(0.0, TAU), rng.range(0.7, 1.3));
                    if keep {
                        spawn_tree(commands, b, place(land, bx, bz, 0.2), yaw, scale);
                    }
                }
            }
        }
    }
    engine::core::log::info!(
        "scenery: {count} trees within {:.0} m of the walk and the viewpoints, {left} beyond left to the near and far woods",
        reach.0
    );
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
            // A grass patch reaches about 2 m from its center.
            if x * x + z * z > extent * extent || in_hard_zone(x, z, 2.0) {
                continue;
            }
            if land.road_weight(x, z) > 0.05
                || land.lake_depth(x, z) > -0.3
                || crate::places::near_lake_trail(x, z, 2.5)
            {
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
            // The grass itself is the GPU meadow ([`meadow_grass`]); these draws stay so
            // the scenery spawned after the meadows keeps its layout.
            let _ = (
                rng.range_i32(0, lib.grass.len() as i32),
                rng.range(20.0, 26.0),
                rng.range(45.0, 95.0),
            );
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

/// Side of the square the valley's GPU meadow covers, m, and its map's grid points per side.
const MEADOW_SIZE: f32 = 1400.0;
const MEADOW_GRID: u32 = 1001;
/// Zones whose own modules grow grass blades over their ground.
const OWN_GRASS_ZONES: &[&str] = &["forest", "meadow", "footsteps"];

/// How thickly the valley's grass grows at (x, z), 0..1, and how dry it is.
fn meadow_cover(land: &Landscape, x: f64, z: f64) -> (f32, f32) {
    let r = (x * x + z * z).sqrt();
    // (Density blends across a grid cell, about 1.4 m, so this keeps blades off the bare
    // ground while the grass still grows right up to it.)
    if r > MEADOW_SIZE as f64 * 0.5 || in_hard_zone(x, z, 0.7) {
        return (0.0, 0.0);
    }
    if ZONES.iter().any(|zn| {
        OWN_GRASS_ZONES.contains(&zn.key)
            && DVec2::new(x - zn.center.x, z - zn.center.z).length() < zn.radius
    }) {
        return (0.0, 0.0);
    }
    // (Below the lake's level only counts by the lake: the valley floor dips under it too.)
    if land.lake_t(x, z) < 1.3 && land.lake_depth(x, z) > -0.3 {
        return (0.0, 0.0);
    }
    let h = land.height(x, z);
    let cover = land.cover(x, z, h, land.normal(x, z, 1.5));
    let mut density = match cover {
        Cover::Grass => 1.0,
        // Thin grass in the woods' shade.
        Cover::Forest => 0.3,
        _ => return (0.0, 0.0),
    };
    // Crop fields are their own, with grass along their margins; young green crops are short,
    // sparse blades all over (low density grows them short).
    if let Some((crop, edge)) = land.field_at(x, z)
        && crop != Crop::Pasture
    {
        let inside = if crop == Crop::Young { 0.4 } else { 0.0 };
        density *= 1.0 - (1.0 - inside) * engine::core::math::smoothstep(1.2, 2.4, edge as f32);
        if crop == Crop::Young {
            return (density, 0.0);
        }
    }
    // Thinner on the high, rocky pastures and in patches.
    let patches = engine::core::math::noise::perlin2(x / 13.0, z / 13.0, 91) * 0.5 + 0.5;
    let high = engine::core::math::smoothstep(250.0, 600.0, h as f32);
    density *= (1.0 - high * 0.6) * (0.65 + 0.35 * patches);
    (density, land.grass_dryness(x, z))
}

/// The valley's meadows as GPU grass: one field of blades over the whole valley floor, thick
/// on the pastures, thin under the trees and along the field margins, none on roads, water,
/// fields and paved zones; yellowing where the ground paint shows hay and dry grass.
/// The GPU meadow's field: heights, density and dryness on a grid over the valley (a million
/// points, so the exe maps it on a worker while the world builds).
pub fn meadow_grass(land: &Landscape) -> GrassField {
    let t0 = std::time::Instant::now();
    // A million grid points: sampled a band of rows per core.
    let n = MEADOW_GRID as usize;
    let mut maps = vec![(0.0f32, 0.0f32, 0.0f32); n * n];
    let threads = std::thread::available_parallelism().map_or(4, |c| c.get());
    let rows_per = n.div_ceil(threads);
    std::thread::scope(|scope| {
        for (band, chunk) in maps.chunks_mut(rows_per * n).enumerate() {
            scope.spawn(move || {
                for (k, out) in chunk.iter_mut().enumerate() {
                    let (i, j) = (k % n, band * rows_per + k / n);
                    let x = (i as f64 / (n - 1) as f64 - 0.5) * MEADOW_SIZE as f64;
                    let z = (j as f64 / (n - 1) as f64 - 0.5) * MEADOW_SIZE as f64;
                    let (density, dryness) = meadow_cover(land, x, z);
                    // Roots a touch into the ground, so the terrain's coarser rows far off
                    // don't leave blades hanging.
                    *out = (land.height(x, z) as f32 - 0.03, density, dryness);
                }
            });
        }
    });
    let field = GrassField {
        size: [MEADOW_SIZE; 2],
        resolution: [MEADOW_GRID; 2],
        heights: maps.iter().map(|m| m.0).collect(),
        density: maps.iter().map(|m| m.1).collect(),
        dryness: maps.iter().map(|m| m.2).collect(),
        ..Default::default()
    }
    .with_style(GrassStyle {
        blades_per_m2: 520.0,
        height: 0.36,
        height_variation: 0.55,
        width: 0.011,
        bend: 0.38,
        // The ground paint's meadow greens and hay, a little brighter for the blades' tips.
        // (Lighter than the paint: the blades shade each other.)
        lush_color: [0.09, 0.18, 0.042],
        dry_color: [0.24, 0.21, 0.085],
        // Far enough that the meadow thins out into the painted ground, not at a line.
        draw_distance: 120.0,
        shadow_distance: 22.0,
        ..Default::default()
    });
    let covered = field.density.iter().filter(|&&d| d > 0.05).count();
    engine::core::log::info!(
        "scenery: GPU meadow {:.0} m across, grass on {:.0}% of it, mapped in {:.2} s",
        MEADOW_SIZE,
        100.0 * covered as f32 / field.density.len() as f32,
        t0.elapsed().as_secs_f32()
    );
    field
}

/// Near a scarp, where the cliff builds its own rock face and talus.
fn near_scarp(land: &Landscape, x: f64, z: f64) -> bool {
    land.scarps.iter().any(|sc| {
        let (along, side) = sc.frame(DVec2::new(x, z), land.seed);
        let len = (sc.b - sc.a).length();
        (-5.0..len + 5.0).contains(&along) && (-14.0..22.0).contains(&side)
    })
}

/// Boulders on the hills and in the woods, smaller stones wherever the ground is rocky.
pub fn spawn_rocks(commands: &mut Commands, lib: &Library, land: &Landscape, rng: &mut Rng) {
    // Shapes, scales and tilts come from their own stream, so the shared one advances exactly
    // as it always has (everything spawned after the rocks keeps its layout).
    let mut pick = Rng::new(land.seed ^ 0x0b01_d3e5);
    let kit = &lib.rocks;
    let mut count = 0;
    for _ in 0..2600 {
        let p = rng.in_unit_disk() * 1400.0;
        let (x, z) = (p.x as f64, p.y as f64);
        if in_zone(x, z, 5.0)
            || land.road_weight(x, z) > 0.0
            || crate::places::near_lake_trail(x, z, 3.0)
        {
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
        let _ = rng.range_i32(0, 6);
        let yaw = rng.range(0.0, TAU);
        // Not on the zones' pads either (the runway runs past the airfield's circle). Checked
        // after the draws, so the layout spawned after the rocks stays as it is.
        if near_scarp(land, x, z) || land.pad_weight(x, z) > 0.0 {
            continue;
        }
        let (size, scale) = if big {
            if s > 1.9 || pick.chance(0.4) {
                (RockSize::Large, 0.75 + (s - 1.2) / 2.3 * 0.95)
            } else {
                (RockSize::Medium, pick.range(1.0, 1.7))
            }
        } else if pick.chance(0.4) {
            (RockSize::Medium, s * pick.range(0.8, 1.1))
        } else {
            (RockSize::Small, s * pick.range(1.6, 2.6))
        };
        let v = kit.pick(size, pick.chance(0.2), &mut pick);
        let squash = pick.range(0.8, 1.1);
        // Settle into the slope: tilt partly with the ground and sink the downhill edge.
        let tilt = Quat::from_rotation_arc(Vec3::Y, n.lerp(Vec3::Y, 0.35).normalize());
        let slope = (1.0 - n.y * n.y).max(0.0).sqrt() / n.y.max(0.3);
        let r = v.extent.x.max(v.extent.z) * scale;
        let sink = v.extent.y * scale * squash * pick.range(0.08, 0.2) + r * slope * 0.35;
        let tf = Transform::from_translation(DVec3::new(x, h - sink as f64, z))
            .with_rotation(tilt * Quat::from_rotation_y(yaw))
            .with_scale(Vec3::new(
                scale,
                scale * squash,
                scale * pick.range(0.9, 1.1),
            ));
        kit.spawn(
            commands,
            v,
            tf,
            RockLook {
                wet: false,
                moss: pick.chance(0.85),
            },
        );
        count += 1;
        if big && near_wildlife(x, z) {
            solid(
                commands,
                DVec3::new(x, h, z),
                r * 0.75,
                v.extent.y * scale * squash,
            );
        }
    }
    engine::core::log::info!("scenery: {count} boulders and stones");
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

/// The cliff along each scarp (layered rock with the waterfall's notch and plunge pool), talus
/// blocks and scree at its foot, stones along the stream's banks and boulders by the falls.
pub fn spawn_cliffs_and_stream(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    lib: &Library,
    land: &Landscape,
    rng: &mut Rng,
) {
    let t0 = std::time::Instant::now();
    let kit = &lib.rocks;
    let cliff = kit.take_cliff(land);
    let (mut near_tris, mut far_tris) = (0, 0);
    let mut talus_count = 0;
    let rock_top = |x: f64, z: f64| cliff.rock_top(land, x, z);
    let mut local = Rng::new(land.seed ^ 0x00c1_1ff5);
    for spot in &cliff.talus {
        // Scree near the face, bigger blocks further out (they roll further).
        let roll = local.f32();
        let size = if spot.reach > 0.5 && roll < 0.2 {
            RockSize::Large
        } else if roll < 0.5 {
            RockSize::Medium
        } else {
            RockSize::Small
        };
        let v = kit.pick(size, local.chance(0.7), &mut local);
        let scale = match size {
            RockSize::Large => local.range(0.45, 0.9),
            RockSize::Medium => local.range(0.6, 1.3),
            RockSize::Small => local.range(0.8, 2.2),
        };
        let up = spot.normal.lerp(Vec3::Y, 0.4).normalize();
        let tilt = Quat::from_rotation_arc(Vec3::Y, up)
            * Quat::from_rotation_x(local.range(-0.25, 0.25))
            * Quat::from_rotation_z(local.range(-0.25, 0.25));
        let squash = local.range(0.75, 1.05);
        let sink = v.extent.y * scale * squash * local.range(0.15, 0.4);
        let tf = Transform::from_translation(spot.pos - DVec3::Y * sink as f64)
            .with_rotation(tilt * Quat::from_rotation_y(local.range(0.0, TAU)))
            .with_scale(Vec3::new(
                scale,
                scale * squash,
                scale * local.range(0.85, 1.15),
            ));
        let look = RockLook {
            wet: spot.wet,
            moss: local.chance(0.45),
        };
        kit.spawn(commands, v, tf, look);
        talus_count += 1;
        // Scree: a few stones round each block.
        for _ in 0..local.range_i32(1, 4) {
            let a = local.range(0.0, TAU);
            let d = local.range(0.5, 1.6);
            let (x, z) = (
                spot.pos.x + (a.cos() * d) as f64,
                spot.pos.z + (a.sin() * d) as f64,
            );
            let Some(y) = rock_top(x, z) else {
                continue;
            };
            let v = kit.pick(RockSize::Small, local.chance(0.7), &mut local);
            let scale = local.range(0.5, 1.4);
            let tf = Transform::from_translation(DVec3::new(
                x,
                y - (v.extent.y * scale * 0.3) as f64,
                z,
            ))
            .with_rotation(
                Quat::from_rotation_arc(Vec3::Y, spot.normal)
                    * Quat::from_rotation_y(local.range(0.0, TAU)),
            )
            .with_scale(Vec3::new(scale, scale * local.range(0.6, 1.0), scale));
            let look = RockLook {
                wet: spot.wet,
                moss: false,
            };
            kit.spawn(commands, v, tf, look);
            talus_count += 1;
        }
    }
    for st in &land.streams {
        for (i, p) in st.path.iter().enumerate().step_by(5) {
            if !rng.chance(0.6) || i + 1 >= st.path.len() {
                continue;
            }
            let dir = (st.path[i + 1] - *p).normalize_or(DVec2::Y);
            let side = DVec2::new(-dir.y, dir.x) * if rng.chance(0.5) { 1.0 } else { -1.0 };
            let off = rng.range(0.3, 1.5);
            let q = *p + side * (st.width * 0.5 + off as f64);
            let s = rng.range(0.5, 1.4);
            let _ = rng.range_i32(0, 6);
            let yaw = rng.range(0.0, TAU);
            let (size, scale) = if s > 1.0 {
                (RockSize::Medium, s * 0.75)
            } else {
                (RockSize::Small, s * 1.8)
            };
            let v = kit.pick(size, false, &mut local);
            let ground = land.height(q.x, q.y);
            let y = rock_top(q.x, q.y).unwrap_or(ground);
            if cliff.covers(q.x, q.y) && y < ground + 0.05 {
                continue;
            }
            let sink = v.extent.y * scale * 0.3;
            let tf = Transform::from_translation(DVec3::new(q.x, y - sink as f64, q.y))
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::new(scale, scale * local.range(0.7, 1.0), scale));
            // At the water's edge they are wet; further up the bank, dry and mossy.
            let look = RockLook {
                wet: off < 1.0,
                moss: off >= 1.0,
            };
            kit.spawn(commands, v, tf, look);
        }
        let (Some((top, bottom)), Some((pool, _))) = (st.falls(), st.pool) else {
            continue;
        };
        // Boulders on the rock beside the channel above the lip...
        let lip = st.path[top];
        let dir = (st.path[top + 1] - lip).normalize_or(DVec2::Y);
        let side = DVec2::new(-dir.y, dir.x);
        for s in [-1.0, 1.0] {
            for k in 0..2 {
                let q = lip + side * s * (st.width * 0.5 + 2.8 + k as f64 * 2.4)
                    - dir * (2.0 + k as f64 * 3.0 + local.range(0.0, 1.5) as f64);
                let Some(y) = rock_top(q.x, q.y) else {
                    continue;
                };
                let v = kit.pick(RockSize::Large, false, &mut local);
                let scale = local.range(0.45, 0.7);
                let tf = Transform::from_translation(DVec3::new(
                    q.x,
                    y - (v.extent.y * scale * 0.25) as f64,
                    q.y,
                ))
                .with_rotation(Quat::from_rotation_y(local.range(0.0, TAU)))
                .with_scale(Vec3::splat(scale));
                let look = RockLook {
                    wet: false,
                    moss: true,
                };
                kit.spawn(commands, v, tf, look);
            }
        }
        // ...and fallen blocks round the plunge pool, clear of the outflow.
        let out = (st.path[(bottom + 8).min(st.path.len() - 1)] - pool).normalize_or(DVec2::Y);
        for k in 0..5 {
            let a = (k as f64 - 2.0) * 0.55 + local.range(-0.15, 0.15) as f64;
            let toward = DVec2::new(
                out.x * a.cos() - out.y * a.sin(),
                out.x * a.sin() + out.y * a.cos(),
            );
            let flip = if k % 2 == 0 { 1.0 } else { -1.0 };
            let q = pool + DVec2::new(-toward.y, toward.x) * flip * local.range(5.8, 7.5) as f64;
            if land
                .stream_at(q.x, q.y)
                .is_some_and(|(d, _)| d < st.width * 0.5 + 2.2)
            {
                continue;
            }
            let size = if k < 2 {
                RockSize::Large
            } else {
                RockSize::Medium
            };
            let v = kit.pick(size, true, &mut local);
            let scale = local.range(0.5, 0.8);
            let ground = rock_top(q.x, q.y).unwrap_or_else(|| land.height(q.x, q.y));
            let tf = Transform::from_translation(DVec3::new(
                q.x,
                ground - (v.extent.y * scale * 0.3) as f64,
                q.y,
            ))
            .with_rotation(
                Quat::from_rotation_y(local.range(0.0, TAU))
                    * Quat::from_rotation_x(local.range(-0.2, 0.2)),
            )
            .with_scale(Vec3::splat(scale));
            let look = RockLook {
                wet: true,
                moss: false,
            };
            kit.spawn(commands, v, tf, look);
        }
    }
    for ch in cliff.chunks {
        near_tris += ch.near.triangle_count() + ch.wet.triangle_count();
        far_tris += ch.far.triangle_count();
        let tf = Transform::from_translation(ch.origin);
        let switch = 160.0;
        let near = meshes.add(ch.near);
        let far = meshes.add(ch.far);
        commands.spawn((
            tf,
            MeshRenderer::new(near, kit.mats.rock),
            Lod {
                near,
                far: Some(far),
                switch,
                hide: f32::INFINITY,
                far_shown: false,
            },
        ));
        if !ch.wet.indices.is_empty() {
            let wet = meshes.add(ch.wet);
            commands.spawn((
                tf,
                MeshRenderer::new(wet, kit.mats.wet),
                Lod::hide_beyond(wet, switch),
            ));
        }
        if !ch.moss.indices.is_empty() {
            let moss = meshes.add(ch.moss);
            commands.spawn((
                tf,
                MeshRenderer::new(moss, kit.mats.moss).without_shadows(),
                Lod::hide_beyond(moss, switch * 0.75),
            ));
        }
    }
    for st in &land.streams {
        spawn_falls_mist(commands, st);
    }
    engine::core::log::info!(
        "scenery: cliff {near_tris} triangles near, {far_tris} far, {talus_count} talus rocks, built in {:.2} s",
        t0.elapsed().as_secs_f32()
    );
}

/// Spray mist where the falls land: a dense, wispy cloud over the plunge pool and a faint haze
/// drifting off downstream, lit by the volumetric fog (sunbeams catch it, the cliff shades it).
fn spawn_falls_mist(commands: &mut Commands, st: &crate::landscape::Stream) {
    let Some((top, bottom)) = st.falls() else {
        return;
    };
    let drop = (st.bed[top] - st.bed[bottom]) as f32;
    let land_at = (bottom + 1).min(st.path.len() - 1);
    let downstream =
        (st.path[(bottom + 12).min(st.path.len() - 1)] - st.path[land_at]).normalize_or(DVec2::Y);
    let surface = st.bed[land_at] + st.depth * 0.55;
    let at = |along: f64, up: f64| {
        let p = st.path[land_at] + downstream * along;
        Transform::from_translation(DVec3::new(p.x, surface + up, p.y))
    };
    let spray = [0.92, 0.94, 0.97];
    commands.spawn((
        at(1.5, 1.6),
        FogVolume::ellipsoid([5.0, (0.3 * drop).clamp(2.0, 4.0), 5.0])
            .with_density(0.06)
            .with_albedo(spray)
            .with_edge_falloff(2.0)
            .with_noise(0.8, 1.4)
            .with_anisotropy(0.6),
    ));
    commands.spawn((
        at(9.0, 2.5),
        FogVolume::ellipsoid([10.0, 4.0, 10.0])
            .with_density(0.012)
            .with_albedo(spray)
            .with_edge_falloff(4.0)
            .with_noise(0.7, 3.0)
            .with_anisotropy(0.6),
    ));
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
/// village. Returns the hedged edges (corner to corner), which get no wall.
pub fn spawn_farms(
    commands: &mut Commands,
    lib: &Library,
    land: &Landscape,
    rng: &mut Rng,
) -> Vec<(DVec2, DVec2)> {
    let mut hedged = Vec::new();
    for (cell, crop) in &land.fields {
        let corners = field_corners(*cell);
        // Hedges on some edges.
        for k in 0..4 {
            if rng.chance(0.4) {
                continue;
            }
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            hedged.push((a, b));
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
    hedged
}

/// Field grid cells within this many cells of the middle can be walled.
const WALL_CELLS: i32 = 17;
/// Walls end here, m from the middle of the valley.
const WALL_RADIUS: f64 = 700.0;
/// The stones show within this distance, m (virtual geometry thins them out on the way).
const WALL_STONES: f32 = 260.0;
/// The core turns stone-colored beyond this distance, m, where the chinks no longer show; past
/// [`WALL_STONES`] it stands for the whole wall.
const WALL_MID: f32 = 160.0;
/// Shorter runs of wall than this many segments are left out: stubs read as rubble.
const WALL_MIN_RUN: usize = 3;
/// Wall colliders exist within this distance of a focus point, m (and go beyond it plus
/// [`WALL_COLLIDER_SLACK`]): there are well over a thousand segments in the valley, and every
/// collider added or removed anywhere has the NPC tactical world rebuilt.
const WALL_COLLIDER_DISTANCE: f64 = 150.0;
const WALL_COLLIDER_SLACK: f64 = 20.0;

/// One straight dry-stone wall segment, laid on the ground.
#[derive(Debug, Clone, Copy)]
pub struct WallSegment {
    /// Middle of the segment's foot, on the ground.
    pub pos: DVec3,
    /// Turns the segment's build frame (along X) onto the ground, following its slope.
    pub rot: Quat,
    /// Length along the slope, m.
    pub len: f32,
    /// Which segment variant, and whether it is mirrored.
    pub variant: usize,
    pub mirrored: bool,
    /// The first or last of its run: built with room for the head at its +X end (mirrored
    /// for the first, so that end is the run's start).
    pub end: bool,
}

/// The head ending a run of wall: its outer face at `pos`, facing along `rot`'s X.
#[derive(Debug, Clone, Copy)]
pub struct WallHeadPlace {
    pub pos: DVec3,
    pub rot: Quat,
    pub variant: usize,
}

/// Where the dry-stone walls go.
#[derive(Debug, Default)]
pub struct WallPlan {
    pub segments: Vec<WallSegment>,
    pub heads: Vec<WallHeadPlace>,
}

/// Where the dry-stone walls go: along the field grid, on most edges of the farm fields that
/// have no hedge and on many between the open pastures around them, so the valley floor reads
/// as an old enclosed patchwork. Gateways here and there; none across roads, streams, zones,
/// the lake or steep ground. Every run is at least [`WALL_MIN_RUN`] segments long and ends in
/// a head at both ends.
pub fn plan_walls(
    land: &Landscape,
    hedged: &[(DVec2, DVec2)],
    variants: usize,
    head_variants: usize,
) -> WallPlan {
    use crate::walls::SEGMENT;
    // Its own stream, so the scenery spawned after the walls keeps its layout.
    let mut rng = Rng::new(land.seed ^ 0x5707_e3a1);
    let is_field = |cell: (i32, i32)| land.fields.iter().any(|f| f.0 == cell);
    // Gentle, open ground a pasture could be on.
    let open = |cell: (i32, i32)| {
        let c = field_corners(cell);
        let center = (c[0] + c[2]) * 0.5;
        let r = center.length();
        let heights = c.map(|p| land.height(p.x, p.y));
        let relief = heights.iter().cloned().fold(f64::MIN, f64::max)
            - heights.iter().cloned().fold(f64::MAX, f64::min);
        (110.0..WALL_RADIUS).contains(&r)
            && relief < 9.0
            && land.woodland(center.x, center.y) < 0.3
            && !in_zone(center.x, center.y, 10.0)
            && (center - land.lake.center).length() > land.lake.radius * 1.6
            && (center - land.cave.center).length() > land.cave.hill_radius
    };
    let is_hedged = |a: DVec2, b: DVec2| {
        hedged.iter().any(|&(p, q)| {
            (p.distance(a) < 0.5 && q.distance(b) < 0.5)
                || (p.distance(b) < 0.5 && q.distance(a) < 0.5)
        })
    };
    let blocked = |p: DVec2| {
        land.road_weight(p.x, p.y) > 0.0
            || in_zone(p.x, p.y, 6.0)
            || land.pad_weight(p.x, p.y) > 0.5
            || (land.lake_t(p.x, p.y) < 1.3 && land.lake_depth(p.x, p.y) > -0.6)
            || p.length() > WALL_RADIUS + 40.0
    };
    let pick = |run: u64, i: u64, salt: u64, n: usize| {
        ((crate::walls::segment_hash(run * 4 + salt, i) * n as f32) as usize).min(n.max(1) - 1)
    };
    let mut plan = WallPlan::default();
    for iu in -WALL_CELLS..=WALL_CELLS {
        for iv in -WALL_CELLS..=WALL_CELLS {
            let cell = (iu, iv);
            let c = field_corners(cell);
            // The cell's edges on its low-v and low-u sides: each grid edge once.
            for (a, b, other) in [(c[0], c[1], (iu, iv - 1)), (c[3], c[0], (iu - 1, iv))] {
                let fields = is_field(cell) as u8 + is_field(other) as u8;
                let keep = match fields {
                    0 if open(cell) && open(other) => 0.6,
                    0 => 0.0,
                    _ if open(cell) || open(other) || fields == 2 => 0.85,
                    _ => 0.5,
                };
                // Draw first, so the stream doesn't depend on the checks.
                let roll = rng.f32();
                let gate = rng.chance(0.4);
                let gate_at = rng.f32();
                if roll >= keep || is_hedged(a, b) {
                    continue;
                }
                let len = (b - a).length();
                let n = (len / SEGMENT as f64).ceil() as i32;
                let step = len / n as f64;
                let dir = (b - a) / len;
                let gate = gate.then(|| 1 + (gate_at * (n - 2) as f32) as i32);
                let yaw = Quat::from_rotation_y((-dir.y).atan2(dir.x) as f32);
                let run_id = ((iu + 100) * 1000 + iv + 100) as u64 * 2 + (other.0 != iu) as u64;
                // Each segment that can stand, then the runs of them long enough to keep.
                let laid: Vec<Option<(DVec3, Quat, f32)>> = (0..n)
                    .map(|i| {
                        let p0 = a + dir * (i as f64 * step);
                        let p1 = p0 + dir * step;
                        let pm = (p0 + p1) * 0.5;
                        if gate == Some(i)
                            || [p0, pm, p1].into_iter().any(blocked)
                            || near_scarp(land, pm.x, pm.y)
                        {
                            return None;
                        }
                        let (h0, h1) = (land.height(p0.x, p0.y), land.height(p1.x, p1.y));
                        if (h1 - h0).abs() > step * 0.4 {
                            return None;
                        }
                        let hm = land.height(pm.x, pm.y);
                        let pitch = Quat::from_rotation_z(((h1 - h0) / step).atan() as f32);
                        let y = ((h0 + h1) * 0.5).min(hm + 0.05);
                        // Pitched along the slope, it must be as long as the slope is, or
                        // neighbors part on a grade.
                        let len = (step * step + (h1 - h0) * (h1 - h0)).sqrt() as f32;
                        Some((DVec3::new(pm.x, y, pm.y), yaw * pitch, len))
                    })
                    .collect();
                let mut i = 0;
                while i < laid.len() {
                    let start = i;
                    while i < laid.len() && laid[i].is_some() {
                        i += 1;
                    }
                    let run = &laid[start..i];
                    i += 1;
                    if run.len() < WALL_MIN_RUN {
                        continue;
                    }
                    let last = run.len() - 1;
                    for (k, &(pos, rot, len)) in run.iter().flatten().enumerate() {
                        let idx = (start + k) as u64;
                        let variant = pick(run_id, idx, 0, variants);
                        let (end, mirrored) = match k {
                            0 => (true, true),
                            k if k == last => (true, false),
                            // Mirrored half the time, for more variety from a few variants.
                            _ => (false, (variant + idx as usize) % 2 == 1),
                        };
                        plan.segments.push(WallSegment {
                            pos,
                            rot,
                            len,
                            variant,
                            mirrored,
                            end,
                        });
                    }
                    let (p_first, r_first, l_first) = run[0].expect("a run is laid segments");
                    let (p_last, r_last, l_last) = run[last].expect("a run is laid segments");
                    for (pos, rot, len, salt) in [
                        (
                            p_first,
                            r_first * Quat::from_rotation_y(std::f32::consts::PI),
                            l_first,
                            1,
                        ),
                        (p_last, r_last, l_last, 2),
                    ] {
                        let x = (rot * Vec3::X * len * 0.5).as_dvec3();
                        plan.heads.push(WallHeadPlace {
                            pos: pos + x,
                            rot,
                            variant: pick(run_id, start as u64, salt, head_variants),
                        });
                    }
                }
            }
        }
    }
    plan
}

/// The hero stones' palette: Rendering's dressed wall stones, a few of each class, their
/// relief built (and cached on disk) by virtual geometry the first time one is drawn.
/// The hero stones' base meshes and displacements (built on a worker).
fn palette_stones() -> Vec<(Mesh, Displacement)> {
    use crate::walls::{HERO_CLASSES, HERO_VARIANTS};
    use engine::render::rock::{StoneKind, WallStone};
    let mut stones = Vec::with_capacity(HERO_CLASSES.len() * HERO_VARIANTS);
    for (c, &(kind, size)) in HERO_CLASSES.iter().enumerate() {
        // Lichen likes the dry tops: the coping and the capstones.
        let lichen = if matches!(c, 4 | 6) { 0.65 } else { 0.45 };
        for v in 0..HERO_VARIANTS {
            // One face stone in four was picked square rather than split: rounder, worn arrises.
            let kind = if c < 3 && v == HERO_VARIANTS - 1 {
                StoneKind::RoughHewn
            } else {
                kind
            };
            let stone = WallStone::new(kind, size, 7000 + (c * HERO_VARIANTS + v) as u32)
                .with_lichen(lichen)
                .with_grime(0.7);
            stones.push((stone.base_mesh(), stone.displacement()));
        }
    }
    stones
}

fn wall_palette(
    stones: Vec<(Mesh, Displacement)>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> WallPalette {
    let stones = stones
        .into_iter()
        .map(|(mesh, displacement)| (meshes.add(mesh), displacement))
        .collect();
    // Each stone a little lighter or darker, warmer or cooler than its palette mesh.
    let shades = [
        [0.94, 0.94, 0.93],
        [0.8, 0.79, 0.77],
        [1.0, 0.95, 0.86],
        [0.7, 0.71, 0.72],
        [0.62, 0.62, 0.58],
        [0.88, 0.84, 0.78],
    ]
    .iter()
    .map(|&[r, g, b]| {
        materials.add(
            StandardMaterial::color(r, g, b)
                .with_roughness(0.88)
                .with_detail(0.35, 0.004),
        )
    })
    .collect();
    WallPalette { stones, shades }
}

/// Hero stones are drawn within this distance of the camera, m; the merged rocks beyond it.
const HERO_IN: f32 = 30.0;
/// Hero stones are spawned this close, so their relief is ready by [`HERO_IN`], m...
const HERO_SPAWN: f32 = 38.0;
/// ...and despawned beyond this, m.
const HERO_OUT: f32 = 46.0;

/// Time (s) a frame may spend spawning hero stones.
const HERO_BUDGET_S: f32 = 0.003;

/// Hidden closer than this to the camera, m (where something finer stands in).
#[derive(Component, Debug, Clone, Copy)]
pub struct HideWithin(pub f32);

/// A wall segment's or head's stones as hero stones up close ([`stream_wall_heroes`]).
#[derive(Component)]
pub struct WallHeroes {
    placed: Arc<[crate::walls::Placed]>,
    stones: Vec<Entity>,
    shown: bool,
}

impl WallHeroes {
    fn new(placed: Arc<[crate::walls::Placed]>) -> Self {
        Self {
            placed,
            stones: Vec::new(),
            shown: false,
        }
    }
}

/// One hero stone.
#[derive(Component)]
pub struct HeroStone;

/// A placed stone's transform under its segment: its box stretched from the palette stone's,
/// a reflection (the wall's -Z face is laid mirrored) as a negative scale.
fn hero_transform(p: &crate::walls::Placed) -> Transform {
    let m = Mat3::from_cols(p.ax[0], p.ax[1], p.ax[2]);
    let mirror = m.determinant() < 0.0;
    let rot = if mirror {
        Mat3::from_cols(p.ax[0], p.ax[1], -p.ax[2])
    } else {
        m
    };
    let mut scale = p.size / crate::walls::HERO_CLASSES[p.class as usize].1;
    if mirror {
        scale.z = -scale.z;
    }
    Transform::from_translation(p.center.as_dvec3())
        .with_rotation(Quat::from_mat3(&rot))
        .with_scale(scale)
}

/// Swaps wall segments and heads near the camera to hero stones (spawned a little before they
/// show, despawned well after) four times a second, and shows them exactly where the merged
/// rocks hide ([`HideWithin`]).
#[allow(clippy::type_complexity)]
pub fn stream_wall_heroes(
    mut commands: Commands,
    mut timer: Local<f32>,
    time: Res<Time>,
    palette: Option<Res<WallPalette>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut walls: Query<(Entity, &GlobalTransform, &mut WallHeroes)>,
    mut stones: Query<&mut MeshRenderer, With<HeroStone>>,
) {
    let Some(palette) = palette else {
        return;
    };
    let Some(eye) = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    else {
        return;
    };
    *timer -= time.real_delta_secs();
    let restream = *timer <= 0.0;
    if restream {
        *timer = 0.25;
    }
    // Walls spawn their stones until this run's budget is spent; the rest go next frame, so
    // arriving among many walls (the first frame, a jump) never stalls a frame for long.
    let started = std::time::Instant::now();
    for (e, t, mut w) in &mut walls {
        let d2 = (t.translation() - eye).length_squared();
        // As `update_lods` decides the merged rocks' `HideWithin`.
        let show = d2 < (HERO_IN as f64).powi(2);
        if restream && w.stones.is_empty() && d2 < (HERO_SPAWN as f64).powi(2) {
            if started.elapsed().as_secs_f32() > HERO_BUDGET_S {
                *timer = 0.0;
                continue;
            }
            let mut spawned = Vec::with_capacity(w.placed.len());
            for p in w.placed.iter() {
                let (mesh, displacement) = &palette.stones
                    [p.class as usize * crate::walls::HERO_VARIANTS + p.variant as usize];
                let mut r = MeshRenderer::new(*mesh, palette.shades[p.shade as usize]);
                r.visible = show;
                spawned.push(
                    commands
                        .spawn((
                            hero_transform(p),
                            r,
                            displacement.clone(),
                            HeroStone,
                            ChildOf(e),
                        ))
                        .id(),
                );
            }
            w.stones = spawned;
            w.shown = show;
            continue;
        }
        if restream && !w.stones.is_empty() && d2 > (HERO_OUT as f64).powi(2) {
            for s in w.stones.drain(..) {
                commands.entity(s).despawn();
            }
            continue;
        }
        if show != w.shown && !w.stones.is_empty() {
            for &s in &w.stones {
                if let Ok(mut r) = stones.get_mut(s) {
                    r.visible = show;
                }
            }
            w.shown = show;
        }
    }
}

/// A wall segment's box collider, there while a focus point is near ([`stream_wall_colliders`]).
#[derive(Component, Clone, Copy)]
pub struct WallCollider {
    pub half: Vec3,
}

/// The dry-stone walls ([`plan_walls`]): the stones, the core, a box collider per segment
/// (streamed in near the action) and the heads that end the runs.
pub fn spawn_walls(
    commands: &mut Commands,
    lib: &Library,
    land: &Landscape,
    hedged: &[(DVec2, DVec2)],
) {
    use crate::walls::{HALF_FOOT, HEIGHT, SEGMENT, SINK};
    let plan = plan_walls(land, hedged, lib.walls.len(), lib.wall_heads.len());
    // The boulders' rock: albedo, normal and occlusion maps under each stone's own tint.
    let stone = lib.rocks.mats.rock;
    for seg in &plan.segments {
        let w = &lib.walls[seg.variant];
        let flip = if seg.mirrored { -1.0 } else { 1.0 };
        let tf = Transform::from_translation(seg.pos)
            .with_rotation(seg.rot)
            .with_scale(Vec3::new(flip * seg.len / SEGMENT, 1.0, 1.0));
        let (near, core) = if seg.end {
            (w.near_end, w.core_end)
        } else {
            (w.near, w.core)
        };
        let placed = if seg.end {
            w.placed_end.clone()
        } else {
            w.placed.clone()
        };
        commands.spawn((
            tf,
            MeshRenderer::new(near, stone),
            Lod::hide_beyond(near, WALL_STONES),
            HideWithin(HERO_IN),
            WallHeroes::new(placed),
        ));
        commands.spawn((
            tf,
            MeshRenderer::new(core, stone),
            Lod::swap_then_hide(core, w.core_far, WALL_MID, f32::INFINITY),
        ));
        let mid_y = (HEIGHT - SINK) * 0.5;
        commands.spawn((
            Transform::from_translation(seg.pos + (seg.rot * Vec3::Y * mid_y).as_dvec3())
                .with_rotation(seg.rot),
            WallCollider {
                half: Vec3::new(seg.len * 0.5, (HEIGHT + SINK) * 0.5, HALF_FOOT * 0.75),
            },
        ));
    }
    for head in &plan.heads {
        let near = lib.wall_heads[head.variant];
        commands.spawn((
            Transform::from_translation(head.pos).with_rotation(head.rot),
            MeshRenderer::new(near, stone),
            Lod::hide_beyond(near, WALL_STONES),
            HideWithin(HERO_IN),
            WallHeroes::new(lib.wall_head_placed[head.variant].clone()),
        ));
    }
    commands.insert_resource(lib.wall_palette.clone());
    let nearest = plan
        .segments
        .iter()
        .map(|s| s.pos)
        .min_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
        .unwrap_or_default();
    engine::core::log::info!(
        "scenery: {} dry-stone wall segments in {} runs, the nearest to the plaza at ({:.0}, {:.0})",
        plan.segments.len(),
        plan.heads.len() / 2,
        nearest.x,
        nearest.z
    );
}

/// Gives wall segments near a focus point their colliders and takes them from those far off,
/// twice a second.
pub fn stream_wall_colliders(
    mut commands: Commands,
    mut timer: Local<f32>,
    time: Res<Time>,
    focus: Option<Res<engine::world::FocusPoints>>,
    walls: Query<(
        Entity,
        &WallCollider,
        &Transform,
        Has<engine::world::physics::Collider>,
    )>,
) {
    *timer -= time.real_delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.5;
    let Some(focus) = focus.filter(|f| !f.points.is_empty()) else {
        return;
    };
    for (e, wall, t, has) in &walls {
        let d = focus.distance(t.translation);
        if !has && d < WALL_COLLIDER_DISTANCE {
            commands
                .entity(e)
                .insert(engine::world::physics::Collider::cuboid(wall.half));
        } else if has && d > WALL_COLLIDER_DISTANCE + WALL_COLLIDER_SLACK {
            commands
                .entity(e)
                .remove::<engine::world::physics::Collider>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meadow_grows_on_pastures_but_not_on_roads_water_or_paved_zones() {
        let land = crate::landscape::shared();
        let (mut grassy, mut total) = (0, 0);
        for i in -60..=60 {
            for j in -60..=60 {
                let (x, z) = (i as f64 * 10.0 + 3.0, j as f64 * 10.0 + 7.0);
                let (density, dryness) = meadow_cover(land, x, z);
                assert!((0.0..=1.0).contains(&density) && (0.0..=1.0).contains(&dryness));
                total += 1;
                if density > 0.0 {
                    grassy += 1;
                    // Up to the ragged verge of a track, not on it.
                    assert!(
                        !matches!(land.cover_at(x, z), Cover::Dirt | Cover::Cobble),
                        "on a road at {x}, {z}"
                    );
                    assert!(land.lake_t(x, z) > 1.0, "in the lake at {x}, {z}");
                    assert!(!in_hard_zone(x, z, 0.0), "in a paved zone at {x}, {z}");
                }
            }
        }
        // Most of the valley floor is meadow.
        assert!(grassy * 2 > total, "grass on {grassy} of {total} samples");
    }

    #[test]
    fn walls_enclose_the_valley_floor_but_keep_off_roads_zones_and_water() {
        let land = crate::landscape::shared();
        let plan = plan_walls(land, &[], 4, 3);
        assert!(
            plan.segments.len() > 1000,
            "only {} segments",
            plan.segments.len()
        );
        // Two heads a run, and no run shorter than the minimum.
        assert_eq!(plan.heads.len() % 2, 0);
        assert!(plan.segments.len() >= plan.heads.len() / 2 * WALL_MIN_RUN);
        for h in &plan.heads {
            assert!(h.variant < 3);
            assert!(
                !in_zone(h.pos.x, h.pos.z, 3.0),
                "head in a zone at {:?}",
                h.pos
            );
        }
        for s in &plan.segments {
            let (x, z) = (s.pos.x, s.pos.z);
            assert_eq!(land.road_weight(x, z), 0.0, "on a road at {x:.0}, {z:.0}");
            assert!(!in_zone(x, z, 6.0), "in a zone at {x:.0}, {z:.0}");
            assert!(land.lake_t(x, z) > 1.2, "in the lake at {x:.0}, {z:.0}");
            assert!(s.variant < 4);
            assert!(
                (s.pos.y - land.height(x, z)).abs() < 0.5,
                "off the ground at {x:.0}, {z:.0}"
            );
        }
    }
}
