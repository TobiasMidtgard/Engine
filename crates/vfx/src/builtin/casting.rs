//! The rest of a spell's life the magic module asks for by name:
//!
//! - `spell.<look>.charge`: gathering in the caster's hand before release (anticipation: motes
//!   converging on a growing heart of the element; loops until stopped);
//! - `spell.<look>.cone`: one gout of a breathed cone, fired along the effect's up axis
//!   (one-shot; magic strings four of them along the cone, growing);
//! - `spell.<look>.wall`: the air round one ~1.5 m piece of a spell wall (magic stands its own
//!   slab, coals, post or veil inside it): a flame curtain, chill fog, crackle; round, so the
//!   piece's facing doesn't matter;
//! - `spell.<look>.fizzle` (and `spell.fizzle`): a cast that failed, tinted by the spell;
//! - `spell.status.<status>`: what a body suffers, played at its middle while it lasts
//!   (burning, chilled, frozen, shocked, bleeding, rot, dread).
//!
//! Status looks follow the readability rules in engine-design/particles-research.md (round
//! 30): each status keeps to its own part of the body (flames from the legs to the chest, blood
//! mist at the shoulders, rot round the belly, dread above the head), stays small enough that
//! the body still reads, and grows by count and brightness rather than by changing shape.

use super::*;

const ELEMENTS: [&str; 6] = ["fire", "frost", "storm", "blood", "blight", "shadow"];

/// The elements' signature colors (magic's `Element::color`), for tinting fizzles.
const ELEMENT_COLORS: [[f32; 3]; 6] = [
    [1.0, 0.55, 0.2],
    [0.55, 0.78, 1.0],
    [0.62, 0.74, 1.0],
    [1.0, 0.07, 0.04],
    [0.38, 1.0, 0.16],
    [0.55, 0.22, 1.0],
];

/// Each authored weave and the element whose charge and cone it borrows, recolored.
const WEAVES: [(&str, &str, [f32; 3]); 8] = [
    ("steam", "frost", [0.92, 0.95, 1.0]),
    ("plasma", "storm", [1.0, 0.55, 1.0]),
    ("tempest", "frost", [0.55, 0.8, 1.0]),
    ("aether", "storm", [0.85, 0.75, 1.0]),
    ("plague", "blight", [0.45, 0.42, 0.08]),
    ("nightfeed", "shadow", [0.7, 0.04, 0.3]),
    ("wither", "blight", [0.42, 0.48, 0.2]),
    ("doom", "shadow", [0.8, 0.05, 0.12]),
];

pub(super) fn all(existing: &[EffectDesc]) -> Vec<EffectDesc> {
    let converge = existing
        .iter()
        .find(|e| e.name == "spell.charge")
        .and_then(|e| e.emitters.iter().find(|l| l.name == "converge"))
        .cloned()
        .expect("spell.charge has a converge layer");
    let mut v = vec![
        fire_charge(&converge),
        frost_charge(&converge),
        storm_charge(&converge),
        blood_charge(&converge),
        blight_charge(&converge),
        shadow_charge(&converge),
        fire_cone(),
        fire_jet(),
        frost_cone(),
        storm_cone(),
        blood_cone(),
        blight_cone(),
        shadow_cone(),
    ];
    v.extend(walls());
    for (weave, base, color) in WEAVES {
        for role in ["charge", "cone", "wall"] {
            let from = format!("spell.{base}.{role}");
            let mut e = v
                .iter()
                .find(|e| e.name == from)
                .cloned()
                .expect("element charge and cone exist");
            e.recolor(color, 0.65);
            e.name = format!("spell.{weave}.{role}");
            e.description = format!("The {weave} weave's {role}: {}", e.description);
            v.push(e);
        }
    }
    let fizzle = fizzle();
    let looks = ELEMENTS
        .iter()
        .zip(ELEMENT_COLORS)
        .map(|(e, c)| (*e, c))
        .chain(WEAVES.iter().map(|w| (w.0, w.2)));
    for (look, color) in looks {
        let mut e = fizzle.clone();
        e.recolor(color, 0.6);
        e.name = format!("spell.{look}.fizzle");
        v.push(e);
    }
    v.push(EffectDesc {
        name: "spell.fizzle.fizzle".into(),
        ..fizzle.clone()
    });
    v.push(fizzle);
    v.extend(statuses());
    v
}

// ---------------------------------------------------------------- charges

/// The converging motes of `spell.charge`, in `rgb`.
fn motes(converge: &EmitterDesc, rgb: [f32; 3], emissive: f32) -> EmitterDesc {
    let [r, gg, b] = rgb;
    EmitterDesc {
        color: g(&[
            (0.0, [r * 0.6, gg * 0.6, b * 0.6, 0.0]),
            (0.45, [r * 0.8, gg * 0.8, b * 0.8, 0.7]),
            (0.85, [r, gg, b, 1.0]),
            (1.0, [1.0, 1.0, 1.0, 0.0]),
        ]),
        emissive,
        ..converge.clone()
    }
}

/// A charge: `layers` gathering in the hand, the lead layer the heart.
fn charge(
    name: &str,
    description: &str,
    rgb: [f32; 3],
    lumens: f32,
    layers: Vec<EmitterDesc>,
) -> EffectDesc {
    EffectDesc {
        light: light(rgb, lumens, 6.0, 0.3, 0.2),
        ..effect(name, description, None, layers)
    }
}

/// A heart that swells as the charge builds: copies crossfading so it never blinks.
fn swelling(
    name: &str,
    sprite: Sprite,
    rgb: [f32; 3],
    size: f32,
    emissive: f32,
    additive: f32,
) -> EmitterDesc {
    EmitterDesc {
        rate: 4.0,
        steady: true,
        speed: [0.0, 0.0],
        local_space: true,
        lifetime: [0.6, 0.6],
        size: [size, size],
        size_over_life: c(&[(0.0, 0.7), (1.0, 1.1)]),
        rotation_deg: [0.0, 360.0],
        sprite,
        lit: 1.0 - additive,
        color: fade(rgb, 1.0, 0.25, 0.7),
        emissive,
        additive,
        soft: 0.0,
        wind: 0.0,
        ..em(name)
    }
}

fn fire_charge(converge: &EmitterDesc) -> EffectDesc {
    charge(
        "spell.fire.charge",
        "Fire gathering in the hand: embers drawn in on spiraling paths to a churning ball of \
         flame with small tongues licking up off it.",
        [1.0, 0.55, 0.2],
        2500.0,
        vec![
            swelling(
                "heart",
                Sprite::Fireball,
                [1.0, 0.62, 0.25],
                0.5,
                9000.0,
                1.0,
            ),
            EmitterDesc {
                rate: 30.0,
                shape: EmitShape::Sphere {
                    radius: 0.12,
                    surface: true,
                },
                speed: [0.0, 0.2],
                acceleration: [0.0, 1.5, 0.0],
                lifetime: [0.25, 0.4],
                size: [0.15, 0.25],
                sprite: Sprite::Flame,
                color: flame_colors(),
                emissive: 16_000.0,
                additive: 1.0,
                soft: 0.0,
                wind: 0.3,
                ..em("tongues")
            },
            motes(converge, [1.0, 0.6, 0.25], 20_000.0),
        ],
    )
}

fn frost_charge(converge: &EmitterDesc) -> EffectDesc {
    charge(
        "spell.frost.charge",
        "Frost gathering in the hand: snow drawn in to a pale blue heart, ice shards circling \
         it and cold mist sliding off.",
        [0.55, 0.78, 1.0],
        1500.0,
        vec![
            swelling("heart", Sprite::Plasma, [0.7, 0.88, 1.0], 0.45, 5000.0, 1.0),
            EmitterDesc {
                rate: 8.0,
                shape: EmitShape::Sphere {
                    radius: 0.3,
                    surface: true,
                },
                speed: [0.0, 0.0],
                orbit_speed: 3.0,
                local_space: true,
                lifetime: [0.6, 0.9],
                size: [0.08, 0.14],
                rotation_deg: [0.0, 360.0],
                sprite: Sprite::Crystal,
                lit: 1.0,
                color: fade([0.75, 0.9, 1.0], 1.0, 0.2, 0.7),
                emissive: 2500.0,
                soft: 0.0,
                wind: 0.0,
                ..em("shards")
            },
            EmitterDesc {
                rate: 6.0,
                shape: EmitShape::Sphere {
                    radius: 0.12,
                    surface: false,
                },
                speed: [0.05, 0.15],
                gravity: 0.2,
                lifetime: [0.8, 1.2],
                size: [0.2, 0.35],
                sprite: Sprite::Dust,
                color: fade([0.85, 0.93, 1.0], 0.25, 0.2, 0.5),
                emissive: 60.0,
                ..smoke("mist", 0.85, 0.25)
            },
            motes(converge, [0.75, 0.9, 1.0], 15_000.0),
        ],
    )
}

fn storm_charge(converge: &EmitterDesc) -> EffectDesc {
    charge(
        "spell.storm.charge",
        "Lightning gathering in the hand: sparks drawn in to a crackling white-blue ball, arcs \
         lashing out of it and snapping back.",
        [0.62, 0.74, 1.0],
        4000.0,
        vec![
            swelling(
                "heart",
                Sprite::Plasma,
                [0.75, 0.85, 1.0],
                0.45,
                12_000.0,
                1.0,
            ),
            EmitterDesc {
                rate: 18.0,
                speed: [0.0, 0.0],
                local_space: true,
                lifetime: [0.06, 0.12],
                size: [0.4, 0.7],
                rotation_deg: [0.0, 360.0],
                sprite: Sprite::Arc,
                color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.55, 0.65, 1.0, 0.0])]),
                emissive: 50_000.0,
                additive: 1.0,
                soft: 0.0,
                wind: 0.0,
                ..em("arcs")
            },
            motes(converge, [0.75, 0.85, 1.0], 22_000.0),
        ],
    )
}

fn blood_charge(converge: &EmitterDesc) -> EffectDesc {
    charge(
        "spell.blood.charge",
        "Blood gathering in the hand: drops drawn in on spiraling paths to a glossy, beating \
         ball of blood glowing deep red inside.",
        [1.0, 0.07, 0.04],
        900.0,
        vec![
            swelling("ball", Sprite::Goo, [0.3, 0.012, 0.01], 0.32, 400.0, 0.0),
            EmitterDesc {
                rate: 50.0,
                shape: EmitShape::Sphere {
                    radius: 0.9,
                    surface: true,
                },
                speed: [0.0, 0.0],
                attract: 12.0,
                vortex: 6.0,
                drag: 1.0,
                local_space: true,
                lifetime: [0.5, 0.7],
                size: [0.03, 0.05],
                sprite: Sprite::Goo,
                lit: 1.0,
                color: fade([0.3, 0.012, 0.01], 1.0, 0.2, 0.9),
                emissive: 250.0,
                soft: 0.0,
                wind: 0.0,
                ..em("drops")
            },
            motes(converge, [1.0, 0.1, 0.06], 6000.0),
        ],
    )
}

fn blight_charge(converge: &EmitterDesc) -> EffectDesc {
    charge(
        "spell.blight.charge",
        "Blight gathering in the hand: a seething glob of green ooze swelling, bubbling, and \
         fumes curling off it.",
        [0.38, 1.0, 0.16],
        1200.0,
        vec![
            swelling("glob", Sprite::Goo, [0.17, 0.4, 0.05], 0.32, 1200.0, 0.0),
            EmitterDesc {
                rate: 8.0,
                shape: EmitShape::Sphere {
                    radius: 0.12,
                    surface: false,
                },
                speed: [0.05, 0.2],
                acceleration: [0.0, 0.4, 0.0],
                lifetime: [0.8, 1.3],
                size: [0.18, 0.3],
                color: g(&[
                    (0.0, [0.26, 0.52, 0.1, 0.0]),
                    (0.2, [0.26, 0.52, 0.1, 0.4]),
                    (1.0, [0.16, 0.26, 0.07, 0.0]),
                ]),
                emissive: 30.0,
                erode: 0.7,
                erode_glow: 600.0,
                ..smoke("fumes", 0.3, 0.4)
            },
            motes(converge, [0.45, 1.0, 0.2], 9000.0),
        ],
    )
}

fn shadow_charge(converge: &EmitterDesc) -> EffectDesc {
    charge(
        "spell.shadow.charge",
        "Darkness gathering in the hand: light drawn in and swallowed by a growing ball of \
         darkness, black wisps curling in round it.",
        [0.55, 0.22, 1.0],
        500.0,
        vec![
            swelling("void", Sprite::Void, [0.55, 0.22, 1.0], 0.45, 3500.0, 0.0),
            EmitterDesc {
                rate: 14.0,
                shape: EmitShape::Sphere {
                    radius: 0.6,
                    surface: true,
                },
                speed: [0.0, 0.0],
                attract: 6.0,
                vortex: 4.0,
                drag: 1.2,
                local_space: true,
                lifetime: [0.5, 0.8],
                size: [0.2, 0.35],
                rotation_deg: [0.0, 360.0],
                sprite: Sprite::Wisp,
                erode: 0.5,
                erode_glow: 2500.0,
                color: fade([0.55, 0.22, 1.0], 0.9, 0.2, 0.6),
                emissive: 1500.0,
                soft: 0.0,
                wind: 0.0,
                ..em("wisps")
            },
            motes(converge, [0.75, 0.5, 1.0], 15_000.0),
        ],
    )
}

// ---------------------------------------------------------------- cones

/// A gout of a cone: particles thrown along +Y in a cone of `angle` degrees.
fn gout(name: &str, count: u32, angle: f32, speed: [f32; 2]) -> EmitterDesc {
    EmitterDesc {
        bursts: vec![at(0.0, count.div_ceil(2)), at(0.1, count / 2)],
        shape: EmitShape::Cone {
            angle_deg: angle,
            radius: 0.15,
        },
        speed,
        drag: 2.0,
        ..em(name)
    }
}

fn cone(
    name: &str,
    description: &str,
    light: Option<EffectLight>,
    layers: Vec<EmitterDesc>,
) -> EffectDesc {
    EffectDesc {
        light,
        ..effect(name, description, Some(0.2), layers)
    }
}

fn fire_cone() -> EffectDesc {
    cone(
        "spell.fire.cone",
        "A gout of breathed fire: rolling flames thrown forward, swelling and curling up as \
         they slow, embers and a trail of smoke.",
        light([1.0, 0.55, 0.2], 9000.0, 9.0, 0.4, 0.4),
        vec![
            EmitterDesc {
                lifetime: [0.45, 0.7],
                size: [0.5, 0.8],
                size_over_life: c(&[(0.0, 0.5), (0.4, 1.3), (1.0, 1.8)]),
                acceleration: [0.0, 0.0, 0.0],
                rotation_deg: [0.0, 360.0],
                spin_deg: [-90.0, 90.0],
                sprite: Sprite::Fireball,
                color: flame_colors(),
                emissive: 16_000.0,
                additive: 1.0,
                soft: 0.2,
                ..gout("flames", 16, 14.0, [5.0, 8.0])
            },
            EmitterDesc {
                lifetime: [0.5, 1.0],
                size: [0.015, 0.03],
                sprite: Sprite::Spark,
                facing: Facing::Velocity,
                stretch: 0.03,
                color: spark_colors(),
                emissive: 22_000.0,
                additive: 1.0,
                soft: 0.0,
                gravity: 0.3,
                ..gout("embers", 30, 18.0, [5.0, 10.0])
            },
            EmitterDesc {
                lifetime: [0.8, 1.2],
                size: [0.45, 0.7],
                acceleration: [0.0, 0.0, 0.0],
                color: smoke_colors(0.3, 0.2),
                ..EmitterDesc {
                    bursts: vec![at(0.1, 3)],
                    shape: EmitShape::Cone {
                        angle_deg: 16.0,
                        radius: 0.15,
                    },
                    speed: [3.0, 5.0],
                    ..smoke("smoke", 0.12, 0.3)
                }
            },
        ],
    )
}

/// Reach of `spell.fire.jet` at scale 1, meters (the beam cue scales it to its length).
pub const FIRE_JET_REACH: f32 = 8.0;

fn fire_jet() -> EffectDesc {
    // Half a second of a channelled stream (the beam cue comes again every 0.5 s while the
    // spell is held, and the copies overlap into one steady jet). Flames leave the hand in a
    // tight cone fast enough to reach the end, swelling into a widening, rolling cone as they
    // slow; a white-hot core near the hand, heat shimmer around it, embers and a little smoke.
    let reach = FIRE_JET_REACH;
    let life = 0.55;
    let stream = |name: &str, rate: f32, angle: f32| EmitterDesc {
        rate,
        duration: Some(0.5),
        shape: EmitShape::Cone {
            angle_deg: angle,
            radius: 0.06,
        },
        // Fast enough to reach the end in a lifetime even with the drag.
        speed: [reach / life * 1.25, reach / life * 1.45],
        drag: 1.2,
        ..em(name)
    };
    EffectDesc {
        light: light([1.0, 0.55, 0.2], 14_000.0, 10.0, 0.5, 0.25),
        ..effect(
            "spell.fire.jet",
            "A channelled jet of flame from the hand: a white-hot core flaring into a widening              cone of rolling fire, heat shimmer, embers and a thin trail of smoke. Scale = reach              / 8 m.",
            Some(0.5),
            vec![
                EmitterDesc {
                    lifetime: [life * 0.8, life],
                    size: [0.4, 0.55],
                    size_over_life: c(&[(0.0, 0.45), (0.3, 1.5), (1.0, 4.0)]),
                    rotation_deg: [0.0, 360.0],
                    spin_deg: [-120.0, 120.0],
                    sprite: Sprite::Fireball,
                    color: flame_colors(),
                    emissive: 24_000.0,
                    additive: 1.0,
                    soft: 0.2,
                    turbulence: 3.0,
                    turbulence_scale: 0.8,
                    ..stream("flames", 150.0, 7.0)
                },
                EmitterDesc {
                    lifetime: [0.12, 0.2],
                    size: [0.22, 0.32],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.6)]),
                    sprite: Sprite::Plasma,
                    facing: Facing::Velocity,
                    stretch: 0.025,
                    color: g(&[
                        (0.0, [1.0, 0.9, 0.7, 0.0]),
                        (0.15, [1.0, 0.85, 0.6, 0.9]),
                        (1.0, [1.0, 0.5, 0.15, 0.0]),
                    ]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..stream("core", 110.0, 2.5)
                },
                EmitterDesc {
                    lifetime: [life * 0.9, life * 1.2],
                    size: [0.4, 0.6],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 2.4)]),
                    ..EmitterDesc {
                        rate: 22.0,
                        duration: Some(0.5),
                        shape: EmitShape::Cone {
                            angle_deg: 7.0,
                            radius: 0.1,
                        },
                        speed: [reach / life * 0.9, reach / life * 1.2],
                        drag: 1.2,
                        ..haze("shimmer", 0.8)
                    }
                },
                EmitterDesc {
                    lifetime: [0.4, 0.8],
                    size: [0.012, 0.025],
                    sprite: Sprite::Spark,
                    facing: Facing::Velocity,
                    stretch: 0.03,
                    color: spark_colors(),
                    emissive: 22_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    gravity: 0.3,
                    ..stream("embers", 40.0, 10.0)
                },
                EmitterDesc {
                    lifetime: [0.9, 1.4],
                    size: [0.3, 0.45],
                    delay: 0.1,
                    acceleration: [0.0, 0.0, 0.0],
                    speed: [reach / life * 0.7, reach / life * 0.9],
                    drag: 2.5,
                    shape: EmitShape::Cone {
                        angle_deg: 9.0,
                        radius: 0.1,
                    },
                    rate: 5.0,
                    duration: Some(0.4),
                    ..smoke("smoke", 0.25, 0.18)
                },
            ],
        )
    }
}

fn frost_cone() -> EffectDesc {
    cone(
        "spell.frost.cone",
        "A gust of frost breath: a rolling white-blue blast of freezing mist thrown forward, \
         ice shards and glittering snow inside it.",
        light([0.55, 0.78, 1.0], 3000.0, 8.0, 0.1, 0.4),
        vec![
            EmitterDesc {
                lifetime: [0.7, 1.1],
                size: [0.5, 0.8],
                size_over_life: c(&[(0.0, 0.5), (0.4, 1.4), (1.0, 2.0)]),
                color: g(&[
                    (0.0, [0.85, 0.93, 1.0, 0.0]),
                    (0.1, [0.85, 0.93, 1.0, 0.55]),
                    (1.0, [0.85, 0.93, 1.0, 0.0]),
                ]),
                emissive: 300.0,
                ..EmitterDesc {
                    sprite: Sprite::Smoke,
                    lit: 1.0,
                    soft: 0.4,
                    erode: 0.5,
                    ..gout("blast", 14, 14.0, [5.0, 7.5])
                }
            },
            EmitterDesc {
                lifetime: [0.35, 0.6],
                size: [0.08, 0.16],
                rotation_deg: [0.0, 360.0],
                sprite: Sprite::Crystal,
                facing: Facing::Velocity,
                stretch: 0.0,
                lit: 1.0,
                color: fade([0.75, 0.9, 1.0], 1.0, 0.1, 0.7),
                emissive: 2500.0,
                soft: 0.0,
                gravity: 0.4,
                ..gout("shards", 14, 12.0, [7.0, 11.0])
            },
            EmitterDesc {
                lifetime: [0.6, 1.0],
                size: [0.025, 0.045],
                sprite: Sprite::Star,
                color: fade([0.85, 0.95, 1.0], 1.0, 0.1, 0.6),
                emissive: 12_000.0,
                additive: 1.0,
                soft: 0.0,
                turbulence: 1.0,
                ..gout("glitter", 40, 18.0, [4.0, 8.0])
            },
        ],
    )
}

fn storm_cone() -> EffectDesc {
    cone(
        "spell.storm.cone",
        "A spray of lightning: arcs forking forward out of the hand, sparks thrown with them.",
        light([0.75, 0.85, 1.0], 20_000.0, 10.0, 0.8, 0.2),
        vec![
            EmitterDesc {
                bursts: vec![at(0.0, 4), at(0.07, 3), at(0.14, 3)],
                shape: EmitShape::Point,
                offset: [0.0, 0.7, 0.0],
                speed: [0.0, 0.0],
                lifetime: [0.07, 0.12],
                size: [1.3, 1.8],
                rotation_deg: [-20.0, 20.0],
                sprite: Sprite::Arc,
                color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.55, 0.65, 1.0, 0.0])]),
                emissive: 70_000.0,
                additive: 1.0,
                soft: 0.0,
                wind: 0.0,
                ..em("forks")
            },
            EmitterDesc {
                lifetime: [0.2, 0.45],
                size: [0.012, 0.022],
                sprite: Sprite::Spark,
                facing: Facing::Velocity,
                stretch: 0.03,
                color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                emissive: 30_000.0,
                additive: 1.0,
                soft: 0.0,
                gravity: 0.6,
                ..gout("sparks", 40, 20.0, [6.0, 12.0])
            },
        ],
    )
}

fn blood_cone() -> EffectDesc {
    cone(
        "spell.blood.cone",
        "A spray of blood: glossy drops and globs flung forward in a fan, a red mist hanging \
         after them.",
        light([1.0, 0.07, 0.04], 1500.0, 6.0, 0.1, 0.3),
        vec![
            EmitterDesc {
                lifetime: [0.5, 0.9],
                size: [0.04, 0.08],
                sprite: Sprite::Goo,
                lit: 1.0,
                gravity: 1.0,
                collide: Collide::Die,
                color: g(&[
                    (0.0, [0.3, 0.012, 0.01, 1.0]),
                    (1.0, [0.3, 0.012, 0.01, 1.0]),
                ]),
                emissive: 200.0,
                soft: 0.0,
                ..gout("drops", 50, 16.0, [5.0, 9.0])
            },
            EmitterDesc {
                lifetime: [1.0, 1.6],
                size: [0.4, 0.7],
                sprite: Sprite::Dust,
                color: fade([0.45, 0.02, 0.02], 0.35, 0.05, 0.4),
                emissive: 40.0,
                ..EmitterDesc {
                    soft: 0.4,
                    lit: 1.0,
                    ..gout("mist", 8, 16.0, [2.0, 4.0])
                }
            },
        ],
    )
}

fn blight_cone() -> EffectDesc {
    cone(
        "spell.blight.cone",
        "A belch of poison: a rolling green cloud thrown forward, its edges eaten away and \
         glowing, gobs of ooze flung in it.",
        light([0.38, 1.0, 0.16], 1500.0, 6.0, 0.1, 0.4),
        vec![
            EmitterDesc {
                lifetime: [1.0, 1.6],
                size: [0.5, 0.8],
                size_over_life: c(&[(0.0, 0.5), (0.4, 1.4), (1.0, 2.2)]),
                color: g(&[
                    (0.0, [0.26, 0.52, 0.1, 0.0]),
                    (0.1, [0.26, 0.52, 0.1, 0.5]),
                    (1.0, [0.16, 0.26, 0.07, 0.0]),
                ]),
                emissive: 30.0,
                erode: 0.7,
                erode_glow: 600.0,
                ..EmitterDesc {
                    sprite: Sprite::Smoke,
                    lit: 1.0,
                    soft: 0.4,
                    ..gout("cloud", 12, 15.0, [4.0, 6.5])
                }
            },
            EmitterDesc {
                lifetime: [0.5, 0.9],
                size: [0.05, 0.09],
                sprite: Sprite::Goo,
                lit: 1.0,
                gravity: 1.0,
                collide: Collide::Die,
                color: g(&[(0.0, [0.17, 0.4, 0.05, 1.0]), (1.0, [0.17, 0.4, 0.05, 1.0])]),
                emissive: 700.0,
                soft: 0.0,
                ..gout("gobs", 16, 14.0, [5.0, 8.0])
            },
        ],
    )
}

fn shadow_cone() -> EffectDesc {
    cone(
        "spell.shadow.cone",
        "A breath of darkness: black wisps thrown forward and curling, rimmed in violet, motes \
         of light snuffed inside them.",
        light([0.55, 0.22, 1.0], 600.0, 5.0, 0.2, 0.4),
        vec![
            EmitterDesc {
                lifetime: [0.7, 1.1],
                size: [0.5, 0.9],
                size_over_life: c(&[(0.0, 0.5), (0.4, 1.3), (1.0, 1.7)]),
                rotation_deg: [0.0, 360.0],
                spin_deg: [-90.0, 90.0],
                sprite: Sprite::Wisp,
                erode: 0.5,
                erode_glow: 2500.0,
                color: fade([0.55, 0.22, 1.0], 0.95, 0.05, 0.5),
                emissive: 2000.0,
                soft: 0.15,
                turbulence: 2.0,
                ..gout("wisps", 16, 15.0, [4.0, 7.0])
            },
            EmitterDesc {
                lifetime: [0.3, 0.6],
                size: [0.015, 0.025],
                sprite: Sprite::Spark,
                facing: Facing::Velocity,
                stretch: 0.02,
                color: g(&[(0.0, [0.75, 0.5, 1.0, 1.0]), (1.0, [0.5, 0.2, 1.0, 0.0])]),
                emissive: 15_000.0,
                additive: 1.0,
                soft: 0.0,
                ..gout("motes", 30, 18.0, [4.0, 8.0])
            },
        ],
    )
}

// ---------------------------------------------------------------- walls

/// One piece of a wall: `layers` round its middle, standing on the ground.
fn wall(
    name: &str,
    description: &str,
    light: Option<EffectLight>,
    layers: Vec<EmitterDesc>,
) -> EffectDesc {
    EffectDesc {
        light,
        prewarm: 1.5,
        ..effect(name, description, None, layers)
    }
}

fn walls() -> Vec<EffectDesc> {
    let foot = EmitShape::Disc { radius: 0.7 };
    vec![
        wall(
            "spell.fire.wall",
            "A piece of a wall of fire: a curtain of tall flames standing up off the coals, \
             embers streaming up and smoke rising above it.",
            light([1.0, 0.55, 0.2], 5000.0, 9.0, 0.4, 0.5),
            vec![
                EmitterDesc {
                    rate: 12.0,
                    shape: foot,
                    speed: [0.0, 0.0],
                    lifetime: [0.8, 1.2],
                    size: [1.4, 2.0],
                    rotation_deg: [0.0, 0.0],
                    local_space: true,
                    steady: true,
                    sprite: Sprite::Blaze,
                    facing: Facing::Axis,
                    stretch: 1.3,
                    color: fade([1.0, 0.62, 0.24], 1.0, 0.3, 0.7),
                    emissive: 11_000.0,
                    additive: 0.5,
                    wind: 0.0,
                    soft: 0.1,
                    ..em("curtain")
                },
                EmitterDesc {
                    rate: 30.0,
                    shape: foot,
                    offset: [0.0, 0.3, 0.0],
                    speed: [0.0, 0.3],
                    acceleration: [0.0, 3.0, 0.0],
                    lifetime: [0.6, 0.9],
                    size: [0.7, 1.1],
                    size_over_life: c(&[(0.0, 0.5), (0.3, 1.0), (1.0, 0.6)]),
                    rotation_deg: [-8.0, 8.0],
                    sprite: Sprite::Flame,
                    color: flame_colors(),
                    emissive: 18_000.0,
                    additive: 1.0,
                    soft: 0.25,
                    wind: 0.4,
                    ..em("tongues")
                },
                EmitterDesc {
                    rate: 30.0,
                    shape: foot,
                    offset: [0.0, 0.8, 0.0],
                    speed: [0.3, 1.0],
                    acceleration: [0.0, 3.0, 0.0],
                    turbulence: 4.0,
                    drag: 1.0,
                    lifetime: [1.0, 2.0],
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
                    rate: 2.0,
                    shape: foot,
                    offset: [0.0, 2.2, 0.0],
                    speed: [0.2, 0.4],
                    acceleration: [0.0, 1.5, 0.0],
                    lifetime: [1.8, 2.6],
                    size: [0.8, 1.2],
                    ..smoke("smoke", 0.25, 0.18)
                },
            ],
        ),
        wall(
            "spell.frost.wall",
            "The air round a piece of an ice wall: freezing fog pouring down off it and \
             spreading over the ground, snow glittering in it.",
            light([0.55, 0.78, 1.0], 1200.0, 6.0, 0.0, 0.5),
            vec![
                EmitterDesc {
                    rate: 7.0,
                    shape: foot,
                    offset: [0.0, 1.2, 0.0],
                    speed: [0.1, 0.3],
                    gravity: 0.25,
                    drag: 1.2,
                    lifetime: [2.0, 3.0],
                    size: [0.6, 0.9],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 2.0)]),
                    sprite: Sprite::Dust,
                    color: fade([0.85, 0.93, 1.0], 0.3, 0.2, 0.6),
                    emissive: 60.0,
                    ..smoke("fog", 0.88, 0.3)
                },
                EmitterDesc {
                    rate: 30.0,
                    shape: foot,
                    offset: [0.0, 2.2, 0.0],
                    speed: [0.0, 0.0],
                    velocity: [0.0, -0.5, 0.0],
                    turbulence: 0.6,
                    lifetime: [3.0, 4.0],
                    size: [0.025, 0.045],
                    sprite: Sprite::Star,
                    color: fade([0.85, 0.95, 1.0], 1.0, 0.15, 0.7),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.3,
                    ..em("snow")
                },
            ],
        ),
        wall(
            "spell.storm.wall",
            "The air round a post of a storm wall: arcs crackling up and down it, sparks \
             spitting and a pale blue glow of charged air.",
            light([0.62, 0.74, 1.0], 5000.0, 9.0, 0.9, 0.3),
            vec![
                EmitterDesc {
                    rate: 14.0,
                    shape: EmitShape::Line { length: 2.0 },
                    offset: [0.0, 1.1, 0.0],
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.07, 0.14],
                    size: [0.6, 1.1],
                    rotation_deg: [0.0, 360.0],
                    sprite: Sprite::Arc,
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.55, 0.65, 1.0, 0.0])]),
                    emissive: 60_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("crackle")
                },
                EmitterDesc {
                    rate: 6.0,
                    shape: EmitShape::Line { length: 1.8 },
                    offset: [0.0, 1.1, 0.0],
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.3, 0.6],
                    size: [1.4, 2.0],
                    color: g(&[
                        (0.0, [0.6, 0.72, 1.0, 0.0]),
                        (0.2, [0.6, 0.72, 1.0, 0.25]),
                        (1.0, [0.5, 0.6, 1.0, 0.0]),
                    ]),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.3,
                    wind: 0.0,
                    ..em("charged air")
                },
                EmitterDesc {
                    bursts: every(0.3, 5),
                    shape: EmitShape::Line { length: 2.0 },
                    offset: [0.0, 1.1, 0.0],
                    color: g(&[(0.0, [0.95, 0.97, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    lifetime: [0.2, 0.5],
                    ..sparks("sparks", 5, [1.0, 3.0])
                },
            ],
        ),
        wall(
            "spell.blood.wall",
            "The air round a piece of a blood wall: a red mist hanging over it and blood \
             running down off it in drops.",
            light([1.0, 0.07, 0.04], 800.0, 5.0, 0.1, 0.5),
            vec![
                EmitterDesc {
                    rate: 6.0,
                    shape: foot,
                    offset: [0.0, 1.0, 0.0],
                    speed: [0.05, 0.2],
                    lifetime: [2.0, 3.0],
                    size: [0.7, 1.1],
                    sprite: Sprite::Dust,
                    color: fade([0.42, 0.02, 0.02], 0.3, 0.25, 0.6),
                    emissive: 35.0,
                    ..smoke("mist", 0.3, 0.3)
                },
                EmitterDesc {
                    rate: 18.0,
                    shape: foot,
                    offset: [0.0, 2.0, 0.0],
                    speed: [0.0, 0.1],
                    lifetime: [0.8, 1.2],
                    size: [0.03, 0.05],
                    emissive: 40.0,
                    color: g(&[(0.0, [0.3, 0.01, 0.01, 1.0]), (1.0, [0.3, 0.01, 0.01, 0.9])]),
                    ..droplets(
                        "drips",
                        0,
                        [0.3, 0.012, 0.01],
                        [0.0, 0.0],
                        0.0,
                        Collide::Die,
                    )
                },
            ],
        ),
        wall(
            "spell.blight.wall",
            "The air round a piece of a blight wall: a green miasma boiling up off it, eaten \
             away and glowing at its edges, spores drifting.",
            light([0.38, 1.0, 0.16], 1000.0, 6.0, 0.15, 0.5),
            vec![
                EmitterDesc {
                    rate: 8.0,
                    shape: foot,
                    offset: [0.0, 0.4, 0.0],
                    speed: [0.1, 0.3],
                    acceleration: [0.0, 0.5, 0.0],
                    drag: 1.0,
                    lifetime: [2.5, 3.5],
                    size: [0.8, 1.2],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.8)]),
                    color: g(&[
                        (0.0, [0.26, 0.52, 0.1, 0.0]),
                        (0.2, [0.26, 0.52, 0.1, 0.45]),
                        (1.0, [0.16, 0.26, 0.07, 0.0]),
                    ]),
                    emissive: 30.0,
                    erode: 0.7,
                    erode_glow: 600.0,
                    ..smoke("miasma", 0.3, 0.45)
                },
                EmitterDesc {
                    rate: 14.0,
                    shape: foot,
                    offset: [0.0, 0.6, 0.0],
                    speed: [0.05, 0.2],
                    acceleration: [0.0, 0.4, 0.0],
                    turbulence: 1.0,
                    lifetime: [2.0, 3.0],
                    size: [0.025, 0.04],
                    color: fade([0.45, 1.0, 0.2], 1.0, 0.2, 0.6),
                    emissive: 6000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("spores")
                },
            ],
        ),
        wall(
            "spell.shadow.wall",
            "The air round a piece of a veil of darkness: black wisps standing up off it like \
             dark flames, rimmed in violet, light motes sinking into it.",
            light([0.55, 0.22, 1.0], 600.0, 5.0, 0.3, 0.5),
            vec![
                EmitterDesc {
                    rate: 14.0,
                    shape: foot,
                    speed: [0.2, 0.5],
                    acceleration: [0.0, 1.0, 0.0],
                    drag: 1.0,
                    turbulence: 0.8,
                    lifetime: [1.4, 2.0],
                    size: [1.0, 1.5],
                    size_over_life: c(&[(0.0, 0.5), (0.4, 1.0), (1.0, 1.2)]),
                    rotation_deg: [-10.0, 10.0],
                    sprite: Sprite::Wisp,
                    erode: 0.5,
                    erode_glow: 2500.0,
                    color: fade([0.55, 0.22, 1.0], 0.95, 0.15, 0.55),
                    emissive: 1500.0,
                    soft: 0.25,
                    wind: 0.3,
                    ..em("veil")
                },
                EmitterDesc {
                    rate: 24.0,
                    shape: foot,
                    offset: [0.0, 2.2, 0.0],
                    speed: [0.0, 0.0],
                    velocity: [0.0, -0.8, 0.0],
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
            ],
        ),
    ]
}

// ---------------------------------------------------------------- fizzle

fn fizzle() -> EffectDesc {
    EffectDesc {
        light: light([0.75, 0.65, 1.0], 800.0, 4.0, 0.6, 0.25),
        ..effect(
            "spell.fizzle",
            "A cast that failed: a weak pop of light in the hand, a puff of grey smoke and a few \
             sparks dropping and dying.",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.15, 0.15],
                    size: [0.6, 0.6],
                    size_over_life: c(&[(0.0, 1.0), (1.0, 0.3)]),
                    color: g(&[(0.0, [0.8, 0.7, 1.0, 0.7]), (1.0, [0.6, 0.5, 1.0, 0.0])]),
                    emissive: 4000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("pop")
                },
                EmitterDesc {
                    bursts: once(5),
                    shape: EmitShape::Sphere {
                        radius: 0.08,
                        surface: false,
                    },
                    speed: [0.1, 0.4],
                    acceleration: [0.0, 0.6, 0.0],
                    lifetime: [1.0, 1.6],
                    size: [0.25, 0.4],
                    ..smoke("smoke", 0.35, 0.45)
                },
                EmitterDesc {
                    color: g(&[
                        (0.0, [0.85, 0.75, 1.0, 1.0]),
                        (0.4, [0.6, 0.45, 1.0, 0.8]),
                        (1.0, [0.4, 0.3, 0.8, 0.0]),
                    ]),
                    emissive: 9000.0,
                    lifetime: [0.3, 0.7],
                    ..sparks("sparks", 12, [0.5, 1.6])
                },
            ],
        )
    }
}

// ---------------------------------------------------------------- statuses

/// The body a status plays on, from its middle: about 0.5 m wide and 1.7 m tall.
fn body(lower: f32, upper: f32) -> (EmitShape, [f32; 3]) {
    (
        EmitShape::Box {
            half_extents: [0.2, (upper - lower) * 0.5, 0.14],
        },
        [0.0, (upper + lower) * 0.5, 0.0],
    )
}

fn status(
    name: &str,
    description: &str,
    light: Option<EffectLight>,
    layers: Vec<EmitterDesc>,
) -> EffectDesc {
    EffectDesc {
        light,
        prewarm: 1.0,
        ..effect(name, description, None, layers)
    }
}

fn statuses() -> Vec<EffectDesc> {
    let (legs_to_chest, legs_to_chest_at) = body(-0.55, 0.45);
    let (shoulders, shoulders_at) = body(0.35, 0.6);
    let (belly, belly_at) = body(-0.1, 0.3);
    let (whole, whole_at) = body(-0.75, 0.75);
    vec![
        status(
            "spell.status.burning",
            "A body on fire: flames licking up from the legs to the chest, embers whirling off \
             and a trail of smoke.",
            light([1.0, 0.55, 0.2], 3000.0, 6.0, 0.5, 0.4),
            vec![
                EmitterDesc {
                    rate: 34.0,
                    shape: legs_to_chest,
                    offset: legs_to_chest_at,
                    speed: [0.0, 0.2],
                    acceleration: [0.0, 2.0, 0.0],
                    lifetime: [0.4, 0.7],
                    size: [0.28, 0.45],
                    size_over_life: c(&[(0.0, 0.5), (0.3, 1.0), (1.0, 0.6)]),
                    rotation_deg: [-10.0, 10.0],
                    sprite: Sprite::Flame,
                    color: flame_colors(),
                    emissive: 16_000.0,
                    additive: 1.0,
                    soft: 0.15,
                    wind: 0.5,
                    ..em("flames")
                },
                EmitterDesc {
                    rate: 24.0,
                    shape: legs_to_chest,
                    offset: legs_to_chest_at,
                    speed: [0.3, 0.8],
                    acceleration: [0.0, 2.5, 0.0],
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
                    rate: 3.0,
                    shape: EmitShape::Sphere {
                        radius: 0.2,
                        surface: false,
                    },
                    offset: [0.0, 0.8, 0.0],
                    speed: [0.1, 0.3],
                    acceleration: [0.0, 1.4, 0.0],
                    lifetime: [1.5, 2.5],
                    size: [0.35, 0.55],
                    ..smoke("smoke", 0.15, 0.25)
                },
            ],
        ),
        status(
            "spell.status.chilled",
            "A chilled body: frost glittering on it, snow motes falling off the shoulders and \
             a cold mist sinking round the legs.",
            light([0.55, 0.78, 1.0], 400.0, 3.0, 0.0, 0.4),
            vec![
                EmitterDesc {
                    rate: 30.0,
                    shape: whole,
                    offset: whole_at,
                    speed: [0.0, 0.05],
                    local_space: true,
                    lifetime: [0.5, 1.0],
                    size: [0.02, 0.04],
                    sprite: Sprite::Star,
                    color: fade([0.8, 0.92, 1.0], 1.0, 0.3, 0.6),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("glints")
                },
                EmitterDesc {
                    rate: 16.0,
                    shape: shoulders,
                    offset: shoulders_at,
                    speed: [0.0, 0.1],
                    velocity: [0.0, -0.4, 0.0],
                    lifetime: [1.5, 2.2],
                    size: [0.02, 0.035],
                    sprite: Sprite::Star,
                    color: fade([0.85, 0.95, 1.0], 1.0, 0.15, 0.7),
                    emissive: 6000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.4,
                    ..em("snow")
                },
                EmitterDesc {
                    rate: 4.0,
                    shape: EmitShape::Disc { radius: 0.3 },
                    offset: [0.0, -0.6, 0.0],
                    speed: [0.1, 0.3],
                    gravity: 0.1,
                    lifetime: [1.5, 2.2],
                    size: [0.4, 0.6],
                    sprite: Sprite::Dust,
                    color: fade([0.85, 0.93, 1.0], 0.2, 0.2, 0.6),
                    emissive: 40.0,
                    ..smoke("mist", 0.85, 0.2)
                },
            ],
        ),
        status(
            "spell.status.frozen",
            "A frozen body: freezing fog sliding off the ice and pooling on the ground, frost \
             glinting all over it and snow sifting down.",
            light([0.55, 0.8, 1.0], 900.0, 4.0, 0.0, 0.4),
            vec![
                EmitterDesc {
                    rate: 10.0,
                    shape: whole,
                    offset: whole_at,
                    speed: [0.05, 0.2],
                    gravity: 0.15,
                    drag: 1.0,
                    lifetime: [1.8, 2.6],
                    size: [0.4, 0.65],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 1.8)]),
                    sprite: Sprite::Dust,
                    color: fade([0.85, 0.93, 1.0], 0.45, 0.2, 0.6),
                    emissive: 80.0,
                    ..smoke("fog", 0.88, 0.45)
                },
                EmitterDesc {
                    rate: 40.0,
                    shape: whole,
                    offset: whole_at,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.4, 0.8],
                    size: [0.025, 0.05],
                    sprite: Sprite::Star,
                    color: fade([0.85, 0.95, 1.0], 1.0, 0.3, 0.6),
                    emissive: 14_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("glints")
                },
            ],
        ),
        status(
            "spell.status.shocked",
            "A shocked body: arcs crawling over it and snapping between its limbs, sparks \
             spitting off.",
            light([0.65, 0.75, 1.0], 3000.0, 6.0, 0.9, 0.2),
            vec![
                EmitterDesc {
                    rate: 32.0,
                    shape: whole,
                    offset: whole_at,
                    speed: [0.0, 0.0],
                    local_space: true,
                    lifetime: [0.06, 0.12],
                    size: [0.7, 1.1],
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
                    bursts: every(0.2, 5),
                    shape: whole,
                    offset: whole_at,
                    color: g(&[(0.0, [0.9, 0.95, 1.0, 1.0]), (1.0, [0.5, 0.6, 1.0, 0.0])]),
                    lifetime: [0.2, 0.5],
                    ..sparks("sparks", 5, [1.0, 3.0])
                },
            ],
        ),
        status(
            "spell.status.bleeding",
            "A bleeding body: a red mist round the shoulders and drops of blood falling from \
             it and spattering.",
            None,
            vec![
                EmitterDesc {
                    rate: 9.0,
                    shape: shoulders,
                    offset: shoulders_at,
                    speed: [0.05, 0.15],
                    orbit_speed: 0.8,
                    lifetime: [1.2, 1.8],
                    size: [0.35, 0.5],
                    sprite: Sprite::Dust,
                    color: fade([0.4, 0.015, 0.015], 0.6, 0.2, 0.5),
                    emissive: 40.0,
                    ..smoke("mist", 0.3, 0.35)
                },
                EmitterDesc {
                    rate: 24.0,
                    bursts: Vec::new(),
                    shape: legs_to_chest,
                    offset: [0.0, 0.2, 0.0],
                    speed: [0.0, 0.3],
                    lifetime: [0.8, 1.2],
                    size: [0.035, 0.065],
                    emissive: 40.0,
                    color: g(&[(0.0, [0.3, 0.01, 0.01, 1.0]), (1.0, [0.3, 0.01, 0.01, 0.9])]),
                    ..droplets(
                        "drips",
                        0,
                        [0.3, 0.012, 0.01],
                        [0.0, 0.0],
                        0.0,
                        Collide::Die,
                    )
                },
            ],
        ),
        status(
            "spell.status.rot",
            "A rotting body: a shroud of green fumes round the belly, eaten away at the edges, \
             spores drifting and flies circling.",
            light([0.38, 1.0, 0.16], 300.0, 3.0, 0.1, 0.4),
            vec![
                EmitterDesc {
                    rate: 8.0,
                    shape: belly,
                    offset: belly_at,
                    speed: [0.05, 0.2],
                    orbit_speed: 0.6,
                    lifetime: [1.5, 2.2],
                    size: [0.35, 0.55],
                    color: g(&[
                        (0.0, [0.26, 0.52, 0.1, 0.0]),
                        (0.2, [0.26, 0.52, 0.1, 0.45]),
                        (1.0, [0.16, 0.26, 0.07, 0.0]),
                    ]),
                    emissive: 15.0,
                    erode: 0.5,
                    erode_glow: 0.0,
                    ..smoke("shroud", 0.3, 0.45)
                },
                EmitterDesc {
                    rate: 12.0,
                    shape: belly,
                    offset: belly_at,
                    speed: [0.05, 0.2],
                    acceleration: [0.0, 0.3, 0.0],
                    turbulence: 1.0,
                    lifetime: [1.5, 2.5],
                    size: [0.02, 0.035],
                    color: fade([0.45, 1.0, 0.2], 1.0, 0.2, 0.6),
                    emissive: 5000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("spores")
                },
                EmitterDesc {
                    offset: [0.0, 0.0, 0.0],
                    shape: EmitShape::Sphere {
                        radius: 0.4,
                        surface: false,
                    },
                    ..super::weaves::flies(8.0, 0.4)
                },
            ],
        ),
        status(
            "spell.status.dread",
            "A body gripped by dread: black wisps rising off the head and shoulders like dark \
             flames, a violet glow above it and its light drawn upward.",
            light([0.55, 0.22, 1.0], 300.0, 3.0, 0.3, 0.4),
            vec![
                EmitterDesc {
                    rate: 10.0,
                    shape: shoulders,
                    offset: [0.0, 0.6, 0.0],
                    speed: [0.1, 0.3],
                    acceleration: [0.0, 0.8, 0.0],
                    drag: 1.0,
                    turbulence: 0.8,
                    lifetime: [0.9, 1.4],
                    size: [0.35, 0.55],
                    size_over_life: c(&[(0.0, 0.5), (0.4, 1.0), (1.0, 1.2)]),
                    rotation_deg: [-10.0, 10.0],
                    sprite: Sprite::Wisp,
                    erode: 0.5,
                    erode_glow: 2500.0,
                    color: fade([0.55, 0.22, 1.0], 0.95, 0.15, 0.55),
                    emissive: 1500.0,
                    soft: 0.15,
                    wind: 0.3,
                    ..em("dark flames")
                },
                EmitterDesc {
                    rate: 20.0,
                    shape: whole,
                    offset: whole_at,
                    speed: [0.0, 0.1],
                    acceleration: [0.0, 1.2, 0.0],
                    lifetime: [0.8, 1.2],
                    size: [0.015, 0.025],
                    color: fade([0.7, 0.45, 1.0], 1.0, 0.2, 0.6),
                    emissive: 9000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("motes")
                },
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_look_magic_asks_for_beyond_bolts_impacts_and_fields_exists() {
        let all = super::super::all();
        let find = |name: &str| {
            let e = all
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name} is missing"));
            e.validate().unwrap_or_else(|err| panic!("{err}"));
            e.clone()
        };
        let looks = super::ELEMENTS
            .iter()
            .copied()
            .chain(super::WEAVES.iter().map(|w| w.0));
        for look in looks {
            assert!(!find(&format!("spell.{look}.charge")).is_one_shot());
            assert!(find(&format!("spell.{look}.cone")).is_one_shot());
            assert!(!find(&format!("spell.{look}.wall")).is_one_shot());
            assert!(find(&format!("spell.{look}.fizzle")).is_one_shot());
        }
        for s in [
            "burning", "chilled", "frozen", "shocked", "bleeding", "rot", "dread",
        ] {
            assert!(!find(&format!("spell.status.{s}")).is_one_shot());
        }
        assert!(find("spell.fizzle").is_one_shot());
        assert!(find("spell.fizzle.fizzle").is_one_shot());
    }
}
