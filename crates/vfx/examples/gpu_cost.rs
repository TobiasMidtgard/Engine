//! Measures what particles cost the GPU: the Spell court seen from each of its tour stops,
//! rendered headless at Ultra, with particles on and then off. Prints the median of the GPU's
//! transparent pass (where particles, ribbons and smoke shadows draw), the particle compute and
//! the whole GPU frame, in milliseconds.
//!
//! On lavapipe the "GPU" is the CPU rasterizing, so the numbers rank pixel cost (overdraw,
//! fragment work) rather than give a card's milliseconds.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example gpu_cost                    # every stop
//! cargo run -p engine_vfx --release --example gpu_cost -- magic_portal    # some stops
//! VFX_GPU_SIZE=1920x1080 VFX_GPU_FRAMES=40 cargo run ...
//! VFX_GPU_SPLIT=1 cargo run ...     # also weigh smoke shadows, soft particles, heat haze
//! VFX_GPU_SHOT=dir cargo run ...     # and save a picture of each stop
//! ```

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::RenderDisabled;
use engine_render::prelude::*;
use engine_vfx::prelude::*;

const STEP: f64 = 1.0 / 30.0;

fn env_size() -> (u32, u32) {
    std::env::var("VFX_GPU_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((1280, 720))
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Median over `frames` frames of the GPU rows: (transparent, vfx compute, frame).
fn measure(app: &mut App, frames: usize) -> (f64, f64, f64) {
    let (mut t, mut c, mut f) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..frames {
        app.update();
        let d = app.world.resource::<Diagnostics>();
        if let Some(v) = d.get("render.gpu.transparent") {
            t.push(v);
        }
        c.push(d.get("render.gpu.compute vfx_particles").unwrap_or(0.0));
        if let Some(v) = d.get("render.gpu_ms") {
            f.push(v);
        }
    }
    (median(t), median(c), median(f))
}

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let frames: usize = std::env::var("VFX_GPU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let (width, height) = env_size();
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.insert_resource(engine_world::Scalability::preset(
        engine_world::QualityPreset::Ultra,
    ));
    app.add_plugins((RenderPlugin, VfxPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 2_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.6, -0.25, -0.75), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b655d).with_roughness(0.9),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(60.0).with_exposure(Exposure::Manual { ev100: 9.0 }),
            Transform::from_xyz(10.0, 4.8, 16.0),
        ))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        engine_vfx::demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    println!(
        "{:<18} {:>9} {:>9} {:>9} | {:>9} {:>9} | {:>9}",
        "stop", "transp", "vfx comp", "gpu", "transp 0", "gpu 0", "vfx gpu"
    );
    for view in engine_vfx::demo::VIEWS {
        if !only.is_empty() && !only.iter().any(|o| view.key.starts_with(o.as_str())) {
            continue;
        }
        let eye = DVec3::from_array(view.eye);
        let target = DVec3::from_array(view.target);
        *app.world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        app.world.resource_mut::<VfxSettings>().enabled = true;
        // Let the court's loops settle and the view's effects get going.
        let warm = ((view.warmup.max(2.0) as f64) / STEP).ceil() as usize;
        app.update_n(warm);
        let on = measure(&mut app, frames);
        // `VFX_GPU_SPLIT=1`: also the transparent pass without smoke shadows, without soft
        // particles and without heat haze, to weigh each.
        let mut split = Vec::new();
        if std::env::var("VFX_GPU_SPLIT").is_ok() {
            type Toggle = fn(&mut VfxSettings, bool);
            let toggles: [(&str, Toggle); 3] = [
                ("shadows", |s, v| s.particle_shadows = v),
                ("soft", |s, v| s.soft_particles = v),
                ("haze", |s, v| s.heat_haze = v),
            ];
            for (name, set) in toggles {
                set(&mut app.world.resource_mut::<VfxSettings>(), false);
                app.update_n(2);
                let m = measure(&mut app, frames);
                set(&mut app.world.resource_mut::<VfxSettings>(), true);
                split.push(format!("no {name} {:.1}", m.0));
            }
        }
        // `VFX_GPU_SHOT=dir`: a picture of the stop with particles on.
        if let Ok(dir) = std::env::var("VFX_GPU_SHOT") {
            let path = std::path::Path::new(&dir).join(format!("{}.png", view.key));
            if let Err(e) = engine_render::capture_now(&mut app.world, &path) {
                eprintln!("{}: {e}", view.key);
            }
        }
        app.world.resource_mut::<VfxSettings>().enabled = false;
        app.update_n(4);
        let off = measure(&mut app, frames);
        println!(
            "{:<18} {:>9.2} {:>9.2} {:>9.2} | {:>9.2} {:>9.2} | {:>9.2}  {}",
            view.key,
            on.0,
            on.1,
            on.2,
            off.0,
            off.2,
            on.2 - off.2,
            split.join(", ")
        );
    }
}
