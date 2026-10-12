//! How the movement test cases look: detailed procedural fighters, set dressing for the
//! parkour course and the duel ring, and effects (blade trails, sparks, dust, afterimages).
//!
//! Meshes, textures and materials are generated once, when the render assets exist, and
//! registered under `movement/...` asset keys, so spawn code only needs
//! `MeshRef::Asset(key)` / `MaterialRef::Asset(key)` (see [`mesh`] and [`material`]).
//! Each part of the look adds its generator to [`LIBRARY`].
//!
//! - [`meshgen`]: the mesh toolkit (rounded and chamfered boxes, lofts, lathes, extrusions).
//! - [`texgen`]: tiling noise and PBR texture baking.
//! - [`materials`]: shared baked materials (`movement/mat/<name>`): concrete, steel, wood,
//!   stone, sand, cloth, lacquer…
//! - [`course`]: set dressing for the parkour course and the duel ring.

pub mod course;
pub mod materials;
pub mod meshgen;
pub mod texgen;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, Mesh, MeshRef, StandardMaterial};

pub use meshgen::{Cap, MeshBuilder, Section};
pub use texgen::{Surface, TextureSet};

/// Where generators put what they build.
pub struct Library<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub images: &'a mut Assets<Image>,
    pub materials: &'a mut Assets<StandardMaterial>,
    /// Texture size for baked materials (pixels per side).
    pub texture_size: u32,
}

impl Library<'_> {
    /// Registers a mesh under `movement/<key>`.
    pub fn mesh(&mut self, key: &str, mesh: MeshBuilder) {
        self.meshes
            .insert_keyed(&format!("movement/{key}"), mesh.build());
    }

    /// Registers a material under `movement/<key>`.
    pub fn material(&mut self, key: &str, material: StandardMaterial) {
        self.materials
            .insert_keyed(&format!("movement/{key}"), material);
    }

    /// Adds a baked texture set's images and returns `base` with them applied.
    pub fn textured(&mut self, set: TextureSet, base: StandardMaterial) -> StandardMaterial {
        let TextureSet {
            base_color,
            normal,
            metallic_roughness,
            occlusion,
        } = set;
        base.with_base_color_texture(self.images.add(base_color))
            .with_normal_texture(self.images.add(normal))
            .with_metallic_roughness_texture(self.images.add(metallic_roughness))
            .with_occlusion_texture(self.images.add(occlusion))
    }
}

/// A generator: registers meshes and materials in the library.
pub type Generator = fn(&mut Library);

/// Everything the look generates, in order.
pub const LIBRARY: &[Generator] = &[materials::generate, course::generate];

/// A mesh registered by a generator.
pub fn mesh(key: &str) -> MeshRef {
    MeshRef::Asset(format!("movement/{key}"))
}

/// A material registered by a generator.
pub fn material(key: &str) -> MaterialRef {
    MaterialRef::Asset(format!("movement/{key}"))
}

/// Marks a sword blade for the effects: this entity sits at the blade's base (where it meets
/// the guard) and `tip` is a descendant entity at the point. `owner` is the fighter holding
/// it. Blade trails are drawn between the two while the owner swings.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Blade {
    pub owner: Entity,
    pub tip: Entity,
    /// Linear RGB of the blade's glow and trail.
    pub color: [f32; 3],
}

/// Leave this entity (and its mesh) out of afterimages and other copies of a character
/// (lights, trails, helper meshes).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct NoGhost;

/// Look settings.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookSettings {
    /// Size of baked textures, pixels per side.
    pub texture_size: u32,
}

impl Default for LookSettings {
    fn default() -> Self {
        Self { texture_size: 1024 }
    }
}

/// Runs the generators once, as soon as the render asset stores exist (before meshes are
/// resolved, so spawned entities find their keys).
fn build_library(
    mut built: Local<bool>,
    settings: Res<LookSettings>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    images: Option<ResMut<Assets<Image>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    if *built {
        return;
    }
    let (Some(mut meshes), Some(mut images), Some(mut materials)) = (meshes, images, materials)
    else {
        return;
    };
    *built = true;
    let mut lib = Library {
        meshes: &mut meshes,
        images: &mut images,
        materials: &mut materials,
        texture_size: settings.texture_size,
    };
    for generate in LIBRARY {
        generate(&mut lib);
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<LookSettings>()
        .add_systems(First, build_library);
}
