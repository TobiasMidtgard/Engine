//! Expressions: MakeHuman's CC0 expression units (lids, brows, nose, mouth, jaw), blended for
//! the person's ancestry, split into left and right halves where ARKit names a side, and
//! subdivided with the skin. [`FaceShapes`] holds them per skin vertex for the shaders;
//! [`channel_weights`] turns a `FaceControls` (ARKit weights) into channel weights each frame.

use engine_core::math::Vec3;
use engine_world::character::{FaceControls, arkit};

use crate::appearance::Appearance;
use crate::mesh::{SkinMesh, vertex_normals};
use crate::morph;
use crate::pack::HumanPack;

/// Which half of the face a channel moves (the person's own left is the base mesh's +X).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Both,
    Left,
    Right,
}

/// One expression shape: expression units (file stems under `expression/units/<ancestry>/`)
/// with their weights, on one half of the face or both.
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    pub name: &'static str,
    pub units: &'static [(&'static str, f32)],
    pub half: Half,
}

const fn ch(name: &'static str, units: &'static [(&'static str, f32)], half: Half) -> Channel {
    Channel { name, units, half }
}

/// Every expression channel, in shader order (at most [`MAX_CHANNELS`]).
pub const CHANNELS: [Channel; 32] = [
    ch("blink_l", &[("eye-left-closure", 1.0)], Half::Both),
    ch("blink_r", &[("eye-right-closure", 1.0)], Half::Both),
    ch("squint_l", &[("eye-left-slit", 1.0)], Half::Both),
    ch("squint_r", &[("eye-right-slit", 1.0)], Half::Both),
    ch("wide_l", &[("eye-left-opened-up", 1.0)], Half::Both),
    ch("wide_r", &[("eye-right-opened-up", 1.0)], Half::Both),
    ch("brow_down_l", &[("eyebrows-left-down", 1.0)], Half::Both),
    ch("brow_down_r", &[("eyebrows-right-down", 1.0)], Half::Both),
    ch(
        "brow_outer_up_l",
        &[("eyebrows-left-extern-up", 1.0)],
        Half::Both,
    ),
    ch(
        "brow_outer_up_r",
        &[("eyebrows-right-extern-up", 1.0)],
        Half::Both,
    ),
    ch(
        "brow_inner_up",
        &[
            ("eyebrows-left-inner-up", 1.0),
            ("eyebrows-right-inner-up", 1.0),
        ],
        Half::Both,
    ),
    ch(
        "sneer_l",
        &[("nose-left-elevation", 1.0), ("nose-left-dilatation", 0.6)],
        Half::Both,
    ),
    ch(
        "sneer_r",
        &[
            ("nose-right-elevation", 1.0),
            ("nose-right-dilatation", 0.6),
        ],
        Half::Both,
    ),
    ch("jaw_open", &[("mouth-open", 1.0)], Half::Both),
    ch(
        "smile_l",
        &[
            ("mouth-corner-puller", 1.0),
            ("mouth-upward-retraction", 0.45),
            ("mouth-parling", 0.5),
        ],
        Half::Left,
    ),
    ch(
        "smile_r",
        &[
            ("mouth-corner-puller", 1.0),
            ("mouth-upward-retraction", 0.45),
            ("mouth-parling", 0.5),
        ],
        Half::Right,
    ),
    ch("frown_l", &[("mouth-depression", 1.0)], Half::Left),
    ch("frown_r", &[("mouth-depression", 1.0)], Half::Right),
    ch("stretch_l", &[("mouth-retraction", 1.0)], Half::Left),
    ch("stretch_r", &[("mouth-retraction", 1.0)], Half::Right),
    ch("dimple_l", &[("mouth-upward-retraction", 1.0)], Half::Left),
    ch("dimple_r", &[("mouth-upward-retraction", 1.0)], Half::Right),
    ch("press_l", &[("mouth-compression", 1.0)], Half::Left),
    ch("press_r", &[("mouth-compression", 1.0)], Half::Right),
    ch("pucker", &[("mouth-pursing", 1.0)], Half::Both),
    ch("funnel", &[("mouth-protusion", 1.0)], Half::Both),
    ch("shrug_lower", &[("mouth-elevation", 1.0)], Half::Both),
    ch("upper_up_l", &[("mouth-parling", 1.0)], Half::Left),
    ch("upper_up_r", &[("mouth-parling", 1.0)], Half::Right),
    ch(
        "lower_down_l",
        &[("mouth-depression-retraction", 1.0)],
        Half::Left,
    ),
    ch(
        "lower_down_r",
        &[("mouth-depression-retraction", 1.0)],
        Half::Right,
    ),
    ch("platysma", &[("neck-platysma", 1.0)], Half::Both),
];

/// Channels the shaders take (the size of their weight arrays).
pub const MAX_CHANNELS: usize = 64;

fn channel(name: &str) -> usize {
    CHANNELS
        .iter()
        .position(|c| c.name == name)
        .expect("expression channel")
}

/// How each ARKit shape drives the channels: (ARKit index, channel, weight).
fn arkit_map() -> &'static [(usize, usize, f32)] {
    static MAP: std::sync::LazyLock<Vec<(usize, usize, f32)>> =
        std::sync::LazyLock::new(build_arkit_map);
    &MAP
}

fn build_arkit_map() -> Vec<(usize, usize, f32)> {
    use arkit::*;
    let named = |s: &str| {
        engine_world::character::ARKIT
            .iter()
            .position(|n| *n == s)
            .expect("ARKit name")
    };
    let mut m = vec![
        (EYE_BLINK_LEFT, channel("blink_l"), 1.0),
        (EYE_BLINK_RIGHT, channel("blink_r"), 1.0),
        (EYE_SQUINT_LEFT, channel("squint_l"), 1.0),
        (EYE_SQUINT_RIGHT, channel("squint_r"), 1.0),
        (EYE_WIDE_LEFT, channel("wide_l"), 1.0),
        (EYE_WIDE_RIGHT, channel("wide_r"), 1.0),
        (BROW_DOWN_LEFT, channel("brow_down_l"), 1.0),
        (BROW_DOWN_RIGHT, channel("brow_down_r"), 1.0),
        (BROW_INNER_UP, channel("brow_inner_up"), 1.0),
        (NOSE_SNEER_LEFT, channel("sneer_l"), 1.0),
        (NOSE_SNEER_RIGHT, channel("sneer_r"), 1.0),
        (JAW_OPEN, channel("jaw_open"), 1.0),
        (MOUTH_SMILE_LEFT, channel("smile_l"), 1.0),
        (MOUTH_SMILE_RIGHT, channel("smile_r"), 1.0),
        (MOUTH_FROWN_LEFT, channel("frown_l"), 1.0),
        (MOUTH_FROWN_RIGHT, channel("frown_r"), 1.0),
        (MOUTH_STRETCH_LEFT, channel("stretch_l"), 1.0),
        (MOUTH_STRETCH_RIGHT, channel("stretch_r"), 1.0),
        (MOUTH_STRETCH_LEFT, channel("platysma"), 0.25),
        (MOUTH_STRETCH_RIGHT, channel("platysma"), 0.25),
        (MOUTH_PRESS_LEFT, channel("press_l"), 1.0),
        (MOUTH_PRESS_RIGHT, channel("press_r"), 1.0),
        (CHEEK_SQUINT_LEFT, channel("squint_l"), 0.35),
        (CHEEK_SQUINT_RIGHT, channel("squint_r"), 0.35),
    ];
    for (name, ch_name, w) in [
        ("browOuterUpLeft", "brow_outer_up_l", 1.0),
        ("browOuterUpRight", "brow_outer_up_r", 1.0),
        ("mouthDimpleLeft", "dimple_l", 0.6),
        ("mouthDimpleRight", "dimple_r", 0.6),
        ("mouthPucker", "pucker", 1.0),
        ("mouthFunnel", "funnel", 1.0),
        ("mouthShrugLower", "shrug_lower", 1.0),
        ("mouthShrugUpper", "shrug_lower", 0.3),
        ("mouthUpperUpLeft", "upper_up_l", 1.0),
        ("mouthUpperUpRight", "upper_up_r", 1.0),
        ("mouthLowerDownLeft", "lower_down_l", 1.0),
        ("mouthLowerDownRight", "lower_down_r", 1.0),
    ] {
        m.push((named(name), channel(ch_name), w));
    }
    m
}

/// Channel weights for a face this frame (unset channels 0). The upper lids follow the gaze
/// down, as real lids do.
pub fn channel_weights(face: &FaceControls, out: &mut [f32]) {
    out.fill(0.0);
    for &(a, c, w) in arkit_map() {
        if let (Some(v), Some(o)) = (face.weights.get(a), out.get_mut(c)) {
            *o += v * w;
        }
    }
    let down = (-face.gaze_pitch / 0.45).clamp(0.0, 1.0) * 0.3;
    for c in [channel("blink_l"), channel("blink_r")] {
        out[c] += down;
    }
    for o in out.iter_mut() {
        *o = o.clamp(0.0, 1.0);
    }
}

/// Expression shapes per skin vertex, packed for the shaders: each entry is
/// `[channel | dx<<16, dy | dz<<16, nx | ny<<16, nz]` with the position and normal offsets as
/// half floats.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FaceShapes {
    /// Per skin vertex, where its entries start (one more than the vertex count).
    pub offsets: Vec<u32>,
    pub entries: Vec<[u32; 4]>,
}

impl FaceShapes {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Half float bits of `x` (round to nearest, tiny values flushed to zero).
pub fn f16_bits(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let mant = b & 0x7f_ffff;
    if exp <= 0 {
        // Subnormal: the mantissa with its leading one, shifted down.
        if exp < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - exp) as u32;
        let mut h = m >> shift;
        if (m >> (shift - 1)) & 1 != 0 {
            h += 1;
        }
        return sign | h as u16;
    }
    if exp >= 31 {
        return sign | 0x7bff;
    }
    let mut h = ((exp as u32) << 10) | (mant >> 13);
    if mant & 0x1000 != 0 {
        h += 1;
    }
    sign | h.min(0x7bff) as u16
}

fn f16_value(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f32;
    if exp == 0 {
        return sign * mant * 2f32.powi(-24);
    }
    sign * (1.0 + mant / 1024.0) * 2f32.powi(exp - 15)
}

/// Unpack one entry: (channel, position offset, normal offset).
pub fn unpack(e: &[u32; 4]) -> (usize, Vec3, Vec3) {
    let lo = |v: u32| f16_value(v as u16);
    let hi = |v: u32| f16_value((v >> 16) as u16);
    (
        (e[0] & 0xffff) as usize,
        Vec3::new(hi(e[0]), lo(e[1]), hi(e[1])),
        Vec3::new(lo(e[2]), hi(e[2]), lo(e[3])),
    )
}

/// Build the expression shapes of a skin (empty when the pack has no expression units).
pub fn build(pack: &HumanPack, skin: &SkinMesh, a: &Appearance) -> FaceShapes {
    let macros = morph::macro_factors(a);
    let ancestries = ["african", "asian", "caucasian"];
    // Per channel, offsets of the pack's vertices.
    let nv = pack.positions.len();
    let mut base: Vec<Vec<Vec3>> = Vec::with_capacity(CHANNELS.len());
    let mut any = false;
    for c in &CHANNELS {
        let mut d = vec![Vec3::ZERO; nv];
        for (unit, w) in c.units {
            for anc in ancestries {
                let share = macros.get(anc).copied().unwrap_or(0.0);
                let Some(t) = pack.target(&format!("expression/units/{anc}/{unit}")) else {
                    continue;
                };
                if share <= 0.0 {
                    continue;
                }
                any = true;
                for (i, delta) in t.indices.iter().zip(&t.deltas) {
                    let x = pack.positions[*i as usize].x;
                    let half = match c.half {
                        Half::Both => 1.0,
                        Half::Left => smooth(-0.008, 0.008, x),
                        Half::Right => 1.0 - smooth(-0.008, 0.008, x),
                    };
                    d[*i as usize] += *delta * (share * w * half);
                }
            }
        }
        base.push(d);
    }
    let n = skin.positions.len();
    let n_skin = skin.skin_vertices();
    if !any || n == 0 || skin.base.is_empty() {
        return FaceShapes::default();
    }
    // Subdivide the offsets with the skin, a few channels at a time.
    const BATCH: usize = 8;
    let mut per_vertex: Vec<Vec<[u32; 4]>> = vec![Vec::new(); n];
    let mut moved = vec![Vec3::ZERO; n];
    for start in (0..CHANNELS.len()).step_by(BATCH) {
        let end = (start + BATCH).min(CHANNELS.len());
        let k = end - start;
        let values = skin.subdivide_values(k * 3, |v, out| {
            for c in 0..k {
                let d = base[start + c][v];
                out[c * 3..c * 3 + 3].copy_from_slice(&d.to_array());
            }
        });
        for c in 0..k {
            let mut any_moved = false;
            for (i, m) in moved.iter_mut().enumerate() {
                *m = if i < n_skin {
                    let o = (i * k + c) * 3;
                    Vec3::new(values[o], values[o + 1], values[o + 2])
                } else {
                    // The mouth follows its anchors.
                    skin.extra[i - n_skin]
                        .iter()
                        .map(|(v, w)| base[start + c][*v as usize] * *w)
                        .sum()
                };
                any_moved |= m.length_squared() > 1e-12;
            }
            if !any_moved {
                continue;
            }
            let posed: Vec<Vec3> = skin
                .positions
                .iter()
                .zip(&moved)
                .map(|(p, d)| *p + *d)
                .collect();
            let normals = vertex_normals(&posed, &skin.indices);
            let channel = (start + c) as u32;
            for i in 0..n {
                let dp = moved[i];
                let dn = if i < n_skin {
                    normals[i] - skin.normals[i]
                } else {
                    Vec3::ZERO
                };
                if dp.length() < 1e-5 && dn.length() < 2e-3 {
                    continue;
                }
                let h = |x: f32| f16_bits(x) as u32;
                per_vertex[i].push([
                    channel | h(dp.x) << 16,
                    h(dp.y) | h(dp.z) << 16,
                    h(dn.x) | h(dn.y) << 16,
                    h(dn.z),
                ]);
            }
        }
    }
    let mut out = FaceShapes {
        offsets: Vec::with_capacity(n + 1),
        entries: Vec::new(),
    };
    for v in per_vertex {
        out.offsets.push(out.entries.len() as u32);
        out.entries.extend(v);
    }
    out.offsets.push(out.entries.len() as u32);
    out
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip() {
        for x in [0.0f32, 0.0413, -0.012, 1e-4, -3.5e-5, 0.5, -1.0] {
            let back = f16_value(f16_bits(x));
            assert!((back - x).abs() <= x.abs() * 1e-3 + 1e-7, "{x} -> {back}");
        }
        let e = [
            7 | (f16_bits(0.01) as u32) << 16,
            f16_bits(-0.02) as u32,
            0,
            f16_bits(0.5) as u32,
        ];
        let (c, dp, dn) = unpack(&e);
        assert_eq!(c, 7);
        assert!((dp.x - 0.01).abs() < 1e-5 && (dp.y + 0.02).abs() < 1e-5);
        assert!((dn.z - 0.5).abs() < 1e-3);
    }

    #[test]
    fn arkit_weights_drive_the_matching_halves() {
        let mut face = FaceControls::default();
        face.set("mouthSmileLeft", 0.8);
        face.set("jawOpen", 0.5);
        face.set("browInnerUp", 1.0);
        let mut w = [0.0; MAX_CHANNELS];
        channel_weights(&face, &mut w);
        assert_eq!(w[channel("smile_l")], 0.8);
        assert_eq!(w[channel("smile_r")], 0.0);
        assert_eq!(w[channel("jaw_open")], 0.5);
        assert_eq!(w[channel("brow_inner_up")], 1.0);
        // Looking down lowers the lids a little.
        face.gaze_pitch = -0.4;
        channel_weights(&face, &mut w);
        assert!(w[channel("blink_l")] > 0.2 && w[channel("blink_l")] < 0.35);
        assert!(CHANNELS.len() <= MAX_CHANNELS);
    }
}
