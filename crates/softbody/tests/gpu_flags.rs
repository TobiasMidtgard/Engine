//! Flags simulated on the GPU (`SimSettings::gpu`), offscreen: a flag in a steady breeze
//! streams out downwind without stretching, flies like the same flag simulated on the CPU,
//! and is drawn like one; it keeps hanging from its pole when the world is re-anchored; a
//! field of 64 flags runs in one dispatch. Needs a GPU or software
//! Vulkan (lavapipe); passes with a note when neither is available (`RenderDisabled`),
//! unless `ENGINE_REQUIRE_GPU=1`.

use engine_core::math::DQuat;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_softbody::{Cloth, ClothPins, GpuClothProbe, SoftBodyPlugin, SoftBodySim, SoftMaterial};
use engine_world::{WeatherState, WorldShift, WorldShiftSet};

const W: u32 = 480;
const H: u32 = 270;
const WIND: f32 = 6.0;

fn app() -> App {
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
    app.add_plugins((RenderPlugin, engine_world::WorldPlugin, SoftBodyPlugin));
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(WIND, 0.0, 0.0);
        weather.gustiness = 0.0;
    }
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.3, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [60.0, 60.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x808080).with_roughness(0.9),
        ),
    ));
    app
}

fn flag(color: u32, gpu: bool) -> Cloth {
    let mut flag = Cloth::flag([1.2, 0.8], SoftMaterial::hex(color));
    flag.resolution = [16, 10];
    flag.sim.gpu = gpu;
    flag
}

fn gpu_ms(app: &App) -> Option<f32> {
    let renderer = app.world.get_resource::<Renderer>()?.describe();
    renderer["features"]
        .as_array()?
        .iter()
        .find(|f| f["name"] == "softbody")?["state"]["gpu_cloth_ms"]
        .as_f64()
        .map(|ms| ms as f32)
}

/// Mean and largest length of the grid's links along u and v over their rest length.
fn stretch(p: &[Vec3], [nu, nv]: [u32; 2], [du, dv]: [f32; 2]) -> (f32, f32) {
    let (nu, nv) = (nu as usize, nv as usize);
    let (mut sum, mut max, mut n) = (0.0, 0.0f32, 0);
    for j in 0..nv {
        for i in 0..nu {
            let a = p[j * nu + i];
            let mut add = |b: Vec3, rest: f32| {
                let r = (b - a).length() / rest;
                sum += r;
                max = max.max(r);
                n += 1;
            };
            if i + 1 < nu {
                add(p[j * nu + i + 1], du);
            }
            if j + 1 < nv {
                add(p[(j + 1) * nu + i], dv);
            }
        }
    }
    (sum / n as f32, max)
}

/// The free edge's mean position relative to the pole's top.
fn trailing_edge(p: &[Vec3], [nu, nv]: [u32; 2]) -> Vec3 {
    let (nu, nv) = (nu as usize, nv as usize);
    (0..nv).map(|j| p[j * nu + nu - 1] - p[0]).sum::<Vec3>() / nv as f32
}

#[test]
fn a_gpu_flag_streams_out_in_the_wind_like_a_cpu_one() {
    let mut app = app();
    let gpu = app
        .world
        .spawn((Transform::from_xyz(0.0, 2.0, 0.0), flag(0xc0201c, true)))
        .id();
    let cpu = app
        .world
        .spawn((Transform::from_xyz(-2.2, 2.0, 0.0), flag(0x2040c0, false)))
        .id();
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(-0.5, 1.8, 4.2).looking_at(DVec3::new(-0.5, 1.6, 0.0), Vec3::Y),
    ));
    app.world.insert_resource(GpuClothProbe {
        entity: Some(gpu),
        ..Default::default()
    });
    app.update_n(180);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    assert!(
        app.world.get::<SoftBodySim>(gpu).is_none(),
        "the GPU flag isn't simulated on the CPU too"
    );

    let probe = app.world.resource::<GpuClothProbe>().clone();
    assert_eq!(probe.grid, [16, 10]);
    assert_eq!(probe.positions.len(), 160);
    let pole = DVec3::new(0.0, 2.0, 0.0);
    let on_gpu: Vec<Vec3> = probe
        .positions
        .iter()
        .map(|p| (*p - pole).as_vec3())
        .collect();
    assert!(on_gpu.iter().all(|p| p.is_finite()));
    let body = app.world.get::<SoftBodySim>(cpu).expect("CPU flag built");
    let on_cpu: Vec<Vec3> = body.sim.pos.clone();

    let rest = [1.2 / 15.0, 0.8 / 9.0];
    let (gpu_mean, gpu_max) = stretch(&on_gpu, probe.grid, rest);
    let (cpu_mean, cpu_max) = stretch(&on_cpu, probe.grid, rest);
    let gpu_edge = trailing_edge(&on_gpu, probe.grid);
    let cpu_edge = trailing_edge(&on_cpu, probe.grid);
    eprintln!(
        "GPU: links {gpu_mean:.3} mean, {gpu_max:.3} max, free edge {gpu_edge:.2}\n\
         CPU: links {cpu_mean:.3} mean, {cpu_max:.3} max, free edge {cpu_edge:.2}\n\
         GPU pass {:?} ms",
        gpu_ms(&app)
    );
    // The pole stays put and the cloth hangs off it, streaming downwind.
    assert!(on_gpu[0].length() < 1e-3, "pole top moved: {}", on_gpu[0]);
    assert!(
        gpu_edge.x > 0.9,
        "flag doesn't stream out: free edge at {gpu_edge}"
    );
    // Stiff fabric: links barely stretch.
    assert!(
        gpu_mean < 1.03 && gpu_max < 1.12,
        "stretch {gpu_mean} mean, {gpu_max} max"
    );
    // Flies like the CPU flag.
    assert!(
        (gpu_edge - cpu_edge).length() < 0.2,
        "GPU free edge {gpu_edge} vs CPU {cpu_edge}"
    );

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("softbody_gpu_flag.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    eprintln!("wrote {}", path.display());
    let img = image::open(&path).expect("png").to_rgba8();
    let count = |f: &dyn Fn(f32, f32, f32) -> bool| {
        img.pixels()
            .filter(|p| f(p.0[0] as f32, p.0[1] as f32, p.0[2] as f32))
            .count()
    };
    let red = count(&|r, g, b| r > 60.0 && r > 2.0 * g && r > 2.0 * b);
    // Saturated blue only: the sky is blue too, but greyer.
    let blue = count(&|r, g, b| b > 60.0 && b > 2.0 * r && b > 2.0 * g);
    eprintln!("red (GPU) {red}, blue (CPU) {blue}");
    assert!(red > 1500, "GPU flag drawn: {red} red pixels");
    assert!(
        red as f32 > blue as f32 * 0.6,
        "the GPU flag covers about as much as the CPU one: {red} vs {blue}"
    );
}

#[test]
fn a_field_of_64_flags_runs_in_one_dispatch() {
    let mut app = app();
    let mut first = None;
    for k in 0..64 {
        let (x, z) = ((k % 8) as f64 * 2.0 - 7.0, -((k / 8) as f64) * 2.0 - 3.0);
        let e = app
            .world
            .spawn((
                Transform::from_xyz(x, 2.5, z),
                flag([0xc0201c, 0xe8c040, 0x2040c0, 0xf0f0f0][k % 4], true),
            ))
            .id();
        first.get_or_insert(e);
    }
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 4.0, 8.0).looking_at(DVec3::new(0.0, 2.0, -8.0), Vec3::Y),
    ));
    app.world.insert_resource(GpuClothProbe {
        entity: first,
        ..Default::default()
    });
    app.update_n(120);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let mut bodies = app.world.query::<&SoftBodySim>();
    assert_eq!(
        bodies.iter(&app.world).count(),
        0,
        "no flag runs on the CPU"
    );
    let probe = app.world.resource::<GpuClothProbe>();
    assert_eq!(probe.positions.len(), 160);
    assert!(probe.positions.iter().all(|p| p.is_finite()));
    let renderer = app.world.resource::<Renderer>().describe();
    let state = renderer["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "softbody")
        .unwrap()["state"]
        .clone();
    assert_eq!(state["gpu_cloths"], 64);
    eprintln!(
        "64 flags of 160 particles: GPU pass {:?} ms on {}",
        gpu_ms(&app),
        renderer["adapter"]
    );
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("softbody_gpu_field.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    eprintln!("wrote {}", path.display());
}

#[derive(Resource, Default)]
struct PendingShift(Option<WorldShift>);

fn write_pending_shift(mut pending: ResMut<PendingShift>, mut shifts: MessageWriter<WorldShift>) {
    if let Some(s) = pending.0.take() {
        shifts.write(s);
    }
}

#[test]
fn a_gpu_cloth_follows_a_world_shift() {
    let mut app = app();
    app.init_resource::<PendingShift>()
        .add_systems(PostUpdate, write_pending_shift.in_set(WorldShiftSet::Write));
    // A banner hanging still from its top edge (no wind, heavily damped), so any drift after
    // the shift is the shift's fault.
    let mut banner = flag(0xc0201c, true);
    banner.pins = ClothPins::TopEdge { every: 1 };
    banner.sim.wind_scale = 0.0;
    banner.sim.damping = 2.0;
    let e = app
        .world
        .spawn((Transform::from_xyz(3.0, 4.0, -2.0), banner))
        .id();
    app.world.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(3.0, 3.0, 2.0).looking_at(DVec3::new(3.0, 3.5, -2.0), Vec3::Y),
    ));
    app.world.insert_resource(GpuClothProbe {
        entity: Some(e),
        ..Default::default()
    });
    app.update_n(240);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let before = app.world.resource::<GpuClothProbe>().positions.clone();
    assert_eq!(before.len(), 160);

    // Re-anchor 40 km away and turn the world about the vertical.
    let shift = WorldShift {
        rotation: DQuat::from_rotation_y(0.9),
        translation: DVec3::new(4.0e4, 0.0, -1.5e4),
        ..WorldShift::IDENTITY
    };
    app.world.resource_mut::<PendingShift>().0 = Some(shift);
    // The probe shows the frame before, so two frames to see the shifted one.
    for (frames, tolerance) in [(2, 0.05), (60, 0.1)] {
        app.update_n(frames);
        let after = &app.world.resource::<GpuClothProbe>().positions;
        for (i, (b, a)) in before.iter().zip(after).enumerate() {
            let want = shift.point(*b);
            assert!(
                a.is_finite() && (*a - want).length() < tolerance,
                "particle {i}: {a} {frames} frames after the shift, expected about {want}"
            );
        }
    }
}

#[test]
fn a_gpu_flag_soaks_in_the_rain_like_a_cpu_one() {
    let mut app = app();
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.precipitation_mm_h = 60.0;
        weather.precipitation_kind = engine_world::PrecipitationKind::Rain;
        weather.temperature_c = 15.0;
    }
    let on_gpu = app
        .world
        .spawn((Transform::from_xyz(0.0, 2.5, -3.0), flag(0x2040c0, true)))
        .id();
    let on_cpu = app
        .world
        .spawn((Transform::from_xyz(0.0, 2.5, 3.0), flag(0x2040c0, false)))
        .id();
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(4.0, 3.0, 0.0).looking_at(DVec3::new(0.0, 2.0, 0.0), Vec3::Y),
    ));
    app.world.insert_resource(GpuClothProbe {
        entity: Some(on_gpu),
        ..Default::default()
    });
    app.update_n(120);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let gpu_wet = app.world.resource::<GpuClothProbe>().wet;
    let cpu = app.world.get::<SoftBodySim>(on_cpu).expect("CPU flag");
    let cpu_wet = cpu.wet.iter().sum::<f32>() / cpu.wet.len() as f32;
    eprintln!("wet after 2 s of heavy rain: GPU {gpu_wet:.3}, CPU {cpu_wet:.3}");
    assert!(gpu_wet > 0.3, "the rain soaks it: {gpu_wet:.3}");
    assert!(
        (gpu_wet - cpu_wet).abs() < 0.05,
        "as wet as the CPU flag: GPU {gpu_wet:.3}, CPU {cpu_wet:.3}"
    );
}
