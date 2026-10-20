//! Imported people: characters rigged with 3ds Max Biped bones (`Bip01 ...`, used by the
//! Microsoft Rocketbox library and many other character packs) driven by the engine's
//! [`humanoid`] rig, loaded in the background with [`HumanModel`].
//!
//! [`from_biped`] takes an imported model and returns everything a character needs:
//! - a humanoid logic skeleton fitted to the model's proportions ([`humanoid::fit_skeleton`]),
//!   so every humanoid clip, IK rig and muscle driver works unchanged;
//! - the [`RetargetMap`] that drives the model's skeleton from it, with hands that close on
//!   the `grip`, `flex` and `fist` curves;
//! - the model's mesh with the humanoid's muscles added as bulge morph targets, placed from
//!   its joints and the shape of its own limbs;
//! - a detail mask from its textures: skin (subsurface, flush, veins on arms and legs) versus
//!   clothing (folds where it compresses, pull lines where it stretches, sheen);
//! - hair cards hanging behind the head re-weighted onto the humanoid's spring-driven
//!   ponytail, so a ponytail swings.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, StandardMaterial};
use engine_world::animation::BodyRequest;
use serde::{Deserialize, Serialize};

use crate::animator::{AnimationLibrary, AnimatorSettings};
use crate::face::Face;
use crate::muscle::{MorphWeights, MuscleRig};
use crate::procedural::spring::SpringChains;
use crate::retarget::{Retarget, Retargeter};
use crate::skin::{SkinMaterial, SkinnedMeshRenderer};

use crate::build::humanoid;
use crate::build::humanoid::Locomotion;
use crate::build::sdf::{MuscleShape, add_muscle_morphs};
use crate::clip::{AnimationClip, BoneTrack, ClipBinding};
use crate::ik::FootIk;
use crate::import::{ImportOptions, ImportedMaterial, ImportedModel, import_clips, import_gltf};
use crate::procedural::spring::SpringChainDef;
use crate::retarget::{HandRig, RetargetMap};
use crate::skeleton::Skeleton;
use crate::skin::SkinnedMesh;
use crate::xform::Xf;

/// A Biped-rigged person ready to spawn.
#[derive(Debug, Clone)]
pub struct HumanCharacter {
    pub name: String,
    /// The humanoid sized to this character (registered in the `AnimationLibrary`).
    pub logic: Skeleton,
    /// The character's own skeleton, which its mesh is skinned to.
    pub target: Arc<Skeleton>,
    pub map: RetargetMap,
    pub mesh: SkinnedMesh,
    pub materials: Vec<ImportedMaterial>,
    pub images: Vec<Image>,
    pub foot_ik: FootIk,
    /// Set when hair cards were found behind the head and weighted to the ponytail bones.
    pub ponytail: Option<SpringChainDef>,
    /// Motion-capture locomotion converted onto `logic` (see [`HumanModel`]).
    pub clips: Vec<AnimationClip>,
    pub locomotion: Option<Locomotion>,
}

const SIDES: [(&str, usize); 2] = [("L", 0), ("R", 1)];

/// Biped bone name for a side: `biped("L", "UpperArm")` → `"Bip01 L UpperArm"`.
fn biped(side: &str, bone: &str) -> String {
    format!("Bip01 {side} {bone}")
}

/// Humanoid bone → Biped bone.
pub fn biped_map() -> RetargetMap {
    let mut bones: Vec<(String, String)> = [
        ("hips", "Bip01"),
        ("hips", "Bip01 Pelvis"),
        ("spine", "Bip01 Spine"),
        ("chest", "Bip01 Spine2"),
        ("neck", "Bip01 Neck"),
        ("head", "Bip01 Head"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let mut hands = Vec::new();
    for (side, i) in SIDES {
        for (logic, b) in [
            (humanoid::CLAVICLE[i], "Clavicle"),
            (humanoid::UPPER_ARM[i], "UpperArm"),
            (humanoid::LOWER_ARM[i], "Forearm"),
            (humanoid::HAND[i], "Hand"),
            (humanoid::UPPER_LEG[i], "Thigh"),
            (humanoid::LOWER_LEG[i], "Calf"),
            (humanoid::FOOT[i], "Foot"),
            (humanoid::TOE[i], "Toe0"),
        ] {
            bones.push((logic.into(), biped(side, b)));
        }
        hands.push(HandRig {
            hand: biped(side, "Hand"),
            fingers: (0..5)
                .map(|f| {
                    ["", "1", "2"]
                        .iter()
                        .map(|k| biped(side, &format!("Finger{f}{k}")))
                        .collect()
                })
                .collect(),
            curves: vec!["grip".into(), "flex".into(), "fist".into()],
            curl_deg: [72.0, 95.0, 55.0],
            thumb_curl_deg: [10.0, 35.0, 40.0],
            relaxed: 0.12,
        });
    }
    RetargetMap {
        bones,
        between: vec![(
            "Bip01 Spine1".into(),
            "Bip01 Spine".into(),
            "Bip01 Spine2".into(),
            0.5,
        )],
        hands,
    }
}

/// Prepare a Biped-rigged model (see the module docs). `name` names the logic skeleton
/// (`humanoid.<name>`).
pub fn from_biped(name: &str, model: ImportedModel) -> Result<HumanCharacter, String> {
    let ImportedModel {
        skeleton,
        mut mesh,
        materials,
        images,
        ..
    } = model;
    let target = Arc::new(skeleton);
    let map = biped_map();
    for (_, t) in &map.bones {
        if target.index_of(t).is_none() {
            return Err(format!("{name}: no Biped bone {t} (not a Biped rig?)"));
        }
    }
    let at = |bone: &str| target.rest_model[target.bone(bone)].t;

    // Hair cards hanging behind the head become the ponytail.
    let ponytail = find_ponytail(&mesh, &materials, at("Bip01 Head"));
    let logic_joint = |bone: &str| -> Option<Vec3> {
        if let Some(p) = &ponytail
            && let Some(k) = ["ponytail_1", "ponytail_2", "ponytail_3"]
                .iter()
                .position(|b| *b == bone)
        {
            return Some(p.joints[k]);
        }
        map.bones
            .iter()
            .find(|(l, t)| l == bone && t != "Bip01")
            .map(|(_, t)| at(t))
    };
    let logic = humanoid::fit_skeleton(&format!("humanoid.{name}"), logic_joint);

    classify_detail(&mut mesh, &materials, &images);
    add_muscles(&mut mesh, &target);
    let ponytail = ponytail.map(|p| {
        weight_ponytail(&mut mesh, &logic, &p);
        SpringChainDef {
            tip_length: p.tip_length,
            ..humanoid::ponytail()
        }
    });

    // Ankle height above the soles.
    let sole = mesh
        .vertices
        .iter()
        .map(|v| v.position.y)
        .fold(f32::MAX, f32::min);
    let foot_height = logic.rest_model[logic.bone(humanoid::FOOT[0])].t.y - sole;
    let foot_ik = FootIk {
        foot_height,
        ..humanoid::foot_ik()
    };
    Ok(HumanCharacter {
        name: name.into(),
        logic,
        target,
        map,
        mesh,
        materials,
        images,
        foot_ik,
        ponytail,
        clips: Vec::new(),
        locomotion: None,
    })
}

/// Add motion-capture locomotion to a character: clip files rigged like its model, converted
/// onto its fitted humanoid. Idle loops seamlessly; walk and run are re-timed to start on the
/// right foot's strike (so they blend in step with each other) and get `footstep` events.
/// Empty paths keep the humanoid's generated clips.
pub fn add_locomotion(
    c: &mut HumanCharacter,
    idle: &str,
    walk: &str,
    run: &str,
) -> Result<(), String> {
    let retargeter = Retargeter::new(&c.logic, c.target.clone(), &c.map)?;
    let mut motion = Locomotion::default();
    for (role, path) in [("idle", idle), ("walk", walk), ("run", run)] {
        if path.is_empty() {
            continue;
        }
        let file = Path::new(path);
        let bytes = std::fs::read(file).map_err(|e| format!("{path}: {e}"))?;
        let options = ImportOptions {
            in_place_bone: Some("Bip01".into()),
            ..Default::default()
        };
        let (clips, info) = import_clips(&bytes, file.parent(), &c.target, 1.0, &options)
            .map_err(|e| format!("{path}: {e}"))?;
        let (Some(clip), Some(info)) = (clips.first(), info.first()) else {
            continue;
        };
        let name = format!("{}/{role}", c.logic.name);
        let mut clip = retargeter.clip_to_source(&c.logic, clip, &name, &["hips"]);
        clip.looping = true;
        face_forward(&mut clip, &c.logic);
        if role == "idle" {
            clip = smooth_loop(&clip, &c.logic, 0.6);
            motion.idle = name;
        } else {
            let right = strikes(&clip, &c.logic, humanoid::FOOT[1]);
            let start = right.first().copied().unwrap_or(0.0);
            clip = resample(&clip, &c.logic, start, None);
            let d = clip.duration.max(1e-4);
            for (foot, bone) in [(humanoid::FOOT[1], "foot_r"), (humanoid::FOOT[0], "foot_l")] {
                for t in strikes(&clip, &c.logic, foot) {
                    clip = clip.with_event((t / d).clamp(0.0, 0.999), "footstep", Some(bone));
                }
            }
            if role == "walk" {
                // The generated slow walk wouldn't match the captured one.
                motion.slow_walk.clear();
                motion.walk = name;
                motion.walk_speed = info.speed.max(0.3);
            } else {
                motion.run = name;
                motion.run_speed = info.speed.max(motion.walk_speed + 0.5);
                motion.sprint_speed = motion.sprint_speed.max(motion.run_speed + 1.5);
            }
        }
        c.clips.push(clip);
    }
    c.locomotion = Some(motion);
    Ok(())
}

/// Turn a clip about the vertical so the hips face the skeleton's forward on average (mocap
/// actors don't all face the capture volume's forward; some idles are turned half around).
fn face_forward(clip: &mut AnimationClip, skeleton: &Skeleton) {
    let Some(hips) = skeleton.index_of("hips") else {
        return;
    };
    let binding = ClipBinding::new(clip, skeleton);
    let rest = skeleton.rest_model[hips].r;
    let mut model = Vec::new();
    let mut facing = Vec3::ZERO;
    for i in 0..32 {
        let t = clip.duration * i as f32 / 32.0;
        skeleton.to_model(&pose_at(clip, &binding, skeleton, t), &mut model);
        let f = model[hips].r * rest.conjugate() * Vec3::NEG_Z;
        facing += Vec3::new(f.x, 0.0, f.z).normalize_or_zero();
    }
    if facing.length_squared() < 1e-6 {
        return;
    }
    // About the vertical only (facing may be opposite to forward), heading measured from +Z.
    let yaw = std::f32::consts::PI - facing.x.atan2(facing.z);
    let yaw = (yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if yaw.abs() < 1f32.to_radians() {
        return;
    }
    let turn = Quat::from_rotation_y(yaw);
    // Only the top of the moving chain is keyed in model space (its parents rest at the origin).
    for track in clip.tracks.iter_mut().filter(|t| t.bone == "hips") {
        for r in &mut track.rotation.values {
            *r = (turn * *r).normalize();
        }
        for t in &mut track.translation.values {
            *t = turn * *t;
        }
    }
}

/// Sample a looping clip's pose on `skeleton` at playback time `t`.
fn pose_at(clip: &AnimationClip, binding: &ClipBinding, skeleton: &Skeleton, t: f32) -> Vec<Xf> {
    let mut local = skeleton.rest.clone();
    clip.sample(t, binding, &mut local);
    local
}

/// When a foot comes down in a looping clip: the first moment of each stretch where it is
/// within 2 cm of its lowest point.
fn strikes(clip: &AnimationClip, skeleton: &Skeleton, foot: &str) -> Vec<f32> {
    let binding = ClipBinding::new(clip, skeleton);
    let Some(f) = skeleton.index_of(foot) else {
        return Vec::new();
    };
    let steps = ((clip.duration * 120.0).ceil() as usize).max(8);
    let mut model = Vec::new();
    let heights: Vec<f32> = (0..steps)
        .map(|i| {
            let t = clip.duration * i as f32 / steps as f32;
            skeleton.to_model(&pose_at(clip, &binding, skeleton, t), &mut model);
            model[f].t.y
        })
        .collect();
    let low = heights.iter().copied().fold(f32::MAX, f32::min);
    let down: Vec<bool> = heights.iter().map(|h| *h < low + 0.02).collect();
    (0..steps)
        .filter(|&i| down[i] && !down[(i + steps - 1) % steps])
        .map(|i| clip.duration * i as f32 / steps as f32)
        .collect()
}

/// Re-sample a looping clip so it starts `start` seconds in; with `blend`, its last `blend`
/// seconds ease into its first pose so it loops without a pop.
fn resample(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    start: f32,
    blend: Option<f32>,
) -> AnimationClip {
    let binding = ClipBinding::new(clip, skeleton);
    let d = clip.duration.max(1e-4);
    let keys = ((d * 30.0).ceil() as usize).max(2);
    let first = pose_at(clip, &binding, skeleton, start);
    let mut out = AnimationClip {
        tracks: clip
            .tracks
            .iter()
            .map(|t| BoneTrack {
                bone: t.bone.clone(),
                ..Default::default()
            })
            .collect(),
        events: Vec::new(),
        ..clip.clone()
    };
    for k in 0..=keys {
        let t = d * k as f32 / keys as f32;
        let mut pose = pose_at(clip, &binding, skeleton, start + t);
        if let Some(b) = blend {
            let w = ((t - (d - b)) / b.max(1e-4)).clamp(0.0, 1.0);
            let w = w * w * (3.0 - 2.0 * w);
            for (p, f) in pose.iter_mut().zip(&first) {
                *p = p.lerp(f, w);
            }
        }
        for (track, (src, bone)) in out
            .tracks
            .iter_mut()
            .zip(clip.tracks.iter().zip(&binding.bones))
        {
            let Some(b) = bone else { continue };
            let x = pose[*b as usize];
            if !src.rotation.values.is_empty() {
                let r = match track.rotation.values.last() {
                    Some(prev) if prev.dot(x.r) < 0.0 => -x.r,
                    _ => x.r,
                };
                track.rotation.times.push(t);
                track.rotation.values.push(r);
            }
            if !src.translation.values.is_empty() {
                track.translation.times.push(t);
                track.translation.values.push(x.t);
            }
        }
    }
    out
}

/// A looping clip whose end eases into its start over `blend` seconds.
fn smooth_loop(clip: &AnimationClip, skeleton: &Skeleton, blend: f32) -> AnimationClip {
    resample(clip, skeleton, 0.0, Some(blend.min(clip.duration * 0.3)))
}

/// Joint names of each vertex's strongest influence.
fn main_joints(mesh: &SkinnedMesh) -> Vec<&str> {
    mesh.vertices
        .iter()
        .map(|v| {
            let mut best = 0;
            for k in 1..4 {
                if v.weights[k] > v.weights[best] {
                    best = k;
                }
            }
            mesh.joints
                .get(v.joints[best] as usize)
                .map_or("", String::as_str)
        })
        .collect()
}

/// Which surface each vertex is drawn with.
fn vertex_surfaces(mesh: &SkinnedMesh) -> Vec<usize> {
    let mut out = vec![0; mesh.vertices.len()];
    for s in &mesh.submeshes {
        let range = s.first_index as usize..(s.first_index + s.index_count) as usize;
        for &i in &mesh.indices[range] {
            out[i as usize] = s.surface;
        }
    }
    out
}

/// Average sRGB color (0..255) of a small texel neighborhood at `uv`.
fn sample(image: &Image, uv: Vec2) -> Option<[f32; 3]> {
    let (w, h) = (image.width as i64, image.height as i64);
    if w == 0 || h == 0 || image.data.len() < (w * h * 4) as usize {
        return None;
    }
    let x = (uv.x.rem_euclid(1.0) * w as f32) as i64;
    let y = (uv.y.rem_euclid(1.0) * h as f32) as i64;
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let (px, py) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
            let i = ((py * w + px) * 4) as usize;
            for (c, v) in sum.iter_mut().enumerate() {
                *v += image.data[i + c] as f32;
            }
            n += 1.0;
        }
    }
    Some(sum.map(|s| s / n))
}

/// How skin-like an sRGB color is (0..1), by its chroma in YCbCr: works across skin tones
/// and rejects grey, white, black, blue and most saturated clothing.
fn skin_likeness([r, g, b]: [f32; 3]) -> f32 {
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    let band = |v: f32, lo: f32, hi: f32, soft: f32| {
        ((v - lo) / soft).clamp(0.0, 1.0) * ((hi - v) / soft).clamp(0.0, 1.0)
    };
    let warm = if r > g && r > b { 1.0 } else { 0.0 };
    band(cr, 133.0, 182.0, 5.0)
        * band(cb, 78.0, 132.0, 5.0)
        * ((y - 40.0) / 20.0).clamp(0.0, 1.0)
        * warm
}

/// Fill each vertex's detail mask (x cloth, y skin, z veins, w sheen) from its surface's base
/// color texture. Heads and hair cards get skin detail but never cloth folds.
fn classify_detail(mesh: &mut SkinnedMesh, materials: &[ImportedMaterial], images: &[Image]) {
    let surfaces = vertex_surfaces(mesh);
    let joints: Vec<String> = main_joints(mesh).into_iter().map(String::from).collect();
    let veined = ["UpperArm", "Forearm", "Hand", "Calf"];
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let material = materials.get(surfaces[i]);
        let color = material
            .and_then(|m| m.base_color_texture)
            .and_then(|t| images.get(t))
            .and_then(|img| sample(img, v.uv));
        let skin = color.map_or(0.0, skin_likeness);
        let joint = joints[i].as_str();
        let head = joint.contains("Head")
            || joint.contains("Neck") && skin > 0.5
            || material.is_some_and(|m| m.alpha_cutoff.is_some())
            || !joint.starts_with("Bip01")
            || is_face_bone(joint);
        let cloth = if head { 0.0 } else { 1.0 - skin };
        let veins = if veined.iter().any(|b| joint.ends_with(b)) {
            skin
        } else {
            0.0
        };
        v.mask = Vec4::new(cloth, skin, veins, cloth * 0.5);
    }
}

/// Facial bones of a Biped head (jaw, lips, brows, eyes...).
fn is_face_bone(joint: &str) -> bool {
    let tail = joint.trim_start_matches("Bip01 ");
    !tail.contains(' ')
        && !matches!(
            tail,
            "Pelvis" | "Spine" | "Spine1" | "Spine2" | "Spine3" | "Neck" | "Head"
        )
        && tail != "Bip01"
}

/// Where on the mesh a body direction meets the surface: the extreme vertex along `dir` among
/// `allowed` vertices within `radius` of the line through `near` along `dir`.
fn surface_point(
    mesh: &SkinnedMesh,
    allowed: &[bool],
    near: Vec3,
    dir: Vec3,
    radius: f32,
) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for (v, ok) in mesh.vertices.iter().zip(allowed) {
        if !ok {
            continue;
        }
        let d = v.position - near;
        let along = d.dot(dir);
        if (d - dir * along).length() > radius {
            continue;
        }
        if best.is_none_or(|(b, _)| along > b) {
            best = Some((along, v.position));
        }
    }
    best.map(|(_, p)| p)
}

/// Median distance from the segment `a → b` of the vertices around parameter `t` on it.
fn limb_radius(mesh: &SkinnedMesh, allowed: &[bool], a: Vec3, b: Vec3, t: f32) -> f32 {
    let axis = b - a;
    let len2 = axis.length_squared().max(1e-8);
    let mut d: Vec<f32> = mesh
        .vertices
        .iter()
        .zip(allowed)
        .filter(|(_, ok)| **ok)
        .filter_map(|(v, _)| {
            let s = (v.position - a).dot(axis) / len2;
            ((s - t).abs() < 0.12).then(|| (v.position - (a + axis * s)).length())
        })
        .collect();
    if d.is_empty() {
        return 0.04;
    }
    d.sort_by(f32::total_cmp);
    d[d.len() / 2]
}

/// The humanoid's muscles (same names as [`humanoid::muscles`]) on an imported mesh in its
/// bind pose, placed from the Biped joints and sized to the limbs.
fn add_muscles(mesh: &mut SkinnedMesh, target: &Skeleton) {
    let at = |bone: &str| target.rest_model[target.bone(bone)].t;
    let joints = main_joints(mesh);
    let mask = |names: &[String]| -> Vec<bool> {
        joints
            .iter()
            .map(|j| names.iter().any(|n| n == j))
            .collect()
    };
    let front = Vec3::NEG_Z;
    let back = Vec3::Z;
    let mut shapes: Vec<(MuscleShape, Vec<String>)> = Vec::new();
    let shoulder_width = (at(&biped("R", "UpperArm")) - at(&biped("L", "UpperArm"))).length();
    for (side, i) in SIDES {
        let s = if i == 0 { -1.0 } else { 1.0 };
        let n = |m: &str| format!("{m}_{}", if i == 0 { "l" } else { "r" });
        let b = |bone: &str| biped(side, bone);
        let (shoulder, elbow, wrist) = (at(&b("UpperArm")), at(&b("Forearm")), at(&b("Hand")));
        let (hip, knee, ankle) = (at(&b("Thigh")), at(&b("Calf")), at(&b("Foot")));
        // A muscle on a limb segment: `t` along it, pushed toward `toward` by `out` limb
        // radii; `along` and `across` are its half sizes (fraction of length, of radius).
        let mut limb = |name: String,
                        bones: Vec<String>,
                        (a, bb): (Vec3, Vec3),
                        t: f32,
                        toward: Vec3,
                        out: f32,
                        (along, across): (f32, f32),
                        bulge: f32,
                        sway: f32| {
            let allowed = mask(&bones);
            let axis = (bb - a).normalize_or(Vec3::NEG_Y);
            let r = limb_radius(mesh, &allowed, a, bb, t);
            let side_dir = (toward - axis * toward.dot(axis)).normalize_or(front);
            let center = a + (bb - a) * t + side_dir * r * out;
            shapes.push((
                MuscleShape {
                    name,
                    center,
                    radii: Vec3::new(r * across, (bb - a).length() * along, r * across),
                    rotation: Quat::from_rotation_arc(Vec3::Y, axis),
                    bulge,
                    sway: Vec3::new(0.0, -sway, 0.0),
                },
                bones,
            ));
        };
        let arm = vec![b("UpperArm")];
        let fore = vec![b("Forearm")];
        let leg = vec![b("Thigh")];
        let calf = vec![b("Calf")];
        let outward = Vec3::X * s;
        limb(
            n("biceps"),
            arm.clone(),
            (shoulder, elbow),
            0.55,
            front,
            0.55,
            (0.3, 0.95),
            0.016,
            0.004,
        );
        limb(
            n("triceps"),
            arm.clone(),
            (shoulder, elbow),
            0.45,
            back,
            0.5,
            (0.35, 0.95),
            0.01,
            0.006,
        );
        limb(
            n("forearm"),
            fore,
            (elbow, wrist),
            0.3,
            front,
            0.35,
            (0.35, 1.1),
            0.008,
            0.0,
        );
        limb(
            n("deltoid"),
            vec![b("UpperArm"), b("Clavicle")],
            (shoulder, elbow),
            0.08,
            outward + Vec3::Y * 0.5,
            0.5,
            (0.3, 1.3),
            0.01,
            0.0,
        );
        limb(
            n("quad"),
            leg,
            (hip, knee),
            0.5,
            front,
            0.45,
            (0.33, 0.95),
            0.012,
            0.004,
        );
        limb(
            n("calf"),
            calf,
            (knee, ankle),
            0.3,
            back,
            0.45,
            (0.25, 0.95),
            0.011,
            0.003,
        );

        // Torso muscles: found on the surface in front of, above and behind the ribcage.
        let torso = vec![
            "Bip01 Spine1".to_string(),
            "Bip01 Spine2".to_string(),
            b("Clavicle"),
        ];
        let chest = at("Bip01 Spine2");
        let mut torso_muscle = |name: String,
                                near: Vec3,
                                dir: Vec3,
                                radii: Vec3,
                                bulge: f32,
                                sway: f32,
                                bones: &[String]| {
            let allowed = mask(bones);
            if let Some(p) = surface_point(mesh, &allowed, near - dir, dir, 0.03) {
                shapes.push((
                    MuscleShape {
                        name,
                        center: p - dir * radii.z * 0.5,
                        radii,
                        rotation: Quat::IDENTITY,
                        bulge,
                        sway: Vec3::new(0.0, -sway, 0.0),
                    },
                    bones.to_vec(),
                ));
            }
        };
        let w = shoulder_width;
        torso_muscle(
            n("pec"),
            Vec3::new(s * w * 0.24, chest.y + 0.035, chest.z),
            front,
            Vec3::new(w * 0.24, 0.07, 0.06),
            0.01,
            0.004,
            &torso,
        );
        torso_muscle(
            n("lat"),
            Vec3::new(s * w * 0.36, chest.y - 0.08, chest.z),
            back,
            Vec3::new(w * 0.18, 0.12, 0.07),
            0.01,
            0.0,
            &torso,
        );
        let neck = at("Bip01 Neck");
        let trap_at = neck.lerp(shoulder, 0.45);
        torso_muscle(
            n("trap"),
            Vec3::new(trap_at.x, trap_at.y, trap_at.z),
            Vec3::Y,
            Vec3::new(w * 0.2, 0.06, 0.05),
            0.012,
            0.0,
            &["Bip01 Spine2".into(), "Bip01 Neck".into(), b("Clavicle")],
        );
        let pelvis = at("Bip01 Pelvis");
        torso_muscle(
            n("glute"),
            Vec3::new(hip.x * 0.95, pelvis.y - 0.07, pelvis.z),
            back,
            Vec3::new(0.085, 0.09, 0.07),
            0.008,
            0.007,
            &["Bip01 Pelvis".into(), "Bip01 Spine".into(), b("Thigh")],
        );
    }
    let allowed: Vec<Vec<bool>> = shapes.iter().map(|(_, bones)| mask(bones)).collect();
    let index: HashMap<String, usize> = shapes
        .iter()
        .enumerate()
        .map(|(k, (m, _))| (m.name.clone(), k))
        .collect();
    let muscles: Vec<MuscleShape> = shapes.into_iter().map(|(m, _)| m).collect();
    add_muscle_morphs(mesh, &muscles, |v, m| {
        index.get(&m.name).is_some_and(|&k| allowed[k][v])
    });
}

/// Hair cards behind the head, from their tie down to the tip.
struct Ponytail {
    vertices: Vec<usize>,
    /// Joint positions for `ponytail_1..3`.
    joints: [Vec3; 3],
    top: f32,
    bottom: f32,
    tip_length: f32,
}

fn find_ponytail(
    mesh: &SkinnedMesh,
    materials: &[ImportedMaterial],
    head: Vec3,
) -> Option<Ponytail> {
    let surfaces = vertex_surfaces(mesh);
    let cards: Vec<usize> = (0..mesh.vertices.len())
        .filter(|&i| {
            materials
                .get(surfaces[i])
                .is_some_and(|m| m.alpha_cutoff.is_some())
                && mesh.vertices[i].position.z > head.z + 0.02
        })
        .collect();
    if cards.len() < 12 {
        return None;
    }
    let ys = cards.iter().map(|&i| mesh.vertices[i].position.y);
    let top = ys.clone().fold(f32::MIN, f32::max);
    let bottom = ys.fold(f32::MAX, f32::min);
    // Only a tail that hangs well below the back of the head.
    if top - bottom < 0.15 || bottom > head.y {
        return None;
    }
    let point_at = |f: f32| -> Vec3 {
        let y = top - (top - bottom) * f;
        let near: Vec<Vec3> = cards
            .iter()
            .map(|&i| mesh.vertices[i].position)
            .filter(|p| (p.y - y).abs() < 0.03)
            .collect();
        if near.is_empty() {
            return Vec3::new(0.0, y, head.z + 0.08);
        }
        let mean = near.iter().copied().sum::<Vec3>() / near.len() as f32;
        Vec3::new(mean.x, y, mean.z)
    };
    let fractions = [0.12, 0.4, 0.68];
    let joints = fractions.map(point_at);
    Some(Ponytail {
        vertices: cards,
        joints,
        top,
        bottom,
        tip_length: (top - bottom) * (1.0 - fractions[2]),
    })
}

/// Skin the hanging hair cards to the logic skeleton's ponytail bones (blending from the
/// head at the tie down the chain).
fn weight_ponytail(mesh: &mut SkinnedMesh, logic: &Skeleton, tail: &Ponytail) {
    let head_joint = mesh
        .joints
        .iter()
        .position(|j| j == "Bip01 Head")
        .unwrap_or(0) as u16;
    let mut chain = [0u16; 3];
    for (k, name) in ["ponytail_1", "ponytail_2", "ponytail_3"]
        .iter()
        .enumerate()
    {
        let b = logic.bone(name);
        chain[k] = mesh.joints.len() as u16;
        mesh.joints.push((*name).into());
        mesh.inverse_bind
            .push(logic.rest_model[b].to_mat4().inverse());
    }
    let span = (tail.top - tail.bottom).max(1e-4);
    // Fractions down the tail where each joint sits; the head holds the tie.
    let stops = [
        (0.0, head_joint),
        (0.12, chain[0]),
        (0.4, chain[1]),
        (0.68, chain[2]),
    ];
    for &i in &tail.vertices {
        let v = &mut mesh.vertices[i];
        let f = ((tail.top - v.position.y) / span).clamp(0.0, 1.0);
        let k = stops.iter().rposition(|s| s.0 <= f).unwrap_or(0);
        let (joints, weights) = if k + 1 < stops.len() {
            let (a, b) = (stops[k], stops[k + 1]);
            let w = ((f - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
            ([a.1, b.1, 0, 0], [1.0 - w, w, 0.0, 0.0])
        } else {
            ([stops[k].1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])
        };
        v.joints = joints;
        v.weights = weights;
    }
}

// ------------------------------------------------------------------------- loading at runtime

/// Gives a person (an entity with the humanoid rig: `AnimatorSettings`, `AnimationParams`,
/// `MuscleRig`, `FootIk`) an imported body: a `.glb`/`.gltf` rigged with Biped bones. The file
/// is loaded and prepared on a background thread, once per path however many people use it;
/// until it is ready (or if it can't be loaded) the person keeps the body it has.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HumanModel {
    /// File path of the model.
    pub path: String,
    /// Procedural detail on top of the model's textures (cloth folds, veins, flush).
    pub material: SkinMaterial,
    /// Motion-capture clip files rigged like the model (one clip each), replacing the
    /// humanoid's generated idle, walk and run. Empty keeps the generated one.
    pub idle: String,
    pub walk: String,
    pub run: String,
}

impl HumanModel {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn with_material(mut self, material: SkinMaterial) -> Self {
        self.material = material;
        self
    }

    /// Identifies what gets loaded (the model and its clips).
    fn key(&self) -> String {
        format!("{}|{}|{}|{}", self.path, self.idle, self.walk, self.run)
    }
}

/// A loaded model, shared by everyone wearing it.
#[derive(Debug, Clone)]
pub struct LoadedHuman {
    /// Name of its fitted humanoid skeleton in the `AnimationLibrary`.
    pub skeleton: String,
    pub mesh: Handle<SkinnedMesh>,
    pub surfaces: Vec<StandardMaterial>,
    pub retargeter: Arc<Retargeter>,
    pub foot_ik: FootIk,
    pub ponytail: Option<SpringChainDef>,
    /// Its graph in the `AnimationLibrary` when it has its own locomotion clips.
    pub graph: Option<String>,
}

enum Load {
    Pending(Mutex<Receiver<Result<HumanCharacter, String>>>),
    Ready(Arc<LoadedHuman>),
    Failed(String),
}

/// Every [`HumanModel`] requested so far (by its file paths).
#[derive(Resource, Default)]
pub struct HumanModels {
    loads: HashMap<String, Load>,
}

impl HumanModels {
    fn get(&self, key: &str) -> Option<Arc<LoadedHuman>> {
        match self.loads.get(key) {
            Some(Load::Ready(l)) => Some(l.clone()),
            _ => None,
        }
    }

    /// A loaded model, once it is ready.
    pub fn loaded(&self, model: &HumanModel) -> Option<Arc<LoadedHuman>> {
        self.get(&model.key())
    }

    /// Why a model couldn't be loaded.
    pub fn error(&self, model: &HumanModel) -> Option<&str> {
        match self.loads.get(&model.key()) {
            Some(Load::Failed(e)) => Some(e),
            _ => None,
        }
    }

    /// Models still loading.
    pub fn pending(&self) -> usize {
        self.loads
            .values()
            .filter(|l| matches!(l, Load::Pending(_)))
            .count()
    }
}

/// Which [`HumanModel`] an entity's current body came from.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct AppliedHumanModel(pub String);

/// Load and prepare a [`HumanModel`]: the model file and its clips (blocking).
pub fn load_human(model: &HumanModel) -> Result<HumanCharacter, String> {
    let mut c = load_human_file(&model.path)?;
    add_locomotion(&mut c, &model.idle, &model.walk, &model.run)?;
    Ok(c)
}

/// Load and prepare a Biped-rigged model file (blocking).
pub fn load_human_file(path: &str) -> Result<HumanCharacter, String> {
    let file = Path::new(path);
    let bytes = std::fs::read(file).map_err(|e| format!("{path}: {e}"))?;
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("person")
        .to_string();
    let model = import_gltf(
        &bytes,
        file.parent(),
        &ImportOptions {
            name: name.clone(),
            ..Default::default()
        },
    )
    .map_err(|e| format!("{path}: {e}"))?;
    from_biped(&name, model)
}

/// Put a prepared character into the world: textures, mesh, fitted skeleton, retargeter.
fn register(world: &mut World, c: HumanCharacter) -> Result<LoadedHuman, String> {
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
    let retargeter = Arc::new(Retargeter::new(&logic, c.target, &c.map)?);
    let graph = c.locomotion.map(|motion| {
        let mut lib = world.resource_mut::<AnimationLibrary>();
        for clip in c.clips {
            lib.add_clip(clip);
        }
        lib.add_graph(humanoid::graph_with(&logic.name, &motion));
        logic.name.clone()
    });
    Ok(LoadedHuman {
        skeleton: logic.name.clone(),
        mesh,
        surfaces,
        retargeter,
        foot_ik: c.foot_ik,
        ponytail: c.ponytail,
        graph,
    })
}

/// Give one entity its loaded body.
fn wear(world: &mut World, entity: Entity, model: &HumanModel, loaded: &LoadedHuman) {
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
        if let Some(g) = &loaded.graph {
            s.graph = g.clone();
        }
    }
    let renderer = e.get::<SkinnedMeshRenderer>().cloned().unwrap_or_default();
    let muscles = e.get::<MuscleRig>().cloned().unwrap_or_default();
    let mut rig = MuscleRig::new(if muscles.muscles.is_empty() {
        humanoid::muscles()
    } else {
        muscles.muscles
    });
    rig.strength = muscles.strength;
    rig.jiggle = muscles.jiggle;
    rig.boost = muscles.boost;
    rig.enabled = muscles.enabled;
    let feet = e.get::<FootIk>().cloned().unwrap_or_default();
    e.insert((
        SkinnedMeshRenderer {
            mesh: Some(loaded.mesh),
            material: model.material,
            surfaces: loaded.surfaces.clone(),
            ..renderer
        },
        MorphWeights::new(names),
        // A fresh rig: its muscles find their morph targets on the new mesh.
        rig,
        FootIk {
            enabled: feet.enabled,
            weight: feet.weight,
            ..loaded.foot_ik.clone()
        },
        Retarget::new(loaded.retargeter.clone()),
        AppliedHumanModel(model.key()),
    ));
    if !e.contains::<Face>() {
        // Seeded from what the requester set, else from where the person stands: entity ids
        // shift whenever any crate adds a resource or message, so they make poor seeds.
        let seed = e.get::<BodyRequest>().map(|r| r.seed).unwrap_or_else(|| {
            let at = e.get::<Transform>().map_or(DVec3::ZERO, |t| t.translation);
            let cm = |v: f64| (v * 100.0).round() as i64 as u64;
            cm(at.x) ^ cm(at.y).rotate_left(21) ^ cm(at.z).rotate_left(42)
        });
        e.insert(Face::seeded(seed.wrapping_add(1)));
    }
    match &loaded.ponytail {
        Some(p) => {
            e.insert(SpringChains::new(vec![p.clone()]));
        }
        None => {
            e.remove::<SpringChains>();
        }
    }
}

/// Start loads for newly requested files, register finished ones, and dress the people
/// waiting for them. With `block`, waits for every pending load first.
fn update_models(world: &mut World, block: bool) {
    let mut wanted: Vec<(Entity, HumanModel)> = Vec::new();
    let mut q = world.query::<(Entity, &HumanModel, Option<&AppliedHumanModel>)>();
    for (e, model, applied) in q.iter(world) {
        if !model.path.is_empty() && applied.is_none_or(|a| a.0 != model.key()) {
            wanted.push((e, model.clone()));
        }
    }
    if wanted.is_empty() {
        return;
    }
    let mut finished: Vec<(String, Result<HumanCharacter, String>)> = Vec::new();
    {
        let mut models = world.get_resource_or_init::<HumanModels>();
        for (_, m) in &wanted {
            models.loads.entry(m.key()).or_insert_with(|| {
                let (tx, rx) = channel();
                let model = m.clone();
                let spawned = std::thread::Builder::new()
                    .name("human model".into())
                    .spawn(move || {
                        let _ = tx.send(load_human(&model));
                    });
                match spawned {
                    Ok(_) => Load::Pending(Mutex::new(rx)),
                    Err(e) => Load::Failed(format!("could not start loading: {e}")),
                }
            });
        }
        for (path, load) in models.loads.iter_mut() {
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
                    Ok(r) => finished.push((path.clone(), r)),
                    Err(e) => *load = Load::Failed(format!("loader stopped: {e}")),
                }
            }
        }
    }
    for (path, result) in finished {
        let load = match result.and_then(|c| register(world, c)) {
            Ok(l) => {
                tracing::info!("human model {path} loaded");
                Load::Ready(Arc::new(l))
            }
            Err(e) => {
                tracing::warn!("human model {path}: {e}");
                Load::Failed(e)
            }
        };
        world.resource_mut::<HumanModels>().loads.insert(path, load);
    }
    for (entity, model) in wanted {
        let loaded = world.resource::<HumanModels>().get(&model.key());
        if let Some(loaded) = loaded {
            wear(world, entity, &model, &loaded);
        }
    }
}

pub(crate) fn load_human_models(world: &mut World) {
    update_models(world, false);
}

/// Wait until every requested [`HumanModel`] (and [`HorseModel`](crate::HorseModel)) is
/// loaded and worn (tests, offline renders).
pub fn finish_loading(world: &mut World) {
    update_models(world, true);
    crate::horse_model::finish_loading(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_tones_are_skin_and_clothes_are_not() {
        for skin in [
            [233.0, 192.0, 160.0],
            [198.0, 134.0, 100.0],
            [141.0, 85.0, 61.0],
            [92.0, 56.0, 40.0],
        ] {
            assert!(skin_likeness(skin) > 0.5, "{skin:?}");
        }
        for cloth in [
            [128.0, 128.0, 128.0],
            [240.0, 240.0, 245.0],
            [20.0, 20.0, 25.0],
            [40.0, 70.0, 150.0],
            [60.0, 90.0, 140.0],
        ] {
            assert!(skin_likeness(cloth) < 0.1, "{cloth:?}");
        }
    }

    #[test]
    fn biped_map_names_every_humanoid_limb() {
        let map = biped_map();
        let logic = humanoid::skeleton();
        for (l, _) in &map.bones {
            assert!(logic.index_of(l).is_some(), "{l}");
        }
        assert_eq!(map.hands.len(), 2);
        assert_eq!(map.hands[0].fingers.len(), 5);
        assert!(map.bones.iter().any(|(_, t)| t == "Bip01 R Toe0"));
    }
}
