//! The effect catalog: step 1 of crafting a spell ("what it does"). Each effect belongs to a
//! subclass and is a preset of the seven dials, so effects from different subclasses combine
//! (combine.rs) and anything with a dark ingredient is dark.

use serde::{Deserialize, Serialize};

use crate::dials::{Dials, Pole};
use crate::schools::{Element, Subclass};

/// Which way a force vector points, taken from the cast (magic-system.md section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForceDir {
    /// Away from the caster (or the blast's center): push back, knock over.
    Away,
    /// Toward the caster: pull, fetch, grapple.
    Toward,
    /// Up against gravity: lift, throw into the air, hover.
    Up,
    /// Down: slam, pin to the ground, force to their knees.
    Down,
}

/// What an effect does when it lands.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EffectKind {
    /// Elemental harm: damage plus the element's status.
    Harm { element: Element },
    /// A force vector; `strength` scales the push (1 = a hard shove).
    Force { dir: ForceDir, strength: f32 },
    /// Restore health.
    Heal,
    /// An absorbing shield on the target.
    Ward,
    /// The target's time runs faster (factor > 1) or slower (< 1).
    TimeScale { factor: f32 },
    /// Short teleport of the caster along the cast.
    Blink { dark: bool },
    /// Unseen: perception skips the target.
    Veil,
    /// Light that reveals and blinds.
    Light,
    /// Terrify: flee, fight badly.
    Fear,
    /// Anger drains away.
    Calm,
    /// Life leaves the target and comes to the caster.
    Drain,
}

/// One entry of the effect catalog.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EffectDef {
    /// Short key used in spell data and the API (`fire`, `push`, `heal`).
    pub key: &'static str,
    pub name: &'static str,
    pub subclass: Subclass,
    pub kind: EffectKind,
    pub description: &'static str,
}

impl EffectDef {
    /// The effect as dials, per unit of power.
    pub fn dials(&self) -> Dials {
        match self.kind {
            EffectKind::Harm { element } => element.dials(),
            EffectKind::Force { dir, strength } => match dir {
                ForceDir::Away | ForceDir::Up => Dials::pole(Pole::Push, strength),
                ForceDir::Toward | ForceDir::Down => Dials::pole(Pole::Pull, strength),
            },
            EffectKind::Heal => Dials::pole(Pole::Grow, 1.0),
            EffectKind::Ward => Dials::pole(Pole::Push, 0.5) + Dials::pole(Pole::Form, 0.5),
            EffectKind::TimeScale { factor } => {
                if factor >= 1.0 {
                    Dials::pole(Pole::Fast, 1.0)
                } else {
                    Dials::pole(Pole::Slow, 1.0)
                }
            }
            EffectKind::Blink { .. } => Dials::pole(Pole::Far, 1.0),
            EffectKind::Veil => Dials::pole(Pole::Veil, 1.0),
            EffectKind::Light => Dials::pole(Pole::Heat, 0.3) + Dials::pole(Pole::Reveal, 0.8),
            EffectKind::Fear => Dials::pole(Pole::Reveal, 0.4) + Dials::pole(Pole::Cold, 0.2),
            EffectKind::Calm => Dials::pole(Pole::Veil, 0.4) + Dials::pole(Pole::Grow, 0.2),
            EffectKind::Drain => Dials::pole(Pole::Wither, 0.7) + Dials::pole(Pole::Near, 0.7),
        }
    }

    pub fn is_dark(&self) -> bool {
        self.subclass.is_dark()
    }

    /// The element of a harm effect.
    pub fn element(&self) -> Option<Element> {
        match self.kind {
            EffectKind::Harm { element } => Some(element),
            EffectKind::Drain => Some(Element::Blood),
            _ => None,
        }
    }

    /// Hurts whoever it lands on (so safeguard and target filters matter).
    pub fn harmful(&self) -> bool {
        matches!(
            self.kind,
            EffectKind::Harm { .. }
                | EffectKind::Drain
                | EffectKind::Fear
                | EffectKind::Force { .. }
                | EffectKind::TimeScale { factor: ..1.0 }
        )
    }
}

const fn def(
    key: &'static str,
    name: &'static str,
    subclass: Subclass,
    kind: EffectKind,
    description: &'static str,
) -> EffectDef {
    EffectDef {
        key,
        name,
        subclass,
        kind,
        description,
    }
}

/// Every effect a spell can be built from.
pub const EFFECTS: &[EffectDef] = &[
    // Evocation: the clean elements.
    def(
        "fire",
        "Fire",
        Subclass::Evocation,
        EffectKind::Harm {
            element: Element::Fire,
        },
        "Burns and spreads: sets targets and dry fuel alight.",
    ),
    def(
        "frost",
        "Frost",
        Subclass::Evocation,
        EffectKind::Harm {
            element: Element::Frost,
        },
        "Chills and slows; five stacks freeze the target solid.",
    ),
    def(
        "storm",
        "Storm",
        Subclass::Evocation,
        EffectKind::Harm {
            element: Element::Storm,
        },
        "Lightning that stuns for a moment and jumps to the shocked and the wet.",
    ),
    // Malefice: the dark elements.
    def(
        "blood",
        "Blood",
        Subclass::Malefice,
        EffectKind::Harm {
            element: Element::Blood,
        },
        "Opens wounds that bleed, worse when the victim moves.",
    ),
    def(
        "blight",
        "Blight",
        Subclass::Malefice,
        EffectKind::Harm {
            element: Element::Blight,
        },
        "Rot and poison; the victim can't be healed while it lasts.",
    ),
    def(
        "shadow",
        "Shadow",
        Subclass::Malefice,
        EffectKind::Harm {
            element: Element::Shadow,
        },
        "Eats light and terrifies: the victim flees or drops their guard.",
    ),
    // Shaping: force vectors and passage.
    def(
        "push",
        "Push",
        Subclass::Shaping,
        EffectKind::Force {
            dir: ForceDir::Away,
            strength: 1.0,
        },
        "A force vector away from you: knock back, knock over, throw.",
    ),
    def(
        "pull",
        "Pull",
        Subclass::Shaping,
        EffectKind::Force {
            dir: ForceDir::Toward,
            strength: 1.0,
        },
        "A force vector toward you: fetch, drag an enemy in.",
    ),
    def(
        "lift",
        "Lift",
        Subclass::Shaping,
        EffectKind::Force {
            dir: ForceDir::Up,
            strength: 1.0,
        },
        "A force vector up: lift a crate, float an enemy helpless, hover.",
    ),
    def(
        "slam",
        "Slam",
        Subclass::Shaping,
        EffectKind::Force {
            dir: ForceDir::Down,
            strength: 1.2,
        },
        "A force vector down: pin to the ground, crush.",
    ),
    def(
        "blink",
        "Blink",
        Subclass::Shaping,
        EffectKind::Blink { dark: false },
        "Step through space to a point in sight.",
    ),
    // Vitalism: healing and wards.
    def(
        "heal",
        "Heal",
        Subclass::Vitalism,
        EffectKind::Heal,
        "Closes wounds and restores health.",
    ),
    def(
        "ward",
        "Ward",
        Subclass::Vitalism,
        EffectKind::Ward,
        "A shield that absorbs harm until it breaks.",
    ),
    def(
        "haste",
        "Haste",
        Subclass::Vitalism,
        EffectKind::TimeScale { factor: 1.35 },
        "Everything the target does is quicker.",
    ),
    // Necromancy.
    def(
        "drain",
        "Drain",
        Subclass::Necromancy,
        EffectKind::Drain,
        "Pulls the victim's life into the caster.",
    ),
    // Pactbinding: chains and dark passage.
    def(
        "chains",
        "Chains",
        Subclass::Pactbinding,
        EffectKind::Force {
            dir: ForceDir::Down,
            strength: 1.6,
        },
        "Chains that drag the victim down and pin them.",
    ),
    def(
        "slow",
        "Slow",
        Subclass::Pactbinding,
        EffectKind::TimeScale { factor: 0.45 },
        "The victim's time drags: every move is slow.",
    ),
    def(
        "shadow_step",
        "Shadow step",
        Subclass::Pactbinding,
        EffectKind::Blink { dark: true },
        "Step from one shadow to another.",
    ),
    // Illusion.
    def(
        "veil",
        "Veil",
        Subclass::Illusion,
        EffectKind::Veil,
        "Light bends around the target: guards look straight through.",
    ),
    def(
        "light",
        "Light",
        Subclass::Illusion,
        EffectKind::Light,
        "A light that reveals the hidden and dazzles the near.",
    ),
    def(
        "calm",
        "Calm",
        Subclass::Illusion,
        EffectKind::Calm,
        "Anger drains away; a fight may end.",
    ),
    // Domination.
    def(
        "fear",
        "Fear",
        Subclass::Domination,
        EffectKind::Fear,
        "Terror: the target flees or freezes.",
    ),
];

/// The effect with this key.
pub fn effect(key: &str) -> Option<&'static EffectDef> {
    EFFECTS.iter().find(|e| e.key == key)
}

/// The harm effect of an element.
pub fn element_effect(e: Element) -> &'static EffectDef {
    effect(e.key()).expect("every element has an effect")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_every_subclass_has_effects() {
        for (i, a) in EFFECTS.iter().enumerate() {
            for b in &EFFECTS[i + 1..] {
                assert_ne!(a.key, b.key);
            }
            assert!(a.dials().depth() >= 1, "{}", a.key);
        }
        for s in Subclass::ALL {
            assert!(EFFECTS.iter().any(|e| e.subclass == s), "{s:?}");
        }
        for e in Element::ALL {
            assert_eq!(element_effect(e).element(), Some(e));
            assert_eq!(element_effect(e).is_dark(), e.is_dark());
        }
    }
}
