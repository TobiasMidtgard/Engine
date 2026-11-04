//! Which trees make each kind of woodland ([`engine_world::StandKind`]), and how a spot picks
//! one from its stand mix.
//!
//! A stand is a community, not a species: a spruce forest has some birch and larch in it, a
//! birch and aspen wood a pine or two. Species within a stand come in groves (aspen spreads
//! by suckers into clones, a beech hall is beech wall to wall), so the pick follows a noise
//! field tens of meters across rather than a coin per tree.

use engine_core::math::noise::fbm2;
use engine_world::{StandKind, StandMix};

use crate::far_woods::FarTree;

/// The canopy trees of a stand and their shares (summing to 1), most common first.
pub(crate) fn canopy(kind: StandKind) -> &'static [(FarTree, f32)] {
    use FarTree::*;
    match kind {
        StandKind::SpruceForest => &[(Spruce, 0.82), (Birch, 0.08), (Larch, 0.06), (Pine, 0.04)],
        StandKind::PineHeath => &[(Pine, 0.84), (Birch, 0.08), (Larch, 0.04), (Spruce, 0.04)],
        StandKind::BirchAspen => &[(Birch, 0.58), (Aspen, 0.32), (Pine, 0.05), (Spruce, 0.05)],
        StandKind::OakHazel => &[(Oak, 0.72), (Birch, 0.12), (Aspen, 0.08), (Alder, 0.08)],
        StandKind::AlderCarr => &[(Alder, 0.8), (Birch, 0.15), (Spruce, 0.05)],
        StandKind::BeechHall => &[(Beech, 0.9), (Oak, 0.06), (Larch, 0.04)],
        StandKind::MountainBirch => &[(MountainBirch, 0.94), (Pine, 0.06)],
    }
}

/// The understory and floor plants a stand brings (for ecology regions grown as that stand):
/// species names from the built-in library.
pub fn floor_species(kind: StandKind) -> &'static [&'static str] {
    match kind {
        StandKind::SpruceForest => &["fern", "bluebell"],
        StandKind::PineHeath => &["juniper", "grass"],
        StandKind::BirchAspen => &["rowan", "fern", "grass", "wildflowers"],
        StandKind::OakHazel => &["hazel", "rowan", "fern", "bluebell", "wildflowers", "grass"],
        StandKind::AlderCarr => &["goat willow", "fern", "grass"],
        StandKind::BeechHall => &["bluebell"],
        StandKind::MountainBirch => &["juniper", "grass"],
    }
}

/// The species names of a stand's canopy (for ecology regions grown as that stand).
pub fn canopy_species(kind: StandKind) -> Vec<&'static str> {
    canopy(kind).iter().map(|(t, _)| t.species()).collect()
}

/// Groves: how far apart the patches of one species within a stand are, meters.
const GROVE_M: f64 = 34.0;

/// The tree at world position `x`, `z` in a wood of mix `mix`. `stand_roll` and `tree_roll`
/// (0..1) are the tree's own random numbers: the first picks the stand from the mix, the
/// second blurs the grove edges a little.
pub(crate) fn pick(mix: &StandMix, x: f64, z: f64, stand_roll: f32, tree_roll: f32) -> FarTree {
    let mix = mix.normalized();
    let kind = {
        let mut left = stand_roll.clamp(0.0, 0.9999);
        let mut chosen = StandKind::BirchAspen;
        for k in StandKind::ALL {
            let w = mix.weight(k);
            if w <= 0.0 {
                continue;
            }
            chosen = k;
            if left < w {
                break;
            }
            left -= w;
        }
        chosen
    };
    let trees = canopy(kind);
    // The commonest tree takes the middle of the noise's range, where it spends the most
    // time, and the rarer ones the two tails, so they come as groves rather than scattered.
    let n = fbm2(x / GROVE_M, z / GROVE_M, 0x5a7d ^ kind as u64, 3, 2.0, 0.5);
    let u = (0.5 + SPREAD * n + 0.15 * (tree_roll - 0.5)).clamp(0.0, 0.9999);
    let (rest, common) = (&trees[1..], trees[0]);
    let low = rest.iter().step_by(2).rev();
    let high = rest.iter().skip(1).step_by(2);
    let mut edge = 0.0;
    for &(t, w) in low.chain(std::iter::once(&common)).chain(high) {
        edge += w;
        if u < edge {
            return t;
        }
    }
    common.0
}

/// How far the grove noise is stretched over the shares: fbm2 spends most of its time
/// within ±0.5, so this keeps the rarer trees' tails near their share.
const SPREAD: f32 = 1.25;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn every_stand_sums_to_one_and_names_real_species() {
        let lib = crate::species::PlantLibrary::default();
        for k in StandKind::ALL {
            let sum: f32 = canopy(k).iter().map(|(_, w)| w).sum();
            assert!((sum - 1.0).abs() < 1e-4, "{k:?}: {sum}");
            for name in canopy_species(k)
                .into_iter()
                .chain(floor_species(k).iter().copied())
            {
                assert!(lib.id(name).is_some(), "{k:?}: {name}");
            }
        }
    }

    fn census(mix: StandMix) -> HashMap<FarTree, f32> {
        let mut out = HashMap::new();
        let n = 120;
        for j in 0..n {
            for i in 0..n {
                let h = |k: i64| engine_core::math::hash01(i, j, k, 9);
                let t = pick(&mix, i as f64 * 6.0, j as f64 * 6.0, h(0), h(1));
                *out.entry(t).or_insert(0.0) += 1.0 / (n * n) as f32;
            }
        }
        out
    }

    #[test]
    fn a_stand_grows_its_own_trees_in_about_their_shares() {
        let beech = census(StandMix::only(StandKind::BeechHall));
        assert!(beech[&FarTree::Beech] > 0.75, "{beech:?}");
        let birch = census(StandMix::only(StandKind::BirchAspen));
        assert!(birch[&FarTree::Birch] > 0.4, "{birch:?}");
        assert!(birch[&FarTree::Aspen] > 0.15, "{birch:?}");
        let mixed = census(StandMix::from_conifer(0.5));
        let conifers: f32 = [FarTree::Spruce, FarTree::Pine, FarTree::Larch]
            .iter()
            .filter_map(|t| mixed.get(t))
            .sum();
        assert!((0.38..0.62).contains(&conifers), "{mixed:?}");
    }

    #[test]
    fn species_grow_in_groves() {
        // Neighbors 6 m apart are mostly the same tree; across the wood they differ.
        let mix = StandMix::only(StandKind::BirchAspen);
        let (mut same, mut pairs) = (0, 0);
        for j in 0..60 {
            for i in 0..60 {
                let h = |x: i64, y: i64, k: i64| engine_core::math::hash01(x, y, k, 4);
                let a = pick(&mix, i as f64 * 6.0, j as f64 * 6.0, h(i, j, 0), h(i, j, 1));
                let b = pick(
                    &mix,
                    (i + 1) as f64 * 6.0,
                    j as f64 * 6.0,
                    h(i + 1, j, 0),
                    h(i + 1, j, 1),
                );
                same += (a == b) as u32;
                pairs += 1;
            }
        }
        let share = same as f32 / pairs as f32;
        // Picking at random from 58/32/5/5 would match 0.44 of the time.
        assert!(share > 0.7, "{share}");
    }
}
