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
//! - [`fighter`]: the armored sword fighters (four styles) and their parts.
//! - [`rig`]: the fighters' skeleton, poses, two-bone IK and the procedural animator.
//! - [`rope`]: the grapple's cable and hook.
//! - [`zipline`]: zip line cables, dipping under their riders.
//! - [`effects`]: blade trails, parry sparks, slash flashes, impact lights, dust and the
//!   render feature that draws them and the afterimage ghosts.

pub mod course;
pub mod effects;
pub mod fighter;
pub mod materials;
pub mod meshgen;
pub mod rig;
pub mod rope;
pub mod texgen;
pub mod zipline;

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
pub const LIBRARY: &[Generator] = &[
    materials::generate,
    course::generate,
    fighter::generate,
    rope::generate,
    zipline::generate,
    crate::lockon::generate,
];

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
    /// Bake the library on a background thread instead of inside the first frame. Movement's
    /// meshes and materials are then registered a few seconds in, when [`LookReady`] appears:
    /// spawn the demo (or anything drawn with the look's asset keys) once it exists.
    pub background: bool,
}

impl Default for LookSettings {
    fn default() -> Self {
        Self {
            texture_size: 1024,
            background: false,
        }
    }
}

/// Present once every generator's meshes, textures and materials are registered.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LookReady;

/// What the generators made, waiting to be registered with the render asset stores.
#[derive(Default)]
struct Baked {
    meshes: Assets<Mesh>,
    images: Assets<Image>,
    materials: Assets<StandardMaterial>,
}

/// Runs every generator, each on a thread of its own (their texture bakes share the cores).
fn bake_library(texture_size: u32) -> Vec<Baked> {
    std::thread::scope(|s| {
        let jobs: Vec<_> = LIBRARY
            .iter()
            .map(|generate| {
                s.spawn(move || {
                    let mut b = Baked::default();
                    generate(&mut Library {
                        meshes: &mut b.meshes,
                        images: &mut b.images,
                        materials: &mut b.materials,
                        texture_size,
                    });
                    b
                })
            })
            .collect();
        jobs.into_iter()
            .map(|j| j.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

/// Moves baked assets into the world's stores under their keys, pointing the materials at
/// the images' new handles.
fn insert_baked(
    baked: Vec<Baked>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) {
    for mut b in baked {
        let added: Vec<_> = b.images.iter().map(|(h, _)| h).collect();
        let mut moved = std::collections::HashMap::with_capacity(added.len());
        for h in added {
            if let Some(image) = b.images.remove(h) {
                moved.insert(h, images.add(image));
            }
        }
        let keyed: Vec<_> = b.meshes.keys().map(|(k, h)| (k.to_string(), h)).collect();
        for (key, h) in keyed {
            if let Some(mesh) = b.meshes.remove(h) {
                meshes.insert_keyed(&key, mesh);
            }
        }
        let keyed: Vec<_> = b
            .materials
            .keys()
            .map(|(k, h)| (k.to_string(), h))
            .collect();
        for (key, h) in keyed {
            let Some(mut m) = b.materials.remove(h) else {
                continue;
            };
            for tex in [
                &mut m.base_color_texture,
                &mut m.normal_texture,
                &mut m.metallic_roughness_texture,
                &mut m.emissive_texture,
                &mut m.occlusion_texture,
                &mut m.height_texture,
            ] {
                *tex = tex.and_then(|t| moved.get(&t).copied());
            }
            materials.insert_keyed(&key, m);
        }
    }
}

/// The library being baked in the background ([`LookSettings::background`]).
#[derive(Resource)]
struct Baking(Option<std::thread::JoinHandle<Vec<Baked>>>);

/// Bakes the library once the render asset stores exist (before meshes are resolved, so
/// spawned entities find their keys), or starts baking it in the background.
fn build_library(
    mut commands: Commands,
    mut started: Local<bool>,
    settings: Res<LookSettings>,
    baking: Option<ResMut<Baking>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    images: Option<ResMut<Assets<Image>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let (Some(mut meshes), Some(mut images), Some(mut materials)) = (meshes, images, materials)
    else {
        return;
    };
    let baked = if let Some(mut baking) = baking {
        if !baking.0.as_ref().is_some_and(|j| j.is_finished()) {
            return;
        }
        commands.remove_resource::<Baking>();
        let job = baking.0.take().expect("checked above");
        job.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
    } else if *started {
        return;
    } else {
        *started = true;
        let size = settings.texture_size;
        if settings.background {
            let job = std::thread::Builder::new()
                .name("movement look".into())
                .spawn(move || bake_library(size))
                .expect("could not start the look library's thread");
            commands.insert_resource(Baking(Some(job)));
            return;
        }
        bake_library(size)
    };
    insert_baked(baked, &mut meshes, &mut images, &mut materials);
    commands.insert_resource(LookReady);
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<LookSettings>()
        .add_systems(First, build_library);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_materials_point_at_their_own_images_after_the_move() {
        let (mut meshes, mut images, mut materials) = (
            Assets::<Mesh>::default(),
            Assets::<Image>::default(),
            Assets::<StandardMaterial>::default(),
        );
        // Images already in the store shift the handles the baked ones get.
        let other = images.add(Image::new(1, 1, vec![0; 4], false));
        insert_baked(bake_library(16), &mut meshes, &mut images, &mut materials);
        let mut textured = 0;
        for (key, h) in materials.keys() {
            assert!(key.starts_with("movement/"), "{key}");
            let m = materials.get(h).unwrap();
            for t in m.textures().into_iter().flatten() {
                assert!(images.get(t).is_some() && t != other, "{key}");
                textured += 1;
            }
        }
        assert!(textured > 100, "{textured}");
        assert!(meshes.len() > 500, "{}", meshes.len());
    }
}
