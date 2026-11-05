//! Instance rows (`MeshInstances`): one entity holding a region's copies of a mesh draws the
//! same picture as an entity per copy (placement, turns, scales, shadows), and tints its rows.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 200;
/// The region's anchor: off the origin and turned, so rows are placed through its transform.
const ANCHOR: DVec3 = DVec3::new(3.0, 0.0, -20.0);
const ANCHOR_YAW: f32 = 0.4;

/// A 6 x 6 grid of boxes, each turned and sized its own way.
fn rows(tint: [f32; 3]) -> Vec<InstanceRow> {
    (0..36)
        .map(|i| {
            let (x, z) = ((i % 6) as f32, (i / 6) as f32);
            InstanceRow {
                position: Vec3::new(x * 2.5 - 6.25, 0.5, z * 2.5 - 6.25),
                yaw: i as f32 * 0.37,
                scale: 0.6 + 0.08 * (i % 5) as f32,
                tint,
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Way {
    Entities,
    Rows([f32; 3]),
}

fn picture(way: Way) -> Option<RgbaImage> {
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
        s.volumetric_fog = false;
        s.probe_gi = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual { step_secs: 1.0 });
    app.world.resource_mut::<Time>().set_max_delta(1.0);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.5 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 12.0, 0.0).looking_at(ANCHOR, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.6, -0.7, -0.3), Vec3::Y),
    ));
    w.spawn((
        Transform::from_translation(ANCHOR),
        MeshRenderer::new(
            Shape::Plane {
                size: [60.0, 60.0],
                subdivisions: 4,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    let cube = || {
        MeshRenderer::new(
            Shape::Cuboid { size: [1.0; 3] },
            StandardMaterial::color(0.6, 0.6, 0.6),
        )
    };
    let anchor =
        Transform::from_translation(ANCHOR).with_rotation(Quat::from_rotation_y(ANCHOR_YAW));
    match way {
        Way::Entities => {
            for r in rows([1.0; 3]) {
                let at = anchor.transform_point(r.position.as_dvec3());
                w.spawn((
                    Transform::from_translation(at)
                        .with_rotation(Quat::from_rotation_y(ANCHOR_YAW + r.yaw))
                        .with_scale(Vec3::splat(r.scale)),
                    cube(),
                ));
            }
        }
        Way::Rows(tint) => {
            w.spawn((anchor, cube(), MeshInstances::new(rows(tint))));
        }
    }
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    app.update_n(4);
    let name = match way {
        Way::Entities => "entities".to_string(),
        Way::Rows(t) => format!("rows_{}", t[0]),
    };
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("instance_rows_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean absolute channel difference (0..255) and the share of pixels differing by more than 24.
fn difference(a: &RgbaImage, b: &RgbaImage) -> (f32, f32) {
    let (mut sum, mut far) = (0u64, 0u32);
    for (p, q) in a.pixels().zip(b.pixels()) {
        let d = (0..3).map(|c| p[c].abs_diff(q[c]) as u64).max().unwrap();
        sum += d;
        far += (d > 24) as u32;
    }
    let n = (W * H) as f32;
    (sum as f32 / n, far as f32 / n)
}

/// Mean red minus green over the pixels where `a` and `b` differ (the boxes).
fn redness(img: &RgbaImage, plain: &RgbaImage) -> f32 {
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for (p, q) in img.pixels().zip(plain.pixels()) {
        if (0..3).any(|c| p[c].abs_diff(q[c]) > 8) {
            sum += p[0] as f32 - p[1] as f32;
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

#[test]
fn rows_draw_what_an_entity_per_copy_draws() {
    let Some(entities) = picture(Way::Entities) else {
        return;
    };
    let Some(rows) = picture(Way::Rows([1.0; 3])) else {
        return;
    };
    let Some(red) = picture(Way::Rows([1.6, 0.5, 0.5])) else {
        return;
    };
    let (mean, share) = difference(&entities, &rows);
    eprintln!(
        "rows vs entities: mean difference {mean:.3}, {:.3}% of pixels far",
        share * 100.0
    );
    assert!(
        mean < 1.0 && share < 0.005,
        "rows drew another picture than an entity per box (mean {mean}, {share} far)"
    );
    let r = redness(&red, &rows);
    eprintln!("tinted rows: red minus green {r:.1}");
    assert!(r > 20.0, "tinted rows are not red ({r})");
}
