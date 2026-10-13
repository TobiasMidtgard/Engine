//! Far shadows: a mountain-sized occluder kilometers away shades the ground 1.5–2.5 km from the
//! camera, far beyond the shadow cascades (`shadow_distance`), through the far shadow map, both
//! when it is drawn instanced and as virtual geometry clusters. Needs a GPU or lavapipe; passes
//! with a note when neither is available.

use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 192;
const H: u32 = 180;

/// A camera 300 m up looking 2 km ahead over a flat plain (20° field of view), with a low sun
/// (10° up) shining along +x. `occluder` is spawned on the plain.
fn plain_app(far_shadow_distance: f32, occluder: impl FnOnce(&mut World)) -> App {
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
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.anti_aliasing = AntiAliasing::None;
        s.volumetric_fog = false;
        s.far_shadow_distance = far_shadow_distance;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.5 },
            ..Camera::perspective(20.0)
        },
        Transform::from_xyz(0.0, 300.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -2000.0), Vec3::Y),
    ));
    let e = 10f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(e.cos(), -e.sin(), 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.0, -2000.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [12_000.0, 12_000.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    occluder(w);
    app
}

/// Mean linear luminance of the middle columns over rows `y0..y1`.
fn band(img: &RgbaImage, y0: u32, y1: u32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (mut sum, mut n) = (0.0, 0.0);
    for y in y0..y1 {
        for x in (W / 2 - 16)..(W / 2 + 16) {
            let p = img.get_pixel(x, y);
            sum += 0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2]);
            n += 1.0;
        }
    }
    sum / n
}

fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

/// Rows of the image looking at ground `d` meters ahead of the camera.
fn row_at(d: f64) -> u32 {
    let below = (300.0 / d).atan().to_degrees();
    let top = (300.0f64 / 2000.0).atan().to_degrees() - 10.0;
    ((below - top) / 20.0 * H as f64) as u32
}

/// Shadowed ground (1.6–2.3 km ahead, behind the occluder from the sun) and open ground in
/// front of it (1.05–1.25 km), with far shadows on and off.
fn check(name: &str, occluder: impl Fn(&mut World) + Copy, wait: impl Fn(&mut App) -> bool) {
    let (shadow0, shadow1) = (row_at(2300.0), row_at(1600.0));
    let (open0, open1) = (row_at(1250.0), row_at(1050.0));
    let mut results = Vec::new();
    for far in [8000.0, 0.0] {
        let mut app = plain_app(far, occluder);
        if !wait(&mut app) {
            return;
        }
        let img = capture(&mut app, &format!("far_shadow_{name}_{far}"));
        results.push((band(&img, shadow0, shadow1), band(&img, open0, open1)));
    }
    let [(on_shadow, on_open), (off_shadow, off_open)] = results[..] else {
        unreachable!()
    };
    eprintln!(
        "{name}: far shadows on: shaded {on_shadow:.4}, open {on_open:.4}; off: {off_shadow:.4}, \
         {off_open:.4}"
    );
    assert!(
        on_open > 0.02,
        "{name}: the open ground is not lit ({on_open})"
    );
    assert!(
        on_shadow < on_open * 0.5,
        "{name}: the far occluder casts no shadow 1.6–2.3 km away ({on_shadow} vs {on_open})"
    );
    assert!(
        off_shadow > off_open * 0.8,
        "{name}: without far shadows that ground should be lit ({off_shadow} vs {off_open})"
    );
}

fn has_gpu(app: &mut App) -> bool {
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return false;
    }
    app.update_n(3);
    true
}

/// A ridge 3 km wide, 600 m high, 4.5 km to the sun's side of the view: at 10° its shadow
/// reaches 3.4 km across the plain, over the ground 1.5–2.5 km ahead.
fn ridge(w: &mut World) {
    w.spawn((
        Transform::from_xyz(-3000.0, 300.0, -2000.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [200.0, 600.0, 1000.0],
            },
            StandardMaterial::color(0.4, 0.4, 0.4),
        ),
    ));
}

#[test]
fn a_distant_ridge_shades_the_plain_kilometers_away() {
    check("instanced", ridge, has_gpu);
}

/// The same as a dense dome drawn as virtual geometry clusters.
fn dome(w: &mut World) {
    let mesh = engine_render::mesh::icosphere(1.0, 6);
    let handle = w.resource_mut::<engine_assets::Assets<Mesh>>().add(mesh);
    w.spawn((
        Transform::from_xyz(-3000.0, 0.0, -2000.0).with_scale(Vec3::new(300.0, 700.0, 600.0)),
        MeshRenderer::new(handle, StandardMaterial::color(0.4, 0.4, 0.4)),
    ));
}

fn wait_for_clusters(app: &mut App) -> bool {
    if !has_gpu(app) {
        return false;
    }
    let started = Instant::now();
    loop {
        let describe = app.world.resource::<Renderer>().describe();
        let ready = describe["features"]
            .as_array()
            .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
            .and_then(|f| f["state"]["virtual_geometry"]["meshes_ready"].as_u64())
            .unwrap_or(0);
        if ready >= 1 {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "the cluster build didn't finish"
        );
        std::thread::sleep(Duration::from_millis(20));
        app.update();
    }
    app.update_n(4);
    true
}

#[test]
fn a_distant_clustered_dome_shades_the_plain_kilometers_away() {
    check("clusters", dome, wait_for_clusters);
}
