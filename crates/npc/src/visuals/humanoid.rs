//! Fighter bodies. With the renderer they are built from the procedural armored kit of
//! [`crate::art::fighter`]; without it, from a few capsules. Either way one skeleton is posed
//! every step from the AI state and from what the body did the step before (`Motion`): the
//! feet stay where they are on the ground while the body moves over them, and step along the
//! gait (or, standing, back under the body when out of place, pivoting on the ball of the
//! foot as the body turns); the pelvis, chest and head are placed from the gait, stance and
//! action (the body behind a blow, a block or a stagger; the hips lagging a turn on the spot);
//! then two-bone chains solve the legs and the arms (hands on the weapon's grips, a magazine
//! or an arrow), so knees and elbows bend. What hangs off the body (the crest, the tabards,
//! the belt pouches) swings after it on damped springs ([`Sway`]).
//!
//! Over that: where it looks (the eyes first, the head a beat behind them, the shoulders
//! taking what the neck can't; see [`Gaze`]), its face (blinks, a set jaw, a grimace, the
//! mouth open breathing hard), a bad wound (a limp or a hunch, a hand pressed to it at ease),
//! the weapon carried at ease and readied at the sight of a foe, getting up when revived,
//! from the pose it settled in on the ground, in the owner's frame (so it plays wherever that
//! is), and reviving a downed squad mate (down on both knees beside it, the weapon set down or
//! held aside, the hands on its chest, looking up at the threat now and then; see [`Tend`]).
//! What it remembers for this is times, amounts and entities, never places in the world (the
//! mate's chest is kept in the owner's frame).

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};
use engine_world::{AttackKind, DamageEvent, DamageKind};

use super::*;
use crate::agent::{
    AlertLevel, AttackPhase, Defense, IncomingAttack, NpcAction, NpcBrain, NpcCombat, NpcVitals,
    NpcWeapon, WeaponStyle,
};
use crate::art::ArtAssets;
use crate::art::fighter::{FACE_SHAPES, Piece, WEARS, basis, kit, rig, weapons};
use crate::combat::{Downed, Lie};
use crate::faction::Faction;
use crate::perception::NpcMemory;
use crate::skills::NpcSkills;

// ============================================================================ parts

// The chest is `Part::Torso`, the helmet and coif `Head` with its brow light `Orb` (it glows
// with the alert level) and rank crest `Band`, the thighs `LegL`/`LegR`, the upper arms
// `ArmL`/`ArmR`, the blade, carbine or bow riser `Weapon`. The other pieces:
const PELVIS: Part = Part::Fighter(0);
const SHIN_L: Part = Part::Fighter(1);
const SHIN_R: Part = Part::Fighter(2);
const FOOT_L: Part = Part::Fighter(3);
const FOOT_R: Part = Part::Fighter(4);
const FOREARM_L: Part = Part::Fighter(5);
const FOREARM_R: Part = Part::Fighter(6);
const HAND_L: Part = Part::Fighter(7);
const HAND_R: Part = Part::Fighter(8);
const TABARD_FRONT: Part = Part::Fighter(9);
const TABARD_BACK: Part = Part::Fighter(10);
const HILT: Part = Part::Fighter(11);
const LIMB_UPPER: Part = Part::Fighter(12);
const LIMB_LOWER: Part = Part::Fighter(13);
const STRING_UPPER: Part = Part::Fighter(14);
const STRING_LOWER: Part = Part::Fighter(15);
const ARROW: Part = Part::Fighter(16);
const QUIVER: Part = Part::Fighter(17);
// Capsule bodies without a renderer: trunk, head, the eight limb segments (thighs, shins,
// upper arms, forearms; left then right), weapon and shield.
const PLAIN_TRUNK: Part = Part::Fighter(18);
const PLAIN_HEAD: Part = Part::Fighter(19);
const PLAIN_SEG: u8 = 20;
const PLAIN_WEAPON: Part = Part::Fighter(28);
const PLAIN_SHIELD: Part = Part::Fighter(29);
/// The face under the helmet.
const FACE: Part = Part::Fighter(30);
/// The carbine's magazine (seated in it, or in the hand changing it).
const MAGAZINE: Part = Part::Fighter(31);
/// The pouches on the belt, right and left (they swing as the body moves).
const POUCH_R: Part = Part::Fighter(32);
const POUCH_L: Part = Part::Fighter(33);
/// The eyes (each turns about its own center to look), the upper lids of both (they turn down
/// over the eyes to blink) and the lower jaw (it drops to open the mouth). Numbered after the
/// painted plates' parts.
const EYE_L: Part = Part::Fighter(FACE0);
const EYE_R: Part = Part::Fighter(FACE0 + 1);
const LIDS: Part = Part::Fighter(FACE0 + 2);
const JAW: Part = Part::Fighter(FACE0 + 3);
/// A medic's armband, over the left upper arm (only medics have the part).
const ARMBAND: Part = Part::Fighter(FACE0 + 4);
const FACE0: u8 = PAINT0 + HUES * PAINTED.len() as u8;
/// Pieces with painted plates. Those are a mesh of their own (textured by the paint texture
/// in the team color, as worn as the fighter's skill says), a part posed with its piece: the
/// `i`th piece's is `Part::Fighter(PAINT0 + hue * PAINTED.len() + i)`, where `hue` is the
/// team color's (see [`hue_of`]), which the plates flash in when hit (the pose doesn't know
/// the team, and the flash can't reach the paint texture).
const PAINTED: [Part; 11] = [
    Part::Torso,
    Part::Head,
    Part::LegL,
    Part::LegR,
    SHIN_L,
    SHIN_R,
    Part::ArmL,
    Part::ArmR,
    FOREARM_L,
    FOREARM_R,
    Part::Shield,
];
const PAINT0: u8 = 34;

/// Every part's place in a pose: the named parts, the fighter's own up to the painted plates
/// (which share their pieces'), then the face's and the armband's.
const FACE_SLOT: usize = 11 + PAINT0 as usize;
const SLOTS: usize = FACE_SLOT + 5;

/// Skin tones (sRGB), picked per fighter.
const SKIN_TONES: [u32; 6] = [0xdcb8a0, 0xc29479, 0xa27760, 0x86614c, 0x5f4537, 0xd4a88f];
/// The darker skin tones (indices into [`SKIN_TONES`]): their eyes are brown or hazel.
const DARK_SKIN: [usize; 2] = [3, 4];
/// Iris colors (sRGB) at the rim and around the pupil: brown, hazel, blue, grey.
const IRISES: [[u32; 2]; 4] = [
    [0x3b2617, 0x5e3b1f],
    [0x4d5530, 0x8a5c28],
    [0x2f5680, 0x7397b8],
    [0x56626a, 0x8e989d],
];

/// Gambeson colors (sRGB), picked per fighter: undyed linen and oatmeal, and wool in tan,
/// grey, dark brown, olive drab and charcoal (mostly darker than the arena's sand, so the
/// team's colors stand out on them).
const GAMBESONS: [u32; 7] = [
    0xbcb09a, 0x7f6449, 0x7c7a73, 0xa08c6c, 0x5f4a38, 0x6b6649, 0x5c5b56,
];
/// Hose colors (sRGB), picked per fighter: dark brown, charcoal, olive-brown and umber wool.
const HOSE: [u32; 4] = [0x5a4836, 0x4a4a4d, 0x585340, 0x463a30];

/// The hues painted plates flash in: grey, then twelve round the color circle.
const HUES: u8 = 13;

/// A team color's hue, for its plates' flash: 0 for a grey, else 1 plus the nearest of twelve
/// hues round the circle, in steps of 30° from red (so 5 is green, 8 azure).
fn hue_of(team_hex: u32) -> u8 {
    let [r, g, b] = [16, 8, 0].map(|s| ((team_hex >> s) & 0xff) as f32);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 0.25 * max || max == 0.0 {
        return 0;
    }
    // In sixths of the circle.
    let h = if max == r {
        (g - b) / (max - min)
    } else if max == g {
        2.0 + (b - r) / (max - min)
    } else {
        4.0 + (r - g) / (max - min)
    };
    1 + ((h * 2.0).round() as i32).rem_euclid(12) as u8
}

/// The part of a piece's painted plates, for a team of hue `hue`.
fn paint_of(part: Part, hue: u8) -> Option<Part> {
    let i = PAINTED.iter().position(|&p| p == part)?;
    Some(Part::Fighter(PAINT0 + hue * PAINTED.len() as u8 + i as u8))
}

/// Where a part's pose is kept (painted plates: their piece's).
fn slot(part: Part) -> Option<usize> {
    let plates = PAINT0..PAINT0 + HUES * PAINTED.len() as u8;
    if let Part::Fighter(n) = part
        && plates.contains(&n)
    {
        return slot(PAINTED[(n - PAINT0) as usize % PAINTED.len()]);
    }
    if let Part::Fighter(n) = part
        && n >= FACE0
    {
        let i = FACE_SLOT + (n - FACE0) as usize;
        return (i < SLOTS).then_some(i);
    }
    let i = match part {
        Part::Torso => 0,
        Part::Head => 1,
        Part::Band => 2,
        Part::Orb => 3,
        Part::LegL => 4,
        Part::LegR => 5,
        Part::ArmL => 6,
        Part::ArmR => 7,
        Part::Weapon => 8,
        Part::Shield => 9,
        Part::Muzzle => 10,
        Part::Fighter(n) => 11 + n as usize,
        _ => return None,
    };
    (i < SLOTS).then_some(i)
}

/// Armor pieces that flash when hit.
const ARMOR: [Part; 12] = [
    Part::Torso,
    Part::Head,
    PELVIS,
    Part::LegL,
    Part::LegR,
    SHIN_L,
    SHIN_R,
    Part::ArmL,
    Part::ArmR,
    FOREARM_L,
    FOREARM_R,
    PLAIN_TRUNK,
];

/// What a fighter carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Two-handed longsword.
    Longsword,
    /// Arming sword and heater shield.
    SwordShield,
    Rifle,
    Bow,
    /// Bare fists (natural weapons).
    Fists,
}

impl Kind {
    fn of(w: &NpcWeapon) -> Self {
        match w.style {
            // Arrows fly slowly; bullets do not.
            WeaponStyle::Ranged if w.projectile_speed < 200.0 => Kind::Bow,
            WeaponStyle::Ranged => Kind::Rifle,
            WeaponStyle::Melee if !w.can_block => Kind::Fists,
            WeaponStyle::Melee if w.shield => Kind::SwordShield,
            WeaponStyle::Melee => Kind::Longsword,
        }
    }
}

/// Linear RGB of an sRGB hex color.
fn linear(hex: u32) -> Vec3 {
    let c = StandardMaterial::hex(hex).base_color;
    Vec3::new(c[0], c[1], c[2])
}

/// A material that glows: `hex` lit by `nits` of its own light (in daylight thousands show).
fn lit(hex: u32, nits: f32) -> StandardMaterial {
    StandardMaterial::hex(hex)
        .with_emissive(linear(hex).to_array(), nits)
        .with_roughness(0.4)
}

/// A part for an instant after a hit or a parry: lit up in a warm white over its own colors.
/// It keeps the mesh's vertex colors (the cloth's, the leather's and the team's, and the
/// baked occlusion) but not the textures, `FLASH` times as bright, plus a faint glow that
/// shows in shade but cannot blow it out to white in daylight.
fn flash() -> StandardMaterial {
    tinted_flash(Vec3::new(FLASH, FLASH * 0.96, FLASH * 0.88))
}

/// How much brighter a struck part's own colors get.
const FLASH: f32 = 1.5;

/// The flash of painted plates (whose color is in the texture the flash can't keep), for a
/// team of hue `hue` (see [`hue_of`]): a light tint of the team's hue, so a struck fighter
/// lights up without losing its team.
fn plate_flash(hue: u8) -> StandardMaterial {
    let tint = if hue == 0 {
        Vec3::new(1.0, 0.96, 0.88)
    } else {
        // The hue at full value and saturation `PLATE_FLASH.1` (sRGB), in linear.
        let h = (hue - 1) as f32 * 0.5;
        let c = [5.0, 3.0, 1.0].map(|n: f32| {
            let k = (n + h) % 6.0;
            1.0 - PLATE_FLASH.1 * k.min(4.0 - k).clamp(0.0, 1.0)
        });
        let hex = c
            .iter()
            .fold(0, |hex, c| hex << 8 | (c * 255.0).round() as u32);
        linear(hex)
    };
    tinted_flash(tint * PLATE_FLASH.0)
}

/// The albedo of struck painted plates, and how saturated their tint is.
const PLATE_FLASH: (f32, f32) = (0.75, 0.35);

/// A flash material: untextured, the vertex colors times `albedo`, with a faint glow.
fn tinted_flash(albedo: Vec3) -> StandardMaterial {
    let glow = albedo / albedo.max_element();
    StandardMaterial {
        base_color: [albedo.x, albedo.y, albedo.z, 1.0],
        perceptual_roughness: 0.6,
        ..Default::default()
    }
    .with_emissive(glow.to_array(), 500.0)
}

/// The string between a bow tip and the nock (constant: the limbs bend instead).
const STRING_HALF: f32 = 0.6516;

/// The parts of a fighter's body (a medic's with its armband).
pub(super) fn parts(
    stand_in: &StandIn,
    weapon: Option<&NpcWeapon>,
    faction: Option<&Faction>,
    skills: Option<&NpcSkills>,
    medic: bool,
    art: &mut ArtAssets,
) -> Vec<(Part, MeshRenderer, Vec3)> {
    let kind = Kind::of(&weapon.copied().unwrap_or_default());
    let team_hex = stand_in.color.unwrap_or_else(|| faction_color(faction));
    if !art.available() {
        return plain_parts(kind, team_hex);
    }
    let kit = kit();
    let base = StandardMaterial {
        base_color: [1.0; 4],
        ..Default::default()
    };
    let Some(MaterialRef::Inline(atlas)) = art.material("fighter/atlas", base, || kit.atlas())
    else {
        return plain_parts(kind, team_hex);
    };
    let team = linear(team_hex);
    // The crest's metal takes the rank color; a master's glows faintly.
    let level = skills.map(|s| s.level()).unwrap_or(0.5);
    let rank_hex = skill_color(level);
    let mut rank = StandardMaterial {
        base_color: StandardMaterial::hex(rank_hex).base_color,
        ..atlas
    };
    if level >= 0.8 {
        rank = rank.with_emissive(linear(rank_hex).to_array(), 400.0);
    }
    let mut list = vec![
        (Part::Torso, Piece::Chest),
        (Part::Head, Piece::Helmet),
        (FACE, Piece::Face),
        (EYE_L, Piece::Eye),
        (EYE_R, Piece::Eye),
        (LIDS, Piece::Lids),
        (JAW, Piece::Jaw),
        (Part::Orb, Piece::Visor),
        (PELVIS, Piece::Pelvis),
        (TABARD_FRONT, Piece::TabardFront),
        (TABARD_BACK, Piece::TabardBack),
        (Part::LegL, Piece::ThighL),
        (Part::LegR, Piece::ThighR),
        (SHIN_L, Piece::ShinL),
        (SHIN_R, Piece::ShinR),
        (FOOT_L, Piece::FootL),
        (FOOT_R, Piece::FootR),
        (Part::ArmL, Piece::UpperArmL),
        (Part::ArmR, Piece::UpperArmR),
        (FOREARM_L, Piece::ForearmL),
        (FOREARM_R, Piece::ForearmR),
        (HAND_L, Piece::HandL),
        (HAND_R, Piece::HandR),
        (POUCH_R, Piece::Pouch),
        (POUCH_L, Piece::Pouch),
    ];
    if !stand_in.plain {
        list.push((Part::Band, Piece::Crest));
    }
    if medic {
        list.push((ARMBAND, Piece::Armband));
    }
    match kind {
        Kind::Longsword => list.extend([(Part::Weapon, Piece::LongBlade), (HILT, Piece::LongHilt)]),
        Kind::SwordShield => list.extend([
            (Part::Weapon, Piece::ArmingBlade),
            (HILT, Piece::ArmingHilt),
            (Part::Shield, Piece::Shield),
        ]),
        Kind::Rifle => list.extend([
            (Part::Weapon, Piece::Rifle),
            (MAGAZINE, Piece::Magazine),
            (Part::Muzzle, Piece::Muzzle),
        ]),
        Kind::Bow => list.extend([
            (Part::Weapon, Piece::BowRiser),
            (LIMB_UPPER, Piece::BowLimb),
            (LIMB_LOWER, Piece::BowLimb),
            (ARROW, Piece::Arrow),
            (QUIVER, Piece::Quiver),
        ]),
        Kind::Fists => {}
    }
    // The face differs between fighters of a squad: its skin tone, shape and eye color all
    // come from one seed. Seeds closer than 24 never share both skin tone and shape. The eyes,
    // the upper lids and the jaw are parts of their own (they move).
    let seed = {
        let f = faction.map_or(0, |f| f.0);
        (f.wrapping_mul(7) + kind as u32 * 3 + (level * 10.0) as u32) as usize
    };
    let (skin_hex, shape, iris_hex) = {
        let skin = seed % SKIN_TONES.len();
        let shape = seed / SKIN_TONES.len() % FACE_SHAPES;
        let mut iris = (seed + seed / 4) % IRISES.len();
        if DARK_SKIN.contains(&skin) {
            iris %= 2;
        }
        (SKIN_TONES[skin], shape, IRISES[iris])
    };
    // The paint wears with skill: fresh on rookies, battle-worn on veterans and masters; the
    // same seed tips fighters near a boundary either way.
    let wear = {
        let tip = (seed * 5 % 7) as f32 / 6.0 - 0.5;
        let w = level + 0.24 * tip;
        if w < 0.35 {
            0
        } else if w < 0.72 {
            1
        } else {
            WEARS - 1
        }
    };
    let Some(paint) = paint_material(art, wear, team_hex) else {
        return plain_parts(kind, team_hex);
    };
    // Under the armor: a gambeson and hose in undyed or earthy wool and linen. Golden-ratio
    // steps spread neighboring seeds (a squad's) over the colors.
    let pick =
        |n: usize, offset: f64| ((seed as f64 * 0.618_034 + offset).fract() * n as f64) as usize;
    let cloth_hex = [
        GAMBESONS[pick(GAMBESONS.len(), 0.0)],
        HOSE[pick(HOSE.len(), 0.37)],
    ];
    let cloth = cloth_hex.map(linear);
    let mut out = Vec::with_capacity(list.len() + 2);
    for (part, piece) in list {
        let skin = linear(skin_hex);
        let mesh = match piece {
            Piece::Face => art.mesh(&format!("fighter/face{shape}/{skin_hex:06x}"), || {
                kit.face(shape, skin)
            }),
            Piece::Jaw => art.mesh(&format!("fighter/jaw{shape}/{skin_hex:06x}"), || {
                kit.jaw(shape, skin)
            }),
            Piece::Lids => art.mesh(&format!("fighter/lids{shape}/{skin_hex:06x}"), || {
                kit.lids(shape, skin)
            }),
            Piece::Eye => {
                let key = format!("fighter/eye/{:06x}{:06x}", iris_hex[0], iris_hex[1]);
                art.mesh(&key, || kit.eye(iris_hex.map(linear)))
            }
            _ => {
                // Keyed by the colors it takes.
                let mut key = format!("fighter/{}", piece.name());
                if piece.team_colored() {
                    key += &format!("/{team_hex:06x}");
                }
                for (used, hex) in piece.cloth_colored().into_iter().zip(cloth_hex) {
                    if used {
                        key += &format!("/{hex:06x}");
                    }
                }
                art.mesh(&key, || kit.mesh(piece, team, cloth))
            }
        };
        let Some(mesh) = mesh else {
            return plain_parts(kind, team_hex);
        };
        let r = match part {
            Part::Band => MeshRenderer::new(mesh, rank),
            Part::Muzzle => MeshRenderer::new(mesh, lit(0xffd49a, 60_000.0).with_roughness(1.0))
                .without_shadows(),
            _ => MeshRenderer::new(mesh, atlas),
        };
        out.push((part, r, Vec3::ONE));
        if let Some(plates) = paint_of(part, hue_of(team_hex)).filter(|_| kit.painted(piece)) {
            let key = format!("fighter/{}/paint", piece.name());
            let Some(mesh) = art.mesh(&key, || kit.paint_mesh(piece).unwrap_or_default()) else {
                return plain_parts(kind, team_hex);
            };
            out.push((plates, MeshRenderer::new(mesh, paint), Vec3::ONE));
        }
    }
    if kind == Kind::Bow {
        for part in [STRING_UPPER, STRING_LOWER] {
            let string = Shape::Cylinder {
                radius: 0.0018,
                height: STRING_HALF,
                segments: 6,
            };
            out.push((part, MeshRenderer::new(string, mat(0xe8dfcc)), Vec3::ONE));
        }
    }
    out
}

/// The painted plates' material: the paint texture at a wear level in the team color.
fn paint_material(art: &mut ArtAssets, wear: usize, team_hex: u32) -> Option<StandardMaterial> {
    let kit = kit();
    let team = linear(team_hex);
    let key = format!("fighter/paint{wear}/{team_hex:06x}");
    let (albedo, ..) = art.textures(&key, || kit.paint_albedo(wear, team))?;
    let (_, normal, orm) =
        art.textures(&format!("fighter/paint{wear}"), || kit.paint_maps(wear))?;
    Some(StandardMaterial {
        base_color: [1.0; 4],
        base_color_texture: Some(albedo),
        normal_texture: Some(normal),
        metallic_roughness_texture: Some(orm),
        occlusion_texture: Some(orm),
        // The ORM map carries roughness and metalness; the factors scale it.
        perceptual_roughness: 1.0,
        metallic: 1.0,
        // Satin paint mirrors less of the sky than the default (which would wash it pale).
        reflectance: 0.3,
        ..Default::default()
    })
}

/// Capsules on the same skeleton, for apps without a renderer (nothing is drawn).
fn plain_parts(kind: Kind, team_hex: u32) -> Vec<(Part, MeshRenderer, Vec3)> {
    let capsule = |radius: f32, height: f32| Shape::Capsule {
        radius,
        height,
        segments: 8,
    };
    let body = mat(team_hex);
    let dark = mat(0x2a2a2e);
    let mut out = vec![
        (PLAIN_TRUNK, capsule(0.17, 0.62), body),
        (
            PLAIN_HEAD,
            Shape::Sphere {
                radius: 0.12,
                segments: 10,
            },
            mat(SKIN),
        ),
    ];
    for k in 0..8u8 {
        let (radius, len, m) = match k / 2 {
            0 => (0.08, rig::THIGH, dark),
            1 => (0.06, rig::SHIN, dark),
            2 => (0.065, rig::UPPER_ARM, body),
            _ => (0.05, rig::FOREARM, body),
        };
        out.push((Part::Fighter(PLAIN_SEG + k), capsule(radius, len), m));
    }
    match kind {
        Kind::Longsword | Kind::SwordShield => out.push((
            PLAIN_WEAPON,
            Shape::Cuboid {
                size: [0.04, 0.06, 0.9],
            },
            mat(STEEL).with_metallic(0.9),
        )),
        Kind::Rifle | Kind::Bow => out.push((
            PLAIN_WEAPON,
            Shape::Cuboid {
                size: [0.06, 0.12, 0.8],
            },
            mat(GUNMETAL),
        )),
        Kind::Fists => {}
    }
    if kind == Kind::SwordShield {
        out.push((
            PLAIN_SHIELD,
            Shape::Cuboid {
                size: [0.5, 0.65, 0.06],
            },
            mat(WOOD),
        ));
    }
    out.into_iter()
        .map(|(part, shape, material)| (part, MeshRenderer::new(shape, material), Vec3::ONE))
        .collect()
}

// ============================================================================ math

/// A joint frame: position and rotation in the owner's space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Bone {
    pub(super) pos: Vec3,
    pub(super) rot: Quat,
}

impl Bone {
    const fn new(pos: Vec3, rot: Quat) -> Self {
        Self { pos, rot }
    }

    /// A point given in this frame.
    pub(super) fn at(&self, p: Vec3) -> Vec3 {
        self.pos + self.rot * p
    }

    /// A frame given in this frame.
    fn then(&self, b: Bone) -> Bone {
        Bone::new(self.at(b.pos), self.rot * b.rot)
    }

    fn mix(self, o: Bone, t: f32) -> Bone {
        Bone::new(self.pos.lerp(o.pos, t), self.rot.slerp(o.rot, t))
    }

    fn placed(self) -> Placed {
        at(self.pos, self.rot)
    }
}

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smoothstep from `e0` to `e1`.
fn step(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rx(a: f32) -> Quat {
    Quat::from_rotation_x(a)
}

fn ry(a: f32) -> Quat {
    Quat::from_rotation_y(a)
}

fn rz(a: f32) -> Quat {
    Quat::from_rotation_z(a)
}

/// Rotation whose −Z points along `fwd`, with +Y as close to `up` as possible.
fn aim(fwd: Vec3, up: Vec3) -> Quat {
    let z = (-fwd).normalize_or(Vec3::Z);
    let x = up.cross(z).normalize_or(z.any_orthonormal_vector());
    basis(x, z.cross(x), z)
}

/// Rotation of a segment hanging from its joint along `down` (its −Y), its +X toward `side`.
fn hang(down: Vec3, side: Vec3) -> Quat {
    let y = (-down).normalize_or(Vec3::Y);
    let x = (side - y * side.dot(y)).normalize_or(y.any_orthonormal_vector());
    basis(x, y, x.cross(y))
}

/// Two-bone chain from `root` reaching for `target` with bones `a` and `b` long, the middle
/// joint bending toward `pole`: the middle joint and where the chain ends (short of the
/// target when it is out of reach).
fn ik(root: Vec3, target: Vec3, a: f32, b: f32, pole: Vec3) -> (Vec3, Vec3) {
    let d = target - root;
    let dir = d.normalize_or(Vec3::NEG_Y);
    let len = d.length().clamp((a - b).abs() + 1e-3, (a + b) * 0.999);
    let x = (len * len + a * a - b * b) / (2.0 * len);
    let h = (a * a - x * x).max(0.0).sqrt();
    let side = (pole - dir * pole.dot(dir)).normalize_or(dir.any_orthonormal_vector());
    (root + dir * x + side * h, root + dir * len)
}

/// The two segments of a limb from `root` toward `target`, hinged about the axis normal to
/// both (`flip` for knees, which bend the other way; `side` when nearly straight). Returns the
/// segments and where the limb ends.
fn chain(
    root: Vec3,
    target: Vec3,
    (a, b): (f32, f32),
    pole: Vec3,
    side: Vec3,
    flip: bool,
) -> (Bone, Bone, Vec3) {
    let (mid, end) = ik(root, target, a, b, pole);
    let n = ((mid - root) / a).cross((end - mid) / b);
    let n = if flip { -n } else { n };
    let w = (n.length() / 0.2).min(1.0);
    let axis = n.normalize_or(side) * w + side * (1.0 - w);
    (
        Bone::new(root, hang(mid - root, axis)),
        Bone::new(mid, hang(end - mid, axis)),
        end,
    )
}

/// Grip center of the left (`k` = 0) or right hand in the hand's frame.
fn grip(k: usize) -> Vec3 {
    let g = rig::GRIP;
    if k == 0 { v3(-g.x, g.y, g.z) } else { g }
}

/// The right shoulder, or the left (`k` = 0), in the chest frame.
fn shoulder(k: usize) -> Vec3 {
    let s = rig::SHOULDER;
    if k == 0 { v3(-s.x, s.y, s.z) } else { s }
}

// ============================================================================ the action

/// What the upper body is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Act {
    /// Out of combat: weapon carried low.
    Relaxed,
    /// In combat, ready.
    Guard,
    /// Telegraphing a blow, aiming or drawing (0..1).
    Windup { p: f32, heavy: bool },
    /// After the blow or shot: seconds since it, how long the recovery lasts, and whether the
    /// blow was a heavy one.
    Strike { t: f32, dur: f32, heavy: bool },
    /// A blow called off before it landed (a feint, or cut short by a hit), `p` of the way
    /// through its telegraph, `u` s ago: it starts into the release, checks and settles back.
    Check { p: f32, u: f32, heavy: bool },
    /// Guard raised against a blow: `k` how far (easing in as it rises, out as it ends), `hit`
    /// seconds since the blow landed on it (negative before), `parry` a timed deflection,
    /// `line` the way the blow comes.
    Block {
        k: f32,
        hit: f32,
        parry: bool,
        line: Line,
    },
    /// Knocked off balance (`k` eases in and out), `e` s ago.
    Stagger { k: f32, e: f32 },
}

/// The way a blow comes at the guard, in the defender's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Line {
    /// From above: a heavy cut.
    High,
    /// From the left: a right-handed cut from in front.
    Left,
    /// From the right: a cut from a fighter off to that side.
    Right,
    /// Straight in: a thrust or a lunge.
    Thrust,
    /// Squared up to it: a shot, or a blow whose line can't be told (the plain guard).
    #[default]
    Square,
}

impl Line {
    #[cfg(test)]
    const ALL: [Line; 5] = [
        Line::High,
        Line::Left,
        Line::Right,
        Line::Thrust,
        Line::Square,
    ];

    /// The line of an attack (`how` it was telegraphed, if seen; `ranged` as the defender
    /// noticed it) from an attacker in the state `foe`, at a defender at `at` facing `facing`:
    /// a shot is met squarely, a dash attack is a thrust, a heavy blow a cut from above; a
    /// light cut comes in from the side the attacker's sword arm is on (the left, from in
    /// front), or from the right when it stands off to that side. Without an attacker to look
    /// at, the guard is raised squarely.
    fn of(
        how: Option<AttackKind>,
        ranged: bool,
        foe: Option<&Foe>,
        at: DVec3,
        facing: Quat,
    ) -> Line {
        if ranged
            || matches!(how, Some(AttackKind::Ranged | AttackKind::Spell))
            || foe.is_some_and(|f| f.ranged)
        {
            return Line::Square;
        }
        if how == Some(AttackKind::Dash) || foe.is_some_and(|f| f.dashing) {
            return Line::Thrust;
        }
        let Some(foe) = foe else {
            return Line::Square;
        };
        if foe.heavy {
            return Line::High;
        }
        let to = facing.inverse() * (foe.at - at).as_vec3();
        if to.x.atan2(-to.z) > 0.45 {
            Line::Right
        } else {
            Line::Left
        }
    }

    /// How a blow on this line drives a guard back (chest frame), `r` how hard: the offset
    /// and the turn.
    fn recoil(self, r: f32) -> (Vec3, Quat) {
        match self {
            Line::High => (v3(0.0, -0.07, 0.03) * r, rx(0.1 * r)),
            Line::Left => (v3(0.06, 0.0, 0.05) * r, rz(-0.3 * r)),
            Line::Right => (v3(-0.06, 0.0, 0.05) * r, rz(0.3 * r)),
            Line::Thrust => (v3(0.0, 0.0, 0.09) * r, rx(0.1 * r)),
            Line::Square => (v3(0.0, 0.03, 0.07) * r, rx(0.25 * r)),
        }
    }
}

/// Where a blow landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Head,
    Body,
    Legs,
}

/// A blow taken, as the pose remembers it (in the owner's frame and on its clock).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hurt {
    /// When (NPC clock).
    since: f64,
    zone: Zone,
    /// The side it landed on: −1 the left .. 1 the right.
    side: f32,
    /// Which way it pushed (owner frame x, z; unit, or zero).
    push: Vec2,
    /// How hard (1 a sword cut).
    strength: f32,
}

impl Hurt {
    /// Blow `b` landing at `now` on a fighter at `at` facing `facing`, from `source`: where it
    /// landed from the point it struck (a heavy cut, harder than its weapon's usual blow,
    /// lands high; a blast or a fall takes the legs), which side from the point or the push.
    fn of(b: &DamageEvent, at: DVec3, facing: Quat, now: f64, source: Option<&Foe>) -> Hurt {
        let inv = facing.inverse();
        let local = inv * (b.point - at).as_vec3();
        let p = inv * b.impulse;
        let push = Vec2::new(p.x, p.z).normalize_or_zero();
        let heavy = source.is_some_and(|f| f.damage > 0.0 && b.amount > 1.3 * f.damage);
        let zone = match b.kind {
            DamageKind::Explosive | DamageKind::Fall => Zone::Legs,
            _ if heavy || local.y > 1.45 => Zone::Head,
            _ if local.y < 0.85 => Zone::Legs,
            _ => Zone::Body,
        };
        let side = if local.x.abs() > 0.04 {
            local.x.signum()
        } else if push.x.abs() > 0.2 {
            // Knocked to the right: struck on the left.
            -push.x.signum()
        } else if matches!(b.kind, DamageKind::Slash | DamageKind::Blunt) {
            // A right-handed cut from in front lands on the left.
            -1.0
        } else {
            0.0
        };
        Hurt {
            since: now,
            zone,
            side,
            push,
            strength: (b.amount / 25.0).clamp(0.5, 1.4),
        }
    }
}

/// Downed: `t` s ago, falling back along `yaw` (owner frame; 0 straight back); the leg that
/// folds out to the side as it lands (the other lies straight) and the arm flung out (the
/// other lies along the body).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fall {
    t: f32,
    yaw: f32,
    bent: usize,
    flung: usize,
}

impl Fall {
    /// How it lies, as its squad mates see it.
    fn lie(&self) -> Lie {
        Lie {
            yaw: self.yaw,
            bent: self.bent,
            flung: self.flung,
        }
    }
}

/// A downed fighter as a mate reviving it sees it (owner frame): the front of its chest over
/// the breastbone (where the hands go), and where the knees go beside it, from its left and
/// from its right (see [`Lie::kneel_by`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Lying {
    pub chest: Vec3,
    pub knees: [Vec3; 2],
}

impl Lying {
    /// Lying as `s`, fallen as `f`.
    fn of(s: &Skeleton, f: Fall) -> Lying {
        let chest = s.chest.at(STERNUM);
        let knees =
            [0, 1].map(|side| v3(chest.x, 0.0, chest.z) + ry(f.yaw) * f.lie().kneel_by(side));
        Lying { chest, knees }
    }
}

/// Another fighter's state, as a defender reads its attacker.
#[derive(Debug, Clone, Copy)]
pub(super) struct Foe {
    pub at: DVec3,
    /// Telegraphing a heavy blow.
    pub heavy: bool,
    pub dashing: bool,
    pub ranged: bool,
    /// Its weapon's damage per blow.
    pub damage: f32,
}

impl Foe {
    pub fn of(
        t: &Transform,
        combat: Option<&NpcCombat>,
        loco: Option<&LocomotionState>,
        weapon: Option<&NpcWeapon>,
    ) -> Self {
        Self {
            at: t.translation,
            heavy: combat
                .is_some_and(|c| matches!(c.phase, AttackPhase::Windup { heavy: true, .. })),
            dashing: loco.is_some_and(|l| l.mode == LocomotionMode::Dash),
            ranged: weapon.is_some_and(|w| w.style == WeaponStyle::Ranged),
            damage: weapon.map_or(0.0, |w| w.damage),
        }
    }
}

/// What a fighter's pose sees beyond its owner this step.
pub(super) struct Seen<'a> {
    pub me: Entity,
    /// 0..1, the same every step for this fighter (from its seed): how it lands differs.
    pub vary: f32,
    /// Where the owner stands.
    pub at: DVec3,
    /// The attacks telegraphed this step: attacker, target and kind.
    pub attacks: &'a [(Entity, Option<Entity>, AttackKind)],
    /// The blows that landed this step.
    pub blows: &'a [DamageEvent],
    /// How long the owner's dash takes (s).
    pub dash_secs: f32,
    /// Another fighter's state.
    pub foe: &'a dyn Fn(Entity) -> Option<Foe>,
    /// What the owner knows (the foes it has seen, the last noise it heard).
    pub memory: Option<&'a NpcMemory>,
    /// Reviving a downed mate: where its chest lies as last posed (the front over the
    /// breastbone), and where the knees go beside it.
    pub patient: Option<(DVec3, DVec3)>,
}

/// Everything the pose depends on.
#[derive(Clone, Copy)]
struct Input {
    now: f64,
    /// Where the feet are, and the gait they move in.
    feet: [Plant; 2],
    gait: Gait,
    /// Acceleration in the owner's frame (x, z; m/s², smoothed): the body leans into it.
    accel: Vec2,
    crouch: bool,
    kind: Kind,
    act: Act,
    /// How long this telegraph lasts (s).
    windup: f32,
    /// Sidestep: −1 (left) .. 1 (right), how far into it.
    dodge: f32,
    fighting: bool,
    /// A blow taken, and how long ago (s).
    hurt: Option<(f32, Hurt)>,
    /// A dash under way: how far through it (0..1), which way (owner frame), and how far the
    /// body is drawn behind the owner as it pushes off (m).
    dash: Option<(f32, Vec2, f32)>,
    /// Downed, and falling.
    fall: Option<Fall>,
    /// How far the hips lag behind the way the body faces, turning on the spot (rad).
    behind: f32,
    /// The last shot or loose.
    shot: Option<Shot>,
    /// Where the eyes and the head look.
    look: Look,
    /// How far the upper lids are shut and the jaw open (0..1).
    lids: f32,
    jaw: f32,
    /// The breath: in (1) and out (−1), deeper when winded.
    breath: f32,
    /// Badly hurt: where the wound is (a leg, or the body), on which side (−1 the left, 1
    /// the right), and how badly it tells (0..1).
    wound: Option<(Zone, f32, f32)>,
    /// How far the weapon has come up from its carry at ease (0) to what the act has it doing
    /// (1), and how far the carry at ease has come down to the low ready of a wary fighter.
    up: f32,
    wary: f32,
    /// How far the stance is from at ease (0) to on guard (1).
    poise: f32,
    /// Getting up off the ground after being revived.
    rise: Option<Rise>,
    /// How far down on one knee it is (1), getting up.
    rising: f32,
    /// Kneeling over a downed squad mate to revive it.
    tend: Option<Tend>,
    /// Per-fighter offset for idle motions.
    seed: f64,
}

/// Reviving a downed squad mate: how far into it (0 standing, 1 down on both knees beside
/// it, the weapon set down or held aside and the hands on its chest), where its chest is and
/// where the knees go (owner frame).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tend {
    k: f32,
    at: Vec3,
    knees: Vec3,
}

/// How long going down beside a downed mate takes, the weapon set down and the hands on it
/// (s: as long as the brain has it kneel before the revive counts), and getting up from it
/// again (about as long as the brain keeps it where it is: see
/// [`NpcBrain::REVIVE_STAND_SECS`]).
const TEND_DOWN: f32 = NpcBrain::REVIVE_KNEEL_SECS;
const TEND_UP: f32 = 1.0;
/// How high its eyes are, down on its knees beside a downed mate (m).
const TEND_EYES: f32 = 1.05;
/// The hands press on the chest this often (Hz) and this deep (m).
const PRESS_HZ: f32 = 1.6;
const PRESS: f32 = 0.03;
/// The front of the chest over the breastbone, where the hands go (chest frame).
const STERNUM: Vec3 = Vec3::new(0.0, 0.22, -0.14);
/// How far the body may kneel off the owner's place to get its knees where they go (m): as
/// far as a reviver may be off its spot and stay down (see [`crate::brain::REVIVE_STAY`]),
/// and a little over for how far off it reckons where the chest lies (see [`Lie::sternum`]).
const KNEEL_REACH: f32 = 1.15;

/// Getting up, `t` s after being revived, from lying as it fell (see [`Fall`]) from what it
/// was doing then (`was`, at `since` on its clock, the feet where they stood; kneeling by a
/// mate as `knelt`, if it was).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rise {
    t: f32,
    fall: Fall,
    was: Act,
    since: f64,
    feet: [Plant; 2],
    knelt: Option<Tend>,
}

/// How badly a wound (see [`Input`]) hobbles the gait (0..1; one to the body less than one
/// to a leg).
fn hobble(wound: Option<(Zone, f32, f32)>) -> f32 {
    match wound {
        Some((Zone::Legs, _, w)) => w,
        Some((_, _, w)) => 0.6 * w,
        None => 0.0,
    }
}

/// Getting up: how long from lying to down on one knee, the weapon picked up, and from there
/// up on guard (s).
const RISE_KNEEL: f32 = 1.0;
const RISE_UP: f32 = 0.55;

/// How long getting up takes with weapon `kind` (an archer nocks an arrow on the way up).
fn rise_secs(kind: Kind) -> f32 {
    RISE_KNEEL + rise_up(kind)
}

fn rise_up(kind: Kind) -> f32 {
    if kind == Kind::Bow {
        readying(kind).0
    } else {
        RISE_UP
    }
}

/// How long the weapon takes to come up on guard from its carry at ease (s), and to go down
/// again: an archer takes as long as nocking an arrow takes (and putting it back).
fn readying(kind: Kind) -> (f32, f32) {
    if kind == Kind::Bow {
        (1.3, 1.3)
    } else {
        (0.45, 0.6)
    }
}

/// How long the weapon takes to come up for a blow of its own, or against one (s): at once,
/// an archer nocking in a hurry.
fn hurried(kind: Kind) -> f32 {
    if kind == Kind::Bow { 0.6 } else { 0.15 }
}

/// Where a fighter looks (owner frame: yaw, to the left positive, and pitch, up positive;
/// rad): the eyes, and the head a beat behind them; how much it is after something (0, at
/// ease, it looks about idly); and at ease, where the eyes wander off the head's line.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Look {
    eyes: Vec2,
    head: Vec2,
    focus: f32,
    dart: Vec2,
}

/// How far the head turns on the neck (rad) before the shoulders turn with it, and how much
/// more the shoulders give.
const NECK_YAW: f32 = 1.2;
const TORSO_YAW: f32 = 0.45;
/// How far the head tips up and down on the neck (rad), and the eyes turn in the head.
const NECK_PITCH: f32 = 0.6;
const EYES_YAW: f32 = 0.5;
const EYES_PITCH: (f32, f32) = (-0.35, 0.3);

/// What a fighter's eyes are doing, as the pose remembers it (owner frame, NPC clock; nothing
/// in the world).
#[derive(Debug, Clone, Copy, Default)]
struct Gaze {
    /// Where the eyes look and the head points (see [`Look`]), and how fast the head turns.
    eyes: Vec2,
    head: Vec2,
    spin: Vec2,
    focus: f32,
    /// A glance away, at another fighter (or at the latest noise), until when.
    glance: Option<(Option<Entity>, f64)>,
    /// When it next looks round for another foe close by (or, reviving a mate, up at the
    /// threat), and whether it is reviving one.
    next_check: f64,
    tending: bool,
    /// The last noise it turned to.
    noise: f64,
    /// At ease, where the eyes rest off the head's line, and when they move next.
    dart: Vec2,
    next_dart: f64,
    /// Blinks: when the last began, when the next does, and how many so far.
    blink: f64,
    next_blink: f64,
    blinks: u32,
    /// How winded it is (0..1: a sprint leaves it breathing hard, and it gets its breath back
    /// slowly), and where in a breath it is (rad).
    winded: f32,
    breath: f64,
}

/// A number in 0..1 that differs from one `n` to the next and from fighter to fighter.
fn hash(seed: f64, n: u32) -> f32 {
    ((seed * 78.233 + f64::from(n) * 12.9898).sin() * 43_758.545)
        .fract()
        .abs() as f32
}

impl Gaze {
    /// Turn the eyes to `want` (none: at ease) `dt` s on, the body having turned by `turned`:
    /// the eyes go at once (a flick), the head after them, a beat behind.
    fn step(&mut self, now: f64, dt: f32, turned: f32, want: Option<Vec2>, seed: f64) -> Look {
        // Kept in the owner's frame: as the body turns, they turn back by as much.
        let back = Vec2::new(turned, 0.0);
        self.eyes -= back;
        self.head -= back;
        let focus = if want.is_some() { 1.0 } else { 0.0 };
        self.focus += (focus - self.focus) * (dt * 4.0).min(1.0);
        if now >= self.next_dart {
            let n = self.blinks.wrapping_add((now * 7.0) as u32);
            self.dart = Vec2::new(0.3 * hash(seed, n) - 0.15, 0.12 * hash(seed, n + 1) - 0.06);
            self.next_dart = now + 0.6 + 1.8 * f64::from(hash(seed, n + 2));
        }
        let to = want.unwrap_or(Vec2::ZERO);
        self.eyes += (to - self.eyes) * (dt / 0.035).min(1.0);
        // The head: a critically damped spring after the eyes' target.
        const W: f32 = 11.0;
        let steps = (dt / (1.0 / 120.0)).ceil().max(1.0);
        let h = dt / steps;
        for _ in 0..steps as usize {
            let a = (to - self.head) * W * W - self.spin * 2.0 * W;
            self.spin += a * h;
            self.head += self.spin * h;
        }
        Look {
            eyes: self.eyes,
            head: self.head,
            focus: self.focus,
            dart: self.dart,
        }
    }
}

/// The last shot or loose, as the pose follows it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shot {
    /// How long ago it was, and how long after it the weapon is ready again (s).
    since: f32,
    ready: f32,
    /// The magazine changes after it.
    reload: bool,
}

/// The posed skeleton (owner space). The pelvis sits between the hip joints, the chest on the
/// waist and the head on the neck; each limb segment on the joint it hangs from (its −Y along
/// the segment, +X the hinge), the hand on the wrist, the foot on the ankle (toes −Z).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Skeleton {
    pub(super) pelvis: Bone,
    pub(super) chest: Bone,
    pub(super) head: Bone,
    /// Left then right.
    pub(super) thigh: [Bone; 2],
    pub(super) shin: [Bone; 2],
    pub(super) foot: [Bone; 2],
    pub(super) upper: [Bone; 2],
    pub(super) fore: [Bone; 2],
    pub(super) hand: [Bone; 2],
    tabard: [Bone; 2],
    /// Where each wrist was asked to go.
    want: [Vec3; 2],
    /// Blade/hilt, carbine or bow riser.
    weapon: Bone,
    shield: Bone,
    /// Bow: limb bend (radians), string pulled past brace height (m), arrow on the string.
    bend: f32,
    draw: f32,
    nocked: bool,
    /// The magazine out of the carbine, or the next arrow in hand.
    held: Option<Bone>,
    /// The eyes' turn in the head (left, right); how far the upper lids are shut and the jaw
    /// open (0..1).
    pub(super) eyes: [Quat; 2],
    lids: f32,
    jaw: f32,
}

/// Where a hand goes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hand {
    /// On a grip: its center and the hand's rotation.
    Grip(Vec3, Quat),
    /// The wrist at a point, the hand in line with the forearm.
    Free(Vec3),
    /// Reaching for a grip, `w` of the way (0..1): the wrist at a point, the hand turned that
    /// far from in line with the forearm toward the grip's rotation.
    Near(Vec3, Quat, f32),
    /// Swinging with the gait.
    Swing,
}

/// The upper body's targets (owner space).
struct Arms {
    weapon: Bone,
    shield: Bone,
    hand: [Hand; 2],
    pole: [Vec3; 2],
    draw: f32,
    nocked: bool,
    /// Something in hand off the weapon: the magazine changing, the next arrow.
    held: Option<Bone>,
}

/// Foot stance: per foot (left, right) its x, how far forward, and its yaw; how far the hips
/// sink; pelvis yaw and the chest's twist and lean on top of it.
struct Stance {
    feet: [(f32, f32, f32); 2],
    drop: f32,
    yaw: f32,
    twist: f32,
    lean: f32,
}

/// The stance `w` of the way from at ease to on guard (its feet as on guard).
fn poised(kind: Kind, w: f32) -> Stance {
    let (a, b) = (stance(kind, false), stance(kind, true));
    Stance {
        feet: b.feet,
        drop: lerp(a.drop, b.drop, w),
        yaw: lerp(a.yaw, b.yaw, w),
        twist: lerp(a.twist, b.twist, w),
        lean: lerp(a.lean, b.lean, w),
    }
}

fn stance(kind: Kind, ready: bool) -> Stance {
    if !ready {
        return Stance {
            feet: [(-0.11, 0.04, 0.12), (0.11, -0.03, -0.14)],
            drop: 0.015,
            yaw: 0.0,
            twist: 0.0,
            lean: 0.03,
        };
    }
    match kind {
        // Side-on to the target, left shoulder leading.
        Kind::Bow => Stance {
            feet: [(-0.05, 0.19, -1.15), (0.07, -0.19, -1.35)],
            drop: 0.03,
            yaw: -0.8,
            twist: -0.3,
            lean: 0.02,
        },
        Kind::Rifle => Stance {
            feet: [(-0.12, 0.15, -0.2), (0.15, -0.17, -0.85)],
            drop: 0.05,
            yaw: -0.45,
            twist: -0.1,
            lean: 0.12,
        },
        _ => Stance {
            feet: [(-0.13, 0.2, -0.1), (0.16, -0.2, -0.85)],
            drop: 0.09,
            yaw: -0.4,
            twist: 0.22,
            lean: 0.1,
        },
    }
}

/// Where each foot stands in the stance (owner space x, z; and yaw).
fn rest_feet(kind: Kind, ready: bool) -> [(Vec2, f32); 2] {
    stance(kind, ready)
        .feet
        .map(|(x, f, yaw)| (Vec2::new(x, -f), yaw))
}

// ============================================================================ the gait

/// Below this speed (m/s) the feet step only to stay under the body, not to a rhythm.
const MOVING: f32 = 0.25;

/// How far up on the ball of the foot (radians) a stride pushes off.
const TOE_OFF: f32 = 0.85;

/// How far behind the hips (m) a foot can push off before the leg runs out.
const MAX_BEHIND: f32 = 0.6;

/// Where a foot is (owner space): the ground point under the ankle when flat (x, z), how high
/// it is lifted, its yaw and its pitch (toe up positive).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Plant {
    at: Vec2,
    lift: f32,
    yaw: f32,
    pitch: f32,
}

/// The gait at a velocity (owner space x, z; m/s): longer strides and quicker steps the
/// faster it goes, shorter ones sideways. Each foot is down for `beta` of a cycle (two steps)
/// and lands `ahead` of its spot, so that it moves back under the body exactly as fast as the
/// ground does.
#[derive(Debug, Clone, Copy, Default)]
struct Gait {
    vel: Vec2,
    speed: f32,
    /// Direction of travel (zero standing).
    dir: Vec2,
    /// 0 walking .. 1 running.
    run: f32,
    /// 0 standing .. 1 striding out.
    moving: f32,
    beta: f32,
    /// Cycle length (m) and rate (cycles/s).
    len: f32,
    rate: f32,
    ahead: f32,
    lift: f32,
    /// Where in the cycle (0..1; 0 the left foot's strike, ½ the right's).
    cycle: f32,
}

impl Gait {
    /// On guard (`ready`) and short of a run, a fighter shuffles in short steps that keep its
    /// stance; sideways, the steps are short and quick.
    fn new(vel: Vec2, cycle: f32, ready: bool) -> Self {
        let speed = vel.length();
        let dir = if speed > 1e-3 {
            vel / speed
        } else {
            Vec2::ZERO
        };
        let run = step(1.8, 3.6, speed);
        // Sprinting, the feet are down for less of the stride (or the legs couldn't reach).
        let sprint = step(4.5, 7.0, speed);
        let side = dir.x.abs();
        let guard = if ready {
            1.0 - step(1.5, 3.5, speed)
        } else {
            0.0
        };
        let beta = lerp(lerp(0.62, 0.31, run) - 0.07 * sprint, 0.5, guard) - 0.1 * side;
        let len = lerp(0.85 + 0.42 * speed, 0.5 + 0.22 * speed, guard)
            * lerp(1.0, lerp(0.5, 0.75, guard), side);
        Self {
            vel,
            speed,
            dir,
            run,
            moving: step(0.05, 0.7, speed),
            beta,
            len,
            rate: speed / len,
            ahead: beta * len * lerp(0.42, 0.4, run),
            lift: (lerp(0.07, 0.26, run) + 0.08 * sprint) * lerp(1.0, 0.6, side),
            cycle,
        }
    }

    /// Hobbled by a wound (`w`, 0..1): shorter strides at the same speed (so quicker ones),
    /// the feet lifted less (the bad leg no less than the other: lower, they would drag).
    fn hobbled(mut self, w: f32) -> Self {
        if w <= 0.0 {
            return self;
        }
        self.len *= 1.0 - 0.3 * w;
        self.rate = self.speed / self.len;
        self.ahead = self.beta * self.len * lerp(0.42, 0.4, self.run);
        self.lift *= 1.0 - 0.2 * w;
        self
    }

    /// Where foot `k` rests mid-stride: its place in the stance (`rest`) giving way to walking
    /// side by side as the gait picks up (a fighter on guard keeps its stance at a shuffle),
    /// wider when stepping sideways. With the yaw.
    fn spot(&self, k: usize, rest: (Vec2, f32), ready: bool) -> (Vec2, f32) {
        let sgn = if k == 0 { -1.0 } else { 1.0 };
        let walk = if ready {
            self.moving * step(1.5, 3.5, self.speed)
        } else {
            self.moving
        };
        let x = lerp(rest.0.x, sgn * 0.1, walk) + sgn * 0.07 * self.dir.x.abs() * self.moving;
        let z = lerp(rest.0.y, 0.0, walk);
        (Vec2::new(x, z), lerp(rest.1, -sgn * 0.08, walk))
    }

    /// Where foot `k` lands next.
    fn target(&self, k: usize, rest: (Vec2, f32), ready: bool) -> (Vec2, f32) {
        let (spot, yaw) = self.spot(k, rest, ready);
        if self.speed < MOVING {
            (spot, yaw)
        } else {
            (spot + self.dir * self.ahead, yaw)
        }
    }

    /// How much the feet roll heel to toe: walking or running ahead (reversed backward), not
    /// stepping sideways.
    fn roll(&self) -> f32 {
        let r = -self.dir.y * self.moving;
        if r < 0.0 { 0.6 * r } else { r }
    }

    /// Foot pitch `u` of the way through its time down: rolling off the heel, flat, then up on
    /// the ball.
    fn stance_pitch(&self, u: f32) -> f32 {
        let heel = 0.3 * (1.0 - self.run);
        (heel * (1.0 - u / 0.2).max(0.0) - TOE_OFF * step(0.6, 1.0, u)) * self.roll()
    }

    /// Foot pitch `u` of the way through a stride in the air: toe down after push-off, heel
    /// down to land.
    fn swing_pitch(&self, u: f32) -> f32 {
        let heel = 0.3 * (1.0 - self.run);
        (-TOE_OFF * (1.0 - step(0.0, 0.35, u)) + heel * step(0.7, 1.0, u)) * self.roll()
    }

    /// How far along (0..1) and how high a foot in the air is, `u` of the way through its
    /// swing (`h` the top): a walking foot swings through evenly; a running one is whipped
    /// through early and high, staying low as it pushes off and as it reaches out to settle
    /// onto the ground (so that in the moment both are up, one is still close to it). It
    /// leaves the ground and lands at rest (in the world) either way.
    fn swing(&self, u: f32, h: f32) -> (f32, f32) {
        let early = 1.0 - (1.0 - u).powi(3) * (1.0 + 3.0 * u);
        let along = lerp(ease(u), early, self.run);
        let arc = (PI * u).sin();
        (along, h * lerp(arc, arc * arc, self.run))
    }

    /// The feet of a steady gait at its point in the cycle (standing: on their spots).
    fn steady(&self, rest: [(Vec2, f32); 2], ready: bool) -> [Plant; 2] {
        std::array::from_fn(|k| {
            let (land, yaw) = self.target(k, rest[k], ready);
            if self.speed < MOVING {
                return Plant {
                    at: land,
                    yaw,
                    ..Default::default()
                };
            }
            let off = land - self.dir * self.beta * self.len;
            let t = (self.cycle + 0.5 * k as f32).rem_euclid(1.0);
            if t < self.beta {
                let u = t / self.beta;
                Plant {
                    at: land.lerp(off, u),
                    lift: 0.0,
                    yaw,
                    pitch: self.stance_pitch(u),
                }
            } else {
                let u = (t - self.beta) / (1.0 - self.beta);
                let (along, lift) = self.swing(u, self.lift);
                Plant {
                    at: off.lerp(land, along),
                    lift,
                    yaw,
                    pitch: self.swing_pitch(u),
                }
            }
        })
    }
}

/// Smoothstep on 0..1.
fn ease(u: f32) -> f32 {
    step(0.0, 1.0, u)
}

/// An angle brought into −π..π.
fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Which way a foot turned `yaw` points (owner frame, x and z).
fn toes(yaw: f32) -> Vec2 {
    Vec2::new(-yaw.sin(), -yaw.cos())
}

/// A point on the ground (owner frame, x and z) turned `yaw` about the owner.
fn yawed(p: Vec2, yaw: f32) -> Vec2 {
    let q = ry(yaw) * Vec3::new(p.x, 0.0, p.y);
    Vec2::new(q.x, q.z)
}

/// The most the hips lag behind the heading, turning on the spot (rad).
const BEHIND: f32 = 1.3;

/// How far the ball of the foot (what it pivots on, heel up) is ahead of the ankle (m).
const BALL: f32 = 0.12;
/// How far out of line a standing foot turns before it pivots on its ball (rad), and how far
/// up on the ball it comes (rad of pitch).
const PIVOT: f32 = 0.15;
const PIVOT_PITCH: f32 = 0.32;

/// One foot's memory: where it stands on the ground (owner space), or stood when it lifted.
#[derive(Debug, Clone, Copy, Default)]
struct Foot {
    at: Vec2,
    yaw: f32,
    /// In the air: since when (NPC time), for how long and how high.
    air: Option<(f64, f32, f32)>,
    /// The gait lap in which it last lifted (a foot lifts once a lap).
    lap: i64,
    /// How far up on its ball it is, pivoting round on it (0..1).
    heel: f32,
}

/// A dash under way, as the pose follows it.
#[derive(Debug, Clone, Copy, Default)]
struct Dash {
    /// When it began (NPC clock), which way (owner frame), and how far it has gone (m).
    t0: f64,
    dir: Vec2,
    went: f32,
    /// Which feet have stepped off.
    stepped: [bool; 2],
}

/// A dash's progress for the feet: how far through it (0..1), which way, how far it goes in
/// all (estimated from how far it has gone), how far the body lags behind the owner, and how
/// long it takes (s).
#[derive(Debug, Clone, Copy)]
struct Carry {
    u: f32,
    dir: Vec2,
    dist: f32,
    lag: f32,
    secs: f32,
}

/// What a fighter's pose remembers from one step to the next: its own gait clock, where its
/// feet stand, and when its guard went up, its dodge or dash began, its telegraph began and
/// ended, or it was hit or went down. Purely visual, and kept in the owner's frame and its
/// clock (no world points: a world shift doesn't disturb it; a jump in heading, as a turned
/// world makes, starts it afresh).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Motion {
    posed: bool,
    /// When it was last posed (NPC clock) and the heading then.
    last: f64,
    yaw: f32,
    /// Gait cycles walked (the fraction is where in the cycle).
    cycles: f64,
    feet: [Foot; 2],
    /// The feet as last posed (downed, they stay there as the body falls).
    plants: [Plant; 2],
    /// Velocity last step (owner frame) and the smoothed acceleration.
    vel: Vec2,
    accel: Vec2,
    /// The guard up (block or parry): its end, when it rose, when the blow lands on it.
    guard: (f64, f64, f64),
    /// The line of the blow the guard meets, and whose blow it is.
    line: Line,
    meets: Option<Entity>,
    /// The last attack telegraphed at this fighter: by whom, and how.
    threat: Option<(Entity, AttackKind)>,
    /// The dodge under way: its end, its start, and its side (−1 left, 1 right, 0 not yet).
    dodge: (f64, f64, f32),
    dash: Option<Dash>,
    /// The telegraph under way: when it began, when it lands, and whether it is heavy; how
    /// long the last one took (s).
    swing: Option<(f64, f64, bool)>,
    swung: f32,
    /// The recovery under way: when it began and when it ends.
    recover: Option<(f64, f64)>,
    /// A blow called off: when, how far through its telegraph, and whether it was heavy.
    check: Option<(f64, f32, bool)>,
    /// The last blow taken.
    hurt: Option<Hurt>,
    /// The feet were walking to a rhythm last step (not just standing).
    striding: bool,
    /// How far the hips (and the stance the feet step round to) lag behind the heading,
    /// turning on the spot (rad).
    behind: f32,
    /// The last shot or loose: when (NPC clock), when the weapon is ready again, and whether
    /// the magazine changes after it; how many have been fired.
    fired: Option<(f64, f64, bool)>,
    shots: u32,
    /// What hangs off the body, swinging after it.
    sway: Sway,
    /// Where the eyes look, blinks and the breath.
    gaze: Gaze,
    /// The blow telegraphed or recovered from is a heavy one.
    heavy: bool,
    down: bool,
    /// Downed: when it went down, and what it was doing then (the pose it falls from: how it
    /// knelt by a mate, if it was).
    fell: Option<(f64, Act, Option<Tend>)>,
    /// Downed, how it lies (to get up from).
    lay: Option<Rise>,
    /// Getting up: when it was revived, and how it lay.
    rise: Option<(f64, Rise)>,
    /// The wound it carries (the last blow to the body or a leg: where, and on which side),
    /// until it is whole again; and how badly it hobbles the gait.
    wound: Option<(Zone, f32)>,
    hobble: f32,
    /// How far the weapon is up from the carry at ease, and how far the carry is down to the
    /// low ready of a wary fighter (0..1; see [`Input`]).
    up: f32,
    wary: f32,
    /// Reviving a downed mate: how far down beside it (0..1, see [`Tend`]), where its chest
    /// is and where the knees go (owner frame).
    tend: f32,
    tend_at: Vec3,
    tend_knees: Vec3,
    /// Downed, how it lies as last posed (where a mate's hands and knees go to revive it).
    lying: Option<Lying>,
    /// The skeleton and the face as last posed.
    body: Option<(Skeleton, Expression)>,
}

impl Motion {
    /// Downed, how it lies as last posed (owner frame; see [`Motion::lying`]).
    pub(super) fn lying(&self) -> Option<Lying> {
        self.lying.filter(|_| self.posed && self.down)
    }

    /// The skeleton and the face as last posed (none before the first pose).
    pub(super) fn body(&self) -> Option<&(Skeleton, Expression)> {
        self.body.as_ref()
    }

    /// Set the feet down as a steady gait has them.
    fn reset_feet(&mut self, now: f64, g: &Gait, rest: [(Vec2, f32); 2], ready: bool) {
        self.striding = g.speed >= MOVING;
        for (k, p) in g.steady(rest, ready).into_iter().enumerate() {
            let c = self.cycles + 0.5 * k as f64;
            let (t, lap) = (c.rem_euclid(1.0) as f32, c.floor() as i64);
            let air = g.speed >= MOVING && t >= g.beta;
            let (at, air) = if air {
                let u = (t - g.beta) / (1.0 - g.beta);
                let dur = (1.0 - g.beta) / g.rate.max(1e-3);
                let (land, _) = g.target(k, rest[k], ready);
                let off = land - g.dir * g.beta * g.len;
                (off, Some((now - (u * dur) as f64, dur, g.lift)))
            } else {
                (p.at, None)
            };
            self.feet[k] = Foot {
                at,
                yaw: p.yaw,
                air,
                lap: if air.is_some() { lap } else { lap - 1 },
                heel: 0.0,
            };
        }
    }

    /// Move the feet on `dt` s, the body having moved at the gait's velocity and turned by
    /// `turned`. A foot on the ground stays where it is on the ground; one in the air swings
    /// to where the gait (or, standing, the stance) wants it. A foot lifts when the gait's
    /// cycle comes round to it, or, standing, when it is out of place and the other one is
    /// down. In a dash (`carry`) the leading foot steps out at once to where it will stand
    /// as the dash ends, while the other drives the body off, up on its ball, until the body
    /// has left it behind, then swings through to land just after.
    #[allow(clippy::too_many_arguments)]
    fn step_feet(
        &mut self,
        now: f64,
        dt: f32,
        turned: f32,
        g: &Gait,
        rest: [(Vec2, f32); 2],
        ready: bool,
        carry: Option<Carry>,
    ) -> [Plant; 2] {
        let turn = ry(-turned);
        let moved = Vec3::new(g.vel.x, 0.0, g.vel.y) * dt;
        for f in &mut self.feet {
            let p = turn * Vec3::new(f.at.x, 0.0, f.at.y) - moved;
            f.at = Vec2::new(p.x, p.z);
            f.yaw = wrap(f.yaw - turned);
        }
        let target = |k: usize| g.target(k, rest[k], ready);
        if carry.is_some() || g.speed >= MOVING {
            for f in &mut self.feet {
                f.heel = 0.0;
            }
        }
        if let Some(c) = carry {
            let lead = if (rest[1].0 - rest[0].0).dot(c.dir) > 0.0 {
                1
            } else {
                0
            };
            // Where each foot ends up (its place in the stance, as far on as the dash still
            // goes), and where the hips are (drawn behind the owner).
            let left = c.dir * c.dist * (1.0 - c.u) * (1.0 - c.u);
            let hips = -c.dir * c.lag;
            let mut stepped = self.dash.map_or([true; 2], |d| d.stepped);
            let plants = std::array::from_fn(|k| {
                let end = rest[k].0 + left;
                let f = &mut self.feet[k];
                if let Some((t0, dur, _)) = f.air
                    && now >= t0 + dur as f64
                {
                    *f = Foot {
                        at: end,
                        yaw: rest[k].1,
                        air: None,
                        heel: 0.0,
                        ..*f
                    };
                }
                if f.air.is_none()
                    && !stepped[k]
                    && c.u < 1.0
                    && (k == lead || (hips - f.at).dot(c.dir) > 0.45 || c.u > 0.45)
                {
                    let land = if k == lead { 0.85 } else { 1.12 };
                    let h = if k == lead { 0.08 } else { 0.14 };
                    f.air = Some((now, ((land - c.u) * c.secs).max(0.03), h));
                    stepped[k] = true;
                }
                match f.air {
                    Some((t0, dur, h)) => {
                        let u = ((now - t0) as f32 / dur).clamp(0.0, 1.0);
                        Plant {
                            at: f.at.lerp(end, ease(u)),
                            lift: h * (PI * u).sin(),
                            yaw: f.yaw + wrap(rest[k].1 - f.yaw) * ease(u),
                            pitch: -0.5 * (1.0 - step(0.0, 0.4, u)),
                        }
                    }
                    None => Plant {
                        at: f.at,
                        lift: 0.0,
                        yaw: f.yaw,
                        pitch: if stepped[k] {
                            0.0
                        } else {
                            -0.6 * step(0.0, 0.2, c.u)
                        },
                    },
                }
            });
            if let Some(d) = &mut self.dash {
                d.stepped = stepped;
            }
            return plants;
        }
        let moving = g.speed >= MOVING;
        // A foot in the air swings to where its target will be when it lands (`left` s from
        // now), the body going on as it goes now, so that it comes down where it will stay
        // instead of skating after the body.
        let spin = if dt > 0.0 { turned / dt } else { 0.0 };
        let landing = |k: usize, left: f32| {
            let (to, yaw) = target(k);
            let at = ry(spin * left) * Vec3::new(to.x + g.vel.x * left, 0.0, to.y + g.vel.y * left);
            (Vec2::new(at.x, at.z), yaw + spin * left)
        };
        // Land (where the target was at the moment, a frame's fraction ago).
        for k in 0..2 {
            if let Some((t0, dur, _)) = self.feet[k].air
                && now >= t0 + dur as f64
            {
                let (at, yaw) = landing(k, (t0 + dur as f64 - now) as f32);
                self.feet[k] = Foot {
                    at,
                    yaw,
                    air: None,
                    heel: 0.0,
                    ..self.feet[k]
                };
            }
        }
        let started = moving && !self.striding;
        self.striding = moving;
        if moving && self.feet.iter().all(|f| f.air.is_none()) {
            // How far each foot has fallen behind where it would land.
            let back = |f: &Foot, k: usize| (target(k).0 - f.at).dot(g.dir);
            let back = [0, 1].map(|k| back(&self.feet[k], k));
            let len = g.len.max(0.1);
            if started {
                // Setting off: the rhythm picks up from where the feet stand, the foot further
                // ahead that far into its time down, the other stepping off.
                let b = if back[0] <= back[1] { 0 } else { 1 };
                let t = (back[b] / len).clamp(0.0, g.beta - 0.02);
                let c = self.cycles + 0.5 * b as f64;
                self.cycles += f64::from(t) - c.rem_euclid(1.0);
                for f in &mut self.feet {
                    f.lap = i64::MIN;
                }
            } else if let Some(k) =
                (0..2).find(|&k| back[k] - g.ahead > (g.beta * len - g.ahead + 0.1).min(MAX_BEHIND))
            {
                // Fallen behind (changing speed, turning), or as far back as a leg goes: it
                // steps off now, and the rhythm with it.
                let c = self.cycles + 0.5 * k as f64;
                let t = c.rem_euclid(1.0) as f32;
                if t < g.beta {
                    self.cycles += f64::from(g.beta - t);
                }
                self.feet[k].lap = i64::MIN;
            }
        }
        // Standing, a foot turned out of line with the body (the body turning on the spot)
        // comes up on its ball and pivots round on it toward its place, the ball staying where
        // it is on the ground; the heel comes down again as it gets there, or it steps.
        if !moving {
            for k in 0..2 {
                let (_, yaw) = target(k);
                let f = &mut self.feet[k];
                if f.air.is_some() {
                    continue;
                }
                let err = wrap(yaw - f.yaw);
                let held = if f.heel > 0.05 { 0.03 } else { PIVOT };
                let want = if err.abs() > held { 1.0 } else { 0.0 };
                f.heel += (want - f.heel) * (dt / 0.05).min(1.0);
                // Only once the heel is clear of the ground.
                let turn = (6.0 * dt * step(0.2, 0.5, f.heel)).min(err.abs()) * err.signum();
                if turn != 0.0 {
                    let ball = f.at + toes(f.yaw) * BALL;
                    f.yaw += turn;
                    f.at = ball - toes(f.yaw) * BALL;
                }
            }
        }
        // Lift, the foot further out of place first.
        let off = |f: &Foot, k: usize| {
            let (at, yaw) = target(k);
            (f.at - at).length().max(wrap(f.yaw - yaw).abs() * 0.2)
        };
        let order = if off(&self.feet[1], 1) > off(&self.feet[0], 0) {
            [1, 0]
        } else {
            [0, 1]
        };
        for k in order {
            let other_down = self.feet[1 - k].air.is_none();
            let out = off(&self.feet[k], k);
            let c = self.cycles + 0.5 * k as f64;
            let (t, lap) = (c.rem_euclid(1.0) as f32, c.floor() as i64);
            let f = &mut self.feet[k];
            if f.air.is_some() {
                continue;
            }
            if moving {
                if t >= g.beta && f.lap != lap {
                    let dur = ((1.0 - t) / g.rate).clamp(0.12, 0.45);
                    f.air = Some((now, dur, g.lift));
                    f.lap = lap;
                }
            } else if other_down && out > 0.08 {
                let dur = if ready { 0.2 } else { 0.28 };
                f.air = Some((now, dur, 0.05));
            }
        }
        std::array::from_fn(|k| {
            let f = self.feet[k];
            let t = (self.cycles + 0.5 * k as f64).rem_euclid(1.0) as f32;
            match f.air {
                Some((t0, dur, h)) => {
                    let u = ((now - t0) as f32 / dur).clamp(0.0, 1.0);
                    let (to, yaw) = landing(k, (t0 + dur as f64 - now).max(0.0) as f32);
                    let (along, lift) = if moving {
                        g.swing(u, h)
                    } else {
                        (ease(u), h * (PI * u).sin())
                    };
                    Plant {
                        at: f.at.lerp(to, along),
                        lift,
                        yaw: f.yaw + wrap(yaw - f.yaw) * ease(u),
                        pitch: if moving {
                            g.swing_pitch(u)
                        } else {
                            // Off the ball it was pivoting on, flattening out.
                            -PIVOT_PITCH * f.heel * (1.0 - step(0.0, 0.5, u))
                        },
                    }
                }
                None => Plant {
                    at: f.at,
                    lift: 0.0,
                    yaw: f.yaw,
                    pitch: if moving && t < g.beta {
                        g.stance_pitch(t / g.beta)
                    } else {
                        -PIVOT_PITCH * f.heel
                    },
                },
            }
        })
    }

    /// Advance the gait and the feet to `now`, the body facing `facing` (world) and moving at
    /// `vel` (world, m/s). `phase` (the mover's gait phase) starts the cycle off. In a dash
    /// (`dash`: how long one takes, s), the feet push off and leap instead of striding, and the
    /// body is drawn behind the owner as it pushes off (the mover's dash is at full speed at
    /// once; the body eases into it). Each foot's place in the stance moves by `rest_shift`
    /// (stumbling back). The gait to pose the body with, the feet, and the dash's progress,
    /// direction and how far the body lags behind.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        now: f64,
        phase: f32,
        facing: Quat,
        vel: Vec3,
        kind: Kind,
        ready: bool,
        dash: Option<f32>,
        rest_shift: [Vec2; 2],
    ) -> (Gait, [Plant; 2], Option<(f32, Vec2, f32)>) {
        let fwd = facing * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        let mut dt = (now - self.last) as f32;
        let mut turned = wrap(yaw - self.yaw);
        let fresh = !self.posed || !(0.0..=0.5).contains(&dt) || turned.abs() > 1.2;
        if fresh {
            dt = 0.0;
            turned = 0.0;
            self.cycles = f64::from(phase / TAU);
            self.accel = Vec2::ZERO;
            self.dash = None;
            self.behind = 0.0;
        }
        let v = facing.inverse() * vel;
        let vel = Vec2::new(v.x, v.z);
        let dashing = dash.is_some();
        if dt > 0.0 && !dashing {
            let a = ((vel - self.vel) / dt).clamp_length_max(12.0);
            self.accel = self.accel.lerp(a, (dt * 6.0).min(1.0));
        }
        if dashing {
            self.accel = Vec2::ZERO;
        }
        self.vel = vel;
        self.last = now;
        self.yaw = yaw;
        self.posed = true;
        let mut g = Gait::new(vel, 0.0, ready).hobbled(self.hobble);
        if g.speed >= MOVING && !dashing {
            self.cycles += f64::from(g.rate * dt);
        }
        g.cycle = self.cycles.rem_euclid(1.0) as f32;
        // Turning on the spot, the hips hold their heading as the body turns, then come round
        // after it, quickly at first, then easing in; walking, they keep up with it.
        let standing = if dashing {
            0.0
        } else {
            1.0 - step(MOVING, 1.0, g.speed)
        };
        let behind = (self.behind - turned * standing).clamp(-BEHIND, BEHIND);
        let tau = lerp(0.04, 0.3, standing);
        let catch = (1.5 * dt + behind.abs() * (1.0 - (-dt / tau).exp())).min(behind.abs());
        self.behind = behind - catch * behind.signum();
        let mut rest = rest_feet(kind, ready);
        for (r, shift) in rest.iter_mut().zip(rest_shift) {
            *r = (yawed(r.0 + shift, self.behind), r.1 + self.behind);
        }
        if fresh {
            self.reset_feet(now, &g, rest, ready);
        }
        let carry = match dash {
            None => {
                self.dash = None;
                None
            }
            Some(secs) => {
                // It began a step ago, when the mover set off (or, seen only now, just now).
                let secs = secs.max(0.05);
                let d = self.dash.get_or_insert(Dash {
                    t0: now - f64::from(dt),
                    dir: g.dir,
                    went: 0.0,
                    stepped: [false; 2],
                });
                d.went += g.speed * dt;
                if g.speed > 0.5 {
                    d.dir = g.dir;
                }
                let u = ((now - d.t0) as f32 / secs).clamp(0.0, 1.0);
                // The mover eases out over the dash: so far it has gone this share of it.
                let done = 1.0 - (1.0 - u) * (1.0 - u);
                let dist = if done > 1e-3 { d.went / done } else { 0.0 };
                Some(Carry {
                    u,
                    dir: d.dir,
                    dist,
                    lag: 1.6 * dist * u * (1.0 - u) * (1.0 - u),
                    secs,
                })
            }
        };
        let feet = self.step_feet(now, dt, turned, &g, rest, ready, carry);
        match carry {
            // Posed as standing: the dash's own lean and drive come on top.
            Some(c) => (
                Gait::new(Vec2::ZERO, g.cycle, ready),
                feet,
                Some((c.u, c.dir, c.lag)),
            ),
            None => (g, feet, None),
        }
    }
}

/// How far a pose moves the body off its feet: the pelvis (owner frame; y down negative), its
/// yaw, the chest's twist, lean and roll on top of it, and how far each heel lifts.
#[derive(Debug, Clone, Copy, Default)]
struct Body {
    shift: Vec3,
    yaw: f32,
    twist: f32,
    lean: f32,
    roll: f32,
    heel: [f32; 2],
}

impl Body {
    /// Both at once.
    fn plus(self, o: Body) -> Body {
        Body {
            shift: self.shift + o.shift,
            yaw: self.yaw + o.yaw,
            twist: self.twist + o.twist,
            lean: self.lean + o.lean,
            roll: self.roll + o.roll,
            heel: [self.heel[0] + o.heel[0], self.heel[1] + o.heel[1]],
        }
    }

    /// `s` times as much.
    fn scaled(self, s: f32) -> Body {
        Body {
            shift: self.shift * s,
            yaw: self.yaw * s,
            twist: self.twist * s,
            lean: self.lean * s,
            roll: self.roll * s,
            heel: self.heel.map(|h| h * s),
        }
    }
}

/// The body behind a blow, `u` s from the hit (negative while it is telegraphed, for `w` s;
/// recovering for `r` s after), `s` times as big (a heavy blow's is bigger): a dip before the
/// telegraph, coiling back onto the rear foot, then the hips leading the shoulders round into
/// the cut with the weight going onto the front foot (the rear heel up), and back.
fn blow_body(u: f32, w: f32, r: f32, s: f32) -> Body {
    let dip = step(-w, -0.88 * w, u) * (1.0 - step(-0.85 * w, -0.6 * w, u));
    let coil = step(-0.88 * w, -0.4 * w, u) * (1.0 - step(-0.07, 0.03, u));
    let hips = step(-0.09, 0.03, u) * (1.0 - step(0.45 * r, r, u));
    let fwd = step(-0.05, 0.09, u) * (1.0 - step(0.4 * r, r, u));
    Body {
        shift: v3(
            0.0,
            -(0.015 * dip + 0.02 * coil + 0.035 * fwd) * s,
            (0.05 * coil - 0.1 * fwd) * s,
        ),
        yaw: (-0.14 * coil + 0.14 * hips) * s,
        twist: (-0.3 * coil + 0.3 * fwd) * s,
        lean: (-0.06 * coil + 0.16 * fwd) * s,
        roll: 0.0,
        heel: [0.0, 0.4 * fwd * s.min(1.3)],
    }
}

/// How long a blow called off takes to settle back on guard (s).
const CHECK: f32 = 0.45;

/// How far into its release a blow called off `u` s ago gets (0..1): out, then held.
fn check_jab(u: f32) -> f32 {
    step(0.0, 0.13, u)
}

/// How far back on guard a blow called off `u` s ago is (0..1).
fn check_back(u: f32) -> f32 {
    step(0.1, 0.42, u)
}

/// The body behind a blow called off `u` s ago, `p` of the way through its telegraph (`w` s
/// long, `s` times as big): it lurches into the release, checks, rocking back onto the rear
/// foot, and uncoils onto its guard.
fn check_body(p: f32, u: f32, w: f32, s: f32) -> Body {
    let coiled = blow_body((p - 1.0) * w, w, 0.45, s).scaled(1.0 - step(0.04, 0.4, u));
    let lurch = step(0.0, 0.06, u) * (1.0 - step(0.06, 0.16, u));
    let rock = step(0.05, 0.13, u) * (1.0 - step(0.2, CHECK, u));
    coiled.plus(Body {
        shift: v3(0.0, -0.02 * rock, -0.04 * lurch + 0.05 * rock) * s,
        twist: 0.1 * lurch * s,
        lean: (0.08 * lurch - 0.1 * rock) * s,
        ..Default::default()
    })
}

/// How hard a blow landing on the guard `hit` s ago still pushes it back (0..1).
fn recoil(hit: f32) -> f32 {
    if hit < 0.0 {
        0.0
    } else {
        (1.6 * (1.0 - (-hit / 0.015).exp()) * (-hit / 0.09).exp()).min(1.0)
    }
}

/// How hard a blow that knocked the body off balance `e` s ago still throws it (0..1): at
/// once, easing off over a few tenths of a second.
fn snap(e: f32) -> f32 {
    (e / 0.04).clamp(0.0, 1.0) * (-(e - 0.04).max(0.0) / 0.2).exp()
}

/// The body braced behind a raised guard, rocked back as the blow lands: crouched under a
/// blow from above, turned to meet one from the side (and knocked across by it), side-on to a
/// thrust with the hips drawn back.
fn block_body(k: f32, hit: f32, parry: bool, line: Line) -> Body {
    let r = recoil(hit) * k;
    let beat = if parry { recoil(hit + 0.03) * k } else { 0.0 };
    let braced = Body {
        shift: v3(0.0, -0.04 * k, -0.03 * k + 0.05 * r),
        yaw: 0.06 * beat,
        twist: 0.06 * k + 0.12 * beat,
        lean: 0.08 * k - 0.14 * r,
        roll: 0.0,
        heel: [0.0; 2],
    };
    let to_line = match line {
        Line::High => Body {
            shift: v3(0.0, -0.05 * k, 0.0),
            lean: -0.04 * k,
            ..Default::default()
        },
        Line::Left => Body {
            shift: v3(0.04 * r, 0.0, 0.0),
            twist: 0.14 * k,
            roll: 0.05 * k - 0.1 * r,
            ..Default::default()
        },
        Line::Right => Body {
            shift: v3(-0.04 * r, 0.0, 0.0),
            twist: -0.2 * k,
            roll: -0.05 * k + 0.1 * r,
            ..Default::default()
        },
        Line::Thrust => Body {
            shift: v3(0.0, -0.02 * k, 0.05 * k),
            yaw: -0.2 * k,
            lean: 0.06 * k,
            ..Default::default()
        },
        Line::Square => Body::default(),
    };
    braced.plus(to_line)
}

/// A blow that landed `e` s ago throws the body as where it landed says: the head is wrenched
/// round away from the struck side and back along the push, the shoulders turning after it
/// (the feet stumble back a half step: see [`stumble`]); the trunk doubles over and turns
/// from it; or the legs buckle on it. The body, the head's pitch (chin down), yaw and roll on
/// top, and how hard the body is thrown (0..1, which knocks a shouldered weapon off its aim).
fn flinch(e: f32, h: &Hurt) -> (Body, Vec3, f32) {
    if !(0.0..1.0).contains(&e) {
        return (Body::default(), Vec3::ZERO, 0.0);
    }
    let (back, across, side, s) = (h.push.y, h.push.x, h.side, h.strength);
    match h.zone {
        Zone::Head => {
            // The snap back is quick; the head stays turned away a little longer, the
            // shoulders coming round after it.
            let j = snap(e) * s;
            let turned = step(0.0, 0.06, e) * (-(e - 0.1).max(0.0) / 0.3).exp() * s;
            let follow = step(0.02, 0.12, e) * (-(e - 0.14).max(0.0) / 0.3).exp() * s;
            // Away from the struck side (or along the push, struck square on).
            let away = if side != 0.0 {
                side
            } else if across.abs() > 0.2 {
                -across.signum()
            } else {
                0.0
            };
            // Caught on the half step back: the hips dip as the foot comes down.
            let dip = step(0.1, 0.18, e) * (1.0 - step(0.22, 0.4, e)) * s;
            (
                Body {
                    shift: v3(0.05 * across * j, -0.02 * j - 0.03 * dip, 0.08 * back * j),
                    yaw: 0.12 * away * follow,
                    twist: 0.38 * away * follow,
                    lean: -0.24 * back * j,
                    roll: -0.1 * across * j + 0.06 * away * follow,
                    ..Default::default()
                },
                v3(-0.4 * back * j, 0.85 * away * turned, 0.32 * away * turned),
                j.max(follow).min(1.0),
            )
        }
        Zone::Body => {
            let d = step(0.0, 0.07, e) * (-(e - 0.07).max(0.0) / 0.25).exp() * s;
            (
                Body {
                    shift: v3(0.04 * across, -0.05, 0.08 * back.max(0.3)) * d,
                    twist: -0.28 * side * d,
                    lean: 0.42 * d,
                    roll: -0.1 * side * d,
                    ..Default::default()
                },
                v3(0.3 * d, 0.0, 0.0),
                d.min(1.0),
            )
        }
        Zone::Legs => {
            let b = step(0.0, 0.09, e) * (1.0 - step(0.25, 0.7, e)) * s;
            (
                Body {
                    shift: v3(0.05 * side, -0.15, 0.03) * b,
                    twist: -0.1 * side * b,
                    lean: 0.18 * b,
                    roll: -0.16 * side * b,
                    ..Default::default()
                },
                v3(0.12 * b, 0.0, 0.0),
                b.min(1.0),
            )
        }
    }
}

/// How far back (m) the feet stumble `e` s after blow `h`: rocked by a blow to the head, a
/// half step back at once (along the push), then back up to where they stood.
fn stumble(e: f32, h: &Hurt) -> f32 {
    if h.zone != Zone::Head || !(0.0..1.0).contains(&e) {
        return 0.0;
    }
    0.2 * h.push.y.max(0.3) * h.strength.min(1.2) * step(0.0, 0.03, e) * (1.0 - step(0.5, 0.9, e))
}

/// Pose the skeleton: on its feet and acting, then, downed, falling from there; getting up
/// once revived; or kneeling over a downed mate to revive it.
fn solve(i: &Input) -> Skeleton {
    if let Some(r) = i.rise {
        return risen(i, r);
    }
    if let Some(t) = i.tend
        && t.k > 0.0
        && i.fall.is_none()
    {
        return tended(i, t);
    }
    let mut s = stand(i);
    if let Some(f) = i.fall {
        // Downed as it knelt by a mate: it goes down from the kneel.
        let knelt = i.tend.filter(|t| t.k > 0.0).map(|t| tended(i, t));
        fallen(&mut s, i.kind, f, knelt.as_ref());
        (s.lids, s.jaw) = (i.lids, i.jaw);
    }
    s
}

/// The skeleton on its feet.
fn stand(i: &Input) -> Skeleton {
    let mut s = Skeleton::default();
    let g = &i.gait;
    let (moving, run) = (g.moving, g.run);
    let ready = i.fighting || i.act != Act::Relaxed;
    let kneel = (if i.crouch { 1.0 - moving } else { 0.0 }).max(i.rising);
    let still = 1.0 - moving;
    let st = poised(i.kind, ease(i.poise));
    // How far the weapon is up from the carry at ease.
    let up = ease(i.up);
    let clock =
        |period: f64, offset: f64| (i.now * TAU as f64 / period + i.seed * offset).sin() as f32;
    let breath = i.breath;
    let sway = clock(7.3, 13.0);
    let glance = (i.now * 0.37 + i.seed * 5.0).sin() as f32;
    // Standing at ease the weight rests on one leg, then the other.
    let settle = (3.0 * clock(8.0, 3.0)).tanh();
    // On guard, a fighter's weight bobs lightly on bent knees and rocks between the feet.
    let bounce = clock(0.9, 2.0);
    let rock = clock(2.7, 5.0);
    let ranged = matches!(i.kind, Kind::Rifle | Kind::Bow);
    let melee = !ranged && i.kind != Kind::Fists;
    let blow_scale = |heavy: bool| match (melee, heavy) {
        (true, true) => 1.5,
        (true, false) => 1.0,
        _ => 0.6,
    };
    let mut body = match i.act {
        Act::Windup { p, heavy } if !ranged => {
            blow_body((p - 1.0) * i.windup, i.windup, 0.45, blow_scale(heavy))
        }
        Act::Strike { t, dur, heavy } if !ranged => blow_body(t, i.windup, dur, blow_scale(heavy)),
        Act::Check { p, u, heavy } if !ranged => check_body(p, u, i.windup, blow_scale(heavy)),
        // A gun or a bow is raised the same way whatever comes at it.
        Act::Block {
            k,
            hit,
            parry,
            line,
        } => block_body(k, hit, parry, if ranged { Line::Square } else { line }),
        Act::Stagger { k, e } => {
            let s = snap(e);
            Body {
                shift: v3(0.0, -0.06 * k - 0.04 * s, 0.12 * k + 0.1 * s),
                yaw: 0.12 * s,
                twist: -0.2 * s,
                ..Default::default()
            }
        }
        _ => Body::default(),
    };
    // A blow taken throws the head, the trunk or the legs.
    let (mut flinched, mut thrown) = (Vec3::ZERO, 0.0);
    if let Some((e, h)) = i.hurt {
        let (b, head, w) = flinch(e, &h);
        body = body.plus(b);
        (flinched, thrown) = (head, w);
    }
    if kneel > 0.0 {
        body = Body::default();
    }
    // Dashing: the body drawn behind the owner as the rear foot drives it off, low and leaning
    // into the dash.
    let (drive, lag, dash_dir) = match i.dash {
        Some((u, dir, lag)) => (
            step(0.0, 0.1, u) * (1.0 - step(0.5, 1.0, u)),
            dir * lag,
            dir,
        ),
        None => (0.0, Vec2::ZERO, Vec2::ZERO),
    };

    // Badly hurt in a leg (`lame`): walking, the hips dip and the trunk lurches over it as it
    // takes the weight (`limp`, through its time down), the other hip dropping; standing, the
    // weight is kept off it (`favour`), its heel up. Hurt in the body: hunched over the wound
    // (`hunch`), low. `side` is the wound's.
    let (mut lame, mut hunch, mut side, mut bad) = (0.0, 0.0, 0.0, 0);
    if let Some((zone, at, w)) = i.wound {
        side = if at < 0.0 { -1.0 } else { 1.0 };
        bad = usize::from(at >= 0.0);
        if zone == Zone::Legs {
            lame = w;
        } else {
            hunch = w;
        }
    }
    let t_bad = (g.cycle + 0.5 * bad as f32).rem_euclid(1.0);
    let on_bad = if t_bad < g.beta {
        (PI * t_bad / g.beta).sin()
    } else {
        0.0
    };
    let limp = lame * on_bad * moving;
    let favour = lame * still * (1.0 - kneel);
    body.heel[bad] = body.heel[bad].max(0.3 * favour);

    // ---------------------------------------------------------------- feet
    let mut feet = [Bone::default(); 2];
    for (k, foot) in feet.iter_mut().enumerate() {
        let p = i.feet[k];
        // A heel lifted by the weight going forward: up on the ball of the foot.
        let pitch = if body.heel[k] > 0.0 && p.lift == 0.0 {
            p.pitch.min(0.0) - body.heel[k]
        } else {
            p.pitch
        };
        let ground = v3(p.at.x, p.lift, p.at.y);
        let rot_y = ry(p.yaw);
        let rot = rot_y * rx(pitch);
        // Roll over the heel (toe up) or the ball of the foot (heel up).
        let pz = if pitch > 0.0 { 0.075 } else { -0.12 };
        let ankle = ground + rot_y * v3(0.0, 0.0, pz) + rot * v3(0.0, rig::ANKLE_Y, -pz);
        *foot = Bone::new(ankle, rot);
    }
    if kneel > 0.0 {
        // Down on the right knee behind low cover, left foot planted forward.
        let kneeling = [
            Bone::new(v3(-0.15, rig::ANKLE_Y, -0.3), ry(0.1)),
            Bone::new(v3(0.13, 0.115, 0.42), ry(-0.15) * rx(-1.25)),
        ];
        for (foot, kneeling) in feet.iter_mut().zip(kneeling) {
            *foot = foot.mix(kneeling, kneel);
        }
    }

    // ---------------------------------------------------------------- pelvis
    let tl = g.cycle;
    let gait_yaw = -0.1 * (TAU * tl).cos() * moving;
    let mid = TAU * (tl - g.beta * 0.5);
    let bob = lerp(0.016, -0.03, run) * (2.0 * mid).cos() * moving;
    // Over the foot that is down; at ease, on one leg; on guard, swaying a little.
    let weight = -0.026 * mid.cos() * moving * (1.0 - 0.6 * run)
        + still * if ready { 0.008 * sway } else { 0.04 * settle }
        + side * (0.02 * limp - 0.035 * favour);

    // On guard the knees stay bent until the fighter breaks into a run.
    let striding = if ready {
        moving * step(1.5, 3.5, g.speed)
    } else {
        moving
    };
    let mut hip_y = rig::HIP_Y - lerp(st.drop, 0.035 * (1.0 + run), striding) + bob + body.shift.y;
    if ready {
        hip_y -= 0.01 * (1.0 + bounce) * still;
    }
    if i.crouch {
        hip_y = lerp(0.52, 0.68, moving);
    }
    hip_y = lerp(hip_y, 0.52, i.rising);
    hip_y -= 0.035 * limp + 0.02 * lame + 0.03 * hunch;
    // Bringing the weapon up (or down), the knees give a little.
    hip_y -= 0.02 * (PI * up).sin() * (1.0 - kneel);
    hip_y -= 0.12 * i.dodge.abs() + 0.09 * drive;
    // At ease the hips turn a little toward the leg they rest on; on guard the weight rocks
    // between the front foot and the back.
    let at_ease = if ready { 0.0 } else { 0.05 * settle * still };
    let rocking = if ready { rock * still } else { 0.0 };
    // Turning on the spot, the hips come round after the body (the feet pivoting and stepping
    // round under them); the shoulders and the head lead them.
    let behind = i.behind * (1.0 - kneel);
    let yaw = lerp(st.yaw, gait_yaw, moving) * (1.0 - 0.5 * kneel) - 0.2 * kneel
        + body.yaw
        + at_ease
        + behind;
    // Leaning into the run and into speeding up; rolling into a sidestep.
    let push = (-i.accel.y * 0.02).clamp(-0.1, 0.14) * (1.0 - kneel);
    let tilt = 0.03 + 0.08 * run + if i.crouch { 0.25 } else { 0.0 } + push * 0.5;
    let side_lean = -0.05 * (g.vel.x / 2.0).clamp(-1.0, 1.0) * moving;
    let roll =
        weight * 1.4 + side_lean + body.roll - 0.22 * drive * dash_dir.x + side * 0.07 * limp;
    let pelvis_rot = ry(yaw) * rx(-tilt) * rz(roll);
    // Standing, the hips settle over the middle of the feet (not while a dash carries them).
    let rest = rest_feet(i.kind, ready).map(|(at, _)| yawed(at, behind));
    let support = if i.dash.is_some() {
        Vec2::ZERO
    } else {
        ((i.feet[0].at + i.feet[1].at) - (rest[0] + rest[1])) * 0.25 * still
    };
    let mut pelvis = Bone::new(
        v3(
            weight + 0.12 * i.dodge + body.shift.x + support.x - lag.x,
            hip_y,
            body.shift.z + support.y - 0.025 * rocking - lag.y,
        ),
        pelvis_rot,
    );
    // Sink the hips until both feet on the ground can be reached; a foot in the air out of
    // reach comes in toward the hip instead (the knee folding).
    let reach = (rig::THIGH + rig::SHIN) * 0.985;
    let hip_at =
        |pelvis: &Bone, k: usize| pelvis.at(v3((k as f32 * 2.0 - 1.0) * rig::HIP_X, 0.0, 0.0));
    let in_air = |k: usize| i.feet[k].lift > 0.0 && kneel == 0.0;
    for (k, foot) in feet.iter().enumerate() {
        if in_air(k) {
            continue;
        }
        let hip = hip_at(&pelvis, k);
        let d = hip - foot.pos;
        let flat = Vec2::new(d.x, d.z).length().min(reach * 0.99);
        let top = foot.pos.y + (reach * reach - flat * flat).sqrt();
        if hip.y > top {
            pelvis.pos.y -= hip.y - top;
        }
    }
    for (k, foot) in feet.iter_mut().enumerate() {
        if in_air(k) {
            let hip = hip_at(&pelvis, k);
            foot.pos = hip + (foot.pos - hip).clamp_length_max(reach);
        }
    }
    s.pelvis = pelvis;

    // ---------------------------------------------------------------- legs
    for (k, foot) in feet.iter().enumerate() {
        let sgn = if k == 0 { -1.0 } else { 1.0 };
        let hip = pelvis.at(v3(sgn * rig::HIP_X, 0.0, 0.0));
        let toes = foot.rot * Vec3::NEG_Z;
        let mut pole = v3(toes.x, 0.0, toes.z).normalize_or(Vec3::NEG_Z) + v3(0.0, 0.1, 0.0);
        if k == 1 {
            // The knee it kneels on goes down (turned over as it gets there, not at once).
            pole = pole.lerp(v3(0.0, -1.0, -0.6), step(0.3, 0.7, kneel));
        }
        let side = foot.rot * Vec3::X;
        let (thigh, shin, ankle) = chain(hip, foot.pos, (rig::THIGH, rig::SHIN), pole, side, true);
        s.thigh[k] = thigh;
        s.shin[k] = shin;
        s.foot[k] = Bone::new(ankle, foot.rot);
    }
    s.tabard = tabards(
        pelvis,
        s.thigh,
        s.shin,
        (run, 0.06 * run * (TAU * tl * 2.0).sin()),
        0.0,
    );

    // ---------------------------------------------------------------- chest and head
    let forward = -dash_dir.y;
    let mut lean = lerp(st.lean, 0.06 + 0.16 * run, moving) + 0.12 * kneel - 0.025 * breath
        + body.lean
        + push
        + 0.03 * rocking
        + drive * if forward > 0.0 { 0.34 } else { 0.12 } * forward;
    let mut twist =
        lerp(st.twist, -gait_yaw * 1.6, moving) + body.twist - 0.65 * behind + side * 0.12 * hunch;
    let mut chest_roll =
        -roll * 0.6 - 0.35 * i.dodge + body.roll * 0.5 - side * (0.16 * limp + 0.12 * hunch);
    lean += 0.05 * lame + 0.2 * hunch;
    match i.act {
        Act::Windup { p, .. } if i.kind == Kind::Bow => {
            twist -= 0.25 * step(0.0, 0.4, p);
        }
        Act::Stagger { k, e } => {
            lean -= (0.22 + 0.06 * (i.now * 12.0).sin() as f32) * k + 0.3 * snap(e);
            chest_roll += 0.1 * k * (i.now * 7.0).sin() as f32;
        }
        _ => {}
    }
    if let (Kind::Rifle, Some(shot)) = (i.kind, i.shot) {
        // The kick rocks the shoulders back.
        lean -= 0.05 * kick(shot.since);
    }
    let aim_to = match (i.kind, i.act) {
        (Kind::Rifle, Act::Windup { .. } | Act::Strike { .. }) => 1.0,
        (Kind::Rifle, Act::Guard | Act::Block { .. } | Act::Check { .. }) => 1.0 - run,
        (Kind::Bow, Act::Windup { p, .. }) => step(0.0, 0.35, p),
        (Kind::Bow, Act::Check { p, u, .. }) => step(0.0, 0.35, p) * (1.0 - step(0.1, 0.4, u)),
        // Loosed, the bow arm holds up a moment before it comes down to nock the next.
        (Kind::Bow, Act::Strike { .. } | Act::Guard) => {
            i.shot.map_or(0.0, |s| 1.0 - step(0.3, 0.6, s.since))
        }
        _ => 0.0,
    } * (1.0 - thrown)
        * (1.0 - drive);
    // The head goes to the sights as the gun comes up.
    let aiming = aim_to * up;
    // The head turns to what the eyes are on (at ease, a little with the body, glancing about
    // now and then; aiming, along the sights); past what the neck allows, the shoulders turn
    // with it.
    let focus = i.look.focus * (1.0 - aiming);
    let idle = 0.15 * (yaw + twist) + if ready { 0.0 } else { 0.25 * glance * still };
    let want = lerp(idle, i.look.head.x, focus);
    let over = wrap(want - (yaw + twist));
    twist += (over - over.clamp(-NECK_YAW, NECK_YAW)).clamp(-TORSO_YAW, TORSO_YAW) * focus;
    let waist = pelvis.at(rig::WAIST) + v3(0.0, 0.004 * breath, 0.0);
    let chest = Bone::new(waist, pelvis.rot * ry(twist) * rx(-lean) * rz(chest_roll));
    s.chest = chest;
    let net_yaw = yaw + twist;
    let mut head_yaw = lerp(
        wrap(want - net_yaw).clamp(-NECK_YAW, NECK_YAW),
        -net_yaw,
        aiming,
    );
    let mut head_pitch = -(tilt + lean) * lerp(0.75, 1.0, focus)
        - (i.look.head.y * focus).clamp(-NECK_PITCH, NECK_PITCH);
    let mut head_roll = -chest_roll * 0.5;
    match i.act {
        // Flinching from the blow, chin down behind the guard (not off a shouldered stock),
        // looking up from under one from above, looking at one from the side.
        Act::Block { k, hit, line, .. } => {
            let duck = if line == Line::High { -0.12 } else { 0.0 };
            head_pitch += (0.1 + duck + 0.12 * recoil(hit)) * k * (1.0 - aiming);
            head_yaw += match line {
                Line::Left => 0.18,
                Line::Right => -0.18,
                _ => 0.0,
            } * k
                * (1.0 - aiming);
        }
        // Snapped back by the blow.
        Act::Stagger { e, .. } => head_pitch -= 0.35 * snap(e),
        // Head up as the blow is checked.
        Act::Check { u, .. } => {
            head_pitch -= 0.1 * step(0.05, 0.13, u) * (1.0 - step(0.2, CHECK, u))
        }
        _ => {}
    }
    head_pitch += flinched.x + 0.06 * lame + 0.12 * hunch;
    head_yaw += flinched.y;
    head_roll += flinched.z;
    if i.kind == Kind::Rifle {
        // Cheek on the stock.
        head_pitch += 0.12 * aiming;
        head_roll -= 0.14 * aiming;
    }
    let neck = chest.at(rig::NECK);
    let head = Bone::new(
        neck,
        chest.rot * ry(head_yaw) * rx(-head_pitch) * rz(head_roll),
    );
    s.head = head;
    (s.eyes, s.lids) = eyes_on(head, &i.look, i.lids);
    s.jaw = i.jaw;

    // ---------------------------------------------------------------- arms and weapon
    // Free arms swing against the legs (pumping when running); hands with something in them
    // swing it a little less.
    let swing = 0.35 * moving * (1.0 + run) * (TAU * tl).cos() * g.roll().abs().max(0.4);
    let jiggle = Vec2::new(clock(2.3, 1.0), clock(3.1, 4.0)) * still;
    // The arms for an act (at ease, carried, or `low`, at the low ready), the gun shouldered
    // as far as `aim`.
    let arms_for = |act: Act, low: bool, aim: f32, ready: bool| {
        let mut arms = match i.kind {
            Kind::Longsword => longsword(act, chest, swing, jiggle, low),
            Kind::SwordShield => sword_and_shield(act, chest, swing, jiggle, low),
            Kind::Rifle => rifle(act, chest, head, pelvis, aim, i.shot, (i.now, i.seed), low),
            Kind::Bow => bow(act, chest, head, aim, ready, swing, i.shot),
            Kind::Fists => fists(act, chest, ready),
        };
        for k in 0..2 {
            let sgn = if k == 0 { -1.0 } else { 1.0 };
            if arms.hand[k] == Hand::Swing {
                let hang_to = v3(sgn * 0.035, -0.52, 0.04).lerp(v3(sgn * -0.02, -0.29, -0.22), run);
                let a = swing * -sgn;
                arms.hand[k] = Hand::Free(chest.at(shoulder(k) + rx(a) * hang_to));
                arms.pole[k] = chest.rot * v3(sgn * 0.3, -0.3, 1.0);
            }
        }
        arms
    };
    // At ease: carried, or (wary) at the low ready.
    let at_ease = || {
        let carried = || arms_for(Act::Relaxed, false, 0.0, false);
        let low = || arms_for(Act::Relaxed, true, 0.0, false);
        match i.wary {
            w if w <= 0.0 => carried(),
            w if w >= 1.0 => low(),
            w => readied(&carried(), &low(), ease(w), chest, i.kind, false),
        }
    };
    let mut arms = if up >= 1.0 {
        arms_for(i.act, false, aim_to, ready)
    } else if up <= 0.0 {
        at_ease()
    } else {
        // Coming up to what the act has it doing, or (at ease now) going down from the guard.
        let (act, aim) = match i.act {
            Act::Relaxed => (
                Act::Guard,
                if i.kind == Kind::Rifle {
                    1.0 - run
                } else {
                    0.0
                },
            ),
            act => (act, aim_to),
        };
        let rest = at_ease();
        let to = arms_for(act, false, aim, true);
        let mut arms = readied(&rest, &to, up, chest, i.kind, true);
        if i.kind == Kind::Bow {
            // The bow comes up in front first while the string hand goes to the quiver (the
            // elbow out to the side as it goes up over the shoulder); an arrow is drawn and
            // nocked on the way up (put back on the way down), as after a loose, at the pace of
            // the clock (`i.up`, not eased again: the hand would race through the middle);
            // last, the string hand goes to what the act has it doing (drawing, winding up).
            const REACH: f32 = 0.35;
            const NOCKED: f32 = 0.9;
            let raw = i.up;
            let reach = step(0.0, REACH, raw);
            arms.weapon = rest.weapon.mix(to.weapon, reach);
            arms.shield = arms.weapon;
            arms.hand[0] = Hand::Grip(arms.weapon.pos, arms.weapon.rot * weapons::BOW_HAND);
            let anchor = head.at(ANCHOR);
            let along = lerp(
                0.25,
                1.0,
                ((raw - REACH) / (NOCKED - REACH)).clamp(0.0, 1.0),
            );
            let shot = Shot {
                since: 0.3 + 0.85 * along,
                ready: 1.27,
                reload: false,
            };
            let (_, nocked, hand, arrow, out) = loosed(chest, arms.weapon, anchor, shot);
            (arms.hand[1], arms.held, arms.nocked) = match (hand, rest.hand[1]) {
                (Hand::Grip(p, r), Hand::Free(from)) if raw < REACH => (
                    Hand::Near(from.lerp(p - r * grip(1), reach), r, reach),
                    None,
                    false,
                ),
                _ => (hand, arrow, nocked),
            };
            let onto = step(0.85, 1.0, raw);
            arms.hand[1] = shift(arms.hand[1], to.hand[1], onto);
            arms.draw = to.draw * onto;
            // The elbow out to the side from the first, as the hand goes up to the quiver.
            let first = step(0.0, 0.5 * REACH, raw);
            let elbow = string_elbow(if raw < REACH { first } else { out });
            arms.pole[1] = rest.pole[1]
                .lerp(chest.rot * elbow, first)
                .lerp(to.pole[1], onto)
                .normalize_or(to.pole[1]);
        }
        arms
    };
    // Badly hurt and at ease, a free hand pressed to the wound (the flank, or the front of the
    // thigh), the elbow out.
    if let Some((zone, _, w)) = i.wound
        && i.act == Act::Relaxed
        && w > 0.0
    {
        let near = usize::from(side > 0.0);
        let free = [near, 1 - near]
            .into_iter()
            .find_map(|k| match arms.hand[k] {
                Hand::Free(from) => Some((k, from)),
                _ => None,
            });
        if let Some((k, from)) = free {
            let sgn = if k == 0 { -1.0 } else { 1.0 };
            let across = sgn != side;
            let press = if zone == Zone::Legs {
                // The front of the thigh (reached across the body, its top).
                let down = if across { 0.06 } else { 0.2 };
                s.thigh[bad].at(v3(0.0, -down, -0.085)) + v3(0.0, 0.07, -0.01)
            } else {
                let at = v3(side * 0.12, 0.1, -0.11);
                let from = if across {
                    v3(side * 0.9, 0.1, -0.4)
                } else {
                    v3(-side * 0.45, 0.15, -0.88)
                };
                chest.at(at - from.normalize() * 0.075)
            };
            let k_in = w * (1.0 - step(0.0, 0.5, up));
            arms.hand[k] = Hand::Free(from.lerp(press, k_in));
            arms.pole[k] =
                chest.rot * v3(sgn * 0.9, -0.3, 0.5).lerp(v3(sgn * 0.3, -0.3, 1.0), 1.0 - k_in);
        }
    }
    for k in 0..2 {
        let sh = chest.at(shoulder(k));
        let (target, rot) = match arms.hand[k] {
            Hand::Grip(point, rot) => (point - rot * grip(k), Some((rot, 1.0))),
            Hand::Near(w, rot, t) => (w, Some((rot, t))),
            Hand::Free(w) => (w, None),
            Hand::Swing => (sh + Vec3::NEG_Y * 0.5, None),
        };
        let side = chest.rot * Vec3::X;
        let (upper, fore, wrist) = chain(
            sh,
            target,
            (rig::UPPER_ARM, rig::FOREARM),
            arms.pole[k],
            side,
            false,
        );
        s.upper[k] = upper;
        s.fore[k] = fore;
        let rot = match rot {
            Some((rot, t)) if t >= 1.0 => rot,
            Some((rot, t)) => fore.rot.slerp(rot, t),
            None => fore.rot,
        };
        s.hand[k] = Bone::new(wrist, rot);
        s.want[k] = target;
    }
    s.weapon = arms.weapon;
    s.shield = arms.shield;
    s.draw = arms.draw;
    s.nocked = arms.nocked;
    s.held = arms.held;
    s.bend = limb_bend(s.draw);
    s
}

/// The eyes on what they `look` at (at ease, wandering a little off the head's line), as far
/// as they turn in the `head`; looking down, the upper lids (shut `lids` of the way) come down
/// with them. The eyes' turn and the lids.
fn eyes_on(head: Bone, look: &Look, lids: f32) -> ([Quat; 2], f32) {
    let ahead = head.rot * Vec3::NEG_Z;
    let along = Vec2::new((-ahead.x).atan2(-ahead.z), ahead.y.clamp(-1.0, 1.0).asin());
    let seen = (along + look.dart).lerp(look.eyes, look.focus);
    let to = head.rot.inverse() * (ry(seen.x) * rx(seen.y) * Vec3::NEG_Z);
    let eye_yaw = (-to.x).atan2(-to.z).clamp(-EYES_YAW, EYES_YAW);
    let eye_pitch =
        to.y.clamp(-1.0, 1.0)
            .asin()
            .clamp(EYES_PITCH.0, EYES_PITCH.1);
    (
        [ry(eye_yaw) * rx(eye_pitch); 2],
        (lids + 0.6 * (-eye_pitch).max(0.0)).min(1.0),
    )
}

/// The tabards on the pelvis, swinging with the legs: the front one rides on the forward
/// thigh (`run` lifts them, `flap` flutters them).
fn tabards(
    pelvis: Bone,
    thigh: [Bone; 2],
    shin: [Bone; 2],
    (run, flap): (f32, f32),
    lying: f32,
) -> [Bone; 2] {
    let thigh_angle = |k: usize| {
        let d = pelvis.rot.inverse() * (shin[k].pos - thigh[k].pos);
        (-d.z).atan2(-d.y)
    };
    let (a, b) = (thigh_angle(0), thigh_angle(1));
    // A thigh raised high (kneeling, a long stride) carries the front tabard with it: it turns
    // and slides over that thigh to lie along it instead of standing out in the air. Lying on
    // the back (`lying` 1), it drapes over the lower thigh instead, and the back one lies flat
    // under the hips.
    let lead = if a > b { 0 } else { 1 };
    let up = lerp(a.max(b).max(0.0), a.min(b) - 0.1, lying);
    let follow = step(0.6, 1.2, up) * (1.0 - lying);
    let knee = pelvis.rot.inverse() * (shin[lead].pos - thigh[lead].pos);
    let turn = (-knee.x).atan2(-knee.z).clamp(-0.6, 0.6) * follow;
    let slide = (thigh[lead].pos - pelvis.pos).dot(pelvis.rot * Vec3::X) * 0.8 * follow;
    [
        pelvis.then(Bone::new(
            v3(slide, 0.085, -0.128),
            ry(turn) * rx(up * 0.95 + 0.04 + 0.1 * run + flap),
        )),
        pelvis.then(Bone::new(
            v3(0.0, 0.085, 0.13),
            rx(lerp(
                a.min(b).min(0.0) * 0.95 - 0.04 - 0.25 * run - flap,
                0.12,
                lying,
            )),
        )),
    ]
}

// ---------------------------------------------------------------------------- falling

/// Lying on its back: how high the hip joints are off the ground (m), and how far over the
/// hips and the chest lie (radians back from upright; past a right angle, the small of the
/// back off the ground).
const HIPS_DOWN: f32 = 0.18;
const HIPS_LIE: f32 = FRAC_PI_2 + 0.12;
const CHEST_LIE: f32 = FRAC_PI_2 + 0.05;
/// How high the wrists lie (m).
const WRISTS_DOWN: f32 = 0.06;
/// When the hands let go of the weapon (s after going down).
const LET_GO: f32 = 0.3;

/// How a value eases into a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ease {
    /// Speeding up into it (falling).
    In,
    /// Slowing into it (rising off a bounce).
    Out,
    /// Both.
    Both,
}

/// A value along keys (time, value, how it eases into it), held before the first and after
/// the last.
fn track(t: f32, keys: &[(f32, f32, Ease)]) -> f32 {
    let Some(&(t0, v0, _)) = keys.first() else {
        return 0.0;
    };
    if t <= t0 {
        return v0;
    }
    for w in keys.windows(2) {
        let ((ta, a, _), (tb, b, ease)) = (w[0], w[1]);
        if t < tb {
            let x = (t - ta) / (tb - ta);
            let e = match ease {
                Ease::In => x * x,
                Ease::Out => x * (2.0 - x),
                Ease::Both => x * x * (3.0 - 2.0 * x),
            };
            return lerp(a, b, e);
        }
    }
    keys[keys.len() - 1].1
}

/// A limb's bend direction a part `x` of the way from `a` to `b`, turned round the limb
/// (along `axis`) rather than through it, so the joint never flips across.
fn about(axis: Vec3, a: Vec3, b: Vec3, x: f32) -> Vec3 {
    let d = axis.normalize_or(Vec3::NEG_Y);
    let flat = |v: Vec3| v - d * v.dot(d);
    let pa = flat(a).normalize_or(d.any_orthonormal_vector());
    let pb = flat(b).normalize_or(pa);
    let turn = pa.angle_between(pb) * d.dot(pa.cross(pb)).signum();
    Quat::from_axis_angle(d, turn * x) * pa
}

/// How far the hips may have to go to where they land (m, flat) for a fall at its full
/// pace (from standing, or kneeling about square to a mate).
const FALL_REACH: f32 = 0.45;

/// How low a falling body's knee joint may go (m): about on the ground, the kneecap just
/// touching it.
const KNEE_FLOOR: f32 = 0.04;

/// The body `t` s after going down, from standing as `s0`, the weapon still in hand: the
/// knees give and the hips drop, the body topples back along the fall, the hips land, then
/// the back, then the head (each bouncing a little), the legs kick up as it rolls back and
/// come down, one straight, one folded out to the side, and the arms go limp, swing up and
/// flop out onto the ground. Each part starts from where it stood, so the fall picks up from
/// any pose.
fn fall_pose(s0: &Skeleton, stood: &Skeleton, kind: Kind, f: Fall, t: f32) -> Skeleton {
    use Ease::{Both, In, Out};
    let r = ry(f.yaw);
    let ri = r.inverse();
    let mid = (stood.foot[0].pos + stood.foot[1].pos) * 0.5;
    let c0 = v3(mid.x, 0.0, mid.z);
    // Into the fall's frame (on the ground between the feet, falling toward +Z), and back.
    let into = |b: Bone| Bone::new(ri * (b.pos - c0), ri * b.rot);
    let back = |b: Bone| Bone::new(c0 + r * b.pos, r * b.rot);
    let sgn = |k: usize| if k == 0 { -1.0 } else { 1.0 };
    let (bs, fs) = (sgn(f.bent), sgn(f.flung));
    // From a kneel leaning well over to one side (to a mate's chest), the hips have farther to
    // go to where they land than from standing: it goes down that much slower.
    let t = {
        let p = into(s0.pelvis).pos;
        let far = Vec2::new(p.x, p.z - 0.42).length();
        t * (FALL_REACH / far).min(1.0)
    };

    // ---------------------------------------------------------------- trunk
    let p0 = into(s0.pelvis);
    let (py, px, pz) = p0.rot.to_euler(EulerRot::YXZ);
    // It lies turned a little the way it stood.
    let stood_yaw = into(stood.pelvis).rot.to_euler(EulerRot::YXZ).0;
    let at = v3(
        track(t, &[(0.0, p0.pos.x, Both), (0.6, 0.03 * bs, Both)]),
        track(
            t,
            &[
                (0.0, p0.pos.y, Both),
                (0.47, HIPS_DOWN, In),
                (0.55, HIPS_DOWN + 0.025, Out),
                (0.66, HIPS_DOWN, In),
            ],
        ),
        track(
            t,
            &[
                (0.0, p0.pos.z, Both),
                (0.12, p0.pos.z, Both),
                (0.5, 0.38, Both),
                (0.7, 0.42, Out),
            ],
        ),
    );
    let pitch = track(
        t,
        &[
            (0.0, px, Both),
            (0.28, 0.32, Both),
            (0.6, HIPS_LIE + 0.05, In),
            (0.7, HIPS_LIE - 0.03, Out),
            (0.8, HIPS_LIE, Both),
        ],
    );
    let yaw = track(t, &[(0.0, py, Both), (0.8, 0.4 * stood_yaw, Both)]);
    let roll = track(t, &[(0.0, pz, Both), (0.5, 0.0, Both)]);
    // Rolled a little onto the hip of the leg that folds out.
    let spin = track(
        t,
        &[(0.0, 0.0, Both), (0.5, 0.0, Both), (1.0, -0.1 * bs, Both)],
    );
    let pelvis = Bone::new(at, ry(yaw) * rx(pitch) * rz(roll) * ry(spin));
    // The chest folds forward as the knees give, lags as the hips go back, and lands after
    // them.
    let (cy, cx, cz) = (ri * s0.chest.rot).to_euler(EulerRot::YXZ);
    let chest = Bone::new(
        pelvis.at(rig::WAIST),
        ry(track(
            t,
            &[(0.0, cy, Both), (0.8, 0.4 * stood_yaw + 0.1 * fs, Both)],
        )) * rx(track(
            t,
            &[
                (0.0, cx, Both),
                (0.28, -0.3, Both),
                (0.64, CHEST_LIE + 0.02, In),
                (0.72, CHEST_LIE - 0.06, Out),
                (0.84, CHEST_LIE, Both),
            ],
        )) * rz(track(t, &[(0.0, cz, Both), (0.9, 0.05 * fs, Both)]))
            * ry(track(
                t,
                &[(0.0, 0.0, Both), (0.5, 0.0, Both), (0.95, 0.1 * fs, Both)],
            )),
    );
    // The head lolls: chin down as the knees give and the back goes over, knocked back as
    // the back lands, then rolled to one side.
    let (hy, hx, hz) = (s0.chest.rot.inverse() * s0.head.rot).to_euler(EulerRot::YXZ);
    let chin = track(
        t,
        &[
            (0.0, -hx, Both),
            (0.25, 0.35, Both),
            (0.58, 0.5, Both),
            (0.7, -0.08, In),
            (0.8, 0.14, Out),
            (0.95, 0.1, Both),
        ],
    );
    let head = Bone::new(
        chest.at(rig::NECK),
        chest.rot
            * ry(track(
                t,
                &[
                    (0.0, hy, Both),
                    (0.6, 0.15 * fs, Both),
                    (1.05, 0.8 * fs, Both),
                ],
            ))
            * rx(-chin)
            * rz(track(t, &[(0.0, hz, Both), (1.05, 0.1 * fs, Both)])),
    );
    let mut s = Skeleton {
        pelvis,
        chest,
        head,
        ..Skeleton::default()
    };

    // ---------------------------------------------------------------- legs
    for k in 0..2 {
        let sg = sgn(k);
        let bent = k == f.bent;
        let a0 = into(s0.foot[k]);
        // Planted while the knees give, then sliding out as the hips go back, kicking up as
        // the body rolls onto its back, and settling: one leg straight out, the other's foot
        // by its knee, the knee fallen out to the side.
        let rest = if bent {
            v3(sg * 0.06, 0.12, 0.02)
        } else {
            v3(sg * 0.2, 0.12, -0.42)
        };
        let kick = if (0.42..0.88).contains(&t) {
            (PI * (t - 0.42) / 0.46).sin()
        } else {
            0.0
        };
        let settle = if (0.86..0.98).contains(&t) {
            0.015 * (PI * (t - 0.86) / 0.12).sin()
        } else {
            0.0
        };
        let lift = if bent { 0.1 } else { 0.2 } * kick + settle;
        // (From down on the knees, the feet come out from under it first, before the hips
        // sit back onto them.)
        let kneeling = into(s0.shin[k]).pos.y < 0.25;
        let out = if kneeling {
            step(0.0, 0.45, t)
        } else {
            step(0.3, 0.8, t)
        };
        let target = a0.pos.lerp(rest, out) + Vec3::Y * lift;
        // The knees bend the way they were bent (toward the front of the hips, for a leg
        // near straight; up, once lying), the folded one falling out to the side.
        let toes = a0.rot * Vec3::NEG_Z;
        let hip0 = into(s0.pelvis).at(v3(sg * rig::HIP_X, 0.0, 0.0));
        let bend0 = into(s0.shin[k]).pos - (hip0 + a0.pos) * 0.5;
        let knee0 = if bend0.length() > 0.03 {
            bend0
        } else {
            v3(toes.x, 0.0, toes.z).normalize_or(Vec3::NEG_Z) + v3(0.0, 0.1, 0.0)
        };
        let knee1 = pelvis.rot
            * if bent {
                v3(sg * 0.9, 0.0, -0.45)
            } else {
                v3(sg * 0.2, 0.0, -1.0)
            };
        let hip = pelvis.at(v3(sg * rig::HIP_X, 0.0, 0.0));
        let pole = if kneeling {
            // From down on the knees: the knee keeps pointing the way it did (forward and
            // down, forward as the hips sit back onto the heels, up as the feet come out
            // ahead), so it doesn't flip as the leg swings round under the hips.
            let ahead = v3(knee0.x, 0.0, knee0.z).normalize_or(Vec3::NEG_Z);
            let ahead = about(target - hip, knee0, ahead, step(0.0, 0.2, t));
            // (The folded one falls out to the side early, as its foot comes in by the hips.)
            let out = if bent {
                step(0.1, 0.4, t)
            } else {
                step(0.2, 0.5, t)
            };
            about(target - hip, ahead, knee1, out)
        } else {
            about(target - hip, knee0, knee1, step(0.05, 0.6, t))
        };
        let leg = |pole: Vec3| {
            chain(
                hip,
                target,
                (rig::THIGH, rig::SHIN),
                pole,
                pelvis.rot * Vec3::X,
                true,
            )
        };
        // Where the knee would go into the ground (the leg swinging round under the hips
        // from a kneel with the mate well off to one side), it stays on the ground instead,
        // the way it was headed, and the foot trails from it.
        let floor = into(s0.shin[k]).pos.y.min(KNEE_FLOOR);
        let (mut thigh, mut shin, mut ankle) = leg(pole);
        let onto = 1.0 - step(floor - 0.04, floor, shin.pos.y);
        if onto > 0.0 {
            let drop = (hip.y - floor).min(rig::THIGH - 1e-3);
            let way = Vec2::new(shin.pos.x - hip.x, shin.pos.z - hip.z).normalize_or(Vec2::NEG_Y);
            let out = (rig::THIGH * rig::THIGH - drop * drop).sqrt();
            let on = hip + v3(way.x * out, -drop, way.y * out);
            let knee = hip + (shin.pos.lerp(on, onto) - hip).normalize() * rig::THIGH;
            let foot = knee + (target - knee).normalize_or(Vec3::Z) * rig::SHIN;
            (thigh, shin, ankle) = chain(
                hip,
                foot,
                (rig::THIGH, rig::SHIN),
                knee - (hip + foot) * 0.5,
                pelvis.rot * Vec3::X,
                true,
            );
        }
        s.thigh[k] = thigh;
        s.shin[k] = shin;
        // The foot goes slack, pointing on along the shin.
        s.foot[k] = Bone::new(
            ankle,
            a0.rot.slerp(shin.rot * rx(-0.5), step(0.42, 0.72, t)),
        );
    }
    s.tabard = tabards(pelvis, s.thigh, s.shin, (0.0, 0.0), step(0.35, 0.75, t));

    // ---------------------------------------------------------------- arms
    for k in 0..2 {
        let sg = sgn(k);
        let sh = chest.at(shoulder(k));
        let w0 = into(s0.hand[k]);
        // Limp, hanging; thrown up as the body goes over; flopped onto the ground (one out
        // wide, one along the body), bouncing once.
        let hang = sh + v3(sg * 0.08, -0.52, -0.06);
        let thrown = sh + v3(sg * 0.3, 0.02, -0.28);
        let ground = if k == f.flung {
            v3(sh.x + sg * 0.5, WRISTS_DOWN, sh.z + 0.12)
        } else {
            v3(sh.x + sg * 0.2, WRISTS_DOWN, sh.z - 0.46)
        };
        let limp = step(0.04, 0.4, t);
        let up = step(0.32, 0.54, t) * (1.0 - step(0.56, 0.84, t));
        let down = step(0.54, 0.9, t);
        let bounce = if (0.9..1.02).contains(&t) {
            0.02 * (PI * (t - 0.9) / 0.12).sin()
        } else {
            0.0
        };
        let mut target = w0.pos.lerp(hang, limp).lerp(thrown, up).lerp(ground, down);
        target.y = target.y.max(WRISTS_DOWN) + bounce;
        let elbow = into(s0.fore[k]).pos - (into(s0.upper[k]).pos + w0.pos) * 0.5;
        let reach = target - sh;
        let pole = about(reach, elbow, v3(sg * 0.4, 0.0, 0.6), limp);
        let pole = about(reach, pole, v3(sg * 0.3, 0.3, -0.5), down);
        let (upper, fore, wrist) = chain(
            sh,
            target,
            (rig::UPPER_ARM, rig::FOREARM),
            pole,
            chest.rot * Vec3::X,
            false,
        );
        s.upper[k] = upper;
        s.fore[k] = fore;
        // The hand goes along with the forearm as it swings (going limp, in line with it).
        let carried = fore.rot * into(s0.fore[k]).rot.inverse() * w0.rot;
        s.hand[k] = Bone::new(wrist, carried.slerp(fore.rot, limp));
        s.want[k] = target;
    }
    // Still in hand (the bow in the left, anything else in the right; the shield on the left
    // arm).
    let held =
        |h: Bone, w: Bone| Bone::new(h.rot.inverse() * (w.pos - h.pos), h.rot.inverse() * w.rot);
    let holder = if kind == Kind::Bow { 0 } else { 1 };
    let weapon = s.hand[holder].then(held(s0.hand[holder], s0.weapon));
    let shield = s.hand[0].then(held(s0.hand[0], s0.shield));
    Skeleton {
        pelvis: back(s.pelvis),
        chest: back(s.chest),
        head: back(s.head),
        thigh: s.thigh.map(&back),
        shin: s.shin.map(&back),
        foot: s.foot.map(&back),
        upper: s.upper.map(&back),
        fore: s.fore.map(&back),
        hand: s.hand.map(&back),
        tabard: s.tabard.map(&back),
        want: s.want.map(|p| c0 + r * p),
        weapon: back(weapon),
        shield: back(shield),
        bend: 0.0,
        draw: 0.0,
        nocked: false,
        held: None,
        eyes: s0.eyes,
        lids: 0.0,
        jaw: 0.0,
    }
}

/// Getting up (see [`Rise`]), on guard: from lying as it fell it sits up, takes up its
/// weapon where it lies and comes round onto one knee (see [`rise_pose`]), the weapon at the
/// low ready; then it pushes up off the knee and brings the weapon up to guard.
fn risen(i: &Input, r: Rise) -> Skeleton {
    let ready = Input {
        act: Act::Guard,
        fighting: true,
        rise: None,
        poise: 1.0,
        wary: 1.0,
        ..*i
    };
    if r.t >= RISE_KNEEL {
        let u = ((r.t - RISE_KNEEL) / rise_up(i.kind)).min(1.0);
        return stand(&Input {
            rising: 1.0 - ease(u),
            up: u,
            ..ready
        });
    }
    // As it went down.
    let was = Input {
        now: r.since,
        feet: r.feet,
        gait: Gait::new(Vec2::ZERO, 0.0, true),
        accel: Vec2::ZERO,
        crouch: false,
        act: r.was,
        dodge: 0.0,
        hurt: None,
        dash: None,
        fall: None,
        behind: 0.0,
        shot: None,
        wound: None,
        up: if r.was == Act::Relaxed { 0.0 } else { 1.0 },
        poise: if r.was == Act::Relaxed { 0.0 } else { 1.0 },
        wary: 0.0,
        rise: None,
        rising: 0.0,
        ..*i
    };
    let s0 = stand(&was);
    let mid = (s0.foot[0].pos + s0.foot[1].pos) * 0.5;
    let mut lying = s0;
    let knelt = r.knelt.filter(|t| t.k > 0.0).map(|t| tended(&was, t));
    fallen(&mut lying, i.kind, r.fall, knelt.as_ref());
    let kneel = stand(&Input {
        rising: 1.0,
        up: 0.0,
        ..ready
    });
    rise_pose(
        &lying,
        &kneel,
        i.kind,
        r.fall,
        v3(mid.x, 0.0, mid.z),
        r.t / RISE_KNEEL,
    )
}

/// The body getting up `u` of the way (0..1) from lying as `lying` (fallen along `f` from
/// between its feet at `c0`) to down on one knee as `kneel`: it sits up, turned to its weapon
/// where it lies and reaching for it (with a shield, rolled toward that first, the left hand
/// taking it up), takes it up, and comes round onto the knee, the other hand pushing off the
/// ground, the feet drawn in under it. Each part starts where it lay and ends where the kneel
/// has it, the weapon (and shield) held as the kneel holds them from the moment they are in
/// hand.
fn rise_pose(
    lying: &Skeleton,
    kneel: &Skeleton,
    kind: Kind,
    f: Fall,
    c0: Vec3,
    u: f32,
) -> Skeleton {
    use Ease::Both;
    let r = ry(f.yaw);
    let ri = r.inverse();
    // Into the fall's frame (on the ground between the feet, lying toward +Z), and back.
    let into = |b: Bone| Bone::new(ri * (b.pos - c0), ri * b.rot);
    let back = |b: Bone| Bone::new(c0 + r * b.pos, r * b.rot);
    let sgn = |k: usize| if k == 0 { -1.0 } else { 1.0 };
    let euler = |q: Quat| q.to_euler(EulerRot::YXZ);
    let (l, k) = (lying, kneel);
    let along = |keys: &[(f32, f32)]| {
        let keys: Vec<(f32, f32, Ease)> = keys.iter().map(|&(t, v)| (t, v, Both)).collect();
        track(u, &keys)
    };
    // Which hand takes the weapon up (it lies on that side), when; a shield is taken up
    // first.
    let holder = if kind == Kind::Bow { 0 } else { 1 };
    let side = sgn(holder);
    let shielded = kind == Kind::SwordShield;
    let grab = if shielded { 0.5 } else { 0.4 };
    let grab_shield = 0.28;

    // ---------------------------------------------------------------- trunk
    // (Its hips and chest `u` of the way.)
    let trunk = |u: f32| {
        let along = |keys: &[(f32, f32)]| {
            let keys: Vec<(f32, f32, Ease)> = keys.iter().map(|&(t, v)| (t, v, Both)).collect();
            track(u, &keys)
        };
        let (p0, pk) = (into(l.pelvis), into(k.pelvis));
        let ((p0y, p0x, p0z), (pky, pkx, pkz)) = (euler(p0.rot), euler(pk.rot));
        let at = v3(
            along(&[
                (0.0, p0.pos.x),
                (0.4, side * 0.12),
                (0.75, 0.5 * (side * 0.06 + pk.pos.x)),
                (1.0, pk.pos.x),
            ]),
            along(&[(0.0, p0.pos.y), (0.4, 0.15), (0.75, 0.42), (1.0, pk.pos.y)]),
            along(&[(0.0, p0.pos.z), (0.4, 0.42), (0.75, 0.15), (1.0, pk.pos.z)]),
        );
        let pelvis = Bone::new(
            at,
            ry(along(&[
                (0.0, p0y),
                (0.4, side * 0.35),
                (0.75, 0.7 * pky + side * 0.15),
                (1.0, pky),
            ])) * rx(along(&[(0.0, p0x), (0.4, 0.85), (0.75, 0.25), (1.0, pkx)]))
                * rz(along(&[
                    (0.0, p0z),
                    (0.4, -side * 0.12),
                    (0.75, 0.0),
                    (1.0, pkz),
                ])),
        );
        // Sitting up leaning back, rolled toward the weapon (toward the shield first); leaning
        // forward over the knee as it comes up onto it.
        let (c0e, cke) = (euler(into(l.chest).rot), euler(into(k.chest).rot));
        let roll = if shielded {
            along(&[
                (0.0, c0e.2),
                (0.25, side * 0.35),
                (0.5, -side * 0.3),
                (0.75, -side * 0.05),
                (1.0, cke.2),
            ])
        } else {
            along(&[
                (0.0, c0e.2),
                (0.4, -side * 0.3),
                (0.75, -side * 0.05),
                (1.0, cke.2),
            ])
        };
        let turn = if shielded {
            along(&[
                (0.0, c0e.0),
                (0.25, -side * 0.3),
                (0.5, side * 0.55),
                (0.75, 0.6 * cke.0 + side * 0.2),
                (1.0, cke.0),
            ])
        } else {
            along(&[
                (0.0, c0e.0),
                (0.4, side * 0.55),
                (0.75, 0.6 * cke.0 + side * 0.2),
                (1.0, cke.0),
            ])
        };
        let chest_rot = ry(turn)
            * rx(along(&[
                (0.0, c0e.1),
                (0.25, 0.9),
                (0.42, 0.45),
                (0.75, -0.25),
                (1.0, cke.1),
            ]))
            * rz(roll);
        // The breath's rise of the chest on the knee, coming in at the end.
        let lift = into(k.chest).pos - pk.at(rig::WAIST);
        let chest = Bone::new(pelvis.at(rig::WAIST) + lift * step(0.6, 1.0, u), chest_rot);
        (pelvis, chest)
    };
    // How the kneel holds the weapon (and the shield) in hand, and where the hand goes to take
    // it up where it lies.
    let grip_of =
        |h: Bone, w: Bone| Bone::new(h.rot.inverse() * (w.pos - h.pos), h.rot.inverse() * w.rot);
    let to_take = |w: Bone, g: Bone| {
        let inv = g.rot.inverse();
        into(w).then(Bone::new(-(inv * g.pos), inv))
    };
    let held = grip_of(k.hand[holder], k.weapon);
    let shield_held = grip_of(k.hand[0], k.shield);
    // (A weapon lying farther from the hand is taken up a little later, the hand getting
    // there no faster.)
    let grab = if shielded || kind == Kind::Fists {
        grab
    } else {
        let far = (to_take(l.weapon, held).pos - into(l.hand[holder]).pos).length();
        grab + 0.4 * (far - 0.7).clamp(0.0, 0.5)
    };
    // A weapon lying beyond the arm's reach when the hand gets to it (one a reviver set down,
    // fallen from the kneel with the mate well off to one side): the chest bends over toward
    // it as the hand goes for it, as far as the arm falls short, so the hand gets there rather
    // than the weapon being drawn in over the ground; then it straightens over the knee.
    let (pelvis, chest) = trunk(u);
    let bend = if set_down(kind).is_none() {
        Quat::IDENTITY
    } else {
        let (_, at_grab) = trunk(grab);
        let (waist, sh) = (at_grab.pos, at_grab.at(shoulder(holder)));
        let to = to_take(l.weapon, held).pos - sh;
        let arm = rig::UPPER_ARM + rig::FOREARM - 0.03;
        let short = (to.length() - arm).max(0.0);
        let lever = sh - waist;
        let axis = lever.cross(to).normalize_or_zero();
        let angle = (short / lever.length()).min(0.6);
        Quat::from_axis_angle(
            axis,
            angle * step(0.0, grab, u) * (1.0 - step(grab, 1.0, u)),
        )
    };
    let chest = Bone::new(chest.pos, bend * chest.rot);
    // The head looks at the weapon as the hand goes for it.
    let rel = |c: Bone, h: Bone| euler(c.rot.inverse() * h.rot);
    let (h0, hk) = (rel(l.chest, l.head), rel(k.chest, k.head));
    let head = Bone::new(
        chest.at(rig::NECK),
        chest.rot
            * ry(along(&[(0.0, h0.0), (0.35, side * 0.4), (1.0, hk.0)]))
            * rx(along(&[
                (0.0, h0.1),
                (0.35, -0.35),
                (0.75, -0.15),
                (1.0, hk.1),
            ]))
            * rz(along(&[(0.0, h0.2), (0.4, 0.0), (1.0, hk.2)])),
    );
    let mut s = Skeleton {
        pelvis,
        chest,
        head,
        ..*k
    };

    // ---------------------------------------------------------------- legs
    // The leg it kneels on (the right) folds in under it, the knee down out to the side; the
    // other is drawn in, the knee up and the foot planted; then each goes to where the kneel
    // has it, lifted a little on the way. The knees go where they are led (from where they
    // lay, to drawn in, to where the kneel has them), so they never turn over.
    let down = if k.shin[0].pos.y < k.shin[1].pos.y {
        0
    } else {
        1
    };
    for j in 0..2 {
        let sg = sgn(j);
        let (a0, ak) = (into(l.foot[j]), into(k.foot[j]));
        let (drawn, bent) = if j == down {
            (
                v3(-sg * 0.02 + side * 0.06, rig::ANKLE_Y, 0.36),
                v3(sg * 0.32 + side * 0.06, 0.2, 0.12),
            )
        } else {
            (
                v3(sg * 0.12 + side * 0.06, rig::ANKLE_Y, 0.08),
                v3(sg * 0.16 + side * 0.06, 0.45, 0.12),
            )
        };
        let x = step(0.0, 0.35, u);
        let y = step(0.4, 0.8, u);
        let lift = 0.06 * (PI * ((u - 0.4) / 0.4).clamp(0.0, 1.0)).sin();
        let target = a0.pos.lerp(drawn, x).lerp(ak.pos, y) + Vec3::Y * lift;
        let rot = a0.rot.slerp(ry(side * 0.2), x).slerp(ak.rot, y);
        let hip = pelvis.at(v3(sg * rig::HIP_X, 0.0, 0.0));
        let knee = into(l.shin[j])
            .pos
            .lerp(bent, x)
            .lerp(into(k.shin[j]).pos, y);
        let legs = (rig::THIGH, rig::SHIN);
        let across = pelvis.rot * Vec3::X;
        let (_, shin, ankle) = chain(hip, target, legs, knee - (hip + target) * 0.5, across, true);
        // Where the knee would go into the ground (the hips still low), it goes down onto the
        // ground instead, the way it was headed, and the foot trails from it.
        const KNEE_DOWN: f32 = 0.06;
        let onto = 1.0 - step(KNEE_DOWN - 0.04, KNEE_DOWN, shin.pos.y);
        let (knee, foot) = if onto > 0.0 {
            let drop = (hip.y - KNEE_DOWN).min(rig::THIGH - 1e-3);
            let way = Vec2::new(shin.pos.x - hip.x, shin.pos.z - hip.z).normalize_or(Vec2::NEG_Y);
            let out = (rig::THIGH * rig::THIGH - drop * drop).sqrt();
            let on = hip + v3(way.x * out, -drop, way.y * out);
            let trail = on + (target - on).normalize_or(Vec3::Z) * rig::SHIN;
            let knee = hip + (shin.pos.lerp(on, onto) - hip).normalize() * rig::THIGH;
            let foot = knee + (ankle.lerp(trail, onto) - knee).normalize() * rig::SHIN;
            (knee, foot)
        } else {
            (shin.pos, ankle)
        };
        let (thigh, shin, ankle) = chain(hip, foot, legs, knee - (hip + foot) * 0.5, across, true);
        s.thigh[j] = thigh;
        s.shin[j] = shin;
        s.foot[j] = Bone::new(ankle, rot);
    }
    // The front tabard: lying over the legs; sitting up with the knees drawn in, fallen
    // between them (along the line of the hips, not standing up along the thighs); on the
    // knee, riding on the forward thigh. The back one lies under the hips until they lift.
    let lie = tabards(pelvis, s.thigh, s.shin, (0.0, 0.0), 1.0);
    let ride = tabards(pelvis, s.thigh, s.shin, (0.0, 0.0), 0.0);
    let hang = pelvis.then(Bone::new(v3(0.0, 0.085, -0.128), rx(0.04)));
    s.tabard = [
        lie[0]
            .mix(hang, step(0.0, 0.2, u))
            .mix(ride[0], step(0.55, 0.85, u)),
        lie[1].mix(ride[1], step(0.4, 0.8, u)),
    ];

    // ---------------------------------------------------------------- arms
    for j in 0..2 {
        let sg = sgn(j);
        let sh = chest.at(shoulder(j));
        let (w0, wk) = (into(l.hand[j]), into(k.hand[j]));
        let (target, rot) = if (j == holder && kind != Kind::Fists) || (shielded && j == 0) {
            // Reaching for it, taking it up, and bringing it round (lifted on the way).
            // (The shield, lying on its face, by its rim: it comes up into the grip.)
            let (take, when) = if j == holder {
                (to_take(l.weapon, held), grab)
            } else {
                let rim = to_take(l.shield, shield_held);
                (Bone::new(rim.pos + Vec3::Y * 0.07, rim.rot), grab_shield)
            };
            if u < when {
                // (Reaching sooner for one lying farther off.)
                let far = (take.pos - w0.pos).length();
                let x = step((when - 0.25 - 0.3 * (far - 0.5).max(0.0)).max(0.0), when, u);
                (w0.pos.lerp(take.pos, x), w0.rot.slerp(take.rot, x))
            } else {
                let x = step(when, 1.0, u);
                let arc = 0.15 * (PI * x).sin();
                (
                    take.pos.lerp(wk.pos, x) + Vec3::Y * arc,
                    take.rot.slerp(wk.rot, x),
                )
            }
        } else {
            // Pushing off the ground behind, then in front, then to where the kneel has it
            // (bare fists, both hands).
            let behind = v3(sg * 0.3, WRISTS_DOWN, 0.55);
            let ahead = v3(sg * 0.22, WRISTS_DOWN, 0.12);
            let pos = w0
                .pos
                .lerp(behind, step(0.05, 0.3, u))
                .lerp(ahead, step(0.42, 0.62, u))
                .lerp(wk.pos, step(0.7, 1.0, u));
            (pos, w0.rot.slerp(wk.rot, step(0.5, 1.0, u)))
        };
        // As far as the arm reaches (a hand left behind is drawn along).
        let arm = rig::UPPER_ARM + rig::FOREARM - 0.015;
        let target = sh + (target - sh).clamp_length_max(arm);
        let elbow = |b: &Skeleton, w: Vec3| into(b.fore[j]).pos - (into(b.upper[j]).pos + w) * 0.5;
        let reach = target - sh;
        // (Bare fists pushing off with both hands: the elbows kept up off the ground.)
        let out = v3(sg * 0.8, if kind == Kind::Fists { 0.3 } else { 0.2 }, 0.3);
        let pole = about(reach, elbow(l, w0.pos), out, step(0.0, 0.3, u));
        let pole = about(reach, pole, elbow(k, wk.pos), step(0.55, 1.0, u));
        let (upper, fore, wrist) = chain(
            sh,
            target,
            (rig::UPPER_ARM, rig::FOREARM),
            pole,
            chest.rot * Vec3::X,
            false,
        );
        s.upper[j] = upper;
        s.fore[j] = fore;
        s.hand[j] = Bone::new(wrist, rot);
        s.want[j] = target;
    }
    // Each comes up into the hand that took it (settling into the grip; the body leans over
    // to one lying out of reach, so the hand gets there first).
    let short = (to_take(l.weapon, held).pos - chest.at(shoulder(holder))).length()
        - (rig::UPPER_ARM + rig::FOREARM - 0.015);
    let taken = step(grab, grab + 0.08 + 0.5 * short.max(0.0), u);
    s.weapon = into(l.weapon).mix(s.hand[holder].then(held), taken);
    s.shield = if !shielded {
        s.weapon
    } else {
        let taken = step(grab_shield, grab_shield + 0.12, u);
        into(l.shield).mix(s.hand[0].then(shield_held), taken)
    };
    if u < 1.0 {
        (s.bend, s.draw, s.nocked, s.held) = (k.bend, 0.0, false, None);
    }
    Skeleton {
        pelvis: back(s.pelvis),
        chest: back(s.chest),
        head: back(s.head),
        thigh: s.thigh.map(&back),
        shin: s.shin.map(&back),
        foot: s.foot.map(&back),
        upper: s.upper.map(&back),
        fore: s.fore.map(&back),
        hand: s.hand.map(&back),
        tabard: s.tabard.map(&back),
        want: s.want.map(|p| c0 + r * p),
        weapon: back(s.weapon),
        shield: back(s.shield),
        ..s
    }
}

/// Downed: the body falls from its standing pose `s` (see [`fall_pose`]), and the weapon and
/// the shield slip from the hands as the knees give and drop beside it.
fn fallen(s: &mut Skeleton, kind: Kind, f: Fall, knelt: Option<&Skeleton>) {
    let s0 = *s;
    let from = knelt.unwrap_or(&s0);
    *s = fall_pose(from, &s0, kind, f, f.t);
    // A weapon set down by a mate stays where it lies.
    let laid = knelt
        .filter(|k| set_down(kind).is_some() && k.weapon.pos.y < 0.1)
        .map(|k| k.weapon);
    if let Some(w) = laid {
        s.weapon = w;
        if kind != Kind::SwordShield {
            s.shield = w;
        }
    }
    if f.t <= LET_GO || kind == Kind::Fists {
        return;
    }
    let held = fall_pose(from, &s0, kind, f, LET_GO);
    let mid = (s0.foot[0].pos + s0.foot[1].pos) * 0.5;
    let (r, c0) = (ry(f.yaw), v3(mid.x, 0.0, mid.z));
    // Where they come to lie (in the fall's frame): blades and carbines on their sides, the
    // shield on its back, the bow flat; out wide with the arm flung out (from the hand that
    // held it), else let go of beside the thigh, along the body (a squad mate kneels on that
    // side: see [`Lie::kneel_by`]).
    let lying = |at: Vec3, rot: Quat| Bone::new(c0 + r * at, r * rot);
    let wide = |hand: usize| f.flung == hand;
    let weapon = match kind {
        Kind::Rifle if wide(1) => lying(v3(0.6, 0.03, 0.7), ry(-0.4) * rz(FRAC_PI_2)),
        Kind::Rifle => lying(DROP_RIFLE, ry(-0.1) * rz(FRAC_PI_2)),
        Kind::Bow if wide(0) => lying(v3(-0.6, 0.03, 0.8), ry(0.3) * rx(-FRAC_PI_2)),
        Kind::Bow => lying(DROP_BOW, ry(0.33) * rz(FRAC_PI_2) * rx(-FRAC_PI_2)),
        _ if wide(1) => lying(v3(0.62, 0.02, 0.75), ry(-0.5) * rz(FRAC_PI_2)),
        _ => lying(DROP_BLADE, ry(-0.12) * rz(FRAC_PI_2)),
    };
    if laid.is_none() {
        s.weapon = dropped(held.weapon, weapon, f.t - LET_GO);
    }
    if kind == Kind::SwordShield {
        let shield = if wide(0) {
            lying(v3(-0.75, 0.02, 0.45), ry(0.3) * rx(FRAC_PI_2))
        } else {
            lying(DROP_SHIELD, ry(0.15) * rx(FRAC_PI_2))
        };
        s.shield = dropped(held.shield, shield, f.t - LET_GO - 0.05);
    }
}

/// Where what is let go of beside the thigh lies (see [`fallen`]; in the fall's frame): a
/// blade, a carbine, a bow and a shield.
const DROP_BLADE: Vec3 = Vec3::new(0.5, 0.02, 0.0);
const DROP_RIFLE: Vec3 = Vec3::new(0.5, 0.03, -0.05);
const DROP_BOW: Vec3 = Vec3::new(-0.55, 0.03, -0.15);
const DROP_SHIELD: Vec3 = Vec3::new(-0.62, 0.02, -0.25);

/// Something let go of as `from` `e` s ago, falling to lie as `to`: across at an even pace,
/// down ever faster, turning as it goes; a little hop as it lands.
fn dropped(from: Bone, to: Bone, e: f32) -> Bone {
    if e <= 0.0 {
        return from;
    }
    let secs = (2.0 * (from.pos.y - to.pos.y).max(0.05) / 9.81)
        .sqrt()
        .max(0.2);
    let x = (e / secs).min(1.0);
    let mut pos = from.pos.lerp(to.pos, x);
    pos.y = lerp(from.pos.y, to.pos.y, x * x);
    let after = e - secs;
    if (0.0..0.14).contains(&after) {
        pos.y += 0.03 * (PI * after / 0.14).sin();
    }
    Bone::new(pos, from.rot.slerp(to.rot, ease(x)))
}

// ---------------------------------------------------------------------------- reviving a mate

/// Where the right hand sets the weapon down to revive a mate (kneel frame: the knees at the
/// origin, the mate's chest toward −Z): on the ground beside the right knee, on its side, the
/// point or muzzle back and out, clear of the mate.
fn set_down(kind: Kind) -> Option<Bone> {
    let back = |turn: f32| ry(PI + turn) * rz(FRAC_PI_2);
    match kind {
        Kind::Rifle => Some(Bone::new(v3(0.42, 0.03, 0.34), back(0.35))),
        Kind::Longsword => Some(Bone::new(v3(0.42, 0.02, 0.3), back(0.3))),
        Kind::SwordShield => Some(Bone::new(v3(0.42, 0.02, 0.3), back(0.35))),
        _ => None,
    }
}

/// What the left hand holds aside instead (kneel frame): a bow or a shield, upright beside
/// the left knee, its foot on the ground.
fn held_aside(kind: Kind) -> Option<Bone> {
    match kind {
        // The string toward the archer.
        Kind::Bow => Some(Bone::new(v3(-0.4, 0.68, 0.18), ry(FRAC_PI_2) * rz(-0.08))),
        // The face out to the left.
        Kind::SwordShield => Some(Bone::new(v3(-0.42, 0.55, 0.16), ry(FRAC_PI_2) * rz(-0.1))),
        _ => None,
    }
}

/// A frame `b` as seen from frame `a`.
fn relative(a: Bone, b: Bone) -> Bone {
    let q = a.rot.inverse();
    Bone::new(q * (b.pos - a.pos), q * b.rot)
}

/// The frame that undoes `b`.
fn undo(b: Bone) -> Bone {
    let q = b.rot.inverse();
    Bone::new(-(q * b.pos), q)
}

/// Reviving a downed mate (see [`Tend`]), from standing as the input has it: down on both
/// knees beside its chest; the weapon set down on the ground beside the right knee (sat back
/// on the heels and leaning over to it), or a bow or a shield held aside in the left hand; the
/// hands on the chest, the body over them, pressing; the head bowed over it, coming up when
/// the eyes go to a threat (the pressing waits meanwhile). Getting up again plays it back.
fn tended(i: &Input, t: Tend) -> Skeleton {
    let s0 = stand(&Input {
        crouch: false,
        rising: 0.0,
        tend: None,
        ..*i
    });
    let kind = i.kind;
    let k = t.k.clamp(0.0, 1.0);
    let laid = set_down(kind);
    let aside = held_aside(kind);
    // Down on the knees; the weapon going down (sat back, leaning over to it); the hands on.
    let down = step(0.0, 0.55, k);
    let lay = if laid.is_some() {
        step(0.15, 0.7, k)
    } else {
        0.0
    };
    let on = if laid.is_some() {
        step(0.7, 1.0, k)
    } else {
        step(0.4, 1.0, k)
    };
    let bend = step(0.0, 0.8, lay) * (1.0 - on);
    // Glancing up (the eyes well above the chest, however far off it kneels), and the
    // pressing (waiting meanwhile).
    let to_chest = (t.at.y - TEND_EYES).atan2(Vec2::new(t.at.x, t.at.z).length().max(0.3));
    let glance = step(0.25, 0.6, i.look.head.y - to_chest) * i.look.focus * on;
    let beat = ((i.now * f64::from(PRESS_HZ)).fract() as f32 * TAU).cos();
    let press = on * (1.0 - glance) * (0.5 - 0.5 * beat);

    // The kneel's frame: the knees where they go beside the mate (as far as the body kneels
    // off the owner's place), facing its chest.
    let knees = Vec2::new(t.knees.x, t.knees.z).clamp_length_max(KNEEL_REACH);
    let dir = (Vec2::new(t.at.x, t.at.z) - knees).normalize_or(Vec2::NEG_Y);
    let r = ry((-dir.x).atan2(-dir.y));
    let o = v3(knees.x, 0.0, knees.y);
    let at = |p: Vec3| o + r * p;
    let frame = |b: Bone| Bone::new(at(b.pos), r * b.rot);
    let mate = r.inverse() * (t.at - o);

    // ---------------------------------------------------------------- trunk
    // Up on the knees; sat back on the heels to set the weapon down (the hips going round the
    // knees, which stay on the ground); then over the chest, the shoulders over the hands.
    let side = if laid.is_some() { 1.0 } else { 0.0 };
    let (over, bow) = over_chest(mate);
    let back = lerp(lerp(0.03, over, on), 0.92, bend);
    let mut hips = Bone::new(
        v3(side * 0.1 * bend, 0.05, 0.0) + v3(0.0, back.cos(), back.sin()) * 0.448,
        rx(-lerp(0.2, 0.45, bend)),
    );
    hips.pos.y -= 0.012 * press;
    let pelvis = s0.pelvis.mix(frame(hips), down);
    // Leaning over the chest (and, sat back, over to the weapon's side).
    let lean = 0.3 * down + 0.6 * bend + (bow - 0.3) * on - 0.15 * glance + 0.05 * press;
    let waist = pelvis.at(rig::WAIST) + (s0.chest.pos - s0.pelvis.at(rig::WAIST)) * (1.0 - down);
    let chest = Bone::new(
        waist,
        s0.chest.rot.slerp(
            r * ry(-side * 0.35 * bend) * rx(-lean) * rz(-side * 0.6 * bend),
            down,
        ),
    );
    // The head bowed over the chest (turned to the weapon going down), up at a threat.
    let ahead = chest.rot * Vec3::NEG_Z;
    let (cy, cp) = ((-ahead.x).atan2(-ahead.z), ahead.y.clamp(-1.0, 1.0).asin());
    let look_yaw = wrap(i.look.head.x - cy).clamp(-NECK_YAW, NECK_YAW);
    let look_pitch = (cp - i.look.head.y).clamp(-0.75, NECK_PITCH);
    let head_yaw = lerp(-side * 0.5 * bend, look_yaw, glance);
    let head_pitch = lerp(0.3 + 0.15 * bend, look_pitch, glance);
    let bowed = ry(head_yaw) * rx(-head_pitch);
    let head = Bone::new(
        chest.at(rig::NECK),
        chest.rot
            * (s0.chest.rot.inverse() * s0.head.rot)
                .slerp(bowed, down)
                .normalize(),
    );
    let mut s = Skeleton {
        pelvis,
        chest,
        head,
        ..s0
    };
    (s.eyes, s.lids) = eyes_on(head, &i.look, i.lids);

    // ---------------------------------------------------------------- legs
    // Both knees down, the toes tucked under, the feet behind.
    for j in 0..2 {
        let sg = if j == 0 { -1.0 } else { 1.0 };
        let hip = pelvis.at(v3(sg * rig::HIP_X, 0.0, 0.0));
        let kneel = frame(Bone::new(
            v3(sg * 0.15, 0.115, 0.43),
            ry(-sg * 0.15) * rx(-1.25),
        ));
        let foot = s0.foot[j].mix(kneel, down);
        // The knees bend forward (down onto the ground, kneeling up; off it a little, sat
        // back).
        let toes = s0.foot[j].rot * Vec3::NEG_Z;
        let ahead = v3(toes.x, 0.0, toes.z).normalize_or(Vec3::NEG_Z) + v3(0.0, 0.1, 0.0);
        let (thigh, shin, ankle) = chain(
            hip,
            foot.pos,
            (rig::THIGH, rig::SHIN),
            ahead.lerp(r * v3(sg * 0.1, -0.2, -1.0), down),
            foot.rot * Vec3::X,
            true,
        );
        s.thigh[j] = thigh;
        s.shin[j] = shin;
        s.foot[j] = Bone::new(ankle, foot.rot);
    }
    s.tabard = tabards(pelvis, s.thigh, s.shin, (0.0, 0.0), 0.0);

    // ---------------------------------------------------------------- arms
    // Where each hand goes on the chest (fists side by side, the knuckles forward; one alone
    // in the middle), pressing.
    let hands_on = |j: usize| {
        let sg = if j == 0 { -1.0 } else { 1.0 };
        let (rot, palm) = if j == 0 {
            (
                basis(Vec3::NEG_Y, Vec3::Z, Vec3::NEG_X),
                v3(0.045, -0.075, 0.0),
            )
        } else {
            (basis(Vec3::Y, Vec3::Z, Vec3::X), v3(-0.045, -0.075, 0.0))
        };
        let apart = if aside.is_some() { 0.0 } else { 0.055 };
        let on = mate + v3(sg * apart, -PRESS * press, 0.0);
        frame(Bone::new(on - rot * palm, rot))
    };
    // How the stance held each hand, the weapon and the shield, carried with the chest.
    let carry = |b: Bone| chest.then(relative(s0.chest, b));
    let held = relative(s0.hand[1], s0.weapon);
    let (item, held_item) = if kind == Kind::Bow {
        (s0.weapon, relative(s0.hand[0], s0.weapon))
    } else {
        (s0.shield, relative(s0.hand[0], s0.shield))
    };
    let mut wants = [Bone::default(); 2];
    for (j, want) in wants.iter_mut().enumerate() {
        let carried = carry(s0.hand[j]);
        *want = if let (1, Some(ground)) = (j, laid) {
            // The weapon goes down beside the knee (out and over on the way), then the hand
            // goes to the chest.
            let w = carry(s0.weapon).mix(frame(ground), lay);
            let w = Bone::new(w.pos + r * v3(0.1, 0.06, 0.0) * (PI * lay).sin(), w.rot);
            let grip = w.then(undo(held));
            let to = grip.mix(hands_on(j), on);
            Bone::new(to.pos + Vec3::Y * 0.1 * (PI * on).sin(), to.rot)
        } else if let (0, Some(by)) = (j, aside) {
            // Holding the bow or the shield aside.
            let w = carry(item).mix(frame(by), ease(step(0.1, 0.6, k)));
            w.then(undo(held_item))
        } else {
            // A free hand rests on its thigh while the weapon goes down, then goes to the
            // chest.
            let rest = if laid.is_some() {
                step(0.1, 0.4, k)
            } else {
                0.0
            };
            let thigh = s.thigh[j].at(v3(0.0, -0.3, 0.0)) + Vec3::Y * 0.09;
            carried
                .mix(Bone::new(thigh, carried.rot), rest)
                .mix(hands_on(j), on)
        };
    }
    for (j, want) in wants.into_iter().enumerate() {
        let sg = if j == 0 { -1.0 } else { 1.0 };
        let sh = chest.at(shoulder(j));
        // As far as the arm reaches (what it holds comes along).
        let arm = rig::UPPER_ARM + rig::FOREARM - 0.015;
        let want = Bone::new(sh + (want.pos - sh).clamp_length_max(arm), want.rot);
        let elbow = s0.fore[j].pos - (s0.upper[j].pos + s0.hand[j].pos) * 0.5;
        let elbow = chest.rot * (s0.chest.rot.inverse() * elbow);
        let pole = about(want.pos - sh, elbow, r * v3(sg * 0.8, -0.3, 0.45), down);
        let (upper, fore, wrist) = chain(
            sh,
            want.pos,
            (rig::UPPER_ARM, rig::FOREARM),
            pole,
            chest.rot * Vec3::X,
            false,
        );
        s.upper[j] = upper;
        s.fore[j] = fore;
        s.hand[j] = Bone::new(wrist, want.rot);
        s.want[j] = want.pos;
    }
    // The weapon in hand, then lying where it was set down; a bow or a shield in hand.
    match (laid, kind) {
        (Some(ground), _) => {
            let ground = frame(ground);
            s.weapon = if on > 0.0 {
                ground
            } else {
                s.hand[1].then(held).mix(ground, step(0.8, 1.0, lay))
            };
        }
        (None, Kind::Fists) => s.weapon = chest,
        (None, _) => s.weapon = s.hand[0].then(held_item),
    }
    s.shield = match kind {
        Kind::SwordShield => s.hand[0].then(held_item),
        Kind::Fists => chest,
        _ => s.weapon,
    };
    // The string let down (an arrow nocked stays on it); a magazine in hand seated in the
    // carbine on its way down, an arrow in hand put back in the quiver.
    s.draw = s0.draw * (1.0 - down);
    s.bend = limb_bend(s.draw);
    let free = usize::from(kind == Kind::Bow);
    let away = step(0.05, 0.35, k);
    s.held = s0.held.filter(|_| away < 1.0).map(|h| {
        let in_hand = s.hand[free].then(relative(s0.hand[free], h));
        let put = if kind == Kind::Bow {
            let (mouth, bottom) = (weapons::QUIVER_MOUTH, weapons::QUIVER_BOTTOM);
            chest.then(Bone::new(mouth, aim(bottom - mouth, Vec3::Z)))
        } else {
            s.weapon.then(Bone::new(weapons::RIFLE_MAG, Quat::IDENTITY))
        };
        in_hand.mix(put, away)
    });
    s
}

/// How a reviver kneels over a downed mate's chest at `mate` (kneel frame: the knees at the
/// origin) to press on it: how far the hips swing round the knees (rad, back positive; see
/// [`tended`]) and how far the body leans over (rad), so the shoulders come over the hands with
/// the arms near straight, whether the chest is near the knees or a little farther off.
fn over_chest(mate: Vec3) -> (f32, f32) {
    let s = rig::SHOULDER;
    let (rad, phase) = (Vec2::new(s.y, s.z).length(), s.z.atan2(s.y));
    // The wrists over the breastbone (see the hands on the chest), and the shoulders a little
    // behind and above them.
    let wrists = mate + v3(0.0, 0.045, 0.075);
    let (spread, arms, behind) = (s.x - 0.055, 0.93 * (rig::UPPER_ARM + rig::FOREARM), 0.04);
    let up = (arms * arms - spread * spread - behind * behind)
        .max(0.0)
        .sqrt();
    let want = wrists + v3(0.0, up, behind);
    // The lean that brings the shoulders to that height with the hips swung `back`, and how
    // far toward the chest they are then.
    let tilt = rx(-0.2);
    let lean_at = |back: f32| {
        let hips = v3(0.0, 0.05 + 0.448 * back.cos(), 0.448 * back.sin());
        let waist = hips + tilt * rig::WAIST;
        let lean = phase + ((want.y - waist.y) / rad).clamp(-1.0, 1.0).acos();
        (lean, waist.z - s.y * lean.sin() + s.z * lean.cos())
    };
    let (mut lo, mut hi) = (-0.7f32, 0.6f32);
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        if lean_at(mid).1 < want.z {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let back = 0.5 * (lo + hi);
    (back, lean_at(back).0)
}

/// How far the bow's limbs bend (radians) for the string pulled `draw` past brace height:
/// the string keeps its length.
fn limb_bend(draw: f32) -> f32 {
    if draw <= 0.0 {
        return 0.0;
    }
    let nock = weapons::BOW_TIP.z + draw;
    let gap = |a: f32| {
        let tip = bow_tip(a);
        Vec2::new(tip.y, nock - tip.z).length() - STRING_HALF
    };
    // The gap shrinks as the limbs bend; bisect for zero.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..18 {
        let m = 0.5 * (lo + hi);
        if gap(m) > 0.0 {
            lo = m;
        } else {
            hi = m;
        }
    }
    0.5 * (lo + hi)
}

/// The upper string tip for limbs bent by `a`, in the bow's frame.
fn bow_tip(a: f32) -> Vec3 {
    let pivot = weapons::BOW_LIMB_PIVOT;
    pivot + rx(a) * (weapons::BOW_TIP - pivot)
}

// ---------------------------------------------------------------------------- weapon poses

/// A sword's grip in the chest frame: where, the blade's direction and the edge's.
fn blade(pos: Vec3, fwd: Vec3, up: Vec3) -> Bone {
    Bone::new(pos, aim(fwd, up))
}

/// The key after `from`: the grip at `pos`, the blade turned the short way to point along
/// `dir` (so through a chain of keys the edge keeps leading).
fn turn(from: Bone, (pos, dir): (Vec3, Vec3)) -> Bone {
    let d = from.rot * Vec3::NEG_Z;
    Bone::new(pos, Quat::from_rotation_arc(d, dir.normalize()) * from.rot)
}

/// Along a chain of keys at `s` (0 the first, 1 the next...): the grip on a smooth curve
/// through them, the blade turning from each to the next.
fn path<const N: usize>(keys: &[Bone; N], s: f32) -> Bone {
    let s = s.clamp(0.0, (N - 1) as f32);
    let i = (s as usize).min(N - 2);
    let x = s - i as f32;
    let p = |j: usize| keys[j.min(N - 1)].pos;
    let (p0, p1, p2, p3) = (p(i.saturating_sub(1)), p(i), p(i + 1), p(i + 2));
    let pos = 0.5
        * (2.0 * p1
            + (p2 - p0) * x
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * x * x
            + (3.0 * p1 - p0 - 3.0 * p2 + p3) * x * x * x);
    Bone::new(pos, keys[i].rot.slerp(keys[i + 1].rot, x))
}

/// How far along a telegraph's keys (guard, dip, halfway up, peak, cocked, impact) it is at
/// `p`: a small dip, up quickly to the peak, held there cocked (the tell), then the release
/// accelerating into the hit at the end.
fn windup_keys(p: f32, heavy: bool) -> f32 {
    let top = if heavy { 0.5 } else { 0.6 };
    if p < 0.12 {
        step(0.0, 0.12, p)
    } else if p < top {
        let x = (p - 0.12) / (top - 0.12);
        1.0 + 2.0 * x * (2.0 - x)
    } else if p < 0.88 {
        3.0 + step(top, 0.88, p)
    } else {
        let x = (p - 0.88) / 0.12;
        4.0 + x * x
    }
}

/// How far along a cut's keys (impact, follow, follow-through, settle, guard) it is `t` s into
/// the recovery (`dur` s long): fast through the follow, carried past it, then back.
fn strike_keys(t: f32, dur: f32) -> f32 {
    let back = (0.28 * dur / 0.45).max(0.2);
    if t < 0.06 {
        let x = t / 0.06;
        x * (2.0 - x)
    } else if t < 0.15 {
        let x = (t - 0.06) / 0.09;
        1.0 + x * (2.0 - x)
    } else if t < back {
        2.0 + step(0.15, back, t)
    } else {
        3.0 + step(back, dur, t)
    }
}

/// Grip positions and blade directions of a melee weapon's moves, in the chest frame.
struct Moves {
    /// Telegraph from the guard: dip, halfway up; light peak and cocked; heavy peak and cocked.
    up: [(Vec3, Vec3); 2],
    light: [(Vec3, Vec3); 2],
    heavy: [(Vec3, Vec3); 2],
    /// The cut: impact, follow, follow-through, settle.
    cut: [(Vec3, Vec3); 4],
}

/// A melee weapon's key poses in the chest frame, each turned from the one before.
struct Blade {
    low: Bone,
    guard: Bone,
    windup: [Bone; 6],
    heavy: [Bone; 6],
    cut: [Bone; 5],
    heavy_cut: [Bone; 5],
    /// Against each [`Line`] (in its order): the guard raised to meet the blow, and beating
    /// it aside (a parry).
    lines: [(Bone, Bone); 5],
    stagger: Bone,
}

impl Blade {
    fn new(low: Bone, guard: Bone, m: &Moves, lines: [(Bone, Bone); 5], stagger: Bone) -> Self {
        let dip = turn(guard, m.up[0]);
        let half = turn(dip, m.up[1]);
        let chain = |peak: [(Vec3, Vec3); 2]| {
            let top = turn(half, peak[0]);
            let cocked = turn(top, peak[1]);
            let impact = turn(cocked, m.cut[0]);
            let follow = turn(impact, m.cut[1]);
            let through = turn(follow, m.cut[2]);
            let settle = turn(through, m.cut[3]);
            (
                [guard, dip, half, top, cocked, impact],
                [impact, follow, through, settle, guard],
            )
        };
        let (windup, cut) = chain(m.light);
        let (heavy, heavy_cut) = chain(m.heavy);
        Self {
            low,
            guard,
            windup,
            heavy,
            cut,
            heavy_cut,
            lines,
            stagger,
        }
    }

    /// The weapon's pose for `act`; on guard nudged by `jiggle` (−1..1 each way).
    fn at(&self, act: Act, jiggle: Vec2) -> Bone {
        match act {
            Act::Relaxed => self.low,
            Act::Guard => Bone::new(
                self.guard.pos + v3(0.006 * jiggle.x, 0.01 * jiggle.y, 0.0),
                ry(0.03 * jiggle.x) * rx(0.04 * jiggle.y) * self.guard.rot,
            ),
            Act::Windup { p, heavy } => {
                let keys = if heavy { &self.heavy } else { &self.windup };
                path(keys, windup_keys(p, heavy))
            }
            Act::Strike { t, dur, heavy } => {
                let keys = if heavy { &self.heavy_cut } else { &self.cut };
                path(keys, strike_keys(t, dur))
            }
            Act::Check { p, u, heavy } => {
                // Started from where the telegraph had got to into the release, stopped a
                // quarter of the way to the hit, then brought back to guard.
                let keys = if heavy { &self.heavy } else { &self.windup };
                path(keys, windup_keys(p, heavy))
                    .mix(keys[5], 0.27 * check_jab(u))
                    .mix(self.guard, check_back(u))
            }
            Act::Block {
                k,
                hit,
                parry,
                line,
            } => {
                let (block, deflect) = self.lines[line as usize];
                let up = self.guard.mix(block, k);
                if parry {
                    // Meeting the blow, then beating it aside.
                    let beat = step(-0.02, 0.05, hit) * (1.0 - step(0.12, 0.3, hit));
                    up.mix(deflect, beat * k)
                } else {
                    // Driven back along the line as the blow lands.
                    let (push, tip) = line.recoil(recoil(hit) * k);
                    Bone::new(up.pos + push, tip * up.rot)
                }
            }
            Act::Stagger { k, .. } => self.guard.mix(self.stagger, k),
        }
    }
}

/// Elbows: low and out at the guard, up and out with the hands high.
fn melee_poles(chest: Bone, hands_y: f32) -> [Vec3; 2] {
    let r = step(0.15, 0.5, hands_y);
    [
        chest.rot * v3(-0.6, -1.0, 0.4).lerp(v3(-0.7, -0.4, -0.3), r),
        chest.rot * v3(0.6, -1.0, 0.4).lerp(v3(1.0, -0.2, 0.3), r),
    ]
}

/// A key swung about shoulder `k` by `a` radians (an arm swinging as it walks).
fn swung(b: Bone, k: usize, a: f32) -> Bone {
    let pivot = shoulder(k);
    Bone::new(pivot + rx(a) * (b.pos - pivot), rx(a) * b.rot)
}

/// Two hands on the longsword: a diagonal cut from high over the right shoulder (a heavy one
/// from higher and further back). Guards meet each line: against a blow from above, a roof
/// over the head (the hands up over the helmet, the pommel hand on its left, the elbows high
/// and out and the blade sloping across above it and down to the right, so the forearms
/// leave the face clear; shedding the blow off to the right on a parry), upright on the side a cut comes from (beating it out wide), the
/// point low across a thrust (sweeping it aside). At ease it rests on the right shoulder (or,
/// `low`, at the low ready: low in the right hand, the point ahead), the left arm swinging.
fn longsword(act: Act, chest: Bone, swing: f32, jiggle: Vec2, low: bool) -> Arms {
    let moves = Moves {
        up: [
            (v3(0.04, 0.05, -0.32), v3(-0.08, 0.3, -0.95)),
            (v3(0.13, 0.32, -0.22), v3(0.1, 0.97, -0.2)),
        ],
        light: [
            (v3(0.17, 0.46, -0.05), v3(0.3, 0.75, 0.6)),
            (v3(0.19, 0.48, 0.0), v3(0.35, 0.65, 0.68)),
        ],
        heavy: [
            (v3(0.21, 0.6, 0.02), v3(0.45, 0.72, 0.53)),
            (v3(0.23, 0.62, 0.06), v3(0.5, 0.6, 0.62)),
        ],
        cut: [
            (v3(0.02, 0.27, -0.46), v3(-0.3, 0.02, -0.95)),
            (v3(-0.08, 0.06, -0.32), v3(-0.62, -0.58, -0.52)),
            (v3(-0.13, 0.05, -0.25), v3(-0.8, -0.5, -0.3)),
            (v3(-0.06, 0.08, -0.3), v3(-0.58, -0.45, -0.68)),
        ],
    };
    let lines = [
        (
            blade(
                v3(0.03, 0.86, -0.1),
                v3(0.95, -0.22, -0.2),
                v3(0.0, -0.9, 0.4),
            ),
            blade(
                v3(0.06, 0.82, -0.16),
                v3(0.8, -0.45, -0.4),
                v3(0.0, -0.9, 0.4),
            ),
        ),
        (
            blade(v3(-0.1, 0.36, -0.33), v3(-0.25, 0.92, -0.3), Vec3::NEG_X),
            blade(v3(-0.2, 0.34, -0.3), v3(-0.75, 0.55, -0.35), Vec3::NEG_X),
        ),
        (
            blade(v3(0.18, 0.3, -0.32), v3(0.15, 0.95, -0.28), Vec3::X),
            blade(v3(0.26, 0.3, -0.3), v3(0.7, 0.6, -0.4), Vec3::X),
        ),
        (
            blade(
                v3(0.0, 0.14, -0.34),
                v3(-0.3, 0.3, -0.9),
                v3(-0.6, 0.8, 0.0),
            ),
            blade(
                v3(-0.06, 0.15, -0.36),
                v3(-0.8, 0.25, -0.55),
                v3(-0.3, 0.9, 0.0),
            ),
        ),
        (
            blade(v3(0.06, 0.4, -0.33), v3(-1.0, 0.18, -0.12), Vec3::Y),
            blade(v3(0.16, 0.36, -0.36), v3(-0.45, 0.8, -0.4), Vec3::Y),
        ),
    ];
    let w = Blade::new(
        blade(v3(0.08, 0.0, -0.27), v3(-0.1, -0.45, -0.88), Vec3::Y),
        blade(v3(0.05, 0.08, -0.3), v3(-0.06, 0.42, -0.9), Vec3::Y),
        &moves,
        lines,
        blade(v3(0.3, 0.22, -0.14), v3(0.45, 0.8, -0.3), Vec3::Z),
    )
    .at(act, jiggle);
    let relaxed = act == Act::Relaxed;
    let shouldered = relaxed && !low;
    let w = if shouldered {
        // The flat on the shoulder, the hand in front of it, the wrist cocked back.
        let carry = blade(
            v3(0.16, 0.26, -0.22),
            v3(0.05, 0.55, 0.83),
            v3(0.0, -0.52, 0.85),
        );
        swung(carry, 1, -0.15 * swing)
    } else if relaxed {
        swung(w, 1, -0.6 * swing)
    } else {
        w
    };
    let weapon = chest.then(w);
    let left = if relaxed || matches!(act, Act::Stagger { k, .. } if k > 0.3) {
        if relaxed {
            Hand::Swing
        } else {
            Hand::Free(chest.at(v3(-0.45, 0.2, -0.08)))
        }
    } else {
        Hand::Grip(weapon.at(weapons::LONGSWORD_LEFT_GRIP), weapon.rot)
    };
    let mut pole = melee_poles(chest, w.pos.y);
    if shouldered {
        pole[1] = chest.rot * v3(0.6, -1.0, 0.1);
    } else if relaxed {
        pole[1] = chest.rot * v3(0.4, -0.6, 0.8);
    }
    Arms {
        weapon,
        shield: weapon,
        hand: [left, Hand::Grip(weapon.pos, weapon.rot)],
        pole,
        draw: 0.0,
        nocked: false,
        held: None,
    }
}

/// Arming sword and heater shield: the same diagonal cut one-handed, the shield pulled in to
/// the chest as the sword arm goes out. The shield meets each line: raised overhead and
/// tilted up against a blow from above, swung out to the side a cut comes from, low and
/// square to a thrust; a parry punches it out at the blow. At ease the sword hangs point down
/// in the right hand (`low_ready`, at the low ready: the point ahead), the shield low on the
/// arm.
fn sword_and_shield(act: Act, chest: Bone, swing: f32, jiggle: Vec2, low_ready: bool) -> Arms {
    let moves = Moves {
        up: [
            (v3(0.21, 0.24, -0.27), v3(-0.15, 0.2, -0.97)),
            (v3(0.26, 0.42, -0.13), v3(0.15, 0.95, -0.25)),
        ],
        light: [
            (v3(0.26, 0.54, 0.02), v3(0.3, 0.7, 0.65)),
            (v3(0.27, 0.56, 0.06), v3(0.35, 0.6, 0.72)),
        ],
        heavy: [
            (v3(0.29, 0.62, 0.07), v3(0.45, 0.65, 0.62)),
            (v3(0.3, 0.63, 0.11), v3(0.5, 0.55, 0.67)),
        ],
        cut: [
            (v3(0.1, 0.3, -0.5), v3(-0.35, 0.0, -0.94)),
            (v3(-0.04, 0.08, -0.38), v3(-0.65, -0.45, -0.6)),
            (v3(-0.1, 0.05, -0.3), v3(-0.85, -0.4, -0.35)),
            (v3(-0.03, 0.1, -0.36), v3(-0.6, -0.4, -0.7)),
        ],
    };
    // The sword waits up beside the head whatever the line (the shield takes the blow).
    let up = blade(
        v3(0.26, 0.3, -0.08),
        v3(0.05, 0.95, 0.3),
        v3(0.0, 0.0, -1.0),
    );
    let w = Blade::new(
        blade(v3(0.25, -0.1, -0.16), v3(0.05, -0.4, -0.92), Vec3::Y),
        blade(v3(0.2, 0.28, -0.24), v3(-0.12, 0.32, -0.94), Vec3::Y),
        &moves,
        [(up, up); 5],
        blade(v3(0.4, 0.15, -0.05), v3(0.6, 0.6, 0.2), Vec3::Z),
    )
    .at(act, jiggle);
    let face = |pos: Vec3, fwd: Vec3| Bone::new(pos, aim(fwd, Vec3::Y));
    let low = face(v3(-0.28, -0.14, -0.15), v3(-0.4, -0.05, -0.9));
    let guard = face(v3(-0.1, 0.2, -0.38), v3(0.1, 0.0, -1.0));
    let tucked = face(v3(-0.22, 0.16, -0.27), v3(-0.35, 0.05, -0.94));
    let thrown = face(v3(-0.42, 0.18, -0.15), v3(-0.8, 0.25, -0.5));
    let lines = [
        (
            face(v3(0.0, 0.58, -0.3), v3(0.0, 0.72, -0.7)),
            face(v3(0.0, 0.62, -0.34), v3(0.0, 0.85, -0.52)),
        ),
        (
            face(v3(-0.25, 0.36, -0.33), v3(-0.55, 0.1, -0.83)),
            face(v3(-0.3, 0.37, -0.4), v3(-0.72, 0.1, -0.69)),
        ),
        (
            face(v3(0.02, 0.32, -0.38), v3(0.5, 0.1, -0.86)),
            face(v3(0.05, 0.33, -0.42), v3(0.62, 0.1, -0.78)),
        ),
        (
            face(v3(-0.08, 0.14, -0.4), v3(0.1, -0.2, -0.97)),
            face(v3(-0.12, 0.16, -0.44), v3(-0.4, -0.1, -0.9)),
        ),
        (
            face(v3(-0.05, 0.34, -0.42), v3(0.05, 0.15, -1.0)),
            face(v3(-0.02, 0.32, -0.5), v3(0.0, 0.1, -1.0)),
        ),
    ];
    // Kept toward the foe as the shoulders coil away from it, until the release.
    let keep = |p: f32, heavy: bool| step(0.1, 0.6, p) * if heavy { 0.6 } else { 0.4 };
    let sh = match act {
        Act::Relaxed => swung(low, 0, 0.5 * swing),
        Act::Guard => Bone::new(guard.pos + v3(0.0, 0.008 * jiggle.y, 0.0), guard.rot),
        Act::Windup { p, heavy } => {
            let keep = keep(p, heavy) * (1.0 - step(0.88, 1.0, p));
            let held = guard.mix(tucked, 0.5 * step(0.85, 1.0, p));
            Bone::new(ry(keep) * held.pos, ry(keep) * held.rot)
        }
        Act::Check { p, u, heavy } => {
            let keep = keep(p, heavy) * (1.0 - check_back(u));
            Bone::new(ry(keep) * guard.pos, ry(keep) * guard.rot)
        }
        Act::Strike { t, dur, .. } => guard.mix(
            tucked,
            (0.5 + 0.5 * step(0.0, 0.06, t)) * (1.0 - step(dur * 0.4, dur, t)),
        ),
        Act::Block {
            k,
            hit,
            parry,
            line,
        } => {
            let (raised, punched) = lines[line as usize];
            let up = guard.mix(raised, k);
            if parry {
                let bash = step(-0.03, 0.03, hit) * (1.0 - step(0.08, 0.25, hit));
                up.mix(punched, bash * k)
            } else {
                let (push, tip) = line.recoil(recoil(hit) * k);
                Bone::new(up.pos + push, tip * up.rot)
            }
        }
        Act::Stagger { k, .. } => guard.mix(thrown, k),
    };
    let w = match act {
        Act::Relaxed if !low_ready => {
            let down = blade(
                v3(0.23, -0.2, -0.08),
                v3(0.08, -0.86, -0.5),
                v3(0.0, 0.5, -0.86),
            );
            swung(down, 1, -0.6 * swing)
        }
        Act::Relaxed => swung(w, 1, -0.6 * swing),
        _ => w,
    };
    let weapon = chest.then(w);
    let shield = chest.then(sh);
    let mut pole = melee_poles(chest, w.pos.y);
    pole[0] = chest.rot * v3(-1.0, -0.6, 0.2);
    Arms {
        weapon,
        shield,
        hand: [
            Hand::Grip(shield.pos, shield.rot * weapons::SHIELD_HAND),
            Hand::Grip(weapon.pos, weapon.rot),
        ],
        pole,
        draw: 0.0,
        nocked: false,
        held: None,
    }
}

/// How many shots a carbine's magazine holds, as the pose shows it: it changes after every
/// that many.
const MAGAZINE_SHOTS: u32 = 6;

/// How far a carbine's muzzle has climbed `t` s after a shot (1 at the top): up at once, back
/// down over a tenth of a second, dipping a little past the aim before it settles.
fn kick(t: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    let rise = (t / 0.025).min(1.0) * (-(t - 0.025).max(0.0) / 0.05).exp();
    rise - 0.15 * step(0.06, 0.12, t) * (1.0 - step(0.14, 0.3, t))
}

/// Partway (`w`, 0..1) from the arms `rest` to `act` (the free hands resolved): the weapon
/// (and the shield) swung from one to the other (along a little arc, `flourish`: lifted and
/// tipped up through the middle, chest frame), the hands on them going with them, a free hand
/// reaching for its grip on the way (or letting go of it).
fn readied(rest: &Arms, act: &Arms, w: f32, chest: Bone, kind: Kind, flourish: bool) -> Arms {
    let arc = if flourish { (PI * w).sin() } else { 0.0 };
    let (lift, tip) = match kind {
        Kind::Longsword => (v3(-0.04, 0.12, -0.06), 0.35),
        Kind::SwordShield => (v3(0.0, 0.08, -0.06), 0.3),
        Kind::Rifle => (v3(0.0, 0.05, -0.05), 0.0),
        _ => (Vec3::ZERO, 0.0),
    };
    // A free hand reaches for its grip through the middle of the way (the carbine's support
    // hand, with furthest to go, all the way).
    let (r0, r1) = if kind == Kind::Rifle {
        (0.0, 1.0)
    } else {
        (0.15, 0.75)
    };
    let tipped = chest.rot * rx(tip * arc) * chest.rot.inverse();
    let mixed = rest.weapon.mix(act.weapon, w);
    let weapon = Bone::new(mixed.pos + chest.rot * lift * arc, tipped * mixed.rot);
    let shield = if kind == Kind::SwordShield {
        rest.shield.mix(act.shield, w)
    } else {
        weapon
    };
    // What each hand holds (the shield the left, with a shield), and its hold on it.
    let on = |a: &Arms, k: usize| {
        if kind == Kind::SwordShield && k == 0 {
            a.shield
        } else {
            a.weapon
        }
    };
    let hold = |a: &Arms, k: usize| match a.hand[k] {
        Hand::Grip(p, r) => {
            let b = on(a, k);
            let inv = b.rot.inverse();
            Some(Bone::new(inv * (p - b.pos), inv * r))
        }
        _ => None,
    };
    let wrist = |a: &Arms, k: usize| match a.hand[k] {
        Hand::Free(p) | Hand::Near(p, ..) => p,
        Hand::Grip(p, r) => p - r * grip(k),
        Hand::Swing => chest.at(shoulder(k)) + Vec3::NEG_Y * 0.5,
    };
    let hand = std::array::from_fn(|k| {
        let held = if kind == Kind::SwordShield && k == 0 {
            shield
        } else {
            weapon
        };
        let reach = step(r0, r1, w);
        match (hold(rest, k), hold(act, k)) {
            (Some(a), Some(b)) => {
                let g = held.then(a.mix(b, w));
                Hand::Grip(g.pos, g.rot)
            }
            (None, Some(b)) => {
                let g = held.then(b);
                let from = wrist(rest, k);
                Hand::Near(from.lerp(g.pos - g.rot * grip(k), reach), g.rot, reach)
            }
            (Some(a), None) => {
                let g = held.then(a);
                let to = wrist(act, k);
                let on = 1.0 - reach;
                Hand::Near(to.lerp(g.pos - g.rot * grip(k), on), g.rot, on)
            }
            (None, None) => Hand::Free(wrist(rest, k).lerp(wrist(act, k), w)),
        }
    });
    let pole = std::array::from_fn(|k| rest.pole[k].lerp(act.pole[k], w).normalize_or(act.pole[k]));
    // An arrow on the string, or a magazine or an arrow in hand: as the nearer of the two.
    let later = if w < 0.5 { rest } else { act };
    Arms {
        weapon,
        shield,
        hand,
        pole,
        draw: later.draw,
        nocked: later.nocked,
        held: later.held,
    }
}

/// Partway (`w`) from one hold to another (both on grips; else the second).
fn shift(a: Hand, b: Hand, w: f32) -> Hand {
    match (a, b) {
        _ if w <= 0.0 => a,
        _ if w >= 1.0 => b,
        (Hand::Grip(p, r), Hand::Grip(q, s)) => Hand::Grip(p.lerp(q, w), r.slerp(s, w)),
        _ => b,
    }
}

/// The carbine: carried across the chest at port arms (on the move); shouldered to aim, the
/// sight line through the eye, drifting a little as the shooter breathes. A shot kicks the
/// muzzle up and the stock back into the shoulder, and they settle. Every few shots the
/// magazine changes while the gun cools down: the gun comes down and cants, the support hand
/// strips the magazine, stows it in the pouch on the left hip and brings a fresh one up, seats
/// it, racks the charging handle and goes back to the handguard as the gun comes up again. At
/// ease (not `low`, at the low ready) it hangs muzzle down from the right hand by its grip.
#[allow(clippy::too_many_arguments)]
fn rifle(
    act: Act,
    chest: Bone,
    head: Bone,
    pelvis: Bone,
    aiming: f32,
    shot: Option<Shot>,
    (now, seed): (f64, f64),
    low_ready: bool,
) -> Arms {
    // Carried across the chest at port arms (knocked aside when staggered).
    let mut dir = v3(-0.55, 0.65, -0.3);
    if let Act::Stagger { k, .. } = act {
        dir = dir.lerp(v3(-0.3, 0.9, -0.35), k);
    }
    let low = chest.then(Bone::new(
        v3(0.15, 0.05, -0.22),
        aim(dir, v3(0.3, 0.3, -1.0)),
    ));
    // Shouldered: the sight line through the eye, straight ahead, wandering a little.
    let clock = |period: f64, offset: f64| (now * TAU as f64 / period + seed * offset).sin() as f32;
    let drift = ry(0.007 * clock(3.3, 1.0)) * rx(0.005 * clock(4.1, 2.0) + 0.004 * clock(3.8, 7.0));
    let k = shot.map_or(0.0, |s| kick(s.since));
    let rot = drift * aim(Vec3::NEG_Z, Vec3::Y) * rx(0.13 * k);
    let eye = head.at(rig::EYES);
    let shouldered = Bone::new(
        eye - rot * weapons::RIFLE_EYE + v3(0.0, 0.0, 0.03 * k.max(0.0)),
        rot,
    );
    let mut weapon = low.mix(shouldered, aiming);
    let on_guard = |w: Bone| {
        Hand::Grip(
            w.at(weapons::RIFLE_LEFT_GRIP),
            w.rot * weapons::rifle_left_hand(),
        )
    };
    let mut left = on_guard(weapon);
    // At ease: hanging muzzle down from the right hand by its grip, the left hand free.
    if act == Act::Relaxed && !low_ready {
        weapon = chest.then(Bone::new(
            v3(0.25, -0.12, -0.09),
            aim(v3(0.1, -0.62, -0.78), v3(0.0, 0.78, -0.62)),
        ));
        left = Hand::Swing;
    }
    let mut held = None;
    // Changing the magazine: `r` of the way through.
    let reload = shot
        .filter(|s| s.reload && !matches!(act, Act::Stagger { .. }))
        .map(|s| s.since / (s.ready + 0.25).clamp(0.6, 1.3))
        .filter(|r| (0.0..1.0).contains(r));
    if let Some(r) = reload {
        let down = step(0.0, 0.12, r) * (1.0 - step(0.86, 1.0, r));
        // In front of the chest, muzzle up and canted so the well faces the support hand.
        let canted = chest.then(Bone::new(
            v3(-0.06, 0.2, -0.26),
            aim(v3(-0.25, 0.5, -0.83), v3(0.45, 0.89, 0.0)),
        ));
        weapon = weapon.mix(canted, down);
        let seated = weapon.then(Bone::new(weapons::RIFLE_MAG, Quat::IDENTITY));
        let out = weapon.then(Bone::new(
            weapons::RIFLE_MAG + v3(0.0, -0.12, 0.04),
            rx(0.25),
        ));
        // Into the pouch on the left hip and out again (the old one stowed, a fresh one
        // drawn).
        let (top, yaw) = rig::POUCHES[1];
        let dip = step(0.42, 0.47, r) * (1.0 - step(0.47, 0.52, r));
        let stowed = pelvis.then(Bone::new(
            top + v3(0.0, 0.13 - 0.05 * dip, 0.0),
            ry(yaw) * rx(0.15),
        ));
        let mag = seated
            .mix(out, step(0.14, 0.26, r))
            .mix(stowed, step(0.26, 0.42, r) * (1.0 - step(0.52, 0.66, r)))
            .mix(seated, step(0.68, 0.75, r));
        held = Some(mag);
        let gripped = Hand::Grip(
            mag.at(weapons::RIFLE_MAG_GRIP),
            mag.rot * weapons::rifle_mag_hand(),
        );
        // Racked back and let go.
        let rack = step(0.84, 0.88, r) * (1.0 - step(0.88, 0.9, r));
        let charger = Hand::Grip(
            weapon.at(weapons::RIFLE_CHARGE + v3(0.0, 0.0, 0.07 * rack)),
            weapon.rot * weapons::rifle_charge_hand(),
        );
        let on_mag = step(0.04, 0.14, r) * (1.0 - step(0.75, 0.8, r));
        let charging = step(0.77, 0.83, r) * (1.0 - step(0.9, 0.97, r));
        left = shift(shift(on_guard(weapon), gripped, on_mag), charger, charging);
    }
    Arms {
        weapon,
        shield: weapon,
        hand: [
            left,
            Hand::Grip(weapon.pos, weapon.rot * weapons::rifle_hand()),
        ],
        pole: [
            chest.rot * v3(-0.3, -1.0, 0.1),
            chest.rot * v3(0.8, -0.6, 0.3),
        ],
        draw: 0.0,
        nocked: false,
        held,
    }
}

/// Where the string hand anchors at full draw (head frame): the corner of the mouth.
const ANCHOR: Vec3 = Vec3::new(0.035, 0.09, -0.075);

/// The bow: carried low in the left hand at ease, held ready in front with an arrow on the
/// string; drawn, the string hand comes back to its anchor at the corner of the mouth and
/// holds there, the last of the draw creeping in, then looses. See [`loosed`] for what
/// follows.
fn bow(
    act: Act,
    chest: Bone,
    head: Bone,
    aiming: f32,
    ready: bool,
    swing: f32,
    shot: Option<Shot>,
) -> Arms {
    // Carried low in the left hand (swinging as it walks), or held ready in front with an
    // arrow on the string.
    let carried = chest.then(swung(
        Bone::new(
            v3(-0.27, -0.22, -0.08),
            aim(v3(0.05, -0.87, -0.5), v3(0.0, 0.5, -0.87)),
        ),
        0,
        0.6 * swing,
    ));
    let held = chest.then(Bone::new(
        v3(-0.14, 0.06, -0.42),
        aim(v3(0.1, -0.5, -0.86), v3(-0.2, 0.86, -0.45)),
    ));
    let rest = if ready { held } else { carried };
    // At full draw: the bow arm straight toward the target at chin height.
    let rot = aim(Vec3::NEG_Z, v3(0.12, 1.0, 0.0));
    let anchor = head.at(ANCHOR);
    let left = chest.at(shoulder(0));
    let rise = anchor.y - left.y;
    let out = (0.55f32 * 0.55 - rise * rise).max(0.04).sqrt();
    let wrist = v3(anchor.x - 0.02, anchor.y, left.z - out);
    let drawn = Bone::new(wrist - rot * (weapons::BOW_HAND * grip(0)), rot);
    let mut weapon = rest.mix(drawn, aiming);
    let full =
        ((anchor - weapon.pos).dot(weapon.rot * Vec3::Z) - weapons::BOW_TIP.z).clamp(0.0, 0.62);
    let on_string = |w: Bone, d: f32| {
        let nock = w.at(v3(0.0, 0.0, weapons::BOW_TIP.z + d));
        Hand::Grip(nock, w.rot * weapons::BOW_HAND)
    };
    let draw_by = |p: f32| full * (0.97 * step(0.22, 0.68, p) + 0.03 * step(0.68, 1.0, p));
    let after = shot.filter(|s| s.since < s.ready);
    let (draw, nocked, right, arrow, out) = match (act, after) {
        (Act::Windup { p, .. }, _) => (draw_by(p), true, on_string(weapon, draw_by(p)), None, 0.0),
        (Act::Check { p, u, .. }, _) => {
            // Let down: the string eased back to rest.
            let d = draw_by(p) * (1.0 - step(0.0, 0.3, u));
            (d, true, on_string(weapon, d), None, 0.0)
        }
        (Act::Strike { .. } | Act::Guard, Some(s)) => {
            // Loosed: the bow rolls forward in the open hand and is caught.
            let roll = 0.35 * step(0.0, 0.1, s.since) * (1.0 - step(0.3, 0.6, s.since));
            weapon = weapon.then(Bone::new(Vec3::ZERO, rx(-roll)));
            loosed(chest, weapon, anchor, s)
        }
        _ if ready => (0.0, true, on_string(weapon, 0.0), None, 0.0),
        _ => (0.0, false, Hand::Swing, None, 0.0),
    };
    Arms {
        weapon,
        shield: weapon,
        hand: [
            Hand::Grip(weapon.pos, weapon.rot * weapons::BOW_HAND),
            right,
        ],
        pole: [
            chest.rot * v3(-1.0, -0.4, 0.0),
            chest.rot * string_elbow(out),
        ],
        draw,
        nocked,
        held: arrow,
    }
}

/// After a loose, `s.since` s ago, with the bow `weapon`: the string hand flies back along
/// the jaw from the anchor and stays there a moment; then it reaches over the right shoulder
/// to the quiver, draws an arrow up out of it, brings it forward and down to the bow, lays it
/// on the rest ahead of the string, slides it back onto the string and takes the string, all
/// before the bow is ready again, the elbow out to the side as the hand goes to and fro (not
/// back, as it draws). The draw, whether an arrow is on the string, the string hand, the arrow
/// in hand, and how far out the elbow is (0..1; see [`string_elbow`]).
fn loosed(
    chest: Bone,
    weapon: Bone,
    anchor: Vec3,
    s: Shot,
) -> (f32, bool, Hand, Option<Bone>, f32) {
    let t = s.since;
    // The hand holds the arrow at its nock as it holds the string.
    let hold = |arrow: Bone| Hand::Grip(arrow.pos, arrow.rot * weapons::BOW_HAND);
    let string = weapon.rot * weapons::BOW_HAND;
    let back = step(0.0, 0.07, t);
    let wrist = anchor + weapon.rot * v3(0.03, 0.02, 0.09 + 0.12 * back);
    let thrown = Hand::Grip(wrist + string * grip(1), string);
    // The arrow along the way: its nock just out of the quiver's mouth, the shaft down in
    // it; drawn up over the shoulder, trailing back; on the rest ahead of the string; nocked.
    let (mouth, bottom) = (weapons::QUIVER_MOUTH, weapons::QUIVER_BOTTOM);
    let axis = (mouth - bottom).normalize();
    let quiver = chest.then(Bone::new(
        mouth + axis * weapons::QUIVER_NOCKS,
        aim(-axis, Vec3::Z),
    ));
    let drawn = chest.then(Bone::new(
        v3(0.2, 0.62, -0.1),
        aim(v3(-0.1, -0.55, 0.83), Vec3::Y),
    ));
    let rest = weapon.then(Bone::new(
        v3(0.0, 0.02, weapons::BOW_TIP.z - 0.12),
        Quat::IDENTITY,
    ));
    let nocked = weapon.then(Bone::new(v3(0.0, 0.0, weapons::BOW_TIP.z), Quat::IDENTITY));
    let start = 0.3;
    let end = (s.ready - 0.12).clamp(start + 0.45, 1.15);
    let u = ((t - start) / (end - start)).clamp(0.0, 1.0);
    let out = step(0.0, 0.15, u) * (1.0 - step(0.7, 1.0, u));
    if u < 0.25 {
        let reach = shift(thrown, hold(quiver), step(0.0, 0.25, u));
        return (0.0, false, reach, None, out);
    }
    if u >= 0.92 {
        return (0.0, true, hold(nocked), None, out);
    }
    let arrow = quiver
        .mix(drawn, step(0.25, 0.45, u))
        .mix(rest, step(0.45, 0.75, u))
        .mix(nocked, step(0.75, 0.92, u));
    (0.0, false, hold(arrow), Some(arrow), out)
}

/// The string hand's elbow (chest frame): back, on the string (`out` 0); out to the side (1),
/// handling an arrow.
fn string_elbow(out: f32) -> Vec3 {
    v3(0.6, 0.3, 1.0).lerp(v3(1.0, -0.05, 0.35), out)
}

fn fists(act: Act, chest: Bone, ready: bool) -> Arms {
    let guard = [v3(-0.12, 0.36, -0.3), v3(0.1, 0.32, -0.24)];
    let cocked = v3(0.2, 0.3, 0.02);
    let punched = v3(0.02, 0.4, -0.5);
    let hands = match act {
        Act::Windup { p, .. } => [guard[0], guard[1].lerp(cocked, p)],
        // Thrown, pulled a third of the way out, and back to guard.
        Act::Check { p, u, .. } => [
            guard[0],
            guard[1]
                .lerp(cocked, p)
                .lerp(punched, 0.35 * check_jab(u))
                .lerp(guard[1], check_back(u)),
        ],
        Act::Strike { t, dur, .. } => {
            let k = step(0.0, 0.06, t) * (1.0 - step(dur * 0.5, dur, t));
            [guard[0], guard[1].lerp(punched, k)]
        }
        Act::Block { k, line, .. } => {
            // Forearms up over the head, up beside it on the side the blow comes from, or
            // low across the body against a thrust.
            let up = match line {
                Line::High => [v3(-0.1, 0.6, -0.22), v3(0.1, 0.6, -0.22)],
                Line::Left => [v3(-0.2, 0.52, -0.2), v3(0.06, 0.42, -0.28)],
                Line::Right => [v3(-0.06, 0.42, -0.28), v3(0.2, 0.52, -0.2)],
                Line::Thrust => [v3(-0.06, 0.2, -0.33), v3(0.06, 0.22, -0.31)],
                Line::Square => [v3(-0.07, 0.48, -0.26), v3(0.07, 0.48, -0.26)],
            };
            [guard[0].lerp(up[0], k), guard[1].lerp(up[1], k)]
        }
        Act::Stagger { k, .. } => [
            guard[0].lerp(v3(-0.45, 0.2, -0.05), k),
            guard[1].lerp(v3(0.45, 0.2, -0.05), k),
        ],
        _ => guard,
    };
    let hand = if ready {
        hands.map(|h| Hand::Free(chest.at(h)))
    } else {
        [Hand::Swing; 2]
    };
    Arms {
        weapon: chest,
        shield: chest,
        hand,
        pole: [
            chest.rot * v3(-0.5, -1.0, 0.2),
            chest.rot * v3(0.5, -1.0, 0.2),
        ],
        draw: 0.0,
        nocked: false,
        held: None,
    }
}

// ---------------------------------------------------------------------------- what hangs

/// A damped spring for something that hangs off the body: how far it is swung (`x` rolled
/// about the joint's z axis, sideways; `y` pitched about its x axis, fore and aft) and how fast
/// that changes.
#[derive(Debug, Clone, Copy, Default)]
struct Spring {
    a: Vec2,
    v: Vec2,
}

/// How a spring rings: its natural frequency (rad/s) and damping ratio, how far it may swing
/// each way (rad, sideways and fore and aft: not into the body), and how hard the joint's
/// acceleration and turning push it (rad/s² per m/s², and per rad/s²).
struct Ring {
    w: f32,
    zeta: f32,
    lo: Vec2,
    hi: Vec2,
    accel: f32,
    turn: f32,
}

/// The crest: stiff horsehair, standing up off the helmet.
const CREST: Ring = Ring {
    w: 16.0,
    zeta: 0.3,
    lo: Vec2::splat(-0.2),
    hi: Vec2::splat(0.2),
    accel: 1.6,
    turn: 0.35,
};
/// The tabards: heavy wool flaps hanging to the knee, the front one swinging out ahead (not
/// back into the thighs), the back one out behind.
const TABARDS: [Ring; 2] = [
    Ring {
        w: 6.5,
        zeta: 0.3,
        lo: Vec2::new(-0.3, -0.08),
        hi: Vec2::new(0.3, 0.55),
        accel: 2.0,
        turn: 1.0,
    },
    Ring {
        w: 6.5,
        zeta: 0.3,
        lo: Vec2::new(-0.3, -0.55),
        hi: Vec2::new(0.3, 0.08),
        accel: 2.0,
        turn: 1.0,
    },
];
/// The pouches: small, on loops round the belt, their backs against the hips.
const POUCH: Ring = Ring {
    w: 16.0,
    zeta: 0.3,
    lo: Vec2::new(-0.35, -0.05),
    hi: Vec2::new(0.35, 0.4),
    accel: 3.5,
    turn: 1.0,
};

impl Spring {
    /// On `dt` s, pushed by `push` (rad/s²).
    fn step(&mut self, dt: f32, push: Vec2, r: &Ring) {
        let n = (dt * 120.0).ceil().max(1.0);
        let h = dt / n;
        for _ in 0..n as usize {
            let acc = push - self.a * (r.w * r.w) - self.v * (2.0 * r.zeta * r.w);
            self.v += acc * h;
            self.a += self.v * h;
        }
        let a = self.a.clamp(r.lo, r.hi);
        if a.x != self.a.x {
            self.v.x = 0.0;
        }
        if a.y != self.a.y {
            self.v.y = 0.0;
        }
        self.a = a;
    }

    /// Its swing as a rotation in the joint's frame.
    fn rot(&self) -> Quat {
        rx(self.a.y) * rz(self.a.x)
    }
}

/// What hangs off the body and swings after it: the crest on the helmet, the tabards and the
/// pouches on the belt, each a damped spring about the joint it hangs from, pushed as that
/// joint speeds up, slows down and turns (a stop throws them on, a blow snaps them round, and
/// they swing back and settle). Kept in the owner's frame: where the joints were as last
/// posed and how fast they went (the owner's own moving and turning included), never a world
/// point.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Sway {
    /// When it was last stepped (NPC clock; 0 never), and the heading then.
    last: f64,
    yaw: f32,
    /// Where each joint was (owner frame) and how fast it went (m/s): the crest's pivot, the
    /// front and back tabards' hinges, the right and left pouches' tops.
    joints: [(Vec3, Vec3); 5],
    /// How the head and the hips were turned (owner frame), and how fast they turned (rad/s).
    turns: [(Quat, Vec3); 2],
    crest: Spring,
    tabard: [Spring; 2],
    pouch: [Spring; 2],
}

impl Sway {
    /// Follow the skeleton `s` as posed at `now`, the owner moving at `vel` (its own frame, x
    /// and z; m/s) and facing `yaw`.
    fn step(&mut self, now: f64, s: &Skeleton, vel: Vec2, yaw: f32) {
        let dt = (now - self.last) as f32;
        let turned = wrap(yaw - self.yaw);
        let fresh = self.last == 0.0 || !(1e-4..=0.25).contains(&dt) || turned.abs() > 1.2;
        self.last = now;
        self.yaw = yaw;
        // Last step's frame, as seen from this one.
        let back = ry(-turned);
        let owner = Vec3::new(vel.x, 0.0, vel.y);
        let joints = [
            s.head.at(rig::CREST_PIVOT),
            s.tabard[0].pos,
            s.tabard[1].pos,
            s.pelvis.at(rig::POUCHES[0].0),
            s.pelvis.at(rig::POUCHES[1].0),
        ];
        let mut accel = [Vec3::ZERO; 5];
        for (k, &p) in joints.iter().enumerate() {
            let (was, went) = self.joints[k];
            let v = if fresh {
                owner
            } else {
                owner + (p - back * was) / dt
            };
            if !fresh {
                accel[k] = ((v - back * went) / dt).clamp_length_max(40.0);
            }
            self.joints[k] = (p, v);
        }
        let mut spins = [Vec3::ZERO; 2];
        for (k, rot) in [s.head.rot, s.pelvis.rot].into_iter().enumerate() {
            let (was, spun) = self.turns[k];
            let mut d = rot * was.inverse() * ry(turned);
            if d.w < 0.0 {
                d = -d;
            }
            let w = if fresh {
                Vec3::ZERO
            } else {
                d.to_scaled_axis() / dt
            };
            if !fresh {
                spins[k] = ((w - back * spun) / dt).clamp_length_max(120.0);
            }
            self.turns[k] = (rot, w);
        }
        if fresh {
            *self = Sway {
                last: self.last,
                yaw: self.yaw,
                joints: self.joints,
                turns: self.turns,
                ..Default::default()
            };
            return;
        }
        // In a joint's frame `rot`: a flap hanging from it swings back against the way the
        // joint speeds up (along `L`, how far its weight hangs below), and lags its turning;
        // the crest, standing up, leans the other way.
        let push = |rot: Quat, a: Vec3, spin: Vec3, r: &Ring, up: bool| {
            let a = rot.inverse() * a * r.accel;
            let w = rot.inverse() * spin * r.turn;
            let hang = if up { -1.0 } else { 1.0 };
            Vec2::new(-a.x, a.z) * hang - Vec2::new(w.z, w.x)
        };
        self.crest.step(
            dt,
            push(s.head.rot, accel[0], spins[0], &CREST, true),
            &CREST,
        );
        for k in 0..2 {
            let p = push(s.tabard[k].rot, accel[1 + k], spins[1], &TABARDS[k], false);
            self.tabard[k].step(dt, p, &TABARDS[k]);
            let rot = s.pelvis.rot * ry(rig::POUCHES[k].1);
            let p = push(rot, accel[3 + k], spins[1], &POUCH, false);
            self.pouch[k].step(dt, p, &POUCH);
        }
    }
}

// ============================================================================ the pose

#[allow(clippy::too_many_arguments)]
pub(super) fn humanoid(
    now: f64,
    phase: f32,
    facing: Quat,
    loco: &LocomotionState,
    intent: Option<&MoveIntent>,
    combat: Option<&NpcCombat>,
    brain: Option<&NpcBrain>,
    vitals: Option<&NpcVitals>,
    downed: Option<&Downed>,
    weapon: NpcWeapon,
    seen: &Seen,
    st: &mut StandInState,
    materials: &mut Vec<(Part, StandardMaterial)>,
    stand_in: &StandIn,
) -> impl Fn(Part) -> Placed + use<> {
    let kind = Kind::of(&weapon);
    let ranged = weapon.style == WeaponStyle::Ranged;
    let fighting = brain.is_some_and(|b| b.alert == AlertLevel::Combat);
    let is_down = vitals.is_some_and(|v| v.downed);
    let seed = brain
        .and_then(|b| b.home)
        .map(|h| (h.x * 0.731 + h.z * 1.37).sin() * 10.0)
        .unwrap_or(0.0);
    // A pose with no past (just built, back from far away, or getting up) takes the states it
    // finds as already under way.
    let m = &mut st.motion;
    let stale = !m.posed || !(0.0..=0.5).contains(&(now - m.last));
    // How long since the last pose, and how far the body has turned since.
    let heading = {
        let f = facing * Vec3::NEG_Z;
        (-f.x).atan2(-f.z)
    };
    let (dt, turned) = if stale {
        (0.0, 0.0)
    } else {
        ((now - m.last) as f32, wrap(heading - m.yaw))
    };
    // Revived: it gets up from where it lay (a pose seen afresh just stands).
    let revived = m.down && !is_down;
    let fresh = stale || revived;
    m.rise = if revived && !stale {
        m.lay.map(|r| (now, r))
    } else if stale || is_down {
        None
    } else {
        m.rise
            .filter(|(t0, _)| ((now - t0) as f32) < rise_secs(kind))
    };
    m.down = is_down;
    if fresh {
        m.swing = None;
        m.recover = None;
        m.check = None;
        m.hurt = None;
        m.threat = None;
        m.meets = None;
        m.fired = None;
        // Fighters of a squad don't all change magazines together.
        m.shots = (seed.abs() * 7.0) as u32 % MAGAZINE_SHOTS;
    }
    // What came at this fighter this step: attacks telegraphed at it, blows that landed.
    for &(attacker, target, how) in seen.attacks {
        if target == Some(seen.me) {
            m.threat = Some((attacker, how));
        }
    }
    for b in seen.blows {
        if b.target == seen.me && b.amount > 0.0 {
            let source = b.source.and_then(|e| (seen.foe)(e));
            let h = Hurt::of(b, seen.at, facing, now, source.as_ref());
            // The wound it carries: the last blow to the body or a leg.
            if h.zone != Zone::Head {
                let side = if h.side != 0.0 {
                    h.side
                } else {
                    seed.sin().signum() as f32
                };
                m.wound = Some((h.zone, side));
            }
            m.hurt = Some(h);
        }
    }
    // Badly hurt (under about a third of its health), the wound tells: a limp on the leg last
    // struck, or hunched over a wound in the body (a leg, struck nowhere it can tell).
    let health = vitals.map_or(1.0, |v| v.fraction());
    if health >= 0.999 {
        m.wound = None;
    }
    let badly = if is_down {
        0.0
    } else {
        1.0 - step(0.3, 0.4, health)
    };
    let wound = (badly > 0.0).then(|| {
        let (zone, side) = m.wound.unwrap_or((Zone::Legs, seed.sin().signum() as f32));
        (zone, side, badly)
    });
    m.hobble = hobble(wound);
    let mut act = if fighting { Act::Guard } else { Act::Relaxed };
    let mut windup = if m.swung > 0.0 {
        m.swung
    } else {
        weapon.windup
    };
    let mut dodge = 0.0;
    let mut winding = false;
    let mut blade_glow: Option<StandardMaterial> = None;
    let mut parry = false;
    if let Some(c) = combat {
        // A telegraph that ends before its blow lands was called off (a feint) or cut short.
        if let Some((start, hit_at, heavy)) = m.swing {
            let going = matches!(c.phase, AttackPhase::Windup { hit_at: h, .. } if h == hit_at);
            if !going {
                if now < hit_at - 0.03 {
                    let p = ((now - start) / (hit_at - start).max(0.05)) as f32;
                    m.check = Some((now, p.clamp(0.0, 1.0), heavy));
                }
                m.swing = None;
            }
        }
        match c.phase {
            AttackPhase::Windup { hit_at, heavy, .. } => {
                winding = true;
                m.heavy = heavy;
                m.check = None;
                // Timed from when it began (or, found under way, as long as a telegraph with
                // this weapon takes).
                let (start, ..) = *m.swing.get_or_insert_with(|| {
                    let est = weapon.windup * if heavy && !ranged { 1.5 } else { 1.0 };
                    let start = if fresh { hit_at - f64::from(est) } else { now };
                    (start.min(hit_at - 0.05), hit_at, heavy)
                });
                let total = (hit_at - start) as f32;
                m.swung = total;
                windup = total;
                let p = (1.0 - (hit_at - now) as f32 / total).clamp(0.0, 1.0);
                act = Act::Windup { p, heavy };
                if heavy && !ranged {
                    blade_glow = Some(lit(0xff7a1a, 12_000.0 * (0.4 + 0.6 * p)));
                }
            }
            AttackPhase::Recovery { until } if now < until => {
                if fresh {
                    m.heavy = false;
                }
                // Timed from when it began (a heavy blow's, or a missed one's, runs longer).
                let (start, _) = *m.recover.get_or_insert_with(|| {
                    let start = if fresh {
                        until - f64::from(weapon.recovery)
                    } else {
                        now
                    };
                    (start.min(until - 0.05), until)
                });
                act = Act::Strike {
                    t: (now - start).max(0.0) as f32,
                    dur: (until - start) as f32,
                    heavy: m.heavy,
                };
            }
            AttackPhase::Staggered { until } if now < until => {
                let dur = vitals.map(|v| v.stagger_secs).unwrap_or(1.0).max(0.2);
                let left = (until - now) as f32;
                let k = step(0.0, 0.12, dur - left).min(step(0.0, 0.25, left));
                act = Act::Stagger { k, e: dur - left };
            }
            _ => {}
        }
        if !matches!(c.phase, AttackPhase::Recovery { .. }) {
            m.recover = None;
        }
        // A blow called off checks, then settles back on guard (or into the short recovery a
        // hit that cut it short leaves).
        if let Some((since, p, heavy)) = m.check {
            let u = (now - since) as f32;
            if u < CHECK && matches!(act, Act::Guard | Act::Relaxed | Act::Strike { .. }) {
                act = Act::Check { p, u, heavy };
            } else {
                m.check = None;
            }
        }
        match c.defense {
            Defense::Block { until } | Defense::Parry { until }
                if now < until && !matches!(act, Act::Stagger { .. }) =>
            {
                parry = matches!(c.defense, Defense::Parry { .. });
                // A guard already up stays up when a second blow stretches it.
                if m.guard.0 != until {
                    let up = now <= m.guard.0 + 0.05 && m.guard.1 <= now && !fresh;
                    let since = if up {
                        m.guard.1
                    } else if fresh {
                        now - 1.0
                    } else {
                        now
                    };
                    let lands = until - if parry { 0.08 } else { 0.35 };
                    m.guard = (until, since, lands);
                    if !up {
                        m.line = Line::default();
                        m.meets = None;
                    }
                }
                // The blow it meets: the last one landed, else the next to land. Its line is
                // read off the attacker while it comes (and what it telegraphed), and kept
                // once it lands.
                let soonest =
                    |a: &&IncomingAttack, b: &&IncomingAttack| a.lands_at.total_cmp(&b.lands_at);
                let landed = c
                    .incoming
                    .iter()
                    .filter(|a| a.lands_at <= now)
                    .max_by(soonest);
                let next = c
                    .incoming
                    .iter()
                    .filter(|a| a.lands_at > now)
                    .min_by(soonest);
                let meet = match (landed, next) {
                    (Some(a), _) if now - a.lands_at < 0.4 => Some(a),
                    (_, Some(a)) if a.lands_at < until + 0.05 => Some(a),
                    _ => None,
                };
                if let Some(a) = meet {
                    m.guard.2 = a.lands_at;
                    if now < a.lands_at || m.meets != Some(a.attacker) {
                        let how = m.threat.filter(|t| t.0 == a.attacker).map(|t| t.1);
                        let foe = (seen.foe)(a.attacker);
                        m.line = Line::of(how, a.ranged, foe.as_ref(), seen.at, facing);
                        m.meets = Some(a.attacker);
                    }
                }
                let rise = if parry { 0.06 } else { 0.1 };
                let k = step(0.0, rise, (now - m.guard.1) as f32).min(step(
                    0.0,
                    0.1,
                    (until - now) as f32,
                ));
                act = Act::Block {
                    k,
                    hit: (now - m.guard.2) as f32,
                    parry,
                    line: m.line,
                };
            }
            Defense::Dodge { until } if now < until => {
                if m.dodge.0 != until {
                    m.dodge = (until, if fresh { now - 1.0 } else { now }, 0.0);
                }
                // Which way: as soon as the body moves sideways.
                let side = (facing.inverse() * loco.velocity).x;
                if m.dodge.2 == 0.0 && side.abs() > 0.3 {
                    m.dodge.2 = side.signum();
                }
                let (until, since, side) = m.dodge;
                dodge = side
                    * step(0.0, 0.06, (now - since) as f32)
                    * step(0.0, 0.15, (until - now) as f32);
            }
            _ => {}
        }
        // A flash when hit (the plates in their team's hue, whichever that is).
        if c.last_hurt > 0.0 && now - c.last_hurt < 0.12 {
            for part in ARMOR {
                materials.push((part, flash()));
            }
            for hue in 0..HUES {
                let m = plate_flash(hue);
                for part in ARMOR.into_iter().filter_map(|p| paint_of(p, hue)) {
                    materials.push((part, m));
                }
            }
        }
    }
    if parry {
        blade_glow = Some(lit(0xffffff, 25_000.0));
        materials.push((Part::Shield, flash()));
        for hue in 0..HUES {
            if let Some(plates) = paint_of(Part::Shield, hue) {
                materials.push((plates, plate_flash(hue)));
            }
        }
    }
    if let Some(m) = blade_glow {
        materials.push((Part::Weapon, m));
        materials.push((PLAIN_WEAPON, m));
        if kind == Kind::Fists {
            materials.push((HAND_R, m));
        }
    }
    // Muzzle flash right after a shot; a carbine's magazine changes every few shots, while
    // it cools down.
    if ranged && st.was_winding && !winding {
        st.flash_until = now + 0.07;
        if matches!(act, Act::Strike { .. }) {
            let m = &mut st.motion;
            m.shots = m.shots.wrapping_add(1);
            let ready_at = combat.map_or(now, |c| c.ready_at).max(now + 0.3);
            let reload = kind == Kind::Rifle && m.shots.is_multiple_of(MAGAZINE_SHOTS);
            m.fired = Some((now, ready_at, reload));
        }
    }
    st.was_winding = winding;
    let flash = kind == Kind::Rifle && now < st.flash_until;
    // The visor glows with the alert level.
    if !stand_in.plain && !is_down {
        let glow = match brain.map(|b| b.alert) {
            Some(AlertLevel::Suspicious) => Some((0xf2c94c, 15_000.0)),
            Some(AlertLevel::Searching) => Some((0xf2994a, 20_000.0)),
            Some(AlertLevel::Combat) => Some((0xff4a3d, 30_000.0)),
            _ => None,
        };
        if let Some((hex, nits)) = glow {
            materials.push((Part::Orb, lit(hex, nits).with_roughness(0.1)));
        }
    }
    let crouch = intent.is_some_and(|i| i.crouch) || loco.mode == LocomotionMode::Crouch;
    let m = &mut st.motion;
    // Downed: it falls from the pose it was in when it went down (long since, when first seen
    // down), along the blow that felled it (if one did just then), else back and a little to
    // one side; how it lands differs from one fighter to the next.
    if !is_down {
        m.fell = None;
    } else if m.fell.is_none() {
        let since = downed.map_or(if stale { now - 10.0 } else { now }, |d| d.since);
        let was = if stale {
            if fighting { Act::Guard } else { Act::Relaxed }
        } else {
            act
        };
        // Down on its knees by a mate, it goes down from there.
        let knelt = (!stale && m.tend > 0.0).then_some(Tend {
            k: m.tend,
            at: m.tend_at,
            knees: m.tend_knees,
        });
        m.fell = Some((since.min(now), was, knelt));
    }
    // (As its squad mates see it lie: see [`Lie`]; else along the blow seen landing then.)
    let fall = m.fell.map(|(since, ..)| {
        let since = downed.map_or(since, |d| d.since);
        let push = downed.and_then(|d| d.push).or_else(|| {
            m.hurt
                .filter(|h| (h.since - since).abs() < 0.3)
                .and_then(|h| Lie::pushed(h.push))
        });
        let lie = Lie::of(seen.vary, push);
        Fall {
            t: (now - since).max(0.0) as f32,
            yaw: lie.yaw,
            bent: lie.bent,
            flung: lie.flung,
        }
    });
    let (now_was, act) = match m.fell {
        Some((since, was, _)) => (since, was),
        None => (now, act),
    };
    // The weapon comes up from its carry at ease: at once against a blow or for a blow of its
    // own (an archer nocking in a hurry), a beat slower at the sight of a foe (an archer slower
    // still, nocking an arrow); and goes down again once there is no fight. Wary, it is carried
    // at the low ready.
    let alert = brain.map_or(AlertLevel::Relaxed, |b| b.alert);
    let up_to = if act == Act::Relaxed { 0.0 } else { 1.0 };
    let wary_to = if matches!(alert, AlertLevel::Suspicious | AlertLevel::Searching) {
        1.0
    } else {
        0.0
    };
    if m.rise.is_some() {
        (m.up, m.wary) = (1.0, 1.0);
    } else if stale {
        (m.up, m.wary) = (up_to, wary_to);
    } else {
        let (raise, lower) = readying(kind);
        if up_to > m.up {
            let secs = if act == Act::Guard {
                raise
            } else {
                hurried(kind)
            };
            m.up = (m.up + dt / secs).min(up_to);
        } else {
            m.up = (m.up - dt / lower).max(up_to);
        }
        m.wary += (wary_to - m.wary) * (1.0 - (-3.0 * dt).exp());
    }
    let ready = fighting || act != Act::Relaxed || m.rise.is_some();
    let poise = match (m.up, ready) {
        (u, _) if u > 0.0 && u < 1.0 => u,
        (_, true) => 1.0,
        _ => 0.0,
    };
    // The feet: knocked back, they stumble back under the body; rocked by a blow to the
    // head, the rear foot takes a half step back, the front one shuffling after it (both step
    // up again after); downed, they stay where they stood.
    let rocked = m.hurt.map_or(0.0, |h| stumble((now - h.since) as f32, &h));
    let rear = {
        let rest = rest_feet(kind, ready);
        usize::from(rest[1].0.y > rest[0].0.y)
    };
    let mut back = [0.35 * rocked; 2];
    back[rear] = rocked;
    if let Act::Stagger { k, .. } = act {
        back = back.map(|b| b.max(0.24 * k));
    }
    let dash_secs = (loco.mode == LocomotionMode::Dash && !is_down).then_some(seen.dash_secs);
    if fresh {
        m.posed = false;
    }
    let (gait, feet, dash) = if is_down && !stale {
        m.last = now;
        let feet = m.plants.map(|p| Plant {
            lift: 0.0,
            pitch: 0.0,
            ..p
        });
        if let (Some(fall), Some((_, was, knelt))) = (fall, m.fell) {
            m.lay = Some(Rise {
                t: 0.0,
                fall,
                was,
                since: now_was,
                feet,
                knelt,
            });
        }
        (Gait::new(Vec2::ZERO, 0.0, ready), feet, None)
    } else {
        let walked = m.walk(
            now,
            phase,
            facing,
            loco.velocity,
            kind,
            ready,
            dash_secs,
            back.map(|b| Vec2::new(0.0, b)),
        );
        m.plants = walked.1;
        walked
    };
    let hurt = m
        .hurt
        .map(|h| ((now - h.since) as f32, h))
        .filter(|(e, _)| *e < 1.0 && !is_down);
    let shot = m.fired.and_then(|(t, ready_at, reload)| {
        let since = (now - t) as f32;
        (since < 3.0 && !is_down).then_some(Shot {
            since,
            ready: (ready_at - t) as f32,
            reload,
        })
    });
    // Reviving a downed mate: down beside it as the brain kneels it there, the weapon set down
    // and the hands on its chest by the time the revive counts; up again after. Its chest and
    // where the knees go are kept in the owner's frame (turning and moving with the owner)
    // between sightings of it.
    let kneeling = brain
        .filter(|b| !is_down && b.action == NpcAction::Revive && b.revive_kneel > 0.0)
        .map(|b| (b.revive_kneel / TEND_DOWN).min(1.0));
    let tending = kneeling.is_some();
    if let Some((chest, knees)) = seen.patient {
        m.tend_at = facing.inverse() * (chest - seen.at).as_vec3();
        m.tend_knees = facing.inverse() * (knees - seen.at).as_vec3();
    } else if m.tend > 0.0 && !stale {
        let moved = |p: Vec3| ry(-turned) * p - v3(m.vel.x, 0.0, m.vel.y) * dt;
        (m.tend_at, m.tend_knees) = (moved(m.tend_at), moved(m.tend_knees));
    } else if m.tend <= 0.0 {
        (m.tend_at, m.tend_knees) = (v3(0.0, 0.28, -0.75), v3(0.0, 0.0, -0.35));
    }
    // Down at the pace the brain kneels it (from where it is, whatever it was doing
    // meanwhile), and up again (quicker if it moves off meanwhile: the knees don't drag). Not
    // while it gets up after a revive itself: it kneels once it is up.
    let speed = Vec2::new(loco.velocity.x, loco.velocity.z).length();
    let (up, lower) = if stale {
        (0.0, 1.0)
    } else {
        (
            m.tend - dt / TEND_UP * (1.0 + speed),
            m.tend + 1.2 * dt / TEND_DOWN,
        )
    };
    m.tend = if is_down || m.rise.is_some() {
        0.0
    } else {
        kneeling.unwrap_or(0.0).min(lower).max(up).max(0.0)
    };
    // Downed: how it knelt as it went down (it falls from there).
    let tend = match m.fell {
        Some((.., knelt)) => knelt,
        None => (m.tend > 0.0).then_some(Tend {
            k: m.tend,
            at: m.tend_at,
            knees: m.tend_knees,
        }),
    };
    let (look, lids, jaw, breath, expression) = face(FaceIn {
        g: &mut m.gaze,
        now,
        dt,
        turned,
        fresh,
        seed,
        facing,
        seen,
        brain,
        vitals,
        speed,
        hurt: m.hurt,
        act,
        melee: !ranged,
        fall,
        tend: tending.then(|| seen.at + (facing * m.tend_at).as_dvec3()),
        suppression: combat.map_or(0.0, |c| c.suppression),
    });
    let skeleton = solve(&Input {
        now: now_was,
        feet,
        gait,
        accel: if is_down { Vec2::ZERO } else { m.accel },
        crouch,
        kind,
        act,
        windup,
        dodge,
        fighting,
        hurt,
        dash,
        fall,
        behind: m.behind,
        shot,
        look,
        lids,
        jaw,
        breath,
        wound,
        up: m.up,
        wary: m.wary,
        poise,
        rise: m.rise.map(|(t0, r)| Rise {
            t: (now - t0) as f32,
            ..r
        }),
        rising: 0.0,
        tend,
        seed,
    });
    m.lying = fall.filter(|_| is_down).map(|f| Lying::of(&skeleton, f));
    m.body = Some((skeleton, expression));
    m.sway.step(now, &skeleton, m.vel, m.yaw);
    let spin = (now * 53.0 % TAU as f64) as f32;
    let slots = place(&skeleton, kind, flash, spin, &m.sway);
    move |part| slot(part).map_or(HIDDEN, |i| slots[i])
}

/// What the eyes and the face go by.
struct FaceIn<'a, 'b> {
    g: &'a mut Gaze,
    now: f64,
    dt: f32,
    turned: f32,
    fresh: bool,
    seed: f64,
    facing: Quat,
    seen: &'a Seen<'b>,
    brain: Option<&'a NpcBrain>,
    vitals: Option<&'a NpcVitals>,
    /// How fast the owner moves (m/s).
    speed: f32,
    hurt: Option<Hurt>,
    act: Act,
    melee: bool,
    fall: Option<Fall>,
    /// Reviving a downed mate: where its chest lies.
    tend: Option<DVec3>,
    /// How pinned down it is by fire (0..1).
    suppression: f32,
}

/// What the face shows, for a face another module draws (see `character.rs`), each 0..1: the
/// lids' blink alone (shut when downed); pain (the flinch of a blow, the ache of a wound);
/// effort (a heavy blow set and shouted, a sprint, a guard braced, a draw held, a mate's chest
/// pressed); fear (pinned down by fire, badly hurt in a fight); how hard it pants, and where
/// in a breath it is (in 1, out −1); how far the jaw is open; and how slack the face is,
/// downed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Expression {
    pub(super) blink: f32,
    pub(super) pain: f32,
    pub(super) effort: f32,
    pub(super) fear: f32,
    pub(super) pant: f32,
    pub(super) breath: f32,
    pub(super) jaw: f32,
    pub(super) slack: f32,
}

/// Where the eyes look, and the face: in a fight, at the foe; for a moment at whoever else
/// telegraphs an attack at it or lands a blow, at another foe close by now and then, or (out
/// of a fight) at a noise; searching, sweeping the ground ahead; wary, at what it heard. The
/// lids blink now and then (more often when alarmed); a sprint, or a bad wound, leaves it
/// breathing hard through the mouth; struck, the eyes squeeze and the teeth bare a moment; a
/// heavy blow sets the jaw and narrows the eyes through the telegraph and goes in with a
/// shout; downed, the eyes close and the jaw goes slack. The look, how far the lids are shut
/// and the jaw open, the breath (in 1, out −1, deeper when winded), and the expression.
fn face(f: FaceIn) -> (Look, f32, f32, f32, Expression) {
    let FaceIn {
        g,
        now,
        dt,
        seed,
        seen,
        ..
    } = f;
    let target = f.brain.and_then(|b| b.target);
    let alert = f.brain.map_or(AlertLevel::Relaxed, |b| b.alert);
    let fighting = alert == AlertLevel::Combat;
    let down = f.fall.is_some();
    let memory = seen.memory;
    let noise = memory.and_then(|mem| mem.stimulus);
    if f.fresh {
        *g = Gaze {
            next_check: now + 1.0 + 2.0 * f64::from(hash(seed, 1)),
            next_blink: now + 0.3 + 3.0 * f64::from(hash(seed, 2)),
            noise: noise.map_or(0.0, |n| n.time),
            breath: seed * 7.0,
            ..Default::default()
        };
    }
    // Toward a point in the world (`h` over it), from the eyes (down on the knees beside a
    // downed mate, lower): yaw and pitch in the owner's frame.
    let eyes = if f.tend.is_some() {
        f64::from(TEND_EYES)
    } else {
        1.62
    };
    let toward = |p: DVec3, h: f64| {
        let d = f.facing.inverse() * (p + DVec3::Y * h - seen.at - DVec3::Y * eyes).as_vec3();
        let flat = Vec2::new(d.x, d.z).length().max(0.3);
        Vec2::new((-d.x).atan2(-d.z), d.y.atan2(flat))
    };
    for &(attacker, at, _) in seen.attacks {
        if at == Some(seen.me) && Some(attacker) != target {
            g.glance = Some((Some(attacker), now + 0.6));
        }
    }
    for b in seen.blows {
        if b.target == seen.me && b.source.is_some() && b.source != target {
            g.glance = Some((b.source, now + 0.5));
        }
    }
    if let Some(n) = noise
        && n.time > g.noise
    {
        g.noise = n.time;
        if !fighting && now - n.time < 0.5 {
            g.glance = Some((None, now + 0.9));
        }
    }
    if f.tend.is_some() != g.tending {
        g.tending = f.tend.is_some();
        if g.tending {
            // Down beside the mate, it first looks up once its hands are on it.
            let wait = f64::from(TEND_DOWN) + 0.2 + 0.6 * f64::from(hash(seed, 4));
            g.next_check = now + wait;
        }
    }
    if f.tend.is_some() && now >= g.next_check {
        // Reviving a mate, it looks up now and then at the threat: its foe, else the nearest
        // one it sees.
        let n = g.blinks.wrapping_add(5);
        g.next_check = now + 1.1 + 1.3 * f64::from(hash(seed, n));
        let threat = target.or_else(|| {
            memory.and_then(|mem| {
                mem.known
                    .iter()
                    .filter(|k| k.visible && k.awareness > 0.5)
                    .map(|k| (k.entity, k.last_position.distance(seen.at)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(e, _)| e)
            })
        });
        if let Some(e) = threat {
            let hold = 0.55 + 0.3 * f64::from(hash(seed, n + 1));
            g.glance = Some((Some(e), now + hold));
        }
    } else if fighting && now >= g.next_check {
        g.next_check = now + 2.0 + 2.5 * f64::from(hash(seed, g.blinks.wrapping_add(3)));
        let other = memory.and_then(|mem| {
            mem.known
                .iter()
                .filter(|k| k.visible && k.awareness > 0.5 && Some(k.entity) != target)
                .map(|k| (k.entity, k.last_position.distance(seen.at)))
                .filter(|&(_, d)| d < 10.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(e, _)| e)
        });
        if let Some(e) = other {
            g.glance = Some((Some(e), now + 0.45));
        }
    }
    if down || g.glance.is_some_and(|(_, until)| now >= until) {
        g.glance = None;
    }
    let foe_at = |e: Entity| (seen.foe)(e).map(|foe| toward(foe.at, 1.55));
    let want = match g.glance {
        Some((Some(e), _)) => foe_at(e),
        Some((None, _)) => noise.map(|n| toward(n.position, 0.3)),
        None => None,
    }
    // Reviving a mate, at its chest.
    .or_else(|| f.tend.map(|chest| toward(chest, 0.0)))
    .or_else(|| match alert {
        _ if down => None,
        AlertLevel::Combat => target.and_then(foe_at),
        // Sweeping the ground ahead, holding a moment at each side.
        AlertLevel::Searching => {
            let phase = now * TAU as f64 / 4.2 + seed * 3.0;
            Some(Vec2::new(0.85 * (2.5 * phase.sin() as f32).tanh(), -0.08))
        }
        AlertLevel::Suspicious => noise
            .filter(|n| now - n.time < 6.0)
            .map(|n| toward(n.position, 0.3))
            .or_else(|| {
                let phase = now * TAU as f64 / 6.0 + seed * 3.0;
                Some(Vec2::new(0.5 * phase.sin() as f32, -0.03))
            }),
        AlertLevel::Relaxed => None,
    });
    let look = g.step(now, dt, f.turned, want, seed);

    // Blinks.
    if now >= g.next_blink {
        g.blink = now;
        g.blinks = g.blinks.wrapping_add(1);
        let h = hash(seed, g.blinks);
        let gap = if alert == AlertLevel::Relaxed {
            lerp(2.4, 5.5, h)
        } else {
            lerp(1.1, 2.8, h)
        };
        g.next_blink = now + f64::from(gap);
    }
    let u = (now - g.blink) as f32;
    let mut lids = step(0.0, 0.05, u) * (1.0 - step(0.09, 0.2, u));
    let mut blink = lids;
    // The breath: quick and deep through the mouth after a sprint, or badly hurt.
    let health = f.vitals.map_or(1.0, |v| v.fraction());
    let wounded = f.vitals.map_or(0.0, |_| 1.0 - step(0.3, 0.4, health));
    let sprint = step(3.8, 5.5, f.speed);
    g.winded = (g.winded + dt * (0.2 * sprint - 0.05)).clamp(0.0, 1.0);
    let hard = g.winded.max(0.7 * wounded);
    // The expression: a wound aches; running hard, and a breath still short, is an effort; pinned
    // down, or badly hurt in a fight, it is afraid.
    let mut pain = (0.6 * wounded).max(0.4 * (1.0 - health));
    let mut effort = (0.7 * sprint).max(0.3 * g.winded);
    let fear = (0.9 * f.suppression).max(if fighting { 0.6 * wounded } else { 0.0 });
    if f.tend.is_some() {
        effort = effort.max(0.35);
    }
    g.breath += f64::from(dt * TAU / lerp(3.8, 1.2, hard));
    let breath = g.breath.sin() as f32;
    let mut jaw = hard * (0.3 + 0.3 * (1.0 + breath));
    // Struck: the eyes squeezed and the teeth bared.
    if let Some(h) = f.hurt {
        let e = (now - h.since) as f32;
        let k = step(0.0, 0.03, e) * (1.0 - step(0.25, 0.7, e)) * h.strength.min(1.0);
        lids = lids.max(0.65 * k);
        jaw = jaw.max(0.25 * k);
        pain = pain.max(k);
    }
    match f.act {
        Act::Windup { p, heavy: true } if f.melee => {
            let set = step(0.1, 0.4, p);
            lids = lids.max(0.35 * set);
            jaw *= 1.0 - set;
            effort = effort.max(set);
        }
        Act::Strike { t, heavy: true, .. } if f.melee => {
            let shout = step(0.0, 0.04, t) * (1.0 - step(0.2, 0.45, t));
            lids = lids.max(0.4 * shout);
            jaw = jaw.max(0.7 * shout);
            effort = effort.max(shout);
        }
        // A light blow, a draw or an aim held, a guard braced, a stagger fought.
        Act::Windup { p, .. } => effort = effort.max(0.45 * step(0.1, 0.6, p)),
        Act::Strike { t, .. } => effort = effort.max(0.45 * (1.0 - step(0.1, 0.3, t))),
        Act::Block { k, .. } => effort = effort.max(0.6 * k),
        Act::Stagger { k, .. } => pain = pain.max(0.5 * k),
        _ => {}
    }
    let mut slack = 0.0;
    if let Some(fall) = f.fall {
        lids = lids.max(step(0.1, 0.5, fall.t));
        jaw = lerp(jaw, 0.3, step(0.2, 0.8, fall.t));
        slack = step(0.1, 0.5, fall.t);
        blink = blink.max(slack);
    }
    let calm = 1.0 - slack;
    let expression = Expression {
        blink,
        pain: pain.min(1.0) * calm,
        effort: effort.min(1.0) * calm,
        fear: fear.min(1.0) * calm,
        pant: hard * calm,
        breath,
        jaw,
        slack,
    };
    (look, lids, jaw, breath * (1.0 + 2.0 * hard), expression)
}

/// Pieces held in the hands rather than worn (the weapon and its pieces, the shield, a
/// magazine or arrow in hand, the muzzle flash): they stay when a character module draws the
/// body.
pub(super) fn held(part: Part) -> bool {
    const HELD: [Part; 12] = [
        Part::Weapon,
        Part::Shield,
        Part::Muzzle,
        HILT,
        LIMB_UPPER,
        LIMB_LOWER,
        STRING_UPPER,
        STRING_LOWER,
        ARROW,
        MAGAZINE,
        PLAIN_WEAPON,
        PLAIN_SHIELD,
    ];
    slot(part).is_some_and(|i| HELD.iter().any(|&h| slot(h) == Some(i)))
}

/// Every part's pose.
fn place(s: &Skeleton, kind: Kind, flash: bool, spin: f32, sway: &Sway) -> [Placed; SLOTS] {
    let mut out = [HIDDEN; SLOTS];
    let mut put = |part: Part, b: Bone| {
        if let Some(i) = slot(part) {
            out[i] = b.placed();
        }
    };
    put(Part::Torso, s.chest);
    put(Part::Head, s.head);
    put(FACE, s.head);
    for (k, part) in [EYE_L, EYE_R].into_iter().enumerate() {
        let e = rig::EYES;
        let at = if k == 0 { v3(-e.x, e.y, e.z) } else { e };
        put(part, s.head.then(Bone::new(at, s.eyes[k])));
    }
    // The lids turn down about the line through the eyes' centers; the jaw drops and turns
    // down about its hinge.
    let shut = rx(-rig::LIDS_SHUT * s.lids);
    let pivot = rig::LIDS_PIVOT;
    put(LIDS, s.head.then(Bone::new(pivot - shut * pivot, shut)));
    let open = rx(-rig::JAW_TURN * s.jaw);
    let hinge = rig::JAW_HINGE;
    let drop = v3(0.0, -rig::JAW_DROP * s.jaw, 0.0);
    put(
        JAW,
        s.head.then(Bone::new(hinge - open * hinge + drop, open)),
    );
    // The crest swings about the middle of the helmet's dome; the tabards and the pouches
    // about where they hang from.
    let crest = sway.crest.rot();
    let pivot = rig::CREST_PIVOT;
    put(
        Part::Band,
        s.head.then(Bone::new(pivot - crest * pivot, crest)),
    );
    put(Part::Orb, s.head);
    put(PELVIS, s.pelvis);
    for (k, part) in [TABARD_FRONT, TABARD_BACK].into_iter().enumerate() {
        put(
            part,
            s.tabard[k].then(Bone::new(Vec3::ZERO, sway.tabard[k].rot())),
        );
    }
    for (k, part) in [POUCH_R, POUCH_L].into_iter().enumerate() {
        let (top, yaw) = rig::POUCHES[k];
        put(
            part,
            s.pelvis.then(Bone::new(top, ry(yaw) * sway.pouch[k].rot())),
        );
    }
    put(Part::LegL, s.thigh[0]);
    put(Part::LegR, s.thigh[1]);
    put(SHIN_L, s.shin[0]);
    put(SHIN_R, s.shin[1]);
    put(FOOT_L, s.foot[0]);
    put(FOOT_R, s.foot[1]);
    put(Part::ArmL, s.upper[0]);
    put(ARMBAND, s.upper[0]);
    put(Part::ArmR, s.upper[1]);
    put(FOREARM_L, s.fore[0]);
    put(FOREARM_R, s.fore[1]);
    put(HAND_L, s.hand[0]);
    put(HAND_R, s.hand[1]);
    put(Part::Weapon, s.weapon);
    put(HILT, s.weapon);
    put(Part::Shield, s.shield);
    if flash {
        put(
            Part::Muzzle,
            s.weapon.then(Bone::new(weapons::RIFLE_MUZZLE, rz(spin))),
        );
    }
    if kind == Kind::Rifle {
        let seated = s.weapon.then(Bone::new(weapons::RIFLE_MAG, Quat::IDENTITY));
        put(MAGAZINE, s.held.unwrap_or(seated));
    }
    if kind == Kind::Bow {
        let pivot = weapons::BOW_LIMB_PIVOT;
        let upper = Bone::new(pivot, rx(s.bend));
        let lower = Bone::new(v3(0.0, -pivot.y, pivot.z), rz(PI) * rx(s.bend));
        put(LIMB_UPPER, s.weapon.then(upper));
        put(LIMB_LOWER, s.weapon.then(lower));
        let tip = bow_tip(s.bend);
        let nock = v3(0.0, 0.0, weapons::BOW_TIP.z + s.draw);
        for (part, end) in [
            (STRING_UPPER, tip),
            (STRING_LOWER, v3(tip.x, -tip.y, tip.z)),
        ] {
            let mid = (end + nock) * 0.5;
            let rot = Quat::from_rotation_arc(Vec3::Y, (end - nock).normalize_or(Vec3::Y));
            put(part, s.weapon.then(Bone::new(mid, rot)));
        }
        if s.nocked {
            put(ARROW, s.weapon.then(Bone::new(nock, Quat::IDENTITY)));
        } else if let Some(arrow) = s.held {
            put(ARROW, arrow);
        }
        put(QUIVER, s.chest);
    }
    // Capsule stand-ins.
    let center = |b: Bone, y: f32| b.then(Bone::new(v3(0.0, y, 0.0), Quat::IDENTITY));
    put(PLAIN_TRUNK, center(s.chest, 0.2));
    put(PLAIN_HEAD, center(s.head, 0.15));
    let segs = [
        (s.thigh, rig::THIGH),
        (s.shin, rig::SHIN),
        (s.upper, rig::UPPER_ARM),
        (s.fore, rig::FOREARM),
    ];
    for (n, (bones, len)) in segs.into_iter().enumerate() {
        for (k, bone) in bones.into_iter().enumerate() {
            let part = Part::Fighter(PLAIN_SEG + (n * 2 + k) as u8);
            put(part, center(bone, -len * 0.5));
        }
    }
    let reach = if matches!(kind, Kind::Longsword | Kind::SwordShield) {
        v3(0.0, 0.0, -0.45)
    } else {
        v3(0.0, 0.05, -0.2)
    };
    if kind != Kind::Fists {
        put(
            PLAIN_WEAPON,
            s.weapon.then(Bone::new(reach, Quat::IDENTITY)),
        );
    }
    if kind == Kind::SwordShield {
        put(
            PLAIN_SHIELD,
            s.shield
                .then(Bone::new(v3(0.0, 0.0, -0.06), Quat::IDENTITY)),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: Kind, act: Act, fighting: bool) -> Input {
        let ready = fighting || act != Act::Relaxed;
        let gait = Gait::new(Vec2::ZERO, 0.0, ready);
        Input {
            now: 1.0,
            feet: gait.steady(rest_feet(kind, ready), ready),
            gait,
            accel: Vec2::ZERO,
            crouch: false,
            kind,
            act,
            windup: 0.45,
            dodge: 0.0,
            fighting,
            hurt: None,
            dash: None,
            fall: None,
            behind: 0.0,
            shot: None,
            look: Look::default(),
            lids: 0.0,
            jaw: 0.0,
            breath: 0.0,
            wound: None,
            up: if ready { 1.0 } else { 0.0 },
            wary: 0.0,
            poise: if ready { 1.0 } else { 0.0 },
            rise: None,
            rising: 0.0,
            tend: None,
            seed: 0.0,
        }
    }

    /// Walking steadily at `vel` (owner frame), `cycle` of the way through a stride.
    fn walking(kind: Kind, vel: Vec2, cycle: f32) -> Input {
        let gait = Gait::new(vel, cycle, false);
        Input {
            feet: gait.steady(rest_feet(kind, false), false),
            gait,
            ..input(kind, Act::Relaxed, false)
        }
    }

    #[test]
    fn hands_reach_their_grips() {
        let mut acts = vec![Act::Relaxed, Act::Guard];
        for e in [0.0, 0.04, 0.2, 0.6] {
            acts.push(Act::Stagger { k: 1.0, e });
        }
        for heavy in [false, true] {
            for p in [0.05, 0.2, 0.4, 0.6, 0.8, 0.9, 0.95, 1.0] {
                acts.push(Act::Windup { p, heavy });
            }
            for t in [0.0, 0.03, 0.08, 0.12, 0.2, 0.3, 0.4] {
                acts.push(Act::Strike {
                    t,
                    dur: 0.45,
                    heavy,
                });
            }
        }
        for line in Line::ALL {
            for parry in [false, true] {
                for hit in [-0.1, 0.0, 0.03, 0.1, 0.25] {
                    acts.push(Act::Block {
                        k: 1.0,
                        hit,
                        parry,
                        line,
                    });
                }
            }
        }
        for heavy in [false, true] {
            for u in [0.0, 0.04, 0.09, 0.2, 0.35] {
                acts.push(Act::Check { p: 0.55, u, heavy });
            }
        }
        // Struck in each place, or pushing off into a dash.
        let blow = |zone| {
            let push = Vec2::new(0.3, 0.95);
            Hurt {
                since: 0.0,
                zone,
                side: -1.0,
                push,
                strength: 1.2,
            }
        };
        let extra = [
            (None, None),
            (Some(blow(Zone::Head)), None),
            (Some(blow(Zone::Body)), None),
            (Some(blow(Zone::Legs)), None),
            (None, Some((0.15, Vec2::NEG_Y, 0.5))),
        ];
        let mut worst = 0.0f32;
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            for &act in &acts {
                for crouch in [false, true] {
                    for (hurt, dash) in extra {
                        let mut i = input(kind, act, act != Act::Relaxed);
                        i.crouch = crouch;
                        i.hurt = hurt.map(|h| (0.08, h));
                        i.dash = dash;
                        let s = solve(&i);
                        let m = [0, 1].map(|k| (s.want[k] - s.hand[k].pos).length());
                        if m[0].max(m[1]) > 0.005 {
                            eprintln!(
                                "{kind:?} {act:?} crouch {crouch} {:?} {dash:?}: {:.3} {:.3}",
                                hurt.map(|h| h.zone),
                                m[0],
                                m[1]
                            );
                        }
                        worst = worst.max(m[0]).max(m[1]);
                    }
                }
            }
        }
        assert!(worst < 0.03, "a hand misses its grip by {worst:.3} m");

        // At ease (the weapon carried, or at the low ready), coming up to guard or going down
        // from it, badly hurt (a hand pressed to the wound), and getting up off one knee.
        let wounds = [
            None,
            Some((Zone::Legs, -1.0, 1.0)),
            Some((Zone::Legs, 1.0, 0.6)),
            Some((Zone::Body, 1.0, 1.0)),
            Some((Zone::Body, -1.0, 0.5)),
        ];
        let mut worst = 0.0f32;
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            for act in [Act::Relaxed, Act::Guard] {
                for up in [0.0, 0.1, 0.25, 0.5, 0.75, 0.95, 1.0] {
                    for wary in [0.0, 0.5, 1.0] {
                        for wound in wounds {
                            for rising in [0.0, 0.5, 1.0] {
                                let mut i = input(kind, act, act != Act::Relaxed);
                                (i.up, i.wary, i.poise, i.wound, i.rising) =
                                    (up, wary, up, wound, rising);
                                let s = solve(&i);
                                let m = [0, 1].map(|k| (s.want[k] - s.hand[k].pos).length());
                                if m[0].max(m[1]) > 0.005 {
                                    eprintln!(
                                        "{kind:?} {act:?} up {up} wary {wary} {wound:?} \
                                         rising {rising}: {:.3} {:.3}",
                                        m[0], m[1]
                                    );
                                }
                                worst = worst.max(m[0]).max(m[1]);
                            }
                        }
                    }
                }
            }
        }
        assert!(worst < 0.03, "a hand misses its grip by {worst:.3} m");
    }

    #[test]
    fn feet_stay_on_the_ground_and_limbs_keep_their_length() {
        for speed in [0.0, 1.5, 3.0, 5.5] {
            for k in 0..16 {
                let s = solve(&walking(
                    Kind::Longsword,
                    Vec2::new(0.0, -speed),
                    k as f32 / 16.0,
                ));
                for side in 0..2 {
                    let thigh = (s.shin[side].pos - s.thigh[side].pos).length();
                    let shin = (s.foot[side].pos - s.shin[side].pos).length();
                    assert!((thigh - rig::THIGH).abs() < 1e-3);
                    assert!((shin - rig::SHIN).abs() < 1e-3);
                    assert!(s.foot[side].pos.y > 0.0 && s.foot[side].pos.y < 0.4);
                }
                // One foot is always near the ground.
                let low = s.foot[0].pos.y.min(s.foot[1].pos.y);
                assert!(
                    low < rig::ANKLE_Y + 0.08,
                    "speed {speed}: both feet up ({low})"
                );
            }
        }
    }

    /// What the feet did on a walk.
    #[derive(Debug)]
    struct Steps {
        /// How far the body went (m).
        went: f32,
        /// How far the heels and balls of the feet slid while on the ground (m, summed), and
        /// the most in one frame.
        slid: f32,
        worst: f32,
        /// The share of the time each foot was on the ground, and how often it lifted.
        down: [f32; 2],
        lifts: [u32; 2],
        /// How low and how high the hips went (m).
        hips: (f32, f32),
        /// How far the body turned (rad); the most a foot was wrung round from the hips (rad,
        /// beyond how it stands at rest), and how far at the end.
        turned: f32,
        wrung: f32,
        settled: f32,
        /// After the first second: how high each foot went (its lower end, m), and how low
        /// the hips went while that foot alone was down (m).
        high: [f32; 2],
        dip: [f32; 2],
    }

    /// Walk a fighter through `secs` s at 60 Hz, `motion(t)` giving its velocity (in its own
    /// frame, x and z) and how fast it turns (rad/s), and watch its feet in the world.
    fn steps(kind: Kind, ready: bool, secs: f32, motion: impl Fn(f32) -> (Vec2, f32)) -> Steps {
        steps_hurt(kind, ready, None, secs, motion)
    }

    /// [`steps`], badly hurt (see [`Input`]).
    fn steps_hurt(
        kind: Kind,
        ready: bool,
        wound: Option<(Zone, f32, f32)>,
        secs: f32,
        motion: impl Fn(f32) -> (Vec2, f32),
    ) -> Steps {
        const GROUND: f32 = 0.006;
        let dt = 1.0 / 60.0;
        let act = if ready { Act::Guard } else { Act::Relaxed };
        let mut m = Motion {
            hobble: hobble(wound),
            ..Default::default()
        };
        let (mut pos, mut yaw) = (Vec3::ZERO, 0.0f32);
        let mut last: Option<[[Vec3; 2]; 2]> = None;
        let mut out = Steps {
            went: 0.0,
            slid: 0.0,
            worst: 0.0,
            down: [0.0; 2],
            lifts: [0; 2],
            hips: (f32::MAX, f32::MIN),
            turned: 0.0,
            wrung: 0.0,
            settled: 0.0,
            high: [0.0; 2],
            dip: [f32::MAX; 2],
        };
        let heading = |b: &Bone| {
            let f = b.rot * Vec3::NEG_Z;
            (-f.x).atan2(-f.z)
        };
        let mut calm: Option<[f32; 2]> = None;
        let mut was_down = [true; 2];
        let n = (secs / dt).round() as usize;
        for f in 0..n {
            let t = f as f32 * dt;
            let (v, w) = motion(t);
            yaw += w * dt;
            out.turned += (w * dt).abs();
            let facing = ry(yaw);
            let vel = facing * Vec3::new(v.x, 0.0, v.y);
            if f > 0 {
                pos += vel * dt;
                out.went += vel.length() * dt;
            }
            let now = 1.0 + f64::from(t);
            let (gait, feet, _) = m.walk(now, 0.0, facing, vel, kind, ready, None, [Vec2::ZERO; 2]);
            let s = solve(&Input {
                now,
                feet,
                gait,
                accel: m.accel,
                behind: m.behind,
                wound,
                ..input(kind, act, ready)
            });
            // How far round from the hips each foot points.
            let twist = [0, 1].map(|k| wrap(heading(&s.foot[k]) - heading(&s.pelvis)));
            let calm = *calm.get_or_insert(twist);
            let off = [0, 1].map(|k| wrap(twist[k] - calm[k]).abs());
            out.settled = off[0].max(off[1]);
            for k in 0..2 {
                if feet[k].lift == 0.0 {
                    out.wrung = out.wrung.max(off[k]);
                }
            }
            // Heel and ball of each foot, in the world.
            let pts: [[Vec3; 2]; 2] = std::array::from_fn(|k| {
                [0.075, -0.12].map(|z| pos + facing * s.foot[k].at(v3(0.0, -rig::ANKLE_Y, z)))
            });
            for k in 0..2 {
                let on = pts[k].map(|p| p.y < GROUND);
                let grounded = on[0] || on[1];
                if grounded {
                    out.down[k] += 1.0 / n as f32;
                }
                if was_down[k] && !grounded {
                    out.lifts[k] += 1;
                }
                was_down[k] = grounded;
                if let Some(prev) = last {
                    for j in 0..2 {
                        if on[j] && prev[k][j].y < GROUND {
                            let d = pts[k][j] - prev[k][j];
                            let d = Vec2::new(d.x, d.z).length();
                            out.slid += d;
                            out.worst = out.worst.max(d);
                        }
                    }
                }
            }
            out.hips = (
                out.hips.0.min(s.pelvis.pos.y),
                out.hips.1.max(s.pelvis.pos.y),
            );
            if t >= 1.0 {
                for k in 0..2 {
                    out.high[k] = out.high[k].max(pts[k][0].y.min(pts[k][1].y));
                    if feet[k].lift == 0.0 && feet[1 - k].lift > 0.0 {
                        out.dip[k] = out.dip[k].min(s.pelvis.pos.y);
                    }
                }
            }
            last = Some(pts);
        }
        out
    }

    /// How a test walk goes: velocity (own frame) and turn rate at each moment.
    type Course<'a> = &'a dyn Fn(f32) -> (Vec2, f32);

    #[test]
    fn feet_do_not_slide() {
        let ramp = |t: f32| step(0.0, 0.6, t) * (1.0 - step(2.2, 2.8, t));
        let cases: [(&str, Kind, bool, f32, Course); 11] = [
            ("walk", Kind::Longsword, false, 4.0, &|_| {
                (Vec2::new(0.0, -1.6), 0.0)
            }),
            ("run", Kind::SwordShield, false, 3.0, &|_| {
                (Vec2::new(0.0, -4.5), 0.0)
            }),
            ("sprint", Kind::Bow, false, 3.0, &|_| {
                (Vec2::new(0.0, -6.8), 0.0)
            }),
            ("strafe", Kind::Longsword, true, 3.0, &|_| {
                (Vec2::new(1.8, 0.0), 0.0)
            }),
            ("back off", Kind::SwordShield, true, 3.0, &|_| {
                (Vec2::new(0.0, 1.2), 0.0)
            }),
            ("diagonal", Kind::Rifle, false, 3.0, &|_| {
                (Vec2::new(-1.1, -1.1), 0.0)
            }),
            ("start and stop", Kind::Longsword, false, 4.0, &|t| {
                (Vec2::new(0.0, -2.5 * ramp(t)), 0.0)
            }),
            ("turn on the spot", Kind::SwordShield, true, 2.5, &|t| {
                (Vec2::ZERO, if t < 1.2 { 2.5 } else { 0.0 })
            }),
            // As fast as a fighter turns (9 rad/s): about, at ease, and a quarter on guard.
            ("about-face", Kind::Longsword, false, 2.2, &|t| {
                (Vec2::ZERO, if t < 0.35 { 9.0 } else { 0.0 })
            }),
            ("snap round on guard", Kind::Rifle, true, 1.6, &|t| {
                (Vec2::ZERO, if t < 0.175 { -9.0 } else { 0.0 })
            }),
            ("walk round a bend", Kind::Longsword, false, 4.0, &|_| {
                (Vec2::new(0.0, -1.6), 0.8)
            }),
        ];
        for (name, kind, ready, secs, motion) in cases {
            let s = steps(kind, ready, secs, motion);
            eprintln!("{name}: {s:?}");
            assert!(
                s.slid <= 0.01 * s.went + 0.01 && s.worst < 0.005,
                "{name}: the feet slid {:.3} m over {:.2} m (at most {:.4} m in a frame)",
                s.slid,
                s.went,
                s.worst
            );
            if s.went > 1.0 {
                assert!(
                    s.lifts.iter().all(|&n| n >= 2),
                    "{name}: the feet hardly stepped ({:?})",
                    s.lifts
                );
            }
            assert!(
                s.down.iter().all(|&d| d > 0.2),
                "{name}: a foot was hardly down ({:?})",
                s.down
            );
            if s.turned > 1.0 && s.went < 0.5 {
                // Turning on the spot, the feet step round (pivoting on their balls), the hips
                // coming round over them, not spinning over feet planted the old way.
                assert!(
                    s.lifts.iter().all(|&n| n >= 1),
                    "{name}: the feet hardly stepped ({:?})",
                    s.lifts
                );
                assert!(
                    s.wrung < 0.6 && s.settled <= PIVOT,
                    "{name}: a foot was wrung {:.2} rad round from the hips ({:.2} at the end)",
                    s.wrung,
                    s.settled
                );
            }
            // The hips ride on the legs: they bob, but don't drop to let a leg reach.
            assert!(
                s.hips.0 > 0.78 && s.hips.1 - s.hips.0 < 0.12,
                "{name}: the hips went from {:.2} m to {:.2} m",
                s.hips.0,
                s.hips.1
            );
        }
    }

    #[test]
    fn guards_meet_the_line_of_the_blow() {
        // Read off the attacker: a heavy blow comes from above, a light cut from the side its
        // sword arm is on (or that it stands on), a dash attack straight in, a shot as one.
        let foe = |x: f64, z: f64, heavy: bool| Foe {
            at: DVec3::new(x, 0.0, z),
            heavy,
            dashing: false,
            ranged: false,
            damage: 25.0,
        };
        let line = |how, f: Foe, facing: Quat| Line::of(how, false, Some(&f), DVec3::ZERO, facing);
        let ahead = Quat::IDENTITY;
        assert_eq!(line(None, foe(0.0, -1.7, true), ahead), Line::High);
        assert_eq!(line(None, foe(0.0, -1.7, false), ahead), Line::Left);
        assert_eq!(line(None, foe(-1.2, -1.2, false), ahead), Line::Left);
        assert_eq!(line(None, foe(1.2, -1.2, false), ahead), Line::Right);
        assert_eq!(line(None, foe(-1.2, 1.2, false), ry(PI)), Line::Right);
        let dash = Some(AttackKind::Dash);
        assert_eq!(line(dash, foe(0.0, -1.7, false), ahead), Line::Thrust);
        let shot = Some(AttackKind::Ranged);
        assert_eq!(line(shot, foe(0.0, -9.0, false), ahead), Line::Square);
        let unseen = Line::of(Some(AttackKind::Melee), false, None, DVec3::ZERO, ahead);
        assert_eq!(unseen, Line::Square);

        // The guard goes there (in the chest's frame: x right, y up, −z ahead).
        let guard = |kind, line| {
            let act = Act::Block {
                k: 1.0,
                hit: -0.1,
                parry: false,
                line,
            };
            let s = solve(&input(kind, act, true));
            let local = |p: Vec3| s.chest.rot.inverse() * (p - s.chest.pos);
            let face = s.chest.rot.inverse() * (s.shield.rot * Vec3::NEG_Z);
            (
                local(s.weapon.pos),
                local(s.weapon.at(weapons::LONGSWORD_TIP)),
                local(s.shield.pos),
                face,
            )
        };
        let (grip, tip, ..) = guard(Kind::Longsword, Line::High);
        assert!(
            grip.y > 0.45 && tip.y > 0.45 && (tip.y - grip.y).abs() < 0.35,
            "across overhead: grip {grip:.2}, tip {tip:.2}"
        );
        let (grip, tip, ..) = guard(Kind::Longsword, Line::Left);
        assert!(
            tip.y > grip.y + 0.6 && grip.x < 0.0,
            "upright on the left: grip {grip:.2}, tip {tip:.2}"
        );
        let (grip, tip, ..) = guard(Kind::Longsword, Line::Right);
        assert!(
            tip.y > grip.y + 0.6 && grip.x > 0.1,
            "upright on the right: grip {grip:.2}, tip {tip:.2}"
        );
        let (grip, tip, ..) = guard(Kind::Longsword, Line::Thrust);
        assert!(
            grip.y < 0.25 && tip.z < grip.z - 0.5,
            "low, the point ahead: grip {grip:.2}, tip {tip:.2}"
        );
        let (.., face) = guard(Kind::SwordShield, Line::High);
        assert!(face.y > 0.5, "the shield faces up: {face:.2}");
        let (.., face) = guard(Kind::SwordShield, Line::Left);
        assert!(face.x < -0.3, "the shield faces left: {face:.2}");
        let (.., face) = guard(Kind::SwordShield, Line::Right);
        assert!(face.x > 0.3, "the shield faces right: {face:.2}");
        let (.., boss, _) = guard(Kind::SwordShield, Line::Thrust);
        assert!(boss.y < 0.25, "the shield low: {boss:.2}");
    }

    #[test]
    fn the_roof_guard_leaves_the_face_clear() {
        for (parry, hit) in [(false, -0.1), (false, 0.05), (true, -0.05), (true, 0.04)] {
            let act = Act::Block {
                k: 1.0,
                hit,
                parry,
                line: Line::High,
            };
            let s = solve(&input(Kind::Longsword, act, true));
            let local = |p: Vec3| s.chest.rot.inverse() * (p - s.chest.pos);
            let eye = local(s.head.at(rig::EYES));
            // Nothing of the arms in front of the face, from the chin to the brow.
            for k in 0..2 {
                let (elbow, wrist) = (local(s.fore[k].pos), local(s.hand[k].pos));
                let fist = local(s.hand[k].at(grip(k)));
                for n in 0..=20 {
                    let x = n as f32 / 20.0;
                    for p in [elbow.lerp(wrist, x), wrist.lerp(fist, x)] {
                        let level = (eye.y - 0.13..eye.y + 0.07).contains(&p.y);
                        let before = p.z < eye.z + 0.06;
                        assert!(
                            !(level && before) || (p.x - eye.x).abs() > 0.1,
                            "parry {parry} at {hit}: arm {k} at {p:.2} over the face at {eye:.2}"
                        );
                    }
                }
                // And the hands are up by the helmet, not down by the chin.
                assert!(fist.y > eye.y + 0.08, "arm {k}: the fist at {fist:.2}");
            }
        }
    }

    #[test]
    fn blows_throw_the_body_by_where_they_land() {
        let calm = solve(&input(Kind::Longsword, Act::Guard, true));
        let struck = |zone| {
            let mut i = input(Kind::Longsword, Act::Guard, true);
            let hurt = Hurt {
                since: 0.0,
                zone,
                side: -1.0,
                push: Vec2::new(0.0, 1.0),
                strength: 1.0,
            };
            i.hurt = Some((0.08, hurt));
            solve(&i)
        };
        let nod = |s: &Skeleton| s.chest.rot.inverse() * s.head.rot;
        let bowed = |s: &Skeleton| (s.chest.rot * Vec3::Y).z;
        // The head snaps back on the neck, the hips hardly moving.
        let head = struck(Zone::Head);
        let snapped = nod(&head).angle_between(nod(&calm));
        assert!(snapped > 0.3, "the head turned {snapped:.2} rad");
        assert!(head.pelvis.pos.y > calm.pelvis.pos.y - 0.03);
        // The trunk doubles over.
        let body = struck(Zone::Body);
        assert!(
            bowed(&body) < bowed(&calm) - 0.2,
            "the chest leans {:.2} (calm {:.2})",
            bowed(&body),
            bowed(&calm)
        );
        // The legs buckle.
        let legs = struck(Zone::Legs);
        assert!(
            legs.pelvis.pos.y < calm.pelvis.pos.y - 0.08,
            "the hips at {:.2} m (calm {:.2} m)",
            legs.pelvis.pos.y,
            calm.pelvis.pos.y
        );
    }

    #[test]
    fn the_downed_fall_and_settle_on_the_ground() {
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            for (yaw, bent, flung) in [(0.0, 0, 1), (0.6, 1, 0), (-0.8, 0, 0), (0.3, 1, 1)] {
                let pose = |t: f32| {
                    let mut i = input(kind, Act::Guard, true);
                    i.fall = Some(Fall {
                        t,
                        yaw,
                        bent,
                        flung,
                    });
                    solve(&i)
                };
                let joints = |s: &Skeleton| {
                    let mut v = vec![s.pelvis.pos, s.chest.pos, s.head.pos];
                    for k in 0..2 {
                        v.extend([s.shin[k].pos, s.foot[k].pos, s.fore[k].pos, s.hand[k].pos]);
                    }
                    v
                };
                let case = format!("{kind:?} falling along {yaw}");
                let mut last: Option<Vec<Vec3>> = None;
                for n in 0..=96 {
                    let t = n as f32 / 60.0;
                    let s = pose(t);
                    for k in 0..2 {
                        let len = |a: Bone, b: Bone| (b.pos - a.pos).length();
                        assert!((len(s.thigh[k], s.shin[k]) - rig::THIGH).abs() < 1e-3);
                        assert!((len(s.shin[k], s.foot[k]) - rig::SHIN).abs() < 1e-3);
                        assert!((len(s.upper[k], s.fore[k]) - rig::UPPER_ARM).abs() < 1e-3);
                        assert!((len(s.fore[k], s.hand[k]) - rig::FOREARM).abs() < 1e-3);
                        for z in [0.094, -0.213] {
                            let p = s.foot[k].at(v3(0.0, -rig::ANKLE_Y, z));
                            assert!(
                                p.y > -0.02,
                                "{case}: a foot in the ground at {t:.2} s ({p:.2})"
                            );
                        }
                    }
                    let now = joints(&s);
                    for p in &now {
                        assert!(
                            p.y > -0.02,
                            "{case}: a joint in the ground at {t:.2} s ({p:.2})"
                        );
                    }
                    // No joint jumps: none moves faster than a falling body does.
                    if let Some(prev) = &last {
                        let jump = now
                            .iter()
                            .zip(prev)
                            .map(|(a, b)| (*a - *b).length())
                            .fold(0.0, f32::max);
                        assert!(jump < 0.12, "{case}: a joint jumps {jump:.2} m at {t:.2} s");
                    }
                    last = Some(now);
                }
                // Settled: on its back along the fall, the limbs on the ground, the weapon
                // beside it.
                let s = pose(1.6);
                assert!(
                    s.pelvis.pos.y < 0.25 && s.chest.pos.y < 0.25 && s.head.pos.y < 0.2,
                    "{case}: lying at {:.2} {:.2} {:.2} m",
                    s.pelvis.pos.y,
                    s.chest.pos.y,
                    s.head.pos.y
                );
                for k in 0..2 {
                    assert!(s.hand[k].pos.y < 0.12 && s.foot[k].pos.y < 0.2, "{case}");
                }
                let along = ry(yaw) * Vec3::Z;
                assert!((s.head.pos - s.pelvis.pos).dot(along) > 0.4, "{case}");
                assert!(
                    s.weapon.pos.y < 0.06,
                    "{case}: the weapon at {:.2} m",
                    s.weapon.pos.y
                );
            }
        }
    }

    #[test]
    fn a_dash_pushes_off_the_rear_foot() {
        let dt = 1.0 / 60.0;
        let (secs, dist) = (0.22f32, 3.5f32);
        let went = |t: f32| {
            let u = (t / secs).clamp(0.0, 1.0);
            dist * (1.0 - (1.0 - u) * (1.0 - u))
        };
        const GROUND: f32 = 0.006;
        let mut m = Motion::default();
        let mut owner = 0.0f32;
        let mut last: Option<([[Vec3; 2]; 2], Vec3)> = None;
        let mut pushed = 0;
        let mut first: Option<(f32, f32)> = None;
        for f in 0..48 {
            // The dash begins at frame 6 (the mover is at full speed at once).
            let t = (f as f32 - 6.0) * dt;
            let now = 1.0 + f64::from(f as f32 * dt);
            let next = went(t);
            let vel = (next - owner) / dt;
            let step_went = next - owner;
            owner = next;
            let dashing = t > 0.0 && t <= secs + 1e-4;
            let (gait, feet, dash) = m.walk(
                now,
                0.0,
                Quat::IDENTITY,
                Vec3::new(0.0, 0.0, -vel),
                Kind::Longsword,
                true,
                dashing.then_some(secs),
                [Vec2::ZERO; 2],
            );
            let s = solve(&Input {
                now,
                feet,
                gait,
                dash,
                ..input(Kind::Longsword, Act::Guard, true)
            });
            let base = Vec3::new(0.0, 0.0, -owner);
            let pts: [[Vec3; 2]; 2] = std::array::from_fn(|k| {
                [0.075, -0.12].map(|z| base + s.foot[k].at(v3(0.0, -rig::ANKLE_Y, z)))
            });
            let hips = base + s.pelvis.pos;
            if let Some((prev, prev_hips)) = last {
                if dashing && first.is_none() {
                    first = Some((prev_hips.z - hips.z, step_went));
                }
                // The rear (right) foot drives: down, and not slipping, for the first frames.
                let ball = |p: [[Vec3; 2]; 2]| p[1][1];
                if dashing
                    && ball(pts).y < GROUND
                    && (ball(pts) - ball(prev)).length() < 0.003
                    && pushed + 7 == f
                {
                    pushed += 1;
                }
                // Once the dash is over and both feet are down, they stay put.
                if t > secs + 0.12 {
                    for k in 0..2 {
                        for j in 0..2 {
                            let d = (pts[k][j] - prev[k][j]).length();
                            assert!(d < 0.005, "foot {k} slides {d:.3} m at {t:.2} s");
                        }
                    }
                }
            }
            last = Some((pts, hips));
        }
        let (hips, owner_step) = first.unwrap();
        assert!(
            hips < 0.5 * owner_step,
            "the body moved {hips:.2} m in the dash's first frame, the owner {owner_step:.2} m"
        );
        assert!(pushed >= 2, "the rear foot drove for {pushed} frames");
        // In the end both feet stand in the stance under the body.
        let (pts, hips) = last.unwrap();
        let base = Vec3::new(0.0, 0.0, -owner);
        for (k, (at, _)) in rest_feet(Kind::Longsword, true).into_iter().enumerate() {
            let heel = pts[k][0] - base;
            assert!(heel.y < GROUND);
            let off = Vec2::new(heel.x, heel.z) - at;
            assert!(off.length() < 0.12, "foot {k} ends {off:.2} off its place");
        }
        assert!((hips - base).z.abs() < 0.15);
    }

    /// Pose a fighter (standing at the origin, on guard) at 60 Hz through `secs` s from 1 s
    /// on: with `weapon`, in the combat state `combat(now)` gives, taking the blows
    /// `blows(now)` gives, moving at `vel(now)` (world, m/s) and facing `yaw(now)`; each pose
    /// is shown to `look`.
    struct Run<'a> {
        secs: f64,
        weapon: NpcWeapon,
        combat: &'a dyn Fn(f64) -> NpcCombat,
        blows: &'a dyn Fn(f64) -> Vec<DamageEvent>,
        vel: &'a dyn Fn(f64) -> Vec3,
        yaw: &'a dyn Fn(f64) -> f32,
        /// What it is about (alert level and target; in a fight unless it says otherwise), how
        /// it is (whole unless it says otherwise; downed from the first moment it says so),
        /// where the other fighters are, what it knows and the attacks telegraphed.
        brain: &'a dyn Fn(f64) -> NpcBrain,
        vitals: &'a dyn Fn(f64) -> Option<NpcVitals>,
        others: &'a dyn Fn(Entity) -> Option<DVec3>,
        memory: &'a dyn Fn(f64) -> Option<NpcMemory>,
        attacks: &'a dyn Fn(f64) -> Vec<(Entity, Option<Entity>, AttackKind)>,
        /// Reviving a downed mate, where its chest lies (world).
        patient: &'a dyn Fn(f64) -> Option<(DVec3, DVec3)>,
    }

    impl Run<'_> {
        fn new(secs: f64, weapon: NpcWeapon) -> Self {
            Run {
                secs,
                weapon,
                combat: &|_| NpcCombat::default(),
                blows: &|_| Vec::new(),
                vel: &|_| Vec3::ZERO,
                yaw: &|_| 0.0,
                brain: &|_| NpcBrain {
                    alert: AlertLevel::Combat,
                    ..Default::default()
                },
                vitals: &|_| None,
                others: &|_| None,
                memory: &|_| None,
                attacks: &|_| Vec::new(),
                patient: &|_| None,
            }
        }

        fn look(&self, mut look: impl FnMut(f64, &dyn Fn(Part) -> Placed)) {
            let mut st = StandInState::default();
            let others = self.others;
            let foe = |e: Entity| {
                others(e).map(|at| Foe {
                    at,
                    heavy: false,
                    dashing: false,
                    ranged: false,
                    damage: 25.0,
                })
            };
            let mut down_since = None;
            for f in 0..(self.secs * 60.0) as usize {
                let now = 1.0 + f as f64 / 60.0;
                let c = (self.combat)(now);
                let landed = (self.blows)(now);
                let attacks = (self.attacks)(now);
                let memory = (self.memory)(now);
                let brain = (self.brain)(now);
                let vitals = (self.vitals)(now);
                let downed = match vitals {
                    Some(v) if v.downed => Some(Downed {
                        since: *down_since.get_or_insert(now),
                        push: None,
                    }),
                    _ => {
                        down_since = None;
                        None
                    }
                };
                let seen = Seen {
                    me: Entity::PLACEHOLDER,
                    vary: ((Entity::PLACEHOLDER.to_bits() as u32) as f32 * 0.618_034).fract(),
                    at: DVec3::ZERO,
                    attacks: &attacks,
                    blows: &landed,
                    dash_secs: 0.22,
                    foe: &foe,
                    memory: memory.as_ref(),
                    patient: (self.patient)(now),
                };
                let loco = LocomotionState {
                    velocity: (self.vel)(now),
                    ..Default::default()
                };
                let mut materials = Vec::new();
                let pose = humanoid(
                    now,
                    0.0,
                    ry((self.yaw)(now)),
                    &loco,
                    None,
                    Some(&c),
                    Some(&brain),
                    vitals.as_ref(),
                    downed.as_ref(),
                    self.weapon,
                    &seen,
                    &mut st,
                    &mut materials,
                    &StandIn::default(),
                );
                look(now, &pose);
            }
        }
    }

    /// The blade tip (owner frame) of a longsword fighter posed at 60 Hz through `secs` s,
    /// in the combat state `combat(now)` gives.
    fn blade_path(secs: f64, combat: impl Fn(f64) -> NpcCombat) -> Vec<Vec3> {
        let mut path = Vec::new();
        Run {
            combat: &combat,
            ..Run::new(secs, NpcWeapon::sword())
        }
        .look(|_, pose| {
            let p = pose(Part::Weapon);
            path.push(p.pos + p.rot * weapons::LONGSWORD_TIP);
        });
        path
    }

    /// A shooter's combat state, firing as fast as `weapon` allows from `first` s on: each
    /// shot telegraphed (aimed or drawn) for the windup, then the recovery, ready again after
    /// the cooldown. The shots' times.
    fn firing(weapon: NpcWeapon, first: f64, until: f64) -> (impl Fn(f64) -> NpcCombat, Vec<f64>) {
        let (windup, recovery, cooldown) = (
            f64::from(weapon.windup),
            f64::from(weapon.recovery),
            f64::from(weapon.cooldown),
        );
        let every = cooldown + windup;
        let shots: Vec<f64> = (0..)
            .map(|n| first + n as f64 * every)
            .take_while(|&t| t < until)
            .collect();
        let state = move |now: f64| {
            let last = first + ((now - first) / every).floor() * every;
            let next = if now < first { first } else { last + every };
            let phase = if now >= first && now < last + recovery {
                AttackPhase::Recovery {
                    until: last + recovery,
                }
            } else if now >= next - windup {
                AttackPhase::Windup {
                    target: None,
                    hit_at: next,
                    heavy: false,
                    feint: false,
                    aim: DVec3::ZERO,
                }
            } else {
                AttackPhase::Ready
            };
            NpcCombat {
                phase,
                ready_at: if now < first { 0.0 } else { last + cooldown },
                ..Default::default()
            }
        };
        (state, shots)
    }

    #[test]
    fn a_carbine_kicks_settles_and_changes_its_magazine_between_shots() {
        let (combat, shots) = firing(NpcWeapon::rifle(), 1.5, 7.9);
        let mut frames = Vec::new();
        Run {
            combat: &combat,
            ..Run::new(7.0, NpcWeapon::rifle())
        }
        .look(|now, pose| {
            frames.push((
                now,
                pose(Part::Weapon),
                pose(MAGAZINE),
                pose(HAND_L),
                pose(POUCH_L),
            ));
        });
        let seat = |w: &Placed| w.pos + w.rot * weapons::RIFLE_MAG;
        let out = |f: &(f64, Placed, Placed, Placed, Placed)| (f.2.pos - seat(&f.1)).length();
        let fist = |f: &(f64, Placed, Placed, Placed, Placed)| f.3.pos + f.3.rot * grip(0);
        // The magazine comes out, down to the pouch on the left hip and back in, the support
        // hand holding it all the while it's out.
        let most = frames.iter().map(out).fold(0.0, f32::max);
        assert!(most > 0.1, "the magazine comes out {most:.2} m");
        let pouch = frames
            .iter()
            .map(|f| (f.2.pos - f.4.pos).length())
            .fold(f32::MAX, f32::min);
        assert!(
            pouch < 0.2,
            "the magazine comes within {pouch:.2} m of the pouch"
        );
        for f in frames.iter().filter(|f| out(f) > 0.02) {
            let on = f.2.pos + f.2.rot * weapons::RIFLE_MAG_GRIP;
            let off = (fist(f) - on).length();
            assert!(
                off < 0.03,
                "at {:.2} s the hand is {off:.2} m off the magazine",
                f.0
            );
        }
        // Every shot goes off with the magazine in and both hands on the gun, and the muzzle
        // kicks up and settles back where it was.
        let mut kicks = 0;
        for &t in shots.iter().filter(|&&t| t < 7.9) {
            let Some(i) = frames.iter().position(|f| f.0 >= t) else {
                continue;
            };
            let f = &frames[i];
            let grip = f.1.pos + f.1.rot * weapons::RIFLE_LEFT_GRIP;
            assert!(
                out(f) < 0.002,
                "at the shot at {t:.2} s the magazine is out"
            );
            assert!(
                (fist(f) - grip).length() < 0.03,
                "at the shot at {t:.2} s the support hand is off the handguard"
            );
            // How far the barrel points up (rad).
            let muzzle = |f: &(f64, Placed, Placed, Placed, Placed)| {
                let d = f.1.rot * Vec3::NEG_Z;
                d.y.asin()
            };
            let after: Vec<_> = frames[i..].iter().take_while(|g| g.0 < t + 0.4).collect();
            if after.iter().any(|g| out(g) > 0.002) || after.len() < 20 {
                continue;
            }
            let aimed = muzzle(&frames[i - 1]);
            let top = after.iter().map(|g| muzzle(g)).fold(f32::MIN, f32::max);
            let settled = muzzle(after.last().unwrap());
            assert!(
                top > aimed + 0.08,
                "the muzzle climbs {:.3} rad",
                top - aimed
            );
            assert!(
                (settled - aimed).abs() < 0.02,
                "the muzzle settles {:.3} rad off",
                settled - aimed
            );
            kicks += 1;
        }
        assert!(kicks >= 3, "only {kicks} shots without a reload after them");
    }

    #[test]
    fn an_archer_anchors_holds_looses_and_nocks_the_next_arrow() {
        let bow = NpcWeapon::bow();
        let (combat, shots) = firing(bow, 1.9, 7.0);
        let mut frames = Vec::new();
        Run {
            combat: &combat,
            ..Run::new(6.0, bow)
        }
        .look(|now, pose| {
            frames.push((
                now,
                pose(Part::Weapon),
                pose(ARROW),
                pose(HAND_R),
                pose(Part::Head),
                pose(Part::Torso),
            ));
        });
        type Frame = (f64, Placed, Placed, Placed, Placed, Placed);
        let fist = |f: &Frame| f.3.pos + f.3.rot * grip(1);
        let string = |f: &Frame| f.1.pos + f.1.rot * v3(0.0, 0.0, weapons::BOW_TIP.z);
        for &t in &shots {
            let Some(i) = frames.iter().position(|f| f.0 >= t) else {
                continue;
            };
            // At full draw the nock is at the corner of the mouth, held there.
            let drawn = &frames[i - 1];
            let anchor = drawn.4.pos + drawn.4.rot * ANCHOR;
            let off = (drawn.2.pos - anchor).length();
            assert!(off < 0.05, "drawn, the nock is {off:.2} m off the anchor");
            let held: Vec<&Frame> = frames[..i].iter().filter(|f| f.0 > t - 0.15).collect();
            let crept = (held[0].2.pos - drawn.2.pos).length();
            assert!(crept < 0.03, "the nock creeps {crept:.2} m in the hold");
            // Loosed, there is no arrow on the string; then one comes up out of the quiver
            // in the string hand and is nocked before the bow is ready again.
            let ready = t + f64::from(bow.cooldown);
            let after: Vec<&Frame> = frames[i..].iter().take_while(|f| f.0 < ready).collect();
            if after.len() + i >= frames.len() {
                continue;
            }
            assert!(
                after
                    .iter()
                    .take_while(|f| f.0 < t + 0.3)
                    .all(|f| !f.2.visible)
            );
            let first = after
                .iter()
                .find(|f| f.2.visible)
                .expect("an arrow comes out of the quiver");
            let mouth = first.5.pos + first.5.rot * weapons::QUIVER_MOUTH;
            let from = (first.2.pos - mouth).length();
            assert!(
                from < 0.15,
                "the arrow comes out {from:.2} m from the quiver"
            );
            for f in after.iter().filter(|f| f.2.visible) {
                let off = (fist(f) - f.2.pos).length();
                assert!(
                    off < 0.03,
                    "at {:.2} s the hand is {off:.2} m off the arrow",
                    f.0
                );
            }
            let last = after.last().unwrap();
            let off = (last.2.pos - string(last)).length();
            assert!(
                last.2.visible && off < 0.01,
                "ready again, the arrow is {off:.2} m off the string"
            );
        }
    }

    #[test]
    fn a_stop_throws_the_tabards_on_and_they_swing_back_and_settle() {
        let s = solve(&input(Kind::Longsword, Act::Relaxed, false));
        let mut sway = Sway::default();
        let mut pitch = Vec::new();
        for f in 0..300 {
            let t = f as f32 / 60.0;
            // Running at 4.5 m/s, stopping over a fifth of a second at 1 s.
            let v = 4.5 * (1.0 - step(1.0, 1.2, t));
            sway.step(1.0 + f64::from(t), &s, Vec2::new(0.0, -v), 0.0);
            pitch.push((t, sway.tabard[0].a.y, sway.pouch[0].a, sway.crest.a));
        }
        let fwd = pitch.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(fwd > 0.15, "the tabard swings on {fwd:.2} rad");
        let top = pitch.iter().position(|p| p.1 == fwd).unwrap();
        let back = pitch[top..].iter().map(|p| p.1).fold(f32::MAX, f32::min);
        assert!(back < -0.02, "and back past hanging {back:.2} rad");
        let pouch = pitch.iter().map(|p| p.2.length()).fold(0.0, f32::max);
        assert!(pouch > 0.15, "the pouch swings {pouch:.2} rad");
        let end = pitch.last().unwrap();
        assert!(
            end.1.abs() < 0.01 && end.2.length() < 0.01 && end.3.length() < 0.01,
            "they settle: {end:?}"
        );
        // A head snapped back by a blow throws the crest forward over it.
        let mut sway = Sway::default();
        let mut lean = 0.0f32;
        for f in 0..90 {
            let t = f as f32 / 60.0;
            let mut s = s;
            let back = 0.4 * step(0.5, 0.56, t) * (1.0 - step(0.6, 0.9, t));
            s.head.rot *= rx(back);
            sway.step(1.0 + f64::from(t), &s, Vec2::ZERO, 0.0);
            lean = lean.max(sway.crest.a.y.abs());
        }
        assert!(lean > 0.05, "the crest swings {lean:.2} rad");
    }

    #[test]
    fn pouches_and_crest_swing_on_after_a_stop_and_a_blow() {
        let (stop, hit_at) = (2.0, 3.0);
        let blows = |now: f64| {
            if (now - hit_at).abs() < 0.008 {
                vec![DamageEvent {
                    target: Entity::PLACEHOLDER,
                    amount: 25.0,
                    kind: DamageKind::Slash,
                    point: DVec3::new(-0.1, 1.62, -0.1),
                    impulse: Vec3::new(0.35, 0.0, 1.0).normalize() * 75.0,
                    source: None,
                }]
            } else {
                Vec::new()
            }
        };
        let vel = |now: f64| {
            if now < stop {
                Vec3::new(0.0, 0.0, -4.5)
            } else {
                Vec3::ZERO
            }
        };
        let mut frames = Vec::new();
        Run {
            blows: &blows,
            vel: &vel,
            ..Run::new(3.5, NpcWeapon::sword())
        }
        .look(|now, pose| {
            let hips = pose(PELVIS).rot * ry(rig::POUCHES[0].1);
            let pouch = (hips.inverse() * pose(POUCH_R).rot).angle_between(Quat::IDENTITY);
            let crest = pose(Part::Head).rot.angle_between(pose(Part::Band).rot);
            frames.push((now, pouch, crest));
        });
        let most = |from: f64, to: f64, k: usize| {
            frames
                .iter()
                .filter(|f| (from..to).contains(&f.0))
                .map(|f| if k == 1 { f.1 } else { f.2 })
                .fold(0.0, f32::max)
        };
        let swung = most(stop, stop + 0.6, 1);
        assert!(
            swung > 0.1,
            "the pouch swings on {swung:.2} rad as the body stops"
        );
        let still = most(hit_at - 0.1, hit_at, 1).max(most(hit_at - 0.1, hit_at, 2));
        assert!(still < 0.03, "and settles ({still:.2} rad)");
        let thrown = most(hit_at, hit_at + 0.4, 2);
        assert!(
            thrown > 0.05,
            "the crest swings {thrown:.2} rad as the head is hit"
        );
        let end = most(4.3, 4.5, 1).max(most(4.3, 4.5, 2));
        assert!(end < 0.02, "and settles ({end:.2} rad)");
    }

    #[test]
    fn a_blow_to_the_head_turns_it_away_and_stumbles_back_a_half_step() {
        let hit_at = 1.1;
        // A cut on the left of the head, knocking it back and to the right.
        let blows = |now: f64| {
            if (now - hit_at).abs() < 0.008 {
                vec![DamageEvent {
                    target: Entity::PLACEHOLDER,
                    amount: 25.0,
                    kind: DamageKind::Slash,
                    point: DVec3::new(-0.1, 1.62, -0.1),
                    impulse: Vec3::new(0.35, 0.0, 1.0).normalize() * 75.0,
                    source: None,
                }]
            } else {
                Vec::new()
            }
        };
        let mut frames = Vec::new();
        Run {
            blows: &blows,
            ..Run::new(1.6, NpcWeapon::sword())
        }
        .look(|now, pose| {
            let feet = [FOOT_L, FOOT_R].map(|p| pose(p).pos);
            let face = pose(Part::Head).rot * Vec3::NEG_Z;
            let chest = pose(Part::Torso).rot * Vec3::NEG_Z;
            frames.push((now, feet, face, chest));
        });
        let calm = frames.iter().rfind(|f| f.0 < hit_at).copied().unwrap();
        let after = |e: f64| {
            frames
                .iter()
                .filter(move |f| (hit_at..hit_at + e).contains(&f.0))
        };
        // The face is wrenched round to the right, away from the blow, the shoulders after it.
        let turned = after(0.2).map(|f| f.2.x - calm.2.x).fold(0.0, f32::max);
        let shoulders = after(0.3).map(|f| f.3.x - calm.3.x).fold(0.0, f32::max);
        assert!(turned > 0.45, "the face turned {turned:.2}");
        assert!(shoulders > 0.15, "the shoulders turned {shoulders:.2}");
        // The rear (right) foot steps back half a pace; the front one shuffles less.
        let back = |k: usize| {
            after(0.7)
                .map(|f| f.1[k].z - calm.1[k].z)
                .fold(0.0, f32::max)
        };
        assert!(back(1) > 0.12, "the rear foot went back {:.2} m", back(1));
        assert!(
            back(0) < back(1),
            "the front foot went back {:.2} m",
            back(0)
        );
        // And they come back up to where they stood.
        let end = frames.last().unwrap();
        for k in 0..2 {
            let off = (end.1[k] - calm.1[k]).length();
            assert!(off < 0.06, "foot {k} ends {off:.2} m from where it stood");
        }
    }

    #[test]
    fn a_called_off_blow_checks_instead_of_snapping() {
        // A light swing telegraphed at 1 s, called off 0.22 s before it would land (as
        // combat calls off a feint).
        let hit_at = 1.0 + f64::from(NpcWeapon::sword().windup);
        let feint = |now: f64| NpcCombat {
            phase: if now < hit_at - 0.22 {
                AttackPhase::Windup {
                    target: None,
                    hit_at,
                    heavy: false,
                    feint: true,
                    aim: DVec3::ZERO,
                }
            } else {
                AttackPhase::Ready
            },
            ..Default::default()
        };
        let path = blade_path(0.9, feint);
        // From the moment it is called off, the blade never jumps (snapped straight back, it
        // would cover the whole way from the telegraph to the guard in one frame).
        let called = ((hit_at - 0.22 - 1.0) * 60.0).ceil() as usize;
        let jump = path[called - 1..]
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .fold(0.0, f32::max);
        let guard = blade_path(0.05, |_| NpcCombat::default());
        let snap = (path[called - 1] - guard[0]).length();
        assert!(
            jump < 0.2 && jump < 0.15 * snap,
            "the blade jumps {jump:.2} m in a frame (of {snap:.2} m back to guard)"
        );
        // Called off, it starts on into the cut before it comes back.
        let ahead = |p: Vec3| -p.z;
        assert!(
            ahead(path[called + 4]) > ahead(path[called - 1]) + 0.05,
            "the tip goes on from {:.2} to {:.2}",
            path[called - 1],
            path[called + 4]
        );
        // And it ends on guard.
        let off = (*path.last().unwrap() - guard[0]).length();
        assert!(off < 0.08, "back on guard but {off:.2} m off");
    }

    #[test]
    fn bow_string_keeps_its_length() {
        for draw in [0.1, 0.3, 0.5] {
            let a = limb_bend(draw);
            assert!(a > 0.0 && a < 1.0);
            let tip = bow_tip(a);
            let nock = weapons::BOW_TIP.z + draw;
            let len = Vec2::new(tip.y, nock - tip.z).length();
            assert!((len - STRING_HALF).abs() < 1e-3);
        }
    }

    #[test]
    fn plates_flash_in_their_teams_hue() {
        // Crimson, green, azure, grey.
        assert_eq!(hue_of(0xb02f35), 1);
        assert_eq!(hue_of(0x3d8a4e), 5);
        assert_eq!(hue_of(0x2f7cf2), 8);
        assert_eq!(hue_of(0x8a8a8a), 0);
        let [red, blue] = [1, 8].map(|hue| plate_flash(hue).base_color);
        assert!(red[0] > 2.0 * red[2] && blue[2] > 2.0 * blue[0]);
        // Every hue's plates are posed with their piece, and no two share a part.
        let mut seen = std::collections::HashSet::new();
        for hue in 0..HUES {
            for &piece in &PAINTED {
                let plates = paint_of(piece, hue).unwrap();
                assert!(seen.insert(plates));
                assert_eq!(slot(plates), slot(piece));
            }
        }
    }

    /// The yaw (rad, to the left positive) a part faces.
    fn yaw_of(rot: Quat) -> f32 {
        let f = rot * Vec3::NEG_Z;
        (-f.x).atan2(-f.z)
    }

    /// How far shut the lids are and open the jaw (0..1), from the parts' poses (each turned
    /// down in the head).
    fn lids_and_jaw(pose: &dyn Fn(Part) -> Placed) -> (f32, f32) {
        let head = pose(Part::Head).rot.inverse();
        let down = |p: Part| {
            let q = head * pose(p).rot;
            -2.0 * q.x.atan2(q.w)
        };
        (down(LIDS) / rig::LIDS_SHUT, down(JAW) / rig::JAW_TURN)
    }

    fn fighter(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    #[test]
    fn the_eyes_lead_the_head_to_a_new_foe_and_the_shoulders_take_what_the_neck_cannot() {
        let (a, b) = (fighter(7), fighter(8));
        let switch = 2.0;
        let brain = |now: f64| NpcBrain {
            alert: AlertLevel::Combat,
            target: Some(if now < switch { a } else { b }),
            ..Default::default()
        };
        // The new foe 1.7 rad round to the left (past what the neck turns).
        let others = |e: Entity| {
            if e == a {
                Some(DVec3::new(0.0, 0.0, -5.0))
            } else if e == b {
                Some((ry(1.7) * Vec3::new(0.0, 0.0, -3.0)).as_dvec3())
            } else {
                None
            }
        };
        let mut frames = Vec::new();
        Run {
            brain: &brain,
            others: &others,
            ..Run::new(2.6, NpcWeapon::sword())
        }
        .look(|now, pose| {
            let head = pose(Part::Head).rot;
            let chest = pose(Part::Torso).rot;
            let neck = (chest.inverse() * head).to_euler(EulerRot::YXZ).0;
            let eyes = yaw_of(pose(EYE_L).rot);
            frames.push((now, eyes, yaw_of(head), yaw_of(chest), neck));
        });
        let at = |t: f64| *frames.iter().find(|f| f.0 >= t).unwrap();
        let before = at(switch - 0.02);
        assert!(
            before.1.abs() < 0.06 && before.2.abs() < 0.06,
            "on the first foe: the eyes at {:.2} rad, the head at {:.2} rad",
            before.1,
            before.2
        );
        // The eyes flick round at once, the head a beat behind them.
        // The eyes flick round at once (as far as they turn in the head), the head a beat
        // behind them.
        let flick = at(switch);
        assert!(
            flick.1 - flick.2 > 0.4 && flick.2 < 0.1,
            "at once, the eyes went {:.2} rad, the head {:.2} rad",
            flick.1,
            flick.2
        );
        let end = at(switch + 1.2);
        let most = frames
            .iter()
            .find(|f| f.2 > 0.9 * end.2)
            .map_or(f64::MAX, |f| f.0 - switch);
        assert!(
            (0.15..0.5).contains(&most),
            "the head most of the way round in {most:.2} s"
        );
        assert!(
            (end.1 - 1.7).abs() < 0.1,
            "the eyes end at {:.2} rad",
            end.1
        );
        assert!(end.2 > 1.2, "the head ends at {:.2} rad", end.2);
        assert!(
            end.3 - before.3 > 0.3,
            "the shoulders turned {:.2} rad",
            end.3 - before.3
        );
        // The neck never twists past its limit, and nothing jumps.
        let neck = frames.iter().map(|f| f.4.abs()).fold(0.0, f32::max);
        assert!(neck < NECK_YAW + 0.05, "the neck twisted {neck:.2} rad");
        let jump = frames
            .windows(2)
            .map(|w| wrap(w[1].2 - w[0].2).abs().max(wrap(w[1].3 - w[0].3).abs()))
            .fold(0.0, f32::max);
        assert!(
            jump < 0.2,
            "the head or the shoulders jumped {jump:.2} rad in a frame"
        );
    }

    #[test]
    fn a_glance_goes_to_a_flanker_and_comes_back_and_a_noise_turns_the_head() {
        let (foe, flanker) = (fighter(7), fighter(9));
        let at_ = 2.0;
        // In a fight: another foe off to the right telegraphs a blow at it.
        let brain = |_: f64| NpcBrain {
            alert: AlertLevel::Combat,
            target: Some(foe),
            ..Default::default()
        };
        let others = |e: Entity| {
            if e == foe {
                Some(DVec3::new(0.0, 0.0, -4.0))
            } else if e == flanker {
                Some((ry(-1.0) * Vec3::new(0.0, 0.0, -2.5)).as_dvec3())
            } else {
                None
            }
        };
        let attacks = |now: f64| {
            if (now - at_).abs() < 0.008 {
                vec![(flanker, Some(Entity::PLACEHOLDER), AttackKind::Melee)]
            } else {
                Vec::new()
            }
        };
        let mut heads = Vec::new();
        Run {
            brain: &brain,
            others: &others,
            attacks: &attacks,
            ..Run::new(2.0, NpcWeapon::sword())
        }
        .look(|now, pose| heads.push((now, yaw_of(pose(Part::Head).rot))));
        let most = |from: f64, to: f64| {
            heads
                .iter()
                .filter(|h| (from..to).contains(&h.0))
                .map(|h| -h.1)
                .fold(f32::MIN, f32::max)
        };
        let glance = most(at_, at_ + 0.6);
        assert!(
            glance > 0.6,
            "the head turned {glance:.2} rad to the flanker"
        );
        let back = heads.iter().rfind(|h| h.0 < at_ + 1.5).unwrap().1;
        assert!(back.abs() < 0.08, "and back to the foe ({back:.2} rad)");

        // At ease, a noise off to the left turns the head to it a moment.
        let heard = 2.0;
        let quiet = |_: f64| NpcBrain::default();
        let noise = |now: f64| {
            (now >= heard).then(|| NpcMemory {
                stimulus: Some(crate::perception::Stimulus {
                    position: (ry(1.0) * Vec3::new(0.0, 0.0, -6.0)).as_dvec3(),
                    time: heard,
                    strength: 1.0,
                }),
                ..Default::default()
            })
        };
        let heads = |memory: &dyn Fn(f64) -> Option<NpcMemory>| {
            let mut heads = Vec::new();
            Run {
                brain: &quiet,
                memory,
                ..Run::new(3.5, NpcWeapon::sword())
            }
            .look(|now, pose| heads.push((now, yaw_of(pose(Part::Head).rot))));
            heads
        };
        let (heard_it, calm) = (heads(&noise), heads(&|_| None));
        let turned = heard_it
            .iter()
            .zip(&calm)
            .filter(|(h, _)| (heard..heard + 0.9).contains(&h.0))
            .map(|(h, c)| h.1 - c.1)
            .fold(f32::MIN, f32::max);
        assert!(turned > 0.6, "the head turned {turned:.2} rad to the noise");
        let (h, c) = (heard_it.last().unwrap(), calm.last().unwrap());
        assert!((h.1 - c.1).abs() < 0.05, "and back ({:.2} rad)", h.1 - c.1);

        // Searching, the head sweeps the ground ahead from side to side.
        let searching = |_: f64| NpcBrain {
            alert: AlertLevel::Searching,
            ..Default::default()
        };
        let mut sweep = (f32::MAX, f32::MIN);
        Run {
            brain: &searching,
            ..Run::new(6.0, NpcWeapon::sword_and_shield())
        }
        .look(|_, pose| {
            let y = yaw_of(pose(Part::Head).rot);
            sweep = (sweep.0.min(y), sweep.1.max(y));
        });
        assert!(
            sweep.0 < -0.5 && sweep.1 > 0.5,
            "searching, the head swept from {:.2} to {:.2} rad",
            sweep.0,
            sweep.1
        );
    }

    #[test]
    fn blinks_come_quicker_when_alarmed() {
        let blinks = |alert: AlertLevel| {
            let brain = move |_: f64| NpcBrain {
                alert,
                ..Default::default()
            };
            let mut lids = Vec::new();
            Run {
                brain: &brain,
                ..Run::new(40.0, NpcWeapon::sword())
            }
            .look(|_, pose| lids.push(lids_and_jaw(pose).0));
            // Each blink shuts the lids and opens them again within a fifth of a second.
            let mut n = 0;
            for (f, w) in lids.windows(2).enumerate() {
                if w[0] < 0.8 && w[1] >= 0.8 {
                    n += 1;
                    let open = lids[f + 1..].iter().take(12).any(|&l| l < 0.2);
                    assert!(open, "{alert:?}: the lids stay shut after a blink");
                }
            }
            n
        };
        let (calm, alarmed) = (blinks(AlertLevel::Relaxed), blinks(AlertLevel::Combat));
        assert!(
            (6..=16).contains(&calm),
            "at ease it blinked {calm} times in 40 s"
        );
        assert!(
            alarmed as f32 >= 1.5 * calm as f32,
            "in a fight it blinked {alarmed} times in 40 s (at ease {calm})"
        );
    }

    #[test]
    fn the_face_shows_effort_and_pain() {
        let faces = |run: Run| {
            let mut out = Vec::new();
            run.look(|now, pose| {
                let (lids, jaw) = lids_and_jaw(pose);
                out.push((now, lids, jaw));
            });
            out
        };
        let range = |f: &[(f64, f32, f32)], from: f64, to: f64, k: usize| {
            f.iter()
                .filter(|x| (from..to).contains(&x.0))
                .map(|x| if k == 1 { x.1 } else { x.2 })
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
        };
        // Breathing hard through the mouth after a sprint, and badly hurt; not at rest.
        let sprint = |now: f64| {
            if (2.0..7.0).contains(&now) {
                Vec3::new(0.0, 0.0, -6.5)
            } else {
                Vec3::ZERO
            }
        };
        let f = faces(Run {
            vel: &sprint,
            ..Run::new(7.0, NpcWeapon::sword())
        });
        let (_, rest) = range(&f, 1.0, 2.0, 2);
        assert!(rest < 0.01, "fresh, the jaw opens {rest:.2}");
        let (gasp, _) = range(&f, 7.1, 8.0, 2);
        assert!(
            gasp > 0.15,
            "after a sprint, the jaw opens at least {gasp:.2}"
        );
        let hurt = |_: f64| {
            Some(NpcVitals {
                health: 20.0,
                ..Default::default()
            })
        };
        let f = faces(Run {
            vitals: &hurt,
            ..Run::new(3.0, NpcWeapon::sword())
        });
        let (gasp, _) = range(&f, 1.0, 4.0, 2);
        assert!(gasp > 0.15, "badly hurt, the jaw opens at least {gasp:.2}");

        // Struck: the eyes squeezed shut and the teeth bared.
        let blow = |now: f64| {
            if (now - 2.0).abs() < 0.008 {
                vec![DamageEvent {
                    target: Entity::PLACEHOLDER,
                    amount: 25.0,
                    kind: DamageKind::Slash,
                    point: DVec3::new(-0.1, 1.1, -0.15),
                    impulse: Vec3::Z * 75.0,
                    source: None,
                }]
            } else {
                Vec::new()
            }
        };
        let f = faces(Run {
            blows: &blow,
            ..Run::new(2.0, NpcWeapon::sword())
        });
        let (_, squeeze) = range(&f, 2.0, 2.3, 1);
        let (_, bared) = range(&f, 2.0, 2.3, 2);
        assert!(
            squeeze > 0.6 && bared > 0.2,
            "struck, the lids shut {squeeze:.2} and the jaw opens {bared:.2}"
        );

        // A heavy blow: the jaw set and the eyes narrowed through the telegraph, a shout as it
        // goes in.
        let hit_at = 2.8;
        let heavy = |now: f64| NpcCombat {
            phase: if now < hit_at {
                AttackPhase::Windup {
                    target: None,
                    hit_at,
                    heavy: true,
                    feint: false,
                    aim: DVec3::ZERO,
                }
            } else {
                AttackPhase::Recovery {
                    until: hit_at + 0.6,
                }
            },
            ..Default::default()
        };
        let f = faces(Run {
            combat: &heavy,
            ..Run::new(2.5, NpcWeapon::sword())
        });
        let (narrow, _) = range(&f, 2.4, 2.75, 1);
        let (_, set) = range(&f, 2.4, 2.75, 2);
        let (_, shout) = range(&f, hit_at, hit_at + 0.2, 2);
        assert!(
            narrow > 0.3 && set < 0.02 && shout > 0.6,
            "heavy: the lids {narrow:.2} and the jaw {set:.2} in the telegraph, the jaw \
             {shout:.2} as it goes in"
        );

        // Downed: the eyes shut and the jaw slack.
        let down = |now: f64| {
            Some(NpcVitals {
                downed: now >= 2.0,
                ..Default::default()
            })
        };
        let f = faces(Run {
            vitals: &down,
            ..Run::new(2.5, NpcWeapon::sword())
        });
        let (shut, _) = range(&f, 3.0, 3.5, 1);
        let (slack, open) = range(&f, 3.0, 3.5, 2);
        assert!(
            shut > 0.95 && slack > 0.2 && open < 0.4,
            "downed: the lids {shut:.2}, the jaw {slack:.2}..{open:.2}"
        );
    }

    #[test]
    fn the_badly_hurt_limp_on_the_leg_last_struck() {
        let walk = |_: f32| (Vec2::new(0.0, -1.4), 0.0);
        let whole = steps_hurt(Kind::Longsword, false, None, 5.0, walk);
        for (bad, side) in [(0, -1.0), (1, 1.0)] {
            let lame = steps_hurt(
                Kind::Longsword,
                false,
                Some((Zone::Legs, side, 1.0)),
                5.0,
                walk,
            );
            let good = 1 - bad;
            let case = format!("lame in leg {bad}: {lame:?} (whole {whole:?})");
            // Shorter steps (so more of them), lifted less, the hips sagging as the bad leg
            // takes the weight; and the feet still don't slide.
            let (n, n0) = (
                lame.lifts[0] + lame.lifts[1],
                whole.lifts[0] + whole.lifts[1],
            );
            assert!(
                n as f32 >= 1.15 * n0 as f32,
                "{case}: {n} steps ({n0} whole)"
            );
            assert!(
                lame.high[bad] < 0.9 * whole.high[bad],
                "{case}: the bad foot lifted {:.3} m",
                lame.high[bad]
            );
            assert!(
                lame.dip[bad] < lame.dip[good] - 0.015,
                "{case}: the hips down to {:.3} m on the bad leg, {:.3} m on the good one",
                lame.dip[bad],
                lame.dip[good]
            );
            assert!(
                lame.slid <= 0.01 * lame.went + 0.01 && lame.worst < 0.005,
                "{case}: the feet slid"
            );
        }
        // Whole, the steps are even.
        assert!(
            (whole.high[0] - whole.high[1]).abs() < 0.2 * whole.high[0]
                && (whole.dip[0] - whole.dip[1]).abs() < 0.01,
            "whole: {whole:?}"
        );
    }

    #[test]
    fn the_badly_hurt_hunch_and_press_a_hand_to_the_wound_at_ease() {
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            let pose = |act: Act, wound| {
                let mut i = input(kind, act, act != Act::Relaxed);
                i.wound = wound;
                solve(&i)
            };
            let calm = pose(Act::Relaxed, None);
            // Sword and shield have no hand free at ease.
            let free = kind != Kind::SwordShield;
            for side in [-1.0, 1.0] {
                let case = format!("{kind:?}, side {side}");
                // A hand on the front of the thigh struck.
                let s = pose(Act::Relaxed, Some((Zone::Legs, side, 1.0)));
                let thigh = s.thigh[usize::from(side > 0.0)];
                let on = |s: &Skeleton| {
                    (0..2)
                        .map(|k| {
                            let p = thigh.rot.inverse() * (s.hand[k].pos - thigh.pos);
                            // In front of the thigh (or its top), on it.
                            if p.z < -0.05 && p.y < 0.12 && p.y > -0.4 {
                                Vec2::new(p.x, p.z + 0.12).length()
                            } else {
                                f32::MAX
                            }
                        })
                        .fold(f32::MAX, f32::min)
                };
                // A hand went there to press it.
                let moved = |s: &Skeleton| {
                    (0..2)
                        .map(|k| (s.hand[k].pos - calm.hand[k].pos).length())
                        .fold(0.0, f32::max)
                };
                if free {
                    assert!(
                        on(&s) < 0.08,
                        "{case}: no hand on the thigh ({:.2})",
                        on(&s)
                    );
                    assert!(moved(&s) > 0.1, "{case}: a hand moved {:.2} m", moved(&s));
                }
                // Struck in the body: hunched over it, low, a hand on the flank.
                let s = pose(Act::Relaxed, Some((Zone::Body, side, 1.0)));
                let flank = |s: &Skeleton| {
                    let at = s.chest.at(v3(side * 0.12, 0.1, -0.11));
                    (0..2)
                        .map(|k| (s.hand[k].pos - at).length())
                        .fold(f32::MAX, f32::min)
                };
                if free {
                    assert!(
                        flank(&s) < 0.1,
                        "{case}: a hand {:.2} m off the wound",
                        flank(&s)
                    );
                    assert!(moved(&s) > 0.1, "{case}: a hand moved {:.2} m", moved(&s));
                }
                let bow = |s: &Skeleton| (s.chest.rot * Vec3::NEG_Z).y;
                assert!(
                    s.head.pos.y < calm.head.pos.y - 0.03 && bow(&s) < bow(&calm) - 0.1,
                    "{case}: hunched, the head {:.2} m down, the chest {:.2} over",
                    calm.head.pos.y - s.head.pos.y,
                    bow(&calm) - bow(&s)
                );
                // On guard, the hands are on the weapon (and shield), not the wound.
                let guard = pose(Act::Guard, Some((Zone::Body, side, 1.0)));
                let whole = pose(Act::Guard, None);
                for k in 0..2 {
                    let off = (guard.hand[k].pos - whole.hand[k].pos).length();
                    assert!(off < 0.2, "{case}: on guard, hand {k} moved {off:.2} m");
                }
            }
        }
    }

    #[test]
    fn the_revived_get_up_from_where_they_lay_onto_guard() {
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            for (yaw, bent, flung) in [(0.0, 0, 1), (0.6, 1, 0), (-0.8, 0, 0)] {
                let case = format!("{kind:?} fallen along {yaw}");
                let fall = Fall {
                    t: 3.0,
                    yaw,
                    bent,
                    flung,
                };
                let was = input(kind, Act::Guard, true);
                let lying = solve(&Input {
                    fall: Some(fall),
                    ..was
                });
                let pose = |t: f32| {
                    solve(&Input {
                        rise: Some(Rise {
                            t,
                            fall,
                            was: Act::Guard,
                            since: was.now,
                            feet: was.feet,
                            knelt: None,
                        }),
                        ..input(kind, Act::Guard, true)
                    })
                };
                let joints = |s: &Skeleton| {
                    let mut v = vec![s.pelvis.pos, s.chest.pos, s.head.pos, s.weapon.pos];
                    for k in 0..2 {
                        v.extend([s.shin[k].pos, s.foot[k].pos, s.fore[k].pos, s.hand[k].pos]);
                    }
                    v
                };
                let off = |a: &Skeleton, b: &Skeleton| {
                    joints(a)
                        .iter()
                        .zip(joints(b))
                        .map(|(p, q)| (*p - q).length())
                        .fold(0.0, f32::max)
                };
                // It starts as it lay, and ends on guard.
                let start = off(&pose(0.0), &lying);
                assert!(start < 1e-3, "{case}: a pop of {start:.3} m as it starts");
                let end = off(&pose(rise_secs(kind)), &solve(&was));
                assert!(end < 1e-3, "{case}: ends {end:.3} m off its guard");
                let mut last = pose(0.0);
                let mut taken = false;
                let n = (rise_secs(kind) * 60.0).ceil() as usize;
                for f in 1..=n {
                    let t = f as f32 / 60.0;
                    let s = pose(t);
                    for k in 0..2 {
                        let len = |a: Bone, b: Bone| (b.pos - a.pos).length();
                        assert!((len(s.thigh[k], s.shin[k]) - rig::THIGH).abs() < 1e-3);
                        assert!((len(s.shin[k], s.foot[k]) - rig::SHIN).abs() < 1e-3);
                        assert!((len(s.upper[k], s.fore[k]) - rig::UPPER_ARM).abs() < 1e-3);
                        assert!((len(s.fore[k], s.hand[k]) - rig::FOREARM).abs() < 1e-3);
                        let miss = (s.want[k] - s.hand[k].pos).length();
                        assert!(
                            miss < 0.03,
                            "{case}: hand {k} misses by {miss:.2} m at {t:.2} s"
                        );
                    }
                    for (n, p) in joints(&s).into_iter().enumerate() {
                        assert!(
                            p.y > -0.02,
                            "{case}: joint {n} in the ground at {t:.2} s ({p:.2})"
                        );
                    }
                    let jump = off(&s, &last);
                    assert!(
                        jump < 0.1,
                        "{case}: a joint jumps {jump:.2} m at {t:.2} s: {:.2?} from {:.2?}",
                        joints(&s),
                        joints(&last)
                    );
                    // The weapon lies where it fell until the hand takes it up.
                    let moved = (s.weapon.pos - lying.weapon.pos).length();
                    if !taken && moved > 1e-4 {
                        taken = true;
                        assert!(
                            moved < 0.03,
                            "{case}: the weapon leaps {moved:.2} m to hand"
                        );
                    }
                    last = s;
                }
                // Halfway, it is on one knee, the weapon in hand.
                let knee = pose(RISE_KNEEL);
                let low = knee.shin[0].pos.y.min(knee.shin[1].pos.y);
                assert!(
                    low < 0.12 && knee.pelvis.pos.y > 0.4 && knee.weapon.pos.y > 0.2,
                    "{case}: on the knee at {low:.2} m, the hips at {:.2} m, the weapon at {:.2} m",
                    knee.pelvis.pos.y,
                    knee.weapon.pos.y
                );
            }
        }
    }

    #[test]
    fn a_revived_fighter_gets_up_without_a_pop_and_lowers_its_weapon() {
        let parts = [
            PELVIS,
            Part::Torso,
            Part::Head,
            Part::LegL,
            Part::LegR,
            SHIN_L,
            SHIN_R,
            FOOT_L,
            FOOT_R,
            FOREARM_L,
            FOREARM_R,
            HAND_L,
            HAND_R,
            Part::Weapon,
        ];
        let (down, up) = (1.5, 4.0);
        for weapon in [
            NpcWeapon::sword(),
            NpcWeapon::sword_and_shield(),
            NpcWeapon::bow(),
        ] {
            // Revived, at ease (as revived fighters are): it gets up onto guard, then lowers
            // its weapon.
            let vitals = |now: f64| {
                Some(NpcVitals {
                    downed: (down..up).contains(&now),
                    ..Default::default()
                })
            };
            let brain = |now: f64| NpcBrain {
                alert: if now < up {
                    AlertLevel::Combat
                } else {
                    AlertLevel::Relaxed
                },
                ..Default::default()
            };
            let mut frames: Vec<(f64, Vec<Vec3>)> = Vec::new();
            Run {
                vitals: &vitals,
                brain: &brain,
                ..Run::new(6.0, weapon)
            }
            .look(|now, pose| frames.push((now, parts.map(|p| pose(p).pos).to_vec())));
            // The most a part moves in a frame, which, and when.
            let worst = |from: f64, to: f64| {
                let mut most = (0.0f32, 0, 0.0);
                for w in frames.windows(2).filter(|w| (from..to).contains(&w[1].0)) {
                    for (n, (a, b)) in w[0].1.iter().zip(&w[1].1).enumerate() {
                        if (*a - *b).length() > most.0 {
                            most = ((*a - *b).length(), n, w[1].0);
                        }
                    }
                }
                most
            };
            let case = format!("{:?}", Kind::of(&weapon));
            let revived = worst(up - 0.001, up + 0.03);
            assert!(
                revived.0 < 0.03,
                "{case}: a part pops as it is revived: {revived:.2?}"
            );
            let rising = worst(up, 7.0);
            assert!(
                rising.0 < 0.1,
                "{case}: a part jumps getting up: {rising:.2?}"
            );
            // Up on its feet again.
            let pelvis = frames.last().unwrap().1[0];
            assert!(pelvis.y > 0.9, "{case}: the hips end at {:.2} m", pelvis.y);
        }
    }

    #[test]
    fn at_ease_the_weapon_is_carried_and_a_foe_brings_it_up_to_guard() {
        let (alarm, calm) = (2.0, 4.5);
        for weapon in [
            NpcWeapon::sword(),
            NpcWeapon::sword_and_shield(),
            NpcWeapon::rifle(),
            NpcWeapon::bow(),
        ] {
            let kind = Kind::of(&weapon);
            let brain = |now: f64| NpcBrain {
                alert: if (alarm..calm).contains(&now) {
                    AlertLevel::Combat
                } else {
                    AlertLevel::Relaxed
                },
                ..Default::default()
            };
            let mut frames = Vec::new();
            let mut arms = Vec::new();
            Run {
                brain: &brain,
                ..Run::new(6.0, weapon)
            }
            .look(|now, pose| {
                frames.push((now, pose(Part::Weapon), pose(Part::Torso)));
                arms.push((
                    now,
                    [FOREARM_L, FOREARM_R, HAND_L, HAND_R].map(|p| pose(p).pos),
                ));
            });
            let mut guard = Vec::new();
            Run::new(6.0, weapon).look(|_, pose| guard.push(pose(Part::Weapon)));
            let at = |t: f64| frames.iter().position(|f| f.0 >= t).unwrap();
            let case = format!("{kind:?}");
            // Carried at ease: the longsword on the right shoulder, the blade up behind it;
            // the arming sword and the carbine hanging, point and muzzle down; the bow low.
            let (_, w, chest) = frames[at(alarm - 0.02)];
            let local = |p: Vec3| chest.rot.inverse() * (p - chest.pos);
            match kind {
                Kind::Longsword => {
                    let tip = local(w.pos + w.rot * weapons::LONGSWORD_TIP);
                    let grip = local(w.pos);
                    let shoulder = shoulder(1);
                    // The blade passes over the right shoulder, the point up and behind.
                    let d = (tip - grip).normalize();
                    let along = (shoulder - grip).dot(d).max(0.0);
                    let near = (grip + d * along - shoulder).length();
                    assert!(
                        tip.y > shoulder.y + 0.2 && tip.z > shoulder.z && near < 0.15,
                        "{case}: the blade {near:.2} m off the shoulder, the tip at {tip:.2}"
                    );
                }
                Kind::SwordShield => {
                    let tip = w.pos + w.rot * weapons::ARMING_TIP;
                    assert!(
                        tip.y < w.pos.y - 0.6 && tip.y > 0.1,
                        "{case}: the point at {:.2} m (the hilt at {:.2} m)",
                        tip.y,
                        w.pos.y
                    );
                }
                Kind::Rifle => {
                    let muzzle = w.pos + w.rot * weapons::RIFLE_MUZZLE;
                    assert!(
                        muzzle.y < w.pos.y - 0.3,
                        "{case}: the muzzle at {muzzle:.2}"
                    );
                }
                _ => assert!(w.pos.y < 1.0, "{case}: the bow at {:.2} m", w.pos.y),
            }
            // Brought up to guard without a jump, there within a second.
            let mut worst = (0.0f32, 0.0f32);
            for f in at(alarm)..frames.len() {
                let (a, b) = (frames[f - 1].1, frames[f].1);
                worst.0 = worst.0.max((b.pos - a.pos).length());
                worst.1 = worst.1.max(a.rot.angle_between(b.rot));
            }
            assert!(
                worst.0 < 0.08 && worst.1 < 0.3,
                "{case}: the weapon jumps {:.2} m, {:.2} rad in a frame",
                worst.0,
                worst.1
            );
            // Nor do the arms, up or down (which part, and when).
            let jump = arms
                .windows(2)
                .flat_map(|w| (0..4).map(move |k| ((w[1].1[k] - w[0].1[k]).length(), k, w[1].0)))
                .fold((0.0, 0, 0.0), |a, b| if b.0 > a.0 { b } else { a });
            assert!(jump.0 < 0.1, "{case}: an arm jumps in a frame: {jump:.2?}");
            let f = at(alarm + f64::from(readying(kind).0) + 0.1);
            let (w, g) = (frames[f].1, guard[f]);
            let (off, turn) = ((w.pos - g.pos).length(), w.rot.angle_between(g.rot));
            assert!(
                off < 0.02 && turn < 0.05,
                "{case}: brought up, the weapon is {off:.2} m, {turn:.2} rad off the guard"
            );
            // And lowered again once the fight is over (to where it was carried on the body).
            let carried =
                |(_, w, chest): (f64, Placed, Placed)| chest.rot.inverse() * (w.pos - chest.pos);
            let (a, b) = (frames[at(alarm - 0.02)], *frames.last().unwrap());
            let off = (carried(a) - carried(b)).length();
            assert!(off < 0.05, "{case}: carried {off:.2} m off where it was");
        }
    }

    /// Every joint of a skeleton, for the checks below: the trunk and the head, the knees, the
    /// ankles, the elbows and the wrists, and the weapon.
    fn tend_joints(s: &Skeleton) -> Vec<Vec3> {
        let mut v = vec![s.pelvis.pos, s.chest.pos, s.head.pos, s.weapon.pos];
        for k in 0..2 {
            v.extend([s.shin[k].pos, s.foot[k].pos, s.fore[k].pos, s.hand[k].pos]);
        }
        v
    }

    #[test]
    fn a_reviver_kneels_sets_its_weapon_down_and_puts_its_hands_on_the_chest() {
        let kinds = [
            Kind::Longsword,
            Kind::SwordShield,
            Kind::Rifle,
            Kind::Bow,
            Kind::Fists,
        ];
        // The mate's chest ahead, the knees going down short of it (beside the arm along its
        // body, beside its belly); ahead and to one side; and further off to the other, the
        // knees a step off.
        let chests = [
            (v3(0.0, 0.28, -0.62), v3(0.0, 0.0, -0.18)),
            (v3(0.3, 0.3, -0.55), v3(0.05, 0.0, -0.25)),
            (v3(-0.5, 0.26, -0.8), v3(-0.3, 0.0, -0.45)),
        ];
        for kind in kinds {
            for (n, (at, knees)) in chests.into_iter().enumerate() {
                for fighting in [true, false] {
                    let act = if fighting { Act::Guard } else { Act::Relaxed };
                    let case = format!("{kind:?} chest {n} fighting {fighting}");
                    let base = input(kind, act, fighting);
                    let pose = |k: f32, now: f64| {
                        solve(&Input {
                            now,
                            tend: Some(Tend { k, at, knees }),
                            ..base
                        })
                    };
                    let stood = solve(&base);
                    let off = |a: &Skeleton, b: &Skeleton| {
                        tend_joints(a)
                            .iter()
                            .zip(tend_joints(b))
                            .map(|(p, q)| (*p - q).length())
                            .fold(0.0, f32::max)
                    };
                    // It starts from where it stood (no pop going down, nor coming up), and goes
                    // down (or, quicker, up again) at 60 Hz without a jump.
                    let start = off(&pose(1e-4, base.now), &stood);
                    assert!(start < 0.01, "{case}: a pop of {start:.3} m as it starts");
                    let n = (TEND_DOWN.min(TEND_UP) * 60.0).ceil() as usize;
                    let mut last = stood;
                    for f in 0..=n {
                        let k = f as f32 / n as f32;
                        let now = base.now + f64::from(k * TEND_DOWN);
                        let s = pose(k, now);
                        for j in 0..2 {
                            let len = |a: Bone, b: Bone| (b.pos - a.pos).length();
                            assert!((len(s.thigh[j], s.shin[j]) - rig::THIGH).abs() < 1e-3);
                            assert!((len(s.shin[j], s.foot[j]) - rig::SHIN).abs() < 1e-3);
                            assert!((len(s.upper[j], s.fore[j]) - rig::UPPER_ARM).abs() < 1e-3);
                            assert!((len(s.fore[j], s.hand[j]) - rig::FOREARM).abs() < 1e-3);
                            let miss = (s.want[j] - s.hand[j].pos).length();
                            assert!(
                                miss < 0.03,
                                "{case}: hand {j} misses by {miss:.3} m at {k:.2}"
                            );
                        }
                        for (i, p) in tend_joints(&s).into_iter().enumerate() {
                            assert!(
                                p.y > -0.02,
                                "{case}: joint {i} in the ground at {k:.2} ({p:.2})"
                            );
                        }
                        let jump = off(&s, &last);
                        assert!(
                            jump < 0.1,
                            "{case}: a joint jumps {jump:.2} m at {k:.2}: {:.2?} from {:.2?}",
                            tend_joints(&s),
                            tend_joints(&last)
                        );
                        last = s;
                    }
                    // Down on both knees beside the chest, the head bowed over it.
                    let s = pose(1.0, base.now);
                    for j in 0..2 {
                        assert!(
                            s.shin[j].pos.y < 0.12,
                            "{case}: knee {j} at {:.2} m",
                            s.shin[j].pos.y
                        );
                    }
                    assert!(
                        s.pelvis.pos.y > 0.35,
                        "{case}: the hips at {:.2} m",
                        s.pelvis.pos.y
                    );
                    let gaze = s.head.rot * Vec3::NEG_Z;
                    assert!(gaze.y < -0.5, "{case}: the head looks along {gaze:.2}");
                    // The hands on the chest (one alone, the other holding a bow or a shield
                    // aside).
                    let on: &[usize] = match kind {
                        Kind::Bow | Kind::SwordShield => &[1],
                        _ => &[0, 1],
                    };
                    for &j in on {
                        let d = (s.hand[j].pos - at).length();
                        assert!(d < 0.16, "{case}: hand {j} is {d:.2} m from the chest");
                    }
                    // The weapon set down on the ground beside the knee, clear of the hand and
                    // of the mate; a bow or a shield held aside, a bow's foot on the ground.
                    if matches!(kind, Kind::Longsword | Kind::Rifle | Kind::SwordShield) {
                        // In hand all the way down to the ground (it doesn't leap there).
                        let held = relative(stood.hand[1], stood.weapon);
                        let laid = pose(0.7, base.now);
                        let gap = (laid.hand[1].then(held).pos - laid.weapon.pos).length();
                        assert!(gap < 0.03, "{case}: set down {gap:.3} m from the hand");
                        let w = s.weapon.pos;
                        assert!(w.y < 0.06, "{case}: the weapon at {w:.2}");
                        let gap = (w - s.hand[1].pos).length();
                        assert!(gap > 0.25, "{case}: the weapon {gap:.2} m from the hand");
                        let tip = s.weapon.at(match kind {
                            Kind::Rifle => weapons::RIFLE_MUZZLE,
                            Kind::Longsword => weapons::LONGSWORD_TIP,
                            _ => weapons::ARMING_TIP,
                        });
                        let to_mate = (tip - at).length();
                        assert!(
                            tip.y < 0.15 && to_mate > 0.6,
                            "{case}: the point at {tip:.2}, {to_mate:.2} m from the chest"
                        );
                    }
                    if kind == Kind::Bow {
                        let foot = s
                            .weapon
                            .at(v3(0.0, -weapons::BOW_TIP.y, weapons::BOW_TIP.z));
                        assert!(foot.y < 0.08, "{case}: the bow's foot at {foot:.2}");
                    }
                    if kind == Kind::SwordShield {
                        let y = s.shield.pos.y;
                        assert!(y > 0.3 && y < 0.6, "{case}: the shield at {y:.2} m");
                    }
                    // The hands press on the chest and let up again.
                    let (lo, hi) = (0..40)
                        .map(|f| pose(1.0, base.now + f64::from(f) / 40.0 / f64::from(PRESS_HZ)))
                        .map(|s| s.hand[1].pos.y)
                        .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
                    let depth = hi - lo;
                    assert!(
                        (depth - PRESS).abs() < 0.01,
                        "{case}: the hands press {depth:.3} m"
                    );
                }
            }
        }
    }

    #[test]
    fn a_reviver_glances_up_at_the_threat_and_gets_up_with_its_weapon_after() {
        // Kneeling beside a downed mate from 1.5 s until 5 s, its foe off to the right.
        let (from, until) = (1.5, 5.0);
        let foe = fighter(9);
        let foe_at = DVec3::new(6.0, 0.0, -4.0);
        let chest = DVec3::new(0.1, 0.28, -0.62);
        let tending = |now: f64| (from..until).contains(&now);
        let brain = |now: f64| NpcBrain {
            alert: AlertLevel::Combat,
            target: Some(foe),
            action: if tending(now) {
                NpcAction::Revive
            } else {
                NpcAction::Idle
            },
            revive_kneel: if tending(now) {
                (now - from) as f32 + 0.01
            } else {
                0.0
            },
            ..Default::default()
        };
        let others = |e: Entity| (e == foe).then_some(foe_at);
        let knees = DVec3::new(0.05, 0.0, -0.22);
        let patient = |now: f64| tending(now).then_some((chest, knees));
        for weapon in [
            NpcWeapon::sword(),
            NpcWeapon::sword_and_shield(),
            NpcWeapon::rifle(),
            NpcWeapon::bow(),
        ] {
            let kind = Kind::of(&weapon);
            let case = format!("{kind:?}");
            let parts = [
                PELVIS,
                Part::Torso,
                Part::Head,
                SHIN_L,
                SHIN_R,
                FOOT_L,
                FOOT_R,
                FOREARM_L,
                FOREARM_R,
                HAND_L,
                HAND_R,
                Part::Weapon,
            ];
            let mut frames: Vec<(f64, Vec<Placed>)> = Vec::new();
            Run {
                brain: &brain,
                others: &others,
                patient: &patient,
                ..Run::new(6.5, weapon)
            }
            .look(|now, pose| frames.push((now, parts.map(&pose).to_vec())));
            let at = |t: f64| &frames.iter().find(|f| f.0 >= t).unwrap().1;
            // No part jumps in a frame, down and up again.
            let jump = frames
                .windows(2)
                .flat_map(|w| {
                    (0..parts.len())
                        .map(move |n| ((w[1].1[n].pos - w[0].1[n].pos).length(), n, w[1].0))
                })
                .fold((0.0f32, 0, 0.0), |a, b| if b.0 > a.0 { b } else { a });
            assert!(
                jump.0 < 0.1,
                "{case}: a part jumps {:.2} m: {jump:?}",
                jump.0
            );
            // Down beside the mate once it kneels there, the hands on its chest.
            let kneeling = at(from + f64::from(TEND_DOWN) + 0.1);
            assert!(
                kneeling[0].pos.y < 0.55 && kneeling[3].pos.y < 0.12,
                "{case}: the hips at {:.2} m, the knee at {:.2} m",
                kneeling[0].pos.y,
                kneeling[3].pos.y
            );
            let hand = kneeling[10].pos.as_dvec3();
            assert!(
                hand.distance(chest) < 0.16,
                "{case}: the hand {:.2} m from the chest",
                hand.distance(chest)
            );
            // Bowed over it, looking up at the foe now and then (the head turned its way).
            let (mut down, mut up, mut glances, mut was_up) = (0, 0, 0, false);
            for (now, f) in frames
                .iter()
                .filter(|f| (from + f64::from(TEND_DOWN) + 0.1..until).contains(&f.0))
            {
                let look = f[2].rot * Vec3::NEG_Z;
                let is_up = look.y > -0.3;
                if is_up {
                    up += 1;
                    let toward = (foe_at - f[2].pos.as_dvec3()).as_vec3();
                    let flat = |v: Vec3| Vec2::new(v.x, v.z).normalize();
                    let along = flat(look).dot(flat(toward));
                    assert!(
                        along > 0.7,
                        "{case}: looked up {along:.2} off the foe at {now:.2}"
                    );
                } else if look.y < -0.5 {
                    down += 1;
                }
                if is_up && !was_up {
                    glances += 1;
                }
                was_up = is_up;
            }
            assert!(glances >= 1, "{case}: never looked up");
            assert!(down > 2 * up, "{case}: down {down} frames, up {up}");
            // Up again after, the weapon back in hand.
            let after = at(until + f64::from(TEND_UP) + 0.3);
            assert!(
                after[0].pos.y > 0.8,
                "{case}: the hips at {:.2} m",
                after[0].pos.y
            );
            let stood = at(from - 0.05);
            let hold = |f: &[Placed]| {
                let h = if kind == Kind::Bow { f[9] } else { f[10] };
                h.rot.inverse() * (f[11].pos - h.pos)
            };
            let off = (hold(after) - hold(stood)).length();
            assert!(
                off < 0.03,
                "{case}: the weapon {off:.2} m off the hand after"
            );
        }
    }

    /// Balls (centre, radius) standing in for a shape.
    type Balls = Vec<(Vec3, f32)>;

    /// Balls along `a`..`b`, their radius going from `ra` to `rb`: a limb, a blade or a stock
    /// as the clearance checks see it.
    fn capsule(a: Vec3, b: Vec3, ra: f32, rb: f32) -> Vec<(Vec3, f32)> {
        let n = ((b - a).length() / 0.02).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                (a.lerp(b, t), lerp(ra, rb, t))
            })
            .collect()
    }

    /// How far apart two shapes made of balls are (m, surface to surface; below 0 where one
    /// cuts into the other).
    fn clearance(a: &[(Vec3, f32)], b: &[(Vec3, f32)]) -> f32 {
        a.iter()
            .flat_map(|&(p, r)| b.iter().map(move |&(q, s)| p.distance(q) - r - s))
            .fold(f32::MAX, f32::min)
    }

    /// What a fighter of `kind` posed as `s` holds, has set down or has let go of, as balls:
    /// a blade with its cross-guard, a carbine, a bow (grip and limbs), a shield (its face).
    fn gear_balls(kind: Kind, s: &Skeleton) -> Vec<(Vec3, f32)> {
        let w = s.weapon;
        let blade = |tip: Vec3, pommel: f32, guard: f32| {
            let mut v = capsule(w.at(v3(0.0, 0.0, pommel)), w.at(tip), 0.025, 0.01);
            let (a, b) = (w.at(v3(0.0, -guard, -0.02)), w.at(v3(0.0, guard, -0.02)));
            v.extend(capsule(a, b, 0.015, 0.015));
            v
        };
        let mut v = match kind {
            Kind::Rifle => capsule(
                w.at(weapons::RIFLE_BUTT),
                w.at(weapons::RIFLE_MUZZLE),
                0.04,
                0.025,
            ),
            Kind::Longsword => blade(weapons::LONGSWORD_TIP, 0.25, 0.12),
            Kind::SwordShield => blade(weapons::ARMING_TIP, 0.12, 0.1),
            Kind::Bow => {
                let tip = weapons::BOW_TIP;
                let mut v = capsule(w.pos, w.at(tip), 0.025, 0.015);
                v.extend(capsule(w.pos, w.at(v3(0.0, -tip.y, tip.z)), 0.025, 0.015));
                v
            }
            Kind::Fists => Vec::new(),
        };
        if kind == Kind::SwordShield {
            // The heater's face (curved round from side to side), from its point to its top.
            let (w, round) = (0.28, (0.28 * 0.28 + 0.42 * 0.42) / (2.0 * 0.28));
            for i in 0..=12 {
                for j in 0..=16 {
                    let (x, y) = (
                        -w + 2.0 * w * i as f32 / 12.0,
                        -0.42 + 0.75 * j as f32 / 16.0,
                    );
                    let side = if y < 0.0 {
                        Vec2::new(x.abs() + round - w, y).length() - round
                    } else {
                        x.abs() - w
                    };
                    if side <= 0.0 {
                        let z = 0.415 - (0.25 - x * x).sqrt();
                        v.push((s.shield.at(v3(x, y, z)), 0.025));
                    }
                }
            }
        }
        v
    }

    /// Arm `j` (upper arm, forearm and hand) as balls.
    fn arm_balls(s: &Skeleton, j: usize) -> Vec<(Vec3, f32)> {
        let mut v = capsule(s.upper[j].pos, s.fore[j].pos, 0.055, 0.045);
        v.extend(capsule(s.fore[j].pos, s.hand[j].pos, 0.045, 0.035));
        v.extend(capsule(
            s.hand[j].pos,
            s.hand[j].at(v3(0.0, -0.1, 0.0)),
            0.045,
            0.04,
        ));
        v
    }

    /// Leg `k` as balls: the knee, and the whole leg (thigh, shin and foot).
    fn leg_balls(s: &Skeleton, k: usize) -> (Balls, Balls) {
        let mut leg = capsule(s.thigh[k].pos, s.shin[k].pos, 0.08, 0.06);
        leg.extend(capsule(s.shin[k].pos, s.foot[k].pos, 0.05, 0.037));
        for z in [0.094, -0.213] {
            let toe = s.foot[k].at(v3(0.0, -rig::ANKLE_Y, z));
            leg.extend(capsule(s.foot[k].pos, toe, 0.04, 0.03));
        }
        (vec![(s.shin[k].pos, 0.056)], leg)
    }

    #[test]
    fn a_reviver_kneels_clear_of_its_mate_and_of_what_it_let_go_of() {
        const KNEE_ARM: &str = "a knee from an arm";
        const LEG_ARM: &str = "a leg from an arm";
        const LEG_BODY: &str = "a leg from the body";
        const LEG_DROPPED: &str = "a leg from the mate's weapon";
        const GEAR_DROPPED: &str = "its weapon from the mate's";
        const GEAR_ARM: &str = "its weapon from an arm";
        const GEAR_BODY: &str = "its weapon from the body";
        // The least clearance of each kind, and where.
        let mut least = std::collections::BTreeMap::<&str, (f32, String)>::new();
        fn note<'a>(
            least: &mut std::collections::BTreeMap<&'a str, (f32, String)>,
            what: &'a str,
            d: f32,
            case: impl FnOnce() -> String,
        ) {
            let e = least.entry(what).or_insert((f32::MAX, String::new()));
            if d < e.0 {
                *e = (d, case());
            }
        }
        let kinds = [
            Kind::Longsword,
            Kind::SwordShield,
            Kind::Rifle,
            Kind::Bow,
            Kind::Fists,
        ];
        let fighters = || {
            kinds
                .into_iter()
                .flat_map(|k| [(k, true), (k, false)])
                .map(|(k, fighting)| {
                    let act = if fighting { Act::Guard } else { Act::Relaxed };
                    (format!("{k:?} ({act:?})"), k, input(k, act, fighting))
                })
        };
        for (mate, pk, was) in fighters() {
            for (bent, flung) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let fall = Fall {
                    t: 5.0,
                    yaw: 0.6,
                    bent,
                    flung,
                };
                let lying = solve(&Input {
                    fall: Some(fall),
                    ..was
                });
                let at = Lying::of(&lying, fall);
                // It kneels on the side of the arm along the body (the brain sends it there),
                // the arm flung out wide (and what that hand flung) across the body from it.
                let side = fall.lie().along();
                let slot = at.knees[side];
                let arms = [arm_balls(&lying, 0), arm_balls(&lying, 1)];
                let dropped = gear_balls(pk, &lying);
                let mut body = capsule(lying.pelvis.pos, lying.chest.pos, 0.15, 0.17);
                body.extend(capsule(
                    lying.head.pos,
                    lying.head.at(v3(0.0, 0.12, 0.0)),
                    0.11,
                    0.11,
                ));
                for k in 0..2 {
                    let (knee, leg) = leg_balls(&lying, k);
                    body.extend(knee);
                    body.extend(leg);
                }
                // Stopped on the spot, or short of it, out from the mate (it kneels over to
                // the spot).
                let out = (slot - at.chest) * v3(1.0, 0.0, 1.0);
                for root in [slot, slot + out.normalize() * 0.5] {
                    let to = at.chest - root;
                    let facing = ry((-to.x).atan2(-to.z));
                    let local = |p: Vec3| facing.inverse() * (p - root);
                    let world = |v: Vec<(Vec3, f32)>| -> Vec<(Vec3, f32)> {
                        v.into_iter().map(|(p, r)| (root + facing * p, r)).collect()
                    };
                    for (reviver, rk, base) in fighters() {
                        let case = format!(
                            "{reviver} reviving {mate} (bent {bent}, flung {flung}) from {:.2}",
                            root - slot
                        );
                        let pose = |k: f32| {
                            solve(&Input {
                                tend: Some(Tend {
                                    k,
                                    at: local(at.chest),
                                    knees: local(slot),
                                }),
                                ..base
                            })
                        };
                        // Getting down, sat back setting its weapon down, and on its knees over
                        // the chest.
                        for k in [0.45, 0.7, 1.0] {
                            let s = pose(k);
                            for j in 0..2 {
                                let (knee, leg) = leg_balls(&s, j);
                                let (knee, leg) = (world(knee), world(leg));
                                let at = || format!("{case}: leg {j} at {k}");
                                for arm in &arms {
                                    note(&mut least, KNEE_ARM, clearance(&knee, arm), at);
                                    note(&mut least, LEG_ARM, clearance(&leg, arm), at);
                                }
                                note(&mut least, LEG_BODY, clearance(&leg, &body), at);
                                note(&mut least, LEG_DROPPED, clearance(&leg, &dropped), at);
                            }
                        }
                        // Its own weapon set down (or a bow or a shield held aside).
                        let gear = world(gear_balls(rk, &pose(1.0)));
                        if gear.is_empty() {
                            continue;
                        }
                        let at = || case.clone();
                        note(&mut least, GEAR_DROPPED, clearance(&gear, &dropped), at);
                        for arm in &arms {
                            note(&mut least, GEAR_ARM, clearance(&gear, arm), at);
                        }
                        note(&mut least, GEAR_BODY, clearance(&gear, &body), at);
                    }
                }
            }
        }
        // The knees well clear of the arms (the one flung out wide, and the one along the body
        // it kneels by); the legs clear of the arms, of the body and of what the mate let go
        // of; its own weapon well clear of the mate's, and clear of its arms and its body.
        let short: Vec<String> = [
            (KNEE_ARM, 0.05),
            (LEG_ARM, 0.03),
            (LEG_BODY, 0.03),
            (LEG_DROPPED, 0.03),
            (GEAR_DROPPED, 0.05),
            (GEAR_ARM, 0.02),
            (GEAR_BODY, 0.02),
        ]
        .into_iter()
        .filter_map(|(what, need)| {
            let (d, case) = &least[what];
            println!("{what}: {d:.3} m ({case})");
            (*d <= need).then(|| format!("{what}: {d:.3} m ({case})"))
        })
        .collect();
        assert!(short.is_empty(), "{short:#?}");
    }

    #[test]
    fn a_reviver_puts_its_hands_on_the_chest_from_anywhere_it_kneels() {
        use crate::brain::{REVIVE_ARRIVE, REVIVE_STAY};
        // The brain sends a reviver to kneel by where it reckons the breastbone lies, and lets
        // it kneel (and stay down) within so far of that spot: the body kneels over to it
        // from anywhere that close.
        const RECKONED: f32 = 0.1;
        assert!(REVIVE_ARRIVE < REVIVE_STAY && REVIVE_STAY as f32 + RECKONED <= KNEEL_REACH);
        let kinds = [
            Kind::Longsword,
            Kind::SwordShield,
            Kind::Rifle,
            Kind::Bow,
            Kind::Fists,
        ];
        let lies = [-0.9, -0.3, 0.4, 0.9]
            .into_iter()
            .flat_map(|yaw| [(0, 0), (0, 1), (1, 0), (1, 1)].map(|(b, f)| (yaw, b, f)));
        for pk in kinds {
            for fighting in [true, false] {
                let act = if fighting { Act::Guard } else { Act::Relaxed };
                for (yaw, bent, flung) in lies.clone() {
                    let fall = Fall {
                        t: 5.0,
                        yaw,
                        bent,
                        flung,
                    };
                    let lying = solve(&Input {
                        fall: Some(fall),
                        ..input(pk, act, fighting)
                    });
                    let at = Lying::of(&lying, fall);
                    let mate = format!("{pk:?} ({act:?}, along {yaw}, bent {bent}, flung {flung})");
                    // Where the brain reckons it lies (from where it stood, turned as it fell).
                    let turn = ry(fall.yaw);
                    let reckoned = turn * fall.lie().sternum();
                    let off = (reckoned - at.chest).length();
                    assert!(
                        off < RECKONED,
                        "{mate}: the chest {off:.3} m off the reckoning"
                    );
                    for side in 0..2 {
                        let spot = turn * (fall.lie().sternum() * v3(1.0, 0.0, 1.0))
                            + turn * fall.lie().kneel_by(side);
                        let off = (spot - at.knees[side]).length();
                        assert!(off < RECKONED, "{mate}: the knees {off:.3} m off the spot");
                        let rings = (0..12).map(|n| {
                            let a = n as f32 * TAU / 12.0;
                            spot + v3(a.cos(), 0.0, a.sin()) * REVIVE_STAY as f32
                        });
                        for root in std::iter::once(spot).chain(rings) {
                            let to = at.chest - root;
                            let facing = ry((-to.x).atan2(-to.z));
                            let local = |p: Vec3| facing.inverse() * (p - root);
                            for rk in kinds {
                                let s = solve(&Input {
                                    tend: Some(Tend {
                                        k: 1.0,
                                        at: local(at.chest),
                                        knees: local(at.knees[side]),
                                    }),
                                    ..input(rk, Act::Guard, true)
                                });
                                // The palms, side by side over the breastbone (or one alone,
                                // the other holding a bow or a shield aside).
                                let palm = |j: usize| {
                                    let x = if j == 0 { 0.045 } else { -0.045 };
                                    root + facing * s.hand[j].at(v3(x, -0.075, 0.0))
                                };
                                let on = match rk {
                                    Kind::Bow | Kind::SwordShield => palm(1),
                                    _ => (palm(0) + palm(1)) * 0.5,
                                };
                                let flat = Vec2::new(on.x - at.chest.x, on.z - at.chest.z);
                                let (miss, depth) = (flat.length(), at.chest.y - on.y);
                                assert!(
                                    miss < 0.03 && (-0.02..PRESS + 0.02).contains(&depth),
                                    "{rk:?} reviving {mate} from side {side}, {:.2} off the \
                                     spot: the hands {miss:.3} m off the chest, {depth:.3} m \
                                     into it",
                                    root - spot
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// The most a part of `frames` moves in a frame between `from` and `to` (m), which part
    /// and when.
    fn worst_jump(frames: &[(f64, Vec<Vec3>)], from: f64, to: f64) -> (f32, usize, f64) {
        let mut most = (0.0f32, 0, 0.0);
        for w in frames.windows(2).filter(|w| (from..to).contains(&w[1].0)) {
            for (n, (a, b)) in w[0].1.iter().zip(&w[1].1).enumerate() {
                if (*a - *b).length() > most.0 {
                    most = ((*a - *b).length(), n, w[1].0);
                }
            }
        }
        most
    }

    const TEND_PARTS: [Part; 12] = [
        PELVIS,
        Part::Torso,
        Part::Head,
        SHIN_L,
        SHIN_R,
        FOOT_L,
        FOOT_R,
        FOREARM_L,
        FOREARM_R,
        HAND_L,
        HAND_R,
        Part::Weapon,
    ];

    #[test]
    fn a_reviver_downed_as_it_kneels_goes_down_from_the_kneel_and_gets_up_from_there() {
        // Kneeling with the mate's chest ahead (a little to one side), and well off to either
        // side (the body kneels over to it).
        let tends = [
            (v3(0.0, 0.28, -0.62), v3(0.0, 0.0, -0.18)),
            (v3(0.05, 0.28, -0.62), v3(0.0, 0.0, -0.2)),
            (v3(0.3, 0.3, -0.55), v3(0.05, 0.0, -0.25)),
            (v3(-0.5, 0.26, -0.8), v3(-0.3, 0.0, -0.45)),
        ]
        .map(|(at, knees)| Tend { k: 1.0, at, knees });
        // (Bare fists hold nothing.)
        let joints = |kind: Kind, s: &Skeleton| {
            let mut v = tend_joints(s);
            if kind == Kind::Fists {
                v.remove(3);
            }
            v
        };
        let off = |kind: Kind, a: &Skeleton, b: &Skeleton| {
            joints(kind, a)
                .iter()
                .zip(joints(kind, b))
                .map(|(p, q)| (*p - q).length())
                .fold(0.0, f32::max)
        };
        for kind in [
            Kind::Longsword,
            Kind::SwordShield,
            Kind::Rifle,
            Kind::Bow,
            Kind::Fists,
        ] {
            let lies = [-0.9, -0.4, 0.0, 0.3, 0.6, 0.9]
                .into_iter()
                .flat_map(|yaw| [(0, 0), (0, 1), (1, 0), (1, 1)].map(|(b, f)| (yaw, b, f)));
            for ((yaw, bent, flung), (n, tend)) in
                lies.flat_map(|lie| tends.into_iter().enumerate().map(move |t| (lie, t)))
            {
                let case = format!(
                    "{kind:?} by chest {n}, fallen along {yaw}, bent {bent}, flung {flung}"
                );
                let base = input(kind, Act::Guard, true);
                let kneel = solve(&Input {
                    tend: Some(tend),
                    ..base
                });
                let fall = |t: f32| Fall {
                    t,
                    yaw,
                    bent,
                    flung,
                };
                let pose = |t: f32| {
                    solve(&Input {
                        tend: Some(tend),
                        fall: Some(fall(t)),
                        ..base
                    })
                };
                let laid = set_down(kind).is_some();
                // It goes down from the kneel (no pop up first), at 60 Hz without a jump, the
                // weapon it set down staying where it lies.
                let start = off(kind, &pose(0.0), &kneel);
                assert!(
                    start < 0.01,
                    "{case}: a pop of {start:.3} m as it goes down: {:.2?} from {:.2?}",
                    joints(kind, &pose(0.0)),
                    joints(kind, &kneel)
                );
                let mut last = pose(0.0);
                for f in 1..=96 {
                    let t = f as f32 / 60.0;
                    let s = pose(t);
                    for j in 0..2 {
                        let len = |a: Bone, b: Bone| (b.pos - a.pos).length();
                        assert!((len(s.thigh[j], s.shin[j]) - rig::THIGH).abs() < 1e-3);
                        assert!((len(s.shin[j], s.foot[j]) - rig::SHIN).abs() < 1e-3);
                        assert!((len(s.upper[j], s.fore[j]) - rig::UPPER_ARM).abs() < 1e-3);
                        assert!((len(s.fore[j], s.hand[j]) - rig::FOREARM).abs() < 1e-3);
                    }
                    for (n, p) in joints(kind, &s).into_iter().enumerate() {
                        assert!(
                            p.y > -0.02,
                            "{case}: joint {n} in the ground at {t:.2} s ({p:.3})"
                        );
                    }
                    let jump = off(kind, &s, &last);
                    assert!(
                        jump < 0.12,
                        "{case}: a joint jumps {jump:.2} m at {t:.2} s: {:.2?} from {:.2?}",
                        joints(kind, &s),
                        joints(kind, &last)
                    );
                    if laid {
                        let moved = (s.weapon.pos - kneel.weapon.pos).length();
                        assert!(moved < 1e-3, "{case}: the weapon moved {moved:.3} m");
                    }
                    last = s;
                }
                let lying = pose(3.0);
                assert!(
                    lying.pelvis.pos.y < 0.35 && lying.head.pos.y < 0.35,
                    "{case}: the hips at {:.2} m, the head at {:.2} m",
                    lying.pelvis.pos.y,
                    lying.head.pos.y
                );
                // Revived, it gets up from there (the weapon taken up from where it lies, and
                // not drawn over the ground to the hand; bare fists push off the ground).
                let armed = kind != Kind::Fists;
                let rise = |t: f32| {
                    solve(&Input {
                        rise: Some(Rise {
                            t,
                            fall: fall(3.0),
                            was: Act::Guard,
                            since: base.now,
                            feet: base.feet,
                            knelt: Some(tend),
                        }),
                        ..base
                    })
                };
                let start = off(kind, &rise(0.0), &lying);
                assert!(start < 1e-3, "{case}: a pop of {start:.3} m as it gets up");
                let end = off(kind, &rise(rise_secs(kind)), &solve(&base));
                assert!(end < 1e-3, "{case}: ends {end:.3} m off its guard");
                let (mut last, mut taken) = (rise(0.0), false);
                for f in 1..=(rise_secs(kind) * 60.0).ceil() as usize {
                    let t = f as f32 / 60.0;
                    let s = rise(t);
                    let jump = off(kind, &s, &last);
                    assert!(
                        jump < 0.1,
                        "{case}: a joint jumps {jump:.2} m at {t:.2} s up: {:.2?} from {:.2?}",
                        joints(kind, &s),
                        joints(kind, &last)
                    );
                    for (n, p) in joints(kind, &s).into_iter().enumerate() {
                        assert!(
                            p.y > -0.02,
                            "{case}: joint {n} in the ground at {t:.2} s up ({p:.3})"
                        );
                    }
                    if !armed {
                        last = s;
                        continue;
                    }
                    let (w, lay) = (s.weapon.pos, lying.weapon.pos);
                    let slid = Vec2::new(w.x - lay.x, w.z - lay.z).length();
                    if w.y < lay.y + 0.01 {
                        assert!(
                            slid < 0.02,
                            "{case}: the weapon drawn {slid:.3} m over the ground at {t:.2} s up"
                        );
                    }
                    let moved = (s.weapon.pos - lying.weapon.pos).length();
                    if !taken && moved > 1e-4 {
                        taken = true;
                        assert!(
                            moved < 0.03,
                            "{case}: the weapon leaps {moved:.2} m to hand"
                        );
                    }
                    last = s;
                }
            }
        }
    }

    #[test]
    fn a_reviver_shot_down_on_its_knees_falls_from_them_and_one_revived_kneels_once_it_is_up() {
        let chest = DVec3::new(0.05, 0.28, -0.62);
        let knees = DVec3::new(0.0, 0.0, -0.2);
        for weapon in [
            NpcWeapon::sword(),
            NpcWeapon::sword_and_shield(),
            NpcWeapon::rifle(),
            NpcWeapon::bow(),
        ] {
            let kind = Kind::of(&weapon);
            let case = format!("{kind:?}");
            let laid = set_down(kind).is_some();
            // Kneeling by a mate from 1.5 s, shot down there at 3.5 s.
            let (from, shot) = (1.5, 3.5);
            let brain = |now: f64| {
                let tending = (from..shot).contains(&now);
                NpcBrain {
                    alert: AlertLevel::Combat,
                    action: if tending {
                        NpcAction::Revive
                    } else if now >= shot {
                        NpcAction::Down
                    } else {
                        NpcAction::Idle
                    },
                    revive_kneel: if tending {
                        (now - from) as f32 + 0.01
                    } else {
                        0.0
                    },
                    ..Default::default()
                }
            };
            let vitals = |now: f64| {
                Some(NpcVitals {
                    downed: now >= shot,
                    ..Default::default()
                })
            };
            let patient = |now: f64| (from..shot).contains(&now).then_some((chest, knees));
            let mut frames: Vec<(f64, Vec<Vec3>)> = Vec::new();
            Run {
                brain: &brain,
                vitals: &vitals,
                patient: &patient,
                ..Run::new(5.5, weapon)
            }
            .look(|now, pose| frames.push((now, TEND_PARTS.map(|p| pose(p).pos).to_vec())));
            let at = |t: f64| &frames.iter().find(|f| f.0 >= t).unwrap().1;
            let jump = worst_jump(&frames, 0.0, 10.0);
            assert!(jump.0 < 0.12, "{case}: a part jumps going down: {jump:.2?}");
            let kneeling = at(shot - 0.02);
            assert!(
                kneeling[0].y < 0.55,
                "{case}: the hips at {:.2} m",
                kneeling[0].y
            );
            let down = at(shot + 2.5);
            assert!(down[0].y < 0.35, "{case}: the hips at {:.2} m", down[0].y);
            if laid {
                // The weapon it set down stays where it lies.
                let moved = frames
                    .iter()
                    .filter(|f| f.0 >= shot - 0.02)
                    .map(|f| (f.1[11] - kneeling[11]).length())
                    .fold(0.0, f32::max);
                assert!(moved < 0.01, "{case}: the weapon moved {moved:.2} m");
            }

            // Revived (down until 2 s), and kneeling by another mate as soon as it is (the
            // brain has it wait, but the pose doesn't count on that): it gets up first, then
            // goes down to the mate, without a jump.
            let (down_from, up) = (1.2, 2.0);
            let vitals = |now: f64| {
                Some(NpcVitals {
                    downed: (down_from..up).contains(&now),
                    ..Default::default()
                })
            };
            let brain = |now: f64| NpcBrain {
                alert: AlertLevel::Combat,
                action: if now >= up + 0.05 {
                    NpcAction::Revive
                } else {
                    NpcAction::Idle
                },
                revive_kneel: (now - up - 0.05).max(0.0) as f32,
                ..Default::default()
            };
            let patient = |now: f64| (now >= up + 0.05).then_some((chest, knees));
            let mut frames: Vec<(f64, Vec<Vec3>)> = Vec::new();
            Run {
                brain: &brain,
                vitals: &vitals,
                patient: &patient,
                ..Run::new(6.0, weapon)
            }
            .look(|now, pose| frames.push((now, TEND_PARTS.map(|p| pose(p).pos).to_vec())));
            let jump = worst_jump(&frames, up - 0.01, 10.0);
            assert!(
                jump.0 < 0.1,
                "{case}: a part jumps getting up to kneel: {jump:.2?}"
            );
            let at = |t: f64| &frames.iter().find(|f| f.0 >= t).unwrap().1;
            let risen = up + f64::from(rise_secs(kind));
            let stood = at(risen - 0.02);
            assert!(
                stood[0].y > 0.8,
                "{case}: still getting up, the hips at {:.2} m",
                stood[0].y
            );
            let knelt = at(risen + f64::from(TEND_DOWN) + 0.1);
            assert!(knelt[0].y < 0.55, "{case}: the hips at {:.2} m", knelt[0].y);
        }
    }

    #[test]
    fn a_reviver_getting_up_does_not_drag_its_knees_over_the_ground() {
        // Kneeling until 3 s, then (were the brain not to hold it a moment) off at once, a run
        // picking up at 20 m/s².
        let (from, until) = (1.2, 3.0);
        let chest = DVec3::new(0.05, 0.28, -0.62);
        let knees = DVec3::new(0.0, 0.0, -0.2);
        let tending = |now: f64| (from..until).contains(&now);
        let brain = |now: f64| NpcBrain {
            alert: AlertLevel::Combat,
            action: if tending(now) {
                NpcAction::Revive
            } else {
                NpcAction::Idle
            },
            revive_kneel: if tending(now) {
                (now - from) as f32 + 0.01
            } else {
                0.0
            },
            ..Default::default()
        };
        let patient = |now: f64| tending(now).then_some((chest, knees));
        for hold in [0.0, NpcBrain::REVIVE_STAND_SECS] {
            let speed = move |now: f64| (20.0 * (now - until - hold)).clamp(0.0, 4.5) as f32;
            let vel = move |now: f64| Vec3::X * speed(now);
            for weapon in [NpcWeapon::sword(), NpcWeapon::rifle(), NpcWeapon::bow()] {
                let case = format!("{:?} held {hold} s", Kind::of(&weapon));
                // Where the knees are in the world (the owner moving off along +X).
                let (mut went, mut frames) = (0.0f32, Vec::new());
                Run {
                    brain: &brain,
                    patient: &patient,
                    vel: &vel,
                    ..Run::new(4.0, weapon)
                }
                .look(|now, pose| {
                    let knees = [SHIN_L, SHIN_R].map(|p| pose(p).pos + Vec3::X * went);
                    frames.push((now, knees));
                    went += speed(now) / 60.0;
                });
                let set = frames.iter().find(|f| f.0 >= until - 0.02).unwrap().1;
                for (now, k) in frames.iter().filter(|f| f.0 >= until) {
                    for j in 0..2 {
                        // (On the ground: about as low as kneeling has it.)
                        if k[j].y < 0.1 {
                            let slid = Vec2::new(k[j].x - set[j].x, k[j].z - set[j].z).length();
                            assert!(
                                slid < 0.1,
                                "{case}: knee {j} slid {slid:.2} m over the ground at {now:.2}"
                            );
                        }
                    }
                }
            }
        }
        // Held, it is about up when it may move.
        let k = 1.0 - NpcBrain::REVIVE_STAND_SECS as f32 / TEND_UP;
        assert!(step(0.0, 0.55, k) < 0.2, "still {k:.2} down when it moves");
    }

    #[test]
    fn nobody_kneels_by_a_mate_before_it_is_up_itself() {
        for kind in [
            Kind::Longsword,
            Kind::SwordShield,
            Kind::Rifle,
            Kind::Bow,
            Kind::Fists,
        ] {
            assert!(
                f64::from(rise_secs(kind)) <= NpcBrain::GET_UP_SECS,
                "{kind:?} takes {} s to get up",
                rise_secs(kind)
            );
        }
    }

    #[test]
    fn a_reviver_looking_at_the_chest_from_a_step_off_presses_and_one_glancing_up_waits() {
        // The chest a step off the knees (kneeling over to it), the eyes on it, or up at a foe.
        let at = v3(0.0, 0.28, -1.2);
        let knees = v3(0.0, 0.0, -0.75);
        let eyes = |p: Vec3| {
            let d = p - v3(0.0, TEND_EYES, 0.0);
            Vec2::new((-d.x).atan2(-d.z), d.y.atan2(Vec2::new(d.x, d.z).length()))
        };
        for kind in [Kind::Longsword, Kind::Rifle, Kind::Bow] {
            for (up, toward) in [(false, at), (true, v3(4.0, 1.55, -8.0))] {
                let case = format!(
                    "{kind:?} looking {}",
                    if up { "up" } else { "at the chest" }
                );
                let base = input(kind, Act::Guard, true);
                let pose = |now: f64| {
                    solve(&Input {
                        now,
                        tend: Some(Tend { k: 1.0, at, knees }),
                        look: Look {
                            head: eyes(toward),
                            eyes: eyes(toward),
                            focus: 1.0,
                            ..Default::default()
                        },
                        ..base
                    })
                };
                let (lo, hi) = (0..40)
                    .map(|f| pose(base.now + f64::from(f) / 40.0 / f64::from(PRESS_HZ)))
                    .map(|s| s.hand[1].pos.y)
                    .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
                let depth = hi - lo;
                let gaze = (pose(base.now).head.rot * Vec3::NEG_Z).y;
                if up {
                    assert!(depth < 0.3 * PRESS, "{case}: the hands press {depth:.3} m");
                    assert!(gaze > -0.3, "{case}: the head looks along {gaze:.2}");
                } else {
                    assert!(
                        (depth - PRESS).abs() < 0.01,
                        "{case}: the hands press {depth:.3} m"
                    );
                    assert!(gaze < -0.5, "{case}: the head looks along {gaze:.2}");
                }
            }
        }
    }

    #[test]
    fn a_magazine_or_an_arrow_in_hand_goes_away_smoothly_as_a_reviver_kneels() {
        let at = v3(0.0, 0.28, -0.62);
        let knees = v3(0.0, 0.0, -0.18);
        for kind in [Kind::Rifle, Kind::Bow] {
            let base = input(kind, Act::Guard, true);
            let ready = if kind == Kind::Rifle { 2.5 } else { 1.0 };
            let shot = |since: f32| Shot {
                since,
                ready,
                reload: kind == Kind::Rifle,
            };
            // A moment in the shot when something is in hand.
            let times: Vec<f32> = (0..300)
                .map(|n| n as f32 * 0.01)
                .filter(|&t| {
                    solve(&Input {
                        shot: Some(shot(t)),
                        ..base
                    })
                    .held
                    .is_some()
                })
                .collect();
            assert!(!times.is_empty(), "{kind:?}: nothing in hand");
            let since = times[times.len() / 2];
            let item = |s: &Skeleton| match kind {
                Kind::Rifle => Some(
                    s.held
                        .unwrap_or(s.weapon.then(Bone::new(weapons::RIFLE_MAG, Quat::IDENTITY)))
                        .pos,
                ),
                _ if s.nocked => None,
                _ => s.held.map(|b| b.pos),
            };
            let n = (TEND_DOWN * 60.0).ceil() as usize;
            let mut last: Option<Vec3> = None;
            let mut last_carriers = [Vec3::ZERO; 3];
            let mut gone = false;
            for f in 0..=n {
                let k = f as f32 / n as f32;
                let s = solve(&Input {
                    shot: Some(shot(since)),
                    tend: Some(Tend { k, at, knees }),
                    ..base
                });
                let now = item(&s);
                // It goes along with what carries it (the hand, then the carbine or the
                // quiver), never faster.
                let carriers = [
                    s.hand[usize::from(kind == Kind::Bow)].pos,
                    s.weapon.at(weapons::RIFLE_MAG),
                    s.chest.at(weapons::QUIVER_MOUTH),
                ];
                let carried = carriers
                    .iter()
                    .zip(last_carriers)
                    .map(|(a, b)| (*a - b).length())
                    .fold(0.0, f32::max);
                last_carriers = carriers;
                match (last, now) {
                    (Some(a), Some(b)) => {
                        let jump = (b - a).length();
                        assert!(
                            jump < carried + 0.02,
                            "{kind:?}: it jumps {jump:.2} m at {k:.2} (what carries it {carried:.2} m)"
                        );
                    }
                    (Some(a), None) => {
                        // Put back in the quiver as it goes.
                        let mouth = s.chest.at(weapons::QUIVER_MOUTH);
                        let off = (a - mouth).length();
                        assert!(
                            off < 0.05,
                            "{kind:?}: gone {off:.2} m off the quiver at {k:.2}"
                        );
                        gone = true;
                    }
                    _ => {}
                }
                last = now;
            }
            if kind == Kind::Bow {
                assert!(gone, "the arrow stayed in hand");
            }
        }
    }
}
