//! The picture healing is drawn with (round-19-plan.md), painted in code: a blossom of light for
//! the healing circle's carpet of flowers, cut out round its petals for alpha-masked cards; and
//! the particle looks of healing ([`heal_looks`]), gold where vfx would borrow blight's green.

use std::f32::consts::{PI, TAU};

use engine_core::math::Vec2;
use engine_render::Image;
use serde_json::{Value, json};

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

// ------------------------------------------------------------------------- particle looks

/// Healing's gold, and the near white of its hottest light.
const GOLD: [f32; 3] = [1.0, 0.78, 0.36];
const HOT: [f32; 3] = [1.0, 0.95, 0.78];

/// A colour over life: `(t, rgb, alpha)` keys.
fn grad(keys: &[(f32, [f32; 3], f32)]) -> Value {
    Value::Array(
        keys.iter()
            .map(|(t, c, a)| json!([t, [c[0], c[1], c[2], a]]))
            .collect(),
    )
}

/// Fade in, hold, fade out.
fn fade(c: [f32; 3], peak: f32, fade_in: f32, fade_out: f32) -> Value {
    grad(&[
        (0.0, c, 0.0),
        (fade_in, c, peak),
        (fade_out, c, peak),
        (1.0, c, 0.0),
    ])
}

/// Motes of light, hot at birth and gold as they rise and go out.
fn mote_colors() -> Value {
    grad(&[
        (0.0, HOT, 0.0),
        (0.15, HOT, 1.0),
        (0.6, GOLD, 0.8),
        (1.0, GOLD, 0.0),
    ])
}

fn light(lumens: f32, range: f32, fade_secs: f32) -> Value {
    json!({ "color": GOLD, "intensity_lm": lumens, "range": range, "flicker": 0.12,
            "offset": [0.0, 0.0, 0.0], "fade_secs": fade_secs })
}

/// The looks healing is drawn with in vfx, as `vfx.define` takes them (the `vfx.describe`
/// format): `spell.life.*` for heal spells (a lone heal weaves to the Life domain's look, `life`;
/// a vfx without them borrows blight by its green, bubbles and miasma) and
/// `spell.radiance.impact`, the gold fire life lights on the dead.
///
/// * bolt: a white-gold core in a soft halo, a ribbon of light behind it, motes and glints
///   shed along its path that drift up;
/// * impact: a flash, a ring of light running out over the ground, motes and stars thrown up;
/// * field: a gold glow over the ground (2 m round at scale 1) with motes rising off it;
/// * aura: motes circling up round a body, with glints;
/// * charge: light drawn into the hand;
/// * jet: a stream of light down a beam (healing hands, a wisp's thread);
/// * radiance: gold flames licking up and sparks.
pub fn heal_looks() -> Vec<Value> {
    let bolt = json!({
        "name": "spell.life.bolt",
        "description": "A mending bolt (magic, Restoration): a white-gold core in a soft halo, \
            a ribbon of light behind it, motes and glints drifting up along its path.",
        "light": light(2500.0, 7.0, 0.2),
        "trail": { "width": 0.06, "lifetime": 0.4, "min_spacing": 0.06,
                   "color": grad(&[(0.0, HOT, 1.0), (1.0, GOLD, 0.0)]),
                   "emissive": 18000.0, "style": "energy" },
        "emitters": [
            { "name": "core", "rate": 30.0, "steady": true, "local_space": true,
              "speed": [0.0, 0.0], "lifetime": [0.12, 0.18], "size": [0.22, 0.28],
              "color": fade(HOT, 1.0, 0.2, 0.7), "emissive": 32000.0, "additive": 1.0,
              "soft": 0.0, "wind": 0.0 },
            { "name": "halo", "rate": 6.0, "steady": true, "local_space": true,
              "speed": [0.0, 0.0], "lifetime": [0.4, 0.4], "size": [0.75, 0.75],
              "color": fade(GOLD, 0.35, 0.3, 0.7), "emissive": 6000.0, "additive": 1.0,
              "soft": 0.0, "wind": 0.0 },
            { "name": "motes", "rate": 30.0, "rate_per_meter": 8.0,
              "shape": { "kind": "sphere", "radius": 0.12, "surface": true },
              "speed": [0.05, 0.3], "velocity": [0.0, 0.5, 0.0], "drag": 1.2,
              "turbulence": 0.6, "turbulence_scale": 0.5, "lifetime": [0.7, 1.3],
              "size": [0.02, 0.034], "color": mote_colors(), "emissive": 20000.0,
              "additive": 1.0, "wind": 0.2 },
            { "name": "glints", "rate": 10.0, "rate_per_meter": 1.5, "sprite": "star",
              "speed": [0.0, 0.2], "lifetime": [0.2, 0.4], "size": [0.1, 0.18],
              "spin_deg": [-90.0, 90.0], "color": fade(HOT, 1.0, 0.2, 0.5),
              "emissive": 26000.0, "additive": 1.0, "wind": 0.0 }
        ]
    });
    let impact = json!({
        "name": "spell.life.impact",
        "description": "Healing landing (magic, Restoration): a flash, a ring of light running \
            out over the ground, gold motes and stars thrown up that drift and go out.",
        "duration": 2.2,
        "light": light(3000.0, 6.0, 0.6),
        "emitters": [
            { "name": "flash", "bursts": [{ "count": 1 }], "local_space": true,
              "speed": [0.0, 0.0], "lifetime": [0.3, 0.3], "size": [1.0, 1.0],
              "size_over_life": [[0.0, 0.6], [1.0, 1.5]],
              "color": grad(&[(0.0, HOT, 0.9), (1.0, GOLD, 0.0)]), "emissive": 14000.0,
              "additive": 1.0, "soft": 0.0, "wind": 0.0 },
            { "name": "ring", "bursts": [{ "count": 1 }], "offset": [0.0, 0.05, 0.0],
              "local_space": true, "speed": [0.0, 0.0], "lifetime": [0.7, 0.7],
              "size": [0.5, 0.5], "size_over_life": [[0.0, 0.4], [1.0, 4.4]],
              "rotation_deg": [0.0, 0.0], "sprite": "ring", "facing": "horizontal",
              "color": grad(&[(0.0, HOT, 0.0), (0.1, HOT, 1.0), (1.0, GOLD, 0.0)]),
              "emissive": 14000.0, "additive": 1.0, "soft": 0.0, "wind": 0.0 },
            { "name": "motes", "bursts": [{ "count": 40 }],
              "shape": { "kind": "sphere", "radius": 0.35 }, "spread_deg": 180.0,
              "speed": [0.2, 1.1], "velocity": [0.0, 1.1, 0.0], "drag": 1.4,
              "turbulence": 0.8, "turbulence_scale": 0.6, "lifetime": [1.0, 1.9],
              "size": [0.02, 0.04], "color": mote_colors(), "emissive": 22000.0,
              "additive": 1.0, "wind": 0.2 },
            { "name": "stars", "bursts": [{ "count": 8 }],
              "shape": { "kind": "sphere", "radius": 0.3 }, "spread_deg": 180.0,
              "speed": [0.3, 0.9], "drag": 1.0, "lifetime": [0.3, 0.6],
              "size": [0.12, 0.26], "spin_deg": [-120.0, 120.0], "sprite": "star",
              "color": fade(HOT, 1.0, 0.1, 0.5), "emissive": 30000.0, "additive": 1.0,
              "wind": 0.0 }
        ]
    });
    let field = json!({
        "name": "spell.life.field",
        "description": "Healing ground (magic, Restoration): a gold glow over the ground, 2 m \
            round at scale 1, with motes of light rising off it and glints.",
        "light": light(1500.0, 6.0, 0.5),
        "emitters": [
            { "name": "pool", "rate": 2.0, "steady": true, "local_space": true,
              "offset": [0.0, 0.04, 0.0], "speed": [0.0, 0.0], "lifetime": [2.0, 2.0],
              "size": [4.2, 4.2], "spin_deg": [-8.0, 8.0], "facing": "horizontal",
              "color": fade(GOLD, 0.2, 0.5, 0.5), "emissive": 2500.0, "additive": 1.0,
              "soft": 0.0, "wind": 0.0 },
            { "name": "motes", "rate": 70.0, "shape": { "kind": "disc", "radius": 1.9 },
              "speed": [0.0, 0.1], "velocity": [0.0, 0.8, 0.0], "orbit_speed": 0.3,
              "turbulence": 0.5, "turbulence_scale": 0.8, "drag": 0.8, "wind": 0.2,
              "lifetime": [1.6, 2.6], "size": [0.016, 0.03], "facing": "velocity",
              "stretch": 0.12, "color": mote_colors(), "emissive": 20000.0,
              "additive": 1.0 },
            { "name": "glints", "rate": 8.0, "shape": { "kind": "disc", "radius": 1.8 },
              "speed": [0.0, 0.0], "velocity": [0.0, 1.0, 0.0], "lifetime": [0.5, 0.9],
              "size": [0.08, 0.15], "spin_deg": [-60.0, 60.0], "sprite": "star",
              "color": fade(HOT, 1.0, 0.2, 0.6), "emissive": 22000.0, "additive": 1.0,
              "wind": 0.0 }
        ]
    });
    let aura = json!({
        "name": "spell.life.aura",
        "description": "A healing aura (magic, Restoration): gold motes circling up round a \
            body, and glints.",
        "light": light(1200.0, 4.0, 0.4),
        "emitters": [
            { "name": "motes", "rate": 40.0,
              "shape": { "kind": "ring", "radius": 0.55, "thickness": 0.1 },
              "speed": [0.0, 0.05], "velocity": [0.0, 0.7, 0.0], "orbit_speed": 0.9,
              "turbulence": 0.3, "drag": 0.6, "wind": 0.1, "lifetime": [1.2, 2.0],
              "size": [0.018, 0.032], "facing": "velocity", "stretch": 0.1,
              "local_space": true, "color": mote_colors(), "emissive": 20000.0,
              "additive": 1.0 },
            { "name": "glints", "rate": 5.0,
              "shape": { "kind": "sphere", "radius": 0.6 }, "speed": [0.0, 0.0],
              "velocity": [0.0, 0.6, 0.0], "lifetime": [0.4, 0.7], "size": [0.08, 0.14],
              "sprite": "star", "spin_deg": [-60.0, 60.0],
              "color": fade(HOT, 1.0, 0.2, 0.6), "emissive": 22000.0, "additive": 1.0,
              "wind": 0.0 }
        ]
    });
    let charge = json!({
        "name": "spell.life.charge",
        "description": "Healing gathering in a hand (magic, Restoration): motes of light drawn \
            in to a growing white-gold glow.",
        "light": light(1200.0, 4.0, 0.2),
        "emitters": [
            { "name": "core", "rate": 20.0, "steady": true, "local_space": true,
              "speed": [0.0, 0.0], "lifetime": [0.2, 0.2], "size": [0.12, 0.16],
              "color": fade(HOT, 1.0, 0.2, 0.7), "emissive": 22000.0, "additive": 1.0,
              "soft": 0.0, "wind": 0.0 },
            { "name": "inflow", "rate": 50.0, "local_space": true,
              "shape": { "kind": "sphere", "radius": 0.45, "surface": true },
              "speed": [0.0, 0.0], "attract": 6.0, "drag": 2.0, "lifetime": [0.4, 0.6],
              "size": [0.015, 0.026], "facing": "velocity", "stretch": 0.05,
              "color": mote_colors(), "emissive": 20000.0, "additive": 1.0, "wind": 0.0 }
        ]
    });
    // vfx pours `spell.<look>.jet` along a beam of that look when it has one: scale = reach / 8 m,
    // its up axis down the beam, half a second a cue.
    let jet = json!({
        "name": "spell.life.jet",
        "description": "Healing hands and a wisp's thread (magic, Restoration): a stream of \
            white-gold light down the beam, motes shed along it that hang and drift up. Scale = \
            reach / 8 m.",
        "duration": 0.6,
        "light": light(2500.0, 6.0, 0.3),
        "emitters": [
            { "name": "stream", "rate": 140.0, "duration": 0.5,
              "shape": { "kind": "cone", "angle_deg": 1.5, "radius": 0.03 },
              "speed": [13.0, 14.5], "lifetime": [0.5, 0.58], "size": [0.05, 0.08],
              "facing": "velocity", "stretch": 0.025,
              "color": grad(&[(0.0, HOT, 0.0), (0.1, HOT, 1.0), (0.6, GOLD, 0.8),
                              (1.0, GOLD, 0.0)]),
              "emissive": 26000.0, "additive": 1.0, "soft": 0.0, "wind": 0.0 },
            { "name": "core", "rate": 60.0, "duration": 0.5,
              "shape": { "kind": "cone", "angle_deg": 0.8, "radius": 0.01 },
              "speed": [13.5, 14.0], "lifetime": [0.5, 0.55], "size": [0.025, 0.035],
              "facing": "velocity", "stretch": 0.03, "color": fade(HOT, 1.0, 0.1, 0.8),
              "emissive": 40000.0, "additive": 1.0, "soft": 0.0, "wind": 0.0 },
            { "name": "motes", "rate": 70.0, "duration": 0.5,
              "shape": { "kind": "cone", "angle_deg": 4.0, "radius": 0.05 },
              "speed": [1.0, 13.0], "drag": 2.5, "gravity": -0.06, "turbulence": 0.6,
              "lifetime": [0.6, 1.1], "size": [0.018, 0.03], "color": mote_colors(),
              "emissive": 20000.0, "additive": 1.0, "wind": 0.2 }
        ]
    });
    let radiance = json!({
        "name": "spell.radiance.impact",
        "description": "Radiance (magic, life on the dead, or fire and heal): gold-white flames \
            licking up off what it burns, a flash and sparks.",
        "duration": 1.6,
        "light": light(3500.0, 6.0, 0.5),
        "emitters": [
            { "name": "flames", "bursts": [{ "count": 10 }, { "time": 0.15, "count": 6 }],
              "shape": { "kind": "sphere", "radius": 0.3 }, "speed": [0.0, 0.3],
              "velocity": [0.0, 1.6, 0.0], "drag": 0.8, "turbulence": 0.8,
              "lifetime": [0.4, 0.8], "size": [0.3, 0.55],
              "size_over_life": [[0.0, 0.6], [0.3, 1.0], [1.0, 0.4]], "sprite": "flame",
              "color": grad(&[(0.0, HOT, 0.0), (0.12, HOT, 1.0), (0.6, GOLD, 0.7),
                              (1.0, GOLD, 0.0)]),
              "emissive": 28000.0, "additive": 1.0, "wind": 0.3 },
            { "name": "flash", "bursts": [{ "count": 1 }], "local_space": true,
              "speed": [0.0, 0.0], "lifetime": [0.25, 0.25], "size": [1.2, 1.2],
              "color": grad(&[(0.0, HOT, 0.8), (1.0, GOLD, 0.0)]), "emissive": 16000.0,
              "additive": 1.0, "soft": 0.0, "wind": 0.0 },
            { "name": "sparks", "bursts": [{ "count": 24 }],
              "shape": { "kind": "sphere", "radius": 0.25 }, "spread_deg": 180.0,
              "speed": [1.0, 3.0], "velocity": [0.0, 1.5, 0.0], "gravity": 0.3, "drag": 1.2,
              "lifetime": [0.4, 0.9], "size": [0.012, 0.022], "sprite": "spark",
              "facing": "velocity", "stretch": 0.04, "color": mote_colors(),
              "emissive": 30000.0, "additive": 1.0, "wind": 0.2 }
        ]
    });
    vec![bolt, impact, field, aura, charge, jet, radiance]
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

    #[test]
    fn the_heal_looks_are_gold_and_named_for_their_roles() {
        let looks = heal_looks();
        let names: Vec<&str> = looks.iter().filter_map(|l| l["name"].as_str()).collect();
        // Under the look a heal spell weaves to.
        let key = crate::combine::weave(&["heal".to_string()]).unwrap().key;
        for role in ["bolt", "impact", "field", "aura", "charge", "jet"] {
            assert!(
                names.contains(&format!("spell.{key}.{role}").as_str()),
                "{names:?}"
            );
        }
        for l in &looks {
            for em in l["emitters"].as_array().unwrap() {
                let keys = em["color"].as_array().unwrap();
                for k in keys {
                    let c = &k[1];
                    let (r, g, b) = (
                        c[0].as_f64().unwrap(),
                        c[1].as_f64().unwrap(),
                        c[2].as_f64().unwrap(),
                    );
                    assert!(r >= g && g > b, "gold, not green: {}", l["name"]);
                }
            }
        }
    }
}
