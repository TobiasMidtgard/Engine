//! Wounds and severed limbs (`engine_world::combat::BodyInjuries`, written by the combat
//! module).
//!
//! A severed zone leaves the mesh at its joint: the neck takes the head, an upper arm goes at
//! the shoulder, a forearm at the elbow, a hand at the wrist, a thigh at the hip, a knee at the
//! knee and a shin at the ankle. Every vertex the cut-off bones move most goes (skin, clothes,
//! shoes, a hood), what stays loses those bones' pull to the nearest bone that stays, and each
//! hole is closed by a stump: a fan of flesh round a bone in the middle (`MouthPart::Stump`).
//! Shells round the legs (a skirt, a coat, a tabard) stay and hang from the hips. The cut is
//! part of the build ([`crate::build::BuildOptions::severed`]); wounds and bleeding are drawn
//! live per bone from the pose buffer ([`bone_zones`]).

use std::collections::HashMap;

use engine_core::math::Vec3;
use engine_world::animation::HUMANOID_BONES;
use engine_world::combat::{BodyInjuries, BodyZone};

use crate::mesh::{MASKS, SkinMesh};
use crate::mouth::MouthPart;

/// The zones that can be cut off, and the joint each goes at (the first bone it takes; the
/// rest of the limb below goes with it).
pub const SEVERABLE: [(BodyZone, &str); 13] = [
    (BodyZone::Neck, "head"),
    (BodyZone::UpperArmL, "upper_arm_l"),
    (BodyZone::UpperArmR, "upper_arm_r"),
    (BodyZone::ForearmL, "lower_arm_l"),
    (BodyZone::ForearmR, "lower_arm_r"),
    (BodyZone::HandL, "hand_l"),
    (BodyZone::HandR, "hand_r"),
    (BodyZone::ThighL, "upper_leg_l"),
    (BodyZone::ThighR, "upper_leg_r"),
    (BodyZone::KneeL, "lower_leg_l"),
    (BodyZone::KneeR, "lower_leg_r"),
    (BodyZone::ShinL, "foot_l"),
    (BodyZone::ShinR, "foot_r"),
];

/// The severed zones as bits (`1 << zone.index()`), only those that can be cut off.
pub fn severed_mask(injuries: &BodyInjuries) -> u16 {
    SEVERABLE
        .iter()
        .filter(|(z, _)| injuries.get(*z).severed)
        .fold(0, |m, (z, _)| m | 1 << z.index())
}

/// The parent of a humanoid bone.
fn parent(bone: &str) -> Option<&'static str> {
    HUMANOID_BONES
        .iter()
        .find(|(b, _)| *b == bone)
        .and_then(|(_, p)| *p)
}

/// Whether `bone` is `root` or hangs below it.
fn below(bone: &str, root: &str) -> bool {
    let mut b = Some(bone);
    while let Some(name) = b {
        if name == root {
            return true;
        }
        b = parent(name);
    }
    false
}

/// Whether a severed set (`mask`) takes this bone.
pub fn cut_off(bone: &str, mask: u16) -> bool {
    SEVERABLE
        .iter()
        .any(|(z, root)| mask & (1 << z.index()) != 0 && below(bone, root))
}

/// Whether a severed set takes the head (no hair, eyes or mouth).
pub fn takes_head(mask: u16) -> bool {
    mask & (1 << BodyZone::Neck.index()) != 0
}

/// Per bone (in `bones` order), the zones whose wounds show on what it moves, as zone
/// indices (255: none).
pub fn bone_zones(bones: &[String]) -> Vec<[u8; 2]> {
    let z = |zone: BodyZone| zone.index() as u8;
    bones
        .iter()
        .map(|b| {
            let side = |l: BodyZone, r: BodyZone| if b.ends_with("_r") { r } else { l };
            let base = b
                .strip_suffix("_l")
                .or_else(|| b.strip_suffix("_r"))
                .unwrap_or(b);
            if crate::skin::finger_of(b).is_some() {
                return [z(side(BodyZone::HandL, BodyZone::HandR)), 255];
            }
            match base {
                "head" | "ponytail_1" | "ponytail_2" | "ponytail_3" => [z(BodyZone::Head), 255],
                "neck" => [z(BodyZone::Neck), 255],
                "chest" | "clavicle" => [z(BodyZone::Chest), 255],
                "spine" => [z(BodyZone::Belly), z(BodyZone::Chest)],
                "hips" => [z(BodyZone::Belly), 255],
                "upper_arm" | "upper_arm_twist_01" | "upper_arm_twist_02" => {
                    [z(side(BodyZone::UpperArmL, BodyZone::UpperArmR)), 255]
                }
                "lower_arm" | "lower_arm_twist_01" | "lower_arm_twist_02" => {
                    [z(side(BodyZone::ForearmL, BodyZone::ForearmR)), 255]
                }
                "hand" => [z(side(BodyZone::HandL, BodyZone::HandR)), 255],
                "upper_leg" | "thigh_twist_01" => [
                    z(side(BodyZone::ThighL, BodyZone::ThighR)),
                    z(side(BodyZone::KneeL, BodyZone::KneeR)),
                ],
                "lower_leg" | "calf_twist_01" => [
                    z(side(BodyZone::ShinL, BodyZone::ShinR)),
                    z(side(BodyZone::KneeL, BodyZone::KneeR)),
                ],
                "foot" | "toe" => [z(side(BodyZone::ShinL, BodyZone::ShinR)), 255],
                _ => [255, 255],
            }
        })
        .collect()
}

/// Per bone, how wounded (x) and bleeding (y) what it moves is, 0..1, from the zones
/// ([`bone_zones`]). The bone a limb was cut from bleeds at its stump.
pub fn bone_injuries(
    zones: &[[u8; 2]],
    bones: &[String],
    injuries: &BodyInjuries,
) -> Vec<[f32; 2]> {
    let mask = severed_mask(injuries);
    zones
        .iter()
        .zip(bones)
        .map(|(zs, bone)| {
            let mut out = [0.0f32; 2];
            for (k, &zi) in zs.iter().enumerate() {
                let Some(z) = injuries.zones.get(zi as usize) else {
                    continue;
                };
                // (The second zone shows weaker: the knee on the thigh and shin.)
                let share = if k == 0 { 1.0 } else { 0.6 };
                out[0] = out[0].max(z.wound * share);
                out[1] = out[1].max(z.bleeding * share);
            }
            // A stump's rim.
            if !cut_off(bone, mask) && bones_cut_from(bone, mask) {
                out = [out[0].max(0.8), out[1].max(0.9)];
            }
            out.map(|v| v.clamp(0.0, 1.0))
        })
        .collect()
}

/// Whether a severed set cuts a limb off at one of this bone's children.
fn bones_cut_from(bone: &str, mask: u16) -> bool {
    SEVERABLE
        .iter()
        .any(|(z, root)| mask & (1 << z.index()) != 0 && parent(root) == Some(bone))
}

/// How far a stump's middle stands out past the cut, as a share of its radius.
const DOME: f32 = 0.25;

/// Cut the severed parts (`mask`) off `skin` and close the holes with stumps. `bones`: the
/// names the skin's bone indices refer to; `anchor`: a pack vertex with no expressions for the
/// stumps' vertices to follow (they move by bones only).
pub fn sever(skin: &mut SkinMesh, bones: &[String], mask: u16, anchor: u32) {
    if mask == 0 || skin.bones.len() != skin.positions.len() {
        return;
    }
    let gone_bone: Vec<bool> = bones.iter().map(|b| cut_off(b, mask)).collect();
    // The bone each cut-off bone hands its pull to: its nearest ancestor that stays.
    let heir: Vec<u8> = bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let mut p = Some(b.as_str());
            while let Some(name) = p {
                if !cut_off(name, mask)
                    && let Some(j) = bones.iter().position(|x| x == name)
                {
                    return j as u8;
                }
                p = parent(name);
            }
            i as u8
        })
        .collect();
    let skin_verts = skin.skin_vertices();
    let part = |i: usize| {
        i.checked_sub(skin_verts)
            .and_then(|e| skin.extra_parts.get(e).copied())
            .unwrap_or(0)
    };
    let shell = |p: u8| {
        [MouthPart::Skirt, MouthPart::Coat, MouthPart::Tabard]
            .iter()
            .any(|s| *s as u8 == p)
    };
    let gone: Vec<bool> = (0..skin.positions.len())
        .map(|i| {
            let (b, w) = (skin.bones[i], skin.weights[i]);
            let top = (0..4).max_by_key(|&k| w[k]).unwrap_or(0);
            !shell(part(i)) && gone_bone.get(b[top] as usize).copied().unwrap_or(false)
        })
        .collect();
    // What stays: the cut-off bones' pull to their heirs.
    for (i, gone) in gone.iter().enumerate() {
        if *gone {
            continue;
        }
        let (mut b, mut w) = (skin.bones[i], skin.weights[i]);
        for k in 0..4 {
            if w[k] > 0 && gone_bone.get(b[k] as usize).copied().unwrap_or(false) {
                let to = heir[b[k] as usize];
                if let Some(j) = (0..4).find(|&j| j != k && b[j] == to && w[j] > 0) {
                    w[j] = w[j].saturating_add(w[k]);
                    w[k] = 0;
                } else {
                    b[k] = to;
                }
            }
        }
        skin.bones[i] = b;
        skin.weights[i] = w;
    }
    // Edges of the kept triangles that were inside the surface before the cut: the holes.
    let edge = |a: u32, b: u32| (a.min(b), a.max(b));
    let mut before: HashMap<(u32, u32), u32> = HashMap::new();
    for t in skin.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            *before.entry(edge(t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    let keep = |indices: &[u32]| -> Vec<u32> {
        indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| t.iter().all(|v| !gone[*v as usize]))
            .flatten()
            .copied()
            .collect()
    };
    skin.indices = keep(&skin.indices);
    skin.coarse_indices = keep(&skin.coarse_indices);
    let mut after: HashMap<(u32, u32), u32> = HashMap::new();
    for t in skin.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            *after.entry(edge(t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    // Directed as the kept triangle runs them, so the stump's winding carries on.
    let mut rim: Vec<(u32, u32)> = Vec::new();
    for t in skin.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let e = edge(a, b);
            if after.get(&e) == Some(&1) && before.get(&e).copied().unwrap_or(0) >= 2 {
                rim.push((a, b));
            }
        }
    }
    if rim.is_empty() {
        return;
    }
    // Loops: rim edges joined at their vertices.
    let mut root: HashMap<u32, u32> = HashMap::new();
    fn find(root: &mut HashMap<u32, u32>, v: u32) -> u32 {
        let p = *root.entry(v).or_insert(v);
        if p == v {
            return v;
        }
        let r = find(root, p);
        root.insert(v, r);
        r
    }
    for &(a, b) in &rim {
        let (ra, rb) = (find(&mut root, a), find(&mut root, b));
        if ra != rb {
            root.insert(ra, rb);
        }
    }
    let mut loops: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for &(a, b) in &rim {
        let r = find(&mut root, a);
        loops.entry(r).or_default().push((a, b));
    }
    // Where each loop's middle is, and how far round it.
    struct Hole {
        edges: Vec<(u32, u32)>,
        center: Vec3,
        radius: f32,
    }
    let mut holes: Vec<Hole> = loops
        .into_values()
        .filter(|e| e.len() >= 3)
        .map(|edges| {
            let n = edges.len() as f32;
            let center = edges
                .iter()
                .map(|(a, _)| skin.positions[*a as usize])
                .sum::<Vec3>()
                / n;
            let radius = edges
                .iter()
                .map(|(a, _)| (skin.positions[*a as usize] - center).length())
                .sum::<f32>()
                / n;
            Hole {
                edges,
                center,
                radius,
            }
        })
        .collect();
    // Clothes over skin leave two loops at one cut: only the outer one is closed (the inner
    // lies under its stump).
    holes.sort_by(|a, b| b.radius.total_cmp(&a.radius));
    let mut closed: Vec<Hole> = Vec::new();
    for h in holes {
        if closed
            .iter()
            .all(|c| (c.center - h.center).length() > c.radius.max(h.radius))
        {
            closed.push(h);
        }
    }
    for h in closed {
        stump(skin, &h.edges, h.center, h.radius, anchor);
    }
}

/// Close one hole with a domed fan of flesh, the bone at its middle. Each rim vertex is
/// copied (the stump shades as flesh with its own normals); the stump's `curvature` carries
/// how far in from the rim a point is (0 at the rim, 1 in the middle).
fn stump(skin: &mut SkinMesh, edges: &[(u32, u32)], center: Vec3, radius: f32, anchor: u32) {
    // Outward: the triangles round the rim face away from the body's inside; the stump's own
    // normal is the rim's winding.
    let mut normal = Vec3::ZERO;
    for &(a, b) in edges {
        let (pa, pb) = (skin.positions[a as usize], skin.positions[b as usize]);
        normal += (pb - center).cross(pa - center);
    }
    let normal = normal.normalize_or(Vec3::Y);
    let mut copy: HashMap<u32, u32> = HashMap::new();
    let push = |skin: &mut SkinMesh, from: u32, p: Vec3, n: Vec3, inward: f32| -> u32 {
        let i = from as usize;
        skin.positions.push(p);
        skin.normals.push(n);
        let r = skin.rest[i];
        skin.rest.push(r);
        skin.masks.extend([0.0; MASKS]);
        skin.curvature.push(inward);
        let (b, w) = (skin.bones[i], skin.weights[i]);
        skin.bones.push(b);
        skin.weights.push(w);
        skin.extra
            .push([(anchor, 1.0), (anchor, 0.0), (anchor, 0.0)]);
        skin.extra_parts.push(MouthPart::Stump as u8);
        skin.positions.len() as u32 - 1
    };
    // The middle follows the rim vertex nearest it in bone weights (any will do: the stump
    // moves with the bone that stays).
    let first = edges[0].0;
    let mid = push(skin, first, center + normal * (radius * DOME), normal, 1.0);
    // Rest position of the middle: the rim's average.
    let rest = edges
        .iter()
        .map(|(a, _)| skin.rest[*a as usize])
        .sum::<Vec3>()
        / edges.len() as f32;
    skin.rest[mid as usize] = rest;
    let mut tris = Vec::with_capacity(edges.len() * 3);
    for &(a, b) in edges {
        let [ca, cb] = [a, b].map(|v| {
            *copy.entry(v).or_insert_with(|| {
                let p = skin.positions[v as usize];
                let out = (p - center).normalize_or(Vec3::X);
                // Rounded at the rim, the skin folding in over the cut.
                let n = (normal + out * 0.6).normalize_or(normal);
                push(skin, v, p, n, 0.0)
            })
        });
        // The kept triangle runs a then b; the stump runs b then a.
        tris.extend([cb, ca, mid]);
    }
    skin.indices.extend(&tris);
    if !skin.coarse_indices.is_empty() {
        skin.coarse_indices.extend(&tris);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_takes_the_limb_below_its_joint() {
        let mut inj = BodyInjuries::default();
        inj.get_mut(BodyZone::ForearmL).severed = true;
        inj.get_mut(BodyZone::KneeR).severed = true;
        // Not every zone can be cut off.
        inj.get_mut(BodyZone::Chest).severed = true;
        let mask = severed_mask(&inj);
        for (bone, off) in [
            ("lower_arm_l", true),
            ("lower_arm_twist_02_l", true),
            ("hand_l", true),
            ("index_03_l", true),
            ("upper_arm_l", false),
            ("lower_arm_r", false),
            ("lower_leg_r", true),
            ("toe_r", true),
            ("upper_leg_r", false),
            ("chest", false),
            ("head", false),
        ] {
            assert_eq!(cut_off(bone, mask), off, "{bone}");
        }
        assert!(!takes_head(mask));
        assert!(bones_cut_from("upper_arm_l", mask) && bones_cut_from("upper_leg_r", mask));
    }

    /// A tube of rings along y, bone 0 above y = 0 and bone 1 below.
    fn tube() -> (SkinMesh, Vec<String>) {
        let mut s = SkinMesh::default();
        let (rings, around) = (6, 12);
        for k in 0..rings {
            let y = 0.25 - k as f32 * 0.1;
            for j in 0..around {
                let a = j as f32 / around as f32 * std::f32::consts::TAU;
                let p = Vec3::new(0.05 * a.cos(), y, 0.05 * a.sin());
                s.positions.push(p);
                s.normals.push(Vec3::new(a.cos(), 0.0, a.sin()));
                s.rest.push(p);
                s.masks.extend([0.0; MASKS]);
                s.curvature.push(0.0);
                s.bones.push([if y > 0.0 { 0 } else { 1 }, 0, 0, 0]);
                s.weights.push([255, 0, 0, 0]);
            }
        }
        for k in 0..rings - 1 {
            for j in 0..around {
                let at = |k: usize, j: usize| (k * around + j % around) as u32;
                let (a, b, c, d) = (at(k, j), at(k, j + 1), at(k + 1, j), at(k + 1, j + 1));
                s.indices.extend([a, b, c, b, d, c]);
            }
        }
        (s, vec!["upper_arm_l".into(), "lower_arm_l".into()])
    }

    #[test]
    fn a_severed_forearm_leaves_a_closed_stump() {
        let (mut s, bones) = tube();
        let mut inj = BodyInjuries::default();
        inj.get_mut(BodyZone::ForearmL).severed = true;
        sever(&mut s, &bones, severed_mask(&inj), 0);
        // Nothing below the cut is drawn but the stump, which caps the tube at its last ring.
        let stump = MouthPart::Stump as u8;
        let sv = s.skin_vertices();
        for &i in &s.indices {
            let i = i as usize;
            let is_stump = i >= sv && s.extra_parts[i - sv] == stump;
            assert!(is_stump || s.positions[i].y > 0.0, "{}", s.positions[i]);
        }
        // The stump's middle stands below the rim (outward, away from the arm).
        let mid = (sv..s.positions.len())
            .find(|i| s.curvature[*i] == 1.0)
            .unwrap();
        assert!(s.positions[mid].y < 0.05, "{}", s.positions[mid]);
        // And the stump faces down, out of the arm.
        let t = s.indices.len() - 3;
        let [a, b, c] = [0, 1, 2].map(|k| s.positions[s.indices[t + k] as usize]);
        assert!((b - a).cross(c - a).y < 0.0);
    }

    #[test]
    fn wounds_show_on_their_zones_bones() {
        let bones: Vec<String> = ["upper_leg_l", "lower_leg_l", "hand_r", "upper_arm_r"]
            .map(String::from)
            .to_vec();
        let zones = bone_zones(&bones);
        let mut inj = BodyInjuries::default();
        inj.get_mut(BodyZone::KneeL).wound = 1.0;
        inj.get_mut(BodyZone::HandR).severed = true;
        let w = bone_injuries(&zones, &bones, &inj);
        assert!(w[0][0] > 0.5 && w[1][0] > 0.5);
        // The arm a hand was cut from bleeds at the wrist (its forearm isn't in this list).
        assert_eq!(w[3], [0.0, 0.0]);
        assert_eq!(w[2], [0.0, 0.0]);
    }
}
