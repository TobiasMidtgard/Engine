//! Realistic horses: a rigged horse model driven by the procedural horse's rig.
//!
//! [`from_rigged_horse`] fits the horse skeleton (the bone names of [`horse::skeleton`], no
//! rest rotations) to a model's joints and to the shape of its legs and neck, then hands the
//! mesh's skin weights over to it: each of the file's bones gives its weight to the fitted
//! bones along it, so a one-piece lower leg becomes cannon and pastern with a fetlock that
//! bends, and a one-piece neck three vertebrae. The horse's gait, idle, muscles and tail
//! springs then drive the model unchanged. It also
//! - smooths the mesh with one level of curved (PN-triangle) subdivision, so the silhouette
//!   stays round up close;
//! - grows a mane, forelock and tail as alpha-cut hair cards, draped over the neck and
//!   swinging with the tail springs;
//! - adds the horse's muscles as bulge morph targets placed from its own joints.
//!
//! [`HorseModel`] loads a file in the background and swaps it onto a procedural horse.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, StandardMaterial};
use serde::{Deserialize, Serialize};

use crate::animator::{AnimationLibrary, AnimatorSettings};
use crate::build::bone;
use crate::build::horse;
use crate::build::sdf::{MuscleShape, add_muscle_morphs};
use crate::import::{ImportOptions, ImportedMaterial, ImportedModel, import_gltf};
use crate::muscle::{MorphWeights, MuscleRig};
use crate::procedural::gait::LeggedGait;
use crate::procedural::spring::{SpringChainDef, SpringChains};
use crate::skeleton::Skeleton;
use crate::skin::{SkinVertex, SkinnedMesh, SkinnedMeshRenderer, Submesh};

/// Which bones of a rigged four-legged model do what. Left and right are found from where
/// the legs stand, not from their names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadrupedBones {
    /// Back, croup and other trunk bones: their weight goes to the body.
    pub trunk: &'static [&'static str],
    /// One bone from the withers to the poll.
    pub neck: &'static str,
    /// Skull, ears, jaw; the first starts at the poll.
    pub head: &'static [&'static str],
    /// The bone at the root of the tail.
    pub tail: &'static str,
    /// Each leg from the body down: shoulder, elbow, and knee to the ground.
    pub front: [[&'static str; 3]; 2],
    /// Each leg from the body down: hip, stifle, and hock to the ground.
    pub hind: [[&'static str; 3]; 2],
}

/// The bones of the paddock's horse (`anim/animals/horse/Horse.gltf`).
pub const RIGGED_HORSE: QuadrupedBones = QuadrupedBones {
    trunk: &["Bone", "Bone.003"],
    neck: "Bone.001",
    head: &["Bone.002", "Bone.001_L", "Bone.001_R"],
    tail: "Bone.004",
    front: [
        ["Bone_L", "Bone_L.001", "Bone_L.002"],
        ["Bone_R", "Bone_R.001", "Bone_R.002"],
    ],
    hind: [
        ["Bone_L.003", "Bone_L.004", "Bone_L.005"],
        ["Bone_R.003", "Bone_R.004", "Bone_R.005"],
    ],
};

/// Fetlock height as a share of the knee's (front) or hock's (hind) height.
const FETLOCK: f32 = 0.32;
/// Knee and hock height of the procedural horse, for sizing muscles, steps and hair.
const KNEE_HEIGHT: f32 = 0.54;
/// Coat detail: skin-like light wrap, veins (on the legs), sheen.
const COAT_MASK: Vec4 = Vec4::new(0.0, 0.55, 0.5, 1.0);
const TRUNK_VEINS: f32 = 0.12;
/// Hair: a little light wrap and full sheen.
const HAIR_MASK: Vec4 = Vec4::new(0.0, 0.2, 0.0, 1.0);
/// Hair cards are cut out below this alpha.
const HAIR_CUTOFF: f32 = 0.4;
/// Card designs side by side in [`hair_card_image`].
const HAIR_COLUMNS: u32 = 4;

/// A rigged horse ready to spawn (see the module docs).
#[derive(Debug, Clone)]
pub struct HorseCharacter {
    /// The horse skeleton fitted to the model (registered in the `AnimationLibrary`).
    pub logic: Skeleton,
    /// The model's mesh skinned to `logic`, with hair cards and muscle targets.
    pub mesh: SkinnedMesh,
    pub materials: Vec<ImportedMaterial>,
    pub images: Vec<Image>,
    /// The horse's gait with soles, hooves and steps sized to the model.
    pub gait: LeggedGait,
    pub tail: SpringChainDef,
}

/// A leg found in the file.
#[derive(Debug, Clone)]
struct Leg {
    front: bool,
    /// 0 left (−X), 1 right.
    side: usize,
    /// Where the file's leg starts (withers or croup).
    top: Vec3,
    /// Heads of upper, lower, cannon and hoof.
    joints: [Vec3; 4],
    file: [&'static str; 3],
}

impl Leg {
    fn bone(&self, part: &str) -> String {
        let prefix = if self.front { "front" } else { "hind" };
        format!("{prefix}_{part}_{}", ["l", "r"][self.side])
    }

    fn sign(&self) -> f32 {
        if self.side == 0 { -1.0 } else { 1.0 }
    }
}

/// Where the fitted skeleton and the hair go.
#[derive(Debug, Clone)]
struct Fitted {
    /// Size relative to the procedural horse.
    s: f32,
    body: Vec3,
    /// Withers, neck_2, neck_3, poll.
    neck: [Vec3; 4],
    /// tail_1 to tail_4 and the tip.
    tail: [Vec3; 5],
    /// Middle of the head.
    muzzle: Vec3,
    /// The top of the neck from the withers to the poll.
    crest: Vec<Vec3>,
    /// Top of the head just in front of the poll.
    forelock: Vec3,
    legs: Vec<Leg>,
}

/// Fit the horse rig to a rigged horse model and prepare its mesh (see the module docs).
pub fn from_rigged_horse(
    name: &str,
    model: ImportedModel,
    bones: &QuadrupedBones,
) -> Result<HorseCharacter, String> {
    let ImportedModel {
        skeleton: file,
        mut mesh,
        mut materials,
        mut images,
        ..
    } = model;
    let at = |b: &str| {
        file.index_of(b)
            .map(|i| file.rest_model[i].t)
            .ok_or_else(|| format!("{name}: no bone {b} (not the expected horse rig?)"))
    };
    if mesh.vertices.is_empty() {
        return Err(format!("{name}: no skinned mesh"));
    }
    let ground = mesh
        .vertices
        .iter()
        .map(|v| v.position.y)
        .fold(f32::MAX, f32::min);
    subdivide(&mut mesh, 0.02);

    // Which file bone carries each vertex most, and which surfaces are coat.
    let names: Vec<String> = mesh.joints.clone();
    let dominant: Vec<&str> = mesh
        .vertices
        .iter()
        .map(|v| {
            let k = (0..4)
                .max_by(|a, b| v.weights[*a].total_cmp(&v.weights[*b]))
                .unwrap_or(0);
            names.get(v.joints[k] as usize).map_or("", String::as_str)
        })
        .collect();
    let surfaces = vertex_surfaces(&mesh);
    let textured = materials.iter().any(|m| m.base_color_texture.is_some());
    let coat: Vec<bool> = surfaces
        .iter()
        .map(|&s| {
            !textured
                || materials
                    .get(s)
                    .is_none_or(|m| m.base_color_texture.is_some())
        })
        .collect();
    let region = |files: &[&str]| -> Vec<Vec3> {
        mesh.vertices
            .iter()
            .zip(&dominant)
            .zip(&coat)
            .filter(|((_, d), c)| **c && files.contains(d))
            .map(|((v, _), _)| v.position)
            .collect()
    };

    let mut knees = 0.0;
    for chain in bones.front.iter().chain(&bones.hind) {
        knees += at(chain[2])?.y - ground;
    }
    let s = knees / 4.0 / KNEE_HEIGHT;

    // Legs. Front: the file's shoulder bone runs from the withers to the elbow; the fitted
    // leg turns at the point of the shoulder, and the shoulder blade above it stays with the
    // body. Hind: the file's first bone is the pelvis (croup to hip joint) and its second runs
    // from the hip to the hock; the fitted leg turns at the hip and bends at a stifle found at
    // the front of the thigh. Both get a fetlock centered in the leg a third of the way up.
    let mut legs = Vec::new();
    for (front, chains) in [(true, &bones.front), (false, &bones.hind)] {
        for chain in chains {
            let (a, b, c) = (at(chain[0])?, at(chain[1])?, at(chain[2])?);
            let fy = ground + FETLOCK * (c.y - ground);
            let slab: Vec<Vec3> = region(&[chain[2]])
                .into_iter()
                .filter(|p| (p.y - fy).abs() < 0.04)
                .collect();
            let center = centroid(&slab).unwrap_or(c);
            let fetlock = Vec3::new(center.x, fy, center.z);
            let joints = if front {
                // The point of the shoulder sits in front of the elbow, halfway up to the
                // withers.
                let shoulder = Vec3::new(b.x * 0.8, b.y + 0.45 * (a.y - b.y), b.z - 0.16 * s);
                [shoulder, b, c, fetlock]
            } else {
                let (lo, hi) = (c.y + 0.35 * (b.y - c.y), c.y + 0.65 * (b.y - c.y));
                let line = b.lerp(c, 0.45);
                let stifle = region(&[chain[1]])
                    .into_iter()
                    .filter(|p| p.y > lo && p.y < hi)
                    .min_by(|p, q| p.z.total_cmp(&q.z))
                    .map_or(line + Vec3::NEG_Z * 0.15 * s, |front_of_thigh| {
                        Vec3::new(
                            line.x,
                            front_of_thigh.y,
                            (front_of_thigh.z + 0.09 * s).min(line.z - 0.05 * s),
                        )
                    });
                [b, stifle, c, fetlock]
            };
            legs.push(Leg {
                front,
                side: usize::from(c.x >= 0.0),
                top: a,
                joints,
                file: *chain,
            });
        }
    }
    for front in [true, false] {
        for side in 0..2 {
            if legs
                .iter()
                .filter(|l| l.front == front && l.side == side)
                .count()
                != 1
            {
                return Err(format!(
                    "{name}: can't tell the left and right {} legs apart",
                    if front { "front" } else { "hind" }
                ));
            }
        }
    }
    // Neck: withers to poll, with two vertebrae centered in the neck between them.
    let withers = at(bones.neck)?.with_x(0.0);
    let poll = at(bones.head[0])?.with_x(0.0);
    let neck_points = region(&[bones.neck]);
    let axis = (poll - withers).normalize_or(Vec3::NEG_Z);
    let neck_len = withers.distance(poll);
    let station = |t: f32| {
        let p = withers.lerp(poll, t);
        let slab: Vec<Vec3> = neck_points
            .iter()
            .copied()
            .filter(|v| (*v - p).dot(axis).abs() < neck_len * 0.08)
            .collect();
        centroid(&slab).map_or(p, |c| c.with_x(0.0))
    };
    let neck = [withers, station(1.0 / 3.0), station(2.0 / 3.0), poll];

    // The crest: the highest neck (or withers) skin above each point of the neck.
    let mut top_points = region(&[bones.neck]);
    top_points.extend(region(bones.trunk));
    let dorsal = (Vec3::Y - axis * Vec3::Y.dot(axis)).normalize_or(Vec3::Y);
    let mut crest: Vec<Vec3> = (0..25)
        .map(|i| {
            let p = withers.lerp(poll, i as f32 / 24.0);
            top_points
                .iter()
                .filter(|v| (**v - p).dot(axis).abs() < neck_len * 0.04 && v.x.abs() < 0.06 * s)
                .max_by(|a, b| (**a - p).dot(dorsal).total_cmp(&(**b - p).dot(dorsal)))
                .map_or(p + dorsal * 0.1 * s, |v| v.with_x(0.0))
        })
        .collect();
    for _ in 0..2 {
        let prev = crest.clone();
        for i in 1..crest.len() - 1 {
            crest[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) / 4.0;
        }
    }
    let head_points = region(&bones.head[..1]);
    let muzzle = centroid(&head_points).unwrap_or(poll + Vec3::new(0.0, -0.3, -0.3) * s);
    let forelock = head_points
        .iter()
        .filter(|v| v.x.abs() < 0.04 * s && (v.z - poll.z).abs() < 0.07 * s)
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .map_or(poll + Vec3::Y * 0.05 * s, |v| v.with_x(0.0));

    // The body turns about the middle of the barrel, between the shoulders and the hips.
    let hips = legs
        .iter()
        .filter(|l| !l.front)
        .map(|l| l.joints[0])
        .sum::<Vec3>()
        / 2.0;
    let middle = withers.lerp(hips, 0.5);
    let body = Vec3::new(
        0.0,
        centroid(&region(bones.trunk)).map_or(middle.y, |c| c.y),
        middle.z,
    );
    // The tail hangs from its root to just below the hocks; the hair covers the bones.
    let root = at(bones.tail)?.with_x(0.0);
    let mut tail = [root; 5];
    for (k, step) in [
        Vec3::new(0.0, -0.09, 0.07),
        Vec3::new(0.0, -0.24, 0.05),
        Vec3::new(0.0, -0.26, 0.02),
        Vec3::new(0.0, -0.24, 0.0),
    ]
    .iter()
    .enumerate()
    {
        tail[k + 1] = tail[k] + *step * s;
    }
    let fitted = Fitted {
        s,
        body,
        neck,
        tail,
        muzzle,
        crest,
        forelock,
        legs,
    };
    let logic = fit_skeleton(&format!("{}.{name}", horse::SKELETON), &fitted)?;

    // Hand the file's weights to the fitted bones.
    enum Handoff {
        Bone(usize),
        Chain(Vec<Vec3>, Vec<usize>, f32),
    }
    let b = |n: &str| logic.bone(n);
    let mut handoff: Vec<Handoff> = names.iter().map(|_| Handoff::Bone(b("body"))).collect();
    let mut set = |file_bone: &str, h: Handoff| {
        if let Some(k) = names.iter().position(|j| j == file_bone) {
            handoff[k] = h;
        }
    };
    set(
        bones.neck,
        Handoff::Chain(
            fitted.neck.to_vec(),
            vec![b("neck_1"), b("neck_2"), b("neck_3")],
            0.1 * s,
        ),
    );
    for h in bones.head {
        set(h, Handoff::Bone(b("head")));
    }
    set(bones.tail, Handoff::Bone(b("tail_1")));
    for leg in &fitted.legs {
        let [upper, lower, knee, fetlock] = leg.joints;
        let (body, upper_bone, lower_bone) =
            (b("body"), b(&leg.bone("upper")), b(&leg.bone("lower")));
        if leg.front {
            set(
                leg.file[0],
                Handoff::Chain(
                    vec![leg.top, upper, lower],
                    vec![body, upper_bone],
                    0.12 * s,
                ),
            );
            set(leg.file[1], Handoff::Bone(lower_bone));
        } else {
            set(leg.file[0], Handoff::Bone(body));
            set(
                leg.file[1],
                Handoff::Chain(
                    vec![leg.top, upper, lower, knee],
                    vec![body, upper_bone, lower_bone],
                    0.12 * s,
                ),
            );
        }
        set(
            leg.file[2],
            Handoff::Chain(
                vec![knee, fetlock, fetlock.with_y(ground)],
                vec![b(&leg.bone("cannon")), b(&leg.bone("hoof"))],
                0.04 * s,
            ),
        );
    }
    let n = logic.names.len();
    let leg_bone: Vec<bool> = logic
        .names
        .iter()
        .map(|b| b.starts_with("front_") || b.starts_with("hind_"))
        .collect();
    let dirs = bone_directions(&logic);
    let mut acc = vec![0.0f32; n];
    for (v, is_coat) in mesh.vertices.iter_mut().zip(&coat) {
        acc.fill(0.0);
        for k in 0..4 {
            let w = v.weights[k];
            if w <= 0.0 {
                continue;
            }
            match handoff.get(v.joints[k] as usize) {
                Some(Handoff::Chain(joints, chain, blend)) => {
                    chain_weights(v.position, joints, chain, *blend, w, &mut acc)
                }
                Some(Handoff::Bone(b)) => acc[*b] += w,
                None => acc[b("body")] += w,
            }
        }
        (v.joints, v.weights) = top4(acc.iter().enumerate().map(|(i, w)| (i as u16, *w)));
        v.pattern = v.position;
        v.axis = blend_axis(v, &dirs);
        v.mask = if !*is_coat {
            Vec4::ZERO
        } else if leg_bone[v.joints[0] as usize] {
            COAT_MASK
        } else {
            COAT_MASK.with_z(TRUNK_VEINS)
        };
    }
    mesh.joints = logic.names.to_vec();
    mesh.inverse_bind = logic.inverse_bind.clone();

    // Muscles (before the hair, which they leave alone), each kept to its own side.
    let shapes = muscle_shapes(&mesh, &logic, &fitted);
    let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.position.x).collect();
    add_muscle_morphs(&mut mesh, &shapes, |i, m| {
        let side = if m.name.ends_with("_l") {
            -1.0
        } else if m.name.ends_with("_r") {
            1.0
        } else {
            0.0
        };
        coat.get(i).copied().unwrap_or(false) && xs[i] * side >= 0.0
    });

    // Mane, forelock and tail.
    let hair_surface = materials.len();
    let skin = Skin::new(
        mesh.vertices
            .iter()
            .zip(&coat)
            .filter(|(_, c)| **c)
            .map(|(v, _)| (v.position, v.normal))
            .collect(),
        0.06 * s,
    );
    let hair = grow_hair(&fitted, &logic, &skin);
    append_surface(&mut mesh, hair, hair_surface);
    images.push(hair_card_image(HAIR_CUTOFF));
    materials.push(ImportedMaterial {
        name: "mane and tail".into(),
        base_color: [0.16, 0.042, 0.015, 1.0],
        base_color_texture: Some(images.len() - 1),
        normal_texture: None,
        metallic_roughness_texture: None,
        roughness: 0.45,
        metallic: 0.0,
        alpha_cutoff: Some(HAIR_CUTOFF),
        double_sided: true,
    });

    let mut gait = horse::gait();
    for leg in &mut gait.legs {
        if let Some(hoof) = leg.bones.last().and_then(|b| logic.index_of(b)) {
            leg.sole = logic.rest_model[hoof].t.y - ground;
        }
        leg.foot_size = leg.foot_size.map(|f| f * s);
    }
    gait.step_height *= s;
    gait.step_height_per_speed *= s;
    let tail = SpringChainDef {
        tip_length: fitted.tail[4].distance(fitted.tail[3]),
        ..horse::tail()
    };
    Ok(HorseCharacter {
        logic,
        mesh,
        materials,
        images,
        gait,
        tail,
    })
}

/// The horse skeleton at the fitted joints (identity rest rotations, like
/// [`horse::skeleton`], so its clips and gait work unchanged).
fn fit_skeleton(name: &str, f: &Fitted) -> Result<Skeleton, String> {
    let i = Quat::IDENTITY;
    let mut defs = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), f.body, i),
        bone("neck_1", Some("body"), f.neck[0] - f.body, i),
        bone("neck_2", Some("neck_1"), f.neck[1] - f.neck[0], i),
        bone("neck_3", Some("neck_2"), f.neck[2] - f.neck[1], i),
        bone("head", Some("neck_3"), f.neck[3] - f.neck[2], i),
        bone("tail_1", Some("body"), f.tail[0] - f.body, i),
        bone("tail_2", Some("tail_1"), f.tail[1] - f.tail[0], i),
        bone("tail_3", Some("tail_2"), f.tail[2] - f.tail[1], i),
        bone("tail_4", Some("tail_3"), f.tail[3] - f.tail[2], i),
    ];
    for leg in &f.legs {
        let [upper, lower, cannon, hoof] =
            ["upper", "lower", "cannon", "hoof"].map(|p| leg.bone(p));
        let j = leg.joints;
        defs.push(bone(&upper, Some("body"), j[0] - f.body, i));
        defs.push(bone(&lower, Some(&upper), j[1] - j[0], i));
        defs.push(bone(&cannon, Some(&lower), j[2] - j[1], i));
        defs.push(bone(&hoof, Some(&cannon), j[3] - j[2], i));
    }
    Skeleton::new(name, defs)
}

/// The horse's muscles (named like [`horse::muscles`]' targets) placed on the model: each
/// leg muscle sits just inside the outer surface of its limb.
fn muscle_shapes(mesh: &SkinnedMesh, logic: &Skeleton, f: &Fitted) -> Vec<MuscleShape> {
    let s = f.s;
    let shape = |name: String, center: Vec3, radii: Vec3, bulge: f32, sway: Vec3| MuscleShape {
        name,
        center,
        radii: radii * s,
        rotation: Quat::IDENTITY,
        bulge: bulge * s,
        sway: sway * s,
    };
    // `p` moved out to just inside the skin of `bone` on the leg's side.
    let lateral = |p: Vec3, sign: f32, bone: &str| -> Vec3 {
        let b = logic.index_of(bone).map_or(u16::MAX, |b| b as u16);
        let surface = mesh
            .vertices
            .iter()
            .filter(|v| {
                v.joints[0] == b
                    && Vec2::new(v.position.y - p.y, v.position.z - p.z).length() < 0.14 * s
            })
            .map(|v| v.position.x * sign)
            .fold(f32::MIN, f32::max);
        if surface == f32::MIN {
            p
        } else {
            p.with_x((surface - 0.06 * s) * sign)
        }
    };
    let mut out = Vec::new();
    for leg in &f.legs {
        let sign = leg.sign();
        let suffix = ["l", "r"][leg.side];
        let [top, mid, low, _] = leg.joints;
        if leg.front {
            out.push(shape(
                format!("shoulder_{suffix}"),
                lateral(top.lerp(mid, 0.5), sign, &leg.bone("upper")),
                Vec3::new(0.13, 0.28, 0.2),
                0.02,
                Vec3::new(0.0, -0.006, 0.0),
            ));
            out.push(shape(
                format!("forearm_{suffix}"),
                lateral(mid.lerp(low, 0.3), sign, &leg.bone("lower")),
                Vec3::new(0.08, 0.17, 0.09),
                0.014,
                Vec3::ZERO,
            ));
        } else {
            out.push(shape(
                format!("haunch_{suffix}"),
                lateral(top.lerp(mid, 0.4), sign, &leg.bone("upper")),
                Vec3::new(0.16, 0.3, 0.3),
                0.028,
                Vec3::new(0.0, -0.01, 0.0),
            ));
            out.push(shape(
                format!("gaskin_{suffix}"),
                lateral(mid.lerp(low, 0.3), sign, &leg.bone("lower")),
                Vec3::new(0.09, 0.17, 0.1),
                0.014,
                Vec3::ZERO,
            ));
        }
    }
    out.push(shape(
        "neck".into(),
        f.neck[0].lerp(f.neck[2], 0.5),
        Vec3::new(0.25, 0.22, 0.22),
        0.014,
        Vec3::ZERO,
    ));
    let body = logic.index_of("body").map_or(u16::MAX, |b| b as u16);
    let belly = mesh
        .vertices
        .iter()
        .filter(|v| v.joints[0] == body && (v.position.z - f.body.z).abs() < 0.2 * s)
        .map(|v| v.position.y)
        .fold(f.body.y, f32::min);
    out.push(shape(
        "belly".into(),
        Vec3::new(0.0, belly + 0.15 * s, f.body.z),
        Vec3::new(0.3, 0.18, 0.55),
        0.004,
        Vec3::new(0.0, -0.03, 0.0),
    ));
    out
}

/// Each bone's direction (toward its first child; leaves continue their parent's).
fn bone_directions(s: &Skeleton) -> Vec<Vec3> {
    (0..s.names.len())
        .map(|b| {
            let here = s.rest_model[b].t;
            let child = s.parents.iter().position(|p| *p == Some(b as u16));
            match (child, s.parents[b]) {
                (Some(c), _) => s.rest_model[c].t - here,
                (None, Some(p)) => here - s.rest_model[p as usize].t,
                (None, None) => Vec3::Y,
            }
            .normalize_or(Vec3::Y)
        })
        .collect()
}

/// A vertex's direction along its limb: its bones' directions, weighted.
fn blend_axis(v: &SkinVertex, dirs: &[Vec3]) -> Vec3 {
    let mut axis = Vec3::ZERO;
    for k in 0..4 {
        let d = dirs.get(v.joints[k] as usize).copied().unwrap_or(Vec3::Y);
        axis += if axis.dot(d) < 0.0 { -d } else { d } * v.weights[k];
    }
    axis.normalize_or(Vec3::Y)
}

/// The four heaviest `(joint, weight)` pairs, normalized.
fn top4(pairs: impl Iterator<Item = (u16, f32)>) -> ([u16; 4], [f32; 4]) {
    let mut best: Vec<(u16, f32)> = pairs.filter(|p| p.1 > 1e-4).collect();
    best.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    best.truncate(4);
    let total = best.iter().map(|b| b.1).sum::<f32>().max(1e-6);
    let mut joints = [0u16; 4];
    let mut weights = [0.0f32; 4];
    for (k, (j, w)) in best.into_iter().enumerate() {
        joints[k] = j;
        weights[k] = w / total;
    }
    if total <= 1e-6 {
        weights[0] = 1.0;
    }
    (joints, weights)
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Hand `w` to the bones along a chain (`joints[i]` starts `bones[i]`; the last joint ends
/// the chain): the bone whose segment is nearest `p` takes it, shared with its neighbor
/// over `blend` meters on each side of a joint.
fn chain_weights(p: Vec3, joints: &[Vec3], bones: &[usize], blend: f32, w: f32, out: &mut [f32]) {
    let mut best = (f32::MAX, 0usize, 0.0f32, 0.0f32);
    for k in 0..bones.len().min(joints.len().saturating_sub(1)) {
        let (a, d) = (joints[k], joints[k + 1] - joints[k]);
        let len = d.length();
        let t = if len > 1e-6 {
            ((p - a).dot(d) / (len * len)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dist = p.distance(a + d * t);
        if dist < best.0 {
            best = (dist, k, t * len, len);
        }
    }
    let (_, k, along, len) = best;
    let Some(&own_bone) = bones.get(k) else {
        return;
    };
    let blend = blend.max(1e-4);
    let share = |x: f32| 0.5 * (1.0 - smoothstep(x));
    let mut own = w;
    if k > 0 {
        let a = share(along / blend) * w;
        out[bones[k - 1]] += a;
        own -= a;
    }
    if k + 1 < bones.len() {
        let a = share((len - along) / blend) * w;
        out[bones[k + 1]] += a;
        own -= a;
    }
    out[own_bone] += own;
}

fn centroid(points: &[Vec3]) -> Option<Vec3> {
    (!points.is_empty()).then(|| points.iter().copied().sum::<Vec3>() / points.len() as f32)
}

/// The surface (submesh's material index) of every vertex.
fn vertex_surfaces(mesh: &SkinnedMesh) -> Vec<usize> {
    let mut out = vec![0; mesh.vertices.len()];
    for sm in &mesh.submeshes {
        let range = sm.first_index as usize..(sm.first_index + sm.index_count) as usize;
        for &i in mesh.indices.get(range).unwrap_or(&[]) {
            if let Some(s) = out.get_mut(i as usize) {
                *s = sm.surface;
            }
        }
    }
    out
}

/// One level of curved (PN-triangle) subdivision of the submeshes whose edges average more
/// than `min_edge` meters: every triangle becomes four, new points lie on the curved surface
/// the vertex normals describe, and the original vertices stay where they are. Points are
/// matched across UV seams so the surface stays closed. Meshes with morph targets are left
/// as they are.
pub fn subdivide(mesh: &mut SkinnedMesh, min_edge: f32) {
    if !mesh.morph_targets.is_empty() || mesh.indices.len() < 3 {
        return;
    }
    // Shared normals per position, so both sides of a seam bend alike.
    let key = |p: Vec3| {
        let q = (p * 1e4).round();
        (q.x as i64, q.y as i64, q.z as i64)
    };
    let mut groups: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut summed: Vec<Vec3> = Vec::new();
    let weld: Vec<usize> = mesh
        .vertices
        .iter()
        .map(|v| {
            let id = *groups.entry(key(v.position)).or_insert_with(|| {
                summed.push(Vec3::ZERO);
                summed.len() - 1
            });
            summed[id] += v.normal;
            id
        })
        .collect();
    let smooth: Vec<Vec3> = summed.iter().map(|n| n.normalize_or(Vec3::Y)).collect();
    let submeshes = if mesh.submeshes.is_empty() {
        vec![Submesh {
            first_index: 0,
            index_count: mesh.indices.len() as u32,
            surface: 0,
        }]
    } else {
        mesh.submeshes.clone()
    };
    let old = std::mem::take(&mut mesh.indices);
    let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut out_subs = Vec::new();
    for sm in &submeshes {
        let range = sm.first_index as usize..(sm.first_index + sm.index_count) as usize;
        let tris = old.get(range).unwrap_or(&[]);
        let first = mesh.indices.len() as u32;
        let mut total = 0.0;
        for t in tris.as_chunks::<3>().0 {
            for e in 0..3 {
                let (a, b) = (t[e] as usize, t[(e + 1) % 3] as usize);
                total += mesh.vertices[a]
                    .position
                    .distance(mesh.vertices[b].position);
            }
        }
        let mean = total / (tris.len().max(1)) as f32;
        if mean < min_edge {
            mesh.indices.extend_from_slice(tris);
        } else {
            for t in tris.as_chunks::<3>().0 {
                let [a, b, c] = *t;
                let mut mid = |u: u32, v: u32| -> u32 {
                    *mids.entry((u.min(v), u.max(v))).or_insert_with(|| {
                        let (p, q) = (&mesh.vertices[u as usize], &mesh.vertices[v as usize]);
                        let vertex =
                            pn_midpoint(p, q, smooth[weld[u as usize]], smooth[weld[v as usize]]);
                        mesh.vertices.push(vertex);
                        mesh.vertices.len() as u32 - 1
                    })
                };
                let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
                mesh.indices
                    .extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
            }
        }
        out_subs.push(Submesh {
            first_index: first,
            index_count: mesh.indices.len() as u32 - first,
            surface: sm.surface,
        });
    }
    if !mesh.submeshes.is_empty() {
        mesh.submeshes = out_subs;
    }
}

/// The middle of the curved edge from `a` to `b` (PN triangles), positioned with the
/// shared normals `na`/`nb` and shaded with the vertices' own.
fn pn_midpoint(a: &SkinVertex, b: &SkinVertex, na: Vec3, nb: Vec3) -> SkinVertex {
    let d = b.position - a.position;
    let position = (a.position + b.position) * 0.5 - (na * d.dot(na) + nb * (-d).dot(nb)) * 0.125;
    let curve = |n0: Vec3, n1: Vec3| {
        let v = 2.0 * d.dot(n0 + n1) / d.length_squared().max(1e-12);
        (n0 + n1 - d * v).normalize_or(n0)
    };
    let mut pairs: Vec<(u16, f32)> = Vec::with_capacity(8);
    for v in [a, b] {
        for k in 0..4 {
            let w = v.weights[k] * 0.5;
            if w <= 0.0 {
                continue;
            }
            match pairs.iter_mut().find(|p| p.0 == v.joints[k]) {
                Some(p) => p.1 += w,
                None => pairs.push((v.joints[k], w)),
            }
        }
    }
    let (joints, weights) = top4(pairs.into_iter());
    SkinVertex {
        position,
        normal: curve(a.normal, b.normal),
        uv: (a.uv + b.uv) * 0.5,
        color: (a.color + b.color) * 0.5,
        joints,
        weights,
        pattern: position,
        axis: (a.axis + b.axis).normalize_or(a.axis),
        mask: (a.mask + b.mask) * 0.5,
        fur: (a.fur + b.fur) * 0.5,
    }
}

/// Append triangles drawn with their own surface.
fn append_surface(
    mesh: &mut SkinnedMesh,
    (vertices, indices): (Vec<SkinVertex>, Vec<u32>),
    surface: usize,
) {
    if mesh.submeshes.is_empty() {
        mesh.submeshes.push(Submesh {
            first_index: 0,
            index_count: mesh.indices.len() as u32,
            surface: 0,
        });
    }
    let base = mesh.vertices.len() as u32;
    let first = mesh.indices.len() as u32;
    mesh.vertices.extend(vertices);
    mesh.indices.extend(indices.iter().map(|i| i + base));
    mesh.submeshes.push(Submesh {
        first_index: first,
        index_count: indices.len() as u32,
        surface,
    });
}

/// The coat's vertices in a hash grid, for keeping hair outside the body.
struct Skin {
    cell: f32,
    grid: HashMap<(i32, i32, i32), Vec<u32>>,
    points: Vec<(Vec3, Vec3)>,
}

impl Skin {
    fn new(points: Vec<(Vec3, Vec3)>, cell: f32) -> Self {
        let cell = cell.max(1e-3);
        let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, (p, _)) in points.iter().enumerate() {
            let c = (*p / cell).floor();
            grid.entry((c.x as i32, c.y as i32, c.z as i32))
                .or_default()
                .push(i as u32);
        }
        Self { cell, grid, points }
    }

    /// The nearest skin point and its normal within a cell or so of `p`.
    fn nearest(&self, p: Vec3) -> Option<(Vec3, Vec3)> {
        let c = (p / self.cell).floor();
        let (cx, cy, cz) = (c.x as i32, c.y as i32, c.z as i32);
        let mut best: Option<(f32, u32)> = None;
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                for z in cz - 1..=cz + 1 {
                    for &i in self.grid.get(&(x, y, z)).map_or(&[][..], Vec::as_slice) {
                        let d = self.points[i as usize].0.distance_squared(p);
                        if best.is_none_or(|b| d < b.0) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        best.map(|(_, i)| self.points[i as usize])
    }

    /// `p` moved out along the nearest skin normal until it is `lift` above the skin.
    fn push_out(&self, p: Vec3, lift: f32) -> Vec3 {
        match self.nearest(p) {
            Some((q, n)) => {
                let d = (p - q).dot(n);
                if d < lift { p + n * (lift - d) } else { p }
            }
            None => p,
        }
    }
}

/// A tiny deterministic random source (hair placement must not change between runs).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// One hair strand: `segments` steps from `root`, leaving along `start` and turning toward
/// `fall` over the first `bend` of its length, kept `lift` above the skin.
#[allow(clippy::too_many_arguments)]
fn strand(
    root: Vec3,
    start: Vec3,
    fall: Vec3,
    length: f32,
    segments: usize,
    bend: f32,
    skin: &Skin,
    lift: f32,
) -> Vec<Vec3> {
    let step = length / segments as f32;
    let mut points = vec![root];
    let mut p = root;
    for k in 0..segments {
        let t = (k as f32 + 0.5) / segments as f32;
        let d = start
            .lerp(fall, smoothstep(t / bend.max(1e-3)))
            .normalize_or(fall);
        let q = skin.push_out(p + d * step, lift);
        p += (q - p).normalize_or(d) * step;
        points.push(p);
    }
    points
}

/// Hair cards under construction.
#[derive(Default)]
struct Cards {
    vertices: Vec<SkinVertex>,
    indices: Vec<u32>,
}

impl Cards {
    /// A card along `points`, facing `facing(point)`, `width` wide at the root and tip,
    /// textured with one column of the hair image and skinned by `skin(point)`.
    fn add(
        &mut self,
        points: &[Vec3],
        facing: impl Fn(Vec3) -> Vec3,
        width: [f32; 2],
        column: u32,
        skin: impl Fn(Vec3) -> ([u16; 4], [f32; 4]),
    ) {
        let n = points.len();
        if n < 2 {
            return;
        }
        let base = self.vertices.len() as u32;
        let u0 = column as f32 / HAIR_COLUMNS as f32;
        let u1 = (column + 1) as f32 / HAIR_COLUMNS as f32;
        for (k, &p) in points.iter().enumerate() {
            let t = k as f32 / (n - 1) as f32;
            let tangent = (points[(k + 1).min(n - 1)] - points[k.saturating_sub(1)])
                .normalize_or(Vec3::NEG_Y);
            let f = facing(p);
            let normal =
                (f - tangent * f.dot(tangent)).normalize_or(tangent.any_orthonormal_vector());
            let side = tangent.cross(normal).normalize_or(Vec3::X);
            let half = (width[0] + (width[1] - width[0]) * t) * 0.5;
            let (joints, weights) = skin(p);
            // Darker near the roots, inside the mass of hair.
            let shade = 0.55 + 0.45 * smoothstep(t / 0.35);
            for (x, u) in [(-half, u0), (half, u1)] {
                self.vertices.push(SkinVertex {
                    position: p + side * x,
                    normal,
                    uv: Vec2::new(u, t),
                    color: Vec4::new(shade, shade, shade, 1.0),
                    joints,
                    weights,
                    pattern: p + side * x,
                    axis: tangent,
                    mask: HAIR_MASK,
                    fur: 0.0,
                });
            }
        }
        for k in 0..n as u32 - 1 {
            let (a, b, c, d) = (
                base + 2 * k,
                base + 2 * k + 1,
                base + 2 * k + 2,
                base + 2 * k + 3,
            );
            self.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
}

/// The mane along the crest (falling to the horse's right, a few locks to the left), the
/// forelock over the forehead, and the tail hanging from the dock to below the hocks.
fn grow_hair(f: &Fitted, logic: &Skeleton, skin: &Skin) -> (Vec<SkinVertex>, Vec<u32>) {
    let s = f.s;
    let b = |n: &str| logic.index_of(n).unwrap_or(0);
    let n = logic.names.len();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut cards = Cards::default();
    let weights_on = |joints: &[Vec3], bones: &[usize], blend: f32, p: Vec3| {
        let mut acc = vec![0.0f32; n];
        chain_weights(p, joints, bones, blend, 1.0, &mut acc);
        top4(acc.iter().enumerate().map(|(i, w)| (i as u16, *w)))
    };
    let facing_skin = |fallback: Vec3| move |p: Vec3| skin.nearest(p).map_or(fallback, |(_, n)| n);

    // Mane: three layers of cards from the crest, each card moving with the neck bone at
    // its root.
    let head_dir = (f.muzzle - f.neck[3]).normalize_or(Vec3::new(0.0, -0.7, -0.7));
    let neck_joints = [
        f.neck[0],
        f.neck[1],
        f.neck[2],
        f.neck[3],
        f.neck[3] + head_dir * 0.3 * s,
    ];
    let neck_bones = [b("neck_1"), b("neck_2"), b("neck_3"), b("head")];
    let axis = (f.neck[3] - f.neck[0]).normalize_or(Vec3::NEG_Z);
    let dorsal = (Vec3::Y - axis * Vec3::Y.dot(axis)).normalize_or(Vec3::Y);
    let crest_at = |t: f32| {
        let x = t.clamp(0.0, 1.0) * (f.crest.len() - 1) as f32;
        let i = (x as usize).min(f.crest.len() - 2);
        f.crest[i].lerp(f.crest[i + 1], x - i as f32)
    };
    let roots = 80;
    for layer in 0..3 {
        for i in 0..roots {
            let t = 0.04 + 0.92 * (i as f32 + rng.next()) / roots as f32;
            let root = crest_at(t) - dorsal * 0.008 * s + Vec3::X * rng.range(-0.008, 0.008) * s;
            let side = if rng.next() < 0.1 { -1.0 } else { 1.0 };
            let lateral = Vec3::X * side;
            let start =
                (dorsal * 0.7 + lateral * (0.6 + 0.1 * layer as f32) - axis * 0.12).normalize();
            let fall = (Vec3::NEG_Y + lateral * 0.12 - axis * 0.12).normalize();
            let profile = (std::f32::consts::PI * t.powf(0.8)).sin();
            let length =
                s * (0.15 + 0.2 * profile) * (0.75 + 0.12 * layer as f32 + rng.range(0.0, 0.25));
            // Each layer lies on the one under it, so the mane has body.
            let lift = (0.01 + 0.012 * layer as f32) * s;
            let points = strand(
                root,
                start,
                fall,
                length,
                10,
                rng.range(0.14, 0.24),
                skin,
                lift,
            );
            let skinning = weights_on(&neck_joints, &neck_bones, 0.1 * s, root);
            cards.add(
                &points,
                facing_skin(lateral),
                [0.06 * s, 0.04 * s],
                (rng.next() * HAIR_COLUMNS as f32) as u32 % HAIR_COLUMNS,
                |_| skinning,
            );
        }
    }

    // Forelock: from the top of the poll forward over the forehead.
    let head = b("head");
    let on_head = |_: Vec3| top4([(head as u16, 1.0)].into_iter());
    for _ in 0..16 {
        let root =
            f.forelock + Vec3::new(rng.range(-0.035, 0.035), -0.008, rng.range(-0.025, 0.02)) * s;
        let start = (head_dir * 0.6 + Vec3::Y * 0.5).normalize();
        let length = s * rng.range(0.18, 0.27);
        let points = strand(root, start, head_dir, length, 8, 0.3, skin, 0.008 * s);
        cards.add(
            &points,
            facing_skin(Vec3::Y),
            [0.04 * s, 0.03 * s],
            (rng.next() * HAIR_COLUMNS as f32) as u32 % HAIR_COLUMNS,
            on_head,
        );
    }

    // Tail: rings of cards around the dock, bending with the tail bones.
    let tail_bones = [b("tail_1"), b("tail_2"), b("tail_3"), b("tail_4")];
    let dock_end = f.tail[1].lerp(f.tail[2], 0.45);
    let dock = |u: f32| {
        if u < 0.5 {
            f.tail[0].lerp(f.tail[1], u * 2.0)
        } else {
            f.tail[1].lerp(dock_end, (u - 0.5) * 2.0)
        }
    };
    let center_at = |p: Vec3| {
        // The tail's centerline at `p`'s height.
        let mut best = (f32::MAX, f.tail[0]);
        for k in 0..4 {
            let (a, d) = (f.tail[k], f.tail[k + 1] - f.tail[k]);
            let t = ((p - a).dot(d) / d.length_squared().max(1e-9)).clamp(0.0, 1.0);
            let q = a + d * t;
            if q.distance(p) < best.0 {
                best = (q.distance(p), q);
            }
        }
        best.1
    };
    for ring in 0..11 {
        let u = (ring as f32 + 0.5) / 11.0;
        let center = dock(u);
        let along =
            (dock((u + 0.05).min(1.0)) - dock((u - 0.05).max(0.0))).normalize_or(Vec3::NEG_Y);
        let across = Vec3::X.cross(along).normalize_or(Vec3::Z);
        for j in 0..7 {
            let angle = (j as f32 + rng.next() * 0.8) / 7.0 * std::f32::consts::TAU;
            let radial = Vec3::X * angle.cos() + across * angle.sin();
            let root = center + radial * 0.035 * s;
            let flat = radial.with_y(0.0).normalize_or(Vec3::Z);
            let start = (radial * 0.5 + along * 0.5 + Vec3::Z * 0.25).normalize();
            let fall = (Vec3::NEG_Y + Vec3::Z * 0.06 + flat * rng.range(0.1, 0.22)).normalize();
            let tip = f.tail[4].y + rng.range(-0.06, 0.05) * s;
            let length = (root.y - tip).max(0.2 * s) * 1.06;
            let points = strand(root, start, fall, length, 14, 0.16, skin, 0.01 * s);
            cards.add(
                &points,
                |p: Vec3| (p - center_at(p)).with_y(0.0).normalize_or(Vec3::Z),
                [0.07 * s, 0.065 * s],
                (rng.next() * HAIR_COLUMNS as f32) as u32 % HAIR_COLUMNS,
                |p| weights_on(&f.tail, &tail_bones, 0.08 * s, p),
            );
        }
    }
    (cards.vertices, cards.indices)
}

/// Strands for hair cards: [`HAIR_COLUMNS`] card designs side by side, grey strands on
/// transparency with roots at the top, tinted by the material. The mips keep the strands'
/// coverage at `cutoff`, so cards don't thin out with distance.
pub fn hair_card_image(cutoff: f32) -> Image {
    const W: usize = 256;
    const H: usize = 512;
    let column = W / HAIR_COLUMNS as usize;
    let mut lum = vec![0.6f32; W * H];
    let mut alpha = vec![0.0f32; W * H];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for col in 0..HAIR_COLUMNS as usize {
        let left = (col * column) as f32;
        for _ in 0..26 {
            let x0 = left + rng.range(4.0, column as f32 - 4.0);
            let drift = rng.range(-5.0, 5.0);
            let (amp, freq, phase) = (
                rng.range(0.0, 2.0),
                rng.range(1.0, 3.5),
                rng.range(0.0, 6.3),
            );
            let width = rng.range(1.1, 2.4);
            let (v0, v1) = (rng.range(0.0, 0.06), rng.range(0.55, 1.0));
            let bright = rng.range(0.42, 0.78);
            for y in (v0 * H as f32) as usize..((v1 * H as f32) as usize).min(H) {
                let v = y as f32 / H as f32;
                let along = (v - v0) / (v1 - v0);
                let cx =
                    x0 + drift * along + amp * (std::f32::consts::TAU * freq * v + phase).sin();
                let w = width * (1.0 - 0.75 * along.powi(3));
                let fade = ((1.0 - along) / 0.15).min(1.0);
                let lo = (cx - w - 1.0).floor().max(left) as usize;
                let hi = (cx + w + 1.0).ceil().min(left + column as f32 - 1.0) as usize;
                for x in lo..=hi {
                    let cover =
                        (w * 0.5 + 0.5 - (x as f32 + 0.5 - cx).abs()).clamp(0.0, 1.0) * fade;
                    let i = y * W + x;
                    lum[i] += (bright - lum[i]) * cover;
                    alpha[i] += cover * (1.0 - alpha[i]);
                }
            }
        }
    }
    let to_srgb = |l: f32| {
        let l = l.clamp(0.0, 1.0);
        let s = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    let mut data = Vec::with_capacity(W * H * 4);
    for (l, a) in lum.iter().zip(&alpha) {
        let g = to_srgb(*l);
        data.extend_from_slice(&[g, g, g, (a.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    let mut image = Image::new(W as u32, H as u32, data, true);
    let target = coverage(&image.data, cutoff);
    let mut mips = image.mip_chain(true);
    for level in &mut mips {
        let (mut lo, mut hi) = (0.25f32, 8.0f32);
        for _ in 0..16 {
            let mid = (lo + hi) * 0.5;
            if coverage_scaled(level, cutoff, mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let k = (lo + hi) * 0.5;
        for px in level.as_chunks_mut::<4>().0 {
            px[3] = (px[3] as f32 * k).min(255.0).round() as u8;
        }
    }
    image.mips = mips;
    image
}

/// Share of texels with alpha at or above `cutoff`.
fn coverage(rgba: &[u8], cutoff: f32) -> f32 {
    coverage_scaled(rgba, cutoff, 1.0)
}

fn coverage_scaled(rgba: &[u8], cutoff: f32, scale: f32) -> f32 {
    let n = (rgba.len() / 4).max(1);
    let threshold = cutoff * 255.0;
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] as f32 * scale >= threshold)
        .count() as f32
        / n as f32
}

/// Gives a horse (an entity with the horse rig: `AnimatorSettings`, `LeggedGait`,
/// `MuscleRig`, `SkinnedMeshRenderer`, as `demo::spawn_horse` makes it) the body of a
/// rigged horse file with [`RIGGED_HORSE`]'s bones. The file is loaded and prepared on a
/// background thread, once per path however many horses use it; until it is ready (or if it
/// can't be loaded) the horse keeps the body it has.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HorseModel {
    /// File path of the model.
    pub path: String,
    /// Uniform scale applied to the file (the paddock's horse file stands 1.9 m at the
    /// withers; 0.85 makes it a 1.6 m riding horse).
    pub scale: f32,
}

impl Default for HorseModel {
    fn default() -> Self {
        Self {
            path: String::new(),
            scale: 1.0,
        }
    }
}

impl HorseModel {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    fn key(&self) -> String {
        format!("{}|{}", self.path, self.scale)
    }
}

/// A loaded horse, shared by every horse wearing it.
#[derive(Debug, Clone)]
pub struct LoadedHorse {
    /// Name of its fitted skeleton in the `AnimationLibrary`.
    pub skeleton: String,
    pub mesh: Handle<SkinnedMesh>,
    pub surfaces: Vec<StandardMaterial>,
    pub gait: LeggedGait,
    pub tail: SpringChainDef,
}

enum Load {
    Pending(Mutex<Receiver<Result<HorseCharacter, String>>>),
    Ready(Arc<LoadedHorse>),
    Failed(String),
}

/// Every [`HorseModel`] requested so far.
#[derive(Resource, Default)]
pub struct HorseModels {
    loads: HashMap<String, Load>,
}

impl HorseModels {
    fn get(&self, key: &str) -> Option<Arc<LoadedHorse>> {
        match self.loads.get(key) {
            Some(Load::Ready(l)) => Some(l.clone()),
            _ => None,
        }
    }

    /// A loaded model, once it is ready.
    pub fn loaded(&self, model: &HorseModel) -> Option<Arc<LoadedHorse>> {
        self.get(&model.key())
    }

    /// Why a model couldn't be loaded.
    pub fn error(&self, model: &HorseModel) -> Option<&str> {
        match self.loads.get(&model.key()) {
            Some(Load::Failed(e)) => Some(e),
            _ => None,
        }
    }
}

/// Which [`HorseModel`] an entity's current body came from.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct AppliedHorseModel(pub String);

/// Load and prepare a [`HorseModel`] (blocking).
pub fn load_horse(model: &HorseModel) -> Result<HorseCharacter, String> {
    let path = &model.path;
    let file = Path::new(path);
    let bytes = std::fs::read(file).map_err(|e| format!("{path}: {e}"))?;
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("horse")
        .to_string();
    let imported = import_gltf(
        &bytes,
        file.parent(),
        &ImportOptions {
            name: name.clone(),
            scale: model.scale,
            ..Default::default()
        },
    )
    .map_err(|e| format!("{path}: {e}"))?;
    from_rigged_horse(&name, imported, &RIGGED_HORSE)
}

fn register(world: &mut World, c: HorseCharacter) -> LoadedHorse {
    let handles: Vec<Handle<Image>> = {
        let mut images = world.get_resource_or_init::<Assets<Image>>();
        c.images.into_iter().map(|i| images.add(i)).collect()
    };
    let surfaces = c
        .materials
        .iter()
        .map(|m| m.to_standard(&handles))
        .collect();
    let mesh = world
        .get_resource_or_init::<Assets<SkinnedMesh>>()
        .add(c.mesh);
    let logic = world
        .get_resource_or_init::<AnimationLibrary>()
        .add_skeleton(c.logic);
    LoadedHorse {
        skeleton: logic.name.clone(),
        mesh,
        surfaces,
        gait: c.gait,
        tail: c.tail,
    }
}

fn wear(world: &mut World, entity: Entity, key: String, loaded: &LoadedHorse) {
    let names = world
        .get_resource::<Assets<SkinnedMesh>>()
        .and_then(|m| m.get(loaded.mesh))
        .map(|m| m.morph_names())
        .unwrap_or_else(|| Vec::new().into());
    let Ok(mut e) = world.get_entity_mut(entity) else {
        return;
    };
    if let Some(mut s) = e.get_mut::<AnimatorSettings>() {
        s.skeleton = loaded.skeleton.clone();
    }
    let renderer = e.get::<SkinnedMeshRenderer>().cloned().unwrap_or_default();
    let muscles = e.get::<MuscleRig>().cloned().unwrap_or_default();
    let mut rig = MuscleRig::new(horse::muscles());
    rig.strength = muscles.strength;
    rig.jiggle = muscles.jiggle;
    rig.boost = muscles.boost;
    rig.enabled = muscles.enabled;
    let enabled = e.get::<LeggedGait>().is_none_or(|g| g.enabled);
    e.insert((
        SkinnedMeshRenderer {
            mesh: Some(loaded.mesh),
            surfaces: loaded.surfaces.clone(),
            ..renderer
        },
        MorphWeights::new(names),
        rig,
        LeggedGait {
            enabled,
            ..loaded.gait.clone()
        },
        SpringChains::new(vec![loaded.tail.clone()]),
        AppliedHorseModel(key),
    ));
}

/// Start loads for newly requested files, register finished ones and dress the horses
/// waiting for them. With `block`, waits for every pending load first.
fn update_models(world: &mut World, block: bool) {
    let mut wanted: Vec<(Entity, HorseModel)> = Vec::new();
    let mut q = world.query::<(Entity, &HorseModel, Option<&AppliedHorseModel>)>();
    for (e, model, applied) in q.iter(world) {
        if !model.path.is_empty() && applied.is_none_or(|a| a.0 != model.key()) {
            wanted.push((e, model.clone()));
        }
    }
    if wanted.is_empty() {
        return;
    }
    let mut finished: Vec<(String, Result<HorseCharacter, String>)> = Vec::new();
    {
        let mut models = world.get_resource_or_init::<HorseModels>();
        for (_, m) in &wanted {
            models.loads.entry(m.key()).or_insert_with(|| {
                let (tx, rx) = channel();
                let model = m.clone();
                let spawned = std::thread::Builder::new()
                    .name("horse model".into())
                    .spawn(move || {
                        let _ = tx.send(load_horse(&model));
                    });
                match spawned {
                    Ok(_) => Load::Pending(Mutex::new(rx)),
                    Err(e) => Load::Failed(format!("could not start loading: {e}")),
                }
            });
        }
        for (key, load) in models.loads.iter_mut() {
            if let Load::Pending(rx) = load {
                let Ok(rx) = rx.get_mut() else {
                    *load = Load::Failed("loader poisoned".into());
                    continue;
                };
                let result = if block {
                    rx.recv().map_err(|e| e.to_string())
                } else {
                    match rx.try_recv() {
                        Ok(r) => Ok(r),
                        Err(TryRecvError::Empty) => continue,
                        Err(e) => Err(e.to_string()),
                    }
                };
                match result {
                    Ok(r) => finished.push((key.clone(), r)),
                    Err(e) => *load = Load::Failed(format!("loader stopped: {e}")),
                }
            }
        }
    }
    for (key, result) in finished {
        let load = match result {
            Ok(c) => {
                tracing::info!("horse model {key} loaded");
                Load::Ready(Arc::new(register(world, c)))
            }
            Err(e) => {
                tracing::warn!("horse model {key}: {e}");
                Load::Failed(e)
            }
        };
        world.resource_mut::<HorseModels>().loads.insert(key, load);
    }
    for (entity, model) in wanted {
        let loaded = world.resource::<HorseModels>().get(&model.key());
        if let Some(loaded) = loaded {
            wear(world, entity, model.key(), &loaded);
        }
    }
}

pub(crate) fn load_horse_models(
    mut commands: Commands,
    q: Query<(&HorseModel, Option<&AppliedHorseModel>)>,
) {
    // A normal system that queues the world work only while a model is wanted, so once
    // everyone wears theirs it runs beside everything else instead of draining the schedule
    // as an exclusive system would every frame.
    if q.iter()
        .any(|(m, applied)| !m.path.is_empty() && applied.is_none_or(|a| a.0 != m.key()))
    {
        commands.queue(|world: &mut World| update_models(world, false));
    }
}

/// Wait until every requested [`HorseModel`] is loaded and worn.
pub fn finish_loading(world: &mut World) {
    update_models(world, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::muscle::MorphWeights;
    use crate::skeleton::BoneDef;

    /// The procedural horse dressed up as a file rigged like [`RIGGED_HORSE`]: one bone for
    /// the neck, one per lower leg, one at the tail root.
    fn rigged_like_the_file() -> ImportedModel {
        let logic = horse::skeleton();
        let at = |b: &str| logic.rest_model[logic.bone(b)].t;
        let mut placed: Vec<(&str, Vec3)> = vec![
            ("Bone", at("hind_upper_l").with_x(0.0)),
            ("Bone.003", at("hind_upper_l").with_x(0.0)),
            ("Bone.004", at("tail_1")),
            ("Bone.001", at("neck_1")),
            ("Bone.002", at("head")),
            ("Bone.001_L", at("head")),
            ("Bone.001_R", at("head")),
        ];
        let file_of = |b: &str| -> &'static str {
            let side = |l: &'static str, r: &'static str| if b.ends_with("_l") { l } else { r };
            match b {
                b if b.starts_with("neck") => "Bone.001",
                "head" => "Bone.002",
                b if b.starts_with("tail") => "Bone.004",
                b if b.starts_with("front_upper") => side("Bone_L", "Bone_R"),
                b if b.starts_with("front_lower") => side("Bone_L.001", "Bone_R.001"),
                b if b.starts_with("front_") => side("Bone_L.002", "Bone_R.002"),
                b if b.starts_with("hind_upper") || b.starts_with("hind_lower") => {
                    side("Bone_L.004", "Bone_R.004")
                }
                b if b.starts_with("hind_") => side("Bone_L.005", "Bone_R.005"),
                _ => "Bone",
            }
        };
        // Like the file: the front legs start at the withers and the hind legs at the croup
        // (a pelvis bone to the hip joint, then one bone from the hip to the hock).
        let croup = at("tail_1").lerp(at("body"), 0.4);
        for (file, place) in [
            ("Bone_L", at("neck_1").with_x(-0.02)),
            ("Bone_L.001", at("front_lower_l")),
            ("Bone_L.002", at("front_cannon_l")),
            ("Bone_R", at("neck_1").with_x(0.02)),
            ("Bone_R.001", at("front_lower_r")),
            ("Bone_R.002", at("front_cannon_r")),
            ("Bone_L.003", croup.with_x(-0.02)),
            ("Bone_L.004", at("hind_upper_l")),
            ("Bone_L.005", at("hind_cannon_l")),
            ("Bone_R.003", croup.with_x(0.02)),
            ("Bone_R.004", at("hind_upper_r")),
            ("Bone_R.005", at("hind_cannon_r")),
        ] {
            placed.push((file, place));
        }
        let mut defs = vec![BoneDef::new("Armature", None, Vec3::ZERO)];
        defs.extend(
            placed
                .iter()
                .map(|(n, p)| BoneDef::new(n, Some("Armature"), *p)),
        );
        let file = Skeleton::new("file", defs).unwrap();
        let mut mesh = horse::mesh();
        mesh.morph_targets.clear();
        for v in &mut mesh.vertices {
            let pairs = (0..4).map(|k| {
                let name = file_of(&logic.names[v.joints[k] as usize]);
                (file.bone(name) as u16, v.weights[k])
            });
            let mut merged: Vec<(u16, f32)> = Vec::new();
            for (j, w) in pairs {
                match merged.iter_mut().find(|m| m.0 == j) {
                    Some(m) => m.1 += w,
                    None => merged.push((j, w)),
                }
            }
            (v.joints, v.weights) = top4(merged.into_iter());
        }
        mesh.joints = file.names.to_vec();
        mesh.inverse_bind = file.inverse_bind.clone();
        ImportedModel {
            skeleton: file,
            mesh,
            materials: Vec::new(),
            images: Vec::new(),
            clips: Vec::new(),
            clip_info: Vec::new(),
            unit_scale: 1.0,
        }
    }

    #[test]
    fn a_horse_with_one_piece_legs_gets_fetlocks_muscles_mane_and_tail() {
        let model = rigged_like_the_file();
        let triangles = model.mesh.triangle_count();
        let c = from_rigged_horse("test", model, &RIGGED_HORSE).unwrap();
        let s = &c.logic;
        // Every bone of the horse rig, so its clips, gait and muscles apply.
        for name in horse::skeleton().names.iter() {
            assert!(s.index_of(name).is_some(), "{name}");
        }
        // Fetlocks a third of the way up to the knee, and the gait plants them there.
        for leg in &c.gait.legs {
            let hoof = s.rest_model[s.bone(&leg.bones[3])].t;
            let knee = s.rest_model[s.bone(&leg.bones[2])].t;
            assert!(
                (hoof.y - FETLOCK * knee.y).abs() < 0.02,
                "{} at {hoof}",
                leg.bones[3]
            );
            assert!(
                (leg.sole - hoof.y).abs() < 0.03,
                "{} sole {}",
                leg.bones[3],
                leg.sole
            );
        }
        // Smoothed: each coat triangle became four.
        let coat = c.mesh.submeshes[0];
        assert_eq!(coat.index_count as usize / 3, triangles * 4);
        // The one-piece lower legs now bend at the fetlock: pastern skin rides the hoof bone.
        let hoof = s.bone("front_hoof_l") as u16;
        let cannon = s.bone("front_cannon_l") as u16;
        let rides = |b: u16| {
            c.mesh.vertices[..coat_vertices(&c.mesh)]
                .iter()
                .filter(|v| v.joints[0] == b && v.weights[0] > 0.5)
                .count()
        };
        assert!(
            rides(hoof) > 20 && rides(cannon) > 20,
            "{} {}",
            rides(hoof),
            rides(cannon)
        );
        for v in &c.mesh.vertices {
            let total: f32 = v.weights.iter().sum();
            assert!((total - 1.0).abs() < 1e-3);
            assert!((v.joints[0] as usize) < s.names.len());
        }
        // Muscles for the horse's muscle rig.
        let morphs = MorphWeights::new(c.mesh.morph_names());
        for m in horse::muscles() {
            assert!(morphs.index_of(&m.bulge).is_some(), "{}", m.bulge);
        }
        // Hair: its own alpha-cut surface; mane on the neck, tail on the tail bones, and the
        // tail's tips hang below the hocks but clear of the ground.
        let hair = *c.mesh.submeshes.last().unwrap();
        assert_eq!(hair.surface, c.materials.len() - 1);
        assert!(c.materials[hair.surface].alpha_cutoff.is_some());
        let first = hair.first_index as usize;
        let hair_vertices: Vec<&SkinVertex> = c.mesh.indices
            [first..first + hair.index_count as usize]
            .iter()
            .map(|&i| &c.mesh.vertices[i as usize])
            .collect();
        assert!(hair_vertices.len() > 3000, "{}", hair_vertices.len());
        let on = |prefix: &str| {
            hair_vertices
                .iter()
                .filter(|v| s.names[v.joints[0] as usize].starts_with(prefix))
                .count()
        };
        assert!(
            on("neck") > 1000 && on("tail") > 1000,
            "{} {}",
            on("neck"),
            on("tail")
        );
        let hock = s.rest_model[s.bone("hind_cannon_l")].t.y;
        let lowest = hair_vertices
            .iter()
            .map(|v| v.position.y)
            .fold(f32::MAX, f32::min);
        assert!(
            lowest > 0.1 && lowest < hock,
            "tail ends at {lowest} (hock {hock})"
        );
        assert_eq!(c.tail.bones.len(), 4);
    }

    fn coat_vertices(mesh: &SkinnedMesh) -> usize {
        let hair = mesh.submeshes.last().unwrap();
        mesh.indices[hair.first_index as usize..]
            .iter()
            .copied()
            .min()
            .unwrap_or(0) as usize
    }

    #[test]
    fn hair_card_mips_keep_their_coverage() {
        let image = hair_card_image(HAIR_CUTOFF);
        let full = coverage(&image.data, HAIR_CUTOFF);
        assert!(full > 0.15 && full < 0.8, "{full}");
        for level in image.mips.iter().take(5) {
            let c = coverage(level, HAIR_CUTOFF);
            assert!((c - full).abs() < 0.05, "{c} vs {full}");
        }
        assert!(image.is_valid());
    }
}
