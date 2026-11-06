//! The six elements of the magic system (engine-design/spell-combination/magic-system.md): the
//! clean Fire, Frost and Storm and the dark Blood, Blight and Shadow. Each has the same stages,
//! named `spell.<element>.<role>` (the magic module's scheme), so a spell picks its look from its element and cast:
//!
//! - `bolt`: the projectile in flight (bolts, lobs, the head of a chain);
//! - `impact`: where it lands (one-shot; leaves the element's mark);
//! - `field`: a lasting patch of the element on the ground (areas, walls, runes; loops until
//!   stopped).
//!
//! Scale (`ParticleEffect::with_scale`) is the spell's size: an impact's blast radius / 2 m, a
//! field's radius / 2 m; bolts grow with power the same way.
//!
//! The elements share one language so they read apart at a glance, by day and by night: the
//! clean ones glow (additive light, white-hot hearts), the dark ones are matter and absence
//! (alpha-blended liquid and darkness rimmed with a little light of their color).

use super::*;

/// Fire and frost bolts and impacts already exist as `spell.*`: give them their element names
/// too, so a spell can ask for any element the same way.
pub(super) fn aliases(existing: &[EffectDesc]) -> Vec<EffectDesc> {
    let pairs = [
        ("spell.fireball", "spell.fire.bolt"),
        ("spell.frost_bolt", "spell.frost.bolt"),
        ("spell.frost_impact", "spell.frost.impact"),
    ];
    pairs
        .iter()
        .filter_map(|(from, to)| {
            existing
                .iter()
                .find(|e| e.name == *from)
                .map(|e| EffectDesc {
                    name: (*to).into(),
                    ..e.clone()
                })
        })
        .collect()
}

pub(super) fn all() -> Vec<EffectDesc> {
    vec![
        fire_impact(),
        fire_area(),
        frost_area(),
        storm_bolt(),
        storm_impact(),
        storm_area(),
        blood_bolt(),
        blood_impact(),
        blood_area(),
        blight_bolt(),
        blight_impact(),
        blight_area(),
        shadow_bolt(),
        shadow_impact(),
        shadow_area(),
    ]
}

// ---------------------------------------------------------------- palettes

const STORM: [f32; 3] = [0.62, 0.74, 1.0];
const STORM_HOT: [f32; 3] = [0.88, 0.93, 1.0];
/// Blood as it looks: a deep red that only shows red where light passes through it.
const BLOOD: [f32; 3] = [0.3, 0.012, 0.01];
const BLOOD_GLOW: [f32; 3] = [1.0, 0.07, 0.04];
const BLIGHT: [f32; 3] = [0.17, 0.4, 0.05];
const BLIGHT_GLOW: [f32; 3] = [0.38, 1.0, 0.16];
const SHADOW: [f32; 3] = [0.55, 0.22, 1.0];

/// A soft round glow lying on the ground under an area, turning slowly.
pub(super) fn ground_glow(rgb: [f32; 3], size: f32, alpha: f32, emissive: f32) -> EmitterDesc {
    let [r, gg, b] = rgb;
    EmitterDesc {
        rate: 2.0,
        offset: [0.0, 0.04, 0.0],
        speed: [0.0, 0.0],
        lifetime: [2.0, 2.0],
        size: [size, size],
        spin_deg: [-10.0, 10.0],
        local_space: true,
        steady: true,
        facing: Facing::Horizontal,
        color: g(&[
            (0.0, [r, gg, b, 0.0]),
            (0.5, [r, gg, b, alpha]),
            (1.0, [r, gg, b, 0.0]),
        ]),
        emissive,
        additive: 1.0,
        soft: 0.0,
        wind: 0.0,
        ..em("ground glow")
    }
}

/// A turning rune circle drawn on the ground in the element's color.
fn rune_circle(rgb: [f32; 3], size: f32, emissive: f32) -> EmitterDesc {
    EmitterDesc {
        rate: 1.5,
        offset: [0.0, 0.03, 0.0],
        speed: [0.0, 0.0],
        lifetime: [2.0, 2.0],
        size: [size, size],
        rotation_deg: [0.0, 0.0],
        spin_deg: [14.0, 14.0],
        local_space: true,
        steady: true,
        sprite: Sprite::Rune,
        facing: Facing::Horizontal,
        color: fade(rgb, 0.8, 0.3, 0.7),
        emissive,
        additive: 1.0,
        soft: 0.0,
        wind: 0.0,
        ..em("runes")
    }
}

// ---------------------------------------------------------------- fire and frost

fn fire_area() -> EffectDesc {
    EffectDesc {
        light: light([1.0, 0.55, 0.22], 9000.0, 12.0, 0.35, 0.6),
        prewarm: 1.5,
        ..effect(
            "spell.fire.field",
            "A patch of burning ground: tongues of flame licking up all over it, the hottest in \
             a ring at its edge, embers whirling up, smoke and heat shimmer over it.",
            None,
            vec![
                // Flames over the whole patch, taller at its rim, where the fire spreads.
                EmitterDesc {
                    rate: 54.7,
                    shape: EmitShape::Ring {
                        radius: 1.81,
                        thickness: 0.35,
                    },
                    speed: [0.0, 0.2],
                    acceleration: [0.0, 2.0, 0.0],
                    lifetime: [0.7, 1.1],
                    size: [0.9, 1.4],
                    size_over_life: c(&[(0.0, 0.5), (0.3, 1.0), (1.0, 0.7)]),
                    sprite: Sprite::Flame,
                    color: flame_colors(),
                    emissive: 18_000.0,
                    additive: 1.0,
                    soft: 0.25,
                    wind: 0.4,
                    rotation_deg: [-8.0, 8.0],
                    ..em("rim flames")
                },
                // Whole fires standing all over the patch, each churning on its own and
                // crossfading as they come and go, so the ground burns as one sheet.
                EmitterDesc {
                    rate: 16.0,
                    shape: EmitShape::Disc { radius: 1.5 },
                    offset: [0.0, -0.02, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.9, 1.4],
                    size: [1.2, 1.9],
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    steady: true,
                    sprite: Sprite::Blaze,
                    facing: Facing::Axis,
                    stretch: 1.1,
                    color: fade([1.0, 0.62, 0.24], 1.0, 0.3, 0.7),
                    emissive: 11_000.0,
                    additive: 0.45,
                    wind: 0.0,
                    soft: 0.1,
                    ..em("blazes")
                },
                // Embers whirled up by the heat.
                EmitterDesc {
                    rate: 63.1,
                    shape: EmitShape::Disc { radius: 1.89 },
                    speed: [0.3, 1.0],
                    acceleration: [0.0, 2.5, 0.0],
                    drag: 1.0,
                    turbulence: 4.0,
                    turbulence_scale: 0.8,
                    lifetime: [1.0, 2.2],
                    size: [0.015, 0.03],
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
                    rate: 10.5,
                    shape: EmitShape::Disc { radius: 1.45 },
                    offset: [0.0, 1.0, 0.0],
                    speed: [0.2, 0.4],
                    acceleration: [0.0, 1.6, 0.0],
                    lifetime: [2.5, 4.0],
                    size: [0.8, 1.2],
                    ..smoke("smoke", 0.12, 0.35)
                },
                EmitterDesc {
                    rate: 8.0,
                    shape: EmitShape::Disc { radius: 1.6 },
                    offset: [0.0, 0.6, 0.0],
                    speed: [0.2, 0.5],
                    acceleration: [0.0, 1.5, 0.0],
                    lifetime: [0.8, 1.3],
                    size: [1.4, 2.0],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.5)]),
                    ..haze("shimmer", 0.6)
                },
                ground_glow([1.0, 0.45, 0.15], 4.93, 0.5, 5000.0),
            ],
        )
    }
}

fn frost_area() -> EffectDesc {
    EffectDesc {
        light: light([0.55, 0.78, 1.0], 3500.0, 10.0, 0.05, 0.6),
        mark: mark(MarkKind::Frost, 2.3, 120.0, 0.0),
        prewarm: 2.0,
        ..effect(
            "spell.frost.field",
            "A freezing field: ice spikes standing out of frosted ground in a ring, snow motes \
             drifting down through a still, low cold mist, glints of frost catching the light.",
            None,
            vec![
                // Ice spikes thrust up round the edge and in the middle, lasting a while each and
                // regrowing, so the field always bristles.
                EmitterDesc {
                    rate: 8.4,
                    shape: EmitShape::Ring {
                        radius: 1.74,
                        thickness: 0.3,
                    },
                    speed: [0.0, 0.0],
                    lifetime: [3.0, 4.5],
                    size: [0.6, 1.0],
                    rotation_deg: [-18.0, 18.0],
                    local_space: true,
                    sprite: Sprite::Crystal,
                    facing: Facing::Axis,
                    stretch: 1.0,
                    lit: 1.0,
                    emissive: 2500.0,
                    color: g(&[
                        (0.0, [0.75, 0.9, 1.0, 1.0]),
                        (0.85, [0.75, 0.9, 1.0, 1.0]),
                        (1.0, [0.75, 0.9, 1.0, 0.0]),
                    ]),
                    soft: 0.05,
                    wind: 0.0,
                    ..em("spikes")
                },
                EmitterDesc {
                    rate: 1.0,
                    shape: EmitShape::Disc { radius: 0.58 },
                    speed: [0.0, 0.0],
                    lifetime: [3.5, 4.5],
                    size: [1.1, 1.4],
                    rotation_deg: [-8.0, 8.0],
                    local_space: true,
                    sprite: Sprite::Crystal,
                    facing: Facing::Axis,
                    stretch: 1.0,
                    lit: 1.0,
                    emissive: 3000.0,
                    color: g(&[
                        (0.0, [0.7, 0.88, 1.0, 1.0]),
                        (0.85, [0.7, 0.88, 1.0, 1.0]),
                        (1.0, [0.7, 0.88, 1.0, 0.0]),
                    ]),
                    soft: 0.05,
                    wind: 0.0,
                    ..em("heart spike")
                },
                // Snow motes drifting down, glinting.
                EmitterDesc {
                    rate: 84.1,
                    shape: EmitShape::Disc { radius: 2.32 },
                    offset: [0.0, 2.6, 0.0],
                    speed: [0.0, 0.0],
                    velocity: [0.0, -0.5, 0.0],
                    drag: 1.0,
                    turbulence: 0.6,
                    wind: 0.3,
                    lifetime: [3.0, 4.5],
                    size: [0.025, 0.045],
                    sprite: Sprite::Star,
                    color: fade([0.85, 0.95, 1.0], 1.0, 0.15, 0.7),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("snow")
                },
                // A still, cold mist lying in the field.
                EmitterDesc {
                    rate: 21.0,
                    shape: EmitShape::Disc { radius: 2.03 },
                    offset: [0.0, 0.15, 0.0],
                    speed: [0.05, 0.25],
                    lifetime: [2.5, 3.5],
                    size: [0.9, 1.4],
                    size_over_life: c(&[(0.0, 0.7), (1.0, 1.4)]),
                    sprite: Sprite::Dust,
                    emissive: 40.0,
                    color: fade([0.85, 0.93, 1.0], 0.22, 0.25, 0.6),
                    wind: 0.2,
                    ..smoke("mist", 0.85, 0.3)
                },
                ground_glow([0.5, 0.75, 1.0], 5.22, 0.18, 2500.0),
            ],
        )
    }
}

// ---------------------------------------------------------------- storm

fn storm_bolt() -> EffectDesc {
    EffectDesc {
        light: light(STORM, 9000.0, 12.0, 0.6, 0.2),
        trail: Some(Trail {
            width: 0.09,
            lifetime: 0.3,
            min_spacing: 0.08,
            color: g(&[(0.0, [0.85, 0.92, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
            emissive: 30_000.0,
            style: RibbonStyle::Lightning,
            ..Default::default()
        }),
        ..effect(
            "spell.storm.bolt",
            "A ball lightning bolt: a white-hot crackling ball of plasma with arcs lashing out of \
             it, shedding sparks and a jagged lightning trail.",
            None,
            vec![
                EmitterDesc {
                    rate: 4.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.5, 0.5],
                    size: [0.8, 0.8],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Plasma,
                    color: fade(STORM, 1.0, 0.25, 0.7),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("ball")
                },
                EmitterDesc {
                    rate: 60.0,
                    lifetime: [0.08, 0.14],
                    size: [0.22, 0.3],
                    speed: [0.0, 0.0],
                    local_space: true,
                    color: g(&[(0.0, [0.95, 0.97, 1.0, 1.0]), (1.0, [0.7, 0.8, 1.0, 0.0])]),
                    emissive: 40_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("core")
                },
                // Arcs lashing out of the ball in every direction, re-striking.
                EmitterDesc {
                    rate: 22.0,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.08, 0.16],
                    size: [0.9, 1.5],
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
                    rate: 30.0,
                    rate_per_meter: 5.0,
                    shape: EmitShape::Sphere {
                        radius: 0.15,
                        surface: true,
                    },
                    speed: [1.0, 4.0],
                    gravity: 0.6,
                    drag: 1.0,
                    lifetime: [0.2, 0.5],
                    size: [0.012, 0.022],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.03,
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("sparks")
                },
            ],
        )
    }
}

/// A fire spell landing, sized for the magic system: at scale 1 the blast is about 1.5 m across
/// its radius (the burst, the shock ring and the scorch), and the smoke stays low, rolling out
/// over the ground and thinning before it climbs past head height, so a big Ascended blast
/// reads as big without blotting out the hall. (`spell.fireball_impact` keeps the old,
/// towering explosion for scripted set pieces.)
fn fire_impact() -> EffectDesc {
    EffectDesc {
        light: light([1.0, 0.55, 0.25], 30_000.0, 9.0, 0.2, 0.6),
        mark: mark(MarkKind::Scorch, 0.9, 120.0, 8.0),
        ..effect(
            "spell.fire.impact",
            "A fire spell landing: a white-hot flash, a tight ball of flame bursting and curling \
             up, a low shock ring, bouncing sparks, embers, and smoke rolling out low over the \
             ground.",
            Some(0.12),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.18, 0.18],
                    size: [1.6, 1.6],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.6)]),
                    color: g(&[(0.0, [1.0, 0.92, 0.75, 1.0]), (1.0, [1.0, 0.45, 0.1, 0.0])]),
                    emissive: 45_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("flash")
                },
                EmitterDesc {
                    bursts: once(10),
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    speed: [1.8, 3.8],
                    drag: 5.0,
                    acceleration: [0.0, 2.0, 0.0],
                    turbulence: 2.5,
                    // Glowing plasma puffs, not the fireball sprite: its soot read as black
                    // lumps by day.
                    lifetime: [0.4, 0.7],
                    size: [0.55, 0.85],
                    size_over_life: c(&[(0.0, 0.4), (0.25, 1.0), (1.0, 1.3)]),
                    rotation_deg: [-180.0, 180.0],
                    spin_deg: [-40.0, 40.0],
                    sprite: Sprite::Plasma,
                    lit: 0.0,
                    color: g(&[
                        (0.0, [1.0, 0.85, 0.55, 1.0]),
                        (0.3, [1.0, 0.5, 0.15, 0.9]),
                        (1.0, [0.8, 0.2, 0.03, 0.0]),
                    ]),
                    emissive: 16_000.0,
                    additive: 1.0,
                    soft: 0.4,
                    ..em("fire")
                },
                EmitterDesc {
                    lifetime: [0.3, 0.7],
                    stretch: 0.03,
                    emissive: 30_000.0,
                    drag: 1.5,
                    ..sparks("sparks", 70, [3.0, 7.0])
                },
                EmitterDesc {
                    bursts: once(1),
                    offset: [0.0, 0.05, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.35, 0.35],
                    size: [0.6, 0.6],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 5.0)]),
                    rotation_deg: [0.0, 0.0],
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[(0.0, [1.0, 0.6, 0.25, 0.9]), (1.0, [1.0, 0.3, 0.05, 0.0])]),
                    emissive: 15_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("shockwave")
                },
                // Smoke rolls outwards over the ground and barely rises: light grey, thin, gone
                // in two seconds.
                EmitterDesc {
                    delay: 0.1,
                    bursts: once(9),
                    shape: EmitShape::Hemisphere { radius: 0.4 },
                    speed: [0.8, 1.6],
                    drag: 2.5,
                    acceleration: [0.0, 0.25, 0.0],
                    lifetime: [1.4, 2.2],
                    size: [0.6, 0.9],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.0)]),
                    color: smoke_colors(0.45, 0.28),
                    ..smoke("smoke", 0.45, 0.28)
                },
                EmitterDesc {
                    bursts: once(20),
                    shape: EmitShape::Sphere {
                        radius: 0.3,
                        surface: false,
                    },
                    speed: [0.8, 2.4],
                    gravity: 0.1,
                    acceleration: [0.0, 0.8, 0.0],
                    drag: 1.5,
                    turbulence: 2.5,
                    lifetime: [0.8, 1.8],
                    size: [0.025, 0.05],
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
                    bursts: once(6),
                    shape: EmitShape::Sphere {
                        radius: 0.3,
                        surface: false,
                    },
                    speed: [0.2, 0.6],
                    acceleration: [0.0, 1.0, 0.0],
                    drag: 1.0,
                    lifetime: [0.5, 0.9],
                    size: [0.8, 1.2],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.4)]),
                    ..haze("haze", 0.8)
                },
            ],
        )
    }
}

fn storm_impact() -> EffectDesc {
    EffectDesc {
        light: light(STORM_HOT, 45_000.0, 18.0, 0.7, 0.35),
        mark: mark(MarkKind::Burn, 1.3, 40.0, 2.5),
        ..effect(
            "spell.storm.impact",
            "Ball lightning bursting: a blinding flash, arcs crawling out over the surface and \
             burning a branching fern into it, a shock ring, bouncing sparks and a whiff of \
             ozone.",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.15, 0.15],
                    size: [3.0, 3.0],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 1.4)]),
                    color: g(&[(0.0, [0.95, 0.97, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 60_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("flash")
                },
                EmitterDesc {
                    bursts: vec![at(0.0, 6), at(0.15, 4), at(0.32, 3)],
                    offset: [0.0, 0.03, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.2, 0.35],
                    size: [2.0, 3.2],
                    rotation_deg: [0.0, 360.0],
                    local_space: true,
                    sprite: Sprite::Arc,
                    facing: Facing::Horizontal,
                    color: g(&[
                        (0.0, [0.9, 0.95, 1.0, 1.0]),
                        (0.6, [0.6, 0.72, 1.0, 1.0]),
                        (1.0, [0.45, 0.55, 1.0, 0.0]),
                    ]),
                    emissive: 70_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("ground arcs")
                },
                EmitterDesc {
                    bursts: once(1),
                    offset: [0.0, 0.05, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.35, 0.35],
                    size: [0.6, 0.6],
                    size_over_life: c(&[(0.0, 0.3), (1.0, 5.0)]),
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[(0.0, [0.8, 0.88, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 25_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("shock ring")
                },
                EmitterDesc {
                    color: g(&[
                        (0.0, [0.95, 0.97, 1.0, 1.0]),
                        (0.5, [0.7, 0.8, 1.0, 1.0]),
                        (1.0, [0.45, 0.55, 1.0, 0.0]),
                    ]),
                    ..sparks("sparks", 90, [3.0, 11.0])
                },
                EmitterDesc {
                    bursts: once(6),
                    shape: EmitShape::Hemisphere { radius: 0.3 },
                    speed: [0.3, 0.8],
                    acceleration: [0.0, 0.8, 0.0],
                    lifetime: [1.2, 2.0],
                    size: [0.6, 0.9],
                    sprite: Sprite::Dust,
                    color: fade([0.75, 0.8, 0.95], 0.18, 0.1, 0.4),
                    emissive: 60.0,
                    ..smoke("ozone", 0.7, 0.2)
                },
            ],
        )
    }
}

fn storm_area() -> EffectDesc {
    EffectDesc {
        light: light(STORM, 6000.0, 11.0, 0.9, 0.5),
        prewarm: 1.0,
        ..effect(
            "spell.storm.field",
            "A charged field: arcs crawling and re-striking over the ground in a ring of runes, \
             motes of plasma drifting up through it and sparks spitting where the arcs meet the \
             ground.",
            None,
            vec![
                EmitterDesc {
                    rate: 29.4,
                    shape: EmitShape::Disc { radius: 1.3 },
                    offset: [0.0, 0.04, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.15, 0.3],
                    size: [1.2, 2.2],
                    rotation_deg: [0.0, 360.0],
                    local_space: true,
                    sprite: Sprite::Arc,
                    facing: Facing::Horizontal,
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 70_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("arcs")
                },
                // Arcs leaping up out of the field, so it crackles above the ground too.
                EmitterDesc {
                    rate: 26.0,
                    shape: EmitShape::Disc { radius: 1.45 },
                    offset: [0.0, 1.1, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.1, 0.18],
                    size: [2.0, 3.0],
                    rotation_deg: [60.0, 120.0],
                    local_space: true,
                    sprite: Sprite::Arc,
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    emissive: 70_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("leaping arcs")
                },
                EmitterDesc {
                    rate: 33.6,
                    shape: EmitShape::Disc { radius: 1.89 },
                    speed: [0.1, 0.3],
                    acceleration: [0.0, 0.6, 0.0],
                    turbulence: 1.5,
                    turbulence_scale: 0.6,
                    lifetime: [1.5, 2.5],
                    size: [0.14, 0.26],
                    sprite: Sprite::Plasma,
                    color: fade(STORM, 1.0, 0.2, 0.6),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("motes")
                },
                EmitterDesc {
                    bursts: every(0.25, 6),
                    shape: EmitShape::Disc { radius: 1.45 },
                    color: g(&[(0.0, [0.95, 0.97, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    lifetime: [0.2, 0.5],
                    ..sparks("sparks", 6, [1.5, 4.0])
                },
                // The charged air: a pale blue glow standing over the field, flickering with
                // the arcs, so it reads as a column of storm by day and not only at night.
                EmitterDesc {
                    rate: 10.0,
                    shape: EmitShape::Disc { radius: 0.9 },
                    offset: [0.0, 1.0, 0.0],
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.3, 0.6],
                    size: [2.4, 3.4],
                    color: g(&[
                        (0.0, [0.6, 0.72, 1.0, 0.0]),
                        (0.2, [0.6, 0.72, 1.0, 0.3]),
                        (1.0, [0.5, 0.6, 1.0, 0.0]),
                    ]),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.3,
                    wind: 0.0,
                    ..em("charged air")
                },
                rune_circle(STORM, 4.35, 9000.0),
                ground_glow(STORM, 4.93, 0.35, 9000.0),
            ],
        )
    }
}

// ---------------------------------------------------------------- blood

/// Drops of blood: glossy, dark, falling and dying where they land.
fn blood_drops(name: &str, count: u32, speed: [f32; 2], up: f32) -> EmitterDesc {
    EmitterDesc {
        size: [0.03, 0.06],
        emissive: 40.0,
        color: g(&[(0.0, [0.3, 0.01, 0.01, 1.0]), (1.0, [0.3, 0.01, 0.01, 0.9])]),
        ..droplets(name, count, BLOOD, speed, up, Collide::Die)
    }
}

fn blood_bolt() -> EffectDesc {
    EffectDesc {
        light: light(BLOOD_GLOW, 1500.0, 7.0, 0.15, 0.2),
        trail: Some(Trail {
            width: 0.16,
            lifetime: 0.35,
            min_spacing: 0.08,
            color: g(&[(0.0, [0.35, 0.02, 0.015, 0.9]), (1.0, [0.2, 0.0, 0.0, 0.0])]),
            width_over_life: c(&[(0.0, 1.0), (1.0, 0.1)]),
            emissive: 0.0,
            additive: 0.0,
            style: RibbonStyle::Smooth,
            ..Default::default()
        }),
        ..effect(
            "spell.blood.bolt",
            "A blood bolt: a wobbling, glossy ball of blood glowing deep red from within, drops \
             whirling round it, dripping and spattering as it flies, a dark red mist behind.",
            None,
            vec![
                EmitterDesc {
                    rate: 4.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.6, 0.6],
                    size: [0.45, 0.5],
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: fade(BLOOD, 1.0, 0.2, 0.8),
                    emissive: 350.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("ball")
                },
                // A red heat in the ball's heart, seen through it.
                EmitterDesc {
                    rate: 3.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.5, 0.5],
                    size: [0.34, 0.34],
                    color: fade(BLOOD_GLOW, 0.35, 0.3, 0.7),
                    emissive: 1500.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("heart")
                },
                // Drops whirling round the ball.
                EmitterDesc {
                    rate: 40.0,
                    shape: EmitShape::Sphere {
                        radius: 0.35,
                        surface: true,
                    },
                    speed: [0.0, 0.0],
                    orbit_speed: 3.0,
                    vortex: 12.0,
                    attract: 8.0,
                    local_space: true,
                    lifetime: [0.3, 0.5],
                    size: [0.04, 0.07],
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: fade(BLOOD, 1.0, 0.2, 0.7),
                    emissive: 200.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("whirl")
                },
                // Drops shed in flight, falling and dying on the ground.
                EmitterDesc {
                    rate: 10.0,
                    rate_per_meter: 4.0,
                    bursts: Vec::new(),
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    speed: [0.2, 0.6],
                    lifetime: [0.8, 1.4],
                    ..blood_drops("drips", 0, [0.0, 0.0], 0.0)
                },
                EmitterDesc {
                    rate: 20.0,
                    rate_per_meter: 8.0,
                    shape: EmitShape::Sphere {
                        radius: 0.15,
                        surface: false,
                    },
                    speed: [0.05, 0.2],
                    lifetime: [0.6, 1.0],
                    size: [0.35, 0.55],
                    sprite: Sprite::Dust,
                    color: fade([0.4, 0.02, 0.02], 0.35, 0.1, 0.4),
                    emissive: 30.0,
                    ..smoke("mist", 0.3, 0.35)
                },
            ],
        )
    }
}

fn blood_impact() -> EffectDesc {
    EffectDesc {
        light: light(BLOOD_GLOW, 6000.0, 9.0, 0.1, 0.4),
        mark: mark(MarkKind::Blood, 0.8, 60.0, 0.0),
        ..effect(
            "spell.blood.impact",
            "A blood bolt bursting: a dark red flash, a spray of glossy drops thrown out and \
             falling, globs that roll off, and a hanging red mist; it leaves a splash of blood.",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.2, 0.2],
                    size: [1.8, 1.8],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 1.3)]),
                    color: g(&[(0.0, [1.0, 0.1, 0.05, 0.8]), (1.0, [0.6, 0.0, 0.0, 0.0])]),
                    emissive: 8000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("flash")
                },
                EmitterDesc {
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    lifetime: [0.6, 1.2],
                    ..blood_drops("spray", 110, [2.0, 7.0], 1.0)
                },
                EmitterDesc {
                    bursts: once(14),
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    speed: [1.0, 3.5],
                    gravity: 1.0,
                    collide: Collide::Die,
                    lifetime: [0.6, 1.0],
                    size: [0.09, 0.16],
                    spin_deg: [-90.0, 90.0],
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: g(&[
                        (0.0, [0.3, 0.012, 0.01, 1.0]),
                        (1.0, [0.3, 0.012, 0.01, 1.0]),
                    ]),
                    emissive: 120.0,
                    soft: 0.0,
                    ..em("globs")
                },
                EmitterDesc {
                    bursts: once(10),
                    shape: EmitShape::Hemisphere { radius: 0.4 },
                    speed: [0.3, 1.0],
                    lifetime: [1.2, 2.0],
                    size: [0.6, 1.0],
                    sprite: Sprite::Dust,
                    color: fade([0.45, 0.02, 0.02], 0.4, 0.05, 0.4),
                    emissive: 40.0,
                    ..smoke("mist", 0.3, 0.4)
                },
            ],
        )
    }
}

fn blood_area() -> EffectDesc {
    EffectDesc {
        light: light(BLOOD_GLOW, 2500.0, 8.0, 0.15, 0.5),
        mark: mark(MarkKind::Blood, 2.2, 120.0, 0.0),
        prewarm: 2.0,
        ..effect(
            "spell.blood.field",
            "A siphon: blood drawn up out of a pool in spiraling streams into a beating ball \
             above it, a crimson mist rolling over the ground and a rune ring glowing red.",
            None,
            vec![
                // Drops torn up out of the pool, spiraling in toward the ball 1.6 m above.
                EmitterDesc {
                    rate: 189.2,
                    shape: EmitShape::Ring {
                        radius: 1.74,
                        thickness: 0.4,
                    },
                    speed: [0.0, 0.2],
                    orbit_speed: 1.5,
                    acceleration: [0.0, 3.0, 0.0],
                    vortex: 3.0,
                    drag: 0.8,
                    local_space: true,
                    lifetime: [1.0, 1.4],
                    size: [0.04, 0.07],
                    size_over_life: c(&[(0.0, 0.6), (0.3, 1.0), (1.0, 0.6)]),
                    sprite: Sprite::Droplet,
                    facing: Facing::Velocity,
                    stretch: 0.06,
                    lit: 1.0,
                    color: fade(BLOOD, 1.0, 0.15, 0.85),
                    emissive: 1200.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("streams")
                },
                // The ball the blood gathers in: pulses as it is fed.
                EmitterDesc {
                    rate: 3.0,
                    steady: true,
                    offset: [0.0, 1.7, 0.0],
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.8, 0.8],
                    size: [0.55, 0.65],
                    size_over_life: c(&[(0.0, 0.9), (0.5, 1.1), (1.0, 0.9)]),
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: fade(BLOOD, 1.0, 0.2, 0.8),
                    emissive: 300.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("ball")
                },
                EmitterDesc {
                    rate: 3.0,
                    steady: true,
                    offset: [0.0, 1.7, 0.0],
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.6, 0.6],
                    size: [0.5, 0.5],
                    color: fade(BLOOD_GLOW, 0.35, 0.3, 0.7),
                    emissive: 1500.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("heart")
                },
                EmitterDesc {
                    rate: 16.8,
                    shape: EmitShape::Disc { radius: 1.89 },
                    offset: [0.0, 0.15, 0.0],
                    speed: [0.05, 0.25],
                    lifetime: [2.5, 3.5],
                    size: [0.9, 1.4],
                    size_over_life: c(&[(0.0, 0.7), (1.0, 1.4)]),
                    sprite: Sprite::Dust,
                    color: fade([0.42, 0.02, 0.02], 0.3, 0.25, 0.6),
                    emissive: 35.0,
                    wind: 0.2,
                    ..smoke("mist", 0.3, 0.3)
                },
                rune_circle(BLOOD_GLOW, 4.35, 1800.0),
            ],
        )
    }
}

// ---------------------------------------------------------------- blight

fn blight_bolt() -> EffectDesc {
    EffectDesc {
        light: light(BLIGHT_GLOW, 2500.0, 8.0, 0.15, 0.2),
        ..effect(
            "spell.blight.bolt",
            "A blight bolt: a seething glob of sickly green ooze glowing from within, dripping as \
             it flies and leaving a trail of poison fumes that hang in the air.",
            None,
            vec![
                EmitterDesc {
                    rate: 4.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.6, 0.6],
                    size: [0.45, 0.5],
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: fade(BLIGHT, 1.0, 0.2, 0.8),
                    emissive: 1200.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("glob")
                },
                EmitterDesc {
                    rate: 3.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.5, 0.5],
                    size: [0.8, 0.8],
                    color: fade(BLIGHT_GLOW, 0.4, 0.3, 0.7),
                    emissive: 2500.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("glow")
                },
                EmitterDesc {
                    rate: 14.0,
                    rate_per_meter: 4.0,
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    speed: [0.2, 0.6],
                    gravity: 1.0,
                    collide: Collide::Die,
                    lifetime: [0.8, 1.4],
                    size: [0.04, 0.08],
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: g(&[(0.0, [0.17, 0.4, 0.05, 1.0]), (1.0, [0.17, 0.4, 0.05, 1.0])]),
                    emissive: 600.0,
                    soft: 0.0,
                    ..em("drips")
                },
                // Poison fumes left hanging along its path, slowly spreading.
                EmitterDesc {
                    rate: 25.0,
                    rate_per_meter: 10.0,
                    shape: EmitShape::Sphere {
                        radius: 0.15,
                        surface: false,
                    },
                    speed: [0.05, 0.25],
                    lifetime: [1.5, 2.5],
                    size: [0.4, 0.6],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.2)]),
                    color: g(&[
                        (0.0, [0.26, 0.52, 0.1, 0.0]),
                        (0.1, [0.26, 0.52, 0.1, 0.45]),
                        (0.5, [0.2, 0.38, 0.08, 0.3]),
                        (1.0, [0.16, 0.26, 0.07, 0.0]),
                    ]),
                    emissive: 30.0,
                    erode: 0.8,
                    erode_glow: 600.0,
                    ..smoke("fumes", 0.45, 0.4)
                },
            ],
        )
    }
}

fn blight_impact() -> EffectDesc {
    EffectDesc {
        light: light(BLIGHT_GLOW, 6000.0, 9.0, 0.1, 0.5),
        mark: mark(MarkKind::Rot, 1.2, 60.0, 8.0),
        ..effect(
            "spell.blight.impact",
            "A blight bolt bursting: a splash of glowing green ooze, gobs flung out, bubbles \
             boiling up and a poison cloud rolling low over the ground and lingering; it leaves \
             rot eating into the surface.",
            Some(1.5),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.2, 0.2],
                    size: [2.0, 2.0],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 1.3)]),
                    color: g(&[
                        (0.0, [0.45, 1.0, 0.25, 0.9]),
                        (1.0, [0.15, 0.55, 0.05, 0.0]),
                    ]),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    duration: Some(0.1),
                    ..em("flash")
                },
                EmitterDesc {
                    bursts: once(60),
                    shape: EmitShape::Hemisphere { radius: 0.2 },
                    speed: [2.0, 6.0],
                    gravity: 1.0,
                    collide: Collide::Die,
                    lifetime: [0.6, 1.2],
                    size: [0.05, 0.12],
                    sprite: Sprite::Goo,
                    lit: 1.0,
                    color: g(&[(0.0, [0.17, 0.4, 0.05, 1.0]), (1.0, [0.17, 0.4, 0.05, 1.0])]),
                    emissive: 700.0,
                    soft: 0.0,
                    duration: Some(0.1),
                    ..em("splash")
                },
                EmitterDesc {
                    rate: 10.0,
                    shape: EmitShape::Disc { radius: 0.6 },
                    speed: [0.0, 0.0],
                    velocity: [0.0, 0.15, 0.0],
                    lifetime: [0.4, 0.8],
                    size: [0.08, 0.18],
                    sprite: Sprite::Bubble,
                    lit: 1.0,
                    color: g(&[
                        (0.0, [0.25, 0.55, 0.08, 0.9]),
                        (1.0, [0.25, 0.55, 0.08, 0.9]),
                    ]),
                    emissive: 500.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("bubbles")
                },
                // The poison cloud: heavier than air, it spreads low and lingers.
                EmitterDesc {
                    rate: 14.0,
                    shape: EmitShape::Disc { radius: 0.5 },
                    offset: [0.0, 0.2, 0.0],
                    speed: [0.8, 1.8],
                    drag: 1.2,
                    lifetime: [2.5, 4.0],
                    size: [0.8, 1.2],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.0)]),
                    color: g(&[
                        (0.0, [0.26, 0.52, 0.1, 0.0]),
                        (0.1, [0.26, 0.52, 0.1, 0.5]),
                        (0.6, [0.2, 0.38, 0.08, 0.35]),
                        (1.0, [0.16, 0.26, 0.07, 0.0]),
                    ]),
                    emissive: 30.0,
                    erode: 0.8,
                    erode_glow: 600.0,
                    ..smoke("cloud", 0.45, 0.5)
                },
            ],
        )
    }
}

fn blight_area() -> EffectDesc {
    EffectDesc {
        light: light(BLIGHT_GLOW, 2500.0, 8.0, 0.2, 0.5),
        mark: mark(MarkKind::Rot, 2.3, 120.0, 0.0),
        prewarm: 3.0,
        ..effect(
            "spell.blight.field",
            "A poisoned pool: rot spreading over the ground, glowing green ooze boiling with \
             bubbles that burst, a heavy green miasma rolling over it and spores drifting up.",
            None,
            vec![
                EmitterDesc {
                    rate: 46.3,
                    shape: EmitShape::Disc { radius: 1.59 },
                    offset: [0.0, 0.03, 0.0],
                    speed: [0.0, 0.0],
                    velocity: [0.0, 0.08, 0.0],
                    lifetime: [0.6, 1.2],
                    size: [0.14, 0.34],
                    sprite: Sprite::Bubble,
                    lit: 1.0,
                    color: g(&[
                        (0.0, [0.25, 0.55, 0.08, 0.9]),
                        (1.0, [0.25, 0.55, 0.08, 0.9]),
                    ]),
                    emissive: 500.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("bubbles")
                },
                EmitterDesc {
                    rate: 21.0,
                    shape: EmitShape::Disc { radius: 1.89 },
                    offset: [0.0, 0.25, 0.0],
                    speed: [0.1, 0.35],
                    drag: 1.0,
                    lifetime: [3.0, 4.5],
                    size: [1.0, 1.5],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.6)]),
                    color: g(&[
                        (0.0, [0.16, 0.3, 0.07, 0.0]),
                        (0.2, [0.16, 0.3, 0.07, 0.45]),
                        (0.7, [0.13, 0.22, 0.06, 0.35]),
                        (1.0, [0.1, 0.16, 0.05, 0.0]),
                    ]),
                    emissive: 30.0,
                    wind: 0.3,
                    erode: 0.7,
                    erode_glow: 600.0,
                    ..smoke("miasma", 0.3, 0.5)
                },
                EmitterDesc {
                    rate: 37.8,
                    shape: EmitShape::Disc { radius: 1.89 },
                    offset: [0.0, 0.2, 0.0],
                    speed: [0.05, 0.2],
                    acceleration: [0.0, 0.3, 0.0],
                    turbulence: 1.0,
                    drag: 1.5,
                    lifetime: [2.5, 4.0],
                    size: [0.025, 0.045],
                    color: fade(BLIGHT_GLOW, 1.0, 0.2, 0.6),
                    emissive: 6000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("spores")
                },
                ground_glow(BLIGHT_GLOW, 4.64, 0.22, 1500.0),
            ],
        )
    }
}

// ---------------------------------------------------------------- shadow

fn shadow_bolt() -> EffectDesc {
    EffectDesc {
        light: light(SHADOW, 900.0, 6.0, 0.2, 0.2),
        trail: Some(Trail {
            width: 0.3,
            lifetime: 0.45,
            min_spacing: 0.08,
            color: g(&[
                (0.0, [0.02, 0.0, 0.04, 0.85]),
                (1.0, [0.02, 0.0, 0.04, 0.0]),
            ]),
            width_over_life: c(&[(0.0, 1.0), (1.0, 0.3)]),
            emissive: 0.0,
            additive: 0.0,
            style: RibbonStyle::Smoke,
            ..Default::default()
        }),
        ..effect(
            "spell.shadow.bolt",
            "A shadow bolt: a ball of darkness that eats the light around it, a violet corona \
             writhing round its edge, black wisps tearing off behind it and motes of light being \
             sucked into it.",
            None,
            vec![
                EmitterDesc {
                    rate: 4.0,
                    steady: true,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.6, 0.6],
                    size: [0.65, 0.65],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Void,
                    color: fade(SHADOW, 1.0, 0.25, 0.75),
                    emissive: 3500.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("void")
                },
                // Black wisps tearing off behind it and curling away.
                EmitterDesc {
                    rate: 26.0,
                    rate_per_meter: 9.0,
                    shape: EmitShape::Sphere {
                        radius: 0.15,
                        surface: false,
                    },
                    speed: [0.1, 0.4],
                    turbulence: 2.0,
                    turbulence_scale: 0.5,
                    drag: 1.5,
                    lifetime: [0.6, 1.0],
                    size: [0.4, 0.7],
                    rotation_deg: [0.0, 360.0],
                    spin_deg: [-90.0, 90.0],
                    sprite: Sprite::Wisp,
                    erode: 0.5,
                    erode_glow: 2500.0,
                    color: fade(SHADOW, 0.9, 0.1, 0.5),
                    emissive: 1800.0,
                    soft: 0.1,
                    ..em("wisps")
                },
                // Motes of light drawn in and swallowed.
                EmitterDesc {
                    rate: 60.0,
                    shape: EmitShape::Sphere {
                        radius: 1.2,
                        surface: true,
                    },
                    speed: [0.0, 0.0],
                    attract: 18.0,
                    vortex: 4.0,
                    drag: 1.0,
                    local_space: true,
                    lifetime: [0.4, 0.6],
                    size: [0.015, 0.025],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.02,
                    color: g(&[
                        (0.0, [0.6, 0.35, 1.0, 0.0]),
                        (0.5, [0.75, 0.5, 1.0, 1.0]),
                        (1.0, [0.5, 0.25, 1.0, 0.0]),
                    ]),
                    emissive: 15_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("swallowed")
                },
            ],
        )
    }
}

fn shadow_impact() -> EffectDesc {
    EffectDesc {
        light: light(SHADOW, 3500.0, 9.0, 0.2, 0.5),
        mark: mark(MarkKind::Void, 1.3, 60.0, 4.0),
        ..effect(
            "spell.shadow.impact",
            "A shadow bolt bursting: light sucked into a point, then a burst of darkness, black \
             wisps flung out and rising, a violet ring and sparks; it burns a black heart with \
             glowing cracks into the surface.",
            Some(0.4),
            vec![
                // The pull: motes from all round drawn into the point of impact.
                EmitterDesc {
                    bursts: once(80),
                    shape: EmitShape::Sphere {
                        radius: 1.6,
                        surface: true,
                    },
                    speed: [0.0, 0.0],
                    attract: 40.0,
                    drag: 1.0,
                    local_space: true,
                    lifetime: [0.22, 0.3],
                    size: [0.015, 0.03],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.025,
                    color: g(&[
                        (0.0, [0.6, 0.35, 1.0, 0.0]),
                        (0.4, [0.8, 0.6, 1.0, 1.0]),
                        (1.0, [0.6, 0.35, 1.0, 1.0]),
                    ]),
                    emissive: 20_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("pull")
                },
                // The burst of darkness.
                EmitterDesc {
                    bursts: vec![at(0.22, 1)],
                    speed: [0.0, 0.0],
                    lifetime: [0.5, 0.5],
                    size: [1.2, 1.2],
                    size_over_life: c(&[(0.0, 0.4), (0.3, 1.4), (1.0, 1.8)]),
                    sprite: Sprite::Void,
                    color: g(&[
                        (0.0, [0.55, 0.22, 1.0, 1.0]),
                        (0.6, [0.55, 0.22, 1.0, 0.8]),
                        (1.0, [0.55, 0.22, 1.0, 0.0]),
                    ]),
                    emissive: 5000.0,
                    soft: 0.0,
                    ..em("burst")
                },
                EmitterDesc {
                    bursts: vec![at(0.22, 24)],
                    shape: EmitShape::Hemisphere { radius: 0.3 },
                    speed: [1.5, 3.5],
                    drag: 2.0,
                    acceleration: [0.0, 1.2, 0.0],
                    turbulence: 2.0,
                    lifetime: [0.8, 1.4],
                    size: [0.5, 0.9],
                    rotation_deg: [0.0, 360.0],
                    spin_deg: [-60.0, 60.0],
                    sprite: Sprite::Wisp,
                    erode: 0.5,
                    erode_glow: 2500.0,
                    color: fade(SHADOW, 0.95, 0.05, 0.5),
                    emissive: 2200.0,
                    soft: 0.15,
                    ..em("wisps")
                },
                EmitterDesc {
                    bursts: vec![at(0.22, 1)],
                    offset: [0.0, 0.05, 0.0],
                    speed: [0.0, 0.0],
                    lifetime: [0.45, 0.45],
                    size: [0.6, 0.6],
                    size_over_life: c(&[(0.0, 0.3), (1.0, 5.5)]),
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[(0.0, [0.75, 0.5, 1.0, 1.0]), (1.0, [0.5, 0.2, 1.0, 0.0])]),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("ring")
                },
                EmitterDesc {
                    bursts: vec![at(0.22, 40)],
                    color: g(&[
                        (0.0, [0.85, 0.7, 1.0, 1.0]),
                        (0.5, [0.6, 0.3, 1.0, 1.0]),
                        (1.0, [0.4, 0.1, 1.0, 0.0]),
                    ]),
                    emissive: 15_000.0,
                    ..sparks("sparks", 40, [2.0, 6.0])
                },
            ],
        )
    }
}

fn shadow_area() -> EffectDesc {
    EffectDesc {
        light: light(SHADOW, 1200.0, 7.0, 0.3, 0.5),
        mark: mark(MarkKind::Void, 2.2, 120.0, 0.0),
        prewarm: 2.0,
        ..effect(
            "spell.shadow.field",
            "A pool of darkness: a black pit lying on the ground with a violet corona, black \
             wisps rising out of it like dark flames, and motes of light sinking into it.",
            None,
            vec![
                // The pit: darkness lying flat on the ground.
                EmitterDesc {
                    rate: 2.0,
                    steady: true,
                    offset: [0.0, 0.03, 0.0],
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [1.5, 1.5],
                    size: [3.8, 3.8],
                    spin_deg: [-8.0, 8.0],
                    sprite: Sprite::Void,
                    facing: Facing::Horizontal,
                    color: fade(SHADOW, 1.0, 0.3, 0.7),
                    emissive: 2500.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("pit")
                },
                // Dark flames: wisps standing up out of the pit.
                EmitterDesc {
                    rate: 29.4,
                    shape: EmitShape::Disc { radius: 1.3 },
                    speed: [0.2, 0.5],
                    acceleration: [0.0, 0.8, 0.0],
                    drag: 1.0,
                    turbulence: 0.8,
                    lifetime: [1.2, 1.8],
                    size: [0.9, 1.4],
                    size_over_life: c(&[(0.0, 0.5), (0.4, 1.0), (1.0, 1.2)]),
                    rotation_deg: [-10.0, 10.0],
                    sprite: Sprite::Wisp,
                    erode: 0.5,
                    erode_glow: 2500.0,
                    color: fade(SHADOW, 0.95, 0.15, 0.55),
                    emissive: 1500.0,
                    soft: 0.25,
                    wind: 0.3,
                    ..em("dark flames")
                },
                // Motes of light sinking into it.
                EmitterDesc {
                    rate: 84.1,
                    shape: EmitShape::Disc { radius: 2.61 },
                    offset: [0.0, 2.0, 0.0],
                    speed: [0.0, 0.0],
                    attract: 3.0,
                    vortex: 1.5,
                    drag: 0.8,
                    local_space: true,
                    lifetime: [1.5, 2.2],
                    size: [0.02, 0.035],
                    color: g(&[
                        (0.0, [0.7, 0.45, 1.0, 0.0]),
                        (0.3, [0.7, 0.45, 1.0, 1.0]),
                        (1.0, [0.5, 0.2, 1.0, 0.0]),
                    ]),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("motes")
                },
                rune_circle(SHADOW, 4.64, 1500.0),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_element_has_a_bolt_an_impact_and_a_field() {
        let all = super::super::all();
        for element in ["fire", "frost", "storm", "blood", "blight", "shadow"] {
            for role in ["bolt", "impact", "field"] {
                let name = format!("spell.{element}.{role}");
                let e = all
                    .iter()
                    .find(|e| e.name == name)
                    .unwrap_or_else(|| panic!("{name} is missing"));
                e.validate().unwrap();
                assert_eq!(
                    e.is_one_shot(),
                    role == "impact",
                    "{name}: impacts end by themselves, bolts and fields loop"
                );
            }
        }
    }
}
