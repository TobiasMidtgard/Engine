//! Auras around a whole floating body (magic round 5's elementals: a body about 1.3 m across,
//! hovering, the effect centered on it). Scale 1 wraps a 1.3 m body; the effect plays where
//! the carrier is, so it follows the body.

use super::*;

/// Radius of the body an aura wraps at scale 1, meters.
pub const AURA_RADIUS: f32 = 0.65;

pub(super) fn all() -> Vec<EffectDesc> {
    vec![fire_aura(), frost_aura(), storm_aura()]
}

fn shell(surface: bool) -> EmitShape {
    EmitShape::Sphere {
        radius: AURA_RADIUS,
        surface,
    }
}

/// A body of fire: tongues of flame licking up off its whole surface, a hot glow at its heart,
/// embers whirling off, heat shimmer above and a thin thread of smoke.
fn fire_aura() -> EffectDesc {
    EffectDesc {
        light: light([1.0, 0.55, 0.2], 6000.0, 8.0, 0.5, 0.4),
        ..effect(
            "spell.fire.aura",
            "A body wrapped in fire: flames licking up off its whole surface, a hot heart, \
             embers whirling off, heat shimmer and a thread of smoke. Scale 1 wraps a 1.3 m body.",
            None,
            vec![
                EmitterDesc {
                    rate: 150.0,
                    shape: shell(true),
                    speed: [0.0, 0.3],
                    acceleration: [0.0, 2.6, 0.0],
                    lifetime: [0.35, 0.6],
                    size: [0.38, 0.6],
                    size_over_life: c(&[(0.0, 0.5), (0.3, 1.0), (1.0, 0.5)]),
                    rotation_deg: [-12.0, 12.0],
                    sprite: Sprite::Flame,
                    color: flame_colors(),
                    emissive: 16_000.0,
                    additive: 1.0,
                    soft: 0.15,
                    wind: 0.4,
                    ..em("flames")
                },
                EmitterDesc {
                    rate: 30.0,
                    shape: EmitShape::Sphere {
                        radius: AURA_RADIUS * 0.6,
                        surface: false,
                    },
                    speed: [0.0, 0.2],
                    acceleration: [0.0, 0.8, 0.0],
                    lifetime: [0.4, 0.6],
                    size: [0.55, 0.8],
                    size_over_life: c(&[(0.0, 0.6), (0.4, 1.0), (1.0, 1.2)]),
                    rotation_deg: [0.0, 360.0],
                    spin_deg: [-90.0, 90.0],
                    sprite: Sprite::Fireball,
                    color: flame_colors(),
                    emissive: 14_000.0,
                    additive: 1.0,
                    soft: 0.1,
                    ..em("core")
                },
                EmitterDesc {
                    rate: 12.0,
                    shape: EmitShape::Point,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.3, 0.5],
                    size: [1.6, 2.0],
                    sprite: Sprite::Glow,
                    color: g(&[
                        (0.0, [1.0, 0.6, 0.25, 0.0]),
                        (0.3, [1.0, 0.55, 0.2, 0.3]),
                        (1.0, [1.0, 0.35, 0.08, 0.0]),
                    ]),
                    emissive: 6000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("heart")
                },
                EmitterDesc {
                    rate: 26.0,
                    shape: shell(true),
                    speed: [0.4, 1.2],
                    acceleration: [0.0, 2.2, 0.0],
                    turbulence: 3.0,
                    drag: 1.0,
                    lifetime: [0.8, 1.5],
                    size: [0.012, 0.025],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.03,
                    color: spark_colors(),
                    emissive: 22_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("embers")
                },
                EmitterDesc {
                    rate: 8.0,
                    shape: EmitShape::Sphere {
                        radius: AURA_RADIUS * 0.6,
                        surface: false,
                    },
                    offset: [0.0, AURA_RADIUS, 0.0],
                    speed: [0.2, 0.5],
                    acceleration: [0.0, 1.5, 0.0],
                    lifetime: [0.6, 1.0],
                    size: [0.6, 0.9],
                    ..haze("shimmer", 0.6)
                },
                EmitterDesc {
                    rate: 2.0,
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    offset: [0.0, AURA_RADIUS, 0.0],
                    speed: [0.1, 0.3],
                    acceleration: [0.0, 1.2, 0.0],
                    lifetime: [1.5, 2.4],
                    size: [0.35, 0.5],
                    ..smoke("smoke", 0.2, 0.18)
                },
            ],
        )
    }
}

/// A body of frost: cold mist swirling round it and sinking off its underside, ice glitter on
/// its skin and snow falling away below.
fn frost_aura() -> EffectDesc {
    EffectDesc {
        light: light([0.55, 0.78, 1.0], 1500.0, 5.0, 0.0, 0.4),
        ..effect(
            "spell.frost.aura",
            "A body of frost: cold mist swirling round it and sinking off its underside, ice \
             glitter on its skin and snow falling away. Scale 1 wraps a 1.3 m body.",
            None,
            vec![
                EmitterDesc {
                    rate: 22.0,
                    shape: EmitShape::Sphere {
                        radius: AURA_RADIUS * 0.8,
                        surface: false,
                    },
                    speed: [0.0, 0.05],
                    velocity: [0.0, -0.15, 0.0],
                    drag: 1.5,
                    lifetime: [0.9, 1.4],
                    size: [0.4, 0.6],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.4)]),
                    sprite: Sprite::Dust,
                    lit: 1.0,
                    emissive: 400.0,
                    color: fade([0.82, 0.9, 1.0], 0.4, 0.15, 0.6),
                    soft: 0.3,
                    wind: 0.1,
                    ..em("mist")
                },
                EmitterDesc {
                    rate: 10.0,
                    shape: EmitShape::Point,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.4, 0.6],
                    size: [1.4, 1.8],
                    sprite: Sprite::Glow,
                    color: fade([0.6, 0.8, 1.0], 0.25, 0.3, 0.6),
                    emissive: 3000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("chill")
                },
                EmitterDesc {
                    rate: 40.0,
                    shape: shell(true),
                    speed: [0.0, 0.03],
                    local_space: true,
                    lifetime: [0.4, 0.9],
                    size: [0.025, 0.05],
                    sprite: Sprite::Star,
                    color: fade([0.8, 0.92, 1.0], 1.0, 0.3, 0.6),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("glitter")
                },
                EmitterDesc {
                    rate: 20.0,
                    shape: EmitShape::Disc {
                        radius: AURA_RADIUS * 0.8,
                    },
                    offset: [0.0, -AURA_RADIUS * 0.6, 0.0],
                    speed: [0.0, 0.05],
                    velocity: [0.0, -0.6, 0.0],
                    turbulence: 0.6,
                    lifetime: [1.5, 2.5],
                    size: [0.02, 0.035],
                    sprite: Sprite::Star,
                    color: fade([0.85, 0.95, 1.0], 1.0, 0.15, 0.7),
                    emissive: 5000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("snow")
                },
            ],
        )
    }
}

/// A body of storm: arcs crawling over it, a flickering charged core, sparks spitting off and
/// a dark churning cloud wrapped round it.
fn storm_aura() -> EffectDesc {
    EffectDesc {
        light: light([0.62, 0.74, 1.0], 9000.0, 9.0, 0.9, 0.2),
        ..effect(
            "spell.storm.aura",
            "A body of storm: lightning arcs crawling over it, a flickering charged core, sparks \
             spitting off and a dark churning cloud round it. Scale 1 wraps a 1.3 m body.",
            None,
            vec![
                EmitterDesc {
                    rate: 70.0,
                    shape: shell(true),
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.08, 0.16],
                    size: [0.9, 1.4],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Arc,
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.55, 0.65, 1.0, 0.0])]),
                    emissive: 50_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("arcs")
                },
                EmitterDesc {
                    rate: 14.0,
                    shape: EmitShape::Sphere {
                        radius: AURA_RADIUS * 0.3,
                        surface: false,
                    },
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.1, 0.25],
                    size: [0.8, 1.1],
                    sprite: Sprite::Plasma,
                    color: g(&[(0.0, [0.75, 0.85, 1.0, 0.9]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 25_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("core")
                },
                EmitterDesc {
                    rate: 25.0,
                    shape: shell(true),
                    speed: [1.0, 3.0],
                    gravity: 0.6,
                    drag: 0.8,
                    lifetime: [0.2, 0.5],
                    size: [0.01, 0.02],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.02,
                    color: g(&[(0.0, [0.85, 0.92, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("sparks")
                },
                EmitterDesc {
                    rate: 10.0,
                    shape: shell(false),
                    speed: [0.0, 0.05],
                    drag: 1.5,
                    lifetime: [0.8, 1.2],
                    size: [0.5, 0.7],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.3)]),
                    wind: 0.1,
                    ..smoke("cloud", 0.3, 0.25)
                },
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elementals_have_a_body_sized_aura_that_loops() {
        for e in all() {
            assert!(
                e.duration.is_none(),
                "{}: an aura plays while it is worn",
                e.name
            );
            assert!(
                e.mark.is_none(),
                "{}: a floating body leaves no mark",
                e.name
            );
            for em in &e.emitters {
                if let EmitShape::Sphere { radius, .. } = em.shape {
                    assert!(radius <= AURA_RADIUS, "{}/{}", e.name, em.name);
                }
            }
        }
    }
}
