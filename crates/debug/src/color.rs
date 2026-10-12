//! Debug colors: linear RGBA `[f32; 4]` with straight (not premultiplied) alpha.
//!
//! Debug lines are written to the screen as display values, not as physical radiance, so
//! `[1.0, 0.0, 0.0, 1.0]` is a clearly visible red at noon, at night and in a cave. Values above
//! 1 make a line brighter (and slightly bloom); alpha below 1 blends it with the scene.

/// Linear RGBA color, components normally in `0..=1`.
pub type Color = [f32; 4];

/// Opaque red (`"red"` in the API).
pub const RED: Color = [1.0, 0.05, 0.05, 1.0];
/// Opaque green (`"green"`).
pub const GREEN: Color = [0.1, 1.0, 0.1, 1.0];
/// Opaque blue (`"blue"`).
pub const BLUE: Color = [0.1, 0.25, 1.0, 1.0];
/// Opaque yellow (`"yellow"`).
pub const YELLOW: Color = [1.0, 0.9, 0.05, 1.0];
/// Opaque cyan (`"cyan"`).
pub const CYAN: Color = [0.05, 0.9, 1.0, 1.0];
/// Opaque magenta (`"magenta"`).
pub const MAGENTA: Color = [1.0, 0.05, 1.0, 1.0];
/// Opaque orange (`"orange"`).
pub const ORANGE: Color = [1.0, 0.35, 0.02, 1.0];
/// Opaque purple (`"purple"`).
pub const PURPLE: Color = [0.45, 0.1, 1.0, 1.0];
/// Opaque pink (`"pink"`).
pub const PINK: Color = [1.0, 0.3, 0.6, 1.0];
/// Opaque lime (`"lime"`).
pub const LIME: Color = [0.55, 1.0, 0.05, 1.0];
/// Opaque white (`"white"`).
pub const WHITE: Color = [1.0, 1.0, 1.0, 1.0];
/// Opaque mid gray (`"gray"` / `"grey"`).
pub const GRAY: Color = [0.35, 0.35, 0.35, 1.0];
/// Opaque black (`"black"`).
pub const BLACK: Color = [0.0, 0.0, 0.0, 1.0];

/// Color of the X axis in [`crate::DebugDraw::axes`].
pub const AXIS_X: Color = RED;
/// Color of the Y axis in [`crate::DebugDraw::axes`].
pub const AXIS_Y: Color = GREEN;
/// Color of the Z axis in [`crate::DebugDraw::axes`].
pub const AXIS_Z: Color = BLUE;

/// A color from linear components.
pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    [r, g, b, a]
}

/// A color from an sRGB hex value such as `0xff8800` (converted to linear, alpha 1).
pub fn hex(rgb: u32) -> Color {
    let c = |shift: u32| srgb_to_linear(((rgb >> shift) & 0xff) as f32 / 255.0);
    [c(16), c(8), c(0), 1.0]
}

/// The same color with a different alpha.
pub const fn with_alpha(c: Color, alpha: f32) -> Color {
    [c[0], c[1], c[2], alpha]
}

/// Linear blend between two colors (`t` = 0 gives `a`, 1 gives `b`).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    // This form is exact at both ends (t = 0 gives `a`, t = 1 gives `b`).
    std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}

/// A blue → cyan → green → yellow → red ramp for values in `0..=1` (calm to extreme).
pub fn ramp(t: f32) -> Color {
    const STOPS: [Color; 5] = [BLUE, CYAN, GREEN, YELLOW, RED];
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let x = t * (STOPS.len() - 1) as f32;
    let i = (x.floor() as usize).min(STOPS.len() - 2);
    mix(STOPS[i], STOPS[i + 1], x - i as f32)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Look up a named color (`"red"`, `"green"`, …, case-insensitive). Used by the JSON API.
pub fn named(name: &str) -> Option<Color> {
    Some(match name.to_ascii_lowercase().as_str() {
        "red" => RED,
        "green" => GREEN,
        "blue" => BLUE,
        "yellow" => YELLOW,
        "cyan" => CYAN,
        "magenta" => MAGENTA,
        "orange" => ORANGE,
        "purple" => PURPLE,
        "pink" => PINK,
        "lime" => LIME,
        "white" => WHITE,
        "gray" | "grey" => GRAY,
        "black" => BLACK,
        _ => return None,
    })
}

/// Names accepted by [`named`].
pub const NAMES: [&str; 13] = [
    "red", "green", "blue", "yellow", "cyan", "magenta", "orange", "purple", "pink", "lime",
    "white", "gray", "black",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_converts_to_linear() {
        let c = hex(0xff8000);
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 0.2158).abs() < 1e-3, "{c:?}");
        assert_eq!(c[2], 0.0);
        assert_eq!(c[3], 1.0);
    }

    #[test]
    fn ramp_ends() {
        assert_eq!(ramp(0.0), BLUE);
        assert_eq!(ramp(1.0), RED);
        assert_eq!(ramp(f32::NAN), RED);
        assert_eq!(ramp(0.5), GREEN);
    }

    #[test]
    fn every_name_resolves() {
        for n in NAMES {
            assert!(named(n).is_some(), "{n}");
        }
        assert_eq!(named("Grey"), Some(GRAY));
        assert_eq!(named("chartreuse"), None);
    }
}
