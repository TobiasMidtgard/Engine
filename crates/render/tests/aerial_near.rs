//! Haze near the camera: the air's haze builds up smoothly from the camera outward. It used to
//! start at 200 m all at once, so in thick air (a small planet's, sixteen times Earth's), seen
//! from 136 m up, the ground nearer than 200 m showed as a sharp-edged disc of brighter,
//! more saturated color. With the aerial perspective table and without. Needs a GPU or
//! lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 96;
const H: u32 = 160;

/// Linear radiance (sRGB decoded, mean of the channels) down the middle column, top to bottom.
fn column(name: &str, table: bool) -> Option<Vec<f32>> {
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
        s.anti_aliasing = AntiAliasing::None;
        s.aerial_table = table;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    {
        let mut air = app.world.resource_mut::<Atmosphere>();
        let a: &mut Atmosphere = &mut air;
        let k = 16.0;
        a.thickness /= k;
        a.rayleigh_scale_height /= k;
        a.mie_scale_height /= k;
        for c in a
            .rayleigh_scattering
            .iter_mut()
            .chain(a.mie_scattering.iter_mut())
            .chain(a.mie_absorption.iter_mut())
            .chain(a.ozone_absorption.iter_mut())
        {
            *c *= k;
        }
    }
    let w = &mut app.world;
    // Steeply down, so the ground 100 to 300 m away fills the view.
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 136.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -170.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20_000.0, 20_000.0],
                subdivisions: 1,
            },
            StandardMaterial {
                perceptual_roughness: 1.0,
                ..StandardMaterial::color(0.45, 0.33, 0.12)
            },
        ),
    ));
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("aerial_near_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let decode = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(
        (0..H)
            .map(|y| {
                // The mean of a few columns in the middle, so one pixel's noise doesn't count.
                let xs = (W / 2 - 4)..(W / 2 + 4);
                let n = xs.len() as f32;
                xs.map(|x| {
                    let p = img.get_pixel(x, y);
                    (decode(p[0]) + decode(p[1]) + decode(p[2])) / 3.0
                })
                .sum::<f32>()
                    / n
            })
            .collect(),
    )
}

#[test]
fn the_haze_has_no_edge_near_the_camera() {
    for table in [true, false] {
        let Some(c) = column(&format!("table_{table}"), table) else {
            return;
        };
        // Row to row, the ground's brightness changes smoothly: the largest step is at most
        // a few times the typical one.
        let steps: Vec<f32> = c.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        let mut sorted = steps.clone();
        sorted.sort_by(f32::total_cmp);
        let typical = sorted[sorted.len() / 2].max(1e-4);
        let (at, largest) = steps
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |m, (i, &s)| if s > m.1 { (i, s) } else { m });
        eprintln!("table {table}: largest step {largest:.5} at row {at}, typical {typical:.5}");
        assert!(
            largest < 4.0 * typical + 0.002,
            "with the table {table}, the haze steps by {largest} at row {at} (typically {typical})"
        );
    }
}
