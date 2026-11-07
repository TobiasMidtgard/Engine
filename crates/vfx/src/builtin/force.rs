//! Telekinesis (magic round 10): the `grip` lifts things, holds them before the hand and
//! throws them. Its look key is `force`; Control's rule holds, so it is grounded with no
//! glowing ball. Only the air bending round the held thing, grit falling off it, wind and dust
//! behind a throw, and matter breaking where it lands.
//!
//! * `spell.force.aura`: on a held body (scale 1 = a thing 0.65 m in radius, centred).
//! * `spell.force.bolt`: on a thrown body in flight (same scale).
//! * `spell.force.impact`: a thrown thing landing (scale 1 = 0.65 m at 20 m/s).
//! * `spell.stone.tear`: a rough chunk ripped out of the floor (scale 1 = a 0.5 m chunk, up =
//!   the floor normal). The scar is the `chip` mark.

use super::*;

/// The grip's colour (magic's `force`): a pale cold blue, used only faintly.
pub const FORCE: [f32; 3] = [0.75, 0.85, 1.0];
const GRIT: [f32; 3] = [0.42, 0.39, 0.35];
const DUST: [f32; 3] = [0.55, 0.52, 0.47];
const WOOD: [f32; 3] = [0.5, 0.37, 0.24];

pub(super) fn all() -> Vec<EffectDesc> {
    vec![force_aura(), force_bolt(), force_impact(), stone_tear()]
}

/// A held thing: the air round it bending in a slow shell, a faint ripple now and then, grit
/// and dust trickling off its underside. Thin enough never to hide what is held.
fn force_aura() -> EffectDesc {
    effect(
        "spell.force.aura",
        "A thing held by telekinesis: the air round it bends in a slow shell, a faint pale \
         ripple passes over it now and then, and grit and dust trickle off its underside. It \
         never hides what is held. Scale 1 is a thing 0.65 m in radius, centred.",
        None,
        vec![
            EmitterDesc {
                rate: 9.0,
                shape: EmitShape::Sphere {
                    radius: 0.72,
                    surface: true,
                },
                speed: [0.0, 0.05],
                orbit_speed: 0.4,
                local_space: true,
                lifetime: [0.7, 1.1],
                size: [0.55, 0.8],
                wind: 0.0,
                ..haze("bend", 0.45)
            },
            // A faint ripple swelling over the held thing.
            EmitterDesc {
                rate: 1.6,
                speed: [0.0, 0.0],
                local_space: true,
                lifetime: [0.7, 0.7],
                size: [1.5, 1.5],
                size_over_life: c(&[(0.0, 0.8), (1.0, 1.25)]),
                sprite: Sprite::Ring,
                color: g(&[
                    (0.0, [FORCE[0], FORCE[1], FORCE[2], 0.0]),
                    (0.3, [FORCE[0], FORCE[1], FORCE[2], 0.16]),
                    (1.0, [FORCE[0], FORCE[1], FORCE[2], 0.0]),
                ]),
                emissive: 2_500.0,
                additive: 1.0,
                soft: 0.0,
                wind: 0.0,
                ..em("ripple")
            },
            // Grit falling off the underside.
            EmitterDesc {
                rate: 16.0,
                bursts: Vec::new(),
                shape: EmitShape::Disc { radius: 0.4 },
                offset: [0.0, -0.55, 0.0],
                speed: [0.0, 0.15],
                lifetime: [0.7, 1.2],
                size: [0.012, 0.03],
                debris: None,
                ..chips("grit", 0, GRIT, [0.0, 0.2])
            },
            EmitterDesc {
                rate: 3.0,
                bursts: Vec::new(),
                shape: EmitShape::Disc { radius: 0.35 },
                offset: [0.0, -0.6, 0.0],
                velocity: [0.0, -0.35, 0.0],
                lifetime: [1.0, 1.6],
                ..dust_puff("dust", 0, DUST, 0.18, [0.0, 0.1], [0.25, 0.4])
            },
        ],
    )
}

/// A thrown thing in flight: wind streaking past it and a thin dust wake behind.
fn force_bolt() -> EffectDesc {
    effect(
        "spell.force.bolt",
        "A thing thrown by telekinesis, in flight: pale streaks of wind tearing past it, the air \
         bending round it and a thin dust wake behind. No glow, no rock lumps. Scale 1 is a \
         thing 0.65 m in radius.",
        None,
        vec![
            EmitterDesc {
                rate: 90.0,
                shape: EmitShape::Sphere {
                    radius: 0.75,
                    surface: true,
                },
                speed: [0.0, 0.0],
                inherit_velocity: 0.75,
                drag: 7.0,
                lifetime: [0.12, 0.22],
                size: [0.012, 0.022],
                sprite: Sprite::Spark,
                facing: Facing::Velocity,
                stretch: 0.035,
                color: g(&[
                    (0.0, [FORCE[0], FORCE[1], FORCE[2], 0.0]),
                    (0.3, [FORCE[0], FORCE[1], FORCE[2], 0.5]),
                    (1.0, [FORCE[0], FORCE[1], FORCE[2], 0.0]),
                ]),
                emissive: 1_800.0,
                additive: 1.0,
                soft: 0.0,
                wind: 0.0,
                ..em("wind")
            },
            EmitterDesc {
                rate: 6.0,
                shape: EmitShape::Sphere {
                    radius: 0.6,
                    surface: true,
                },
                speed: [0.0, 0.05],
                local_space: true,
                lifetime: [0.4, 0.6],
                size: [0.5, 0.7],
                wind: 0.0,
                ..haze("bend", 0.35)
            },
            EmitterDesc {
                rate_per_meter: 1.3,
                bursts: Vec::new(),
                shape: EmitShape::Sphere {
                    radius: 0.35,
                    surface: false,
                },
                lifetime: [0.8, 1.4],
                ..dust_puff("wake", 0, DUST, 0.22, [0.0, 0.2], [0.3, 0.5])
            },
            EmitterDesc {
                rate_per_meter: 4.0,
                bursts: Vec::new(),
                shape: EmitShape::Sphere {
                    radius: 0.4,
                    surface: false,
                },
                lifetime: [0.6, 1.1],
                size: [0.01, 0.025],
                debris: None,
                ..chips("grit", 0, GRIT, [0.0, 0.3])
            },
        ],
    )
}

/// A thrown thing landing: a dust burst, splinters and grit, and dust rolling out low.
fn force_impact() -> EffectDesc {
    effect(
        "spell.force.impact",
        "A thrown thing landing hard: a burst of dust, wood splinters and grit sprayed out, and \
         dust rolling out low along the floor. No flash. Scale 1 is a thing 0.65 m in radius \
         at 20 m/s.",
        Some(0.1),
        vec![
            EmitterDesc {
                shape: EmitShape::Hemisphere { radius: 0.4 },
                acceleration: [0.0, 0.2, 0.0],
                lifetime: [1.3, 2.2],
                ..dust_puff("dust", 14, DUST, 0.5, [1.2, 3.2], [0.5, 0.8])
            },
            EmitterDesc {
                shape: EmitShape::Hemisphere { radius: 0.3 },
                size: [0.06, 0.16],
                debris: Some(Debris::Splinter),
                ..chips("splinters", 22, WOOD, [2.5, 6.5])
            },
            EmitterDesc {
                shape: EmitShape::Hemisphere { radius: 0.3 },
                size: [0.02, 0.05],
                ..chips("grit", 40, GRIT, [2.0, 6.0])
            },
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.45,
                    thickness: 0.15,
                },
                offset: [0.0, 0.08, 0.0],
                drag: 3.0,
                lifetime: [1.0, 1.6],
                sprite: Sprite::Dust,
                ..dust_puff("skirt", 18, DUST, 0.38, [2.5, 4.0], [0.3, 0.45])
            },
        ],
    )
}

/// A chunk ripped out of the floor: chips and dust bursting up and out, clods of the bedding
/// thrown up, and a ring of dust along the floor.
fn stone_tear() -> EffectDesc {
    effect(
        "spell.stone.tear",
        "A rough chunk ripped out of the floor: stone chips and grit bursting up and out, a \
         column of dust thrown up after the chunk and a ring of dust rolling out along the \
         floor. Scale 1 is a 0.5 m chunk; up is the floor's normal.",
        Some(0.1),
        vec![
            EmitterDesc {
                shape: EmitShape::Cone {
                    angle_deg: 35.0,
                    radius: 0.3,
                },
                size: [0.03, 0.08],
                ..chips("chips", 40, GRIT, [2.5, 6.0])
            },
            EmitterDesc {
                shape: EmitShape::Cone {
                    angle_deg: 25.0,
                    radius: 0.25,
                },
                size: [0.1, 0.18],
                spin_deg: [-360.0, 360.0],
                lifetime: [2.0, 3.5],
                ..chips("chunks", 6, [0.36, 0.33, 0.29], [1.5, 3.5])
            },
            // Dust thrown up after the chunk.
            EmitterDesc {
                shape: EmitShape::Disc { radius: 0.3 },
                velocity: [0.0, 1.8, 0.0],
                acceleration: [0.0, 0.1, 0.0],
                lifetime: [1.2, 2.2],
                ..dust_puff("column", 12, DUST, 0.5, [0.2, 0.8], [0.4, 0.65])
            },
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.4,
                    thickness: 0.1,
                },
                offset: [0.0, 0.06, 0.0],
                drag: 3.0,
                lifetime: [1.0, 1.7],
                sprite: Sprite::Dust,
                ..dust_puff("ring", 20, DUST, 0.4, [2.2, 3.6], [0.3, 0.45])
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grip_is_grounded_and_never_a_glowing_ball() {
        for e in all() {
            e.validate().unwrap();
            assert!(e.light.is_none(), "{}: casts no light", e.name);
            for em in &e.emitters {
                assert!(
                    em.emissive <= 3_000.0,
                    "{}/{}: at most a faint shimmer",
                    e.name,
                    em.name
                );
            }
        }
        let aura = force_aura();
        assert!(
            aura.duration.is_none() && force_bolt().duration.is_none(),
            "held and flying loop"
        );
        assert!(force_impact().duration.is_some() && stone_tear().duration.is_some());
        // What is held stays visible: nothing in the aura is more than faint.
        for em in &aura.emitters {
            let peak = em.color.0.iter().map(|k| k.1[3]).fold(0.0, f32::max);
            assert!(
                peak <= 1.0 && (em.sprite != Sprite::Ring || peak < 0.2),
                "{}",
                em.name
            );
        }
    }
}
