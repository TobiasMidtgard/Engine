//! Grass on screen: a meadow covers the ground with green blades that thin out into the
//! distance and move in the wind. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::{shade_skip, trial_cut};
use engine_render::{DebugView, GrassField, GrassStyle, RenderDisabled, Renderer, capture_now};
use engine_world::WeatherState;
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A flat meadow on brown soil seen from eye height, sun low ahead and to the left.
fn meadow_app(wind: Vec3) -> App {
    meadow_app_with(wind, |_| {})
}

fn meadow_app_with(wind: Vec3, configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.vignette = 0.0;
        configure(&mut s);
    }
    app.world.insert_resource(WeatherState {
        wind,
        ..Default::default()
    });
    let w = &mut app.world;
    w.spawn((
        Camera {
            // Fixed, so frames differ only by what moves.
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.6, 8.0).looking_at(DVec3::new(0.0, 0.4, -4.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.45, 0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.22, 0.14, 0.08).with_roughness(0.95),
        ),
    ));
    // Small enough to keep a software renderer quick, big enough to fill the view.
    let mut field = GrassField::flat(60.0, 60.0);
    field.style.draw_distance = 40.0;
    w.spawn((Transform::from_xyz(0.0, 0.0, -18.0), field));
    app
}

fn render(app: &mut App, name: &str, frames: usize) -> Option<RgbaImage> {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean color and mean absolute difference between horizontal neighbors (texture) over the
/// rows `y0..y1`.
fn band(img: &RgbaImage, y0: u32, y1: u32) -> ([f32; 3], f32) {
    let mut mean = [0.0; 3];
    let mut detail = 0.0;
    let mut n = 0.0;
    for y in y0..y1 {
        for x in 1..W {
            let p = img.get_pixel(x, y);
            let q = img.get_pixel(x - 1, y);
            for c in 0..3 {
                mean[c] += p[c] as f32;
                detail += (p[c] as f32 - q[c] as f32).abs() / 3.0;
            }
            n += 1.0;
        }
    }
    (mean.map(|m| m / n), detail / n)
}

#[test]
fn a_meadow_covers_the_ground_with_blades() {
    let mut app = meadow_app(Vec3::new(3.0, 0.0, -1.0));
    let Some(img) = render(&mut app, "grass_meadow", 6) else {
        return;
    };
    // Near the camera (bottom of the frame): green, and full of blade-sized detail.
    let (near, near_detail) = band(&img, H - 40, H);
    eprintln!("near grass: color {near:?}, detail {near_detail:.1}");
    assert!(
        near[1] > near[0] && near[1] > near[2],
        "the near ground isn't green: {near:?}"
    );
    assert!(
        near_detail > 6.0,
        "the near grass has no blade detail: {near_detail:.1}"
    );
}

#[test]
fn wind_moves_the_blades() {
    // Normals only: noisy lighting effects change every frame without TAA, the blades' shapes
    // change only when they move. Compared in 16-pixel blocks: a calm meadow's idle quiver
    // shifts single blades by a pixel, gusts sweeping through change how whole patches lean.
    const BLOCK: u32 = 16;
    let block_mean = |img: &RgbaImage, bx: u32, by: u32| {
        let mut sum = [0.0f32; 3];
        for y in by * BLOCK..(by + 1) * BLOCK {
            for x in bx * BLOCK..(bx + 1) * BLOCK {
                let p = img.get_pixel(x, y);
                for c in 0..3 {
                    sum[c] += p[c] as f32;
                }
            }
        }
        sum.map(|s| s / (BLOCK * BLOCK) as f32)
    };
    let diff = |wind: Vec3, name: &str| {
        let mut app = meadow_app_with(wind, |s| s.debug_view = DebugView::Normals);
        // A fixed 0.15 s of world time a frame (1.5 s between the two captures), however
        // slow the frames are: the frame clock advances only as far as the fixed steps do, so
        // a software rasterizer's second-long frames would each be cut to the step cap, and
        // the wall clock made the gust phase at each capture a matter of luck.
        let step = 0.15;
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: step });
        let hz = app.world.resource::<FixedTime>().hz();
        app.world
            .resource_mut::<FixedTime>()
            .set_max_steps((step * hz).ceil() as u32 + 1);
        let a = render(&mut app, &format!("{name}_a"), 6)?;
        let b = render(&mut app, &format!("{name}_b"), 10)?;
        let (mut sum, mut n) = (0.0, 0.0);
        for by in H / 2 / BLOCK..H / BLOCK {
            for bx in 0..W / BLOCK {
                let (p, q) = (block_mean(&a, bx, by), block_mean(&b, bx, by));
                for c in 0..3 {
                    sum += (p[c] - q[c]).abs();
                }
                n += 3.0;
            }
        }
        Some(sum / n)
    };
    let (Some(windy), Some(calm)) = (
        diff(Vec3::new(8.0, 0.0, 0.0), "grass_windy"),
        diff(Vec3::ZERO, "grass_calm"),
    ) else {
        return;
    };
    eprintln!("change between frames: {windy:.2} in wind, {calm:.2} calm");
    assert!(
        windy > calm * 2.0 + 0.5,
        "the wind doesn't move the grass: {windy:.2} vs {calm:.2} calm"
    );
}

#[test]
fn deep_snow_buries_the_meadow() {
    let mut app = meadow_app(Vec3::ZERO);
    let mut layers = engine_world::SurfaceLayers::default();
    layers.grid.fill(engine_world::SurfaceCell {
        snow_m: 0.6,
        ..Default::default()
    });
    app.world.insert_resource(layers);
    let Some(img) = render(&mut app, "grass_snowed", 6) else {
        return;
    };
    // Near the camera: snow, no green blades standing out of it.
    let (near, _) = band(&img, H - 40, H);
    eprintln!("near ground under snow: color {near:?}");
    let green = near[1] - 0.5 * (near[0] + near[2]);
    assert!(green < 4.0, "green blades stand out of deep snow: {near:?}");
}

/// The meadow with a stand of ferns (fronds cut into pinnae, which their program discards) in
/// front of it and a crate standing among them, under a clock that steps the same every frame:
/// the picture, and with `shade_skip::COUNT_MESH_RUNS` in `debug_skip` how often the blades'
/// pixel program ran in the scene pass.
fn ferns_and_a_crate(debug_skip: u32, name: &str) -> Option<(RgbaImage, Option<u64>)> {
    let mut app = meadow_app_with(Vec3::new(3.0, 0.0, -1.0), |s| {
        s.debug_skip = debug_skip;
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.sharpness = 0.0;
        // Their rays are paced by the clock: not the same light in two runs.
        s.probe_gi = false;
    });
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
    let w = &mut app.world;
    w.spawn((
        Transform::from_xyz(0.0, 0.0, 3.0),
        GrassField::flat(14.0, 8.0).with_style(GrassStyle::fern()),
    ));
    w.spawn((
        Transform::from_xyz(-1.2, 0.6, 4.0),
        MeshRenderer::new(
            Shape::Cuboid { size: [1.2; 3] },
            StandardMaterial::color(0.5, 0.4, 0.3),
        ),
    ));
    let img = render(&mut app, name, 8)?;
    if debug_skip & shade_skip::COUNT_MESH_RUNS != 0 {
        // A count is read back frames after it was taken, sooner or later with the machine's
        // speed, and the blades move from frame to frame: three more pictures of this same
        // moment, each waiting for the GPU, and the count read is this moment's in every run.
        let path =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_again.png"));
        for _ in 0..3 {
            capture_now(&mut app.world, &path).expect("screenshot");
        }
    }
    let described = app.world.resource::<Renderer>().describe();
    let grass = described["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "grass"))
        .map(|f| f["state"].clone())
        .unwrap_or_default();
    let old = debug_skip & shade_skip::GRASS_OLD_WAY != 0;
    assert_eq!(grass["old_way"], old, "{grass}");
    Some((img, grass["grass_runs"]["scene"]["fragment_runs"].as_u64()))
}

#[test]
fn grass_lit_only_where_it_shows_is_the_same_picture() {
    // The scene pass lights the blades the prepass drew where the depth is theirs, tested
    // before the program runs (grass.wgsl `fs_main`); until 17 Nov 2026 the cut fronds'
    // program ran at every layer before the test, and each run read whether it was the
    // prepass's surface (`fs_main_old`). The same pixels, from fewer runs.
    let Some((now, _)) = ferns_and_a_crate(0, "grass_shown") else {
        return;
    };
    let Some((before, _)) = ferns_and_a_crate(shade_skip::GRASS_OLD_WAY, "grass_shown_old_way")
    else {
        return;
    };
    // (The output's dither changes with the wall clock: a level or two.)
    let (mut differ, mut worst, mut ferns) = (0, 0, 0);
    for (p, q) in now.pixels().zip(before.pixels()) {
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
        worst = worst.max(d);
        differ += (d > 2) as u32;
        ferns += (p[1] > p[0] && p[1] > p[2]) as u32;
    }
    eprintln!(
        "now against the old way: {differ} pixels differ by more than 2/255, the worst by {worst}; {ferns} green pixels"
    );
    assert!(
        ferns > W * H / 6,
        "ferns and grass fill the view: {ferns} pixels"
    );
    assert_eq!(differ, 0, "the worst pixel differs by {worst}");

    // Counted by the GPU where it can: the runs fall to about the pixels the blades show in.
    let Some((_, Some(runs))) =
        ferns_and_a_crate(shade_skip::COUNT_MESH_RUNS, "grass_shown_counted")
    else {
        return;
    };
    let Some((_, Some(runs_before))) = ferns_and_a_crate(
        shade_skip::COUNT_MESH_RUNS | shade_skip::GRASS_OLD_WAY,
        "grass_shown_old_way_counted",
    ) else {
        return;
    };
    eprintln!(
        "the blades' pixel program in the scene pass: {runs} runs, {runs_before} the old way"
    );
    assert!(
        runs > 0 && runs <= runs_before,
        "{runs} runs against {runs_before} the old way"
    );
}

/// A person's bones standing at the origin and nothing drawn of them: what people's capsule
/// shadows are made from (capsules.rs).
fn standing_bones() -> engine_world::animation::SkeletonPose {
    use engine_world::animation::{BonePose, HUMANOID_BONES, SkeletonPose};
    let at = |name: &str| -> Vec3 {
        let side = if name.ends_with("_l") {
            -0.12
        } else if name.ends_with("_r") {
            0.12
        } else {
            0.0
        };
        let y = match name.trim_end_matches("_l").trim_end_matches("_r") {
            "root" => 0.0,
            "hips" => 1.0,
            "spine" => 1.15,
            "chest" => 1.3,
            "neck" => 1.5,
            "head" => 1.6,
            "upper_arm" | "clavicle" => 1.45,
            "lower_arm" => 1.2,
            "hand" | "upper_leg" => 0.95,
            "lower_leg" => 0.5,
            "foot" => 0.08,
            "toe" => 0.03,
            _ => 1.0,
        };
        let z = if name.starts_with("toe") { -0.12 } else { 0.0 };
        Vec3::new(side, y, z)
    };
    SkeletonPose {
        names: HUMANOID_BONES
            .iter()
            .map(|(n, _)| n.to_string())
            .collect::<Vec<_>>()
            .into(),
        parents: std::sync::Arc::from(vec![None; HUMANOID_BONES.len()]),
        bones: HUMANOID_BONES
            .iter()
            .map(|(n, _)| BonePose {
                translation: at(n),
                ..Default::default()
            })
            .collect(),
    }
}

#[test]
fn a_person_in_the_meadow_shades_the_blades_at_their_feet() {
    // The screen light maps hold the sun people's capsules leave apart from the shadow maps'
    // (`light_b.a`; light through a blade takes the maps alone). The shade at a person's feet
    // must be there, and as deep as when every pixel works its light out itself
    // (`trial_cut::SCREEN_LIGHT` off): a channel nobody reads would leave the meadow sunlit
    // under them.
    let shot = |skip: u32, cuts_off: u32, name: &str| -> Option<RgbaImage> {
        let mut app = meadow_app_with(Vec3::ZERO, |s| {
            s.debug_skip = skip;
            s.trial_cuts &= !cuts_off;
            s.tonemapper = Tonemapper::None;
            s.bloom_intensity = 0.0;
            s.sharpness = 0.0;
            s.probe_gi = false;
        });
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        app.world
            .spawn((Transform::from_xyz(0.0, 0.0, 4.5), standing_bones()));
        render(&mut app, name, 8)
    };
    // How many pixels the capsules darken by more than the output's dither, and by how much
    // in all (levels of the brightest channel).
    let shade = |with: &RgbaImage, without: &RgbaImage| -> (u32, u64) {
        let (mut pixels, mut levels) = (0, 0);
        for (p, q) in with.pixels().zip(without.pixels()) {
            let d = (0..3).map(|c| q[c].saturating_sub(p[c])).max().unwrap_or(0);
            if d > 8 {
                pixels += 1;
                levels += d as u64;
            }
        }
        (pixels, levels)
    };
    let (Some(maps), Some(maps_none)) = (
        shot(0, 0, "grass_person_maps"),
        shot(shade_skip::CAPSULES, 0, "grass_person_maps_no_capsules"),
    ) else {
        return;
    };
    let (Some(direct), Some(direct_none)) = (
        shot(0, trial_cut::SCREEN_LIGHT, "grass_person_direct"),
        shot(
            shade_skip::CAPSULES,
            trial_cut::SCREEN_LIGHT,
            "grass_person_direct_no_capsules",
        ),
    ) else {
        return;
    };
    let (from_maps, from_pixels) = (shade(&maps, &maps_none), shade(&direct, &direct_none));
    eprintln!(
        "a person's capsules darken {} pixels by {} levels through the light maps, {} by {} \
         worked out in each pixel",
        from_maps.0, from_maps.1, from_pixels.0, from_pixels.1
    );
    assert!(
        from_pixels.0 > 30,
        "no shade at the person's feet to compare: {from_pixels:?}"
    );
    assert!(
        from_maps.1.abs_diff(from_pixels.1) * 10 <= from_pixels.1,
        "the shade through the maps is not the pixels' own: {from_maps:?} against {from_pixels:?}"
    );
}
