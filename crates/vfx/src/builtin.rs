//! Built-in effects. `vfx.effects` lists them and `vfx.describe` prints any of them as JSON,
//! which is the easiest starting point for a new effect.
//!
//! Naming: `spell.*` magic, `water.*` splashes and fountains, `impact.<surface>` hits on a
//! surface material (the names `SurfaceMaterial::impact_effect` uses), `step.<surface>`
//! footstep kicks, `impulse.*` ground shockwaves, `exhaust.*` engine flames and fumes, `fire.*`,
//! `ambient.*`, `magic.*`.
//!
//! Glow is in cd/m²: daylight scenes expose for roughly 5 000–100 000 cd/m², so cores of
//! 40 000–100 000 read as white-hot by day and bloom at night.

use crate::effect::*;
use crate::marks::{MarkDesc, MarkKind};
use crate::trail::{RibbonStyle, Trail};

fn em(name: &str) -> EmitterDesc {
    EmitterDesc {
        name: name.into(),
        ..Default::default()
    }
}

fn g(keys: &[(f32, [f32; 4])]) -> Gradient {
    Gradient::new(keys)
}

fn c(keys: &[(f32, f32)]) -> Curve {
    Curve::new(keys)
}

fn once(count: u32) -> Vec<Burst> {
    vec![Burst {
        count,
        ..Default::default()
    }]
}

fn at(time: f32, count: u32) -> Burst {
    Burst {
        time,
        count,
        ..Default::default()
    }
}

fn every(period: f32, count: u32) -> Vec<Burst> {
    vec![Burst {
        time: 0.0,
        count,
        repeat: period,
        cycles: 0,
    }]
}

fn effect(
    name: &str,
    description: &str,
    duration: Option<f32>,
    emitters: Vec<EmitterDesc>,
) -> EffectDesc {
    EffectDesc {
        name: name.into(),
        description: description.into(),
        duration,
        emitters,
        ..Default::default()
    }
}

/// The mark an effect leaves where it starts: `radius` m at scale 1, lasting `life` s, with
/// embers glowing for `glow` s (scorches).
fn mark(kind: MarkKind, radius: f32, life: f32, glow: f32) -> Option<MarkDesc> {
    Some(MarkDesc {
        kind,
        radius,
        life_secs: life,
        strength: 1.0,
        glow_secs: glow,
    })
}

fn light(color: [f32; 3], lumens: f32, range: f32, flicker: f32, fade: f32) -> Option<EffectLight> {
    Some(EffectLight {
        color,
        intensity_lm: lumens,
        range,
        flicker,
        offset: [0.0; 3],
        fade_secs: fade,
    })
}

/// A light `offset` m out from the effect along its local axes.
fn light_at(
    color: [f32; 3],
    lumens: f32,
    range: f32,
    flicker: f32,
    offset: [f32; 3],
) -> Option<EffectLight> {
    light(color, lumens, range, flicker, 0.15).map(|l| EffectLight { offset, ..l })
}

/// Fade in quickly, hold, fade out: `[t, alpha]` shape applied to one color.
fn fade(rgb: [f32; 3], peak: f32, fade_in: f32, fade_out_start: f32) -> Gradient {
    let [r, gg, b] = rgb;
    g(&[
        (0.0, [r, gg, b, 0.0]),
        (fade_in, [r, gg, b, peak]),
        (fade_out_start, [r, gg, b, peak]),
        (1.0, [r, gg, b, 0.0]),
    ])
}

const FIRE_CORE: [f32; 4] = [1.0, 0.86, 0.55, 1.0];

fn flame_colors() -> Gradient {
    g(&[
        (0.0, [1.0, 0.75, 0.35, 0.0]),
        (0.08, [1.0, 0.68, 0.28, 1.0]),
        (0.4, [1.0, 0.36, 0.06, 0.85]),
        (1.0, [0.5, 0.07, 0.01, 0.0]),
    ])
}

fn spark_colors() -> Gradient {
    g(&[
        (0.0, [1.0, 0.74, 0.34, 1.0]),
        (0.5, [1.0, 0.44, 0.1, 1.0]),
        (1.0, [0.75, 0.14, 0.03, 0.0]),
    ])
}

fn smoke_colors(albedo: f32, peak: f32) -> Gradient {
    let a = albedo;
    g(&[
        (0.0, [a, a * 0.95, a * 0.9, 0.0]),
        (0.1, [a, a * 0.95, a * 0.9, peak]),
        (1.0, [a * 1.4, a * 1.38, a * 1.35, 0.0]),
    ])
}

fn smoke(name: &str, albedo: f32, peak: f32) -> EmitterDesc {
    EmitterDesc {
        sprite: Sprite::Smoke,
        lit: 1.0,
        color: smoke_colors(albedo, peak),
        spin_deg: [-25.0, 25.0],
        drag: 1.5,
        turbulence: 0.6,
        turbulence_scale: 1.5,
        size_over_life: c(&[(0.0, 0.6), (1.0, 2.6)]),
        soft: 0.5,
        ..em(name)
    }
}

fn sparks(name: &str, count: u32, speed: [f32; 2]) -> EmitterDesc {
    EmitterDesc {
        bursts: once(count),
        shape: EmitShape::Hemisphere { radius: 0.05 },
        speed,
        gravity: 1.0,
        drag: 0.5,
        collide: Collide::Bounce {
            restitution: 0.4,
            friction: 0.25,
        },
        lifetime: [0.4, 1.1],
        size: [0.015, 0.03],
        sprite: Sprite::Spark,
        facing: Facing::Velocity,
        stretch: 0.03,
        color: spark_colors(),
        emissive: 25_000.0,
        additive: 1.0,
        soft: 0.0,
        ..em(name)
    }
}

/// Heat haze rising off something hot: bends the scene behind it. `strength` 0..1.
fn haze(name: &str, strength: f32) -> EmitterDesc {
    EmitterDesc {
        sprite: Sprite::Haze,
        additive: 1.0,
        lit: 0.0,
        emissive: 0.0,
        soft: 0.0,
        wind: 0.6,
        turbulence: 1.0,
        turbulence_scale: 0.8,
        spin_deg: [-30.0, 30.0],
        color: g(&[
            (0.0, [1.0, 1.0, 1.0, 0.0]),
            (0.25, [1.0, 1.0, 1.0, strength]),
            (1.0, [1.0, 1.0, 1.0, 0.0]),
        ]),
        ..em(name)
    }
}

/// Solid stone chips that bounce, settle and crumble away. Set `debris` to another kind (or
/// `None` for flat sprite flakes such as leaves and grains).
fn chips(name: &str, count: u32, albedo: [f32; 3], speed: [f32; 2]) -> EmitterDesc {
    let [r, gg, b] = albedo;
    EmitterDesc {
        bursts: once(count),
        shape: EmitShape::Hemisphere { radius: 0.05 },
        speed,
        gravity: 1.0,
        drag: 0.3,
        collide: Collide::Bounce {
            restitution: 0.3,
            friction: 0.6,
        },
        lifetime: [1.6, 3.2],
        size: [0.03, 0.09],
        spin_deg: [-720.0, 720.0],
        sprite: Sprite::Shard,
        debris: Some(Debris::Rock),
        lit: 1.0,
        color: g(&[
            (0.0, [r, gg, b, 1.0]),
            (0.8, [r, gg, b, 1.0]),
            (1.0, [r, gg, b, 0.0]),
        ]),
        soft: 0.0,
        ..em(name)
    }
}

fn dust_puff(
    name: &str,
    count: u32,
    albedo: [f32; 3],
    peak: f32,
    speed: [f32; 2],
    size: [f32; 2],
) -> EmitterDesc {
    let [r, gg, b] = albedo;
    EmitterDesc {
        bursts: once(count),
        shape: EmitShape::Hemisphere { radius: 0.15 },
        speed,
        drag: 3.0,
        gravity: 0.03,
        lifetime: [0.8, 1.6],
        size,
        size_over_life: c(&[(0.0, 0.5), (1.0, 2.4)]),
        sprite: Sprite::Smoke,
        lit: 1.0,
        color: g(&[
            (0.0, [r, gg, b, 0.0]),
            (0.08, [r, gg, b, peak]),
            (1.0, [r * 1.1, gg * 1.1, b * 1.1, 0.0]),
        ]),
        spin_deg: [-40.0, 40.0],
        turbulence: 0.4,
        soft: 0.15,
        ..em(name)
    }
}

fn droplets(
    name: &str,
    count: u32,
    rgb: [f32; 3],
    speed: [f32; 2],
    up: f32,
    collide: Collide,
) -> EmitterDesc {
    let [r, gg, b] = rgb;
    EmitterDesc {
        bursts: once(count),
        shape: EmitShape::Ring {
            radius: 0.2,
            thickness: 0.1,
        },
        speed,
        velocity: [0.0, up, 0.0],
        gravity: 1.0,
        drag: 0.2,
        wind: 0.3,
        collide,
        lifetime: [0.6, 1.2],
        size: [0.035, 0.07],
        sprite: Sprite::Droplet,
        facing: Facing::Velocity,
        stretch: 0.012,
        lit: 1.0,
        color: g(&[(0.0, [r, gg, b, 0.9]), (1.0, [r, gg, b, 0.75])]),
        soft: 0.05,
        ..em(name)
    }
}

fn ripples(name: &str, times: &[f32], grow: f32, life: f32, alpha: f32) -> EmitterDesc {
    EmitterDesc {
        bursts: times.iter().map(|t| at(*t, 1)).collect(),
        offset: [0.0, 0.02, 0.0],
        speed: [0.0, 0.0],
        lifetime: [life, life],
        size: [0.5, 0.5],
        size_over_life: c(&[(0.0, 0.5), (1.0, grow)]),
        sprite: Sprite::Ripple,
        facing: Facing::Horizontal,
        // Riding the surface: not held up or let down by the air.
        local_space: true,
        rotation_deg: [0.0, 0.0],
        lit: 1.0,
        color: g(&[
            (0.0, [0.9, 0.95, 1.0, alpha]),
            (1.0, [0.9, 0.95, 1.0, alpha]),
        ]),
        soft: 0.0,
        wind: 0.0,
        ..em(name)
    }
}

/// The crown of a splash, `span` m across at its widest, lasting `life` s (see [`Sprite::Crown`]).
fn crown(span: f32, life: f32) -> EmitterDesc {
    EmitterDesc {
        bursts: once(1),
        speed: [0.0, 0.0],
        gravity: 0.0,
        lifetime: [life, life],
        size: [span, span],
        sprite: Sprite::Crown,
        lit: 1.0,
        color: g(&[(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])]),
        soft: 0.0,
        wind: 0.0,
        ..em("crown_wall")
    }
}

/// `count` patches of foam spreading over a disc of `radius` m and lasting about `life` s.
fn foam(count: u32, radius: f32, size: [f32; 2], life: f32) -> EmitterDesc {
    EmitterDesc {
        bursts: once(count),
        shape: EmitShape::Disc { radius },
        offset: [0.0, 0.025, 0.0],
        speed: [0.2, 0.7],
        drag: 1.4,
        gravity: 0.0,
        lifetime: [life * 0.7, life],
        size,
        size_over_life: c(&[(0.0, 0.6), (0.3, 1.0), (1.0, 1.5)]),
        sprite: Sprite::Foam,
        facing: Facing::Horizontal,
        local_space: true,
        rotation_deg: [0.0, 360.0],
        lit: 1.0,
        color: fade([0.95, 0.97, 1.0], 0.9, 0.05, 0.75),
        soft: 0.0,
        wind: 0.0,
        ..em("foam")
    }
}

/// Every built-in effect.
pub fn all() -> Vec<EffectDesc> {
    let mut v = Vec::new();
    v.extend(spells());
    v.extend(water());
    v.extend(impacts());
    v.extend(steps());
    v.extend(exhausts());
    v.extend(misc());
    v
}

fn spells() -> Vec<EffectDesc> {
    let fireball = EffectDesc {
        light: light([1.0, 0.55, 0.2], 12_000.0, 14.0, 0.35, 0.3),
        ..effect(
            "spell.fireball",
            "A flying fireball: a roiling ball of fire round a white-hot heart, a tail of burning \
             gas cooling to soot behind it, sparks and a warm light. Move its entity to \
             fly it.",
            None,
            vec![
                // The ball itself: rolling fire round the heart, a few overlapping copies
                // turning and crossfading, gone before they cool.
                EmitterDesc {
                    rate: 16.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.3, 0.3],
                    size: [0.6, 0.6],
                    rotation_deg: [-180.0, 180.0],
                    spin_deg: [-90.0, 90.0],
                    sprite: Sprite::Fireball,
                    lit: 1.0,
                    color: fade([1.0, 0.5, 0.15], 1.0, 0.1, 0.2),
                    emissive: 18_000.0,
                    additive: 0.0,
                    soft: 0.2,
                    wind: 0.0,
                    ..em("ball")
                },
                // The white-hot heart, drawn over the ball so it shows through.
                EmitterDesc {
                    rate: 90.0,
                    shape: EmitShape::Sphere {
                        radius: 0.08,
                        surface: false,
                    },
                    speed: [0.0, 0.2],
                    lifetime: [0.12, 0.22],
                    size: [0.45, 0.6],
                    size_over_life: c(&[(0.0, 1.0), (1.0, 0.6)]),
                    local_space: true,
                    color: g(&[(0.0, FIRE_CORE), (1.0, [1.0, 0.45, 0.1, 0.0])]),
                    emissive: 21_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    soft: 0.1,
                    ..em("core")
                },
                // Burning gas shed behind it in flight, swelling, rising and cooling to soot,
                // so the tail runs from fire at the ball to smoke further back.
                EmitterDesc {
                    rate: 20.0,
                    rate_per_meter: 12.0,
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    speed: [0.3, 1.2],
                    drag: 2.0,
                    acceleration: [0.0, 1.5, 0.0],
                    turbulence: 3.0,
                    turbulence_scale: 0.5,
                    wind: 0.5,
                    lifetime: [0.35, 0.8],
                    size: [0.25, 0.6],
                    size_over_life: c(&[(0.0, 0.7), (0.3, 1.0), (1.0, 2.0)]),
                    rotation_deg: [-180.0, 180.0],
                    spin_deg: [-60.0, 60.0],
                    sprite: Sprite::Fireball,
                    lit: 1.0,
                    color: fade([1.0, 0.45, 0.12], 1.0, 0.03, 0.4),
                    emissive: 18_000.0,
                    additive: 0.0,
                    soft: 0.3,
                    ..em("tail")
                },
                EmitterDesc {
                    rate: 35.0,
                    rate_per_meter: 6.0,
                    shape: EmitShape::Sphere {
                        radius: 0.15,
                        surface: true,
                    },
                    speed: [1.0, 3.5],
                    lifetime: [0.4, 0.9],
                    size: [0.03, 0.05],
                    gravity: 0.6,
                    drag: 0.8,
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.06,
                    color: spark_colors(),
                    emissive: 25_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("sparks")
                },
                EmitterDesc {
                    rate: 18.0,
                    rate_per_meter: 5.0,
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    speed: [0.1, 0.4],
                    lifetime: [1.2, 2.2],
                    size: [0.35, 0.6],
                    acceleration: [0.0, 0.7, 0.0],
                    ..smoke("smoke", 0.3, 0.35)
                },
                // No heat haze: drawn after the fire it would paint the scene behind back over
                // the burning tail, a band of horizon cut through it.
            ],
        )
    };

    let fireball_impact = EffectDesc {
        light: light([1.0, 0.55, 0.25], 45_000.0, 18.0, 0.2, 0.7),
        mark: mark(MarkKind::Scorch, 1.1, 120.0, 8.0),
        ..effect(
            "spell.fireball_impact",
            "A fireball exploding: flash, fire burst, bouncing sparks, a smoke cloud, a ground \
             shockwave and floating embers.",
            Some(0.15),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.22, 0.22],
                    size: [2.5, 2.5],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.8)]),
                    color: g(&[(0.0, [1.0, 0.9, 0.7, 1.0]), (1.0, [1.0, 0.4, 0.1, 0.0])]),
                    emissive: 45_000.0,
                    additive: 1.0,
                    ..em("flash")
                },
                EmitterDesc {
                    bursts: once(12),
                    shape: EmitShape::Sphere {
                        radius: 0.3,
                        surface: false,
                    },
                    speed: [2.5, 6.0],
                    drag: 3.5,
                    acceleration: [0.0, 3.0, 0.0],
                    turbulence: 3.0,
                    lifetime: [1.0, 1.6],
                    size: [0.9, 1.4],
                    size_over_life: c(&[(0.0, 0.4), (0.25, 1.0), (1.0, 1.5)]),
                    rotation_deg: [-180.0, 180.0],
                    spin_deg: [-30.0, 30.0],
                    sprite: Sprite::Fireball,
                    lit: 1.0,
                    color: fade([1.0, 0.45, 0.12], 1.0, 0.03, 0.75),
                    emissive: 18_000.0,
                    additive: 0.0,
                    soft: 0.6,
                    ..em("fire")
                },
                EmitterDesc {
                    lifetime: [0.6, 1.5],
                    stretch: 0.035,
                    emissive: 30_000.0,
                    ..sparks("sparks", 130, [5.0, 13.0])
                },
                // Soot, rising as the fire burns through (the effect stops emitting at 0.15 s).
                EmitterDesc {
                    delay: 0.12,
                    bursts: once(28),
                    shape: EmitShape::Hemisphere { radius: 0.6 },
                    speed: [0.8, 2.2],
                    drag: 1.2,
                    acceleration: [0.0, 1.1, 0.0],
                    lifetime: [2.5, 4.5],
                    size: [1.0, 1.6],
                    size_over_life: c(&[(0.0, 0.7), (1.0, 2.8)]),
                    // Soot: dark enough that moonlight scattered through it from behind
                    // doesn't turn it into a pale cloud at night.
                    color: g(&[
                        (0.0, [0.1, 0.085, 0.075, 0.0]),
                        (0.08, [0.1, 0.09, 0.08, 0.7]),
                        (1.0, [0.2, 0.19, 0.18, 0.0]),
                    ]),
                    ..smoke("smoke", 0.15, 0.7)
                },
                EmitterDesc {
                    bursts: once(1),
                    offset: [0.0, 0.05, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.45, 0.45],
                    size: [1.0, 1.0],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 9.0)]),
                    rotation_deg: [0.0, 0.0],
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[(0.0, [1.0, 0.6, 0.25, 0.9]), (1.0, [1.0, 0.3, 0.05, 0.0])]),
                    emissive: 15_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("shockwave")
                },
                EmitterDesc {
                    bursts: once(45),
                    shape: EmitShape::Sphere {
                        radius: 0.5,
                        surface: false,
                    },
                    speed: [1.5, 4.5],
                    gravity: 0.1,
                    acceleration: [0.0, 1.2, 0.0],
                    drag: 1.2,
                    turbulence: 3.0,
                    turbulence_scale: 1.2,
                    lifetime: [1.5, 3.0],
                    size: [0.03, 0.06],
                    color: g(&[
                        (0.0, [1.0, 0.6, 0.2, 1.0]),
                        (0.7, [1.0, 0.35, 0.05, 0.9]),
                        (1.0, [0.8, 0.1, 0.0, 0.0]),
                    ]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    ..em("embers")
                },
                EmitterDesc {
                    bursts: once(14),
                    shape: EmitShape::Sphere {
                        radius: 0.6,
                        surface: false,
                    },
                    speed: [0.3, 1.0],
                    acceleration: [0.0, 1.5, 0.0],
                    drag: 1.0,
                    lifetime: [0.7, 1.2],
                    size: [1.4, 2.2],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.5)]),
                    ..haze("haze", 1.0)
                },
            ],
        )
    };

    let frost_bolt = EffectDesc {
        light: light([0.5, 0.75, 1.0], 7000.0, 12.0, 0.1, 0.3),
        trail: Some(Trail {
            width: 0.12,
            lifetime: 0.25,
            min_spacing: 0.1,
            color: g(&[(0.0, [0.7, 0.9, 1.0, 0.8]), (1.0, [0.4, 0.7, 1.0, 0.0])]),
            emissive: 8000.0,
            style: RibbonStyle::Smooth,
            ..Default::default()
        }),
        ..effect(
            "spell.frost_bolt",
            "An ice bolt: a crackling ball of cold light, glinting frost, tumbling ice shards and \
             a thin cold mist.",
            None,
            vec![
                EmitterDesc {
                    rate: 80.0,
                    lifetime: [0.12, 0.2],
                    size: [0.2, 0.28],
                    speed: [0.0, 0.1],
                    local_space: true,
                    color: g(&[(0.0, [0.7, 0.88, 1.0, 1.0]), (1.0, [0.4, 0.7, 1.0, 0.0])]),
                    emissive: 14_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    ..em("core")
                },
                EmitterDesc {
                    rate: 3.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.6, 0.6],
                    size: [0.75, 0.75],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Plasma,
                    color: fade([0.55, 0.8, 1.0], 1.0, 0.3, 0.7),
                    emissive: 5_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("ball")
                },
                EmitterDesc {
                    rate: 20.0,
                    rate_per_meter: 4.0,
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    speed: [0.2, 1.0],
                    drag: 2.0,
                    lifetime: [0.2, 0.4],
                    size: [0.05, 0.1],
                    spin_deg: [-200.0, 200.0],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [0.8, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.8, 1.0, 0.0])]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    ..em("glints")
                },
                EmitterDesc {
                    rate: 25.0,
                    rate_per_meter: 6.0,
                    shape: EmitShape::Sphere {
                        radius: 0.15,
                        surface: false,
                    },
                    speed: [0.5, 1.5],
                    gravity: 0.5,
                    lifetime: [0.4, 0.8],
                    size: [0.05, 0.12],
                    spin_deg: [-360.0, 360.0],
                    sprite: Sprite::Shard,
                    debris: Some(Debris::Shard),
                    lit: 1.0,
                    emissive: 1500.0,
                    color: g(&[
                        (0.0, [0.75, 0.9, 1.0, 1.0]),
                        (0.7, [0.65, 0.87, 1.0, 1.0]),
                        (1.0, [0.6, 0.85, 1.0, 0.0]),
                    ]),
                    ..em("shards")
                },
                // Many faint overlapping puffs, so the cold mist trails as one wisp rather
                // than a string of separate balls.
                EmitterDesc {
                    rate: 30.0,
                    rate_per_meter: 16.0,
                    shape: EmitShape::Sphere {
                        radius: 0.12,
                        surface: false,
                    },
                    speed: [0.1, 0.3],
                    lifetime: [0.8, 1.4],
                    size: [0.35, 0.55],
                    emissive: 30.0,
                    color: fade([0.85, 0.93, 1.0], 0.05, 0.15, 0.35),
                    ..smoke("mist", 0.85, 0.35)
                },
            ],
        )
    };

    let frost_impact = EffectDesc {
        light: light([0.55, 0.8, 1.0], 20_000.0, 14.0, 0.1, 0.5),
        mark: mark(MarkKind::Frost, 1.3, 30.0, 0.0),
        ..effect(
            "spell.frost_impact",
            "An ice bolt shattering: flash, ice spikes thrust out where it struck, a spray of \
             bouncing ice shards, glints, a frost ring and a rolling cold mist.",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.2, 0.2],
                    size: [2.0, 2.0],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.6)]),
                    color: g(&[(0.0, [0.6, 0.85, 1.0, 1.0]), (1.0, [0.25, 0.55, 1.0, 0.0])]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    ..em("flash")
                },
                EmitterDesc {
                    bursts: once(60),
                    shape: EmitShape::Hemisphere { radius: 0.3 },
                    speed: [3.0, 8.0],
                    lifetime: [1.2, 2.4],
                    size: [0.05, 0.17],
                    collide: Collide::Bounce {
                        restitution: 0.25,
                        friction: 0.5,
                    },
                    debris: Some(Debris::Shard),
                    emissive: 3000.0,
                    color: g(&[
                        (0.0, [0.62, 0.85, 1.0, 1.0]),
                        (0.8, [0.62, 0.85, 1.0, 1.0]),
                        (1.0, [0.62, 0.85, 1.0, 0.0]),
                    ]),
                    ..chips("shards", 60, [0.8, 0.93, 1.0], [3.0, 8.0])
                },
                EmitterDesc {
                    bursts: once(40),
                    shape: EmitShape::Hemisphere { radius: 0.5 },
                    speed: [1.0, 4.0],
                    drag: 2.0,
                    lifetime: [0.4, 1.0],
                    size: [0.12, 0.3],
                    spin_deg: [-200.0, 200.0],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [0.85, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.8, 1.0, 0.0])]),
                    emissive: 35_000.0,
                    additive: 1.0,
                    ..em("glints")
                },
                // A thin cold mist rolling out low along the ground.
                EmitterDesc {
                    bursts: once(16),
                    shape: EmitShape::Ring {
                        radius: 0.3,
                        thickness: 0.2,
                    },
                    speed: [1.8, 3.5],
                    drag: 2.0,
                    gravity: 0.02,
                    turbulence: 0.3,
                    lifetime: [1.4, 2.4],
                    size: [0.5, 0.8],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.6)]),
                    emissive: 60.0,
                    ..smoke("mist", 0.75, 0.22)
                },
                EmitterDesc {
                    bursts: once(1),
                    offset: [0.0, 0.04, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.6, 0.6],
                    size: [1.0, 1.0],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 5.0)]),
                    rotation_deg: [0.0, 0.0],
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[(0.0, [0.6, 0.85, 1.0, 1.0]), (1.0, [0.5, 0.8, 1.0, 0.0])]),
                    emissive: 12_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("frost_ring")
                },
                // Ice spikes thrust out where it struck: one big cluster and a few small ones
                // around it, cracking and thinning away after a couple of seconds.
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [2.4, 2.4],
                    size: [1.1, 1.1],
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    sprite: Sprite::Crystal,
                    facing: Facing::Axis,
                    stretch: 1.0,
                    lit: 1.0,
                    color: Gradient::constant([0.75, 0.9, 1.0, 1.0]),
                    emissive: 1500.0,
                    soft: 0.1,
                    wind: 0.0,
                    ..em("spikes")
                },
                EmitterDesc {
                    bursts: once(4),
                    shape: EmitShape::Ring {
                        radius: 0.45,
                        thickness: 0.2,
                    },
                    speed: [0.0, 0.0],
                    lifetime: [1.6, 2.2],
                    size: [0.35, 0.6],
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    sprite: Sprite::Crystal,
                    facing: Facing::Axis,
                    stretch: 1.0,
                    lit: 1.0,
                    color: Gradient::constant([0.75, 0.9, 1.0, 1.0]),
                    emissive: 1500.0,
                    soft: 0.1,
                    wind: 0.0,
                    ..em("small_spikes")
                },
            ],
        )
    };

    let violet = [0.7, 0.42, 1.0];
    let portal = EffectDesc {
        prewarm: 3.0,
        light: Some(EffectLight {
            offset: [0.0, 1.0, 0.0],
            ..light([0.7, 0.45, 1.0], 5000.0, 14.0, 0.15, 0.6).unwrap()
        }),
        ..effect(
            "spell.arcane_portal",
            "A swirling arcane portal: two counter-rotating rune circles on the ground, motes \
             spiraling inward and up, rising stars and a violet glow.",
            None,
            vec![
                EmitterDesc {
                    bursts: every(30.0, 1),
                    offset: [0.0, 0.05, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [30.0, 30.0],
                    size: [6.0, 6.0],
                    rotation_deg: [0.0, 0.0],
                    spin_deg: [20.0, 20.0],
                    local_space: true,
                    sprite: Sprite::Rune,
                    facing: Facing::Horizontal,
                    color: fade(violet, 1.0, 0.02, 0.97),
                    emissive: 14_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("circle")
                },
                EmitterDesc {
                    bursts: every(30.0, 1),
                    offset: [0.0, 0.07, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [30.0, 30.0],
                    size: [3.2, 3.2],
                    rotation_deg: [0.0, 0.0],
                    spin_deg: [-35.0, -35.0],
                    local_space: true,
                    sprite: Sprite::Rune,
                    facing: Facing::Horizontal,
                    color: fade([0.95, 0.55, 1.0], 1.0, 0.02, 0.97),
                    emissive: 20_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("inner_circle")
                },
                EmitterDesc {
                    rate: 260.0,
                    shape: EmitShape::Ring {
                        radius: 2.6,
                        thickness: 0.4,
                    },
                    speed: [0.0, 0.2],
                    velocity: [0.0, 0.6, 0.0],
                    orbit_speed: 3.5,
                    vortex: 3.0,
                    attract: 2.2,
                    drag: 0.6,
                    wind: 0.0,
                    lifetime: [1.5, 2.5],
                    size: [0.05, 0.1],
                    color: g(&[
                        (0.0, [0.6, 0.35, 1.0, 0.0]),
                        (0.2, [0.75, 0.45, 1.0, 1.0]),
                        (1.0, [1.0, 0.5, 0.9, 0.0]),
                    ]),
                    emissive: 25_000.0,
                    additive: 1.0,
                    ..em("swirl")
                },
                EmitterDesc {
                    rate: 50.0,
                    shape: EmitShape::Disc { radius: 0.8 },
                    speed: [0.0, 0.3],
                    velocity: [0.0, 2.5, 0.0],
                    drag: 0.3,
                    wind: 0.2,
                    lifetime: [1.0, 2.0],
                    size: [0.08, 0.16],
                    spin_deg: [-90.0, 90.0],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [0.9, 0.75, 1.0, 1.0]), (1.0, [0.6, 0.3, 1.0, 0.0])]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    ..em("column")
                },
                EmitterDesc {
                    rate: 30.0,
                    offset: [0.0, 1.2, 0.0],
                    shape: EmitShape::Sphere {
                        radius: 0.1,
                        surface: false,
                    },
                    speed: [0.0, 0.1],
                    lifetime: [0.3, 0.5],
                    size: [1.2, 1.8],
                    local_space: true,
                    color: g(&[(0.0, [0.8, 0.5, 1.0, 0.5]), (1.0, [0.5, 0.2, 1.0, 0.0])]),
                    emissive: 3_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    ..em("core")
                },
            ],
        )
    };

    let gate = EffectDesc {
        prewarm: 3.0,
        light: Some(EffectLight {
            offset: [0.0, 0.6, 0.0],
            ..light(violet, 3_000.0, 12.0, 0.12, 0.8).unwrap()
        }),
        ..effect(
            "spell.portal_gate",
            "A standing portal 5 m across: the world behind it swirled into a violet vortex that \
             darkens toward its eye, glowing arms turning inward, a burning rim crackling with \
             sparks, motes drawn in and stars spilling out. Turn the effect so its up axis faces \
             out of the portal.",
            None,
            vec![
                EmitterDesc {
                    bursts: every(30.0, 1),
                    speed: [0.0, 0.0],
                    lifetime: [30.0, 30.0],
                    size: [4.9, 4.9],
                    rotation_deg: [0.0, 0.0],
                    spin_deg: [-14.0, -14.0],
                    local_space: true,
                    sprite: Sprite::Vortex,
                    facing: Facing::Horizontal,
                    color: fade(violet, 1.0, 0.03, 0.97),
                    emissive: 2_500.0,
                    additive: 0.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("vortex")
                },
                // Energy crackling around the rim.
                EmitterDesc {
                    rate: 260.0,
                    shape: EmitShape::Ring {
                        radius: 2.3,
                        thickness: 0.08,
                    },
                    speed: [0.0, 0.0],
                    velocity: [0.0, 0.15, 0.0],
                    orbit_speed: 1.6,
                    drag: 0.4,
                    wind: 0.0,
                    lifetime: [0.25, 0.6],
                    size: [0.03, 0.07],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.06,
                    color: g(&[(0.0, [1.0, 0.8, 1.0, 1.0]), (1.0, [0.8, 0.35, 1.0, 0.0])]),
                    emissive: 40_000.0,
                    additive: 1.0,
                    ..em("rim_sparks")
                },
                // Motes drawn in, spiraling toward the eye.
                EmitterDesc {
                    rate: 220.0,
                    shape: EmitShape::Ring {
                        radius: 2.6,
                        thickness: 0.7,
                    },
                    speed: [0.0, 0.1],
                    velocity: [0.0, -0.1, 0.0],
                    orbit_speed: 2.4,
                    vortex: 2.0,
                    attract: 1.7,
                    drag: 0.5,
                    wind: 0.0,
                    lifetime: [1.4, 2.4],
                    size: [0.04, 0.09],
                    color: g(&[
                        (0.0, [0.6, 0.35, 1.0, 0.0]),
                        (0.2, [0.8, 0.5, 1.0, 1.0]),
                        (1.0, [1.0, 0.55, 0.95, 0.0]),
                    ]),
                    emissive: 25_000.0,
                    additive: 1.0,
                    ..em("inflow")
                },
                // Stars spilling out toward whoever stands before it, drifting down.
                EmitterDesc {
                    rate: 14.0,
                    shape: EmitShape::Disc { radius: 1.4 },
                    speed: [0.0, 0.3],
                    velocity: [0.0, 1.4, 0.0],
                    gravity: 0.15,
                    drag: 0.6,
                    wind: 0.3,
                    lifetime: [1.2, 2.2],
                    size: [0.08, 0.16],
                    spin_deg: [-90.0, 90.0],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [0.95, 0.8, 1.0, 1.0]), (1.0, [0.6, 0.3, 1.0, 0.0])]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    ..em("spill")
                },
            ],
        )
    };

    let heal_green = [0.5, 1.0, 0.6];
    let heal = EffectDesc {
        prewarm: 3.0,
        light: light([0.55, 1.0, 0.55], 2500.0, 8.0, 0.1, 0.5),
        ..effect(
            "spell.heal_aura",
            "A healing aura: a column of light rising from a slowly turning circle, golden-green \
             motes spiraling up it, pulsing rings on the ground and glints.",
            None,
            vec![
                // The column: a tube of light as wide as the circle, a few overlapping copies
                // crossfading so it never blinks.
                EmitterDesc {
                    rate: 1.0,
                    speed: [0.0, 0.0],
                    lifetime: [3.0, 3.0],
                    size: [2.8, 2.8],
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    steady: true,
                    sprite: Sprite::Veil,
                    facing: Facing::Axis,
                    stretch: 0.82,
                    color: fade(heal_green, 0.5, 0.3, 0.7),
                    emissive: 6000.0,
                    additive: 1.0,
                    soft: 0.2,
                    wind: 0.0,
                    ..em("column")
                },
                // The circle it rises from, turning slowly.
                EmitterDesc {
                    bursts: every(30.0, 1),
                    offset: [0.0, 0.01, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [30.0, 30.0],
                    size: [2.3, 2.3],
                    rotation_deg: [0.0, 0.0],
                    spin_deg: [8.0, 8.0],
                    local_space: true,
                    sprite: Sprite::Rune,
                    facing: Facing::Horizontal,
                    color: fade(heal_green, 0.7, 0.02, 0.97),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("circle")
                },
                // Motes spiraling up the column, drawn as short streaks.
                EmitterDesc {
                    rate: 110.0,
                    shape: EmitShape::Disc { radius: 1.15 },
                    speed: [0.0, 0.1],
                    velocity: [0.0, 1.0, 0.0],
                    orbit_speed: 0.6,
                    turbulence: 0.5,
                    turbulence_scale: 0.8,
                    drag: 0.8,
                    wind: 0.2,
                    lifetime: [1.8, 2.8],
                    size: [0.012, 0.024],
                    facing: Facing::Velocity,
                    stretch: 0.15,
                    color: g(&[
                        (0.0, [0.5, 1.0, 0.5, 0.0]),
                        (0.2, [0.65, 1.0, 0.55, 1.0]),
                        (1.0, [1.0, 0.95, 0.5, 0.0]),
                    ]),
                    emissive: 22_000.0,
                    additive: 1.0,
                    ..em("motes")
                },
                EmitterDesc {
                    bursts: every(1.2, 1),
                    offset: [0.0, 0.06, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [1.4, 1.4],
                    size: [0.6, 0.6],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 4.6)]),
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[
                        (0.0, [0.5, 1.0, 0.6, 0.0]),
                        (0.15, [0.5, 1.0, 0.6, 0.8]),
                        (1.0, [0.6, 1.0, 0.5, 0.0]),
                    ]),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("rings")
                },
                EmitterDesc {
                    rate: 10.0,
                    shape: EmitShape::Disc { radius: 1.1 },
                    speed: [0.0, 0.0],
                    velocity: [0.0, 1.5, 0.0],
                    lifetime: [0.6, 1.0],
                    size: [0.06, 0.12],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [1.0, 0.95, 0.6, 1.0]), (1.0, [0.8, 1.0, 0.5, 0.0])]),
                    emissive: 18_000.0,
                    additive: 1.0,
                    ..em("glints")
                },
                EmitterDesc {
                    rate: 3.0,
                    offset: [0.0, 0.05, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [1.5, 1.5],
                    size: [3.4, 3.4],
                    rotation_deg: [0.0, 360.0],
                    spin_deg: [-12.0, 12.0],
                    local_space: true,
                    sprite: Sprite::Glow,
                    facing: Facing::Horizontal,
                    color: g(&[
                        (0.0, [0.5, 1.0, 0.55, 0.0]),
                        (0.5, [0.5, 1.0, 0.55, 0.3]),
                        (1.0, [0.5, 1.0, 0.55, 0.0]),
                    ]),
                    emissive: 3000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("pool")
                },
            ],
        )
    };

    let charge = EffectDesc {
        light: light([0.6, 0.8, 1.0], 2500.0, 10.0, 0.25, 0.2),
        ..effect(
            "spell.charge",
            "Gathering energy before a cast: motes drawn in from all around on spiraling paths, \
             brightening as they near a crackling white-blue ball of energy.",
            None,
            vec![
                // Motes appear faint and blue on a sphere around the core and spiral in on
                // comet tails, so the pull reads at a glance instead of as scattered dashes.
                EmitterDesc {
                    rate: 70.0,
                    shape: EmitShape::Sphere {
                        radius: 2.2,
                        surface: true,
                    },
                    speed: [0.0, 0.0],
                    attract: 16.0,
                    vortex: 9.0,
                    drag: 1.2,
                    wind: 0.0,
                    local_space: true,
                    lifetime: [0.7, 1.0],
                    size: [0.06, 0.1],
                    size_over_life: c(&[(0.0, 0.6), (0.7, 1.0), (1.0, 0.5)]),
                    facing: Facing::Velocity,
                    stretch: 0.05,
                    color: g(&[
                        (0.0, [0.35, 0.55, 1.0, 0.0]),
                        (0.45, [0.45, 0.65, 1.0, 0.7]),
                        (0.85, [0.75, 0.88, 1.0, 1.0]),
                        (1.0, [0.9, 0.95, 1.0, 0.0]),
                    ]),
                    emissive: 10_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("converge")
                },
                // The gathered charge: a crackling ball of energy, a few overlapping copies
                // crossfading so it never blinks.
                EmitterDesc {
                    rate: 3.0,
                    speed: [0.0, 0.0],
                    local_space: true,
                    steady: true,
                    lifetime: [0.7, 0.7],
                    size: [0.9, 0.9],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Plasma,
                    color: fade([0.6, 0.8, 1.0], 1.0, 0.3, 0.7),
                    emissive: 5_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("core")
                },
            ],
        )
    };

    let lightning_hit = EffectDesc {
        light: light([0.7, 0.8, 1.0], 30_000.0, 16.0, 0.6, 0.3),
        ..effect(
            "spell.lightning_hit",
            "Where a lightning bolt strikes: white-blue flash, a starburst, electric arcs \
             crawling out over the ground, bouncing sparks and a wisp of scorched smoke.",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.15, 0.15],
                    size: [1.6, 1.6],
                    color: g(&[(0.0, [0.85, 0.9, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 100_000.0,
                    additive: 1.0,
                    ..em("flash")
                },
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.12, 0.12],
                    size: [2.5, 2.5],
                    sprite: Sprite::Star,
                    rotation_deg: [0.0, 90.0],
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.6, 0.7, 1.0, 0.0])]),
                    emissive: 80_000.0,
                    additive: 1.0,
                    ..em("star")
                },
                EmitterDesc {
                    color: g(&[
                        (0.0, [0.9, 0.95, 1.0, 1.0]),
                        (0.5, [0.6, 0.75, 1.0, 1.0]),
                        (1.0, [0.4, 0.5, 1.0, 0.0]),
                    ]),
                    ..sparks("sparks", 80, [3.0, 10.0])
                },
                // Arcs crawling out over the ground, crackling for a moment.
                EmitterDesc {
                    bursts: once(7),
                    offset: [0.0, 0.03, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.25, 0.45],
                    size: [1.6, 2.6],
                    rotation_deg: [0.0, 360.0],
                    local_space: true,
                    sprite: Sprite::Arc,
                    facing: Facing::Horizontal,
                    color: g(&[
                        (0.0, [0.85, 0.92, 1.0, 1.0]),
                        (0.6, [0.6, 0.75, 1.0, 1.0]),
                        (1.0, [0.45, 0.55, 1.0, 0.0]),
                    ]),
                    emissive: 60_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("arcs")
                },
                EmitterDesc {
                    bursts: once(4),
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    speed: [0.3, 0.8],
                    acceleration: [0.0, 0.6, 0.0],
                    lifetime: [1.2, 2.0],
                    size: [0.3, 0.45],
                    ..smoke("smoke", 0.3, 0.3)
                },
            ],
        )
    };

    let orb = EffectDesc {
        light: light([1.0, 0.75, 0.45], 700.0, 6.0, 0.15, 0.4),
        ..effect(
            "magic.orb",
            "A floating light orb (wisp, lantern spirit): a small crackling ball of light, \
             white-hot at its heart, in a soft amber halo, with orbiting sparkles.",
            None,
            vec![
                EmitterDesc {
                    rate: 3.0,
                    speed: [0.0, 0.0],
                    local_space: true,
                    steady: true,
                    lifetime: [0.8, 0.8],
                    size: [0.32, 0.32],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Plasma,
                    color: fade([1.0, 0.7, 0.4], 1.0, 0.3, 0.7),
                    emissive: 5_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("core")
                },
                // A dim glow around the heart, so it reads as a ball of light rather than a dot.
                EmitterDesc {
                    rate: 12.0,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.5, 0.7],
                    size: [0.5, 0.65],
                    color: g(&[
                        (0.0, [1.0, 0.55, 0.2, 0.0]),
                        (0.3, [1.0, 0.55, 0.2, 0.35]),
                        (1.0, [1.0, 0.45, 0.15, 0.0]),
                    ]),
                    emissive: 3_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    ..em("halo")
                },
                EmitterDesc {
                    rate: 14.0,
                    shape: EmitShape::Ring {
                        radius: 0.35,
                        thickness: 0.05,
                    },
                    speed: [0.0, 0.0],
                    orbit_speed: 1.5,
                    vortex: 3.0,
                    local_space: true,
                    lifetime: [0.6, 1.0],
                    size: [0.06, 0.1],
                    sprite: Sprite::Star,
                    spin_deg: [-180.0, 180.0],
                    color: g(&[(0.0, [1.0, 0.9, 0.7, 1.0]), (1.0, [1.0, 0.6, 0.3, 0.0])]),
                    emissive: 20_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    ..em("sparkles")
                },
            ],
        )
    };

    // A shell climbing straight up: the same flight for every layer that rides it, so each can
    // burst into its own kind of star.
    let shell = |name: &str| EmitterDesc {
        bursts: once(1),
        speed: [26.0, 26.0],
        gravity: 1.0,
        drag: 0.15,
        wind: 0.0,
        lifetime: [1.75, 1.75],
        size: [0.2, 0.2],
        sprite: Sprite::Spark,
        facing: Facing::Velocity,
        stretch: 0.05,
        color: g(&[
            (0.0, [1.0, 0.8, 0.5, 1.0]),
            (0.8, [1.0, 0.62, 0.3, 1.0]),
            (1.0, [1.0, 0.9, 0.8, 1.0]),
        ]),
        emissive: 30_000.0,
        additive: 1.0,
        soft: 0.0,
        ..em(name)
    };
    let unseen = || g(&[(0.0, [1.0, 1.0, 1.0, 0.0]), (1.0, [1.0, 1.0, 1.0, 0.0])]);
    let stars = |name: &str, speed: [f32; 2], life: [f32; 2], colors: Gradient| EmitterDesc {
        shape: EmitShape::Sphere {
            radius: 0.3,
            surface: true,
        },
        speed,
        gravity: 0.5,
        drag: 0.8,
        wind: 0.3,
        lifetime: life,
        size: [0.2, 0.28],
        size_over_life: c(&[(0.0, 1.0), (0.7, 0.85), (1.0, 0.4)]),
        sprite: Sprite::Spark,
        facing: Facing::Velocity,
        stretch: 0.07,
        color: colors,
        emissive: 35_000.0,
        additive: 1.0,
        soft: 0.0,
        ..em(name)
    };
    // Burning bits a star or shell drops behind it, hanging and falling as they fade.
    let tail = |name: &str, life: [f32; 2], colors: Gradient| EmitterDesc {
        shape: EmitShape::Sphere {
            radius: 0.05,
            surface: false,
        },
        speed: [0.1, 0.5],
        gravity: 0.35,
        drag: 2.5,
        wind: 0.4,
        lifetime: life,
        size: [0.14, 0.22],
        size_over_life: c(&[(0.0, 1.0), (1.0, 0.3)]),
        sprite: Sprite::Glow,
        color: colors,
        emissive: 20_000.0,
        additive: 1.0,
        soft: 0.0,
        ..em(name)
    };
    let starburst = effect(
        "spell.starburst",
        "A mage's firework: a shell streaks about 25 m up on a tail of sparks and bursts into a \
         sphere of violet stars around a gold heart; each star trails glitter, droops, and \
         crackles as it burns out, leaving a drifting smoke ball. Fire it from the ground, up \
         axis to the sky.",
        Some(0.1),
        vec![
            EmitterDesc {
                on_death: Some(SubEmit {
                    inherit_velocity: 0.2,
                    ..SubEmit::new("stars", 160)
                }),
                trailing: Some(SubEmit {
                    inherit_velocity: 0.1,
                    ..SubEmit::new("shell_tail", 160)
                }),
                ..shell("shell")
            },
            // Rides the same flight unseen and bursts into the gold heart.
            EmitterDesc {
                color: unseen(),
                emissive: 0.0,
                on_death: Some(SubEmit {
                    inherit_velocity: 0.2,
                    ..SubEmit::new("heart", 70)
                }),
                ..shell("heart_shell")
            },
            // And into the flash and the smoke the burst leaves hanging.
            EmitterDesc {
                color: unseen(),
                emissive: 0.0,
                on_death: Some(SubEmit::new("flash", 1)),
                ..shell("flash_shell")
            },
            EmitterDesc {
                color: unseen(),
                emissive: 0.0,
                on_death: Some(SubEmit::new("burst_smoke", 12)),
                ..shell("smoke_shell")
            },
            EmitterDesc {
                on_death: Some(SubEmit {
                    chance: 0.75,
                    inherit_velocity: 0.3,
                    ..SubEmit::new("crackle", 3)
                }),
                trailing: Some(SubEmit::new("star_tail", 30)),
                ..stars(
                    "stars",
                    [14.0, 16.0],
                    [2.0, 2.7],
                    g(&[
                        (0.0, [1.0, 0.95, 1.0, 1.0]),
                        (0.08, [0.8, 0.52, 1.0, 1.0]),
                        (0.6, [0.62, 0.3, 1.0, 1.0]),
                        (1.0, [0.95, 0.4, 0.9, 0.0]),
                    ]),
                )
            },
            stars(
                "heart",
                [6.0, 7.5],
                [1.5, 1.9],
                g(&[
                    (0.0, [1.0, 1.0, 0.9, 1.0]),
                    (0.12, [1.0, 0.84, 0.45, 1.0]),
                    (1.0, [1.0, 0.5, 0.15, 0.0]),
                ]),
            ),
            tail(
                "shell_tail",
                [0.3, 0.7],
                g(&[(0.0, [1.0, 0.75, 0.4, 1.0]), (1.0, [1.0, 0.35, 0.08, 0.0])]),
            ),
            // Gold glitter, dimmer than the stars so it stays gold rather than white at night.
            EmitterDesc {
                emissive: 12_000.0,
                ..tail(
                    "star_tail",
                    [0.5, 1.1],
                    g(&[
                        (0.0, [1.0, 0.78, 0.9, 0.9]),
                        (0.25, [1.0, 0.62, 0.3, 0.75]),
                        (1.0, [1.0, 0.4, 0.12, 0.0]),
                    ]),
                )
            },
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 0.05,
                    surface: false,
                },
                speed: [0.8, 2.5],
                gravity: 0.3,
                drag: 2.0,
                wind: 0.3,
                lifetime: [0.12, 0.4],
                size: [0.3, 0.55],
                sprite: Sprite::Star,
                spin_deg: [-360.0, 360.0],
                color: g(&[
                    (0.0, [1.0, 0.97, 0.88, 1.0]),
                    (0.5, [1.0, 0.88, 0.62, 1.0]),
                    (1.0, [1.0, 0.7, 0.4, 0.0]),
                ]),
                emissive: 50_000.0,
                additive: 1.0,
                soft: 0.0,
                ..em("crackle")
            },
            // The moment of the burst: a big soft flash.
            EmitterDesc {
                speed: [0.0, 0.0],
                wind: 0.0,
                lifetime: [0.25, 0.25],
                size: [14.0, 14.0],
                size_over_life: c(&[(0.0, 0.5), (1.0, 1.0)]),
                sprite: Sprite::Glow,
                color: g(&[(0.0, [0.85, 0.7, 1.0, 0.5]), (1.0, [0.7, 0.5, 1.0, 0.0])]),
                emissive: 12_000.0,
                additive: 1.0,
                soft: 0.0,
                ..em("flash")
            },
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 3.0,
                    surface: false,
                },
                speed: [0.3, 1.5],
                drag: 0.8,
                wind: 1.0,
                lifetime: [6.0, 8.0],
                size: [4.0, 6.0],
                size_over_life: c(&[(0.0, 0.6), (1.0, 1.6)]),
                ..smoke("burst_smoke", 0.45, 0.14)
            },
            // The launch: a spurt of sparks and smoke from the ground.
            EmitterDesc {
                shape: EmitShape::Cone {
                    angle_deg: 18.0,
                    radius: 0.05,
                },
                collide: Collide::None,
                lifetime: [0.25, 0.6],
                ..sparks("launch_sparks", 40, [3.0, 9.0])
            },
            EmitterDesc {
                bursts: once(6),
                shape: EmitShape::Hemisphere { radius: 0.1 },
                speed: [0.3, 1.0],
                lifetime: [1.5, 2.5],
                size: [0.3, 0.6],
                ..smoke("launch_smoke", 0.4, 0.35)
            },
        ],
    );

    vec![
        fireball,
        fireball_impact,
        frost_bolt,
        frost_impact,
        portal,
        gate,
        heal,
        charge,
        lightning_hit,
        orb,
        starburst,
    ]
}

fn water() -> Vec<EffectDesc> {
    let water_rgb = [0.78, 0.86, 0.9];
    let splash = effect(
        "water.splash",
        "Something heavy hitting water: a crown of droplets, a central column, spray, foam and \
         expanding ripples. Put the entity on the water surface.",
        Some(0.45),
        vec![
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.2,
                    thickness: 0.05,
                },
                size: [0.008, 0.022],
                lifetime: [0.7, 1.4],
                stretch: 0.012,
                ..droplets("crown", 160, water_rgb, [1.2, 2.6], 3.2, Collide::Die)
            },
            // The crown's thin wall of water, rising, tearing and slumping back as the jet
            // shoots up from the middle.
            crown(1.3, 0.95),
            // Drops breaking off the top of the jet.
            EmitterDesc {
                bursts: vec![at(0.42, 40), at(0.5, 30)],
                shape: EmitShape::Cone {
                    angle_deg: 9.0,
                    radius: 0.03,
                },
                offset: [0.0, 0.5, 0.0],
                speed: [1.0, 2.6],
                velocity: [0.0, 0.0, 0.0],
                lifetime: [0.8, 1.3],
                size: [0.03, 0.06],
                stretch: 0.04,
                ..droplets("column", 70, water_rgb, [1.0, 2.6], 0.0, Collide::Die)
            },
            EmitterDesc {
                bursts: once(10),
                shape: EmitShape::Hemisphere { radius: 0.3 },
                speed: [0.8, 2.2],
                drag: 2.2,
                gravity: 0.1,
                lifetime: [0.8, 1.5],
                size: [0.3, 0.55],
                size_over_life: c(&[(0.0, 0.6), (1.0, 2.0)]),
                color: g(&[
                    (0.0, [0.92, 0.96, 1.0, 0.28]),
                    (1.0, [0.92, 0.96, 1.0, 0.0]),
                ]),
                ..smoke("spray", 0.92, 0.28)
            },
            ripples("ripples", &[0.0, 0.18, 0.4], 7.0, 2.2, 1.0),
            foam(10, 0.4, [0.35, 0.6], 3.5),
        ],
    );

    let splash_small = effect(
        "water.splash_small",
        "A small splash (a bullet, a stone or a footstep in water): a few droplets and one \
         ripple.",
        Some(0.2),
        vec![
            crown(0.32, 0.45),
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.05,
                    thickness: 0.02,
                },
                size: [0.012, 0.025],
                lifetime: [0.4, 0.8],
                ..droplets("crown", 26, water_rgb, [0.6, 1.4], 1.8, Collide::Die)
            },
            ripples("ripple", &[0.0, 0.12], 3.5, 1.4, 0.8),
            foam(3, 0.06, [0.08, 0.14], 1.5),
        ],
    );

    let fountain = EffectDesc {
        prewarm: 3.0,
        ..effect(
            "water.fountain",
            "A fountain jet: a glassy rod of water rising 2.3 m, breaking into beads at the top \
             and raining back around itself, with ripples and mist where it lands. The landing \
             layers sit 0.6 m below the nozzle.",
            None,
            vec![
                // The jet itself: a glassy rod of water rising 2.3 m, a few overlapping copies
                // crossfading so it flows without a break.
                EmitterDesc {
                    rate: 4.0,
                    speed: [0.0, 0.0],
                    lifetime: [0.7, 0.9],
                    size: [2.3, 2.3],
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    steady: true,
                    sprite: Sprite::Jet,
                    facing: Facing::Axis,
                    stretch: 0.09,
                    lit: 1.0,
                    color: fade([0.8, 0.88, 0.95], 0.8, 0.3, 0.7),
                    wind: 0.0,
                    soft: 0.0,
                    ..em("jet")
                },
                // Water turning over at the top and raining back down around the jet.
                EmitterDesc {
                    rate: 700.0,
                    shape: EmitShape::Cone {
                        angle_deg: 85.0,
                        radius: 0.05,
                    },
                    offset: [0.0, 2.15, 0.0],
                    speed: [0.2, 1.1],
                    gravity: 1.0,
                    drag: 0.08,
                    wind: 0.4,
                    collide: Collide::Die,
                    lifetime: [0.9, 1.1],
                    size: [0.016, 0.032],
                    sprite: Sprite::Droplet,
                    facing: Facing::Velocity,
                    stretch: 0.03,
                    lit: 1.0,
                    color: g(&[
                        (0.0, [0.8, 0.88, 0.95, 0.0]),
                        (0.1, [0.8, 0.88, 0.95, 0.55]),
                        (1.0, [0.8, 0.88, 0.95, 0.5]),
                    ]),
                    soft: 0.05,
                    ..em("fall")
                },
                // Spray shed along the jet as it rises.
                EmitterDesc {
                    rate: 60.0,
                    shape: EmitShape::Cone {
                        angle_deg: 3.0,
                        radius: 0.03,
                    },
                    speed: [5.0, 6.9],
                    gravity: 1.0,
                    drag: 0.1,
                    wind: 0.4,
                    collide: Collide::Die,
                    lifetime: [1.4, 1.8],
                    size: [0.008, 0.016],
                    sprite: Sprite::Droplet,
                    facing: Facing::Velocity,
                    stretch: 0.03,
                    lit: 1.0,
                    color: fade([0.8, 0.88, 0.95], 0.45, 0.05, 0.8),
                    soft: 0.05,
                    ..em("spray")
                },
                EmitterDesc {
                    rate: 25.0,
                    shape: EmitShape::Disc { radius: 0.8 },
                    offset: [0.0, -0.55, 0.0],
                    speed: [0.1, 0.4],
                    velocity: [0.0, 0.3, 0.0],
                    lifetime: [1.5, 2.5],
                    size: [0.3, 0.6],
                    size_over_life: c(&[(0.0, 0.7), (1.0, 2.0)]),
                    color: fade([0.92, 0.96, 1.0], 0.08, 0.15, 0.5),
                    ..smoke("mist", 0.9, 0.08)
                },
                EmitterDesc {
                    rate: 7.0,
                    shape: EmitShape::Disc { radius: 0.7 },
                    lifetime: [1.0, 1.4],
                    size: [0.35, 0.5],
                    offset: [0.0, -0.58, 0.0],
                    bursts: Vec::new(),
                    ..ripples("ripples", &[], 5.0, 1.2, 0.55)
                },
                EmitterDesc {
                    rate: 140.0,
                    shape: EmitShape::Disc { radius: 0.6 },
                    offset: [0.0, -0.6, 0.0],
                    speed: [0.3, 0.8],
                    velocity: [0.0, 1.3, 0.0],
                    bursts: Vec::new(),
                    lifetime: [0.3, 0.6],
                    size: [0.012, 0.024],
                    ..droplets("splashes", 0, water_rgb, [0.3, 0.8], 1.3, Collide::None)
                },
            ],
        )
    };

    let foam_white = [0.93, 0.96, 1.0];
    let column = effect(
        "water.column",
        "An explosion under or on the water: a white column thrown about 10 m up (scale 2 for \
         about 40 m), water falling back as droplets, a surge of spray rolling outward from \
         its foot, lingering mist, foam and wide ripples. Put the entity on the surface.",
        Some(0.6),
        vec![
            // The column itself: white water shooting up for half a second. The spread of
            // speeds fills the trunk from the surface to the top.
            EmitterDesc {
                rate: 420.0,
                duration: Some(0.45),
                shape: EmitShape::Cone {
                    angle_deg: 4.0,
                    radius: 0.55,
                },
                speed: [3.5, 14.5],
                gravity: 1.0,
                drag: 0.2,
                turbulence: 0.4,
                lifetime: [1.4, 2.4],
                size: [0.55, 1.0],
                size_over_life: c(&[(0.0, 0.5), (0.25, 1.3), (1.0, 2.4)]),
                // Albedo above one stands in for light scattered many times inside the foam.
                color: g(&[
                    (0.0, [1.2, 1.23, 1.28, 0.0]),
                    (0.04, [1.2, 1.23, 1.28, 0.75]),
                    (0.6, [1.2, 1.23, 1.28, 0.5]),
                    (1.0, [1.2, 1.23, 1.28, 0.0]),
                ]),
                ..smoke("column", 1.2, 0.75)
            },
            // The dome of white water heaving up at the column's foot.
            EmitterDesc {
                bursts: once(36),
                shape: EmitShape::Hemisphere { radius: 1.0 },
                speed: [2.0, 5.0],
                velocity: [0.0, 1.5, 0.0],
                gravity: 0.6,
                drag: 1.4,
                lifetime: [1.2, 2.0],
                size: [0.9, 1.5],
                size_over_life: c(&[(0.0, 0.6), (1.0, 2.2)]),
                color: fade([1.15, 1.18, 1.22], 0.7, 0.04, 0.45),
                ..smoke("dome", 1.15, 0.7)
            },
            // Water flung out of the column, falling back in long arcs.
            EmitterDesc {
                rate: 1600.0,
                duration: Some(0.4),
                bursts: Vec::new(),
                shape: EmitShape::Cone {
                    angle_deg: 14.0,
                    radius: 0.5,
                },
                speed: [5.0, 15.0],
                velocity: [0.0, 0.0, 0.0],
                lifetime: [2.0, 3.0],
                size: [0.04, 0.09],
                stretch: 0.03,
                drag: 0.12,
                color: g(&[(0.0, [0.9, 0.94, 0.98, 0.9]), (1.0, [0.9, 0.94, 0.98, 0.8])]),
                ..droplets("fallback", 0, water_rgb, [5.0, 15.0], 0.0, Collide::Die)
            },
            // The base surge: spray rolling outward low over the water.
            EmitterDesc {
                bursts: vec![at(0.12, 36)],
                shape: EmitShape::Ring {
                    radius: 1.2,
                    thickness: 0.4,
                },
                speed: [3.5, 7.0],
                velocity: [0.0, 1.2, 0.0],
                gravity: 0.05,
                drag: 1.1,
                lifetime: [2.5, 3.8],
                size: [1.0, 1.8],
                size_over_life: c(&[(0.0, 0.6), (1.0, 3.2)]),
                color: fade(foam_white, 0.55, 0.06, 0.4),
                ..smoke("surge", 0.94, 0.55)
            },
            // Mist left hanging where the column stood, drifting off with the wind.
            EmitterDesc {
                bursts: vec![at(0.9, 16)],
                shape: EmitShape::Sphere {
                    radius: 3.0,
                    surface: false,
                },
                offset: [0.0, 4.0, 0.0],
                speed: [0.1, 0.5],
                gravity: 0.0,
                drag: 1.5,
                wind: 1.0,
                lifetime: [4.0, 6.5],
                size: [2.0, 3.2],
                size_over_life: c(&[(0.0, 0.7), (1.0, 2.0)]),
                color: fade(foam_white, 0.22, 0.2, 0.45),
                ..smoke("mist", 0.95, 0.22)
            },
            EmitterDesc {
                speed: [0.4, 1.6],
                ..foam(24, 2.0, [1.0, 1.8], 6.0)
            },
            EmitterDesc {
                size: [1.5, 1.5],
                ..ripples("ripples", &[0.0, 0.3, 0.7, 1.2], 16.0, 2.6, 0.7)
            },
        ],
    );

    let bow_spray = effect(
        "water.bow_spray",
        "A puff of spray thrown off a fast boat's bow or a skimming object: droplets and fine \
         spray flung along the entity's +Y (point it along the spray's velocity).",
        Some(0.12),
        vec![
            EmitterDesc {
                shape: EmitShape::Cone {
                    angle_deg: 22.0,
                    radius: 0.15,
                },
                lifetime: [0.6, 1.1],
                size: [0.025, 0.055],
                stretch: 0.03,
                ..droplets("droplets", 70, water_rgb, [3.0, 7.0], 0.0, Collide::Die)
            },
            EmitterDesc {
                bursts: once(6),
                shape: EmitShape::Cone {
                    angle_deg: 25.0,
                    radius: 0.2,
                },
                speed: [2.0, 4.5],
                gravity: 0.15,
                drag: 2.2,
                wind: 1.0,
                lifetime: [0.8, 1.4],
                size: [0.35, 0.6],
                size_over_life: c(&[(0.0, 0.6), (1.0, 2.4)]),
                color: fade(foam_white, 0.35, 0.05, 0.3),
                ..smoke("spray", 0.94, 0.35)
            },
        ],
    );

    let mist = effect(
        "water.mist",
        "A puff of fine mist, as at the foot of a waterfall: soft, slow, carried off by the \
         wind and lingering for several seconds. Meant to be played a few times a second; \
         scale 1 is about 1.5 m across.",
        Some(0.1),
        vec![
            EmitterDesc {
                bursts: once(4),
                shape: EmitShape::Sphere {
                    radius: 0.6,
                    surface: false,
                },
                speed: [0.2, 0.8],
                velocity: [0.0, 1.0, 0.0],
                gravity: 0.0,
                drag: 0.9,
                wind: 1.0,
                turbulence: 0.5,
                lifetime: [4.0, 7.0],
                size: [1.0, 1.8],
                size_over_life: c(&[(0.0, 0.6), (1.0, 2.6)]),
                color: fade([0.9, 0.94, 1.0], 0.14, 0.15, 0.4),
                soft: 0.8,
                ..smoke("mist", 0.95, 0.14)
            },
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 0.4,
                    surface: false,
                },
                lifetime: [0.5, 1.0],
                size: [0.015, 0.03],
                ..droplets("drizzle", 10, water_rgb, [0.5, 2.0], 1.5, Collide::Die)
            },
        ],
    );

    vec![splash, splash_small, fountain, column, bow_spray, mist]
}

fn impacts() -> Vec<EffectDesc> {
    let sparks_effect = |name: &str| EffectDesc {
        light: light([1.0, 0.7, 0.4], 8000.0, 6.0, 0.5, 0.15),
        ..effect(
            name,
            "Metal struck hard: a shower of bouncing sparks that split as they skid and burst \
             as they burn out, with a bright glint and a wisp of smoke.",
            Some(0.1),
            vec![
                EmitterDesc {
                    on_hit: Some(SubEmit {
                        chance: 0.3,
                        inherit_velocity: 0.35,
                        ..SubEmit::new("split", 3)
                    }),
                    on_death: Some(SubEmit {
                        chance: 0.2,
                        inherit_velocity: 0.5,
                        ..SubEmit::new("split", 4)
                    }),
                    ..sparks("sparks", 70, [3.0, 10.0])
                },
                // Hot steel sparks fork: a skid throws off a few smaller ones, and some burst
                // into a little star as they burn out.
                EmitterDesc {
                    bursts: Vec::new(),
                    shape: EmitShape::Hemisphere { radius: 0.01 },
                    speed: [1.0, 3.0],
                    drag: 0.8,
                    collide: Collide::None,
                    lifetime: [0.12, 0.3],
                    size: [0.008, 0.016],
                    stretch: 0.025,
                    ..sparks("split", 0, [1.0, 3.0])
                },
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.08, 0.08],
                    size: [0.8, 0.8],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [1.0, 0.9, 0.7, 1.0]), (1.0, [1.0, 0.6, 0.3, 0.0])]),
                    emissive: 60_000.0,
                    additive: 1.0,
                    ..em("glint")
                },
                EmitterDesc {
                    bursts: once(4),
                    shape: EmitShape::Hemisphere { radius: 0.05 },
                    speed: [0.2, 0.6],
                    lifetime: [0.8, 1.4],
                    size: [0.15, 0.3],
                    ..smoke("smoke", 0.3, 0.3)
                },
            ],
        )
    };
    // `kind`: solid chips, or `None` for flat flakes (torn grass, leaves).
    let surface = |name: &str,
                   desc: &str,
                   albedo: [f32; 3],
                   (chip, kind): ([f32; 3], Option<Debris>),
                   extra: Vec<EmitterDesc>| {
        let mut emitters = vec![
            dust_puff("dust", 12, albedo, 0.55, [0.6, 1.8], [0.25, 0.45]),
            EmitterDesc {
                debris: kind,
                ..chips("chips", 16, chip, [2.0, 5.0])
            },
        ];
        emitters.extend(extra);
        effect(name, desc, Some(0.1), emitters)
    };
    let dirt = [0.42, 0.34, 0.26];
    let rock = [0.5, 0.49, 0.47];
    vec![
        sparks_effect("impact.sparks"),
        sparks_effect("impact.metal"),
        surface(
            "impact.default",
            "A generic hit on the ground: a dust puff and chips.",
            dirt,
            ([0.24, 0.19, 0.14], Some(Debris::Rock)),
            vec![],
        ),
        surface(
            "impact.dirt",
            "A hit on dirt: a brown dust puff and clods.",
            dirt,
            ([0.2, 0.15, 0.1], Some(Debris::Rock)),
            vec![],
        ),
        EffectDesc {
            mark: mark(MarkKind::Chip, 0.07, 120.0, 0.0),
            ..surface(
                "impact.rock",
                "A hit on rock: grey dust, stone chips, a few sparks and a chipped pock.",
                rock,
                ([0.3, 0.29, 0.27], Some(Debris::Rock)),
                vec![EmitterDesc {
                    lifetime: [0.2, 0.5],
                    ..sparks("sparks", 6, [3.0, 8.0])
                }],
            )
        },
        EffectDesc {
            mark: mark(MarkKind::Chip, 0.07, 120.0, 0.0),
            ..surface(
                "impact.concrete",
                "A hit on concrete: pale dust, chips, a few sparks and a chipped pock.",
                [0.62, 0.6, 0.56],
                ([0.4, 0.39, 0.37], Some(Debris::Rock)),
                vec![EmitterDesc {
                    lifetime: [0.2, 0.5],
                    ..sparks("sparks", 5, [3.0, 8.0])
                }],
            )
        },
        surface(
            "impact.wood",
            "A hit on wood: splinters and light dust.",
            [0.55, 0.45, 0.32],
            ([0.45, 0.31, 0.17], Some(Debris::Splinter)),
            vec![EmitterDesc {
                size: [0.05, 0.13],
                spin_deg: [-540.0, 540.0],
                debris: Some(Debris::Splinter),
                ..chips("splinters", 14, [0.55, 0.4, 0.24], [2.5, 6.0])
            }],
        ),
        surface(
            "impact.sand",
            "A hit on sand: a spray of grains and a tan cloud.",
            [0.72, 0.62, 0.45],
            ([0.45, 0.38, 0.27], Some(Debris::Rock)),
            vec![EmitterDesc {
                sprite: Sprite::Dust,
                debris: None,
                size: [0.02, 0.04],
                lifetime: [0.7, 1.4],
                collide: Collide::Die,
                ..chips("grains", 40, [0.72, 0.62, 0.45], [1.5, 4.5])
            }],
        ),
        surface(
            "impact.grass",
            "A hit on grass: torn green bits fluttering down and a little dust.",
            [0.36, 0.38, 0.24],
            ([0.3, 0.5, 0.18], None),
            vec![EmitterDesc {
                drag: 2.5,
                gravity: 0.4,
                size: [0.04, 0.08],
                lifetime: [1.0, 1.8],
                debris: None,
                ..chips("bits", 16, [0.3, 0.5, 0.18], [2.0, 4.0])
            }],
        ),
        surface(
            "impact.leaves",
            "A hit on leaf litter: leaves tossed up and fluttering down.",
            [0.4, 0.33, 0.2],
            ([0.55, 0.35, 0.12], None),
            vec![EmitterDesc {
                drag: 3.0,
                gravity: 0.35,
                size: [0.06, 0.1],
                lifetime: [1.2, 2.2],
                turbulence: 3.0,
                debris: None,
                ..chips("leaves", 14, [0.6, 0.38, 0.12], [2.0, 4.0])
            }],
        ),
        effect(
            "impact.snow",
            "A hit on snow: a burst of white powder and drifting flakes.",
            Some(0.1),
            vec![
                dust_puff(
                    "powder",
                    16,
                    [0.92, 0.94, 0.97],
                    0.75,
                    [0.8, 2.4],
                    [0.3, 0.5],
                ),
                EmitterDesc {
                    bursts: once(30),
                    shape: EmitShape::Hemisphere { radius: 0.1 },
                    speed: [1.0, 3.0],
                    gravity: 0.15,
                    drag: 2.5,
                    turbulence: 1.5,
                    lifetime: [1.0, 2.0],
                    size: [0.02, 0.04],
                    sprite: Sprite::Dust,
                    lit: 1.0,
                    color: fade([0.95, 0.97, 1.0], 0.9, 0.05, 0.7),
                    soft: 0.0,
                    ..em("flakes")
                },
            ],
        ),
        effect(
            "impact.mud",
            "A hit on mud: wet clumps splattering and sticking.",
            Some(0.1),
            vec![
                EmitterDesc {
                    shape: EmitShape::Hemisphere { radius: 0.05 },
                    size: [0.04, 0.09],
                    lifetime: [1.5, 2.5],
                    stretch: 0.005,
                    color: g(&[
                        (0.0, [0.22, 0.16, 0.1, 1.0]),
                        (0.9, [0.22, 0.16, 0.1, 1.0]),
                        (1.0, [0.22, 0.16, 0.1, 0.0]),
                    ]),
                    ..droplets(
                        "clumps",
                        24,
                        [0.22, 0.16, 0.1],
                        [1.5, 4.0],
                        0.5,
                        Collide::Stick,
                    )
                },
                dust_puff(
                    "spatter",
                    4,
                    [0.3, 0.24, 0.17],
                    0.35,
                    [0.3, 0.8],
                    [0.2, 0.3],
                ),
            ],
        ),
        EffectDesc {
            name: "impact.water".into(),
            description: "A hit on water (same as water.splash_small).".into(),
            ..water()
                .into_iter()
                .find(|e| e.name == "water.splash_small")
                .unwrap()
        },
        effect(
            "impact.flesh",
            "A hit on a creature: a dark red spray of droplets and a thin mist.",
            Some(0.1),
            vec![
                EmitterDesc {
                    shape: EmitShape::Cone {
                        angle_deg: 35.0,
                        radius: 0.03,
                    },
                    size: [0.02, 0.05],
                    lifetime: [0.5, 1.0],
                    ..droplets(
                        "drops",
                        25,
                        [0.35, 0.02, 0.02],
                        [2.0, 5.0],
                        0.0,
                        Collide::Stick,
                    )
                },
                EmitterDesc {
                    bursts: once(4),
                    shape: EmitShape::Point,
                    spread_deg: 40.0,
                    speed: [0.5, 1.5],
                    lifetime: [0.4, 0.8],
                    size: [0.2, 0.3],
                    color: fade([0.4, 0.04, 0.04], 0.4, 0.1, 0.3),
                    ..smoke("mist", 0.4, 0.4)
                },
            ],
        ),
        effect(
            "impact.ice",
            "A hit on ice: glinting ice chips and a frosty puff.",
            Some(0.1),
            vec![
                EmitterDesc {
                    debris: Some(Debris::Shard),
                    ..chips("chips", 20, [0.82, 0.92, 1.0], [2.0, 5.0])
                },
                dust_puff("frost", 6, [0.9, 0.95, 1.0], 0.4, [0.5, 1.5], [0.2, 0.35]),
                EmitterDesc {
                    bursts: once(8),
                    shape: EmitShape::Hemisphere { radius: 0.1 },
                    speed: [0.5, 2.0],
                    drag: 2.0,
                    lifetime: [0.3, 0.6],
                    size: [0.08, 0.15],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [0.9, 0.97, 1.0, 1.0]), (1.0, [0.6, 0.85, 1.0, 0.0])]),
                    emissive: 15_000.0,
                    additive: 1.0,
                    ..em("glints")
                },
            ],
        ),
        effect(
            "impact.glass",
            "Glass breaking: a spray of glinting shards.",
            Some(0.1),
            vec![
                EmitterDesc {
                    size: [0.03, 0.1],
                    debris: Some(Debris::Shard),
                    ..chips("shards", 40, [0.55, 0.66, 0.65], [2.0, 6.0])
                },
                EmitterDesc {
                    bursts: once(10),
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    speed: [1.0, 3.0],
                    drag: 1.5,
                    lifetime: [0.2, 0.5],
                    size: [0.1, 0.18],
                    sprite: Sprite::Star,
                    color: g(&[(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [0.8, 0.9, 1.0, 0.0])]),
                    emissive: 20_000.0,
                    additive: 1.0,
                    ..em("glints")
                },
            ],
        ),
        effect(
            "impulse.dust",
            "A ground shockwave (explosion, heavy landing, slam): a ring of dust rushing outward, \
             a pressure ring and flying debris.",
            Some(0.15),
            vec![
                EmitterDesc {
                    bursts: once(40),
                    shape: EmitShape::Ring {
                        radius: 0.6,
                        thickness: 0.3,
                    },
                    speed: [5.0, 9.0],
                    velocity: [0.0, 0.6, 0.0],
                    drag: 2.2,
                    lifetime: [1.5, 3.0],
                    size: [0.8, 1.4],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 3.5)]),
                    ..smoke("dust", 0.42, 0.6)
                },
                EmitterDesc {
                    lit: 1.0,
                    color: g(&[
                        (0.0, [0.7, 0.65, 0.58, 0.55]),
                        (1.0, [0.7, 0.65, 0.58, 0.0]),
                    ]),
                    ..ripples("wave", &[0.0], 12.0, 0.6, 0.55)
                },
                EmitterDesc {
                    size: [0.04, 0.14],
                    ..chips("debris", 30, [0.27, 0.24, 0.21], [4.0, 9.0])
                },
            ],
        ),
        EffectDesc {
            light: light([1.0, 0.6, 0.3], 60_000.0, 30.0, 0.25, 1.0),
            ..effect(
                "explosion.large",
                "A big explosion: fireball, sparks, debris, a shockwave and a smoke column rising \
                 for a few seconds.",
                Some(2.5),
                vec![
                    // The fireball: a few big lobes of burning gas thrown out and rolling up,
                    // charring to soot from the outside in.
                    EmitterDesc {
                        bursts: once(16),
                        offset: [0.0, 0.8, 0.0],
                        shape: EmitShape::Hemisphere { radius: 0.9 },
                        speed: [3.0, 8.0],
                        drag: 3.0,
                        acceleration: [0.0, 4.0, 0.0],
                        turbulence: 3.0,
                        turbulence_scale: 3.0,
                        lifetime: [1.2, 2.2],
                        size: [2.6, 3.8],
                        size_over_life: c(&[(0.0, 0.4), (0.25, 1.0), (1.0, 1.5)]),
                        rotation_deg: [-180.0, 180.0],
                        spin_deg: [-20.0, 20.0],
                        sprite: Sprite::Fireball,
                        lit: 1.0,
                        color: fade([1.0, 0.45, 0.12], 1.0, 0.03, 0.75),
                        emissive: 18_000.0,
                        additive: 0.0,
                        soft: 1.2,
                        ..em("fireball")
                    },
                    EmitterDesc {
                        lifetime: [1.0, 2.5],
                        ..sparks("sparks", 200, [8.0, 22.0])
                    },
                    EmitterDesc {
                        size: [0.06, 0.22],
                        lifetime: [2.5, 4.5],
                        ..chips("debris", 50, [0.2, 0.18, 0.16], [6.0, 16.0])
                    },
                    EmitterDesc {
                        size: [0.08, 0.2],
                        lifetime: [2.5, 4.5],
                        debris: Some(Debris::Scrap),
                        ..chips("scrap", 12, [0.32, 0.3, 0.28], [5.0, 13.0])
                    },
                    EmitterDesc {
                        delay: 0.6,
                        duration: Some(1.9),
                        rate: 30.0,
                        bursts: once(30),
                        shape: EmitShape::Hemisphere { radius: 1.2 },
                        speed: [0.5, 2.0],
                        acceleration: [0.0, 3.0, 0.0],
                        lifetime: [3.0, 6.0],
                        size: [1.5, 2.5],
                        size_over_life: c(&[(0.0, 0.6), (1.0, 3.5)]),
                        turbulence: 1.0,
                        turbulence_scale: 3.0,
                        ..smoke("smoke", 0.1, 0.8)
                    },
                    EmitterDesc {
                        bursts: once(1),
                        offset: [0.0, 0.05, 0.0],
                        speed: [0.0, 0.0],
                        lifetime: [0.6, 0.6],
                        size: [2.0, 2.0],
                        size_over_life: c(&[(0.0, 0.5), (1.0, 14.0)]),
                        rotation_deg: [0.0, 0.0],
                        sprite: Sprite::Ring,
                        facing: Facing::Horizontal,
                        color: g(&[(0.0, [1.0, 0.7, 0.4, 0.8]), (1.0, [1.0, 0.4, 0.1, 0.0])]),
                        emissive: 20_000.0,
                        additive: 1.0,
                        soft: 0.0,
                        ..em("shockwave")
                    },
                    EmitterDesc {
                        bursts: once(24),
                        shape: EmitShape::Sphere {
                            radius: 1.2,
                            surface: false,
                        },
                        speed: [0.5, 2.0],
                        acceleration: [0.0, 2.0, 0.0],
                        drag: 0.8,
                        lifetime: [1.0, 1.8],
                        size: [3.0, 4.5],
                        size_over_life: c(&[(0.0, 0.5), (1.0, 1.6)]),
                        ..haze("haze", 1.0)
                    },
                ],
            )
        },
    ]
}

fn steps() -> Vec<EffectDesc> {
    let puff = |name: &str, desc: &str, albedo: [f32; 3], peak: f32| {
        effect(
            name,
            desc,
            Some(0.05),
            vec![EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.1,
                    thickness: 0.05,
                },
                lifetime: [0.6, 1.0],
                ..dust_puff("puff", 8, albedo, peak, [0.4, 1.0], [0.2, 0.35])
            }],
        )
    };
    vec![
        puff(
            "step.dirt",
            "A footstep on dry dirt: a small dust puff.",
            [0.45, 0.37, 0.28],
            0.35,
        ),
        puff(
            "step.default",
            "A footstep: a faint dust puff.",
            [0.45, 0.4, 0.33],
            0.2,
        ),
        effect(
            "step.sand",
            "A footstep on sand: a tan puff and kicked grains.",
            Some(0.05),
            vec![
                dust_puff("puff", 8, [0.72, 0.62, 0.45], 0.45, [0.4, 1.0], [0.2, 0.35]),
                EmitterDesc {
                    sprite: Sprite::Dust,
                    debris: None,
                    size: [0.02, 0.04],
                    lifetime: [0.7, 1.4],
                    collide: Collide::Die,
                    shape: EmitShape::Cone {
                        angle_deg: 50.0,
                        radius: 0.05,
                    },
                    ..chips("grains", 30, [0.72, 0.62, 0.45], [0.8, 2.0])
                },
            ],
        ),
        effect(
            "step.snow",
            "A footstep in snow: a puff of powder and a few flakes.",
            Some(0.05),
            vec![
                dust_puff(
                    "powder",
                    12,
                    [0.93, 0.95, 0.98],
                    0.85,
                    [0.4, 1.2],
                    [0.2, 0.4],
                ),
                EmitterDesc {
                    bursts: once(24),
                    shape: EmitShape::Cone {
                        angle_deg: 45.0,
                        radius: 0.1,
                    },
                    speed: [0.6, 1.6],
                    gravity: 0.3,
                    drag: 2.0,
                    lifetime: [0.6, 1.2],
                    size: [0.025, 0.045],
                    sprite: Sprite::Dust,
                    lit: 1.0,
                    color: fade([0.95, 0.97, 1.0], 0.9, 0.05, 0.7),
                    soft: 0.0,
                    ..em("flakes")
                },
            ],
        ),
        effect(
            "step.mud",
            "A footstep in mud: wet clumps squelching out.",
            Some(0.05),
            vec![EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.12,
                    thickness: 0.05,
                },
                size: [0.03, 0.07],
                lifetime: [1.2, 2.2],
                stretch: 0.0,
                facing: Facing::Camera,
                sprite: Sprite::Shard,
                debris: Some(Debris::Rock),
                spin_deg: [-360.0, 360.0],
                color: g(&[
                    (0.0, [0.2, 0.15, 0.09, 1.0]),
                    (0.75, [0.2, 0.15, 0.09, 1.0]),
                    (1.0, [0.2, 0.15, 0.09, 0.0]),
                ]),
                ..droplets(
                    "clumps",
                    20,
                    [0.2, 0.15, 0.09],
                    [0.8, 1.8],
                    1.2,
                    Collide::Stick,
                )
            }],
        ),
        EffectDesc {
            name: "step.puddle".into(),
            description: "A footstep in a puddle: a small splash and a ripple.".into(),
            ..water()
                .into_iter()
                .find(|e| e.name == "water.splash_small")
                .unwrap()
        },
        EffectDesc {
            name: "step.water".into(),
            description: "A footstep in shallow water: a small splash and a ripple.".into(),
            ..water()
                .into_iter()
                .find(|e| e.name == "water.splash_small")
                .unwrap()
        },
        effect(
            "step.leaves",
            "A footstep in leaf litter: a couple of leaves flicked up.",
            Some(0.05),
            vec![EmitterDesc {
                drag: 3.0,
                gravity: 0.35,
                size: [0.05, 0.09],
                lifetime: [0.8, 1.4],
                turbulence: 2.0,
                debris: None,
                shape: EmitShape::Cone {
                    angle_deg: 40.0,
                    radius: 0.1,
                },
                ..chips("leaves", 3, [0.6, 0.38, 0.12], [1.0, 2.0])
            }],
        ),
    ]
}

/// The flame of one nozzle: short, overlapping plume sprites standing on the nozzle (so it
/// shimmers), `length` m long and `width` m across at its widest, glowing `glow` cd/m².
fn plume(name: &str, rgb: [f32; 3], length: f32, width: f32, glow: f32) -> EmitterDesc {
    EmitterDesc {
        rate: 12.0,
        speed: [0.0, 0.0],
        lifetime: [0.16, 0.24],
        size: [length * 0.94, length * 1.06],
        rotation_deg: [0.0, 0.0],
        local_space: true,
        steady: true,
        sprite: Sprite::Plume,
        facing: Facing::Axis,
        stretch: width / length,
        color: fade(rgb, 1.0, 0.3, 0.7),
        emissive: glow,
        additive: 1.0,
        wind: 0.0,
        soft: 0.4,
        ..em(name)
    }
}

/// The white-hot mouth of a nozzle, a glow that faces the camera so a flame seen end-on still
/// shows: `size` m across.
fn nozzle_glow(rgb: [f32; 3], size: f32, glow: f32) -> EmitterDesc {
    EmitterDesc {
        rate: 12.0,
        speed: [0.0, 0.0],
        offset: [0.0, size * 0.15, 0.0],
        lifetime: [0.2, 0.3],
        size: [size * 0.9, size * 1.1],
        local_space: true,
        steady: true,
        color: fade(rgb, 1.0, 0.3, 0.6),
        emissive: glow,
        additive: 1.0,
        wind: 0.0,
        soft: 0.1,
        ..em("nozzle")
    }
}

/// Engine flames and fumes. Every one plays at a nozzle with its +Y axis pointing out of it,
/// down the exhaust; `scale` 1 is the nozzle size each describes and `intensity` follows the
/// throttle. Plumes widen and lose their shock diamonds as the air thins.
fn exhausts() -> Vec<EffectDesc> {
    const KEROSENE: [f32; 3] = [1.0, 0.6, 0.3];
    const AFTERBURNER: [f32; 3] = [1.0, 0.55, 0.28];
    const ION_BLUE: [f32; 3] = [0.55, 0.72, 1.0];
    vec![
        EffectDesc {
            light: light_at(KEROSENE, 10_000.0, 30.0, 0.12, [0.0, 0.6, 0.0]),
            ..effect(
                "exhaust.rocket",
                "A kerosene rocket engine at full thrust: a blinding orange plume with a train \
                 of shock diamonds and a ragged tail, a white-hot nozzle and a cloud of exhaust \
                 billowing up where the plume dies away. Scale 1 is a nozzle 1 m across.",
                None,
                vec![
                    plume("plume", KEROSENE, 9.0, 3.3, 14_000.0),
                    nozzle_glow([1.0, 0.86, 0.62], 1.0, 40_000.0),
                    // Licks of flame torn off the plume's edge, streaking out along it.
                    EmitterDesc {
                        rate: 70.0,
                        shape: EmitShape::Cone {
                            angle_deg: 4.0,
                            radius: 0.35,
                        },
                        offset: [0.0, 2.0, 0.0],
                        speed: [24.0, 34.0],
                        drag: 2.0,
                        turbulence: 10.0,
                        turbulence_scale: 1.0,
                        lifetime: [0.12, 0.22],
                        size: [0.7, 1.2],
                        size_over_life: c(&[(0.0, 0.7), (1.0, 1.8)]),
                        sprite: Sprite::Flame,
                        facing: Facing::Velocity,
                        stretch: 0.03,
                        color: flame_colors(),
                        emissive: 9000.0,
                        additive: 1.0,
                        wind: 0.0,
                        soft: 0.3,
                        ..em("licks")
                    },
                    // The exhaust cloud billowing up where the plume dies away.
                    EmitterDesc {
                        rate: 30.0,
                        shape: EmitShape::Cone {
                            angle_deg: 30.0,
                            radius: 0.8,
                        },
                        offset: [0.0, 7.0, 0.0],
                        speed: [4.0, 8.0],
                        drag: 1.2,
                        acceleration: [0.0, 0.5, 0.0],
                        collide: Collide::Bounce {
                            restitution: 0.05,
                            friction: 0.05,
                        },
                        turbulence: 1.5,
                        turbulence_scale: 4.0,
                        lifetime: [3.0, 5.0],
                        size: [1.6, 2.4],
                        size_over_life: c(&[(0.0, 0.6), (1.0, 4.0)]),
                        soft: 1.0,
                        ..smoke("cloud", 0.68, 0.35)
                    },
                    // The trail a moving rocket leaves: laid down per meter it travels, close
                    // enough for its puffs to overlap, so it stands as one unbroken column
                    // however fast it goes, lingering and spreading long after it has gone.
                    EmitterDesc {
                        rate_per_meter: 2.0,
                        shape: EmitShape::Cone {
                            angle_deg: 60.0,
                            radius: 0.6,
                        },
                        offset: [0.0, 6.0, 0.0],
                        speed: [0.5, 1.5],
                        drag: 1.5,
                        turbulence: 1.2,
                        turbulence_scale: 6.0,
                        lifetime: [8.0, 12.0],
                        size: [1.8, 2.6],
                        size_over_life: c(&[(0.0, 1.0), (0.2, 1.8), (1.0, 4.5)]),
                        soft: 1.0,
                        ..smoke("trail", 0.8, 0.4)
                    },
                    EmitterDesc {
                        rate: 30.0,
                        offset: [0.0, 2.0, 0.0],
                        spread_deg: 6.0,
                        speed: [12.0, 18.0],
                        lifetime: [0.3, 0.5],
                        size: [2.5, 3.5],
                        size_over_life: c(&[(0.0, 0.6), (1.0, 1.6)]),
                        ..haze("haze", 1.0)
                    },
                ],
            )
        },
        EffectDesc {
            light: light_at(AFTERBURNER, 120_000.0, 45.0, 0.15, [0.0, 2.0, 0.0]),
            ..effect(
                "exhaust.afterburner",
                "A jet engine on afterburner: a long flame with glowing shock diamonds, a blue \
                 heart in the nozzle and a stream of heat haze. Scale 1 is a nozzle 1 m across.",
                None,
                vec![
                    plume("plume", AFTERBURNER, 7.0, 3.3, 16_000.0),
                    nozzle_glow([0.35, 0.45, 1.0], 1.2, 10_000.0),
                    EmitterDesc {
                        rate: 36.0,
                        offset: [0.0, 1.0, 0.0],
                        spread_deg: 5.0,
                        speed: [18.0, 26.0],
                        lifetime: [0.35, 0.6],
                        size: [1.4, 2.0],
                        size_over_life: c(&[(0.0, 0.6), (1.0, 2.2)]),
                        ..haze("haze", 1.0)
                    },
                ],
            )
        },
        effect(
            "exhaust.jet",
            "A jet engine at dry thrust: no flame, only the dull glow of the turbine deep in \
             the nozzle and a fast stream of shimmering hot air. Scale 1 is a nozzle 1 m across.",
            None,
            vec![
                EmitterDesc {
                    rate: 6.0,
                    speed: [0.0, 0.0],
                    offset: [0.0, -0.15, 0.0],
                    lifetime: [0.4, 0.6],
                    size: [0.8, 0.9],
                    local_space: true,
                    color: fade([1.0, 0.45, 0.2], 0.8, 0.3, 0.6),
                    emissive: 2500.0,
                    additive: 1.0,
                    wind: 0.0,
                    soft: 0.05,
                    ..em("turbine")
                },
                EmitterDesc {
                    rate: 40.0,
                    shape: EmitShape::Cone {
                        angle_deg: 5.0,
                        radius: 0.3,
                    },
                    offset: [0.0, 0.5, 0.0],
                    speed: [20.0, 30.0],
                    lifetime: [0.4, 0.7],
                    size: [1.0, 1.4],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.6)]),
                    ..haze("haze", 1.0)
                },
            ],
        ),
        EffectDesc {
            light: light_at(ION_BLUE, 50_000.0, 25.0, 0.05, [0.0, 1.0, 0.0]),
            ..effect(
                "exhaust.thruster",
                "A spacecraft's thruster: a pale blue flame with a white-hot throat, which \
                 balloons into a wide, faint bell in the vacuum of space. Scale 1 is a nozzle \
                 1 m across.",
                None,
                vec![
                    plume("plume", ION_BLUE, 6.0, 3.4, 8_000.0),
                    nozzle_glow([0.75, 0.85, 1.0], 1.3, 30_000.0),
                ],
            )
        },
        effect(
            "exhaust.tailpipe",
            "A car's tailpipe at idle: faint puffs of fumes and a shimmer of hot air; raise \
             `rate` as it revs. Scale 1 is a tailpipe 6 cm across.",
            None,
            vec![
                EmitterDesc {
                    rate: 9.0,
                    shape: EmitShape::Cone {
                        angle_deg: 12.0,
                        radius: 0.02,
                    },
                    speed: [1.5, 2.5],
                    drag: 2.5,
                    acceleration: [0.0, 0.4, 0.0],
                    turbulence: 0.5,
                    turbulence_scale: 0.4,
                    lifetime: [1.2, 2.2],
                    size: [0.12, 0.2],
                    size_over_life: c(&[(0.0, 0.8), (1.0, 6.0)]),
                    soft: 0.1,
                    ..smoke("fumes", 0.62, 0.3)
                },
                EmitterDesc {
                    rate: 14.0,
                    offset: [0.0, 0.1, 0.0],
                    spread_deg: 10.0,
                    speed: [1.0, 1.8],
                    lifetime: [0.4, 0.7],
                    size: [0.15, 0.22],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.0)]),
                    ..haze("haze", 0.7)
                },
            ],
        ),
        EffectDesc {
            light: light_at([1.0, 0.6, 0.3], 9000.0, 6.0, 0.6, [0.0, 0.3, 0.0]),
            ..effect(
                "exhaust.backfire",
                "A tailpipe backfire: a bang of flame out of the pipe, a few sparks and a puff \
                 of dark smoke. Scale 1 is a tailpipe 6 cm across.",
                Some(0.06),
                vec![
                    EmitterDesc {
                        rate: 0.0,
                        bursts: vec![at(0.0, 1), at(0.03, 1)],
                        lifetime: [0.09, 0.13],
                        ..plume("flame", [1.0, 0.55, 0.22], 0.6, 0.25, 15_000.0)
                    },
                    EmitterDesc {
                        color: g(&[(0.0, [1.0, 0.8, 0.45, 1.0]), (1.0, [1.0, 0.35, 0.08, 0.0])]),
                        shape: EmitShape::Point,
                        spread_deg: 25.0,
                        lifetime: [0.2, 0.5],
                        size: [0.008, 0.015],
                        ..sparks("sparks", 10, [5.0, 10.0])
                    },
                    EmitterDesc {
                        bursts: vec![at(0.02, 4)],
                        shape: EmitShape::Cone {
                            angle_deg: 20.0,
                            radius: 0.03,
                        },
                        speed: [2.5, 4.0],
                        drag: 3.0,
                        lifetime: [0.8, 1.4],
                        size: [0.15, 0.25],
                        size_over_life: c(&[(0.0, 0.6), (1.0, 3.5)]),
                        ..smoke("smoke", 0.12, 0.5)
                    },
                ],
            )
        },
        EffectDesc {
            light: light_at(ION_BLUE, 20_000.0, 8.0, 0.3, [0.0, 0.4, 0.0]),
            ..effect(
                "exhaust.jumpjet",
                "A jump pack's jet: a short blue-white flame, a shimmer of hot air and a few \
                 sparks, for as long as it fires. Scale 1 is a nozzle 12 cm across.",
                None,
                vec![
                    plume("flame", [0.62, 0.78, 1.0], 1.1, 0.42, 12_000.0),
                    nozzle_glow([0.8, 0.88, 1.0], 0.3, 30_000.0),
                    EmitterDesc {
                        rate: 40.0,
                        offset: [0.0, 0.3, 0.0],
                        spread_deg: 8.0,
                        speed: [6.0, 9.0],
                        lifetime: [0.25, 0.4],
                        size: [0.3, 0.45],
                        size_over_life: c(&[(0.0, 0.6), (1.0, 2.0)]),
                        ..haze("haze", 0.9)
                    },
                    EmitterDesc {
                        rate: 30.0,
                        bursts: Vec::new(),
                        color: g(&[(0.0, [0.85, 0.92, 1.0, 1.0]), (1.0, [0.4, 0.6, 1.0, 0.0])]),
                        shape: EmitShape::Point,
                        spread_deg: 12.0,
                        gravity: 0.3,
                        lifetime: [0.15, 0.35],
                        size: [0.006, 0.012],
                        ..sparks("sparks", 0, [8.0, 14.0])
                    },
                ],
            )
        },
    ]
}

fn misc() -> Vec<EffectDesc> {
    let lightning_ground = EffectDesc {
        light: light([0.75, 0.82, 1.0], 400_000.0, 60.0, 0.9, 0.35),
        ..effect(
            "lightning.ground_strike",
            "Where a cloud-to-ground lightning strike lands: a blinding blue-white flash on the \
             ground, a burst of molten sparks, soil blasted up and a drifting puff of steam and \
             smoke. Scale 1 is a typical 30 kA stroke.",
            Some(0.12),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    offset: [0.0, 0.6, 0.0],
                    lifetime: [0.2, 0.2],
                    size: [5.0, 5.0],
                    size_over_life: c(&[(0.0, 0.7), (1.0, 1.3)]),
                    color: g(&[(0.0, [0.9, 0.93, 1.0, 1.0]), (1.0, [0.55, 0.65, 1.0, 0.0])]),
                    emissive: 400_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    ..em("flash")
                },
                // The ground itself glowing where the current spread out.
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    offset: [0.0, 0.05, 0.0],
                    lifetime: [0.45, 0.45],
                    size: [3.5, 3.5],
                    facing: Facing::Horizontal,
                    color: g(&[
                        (0.0, [0.85, 0.9, 1.0, 1.0]),
                        (0.3, [1.0, 0.6, 0.3, 0.7]),
                        (1.0, [1.0, 0.35, 0.1, 0.0]),
                    ]),
                    emissive: 60_000.0,
                    additive: 1.0,
                    wind: 0.0,
                    ..em("scorch")
                },
                EmitterDesc {
                    color: g(&[
                        (0.0, [0.95, 0.97, 1.0, 1.0]),
                        (0.3, [1.0, 0.8, 0.5, 1.0]),
                        (1.0, [1.0, 0.4, 0.1, 0.0]),
                    ]),
                    size: [0.025, 0.05],
                    lifetime: [0.6, 1.6],
                    ..sparks("sparks", 260, [4.0, 14.0])
                },
                // Clods of soil blown out of the crater.
                EmitterDesc {
                    bursts: once(40),
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    speed: [3.0, 8.0],
                    velocity: [0.0, 2.0, 0.0],
                    gravity: 1.0,
                    drag: 0.3,
                    collide: Collide::Die,
                    lifetime: [0.8, 1.5],
                    size: [0.04, 0.12],
                    sprite: Sprite::Shard,
                    debris: Some(Debris::Rock),
                    spin_deg: [-400.0, 400.0],
                    lit: 1.0,
                    color: g(&[(0.0, [0.2, 0.15, 0.1, 1.0]), (1.0, [0.2, 0.15, 0.1, 1.0])]),
                    soft: 0.0,
                    ..em("soil")
                },
                EmitterDesc {
                    bursts: once(24),
                    shape: EmitShape::Hemisphere { radius: 0.4 },
                    speed: [1.0, 3.5],
                    velocity: [0.0, 1.0, 0.0],
                    drag: 2.0,
                    gravity: 0.2,
                    lifetime: [1.0, 2.0],
                    size: [0.5, 0.9],
                    sprite: Sprite::Dust,
                    lit: 1.0,
                    color: fade([0.42, 0.36, 0.3], 0.6, 0.05, 0.4),
                    soft: 0.3,
                    ..em("dust")
                },
                EmitterDesc {
                    bursts: vec![at(0.05, 14)],
                    shape: EmitShape::Hemisphere { radius: 0.4 },
                    speed: [0.4, 1.2],
                    acceleration: [0.0, 1.0, 0.0],
                    lifetime: [2.5, 4.0],
                    size: [0.7, 1.1],
                    ..smoke("steam", 0.55, 0.45)
                },
            ],
        )
    };
    vec![
        lightning_ground,
        EffectDesc {
            light: light([1.0, 0.75, 0.45], 6000.0, 8.0, 0.8, 0.08),
            ..effect(
                "muzzle.flash",
                "A gun's muzzle flash. The entity's +Y axis points out of the barrel.",
                Some(0.04),
                vec![
                    EmitterDesc {
                        bursts: once(1),
                        speed: [0.0, 0.0],
                        lifetime: [0.05, 0.05],
                        size: [0.5, 0.6],
                        sprite: Sprite::Star,
                        color: g(&[(0.0, [1.0, 0.85, 0.6, 1.0]), (1.0, [1.0, 0.6, 0.3, 0.0])]),
                        emissive: 100_000.0,
                        additive: 1.0,
                        ..em("flash")
                    },
                    EmitterDesc {
                        bursts: once(4),
                        shape: EmitShape::Cone {
                            angle_deg: 8.0,
                            radius: 0.02,
                        },
                        speed: [2.0, 5.0],
                        drag: 8.0,
                        lifetime: [0.04, 0.07],
                        size: [0.15, 0.3],
                        sprite: Sprite::Flame,
                        color: flame_colors(),
                        emissive: 30_000.0,
                        additive: 1.0,
                        ..em("flame")
                    },
                    EmitterDesc {
                        shape: EmitShape::Cone {
                            angle_deg: 15.0,
                            radius: 0.0,
                        },
                        lifetime: [0.1, 0.3],
                        ..sparks("sparks", 12, [8.0, 20.0])
                    },
                    EmitterDesc {
                        bursts: once(4),
                        shape: EmitShape::Cone {
                            angle_deg: 20.0,
                            radius: 0.02,
                        },
                        speed: [0.5, 2.0],
                        drag: 3.0,
                        lifetime: [0.6, 1.2],
                        size: [0.1, 0.2],
                        ..smoke("smoke", 0.5, 0.25)
                    },
                ],
            )
        },
        EffectDesc {
            prewarm: 2.0,
            light: Some(EffectLight {
                offset: [0.0, 0.2, 0.0],
                ..light([1.0, 0.6, 0.3], 1800.0, 9.0, 0.45, 0.3).unwrap()
            }),
            ..effect(
                "fire.torch",
                "A torch or brazier flame: a hot bed over the coals, licking tongues whose tips \
                 tear off and flicker out, rising embers, a thin smoke wisp, heat haze and a \
                 flickering light.",
                None,
                vec![
                    // The body of the fire: a few overlapping blazes standing on the coals,
                    // each churning and swaying on its own, crossfading as they come and go.
                    EmitterDesc {
                        rate: 6.0,
                        offset: [0.0, -0.02, 0.0],
                        shape: EmitShape::Disc { radius: 0.08 },
                        speed: [0.0, 0.0],
                        lifetime: [0.9, 1.3],
                        size: [0.46, 0.56],
                        rotation_deg: [0.0, 0.0],
                        local_space: true,
                        steady: true,
                        sprite: Sprite::Blaze,
                        facing: Facing::Axis,
                        stretch: 1.3,
                        color: fade([1.0, 0.62, 0.24], 1.0, 0.3, 0.7),
                        emissive: 11_000.0,
                        additive: 0.45,
                        wind: 0.0,
                        soft: 0.05,
                        ..em("blaze")
                    },
                    // Tongues: rise fast on their own heat, stretch, narrow to a point and redden.
                    EmitterDesc {
                        rate: 10.0,
                        offset: [0.0, 0.12, 0.0],
                        shape: EmitShape::Disc { radius: 0.09 },
                        speed: [0.3, 0.6],
                        acceleration: [0.0, 2.5, 0.0],
                        drag: 1.4,
                        turbulence: 3.0,
                        turbulence_scale: 0.25,
                        wind: 0.3,
                        lifetime: [0.35, 0.55],
                        size: [0.26, 0.4],
                        size_over_life: c(&[(0.0, 0.55), (0.25, 1.0), (0.7, 0.7), (1.0, 0.2)]),
                        rotation_deg: [-12.0, 12.0],
                        sprite: Sprite::Flame,
                        color: g(&[
                            (0.0, [1.0, 0.72, 0.32, 0.0]),
                            (0.08, [1.0, 0.68, 0.28, 0.9]),
                            (0.35, [1.0, 0.56, 0.18, 0.5]),
                            (0.6, [1.0, 0.46, 0.12, 0.0]),
                            (1.0, [1.0, 0.4, 0.1, 0.0]),
                        ]),
                        emissive: 9_000.0,
                        additive: 0.9,
                        ..em("flames")
                    },
                    // Tips: scraps torn off the tongues' tops, flickering out within a blink.
                    EmitterDesc {
                        rate: 16.0,
                        offset: [0.0, 0.3, 0.0],
                        shape: EmitShape::Disc { radius: 0.09 },
                        speed: [0.3, 0.6],
                        acceleration: [0.0, 4.0, 0.0],
                        drag: 1.5,
                        turbulence: 4.0,
                        turbulence_scale: 0.15,
                        wind: 0.5,
                        lifetime: [0.08, 0.16],
                        size: [0.05, 0.1],
                        size_over_life: c(&[(0.0, 1.0), (1.0, 0.3)]),
                        rotation_deg: [-20.0, 20.0],
                        sprite: Sprite::Flame,
                        color: g(&[
                            (0.0, [1.0, 0.5, 0.12, 0.0]),
                            (0.15, [1.0, 0.45, 0.1, 0.9]),
                            (1.0, [0.7, 0.12, 0.02, 0.0]),
                        ]),
                        emissive: 9000.0,
                        additive: 0.8,
                        ..em("tips")
                    },
                    // Embers: swept up in the draft, zigzagging, cooling from gold to red.
                    EmitterDesc {
                        rate: 9.0,
                        shape: EmitShape::Disc { radius: 0.12 },
                        speed: [0.3, 0.8],
                        velocity: [0.0, 1.2, 0.0],
                        acceleration: [0.0, 0.5, 0.0],
                        drag: 1.0,
                        turbulence: 4.0,
                        turbulence_scale: 0.35,
                        wind: 1.0,
                        lifetime: [0.8, 1.8],
                        size: [0.014, 0.026],
                        size_over_life: c(&[(0.0, 1.0), (1.0, 0.5)]),
                        facing: Facing::Velocity,
                        stretch: 0.02,
                        sprite: Sprite::Spark,
                        color: g(&[
                            (0.0, [1.0, 0.75, 0.35, 1.0]),
                            (0.5, [1.0, 0.42, 0.1, 1.0]),
                            (1.0, [0.9, 0.16, 0.02, 0.0]),
                        ]),
                        emissive: 30_000.0,
                        additive: 1.0,
                        ..em("embers")
                    },
                    // A thin, dark wisp: many faint puffs from above the tips, spreading as it rises.
                    EmitterDesc {
                        rate: 22.0,
                        offset: [0.0, 0.55, 0.0],
                        shape: EmitShape::Disc { radius: 0.05 },
                        speed: [0.05, 0.15],
                        velocity: [0.0, 0.7, 0.0],
                        acceleration: [0.0, 0.4, 0.0],
                        turbulence: 1.6,
                        turbulence_scale: 0.4,
                        wind: 1.0,
                        lifetime: [2.5, 3.5],
                        size: [0.1, 0.14],
                        size_over_life: c(&[(0.0, 0.7), (1.0, 5.0)]),
                        ..smoke("smoke", 0.06, 0.08)
                    },
                    EmitterDesc {
                        rate: 10.0,
                        offset: [0.0, 0.4, 0.0],
                        shape: EmitShape::Disc { radius: 0.1 },
                        speed: [0.05, 0.15],
                        velocity: [0.0, 0.8, 0.0],
                        acceleration: [0.0, 0.5, 0.0],
                        lifetime: [0.8, 1.2],
                        size: [0.45, 0.65],
                        size_over_life: c(&[(0.0, 0.8), (1.0, 1.6)]),
                        ..haze("haze", 1.0)
                    },
                ],
            )
        },
        EffectDesc {
            prewarm: 4.0,
            ..effect(
                "ambient.fireflies",
                "Fireflies drifting and blinking over a 12 × 12 m area.",
                None,
                vec![EmitterDesc {
                    rate: 6.0,
                    shape: EmitShape::Box {
                        half_extents: [6.0, 1.2, 6.0],
                    },
                    speed: [0.0, 0.3],
                    spread_deg: 180.0,
                    turbulence: 1.2,
                    turbulence_scale: 2.5,
                    drag: 1.0,
                    wind: 0.3,
                    lifetime: [5.0, 8.0],
                    size: [0.04, 0.06],
                    color: g(&[
                        (0.0, [0.8, 1.0, 0.3, 0.0]),
                        (0.1, [0.8, 1.0, 0.3, 1.0]),
                        (0.45, [0.8, 1.0, 0.3, 0.15]),
                        (0.6, [0.8, 1.0, 0.3, 1.0]),
                        (1.0, [0.8, 1.0, 0.3, 0.0]),
                    ]),
                    emissive: 8000.0,
                    additive: 1.0,
                    ..em("flies")
                }],
            )
        },
    ]
}
