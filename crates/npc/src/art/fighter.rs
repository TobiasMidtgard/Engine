//! Procedural fighters: near-future arena combatants. Padded undersuits under segmented,
//! painted armor plates with rolled rims, leather belts and straps, team tabards, open helmets
//! with a horsehair crest and a brow light over mail coifs, sculpted faces (the skin tone
//! varies between fighters), gloved fists, boots, and their weapons (a two-handed longsword,
//! an arming sword with a steel-rimmed heater shield, a carbine with an optic, and a composite
//! recurve bow).
//!
//! Every mesh is built from one of three tools: signed-distance sculpting meshed with surface
//! nets for the soft, organic parts (undersuit, gloves, boots, face, coif, crest), [`Plate`]s
//! for hard armor (a thick panel laid over a body surface, outlined by a 2D distance field,
//! with a rolled rim and relief), and lathes, sweeps and extrusions for weapons. All pieces
//! share one texture atlas ([`Zone`] tiles: paint, padded suit, cloth, leather, metal, polymer,
//! glass, wood, mail, skin, hair); vertex colors carry the tint of each sub-part and baked
//! ambient occlusion, and team colors are applied to a copy of a mesh ([`Kit::mesh`]), so all
//! fighters of a team share one mesh per piece.
//!
//! Each piece is modeled in its joint's frame (see [`rig`]): limbs hang along −Y from the
//! joint at the origin, the body faces −Z, the right side is +X; left limbs mirror the right.
//! The kit is built once per process ([`kit`]), on all cores, in well under two seconds.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine_core::prelude::*;
use engine_render::{Image, Mesh, Vertex};

use super::noise::{fbm2, perlin2, ridged2, worley2};
use super::sdf::{self, Grid, capsule, ellipsoid, round_box, round_cone, smax, smin, sphere, ssub};
use super::tex::{Texel, TextureSet, mix, smooth};

// ====================================================================== skeleton

/// The fighter's skeleton in its rest pose: meters, feet on the ground, facing −Z, right side
/// +X. Pieces are modeled in their joint's frame and the pose places the joints.
pub mod rig {
    use engine_core::prelude::Vec3;

    /// Hip joint height above the ground.
    pub const HIP_Y: f32 = THIGH + SHIN + ANKLE_Y;
    /// Hip joints are this far left and right of the pelvis center.
    pub const HIP_X: f32 = 0.095;
    /// Hip to knee.
    pub const THIGH: f32 = 0.45;
    /// Knee to ankle.
    pub const SHIN: f32 = 0.44;
    /// Ankle joint height above the sole.
    pub const ANKLE_Y: f32 = 0.075;
    /// Waist joint (the chest piece's origin) from the pelvis origin (between the hips).
    pub const WAIST: Vec3 = Vec3::new(0.0, 0.12, 0.01);
    /// Right shoulder joint from the waist joint.
    pub const SHOULDER: Vec3 = Vec3::new(0.185, 0.345, 0.025);
    /// Neck joint (the head piece's origin) from the waist joint.
    pub const NECK: Vec3 = Vec3::new(0.0, 0.43, 0.02);
    /// Shoulder to elbow.
    pub const UPPER_ARM: f32 = 0.30;
    /// Elbow to wrist.
    pub const FOREARM: f32 = 0.26;
    /// Center of a gripped handle in the right hand's frame (wrist at the origin, fingers
    /// down −Y, palm toward −X). The handle runs along Z, the index finger toward −Z. The left
    /// hand mirrors x.
    pub const GRIP: Vec3 = Vec3::new(-0.028, -0.088, 0.0);
    /// The right eye (the one that sights along a shouldered carbine) from the neck joint.
    pub const EYES: Vec3 = Vec3::new(0.031, 0.166, -0.081);
}

/// Where hands, string and muzzle sit on each weapon, in the weapon's own frame (the frame its
/// mesh is modeled in). A hand holding a weapon takes the weapon's rotation times the grip
/// rotation, and its grip center ([`rig::GRIP`]) sits on the grip point.
pub mod weapons {
    use std::f32::consts::FRAC_1_SQRT_2;

    use engine_core::prelude::{Quat, Vec3};

    /// Longsword: the right hand's grip center is the origin, the blade points −Z, the edges
    /// face ±Y, the flats ±X. The left hand holds the grip below the right, near the pommel.
    /// Both hands use the weapon's rotation.
    pub const LONGSWORD_LEFT_GRIP: Vec3 = Vec3::new(0.0, 0.0, 0.115);
    /// Longsword blade tip.
    pub const LONGSWORD_TIP: Vec3 = Vec3::new(0.0, 0.0, -1.06);
    /// Arming sword (one hand): grip at the origin, tip.
    pub const ARMING_TIP: Vec3 = Vec3::new(0.0, 0.0, -0.84);
    /// Shield: the left hand's grip (a vertical handle behind the boss) is the origin; the
    /// face looks −Z, up is +Y.
    pub const SHIELD_HAND: Quat = Quat::from_xyzw(FRAC_1_SQRT_2, 0.0, 0.0, FRAC_1_SQRT_2);
    /// Carbine: barrel along −Z, up +Y; the origin is the right hand's grip center on the
    /// pistol grip.
    pub const RIFLE_MUZZLE: Vec3 = Vec3::new(0.0, 0.1, -0.708);
    /// The middle of the butt pad.
    pub const RIFLE_BUTT: Vec3 = Vec3::new(0.0, 0.07, 0.29);
    /// The left (support) hand's grip center: on the handguard's axis.
    pub const RIFLE_LEFT_GRIP: Vec3 = Vec3::new(0.0, 0.1, -0.26);
    /// Where the shooter's eye goes, behind the optic.
    pub const RIFLE_EYE: Vec3 = Vec3::new(0.0, 0.16, 0.2);
    /// Bow: the left hand's grip is the origin, the bow stands along Y, the archer is toward
    /// +Z. The upper limb hinges here (the lower mirrors y).
    pub const BOW_LIMB_PIVOT: Vec3 = Vec3::new(0.0, 0.17, 0.025);
    /// Where the string leaves the upper limb of an undrawn bow (the lower mirrors y).
    pub const BOW_TIP: Vec3 = Vec3::new(0.0, 0.6516, 0.19);
    /// The bow hand's rotation (also used to hold the string).
    pub const BOW_HAND: Quat = Quat::from_xyzw(FRAC_1_SQRT_2, 0.0, 0.0, FRAC_1_SQRT_2);
    /// Arrow length, nock to point.
    pub const ARROW: f32 = 0.78;

    /// The right hand on the pistol grip (raked 20°): fingers forward, palm left.
    pub fn rifle_hand() -> Quat {
        let (s, c) = 20f32.to_radians().sin_cos();
        super::basis(Vec3::X, Vec3::new(0.0, s, c), Vec3::new(0.0, -c, s))
    }

    /// The left hand under the handguard, palm up and to the right.
    pub fn rifle_left_hand() -> Quat {
        let x = Vec3::new(0.5, 0.866, 0.0);
        let z = Vec3::Z;
        super::basis(x, z.cross(x), z)
    }
}

/// The rotation whose columns are the given axes (they must be orthonormal).
pub fn basis(x: Vec3, y: Vec3, z: Vec3) -> Quat {
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

// ====================================================================== texture atlas

/// Material tiles of the fighter atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Satin-painted composite armor (white; tinted per vertex).
    Paint,
    /// Padded, quilted technical fabric of the undersuit.
    Suit,
    /// Heavy woven tabard cloth.
    Cloth,
    Leather,
    /// Brushed and scratched steel (metallic).
    Metal,
    /// Matte, stippled polymer and rubber.
    Polymer,
    /// Smoked visor and optic glass.
    Glass,
    /// Laminated wood.
    Wood,
    /// Riveted steel rings (the coif).
    Mail,
    Skin,
    /// Coarse horsehair (the crest, eyebrows).
    Hair,
}

impl Zone {
    pub const ALL: [Zone; 11] = [
        Zone::Paint,
        Zone::Suit,
        Zone::Cloth,
        Zone::Leather,
        Zone::Metal,
        Zone::Polymer,
        Zone::Glass,
        Zone::Wood,
        Zone::Mail,
        Zone::Skin,
        Zone::Hair,
    ];
}

/// Atlas layout: tiles of `TILE`² pixels in `ATLAS_COLS` × `ATLAS_ROWS`.
pub const ATLAS_COLS: u32 = 4;
pub const ATLAS_ROWS: u32 = 3;
pub const TILE: u32 = 256;
/// Unused pixels around each tile's content (keeps mip levels from bleeding).
const GUTTER: f32 = 10.0;
/// Meters of surface one tile spans; bigger parts are squeezed to fit.
pub const TILE_M: f32 = 0.5;

/// Atlas UV of a point `m` (meters, 0..`span`) of a part in `zone`.
fn tile_uv(zone: Zone, m: Vec2, span: f32) -> Vec2 {
    let i = zone as u32;
    let cell = Vec2::new((i % ATLAS_COLS) as f32, (i / ATLAS_COLS) as f32);
    let usable = TILE as f32 - 2.0 * GUTTER;
    let px =
        cell * TILE as f32 + Vec2::splat(GUTTER) + (m / span).clamp(Vec2::ZERO, Vec2::ONE) * usable;
    px / Vec2::new((ATLAS_COLS * TILE) as f32, (ATLAS_ROWS * TILE) as f32)
}

fn lum(v: f32) -> Vec3 {
    Vec3::splat(v)
}

/// Thin scratches running in random directions per patch: 0..1.
fn scratches(p: Vec2, density: f32, seed: u32) -> f32 {
    let (_, _, cell) = worley2(p * 7.0, 0, seed);
    let a = cell * TAU;
    let dir = Vec2::new(a.cos(), a.sin());
    let q = Vec2::new(p.dot(dir), p.perp_dot(dir));
    let s = ridged2(q * Vec2::new(14.0, 210.0), 0, 2, seed + 1);
    let mask = smooth(
        1.0 - density,
        1.0,
        fbm2(p * 11.0, 0, 2, 0.5, seed + 2) * 0.5 + 0.5,
    );
    smooth(0.9, 0.985, s) * mask
}

fn texel(zone: Zone, p: Vec2) -> Texel {
    match zone {
        Zone::Paint => {
            let mottle = fbm2(p * 5.0, 0, 3, 0.5, 11) * 0.5 + 0.5;
            let peel = perlin2(p * 160.0, 0, 12);
            let scratch = scratches(p, 0.45, 13);
            let chip_n = fbm2(p * 18.0, 0, 3, 0.55, 16) * 0.5 + 0.5;
            let chip = smooth(0.79, 0.81, chip_n * (0.8 + 0.4 * mottle));
            let rim = smooth(0.76, 0.79, chip_n * (0.8 + 0.4 * mottle)) - chip;
            let albedo = mix(lum(0.8 + 0.06 * (mottle - 0.5)), lum(0.95), scratch * 0.5);
            let albedo = mix(albedo, lum(0.95), rim * 0.6);
            Texel {
                albedo: mix(albedo, lum(0.1), chip),
                height: peel * 0.000_012 - chip * 0.000_12 - scratch * 0.000_03,
                roughness: 0.34 + 0.14 * mottle + 0.18 * scratch + 0.35 * chip,
                metallic: 0.0,
                occlusion: 1.0 - chip * 0.25,
            }
        }
        Zone::Suit => {
            // Diamond quilting (4.5 cm), ripstop grid (6 mm) and wear.
            let q = p / 0.045;
            let a = (q.x + q.y).rem_euclid(1.0) - 0.5;
            let b = (q.x - q.y).rem_euclid(1.0) - 0.5;
            let seam = a.abs().max(b.abs()) * 2.0; // 1 at seams, 0 in pad centers
            let puff = 1.0 - seam * seam;
            let stitch = smooth(0.9, 1.0, seam);
            let r = p / 0.006;
            let rip = smooth(0.85, 1.0, (r.x.rem_euclid(1.0) - 0.5).abs() * 2.0).max(smooth(
                0.85,
                1.0,
                (r.y.rem_euclid(1.0) - 0.5).abs() * 2.0,
            ));
            let n = fbm2(p * 9.0, 0, 3, 0.5, 21) * 0.5 + 0.5;
            let fuzz = perlin2(p * 300.0, 0, 22);
            Texel {
                albedo: lum(0.62 + 0.1 * n - 0.18 * stitch + 0.03 * fuzz),
                height: puff * 0.0018 + rip * 0.000_08 + fuzz * 0.000_02,
                roughness: 0.82 + 0.1 * n,
                metallic: 0.0,
                occlusion: 1.0 - 0.45 * stitch,
            }
        }
        Zone::Cloth => {
            // Coarse twill with slubs and a little fading.
            let t = ((p.x + p.y) / 0.004 * TAU).sin();
            let warp = (p.x / 0.0025 * TAU).sin();
            let slub = fbm2(p * Vec2::new(4.0, 60.0), 0, 3, 0.5, 31) * 0.5 + 0.5;
            let fade = fbm2(p * 4.0, 0, 3, 0.5, 32) * 0.5 + 0.5;
            Texel {
                albedo: lum(0.6 + 0.12 * slub + 0.12 * fade + 0.04 * t),
                height: t * 0.000_15 + warp * 0.000_05 + slub * 0.000_1,
                roughness: 0.9 + 0.08 * slub,
                metallic: 0.0,
                occlusion: 0.92 + 0.08 * t,
            }
        }
        Zone::Leather => {
            let (d1, d2, _) = worley2(p / 0.0045, 0, 41);
            let grain = smooth(0.0, 0.35, d2 - d1);
            let crease = ridged2(p * Vec2::new(9.0, 22.0), 0, 3, 42);
            let crease = smooth(0.72, 0.95, crease);
            let mottle = fbm2(p * 6.0, 0, 3, 0.5, 43) * 0.5 + 0.5;
            let scuff = smooth(0.62, 0.8, fbm2(p * 14.0, 0, 3, 0.6, 44) * 0.5 + 0.5);
            Texel {
                albedo: lum(0.55 + 0.15 * mottle - 0.14 * crease + 0.12 * scuff),
                height: grain * 0.000_08 - crease * 0.000_25,
                roughness: 0.5 + 0.18 * mottle + 0.15 * crease + 0.12 * scuff,
                metallic: 0.0,
                occlusion: 1.0 - 0.3 * crease,
            }
        }
        Zone::Metal => {
            // Brushed along u, scratched, with darker oxidized patches.
            let brush = fbm2(p * Vec2::new(3.0, 260.0), 0, 3, 0.6, 51);
            let scratch = scratches(p, 0.6, 52);
            let patina = smooth(0.55, 0.85, fbm2(p * 6.0, 0, 4, 0.55, 53) * 0.5 + 0.5);
            let pit = smooth(0.82, 0.9, fbm2(p * 70.0, 0, 2, 0.5, 54) * 0.5 + 0.5) * patina;
            Texel {
                albedo: lum(0.88 + 0.05 * brush + 0.06 * scratch - 0.3 * patina - 0.2 * pit),
                height: brush * 0.000_01 - scratch * 0.000_02 - pit * 0.000_05,
                roughness: 0.24 + 0.06 * brush - 0.08 * scratch + 0.28 * patina + 0.2 * pit,
                metallic: 1.0 - 0.3 * pit,
                occlusion: 1.0 - 0.3 * pit,
            }
        }
        Zone::Polymer => {
            let (d1, _, id) = worley2(p / 0.003, 0, 61);
            let dimple = smooth(0.45, 0.0, d1) * (0.5 + 0.5 * id);
            let n = fbm2(p * 8.0, 0, 3, 0.5, 62) * 0.5 + 0.5;
            let scuff = scratches(p, 0.35, 63);
            Texel {
                albedo: lum(0.7 + 0.08 * n + 0.2 * scuff),
                height: -dimple * 0.000_05,
                roughness: 0.58 + 0.12 * n + 0.15 * scuff,
                metallic: 0.0,
                occlusion: 1.0,
            }
        }
        Zone::Glass => {
            let smudge = fbm2(p * 7.0, 0, 3, 0.5, 71) * 0.5 + 0.5;
            let scratch = scratches(p, 0.25, 72);
            Texel {
                albedo: lum(0.9),
                height: 0.0,
                roughness: 0.05 + 0.08 * smooth(0.5, 0.9, smudge) + 0.2 * scratch,
                metallic: 0.0,
                occlusion: 1.0,
            }
        }
        Zone::Wood => {
            let w = fbm2(p * Vec2::new(30.0, 1.5), 0, 3, 0.5, 81);
            let rings = ((p.x * 160.0 + w * 6.0) * PI).sin() * 0.5 + 0.5;
            let fine = fbm2(p * Vec2::new(300.0, 8.0), 0, 2, 0.5, 82);
            Texel {
                albedo: lum(0.55 + 0.25 * rings + 0.06 * fine),
                height: rings * 0.000_06 + fine * 0.000_02,
                roughness: 0.48 + 0.12 * rings,
                metallic: 0.0,
                occlusion: 1.0,
            }
        }
        Zone::Mail => {
            // Rows of interlinked rings (10 mm across, every other row offset by half).
            let (pitch, row) = (0.013, 0.0098);
            let j = (p.y / row).floor();
            let shift = if j.rem_euclid(2.0) == 0.0 { 0.0 } else { 0.5 };
            let c = Vec2::new(
                ((p.x / pitch + shift).floor() + 0.5 - shift) * pitch,
                (j + 0.5) * row,
            );
            // Each ring overlaps its neighbors: take the nearest of the three below and beside.
            let mut ring: f32 = 0.0;
            for d in [
                v2(0.0, 0.0),
                v2(pitch, 0.0),
                v2(-pitch, 0.0),
                v2(pitch * 0.5, row),
                v2(-pitch * 0.5, row),
                v2(pitch * 0.5, -row),
                v2(-pitch * 0.5, -row),
            ] {
                let r = (p - c - d).length() / (pitch * 0.5);
                ring = ring.max(1.0 - ((r - 0.72) / 0.22).powi(2));
            }
            let ring = ring.max(0.0);
            let rust = smooth(0.6, 0.9, fbm2(p * 7.0, 0, 3, 0.5, 101) * 0.5 + 0.5);
            Texel {
                albedo: lum(0.1 + 0.6 * ring - 0.25 * rust * ring),
                height: ring * 0.0008,
                roughness: 0.38 + 0.3 * (1.0 - ring) + 0.25 * rust,
                metallic: ring * (1.0 - 0.5 * rust),
                occlusion: 0.35 + 0.65 * ring,
            }
        }
        Zone::Skin => {
            // Even skin with faint blotches, a little redness and fine bumps.
            let blotch = fbm2(p * 18.0, 0, 3, 0.5, 111) * 0.5 + 0.5;
            let red = smooth(0.45, 0.8, fbm2(p * 6.0, 0, 3, 0.5, 112) * 0.5 + 0.5);
            let bump = fbm2(p * 260.0, 0, 2, 0.5, 113);
            let base = Vec3::new(0.96, 0.92, 0.9) * (0.92 + 0.08 * blotch);
            Texel {
                albedo: mix(base, Vec3::new(0.97, 0.82, 0.8), red * 0.35),
                height: bump * 0.000_02 + blotch * 0.000_03,
                roughness: 0.68 + 0.1 * blotch,
                metallic: 0.0,
                occlusion: 1.0,
            }
        }
        Zone::Hair => {
            // Clumps of coarse strands running along v.
            let clump = fbm2(p * Vec2::new(30.0, 1.5), 0, 3, 0.5, 121) * 0.5 + 0.5;
            let strand = fbm2(p * Vec2::new(170.0, 5.0), 0, 2, 0.5, 122) * 0.5 + 0.5;
            let s = smooth(0.3, 0.7, strand) * (0.6 + 0.4 * clump);
            Texel {
                albedo: lum(0.45 + 0.5 * s),
                height: s * 0.0003 + clump * 0.0003,
                roughness: 0.42 + 0.25 * (1.0 - s),
                metallic: 0.0,
                occlusion: 0.7 + 0.3 * s,
            }
        }
    }
}

fn srgb8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// One baked tile: albedo, normal and ORM pixels (RGBA8, row-major).
struct BakedTile {
    zone: Zone,
    maps: [Vec<u8>; 3],
}

fn bake_tile(zone: Zone) -> BakedTile {
    let n = TILE as usize;
    let usable = TILE as f32 - 2.0 * GUTTER;
    let texel_m = TILE_M / usable;
    let mut texels = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let m = (Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - Vec2::splat(GUTTER)) * texel_m;
            texels.push(texel(zone, m));
        }
    }
    let h = |x: i64, y: i64| {
        let (x, y) = (
            x.clamp(0, n as i64 - 1) as usize,
            y.clamp(0, n as i64 - 1) as usize,
        );
        texels[y * n + x].height
    };
    let mut maps = [
        Vec::with_capacity(n * n * 4),
        Vec::with_capacity(n * n * 4),
        Vec::with_capacity(n * n * 4),
    ];
    for y in 0..n as i64 {
        for x in 0..n as i64 {
            let t = texels[y as usize * n + x as usize];
            maps[0].extend_from_slice(&[
                srgb8(t.albedo.x),
                srgb8(t.albedo.y),
                srgb8(t.albedo.z),
                255,
            ]);
            let dx = (h(x + 1, y - 1) + 2.0 * h(x + 1, y) + h(x + 1, y + 1)
                - h(x - 1, y - 1)
                - 2.0 * h(x - 1, y)
                - h(x - 1, y + 1))
                / (8.0 * texel_m);
            let dy = (h(x - 1, y + 1) + 2.0 * h(x, y + 1) + h(x + 1, y + 1)
                - h(x - 1, y - 1)
                - 2.0 * h(x, y - 1)
                - h(x + 1, y - 1))
                / (8.0 * texel_m);
            let nrm = Vec3::new(-dx, dy, 1.0).normalize();
            maps[1].extend_from_slice(&[
                unorm8(nrm.x * 0.5 + 0.5),
                unorm8(nrm.y * 0.5 + 0.5),
                unorm8(nrm.z * 0.5 + 0.5),
                255,
            ]);
            maps[2].extend_from_slice(&[
                unorm8(t.occlusion),
                unorm8(t.roughness),
                unorm8(t.metallic),
                255,
            ]);
        }
    }
    BakedTile { zone, maps }
}

fn assemble_atlas(tiles: &[BakedTile]) -> TextureSet {
    let (w, h) = ((ATLAS_COLS * TILE) as usize, (ATLAS_ROWS * TILE) as usize);
    let mut data = [
        vec![0u8; w * h * 4],
        vec![0u8; w * h * 4],
        vec![0u8; w * h * 4],
    ];
    for t in tiles {
        let i = t.zone as usize;
        let (x0, y0) = (
            (i % ATLAS_COLS as usize) * TILE as usize,
            (i / ATLAS_COLS as usize) * TILE as usize,
        );
        let row = TILE as usize * 4;
        for (k, map) in t.maps.iter().enumerate() {
            for y in 0..TILE as usize {
                let dst = ((y0 + y) * w + x0) * 4;
                data[k][dst..dst + row].copy_from_slice(&map[y * row..(y + 1) * row]);
            }
        }
    }
    let [a, n, o] = data;
    let mut set = TextureSet {
        albedo: Image::new(w as u32, h as u32, a, true),
        normal: Image::new(w as u32, h as u32, n, false),
        orm: Image::new(w as u32, h as u32, o, false),
    };
    set.albedo.generate_mips();
    set.normal.generate_mips();
    set.orm.generate_mips();
    set
}

// ====================================================================== mesh tools

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn v2(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

/// Unit gradient of a distance field.
fn grad(f: &dyn Fn(Vec3) -> f32, p: Vec3) -> Vec3 {
    let e = 4e-4;
    Vec3::new(
        f(p + Vec3::X * e) - f(p - Vec3::X * e),
        f(p + Vec3::Y * e) - f(p - Vec3::Y * e),
        f(p + Vec3::Z * e) - f(p - Vec3::Z * e),
    )
    .normalize_or(Vec3::Y)
}

/// Mesh the zero surface of `f` inside a box with surface nets. Far from the surface a coarse
/// grid four voxels apart answers instead of `f`, which makes big boxes cheap.
fn sdf_mesh(f: &dyn Fn(Vec3) -> f32, min: Vec3, max: Vec3, voxel: f32) -> Mesh {
    sdf_mesh_tol(f, min, max, voxel, voxel * DECIMATE)
}

/// [`sdf_mesh`], simplified until the surface has moved about `tolerance` meters.
fn sdf_mesh_tol(f: &dyn Fn(Vec3) -> f32, min: Vec3, max: Vec3, voxel: f32, tolerance: f32) -> Mesh {
    let step = voxel * 4.0;
    let lo = min - Vec3::splat(step);
    let dims = ((max - min) / step).ceil() + Vec3::splat(4.0);
    let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
    let mut coarse = vec![0.0f32; nx * ny * nz];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                coarse[(z * ny + y) * nx + x] =
                    f(lo + Vec3::new(x as f32, y as f32, z as f32) * step);
            }
        }
    }
    let banded = |p: Vec3| {
        let g = ((p - lo) / step).round();
        if g.min_element() >= 0.0 && g.x < nx as f32 && g.y < ny as f32 && g.z < nz as f32 {
            let c = lo + g * step;
            let d = coarse[(g.z as usize * ny + g.y as usize) * nx + g.x as usize];
            let r = (p - c).length();
            if d.abs() > r + voxel * 2.0 {
                return d.signum() * (d.abs() - r);
            }
        }
        f(p)
    };
    let mut grid = Grid::new(min, max, voxel);
    grid.occlusion = false;
    let m = sdf::polygonize(&banded, grid);
    charts(&decimate(&m, tolerance))
}

/// How far decimation may move an SDF surface, in voxels.
const DECIMATE: f32 = 0.3;

/// Drop the triangles buried inside `solid` (every corner deeper than `margin`), then the
/// vertices nothing uses.
fn cull(m: Mesh, solid: &dyn Fn(Vec3) -> f32, margin: f32) -> Mesh {
    let inside: Vec<bool> = m
        .vertices
        .iter()
        .map(|v| solid(Vec3::from(v.position)) < -margin)
        .collect();
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks(3) {
        if t.iter().all(|&i| inside[i as usize]) {
            continue;
        }
        idx.extend_from_slice(t);
    }
    let mut remap = vec![u32::MAX; m.vertices.len()];
    let mut verts = Vec::new();
    for i in &mut idx {
        let r = &mut remap[*i as usize];
        if *r == u32::MAX {
            *r = verts.len() as u32;
            verts.push(m.vertices[*i as usize]);
        }
        *i = *r;
    }
    Mesh::new(verts, idx)
}

/// Sum of squared distances to a set of planes, as a symmetric 4×4 matrix.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: Vec3, d: f32) -> Self {
        let (a, b, c, d) = (n.x as f64, n.y as f64, n.z as f64, d as f64);
        Self([
            a * a,
            a * b,
            a * c,
            a * d,
            b * b,
            b * c,
            b * d,
            c * c,
            c * d,
            d * d,
        ])
    }

    fn add(&mut self, o: &Quadric) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a += b;
        }
    }

    fn eval(&self, p: Vec3) -> f64 {
        let q = &self.0;
        let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
        q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9]
    }
}

/// Simplify a mesh by collapsing edges onto one of their ends, cheapest first by quadric error,
/// while the surface moves less than about `tolerance` meters. Open borders stay as they are,
/// collapses that would fold or sliver a triangle are skipped, and surviving vertices keep
/// their attributes (normals come from the distance field, so shading stays smooth).
fn decimate(m: &Mesh, tolerance: f32) -> Mesh {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let n = m.vertices.len();
    let pos: Vec<Vec3> = m.vertices.iter().map(|v| Vec3::from(v.position)).collect();
    let mut tris: Vec<[u32; 3]> = m.indices.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut alive = vec![true; tris.len()];
    let mut around: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut q = vec![Quadric::default(); n];
    let mut uses: HashMap<(u32, u32), u8> = HashMap::with_capacity(tris.len() * 2);
    for (ti, t) in tris.iter().enumerate() {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let nrm = (b - a).cross(c - a).normalize_or_zero();
        let plane = Quadric::plane(nrm, -nrm.dot(a));
        for k in 0..3 {
            let i = t[k] as usize;
            q[i].add(&plane);
            around[i].push(ti as u32);
            let (e0, e1) = (t[k], t[(k + 1) % 3]);
            let e = uses.entry((e0.min(e1), e0.max(e1))).or_default();
            *e = e.saturating_add(1);
        }
    }
    let mut locked = vec![false; n];
    for (&(a, b), &c) in &uses {
        if c != 2 {
            locked[a as usize] = true;
            locked[b as usize] = true;
        }
    }
    let limit = (tolerance as f64).powi(2);
    let mut stamp = vec![0u32; n];
    let mut dead = vec![false; n];
    let mut heap = BinaryHeap::new();
    let cost = |q: &[Quadric], u: usize, v: usize| {
        let mut sum = q[u];
        sum.add(&q[v]);
        sum.eval(pos[v]).max(0.0)
    };
    for &(a, b) in uses.keys() {
        for (u, v) in [(a as usize, b as usize), (b as usize, a as usize)] {
            let c = cost(&q, u, v);
            if !locked[u] && c <= limit {
                heap.push(Reverse((c.to_bits(), u as u32, v as u32, 0u32, 0u32)));
            }
        }
    }
    let quality = |p: [Vec3; 3]| {
        let area2 = (p[1] - p[0]).cross(p[2] - p[0]).length();
        let sum = (p[1] - p[0]).length_squared()
            + (p[2] - p[1]).length_squared()
            + (p[0] - p[2]).length_squared();
        // 1 for an equilateral triangle, 0 for a degenerate one.
        2.0 * 3f32.sqrt() * area2 / sum.max(1e-20)
    };
    /// The vertices around `x`.
    fn ring(x: usize, out: &mut Vec<u32>, around: &[Vec<u32>], alive: &[bool], tris: &[[u32; 3]]) {
        out.clear();
        for &t in &around[x] {
            if alive[t as usize] {
                for i in tris[t as usize] {
                    if i as usize != x && !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
    }
    let mut ring_u: Vec<u32> = Vec::new();
    let mut ring_v: Vec<u32> = Vec::new();
    while let Some(Reverse((_, u, v, su, sv))) = heap.pop() {
        let (u, v) = (u as usize, v as usize);
        if dead[u] || dead[v] || stamp[u] != su || stamp[v] != sv {
            continue;
        }
        // The edge must still exist, and its ends share exactly the two vertices across it.
        ring(u, &mut ring_u, &around, &alive, &tris);
        if !ring_u.contains(&(v as u32)) {
            continue;
        }
        ring(v, &mut ring_v, &around, &alive, &tris);
        let shared = ring_u.iter().filter(|i| ring_v.contains(i)).count();
        if shared != 2 {
            continue;
        }
        let mut ok = true;
        for &t in &around[u] {
            let t = t as usize;
            if !alive[t] || tris[t].contains(&(v as u32)) {
                continue;
            }
            let before = tris[t].map(|i| pos[i as usize]);
            let after = tris[t].map(|i| {
                if i as usize == u {
                    pos[v]
                } else {
                    pos[i as usize]
                }
            });
            let n0 = (before[1] - before[0])
                .cross(before[2] - before[0])
                .normalize_or_zero();
            let n1 = (after[1] - after[0])
                .cross(after[2] - after[0])
                .normalize_or_zero();
            if n0.dot(n1) < 0.3 || quality(after) < 0.2_f32.min(quality(before)) {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let list = std::mem::take(&mut around[u]);
        for &t in &list {
            let ti = t as usize;
            if !alive[ti] {
                continue;
            }
            if tris[ti].contains(&(v as u32)) {
                alive[ti] = false;
            } else {
                for i in &mut tris[ti] {
                    if *i as usize == u {
                        *i = v as u32;
                    }
                }
                around[v].push(t);
            }
        }
        around[v].retain(|&t| alive[t as usize]);
        dead[u] = true;
        let qu = q[u];
        q[v].add(&qu);
        stamp[v] += 1;
        ring(v, &mut ring_v, &around, &alive, &tris);
        for &w in &ring_v {
            let w = w as usize;
            for (a, b) in [(v, w), (w, v)] {
                let c = cost(&q, a, b);
                if !locked[a] && c <= limit {
                    heap.push(Reverse((
                        c.to_bits(),
                        a as u32,
                        b as u32,
                        stamp[a],
                        stamp[b],
                    )));
                }
            }
        }
    }
    let mut remap = vec![u32::MAX; n];
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    for (t, _) in tris.iter().zip(&alive).filter(|(_, a)| **a) {
        for &i in t {
            let r = &mut remap[i as usize];
            if *r == u32::MAX {
                *r = verts.len() as u32;
                verts.push(m.vertices[i as usize]);
            }
            idx.push(*r);
        }
    }
    Mesh::new(verts, idx)
}

/// Give every triangle the box projection of its own facing (splitting vertices where the
/// facing changes), so textures do not smear across projection changes.
fn charts(m: &Mesh) -> Mesh {
    let mut map: HashMap<(u32, u8), u32> = HashMap::with_capacity(m.vertices.len() * 2);
    let mut verts = Vec::with_capacity(m.vertices.len() * 5 / 4);
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(m.vertices[t[k] as usize].position));
        let n = (b - a).cross(c - a);
        let s = n.abs();
        let chart = if s.x >= s.y && s.x >= s.z {
            if n.x > 0.0 { 0 } else { 1 }
        } else if s.y >= s.z {
            if n.y > 0.0 { 2 } else { 3 }
        } else if n.z > 0.0 {
            4
        } else {
            5
        };
        for &i in t {
            let next = verts.len() as u32;
            let j = *map.entry((i, chart)).or_insert(next);
            if j == next {
                let mut v = m.vertices[i as usize];
                let p = Vec3::from(v.position);
                let sign = if chart % 2 == 0 { 1.0 } else { -1.0 };
                let uv = match chart / 2 {
                    0 => v2(p.z * sign, -p.y),
                    1 => v2(p.x, p.z * sign),
                    _ => v2(-p.x * sign, -p.y),
                };
                v.uv = uv.to_array();
                verts.push(v);
            }
            idx.push(j);
        }
    }
    Mesh::new(verts, idx)
}

/// Mirror a mesh across the YZ plane (a right piece into a left one).
fn mirrored(m: &Mesh) -> Mesh {
    let mut out = m.clone();
    for v in &mut out.vertices {
        v.position[0] = -v.position[0];
        v.normal[0] = -v.normal[0];
    }
    for t in out.indices.chunks_mut(3) {
        t.swap(1, 2);
    }
    out
}

/// Move and rotate a mesh.
fn placed(mut m: Mesh, rot: Quat, offset: Vec3) -> Mesh {
    m.transform(&Mat4::from_rotation_translation(rot, offset));
    m
}

/// Orient every triangle so it faces the way its vertex normals point.
fn fix_winding(m: &mut Mesh) {
    for t in m.indices.chunks_mut(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize]);
        let (pa, pb, pc) = (
            Vec3::from(a.position),
            Vec3::from(b.position),
            Vec3::from(c.position),
        );
        let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
        if (pb - pa).cross(pc - pa).dot(n) < 0.0 {
            t.swap(1, 2);
        }
    }
}

// ---------------------------------------------------------------------- 2D outlines

fn box2(p: Vec2, c: Vec2, half: Vec2, r: f32) -> f32 {
    let q = (p - c).abs() - half + Vec2::splat(r);
    q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - r
}

fn circle2(p: Vec2, c: Vec2, r: f32) -> f32 {
    p.distance(c) - r
}

// ---------------------------------------------------------------------- plates

/// How a plate's parameters map onto a body: rays from an axis (cylindrical) or a center
/// (spherical). `u` is arc length at radius `r` around the axis (0 toward the frame's −Z,
/// positive toward +X); `v` is height along the axis, or elevation arc length.
#[derive(Debug, Clone, Copy)]
struct Param {
    origin: Vec3,
    rot: Quat,
    r: f32,
    sphere: bool,
}

impl Param {
    fn cyl(origin: Vec3, rot: Quat, r: f32) -> Self {
        Self {
            origin,
            rot,
            r,
            sphere: false,
        }
    }

    fn sph(origin: Vec3, rot: Quat, r: f32) -> Self {
        Self {
            origin,
            rot,
            r,
            sphere: true,
        }
    }

    /// A point inside the body and the outward ray direction for parameter `uv`.
    fn ray(&self, uv: Vec2) -> (Vec3, Vec3) {
        let a = uv.x / self.r;
        if self.sphere {
            let e = uv.y / self.r;
            let d = Vec3::new(e.cos() * a.sin(), e.sin(), -e.cos() * a.cos());
            (self.origin, self.rot * d)
        } else {
            (
                self.origin + self.rot * Vec3::new(0.0, uv.y, 0.0),
                self.rot * Vec3::new(a.sin(), 0.0, -a.cos()),
            )
        }
    }

    /// The parameter of point `p` (inverse of [`Param::ray`]).
    fn param(&self, p: Vec3) -> Vec2 {
        let q = self.rot.inverse() * (p - self.origin);
        let a = q.x.atan2(-q.z);
        if self.sphere {
            let e = q.y.atan2(Vec2::new(q.x, q.z).length());
            Vec2::new(a, e) * self.r
        } else {
            Vec2::new(a * self.r, q.y)
        }
    }
}

/// Where the ray from `inside` along `dir` leaves the surface of `f`, and the normal there.
fn cast(f: &dyn Fn(Vec3) -> f32, inside: Vec3, dir: Vec3, reach: f32) -> (Vec3, Vec3) {
    let start = inside + dir * reach;
    let mut t = 0.0;
    let mut p = start;
    for _ in 0..80 {
        let d = f(p);
        if d.abs() < 2e-5 {
            break;
        }
        t += d * 0.9;
        if !(-0.05..=reach).contains(&t) {
            break;
        }
        p = start - dir * t;
    }
    (p, grad(f, p))
}

/// A thick panel (armor plate, belt, tabard, visor) over a base surface.
struct Plate<'a> {
    param: Param,
    /// The surface it lies on (meters, negative inside).
    base: &'a dyn Fn(Vec3) -> f32,
    /// Clearance between the base surface and the plate's back.
    gap: f32,
    /// Outline in parameter space (negative inside).
    outline: &'a dyn Fn(Vec2) -> f32,
    /// Thickness plus relief: f(parameter, outline distance).
    height: &'a dyn Fn(Vec2, f32) -> f32,
    min: Vec2,
    max: Vec2,
    /// Grid spacing in parameter space, meters.
    step: f32,
    /// How far the rolled rim bulges past the outline, meters.
    roll: f32,
    /// Rim segments (front to back).
    rim: usize,
    /// Emit the back face (for plates that stand off the body).
    back: bool,
    reach: f32,
}

#[derive(Clone, Copy)]
struct PlateSample {
    uv: Vec2,
    s: f32,
    front: Vec3,
    back: Vec3,
    n: Vec3,
    valid: bool,
}

impl Plate<'_> {
    /// A rough distance field of the plate (for ambient occlusion).
    fn proxy(&self, p: Vec3) -> f32 {
        let uv = self.param.param(p);
        let s = (self.outline)(uv);
        let d = (self.base)(p) - self.gap - (self.height)(uv, s.min(0.0));
        d.max(s)
    }

    fn mesh(&self) -> Mesh {
        let size = ((self.max - self.min) / self.step).ceil();
        let (nu, nv) = (size.x as usize + 3, size.y as usize + 3);
        let uv_at =
            |i: usize, j: usize| self.min + Vec2::new(i as f32 - 1.0, j as f32 - 1.0) * self.step;
        let mut g: Vec<PlateSample> = Vec::with_capacity(nu * nv);
        for j in 0..nv {
            for i in 0..nu {
                let uv = uv_at(i, j);
                let s = (self.outline)(uv);
                let mut sample = PlateSample {
                    uv,
                    s,
                    front: Vec3::ZERO,
                    back: Vec3::ZERO,
                    n: Vec3::Y,
                    valid: false,
                };
                if s < self.step * 2.5 {
                    let (o, d) = self.param.ray(uv);
                    let (b, n) = cast(self.base, o, d, self.reach);
                    sample.back = b + n * self.gap;
                    sample.front = b + n * (self.gap + (self.height)(uv, s.min(0.0)));
                    sample.n = n;
                    sample.valid = true;
                }
                g.push(sample);
            }
        }
        let at = |i: usize, j: usize| &g[j * nu + i];
        // Front normals and tangents from neighbors on the grid.
        let tangents = |i: usize, j: usize| -> (Vec3, Vec3) {
            let pick = |a: usize, b: usize| {
                let s = at(a, b);
                if s.valid { Some(s.front) } else { None }
            };
            let c = at(i, j).front;
            let du = match (pick(i.saturating_sub(1), j), pick((i + 1).min(nu - 1), j)) {
                (Some(a), Some(b)) => b - a,
                (Some(a), None) => c - a,
                (None, Some(b)) => b - c,
                _ => Vec3::ZERO,
            };
            let dv = match (pick(i, j.saturating_sub(1)), pick(i, (j + 1).min(nv - 1))) {
                (Some(a), Some(b)) => b - a,
                (Some(a), None) => c - a,
                (None, Some(b)) => b - c,
                _ => Vec3::ZERO,
            };
            (du, dv)
        };
        let normal_at = |i: usize, j: usize| {
            let (du, dv) = tangents(i, j);
            let n0 = at(i, j).n;
            let n = du.cross(dv).normalize_or(n0);
            if n.dot(n0) < 0.0 { -n } else { n }
        };
        let mut verts: Vec<Vertex> = Vec::new();
        let mut index: HashMap<(usize, u8), u32> = HashMap::new();
        // Crossing info for rims: front vertex, back point, outward direction, uv.
        let mut crossings: HashMap<usize, (u32, Vec3, Vec3, Vec3, Vec2)> = HashMap::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut boundary: Vec<(usize, usize)> = Vec::new();
        let mut back_tris: Vec<[usize; 3]> = Vec::new();
        // Keys: corner (i, j) = (j*nu+i, 0); edge along u from (i, j) = (.., 1); along v = 2.
        let corner = |i: usize,
                      j: usize,
                      verts: &mut Vec<Vertex>,
                      index: &mut HashMap<(usize, u8), u32>|
         -> u32 {
            *index.entry((j * nu + i, 0)).or_insert_with(|| {
                let s = at(i, j);
                verts.push(Vertex::new(s.front, normal_at(i, j), s.uv - self.min));
                verts.len() as u32 - 1
            })
        };
        let outline_grad = |uv: Vec2| {
            let e = self.step * 0.25;
            Vec2::new(
                (self.outline)(uv + Vec2::X * e) - (self.outline)(uv - Vec2::X * e),
                (self.outline)(uv + Vec2::Y * e) - (self.outline)(uv - Vec2::Y * e),
            )
            .normalize_or(Vec2::X)
        };
        let mut corner_ids: Vec<(usize, u8, u32)> = Vec::new();
        for j in 0..nv - 1 {
            for i in 0..nu - 1 {
                let cs = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                if cs.iter().all(|&(a, b)| at(a, b).s >= 0.0) {
                    continue;
                }
                if cs.iter().any(|&(a, b)| !at(a, b).valid) {
                    continue;
                }
                // Clip the cell to the inside of the outline.
                let mut poly: Vec<(u32, bool, usize)> = Vec::new();
                for k in 0..4 {
                    let (a, b) = (cs[k], cs[(k + 1) % 4]);
                    let (sa, sb) = (at(a.0, a.1).s, at(b.0, b.1).s);
                    if sa < 0.0 {
                        let id = corner(a.0, a.1, &mut verts, &mut index);
                        poly.push((id, false, 0));
                        corner_ids.push((a.1 * nu + a.0, 0, id));
                    }
                    if (sa < 0.0) != (sb < 0.0) {
                        // Edge key: the lower corner and the direction.
                        let (lo, dir) = match k {
                            0 => ((i, j), 1u8),
                            1 => ((i + 1, j), 2u8),
                            2 => ((i, j + 1), 1u8),
                            _ => ((i, j), 2u8),
                        };
                        let key = (lo.1 * nu + lo.0, dir);
                        let ek = key.0 * 3 + dir as usize;
                        let id = match index.get(&key) {
                            Some(&id) => id,
                            None => {
                                let (sa_, sb_) = (at(a.0, a.1), at(b.0, b.1));
                                let t = (sa_.s / (sa_.s - sb_.s)).clamp(0.03, 0.97);
                                let front = sa_.front.lerp(sb_.front, t);
                                let back = sa_.back.lerp(sb_.back, t);
                                let uv = sa_.uv.lerp(sb_.uv, t);
                                let n =
                                    normal_at(a.0, a.1).lerp(normal_at(b.0, b.1), t).normalize();
                                let (tu_a, tv_a) = tangents(a.0, a.1);
                                let (tu_b, tv_b) = tangents(b.0, b.1);
                                let (tu, tv) = (tu_a.lerp(tu_b, t), tv_a.lerp(tv_b, t));
                                let og = outline_grad(uv);
                                let mut o = tu * og.x + tv * og.y;
                                o = (o - n * o.dot(n)).normalize_or(Vec3::X);
                                verts.push(Vertex::new(front, n, uv - self.min));
                                let id = verts.len() as u32 - 1;
                                index.insert(key, id);
                                crossings.insert(ek, (id, back, n, o, uv));
                                id
                            }
                        };
                        poly.push((id, true, ek));
                    }
                }
                if poly.len() < 3 {
                    continue;
                }
                for k in 1..poly.len() - 1 {
                    tris.push([poly[0].0, poly[k].0, poly[k + 1].0]);
                    back_tris.push([
                        poly[0].0 as usize,
                        poly[k].0 as usize,
                        poly[k + 1].0 as usize,
                    ]);
                }
                for k in 0..poly.len() {
                    let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                    if a.1 && b.1 && a.2 != b.2 {
                        boundary.push((a.2, b.2));
                    }
                }
            }
        }
        // Back face: copies of the front vertices moved to the back, facing inward.
        let mut back_of: HashMap<u32, u32> = HashMap::new();
        if self.back {
            let corner_back: HashMap<u32, Vec3> = corner_ids
                .iter()
                .map(|&(gi, _, id)| (id, g[gi].back))
                .collect();
            let crossing_back: HashMap<u32, Vec3> =
                crossings.values().map(|c| (c.0, c.1)).collect();
            for t in &back_tris {
                for &v in t {
                    let id = v as u32;
                    if back_of.contains_key(&id) {
                        continue;
                    }
                    let p = corner_back
                        .get(&id)
                        .or_else(|| crossing_back.get(&id))
                        .copied()
                        .unwrap_or(Vec3::from(verts[v].position));
                    let n = -Vec3::from(verts[v].normal);
                    let uv = Vec2::from(verts[v].uv);
                    verts.push(Vertex::new(p, n, uv));
                    back_of.insert(id, verts.len() as u32 - 1);
                }
            }
            for t in &back_tris {
                tris.push([
                    back_of[&(t[0] as u32)],
                    back_of[&(t[2] as u32)],
                    back_of[&(t[1] as u32)],
                ]);
            }
        }
        // Rolled rim around the outline.
        let mut rings: HashMap<usize, Vec<u32>> = HashMap::new();
        for (ek, c) in &crossings {
            let (front_id, back, n, o, uv) = *c;
            let front = Vec3::from(verts[front_id as usize].position);
            let mid = (front + back) * 0.5;
            let a = (front - back) * 0.5;
            let ha = a.length().max(1e-5);
            let an = a / ha;
            let mut ring = vec![front_id];
            for k in 1..=self.rim {
                if k == self.rim && self.back {
                    ring.push(back_of.get(&front_id).copied().unwrap_or(front_id));
                    continue;
                }
                let phi = PI * k as f32 / self.rim as f32;
                let p = mid + a * phi.cos() + o * self.roll * phi.sin();
                let nn = (an * self.roll * phi.cos() + o * ha * phi.sin()).normalize_or(n);
                let ruv = uv - self.min + Vec2::splat(self.roll * phi.sin());
                verts.push(Vertex::new(p, nn, ruv).with_color(Vec4::new(0.0, 1.0, 1.0, 1.0)));
                ring.push(verts.len() as u32 - 1);
            }
            rings.insert(*ek, ring);
        }
        for (a, b) in &boundary {
            let (Some(ra), Some(rb)) = (rings.get(a), rings.get(b)) else {
                continue;
            };
            for k in 0..self.rim {
                tris.push([ra[k], ra[k + 1], rb[k + 1]]);
                tris.push([ra[k], rb[k + 1], rb[k]]);
            }
        }
        let mut mesh = Mesh::new(verts, tris.into_iter().flatten().collect());
        fix_winding(&mut mesh);
        decimate(&mesh, PLATE_TOLERANCE)
    }
}

/// How far decimation may move a plate's surface, meters.
const PLATE_TOLERANCE: f32 = 0.0008;

// ---------------------------------------------------------------------- lathe, sweep, extrude

/// Revolve a profile of (radius, y) points around Y. Profile points listed in `sharp` crease.
fn lathe(profile: &[Vec2], segs: usize, sharp: &[usize]) -> Mesh {
    // Split the profile at sharp points so each side gets its own normal.
    let mut pts: Vec<(Vec2, Vec2)> = Vec::new();
    let n = profile.len();
    let seg_normal = |a: Vec2, b: Vec2| {
        let d = (b - a).normalize_or(Vec2::Y);
        Vec2::new(d.y, -d.x)
    };
    for i in 0..n {
        let prev = (i > 0).then(|| seg_normal(profile[i - 1], profile[i]));
        let next = (i + 1 < n).then(|| seg_normal(profile[i], profile[i + 1]));
        match (prev, next) {
            (Some(a), Some(b)) if sharp.contains(&i) => {
                pts.push((profile[i], a));
                pts.push((profile[i], b));
            }
            (Some(a), Some(b)) => pts.push((profile[i], (a + b).normalize_or(a))),
            (Some(a), None) | (None, Some(a)) => pts.push((profile[i], a)),
            _ => pts.push((profile[i], Vec2::X)),
        }
    }
    let mut verts = Vec::new();
    let mut arc = 0.0;
    let r_ref = profile.iter().map(|p| p.x).fold(0.0, f32::max).max(0.01);
    for (k, &(p, nrm)) in pts.iter().enumerate() {
        if k > 0 {
            arc += p.distance(pts[k - 1].0);
        }
        for s in 0..=segs {
            let a = s as f32 / segs as f32 * TAU;
            let (sn, cs) = a.sin_cos();
            let pos = Vec3::new(p.x * cs, p.y, p.x * sn);
            let nn = Vec3::new(nrm.x * cs, nrm.y, nrm.x * sn);
            verts.push(Vertex::new(pos, nn, Vec2::new(a * r_ref, arc)));
        }
    }
    let mut idx = Vec::new();
    let w = segs + 1;
    for k in 0..pts.len() - 1 {
        if pts[k].0 == pts[k + 1].0 {
            continue;
        }
        for s in 0..segs {
            let (a, b, c, d) = (
                k * w + s,
                k * w + s + 1,
                (k + 1) * w + s + 1,
                (k + 1) * w + s,
            );
            idx.extend_from_slice(&[a as u32, b as u32, c as u32, a as u32, c as u32, d as u32]);
        }
    }
    let mut m = Mesh::new(verts, idx);
    fix_winding(&mut m);
    m
}

/// Sweep cross-sections along a path. Section `i` lies in the plane of path point `i`, with
/// its x along `side` and its y along `tangent × side`. Sections have equal point counts and
/// are closed; repeat a point to make a crease. Both ends are capped.
fn sweep(path: &[Vec3], side: Vec3, sections: &[Vec<Vec2>]) -> Mesh {
    let n = path.len();
    let k = sections[0].len();
    let mut verts = Vec::new();
    let mut arc = 0.0;
    let mut frames = Vec::with_capacity(n);
    for i in 0..n {
        let t = (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec3::Z);
        let x = (side - t * side.dot(t)).normalize_or(Vec3::X);
        let y = t.cross(x);
        frames.push((x, y, t));
    }
    for i in 0..n {
        if i > 0 {
            arc += path[i].distance(path[i - 1]);
        }
        let (x, y, _) = frames[i];
        let sec = &sections[i];
        let c = sec.iter().copied().sum::<Vec2>() / k as f32;
        for (j, &q) in sec.iter().enumerate() {
            let pos = path[i] + x * q.x + y * q.y;
            let out = (x * (q.x - c.x) + y * (q.y - c.y)).normalize_or(x);
            verts.push(Vertex::new(
                pos,
                out,
                Vec2::new(j as f32 / k as f32 * 0.1, arc),
            ));
        }
    }
    let mut idx = Vec::new();
    for i in 0..n - 1 {
        for j in 0..k {
            let j1 = (j + 1) % k;
            let (a, b, c, d) = (i * k + j, i * k + j1, (i + 1) * k + j1, (i + 1) * k + j);
            idx.extend_from_slice(&[a as u32, b as u32, c as u32, a as u32, c as u32, d as u32]);
        }
    }
    let mut m = Mesh::new(verts, idx);
    m.compute_normals();
    // Orient the walls outward from the path.
    for t in m.indices.chunks_mut(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
        let i = t[0] as usize / k;
        let out = (a + b + c) / 3.0 - path[i];
        if (b - a).cross(c - a).dot(out) < 0.0 {
            t.swap(1, 2);
        }
    }
    m.compute_normals();
    // Caps.
    for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
        let (x, y, t) = frames[i];
        let sec = &sections[i];
        let c = sec.iter().copied().sum::<Vec2>() / k as f32;
        if sec.iter().all(|q| q.distance(c) < 1e-5) {
            continue;
        }
        let base = m.vertices.len() as u32;
        let nrm = t * sign;
        m.vertices
            .push(Vertex::new(path[i] + x * c.x + y * c.y, nrm, c));
        for &q in sec {
            m.vertices
                .push(Vertex::new(path[i] + x * q.x + y * q.y, nrm, q));
        }
        for j in 0..k as u32 {
            let (p, q) = (base + 1 + j, base + 1 + (j + 1) % k as u32);
            let [pa, pb, pc] = [base, p, q].map(|i| Vec3::from(m.vertices[i as usize].position));
            if (pb - pa).cross(pc - pa).dot(nrm) >= 0.0 {
                m.indices.extend_from_slice(&[base, p, q]);
            } else {
                m.indices.extend_from_slice(&[base, q, p]);
            }
        }
    }
    m
}

/// Ear-clipping triangulation of a simple counter-clockwise polygon.
fn triangulate(pts: &[Vec2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::new();
    let cross = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) <= 1e-12 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = pts[j];
                cross(a, b, p) > 0.0 && cross(b, c, p) > 0.0 && cross(c, a, p) > 0.0
            });
            if inside {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

/// Signed area (positive = counter-clockwise).
fn area2(pts: &[Vec2]) -> f32 {
    let n = pts.len();
    (0..n)
        .map(|i| pts[i].perp_dot(pts[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// Offset a closed counter-clockwise polygon inward by `d` (mitered).
fn inset(pts: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let n1 = (b - a).normalize_or(Vec2::X).perp();
            let n2 = (c - b).normalize_or(Vec2::X).perp();
            let m = (n1 + n2).normalize_or(n1);
            let s = m.dot(n1).max(0.35);
            b + m * (d / s)
        })
        .collect()
}

/// Extrude a closed outline (XY plane) along Z, centered, `depth` deep, with chamfered cap
/// edges. Corners turning more than ~35° crease; gentler ones are smooth.
fn extrude(outline: &[Vec2], depth: f32, bevel: f32) -> Mesh {
    let mut pts = outline.to_vec();
    if area2(&pts) < 0.0 {
        pts.reverse();
    }
    let n = pts.len();
    let h = depth * 0.5;
    let inner = inset(&pts, bevel);
    let mut verts: Vec<Vertex> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    let edge_n = |i: usize| -> Vec2 {
        let d = (pts[(i + 1) % n] - pts[i]).normalize_or(Vec2::X);
        Vec2::new(d.y, -d.x)
    };
    // Per-corner normals for the side walls (left edge, right edge).
    let corner_n = |i: usize| -> (Vec2, Vec2) {
        let (a, b) = (edge_n((i + n - 1) % n), edge_n(i));
        if a.dot(b) > 0.82 {
            let m = (a + b).normalize_or(a);
            (m, m)
        } else {
            (a, b)
        }
    };
    let mut perim = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        let (_, ni) = corner_n(i);
        let (nj, _) = corner_n(j);
        let len = pts[i].distance(pts[j]);
        let layers: [(Vec2, Vec2, f32, Vec3, Vec3); 4] = [
            (
                inner[i],
                inner[j],
                h,
                (ni.extend(0.0) + Vec3::Z * 1.4).normalize(),
                (nj.extend(0.0) + Vec3::Z * 1.4).normalize(),
            ),
            (pts[i], pts[j], h - bevel, ni.extend(0.0), nj.extend(0.0)),
            (pts[i], pts[j], -h + bevel, ni.extend(0.0), nj.extend(0.0)),
            (
                inner[i],
                inner[j],
                -h,
                (ni.extend(0.0) - Vec3::Z * 1.4).normalize(),
                (nj.extend(0.0) - Vec3::Z * 1.4).normalize(),
            ),
        ];
        let base = verts.len() as u32;
        for (k, &(a, b, z, na, nb)) in layers.iter().enumerate() {
            let v = k as f32 * depth / 3.0;
            verts.push(Vertex::new(a.extend(z), na, Vec2::new(perim, v)));
            verts.push(Vertex::new(b.extend(z), nb, Vec2::new(perim + len, v)));
        }
        for k in 0..3u32 {
            let (a, b, c, d) = (
                base + k * 2,
                base + k * 2 + 1,
                base + k * 2 + 3,
                base + k * 2 + 2,
            );
            idx.extend_from_slice(&[a, b, c, a, c, d]);
        }
        perim += len;
    }
    // Caps.
    let tris = triangulate(&inner);
    for (z, nz) in [(h, 1.0f32), (-h, -1.0)] {
        let base = verts.len() as u32;
        for p in &inner {
            verts.push(Vertex::new(p.extend(z), Vec3::Z * nz, *p));
        }
        for t in &tris {
            idx.extend_from_slice(&[base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]);
        }
    }
    let mut m = Mesh::new(verts, idx);
    fix_winding(&mut m);
    m
}

/// A closed outline through `pts` with each corner rounded by its radius.
fn rounded(pts: &[(Vec2, f32)], arc_segs: usize) -> Vec<Vec2> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, (b, r), c) = (pts[(i + n - 1) % n].0, pts[i], pts[(i + 1) % n].0);
        if r <= 0.0 {
            out.push(b);
            continue;
        }
        let (da, dc) = ((a - b).normalize_or(Vec2::X), (c - b).normalize_or(Vec2::Y));
        let half = da.dot(dc).clamp(-1.0, 1.0).acos() * 0.5;
        let t = (r / half.tan())
            .min(a.distance(b) * 0.5)
            .min(c.distance(b) * 0.5);
        let (p0, p1) = (b + da * t, b + dc * t);
        // Quadratic Bézier through the corner (close to a circular arc).
        for k in 0..=arc_segs {
            let s = k as f32 / arc_segs as f32;
            out.push(p0 * (1.0 - s) * (1.0 - s) + b * 2.0 * s * (1.0 - s) + p1 * s * s);
        }
    }
    out
}

// ====================================================================== piece builder

/// How a sub-part is colored: a fixed linear color, or the team color times a brightness
/// (`wear` lightens its rolled rims toward worn paint).
#[derive(Debug, Clone, Copy)]
enum Tint {
    Rgb(Vec3),
    Team(f32),
    /// A fixed color dyed toward the team color by the weight.
    Dyed(Vec3, f32),
}

/// The padded undersuit, dyed a little toward the team color so teams read from afar.
const SUIT_DYE: f32 = 0.45;

/// sRGB hex to linear.
fn lin(hex: u32) -> Vec3 {
    super::tex::hex(hex)
}

const SUIT: u32 = 0x34363b;
const LEATHER: u32 = 0x4a3324;
const LEATHER_DARK: u32 = 0x2b2420;
const STEEL: u32 = 0xc4c8cf;
const GUNMETAL: u32 = 0x4a4d53;
const BRASS: u32 = 0xc89a55;
const POLYMER: u32 = 0x2a2b2e;
const RUBBER: u32 = 0x1c1c1e;
const GLASS: u32 = 0x0b0d10;
const WOOD: u32 = 0x8a5a34;
const CREAM: u32 = 0xe2d6bc;
/// Worn paint at plate rims (mixed with the team color).
const EDGE_WEAR: f32 = 0.3;

/// A built piece: its mesh (vertex colors hold occlusion × fixed tints) and the vertices that
/// take the team color, with their weight.
#[derive(Clone, Default)]
pub struct PieceMesh {
    mesh: Mesh,
    team: Vec<(u32, f32)>,
}

impl PieceMesh {
    fn mirrored(&self) -> Self {
        Self {
            mesh: mirrored(&self.mesh),
            team: self.team.clone(),
        }
    }

    pub fn triangles(&self) -> usize {
        self.mesh.triangle_count()
    }
}

#[derive(Default)]
struct Builder {
    mesh: Mesh,
    /// Per vertex: team weight (0 = fixed color only) and the fixed color.
    tint: Vec<(f32, Vec3)>,
}

impl Builder {
    fn add(&mut self, m: Mesh, zone: Zone, tint: Tint) {
        self.add_fn(m, zone, &|_| tint);
    }

    /// Add a sub-part whose tint varies over it (painted by position).
    fn add_fn(&mut self, m: Mesh, zone: Zone, tint: &dyn Fn(Vec3) -> Tint) {
        if m.vertices.is_empty() {
            return;
        }
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for v in &m.vertices {
            let uv = Vec2::from(v.uv);
            lo = lo.min(uv);
            hi = hi.max(uv);
        }
        let span = (hi - lo).max_element().max(TILE_M);
        let base = self.mesh.vertices.len() as u32;
        for v in &m.vertices {
            let mut out = *v;
            out.uv = tile_uv(zone, Vec2::from(v.uv) - lo, span).to_array();
            // Plate rims come marked with red = 0.
            let rim = v.color[0] < 0.5;
            out.color = [1.0; 4];
            self.mesh.vertices.push(out);
            self.tint.push(match tint(Vec3::from(v.position)) {
                Tint::Rgb(c) => (0.0, c),
                Tint::Team(k) if rim => (k * (1.0 - EDGE_WEAR), Vec3::splat(0.55 * EDGE_WEAR)),
                Tint::Team(k) => (k, Vec3::ZERO),
                Tint::Dyed(c, k) => (k, c * (1.0 - k)),
            });
        }
        self.mesh.indices.extend(m.indices.iter().map(|i| i + base));
    }

    /// Bake occlusion from `field` (the whole piece) and fix the colors.
    fn finish(mut self, field: &dyn Fn(Vec3) -> f32, ao: f32) -> PieceMesh {
        let mut team = Vec::new();
        for (i, v) in self.mesh.vertices.iter_mut().enumerate() {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            let mut occ = 0.0;
            let mut w = 0.5;
            for d in [0.008f32, 0.018, 0.035, 0.07] {
                occ += w * (d - field(p + n * d)).max(0.0) / d;
                w *= 0.55;
            }
            let o = (1.0 - occ * ao).clamp(0.35, 1.0);
            let (k, c) = self.tint[i];
            v.color = [c.x * o, c.y * o, c.z * o, 1.0];
            if k > 0.0 {
                team.push((i as u32, k * o));
            }
        }
        PieceMesh {
            mesh: self.mesh,
            team,
        }
    }
}

/// Plate with the usual defaults: thickness `t`, rim rolled by half of it.
#[allow(clippy::too_many_arguments)]
fn plate<'a>(
    param: Param,
    base: &'a dyn Fn(Vec3) -> f32,
    gap: f32,
    outline: &'a dyn Fn(Vec2) -> f32,
    height: &'a dyn Fn(Vec2, f32) -> f32,
    min: Vec2,
    max: Vec2,
    step: f32,
) -> Plate<'a> {
    Plate {
        param,
        base,
        gap,
        outline,
        height,
        min,
        max,
        step,
        roll: 0.004,
        rim: 4,
        back: false,
        reach: 0.4,
    }
}

/// A raised lip along a plate's rim: 0 inside, `h` at the edge, over `w` meters.
fn lip(s: f32, w: f32, h: f32) -> f32 {
    h * smooth(-w, 0.0, s)
}

// ====================================================================== body pieces

use rig::*;

fn ax(p: Vec3) -> Vec3 {
    // Fold the left/right symmetry of a centered piece.
    Vec3::new(p.x.abs(), p.y, p.z)
}

/// A round tube through `path` with per-point radii (fingers, cords), capped.
fn tube(path: &[Vec3], radii: &[f32], sides: usize, side: Vec3) -> Mesh {
    let secs: Vec<Vec<Vec2>> = radii
        .iter()
        .map(|&r| {
            (0..sides)
                .map(|k| {
                    let a = k as f32 / sides as f32 * TAU;
                    Vec2::new(a.cos(), a.sin()) * r
                })
                .collect()
        })
        .collect();
    sweep(path, side, &secs)
}

/// An extruded rounded box: `half` extents, corner radius `r` (in its XY outline), bevel `b`.
fn slab(half: Vec3, r: f32, b: f32) -> Mesh {
    let pts = rounded(
        &[
            (v2(-half.x, -half.y), r),
            (v2(half.x, -half.y), r),
            (v2(half.x, half.y), r),
            (v2(-half.x, half.y), r),
        ],
        3,
    );
    extrude(&pts, half.z * 2.0, b)
}

// ---------------------------------------------------------------------- head

/// The middle of the head (between the ears), from the neck joint.
const HEAD_C: Vec3 = Vec3::new(0.0, 0.158, -0.012);
/// The right eyeball's center (the left mirrors it) and its radius.
const EYE: Vec3 = Vec3::new(0.031, 0.166, -0.081);
const EYE_R: f32 = 0.0122;
/// Where the helmet's parameter rays start: level with the ears, which sit on its poles.
const HELM_O: Vec3 = Vec3::new(0.0, 0.158, 0.0);
/// Helmet shell and brow band thickness.
const HELM_T: f32 = 0.0055;
const BAND_T: f32 = 0.0028;

/// The big forms of the head and face, without features (the coif hugs this).
fn face_base(p: Vec3) -> f32 {
    let r = p - HEAD_C;
    let a = ax(r);
    let skull = ellipsoid(r, v3(0.0, 0.03, 0.008), v3(0.074, 0.09, 0.098));
    let brow = ellipsoid(r, v3(0.0, 0.036, -0.046), v3(0.062, 0.05, 0.05));
    let mid = ellipsoid(r, v3(0.0, -0.036, -0.05), v3(0.052, 0.05, 0.046));
    let cheek = ellipsoid(a, v3(0.044, -0.01, -0.056), v3(0.026, 0.02, 0.027));
    let jaw = capsule(a, v3(0.057, -0.04, 0.004), v3(0.029, -0.096, -0.06), 0.0135);
    let lower = ellipsoid(r, v3(0.0, -0.072, -0.058), v3(0.043, 0.033, 0.036));
    let chin = ellipsoid(r, v3(0.0, -0.097, -0.07), v3(0.025, 0.02, 0.022));
    let ridge = capsule(a, v3(0.0, 0.026, -0.089), v3(0.046, 0.027, -0.079), 0.0085);
    let d = smin(skull, brow, 0.03);
    let d = smin(d, ridge, 0.012);
    let d = smin(d, mid, 0.03);
    let d = smin(d, cheek, 0.02);
    let d = smin(d, jaw, 0.025);
    let d = smin(d, lower, 0.02);
    smin(d, chin, 0.015)
}

/// The face with its brows, eyelids, nose and mouth (head frame).
fn face_form(p: Vec3) -> f32 {
    let r = p - HEAD_C;
    let a = ax(r);
    let e = EYE - HEAD_C;
    let mut d = face_base(p);
    // Orbits under the brow, then lids wrapping the eyeball.
    d = ssub(
        d,
        ellipsoid(a, e + v3(0.0, 0.004, -0.012), v3(0.019, 0.014, 0.014)),
        0.007,
    );
    d = smin(d, sphere(a, e, EYE_R + 0.0016), 0.003);
    // The lid opening: an almond, its outer corner a little higher.
    let q = a - (e + v3(0.0005, -0.0005, -0.011));
    let (s, c) = 0.12f32.sin_cos();
    let q = v3(q.x * c + q.y * s, q.y * c - q.x * s, q.z);
    d = ssub(
        d,
        ellipsoid(q, Vec3::ZERO, v3(0.0126, 0.0041, 0.0125)),
        0.0015,
    );
    // Nose.
    let bridge = round_cone(
        r,
        v3(0.0, 0.012, -0.088),
        v3(0.0, -0.028, -0.111),
        0.0068,
        0.0085,
    );
    let tip = sphere(r, v3(0.0, -0.032, -0.109), 0.0105);
    let wings = sphere(a, v3(0.0125, -0.038, -0.099), 0.0085);
    let nose = smin(smin(bridge, tip, 0.008), wings, 0.005);
    d = smin(d, nose, 0.006);
    let nostril = ellipsoid(a, v3(0.0072, -0.0455, -0.1055), v3(0.0034, 0.0022, 0.0036));
    d = ssub(d, nostril, 0.0015);
    // Mouth.
    let upper = capsule(
        a,
        v3(0.0, -0.0585, -0.0962),
        v3(0.02, -0.0605, -0.091),
        0.0038,
    );
    let lower = capsule(
        a,
        v3(0.0, -0.068, -0.0948),
        v3(0.017, -0.0665, -0.091),
        0.0043,
    );
    d = smin(d, smin(upper, lower, 0.002), 0.005);
    let line = capsule(
        a,
        v3(0.0, -0.0632, -0.106),
        v3(0.0235, -0.0625, -0.096),
        0.0011,
    );
    ssub(d, line, 0.0015)
}

/// Both eyeballs.
fn eyeballs(p: Vec3) -> f32 {
    sphere(ax(p), EYE, EYE_R)
}

/// The eyebrows: a thin layer over the brow ridge.
fn brow_hair(p: Vec3) -> f32 {
    let a = ax(p - HEAD_C);
    let inner = round_cone(
        a,
        v3(0.011, 0.03, -0.096),
        v3(0.03, 0.035, -0.093),
        0.0055,
        0.0048,
    );
    let outer = round_cone(
        a,
        v3(0.03, 0.035, -0.093),
        v3(0.053, 0.03, -0.08),
        0.0048,
        0.0026,
    );
    let region = inner.min(outer);
    if region > 0.005 {
        return region;
    }
    smax(face_form(p) - 0.0022, region, 0.001)
}

/// The coif's opening for the face (negative inside): an oval seen from the front, narrowing
/// to the chin, cut through the front half of the hood.
fn face_hole(p: Vec3) -> f32 {
    let r = p - HEAD_C;
    let hw = 0.054 - 0.02 * smooth(-0.03, -0.115, r.y);
    let (top, bottom, round) = (0.07, -0.123, 0.018);
    let q = v2(
        r.x.abs() - hw + round,
        (r.y - top).max(bottom - r.y) + round,
    );
    let d = q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - round;
    d.max(r.z + 0.035)
}

/// The coif before the face opening is cut.
fn coif_whole(p: Vec3) -> f32 {
    let r = p - HEAD_C;
    let hood = ellipsoid(r, v3(0.0, 0.028, 0.01), v3(0.084, 0.101, 0.108));
    let around = face_base(p) - 0.008;
    let neck = round_cone(p, v3(0.0, -0.04, -0.002), v3(0.0, 0.1, 0.004), 0.058, 0.056);
    let throat = ellipsoid(p, v3(0.0, 0.045, -0.04), v3(0.048, 0.045, 0.045));
    let d = smin(hood, around, 0.02);
    smin(d, smin(neck, throat, 0.03), 0.03)
}

/// The mail coif: a hood over the head and neck, open for the face.
fn coif(p: Vec3) -> f32 {
    ssub(coif_whole(p), face_hole(p), 0.003)
}

/// The surface the helmet shell lies on: a dome flaring a little at the nape and brow.
fn helmet_base(p: Vec3) -> f32 {
    let q = p - HEAD_C - v3(0.0, 0.03, 0.01);
    let dome = ellipsoid(q, Vec3::ZERO, v3(0.091, 0.106, 0.116));
    let flare = 0.014 * smooth(-0.03, -0.1, q.y) * smooth(0.0, 0.1, q.z);
    let brim = 0.005 * smooth(0.035, 0.012, q.y) * smooth(-0.06, -0.1, q.z);
    dome - flare - brim
}

/// The helmet's outer surface (approximately: without the keel or the rim's lip).
fn helmet_outer(p: Vec3) -> f32 {
    helmet_base(p) - HELM_T
}

/// Rays around the helmet: poles through the ears, `u` = 0 straight up and growing toward
/// the face, the seam under the chin.
fn helmet_param() -> Param {
    Param::sph(
        HELM_O,
        basis(v3(0.0, 0.0, -1.0), Vec3::X, v3(0.0, -1.0, 0.0)),
        0.11,
    )
}

/// The full parameter range of [`helmet_param`].
const HELM_UV: (Vec2, Vec2) = (
    Vec2::new(-PI * 0.11, -FRAC_PI_2 * 0.11),
    Vec2::new(PI * 0.11, FRAC_PI_2 * 0.11),
);

/// Horizontal angle of a ray around the head (0 ahead, ±π behind).
fn around(d: Vec3) -> f32 {
    d.x.atan2(-d.z)
}

/// Height of the helmet's rim (as the rise of a unit ray from [`HELM_O`]) at a horizontal
/// angle: just above the brows in front, dipping to the ears, low at the nape.
fn rim_rise(phi: f32) -> f32 {
    let f = phi.abs();
    0.44 - 0.12 * smooth(0.0, 0.9, f) - 0.53 * smooth(0.9, 1.75, f) - 0.41 * smooth(1.75, 2.9, f)
}

/// How far below the rim (meters, negative under the shell) a ray meets the helmet.
fn rim_dist(d: Vec3) -> f32 {
    (rim_rise(around(d)) - d.y) * 0.11
}

/// The helmet shell's outline: above the rim, with holes over the ears.
fn helmet_outline(param: Param, uv: Vec2) -> f32 {
    let (_, d) = param.ray(uv);
    let hole = (0.29 - d.x.abs().min(1.0).acos()) * 0.1;
    rim_dist(d).max(hole)
}

/// The gunmetal brow band along the front of the rim.
fn band_outline(param: Param, uv: Vec2) -> f32 {
    let (_, d) = param.ray(uv);
    let s = rim_dist(d);
    (s + 0.006)
        .max(-s - 0.026)
        .max((around(d).abs() - 1.3) * 0.1)
}

/// The glass strip in the brow band (the alert light).
fn lamp_outline(param: Param, uv: Vec2) -> f32 {
    let (_, d) = param.ray(uv);
    let s = rim_dist(d);
    ((s + 0.016).abs() - 0.0032).max((around(d).abs() - 0.5) * 0.11)
}

/// A domed rivet head, axis +Y.
fn rivet(r: f32) -> Mesh {
    lathe(
        &[
            v2(0.0, r * 0.62),
            v2(r * 0.55, r * 0.5),
            v2(r * 0.92, r * 0.22),
            v2(r, -r * 0.3),
        ],
        10,
        &[],
    )
}

/// Keep the triangles whose center passes `pick`.
fn select(m: &Mesh, pick: &dyn Fn(Vec3) -> bool) -> Mesh {
    let mut idx = Vec::new();
    for t in m.indices.chunks(3) {
        let c = t
            .iter()
            .map(|&i| Vec3::from(m.vertices[i as usize].position))
            .sum::<Vec3>()
            / 3.0;
        if pick(c) {
            idx.extend_from_slice(t);
        }
    }
    let mut remap = vec![u32::MAX; m.vertices.len()];
    let mut verts = Vec::new();
    for i in &mut idx {
        let r = &mut remap[*i as usize];
        if *r == u32::MAX {
            *r = verts.len() as u32;
            verts.push(m.vertices[*i as usize]);
        }
        *i = *r;
    }
    Mesh::new(verts, idx)
}

/// The helmet shell (team paint) with its brow band, ear discs and rivets, over the mail coif.
fn helmet() -> PieceMesh {
    let param = helmet_param();
    let mut b = Builder::default();
    let outline = |uv: Vec2| helmet_outline(param, uv);
    let height = |uv: Vec2, s: f32| {
        // A keel along the middle and a pair of shallow flutes beside it, both ending above
        // the brow band, and a rolled lip.
        let above = smooth(-0.026, -0.036, s);
        let keel = 0.0035 * (-(uv.y / 0.009).powi(2)).exp();
        let flute = 0.0015 * (-((uv.y.abs() - 0.035) / 0.008).powi(2)).exp();
        HELM_T + (keel + flute) * above + lip(s, 0.012, 0.0022)
    };
    let shell = Plate {
        reach: 0.3,
        ..plate(
            param,
            &helmet_base,
            0.0,
            &outline,
            &height,
            HELM_UV.0,
            HELM_UV.1,
            0.008,
        )
    };
    b.add(shell.mesh(), Zone::Paint, Tint::Team(1.0));
    let band_out = |uv: Vec2| band_outline(param, uv);
    let band_h = |_: Vec2, s: f32| BAND_T + lip(s, 0.004, 0.0008);
    let band = Plate {
        roll: 0.0015,
        rim: 2,
        reach: 0.3,
        ..plate(
            param,
            &helmet_outer,
            0.0,
            &band_out,
            &band_h,
            HELM_UV.0,
            HELM_UV.1,
            0.004,
        )
    };
    b.add(band.mesh(), Zone::Metal, Tint::Rgb(lin(STEEL) * 0.6));
    // Rivets along the band, beside the light.
    let on_band = |p: Vec3| helmet_outer(p) - BAND_T;
    for phi in [-1.05f32, -0.72, 0.72, 1.05] {
        let y = rim_rise(phi) + 0.016 / 0.11;
        let h = (1.0 - y * y).max(0.0).sqrt();
        let (hit, n) = cast(&on_band, HELM_O, v3(phi.sin() * h, y, -phi.cos() * h), 0.3);
        let m = placed(rivet(0.0038), Quat::from_rotation_arc(Vec3::Y, n), hit);
        b.add(m, Zone::Metal, Tint::Rgb(lin(STEEL) * 0.8));
    }
    // Ear discs over the shell's poles.
    let pod = lathe(
        &[
            v2(0.0, 0.01),
            v2(0.014, 0.01),
            v2(0.017, 0.007),
            v2(0.026, 0.0055),
            v2(0.033, 0.002),
            v2(0.036, -0.006),
        ],
        24,
        &[2, 3],
    );
    for side in [-1.0f32, 1.0] {
        let (hit, n) = cast(&helmet_outer, HELM_O, v3(side, 0.0, 0.0), 0.3);
        let m = placed(pod.clone(), Quat::from_rotation_arc(Vec3::Y, n), hit);
        b.add(m, Zone::Metal, Tint::Rgb(lin(GUNMETAL)));
    }
    // The mail coif under it, edged in leather around the face.
    // Above the brow it is all under the helmet; it runs on 15 mm under the rim.
    let mail = sdf_mesh_tol(
        &coif,
        v3(-0.1, -0.06, -0.13),
        v3(0.1, 0.225, 0.13),
        0.0045,
        0.0015,
    );
    let mail = cull(mail, &helmet_base, 0.015);
    let mail = cull(mail, &face_form, 0.001);
    let trim = |p: Vec3| coif_whole(p) < -0.0012 || face_hole(p) < 0.004;
    b.add(
        select(&mail, &|p| !trim(p)),
        Zone::Mail,
        Tint::Rgb(lin(STEEL) * 0.85),
    );
    b.add(
        select(&mail, &trim),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER_DARK)),
    );
    let field = |p: Vec3| shell.proxy(p).min(band.proxy(p)).min(coif(p));
    b.finish(&field, 1.0)
}

/// The face, its eyes and brows. The skin takes its tone from the "team" color.
fn face() -> PieceMesh {
    let mut b = Builder::default();
    let skin = sdf_mesh_tol(
        &face_form,
        v3(-0.066, 0.03, -0.145),
        v3(0.066, 0.215, -0.04),
        0.0025,
        0.0008,
    );
    let skin = cull(cull(skin, &coif, 0.0015), &eyeballs, 0.0005);
    let paint = |p: Vec3| {
        let a = ax(p - HEAD_C);
        let e = EYE - HEAD_C;
        let mouth = capsule(a, v3(0.0, -0.063, -0.1), v3(0.02, -0.0625, -0.094), 0.0055);
        let lips = 1.0 - smooth(0.001, 0.005, mouth);
        let flush = 1.0 - smooth(0.0, 0.02, sphere(a, v3(0.045, -0.025, -0.078), 0.01));
        let lash =
            (1.0 - smooth(0.0006, 0.0028, sphere(a, e, EYE_R))) * smooth(-0.004, 0.0, a.y - e.y);
        let (wl, wf, wd) = (0.28 * lips, 0.1 * flush * (1.0 - lips), 0.7 * lash);
        let k = (1.0 - wl - wf - wd).max(0.0);
        let c = (lin(0x7a3b38) * wl + lin(0xb5655a) * wf + lin(0x1a1210) * wd)
            / (wl + wf + wd).max(1e-4);
        Tint::Dyed(c, k)
    };
    b.add_fn(skin, Zone::Skin, &paint);
    // Eyes: white, iris and pupil, looking ahead.
    let iris_r = 0.0062;
    let pupil_r = 0.0026;
    let on_ball = |r: f32| (EYE_R * EYE_R - r * r).sqrt();
    let t0 = (iris_r / EYE_R).asin();
    let white: Vec<Vec2> = (0..=8)
        .map(|k| {
            let t = t0 + (PI - t0) * k as f32 / 8.0;
            v2(EYE_R * t.sin(), EYE_R * t.cos())
        })
        .collect();
    let iris = [
        v2(pupil_r, on_ball(pupil_r) + 0.0005),
        v2(iris_r, on_ball(iris_r)),
    ];
    let pupil = [
        v2(0.0, EYE_R + 0.0007),
        v2(pupil_r, on_ball(pupil_r) + 0.0005),
    ];
    let to_front = Quat::from_rotation_x(-FRAC_PI_2);
    // Matte white and iris; the pupil is glossy and catches the light.
    for (profile, color, zone) in [
        (&white[..], 0xc2b8ad, Zone::Skin),
        (&iris[..], 0x5a422c, Zone::Skin),
        (&pupil[..], 0x060606, Zone::Glass),
    ] {
        let right = placed(lathe(profile, 16, &[]), to_front, EYE);
        b.add(mirrored(&right), zone, Tint::Rgb(lin(color)));
        b.add(right, zone, Tint::Rgb(lin(color)));
    }
    let brows = sdf_mesh_tol(
        &brow_hair,
        v3(-0.062, 0.176, -0.113),
        v3(0.062, 0.205, -0.08),
        0.0016,
        0.0006,
    );
    let brows = cull(brows, &face_form, 0.0004);
    b.add(brows, Zone::Hair, Tint::Dyed(lin(0x2a1d16), 0.15));
    let param = helmet_param();
    let outline = |uv: Vec2| helmet_outline(param, uv);
    let height = |_: Vec2, _: f32| HELM_T;
    let shell = plate(
        param,
        &helmet_base,
        0.0,
        &outline,
        &height,
        HELM_UV.0,
        HELM_UV.1,
        0.006,
    );
    let field = |p: Vec3| face_form(p).min(coif(p)).min(shell.proxy(p));
    b.finish(&field, 1.0)
}

/// The brow light (glass in the brow band; it glows with the alert level).
fn visor() -> PieceMesh {
    let param = helmet_param();
    let base = |p: Vec3| helmet_outer(p) - BAND_T;
    let outline = |uv: Vec2| lamp_outline(param, uv);
    let height = |_: Vec2, s: f32| 0.0012 + lip(s, 0.0015, 0.0004);
    let pl = Plate {
        roll: 0.0008,
        rim: 2,
        reach: 0.3,
        ..plate(
            param, &base, 0.0, &outline, &height, HELM_UV.0, HELM_UV.1, 0.003,
        )
    };
    let mut b = Builder::default();
    b.add(pl.mesh(), Zone::Glass, Tint::Rgb(lin(GLASS)));
    b.finish(&|p| pl.proxy(p), 0.0)
}

/// The rank crest: a brush of horsehair along the helmet's keel.
fn crest() -> PieceMesh {
    // The keel's distance from the helmet's center by angle (0 up, π/2 ahead).
    let keel = |p: Vec3| helmet_outer(p) - 0.0035;
    let n = 96;
    let table: Vec<f32> = (0..=n)
        .map(|k| {
            let a = -PI + TAU * k as f32 / n as f32;
            let (hit, _) = cast(&keel, HELM_O, v3(0.0, a.cos(), -a.sin()), 0.3);
            (hit - HELM_O).length()
        })
        .collect();
    let radius = |a: f32| {
        let t = ((a + PI) / TAU * n as f32).clamp(0.0, n as f32 - 1e-3);
        let i = t.floor() as usize;
        let f = t - i as f32;
        table[i] * (1.0 - f) + table[i + 1] * f
    };
    let (a0, a1) = (0.8f32, -1.75f32);
    let (mid, half) = ((a0 + a1) * 0.5, (a0 - a1) * 0.5);
    let brush = |p: Vec3| {
        let q = p - HELM_O;
        let rho = Vec2::new(q.y, q.z).length();
        let a = (-q.z).atan2(q.y);
        let t = rho - radius(a) + 0.002;
        let k = ((a - mid) / half).clamp(-1.0, 1.0);
        let tuft = 1.0 + 0.04 * (a * 13.0 + 0.5).sin() + 0.025 * (a * 29.0 + 2.1).sin();
        let h = (0.056 * (1.0 - k.powi(4)) + 0.008) * tuft;
        let w = 0.0105 + 0.0035 * (t / h).clamp(0.0, 1.0);
        let r = w.min(h * 0.5) * 0.95;
        let cross = box2(v2(q.x, t), v2(0.0, h * 0.5), v2(w, h * 0.5), r);
        let ends = ((a - mid).abs() - half) * rho;
        smax(cross, ends, 0.008)
    };
    let m = sdf_mesh_tol(
        &brush,
        v3(-0.022, 0.11, -0.17),
        v3(0.022, 0.38, 0.19),
        0.0035,
        0.0015,
    );
    let m = cull(m, &keel, 0.003);
    let mut b = Builder::default();
    b.add(m, Zone::Hair, Tint::Rgb(Vec3::ONE));
    b.finish(&|p| brush(p).min(keel(p)), 0.8)
}

// ---------------------------------------------------------------------- torso

/// Chest base for plates (chest frame, origin at the waist joint).
fn torso_base(p: Vec3) -> f32 {
    let rib = ellipsoid(p, v3(0.0, 0.25, 0.0), v3(0.163, 0.205, 0.122));
    let pec = ellipsoid(ax(p), v3(0.07, 0.3, -0.035), v3(0.1, 0.1, 0.09));
    let abd = ellipsoid(p, v3(0.0, 0.08, 0.0), v3(0.138, 0.12, 0.108));
    smin(smin(rib, pec, 0.04), abd, 0.06)
}

fn chest_suit(p: Vec3) -> f32 {
    let body = torso_base(p) + 0.006;
    let a = ax(p);
    let trap = capsule(
        a,
        v3(0.03, 0.415, 0.03),
        SHOULDER + v3(-0.02, 0.03, 0.0),
        0.05,
    );
    let delt = sphere(a, SHOULDER, 0.06);
    let neck = capsule(p, v3(0.0, 0.36, 0.02), v3(0.0, 0.47, 0.01), 0.056);
    let lat = ellipsoid(a, v3(0.1, 0.22, 0.04), v3(0.07, 0.14, 0.08));
    let d = smin(smin(body, trap, 0.05), delt, 0.04);
    smin(smin(d, neck, 0.03), lat, 0.04)
}

fn chest() -> PieceMesh {
    let mut b = Builder::default();
    let base = |p: Vec3| torso_base(p);
    // Breastplate with a center crease, scooped neck and arm holes.
    let front = Param::cyl(Vec3::ZERO, Quat::IDENTITY, 0.15);
    let breast_out = |uv: Vec2| {
        let bottom = 0.17 + 0.06 * (uv.x / 0.17).powi(2) - uv.y;
        let d = box2(uv, v2(0.0, 0.29), v2(0.175, 0.12), 0.03).max(bottom);
        let d = d.max(-circle2(uv, v2(0.0, 0.47), 0.085));
        d.max(-circle2(uv, v2(0.205, 0.415), 0.07))
            .max(-circle2(uv, v2(-0.205, 0.415), 0.07))
    };
    let breast_h = |uv: Vec2, s: f32| {
        0.011
            + 0.006 * (-(uv.x / 0.035).powi(2)).exp() * smooth(0.12, 0.3, uv.y)
            + lip(s, 0.014, 0.003)
    };
    let breast = plate(
        front,
        &base,
        0.006,
        &breast_out,
        &breast_h,
        v2(-0.23, 0.13),
        v2(0.23, 0.45),
        0.013,
    );
    // Back plate.
    let back = Param::cyl(Vec3::ZERO, Quat::from_rotation_y(PI), 0.15);
    let back_out = |uv: Vec2| {
        let d = box2(uv, v2(0.0, 0.27), v2(0.165, 0.13), 0.03);
        let d = d.max(-circle2(uv, v2(0.0, 0.47), 0.08));
        d.max(-circle2(uv, v2(0.2, 0.43), 0.07))
            .max(-circle2(uv, v2(-0.2, 0.43), 0.07))
    };
    let back_h =
        |uv: Vec2, s: f32| 0.01 + 0.004 * (-(uv.x / 0.02).powi(2)).exp() + lip(s, 0.012, 0.0025);
    let backplate = plate(
        back,
        &base,
        0.006,
        &back_out,
        &back_h,
        v2(-0.22, 0.12),
        v2(0.22, 0.45),
        0.015,
    );
    // Two abdominal lames, the upper one over the lower.
    let lame_out = |lo: f32, hi: f32| {
        move |uv: Vec2| {
            box2(
                uv,
                v2(0.0, (lo + hi) * 0.5),
                v2(0.135 + 0.01 * hi, (hi - lo) * 0.5),
                0.02,
            )
        }
    };
    let lame1_out = lame_out(0.105, 0.185);
    let lame2_out = lame_out(0.035, 0.115);
    let lame1_h = |_: Vec2, s: f32| 0.0085 + lip(s, 0.01, 0.002);
    let lame2_h = |_: Vec2, s: f32| 0.0055 + lip(s, 0.01, 0.002);
    let lame1 = plate(
        front,
        &base,
        0.006,
        &lame1_out,
        &lame1_h,
        v2(-0.17, 0.09),
        v2(0.17, 0.2),
        0.013,
    );
    let lame2 = plate(
        front,
        &base,
        0.006,
        &lame2_out,
        &lame2_h,
        v2(-0.17, 0.02),
        v2(0.17, 0.13),
        0.013,
    );
    // Collar: a standing gorget open at the back.
    let neck_base = |p: Vec3| capsule(p, v3(0.0, 0.3, 0.02), v3(0.0, 0.5, 0.01), 0.062);
    let collar_param = Param::cyl(Vec3::ZERO, Quat::IDENTITY, 0.065);
    let collar_out = |uv: Vec2| box2(uv, v2(0.0, 0.43), v2(0.155, 0.032), 0.012);
    let collar_h = |_: Vec2, s: f32| 0.008 + lip(s, 0.008, 0.002);
    let collar = Plate {
        back: true,
        ..plate(
            collar_param,
            &neck_base,
            0.004,
            &collar_out,
            &collar_h,
            v2(-0.2, 0.38),
            v2(0.2, 0.48),
            0.012,
        )
    };
    // Leather straps over the shoulders joining breast and back plates.
    let strap_param =
        |x: f32| Param::cyl(v3(x, 0.33, 0.02), Quat::from_rotation_z(-FRAC_PI_2), 0.1);
    let strap_out = |uv: Vec2| box2(uv, v2(-0.155, 0.0), v2(0.07, 0.018), 0.004);
    let strap_h = |_: Vec2, _: f32| 0.004;
    let plates = |p: Vec3| {
        breast
            .proxy(p)
            .min(backplate.proxy(p))
            .min(lame1.proxy(p))
            .min(lame2.proxy(p))
            .min(collar.proxy(p))
    };
    let suit = cull(
        sdf_mesh(
            &chest_suit,
            v3(-0.27, -0.14, -0.2),
            v3(0.27, 0.52, 0.2),
            0.015,
        ),
        &plates,
        0.008,
    );
    b.add(suit, Zone::Suit, Tint::Dyed(lin(SUIT), SUIT_DYE));
    b.add(breast.mesh(), Zone::Paint, Tint::Team(1.0));
    b.add(backplate.mesh(), Zone::Paint, Tint::Team(0.9));
    b.add(lame1.mesh(), Zone::Paint, Tint::Team(0.95));
    b.add(lame2.mesh(), Zone::Paint, Tint::Team(0.9));
    b.add(collar.mesh(), Zone::Metal, Tint::Rgb(lin(GUNMETAL)));
    for side in [-1.0f32, 1.0] {
        let st = Plate {
            roll: 0.002,
            rim: 2,
            ..plate(
                strap_param(0.105 * side),
                &chest_suit,
                0.002,
                &strap_out,
                &strap_h,
                v2(-0.26, -0.03),
                v2(-0.05, 0.03),
                0.01,
            )
        };
        b.add(st.mesh(), Zone::Leather, Tint::Rgb(lin(LEATHER)));
    }
    let field = |p: Vec3| chest_suit(p).min(plates(p));
    b.finish(&field, 1.0)
}

// ---------------------------------------------------------------------- pelvis

fn pelvis_base(p: Vec3) -> f32 {
    let a = ax(p);
    let hips = ellipsoid(p, v3(0.0, 0.03, 0.01), v3(0.16, 0.12, 0.115));
    let glute = ellipsoid(a, v3(0.07, -0.02, 0.05), v3(0.085, 0.1, 0.075));
    let belly = ellipsoid(p, v3(0.0, 0.1, -0.005), v3(0.14, 0.09, 0.105));
    smin(smin(hips, glute, 0.04), belly, 0.05)
}

fn pelvis_suit(p: Vec3) -> f32 {
    let a = ax(p);
    let ball = sphere(a, v3(HIP_X, 0.0, 0.0), 0.088);
    let crotch = capsule(p, v3(-0.05, -0.07, 0.0), v3(0.05, -0.07, 0.0), 0.055);
    smin(smin(pelvis_base(p), ball, 0.04), crotch, 0.05)
}

fn pelvis() -> PieceMesh {
    let mut b = Builder::default();
    // Belt all the way round, its ends meeting under the buckle at the front.
    let around = Param::cyl(Vec3::ZERO, Quat::from_rotation_y(PI), 0.15);
    let half = PI * 0.15;
    let belt_out = |uv: Vec2| box2(uv, v2(0.0, 0.1), v2(half - 0.012, 0.026), 0.006);
    let belt_h = |_: Vec2, s: f32| 0.008 + lip(s, 0.006, 0.0015);
    let belt = Plate {
        roll: 0.003,
        rim: 3,
        ..plate(
            around,
            &pelvis_base,
            0.004,
            &belt_out,
            &belt_h,
            v2(-half, 0.06),
            v2(half, 0.14),
            0.016,
        )
    };
    let suit = cull(
        sdf_mesh(
            &pelvis_suit,
            v3(-0.24, -0.16, -0.17),
            v3(0.24, 0.22, 0.19),
            0.015,
        ),
        &|p| belt.proxy(p),
        0.006,
    );
    b.add(suit, Zone::Suit, Tint::Dyed(lin(SUIT), SUIT_DYE));
    b.add(belt.mesh(), Zone::Leather, Tint::Rgb(lin(LEATHER_DARK)));
    // Buckle: a brass frame over the belt's ends.
    let front_z = -0.118;
    let frame = rounded(
        &[
            (v2(-0.035, -0.03), 0.008),
            (v2(0.035, -0.03), 0.008),
            (v2(0.035, 0.03), 0.008),
            (v2(-0.035, 0.03), 0.008),
        ],
        3,
    );
    let buckle = placed(
        extrude(&frame, 0.012, 0.003),
        Quat::IDENTITY,
        v3(0.0, 0.1, front_z - 0.012),
    );
    b.add(buckle, Zone::Metal, Tint::Rgb(lin(BRASS)));
    let plate_in = placed(
        slab(v3(0.022, 0.018, 0.004), 0.004, 0.0015),
        Quat::IDENTITY,
        v3(0.0, 0.1, front_z - 0.02),
    );
    b.add(plate_in, Zone::Metal, Tint::Rgb(lin(GUNMETAL)));
    // Pouches on the hips and the back.
    for (x, z, yaw, size) in [
        (0.155, -0.04, -1.2f32, v3(0.04, 0.05, 0.022)),
        (-0.155, -0.04, 1.2, v3(0.04, 0.05, 0.022)),
        (0.09, 0.12, -2.6, v3(0.045, 0.045, 0.02)),
    ] {
        let rot = Quat::from_rotation_y(yaw);
        let at = v3(x, 0.06, z);
        let body = placed(slab(size, 0.012, 0.006), rot, at);
        let flap = placed(
            slab(v3(size.x + 0.003, size.y * 0.42, 0.004), 0.008, 0.002),
            rot,
            at + rot * v3(0.0, size.y * 0.6, -size.z - 0.002),
        );
        b.add(body, Zone::Leather, Tint::Rgb(lin(0x5a4330)));
        b.add(flap, Zone::Leather, Tint::Rgb(lin(0x4a3628)));
    }
    let field = |p: Vec3| pelvis_suit(p).min(belt.proxy(p));
    b.finish(&field, 1.0)
}

/// Tabard flap hanging from the belt; origin at its hinge, hanging along −Y, facing −Z
/// (`front`) or +Z.
fn tabard(front: bool) -> PieceMesh {
    let len = if front { 0.43 } else { 0.4 };
    let base = |p: Vec3| (Vec2::new(p.x, p.z) - Vec2::new(0.0, 0.32)).length() - 0.32;
    let param = Param::cyl(v3(0.0, 0.0, 0.32), Quat::IDENTITY, 0.32);
    let (w0, w1) = (0.12, 0.145);
    let outline = move |uv: Vec2| {
        let t = (-uv.y / len).clamp(0.0, 1.0);
        let w = w0 + (w1 - w0) * t;
        let side = uv.x.abs() - w;
        // Shallow swallowtail at the bottom.
        let bottom = -len + 0.04 * (uv.x.abs() / w1) - uv.y;
        side.max(uv.y).max(bottom)
    };
    let folds = move |uv: Vec2, s: f32| {
        let t = (-uv.y / len).clamp(0.0, 1.0);
        let pleat = (uv.x * 46.0 + 0.6).sin() * 0.5 + (uv.x * 23.0 - 0.4).sin() * 0.5;
        0.004 + pleat * 0.006 * t * t + lip(s, 0.01, 0.0012)
    };
    let pl = Plate {
        back: true,
        roll: 0.002,
        rim: 2,
        ..plate(
            param,
            &base,
            0.0,
            &outline,
            &folds,
            v2(-0.16, -len - 0.01),
            v2(0.16, 0.01),
            0.015,
        )
    };
    // Trim band at the hem and the team emblem (front only).
    let trim_out = move |uv: Vec2| {
        let o = outline(uv);
        o.max(-(o + 0.022)).max(uv.y + len - 0.09)
    };
    let trim_h = move |uv: Vec2, s: f32| folds(uv, s.min(-0.01)) + 0.0014;
    let trim = Plate {
        roll: 0.001,
        rim: 2,
        ..plate(
            param,
            &base,
            0.0,
            &trim_out,
            &trim_h,
            v2(-0.16, -len - 0.01),
            v2(0.16, -len + 0.11),
            0.01,
        )
    };
    let emblem_out = |uv: Vec2| {
        let q = uv - v2(0.0, -0.15);
        // A chevron over a diamond.
        let diamond = (q.x.abs() + (q.y + 0.025).abs()) - 0.035;
        let chev = (q.x.abs() * 0.75 - (q.y - 0.04)).abs() - 0.012;
        let chev = chev.max(q.x.abs() - 0.07).max(q.y - 0.07);
        diamond.min(chev)
    };
    let emblem_h = move |uv: Vec2, s: f32| folds(uv, s.min(-0.01)) + 0.0016;
    let emblem = Plate {
        roll: 0.0008,
        rim: 1,
        ..plate(
            param,
            &base,
            0.0,
            &emblem_out,
            &emblem_h,
            v2(-0.09, -0.25),
            v2(0.09, -0.06),
            0.007,
        )
    };
    let mut b = Builder::default();
    b.add(pl.mesh(), Zone::Cloth, Tint::Team(0.85));
    b.add(trim.mesh(), Zone::Cloth, Tint::Rgb(lin(CREAM) * 0.8));
    if front {
        b.add(emblem.mesh(), Zone::Cloth, Tint::Rgb(lin(CREAM) * 0.85));
    }
    let mut out = b.finish(&|p| pl.proxy(p), 0.6);
    if !front {
        out.mesh.transform(&Mat4::from_rotation_y(PI));
    }
    out
}

// ---------------------------------------------------------------------- legs (right side)

fn thigh_base(p: Vec3) -> f32 {
    let leg = round_cone(
        p,
        v3(0.0, -0.02, 0.0),
        v3(0.0, -THIGH + 0.02, 0.0),
        0.083,
        0.06,
    );
    let quad = ellipsoid(p, v3(0.008, -0.17, -0.02), v3(0.07, 0.15, 0.066));
    smin(leg, quad, 0.04)
}

fn thigh_suit(p: Vec3) -> f32 {
    let ham = ellipsoid(p, v3(-0.004, -0.2, 0.022), v3(0.064, 0.15, 0.06));
    let hip = sphere(p, Vec3::ZERO, 0.086);
    let knee = sphere(p, v3(0.0, -THIGH, 0.0), 0.056);
    smin(
        smin(smin(thigh_base(p) - 0.002, ham, 0.04), hip, 0.03),
        knee,
        0.04,
    )
}

fn thigh() -> PieceMesh {
    let mut b = Builder::default();
    let param = Param::cyl(Vec3::ZERO, Quat::IDENTITY, 0.08);
    // Cuisse: front and outer thigh, higher on the outside, with a center ridge.
    let cuisse_out = |uv: Vec2| {
        let top = uv.y - (-0.1 + 0.05 * (uv.x / 0.1));
        let d = box2(uv, v2(0.025, -0.22), v2(0.085, 0.16), 0.03).max(top);
        d.max(-0.36 - uv.y)
    };
    let cuisse_h = |uv: Vec2, s: f32| {
        0.009 + 0.004 * (-((uv.x - 0.02) / 0.025).powi(2)).exp() + lip(s, 0.012, 0.0025)
    };
    let cuisse = plate(
        param,
        &thigh_base,
        0.005,
        &cuisse_out,
        &cuisse_h,
        v2(-0.08, -0.4),
        v2(0.13, -0.04),
        0.013,
    );
    // A lame below it over the knee top.
    let lame_out = |uv: Vec2| box2(uv, v2(0.015, -0.385), v2(0.07, 0.03), 0.015);
    let lame_h = |_: Vec2, s: f32| 0.007 + lip(s, 0.008, 0.002);
    let lame = plate(
        param,
        &thigh_base,
        0.004,
        &lame_out,
        &lame_h,
        v2(-0.07, -0.43),
        v2(0.1, -0.34),
        0.011,
    );
    // Strap round the back of the thigh.
    let back = Param::cyl(Vec3::ZERO, Quat::from_rotation_y(PI), 0.08);
    let strap_out = |uv: Vec2| box2(uv, v2(0.0, -0.26), v2(0.12, 0.016), 0.004);
    let strap_h = |_: Vec2, _: f32| 0.004;
    let strap = Plate {
        roll: 0.002,
        rim: 2,
        ..plate(
            back,
            &thigh_suit,
            0.001,
            &strap_out,
            &strap_h,
            v2(-0.15, -0.29),
            v2(0.15, -0.23),
            0.012,
        )
    };
    let plates = |p: Vec3| cuisse.proxy(p).min(lame.proxy(p));
    let suit = cull(
        sdf_mesh(
            &thigh_suit,
            v3(-0.11, -0.52, -0.11),
            v3(0.11, 0.1, 0.11),
            0.014,
        ),
        &plates,
        0.008,
    );
    b.add(suit, Zone::Suit, Tint::Dyed(lin(SUIT), SUIT_DYE));
    b.add(cuisse.mesh(), Zone::Paint, Tint::Team(1.0));
    b.add(lame.mesh(), Zone::Paint, Tint::Team(0.9));
    b.add(strap.mesh(), Zone::Leather, Tint::Rgb(lin(LEATHER)));
    let field = |p: Vec3| thigh_suit(p).min(plates(p));
    b.finish(&field, 1.0)
}

fn shin_suit(p: Vec3) -> f32 {
    let knee = sphere(p, Vec3::ZERO, 0.054);
    let leg = round_cone(p, Vec3::ZERO, v3(0.0, -SHIN + 0.03, 0.0), 0.05, 0.037);
    let calf = ellipsoid(p, v3(0.0, -0.12, 0.022), v3(0.052, 0.11, 0.05));
    smin(smin(knee, leg, 0.03), calf, 0.04)
}

fn boot_shaft(p: Vec3) -> f32 {
    let shaft = round_cone(
        p,
        v3(0.0, -SHIN - 0.01, 0.006),
        v3(0.0, -0.215, 0.012),
        0.05,
        0.052,
    );
    smax(shaft, -0.225 - p.y, 0.004)
}

fn shin() -> PieceMesh {
    let mut b = Builder::default();
    let param = Param::cyl(Vec3::ZERO, Quat::IDENTITY, 0.05);
    let base = |p: Vec3| shin_suit(p).min(boot_shaft(p) - 0.002);
    // Greave over the shin, flaring over the boot.
    let greave_out = |uv: Vec2| {
        let w = 0.055 + 0.012 * smooth(-0.2, -0.38, uv.y);
        let d = (uv.x.abs() - w).max(uv.y + 0.075).max(-0.39 - uv.y);
        d.max(box2(uv, v2(0.0, -0.23), v2(0.1, 0.16), 0.025))
    };
    let greave_h =
        |uv: Vec2, s: f32| 0.008 + 0.004 * (-(uv.x / 0.018).powi(2)).exp() + lip(s, 0.01, 0.0025);
    let greave = plate(
        param,
        &base,
        0.004,
        &greave_out,
        &greave_h,
        v2(-0.09, -0.41),
        v2(0.09, -0.05),
        0.011,
    );
    // Knee cop: a rounded cap with side wings.
    let kparam = Param::sph(v3(0.0, -0.005, 0.0), Quat::IDENTITY, 0.06);
    let knee_out = |uv: Vec2| {
        let cap = circle2(uv, v2(0.0, 0.0), 0.042);
        let wings = box2(uv, v2(0.0, -0.012), v2(0.07, 0.016), 0.012);
        cap.min(wings)
    };
    let knee_h = |uv: Vec2, s: f32| {
        0.008 + 0.004 * (1.0 - (uv.length() / 0.04).min(1.0)) + lip(s, 0.008, 0.002)
    };
    let kcop = Plate {
        back: true,
        ..plate(
            kparam,
            &shin_suit,
            0.004,
            &knee_out,
            &knee_h,
            v2(-0.09, -0.06),
            v2(0.09, 0.05),
            0.009,
        )
    };
    let plates = |p: Vec3| greave.proxy(p).min(kcop.proxy(p));
    let suit = cull(
        sdf_mesh(
            &shin_suit,
            v3(-0.08, -0.48, -0.08),
            v3(0.08, 0.07, 0.09),
            0.013,
        ),
        &|p| plates(p).min(boot_shaft(p)),
        0.006,
    );
    b.add(suit, Zone::Suit, Tint::Dyed(lin(SUIT), SUIT_DYE));
    let shaft = cull(
        sdf_mesh(
            &boot_shaft,
            v3(-0.07, -0.5, -0.07),
            v3(0.07, -0.2, 0.08),
            0.011,
        ),
        &plates,
        0.006,
    );
    b.add(shaft, Zone::Leather, Tint::Rgb(lin(LEATHER_DARK)));
    b.add(greave.mesh(), Zone::Paint, Tint::Team(0.95));
    b.add(kcop.mesh(), Zone::Metal, Tint::Rgb(lin(GUNMETAL)));
    let field = |p: Vec3| base(p).min(plates(p));
    b.finish(&field, 1.0)
}

fn boot_upper(p: Vec3) -> f32 {
    let ankle = sphere(p, Vec3::ZERO, 0.047);
    let heel = ellipsoid(p, v3(0.0, -0.035, 0.035), v3(0.043, 0.045, 0.045));
    let foot = round_cone(
        p,
        v3(0.0, -0.035, 0.02),
        v3(0.004, -0.045, -0.13),
        0.045,
        0.038,
    );
    let toe = ellipsoid(p, v3(0.006, -0.048, -0.15), v3(0.046, 0.03, 0.06));
    let d = smin(smin(smin(ankle, heel, 0.03), foot, 0.03), toe, 0.03);
    smax(d, -ANKLE_Y + 0.012 - p.y, 0.006)
}

fn toe_cap(p: Vec3) -> f32 {
    smax(boot_upper(p) - 0.003, p.z + 0.125, 0.012)
}

fn foot() -> PieceMesh {
    let mut b = Builder::default();
    let upper = cull(
        sdf_mesh(
            &boot_upper,
            v3(-0.07, -0.08, -0.23),
            v3(0.07, 0.06, 0.1),
            0.0085,
        ),
        &toe_cap,
        0.0005,
    );
    b.add(upper, Zone::Leather, Tint::Rgb(lin(LEATHER_DARK)));
    let cap = cull(
        sdf_mesh(
            &toe_cap,
            v3(-0.06, -0.08, -0.23),
            v3(0.07, 0.0, -0.1),
            0.008,
        ),
        &boot_upper,
        0.006,
    );
    b.add(cap, Zone::Polymer, Tint::Rgb(lin(POLYMER)));
    // Sole: an extruded footprint, heel block and lug edge.
    let print = rounded(
        &[
            (v2(0.0, -0.1), 0.03),
            (v2(0.038, -0.075), 0.03),
            (v2(0.042, 0.0), 0.05),
            (v2(0.052, 0.1), 0.04),
            (v2(0.034, 0.178), 0.025),
            (v2(-0.002, 0.192), 0.02),
            (v2(-0.034, 0.17), 0.03),
            (v2(-0.046, 0.1), 0.04),
            (v2(-0.032, 0.0), 0.05),
            (v2(-0.036, -0.075), 0.03),
        ],
        3,
    );
    let sole = extrude(&print, 0.02, 0.004);
    // Profile x → x, profile y → forward (−z), depth → up.
    let sole = placed(
        sole,
        basis(Vec3::X, -Vec3::Z, Vec3::Y),
        v3(0.004, -ANKLE_Y + 0.01, -0.0),
    );
    b.add(sole, Zone::Polymer, Tint::Rgb(lin(RUBBER)));
    let field = |p: Vec3| boot_upper(p).min(toe_cap(p));
    b.finish(&field, 1.0)
}

// ---------------------------------------------------------------------- arms (right side)

fn upper_arm_suit(p: Vec3) -> f32 {
    let delt = ellipsoid(p, v3(0.012, -0.04, 0.0), v3(0.066, 0.09, 0.064));
    let ball = sphere(p, Vec3::ZERO, 0.06);
    let arm = round_cone(p, Vec3::ZERO, v3(0.0, -UPPER_ARM, 0.0), 0.052, 0.042);
    let bi = ellipsoid(p, v3(0.0, -0.15, -0.018), v3(0.046, 0.09, 0.046));
    let tri = ellipsoid(p, v3(0.0, -0.14, 0.02), v3(0.048, 0.11, 0.048));
    let elbow = sphere(p, v3(0.0, -UPPER_ARM, 0.0), 0.042);
    let d = smin(smin(delt, ball, 0.03), arm, 0.03);
    smin(smin(smin(d, bi, 0.03), tri, 0.03), elbow, 0.03)
}

fn upper_arm() -> PieceMesh {
    let mut b = Builder::default();
    // Pauldron: a cap over the shoulder and two lames below, each overlapping the next.
    let cap_base = |p: Vec3| ellipsoid(p, v3(0.01, -0.01, 0.0), v3(0.078, 0.082, 0.078));
    // Spherical parameters with the pole pointing back (+Z): u runs over the top (−) to the
    // outside (+), v runs front to back.
    let rot = Quat::from_rotation_x(-FRAC_PI_2) * Quat::from_rotation_y(PI);
    let cap_param = Param::sph(v3(0.0, -0.01, 0.0), rot, 0.08);
    let cap_out = |uv: Vec2| box2(uv, v2(0.05, 0.0), v2(0.1, 0.085), 0.04).max(-0.035 - uv.x);
    let cap_h =
        |uv: Vec2, s: f32| 0.012 + 0.004 * (-((uv.y) / 0.02).powi(2)).exp() + lip(s, 0.012, 0.003);
    let cap = Plate {
        back: true,
        roll: 0.005,
        ..plate(
            cap_param,
            &cap_base,
            0.006,
            &cap_out,
            &cap_h,
            v2(-0.06, -0.11),
            v2(0.17, 0.11),
            0.012,
        )
    };
    let arm_param = Param::cyl(Vec3::ZERO, Quat::from_rotation_y(-FRAC_PI_2), 0.06);
    let lame_out = |lo: f32, hi: f32| {
        move |uv: Vec2| {
            box2(
                uv,
                v2(0.0, (lo + hi) * 0.5),
                v2(0.085, (hi - lo) * 0.5),
                0.015,
            )
        }
    };
    let l1_out = lame_out(-0.13, -0.075);
    let l2_out = lame_out(-0.17, -0.115);
    let l1_h = |_: Vec2, s: f32| 0.011 + lip(s, 0.008, 0.002);
    let l2_h = |_: Vec2, s: f32| 0.008 + lip(s, 0.008, 0.002);
    let l1 = Plate {
        back: true,
        ..plate(
            arm_param,
            &upper_arm_suit,
            0.006,
            &l1_out,
            &l1_h,
            v2(-0.1, -0.14),
            v2(0.1, -0.06),
            0.012,
        )
    };
    let l2 = Plate {
        back: true,
        ..plate(
            arm_param,
            &upper_arm_suit,
            0.005,
            &l2_out,
            &l2_h,
            v2(-0.1, -0.18),
            v2(0.1, -0.1),
            0.012,
        )
    };
    let plates = |p: Vec3| cap.proxy(p).min(l1.proxy(p)).min(l2.proxy(p));
    let suit = cull(
        sdf_mesh(
            &upper_arm_suit,
            v3(-0.09, -0.36, -0.09),
            v3(0.1, 0.08, 0.09),
            0.0125,
        ),
        &plates,
        0.008,
    );
    b.add(suit, Zone::Suit, Tint::Dyed(lin(SUIT), SUIT_DYE));
    b.add(cap.mesh(), Zone::Paint, Tint::Team(1.0));
    b.add(l1.mesh(), Zone::Paint, Tint::Team(0.92));
    b.add(l2.mesh(), Zone::Paint, Tint::Team(0.86));
    let field = |p: Vec3| upper_arm_suit(p).min(plates(p));
    b.finish(&field, 1.0)
}

fn forearm_suit(p: Vec3) -> f32 {
    let elbow = sphere(p, Vec3::ZERO, 0.043);
    let arm = round_cone(p, Vec3::ZERO, v3(0.0, -FOREARM + 0.01, 0.0), 0.046, 0.031);
    let mass = ellipsoid(p, v3(0.004, -0.065, -0.004), v3(0.05, 0.085, 0.046));
    smin(smin(elbow, arm, 0.03), mass, 0.04)
}

fn forearm() -> PieceMesh {
    let mut b = Builder::default();
    // Vambrace wrapping all but the inner side.
    let param = Param::cyl(Vec3::ZERO, Quat::from_rotation_y(-FRAC_PI_2), 0.045);
    let out = |uv: Vec2| box2(uv, v2(0.0, -0.165), v2(0.105, 0.07), 0.012);
    let h =
        |uv: Vec2, s: f32| 0.008 + 0.003 * (-(uv.x / 0.015).powi(2)).exp() + lip(s, 0.008, 0.002);
    let vam = plate(
        param,
        &forearm_suit,
        0.004,
        &out,
        &h,
        v2(-0.12, -0.25),
        v2(0.12, -0.08),
        0.011,
    );
    // Elbow cop.
    let eparam = Param::sph(Vec3::ZERO, Quat::from_rotation_y(PI), 0.05);
    let e_out = |uv: Vec2| circle2(uv, v2(0.0, 0.0), 0.038);
    let e_h = |uv: Vec2, s: f32| {
        0.008 + 0.004 * (1.0 - (uv.length() / 0.035).min(1.0)) + lip(s, 0.006, 0.002)
    };
    let ecop = Plate {
        back: true,
        ..plate(
            eparam,
            &forearm_suit,
            0.004,
            &e_out,
            &e_h,
            v2(-0.05, -0.05),
            v2(0.05, 0.05),
            0.009,
        )
    };
    let plates = |p: Vec3| vam.proxy(p).min(ecop.proxy(p));
    let suit = cull(
        sdf_mesh(
            &forearm_suit,
            v3(-0.08, -0.29, -0.08),
            v3(0.08, 0.06, 0.08),
            0.012,
        ),
        &plates,
        0.006,
    );
    b.add(suit, Zone::Suit, Tint::Dyed(lin(SUIT), SUIT_DYE));
    b.add(vam.mesh(), Zone::Paint, Tint::Team(0.9));
    b.add(ecop.mesh(), Zone::Metal, Tint::Rgb(lin(GUNMETAL)));
    let field = |p: Vec3| forearm_suit(p).min(plates(p));
    b.finish(&field, 1.0)
}

/// Points of a finger curled around the handle: from its knuckle around the grip axis.
fn finger_path(i: usize) -> (Vec<Vec3>, Vec<f32>) {
    let z = [-0.03f32, -0.01, 0.01, 0.029][i];
    let r = 0.0095 - i as f32 * 0.0006;
    let rf = 0.026;
    let g = GRIP;
    let knuckle = v3(0.006, -0.084, z * 0.95);
    let a_end = -3.05 + i as f32 * 0.12;
    let mut path = vec![knuckle];
    let mut radii = vec![r * 1.05];
    let n = 9;
    for k in 0..=n {
        let a = 0.25 + (a_end - 0.25) * k as f32 / n as f32;
        path.push(g + v3(a.cos() * rf, a.sin() * rf, z));
        radii.push(r * (1.0 - 0.12 * k as f32 / n as f32));
    }
    // Rounded tip.
    let last = path[path.len() - 1];
    let prev = path[path.len() - 2];
    let dir = (last - prev).normalize();
    for (k, s) in [(0.004, 0.8), (0.007, 0.45)] {
        path.push(last + dir * k);
        radii.push(r * s);
    }
    (path, radii)
}

/// A gloved right fist (palm and cuff; fingers are tubes) around a handle along Z through
/// [`rig::GRIP`].
fn palm(p: Vec3) -> f32 {
    let palm = round_box(
        p,
        v3(0.004, -0.052, 0.0),
        Quat::from_rotation_z(0.08),
        v3(0.017, 0.04, 0.041),
        0.014,
    );
    // Thenar pad under the thumb.
    let thenar = ellipsoid(p, v3(-0.012, -0.04, -0.026), v3(0.016, 0.03, 0.02));
    let cuff = round_cone(p, v3(0.0, -0.01, 0.0), v3(0.0, 0.035, 0.0), 0.04, 0.046);
    let cuff = smax(cuff, -0.022 - p.y, 0.004);
    smin(smin(palm, thenar, 0.012), cuff, 0.006)
}

fn hand() -> PieceMesh {
    let mut b = Builder::default();
    b.add(
        sdf_mesh(&palm, v3(-0.05, -0.1, -0.06), v3(0.05, 0.05, 0.06), 0.0068),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER_DARK)),
    );
    for i in 0..4 {
        let (path, radii) = finger_path(i);
        b.add(
            tube(&path, &radii, 8, Vec3::Z),
            Zone::Leather,
            Tint::Rgb(lin(LEATHER_DARK)),
        );
    }
    // Thumb wraps over the handle in front of the index finger.
    let g = GRIP;
    let thumb_path = [
        v3(-0.008, -0.03, -0.028),
        v3(-0.028, -0.05, -0.036),
        g + v3(-0.03, 0.012, -0.042),
        g + v3(-0.018, 0.026, -0.045),
        g + v3(-0.004, 0.03, -0.046),
        g + v3(0.0, 0.03, -0.0465),
    ];
    let thumb_r = [0.0125, 0.0115, 0.0105, 0.0098, 0.008, 0.005];
    b.add(
        tube(&thumb_path, &thumb_r, 8, Vec3::Z),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER_DARK)),
    );
    // Knuckle guard on the back of the hand.
    let guard = placed(
        slab(v3(0.007, 0.02, 0.045), 0.006, 0.003),
        Quat::from_rotation_z(0.25),
        v3(0.02, -0.072, 0.0),
    );
    b.add(guard, Zone::Polymer, Tint::Rgb(lin(POLYMER)));
    let field = |p: Vec3| {
        let mut d = palm(p);
        for i in 0..4 {
            let (path, radii) = finger_path(i);
            for k in 1..path.len() {
                d = d.min(capsule(p, path[k - 1], path[k], radii[k]));
            }
        }
        d
    };
    b.finish(&field, 0.9)
}

// ====================================================================== weapons

/// Diamond blade section with a fuller, half-width `w`, half-thickness `t`.
fn blade_section(w: f32, t: f32, fuller: f32) -> Vec<Vec2> {
    // x across the flat (thickness), y along the edge direction.
    let f = fuller.min(w * 0.4);
    vec![
        v2(0.0, w),
        v2(0.0, w),
        v2(t * 0.55, w * 0.5),
        v2(t, f),
        v2(t * 0.6, f * 0.4),
        v2(t * 0.6, -f * 0.4),
        v2(t, -f),
        v2(t * 0.55, -w * 0.5),
        v2(0.0, -w),
        v2(0.0, -w),
        v2(-t * 0.55, -w * 0.5),
        v2(-t, -f),
        v2(-t * 0.6, -f * 0.4),
        v2(-t * 0.6, f * 0.4),
        v2(-t, f),
        v2(-t * 0.55, w * 0.5),
    ]
}

/// Sword blade from `z0` (shoulder at the guard) to the tip at `z1` (both negative).
fn blade(z0: f32, z1: f32, w0: f32) -> PieceMesh {
    let n = 22;
    let mut path = Vec::new();
    let mut secs = Vec::new();
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let z = z0 + (z1 - z0) * t;
        // Profile taper: straight-ish, then an acute point over the last 12%.
        let taper = 1.0 - 0.38 * t;
        let point = if t > 0.86 {
            ((1.0 - t) / 0.14).max(0.0).powf(0.75)
        } else {
            1.0
        };
        let w = (w0 * taper * point).max(0.0006);
        let th = (0.0042 * (1.0 - 0.45 * t) * point.max(0.15)).max(0.0006);
        let fuller = if t < 0.6 {
            w * 0.32
        } else {
            w * 0.32 * (1.0 - (t - 0.6) / 0.4).max(0.0)
        };
        path.push(v3(0.0, 0.0, z));
        secs.push(blade_section(w, th, fuller));
    }
    let m = sweep(&path, Vec3::X, &secs);
    let mut b = Builder::default();
    b.add(m, Zone::Metal, Tint::Rgb(lin(STEEL)));
    b.finish(&|_| 1.0, 0.0)
}

fn hilt(long: bool) -> PieceMesh {
    let mut b = Builder::default();
    let (grip_lo, grip_hi) = if long { (-0.075, 0.16) } else { (-0.045, 0.06) };
    // Crossguard: an extruded bar with flared, down-swept quillons, flats along X.
    let gw = if long { 0.125 } else { 0.095 };
    let guard_pts = rounded(
        &[
            (v2(-gw, -0.012), 0.006),
            (v2(-gw * 0.35, 0.0), 0.02),
            (v2(-0.016, 0.012), 0.004),
            (v2(0.016, 0.012), 0.004),
            (v2(gw * 0.35, 0.0), 0.02),
            (v2(gw, -0.012), 0.006),
            (v2(gw + 0.004, 0.006), 0.006),
            (v2(gw * 0.35, 0.02), 0.02),
            (v2(0.0, 0.026), 0.01),
            (v2(-gw * 0.35, 0.02), 0.02),
            (v2(-gw - 0.004, 0.006), 0.006),
        ],
        3,
    );
    let guard = extrude(&guard_pts, 0.022, 0.004);
    // Profile x runs along the edges (Y), profile y toward the pommel (+Z), depth across X.
    let guard = placed(
        guard,
        basis(Vec3::Y, Vec3::Z, Vec3::X),
        v3(0.0, 0.0, grip_lo - 0.014),
    );
    b.add(guard, Zone::Metal, Tint::Rgb(lin(STEEL) * 0.85));
    // Grip: a slightly swelled leather wrap with cord ridges.
    let len = grip_hi - grip_lo;
    let ridges = (len / 0.011).round() as usize;
    let mut profile = vec![v2(0.0, 0.0)];
    for k in 0..=ridges * 2 {
        let t = k as f32 / (ridges * 2) as f32;
        let r = 0.0145 + 0.0028 * (t * PI).sin() + if k % 2 == 0 { 0.0 } else { 0.0011 };
        profile.push(v2(r, t * len));
    }
    profile.push(v2(0.0, len));
    let grip_mesh = lathe(&profile, 12, &[]);
    b.add(
        placed(
            grip_mesh,
            Quat::from_rotation_x(FRAC_PI_2),
            v3(0.0, 0.0, grip_lo),
        ),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER_DARK)),
    );
    let grip = move |p: Vec3| capsule(p, v3(0.0, 0.0, grip_lo), v3(0.0, 0.0, grip_hi), 0.016);
    // Pommel: faceted scent-stopper (long) or wheel (arming).
    let pommel = if long {
        lathe(
            &[
                v2(0.0, 0.0),
                v2(0.012, 0.0),
                v2(0.022, 0.012),
                v2(0.024, 0.03),
                v2(0.016, 0.046),
                v2(0.008, 0.052),
                v2(0.0, 0.054),
            ],
            8,
            &[1, 2, 3, 4],
        )
    } else {
        lathe(
            &[
                v2(0.0, 0.0),
                v2(0.01, 0.0),
                v2(0.03, 0.012),
                v2(0.032, 0.02),
                v2(0.03, 0.028),
                v2(0.01, 0.04),
                v2(0.0, 0.04),
            ],
            16,
            &[1, 5],
        )
    };
    let pommel = if long {
        placed(
            pommel,
            Quat::from_rotation_x(FRAC_PI_2),
            v3(0.0, 0.0, grip_hi - 0.004),
        )
    } else {
        // A wheel standing in the edge plane.
        placed(
            pommel,
            Quat::from_rotation_z(-FRAC_PI_2),
            v3(-0.02, 0.0, grip_hi + 0.026),
        )
    };
    b.add(pommel, Zone::Metal, Tint::Rgb(lin(STEEL) * 0.85));
    b.finish(&grip, 0.5)
}

/// Heater shield on its left-hand grip (see [`weapons`]).
fn shield() -> PieceMesh {
    let mut b = Builder::default();
    // Face: a cylinder section (curved left to right), outlined as a heater.
    let r = 0.5;
    let face_z = -0.085;
    let base = move |p: Vec3| Vec2::new(p.x, p.z - (face_z + r)).length() - r;
    let param = Param::cyl(v3(0.0, 0.0, face_z + r), Quat::IDENTITY, r);
    let heater = |uv: Vec2| {
        let w = 0.28;
        let top = uv.y - 0.3 - 0.035 * (1.0 - (uv.x / w).powi(2)).max(0.0);
        // Below y = 0 the sides are arcs meeting in the point at y = −0.42.
        let rr = (w * w + 0.42 * 0.42) / (2.0 * w);
        let side = if uv.y < 0.0 {
            Vec2::new(uv.x.abs() + rr - w, uv.y).length() - rr
        } else {
            uv.x.abs() - w
        };
        side.max(top)
    };
    let face_h = |uv: Vec2, s: f32| 0.01 + 0.003 * (1.0 - (uv.x / 0.3).powi(2)) + lip(s, 0.02, 0.0);
    let face = Plate {
        back: true,
        roll: 0.006,
        rim: 4,
        reach: 1.0,
        ..plate(
            param,
            &base,
            0.0,
            &heater,
            &face_h,
            v2(-0.31, -0.44),
            v2(0.31, 0.36),
            0.024,
        )
    };
    // Steel rim band around the edge, proud of the face.
    let rim_out = |uv: Vec2| {
        let o = heater(uv);
        o.max(-(o + 0.028))
    };
    let rim_h =
        |_: Vec2, s: f32| 0.0135 + 0.003 * smooth(-0.028, -0.014, s) * smooth(0.0, -0.014, s);
    let rim = Plate {
        roll: 0.007,
        rim: 4,
        reach: 1.0,
        ..plate(
            param,
            &base,
            0.0,
            &rim_out,
            &rim_h,
            v2(-0.31, -0.44),
            v2(0.31, 0.36),
            0.0125,
        )
    };
    // Emblem: a cream chevron under a bar.
    let emblem_out = |uv: Vec2| {
        let q = uv - v2(0.0, 0.02);
        let chev = (q.x.abs() * 0.85 - (q.y + 0.07)).abs() - 0.032;
        let chev = chev.max(q.x.abs() - 0.25).max(q.y - 0.16);
        let bar = box2(uv, v2(0.0, 0.235), v2(0.25, 0.022), 0.006);
        chev.min(bar).max(heater(uv) + 0.04)
    };
    let emblem_h = |_: Vec2, _: f32| 0.0118;
    let emblem = Plate {
        roll: 0.0012,
        rim: 2,
        reach: 1.0,
        ..plate(
            param,
            &base,
            0.0,
            &emblem_out,
            &emblem_h,
            v2(-0.3, -0.3),
            v2(0.3, 0.32),
            0.012,
        )
    };
    b.add(face.mesh(), Zone::Paint, Tint::Team(1.0));
    b.add(rim.mesh(), Zone::Metal, Tint::Rgb(lin(STEEL) * 0.8));
    b.add(emblem.mesh(), Zone::Paint, Tint::Rgb(lin(CREAM) * 0.9));
    // Boss.
    let boss = lathe(
        &[
            v2(0.0, 0.032),
            v2(0.03, 0.027),
            v2(0.055, 0.013),
            v2(0.066, 0.004),
            v2(0.07, 0.0),
        ],
        24,
        &[3],
    );
    b.add(
        placed(
            boss,
            Quat::from_rotation_x(-FRAC_PI_2),
            v3(0.0, -0.035, face_z - 0.011),
        ),
        Zone::Metal,
        Tint::Rgb(lin(STEEL)),
    );
    // Back: a leather-wrapped handle on two posts.
    let handle = |p: Vec3| {
        let bar = capsule(p, v3(0.0, -0.06, 0.0), v3(0.0, 0.06, 0.0), 0.016);
        let posts = capsule(
            p,
            v3(0.0, -0.065, 0.0),
            v3(0.0, -0.065, face_z + 0.01),
            0.011,
        )
        .min(capsule(
            p,
            v3(0.0, 0.065, 0.0),
            v3(0.0, 0.065, face_z + 0.01),
            0.011,
        ));
        bar.min(posts)
    };
    b.add(
        sdf_mesh(
            &handle,
            v3(-0.03, -0.09, -0.1),
            v3(0.03, 0.09, 0.03),
            0.0065,
        ),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER_DARK)),
    );
    let field = |p: Vec3| face.proxy(p).min(rim.proxy(p)).min(handle(p));
    b.finish(&field, 0.8)
}

/// Carbine (see [`weapons`] for its frame).
fn rifle() -> PieceMesh {
    let mut b = Builder::default();
    // Designed with the bore at y = 0.095; shifted at the end so the pistol grip's center is
    // the origin.
    let grip_at = v3(0.0, -0.005, 0.068);
    // Side profiles are drawn in (z, y) and extruded across x.
    let side = |pts: &[(Vec2, f32)], width: f32, bevel: f32| {
        let outline: Vec<Vec2> = rounded(pts, 2).into_iter().map(|p| v2(-p.x, p.y)).collect();
        placed(
            extrude(&outline, width, bevel),
            Quat::from_rotation_y(FRAC_PI_2),
            Vec3::ZERO,
        )
    };
    let p = |z: f32, y: f32, r: f32| (v2(z, y), r);
    let furniture = lin(0x6b5f4d);
    let black = lin(0x323438);
    // Upper receiver.
    let upper = side(
        &[
            p(-0.2, 0.075, 0.003),
            p(0.17, 0.075, 0.003),
            p(0.17, 0.11, 0.004),
            p(0.13, 0.115, 0.004),
            p(-0.2, 0.115, 0.003),
        ],
        0.044,
        0.006,
    );
    b.add(upper, Zone::Polymer, Tint::Rgb(black));
    // Lower receiver with the magazine well.
    let lower = side(
        &[
            p(-0.1, 0.076, 0.003),
            p(0.165, 0.076, 0.003),
            p(0.168, 0.05, 0.008),
            p(0.07, 0.042, 0.006),
            p(-0.03, 0.042, 0.006),
            p(-0.034, 0.018, 0.004),
            p(-0.1, 0.018, 0.004),
        ],
        0.038,
        0.005,
    );
    b.add(lower, Zone::Polymer, Tint::Rgb(black * 0.9));
    // Pistol grip, raked back.
    let grip = side(
        &[
            p(0.028, 0.045, 0.006),
            p(0.07, 0.045, 0.006),
            p(0.112, -0.05, 0.01),
            p(0.1, -0.058, 0.006),
            p(0.066, -0.058, 0.008),
            p(0.042, -0.01, 0.014),
        ],
        0.03,
        0.006,
    );
    b.add(grip, Zone::Polymer, Tint::Rgb(furniture));
    // Trigger guard and trigger.
    let guard = |q: Vec3| {
        capsule(q, v3(0.0, 0.04, -0.028), v3(0.0, 0.014, -0.022), 0.004)
            .min(capsule(
                q,
                v3(0.0, 0.014, -0.022),
                v3(0.0, 0.014, 0.034),
                0.004,
            ))
            .min(capsule(
                q,
                v3(0.0, 0.014, 0.034),
                v3(0.0, 0.04, 0.04),
                0.004,
            ))
            .min(capsule(
                q,
                v3(0.0, 0.042, 0.004),
                v3(0.0, 0.024, 0.012),
                0.003,
            ))
    };
    b.add(
        sdf_mesh(
            &guard,
            v3(-0.01, 0.005, -0.04),
            v3(0.01, 0.05, 0.05),
            0.0032,
        ),
        Zone::Polymer,
        Tint::Rgb(black),
    );
    // Curved magazine.
    let mag = side(
        &[
            p(-0.095, 0.03, 0.003),
            p(-0.036, 0.03, 0.003),
            p(-0.04, -0.05, 0.02),
            p(-0.062, -0.122, 0.006),
            p(-0.122, -0.11, 0.006),
            p(-0.1, -0.04, 0.02),
        ],
        0.027,
        0.004,
    );
    b.add(mag, Zone::Polymer, Tint::Rgb(lin(0x26272a)));
    // Handguard: a swept rounded octagon with slot inserts along the sides.
    let oct: Vec<Vec2> = rounded(
        &(0..8)
            .map(|k| {
                let a = (k as f32 + 0.5) * TAU / 8.0;
                (v2(a.cos() * 0.0255, a.sin() * 0.0265), 0.005)
            })
            .collect::<Vec<_>>(),
        2,
    );
    let ring = |s: f32| oct.iter().map(|q| *q * s).collect::<Vec<Vec2>>();
    let hg_path: Vec<Vec3> = [-0.2f32, -0.205, -0.21, -0.5, -0.505, -0.51]
        .iter()
        .map(|&z| v3(0.0, 0.095, z))
        .collect();
    let hg_secs = vec![
        ring(0.82),
        ring(0.96),
        ring(1.0),
        ring(1.0),
        ring(0.96),
        ring(0.82),
    ];
    b.add(
        sweep(&hg_path, Vec3::X, &hg_secs),
        Zone::Polymer,
        Tint::Rgb(furniture),
    );
    let slot = slab(v3(0.0015, 0.006, 0.014), 0.003, 0.0006);
    for k in 0..5 {
        for side in [-1.0f32, 1.0] {
            let at = v3(0.0262 * side, 0.095, -0.255 - k as f32 * 0.05);
            b.add(
                placed(slot.clone(), Quat::IDENTITY, at),
                Zone::Polymer,
                Tint::Rgb(lin(0x1c1c1e)),
            );
        }
    }
    // Top rail over receiver and handguard: a bar with cross teeth.
    let bar = placed(
        slab(v3(0.0095, 0.004, 0.33), 0.002, 0.0015),
        Quat::IDENTITY,
        v3(0.0, 0.119, -0.17),
    );
    b.add(bar, Zone::Metal, Tint::Rgb(lin(GUNMETAL)));
    let tooth = slab(v3(0.011, 0.0025, 0.0028), 0.001, 0.0008);
    for k in 0..33 {
        let z = 0.15 - k as f32 * 0.02;
        b.add(
            placed(tooth.clone(), Quat::IDENTITY, v3(0.0, 0.1245, z)),
            Zone::Metal,
            Tint::Rgb(lin(GUNMETAL)),
        );
    }
    // Barrel and muzzle brake.
    let barrel = lathe(
        &[
            v2(0.0, 0.0),
            v2(0.008, 0.0),
            v2(0.008, 0.1),
            v2(0.0115, 0.1),
            v2(0.0125, 0.105),
            v2(0.0125, 0.14),
            v2(0.009, 0.145),
            v2(0.0, 0.145),
        ],
        12,
        &[1, 2, 3, 4, 5, 6],
    );
    b.add(
        placed(
            barrel,
            Quat::from_rotation_x(-FRAC_PI_2),
            v3(0.0, 0.095, -0.5),
        ),
        Zone::Metal,
        Tint::Rgb(lin(GUNMETAL)),
    );
    // Stock on a buffer tube, with a rubber butt pad.
    let tube = lathe(
        &[v2(0.0, 0.0), v2(0.016, 0.0), v2(0.016, 0.17), v2(0.0, 0.17)],
        14,
        &[1, 2],
    );
    b.add(
        placed(tube, Quat::from_rotation_x(FRAC_PI_2), v3(0.0, 0.092, 0.16)),
        Zone::Metal,
        Tint::Rgb(lin(GUNMETAL)),
    );
    let stock = side(
        &[
            p(0.22, 0.112, 0.01),
            p(0.335, 0.118, 0.01),
            p(0.338, 0.0, 0.008),
            p(0.31, -0.008, 0.01),
            p(0.255, 0.05, 0.03),
            p(0.22, 0.07, 0.01),
        ],
        0.04,
        0.008,
    );
    b.add(stock, Zone::Polymer, Tint::Rgb(furniture));
    let pad = side(
        &[
            p(0.334, 0.122, 0.008),
            p(0.352, 0.122, 0.008),
            p(0.352, -0.012, 0.008),
            p(0.334, -0.012, 0.008),
        ],
        0.044,
        0.006,
    );
    b.add(pad, Zone::Polymer, Tint::Rgb(lin(RUBBER)));
    // Optic: a red-dot tube on a mount.
    let optic = lathe(
        &[
            v2(0.0, 0.0),
            v2(0.02, 0.0),
            v2(0.021, 0.006),
            v2(0.017, 0.012),
            v2(0.017, 0.072),
            v2(0.021, 0.078),
            v2(0.02, 0.086),
            v2(0.0, 0.086),
        ],
        18,
        &[1, 2, 3, 4, 5, 6],
    );
    b.add(
        placed(
            optic,
            Quat::from_rotation_x(-FRAC_PI_2),
            v3(0.0, 0.158, 0.07),
        ),
        Zone::Polymer,
        Tint::Rgb(lin(0x1d1e20)),
    );
    let mount = |q: Vec3| {
        round_box(
            q - v3(0.0, 0.136, 0.03),
            Vec3::ZERO,
            Quat::IDENTITY,
            v3(0.012, 0.012, 0.028),
            0.004,
        )
    };
    b.add(
        sdf_mesh(
            &mount,
            v3(-0.03, 0.115, -0.01),
            v3(0.03, 0.16, 0.07),
            0.0035,
        ),
        Zone::Metal,
        Tint::Rgb(lin(GUNMETAL)),
    );
    let lens = lathe(&[v2(0.0, 0.002), v2(0.016, 0.0)], 16, &[]);
    b.add(
        placed(
            lens,
            Quat::from_rotation_x(FRAC_PI_2),
            v3(0.0, 0.158, -0.012),
        ),
        Zone::Glass,
        Tint::Rgb(v3(0.08, 0.2, 0.17)),
    );
    let mut out = b.finish(&|_| 1.0, 0.0);
    out.mesh.transform(&Mat4::from_translation(-grip_at));
    out
}

/// Bow riser (handle) on the left-hand grip; limbs hinge at [`weapons::BOW_LIMB_PIVOT`].
fn bow_riser() -> PieceMesh {
    let mut b = Builder::default();
    let riser = |p: Vec3| {
        let t = (p.y / 0.2).clamp(-1.0, 1.0);
        // Deeper at the grip (bulging toward the archer), tapering into the limb pockets.
        let depth = 0.014 + 0.012 * (1.0 - t * t);
        let width = 0.014 + 0.005 * (1.0 - t * t);
        let center_z = 0.01 + 0.008 * (1.0 - t * t);
        let q = Vec2::new(p.x / width, (p.z - center_z) / depth);
        let d = (q.length() - 1.0) * width.min(depth);
        // Arrow shelf cut into the left side above the grip.
        let window = round_box(
            p - v3(-0.016, 0.065, 0.0),
            Vec3::ZERO,
            Quat::IDENTITY,
            v3(0.008, 0.045, 0.04),
            0.006,
        );
        ssub(smax(d, p.y.abs() - 0.21, 0.012), window, 0.004)
    };
    b.add(
        sdf_mesh(
            &riser,
            v3(-0.04, -0.24, -0.04),
            v3(0.04, 0.24, 0.06),
            0.0058,
        ),
        Zone::Wood,
        Tint::Rgb(lin(WOOD)),
    );
    let wrap = |p: Vec3| {
        let r = round_cone(
            p,
            v3(0.0, -0.055, 0.017),
            v3(0.0, 0.045, 0.017),
            0.022,
            0.021,
        );
        let rings = ((p.y * 300.0).sin() * 0.5 + 0.5) * 0.0008;
        smax(r + rings, -0.004 - (p.z - 0.017).abs() * 0.0, 0.0)
    };
    b.add(
        sdf_mesh(
            &wrap,
            v3(-0.035, -0.08, -0.015),
            v3(0.035, 0.07, 0.05),
            0.005,
        ),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER)),
    );
    b.finish(&riser, 0.6)
}

/// The limb's centerline in its pivot's frame for `t` in 0..1: back toward the archer, then
/// recurving forward past the string's contact point at t = 0.86.
fn limb_path(t: f32) -> Vec3 {
    let y = 0.56 * t;
    let z = if t <= 0.86 {
        0.165 * (1.0 - (1.0 - t / 0.86).powi(2))
    } else {
        0.165 - 0.045 * ((t - 0.86) / 0.14).powi(2)
    };
    v3(0.0, y, z)
}

/// The upper limb of the bow in its pivot's frame (the lower one mirrors y).
fn bow_limb() -> PieceMesh {
    let n = 22;
    let mut path = Vec::new();
    let mut secs = Vec::new();
    for k in 0..=n {
        let t = k as f32 / n as f32;
        path.push(limb_path(t));
        let w = 0.021 * (1.0 - 0.6 * t) + 0.004;
        let th = 0.0065 * (1.0 - 0.5 * t) + 0.002;
        secs.push(vec![
            v2(-w, -th),
            v2(-w, -th),
            v2(w, -th),
            v2(w, -th),
            v2(w, th),
            v2(w, th),
            v2(-w, th),
            v2(-w, th),
        ]);
    }
    let m = sweep(&path, Vec3::X, &secs);
    let mut b = Builder::default();
    b.add(m, Zone::Wood, Tint::Rgb(lin(0x3b2a1d)));
    // Horn tip overlays.
    let tip = limb_path(1.0);
    let cap = |p: Vec3| round_cone(p, limb_path(0.9), tip, 0.008, 0.006);
    b.add(
        sdf_mesh(
            &cap,
            tip - Vec3::splat(0.07),
            tip + Vec3::splat(0.03),
            0.003,
        ),
        Zone::Polymer,
        Tint::Rgb(lin(0xd8cfbf)),
    );
    b.finish(&|_| 1.0, 0.0)
}

fn arrow() -> PieceMesh {
    let mut b = Builder::default();
    let len = weapons::ARROW;
    // Shaft along −Z from the nock at the origin.
    let shaft = lathe(
        &[
            v2(0.0, -0.004),
            v2(0.0045, 0.002),
            v2(0.0042, len - 0.03),
            v2(0.0, len - 0.03),
        ],
        8,
        &[1, 2],
    );
    b.add(
        placed(shaft, Quat::from_rotation_x(-FRAC_PI_2), Vec3::ZERO),
        Zone::Wood,
        Tint::Rgb(lin(0xb08a5a)),
    );
    let head = lathe(
        &[
            v2(0.0, 0.0),
            v2(0.006, 0.0),
            v2(0.011, 0.014),
            v2(0.0, 0.05),
        ],
        4,
        &[1, 2],
    );
    b.add(
        placed(
            head,
            Quat::from_rotation_x(-FRAC_PI_2),
            v3(0.0, 0.0, -(len - 0.035)),
        ),
        Zone::Metal,
        Tint::Rgb(lin(GUNMETAL)),
    );
    for k in 0..3 {
        let a = k as f32 * TAU / 3.0;
        let vane = extrude(
            &[
                v2(0.0, 0.0),
                v2(0.1, 0.0),
                v2(0.085, 0.017),
                v2(0.025, 0.017),
            ],
            0.0015,
            0.0,
        );
        let vane = placed(
            vane,
            Quat::from_rotation_z(a) * Quat::from_rotation_y(FRAC_PI_2),
            v3(0.0, 0.0, -0.02),
        );
        b.add(vane, Zone::Cloth, Tint::Rgb(lin(CREAM)));
    }
    b.finish(&|_| 1.0, 0.0)
}

/// A back quiver with arrows, hung on the chest piece (chest frame), opening over the right
/// shoulder.
fn quiver() -> PieceMesh {
    let mut b = Builder::default();
    let a = v3(-0.09, 0.02, 0.19);
    let c = v3(0.1, 0.47, 0.2);
    let dir = (c - a).normalize();
    let body = move |p: Vec3| round_cone(p, a, c, 0.036, 0.044);
    let open = move |p: Vec3| smax(body(p), (p - c).dot(dir), 0.004);
    b.add(
        sdf_mesh(&open, v3(-0.16, -0.04, 0.12), v3(0.17, 0.53, 0.27), 0.0095),
        Zone::Leather,
        Tint::Rgb(lin(LEATHER)),
    );
    let fletch = move |p: Vec3| {
        let mut d = f32::MAX;
        for k in 0..5 {
            let off = v3(
                (k as f32 - 2.0) * 0.013,
                ((k % 2) as f32 - 0.5) * 0.012,
                (k as f32 * 1.7).sin() * 0.012,
            );
            let s = c + off + dir * (0.04 + 0.01 * (k % 3) as f32);
            d = d.min(ellipsoid(
                p - s,
                Vec3::ZERO,
                v3(0.012, 0.012, 0.012) + dir.abs() * 0.04,
            ));
            d = d.min(capsule(p, s - dir * 0.08, s + dir * 0.05, 0.0035));
        }
        d
    };
    b.add(
        sdf_mesh(&fletch, c - Vec3::splat(0.1), c + Vec3::splat(0.12), 0.0055),
        Zone::Cloth,
        Tint::Rgb(lin(CREAM) * 0.9),
    );
    b.finish(&|p| body(p).min(fletch(p)), 0.6)
}

/// Muzzle flash: a star of petals around a hot core, pointing −Z.
fn muzzle_flash() -> PieceMesh {
    let f = |p: Vec3| {
        let core = ellipsoid(p, v3(0.0, 0.0, -0.06), v3(0.035, 0.035, 0.08));
        let mut d = core;
        for k in 0..4 {
            let a = k as f32 * TAU / 4.0 + 0.4;
            let dir = v3(a.cos(), a.sin(), -0.8).normalize();
            d = smin(d, round_cone(p, Vec3::ZERO, dir * 0.11, 0.018, 0.002), 0.02);
        }
        d.min(round_cone(p, Vec3::ZERO, v3(0.0, 0.0, -0.22), 0.03, 0.004))
    };
    let mut b = Builder::default();
    b.add(
        sdf_mesh(&f, v3(-0.12, -0.12, -0.25), v3(0.12, 0.12, 0.03), 0.009),
        Zone::Glass,
        Tint::Rgb(Vec3::ONE),
    );
    b.finish(&|_| 1.0, 0.0)
}

// ====================================================================== the kit

/// Every mesh of the fighter kit. Right/left pairs mirror each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Piece {
    Helmet,
    /// The face, its skin tinted by the "team" color (the skin tone).
    Face,
    Visor,
    Crest,
    Chest,
    Pelvis,
    TabardFront,
    TabardBack,
    ThighR,
    ThighL,
    ShinR,
    ShinL,
    FootR,
    FootL,
    UpperArmR,
    UpperArmL,
    ForearmR,
    ForearmL,
    HandR,
    HandL,
    LongBlade,
    LongHilt,
    ArmingBlade,
    ArmingHilt,
    Shield,
    Rifle,
    BowRiser,
    BowLimb,
    Arrow,
    Quiver,
    Muzzle,
}

impl Piece {
    pub const ALL: [Piece; 31] = [
        Piece::Helmet,
        Piece::Face,
        Piece::Visor,
        Piece::Crest,
        Piece::Chest,
        Piece::Pelvis,
        Piece::TabardFront,
        Piece::TabardBack,
        Piece::ThighR,
        Piece::ThighL,
        Piece::ShinR,
        Piece::ShinL,
        Piece::FootR,
        Piece::FootL,
        Piece::UpperArmR,
        Piece::UpperArmL,
        Piece::ForearmR,
        Piece::ForearmL,
        Piece::HandR,
        Piece::HandL,
        Piece::LongBlade,
        Piece::LongHilt,
        Piece::ArmingBlade,
        Piece::ArmingHilt,
        Piece::Shield,
        Piece::Rifle,
        Piece::BowRiser,
        Piece::BowLimb,
        Piece::Arrow,
        Piece::Quiver,
        Piece::Muzzle,
    ];

    /// Asset key fragment.
    pub fn name(self) -> &'static str {
        match self {
            Piece::Helmet => "helmet",
            Piece::Face => "face",
            Piece::Visor => "visor",
            Piece::Crest => "crest",
            Piece::Chest => "chest",
            Piece::Pelvis => "pelvis",
            Piece::TabardFront => "tabard_front",
            Piece::TabardBack => "tabard_back",
            Piece::ThighR => "thigh_r",
            Piece::ThighL => "thigh_l",
            Piece::ShinR => "shin_r",
            Piece::ShinL => "shin_l",
            Piece::FootR => "foot_r",
            Piece::FootL => "foot_l",
            Piece::UpperArmR => "upper_arm_r",
            Piece::UpperArmL => "upper_arm_l",
            Piece::ForearmR => "forearm_r",
            Piece::ForearmL => "forearm_l",
            Piece::HandR => "hand_r",
            Piece::HandL => "hand_l",
            Piece::LongBlade => "long_blade",
            Piece::LongHilt => "long_hilt",
            Piece::ArmingBlade => "arming_blade",
            Piece::ArmingHilt => "arming_hilt",
            Piece::Shield => "shield",
            Piece::Rifle => "rifle",
            Piece::BowRiser => "bow_riser",
            Piece::BowLimb => "bow_limb",
            Piece::Arrow => "arrow",
            Piece::Quiver => "quiver",
            Piece::Muzzle => "muzzle",
        }
    }

    /// True if part of the mesh takes the team color.
    pub fn team_colored(self) -> bool {
        !kit().pieces[self as usize].team.is_empty()
    }
}

/// The built kit: the atlas and every piece's mesh.
pub struct Kit {
    atlas: TextureSet,
    pieces: Vec<PieceMesh>,
    /// Seconds it took to build (wall clock, all threads).
    pub build_secs: f32,
}

impl Kit {
    /// The atlas's albedo, normal and ORM images.
    pub fn atlas(&self) -> TextureSet {
        TextureSet {
            albedo: self.atlas.albedo.clone(),
            normal: self.atlas.normal.clone(),
            orm: self.atlas.orm.clone(),
        }
    }

    /// A piece's mesh painted with `team` (linear RGB).
    pub fn mesh(&self, piece: Piece, team: Vec3) -> Mesh {
        let p = &self.pieces[piece as usize];
        let mut m = p.mesh.clone();
        for &(i, k) in &p.team {
            let c = &mut m.vertices[i as usize].color;
            c[0] += team.x * k;
            c[1] += team.y * k;
            c[2] += team.z * k;
        }
        m
    }

    /// Triangles in a piece.
    pub fn triangles(&self, piece: Piece) -> usize {
        self.pieces[piece as usize].triangles()
    }
}

enum Job {
    Tile(Zone),
    Pieces(fn() -> Vec<(Piece, PieceMesh)>),
}

enum Output {
    Tile(BakedTile),
    Pieces(Vec<(Piece, PieceMesh)>),
}

fn pair(right: Piece, left: Piece, make: fn() -> PieceMesh) -> Vec<(Piece, PieceMesh)> {
    let r = make();
    let l = r.mirrored();
    vec![(right, r), (left, l)]
}

fn build_kit() -> Kit {
    let start = std::time::Instant::now();
    let mut jobs: Vec<Job> = vec![
        Job::Pieces(|| vec![(Piece::Helmet, helmet())]),
        Job::Pieces(|| vec![(Piece::Face, face())]),
        Job::Pieces(|| vec![(Piece::Crest, crest())]),
        Job::Pieces(|| vec![(Piece::Chest, chest())]),
        Job::Pieces(|| vec![(Piece::Pelvis, pelvis())]),
        Job::Pieces(|| pair(Piece::HandR, Piece::HandL, hand)),
        Job::Pieces(|| pair(Piece::FootR, Piece::FootL, foot)),
        Job::Pieces(|| pair(Piece::ThighR, Piece::ThighL, thigh)),
        Job::Pieces(|| pair(Piece::ShinR, Piece::ShinL, shin)),
        Job::Pieces(|| pair(Piece::UpperArmR, Piece::UpperArmL, upper_arm)),
        Job::Pieces(|| pair(Piece::ForearmR, Piece::ForearmL, forearm)),
        Job::Pieces(|| vec![(Piece::Shield, shield())]),
        Job::Pieces(|| vec![(Piece::Rifle, rifle())]),
        Job::Pieces(|| {
            vec![
                (Piece::LongHilt, hilt(true)),
                (Piece::ArmingHilt, hilt(false)),
            ]
        }),
        Job::Pieces(|| {
            vec![
                (Piece::TabardFront, tabard(true)),
                (Piece::TabardBack, tabard(false)),
                (Piece::Visor, visor()),
            ]
        }),
        Job::Pieces(|| {
            vec![
                (Piece::LongBlade, blade(-0.09, -1.06, 0.024)),
                (Piece::ArmingBlade, blade(-0.06, -0.84, 0.024)),
                (Piece::BowRiser, bow_riser()),
                (Piece::BowLimb, bow_limb()),
                (Piece::Arrow, arrow()),
                (Piece::Quiver, quiver()),
                (Piece::Muzzle, muzzle_flash()),
            ]
        }),
    ];
    jobs.extend(Zone::ALL.into_iter().map(Job::Tile));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Output>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    let out = match job {
                        Job::Tile(z) => Output::Tile(bake_tile(*z)),
                        Job::Pieces(f) => Output::Pieces(f()),
                    };
                    results.lock().unwrap().push(out);
                }
            });
        }
    });
    let mut tiles = Vec::new();
    let mut pieces = vec![PieceMesh::default(); Piece::ALL.len()];
    for out in results.into_inner().unwrap() {
        match out {
            Output::Tile(t) => tiles.push(t),
            Output::Pieces(list) => {
                for (p, m) in list {
                    pieces[p as usize] = m;
                }
            }
        }
    }
    let atlas = assemble_atlas(&tiles);
    Kit {
        atlas,
        pieces,
        build_secs: start.elapsed().as_secs_f32(),
    }
}

static KIT: OnceLock<Kit> = OnceLock::new();

/// The fighter kit, built on first use (once per process, shared by every app).
pub fn kit() -> &'static Kit {
    KIT.get_or_init(build_kit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_report() {
        let kit = kit();
        for p in Piece::ALL {
            eprintln!("{:>14} {:>6} tris", p.name(), kit.triangles(p));
        }
        eprintln!("built in {:.2} s", kit.build_secs);
    }

    #[test]
    fn plates_are_closed_and_face_out() {
        let base = |p: Vec3| p.length() - 0.2;
        let outline = |uv: Vec2| box2(uv, Vec2::ZERO, v2(0.1, 0.08), 0.02);
        let height = |_: Vec2, _: f32| 0.01;
        let pl = Plate {
            back: true,
            ..plate(
                Param::sph(Vec3::ZERO, Quat::IDENTITY, 0.2),
                &base,
                0.0,
                &outline,
                &height,
                v2(-0.15, -0.12),
                v2(0.15, 0.12),
                0.01,
            )
        };
        let m = pl.mesh();
        assert!(m.triangle_count() > 200);
        // Every edge is shared by exactly two triangles (a closed shell).
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        let pos = |i: u32| {
            let p = Vec3::from(m.vertices[i as usize].position);
            ((p.x * 1e5) as i64, (p.y * 1e5) as i64, (p.z * 1e5) as i64)
        };
        let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let canon: Vec<u32> = (0..m.vertices.len() as u32)
            .map(|i| {
                let n = ids.len() as u32;
                *ids.entry(pos(i)).or_insert(n)
            })
            .collect();
        for t in m.indices.chunks(3) {
            for k in 0..3 {
                let (a, b) = (canon[t[k] as usize], canon[t[(k + 1) % 3] as usize]);
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open = edges.values().filter(|&&c| c != 2).count();
        assert!(open == 0, "{open} open edges of {}", edges.len());
        // Front faces point away from the sphere.
        let front = m
            .vertices
            .iter()
            .filter(|v| Vec3::from(v.position).length() > 0.205)
            .filter(|v| Vec3::from(v.normal).dot(Vec3::from(v.position).normalize()) > 0.5)
            .count();
        assert!(front > 100);
    }

    #[test]
    fn extrusions_and_lathes_are_sane() {
        let sq = [v2(0.0, 0.0), v2(0.1, 0.0), v2(0.1, 0.1), v2(0.0, 0.1)];
        let m = extrude(&sq, 0.05, 0.005);
        assert_eq!(m.triangle_count(), 4 * 6 + 2 * 2);
        for t in m.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(m.vertices[t[i] as usize].position));
            let center = v3(0.05, 0.05, 0.0);
            assert!((b - a).cross(c - a).dot((a + b + c) / 3.0 - center) > -1e-9);
        }
        let l = lathe(
            &[v2(0.0, 0.0), v2(0.1, 0.0), v2(0.1, 0.2), v2(0.0, 0.2)],
            12,
            &[1, 2],
        );
        assert!(l.triangle_count() >= 12 * 3 * 2 - 24);
    }
}
