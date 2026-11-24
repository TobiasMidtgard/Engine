//! The terrain's part of the frame's ray scene (`TerrainFeature::ray_scene`, the ray-traced
//! lighting round's step 5): the patches of ground around the camera, whichever way it looks,
//! are ground in the rays, built straight from the terrain's own pages, and they go with the
//! camera when it goes elsewhere; and with the ground colour map on (step 11,
//! `GroundMapDraw`), a ray that hits the ground takes the ground's own colour. Needs a GPU
//! with ray queries (lavapipe has them); passes with a note without.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::trial_cut_more::{RAY_GROUND_MAP, RAY_WORLD, RAYS};
use engine_render::{RenderDisabled, capture_now};
use engine_terrain::{FlatPad, PlanetTerrain, TerrainPlugin};

fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 96;
        w.height = 64;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(TerrainPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probes_per_frame = 16;
        // The frame's ray scene and no other part of the ray-traced lighting, whatever the
        // defaults hold.
        s.trial_cuts_more = (s.trial_cuts_more & !RAYS) | RAY_WORLD;
    }
    // A flat square 4 km across, its pad level at the origin.
    let pad = FlatPad::default();
    let terrain = PlanetTerrain::flat(4096.0, 3).with_pad(pad);
    let at = terrain.home_transform(pad);
    app.world.spawn((at, terrain));
    app.world.spawn((
        DirectionalLight {
            illuminance: 50_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    app
}

/// Frames until every patch the rays' walk asks for is resident, none is on its way, and the
/// terrain's choice of patches for the rays and the ground in the rays agree, for 5 frames
/// running (the patches stream in, coarse ones first, then their structures are built a share
/// a frame): the patches chosen, and the frames it took.
fn settle(app: &mut App, what: &str) -> (f64, u32) {
    let mut held = 0;
    for frame in 1..=2000 {
        app.update();
        let chosen = diag(app, "terrain.ray_patches");
        let placed = diag(app, "render.ray.class_ground");
        let streaming = diag(app, "terrain.ray_patches_missing") != 0.0
            || diag(app, "terrain.patches_building") != 0.0;
        if !streaming && chosen > 0.0 && placed == chosen.min(512.0) {
            held += 1;
            if held == 5 {
                return (chosen, frame);
            }
        } else {
            held = 0;
        }
    }
    panic!(
        "{what}: the rays' ground never settled ({} chosen, {} placed, {} missing, {} building)",
        diag(app, "terrain.ray_patches"),
        diag(app, "render.ray.class_ground"),
        diag(app, "terrain.ray_patches_missing"),
        diag(app, "terrain.patches_building")
    );
}

fn gpu_errors() -> Vec<String> {
    engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect()
}

#[test]
fn the_ground_around_the_camera_is_in_the_rays() {
    let mut app = app();
    let look = |from: DVec3, toward: DVec3| {
        Transform::from_translation(from).looking_at(from + toward, Vec3::Y)
    };
    let north = DVec3::new(0.0, -0.1, -1.0);
    let camera = app
        .world
        .spawn((Camera::default(), look(DVec3::new(0.0, 2.0, 0.0), north)))
        .id();
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    if diag(&app, "render.ray.supported") != 1.0 {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    assert_eq!(diag(&app, "render.ray.on"), 1.0, "no ray scene");
    let (ahead, frames) = settle(&mut app, "looking north");
    eprintln!(
        "looking north: {ahead} patches in the rays after {frames} frames ({} resident, {} \
         drawn, {} triangles in the rays)",
        diag(&app, "terrain.patches_resident"),
        diag(&app, "terrain.patches_drawn"),
        diag(&app, "render.ray.triangles")
    );
    // A few hundred patches: 16 m ones near the camera at the view's own detail, coarser ones
    // farther off, and past 144 m none smaller than a third of its distance, out to the far
    // reach (384 m).
    assert!(
        (50.0..=512.0).contains(&ahead),
        "{ahead} patches around the camera"
    );

    // The ground behind the camera is in the rays as much as the ground ahead.
    let south = DVec3::new(0.0, -0.1, 1.0);
    *app.world.get_mut::<Transform>(camera).unwrap() = look(DVec3::new(0.0, 2.0, 0.0), south);
    let (behind, frames) = settle(&mut app, "looking south");
    eprintln!("looking south: {behind} patches after {frames} frames");
    assert!(
        (behind - ahead).abs() <= 0.2 * ahead,
        "{behind} patches looking south, {ahead} looking north"
    );

    // Off 1.5 km: the patches left behind leave the rays, those around the camera come in.
    *app.world.get_mut::<Transform>(camera).unwrap() = look(DVec3::new(1500.0, 2.0, 0.0), north);
    let (there, frames) = settle(&mut app, "1.5 km off");
    eprintln!("1.5 km off: {there} patches after {frames} frames");
    assert!(
        (behind - there).abs() <= 0.2 * behind,
        "{there} patches 1.5 km off, {behind} before"
    );
    let errors = gpu_errors();
    assert!(errors.is_empty(), "GPU errors: {errors:#?}");
}

/// The meadow of [`the_rays_take_the_meadows_own_colour_from_the_ground_map`]: a green flat
/// ground under a high sun, the air's ground colour gray, a white wall standing on it that the
/// sun does not light, seen from 4 m (exposed so nothing clips); only the probes bounce light.
/// `map`: the ground colour map on.
fn meadow(map: bool) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 96;
        w.height = 64;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(TerrainPlugin);
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
        s.gi_probes_per_frame = 16 * 8 * 16;
        s.gi_hysteresis = 0.5;
        // The frame's ray scene, with the ground colour map or without, and no other part of
        // the ray-traced lighting, whatever the defaults hold.
        s.trial_cuts_more = (s.trial_cuts_more & !RAYS) | RAY_WORLD;
        if map {
            s.trial_cuts_more |= RAY_GROUND_MAP;
        }
    }
    app.world.resource_mut::<Atmosphere>().ground_color = [0.25; 3];
    let pad = FlatPad::default();
    let mut terrain = PlanetTerrain::flat(4096.0, 3).with_pad(pad);
    let green = [0.04, 0.5, 0.04];
    terrain.palette.lush = green;
    terrain.palette.dry = green;
    terrain.palette.soil = green;
    let at = terrain.home_transform(pad);
    app.world.spawn((at, terrain));
    app.world.spawn((
        DirectionalLight {
            illuminance: 110_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.1, -1.0, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 1.0, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 2.0, 0.2],
            },
            StandardMaterial::color(0.8, 0.8, 0.8).with_roughness(0.9),
        ),
    ));
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 15.0 },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(DVec3::new(0.0, 1.0, -4.0))
            .looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    app
}

/// The picture as the scene stands now (also copied to `ENGINE_SHOTS`, when set, as `name`).
fn picture(app: &mut App, name: &str) -> image::RgbImage {
    let dir = std::env::temp_dir().join("engine-terrain-rays");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    image::open(&path).expect("read the shot").to_rgb8()
}

/// How much greener than gray the middle of the picture is (the wall, 20 % of it each way):
/// green over the mean of red and blue, less 1.
fn green_share(image: &image::RgbImage) -> f32 {
    let (w, h) = image.dimensions();
    let mut s = [0.0f32; 3];
    for y in h * 2 / 5..h * 3 / 5 {
        for x in w * 2 / 5..w * 3 / 5 {
            for (sum, v) in s.iter_mut().zip(image.get_pixel(x, y).0) {
                *sum += v as f32;
            }
        }
    }
    s[1] / (0.5 * (s[0] + s[2])).max(1.0) - 1.0
}

/// `render.ray.ground_map`, `ground_draws`, `ground_drawers`.
fn map_rows(app: &App) -> [f64; 3] {
    ["map", "draws", "drawers"].map(|row| diag(app, &format!("render.ray.ground_{row}")))
}

/// The ground colour map (`GroundMapDraw`, step 11 of the ray-traced lighting round): with it
/// on, the terrain draws its own colour into the map, and a probe ray that hits the ground
/// takes that colour in place of the air's one gray. The green meadow then lights the white
/// wall standing on it green, where the gray did not. The map is drawn again only when
/// something in it changes: not while the camera holds still.
#[test]
fn the_rays_take_the_meadows_own_colour_from_the_ground_map() {
    let mut shares = Vec::new();
    for map in [false, true] {
        let name = if map { "map on" } else { "map off" };
        let mut app = meadow(map);
        app.update_n(3);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        if diag(&app, "render.ray.supported") != 1.0 {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return;
        }
        let (patches, frames) = settle(&mut app, name);
        // (The probes take a few updates to settle.)
        app.update_n(20);
        let [read, draws, drawers] = map_rows(&app);
        eprintln!(
            "{name}: {patches} patches in the rays after {frames} frames; the map read {read}, \
             drawn {draws} times by {drawers}, from {} patches",
            diag(&app, "terrain.ground_map_patches")
        );
        if map {
            assert_eq!(read, 1.0, "{name}: the rays read the map");
            assert_eq!(drawers, 1.0, "{name}: the terrain draws it");
            assert!(draws >= 1.0, "{name}: drawn");
            assert!(
                diag(&app, "terrain.ground_map_patches") > 0.0,
                "{name}: patches to draw"
            );
            // Nothing changes while the camera holds still: the map is not drawn again.
            app.update_n(24);
            assert_eq!(
                diag(&app, "render.ray.ground_draws"),
                draws,
                "{name}: drawn again while nothing changed"
            );
        } else {
            assert_eq!([read, draws, drawers], [0.0; 3], "{name}: no map");
        }
        let file = format!("terrain_ground_map_{}.png", if map { "on" } else { "off" });
        shares.push(green_share(&picture(&mut app, &file)));
        let errors = gpu_errors();
        assert!(errors.is_empty(), "{name}: GPU errors: {errors:#?}");
    }
    eprintln!("the wall's green share: {shares:?} (map off, on)");
    assert!(
        shares[1] > shares[0] + 0.05,
        "the meadow lights the wall green with the map: {:.3} against {:.3} without",
        shares[1],
        shares[0]
    );
}
