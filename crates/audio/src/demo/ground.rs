//! The trail's grounds and props: a displaced ribbon of ground per stretch that follows the
//! curve, with textured shoulders blending into the surrounding ground and ragged, soft
//! transitions into the next stretch; grass verges along the whole loop; each stretch
//! dressed with its own props (grass, ruts and prints, snow banks, a boardwalk, a bar
//! grating over a pit, a cast-concrete underpass, dunes and driftwood, a leaf-strewn forest
//! floor, puddles); and a trail marker at the start of every stretch.
//!
//! Everything is procedural: tileable PBR texture sets are painted and baked at spawn
//! ([`textures`]), meshes are generated from noise ([`procgen`]), both in parallel. Small
//! props are merged per three meters of trail and material, so the whole trail is a few
//! hundred draw-call-friendly entities. The result is kept in [`TrailGroundCache`], so
//! a second trail in the same world reuses every texture and mesh.
//!
//! The walking band (about ±1 m round the centre line) stays at `origin.y + 0.06` within a
//! few centimeters, where the walkers put their feet.

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::par_chunks_mut;
use engine_render::{
    AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, SkyLightVolume,
    StandardMaterial,
};

mod dress;
pub(super) mod procgen;
mod ribbon;
mod snow;
mod structures;
mod textures;

/// Every material the trail's grounds and props use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Mat {
    Verge,
    Dirt,
    Mud,
    WetMud,
    Snow,
    Ice,
    Gravel,
    PitFloor,
    Concrete,
    Formwork,
    Sand,
    Forest,
    Soil,
    Planks,
    Timber,
    Zinc,
    Steel,
    Leaf,
    Stone,
    Grass,
    Flowers,
    Water,
    Straw,
    Shell,
    Mushroom,
    Bark,
    Driftwood,
    Lamp,
    LampBody,
    /// Dark bitumen in the underpass floor's expansion joints.
    Sealant,
    /// A trail marker's sign, painted in its stretch's ring colour (index into `STRETCHES`).
    Sign(u8),
    /// The small glowing accent on a marker's sign.
    Glow(u8),
}

/// A piece of the trail to spawn: a mesh (owned, or shared by name) with a material at a
/// place relative to the trail's origin.
pub(super) struct Part {
    pub mesh: PartMesh,
    pub mat: Mat,
    pub at: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub shadows: bool,
}

pub(super) enum PartMesh {
    Own(Mesh),
    Shared(String),
}

impl Part {
    pub fn own(mesh: Mesh, mat: Mat, at: Vec3) -> Self {
        Self {
            mesh: PartMesh::Own(mesh),
            mat,
            at,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            shadows: true,
        }
    }

    pub fn shared(key: &str, mat: Mat, at: Vec3, rot: Quat, scale: Vec3) -> Self {
        Self {
            mesh: PartMesh::Shared(key.to_string()),
            mat,
            at,
            rot,
            scale,
            shadows: true,
        }
    }

    pub fn no_shadows(mut self) -> Self {
        self.shadows = false;
        self
    }
}

/// What one build job makes: parts, meshes shared by name, and lamps.
#[derive(Default)]
pub(super) struct Out {
    pub parts: Vec<Part>,
    pub shared: Vec<(String, Mesh)>,
    pub lights: Vec<(Vec3, PointLight)>,
}

impl Out {
    pub fn add(&mut self, other: Out) {
        self.parts.extend(other.parts);
        self.shared.extend(other.shared);
        self.lights.extend(other.lights);
    }

    pub fn has_shared(&self, key: &str) -> bool {
        self.shared.iter().any(|(k, _)| k == key)
    }
}

/// Small props merged per stretch of trail and material, so thousands of stones, blades or
/// leaves cost a handful of entities. Positions go in relative to the trail's origin.
#[derive(Default)]
pub(super) struct Batch {
    meshes: std::collections::BTreeMap<(i32, Mat), Mesh>,
}

/// Length of trail merged into one mesh, meters.
pub(super) const CHUNK: f64 = 3.0;

impl Batch {
    /// The mesh collecting `mat` props near `s` meters along the trail.
    pub fn at(&mut self, s: f64, mat: Mat) -> &mut Mesh {
        let k = (s / CHUNK).floor() as i32;
        self.meshes.entry((k, mat)).or_default()
    }

    /// One part per chunk and material, its vertices moved round the chunk's centre.
    pub fn finish(self) -> Vec<Part> {
        self.meshes
            .into_iter()
            .filter(|(_, m)| !m.indices.is_empty())
            .map(|((k, mat), mut m)| {
                let (p, _) = super::trail_point((k as f64 + 0.5) * CHUNK);
                let anchor = p.as_vec3();
                for v in &mut m.vertices {
                    v.position = (Vec3::from(v.position) - anchor).to_array();
                }
                Part::own(m, mat, anchor)
            })
            .collect()
    }
}

/// One spawned piece: mesh, material, offset from the origin, rotation, scale, and whether
/// it casts shadows.
type Item = (
    Handle<Mesh>,
    Handle<StandardMaterial>,
    Vec3,
    Quat,
    Vec3,
    bool,
);

/// Everything the trail's ground spawns, resolved to asset handles. Kept as a resource so a
/// second trail in the same world spawns from it instead of building again.
#[derive(Resource, Clone, Default)]
pub struct TrailGroundCache {
    items: Vec<Item>,
    lights: Vec<(Vec3, PointLight)>,
}

/// Lay the grounds, dress every stretch and build the underpass round `origin`.
pub(super) fn spawn(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| {
        structures::underpass_volumes(world, origin);
        let renders = world.contains_resource::<Assets<Mesh>>()
            && world.contains_resource::<Assets<Image>>()
            && world.contains_resource::<Assets<StandardMaterial>>();
        if !renders {
            return;
        }
        if !world.contains_resource::<TrailGroundCache>() {
            let cache = build(world);
            world.insert_resource(cache);
        }
        let cache = world.resource::<TrailGroundCache>().clone();
        world.spawn_batch(cache.items.into_iter().map(
            move |(mesh, material, at, rot, scale, shadows)| {
                (
                    Transform::from_translation(origin + at.as_dvec3())
                        .with_rotation(rot)
                        .with_scale(scale),
                    MeshRenderer {
                        mesh: MeshRef::Handle(mesh),
                        material: MaterialRef::Handle(material),
                        cast_shadows: shadows,
                        visible: true,
                    },
                )
            },
        ));
        for (at, light) in cache.lights {
            world.spawn((
                Name::new("Underpass lamp"),
                light,
                Transform::from_translation(origin + at.as_dvec3()),
            ));
        }
    });
}

/// Paint the textures and build every mesh (both in parallel), then register them.
fn build(world: &mut World) -> TrailGroundCache {
    // Textures, biggest first so the threads finish together.
    let mut slots: Vec<(usize, Option<procgen::TexSet>)> =
        (0..textures::RECIPES.len()).map(|i| (i, None)).collect();
    {
        let images = world.resource::<Assets<Image>>();
        for slot in &mut slots {
            let key = textures::RECIPES[slot.0].0;
            if images.by_key(&format!("trail:{key}:albedo")).is_some() {
                slot.0 = usize::MAX;
            }
        }
    }
    slots.retain(|s| s.0 != usize::MAX);
    par_chunks_mut(&mut slots, 1, |_, s| {
        s[0].1 = Some((textures::RECIPES[s[0].0].1)());
    });
    let mut images = world.resource_mut::<Assets<Image>>();
    for (i, set) in slots {
        let key = textures::RECIPES[i].0;
        if let Some(set) = set {
            images.insert_keyed(&format!("trail:{key}:albedo"), set.albedo);
            images.insert_keyed(&format!("trail:{key}:normal"), set.normal);
            images.insert_keyed(&format!("trail:{key}:orm"), set.orm);
        }
    }
    let tex = |name: &str| {
        let images = world.resource::<Assets<Image>>();
        let get = |k: &str| images.by_key(&format!("trail:{name}:{k}"));
        (get("albedo"), get("normal"), get("orm"))
    };
    let mut cache_tex = std::collections::HashMap::new();
    for (name, _) in textures::RECIPES {
        cache_tex.insert(*name, tex(name));
    }

    // Meshes.
    let jobs = dress::jobs();
    let mut results: Vec<(usize, Option<Out>)> = (0..jobs.len()).map(|i| (i, None)).collect();
    par_chunks_mut(&mut results, 1, |_, r| {
        r[0].1 = Some((jobs[r[0].0])());
    });
    let mut all = Out::default();
    for (_, out) in results {
        if let Some(out) = out {
            all.add(out);
        }
    }

    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let mut shared = std::collections::HashMap::new();
    for (key, mesh) in all.shared {
        shared
            .entry(key.clone())
            .or_insert_with(|| meshes.get_or_insert_keyed(&format!("trail:{key}"), || mesh));
    }
    let mut parts = Vec::with_capacity(all.parts.len());
    for p in all.parts {
        let handle = match p.mesh {
            PartMesh::Own(m) => {
                if m.indices.is_empty() {
                    continue;
                }
                meshes.add(m)
            }
            PartMesh::Shared(key) => match shared.get(&key) {
                Some(h) => *h,
                None => continue,
            },
        };
        parts.push((handle, p.mat, p.at, p.rot, p.scale, p.shadows));
    }

    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    let mut mats = std::collections::HashMap::new();
    let mut items = Vec::with_capacity(parts.len());
    for (mesh, mat, at, rot, scale, shadows) in parts {
        let handle = *mats.entry(mat).or_insert_with(|| {
            materials.get_or_insert_keyed(&format!("trail:{mat:?}"), || material(mat, &cache_tex))
        });
        items.push((mesh, handle, at, rot, scale, shadows));
    }
    TrailGroundCache {
        items,
        lights: all.lights,
    }
}

type TexHandles = (
    Option<Handle<Image>>,
    Option<Handle<Image>>,
    Option<Handle<Image>>,
);

fn textured(
    base: StandardMaterial,
    tex: &std::collections::HashMap<&str, TexHandles>,
    name: &str,
    tile: (f32, f32),
) -> StandardMaterial {
    let mut m = base.with_uv_scale(1.0 / tile.0, 1.0 / tile.1);
    if let Some((a, n, orm)) = tex.get(name) {
        m.base_color_texture = *a;
        m.normal_texture = *n;
        m.metallic_roughness_texture = *orm;
        m.occlusion_texture = *orm;
    }
    m
}

/// A ground material: textured, alpha-masked so its edges can fray (see `ribbon`).
fn ground(
    tex: &std::collections::HashMap<&str, TexHandles>,
    name: &str,
    tile: f32,
    tint: u32,
    roughness: f32,
) -> StandardMaterial {
    let mut m = textured(
        StandardMaterial::hex(tint).with_roughness(roughness),
        tex,
        name,
        (tile, tile),
    );
    m.alpha_mode = AlphaMode::Mask(0.5);
    m
}

fn material(mat: Mat, tex: &std::collections::HashMap<&str, TexHandles>) -> StandardMaterial {
    let plain = |c: u32, r: f32| StandardMaterial::hex(c).with_roughness(r);
    match mat {
        Mat::Verge => {
            // A mat of blades shadows most of its own sheen.
            let mut m = ground(tex, "verge", 1.5, 0xffffff, 1.0);
            m.reflectance = 0.3;
            m
        }
        Mat::Dirt => ground(tex, "dirt", 1.5, 0xffffff, 1.0),
        Mat::Mud => ground(tex, "mud", 2.0, 0xffffff, 1.0),
        Mat::WetMud => ground(tex, "mud", 2.0, 0xd8d0c8, 0.55),
        Mat::Snow => ground(tex, "snow", 2.0, 0xffffff, 1.0).with_subsurface(0.45),
        Mat::Ice => {
            let mut m = ground(tex, "ice", 2.5, 0xffffff, 1.0).with_subsurface(0.15);
            m.reflectance = 0.45;
            m
        }
        Mat::Gravel => ground(tex, "gravel", 1.0, 0xffffff, 1.0),
        Mat::PitFloor => ground(tex, "gravel", 1.0, 0x6a6560, 0.55),
        Mat::Concrete => ground(tex, "concrete", 2.0, 0xffffff, 1.0),
        Mat::Formwork => textured(plain(0xffffff, 1.0), tex, "formwork", (3.0, 3.0)),
        Mat::Sand => ground(tex, "sand", 1.5, 0xffffff, 1.0),
        Mat::Forest => ground(tex, "forest", 2.0, 0xffffff, 1.0),
        Mat::Soil => ground(tex, "forest", 2.0, 0x8a8478, 0.8),
        Mat::Planks => textured(plain(0xffffff, 1.0), tex, "planks", (2.0, 0.5)),
        Mat::Timber => textured(plain(0xb8aa98, 1.0), tex, "planks", (2.0, 0.5)),
        Mat::Zinc => textured(
            plain(0xffffff, 1.0).with_metallic(1.0),
            tex,
            "zinc",
            (0.4, 0.4),
        ),
        Mat::Steel => plain(0x3d3a36, 0.55).with_metallic(0.85),
        Mat::Leaf => {
            let mut m =
                textured(plain(0xffffff, 1.0), tex, "leaf", (1.0, 1.0)).with_subsurface(0.45);
            m.alpha_mode = AlphaMode::Mask(0.5);
            m.double_sided = true;
            m
        }
        Mat::Stone => textured(plain(0xffffff, 1.0), tex, "stone", (0.3, 0.3)),
        Mat::Grass => plain(0xffffff, 0.55)
            .with_subsurface(0.55)
            .with_wind_sway(0.045),
        Mat::Flowers => plain(0xffffff, 0.5)
            .with_subsurface(0.5)
            .with_wind_sway(0.04),
        Mat::Water => {
            // Murky and shallow: the ground under it shows through.
            let mut m = plain(0x2b2419, 0.03).with_alpha(0.72);
            m.reflectance = 0.5;
            m.receives_wetness = false;
            m
        }
        Mat::Straw => plain(0xffffff, 0.75).with_subsurface(0.3),
        Mat::Shell => plain(0xffffff, 0.45).with_subsurface(0.2),
        Mat::Mushroom => plain(0xffffff, 0.5).with_subsurface(0.35),
        Mat::Bark => textured(plain(0x7a6a5c, 1.0), tex, "planks", (0.6, 0.15)),
        Mat::Driftwood => textured(plain(0xcfc8bd, 1.0), tex, "planks", (1.2, 0.3)),
        Mat::Lamp => plain(0xfff4e0, 0.4).with_emissive([1.0, 0.78, 0.5], 900.0),
        Mat::LampBody => plain(0x2c2c2a, 0.5).with_metallic(0.6),
        Mat::Sealant => plain(0x1c1b19, 0.9),
        Mat::Sign(i) => {
            let c = super::rings::ring_color(super::STRETCHES[i as usize].0);
            let mut m = textured(
                StandardMaterial::color(c[0] * 0.55, c[1] * 0.55, c[2] * 0.55),
                tex,
                "planks",
                (0.6, 0.15),
            );
            m.perceptual_roughness = 0.75;
            m
        }
        Mat::Glow(i) => {
            let c = super::rings::ring_color(super::STRETCHES[i as usize].0);
            StandardMaterial::color(c[0] * 0.6, c[1] * 0.6, c[2] * 0.6)
                .with_roughness(0.3)
                .with_emissive(c, 60.0)
        }
    }
}

/// Light the underpass and keep the sky out under its roof (see `structures`): `sky_light`
/// left deep inside, rising to full over `fade` meters toward its open side and ends.
pub(super) fn sky_volume(
    half_extents: [f32; 3],
    open: engine_render::BoxFace,
    sky_light: f32,
    fade: f32,
) -> SkyLightVolume {
    SkyLightVolume::new(half_extents)
        .open(open)
        .open(engine_render::BoxFace::PosZ)
        .open(engine_render::BoxFace::NegZ)
        .with_sky_light(sky_light)
        .with_fade(fade)
}
