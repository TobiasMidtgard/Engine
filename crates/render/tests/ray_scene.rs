//! The frame's ray scene (`trial_cut_more::RAY_WORLD`, `ray_scene/mod.rs`): on ray-tracing
//! hardware every ray of the frame (probe light, world reflections, the sun's measuring rays)
//! traces one scene that the features fill on the render thread, instead of the scene probe GI
//! gathers for itself. The same triangles, so each scene of `probe_gi.rs` is lit as before; a
//! mirror shows a smooth ball round and a wall in its vertex paint (the triangle records), and
//! a checkered wall in its checks, as bright as its colour (the tiles); a thing that moves is in the rays where it went and one that leaves is gone; the sun's rays
//! are stopped where the shadow falls, by what casts it or by its stand-in, and pass what
//! casts none; a thing inside its stand-in is not shaded by it; a dense mesh is in the rays at
//! a coarse level of detail that casts its shadow; a mesh drawn only as clusters is in the
//! rays; the scene follows the camera as
//! it walks and when it jumps; a world shift leaves the structures as built, whatever is
//! turned, stretched or hangs on another thing; more movers than the moving structure holds
//! wait their turn, and so do more things at rest than the still one holds; a mesh that takes
//! another's handle is a geometry of its own; things that keep moving are lit as things at
//! rest; a mirror keeps up while the probe grid scrolls; the switch can be thrown while the
//! game runs; a mirror shows the wall behind a cut-out card through the card's holes; each
//! kind of thing is in the rays as its class; the reaches are settings; each row of a
//! `MeshInstances` slot is an instance of its own; and the measuring fill fills both
//! structures to their caps. Needs a GPU with ray queries (lavapipe has them); passes with a
//! note without.

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::rock::rock;
use engine_render::settings::shade_skip;
use engine_render::settings::trial_cut_more::RAY_WORLD;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;
/// The room's inside: 6 m square, 3 m high, the floor's top at y = 0 (as in `probe_gi.rs`).
const HALF: f64 = 3.0;
const HEIGHT: f64 = 3.0;
const THICK: f64 = 0.3;

/// Probe GI on ray-tracing hardware, its rays through the frame's ray scene (`ray_world`) or
/// through the scene the probes gather themselves.
fn app(ray_world: bool) -> App {
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
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probe_spacing = 1.0;
        s.gi_probe_grid = [16, 8, 16];
        s.gi_probes_per_frame = ALL_PROBES;
        s.gi_hysteresis = 0.5;
    }
    set_ray_world(&mut app, ray_world);
    app
}

/// Probes the frames update (`RenderSettings::gi_probes_per_frame`): all of them here, so a
/// picture settles in a few frames.
const ALL_PROBES: u32 = 16 * 8 * 16;
/// A few, for the frames of a test that only waits or counts (a frame's probe rays are most
/// of its time on a software GPU).
const FEW_PROBES: u32 = 16;

fn set_probes(app: &mut App, probes: u32) {
    app.world
        .resource_mut::<RenderSettings>()
        .gi_probes_per_frame = probes;
}

fn set_ray_world(app: &mut App, on: bool) {
    let mut s = app.world.resource_mut::<RenderSettings>();
    if on {
        s.trial_cuts_more |= RAY_WORLD;
    } else {
        s.trial_cuts_more &= !RAY_WORLD;
    }
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

/// The frame's rays traced the frame's ray scene, with nothing lost so far (a loss shows in
/// the diagnostics the frame after it). Only where [`can_run`].
fn assert_in_ray_scene(app: &App, what: &str) {
    assert_eq!(
        diag(app, "render.ray.losses"),
        0.0,
        "{what}: the GPU lost the ray scene"
    );
    assert_eq!(
        diag(app, "render.gi.rt_losses"),
        0.0,
        "{what}: the GPU lost the probes' structures"
    );
    assert_eq!(diag(app, "render.ray.on"), 1.0, "{what}: no ray scene");
    assert_eq!(
        diag(app, "render.gi.ray_world"),
        1.0,
        "{what}: the probes trace a scene of their own"
    );
    assert_eq!(
        diag(app, "render.gi.hardware_rays"),
        1.0,
        "{what}: the rays fell back to software"
    );
}

/// The rays traced the probes' own scene on the hardware, and no ray scene was made.
fn assert_in_own_scene(app: &App, what: &str) {
    assert_eq!(diag(app, "render.gi.rt_losses"), 0.0, "{what}");
    assert_eq!(diag(app, "render.ray.on"), 0.0, "{what}: a ray scene");
    assert_eq!(diag(app, "render.gi.ray_world"), 0.0, "{what}");
    assert_eq!(diag(app, "render.gi.hardware_rays"), 1.0, "{what}");
}

fn slab(w: &mut World, center: DVec3, size: DVec3, albedo: f32) -> Entity {
    w.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(
            Shape::Cuboid {
                size: size.as_vec3().to_array(),
            },
            StandardMaterial::color(albedo, albedo, albedo).with_roughness(0.9),
        ),
    ))
    .id()
}

/// A crate 0.5 m square and 1 m high, standing on `at`.
fn crate_at(w: &mut World, at: DVec3) -> Entity {
    slab(w, at + DVec3::Y * 0.5, DVec3::new(0.5, 1.0, 0.5), 0.5)
}

/// A closed room centered on `at` (its floor's top), its walls `albedo`; `hole`: a square
/// opening of that size in the roof, its middle above `at + (0, 0, hole_z)`; `floor`: the
/// floor's albedo. Six slabs, nine with the opening.
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
        None => {
            slab(
                w,
                at + DVec3::new(0.0, roof_y, 0.0),
                DVec3::new(outer, t, outer),
                albedo,
            );
        }
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

fn camera(w: &mut World, eye: DVec3, target: DVec3, ev100: f32) -> Entity {
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ))
    .id()
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

/// The ground the rooms stand on, level with the underside of their floors.
fn ground(w: &mut World) {
    ground_at(w, -THICK);
}

fn ground_at(w: &mut World, y: f64) {
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, y, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
}

/// Looking at the far wall and the ceiling from near the back wall.
const EYE: DVec3 = DVec3::new(-0.5, 1.4, 2.4);
const TARGET: DVec3 = DVec3::new(0.3, 1.9, -3.0);
/// The far wall and ceiling, clear of the floor.
const UPPER: [f32; 4] = [0.15, 0.1, 0.85, 0.5];
const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// The room with an opening in its roof that the noon sun shines through, on the ground.
fn sunlit_room(w: &mut World) -> Entity {
    sunlit_room_on(w, -THICK)
}

/// [`sunlit_room`], its ground `ground_y` high.
fn sunlit_room_on(w: &mut World, ground_y: f64) -> Entity {
    room(w, DVec3::ZERO, 0.7, 0.8, Some((1.4, -0.5)));
    ground_at(w, ground_y);
    sun(w, 110_000.0, DVec3::new(0.05, -1.0, -0.1));
    camera(w, EYE, TARGET, 12.0)
}

/// The picture as the scene stands now.
fn picture(app: &mut App, name: &str) -> image::RgbImage {
    let dir = std::env::temp_dir().join("engine-ray-scene");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    image::open(&path).expect("read the shot").to_rgb8()
}

/// Mean brightness (0..255, linear-ish without a tonemapper) of the picture's rectangle
/// `(x0, y0, x1, y1)` in fractions of its size, as the scene stands now.
fn brightness(app: &mut App, name: &str, rect: [f32; 4]) -> f32 {
    let image = picture(app, name);
    let (x0, y0) = ((rect[0] * W as f32) as u32, (rect[1] * H as f32) as u32);
    let (x1, y1) = ((rect[2] * W as f32) as u32, (rect[3] * H as f32) as u32);
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = image.get_pixel(x, y);
            sum += (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f32
}

/// Two pictures of one scene are this far apart (of 255) where only the output's dither
/// differs (post.wgsl's dither moves each pixel a level either way, seeded by the clock).
const DITHER: u8 = 2;
/// How far the ray scene's pictures may be from those of the probes' own scene: the share of
/// pixels apart by more than the dither, the most any is, and the mean (the limits the
/// hardware rays' pictures are held to against the software walk's in `probe_gi.rs`: a wrong
/// face, side, transform or look moves whole regions by tens of levels).
const APART_SHARE: f32 = 0.005;
const APART_MOST: u8 = 32;
const APART_MEAN: f32 = 1.0;

/// How far apart two pictures are: the share of pixels apart by more than the dither, the
/// most any is, and the mean.
fn apart(a: &image::RgbImage, b: &image::RgbImage) -> (f32, u8, f32) {
    let (mut over, mut most, mut sum) = (0u32, 0u8, 0u64);
    for (a, b) in a.pixels().zip(b.pixels()) {
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        over += (d > DITHER) as u32;
        most = most.max(d);
        sum += d as u64;
    }
    let pixels = (W * H) as f32;
    (over as f32 / pixels, most, sum as f32 / pixels)
}

/// A lamp in the middle of room A at night; room B shares A's +X wall.
fn night(w: &mut World) {
    room(w, DVec3::ZERO, 0.7, 0.6, None);
    room(w, DVec3::X * (2.0 * HALF + THICK), 0.7, 0.6, None);
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
    camera(w, EYE, TARGET, 2.0);
}

/// A mirror facing the camera, and a red wall behind the camera that the sun lights: off
/// screen, so only world reflections show it in the mirror.
fn mirror(w: &mut World) {
    mirror_parts(w);
}

/// [`mirror`], and its camera and its red wall.
fn mirror_parts(w: &mut World) -> (Entity, Entity) {
    mirror_with(w, false)
}

/// [`mirror_parts`]. `stand_in`: the red wall casts no shadow itself and stands inside a
/// closed stand-in that casts for it (`ShadowOnly`), 8 cm larger on every side.
fn mirror_with(w: &mut World, stand_in: bool) -> (Entity, Entity) {
    // From behind the mirror, onto the red wall's face.
    mirror_lit(w, stand_in, DVec3::new(0.2, -0.8, 0.6))
}

/// [`mirror_with`], the sun's light going `toward`.
fn mirror_lit(w: &mut World, stand_in: bool, toward: DVec3) -> (Entity, Entity) {
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
    let red = MeshRenderer::new(
        Shape::Cuboid {
            size: [8.0, 4.0, 0.3],
        },
        StandardMaterial::color(0.8, 0.05, 0.05).with_roughness(0.9),
    );
    let at = Transform::from_translation(DVec3::new(0.0, 2.0, 4.0));
    let wall = if stand_in {
        w.spawn((
            at,
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [8.16, 4.16, 0.46],
                },
                StandardMaterial::color(0.5, 0.5, 0.5),
            ),
            ShadowOnly,
        ));
        w.spawn((at, red.without_shadows())).id()
    } else {
        w.spawn((at, red)).id()
    };
    sun(w, 110_000.0, toward);
    let camera = camera(
        w,
        DVec3::new(0.0, 1.5, 1.0),
        DVec3::new(0.0, 1.5, -3.0),
        14.0,
    );
    (camera, wall)
}

/// How much redder than its other channels the middle of a picture is (0 = gray, 1 = pure
/// red).
fn red_share(image: &image::RgbImage) -> f32 {
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
    (r - 0.5 * (g + b)) / r.max(1.0)
}

/// Boxes turned, stretched unevenly and mirrored (an odd number of negative scales), every
/// other one single-sided, in the room with the opening in its roof: the rays must find each
/// box's faces, and which side of a one-sided face they hit.
fn turned_boxes(w: &mut World) {
    turned_boxes_on(w, -THICK);
}

/// [`turned_boxes`], the ground `ground_y` high.
fn turned_boxes_on(w: &mut World, ground_y: f64) {
    sunlit_room_on(w, ground_y);
    for i in 0..24 {
        let f = i as f32;
        let at = DVec3::new(
            -2.4 + (i % 6) as f64 * 0.95,
            0.4 + (i / 12) as f64 * 1.6,
            -2.2 + ((i / 6) % 2) as f64 * 2.4,
        );
        let mirror = if i % 3 == 0 { -1.0 } else { 1.0 };
        let scale = Vec3::new(
            mirror * (0.5 + 0.35 * (f * 1.7).sin().abs()),
            0.4 + 0.5 * (f * 0.9).cos().abs(),
            0.3 + 0.6 * (f * 2.3).sin().abs(),
        );
        let rotation = Quat::from_euler(EulerRot::YXZ, f * 0.61, f * 0.37 - 1.0, f * 0.23);
        let tint = 0.3 + 0.6 * (f * 0.5).sin().abs();
        w.spawn((
            Transform::from_translation(at)
                .with_rotation(rotation)
                .with_scale(scale),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.0, 1.0, 1.0],
                },
                StandardMaterial {
                    double_sided: i % 2 == 0,
                    ..StandardMaterial::color(tint, 0.9 - tint * 0.5, 0.5).with_roughness(0.8)
                },
            ),
        ));
    }
}

/// The scenes of `probe_gi.rs` (a closed room at noon, one lit by a sunlit patch on its
/// floor, one by a lamp at night, a mirror's world reflections, and boxes turned, stretched
/// and mirrored) are lit through the frame's ray scene as through the probes' own: the same
/// triangles with the same looks, found by the same hardware, so the same pictures but for
/// the output's dither and a ray through a triangle's very edge.
#[test]
fn the_ray_scene_lights_each_scene_as_the_probes_own_scene_does() {
    type Build = fn(&mut World);
    let scenes: [(&str, usize, Build); 5] = [
        ("closed", 12, |w| {
            room(w, DVec3::ZERO, 0.7, 0.5, None);
            ground(w);
            sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
            camera(w, EYE, TARGET, 12.0);
        }),
        ("patch", 16, |w| {
            sunlit_room(w);
        }),
        ("night", 16, night),
        ("mirror", 12, mirror),
        ("boxes", 16, turned_boxes),
    ];
    for (name, frames, scene) in scenes {
        let mut own = app(false);
        scene(&mut own.world);
        own.update_n(frames);
        if !can_run(&own) {
            return;
        }
        assert_in_own_scene(&own, name);
        let own_instances = diag(&own, "render.gi.instances");
        let own_picture = picture(&mut own, &format!("{name}_own.png"));

        let mut world = app(true);
        scene(&mut world.world);
        world.update_n(frames);
        assert_in_ray_scene(&world, name);
        let (instances, still, moving) = (
            diag(&world, "render.ray.instances"),
            diag(&world, "render.ray.still_instances"),
            diag(&world, "render.ray.moving_instances"),
        );
        let world_picture = picture(&mut world, &format!("{name}_world.png"));
        let (share, most, mean) = apart(&own_picture, &world_picture);
        eprintln!(
            "{name}: {own_instances} instances in the probes' own scene, {instances} in the ray \
             scene ({still} still, {moving} moving); the pictures {:.3}% apart by more than the \
             dither, at most {most}, mean {mean:.3}",
            share * 100.0
        );
        assert!(own_instances > 0.0, "{name}: nothing in the probes' scene");
        // Everything here is in reach, and the ray scene takes a thing by its bounding
        // sphere where the probes' gather takes its box.
        assert!(
            instances >= own_instances,
            "{name}: {instances} instances in the ray scene, {own_instances} in the probes' own"
        );
        assert_eq!(
            (still, moving),
            (instances, 0.0),
            "{name}: a scene at rest is all in the still structure"
        );
        assert_eq!(diag(&world, "render.ray.parked"), 0.0, "{name}");
        assert_eq!(diag(&world, "render.ray.refused"), 0.0, "{name}");
        if name == "mirror" {
            let red = red_share(&world_picture);
            assert!(
                red > 0.25,
                "the mirror shows the red wall behind the camera: red share {red:.2}"
            );
        }
        assert!(
            share <= APART_SHARE && most <= APART_MOST && mean <= APART_MEAN,
            "{name}: the ray scene's picture is {:.3}% apart from the probes' own scene's by \
             more than the dither, at most {most}, mean {mean:.3}",
            share * 100.0
        );
    }
}

/// Brightness (0..255, the mean of the three channels) of the pixels of column `x`, rows `rows`,
/// each the mean of the column and its two neighbours.
fn column(image: &image::RgbImage, x: u32, rows: std::ops::Range<u32>) -> Vec<f32> {
    rows.map(|y| {
        let mut sum = 0.0;
        for x in x - 1..=x + 1 {
            let p = image.get_pixel(x, y);
            sum += (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
        }
        sum / 3.0
    })
    .collect()
}

/// The largest second difference along `line`, and how far it spans (most less least).
fn bends(line: &[f32]) -> (f32, f32) {
    let bend = line
        .windows(3)
        .map(|w| (w[0] - 2.0 * w[1] + w[2]).abs())
        .fold(0.0, f32::max);
    let most = line.iter().copied().fold(f32::MIN, f32::max);
    let least = line.iter().copied().fold(f32::MAX, f32::min);
    (bend, most - least)
}

/// Behind the camera, seen only in a mirror (world reflections): a smooth ball and its faceted
/// twin in the sun, and a white wall painted green in its vertex colour. The rays read the
/// triangle records: the smooth ball's light runs smoothly over its faces where the faceted
/// one's steps from face to face, and the wall shows its paint (white without the records).
#[test]
fn a_mirror_shows_a_ball_round_and_a_painted_wall_in_its_paint() {
    let mut app = app(true);
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
    let ball = Shape::Sphere {
        radius: 1.0,
        segments: 12,
    }
    .mesh();
    let mut faceted = ball.clone();
    faceted.compute_flat_normals();
    let wall = Shape::Cuboid {
        size: [8.0, 4.0, 0.3],
    }
    .mesh()
    .with_vertex_color(Vec4::new(0.1, 0.8, 0.1, 1.0));
    let (ball, faceted, wall) = {
        let mut meshes = w.resource_mut::<Assets<Mesh>>();
        (meshes.add(ball), meshes.add(faceted), meshes.add(wall))
    };
    let gray = || StandardMaterial::color(0.45, 0.45, 0.45).with_roughness(0.9);
    // The smooth ball on the left of the mirror's picture, the faceted one on the right.
    for (mesh, x) in [(ball, -1.5), (faceted, 1.5)] {
        w.spawn((
            Transform::from_translation(DVec3::new(x, 1.5, 2.4)),
            MeshRenderer::new(mesh, gray()),
        ));
    }
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, 2.0, 4.0)),
        MeshRenderer::new(
            wall,
            StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.9),
        ),
    ));
    // From behind the mirror, onto the balls' and the wall's faces toward it.
    sun(w, 110_000.0, DVec3::new(0.2, -0.8, 0.6));
    // 40 degrees high: the mirror fills the picture, a ball's image is 35 pixels across.
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_translation(DVec3::new(0.0, 1.5, 1.0))
            .looking_at(DVec3::new(0.0, 1.5, -3.0), Vec3::Y),
    ));
    app.update_n(12);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "the mirror");
    let (tris, triangles, geometries) = (
        diag(&app, "render.ray.tris"),
        diag(&app, "render.ray.triangles"),
        diag(&app, "render.ray.geometries"),
    );
    assert_eq!(
        diag(&app, "render.ray.tris_refused"),
        0.0,
        "no geometry went without"
    );
    assert_eq!(
        diag(&app, "render.ray.retired"),
        0.0,
        "nothing was built again"
    );
    assert_eq!(
        tris,
        triangles + geometries,
        "every geometry here is the mesh pool's: a record a triangle and a header each"
    );
    // (20 bytes a record.) What the records need is what they hold: none went without.
    let held_mb = tris * 20.0 / (1u64 << 20) as f64;
    assert!(
        (diag(&app, "render.ray.tris_wanted_mb") - held_mb).abs() < 1e-4,
        "the records' need is what they hold"
    );
    let image = picture(&mut app, "mirror_records.png");
    // The balls' images: 17.7 pixels in radius around (53.4, 60) and (106.6, 60) (a focal
    // length of 164.8 pixels, 9.3 m away in the mirror). Columns a few pixels off their middles
    // (which run along an edge between two columns of faces), rows over the lit upper part
    // (above the terminator, clear of the outline).
    let rows = 47..65;
    let (smooth, smooth_span) = bends(&column(&image, 57, rows.clone()));
    let (stepped, stepped_span) = bends(&column(&image, 110, rows));
    // The wall between and above the balls.
    let (mut r, mut g, mut b, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for y in 26..38 {
        for x in 45..115 {
            let p = image.get_pixel(x, y);
            r += p[0] as f32;
            g += p[1] as f32;
            b += p[2] as f32;
            n += 1.0;
        }
    }
    let (r, g, b) = (r / n, g / n, b / n);
    let green = (g - 0.5 * (r + b)) / g.max(1.0);
    eprintln!(
        "records: {tris} for {triangles} triangles in {geometries} geometries; the smooth \
         ball's largest bend {smooth:.1} over {smooth_span:.0} levels, the faceted one's \
         {stepped:.1} over {stepped_span:.0}; the wall {r:.0} {g:.0} {b:.0}, green share \
         {green:.2}"
    );
    assert!(
        smooth_span > 40.0 && stepped_span > 40.0,
        "both balls are lit across their faces: {smooth_span:.0} and {stepped_span:.0} levels"
    );
    assert!(
        smooth * 3.0 < stepped,
        "the smooth ball is round in the mirror: its light bends by {smooth:.1} at most, the \
         faceted one's by {stepped:.1}"
    );
    assert!(
        green > 0.25,
        "the wall shows its green paint in the mirror: green share {green:.2} ({r:.0} {g:.0} {b:.0})"
    );
}

/// A mirror facing the camera (as in [`mirror_with`]), and behind the camera two walls the sun
/// lights side by side, 4 m wide each: on the left one of `checker` (2 x 2 cells), on the right
/// one of `gray`, the checker's mean. The camera looks at the walls (so that the checker is
/// drawn, and uploaded: a tile is filled from what the raster draws with).
fn checker_mirror(w: &mut World, checker: Handle<Image>, gray: f32) -> Entity {
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
    let checkered = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_base_color_texture(checker);
    let plain = StandardMaterial::color(gray, gray, gray).with_roughness(0.9);
    for (x, material) in [(-2.0, checkered), (2.0, plain)] {
        w.spawn((
            Transform::from_translation(DVec3::new(x, 2.0, 4.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [4.0, 4.0, 0.3],
                },
                material,
            ),
        ));
    }
    sun(w, 110_000.0, DVec3::new(0.2, -0.8, 0.6));
    camera(
        w,
        DVec3::new(0.0, 1.5, 1.0),
        DVec3::new(0.0, 1.5, 4.0),
        14.0,
    )
}

/// Turn `camera` to look at `target` from where it stands.
fn turn(app: &mut App, camera: Entity, target: DVec3) {
    let eye = app
        .world
        .get::<Transform>(camera)
        .expect("camera")
        .translation;
    let at = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    *app.world.get_mut::<Transform>(camera).expect("camera") = at;
    // (For a picture taken before the next update, which is what carries this over.)
    *app.world
        .get_mut::<GlobalTransform>(camera)
        .expect("camera") = at.into();
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Mean linear light of the pixels `[x0, y0, x1, y1)` (the output is sRGB-encoded; with no
/// tonemapper the decoded means compare as the light does).
fn linear_mean(image: &image::RgbImage, [x0, y0, x1, y1]: [u32; 4]) -> f32 {
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = image.get_pixel(x, y);
            sum += (0..3).map(|c| srgb_to_linear(p[c])).sum::<f32>() / 3.0;
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f32
}

/// The walls' cells in the mirror (pixels `[x0, y0, x1, y1)`, 2 px inside each edge): the
/// checker's four, and the same places on the gray wall. The faces stand 10.75 m away through
/// the mirror (the camera 3.95 m before it, the walls 6.8 m behind it); 70 degrees high and
/// 4:3, the two 8 m by 4 m span x 48-112 and y 40-72, meeting at x 80.
const CHECKER_CELLS: [[u32; 4]; 4] = [
    [50, 42, 62, 54],
    [66, 42, 78, 54],
    [50, 58, 62, 70],
    [66, 58, 78, 70],
];
const GRAY_CELLS: [[u32; 4]; 4] = [
    [82, 42, 94, 54],
    [98, 42, 110, 54],
    [82, 58, 94, 70],
    [98, 58, 110, 70],
];

/// A checkered wall behind the camera, seen in the mirror: its hits read its texture's tile, so
/// the mirror shows the checker, as bright on average as the gray wall beside it, whose colour
/// is the texture's mean (what every hit of the checkered wall had before the tiles: its row's
/// colour).
#[test]
fn a_mirror_shows_the_texture_of_a_wall_behind_the_camera() {
    let mut app = app(true);
    let (light, dark) = (200u8, 50u8);
    let gray = (srgb_to_linear(light) + srgb_to_linear(dark)) * 0.5;
    let checker = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(Image::checker(
            64,
            2,
            [light, light, light, 255],
            [dark, dark, dark, 255],
        ));
    let camera = checker_mirror(&mut app.world, checker, gray);
    app.update_n(4);
    if !can_run(&app) {
        return;
    }
    turn(&mut app, camera, DVec3::new(0.0, 1.5, -3.0));
    app.update_n(12);
    assert_in_ray_scene(&app, "the checkered wall");
    let image = picture(&mut app, "tiles_mirror.png");
    let checker = CHECKER_CELLS.map(|r| linear_mean(&image, r));
    let plain = GRAY_CELLS.map(|r| linear_mean(&image, r));
    let (tiles, waiting, filled) = (
        diag(&app, "render.ray.tiles"),
        diag(&app, "render.ray.tiles_waiting"),
        diag(&app, "render.ray.tiles_filled"),
    );
    eprintln!(
        "the walls' cells in the mirror (linear): checker {checker:?}, gray {plain:?}; {tiles} \
         tiles, {waiting} waiting, {filled} filled this frame"
    );
    assert_eq!(
        (tiles, waiting),
        (1.0, 0.0),
        "the checker's texture has its tile"
    );
    let spread = |m: &[f32; 4]| {
        m.iter().fold(f32::MIN, |a, &b| a.max(b)) - m.iter().fold(f32::MAX, |a, &b| a.min(b))
    };
    let mean = |m: &[f32; 4]| m.iter().sum::<f32>() / 4.0;
    assert!(
        spread(&checker) > 0.5 * mean(&checker),
        "the mirror shows the checker: {checker:?}"
    );
    assert!(
        spread(&plain) < 0.1 * mean(&plain),
        "the gray wall shows even: {plain:?}"
    );
    let ratio = mean(&checker) / mean(&plain);
    assert!(
        (ratio - 1.0).abs() < 0.1,
        "the checker is {ratio:.3} times as bright as the gray wall beside it"
    );
}

/// A lid over the roof's opening slides aside while the scene runs: it is in the moving
/// structure where it went (the room lights up as much as one whose lid lay aside from the
/// start), and back in the still one once it has lain still for a while. A lid taken away
/// leaves the rays: the room is lit like one that never had it.
#[test]
fn a_lid_that_slides_or_leaves_lets_the_sun_in() {
    let lid_over = DVec3::new(0.0, HEIGHT + THICK * 1.5, -0.5);
    let lid_aside = DVec3::new(2.2, HEIGHT + THICK * 1.5, -0.5);
    let scene = |lid_at: Option<DVec3>| {
        let mut app = app(true);
        let w = &mut app.world;
        sunlit_room(w);
        let lid = lid_at.map(|at| slab(w, at, DVec3::new(1.8, THICK, 2.0), 0.7));
        (app, lid)
    };
    let (mut slid, lid) = scene(Some(lid_over));
    let lid = lid.expect("lid");
    slid.update_n(16);
    if !can_run(&slid) {
        return;
    }
    assert_in_ray_scene(&slid, "lid on");
    let instances = diag(&slid, "render.ray.instances");
    assert_eq!(instances, 11.0, "the room, the ground and the lid");
    assert_eq!(diag(&slid, "render.ray.moving_instances"), 0.0);
    assert_eq!(
        diag(&slid, "render.ray.structures_built"),
        0.0,
        "nothing is built for a scene at rest"
    );
    let closed = brightness(&mut slid, "lid_closed.png", UPPER);

    // Slide it aside over a few frames.
    for step in 1..=4 {
        slid.world
            .get_mut::<Transform>(lid)
            .expect("lid")
            .translation = lid_over.lerp(lid_aside, step as f64 / 4.0);
        slid.update();
        assert_eq!(
            (
                diag(&slid, "render.ray.moving_instances"),
                diag(&slid, "render.ray.instances")
            ),
            (1.0, instances),
            "the lid moves: it alone is in the moving structure"
        );
        assert_eq!(diag(&slid, "render.meshes.ray_sweeping"), 0.0);
    }
    slid.update_n(16);
    assert_in_ray_scene(&slid, "lid slid");
    let slid_aside = brightness(&mut slid, "lid_slid.png", UPPER);

    let (mut open, _) = scene(Some(lid_aside));
    open.update_n(20);
    let open = brightness(&mut open, "lid_open.png", UPPER);
    eprintln!("lid on {closed:.1}, slid aside {slid_aside:.1}, never on {open:.1}");
    assert!(
        open > closed * 1.3 + 3.0,
        "the opening should light the room: lid on {closed:.1}, never on {open:.1}"
    );
    assert!(
        (slid_aside - open).abs() < (open - closed) * 0.15 + 1.0,
        "lid slid aside {slid_aside:.1}, never on {open:.1}, on {closed:.1}"
    );

    // Lying still, it joins the still structure again (the meshes hold a thing that moved
    // still after 90 frames), and the room stays as lit.
    set_probes(&mut slid, FEW_PROBES);
    slid.update_n(90);
    assert_eq!(
        (
            diag(&slid, "render.ray.moving_instances"),
            diag(&slid, "render.ray.still_instances")
        ),
        (0.0, instances),
        "the lid at rest is back in the still structure"
    );
    set_probes(&mut slid, ALL_PROBES);
    slid.update_n(8);
    assert_in_ray_scene(&slid, "lid at rest");
    let at_rest = brightness(&mut slid, "lid_at_rest.png", UPPER);
    assert!(
        (at_rest - open).abs() < (open - closed) * 0.15 + 1.0,
        "lid at rest aside {at_rest:.1}, never on {open:.1}"
    );

    // A lid taken away.
    let (mut gone, lid) = scene(Some(lid_over));
    gone.update_n(16);
    gone.world.despawn(lid.expect("lid"));
    gone.update();
    assert_eq!(
        diag(&gone, "render.ray.instances"),
        instances - 1.0,
        "the lid left the rays the frame it went"
    );
    gone.update_n(20);
    assert_in_ray_scene(&gone, "lid gone");
    let gone = brightness(&mut gone, "lid_gone.png", UPPER);
    let (mut never, _) = scene(None);
    never.update_n(20);
    let never = brightness(&mut never, "lid_never.png", UPPER);
    eprintln!("lid taken away {gone:.1}, never there {never:.1}");
    assert!(
        (gone - never).abs() < (never - closed) * 0.15 + 1.0,
        "lid taken away {gone:.1}, never there {never:.1}, on {closed:.1}"
    );
}

/// The ground seen from above with no sky in view, the sun from the side, and (`roof`) a thin
/// roof 3 m square 2 m up whose shadow falls beside it. The sun's rays on, and counted.
fn under_the_sun(roof: bool) -> App {
    let mut app = app(true);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.ray_sun_shadows = true;
        s.debug_skip = engine_render::settings::shade_skip::COUNT_MESH_RUNS;
    }
    let w = &mut app.world;
    ground(w);
    sun(w, 110_000.0, DVec3::new(1.0, -1.0, 0.0));
    camera(w, DVec3::new(0.0, 7.0, 5.0), DVec3::ZERO, 14.0);
    if roof {
        slab(w, DVec3::new(0.0, 2.0, 0.0), DVec3::new(3.0, 0.1, 3.0), 0.3);
    }
    app
}

/// The sun's rays counted in a frame of the scene as it stands: rays shot, rays stopped. A
/// count is read back frames after it was taken, so four pictures are taken first: each waits
/// for the GPU, and by the fourth the count read is one taken since the first.
fn sun_counts(app: &mut App) -> (u32, u32) {
    for _ in 0..4 {
        picture(app, "sun_count.png");
    }
    let count = |name: &str| {
        let n = diag(app, name);
        assert!(n >= 0.0, "{name}: the rays are counted");
        n as u32
    };
    (count("render.gi.sun_rays"), count("render.gi.sun_hits"))
}

/// `RenderSettings::ray_sun_shadows` in the frame's one ray pass: every pixel the prepass drew
/// shoots a ray toward the sun through the ray scene. Over open ground none is stopped; under
/// a roof the rays stopped are the pixels the shadow maps leave dark.
#[test]
fn the_suns_rays_are_stopped_where_the_shadow_falls() {
    let mut open = under_the_sun(false);
    open.update_n(8);
    if !can_run(&open) {
        return;
    }
    assert_in_ray_scene(&open, "open ground");
    let (rays, hits) = sun_counts(&mut open);
    assert_eq!(
        rays,
        W * H,
        "a ray from every pixel: the ground fills the view"
    );
    assert_eq!(hits, 0, "nothing stands between open ground and the sun");

    let mut roofed = under_the_sun(true);
    roofed.update_n(8);
    assert_in_ray_scene(&roofed, "under the roof");
    let (rays, hits) = sun_counts(&mut roofed);
    assert_eq!(rays, W * H);
    // The pixels the frame draws in shadow: under half the open ground's brightness.
    let lit = brightness(&mut open, "sun_open.png", WHOLE);
    let image = picture(&mut roofed, "sun_roof.png");
    let dark = image
        .pixels()
        .filter(|p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0 < lit * 0.5)
        .count() as u32;
    eprintln!(
        "{hits} of {rays} rays stopped; {dark} pixels drawn in shadow (open ground {lit:.1})"
    );
    assert!(
        dark > 300,
        "the roof's shadow is in view: {dark} dark pixels"
    );
    assert!(
        hits.abs_diff(dark) * 8 <= dark,
        "the rays stopped ({hits}) are the pixels in shadow ({dark})"
    );

    // The same roof casting no shadow (`MeshRenderer::without_shadows`): it is in the scene,
    // for the rays that ask what they meet, and the sun's rays pass it as the shadow maps do.
    let mut unshadowed = under_the_sun(false);
    unshadowed.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 2.0, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 0.1, 3.0],
            },
            StandardMaterial::color(0.3, 0.3, 0.3).with_roughness(0.9),
        )
        .without_shadows(),
    ));
    unshadowed.update_n(8);
    assert_in_ray_scene(&unshadowed, "under a roof that casts no shadow");
    assert_eq!(
        diag(&unshadowed, "render.ray.instances"),
        2.0,
        "the ground and the roof are both in the scene"
    );
    let (rays, hits) = sun_counts(&mut unshadowed);
    assert_eq!(rays, W * H);
    assert_eq!(hits, 0, "the sun's rays pass a roof that casts no shadow");

    // That roof with a stand-in that casts for it (`ShadowOnly`, a little smaller, inside
    // it): the shadow maps draw the stand-in's shadow, and the stand-in stops the sun's rays.
    unshadowed.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 2.0, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [2.9, 0.06, 2.9],
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
        ShadowOnly,
    ));
    unshadowed.update_n(8);
    assert_in_ray_scene(&unshadowed, "under a roof with a stand-in");
    assert_eq!(
        diag(&unshadowed, "render.ray.instances"),
        3.0,
        "the ground, the roof and its stand-in are in the scene"
    );
    let (rays, hits) = sun_counts(&mut unshadowed);
    assert_eq!(rays, W * H);
    let image = picture(&mut unshadowed, "sun_stand_in.png");
    let dark = image
        .pixels()
        .filter(|p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0 < lit * 0.5)
        .count() as u32;
    eprintln!("with a stand-in: {hits} rays stopped; {dark} pixels drawn in shadow");
    assert!(
        dark > 300,
        "the stand-in's shadow is in view: {dark} dark pixels"
    );
    assert!(
        hits.abs_diff(dark) * 8 <= dark,
        "the rays the stand-in stops ({hits}) are the pixels in its shadow ({dark})"
    );
}

/// A thing that casts no shadow may stand inside the stand-in that casts for it (a far wall's
/// core does, 8 cm inside its block). A shadow ray from the thing's surface starts inside
/// the stand-in and is not stopped on its way out: a mirror shows the sunlit red wall behind
/// the camera as bright when a stand-in around the wall casts for it as when the wall casts
/// for itself.
#[test]
fn a_thing_inside_its_shadow_stand_in_is_lit() {
    // The mean red of the picture's middle (the mirror), 0..255.
    let red = |image: &image::RgbImage| {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for y in H * 3 / 10..H * 7 / 10 {
            for x in W * 3 / 10..W * 7 / 10 {
                sum += image.get_pixel(x, y)[0] as f32;
                n += 1.0;
            }
        }
        sum / n
    };
    let run = |stand_in: bool| {
        let mut app = app(true);
        mirror_with(&mut app.world, stand_in);
        app.update_n(12);
        app
    };
    let mut plain = run(false);
    if !can_run(&plain) {
        return;
    }
    assert_in_ray_scene(&plain, "the wall that casts for itself");
    assert_eq!(diag(&plain, "render.ray.stand_ins"), 0.0);
    let plain_picture = picture(&mut plain, "stand_in_plain.png");
    let mut inside = run(true);
    assert_in_ray_scene(&inside, "the wall inside its stand-in");
    assert_eq!(
        diag(&inside, "render.ray.stand_ins"),
        1.0,
        "the stand-in is in the scene, counted as one"
    );
    let inside_picture = picture(&mut inside, "stand_in_inside.png");
    let (plain_red, inside_red) = (red(&plain_picture), red(&inside_picture));
    eprintln!(
        "the mirror's red: {plain_red:.1} with the wall casting for itself, {inside_red:.1} \
         with the wall inside a stand-in (red share {:.2} and {:.2})",
        red_share(&plain_picture),
        red_share(&inside_picture)
    );
    assert!(
        red_share(&plain_picture) > 0.25 && plain_red > 150.0,
        "the mirror shows the sunlit red wall: red {plain_red:.1}"
    );
    assert!(
        inside_red > plain_red * 0.9,
        "the wall inside its stand-in is shaded by it: red {inside_red:.1}, {plain_red:.1} \
         when it casts for itself"
    );
}

/// A closed sphere of 66k triangles around the camera, dense enough to be drawn as clusters
/// once those are built. Until then the mesh pool holds it, and the scene builds it straight
/// from the pool's ranges: it is in the rays the frame it shows up. Once the pool has let it
/// go, a new scene builds it from a copy of its triangles made on the main thread. Either
/// way it keeps most of the noon sky out.
/// A dense stone is in the rays whole in the frame it shows up and, once its levels of detail
/// are built, at the coarsest of them that stays within 3 cm of it: fewer triangles in the
/// scene and the same shadow on the ground. Placed again 16 times as large, the one structure
/// both share is built again at a finer level.
#[test]
fn a_dense_mesh_is_in_the_rays_at_a_coarse_level() {
    // The ground alone for 6 frames, then the stone: 4 m across, 12 m up the sun's way from
    // the middle of the view and outside it, so that only its shadow is in view. Returns the
    // ground's triangles and the stone's.
    let scene = |levels: bool| {
        let mut app = under_the_sun(false);
        if !levels {
            app.world.resource_mut::<RenderSettings>().mesh_lod_pixels = 0.0;
        }
        set_probes(&mut app, FEW_PROBES);
        app.update_n(6);
        let ground = diag(&app, "render.ray.triangles");
        let mesh = rock(Vec3::new(1.0, 0.6, 0.8), 7, 5);
        let whole = (mesh.indices.len() / 3) as f64;
        let stone = app.world.resource_mut::<Assets<Mesh>>().add(mesh);
        app.world.spawn((
            Transform::from_translation(DVec3::new(-12.0, 12.0, 0.0)).with_scale(Vec3::splat(4.0)),
            MeshRenderer::new(
                stone,
                StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9),
            ),
        ));
        (app, stone, ground, whole)
    };

    // Without levels of detail the stone stays whole.
    let (mut full, _, ground, whole) = scene(false);
    if !can_run(&full) {
        return;
    }
    assert!(
        ground > 0.0,
        "the ground is in the rays before the stone shows up"
    );
    full.update_n(8);
    assert_in_ray_scene(&full, "the stone, whole");
    assert_eq!(diag(&full, "render.ray.triangles"), ground + whole);
    let (rays, hits_full) = sun_counts(&mut full);
    assert_eq!(rays, W * H, "the stone is not in view");
    assert!(hits_full > 100, "its shadow is: {hits_full} rays stopped");
    drop(full);

    // With them it is whole in the frame it shows up ...
    let (mut app, stone, ground, whole) = scene(true);
    for _ in 0..8 {
        app.update();
        if diag(&app, "render.ray.instances") == 2.0 {
            break;
        }
    }
    assert_eq!(diag(&app, "render.ray.instances"), 2.0);
    assert_eq!(
        diag(&app, "render.ray.triangles"),
        ground + whole,
        "whole at once: the levels are not waited for"
    );
    // ... and at its ray level once they are built (a worker's time: frames and wall time
    // bound the wait) and its first structure is 30 frames old.
    let started = std::time::Instant::now();
    let mut frames = 0;
    while diag(&app, "render.ray.triangles") >= ground + whole
        && (frames < 600 || started.elapsed() < std::time::Duration::from_secs(120))
    {
        app.update();
        frames += 1;
        assert_eq!(
            diag(&app, "render.meshes.ray_waiting"),
            0.0,
            "the whole stone serves"
        );
        assert_eq!(diag(&app, "render.ray.instances"), 2.0);
    }
    let coarse = diag(&app, "render.ray.triangles") - ground;
    eprintln!("the stone's ray level: {coarse} of {whole} triangles after {frames} frames");
    // (A level keeps at most 80% of the one above: `mesh_lod::MIN_GAIN`.)
    assert!(
        (16.0..=whole * 0.8).contains(&coarse),
        "{coarse} of {whole} triangles"
    );
    assert_eq!(
        diag(&app, "render.ray.geometries"),
        2.0,
        "the same geometry, built again"
    );
    app.update_n(48);
    assert_in_ray_scene(&app, "the stone at its ray level");
    // (Read 30 frames after the structures were last touched; the whole stone's structure
    // goes once the GPU is past it, which touches them once more.)
    for _ in 0..120 {
        if diag(&app, "render.ray.blas_mb") > 0.0 {
            break;
        }
        app.update();
    }
    assert!(
        diag(&app, "render.ray.blas_mb") > 0.0,
        "the structures' memory is read"
    );
    let (rays, hits) = sun_counts(&mut app);
    assert_eq!(rays, W * H);
    assert!(
        hits.abs_diff(hits_full) * 8 <= hits_full,
        "the coarse stone casts the whole stone's shadow: {hits} rays stopped, {hits_full}"
    );

    // The same mesh at scale 64, behind the camera: 3 cm there is half a millimeter of the
    // mesh, so the structure both instances share is built again, finer.
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 0.0, 150.0)).with_scale(Vec3::splat(64.0)),
        MeshRenderer::new(stone, StandardMaterial::color(0.5, 0.5, 0.5)),
    ));
    app.update_n(40);
    let fine = diag(&app, "render.ray.triangles") - ground;
    eprintln!("placed at scale 64: {fine} triangles");
    assert!(
        fine > coarse,
        "{fine} triangles for the larger stone, {coarse} before"
    );
    assert_eq!(
        diag(&app, "render.ray.geometries"),
        2.0,
        "one structure serves both"
    );
    assert_eq!(diag(&app, "render.ray.instances"), 3.0);
    assert_eq!(diag(&app, "render.meshes.ray_waiting"), 0.0);
}

#[test]
fn a_mesh_drawn_as_clusters_is_in_the_rays() {
    let scene = |probe_gi: bool| {
        let mut app = app(true);
        {
            let mut s = app.world.resource_mut::<RenderSettings>();
            s.probe_gi = probe_gi;
            // (About the copy, which is the whole mesh: so is what the pool's copy gives, and
            // the counts and pictures of the two compare.)
            s.mesh_lod_pixels = 0.0;
        }
        let w = &mut app.world;
        w.spawn((
            Transform::from_translation(DVec3::new(0.0, 1.5, 0.0)),
            MeshRenderer::new(
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
    if !can_run(&app) {
        return;
    }
    let mut seen_after = None;
    for frame in 1..200 {
        if diag(&app, "render.ray.instances") == 1.0 {
            seen_after = Some(frame);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.update();
    }
    eprintln!(
        "the sphere is in the rays after {seen_after:?} frames ({} triangles in the scene)",
        diag(&app, "render.ray.triangles")
    );
    assert!(seen_after.is_some(), "the sphere never reached the rays");
    assert!(
        diag(&app, "render.ray.triangles") > 60_000.0,
        "all of the sphere's triangles are in the scene"
    );
    let triangles = diag(&app, "render.ray.triangles");
    app.update_n(2);
    assert_in_ray_scene(&app, "the sphere");
    assert_eq!(
        diag(&app, "render.meshes.ray_copies"),
        0.0,
        "a copy is dropped once its structure is built"
    );
    assert_eq!(diag(&app, "render.meshes.ray_waiting"), 0.0);

    // Its clusters are built on a worker; drawn only as clusters for a while, the pool lets
    // its copy go. A new ray scene then finds nothing of the sphere on the GPU to build from
    // and asks the main thread for a copy of its triangles.
    set_probes(&mut app, FEW_PROBES);
    // (Bounded by frames and by wall time: the clusters' build is a worker's, and how many
    // frames it takes is the machine's load.)
    let started = std::time::Instant::now();
    let mut freed_after = None;
    for frame in 0.. {
        if diag(&app, "render.meshes.resident") == 0.0 {
            freed_after = Some(frame);
            break;
        }
        if frame >= 600 && started.elapsed() > std::time::Duration::from_secs(120) {
            break;
        }
        app.update();
    }
    eprintln!("the pool let the sphere go after {freed_after:?} more frames");
    assert!(
        freed_after.is_some(),
        "the sphere was never drawn only as clusters"
    );
    assert_eq!(
        diag(&app, "render.ray.instances"),
        1.0,
        "the structure built from the pool outlives the pool's copy"
    );
    set_probes(&mut app, ALL_PROBES);
    app.update_n(12);
    let from_the_pool = brightness(&mut app, "sphere_from_the_pool.png", UPPER);
    // One probe a frame while the scene is switched: a probe traced while the scene is away
    // or still empty sees the sky, many times the light inside the sphere.
    set_probes(&mut app, 1);
    set_ray_world(&mut app, false);
    app.update_n(2);
    set_ray_world(&mut app, true);
    let mut seen_after = None;
    for frame in 1..30 {
        app.update();
        if diag(&app, "render.ray.instances") == 1.0 {
            seen_after = Some(frame);
            break;
        }
    }
    // (The meshes' numbers are a frame behind the scene's.)
    app.update_n(2);
    eprintln!(
        "in a new scene the sphere is in the rays after {seen_after:?} frames ({} copies made)",
        diag(&app, "render.meshes.ray_copied")
    );
    assert!(
        seen_after.is_some(),
        "the sphere never reached the new scene"
    );
    assert_eq!(
        diag(&app, "render.meshes.ray_copied"),
        1.0,
        "built from a copy of its triangles"
    );
    assert_eq!(diag(&app, "render.meshes.ray_copies"), 0.0);
    assert_eq!(diag(&app, "render.meshes.resident"), 0.0);
    assert_eq!(
        diag(&app, "render.ray.triangles"),
        triangles,
        "the copy holds the triangles the pool held"
    );
    // The light inside is what leaks between the probes, bounced many times, so the probes
    // that saw the sky meanwhile take some frames to settle; a hole in the copy would not.
    set_probes(&mut app, ALL_PROBES);
    let mut settling = Vec::new();
    for _ in 0..6 {
        app.update_n(12);
        settling.push(brightness(&mut app, "sphere.png", UPPER));
        if settling
            .last()
            .is_some_and(|on| (on - from_the_pool).abs() <= from_the_pool * 0.05)
        {
            break;
        }
    }
    eprintln!(
        "inside the sphere built from the pool {from_the_pool:.1}, from a copy {settling:.1?} \
         (every 12 frames)"
    );
    assert_in_ray_scene(&app, "the sphere from a copy");
    let on = settling.last().copied().unwrap_or(0.0);
    assert!(
        (on - from_the_pool).abs() <= from_the_pool * 0.1 + 1.0,
        "inside the sphere built from a copy {settling:.1?}, from the pool {from_the_pool:.1}"
    );
    let mut off = scene(false);
    off.update_n(6);
    let off = brightness(&mut off, "sphere_off.png", UPPER);
    // Without the probes the inside sees the sky through the guess.
    assert!(
        on < off * 0.7,
        "inside the sphere: {on:.1}, {off:.1} without probes"
    );
}

/// The scene holds what is within reach of the camera: every caster within 160 m, and past
/// that only what is large for its distance. Crates 300 m away come into the rays as the
/// camera walks toward them and leave as it walks back (the walk in turn finds them: nothing
/// about them changed); after a jump of 2 km the scene holds the crates there and nothing of
/// the room, and after the jump back the room again.
#[test]
fn the_scene_follows_the_camera() {
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    let w = &mut app.world;
    room(w, DVec3::ZERO, 0.7, 0.5, None);
    ground(w);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    let eye = camera(w, EYE, TARGET, 12.0);
    for i in 0..30 {
        let (x, z) = ((i % 6) as f64, (i / 6) as f64);
        crate_at(w, DVec3::new(300.0 + 2.0 * x, 0.0, 2.0 * z));
        crate_at(w, DVec3::new(2000.0 + 2.0 * x, 0.0, 2.0 * z));
    }
    app.update_n(8);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "at the room");
    let instances = |app: &App| diag(app, "render.ray.instances");
    let epoch = diag(&app, "render.ray.epoch");
    assert_eq!(instances(&app), 7.0, "the room and the ground");

    // Walk toward the crates 300 m away, 15 m a frame, and back.
    let walk = |app: &mut App, from: f64, to: f64| {
        for step in 1..=10 {
            let x = from + (to - from) * step as f64 / 10.0;
            app.world
                .get_mut::<Transform>(eye)
                .expect("camera")
                .translation = EYE + DVec3::X * x;
            app.update();
        }
        app.update_n(2);
    };
    walk(&mut app, 0.0, 150.0);
    assert_eq!(
        instances(&app),
        37.0,
        "150 m on: the room behind and the crates ahead"
    );
    assert_eq!(
        diag(&app, "render.ray.epoch"),
        epoch,
        "a walk is not a jump"
    );
    walk(&mut app, 150.0, 0.0);
    assert_eq!(instances(&app), 7.0, "back at the room");

    // Jump 2 km, and back.
    let jump = |app: &mut App, x: f64| {
        app.world
            .get_mut::<Transform>(eye)
            .expect("camera")
            .translation = EYE + DVec3::X * x;
        app.update_n(3);
    };
    jump(&mut app, 2000.0);
    assert_eq!(instances(&app), 30.0, "the crates 2 km away, alone");
    assert_eq!(diag(&app, "render.ray.epoch"), epoch + 1.0, "a jump");
    assert_eq!(diag(&app, "render.meshes.ray_sweeping"), 0.0);
    jump(&mut app, 0.0);
    assert_eq!(instances(&app), 7.0, "back at the room after the jump");
    app.update_n(8);
    assert_eq!(diag(&app, "render.ray.moving_instances"), 0.0);
    assert_in_ray_scene(&app, "after the jumps");
}

/// A [`WorldShift`](engine_world::WorldShift) (a floating-origin move, or a jump that
/// re-anchors the world) moves every root transform. The scene moves what it holds with the
/// shift: a plain move leaves both structures as built (their instances stand around an
/// anchor that moved along), one that also turns the world builds them again; either way no
/// instance is set again, none comes or goes, and the closed room keeps the noon sky out as
/// before. That holds for things turned and stretched and for one that hangs far from its
/// root, where the game's rounding of a shift and the scene's differ most. And what was out
/// of reach when the world moved is no mover when the camera reaches it right after.
#[test]
fn a_world_shift_moves_the_scene_along() {
    let turn = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
    let shifts = [
        (
            "move",
            engine_world::WorldShift::translate(DVec3::new(3000.0, 0.0, -1700.0)),
        ),
        (
            "turn",
            engine_world::WorldShift {
                rotation: turn,
                ..engine_world::WorldShift::translate(DVec3::new(-850.0, 0.0, 420.0))
            },
        ),
    ];
    for (name, shift) in shifts {
        let mut app = app(true);
        app.add_message::<engine_world::WorldShift>();
        let w = &mut app.world;
        room(w, DVec3::ZERO, 0.7, 0.5, None);
        sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
        let eye = camera(w, EYE, TARGET, 12.0);
        for i in 0..3 {
            crate_at(w, DVec3::new(-1.5 + i as f64, 0.0, -1.0));
        }
        // Outside the room: boxes turned and stretched (the last sixteen times), and one that
        // hangs 30 m from its turned root. The game turns each root's `f32` rotation, rounds
        // it and places the children from it; the scene turns each placement in `f64`. The two
        // differ by rounding that grows with a thing's scale and its distance from its root.
        let gray = || StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9);
        let unit = Shape::Cuboid {
            size: [1.0, 1.0, 1.0],
        };
        for i in 0..6 {
            let f = i as f32;
            let scale = if i == 5 {
                Vec3::splat(16.0)
            } else {
                Vec3::new(0.5 + 0.3 * f, 0.8, 1.0 + 0.2 * f)
            };
            w.spawn((
                Transform::from_translation(DVec3::new(-40.0 + 12.0 * i as f64, 6.0, 30.0))
                    .with_rotation(Quat::from_euler(
                        EulerRot::YXZ,
                        f * 0.61 + 0.2,
                        f * 0.37 - 1.0,
                        f * 0.23,
                    ))
                    .with_scale(scale),
                MeshRenderer::new(unit, gray()),
            ));
        }
        let root = w
            .spawn(
                Transform::from_translation(DVec3::new(-20.0, 0.0, -30.0))
                    .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.9, 0.4, -0.3)),
            )
            .id();
        w.spawn((
            ChildOf(root),
            Transform::from_translation(DVec3::new(30.0, 2.0, 5.0))
                .with_rotation(Quat::from_rotation_x(0.7))
                .with_scale(Vec3::splat(2.0)),
            MeshRenderer::new(unit, gray()),
        ));
        // 2000 crates 1-2 km away.
        for i in 0..2000 {
            let (x, z) = ((i % 50) as f64, (i / 50) as f64);
            crate_at(w, DVec3::new(1000.0 + 20.0 * x, 0.0, 20.0 * z));
        }
        app.update_n(16);
        if !can_run(&app) {
            return;
        }
        assert_in_ray_scene(&app, name);
        let instances = diag(&app, "render.ray.instances");
        assert_eq!(
            instances, 16.0,
            "{name}: the room, the crates in it and the boxes around it"
        );
        let before = brightness(&mut app, &format!("shift_{name}_before.png"), UPPER);

        // The shift, as `engine_world` applies it: every root transform moves.
        let mut roots = app
            .world
            .query_filtered::<&mut Transform, Without<ChildOf>>();
        for mut t in roots.iter_mut(&mut app.world) {
            *t = shift.transform(&t);
        }
        app.world.write_message(shift);
        app.update();
        let (built, set) = (
            diag(&app, "render.ray.structures_built"),
            diag(&app, "render.ray.slots_set"),
        );
        eprintln!(
            "{name}: structures built {built}, slots set {set}, the meshes looked at {} slots \
             in {:.2} ms",
            diag(&app, "render.meshes.ray_looked"),
            diag(&app, "render.meshes.ray_ms")
        );
        if name == "move" {
            assert_eq!(built, 0.0, "{name}: the structures stay as built");
        } else {
            assert_eq!(
                built as u32 & 1,
                1,
                "{name}: the still structure is built again"
            );
        }
        assert_eq!(
            (
                diag(&app, "render.ray.instances"),
                diag(&app, "render.ray.still_instances")
            ),
            (instances, instances),
            "{name}: every instance stays, in the still structure"
        );

        // The crates far away were not in the scene when the world moved. The camera reaches
        // them well within the 90 frames a thing that moved takes to count as still: they hold
        // still all the same (the shift moved them as it moved everything), and so does the
        // room when the camera is back.
        let home = app.world.get::<Transform>(eye).expect("camera").translation;
        app.world
            .get_mut::<Transform>(eye)
            .expect("camera")
            .translation = shift.point(DVec3::new(1510.0, 1.4, 410.0));
        app.update_n(3);
        let (far, still, moving, parked) = (
            diag(&app, "render.ray.instances"),
            diag(&app, "render.ray.still_instances"),
            diag(&app, "render.ray.moving_instances"),
            diag(&app, "render.ray.parked"),
        );
        eprintln!("{name}: at the far crates {far} instances: {still} still, {moving} moving");
        assert!(
            far > 150.0 && far < 260.0,
            "{name}: the crates within 160 m of the camera, {far}"
        );
        assert_eq!(
            (still, moving, parked),
            (far, 0.0, 0.0),
            "{name}: what comes into reach after a shift holds still"
        );
        app.world
            .get_mut::<Transform>(eye)
            .expect("camera")
            .translation = home;

        // The probes start over after the jump; light that came in through the first frames'
        // guesses bounces out over a few dozen frames (as after any jump).
        app.update_n(60);
        assert_in_ray_scene(&app, name);
        assert_eq!(
            (
                diag(&app, "render.ray.instances"),
                diag(&app, "render.ray.still_instances"),
                diag(&app, "render.ray.moving_instances")
            ),
            (instances, instances, 0.0),
            "{name}: back at the room"
        );
        let after = brightness(&mut app, &format!("shift_{name}_after.png"), UPPER);
        eprintln!("{name}: the room {before:.1} before the shift, {after:.1} after");
        assert!(
            (after - before).abs() <= before * 0.15 + 2.0,
            "{name}: the room {before:.1} before the shift, {after:.1} after"
        );
    }
}

/// More things move at once than the moving structure holds (2048): the rest wait outside
/// the rays, counted, and nothing is lost. Once they all lie still they are all in the still
/// structure.
#[test]
fn more_movers_than_the_moving_structure_holds_wait_their_turn() {
    const CRATES: usize = 2200;
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    let w = &mut app.world;
    room(w, DVec3::ZERO, 0.7, 0.5, None);
    ground(w);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(w, EYE, TARGET, 12.0);
    // Around the room, all within 80 m of it.
    let crates: Vec<Entity> = (0..CRATES)
        .map(|i| {
            let (x, z) = ((i % 50) as f64, (i / 50) as f64);
            crate_at(w, DVec3::new(-49.0 + 2.0 * x, 0.0, 6.0 + 1.5 * z))
        })
        .collect();
    app.update_n(8);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "at rest");
    let all = (CRATES + 7) as f64;
    assert_eq!(
        (
            diag(&app, "render.ray.still_instances"),
            diag(&app, "render.ray.moving_instances"),
            diag(&app, "render.ray.parked")
        ),
        (all, 0.0, 0.0),
        "at rest, everything is in the still structure"
    );

    for step in 0..3 {
        for &e in &crates {
            app.world
                .get_mut::<Transform>(e)
                .expect("crate")
                .translation
                .y += 0.05;
        }
        app.update();
        let (still, moving, parked) = (
            diag(&app, "render.ray.still_instances"),
            diag(&app, "render.ray.moving_instances"),
            diag(&app, "render.ray.parked"),
        );
        eprintln!(
            "step {step}: {still} still, {moving} moving, {parked} waiting (the meshes looked \
             at {} slots in {:.2} ms, the scene's gather {:.2} ms)",
            diag(&app, "render.meshes.ray_looked"),
            diag(&app, "render.meshes.ray_ms"),
            diag(&app, "render.ray.ms_gather")
        );
        assert_eq!(
            (still, moving, parked),
            (7.0, 2048.0, (CRATES - 2048) as f64),
            "every crate moves: the moving structure is full and the rest wait"
        );
        assert_eq!(diag(&app, "render.ray.instances"), all);
    }
    assert_in_ray_scene(&app, "moving");

    // At rest again: still after 90 frames.
    app.update_n(96);
    assert_eq!(
        (
            diag(&app, "render.ray.still_instances"),
            diag(&app, "render.ray.moving_instances"),
            diag(&app, "render.ray.parked")
        ),
        (all, 0.0, 0.0),
        "at rest again, everything is back in the still structure"
    );
    assert_in_ray_scene(&app, "at rest again");
}

/// More things stand in reach than the still structure holds (16,384): the rest wait outside
/// the rays, counted. One that moves leaves its slot to one that waited; at rest again it
/// stays in the moving structure, and the still one is not built again and again for it.
/// When things leave, the ones that waited and the rested mover take their slots. Among this
/// many slots (the walk in turn takes eight frames to come round), a mesh that loads after
/// its entity was spawned, as streamed content does, is in the rays as soon as it is drawn.
#[test]
fn more_things_than_the_still_structure_holds_wait_for_room() {
    const SIDE: usize = 129;
    const CRATES: usize = SIDE * SIDE;
    const ROOM: f64 = 16_384.0;
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    let w = &mut app.world;
    ground(w);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(w, EYE, TARGET, 12.0);
    // A square of crates 1.5 m apart around the camera, the farthest 140 m away: all within
    // the casters' reach. Among them, things whose mesh is not loaded yet.
    let mut crates = Vec::with_capacity(CRATES);
    let mut late = Vec::new();
    for i in 0..CRATES {
        let at = DVec3::new(
            -96.0 + 1.5 * (i % SIDE) as f64,
            0.0,
            -96.0 + 1.5 * (i / SIDE) as f64,
        );
        crates.push(crate_at(w, at));
        if i % 800 == 400 {
            let mesh = w
                .resource_mut::<Assets<Mesh>>()
                .reserve_keyed(&format!("late {i}"));
            w.spawn((
                Transform::from_translation(at + DVec3::Y * 3.0),
                MeshRenderer::new(mesh, StandardMaterial::color(0.5, 0.5, 0.5)),
            ));
            late.push(mesh);
        }
    }
    app.update_n(10);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "full");
    let counts = |app: &App| {
        (
            diag(app, "render.ray.still_instances"),
            diag(app, "render.ray.moving_instances"),
            diag(app, "render.ray.parked"),
        )
    };
    let all = (CRATES + 1) as f64;
    assert_eq!(diag(&app, "render.ray.instances"), all);
    assert_eq!(
        counts(&app),
        (ROOM, 0.0, all - ROOM),
        "the still structure is full and the rest wait"
    );

    // One crate is nudged: it is a mover, and one that waited has its slot.
    app.world
        .get_mut::<Transform>(crates[0])
        .expect("crate")
        .translation
        .y += 0.05;
    app.update_n(10);
    assert_eq!(
        counts(&app),
        (ROOM, 1.0, all - ROOM - 1.0),
        "the mover is in the moving structure, and one that waited took its slot"
    );
    // At rest it would go back, but the still structure is full: it stays where it is, and
    // nothing is built for it.
    app.update_n(90);
    for frame in 0..8 {
        app.update();
        assert_eq!(
            diag(&app, "render.ray.structures_built"),
            0.0,
            "frame {frame}: no structure is built for a mover at rest that has no room"
        );
    }
    assert_eq!(counts(&app), (ROOM, 1.0, all - ROOM - 1.0));
    assert_in_ray_scene(&app, "full, a mover at rest");

    // 300 crates leave: the ones that waited and the rested mover take the room.
    for &e in &crates[1..301] {
        app.world.despawn(e);
    }
    app.update_n(12);
    let left = all - 300.0;
    assert_eq!(diag(&app, "render.ray.instances"), left);
    assert_eq!(
        counts(&app),
        (left, 0.0, 0.0),
        "everything left is in the still structure"
    );

    // The late meshes load, all in one frame. The walk in turn would take eight frames to
    // have looked at each of their slots.
    for (k, &mesh) in late.iter().enumerate() {
        let size = 0.6 + 0.01 * k as f32;
        app.world
            .resource_mut::<Assets<Mesh>>()
            .set(mesh, Shape::Cuboid { size: [size; 3] }.mesh());
    }
    app.update_n(3);
    assert_eq!(
        diag(&app, "render.ray.instances"),
        left + late.len() as f64,
        "a mesh that loads after its entity is in the rays as soon as it is drawn"
    );
    app.update_n(5);
    assert_eq!(counts(&app), (left + late.len() as f64, 0.0, 0.0));
    assert_in_ray_scene(&app, "after the late meshes");
}

/// A mesh's handle index goes to another mesh once the first is removed (a zone streamed out
/// and another in): the newcomer is a geometry of its own at once, not a newer version of the
/// one before, whose triangles the rays would trace in its place until it is built again. A
/// mesh edited in place keeps its geometry's name: its instance stays, with the shape before
/// until the new one is built (half a second after the last build at 60 frames a second).
#[test]
fn a_reused_mesh_handle_is_another_geometry_and_an_edited_mesh_the_same() {
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    // (About names and edits: the meshes stay whole, so their triangles add up.)
    app.world.resource_mut::<RenderSettings>().mesh_lod_pixels = 0.0;
    ground(&mut app.world);
    sun(&mut app.world, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(&mut app.world, EYE, TARGET, 12.0);
    let add = |app: &mut App, mesh: Mesh| app.world.resource_mut::<Assets<Mesh>>().add(mesh);
    let spawn = |app: &mut App, mesh: Handle<Mesh>| {
        app.world
            .spawn((
                Transform::from_translation(DVec3::new(0.0, 0.5, -2.0)),
                MeshRenderer::new(mesh, StandardMaterial::color(0.5, 0.5, 0.5)),
            ))
            .id()
    };
    let scene = |app: &App| {
        (
            diag(app, "render.ray.instances"),
            diag(app, "render.ray.geometries"),
            diag(app, "render.ray.triangles"),
        )
    };
    let first = add(&mut app, Shape::Cuboid { size: [1.0; 3] }.mesh());
    let thing = spawn(&mut app, first);
    app.update_n(6);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "the first mesh");
    let (instances, geometries, triangles) = scene(&app);
    assert_eq!(
        (instances, geometries),
        (2.0, 2.0),
        "the ground and the box"
    );

    // The zone leaves and another comes in the same frame: the box's mesh is removed, and a
    // sphere takes its handle index.
    app.world.despawn(thing);
    app.world.resource_mut::<Assets<Mesh>>().remove(first);
    let sphere = Shape::Sphere {
        radius: 0.5,
        segments: 16,
    }
    .mesh();
    let sphere_triangles = (sphere.indices.len() / 3) as f64;
    let second = add(&mut app, sphere);
    assert_eq!(
        second.index(),
        first.index(),
        "the index is given out again"
    );
    spawn(&mut app, second);
    app.update_n(3);
    assert_in_ray_scene(&app, "the second mesh");
    assert_eq!(
        scene(&app),
        (2.0, geometries + 1.0, triangles + sphere_triangles),
        "the sphere is a geometry of its own, beside the box's (forgotten later)"
    );

    // The sphere's mesh is edited in place: a finer sphere under the same handle.
    let finer = Shape::Sphere {
        radius: 0.5,
        segments: 32,
    }
    .mesh();
    let finer_triangles = (finer.indices.len() / 3) as f64;
    assert!(finer_triangles > sphere_triangles);
    app.world.resource_mut::<Assets<Mesh>>().set(second, finer);
    app.update_n(2);
    assert_eq!(
        scene(&app),
        (2.0, geometries + 1.0, triangles + sphere_triangles),
        "the instance stays, with the shape before, while the edited mesh waits to be built"
    );
    app.update_n(36);
    assert_eq!(
        scene(&app),
        (2.0, geometries + 1.0, triangles + finer_triangles),
        "the edited mesh's geometry is built again under its name"
    );
    assert_eq!(diag(&app, "render.ray.moving_instances"), 0.0);
    assert_in_ray_scene(&app, "the edited mesh");
}

/// A thing that moves is in the structure that is built every frame, and a ray that meets it
/// reads its look from the rows past the still structure's. Boxes that keep shifting are lit
/// as the probes' own scene lights them, within the limits a scene at rest is held to.
///
/// The ground lies 5 cm under the room's floor here, not against its underside as in the
/// other scenes. Of two surfaces in one plane a ray reports the one its structure comes to
/// first, which is the same in both scenes while they hold the same instances in one
/// structure each. Here the ray scene holds them in two: the probes on the floor, whose
/// downward rays end on that plane, took the floor's underside (the inside of something) in
/// one scene and the ground in the other, and lit 14 pixels between the boxes differently
/// with nothing moving at all.
#[test]
fn things_that_keep_moving_are_lit_as_the_probes_own_scene_lights_them() {
    let run = |ray_world: bool| {
        let mut app = app(ray_world);
        turned_boxes_on(&mut app.world, -THICK - 0.05);
        app.update_n(4);
        // The boxes: the only things here that are stretched.
        let boxes: Vec<Entity> = app
            .world
            .query_filtered::<(Entity, &Transform), With<MeshRenderer>>()
            .iter(&app.world)
            .filter(|(_, at)| at.scale != Vec3::ONE)
            .map(|(e, _)| e)
            .collect();
        assert_eq!(boxes.len(), 24);
        // Each steps 5 mm up, then back down, frame after frame.
        for frame in 0..16 {
            let step = if frame % 2 == 0 { 0.005 } else { -0.005 };
            for &b in &boxes {
                app.world
                    .get_mut::<Transform>(b)
                    .expect("box")
                    .translation
                    .y += step;
            }
            app.update();
        }
        app
    };
    let mut own = run(false);
    if !can_run(&own) {
        return;
    }
    assert_in_own_scene(&own, "moving boxes");
    let own_picture = picture(&mut own, "moving_own.png");
    let mut world = run(true);
    assert_in_ray_scene(&world, "moving boxes");
    let (instances, still, moving) = (
        diag(&world, "render.ray.instances"),
        diag(&world, "render.ray.still_instances"),
        diag(&world, "render.ray.moving_instances"),
    );
    assert_eq!(
        (moving, still),
        (24.0, instances - 24.0),
        "the boxes are in the moving structure, the room in the still one"
    );
    let world_picture = picture(&mut world, "moving_world.png");
    let (share, most, mean) = apart(&own_picture, &world_picture);
    eprintln!(
        "{moving} moving and {still} still: the pictures {:.3}% apart by more than the dither, \
         at most {most}, mean {mean:.3}",
        share * 100.0
    );
    assert!(
        share <= APART_SHARE && most <= APART_MOST && mean <= APART_MEAN,
        "moving boxes: the ray scene's picture is {:.3}% apart from the probes' own scene's by \
         more than the dither, at most {most}, mean {mean:.3}",
        share * 100.0
    );
}

/// In a frame whose probe grid scrolled the world reflections run after the probes' atlases
/// were copied, in a pass of their own. A camera that steps across a probe cell's edge every
/// frame scrolls the grid every frame: its mirror shows the scene as it is now (the red wall
/// behind the camera has left), not the reflections of the last frame that stood still.
#[test]
fn a_mirror_keeps_up_while_the_probe_grid_scrolls() {
    let mut app = app(true);
    let (camera, wall) = mirror_parts(&mut app.world);
    app.update_n(12);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "the mirror");
    let before = red_share(&picture(&mut app, "scroll_before.png"));
    assert!(
        before > 0.25,
        "the mirror shows the red wall behind the camera: red share {before:.2}"
    );
    // The wall leaves, and the camera stands 0.6 m to either side of a probe cell's edge in
    // turn (the probes are 1 m apart), looking the same way.
    app.world.despawn(wall);
    let stand = |app: &mut App, step: u32| {
        let x = if step.is_multiple_of(2) { 0.6 } else { -0.6 };
        let eye = DVec3::new(x, 1.5, 1.0);
        let at = Transform::from_translation(eye).looking_at(eye - DVec3::Z * 4.0, Vec3::Y);
        *app.world.get_mut::<Transform>(camera).expect("camera") = at;
        // (For a picture taken before the next update, which is what carries this over.)
        *app.world
            .get_mut::<GlobalTransform>(camera)
            .expect("camera") = at.into();
    };
    for step in 0..8 {
        stand(&mut app, step);
        app.update();
        assert!(
            step == 0 || diag(&app, "render.gi.scrolled_in") > 0.0,
            "step {step}: the probe grid scrolled"
        );
    }
    stand(&mut app, 8);
    let after = red_share(&picture(&mut app, "scroll_after.png"));
    eprintln!(
        "the mirror's red share: {before:.2} with the wall, {after:.2} after it left with the \
         grid scrolling every frame"
    );
    assert!(
        diag(&app, "render.gi.scrolled_in") > 0.0,
        "the picture's frame scrolled the grid too"
    );
    assert_in_ray_scene(&app, "the mirror, scrolling");
    assert!(
        after < before * 0.2,
        "the mirror still shows the wall that left: red share {after:.2}, {before:.2} with it"
    );
}

/// The switch is thrown while the game runs (as the GPU sweep's `ray_world` row does at every
/// stop): the rays go back to the probes' own scene and the ray scene is dropped, then a new
/// one is made and filled, with nothing lost, and each lights the room and the mirror as
/// before. In between, a switch every frame, each dropping a scene the GPU may still trace.
#[test]
fn the_ray_scene_is_switched_while_the_game_runs() {
    type Build = fn(&mut World);
    let scenes: [(&str, Build); 2] = [
        ("patch", |w| {
            sunlit_room(w);
        }),
        ("mirror", mirror),
    ];
    for (name, scene) in scenes {
        let mut app = app(true);
        scene(&mut app.world);
        app.update_n(16);
        if !can_run(&app) {
            return;
        }
        assert_in_ray_scene(&app, name);
        let instances = diag(&app, "render.ray.instances");
        let world = brightness(&mut app, &format!("switch_{name}_world.png"), WHOLE);
        set_ray_world(&mut app, false);
        app.update_n(16);
        assert_in_own_scene(&app, name);
        assert_eq!(
            (
                diag(&app, "render.ray.instances"),
                diag(&app, "render.meshes.ray_looked")
            ),
            (0.0, 0.0),
            "{name}: the dropped scene's numbers do not stand beside `render.ray.on` 0"
        );
        let own = brightness(&mut app, &format!("switch_{name}_own.png"), WHOLE);
        for frame in 0..6 {
            set_ray_world(&mut app, frame % 2 == 0);
            app.update();
        }
        set_ray_world(&mut app, true);
        app.update_n(16);
        assert_in_ray_scene(&app, name);
        assert_eq!(
            diag(&app, "render.ray.instances"),
            instances,
            "{name}: the new scene holds what the first did"
        );
        let again = brightness(&mut app, &format!("switch_{name}_world_again.png"), WHOLE);
        eprintln!(
            "{name}: ray scene {world:.2}, the probes' own {own:.2}, ray scene again {again:.2}"
        );
        assert!(
            world > 3.0,
            "{name}: the picture is dark ({world:.2}): nothing to compare"
        );
        let near = |a: f32| (a - world).abs() <= world * 0.06 + 1.0;
        assert!(
            near(own) && near(again),
            "{name}: ray scene {world:.2}, the probes' own {own:.2}, ray scene again {again:.2}"
        );
    }
}

/// How much more of channel `c` than of the other two the pixels `[x0, y0, x1, y1)` hold (0:
/// gray; 1: only `c`).
fn share(image: &image::RgbImage, [x0, y0, x1, y1]: [u32; 4], c: usize) -> f32 {
    let mut s = [0.0f32; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            for (sum, v) in s.iter_mut().zip(image.get_pixel(x, y).0) {
                *sum += v as f32;
            }
        }
    }
    (s[c] - 0.5 * (s.iter().sum::<f32>() - s[c])) / s[c].max(1.0)
}

/// The leaf card's four cells in the mirror (pixels `[x0, y0, x1, y1)`, 4 px inside each edge:
/// reflections are traced at half size and filtered), and the red wall beside it. Through the
/// mirror the card is 9.7 m away and the wall 10.75 m; 40 degrees high, the card spans x
/// 60-100 and y 31-72, its cells meeting at (80, 51.5).
const CARD_CELLS: [[u32; 4]; 4] = [
    [64, 36, 76, 48],
    [84, 36, 96, 48],
    [64, 56, 76, 68],
    [84, 56, 96, 68],
];
const WALL_BESIDE: [[u32; 4]; 2] = [[26, 36, 52, 68], [108, 36, 134, 68]];

/// The cut-out card of [`a_ray_passes_the_holes_of_a_cut_out_card`], its texture `texture`.
fn cut_out_card(w: &mut World, texture: Handle<Image>) -> Entity {
    let mut leaf = StandardMaterial::color(0.05, 0.8, 0.05)
        .with_roughness(0.9)
        .with_base_color_texture(texture)
        .with_alpha_mask(0.5);
    leaf.double_sided = true;
    w.spawn((
        Transform::from_xyz(0.0, 2.0, 2.8)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        MeshRenderer::new(Shape::Quad { size: [2.4, 2.4] }, leaf),
    ))
    .id()
}

/// Turn the camera onto the card behind it and back to the mirror (`frames` frames there): a
/// card's tile is filled from the texture the raster uploads.
fn look_at_card_and_back(app: &mut App, camera: Entity, frames: usize) {
    turn(app, camera, DVec3::new(0.0, 1.5, 4.0));
    app.update_n(4);
    turn(app, camera, DVec3::new(0.0, 1.5, -3.0));
    app.update_n(frames);
}

/// A card cut out by its texture hangs between the camera and the red wall behind it: the
/// mirror shows the wall through the card's clear cells, lit as through a card whose texture
/// is clear everywhere, and the card's green in its opaque ones (world reflections ask each of
/// a cut-out's triangles). (Probe light takes any cut-out whole: behind the card, the wall is
/// darker than beside it either way.) The card holds still, a fence more than a leaf: it is
/// `Solid`, its triangles asked as a leaf's, and no canopy map stands for it. A steep sun
/// casts its shadow on the ground before the wall, not on the wall seen through its holes.
/// With the measuring rung `ray_leaves_solid` every cell shows the card, and the opaque cells
/// are as they were.
#[test]
fn a_ray_passes_the_holes_of_a_cut_out_card() {
    let mut app = app(true);
    let checker = |a: u8, b: u8| Image::checker(64, 2, [255, 255, 255, a], [255, 255, 255, b]);
    let cutout = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(checker(255, 0));
    let (camera, _) = mirror_lit(&mut app.world, false, DVec3::new(0.2, -0.95, 0.25));
    *app.world.get_mut::<Camera>(camera).expect("camera") = Camera {
        exposure: Exposure::Manual { ev100: 14.0 },
        ..Camera::perspective(40.0)
    };
    let card = cut_out_card(&mut app.world, cutout);
    turn(&mut app, camera, DVec3::new(0.0, 1.5, 4.0));
    app.update_n(1);
    if !can_run(&app) {
        return;
    }
    look_at_card_and_back(&mut app, camera, 8);
    assert_in_ray_scene(&app, "the cut-out card");
    assert_eq!(
        (
            diag(&app, "render.ray.class_solid"),
            diag(&app, "render.ray.class_leaf")
        ),
        (4.0, 0.0),
        "the ground, the mirror, the wall and the card that holds still are solid"
    );
    assert_eq!(
        (
            diag(&app, "render.ray.tiles"),
            diag(&app, "render.ray.tiles_waiting")
        ),
        (1.0, 0.0),
        "the card's texture has its tile"
    );
    let holes = picture(&mut app, "leaf_card_holes.png");
    app.world.resource_mut::<RenderSettings>().debug_skip |= shade_skip::RAY_LEAVES_SOLID;
    app.update_n(2);
    let solid = picture(&mut app, "leaf_card_solid.png");
    // The same card, clear everywhere.
    app.world.resource_mut::<RenderSettings>().debug_skip &= !shade_skip::RAY_LEAVES_SOLID;
    let clear_texture = app.world.resource_mut::<Assets<Image>>().add(checker(0, 0));
    app.world.despawn(card);
    cut_out_card(&mut app.world, clear_texture);
    look_at_card_and_back(&mut app, camera, 8);
    assert_in_ray_scene(&app, "the clear card");
    let clear = picture(&mut app, "leaf_card_clear.png");
    let wall = WALL_BESIDE
        .map(|r| linear_mean(&holes, r))
        .iter()
        .sum::<f32>()
        / 2.0;
    let red = |p: &image::RgbImage| CARD_CELLS.map(|r| share(p, r, 0));
    let green = |p: &image::RgbImage| CARD_CELLS.map(|r| share(p, r, 1));
    let mean = |p: &image::RgbImage| CARD_CELLS.map(|r| linear_mean(p, r));
    eprintln!(
        "the card's cells in the mirror: red share {:?}, green {:?}, light {:?} (the wall \
         beside it {wall:.4}); with the rung: green {:?}, light {:?}; the clear card: red \
         share {:?}, light {:?}",
        red(&holes),
        green(&holes),
        mean(&holes),
        green(&solid),
        mean(&solid),
        red(&clear),
        mean(&clear)
    );
    for i in 0..4 {
        assert!(
            red(&clear)[i] > 0.25,
            "cell {i}: the wall through the clear card, red {:?}",
            red(&clear)
        );
    }
    let red = red(&holes);
    // (Which diagonal is clear follows the card's uv, its turn and the mirror.)
    let (open, kept) = if red[0] > red[1] {
        ([0, 3], [1, 2])
    } else {
        ([1, 2], [0, 3])
    };
    for i in open {
        assert!(
            red[i] > 0.25,
            "cell {i}: the wall through a hole, red {red:?}"
        );
        let light = mean(&holes)[i] / mean(&clear)[i];
        assert!(
            (light - 1.0).abs() < 0.06,
            "cell {i}: the wall through a hole is {light:.3} times as bright as through the \
             clear card"
        );
        assert!(
            green(&solid)[i] > 0.25,
            "cell {i}: whole with the rung, green {:?}",
            green(&solid)
        );
        let even = mean(&solid)[i] / mean(&solid)[kept[0]];
        assert!(
            (even - 1.0).abs() < 0.1,
            "cell {i}: {even:.3} times as bright as a kept cell with the rung"
        );
    }
    for i in kept {
        assert!(
            green(&holes)[i] > 0.25,
            "cell {i}: the card itself, green {:?}",
            green(&holes)
        );
        let same = mean(&holes)[i] / mean(&solid)[i];
        assert!(
            (same - 1.0).abs() < 0.03,
            "cell {i}: the rung leaves the card's texels alone ({same:.3})"
        );
    }
}

/// Each kind of thing is in the rays as its class (`RayClass`): what casts a shadow and holds
/// its shape is `Solid`, a fence cut out by its texture too, and a mask over no texture (drawn
/// whole); a crown's stand-in (swaying cards) `Leaf`; what sways in the wind `Sway`; a caster
/// under 0.3 m `Small`; what casts no shadow `NoCast`; a stand-in that casts as a closed shape
/// `ShadowOnly`. A small thing that casts nothing, and a swaying card that casts nothing (a
/// crown beside its stand-in), are in no ray's way. One mesh placed whole and cut out is two
/// geometries.
#[test]
fn each_kind_of_thing_is_in_the_rays_as_its_class() {
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    let holes = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(Image::checker(
            64,
            2,
            [255, 255, 255, 255],
            [255, 255, 255, 0],
        ));
    let w = &mut app.world;
    ground(w);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(w, EYE, TARGET, 12.0);
    let gray = StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9);
    let cut_out = gray.with_base_color_texture(holes).with_alpha_mask(0.5);
    let leaves = cut_out.with_wind_sway(0.2);
    let crate_mesh = w.resource_mut::<Assets<Mesh>>().add(
        Shape::Cuboid {
            size: [0.5, 1.0, 0.5],
        }
        .mesh(),
    );
    let cube = |size: f32| Shape::Cuboid { size: [size; 3] };
    let at = |x: f64| Transform::from_xyz(x, 0.5, -2.0);
    w.spawn((at(-4.0), MeshRenderer::new(crate_mesh, gray)));
    w.spawn((at(-3.0), MeshRenderer::new(crate_mesh, cut_out)));
    w.spawn((
        at(-5.0),
        MeshRenderer::new(crate_mesh, gray.with_alpha_mask(0.5)),
    ));
    w.spawn((at(-2.0), MeshRenderer::new(cube(0.2), gray)));
    w.spawn((
        at(-1.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.3, 2.0, 0.3],
            },
            gray.with_wind_sway(0.2),
        ),
    ));
    w.spawn((
        at(0.0),
        MeshRenderer::new(cube(1.0), gray).without_shadows(),
    ));
    w.spawn((
        at(1.0),
        MeshRenderer::new(cube(0.1), gray).without_shadows(),
    ));
    // A crown and its stand-in, and a stand-in that casts as a closed shape.
    w.spawn((
        at(2.5),
        MeshRenderer::new(cube(1.5), leaves).without_shadows(),
    ));
    w.spawn((at(2.5), MeshRenderer::new(cube(1.6), leaves), ShadowOnly));
    w.spawn((at(4.5), MeshRenderer::new(cube(0.8), gray), ShadowOnly));
    app.update_n(6);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "each kind");
    let classes = [
        ("solid", 4.0),
        ("ground", 0.0),
        ("sway", 1.0),
        ("leaf", 1.0),
        ("water", 0.0),
        ("no_cast", 1.0),
        ("small", 1.0),
        ("shadow_only", 1.0),
    ];
    for (class, expected) in classes {
        assert_eq!(
            diag(&app, &format!("render.ray.class_{class}")),
            expected,
            "class {class}"
        );
    }
    assert_eq!(diag(&app, "render.ray.instances"), 9.0);
    assert_eq!(diag(&app, "render.ray.stand_ins"), 1.0);
    // The ground, the crate's mesh twice (whole, and cut out: the crate under a mask over no
    // texture is whole), and each of the five others in the rays.
    assert_eq!(diag(&app, "render.ray.geometries"), 8.0);
}

/// The reaches are settings (`RenderSettings::ray_caster_reach`, `ray_far_reach`): a crate
/// past the casters' reach comes into the rays once the reach takes it in, and a wall past
/// the far reach leaves once the far reach falls short of it. Each change makes the scene look
/// at everything again (a new epoch).
#[test]
fn the_ray_scene_reaches_as_far_as_its_settings() {
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    let w = &mut app.world;
    ground(w);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(w, EYE, TARGET, 12.0);
    // 100 m away, within the casters' reach; 200 m away, too small for the far reach's cut;
    // a wall 300 m away, large enough for it.
    crate_at(w, DVec3::new(0.0, 0.0, -100.0));
    crate_at(w, DVec3::new(0.0, 0.0, -200.0));
    slab(
        w,
        DVec3::new(0.0, 4.0, -300.0),
        DVec3::new(20.0, 8.0, 1.0),
        0.5,
    );
    app.update_n(6);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "the default reaches");
    let instances = |app: &App| diag(app, "render.ray.instances");
    assert_eq!(
        instances(&app),
        3.0,
        "the ground, the near crate and the far wall"
    );
    let epoch = diag(&app, "render.ray.epoch");
    app.world.resource_mut::<RenderSettings>().ray_caster_reach = 250.0;
    app.update_n(6);
    assert!(diag(&app, "render.ray.epoch") > epoch, "a new epoch");
    assert_eq!(
        instances(&app),
        4.0,
        "the crate 200 m away is within the casters' reach"
    );
    app.world.resource_mut::<RenderSettings>().ray_far_reach = 260.0;
    app.update_n(6);
    assert_eq!(instances(&app), 3.0, "the wall past the far reach left");
    assert_in_ray_scene(&app, "the reaches changed");
}

/// Each row of a `MeshInstances` slot is an instance of its own in the rays, of one geometry,
/// placed where the raster draws it: a wall of red cubes drawn as rows behind the camera shows
/// in the mirror. A row past every reach is left out, rows taken away leave the rays, and the
/// entity drawn once is one instance again.
#[test]
fn each_row_of_mesh_instances_is_in_the_rays() {
    let mut app = app(true);
    let (_, wall) = mirror_parts(&mut app.world);
    app.world.despawn(wall);
    let mut rows: Vec<InstanceRow> = (0..32)
        .map(|k| InstanceRow {
            position: Vec3::new(-3.5 + (k % 8) as f32, 0.5 + (k / 8) as f32, 0.0),
            yaw: 0.0,
            scale: 1.0,
            ..Default::default()
        })
        .collect();
    // Out of every reach: 0.5 m across, 300 m away.
    rows.push(InstanceRow {
        position: Vec3::new(0.0, 0.5, 300.0),
        scale: 0.5,
        ..Default::default()
    });
    let region = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.0, 4.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.0, 1.0, 1.0],
                },
                StandardMaterial::color(0.8, 0.05, 0.05).with_roughness(0.9),
            ),
            MeshInstances::new(rows.clone()),
        ))
        .id();
    app.update_n(12);
    if !can_run(&app) {
        return;
    }
    assert_in_ray_scene(&app, "rows");
    let counts = |app: &App| {
        (
            diag(app, "render.meshes.ray_rows"),
            diag(app, "render.ray.instances"),
        )
    };
    // The ground, the mirror and the 32 rows in reach.
    assert_eq!(counts(&app), (32.0, 34.0));
    assert_eq!(diag(&app, "render.ray.geometries"), 3.0);
    let red = red_share(&picture(&mut app, "rows_mirror.png"));
    assert!(
        red > 0.25,
        "the mirror shows the wall of cubes behind the camera: red share {red:.2}"
    );
    app.world
        .get_mut::<MeshInstances>(region)
        .expect("rows")
        .rows = rows[..8].into();
    app.update_n(4);
    assert_eq!(counts(&app), (8.0, 10.0), "eight rows left");
    app.world.entity_mut(region).remove::<MeshInstances>();
    app.update_n(4);
    assert_eq!(counts(&app), (0.0, 3.0), "the entity drawn once");
    assert_in_ray_scene(&app, "fewer rows");
}

/// The measuring fill (`RenderSettings::ray_scene_stress`) fills both structures to their caps
/// with copies of what is there, each built in full every frame; it leaves the class counts,
/// the still set and the valve alone, and its copies go at 0.
#[test]
fn the_ray_scene_fills_to_its_caps_for_a_measurement() {
    const STILL: f64 = 16_384.0;
    const MOVING: f64 = 2_048.0;
    let mut app = app(true);
    set_probes(&mut app, FEW_PROBES);
    let w = &mut app.world;
    ground(w);
    sun(w, 110_000.0, DVec3::new(0.3, -1.0, -0.4));
    camera(w, EYE, TARGET, 12.0);
    crate_at(w, DVec3::new(0.0, 0.0, -3.0));
    app.update_n(6);
    if !can_run(&app) {
        return;
    }
    let counts = |app: &App| {
        (
            diag(app, "render.ray.still_instances"),
            diag(app, "render.ray.moving_instances"),
            diag(app, "render.ray.parked"),
        )
    };
    assert_eq!(counts(&app), (2.0, 0.0, 0.0));
    let copies = (STILL + MOVING) as u32;
    app.world.resource_mut::<RenderSettings>().ray_scene_stress = copies;
    app.update_n(4);
    assert_in_ray_scene(&app, "filled to the caps");
    assert_eq!(diag(&app, "render.ray.copies"), copies as f64);
    assert_eq!(
        counts(&app),
        (STILL, MOVING, 2.0),
        "both structures full, two copies past their room"
    );
    for frame in 0..3 {
        app.update();
        // (Bits: 1 the still structure, 2 the moving one.)
        assert_eq!(
            diag(&app, "render.ray.structures_built"),
            3.0,
            "frame {frame}: both structures are built every frame"
        );
    }
    // (The copies could not open the valve if they counted: those that hold still fit.)
    assert_eq!(
        (
            diag(&app, "render.ray.class_solid"),
            diag(&app, "render.ray.still_set"),
            diag(&app, "render.ray.valve"),
        ),
        (2.0, 2.0, 0.0),
        "the copies are not counted, and the valve stays shut"
    );
    app.world.resource_mut::<RenderSettings>().ray_scene_stress = 0;
    app.update_n(4);
    assert_eq!(diag(&app, "render.ray.copies"), 0.0);
    assert_eq!(counts(&app), (2.0, 0.0, 0.0), "the copies are gone");
    assert_in_ray_scene(&app, "the copies gone");
}
