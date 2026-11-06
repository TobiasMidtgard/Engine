//! Space magic (magic round 9): portals, blink and recall. Magic draws a portal's frame and
//! membrane and the afterimage ghosts as meshes; these are the bursts and glitter around them.
//!
//! * `spell.portal.impact`: something passing through a portal end, or an end opening (cue
//!   scale 1.3). A ring of light flashes out in the plane of the opening and sparks fly out
//!   along the end's facing (the effect's up). Violet here; spells.rs tints a copy to each
//!   end's own colour (teal for the other end of a plain pair).
//! * `spell.portal.fizzle`: an end closing: motes streak in to the centre, then a small pop.
//! * `spell.blink.impact`: the burst at each end of a blink (and of a recall).
//! * `spell.blink.trail`: glitter left hanging along a blink's streak, one metre of it at
//!   scale 1 along the effect's up; spells.rs lays copies along the beam. The streak itself
//!   is a beam ribbon that wipes away from its start (`Beam::wipe_secs`).

use super::*;

/// A plain portal's first end (magic's rim colour); the second end is teal.
pub const PORTAL: [f32; 3] = [0.62, 0.3, 1.0];
const PORTAL_HOT: [f32; 3] = [0.92, 0.82, 1.0];
/// Blink and recall: a pale blue-violet.
pub const BLINK: [f32; 3] = [0.55, 0.62, 1.0];
const BLINK_HOT: [f32; 3] = [0.9, 0.93, 1.0];

pub(super) fn all() -> Vec<EffectDesc> {
    vec![
        portal_impact(),
        portal_fizzle(),
        blink_impact(),
        blink_trail(),
    ]
}

/// White-hot at birth, cooling to `rgb`, fading out.
fn cooling(hot: [f32; 3], rgb: [f32; 3], peak: f32) -> Gradient {
    g(&[
        (0.0, [hot[0], hot[1], hot[2], peak]),
        (0.35, [rgb[0], rgb[1], rgb[2], peak * 0.85]),
        (1.0, [rgb[0], rgb[1], rgb[2], 0.0]),
    ])
}

/// Small glowing motes.
fn motes(name: &str, count: u32, hot: [f32; 3], rgb: [f32; 3], glow: f32) -> EmitterDesc {
    EmitterDesc {
        bursts: once(count),
        lifetime: [0.5, 1.0],
        size: [0.025, 0.05],
        color: cooling(hot, rgb, 1.0),
        emissive: glow,
        additive: 1.0,
        soft: 0.0,
        wind: 0.0,
        ..em(name)
    }
}

/// Passing through an end: a flash filling the opening, two rings of light racing out across
/// its plane, motes swirling off the rim and sparks thrown out of the face.
fn portal_impact() -> EffectDesc {
    EffectDesc {
        light: light(PORTAL, 16_000.0, 10.0, 0.0, 0.35),
        ..effect(
            "spell.portal.impact",
            "Something passing through a portal end: a violet flash filling the opening, rings \
             of light racing out across its plane, motes swirling off the rim and sparks thrown \
             out of the face. The effect's up is the end's facing; scale 1 is an end 1.4 m \
             wide (an end opening sends 1.3).",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.16, 0.16],
                    size: [2.2, 2.2],
                    size_over_life: c(&[(0.0, 0.5), (1.0, 1.2)]),
                    color: g(&[
                        (0.0, [PORTAL_HOT[0], PORTAL_HOT[1], PORTAL_HOT[2], 0.9]),
                        (0.4, [PORTAL[0], PORTAL[1], PORTAL[2], 0.6]),
                        (1.0, [PORTAL[0], PORTAL[1], PORTAL[2], 0.0]),
                    ]),
                    emissive: 26_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("flash")
                },
                // Rings of light lying in the opening, racing out past the frame.
                EmitterDesc {
                    bursts: vec![at(0.0, 1), at(0.08, 1)],
                    speed: [0.0, 0.0],
                    lifetime: [0.42, 0.42],
                    size: [1.5, 1.5],
                    size_over_life: c(&[(0.0, 0.5), (0.4, 1.4), (1.0, 2.3)]),
                    local_space: true,
                    sprite: Sprite::Ring,
                    facing: Facing::Horizontal,
                    color: g(&[
                        (0.0, [PORTAL_HOT[0], PORTAL_HOT[1], PORTAL_HOT[2], 1.0]),
                        (0.3, [PORTAL[0], PORTAL[1], PORTAL[2], 0.9]),
                        (1.0, [PORTAL[0], PORTAL[1], PORTAL[2], 0.0]),
                    ]),
                    emissive: 30_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    wind: 0.0,
                    ..em("rings")
                },
                // Motes spun off the rim, swirling round the opening as they drift out.
                EmitterDesc {
                    shape: EmitShape::Ring {
                        radius: 0.7,
                        thickness: 0.08,
                    },
                    speed: [0.3, 1.4],
                    velocity: [0.0, 0.5, 0.0],
                    orbit_speed: 2.5,
                    vortex: 3.0,
                    drag: 2.2,
                    lifetime: [0.6, 1.2],
                    ..motes("rim motes", 90, PORTAL_HOT, PORTAL, 22_000.0)
                },
                // Sparks thrown out of the face.
                EmitterDesc {
                    bursts: once(55),
                    shape: EmitShape::Cone {
                        angle_deg: 38.0,
                        radius: 0.45,
                    },
                    speed: [2.5, 7.5],
                    gravity: 0.25,
                    drag: 1.6,
                    collide: Collide::None,
                    lifetime: [0.25, 0.6],
                    size: [0.018, 0.035],
                    color: cooling(PORTAL_HOT, PORTAL, 1.0),
                    emissive: 26_000.0,
                    ..sparks("sparks", 55, [2.5, 7.5])
                },
                // A few star glints in the opening.
                EmitterDesc {
                    bursts: once(7),
                    shape: EmitShape::Disc { radius: 0.55 },
                    speed: [0.0, 0.3],
                    velocity: [0.0, 0.6, 0.0],
                    lifetime: [0.25, 0.5],
                    size: [0.18, 0.35],
                    rotation_deg: [0.0, 90.0],
                    sprite: Sprite::Star,
                    ..motes("stars", 7, PORTAL_HOT, PORTAL, 30_000.0)
                },
            ],
        )
    }
}

/// An end closing: motes streak in from all round to the centre, then a small pop.
fn portal_fizzle() -> EffectDesc {
    // Magic sends the fizzle at scale 0.6: the motes start about a metre out.
    const PULL: f32 = 0.5;
    EffectDesc {
        light: light(PORTAL, 5_000.0, 6.0, 0.0, 0.25),
        ..effect(
            "spell.portal.fizzle",
            "A portal end closing: violet motes streak in from all round to its centre, then it \
             snuffs out in a small pop of light and sparks. Designed for the cue's scale 0.6 \
             (motes start about a metre out).",
            Some(0.5),
            vec![
                // Sucked in: born on a shell and flying inward, dying as they reach the middle.
                EmitterDesc {
                    bursts: vec![at(0.0, 50), at(0.12, 30)],
                    shape: EmitShape::Sphere {
                        radius: 1.7,
                        surface: true,
                    },
                    speed: [-1.7 / PULL, -1.6 / PULL],
                    vortex: 3.0,
                    lifetime: [PULL * 0.9, PULL],
                    size: [0.035, 0.06],
                    facing: Facing::Velocity,
                    stretch: 0.06,
                    sprite: Sprite::Spark,
                    color: g(&[
                        (0.0, [PORTAL[0], PORTAL[1], PORTAL[2], 0.0]),
                        (0.3, [PORTAL[0], PORTAL[1], PORTAL[2], 0.9]),
                        (1.0, [PORTAL_HOT[0], PORTAL_HOT[1], PORTAL_HOT[2], 1.0]),
                    ]),
                    ..motes("inrush", 50, PORTAL_HOT, PORTAL, 18_000.0)
                },
                // The centre gathering as they arrive.
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.48, 0.48],
                    size: [0.9, 0.9],
                    size_over_life: c(&[(0.0, 0.2), (0.85, 1.0), (1.0, 0.3)]),
                    color: g(&[
                        (0.0, [PORTAL[0], PORTAL[1], PORTAL[2], 0.0]),
                        (0.85, [PORTAL_HOT[0], PORTAL_HOT[1], PORTAL_HOT[2], 0.9]),
                        (1.0, [PORTAL_HOT[0], PORTAL_HOT[1], PORTAL_HOT[2], 0.0]),
                    ]),
                    emissive: 14_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("gather")
                },
                // The pop.
                EmitterDesc {
                    delay: 0.45,
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.14, 0.14],
                    size: [1.6, 1.6],
                    size_over_life: c(&[(0.0, 0.4), (1.0, 1.3)]),
                    color: cooling(PORTAL_HOT, PORTAL, 1.0),
                    emissive: 30_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("pop")
                },
                EmitterDesc {
                    delay: 0.45,
                    shape: EmitShape::Sphere {
                        radius: 0.05,
                        surface: false,
                    },
                    gravity: 0.3,
                    drag: 2.0,
                    collide: Collide::None,
                    color: cooling(PORTAL_HOT, PORTAL, 1.0),
                    emissive: 24_000.0,
                    lifetime: [0.2, 0.45],
                    ..sparks("pop sparks", 30, [2.0, 5.0])
                },
            ],
        )
    }
}

/// The burst at each end of a blink: a flash, a bright pinch collapsing to a point, a ring of
/// light, motes bursting out and a few star glints.
fn blink_impact() -> EffectDesc {
    EffectDesc {
        light: light(BLINK, 12_000.0, 8.0, 0.0, 0.3),
        ..effect(
            "spell.blink.impact",
            "Where a blink leaves or lands: a pale flash the size of a body, a hard bright pinch \
             collapsing to a point, a ring of light, motes bursting out and drifting and star \
             glints. Scale 1 is a person, the cue at chest height like the beam.",
            Some(0.1),
            vec![
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.14, 0.14],
                    size: [1.9, 1.9],
                    size_over_life: c(&[(0.0, 0.6), (1.0, 1.1)]),
                    color: cooling(BLINK_HOT, BLINK, 0.9),
                    emissive: 24_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("flash")
                },
                // A hard bright pinch at the heart, collapsing to a point.
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.22, 0.22],
                    size: [0.7, 0.7],
                    size_over_life: c(&[(0.0, 1.0), (1.0, 0.0)]),
                    color: g(&[
                        (0.0, [BLINK_HOT[0], BLINK_HOT[1], BLINK_HOT[2], 1.0]),
                        (1.0, [BLINK[0], BLINK[1], BLINK[2], 0.6]),
                    ]),
                    emissive: 60_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("pinch")
                },
                EmitterDesc {
                    bursts: once(1),
                    speed: [0.0, 0.0],
                    lifetime: [0.22, 0.22],
                    size: [1.0, 1.0],
                    size_over_life: c(&[(0.0, 0.4), (1.0, 1.7)]),
                    sprite: Sprite::Ring,
                    color: cooling(BLINK_HOT, BLINK, 1.0),
                    emissive: 34_000.0,
                    additive: 1.0,
                    soft: 0.0,
                    ..em("ring")
                },
                EmitterDesc {
                    shape: EmitShape::Sphere {
                        radius: 0.35,
                        surface: false,
                    },
                    speed: [0.8, 3.2],
                    drag: 3.0,
                    turbulence: 1.5,
                    lifetime: [0.5, 1.1],
                    ..motes("motes", 70, BLINK_HOT, BLINK, 20_000.0)
                },
                EmitterDesc {
                    shape: EmitShape::Sphere {
                        radius: 0.4,
                        surface: false,
                    },
                    speed: [0.0, 0.4],
                    lifetime: [0.2, 0.45],
                    size: [0.15, 0.3],
                    rotation_deg: [0.0, 90.0],
                    sprite: Sprite::Star,
                    ..motes("stars", 6, BLINK_HOT, BLINK, 28_000.0)
                },
            ],
        )
    }
}

/// Glitter hanging along a blink's streak: one metre of it at scale 1, along the effect's up.
fn blink_trail() -> EffectDesc {
    effect(
        "spell.blink.trail",
        "Glitter left hanging along a blink's streak, drifting and winking out: one metre of \
         it at scale 1, laid along the effect's up (vfx lays copies along the beam).",
        Some(0.1),
        vec![EmitterDesc {
            shape: EmitShape::Line { length: 1.0 },
            speed: [0.0, 0.0],
            drag: 1.5,
            turbulence: 1.2,
            turbulence_scale: 0.4,
            lifetime: [0.25, 0.7],
            size: [0.02, 0.04],
            ..motes("glitter", 14, BLINK_HOT, BLINK, 18_000.0)
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_looks_end_and_glow() {
        let all = all();
        for name in [
            "spell.portal.impact",
            "spell.portal.fizzle",
            "spell.blink.impact",
            "spell.blink.trail",
        ] {
            let e = all.iter().find(|e| e.name == name).expect(name);
            e.validate().unwrap();
            assert!(e.duration.is_some(), "{name}: a one-shot ends");
            assert!(
                e.emitters.iter().all(|em| em.emissive > 0.0),
                "{name}: light, not matter"
            );
        }
    }

    #[test]
    fn the_portal_closes_with_a_pop_after_the_inrush() {
        let fizzle = portal_fizzle();
        let inrush = &fizzle.emitters[0];
        assert!(inrush.speed[1] < 0.0, "motes fly inward");
        let pop = fizzle.layer("pop").map(|i| &fizzle.emitters[i]).unwrap();
        assert!(
            pop.delay >= inrush.lifetime[0],
            "the pop comes as the motes arrive"
        );
        assert!(
            fizzle.duration.unwrap() >= pop.delay,
            "the pop is inside the effect"
        );
    }
}
