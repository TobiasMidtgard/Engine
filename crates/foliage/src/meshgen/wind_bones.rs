//! A tree's wind skeleton: the bones the shared plant wind (Rendering's `plant_wind`) bends.
//!
//! engine-design/foliage-wind-research.md §3.1. Every limb, branch and leaf spray of a tree
//! becomes a bone with a pivot, an axis, a parent, a phase and a stiffness; each vertex will
//! name its bone in its color alpha ([`pack`]), and the shader turns it about its own pivot and
//! then its parents' (deepest first), so a spray flutters on a branch that sways on a limb that
//! nods on a trunk that leans with the gust. All levels of detail of one tree share its bones,
//! so a switch between them doesn't pop.
//!
//! Levels: 0 the trunk (one bone, bent rather than turned), 1 limbs, 2 branches, 3 leaf sprays.
//! Twigs ride their branch's bone, so a vertex turns about at most three pivots.
//!
//! Parents are found by where a branch starts: the nearest point of the skeleton a level up,
//! which needs nothing from the growth code that made it (and works for every crown builder).

use std::collections::HashMap;

use engine_core::math::Vec3;

/// The most bones a skeleton holds: 14 bits of the packed vertex word. A crown with more leaf
/// sprays gives its sprays their branch's bone instead.
pub const MAX_BONES: usize = (1 << 14) - 1;

/// One bone (§3.1's `WindBone`, before packing for the GPU).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindBone {
    /// Mesh space, meters.
    pub pivot: Vec3,
    /// Pivot to tip, meters.
    pub length: f32,
    /// Unit direction from pivot to tip.
    pub axis: Vec3,
    /// A spray's card facing (its twist turns this away from the wind, showing the underside);
    /// for other bones a unit vector across the axis.
    pub face: Vec3,
    /// The bone it hangs from (none for the trunk).
    pub parent: Option<u16>,
    /// 0 trunk, 1 limb, 2 branch, 3 spray.
    pub level: u8,
    /// Its own place in the sway cycle, 0..1, so neighbors don't move in step.
    pub phase: f32,
    /// 1 for a typical bone of its level; thicker limbs more, thin ones less. Divides the bend.
    pub stiffness: f32,
    /// The share of the tree's leaf cards this bone carries (itself and below): how much of
    /// the crown's drag acts on it.
    pub drag_share: f32,
}

/// A tree's bones, trunk first, every parent before its children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindSkeleton {
    pub bones: Vec<WindBone>,
}

/// A branch as the skeleton sees it: its path from base to tip, its radius at the base and its
/// level in the tree (0 trunk, 1 limb, 2 branch, 3 twig).
pub struct BranchRef<'a> {
    pub path: &'a [Vec3],
    pub base_radius: f32,
    pub level: u8,
}

/// A leaf card as the skeleton sees it.
pub struct CardRef {
    pub base: Vec3,
    pub dir: Vec3,
    pub face: Vec3,
    pub length: f32,
}

/// The skeleton and which bone each branch's and card's vertices ride.
pub struct Built {
    pub skeleton: WindSkeleton,
    pub branch_bones: Vec<u16>,
    pub card_bones: Vec<u16>,
}

/// Points of the skeleton so far, in 1 m cells, with the bone each belongs to and its level.
#[derive(Default)]
struct Grid {
    cells: HashMap<[i32; 3], Vec<(Vec3, u16, u8)>>,
}

impl Grid {
    const CELL: f32 = 1.0;

    fn key(p: Vec3) -> [i32; 3] {
        let c = (p / Self::CELL).floor();
        [c.x as i32, c.y as i32, c.z as i32]
    }

    fn add_path(&mut self, path: &[Vec3], bone: u16, level: u8) {
        for w in path.windows(2) {
            let steps = ((w[1] - w[0]).length() / (Self::CELL * 0.5))
                .ceil()
                .max(1.0) as usize;
            for s in 0..=steps {
                let p = w[0].lerp(w[1], s as f32 / steps as f32);
                self.cells
                    .entry(Self::key(p))
                    .or_default()
                    .push((p, bone, level));
            }
        }
        if path.len() == 1 {
            self.cells
                .entry(Self::key(path[0]))
                .or_default()
                .push((path[0], bone, level));
        }
    }

    /// The bone nearest `p` among those of a level below `below` (searching outward ring by
    /// ring, up to `reach` cells), or `None`.
    fn nearest(&self, p: Vec3, below: u8, reach: i32) -> Option<u16> {
        let k = Self::key(p);
        let mut best: Option<(f32, u16)> = None;
        for r in 0..=reach {
            for x in -r..=r {
                for y in -r..=r {
                    for z in -r..=r {
                        if x.abs().max(y.abs()).max(z.abs()) != r {
                            continue;
                        }
                        let Some(list) = self.cells.get(&[k[0] + x, k[1] + y, k[2] + z]) else {
                            continue;
                        };
                        for &(q, bone, level) in list {
                            if level >= below {
                                continue;
                            }
                            let d = q.distance_squared(p);
                            if best.is_none_or(|(b, _)| d < b) {
                                best = Some((d, bone));
                            }
                        }
                    }
                }
            }
            // A point found within this ring may still lose to one a ring further out only by
            // a cell's diagonal; one more ring settles it.
            if let Some((d, bone)) = best
                && d.sqrt() < r as f32 * Self::CELL
            {
                return Some(bone);
            }
        }
        best.map(|(_, b)| b)
    }
}

fn across(axis: Vec3) -> Vec3 {
    let a = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    axis.cross(a).normalize_or_zero()
}

fn phase(i: usize, seed: u64) -> f32 {
    engine_core::math::hash01(i as i64, 0x77, 0x31, seed)
}

impl WindSkeleton {
    /// Build the skeleton of a tree `height` meters tall from its trunk path, its branches and
    /// its leaf cards (any order; mesh space).
    pub fn build(
        trunk: &[Vec3],
        branches: &[BranchRef<'_>],
        cards: &[CardRef],
        height: f32,
        seed: u64,
    ) -> Built {
        let root = trunk.first().copied().unwrap_or(Vec3::ZERO);
        let top = trunk
            .last()
            .copied()
            .unwrap_or(root + Vec3::Y * height.max(0.1));
        let axis = (top - root).normalize_or(Vec3::Y);
        let mut bones = vec![WindBone {
            pivot: root,
            length: height.max((top - root).length()),
            axis,
            face: across(axis),
            parent: None,
            level: 0,
            phase: phase(0, seed),
            stiffness: 1.0,
            drag_share: 1.0,
        }];
        let mut grid = Grid::default();
        grid.add_path(trunk, 0, 0);
        let mut branch_bones = vec![0u16; branches.len()];
        // Limbs, then branches, then twigs: each finds its parent among those already placed.
        let typical = |level: u8| -> f32 {
            let rs: Vec<f32> = branches
                .iter()
                .filter(|b| b.level == level)
                .map(|b| b.base_radius)
                .collect();
            if rs.is_empty() {
                1.0
            } else {
                (rs.iter().sum::<f32>() / rs.len() as f32).max(1e-3)
            }
        };
        let typical_r = [1.0, typical(1), typical(2)];
        for level in 0..=3u8 {
            for (i, b) in branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.level == level)
            {
                let Some(&base) = b.path.first() else {
                    continue;
                };
                if level == 0 {
                    // A trunk drawn as a branch: it is the trunk bone.
                    grid.add_path(b.path, 0, 0);
                    continue;
                }
                let parent = grid.nearest(base, level.min(3), 6).unwrap_or(0);
                if level >= 3 || bones.len() >= MAX_BONES {
                    // Twigs ride their branch, and so do the leaves along them.
                    branch_bones[i] = parent;
                    let parent_level = bones[parent as usize].level;
                    grid.add_path(b.path, parent, parent_level);
                    continue;
                }
                let tip = *b.path.last().unwrap_or(&base);
                let length = b
                    .path
                    .windows(2)
                    .map(|w| (w[1] - w[0]).length())
                    .sum::<f32>();
                let axis = (tip - base).normalize_or(Vec3::Y);
                let bone = bones.len() as u16;
                bones.push(WindBone {
                    pivot: base,
                    length: length.max(0.05),
                    axis,
                    face: across(axis),
                    parent: Some(parent),
                    level,
                    phase: phase(bones.len(), seed),
                    stiffness: (b.base_radius / typical_r[level as usize]).clamp(0.4, 2.5),
                    drag_share: 0.0,
                });
                branch_bones[i] = bone;
                grid.add_path(b.path, bone, level);
            }
        }
        // Sprays: one bone per card while there's room, else the card rides its branch.
        let room = MAX_BONES.saturating_sub(bones.len());
        let own = cards.len() <= room;
        let mut card_bones = Vec::with_capacity(cards.len());
        for (i, c) in cards.iter().enumerate() {
            let parent = grid.nearest(c.base, 3, 6).unwrap_or(0);
            if !own {
                card_bones.push(parent);
                continue;
            }
            let bone = bones.len() as u16;
            bones.push(WindBone {
                pivot: c.base,
                length: c.length.max(0.02),
                axis: c.dir.normalize_or(Vec3::Y),
                face: c.face.normalize_or(Vec3::Y),
                parent: Some(parent),
                level: 3,
                phase: phase(1_000_000 + i, seed),
                stiffness: 1.0,
                drag_share: 0.0,
            });
            card_bones.push(bone);
        }
        // Leaf area carried: every card adds to its bone and each bone above it.
        let share = 1.0 / cards.len().max(1) as f32;
        for &b in &card_bones {
            let mut at = Some(b);
            while let Some(i) = at {
                let bone = &mut bones[i as usize];
                if bone.level > 0 {
                    bone.drag_share += share;
                }
                at = bone.parent;
            }
        }
        Built {
            skeleton: WindSkeleton { bones },
            branch_bones,
            card_bones,
        }
    }

    /// The bones from `bone` up to the trunk, `bone` first.
    pub fn chain(&self, bone: u16) -> impl Iterator<Item = &WindBone> {
        let mut at = Some(bone);
        std::iter::from_fn(move || {
            let b = self.bones.get(at? as usize)?;
            at = b.parent;
            Some(b)
        })
    }

    /// The furthest any point can move when every bone turns by up to `angle` radians per
    /// level (limb, branch, spray) and the trunk's top leans by `lean_m` meters: how much
    /// culling bounds must grow (§3.2.2).
    pub fn max_offset_m(&self, angle: [f32; 3], lean_m: f32) -> f32 {
        let mut reach = vec![0.0f32; self.bones.len()];
        let mut most = 0.0f32;
        for (i, b) in self.bones.iter().enumerate() {
            let own = match b.level {
                1..=3 => b.length * angle[b.level as usize - 1].min(std::f32::consts::PI),
                _ => 0.0,
            };
            let up = b.parent.map_or(0.0, |p| reach[p as usize]);
            reach[i] = own + up;
            most = most.max(reach[i]);
        }
        most + lean_m
    }
}

/// The vertex word: bone (14 bits), tip weight 0..1 (5 bits), edge 0..1 (4 bits) and whether
/// the vertex is on a leaf card's underside (1 bit), as an integer an f32 holds exactly
/// (below 2^24), for the vertex color's alpha. Rendering's `plant_wind` decodes it.
pub fn pack(bone: u16, tip: f32, edge: f32, underside: bool) -> f32 {
    let bone = (bone as u32).min(MAX_BONES as u32);
    let tip = (tip.clamp(0.0, 1.0) * 31.0).round() as u32;
    let edge = (edge.clamp(0.0, 1.0) * 15.0).round() as u32;
    (bone | tip << 14 | edge << 19 | (underside as u32) << 23) as f32
}

/// [`pack`] undone: bone, tip, edge, underside.
pub fn unpack(word: f32) -> (u16, f32, f32, bool) {
    let w = word.max(0.0) as u32;
    (
        (w & 0x3fff) as u16,
        ((w >> 14) & 31) as f32 / 31.0,
        ((w >> 19) & 15) as f32 / 15.0,
        (w >> 23) & 1 == 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vertex_word_survives_a_float() {
        for (bone, tip, edge, under) in [
            (0u16, 0.0, 0.0, false),
            (16383, 1.0, 1.0, true),
            (1234, 0.5, 0.2, false),
            (77, 0.03, 0.97, true),
        ] {
            let w = pack(bone, tip, edge, under);
            assert!(w < (1u32 << 24) as f32 && w.fract() == 0.0);
            let (b, t, e, u) = unpack(w);
            assert_eq!(b, bone);
            assert!((t - tip).abs() <= 0.5 / 31.0 + 1e-6, "{t} vs {tip}");
            assert!((e - edge).abs() <= 0.5 / 15.0 + 1e-6, "{e} vs {edge}");
            assert_eq!(u, under);
        }
    }

    /// A trunk 10 m tall, two limbs from it, a branch on each limb, a twig on one branch and
    /// a card on each branch's tip.
    fn little_tree() -> Built {
        let trunk = [Vec3::ZERO, Vec3::Y * 5.0, Vec3::Y * 10.0];
        let limb_a = [Vec3::Y * 4.0, Vec3::new(3.0, 6.0, 0.0)];
        let limb_b = [Vec3::Y * 6.0, Vec3::new(-3.0, 8.0, 0.0)];
        let branch_a = [Vec3::new(2.0, 5.4, 0.0), Vec3::new(2.5, 6.5, 1.5)];
        let branch_b = [Vec3::new(-2.0, 7.4, 0.0), Vec3::new(-2.6, 8.8, -1.4)];
        let twig = [Vec3::new(2.4, 6.2, 1.2), Vec3::new(2.6, 6.6, 1.8)];
        let br = |path: &'static [Vec3], r: f32, level: u8| BranchRef {
            path,
            base_radius: r,
            level,
        };
        let paths: [&'static [Vec3]; 5] = [
            Box::leak(Box::new(limb_a)),
            Box::leak(Box::new(limb_b)),
            Box::leak(Box::new(branch_a)),
            Box::leak(Box::new(branch_b)),
            Box::leak(Box::new(twig)),
        ];
        let branches = [
            br(paths[4], 0.01, 3),
            br(paths[2], 0.04, 2),
            br(paths[0], 0.12, 1),
            br(paths[3], 0.04, 2),
            br(paths[1], 0.10, 1),
        ];
        let card = |base: Vec3| CardRef {
            base,
            dir: Vec3::Y,
            face: Vec3::Z,
            length: 0.3,
        };
        let cards = [
            card(Vec3::new(2.5, 6.5, 1.5)),
            card(Vec3::new(-2.6, 8.8, -1.4)),
        ];
        WindSkeleton::build(&trunk, &branches, &cards, 10.0, 7)
    }

    #[test]
    fn every_bone_hangs_from_the_right_one_a_level_up() {
        let t = little_tree();
        let s = &t.skeleton;
        // Trunk, two limbs, two branches, two sprays; the twig rides its branch.
        assert_eq!(s.bones.len(), 7, "{:#?}", s.bones);
        for (i, b) in s.bones.iter().enumerate() {
            match b.parent {
                None => assert_eq!(i, 0),
                Some(p) => {
                    assert!((p as usize) < i, "parents come first");
                    assert!(s.bones[p as usize].level < b.level);
                }
            }
            assert!(s.chain(i as u16).count() <= 4);
        }
        let level = |i: usize| s.bones[t.branch_bones[i] as usize].level;
        // The limbs hang from the trunk, each branch from its own limb.
        assert_eq!((level(2), level(4)), (1, 1));
        let limb_of = |i: usize| s.bones[t.branch_bones[i] as usize].parent.unwrap();
        assert_eq!(limb_of(1), t.branch_bones[2]);
        assert_eq!(limb_of(3), t.branch_bones[4]);
        // The twig rides the first branch's bone.
        assert_eq!(t.branch_bones[0], t.branch_bones[1]);
        // Each card's spray hangs from the branch it grows on.
        assert_eq!(
            s.bones[t.card_bones[0] as usize].parent,
            Some(t.branch_bones[1])
        );
        assert_eq!(
            s.bones[t.card_bones[1] as usize].parent,
            Some(t.branch_bones[3])
        );
        // Each limb carries half the leaves.
        assert!((s.bones[t.branch_bones[2] as usize].drag_share - 0.5).abs() < 1e-5);
        // The thicker limb is the stiffer.
        assert!(
            s.bones[t.branch_bones[2] as usize].stiffness
                > s.bones[t.branch_bones[4] as usize].stiffness
        );
    }

    #[test]
    fn the_furthest_reach_adds_up_the_chain() {
        let t = little_tree();
        let calm = t.skeleton.max_offset_m([0.0; 3], 0.0);
        assert_eq!(calm, 0.0);
        let storm = t.skeleton.max_offset_m([0.1, 0.2, 0.5], 0.8);
        // A limb (~3.6 m x 0.1) + a branch (~2.1 m x 0.2) + a spray (0.3 m x 0.5) + the lean.
        assert!((1.5..2.0).contains(&storm), "{storm}");
    }
}
