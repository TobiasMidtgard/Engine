//! The range's art: procedural textures and materials registered once by key, and the meshes
//! of every prop (built by the prop modules) registered once by key.
//!
//! Everything is registered in `Assets<Image>`, `Assets<StandardMaterial>` and `Assets<Mesh>`
//! under keys starting with `smoke/range/`, and entities name them with `MeshRef::Asset` and
//! `MaterialRef::Asset`. Without a renderer (tests, servers) nothing is built.

use engine_assets::{Assets, Handle};
use engine_core::math::{Mat4, Vec3};
use engine_core::prelude::*;
use engine_render::{AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial};

use super::kit;
use super::tex::{self, Texel};

// Material keys.
pub const CONCRETE: &str = "smoke/range/mat/concrete";
pub const PRECAST: &str = "smoke/range/mat/precast";
pub const SANDBAG: &str = "smoke/range/mat/sandbag";
pub const OLIVE: &str = "smoke/range/mat/olive";
pub const DARK: &str = "smoke/range/mat/dark_paint";
pub const FAN: &str = "smoke/range/mat/fan_paint";
pub const CART: &str = "smoke/range/mat/cart_paint";
pub const RED: &str = "smoke/range/mat/red_paint";
pub const LIGHT: &str = "smoke/range/mat/light_paint";
pub const GUNMETAL: &str = "smoke/range/mat/gunmetal";
pub const JACKET: &str = "smoke/range/mat/jacket";
pub const RUST: &str = "smoke/range/mat/rust";
pub const STEEL: &str = "smoke/range/mat/steel";
pub const BRASS: &str = "smoke/range/mat/brass";
pub const COPPER: &str = "smoke/range/mat/copper";
pub const TIMBER: &str = "smoke/range/mat/timber";
pub const SLEEPER: &str = "smoke/range/mat/sleeper";
pub const EARTH: &str = "smoke/range/mat/earth";
pub const GRAVEL: &str = "smoke/range/mat/gravel";
pub const LINE_WHITE: &str = "smoke/range/mat/line_white";
pub const LINE_YELLOW: &str = "smoke/range/mat/line_yellow";
pub const GALV: &str = "smoke/range/mat/galvanized";
pub const ALU: &str = "smoke/range/mat/aluminum";
pub const TARGET: &str = "smoke/range/mat/target";
pub const RUBBER: &str = "smoke/range/mat/rubber";
pub const SCORCH: &str = "smoke/range/mat/scorch";
pub const OIL: &str = "smoke/range/mat/oil";
pub const GRASS: &str = "smoke/range/mat/grass";
pub const GRAVEL_FINE: &str = "smoke/range/mat/gravel_fine";
pub const FLAG: &str = "smoke/range/mat/flag";

/// Meshes grouped by material key.
#[derive(Default, Clone)]
pub struct Parts(pub Vec<(&'static str, Mesh)>);

impl Parts {
    /// Add a mesh drawn with material `mat`.
    pub fn add(&mut self, mat: &'static str, m: Mesh) {
        if let Some((_, mm)) = self.0.iter_mut().find(|(k, _)| *k == mat) {
            mm.merge(&m);
        } else {
            self.0.push((mat, m));
        }
    }

    /// Add a mesh after transforming it.
    pub fn add_at(&mut self, mat: &'static str, m: Mesh, t: Mat4) {
        self.add(mat, kit::placed(m, t));
    }

    /// Add every part of `other`, transformed.
    pub fn absorb(&mut self, other: Parts, t: Mat4) {
        for (k, m) in other.0 {
            self.add(k, kit::placed(m, t));
        }
    }

    /// Multiply the vertex colors of every part by `f(position, normal)`.
    pub fn shade(&mut self, f: impl Fn(Vec3, Vec3) -> Vec3 + Copy) {
        for (_, m) in &mut self.0 {
            kit::shade(m, f);
        }
    }
}

/// Mesh key for one material of a prop group.
pub fn mesh_key(group: &str, mat: &str) -> String {
    format!(
        "smoke/range/mesh/{group}/{}",
        mat.rsplit('/').next().unwrap_or(mat)
    )
}

/// Register the parts of a prop group (once) and return (mesh key, material key) pairs to
/// spawn. Without a renderer the parts are not built, and `parts` is not called.
pub fn register(
    world: &mut World,
    group: &str,
    parts: impl FnOnce() -> Parts,
) -> Vec<(String, &'static str)> {
    let index_key = format!("smoke/range/mesh/{group}/#index");
    let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
        return Vec::new();
    };
    if meshes.by_key(&index_key).is_some() {
        return ALL_MATERIALS
            .iter()
            .map(|mat| (mesh_key(group, mat), *mat))
            .filter(|(key, _)| meshes.by_key(key).is_some())
            .collect();
    }
    let parts = parts();
    let mut out = Vec::new();
    for (mat, mesh) in parts.0 {
        if mesh.indices.is_empty() {
            continue;
        }
        let key = mesh_key(group, mat);
        meshes.insert_keyed(&key, mesh);
        out.push((key, mat));
    }
    meshes.insert_keyed(&index_key, Mesh::default());
    out
}

/// The renderer components for one registered part.
pub fn renderer(mesh: &str, mat: &str) -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Asset(mesh.to_string()),
        MaterialRef::Asset(mat.to_string()),
    )
}

/// Spawn one child of `parent` (at its origin) per registered part.
pub fn spawn_parts(world: &mut World, parent: Entity, parts: &[(String, &'static str)]) {
    for (mesh, mat) in parts {
        world.spawn((Transform::default(), renderer(mesh, mat), ChildOf(parent)));
    }
}

const ALL_MATERIALS: &[&str] = &[
    CONCRETE,
    PRECAST,
    SANDBAG,
    OLIVE,
    DARK,
    FAN,
    CART,
    RED,
    LIGHT,
    GUNMETAL,
    JACKET,
    RUST,
    STEEL,
    BRASS,
    COPPER,
    TIMBER,
    SLEEPER,
    EARTH,
    GRAVEL,
    LINE_WHITE,
    LINE_YELLOW,
    GALV,
    ALU,
    TARGET,
    RUBBER,
    SCORCH,
    OIL,
    GRASS,
    GRAVEL_FINE,
    FLAG,
];

/// A baked texture set: keys of its albedo, normal and ORM images.
struct Set {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    orm: Handle<Image>,
}

fn tex_key(name: &str, slot: &str) -> String {
    format!("smoke/range/tex/{name}/{slot}")
}

fn bake_set(
    images: &mut Assets<Image>,
    name: &str,
    size: u32,
    tile_m: f32,
    f: impl Fn(f32, f32) -> Texel + Sync,
) -> Set {
    if let (Some(albedo), Some(normal), Some(orm)) = (
        images.by_key(&tex_key(name, "albedo")),
        images.by_key(&tex_key(name, "normal")),
        images.by_key(&tex_key(name, "orm")),
    ) {
        return Set {
            albedo,
            normal,
            orm,
        };
    }
    let texels = tex::texels(size, f);
    let maps = tex::bake(size, tile_m, &texels);
    Set {
        albedo: images.insert_keyed(&tex_key(name, "albedo"), maps.albedo),
        normal: images.insert_keyed(&tex_key(name, "normal"), maps.normal),
        orm: images.insert_keyed(&tex_key(name, "orm"), maps.orm),
    }
}

/// How a material uses its texture set.
struct Look {
    base: [f32; 3],
    rough: f32,
    metal: f32,
    /// Texture tile size, meters (uv_scale = 1 / tile).
    tile_m: f32,
    normal_scale: f32,
    mask: Option<f32>,
    blend: bool,
    double_sided: bool,
}

impl Look {
    fn new(tile_m: f32) -> Self {
        Self {
            base: [1.0; 3],
            rough: 1.0,
            metal: 1.0,
            tile_m,
            normal_scale: 1.0,
            mask: None,
            blend: false,
            double_sided: false,
        }
    }
    fn base(mut self, b: [f32; 3]) -> Self {
        self.base = b;
        self
    }
    fn rough(mut self, r: f32) -> Self {
        self.rough = r;
        self
    }
    fn normals(mut self, s: f32) -> Self {
        self.normal_scale = s;
        self
    }
    fn mask(mut self, cutoff: f32) -> Self {
        self.mask = Some(cutoff);
        self
    }
    fn blend(mut self) -> Self {
        self.blend = true;
        self
    }
    fn two_sided(mut self) -> Self {
        self.double_sided = true;
        self
    }
}

fn material(set: &Set, albedo: Option<Handle<Image>>, look: &Look) -> StandardMaterial {
    let mut m = StandardMaterial::color(look.base[0], look.base[1], look.base[2])
        .with_roughness(look.rough)
        .with_metallic(look.metal)
        .with_base_color_texture(albedo.unwrap_or(set.albedo))
        .with_normal_texture(set.normal)
        .with_metallic_roughness_texture(set.orm)
        .with_occlusion_texture(set.orm)
        .with_uv_scale(1.0 / look.tile_m, 1.0 / look.tile_m);
    m.normal_scale = look.normal_scale;
    m.double_sided = look.double_sided;
    if let Some(c) = look.mask {
        m.alpha_mode = AlphaMode::Mask(c);
    }
    if look.blend {
        m.alpha_mode = AlphaMode::Blend;
    }
    m
}

/// Paint colors over the shared painted-metal pattern: (name, linear color).
fn paint_colors() -> [(&'static str, Vec3); 6] {
    [
        ("olive", tex::hex(0x4a4b2f)),
        ("dark", tex::hex(0x2a2c2b)),
        ("fan", tex::hex(0x4f6b78)),
        ("cart", tex::hex(0xd99a1e)),
        ("red", tex::hex(0x9c1f17)),
        ("light", tex::hex(0xd8d6cf)),
    ]
}

/// Bake every texture and register every material (once; later calls return at once).
pub fn ensure_materials(world: &mut World) {
    let done_key = "smoke/range/mat/#done";
    let (Some(_), Some(_)) = (
        world.get_resource::<Assets<Image>>(),
        world.get_resource::<Assets<StandardMaterial>>(),
    ) else {
        return;
    };
    if world
        .resource::<Assets<StandardMaterial>>()
        .by_key(done_key)
        .is_some()
    {
        return;
    }
    let started = std::time::Instant::now();
    let mut mats: Vec<(&'static str, StandardMaterial)> = Vec::new();
    {
        let mut images = world.resource_mut::<Assets<Image>>();
        let images = &mut *images;

        let concrete = bake_set(images, "concrete", 2048, 3.0, |u, v| tex::concrete(u, v, 7));
        mats.push((CONCRETE, material(&concrete, None, &Look::new(3.0))));
        mats.push((
            PRECAST,
            material(
                &concrete,
                None,
                &Look::new(2.1).base([1.18, 1.16, 1.12]).normals(1.3),
            ),
        ));

        let cloth = bake_set(images, "cloth", 1024, 0.12, |u, v| tex::cloth(u, v, 3));
        mats.push((SANDBAG, material(&cloth, None, &Look::new(0.12))));
        let mut flag = material(
            &cloth,
            None,
            &Look::new(0.05)
                .base([1.55, 0.22, 0.2])
                .two_sided()
                .normals(0.5),
        );
        flag.subsurface = 0.45;
        mats.push((FLAG, flag));

        // Painted metal: one wear pattern, one albedo per paint color.
        let paint = if let (Some(normal), Some(orm)) = (
            images.by_key(&tex_key("paint", "normal")),
            images.by_key(&tex_key("paint", "orm")),
        ) {
            Set {
                albedo: normal,
                normal,
                orm,
            }
        } else {
            let size = 1024;
            let wear = tex::grid(size, Default::default(), |u, v| {
                tex::paint_wear(u, v, 0.6, 17)
            });
            let texels: Vec<Texel> = wear.iter().map(|w| w.1).collect();
            let maps = tex::bake(size, 0.5, &texels);
            let normal = images.insert_keyed(&tex_key("paint", "normal"), maps.normal);
            let orm = images.insert_keyed(&tex_key("paint", "orm"), maps.orm);
            let n = size as usize;
            for (name, color) in paint_colors() {
                let albedo = tex::texels(size, |u, v| {
                    let (x, y) = ((u * n as f32) as usize, (v * n as f32) as usize);
                    let w = &wear[y.min(n - 1) * n + x.min(n - 1)].0;
                    Texel {
                        albedo: tex::paint_albedo(w, color, u, v, 29),
                        ..Default::default()
                    }
                });
                images.insert_keyed(&tex_key("paint", name), tex::albedo_image(size, &albedo));
            }
            Set {
                albedo: normal,
                normal,
                orm,
            }
        };
        for (key, name, rough) in [
            (OLIVE, "olive", 1.0),
            (DARK, "dark", 1.0),
            (FAN, "fan", 0.9),
            (CART, "cart", 1.0),
            (RED, "red", 0.85),
            (LIGHT, "light", 1.0),
        ] {
            let albedo = images.by_key(&tex_key("paint", name));
            mats.push((key, material(&paint, albedo, &Look::new(0.5).rough(rough))));
        }

        let gun = bake_set(images, "gunmetal", 1024, 0.3, |u, v| tex::gunmetal(u, v, 5));
        mats.push((GUNMETAL, material(&gun, None, &Look::new(0.3))));
        let jacket = bake_set(images, "jacket", 512, 0.1, |u, v| tex::jacket(u, v, 5));
        mats.push((
            JACKET,
            material(&jacket, None, &Look::new(0.1).mask(0.5).two_sided()),
        ));
        let rusty = bake_set(images, "rusty", 1024, 0.6, |u, v| tex::rusty(u, v, 11));
        mats.push((RUST, material(&rusty, None, &Look::new(0.6))));
        let polished = bake_set(images, "polished", 512, 0.25, |u, v| {
            tex::polished(u, v, 13)
        });
        mats.push((STEEL, material(&polished, None, &Look::new(0.25))));
        mats.push((
            BRASS,
            material(
                &polished,
                None,
                &Look::new(0.25).base([1.45, 1.1, 0.55]).rough(1.3),
            ),
        ));
        mats.push((
            COPPER,
            material(
                &polished,
                None,
                &Look::new(0.25).base([1.45, 0.82, 0.55]).rough(1.4),
            ),
        ));
        let timber = bake_set(images, "timber", 1024, 1.2, |u, v| tex::timber(u, v, 19));
        mats.push((TIMBER, material(&timber, None, &Look::new(1.2))));
        mats.push((
            SLEEPER,
            material(
                &timber,
                None,
                &Look::new(1.0).base([0.46, 0.4, 0.36]).normals(1.2),
            ),
        ));
        let earth = bake_set(images, "earth", 1024, 2.5, |u, v| tex::earth(u, v, 23));
        mats.push((EARTH, material(&earth, None, &Look::new(2.5))));
        let gravel = bake_set(images, "gravel", 1024, 1.0, |u, v| tex::gravel(u, v, 31));
        mats.push((GRAVEL, material(&gravel, None, &Look::new(1.0))));
        let line = bake_set(images, "line", 512, 1.0, |u, v| tex::line_paint(u, v, 37));
        mats.push((
            LINE_WHITE,
            material(
                &line,
                None,
                &Look::new(1.0).base([0.8, 0.8, 0.77]).mask(0.5),
            ),
        ));
        mats.push((
            LINE_YELLOW,
            material(
                &line,
                None,
                &Look::new(1.0).base([1.0, 0.62, 0.05]).mask(0.5),
            ),
        ));
        let galv = bake_set(images, "galvanized", 512, 0.3, |u, v| {
            tex::galvanized(u, v, 41)
        });
        mats.push((GALV, material(&galv, None, &Look::new(0.3))));
        mats.push((
            ALU,
            material(
                &galv,
                None,
                &Look::new(0.6).base([1.12, 1.12, 1.14]).rough(0.85),
            ),
        ));
        let hits = tex::TargetHits::new(43);
        let target = bake_set(images, "target", 2048, 1.2, |u, v| {
            tex::target_face(&hits, u, v, 43)
        });
        mats.push((TARGET, material(&target, None, &Look::new(1.0))));
        let rubber = bake_set(images, "rubber", 256, 0.1, |u, v| tex::rubber(u, v, 47));
        mats.push((RUBBER, material(&rubber, None, &Look::new(0.1))));
        let scorch = bake_set(images, "scorch", 512, 1.0, |u, v| tex::scorch(u, v, 53));
        mats.push((SCORCH, material(&scorch, None, &Look::new(1.0).blend())));
        let oil = bake_set(images, "oil", 256, 1.0, |u, v| tex::oil(u, v, 59));
        mats.push((OIL, material(&oil, None, &Look::new(1.0).blend())));
        let grass = bake_set(images, "grass", 512, 0.5, |u, v| tex::grass(u, v, 61));
        let mut g = material(&grass, None, &Look::new(1.0).mask(0.45));
        g.subsurface = 0.3;
        g.receives_wetness = false;
        mats.push((GRASS, g));
        mats.push((
            GRAVEL_FINE,
            material(
                &gravel,
                None,
                &Look::new(0.4).base([0.95, 0.9, 0.82]).normals(0.6),
            ),
        ));
    }
    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    for (key, m) in mats {
        materials.insert_keyed(key, m);
    }
    materials.insert_keyed(done_key, StandardMaterial::default());
    tracing::info!(
        "smoke range: baked textures in {:.2} s",
        started.elapsed().as_secs_f32()
    );
}
