//! Tiling and part textures shared by both cars: carbon twill, tire sidewalls and tread,
//! brake discs.

use engine_core::math::Vec2;
use engine_render::Image;

use super::sdf::sstep;
use super::tex::{self, Canvas};
use super::wheel::{self, Tread, WheelSpec};

/// 2×2 twill carbon weave, one tile = 8 tows each way (color, normal). 256².
pub fn carbon() -> (Image, Image) {
    let n = 256u32;
    let tow = n as f32 / 8.0;
    // Which way the top fiber runs in cell (i, j), and the bump across it.
    let cell = |x: u32, y: u32| -> (bool, f32, f32) {
        let (fx, fy) = (x as f32 / tow, y as f32 / tow);
        let (i, j) = (fx as i32, fy as i32);
        let horizontal = ((i + j).rem_euclid(4)) < 2;
        let (u, v) = (fx.fract(), fy.fract());
        // Fiber bundle cross-section: a dome across the tow, flattened along it.
        let across = if horizontal { v } else { u };
        let along = if horizontal { u } else { v };
        let dome = (1.0 - (2.0 * across - 1.0).powi(2)).max(0.0).sqrt();
        let ends = sstep(0.0, 0.12, along) * sstep(1.0, 0.88, along);
        (horizontal, dome * (0.55 + 0.45 * ends), across)
    };
    let height = tex::par_buffer(n, n, |x, y| cell(x, y).1 * 2.0);
    let normal = tex::normal_map(n, n, &height, 0.9, true);
    let color = tex::par_image(n, n, true, |x, y| {
        let (h, dome, across) = cell(x, y);
        // Fine fiber streaks along each tow.
        let streak = tex::hash(if h { y as i32 } else { x as i32 }, 0, 11) * 0.25;
        let base = if h { 0.055 } else { 0.03 };
        let v = base * (0.6 + 0.4 * dome) * (0.85 + streak) + 0.004 * (1.0 - across);
        let c = tex::srgb(v);
        [c, c, (c as f32 * 1.04).min(255.0) as u8, 255]
    });
    (color, normal)
}

/// Tire sidewall lettering and tread siping (color, normal), laid out for
/// [`wheel::tire`]'s UVs: u around the wheel, v across the profile. 2048 × 512.
pub fn tire(spec: &WheelSpec, brand: &str, size: &str) -> (Image, Image) {
    let (w, h) = (2048u32, 512u32);
    let layout = wheel::tire_layout(spec);
    let mut cv = Canvas::new(w, h);
    // Lettering on the outer sidewall, twice around, reading outward-up.
    let r_text = spec.rim + (spec.radius - spec.rim) * 0.52;
    let v_text = layout.outer_v(r_text);
    let letter = 0.020;
    // Pixel scales: u per meter at that radius, v per meter of profile.
    let ku = w as f32 / (std::f32::consts::TAU * r_text);
    let kv = h as f32 / layout.length;
    let px = (ku + kv) * 0.5;
    let size_px = letter * px;
    for (k, text, scale) in [
        (0, brand, 1.0f32),
        (1, size, 0.6),
        (2, brand, 1.0),
        (3, size, 0.6),
    ] {
        let s = size_px * scale;
        let width = tex::text_width(text, s);
        let center_u = (k as f32 + 0.5) / 4.0 * w as f32;
        // Text runs toward −u (reads left to right seen from outside), glyph up = image up.
        let origin = Vec2::new(center_u + width * 0.5, v_text * h as f32 + s * 0.5);
        for (a, b) in
            tex::text_segments(text, origin, s, Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0))
        {
            cv.segment(a, b, 3.0, 1);
        }
    }
    // Rim protector line and a molded ring near the bead.
    for (r, tag) in [(spec.rim + 0.02, 2u8), (spec.radius - 0.012, 2)] {
        let v = layout.outer_v(r) * h as f32;
        cv.segment(Vec2::new(0.0, v), Vec2::new(w as f32, v), 3.0, tag);
    }
    // Tread: slanted shoulder grooves and center sipes.
    let (t0, t1) = layout.tread_v;
    if spec.tread == Tread::Performance {
        let pitches = 72;
        for i in 0..pitches {
            let u = (i as f32 + 0.5) / pitches as f32 * w as f32;
            let du = w as f32 / pitches as f32;
            for (va, vb, slant) in [(0.0f32, 0.22f32, 0.35f32), (0.78, 1.0, -0.35)] {
                let a = Vec2::new(u, (t0 + (t1 - t0) * va) * h as f32);
                let b = Vec2::new(u + slant * du, (t0 + (t1 - t0) * vb) * h as f32);
                cv.segment(a, b, 3.0, 3);
            }
            for vc in [0.42f32, 0.58] {
                let a = Vec2::new(u + 0.2 * du, (t0 + (t1 - t0) * (vc - 0.04)) * h as f32);
                let b = Vec2::new(u + 0.4 * du, (t0 + (t1 - t0) * (vc + 0.04)) * h as f32);
                cv.segment(a, b, 2.0, 4);
            }
        }
    }
    let height = tex::par_buffer(w, h, |x, y| {
        let i = (y * w + x) as usize;
        let d = cv.dist[i];
        match cv.tag[i] {
            1 => 1.4 * (1.0 - sstep(0.6, 1.9, d)),
            2 => 0.8 * (1.0 - sstep(0.4, 1.6, d)),
            3 => -2.0 * (1.0 - sstep(0.8, 2.6, d)),
            4 => -(1.0 - sstep(0.2, 1.2, d)),
            _ => 0.0,
        }
    });
    let normal = tex::normal_map(w, h, &height, 0.8, true);
    let color = tex::par_image(w, h, true, |x, y| {
        let i = (y * w + x) as usize;
        let d = cv.dist[i];
        let grain = 0.92 + 0.08 * tex::hash(x as i32 >> 2, y as i32 >> 2, 5);
        let v = match cv.tag[i] {
            1 => 1.0 + 0.45 * (1.0 - sstep(0.6, 1.9, d)),
            3 => 1.0 - 0.35 * (1.0 - sstep(0.8, 2.6, d)),
            _ => 1.0,
        } * grain;
        let c = tex::srgb((v * 0.5).min(1.0));
        [c, c, c, 255]
    });
    (color, normal)
}

/// Brake disc face: machining rings, cross-drilled holes (or curved slots), a heat-tinted
/// edge; laid out for [`wheel::disc`]'s UVs. 1024 × 128, sRGB, multiplies the disc color.
pub fn disc(drilled: bool) -> Image {
    let (w, h) = (1024u32, 128u32);
    let face = 0.45 * h as f32;
    let mut cv = Canvas::new(w, h);
    let rows = [0.28f32, 0.5, 0.72];
    let holes = 36;
    for (ri, f) in rows.iter().enumerate() {
        for k in 0..holes {
            let u = (k as f32 + ri as f32 / 3.0) / holes as f32 * w as f32;
            // Outer face (v 0..0.45) and the inner face (mirrored, v 0.55..1).
            for v in [f * face, h as f32 - f * face] {
                if drilled {
                    let p = Vec2::new(u, v);
                    cv.segment(p, p, 6.0, 1);
                } else if ri == 1 {
                    let a = Vec2::new(u - 10.0, v - 0.30 * face);
                    let b = Vec2::new(u + 10.0, v + 0.30 * face);
                    cv.segment(a, b, 4.0, 2);
                }
            }
        }
    }
    tex::par_image(w, h, true, |x, y| {
        let i = (y * w + x) as usize;
        let d = cv.dist[i];
        let v = y as f32;
        let on_face = v < face || v > h as f32 - face;
        let rings =
            0.9 + 0.1 * tex::hash(0, y as i32, 3) + 0.05 * tex::hash(x as i32 >> 5, y as i32, 4);
        let mut c = if on_face { rings } else { 0.45 };
        if on_face {
            let f = if v < face {
                v / face
            } else {
                (h as f32 - v) / face
            };
            // Bluish-brown heat tint toward the outer edge.
            c *= 1.0 - 0.15 * sstep(0.75, 1.0, f);
            match cv.tag[i] {
                1 => {
                    c *= if d < 3.4 {
                        0.08
                    } else {
                        1.0 - 0.35 * (1.0 - sstep(3.4, 5.0, d))
                    };
                }
                2 => c *= 0.25 + 0.75 * sstep(1.0, 2.5, d),
                _ => {}
            }
        }
        let s = tex::srgb(c);
        [s, s, (s as f32 * 0.98) as u8, 255]
    })
}
