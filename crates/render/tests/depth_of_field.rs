//! Depth of field (`DepthOfField` on the camera): what is in focus keeps its detail and what is
//! far behind loses it, a small aperture keeps everything sharp, a blurred foreground edge is
//! soft and spreads over the sharp background behind it, an in-focus edge in front of a blurred
//! background stays crisp with no halo, a far blurred background spreads no farther over a less
//! blurred object in front of it than that object's own blur, a small bright light opens into a
//! round, even disc, and autofocus finds the object at the screen center (ignoring sky through a
//! gap), eases toward a new distance and snaps after a camera cut. Needs a GPU or lavapipe;
//! passes with a note when neither is available.
//!
//! The camera stands at the origin looking down -Z through an 85 mm lens on a full-frame
//! sensor; the scenes are unlit, so every pixel is exactly the color it was given.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 480;
const H: u32 = 270;

fn fov_y_deg() -> f32 {
    DepthOfField::fov_y_deg_for_lens(85.0, 24.0)
}

/// Pixels per meter, sideways, of something `distance` m in front of the camera.
fn px_per_m(distance: f64) -> f64 {
    0.5 * H as f64 / ((0.5 * fov_y_deg() as f64).to_radians().tan() * distance)
}

/// Where `p` (in front of the camera at the origin, looking down -Z) lands on screen.
fn to_screen(p: DVec3) -> (f64, f64) {
    let s = px_per_m(-p.z);
    (0.5 * W as f64 + p.x * s, 0.5 * H as f64 - p.y * s)
}

/// The circle of confusion radius (pixels) of a point `distance` m away with the lens focused
/// at `focus` m.
fn coc_px(dof: &DepthOfField, distance: f32, focus: f32) -> f32 {
    dof.coc_radius_px(distance, focus, fov_y_deg().to_radians(), H as f32)
}

fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.vignette = 0.0;
        s.bloom_intensity = 0.0;
        s.volumetric_fog = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app
}

fn camera(app: &mut App, dof: Option<DepthOfField>) -> Entity {
    let mut e = app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 0.0 },
            ..Camera::perspective(fov_y_deg())
        },
        Transform::default(),
    ));
    if let Some(dof) = dof {
        e.insert(dof);
    }
    e.id()
}

fn unlit(rgb: [f32; 3]) -> StandardMaterial {
    StandardMaterial {
        unlit: true,
        ..StandardMaterial::color(rgb[0], rgb[1], rgb[2])
    }
}

/// An unlit quad facing the camera, `size` m, centered at `at`.
fn quad(app: &mut App, at: DVec3, size: [f32; 2], material: StandardMaterial) {
    app.world.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(Shape::Quad { size }, material).without_shadows(),
    ));
}

/// An unlit black-and-white checkerboard `size` m square at `at`, `cells` cells a side.
fn checkerboard(app: &mut App, at: DVec3, size: f32, cells: u32) {
    let image = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(Image::checker(
            256,
            cells,
            [255, 255, 255, 255],
            [24, 24, 24, 255],
        ));
    quad(
        app,
        at,
        [size, size],
        unlit([1.0, 1.0, 1.0]).with_base_color_texture(image),
    );
}

fn gpu_missing(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return true;
    }
    false
}

fn shoot(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("dof_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let gpu_errors: Vec<String> = engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect();
    assert!(
        gpu_errors.is_empty(),
        "GPU errors while rendering {name}: {gpu_errors:#?}"
    );
    image::open(&path).expect("png").to_rgba8()
}

fn luma(img: &RgbaImage, x: u32, y: u32) -> f64 {
    let p = img.get_pixel(x.min(W - 1), y.min(H - 1));
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

/// Pixels of a rectangle (x0, y0, x1, y1), half open.
type Rect = (u32, u32, u32, u32);

/// The middle 60% of a square `size` m across centered at `at`.
fn inner(at: DVec3, size: f64) -> Rect {
    let (x, y) = to_screen(at);
    let h = 0.3 * size * px_per_m(-at.z);
    (
        (x - h).max(0.0) as u32,
        (y - h).max(0.0) as u32,
        ((x + h) as u32).min(W),
        ((y + h) as u32).min(H),
    )
}

/// Standard deviation of the luma over `r`: how much detail is left.
fn contrast(img: &RgbaImage, r: Rect) -> f64 {
    let values: Vec<f64> = (r.1..r.3)
        .flat_map(|y| (r.0..r.2).map(move |x| (x, y)))
        .map(|(x, y)| luma(img, x, y))
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

/// Mean luma of column `x` over rows `y0..y1`.
fn column(img: &RgbaImage, x: u32, y0: u32, y1: u32) -> f64 {
    (y0..y1).map(|y| luma(img, x, y)).sum::<f64>() / (y1 - y0) as f64
}

const NEAR_BOARD: DVec3 = DVec3::new(-0.24, 0.0, -2.0);
const FAR_BOARD: DVec3 = DVec3::new(2.4, 0.0, -20.0);

/// A checkerboard 2 m away (12 px cells) and one 20 m away (as large on screen).
fn two_boards(dof: Option<DepthOfField>, name: &str) -> Option<(RgbaImage, f64, f64)> {
    let mut app = app();
    camera(&mut app, dof);
    checkerboard(&mut app, NEAR_BOARD, 0.4, 16);
    checkerboard(&mut app, FAR_BOARD, 4.0, 16);
    app.update_n(3);
    if gpu_missing(&app) {
        return None;
    }
    let img = shoot(&mut app, name);
    let near = contrast(&img, inner(NEAR_BOARD, 0.4));
    let far = contrast(&img, inner(FAR_BOARD, 4.0));
    eprintln!("{name}: contrast near {near:.1}, far {far:.1}");
    Some((img, near, far))
}

#[test]
fn the_focus_plane_stays_sharp_and_far_behind_it_blurs() {
    let Some((_, near_off, far_off)) = two_boards(None, "off") else {
        return;
    };
    let dof = DepthOfField::new(2.0, 1.4);
    eprintln!("CoC 20 m behind: {:.1} px", coc_px(&dof, 20.0, 2.0));
    let (_, near, far) = two_boards(Some(dof), "f1.4").unwrap();
    assert!(
        near_off > 60.0 && far_off > 60.0,
        "the boards aren't in view"
    );
    assert!(
        near > 0.9 * near_off,
        "the board in focus lost detail: contrast {near:.1} (off {near_off:.1})"
    );
    assert!(
        far < 0.25 * far_off,
        "the board far behind the focus kept detail: contrast {far:.1} (off {far_off:.1})"
    );
}

#[test]
fn a_tiny_aperture_keeps_both_sharp() {
    let Some((_, near_off, far_off)) = two_boards(None, "off_ref") else {
        return;
    };
    let dof = DepthOfField::new(2.0, 22.0);
    eprintln!("CoC 20 m behind at f/22: {:.2} px", coc_px(&dof, 20.0, 2.0));
    let (_, near, far) = two_boards(Some(dof), "f22").unwrap();
    assert!(
        near > 0.9 * near_off && far > 0.9 * far_off,
        "f/22 should keep both sharp: near {near:.1} (off {near_off:.1}), far {far:.1} (off {far_off:.1})"
    );
}

/// Mean luma over the screen rectangle of a rectangle on the plane `z` m in front of the
/// camera, from (x0, y0) to (x1, y1) m, less a 2 px margin.
fn plane_mean(img: &RgbaImage, z: f64, (x0, y0): (f64, f64), (x1, y1): (f64, f64)) -> f64 {
    let (left, bottom) = to_screen(DVec3::new(x0, y0, -z));
    let (right, top) = to_screen(DVec3::new(x1, y1, -z));
    let (left, right, top, bottom) = (
        left as u32 + 2,
        right as u32 - 2,
        top as u32 + 2,
        bottom as u32 - 2,
    );
    let sum: f64 = (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .map(|(x, y)| luma(img, x, y))
        .sum();
    sum / ((right - left) * (bottom - top)) as f64
}

/// Gray of the wall behind the black foreground board.
const WALL: f64 = 0.5;
/// Gray patches from black to the wall's gray, for undoing the tonemapping.
const PATCHES: usize = 11;

fn patch_gray(i: usize) -> f64 {
    WALL * i as f64 / (PATCHES - 1) as f64
}

/// A black board 1 m away covering the left half of the view, in front of a gray wall 10 m away
/// with a checkered strip along its top, the lens focused on the wall. Along the bottom right,
/// in focus: patches from black to the wall's gray, to undo the tonemapping when measuring how
/// much of the black board covers the wall. With `band_m`, a band of the same gray `band_m` away
/// stands in front of the wall where the coverage is measured.
fn foreground_edge(
    dof: Option<DepthOfField>,
    band_m: Option<f64>,
    name: &str,
) -> Option<RgbaImage> {
    let mut app = app();
    camera(&mut app, dof);
    quad(
        &mut app,
        DVec3::new(0.0, 0.0, -10.0),
        [6.0, 6.0],
        unlit([WALL as f32; 3]),
    );
    if let Some(d) = band_m {
        // Rows from 0.4 m above the axis to 0.6 m below it on the wall.
        let s = d / 10.0;
        quad(
            &mut app,
            DVec3::new(0.0, -0.1 * s, -d),
            [6.0 * s as f32, 1.0 * s as f32],
            unlit([WALL as f32; 3]),
        );
    }
    let image = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(Image::checker(
            256,
            8,
            [255, 255, 255, 255],
            [24, 24, 24, 255],
        ));
    quad(
        &mut app,
        DVec3::new(1.25, 0.9, -9.98),
        [2.5, 0.6],
        unlit([1.0; 3])
            .with_base_color_texture(image)
            .with_uv_scale(4.0, 1.0),
    );
    for i in 0..PATCHES {
        quad(
            &mut app,
            DVec3::new(1.15 + 0.12 * i as f64, -1.05, -9.98),
            [0.12, 0.3],
            unlit([patch_gray(i) as f32; 3]),
        );
    }
    quad(
        &mut app,
        DVec3::new(-0.5, 0.0, -1.0),
        [1.0, 1.0],
        unlit([0.0; 3]),
    );
    app.update_n(3);
    if gpu_missing(&app) {
        return None;
    }
    Some(shoot(&mut app, name))
}

/// How much of the black board covers the gray behind it at `CoC / 6` steps across its edge,
/// from -8 to 8 (linear, from the patches).
fn edge_coverage(img: &RgbaImage, coc: f64) -> Vec<f64> {
    let levels: Vec<(f64, f64)> = (0..PATCHES)
        .map(|i| {
            let x = 1.09 + 0.12 * i as f64;
            let d = plane_mean(img, 10.0, (x, -1.2), (x + 0.12, -0.9));
            (d, patch_gray(i))
        })
        .collect();
    let linear = |d: f64| {
        let i = levels
            .windows(2)
            .position(|w| d <= w[1].0)
            .unwrap_or(PATCHES - 2);
        let (a, b) = (levels[i], levels[i + 1]);
        a.1 + (b.1 - a.1) * ((d - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0)
    };
    let edge = to_screen(DVec3::new(0.0, 0.0, -1.0)).0;
    // Rows of plain gray.
    let (y0, y1) = (
        to_screen(DVec3::new(0.0, 0.35, -10.0)).1 as u32,
        to_screen(DVec3::new(0.0, -0.55, -10.0)).1 as u32,
    );
    (-8..=8)
        .map(|i| {
            let x = edge + i as f64 * coc / 6.0;
            1.0 - linear(column(img, x.floor() as u32, y0, y1)) / WALL
        })
        .collect()
}

/// The coverage profile across a blurred foreground's edge: opaque well inside, fading out
/// over its blur on both sides of its edge, gone past it.
fn assert_soft_edge(profile: &[f64], what: &str) {
    let shown: Vec<String> = profile.iter().map(|c| format!("{c:.2}")).collect();
    eprintln!("{what}: coverage across the edge, every CoC/6: {shown:?}");
    // Index 8 is the edge; every 3 steps is half the CoC.
    for (i, lo, hi, where_) in [
        (0, 0.97, 1.02, "deep inside the foreground"),
        (
            5,
            0.6,
            0.9,
            "half its blur inside its edge (it should fade out)",
        ),
        (7, 0.5, 0.78, "a sixth of its blur inside its edge"),
        (8, 0.3, 0.7, "at its edge"),
        (
            11,
            0.05,
            0.4,
            "half its blur outside its edge (it should spread over what's behind)",
        ),
        (16, -0.02, 0.02, "past its blur"),
    ] {
        let c = profile[i];
        assert!(
            (lo..=hi).contains(&c),
            "{what}: the foreground covers {c:.2} {where_}; expected {lo}..{hi} ({shown:?})"
        );
    }
}

#[test]
fn a_blurred_foreground_edge_is_soft_and_spreads_over_the_sharp_background() {
    let Some(off) = foreground_edge(None, None, "edge_off") else {
        return;
    };
    let dof = DepthOfField::new(10.0, 1.4);
    let blurred = foreground_edge(Some(dof), None, "edge").unwrap();
    let r = coc_px(&dof, 1.0, 10.0).abs().min(dof.max_radius_px) as f64;
    assert_soft_edge(&edge_coverage(&blurred, r), "over a sharp wall");
    // The checkered strip past the foreground's blur keeps its detail.
    let x0 = (to_screen(DVec3::new(0.0, 0.0, -1.0)).0 + r + 6.0) as u32;
    let sharp = (
        x0,
        to_screen(DVec3::new(0.0, 1.15, -10.0)).1 as u32,
        x0 + 48,
        to_screen(DVec3::new(0.0, 0.65, -10.0)).1 as u32,
    );
    let (c, c_off) = (contrast(&blurred, sharp), contrast(&off, sharp));
    eprintln!("wall detail past the blur: contrast {c:.1} (off {c_off:.1})");
    assert!(c_off > 40.0, "the checkered strip isn't in view");
    assert!(
        c > 0.9 * c_off,
        "the wall in focus lost detail: {c:.1} (off {c_off:.1})"
    );
}

/// The same board in front of a gray band 3 m away, itself in front of the focus plane but far
/// less blurred: the board's blur still spreads over it, the nearer over the farther.
#[test]
fn a_blurred_foreground_spreads_over_a_less_blurred_one_behind_it() {
    let dof = DepthOfField::new(10.0, 1.4);
    let Some(blurred) = foreground_edge(Some(dof), Some(3.0), "edge_layered") else {
        return;
    };
    let r = coc_px(&dof, 1.0, 10.0).abs().min(dof.max_radius_px) as f64;
    eprintln!("band CoC {:.1} px", coc_px(&dof, 3.0, 10.0));
    assert_soft_edge(&edge_coverage(&blurred, r), "over a band 3 m away");
}

/// A black board 2 m away, in focus, covering the left half of the view, in front of a white
/// wall 20 m away, far out of focus.
#[test]
fn an_in_focus_edge_in_front_of_a_blurred_background_has_no_halo() {
    let render = |dof: Option<DepthOfField>, name: &str| {
        let mut app = app();
        camera(&mut app, dof);
        quad(
            &mut app,
            DVec3::new(0.0, 0.0, -20.0),
            [20.0, 20.0],
            unlit([0.8, 0.8, 0.8]),
        );
        quad(
            &mut app,
            DVec3::new(-1.0, 0.0, -2.0),
            [2.0, 2.0],
            unlit([0.0, 0.0, 0.0]),
        );
        app.update_n(3);
        if gpu_missing(&app) {
            return None;
        }
        Some(shoot(&mut app, name))
    };
    let Some(off) = render(None, "halo_off") else {
        return;
    };
    let dof = DepthOfField::new(2.0, 1.4);
    let blurred = render(Some(dof), "halo").unwrap();
    let edge = to_screen(DVec3::new(0.0, 0.0, -2.0)).0.round() as u32;
    let (y0, y1) = (40, H - 40);
    let profile: Vec<String> = (edge - 6..edge + 6)
        .map(|x| {
            format!(
                "{:.0}/{:.0}",
                column(&off, x, y0, y1),
                column(&blurred, x, y0, y1)
            )
        })
        .collect();
    eprintln!("off/on across the edge at x = {edge}: {profile:?}");
    for d in 2..6 {
        let (on, sharp) = (
            column(&blurred, edge - d, y0, y1),
            column(&off, edge - d, y0, y1),
        );
        assert!(
            (on - sharp).abs() < 3.0,
            "the in-focus board {d} px from its edge changed: {on:.1} (off {sharp:.1})"
        );
        let (on, sharp) = (
            column(&blurred, edge + d, y0, y1),
            column(&off, edge + d, y0, y1),
        );
        assert!(
            (on - sharp).abs() < 3.0,
            "the background {d} px from the in-focus edge changed: {on:.1} (off {sharp:.1})"
        );
    }
}

/// A black board 3 m away covering the left half of the view, in front of a white wall 60 m
/// away, the lens focused 2 m away: both are behind the focus plane, the wall far more blurred
/// than the board. The board hides what is behind it, so the wall's blur spreads no farther over
/// it than the board's own, and the board's blurred edge fades out around where its edge is
/// (half covering the wall there), as it would in front of a sharp wall. Below the measured rows,
/// in focus: gray patches as the board covering 30%, 60%, 70% and 90% of the wall would look.
#[test]
fn a_blurred_object_hides_the_more_blurred_background_behind_it() {
    const WALL_GRAY: f32 = 0.8;
    let patch = |coverage: f64| DVec3::new(0.1 + 0.4 * coverage, -0.24, -2.0);
    let render = |dof: Option<DepthOfField>, name: &str| {
        let mut app = app();
        camera(&mut app, dof);
        quad(
            &mut app,
            DVec3::new(0.0, 0.0, -60.0),
            [60.0, 60.0],
            unlit([WALL_GRAY; 3]),
        );
        quad(
            &mut app,
            DVec3::new(-1.5, 0.0, -3.0),
            [3.0, 3.0],
            unlit([0.0, 0.0, 0.0]),
        );
        for coverage in [0.3, 0.6, 0.7, 0.9] {
            let gray = WALL_GRAY * (1.0 - coverage as f32);
            quad(&mut app, patch(coverage), [0.04, 0.04], unlit([gray; 3]));
        }
        app.update_n(3);
        if gpu_missing(&app) {
            return None;
        }
        Some(shoot(&mut app, name))
    };
    let Some(off) = render(None, "occlusion_off") else {
        return;
    };
    let dof = DepthOfField::new(2.0, 1.4);
    let blurred = render(Some(dof), "occlusion").unwrap();
    let (board, wall) = (coc_px(&dof, 3.0, 2.0), coc_px(&dof, 60.0, 2.0));
    let edge = to_screen(DVec3::new(0.0, 0.0, -3.0)).0.round() as u32;
    let (y0, y1) = (40, H - 40);
    let profile: Vec<String> = (edge - wall.ceil() as u32 - 1..edge + board.ceil() as u32 + 2)
        .map(|x| format!("{:.0}", column(&blurred, x, y0, y1)))
        .collect();
    let patch_luma = |coverage: f64| {
        let (x, y) = to_screen(patch(coverage));
        let (x, y) = (x as u32, y as u32);
        (y - 3..y + 4)
            .flat_map(|y| (x - 3..x + 4).map(move |x| (x, y)))
            .map(|(x, y)| luma(&blurred, x, y))
            .sum::<f64>()
            / 49.0
    };
    // The pixels on either side of the edge (it falls between two columns), and the column half
    // the board's blur inside it.
    let at_edge = 0.5 * (column(&blurred, edge - 1, y0, y1) + column(&blurred, edge, y0, y1));
    let half_inside = column(&blurred, (edge as f32 - 0.5 * board) as u32, y0, y1);
    eprintln!(
        "board CoC {board:.1} px, wall CoC {wall:.1} px; at the edge {at_edge:.0}, half its blur \
         inside {half_inside:.0} (90/70/60/30% covered {:.0}/{:.0}/{:.0}/{:.0}); across it from \
         the board: {profile:?}",
        patch_luma(0.9),
        patch_luma(0.7),
        patch_luma(0.6),
        patch_luma(0.3)
    );
    for (value, (most, least), what) in [
        (at_edge, (0.7, 0.3), "where its edge is"),
        (half_inside, (0.9, 0.6), "half its blur inside its edge"),
    ] {
        let (lo, hi) = (patch_luma(most), patch_luma(least));
        assert!(
            (lo..=hi).contains(&value),
            "the board's blurred edge should fade out over the wall {what}: {value:.0}, \
             expected {lo:.0}..{hi:.0} ({:.0}%..{:.0}% covered)",
            100.0 * most,
            100.0 * least
        );
    }
    let inside = (board + 2.0).ceil() as u32..(wall - 2.0).floor() as u32;
    assert!(inside.len() >= 4, "the scene should leave room: {inside:?}");
    for d in inside {
        let (on, sharp) = (
            column(&blurred, edge - d, y0, y1),
            column(&off, edge - d, y0, y1),
        );
        assert!(
            (on - sharp).abs() < 3.0,
            "the wall spread over the board {d} px inside its edge, past the board's own \
             {board:.1} px blur: {on:.1} (off {sharp:.1})"
        );
    }
}

/// A small bright light 20 m away with the lens focused 1.2 m away opens into a disc: round,
/// as large as the circle of confusion, and evenly lit.
#[test]
fn a_bright_point_opens_into_a_round_even_disc() {
    let dof = DepthOfField::new(1.2, 1.4);
    let mut app = app();
    camera(&mut app, Some(dof));
    quad(
        &mut app,
        DVec3::new(0.0, 0.0, -60.0),
        [60.0, 60.0],
        unlit([0.0, 0.0, 0.0]),
    );
    let at = DVec3::new(0.0, 0.0, -20.0);
    app.world.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.04,
                segments: 16,
            },
            unlit([0.0, 0.0, 0.0]).with_emissive([1.0, 1.0, 1.0], 60.0),
        )
        .without_shadows(),
    ));
    app.update_n(3);
    if gpu_missing(&app) {
        return;
    }
    let img = shoot(&mut app, "disc");
    let expected = coc_px(&dof, 20.0, 1.2) as f64;
    let (cx, cy) = to_screen(at);
    // Interior: within 60% of the radius.
    let mut interior: Vec<f64> = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            if d < 0.6 * expected {
                interior.push(luma(&img, x, y));
            }
        }
    }
    interior.sort_by(|a, b| a.total_cmp(b));
    let at_q = |q: f64| interior[((interior.len() - 1) as f64 * q) as usize];
    let median = at_q(0.5);
    // Radius in 16 directions: where the luma falls under half the interior's.
    let radii: Vec<f64> = (0..16)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 16.0;
            let mut r = 0.0;
            while r < 3.0 * expected {
                let (x, y) = (cx + a.cos() * r, cy + a.sin() * r);
                if luma(&img, x as u32, y as u32) < 0.5 * median {
                    break;
                }
                r += 0.25;
            }
            r
        })
        .collect();
    let mean = radii.iter().sum::<f64>() / radii.len() as f64;
    let spread = radii.iter().map(|r| (r - mean).abs()).fold(0.0, f64::max);
    eprintln!(
        "disc: expected radius {expected:.1} px, measured {mean:.1} (largest deviation \
         {spread:.1}); interior luma 5%/50%/95% {:.0}/{median:.0}/{:.0}; radii {radii:.1?}",
        at_q(0.05),
        at_q(0.95)
    );
    assert!(
        median > 40.0 && median < 230.0,
        "the disc should be lit but not clipped: {median:.0}"
    );
    assert!(
        (mean - expected).abs() < 0.15 * expected,
        "disc radius {mean:.1} px, expected {expected:.1}"
    );
    assert!(
        spread < 0.12 * expected,
        "the disc isn't round: radii {radii:.1?}"
    );
    assert!(
        at_q(0.95) - at_q(0.05) < 0.1 * median,
        "the disc isn't evenly lit: 5%/95% {:.0}/{:.0} around {median:.0}",
        at_q(0.05),
        at_q(0.95)
    );
}

fn focus_m(app: &App) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get("render.dof_focus_m")
        .unwrap_or(f64::NAN)
}

/// A checkerboard 1 m square `distance` m in front of the camera, cut down the middle by a slit
/// a few pixels wide with nothing (sky) behind it, which the autofocus window straddles.
fn slit_board(app: &mut App, distance: f64) -> [Entity; 2] {
    let gap = 6.0 / px_per_m(distance);
    let half = 0.5 - 0.5 * gap;
    [-1.0, 1.0].map(|side| {
        let image = app
            .world
            .resource_mut::<Assets<Image>>()
            .add(Image::checker(
                256,
                8,
                [255, 255, 255, 255],
                [24, 24, 24, 255],
            ));
        app.world
            .spawn((
                Transform::from_xyz(side * (0.5 * gap + 0.5 * half), 0.0, -distance),
                MeshRenderer::new(
                    Shape::Quad {
                        size: [half as f32, 1.0],
                    },
                    unlit([1.0, 1.0, 1.0]).with_base_color_texture(image),
                )
                .without_shadows(),
            ))
            .id()
    })
}

#[test]
fn autofocus_finds_the_object_at_the_center_past_a_gap() {
    let mut app = app();
    camera(&mut app, Some(DepthOfField::autofocus(1.4)));
    slit_board(&mut app, 5.0);
    // Something far behind, beside it.
    checkerboard(&mut app, DVec3::new(-6.0, 0.0, -40.0), 8.0, 16);
    app.update_n(6);
    if gpu_missing(&app) {
        return;
    }
    let focus = focus_m(&app);
    let img = shoot(&mut app, "autofocus");
    let mut off = self::app();
    camera(&mut off, None);
    slit_board(&mut off, 5.0);
    checkerboard(&mut off, DVec3::new(-6.0, 0.0, -40.0), 8.0, 16);
    off.update_n(3);
    let sharp = shoot(&mut off, "autofocus_off");
    let subject = (
        (0.5 * W as f64 + 0.1 * px_per_m(5.0)) as u32,
        (0.5 * H as f64 - 0.3 * px_per_m(5.0)) as u32,
        (0.5 * W as f64 + 0.4 * px_per_m(5.0)) as u32,
        (0.5 * H as f64 + 0.3 * px_per_m(5.0)) as u32,
    );
    let (c, c_off) = (contrast(&img, subject), contrast(&sharp, subject));
    eprintln!("autofocus at {focus:.2} m; subject contrast {c:.1} (off {c_off:.1})");
    assert!(
        (focus - 5.0).abs() < 0.15,
        "autofocus should find the board 5 m away, not {focus:.2} m"
    );
    assert!(
        c > 0.9 * c_off,
        "the subject should be sharp: contrast {c:.1} (off {c_off:.1})"
    );
}

#[test]
fn autofocus_eases_toward_a_new_distance_and_snaps_after_a_cut() {
    let mut app = app();
    let cam = camera(&mut app, Some(DepthOfField::autofocus(1.4)));
    let boards = slit_board(&mut app, 5.0);
    app.update_n(6);
    if gpu_missing(&app) {
        return;
    }
    let settled = focus_m(&app);
    // The subject steps back to 10 m: the focus follows over a fraction of a second.
    for b in boards {
        app.world.get_mut::<Transform>(b).unwrap().translation.z -= 5.0;
    }
    app.update_n(4);
    let easing = focus_m(&app);
    app.update_n(60);
    let followed = focus_m(&app);
    // The camera cuts 6 m back: the focus jumps to the subject's new distance.
    app.world.get_mut::<Transform>(cam).unwrap().translation.z += 6.0;
    app.update_n(3);
    let after_cut = focus_m(&app);
    eprintln!(
        "focus: settled {settled:.2} m, 4 frames after the subject stepped back {easing:.2} m, \
         a second later {followed:.2} m, 3 frames after the cut {after_cut:.2} m"
    );
    assert!((settled - 5.0).abs() < 0.15, "settled at {settled:.2} m");
    assert!(
        easing > 5.1 && easing < 7.0,
        "autofocus should ease toward the new distance, not jump: {easing:.2} m"
    );
    assert!((followed - 10.0).abs() < 0.4, "followed to {followed:.2} m");
    assert!(
        (after_cut - 16.0).abs() < 0.6,
        "autofocus should snap after a cut: {after_cut:.2} m (subject 16 m away)"
    );
}

/// Two consecutive frames of a still scene with TAA, whose jitter moves every silhouette by a
/// fraction of a pixel each frame: an in-focus board over a blurred wall and a blurred post over
/// it. Returns the largest change of any pixel between the frames, and the mean change.
fn taa_flicker(dof: Option<DepthOfField>, name: &str) -> Option<(f64, f64)> {
    let mut app = app();
    app.world.resource_mut::<RenderSettings>().anti_aliasing = AntiAliasing::Taa;
    camera(&mut app, dof);
    checkerboard(&mut app, DVec3::new(0.0, 0.0, -20.0), 12.0, 24);
    // In focus, tilted so its edge is a slanted line.
    app.world.spawn((
        Transform::from_xyz(-0.35, -0.05, -2.0).with_rotation(Quat::from_rotation_z(0.3)),
        MeshRenderer::new(Shape::Quad { size: [0.5, 0.5] }, unlit([0.05, 0.05, 0.05]))
            .without_shadows(),
    ));
    // Far out of focus in front.
    app.world.spawn((
        Transform::from_xyz(0.18, 0.0, -0.8).with_rotation(Quat::from_rotation_z(-0.2)),
        MeshRenderer::new(Shape::Quad { size: [0.05, 1.0] }, unlit([0.9, 0.5, 0.2]))
            .without_shadows(),
    ));
    app.update_n(24);
    if gpu_missing(&app) {
        return None;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let mut frames = Vec::new();
    for i in 0..2 {
        let path = dir.join(format!("dof_taa_{name}_{i}.png"));
        app.world
            .resource_mut::<engine_render::Screenshots>()
            .request(&path);
        app.update();
        frames.push(image::open(&path).expect("png").to_rgba8());
    }
    let diffs: Vec<f64> = frames[0]
        .pixels()
        .zip(frames[1].pixels())
        .map(|(a, b)| {
            (0..3)
                .map(|c| (a[c] as f64 - b[c] as f64).abs())
                .fold(0.0, f64::max)
        })
        .collect();
    let max = diffs.iter().copied().fold(0.0, f64::max);
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    eprintln!("{name}: frame to frame change: largest {max}, mean {mean:.3}");
    Some((max, mean))
}

#[test]
fn depth_of_field_does_not_flicker_under_taa_jitter() {
    let Some((max_off, mean_off)) = taa_flicker(None, "off") else {
        return;
    };
    let (max, mean) = taa_flicker(Some(DepthOfField::new(2.0, 1.4)), "on").unwrap();
    assert!(
        max <= max_off.max(8.0) && mean <= mean_off + 0.1,
        "depth of field flickers between TAA frames: largest change {max} (off {max_off}), \
         mean {mean:.3} (off {mean_off:.3})"
    );
}
