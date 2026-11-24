//! The ray-traced lighting round's switches on a GPU without ray queries
//! (`ENGINE_RAY_QUERIES=off` leaves the device without them, as a GPU that has none is): the
//! frame's ray scene (`trial_cut_more::RAY_WORLD`) is not made, nothing prepares for it, the
//! sun's measuring rays are not shot nor its trace (`trial_cut_more::RAY_SUN`; its mark of
//! what sways is still written, and every other reader takes it as before), so the light pass
//! joins nothing with the shadow maps (`shade_skip::RAY_SUN_TRACE_ONLY` or not), the probes
//! are not traced more often nor put to sleep when stuck (`trial_cut_more::RAY_PROBES`), no
//! pixel's reflection is traced (`trial_cut_more::RAY_REFLECTIONS`, with
//! `shade_skip::RAY_REFLECT_RAYS_ONLY` or not: screen-space reflections over world reflections
//! draw the mirror as before), no ground colour map is made or drawn
//! (`trial_cut_more::RAY_GROUND_MAP`, with `RenderSettings::ray_ground_every_frame` or not),
//! and the rays walk the probes' own scene in software, so the picture is the one drawn with
//! the switches off. A test binary of its own: the variable is the whole process's, read when the device is
//! made. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::trial_cut_more::{
    RAY_GROUND_MAP, RAY_PROBES, RAY_REFLECTIONS, RAY_SUN, RAY_WORLD,
};
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;

fn app(switches: bool) -> App {
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
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.screen_space_gi = false;
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probe_spacing = 1.0;
        s.gi_probe_grid = [16, 8, 16];
        // A quarter of the grid a frame: four times as many would show.
        s.gi_probes_per_frame = 16 * 8 * 16 / 4;
        s.gi_hysteresis = 0.5;
        if switches {
            s.trial_cuts_more |=
                RAY_WORLD | RAY_PROBES | RAY_SUN | RAY_REFLECTIONS | RAY_GROUND_MAP;
            s.ray_sun_shadows = true;
            s.ray_ground_every_frame = true;
            s.debug_skip |= engine_render::settings::shade_skip::RAY_LEAVES_SOLID
                | engine_render::settings::shade_skip::COUNT_MESH_RUNS
                | engine_render::settings::shade_skip::RAY_SUN_TRACE_ONLY
                | engine_render::settings::shade_skip::RAY_REFLECT_RAYS_ONLY;
            s.ray_caster_reach = 40.0;
            s.ray_far_reach = 90.0;
            s.ray_sun_reach = 60.0;
        }
    }
    app
}

fn slab(w: &mut World, center: DVec3, size: DVec3, material: StandardMaterial) {
    w.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(
            Shape::Cuboid {
                size: size.as_vec3().to_array(),
            },
            material,
        ),
    ));
}

/// A sunlit yard: the ground, a mirror facing the camera, a red wall behind the camera that
/// only world reflections show in the mirror, and a block whose shadow and bounce the probes
/// carry.
fn yard(w: &mut World) {
    let gray = |albedo: f32| StandardMaterial::color(albedo, albedo, albedo).with_roughness(0.9);
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, -0.3, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
    slab(
        w,
        DVec3::new(0.0, 1.5, -3.0),
        DVec3::new(5.0, 3.0, 0.1),
        StandardMaterial::color(0.95, 0.95, 0.95)
            .with_metallic(1.0)
            .with_roughness(0.05),
    );
    slab(
        w,
        DVec3::new(0.0, 2.0, 4.0),
        DVec3::new(8.0, 4.0, 0.3),
        StandardMaterial::color(0.8, 0.05, 0.05).with_roughness(0.9),
    );
    slab(
        w,
        DVec3::new(-1.6, 0.5, -1.0),
        DVec3::new(1.0, 1.6, 1.0),
        gray(0.7),
    );
    w.spawn((
        DirectionalLight {
            illuminance: 110_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.2, -0.8, 0.6), Vec3::Y),
    ));
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(DVec3::new(0.0, 1.5, 1.0))
            .looking_at(DVec3::new(0.0, 1.5, -3.0), Vec3::Y),
    ));
}

fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

/// The picture after 16 frames; `None` without a GPU.
fn picture(app: &mut App, name: &str) -> Option<image::RgbImage> {
    app.update_n(16);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-ray-off");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    Some(image::open(&path).expect("read the shot").to_rgb8())
}

#[test]
fn without_ray_queries_the_switches_change_nothing() {
    // SAFETY: the only test of this binary, before any other thread exists.
    unsafe { std::env::set_var("ENGINE_RAY_QUERIES", "off") };
    let mut plain = app(false);
    yard(&mut plain.world);
    let Some(before) = picture(&mut plain, "plain.png") else {
        return;
    };
    let mut switched = app(true);
    yard(&mut switched.world);
    let after = picture(&mut switched, "switched.png").expect("GPU");
    for (app, what) in [(&plain, "switches off"), (&switched, "switches on")] {
        for (name, expected) in [
            ("render.ray_queries", 0.0),
            ("render.ray.supported", 0.0),
            ("render.ray.on", 0.0),
            ("render.ray.losses", 0.0),
            ("render.gi.on", 1.0),
            ("render.gi.hardware_rays", 0.0),
            ("render.gi.ray_world", 0.0),
            ("render.gi.probes_raised", 0.0),
            ("render.gi.probes_updated", 512.0),
            ("render.gi.probes_budget_cut", 0.0),
            // Never published: the meshes prepare nothing for a scene that is not made, no
            // ray is shot toward the sun, no pixel's reflection is traced, and no ground
            // colour map is drawn.
            ("render.meshes.ray_looked", -1.0),
            ("render.ray.instances", -1.0),
            ("render.ray.sun_rays", -1.0),
            ("render.ray.sun_missed", -1.0),
            ("render.ray.sun_false", -1.0),
            ("render.ray.sun_joined", -1.0),
            ("render.ray.sun_history", -1.0),
            ("render.gi.sun_rays", -1.0),
            ("render.ray.reflect_full", -1.0),
            ("render.ray.reflect_pixels", -1.0),
            ("render.ray.reflect_screen", -1.0),
            ("render.ray.reflect_rays", -1.0),
            ("render.ray.reflect_reused", -1.0),
            ("render.ray.ground_map", -1.0),
            ("render.ray.ground_draws", -1.0),
            ("render.ray.ground_drawers", -1.0),
            ("render.ray.ground_mb", -1.0),
            ("render.cpu.ray ground map", -1.0),
            ("render.gpu.ray ground map", -1.0),
        ] {
            assert_eq!(diag(app, name), expected, "{what}: {name}");
        }
    }
    // The software walk counts what its update did too (which quarter of the grid a count
    // read back covers depends on when it arrived, so the two runs' counts aren't compared).
    for (app, what) in [(&plain, "switches off"), (&switched, "switches on")] {
        assert!(
            diag(app, "render.gi.probes_awake") > 0.0,
            "{what}: the probes' update is counted"
        );
    }
    // Two runs of one scene differ by the output's dither alone (post.wgsl moves each pixel
    // a level either way, seeded by the clock).
    let (mut over, mut most, mut sum) = (0u32, 0u8, 0u64);
    for (a, b) in before.pixels().zip(after.pixels()) {
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        over += (d > 2) as u32;
        most = most.max(d);
        sum += d as u64;
    }
    let mean = sum as f32 / (W * H) as f32;
    eprintln!("{over} pixels apart by more than the dither, at most {most}, mean {mean:.3}");
    assert!(
        over <= W * H / 1000 && most <= 8 && mean <= 1.0,
        "the switches changed the picture: {over} pixels apart by more than the dither, at \
         most {most}, mean {mean:.3}"
    );
    // And it is a picture: the mirror shows the red wall.
    let red = {
        let mut sum = [0.0f32; 3];
        for y in H * 3 / 10..H * 7 / 10 {
            for x in W * 3 / 10..W * 7 / 10 {
                let p = after.get_pixel(x, y);
                for c in 0..3 {
                    sum[c] += p[c] as f32;
                }
            }
        }
        (sum[0] - 0.5 * (sum[1] + sum[2])) / sum[0].max(1.0)
    };
    assert!(red > 0.25, "the mirror's red share: {red:.2}");
}
