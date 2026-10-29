//! Renders skinned meshes offscreen and checks the pictures: the GPU skinning pass draws a
//! character with its paint and the drawn pose follows the animation (a flex raises the
//! arms); textured submeshes draw with their own surfaces (a checker, an alpha cutout on a
//! double-sided card), from the renderer's shared images even after their CPU pixels are freed;
//! fur shells grow strands over a furred skin. Needs a GPU or a software Vulkan driver (lavapipe); passes with a note
//! when neither is available (CI sets ENGINE_REQUIRE_GPU=1, which turns that into a failure).

use std::sync::Arc;

use engine_anim::build::humanoid::Outfit;
use engine_anim::demo::spawn_humanoid;
use engine_anim::skeleton::BoneDef;
use engine_anim::skin::{SkinMaterial, SkinVertex, Submesh};
use engine_anim::{AnimationPlugin, Animator, Skeleton, SkinnedMesh, SkinnedMeshRenderer};
use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::image_cache::RELEASE_AFTER_FRAMES;
use engine_render::prelude::*;
use engine_render::{AlphaMode, Image, RenderDisabled, Renderer, capture_now};
use engine_world::WorldPlugin;
use engine_world::animation::AnimationTrigger;

const W: u32 = 320;
const H: u32 = 240;

fn capture(app: &mut App, name: &str) -> image::RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

/// Pixels of the athlete's red tank top inside a box (fractions of the image).
fn red_pixels(img: &image::RgbaImage, x0: f32, x1: f32, y0: f32, y1: f32) -> usize {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut n = 0;
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = img.get_pixel(x, y);
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            if r > 40.0 && r > g * 1.6 && r > b * 1.6 {
                n += 1;
            }
        }
    }
    n
}

/// Skin-colored pixels (the bare arms) in a box.
fn skin_pixels(img: &image::RgbaImage, x0: f32, x1: f32, y0: f32, y1: f32) -> usize {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut n = 0;
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = img.get_pixel(x, y);
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            if r > 60.0 && r > g * 1.1 && g > b * 1.05 && r < g * 1.8 {
                n += 1;
            }
        }
    }
    n
}

/// A headless W×H app with the renderer and animation, stepping 1/60 s per update.
fn headless_app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, RenderPlugin, AnimationPlugin));
    app
}

#[test]
fn skinned_character_renders_and_follows_the_pose() {
    let mut app = headless_app();
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 80_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.3, -0.7, -0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b7b5a).with_roughness(0.95),
        ),
    ));
    w.spawn((
        Camera::perspective(40.0),
        Transform::from_xyz(0.0, 1.15, 3.6).looking_at(DVec3::new(0.0, 1.05, 0.0), Vec3::Y),
    ));
    // Facing the camera (+Z).
    let athlete = spawn_humanoid(
        w,
        Outfit::Athlete,
        Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
    );
    app.update_n(10);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let idle = capture(&mut app, "anim_idle.png");
    let stats = app.world.resource::<engine_render::RenderStats>().clone();
    assert!(
        stats.features.iter().any(|f| f.contains("skin")),
        "skinning feature missing: {:?}",
        stats.features
    );
    // The tank top fills the middle of the picture.
    let top = red_pixels(&idle, 0.4, 0.6, 0.25, 0.55);
    assert!(top > 400, "tank top not drawn ({top} red pixels)");

    // Double-biceps: the fists come up beside the head.
    app.world.write_message(AnimationTrigger {
        entity: athlete,
        name: "flex".into(),
    });
    app.update_n(90);
    let flex = capture(&mut app, "anim_flex.png");
    let above_shoulders = |img: &image::RgbaImage| {
        skin_pixels(img, 0.2, 0.42, 0.05, 0.3) + skin_pixels(img, 0.58, 0.8, 0.05, 0.3)
    };
    let (before, after) = (above_shoulders(&idle), above_shoulders(&flex));
    assert!(
        after > before + 150,
        "arms did not come up: {before} skin pixels beside the head at idle, {after} flexing"
    );
}

/// Fraction of a box's pixels (fractions of the image) that differ by more than 40 in any
/// channel between two pictures.
fn changed(
    a: &image::RgbaImage,
    b: &image::RgbaImage,
    (x0, x1, y0, y1): (f32, f32, f32, f32),
) -> f32 {
    let (w, h) = (a.width() as f32, a.height() as f32);
    let (mut n, mut total) = (0, 0);
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
            total += 1;
            if (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 40) {
                n += 1;
            }
        }
    }
    n as f32 / total.max(1) as f32
}

#[test]
fn characters_arriving_later_join_without_disturbing_the_drawn_ones() {
    // New meshes are appended to the skin's static buffers (which grow, copying what they
    // hold on the GPU), and a mesh edited over and over leaves space that is reclaimed by
    // packing the live meshes again: through all of it every character keeps drawing.
    let mut app = headless_app();
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 80_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.3, -0.7, -0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b7b5a).with_roughness(0.95),
        ),
    ));
    w.spawn((
        Camera::perspective(40.0),
        Transform::from_xyz(0.0, 1.15, 3.6).looking_at(DVec3::new(0.0, 1.05, 0.0), Vec3::Y),
    ));
    let facing = Quat::from_rotation_y(std::f32::consts::PI);
    let athlete = spawn_humanoid(
        w,
        Outfit::Athlete,
        Transform::from_xyz(-1.0, 0.0, 0.0).with_rotation(facing),
    );
    app.update_n(10);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let left = (0.08, 0.34, 0.25, 0.55);
    let red = |img: &image::RgbaImage| red_pixels(img, left.0, left.1, left.2, left.3);
    let alone = capture(&mut app, "anim_arrive_alone.png");
    let first = red(&alone);
    assert!(first > 150, "the athlete isn't drawn ({first} red pixels)");

    // Two more people, each with a mesh of its own, a few frames apart.
    spawn_humanoid(
        &mut app.world,
        Outfit::Shirt,
        Transform::from_xyz(0.0, 0.0, 0.0).with_rotation(facing),
    );
    app.update_n(3);
    spawn_humanoid(
        &mut app.world,
        Outfit::Lifter,
        Transform::from_xyz(1.0, 0.0, 0.0).with_rotation(facing),
    );
    app.update_n(10);
    let all = capture(&mut app, "anim_arrive_all.png");
    let still = red(&all);
    assert!(
        still * 10 >= first * 8,
        "the athlete faded as the others arrived: {first} red pixels alone, {still} after"
    );
    let (middle, right) = ((0.38, 0.62, 0.2, 0.9), (0.66, 0.92, 0.2, 0.9));
    for (name, area) in [("shirt", middle), ("lifter", right)] {
        let drawn = changed(&alone, &all, area);
        assert!(
            drawn > 0.1,
            "the {name} isn't drawn ({drawn:.2} of its box changed)"
        );
    }

    // Edit the athlete's mesh again and again: each new version is appended and the old one
    // left behind, until packing reclaims the space.
    let handle = app
        .world
        .get::<SkinnedMeshRenderer>(athlete)
        .and_then(|r| r.mesh)
        .expect("athlete mesh");
    let bytes = {
        let meshes = app.world.resource::<Assets<SkinnedMesh>>();
        let m = meshes.get(handle).expect("mesh");
        m.vertices.len() * 200 + m.indices.len() * 4
    };
    let edits = (40 << 20) / bytes.max(1) + 2;
    for _ in 0..edits {
        let _ = app
            .world
            .resource_mut::<Assets<SkinnedMesh>>()
            .get_mut(handle);
        app.update();
    }
    app.update_n(3);
    let packed = capture(&mut app, "anim_arrive_packed.png");
    let after = red(&packed);
    assert!(
        after * 10 >= first * 8,
        "the athlete faded after {edits} edits: {first} red pixels before, {after} after"
    );
    for (name, area) in [("shirt", middle), ("lifter", right)] {
        let moved = changed(&all, &packed, area);
        assert!(
            moved < 0.15,
            "the {name} changed after the athlete's edits ({moved:.2} of its box)"
        );
    }
}

/// Counts pixels in a box (fractions of the image) for which `pred(r, g, b)` holds; also
/// returns the box's pixel count.
fn count(
    img: &image::RgbaImage,
    (x0, x1, y0, y1): (f32, f32, f32, f32),
    pred: impl Fn(f32, f32, f32) -> bool,
) -> (usize, usize) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (mut n, mut total) = (0, 0);
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = img.get_pixel(x, y);
            total += 1;
            if pred(p[0] as f32, p[1] as f32, p[2] as f32) {
                n += 1;
            }
        }
    }
    (n, total)
}

/// Two 1 m panels side by side, skinned to a two-bone skeleton (bottom row on `root`, top row
/// on `top`). Submesh 0: the left panel, facing the camera (+Z). Submesh 1: the right panel,
/// facing away (−Z), so it only shows if its surface is double-sided.
fn panels() -> (Skeleton, SkinnedMesh) {
    let skeleton = Skeleton::new(
        "panels",
        vec![
            BoneDef::new("root", None, Vec3::ZERO),
            BoneDef::new("top", Some("root"), Vec3::Y),
        ],
    )
    .unwrap();
    let mut mesh = SkinnedMesh {
        joints: vec!["root".into(), "top".into()],
        inverse_bind: skeleton.inverse_bind.clone(),
        ..Default::default()
    };
    for (x0, normal) in [(-1.1, Vec3::Z), (0.1, Vec3::NEG_Z)] {
        let base = mesh.vertices.len() as u32;
        for (u, v) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            // v = 0 at the top (glTF convention); the top row follows the `top` bone.
            let top = v == 0.0;
            mesh.vertices.push(SkinVertex {
                position: Vec3::new(x0 + u, 1.0 - v, 0.0),
                normal,
                uv: Vec2::new(u, v),
                joints: [u16::from(top), 0, 0, 0],
                ..Default::default()
            });
        }
        let first = mesh.indices.len() as u32;
        if normal.z > 0.0 {
            mesh.indices
                .extend([0, 1, 2, 0, 2, 3].map(|i: u32| base + i));
        } else {
            mesh.indices
                .extend([0, 2, 1, 0, 3, 2].map(|i: u32| base + i));
        }
        mesh.submeshes.push(Submesh {
            first_index: first,
            index_count: 6,
            surface: mesh.submeshes.len(),
        });
    }
    (skeleton, mesh)
}

#[test]
fn textured_submeshes_draw_their_own_surfaces() {
    let mut app = headless_app();
    let w = &mut app.world;
    // Sun from behind the camera, 45° up: the panels' shadows fall on the backdrop below them,
    // clear of what the camera sees through the cutout.
    w.spawn((
        DirectionalLight {
            illuminance: 80_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.0, -1.0, -1.0), Vec3::Y),
    ));
    // Grey backdrop behind the panels, facing the camera.
    w.spawn((
        Transform::from_xyz(0.0, 0.5, -1.5)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        MeshRenderer::new(
            Shape::Plane {
                size: [12.0, 12.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.4, 0.4, 0.4).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Camera::perspective(40.0),
        Transform::from_xyz(0.0, 0.5, 3.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    let (checker, cutout) = {
        let mut images = w.get_resource_or_init::<Assets<Image>>();
        let checker = images.add(Image::checker(64, 4, [255, 0, 0, 255], [0, 0, 255, 255]));
        // Left half opaque green, right half fully transparent.
        let cutout = images.add(Image::from_fn(64, 64, true, |x, _| {
            [0, 255, 0, if x < 32 { 255 } else { 0 }]
        }));
        (checker, cutout)
    };
    let (skeleton, mesh) = panels();
    let mesh = w.get_resource_or_init::<Assets<SkinnedMesh>>().add(mesh);
    w.spawn((
        Transform::default(),
        GlobalTransform::default(),
        Animator::procedural(Arc::new(skeleton)),
        SkinnedMeshRenderer {
            mesh: Some(mesh),
            surfaces: vec![
                StandardMaterial {
                    base_color: [1.0; 4],
                    perceptual_roughness: 0.8,
                    ..Default::default()
                }
                .with_base_color_texture(checker),
                StandardMaterial {
                    base_color: [1.0; 4],
                    perceptual_roughness: 0.8,
                    alpha_mode: AlphaMode::Mask(0.5),
                    double_sided: true,
                    ..Default::default()
                }
                .with_base_color_texture(cutout),
            ],
            ..Default::default()
        },
    ));
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let img = capture(&mut app, "anim_textured_submeshes.png");
    let stats = app.world.resource::<engine_render::RenderStats>().clone();

    // Left panel (screen x 0.12..0.47, y 0.27..0.73): red and blue checker squares.
    let left = (0.15, 0.44, 0.3, 0.7);
    let (red, total) = count(&img, left, |r, g, b| r > 40.0 && r > 2.0 * g && r > 2.0 * b);
    let (blue, _) = count(&img, left, |r, g, b| b > 40.0 && b > 2.0 * r && b > 1.5 * g);
    assert!(
        red > total / 5 && blue > total / 5,
        "checker surface not drawn: {red} red and {blue} blue of {total} pixels ({stats:?})"
    );
    // Right panel, seen from behind: its opaque half (screen x 0.53..0.71) is green ...
    let (green, total) = count(&img, (0.56, 0.68, 0.3, 0.7), |r, g, b| {
        g > 40.0 && g > 2.0 * r && g > 1.5 * b
    });
    assert!(
        green > total * 2 / 3,
        "double-sided cutout surface not drawn: {green} green of {total} pixels"
    );
    // ... and its transparent half (x 0.71..0.88) shows the grey backdrop.
    let (green, total) = count(&img, (0.74, 0.86, 0.3, 0.7), |r, g, b| {
        g > 40.0 && g > 1.5 * r && g > 1.2 * b
    });
    let (grey, _) = count(&img, (0.74, 0.86, 0.3, 0.7), |r, g, b| {
        let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
        hi > 30.0 && hi - lo < 0.35 * hi
    });
    assert!(
        green < total / 50 && grey > total * 9 / 10,
        "cut-out half not transparent: {green} green and {grey} grey of {total} pixels"
    );
    // Shadows on the backdrop (y 0.85..0.95): the card's opaque half casts one (x 0.53..0.62),
    // its cut-out half does not (x 0.67..0.8), so the masked shadow pass discards too.
    let dark = |r: f32, g: f32, b: f32| r.max(g).max(b) < 110.0;
    let (shadow, total) = count(&img, (0.53, 0.62, 0.85, 0.95), dark);
    assert!(
        shadow > total * 9 / 10,
        "the card's opaque half casts no shadow: {shadow} dark of {total} pixels"
    );
    let (shadow, total) = count(&img, (0.67, 0.8, 0.85, 0.95), dark);
    assert!(
        shadow < total / 20,
        "the cut-out half casts a shadow: {shadow} dark of {total} pixels"
    );
}

/// With `release_image_pixels` on, the renderer frees an image's CPU pixels once it is on the
/// GPU. A skin keeps drawing that image from the renderer's copy, the same upload a standard
/// mesh with the image uses, and nothing is uploaded again.
#[test]
fn textured_skins_draw_from_the_shared_images_after_their_pixels_are_freed() {
    let mut app = headless_app();
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.release_image_pixels = true;
        // Nothing here needs the expensive effects; frames are many.
        s.anti_aliasing = AntiAliasing::None;
        s.volumetric_fog = false;
        s.sky_cloud_layer = false;
        s.screen_space_reflections = false;
        s.screen_space_gi = false;
        s.ambient_occlusion = false;
    }
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 80_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.0, -1.0, -1.0), Vec3::Y),
    ));
    w.spawn((
        Camera::perspective(40.0),
        Transform::from_xyz(0.0, 0.5, 3.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    let checker = w
        .get_resource_or_init::<Assets<Image>>()
        .add(Image::checker(64, 4, [255, 0, 0, 255], [0, 0, 255, 255]));
    let surface = StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 0.8,
        double_sided: true,
        ..Default::default()
    }
    .with_base_color_texture(checker);
    // A standard quad under the panels with the same image.
    w.spawn((
        Transform::from_xyz(0.0, -0.3, 0.0),
        MeshRenderer::new(Shape::Quad { size: [0.4, 0.4] }, surface),
    ));
    let (skeleton, mesh) = panels();
    let mesh = w.get_resource_or_init::<Assets<SkinnedMesh>>().add(mesh);
    w.spawn((
        Transform::default(),
        GlobalTransform::default(),
        Animator::procedural(Arc::new(skeleton)),
        SkinnedMeshRenderer {
            mesh: Some(mesh),
            surfaces: vec![surface, surface],
            ..Default::default()
        },
    ));
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let uploads = app
        .world
        .resource::<Renderer>()
        .shared
        .images
        .stats()
        .uploads;
    assert!(uploads >= 1, "the checker went through the shared images");
    app.update_n(RELEASE_AFTER_FRAMES as usize + 10);
    assert!(
        app.world
            .resource::<Assets<Image>>()
            .get(checker)
            .is_some_and(|i| i.is_released()),
        "the checker's pixels were freed"
    );
    let stats = app.world.resource::<Renderer>().shared.images.stats();
    assert_eq!(stats.uploads, uploads, "nothing uploaded again ({stats:?})");

    let img = capture(&mut app, "anim_released_pixels.png");
    // Both panels still show red and blue squares (a skin whose image had no pixels would
    // draw it white).
    for (panel, area) in [
        ("left", (0.15, 0.44, 0.3, 0.7)),
        ("right", (0.56, 0.85, 0.3, 0.7)),
    ] {
        let (red, total) = count(&img, area, |r, g, b| r > 40.0 && r > 2.0 * g && r > 2.0 * b);
        let (blue, _) = count(&img, area, |r, g, b| b > 40.0 && b > 2.0 * r && b > 1.5 * g);
        assert!(
            red > total / 5 && blue > total / 5,
            "{panel} panel lost its checker after release: {red} red and {blue} blue of {total} \
             pixels"
        );
    }
}

/// A 1 m square lying flat at `y`, facing up, red, with fur everywhere.
fn furred_floor(y: f32) -> (Skeleton, SkinnedMesh) {
    let skeleton = Skeleton::new("floor", vec![BoneDef::new("root", None, Vec3::ZERO)]).unwrap();
    let mut mesh = SkinnedMesh {
        joints: vec!["root".into()],
        inverse_bind: skeleton.inverse_bind.clone(),
        ..Default::default()
    };
    // A grid, so the shells' strands have vertices to bend with.
    let n = 8;
    for j in 0..=n {
        for i in 0..=n {
            let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
            let p = Vec3::new(u - 0.5, y, v - 0.5);
            mesh.vertices.push(SkinVertex {
                position: p,
                normal: Vec3::Y,
                color: Vec4::new(0.8, 0.05, 0.04, 1.0),
                pattern: p,
                mask: Vec4::new(0.0, 0.6, 0.0, 1.0),
                fur: 1.0,
                ..Default::default()
            });
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            let (b, c, d) = (a + 1, a + n + 1, a + n + 2);
            mesh.indices.extend([a, c, b, b, c, d]);
        }
    }
    (skeleton, mesh)
}

#[test]
fn fur_shells_grow_strands_over_the_skin() {
    let mut app = headless_app();
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 80_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.2, -1.0, -0.4), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.5, -1.5)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        MeshRenderer::new(
            Shape::Plane {
                size: [12.0, 12.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.4, 0.4, 0.4).with_roughness(0.9),
        ),
    ));
    // Nearly edge-on: the bare floor is a thin line across the middle of the picture, and fur
    // shows as a band of strands above it.
    w.spawn((
        Camera::perspective(40.0),
        Transform::from_xyz(0.0, 0.62, 3.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    let (skeleton, mesh) = furred_floor(0.5);
    let mesh = w.get_resource_or_init::<Assets<SkinnedMesh>>().add(mesh);
    let bare = SkinMaterial {
        wrinkles: 0.0,
        veins: 0.0,
        ..Default::default()
    };
    let floor = w
        .spawn((
            Transform::default(),
            GlobalTransform::default(),
            Animator::procedural(Arc::new(skeleton)),
            SkinnedMeshRenderer::new(mesh, bare),
        ))
        .id();
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    // Just above the bare floor (rows 0.492..0.51): the thick lower half of 15 cm strands
    // (their thin tips are mostly narrower than a pixel at this distance).
    let band = (0.3, 0.7, 0.475, 0.492);
    let red = |r: f32, g: f32, b: f32| r > 25.0 && r > 2.0 * g && r > 2.0 * b;
    let img = capture(&mut app, "anim_fur_bare.png");
    let (before, total) = count(&img, band, red);
    assert!(
        before < total / 50,
        "the bare floor fills the band: {before} of {total}"
    );

    app.world
        .get_mut::<SkinnedMeshRenderer>(floor)
        .unwrap()
        .material = SkinMaterial {
        fur_length: 0.15,
        fur_density: 60.0,
        ..bare
    };
    app.update_n(3);
    let img = capture(&mut app, "anim_fur_shells.png");
    let (after, _) = count(&img, band, red);
    let stats = app.world.resource::<engine_render::RenderStats>().clone();
    assert!(
        after > total / 6,
        "no fur above the floor: {after} red of {total} pixels ({stats:?})"
    );
    // Strands, not a slab: the backdrop shows between them.
    assert!(
        after < total * 19 / 20,
        "the fur is a solid slab: {after} red of {total} pixels"
    );
}
