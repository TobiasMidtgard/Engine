//! GPU cost of the lighting each opaque pixel pays: a field of boxes on a ground slab under a
//! sun, rendered at a small size with the default settings, then again with one lighting part
//! turned off at a time. Prints the GPU passes (`render.gpu.*`, averaged) for each, so the share
//! of each part in `scene: meshes` shows. Under lavapipe (a CPU rasterizer) pass times follow
//! shader work closely; on a GPU they are the real thing. Needs a GPU or lavapipe.
//!
//! ```sh
//! cargo run -p engine_render --release --example lighting_cost -- 30
//! ```
//!
//! The first number is the frames averaged per run (30); `--small` renders 480x270 instead of
//! 1600x900 (lavapipe). Lavapipe's ratios are not a GPU's (its rasterizer and compute run on the
//! CPU): read the shares on a real GPU. The last two runs add a lake-sized mirror
//! (`PlanarReflection`) in front of the camera and print its view's cost (`render.extra_views*`)
//! with its own shadow maps and with the main view's (`mirror_shares_lighting`).

use std::collections::BTreeMap;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::RenderDisabled;
use engine_render::prelude::*;

type Tweak = fn(&mut RenderSettings);

fn scene(width: u32, height: u32, tweak: Tweak, mirror: bool) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(RenderPlugin);
    tweak(&mut app.world.resource_mut::<RenderSettings>());
    let w = &mut app.world;
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, -0.5, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [400.0, 1.0, 400.0],
            },
            StandardMaterial::color(0.35, 0.33, 0.3).with_roughness(0.9),
        ),
    ));
    for i in 0..400 {
        let (x, z) = ((i % 20) as f64, (i / 20) as f64);
        let h = 1.0 + (i * 7 % 5) as f32;
        w.spawn((
            Transform::from_translation(DVec3::new(
                (x - 10.0) * 4.0,
                h as f64 * 0.5,
                -(z * 4.0) - 4.0,
            )),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.5, h, 1.5],
                },
                StandardMaterial::color(0.2 + 0.03 * (i % 20) as f32, 0.5, 0.4)
                    .with_roughness(0.3 + 0.03 * (i % 15) as f32),
            ),
        ));
    }
    if mirror {
        w.spawn((
            Transform::from_xyz(0.0, 0.05, -20.0),
            engine_render::PlanarReflection::new(Vec2::new(60.0, 30.0)),
        ));
    }
    w.spawn((
        Camera::perspective(70.0),
        Transform::from_xyz(0.0, 3.0, 4.0).looking_at(DVec3::new(0.0, 1.0, -30.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.4, -0.6, -0.5), Vec3::Y),
    ));
    app
}

/// The window size: 1600x900 (the showcase's), or `--small` for 480x270 under lavapipe.
fn size() -> (u32, u32) {
    if std::env::args().any(|a| a == "--small") {
        (480, 270)
    } else {
        (1600, 900)
    }
}

fn measure(frames: usize, tweak: Tweak, mirror: bool) -> Option<BTreeMap<String, f64>> {
    let (width, height) = size();
    let mut app = scene(width, height, tweak, mirror);
    app.update_n(20);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        return None;
    }
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    for _ in 0..frames {
        app.update();
        let d = app.world.resource::<Diagnostics>();
        for (k, v) in &d.metrics {
            if let Some(pass) = k.strip_prefix("render.gpu.") {
                *sums.entry(pass.to_string()).or_default() += v / frames as f64;
            } else if k.starts_with("render.extra_views") {
                *sums.entry(k.to_string()).or_default() += v / frames as f64;
            }
        }
    }
    Some(sums)
}

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    let runs: [(&str, Tweak); 10] = [
        ("defaults", |_| {}),
        ("no sky occlusion", |s| s.sky_occlusion = false),
        ("no probe gi", |s| s.probe_gi = false),
        ("hard shadows", |s| s.shadow_softness = 0.0),
        ("no sun shadows", |s| s.shadow_distance = 0.0),
        ("no screen-space gi", |s| s.screen_space_gi = false),
        ("no reflections", |s| {
            s.screen_space_reflections = false;
            s.world_reflections = false;
        }),
        ("no volumetric fog", |s| s.volumetric_fog = false),
        ("no ambient occlusion", |s| s.ambient_occlusion = false),
        ("render scale 0.75", |s| s.render_scale = 0.75),
    ];
    let mut base: Option<BTreeMap<String, f64>> = None;
    for (name, tweak) in runs {
        let Some(m) = measure(frames, tweak, false) else {
            eprintln!("no GPU");
            return;
        };
        let get = |m: &BTreeMap<String, f64>, k: &str| m.get(k).copied().unwrap_or(0.0);
        let total: f64 = m
            .iter()
            .filter(|(k, _)| !k.contains(": "))
            .map(|(_, v)| v)
            .sum();
        println!(
            "{name:<22} total {total:7.2}  scene {:7.2}  prepass {:6.2}  shadows {:6.2}  \
             sky occlusion {:5.2}  probe gi {:5.2}  world reflections {:5.2}  volumetric fog {:5.2}",
            get(&m, "scene"),
            get(&m, "prepass"),
            get(&m, "shadows"),
            get(&m, "sky occlusion"),
            get(&m, "probe gi"),
            get(&m, "world reflections"),
            get(&m, "volumetric fog"),
        );
        if base.is_none() {
            let mut rows: Vec<_> = m.iter().collect();
            rows.sort_by(|a, b| b.1.total_cmp(a.1));
            for (k, v) in rows.iter().take(14) {
                println!("    {k:<28} {v:7.2}");
            }
            base = Some(m);
        }
    }
    let mirrors: [(&str, Tweak); 2] = [
        ("mirror, own maps", |s| s.mirror_shares_lighting = false),
        ("mirror, main view's", |_| {}),
    ];
    for (name, tweak) in mirrors {
        let Some(m) = measure(frames, tweak, true) else {
            return;
        };
        let get = |k: &str| m.get(k).copied().unwrap_or(0.0);
        println!(
            "{name:<22} extra view CPU {:6.2} ms  GPU {:6.2} ms  (views {:.0}, sharing {:.0})",
            get("render.extra_views_ms"),
            get("render.extra_views_gpu_ms"),
            get("render.extra_views"),
            get("render.extra_views_shared_lighting"),
        );
    }
}
