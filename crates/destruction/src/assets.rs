//! Shared meshes and materials for walls, debris, chips, dust and charges, created once so
//! everything the module draws batches into a few instanced draw calls.

use std::collections::HashMap;

use bevy_tasks::{Task, TaskPool, TaskPoolBuilder, block_on, poll_once};
use engine_assets::{Assets, Handle};
use engine_core::math::{Rng, Vec3};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, Shape, StandardMaterial};

use crate::fracture::{ConvexPoly, impact_seeds, voronoi};
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::meshing::MeshBuilder;
use crate::textures::{self, SurfaceKind, TextureSet};

/// Emission strength of embers glowing in the cracks of flaming char.
pub const EMBER_GLOW: f32 = 40_000.0;
/// Emission strength of smoldering char.
pub const SMOLDER_GLOW: f32 = 8_000.0;

/// Alpha levels of the dust materials.
pub const DUST_LEVELS: usize = 6;

/// Albedo scale of textured rock and soil chips.
const CHIP_SHADE: f32 = 0.55;

/// Handles shared by everything destruction draws.
#[derive(Resource, Debug, Clone)]
pub struct DestructionAssets {
    /// Vertex-colored surface material per [`DestructibleMaterial`] (walls, voxels, chunks).
    pub surface: Vec<Handle<StandardMaterial>>,
    /// Small irregular chip meshes (unit size, white vertex colors).
    pub chips: Vec<Handle<Mesh>>,
    /// Chip material per [`DestructibleMaterial`] (the broken interior color).
    pub chip_material: Vec<Handle<StandardMaterial>>,
    /// Fresh fracture faces per [`DestructibleMaterial`]: the surface material, except where
    /// the surface texture has mortar joints (stone, brick), which a broken face shows as
    /// plain broken stone or fired clay instead. Shared with the surface until
    /// [`Self::texture`].
    pub broken: Vec<Handle<StandardMaterial>>,
    /// Glowing spark material (thermite, metal grinding).
    pub spark: Handle<StandardMaterial>,
    /// Dust puff sphere.
    pub dust_mesh: Handle<Mesh>,
    /// Soft dust puff frames ([`textures::dust_puffs`]) for camera-facing sprites; `None`
    /// (no image store) draws puffs as [`Self::dust_mesh`].
    pub dust_sprite: Option<Handle<Image>>,
    /// Dust materials from faint to dense, per darkness level (light, mid, dark).
    pub dust: [[Handle<StandardMaterial>; DUST_LEVELS]; 3],
    /// Breach charge body and its glowing thermite strip.
    pub charge_body: Handle<StandardMaterial>,
    pub thermite_glow: Handle<StandardMaterial>,
    /// Valheim-style support colors: blue (on the ground), green, yellow, orange, red (about
    /// to fail).
    pub support: [Handle<StandardMaterial>; 5],
    /// The support colors per surface, keeping the surface's relief (normal and roughness
    /// maps), so tinted wood still shows its grain. Empty until [`Self::texture`].
    pub support_surface: Vec<[Handle<StandardMaterial>; 5]>,
    /// Build-mode ghosts ([`crate::build`]): the support colors, translucent, then a deep red
    /// where the piece can't go.
    pub ghost: [Handle<StandardMaterial>; 6],
    /// The ghost colors per surface, keeping the surface's grain and joints. Empty until
    /// [`Self::texture`].
    pub ghost_surface: Vec<[Handle<StandardMaterial>; 6]>,
    /// Grassy topsoil on voxel hills ([`crate::voxel::GRASS_LAYER`]).
    pub grass: Handle<StandardMaterial>,
    /// Gravel ground ([`crate::dress::GravelPad`]).
    pub gravel: Handle<StandardMaterial>,
    /// Burnt wood by [`crate::burn::CharLevel`]: scorched, glowing char, smoldering, cold.
    pub char: [Handle<StandardMaterial>; 4],
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
        let dust = [0.1f32, 0.45, 0.85].map(|darkness| {
            let c = Self::dust_color(darkness);
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
        let support = SUPPORT_COLORS.map(|hex| {
            let m = StandardMaterial::hex(hex).with_roughness(0.7);
            let c = crate::material::linear(hex);
            materials.add(m.with_emissive(c, SUPPORT_GLOW))
        });
        let ghost = GHOST_COLORS.map(|hex| {
            let c = crate::material::linear(hex);
            materials.add(
                StandardMaterial::hex(hex)
                    .with_roughness(0.6)
                    .with_alpha(GHOST_ALPHA)
                    .with_emissive(c, SUPPORT_GLOW),
            )
        });
        let gravel = materials.add(StandardMaterial::hex(0x6d695f).with_roughness(0.95));
        // Untextured char: flat colors (the props' vertex colors carry the wood's color, so
        // scorch multiplies it down), the embers a faint glow over the whole surface.
        let char = [
            StandardMaterial::color(0.3, 0.24, 0.2).with_roughness(0.8),
            StandardMaterial::color(0.05, 0.045, 0.04)
                .with_roughness(0.6)
                .with_emissive([1.0, 0.3, 0.06], EMBER_GLOW * 0.08),
            StandardMaterial::color(0.05, 0.045, 0.04)
                .with_roughness(0.6)
                .with_emissive([1.0, 0.25, 0.05], SMOLDER_GLOW * 0.08),
            StandardMaterial::color(0.09, 0.088, 0.085).with_roughness(0.75),
        ]
        .map(|m| materials.add(m));
        Self {
            grass: surface[DestructibleMaterial::Dirt.index()],
            broken: surface.clone(),
            gravel,
            char,
            surface,
            chips,
            chip_material,
            spark,
            dust_mesh,
            dust_sprite: None,
            dust,
            charge_body,
            thermite_glow,
            support,
            support_surface: Vec::new(),
            ghost,
            ghost_surface: Vec::new(),
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
        self.texture_from(materials, images, &mut MadeTextures::default());
    }

    /// [`Self::texture`] with textures made ahead ([`MadeTextures`]); any it lacks are made
    /// on the spot.
    pub fn texture_from(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        made: &mut MadeTextures,
    ) {
        for (i, m) in DestructibleMaterial::ALL.iter().enumerate() {
            let mut mat = made.material(surface_kind(*m), SURFACE_SEED + i as u64, images);
            mat.receives_wetness = *m != DestructibleMaterial::Glass;
            if *m == DestructibleMaterial::Plaster {
                // The texture is fresh interior paint (albedo about 0.8), which blows out in
                // full sun; outdoor render here is a weathered off-white.
                mat.base_color = [0.66, 0.65, 0.62, 1.0];
            }
            // The texture stays (grain, joints) under a strong tint and a little glow.
            let tints = SUPPORT_COLORS.map(|hex| {
                let c = crate::material::linear(hex);
                let mut t = mat.with_emissive(c, SUPPORT_GLOW * 0.5);
                t.base_color = [c[0] * 2.4, c[1] * 2.4, c[2] * 2.4, 1.0];
                materials.add(t)
            });
            self.support_surface.push(tints);
            let ghosts = GHOST_COLORS.map(|hex| {
                let c = crate::material::linear(hex);
                let mut t = mat
                    .with_emissive(c, SUPPORT_GLOW * 0.6)
                    .with_alpha(GHOST_ALPHA);
                t.base_color = [c[0] * 2.4, c[1] * 2.4, c[2] * 2.4, GHOST_ALPHA];
                t.receives_wetness = false;
                materials.add(t)
            });
            self.ghost_surface.push(ghosts);
            materials.set(self.surface[i], mat);
        }
        let mut grass = made.material(SurfaceKind::Grass, GRASS_SEED, images);
        grass.receives_wetness = true;
        self.grass = materials.add(grass);
        let mut gravel = made.material(SurfaceKind::Gravel, GRAVEL_SEED, images);
        gravel.receives_wetness = true;
        materials.set(self.gravel, gravel);
        self.texture_char(materials, images, made);
        self.texture_broken(materials, images, made);
        self.texture_chips(materials);
        self.dust_sprite = Some(images.add(made.image(TextureJob::DustPuffs)));
        self.textured = true;
    }

    /// Chips of rock, soil, masonry and wood wear their broken surface's texture, darkened
    /// like shaded relief, so spoil, rubble and splinters match what they came from (flat
    /// fresh-break colors, or a bare texture in full sun, read as pale or glowing yellow flakes
    /// on soil and grass).
    fn texture_chips(&mut self, materials: &mut Assets<StandardMaterial>) {
        use DestructibleMaterial as M;
        for m in [
            M::Rock,
            M::Dirt,
            M::Stone,
            M::Brick,
            M::Concrete,
            M::ReinforcedConcrete,
            M::Wood,
        ] {
            let Some(mut chip) = materials.get(self.broken[m.index()]).copied() else {
                continue;
            };
            for c in &mut chip.base_color[..3] {
                *c *= CHIP_SHADE;
            }
            materials.set(self.chip_material[m.index()], chip);
        }
    }

    /// Joint-free fracture materials for masonry: the rock texture tinted so that, under the
    /// same vertex colors as the surface, it averages the material's own color.
    fn texture_broken(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        made: &mut MadeTextures,
    ) {
        let table = DestructionMaterials::default();
        let rock = table.get(DestructibleMaterial::Rock).exterior;
        for m in [DestructibleMaterial::Stone, DestructibleMaterial::Brick] {
            let own = table.get(m).exterior;
            let mut mat = made.material(SurfaceKind::Rock, BROKEN_SEED + m.index() as u64, images);
            // Half as saturated as the ratio: the masonry textures average greyer than their
            // nominal colors, and a broken face beside them shouldn't look orange.
            let tint: [f32; 3] = std::array::from_fn(|k| own[k] / rock[k].max(1e-3));
            let grey = (tint[0] + tint[1] + tint[2]) / 3.0;
            mat.base_color = [
                (tint[0] + grey) * 0.5,
                (tint[1] + grey) * 0.5,
                (tint[2] + grey) * 0.5,
                1.0,
            ];
            mat.receives_wetness = true;
            self.broken[m.index()] = materials.add(mat);
        }
    }

    /// Char materials from the charred-timber texture, the embers glowing in its cracks.
    fn texture_char(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        made: &mut MadeTextures,
    ) {
        let wood = DestructibleMaterial::Wood.index();
        if let Some(mut scorched) = materials.get(self.surface[wood]).cloned() {
            scorched.base_color = [0.3, 0.23, 0.19, 1.0];
            scorched.perceptual_roughness = 0.85;
            materials.set(self.char[0], scorched);
        }
        let charcoal = made.material(SurfaceKind::Charcoal, CHAR_SEED, images);
        let glow = images.add(made.image(TextureJob::EmberGlow));
        let ember = |strength: f32| {
            let mut m = charcoal;
            m.emissive = [1.0; 3];
            m.emissive_strength = strength;
            m.emissive_texture = Some(glow);
            m
        };
        materials.set(self.char[1], ember(EMBER_GLOW));
        materials.set(self.char[2], ember(SMOLDER_GLOW));
        let mut cold = charcoal;
        // Cooled char greys over with ash.
        cold.base_color = [1.35, 1.32, 1.28, 1.0];
        cold.receives_wetness = true;
        materials.set(self.char[3], cold);
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

    /// Build-mode ghost material for a piece of `m` at a [`crate::build::Placement::level`].
    pub fn ghost(&self, m: DestructibleMaterial, level: usize) -> Handle<StandardMaterial> {
        let level = level.min(GHOST_COLORS.len() - 1);
        self.ghost_surface
            .get(m.index())
            .map_or(self.ghost[level], |g| g[level])
    }

    pub fn broken(&self, m: DestructibleMaterial) -> Handle<StandardMaterial> {
        self.broken[m.index()]
    }

    pub fn chip_material(&self, m: DestructibleMaterial) -> Handle<StandardMaterial> {
        self.chip_material[m.index()]
    }

    /// Support color for a piece: blue on the ground, then green → red as support runs out.
    pub fn support_color(&self, grounded: bool, fraction: f32) -> Handle<StandardMaterial> {
        self.support[support_level(grounded, fraction)]
    }

    /// Support color for something drawn in `original`: a surface keeps its relief under the
    /// color when textured; anything else gets the flat color.
    pub fn support_color_on(
        &self,
        original: Handle<StandardMaterial>,
        grounded: bool,
        fraction: f32,
    ) -> Handle<StandardMaterial> {
        let level = support_level(grounded, fraction);
        self.surface
            .iter()
            .position(|s| *s == original)
            .and_then(|i| self.support_surface.get(i))
            .map_or(self.support[level], |tints| tints[level])
    }

    /// Linear color of dust `darkness` dark (0 pale stone dust, 1 soot).
    pub fn dust_color(darkness: f32) -> [f32; 3] {
        let light = [0.62, 0.58, 0.52];
        let dark = [0.12, 0.11, 0.1];
        std::array::from_fn(|k| light[k] + (dark[k] - light[k]) * darkness)
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

/// Valheim-style support colors: blue (on the ground), green, yellow, orange, red.
const SUPPORT_COLORS: [u32; 5] = [0x2f6bff, 0x34b84a, 0xe6cf2e, 0xf08a24, 0xe0352a];
/// Glow of the support colors, so they read in shade too.
const SUPPORT_GLOW: f32 = 1500.0;
/// Build-mode ghost colors: the support colors, then the deep red of a piece that can't go.
const GHOST_COLORS: [u32; 6] = [0x2f6bff, 0x34b84a, 0xe6cf2e, 0xf08a24, 0xe0352a, 0xff1010];
/// Ghost opacity.
const GHOST_ALPHA: f32 = 0.55;

/// Index into [`SUPPORT_COLORS`]: on the ground, then by the fraction of support left.
pub fn support_level(grounded: bool, fraction: f32) -> usize {
    if grounded {
        0
    } else if fraction >= 0.7 {
        1
    } else if fraction >= 0.45 {
        2
    } else if fraction >= 0.25 {
        3
    } else {
        4
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

// Seeds of the textures [`DestructionAssets::texture`] makes.
const SURFACE_SEED: u64 = 0xDE57;
const GRASS_SEED: u64 = 0xDE57_06A5;
const GRAVEL_SEED: u64 = 0xDE57_06A6;
const BROKEN_SEED: u64 = 0xB40C;
const CHAR_SEED: u64 = 0xC4A2;
const DUST_SEED: u64 = 0xD057;
/// Size of the dust puff frames, pixels.
const DUST_PUFF_SIZE: u32 = 64;

/// One texture [`DestructionAssets::texture`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureJob {
    /// A surface's texture set.
    Set(SurfaceKind, u64),
    /// The glow in charred wood's cracks.
    EmberGlow,
    /// The dust puff frames.
    DustPuffs,
}

/// A made [`TextureJob`].
pub enum MadeTexture {
    Set(TextureSet),
    Image(Image),
}

impl TextureJob {
    /// Every texture [`DestructionAssets::texture`] takes.
    pub fn all() -> Vec<TextureJob> {
        let mut jobs: Vec<TextureJob> = DestructibleMaterial::ALL
            .iter()
            .enumerate()
            .map(|(i, m)| Self::Set(surface_kind(*m), SURFACE_SEED + i as u64))
            .collect();
        jobs.extend([
            Self::Set(SurfaceKind::Grass, GRASS_SEED),
            Self::Set(SurfaceKind::Gravel, GRAVEL_SEED),
            Self::Set(
                SurfaceKind::Rock,
                BROKEN_SEED + DestructibleMaterial::Stone.index() as u64,
            ),
            Self::Set(
                SurfaceKind::Rock,
                BROKEN_SEED + DestructibleMaterial::Brick.index() as u64,
            ),
            Self::Set(SurfaceKind::Charcoal, CHAR_SEED),
            Self::EmberGlow,
            Self::DustPuffs,
        ]);
        jobs
    }

    /// Make it (pure CPU work, any thread).
    pub fn make(self) -> MadeTexture {
        match self {
            Self::Set(kind, seed) => {
                MadeTexture::Set(textures::generate(kind, texture_size(kind), seed))
            }
            Self::EmberGlow => MadeTexture::Image(textures::ember_glow(
                texture_size(SurfaceKind::Charcoal),
                CHAR_SEED,
            )),
            Self::DustPuffs => MadeTexture::Image(textures::dust_puffs(DUST_PUFF_SIZE, DUST_SEED)),
        }
    }
}

/// Textures made ahead for [`DestructionAssets::texture_from`], taken as it needs them.
#[derive(Default)]
pub struct MadeTextures(HashMap<TextureJob, MadeTexture>);

impl MadeTextures {
    /// Whether every texture made ahead has been taken.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A textured material for `kind` (white base color: the texture carries the albedo and
    /// vertex colors only shade it relative to the surface's own color).
    fn material(
        &mut self,
        kind: SurfaceKind,
        seed: u64,
        images: &mut Assets<Image>,
    ) -> StandardMaterial {
        let job = TextureJob::Set(kind, seed);
        let set = match self.0.remove(&job) {
            Some(MadeTexture::Set(set)) => set,
            _ => textures::generate(kind, texture_size(kind), seed),
        };
        set.into_material(images)
    }

    fn image(&mut self, job: TextureJob) -> Image {
        match self.0.remove(&job).unwrap_or_else(|| job.make()) {
            MadeTexture::Image(image) => image,
            // Image jobs make images; a set here would be a mixed-up job, shown as its albedo.
            MadeTexture::Set(set) => set.albedo,
        }
    }
}

/// Threads for first-use builds (textures, prop looks with their baked occlusion), a pool of
/// their own: the async compute pool has a quarter of the cores and is shared with every other
/// module's loading, and the compute pool runs the frame's parallel systems, whose waits could
/// pick up a look that takes most of a second. Idle (parked) once everything is built.
pub fn bake_pool() -> &'static TaskPool {
    static POOL: std::sync::OnceLock<TaskPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
        TaskPoolBuilder::new()
            .num_threads(threads.saturating_sub(1).max(1))
            .thread_name("destruction bake".into())
            .build()
    })
}

/// [`DestructionAssets`]' textures being made on the bake threads ([`bake_pool`]), one task per
/// texture, while the game keeps drawing (the loading screen waits for the resource). The
/// way Unity's async loading and Unreal's async loading thread work: the heavy work on
/// worker threads, then a short hand-over on the frame thread once all of it is done.
#[derive(Resource)]
pub struct AssetsLoading {
    pending: Vec<(TextureJob, Task<MadeTexture>)>,
    made: MadeTextures,
}

impl AssetsLoading {
    fn start() -> Self {
        let pool = bake_pool();
        Self {
            pending: TextureJob::all()
                .into_iter()
                .map(|job| (job, pool.spawn(async move { job.make() })))
                .collect(),
            made: MadeTextures::default(),
        }
    }

    /// Collect the finished textures; true once all are in.
    fn poll(&mut self) -> bool {
        let mut i = 0;
        while i < self.pending.len() {
            match block_on(poll_once(&mut self.pending[i].1)) {
                Some(made) => {
                    let (job, _) = self.pending.swap_remove(i);
                    self.made.0.insert(job, made);
                }
                None => i += 1,
            }
        }
        self.pending.is_empty()
    }

    /// Textures still being made.
    pub fn remaining(&self) -> usize {
        self.pending.len()
    }
}

/// Create [`DestructionAssets`] once the asset stores exist. With an image store its textures
/// are made on worker threads first ([`AssetsLoading`]) and it appears a few frames later;
/// without one (headless simulation) it is made at once.
pub fn ensure_assets(world: &mut World) {
    if world.contains_resource::<DestructionAssets>() {
        return;
    }
    if !world.contains_resource::<Assets<Mesh>>()
        || !world.contains_resource::<Assets<StandardMaterial>>()
    {
        return;
    }
    let made = if world.contains_resource::<Assets<Image>>() {
        let Some(mut loading) = world.remove_resource::<AssetsLoading>() else {
            world.insert_resource(AssetsLoading::start());
            return;
        };
        if !loading.poll() {
            world.insert_resource(loading);
            return;
        }
        Some(loading.made)
    } else {
        None
    };
    finish_assets(world, made);
}

/// Block until [`DestructionAssets`] exists (tools, examples and tests that draw at once).
pub fn wait_for_assets(world: &mut World) {
    ensure_assets(world);
    if let Some(mut loading) = world.remove_resource::<AssetsLoading>() {
        for (job, task) in loading.pending.drain(..) {
            loading.made.0.insert(job, block_on(task));
        }
        finish_assets(world, Some(loading.made));
    }
}

/// Make the materials and meshes, textured with `made` when there is an image store.
fn finish_assets(world: &mut World, made: Option<MadeTextures>) {
    let _span = tracing::info_span!("destruction assets hand-over").entered();
    let table = world
        .get_resource::<DestructionMaterials>()
        .cloned()
        .unwrap_or_default();
    let mut assets = world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        DestructionAssets::new(&mut meshes, &mut materials, &table)
    });
    if let Some(mut made) = made {
        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            assets.texture_from(&mut materials, &mut images, &mut made);
        });
    }
    world.insert_resource(assets);
}
