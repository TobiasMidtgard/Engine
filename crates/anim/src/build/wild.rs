//! Wild animals for the NPC module's wildlife (characters week A4): a red deer, a grey wolf, a
//! wild boar and a European rabbit, asked for as `BodyRequest` kinds `deer`, `wolf`, `boar` and
//! `rabbit`.
//!
//! All four share one rig, the dog's ([`super::dog`]): a body and a loin, a two-bone neck and
//! the head with its jaw, two ears, a four-bone tail, forelegs hung from shoulder blades and
//! hind legs from the loin, each ending in a `foot` bone (a cloven hoof, a paw, a hare's long
//! foot). What differs is where the joints are ([`Frame`]), the body grown over them (smooth
//! signed-distance shapes, [`super::sdf`]), the coat, the gaits and the actions.
//!
//! Each kind is built at its typical adult's shoulder height (`scale` 1) and scales from there
//! with dynamic similarity, as the dog does: speeds with √size, stride rates with 1/√size.
//! The look varies with the request: a deer taller than the typical adult (a stag) grows
//! antlers sized by its seed, a young deer wears a fawn's spots, and the coat's shade shifts a
//! little with the seed.
//!
//! The actions ([`crate::actions`]) hold while their parameter is up: `graze` (head to the
//! ground: browsing, a wolf eating, a boar rooting, a rabbit nibbling), `lie` (or the older
//! `rest`), `alert` (head up and still; a rabbit sits up on its haunches) and `downed` (it
//! falls on its side and stays down). The triggers `flinch` (all), `rear` (deer) and `howl`
//! (wolf) play once, even on the move.

// Legs and ears are built per side by index: `side` picks the name, the mirror and the
// sign together.
#![allow(clippy::needless_range_loop)]

use engine_core::prelude::*;

use super::quadruped::{Leg, Rig, Stance, sample, sized_name};
use super::sdf::{Body, Paint, Shape};
use super::{PoseBuilder, bone, cycle_sin, keyed};
use crate::clip::AnimationClip;
use crate::graph::{AnimGraph, NodeDef};
use crate::procedural::gait::{FootKind, GaitLeg, GaitStage, GaitSway, LegStyle, LeggedGait};
use crate::procedural::spring::SpringChainDef;
use crate::skeleton::Skeleton;
use crate::skin::{SkinMaterial, SkinnedMesh};

/// Which wild animal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Wild {
    Deer,
    Wolf,
    Boar,
    Rabbit,
}

pub const ALL: [Wild; 4] = [Wild::Deer, Wild::Wolf, Wild::Boar, Wild::Rabbit];

const SIDE: [&str; 2] = ["l", "r"];

impl Wild {
    /// The animal a `BodyRequest` kind asks for.
    pub fn of_kind(kind: &str) -> Option<Wild> {
        match kind {
            "deer" | "stag" | "doe" | "hind" | "fawn" | "red_deer" => Some(Wild::Deer),
            "wolf" | "grey_wolf" | "gray_wolf" => Some(Wild::Wolf),
            "boar" | "wild_boar" | "sow" | "pig" => Some(Wild::Boar),
            "rabbit" | "hare" | "bunny" => Some(Wild::Rabbit),
            _ => None,
        }
    }

    /// The animal a skeleton is (its name before any size).
    pub fn of_skeleton(name: &str) -> Option<Wild> {
        let family = name.split('@').next().unwrap_or_default();
        ALL.into_iter().find(|w| w.name() == family)
    }

    pub fn name(self) -> &'static str {
        match self {
            Wild::Deer => "deer",
            Wild::Wolf => "wolf",
            Wild::Boar => "boar",
            Wild::Rabbit => "rabbit",
        }
    }

    /// The typical adult's height at the withers (scale 1), m.
    pub fn withers(self) -> f32 {
        match self {
            Wild::Deer => 1.2,
            Wild::Wolf => 0.8,
            Wild::Boar => 0.85,
            Wild::Rabbit => 0.2,
        }
    }

    /// The typical adult's weight, kg.
    pub fn mass_kg(self) -> f32 {
        match self {
            Wild::Deer => 150.0,
            Wild::Wolf => 40.0,
            Wild::Boar => 100.0,
            Wild::Rabbit => 1.8,
        }
    }

    /// The graph (one per kind; its idle only rotates bones, so every size shares it).
    pub fn graph_name(self) -> String {
        self.name().to_string()
    }

    fn idle_name(self) -> String {
        format!("{}_idle", self.name())
    }

    fn hoofed(self) -> bool {
        matches!(self, Wild::Deer | Wild::Boar)
    }
}

/// The skeleton name for `kind` at `scale` (sizes kept to whole centimeters at the withers;
/// a rabbit's to millimeters).
pub fn skeleton_name(kind: Wild, scale: f32) -> String {
    let unit = if kind == Wild::Rabbit { 1000.0 } else { 100.0 };
    let here = (scale * kind.withers() * unit).round() as i32;
    if here == (kind.withers() * unit).round() as i32 {
        kind.name().into()
    } else if kind == Wild::Rabbit {
        format!("{}@{here}mm", kind.name())
    } else {
        format!("{}@{here}cm", kind.name())
    }
}

/// The scale a requested withers height asks for (rounded as [`skeleton_name`] rounds).
pub fn scale_for_height(kind: Wild, height: f32) -> f32 {
    let w = kind.withers();
    let unit = if kind == Wild::Rabbit { 1000.0 } else { 100.0 };
    let h = (height.clamp(w * 0.3, w * 1.6) * unit).round() / unit;
    h / w
}

/// How one animal looks beyond its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Antler size (0 none, 1 a full royal's).
    pub antlers: f32,
    /// A fawn's white spots.
    pub spots: bool,
    /// A piglet's pale stripes along its flanks.
    pub stripes: bool,
    /// Coat shade, -1 (darker) .. 1 (paler).
    pub shade: f32,
}

impl Look {
    pub const PLAIN: Look = Look {
        antlers: 0.0,
        spots: false,
        stripes: false,
        shade: 0.0,
    };

    /// The look a request gets: a stag (a deer above the typical adult, or asked for as
    /// `stag`) grows antlers sized by the seed; a deer under 70% of adult size is a spotted
    /// fawn, a boar under 65% a striped piglet; the seed shades the coat.
    pub fn of(kind: Wild, asked: &str, scale: f32, seed: u64) -> Look {
        let h = |n: u64| {
            let mut x = seed ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            x ^= x >> 31;
            x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x ^= x >> 29;
            (x % 1000) as f32 / 999.0
        };
        let stag = kind == Wild::Deer && (asked == "stag" || scale > 1.04);
        Look {
            antlers: if stag { 0.7 + 0.3 * h(1) } else { 0.0 },
            spots: kind == Wild::Deer && scale < 0.7,
            stripes: kind == Wild::Boar && scale < 0.65,
            shade: (h(2) - 0.5) * 2.0,
        }
    }

    /// A key for meshes of this look (looks are quantized so a herd shares a few meshes).
    pub fn key(&self) -> String {
        format!(
            "a{}s{}t{}c{}",
            (self.antlers * 4.0).round() as i32,
            u8::from(self.spots),
            u8::from(self.stripes),
            (self.shade * 2.0).round() as i32
        )
    }

    /// The look as its key says it (quantized).
    fn quantized(self) -> Look {
        Look {
            antlers: (self.antlers * 4.0).round() / 4.0,
            spots: self.spots,
            stripes: self.stripes,
            shade: (self.shade * 2.0).round() / 2.0,
        }
    }
}

/// Where the joints are at scale 1 (model space, m, facing −Z, ground at y 0); legs given
/// for the right side (+X).
struct Frame {
    /// Top of the shoulder blade, shoulder, elbow, carpus, fetlock (or paw).
    front: [Vec3; 5],
    /// Hip, stifle, hock, fetlock (or the toes).
    hind: [Vec3; 4],
    body: Vec3,
    loin: Vec3,
    /// Neck base, mid-neck, the head's joint (the poll).
    neck: [Vec3; 3],
    jaw: Vec3,
    ear: Vec3,
    tail: [Vec3; 5],
    /// The foot bone's height above the ground when standing, m.
    sole: f32,
    /// Half the trunk's height at the body bone (how low it can lie), m.
    belly: f32,
    /// Half the trunk's width (how it lies on its side), m.
    flank: f32,
    /// The nose tip from the head joint (head frame, rest).
    muzzle: Vec3,
}

const fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn frame(kind: Wild) -> Frame {
    match kind {
        Wild::Deer => Frame {
            front: [
                v(0.08, 1.12, -0.40),
                v(0.115, 0.93, -0.55),
                v(0.12, 0.73, -0.43),
                v(0.105, 0.40, -0.46),
                v(0.105, 0.11, -0.47),
            ],
            hind: [
                v(0.10, 1.07, 0.46),
                v(0.125, 0.78, 0.30),
                v(0.105, 0.47, 0.60),
                v(0.105, 0.11, 0.57),
            ],
            body: v(0.0, 1.0, -0.2),
            loin: v(0.0, 1.04, 0.24),
            neck: [
                v(0.0, 0.96, -0.52),
                v(0.0, 1.25, -0.69),
                v(0.0, 1.52, -0.80),
            ],
            jaw: v(0.0, 1.44, -0.88),
            ear: v(0.065, 1.6, -0.8),
            tail: [
                v(0.0, 1.06, 0.64),
                v(0.0, 1.02, 0.69),
                v(0.0, 0.97, 0.72),
                v(0.0, 0.93, 0.73),
                v(0.0, 0.89, 0.74),
            ],
            sole: 0.11,
            belly: 0.24,
            flank: 0.2,
            muzzle: v(0.0, -0.15, -0.34),
        },
        Wild::Wolf => Frame {
            front: [
                v(0.055, 0.73, -0.30),
                v(0.075, 0.585, -0.40),
                v(0.08, 0.42, -0.29),
                v(0.072, 0.15, -0.32),
                v(0.072, 0.048, -0.335),
            ],
            hind: [
                v(0.07, 0.64, 0.31),
                v(0.082, 0.43, 0.2),
                v(0.07, 0.2, 0.355),
                v(0.07, 0.048, 0.345),
            ],
            body: v(0.0, 0.62, -0.17),
            loin: v(0.0, 0.65, 0.16),
            neck: [
                v(0.0, 0.60, -0.36),
                v(0.0, 0.75, -0.48),
                v(0.0, 0.86, -0.56),
            ],
            jaw: v(0.0, 0.80, -0.6),
            ear: v(0.05, 0.94, -0.57),
            tail: [
                v(0.0, 0.66, 0.42),
                v(0.0, 0.58, 0.50),
                v(0.0, 0.48, 0.55),
                v(0.0, 0.38, 0.57),
                v(0.0, 0.28, 0.58),
            ],
            sole: 0.048,
            belly: 0.16,
            flank: 0.12,
            muzzle: v(0.0, -0.02, -0.28),
        },
        Wild::Boar => Frame {
            front: [
                v(0.07, 0.79, -0.38),
                v(0.11, 0.57, -0.50),
                v(0.12, 0.39, -0.40),
                v(0.10, 0.19, -0.43),
                v(0.10, 0.065, -0.45),
            ],
            hind: [
                v(0.10, 0.68, 0.44),
                v(0.12, 0.46, 0.32),
                v(0.10, 0.23, 0.52),
                v(0.10, 0.065, 0.50),
            ],
            body: v(0.0, 0.60, -0.18),
            loin: v(0.0, 0.64, 0.24),
            neck: [v(0.0, 0.58, -0.5), v(0.0, 0.62, -0.63), v(0.0, 0.62, -0.74)],
            jaw: v(0.0, 0.48, -0.86),
            ear: v(0.075, 0.76, -0.76),
            tail: [
                v(0.0, 0.67, 0.62),
                v(0.0, 0.60, 0.66),
                v(0.0, 0.53, 0.68),
                v(0.0, 0.46, 0.685),
                v(0.0, 0.40, 0.69),
            ],
            sole: 0.065,
            belly: 0.22,
            flank: 0.19,
            muzzle: v(0.0, -0.21, -0.39),
        },
        Wild::Rabbit => Frame {
            front: [
                v(0.025, 0.16, -0.08),
                v(0.033, 0.12, -0.11),
                v(0.035, 0.08, -0.085),
                v(0.03, 0.03, -0.1),
                v(0.03, 0.01, -0.115),
            ],
            hind: [
                v(0.035, 0.14, 0.10),
                v(0.05, 0.08, 0.035),
                v(0.045, 0.028, 0.12),
                v(0.045, 0.01, 0.0),
            ],
            body: v(0.0, 0.14, -0.04),
            loin: v(0.0, 0.16, 0.06),
            neck: [v(0.0, 0.125, -0.1), v(0.0, 0.17, -0.14), v(0.0, 0.2, -0.16)],
            jaw: v(0.0, 0.18, -0.19),
            ear: v(0.016, 0.23, -0.155),
            tail: [
                v(0.0, 0.15, 0.15),
                v(0.0, 0.155, 0.165),
                v(0.0, 0.16, 0.175),
                v(0.0, 0.165, 0.185),
                v(0.0, 0.17, 0.19),
            ],
            sole: 0.01,
            belly: 0.09,
            flank: 0.055,
            muzzle: v(0.0, -0.012, -0.062),
        },
    }
}

/// `p` on the side `side` (0 left, 1 right) from its right-side position.
fn sided(p: Vec3, side: usize) -> Vec3 {
    if side == 0 { v(-p.x, p.y, p.z) } else { p }
}

fn name(prefix: &str, part: &str, side: usize) -> String {
    format!("{prefix}_{part}_{}", SIDE[side])
}

pub fn skeleton(kind: Wild, scale: f32) -> Skeleton {
    let f = frame(kind);
    let i = Quat::IDENTITY;
    let s = scale;
    let n = f.neck;
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), f.body * s, i),
        bone("loin", Some("body"), (f.loin - f.body) * s, i),
        bone("neck_1", Some("body"), (n[0] - f.body) * s, i),
        bone("neck_2", Some("neck_1"), (n[1] - n[0]) * s, i),
        bone("head", Some("neck_2"), (n[2] - n[1]) * s, i),
        bone("jaw", Some("head"), (f.jaw - n[2]) * s, i),
        bone("tail_1", Some("loin"), (f.tail[0] - f.loin) * s, i),
        bone("tail_2", Some("tail_1"), (f.tail[1] - f.tail[0]) * s, i),
        bone("tail_3", Some("tail_2"), (f.tail[2] - f.tail[1]) * s, i),
        bone("tail_4", Some("tail_3"), (f.tail[3] - f.tail[2]) * s, i),
    ];
    for side in 0..2 {
        bones.push(bone(
            &format!("ear_{}", SIDE[side]),
            Some("head"),
            (sided(f.ear, side) - n[2]) * s,
            i,
        ));
        let [blade, shoulder, elbow, carpus, foot] = f.front.map(|p| sided(p, side));
        let scapula = name("front", "scapula", side);
        bones.push(bone(&scapula, Some("body"), (blade - f.body) * s, i));
        let hind = f.hind.map(|p| sided(p, side));
        for (prefix, parent, start, j) in [
            (
                "front",
                scapula.as_str(),
                blade,
                [shoulder, elbow, carpus, foot],
            ),
            ("hind", "loin", f.loin, hind),
        ] {
            let upper = name(prefix, "upper", side);
            let lower = name(prefix, "lower", side);
            let cannon = name(prefix, "cannon", side);
            bones.push(bone(&upper, Some(parent), (j[0] - start) * s, i));
            bones.push(bone(&lower, Some(&upper), (j[1] - j[0]) * s, i));
            bones.push(bone(&cannon, Some(&lower), (j[2] - j[1]) * s, i));
            bones.push(bone(
                &name(prefix, "foot", side),
                Some(&cannon),
                (j[3] - j[2]) * s,
                i,
            ));
        }
    }
    Skeleton::new(&skeleton_name(kind, scale), bones).expect("wild skeleton")
}

// ------------------------------------------------------------------------------------------
// The body.

/// The coat's colors (linear RGB) and how much fur grows on each patch.
struct Coat {
    back: Paint,
    side: Paint,
    under: Paint,
    leg: Paint,
    foot: Paint,
    face: Paint,
    muzzle: Paint,
    ear: Paint,
    tail: Paint,
}

fn shade(rgb: [f32; 3], k: f32) -> [f32; 3] {
    let m = 1.0 + 0.12 * k;
    rgb.map(|c| (c * m).min(1.0))
}

fn coat(kind: Wild, look: Look) -> Coat {
    let f = |rgb: [f32; 3], amount: f32| Paint::fur(shade(rgb, look.shade), amount);
    match kind {
        // Red deer in summer coat: red-brown, a buff rump and belly, darker legs and face.
        Wild::Deer => Coat {
            back: f([0.12, 0.062, 0.034], 0.9),
            side: f([0.15, 0.08, 0.043], 0.85),
            under: f([0.34, 0.26, 0.17], 0.7),
            leg: f([0.11, 0.065, 0.038], 0.4),
            foot: f([0.1, 0.055, 0.03], 0.25),
            face: f([0.13, 0.075, 0.045], 0.3),
            muzzle: f([0.07, 0.045, 0.03], 0.1),
            ear: f([0.14, 0.09, 0.055], 0.45),
            tail: f([0.1, 0.06, 0.032], 0.8),
        },
        // Grey wolf: an agouti grey with a dark saddle, cream underparts and cheeks, tawny
        // legs, a black-tipped tail.
        Wild::Wolf => Coat {
            back: f([0.075, 0.065, 0.055], 1.0),
            side: f([0.2, 0.17, 0.13], 0.95),
            under: f([0.5, 0.43, 0.32], 0.85),
            leg: f([0.3, 0.21, 0.12], 0.45),
            foot: f([0.33, 0.25, 0.16], 0.3),
            face: f([0.24, 0.2, 0.15], 0.4),
            muzzle: f([0.45, 0.39, 0.3], 0.18),
            ear: f([0.17, 0.12, 0.08], 0.5),
            tail: f([0.16, 0.14, 0.11], 1.0),
        },
        // A piglet: a ginger-brown humbug coat (its pale stripes are painted on in
        // `boar_body`), softer and shorter.
        Wild::Boar if look.stripes => Coat {
            back: f([0.075, 0.042, 0.022], 0.9),
            side: f([0.13, 0.072, 0.036], 0.85),
            under: f([0.2, 0.135, 0.075], 0.7),
            leg: f([0.06, 0.038, 0.022], 0.5),
            foot: f([0.03, 0.022, 0.016], 0.3),
            face: f([0.12, 0.07, 0.038], 0.5),
            muzzle: f([0.06, 0.04, 0.028], 0.25),
            ear: f([0.06, 0.04, 0.025], 0.6),
            tail: f([0.05, 0.03, 0.018], 0.5),
        },
        // Wild boar: coarse grizzled grey-brown bristles, a long black mane along the spine.
        Wild::Boar => Coat {
            back: f([0.025, 0.02, 0.016], 1.2),
            side: f([0.06, 0.048, 0.036], 1.05),
            under: f([0.08, 0.062, 0.046], 0.6),
            leg: f([0.035, 0.028, 0.022], 0.5),
            foot: f([0.03, 0.025, 0.02], 0.3),
            face: f([0.07, 0.055, 0.04], 0.45),
            muzzle: f([0.05, 0.04, 0.032], 0.25),
            ear: f([0.04, 0.032, 0.025], 0.6),
            tail: f([0.03, 0.024, 0.018], 0.5),
        },
        // European rabbit: agouti grey-brown, a rufous nape, white belly and scut.
        Wild::Rabbit => Coat {
            back: f([0.2, 0.135, 0.075], 1.0),
            side: f([0.25, 0.18, 0.11], 0.95),
            under: f([0.62, 0.58, 0.52], 0.85),
            leg: f([0.24, 0.17, 0.1], 0.7),
            foot: f([0.34, 0.27, 0.19], 0.6),
            face: f([0.22, 0.15, 0.085], 0.55),
            muzzle: f([0.3, 0.23, 0.15], 0.35),
            ear: f([0.2, 0.14, 0.08], 0.35),
            tail: f([0.66, 0.62, 0.56], 1.0),
        },
    }
}

/// Leg girths at scale 1 (radii, m).
struct Girth {
    scapula: Vec3,
    arm: (f32, f32),
    forearm: (f32, f32),
    front_cannon: (f32, f32),
    haunch: Vec3,
    thigh: (f32, f32),
    gaskin: (f32, f32),
    hind_cannon: (f32, f32),
}

fn girth(kind: Wild) -> Girth {
    match kind {
        Wild::Deer => Girth {
            scapula: v(0.06, 0.15, 0.11),
            arm: (0.085, 0.065),
            forearm: (0.05, 0.03),
            front_cannon: (0.022, 0.02),
            haunch: v(0.06, 0.15, 0.15),
            thigh: (0.088, 0.066),
            gaskin: (0.06, 0.028),
            hind_cannon: (0.022, 0.02),
        },
        Wild::Wolf => Girth {
            scapula: v(0.045, 0.1, 0.075),
            arm: (0.06, 0.047),
            forearm: (0.036, 0.024),
            front_cannon: (0.023, 0.024),
            haunch: v(0.06, 0.115, 0.1),
            thigh: (0.07, 0.05),
            gaskin: (0.045, 0.023),
            hind_cannon: (0.022, 0.023),
        },
        Wild::Boar => Girth {
            scapula: v(0.06, 0.13, 0.11),
            arm: (0.086, 0.07),
            forearm: (0.055, 0.035),
            front_cannon: (0.03, 0.028),
            haunch: v(0.06, 0.12, 0.12),
            thigh: (0.07, 0.05),
            gaskin: (0.05, 0.03),
            hind_cannon: (0.028, 0.026),
        },
        Wild::Rabbit => Girth {
            scapula: v(0.016, 0.03, 0.025),
            arm: (0.018, 0.013),
            forearm: (0.009, 0.007),
            front_cannon: (0.007, 0.0065),
            haunch: v(0.028, 0.045, 0.05),
            thigh: (0.03, 0.02),
            gaskin: (0.016, 0.009),
            hind_cannon: (0.009, 0.009),
        },
    }
}

/// Grows the animal's body at `scale`.
struct Grow<'a> {
    b: &'a mut Body,
    s: f32,
}

impl Grow<'_> {
    fn el(&mut self, bone: &str, c: Vec3, r: Vec3, paint: Paint, blend: f32) -> &mut Self {
        self.b.part(
            bone,
            Shape::ellipsoid(c * self.s, r * self.s),
            paint,
            blend * self.s,
        );
        self
    }

    fn el_rot(
        &mut self,
        bone: &str,
        c: Vec3,
        r: Vec3,
        q: Quat,
        paint: Paint,
        blend: f32,
    ) -> &mut Self {
        self.b.part(
            bone,
            Shape::ellipsoid(c * self.s, r * self.s).rotated(q),
            paint,
            blend * self.s,
        );
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn cap(
        &mut self,
        bone: &str,
        a: Vec3,
        b: Vec3,
        ra: f32,
        rb: f32,
        paint: Paint,
        blend: f32,
    ) -> &mut Self {
        self.b.part(
            bone,
            Shape::capsule(a * self.s, b * self.s, ra * self.s, rb * self.s),
            paint,
            blend * self.s,
        );
        self
    }

    fn muscle(&mut self, name: &str, c: Vec3, r: Vec3, bulge: f32, sway: Vec3) -> &mut Self {
        self.b
            .muscle(name, c * self.s, r * self.s, bulge * self.s, sway * self.s);
        self
    }
}

const HOOF: Paint = Paint {
    color: Vec4::new(0.03, 0.026, 0.022, 1.0),
    mask: Vec4::ZERO,
    fur: 0.0,
};
const BLACK: Paint = Paint {
    color: Vec4::new(0.012, 0.01, 0.01, 1.0),
    mask: Vec4::ZERO,
    fur: 0.0,
};

/// A cloven hoof under the fetlock `p` (the foot bone): the pastern down to two claws on the
/// ground and two dewclaws behind. `w` is the hoof's size (m, its length).
fn cloven(g: &mut Grow, bone: &str, p: Vec3, w: f32, coat: Paint) {
    let coronet = v(p.x, w * 0.42, p.z - w * 0.3);
    g.cap(bone, p, coronet, w * 0.3, w * 0.27, coat, w * 0.12);
    for dx in [-1.0f32, 1.0] {
        let x = dx * w * 0.17;
        g.el_rot(
            bone,
            v(coronet.x + x, w * 0.24, coronet.z - w * 0.2),
            v(w * 0.15, w * 0.25, w * 0.42),
            Quat::from_rotation_x(-0.5),
            HOOF,
            w * 0.06,
        );
        // Dewclaws, small and dark above the heel.
        g.el(
            bone,
            v(p.x + x * 1.1, p.y - w * 0.15, p.z + w * 0.28),
            Vec3::splat(w * 0.08),
            HOOF,
            w * 0.03,
        );
    }
}

/// A paw on the foot bone at `p`: the foot, four toes with claws, pads. `w` is its length.
fn paw(g: &mut Grow, bone: &str, p: Vec3, w: f32, coat: Paint) {
    let pad = Paint::hard([0.03, 0.022, 0.02]);
    g.el(
        bone,
        p + v(0.0, -w * 0.11, -w * 0.25),
        v(w * 0.36, w * 0.3, w * 0.47),
        coat,
        w * 0.11,
    );
    for (k, dx) in [-0.24f32, -0.085, 0.085, 0.24].into_iter().enumerate() {
        let front = if k == 1 || k == 2 { -w * 0.17 } else { 0.0 };
        g.el(
            bone,
            p + v(dx * w, -w * 0.28, -w * 0.62 + front),
            v(w * 0.15, w * 0.17, w * 0.2),
            coat,
            w * 0.055,
        );
        g.cap(
            bone,
            p + v(dx * w * 1.05, -w * 0.3, -w * 0.78 + front),
            p + v(dx * w * 1.1, -w * 0.42, -w * 0.89 + front),
            w * 0.05,
            w * 0.02,
            BLACK,
            w * 0.03,
        );
    }
    g.el(
        bone,
        p + v(0.0, -w * 0.42, -w * 0.3),
        v(w * 0.28, w * 0.08, w * 0.25),
        pad,
        w * 0.04,
    );
}

/// The legs (both sides), with their muscles.
fn legs(g: &mut Grow, kind: Wild, f: &Frame, c: &Coat) {
    let gr = girth(kind);
    let k = kind.withers() / 0.56;
    for side in 0..2 {
        let x = if side == 0 { -1.0 } else { 1.0 };
        let [blade, sh, el, ca, ft] = f.front.map(|p| sided(p, side));
        let fr = |part: &str| name("front", part, side);
        g.el(
            &fr("scapula"),
            (blade + sh) * 0.5 + v(0.012 * k * x, 0.0, 0.0),
            gr.scapula,
            c.side,
            0.04 * k,
        )
        .cap(&fr("upper"), sh, el, gr.arm.0, gr.arm.1, c.side, 0.025 * k)
        .el(
            &fr("upper"),
            el + v(0.005 * k * x, 0.05 * k, 0.012 * k),
            v(gr.forearm.0 * 1.15, gr.forearm.0 * 2.0, gr.forearm.0 * 1.4),
            c.leg,
            0.02 * k,
        )
        .cap(
            &fr("lower"),
            el,
            ca,
            gr.forearm.0,
            gr.forearm.1,
            c.leg,
            0.015 * k,
        )
        .el(
            &fr("cannon"),
            ca,
            Vec3::splat(gr.forearm.1 * 1.1),
            c.leg,
            0.01 * k,
        )
        .cap(
            &fr("cannon"),
            ca,
            ft,
            gr.front_cannon.0,
            gr.front_cannon.1,
            c.foot,
            0.008 * k,
        );
        let [hip, st, ho, hf] = f.hind.map(|p| sided(p, side));
        let hn = |part: &str| name("hind", part, side);
        g.el(
            &hn("upper"),
            hip + v(0.0, -gr.haunch.y * 0.62, 0.005 * k),
            gr.haunch,
            c.side,
            0.04 * k,
        )
        .cap(
            &hn("upper"),
            hip,
            st,
            gr.thigh.0,
            gr.thigh.1,
            c.side,
            0.03 * k,
        )
        .cap(
            &hn("lower"),
            st,
            ho,
            gr.gaskin.0,
            gr.gaskin.1,
            c.leg,
            0.015 * k,
        )
        .el(
            &hn("lower"),
            st + (ho - st) * 0.3 + v(0.0, 0.0, 0.012 * k),
            v(gr.gaskin.0 * 0.85, gr.gaskin.0 * 1.5, gr.gaskin.0 * 0.95),
            c.leg,
            0.012 * k,
        )
        .el(
            &hn("cannon"),
            ho + v(0.0, 0.0, 0.01 * k),
            Vec3::splat(gr.hind_cannon.0 * 1.1),
            c.leg,
            0.008 * k,
        )
        .cap(
            &hn("cannon"),
            ho,
            hf,
            gr.hind_cannon.0,
            gr.hind_cannon.1,
            c.foot,
            0.008 * k,
        );
        match kind {
            Wild::Deer => {
                cloven(g, &fr("foot"), ft, 0.075, c.foot);
                cloven(g, &hn("foot"), hf, 0.072, c.foot);
            }
            Wild::Boar => {
                cloven(g, &fr("foot"), ft, 0.07, c.foot);
                cloven(g, &hn("foot"), hf, 0.066, c.foot);
            }
            Wild::Wolf => {
                paw(g, &fr("foot"), ft, 0.1, c.foot);
                paw(g, &hn("foot"), hf, 0.09, c.foot);
            }
            Wild::Rabbit => {
                g.el(
                    &fr("foot"),
                    ft + v(0.0, -0.002, -0.01),
                    v(0.009, 0.007, 0.014),
                    c.foot,
                    0.003,
                );
                // The long hind foot lies along the cannon; the toes end it.
                g.el(&hn("foot"), hf, v(0.012, 0.008, 0.02), c.foot, 0.004);
            }
        }
        // Muscles (named as the dog's, so the same muscle rig drives them).
        let m = |n: &str| format!("{n}_{}", SIDE[side]);
        let gk = gr.haunch / v(0.052, 0.098, 0.085);
        g.muscle(
            &m("haunch"),
            v(
                x * (hip.x.abs() + gr.haunch.x * 0.75),
                hip.y - 0.05 * k,
                hip.z - 0.01 * k,
            ),
            v(0.05, 0.1, 0.09) * gk,
            0.006 * k,
            v(0.0, -0.003 * k, 0.0),
        )
        .muscle(
            &m("shoulder"),
            v(
                x * (sh.x.abs() + gr.arm.0 * 0.8),
                sh.y + 0.02 * k,
                sh.z + 0.04 * k,
            ),
            v(0.045, 0.09, 0.07) * gk,
            0.004 * k,
            v(0.0, -0.002 * k, 0.0),
        )
        .muscle(
            &m("forearm"),
            el + v(0.012 * k * x, -0.05 * k, -0.005 * k),
            v(gr.forearm.0 * 0.85, gr.forearm.0 * 1.8, gr.forearm.0 * 0.95),
            0.003 * k,
            Vec3::ZERO,
        )
        .muscle(
            &m("gaskin"),
            st + v(0.012 * k * x, -0.05 * k, 0.035 * k),
            v(gr.gaskin.0 * 0.7, gr.gaskin.0 * 1.35, gr.gaskin.0 * 0.8),
            0.003 * k,
            Vec3::ZERO,
        );
    }
    let n = f.neck;
    let crest = match kind {
        Wild::Deer => 0.1,
        Wild::Wolf => 0.09,
        Wild::Boar => 0.17,
        Wild::Rabbit => 0.03,
    };
    g.muscle(
        "neck",
        (n[0] + n[1]) * 0.5 + v(0.0, crest, 0.0),
        v(0.08, 0.08, 0.07) * k,
        0.003 * k,
        Vec3::ZERO,
    )
    .muscle(
        "belly",
        v(0.0, f.body.y - f.belly * 0.95, (f.body.z + f.loin.z) * 0.5),
        v(0.08, 0.06, 0.16) * k,
        0.002 * k,
        v(0.0, -0.008 * k, 0.0),
    );
}

/// Eyes: a dark rim and the glossy eye, at `p` (right side, mirrored), radius `r`.
fn eyes(g: &mut Grow, p: Vec3, r: f32, iris: [f32; 3]) {
    for side in 0..2 {
        let at = sided(p, side);
        g.el(
            "head",
            at - v(0.0, 0.0, 0.0),
            Vec3::splat(r * 1.15),
            Paint::hard([0.02, 0.014, 0.01]),
            r * 0.15,
        )
        .el(
            "head",
            at + v(if side == 0 { -r * 0.25 } else { r * 0.25 }, 0.0, -r * 0.2),
            Vec3::splat(r),
            Paint::hard(iris),
            r * 0.08,
        );
    }
}

fn deer_body(g: &mut Grow, f: &Frame, c: &Coat, look: Look) {
    // Trunk: a deep, narrow rib cage, deepest just behind the elbows, the belly rising to a
    // tucked-up flank; the withers, a broad brisket ahead of the front legs, and the croup
    // running level into the hams rather than standing up as a ball. The pale underside only
    // under the belly, not up the flanks.
    g.el(
        "body",
        v(0.0, 0.96, -0.28),
        v(0.175, 0.255, 0.37),
        c.side,
        0.08,
    )
    .el(
        "body",
        v(0.0, 1.09, -0.42),
        v(0.12, 0.12, 0.17),
        c.back,
        0.06,
    )
    .el(
        "body",
        v(0.0, 0.88, -0.54),
        v(0.125, 0.17, 0.12),
        c.side,
        0.06,
    )
    .el(
        "body",
        v(0.0, 0.735, -0.3),
        v(0.085, 0.045, 0.22),
        c.under,
        0.05,
    )
    .el(
        "loin",
        v(0.0, 0.97, 0.12),
        v(0.168, 0.165, 0.27),
        c.side,
        0.08,
    )
    .el("loin", v(0.0, 1.09, 0.15), v(0.1, 0.06, 0.26), c.back, 0.05)
    .el("loin", v(0.0, 0.83, 0.0), v(0.08, 0.05, 0.2), c.under, 0.05)
    .el(
        "loin",
        v(0.0, 1.0, 0.45),
        v(0.168, 0.145, 0.2),
        c.side,
        0.08,
    )
    // The buff rump patch around the tail.
    .el(
        "loin",
        v(0.0, 0.97, 0.6),
        v(0.11, 0.15, 0.07),
        c.under,
        0.04,
    );
    // The hams: a deep, flat thigh from the croup down to the buttock and the gaskin, and
    // the fold of skin from the flank to the stifle, so the hind leg grows out of the body.
    for side in 0..2 {
        let hip = name("hind", "upper", side);
        g.el(
            &hip,
            sided(v(0.09, 0.87, 0.47), side),
            v(0.072, 0.16, 0.15),
            c.side,
            0.07,
        )
        .el(
            &hip,
            sided(v(0.1, 0.83, 0.27), side),
            v(0.06, 0.075, 0.1),
            c.side,
            0.07,
        );
    }
    // Neck: deeper than it is wide, broad where it joins the chest and slimming to the
    // throat; a stag's thicker, with a dark mane under it.
    let rut = look.antlers;
    let along = Quat::from_rotation_x(-0.46);
    let thick = 1.0 + 0.3 * rut;
    g.el_rot(
        "neck_1",
        v(0.0, 1.08, -0.59),
        v(0.088 * thick, 0.2, 0.125 * thick),
        along,
        c.side,
        0.07,
    )
    .el_rot(
        "neck_2",
        v(0.0, 1.36, -0.74),
        v(0.064 * (1.0 + 0.15 * rut), 0.16, 0.082 * (1.0 + 0.15 * rut)),
        along,
        c.side,
        0.05,
    )
    .el(
        "neck_2",
        v(0.0, 1.34, -0.775),
        v(0.04, 0.08, 0.03),
        c.under,
        0.03,
    );
    if rut > 0.0 {
        let mane = Paint::fur([0.07, 0.04, 0.025], 1.3 + 1.0 * rut);
        g.el_rot(
            "neck_1",
            v(0.0, 1.1, -0.7),
            v(0.065 * thick, 0.2, 0.06 * thick),
            along,
            mane,
            0.06,
        )
        .el_rot(
            "neck_2",
            v(0.0, 1.3, -0.79),
            v(0.05, 0.1, 0.04),
            along,
            mane,
            0.05,
        );
    }
    // Head: a long wedge, broad across the brow and eyes, narrowing to a slim muzzle and a
    // black nose; a deep jaw under the eyes closing up under the face, with no gap between.
    g.el(
        "head",
        v(0.0, 1.56, -0.845),
        v(0.07, 0.068, 0.085),
        c.face,
        0.03,
    )
    .el(
        "head",
        v(0.0, 1.535, -0.92),
        v(0.072, 0.055, 0.06),
        c.face,
        0.03,
    )
    .cap(
        "head",
        v(0.0, 1.525, -0.93),
        v(0.0, 1.405, -1.085),
        0.055,
        0.031,
        c.face,
        0.03,
    )
    .el(
        "head",
        v(0.0, 1.385, -1.1),
        v(0.034, 0.033, 0.04),
        c.muzzle,
        0.015,
    )
    .el(
        "head",
        v(0.0, 1.392, -1.132),
        v(0.026, 0.021, 0.015),
        BLACK,
        0.008,
    )
    .el(
        "jaw",
        v(0.0, 1.465, -0.885),
        v(0.056, 0.062, 0.07),
        c.face,
        0.03,
    )
    .cap(
        "jaw",
        v(0.0, 1.445, -0.925),
        v(0.0, 1.358, -1.08),
        0.044,
        0.027,
        c.face,
        0.025,
    )
    .el(
        "jaw",
        v(0.0, 1.348, -1.06),
        v(0.022, 0.014, 0.026),
        c.under,
        0.01,
    );
    eyes(g, v(0.064, 1.552, -0.925), 0.018, [0.03, 0.018, 0.01]);
    // Preorbital glands: dark slits ahead of the eyes.
    for side in 0..2 {
        g.cap(
            "head",
            sided(v(0.05, 1.53, -0.96), side),
            sided(v(0.045, 1.5, -1.0), side),
            0.006,
            0.004,
            BLACK,
            0.004,
        );
    }
    // Big, mobile ears set out to the sides.
    for side in 0..2 {
        let x = if side == 0 { -1.0 } else { 1.0 };
        let ear = format!("ear_{}", SIDE[side]);
        let q = Quat::from_rotation_z(-0.75 * x) * Quat::from_rotation_x(0.15);
        g.el_rot(
            &ear,
            sided(v(0.115, 1.66, -0.79), side),
            v(0.04, 0.095, 0.016),
            q,
            c.ear,
            0.012,
        )
        .el_rot(
            &ear,
            sided(v(0.112, 1.66, -0.8), side),
            v(0.028, 0.07, 0.006),
            q,
            Paint::fur([0.5, 0.42, 0.33], 0.6),
            0.004,
        );
    }
    // A short tail, dark above.
    for (k, (ra, rb)) in [(0.045, 0.04), (0.04, 0.035), (0.035, 0.03), (0.03, 0.022)]
        .into_iter()
        .enumerate()
    {
        g.cap(
            &format!("tail_{}", k + 1),
            f.tail[k],
            f.tail[k + 1],
            ra,
            rb,
            c.tail,
            0.015,
        );
    }
    if look.antlers > 0.0 {
        antlers(g, f, look.antlers);
    }
}

/// A fawn's dapples: rows of white spots either side of the spine and down the flanks, set
/// just under the coat's surface (found by bisection) so they colour it without standing up.
fn fawn_spots(g: &mut Grow) {
    let spot = Paint::fur([0.74, 0.68, 0.58], 0.6);
    let r = 0.02;
    // (angle up from level, m; first z, spots)
    for (row, (up, from, n)) in [
        (1.25f32, -0.42f32, 7),
        (0.95, -0.38, 7),
        (0.62, -0.3, 6),
        (0.3, -0.24, 5),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = |side: usize| sided(v(up.cos(), up.sin(), 0.0), side);
        for k in 0..n {
            let z = from + 0.14 * k as f32 + 0.05 * (row % 2) as f32;
            for side in 0..2 {
                let at = |t: f32| v(0.0, 0.97, z) + dir(side) * t;
                if g.b.distance(at(0.0) * g.s) >= 0.0 {
                    continue;
                }
                let (mut lo, mut hi) = (0.0, 0.5);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if g.b.distance(at(mid) * g.s) < 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let bone = if z < -0.4 {
                    name("front", "scapula", side)
                } else if z > 0.36 {
                    name("hind", "upper", side)
                } else if z < 0.0 {
                    "body".to_string()
                } else {
                    "loin".to_string()
                };
                g.el(
                    &bone,
                    at((lo - r - 0.002).max(0.0)),
                    Vec3::splat(r),
                    spot,
                    0.004,
                );
            }
        }
    }
}

/// A red deer stag's antlers, `size` 0..1: from the pedicles a beam rises back and out with
/// a brow tine, a trez tine and a crown of three at the top.
fn antlers(g: &mut Grow, f: &Frame, size: f32) {
    let bone = Paint::hard([0.17, 0.11, 0.06]);
    let tip = Paint::hard([0.62, 0.56, 0.46]);
    let head = f.neck[2];
    for side in 0..2 {
        let at = |x: f32, y: f32, z: f32| {
            sided(
                head + v(0.045 + x * size, 0.07 + y * size, -0.03 + z * size),
                side,
            )
        };
        let beam = [
            at(0.0, 0.0, 0.0),
            at(0.07, 0.16, 0.05),
            at(0.15, 0.36, 0.13),
            at(0.2, 0.56, 0.16),
            at(0.21, 0.7, 0.11),
        ];
        // The coronet (burr) at the base.
        g.el("head", beam[0], Vec3::splat(0.028), bone, 0.01);
        let radii = [0.022, 0.019, 0.016, 0.013, 0.01];
        for k in 0..4 {
            g.cap(
                "head",
                beam[k],
                beam[k + 1],
                radii[k] * (0.6 + 0.4 * size),
                radii[k + 1] * (0.6 + 0.4 * size),
                bone,
                0.008,
            );
        }
        let tine = |g: &mut Grow, from: Vec3, d: Vec3| {
            let end = from + d * size;
            let mid = from + d * size * 0.7;
            g.cap("head", from, mid, 0.012, 0.008, bone, 0.006)
                .cap("head", mid, end, 0.008, 0.004, tip, 0.003);
        };
        let x = if side == 0 { -1.0 } else { 1.0 };
        // Brow tine forward over the face, trez forward from the middle, the crown.
        tine(g, beam[0].lerp(beam[1], 0.25), v(0.02 * x, 0.06, -0.2));
        tine(g, beam[1].lerp(beam[2], 0.2), v(0.01 * x, 0.07, -0.16));
        tine(g, beam[2].lerp(beam[3], 0.35), v(0.0, 0.08, -0.15));
        tine(g, beam[3], v(0.04 * x, 0.12, -0.08));
        tine(g, beam[3].lerp(beam[4], 0.5), v(-0.03 * x, 0.11, 0.02));
        tine(g, beam[4], v(0.02 * x, 0.08, -0.04));
    }
}

fn wolf_body(g: &mut Grow, f: &Frame, c: &Coat) {
    // A narrow, deep chest, a dark saddle over the shoulders and back, a tucked waist.
    g.el("body", v(0.0, 0.6, -0.2), v(0.12, 0.17, 0.24), c.side, 0.05)
        .el(
            "body",
            v(0.0, 0.69, -0.22),
            v(0.09, 0.08, 0.2),
            c.back,
            0.04,
        )
        .el(
            "body",
            v(0.0, 0.57, -0.37),
            v(0.085, 0.12, 0.085),
            c.under,
            0.04,
        )
        .el(
            "body",
            v(0.0, 0.47, -0.2),
            v(0.08, 0.06, 0.15),
            c.under,
            0.04,
        )
        .el(
            "loin",
            v(0.0, 0.645, 0.08),
            v(0.092, 0.1, 0.17),
            c.side,
            0.05,
        )
        .el("loin", v(0.0, 0.7, 0.08), v(0.07, 0.06, 0.15), c.back, 0.04)
        .el(
            "loin",
            v(0.0, 0.565, 0.03),
            v(0.075, 0.065, 0.13),
            c.under,
            0.04,
        )
        .el("loin", v(0.0, 0.65, 0.28), v(0.1, 0.1, 0.11), c.side, 0.04);
    // A thick ruff of fur around the neck, paler at the throat and cheeks.
    let n = f.neck;
    g.cap(
        "neck_1",
        v(0.0, 0.66, -0.37),
        n[1] + v(0.0, 0.03, -0.02),
        0.11,
        0.078,
        c.side,
        0.04,
    )
    .el(
        "neck_1",
        v(0.0, 0.72, -0.43),
        v(0.11, 0.12, 0.1),
        c.side,
        0.04,
    )
    .el(
        "neck_1",
        v(0.0, 0.77, -0.44),
        v(0.07, 0.07, 0.08),
        c.back,
        0.03,
    )
    .el(
        "neck_1",
        v(0.0, 0.64, -0.47),
        v(0.075, 0.09, 0.06),
        c.under,
        0.03,
    )
    .cap(
        "neck_2",
        n[1] + v(0.0, 0.03, -0.02),
        v(0.0, 0.88, -0.58),
        0.078,
        0.07,
        c.side,
        0.03,
    );
    // Head: a broad skull, cheek ruffs, a long tapering muzzle with a pale mask.
    g.el(
        "head",
        v(0.0, 0.895, -0.6),
        v(0.072, 0.058, 0.085),
        c.face,
        0.02,
    )
    .el(
        "head",
        v(0.0, 0.875, -0.585),
        v(0.09, 0.05, 0.06),
        c.face,
        0.02,
    )
    .el(
        "head",
        v(0.0, 0.9, -0.665),
        v(0.045, 0.032, 0.035),
        c.face,
        0.012,
    );
    for side in 0..2 {
        g.el(
            "head",
            sided(v(0.06, 0.85, -0.6), side),
            v(0.045, 0.05, 0.05),
            c.under,
            0.015,
        );
    }
    g.cap(
        "head",
        v(0.0, 0.866, -0.68),
        v(0.0, 0.85, -0.8),
        0.04,
        0.028,
        c.muzzle,
        0.012,
    )
    .el(
        "head",
        v(0.0, 0.843, -0.73),
        v(0.042, 0.03, 0.06),
        c.muzzle,
        0.008,
    )
    .el(
        "head",
        v(0.0, 0.858, -0.828),
        v(0.023, 0.019, 0.017),
        BLACK,
        0.006,
    )
    .cap(
        "jaw",
        f.jaw + v(0.0, 0.01, -0.04),
        v(0.0, 0.815, -0.79),
        0.03,
        0.02,
        c.muzzle,
        0.006,
    );
    for side in 0..2 {
        // The dark lip line.
        g.cap(
            "head",
            sided(v(0.034, 0.83, -0.66), side),
            sided(v(0.024, 0.826, -0.79), side),
            0.004,
            0.0035,
            BLACK,
            0.003,
        );
    }
    eyes(g, v(0.04, 0.895, -0.676), 0.0125, [0.42, 0.24, 0.04]);
    // Erect triangular ears, dark-rimmed.
    for side in 0..2 {
        let x = if side == 0 { -1.0 } else { 1.0 };
        let ear = format!("ear_{}", SIDE[side]);
        let q = Quat::from_rotation_z(-0.28 * x) * Quat::from_rotation_x(-0.12);
        g.el_rot(
            &ear,
            sided(v(0.058, 0.985, -0.58), side),
            v(0.032, 0.055, 0.012),
            q,
            c.ear,
            0.01,
        )
        .el_rot(
            &ear,
            sided(v(0.057, 0.98, -0.585), side),
            v(0.022, 0.04, 0.006),
            q,
            Paint::fur([0.48, 0.42, 0.33], 0.7),
            0.004,
        );
    }
    // The bushy tail, hanging straight, black at the tip.
    let tip = Paint::fur([0.02, 0.018, 0.016], 1.0);
    for (k, (ra, rb)) in [
        (0.045, 0.055),
        (0.055, 0.055),
        (0.055, 0.048),
        (0.048, 0.035),
    ]
    .into_iter()
    .enumerate()
    {
        g.cap(
            &format!("tail_{}", k + 1),
            f.tail[k],
            f.tail[k + 1],
            ra,
            rb,
            if k == 3 { tip } else { c.tail },
            0.02,
        );
    }
    g.el(
        "tail_1",
        f.tail[0] + v(0.0, 0.01, 0.02),
        v(0.035, 0.03, 0.035),
        c.back,
        0.01,
    );
}

fn boar_body(g: &mut Grow, f: &Frame, c: &Coat, look: Look) {
    // Narrow and deep: heavy forequarters under high withers, narrowing to a small ham.
    g.el(
        "body",
        v(0.0, 0.57, -0.24),
        v(0.185, 0.235, 0.34),
        c.side,
        0.08,
    )
    .el("body", v(0.0, 0.7, -0.36), v(0.14, 0.15, 0.2), c.back, 0.06)
    .el(
        "body",
        v(0.0, 0.43, -0.2),
        v(0.12, 0.09, 0.28),
        c.under,
        0.06,
    )
    .el(
        "loin",
        v(0.0, 0.59, 0.12),
        v(0.175, 0.18, 0.27),
        c.side,
        0.07,
    )
    .el(
        "loin",
        v(0.0, 0.47, 0.06),
        v(0.11, 0.08, 0.22),
        c.under,
        0.05,
    )
    .el("loin", v(0.0, 0.6, 0.36), v(0.16, 0.14, 0.17), c.side, 0.07)
    .el(
        "loin",
        v(0.0, 0.66, 0.05),
        v(0.155, 0.11, 0.22),
        c.side,
        0.07,
    )
    // A broad, deep chest between the forelegs, so the elbows lie against it, and the
    // massive forequarters the shoulder blades sink into.
    .el(
        "body",
        v(0.0, 0.43, -0.37),
        v(0.135, 0.11, 0.17),
        c.side,
        0.07,
    )
    .el(
        "body",
        v(0.0, 0.6, -0.47),
        v(0.185, 0.18, 0.17),
        c.side,
        0.08,
    );
    // Hams that slope down from the croup, narrower than the shoulders, and the fold of
    // skin from the flank to the stifle.
    for side in 0..2 {
        let hip = name("hind", "upper", side);
        g.el(
            &hip,
            sided(v(0.08, 0.53, 0.45), side),
            v(0.065, 0.12, 0.12),
            c.side,
            0.06,
        )
        .el(
            &hip,
            sided(v(0.09, 0.47, 0.3), side),
            v(0.05, 0.06, 0.08),
            c.side,
            0.06,
        );
    }
    // The mane: a blade of long, coarse bristles standing up along the spine from the nape,
    // tallest over the withers and falling away to the rump, so the back slopes down from a
    // hump. (A piglet's is short.)
    let mane = Paint {
        fur: if look.stripes { 1.1 } else { 2.6 },
        ..c.back
    };
    for (z, y) in [
        (-0.7, 0.77),
        (-0.58, 0.85),
        (-0.45, 0.885),
        (-0.3, 0.865),
        (-0.15, 0.82),
        (0.0, 0.78),
        (0.15, 0.75),
        (0.3, 0.715),
        (0.42, 0.68),
    ] {
        let bone = if z < -0.55 {
            "neck_1"
        } else if z < 0.0 {
            "body"
        } else {
            "loin"
        };
        g.el(bone, v(0.0, y - 0.01, z), v(0.05, 0.07, 0.15), mane, 0.07);
    }
    // A thick neck straight into the wedge of the head.
    // (Deeper than wide, so the forequarters don't bulge out at the sides.)
    g.el(
        "neck_1",
        v(0.0, 0.61, -0.55),
        v(0.15, 0.2, 0.15),
        c.side,
        0.08,
    )
    .el(
        "neck_2",
        v(0.0, 0.615, -0.68),
        v(0.135, 0.165, 0.1),
        c.face,
        0.06,
    )
    .cap(
        "neck_2",
        v(0.0, 0.74, -0.5),
        v(0.0, 0.76, -0.68),
        0.05,
        0.04,
        c.back,
        0.03,
    );
    // A big wedge of a head, about a third of the body's length, the brow running straight
    // down to the snout.
    g.cap(
        "head",
        v(0.0, 0.65, -0.72),
        v(0.0, 0.45, -1.08),
        0.15,
        0.06,
        c.face,
        0.04,
    )
    .el(
        "head",
        v(0.0, 0.61, -0.78),
        v(0.135, 0.13, 0.12),
        c.face,
        0.03,
    );
    for side in 0..2 {
        g.el(
            "head",
            sided(v(0.07, 0.56, -0.83), side),
            v(0.06, 0.075, 0.1),
            c.face,
            0.04,
        );
    }
    // The snout disc, bare and moist, with two nostrils.
    let disc = Paint::skin([0.075, 0.055, 0.05]);
    g.b.part(
        "head",
        Shape::cylinder(
            v(0.0, 0.415, -1.11) * g.s,
            v(0.0, 0.3, -1.0).normalize(),
            0.055 * g.s,
            0.02 * g.s,
            0.012 * g.s,
        ),
        disc,
        0.012 * g.s,
    );
    for side in 0..2 {
        g.b.cut(
            "head",
            Shape::ellipsoid(
                sided(v(0.02, 0.425, -1.13), side) * g.s,
                v(0.009, 0.012, 0.01) * g.s,
            ),
            BLACK,
            0.004 * g.s,
        );
    }
    // The lower jaw: deep under the cheeks, running straight to the chin under the snout,
    // so the head is a wedge from the side rather than a tube.
    g.cap(
        "jaw",
        v(0.0, 0.5, -0.8),
        v(0.0, 0.405, -1.02),
        0.085,
        0.04,
        c.face,
        0.03,
    )
    .cap(
        "jaw",
        v(0.0, 0.455, -0.9),
        v(0.0, 0.395, -1.03),
        0.045,
        0.032,
        c.under,
        0.012,
    );
    // Tusks curving up and out of the lower jaw.
    let ivory = Paint::hard([0.62, 0.58, 0.48]);
    for side in 0..2 {
        g.cap(
            "jaw",
            sided(v(0.038, 0.42, -0.98), side),
            sided(v(0.058, 0.46, -0.99), side),
            0.012,
            0.009,
            ivory,
            0.004,
        )
        .cap(
            "jaw",
            sided(v(0.058, 0.46, -0.99), side),
            sided(v(0.07, 0.505, -0.965), side),
            0.009,
            0.004,
            ivory,
            0.003,
        );
    }
    eyes(g, v(0.075, 0.625, -0.86), 0.01, [0.04, 0.025, 0.015]);
    // Small pointed ears, fringed.
    for side in 0..2 {
        let x = if side == 0 { -1.0 } else { 1.0 };
        let ear = format!("ear_{}", SIDE[side]);
        let q = Quat::from_rotation_z(-0.45 * x) * Quat::from_rotation_x(0.2);
        g.el_rot(
            &ear,
            sided(v(0.09, 0.815, -0.75), side),
            v(0.035, 0.065, 0.014),
            q,
            c.ear,
            0.01,
        );
    }
    // A thin tail with a tassel.
    for k in 0..4 {
        g.cap(
            &format!("tail_{}", k + 1),
            f.tail[k],
            f.tail[k + 1],
            0.018 - 0.002 * k as f32,
            0.016 - 0.002 * k as f32,
            c.tail,
            0.006,
        );
    }
    g.el(
        "tail_4",
        f.tail[4] + v(0.0, -0.01, 0.0),
        v(0.014, 0.04, 0.012),
        Paint { fur: 2.2, ..c.back },
        0.006,
    );
}

fn rabbit_body(g: &mut Grow, f: &Frame, c: &Coat) {
    // An egg of a body, the big round haunches behind, a white belly.
    g.el(
        "body",
        v(0.0, 0.13, -0.06),
        v(0.05, 0.058, 0.075),
        c.side,
        0.02,
    )
    .el(
        "body",
        v(0.0, 0.16, -0.06),
        v(0.035, 0.03, 0.06),
        c.back,
        0.015,
    )
    .el(
        "body",
        v(0.0, 0.1, -0.08),
        v(0.035, 0.04, 0.045),
        c.under,
        0.015,
    )
    .el(
        "loin",
        v(0.0, 0.14, 0.07),
        v(0.062, 0.07, 0.085),
        c.side,
        0.02,
    )
    .el(
        "loin",
        v(0.0, 0.18, 0.06),
        v(0.045, 0.035, 0.07),
        c.back,
        0.015,
    )
    .el(
        "loin",
        v(0.0, 0.09, 0.02),
        v(0.04, 0.04, 0.07),
        c.under,
        0.015,
    );
    let n = f.neck;
    let nape = Paint::fur([0.32, 0.15, 0.06], 0.7);
    g.cap(
        "neck_1",
        v(0.0, 0.145, -0.1),
        n[1] + v(0.0, 0.01, 0.0),
        0.04,
        0.034,
        c.side,
        0.015,
    )
    .el(
        "neck_1",
        v(0.0, 0.175, -0.13),
        v(0.025, 0.02, 0.025),
        nape,
        0.008,
    )
    .cap("neck_2", n[1], n[2], 0.034, 0.032, c.face, 0.01);
    // A round head, big eyes set on the sides, a twitching nose over a split lip.
    g.el(
        "head",
        v(0.0, 0.203, -0.172),
        v(0.034, 0.034, 0.042),
        c.face,
        0.01,
    )
    .el(
        "head",
        v(0.0, 0.19, -0.205),
        v(0.022, 0.022, 0.022),
        c.muzzle,
        0.007,
    )
    .el(
        "head",
        v(0.0, 0.195, -0.224),
        v(0.0065, 0.005, 0.004),
        Paint::skin([0.24, 0.12, 0.1]),
        0.002,
    );
    for side in 0..2 {
        g.el(
            "head",
            sided(v(0.01, 0.183, -0.214), side),
            v(0.011, 0.01, 0.009),
            c.under,
            0.003,
        );
    }
    g.cap(
        "jaw",
        f.jaw,
        v(0.0, 0.172, -0.205),
        0.012,
        0.009,
        c.under,
        0.004,
    );
    eyes(g, v(0.028, 0.208, -0.183), 0.0085, [0.05, 0.028, 0.015]);
    // Long upright ears, dark-tipped.
    let tip = Paint::fur([0.05, 0.04, 0.03], 0.5);
    for side in 0..2 {
        let x = if side == 0 { -1.0 } else { 1.0 };
        let ear = format!("ear_{}", SIDE[side]);
        let q = Quat::from_rotation_z(-0.12 * x) * Quat::from_rotation_x(0.3);
        g.el_rot(
            &ear,
            sided(v(0.021, 0.285, -0.14), side),
            v(0.016, 0.058, 0.006),
            q,
            c.ear,
            0.004,
        )
        .el_rot(
            &ear,
            sided(v(0.021, 0.284, -0.141), side),
            v(0.011, 0.045, 0.003),
            q,
            Paint::fur([0.36, 0.25, 0.2], 0.3),
            0.0015,
        )
        .el_rot(
            &ear,
            sided(v(0.023, 0.335, -0.125), side),
            v(0.012, 0.012, 0.005),
            q,
            tip,
            0.002,
        );
    }
    // The cotton scut.
    g.el(
        "tail_2",
        (f.tail[1] + f.tail[2]) * 0.5,
        v(0.02, 0.022, 0.02),
        c.tail,
        0.006,
    );
}

/// A piglet's humbug stripes: pale bands along each flank, from the shoulder to the ham,
/// painted onto the coat (each band is a chain of capsules lying just inside the body's
/// surface, so it colours the skin without raising it).
fn piglet_stripes(g: &mut Grow) {
    let stripe = Paint::fur([0.36, 0.27, 0.15], 0.85);
    let r = 0.016;
    for y in [0.735, 0.655, 0.575, 0.5] {
        for side in 0..2 {
            // Where the flank is at this height, by bisection outward from the middle.
            let surface = |z: f32| -> Option<f32> {
                let d = |x: f32| g.b.distance(sided(v(x, y, z), side) * g.s);
                if d(0.0) >= 0.0 {
                    return None;
                }
                let (mut lo, mut hi) = (0.0, 0.4);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if d(mid) < 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                Some(lo)
            };
            let points: Vec<Vec3> = (0..19)
                .filter_map(|k| {
                    let z = -0.56 + 0.06 * k as f32;
                    surface(z).map(|x| sided(v((x - r - 0.002).max(0.0), y, z), side))
                })
                .collect();
            for pair in points.windows(2) {
                // Over the shoulder blade and the ham the band moves with the leg.
                let z = 0.5 * (pair[0].z + pair[1].z);
                let bone = if z < -0.4 {
                    name("front", "scapula", side)
                } else if z > 0.36 {
                    name("hind", "upper", side)
                } else if z < 0.0 {
                    "body".to_string()
                } else {
                    "loin".to_string()
                };
                g.cap(&bone, pair[0], pair[1], r, r, stripe, 0.004);
            }
        }
    }
}

/// The animal's body (scale 1 lengths grown at `scale`).
pub fn body(kind: Wild, scale: f32, look: Look) -> Body {
    let f = frame(kind);
    let c = coat(kind, look);
    let mut b = Body::default();
    let mut g = Grow {
        b: &mut b,
        s: scale,
    };
    match kind {
        Wild::Deer => deer_body(&mut g, &f, &c, look),
        Wild::Wolf => wolf_body(&mut g, &f, &c),
        Wild::Boar => boar_body(&mut g, &f, &c, look),
        Wild::Rabbit => rabbit_body(&mut g, &f, &c),
    }
    legs(&mut g, kind, &f, &c);
    // (Over the shoulders and hams too, so after the legs.)
    if kind == Wild::Boar && look.stripes {
        piglet_stripes(&mut g);
    }
    if kind == Wild::Deer && look.spots {
        fawn_spots(&mut g);
    }
    b
}

pub fn mesh(kind: Wild, scale: f32, look: Look) -> SkinnedMesh {
    let w = kind.withers();
    // Finer for the deer, whose antler tines are thin.
    let cell = if kind == Wild::Deer { 0.0075 } else { 0.0095 };
    body(kind, scale, look.quantized()).build(
        &skeleton(kind, scale),
        cell * w * scale,
        0.025 * w * scale,
    )
}

/// The coat: fur shells (or, with a fur drawer, strands) over the hide.
pub fn material(kind: Wild, scale: f32) -> SkinMaterial {
    let w = kind.withers() * scale;
    let (length, density, droop, comb, tip) = match kind {
        // A short, sleek summer coat.
        Wild::Deer => (0.011, 900.0, 0.35, 0.9, [1.1, 1.06, 1.0]),
        // Long guard hairs over the underfur, banded pale at the tips (agouti).
        Wild::Wolf => (0.034, 520.0, 0.4, 0.75, [1.45, 1.4, 1.3]),
        // Coarse, sparse bristles.
        Wild::Boar => (0.032, 340.0, 0.25, 0.6, [1.3, 1.25, 1.2]),
        // Dense, soft and short.
        Wild::Rabbit => (0.012, 2400.0, 0.3, 0.7, [1.35, 1.3, 1.2]),
    };
    SkinMaterial {
        skin_roughness: 0.66,
        subsurface: 0.22,
        sheen: 0.55,
        veins: 0.0,
        wrinkles: 0.0,
        fur_length: length * scale.sqrt().min(1.0) * if w < 0.1 { 0.8 } else { 1.0 },
        fur_density: density / scale,
        fur_shells: 18,
        fur_lod: 7.0 * (w / 0.56).sqrt(),
        fur_droop: droop,
        fur_comb: comb,
        fur_root: 0.42,
        fur_tip: tip,
        ..Default::default()
    }
}

/// Legs in the order front left, front right, hind left, hind right.
pub fn gait(kind: Wild, scale: f32) -> LeggedGait {
    let f = frame(kind);
    let s = scale;
    let speed = s.sqrt();
    let rate = 1.0 / speed;
    let mut legs = Vec::new();
    for prefix in ["front", "hind"] {
        for side in 0..2 {
            let (parts, style): (&[&str], _) = if prefix == "front" {
                (
                    &["scapula", "upper", "lower", "cannon", "foot"],
                    LegStyle::Scapular,
                )
            } else {
                (&["upper", "lower", "cannon", "foot"], LegStyle::Hoofed)
            };
            let (hip, foot) = (f.hind[0], f.hind[3]);
            legs.push(GaitLeg {
                release: RIG.release[legs.len()].into(),
                bones: parts.iter().map(|p| name(prefix, p, side)).collect(),
                style,
                phase: 0.0,
                pole: if prefix == "front" {
                    Vec3::Z
                } else {
                    Vec3::NEG_Z
                },
                kind: if kind.hoofed() {
                    FootKind::Hoof
                } else {
                    FootKind::Paw
                },
                foot_size: match kind {
                    Wild::Deer => [0.055 * s, 0.075 * s],
                    Wild::Wolf => [0.08 * s, 0.1 * s],
                    Wild::Boar => [0.06 * s, 0.07 * s],
                    Wild::Rabbit => [0.02 * s, if prefix == "front" { 0.025 } else { 0.11 } * s],
                },
                sole: f.sole * s,
                stance_offset: if prefix == "front" || kind == Wild::Rabbit {
                    Vec3::ZERO
                } else {
                    v(0.0, 0.0, 0.5 * (hip.z - foot.z) * s)
                },
            });
        }
    }
    let w = kind.withers() * s;
    let stage = |name: &str, min_speed: f32, offsets: [f32; 4], duty: f32, bob: f32, pitch: f32| {
        GaitStage {
            name: name.into(),
            min_speed: min_speed * speed,
            offsets: offsets.to_vec(),
            duty,
            bob: bob * w,
            pitch_deg: pitch,
        }
    };
    // Bob in withers heights.
    let (stages, base, per, step, full): (Vec<GaitStage>, f32, f32, f32, f32) = match kind {
        Wild::Deer => (
            vec![
                stage("walk", 0.0, [0.25, 0.75, 0.0, 0.5], 0.64, 0.012, 1.2),
                stage("trot", 1.6, [0.0, 0.5, 0.5, 0.0], 0.42, 0.02, 1.0),
                stage("canter", 4.2, [0.58, 0.32, 0.32, 0.0], 0.34, 0.03, 6.0),
                // The transverse gallop of a running deer, with long flight.
                stage("gallop", 7.0, [0.55, 0.45, 0.05, 0.0], 0.24, 0.04, 8.0),
            ],
            0.95,
            0.22,
            0.1,
            14.0,
        ),
        Wild::Wolf => (
            vec![
                stage("walk", 0.0, [0.25, 0.75, 0.0, 0.5], 0.62, 0.01, 1.2),
                // The wolf's tireless travelling trot.
                stage("trot", 1.7, [0.0, 0.5, 0.5, 0.0], 0.4, 0.016, 1.0),
                stage("canter", 5.0, [0.58, 0.32, 0.32, 0.0], 0.34, 0.025, 6.0),
                // Rotary gallop, the spine flexing.
                stage("gallop", 7.5, [0.6, 0.48, 0.0, 0.1], 0.25, 0.03, 8.0),
            ],
            1.15,
            0.32,
            0.06,
            15.0,
        ),
        Wild::Boar => (
            vec![
                stage("walk", 0.0, [0.25, 0.75, 0.0, 0.5], 0.66, 0.008, 1.0),
                stage("trot", 1.2, [0.0, 0.5, 0.5, 0.0], 0.45, 0.014, 1.0),
                stage("gallop", 3.6, [0.58, 0.46, 0.05, 0.0], 0.3, 0.025, 5.0),
            ],
            1.4,
            0.45,
            0.04,
            11.0,
        ),
        Wild::Rabbit => (
            vec![
                // A slow hop: the forefeet one after the other, then the hind feet together.
                stage("hop", 0.0, [0.0, 0.1, 0.5, 0.52], 0.5, 0.06, 6.0),
                // The bound: long flights, the hind feet landing ahead of the fore.
                stage("bound", 1.4, [0.0, 0.08, 0.55, 0.56], 0.3, 0.1, 12.0),
            ],
            2.2,
            0.9,
            0.035,
            12.0,
        ),
    };
    LeggedGait {
        body: "body".into(),
        legs,
        stages,
        base_frequency: base * rate,
        frequency_per_speed: per * rate / speed,
        step_height: step * s,
        step_height_per_speed: 0.012 * s / speed,
        body_follow_ground: 0.8,
        body_tilt: 0.8,
        full_speed: full * speed,
        turn_radius: 0.3 * w,
        mass_kg: kind.mass_kg() * s * s * s,
        sway: vec![
            GaitSway {
                bone: "loin".into(),
                axis: Vec3::X,
                amplitude_deg: match kind {
                    Wild::Rabbit => 16.0,
                    Wild::Wolf => 9.0,
                    _ => 5.0,
                },
                frequency: 1.0,
                phase: 0.15,
            },
            GaitSway {
                bone: "neck_1".into(),
                axis: Vec3::X,
                amplitude_deg: if kind == Wild::Deer { 5.0 } else { 3.0 },
                frequency: 2.0,
                phase: 0.0,
            },
            GaitSway {
                bone: "head".into(),
                axis: Vec3::X,
                amplitude_deg: -3.0,
                frequency: 2.0,
                phase: 0.1,
            },
        ],
        ..Default::default()
    }
}

/// The tail and the two ears, on springs.
pub fn springs(kind: Wild, scale: f32) -> Vec<SpringChainDef> {
    let w = kind.withers() * scale;
    let (tail_stiff, tail_gravity, ear_stiff, ear_gravity) = match kind {
        Wild::Wolf => (0.12, 0.3, 0.55, 0.05),
        Wild::Deer => (0.45, 0.15, 0.5, 0.05),
        Wild::Boar => (0.2, 0.4, 0.45, 0.08),
        Wild::Rabbit => (0.7, 0.05, 0.4, 0.06),
    };
    let tail = SpringChainDef {
        bones: (1..=4).map(|k| format!("tail_{k}")).collect(),
        stiffness: tail_stiff,
        damping: 0.86,
        gravity: tail_gravity,
        wind: 0.3,
        tip_length: 0.08 * w,
    };
    let ears = SIDE.iter().map(|side| SpringChainDef {
        bones: vec![format!("ear_{side}")],
        stiffness: ear_stiff,
        damping: 0.8,
        gravity: ear_gravity,
        wind: 0.3,
        tip_length: 0.1 * w,
    });
    std::iter::once(tail).chain(ears).collect()
}

/// Standing about: breathing, looking around, the ears turning on their own, the tail
/// flicking; a deer chews the cud, a boar sniffs, a rabbit's nose twitches. Rotations only,
/// so every size shares it.
fn idle(kind: Wild, s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn(&kind.idle_name(), 8.0, 241, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let look = cycle_sin(ph);
        let breathe = cycle_sin(ph * 10.0);
        let ear_l = cycle_sin(ph * 3.0 + 0.2);
        let ear_r = cycle_sin(ph * 2.0);
        p.rot("body", 0.4 * breathe, 0.0, 0.0);
        match kind {
            Wild::Deer => {
                let chew = cycle_sin(ph * 12.0);
                // An ear flick every few seconds.
                let flick = super::bump(ph, 0.3, 0.012) - super::bump(ph, 0.7, 0.012);
                p.rot("neck_1", 0.0, 8.0 * look, 0.0)
                    .rot("neck_2", 0.0, 6.0 * look, 0.0)
                    .rot("head", 2.0 * cycle_sin(ph * 2.0), 10.0 * look, -3.0 * look)
                    .rot("jaw", 3.0 + 2.0 * chew, 4.0 * chew, 0.0)
                    .rot("ear_l", -12.0 * ear_l, 18.0 * ear_l, 25.0 * flick)
                    .rot("ear_r", -12.0 * ear_r, -18.0 * ear_r, 0.0)
                    .rot(
                        "tail_1",
                        6.0 + 20.0 * super::bump(ph, 0.55, 0.015),
                        0.0,
                        0.0,
                    );
            }
            Wild::Wolf => {
                let pant = cycle_sin(ph * 16.0);
                p.rot("neck_1", -2.0, 7.0 * look, 0.0)
                    .rot("neck_2", 0.0, 6.0 * look, 2.0 * look)
                    .rot("head", 3.0 * cycle_sin(ph * 2.0), 12.0 * look, -4.0 * look)
                    .rot("jaw", 4.0 + 3.0 * pant, 0.0, 0.0)
                    .rot("ear_l", -6.0 * ear_l, 14.0 * ear_l, 0.0)
                    .rot("ear_r", -6.0 * ear_r, -14.0 * ear_r, 0.0)
                    .rot("tail_1", -4.0, 6.0 * cycle_sin(ph * 2.0), 0.0);
            }
            Wild::Boar => {
                let sniff = cycle_sin(ph * 24.0);
                p.rot("neck_1", -3.0, 6.0 * look, 0.0)
                    .rot("head", -4.0 + 2.0 * sniff, 8.0 * look, 0.0)
                    .rot("jaw", 2.0 + 1.0 * sniff, 0.0, 0.0)
                    .rot("ear_l", -8.0 * ear_l, 10.0 * ear_l, 0.0)
                    .rot("ear_r", -8.0 * ear_r, -10.0 * ear_r, 0.0)
                    .rot("tail_1", 0.0, 25.0 * cycle_sin(ph * 6.0), 0.0)
                    .rot("tail_2", 0.0, 20.0 * cycle_sin(ph * 6.0 - 0.1), 0.0);
            }
            Wild::Rabbit => {
                let twitch = cycle_sin(ph * 40.0);
                p.rot("neck_1", 0.0, 10.0 * look, 0.0)
                    .rot("head", 1.5 * twitch, 12.0 * look, 0.0)
                    .rot("jaw", 3.0 + 2.0 * twitch.max(0.0), 0.0, 0.0)
                    .rot("ear_l", -10.0 * ear_l, 12.0 * ear_l, 0.0)
                    .rot("ear_r", -10.0 * ear_r, -12.0 * ear_r, 0.0);
            }
        }
        p.take()
    })
}

/// The kind's idle clip at full size, its graph's only clip.
pub fn clips(kind: Wild, s: &Skeleton) -> Vec<AnimationClip> {
    vec![idle(kind, s)]
}

pub fn graph(kind: Wild) -> AnimGraph {
    AnimGraph {
        name: kind.graph_name(),
        root: NodeDef::clip(&kind.idle_name()),
    }
}

// ------------------------------------------------------------------------------------------
// Actions.

/// What the actions move (the same bones for every kind).
pub(crate) const RIG: Rig = Rig {
    legs: [
        &[
            "front_scapula_l",
            "front_upper_l",
            "front_lower_l",
            "front_cannon_l",
            "front_foot_l",
        ],
        &[
            "front_scapula_r",
            "front_upper_r",
            "front_lower_r",
            "front_cannon_r",
            "front_foot_r",
        ],
        &[
            "hind_upper_l",
            "hind_lower_l",
            "hind_cannon_l",
            "hind_foot_l",
        ],
        &[
            "hind_upper_r",
            "hind_lower_r",
            "hind_cannon_r",
            "hind_foot_r",
        ],
    ],
    release: [
        "release.front_l",
        "release.front_r",
        "release.hind_l",
        "release.hind_r",
    ],
    neck: &["neck_1", "neck_2", "head"],
    jaw: Some("jaw"),
    tail: &["tail_1", "tail_2", "tail_3", "tail_4"],
    loin: Some("loin"),
};

/// The nose tip in the head bone's frame at full size, and the head's rest height.
pub fn muzzle(kind: Wild) -> (Vec3, f32) {
    let f = frame(kind);
    (f.muzzle, f.neck[2].y)
}

/// The actions every size of `kind` has (base names; [`sized_name`] adds the size):
/// `{kind}_graze`, `_lie`, `_alert`, `_flinch`, `_fall`, and `deer_rear` / `wolf_howl`.
pub fn action_names(kind: Wild) -> Vec<String> {
    let mut out: Vec<String> = ["graze", "lie", "alert", "flinch", "fall"]
        .iter()
        .map(|a| format!("{}_{a}", kind.name()))
        .collect();
    match kind {
        Wild::Deer => out.push("deer_rear".into()),
        Wild::Wolf => out.push("wolf_howl".into()),
        _ => {}
    }
    out
}

/// When each holding action reaches its held pose, s (held there while its parameter is
/// up, then it plays on to its end).
pub fn hold_at(kind: Wild, action: &str) -> Option<f32> {
    let _ = kind;
    match action {
        "graze" => Some(GRAZE_HOLD),
        "lie" => Some(LIE_HOLD),
        "alert" => Some(ALERT_HOLD),
        "fall" => Some(FALL_HOLD),
        _ => None,
    }
}

pub const GRAZE_HOLD: f32 = 2.6;
pub const LIE_HOLD: f32 = 2.4;
pub const ALERT_HOLD: f32 = 1.2;
pub const FALL_HOLD: f32 = 1.4;

/// A leg off the gait: its cannon's top at `joint` (scale-1 model space), the cannon along
/// `cannon`, the foot bent back `end` degrees, the joint above bending toward `pole`.
fn free(joint: Vec3, cannon: Vec3, end: f32, pole: Vec3) -> Leg {
    Leg::Free {
        joint,
        cannon,
        end,
        pole,
    }
}

fn mirror(l: Leg) -> Leg {
    match l {
        Leg::Free {
            joint,
            cannon,
            end,
            pole,
        } => Leg::Free {
            joint: v(-joint.x, joint.y, joint.z),
            cannon: v(-cannon.x, cannon.y, cannon.z),
            end,
            pole: v(-pole.x, pole.y, pole.z),
        },
        l => l,
    }
}

fn pair(l: Leg) -> [Leg; 2] {
    [mirror(l), l]
}

fn len(a: Vec3, b: Vec3) -> f32 {
    (b - a).length()
}

/// Where a folded leg's cannon top goes: on the ground (at `y`), as far from `from` (the
/// upper joint, already lowered) along `dir` as legs of `reach` allow.
fn fold_to(from: Vec3, reach: f32, y: f32, dir: Vec3) -> Vec3 {
    let dy = (from.y - y).max(0.0);
    let flat = (reach * reach - dy * dy).max(0.0).sqrt();
    v(from.x, y, from.z) + dir.normalize() * flat
}

/// The legs of an animal lying on its chest with its body `drop` m lower: ungulates fold
/// the forelegs under the chest (the knee forward, the cannon tucked back), a wolf or a rabbit
/// lays them out ahead; the hind legs fold at the sides, hocks on the ground.
fn lie_legs(kind: Wild, f: &Frame, drop: f32) -> [Leg; 4] {
    let w = kind.withers();
    let [_, sh, el, ca, ft] = f.front;
    let [hip, st, ho, hf] = f.hind;
    let down = v(0.0, drop, 0.0);
    let on_ground = 0.035 * w;
    let front = if kind.hoofed() {
        let reach = 0.62 * (len(sh, el) + len(el, ca));
        let joint = fold_to(sh - down, reach, on_ground, v(0.0, 0.0, -1.0));
        free(
            joint + v(-0.01 * w, 0.0, 0.0),
            v(0.0, -0.02, 1.0),
            60.0,
            v(0.0, 0.0, 1.0),
        )
    } else {
        let reach = 0.92 * (len(sh, el) + len(el, ca));
        let joint = fold_to(sh - down, reach, on_ground, v(0.0, 0.0, -1.0));
        let toes = len(ca, ft).max(1e-3);
        let _ = toes;
        free(joint, v(0.0, -0.06, -1.0), 84.0, v(0.2, 0.0, 1.0))
    };
    // The hock comes down beside the rump, a little behind the hip, the thigh folded forward
    // along the belly and the cannon tucked forward under it.
    let _ = (st, ho, hf);
    let hind = free(
        v(hip.x + 0.06 * w, on_ground, hip.z + 0.14 * w),
        v(0.15, -0.06, -1.0),
        if kind.hoofed() { 40.0 } else { 84.0 },
        v(0.7, 0.3, -1.0),
    );
    let [fl, fr] = pair(front);
    let [hl, hr] = pair(hind);
    [fl, fr, hl, hr]
}

/// How far the body comes down to lie on its chest, m (scale 1).
fn lie_drop(kind: Wild, f: &Frame) -> f32 {
    (f.body.y - (f.belly + 0.02 * kind.withers())).max(0.0)
}

/// Neck pitches that bring the nose down to `height` (scale 1) from `base`, by bisection on
/// how far along `bend` (a full bend's pitches) the neck goes.
fn neck_to(kind: Wild, s: &Skeleton, base: Stance, bend: [f32; 4], height: f32) -> [f32; 4] {
    let (muzzle, _) = muzzle(kind);
    let nose = |t: f32| {
        let st = Stance {
            neck: std::array::from_fn(|i| base.neck[i] + bend[i] * t),
            at: 0.0,
            ..base
        };
        let keys = [st, st.at(1.0)];
        let (posed, _) = sample(&RIG, s, 1.0, &keys, 0.0);
        let mut local = s.rest.clone();
        for (b, x) in &posed {
            let k = s.bone(b);
            local[k].r = x.r;
            if x.t != Vec3::ZERO {
                local[k].t = x.t;
            }
        }
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        model[s.bone("head")].transform_point(muzzle).y
    };
    // The nose falls as the neck bends until it curls under: bisect up to the first grid
    // step at or below `height` (or the lowest one).
    let steps = 32;
    let at = |i: usize| 1.6 * i as f32 / steps as f32;
    let first = (0..=steps).find(|&i| nose(at(i)) <= height);
    let (mut lo, mut hi) = match first {
        Some(0) => (0.0, 0.0),
        Some(i) => (at(i - 1), at(i)),
        None => {
            let low = (0..=steps)
                .min_by(|a, b| nose(at(*a)).total_cmp(&nose(at(*b))))
                .unwrap_or(steps);
            (at(low), at(low))
        }
    };
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if nose(mid) > height {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = 0.5 * (lo + hi);
    std::array::from_fn(|i| base.neck[i] + bend[i] * t)
}

/// One ear key: both ears turned `pitch` (+ back) and `spread` (+ out) degrees.
#[derive(Clone, Copy)]
struct Ears {
    at: f32,
    pitch: f32,
    spread: f32,
}

const fn ears(at: f32, pitch: f32, spread: f32) -> Ears {
    Ears { at, pitch, spread }
}

/// An action clip through `keys` with the ears keyed by `ear_keys` and the `reach` curve.
fn wild_clip(
    s: &Skeleton,
    scale: f32,
    name: &str,
    keys: &[Stance],
    ear_keys: &[Ears],
    reach: &[(f32, f32)],
) -> AnimationClip {
    let secs = keys.last().map_or(1.0, |k| k.at).max(0.1);
    let samples = (secs * 30.0) as usize + 1;
    let ear_at = |t: f32| -> (f32, f32) {
        let p: Vec<(f32, f32)> = ear_keys.iter().map(|e| (e.at, e.pitch)).collect();
        let w: Vec<(f32, f32)> = ear_keys.iter().map(|e| (e.at, e.spread)).collect();
        (keyed(t, &p), keyed(t, &w))
    };
    let mut clip = AnimationClip::from_fn(name, secs, samples, false, |ph| {
        let t = ph * secs;
        let (mut posed, _) = sample(&RIG, s, scale, keys, t);
        if !ear_keys.is_empty() {
            let (pitch, spread) = ear_at(t);
            let mut p = PoseBuilder::new(s);
            p.rot("ear_l", pitch, 0.0, spread)
                .rot("ear_r", pitch, 0.0, -spread);
            posed.extend(p.take());
        }
        posed
    });
    for (leg, curve) in RIG.release.iter().enumerate() {
        clip = clip.with_curve(curve, samples, |ph| {
            sample(&RIG, s, scale, keys, ph * secs).1[leg]
        });
    }
    clip.with_curve("reach", samples, |ph| keyed(ph * secs, reach))
}

/// The kind's action clips for its skeleton `s` at `scale` (named for the size).
pub fn action_clips(kind: Wild, s: &Skeleton, scale: f32) -> Vec<AnimationClip> {
    let f = frame(kind);
    let w = kind.withers();
    let k = kind.name();
    let rest = Stance::REST;
    let clip = |base: &str, keys: &[Stance], ear_keys: &[Ears], reach: &[(f32, f32)]| {
        wild_clip(
            s,
            scale,
            &sized_name(&format!("{k}_{base}"), &s.name),
            keys,
            ear_keys,
            reach,
        )
    };
    // The full-size skeleton for neck solving (lengths in the keys are at scale 1).
    let unit = if scale == 1.0 {
        s.clone()
    } else {
        skeleton(kind, 1.0)
    };
    let mut out = Vec::new();

    // Graze: the head goes down to the grass, the jaw works, and every so often the head comes
    // up a little to look about before going down again. A boar roots, nodding into the soil;
    // a rabbit crouches lower and nibbles fast.
    let crouch = match kind {
        Wild::Rabbit => Stance {
            body: v(0.0, -0.012, 0.0),
            pitch: -6.0,
            ..rest
        },
        Wild::Boar => Stance {
            pitch: -3.0,
            ..rest
        },
        _ => Stance {
            body: v(0.0, -0.03 * w, 0.0),
            pitch: -5.0,
            ..rest
        },
    };
    let bend = match kind {
        // The neck swings down from its base while the head tips back up, nose to the
        // ground (bending every bone the same way would curl the nose under).
        Wild::Deer => [-100.0, -10.0, 0.0, 45.0],
        Wild::Wolf => [-100.0, -10.0, 0.0, 40.0],
        Wild::Boar => [-30.0, -10.0, 0.0, 0.0],
        Wild::Rabbit => [-60.0, -10.0, 0.0, 25.0],
    };
    let ground = match kind {
        Wild::Boar => -0.01 * w,
        _ => 0.03 * w,
    };
    let down = neck_to(kind, &unit, crouch, bend, ground);
    let half = neck_to(kind, &unit, crouch, bend, ground + 0.18 * w);
    let feed = |at: f32, chew: f32, yaw: f32| Stance {
        at,
        neck: down,
        neck_yaw: [yaw, yaw * 0.8, 0.0, yaw * 0.5],
        jaw: 4.0 + 6.0 * chew,
        tail: 4.0,
        ..crouch
    };
    let mut graze = vec![rest.at(0.0), feed(0.9, 0.0, 0.0)];
    let chew_rate = if kind == Wild::Rabbit { 0.12 } else { 0.25 };
    let mut t = 1.0;
    while t < GRAZE_HOLD - 0.05 {
        let n = (t / chew_rate) as i32;
        graze.push(feed(t, (n % 2) as f32, 6.0 * cycle_sin(t * 0.3)));
        t += chew_rate;
    }
    graze.push(feed(GRAZE_HOLD, 0.0, 0.0));
    // Head up to look, chewing, then down again (the held pose loops back to here).
    graze.extend([
        Stance {
            at: GRAZE_HOLD + 0.6,
            neck: half,
            neck_yaw: [10.0, 8.0, 0.0, 10.0],
            jaw: 6.0,
            ..crouch
        },
        Stance {
            at: GRAZE_HOLD + 1.4,
            neck: half,
            neck_yaw: [-6.0, -4.0, 0.0, -8.0],
            jaw: 3.0,
            ..crouch
        },
        rest.at(GRAZE_HOLD + 2.2),
    ]);
    if kind == Wild::Boar {
        // Rooting: the snout shoves forward and down through the soil.
        for st in graze.iter_mut().skip(2).take(8) {
            st.neck[3] -= 8.0 * ((st.at * 4.0).sin());
            st.body.z -= 0.01 * w * (st.at * 4.0).sin();
        }
    }
    out.push(clip(
        "graze",
        &graze,
        &[
            ears(0.0, 0.0, 0.0),
            ears(0.9, 10.0, 8.0),
            ears(GRAZE_HOLD, 10.0, 8.0),
            ears(GRAZE_HOLD + 0.6, -10.0, 0.0),
            ears(GRAZE_HOLD + 2.2, 0.0, 0.0),
        ],
        &[
            (0.0, 0.0),
            (0.6, 1.0),
            (GRAZE_HOLD, 1.0),
            (GRAZE_HOLD + 0.4, 0.0),
        ],
    ));

    // Lie: down on the chest (a deer's hindquarters go first, then it kneels in front), the
    // head up, held; then up again.
    let drop = lie_drop(kind, &f);
    let legs = lie_legs(kind, &f, drop);
    let lying = Stance {
        body: v(0.0, -drop, 0.0),
        pitch: 2.0,
        loin: 3.0,
        neck: match kind {
            Wild::Deer => [-8.0, 6.0, 0.0, -4.0],
            Wild::Boar => [-6.0, -4.0, 0.0, -10.0],
            _ => [4.0, 2.0, 0.0, -6.0],
        },
        tail: 2.0,
        legs,
        ..rest
    };
    let halfway = Stance {
        body: v(0.0, -drop * 0.5, 0.02 * w),
        pitch: if kind.hoofed() { -14.0 } else { 14.0 },
        loin: 6.0,
        legs: if kind.hoofed() {
            [legs[0], legs[1], Leg::Planted, Leg::Planted]
        } else {
            [Leg::Planted, Leg::Planted, legs[2], legs[3]]
        },
        ..rest
    };
    let lie = vec![
        rest.at(0.0),
        halfway.at(0.8),
        lying.at(1.6),
        Stance {
            neck_yaw: [14.0, 10.0, 0.0, 12.0],
            ..lying.at(2.0)
        },
        lying.at(LIE_HOLD),
        halfway.at(LIE_HOLD + 0.9),
        rest.at(LIE_HOLD + 1.6),
    ];
    out.push(clip(
        "lie",
        &lie,
        &[
            ears(0.0, 0.0, 0.0),
            ears(1.6, 12.0, 6.0),
            ears(LIE_HOLD + 1.6, 0.0, 0.0),
        ],
        &[(0.0, 0.0)],
    ));

    // Alert: still, the head up and the ears forward at the danger; a deer's tail flares, a
    // boar's crest rises, a rabbit sits up on its haunches.
    let alert = match kind {
        Wild::Rabbit => {
            let tuck = free(
                f.front[3] + v(0.0, 0.1 * w, 0.15 * w),
                v(0.0, -0.4, -1.0),
                40.0,
                v(0.0, 0.0, 1.0),
            );
            let [fl, fr] = pair(tuck);
            Stance {
                body: v(0.0, 0.25 * w, 0.06 * w),
                pitch: 55.0,
                loin: -18.0,
                neck: [-25.0, -10.0, 0.0, -18.0],
                legs: [fl, fr, Leg::Planted, Leg::Planted],
                ..rest
            }
        }
        Wild::Deer => Stance {
            pitch: 3.0,
            neck: [18.0, 10.0, 0.0, -8.0],
            tail: 35.0,
            ..rest
        },
        Wild::Wolf => Stance {
            neck: [16.0, 8.0, 0.0, -10.0],
            tail: 14.0,
            jaw: -2.0,
            ..rest
        },
        Wild::Boar => Stance {
            neck: [12.0, 6.0, 0.0, 6.0],
            tail: 25.0,
            ..rest
        },
    };
    let alert_keys = [
        rest.at(0.0),
        alert.at(0.5),
        Stance {
            neck_yaw: [6.0, 5.0, 0.0, 8.0],
            ..alert.at(0.9)
        },
        alert.at(ALERT_HOLD),
        rest.at(ALERT_HOLD + 0.8),
    ];
    out.push(clip(
        "alert",
        &alert_keys,
        &[
            ears(0.0, 0.0, 0.0),
            ears(0.35, -22.0, -6.0),
            ears(ALERT_HOLD, -22.0, -6.0),
            ears(ALERT_HOLD + 0.8, 0.0, 0.0),
        ],
        &[(0.0, 0.0)],
    ));

    // Flinch: a jolt away from the hurt, the head jerking, the tail clamped.
    let jolt = Stance {
        body: v(0.0, -0.04 * w, 0.03 * w),
        pitch: -5.0,
        roll: 7.0,
        yaw: 6.0,
        neck: [10.0, 8.0, 0.0, 12.0],
        neck_yaw: [-12.0, -10.0, 0.0, -14.0],
        jaw: 10.0,
        tail: -12.0,
        ..rest
    };
    out.push(clip(
        "flinch",
        &[
            rest.at(0.0),
            jolt.at(0.1),
            Stance {
                roll: -3.0,
                ..jolt.at(0.25)
            },
            rest.at(0.65),
        ],
        &[
            ears(0.0, 0.0, 0.0),
            ears(0.1, 35.0, 10.0),
            ears(0.65, 0.0, 0.0),
        ],
        &[(0.0, 0.0)],
    ));

    // Fall: the legs give, it goes down on its side and lies still.
    let roll = 88.0f32;
    let q = Quat::from_rotation_z(roll.to_radians());
    let lie_y = f.flank + 0.01 * w;
    let pivot = f.body;
    let offset = v(0.0, lie_y - f.body.y, 0.0);
    let on_side = |p: Vec3| pivot + offset + q * (p - pivot);
    let side_leg = |front: bool, side: usize| {
        let (joint, end) = if front {
            (f.front[3], f.front[4])
        } else {
            (f.hind[2], f.hind[3])
        };
        let (j, e) = (sided(joint, side), sided(end, side));
        // Limp: drawn in a little toward the body.
        let jj = on_side(j);
        let toward = (on_side(if front {
            sided(f.front[1], side)
        } else {
            sided(f.hind[0], side)
        }) - jj)
            * 0.12;
        free(
            jj + toward,
            q * (e - j).normalize(),
            25.0,
            q * if front { Vec3::Z } else { Vec3::NEG_Z },
        )
    };
    let side_legs = [
        side_leg(true, 0),
        side_leg(true, 1),
        side_leg(false, 0),
        side_leg(false, 1),
    ];
    let mut down_side = Stance {
        body: offset,
        roll,
        legs: side_legs,
        tail: -4.0,
        jaw: 8.0,
        ..rest
    };
    // Which way the neck yaws to lay the head on the ground (its frame rolled with the body).
    let head_height = |yaw: f32| {
        let st = Stance {
            neck_yaw: [yaw, yaw * 0.8, 0.0, yaw * 0.6],
            ..down_side
        };
        let (posed, _) = sample(&RIG, &unit, 1.0, &[st, st.at(1.0)], 0.0);
        let mut local = unit.rest.clone();
        for (b, x) in &posed {
            let i = unit.bone(b);
            local[i].r = x.r;
            if x.t != Vec3::ZERO {
                local[i].t = x.t;
            }
        }
        let mut model = unit.rest_model.clone();
        unit.to_model(&local, &mut model);
        model[unit.bone("head")].t.y
    };
    let yaw = if head_height(25.0) < head_height(-25.0) {
        25.0
    } else {
        -25.0
    };
    down_side.neck_yaw = [yaw, yaw * 0.8, 0.0, yaw * 0.6];
    let buckle = Stance {
        body: v(0.0, -0.3 * (f.body.y - lie_y), 0.0),
        roll: 18.0,
        pitch: -8.0,
        neck: [-10.0, -8.0, 0.0, -6.0],
        legs: lie_legs(kind, &f, 0.3 * (f.body.y - lie_y)),
        ..rest
    };
    out.push(clip(
        "fall",
        &[
            rest.at(0.0),
            buckle.at(0.45),
            down_side.at(1.1),
            down_side.at(FALL_HOLD),
            // Past the hold by more than the overlay's fade-out, so it holds at full weight.
            down_side.at(FALL_HOLD + 0.6),
        ],
        &[ears(0.0, 0.0, 0.0), ears(1.1, 30.0, 10.0)],
        &[(0.0, 0.0)],
    ));

    match kind {
        // Rear: a deer bolting from something close goes up on its hind legs, the forelegs
        // tucked, then springs away.
        Wild::Deer => {
            let tuck = free(
                f.front[3] + v(0.0, 0.32, 0.06),
                v(0.0, -0.3, 1.0),
                70.0,
                v(0.0, 0.0, 1.0),
            );
            let [fl, fr] = pair(tuck);
            let up = Stance {
                body: v(0.0, 0.12, 0.25),
                pitch: 38.0,
                loin: -12.0,
                neck: [14.0, 6.0, 0.0, -14.0],
                tail: 40.0,
                legs: [fl, fr, Leg::Planted, Leg::Planted],
                ..rest
            };
            let gather = Stance {
                body: v(0.0, -0.08, 0.06),
                pitch: 10.0,
                loin: 8.0,
                neck: [10.0, 4.0, 0.0, -6.0],
                tail: 30.0,
                ..rest
            };
            out.push(clip(
                "rear",
                &[
                    rest.at(0.0),
                    gather.at(0.2),
                    up.at(0.55),
                    up.at(0.75),
                    gather.at(1.05),
                    rest.at(1.35),
                ],
                &[
                    ears(0.0, 0.0, 0.0),
                    ears(0.2, -25.0, -5.0),
                    ears(1.35, 0.0, 0.0),
                ],
                &[(0.0, 0.0)],
            ));
        }
        // Howl: it sits, raises its muzzle to the sky and howls, the jaw rounding open.
        Wild::Wolf => {
            let reach = 0.78 * (len(f.hind[0], f.hind[1]) + len(f.hind[1], f.hind[2]));
            let joint = fold_to(
                f.hind[0] - v(0.0, 0.18, 0.0),
                reach,
                0.035 * w,
                v(0.2, 0.0, 0.2),
            );
            let [hl, hr] = pair(free(joint, v(0.0, -0.06, -1.0), 84.0, v(0.4, 0.2, -1.0)));
            let sit = Stance {
                body: v(0.0, -0.12, 0.04),
                pitch: 32.0,
                loin: 14.0,
                neck: [-12.0, -8.0, 0.0, -10.0],
                tail: 6.0,
                legs: [Leg::Planted, Leg::Planted, hl, hr],
                ..rest
            };
            let howl = |at: f32, open: f32| Stance {
                at,
                neck: [22.0, 24.0, 0.0, 28.0],
                jaw: 6.0 + 16.0 * open,
                ..sit
            };
            out.push(clip(
                "howl",
                &[
                    rest.at(0.0),
                    sit.at(0.8),
                    howl(1.4, 0.3),
                    howl(1.9, 1.0),
                    howl(3.2, 0.8),
                    howl(3.8, 1.0),
                    howl(4.4, 0.2),
                    sit.at(5.0),
                    rest.at(5.8),
                ],
                &[
                    ears(0.0, 0.0, 0.0),
                    ears(1.4, 20.0, 4.0),
                    ears(4.4, 20.0, 4.0),
                    ears(5.8, 0.0, 0.0),
                ],
                &[(0.0, 0.0)],
            ));
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::muscle::MorphWeights;

    fn pose_at(clip: &AnimationClip, s: &Skeleton, t: f32) -> Vec<crate::xform::Xf> {
        let binding = crate::clip::ClipBinding::new(clip, s);
        let mut local = s.rest.clone();
        clip.sample(t, &binding, &mut local);
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        model
    }

    #[test]
    fn every_animal_stands_on_its_feet_with_its_head_above_its_withers() {
        for kind in ALL {
            for scale in [1.0, 0.5] {
                let s = skeleton(kind, scale);
                let f = frame(kind);
                for side in 0..2 {
                    for prefix in ["front", "hind"] {
                        let p = s.rest_model[s.bone(&name(prefix, "foot", side))].t;
                        assert!(
                            (p.y - f.sole * scale).abs() < 1e-4,
                            "{kind:?} {prefix} foot at {}",
                            p.y
                        );
                    }
                }
                let mesh = mesh(kind, scale, Look::PLAIN);
                let (lowest, highest) = mesh.vertices.iter().fold((f32::MAX, f32::MIN), |a, v| {
                    (a.0.min(v.position.y), a.1.max(v.position.y))
                });
                let w = kind.withers() * scale;
                assert!(lowest.abs() < 0.02 * w, "{kind:?}: feet end at {lowest}");
                assert!(
                    highest > w,
                    "{kind:?}: top at {highest} under the withers {w}"
                );
                let morphs = MorphWeights::new(mesh.morph_names());
                for m in super::super::dog::muscles() {
                    assert!(morphs.index_of(&m.bulge).is_some(), "{kind:?} {}", m.bulge);
                }
                let furred = mesh.vertices.iter().filter(|v| v.fur > 0.5).count();
                assert!(
                    furred > mesh.vertices.len() / 4,
                    "{kind:?}: {furred} furred"
                );
            }
        }
    }

    #[test]
    fn a_stag_grows_antlers_and_a_fawn_wears_spots() {
        let stag = Look::of(Wild::Deer, "deer", scale_for_height(Wild::Deer, 1.32), 7);
        let doe = Look::of(Wild::Deer, "deer", 1.0, 7);
        let fawn = Look::of(Wild::Deer, "deer", 0.5, 7);
        assert!(stag.antlers >= 0.7 && doe.antlers == 0.0 && fawn.antlers == 0.0);
        assert!(fawn.spots && !doe.spots);
        assert!(Look::of(Wild::Deer, "stag", 1.0, 3).antlers > 0.0);
        let top = |look: Look| {
            mesh(Wild::Deer, 1.0, look)
                .vertices
                .iter()
                .fold(f32::MIN, |a, v| a.max(v.position.y))
        };
        assert!(top(stag) > top(doe) + 0.4, "antlers rise above the head");
        assert_eq!(Wild::of_kind("stag"), Some(Wild::Deer));
        assert_eq!(Wild::of_skeleton("rabbit@100mm"), Some(Wild::Rabbit));
        assert_eq!(skeleton_name(Wild::Deer, 1.0), "deer");
        assert_eq!(
            skeleton_name(Wild::Deer, scale_for_height(Wild::Deer, 0.6)),
            "deer@60cm"
        );
        assert_eq!(
            skeleton_name(Wild::Rabbit, scale_for_height(Wild::Rabbit, 0.1)),
            "rabbit@100mm"
        );
    }

    #[test]
    fn a_deer_has_a_deep_chest_a_tucked_up_flank_and_its_fawn_spots_lie_in_the_coat() {
        let hind = Look::of(Wild::Deer, "deer", 1.0, 3);
        let m = mesh(Wild::Deer, 1.0, hind);
        // The underline of the trunk (between the legs, off the legs themselves) at a z.
        let under = |z: f32| {
            m.vertices
                .iter()
                .filter(|v| (v.position.z - z).abs() < 0.03 && v.position.x.abs() < 0.04)
                .fold(f32::MAX, |a, v| a.min(v.position.y))
        };
        let (girth, flank) = (under(-0.3), under(0.18));
        assert!(
            girth < 0.74 && flank > girth + 0.05,
            "belly at {girth} behind the elbows, {flank} at the flank"
        );
        // The hams no wider than the croup and barrel around them carry.
        let widest = |z0: f32, z1: f32| {
            m.vertices
                .iter()
                .filter(|v| v.position.z > z0 && v.position.z < z1 && v.position.y > 0.85)
                .fold(0.0f32, |a, v| a.max(v.position.x.abs()))
        };
        let (barrel, hams) = (widest(-0.3, 0.1), widest(0.35, 0.6));
        assert!(
            hams < barrel + 0.035,
            "hams {hams} m out, the barrel {barrel} m"
        );
        // A fawn's spots colour the coat without standing up from the back.
        let scale = 0.5;
        let fawn = Look::of(Wild::Deer, "deer", scale, 5);
        assert!(fawn.spots);
        let top = |look: Look| {
            let m = mesh(Wild::Deer, scale, look);
            let pale = m
                .vertices
                .iter()
                .filter(|v| v.position.z.abs() < 0.4 * scale && v.color.x > 0.45)
                .count();
            let back = m
                .vertices
                .iter()
                .filter(|v| v.position.z.abs() < 0.4 * scale)
                .fold(f32::MIN, |a, v| a.max(v.position.y));
            (pale, back)
        };
        let (spotted, back) = top(fawn);
        let (plain, bare) = top(Look {
            spots: false,
            ..fawn
        });
        assert!(
            spotted > plain + 50,
            "{spotted} pale vertices, {plain} plain"
        );
        assert!(
            back < bare + 0.004 * scale,
            "the spotted back at {back}, plain {bare}"
        );
    }

    #[test]
    fn a_boar_is_widest_at_its_shoulders_with_no_waist_before_its_hams() {
        let m = mesh(Wild::Boar, 1.0, Look::PLAIN);
        // Half the body's width over the back half of its height, around a z.
        let width = |z: f32| {
            m.vertices
                .iter()
                .filter(|v| (v.position.z - z).abs() < 0.03 && v.position.y > 0.45)
                .fold(0.0f32, |a, v| a.max(v.position.x.abs()))
        };
        let shoulders = width(-0.45);
        let barrel = [-0.1, 0.0, 0.1].map(width);
        let hams = width(0.4);
        let waist = barrel.into_iter().fold(f32::MAX, f32::min);
        assert!(
            shoulders > hams && waist > hams - 0.02,
            "shoulders {shoulders}, barrel {barrel:?}, hams {hams}"
        );
    }

    #[test]
    fn a_piglet_wears_pale_stripes_along_its_flanks() {
        let piglet_scale = scale_for_height(Wild::Boar, 0.425);
        let piglet = Look::of(Wild::Boar, "boar", piglet_scale, 8);
        let sow = Look::of(Wild::Boar, "boar", 1.0, 8);
        assert!(piglet.stripes && !sow.stripes);
        assert_ne!(
            piglet.key(),
            Look {
                stripes: false,
                ..piglet
            }
            .key()
        );
        // Pale vertices on the flanks, in bands one above another.
        let pale_heights = |look: Look, scale: f32| {
            let mut ys: Vec<i32> = mesh(Wild::Boar, scale, look)
                .vertices
                .iter()
                .filter(|v| {
                    let p = v.position / scale;
                    p.x.abs() > 0.06 && p.z.abs() < 0.5 && v.color.x > 0.25
                })
                .map(|v| (v.position.y / scale * 100.0).round() as i32)
                .collect();
            ys.sort_unstable();
            ys.dedup();
            ys
        };
        let bands = pale_heights(piglet, piglet_scale);
        assert!(
            bands.len() >= 8,
            "pale at heights {bands:?} (cm at scale 1)"
        );
        assert!(pale_heights(sow, 1.0).is_empty());
    }

    #[test]
    fn grazing_puts_the_nose_to_the_ground_and_lying_folds_the_legs_under() {
        for kind in ALL {
            for scale in [1.0, 0.5] {
                let s = skeleton(kind, scale);
                let clips = action_clips(kind, &s, scale);
                let get = |base: &str| {
                    let n = sized_name(&format!("{}_{base}", kind.name()), &s.name);
                    clips
                        .iter()
                        .find(|c| c.name == n)
                        .unwrap_or_else(|| panic!("{n}"))
                };
                let w = kind.withers() * scale;
                let (muzzle, _) = muzzle(kind);
                let model = pose_at(get("graze"), &s, GRAZE_HOLD - 0.05);
                let nose = model[s.bone("head")].transform_point(muzzle * scale).y;
                assert!(nose < 0.12 * w, "{kind:?} {scale}: nose at {nose} grazing");
                let model = pose_at(get("lie"), &s, LIE_HOLD - 0.05);
                let body = model[s.bone("body")].t.y;
                assert!(body < 0.55 * w, "{kind:?} {scale}: body at {body} lying");
                for prefix in ["front", "hind"] {
                    for side in 0..2 {
                        let y = model[s.bone(&name(prefix, "cannon", side))].t.y;
                        assert!(
                            y > -0.03 * w && y < 0.12 * w,
                            "{kind:?} {scale}: {prefix} cannon {side} at {y} lying"
                        );
                    }
                }
                let lie = get("lie");
                let release = lie.curve("release.hind_l", LIE_HOLD).unwrap();
                assert!(release > 0.99);
                // Down on its side: the body low, the head on the ground.
                let model = pose_at(get("fall"), &s, FALL_HOLD);
                let body = model[s.bone("body")].t.y;
                let head = model[s.bone("head")].t.y;
                assert!(body < 0.35 * w, "{kind:?}: body at {body} fallen");
                assert!(head < 0.45 * w, "{kind:?}: head at {head} fallen");
                for name in action_names(kind) {
                    let n = sized_name(&name, &s.name);
                    assert!(clips.iter().any(|c| c.name == n), "{n}");
                }
            }
        }
    }

    #[test]
    fn a_howling_wolf_points_its_muzzle_at_the_sky_and_a_rabbit_sits_up_to_look() {
        let s = skeleton(Wild::Wolf, 1.0);
        let clips = action_clips(Wild::Wolf, &s, 1.0);
        let howl = clips.iter().find(|c| c.name == "wolf_howl").unwrap();
        let model = pose_at(howl, &s, 2.5);
        let head = model[s.bone("head")];
        let nose = head.transform_point(frame(Wild::Wolf).muzzle);
        let up = (nose - head.t).normalize();
        assert!(up.y > 0.5, "the muzzle points {up}");
        let s = skeleton(Wild::Rabbit, 1.0);
        let clips = action_clips(Wild::Rabbit, &s, 1.0);
        let alert = clips.iter().find(|c| c.name == "rabbit_alert").unwrap();
        let model = pose_at(alert, &s, ALERT_HOLD);
        let head = model[s.bone("head")].t.y;
        assert!(head > 0.28, "the rabbit's head at {head} sitting up");
    }
}
