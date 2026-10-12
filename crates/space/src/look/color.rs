//! Small color helpers for the painters (linear RGB as `[f32; 3]`).

pub(crate) type Rgb = [f32; 3];

/// A linear color from an sRGB hex literal, converted at compile time: `srgb!(0xe8d5b5)`.
macro_rules! srgb {
    ($v:expr) => {{
        const C: [f32; 3] = $crate::look::color::hex($v);
        C
    }};
}
pub(crate) use srgb;

/// sRGB hex (0xRRGGBB) to linear RGB. Slow at run time: use [`srgb!`] for literals.
pub(crate) const fn hex(rgb: u32) -> Rgb {
    [
        srgb_to_linear(((rgb >> 16) & 0xff) as f32 / 255.0),
        srgb_to_linear(((rgb >> 8) & 0xff) as f32 / 255.0),
        srgb_to_linear((rgb & 0xff) as f32 / 255.0),
    ]
}

/// The sRGB transfer function (exact), usable in constants.
const fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        // ((s + 0.055) / 1.055)^2.4 without powf (not const): exp2(2.4 * log2(x)).
        pow24((s + 0.055) / 1.055)
    }
}

/// x^2.4 for x in (0, 1], const-evaluable: x^2 · x^0.4 with x^0.4 from a few Newton steps on
/// y^5 = x^2.
const fn pow24(x: f32) -> f32 {
    let x2 = x * x;
    // Solve y^5 = x2 for y = x^0.4, starting from a decent guess.
    let mut y = 0.4 * x + 0.6;
    let mut i = 0;
    while i < 30 {
        let y4 = y * y * y * y;
        y -= (y4 * y - x2) / (5.0 * y4);
        i += 1;
    }
    x2 * y
}

#[inline]
pub(crate) fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
pub(crate) fn scale(a: Rgb, k: f32) -> Rgb {
    [a[0] * k, a[1] * k, a[2] * k]
}

#[inline]
pub(crate) fn mul(a: Rgb, b: Rgb) -> Rgb {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

#[inline]
pub(crate) fn luma(a: Rgb) -> f32 {
    0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2]
}

/// Change saturation around the luminance (1 = unchanged).
#[inline]
pub(crate) fn saturate_by(a: Rgb, k: f32) -> Rgb {
    let l = luma(a);
    [
        (l + (a[0] - l) * k).max(0.0),
        (l + (a[1] - l) * k).max(0.0),
        (l + (a[2] - l) * k).max(0.0),
    ]
}

/// A color ramp: linear interpolation between `(position, color)` stops sorted by position
/// (ascending). Clamps outside.
pub(crate) fn ramp(stops: &[(f32, Rgb)], x: f32) -> Rgb {
    if stops.is_empty() {
        return [0.0; 3];
    }
    if x <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if x <= b.0 {
            let t = (x - a.0) / (b.0 - a.0).max(1e-6);
            return mix(a.1, b.1, t);
        }
    }
    stops[stops.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_matches_the_srgb_curve() {
        let c = hex(0x808080);
        assert!((c[0] - 0.2158).abs() < 1e-3, "{c:?}");
        assert_eq!(hex(0xffffff)[1], 1.0);
        assert_eq!(hex(0)[2], 0.0);
        let r = ramp(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], 0.25);
        assert!((r[0] - 0.25).abs() < 1e-6);
    }
}
