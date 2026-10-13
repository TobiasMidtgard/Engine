//! Shared assets of the yard: procedural textures and materials, made once per world, and
//! multi-part models whose meshes are registered under asset keys so every instance of a
//! prop shares one mesh per material (one instanced draw per part).
//!
//! Scene code builds a [`Model`] with a [`ModelBuilder`] (meshes grouped by [`Mat`]) and
//! spawns it as often as it likes; [`ensure_materials`] runs (queued) before anything is
//! drawn and fills the material keys the parts refer to.

use std::collections::BTreeMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial};
use engine_world::physics::{Collider, ColliderShape, RigidBody};

use super::meshkit::grime;
use super::texgen::{self, TexSet};

/// Materials of the yard's props.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mat {
    /// Weathered structural timber (frames, towers, A-frames).
    Timber,
    /// Dark-stained timber (the curtain stage).
    TimberDark,
    /// Light pine (crates, seats, treads).
    Pine,
    /// Weathered crane-yellow paint on steel.
    PaintYellow,
    /// Olive drab paint on steel.
    PaintGreen,
    /// Oxide-red paint on steel.
    PaintRed,
    /// Hot-dip galvanized steel.
    Galvanized,
    /// Painted cast iron (machine green).
    CastIron,
    /// Blackened steel hardware.
    BlackSteel,
    /// Machined bright steel.
    BrightSteel,
    /// Dark forged steel (wrecking ball, shackles, chain).
    DarkSteel,
    Brass,
    /// Gilded finials.
    Gold,
    /// Sandstone blocks.
    Stone,
    /// Lime mortar between blocks.
    Mortar,
    Concrete,
    /// Hemp rope (halyards, lashings).
    Rope,
    /// Black rubber (hoses, grips, feet).
    Rubber,
    /// Varnished mahogany: warm red-brown, glossy, with grain (boat trim, transom, rudder).
    Varnish,
    /// Varnished spruce and oak: golden, glossy (spars, frames, oars, mast hoops).
    VarnishLight,
    /// Glossy deep-green topside enamel.
    HullPaint,
    /// Glossy white enamel (the boot-top stripe).
    BootTop,
    /// Cream semi-gloss interior paint.
    InteriorPaint,
    /// Matte red-brown antifouling below the waterline.
    Antifoul,
    /// Cast and polished bronze (boat fittings).
    Bronze,
}

impl Mat {
    /// Every material, in declaration order.
    pub const ALL: [Mat; 25] = [
        Mat::Timber,
        Mat::TimberDark,
        Mat::Pine,
        Mat::PaintYellow,
        Mat::PaintGreen,
        Mat::PaintRed,
        Mat::Galvanized,
        Mat::CastIron,
        Mat::BlackSteel,
        Mat::BrightSteel,
        Mat::DarkSteel,
        Mat::Brass,
        Mat::Gold,
        Mat::Stone,
        Mat::Mortar,
        Mat::Concrete,
        Mat::Rope,
        Mat::Rubber,
        Mat::Varnish,
        Mat::VarnishLight,
        Mat::HullPaint,
        Mat::BootTop,
        Mat::InteriorPaint,
        Mat::Antifoul,
        Mat::Bronze,
    ];

    /// Asset key of this material in `Assets<StandardMaterial>`.
    pub fn key(self) -> String {
        format!("softbody/yard/mat/{self:?}")
    }
}

/// Texture resolution of the yard's materials (texels across one tile).
const WOOD_SIZE: u32 = 1024;
const SIZE: u32 = 512;
const ROPE_SIZE: u32 = 256;
/// The boat's finishes: varnished wood (texels along × across the grain), enamel, antifouling.
const VARNISH_SIZE: (u32, u32) = (1024, 512);
const PAINT_SIZE: u32 = 512;
const ANTIFOUL_SIZE: u32 = 256;

/// Albedo, normal and ORM handles of one texture set.
#[derive(Clone, Copy)]
pub(super) struct Set {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    orm: Handle<Image>,
}

/// Upload a texture set's three images.
pub(super) fn add_set(images: &mut Assets<Image>, t: TexSet) -> Set {
    Set {
        albedo: images.add(t.albedo),
        normal: images.add(t.normal),
        orm: images.add(t.orm),
    }
}

/// A textured material: `tiles_per_m` sets the texture repeat for meshes with meter UVs.
pub(super) fn textured(
    set: Set,
    base: [f32; 3],
    rough: f32,
    metal: f32,
    tiles_per_m: f32,
) -> StandardMaterial {
    StandardMaterial {
        base_color: [base[0], base[1], base[2], 1.0],
        perceptual_roughness: rough,
        metallic: metal,
        base_color_texture: Some(set.albedo),
        normal_texture: Some(set.normal),
        metallic_roughness_texture: Some(set.orm),
        occlusion_texture: Some(set.orm),
        uv_scale: [tiles_per_m, tiles_per_m],
        ..StandardMaterial::default()
    }
}

/// Paint colors (linear RGB) baked into the painted-steel albedos.
const PAINTS: [[f32; 3]; 3] = [
    [0.62, 0.36, 0.015],
    [0.085, 0.10, 0.045],
    [0.24, 0.04, 0.025],
];

/// Make the yard's textures and materials (once per world; later calls return at once).
/// Does nothing without a renderer (`Assets<Image>` missing).
pub fn ensure_materials(world: &mut World) {
    if !world.contains_resource::<Assets<Image>>()
        || !world.contains_resource::<Assets<StandardMaterial>>()
    {
        return;
    }
    if world
        .resource::<Assets<StandardMaterial>>()
        .loaded_by_key(&Mat::Timber.key())
        .is_some()
    {
        return;
    }
    let start = std::time::Instant::now();
    let wood = texgen::wood(WOOD_SIZE);
    let paints: Vec<Vec3> = PAINTS.iter().map(|&c| Vec3::from(c)).collect();
    let (paint_albedos, paint_normal, paint_orm) = texgen::painted_steel(SIZE, &paints);
    let galv = texgen::galvanized(SIZE);
    let iron = texgen::cast_iron(SIZE, Vec3::new(0.035, 0.065, 0.05));
    let metal = texgen::worn_metal(SIZE);
    let stone = texgen::stone(SIZE);
    let concrete = texgen::concrete(SIZE);
    let rope = texgen::rope(ROPE_SIZE);
    let varnish = texgen::varnished_wood(VARNISH_SIZE.0, VARNISH_SIZE.1);
    let enamel = texgen::gloss_paint(PAINT_SIZE);
    let antifoul = texgen::antifoul(ANTIFOUL_SIZE);
    let mut bytes: usize = [
        &wood, &galv, &iron, &metal, &stone, &concrete, &rope, &varnish, &enamel, &antifoul,
    ]
    .iter()
    .map(|t| t.bytes())
    .sum();
    bytes += (paint_albedos.len() + 2) * (SIZE * SIZE * 4) as usize * 4 / 3;

    let mut images = world.resource_mut::<Assets<Image>>();
    let wood = add_set(&mut images, wood);
    let paint_normal = images.add(paint_normal);
    let paint_orm = images.add(paint_orm);
    let paint: Vec<Set> = paint_albedos
        .into_iter()
        .map(|a| Set {
            albedo: images.add(a),
            normal: paint_normal,
            orm: paint_orm,
        })
        .collect();
    let galv = add_set(&mut images, galv);
    let iron = add_set(&mut images, iron);
    let metal = add_set(&mut images, metal);
    let stone = add_set(&mut images, stone);
    let concrete = add_set(&mut images, concrete);
    let rope = add_set(&mut images, rope);
    let varnish = add_set(&mut images, varnish);
    let enamel = add_set(&mut images, enamel);
    let antifoul = add_set(&mut images, antifoul);
    // Varnished wood tiles 1.6 m along the grain (u) and 0.3 m across it (v).
    let varnished = |base: [f32; 3], rough: f32| StandardMaterial {
        uv_scale: [
            1.0 / texgen::VARNISH_TILE_M.0,
            1.0 / texgen::VARNISH_TILE_M.1,
        ],
        ..textured(varnish, base, rough, 0.0, 1.0)
    };

    let mut mats = world.resource_mut::<Assets<StandardMaterial>>();
    for m in Mat::ALL {
        let material = match m {
            Mat::Timber => textured(wood, [0.9, 0.86, 0.8], 1.0, 0.0, 1.0),
            Mat::TimberDark => textured(wood, [0.5, 0.38, 0.29], 0.95, 0.0, 1.0),
            Mat::Pine => textured(wood, [1.0, 0.9, 0.68], 1.0, 0.0, 1.0),
            Mat::PaintYellow => textured(paint[0], [1.0; 3], 1.0, 1.0, 1.0),
            Mat::PaintGreen => textured(paint[1], [1.0; 3], 1.0, 1.0, 1.0),
            Mat::PaintRed => textured(paint[2], [1.0; 3], 1.0, 1.0, 1.0),
            Mat::Galvanized => textured(galv, [1.0; 3], 1.0, 1.0, 2.0),
            Mat::CastIron => textured(iron, [1.0; 3], 1.0, 1.0, 1.0 / 0.6),
            Mat::BlackSteel => textured(metal, [0.07, 0.07, 0.075], 0.6, 0.7, 2.0),
            Mat::BrightSteel => textured(metal, [0.62, 0.62, 0.63], 0.42, 1.0, 2.0),
            Mat::DarkSteel => textured(metal, [0.28, 0.28, 0.29], 0.55, 1.0, 2.0),
            Mat::Brass => textured(metal, [0.80, 0.60, 0.30], 0.42, 1.0, 2.0),
            Mat::Gold => textured(metal, [0.98, 0.76, 0.36], 0.36, 1.0, 2.0),
            Mat::Stone => textured(stone, [1.0; 3], 1.0, 0.0, 1.0),
            Mat::Mortar => textured(concrete, [0.9, 0.86, 0.78], 1.0, 0.0, 1.0 / 1.5),
            Mat::Concrete => textured(concrete, [1.0; 3], 1.0, 0.0, 1.0 / 1.5),
            Mat::Rope => textured(rope, [1.0; 3], 1.0, 0.0, 1.0),
            Mat::Rubber => textured(metal, [0.035, 0.035, 0.035], 0.75, 0.0, 2.0),
            Mat::Varnish => varnished([0.30, 0.12, 0.058], 0.26),
            Mat::VarnishLight => varnished([0.95, 0.63, 0.33], 0.28),
            Mat::HullPaint => textured(enamel, [0.011, 0.05, 0.03], 0.2, 0.0, 1.0),
            Mat::BootTop => textured(enamel, [0.86, 0.86, 0.83], 0.22, 0.0, 1.0),
            Mat::InteriorPaint => textured(enamel, [0.88, 0.74, 0.5], 0.55, 0.0, 1.0),
            Mat::Antifoul => textured(antifoul, [0.24, 0.05, 0.03], 1.0, 0.0, 2.0),
            Mat::Bronze => textured(metal, [0.74, 0.47, 0.24], 0.42, 1.0, 2.0),
        };
        mats.insert_keyed(&m.key(), material);
    }
    engine_core::log::info!(
        "softbody yard: made {} materials ({:.1} MB of textures) in {:.2} s",
        Mat::ALL.len(),
        bytes as f64 / 1e6,
        start.elapsed().as_secs_f64()
    );
}

/// Register `mesh` under `key` (once) when the world has a renderer.
pub fn register_mesh(commands: &mut Commands, key: String, mesh: Mesh) {
    commands.queue(move |world: &mut World| {
        if let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() {
            meshes.get_or_insert_keyed(&key, || mesh);
        }
    });
}

/// A renderer for the mesh registered under `key` with material `mat`.
pub fn renderer(key: &str, mat: Mat) -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Asset(key.to_string()),
        MaterialRef::Asset(mat.key()),
    )
}

/// Collects a prop's meshes by material, then registers them as one [`Model`].
pub struct ModelBuilder {
    name: String,
    parts: BTreeMap<(Mat, bool), Mesh>,
    grounded: bool,
}

impl ModelBuilder {
    /// An empty model; `name` keys its meshes (`softbody/yard/{name}/...`), so it must be
    /// unique per distinct model.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parts: BTreeMap::new(),
            grounded: false,
        }
    }

    /// The prop stands on the ground at its local y = 0: darken everything in the lowest
    /// 0.4 m (dirt splashed up, damp).
    pub fn grounded(mut self) -> Self {
        self.grounded = true;
        self
    }

    /// Add a mesh that casts shadows.
    pub fn add(&mut self, mat: Mat, mesh: Mesh) -> &mut Self {
        self.parts.entry((mat, true)).or_default().merge(&mesh);
        self
    }

    /// Add a small detail (bolt, rivet, ring) that doesn't cast shadows.
    pub fn add_small(&mut self, mat: Mat, mesh: Mesh) -> &mut Self {
        self.parts.entry((mat, false)).or_default().merge(&mesh);
        self
    }

    /// Register the meshes (once per key) and return the model.
    pub fn build(self, commands: &mut Commands) -> Model {
        let (model, meshes) = self.finish();
        for (key, mesh) in meshes {
            register_mesh(commands, key, mesh);
        }
        model
    }

    /// The model and its meshes under their asset keys, without registering them (for
    /// callers that register later or only when a world lacks them).
    pub fn finish(self) -> (Model, Vec<(String, Mesh)>) {
        let triangles: usize = self.parts.values().map(Mesh::triangle_count).sum();
        engine_core::log::debug!(
            "softbody yard: model {} has {triangles} triangles",
            self.name
        );
        let mut parts = Vec::new();
        let mut meshes = Vec::new();
        for ((mat, shadows), mesh) in self.parts {
            if mesh.indices.is_empty() {
                continue;
            }
            let key = format!(
                "softbody/yard/{}/{mat:?}{}",
                self.name,
                if shadows { "" } else { "/small" }
            );
            let mesh = if self.grounded {
                grime(mesh, 0.0, 0.4, 0.68)
            } else {
                mesh
            };
            meshes.push((key.clone(), mesh));
            parts.push((key, mat, shadows));
        }
        (Model { parts }, meshes)
    }
}

/// A registered multi-material prop. Cheap to clone and spawn many times.
#[derive(Debug, Clone, Default)]
pub struct Model {
    parts: Vec<(String, Mat, bool)>,
}

impl Model {
    /// Asset keys of the model's meshes.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().map(|(key, _, _)| key.as_str())
    }

    /// Spawn the parts as children of `root` (at the root's origin, or at `local`).
    pub fn attach(&self, commands: &mut Commands, root: Entity, local: Transform) {
        for (key, mat, shadows) in &self.parts {
            let mut r = renderer(key, *mat);
            r.cast_shadows = *shadows;
            commands.spawn((local, r, ChildOf(root)));
        }
    }

    /// Spawn a root entity at `t` with the parts under it.
    pub fn spawn(&self, commands: &mut Commands, t: Transform) -> Entity {
        let root = commands.spawn(t).id();
        self.attach(commands, root, Transform::default());
        root
    }

    /// Spawn it as fixed (static) geometry soft bodies collide with.
    pub fn spawn_solid(
        &self,
        commands: &mut Commands,
        t: Transform,
        shape: ColliderShape,
    ) -> Entity {
        let root = commands
            .spawn((
                t,
                RigidBody::fixed(),
                Collider {
                    shape,
                    ..Collider::default()
                },
            ))
            .id();
        self.attach(commands, root, Transform::default());
        root
    }
}

/// Box collider shape from full size.
pub fn cuboid(size: Vec3) -> ColliderShape {
    ColliderShape::Cuboid {
        half_extents: size * 0.5,
    }
}

/// Cylinder collider shape (along Y).
pub fn cylinder(radius: f32, height: f32) -> ColliderShape {
    ColliderShape::Cylinder {
        radius,
        half_height: height * 0.5,
    }
}

/// A part of a compound collider at `at`.
pub fn part(at: Vec3, shape: ColliderShape) -> (Vec3, Quat, ColliderShape) {
    (at, Quat::IDENTITY, shape)
}

/// A rotated part of a compound collider.
pub fn part_rot(at: Vec3, rot: Quat, shape: ColliderShape) -> (Vec3, Quat, ColliderShape) {
    (at, rot, shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every prop of the yard registers non-empty meshes with finite positions, unit
    /// normals and valid indices, and every solid prop has a collider.
    #[test]
    fn yard_props_register_valid_meshes() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        {
            let mut commands = world.commands();
            super::super::spawn_demo(&mut commands, DVec3::ZERO);
        }
        world.flush();
        let meshes = world.resource::<Assets<Mesh>>();
        let mut count = 0;
        for (key, handle) in meshes.keys() {
            if !key.starts_with("softbody/yard/") {
                continue;
            }
            count += 1;
            let m = meshes.get(handle).expect(key);
            assert!(
                !m.indices.is_empty() && m.indices.len().is_multiple_of(3),
                "{key}"
            );
            for v in &m.vertices {
                let p = Vec3::from(v.position);
                let n = Vec3::from(v.normal);
                assert!(
                    p.is_finite() && p.abs().max_element() < 20.0,
                    "{key}: position {p}"
                );
                assert!((n.length() - 1.0).abs() < 1e-3, "{key}: normal {n}");
            }
            assert!(
                m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
                "{key}"
            );
        }
        assert!(count > 40, "only {count} prop meshes");
        let solids = world
            .query::<(&RigidBody, &Collider)>()
            .iter(&world)
            .count();
        assert!(solids > 70, "only {solids} solid props");
    }
}
