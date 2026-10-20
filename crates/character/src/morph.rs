//! Turning an [`Appearance`] into vertex positions: MakeHuman's macro and slider weighting.
//!
//! Every target is weighed by the product of its factors. Macro factors come from the macro
//! values the way MakeHuman computes them: `male`/`female` from the sex; `baby`, `child`,
//! `young`, `old` from the age (1, 11, 25 and 90 years, each blending into the next);
//! `min`/`average`/`max` muscle, weight, height, cup and firmness from their 0..1 values;
//! `african`, `asian`, `caucasian` from the normalized ancestry; `uncommon`/`regular`/`ideal`
//! proportions. Slider factors are the two halves of each slider: `nose-hump-decr` is the
//! slider's value below 0 (as a positive amount), `nose-hump-incr` its value above 0.

use std::collections::HashMap;

use engine_core::math::Vec3;

use crate::appearance::{AncestryExt, Appearance, AppearanceExt};
use crate::pack::HumanPack;
use crate::sliders::SLIDERS;

/// MakeHuman's age scale: 0 → 1 year, 0.1875 → 11, 0.5 → 25, 1 → 90.
pub fn age_value(years: f32) -> f32 {
    let y = years.clamp(1.0, 90.0);
    if y < 25.0 {
        (y - 1.0) / (25.0 - 1.0) * 0.5
    } else {
        0.5 + (y - 25.0) / (90.0 - 25.0) * 0.5
    }
}

/// `min`/`average`/`max` weights of a 0..1 macro value, as MakeHuman's `_set*Vals`.
fn three_way(v: f32) -> [f32; 3] {
    let v = v.clamp(0.0, 1.0);
    let max = (v * 2.0 - 1.0).max(0.0);
    let min = (1.0 - v * 2.0).max(0.0);
    [min, 1.0 - max.max(min), max]
}

/// Every macro factor of an appearance.
pub fn macro_factors(a: &Appearance) -> HashMap<&'static str, f32> {
    let mut f = HashMap::new();
    let sex = a.sex.clamp(0.0, 1.0);
    f.insert("male", sex);
    f.insert("female", 1.0 - sex);
    let age = age_value(a.age);
    let (baby, child, young, old);
    if age < 0.5 {
        old = 0.0;
        baby = (1.0 - age * 5.333).max(0.0);
        young = ((age - 0.1875) * 3.2).max(0.0);
        child = ((5.333 * age).min(1.0) - young).max(0.0);
    } else {
        child = 0.0;
        baby = 0.0;
        old = (age * 2.0 - 1.0).max(0.0);
        young = 1.0 - old;
    }
    f.insert("baby", baby);
    f.insert("child", child);
    f.insert("young", young);
    f.insert("old", old);
    // Muscle and weight: MakeHuman's average is exactly 1 - max - min.
    for (v, names) in [
        (a.muscle, ["minmuscle", "averagemuscle", "maxmuscle"]),
        (a.weight, ["minweight", "averageweight", "maxweight"]),
    ] {
        let v = v.clamp(0.0, 1.0);
        let max = (v * 2.0 - 1.0).max(0.0);
        let min = (1.0 - v * 2.0).max(0.0);
        f.insert(names[0], min);
        f.insert(names[1], 1.0 - max - min);
        f.insert(names[2], max);
    }
    for (v, names) in [
        (a.height, ["minheight", "averageheight", "maxheight"]),
        (0.5, ["mincup", "averagecup", "maxcup"]),
        (0.5, ["minfirmness", "averagefirmness", "maxfirmness"]),
        (
            a.proportions,
            [
                "uncommonproportions",
                "regularproportions",
                "idealproportions",
            ],
        ),
    ] {
        let [min, avg, max] = three_way(v);
        f.insert(names[0], min);
        f.insert(names[1], avg);
        f.insert(names[2], max);
    }
    let [african, asian, european] = a.ancestry.normalized();
    f.insert("african", african);
    f.insert("asian", asian);
    f.insert("caucasian", european);
    f
}

/// Slider factors (`nose-hump-incr` → 0.4, …) of an appearance; unset sliders are absent.
pub fn slider_factors(a: &Appearance) -> HashMap<String, f32> {
    let mut f = HashMap::new();
    for s in SLIDERS {
        let v = a.slider(&s.key());
        if v == 0.0 {
            continue;
        }
        if s.low.is_empty() {
            f.insert(s.target.to_string(), v.max(0.0));
        } else {
            f.insert(format!("{}-{}", s.target, s.low), (-v).max(0.0));
            f.insert(format!("{}-{}", s.target, s.high), v.max(0.0));
        }
    }
    f
}

/// Weight of every target of `pack` for `a`, as `(target index, weight)` for the nonzero ones.
pub fn target_weights(pack: &HumanPack, a: &Appearance) -> Vec<(usize, f32)> {
    let macros = macro_factors(a);
    let sliders = slider_factors(a);
    let mut out = Vec::new();
    for (i, t) in pack.targets.iter().enumerate() {
        let mut w = 1.0f32;
        for f in &t.factors {
            let v = macros
                .get(f.as_str())
                .copied()
                .or_else(|| sliders.get(f).copied())
                .unwrap_or(0.0);
            w *= v;
            if w == 0.0 {
                break;
            }
        }
        if w.abs() > 1e-5 {
            out.push((i, w));
        }
    }
    out
}

/// The base mesh's positions with every target applied for `a`.
pub fn morph(pack: &HumanPack, a: &Appearance) -> Vec<Vec3> {
    let mut p = pack.positions.clone();
    for (i, w) in target_weights(pack, a) {
        let t = &pack.targets[i];
        for (&v, d) in t.indices.iter().zip(&t.deltas) {
            p[v as usize] += *d * w;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::Target;
    use engine_core::math::Vec2;

    #[test]
    fn age_scale_matches_makehuman() {
        assert!((age_value(25.0) - 0.5).abs() < 1e-6);
        assert!((age_value(90.0) - 1.0).abs() < 1e-6);
        assert!((age_value(11.0) - 0.2083).abs() < 1e-3);
        let f = macro_factors(&Appearance {
            age: 25.0,
            ..Default::default()
        });
        assert_eq!(f["young"], 1.0);
        assert_eq!(f["old"], 0.0);
        let f = macro_factors(&Appearance {
            age: 57.5,
            ..Default::default()
        });
        assert!((f["old"] - 0.5).abs() < 1e-4 && (f["young"] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn macro_halves_sum_to_one() {
        let a = Appearance {
            sex: 0.3,
            muscle: 0.8,
            weight: 0.1,
            age: 5.0,
            ..Default::default()
        };
        let f = macro_factors(&a);
        assert!((f["male"] + f["female"] - 1.0).abs() < 1e-6);
        let m = f["minmuscle"] + f["averagemuscle"] + f["maxmuscle"];
        assert!((m - 1.0).abs() < 1e-6);
        let ages = f["baby"] + f["child"] + f["young"] + f["old"];
        assert!((ages - 1.0).abs() < 1e-3, "{ages}");
        let races = f["african"] + f["asian"] + f["caucasian"];
        assert!((races - 1.0).abs() < 1e-6);
    }

    fn pack_with(targets: Vec<Target>) -> HumanPack {
        HumanPack::new(
            vec![Vec3::ZERO; 2],
            vec![Vec2::ZERO],
            vec!["body".into()],
            vec![],
            targets,
        )
    }

    #[test]
    fn sliders_and_macros_weigh_targets() {
        let t = |name: &str, factors: &[&str], d: f32| Target {
            name: name.into(),
            factors: factors.iter().map(|s| s.to_string()).collect(),
            indices: vec![0],
            deltas: vec![Vec3::new(d, 0.0, 0.0)],
        };
        let pack = pack_with(vec![
            t("nose/nose-hump-incr", &["nose-hump-incr"], 1.0),
            t("nose/nose-hump-decr", &["nose-hump-decr"], -1.0),
            t(
                "macrodetails/universal-male-young",
                &["male", "young"],
                10.0,
            ),
        ]);
        let mut a = Appearance {
            sex: 1.0,
            ..Default::default()
        };
        a.set_slider("nose-hump-decr|incr", -0.5);
        let p = morph(&pack, &a);
        // Half of the decr target (-1 → -0.5) plus the whole male-young target.
        assert!((p[0].x - 9.5).abs() < 1e-5, "{}", p[0].x);
        assert_eq!(p[1], Vec3::ZERO);
    }
}
