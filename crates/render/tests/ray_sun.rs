//! The sun's trace (`trial_cut_more::RAY_SUN`, ray_sun.wgsl), steps 7 and 8 of the ray-traced
//! lighting round: a ray toward the sun's disc from each pixel of the prepass within the reach
//! that faces the sun, through the frame's ray scene's still structure, kept over the frames
//! (ray_filters.wgsl) and joined with the shadow maps in the screen light pass
//! (screen_light.wgsl `sun_joined`).
//!
//! The trace, counted (`shade_skip::COUNT_MESH_RUNS`): over open ground no ray is stopped and
//! under a roof those its shadow falls on are, as many as the pixels the shadow maps leave
//! dark, and the still images agree with the rays; a swaying mesh's pixels are left to the
//! shadow maps (not traced), in both of the prepass's ways of drawing a mesh, and the picture
//! is the one without the trace; a pixel on the drawn ground just under a ground another
//! feature brings through the hook is lit (the rays pass up through its back), and that ground
//! turned down as a roof stops them.
//!
//! The join, against the same scene drawn with the shadow maps alone
//! (`shade_skip::RAY_SUN_TRACE_ONLY`): a post the maps leave out casts its shadow; a roof's
//! shadow is the maps' but at its edge; a caster that moves is shadowed by the maps alone and
//! leaves no trail where it stood still; a caster farther up-sun than the rays reach still
//! shades the ground; a roof the rays leave out, and one past the traced pixels' reach, keep
//! the maps' shadow; and a camera cut drops the trace's history. Needs a GPU with ray queries
//! (lavapipe has them); passes with a note without.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::shade_skip::RAY_SUN_TRACE_ONLY;
use engine_render::settings::trial_cut::MESHES_ONCE;
use engine_render::settings::trial_cut_more::{RAY_SUN, RAY_WORLD, RAYS};
use engine_render::{
    GpuShared, RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RaySphere,
    RayView, RenderDisabled, capture_now,
};

const W: u32 = 160;
const H: u32 = 120;
/// The ground's top (the rooms' floors in `ray_scene.rs` stand on it).
const GROUND_Y: f64 = -0.3;

/// Probe GI on ray-tracing hardware through the frame's ray scene, a few probes a frame (the
/// probes' rays are most of a frame's time on a software GPU), the sun's trace on (`trace`)
/// and counted.
fn app(trace: bool) -> App {
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
        // The frame's ray scene with the sun's trace or without, and no other part of the
        // ray-traced lighting, whatever the defaults hold.
        s.trial_cuts_more = (s.trial_cuts_more & !RAYS) | RAY_WORLD;
        if trace {
            s.trial_cuts_more |= RAY_SUN;
        }
        s.debug_skip |= engine_render::settings::shade_skip::COUNT_MESH_RUNS;
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

/// The frame's rays traced the frame's ray scene, with nothing lost so far.
fn assert_in_ray_scene(app: &App, what: &str) {
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
}

fn slab(w: &mut World, center: DVec3, size: DVec3, material: StandardMaterial) -> Entity {
    w.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(
            Shape::Cuboid {
                size: size.as_vec3().to_array(),
            },
            material,
        ),
    ))
    .id()
}

fn gray(albedo: f32) -> StandardMaterial {
    StandardMaterial::color(albedo, albedo, albedo).with_roughness(0.9)
}

/// A material that sways in the wind (as a plant's: `wind_sway`).
fn swaying(albedo: f32) -> StandardMaterial {
    let mut material = gray(albedo);
    material.wind_sway = 1.0;
    material
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

/// The ground seen from above with no sky in view (all of it within the trace's reach, and
/// facing the sun), the sun from the side.
fn open_ground(trace: bool) -> App {
    let mut app = app(trace);
    let w = &mut app.world;
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, GROUND_Y, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
    sun(w, 110_000.0, DVec3::new(1.0, -1.0, 0.0));
    camera(w, DVec3::new(0.0, 7.0, 5.0), DVec3::ZERO, 14.0);
    app
}

/// The picture as the scene stands now.
fn picture(app: &mut App, name: &str) -> image::RgbImage {
    let dir = std::env::temp_dir().join("engine-ray-sun");
    let _ = std::fs::create_dir_all(&dir);
    // (A file a test: libtest runs the tests side by side, and a picture read back while
    // another test writes it under the same name is half a file.)
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

fn mean(p: &image::Rgb<u8>) -> f32 {
    (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0
}

/// What the trace counted in a frame of the scene as it stands: rays, rays stopped, "missed"
/// and "false" (ray_sun.wgsl). A count is read back frames after it was taken, so four
/// pictures are taken first: each waits for the GPU, and by the fourth the count read is one
/// taken since the first.
fn trace_counts(app: &mut App) -> [u32; 4] {
    for _ in 0..4 {
        picture(app, "trace_count.png");
    }
    ["sun_rays", "sun_hits", "sun_missed", "sun_false"].map(|row| {
        let name = format!("render.ray.{row}");
        let n = diag(app, &name);
        assert!(n >= 0.0, "{name}: the trace is counted");
        n as u32
    })
}

/// Over open ground every pixel shoots a ray and none is stopped; under a roof 2 m up the rays
/// stopped are the pixels the shadow maps leave dark, and where the shadow maps' still images
/// are sure of a pixel (a caster within the rays' length over every texel around it, or none)
/// a ray toward the sun's center says the same. A roof the rays leave out (past their reach)
/// is counted where its shadow falls: "missed".
#[test]
fn the_suns_trace_is_stopped_where_the_shadow_falls() {
    let mut open = open_ground(true);
    open.update_n(8);
    if !can_run(&open) {
        return;
    }
    assert_in_ray_scene(&open, "open ground");
    let [rays, hits, missed, wrong] = trace_counts(&mut open);
    eprintln!("open ground: {hits} of {rays} rays stopped, {missed} missed, {wrong} false");
    assert_eq!(
        rays,
        W * H,
        "a ray from every pixel: the ground fills the view, faces the sun, lies within reach"
    );
    assert_eq!(hits, 0, "nothing stands between open ground and the sun");
    assert_eq!(
        missed + wrong,
        0,
        "the still images see open ground as the rays do"
    );
    let lit = picture(&mut open, "open.png")
        .pixels()
        .map(mean)
        .sum::<f32>()
        / (W * H) as f32;

    // (The picture with the shadow maps alone: the rays are compared with the maps.)
    let mut roofed = open_ground(true);
    roofed.world.resource_mut::<RenderSettings>().debug_skip |= RAY_SUN_TRACE_ONLY;
    slab(
        &mut roofed.world,
        DVec3::new(0.0, 2.0, 0.0),
        DVec3::new(3.0, 0.1, 3.0),
        gray(0.3),
    );
    roofed.update_n(8);
    assert_in_ray_scene(&roofed, "under the roof");
    let [rays, hits, missed, wrong] = trace_counts(&mut roofed);
    let dark = picture(&mut roofed, "roof.png")
        .pixels()
        .filter(|p| mean(p) < lit * 0.5)
        .count() as u32;
    eprintln!(
        "under the roof: {hits} of {rays} rays stopped, {missed} missed, {wrong} false; {dark} \
         pixels drawn in shadow (open ground {lit:.1})"
    );
    // (The roof's sides away from the sun take no ray.)
    assert!(
        rays > W * H * 9 / 10,
        "the ground and the roof's top shoot their rays ({rays})"
    );
    assert!(
        dark > 300,
        "the roof's shadow is in view: {dark} dark pixels"
    );
    assert!(
        hits.abs_diff(dark) * 8 <= dark,
        "the rays stopped ({hits}) are the pixels in shadow ({dark})"
    );
    assert!(
        (missed + wrong) * 200 <= rays,
        "the still images agree with the rays where they are sure: {missed} missed, {wrong} \
         false of {rays}"
    );

    // The rays' reaches cut to a meter: the roof is not in the rays, its shadow is in the
    // shadow maps.
    let mut unreached = open_ground(true);
    {
        let mut s = unreached.world.resource_mut::<RenderSettings>();
        s.ray_caster_reach = 1.0;
        s.ray_far_reach = 1.0;
    }
    slab(
        &mut unreached.world,
        DVec3::new(0.0, 2.0, 0.0),
        DVec3::new(3.0, 0.1, 3.0),
        gray(0.3),
    );
    unreached.update_n(8);
    assert_in_ray_scene(&unreached, "a roof past the rays' reach");
    let [rays, hits, missed, wrong] = trace_counts(&mut unreached);
    eprintln!(
        "a roof past the rays' reach: {hits} of {rays} rays stopped, {missed} missed, {wrong} \
         false"
    );
    assert_eq!(hits, 0, "the rays pass a roof they leave out");
    // (The shadow's edge is left out: there the still image is not sure.)
    assert!(
        missed * 2 >= dark && missed <= dark + dark / 8,
        "its shadow is missed by the rays ({missed} missed, {dark} pixels in its shadow)"
    );
    assert_eq!(wrong, 0, "nothing stops the rays");
}

/// A swaying mesh's pixels are left to the shadow maps, which draw it moving: the prepass marks
/// them (`prepass_sway_mark`) and the trace shoots no ray from them, through the prepass's
/// shared material step and through its own program alike (`trial_cut::MESHES_ONCE` on and
/// off). The mark changes nothing else, and the light pass takes the maps alone where nothing
/// was traced: the picture is the one drawn without the trace.
#[test]
fn a_swaying_plant_pixel_keeps_the_maps() {
    // A box whose top fills the view, standing on the ground.
    let scene = |trace: bool, sways: bool, once: bool| {
        let mut app = open_ground(trace);
        if !once {
            app.world.resource_mut::<RenderSettings>().trial_cuts &= !MESHES_ONCE;
        }
        let material = if sways { swaying(0.4) } else { gray(0.4) };
        slab(
            &mut app.world,
            DVec3::new(0.0, 0.2, 0.0),
            DVec3::new(100.0, 0.2, 100.0),
            material,
        );
        app.update_n(8);
        app
    };
    let mut still = scene(true, false, true);
    if !can_run(&still) {
        return;
    }
    assert_in_ray_scene(&still, "a box at rest");
    let [rays, hits, ..] = trace_counts(&mut still);
    assert_eq!(rays, W * H, "the box's top takes a ray at every pixel");
    assert_eq!(hits, 0, "nothing stands over the box");
    for once in [true, false] {
        let mut sways = scene(true, true, once);
        assert_in_ray_scene(&sways, "a swaying box");
        let [rays, ..] = trace_counts(&mut sways);
        assert_eq!(
            rays, 0,
            "no ray from a swaying mesh's pixels (meshes once: {once})"
        );
        let marked = picture(&mut sways, &format!("sway_trace_{once}.png"));
        // (As many frames as the counted one: the probes' light settles frame by frame.)
        let mut plain = scene(false, true, once);
        for _ in 0..4 {
            picture(&mut plain, "sway_wait.png");
        }
        let unmarked = picture(&mut plain, &format!("sway_plain_{once}.png"));
        // (Two runs of one scene differ by the output's dither alone: post.wgsl moves each
        // pixel a level either way, seeded by the clock.)
        let (mut over, mut most, mut sum) = (0u32, 0u8, 0u64);
        for (a, b) in marked.pixels().zip(unmarked.pixels()) {
            let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            over += (d > 2) as u32;
            most = most.max(d);
            sum += d as u64;
        }
        let mean = sum as f32 / (W * H) as f32;
        eprintln!(
            "meshes once {once}: {over} pixels apart by more than the dither, at most {most}, \
             mean {mean:.3}"
        );
        assert!(
            over <= W * H / 1000 && most <= 8 && mean <= 1.0,
            "the mark changed the picture (meshes once: {once}): {over} pixels apart by more \
             than the dither, at most {most}, mean {mean:.3}"
        );
    }
}

/// A ground another feature brings into the rays through the hook (`RayClass::Ground`, as the
/// terrain brings its patches): its squares, each placed still at once.
struct HookedGround(Vec<DAffine3>);

impl RenderFeature for HookedGround {
    fn name(&self) -> &str {
        "hooked ground"
    }

    fn ray_scene(&mut self, _gpu: &GpuShared, _view: &RayView, out: &mut RaySceneOut<'_>) {
        let square = RayKey(0, 0);
        let state = out.geometry(square, 1, RayClass::Ground.opaque(), RaySource::UnitSquare);
        if !matches!(state, RayGeometryState::Ready | RayGeometryState::Older) {
            return;
        }
        for (i, to_world) in self.0.iter().enumerate() {
            let m = to_world.matrix3;
            let sphere = RaySphere {
                center: to_world.translation,
                radius: (m.x_axis.length() + m.z_axis.length()) as f32,
            };
            let look = RayLook {
                albedo: [0.3; 3],
                ..Default::default()
            };
            let id = RayKey(1, i as u64);
            out.place(id, square, to_world, sphere, RayClass::Ground, 0, &look);
        }
    }
}

/// A square of hooked ground `size` m across its x and z, turned by `rotation` (its front, up
/// before the turn), its middle at `center`.
fn hooked_square(size: f64, rotation: DQuat, center: DVec3) -> DAffine3 {
    DAffine3::from_scale_rotation_translation(
        DVec3::new(size * 0.5, 1.0, size * 0.5),
        rotation,
        center,
    )
}

/// The rays' ground is coarser than the drawn one, and the drawn ground may lie just under it:
/// a ray from there passes up through the rays' ground's back (`RULE_GROUND_FRONT`), so a
/// hooked ground 5 cm over the drawn one shades none of it; turned down 2 m up, as a roof, its
/// front stops the rays its shadow falls on.
#[test]
fn a_pixel_below_the_rays_ground_is_not_shadowed_by_it() {
    let turned_down = DQuat::from_rotation_x(std::f64::consts::PI);
    let cases = [
        (
            "under",
            hooked_square(
                200.0,
                DQuat::IDENTITY,
                DVec3::new(0.0, GROUND_Y + 0.05, 0.0),
            ),
        ),
        (
            "roof",
            hooked_square(3.0, turned_down, DVec3::new(0.0, 2.0, 0.0)),
        ),
    ];
    let mut stopped = Vec::new();
    for (name, square) in cases {
        let mut app = open_ground(true);
        app.add_render_feature(HookedGround(vec![square]));
        app.update_n(8);
        if !can_run(&app) {
            return;
        }
        assert_in_ray_scene(&app, name);
        let [rays, hits, missed, wrong] = trace_counts(&mut app);
        eprintln!("{name}: {hits} of {rays} rays stopped, {missed} missed, {wrong} false");
        assert!(rays > 1000, "{name}: the trace shot its rays ({rays})");
        assert_eq!(
            missed, 0,
            "{name}: the shadow maps hold nothing the rays pass"
        );
        // The hooked ground is in the rays alone (no feature draws it into the shadow maps):
        // what it stops is counted where the still images are sure of open ground (the rays
        // toward the sun's center the count compares with, as many as those toward its disc
        // but at the shadow's edge).
        assert!(
            wrong.abs_diff(hits) * 8 <= hits,
            "{name}: the rays it stops are false to the still images ({wrong}, {hits} stopped)"
        );
        stopped.push(hits as f32 / rays as f32);
    }
    assert!(
        stopped[0] < 0.001,
        "the ground under a hooked one is lit: {:.4} of the rays stopped",
        stopped[0]
    );
    assert!(
        stopped[1] > 0.01,
        "a hooked ground's front stops the rays: {:.4} stopped under the roof",
        stopped[1]
    );
}

// ---------------------------------------------------------------- the join (step 8)

/// Ground seen from `eye` (looking at `target`), the sun from the side as in `open_ground`, the
/// trace joined with the shadow maps (`join`) or measured alone, the light pass then taking the
/// maps alone (`RAY_SUN_TRACE_ONLY`). With the camera's entity.
fn ground_from(join: bool, eye: DVec3, target: DVec3) -> (App, Entity) {
    let mut app = app(true);
    if !join {
        app.world.resource_mut::<RenderSettings>().debug_skip |= RAY_SUN_TRACE_ONLY;
    }
    let w = &mut app.world;
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, GROUND_Y, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
    sun(w, 110_000.0, DVec3::new(1.0, -1.0, 0.0));
    let camera = camera(w, eye, target, 14.0);
    (app, camera)
}

/// `ground_from` with `open_ground`'s view.
fn ground(join: bool) -> App {
    ground_from(join, DVec3::new(0.0, 7.0, 5.0), DVec3::ZERO).0
}

/// A roof 2 m up over the ground at `x`, `z`, as in the trace's own tests.
fn roof(app: &mut App, x: f64, z: f64) -> Entity {
    slab(
        &mut app.world,
        DVec3::new(x, 2.0, z),
        DVec3::new(3.0, 0.1, 3.0),
        gray(0.3),
    )
}

/// Checks that a frame of `app` traced in the frame's ray scene and was joined (`join`) or not.
fn assert_joined(app: &App, join: bool, what: &str) {
    assert_in_ray_scene(app, what);
    assert_eq!(
        diag(app, "render.ray.sun_joined"),
        f64::from(u8::from(join)),
        "{what}: the trace is joined with the shadow maps: {join}"
    );
}

/// The scene `build` makes over `ground_from`'s ground, drawn after `frames` frames with the
/// shadow maps alone and joined with the trace (`None`: no GPU that traces).
fn maps_and_joined(
    name: &str,
    eye: DVec3,
    target: DVec3,
    frames: usize,
    build: impl Fn(&mut App),
) -> Option<[image::RgbImage; 2]> {
    let mut pictures = Vec::new();
    for join in [false, true] {
        let (mut app, _) = ground_from(join, eye, target);
        build(&mut app);
        app.update_n(frames);
        if !can_run(&app) {
            return None;
        }
        assert_joined(&app, join, name);
        let way = if join { "joined" } else { "maps" };
        pictures.push(picture(&mut app, &format!("{name}_{way}.png")));
    }
    let joined = pictures.pop().expect("two");
    Some([pictures.pop().expect("two"), joined])
}

/// How bright the ground in full sun is in `p`: the brightest tenth of its pixels begins there.
fn lit_level(p: &image::RgbImage) -> f32 {
    let mut values: Vec<f32> = p.pixels().map(mean).collect();
    values.sort_by(f32::total_cmp);
    values[values.len() * 9 / 10]
}

/// Pixels in shadow in both pictures, in the first alone, and in the second alone (darker
/// than half of `lit`).
fn shadow_overlap(a: &image::RgbImage, b: &image::RgbImage, lit: f32) -> [u32; 3] {
    let mut n = [0u32; 3];
    for (p, q) in a.pixels().zip(b.pixels()) {
        let (x, y) = (mean(p) < lit * 0.5, mean(q) < lit * 0.5);
        n[0] += (x && y) as u32;
        n[1] += (x && !y) as u32;
        n[2] += (!x && y) as u32;
    }
    n
}

/// A post too thin for the shadow maps (the lists leave out casters under 60 of cascade 0's
/// texels across, 1.3 m in these tests, from every map) is in the rays: its shadow falls on the
/// ground in the joined picture and not in the maps' one, and nothing else changes.
#[test]
fn a_thin_post_casts_a_shadow_the_maps_lose() {
    // Closer than `open_ground`'s eye: a pixel about 4 cm at the post, its shadow (1 m long
    // along +x, 10 cm wide) about 25 by 2.5 pixels.
    let eye = DVec3::new(0.5, 2.5, 2.0);
    let target = DVec3::new(0.5, GROUND_Y, 0.0);
    let Some([maps, joined]) = maps_and_joined("thin_post", eye, target, 16, |app| {
        {
            let mut s = app.world.resource_mut::<RenderSettings>();
            s.shadow_caster_min_texels = 60.0;
            s.contact_shadow_length = 0.0;
        }
        slab(
            &mut app.world,
            DVec3::new(0.0, GROUND_Y + 0.5, 0.0),
            DVec3::new(0.1, 1.0, 0.1),
            gray(0.5),
        );
    }) else {
        return;
    };
    let lit = lit_level(&maps);
    let [both, maps_only, rays_only] = shadow_overlap(&maps, &joined, lit);
    eprintln!(
        "thin post: {both} pixels dark in both, {maps_only} in the maps' picture alone, \
         {rays_only} in the joined one alone (lit {lit:.1})"
    );
    assert!(
        rays_only >= 30,
        "the post's shadow by ray: {rays_only} pixels dark in the joined picture alone"
    );
    assert!(
        maps_only <= 4,
        "the join takes no shadow away: {maps_only} pixels dark in the maps' picture alone"
    );
}

/// A roof's shadow traced (the roof stands still: it is in the rays and in the still images, so
/// the join leaves it to the rays) is the maps' shadow but at its edge, and as dark.
#[test]
fn a_ray_shadow_and_a_map_shadow_agree_outside_the_penumbra() {
    let eye = DVec3::new(0.0, 7.0, 5.0);
    let Some([maps, joined]) = maps_and_joined("roof", eye, DVec3::ZERO, 16, |app| {
        roof(app, 0.0, 0.0);
    }) else {
        return;
    };
    let lit = lit_level(&maps);
    let [both, maps_only, rays_only] = shadow_overlap(&maps, &joined, lit);
    // How far apart the two are where both see shadow and where both see sun.
    let (mut shade_gap, mut sun_gap, mut shade_n, mut sun_n) = (0.0, 0.0, 0u32, 0u32);
    for (p, q) in maps.pixels().zip(joined.pixels()) {
        let (a, b) = (mean(p), mean(q));
        if a < lit * 0.5 && b < lit * 0.5 {
            shade_gap += (a - b).abs();
            shade_n += 1;
        } else if a > lit * 0.8 && b > lit * 0.8 {
            sun_gap += (a - b).abs();
            sun_n += 1;
        }
    }
    let shade_gap = shade_gap / shade_n.max(1) as f32;
    let sun_gap = sun_gap / sun_n.max(1) as f32;
    eprintln!(
        "roof: {both} pixels dark in both, {maps_only} in the maps' alone, {rays_only} in the \
         joined alone; apart by {shade_gap:.2} in shadow, {sun_gap:.2} in sun (lit {lit:.1})"
    );
    assert!(both > 300, "the roof's shadow is in view: {both} pixels");
    assert!(
        (maps_only + rays_only) * 8 <= both,
        "the shadows differ at their edge alone: {maps_only} + {rays_only} of {both}"
    );
    assert!(
        shade_gap <= 3.0 && sun_gap <= 3.0,
        "as dark and as bright: {shade_gap:.2} apart in shadow, {sun_gap:.2} in sun"
    );
}

/// A roof that moves is not in the still structure the trace asks: its shadow is the maps'
/// alone (not darkened twice), and where it stood still before it moved, in the rays, its
/// traced shadow kept over the frames leaves no trail.
#[test]
fn a_moving_caster_is_shadowed_once_and_leaves_no_trail() {
    let mut pictures = Vec::new();
    for join in [false, true] {
        let mut app = ground(join);
        let moving = roof(&mut app, 0.0, 1.0);
        app.update_n(16);
        if !can_run(&app) {
            return;
        }
        // 4 m toward the view's top over 40 frames: the shadow leaves where it fell.
        for _ in 0..40 {
            app.world
                .get_mut::<Transform>(moving)
                .expect("the roof")
                .translation
                .z -= 0.1;
            app.update();
        }
        assert_joined(&app, join, "a moving roof");
        let way = if join { "joined" } else { "maps" };
        pictures.push(picture(&mut app, &format!("moving_{way}.png")));
    }
    let [maps, joined] = [&pictures[0], &pictures[1]];
    let lit = lit_level(maps);
    let [both, maps_only, rays_only] = shadow_overlap(maps, joined, lit);
    let (mut most, mut sum) = (0u8, 0u64);
    for (a, b) in maps.pixels().zip(joined.pixels()) {
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        most = most.max(d);
        sum += d as u64;
    }
    let gap = sum as f32 / (W * H) as f32;
    eprintln!(
        "moving roof: {both} pixels dark in both, {maps_only} in the maps' alone, {rays_only} \
         in the joined alone; apart by {gap:.2} on average, {most} at most"
    );
    assert!(
        both > 300,
        "the moving roof's shadow is in view: {both} pixels"
    );
    assert!(
        maps_only + rays_only <= both / 50,
        "the same shadow, and no trail where the roof stood: {maps_only} + {rays_only} of {both}"
    );
    assert!(
        gap <= 1.5,
        "the moving roof's shadow is the maps' alone: apart by {gap:.2}"
    );
}

/// A slab 75 m up-sun of the ground in view stands farther than the rays reach (64 m): the
/// rays pass under it, the maps hold it, and the join takes its shadow from the maps
/// (`shadow_rest` counts what stands past 0.9 of the rays' length).
#[test]
fn a_far_blocker_still_shades_a_traced_pixel() {
    // Up-sun of the origin: (-1, 1, 0) / sqrt 2 (the sun shines toward (1, -1, 0)).
    let up = 75.0 / 2f64.sqrt();
    let eye = DVec3::new(0.0, 7.0, 5.0);
    let Some([maps, joined]) = maps_and_joined("far_blocker", eye, DVec3::ZERO, 16, |app| {
        // 40 m by 10 m: its shadow covers the ground at z between -5 and 5, the view's
        // lower half.
        slab(
            &mut app.world,
            DVec3::new(-up, GROUND_Y + up, 0.0),
            DVec3::new(40.0, 1.0, 10.0),
            gray(0.3),
        );
    }) else {
        return;
    };
    let lit = lit_level(&maps);
    let [both, maps_only, rays_only] = shadow_overlap(&maps, &joined, lit);
    eprintln!(
        "far blocker: {both} pixels dark in both, {maps_only} in the maps' alone, {rays_only} \
         in the joined alone (lit {lit:.1})"
    );
    assert!(
        both > W * H / 4,
        "the far slab's shadow covers much of the view: {both} pixels"
    );
    assert!(
        (maps_only + rays_only) * 10 <= both,
        "the join keeps it: {maps_only} pixels dark in the maps' picture alone, {rays_only} in \
         the joined one alone"
    );
}

/// The trace's history goes on from frame to frame, and a camera cut drops it: the frame of a
/// 10 m jump starts again (`render.ray.sun_history` 0, `render.history_cut` 1) and the next
/// goes on; the roof's shadow is there from the frame after the cut.
#[test]
fn after_a_cut_no_shadow_history_survives() {
    let (mut app, camera) = ground_from(true, DVec3::new(0.0, 7.0, 5.0), DVec3::ZERO);
    roof(&mut app, 0.0, 0.0);
    roof(&mut app, 10.0, 0.0);
    app.update_n(12);
    if !can_run(&app) {
        return;
    }
    assert_joined(&app, true, "before the cut");
    assert_eq!(
        diag(&app, "render.ray.sun_history"),
        1.0,
        "the history goes on from frame to frame"
    );
    let jump = |app: &mut App| {
        app.world
            .get_mut::<Transform>(camera)
            .expect("the camera")
            .translation
            .x += 10.0;
        app.update();
    };
    jump(&mut app);
    assert_eq!(
        diag(&app, "render.history_cut"),
        1.0,
        "a 10 m jump is a cut"
    );
    assert_eq!(
        diag(&app, "render.ray.sun_history"),
        0.0,
        "the frame of the cut starts the trace's history again"
    );
    assert_joined(&app, true, "the frame of the cut");
    // (The picture is the frame after the cut, the second of the new history.)
    let after = picture(&mut app, "cut_after.png");
    assert_eq!(
        diag(&app, "render.ray.sun_history"),
        1.0,
        "the frame after the cut goes on from it"
    );
    app.update_n(12);
    let settled = picture(&mut app, "cut_settled.png");
    let lit = lit_level(&settled);
    let [both, early, late] = shadow_overlap(&after, &settled, lit);
    eprintln!(
        "after the cut: {both} pixels dark in both, {early} in the frame after alone, {late} \
         settled alone"
    );
    assert!(
        both > 300,
        "the second roof's shadow is in view: {both} pixels"
    );
    assert!(
        (early + late) * 8 <= both,
        "the shadow is there from the frame after the cut: {early} + {late} of {both}"
    );
}

/// What the rays don't hold keeps the shadow maps' shadow. A roof past the rays' reaches (cut
/// to a meter: it is in no ray) is held by its still image, which holds a caster within the
/// rays' length all round the pixels of its shadow (all but a texel at its edge); a roof whose
/// shadow falls past the traced pixels' reach (`ray_sun_reach` 12 m, the shadow about 15 m
/// away) is lit by the maps alone there.
/// A case of `a_still_caster_the_rays_leave_out_keeps_its_map_shadow`: its name, the settings
/// it changes, and where its roof stands (z).
type LeftOut = (&'static str, fn(&mut RenderSettings), f64);

#[test]
fn a_still_caster_the_rays_leave_out_keeps_its_map_shadow() {
    let eye = DVec3::new(0.0, 7.0, 5.0);
    let cases: [LeftOut; 2] = [
        (
            "unreached",
            |s| {
                s.ray_caster_reach = 1.0;
                s.ray_far_reach = 1.0;
            },
            0.0,
        ),
        ("untraced", |s| s.ray_sun_reach = 12.0, -8.0),
    ];
    for (name, set, z) in cases {
        let Some([maps, joined]) = maps_and_joined(name, eye, DVec3::ZERO, 16, |app| {
            set(&mut app.world.resource_mut::<RenderSettings>());
            roof(app, 0.0, z);
        }) else {
            return;
        };
        let lit = lit_level(&maps);
        let [both, maps_only, rays_only] = shadow_overlap(&maps, &joined, lit);
        eprintln!(
            "{name}: {both} pixels dark in both, {maps_only} in the maps' alone, {rays_only} in \
             the joined alone (lit {lit:.1})"
        );
        assert!(
            both > 150,
            "{name}: the roof's shadow is in view: {both} pixels"
        );
        assert!(
            (maps_only + rays_only) * 10 <= both,
            "{name}: the maps' shadow stands: {maps_only} pixels dark in their picture alone, \
             {rays_only} in the joined one alone"
        );
    }
}
