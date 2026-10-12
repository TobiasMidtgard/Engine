//! Skeletons and poses.
//!
//! A [`Skeleton`] is a bone hierarchy with a rest (bind) pose. Bones are ordered so every
//! parent comes before its children, which lets a pose be converted from local to model space
//! in one pass. A *pose* is one [`Xf`] per bone, local to the bone's parent.

use std::sync::Arc;

use engine_core::math::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::xform::Xf;

/// A bone hierarchy with its rest pose. Shared (`Arc`) by every character that uses it.
#[derive(Debug, Clone)]
pub struct Skeleton {
    /// Identifier, e.g. `"humanoid"`.
    pub name: String,
    pub names: Arc<[String]>,
    /// Parent index per bone (`None` for roots). Parents always precede children.
    pub parents: Arc<[Option<u16>]>,
    /// Rest pose, local to each parent.
    pub rest: Vec<Xf>,
    /// Rest pose in model space.
    pub rest_model: Vec<Xf>,
    /// Inverse of each bone's model-space bind matrix (mesh space → bone space).
    pub inverse_bind: Vec<Mat4>,
    children: Vec<Vec<u16>>,
}

/// One bone for [`Skeleton::new`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoneDef {
    pub name: String,
    pub parent: Option<String>,
    /// Rest transform relative to the parent (meters).
    pub rest: Xf,
}

impl BoneDef {
    pub fn new(name: &str, parent: Option<&str>, offset: Vec3) -> Self {
        Self {
            name: name.into(),
            parent: parent.map(Into::into),
            rest: Xf::from_t(offset),
        }
    }
}

impl Skeleton {
    /// Build a skeleton. Bones may be listed in any order; they are sorted parents-first.
    /// The bind pose is the rest pose. Errors name the missing parent or the cycle.
    pub fn new(name: &str, bones: Vec<BoneDef>) -> Result<Self, String> {
        let n = bones.len();
        if n > u16::MAX as usize {
            return Err(format!(
                "skeleton {name} has {n} bones; at most 65535 are supported"
            ));
        }
        // Resolve parents by name.
        let index_of = |s: &str| bones.iter().position(|b| b.name == s);
        let mut parent_idx = Vec::with_capacity(n);
        for b in &bones {
            match &b.parent {
                None => parent_idx.push(None),
                Some(p) => parent_idx.push(Some(index_of(p).ok_or_else(|| {
                    format!(
                        "bone {} names parent {p}, which is not in skeleton {name}",
                        b.name
                    )
                })?)),
            }
        }
        // Topological order (parents first), stable.
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        while order.len() < n {
            let before = order.len();
            for i in 0..n {
                if !placed[i] && parent_idx[i].is_none_or(|p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                return Err(format!("skeleton {name} has a parent cycle"));
            }
        }
        let mut new_index = vec![0usize; n];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new;
        }
        let names: Vec<String> = order.iter().map(|&i| bones[i].name.clone()).collect();
        let parents: Vec<Option<u16>> = order
            .iter()
            .map(|&i| parent_idx[i].map(|p| new_index[p] as u16))
            .collect();
        let rest: Vec<Xf> = order.iter().map(|&i| bones[i].rest).collect();
        Ok(Self::from_parts(name, names, parents, rest, None))
    }

    /// Build from already-ordered parts (parents before children). `inverse_bind` defaults to
    /// the inverse of the rest pose in model space.
    pub fn from_parts(
        name: &str,
        names: Vec<String>,
        parents: Vec<Option<u16>>,
        rest: Vec<Xf>,
        inverse_bind: Option<Vec<Mat4>>,
    ) -> Self {
        let n = names.len();
        let mut rest_model = vec![Xf::IDENTITY; n];
        for i in 0..n {
            rest_model[i] = match parents[i] {
                Some(p) => rest_model[p as usize].mul(&rest[i]),
                None => rest[i],
            };
        }
        let inverse_bind = inverse_bind
            .unwrap_or_else(|| rest_model.iter().map(|x| x.to_mat4().inverse()).collect());
        let mut children = vec![Vec::new(); n];
        for (i, p) in parents.iter().enumerate() {
            if let Some(p) = p {
                children[*p as usize].push(i as u16);
            }
        }
        Self {
            name: name.into(),
            names: names.into(),
            parents: parents.into(),
            rest,
            rest_model,
            inverse_bind,
            children,
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// Index of a bone that must exist (panics with the bone and skeleton names otherwise).
    pub fn bone(&self, name: &str) -> usize {
        self.index_of(name)
            .unwrap_or_else(|| panic!("skeleton {} has no bone {name}", self.name))
    }

    pub fn children(&self, i: usize) -> &[u16] {
        &self.children[i]
    }

    /// Convert a local pose to model space.
    pub fn to_model(&self, local: &[Xf], model: &mut Vec<Xf>) {
        model.clear();
        model.reserve(local.len());
        for (i, l) in local.iter().enumerate() {
            let m = match self.parents[i] {
                Some(p) => model[p as usize].mul(l),
                None => *l,
            };
            model.push(m);
        }
    }

    /// Recompute model transforms of `bone` and all its descendants after its local
    /// transform changed (the rest of `model` must already be up to date).
    pub fn update_model_subtree(&self, local: &[Xf], model: &mut [Xf], bone: usize) {
        let mut stack = vec![bone];
        while let Some(i) = stack.pop() {
            model[i] = match self.parents[i] {
                Some(p) => model[p as usize].mul(&local[i]),
                None => local[i],
            };
            stack.extend(self.children[i].iter().map(|&c| c as usize));
        }
    }

    /// Per-bone weights: 1 for the named bones (and their descendants when
    /// `include_children`), 0 elsewhere. Unknown names are ignored.
    pub fn mask(&self, bones: &[String], include_children: bool) -> Vec<f32> {
        let mut m = vec![0.0; self.len()];
        for name in bones {
            let Some(i) = self.index_of(name) else {
                continue;
            };
            if include_children {
                let mut stack = vec![i];
                while let Some(b) = stack.pop() {
                    m[b] = 1.0;
                    stack.extend(self.children[b].iter().map(|&c| c as usize));
                }
            } else {
                m[i] = 1.0;
            }
        }
        m
    }

    /// Length from a bone's origin to its first child's origin (0 for leaves).
    pub fn bone_length(&self, i: usize) -> f32 {
        self.children[i]
            .first()
            .map(|&c| self.rest[c as usize].t.length())
            .unwrap_or(0.0)
    }
}

/// Blend `b` into `a` per bone: `a = lerp(a, b, w * mask[i])`.
pub fn blend_masked(a: &mut [Xf], b: &[Xf], w: f32, mask: Option<&[f32]>) {
    if w <= 0.0 {
        return;
    }
    for i in 0..a.len().min(b.len()) {
        let wi = w * mask.map_or(1.0, |m| m.get(i).copied().unwrap_or(0.0));
        if wi >= 0.9999 {
            a[i] = b[i];
        } else if wi > 1e-5 {
            a[i] = a[i].lerp(&b[i], wi);
        }
    }
}

/// Apply an additive pose: `a = a ∘ (reference⁻¹ ∘ add)` weighted per bone.
pub fn apply_additive(a: &mut [Xf], add: &[Xf], reference: &[Xf], w: f32, mask: Option<&[f32]>) {
    if w <= 0.0 {
        return;
    }
    for i in 0..a.len().min(add.len()).min(reference.len()) {
        let wi = w * mask.map_or(1.0, |m| m.get(i).copied().unwrap_or(0.0));
        if wi <= 1e-5 {
            continue;
        }
        let dr = reference[i].r.conjugate() * add[i].r;
        let dt = add[i].t - reference[i].t;
        let ds = add[i].s / reference[i].s;
        let full = Xf {
            t: a[i].t + dt,
            r: (a[i].r * dr).normalize(),
            s: a[i].s * ds,
        };
        a[i] = if wi >= 0.9999 {
            full
        } else {
            a[i].lerp(&full, wi)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Quat;

    fn chain() -> Skeleton {
        Skeleton::new(
            "chain",
            vec![
                BoneDef::new("c", Some("b"), Vec3::new(0.0, 1.0, 0.0)),
                BoneDef::new("a", None, Vec3::ZERO),
                BoneDef::new("b", Some("a"), Vec3::new(0.0, 1.0, 0.0)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn bones_are_sorted_parents_first() {
        let s = chain();
        assert_eq!(&*s.names, &["a".to_string(), "b".into(), "c".into()]);
        assert_eq!(&*s.parents, &[None, Some(0), Some(1)]);
        assert!((s.rest_model[2].t.y - 2.0).abs() < 1e-6);
        assert_eq!(s.bone_length(0), 1.0);
    }

    #[test]
    fn errors_are_readable() {
        let e = Skeleton::new("x", vec![BoneDef::new("a", Some("zz"), Vec3::ZERO)]).unwrap_err();
        assert!(e.contains("zz"), "{e}");
    }

    #[test]
    fn model_space_follows_rotations() {
        let s = chain();
        let mut local = s.rest.clone();
        local[1].r = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mut model = Vec::new();
        s.to_model(&local, &mut model);
        // Rotating b by +90° about Z swings c from +Y to −X.
        assert!(
            model[2].t.abs_diff_eq(Vec3::new(-1.0, 1.0, 0.0), 1e-5),
            "{:?}",
            model[2].t
        );
        // Subtree update matches a full recompute.
        local[1].r = Quat::from_rotation_z(-0.5);
        let mut full = Vec::new();
        s.to_model(&local, &mut full);
        s.update_model_subtree(&local, &mut model, 1);
        assert!(model[2].t.abs_diff_eq(full[2].t, 1e-5));
    }

    #[test]
    fn masks_include_descendants() {
        let s = chain();
        assert_eq!(s.mask(&["b".into()], true), vec![0.0, 1.0, 1.0]);
        assert_eq!(s.mask(&["b".into()], false), vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn additive_adds_the_difference() {
        let mut base = vec![Xf::from_tr(Vec3::ZERO, Quat::from_rotation_x(0.2))];
        let reference = vec![Xf::IDENTITY];
        let add = vec![Xf::from_tr(Vec3::Y, Quat::from_rotation_x(0.3))];
        apply_additive(&mut base, &add, &reference, 1.0, None);
        assert!(base[0].r.angle_between(Quat::from_rotation_x(0.5)) < 1e-4);
        assert!(base[0].t.abs_diff_eq(Vec3::Y, 1e-6));
    }
}
