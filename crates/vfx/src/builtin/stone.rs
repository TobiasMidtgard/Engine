//! Shaping's raised stone (magic round 3): the look key `stone`. Only the bolt and its impact
//! are drawn here; walls and pillars are real rock meshes, and magic writes their dust.
//!
//! Stone is matter, not magic: lit solid rock and grit, dust, and no flash or glow, so it reads
//! apart from the six elements at a glance.

use super::*;

/// Shaping's stone, as magic colors it.
const STONE: [f32; 3] = [0.42, 0.38, 0.33];
const DUST: [f32; 3] = [0.5, 0.46, 0.4];

pub(super) fn all() -> Vec<EffectDesc> {
    vec![stone_bolt(), stone_impact()]
}

/// A tumbling fist of rock: a few lumps grinding round each other, shedding grit and a thin
/// trail of dust.
fn stone_bolt() -> EffectDesc {
    effect(
        "spell.stone.bolt",
        "A stone bolt: a tumbling fist of rock lumps grinding round each other, grit falling off \
         it and a thin trail of dust behind.",
        None,
        vec![
            EmitterDesc {
                rate: 7.0,
                shape: EmitShape::Sphere {
                    radius: 0.06,
                    surface: false,
                },
                speed: [0.0, 0.05],
                local_space: true,
                lifetime: [0.45, 0.6],
                size: [0.18, 0.28],
                size_over_life: c(&[(0.0, 0.7), (0.15, 1.0), (0.85, 1.0), (1.0, 0.6)]),
                rotation_deg: [-180.0, 180.0],
                spin_deg: [-420.0, 420.0],
                sprite: Sprite::Shard,
                debris: Some(Debris::Rock),
                lit: 1.0,
                color: g(&[
                    (0.0, [STONE[0], STONE[1], STONE[2], 1.0]),
                    (1.0, [STONE[0], STONE[1], STONE[2], 1.0]),
                ]),
                soft: 0.0,
                wind: 0.0,
                ..em("fist")
            },
            EmitterDesc {
                rate: 10.0,
                rate_per_meter: 6.0,
                gravity: 1.0,
                drag: 0.4,
                speed: [0.3, 1.2],
                lifetime: [0.6, 1.1],
                size: [0.015, 0.04],
                collide: Collide::Die,
                bursts: Vec::new(),
                ..chips("grit", 0, STONE, [0.3, 1.2])
            },
            EmitterDesc {
                rate: 6.0,
                rate_per_meter: 3.0,
                shape: EmitShape::Sphere {
                    radius: 0.1,
                    surface: false,
                },
                lifetime: [0.6, 1.0],
                size: [0.14, 0.22],
                bursts: Vec::new(),
                ..dust_puff("dust", 0, DUST, 0.22, [0.05, 0.3], [0.14, 0.22])
            },
        ],
    )
}

/// A stone bolt landing (or a stone grip cracking): gravel and chips spraying low, a few heavy
/// chunks, and a dust puff that rolls out over the ground. No flash.
fn stone_impact() -> EffectDesc {
    EffectDesc {
        mark: mark(MarkKind::Chip, 0.35, 120.0, 0.0),
        ..effect(
            "spell.stone.impact",
            "Stone shattering: gravel and chips spraying low and bouncing, a few heavy chunks, \
             and a dust puff rolling out over the ground. No flash.",
            Some(0.1),
            vec![
                EmitterDesc {
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    size: [0.03, 0.07],
                    ..chips("gravel", 45, STONE, [2.0, 5.0])
                },
                EmitterDesc {
                    shape: EmitShape::Hemisphere { radius: 0.15 },
                    size: [0.09, 0.16],
                    spin_deg: [-360.0, 360.0],
                    lifetime: [2.0, 3.5],
                    ..chips("chunks", 7, [0.38, 0.34, 0.3], [1.2, 3.0])
                },
                EmitterDesc {
                    shape: EmitShape::Hemisphere { radius: 0.3 },
                    acceleration: [0.0, 0.15, 0.0],
                    lifetime: [1.2, 2.0],
                    ..dust_puff("dust", 10, DUST, 0.5, [1.0, 2.8], [0.45, 0.7])
                },
                // A ring of dust kicked out flat along the ground.
                EmitterDesc {
                    shape: EmitShape::Hemisphere { radius: 0.15 },
                    velocity: [0.0, -0.6, 0.0],
                    lifetime: [0.9, 1.4],
                    ..dust_puff("skirt", 14, DUST, 0.35, [3.0, 4.5], [0.3, 0.45])
                },
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stone_has_a_bolt_and_an_impact_and_never_glows() {
        let all = all();
        for name in ["spell.stone.bolt", "spell.stone.impact"] {
            let e = all.iter().find(|e| e.name == name).expect(name);
            assert!(e.light.is_none(), "{name}: stone casts no light");
            assert!(
                e.emitters
                    .iter()
                    .all(|em| em.emissive == 0.0 && em.additive == 0.0),
                "{name}: stone is matter, never a flash"
            );
        }
    }
}
