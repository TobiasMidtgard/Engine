//! Combining: the Infinite Craft part (spell-combination.md section 5, magic-system.md section 4).
//!
//! One table answers three questions, so they always agree:
//! - the weave board: drop Fire on Frost and get Steam;
//! - a spell with several effects: `["fire", "frost"]` is a steam spell;
//! - elements meeting in the world: a fire bolt hitting a chilled victim, or flying through a
//!   frost field, makes a steam burst (the way Genshin's aura reactions and Magicka's element
//!   rules work, but from the same data).
//!
//! A combination is resolved in order: an authored recipe (named, with a reaction), else the
//! systemic fallback: the dials of every ingredient at once, named from its poles and the 21
//! domain weaves, so no combination is ever "nothing happens". Combinations are unordered and
//! nest: Steam + Storm is Fire + Frost + Storm, which is Aether. Anything with a dark ingredient
//! is dark. Ingredients that pull against each other (fire and frost) are unstable: stronger,
//! with a chance to backfire.

use serde::{Deserialize, Serialize};

use crate::dials::{Dials, Domain, Pole};
use crate::effects::{effect, element_effect};
use crate::schools::{Element, Status};

/// What happens when two elements meet on a victim or in the air (and a woven spell's bonus).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Reaction {
    /// Multiplier on the hit that triggers it (plasma cuts armour: 2x).
    pub bonus: f32,
    /// Statuses it leaves (in stacks), beyond the hit's own.
    pub statuses: &'static [(Status, f32)],
    /// A cloud left where it happens (steam, miasma), radius m; 0 = none.
    pub cloud_m: f32,
    /// The cloud's darkness, 0 = white steam .. 1 = black.
    pub cloud_dark: f32,
    /// Jumps to this many nearby victims (tempest, blood lightning).
    pub chains: u32,
    /// Share of the damage that heals the caster (nightfeed).
    pub drain: f32,
    /// Spreads its statuses to bodies within a few meters (plague, doom).
    pub spreads: bool,
    /// Puts out burning and thaws chill (steam).
    pub quench: bool,
}

const fn reaction(bonus: f32, statuses: &'static [(Status, f32)]) -> Reaction {
    Reaction {
        bonus,
        statuses,
        cloud_m: 0.0,
        cloud_dark: 0.0,
        chains: 0,
        drain: 0.0,
        spreads: false,
        quench: false,
    }
}

/// A hand-made combination.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Recipe {
    /// Look and lookup key (`steam`): the vfx looks are `spell.<key>.<role>`.
    pub key: &'static str,
    pub name: &'static str,
    /// Ingredient effect keys, sorted.
    pub ingredients: &'static [&'static str],
    pub description: &'static str,
    pub reaction: Reaction,
    /// Signature color, linear RGB.
    pub color: [f32; 3],
}

use Status::*;

/// Every authored recipe. Element pairs need Adept in both, triples Master (magic-system.md).
pub const RECIPES: &[Recipe] = &[
    // Clean pairs and triple.
    Recipe {
        key: "steam",
        name: "Steam",
        ingredients: &["fire", "frost"],
        description: "Scalding clouds that burn and blind.",
        reaction: Reaction {
            cloud_m: 3.5,
            quench: true,
            ..reaction(1.5, &[(Burning, 0.5)])
        },
        color: [0.92, 0.95, 1.0],
    },
    Recipe {
        key: "plasma",
        name: "Plasma",
        ingredients: &["fire", "storm"],
        description: "A white-hot charge that cuts armour.",
        reaction: reaction(2.0, &[(Burning, 1.0), (Shocked, 1.0)]),
        color: [1.0, 0.85, 1.0],
    },
    Recipe {
        key: "tempest",
        name: "Tempest",
        ingredients: &["frost", "storm"],
        description: "Hail, wind and lightning: slows and chains.",
        reaction: Reaction {
            chains: 3,
            ..reaction(1.3, &[(Chilled, 2.0), (Shocked, 1.0)])
        },
        color: [0.55, 0.8, 1.0],
    },
    Recipe {
        key: "aether",
        name: "Aether",
        ingredients: &["fire", "frost", "storm"],
        description: "Shifts between all three and strikes whatever a target is weakest to.",
        reaction: Reaction {
            chains: 2,
            ..reaction(1.8, &[(Burning, 1.0), (Chilled, 2.0), (Shocked, 1.0)])
        },
        color: [0.85, 0.75, 1.0],
    },
    // Dark pairs and triple.
    Recipe {
        key: "plague",
        name: "Plague",
        ingredients: &["blight", "blood"],
        description: "Infectious bleeding that spreads to the next body.",
        reaction: Reaction {
            spreads: true,
            cloud_m: 2.5,
            cloud_dark: 0.45,
            ..reaction(1.2, &[(Bleeding, 1.0), (Rot, 1.0)])
        },
        color: [0.55, 0.45, 0.08],
    },
    Recipe {
        key: "nightfeed",
        name: "Nightfeed",
        ingredients: &["blood", "shadow"],
        description: "Drains life from anything standing in darkness, unseen.",
        reaction: Reaction {
            drain: 0.5,
            ..reaction(1.3, &[(Bleeding, 1.0), (Dread, 1.0)])
        },
        color: [0.45, 0.02, 0.18],
    },
    Recipe {
        key: "wither",
        name: "Wither",
        ingredients: &["blight", "shadow"],
        description: "Necrotic decay: ages flesh, and the wound cannot be healed.",
        reaction: reaction(1.4, &[(Rot, 2.0), (Dread, 1.0)]),
        color: [0.3, 0.32, 0.12],
    },
    Recipe {
        key: "doom",
        name: "Doom",
        ingredients: &["blight", "blood", "shadow"],
        description: "A curse that does all three and leaps to a new host when its victim dies.",
        reaction: Reaction {
            spreads: true,
            drain: 0.25,
            ..reaction(1.6, &[(Bleeding, 1.0), (Rot, 1.0), (Dread, 1.0)])
        },
        color: [0.25, 0.02, 0.12],
    },
    // Clean + dark: always dark.
    Recipe {
        key: "boiling_blood",
        name: "Boiling blood",
        ingredients: &["blood", "fire"],
        description: "The victim's blood boils in their veins.",
        reaction: reaction(1.6, &[(Bleeding, 1.0), (Burning, 1.0)]),
        color: [1.0, 0.15, 0.05],
    },
    Recipe {
        key: "hellfire",
        name: "Hellfire",
        ingredients: &["blight", "fire"],
        description: "Burns that never heal.",
        reaction: reaction(1.4, &[(Burning, 1.5), (Rot, 1.0)]),
        color: [0.65, 0.85, 0.1],
    },
    Recipe {
        key: "black_flame",
        name: "Black flame",
        ingredients: &["fire", "shadow"],
        description: "Fire that gives no light.",
        reaction: reaction(1.3, &[(Burning, 1.0), (Dread, 1.0)]),
        color: [0.3, 0.05, 0.4],
    },
    Recipe {
        key: "frozen_veins",
        name: "Frozen veins",
        ingredients: &["blood", "frost"],
        description: "Blood turns to ice inside the body.",
        reaction: reaction(1.4, &[(Chilled, 3.0), (Bleeding, 1.0)]),
        color: [0.55, 0.1, 0.25],
    },
    Recipe {
        key: "frostrot",
        name: "Frostrot",
        ingredients: &["blight", "frost"],
        description: "Frostbite that rots.",
        reaction: reaction(1.3, &[(Chilled, 2.0), (Rot, 1.0)]),
        color: [0.45, 0.7, 0.5],
    },
    Recipe {
        key: "black_ice",
        name: "Black ice",
        ingredients: &["frost", "shadow"],
        description: "A freezing dark that terrifies.",
        reaction: reaction(1.3, &[(Chilled, 2.0), (Dread, 1.0)]),
        color: [0.2, 0.25, 0.45],
    },
    Recipe {
        key: "blood_lightning",
        name: "Blood lightning",
        ingredients: &["blood", "storm"],
        description: "Lightning that chains through everyone bleeding.",
        reaction: Reaction {
            chains: 4,
            ..reaction(1.3, &[(Shocked, 1.0), (Bleeding, 1.0)])
        },
        color: [0.9, 0.1, 0.25],
    },
    Recipe {
        key: "acid_storm",
        name: "Acid storm",
        ingredients: &["blight", "storm"],
        description: "A storm that eats armour.",
        reaction: reaction(1.5, &[(Shocked, 1.0), (Rot, 1.0)]),
        color: [0.6, 0.9, 0.2],
    },
    Recipe {
        key: "dark_thunder",
        name: "Dark thunder",
        ingredients: &["shadow", "storm"],
        description: "Lightning nobody sees coming.",
        reaction: reaction(1.5, &[(Shocked, 1.0), (Dread, 1.0)]),
        color: [0.4, 0.25, 0.7],
    },
    // Elements with other families (magic-system.md section 4).
    Recipe {
        key: "rocket_leap",
        name: "Rocket leap",
        ingredients: &["fire", "lift"],
        description: "A burst of flame under your feet throws you into the air.",
        reaction: reaction(1.0, &[(Burning, 0.5)]),
        color: [1.0, 0.5, 0.15],
    },
    Recipe {
        key: "shardstorm",
        name: "Shardstorm",
        ingredients: &["frost", "push"],
        description: "A blast of ice shards that knocks people back.",
        reaction: reaction(1.2, &[(Chilled, 1.0)]),
        color: [0.6, 0.85, 1.0],
    },
    Recipe {
        key: "thunderclap",
        name: "Thunderclap",
        ingredients: &["push", "storm"],
        description: "A shockwave of thunder that stuns everyone near.",
        reaction: reaction(1.2, &[(Shocked, 1.5)]),
        color: [0.7, 0.75, 1.0],
    },
    Recipe {
        key: "radiance",
        name: "Radiance",
        ingredients: &["fire", "heal"],
        description: "Sunlight that heals friends and burns the undead.",
        reaction: reaction(1.0, &[]),
        color: [1.0, 0.85, 0.45],
    },
    Recipe {
        key: "ghost_step",
        name: "Ghost step",
        ingredients: &["blink", "veil"],
        description: "A step through space that leaves no trace.",
        reaction: reaction(1.0, &[]),
        color: [0.7, 0.8, 1.0],
    },
    Recipe {
        key: "night_terror",
        name: "Night terror",
        ingredients: &["fear", "shadow"],
        description: "Darkness that brings the victim's worst fear.",
        reaction: reaction(1.2, &[(Dread, 3.0)]),
        color: [0.25, 0.05, 0.35],
    },
    Recipe {
        key: "deep_freeze",
        name: "Deep freeze",
        ingredients: &["frost", "slow"],
        description: "Cold so deep that time itself stiffens.",
        reaction: reaction(1.1, &[(Chilled, 4.0)]),
        color: [0.5, 0.8, 1.0],
    },
    Recipe {
        key: "transfer",
        name: "Transfer",
        ingredients: &["drain", "heal"],
        description: "Moves health from one body to another.",
        reaction: Reaction {
            drain: 1.0,
            ..reaction(1.0, &[])
        },
        color: [0.85, 0.2, 0.3],
    },
];

/// The 21 pairs of domains and what each weaves into (spell-combination.md section 5). The
/// name may depend on which way the first domain is turned (heat or cold, light or dark).
fn domain_weave(a: Domain, b: Domain, a_positive: bool) -> &'static str {
    use Domain::*;
    let (a, b, pos) = if a <= b {
        (a, b, a_positive)
    } else {
        (b, a, true)
    };
    match (a, b) {
        (Energy, Force) => "Lightning",
        (Energy, Life) => "Radiance",
        (Energy, Matter) => {
            if pos {
                "Lava"
            } else {
                "Ice"
            }
        }
        (Energy, Mind) => {
            if pos {
                "Light"
            } else {
                "Shadow"
            }
        }
        (Energy, Space) => "Sunlance",
        (Energy, Time) => "Charge",
        (Force, Life) => "Vigor",
        (Force, Matter) => "Hurl",
        (Force, Mind) => "Finesse",
        (Force, Space) => "Gravity",
        (Force, Time) => "Momentum",
        (Life, Matter) => "Conjured beast",
        (Life, Mind) => "Bond",
        (Life, Space) => "Transfer",
        (Life, Time) => {
            if pos {
                "Renewal"
            } else {
                "Aging"
            }
        }
        (Matter, Mind) => "Guise",
        (Matter, Space) => "Pocket",
        (Matter, Time) => {
            if pos {
                "Restore"
            } else {
                "Erode"
            }
        }
        (Mind, Space) => "Scry",
        (Mind, Time) => "Foresight",
        (Space, Time) => "Recall",
        _ => "Mastery",
    }
}

fn pole_noun(p: Pole) -> &'static str {
    match p {
        Pole::Heat => "Flame",
        Pole::Cold => "Frost",
        Pole::Push => "Force",
        Pole::Pull => "Grasp",
        Pole::Grow => "Bloom",
        Pole::Wither => "Blight",
        Pole::Form => "Shaping",
        Pole::Unform => "Unmaking",
        Pole::Reveal => "Sight",
        Pole::Veil => "Veil",
        Pole::Far => "Passage",
        Pole::Near => "Binding",
        Pole::Fast => "Haste",
        Pole::Slow => "Stasis",
    }
}

fn pole_adjective(p: Pole) -> &'static str {
    match p {
        Pole::Heat => "Searing",
        Pole::Cold => "Freezing",
        Pole::Push => "Hurling",
        Pole::Pull => "Grasping",
        Pole::Grow => "Living",
        Pole::Wither => "Withering",
        Pole::Form => "Forged",
        Pole::Unform => "Dissolving",
        Pole::Reveal => "Revealing",
        Pole::Veil => "Veiled",
        Pole::Far => "Far-flung",
        Pole::Near => "Binding",
        Pole::Fast => "Quickened",
        Pole::Slow => "Slowing",
    }
}

/// The result of combining ingredients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weave {
    /// Look key: the recipe's (`steam`), else the strongest element's (`fire`), else the
    /// strongest domain's (`force`, `life`, `mind`, `space`, `time`, `matter`).
    pub key: String,
    pub name: String,
    pub description: String,
    /// The effect keys it is made of, sorted and without repeats.
    pub ingredients: Vec<String>,
    pub dials: Dials,
    /// Elements among the ingredients (in [`Element::ALL`] order).
    pub elements: Vec<Element>,
    pub dark: bool,
    /// How hard the ingredients pull against each other, 0..1 (fire with frost is about 1).
    pub unstable: f32,
    /// From a hand-made recipe (otherwise systemic).
    pub authored: bool,
    /// Signature color, linear RGB.
    pub color: [f32; 3],
}

impl Weave {
    /// The recipe's reaction, or a plain one built from the elements.
    pub fn reaction(&self) -> Reaction {
        if let Some(r) = recipe_for(&self.ingredients) {
            return r.reaction;
        }
        reaction(
            1.0 + 0.1 * self.elements.len().saturating_sub(1) as f32,
            &[],
        )
    }
}

/// The recipe made of exactly these ingredients (any order).
pub fn recipe_for(ingredients: &[String]) -> Option<&'static Recipe> {
    let mut want: Vec<&str> = ingredients.iter().map(String::as_str).collect();
    want.sort_unstable();
    want.dedup();
    RECIPES.iter().find(|r| r.ingredients == want.as_slice())
}

/// The recipe with this result key (`steam`).
pub fn recipe(key: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|r| r.key == key)
}

/// The ingredient effect keys a name stands for: an effect key stands for itself; a recipe key
/// (`steam`) for its ingredients, so results combine again.
pub fn ingredients_of(key: &str) -> Option<Vec<String>> {
    if effect(key).is_some() {
        return Some(vec![key.to_string()]);
    }
    recipe(key).map(|r| r.ingredients.iter().map(|s| s.to_string()).collect())
}

/// Weave effect keys (or recipe keys) into one spell weave. Unknown keys are skipped; returns
/// `None` if nothing known is left.
pub fn weave(keys: &[String]) -> Option<Weave> {
    let mut ingredients: Vec<String> = keys
        .iter()
        .filter_map(|k| ingredients_of(k))
        .flatten()
        .collect();
    ingredients.sort_unstable();
    ingredients.dedup();
    if ingredients.is_empty() {
        return None;
    }
    let defs: Vec<_> = ingredients.iter().filter_map(|k| effect(k)).collect();
    let dark = defs.iter().any(|d| d.is_dark());
    let mut elements: Vec<Element> = defs.iter().filter_map(|d| d.element()).collect();
    elements.sort_unstable();
    elements.dedup();
    // Dials: every ingredient at once, and how much they fight.
    let mut dials = Dials::ZERO;
    let mut unstable: f32 = 0.0;
    for d in &defs {
        let x = d.dials();
        unstable = unstable.max(dials.tension(&x));
        dials = dials.weave(&x);
    }
    if let Some(r) = recipe_for(&ingredients) {
        return Some(Weave {
            key: r.key.to_string(),
            name: r.name.to_string(),
            description: r.description.to_string(),
            ingredients,
            dials,
            elements,
            dark,
            unstable,
            authored: true,
            color: r.color,
        });
    }
    let (key, name, description, color) = if defs.len() == 1 {
        let d = defs[0];
        let color = d
            .element()
            .map(Element::color)
            .unwrap_or_else(|| domain_color(dials.dominant().map(Pole::domain)));
        (
            d.element()
                .map(|e| e.key().to_string())
                .unwrap_or_else(|| look_of_dials(&dials)),
            d.name.to_string(),
            d.description.to_string(),
            color,
        )
    } else {
        systemic(&ingredients, &dials, &elements)
    };
    // Raised stone looks like stone (dust, grit, flung rock), whatever else is woven in,
    // unless an element colors it; with lift it is the Pillar of the Shaping list.
    let (key, name, color) = if ingredients.iter().any(|k| k == "stone") && elements.is_empty() {
        let lifted = ingredients.iter().any(|k| k == "lift");
        let name = match (lifted, defs.len()) {
            (true, 2) => "Pillar".to_string(),
            _ => name,
        };
        ("stone".to_string(), name, STONE_COLOR)
    } else if ingredients.iter().any(|k| k == "blade") && defs.len() <= 2 {
        // A conjured blade is named for what it is made of, and a plain one is glintstone
        // blue (blades.rs), with the force look for its flight and its breaking.
        let name = match elements.first() {
            Some(e) if defs.len() == 2 => format!("{} blade", element_effect(*e).name),
            _ => name,
        };
        if elements.is_empty() {
            ("force".to_string(), name, BLADE_COLOR)
        } else {
            (key, name, color)
        }
    } else if ingredients.iter().any(|k| k == "demon") && elements.is_empty() {
        // A pact's demons come in hellfire: fire's look, a deeper red (demons.rs).
        ("fire".to_string(), name, DEMON_COLOR)
    } else if ingredients.iter().any(|k| k == "chains") && elements.is_empty() {
        // Chains are iron and embers (chains.rs): the chain look (vfx draws a chain's crackle
        // and lends its bursts the nearest element's, fire's, in the embers' red).
        ("chain".to_string(), name, crate::chains::EMBER)
    } else if ingredients.iter().any(|k| k == "wilds") && elements.is_empty() {
        // Living wood breaks out of the ground: stone's look (dust, grit, clods) in the soil's
        // brown (wilds.rs), not the Life domain's green mist.
        ("stone".to_string(), name, crate::wilds::SOIL)
    } else if ingredients.iter().any(|k| k == "raise") && elements.is_empty() {
        // Raising the dead is dark: it looks like shadow, in the grave's violet, not like the
        // green of the Life domain its dials sit in (a bone blade's cut would puff green).
        ("shadow".to_string(), name, RAISE_COLOR)
    } else {
        (key, name, color)
    };
    Some(Weave {
        key,
        name,
        description,
        ingredients,
        dials,
        elements,
        dark,
        unstable,
        authored: false,
        color,
    })
}

/// A plain conjured blade's color: glintstone blue, linear RGB.
pub const BLADE_COLOR: [f32; 3] = [0.38, 0.66, 1.0];

/// Raised stone's color (a grey-brown granite), linear RGB.
pub const STONE_COLOR: [f32; 3] = [0.42, 0.38, 0.33];

/// The raised dead's color: the violet of the grave light in their eyes, linear RGB.
pub const RAISE_COLOR: [f32; 3] = [0.62, 0.3, 1.0];
/// Hellfire: a pact's demons' red.
pub const DEMON_COLOR: [f32; 3] = [1.0, 0.24, 0.05];

/// The look key of an effect without an element: its strongest domain.
fn look_of_dials(dials: &Dials) -> String {
    dials
        .dominant()
        .map(|p| p.domain().name().to_lowercase())
        .unwrap_or_else(|| "arcane".into())
}

fn domain_color(d: Option<Domain>) -> [f32; 3] {
    match d {
        Some(Domain::Energy) => [1.0, 0.6, 0.25],
        Some(Domain::Force) => [0.75, 0.85, 1.0],
        Some(Domain::Life) => [0.45, 1.0, 0.55],
        Some(Domain::Matter) => [0.85, 0.7, 0.5],
        Some(Domain::Mind) => [0.85, 0.55, 1.0],
        Some(Domain::Space) => [0.55, 0.4, 1.0],
        Some(Domain::Time) => [1.0, 0.9, 0.55],
        None => [0.8, 0.8, 1.0],
    }
}

/// The systemic fallback: name, look and color from the dials. A pair of different domains
/// takes its domain weave's name ("Lightning", "Gravity"); a third pole adds an adjective; a
/// spell whose elements already make a recipe keeps that recipe's name with the rest added
/// ("Steam of Haste").
fn systemic(
    ingredients: &[String],
    dials: &Dials,
    elements: &[Element],
) -> (String, String, String, [f32; 3]) {
    // Elements first: the best element recipe inside the mix names it.
    let element_keys: Vec<String> = elements.iter().map(|e| e.key().to_string()).collect();
    let base = if element_keys.len() >= 2 {
        recipe_for(&element_keys)
    } else {
        None
    };
    let poles = dials.poles();
    if let Some(r) = base {
        let rest: Vec<&str> = ingredients
            .iter()
            .filter(|k| !r.ingredients.contains(&k.as_str()))
            .filter_map(|k| effect(k).map(|e| e.name))
            .collect();
        let name = format!("{} of {}", r.name, rest.join(" and "));
        return (
            r.key.to_string(),
            name,
            format!("{} Woven with {}.", r.description, rest.join(" and ")),
            r.color,
        );
    }
    let key = elements
        .first()
        .map(|e| e.key().to_string())
        .unwrap_or_else(|| look_of_dials(dials));
    let color = elements
        .first()
        .map(|e| e.color())
        .unwrap_or_else(|| domain_color(dials.dominant().map(Pole::domain)));
    let names: Vec<&str> = ingredients
        .iter()
        .filter_map(|k| effect(k).map(|e| e.name))
        .collect();
    let name = match poles.as_slice() {
        [] => "Arcane spark".to_string(),
        [p] => format!("Pure {}", pole_noun(*p)),
        [a, b] if a.domain() != b.domain() => {
            domain_weave(a.domain(), b.domain(), a.sign() > 0.0).to_string()
        }
        [a, b, c, ..] => format!(
            "{} {}",
            pole_adjective(*c),
            domain_weave(a.domain(), b.domain(), a.sign() > 0.0)
        ),
        [a, ..] => format!("Pure {}", pole_noun(*a)),
    };
    let description = format!("A weave of {}.", names.join(", "));
    (key, name, description, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_design_tables_resolve_in_any_order() {
        let cases = [
            (&["fire", "frost"][..], "Steam"),
            (&["storm", "fire"][..], "Plasma"),
            (&["frost", "storm"][..], "Tempest"),
            (&["storm", "frost", "fire"][..], "Aether"),
            (&["blood", "blight"][..], "Plague"),
            (&["shadow", "blood"][..], "Nightfeed"),
            (&["shadow", "blight"][..], "Wither"),
            (&["shadow", "blood", "blight"][..], "Doom"),
            (&["fire", "blood"][..], "Boiling blood"),
            (&["blight", "fire"][..], "Hellfire"),
            (&["fire", "shadow"][..], "Black flame"),
            (&["frost", "blood"][..], "Frozen veins"),
            (&["frost", "blight"][..], "Frostrot"),
            (&["shadow", "frost"][..], "Black ice"),
            (&["storm", "blood"][..], "Blood lightning"),
            (&["blight", "storm"][..], "Acid storm"),
            (&["storm", "shadow"][..], "Dark thunder"),
        ];
        for (ks, name) in cases {
            let w = weave(&keys(ks)).unwrap();
            assert_eq!(w.name, name, "{ks:?}");
            assert!(w.authored);
            let dark = ks.iter().any(|k| effect(k).unwrap().is_dark());
            assert_eq!(w.dark, dark, "{ks:?}");
        }
    }

    #[test]
    fn results_combine_again() {
        // Steam + Storm = Fire + Frost + Storm = Aether, the Infinite Craft chain.
        let w = weave(&keys(&["steam", "storm"])).unwrap();
        assert_eq!(w.name, "Aether");
        let w = weave(&keys(&["plague", "shadow"])).unwrap();
        assert_eq!(w.name, "Doom");
        // Fire and frost fight: unstable. Fire and push agree: stable.
        assert!(weave(&keys(&["fire", "frost"])).unwrap().unstable > 0.9);
        assert_eq!(weave(&keys(&["fire", "lift"])).unwrap().unstable, 0.0);
    }

    #[test]
    fn the_raised_dead_look_like_shadow_until_an_element_colors_them() {
        let w = weave(&keys(&["raise"])).unwrap();
        assert_eq!((w.key.as_str(), w.color), ("shadow", RAISE_COLOR));
        assert!(w.dark);
        assert_eq!(weave(&keys(&["raise", "storm"])).unwrap().key, "storm");
        assert_eq!(weave(&keys(&["raise", "stone"])).unwrap().key, "stone");
    }

    #[test]
    fn every_combination_gives_something() {
        // No combination is ever "nothing happens": every pair of effects has a name, a look
        // and dials.
        let all: Vec<&str> = crate::effects::EFFECTS.iter().map(|e| e.key).collect();
        for a in &all {
            for b in &all {
                let w = weave(&keys(&[a, b])).unwrap();
                assert!(!w.name.is_empty(), "{a} + {b}");
                assert!(!w.key.is_empty(), "{a} + {b}");
                assert!(w.dials.magnitude() > 0.0, "{a} + {b}");
            }
        }
        // Systemic names come from the domain weaves.
        assert_eq!(weave(&keys(&["heal", "push"])).unwrap().name, "Vigor");
        assert_eq!(weave(&keys(&["lift", "blink"])).unwrap().name, "Gravity");
        let w = weave(&keys(&["fire", "frost", "haste"])).unwrap();
        assert_eq!(w.name, "Steam of Haste");
        assert_eq!(w.key, "steam");
        assert_eq!(weave(&keys(&["nothing"])), None);
    }
}
