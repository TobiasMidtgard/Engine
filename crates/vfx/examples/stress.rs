//! Particle stress test: keeps a million particles alive and reports frame times.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example stress                  # 1 000 000 glowing motes
//! cargo run -p engine_vfx --release --example stress -- 250000 lit    # fewer, lit by sun and sky
//! cargo run -p engine_vfx --release --example stress -- 2000000 frames=600
//! ```
//!
//! Renders offscreen at 1920×1080 and waits for the GPU after every frame, so each time covers
//! the whole frame. It first times the empty scene, then the same scene with the particles, and
//! prints both and the difference (what the particles cost). Run it on the GPU you care about: a
//! software driver such as lavapipe is far slower than any real GPU.

use std::time::Instant;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats, Renderer, wgpu};
use engine_vfx::prelude::*;
use engine_vfx::{EffectDesc, EmitShape, Gradient, Sprite, VfxRuntime};

const W: u32 = 1920;
const H: u32 = 1080;
/// Emitters in a square grid; each keeps `particles / GRID²` alive.
const GRID: usize = 10;
const LIFETIME: f32 = 2.0;

struct Options {
    particles: u32,
    lit: bool,
    frames: usize,
}

fn options() -> Options {
    let mut o = Options {
        particles: 1_000_000,
        lit: false,
        frames: 300,
    };
    for arg in std::env::args().skip(1) {
        if let Ok(n) = arg.replace('_', "").parse::<u32>() {
            o.particles = n;
        } else if arg == "lit" {
            o.lit = true;
        } else if let Some(n) = arg.strip_prefix("frames=").and_then(|n| n.parse().ok()) {
            o.frames = n;
        } else {
            eprintln!("ignoring unknown argument {arg:?} (use a particle count, \"lit\" or frames=N)");
        }
    }
    o
}

/// One emitter's worth of particles: a swirling ball that stays full.
fn cloud(per_emitter: u32, lit: bool) -> EffectDesc {
    EffectDesc {
        name: "stress.cloud".into(),
        prewarm: LIFETIME,
        emitters: vec![EmitterDesc {
            rate: per_emitter as f32 / LIFETIME,
            shape: EmitShape::Sphere {
                radius: 1.6,
                surface: false,
            },
            speed: [0.2, 0.8],
            lifetime: [LIFETIME, LIFETIME],
            size: [0.04, 0.08],
            drag: 0.5,
            turbulence: 1.0,
            wind: 0.0,
            sprite: if lit { Sprite::Dust } else { Sprite::Glow },
            lit: if lit { 1.0 } else { 0.0 },
            emissive: if lit { 0.0 } else { 1500.0 },
            additive: if lit { 0.0 } else { 1.0 },
            color: Gradient::constant([1.0, 0.55, 0.2, 0.8]),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Frame times in milliseconds, each waiting for the GPU to finish.
fn time_frames(app: &mut App, frames: usize) -> Vec<f64> {
    let mut times = Vec::with_capacity(frames);
    for _ in 0..frames {
        let t = Instant::now();
        app.update();
        let _ = app
            .world
            .resource::<Renderer>()
            .device()
            .poll(wgpu::PollType::wait_indefinitely());
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times
}

fn summary(times: &mut [f64]) -> (f64, f64, f64, f64) {
    times.sort_by(f64::total_cmp);
    let avg = times.iter().sum::<f64>() / times.len().max(1) as f64;
    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    (avg, at(0.5), at(0.95), at(0.99))
}

fn print_line(label: &str, times: &mut [f64]) -> f64 {
    let (avg, p50, p95, p99) = summary(times);
    println!(
        "{label:<10} avg {avg:6.2} ms  p50 {p50:6.2}  p95 {p95:6.2}  p99 {p99:6.2}  ({:.0} fps)",
        1000.0 / avg
    );
    avg
}

fn main() {
    let o = options();
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
    app.add_plugins((RenderPlugin, VfxPlugin));
    app.world.resource_mut::<VfxSettings>().max_particles = (o.particles as f32 * 1.3) as u32;
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 20_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.6, -0.7), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [120.0, 120.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55504a).with_roughness(0.9),
        ),
    ));
    let span = (GRID - 1) as f64 * 5.0;
    w.spawn((
        Camera::perspective(55.0).with_exposure(Exposure::Manual { ev100: 11.0 }),
        Transform::from_xyz(0.0, 18.0, span * 0.5 + 30.0).looking_at(DVec3::new(0.0, 2.0, 0.0), Vec3::Y),
    ));

    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    let adapter = app.world.resource::<RenderStats>().adapter.clone();
    println!("GPU: {adapter}");

    // The empty scene first.
    time_frames(&mut app, 20);
    let mut base = time_frames(&mut app, o.frames / 2);

    // Then the particles.
    let per = (o.particles / (GRID * GRID) as u32).max(1);
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(cloud(per, o.lit))
        .expect("stress effect");
    for i in 0..GRID {
        for j in 0..GRID {
            let p = DVec3::new(
                i as f64 * 5.0 - span * 0.5,
                2.0,
                j as f64 * 5.0 - span * 0.5,
            );
            app.world.spawn((
                Transform::from_translation(p),
                ParticleEffect::new("stress.cloud"),
            ));
        }
    }
    time_frames(&mut app, 30);
    let mut with = time_frames(&mut app, o.frames);

    let runtime = app.world.resource::<VfxRuntime>();
    let starved = runtime.stats.starved;
    println!(
        "{} particles in {} emitters ({} pool slots), {W}×{H}, {}",
        per as usize * GRID * GRID,
        GRID * GRID,
        runtime.pool().used(),
        if o.lit { "lit" } else { "glowing" }
    );
    if starved > 0 {
        println!("warning: {starved} emitters could not get room in the pool");
    }
    let a = print_line("scene", &mut base);
    let b = print_line("particles", &mut with);
    println!("particles cost {:.2} ms per frame", b - a);
}
