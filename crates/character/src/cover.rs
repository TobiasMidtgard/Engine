//! Hide groups (`Appearance::hide`): the body regions a covering layer (armor, a long coat)
//! hides are left out of the mesh, skin and clothes alike, so nothing under the layer pokes
//! through it as the body bends. A vertex belongs to the region of the bone that moves it most;
//! a triangle with any vertex in a hidden region goes.

use crate::mesh::SkinMesh;

/// Every hide group id, head to feet.
pub const REGIONS: [&str; 10] = [
    "head",
    "neck",
    "torso",
    "upper_arms",
    "forearms",
    "hands",
    "hips",
    "thighs",
    "shins",
    "feet",
];

/// The hide group a bone belongs to.
pub fn region_of(bone: &str) -> Option<&'static str> {
    let base = bone
        .strip_suffix("_l")
        .or_else(|| bone.strip_suffix("_r"))
        .unwrap_or(bone);
    if crate::skin::finger_of(bone).is_some() {
        return Some("hands");
    }
    Some(match base {
        "head" => "head",
        "neck" => "neck",
        "spine" | "chest" | "clavicle" => "torso",
        "upper_arm" | "upper_arm_twist_01" | "upper_arm_twist_02" => "upper_arms",
        "lower_arm" | "lower_arm_twist_01" | "lower_arm_twist_02" => "forearms",
        "hand" => "hands",
        "hips" | "root" => "hips",
        "upper_leg" | "thigh_twist_01" => "thighs",
        "lower_leg" | "calf_twist_01" => "shins",
        "foot" | "toe" => "feet",
        _ => return None,
    })
}

/// Leave out what `hide` covers. `bones`: the names the skin's bone indices refer to.
pub fn hide(skin: &mut SkinMesh, bones: &[String], hide: &[String]) {
    if hide.is_empty() || skin.bones.is_empty() {
        return;
    }
    let hidden_bone: Vec<bool> = bones
        .iter()
        .map(|b| region_of(b).is_some_and(|r| hide.iter().any(|h| h == r)))
        .collect();
    let hidden: Vec<bool> = skin
        .bones
        .iter()
        .zip(&skin.weights)
        .map(|(b, w)| {
            let top = (0..4).max_by_key(|&k| w[k]).unwrap_or(0);
            hidden_bone.get(b[top] as usize).copied().unwrap_or(false)
        })
        .collect();
    let keep = |indices: &[u32]| -> Vec<u32> {
        indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| {
                t.iter()
                    .all(|v| !hidden.get(*v as usize).copied().unwrap_or(false))
            })
            .flatten()
            .copied()
            .collect()
    };
    skin.indices = keep(&skin.indices);
    skin.coarse_indices = keep(&skin.coarse_indices);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_posable_bone_has_a_hide_group() {
        for (bone, _) in engine_world::animation::HUMANOID_BONES {
            if bone.starts_with("ponytail") {
                continue;
            }
            let r = region_of(bone);
            assert!(r.is_some_and(|r| REGIONS.contains(&r)), "{bone}: {r:?}");
        }
        assert_eq!(region_of("index_02_r"), Some("hands"));
        assert_eq!(region_of("calf_twist_01_l"), Some("shins"));
    }
}
