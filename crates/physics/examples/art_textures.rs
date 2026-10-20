//! Preview harness for the proving ground's procedural PBR texture sets
//! (`src/demo/art/textures.rs`). Times every set, writes its maps, renders lit swatches with
//! the engine (a sphere, a rounded cube and a 3 × 3 m plane under a sun and sky, plus a wide
//! field of the texture for tiling) and saves one contact sheet per set:
//!
//! ```text
//! | albedo | normal | ORM |
//! | 0.5 m close-up      | 3 m: sphere, cube, plane |
//! | 1.2 m object detail | 15 m: wide field          |
//! ```
//!
//! ```sh
//! cargo run -p engine_physics --example art_textures -- <out_dir> [filter] [work_dir]
//! ```
//!
//! `filter` keeps the sets whose name contains it (`all` or empty for every set). Rendering
//! needs a GPU or a software Vulkan driver (lavapipe: `apt-get install mesa-vulkan-drivers`);
//! without one only the maps are written. With `ART_TEXTURES_TIME_ONLY=1` the sets are only
//! generated (best of five runs) and timed: nothing is written or rendered.

#[path = "../src/demo/art/textures.rs"]
mod textures;

use std::f32::consts::{FRAC_PI_4, PI, TAU};
use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::{RgbImage, RgbaImage, imageops};
use serde_json::{Value, json};
use textures::TextureSet;

/// One set to preview: how to make it and how to show it.
struct Preview {
    name: &'static str,
    make: Box<dyn Fn() -> TextureSet>,
    /// Linear base color factor used in the renders (tint for rubber / vinyl / crates).
    tint: [f32; 3],
}

const VIEW_W: u32 = 768;
const VIEW_H: u32 = 512;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or("art_textures".into()));
    let filter = args
        .get(2)
        .cloned()
        .filter(|f| f != "all")
        .unwrap_or_default();
    let work = args
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| out.join("work"));
    let time_only = std::env::var_os("ART_TEXTURES_TIME_ONLY").is_some();
    if !time_only {
        std::fs::create_dir_all(&out).expect("output directory");
        std::fs::create_dir_all(&work).expect("work directory");
    }
    let previews = previews();
    let mut made = Vec::new();
    for p in previews.iter().filter(|p| p.name.contains(filter.as_str())) {
        // Best of several runs: the first also pays for page faults and thread start-up.
        let mut best = f64::MAX;
        let mut set = None;
        for _ in 0..if time_only { 5 } else { 2 } {
            let t = Instant::now();
            let s = (p.make)();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            set = Some(s);
        }
        let set = set.expect("generated");
        let stats = map_stats(&set);
        println!(
            "{:<18} {:>4}²  {:7.1} ms   albedo avg sRGB #{:02x}{:02x}{:02x} (min luma {}, max {})  roughness {:.2}..{:.2} avg {:.2}  metal avg {:.2}  ao avg {:.2}",
            p.name,
            set.albedo.width,
            best,
            stats.avg_srgb[0],
            stats.avg_srgb[1],
            stats.avg_srgb[2],
            stats.min_luma,
            stats.max_luma,
            stats.rough[0],
            stats.rough[1],
            stats.rough[2],
            stats.metal,
            stats.ao,
        );
        if !time_only {
            write_set(&work, p.name, &set);
            made.push((p, set));
        }
    }
    if time_only {
        return;
    }
    // A seeded crate variant, to check it differs from the default boards.
    if "crate_wood".contains(filter.as_str()) || filter.is_empty() {
        let t = Instant::now();
        let v = textures::crate_wood_seeded(512, 1);
        println!(
            "crate_wood_seeded(512, 1): {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
        save_png(&work.join("crate_wood_seed1_albedo.png"), &v.albedo);
    }
    if made.is_empty() {
        eprintln!("no set matches '{filter}'");
        return;
    }

    let Some(mut app) = render_app(&work) else {
        return;
    };
    write_meshes(&work);
    load(&mut app, "meshes.gltf");
    for (p, set) in &made {
        let views = render_set(&mut app, &work, p, set);
        if views.is_empty() {
            return;
        }
        let sheet = contact_sheet(set, &views);
        let path = out.join(format!("{}.jpg", p.name));
        save_jpeg(&path, &sheet);
        println!("{}", path.display());
    }
}

fn previews() -> Vec<Preview> {
    let white = [1.0; 3];
    vec![
        Preview {
            name: "concrete_slab",
            make: Box::new(|| textures::concrete_slab(1024)),
            tint: white,
        },
        Preview {
            name: "cast_concrete",
            make: Box::new(|| textures::cast_concrete(512)),
            tint: white,
        },
        Preview {
            name: "crate_wood",
            make: Box::new(|| textures::crate_wood(512)),
            tint: white,
        },
        Preview {
            name: "weathered_timber",
            make: Box::new(|| textures::weathered_timber(512)),
            tint: white,
        },
        Preview {
            name: "painted_steel",
            make: Box::new(|| textures::painted_steel(512, [0.72, 0.16, 0.02], 0.45)),
            tint: white,
        },
        Preview {
            name: "painted_steel_worn",
            make: Box::new(|| textures::painted_steel(512, [0.04, 0.12, 0.3], 0.9)),
            tint: white,
        },
        Preview {
            name: "forged_steel",
            make: Box::new(|| textures::forged_steel(512)),
            tint: white,
        },
        Preview {
            name: "diamond_plate",
            make: Box::new(|| textures::diamond_plate(512)),
            tint: white,
        },
        Preview {
            name: "rubber",
            make: Box::new(|| textures::rubber(512)),
            tint: [0.62, 0.06, 0.04],
        },
        Preview {
            name: "vinyl_skin",
            make: Box::new(|| textures::vinyl_skin(512)),
            tint: [0.95, 0.6, 0.04],
        },
        Preview {
            name: "hazard_stripes",
            make: Box::new(|| textures::hazard_stripes(512)),
            tint: white,
        },
        Preview {
            name: "lawn",
            make: Box::new(|| textures::lawn(512)),
            tint: white,
        },
        Preview {
            name: "asphalt",
            make: Box::new(|| textures::asphalt(1024)),
            tint: white,
        },
        Preview {
            name: "raked_sand",
            make: Box::new(|| textures::raked_sand(512)),
            tint: white,
        },
        Preview {
            name: "galvanized",
            make: Box::new(|| textures::galvanized(512)),
            tint: white,
        },
        Preview {
            name: "granite",
            make: Box::new(|| textures::granite(512)),
            tint: white,
        },
        Preview {
            name: "sanded_wood",
            make: Box::new(|| textures::sanded_wood(512, 7)),
            tint: white,
        },
    ]
}

// ---------------------------------------------------------------------------------- stats

struct Stats {
    avg_srgb: [u8; 3],
    min_luma: u8,
    max_luma: u8,
    rough: [f32; 3],
    metal: f32,
    ao: f32,
}

fn map_stats(set: &TextureSet) -> Stats {
    let to_lin = |s: u8| {
        let s = s as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |l: f32| {
        let s = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0 + 0.5) as u8
    };
    let mut sum = [0.0f64; 3];
    let (mut lo, mut hi) = (255u8, 0u8);
    let px = set.albedo.data.as_chunks::<4>().0;
    let count = px.len() as f64;
    for p in px {
        for (s, &c) in sum.iter_mut().zip(p) {
            *s += to_lin(c) as f64;
        }
        let luma = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) as u8;
        lo = lo.min(luma);
        hi = hi.max(luma);
    }
    let (mut rmin, mut rmax, mut rsum, mut msum, mut aosum) = (1.0f32, 0.0f32, 0.0, 0.0, 0.0);
    for p in set.orm.data.as_chunks::<4>().0 {
        let r = p[1] as f32 / 255.0;
        rmin = rmin.min(r);
        rmax = rmax.max(r);
        rsum += r as f64;
        msum += p[2] as f64 / 255.0;
        aosum += p[0] as f64 / 255.0;
    }
    Stats {
        avg_srgb: sum.map(|s| to_srgb((s / count) as f32)),
        min_luma: lo,
        max_luma: hi,
        rough: [rmin, rmax, (rsum / count) as f32],
        metal: (msum / count) as f32,
        ao: (aosum / count) as f32,
    }
}

// ---------------------------------------------------------------------------------- files

fn rgba(img: &engine_render::Image) -> RgbaImage {
    RgbaImage::from_raw(img.width, img.height, img.data.clone()).expect("RGBA8 image")
}

fn save_png(path: &Path, img: &engine_render::Image) {
    rgba(img).save(path).expect("write png");
}

fn save_jpeg(path: &Path, img: &RgbImage) {
    let file = std::fs::File::create(path).expect("create jpeg");
    let mut w = std::io::BufWriter::new(file);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 86)
        .encode_image(img)
        .expect("encode jpeg");
}

/// Write the three maps and a glTF that references them, so the asset server loads them with
/// the right color spaces (albedo sRGB, normal and ORM linear) and mip chains.
fn write_set(work: &Path, name: &str, set: &TextureSet) {
    let dir = work.join(name);
    std::fs::create_dir_all(&dir).expect("set directory");
    save_png(&dir.join("albedo.png"), &set.albedo);
    save_png(&dir.join("normal.png"), &set.normal);
    save_png(&dir.join("orm.png"), &set.orm);
    let gltf = json!({
        "asset": {"version": "2.0"},
        "images": [{"uri": "albedo.png"}, {"uri": "normal.png"}, {"uri": "orm.png"}],
        "textures": [{"source": 0}, {"source": 1}, {"source": 2}],
        "materials": [{
            "pbrMetallicRoughness": {
                "baseColorTexture": {"index": 0},
                "metallicRoughnessTexture": {"index": 2}
            },
            "normalTexture": {"index": 1},
            "occlusionTexture": {"index": 2}
        }]
    });
    std::fs::write(dir.join("set.gltf"), gltf.to_string()).expect("write gltf");
}

/// Swatch meshes with metric UVs (1 UV unit = 1 m): Mesh0 a 1 m cube with 6 cm rounded
/// edges, Mesh1 a 0.5 m radius sphere.
fn write_meshes(work: &Path) {
    let meshes = [rounded_cube(1.0, 0.06, 10), metric_sphere(0.5, 64)];
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut gltf_meshes = Vec::new();
    for m in &meshes {
        let mut view = |bytes: &[u8]| {
            let offset = bin.len();
            bin.extend_from_slice(bytes);
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len()}));
            views.len() - 1
        };
        let flat = |v: &[[f32; 3]]| -> Vec<u8> {
            v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
        };
        let pv = view(&flat(&m.positions));
        let nv = view(&flat(&m.normals));
        let uv_bytes: Vec<u8> = m
            .uvs
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let tv = view(&uv_bytes);
        let iv = view(
            &m.indices
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &m.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let base = accessors.len();
        let count = m.positions.len();
        accessors.push(json!({"bufferView": pv, "componentType": 5126, "count": count, "type": "VEC3", "min": lo, "max": hi}));
        accessors
            .push(json!({"bufferView": nv, "componentType": 5126, "count": count, "type": "VEC3"}));
        accessors
            .push(json!({"bufferView": tv, "componentType": 5126, "count": count, "type": "VEC2"}));
        accessors.push(json!({"bufferView": iv, "componentType": 5125, "count": m.indices.len(), "type": "SCALAR"}));
        gltf_meshes.push(json!({"primitives": [{
            "attributes": {"POSITION": base, "NORMAL": base + 1, "TEXCOORD_0": base + 2},
            "indices": base + 3
        }]}));
    }
    std::fs::write(work.join("meshes.bin"), &bin).expect("write bin");
    let gltf = json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": bin.len(), "uri": "meshes.bin"}],
        "bufferViews": views,
        "accessors": accessors,
        "meshes": gltf_meshes,
    });
    std::fs::write(work.join("meshes.gltf"), gltf.to_string()).expect("write gltf");
}

struct MeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// Cube of edge `size` with edges rounded at `radius`; UVs in meters along each face, arc
/// length over the rounded edges. `band` vertices per rounded edge half.
fn rounded_cube(size: f32, radius: f32, band: usize) -> MeshData {
    let h = size * 0.5;
    let inner = h - radius;
    // Face coordinates: dense over the rounding (uniform in angle), sparse over the flat.
    let mut s: Vec<f32> = Vec::new();
    for k in (1..=band).rev() {
        let a = k as f32 / band as f32 * FRAC_PI_4;
        s.push(-inner - radius * a.tan());
    }
    let flat = 8;
    for k in 0..=flat {
        s.push(-inner + 2.0 * inner * k as f32 / flat as f32);
    }
    for k in 1..=band {
        let a = k as f32 / band as f32 * FRAC_PI_4;
        s.push(inner + radius * a.tan());
    }
    let arc = |x: f32| {
        if x.abs() <= inner {
            x
        } else {
            x.signum() * (inner + radius * ((x.abs() - inner) / radius).atan())
        }
    };
    let half_arc = inner + radius * FRAC_PI_4;
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut m = MeshData {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
    };
    let k = s.len();
    for (n, u, v) in faces {
        let base = m.positions.len() as u32;
        for &t in &s {
            for &x in &s {
                let q = n * h + u * x + v * t;
                let c = q.clamp(Vec3::splat(-inner), Vec3::splat(inner));
                let d = (q - c).normalize_or(n);
                m.positions.push((c + d * radius).to_array());
                m.normals.push(d.to_array());
                m.uvs.push([arc(x) + half_arc, half_arc - arc(t)]);
            }
        }
        for j in 0..k as u32 - 1 {
            for i in 0..k as u32 - 1 {
                let a = base + j * k as u32 + i;
                let b = a + 1;
                let c = a + k as u32 + 1;
                let d = a + k as u32;
                m.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    m
}

/// UV sphere with UVs in meters at the equator (U around, V pole to pole).
fn metric_sphere(radius: f32, segments: u32) -> MeshData {
    let rings = segments / 2;
    let mut m = MeshData {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
    };
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * TAU;
            let n = [phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin()];
            m.positions.push(n.map(|c| c * radius));
            m.normals.push(n);
            m.uvs.push([u * TAU * radius, v * PI * radius]);
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let i = r * row + s;
            m.indices
                .extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    m
}

// ---------------------------------------------------------------------------------- rendering

fn render_app(work: &Path) -> Option<App> {
    let mut app = App::new();
    {
        let mut cfg = app.world.resource_mut::<EngineConfig>();
        cfg.headless = true;
        cfg.asset_root = work.display().to_string();
    }
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = VIEW_W;
        win.height = VIEW_H;
    }
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<AmbientLight>().brightness = 400.0;
    // Neutral ground far around the swatches.
    app.world.spawn((
        Transform::from_xyz(0.0, -0.01, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [800.0, 800.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6e6a64).with_roughness(0.95),
        ),
    ));
    // Sun from the right, a little behind the camera, 33° up: raking enough to show relief.
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.72, -0.55, -0.42), Vec3::Y),
    ));
    app.world.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(0.0, 2.0, 4.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU, maps only: {}", d.0);
        return None;
    }
    Some(app)
}

/// Load a file through the asset server and return its keys → handles.
fn load(app: &mut App, path: &str) -> Vec<(String, u64)> {
    let info: Value = call_api(
        &mut app.world,
        "assets.load",
        json!({"path": path, "wait": true}),
    )
    .unwrap_or_else(|e| panic!("loading {path}: {e}"));
    if info["status"] != "loaded" {
        panic!("loading {path}: {info}");
    }
    info["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|k| {
                    (
                        k["key"].as_str().unwrap_or_default().to_string(),
                        k["handle"].as_u64().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Render the four views of one set; returns the captured images (empty without a GPU).
fn render_set(app: &mut App, work: &Path, p: &Preview, set: &TextureSet) -> Vec<RgbaImage> {
    let keys = load(app, &format!("{}/set.gltf", p.name));
    let handle = |label: &str| {
        keys.iter()
            .find(|(k, _)| k.ends_with(label))
            .map(|(_, h)| *h)
            .unwrap_or_else(|| panic!("{} has no {label}", p.name))
    };
    let (albedo, normal, orm) = (handle("#Image0"), handle("#Image1"), handle("#Image2"));
    let s = 1.0 / set.tile_m;
    let material: StandardMaterial = serde_json::from_value(json!({
        "base_color": [p.tint[0], p.tint[1], p.tint[2], 1.0],
        "metallic": 1.0,
        "perceptual_roughness": 1.0,
        "base_color_texture": albedo,
        "normal_texture": normal,
        "metallic_roughness_texture": orm,
        "occlusion_texture": orm,
        "uv_scale": [s, s],
        "normal_scale": 1.0,
        "occlusion_strength": 1.0,
    }))
    .expect("material json");

    let swatch = 3.0f32;
    let field = 36.0f32;
    let field_z = -60.0;
    let mut spawned = vec![
        app.world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [swatch, swatch],
                        subdivisions: 1,
                    },
                    material,
                ),
            ))
            .id(),
        app.world
            .spawn((
                Transform::from_xyz(0.75, 0.5, -0.2).with_rotation(Quat::from_rotation_y(0.45)),
                MeshRenderer::new(
                    MeshRef::Asset("meshes.gltf#Mesh0/Primitive0".into()),
                    material,
                ),
            ))
            .id(),
        app.world
            .spawn((
                Transform::from_xyz(-0.7, 0.5, 0.15),
                MeshRenderer::new(
                    MeshRef::Asset("meshes.gltf#Mesh1/Primitive0".into()),
                    material,
                ),
            ))
            .id(),
        app.world
            .spawn((
                Transform::from_xyz(0.0, 0.0, field_z),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [field, field],
                        subdivisions: 1,
                    },
                    material,
                ),
            ))
            .id(),
    ];

    // Close-up target: a tile corner inside the swatch if there is one (seam check), else a
    // point off-center on the plane. The plane's UVs start at its -X/-Z corner.
    let corner = (swatch * 0.5 / set.tile_m).ceil() * set.tile_m - swatch * 0.5;
    let close_target = if corner < swatch * 0.5 - 0.2 {
        [corner as f64, 0.0, corner as f64]
    } else {
        [-0.9, 0.0, 1.0]
    };
    let views: [([f64; 3], [f64; 3]); 4] = [
        (
            [close_target[0] + 0.1, 0.42, close_target[2] + 0.3],
            close_target,
        ),
        ([0.4, 1.75, 3.1], [0.0, 0.35, 0.0]),
        ([-0.15, 0.95, 1.05], [-0.35, 0.45, 0.0]),
        ([2.0, 6.5, field_z + 13.5], [0.0, 0.0, field_z]),
    ];
    let camera = app
        .world
        .query_filtered::<Entity, With<Camera>>()
        .single(&app.world)
        .expect("camera");
    let mut shots = Vec::new();
    for (k, (eye, target)) in views.iter().enumerate() {
        *app.world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(DVec3::from_array(*eye))
                .looking_at(DVec3::from_array(*target), Vec3::Y);
        for _ in 0..5 {
            app.update();
        }
        let path = work.join(format!("{}_view{k}.png", p.name));
        if let Err(e) = capture_now(&mut app.world, &path) {
            eprintln!("{}: {e}", p.name);
            return Vec::new();
        }
        shots.push(image::open(&path).expect("read capture").to_rgba8());
    }
    for e in spawned.drain(..) {
        app.world.despawn(e);
    }
    shots
}

/// Maps on top (512 px each), the four renders below in two rows.
fn contact_sheet(set: &TextureSet, views: &[RgbaImage]) -> RgbImage {
    let w = 3 * 512;
    let h = 512 + 2 * VIEW_H;
    let mut sheet = RgbaImage::from_pixel(w, h, image::Rgba([20, 20, 20, 255]));
    for (i, map) in [&set.albedo, &set.normal, &set.orm].iter().enumerate() {
        let img = rgba(map);
        let img = if img.width() == 512 {
            img
        } else {
            imageops::resize(&img, 512, 512, imageops::FilterType::Triangle)
        };
        imageops::overlay(&mut sheet, &img, i as i64 * 512, 0);
    }
    for (i, v) in views.iter().enumerate() {
        let x = (i % 2) as i64 * VIEW_W as i64;
        let y = 512 + (i / 2) as i64 * VIEW_H as i64;
        imageops::overlay(&mut sheet, v, x, y);
    }
    image::DynamicImage::ImageRgba8(sheet).to_rgb8()
}
