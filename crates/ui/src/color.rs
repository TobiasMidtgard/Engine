//! [`Color`]: linear RGBA with straight alpha, written as sRGB hex in data (`"#e8b65a"`).

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A color in linear RGB with straight (not premultiplied) alpha.
///
/// Data files and the AI bridge write colors as sRGB hex strings (`"#rgb"`, `"#rrggbb"`,
/// `"#rrggbbaa"`), as `[r, g, b]` / `[r, g, b, a]` arrays of sRGB values in 0..1, or as a
/// CSS-style name (`"white"`, `"transparent"`). Blending happens in linear space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

impl Color {
    pub const TRANSPARENT: Color = Color::linear(0.0, 0.0, 0.0, 0.0);
    pub const WHITE: Color = Color::linear(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::linear(0.0, 0.0, 0.0, 1.0);

    /// From linear components.
    pub const fn linear(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From sRGB components in 0..1.
    pub fn srgb(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::linear(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a)
    }

    /// From a `0xRRGGBB` sRGB value, opaque.
    pub fn hex(rgb: u32) -> Self {
        Self::srgb8(
            (rgb >> 16) as u8,
            (rgb >> 8) as u8,
            rgb as u8,
            255,
        )
    }

    /// From a `0xRRGGBBAA` sRGB value.
    pub fn hexa(rgba: u32) -> Self {
        Self::srgb8(
            (rgba >> 24) as u8,
            (rgba >> 16) as u8,
            (rgba >> 8) as u8,
            rgba as u8,
        )
    }

    /// From 8-bit sRGB components.
    pub fn srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::srgb(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        )
    }

    /// Same color with alpha `a`.
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Alpha multiplied by `f`.
    pub fn fade(self, f: f32) -> Self {
        Self {
            a: self.a * f,
            ..self
        }
    }

    /// Linear interpolation (in linear space).
    pub fn lerp(self, other: Color, t: f32) -> Self {
        Self::linear(
            self.r + (other.r - self.r) * t,
            self.g + (other.g - self.g) * t,
            self.b + (other.b - self.b) * t,
            self.a + (other.a - self.a) * t,
        )
    }

    /// Brighter (`f > 1`) or darker (`f < 1`), alpha unchanged.
    pub fn scale(self, f: f32) -> Self {
        Self::linear(self.r * f, self.g * f, self.b * f, self.a)
    }

    /// Linear RGBA array (what shaders get).
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// 8-bit sRGB components.
    pub fn to_srgb8(self) -> [u8; 4] {
        let q = |c: f32| (linear_to_srgb(c) * 255.0).round() as u8;
        [
            q(self.r),
            q(self.g),
            q(self.b),
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    /// `#rrggbb` or `#rrggbbaa` (sRGB).
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_srgb8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    /// Parse `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` or a name.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).map(|v| v * 17);
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16);
            let msg = || format!("\"{s}\" is not a hex color. Use #rgb, #rrggbb or #rrggbbaa.");
            if !hex.is_ascii() {
                return Err(msg());
            }
            let bad = |_: std::num::ParseIntError| msg();
            return match hex.len() {
                3 => Ok(Self::srgb8(
                    digit(0).map_err(bad)?,
                    digit(1).map_err(bad)?,
                    digit(2).map_err(bad)?,
                    255,
                )),
                4 => Ok(Self::srgb8(
                    digit(0).map_err(bad)?,
                    digit(1).map_err(bad)?,
                    digit(2).map_err(bad)?,
                    digit(3).map_err(bad)?,
                )),
                6 => Ok(Self::srgb8(
                    byte(0).map_err(bad)?,
                    byte(2).map_err(bad)?,
                    byte(4).map_err(bad)?,
                    255,
                )),
                8 => Ok(Self::srgb8(
                    byte(0).map_err(bad)?,
                    byte(2).map_err(bad)?,
                    byte(4).map_err(bad)?,
                    byte(6).map_err(bad)?,
                )),
                _ => Err(msg()),
            };
        }
        Ok(match s.to_ascii_lowercase().as_str() {
            "transparent" | "none" => Self::TRANSPARENT,
            "white" => Self::WHITE,
            "black" => Self::BLACK,
            "red" => Self::hex(0xe5484d),
            "green" => Self::hex(0x46a758),
            "blue" => Self::hex(0x3e63dd),
            "yellow" => Self::hex(0xf5d90a),
            "orange" => Self::hex(0xf76808),
            "purple" => Self::hex(0x8e4ec6),
            "cyan" => Self::hex(0x5fd0ff),
            "gold" => Self::hex(0xe8b65a),
            "gray" | "grey" => Self::hex(0x8b8d98),
            _ => {
                return Err(format!(
                    "Unknown color \"{s}\". Use a hex color like \"#e8b65a\" or a name (white, black, red, green, blue, yellow, orange, purple, cyan, gold, gray, transparent)."
                ));
            }
        })
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Color;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a color: \"#rrggbb\", \"#rrggbbaa\", a name, or [r, g, b, a] in 0..1 sRGB")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Color, E> {
                Color::parse(v).map_err(E::custom)
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Color, A::Error> {
                let mut c = [0.0f32, 0.0, 0.0, 1.0];
                let mut n = 0;
                while let Some(v) = seq.next_element::<f32>()? {
                    if n < 4 {
                        c[n] = v;
                    }
                    n += 1;
                }
                if n < 3 {
                    return Err(de::Error::custom("a color array needs 3 or 4 numbers"));
                }
                Ok(Color::srgb(c[0], c[1], c[2], c[3]))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for s in ["#e8b65a", "#00000080", "#ffffff", "#123456"] {
            assert_eq!(Color::parse(s).unwrap().to_hex(), s);
        }
        assert_eq!(Color::parse("#fff").unwrap().to_hex(), "#ffffff");
        assert!(Color::parse("#12").is_err());
        assert!(Color::parse("chartreuse-ish").is_err());
    }

    #[test]
    fn serde_accepts_hex_names_and_arrays() {
        let c: Color = serde_json::from_str("\"gold\"").unwrap();
        assert_eq!(c.to_hex(), "#e8b65a");
        let c: Color = serde_json::from_str("[1, 0, 0, 0.5]").unwrap();
        assert_eq!(c.to_hex(), "#ff000080");
        assert_eq!(serde_json::to_string(&Color::WHITE).unwrap(), "\"#ffffff\"");
    }

    #[test]
    fn srgb_mid_gray_is_dark_in_linear() {
        let c = Color::hex(0x808080);
        assert!((c.r - 0.2158).abs() < 0.001);
    }
}
