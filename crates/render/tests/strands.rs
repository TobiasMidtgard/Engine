//! The strand renderer (characters week R2): a curtain of fair hair half a meter from the
//! camera, thinning toward its sides, with a box in front of its right part. The hair shows
//! where it is dense, fades through partly covered pixels toward its sides (real coverage, not
//! all-or-nothing pixels), and nothing of it shows through the box. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, StrandGroom, StrandInstance, capture_now};
use image::RgbImage;

const W: u32 = 320;
const H: u32 = 240;
const STRANDS: usize = 3000;
const POINTS: u32 = 10;

/// Strands hanging 20 cm from y = 0.1 m, spread across x around 0 (denser in the middle), with
/// a gentle wave each its own way.
fn curtain() -> StrandGroom {
    let mut g = StrandGroom {
        points_per: POINTS,
        ..Default::default()
    };
    let mut seed = 12345u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for _ in 0..STRANDS {
        // A triangle distribution over ±9 cm: dense in the middle, sparse at the sides.
        let x = (rand() + rand() - 1.0) * 0.09;
        let z = (rand() - 0.5) * 0.01;
        let phase = rand() * 6.28;
        for j in 0..POINTS {
            let t = j as f32 / (POINTS - 1) as f32;
            let y = 0.1 - 0.2 * t;
            let wave = (t * 6.0 + phase).sin() * 0.004 * t;
            g.points.push([x + wave, y, z, 4e-5 * (1.0 - 0.6 * t)]);
        }
        g.roots.push([0.55, 0.42, 0.25, rand()]);
    }
    g
}

fn render(hair: bool, shot: &str) -> Option<RgbImage> {
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
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.sharpness = 0.0;
        s.screen_space_reflections = false;
        s.screen_space_gi = false;
    }
    let groom = app
        .world
        .resource_mut::<Assets<StrandGroom>>()
        .add(curtain());
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 0.0, 0.5).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.5, -1.0), Vec3::Y),
    ));
    // A dark wall behind, and a box in front of the hair's right part.
    w.spawn((
        Transform::from_xyz(0.0, 0.0, -0.3),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [2.0, 2.0, 0.02],
            },
            StandardMaterial::color(0.05, 0.06, 0.08),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.07, -0.02, 0.08),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.05, 0.05, 0.05],
            },
            StandardMaterial::color(0.2, 0.5, 0.2),
        ),
    ));
    if hair {
        w.spawn((Transform::default(), StrandInstance::new(groom)));
    }
    app.update_n(6);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    Some(image::open(&path).expect("read the shot").to_rgb8())
}

/// How far a pixel moved from the plain scene toward the hair's color, 0..1 (over the color
/// channels' distance between the wall and the dense hair).
fn cover(plain: &RgbImage, hair: &RgbImage, x: u32, y: u32, full: [f32; 3]) -> f32 {
    let p = plain.get_pixel(x, y).0;
    let h = hair.get_pixel(x, y).0;
    let mut num = 0.0;
    let mut den = 0.0;
    for c in 0..3 {
        let d = full[c] - p[c] as f32;
        num += (h[c] as f32 - p[c] as f32) * d;
        den += d * d;
    }
    if den < 1.0 { 0.0 } else { num / den }
}

#[test]
fn hair_covers_softly_and_hides_behind_solid_things() {
    let Some(plain) = render(false, "strands_plain.png") else {
        return;
    };
    let Some(hair) = render(true, "strands_curtain.png") else {
        return;
    };
    // 0.5 m away, 40 degrees: pixels per meter at the curtain.
    let px_per_m = (H as f32 / 2.0) / (20f32.to_radians().tan() * 0.5);
    let col = |x: f32| (W as f32 / 2.0 + x * px_per_m).round() as u32;
    let row = |y: f32| (H as f32 / 2.0 - y * px_per_m).round() as u32;
    // The dense middle: the hair's own color.
    let mid = hair.get_pixel(col(-0.01), row(0.0)).0;
    let wall = plain.get_pixel(col(-0.01), row(0.0)).0;
    eprintln!("middle: hair {mid:?}, wall {wall:?}");
    assert!(
        mid.iter()
            .zip(&wall)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .sum::<i32>()
            > 40,
        "the hair shows where it is dense"
    );
    assert!(mid[0] > mid[2], "fair hair is warm: {mid:?}");
    let full = [mid[0] as f32, mid[1] as f32, mid[2] as f32];
    // Across the left half, out to its sparse side: partly covered pixels, not only all or
    // nothing.
    let y = row(0.03);
    let (mut partial, mut covered, mut n) = (0, 0, 0);
    for x in col(-0.1)..col(-0.01) {
        let c = cover(&plain, &hair, x, y, full);
        n += 1;
        if c > 0.1 && c < 0.75 {
            partial += 1;
        }
        if c >= 0.75 {
            covered += 1;
        }
    }
    eprintln!("left half: {partial} partly covered, {covered} covered of {n}");
    assert!(
        partial >= n / 5,
        "soft cover toward the sides: {partial} of {n}"
    );
    assert!(covered >= 3, "dense in the middle: {covered} of {n}");
    // Behind the box: nothing changes.
    for (x, y) in [(0.06, -0.02), (0.08, -0.01), (0.07, -0.03)] {
        // The box at z = 0.08 is nearer: it covers more of the view than its size at 0.5 m.
        let k = 0.5 / (0.5 - 0.08);
        let (cx, cy) = (col(x * k), row(y * k));
        let p = plain.get_pixel(cx, cy).0;
        let h = hair.get_pixel(cx, cy).0;
        let diff = p
            .iter()
            .zip(&h)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max();
        assert!(
            diff <= Some(2),
            "the hair shows through the box at ({cx}, {cy}): {p:?} -> {h:?}"
        );
    }
}
