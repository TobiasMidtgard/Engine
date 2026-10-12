//! A small stroke font for painted markings and signs: runway designators, grid slot numbers,
//! pad codes, sponsor boards, taxiway signs.
//!
//! Every glyph is a few polylines in a 4 × 6 box (x right, y up from the baseline), drawn
//! with chamfered corners in the style of airfield and road-marking typefaces. Glyphs are
//! monospaced: each takes [`ADVANCE`] units.

/// Glyph box width (units).
pub const WIDTH: f32 = 4.0;
/// Cap height (units).
pub const HEIGHT: f32 = 6.0;
/// Horizontal advance per character, including the gap (units).
pub const ADVANCE: f32 = 5.2;

type Line = &'static [(f32, f32)];

/// The polylines of `c` (uppercase letters, digits, `- . / < > ^`); `None` for spaces and
/// unknown characters.
pub fn glyph(c: char) -> Option<&'static [Line]> {
    const O: Line = &[
        (0.8, 0.0),
        (3.2, 0.0),
        (4.0, 0.8),
        (4.0, 5.2),
        (3.2, 6.0),
        (0.8, 6.0),
        (0.0, 5.2),
        (0.0, 0.8),
        (0.8, 0.0),
    ];
    const P_BOWL: Line = &[
        (0.0, 0.0),
        (0.0, 6.0),
        (3.2, 6.0),
        (4.0, 5.2),
        (4.0, 3.6),
        (3.2, 2.8),
        (0.0, 2.8),
    ];
    Some(match c.to_ascii_uppercase() {
        '0' | 'O' => &[O],
        '1' => &[
            &[(0.9, 4.9), (2.1, 6.0), (2.1, 0.0)],
            &[(0.9, 0.0), (3.3, 0.0)],
        ],
        '2' => &[&[
            (0.0, 5.2),
            (0.8, 6.0),
            (3.2, 6.0),
            (4.0, 5.2),
            (4.0, 4.0),
            (0.0, 0.0),
            (4.0, 0.0),
        ]],
        '3' => &[
            &[
                (0.0, 5.2),
                (0.8, 6.0),
                (3.2, 6.0),
                (4.0, 5.2),
                (4.0, 3.8),
                (3.2, 3.1),
                (1.3, 3.1),
            ],
            &[
                (3.2, 3.1),
                (4.0, 2.4),
                (4.0, 0.8),
                (3.2, 0.0),
                (0.8, 0.0),
                (0.0, 0.8),
            ],
        ],
        '4' => &[&[(3.0, 0.0), (3.0, 6.0), (0.0, 1.9), (4.0, 1.9)]],
        '5' => &[&[
            (4.0, 6.0),
            (0.2, 6.0),
            (0.0, 3.4),
            (3.2, 3.4),
            (4.0, 2.6),
            (4.0, 0.8),
            (3.2, 0.0),
            (0.8, 0.0),
            (0.0, 0.8),
        ]],
        '6' => &[&[
            (3.8, 6.0),
            (1.4, 6.0),
            (0.0, 4.6),
            (0.0, 0.8),
            (0.8, 0.0),
            (3.2, 0.0),
            (4.0, 0.8),
            (4.0, 2.6),
            (3.2, 3.4),
            (0.0, 3.4),
        ]],
        '7' => &[&[(0.0, 6.0), (4.0, 6.0), (4.0, 5.2), (1.6, 0.0)]],
        '8' => &[
            &[
                (0.8, 3.2),
                (0.0, 2.4),
                (0.0, 0.8),
                (0.8, 0.0),
                (3.2, 0.0),
                (4.0, 0.8),
                (4.0, 2.4),
                (3.2, 3.2),
                (0.8, 3.2),
            ],
            &[
                (0.8, 3.2),
                (0.2, 3.8),
                (0.2, 5.3),
                (0.9, 6.0),
                (3.1, 6.0),
                (3.8, 5.3),
                (3.8, 3.8),
                (3.2, 3.2),
            ],
        ],
        '9' => &[&[
            (0.2, 0.0),
            (2.6, 0.0),
            (4.0, 1.4),
            (4.0, 5.2),
            (3.2, 6.0),
            (0.8, 6.0),
            (0.0, 5.2),
            (0.0, 3.4),
            (0.8, 2.6),
            (4.0, 2.6),
        ]],
        'A' => &[
            &[
                (0.0, 0.0),
                (0.0, 4.4),
                (1.6, 6.0),
                (2.4, 6.0),
                (4.0, 4.4),
                (4.0, 0.0),
            ],
            &[(0.0, 2.6), (4.0, 2.6)],
        ],
        'B' => &[
            &[
                (0.0, 0.0),
                (0.0, 6.0),
                (3.0, 6.0),
                (3.8, 5.2),
                (3.8, 3.9),
                (3.0, 3.1),
                (0.0, 3.1),
            ],
            &[(3.0, 3.1), (4.0, 2.3), (4.0, 0.8), (3.2, 0.0), (0.0, 0.0)],
        ],
        'C' => &[&[
            (4.0, 5.2),
            (3.2, 6.0),
            (0.8, 6.0),
            (0.0, 5.2),
            (0.0, 0.8),
            (0.8, 0.0),
            (3.2, 0.0),
            (4.0, 0.8),
        ]],
        'D' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (2.6, 6.0),
            (4.0, 4.6),
            (4.0, 1.4),
            (2.6, 0.0),
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
            (4.0, 5.2),
            (3.2, 6.0),
            (0.8, 6.0),
            (0.0, 5.2),
            (0.0, 0.8),
            (0.8, 0.0),
            (3.2, 0.0),
            (4.0, 0.8),
            (4.0, 2.8),
            (2.2, 2.8),
        ]],
        'H' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 0.0), (4.0, 6.0)],
            &[(0.0, 3.1), (4.0, 3.1)],
        ],
        'I' => &[
            &[(2.0, 0.0), (2.0, 6.0)],
            &[(0.9, 6.0), (3.1, 6.0)],
            &[(0.9, 0.0), (3.1, 0.0)],
        ],
        'J' => &[&[
            (4.0, 6.0),
            (4.0, 0.8),
            (3.2, 0.0),
            (0.8, 0.0),
            (0.0, 0.8),
            (0.0, 1.8),
        ]],
        'K' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 6.0), (0.3, 2.6)],
            &[(1.5, 3.7), (4.0, 0.0)],
        ],
        'L' => &[&[(0.0, 6.0), (0.0, 0.0), (4.0, 0.0)]],
        'M' => &[&[(0.0, 0.0), (0.0, 6.0), (2.0, 3.0), (4.0, 6.0), (4.0, 0.0)]],
        'N' => &[&[(0.0, 0.0), (0.0, 6.0), (4.0, 0.0), (4.0, 6.0)]],
        'P' => &[P_BOWL],
        'Q' => &[O, &[(2.6, 1.2), (4.2, -0.4)]],
        'R' => &[P_BOWL, &[(2.4, 2.8), (4.0, 0.0)]],
        'S' => &[&[
            (4.0, 5.2),
            (3.2, 6.0),
            (0.8, 6.0),
            (0.0, 5.2),
            (0.0, 3.8),
            (0.7, 3.1),
            (3.3, 2.9),
            (4.0, 2.2),
            (4.0, 0.8),
            (3.2, 0.0),
            (0.8, 0.0),
            (0.0, 0.8),
        ]],
        'T' => &[&[(0.0, 6.0), (4.0, 6.0)], &[(2.0, 6.0), (2.0, 0.0)]],
        'U' => &[&[
            (0.0, 6.0),
            (0.0, 0.8),
            (0.8, 0.0),
            (3.2, 0.0),
            (4.0, 0.8),
            (4.0, 6.0),
        ]],
        'V' => &[&[(0.0, 6.0), (2.0, 0.0), (4.0, 6.0)]],
        'W' => &[&[(0.0, 6.0), (0.9, 0.0), (2.0, 4.0), (3.1, 0.0), (4.0, 6.0)]],
        'X' => &[&[(0.0, 6.0), (4.0, 0.0)], &[(0.0, 0.0), (4.0, 6.0)]],
        'Y' => &[
            &[(0.0, 6.0), (2.0, 3.0), (4.0, 6.0)],
            &[(2.0, 3.0), (2.0, 0.0)],
        ],
        'Z' => &[&[(0.0, 6.0), (4.0, 6.0), (0.0, 0.0), (4.0, 0.0)]],
        '-' => &[&[(0.8, 3.0), (3.2, 3.0)]],
        '.' => &[&[(1.9, 0.25), (2.1, 0.25)]],
        '/' => &[&[(0.6, 0.0), (3.4, 6.0)]],
        '>' => &[
            &[(0.4, 6.0), (3.6, 3.0), (0.4, 0.0)],
            &[(-0.8, 3.0), (3.4, 3.0)],
        ],
        '<' => &[
            &[(3.6, 6.0), (0.4, 3.0), (3.6, 0.0)],
            &[(4.8, 3.0), (0.6, 3.0)],
        ],
        '^' => &[
            &[(0.0, 2.8), (2.0, 6.0), (4.0, 2.8)],
            &[(2.0, 5.6), (2.0, 0.0)],
        ],
        _ => return None,
    })
}

/// Distance (glyph units) from `(x, y)` to the strokes of `c`, or a large value.
pub fn distance(c: char, x: f32, y: f32) -> f32 {
    let Some(lines) = glyph(c) else {
        return 1e3;
    };
    let mut best = 1e3f32;
    for line in lines {
        for w in line.windows(2) {
            let (ax, ay) = w[0];
            let (bx, by) = w[1];
            let (dx, dy) = (bx - ax, by - ay);
            let (px, py) = (x - ax, y - ay);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 {
                ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ey) = (px - dx * t, py - dy * t);
            best = best.min((ex * ex + ey * ey).sqrt());
        }
    }
    best
}

/// Ink coverage 0..1 of `c` at `(x, y)` (glyph units) for strokes `weight` units thick,
/// antialiased over `aa` units (about one pixel).
pub fn coverage(c: char, x: f32, y: f32, weight: f32, aa: f32) -> f32 {
    if x < -weight - 1.0 || x > WIDTH + weight + 1.0 || y < -weight - 1.0 || y > HEIGHT + weight {
        return 0.0;
    }
    let d = distance(c, x, y);
    ((weight * 0.5 - d) / aa.max(1e-4) + 0.5).clamp(0.0, 1.0)
}

/// Coverage of a whole line of `text` at `(x, y)` in glyph units, the text starting at x = 0
/// on the baseline.
pub fn text_coverage(text: &str, x: f32, y: f32, weight: f32, aa: f32) -> f32 {
    if y < -weight - 1.0 || y > HEIGHT + weight + 1.0 || x < -2.0 {
        return 0.0;
    }
    let i = (x / ADVANCE).floor() as i32;
    let mut best = 0.0f32;
    // The glyph under x and its neighbors (strokes can reach past their box).
    for k in (i - 1)..=(i + 1) {
        if k < 0 {
            continue;
        }
        if let Some(c) = text.chars().nth(k as usize) {
            best = best.max(coverage(c, x - k as f32 * ADVANCE, y, weight, aa));
        }
    }
    best
}

/// Width of `text` in glyph units (from the left of the first glyph to the right of the last).
pub fn text_width(text: &str) -> f32 {
    let n = text.chars().count();
    if n == 0 {
        0.0
    } else {
        (n - 1) as f32 * ADVANCE + WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_cover_their_strokes() {
        for c in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-/<>^".chars() {
            let lines = glyph(c).unwrap_or_else(|| panic!("{c}"));
            let (x, y) = lines[0][0];
            assert!(coverage(c, x, y, 0.8, 0.1) > 0.99, "{c}");
        }
        assert!(glyph(' ').is_none());
        assert_eq!(coverage('1', 3.9, 3.0, 0.8, 0.1), 0.0);
        assert!(text_coverage("18", ADVANCE + 2.1, 3.0, 0.8, 0.1) > 0.9);
    }
}
