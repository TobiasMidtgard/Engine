//! Impostors: a valley of leaf-card trees (separate quads the simplifier can't merge, so they
//! get no levels of detail) is drawn with far fewer triangles once the far trees are baked into
//! impostors, in the view and in the shadow maps, and looks the same. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, RenderStats, Renderer, Vertex, capture_now};

const W: u32 = 320;
const H: u32 = 180;

/// A tree of crossed trunk cards and a crown of 300 leaf cards, 2 triangles each.
fn card_tree(seed: u32) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut card = |center: Vec3, right: Vec3, up: Vec3, color: [f32; 4]| {
        let normal = right.cross(up).normalize();
        let base = vertices.len() as u32;
        for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let mut vertex = Vertex::new(
                center + right * u + up * v,
                normal,
                Vec2::new(u * 0.5 + 0.5, v * 0.5 + 0.5),
            );
            vertex.color = color;
            vertices.push(vertex);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    let bark = [0.35, 0.25, 0.18, 1.0];
    card(
        Vec3::new(0.0, 2.5, 0.0),
        Vec3::X * 0.25,
        Vec3::Y * 2.5,
        bark,
    );
    card(
        Vec3::new(0.0, 2.5, 0.0),
        Vec3::Z * 0.25,
        Vec3::Y * 2.5,
        bark,
    );
    let mut state = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let mut random = move || {
        state = state.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        ((state >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    };
    for _ in 0..300 {
        let p = Vec3::new(random(), random(), random());
        let center = Vec3::new(p.x * 2.2, 6.0 + p.y * 2.5, p.z * 2.2);
        let axis = Vec3::new(random(), random(), random()).normalize_or(Vec3::Y);
        let right = axis.any_orthonormal_vector();
        let up = axis.cross(right);
        let shade = 0.8 + 0.2 * random();
        card(center, right * 0.35, up * 0.35, [shade, shade, shade, 1.0]);
    }
    Mesh::new(vertices, indices)
}

/// The valley: a camera on a hill, the sun, the ground and 360 trees of three card meshes with
/// the material `leaves`.
fn valley(w: &mut World, leaves: MaterialRef) {
    w.spawn((
        Camera::perspective(60.0),
        // From a hill: rows of trees from near to far.
        Transform::from_xyz(0.0, 30.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -160.0), Vec3::Y),
    ));
    let e = 40f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.6 * e.cos(), -e.sin(), -0.8 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.32, 0.22),
        ),
    ));
    let trees: Vec<_> = (0..3)
        .map(|seed| w.resource_mut::<Assets<Mesh>>().add(card_tree(seed)))
        .collect();
    // Rows of trees from 15 m to about 700 m away, on both sides of the view.
    for i in 0..360u32 {
        let row = (i / 12) as f64;
        let col = (i % 12) as f64 - 5.5;
        let z = -15.0 - row * row * 0.7 - row * 2.0;
        let x = col * (6.0 + row * 1.2);
        w.spawn((
            Transform::from_xyz(x, 0.0, z).with_rotation(Quat::from_rotation_y(i as f32 * 2.4)),
            MeshRenderer::new(trees[(i % 3) as usize], leaves.clone()),
        ));
    }
}

/// The meshes feature's state in `Renderer::describe`.
fn describe(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .map(|f| f["state"].clone())
        .unwrap_or_default()
}

/// Render the valley with impostors off, on, on with a BC7 atlas (`Bc7`) or only for trees
/// under a fifth as big on screen (`Fade`, so that rows of trees cross-fade): the triangles
/// drawn, the impostors in view, those of them cross-fading from their meshes and the image's
/// pixels.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Meshes,
    Impostors,
    Bc7,
    Fade,
}

fn render(mode: Mode, shot: &str) -> Option<(u64, u64, u64, Vec<u8>)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        // Temporal anti-aliasing blends the impostors' neighboring views, as in a game.
        s.film_grain = 0.0;
        match mode {
            Mode::Meshes => s.impostor_pixels = 0.0,
            Mode::Impostors => s.texture_compression = TextureCompression::Off,
            // Compressed atlas layers, also under a software GPU.
            Mode::Bc7 => s.texture_compression_on_cpu = true,
            Mode::Fade => {
                s.texture_compression = TextureCompression::Off;
                s.impostor_pixels = 20.0;
            }
        }
    }
    let leaves = StandardMaterial {
        double_sided: true,
        ..StandardMaterial::color(0.22, 0.4, 0.15)
    };
    valley(&mut app.world, leaves.into());
    let mut last = (u64::MAX, u64::MAX);
    let start = std::time::Instant::now();
    let mut frames = 0u64;
    // The compressed atlas waits for the GPU encoders, which take a while to build under a
    // software GPU: until the three trees are baked.
    if mode == Mode::Bc7 {
        while start.elapsed().as_secs() < 120 {
            app.update_n(5);
            frames += 5;
            if app.world.get_resource::<RenderDisabled>().is_some()
                || describe(&app)["impostors"].as_u64() >= Some(3)
            {
                break;
            }
        }
    }
    // Until the levels are tried and the impostors baked (the counts stop changing).
    let mut steady = 0;
    while steady < 5 && start.elapsed().as_secs() < 120 {
        app.update();
        frames += 1;
        if app.world.get_resource::<RenderDisabled>().is_some() {
            break;
        }
        let stats = app.world.get_resource::<RenderStats>();
        let now = stats.map_or((0, 0), |s| (s.triangles, s.instances as u64));
        steady = if now == last { steady + 1 } else { 0 };
        last = now;
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    // Every run captures at the same point of the TAA jitter cycle and of the impostors' 64-frame
    // dither noise, so near leaves land on the same sub-pixels and far trees pick the same tiles
    // however long the bakes took.
    const PERIOD: u64 = 64;
    const _: () = assert!(PERIOD.is_multiple_of(engine_render::effects::JITTER_SAMPLES));
    while !frames.is_multiple_of(PERIOD) {
        app.update();
        frames += 1;
    }
    let dir = std::env::temp_dir().join("engine-impostors");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let image = image::open(&path).expect("read the shot").to_rgb8();
    let state = describe(&app);
    let in_view = state["impostors_in_view"].as_u64().unwrap_or(0);
    let fading = state["impostors_fading"].as_u64().unwrap_or(0);
    assert_eq!(
        state["impostor_atlas_bc7"].as_bool(),
        Some(mode == Mode::Bc7),
        "atlas format: {state}"
    );
    let tris = app.world.resource::<RenderStats>().triangles;
    Some((tris, in_view, fading, image.into_raw()))
}

#[test]
fn far_card_trees_become_impostors_and_look_the_same() {
    let Some((full, _, _, before)) = render(Mode::Meshes, "trees_meshes.png") else {
        return;
    };
    let Some((fewer, impostors, fading, after)) = render(Mode::Impostors, "trees_impostors.png")
    else {
        return;
    };
    eprintln!(
        "triangles: {full} as meshes, {fewer} with {impostors} impostors in view \
         ({fading} cross-fading from their meshes)"
    );
    assert!(impostors > 100, "{impostors} impostors in view");
    assert!(fewer * 2 < full, "{fewer} of {full} triangles");
    let diff = difference(&before, &after);
    eprintln!("mean difference over 4x4 blocks {diff:.2} of 255");
    assert!(
        diff < 3.5,
        "the images differ by {diff:.2} of 255 on average"
    );
}

#[test]
fn meshes_cross_fade_into_their_impostors() {
    let Some((_, _, _, meshes)) = render(Mode::Meshes, "trees_meshes_fade_test.png") else {
        return;
    };
    let Some((_, impostors, fading, faded)) = render(Mode::Fade, "trees_cross_fade.png") else {
        return;
    };
    eprintln!("{impostors} impostors in view, {fading} of them cross-fading from their meshes");
    // The rows where meshes hand over to impostors draw both, dithered, instead of popping.
    assert!(fading > 5, "{fading} trees cross-fading");
    assert!(impostors > fading, "{impostors} impostors in view");
    // Each tree in the band is drawn once over: mesh and impostor split its pixels.
    let diff = difference(&meshes, &faded);
    eprintln!("cross-fade: mean difference over 4x4 blocks {diff:.2} of 255");
    assert!(
        diff < 3.5,
        "the images differ by {diff:.2} of 255 on average"
    );
}

#[test]
fn a_bc7_atlas_looks_like_the_rgba8_one() {
    let Some((_, _, _, meshes)) = render(Mode::Meshes, "trees_meshes_bc7_test.png") else {
        return;
    };
    let Some((_, impostors, _, bc7)) = render(Mode::Bc7, "trees_impostors_bc7.png") else {
        return;
    };
    assert!(impostors > 100, "{impostors} impostors in view");
    let diff = difference(&meshes, &bc7);
    eprintln!("BC7 atlas: mean difference over 4x4 blocks {diff:.2} of 255");
    assert!(
        diff < 3.5,
        "the images differ by {diff:.2} of 255 on average"
    );
}

/// Compared over 4x4 pixel blocks: single leaves land a pixel apart (the impostor's view is the
/// nearest baked one), the trees' shapes, colors, light and shadows must match.
fn difference(before: &[u8], after: &[u8]) -> f32 {
    let blocks = |image: &[u8]| {
        let mut out = vec![0f32; (W / 4 * (H / 4) * 3) as usize];
        for y in 0..H {
            for x in 0..W {
                for c in 0..3 {
                    let i = ((y / 4 * (W / 4) + x / 4) * 3 + c) as usize;
                    out[i] += image[((y * W + x) * 3 + c) as usize] as f32 / 16.0;
                }
            }
        }
        out
    };
    let (a, b) = (blocks(before), blocks(after));
    a.iter().zip(&b).map(|(a, b)| (a - b).abs()).sum::<f32>() / a.len() as f32
}

/// Run until the impostors baked reach `baked` and the counts stop changing; false without a
/// GPU.
fn settle(app: &mut App, baked: u64) -> bool {
    let start = std::time::Instant::now();
    let (mut last, mut steady) = ((u64::MAX, u64::MAX), 0);
    while steady < 5 && start.elapsed().as_secs() < 180 {
        app.update();
        if app.world.get_resource::<RenderDisabled>().is_some() {
            return false;
        }
        let stats = app.world.resource::<RenderStats>();
        let now = (stats.triangles, stats.instances as u64);
        let ready = describe(app)["impostors"].as_u64() >= Some(baked);
        steady = if ready && now == last { steady + 1 } else { 0 };
        last = now;
    }
    true
}

/// How much redder than green the far trees are: the mean of red minus green (0..255) over the
/// band of the image the trees past about 150 m fill.
fn far_redness(app: &mut App, shot: &str) -> f32 {
    let path = std::env::temp_dir().join("engine-impostors").join(shot);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let image = image::open(&path).expect("read the shot").to_rgb8();
    let rows = (H * 35 / 100)..(H * 45 / 100);
    let count = (rows.len() as u32 * W) as f32;
    rows.flat_map(|y| (0..W).map(move |x| (x, y)))
        .map(|(x, y)| {
            let p = image.get_pixel(x, y);
            p[0] as f32 - p[1] as f32
        })
        .sum::<f32>()
        / count
}

/// Regression: an impostor baked while its material still showed something else (images still
/// loading, or a color since changed) is baked again, so far trees don't keep the old look.
#[test]
fn far_trees_follow_a_material_that_changes() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.film_grain = 0.0;
        s.texture_compression = TextureCompression::Off;
    }
    let leaves = app
        .world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            double_sided: true,
            ..StandardMaterial::color(0.12, 0.45, 0.08)
        });
    valley(&mut app.world, leaves.into());
    if !settle(&mut app, 3) {
        eprintln!("skipping: no GPU");
        return;
    }
    let green = far_redness(&mut app, "trees_green.png");
    let baked = describe(&app)["impostors"].as_u64().unwrap_or(0);

    app.world
        .resource_mut::<Assets<StandardMaterial>>()
        .get_mut(leaves)
        .expect("the leaves")
        .base_color = [0.6, 0.08, 0.06, 1.0];
    assert!(settle(&mut app, baked + 3), "GPU lost");
    let rebaked = describe(&app)["impostors"].as_u64().unwrap_or(0);
    let red = far_redness(&mut app, "trees_red.png");
    eprintln!(
        "far trees: red - green {green:.1} before, {red:.1} after; {baked} then {rebaked} impostors"
    );
    assert!(
        rebaked >= baked + 3,
        "{rebaked} impostors after the change, {baked} before"
    );
    assert!(
        red > green + 10.0,
        "far trees: red - green {green:.1} before, {red:.1} after"
    );
}
