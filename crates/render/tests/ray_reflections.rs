//! Every pixel's reflection (`trial_cut_more::RAY_REFLECTIONS`, ray_reflect.wgsl), step 9 of the
//! ray-traced lighting round: each prepass pixel up to the roughness limit marches the screen
//! and takes what it meets from last frame's picture, or else traces a ray through the frame's
//! ray scene, as long as the roughness lets a reflection be told apart; ssr.wgsl
//! `fs_reflect_resolve` blurs them over each pixel's lobe.
//!
//! A mirror shows a striped wall behind the camera stripe by stripe (the half-size world
//! reflections blur the stripes together); a mirror floor's reflection of a box keeps its colour
//! when the box leaves the screen (screen first, then the ray's own light), and the hit a ray
//! meets where the camera sees it is taken from last frame's picture; a rough floor reflects an
//! overhead glow 6 m up and not one 40 m up, which a mirror floor reflects; indoors, a mirror
//! floor shows the sky through a window; in thick fog a wall's reflection keeps its fogged
//! colour where the rays take over from the screen (the rays' light lit inside the fog as the
//! camera's surfaces are and fogged as the screen's picture is), in a bank that hides the sun
//! and in a layer that lets some of it through and lights the wall with its own light, and
//! indoors the fog over what the rays light is as dark as the room's. Needs a GPU with ray
//! queries (lavapipe has them); passes with a note without.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::shade_skip::{COUNT_MESH_RUNS, RAY_REFLECT_RAYS_ONLY};
use engine_render::settings::trial_cut_more::{RAY_REFLECTIONS, RAY_WORLD, RAYS};
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;
/// Half the vertical field of view (degrees) of every camera here.
const HALF_FOV: f64 = 35.0;

/// Probe GI on ray-tracing hardware through the frame's ray scene (a few probes a frame: the
/// probes' rays are most of a frame's time on a software GPU), every pixel's reflection on
/// (`full`, else the half-size world reflections under screen-space ones), counted.
fn app(full: bool) -> App {
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
        s.gi_probes_per_frame = 16;
        s.gi_hysteresis = 0.5;
        // The frame's ray scene with every pixel's reflection or without, and no other part of
        // the ray-traced lighting, whatever the defaults hold.
        s.trial_cuts_more = (s.trial_cuts_more & !RAYS) | RAY_WORLD;
        if full {
            s.trial_cuts_more |= RAY_REFLECTIONS;
        }
        s.debug_skip |= COUNT_MESH_RUNS;
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

/// The frame's rays traced the frame's ray scene with nothing lost, and the reflections were
/// every pixel's (`full`) or not.
fn assert_traced(app: &App, full: bool, what: &str) {
    assert_eq!(
        diag(app, "render.ray.losses"),
        0.0,
        "{what}: the GPU lost the ray scene"
    );
    assert_eq!(diag(app, "render.ray.on"), 1.0, "{what}: no ray scene");
    assert_eq!(
        diag(app, "render.gi.ray_world"),
        1.0,
        "{what}: the probes trace a scene of their own"
    );
    assert_eq!(
        diag(app, "render.ray.reflect_full"),
        if full { 1.0 } else { 0.0 },
        "{what}: every pixel's reflection traced"
    );
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

fn ground(w: &mut World, y: f64, material: StandardMaterial) {
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, y, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            material,
        ),
    ));
}

fn rough(r: f32, g: f32, b: f32) -> StandardMaterial {
    StandardMaterial::color(r, g, b).with_roughness(0.9)
}

/// Bright metal of `roughness`.
fn metal(roughness: f32) -> StandardMaterial {
    StandardMaterial::color(0.95, 0.95, 0.95)
        .with_metallic(1.0)
        .with_roughness(roughness)
}

fn sun(w: &mut World, toward: DVec3) {
    w.spawn((
        DirectionalLight {
            illuminance: 110_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(toward, Vec3::Y),
    ));
}

fn camera(w: &mut World, eye: DVec3, target: DVec3) -> Entity {
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective((2.0 * HALF_FOV) as f32)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ))
    .id()
}

/// The picture as the scene stands now.
fn picture(app: &mut App, name: &str) -> image::RgbImage {
    let dir = std::env::temp_dir().join("engine-ray-reflections");
    let _ = std::fs::create_dir_all(&dir);
    // (A file a test: libtest runs the tests side by side.)
    let test = std::thread::current()
        .name()
        .unwrap_or("main")
        .replace(':', "_");
    let path = dir.join(format!("{test}-{name}"));
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    image::open(&path).expect("read the shot").to_rgb8()
}

/// What every pixel's reflection counted in a frame of the scene as it stands: pixels under the
/// roughness limit, those the screen answered, rays, rays' hits taken from last frame's
/// picture. A count is read back frames after it was taken, so four pictures are taken first.
fn reflect_counts(app: &mut App) -> [u32; 4] {
    for _ in 0..4 {
        picture(app, "count.png");
    }
    [
        "reflect_pixels",
        "reflect_screen",
        "reflect_rays",
        "reflect_reused",
    ]
    .map(|row| {
        let name = format!("render.ray.{row}");
        let n = diag(app, &name);
        assert!(n >= 0.0, "{name}: the reflections are counted");
        n as u32
    })
}

/// The mean colour of the pixels in `rows` x `cols`.
fn mean_rgb(
    p: &image::RgbImage,
    rows: std::ops::Range<u32>,
    cols: std::ops::Range<u32>,
) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut n = 0.0f32;
    for y in rows {
        for x in cols.clone() {
            let px = p.get_pixel(x, y);
            for c in 0..3 {
                sum[c] += px[c] as f32;
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n.max(1.0))
}

/// The row where a camera pitched by `pitch` degrees (negative: down) sees the direction
/// `elevation` degrees above the horizon, along the picture's middle column.
fn row_of(elevation: f64, pitch: f64) -> u32 {
    let t = (elevation - pitch).to_radians().tan() / HALF_FOV.to_radians().tan();
    ((H as f64) * 0.5 * (1.0 - t))
        .round()
        .clamp(0.0, (H - 1) as f64) as u32
}

/// A yard: rough ground, a mirror 4 m ahead of the camera, and 3 m behind the camera a wall of
/// red and blue stripes 0.4 m wide, three pixels each in the mirror at this size.
fn striped_yard(full: bool) -> App {
    let mut app = app(full);
    let w = &mut app.world;
    ground(w, -0.3, rough(0.3, 0.3, 0.3));
    slab(
        w,
        DVec3::new(0.0, 1.5, -3.0),
        DVec3::new(5.0, 3.0, 0.1),
        metal(0.05),
    );
    for i in 0..40 {
        let color = if i % 2 == 0 {
            rough(0.8, 0.05, 0.05)
        } else {
            rough(0.05, 0.1, 0.8)
        };
        slab(
            w,
            DVec3::new(-8.0 + 0.4 * i as f64 + 0.2, 4.0, 4.0),
            DVec3::new(0.4, 8.6, 0.3),
            color,
        );
    }
    sun(w, DVec3::new(0.2, -0.8, 0.6));
    camera(w, DVec3::new(0.0, 1.5, 1.0), DVec3::new(0.0, 1.5, -3.0));
    app
}

/// Of the mirror's pixels that show the stripes, the shares clearly red and clearly blue.
fn stripe_shares(p: &image::RgbImage) -> (f32, f32) {
    let (mut red, mut blue, mut n) = (0u32, 0u32, 0u32);
    for y in 34..68 {
        for x in 32..128 {
            let px = p.get_pixel(x, y);
            let [r, g, b] = [px[0], px[1], px[2]].map(|c| c as f32);
            red += (r > 1.6 * b.max(g) && r > 20.0) as u32;
            blue += (b > 1.6 * r.max(g) && b > 20.0) as u32;
            n += 1;
        }
    }
    (red as f32 / n as f32, blue as f32 / n as f32)
}

/// A mirror facing the camera shows a striped wall behind the camera, off the screen, stripe by
/// stripe at every pixel: each pixel's own ray, where the half-size world reflections took one
/// for each 2x2 pixels and blurred them. Every pixel under the roughness limit is the screen's
/// or a ray's.
#[test]
fn a_mirror_reflects_a_wall_behind_the_camera_at_every_pixel() {
    let mut full = striped_yard(true);
    full.update_n(8);
    if !can_run(&full) {
        return;
    }
    assert_traced(&full, true, "every pixel's reflection");
    let [pixels, screen, rays, reused] = reflect_counts(&mut full);
    eprintln!(
        "{pixels} pixels under the limit: {screen} from the screen, {rays} rays, {reused} reused"
    );
    assert!(
        pixels > W * H / 4,
        "the mirror's pixels are under the roughness limit ({pixels})"
    );
    assert_eq!(
        screen + rays,
        pixels,
        "each pixel under the limit is the screen's or a ray's"
    );
    assert!(
        rays * 2 > pixels,
        "what is behind the camera is the rays' ({rays} of {pixels})"
    );
    let (red, blue) = stripe_shares(&picture(&mut full, "mirror_full.png"));

    let mut half = striped_yard(false);
    half.update_n(8);
    assert_traced(&half, false, "half-size world reflections");
    let (half_red, half_blue) = stripe_shares(&picture(&mut half, "mirror_half.png"));
    eprintln!(
        "stripes in the mirror: {:.0}% red, {:.0}% blue (half-size world reflections: {:.0}%, \
         {:.0}%)",
        red * 100.0,
        blue * 100.0,
        half_red * 100.0,
        half_blue * 100.0
    );
    assert!(
        red > 0.3 && blue > 0.3,
        "the mirror shows the stripes apart: {:.0}% red, {:.0}% blue",
        red * 100.0,
        blue * 100.0
    );
    assert!(
        red + blue > half_red + half_blue + 0.1,
        "sharper than the half-size world reflections ({:.0}% against {:.0}%)",
        (red + blue) * 100.0,
        (half_red + half_blue) * 100.0
    );

    // Without world reflections (`RenderSettings::world_reflections` off) no reflection is
    // traced, every pixel's or half-size: screen-space reflections over the sky, so the wall
    // behind the camera isn't in the mirror.
    let mut none = striped_yard(true);
    none.world
        .resource_mut::<RenderSettings>()
        .world_reflections = false;
    none.update_n(8);
    assert_traced(&none, false, "without world reflections");
    let (none_red, none_blue) = stripe_shares(&picture(&mut none, "mirror_none.png"));
    eprintln!(
        "without world reflections: {:.0}% red, {:.0}% blue",
        none_red * 100.0,
        none_blue * 100.0
    );
    assert!(
        none_red + none_blue + 0.2 < red + blue,
        "without world reflections the mirror still shows the wall behind the camera \
         ({:.0}% against {:.0}% with them)",
        (none_red + none_blue) * 100.0,
        (red + blue) * 100.0
    );
}

/// A mirror floor, a green box 10 m ahead on it, the camera 1 m up, pitched by `pitch` degrees.
fn box_on_mirror(pitch: f64) -> App {
    let mut app = app(true);
    let w = &mut app.world;
    ground(w, 0.0, metal(0.05));
    slab(
        w,
        DVec3::new(0.0, 2.0, -6.0),
        DVec3::new(3.0, 4.0, 1.0),
        rough(0.1, 0.6, 0.1),
    );
    sun(w, DVec3::new(0.25, -0.7, -0.6));
    let eye = DVec3::new(0.0, 1.0, 4.0);
    camera(
        w,
        eye,
        eye + DVec3::new(0.0, pitch.to_radians().tan(), -1.0),
    );
    app
}

/// The box's reflection seen 22 to 14 degrees below the horizon (the box stands 6 to 26 degrees
/// below it in the mirror floor), across its middle.
fn reflection_of_box(p: &image::RgbImage, pitch: f64) -> [f32; 3] {
    mean_rgb(p, row_of(-14.0, pitch)..row_of(-22.0, pitch), 74..86)
}

fn green_share(c: [f32; 3]) -> f32 {
    c[1] / (c[0] + c[1] + c[2]).max(1.0)
}

/// A mirror floor's reflection of a box keeps its colour as the box leaves the screen: with the
/// box in view the screen answers its reflection (last frame's picture); pitched down until the
/// box is off the top of the screen, the rays do, lit by their own light. The rays' hits the
/// camera sees take last frame's picture as the screen's do.
#[test]
fn a_reflection_keeps_its_colour_as_its_subject_leaves_the_screen() {
    let mut seen = box_on_mirror(-10.0);
    seen.update_n(8);
    if !can_run(&seen) {
        return;
    }
    assert_traced(&seen, true, "the box in view");
    let [pixels, screen, rays, reused] = reflect_counts(&mut seen);
    eprintln!(
        "the box in view: {pixels} pixels under the limit, {screen} from the screen, {rays} \
         rays, {reused} reused"
    );
    assert!(
        screen > 400,
        "the screen answers the box's reflection ({screen})"
    );
    let in_view = reflection_of_box(&picture(&mut seen, "box_in_view.png"), -10.0);

    // Every reflection a ray's: those that meet the box take last frame's picture of it.
    seen.world.resource_mut::<RenderSettings>().debug_skip |= RAY_REFLECT_RAYS_ONLY;
    seen.update_n(4);
    let [pixels, screen, rays, reused] = reflect_counts(&mut seen);
    eprintln!(
        "rays only: {pixels} pixels under the limit, {screen} from the screen, {rays} rays, \
         {reused} reused"
    );
    assert_eq!(screen, 0, "no march");
    assert_eq!(rays, pixels, "every pixel's reflection is a ray's");
    assert!(
        reused > 400,
        "the rays that meet the box take its picture ({reused})"
    );
    let by_rays = reflection_of_box(&picture(&mut seen, "box_by_rays.png"), -10.0);

    let mut left = box_on_mirror(-45.0);
    left.update_n(8);
    assert_traced(&left, true, "the box off the screen");
    let [pixels, screen, rays, reused] = reflect_counts(&mut left);
    eprintln!(
        "the box off the screen: {pixels} pixels under the limit, {screen} from the screen, \
         {rays} rays, {reused} reused"
    );
    let off_screen = reflection_of_box(&picture(&mut left, "box_left_the_screen.png"), -45.0);
    eprintln!(
        "the box's reflection: {in_view:?} in view, {by_rays:?} by rays, {off_screen:?} off the \
         screen"
    );
    for (c, what) in [
        (in_view, "in view"),
        (by_rays, "by rays"),
        (off_screen, "off the screen"),
    ] {
        assert!(
            green_share(c) > 0.5,
            "{what}: the reflection is the box's green ({c:?})"
        );
    }
    // Every channel: the box's own light (off the screen) is about 9% less green than its
    // picture, 13% less red and 18% less blue here (outside the probes' grid it has no sky),
    // so a green-only 10% could not tell the two apart (23 Nov).
    let close = |a: [f32; 3], b: [f32; 3], share: f32| {
        (0..3).all(|c| (a[c] - b[c]).abs() <= share * a[c].max(1.0))
    };
    assert!(
        close(in_view, by_rays, 0.03),
        "a ray's hit the camera sees is last frame's picture ({in_view:?} against {by_rays:?})"
    );
    assert!(
        !close(in_view, off_screen, 0.03),
        "the check above tells last frame's picture from a hit's own light ({in_view:?} \
         against {off_screen:?})"
    );
    assert!(
        close(in_view, off_screen, 0.3),
        "the reflection keeps its brightness off the screen ({in_view:?} against \
         {off_screen:?})"
    );
}

/// The fog round [`wall_in_fog`]'s wall: none; a bank alike at every height, whose column
/// above (300 optical depths) lets no sun through to the wall and scatters next to none down
/// onto it (a third of a percent), so only the sky lights it; or a layer thinning upward as
/// the default fog does (by e every 50 m), which lets about a fifth of the sun through to where
/// the floor shows the wall and scatters about as much light again down onto it
/// (`height_fog_sunlight`). Both leave 30 to 40% of the wall's light after 40 m.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fog {
    Clear,
    Bank,
    Layer,
}

/// The height fog `fog`, alone: analytic (the froxels' fog is lit by the shadow maps, which the
/// rays don't read) and scattering alike in every direction.
fn set_fog(app: &mut App, fog: Fog) {
    app.world.resource_mut::<RenderSettings>().volumetric_fog = false;
    let mut f = app.world.resource_mut::<FogSettings>();
    f.density = if fog == Fog::Clear { 0.0 } else { 0.03 };
    f.height_falloff = if fog == Fog::Layer { 0.02 } else { 0.0001 };
    f.sun_scattering = 0.0;
}

/// A mirror floor and a green wall 30 m wide and 15 m high, 40 m ahead of the camera 1 m up,
/// pitched by `pitch` degrees, in `fog` ([`set_fog`]), so the screen's fog and the rays' are
/// the same fog.
fn wall_in_fog(pitch: f64, fog: Fog) -> App {
    let mut app = app(true);
    set_fog(&mut app, fog);
    let w = &mut app.world;
    ground(w, 0.0, metal(0.05));
    slab(
        w,
        DVec3::new(0.0, 7.5, -36.5),
        DVec3::new(30.0, 15.0, 1.0),
        rough(0.1, 0.6, 0.1),
    );
    sun(w, DVec3::new(0.25, -0.7, -0.6));
    let eye = DVec3::new(0.0, 1.0, 4.0);
    camera(
        w,
        eye,
        eye + DVec3::new(0.0, pitch.to_radians().tan(), -1.0),
    );
    app
}

/// The wall's reflection seen 12 to 18 degrees below the horizon (the wall stands 1.4 to 22
/// degrees below it in the mirror floor; this is the wall 7.5 to 12 m up), across its middle.
fn reflection_of_wall(p: &image::RgbImage, pitch: f64) -> [f32; 3] {
    mean_rgb(p, row_of(-12.0, pitch)..row_of(-18.0, pitch), 70..90)
}

/// The rays' own light is fogged from the camera to the wall, as the camera's picture of the
/// wall is, and the wall is lit inside the fog as the camera's surfaces are (the sun dimmed by
/// the fog above it, and the fog's own light): in thick fog the wall's reflection keeps its
/// colour as the wall leaves the screen and the rays take over from the screen, within a
/// twentieth of what the fog changes, where the rays' light lit and fogged as in clear air
/// stands far apart. In a bank the sun never reaches the wall; in a layer a fifth of it does,
/// and the fog's own light about as much again. (25 Nov, lavapipe: 0.3 and 0.4 levels of 255
/// apart in the bank and the layer, of the 104 and 52 the fog changes; 96 and 48 with neither
/// the fog nor the fog's light on the rays' hits, 38 and 18 with their fog alone, and 6.4 in
/// the layer without the fog's own light.)
#[test]
fn a_reflection_in_fog_keeps_its_colour_where_the_rays_take_over() {
    let mut shots = Vec::new();
    for (pitch, fog, what, name) in [
        (
            -10.0,
            Fog::Bank,
            "the wall in view, in a fog bank",
            "fog_bank_wall_in_view.png",
        ),
        (
            -45.0,
            Fog::Bank,
            "the wall off the screen, in a fog bank",
            "fog_bank_wall_left.png",
        ),
        (
            -10.0,
            Fog::Layer,
            "the wall in view, in a fog layer",
            "fog_layer_wall_in_view.png",
        ),
        (
            -45.0,
            Fog::Layer,
            "the wall off the screen, in a fog layer",
            "fog_layer_wall_left.png",
        ),
        (
            -45.0,
            Fog::Clear,
            "the wall off the screen, clear",
            "clear_wall_left.png",
        ),
    ] {
        let mut app = wall_in_fog(pitch, fog);
        app.update_n(8);
        if !can_run(&app) {
            return;
        }
        assert_traced(&app, true, what);
        let [pixels, screen, rays, reused] = reflect_counts(&mut app);
        eprintln!(
            "{what}: {pixels} pixels under the limit, {screen} from the screen, {rays} rays, \
             {reused} reused"
        );
        if pitch > -20.0 {
            assert!(screen > 400, "{what}: the screen answers ({screen})");
        } else {
            assert!(
                rays > 400 && reused < 50,
                "{what}: the rays answer with their own light ({rays} rays, {reused} reused)"
            );
        }
        shots.push(reflection_of_wall(&picture(&mut app, name), pitch));
    }
    let clear = shots[4];
    // How far apart two colours are in the channel that differs most.
    let apart = |a: [f32; 3], b: [f32; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max);
    let fogs = [
        (Fog::Bank, shots[0], shots[1]),
        (Fog::Layer, shots[2], shots[3]),
    ];
    // (Both told before either is judged.)
    for (fog, in_view, left) in fogs {
        eprintln!(
            "the wall's reflection in the {fog:?}: {in_view:.1?} in view, {left:.1?} off the \
             screen ({:.1} apart; {clear:.1?} off the screen without fog, {:.1} apart)",
            apart(left, in_view),
            apart(clear, in_view)
        );
    }
    for (fog, in_view, left) in fogs {
        let (fogged, off) = (apart(clear, in_view), apart(left, in_view));
        assert!(
            fogged > 20.0,
            "the {fog:?} changes the wall's reflection: {in_view:.1?} in view, {clear:.1?} \
             without fog"
        );
        assert!(
            off < 0.05 * fogged,
            "in the {fog:?} the rays' light is lit and fogged as the screen's picture is: \
             {left:.1?} against {in_view:.1?} ({off:.1} apart, the fog makes {fogged:.1})"
        );
    }
}

/// A closed room (inside x -3..3, y 0..3, z -4..4) with a window in the far wall (x -1..1,
/// y 1.2..2.6) and a polished metal floor, seen from near the back wall, an indoor volume
/// around it (sky light 0.02): reflections.rs `room_app` with every pixel's reflection.
fn room_with_window() -> App {
    room_seen(DVec3::new(0.0, 1.6, 3.4), DVec3::new(0.0, 1.0, -4.0))
}

/// [`room_with_window`]'s room, its camera at `eye` looking at `target`.
fn room_seen(eye: DVec3, target: DVec3) -> App {
    let mut app = app(true);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.0 },
            ..Camera::perspective((2.0 * HALF_FOV) as f32)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.2, -0.8, -0.5), Vec3::Y),
    ));
    let plaster = StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9);
    let floor = StandardMaterial::color(0.6, 0.6, 0.6)
        .with_metallic(1.0)
        .with_roughness(0.1);
    let boxes = [
        // Floor, ceiling, side walls, back wall.
        ((-3.3, -0.3, -4.3), (3.3, 0.0, 4.3), floor),
        ((-3.3, 3.0, -4.3), (3.3, 3.3, 4.3), plaster),
        ((-3.3, 0.0, -4.3), (-3.0, 3.0, 4.3), plaster),
        ((3.0, 0.0, -4.3), (3.3, 3.0, 4.3), plaster),
        ((-3.0, 0.0, 4.0), (3.0, 3.0, 4.3), plaster),
        // The far wall around the window.
        ((-3.0, 0.0, -4.3), (-1.0, 3.0, -4.0), plaster),
        ((1.0, 0.0, -4.3), (3.0, 3.0, -4.0), plaster),
        ((-1.0, 0.0, -4.3), (1.0, 1.2, -4.0), plaster),
        ((-1.0, 2.6, -4.3), (1.0, 3.0, -4.0), plaster),
    ];
    for (min, max, material) in boxes {
        let (min, max) = (DVec3::from(min), DVec3::from(max));
        slab(w, (min + max) * 0.5, max - min, material);
    }
    w.spawn((
        Transform::from_xyz(0.0, 1.5, 0.0),
        SkyLightVolume::new([3.0, 1.5, 4.0])
            .with_sky_light(0.02)
            .with_fade(2.0),
    ));
    app
}

/// The mean colour of the 5x5 pixels around where world point `p` lands.
fn colour_at(app: &App, p: &image::RgbImage, at: DVec3) -> [f32; 3] {
    let view = app
        .world
        .resource::<engine_render::Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (at - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * W as f32) as u32;
    let y = ((0.5 - ndc.y * 0.5) * H as f32) as u32;
    mean_rgb(
        p,
        y.saturating_sub(2)..(y + 3).min(H),
        x.saturating_sub(2)..(x + 3).min(W),
    )
}

/// Indoors under a roof, a mirror floor shows the sky through the window as brightly as the
/// screen-space reflections do (reflections.rs `a_floor_indoors_mirrors_the_sky_through_the_window`):
/// the march passes over the window's sky and the ray meets nothing past it, so the sky there
/// is open, whatever the cover overhead and the indoor volume's face across the window say.
/// (Before, the reflection read as open sky under the roof's cover: a dark patch.)
#[test]
fn a_floor_indoors_mirrors_the_sky_through_the_window() {
    let mut app = room_with_window();
    app.update_n(8);
    if !can_run(&app) {
        return;
    }
    assert_traced(&app, true, "the room");
    let shot = picture(&mut app, "room_window.png");
    let window = colour_at(&app, &shot, DVec3::new(0.0, 0.0, -0.05));
    let wall = colour_at(&app, &shot, DVec3::new(0.9, 0.0, 0.0));
    eprintln!("floor mirroring the window: {window:?}, the floor beside it: {wall:?}");
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    assert!(
        luma(window) > luma(wall) * 3.0 + 20.0,
        "the floor mirrors the sky through the window: {window:?} against {wall:?}"
    );
    assert!(
        window[2] > window[0],
        "the window's mirror image is sky blue: {window:?}"
    );
}

/// A floor of metal of `roughness` under a red glow `height` m up, seen from 2 m up, looking
/// 60 degrees down (the glow is off the screen), the sun low from the side.
fn floor_under_glow(roughness: f32, height: f64) -> App {
    let mut app = app(true);
    let w = &mut app.world;
    ground(w, 0.0, metal(roughness));
    slab(
        w,
        DVec3::new(0.0, height, -60.0),
        DVec3::new(200.0, 0.5, 240.0),
        StandardMaterial::color(0.0, 0.0, 0.0).with_emissive([1.0, 0.05, 0.05], 8000.0),
    );
    sun(w, DVec3::new(1.0, -0.25, 0.0));
    let eye = DVec3::new(0.0, 2.0, 0.0);
    camera(
        w,
        eye,
        eye + DVec3::new(0.0, -60f64.to_radians().tan(), -1.0),
    );
    app
}

/// The red share of the picture's middle.
fn red_share(p: &image::RgbImage) -> f32 {
    let c = mean_rgb(p, 30..90, 40..120);
    (c[0] - 0.5 * (c[1] + c[2])) / c[0].max(1.0)
}

/// A rough floor (roughness 0.5: rays of 33 m) reflects a glow 6 m overhead and not one 40 m
/// up (the rays from the floor to it are 46 m and more), which a mirror floor (rays of 311 m)
/// reflects.
#[test]
fn a_rough_floor_reflects_only_what_is_near() {
    let mut near = floor_under_glow(0.5, 6.0);
    near.update_n(8);
    if !can_run(&near) {
        return;
    }
    assert_traced(&near, true, "a glow 6 m up");
    let [pixels, screen, rays, reused] = reflect_counts(&mut near);
    eprintln!(
        "{pixels} pixels under the limit, {screen} from the screen, {rays} rays, {reused} reused"
    );
    assert!(
        pixels > W * H * 9 / 10,
        "the floor fills the view ({pixels})"
    );
    let near_red = red_share(&picture(&mut near, "rough_near.png"));

    let mut far = floor_under_glow(0.5, 40.0);
    far.update_n(8);
    assert_traced(&far, true, "a glow 40 m up");
    let far_red = red_share(&picture(&mut far, "rough_far.png"));

    let mut mirror = floor_under_glow(0.05, 40.0);
    mirror.update_n(8);
    assert_traced(&mirror, true, "a mirror floor");
    let mirror_red = red_share(&picture(&mut mirror, "mirror_far.png"));
    eprintln!(
        "red share: {near_red:.2} under a glow 6 m up, {far_red:.2} under one 40 m up, \
         {mirror_red:.2} in a mirror floor under that"
    );
    assert!(
        near_red > 0.6,
        "a rough floor reflects what is near: {near_red:.2}"
    );
    assert!(
        far_red < 0.2,
        "and not what its rays don't reach: {far_red:.2}"
    );
    assert!(
        mirror_red > 0.6,
        "a mirror floor reflects what is far: {mirror_red:.2}"
    );
}

/// Beside a rug too rough to reflect (roughness 0.95, past the limit: never traced), a glossy
/// floor (roughness 0.4: its blur gathers about 10 pixels around) reflects the glow overhead as
/// brightly as it does away from the rug. The rug's pixels say nothing of the floor's
/// reflection; counted as misses, they gave the floor beside the rug, a lobe wide, the sky the
/// red glow hides.
#[test]
fn a_rug_too_rough_to_reflect_leaves_the_floor_beside_it_as_it_is() {
    let mut app = floor_under_glow(0.4, 6.0);
    slab(
        &mut app.world,
        DVec3::new(0.0, 0.005, -1.5),
        DVec3::new(0.3, 0.01, 3.0),
        StandardMaterial::color(0.3, 0.3, 0.3).with_roughness(0.95),
    );
    app.update_n(8);
    if !can_run(&app) {
        return;
    }
    assert_traced(&app, true, "the rug");
    let shot = picture(&mut app, "rug.png");
    let beside = colour_at(&app, &shot, DVec3::new(0.24, 0.0, -1.2));
    let away = colour_at(&app, &shot, DVec3::new(1.0, 0.0, -1.2));
    eprintln!("the floor: {beside:?} beside the rug, {away:?} away from it");
    assert!(
        away[0] > 2.0 * away[2],
        "the floor reflects the red glow: {away:?}"
    );
    assert!(
        (0..3).all(|c| (beside[c] - away[c]).abs() <= 0.08 * away[c].max(10.0)),
        "the floor beside the rug reflects as it does away from it: {beside:?} against {away:?}"
    );
}

/// Indoors in thick fog, the floor's reflection of the far wall where the rays answer it (the
/// wall off the top of the screen) is as dark as where the screen does: the haze over what the
/// rays light is lit by what the room leaves of the sky and the sun, as the camera's haze in the
/// room is (`fog_sky_visibility`: next to nothing). Lit as open air, it laid a bright veil over
/// the reflection.
#[test]
fn indoors_a_reflections_fog_is_as_dark_as_the_rooms() {
    let eye = DVec3::new(0.0, 1.6, 3.4);
    // Where the floor shows the far wall 2 m up and 2 m left of the middle (beside the window)
    // and around it: the wall's points mirrored in the floor.
    let images = [
        DVec3::new(-2.0, -2.0, -4.0),
        DVec3::new(-2.3, -1.8, -4.0),
        DVec3::new(-1.7, -2.2, -4.0),
    ];
    let mut shots = Vec::new();
    for (pitch, fog, what, name) in [
        (
            -10.0_f64,
            Fog::Bank,
            "the far wall in view, in fog",
            "room_fog_in_view.png",
        ),
        (
            -55.0,
            Fog::Bank,
            "the far wall off the screen, in fog",
            "room_fog_left.png",
        ),
        (
            -10.0,
            Fog::Clear,
            "the far wall in view, clear",
            "room_clear_in_view.png",
        ),
        (
            -55.0,
            Fog::Clear,
            "the far wall off the screen, clear",
            "room_clear_left.png",
        ),
    ] {
        let mut app = room_seen(eye, eye + DVec3::new(0.0, pitch.to_radians().tan(), -1.0));
        set_fog(&mut app, fog);
        app.update_n(8);
        if !can_run(&app) {
            return;
        }
        assert_traced(&app, true, what);
        let [pixels, screen, rays, reused] = reflect_counts(&mut app);
        eprintln!(
            "{what}: {pixels} pixels under the limit, {screen} from the screen, {rays} rays, \
             {reused} reused"
        );
        let p = picture(&mut app, name);
        let mut c = [0.0f32; 3];
        for at in images {
            let one = colour_at(&app, &p, at);
            for k in 0..3 {
                c[k] += one[k] / images.len() as f32;
            }
        }
        shots.push(c);
    }
    let [fog_in_view, fog_left, clear_in_view, clear_left] =
        [shots[0], shots[1], shots[2], shots[3]];
    let apart = |a: [f32; 3], b: [f32; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max);
    eprintln!(
        "the far wall in the floor: in fog {fog_in_view:.1?} in view, {fog_left:.1?} off the \
         screen ({:.1} apart); clear {clear_in_view:.1?} in view, {clear_left:.1?} off the screen \
         ({:.1} apart)",
        apart(fog_left, fog_in_view),
        apart(clear_left, clear_in_view)
    );
    assert!(
        apart(fog_left, fog_in_view) <= apart(clear_left, clear_in_view) + 4.0,
        "indoors the rays' fog is the room's: {fog_left:.1?} off the screen against \
         {fog_in_view:.1?} in view, where clear air leaves them {:.1} apart",
        apart(clear_left, clear_in_view)
    );
}
