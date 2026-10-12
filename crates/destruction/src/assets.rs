//! Shared meshes and materials for walls, debris, chips, dust and charges, created once so
//! everything the module draws batches into a few instanced draw calls.

use engine_assets::{Assets, Handle};
use engine_core::math::{Rng, Vec3};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, Shape, StandardMaterial};

use crate::fracture::{ConvexPoly, impact_seeds, voronoi};
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::meshing::MeshBuilder;
use crate::textures::{self, SurfaceKind};

/// Alpha levels of the dust materials.
pub const DUST_LEVELS: usize = 6;

/// Handles shared by everything destruction draws.
#[derive(Resource, Debug, Clone)]
pub struct DestructionAssets {
    /// Vertex-colored surface material per [`DestructibleMaterial`] (walls, voxels, chunks).
    pub surface: Vec<Handle<StandardMaterial>>,
    /// Small irregular chip meshes (unit size, white vertex colors).
    pub chips: Vec<Handle<Mesh>>,
    /// Chip material per [`DestructibleMaterial`] (the broken interior color).
    pub chip_material: Vec<Handle<StandardMaterial>>,
    /// Glowing spark material (thermite, metal grinding).
    pub spark: Handle<StandardMaterial>,
    /// Dust puff sphere.
    pub dust_mesh: Handle<Mesh>,
    /// Dust materials from faint to dense, per darkness level (light, mid, dark).
    pub dust: [[Handle<StandardMaterial>; DUST_LEVELS]; 3],
    /// Breach charge body and its glowing thermite strip.
    pub charge_body: Handle<StandardMaterial>,
    pub thermite_glow: Handle<StandardMaterial>,
    /// Valheim-style support colors: blue (on the ground), green, yellow, orange, red (about
    /// to fail).
    pub support: [Handle<StandardMaterial>; 5],
    /// Grassy topsoil on voxel hills ([`crate::voxel::GRASS_LAYER`]).
    pub grass: Handle<StandardMaterial>,
    /// Surfaces carry procedural PBR textures (needs an image store); voxel meshes then split
    /// into one layer per surface.
    pub textured: bool,
}

impl DestructionAssets {
    pub fn new(
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        table: &DestructionMaterials,
    ) -> Self {
        let surface: Vec<Handle<StandardMaterial>> = DestructibleMaterial::ALL
            .iter()
            .map(|m| {
                let p = table.get(*m);
                let mut mat = StandardMaterial::color(1.0, 1.0, 1.0)
                    .with_roughness(p.roughness)
                    .with_metallic(p.metallic);
                mat.double_sided = false;
                materials.add(mat)
            })
            .collect();
        let chip_material = DestructibleMaterial::ALL
            .iter()
            .map(|m| {
                let p = table.get(*m);
                let c = p.interior;
                materials.add(
                    StandardMaterial::color(c[0], c[1], c[2])
                        .with_roughness(p.roughness)
                        .with_metallic(p.metallic),
                )
            })
            .collect();
        let mut rng = Rng::new(0xC41F5);
        let cube = ConvexPoly::cuboid(Vec3::splat(0.5));
        let chips = (0..6)
            .map(|k| {
                let seeds = impact_seeds(&cube, Vec3::ZERO, 5, 0.5, &mut rng);
                let pieces = voronoi(&cube, &seeds);
                let piece = &pieces[k % pieces.len().max(1)];
                let (_, c) = piece.volume_centroid();
                let (lo, hi) = piece.bounds();
                let size = (hi - lo).max_element().max(1e-3);
                let mut b: MeshBuilder = piece.to_mesh(c, [1.0; 3], [1.0; 3]);
                b.translate(Vec3::ZERO);
                for v in &mut b.vertices {
                    v.position = (Vec3::from(v.position) / size).to_array();
                }
                meshes.add(b.build())
            })
            .collect();
        let spark = materials.add(
            StandardMaterial::color(1.0, 0.55, 0.15).with_emissive([1.0, 0.45, 0.1], 60_000.0),
        );
        // A lumpy puff rather than a ball, so overlapping puffs read as a cloud.
        let dust_mesh = {
            let mut m = Shape::Sphere {
                radius: 0.5,
                segments: 10,
            }
            .mesh();
            for v in &mut m.vertices {
                let p = Vec3::from(v.position);
                let d = p.normalize_or(Vec3::Y);
                let n = engine_core::math::noise::perlin3(
                    d.x as f64 * 1.7,
                    d.y as f64 * 1.7,
                    d.z as f64 * 1.7,
                    17,
                );
                v.position = (d * 0.5 * (1.0 + 0.45 * n)).to_array();
            }
            m.compute_normals();
            meshes.add(m)
        };
        let dust_color = |darkness: f32| {
            let light = [0.62, 0.58, 0.52];
            let dark = [0.12, 0.11, 0.1];
            [
                light[0] + (dark[0] - light[0]) * darkness,
                light[1] + (dark[1] - light[1]) * darkness,
                light[2] + (dark[2] - light[2]) * darkness,
            ]
        };
        let dust = [0.1f32, 0.45, 0.85].map(|darkness| {
            let c = dust_color(darkness);
            std::array::from_fn(|level| {
                let alpha = 0.3 * (level + 1) as f32 / DUST_LEVELS as f32;
                let mut m = StandardMaterial::color(c[0], c[1], c[2])
                    .with_roughness(1.0)
                    .with_alpha(alpha);
                m.receives_wetness = false;
                m.subsurface = 0.6;
                materials.add(m)
            })
        });
        let charge_body = materials.add(StandardMaterial::hex(0x2d3330).with_roughness(0.6));
        let thermite_glow = materials
            .add(StandardMaterial::color(1.0, 0.6, 0.2).with_emissive([1.0, 0.5, 0.12], 120_000.0));
        let support = [0x2f6bff, 0x34b84a, 0xe6cf2e, 0xf08a24, 0xe0352a].map(|hex| {
            let m = StandardMaterial::hex(hex).with_roughness(0.7);
            let c = crate::material::linear(hex);
            materials.add(m.with_emissive(c, 1500.0))
        });
        Self {
            grass: surface[DestructibleMaterial::Dirt.index()],
            surface,
            chips,
            chip_material,
            spark,
            dust_mesh,
            dust,
            charge_body,
            thermite_glow,
            support,
            textured: false,
        }
    }

    /// Put procedural PBR textures (see [`crate::textures`]) on every surface material and
    /// the grass layer. Meshes then carry vertex colors relative to each surface's own color
    /// (fresh breaks lighter, scorch darker), and the texture supplies the albedo.
    pub fn texture(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) {
        for (i, m) in DestructibleMaterial::ALL.iter().enumerate() {
            let mut mat = textured(surface_kind(*m), 0xDE57 + i as u64, images);
            mat.receives_wetness = *m != DestructibleMaterial::Glass;
            materials.set(self.surface[i], mat);
        }
        let mut grass = textured(SurfaceKind::Grass, 0xDE57_06A5, images);
        grass.receives_wetness = true;
        self.grass = materials.add(grass);
        self.textured = true;
    }

    /// Material of a voxel mesh layer (a material index or the grass layer).
    pub fn layer_material(&self, layer: u8) -> Handle<StandardMaterial> {
        if layer == crate::voxel::GRASS_LAYER {
            self.grass
        } else {
            self.surface[layer as usize % self.surface.len()]
        }
    }

    pub fn surface(&self, m: DestructibleMaterial) -> Handle<StandardMaterial> {
        self.surface[m.index()]
    }

    pub fn chip_material(&self, m: DestructibleMaterial) -> Handle<StandardMaterial> {
        self.chip_material[m.index()]
    }

    /// Support color for a piece: blue on the ground, then green → red as support runs out.
    pub fn support_color(&self, grounded: bool, fraction: f32) -> Handle<StandardMaterial> {
        let i = if grounded {
            0
        } else if fraction >= 0.7 {
            1
        } else if fraction >= 0.45 {
            2
        } else if fraction >= 0.25 {
            3
        } else {
            4
        };
        self.support[i]
    }

    /// Dust material for a darkness (0..1) and opacity (0..1).
    pub fn dust(&self, darkness: f32, opacity: f32) -> Handle<StandardMaterial> {
        let d = if darkness < 0.3 {
            0
        } else if darkness < 0.65 {
            1
        } else {
            2
        };
        let level = ((opacity.clamp(0.0, 1.0) * DUST_LEVELS as f32).ceil() as usize)
            .clamp(1, DUST_LEVELS)
            - 1;
        self.dust[d][level]
    }
}

/// The texture each material wears.
pub fn surface_kind(m: DestructibleMaterial) -> SurfaceKind {
    match m {
        DestructibleMaterial::Plaster => SurfaceKind::Plaster,
        DestructibleMaterial::Wood => SurfaceKind::Wood,
        DestructibleMaterial::Brick => SurfaceKind::Brick,
        DestructibleMaterial::Stone => SurfaceKind::Stone,
        DestructibleMaterial::Concrete | DestructibleMaterial::ReinforcedConcrete => {
            SurfaceKind::Concrete
        }
        DestructibleMaterial::Rock => SurfaceKind::Rock,
        DestructibleMaterial::Dirt => SurfaceKind::Dirt,
        DestructibleMaterial::Metal => SurfaceKind::Metal,
        DestructibleMaterial::Glass => SurfaceKind::Glass,
        DestructibleMaterial::Ice => SurfaceKind::Ice,
    }
}

/// Image width for runtime surfaces: at most 1024 pixels, and small in debug builds so
/// tests that render stay fast.
fn texture_size(kind: SurfaceKind) -> u32 {
    if cfg!(debug_assertions) {
        128
    } else {
        kind.default_size().min(1024)
    }
}

/// A textured material for `kind` (white base color: the texture carries the albedo and
/// vertex colors only shade it relative to the surface's own color).
fn textured(kind: SurfaceKind, seed: u64, images: &mut Assets<Image>) -> StandardMaterial {
    textures::generate(kind, texture_size(kind), seed).into_material(images)
}

/// Create [`DestructionAssets`] the first time the asset stores exist.
pub fn ensure_assets(world: &mut World) {
    if world.contains_resource::<DestructionAssets>() {
        return;
    }
    if !world.contains_resource::<Assets<Mesh>>()
        || !world.contains_resource::<Assets<StandardMaterial>>()
    {
        return;
    }
    let table = world
        .get_resource::<DestructionMaterials>()
        .cloned()
        .unwrap_or_default();
    let mut assets = world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        DestructionAssets::new(&mut meshes, &mut materials, &table)
    });
    if world.contains_resource::<Assets<Image>>() {
        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            assets.texture(&mut materials, &mut images);
        });
    }
    world.insert_resource(assets);
}
