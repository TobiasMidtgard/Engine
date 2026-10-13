//! Per-vertex wind weight: on a swaying material (`StandardMaterial::wind_sway`) the vertex
//! color's alpha scales the sway (0 = still, 1 = full), and is not taken as opacity. Meshes
//! without vertex colors sway fully, as before. Needs a GPU or lavapipe; passes with a note
//! when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WeatherState;
use image::RgbaImage;

const W: u32 = 160;
const H: u32 = 160;

/// A 6 m post in a strong wind (15 m/s along +x), seen from the side, with `sway` meters of
/// wind sway and every vertex's alpha set to `alpha` (`None`: the mesh's default colors).
fn render_post(name: &str, sway: f32, alpha: Option<f32>) -> Option<RgbaImage> {
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
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.bloom_intensity = 0.0;
    }
    app.world.insert_resource(WeatherState {
        wind: Vec3::new(15.0, 0.0, 0.0),
        ..Default::default()
    });
    let mut mesh = Shape::Cuboid {
        size: [0.4, 6.0, 0.4],
    }
    .mesh();
    if let Some(a) = alpha {
        for v in &mut mesh.vertices {
            v.color[3] = a;
        }
    }
    let handle = app
        .world
        .resource_mut::<engine_assets::Assets<Mesh>>()
        .add(mesh);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 3.0, 9.0).looking_at(DVec3::new(0.0, 3.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.3, -0.8, -0.5), Vec3::Y),
    ));
    let mut material = StandardMaterial::color(0.8, 0.2, 0.1);
    material.wind_sway = sway;
    w.spawn((
        Transform::from_xyz(0.0, 3.0, 0.0),
        MeshRenderer::new(handle, material),
    ));
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("wind_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Columns where the top fifth of the image is the post's red, not the sky.
fn red_columns_at_top(img: &RgbaImage) -> Vec<u32> {
    (0..W)
        .filter(|&x| {
            (H / 10..H / 5).any(|y| {
                let p = img.get_pixel(x, y);
                p[0] as i32 > p[2] as i32 + 40
            })
        })
        .collect()
}

fn mean_column(cols: &[u32]) -> f32 {
    cols.iter().map(|&c| c as f32).sum::<f32>() / cols.len().max(1) as f32
}

#[test]
fn vertex_alpha_scales_wind_sway() {
    let Some(still) = render_post("still", 0.0, None) else {
        return;
    };
    let swaying = render_post("full", 1.5, None).expect("gpu");
    let weightless = render_post("weight0", 1.5, Some(0.0)).expect("gpu");
    let (c_still, c_sway, c_zero) = (
        red_columns_at_top(&still),
        red_columns_at_top(&swaying),
        red_columns_at_top(&weightless),
    );
    eprintln!(
        "top of the post: still {:.1} ({} px), swaying {:.1} ({} px), weight 0 {:.1} ({} px)",
        mean_column(&c_still),
        c_still.len(),
        mean_column(&c_sway),
        c_sway.len(),
        mean_column(&c_zero),
        c_zero.len()
    );
    assert!(!c_still.is_empty(), "the post's top is not in view");
    // Without vertex colors (alpha 1) the top sways downwind (+x, to the right).
    assert!(
        mean_column(&c_sway) > mean_column(&c_still) + 2.0,
        "a swaying post without vertex colors doesn't sway"
    );
    // Alpha 0: it stays where the still post is, and stays opaque (alpha is not opacity).
    assert!(
        (mean_column(&c_zero) - mean_column(&c_still)).abs() < 0.5,
        "a post with wind weight 0 still sways"
    );
    assert_eq!(
        c_zero.len(),
        c_still.len(),
        "a weight of 0 made the post see-through"
    );
}
