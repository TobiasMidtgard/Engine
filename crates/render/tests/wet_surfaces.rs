//! Rain on porous and sealed surfaces: wet grass and soil darken but stay rough, while a wet
//! sealed surface (glazed tile, paint) turns nearly mirror-like. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_core::math::DVec2;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::{Grid2, SurfaceCell, SurfaceLayers};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// Where the sun (35° up straight ahead) is mirrored toward a camera 3 m up at z = 6.
const GLINT: DVec3 = DVec3::new(0.0, 0.0, 1.716);

/// A plane of `material` with the ground `wetness` everywhere, the sun 35° up straight ahead,
/// seen from 3 m up looking at the sun's mirror image in it. Exposed for that glint (EV 20), so
/// a diffuse or rough surface stays dark and only a glossy one lights up.
fn wet_plane(material: StandardMaterial, wetness: f32) -> App {
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
    }
    let cell = SurfaceCell {
        wetness,
        ..Default::default()
    };
    app.world.insert_resource(SurfaceLayers {
        grid: Grid2::centered(DVec2::ZERO, 64.0, 1.0, cell),
    });
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 20.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 3.0, 6.0).looking_at(GLINT, Vec3::Y),
    ));
    let e = 35f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.0, -e.sin(), e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            material,
        ),
    ));
    app
}

/// The brightest luma (0..255) within 7 pixels of where the sun's mirror image lands.
fn glint(material: StandardMaterial, wetness: f32, name: &str) -> Option<f32> {
    let mut app = wet_plane(material, wetness);
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img: RgbaImage = image::open(&path).expect("png").to_rgba8();
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.view_proj * (GLINT - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let (x, y) = (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    );
    let mut brightest = 0.0f32;
    for dy in -7..=7 {
        for dx in -7..=7 {
            let p = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            brightest =
                brightest.max(0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32);
        }
    }
    Some(brightest)
}

fn grass() -> StandardMaterial {
    StandardMaterial::color(0.12, 0.25, 0.06).with_roughness(0.9)
}

fn tile() -> StandardMaterial {
    StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.3)
}

#[test]
fn wet_grass_stays_rough_while_a_wet_sealed_surface_turns_glossy() {
    let Some(grass_dry) = glint(grass(), 0.0, "wet_grass_dry") else {
        return;
    };
    let grass_wet = glint(grass(), 1.0, "wet_grass_wet").unwrap();
    let tile_dry = glint(tile(), 0.0, "wet_tile_dry").unwrap();
    let tile_wet = glint(tile(), 1.0, "wet_tile_wet").unwrap();
    eprintln!(
        "sun glint: grass dry {grass_dry} wet {grass_wet}; sealed tile dry {tile_dry} wet {tile_wet}"
    );
    // Soaked grass is darker and only a little less rough: no mirror image of the sun.
    assert!(
        grass_wet < grass_dry + 10.0,
        "wet grass should not turn glossy ({grass_wet} vs {grass_dry} dry)"
    );
    // A sealed surface carries a film of water: a sharp, bright image of the sun.
    assert!(
        tile_wet > tile_dry + 60.0,
        "a wet sealed surface should turn glossy ({tile_wet} vs {tile_dry} dry)"
    );
}
