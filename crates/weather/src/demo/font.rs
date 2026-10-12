//! A small stroke font and an anti-aliased coverage canvas for painting signs, scales and
//! labels into textures (thermometer scale, snow stake numbers, the station sign).
//!
//! Glyphs are polylines on a 4 × 6 grid (baseline at y = 0, cap height 6), drawn with round
//! caps at any size, like the DIN lettering on road signs.

use engine_core::math::Vec2;

/// Strokes of a glyph: polylines of (x, y) points, y up.
fn glyph(c: char) -> &'static [&'static [(f32, f32)]] {
    match c {
        '0' => &[&[
            (1.2, 0.0),
            (2.8, 0.0),
            (4.0, 1.2),
            (4.0, 4.8),
            (2.8, 6.0),
            (1.2, 6.0),
            (0.0, 4.8),
            (0.0, 1.2),
            (1.2, 0.0),
        ]],
        '1' => &[&[(0.9, 4.6), (2.6, 6.0), (2.6, 0.0)]],
        '2' => &[&[
            (0.0, 4.9),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.9),
            (0.0, 0.0),
            (4.1, 0.0),
        ]],
        '3' => &[
            &[
                (0.0, 5.1),
                (1.0, 6.0),
                (3.0, 6.0),
                (4.0, 5.0),
                (4.0, 4.1),
                (3.0, 3.2),
                (1.4, 3.2),
            ],
            &[
                (3.0, 3.2),
                (4.0, 2.3),
                (4.0, 1.0),
                (3.0, 0.0),
                (1.0, 0.0),
                (0.0, 0.9),
            ],
        ],
        '4' => &[&[(3.1, 0.0), (3.1, 6.0), (0.0, 1.8), (4.1, 1.8)]],
        '5' => &[&[
            (3.9, 6.0),
            (0.5, 6.0),
            (0.2, 3.3),
            (1.2, 3.8),
            (3.0, 3.8),
            (4.0, 2.8),
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 0.9),
        ]],
        '6' => &[&[
            (3.6, 5.5),
            (2.7, 6.0),
            (1.3, 6.0),
            (0.0, 4.6),
            (0.0, 1.1),
            (1.1, 0.0),
            (2.9, 0.0),
            (4.0, 1.1),
            (4.0, 2.6),
            (3.0, 3.6),
            (1.1, 3.6),
            (0.0, 2.6),
        ]],
        '7' => &[&[(0.0, 6.0), (4.0, 6.0), (1.4, 0.0)]],
        '8' => &[&[
            (2.0, 3.3),
            (0.6, 3.9),
            (0.4, 5.0),
            (1.2, 6.0),
            (2.8, 6.0),
            (3.6, 5.0),
            (3.4, 3.9),
            (2.0, 3.3),
            (0.3, 2.5),
            (0.0, 1.2),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.2),
            (3.7, 2.5),
            (2.0, 3.3),
        ]],
        '9' => &[&[
            (0.4, 0.5),
            (1.3, 0.0),
            (2.7, 0.0),
            (4.0, 1.4),
            (4.0, 4.9),
            (2.9, 6.0),
            (1.1, 6.0),
            (0.0, 4.9),
            (0.0, 3.4),
            (1.0, 2.4),
            (2.9, 2.4),
            (4.0, 3.4),
        ]],
        '-' => &[&[(0.6, 3.0), (3.4, 3.0)]],
        '+' => &[&[(2.0, 1.2), (2.0, 4.8)], &[(0.2, 3.0), (3.8, 3.0)]],
        '.' => &[&[(2.0, 0.15), (2.0, 0.2)]],
        ',' => &[&[(2.1, 0.3), (1.6, -0.9)]],
        ':' => &[&[(2.0, 0.2), (2.0, 0.25)], &[(2.0, 3.8), (2.0, 3.85)]],
        '/' => &[&[(0.2, 0.0), (3.8, 6.0)]],
        '\'' => &[&[(2.0, 6.0), (2.0, 4.6)]],
        '°' => &[&[
            (1.2, 6.0),
            (2.0, 5.6),
            (2.0, 4.8),
            (1.2, 4.4),
            (0.4, 4.8),
            (0.4, 5.6),
            (1.2, 6.0),
        ]],
        'A' => &[
            &[(0.0, 0.0), (2.0, 6.0), (4.0, 0.0)],
            &[(0.7, 2.0), (3.3, 2.0)],
        ],
        'B' => &[
            &[
                (0.0, 0.0),
                (0.0, 6.0),
                (2.8, 6.0),
                (3.8, 5.2),
                (3.8, 3.9),
                (2.8, 3.1),
                (0.0, 3.1),
            ],
            &[(2.8, 3.1), (4.0, 2.2), (4.0, 0.9), (3.0, 0.0), (0.0, 0.0)],
        ],
        'C' => &[&[
            (4.0, 4.9),
            (2.9, 6.0),
            (1.1, 6.0),
            (0.0, 4.9),
            (0.0, 1.1),
            (1.1, 0.0),
            (2.9, 0.0),
            (4.0, 1.1),
        ]],
        'D' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (2.4, 6.0),
            (4.0, 4.4),
            (4.0, 1.6),
            (2.4, 0.0),
            (0.0, 0.0),
        ]],
        'E' => &[
            &[(4.0, 6.0), (0.0, 6.0), (0.0, 0.0), (4.0, 0.0)],
            &[(0.0, 3.1), (3.0, 3.1)],
        ],
        'F' => &[
            &[(4.0, 6.0), (0.0, 6.0), (0.0, 0.0)],
            &[(0.0, 3.1), (3.0, 3.1)],
        ],
        'G' => &[&[
            (4.0, 4.9),
            (2.9, 6.0),
            (1.1, 6.0),
            (0.0, 4.9),
            (0.0, 1.1),
            (1.1, 0.0),
            (2.9, 0.0),
            (4.0, 1.1),
            (4.0, 2.8),
            (2.3, 2.8),
        ]],
        'H' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 0.0), (4.0, 6.0)],
            &[(0.0, 3.1), (4.0, 3.1)],
        ],
        'I' => &[
            &[(2.0, 0.0), (2.0, 6.0)],
            &[(1.0, 6.0), (3.0, 6.0)],
            &[(1.0, 0.0), (3.0, 0.0)],
        ],
        'J' => &[&[(4.0, 6.0), (4.0, 1.1), (2.9, 0.0), (1.1, 0.0), (0.0, 1.1)]],
        'K' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 6.0), (0.0, 2.2)],
            &[(1.4, 3.5), (4.0, 0.0)],
        ],
        'L' => &[&[(0.0, 6.0), (0.0, 0.0), (4.0, 0.0)]],
        'M' => &[&[(0.0, 0.0), (0.0, 6.0), (2.0, 2.4), (4.0, 6.0), (4.0, 0.0)]],
        'N' => &[&[(0.0, 0.0), (0.0, 6.0), (4.0, 0.0), (4.0, 6.0)]],
        'O' => &[&[
            (1.2, 0.0),
            (2.8, 0.0),
            (4.0, 1.2),
            (4.0, 4.8),
            (2.8, 6.0),
            (1.2, 6.0),
            (0.0, 4.8),
            (0.0, 1.2),
            (1.2, 0.0),
        ]],
        'P' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.8),
            (3.0, 2.8),
            (0.0, 2.8),
        ]],
        'Q' => &[
            &[
                (1.2, 0.0),
                (2.8, 0.0),
                (4.0, 1.2),
                (4.0, 4.8),
                (2.8, 6.0),
                (1.2, 6.0),
                (0.0, 4.8),
                (0.0, 1.2),
                (1.2, 0.0),
            ],
            &[(2.5, 1.5), (4.0, 0.0)],
        ],
        'R' => &[
            &[
                (0.0, 0.0),
                (0.0, 6.0),
                (3.0, 6.0),
                (4.0, 5.0),
                (4.0, 3.8),
                (3.0, 2.8),
                (0.0, 2.8),
            ],
            &[(2.0, 2.8), (4.0, 0.0)],
        ],
        'S' => &[&[
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 4.1),
            (1.0, 3.2),
            (3.0, 2.9),
            (4.0, 2.0),
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
        ]],
        'T' => &[&[(0.0, 6.0), (4.0, 6.0)], &[(2.0, 6.0), (2.0, 0.0)]],
        'U' => &[&[
            (0.0, 6.0),
            (0.0, 1.1),
            (1.1, 0.0),
            (2.9, 0.0),
            (4.0, 1.1),
            (4.0, 6.0),
        ]],
        'V' => &[&[(0.0, 6.0), (2.0, 0.0), (4.0, 6.0)]],
        'W' => &[&[(0.0, 6.0), (1.0, 0.0), (2.0, 4.0), (3.0, 0.0), (4.0, 6.0)]],
        'X' => &[&[(0.0, 0.0), (4.0, 6.0)], &[(0.0, 6.0), (4.0, 0.0)]],
        'Y' => &[
            &[(0.0, 6.0), (2.0, 3.0), (4.0, 6.0)],
            &[(2.0, 3.0), (2.0, 0.0)],
        ],
        'Z' => &[&[(0.0, 6.0), (4.0, 6.0), (0.0, 0.0), (4.0, 0.0)]],
        _ => &[],
    }
}

/// Horizontal advance of a glyph in grid units (letters are 4 wide plus spacing).
fn advance(c: char) -> f32 {
    match c {
        ' ' => 3.6,
        '.' | ':' | ',' | '\'' => 2.4,
        '1' | 'I' => 4.6,
        '°' => 3.0,
        _ => 5.6,
    }
}

/// Width of `text` in grid units (without the trailing spacing).
pub fn text_width(text: &str) -> f32 {
    text.chars().map(advance).sum::<f32>() - 1.6
}

/// Anti-aliased coverage buffer (0..1 per pixel) to paint strokes and shapes into.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub data: Vec<f32>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            data: vec![0.0; (w * h) as usize],
        }
    }

    #[inline]
    pub fn at(&self, x: u32, y: u32) -> f32 {
        self.data[(y.min(self.h - 1) * self.w + x.min(self.w - 1)) as usize]
    }

    /// Thick line from `a` to `b` (pixels, y down) with round caps, `width` pixels wide.
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32) {
        let r = width * 0.5;
        let lo = a.min(b) - Vec2::splat(r + 1.0);
        let hi = a.max(b) + Vec2::splat(r + 1.0);
        let x0 = lo.x.floor().max(0.0) as u32;
        let y0 = lo.y.floor().max(0.0) as u32;
        let x1 = (hi.x.ceil() as i64).clamp(0, self.w as i64) as u32;
        let y1 = (hi.y.ceil() as i64).clamp(0, self.h as i64) as u32;
        let ab = b - a;
        let len2 = ab.length_squared().max(1e-9);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let d = p.distance(a + ab * t);
                let c = (r - d + 0.5).clamp(0.0, 1.0);
                let i = (y * self.w + x) as usize;
                if c > self.data[i] {
                    self.data[i] = c;
                }
            }
        }
    }

    /// Filled axis-aligned rectangle (pixels), anti-aliased edges.
    pub fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let xa = x0.floor().max(0.0) as u32;
        let ya = y0.floor().max(0.0) as u32;
        let xb = (x1.ceil() as i64).clamp(0, self.w as i64) as u32;
        let yb = (y1.ceil() as i64).clamp(0, self.h as i64) as u32;
        for y in ya..yb {
            let cy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
            for x in xa..xb {
                let cx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                let i = (y * self.w + x) as usize;
                self.data[i] = self.data[i].max(cx * cy);
            }
        }
    }

    /// Filled disc (pixels).
    pub fn disc(&mut self, c: Vec2, r: f32) {
        self.line(c, c, r * 2.0);
    }

    /// Circle outline (pixels).
    pub fn ring(&mut self, c: Vec2, r: f32, width: f32) {
        let n = ((r * 0.8) as u32).clamp(12, 96);
        for k in 0..n {
            let a0 = k as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (k + 1) as f32 / n as f32 * std::f32::consts::TAU;
            self.line(
                c + Vec2::new(a0.cos(), a0.sin()) * r,
                c + Vec2::new(a1.cos(), a1.sin()) * r,
                width,
            );
        }
    }

    /// Write `text` with its baseline-left corner at `origin` (pixels, y down), `cap` pixels
    /// tall, strokes `weight` pixels wide. `aspect` stretches x (for non-square texels).
    pub fn text(&mut self, text: &str, origin: Vec2, cap: f32, weight: f32, aspect: f32) {
        let s = cap / 6.0;
        let mut x = 0.0;
        for c in text.chars() {
            for stroke in glyph(c) {
                for w in stroke.windows(2) {
                    let p = |q: (f32, f32)| origin + Vec2::new((x + q.0) * s * aspect, -q.1 * s);
                    self.line(p(w[0]), p(w[1]), weight);
                }
            }
            x += advance(c);
        }
    }

    /// Like [`Canvas::text`], centered horizontally on `center_x`.
    pub fn text_centered(
        &mut self,
        text: &str,
        center_x: f32,
        baseline: f32,
        cap: f32,
        weight: f32,
    ) {
        let w = text_width(text) * cap / 6.0;
        self.text(
            text,
            Vec2::new(center_x - w * 0.5, baseline),
            cap,
            weight,
            1.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_paints_inside_its_box() {
        let mut c = Canvas::new(200, 60);
        c.text("WEATHER 12.5°C", Vec2::new(4.0, 50.0), 30.0, 4.0, 0.45);
        let painted = c.data.iter().filter(|v| **v > 0.5).count();
        assert!(painted > 300, "{painted}");
        // Nothing above the cap height (minus the stroke radius).
        for y in 0..16 {
            for x in 0..200 {
                assert!(c.at(x, y) < 0.01, "paint at {x},{y}");
            }
        }
        for ch in "WEATHER STATION TEMPERATURE 0123456789-+.:/°MSHKBUOPRNDGFCLJQVXYZ".chars() {
            assert!(ch == ' ' || !glyph(ch).is_empty(), "{ch} has no glyph");
        }
    }
}
