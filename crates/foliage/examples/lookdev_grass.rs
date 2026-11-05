//! Grass look-development: GPU grass fields of several biomes, rendered headless into PNGs.
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_grass --release -- /tmp/lookdev_grass [shot..]
//! ```
//!
//! Renders 1280×720 (or `LOOKDEV_SIZE=WxH`). Shots: `meadow` (standing in a summer meadow),
//! `meadow_low` (knee height, the blades up close), `meadow_back` (looking toward a low sun
//! through the blades), `savanna` (tall dry tussocks), `tundra` (short, dense, olive and
//! russet), `marsh` (tall, upright, dark), `alien` (a low-gravity world's violet grass under a
//! paler sky), `path` (a footpath worn through the meadow: `GrassWear`), `snow` (the meadow
//! under 12 cm of fresh snow), `herbs` and `litter` (a forest floor's shade herbs and fallen
//! leaves on humus; `herbs_close` and `litter_close` from knee height), `ferns` (woodland ferns,
//! fronds cut into pinnae; `ferns_close`), `field` (the showcase's
//! meadow: drawn out to 120 m, shadows to 22 m, standing in it and looking out over it),
//! `communities` (a 300 m meadow growing `engine_foliage::meadow`'s kinds of grass, painted
//! from a made-up site: a wet hollow and creek, a footpath, a wood's edge along one side, a
//! burnt patch and a parched rise; drawn to 150 m; `communities_low` at knee height in a flower
//! drift, `communities_rise` from 4 m up). Each biome is its own field 1 km from the others,
//! on ground of its own color.
//!
//! `walkers` and `walkers_off`: someone walking through the meadow, with the grass parting
//! around their legs and standing back up behind them (`engine_foliage::walkers`, fed the
//! same `GroundContact`s movement writes), and the same walk with no contacts (blades through
//! the legs, as before).
//!
//! `gale` and `gale_even`: the showcase meadow in a gale with and without the weather's gust
//! fronts (`engine_world::GustWaves`), from 8 m up with the sun behind; `wet`: the `field` view
//! after rain (`SurfaceLayers` wetness 1).
//!
//! `LOOKDEV_PERF=1` (with `ENGINE_PROFILE=1`) also prints, for each shot, what the grass costs:
//! the GPU passes and triangles with the grass drawn and without it (`grass_density` 0).

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{Exposure, GrassField, GrassStyle, GrassWear, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 30.0;

/// A biome's grass and where its field lies.
struct Biome {
    name: &'static str,
    x: f64,
    style: GrassStyle,
}

fn biomes() -> Vec<Biome> {
    let mut out = Vec::new();
    let mut add = |name, style| {
        let x = out.len() as f64 * 1000.0;
        out.push(Biome { name, x, style });
    };
    add("meadow", GrassStyle::meadow());
    add("savanna", GrassStyle::savanna());
    add("tundra", GrassStyle::tundra());
    add("marsh", GrassStyle::marsh());
    add("alien", GrassStyle::alien());
    add("path", GrassStyle::meadow());
    add("snow", GrassStyle::meadow());
    add("herbs", GrassStyle::forest_herbs());
    add("sorrel", GrassStyle::wood_sorrel());
    add("woodruff", GrassStyle::woodruff());
    add("anemone", GrassStyle::wood_anemone());
    add("ferns", GrassStyle::fern());
    add("bilberry", GrassStyle::bilberry());
    add("lingonberry", GrassStyle::lingonberry());
    add("heather", GrassStyle::heather());
    add("litter", GrassStyle::leaf_litter());
    add("twigs", GrassStyle::twigs());
    // games/showcase's `meadow_grass`.
    add(
        "field",
        GrassStyle {
            height: 0.36,
            height_variation: 0.55,
            lush_color: [0.09, 0.18, 0.042],
            dry_color: [0.24, 0.21, 0.085],
            draw_distance: 120.0,
            shadow_distance: 22.0,
            ..GrassStyle::meadow()
        },
    );
    // Tall summer grass on a hillside (the reference video's valley view): flat ground up to
    // x = -20 m from the field's middle, then a 32 degree slope rising toward +x.
    add(
        "hillside",
        GrassStyle {
            height: 0.55,
            height_variation: 0.5,
            draw_distance: 120.0,
            shadow_distance: 22.0,
            ..GrassStyle::meadow()
        },
    );
    // The meadow's tall grass in early summer: seed heads standing above the blades.
    // (`LOOKDEV_ROUND_HEADS=1`: round heads, as before seed heads had a length.)
    let mut tall = engine_foliage::meadow::palette(&GrassStyle::meadow(), 175.0)
        [engine_foliage::meadow::GrassKind::Tall as usize - 1];
    if std::env::var("LOOKDEV_ROUND_HEADS").is_ok_and(|v| v == "1") {
        tall.bloom_length = 0.0;
    }
    add("seedheads", tall);
    add(
        "communities",
        GrassStyle {
            draw_distance: 150.0,
            shadow_distance: 22.0,
            ..GrassStyle::meadow()
        },
    );
    out
}

/// The made-up site under the `communities` field (field-local meters, 300 m across): a
/// wet hollow draining into a creek along z = 210, a footpath along x = 150, a wood's edge
/// along x = 40, a burnt patch, and a parched rise in the far corner.
fn site(x: f32, z: f32) -> engine_foliage::meadow::Site {
    use engine_core::math::smoothstep;
    let n = |s: f64, seed| {
        engine_core::math::noise::fbm2(x as f64 / s, z as f64 / s, seed, 3, 2.0, 0.5) as f32
    };
    let creek = 1.0 - smoothstep(2.0, 9.0, (z - 210.0 - 6.0 * (x * 0.03).sin()).abs());
    let hollow = 1.0
        - smoothstep(
            10.0,
            45.0,
            ((x - 200.0).powi(2) + (z - 180.0).powi(2)).sqrt(),
        );
    let rise = 1.0
        - smoothstep(
            40.0,
            90.0,
            ((x - 260.0).powi(2) + (z - 50.0).powi(2)).sqrt(),
        );
    let water =
        (0.48 + 0.12 * n(60.0, 3) + 0.45 * creek.max(hollow * 0.7) - 0.35 * rise).clamp(0.0, 1.0);
    let path = 1.0 - smoothstep(0.8, 3.5, (x - 150.0 - 3.0 * (z * 0.05).sin()).abs());
    let edge = 1.0 - smoothstep(0.0, 22.0, (x - 40.0 + 6.0 * n(25.0, 9)).abs());
    let burn = 1.0 - smoothstep(12.0, 22.0, ((x - 90.0).powi(2) + (z - 70.0).powi(2)).sqrt());
    engine_foliage::meadow::Site {
        water,
        shade: 0.5 * edge * smoothstep(40.0, 10.0, x),
        slope: 0.4 * rise,
        sunny: rise,
        wear: path,
        edge,
        opened: burn,
    }
}

/// The `communities` field: the kinds painted from [`site`], density and dryness as `field`.
fn communities(style: GrassStyle) -> GrassField {
    let size = 300.0;
    let mut f = field(style, size);
    f.kinds = engine_foliage::meadow::paint_kinds(f.resolution, [size; 2], [0.0; 2], 21, |x, z| {
        Some(site(x, z))
    });
    // Midsummer: the flowers out.
    f.styles = engine_foliage::meadow::palette(&style, 175.0);
    // Dry where it is parched, fresh by the water.
    for (k, d) in f.dryness.iter_mut().enumerate() {
        let (i, j) = (k as u32 % f.resolution[0], k as u32 / f.resolution[0]);
        let s = site(i as f32, j as f32);
        *d = (*d + 0.5 * (0.35 - s.water).max(0.0)).clamp(0.0, 1.0);
    }
    f
}

/// Patches: thicker and fresher in hollows, thinner and drier on rises.
fn field(style: GrassStyle, size: f32) -> GrassField {
    let points = size as u32 + 1;
    GrassField::flat(size, size)
        .sample([points, points], |x, z| {
            let n =
                engine_core::math::noise::fbm2(x as f64 * 0.045, z as f64 * 0.045, 11, 3, 2.0, 0.5);
            let density = (0.75 + 0.35 * n).clamp(0.15, 1.0);
            let dryness = (style.dryness + 0.25 * (-n)).clamp(0.0, 1.0);
            (0.0, density, dryness)
        })
        .with_style(style)
}

/// The hillside's slope (rise per meter) and where it starts (meters from the field's middle).
const HILL_SLOPE: f32 = 0.625;
const HILL_FOOT: f32 = -20.0;

/// The hillside's ground height at `x` meters from the field's middle.
fn hill_height(x: f32) -> f32 {
    (x - HILL_FOOT).max(0.0) * HILL_SLOPE
}

/// The `hillside` field: [`field`]'s patches over [`hill_height`].
fn hillside(style: GrassStyle) -> GrassField {
    let size = 160.0;
    let points = size as u32 + 1;
    GrassField::flat(size, size)
        .sample([points, points], |x, z| {
            let n =
                engine_core::math::noise::fbm2(x as f64 * 0.045, z as f64 * 0.045, 11, 3, 2.0, 0.5);
            let density = (0.8 + 0.3 * n).clamp(0.3, 1.0);
            let dryness = (style.dryness + 0.25 * (-n)).clamp(0.0, 1.0);
            (hill_height(x), density, dryness)
        })
        .with_style(style)
}

/// Field-local x of the footpath's middle at field-local z (it runs along z, wandering).
fn path_x(z: f32) -> f32 {
    80.0 + (z * 0.06).sin() * 0.7 + (z * 0.17).sin() * 0.25
}

/// A footpath down the middle of the 160 m field: worn bare along its middle, trodden flat
/// toward its edges, ragged where feet strayed.
fn footpath() -> GrassWear {
    let cell = 0.25;
    let (x0, w) = (76.0, 8.0);
    let resolution = [(w / cell) as u32 + 1, (160.0 / cell) as u32 + 1];
    let mut wear = Vec::new();
    for j in 0..resolution[1] {
        for i in 0..resolution[0] {
            let (x, z) = (x0 + i as f32 * cell, j as f32 * cell);
            let ragged =
                engine_core::math::noise::fbm2(x as f64 * 0.9, z as f64 * 0.9, 5, 3, 2.0, 0.5);
            let off = (x - path_x(z)).abs() + 0.35 * ragged;
            wear.push(1.0 - engine_core::math::smoothstep(0.2, 1.9, off));
        }
    }
    GrassWear {
        origin: [x0, 0.0],
        cell,
        resolution,
        wear,
    }
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn set_sun(world: &mut World, to_sun: Vec3) {
    let dir = to_sun.normalize();
    world.resource_mut::<WeatherState>().sun_direction = dir;
    let mut q = world.query_filtered::<&mut Transform, With<DirectionalLight>>();
    for mut t in q.iter_mut(world) {
        let mut sun = Transform::default();
        sun.look_to(-dir, Vec3::Y);
        *t = sun;
    }
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3, to_sun: Vec3) {
    aim(&mut app.world, eye, target);
    set_sun(&mut app.world, to_sun);
    app.update_n(12);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    app.update();
    println!("saved {path}");
    if std::env::var("LOOKDEV_PERF").is_ok_and(|v| v == "1") {
        perf(app, name);
    }
}

/// Steps 3 s in a few long frames, so the grass's steady wind (eased toward the weather's over
/// most of a second) has caught up with a change of wind.
fn settle_wind(app: &mut App) {
    let mut time = app.world.resource_mut::<Time>();
    time.set_source(TimeSource::Manual { step_secs: 0.25 });
    app.update_n(12);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
}

/// Someone walks 7 m through the meadow at `at` and is photographed from above and behind as
/// they stop; with `contacts`, their feet write `GroundContact`s as movement's do.
fn walk_through(app: &mut App, out: &str, name: &str, at: DVec3, contacts: bool) {
    let walker = app
        .world
        .spawn((
            Transform::from_translation(at + DVec3::new(-7.0, 0.9, 0.0)),
            MeshRenderer::new(
                Shape::Capsule {
                    radius: 0.24,
                    height: 1.8,
                    segments: 16,
                },
                StandardMaterial::color(0.18, 0.2, 0.26).with_roughness(0.8),
            ),
        ))
        .id();
    // 1.4 m/s, a step every 0.35 s.
    let frames = (7.0 / 1.4 / STEP) as usize;
    for i in 0..=frames {
        let x = -7.0 + 7.0 * i as f64 / frames as f64;
        let body = at + DVec3::new(x, 0.9, 0.0);
        app.world.get_mut::<Transform>(walker).unwrap().translation = body;
        let step = (i as f64 * STEP / 0.35) as i64;
        if contacts && (i as f64 * STEP / 0.35).fract() < STEP / 0.35 {
            let side = if step % 2 == 0 { 0.12 } else { -0.12 };
            app.world.write_message(engine_world::GroundContact {
                position: at + DVec3::new(x, 0.0, side),
                normal: Vec3::Y,
                size: [0.11, 0.28],
                yaw: engine_world::GroundContact::yaw_of(Vec3::X),
                load_kg: 40.0,
                speed: 1.4,
                kind: engine_world::ContactKind::Foot,
                surface: None,
                entity: Some(walker),
            });
        }
        app.update();
    }
    // From above and behind to one side: the swath they walked and where they stand.
    let eye = at + DVec3::new(-4.2, 2.7, 3.0);
    shot(
        app,
        out,
        name,
        eye,
        at + DVec3::new(-1.0, 0.1, 0.0),
        Vec3::new(0.3, 0.6, 0.75),
    );
    app.world.entity_mut(walker).despawn();
    app.update_n(60);
}

/// The GPU passes (ms, averaged over frames) and triangles, with and without the grass.
fn perf(app: &mut App, name: &str) {
    let measure = |app: &mut App| {
        app.update_n(8);
        let mut sum = std::collections::BTreeMap::<String, f64>::new();
        let frames = 24;
        for _ in 0..frames {
            app.update();
            let d = app.world.resource::<Diagnostics>();
            for (k, v) in &d.metrics {
                if k.starts_with("render.gpu") || k == "render.triangles" {
                    *sum.entry(k.clone()).or_default() += v / frames as f64;
                }
            }
        }
        sum
    };
    let with = measure(app);
    let density = app.world.resource::<RenderSettings>().grass_density;
    app.world.resource_mut::<RenderSettings>().grass_density = 0.0;
    let without = measure(app);
    app.world.resource_mut::<RenderSettings>().grass_density = density;
    let get =
        |m: &std::collections::BTreeMap<String, f64>, k: &str| m.get(k).copied().unwrap_or(0.0);
    let line: Vec<String> = [
        "render.gpu_ms",
        "render.gpu.shadows",
        "render.gpu.prepass",
        "render.gpu.scene: grass",
    ]
    .iter()
    .map(|k| {
        format!(
            "{} {:.2}",
            k.trim_start_matches("render.gpu")
                .trim_start_matches(['.', '_']),
            get(&with, k) - get(&without, k)
        )
    })
    .collect();
    println!(
        "perf {name}: grass costs {} ms; {:.2} M triangles",
        line.join(", "),
        (get(&with, "render.triangles") - get(&without, "render.triangles")) / 1e6
    );
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_grass".into());
    let only: Vec<String> = args.collect();
    let wanted = |name: &str| only.is_empty() || only.iter().any(|o| o == name);
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let size = std::env::var("LOOKDEV_SIZE").ok();
        let (width, height) = size
            .as_deref()
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((1280, 720));
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_message::<engine_world::GroundContact>()
        .add_systems(
            Update,
            (
                engine_foliage::walkers::bend_grass_for_walkers,
                engine_foliage::walkers::leave_grass_wakes,
            )
                .chain(),
        );
    // A fresh breeze, so the blades lean and the gusts show.
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(4.0, 0.0, 1.5);
    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.14, 0.05];
    let biomes = biomes();
    for b in &biomes {
        let at = DVec3::new(b.x, 0.0, 0.0);
        let [r, g, bl] = if b.style.leaf > 0.0 {
            // Humus.
            [0.05, 0.035, 0.02]
        } else {
            b.style.ground_color()
        };
        if b.name == "hillside" {
            // The flat foot and the slope, as two planes meeting at the foot.
            let ground = StandardMaterial::color(r, g, bl).with_roughness(0.95);
            let foot = HILL_FOOT as f64;
            app.world.spawn((
                Transform::from_translation(at + DVec3::new(foot - 100.0, 0.0, 0.0)),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [200.0, 400.0],
                        subdivisions: 1,
                    },
                    ground,
                ),
            ));
            let angle = HILL_SLOPE.atan();
            let length = 200.0f32;
            let mid = DVec3::new(
                foot + 0.5 * (length * angle.cos()) as f64,
                0.5 * (length * angle.sin()) as f64,
                0.0,
            );
            app.world.spawn((
                Transform::from_translation(at + mid).with_rotation(Quat::from_rotation_z(angle)),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [length, 400.0],
                        subdivisions: 1,
                    },
                    ground,
                ),
            ));
        } else {
            app.world.spawn((
                Transform::from_translation(at),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [400.0, 400.0],
                        subdivisions: 1,
                    },
                    StandardMaterial::color(r, g, bl).with_roughness(0.95),
                ),
            ));
        }
        let grass = app
            .world
            .spawn((
                Transform::from_translation(at),
                if b.name == "communities" {
                    communities(b.style)
                } else if b.name == "hillside" {
                    hillside(b.style)
                } else {
                    field(b.style, if b.name == "field" { 300.0 } else { 160.0 })
                },
            ))
            .id();
        if b.name == "path" {
            app.world.entity_mut(grass).insert(footpath());
            // Bare earth under the path, in pieces that follow its wander.
            for k in 0..80 {
                let z = k as f32 * 2.0 + 1.0;
                let p = at + DVec3::new(path_x(z) as f64 - 80.0, 0.004, z as f64 - 80.0);
                app.world.spawn((
                    Transform::from_translation(p),
                    MeshRenderer::new(
                        Shape::Plane {
                            size: [2.3, 2.05],
                            subdivisions: 1,
                        },
                        StandardMaterial::color(0.16, 0.12, 0.08).with_roughness(0.95),
                    ),
                ));
            }
        }
    }
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default(),
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 55.0,
                near: 0.05,
            },
            ..Default::default()
        },
        Transform::default(),
    ));
    let mut cams = app.world.query::<&mut Camera>();
    for mut cam in cams.iter_mut(&mut app.world) {
        if let Exposure::Auto { speed, .. } = &mut cam.exposure {
            *speed = 40.0;
        }
    }

    let side_sun = Vec3::new(0.8, 0.75, 0.35);
    for b in &biomes {
        let o = DVec3::new(b.x, 0.0, 0.0);
        if b.name == "meadow" {
            if wanted("meadow") {
                let eye = o + DVec3::new(-20.0, 1.65, 20.0);
                shot(
                    &mut app,
                    &out,
                    "meadow",
                    eye,
                    eye + DVec3::new(12.0, -1.6, -14.0),
                    side_sun,
                );
            }
            if wanted("meadow_low") {
                let eye = o + DVec3::new(-10.0, 0.45, 10.0);
                shot(
                    &mut app,
                    &out,
                    "meadow_low",
                    eye,
                    eye + DVec3::new(3.0, -0.25, -3.0),
                    side_sun,
                );
            }
            for (name, contacts) in [("walkers_off", false), ("walkers", true)] {
                if wanted(name) {
                    walk_through(
                        &mut app,
                        &out,
                        name,
                        o + DVec3::new(-30.0, 0.0, -30.0),
                        contacts,
                    );
                }
            }
            if wanted("meadow_back") {
                let eye = o + DVec3::new(-10.0, 1.0, 10.0);
                let toward = Vec3::new(0.6, 0.2, -0.75);
                shot(
                    &mut app,
                    &out,
                    "meadow_back",
                    eye,
                    eye + DVec3::new(6.0, -0.9, -7.5),
                    toward,
                );
            }
        } else if b.name == "snow" {
            if wanted("snow") {
                let mut layers = engine_world::SurfaceLayers {
                    grid: engine_world::Grid2::centered(
                        DVec2::new(o.x, o.z),
                        200.0,
                        1.0,
                        Default::default(),
                    ),
                };
                layers.grid.fill(engine_world::SurfaceCell {
                    snow_m: 0.12,
                    wetness: 0.3,
                    ..Default::default()
                });
                app.world.insert_resource(layers);
                let eye = o + DVec3::new(-15.0, 1.65, 15.0);
                shot(
                    &mut app,
                    &out,
                    "snow",
                    eye,
                    eye + DVec3::new(12.0, -1.5, -14.0),
                    side_sun,
                );
                app.world.remove_resource::<engine_world::SurfaceLayers>();
            }
        } else if b.name == "field" {
            // A gale (Beaufort 8 at 10 m) over the meadow, from 8 m up with the sun behind: with
            // the weather's gust fronts the grass lies flat in bands that roll across it; without
            // them it leans evenly. Then the same meadow after rain, glossy.
            for (name, fronts) in [("gale_even", false), ("gale", true)] {
                if wanted(name) {
                    let gale = Vec3::new(17.0, 0.0, 6.0);
                    app.world.resource_mut::<WeatherState>().wind = gale;
                    {
                        let mut wind = app.world.resource_mut::<engine_world::WindField>();
                        wind.waves.broad.strength = if fronts { 0.55 } else { 0.0 };
                        wind.waves.fine.strength = if fronts { 0.3 } else { 0.0 };
                        wind.waves.advance(3.0, gale);
                    }
                    // Let the meadow's steady wind (eased over most of a second) catch up.
                    settle_wind(&mut app);
                    let eye = o + DVec3::new(-60.0, 8.0, 45.0);
                    // Sun over the shoulder: flattened blades turn their lit faces up.
                    let behind = Vec3::new(-0.45, 0.5, 0.75);
                    shot(&mut app, &out, name, eye, o, behind);
                    app.world.resource_mut::<WeatherState>().wind = Vec3::new(4.0, 0.0, 1.5);
                    let mut wind = app.world.resource_mut::<engine_world::WindField>();
                    wind.waves.broad.strength = 0.0;
                    wind.waves.fine.strength = 0.0;
                    settle_wind(&mut app);
                }
            }
            if wanted("wet") {
                let before = app.world.remove_resource::<engine_world::SurfaceLayers>();
                let mut layers = engine_world::SurfaceLayers {
                    grid: engine_world::Grid2::centered(
                        DVec2::new(o.x, o.z),
                        200.0,
                        1.0,
                        Default::default(),
                    ),
                };
                layers.grid.fill(engine_world::SurfaceCell {
                    wetness: 1.0,
                    ..Default::default()
                });
                app.world.insert_resource(layers);
                let eye = o + DVec3::new(0.0, 1.7, 0.0);
                shot(
                    &mut app,
                    &out,
                    "wet",
                    eye,
                    eye + DVec3::new(10.0, -0.9, -14.0),
                    side_sun,
                );
                match before {
                    Some(b) => app.world.insert_resource(b),
                    None => {
                        app.world.remove_resource::<engine_world::SurfaceLayers>();
                    }
                }
            }
            if wanted("field") {
                // Standing in the meadow's middle, looking out over it toward the horizon.
                let eye = o + DVec3::new(0.0, 1.7, 0.0);
                shot(
                    &mut app,
                    &out,
                    "field",
                    eye,
                    eye + DVec3::new(10.0, -0.9, -14.0),
                    side_sun,
                );
            }
        } else if b.name == "communities" {
            // The field's -x -z corner is 150 m from its center.
            let c = |x: f64, y: f64, z: f64| o + DVec3::new(x - 150.0, y, z - 150.0);
            if wanted("communities") {
                // On the path, looking up it past the hollow toward the creek.
                let eye = c(152.0, 1.7, 120.0);
                shot(
                    &mut app,
                    &out,
                    "communities",
                    eye,
                    c(175.0, 0.4, 190.0),
                    side_sun,
                );
            }
            if wanted("communities_low") {
                let eye = c(120.0, 0.6, 140.0);
                shot(
                    &mut app,
                    &out,
                    "communities_low",
                    eye,
                    c(126.0, 0.1, 147.0),
                    side_sun,
                );
            }
            if wanted("communities_rise") {
                let eye = c(255.0, 4.0, 60.0);
                shot(
                    &mut app,
                    &out,
                    "communities_rise",
                    eye,
                    c(150.0, 0.0, 200.0),
                    side_sun,
                );
            }
        } else if b.name == "hillside" {
            let on_hill =
                |x: f64, y: f64, z: f64| o + DVec3::new(x, hill_height(x as f32) as f64 + y, z);
            if wanted("hillside") {
                // Standing on the slope, looking along it: the hill rising on the left, the
                // foot below on the right (the reference video's valley view).
                let eye = on_hill(30.0, 1.65, -60.0);
                shot(
                    &mut app,
                    &out,
                    "hillside",
                    eye,
                    on_hill(24.0, -2.5, -42.0),
                    side_sun,
                );
            }
            if wanted("hillside_up") {
                // From the foot, looking up the hill: the long blades lie toward you.
                let eye = on_hill(-24.0, 1.2, 0.0);
                shot(
                    &mut app,
                    &out,
                    "hillside_up",
                    eye,
                    on_hill(-4.0, 0.0, -3.0),
                    side_sun,
                );
            }
        } else if b.name == "path" {
            if wanted("path") {
                let eye = o + DVec3::new(0.3, 1.65, 70.0);
                shot(
                    &mut app,
                    &out,
                    "path",
                    eye,
                    eye + DVec3::new(-0.6, -1.4, -14.0),
                    side_sun,
                );
            }
        } else if wanted(b.name) {
            let eye = o + DVec3::new(-15.0, 1.65, 15.0);
            let mut sun = side_sun;
            if b.name == "alien" {
                sun = Vec3::new(-0.5, 0.45, -0.7);
            }
            shot(
                &mut app,
                &out,
                b.name,
                eye,
                eye + DVec3::new(12.0, -1.5, -14.0),
                sun,
            );
            let close = format!("{}_close", b.name);
            if b.style.leaf > 0.0 && wanted(&close) {
                let eye = o + DVec3::new(-10.0, 0.6, 10.0);
                shot(
                    &mut app,
                    &out,
                    &close,
                    eye,
                    eye + DVec3::new(2.0, -0.5, -2.2),
                    sun,
                );
            }
        }
    }
}
