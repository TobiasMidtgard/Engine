//! Before/after gallery of the volumetric fog: a pine forest at a low sun, a dark room with a
//! sunlit window and a hanging lamp, a misty cave with a crack in its roof, a row of lamps on a
//! misty night, and a valley full of ground fog at sunrise. Each scene is rendered twice, with
//! the volumetric fog off (`<scene>_before.png`: the analytic height fog alone) and on
//! (`<scene>_after.png`), and the time per frame of both is printed.
//!
//! ```text
//! cargo run -p engine_render --example volumetric_fog_gallery -- <out dir> [scene] [WxH]
//! ```
//!
//! `scene` is `forest`, `room`, `cave`, `lamps`, `valley` or `all` (default); the size defaults
//! to 1280x720.
//! Needs a GPU or a software Vulkan driver (lavapipe).

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::{self, Mesh, Vertex};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// Frames rendered before the capture: the fog's and the anti-aliasing's history settle.
const WARM_UP: usize = 40;
/// Frames timed after the warm-up.
const TIMED: usize = 4;

/// Builds a scene into a fresh app.
type Scene = fn(&mut App);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().map(String::as_str).unwrap_or("screenshots"));
    let which = args.get(1).map(String::as_str).unwrap_or("all");
    let (width, height) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280, 720));
    std::fs::create_dir_all(&out).expect("output directory");
    let scenes: [(&str, Scene); 5] = [
        ("forest", forest),
        ("room", room),
        ("cave", cave),
        ("lamps", lamps),
        ("valley", valley),
    ];
    for (name, build) in scenes {
        if which != "all" && which != name {
            continue;
        }
        for (volumetric, suffix) in [(false, "before"), (true, "after")] {
            let path = out.join(format!("{name}_{suffix}.png"));
            match shoot(build, volumetric, (width, height), &path) {
                Some(ms) => println!("{}: {ms:.0} ms per frame", path.display()),
                None => {
                    eprintln!("no GPU: nothing rendered");
                    return;
                }
            }
        }
    }
}

/// Renders the scene `build` makes and saves it to `path`; returns the time per frame (ms).
fn shoot(build: Scene, volumetric: bool, (width, height): (u32, u32), path: &Path) -> Option<f64> {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin);
    build(&mut app);
    app.world.resource_mut::<RenderSettings>().volumetric_fog = volumetric;
    app.update_n(WARM_UP);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("rendering disabled: {}", d.0);
        return None;
    }
    let start = Instant::now();
    app.update_n(TIMED);
    let ms = start.elapsed().as_secs_f64() * 1000.0 / TIMED as f64;
    capture_now(&mut app.world, path).expect("screenshot");
    Some(ms)
}

// ------------------------------------------------------------------------------------ scenes

/// A pine forest in the morning: tall trunks against a low sun, mist hanging between them.
/// The sun is just off to the right of the view and low enough to shine in under the crowns
/// from the forest's edge 60 m away, so the forward-scattering fog glows toward it and the
/// trunks cut dark lanes into it that run across the picture.
fn forest(app: &mut App) {
    let ground_at = |x: f32, z: f32| {
        (fbm(x / 40.0, z / 40.0, 4, 11) - 0.5) * 3.0 + (fbm(x / 7.0, z / 7.0, 3, 12) - 0.5) * 0.4
    };
    *app.world.resource_mut::<FogSettings>() = FogSettings {
        density: 0.012,
        height_falloff: 0.06,
        base_height: 0.0,
        color: [0.92, 0.94, 0.97],
        sun_scattering: 0.7,
        ..Default::default()
    };
    let w = &mut app.world;
    let eye = Vec3::new(0.0, ground_at(0.0, 0.0) + 1.7, 0.0);
    spawn_camera(w, eye, eye + Vec3::new(-0.05, 0.07, -1.0), 62.0, 11.7);
    let sun = sun_toward(50.0, 7.0);
    spawn_sun(w, sun, 110_000.0);
    let toward_sun = Vec2::new(sun.x, sun.z).normalize();

    let ground = terrain(Vec2::ZERO, 500.0, 360, 0.35, ground_at, |p, n| {
        let moss = fbm(p.x / 9.0, p.z / 9.0, 4, 21);
        let litter = fbm(p.x / 2.5, p.z / 2.5, 3, 22);
        let soil = Vec3::new(0.085, 0.065, 0.045).lerp(Vec3::new(0.12, 0.075, 0.04), litter);
        let c = soil.lerp(Vec3::new(0.06, 0.085, 0.035), smooth(0.4, 0.7, moss));
        c * (0.75 + 0.25 * n.y)
    });
    spawn_mesh(
        w,
        ground,
        white().with_roughness(0.95).with_detail(0.5, 0.35),
    );

    // Trunks at least 3 m apart, none right in front of the camera nor beyond the forest's
    // edge toward the sun.
    let mut rng = Rng(0x5eed_f0e5);
    let mut spots: Vec<Vec2> = Vec::new();
    for _ in 0..12000 {
        let p = Vec2::new(rng.range(-170.0, 170.0), rng.range(-200.0, 50.0));
        let from_eye = p - Vec2::new(eye.x, eye.z);
        let near = from_eye.length() < 4.0;
        let beyond_edge =
            from_eye.dot(toward_sun) > 60.0 + 8.0 * fbm(p.x / 20.0, p.y / 20.0, 2, 13);
        if !near && !beyond_edge && spots.iter().all(|q| q.distance(p) > 3.0) {
            spots.push(p);
        }
        if spots.len() >= 1700 {
            break;
        }
    }
    let mut trees = Mesh::default();
    for p in &spots {
        let tall = rng.range(22.0, 34.0);
        let young = rng.next() < 0.15;
        let height = if young { rng.range(3.0, 9.0) } else { tall };
        let crown_from = if young { 0.12 } else { rng.range(0.5, 0.66) };
        let mut tree = conifer(&mut rng, height, crown_from);
        let lean = Quat::from_rotation_z(rng.range(-0.04, 0.04))
            * Quat::from_rotation_x(rng.range(-0.04, 0.04))
            * Quat::from_rotation_y(rng.range(0.0, TAU));
        let base = Vec3::new(p.x, ground_at(p.x, p.y) - 0.3, p.y);
        tree.transform(&Mat4::from_rotation_translation(lean, base));
        trees.merge(&tree);
    }
    spawn_mesh(
        w,
        trees,
        white()
            .with_roughness(0.85)
            .with_detail(0.6, 0.12)
            .with_subsurface(0.3),
    );

    // Boulders and fallen trunks on the forest floor.
    let mut props = Mesh::default();
    for seed in 0..36 {
        let p = Vec2::new(rng.range(-50.0, 50.0), rng.range(-80.0, -6.0));
        let s = rng.range(0.5, 1.8);
        let size = Vec3::new(s * rng.range(0.9, 1.4), s * rng.range(0.5, 0.8), s);
        let mut rock = Shape::Rock {
            size: size.to_array(),
            seed,
            detail: 4,
        }
        .mesh();
        let tone = rng.range(0.8, 1.1);
        for v in &mut rock.vertices {
            let moss = smooth(0.4, 0.9, v.normal[1]);
            let c = Vec3::new(0.2, 0.19, 0.17).lerp(Vec3::new(0.07, 0.1, 0.035), moss) * tone;
            v.color = c.extend(1.0).to_array();
        }
        let at = Vec3::new(p.x, ground_at(p.x, p.y) + size.y * 0.35, p.y);
        let turn = Quat::from_rotation_y(rng.range(0.0, TAU));
        rock.transform(&Mat4::from_rotation_translation(turn, at));
        props.merge(&rock);
    }
    for _ in 0..8 {
        let p = Vec2::new(rng.range(-40.0, 40.0), rng.range(-70.0, -8.0));
        let len = rng.range(6.0, 14.0);
        let r = rng.range(0.2, 0.35);
        let mut log = mesh::cylinder(r, r * 0.8, len, 16);
        paint(&mut log, Vec3::new(0.09, 0.07, 0.05));
        let turn = Quat::from_rotation_y(rng.range(0.0, TAU)) * Quat::from_rotation_z(1.55);
        let at = Vec3::new(p.x, ground_at(p.x, p.y) + r * 0.7, p.y);
        log.transform(&Mat4::from_rotation_translation(turn, at));
        props.merge(&log);
    }
    spawn_mesh(w, props, white().with_roughness(0.9).with_detail(0.7, 0.25));

    // Patchy mist near the ground, drifting slowly with the wind.
    w.spawn((
        Transform::from_xyz(0.0, 5.0, -60.0),
        FogVolume::new([150.0, 7.0, 150.0])
            .with_density(0.025)
            .with_edge_falloff(5.0)
            .with_noise(0.8, 9.0)
            .with_anisotropy(0.7),
    ));
}

/// A dark room in the afternoon: sunlight falls through a four-pane window across dusty air
/// onto the floor, and a bare bulb glows in the haze over a table.
fn room(app: &mut App) {
    // A light outdoor haze; the dust is a fog volume filling the room.
    app.world.resource_mut::<FogSettings>().density = 0.0008;
    let w = &mut app.world;
    let eye = Vec3::new(-4.3, 1.55, 3.4);
    spawn_camera(w, eye, Vec3::new(3.0, 1.75, -2.2), 62.0, 8.0);
    spawn_sun(w, Vec3::new(1.0, 0.55, 0.25).normalize(), 110_000.0);

    // Outside: a meadow.
    let meadow = terrain(
        Vec2::ZERO,
        800.0,
        160,
        2.0,
        |_, _| -0.05,
        |p, _| {
            let t = fbm(p.x / 12.0, p.z / 12.0, 4, 5);
            Vec3::new(0.07, 0.1, 0.035).lerp(Vec3::new(0.12, 0.11, 0.05), t)
        },
    );
    spawn_mesh(
        w,
        meadow,
        white().with_roughness(0.95).with_detail(0.4, 0.5),
    );

    // The room: inside x -5..5, y 0..4, z -4..4, walls 0.3 m thick reaching into the 0.5 m
    // ceiling (no seams for light to leak through). The window in the +x wall
    // spans z -2.0..0.2 and y 1.0..3.2.
    let plaster = StandardMaterial::color(0.62, 0.58, 0.52)
        .with_roughness(0.92)
        .with_detail(0.25, 0.4);
    let boards = StandardMaterial::color(0.28, 0.16, 0.08)
        .with_roughness(0.55)
        .with_detail(0.5, 0.15);
    let frame = StandardMaterial::color(0.18, 0.13, 0.09).with_roughness(0.6);
    let wood = StandardMaterial::color(0.22, 0.12, 0.06)
        .with_roughness(0.45)
        .with_detail(0.4, 0.1);
    let walls: [(Vec3, Vec3, StandardMaterial); 12] = [
        // Floor, ceiling, back, front, left.
        ((-5.3, -0.3, -4.3).into(), (5.3, 0.0, 4.3).into(), boards),
        ((-5.3, 4.0, -4.3).into(), (5.3, 4.5, 4.3).into(), plaster),
        ((-5.3, 0.0, -4.3).into(), (5.3, 4.3, -4.0).into(), plaster),
        ((-5.3, 0.0, 4.0).into(), (5.3, 4.3, 4.3).into(), plaster),
        ((-5.3, 0.0, -4.0).into(), (-5.0, 4.3, 4.0).into(), plaster),
        // The window wall, around the opening.
        ((5.0, 0.0, -4.0).into(), (5.3, 4.3, -2.0).into(), plaster),
        ((5.0, 0.0, 0.2).into(), (5.3, 4.3, 4.0).into(), plaster),
        ((5.0, 0.0, -2.0).into(), (5.3, 1.0, 0.2).into(), plaster),
        ((5.0, 3.2, -2.0).into(), (5.3, 4.3, 0.2).into(), plaster),
        // Mullions and the sill.
        ((5.05, 2.05, -2.0).into(), (5.17, 2.15, 0.2).into(), frame),
        ((5.05, 1.0, -0.95).into(), (5.17, 3.2, -0.85).into(), frame),
        ((4.85, 0.94, -2.1).into(), (5.32, 1.0, 0.3).into(), frame),
    ];
    for (min, max, material) in walls {
        spawn_box(w, (min + max) * 0.5, max - min, material);
    }
    // Skirting boards.
    for (min, max) in [
        (Vec3::new(-5.0, 0.0, -4.0), Vec3::new(5.0, 0.12, -3.98)),
        (Vec3::new(-5.0, 0.0, -4.0), Vec3::new(-4.98, 0.12, 4.0)),
    ] {
        spawn_box(w, (min + max) * 0.5, max - min, frame);
    }

    // Furniture with rounded edges: a table under the lamp, a chair, a cabinet, a rug.
    let rounded = |w: &mut World, min: Vec3, max: Vec3, radius: f32, m: StandardMaterial| {
        w.spawn((
            Transform::from_translation(((min + max) * 0.5).as_dvec3()),
            MeshRenderer::new(
                Shape::RoundedCuboid {
                    size: (max - min).to_array(),
                    radius,
                    segments: 0,
                },
                m,
            ),
        ));
    };
    let table = (Vec3::new(-2.6, 0.0, -3.7), Vec3::new(-0.4, 0.78, -2.4));
    rounded(
        w,
        Vec3::new(table.0.x, 0.74, table.0.z),
        Vec3::new(table.1.x, 0.78, table.1.z),
        0.015,
        wood,
    );
    for (x, z) in [
        (table.0.x + 0.08, table.0.z + 0.08),
        (table.1.x - 0.08, table.0.z + 0.08),
        (table.0.x + 0.08, table.1.z - 0.08),
        (table.1.x - 0.08, table.1.z - 0.08),
    ] {
        rounded(
            w,
            Vec3::new(x - 0.035, 0.0, z - 0.035),
            Vec3::new(x + 0.035, 0.74, z + 0.035),
            0.01,
            wood,
        );
    }
    let chair = Vec3::new(-1.3, 0.0, -1.9);
    rounded(
        w,
        chair + Vec3::new(-0.23, 0.42, -0.23),
        chair + Vec3::new(0.23, 0.46, 0.23),
        0.012,
        wood,
    );
    rounded(
        w,
        chair + Vec3::new(-0.23, 0.46, 0.19),
        chair + Vec3::new(0.23, 0.98, 0.23),
        0.012,
        wood,
    );
    for (x, z) in [(-0.2, -0.2), (0.2, -0.2), (-0.2, 0.2), (0.2, 0.2)] {
        rounded(
            w,
            chair + Vec3::new(x - 0.02, 0.0, z - 0.02),
            chair + Vec3::new(x + 0.02, 0.42, z + 0.02),
            0.008,
            wood,
        );
    }
    rounded(
        w,
        Vec3::new(-4.98, 0.0, -1.2),
        Vec3::new(-4.45, 1.9, 1.2),
        0.02,
        wood,
    );
    rounded(
        w,
        Vec3::new(-1.0, 0.0, -2.0),
        Vec3::new(2.6, 0.012, 0.6),
        0.005,
        StandardMaterial::color(0.32, 0.08, 0.06)
            .with_roughness(0.95)
            .with_detail(0.3, 0.05),
    );

    // The lamp: a bare bulb on a cord over the table.
    let bulb_at = Vec3::new(-1.5, 2.3, -3.05);
    spawn_box(
        w,
        bulb_at + Vec3::Y * 0.9,
        Vec3::new(0.012, 1.7, 0.012),
        StandardMaterial::color(0.02, 0.02, 0.02),
    );
    w.spawn((
        Transform::from_translation(bulb_at.as_dvec3()),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.05,
                segments: 24,
            },
            StandardMaterial::color(1.0, 0.9, 0.7).with_emissive([1.0, 0.75, 0.45], 2000.0),
        ),
    ));
    w.spawn((
        Transform::from_translation(bulb_at.as_dvec3()),
        PointLight {
            color: [1.0, 0.72, 0.42],
            intensity_lm: 5000.0,
            range: 16.0,
            radius: 0.05,
        },
    ));

    // Indoors the sky only lights the room through the window.
    w.spawn((
        Transform::from_xyz(0.0, 2.0, 0.0),
        SkyLightVolume::new([5.0, 2.0, 4.0])
            .with_sky_light(0.012)
            .with_fade(3.0),
    ));
    // Dust in the air.
    w.spawn((
        Transform::from_xyz(0.0, 2.0, 0.0),
        FogVolume::new([5.0, 2.0, 4.0])
            .with_density(0.07)
            .with_albedo([0.85, 0.82, 0.78])
            .with_edge_falloff(0.1)
            .with_noise(0.6, 1.2)
            .with_anisotropy(0.45),
    ));
}

/// Deep in a cave, looking out of its mouth at a misty valley in the morning sun. Damp mist
/// fills the cave; sky light only comes in at the mouth, so the mist deep inside stays dark,
/// and the sun reaches in only through the mouth and a crack in the roof, which drops a beam
/// onto the floor in front of the camera.
fn cave(app: &mut App) {
    // Morning mist in the valley below the cave; the ground outside drops away from a ledge.
    *app.world.resource_mut::<FogSettings>() = FogSettings {
        density: 0.012,
        height_falloff: 0.07,
        base_height: -38.0,
        color: [0.94, 0.95, 0.98],
        sun_scattering: 0.65,
        ..Default::default()
    };
    let ground_at = |x: f32, z: f32| {
        let drop = -38.0 * smooth(6.0, 140.0, z);
        let ridge =
            120.0 * smooth(260.0, 700.0, z) * (0.6 + 0.6 * fbm(x / 150.0, z / 150.0, 3, 61));
        let bumps = (fbm(x / 9.0, z / 9.0, 4, 62) - 0.5) * 0.6 * (0.3 + smooth(0.0, 20.0, z));
        drop + ridge + bumps
    };
    let w = &mut app.world;
    let eye = Vec3::new(-2.2, 1.65, -24.0);
    spawn_camera(w, eye, Vec3::new(1.0, 2.6, 10.0), 66.0, 12.0);
    let sun = sun_toward(168.0, 42.0);
    spawn_sun(w, sun, 110_000.0);

    let ground = terrain(Vec2::new(0.0, -10.0), 900.0, 420, 0.4, ground_at, |p, n| {
        let t = fbm(p.x / 14.0, p.z / 14.0, 4, 63);
        let grass = Vec3::new(0.07, 0.09, 0.035).lerp(Vec3::new(0.12, 0.11, 0.05), t);
        let stone = Vec3::new(0.15, 0.14, 0.12) * (0.8 + 0.4 * fbm(p.x / 3.0, p.z / 3.0, 3, 64));
        // Bare, damp stone inside the cave and on the ledge.
        let bare = smooth(8.0, 2.0, p.z).max(smooth(0.8, 0.6, n.y));
        grass.lerp(stone * 0.8, bare)
    });
    spawn_mesh(w, ground, white().with_roughness(0.9).with_detail(0.6, 0.4));

    // The cave: inside x -7..7, y 0..6, z -27..0, open at z = 0. A thick shell of rock (no seams
    // for light to leak through), with a crack in the 2 m roof at x -1.2..1.2, z -16..-11 (long
    // enough for the slanting sun to get through).
    let rock = StandardMaterial::color(0.14, 0.13, 0.12)
        .with_roughness(0.9)
        .with_detail(0.9, 0.6);
    let shell: [(Vec3, Vec3); 7] = [
        ((-10.0, -3.0, -30.0).into(), (-7.0, 8.0, 0.0).into()),
        ((7.0, -3.0, -30.0).into(), (10.0, 8.0, 0.0).into()),
        ((-10.0, -3.0, -30.0).into(), (10.0, 8.0, -27.0).into()),
        // The roof, around the crack.
        ((-10.0, 6.0, -27.0).into(), (10.0, 8.0, -16.0).into()),
        ((-10.0, 6.0, -11.0).into(), (10.0, 8.0, 0.0).into()),
        ((-10.0, 6.0, -16.0).into(), (-1.2, 8.0, -11.0).into()),
        ((1.2, 6.0, -16.0).into(), (10.0, 8.0, -11.0).into()),
    ];
    for (min, max) in shell {
        spawn_box(w, (min + max) * 0.5, max - min, rock);
    }
    // Rough rock lining the walls, slabs hanging from the roof, an uneven arch at the mouth and
    // boulders on the floor.
    let mut rng = Rng(0xca7e_0001);
    let mut rocks = Mesh::default();
    let mut add_rock = |rng: &mut Rng, at: Vec3, size: Vec3, upside_down: bool| {
        let mut m = Shape::Rock {
            size: size.to_array(),
            seed: (rng.next() * 1e6) as u32,
            detail: 4,
        }
        .mesh();
        let tone = rng.range(0.75, 1.15);
        for v in &mut m.vertices {
            let wet = smooth(0.5, -0.5, v.normal[1]);
            let c = Vec3::new(0.16, 0.15, 0.13).lerp(Vec3::new(0.09, 0.085, 0.08), wet) * tone;
            v.color = c.extend(1.0).to_array();
        }
        let flip = if upside_down {
            Quat::from_rotation_x(std::f32::consts::PI)
        } else {
            Quat::IDENTITY
        };
        let turn = Quat::from_rotation_y(rng.range(0.0, TAU)) * flip;
        m.transform(&Mat4::from_rotation_translation(turn, at));
        rocks.merge(&m);
    };
    let mut z = -27.0;
    while z < 0.5 {
        for side in [-1.0, 1.0] {
            let s = rng.range(2.5, 4.0);
            let at = Vec3::new(side * rng.range(6.8, 7.6), rng.range(-0.5, 2.5), z);
            let size = Vec3::new(s, s * rng.range(1.0, 1.6), s);
            add_rock(&mut rng, at, size, false);
        }
        z += rng.range(2.0, 3.2);
    }
    for i in 0..5 {
        let x = -6.0 + i as f32 * 3.0 + rng.range(-0.8, 0.8);
        let s = rng.range(2.5, 3.5);
        add_rock(&mut rng, Vec3::new(x, -0.5, -26.8), Vec3::splat(s), false);
    }
    for _ in 0..22 {
        let at = Vec3::new(rng.range(-6.5, 6.5), 6.4, rng.range(-26.5, 0.3));
        if at.x.abs() < 2.4 && (-17.5..-9.5).contains(&at.z) {
            continue;
        }
        let s = rng.range(1.2, 2.6);
        add_rock(&mut rng, at, Vec3::new(s, s * 0.7, s), true);
    }
    // Ragged edges for the crack, so its beam isn't a neat rectangle.
    for (x, z) in [
        (-1.5, -15.4),
        (-1.4, -13.9),
        (-1.6, -12.2),
        (1.5, -15.0),
        (1.6, -13.4),
        (1.4, -11.8),
        (-0.4, -16.4),
        (0.7, -16.2),
    ] {
        let s = rng.range(1.0, 1.5);
        let size = Vec3::new(s, s * rng.range(0.6, 0.9), s);
        add_rock(&mut rng, Vec3::new(x, 6.3, z), size, true);
    }
    // An uneven arch over the mouth and big boulders at its sides.
    for x in [-6.6, -4.6, -2.3, 0.2, 2.5, 4.7, 6.6] {
        let s = rng.range(2.2, 3.2);
        let at = Vec3::new(x + rng.range(-0.4, 0.4), 6.6, rng.range(-0.6, 0.4));
        let size = Vec3::new(s, s * rng.range(0.9, 1.4), s);
        add_rock(&mut rng, at, size, true);
    }
    for (x, z, s) in [(-6.2, -0.5, 4.5), (6.4, 0.2, 4.0), (-5.0, 1.2, 2.4)] {
        add_rock(
            &mut rng,
            Vec3::new(x, -0.4, z),
            Vec3::new(s, s * 1.3, s),
            false,
        );
    }
    for _ in 0..14 {
        let at = Vec3::new(rng.range(-6.0, 6.0), -0.15, rng.range(-25.0, 4.0));
        if at.x.abs() < 1.8 && at.z < -18.0 {
            continue;
        }
        let s = rng.range(0.4, 1.3);
        add_rock(&mut rng, at, Vec3::new(s, s * 0.6, s), false);
    }
    spawn_mesh(w, rocks, white().with_roughness(0.85).with_detail(0.8, 0.3));

    // Pines on the slopes and the valley floor outside.
    let mut trees = Mesh::default();
    let mut count = 0;
    for _ in 0..40000 {
        let p = Vec2::new(rng.range(-400.0, 400.0), rng.range(14.0, 600.0));
        if rng.next() > smooth(0.42, 0.62, fbm(p.x / 60.0, p.y / 60.0, 3, 65)) {
            continue;
        }
        let height = rng.range(9.0, 24.0);
        let crown_from = rng.range(0.12, 0.3);
        let mut tree = conifer(&mut rng, height, crown_from);
        let turn = Quat::from_rotation_y(rng.range(0.0, TAU));
        let base = Vec3::new(p.x, ground_at(p.x, p.y) - 0.3, p.y);
        tree.transform(&Mat4::from_rotation_translation(turn, base));
        trees.merge(&tree);
        count += 1;
        if count >= 1200 {
            break;
        }
    }
    spawn_mesh(
        w,
        trees,
        white()
            .with_roughness(0.85)
            .with_detail(0.5, 0.15)
            .with_subsurface(0.3),
    );

    // Sky light comes in only at the mouth; damp mist fills the cave. The box reaches a little
    // into the walls and under the floor, so their bases don't catch the open sky's light.
    w.spawn((
        Transform::from_xyz(0.0, 2.4, -13.5),
        SkyLightVolume::new([7.4, 3.6, 13.6])
            .open(BoxFace::PosZ)
            .with_sky_light(0.02)
            .with_fade(8.0),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 3.0, -13.0),
        FogVolume::new([7.5, 3.5, 14.5])
            .with_density(0.05)
            .with_albedo([0.9, 0.9, 0.88])
            .with_edge_falloff(1.5)
            .with_noise(0.5, 2.5)
            .with_anisotropy(0.6),
    ));
}

/// A path through a park on a misty night: a row of lamps recedes into the fog, each one glowing
/// in the mist around it, under a faint moon.
fn lamps(app: &mut App) {
    *app.world.resource_mut::<FogSettings>() = FogSettings {
        density: 0.035,
        height_falloff: 0.12,
        base_height: 0.0,
        color: [0.9, 0.92, 0.95],
        sun_scattering: 0.55,
        ..Default::default()
    };
    let path_x = |z: f32| 2.5 * (z / 23.0).sin();
    let ground_at = |x: f32, z: f32| (fbm(x / 25.0, z / 25.0, 3, 71) - 0.5) * 1.2;
    let w = &mut app.world;
    let eye = Vec3::new(path_x(4.0) - 0.4, ground_at(0.0, 4.0) + 1.7, 4.0);
    spawn_camera(
        w,
        eye,
        Vec3::new(path_x(-40.0), ground_at(0.0, -40.0) + 2.6, -40.0),
        58.0,
        2.5,
    );
    // The moon, high behind the camera.
    spawn_sun(w, sun_toward(200.0, 40.0), 0.3);

    let ground = terrain(Vec2::new(0.0, -10.0), 400.0, 300, 0.3, ground_at, |p, _| {
        let on_path = smooth(1.6, 1.1, (p.x - path_x(p.z)).abs());
        let grass = Vec3::new(0.05, 0.08, 0.03)
            .lerp(Vec3::new(0.09, 0.1, 0.04), fbm(p.x / 6.0, p.z / 6.0, 4, 72));
        let gravel = Vec3::new(0.2, 0.18, 0.15) * (0.8 + 0.4 * fbm(p.x * 2.0, p.z * 2.0, 3, 73));
        grass.lerp(gravel, on_path)
    });
    spawn_mesh(w, ground, white().with_roughness(0.9).with_detail(0.6, 0.3));

    // Lamp posts along the path, alternating sides.
    let iron = StandardMaterial::color(0.03, 0.03, 0.03).with_roughness(0.4);
    let glass = StandardMaterial::color(1.0, 0.85, 0.6).with_emissive([1.0, 0.72, 0.4], 600.0);
    for i in 0..7 {
        let z = -4.0 - 13.0 * i as f32;
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let x = path_x(z) + side * 1.9;
        let base = ground_at(x, z);
        spawn_box(
            w,
            Vec3::new(x, base + 1.9, z),
            Vec3::new(0.09, 3.8, 0.09),
            iron,
        );
        let head = Vec3::new(x, base + 4.0, z);
        spawn_box(w, head + Vec3::Y * 0.22, Vec3::new(0.42, 0.06, 0.42), iron);
        spawn_box(w, head, Vec3::new(0.28, 0.38, 0.28), glass);
        w.spawn((
            Transform::from_translation(head.as_dvec3()),
            PointLight {
                color: [1.0, 0.72, 0.42],
                intensity_lm: 2500.0,
                range: 30.0,
                radius: 0.12,
            },
        ));
    }

    // Trees beside the path and a bench under the first lamp.
    let mut rng = Rng(0x1a3b_0007);
    let mut trees = Mesh::default();
    let mut count = 0;
    for _ in 0..20000 {
        let p = Vec2::new(rng.range(-60.0, 60.0), rng.range(-200.0, 0.0));
        if (p.x - path_x(p.y)).abs() < 5.0 || rng.next() > 0.35 {
            continue;
        }
        let (height, crown_from) = (rng.range(8.0, 18.0), rng.range(0.25, 0.45));
        let mut tree = conifer(&mut rng, height, crown_from);
        let turn = Quat::from_rotation_y(rng.range(0.0, TAU));
        let base = Vec3::new(p.x, ground_at(p.x, p.y) - 0.2, p.y);
        tree.transform(&Mat4::from_rotation_translation(turn, base));
        trees.merge(&tree);
        count += 1;
        if count >= 260 {
            break;
        }
    }
    spawn_mesh(
        w,
        trees,
        white().with_roughness(0.85).with_detail(0.5, 0.15),
    );
    let wood = StandardMaterial::color(0.2, 0.12, 0.07).with_roughness(0.6);
    let bench = Vec3::new(
        path_x(-4.0) + 1.5,
        ground_at(path_x(-4.0) + 1.5, -6.0),
        -6.0,
    );
    spawn_box(
        w,
        bench + Vec3::new(0.0, 0.45, 0.0),
        Vec3::new(0.45, 0.05, 1.6),
        wood,
    );
    spawn_box(
        w,
        bench + Vec3::new(0.2, 0.75, 0.0),
        Vec3::new(0.05, 0.4, 1.6),
        wood,
    );
    for dz in [-0.7, 0.7] {
        spawn_box(
            w,
            bench + Vec3::new(0.0, 0.22, dz),
            Vec3::new(0.4, 0.44, 0.06),
            iron,
        );
    }
}

/// A broad valley at sunrise, seen from the edge of a cliff at its head: ground fog fills the
/// valley floor like a lake, the sun rises over the low hills at its far end, and the pines
/// on the floor and the slopes stand in the mist and cast long shadows across it toward the
/// camera. The volumetric fog reaches 1 km here, a vista's range.
fn valley(app: &mut App) {
    *app.world.resource_mut::<FogSettings>() = FogSettings {
        density: 0.016,
        height_falloff: 0.12,
        base_height: 4.0,
        color: [0.95, 0.96, 1.0],
        sun_scattering: 0.65,
        ..Default::default()
    };
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.volumetric_fog_range = 1000.0;
        s.shadow_distance = 1000.0;
    }
    let w = &mut app.world;

    // The lookout: the edge of the cliff at the head of the valley, 120 m above its floor.
    let eye = Vec3::new(
        valley_center(0.0),
        valley_height(valley_center(0.0), 0.0) + 2.5,
        0.0,
    );
    let target = Vec3::new(valley_center(-1500.0), -120.0, -1500.0);
    spawn_camera(w, eye, target, 50.0, 11.5);
    spawn_sun(w, sun_toward(2.0, 4.0), 110_000.0);

    let ground = terrain(
        Vec2::new(eye.x, -2400.0),
        3600.0,
        640,
        1.0,
        valley_height,
        |p, n| {
            let t = fbm(p.x / 30.0, p.z / 30.0, 4, 31);
            let grass = Vec3::new(0.08, 0.1, 0.04).lerp(Vec3::new(0.13, 0.12, 0.06), t);
            let stone =
                Vec3::new(0.16, 0.15, 0.14) * (0.8 + 0.4 * fbm(p.x / 6.0, p.z / 6.0, 3, 32));
            let steep = smooth(0.85, 0.65, n.y);
            let wet = smooth(3.0, 0.5, p.y);
            grass
                .lerp(stone, steep)
                .lerp(Vec3::new(0.05, 0.06, 0.035), wet * 0.6)
        },
    );
    spawn_mesh(
        w,
        ground,
        white().with_roughness(0.92).with_detail(0.5, 0.6),
    );

    // A slow river along the valley floor, showing where the ground dips below it.
    w.spawn((
        Transform::from_xyz(valley_center(-1200.0) as f64, 0.4, -1200.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [500.0, 2400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.02, 0.025, 0.03).with_roughness(0.04),
        ),
    ));

    // Pines in clumps on the floor and the slopes, none on the cliff.
    let mut rng = Rng(0x007a_11e7);
    let mut trees = Mesh::default();
    let mut count = 0;
    for _ in 0..80000 {
        let p = Vec2::new(
            rng.range(eye.x - 900.0, eye.x + 900.0),
            rng.range(-2200.0, -80.0),
        );
        let h = valley_height(p.x, p.y);
        if !(1.0..320.0).contains(&h) {
            continue;
        }
        let clump = fbm(p.x / 90.0, p.y / 90.0, 3, 41);
        let on_floor = smooth(12.0, 4.0, h);
        if rng.next() > smooth(0.45, 0.65, clump) * (1.0 + 0.5 * on_floor) {
            continue;
        }
        // Not on cliffs.
        let slope = (valley_height(p.x + 2.0, p.y) - valley_height(p.x - 2.0, p.y)).abs() / 4.0;
        let slope_z = (valley_height(p.x, p.y + 2.0) - valley_height(p.x, p.y - 2.0)).abs() / 4.0;
        if slope.max(slope_z) > 0.9 {
            continue;
        }
        let height = rng.range(10.0, 26.0);
        let crown_from = rng.range(0.12, 0.3);
        let mut tree = conifer(&mut rng, height, crown_from);
        let turn = Quat::from_rotation_y(rng.range(0.0, TAU));
        tree.transform(&Mat4::from_rotation_translation(
            turn,
            Vec3::new(p.x, h - 0.4, p.y),
        ));
        trees.merge(&tree);
        count += 1;
        if count >= 3000 {
            break;
        }
    }
    spawn_mesh(
        w,
        trees,
        white()
            .with_roughness(0.85)
            .with_detail(0.5, 0.15)
            .with_subsurface(0.3),
    );

    // A bank of denser mist rolling over the valley floor ahead.
    let bank = Vec3::new(valley_center(-480.0) - 40.0, 12.0, -480.0);
    w.spawn((
        Transform::from_translation(bank.as_dvec3()).with_rotation(Quat::from_rotation_y(-0.3)),
        FogVolume::ellipsoid([150.0, 22.0, 230.0])
            .with_density(0.03)
            .with_edge_falloff(14.0)
            .with_noise(0.75, 24.0)
            .with_anisotropy(0.65),
    ));
}

/// Center line (x) of the valley at `z`: nearly straight, so the rising sun shines along it.
fn valley_center(z: f32) -> f32 {
    15.0 * (z / 700.0).sin()
}

/// The valley's terrain: a floor about 500 m wide, slopes rising to rolling ridges 250-400 m
/// high with rocky crests, a 120 m cliff at its head (the lookout, at z = 0) and low hills
/// closing it far away.
fn valley_height(x: f32, z: f32) -> f32 {
    let d = (x - valley_center(z)).abs();
    let walls = ((d - 250.0).max(0.0) / 400.0).powf(1.4) * 260.0;
    let hills = (fbm(x / 600.0, z / 600.0, 5, 51) - 0.45) * 200.0 * smooth(220.0, 700.0, d);
    let crests = (1.0 - (2.0 * fbm(x / 140.0, z / 140.0, 4, 53) - 1.0).abs())
        * 25.0
        * smooth(280.0, 600.0, d);
    let bumps = (fbm(x / 35.0, z / 35.0, 4, 52) - 0.5) * 3.0;
    let cliff = 120.0 * smooth(-45.0, -5.0, z) + 0.1 * z.max(0.0);
    let far_hills = smooth(-3000.0, -6000.0, z) * (100.0 + 50.0 * fbm(x / 300.0, z / 300.0, 3, 54));
    walls + hills + crests + bumps + cliff + far_hills
}

// ------------------------------------------------------------------------------------ helpers

fn spawn_camera(w: &mut World, eye: Vec3, target: Vec3, fov_y: f32, ev100: f32) {
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(fov_y)
        },
        Transform::from_translation(eye.as_dvec3()).looking_at(target.as_dvec3(), Vec3::Y),
    ));
}

fn spawn_sun(w: &mut World, toward_sun: Vec3, lux: f32) {
    w.spawn((
        DirectionalLight {
            illuminance: lux,
            ..Default::default()
        },
        Transform::default().looking_at((-toward_sun).as_dvec3(), Vec3::Y),
    ));
}

/// Direction toward the sun: `azimuth` degrees clockwise from -z seen from above, `elevation`
/// degrees above the horizon.
fn sun_toward(azimuth: f32, elevation: f32) -> Vec3 {
    let (az, el) = (azimuth.to_radians(), elevation.to_radians());
    Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos())
}

/// A mesh in world coordinates.
fn spawn_mesh(w: &mut World, mesh: Mesh, material: StandardMaterial) {
    let handle = w.resource_mut::<Assets<Mesh>>().add(mesh);
    w.spawn((Transform::default(), MeshRenderer::new(handle, material)));
}

fn spawn_box(w: &mut World, center: Vec3, size: Vec3, material: StandardMaterial) {
    w.spawn((
        Transform::from_translation(center.as_dvec3()),
        MeshRenderer::new(
            Shape::Cuboid {
                size: size.to_array(),
            },
            material,
        ),
    ));
}

/// White base color: the meshes carry their colors per vertex.
fn white() -> StandardMaterial {
    StandardMaterial::color(1.0, 1.0, 1.0)
}

fn paint(m: &mut Mesh, color: Vec3) {
    for v in &mut m.vertices {
        v.color = color.extend(1.0).to_array();
    }
}

/// A height field around `center` (x, z) reaching `half` meters to each side. Its cells grow
/// from about `near_cell` meters at the center to coarse ones at the edges, so the ground is
/// detailed where the camera stands and still reaches the horizon.
fn terrain(
    center: Vec2,
    half: f32,
    cells: u32,
    near_cell: f32,
    height: impl Fn(f32, f32) -> f32,
    color: impl Fn(Vec3, Vec3) -> Vec3,
) -> Mesh {
    let a = (near_cell * cells as f32 / (2.0 * half)).clamp(0.01, 1.0);
    let warp = |u: f32| half * (a * u + (1.0 - a) * u * u * u);
    let mut m = Mesh::default();
    for j in 0..=cells {
        for i in 0..=cells {
            let u = i as f32 / cells as f32 * 2.0 - 1.0;
            let v = j as f32 / cells as f32 * 2.0 - 1.0;
            let (x, z) = (center.x + warp(u), center.y + warp(v));
            let p = Vec3::new(x, height(x, z), z);
            m.vertices
                .push(Vertex::new(p, Vec3::Y, Vec2::new(x, z) * 0.25));
        }
    }
    let row = cells + 1;
    for j in 0..cells {
        for i in 0..cells {
            let k = j * row + i;
            m.indices
                .extend_from_slice(&[k, k + row, k + 1, k + 1, k + row, k + row + 1]);
        }
    }
    m.compute_normals();
    for v in &mut m.vertices {
        let c = color(Vec3::from(v.position), Vec3::from(v.normal));
        v.color = c.extend(1.0).to_array();
    }
    m
}

/// A pine with its base at the origin: a tapering trunk and whorls of drooping boughs from
/// `crown_from` (a fraction of the height) to the top.
fn conifer(rng: &mut Rng, height: f32, crown_from: f32) -> Mesh {
    let bark = Vec3::new(0.11, 0.075, 0.055) * rng.range(0.7, 1.2);
    let needles = Vec3::new(0.035, 0.06, 0.03) * rng.range(0.7, 1.25);
    let r = 0.06 + height * 0.012;
    let mut m = mesh::cylinder(r, r * 0.3, height, 12);
    m.transform(&Mat4::from_translation(Vec3::Y * height * 0.5));
    paint(&mut m, bark);
    let crown = height * (1.0 - crown_from);
    // Whorls closer together on young trees, so small trees aren't a few big cones.
    let whorls = (crown / (0.45 + 0.04 * height)).clamp(5.0, 16.0) as u32;
    let widest = height * rng.range(0.11, 0.15);
    for t in 0..whorls {
        let f = t as f32 / whorls as f32;
        let y0 = height * crown_from + crown * f * 0.95;
        let tip = y0 + crown * 2.4 / whorls as f32;
        let radius = (widest * (1.0 - f).powf(0.9) + 0.25) * rng.range(0.85, 1.1);
        let shade = needles * rng.range(0.85, 1.15);
        whorl(&mut m, rng, y0, tip, radius, shade);
    }
    m
}

/// One whorl of boughs: a star of drooping branch tips around the trunk at `y0`, rising to a
/// point at `tip`, hollow underneath.
fn whorl(m: &mut Mesh, rng: &mut Rng, y0: f32, tip: f32, radius: f32, color: Vec3) {
    const POINTS: u32 = 16;
    let phase = rng.range(0.0, TAU);
    let lean = Vec3::new(rng.range(-0.06, 0.06), 0.0, rng.range(-0.06, 0.06)) * radius;
    let rim: Vec<Vec3> = (0..POINTS)
        .map(|s| {
            let branch = s % 2 == 0;
            let a = (s as f32 + rng.range(-0.2, 0.2)) / POINTS as f32 * TAU + phase;
            let (r, droop) = if branch {
                let r = radius * rng.range(0.7, 1.05);
                (r, r * rng.range(0.25, 0.45))
            } else {
                let r = radius * rng.range(0.3, 0.5);
                (r, r * 0.1)
            };
            Vec3::new(a.cos() * r, y0 - droop, -a.sin() * r) + lean
        })
        .collect();
    // Lighter at the tips, darker inside the tree and underneath.
    let tint = |p: Vec3, under: f32| {
        let out = (p.x * p.x + p.z * p.z).sqrt() / radius.max(1e-3);
        (color * (0.5 + 0.55 * out) * under).extend(1.0)
    };
    let outward = |p: Vec3| Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X);
    // Upper side: one shared apex, normals leaning out like a rounded crown.
    let base = m.vertices.len() as u32;
    m.vertices.push(
        Vertex::new(Vec3::new(0.0, tip, 0.0), Vec3::Y, Vec2::ZERO)
            .with_color((color * 0.9).extend(1.0)),
    );
    for &p in &rim {
        let n = (outward(p) + Vec3::Y * 0.8).normalize();
        m.vertices
            .push(Vertex::new(p, n, Vec2::new(p.x, p.z)).with_color(tint(p, 1.0)));
    }
    for s in 0..POINTS {
        let a = base + 1 + s;
        let b = base + 1 + (s + 1) % POINTS;
        m.indices.extend_from_slice(&[a, b, base]);
    }
    // Underside, hollow like real boughs.
    let base = m.vertices.len() as u32;
    let hollow = Vec3::new(0.0, y0 + (tip - y0) * 0.3, 0.0);
    m.vertices
        .push(Vertex::new(hollow, Vec3::NEG_Y, Vec2::ZERO).with_color((color * 0.25).extend(1.0)));
    for &p in &rim {
        let n = (outward(p) * 0.5 - Vec3::Y).normalize();
        m.vertices
            .push(Vertex::new(p, n, Vec2::new(p.x, p.z)).with_color(tint(p, 0.6)));
    }
    for s in 0..POINTS {
        let a = base + 1 + s;
        let b = base + 1 + (s + 1) % POINTS;
        m.indices.extend_from_slice(&[base, b, a]);
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Xorshift random numbers: the same scene every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32
}

/// Smooth value noise, 0..1.
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (i, j) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = hash(i, j, seed) + (hash(i + 1, j, seed) - hash(i, j, seed)) * ux;
    let b = hash(i, j + 1, seed) + (hash(i + 1, j + 1, seed) - hash(i, j + 1, seed)) * ux;
    a + (b - a) * uy
}

/// Fractal noise, about 0..1.
fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 0.5, 0.0, 1.0);
    for o in 0..octaves {
        sum += noise(x * f + o as f32 * 17.3, y * f - o as f32 * 9.1, seed + o) * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm
}
