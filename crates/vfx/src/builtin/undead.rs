//! Necromancy's raised dead (magic round 6): the look keys `raise`, `ash` and `bones`, sent as
//! impacts in grey-brown. Like stone they are matter, not magic: lit grave dust, clods, ash and
//! bone, with no flash and no glow.
//!
//! * `spell.raise.impact`: grave dust where bones climb out of the floor or a body stands up
//!   (cue scale about 1.2), and a blade biting into a raised body (about 0.35).
//! * `spell.ash.impact`: a raised body collapsing into grey ash that pours down and settles.
//! * `spell.bones.impact`: a skeleton falling apart into bone splinters and dust (about 0.8).

use super::*;

/// Grave earth, the dust it gives off, ash and bone.
const EARTH: [f32; 3] = [0.3, 0.26, 0.21];
const GRAVE_DUST: [f32; 3] = [0.45, 0.42, 0.38];
const ASH: [f32; 3] = [0.52, 0.51, 0.5];
const BONE: [f32; 3] = [0.82, 0.78, 0.68];

pub(super) fn all() -> Vec<EffectDesc> {
    vec![raise_impact(), ash_impact(), bones_impact()]
}

/// Bones climbing out of the floor: grave dust rolling out low along the ground, clods of earth
/// thrown up and tumbling back, a dusty haze hanging over the spot and a trickle of grit.
fn raise_impact() -> EffectDesc {
    effect(
        "spell.raise.impact",
        "Grave dust where the dead climb out of the floor: dust rolling out low along the ground, \
         clods of earth thrown up and falling back, a haze hanging over the spot and grit. \
         Scale 1 is about a body's width (a rise sends 1.2, a blade hit 0.35).",
        Some(0.1),
        vec![
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.3,
                    thickness: 0.15,
                },
                offset: [0.0, 0.08, 0.0],
                velocity: [0.0, 0.1, 0.0],
                drag: 2.5,
                lifetime: [1.4, 2.4],
                sprite: Sprite::Dust,
                ..dust_puff("roll", 26, GRAVE_DUST, 0.7, [1.2, 2.4], [0.35, 0.55])
            },
            EmitterDesc {
                shape: EmitShape::Disc { radius: 0.3 },
                offset: [0.0, 0.15, 0.0],
                velocity: [0.0, 0.35, 0.0],
                lifetime: [1.8, 2.8],
                sprite: Sprite::Dust,
                ..dust_puff("haze", 8, GRAVE_DUST, 0.5, [0.2, 0.6], [0.55, 0.8])
            },
            EmitterDesc {
                shape: EmitShape::Disc { radius: 0.25 },
                velocity: [0.0, 2.2, 0.0],
                size: [0.07, 0.14],
                lifetime: [1.5, 2.6],
                ..chips("clods", 20, EARTH, [0.6, 1.8])
            },
            EmitterDesc {
                shape: EmitShape::Disc { radius: 0.3 },
                velocity: [0.0, 1.4, 0.0],
                size: [0.015, 0.035],
                ..chips("grit", 24, GRAVE_DUST, [0.4, 1.4])
            },
        ],
    )
}

/// A raised body collapsing into ash: a column of grey ash pouring down where it stood, a puff
/// rolling out over the ground as it lands, and flakes drifting down after.
fn ash_impact() -> EffectDesc {
    effect(
        "spell.ash.impact",
        "A raised body collapsing into ash: a column of grey ash pouring down where it stood, a \
         puff rolling out along the ground as it lands and flakes drifting down after. Scale 1 \
         is a standing body.",
        Some(0.5),
        vec![
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 0.3,
                    surface: false,
                },
                offset: [0.0, 0.9, 0.0],
                velocity: [0.0, -1.1, 0.0],
                drag: 1.2,
                lifetime: [0.9, 1.5],
                size_over_life: c(&[(0.0, 0.8), (1.0, 1.8)]),
                sprite: Sprite::Dust,
                ..dust_puff("column", 14, ASH, 0.6, [0.0, 0.3], [0.35, 0.5])
            },
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.25,
                    thickness: 0.15,
                },
                offset: [0.0, 0.1, 0.0],
                delay: 0.35,
                drag: 2.5,
                lifetime: [1.6, 2.6],
                sprite: Sprite::Dust,
                ..dust_puff("settle", 16, ASH, 0.5, [1.0, 2.0], [0.4, 0.6])
            },
            EmitterDesc {
                bursts: once(60),
                shape: EmitShape::Sphere {
                    radius: 0.35,
                    surface: false,
                },
                offset: [0.0, 1.2, 0.0],
                speed: [0.05, 0.4],
                gravity: 0.08,
                drag: 2.0,
                turbulence: 1.2,
                lifetime: [2.0, 3.5],
                size: [0.012, 0.025],
                spin_deg: [-360.0, 360.0],
                sprite: Sprite::Shard,
                lit: 1.0,
                color: g(&[
                    (0.0, [ASH[0], ASH[1], ASH[2], 0.0]),
                    (0.1, [ASH[0], ASH[1], ASH[2], 0.9]),
                    (0.8, [ASH[0], ASH[1], ASH[2], 0.9]),
                    (1.0, [ASH[0], ASH[1], ASH[2], 0.0]),
                ]),
                soft: 0.0,
                wind: 0.6,
                ..em("flakes")
            },
        ],
    )
}

/// A skeleton falling apart: bone splinters and a few long shards scattering and clattering on
/// the floor, and a puff of pale bone dust.
fn bones_impact() -> EffectDesc {
    effect(
        "spell.bones.impact",
        "A skeleton falling apart: bone splinters and long shards scattering and clattering on \
         the floor, and a puff of pale bone dust. Scale 1 is a standing skeleton (cue about 0.8).",
        Some(0.4),
        vec![
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 0.25,
                    surface: false,
                },
                offset: [0.0, 0.7, 0.0],
                size: [0.05, 0.1],
                debris: Some(Debris::Splinter),
                ..chips("splinters", 30, BONE, [1.0, 3.0])
            },
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 0.2,
                    surface: false,
                },
                offset: [0.0, 0.6, 0.0],
                size: [0.16, 0.24],
                spin_deg: [-300.0, 300.0],
                lifetime: [2.5, 3.5],
                debris: Some(Debris::Splinter),
                ..chips("shards", 6, [0.78, 0.74, 0.64], [0.6, 1.6])
            },
            EmitterDesc {
                shape: EmitShape::Sphere {
                    radius: 0.3,
                    surface: false,
                },
                offset: [0.0, 0.6, 0.0],
                velocity: [0.0, -0.3, 0.0],
                lifetime: [1.2, 2.0],
                sprite: Sprite::Dust,
                ..dust_puff(
                    "dust",
                    12,
                    [0.62, 0.59, 0.53],
                    0.55,
                    [0.4, 1.2],
                    [0.35, 0.55],
                )
            },
            EmitterDesc {
                shape: EmitShape::Ring {
                    radius: 0.2,
                    thickness: 0.1,
                },
                offset: [0.0, 0.08, 0.0],
                delay: 0.25,
                lifetime: [1.0, 1.6],
                sprite: Sprite::Dust,
                ..dust_puff("skirt", 10, GRAVE_DUST, 0.35, [1.5, 2.5], [0.3, 0.45])
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_raised_dead_are_matter_that_never_glows() {
        let all = all();
        for name in [
            "spell.raise.impact",
            "spell.ash.impact",
            "spell.bones.impact",
        ] {
            let e = all.iter().find(|e| e.name == name).expect(name);
            assert!(e.light.is_none(), "{name}: casts no light");
            assert!(e.duration.is_some(), "{name}: an impact ends");
            assert!(
                e.emitters
                    .iter()
                    .all(|em| em.emissive == 0.0 && em.additive == 0.0),
                "{name}: dust and bone, never a flash"
            );
        }
    }
}
