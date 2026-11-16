//! The strand renderer (characters week R2): a curtain of fair hair half a meter from the
//! camera, thinning toward its sides, with a box in front of its right part. The hair shows
//! where it is dense, fades through partly covered pixels toward its sides (real coverage, not
//! all-or-nothing pixels), and nothing of it shows through the box; seen from 2 and 5 m it
//! covers the same share of itself (fewer, wider strands). Leaving out the strands the box
//! hides changes no pixel, and a dense pelt far away, which draws as many strands as its
//! pixels can show, covers what it did with twelve times as many. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::shade_skip;
use engine_render::{
    AmbientLight, RenderDisabled, Renderer, StrandFiber, StrandGroom, StrandInstance, capture_now,
};
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
        let phase = rand() * std::f32::consts::TAU;
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
    render_at(hair, 0.5, shot)
}

/// The scene with the camera `distance` meters from the curtain.
fn render_at(hair: bool, distance: f64, shot: &str) -> Option<RgbImage> {
    render_with(hair, distance, 0, shot)
}

/// The same with `RenderSettings::debug_skip` set.
fn render_with(hair: bool, distance: f64, debug_skip: u32, shot: &str) -> Option<RgbImage> {
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
        s.debug_skip = debug_skip;
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
        Transform::from_xyz(0.0, 0.0, distance).looking_at(DVec3::ZERO, Vec3::Y),
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

/// Mean cover of the curtain's left part (x -0.09..0.04 m, y -0.1..0.1 m) seen from `distance`.
fn mean_cover(plain: &RgbImage, hair: &RgbImage, distance: f32, full: [f32; 3]) -> f32 {
    let px_per_m = (H as f32 / 2.0) / (20f32.to_radians().tan() * distance);
    let col = |x: f32| (W as f32 / 2.0 + x * px_per_m).round() as u32;
    let row = |y: f32| (H as f32 / 2.0 - y * px_per_m).round() as u32;
    let (mut sum, mut n) = (0.0, 0);
    for y in row(0.1)..row(-0.1) {
        for x in col(-0.09)..col(0.04) {
            sum += cover(plain, hair, x, y, full).clamp(0.0, 1.0);
            n += 1;
        }
    }
    sum / n.max(1) as f32
}

#[test]
fn far_hair_keeps_its_cover() {
    // Shot names of their own: the other test writes its shots at the same time.
    let Some(plain) = render_at(false, 0.5, "strands_far_test_plain.png") else {
        return;
    };
    let Some(near) = render_at(true, 0.5, "strands_far_test_near.png") else {
        return;
    };
    let px_per_m = (H as f32 / 2.0) / (20f32.to_radians().tan() * 0.5);
    let mid = near
        .get_pixel(
            (W as f32 / 2.0 - 0.01 * px_per_m).round() as u32,
            (H as f32 / 2.0).round() as u32,
        )
        .0;
    let full = [mid[0] as f32, mid[1] as f32, mid[2] as f32];
    let close = mean_cover(&plain, &near, 0.5, full);
    for distance in [2.0f32, 5.0] {
        let Some(far_plain) = render_at(
            false,
            distance as f64,
            &format!("strands_far_plain_{distance}.png"),
        ) else {
            return;
        };
        let Some(far) = render_at(
            true,
            distance as f64,
            &format!("strands_far_{distance}.png"),
        ) else {
            return;
        };
        let c = mean_cover(&far_plain, &far, distance, full);
        eprintln!("mean cover: {close:.3} at 0.5 m, {c:.3} at {distance} m");
        // Fewer strands far away, each wider by as much: the same share of the curtain.
        assert!(
            (c - close).abs() < 0.12,
            "far hair keeps its cover: {close:.3} at 0.5 m, {c:.3} at {distance} m"
        );
    }
}

#[test]
fn hidden_strands_are_left_out_without_a_trace() {
    // The curtain with the box in front of its right part, drawn as it is now (the strands
    // behind the box are neither lit nor sorted into tiles) and the old way (all of them are).
    // This close the two draw the same strands, so the pictures must be the same.
    let Some(now) = render_with(true, 0.5, 0, "strands_hidden_now.png") else {
        return;
    };
    let Some(before) = render_with(
        true,
        0.5,
        shade_skip::STRANDS_OLD_WAY,
        "strands_hidden_old_way.png",
    ) else {
        return;
    };
    let (mut worst, mut differ) = (0, 0);
    for (a, b) in now.pixels().zip(before.pixels()) {
        let d = (0..3)
            .map(|c| (a.0[c] as i32 - b.0[c] as i32).abs())
            .max()
            .unwrap_or(0);
        worst = worst.max(d);
        differ += (d > 2) as u32;
    }
    eprintln!(
        "lit and sorted or left out: {differ} pixels differ by more than 2/255, the worst by {worst}"
    );
    assert_eq!(
        differ, 0,
        "leaving out hidden strands changed the picture (worst {worst}/255)"
    );
}

/// A patch of short fine fur, 20 cm square in the plane z = 0 and facing the camera: `strands`
/// hairs 2 cm long, combed downward and a little outward.
fn pelt(strands: usize) -> StrandGroom {
    const PER: u32 = 4;
    let mut g = StrandGroom {
        points_per: PER,
        ..Default::default()
    };
    let mut seed = 977u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for _ in 0..strands {
        let root = Vec3::new((rand() - 0.5) * 0.2, (rand() - 0.5) * 0.2, 0.0);
        let dir = Vec3::new((rand() - 0.5) * 0.4, -1.0, 0.3 + 0.4 * rand()).normalize();
        for j in 0..PER {
            let t = j as f32 / (PER - 1) as f32;
            let p = root + dir * (0.02 * t);
            g.points.push([p.x, p.y, p.z, 2e-5 * (1.0 - 0.5 * t)]);
        }
        g.roots.push([0.6, 0.45, 0.25, rand()]);
    }
    g
}

/// A pelt of `hairs` hairs (0: none) 5 m away over a dark wall under a bright even light: the
/// picture and what the strand renderer says it drew. Its hairs are flat-colored (no light of
/// their own, so only what they cover shows) unless `lit`, which also adds a sun from the side.
fn render_pelt(
    hairs: usize,
    lit: bool,
    debug_skip: u32,
    shot: &str,
) -> Option<(RgbImage, serde_json::Value)> {
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
        s.debug_skip = debug_skip | if lit { 0 } else { shade_skip::STRAND_LIGHT };
    }
    app.world.insert_resource(AmbientLight {
        color: [1.0, 1.0, 1.0],
        brightness: 6000.0,
    });
    let groom = (hairs > 0).then(|| {
        app.world
            .resource_mut::<Assets<StrandGroom>>()
            .add(pelt(hairs))
    });
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.0, -0.3),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [4.0, 4.0, 0.02],
            },
            StandardMaterial::color(0.02, 0.02, 0.02),
        ),
    ));
    if lit {
        w.spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(DVec3::new(0.8, -0.5, -0.6), Vec3::Y),
        ));
    }
    if let Some(groom) = groom {
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
    let describe = app.world.resource::<Renderer>().describe();
    let state = describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "strands"))
        .map(|f| f["state"].clone())
        .unwrap_or_default();
    Some((image::open(&path).expect("read the shot").to_rgb8(), state))
}

#[test]
fn a_dense_pelt_far_away_draws_by_its_pixels_and_covers_the_same() {
    const HAIRS: usize = 60_000;
    let Some((plain, _)) = render_pelt(0, false, 0, "strands_pelt_plain.png") else {
        return;
    };
    // About twenty pixels tall: its pixels set the number of strands drawn...
    let Some((now, drew)) = render_pelt(HAIRS, false, 0, "strands_pelt_now.png") else {
        return;
    };
    // ...where the old way drew a sixteenth of the groom, each strand sixteen times as wide.
    let Some((before, drew_before)) = render_pelt(
        HAIRS,
        false,
        shade_skip::STRANDS_OLD_WAY,
        "strands_pelt_old_way.png",
    ) else {
        return;
    };
    eprintln!("drawn now: {drew}; the old way: {drew_before}");
    assert_eq!(drew["strands_in_grooms"], HAIRS as u64, "{drew}");
    let strands = drew["strands"].as_u64().unwrap_or(0);
    assert!(
        (800..=2400).contains(&strands),
        "the pelt draws by its pixels (some 1 600 strands, not 3 750): {drew}"
    );
    assert_eq!(drew["points"], strands * 4, "{drew}");
    assert_eq!(drew_before["strands"], HAIRS as u64 / 16, "{drew_before}");
    // What each adds to the wall over the pelt and a margin around it (wider strands spread a
    // little further): the sum of every channel's change.
    let px_per_m = (H as f32 / 2.0) / (20f32.to_radians().tan() * 5.0);
    let col = |x: f32| (W as f32 / 2.0 + x * px_per_m).round() as u32;
    let row = |y: f32| (H as f32 / 2.0 - y * px_per_m).round() as u32;
    let added = |hair: &RgbImage| -> f32 {
        let mut sum = 0.0;
        for y in row(0.16)..=row(-0.18) {
            for x in col(-0.16)..=col(0.16) {
                let (h, p) = (hair.get_pixel(x, y).0, plain.get_pixel(x, y).0);
                sum += (0..3).map(|c| h[c] as f32 - p[c] as f32).sum::<f32>();
            }
        }
        sum
    };
    let (a, b) = (added(&now), added(&before));
    let mid = before.get_pixel(col(0.0), row(0.0)).0;
    let wall = plain.get_pixel(col(0.0), row(0.0)).0;
    eprintln!(
        "added to the wall: {a:.0} by its pixels, {b:.0} the old way; middle {mid:?} over {wall:?}"
    );
    assert!(
        b > 3000.0 && mid[0] as i32 - wall[0] as i32 > 30,
        "the pelt shows on the wall: {b:.0}, {mid:?} over {wall:?}"
    );
    assert!(
        (a - b).abs() < 0.2 * b,
        "a pelt drawn by its pixels covers what it did the old way: {a:.0} against {b:.0}"
    );
}

/// The mean and the standard deviation of the pelt's inner pixels (sum of the channels).
fn pelt_inside(shot: &RgbImage) -> (f32, f32) {
    let px_per_m = (H as f32 / 2.0) / (20f32.to_radians().tan() * 5.0);
    let col = |x: f32| (W as f32 / 2.0 + x * px_per_m).round() as u32;
    let row = |y: f32| (H as f32 / 2.0 - y * px_per_m).round() as u32;
    let mut values = Vec::new();
    for y in row(0.06)..=row(-0.08) {
        for x in col(-0.07)..=col(0.07) {
            let p = shot.get_pixel(x, y).0;
            values.push(p[0] as f32 + p[1] as f32 + p[2] as f32);
        }
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32;
    (mean, var.sqrt())
}

#[test]
fn a_far_pelt_shades_itself_as_it_did_with_every_sixteenth_strand() {
    // The density volume that the hairs' self-shadowing reads is filled from a sixteenth of
    // the groom even where a few hundred strands are drawn: under the sun and the sky the far
    // pelt is as bright as the old way's, not lighter and speckled (which it was while only
    // the drawn strands, each a hundred hairs wide, filled the volume).
    const HAIRS: usize = 60_000;
    let Some((now, drew)) = render_pelt(HAIRS, true, 0, "strands_pelt_lit_now.png") else {
        return;
    };
    let Some((before, _)) = render_pelt(
        HAIRS,
        true,
        shade_skip::STRANDS_OLD_WAY,
        "strands_pelt_lit_old_way.png",
    ) else {
        return;
    };
    let Some((flat, _)) = render_pelt(HAIRS, false, 0, "strands_pelt_flat_now.png") else {
        return;
    };
    let (a, a_dev) = pelt_inside(&now);
    let (b, b_dev) = pelt_inside(&before);
    let (f, _) = pelt_inside(&flat);
    eprintln!(
        "lit pelt inside: {a:.1} (deviation {a_dev:.1}) by its pixels, {b:.1} ({b_dev:.1}) the old way; unlit {f:.1}; {drew}"
    );
    assert!(
        (b - f).abs() > 0.05 * f,
        "the light changes the pelt, so the comparison means something: {b:.1} lit, {f:.1} flat"
    );
    assert!(
        (a - b).abs() < 0.05 * b,
        "a far pelt is as bright as it was the old way: {a:.1} against {b:.1}"
    );
}

/// A coat like the game's deer wears: `strands` hairs 2.2 cm long and 0.31 mm thick at the root
/// over a body 1.2 m long and half a meter through, combed backward and a little outward.
fn coat(strands: usize) -> StrandGroom {
    const PER: u32 = 4;
    let mut g = StrandGroom {
        fiber: StrandFiber::Fur,
        points_per: PER,
        ..Default::default()
    };
    let mut seed = 4099u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let radii = Vec3::new(0.6, 0.25, 0.25);
    for _ in 0..strands {
        let u = rand() * 2.0 - 1.0;
        let phi = rand() * std::f32::consts::TAU;
        let s = (1.0 - u * u).sqrt();
        let n = Vec3::new(u, s * phi.cos(), s * phi.sin());
        let root = n * radii;
        let normal = (n / radii).normalize();
        let back = (Vec3::NEG_X - normal * Vec3::NEG_X.dot(normal)).normalize_or_zero();
        let stray = Vec3::new(rand() - 0.5, rand() - 0.5, rand() - 0.5) * 0.3;
        let dir = (back * 0.85 + normal * 0.5 + stray).normalize();
        for j in 0..PER {
            let t = j as f32 / (PER - 1) as f32;
            let p = root + dir * (0.022 * t);
            g.points.push([p.x, p.y, p.z, 1.55e-4 * (1.0 - 0.6 * t)]);
        }
        let shade = 0.8 + 0.4 * rand();
        g.roots
            .push([0.42 * shade, 0.27 * shade, 0.15 * shade, rand()]);
    }
    g
}

const COAT_W: u32 = 640;
const COAT_H: u32 = 480;

/// The coat on its body from the side, `distance` m away, under a sun and the sky: the picture
/// and what the strand renderer says it drew.
fn render_coat(
    distance: f64,
    debug_skip: u32,
    shot: &str,
) -> Option<(RgbImage, serde_json::Value)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = COAT_W;
        w.height = COAT_H;
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
        s.debug_skip = debug_skip;
    }
    let groom = app
        .world
        .resource_mut::<Assets<StrandGroom>>()
        .add(coat(200_000));
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 0.0, distance).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.6, -1.0), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.0, -1.5),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [40.0, 40.0, 0.02],
            },
            StandardMaterial::color(0.05, 0.06, 0.08),
        ),
    ));
    // The body under the coat, a hair's breadth inside the roots.
    w.spawn((
        Transform::default().with_scale(Vec3::new(1.196, 0.496, 0.496)),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.5,
                segments: 48,
            },
            StandardMaterial::color(0.2, 0.13, 0.08),
        ),
    ));
    w.spawn((Transform::default(), StrandInstance::new(groom)));
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
    let describe = app.world.resource::<Renderer>().describe();
    let state = describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "strands"))
        .map(|f| f["state"].clone())
        .unwrap_or_default();
    Some((image::open(&path).expect("read the shot").to_rgb8(), state))
}

/// The coat's flank in a picture taken from `distance` m: its mean (sum of the channels) and
/// its grain, the root mean square of what each pixel differs from the mean of the 3 x 3
/// around it.
fn coat_flank(shot: &RgbImage, distance: f64) -> (f32, f32) {
    let px_per_m = (COAT_H as f32 / 2.0) / (20f32.to_radians().tan() * distance as f32);
    let col = |x: f32| (COAT_W as f32 / 2.0 + x * px_per_m).round() as i32;
    let row = |y: f32| (COAT_H as f32 / 2.0 - y * px_per_m).round() as i32;
    let value = |x: i32, y: i32| {
        let p = shot.get_pixel(x as u32, y as u32).0;
        p[0] as f32 + p[1] as f32 + p[2] as f32
    };
    let (mut sum, mut grain, mut n) = (0.0, 0.0, 0.0);
    for y in row(0.12)..=row(-0.12) {
        for x in col(-0.35)..=col(0.35) {
            let mut around = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    around += value(x + dx, y + dy);
                }
            }
            let v = value(x, y);
            sum += v;
            grain += (v - around / 9.0) * (v - around / 9.0);
            n += 1.0;
        }
    }
    (sum / n, (grain / n).sqrt())
}

#[test]
fn a_coat_far_away_is_as_smooth_as_with_the_near_rule() {
    // A deer's coat thirty pixels tall draws 3 600 of its 200 000 hairs by its pixels where
    // the near rule drew 12 500: the same cover and light from under a third of the strands,
    // each as much wider. It must not look coarser for it: with one strand a pixel instead of
    // four (`STRANDS_SPARSE`) it does, twice as grainy with the skin showing between them.
    // (With `ENGINE_SHOTS` set, also at 20, 50 and 100 pixels and the sparse way, to look at.)
    let look = std::env::var("ENGINE_SHOTS").is_ok();
    let mut cases = vec![(32.0, "30")];
    if look {
        cases.extend([(48.2, "20"), (19.3, "50"), (9.6, "100")]);
    }
    for (distance, name) in cases {
        let Some((now, drew)) = render_coat(distance, 0, &format!("strands_coat_{name}_now.png"))
        else {
            return;
        };
        let Some((before, drew_before)) = render_coat(
            distance,
            shade_skip::STRANDS_OLD_WAY,
            &format!("strands_coat_{name}_old_way.png"),
        ) else {
            return;
        };
        let (a, a_grain) = coat_flank(&now, distance);
        let (b, b_grain) = coat_flank(&before, distance);
        eprintln!(
            "coat at {name} px: flank {a:.1} (grain {a_grain:.2}) drawing {}, the old way {b:.1} ({b_grain:.2}) drawing {}",
            drew["strands"], drew_before["strands"]
        );
        if look
            && let Some((sparse, drew_sparse)) = render_coat(
                distance,
                shade_skip::STRANDS_SPARSE,
                &format!("strands_coat_{name}_sparse.png"),
            )
        {
            let (c, c_grain) = coat_flank(&sparse, distance);
            eprintln!(
                "    sparse: flank {c:.1} (grain {c_grain:.2}) drawing {}",
                drew_sparse["strands"]
            );
        }
        assert!(
            drew["strands"].as_u64() < drew_before["strands"].as_u64(),
            "the coat draws fewer strands by its pixels: {drew} against {drew_before}"
        );
        assert!(
            (a - b).abs() < 0.06 * b,
            "the coat at {name} px is as bright as the old way's: {a:.1} against {b:.1}"
        );
        assert!(
            a_grain < 1.5 * b_grain + 1.0,
            "the coat at {name} px is as smooth as the old way's: grain {a_grain:.2} against {b_grain:.2}"
        );
    }
}
