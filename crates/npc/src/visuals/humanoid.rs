//! Fighter bodies. With the renderer they are built from the procedural armored kit of
//! [`crate::art::fighter`]; without it, from a few capsules. Either way one skeleton is posed
//! every step from the AI state: the pelvis, chest and head are placed from the gait, stance
//! and action, then two-bone chains solve the legs (feet planted or swinging along the gait)
//! and the arms (hands on the weapon's grips), so knees and elbows bend.

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
/// in the team color, as worn as the fighter's skill says), a part from `PAINT0` on (in this
/// order) posed with its piece.
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

/// The part of a piece's painted plates.
fn paint_of(part: Part) -> Option<Part> {
    let i = PAINTED.iter().position(|&p| p == part)?;
    Some(Part::Fighter(PAINT0 + i as u8))
}

/// Where a part's pose is kept (painted plates: their piece's).
fn slot(part: Part) -> Option<usize> {
    if let Part::Fighter(n) = part
        && let Some(&piece) = PAINTED.get(n.wrapping_sub(PAINT0) as usize)
    {
        return slot(piece);
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
/// baked occlusion) but not the textures, `k` times as bright, plus a faint glow that shows in
/// shade but cannot blow it out to white in daylight. Painted plates (whose color is in their
/// texture) flash a pale warm grey.
fn flash(k: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [k, k * 0.96, k * 0.88, 1.0],
        perceptual_roughness: 0.6,
        ..Default::default()
    }
    .with_emissive([1.0, 0.94, 0.82], 500.0)
}

/// How much brighter a struck part's own colors get, and the albedo of its painted plates.
const FLASH: f32 = 1.5;
const FLASH_PAINT: f32 = 0.5;

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
        if let Some(plates) = paint_of(part).filter(|_| kit.painted(piece)) {
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
    /// After the blow or shot: seconds since it, and how long the recovery lasts.
    Strike { t: f32, dur: f32 },
    /// Guard raised against a blow; `k` eases back to the guard as it ends.
    Block { k: f32 },
    /// Knocked off balance (`k` eases in and out).
    Stagger { k: f32 },
}

/// Everything the pose depends on.
struct Input {
    now: f64,
    phase: f32,
    speed: f32,
    crouch: bool,
    kind: Kind,
    act: Act,
    /// Sidestep lean −1..1.
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

/// One foot along the gait at cycle `t` (0 = heel strike): how far forward of the hip, how
/// high, and its pitch (toe up positive).
fn gait(t: f32, beta: f32, half: f32, lift: f32, run: f32) -> (f32, f32, f32) {
    let heel = 0.3 * (1.0 - run);
    if t < beta {
        let u = t / beta;
        let pitch = heel * (1.0 - u / 0.2).max(0.0) - 0.55 * step(0.6, 1.0, u);
        (half * (1.0 - 2.0 * u), 0.0, pitch)
    } else {
        let u = (t - beta) / (1.0 - beta);
        let e = u * u * (3.0 - 2.0 * u);
        let pitch = -0.55 * (1.0 - step(0.0, 0.35, u)) + heel * step(0.7, 1.0, u);
        (-half + 2.0 * half * e, lift * (PI * u).sin(), pitch)
    }
}

/// Pose the skeleton.
fn solve(i: &Input) -> Skeleton {
    let mut s = Skeleton::default();
    let moving = step(0.05, 0.7, i.speed);
    let run = step(2.0, 4.5, i.speed);
    let down = i.fall > 0.0;
    let ready = (i.fighting || i.act != Act::Relaxed) && !down;
    let kneel = if i.crouch { 1.0 - moving } else { 0.0 };
    let still = 1.0 - moving;
    let st = stance(i.kind, ready);
    let breath = (i.now * TAU as f64 / 3.8 + i.seed * 7.0).sin() as f32;
    let sway = (i.now * TAU as f64 / 7.3 + i.seed * 13.0).sin() as f32;
    let glance = (i.now * 0.37 + i.seed * 5.0).sin() as f32;

    // ---------------------------------------------------------------- feet
    let beta = lerp(0.62, 0.38, run);
    let half = beta * 0.75;
    let lift = lerp(0.07, 0.17, run);
    let tl = (i.phase / TAU).rem_euclid(1.0);
    let mut feet = [Bone::default(); 2];
    for (k, foot) in feet.iter_mut().enumerate() {
        let sgn = if k == 0 { -1.0 } else { 1.0 };
        let (sx, sf, syaw) = st.feet[k];
        let t = (tl + 0.5 * k as f32).rem_euclid(1.0);
        let (gf, gl, gp) = gait(t, beta, half, lift, run);
        let x = lerp(sx, sgn * 0.1, moving);
        let f = lerp(sf, gf, moving);
        let yaw = lerp(syaw, -sgn * 0.08, moving);
        let (up, pitch) = (gl * moving, gp * moving);
        let ground = v3(x, up, -f);
        let rot_y = ry(yaw);
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
    let gait_yaw = -0.1 * (TAU * tl).cos() * moving;
    let mid = TAU * (tl - beta * 0.5);
    let bob = lerp(0.016, -0.03, run) * (2.0 * mid).cos() * moving;
    let weight = -0.022 * mid.cos() * moving * (1.0 - 0.6 * run)
        + sway * 0.028 * still * if ready { 0.3 } else { 1.0 };
    let mut hip_y = rig::HIP_Y - lerp(st.drop, 0.02 + 0.04 * run, moving) + bob;
    if i.crouch {
        hip_y = lerp(0.52, 0.68, moving);
    }
    hip_y -= 0.12 * i.dodge.abs();
    if let Act::Stagger { k } = i.act {
        hip_y -= 0.05 * k;
    }
    let yaw = lerp(st.yaw, gait_yaw, moving) * (1.0 - 0.5 * kneel) - 0.2 * kneel;
    let tilt = 0.03 + 0.08 * run + if i.crouch { 0.25 } else { 0.0 };
    let roll = weight * 1.4;
    let pelvis_rot = ry(yaw) * rx(-tilt) * rz(roll);
    let mut pelvis = Bone::new(v3(weight + 0.12 * i.dodge, hip_y, 0.0), pelvis_rot);
    // Sink the hips until both feet can be reached.
    let reach = (rig::THIGH + rig::SHIN) * 0.985;
    for (k, foot) in feet.iter().enumerate() {
        let sgn = if k == 0 { -1.0 } else { 1.0 };
        let hip = pelvis.at(v3(sgn * rig::HIP_X, 0.0, 0.0));
        let d = hip - foot.pos;
        let flat = Vec2::new(d.x, d.z).length().min(reach * 0.99);
        let top = foot.pos.y + (reach * reach - flat * flat).sqrt();
        if hip.y > top {
            pelvis.pos.y -= hip.y - top;
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
    let mut lean = lerp(st.lean, 0.06 + 0.16 * run, moving) + 0.12 * kneel - 0.012 * breath;
    let mut twist = lerp(st.twist, -gait_yaw * 1.6, moving);
    let mut chest_roll = -roll * 0.6 - 0.35 * i.dodge;
    let ranged = matches!(i.kind, Kind::Rifle | Kind::Bow);
    match i.act {
        Act::Windup { p, .. } if !ranged => {
            lean -= 0.08 * p;
            twist -= 0.3 * p;
        }
        Act::Windup { p, .. } if i.kind == Kind::Bow => {
            twist -= 0.25 * step(0.0, 0.4, p);
        }
        Act::Strike { t, dur } if !ranged => {
            let k = step(0.0, 0.1, t) * (1.0 - step(dur * 0.5, dur, t));
            lean += 0.18 * k;
            twist += 0.4 * k;
        }
        Act::Stagger { k } => {
            lean -= (0.3 + 0.05 * (i.now * 12.0).sin() as f32) * k;
            chest_roll += 0.1 * k * (i.now * 7.0).sin() as f32;
        }
        _ => {}
    }
    let waist = pelvis.at(rig::WAIST) + v3(0.0, 0.002 * breath, 0.0);
    let chest = Bone::new(waist, pelvis.rot * ry(twist) * rx(-lean) * rz(chest_roll));
    s.chest = chest;
    let aiming = if down {
        0.0
    } else {
        match (i.kind, i.act) {
            (Kind::Rifle, Act::Windup { .. } | Act::Strike { .. }) => 1.0,
            (Kind::Rifle, Act::Guard | Act::Block { .. }) => 1.0 - run,
            (Kind::Bow, Act::Windup { p, .. }) => step(0.0, 0.35, p),
            (Kind::Bow, Act::Strike { t, dur }) => 1.0 - step(dur * 0.6, dur, t),
            _ => 0.0,
        }
    };
    let net_yaw = yaw + twist;
    let look = if ready { 0.0 } else { 0.25 * glance * still };
    let head_yaw = -net_yaw * lerp(0.85, 1.0, aiming) + look;
    let mut head_pitch = -(tilt + lean) * 0.75;
    let mut head_roll = -chest_roll * 0.5;
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
    let mut arms = match i.kind {
        Kind::Longsword => longsword(i.act, chest),
        Kind::SwordShield => sword_and_shield(i.act, chest),
        Kind::Rifle => rifle(i.act, chest, head, aiming),
        Kind::Bow => bow(i.act, chest, head, aiming, ready),
        Kind::Fists => fists(i.act, chest, ready),
    };
    // Free arms swing against the legs (pumping when running), or fly out when falling.
    let swing = 0.35 * moving * (1.0 + run) * (TAU * tl).cos();
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

/// The key poses of a melee weapon in the chest frame.
struct Swing {
    /// Carried out of combat.
    low: Bone,
    guard: Bone,
    /// Telegraph peaks (light and heavy).
    high: Bone,
    heavy: Bone,
    /// End of the blow.
    follow: Bone,
    block: Bone,
    stagger: Bone,
}

impl Swing {
    fn at(&self, act: Act) -> Bone {
        match act {
            Act::Relaxed => self.low,
            Act::Guard => self.guard,
            Act::Windup { p, heavy } => {
                let e = 1.0 - (1.0 - p).powi(3);
                self.guard
                    .mix(if heavy { self.heavy } else { self.high }, e)
            }
            Act::Strike { t, dur } => self
                .high
                .mix(self.follow, step(0.0, 0.1, t))
                .mix(self.guard, step(dur * 0.55, dur, t)),
            Act::Block { k } => self.guard.mix(self.block, k),
            Act::Stagger { k } => self.guard.mix(self.stagger, k),
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

fn longsword(act: Act, chest: Bone) -> Arms {
    let w = Swing {
        low: blade(v3(0.08, 0.0, -0.27), v3(-0.1, -0.45, -0.88), Vec3::Y),
        guard: blade(v3(0.05, 0.08, -0.3), v3(-0.06, 0.42, -0.9), Vec3::Y),
        high: blade(
            v3(0.14, 0.5, -0.06),
            v3(0.05, 0.75, 0.66),
            v3(0.0, 0.3, -1.0),
        ),
        heavy: blade(
            v3(0.13, 0.52, 0.02),
            v3(0.0, 0.25, 0.97),
            v3(0.0, 1.0, -0.2),
        ),
        follow: blade(
            v3(-0.09, 0.0, -0.31),
            v3(-0.5, -0.6, -0.62),
            v3(0.0, 0.5, -1.0),
        ),
        block: blade(v3(0.06, 0.4, -0.33), v3(-1.0, 0.18, -0.12), Vec3::Y),
        stagger: blade(v3(0.3, 0.22, -0.14), v3(0.45, 0.8, -0.3), Vec3::Z),
    }
    .at(act);
    let weapon = chest.then(w);
    let left = if matches!(act, Act::Stagger { k } if k > 0.3) {
        Hand::Free(chest.at(v3(-0.45, 0.2, -0.08)))
    } else {
        Hand::Grip(weapon.at(weapons::LONGSWORD_LEFT_GRIP), weapon.rot)
    };
    Arms {
        weapon,
        shield: weapon,
        hand: [left, Hand::Grip(weapon.pos, weapon.rot)],
        pole: melee_poles(chest, w.pos.y),
        draw: 0.0,
        nocked: false,
    }
}

fn sword_and_shield(act: Act, chest: Bone) -> Arms {
    let w = Swing {
        low: blade(v3(0.25, -0.1, -0.16), v3(0.05, -0.4, -0.92), Vec3::Y),
        guard: blade(v3(0.2, 0.28, -0.24), v3(-0.12, 0.32, -0.94), Vec3::Y),
        high: blade(v3(0.22, 0.52, 0.0), v3(0.1, 0.6, 0.8), v3(0.0, 0.3, -1.0)),
        heavy: blade(v3(0.2, 0.55, 0.06), v3(0.05, 0.2, 0.98), Vec3::Y),
        follow: blade(
            v3(-0.01, 0.07, -0.37),
            v3(-0.55, -0.45, -0.7),
            v3(0.0, 0.5, -1.0),
        ),
        block: blade(
            v3(0.26, 0.3, -0.08),
            v3(0.05, 0.95, 0.3),
            v3(0.0, 0.0, -1.0),
        ),
        stagger: blade(v3(0.4, 0.15, -0.05), v3(0.6, 0.6, 0.2), Vec3::Z),
    }
    .at(act);
    let face = |pos: Vec3, fwd: Vec3| Bone::new(pos, aim(fwd, Vec3::Y));
    let low = face(v3(-0.28, -0.14, -0.15), v3(-0.4, -0.05, -0.9));
    let guard = face(v3(-0.1, 0.2, -0.38), v3(0.1, 0.0, -1.0));
    let raised = face(v3(-0.05, 0.34, -0.42), v3(0.05, 0.15, -1.0));
    let thrown = face(v3(-0.42, 0.18, -0.15), v3(-0.8, 0.25, -0.5));
    let sh = match act {
        Act::Relaxed => low,
        Act::Guard | Act::Windup { .. } => guard,
        Act::Strike { t, dur } => guard.mix(
            low,
            0.4 * step(0.0, 0.1, t) * (1.0 - step(dur * 0.5, dur, t)),
        ),
        Act::Block { k } => guard.mix(raised, k),
        Act::Stagger { k } => guard.mix(thrown, k),
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
    if let Act::Stagger { k } = act {
        dir = dir.lerp(v3(0.3, 0.8, -0.3), k);
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

fn bow(act: Act, chest: Bone, head: Bone, aiming: f32, ready: bool) -> Arms {
    // Carried low in the left hand, or held ready in front with an arrow on the string.
    let carried = chest.then(Bone::new(
        v3(-0.27, -0.22, -0.08),
        aim(v3(0.05, -0.87, -0.5), v3(0.0, 0.5, -0.87)),
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
        Act::Strike { t, dur } => {
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
        Act::Strike { t, dur } => {
            let k = step(0.0, 0.06, t) * (1.0 - step(dur * 0.5, dur, t));
            [guard[0], guard[1].lerp(v3(0.02, 0.4, -0.5), k)]
        }
        Act::Block { k } => [
            guard[0].lerp(v3(-0.07, 0.48, -0.26), k),
            guard[1].lerp(v3(0.07, 0.48, -0.26), k),
        ],
        Act::Stagger { k } => [
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
    speed: f32,
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
    let mut act = if fighting { Act::Guard } else { Act::Relaxed };
    let mut dodge = 0.0;
    let mut winding = false;
    let mut blade_glow: Option<StandardMaterial> = None;
    let mut parry = false;
    if let Some(c) = combat {
        match c.phase {
            AttackPhase::Windup { hit_at, heavy, .. } => {
                winding = true;
                let total = (weapon.windup * if heavy { 1.6 } else { 1.0 }).max(0.05) as f64;
                let p = (1.0 - (hit_at - now) / total).clamp(0.0, 1.0) as f32;
                act = Act::Windup { p, heavy };
                if heavy && weapon.style == WeaponStyle::Melee {
                    blade_glow = Some(lit(0xff7a1a, 12_000.0 * (0.4 + 0.6 * p)));
                }
            }
            AttackPhase::Recovery { until } if now < until => {
                let dur = weapon.recovery.max(0.05);
                let t = (now - (until - dur as f64)).max(0.0) as f32;
                act = Act::Strike { t, dur };
            }
            AttackPhase::Staggered { until } if now < until => {
                let dur = vitals.map(|v| v.stagger_secs).unwrap_or(1.0).max(0.2);
                let left = (until - now) as f32;
                let k = step(0.0, 0.12, dur - left).min(step(0.0, 0.25, left));
                act = Act::Stagger { k };
            }
            _ => {}
        }
        match c.defense {
            Defense::Block { until } | Defense::Parry { until }
                if now < until && !matches!(act, Act::Stagger { .. }) =>
            {
                act = Act::Block {
                    k: step(0.0, 0.1, (until - now) as f32),
                };
                parry = matches!(c.defense, Defense::Parry { .. });
            }
            Defense::Dodge { until } if now < until => {
                let side = if loco.velocity.x < 0.0 { -1.0 } else { 1.0 };
                dodge = side * step(0.0, 0.12, (until - now) as f32);
            }
            _ => {}
        }
        // A flash when hit.
        if c.last_hurt > 0.0 && now - c.last_hurt < 0.12 {
            for part in ARMOR {
                materials.push((part, flash(FLASH)));
            }
            for part in ARMOR.into_iter().filter_map(paint_of) {
                materials.push((part, flash(FLASH_PAINT)));
            }
        }
    }
    if parry {
        blade_glow = Some(lit(0xffffff, 25_000.0));
        materials.push((Part::Shield, flash(FLASH)));
        if let Some(plates) = paint_of(Part::Shield) {
            materials.push((plates, flash(FLASH_PAINT)));
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
    let skeleton = solve(&Input {
        now,
        phase,
        speed,
        crouch,
        kind,
        act,
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
        Input {
            now: 1.0,
            phase: 0.0,
            speed: 0.0,
            crouch: false,
            kind,
            act,
            dodge: 0.0,
            fighting,
            fall: 0.0,
            seed: 0.0,
        }
    }

    #[test]
    fn hands_reach_their_grips() {
        let acts = [
            Act::Relaxed,
            Act::Guard,
            Act::Windup {
                p: 0.5,
                heavy: false,
            },
            Act::Windup {
                p: 1.0,
                heavy: false,
            },
            Act::Windup {
                p: 1.0,
                heavy: true,
            },
            Act::Strike { t: 0.05, dur: 0.45 },
            Act::Strike { t: 0.2, dur: 0.45 },
            Act::Block { k: 1.0 },
        ];
        let mut worst = 0.0f32;
        for kind in [Kind::Longsword, Kind::SwordShield, Kind::Rifle, Kind::Bow] {
            for act in acts {
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
                let mut i = input(Kind::Longsword, Act::Relaxed, false);
                i.speed = speed;
                i.phase = k as f32 / 16.0 * TAU;
                let s = solve(&i);
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
}
