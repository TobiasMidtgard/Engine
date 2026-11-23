//! Rendering and lighting's check of the probes before the ray-traced lighting round turns on
//! (`RenderSettings::ray_probe_check`, ray_probe_check.wgsl): under a canopy, the probe nearest
//! the camera holds the irradiance a brute-force reference of 4,096 rays a direction finds
//! there, within 10%, facing each of the six axis directions. The scene is a forest trail's in
//! miniature: soil underfoot, a roof of planks 6 m up with gaps the sun falls through in
//! stripes, the sky between them. The reference's rays see what a probe ray sees (the ground
//! and the canopy both in the frame's ray scene, each hit lit as a probe ray's is), so the two
//! differ by what the probes leave out: their few directions a frame, their limit on bright
//! rays, their maps' 8 texels a side. Both are means over the same frames: a probe's 64 rays
//! an update leave each update a fifth off here, which says nothing of whether it is right on
//! the whole; how far it strays from its mean at the game's own blend (its flicker) is printed
//! beside. Needs a GPU with ray queries (lavapipe has them); passes with a note without.

use engine_core::prelude::*;
use engine_render::RenderDisabled;
use engine_render::prelude::*;
use engine_render::settings::trial_cut_more::{RAY_PROBES, RAY_WORLD};

const W: u32 = 160;
const H: u32 = 120;
/// The six axis directions, as the diagnostics name them (+x, -x, +y, -y, +z, -z).
const AXES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];

/// Probe GI through the frame's ray scene (`bits`: `RAY_WORLD`, with `RAY_PROBES` or not), a
/// small grid (8 x 4 x 8 probes 1.5 m apart, a quarter of them a frame without `RAY_PROBES`),
/// quick to settle.
fn app(bits: u32) -> App {
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
        s.anti_aliasing = AntiAliasing::None;
        s.screen_space_gi = false;
        s.screen_space_reflections = false;
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probe_spacing = 1.5;
        s.gi_probe_grid = [8, 4, 8];
        s.gi_probes_per_frame = 64;
        s.gi_hysteresis = 0.5;
        s.trial_cuts_more |= bits;
    }
    app
}

fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

/// Whether there is a GPU that can hold the ray scene; says why not.
fn can_run(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return false;
    }
    if diag(app, "render.ray.supported") != 1.0 {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return false;
    }
    true
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

/// The trail: soil underfoot, a roof of planks 2.5 m wide 6 m up with 1.5 m gaps between them,
/// 40 m across, the sun 50 degrees up across the planks, the camera standing under the roof.
fn trail(w: &mut World) {
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.25, 0.2, 0.15).with_roughness(0.95),
        ),
    ));
    let leaves = StandardMaterial::color(0.08, 0.14, 0.05).with_roughness(0.9);
    for i in -5..=5 {
        slab(
            w,
            DVec3::new(i as f64 * 4.0, 6.15, 0.0),
            DVec3::new(2.5, 0.3, 40.0),
            leaves,
        );
    }
    let elevation = 50f64.to_radians();
    w.spawn((
        DirectionalLight {
            illuminance: 110_000.0,
            ..Default::default()
        },
        Transform::default()
            .looking_at(DVec3::new(elevation.cos(), -elevation.sin(), 0.3), Vec3::Y),
    ));
    let eye = DVec3::new(0.4, 1.6, 0.3);
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.0 },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(eye).looking_at(eye + DVec3::new(1.0, -0.1, 0.2), Vec3::Y),
    ));
}

/// The trail with the probes settled (24 frames at the quick blend, then `settle` at the
/// game's own, so that the flicker printed is the game's and not what the quick blend left),
/// then checked over `checked` frames.
fn checked(bits: u32, settle: usize, checked: usize) -> Option<App> {
    let mut app = app(bits);
    trail(&mut app.world);
    app.update_n(24);
    if !can_run(&app) {
        return None;
    }
    assert_eq!(diag(&app, "render.ray.on"), 1.0, "no ray scene");
    assert_eq!(
        diag(&app, "render.ray.losses"),
        0.0,
        "the ray scene was lost"
    );
    assert_eq!(
        diag(&app, "render.gi.ray_world"),
        1.0,
        "the probes trace a scene of their own"
    );
    app.world.resource_mut::<RenderSettings>().gi_hysteresis =
        RenderSettings::default().gi_hysteresis;
    app.update_n(settle);
    app.world.resource_mut::<RenderSettings>().ray_probe_check = true;
    app.update_n(checked);
    Some(app)
}

/// Each direction's probe and reference irradiance (lux, means over the same frames), their
/// ratio and the probe's spread from frame to frame, printed.
fn report(app: &App, what: &str) -> [(f64, f64, f64); 6] {
    let mut out = [(0.0, 0.0, 0.0); 6];
    for (k, axis) in AXES.iter().enumerate() {
        out[k] = (
            diag(app, &format!("render.gi.check_probe_{axis}")),
            diag(app, &format!("render.gi.check_ref_{axis}")),
            diag(app, &format!("render.gi.check_ratio_{axis}")),
        );
    }
    eprintln!(
        "{what}: {} frames in the means, probe state {}, probe at ({:.2}, {:.2}, {:.2}) from \
         the camera, worst {:.3}, flicker {:.3}",
        diag(app, "render.gi.check_frames"),
        diag(app, "render.gi.check_state"),
        diag(app, "render.gi.check_dx"),
        diag(app, "render.gi.check_dy"),
        diag(app, "render.gi.check_dz"),
        diag(app, "render.gi.check_worst"),
        diag(app, "render.gi.check_flicker"),
    );
    for (axis, (own, reference, ratio)) in AXES.iter().zip(out) {
        let spread = diag(app, &format!("render.gi.check_spread_{axis}"));
        eprintln!(
            "  {axis}: probe {own:9.1} lux, reference {reference:9.1} lux, ratio {ratio:.3}, \
             spread {spread:.3}"
        );
    }
    out
}

/// Under the canopy, the probe nearest the camera agrees with the brute-force reference within
/// 10% facing every direction whose reference holds a tenth of the brightest's, with the probes
/// traced four times as often (`RAY_PROBES`, as the round turns on: every probe of this grid
/// every frame). 20 frames to settle are two spans of the game's blend; 192 checked frames
/// leave the probe's mean about 2% off from its own noise. Over five windows of 96 frames apart
/// (23 Nov) it read 1 to 2% off on the whole facing five ways and 3% bright facing up (its map
/// is 8 texels a side: the dark straight up under a plank blends with the gaps' light beside
/// it), and one window's worst was 6%. Before its maps had a border it read 8% dark facing -z.
/// The probes at their own rate are the same probes four times as slow, so as many updates
/// take four times the frames: that one is the PC's (`gpu_sweep` row `check_ray_probe_world`).
#[test]
fn a_probe_under_the_canopy_agrees_with_a_brute_force_reference() {
    let what = "probes four times as often";
    let Some(app) = checked(RAY_WORLD | RAY_PROBES, 20, 192) else {
        return;
    };
    let rows = report(&app, what);
    assert!(
        diag(&app, "render.gi.check_frames") >= 60.0,
        "{what}: the reference's readbacks arrived"
    );
    assert_eq!(
        diag(&app, "render.gi.check_state"),
        3.0,
        "{what}: the probe is lit and in use"
    );
    // Downward the probe sees the soil, lit in stripes through the gaps: its bounce, not black
    // (Rendering's first check).
    let (down, down_ref, _) = rows[3];
    assert!(
        down > 0.0 && down_ref > 0.0,
        "{what}: the ground below bounces light ({down:.1} lux, reference {down_ref:.1})"
    );
    let worst = diag(&app, "render.gi.check_worst");
    assert!(
        (0.0..=0.10).contains(&worst),
        "{what}: the probe is {worst:.3} off the reference in its worst direction"
    );
}
