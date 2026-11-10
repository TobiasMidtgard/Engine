//! The picture healing is drawn with (round-19-plan.md), painted in code: a blossom of light for
//! the healing circle's carpet of flowers, cut out round its petals for alpha-masked cards.

use std::f32::consts::{PI, TAU};

use engine_core::math::Vec2;
use engine_render::Image;

use crate::pact_art::{lerp, pixel, smoothstep};

/// The blossom picture's size, px.
pub const BLOSSOM_PX: u32 = 128;
/// Petals on a blossom.
pub const PETALS: u32 = 5;

/// The blossom's colour and cover at a point of the card (0..1 across, y down): five rounded
/// petals, pale gold at their base and near white at their tips, with a fine line down each and
/// a golden heart dotted with stamens.
pub fn blossom_at(x: f32, y: f32) -> ([f32; 3], f32) {
    let p = Vec2::new(x - 0.5, y - 0.5) * 2.0;
    let r = p.length();
    let a = p.y.atan2(p.x);
    // The heart: a golden disc with darker stamen dots round its rim.
    let heart = 0.2;
    if r < heart {
        let ring = smoothstep(heart * 0.5, heart * 0.9, r);
        let dots = ((a * 9.0).sin() * 0.5 + 0.5).powf(6.0) * ring;
        let c = [
            lerp(1.0, 0.72, dots),
            lerp(0.78, 0.46, dots),
            lerp(0.22, 0.1, dots),
        ];
        return (c, 1.0);
    }
    // The petals: each a lobe narrow at the heart, widest a third of the way out and rounded
    // at its tip; how far round from the nearest petal's middle line decides its edge.
    let step = TAU / PETALS as f32;
    let turn = (a + PI / 2.0 + step * 0.5).rem_euclid(step) - step * 0.5;
    let along = r;
    let off = (turn * r).abs();
    let u = ((along - heart * 0.6) / (0.97 - heart * 0.6)).clamp(0.0, 1.0);
    let half = step * 0.5 * (0.3 + 0.62 * (PI * u.powf(0.7)).sin());
    let edge = (half - turn.abs()) * along;
    // The tip rounds off short of the card's edge.
    let tip = 0.97 - along;
    let cover = smoothstep(0.0, 0.03, edge.min(tip));
    if cover <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    let base = smoothstep(0.6, heart, along);
    let line = smoothstep(0.02, 0.0, off) * smoothstep(heart, 0.45, along) * 0.25;
    let shade = 0.82 + 0.18 * smoothstep(0.0, 0.12, edge);
    let c = [
        (lerp(1.0, 1.0, base) - line * 0.3) * shade,
        (lerp(0.97, 0.86, base) - line * 0.5) * shade,
        (lerp(0.9, 0.52, base) - line * 0.6) * shade,
    ];
    (c, cover)
}

/// The blossom as a picture: its colour, and cut out round the petals.
pub fn blossom_image() -> Image {
    let n = BLOSSOM_PX;
    let mut img = Image::from_fn(n, n, true, |px, py| {
        let (c, a) = blossom_at((px as f32 + 0.5) / n as f32, (py as f32 + 0.5) / n as f32);
        pixel(c, a)
    });
    img.generate_mips();
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blossom_has_a_golden_heart_white_petal_tips_and_gaps_between_its_petals() {
        let (heart, a) = blossom_at(0.5, 0.5);
        assert_eq!(a, 1.0);
        assert!(heart[0] > 0.9 && heart[2] < 0.3, "{heart:?}");
        // Straight up from the heart runs a petal's middle (the first petal points up the card).
        let (tip, a) = blossom_at(0.5, 0.5 - 0.35);
        assert!(a > 0.9, "{a}");
        assert!(tip[2] > 0.6, "{tip:?}");
        // Halfway round to the next petal, out past the heart, there's a gap.
        let gap = -PI / 2.0 + TAU / PETALS as f32 * 0.5;
        let (_, a) = blossom_at(0.5 + gap.cos() * 0.4, 0.5 + gap.sin() * 0.4);
        assert!(a < 0.1, "{a}");
        // Nothing past the petals' tips.
        let (_, a) = blossom_at(0.99, 0.5);
        assert_eq!(a, 0.0);
        // Five petals: count the runs of cover round a circle out on the petals.
        let mut runs = 0;
        let mut was = false;
        for i in 0..360 {
            let t = i as f32 / 360.0 * TAU;
            let (_, a) = blossom_at(0.5 + t.cos() * 0.3, 0.5 + t.sin() * 0.3);
            let on = a > 0.5;
            if on && !was {
                runs += 1;
            }
            was = on;
        }
        let first_on = blossom_at(0.5 + 0.3, 0.5).1 > 0.5;
        if first_on && was {
            runs -= 1;
        }
        assert_eq!(runs, PETALS);
    }
}
