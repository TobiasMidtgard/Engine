//! Offscreen renders of textured materials: procedural images, normal-map orientation, a glTF
//! model with an embedded texture, alpha-masked shadow casters and mirrored instances. Needs a
//! GPU or a software Vulkan driver (lavapipe); passes with a note when neither is available.
//!
//! Every render also fails on GPU validation errors: the renderer only logs those (and drops
//! the frame), so pixel checks alone could miss them.

use std::path::{Path, PathBuf};

use engine_assets::{AssetPlugin, AssetServer, Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const SIZE: u32 = 256;
const FOV_Y_DEG: f32 = 60.0;
const CAMERA_DISTANCE: f32 = 3.0;

/// Headless app with a camera at `+Z` looking at the origin and a sun shining from the
/// upper left, slightly towards the camera's side.
fn front_lit_app(asset_root: Option<PathBuf>) -> App {
    // Capture errors in the log buffer so `render` can check for GPU validation errors.
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = SIZE;
        w.height = SIZE;
    }
    app.add_plugins(AssetPlugin {
        hot_reload: false,
        root: asset_root,
    });
    app.add_plugins(RenderPlugin);
    app.world.spawn((
        Camera::perspective(FOV_Y_DEG),
        Transform::from_xyz(0.0, 0.0, CAMERA_DISTANCE as f64).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.3, -1.0), Vec3::Y),
    ));
    app
}

/// Render a few frames and read the picture back; `None` (with a note) without a GPU.
/// Panics if the GPU reported a validation error.
fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {name}: no GPU ({})", d.0);
        return None;
    }
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let gpu_errors: Vec<String> = engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect();
    assert!(
        gpu_errors.is_empty(),
        "GPU errors while rendering {name}: {gpu_errors:#?}"
    );
    Some(image::open(&path).expect("png").to_rgba8())
}

fn add_image(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

/// Pixel of a point on the `z = 0` plane, seen by the camera of [`front_lit_app`].
fn project(x: f32, y: f32) -> (u32, u32) {
    let half = CAMERA_DISTANCE * (FOV_Y_DEG.to_radians() * 0.5).tan();
    let px = (x / half * 0.5 + 0.5) * SIZE as f32;
    let py = (0.5 - y / half * 0.5) * SIZE as f32;
    (px as u32, py as u32)
}

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// Mean color in a small box around a pixel.
fn mean_rgb(img: &RgbaImage, (cx, cy): (u32, u32)) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for y in cy.saturating_sub(2)..=(cy + 2).min(img.height() - 1) {
        for x in cx.saturating_sub(2)..=(cx + 2).min(img.width() - 1) {
            let p = img.get_pixel(x, y);
            for c in 0..3 {
                sum[c] += p[c] as f32;
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

fn mean_luma(img: &RgbaImage, at: (u32, u32)) -> f32 {
    let [r, g, b] = mean_rgb(img, at);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// RGBA8 texel of a tangent-space normal.
fn encode_normal(n: Vec3) -> [u8; 4] {
    let n = n.normalize();
    let c = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
    [c(n.x), c(n.y), c(n.z), 255]
}

#[test]
fn checker_texture_with_normal_map_varies_across_the_surface() {
    let mut app = front_lit_app(None);
    // 4×4 checker: cell (i, j) is bright when i + j is even.
    let checker = add_image(
        &mut app,
        Image::checker(64, 4, [200, 200, 200, 255], [20, 20, 20, 255]),
    );
    // Gentle bumps, finer than the checker cells.
    let bumps = add_image(
        &mut app,
        Image::from_fn(64, 64, false, |x, y| {
            let a = x as f32 / 64.0 * std::f32::consts::TAU * 8.0;
            let b = y as f32 / 64.0 * std::f32::consts::TAU * 8.0;
            encode_normal(Vec3::new(0.35 * a.sin(), 0.35 * b.cos(), 1.0))
        }),
    );
    let material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.8)
        .with_base_color_texture(checker)
        .with_normal_texture(bumps);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(Shape::Quad { size: [2.0, 2.0] }, material),
    ));
    let Some(img) = render(&mut app, "textured_checker") else {
        return;
    };

    // Cell centers: u, v in [0, 1] map to x in [-1, 1] and y in [1, -1].
    let mut bright = Vec::new();
    let mut dark = Vec::new();
    for j in 0..4 {
        for i in 0..4 {
            let x = -1.0 + (i as f32 + 0.5) * 0.5;
            let y = 1.0 - (j as f32 + 0.5) * 0.5;
            let l = mean_luma(&img, project(x, y));
            if (i + j) % 2 == 0 {
                bright.push(l)
            } else {
                dark.push(l)
            }
        }
    }
    let min_bright = bright.iter().copied().fold(f32::MAX, f32::min);
    let max_dark = dark.iter().copied().fold(0.0, f32::max);
    assert!(
        min_bright > max_dark * 1.5 + 10.0,
        "checker cells do not alternate: bright {bright:?}, dark {dark:?}"
    );

    // Lots of distinct shades inside the quad (texture filtering and bumps).
    let (x0, y0) = project(-0.9, 0.9);
    let (x1, y1) = project(0.9, -0.9);
    let mut distinct = std::collections::HashSet::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y);
            distinct.insert([p[0] / 8, p[1] / 8, p[2] / 8]);
        }
    }
    assert!(
        distinct.len() > 20,
        "textured surface looks flat ({} shades)",
        distinct.len()
    );

    let stats = app.world.resource::<engine_render::RenderStats>();
    eprintln!("textured checker on {}: {stats:?}", stats.adapter);
}

#[test]
fn normal_map_tilts_follow_the_gltf_convention() {
    let mut app = front_lit_app(None);
    // Quadrants (u right, v down): top-left tilts +X, top-right −X, bottom-left +Y ("up" in
    // the image, glTF convention), bottom-right −Y.
    let normals = add_image(
        &mut app,
        Image::from_fn(64, 64, false, |x, y| {
            let t = 0.6;
            let n = match (x < 32, y < 32) {
                (true, true) => Vec3::new(t, 0.0, 0.8),
                (false, true) => Vec3::new(-t, 0.0, 0.8),
                (true, false) => Vec3::new(0.0, t, 0.8),
                (false, false) => Vec3::new(0.0, -t, 0.8),
            };
            encode_normal(n)
        }),
    );
    let material = StandardMaterial::color(0.25, 0.25, 0.25)
        .with_roughness(0.8)
        .with_normal_texture(normals);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(Shape::Quad { size: [2.0, 2.0] }, material),
    ));
    let Some(img) = render(&mut app, "normal_map_quadrants") else {
        return;
    };
    // The light comes from −X, +Y, +Z: normals tilted towards −X and +Y face it more.
    let plus_x = mean_luma(&img, project(-0.5, 0.5));
    let minus_x = mean_luma(&img, project(0.5, 0.5));
    let plus_y = mean_luma(&img, project(-0.5, -0.5));
    let minus_y = mean_luma(&img, project(0.5, -0.5));
    eprintln!("normal map quadrants: +x {plus_x} −x {minus_x} +y {plus_y} −y {minus_y}");
    assert!(
        minus_x > plus_x * 1.08 + 3.0,
        "X tilt has the wrong sign: +x {plus_x}, −x {minus_x}"
    );
    assert!(
        plus_y > minus_y * 1.08 + 3.0,
        "Y tilt has the wrong sign: +y {plus_y}, −y {minus_y}"
    );
}

/// Standard base64 with padding.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A 2×2 m card facing +Z whose texture (embedded PNG) is red on the left and blue on the
/// right; the vertex buffer is embedded too and the file has no normals.
fn write_card_gltf(dir: &Path) {
    let texture = RgbaImage::from_fn(8, 8, |x, _| {
        if x < 4 {
            image::Rgba([220, 30, 30, 255])
        } else {
            image::Rgba([30, 60, 220, 255])
        }
    });
    let mut png = std::io::Cursor::new(Vec::new());
    texture.write_to(&mut png, image::ImageFormat::Png).unwrap();

    let mut bin: Vec<u8> = Vec::new();
    for p in [
        [-1.0f32, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ] {
        p.iter()
            .for_each(|v| bin.extend_from_slice(&v.to_le_bytes()));
    }
    for uv in [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]] {
        uv.iter()
            .for_each(|v| bin.extend_from_slice(&v.to_le_bytes()));
    }
    for i in [0u16, 1, 2, 0, 2, 3] {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let gltf = serde_json::json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"name": "Card", "nodes": [0]}],
        "nodes": [{"name": "Card", "mesh": 0}],
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
            "indices": 2,
            "material": 0
        }]}],
        "materials": [{
            "name": "Painted",
            "pbrMetallicRoughness": {
                "baseColorTexture": {"index": 0},
                "metallicFactor": 0.0,
                "roughnessFactor": 0.9
            }
        }],
        "textures": [{"source": 0}],
        "images": [{"uri": format!("data:image/png;base64,{}", base64(png.get_ref()))}],
        "buffers": [{
            "byteLength": bin.len(),
            "uri": format!("data:application/octet-stream;base64,{}", base64(&bin))
        }],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 48},
            {"buffer": 0, "byteOffset": 48, "byteLength": 32},
            {"buffer": 0, "byteOffset": 80, "byteLength": 12}
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
             "min": [-1.0, -1.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"},
            {"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"}
        ]
    });
    std::fs::write(
        dir.join("card.gltf"),
        serde_json::to_vec_pretty(&gltf).unwrap(),
    )
    .unwrap();
}

#[test]
fn gltf_with_embedded_texture_loads_spawns_and_renders() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gltf_card_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_card_gltf(&dir);
    let mut app = front_lit_app(Some(dir.clone()));

    engine_assets::load_now(&mut app.world, "card.gltf").expect("card.gltf loads");
    let scene = AssetServer::handle::<GltfScene>(&app.world, "card.gltf").expect("scene handle");
    let root = spawn_gltf_scene(&mut app.world, scene, Transform::default()).expect("spawn");
    let image =
        AssetServer::handle::<Image>(&app.world, "card.gltf#Image0").expect("embedded image");
    {
        let images = app.world.resource::<Assets<Image>>();
        let image = images.get(image).unwrap();
        assert_eq!((image.width, image.height), (8, 8));
        assert!(image.srgb, "base color textures are sRGB");
    }
    let mesh = AssetServer::handle::<Mesh>(&app.world, "card.gltf#Mesh0").expect("mesh alias");
    assert!(
        app.world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .unwrap()
            .vertices
            .iter()
            .all(|v| v.normal[2] > 0.99),
        "missing normals are computed facing the front"
    );

    let Some(img) = render(&mut app, "gltf_card") else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    // The scene root holds the node "Card", which renders the card.
    let mut nodes = app.world.query::<(&Name, &ChildOf, &MeshRenderer)>();
    let (name, parent, renderer) = nodes.single(&app.world).expect("one rendered node");
    assert_eq!((name.as_str(), parent.parent()), ("Card", root));
    assert!(matches!(&renderer.material, MaterialRef::Asset(key) if key == "card.gltf#Material0"));
    let [lr, _, lb] = mean_rgb(&img, project(-0.5, 0.0));
    let [rr, _, rb] = mean_rgb(&img, project(0.5, 0.0));
    assert!(
        lr > lb * 1.5 + 10.0,
        "left half should be red: r {lr} b {lb}"
    );
    assert!(
        rb > rr * 1.5 + 10.0,
        "right half should be blue: r {rr} b {rb}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Camera and sun straight above a grey ground; a 10×10 m card above the camera (out of
/// view) shadows all of the visible ground.
fn shadow_app(card: StandardMaterial) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = SIZE;
        w.height = SIZE;
    }
    app.add_plugins(RenderPlugin);
    // Fixed exposure: the two shadow images are compared by absolute brightness, and the
    // metered auto-exposure would brighten a fully shadowed view until it looks sunlit.
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 15.0 },
            ..Camera::perspective(FOV_Y_DEG)
        },
        Transform::from_xyz(0.0, 6.0, 0.0).looking_at(DVec3::ZERO, Vec3::NEG_Z),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.0, -1.0, 0.0), Vec3::NEG_Z),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3).with_roughness(0.9),
        ),
    ));
    let alpha_checker = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(Image::checker(
            64,
            4,
            [255, 255, 255, 255],
            [255, 255, 255, 0],
        ));
    // Plane UVs are in meters: scale them so the texture spans the card once (2.5 m cells).
    let card = card
        .with_base_color_texture(alpha_checker)
        .with_uv_scale(0.1, 0.1);
    app.world.spawn((
        Transform::from_xyz(0.0, 8.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [10.0, 10.0],
                subdivisions: 1,
            },
            card,
        ),
    ));
    app
}

#[test]
fn alpha_masked_casters_cut_holes_in_their_shadows() {
    let mut masked = shadow_app(StandardMaterial::color(1.0, 1.0, 1.0).with_alpha_mask(0.5));
    let Some(holes) = render(&mut masked, "masked_shadow") else {
        return;
    };
    let mut opaque = shadow_app(StandardMaterial::color(1.0, 1.0, 1.0));
    let Some(solid) = render(&mut opaque, "opaque_shadow") else {
        return;
    };

    let mut l: Vec<f32> = holes.pixels().map(luma).collect();
    l.sort_by(f32::total_cmp);
    let (lo, hi) = (l[l.len() / 20], l[l.len() * 19 / 20]);
    let threshold = (lo + hi) * 0.5;
    let lit = |img: &RgbaImage| {
        img.pixels().filter(|p| luma(p) > threshold).count() as f32 / (SIZE * SIZE) as f32
    };
    let (lit_masked, lit_opaque) = (lit(&holes), lit(&solid));
    eprintln!(
        "shadow lumas 5%={lo} 95%={hi}; lit fraction masked {lit_masked}, opaque {lit_opaque}"
    );
    assert!(
        hi > lo * 1.5 + 15.0,
        "no contrast between sun and shadow ({lo}..{hi})"
    );
    assert!(
        (0.15..=0.85).contains(&lit_masked),
        "the masked card's shadow should have holes (lit fraction {lit_masked})"
    );
    assert!(
        lit_opaque < 0.05,
        "an opaque card shadows everything (lit fraction {lit_opaque})"
    );
}

#[test]
fn mirrored_instances_keep_their_front_faces() {
    // A single-sided quad mirrored in X (negative determinant, common in glTF files): its
    // triangles wind clockwise on screen but it still faces the camera and must be drawn.
    let mut app = front_lit_app(None);
    let red = StandardMaterial::color(0.8, 0.05, 0.05).with_roughness(0.9);
    let mut mirrored = Transform::from_xyz(-0.6, 0.0, 0.0);
    mirrored.scale = Vec3::new(-1.0, 1.0, 1.0);
    app.world.spawn((
        mirrored,
        MeshRenderer::new(Shape::Quad { size: [1.0, 1.0] }, red),
    ));
    // The same quad unmirrored, for reference.
    app.world.spawn((
        Transform::from_xyz(0.6, 0.0, 0.0),
        MeshRenderer::new(Shape::Quad { size: [1.0, 1.0] }, red),
    ));
    let Some(img) = render(&mut app, "mirrored_quad") else {
        return;
    };
    let [mr, mg, mb] = mean_rgb(&img, project(-0.6, 0.0));
    let [r, g, b] = mean_rgb(&img, project(0.6, 0.0));
    assert!(
        r > (g + b) * 0.8 + 20.0,
        "reference quad is not red: {r} {g} {b}"
    );
    assert!(
        mr > (mg + mb) * 0.8 + 20.0,
        "the mirrored quad was culled: {mr} {mg} {mb} (reference {r} {g} {b})"
    );
    assert!(
        (mr - r).abs() < 0.15 * r + 8.0,
        "a mirrored quad facing the camera should shade like the original: {mr} vs {r}"
    );
}

#[test]
fn handles_requested_under_aliases_share_one_gpu_mesh() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gltf_alias_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_card_gltf(&dir);
    let mut app = front_lit_app(Some(dir.clone()));
    // Two keys for the card's only mesh, both requested before the file has loaded: each gets
    // its own reserved handle, which the loader then points at the same asset.
    for (x, mesh) in [(-0.6, "card.gltf"), (0.6, "card.gltf#Mesh0")] {
        let mut t = Transform::from_xyz(x, 0.0, 0.0);
        t.scale = Vec3::splat(0.25);
        app.world.spawn((
            t,
            MeshRenderer::new(
                MeshRef::Asset(mesh.into()),
                MaterialRef::Asset("card.gltf#Material0".into()),
            ),
        ));
    }
    app.update();
    let start = std::time::Instant::now();
    while !app.world.resource::<AssetServer>().is_loaded("card.gltf") {
        assert!(start.elapsed().as_secs() < 20, "card.gltf never loaded");
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.update();
    }
    let mut q = app
        .world
        .query::<&engine_render::components::ResolvedMesh>();
    let handles: Vec<_> = q.iter(&app.world).map(|r| r.mesh).collect();
    assert_ne!(
        handles[0], handles[1],
        "the test needs two distinct handles"
    );
    let Some(img) = render(&mut app, "gltf_alias") else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    // Both cards are drawn ...
    for x in [-0.6, 0.6] {
        let [r, _, b] = mean_rgb(&img, project(x - 0.1, 0.0));
        assert!(r > b * 1.5 + 10.0, "card at x={x} is missing: r {r} b {b}");
    }
    // ... from one uploaded mesh, in one instanced batch.
    let stats =
        engine_core::registry::call_api(&mut app.world, "render.stats", serde_json::json!({}))
            .unwrap();
    let meshes = stats["renderer"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "meshes")
        .unwrap()
        .clone();
    assert_eq!(meshes["state"]["gpu_meshes"], 1, "{meshes}");
    assert_eq!(meshes["state"]["opaque_batches"], 1, "{meshes}");
    let _ = std::fs::remove_dir_all(&dir);
}
