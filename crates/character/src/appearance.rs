//! What a person looks like: the data a character creator edits, a save game stores and an AI
//! agent patches. Everything is plain numbers with physical or 0..1 meanings, so any mix is
//! valid and sliders, presets and random people all use the same fields.

use engine_core::prelude::*;

use crate::sliders::SLIDERS;

pub use engine_world::character::{
    Ancestry, Appearance, AppearanceRequest, Beard, Brows, Eyes, Hair, HairColor, Skin,
};

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

    /// The style for an id (`Appearance.hair.style`, any case); unknown ids are the default.
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.name().eq_ignore_ascii_case(id))
            .unwrap_or_default()
    }

    /// Lowercase id, as `Appearance.hair.style` and the API take it.
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

    /// The style for an id (`Appearance.beard.style`, any case); unknown ids are the default.
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.name().eq_ignore_ascii_case(id))
            .unwrap_or_default()
    }

    /// Lowercase id, as `Appearance.beard.style` and the API take it.
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

/// Ancestry weights as this crate uses them.
pub trait AncestryExt {
    /// The weights scaled to sum to 1 (equal thirds when all are 0 or negative).
    fn normalized(&self) -> [f32; 3];
}

impl AncestryExt for Ancestry {
    fn normalized(&self) -> [f32; 3] {
        let w = [self.african, self.asian, self.european].map(|v| v.max(0.0));
        let sum: f32 = w.iter().sum();
        if sum <= 1e-6 {
            [1.0 / 3.0; 3]
        } else {
            w.map(|v| v / sum)
        }
    }
}

/// What this crate adds to the shared [`Appearance`]: sliders by key, derived colors and
/// styles, random people and presets. `use engine_character::AppearanceExt` to call them.
pub trait AppearanceExt: Sized {
    /// A slider's value (0 when unset).
    fn slider(&self, key: &str) -> f32;
    /// Set a slider, clamped to its range. Returns false for an unknown key.
    fn set_slider(&mut self, key: &str, value: f32) -> bool;
    /// Set a slider on both sides of the face (`eye-height1-decr|incr` sets the `l-` and `r-`
    /// sliders; keys without sides are set as they are). Returns false if nothing matched.
    fn set_symmetric(&mut self, key: &str, value: f32) -> bool;
    /// The scalp hair color.
    fn hair_color(&self) -> HairColor;
    /// The brows' color (their own, or the hair's darkened slightly as brows usually are).
    fn brow_color(&self) -> HairColor;
    /// The beard's color.
    fn beard_color(&self) -> HairColor;
    /// The scalp hair style (unknown ids are the default).
    fn hair_style(&self) -> HairStyle;
    /// The beard style (unknown ids are none).
    fn beard_style(&self) -> BeardStyle;
    /// A plausible random person, the same for the same seed.
    fn random(seed: u64) -> Self;
    /// A person for a request: the request's own seed, its sex, age range and ancestry where
    /// given, and, with a family seed, ancestry, coloring and face shape drawn toward the
    /// family's so relatives look related but distinct.
    fn from_request(req: &AppearanceRequest) -> Self;
    /// Named starting points for the creator (and the showcase): `(name, appearance)`.
    fn presets() -> Vec<(&'static str, Self)>;
    /// A preset by name (any case).
    fn preset(name: &str) -> Option<Self>;
}

impl AppearanceExt for Appearance {
    fn hair_style(&self) -> HairStyle {
        HairStyle::from_id(&self.hair.style)
    }

    fn beard_style(&self) -> BeardStyle {
        BeardStyle::from_id(&self.beard.style)
    }

    /// A slider's value (0 when unset).
    fn slider(&self, key: &str) -> f32 {
        self.shape.get(key).copied().unwrap_or(0.0)
    }

    /// Set a slider, clamped to its range. Returns false for an unknown key.
    fn set_slider(&mut self, key: &str, value: f32) -> bool {
        let Some(s) = SLIDERS.iter().find(|s| s.key() == key) else {
            return false;
        };
        let lo = if s.low.is_empty() { 0.0 } else { -1.0 };
        self.shape.insert(key.to_string(), value.clamp(lo, 1.0));
        true
    }

    /// Set a slider on both sides of the face (`eye-height1-decr|incr` sets the `l-` and `r-`
    /// sliders; keys without sides are set as they are). Returns false if nothing matched.
    fn set_symmetric(&mut self, key: &str, value: f32) -> bool {
        let both = self.set_slider(&format!("l-{key}"), value);
        let right = self.set_slider(&format!("r-{key}"), value);
        both | right || self.set_slider(key, value)
    }

    /// The scalp hair color.
    fn hair_color(&self) -> HairColor {
        self.hair.color
    }

    /// The brows' color (their own, or the hair's darkened slightly as brows usually are).
    fn brow_color(&self) -> HairColor {
        // Brows are darker than the hair, fair people's most of all (a blonde's are light
        // brown, not yellow).
        self.brows.color.unwrap_or(HairColor {
            melanin: (self.hair.color.melanin + 0.05 + 0.12 * (1.0 - self.hair.color.melanin))
                .min(1.0),
            red: self.hair.color.red * 0.5,
            grey: self.hair.color.grey * 0.6,
        })
    }

    /// The beard's color.
    fn beard_color(&self) -> HairColor {
        self.beard.color.unwrap_or(HairColor {
            red: (self.hair.color.red + 0.1).min(1.0),
            grey: (self.hair.color.grey * 1.2).min(1.0),
            ..self.hair.color
        })
    }

    /// A plausible random person, the same for the same seed: ancestry, sex and age first,
    /// then coloring that goes with them (darker skin, hair and eyes where melanin is common),
    /// then small face differences on every slider so no two look alike.
    fn random(seed: u64) -> Self {
        generate(seed, &AppearanceRequest::default())
    }

    fn from_request(req: &AppearanceRequest) -> Self {
        let mut a = generate(req.seed, req);
        let Some(family) = req.family else {
            return a;
        };
        let f = generate(family, req);
        let toward = |own: f32, fam: f32, k: f32| own + (fam - own) * k;
        a.ancestry = f.ancestry;
        a.skin.melanin = toward(a.skin.melanin, f.skin.melanin, 0.8);
        a.skin.undertone = toward(a.skin.undertone, f.skin.undertone, 0.7);
        a.skin.freckles = toward(a.skin.freckles, f.skin.freckles, 0.6);
        a.eyes.melanin = toward(a.eyes.melanin, f.eyes.melanin, 0.6);
        a.hair.color.melanin = toward(a.hair.color.melanin, f.hair.color.melanin, 0.7);
        a.hair.color.red = toward(a.hair.color.red, f.hair.color.red, 0.7);
        for (key, v) in &f.shape {
            let own = a.slider(key);
            a.shape.insert(key.clone(), toward(own, *v, 0.5));
        }
        a
    }

    /// Named starting points for the creator (and the showcase): `(name, appearance)`.
    fn presets() -> Vec<(&'static str, Appearance)> {
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
                style: HairStyle::Long.name().into(),
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
                style: HairStyle::Buzz.name().into(),
                color: HairColor {
                    melanin: 1.0,
                    red: 0.05,
                    grey: 0.0,
                },
                curl: 1.0,
                ..Default::default()
            },
            beard: Beard {
                style: BeardStyle::Short.name().into(),
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
                style: HairStyle::Bun.name().into(),
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
                style: HairStyle::Short.name().into(),
                color: HairColor {
                    melanin: 0.85,
                    red: 0.1,
                    grey: 0.3,
                },
                recession: 0.35,
                ..Default::default()
            },
            beard: Beard {
                style: BeardStyle::Full.name().into(),
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
                style: HairStyle::Short.name().into(),
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
                style: HairStyle::Medium.name().into(),
                color: HairColor {
                    melanin: 0.95,
                    red: 0.05,
                    grey: 0.0,
                },
                curl: 0.45,
                ..Default::default()
            },
            beard: Beard {
                style: BeardStyle::Stubble.name().into(),
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
    fn preset(name: &str) -> Option<Appearance> {
        Self::presets()
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, a)| a)
    }
}

/// A random person, with the request's sex, age range and ancestry where it gives them (drawn
/// before what depends on them, so coloring matches the ancestry).
fn generate(seed: u64, req: &AppearanceRequest) -> Appearance {
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
    a.ancestry = req.ancestry.unwrap_or(Ancestry {
        african: w[0],
        asian: w[1],
        european: w[2],
    });
    let n = a.ancestry.normalized();
    let sex = if r.f32() < 0.5 {
        r.range(0.0, 0.1)
    } else {
        r.range(0.9, 1.0)
    };
    a.sex = req.sex.map_or(sex, |s| s.clamp(0.0, 1.0));
    let age = (r.range(18.0, 75.0) + r.normal() * 4.0).clamp(16.0, 88.0);
    a.age = match req.age {
        Some([lo, hi]) => {
            let (lo, hi) = (lo.min(hi).clamp(1.0, 90.0), lo.max(hi).clamp(1.0, 90.0));
            lo + (hi - lo) * ((age - 16.0) / 72.0).clamp(0.0, 1.0)
        }
        None => age,
    };
    a.muscle = (0.5 + r.normal() * 0.15).clamp(0.05, 0.95);
    a.weight = (0.5 + r.normal() * 0.15).clamp(0.05, 0.95);
    a.height = (0.5 + r.normal() * 0.2).clamp(0.0, 1.0);
    // Pigments by ancestry, with spread.
    let mel = n[0] * r.range(0.62, 0.95) + n[1] * r.range(0.22, 0.45) + n[2] * r.range(0.04, 0.4);
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
        }
        .name()
        .into(),
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
            style: BeardStyle::ALL[(r.f32() * 5.0) as usize % 5].name().into(),
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
        assert_eq!(partial.hair_style(), HairStyle::Long);
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

    #[test]
    fn requests_keep_their_limits_and_families_look_related() {
        let req = |seed, family| AppearanceRequest {
            seed,
            family,
            sex: Some(1.0),
            age: Some([20.0, 30.0]),
            ancestry: None,
        };
        let a = Appearance::from_request(&req(5, None));
        assert_eq!(a, Appearance::from_request(&req(5, None)));
        assert_eq!(a.seed, 5);
        assert_eq!(a.sex, 1.0);
        assert!((20.0..=30.0).contains(&a.age), "{}", a.age);
        // Two members of one family are closer than two strangers.
        let (b, c) = (
            Appearance::from_request(&req(1, Some(9))),
            Appearance::from_request(&req(2, Some(9))),
        );
        let (x, y) = (
            Appearance::from_request(&req(1, None)),
            Appearance::from_request(&req(2, None)),
        );
        assert_eq!(b.ancestry, c.ancestry);
        let face = |p: &Appearance, q: &Appearance| -> f32 {
            p.shape
                .keys()
                .map(|k| (p.slider(k) - q.slider(k)).abs())
                .sum()
        };
        assert!(
            face(&b, &c) < face(&x, &y) * 0.8,
            "{} {}",
            face(&b, &c),
            face(&x, &y)
        );
        assert_ne!(b, c);
    }
}
