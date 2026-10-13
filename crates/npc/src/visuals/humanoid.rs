//! Fighter bodies. With the renderer they are built from the procedural armored kit of
//! [`crate::art::fighter`]; without it, from a few capsules. Either way one skeleton is posed
//! every step from the AI state and from what the body did the step before (`Motion`): the
//! feet stay where they are on the ground while the body moves over them, and step along the
//! gait (or, standing, back under the body when out of place); the pelvis, chest and head are
//! placed from the gait, stance and action (the body behind a blow, a block or a stagger);
//! then two-bone chains solve the legs and the arms (hands on the weapon's grips), so knees
//! and elbows bend.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};

use super::*;
use crate::agent::{
    AlertLevel, AttackPhase, Defense, NpcBrain, NpcCombat, NpcVitals, NpcWeapon, WeaponStyle,
};
use crate::art::ArtAssets;
use crate::art::fighter::{FACE_SHAPES, Piece, WEARS, basis, kit, rig, weapons};
use crate::combat::Downed;
use crate::faction::Faction;
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
const PAINT0: u8 = 31;

const SLOTS: usize = 42;

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

/// The parts of a fighter's body.
pub(super) fn parts(
    stand_in: &StandIn,
    weapon: Option<&NpcWeapon>,
    faction: Option<&Faction>,
    skills: Option<&NpcSkills>,
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
    ];
    if !stand_in.plain {
        list.push((Part::Band, Piece::Crest));
    }
    match kind {
        Kind::Longsword => list.extend([(Part::Weapon, Piece::LongBlade), (HILT, Piece::LongHilt)]),
        Kind::SwordShield => list.extend([
            (Part::Weapon, Piece::ArmingBlade),
            (HILT, Piece::ArmingHilt),
            (Part::Shield, Piece::Shield),
        ]),
        Kind::Rifle => list.extend([(Part::Weapon, Piece::Rifle), (Part::Muzzle, Piece::Muzzle)]),
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
    // come from one seed. Seeds closer than 24 never share both skin tone and shape.
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
        let mesh = if piece == Piece::Face {
            let key = format!(
                "fighter/face{shape}/{skin_hex:06x}/{:06x}{:06x}",
                iris_hex[0], iris_hex[1]
            );
            let iris = iris_hex.map(linear);
            art.mesh(&key, || kit.face(shape, linear(skin_hex), iris))
        } else {
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
struct Bone {
    pos: Vec3,
    rot: Quat,
}

impl Bone {
    const fn new(pos: Vec3, rot: Quat) -> Self {
        Self { pos, rot }
    }

    /// A point given in this frame.
    fn at(&self, p: Vec3) -> Vec3 {
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
    /// Guard raised against a blow: `k` how far (easing in as it rises, out as it ends), `hit`
    /// seconds since the blow landed on it (negative before), `parry` a timed deflection.
    Block { k: f32, hit: f32, parry: bool },
    /// Knocked off balance (`k` eases in and out), `e` s ago.
    Stagger { k: f32, e: f32 },
}

/// Everything the pose depends on.
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
    /// 0 standing .. 1 lying on the back; above 0 only when downed.
    fall: f32,
    /// Per-fighter offset for idle motions.
    seed: f64,
}

/// The posed skeleton (owner space).
#[derive(Debug, Clone, Copy, Default)]
struct Skeleton {
    pelvis: Bone,
    chest: Bone,
    head: Bone,
    /// Left then right.
    thigh: [Bone; 2],
    shin: [Bone; 2],
    foot: [Bone; 2],
    upper: [Bone; 2],
    fore: [Bone; 2],
    hand: [Bone; 2],
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
}

/// Where a hand goes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hand {
    /// On a grip: its center and the hand's rotation.
    Grip(Vec3, Quat),
    /// The wrist at a point, the hand in line with the forearm.
    Free(Vec3),
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

/// One foot's memory: where it stands on the ground (owner space), or stood when it lifted.
#[derive(Debug, Clone, Copy, Default)]
struct Foot {
    at: Vec2,
    yaw: f32,
    /// In the air: since when (NPC time), for how long and how high.
    air: Option<(f64, f32, f32)>,
    /// The gait lap in which it last lifted (a foot lifts once a lap).
    lap: i64,
}

/// What a fighter's pose remembers from one step to the next: its own gait clock, where its
/// feet stand, and when its guard went up or its dodge began. Purely visual, and kept in the
/// owner's frame and its clock (no world points: a world shift doesn't disturb it; a jump in
/// heading, as a turned world makes, starts it afresh).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Motion {
    posed: bool,
    /// When it was last posed (NPC clock) and the heading then.
    last: f64,
    yaw: f32,
    /// Gait cycles walked (the fraction is where in the cycle).
    cycles: f64,
    feet: [Foot; 2],
    /// Velocity last step (owner frame) and the smoothed acceleration.
    vel: Vec2,
    accel: Vec2,
    /// The guard up (block or parry): its end, when it rose, when the blow lands on it.
    guard: (f64, f64, f64),
    /// The dodge under way: its end, its start, and its side (−1 left, 1 right, 0 not yet).
    dodge: (f64, f64, f32),
    /// A dash under way: since when, and which way (owner frame).
    dash: Option<(f64, Vec2)>,
    /// The feet were walking to a rhythm last step (not just standing).
    striding: bool,
    /// The blow telegraphed or recovered from is a heavy one.
    heavy: bool,
    down: bool,
}

impl Motion {
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
            };
        }
    }

    /// Move the feet on `dt` s, the body having moved at the gait's velocity and turned by
    /// `turned`. A foot on the ground stays where it is on the ground; one in the air swings
    /// to where the gait (or, standing, the stance) wants it. A foot lifts when the gait's
    /// cycle comes round to it, or, standing, when it is out of place and the other one is
    /// down. In a dash (`carry`: how far through it, 0..1) both are carried along in a low hop,
    /// the leading one reaching out, and set down wherever that leaves them.
    #[allow(clippy::too_many_arguments)]
    fn step_feet(
        &mut self,
        now: f64,
        dt: f32,
        turned: f32,
        g: &Gait,
        rest: [(Vec2, f32); 2],
        ready: bool,
        carry: Option<(f32, Vec2)>,
    ) -> [Plant; 2] {
        let turn = ry(-turned);
        let moved = Vec3::new(g.vel.x, 0.0, g.vel.y) * dt;
        for f in &mut self.feet {
            let p = turn * Vec3::new(f.at.x, 0.0, f.at.y) - moved;
            f.at = Vec2::new(p.x, p.z);
            f.yaw = wrap(f.yaw - turned);
        }
        let target = |k: usize| g.target(k, rest[k], ready);
        if let Some((u, dir)) = carry {
            let lead = if (rest[1].0 - rest[0].0).dot(dir) > 0.0 {
                1
            } else {
                0
            };
            return std::array::from_fn(|k| {
                let (spot, yaw) = g.spot(k, rest[k], ready);
                let reach = if k == lead { 0.24 } else { -0.1 };
                let at = spot + dir * reach * ease(u);
                self.feet[k] = Foot {
                    at,
                    yaw,
                    air: None,
                    lap: self.feet[k].lap,
                };
                Plant {
                    at,
                    lift: 0.06 * (PI * u).sin(),
                    yaw,
                    pitch: 0.0,
                }
            });
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
                        pitch: if moving { g.swing_pitch(u) } else { 0.0 },
                    }
                }
                None => Plant {
                    at: f.at,
                    lift: 0.0,
                    yaw: f.yaw,
                    pitch: if moving && t < g.beta {
                        g.stance_pitch(t / g.beta)
                    } else {
                        0.0
                    },
                },
            }
        })
    }

    /// Advance the gait and the feet to `now`, the body facing `facing` (world) and moving at
    /// `vel` (world, m/s). `phase` (the mover's gait phase) starts the cycle off.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        now: f64,
        phase: f32,
        facing: Quat,
        vel: Vec3,
        kind: Kind,
        ready: bool,
        dashing: bool,
        rest_shift: Vec2,
    ) -> (Gait, [Plant; 2], bool) {
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
        }
        let v = facing.inverse() * vel;
        let vel = Vec2::new(v.x, v.z);
        if dt > 0.0 && !dashing {
            let a = ((vel - self.vel) / dt).clamp_length_max(12.0);
            self.accel = self.accel.lerp(a, (dt * 6.0).min(1.0));
        }
        self.accel *= if dashing { 0.0 } else { 1.0 };
        self.vel = vel;
        self.last = now;
        self.yaw = yaw;
        self.posed = true;
        let mut g = Gait::new(vel, 0.0, ready);
        if g.speed >= MOVING && !dashing {
            self.cycles += f64::from(g.rate * dt);
        }
        g.cycle = self.cycles.rem_euclid(1.0) as f32;
        let mut rest = rest_feet(kind, ready);
        for r in &mut rest {
            r.0 += rest_shift;
        }
        if fresh {
            self.reset_feet(now, &g, rest, ready);
        }
        let carry = match (dashing, self.dash) {
            (false, _) => {
                self.dash = None;
                None
            }
            (true, None) => {
                self.dash = Some((now, g.dir));
                Some((0.0, g.dir))
            }
            (true, Some((t0, dir))) => Some((((now - t0) / 0.22).clamp(0.0, 1.0) as f32, dir)),
        };
        let feet = self.step_feet(now, dt, turned, &g, rest, ready, carry);
        (g, feet, fresh)
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

/// The body braced behind a raised guard, rocked back as the blow lands.
fn block_body(k: f32, hit: f32, parry: bool) -> Body {
    let r = recoil(hit) * k;
    let beat = if parry { recoil(hit + 0.03) * k } else { 0.0 };
    Body {
        shift: v3(0.0, -0.04 * k, -0.03 * k + 0.05 * r),
        yaw: 0.06 * beat,
        twist: 0.06 * k + 0.12 * beat,
        lean: 0.08 * k - 0.14 * r,
        roll: 0.0,
        heel: [0.0; 2],
    }
}

/// Pose the skeleton.
fn solve(i: &Input) -> Skeleton {
    let mut s = Skeleton::default();
    let g = &i.gait;
    let (moving, run) = (g.moving, g.run);
    let down = i.fall > 0.0;
    let ready = (i.fighting || i.act != Act::Relaxed) && !down;
    let kneel = if i.crouch { 1.0 - moving } else { 0.0 };
    let still = 1.0 - moving;
    let st = stance(i.kind, ready);
    let clock =
        |period: f64, offset: f64| (i.now * TAU as f64 / period + i.seed * offset).sin() as f32;
    let breath = clock(3.8, 7.0);
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
        Act::Block { k, hit, parry } => block_body(k, hit, parry),
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
    if kneel > 0.0 {
        body = Body::default();
    }

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
        + still * if ready { 0.008 * sway } else { 0.04 * settle };
    // On guard the knees stay bent until the fighter breaks into a run.
    let striding = if ready {
        moving * step(1.5, 3.5, g.speed)
    } else {
        moving
    };
    let mut hip_y = rig::HIP_Y - lerp(st.drop, 0.035 * (1.0 + run), striding) + bob + body.shift.y;
    if ready && !down {
        hip_y -= 0.01 * (1.0 + bounce) * still;
    }
    if i.crouch {
        hip_y = lerp(0.52, 0.68, moving);
    }
    hip_y -= 0.12 * i.dodge.abs();
    // At ease the hips turn a little toward the leg they rest on; on guard the weight rocks
    // between the front foot and the back.
    let at_ease = if ready { 0.0 } else { 0.05 * settle * still };
    let rocking = if ready && !down { rock * still } else { 0.0 };
    let yaw =
        lerp(st.yaw, gait_yaw, moving) * (1.0 - 0.5 * kneel) - 0.2 * kneel + body.yaw + at_ease;
    // Leaning into the run and into speeding up; rolling into a sidestep.
    let push = (-i.accel.y * 0.02).clamp(-0.1, 0.14) * (1.0 - kneel);
    let tilt = 0.03 + 0.08 * run + if i.crouch { 0.25 } else { 0.0 } + push * 0.5;
    let side_lean = -0.05 * (g.vel.x / 2.0).clamp(-1.0, 1.0) * moving;
    let roll = weight * 1.4 + side_lean + body.roll;
    let pelvis_rot = ry(yaw) * rx(-tilt) * rz(roll);
    // Standing, the hips settle over the middle of the feet.
    let rest = rest_feet(i.kind, ready);
    let support = ((i.feet[0].at + i.feet[1].at) - (rest[0].0 + rest[1].0)) * 0.25 * still;
    let mut pelvis = Bone::new(
        v3(
            weight + 0.12 * i.dodge + body.shift.x + support.x,
            hip_y,
            body.shift.z + support.y - 0.025 * rocking,
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
        if k == 1 && kneel > 0.5 {
            pole = v3(0.0, -1.0, -0.6);
        }
        let side = foot.rot * Vec3::X;
        let (thigh, shin, ankle) = chain(hip, foot.pos, (rig::THIGH, rig::SHIN), pole, side, true);
        s.thigh[k] = thigh;
        s.shin[k] = shin;
        s.foot[k] = Bone::new(ankle, foot.rot);
    }
    // Tabards swing with the legs: the front one rides on the forward thigh.
    let thigh_angle = |k: usize| {
        let d = pelvis.rot.inverse() * (s.shin[k].pos - s.thigh[k].pos);
        (-d.z).atan2(-d.y)
    };
    let (a, b) = (thigh_angle(0), thigh_angle(1));
    let flap = 0.06 * run * (TAU * tl * 2.0).sin();
    // A thigh raised high (kneeling, a long stride) carries the front tabard with it: it turns
    // and slides over that thigh to lie along it instead of standing out in the air.
    let lead = if a > b { 0 } else { 1 };
    let up = a.max(b).max(0.0);
    let follow = step(0.6, 1.2, up);
    let knee = pelvis.rot.inverse() * (s.shin[lead].pos - s.thigh[lead].pos);
    let turn = (-knee.x).atan2(-knee.z).clamp(-0.6, 0.6) * follow;
    let slide = (s.thigh[lead].pos - pelvis.pos).dot(pelvis.rot * Vec3::X) * 0.8 * follow;
    s.tabard[0] = pelvis.then(Bone::new(
        v3(slide, 0.085, -0.128),
        ry(turn) * rx(up * 0.95 + 0.04 + 0.1 * run + flap),
    ));
    s.tabard[1] = pelvis.then(Bone::new(
        v3(0.0, 0.085, 0.13),
        rx(a.min(b).min(0.0) * 0.95 - 0.04 - 0.25 * run - flap),
    ));

    // ---------------------------------------------------------------- chest and head
    let mut lean = lerp(st.lean, 0.06 + 0.16 * run, moving) + 0.12 * kneel - 0.025 * breath
        + body.lean
        + push
        + 0.03 * rocking;
    let mut twist = lerp(st.twist, -gait_yaw * 1.6, moving) + body.twist;
    let mut chest_roll = -roll * 0.6 - 0.35 * i.dodge + body.roll * 0.5;
    match i.act {
        Act::Windup { p, .. } if i.kind == Kind::Bow => {
            twist -= 0.25 * step(0.0, 0.4, p);
        }
        Act::Strike { t, .. } if i.kind == Kind::Rifle => {
            // The kick rocks the shoulders back.
            lean -= 0.05 * (-t / 0.06).exp();
        }
        Act::Stagger { k, e } => {
            lean -= (0.22 + 0.06 * (i.now * 12.0).sin() as f32) * k + 0.3 * snap(e);
            chest_roll += 0.1 * k * (i.now * 7.0).sin() as f32;
        }
        _ => {}
    }
    let waist = pelvis.at(rig::WAIST) + v3(0.0, 0.004 * breath, 0.0);
    let chest = Bone::new(waist, pelvis.rot * ry(twist) * rx(-lean) * rz(chest_roll));
    s.chest = chest;
    let aiming = if down {
        0.0
    } else {
        match (i.kind, i.act) {
            (Kind::Rifle, Act::Windup { .. } | Act::Strike { .. }) => 1.0,
            (Kind::Rifle, Act::Guard | Act::Block { .. }) => 1.0 - run,
            (Kind::Bow, Act::Windup { p, .. }) => step(0.0, 0.35, p),
            (Kind::Bow, Act::Strike { t, dur, .. }) => 1.0 - step(dur * 0.6, dur, t),
            _ => 0.0,
        }
    };
    let net_yaw = yaw + twist;
    let look = if ready { 0.0 } else { 0.25 * glance * still };
    let head_yaw = -net_yaw * lerp(0.85, 1.0, aiming) + look;
    let mut head_pitch = -(tilt + lean) * 0.75;
    let mut head_roll = -chest_roll * 0.5;
    match i.act {
        // Flinching from the blow, chin down behind the guard (not off a shouldered stock).
        Act::Block { k, hit, .. } => head_pitch += (0.1 + 0.12 * recoil(hit)) * k * (1.0 - aiming),
        // Snapped back by the blow.
        Act::Stagger { e, .. } => head_pitch -= 0.35 * snap(e),
        _ => {}
    }
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

    // ---------------------------------------------------------------- arms and weapon
    // Free arms swing against the legs (pumping when running); hands with something in them
    // swing it a little less.
    let swing = 0.35 * moving * (1.0 + run) * (TAU * tl).cos() * g.roll().abs().max(0.4);
    let jiggle = Vec2::new(clock(2.3, 1.0), clock(3.1, 4.0)) * still;
    let mut arms = match i.kind {
        Kind::Longsword => longsword(i.act, chest, swing, jiggle),
        Kind::SwordShield => sword_and_shield(i.act, chest, swing, jiggle),
        Kind::Rifle => rifle(i.act, chest, head, aiming),
        Kind::Bow => bow(i.act, chest, head, aiming, ready, swing),
        Kind::Fists => fists(i.act, chest, ready),
    };
    for k in 0..2 {
        let sgn = if k == 0 { -1.0 } else { 1.0 };
        if down {
            arms.hand[k] = Hand::Free(chest.at(v3(sgn * 0.42, 0.35 + 0.1 * sgn, 0.12)));
            arms.pole[k] = chest.rot * v3(sgn, -0.3, 0.6);
        } else if arms.hand[k] == Hand::Swing {
            let hang_to = v3(sgn * 0.035, -0.52, 0.04).lerp(v3(sgn * -0.02, -0.29, -0.22), run);
            let a = swing * -sgn;
            arms.hand[k] = Hand::Free(chest.at(shoulder(k) + rx(a) * hang_to));
            arms.pole[k] = chest.rot * v3(sgn * 0.3, -0.3, 1.0);
        }
    }
    for k in 0..2 {
        let sh = chest.at(shoulder(k));
        let (target, rot) = match arms.hand[k] {
            Hand::Grip(point, rot) => (point - rot * grip(k), Some(rot)),
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
        s.hand[k] = Bone::new(wrist, rot.unwrap_or(fore.rot));
        s.want[k] = target;
    }
    s.weapon = arms.weapon;
    s.shield = arms.shield;
    s.draw = arms.draw;
    s.nocked = arms.nocked && !down;
    s.bend = limb_bend(s.draw);

    // ---------------------------------------------------------------- falling
    if down {
        let f = i.fall;
        let pivot = v3(0.0, 0.0, 0.12);
        let r = rx(f * FRAC_PI_2);
        let lift = v3(0.0, 0.06 * f, 0.0);
        let fall = |b: Bone| Bone::new(pivot + r * (b.pos - pivot) + lift, r * b.rot);
        let held_weapon = fall(s.weapon);
        let held_shield = fall(s.shield);
        for b in [&mut s.pelvis, &mut s.chest, &mut s.head] {
            *b = fall(*b);
        }
        for k in 0..2 {
            for b in [
                &mut s.thigh[k],
                &mut s.shin[k],
                &mut s.foot[k],
                &mut s.upper[k],
                &mut s.fore[k],
                &mut s.hand[k],
                &mut s.tabard[k],
            ] {
                *b = fall(*b);
            }
        }
        // The weapon drops beside the body.
        let lying = |at: Vec3, yaw: f32, tip: Quat| Bone::new(at, ry(yaw) * tip);
        let (w, sh) = match i.kind {
            Kind::Longsword | Kind::SwordShield => (
                lying(v3(0.55, 0.02, 0.95), 0.3, rz(FRAC_PI_2)),
                lying(v3(-0.6, 0.075, 0.7), 0.4, rx(-FRAC_PI_2)),
            ),
            Kind::Rifle => (lying(v3(0.55, 0.025, 0.8), 0.5, rz(FRAC_PI_2)), held_shield),
            Kind::Bow => (
                lying(v3(-0.55, 0.03, 0.9), -0.4, rz(FRAC_PI_2)),
                held_shield,
            ),
            Kind::Fists => (held_weapon, held_shield),
        };
        let drop = step(0.15, 0.9, f);
        s.weapon = held_weapon.mix(w, drop);
        s.shield = held_shield.mix(sh, drop);
        s.draw = 0.0;
        s.bend = 0.0;
    }
    s
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
    block: Bone,
    /// The blade beating the blow aside (a parry).
    deflect: Bone,
    stagger: Bone,
}

impl Blade {
    fn new(low: Bone, guard: Bone, m: &Moves, block: Bone, deflect: Bone, stagger: Bone) -> Self {
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
            block,
            deflect,
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
            Act::Block { k, hit, parry } => {
                let up = self.guard.mix(self.block, k);
                if parry {
                    // Meeting the blow, then beating it aside.
                    let beat = step(-0.02, 0.05, hit) * (1.0 - step(0.12, 0.3, hit));
                    up.mix(self.deflect, beat * k)
                } else {
                    // Driven back and up as the blow lands.
                    let r = recoil(hit) * k;
                    Bone::new(up.pos + v3(0.0, 0.03, 0.07) * r, rx(0.25 * r) * up.rot)
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
/// from higher and further back), hanging parry overhead. Carried low in the right hand while
/// walking, the left arm swinging.
fn longsword(act: Act, chest: Bone, swing: f32, jiggle: Vec2) -> Arms {
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
    let w = Blade::new(
        blade(v3(0.08, 0.0, -0.27), v3(-0.1, -0.45, -0.88), Vec3::Y),
        blade(v3(0.05, 0.08, -0.3), v3(-0.06, 0.42, -0.9), Vec3::Y),
        &moves,
        blade(v3(0.06, 0.4, -0.33), v3(-1.0, 0.18, -0.12), Vec3::Y),
        blade(v3(0.16, 0.36, -0.36), v3(-0.45, 0.8, -0.4), Vec3::Y),
        blade(v3(0.3, 0.22, -0.14), v3(0.45, 0.8, -0.3), Vec3::Z),
    )
    .at(act, jiggle);
    let relaxed = act == Act::Relaxed;
    let w = if relaxed {
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
    if relaxed {
        pole[1] = chest.rot * v3(0.4, -0.6, 0.8);
    }
    Arms {
        weapon,
        shield: weapon,
        hand: [left, Hand::Grip(weapon.pos, weapon.rot)],
        pole,
        draw: 0.0,
        nocked: false,
    }
}

/// Arming sword and heater shield: the same diagonal cut one-handed, the shield pulled in to
/// the chest as the sword arm goes out; a block behind the raised shield, a parry punching it
/// out to meet the blow.
fn sword_and_shield(act: Act, chest: Bone, swing: f32, jiggle: Vec2) -> Arms {
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
    let w = Blade::new(
        blade(v3(0.25, -0.1, -0.16), v3(0.05, -0.4, -0.92), Vec3::Y),
        blade(v3(0.2, 0.28, -0.24), v3(-0.12, 0.32, -0.94), Vec3::Y),
        &moves,
        blade(
            v3(0.26, 0.3, -0.08),
            v3(0.05, 0.95, 0.3),
            v3(0.0, 0.0, -1.0),
        ),
        blade(
            v3(0.26, 0.3, -0.08),
            v3(0.05, 0.95, 0.3),
            v3(0.0, 0.0, -1.0),
        ),
        blade(v3(0.4, 0.15, -0.05), v3(0.6, 0.6, 0.2), Vec3::Z),
    )
    .at(act, jiggle);
    let face = |pos: Vec3, fwd: Vec3| Bone::new(pos, aim(fwd, Vec3::Y));
    let low = face(v3(-0.28, -0.14, -0.15), v3(-0.4, -0.05, -0.9));
    let guard = face(v3(-0.1, 0.2, -0.38), v3(0.1, 0.0, -1.0));
    let raised = face(v3(-0.05, 0.34, -0.42), v3(0.05, 0.15, -1.0));
    let punched = face(v3(-0.02, 0.32, -0.5), v3(0.0, 0.1, -1.0));
    let tucked = face(v3(-0.22, 0.16, -0.27), v3(-0.35, 0.05, -0.94));
    let thrown = face(v3(-0.42, 0.18, -0.15), v3(-0.8, 0.25, -0.5));
    let sh = match act {
        Act::Relaxed => swung(low, 0, 0.5 * swing),
        Act::Guard => Bone::new(guard.pos + v3(0.0, 0.008 * jiggle.y, 0.0), guard.rot),
        Act::Windup { p, heavy } => {
            // Kept toward the foe as the shoulders coil away from it, until the release.
            let keep =
                step(0.1, 0.6, p) * (1.0 - step(0.88, 1.0, p)) * if heavy { 0.6 } else { 0.4 };
            let held = guard.mix(tucked, 0.5 * step(0.85, 1.0, p));
            Bone::new(ry(keep) * held.pos, ry(keep) * held.rot)
        }
        Act::Strike { t, dur, .. } => guard.mix(
            tucked,
            (0.5 + 0.5 * step(0.0, 0.06, t)) * (1.0 - step(dur * 0.4, dur, t)),
        ),
        Act::Block { k, hit, parry } => {
            let up = guard.mix(raised, k);
            if parry {
                let bash = step(-0.03, 0.03, hit) * (1.0 - step(0.08, 0.25, hit));
                up.mix(punched, bash * k)
            } else {
                let r = recoil(hit) * k;
                Bone::new(up.pos + v3(0.0, 0.02, 0.08) * r, rx(0.2 * r) * up.rot)
            }
        }
        Act::Stagger { k, .. } => guard.mix(thrown, k),
    };
    let w = if act == Act::Relaxed {
        swung(w, 1, -0.6 * swing)
    } else {
        w
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
    }
}

fn rifle(act: Act, chest: Bone, head: Bone, aiming: f32) -> Arms {
    // Carried across the chest at port arms (knocked aside when staggered).
    let mut dir = v3(-0.55, 0.65, -0.3);
    if let Act::Stagger { k, .. } = act {
        dir = dir.lerp(v3(-0.3, 0.9, -0.35), k);
    }
    let low = chest.then(Bone::new(
        v3(0.15, 0.05, -0.22),
        aim(dir, v3(0.3, 0.3, -1.0)),
    ));
    // Shouldered: the sight line through the eye, straight ahead; kicks after a shot.
    let kick = match act {
        Act::Strike { t, .. } => (-t / 0.05).exp(),
        _ => 0.0,
    };
    let rot = aim(Vec3::NEG_Z, Vec3::Y) * rx(0.12 * kick);
    let eye = head.at(rig::EYES);
    let shouldered = Bone::new(
        eye - rot * weapons::RIFLE_EYE + v3(0.0, 0.0, 0.02 * kick),
        rot,
    );
    let weapon = low.mix(shouldered, aiming);
    Arms {
        weapon,
        shield: weapon,
        hand: [
            Hand::Grip(
                weapon.at(weapons::RIFLE_LEFT_GRIP),
                weapon.rot * weapons::rifle_left_hand(),
            ),
            Hand::Grip(weapon.pos, weapon.rot * weapons::rifle_hand()),
        ],
        pole: [
            chest.rot * v3(-0.3, -1.0, 0.1),
            chest.rot * v3(0.8, -0.6, 0.3),
        ],
        draw: 0.0,
        nocked: false,
    }
}

fn bow(act: Act, chest: Bone, head: Bone, aiming: f32, ready: bool, swing: f32) -> Arms {
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
    let anchor = head.at(v3(0.02, 0.06, -0.07));
    let left = chest.at(shoulder(0));
    let rise = anchor.y - left.y;
    let out = (0.55f32 * 0.55 - rise * rise).max(0.04).sqrt();
    let wrist = v3(anchor.x - 0.02, anchor.y, left.z - out);
    let drawn = Bone::new(wrist - rot * (weapons::BOW_HAND * grip(0)), rot);
    let weapon = rest.mix(drawn, aiming);
    let full =
        ((anchor - weapon.pos).dot(weapon.rot * Vec3::Z) - weapons::BOW_TIP.z).clamp(0.0, 0.62);
    let on_string = |d: f32| {
        let nock = weapon.at(v3(0.0, 0.0, weapons::BOW_TIP.z + d));
        Hand::Grip(nock, weapon.rot * weapons::BOW_HAND)
    };
    let (draw, nocked, right) = match act {
        Act::Windup { p, .. } => {
            let d = full * step(0.3, 0.9, p);
            (d, true, on_string(d))
        }
        Act::Strike { t, dur, .. } => {
            // Released: the hand flies back past the jaw, then returns to the string.
            let back = step(0.0, 0.08, t) * (1.0 - step(dur * 0.6, dur, t));
            let wrist = anchor + weapon.rot * v3(0.03, 0.02, 0.09 + 0.12 * back);
            (0.0, false, Hand::Free(wrist))
        }
        _ if ready => (0.0, true, on_string(0.0)),
        _ => (0.0, false, Hand::Swing),
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
            chest.rot * v3(0.6, 0.3, 1.0),
        ],
        draw,
        nocked,
    }
}

fn fists(act: Act, chest: Bone, ready: bool) -> Arms {
    let guard = [v3(-0.12, 0.36, -0.3), v3(0.1, 0.32, -0.24)];
    let hands = match act {
        Act::Windup { p, .. } => [guard[0], guard[1].lerp(v3(0.2, 0.3, 0.02), p)],
        Act::Strike { t, dur, .. } => {
            let k = step(0.0, 0.06, t) * (1.0 - step(dur * 0.5, dur, t));
            [guard[0], guard[1].lerp(v3(0.02, 0.4, -0.5), k)]
        }
        Act::Block { k, .. } => [
            guard[0].lerp(v3(-0.07, 0.48, -0.26), k),
            guard[1].lerp(v3(0.07, 0.48, -0.26), k),
        ],
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
    st: &mut StandInState,
    materials: &mut Vec<(Part, StandardMaterial)>,
    stand_in: &StandIn,
) -> impl Fn(Part) -> Placed + use<> {
    let kind = Kind::of(&weapon);
    let fighting = brain.is_some_and(|b| b.alert == AlertLevel::Combat);
    let is_down = vitals.is_some_and(|v| v.downed);
    // A pose with no past (just built, back from far away or from being down) takes the
    // states it finds as already under way.
    let m = &mut st.motion;
    let fresh = !m.posed || !(0.0..=0.5).contains(&(now - m.last)) || m.down != is_down;
    m.down = is_down;
    let mut act = if fighting { Act::Guard } else { Act::Relaxed };
    let mut windup = weapon.windup;
    let mut dodge = 0.0;
    let mut winding = false;
    let mut blade_glow: Option<StandardMaterial> = None;
    let mut parry = false;
    if let Some(c) = combat {
        match c.phase {
            AttackPhase::Windup { hit_at, heavy, .. } => {
                winding = true;
                m.heavy = heavy;
                let total = (weapon.windup * if heavy { 1.6 } else { 1.0 }).max(0.05);
                windup = total;
                let p = (1.0 - (hit_at - now) / total as f64).clamp(0.0, 1.0) as f32;
                act = Act::Windup { p, heavy };
                if heavy && weapon.style == WeaponStyle::Melee {
                    blade_glow = Some(lit(0xff7a1a, 12_000.0 * (0.4 + 0.6 * p)));
                }
            }
            AttackPhase::Recovery { until } if now < until => {
                let dur = weapon.recovery.max(0.05);
                let t = (now - (until - dur as f64)).max(0.0) as f32;
                if fresh {
                    m.heavy = false;
                }
                windup = weapon.windup * if m.heavy { 1.6 } else { 1.0 };
                act = Act::Strike {
                    t,
                    dur,
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
                }
                // The blow it meets: the last one landed, else the next to land.
                let lands = c.incoming.iter().map(|a| a.lands_at);
                let last = lands.clone().filter(|&t| t <= now).fold(f64::MIN, f64::max);
                let next = lands.filter(|&t| t > now).fold(f64::MAX, f64::min);
                if now - last < 0.4 {
                    m.guard.2 = last;
                } else if next < until + 0.05 {
                    m.guard.2 = next;
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
    // Muzzle flash right after a shot.
    let ranged = weapon.style == WeaponStyle::Ranged;
    if ranged && st.was_winding && !winding {
        st.flash_until = now + 0.07;
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
    // Downed: crumple backward over half a second.
    let fall = match downed {
        _ if !is_down => 0.0,
        Some(d) => ((now - d.since) / 0.5).clamp(0.0, 1.0) as f32,
        None => 1.0,
    };
    let fall = if is_down {
        (fall * fall * (3.0 - 2.0 * fall)).max(1e-3)
    } else {
        0.0
    };
    let seed = brain
        .and_then(|b| b.home)
        .map(|h| (h.x * 0.731 + h.z * 1.37).sin() * 10.0)
        .unwrap_or(0.0);
    let crouch = intent.is_some_and(|i| i.crouch) || loco.mode == LocomotionMode::Crouch;
    // The feet: knocked back, they stumble back under the body (and step up again after).
    let ready = (fighting || act != Act::Relaxed) && !is_down;
    let back = match act {
        Act::Stagger { k, .. } => 0.24 * k,
        _ => 0.0,
    };
    let dashing = loco.mode == LocomotionMode::Dash && !is_down;
    let m = &mut st.motion;
    if fresh {
        m.posed = false;
    }
    let (gait, feet, _) = m.walk(
        now,
        phase,
        facing,
        loco.velocity,
        kind,
        ready,
        dashing,
        Vec2::new(0.0, back),
    );
    let skeleton = solve(&Input {
        now,
        feet,
        gait,
        accel: m.accel,
        crouch,
        kind,
        act,
        windup,
        dodge,
        fighting,
        fall,
        seed,
    });
    let spin = (now * 53.0 % TAU as f64) as f32;
    let slots = place(&skeleton, kind, flash, spin);
    move |part| slot(part).map_or(HIDDEN, |i| slots[i])
}

/// Every part's pose.
fn place(s: &Skeleton, kind: Kind, flash: bool, spin: f32) -> [Placed; SLOTS] {
    let mut out = [HIDDEN; SLOTS];
    let mut put = |part: Part, b: Bone| {
        if let Some(i) = slot(part) {
            out[i] = b.placed();
        }
    };
    put(Part::Torso, s.chest);
    put(Part::Head, s.head);
    put(FACE, s.head);
    put(Part::Band, s.head);
    put(Part::Orb, s.head);
    put(PELVIS, s.pelvis);
    put(TABARD_FRONT, s.tabard[0]);
    put(TABARD_BACK, s.tabard[1]);
    put(Part::LegL, s.thigh[0]);
    put(Part::LegR, s.thigh[1]);
    put(SHIN_L, s.shin[0]);
    put(SHIN_R, s.shin[1]);
    put(FOOT_L, s.foot[0]);
    put(FOOT_R, s.foot[1]);
    put(Part::ArmL, s.upper[0]);
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
            fall: 0.0,
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
        for parry in [false, true] {
            for hit in [-0.1, 0.0, 0.03, 0.1, 0.25] {
                acts.push(Act::Block { k: 1.0, hit, parry });
            }
        }
        let mut worst = 0.0f32;
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            for &act in &acts {
                for crouch in [false, true] {
                    let mut i = input(kind, act, act != Act::Relaxed);
                    i.crouch = crouch;
                    let s = solve(&i);
                    let m = [0, 1].map(|k| (s.want[k] - s.hand[k].pos).length());
                    if m[0].max(m[1]) > 0.005 {
                        eprintln!("{kind:?} {act:?} crouch {crouch}: {:.3} {:.3}", m[0], m[1]);
                    }
                    worst = worst.max(m[0]).max(m[1]);
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
    }

    /// Walk a fighter through `secs` s at 60 Hz, `motion(t)` giving its velocity (in its own
    /// frame, x and z) and how fast it turns (rad/s), and watch its feet in the world.
    fn steps(kind: Kind, ready: bool, secs: f32, motion: impl Fn(f32) -> (Vec2, f32)) -> Steps {
        const GROUND: f32 = 0.006;
        let dt = 1.0 / 60.0;
        let act = if ready { Act::Guard } else { Act::Relaxed };
        let mut m = Motion::default();
        let (mut pos, mut yaw) = (Vec3::ZERO, 0.0f32);
        let mut last: Option<[[Vec3; 2]; 2]> = None;
        let mut out = Steps {
            went: 0.0,
            slid: 0.0,
            worst: 0.0,
            down: [0.0; 2],
            lifts: [0; 2],
            hips: (f32::MAX, f32::MIN),
        };
        let mut was_down = [true; 2];
        let n = (secs / dt).round() as usize;
        for f in 0..n {
            let t = f as f32 * dt;
            let (v, w) = motion(t);
            yaw += w * dt;
            let facing = ry(yaw);
            let vel = facing * Vec3::new(v.x, 0.0, v.y);
            if f > 0 {
                pos += vel * dt;
                out.went += vel.length() * dt;
            }
            let now = 1.0 + f64::from(t);
            let (gait, feet, _) = m.walk(now, 0.0, facing, vel, kind, ready, false, Vec2::ZERO);
            let s = solve(&Input {
                now,
                feet,
                gait,
                accel: m.accel,
                ..input(kind, act, ready)
            });
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
            last = Some(pts);
        }
        out
    }

    /// How a test walk goes: velocity (own frame) and turn rate at each moment.
    type Course<'a> = &'a dyn Fn(f32) -> (Vec2, f32);

    #[test]
    fn feet_do_not_slide() {
        let ramp = |t: f32| step(0.0, 0.6, t) * (1.0 - step(2.2, 2.8, t));
        let cases: [(&str, Kind, bool, f32, Course); 9] = [
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
}
