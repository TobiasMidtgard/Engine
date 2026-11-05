//! How each species moves in the wind and carries rain and snow (F5 of
//! engine-design/fields-and-forests-look.md; design in foliage-wind-research.md §3.2-3.5).
//!
//! [`PlantWind`] holds a species' answers: how fast its trunk sways (0.2-0.3 Hz for forest
//! trees, faster for small ones), how much its sway is damped, how stiff it is, from which wind
//! speed its leaves flutter, its branches sway and its crown bends (Horizon Zero Dawn's
//! response ramps, keyed to the Beaufort scale's land descriptions), how far its sprays twist
//! to show their pale undersides, and how much snow it holds. Rendering's shared `plant_wind`
//! reads them per species slot; the functions here are the same motions on the CPU, for the
//! tests and for anything the CPU must agree with (bounds, drips, shedding snow).

use serde::{Deserialize, Serialize};

use crate::species::{PlantForm, PlantSpecies};

/// A species' wind and weather response.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantWind {
    /// Trunk's natural sway frequency, Hz, at its mature height in leaf.
    pub f0_hz: f32,
    /// Damping ratio in leaf (Jackson et al. 2019: ~8.6 % in leaf, ~3.9 % bare).
    pub damping: f32,
    /// 1 for a reference tree; divides every static bend.
    pub stiffness: f32,
    /// Limb and branch sway frequencies as multiples of `f0_hz`.
    pub limb_freq: f32,
    pub branch_freq: f32,
    /// Leaf flutter, Hz (barely depends on wind speed: Tadrist et al. 2018). 0: none (needles).
    pub flutter_hz: f32,
    /// Wind speeds (m/s at 10 m) where leaf flutter, branch sway and crown bending start and
    /// reach full strength.
    pub flutter_ramp: [f32; 2],
    pub branch_ramp: [f32; 2],
    pub crown_ramp: [f32; 2],
    /// Leaf sprays twisting to show their undersides: onset and full wind (m/s) and the most
    /// they turn (degrees); 0 degrees for needles.
    pub flip: [f32; 3],
    /// Albedo multiplier of a leaf's underside (paler: silvery aspen, grey-felted willow).
    pub underside_gain: f32,
    /// Share of the crown's snow capacity it really holds (spruce all of it, birch little).
    pub snow_hold: f32,
    /// How porous the leaves are to rain (waxy leaves shed it: they gloss more than darken).
    pub leaf_porosity: f32,
}

impl Default for PlantWind {
    fn default() -> Self {
        // A middling broadleaf.
        Self {
            f0_hz: 0.35,
            damping: 0.09,
            stiffness: 1.0,
            limb_freq: 1.9,
            branch_freq: 3.4,
            flutter_hz: 3.0,
            flutter_ramp: [2.0, 5.0],
            branch_ramp: [4.5, 9.0],
            crown_ramp: [8.0, 15.0],
            flip: [8.0, 15.0, 110.0],
            underside_gain: 1.3,
            snow_hold: 0.4,
            leaf_porosity: 0.15,
        }
    }
}

/// Wind speed's drag on a crown: about U² in a breeze, about U in a gale as leaves and
/// branches streamline (reconfiguration; Vogel 1989). `u` in m/s.
pub fn drag(u: f32) -> f32 {
    let u = u.max(0.0);
    u * u / (6.0 + u)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl PlantWind {
    /// The values for a library species: from the table of foliage-wind-research.md §3.4 by
    /// name, else from its form and size.
    pub fn for_species(sp: &PlantSpecies) -> Self {
        let base = Self::default();
        let t = |f0: f32,
                 zeta: f32,
                 stiff: f32,
                 flutter: f32,
                 fr: [f32; 2],
                 br: [f32; 2],
                 cr: [f32; 2],
                 flip: [f32; 3],
                 gain: f32,
                 snow: f32| Self {
            f0_hz: f0,
            damping: zeta,
            stiffness: stiff,
            flutter_hz: flutter,
            flutter_ramp: fr,
            branch_ramp: br,
            crown_ramp: cr,
            flip,
            underside_gain: gain,
            snow_hold: snow,
            ..base
        };
        let none = [0.0, 0.0, 0.0];
        let still = [99.0, 99.0];
        match sp.name.as_str() {
            "aspen" => t(
                0.30,
                0.08,
                0.8,
                6.0,
                [0.8, 2.5],
                [4.0, 9.0],
                [8.0, 15.0],
                [5.0, 11.0, 150.0],
                1.5,
                0.3,
            ),
            "birch" => t(
                0.32,
                0.08,
                0.7,
                4.0,
                [1.5, 4.0],
                [3.5, 8.0],
                [7.0, 14.0],
                [7.0, 14.0, 110.0],
                1.3,
                0.3,
            ),
            "mountain birch" => t(
                0.60,
                0.09,
                0.8,
                4.0,
                [1.5, 4.0],
                [4.0, 9.0],
                [8.0, 15.0],
                [8.0, 15.0, 100.0],
                1.3,
                0.35,
            ),
            "goat willow" => t(
                0.60,
                0.09,
                0.8,
                3.0,
                [2.0, 5.0],
                [4.0, 9.0],
                [8.0, 15.0],
                [6.0, 12.0, 140.0],
                1.8,
                0.35,
            ),
            "alder" => t(
                0.35,
                0.09,
                1.0,
                3.0,
                [2.5, 6.0],
                [5.0, 10.0],
                [9.0, 16.0],
                [9.0, 16.0, 100.0],
                1.2,
                0.4,
            ),
            "rowan" => t(
                0.50,
                0.09,
                0.9,
                3.5,
                [2.0, 5.0],
                [4.5, 9.0],
                [8.0, 15.0],
                [8.0, 15.0, 110.0],
                1.4,
                0.4,
            ),
            "oak" => t(
                0.30,
                0.10,
                1.4,
                2.5,
                [3.5, 8.0],
                [6.0, 12.0],
                [11.0, 18.0],
                [10.0, 18.0, 90.0],
                1.3,
                0.35,
            ),
            "beech" => t(
                0.30,
                0.10,
                1.3,
                2.5,
                [3.5, 8.0],
                [6.0, 12.0],
                [11.0, 18.0],
                [10.0, 18.0, 90.0],
                1.25,
                0.35,
            ),
            "hazel" => t(
                0.80,
                0.10,
                0.9,
                3.0,
                [2.5, 6.0],
                [3.5, 8.0],
                [6.0, 12.0],
                [9.0, 16.0, 100.0],
                1.3,
                0.5,
            ),
            "pine" => t(
                0.24,
                0.07,
                1.1,
                0.0,
                still,
                [6.0, 12.0],
                [10.0, 18.0],
                none,
                1.0,
                0.8,
            ),
            "spruce" => t(
                0.28,
                0.07,
                1.6,
                0.0,
                still,
                [8.0, 14.0],
                [13.0, 21.0],
                none,
                1.0,
                1.0,
            ),
            "larch" => t(
                0.26,
                0.07,
                1.0,
                1.5,
                [3.0, 8.0],
                [6.0, 12.0],
                [10.0, 18.0],
                none,
                1.0,
                0.25,
            ),
            "juniper" => t(
                0.90,
                0.10,
                1.8,
                0.0,
                still,
                [9.0, 15.0],
                [14.0, 22.0],
                none,
                1.0,
                0.8,
            ),
            "fern" => t(
                1.5,
                0.12,
                0.5,
                3.0,
                [2.0, 5.0],
                [2.0, 6.0],
                still,
                [8.0, 14.0, 70.0],
                1.2,
                0.2,
            ),
            _ => {
                // By form: small and soft things sway fast and early, conifers late.
                let h = sp.max_height_m.max(0.3);
                let f0 = (0.30 * (17.0 / h).sqrt()).clamp(0.2, 2.0);
                match sp.form {
                    PlantForm::Conifer => t(
                        f0,
                        0.07,
                        1.3,
                        0.0,
                        still,
                        [7.0, 13.0],
                        [11.0, 19.0],
                        none,
                        1.0,
                        0.8,
                    ),
                    PlantForm::Broadleaf => Self { f0_hz: f0, ..base },
                    _ => Self {
                        f0_hz: f0,
                        stiffness: 0.6,
                        flutter_ramp: [1.5, 4.0],
                        branch_ramp: [2.5, 7.0],
                        ..base
                    },
                }
            }
        }
    }

    /// The trunk's sway frequency for a plant `h` meters tall of a species that grows to
    /// `mature_h`: f ∝ DBH / H² with DBH ∝ H^1.5 gives f ∝ H^-0.5 (Moore & Maguire 2004), so a
    /// sapling a quarter grown sways twice as fast. Bare trees sway 19 % faster (Jackson).
    pub fn frequency_hz(&self, mature_h: f32, h: f32, leafless: bool) -> f32 {
        let f = self.f0_hz * (mature_h.max(0.1) / h.max(0.05)).sqrt();
        if leafless { f * 1.19 } else { f }
    }

    /// The damping ratio, in leaf or bare.
    pub fn damping(&self, leafless: bool) -> f32 {
        if leafless {
            self.damping * 0.45
        } else {
            self.damping
        }
    }

    /// How far into each response the wind `u` (m/s at 10 m) is: leaf flutter, branch sway,
    /// crown bending (0 still .. 1 full).
    pub fn responses(&self, u: f32) -> [f32; 3] {
        let r = |ramp: [f32; 2]| smoothstep(ramp[0], ramp[1], u);
        [
            r(self.flutter_ramp),
            r(self.branch_ramp),
            r(self.crown_ramp),
        ]
    }

    /// How far a spray twists (radians) in wind `u` with gust share `gust` (0 between fronts),
    /// before the sign that turns its face away from the wind.
    pub fn flip(&self, u: f32, gust: f32) -> f32 {
        let [on, full, deg] = self.flip;
        deg.to_radians() * smoothstep(on, full, u) * (0.6 + 0.4 * gust.clamp(0.0, 1.0))
    }

    /// The trunk's lean (share of its full lean) at a moment, from the drag it felt `T/4`,
    /// `3T/4` and `5T/4` ago (`T` its sway period): it rises over a quarter period behind a
    /// gust, overshoots by e^(-π ζ) and undershoots by its square before settling, as a damped
    /// spring does, with nothing remembered between frames (foliage-wind-research.md §3.2.1).
    /// `drag_at(age)`: the drag the tree felt `age` seconds ago, read from the gust fronts
    /// upwind of it.
    pub fn trunk_lean(&self, period_s: f32, leafless: bool, drag_at: impl Fn(f32) -> f32) -> f32 {
        let kappa = (-std::f32::consts::PI * self.damping(leafless)).exp();
        let f1 = drag_at(period_s * 0.25);
        let f3 = drag_at(period_s * 0.75);
        let f5 = drag_at(period_s * 1.25);
        (f1 + kappa * (f1 - f3) - kappa * kappa * (f3 - f5)) / self.stiffness.max(0.05)
    }
}

/// Every species of `library` gets its [`PlantWind`].
pub fn fill(library: &mut [PlantSpecies]) {
    for sp in library {
        sp.wind = PlantWind::for_species(sp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::PlantLibrary;

    fn wind(name: &str) -> PlantWind {
        let lib = PlantLibrary::default();
        lib.get(lib.id(name).unwrap()).wind
    }

    #[test]
    fn every_species_has_its_wind_and_aspen_trembles_first() {
        let lib = PlantLibrary::default();
        for sp in &lib.species {
            let w = sp.wind;
            assert!((0.15..=2.5).contains(&w.f0_hz), "{}: {w:?}", sp.name);
            // Needles don't flutter; everything else ramps in.
            assert!(
                w.flutter_hz == 0.0 || w.flutter_ramp[0] < w.flutter_ramp[1],
                "{}",
                sp.name
            );
        }
        // In a light breeze (Beaufort 2, ~2.5 m/s) an aspen's leaves are already all aquiver,
        // a birch's stirring, an oak's still; a spruce doesn't flutter at all.
        let leaves = |n: &str| wind(n).responses(2.5)[0];
        assert!(leaves("aspen") > 0.95);
        assert!(leaves("birch") > 0.2 && leaves("birch") < leaves("aspen"));
        assert_eq!(leaves("oak"), 0.0);
        assert_eq!(leaves("spruce"), 0.0);
        // A spruce's crown "barely moves until a gale": still bending less than an oak's at
        // Beaufort 7 (15 m/s).
        assert!(wind("spruce").responses(15.0)[2] < wind("oak").responses(15.0)[2]);
        // Willow shows the palest undersides.
        assert!(wind("goat willow").underside_gain > wind("oak").underside_gain);
    }

    #[test]
    fn small_trees_sway_faster() {
        let oak = wind("oak");
        let grown = oak.frequency_hz(17.0, 17.0, false);
        assert!((grown - 0.30).abs() < 1e-5);
        assert!((oak.frequency_hz(17.0, 17.0 / 4.0, false) - 2.0 * grown).abs() < 1e-4);
        assert!(oak.frequency_hz(17.0, 17.0, true) > grown * 1.15);
    }

    #[test]
    fn a_trunk_overshoots_a_gust_then_settles() {
        // A steady breeze, then from t = 0 a gust that stays: the lean a quarter period on
        // is the full lean plus the overshoot, half a period on below it, then settled.
        let oak = wind("oak");
        let period = 1.0 / oak.f0_hz;
        let gust_since = |now: f32| {
            move |age: f32| {
                if now - age >= 0.0 {
                    drag(12.0)
                } else {
                    drag(5.0)
                }
            }
        };
        let lean = |now: f32| oak.trunk_lean(period, false, gust_since(now));
        let before = lean(-1.0);
        let full = drag(12.0) / oak.stiffness;
        let peak = lean(period * 0.25 + 0.01);
        let dip = lean(period * 0.75 + 0.01);
        let settled = lean(period * 2.0);
        assert!((before - drag(5.0) / oak.stiffness).abs() < 1e-4);
        assert!(peak > full * 1.2, "{peak} vs {full}");
        assert!(dip < full && dip > before, "{dip}");
        assert!((settled - full).abs() < 1e-4);
    }

    #[test]
    fn drag_streamlines_in_a_gale() {
        // Quadratic in a breeze, near linear in a gale.
        assert!(drag(2.0) / drag(1.0) > 3.4);
        assert!(drag(30.0) / drag(15.0) < 2.4);
    }
}
