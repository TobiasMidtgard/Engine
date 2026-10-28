//! Probe GI (`RenderSettings::probe_gi`): a closed room keeps the noon sky out, a sunlit patch
//! on the floor lights the room around it, a torch's light bounces off the walls of its room at
//! night, and none of it leaks through a wall into the room next door; meshes far from the probe
//! grid are not built for its rays; an indoor volume's closed faces stop the probes' rays where
//! no mesh does (rock drawn by another feature); a mirror reflects what is behind the camera
//! through world reflections; a still scene isn't gathered again every frame, and a large mesh
//! builds on a worker before the rays see it. Needs a GPU or lavapipe; passes with a note when
//! neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;
/// The room's inside: 6 m square, 3 m high, the floor's top at y = 0.
const HALF: f64 = 3.0;
const HEIGHT: f64 = 3.0;
const THICK: f64 = 0.3;

fn app(probe_gi: bool) -> App {
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
        // Only the probes bounce light here.
        s.screen_space_gi = false;
        s.probe_gi = probe_gi;
        s.gi_probe_spacing = 1.0;
        s.gi_probe_grid = [16, 8, 16];
        s.gi_probes_per_frame = 16 * 8 * 16;
        s.gi_hysteresis = 0.5;
    }
    app
}

fn slab(w: &mut World, center: DVec3, size: DVec3, albedo: f32) {
    w.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(
            Shape::Cuboid {
                size: size.as_vec3().to_array(),
            },
            StandardMaterial::color(albedo, albedo, albedo).with_roughness(0.9),
        ),
    ));
}

/// A closed room centered on `at` (its floor's top), its walls `albedo`; `hole`: a square
/// opening of that size in the roof, its middle above `at + (0, 0, hole_z)`; `floor`: the floor's
/// albedo.
fn room(w: &mut World, at: DVec3, albedo: f32, floor: f32, hole: Option<(f64, f64)>) {
    let (h, t) = (HALF, THICK);
    let outer = 2.0 * (h + t);
    slab(
        w,
        at + DVec3::new(0.0, -t * 0.5, 0.0),
        DVec3::new(outer, t, outer),
        floor,
    );
    for (x, z, sx, sz) in [
        (h + t * 0.5, 0.0, t, outer),
        (-h - t * 0.5, 0.0, t, outer),
        (0.0, h + t * 0.5, outer, t),
        (0.0, -h - t * 0.5, outer, t),
    ] {
        slab(
            w,
            at + DVec3::new(x, HEIGHT * 0.5, z),
            DVec3::new(sx, HEIGHT, sz),
            albedo,
        );
    }
    let roof_y = HEIGHT + t * 0.5;
    match hole {
        None => slab(
            w,
            at + DVec3::new(0.0, roof_y, 0.0),
            DVec3::new(outer, t, outer),
            albedo,
        ),
        Some((size, z)) => {
            // Four slabs around the opening.
            let (lo, hi) = (z - size * 0.5, z + size * 0.5);
            let edge = h + t;
            slab(
                w,
                at + DVec3::new(0.0, roof_y, (lo - edge) * 0.5),
                DVec3::new(outer, t, lo + edge),
                albedo,
            );
            slab(
                w,
                at + DVec3::new(0.0, roof_y, (hi + edge) * 0.5),
                DVec3::new(outer, t, edge - hi),
                albedo,
            );
            for side in [-1.0, 1.0] {
                slab(
                    w,
                    at + DVec3::new(side * (size * 0.5 + edge) * 0.5, roof_y, z),
                    DVec3::new(edge - size * 0.5, t, size),
                    albedo,
                );
            }
        }
    }
}

fn camera(w: &mut World, eye: DVec3, target: DVec3, ev100: f32) {
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
}

fn sun(w: &mut World, illuminance: f32, toward: DVec3) {
    w.spawn((
        DirectionalLight {
            illuminance,
            ..Default::default()
        },
        Transform::default().looking_at(toward, Vec3::Y),
    ));
}

fn ground(w: &mut World) {
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, -THICK, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
}

/// Mean brightness (0..255, linear-ish without a tonemapper) of the image's rectangle
/// `(x0, y0, x1, y1)` in fractions of its size, after `frames` frames; `None` without a GPU.
fn shoot(mut app: App, frames: usize, name: &str, rect: [f32; 4]) -> Option<f32> {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-probe-gi");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    let image = image::open(&path).expect("read the shot").to_rgb8();
    let (x0, y0) = ((rect[0] * W as f32) as u32, (rect[1] * H as f32) as u32);
    let (x1, y1) = ((rect[2] * W as f32) as u32, (rect[3] * H as f32) as u32);
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = image.get_pixel(x, y);
            sum += (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
        }
    }
    let d = app.world.resource::<Diagnostics>();
    eprintln!(
        "{name}: {:.1} (probes {:?}, instances {:?})",
        sum / ((x1 - x0) * (y1 - y0)) as f32,
        d.get("render.gi.probes_updated"),
        d.get("render.gi.instances")
    );
    Some(sum / ((x1 - x0) * (y1 - y0)) as f32)
}

/// Looking at the far wall and the ceiling from near the back wall.
const EYE: DVec3 = DVec3::new(-0.5, 1.4, 2.4);
const TARGET: DVec3 = DVec3::new(0.3, 1.9, -3.0);
/// The far wall and ceiling, clear of the floor.
const UPPER: [f32; 4] = [0.15, 0.1, 0.85, 0.5];

#[test]
fn a_closed_room_keeps_the_noon_sky_out() {
    let scene = |gi: bool| {
        let mut app = app(gi);
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, 0.5, None);
        ground(w);
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
        camera(w, EYE, TARGET, 12.0);
        app
    };
    let Some(off) = shoot(scene(false), 6, "closed_off.png", UPPER) else {
        return;
    };
    let on = shoot(scene(true), 12, "closed_on.png", UPPER).expect("GPU");
    // Without the probes the walls see the sky through the guess; with them, only the fill.
    assert!(
        on < off * 0.4,
        "closed room: {on:.1} with probes, {off:.1} without"
    );
    assert!(on < 25.0, "closed room with probes: {on:.1}");
}

#[test]
fn a_sunlit_patch_on_the_floor_lights_the_room() {
    let scene = |floor: f32| {
        let mut app = app(true);
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, floor, Some((1.4, -0.5)));
        ground(w);
        // Nearly overhead, so the patch falls on the floor below the opening.
        sun(w, 110_000.0, DVec3::new(0.05, -1.0, -0.1));
        camera(w, EYE, TARGET, 12.0);
        app
    };
    let Some(dark) = shoot(scene(0.03), 16, "patch_dark_floor.png", UPPER) else {
        return;
    };
    let white = shoot(scene(0.8), 16, "patch_white_floor.png", UPPER).expect("GPU");
    // The walls and ceiling see the sunlit floor through the probes.
    assert!(
        white > dark * 1.5 + 4.0,
        "walls over a white floor {white:.1}, over a black one {dark:.1}"
    );
}

/// A lamp in the middle of room A at night; room B shares A's +X wall.
fn night(gi: bool, look_into_b: bool) -> App {
    let mut app = app(gi);
    let w = &mut app.world;
    room(w, DVec3::ZERO, 0.7, 0.6, None);
    let b = DVec3::X * (2.0 * HALF + THICK);
    room(w, b, 0.7, 0.6, None);
    ground(w);
    sun(w, 0.05, DVec3::new(0.3, -1.0, -0.4));
    w.spawn((
        Transform::from_translation(DVec3::new(-0.5, 1.2, -0.5)),
        PointLight {
            color: [1.0, 0.7, 0.4],
            intensity_lm: 2000.0,
            // Directly it reaches only the walls of its own room.
            range: 3.4,
            radius: 0.05,
        },
    ));
    let at = if look_into_b { b } else { DVec3::ZERO };
    camera(w, at + EYE, at + TARGET, 2.0);
    app
}

#[test]
fn a_lamp_lights_its_room_by_bounce_and_nothing_leaks_next_door() {
    let Some(a_off) = shoot(night(false, false), 6, "night_a_off.png", UPPER) else {
        return;
    };
    let a_on = shoot(night(true, false), 16, "night_a_on.png", UPPER).expect("GPU");
    let b_off = shoot(night(false, true), 6, "night_b_off.png", UPPER).expect("GPU");
    let b_on = shoot(night(true, true), 16, "night_b_on.png", UPPER).expect("GPU");
    assert!(
        a_on > a_off * 1.2 + 3.0,
        "the lamp's room: {a_on:.1} with probes, {a_off:.1} without"
    );
    assert!(
        b_on < b_off + 3.0 && b_on < a_on * 0.2,
        "the room next door: {b_on:.1} with probes, {b_off:.1} without (the lamp's {a_on:.1})"
    );
}

#[test]
fn meshes_far_from_the_grid_are_not_built_for_its_rays() {
    let meshes = |far: bool| {
        let mut app = app(true);
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, 0.5, None);
        ground(w);
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
        camera(w, EYE, TARGET, 12.0);
        if far {
            // Its own mesh (no other cuboid has this size), 2 km away.
            slab(w, DVec3::new(2000.0, 4.0, 0.0), DVec3::splat(7.7), 0.5);
        }
        app.update_n(4);
        if app.world.get_resource::<RenderDisabled>().is_some() {
            return None;
        }
        app.world.resource::<Diagnostics>().get("render.gi.meshes")
    };
    let Some(near) = meshes(false) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let with_far = meshes(true).expect("GPU");
    eprintln!("meshes built: {near} near the room, {with_far} with a far one");
    assert!(near > 0.0);
    assert_eq!(with_far, near, "the far mesh was built");
}

#[test]
fn an_indoor_volume_keeps_the_sky_out_where_no_mesh_roofs_the_room() {
    // A room whose roof the probes can't see (a cave's rock drawn by another feature): open to
    // the sky for their rays unless a `SkyLightVolume` closes it.
    let scene = |volume: bool| {
        let mut app = app(true);
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, 0.5, Some((2.0 * HALF, 0.0)));
        ground(w);
        // Its light falls away from the far wall's inner face.
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, 0.4));
        if volume {
            w.spawn((
                Transform::from_xyz(0.0, HEIGHT * 0.5, 0.0),
                SkyLightVolume::new([HALF as f32, HEIGHT as f32 * 0.5 + 0.2, HALF as f32])
                    .open(BoxFace::NegZ),
            ));
        }
        camera(w, EYE, DVec3::new(0.3, 1.0, -3.0), 12.0);
        app
    };
    // The far wall, below the sky over the open top.
    const WALL: [f32; 4] = [0.2, 0.35, 0.8, 0.65];
    let Some(open) = shoot(scene(false), 16, "roofless_open.png", WALL) else {
        return;
    };
    let closed = shoot(scene(true), 16, "roofless_volume.png", WALL).expect("GPU");
    assert!(
        closed < open * 0.5,
        "far wall: {closed:.1} inside the volume, {open:.1} without it"
    );
}

#[test]
fn a_volume_lets_the_sky_in_only_through_its_opening() {
    // The same roofless room, its volume open at the top: through the whole top, or only
    // through a skylight a tenth of its width (a cave mouth in a wide rock face).
    let scene = |opening: bool| {
        let mut app = app(true);
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, 0.5, Some((2.0 * HALF, 0.0)));
        ground(w);
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, 0.4));
        let mut volume = SkyLightVolume::new([HALF as f32, HEIGHT as f32 * 0.5 + 0.2, HALF as f32])
            .open(BoxFace::PosY);
        if opening {
            let r = HALF as f32 * 0.1;
            volume = volume.with_opening([-r, -r], [r, r]);
        }
        w.spawn((Transform::from_xyz(0.0, HEIGHT * 0.5, 0.0), volume));
        camera(w, EYE, DVec3::new(0.3, 1.0, -3.0), 12.0);
        app
    };
    const WALL: [f32; 4] = [0.2, 0.35, 0.8, 0.65];
    let Some(whole) = shoot(scene(false), 16, "skylight_whole.png", WALL) else {
        return;
    };
    let small = shoot(scene(true), 16, "skylight_small.png", WALL).expect("GPU");
    assert!(
        small < whole * 0.5,
        "far wall: {small:.1} through a skylight, {whole:.1} through the whole top"
    );
}

#[test]
fn probes_outside_a_closed_volume_do_not_light_its_walls() {
    // The roofless room under its closed volume: the probes above it, outside the volume, see
    // the open sky. The upper walls, between them and the probes inside, must not take their
    // light.
    let scene = |volume: bool| {
        let mut app = app(true);
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, 0.5, Some((2.0 * HALF, 0.0)));
        ground(w);
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, 0.4));
        if volume {
            w.spawn((
                Transform::from_xyz(0.0, HEIGHT * 0.5, 0.0),
                SkyLightVolume::new([HALF as f32, HEIGHT as f32 * 0.5 + 0.2, HALF as f32]),
            ));
        }
        camera(w, EYE, TARGET, 12.0);
        app
    };
    // The far wall's upper part, in its shade.
    const FAR_UPPER: [f32; 4] = [0.15, 0.35, 0.8, 0.6];
    let Some(open) = shoot(scene(false), 16, "walled_open.png", FAR_UPPER) else {
        return;
    };
    let closed = shoot(scene(true), 16, "walled_closed.png", FAR_UPPER).expect("GPU");
    assert!(
        closed < open * 0.1,
        "upper walls: {closed:.1} inside the closed volume, {open:.1} without it"
    );
}

/// A mirror facing the camera reflects a sunlit red wall behind the camera: off screen, so
/// screen-space reflections can't see it, but world reflections trace it in probe GI's scene.
#[test]
fn a_mirror_reflects_a_wall_behind_the_camera() {
    let scene = |world_reflections: bool| {
        let mut app = app(true);
        app.world.resource_mut::<RenderSettings>().world_reflections = world_reflections;
        let w = &mut app.world;
        ground(w);
        w.spawn((
            Transform::from_translation(DVec3::new(0.0, 1.5, -3.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [5.0, 3.0, 0.1],
                },
                StandardMaterial::color(0.95, 0.95, 0.95)
                    .with_metallic(1.0)
                    .with_roughness(0.05),
            ),
        ));
        w.spawn((
            Transform::from_translation(DVec3::new(0.0, 2.0, 4.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [8.0, 4.0, 0.3],
                },
                StandardMaterial::color(0.8, 0.05, 0.05).with_roughness(0.9),
            ),
        ));
        // From behind the mirror, onto the red wall's face.
        sun(w, 110_000.0, DVec3::new(0.2, -0.8, 0.6));
        camera(
            w,
            DVec3::new(0.0, 1.5, 1.0),
            DVec3::new(0.0, 1.5, -3.0),
            14.0,
        );
        app
    };
    let Some(off) = red_share(scene(false), "mirror_off.png") else {
        return;
    };
    let on = red_share(scene(true), "mirror_on.png").expect("GPU");
    assert!(
        on > 0.25 && on > off + 0.2,
        "mirror's red share: {on:.2} with world reflections, {off:.2} without"
    );
}

/// How much redder than its other channels the middle of the picture is (0 = gray, 1 = pure
/// red), after 12 frames; `None` without a GPU.
fn red_share(mut app: App, name: &str) -> Option<f32> {
    app.update_n(12);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-probe-gi");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    let image = image::open(&path).expect("read the shot").to_rgb8();
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for y in H * 3 / 10..H * 7 / 10 {
        for x in W * 3 / 10..W * 7 / 10 {
            let p = image.get_pixel(x, y);
            for c in 0..3 {
                sum[c] += p[c] as f32;
            }
            n += 1.0;
        }
    }
    let [r, g, b] = sum.map(|c| c / n);
    let share = (r - 0.5 * (g + b)) / r.max(1.0);
    eprintln!("{name}: rgb {r:.1} {g:.1} {b:.1}, red share {share:.2}");
    Some(share)
}

/// The instance gather runs again only when the scene changes: a still room is gathered in the
/// first frames and then left alone, and a wall that moves is gathered again the next frame.
#[test]
fn a_still_scene_is_not_gathered_again_every_frame() {
    let mut app = app(true);
    let w = &mut app.world;
    room(w, DVec3::ZERO, 0.7, 0.5, None);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(w, EYE, TARGET, 12.0);
    let block = w
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 0.5, 0.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.5, 1.0, 0.5],
                },
                StandardMaterial::color(0.5, 0.5, 0.5),
            ),
        ))
        .id();
    app.update_n(6);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    let gathered = |app: &App| {
        let d = app.world.resource::<Diagnostics>();
        (
            d.get("render.gi.gathered").unwrap_or(-1.0),
            d.get("render.gi.instances").unwrap_or(-1.0),
        )
    };
    let still = gathered(&app);
    app.world
        .get_mut::<Transform>(block)
        .expect("block")
        .translation
        .x = 1.0;
    app.update();
    let moved = gathered(&app);
    eprintln!("still: {still:?}, after a move: {moved:?}");
    assert_eq!(
        still,
        (0.0, 7.0),
        "a still room's gather (gathered, instances)"
    );
    assert_eq!(moved, (1.0, 7.0), "after the block moved");
}

/// A mesh too large to build on the render thread builds on a worker, and the rays see it once
/// it's done: a closed sphere of 66k triangles around the camera keeps most of the noon sky out.
#[test]
fn a_large_mesh_builds_on_a_worker_and_then_blocks_the_sky() {
    let scene = |gi: bool| {
        let mut app = app(gi);
        let w = &mut app.world;
        w.spawn((
            Transform::from_translation(DVec3::new(0.0, 1.5, 0.0)),
            MeshRenderer::new(
                // Inside the probe grid (8 m high around the camera).
                Shape::Sphere {
                    radius: 3.0,
                    segments: 256,
                },
                StandardMaterial {
                    double_sided: true,
                    ..StandardMaterial::color(0.7, 0.7, 0.7).with_roughness(0.9)
                },
            ),
        ));
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
        camera(w, EYE, TARGET, 12.0);
        app
    };
    let mut app = scene(true);
    app.update();
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    let diag = |app: &App, name: &str| {
        app.world
            .resource::<Diagnostics>()
            .get(name)
            .unwrap_or(-1.0)
    };
    let building = diag(&app, "render.gi.building");
    let mut seen_after = None;
    for frame in 1..200 {
        if diag(&app, "render.gi.instances") == 1.0 {
            seen_after = Some(frame);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.update();
    }
    eprintln!("building at first: {building}, in the rays after {seen_after:?} frames");
    assert_eq!(building, 1.0, "the sphere builds on a worker");
    assert!(seen_after.is_some(), "the sphere never reached the rays");
    assert_eq!(diag(&app, "render.gi.building"), 0.0);
    let on = shoot(app, 12, "large_sphere.png", UPPER).expect("GPU");
    let off = shoot(scene(false), 6, "large_sphere_off.png", UPPER).expect("GPU");
    // Without the probes the inside sees the sky through the guess.
    assert!(
        on < off * 0.7,
        "inside the sphere: {on:.1}, {off:.1} without probes"
    );
}
