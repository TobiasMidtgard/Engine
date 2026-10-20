//! What a person looks like: the data a character creator edits, a save game stores and an AI
//! agent patches. Everything is plain numbers with physical or 0..1 meanings, so any mix is
//! valid and sliders, presets and random people all use the same fields.

use std::collections::BTreeMap;

use engine_core::prelude::*;

use crate::sliders::SLIDERS;

/// A whole person's look.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// 0 female .. 1 male (blends the body and face shapes).
    pub sex: f32,
    /// Age, years (1..90). Shapes the skull, face and body, and ages the skin.
    pub age: f32,
    /// 0 slight .. 0.5 average .. 1 muscular.
    pub muscle: f32,
    /// 0 thin .. 0.5 average .. 1 heavy.
    pub weight: f32,
    /// 0 short .. 0.5 average .. 1 tall (for the sex and age).
    pub height: f32,
    /// 0 uncommon .. 0.5 regular .. 1 idealized body proportions.
    pub proportions: f32,
    /// Shape of the face and body by ancestry (relative weights, normalized when used).
    pub ancestry: Ancestry,
    /// Face, neck and torso sliders by key (`nose-hump-decr|incr`, `head-oval`), -1..1 (0..1
    /// for one-sided ones). Missing keys are 0. See [`crate::sliders::SLIDERS`].
    pub shape: BTreeMap<String, f32>,
    pub skin: Skin,
    pub eyes: Eyes,
    pub hair: Hair,
    pub brows: Brows,
    pub beard: Beard,
    /// Varies what has no slider (pores, freckle and hair placement), the same way every run.
    pub seed: u64,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            sex: 0.5,
            age: 25.0,
            muscle: 0.5,
            weight: 0.5,
            height: 0.5,
            proportions: 0.5,
            ancestry: Ancestry::default(),
            shape: BTreeMap::new(),
            skin: Skin::default(),
            eyes: Eyes::default(),
            hair: Hair::default(),
            brows: Brows::default(),
            beard: Beard::default(),
            seed: 0,
        }
    }
}

/// Relative weights of MakeHuman's three ancestry shape sets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ancestry {
    pub african: f32,
    pub asian: f32,
    pub european: f32,
}

impl Default for Ancestry {
    fn default() -> Self {
        Self {
            african: 1.0 / 3.0,
            asian: 1.0 / 3.0,
            european: 1.0 / 3.0,
        }
    }
}

impl Ancestry {
    /// The weights scaled to sum to 1 (equal thirds when all are 0 or negative).
    pub fn normalized(&self) -> [f32; 3] {
        let w = [self.african, self.asian, self.european].map(|v| v.max(0.0));
        let sum: f32 = w.iter().sum();
        if sum <= 1e-6 {
            [1.0 / 3.0; 3]
        } else {
            w.map(|v| v / sum)
        }
    }
}

/// Skin, from the two pigments that color real skin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skin {
    /// Melanin, 0 very fair .. 1 very dark.
    pub melanin: f32,
    /// Blood under the skin, 0 pale .. 1 ruddy (cheeks, nose, ears and lips flush more).
    pub redness: f32,
    /// -1 cool (pink) .. 1 warm (olive, golden).
    pub undertone: f32,
    /// Freckles, 0 none .. 1 many (more on the nose and cheeks).
    pub freckles: f32,
    /// Moles and small marks, 0 none .. 1 many.
    pub moles: f32,
    /// Shine of the skin, 0 matte .. 1 oily (most on the forehead, nose and chin).
    pub oiliness: f32,
    /// Wrinkles beyond what the age gives, -1 smooth .. 1 deeply lined.
    pub wrinkles: f32,
    /// Size of the pores, 0 fine .. 1 coarse.
    pub pores: f32,
}

impl Default for Skin {
    fn default() -> Self {
        Self {
            melanin: 0.25,
            redness: 0.4,
            undertone: 0.1,
            freckles: 0.0,
            moles: 0.2,
            oiliness: 0.35,
            wrinkles: 0.0,
            pores: 0.45,
        }
    }
}

/// Eyes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Eyes {
    /// Iris melanin, 0 blue .. 0.35 green/hazel .. 1 dark brown.
    pub melanin: f32,
    /// Extra yellow-green in the iris (hazel, green), 0..1.
    pub lipochrome: f32,
    /// Darkness of the ring around the iris, 0..1.
    pub limbal_ring: f32,
    /// Pupil opening, 0 small .. 1 wide.
    pub pupil: f32,
    /// Redness of the whites, 0..1.
    pub redness: f32,
}

impl Default for Eyes {
    fn default() -> Self {
        Self {
            melanin: 0.6,
            lipochrome: 0.2,
            limbal_ring: 0.6,
            pupil: 0.4,
            redness: 0.15,
        }
    }
}

/// Hair color from its pigments (as in real hair: eumelanin browns and blacks, pheomelanin
/// reds and blondes), plus grey.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HairColor {
    /// Eumelanin, 0 platinum .. 0.3 blonde .. 0.6 brown .. 1 black.
    pub melanin: f32,
    /// Share of red pheomelanin, 0 ash .. 1 auburn/ginger.
    pub red: f32,
    /// Grey or white hairs mixed in, 0..1.
    pub grey: f32,
}

impl Default for HairColor {
    fn default() -> Self {
        Self {
            melanin: 0.65,
            red: 0.2,
            grey: 0.0,
        }
    }
}

/// Scalp hair styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HairStyle {
    /// No hair.
    Bald,
    /// Clipped to a few millimeters.
    Buzz,
    /// A short crop, a few centimeters, combed back and to the side.
    #[default]
    Short,
    /// Chin to shoulder length.
    Medium,
    /// Below the shoulders.
    Long,
    /// Long hair gathered into a bun at the back.
    Bun,
}

impl HairStyle {
    /// Every style, in creator order.
    pub const ALL: [HairStyle; 6] = [
        HairStyle::Bald,
        HairStyle::Buzz,
        HairStyle::Short,
        HairStyle::Medium,
        HairStyle::Long,
        HairStyle::Bun,
    ];

    /// Lowercase name, as the API takes it.
    pub fn name(self) -> &'static str {
        match self {
            HairStyle::Bald => "bald",
            HairStyle::Buzz => "buzz",
            HairStyle::Short => "short",
            HairStyle::Medium => "medium",
            HairStyle::Long => "long",
            HairStyle::Bun => "bun",
        }
    }
}

/// Scalp hair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hair {
    pub style: HairStyle,
    pub color: HairColor,
    /// Length multiplier for the style, 0.5..1.5.
    pub length: f32,
    /// 0 straight .. 1 curly.
    pub curl: f32,
    /// Hairline height, 0 full .. 1 receded (temples back, thinner crown).
    pub recession: f32,
}

impl Default for Hair {
    fn default() -> Self {
        Self {
            style: HairStyle::Short,
            color: HairColor::default(),
            length: 1.0,
            curl: 0.15,
            recession: 0.0,
        }
    }
}

/// Eyebrows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brows {
    /// 0 sparse .. 1 thick.
    pub thickness: f32,
    /// Color; `None` takes the scalp hair's.
    pub color: Option<HairColor>,
}

impl Default for Brows {
    fn default() -> Self {
        Self {
            thickness: 0.5,
            color: None,
        }
    }
}

/// Facial hair styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BeardStyle {
    #[default]
    None,
    /// A day or two of growth.
    Stubble,
    /// A short, trimmed beard.
    Short,
    /// A full beard.
    Full,
    /// Moustache and chin, cheeks shaved.
    Goatee,
}

impl BeardStyle {
    /// Every style, in creator order.
    pub const ALL: [BeardStyle; 5] = [
        BeardStyle::None,
        BeardStyle::Stubble,
        BeardStyle::Short,
        BeardStyle::Full,
        BeardStyle::Goatee,
    ];

    /// Lowercase name, as the API takes it.
    pub fn name(self) -> &'static str {
        match self {
            BeardStyle::None => "none",
            BeardStyle::Stubble => "stubble",
            BeardStyle::Short => "short",
            BeardStyle::Full => "full",
            BeardStyle::Goatee => "goatee",
        }
    }
}

/// Facial hair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beard {
    pub style: BeardStyle,
    /// 0 patchy .. 1 dense.
    pub density: f32,
    /// Color; `None` takes the scalp hair's.
    pub color: Option<HairColor>,
}

impl Default for Beard {
    fn default() -> Self {
        Self {
            style: BeardStyle::None,
            density: 0.8,
            color: None,
        }
    }
}

/// A small deterministic random source (SplitMix64) for random people.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in 0..1.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    /// Roughly normal, mean 0, standard deviation 1 (sum of four uniforms).
    pub fn normal(&mut self) -> f32 {
        (self.f32() + self.f32() + self.f32() + self.f32() - 2.0) * 1.732
    }
}

impl Appearance {
    /// A slider's value (0 when unset).
    pub fn slider(&self, key: &str) -> f32 {
        self.shape.get(key).copied().unwrap_or(0.0)
    }

    /// Set a slider, clamped to its range. Returns false for an unknown key.
    pub fn set_slider(&mut self, key: &str, value: f32) -> bool {
        let Some(s) = SLIDERS.iter().find(|s| s.key() == key) else {
            return false;
        };
        let lo = if s.low.is_empty() { 0.0 } else { -1.0 };
        self.shape.insert(key.to_string(), value.clamp(lo, 1.0));
        true
    }

    /// Set a slider on both sides of the face (`eye-height1-decr|incr` sets the `l-` and `r-`
    /// sliders; keys without sides are set as they are). Returns false if nothing matched.
    pub fn set_symmetric(&mut self, key: &str, value: f32) -> bool {
        let both = self.set_slider(&format!("l-{key}"), value);
        let right = self.set_slider(&format!("r-{key}"), value);
        both | right || self.set_slider(key, value)
    }

    /// The scalp hair color.
    pub fn hair_color(&self) -> HairColor {
        self.hair.color
    }

    /// The brows' color (their own, or the hair's darkened slightly as brows usually are).
    pub fn brow_color(&self) -> HairColor {
        self.brows.color.unwrap_or(HairColor {
            melanin: (self.hair.color.melanin + 0.08).min(1.0),
            grey: self.hair.color.grey * 0.6,
            ..self.hair.color
        })
    }

    /// The beard's color.
    pub fn beard_color(&self) -> HairColor {
        self.beard.color.unwrap_or(HairColor {
            red: (self.hair.color.red + 0.1).min(1.0),
            grey: (self.hair.color.grey * 1.2).min(1.0),
            ..self.hair.color
        })
    }

    /// A plausible random person, the same for the same seed: ancestry, sex and age first,
    /// then coloring that goes with them (darker skin, hair and eyes where melanin is common),
    /// then small face differences on every slider so no two look alike.
    pub fn random(seed: u64) -> Self {
        let mut r = Rng::new(seed);
        let mut a = Appearance {
            seed,
            ..Default::default()
        };
        // One main ancestry, sometimes mixed.
        let main = (r.f32() * 3.0) as usize % 3;
        let mut w = [0.05f32; 3];
        w[main] = 1.0;
        if r.f32() < 0.35 {
            w[(main + 1 + (r.f32() * 2.0) as usize) % 3] += r.range(0.2, 0.8);
        }
        a.ancestry = Ancestry {
            african: w[0],
            asian: w[1],
            european: w[2],
        };
        let n = a.ancestry.normalized();
        a.sex = if r.f32() < 0.5 {
            r.range(0.0, 0.1)
        } else {
            r.range(0.9, 1.0)
        };
        a.age = (r.range(18.0, 75.0) + r.normal() * 4.0).clamp(16.0, 88.0);
        a.muscle = (0.5 + r.normal() * 0.15).clamp(0.05, 0.95);
        a.weight = (0.5 + r.normal() * 0.15).clamp(0.05, 0.95);
        a.height = (0.5 + r.normal() * 0.2).clamp(0.0, 1.0);
        // Pigments by ancestry, with spread.
        let mel =
            n[0] * r.range(0.62, 0.95) + n[1] * r.range(0.22, 0.45) + n[2] * r.range(0.04, 0.4);
        a.skin = Skin {
            melanin: mel.clamp(0.0, 1.0),
            redness: r.range(0.25, 0.6),
            undertone: n[1] * 0.6 + n[2] * r.range(-0.4, 0.4) + n[0] * 0.2,
            freckles: if mel < 0.2 && r.f32() < 0.4 {
                r.range(0.2, 0.8)
            } else {
                0.0
            },
            moles: r.range(0.0, 0.5),
            oiliness: r.range(0.15, 0.6),
            wrinkles: r.normal() * 0.25,
            pores: r.range(0.3, 0.7),
        };
        let light = n[2] * (1.0 - mel * 1.5).clamp(0.0, 1.0);
        a.eyes = Eyes {
            melanin: if r.f32() < light {
                r.range(0.0, 0.45)
            } else {
                r.range(0.6, 1.0)
            },
            lipochrome: r.range(0.0, 0.6),
            limbal_ring: r.range(0.4, 0.9),
            pupil: r.range(0.25, 0.55),
            redness: r.range(0.05, 0.3),
        };
        let fair_hair = r.f32() < light * 0.8;
        let grey = ((a.age - 38.0) / 35.0).clamp(0.0, 1.0) * r.range(0.4, 1.0);
        a.hair = Hair {
            style: if a.sex > 0.5 {
                [
                    HairStyle::Buzz,
                    HairStyle::Short,
                    HairStyle::Short,
                    HairStyle::Medium,
                ][(r.f32() * 4.0) as usize % 4]
            } else {
                [
                    HairStyle::Medium,
                    HairStyle::Long,
                    HairStyle::Bun,
                    HairStyle::Short,
                ][(r.f32() * 4.0) as usize % 4]
            },
            color: HairColor {
                melanin: if fair_hair {
                    r.range(0.1, 0.5)
                } else {
                    r.range(0.6, 1.0)
                },
                red: if fair_hair && r.f32() < 0.25 {
                    r.range(0.6, 1.0)
                } else {
                    r.range(0.0, 0.3)
                },
                grey,
            },
            length: r.range(0.8, 1.2),
            curl: n[0] * r.range(0.6, 1.0) + (1.0 - n[0]) * r.range(0.0, 0.4),
            recession: if a.sex > 0.5 {
                (((a.age - 30.0) / 40.0).clamp(0.0, 1.0) * r.range(0.0, 1.0)).min(0.9)
            } else {
                0.0
            },
        };
        a.brows.thickness = r.range(0.3, 0.8) + a.sex * 0.15;
        if a.sex > 0.5 && a.age > 17.0 {
            a.beard = Beard {
                style: BeardStyle::ALL[(r.f32() * 5.0) as usize % 5],
                density: r.range(0.5, 1.0),
                color: None,
            };
        }
        for s in SLIDERS {
            if s.group == "torso" {
                continue;
            }
            let v = r.normal() * 0.22;
            let v = if s.low.is_empty() { v.abs() * 0.6 } else { v };
            a.set_slider(&s.key(), v);
        }
        a
    }

    /// Named starting points for the creator (and the showcase): `(name, appearance)`.
    pub fn presets() -> Vec<(&'static str, Appearance)> {
        let mut out = Vec::new();
        let mut astrid = Appearance {
            sex: 0.0,
            age: 27.0,
            ancestry: Ancestry {
                african: 0.0,
                asian: 0.05,
                european: 1.0,
            },
            skin: Skin {
                melanin: 0.08,
                redness: 0.45,
                undertone: -0.2,
                freckles: 0.35,
                pores: 0.35,
                ..Default::default()
            },
            eyes: Eyes {
                melanin: 0.1,
                lipochrome: 0.15,
                ..Default::default()
            },
            hair: Hair {
                style: HairStyle::Long,
                color: HairColor {
                    melanin: 0.25,
                    red: 0.08,
                    grey: 0.0,
                },
                curl: 0.25,
                ..Default::default()
            },
            seed: 11,
            ..Default::default()
        };
        astrid.set_slider("nose-scale-horiz-decr|incr", -0.2);
        astrid.set_slider("mouth-upperlip-volume-decr|incr", 0.2);
        out.push(("Astrid", astrid));

        let mut kofi = Appearance {
            sex: 1.0,
            age: 34.0,
            muscle: 0.65,
            ancestry: Ancestry {
                african: 1.0,
                asian: 0.0,
                european: 0.05,
            },
            skin: Skin {
                melanin: 0.85,
                redness: 0.35,
                undertone: 0.25,
                oiliness: 0.3,
                ..Default::default()
            },
            eyes: Eyes {
                melanin: 0.95,
                ..Default::default()
            },
            hair: Hair {
                style: HairStyle::Buzz,
                color: HairColor {
                    melanin: 1.0,
                    red: 0.05,
                    grey: 0.0,
                },
                curl: 1.0,
                ..Default::default()
            },
            beard: Beard {
                style: BeardStyle::Short,
                density: 0.85,
                color: None,
            },
            seed: 22,
            ..Default::default()
        };
        kofi.set_slider("chin-width-decr|incr", 0.3);
        out.push(("Kofi", kofi));

        let mut mei = Appearance {
            sex: 0.0,
            age: 23.0,
            weight: 0.4,
            ancestry: Ancestry {
                african: 0.0,
                asian: 1.0,
                european: 0.05,
            },
            skin: Skin {
                melanin: 0.28,
                redness: 0.35,
                undertone: 0.45,
                pores: 0.3,
                ..Default::default()
            },
            eyes: Eyes {
                melanin: 1.0,
                ..Default::default()
            },
            hair: Hair {
                style: HairStyle::Bun,
                color: HairColor {
                    melanin: 0.95,
                    red: 0.05,
                    grey: 0.0,
                },
                curl: 0.0,
                ..Default::default()
            },
            seed: 33,
            ..Default::default()
        };
        mei.set_slider("head-round", 0.3);
        out.push(("Mei", mei));

        let mut tomas = Appearance {
            sex: 1.0,
            age: 46.0,
            weight: 0.6,
            ancestry: Ancestry {
                african: 0.05,
                asian: 0.0,
                european: 1.0,
            },
            skin: Skin {
                melanin: 0.38,
                redness: 0.5,
                undertone: 0.45,
                wrinkles: 0.2,
                pores: 0.6,
                ..Default::default()
            },
            eyes: Eyes {
                melanin: 0.75,
                lipochrome: 0.4,
                ..Default::default()
            },
            hair: Hair {
                style: HairStyle::Short,
                color: HairColor {
                    melanin: 0.85,
                    red: 0.1,
                    grey: 0.3,
                },
                recession: 0.35,
                ..Default::default()
            },
            beard: Beard {
                style: BeardStyle::Full,
                density: 0.9,
                color: None,
            },
            seed: 44,
            ..Default::default()
        };
        tomas.set_slider("nose-hump-decr|incr", 0.45);
        tomas.set_slider("nose-scale-vert-decr|incr", 0.2);
        out.push(("Tomas", tomas));

        let mut ruth = Appearance {
            sex: 0.0,
            age: 72.0,
            weight: 0.55,
            ancestry: Ancestry {
                african: 0.0,
                asian: 0.0,
                european: 1.0,
            },
            skin: Skin {
                melanin: 0.12,
                redness: 0.5,
                undertone: -0.1,
                moles: 0.5,
                wrinkles: 0.3,
                ..Default::default()
            },
            eyes: Eyes {
                melanin: 0.3,
                lipochrome: 0.3,
                ..Default::default()
            },
            hair: Hair {
                style: HairStyle::Short,
                color: HairColor {
                    melanin: 0.4,
                    red: 0.1,
                    grey: 0.95,
                },
                curl: 0.5,
                ..Default::default()
            },
            seed: 55,
            ..Default::default()
        };
        ruth.set_slider("mouth-upperlip-volume-decr|incr", -0.3);
        out.push(("Ruth", ruth));

        let mut arjun = Appearance {
            sex: 1.0,
            age: 29.0,
            weight: 0.45,
            ancestry: Ancestry {
                african: 0.15,
                asian: 0.35,
                european: 0.6,
            },
            skin: Skin {
                melanin: 0.6,
                redness: 0.3,
                undertone: 0.5,
                ..Default::default()
            },
            eyes: Eyes {
                melanin: 0.95,
                ..Default::default()
            },
            hair: Hair {
                style: HairStyle::Medium,
                color: HairColor {
                    melanin: 0.95,
                    red: 0.05,
                    grey: 0.0,
                },
                curl: 0.45,
                ..Default::default()
            },
            beard: Beard {
                style: BeardStyle::Stubble,
                density: 0.9,
                color: None,
            },
            seed: 66,
            ..Default::default()
        };
        arjun.set_slider("nose-point-width-decr|incr", 0.2);
        out.push(("Arjun", arjun));
        out
    }

    /// The preset with this name (any case).
    pub fn preset(name: &str) -> Option<Appearance> {
        Self::presets()
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, a)| a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearances_round_trip_and_fill_in_from_partial_json() {
        let a = Appearance::random(7);
        let back: Appearance = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(a, back);
        let partial: Appearance =
            serde_json::from_str(r#"{"sex": 1.0, "hair": {"style": "Long"}}"#).unwrap();
        assert_eq!(partial.sex, 1.0);
        assert_eq!(partial.hair.style, HairStyle::Long);
        assert_eq!(partial.age, 25.0);
    }

    #[test]
    fn random_people_are_repeatable_and_varied() {
        assert_eq!(Appearance::random(3), Appearance::random(3));
        let people: Vec<_> = (0..40).map(Appearance::random).collect();
        let men = people.iter().filter(|a| a.sex > 0.5).count();
        assert!((8..=32).contains(&men), "{men} men of 40");
        let dark = people.iter().filter(|a| a.skin.melanin > 0.6).count();
        let fair = people.iter().filter(|a| a.skin.melanin < 0.3).count();
        assert!(dark > 3 && fair > 3, "dark {dark}, fair {fair}");
        assert!(people.iter().all(|a| (16.0..=88.0).contains(&a.age)));
    }

    #[test]
    fn sliders_clamp_and_set_both_sides() {
        let mut a = Appearance::default();
        assert!(a.set_slider("nose-hump-decr|incr", 3.0));
        assert_eq!(a.slider("nose-hump-decr|incr"), 1.0);
        assert!(a.set_slider("head-oval", -1.0));
        assert_eq!(a.slider("head-oval"), 0.0);
        assert!(!a.set_slider("nose-wings", 0.5));
        assert!(a.set_symmetric("eye-height1-decr|incr", 0.4));
        assert_eq!(a.slider("l-eye-height1-decr|incr"), 0.4);
        assert_eq!(a.slider("r-eye-height1-decr|incr"), 0.4);
    }

    #[test]
    fn presets_are_found_by_name() {
        assert!(Appearance::presets().len() >= 5);
        assert_eq!(Appearance::preset("kofi").unwrap().sex, 1.0);
        assert!(Appearance::preset("nobody").is_none());
    }
}
