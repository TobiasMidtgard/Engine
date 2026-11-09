//! A spell as a player or designer writes it: effect + cast type + modifiers + stages + power
//! (magic-system.md section 5, spell-combination.md section 6). [`SpellDesign`] is plain data
//! (JSON through the `magic.*` API); [`crate::compile`] turns it into a runtime spell.
//!
//! Every part has a complexity cost, and a caster can only hold spells within the budget their
//! mastery gives them:
//!
//! | Part | Complexity |
//! |---|---|
//! | Each effect beyond the first | +1 |
//! | Area-like cast types (area, wall, aura, rain, wave, rune) | +1 |
//! | Most modifiers | +1 |
//! | Seek, boomerang, rend, a linger stage, a blast (impact stage + area) | +2 |
//! | Split and fork | +1 per extra projectile |
//! | Each stage | +1, a condition (on kill, on block) +2 |
//!
//! Worked out (tested below): the Master fire volley, five homing fireballs that explode and
//! leave burning ground, is 4 (split) + 2 (seek) + 2 (blast) + 1 (the blast's push) + 2
//! (linger) = 11, inside a Master's 12 and far past an Expert's 8.

use serde::{Deserialize, Serialize};

use crate::effects::{EffectKind, ForceDir, effect};

/// How a spell leaves the caster (magic-system.md section 5, step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastType {
    /// On what you touch: cheapest and strongest.
    Touch,
    /// On your weapon; triggers on each hit (the spellblade's cast type).
    Imbue,
    /// A projectile in a straight line.
    #[default]
    Bolt,
    /// A thrown arc that goes over cover.
    Lob,
    /// A continuous line while held.
    Beam,
    /// A spray in front of you.
    Cone,
    /// Travels along the ground and climbs slopes.
    Wave,
    /// A line drawn on the ground.
    Wall,
    /// A circle where you aim.
    Area,
    /// Around you while it lasts; moves with you.
    Aura,
    /// A glyph on a surface that fires when someone steps on it.
    Rune,
    /// Circles you and hits what comes close.
    Orbit,
    /// Links you to a target while in range.
    Tether,
    /// Jumps from target to target.
    Chain,
    /// Falls from the sky over an area.
    Rain,
    /// On you.
    #[serde(rename = "self")]
    OnSelf,
    /// Places a creature, object or construct.
    Summon,
}

impl CastType {
    pub const ALL: [CastType; 17] = [
        CastType::Touch,
        CastType::Imbue,
        CastType::Bolt,
        CastType::Lob,
        CastType::Beam,
        CastType::Cone,
        CastType::Wave,
        CastType::Wall,
        CastType::Area,
        CastType::Aura,
        CastType::Rune,
        CastType::Orbit,
        CastType::Tether,
        CastType::Chain,
        CastType::Rain,
        CastType::OnSelf,
        CastType::Summon,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CastType::Touch => "Touch",
            CastType::Imbue => "Imbue",
            CastType::Bolt => "Bolt",
            CastType::Lob => "Lob",
            CastType::Beam => "Beam",
            CastType::Cone => "Cone",
            CastType::Wave => "Wave",
            CastType::Wall => "Wall",
            CastType::Area => "Area",
            CastType::Aura => "Aura",
            CastType::Rune => "Rune",
            CastType::Orbit => "Orbit",
            CastType::Tether => "Tether",
            CastType::Chain => "Chain",
            CastType::Rain => "Rain",
            CastType::OnSelf => "Self",
            CastType::Summon => "Summon",
        }
    }

    /// Covers an area (costs one more complexity).
    pub fn area_like(self) -> bool {
        matches!(
            self,
            CastType::Area
                | CastType::Wall
                | CastType::Aura
                | CastType::Rain
                | CastType::Wave
                | CastType::Rune
        )
    }

    /// The engine runs it (every cast type does since Imbue rides on weapons, round 14).
    pub fn runs(self) -> bool {
        true
    }

    /// Base mana of a spell of this form at the caster's own power.
    pub fn base_mana(self) -> f32 {
        match self {
            CastType::Touch => 6.0,
            CastType::Imbue => 12.0,
            CastType::Bolt => 10.0,
            CastType::Lob => 12.0,
            CastType::Beam => 14.0,
            CastType::Cone => 12.0,
            CastType::Wave => 14.0,
            CastType::Wall => 18.0,
            CastType::Area => 16.0,
            CastType::Aura => 14.0,
            CastType::Rune => 12.0,
            CastType::Orbit => 16.0,
            CastType::Tether => 8.0,
            CastType::Chain => 16.0,
            CastType::Rain => 30.0,
            CastType::OnSelf => 8.0,
            CastType::Summon => 25.0,
        }
    }

    /// How hard it hits compared with a bolt: touch is strongest, areas and rains spread their
    /// energy out.
    pub fn strength(self) -> f32 {
        match self {
            CastType::Touch => 1.5,
            CastType::Imbue => 0.6,
            CastType::Beam => 0.35,
            CastType::Cone => 0.7,
            CastType::Wave | CastType::Wall => 0.6,
            CastType::Area => 0.8,
            CastType::Aura => 0.3,
            CastType::Rune => 1.2,
            CastType::Orbit => 0.5,
            CastType::Tether => 0.4,
            CastType::Chain => 0.75,
            CastType::Rain => 0.45,
            _ => 1.0,
        }
    }
}

/// Who a spell may affect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetFilter {
    /// Harmful spells hit everyone but the caster; helpful ones help everyone.
    #[default]
    Everyone,
    /// Only those the caster's side counts as foes.
    Enemies,
    /// Only the caster and friends.
    Allies,
}

/// Step 3: metamagic. Each costs complexity (see the module docs); dark-only ones are marked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Modifier {
    /// Explodes on impact with this radius (adds a push if the spell has none).
    Blast { radius_m: f32 },
    /// Flies faster (multiplier).
    Speed { factor: f32 },
    /// Goes farther (multiplier).
    Range { factor: f32 },
    /// Bigger projectile or area (multiplier).
    Size { factor: f32 },
    /// Several projectiles in a fan.
    Split { count: u32 },
    /// Splits into more when it hits.
    Fork { count: u32 },
    /// Jumps to further targets.
    Chain { jumps: u32 },
    /// Homes in on targets.
    Seek,
    /// Passes through this many targets.
    Pierce { count: u32 },
    /// Ricochets off walls.
    Bounce { count: u32 },
    /// Flies out and back, hitting on both passes.
    Boomerang,
    /// Lodges in the target and stacks; recast to rip them out.
    Rend,
    /// Each hit adds a stack; at `threshold` the status bursts (frost freezes, blood bursts).
    Stacking { threshold: u32 },
    /// Lasts longer (multiplier).
    Duration { factor: f32 },
    /// Leaves a field where it lands: burning ground, a healing pool.
    Linger { secs: f32, radius_m: f32 },
    /// Fires after a fuse.
    Delay { secs: f32 },
    /// Hold to grow it (up to `max_secs`, doubling power at full charge), release to cast.
    Charge { max_secs: f32 },
    /// Recasts itself once a moment later.
    Echo,
    /// Only affects these.
    Filter { who: TargetFilter },
    /// Never harms allies or you (cheap on clean spells, costly on dark).
    Safeguard,
    /// No sound or light when cast.
    Silent,
    /// Knocks out instead of killing (clean only).
    Subdue,
    /// Part of the damage heals the caster (dark only).
    Lifesteal { share: f32 },
    /// Pay with health instead of mana (dark only).
    BloodPrice,
    /// Past the budget, with a growing chance to backfire.
    Overcharge,
}

impl Modifier {
    /// Complexity this modifier adds to a spell.
    pub fn complexity(&self, dark: bool) -> u32 {
        match *self {
            Modifier::Split { count } | Modifier::Fork { count } => count.saturating_sub(1),
            Modifier::Chain { jumps } => jumps.div_ceil(2).max(1),
            Modifier::Seek | Modifier::Boomerang | Modifier::Rend => 2,
            Modifier::Blast { .. } | Modifier::Linger { .. } => 2,
            Modifier::Safeguard => {
                if dark {
                    2
                } else {
                    1
                }
            }
            Modifier::BloodPrice | Modifier::Overcharge => 0,
            _ => 1,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Modifier::Blast { .. } => "Blast zone",
            Modifier::Speed { .. } => "Speed",
            Modifier::Range { .. } => "Range",
            Modifier::Size { .. } => "Size",
            Modifier::Split { .. } => "Split",
            Modifier::Fork { .. } => "Fork",
            Modifier::Chain { .. } => "Chain",
            Modifier::Seek => "Seek",
            Modifier::Pierce { .. } => "Pierce",
            Modifier::Bounce { .. } => "Bounce",
            Modifier::Boomerang => "Boomerang",
            Modifier::Rend => "Rend",
            Modifier::Stacking { .. } => "Stacking",
            Modifier::Duration { .. } => "Duration",
            Modifier::Linger { .. } => "Linger",
            Modifier::Delay { .. } => "Delay",
            Modifier::Charge { .. } => "Charge",
            Modifier::Echo => "Echo",
            Modifier::Filter { .. } => "Target filter",
            Modifier::Safeguard => "Safeguard",
            Modifier::Silent => "Silent",
            Modifier::Subdue => "Subdue",
            Modifier::Lifesteal { .. } => "Lifesteal",
            Modifier::BloodPrice => "Blood price",
            Modifier::Overcharge => "Overcharge",
        }
    }

    /// Only dark spells may carry it.
    pub fn dark_only(&self) -> bool {
        matches!(self, Modifier::Lifesteal { .. } | Modifier::BloodPrice)
    }

    /// Only clean spells may carry it.
    pub fn clean_only(&self) -> bool {
        matches!(self, Modifier::Subdue)
    }
}

/// When a stage fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// When the carrier hits something or reaches its end.
    Impact,
    /// On each creature it hits.
    Hit,
    /// When it ends (a ward that explodes when it breaks).
    Expire,
    /// When its victim dies.
    Kill,
}

impl Trigger {
    /// Complexity of a stage with this trigger.
    pub fn complexity(self) -> u32 {
        match self {
            Trigger::Impact | Trigger::Hit | Trigger::Expire => 1,
            Trigger::Kill => 2,
        }
    }
}

/// A further spell fired by a trigger (spell-combination.md section 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub trigger: Trigger,
    pub spell: SpellDesign,
}

/// A spell as data: what it does, how it leaves you, its metamagic and later stages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellDesign {
    /// Its name in the grimoire (empty: named from its weave).
    pub name: String,
    /// Effect keys (`fire`, `push`, `heal`...); several weave into one.
    pub effects: Vec<String>,
    pub cast: CastType,
    pub modifiers: Vec<Modifier>,
    pub stages: Vec<Stage>,
}

impl Default for SpellDesign {
    fn default() -> Self {
        Self {
            name: String::new(),
            effects: vec!["fire".into()],
            cast: CastType::Bolt,
            modifiers: Vec::new(),
            stages: Vec::new(),
        }
    }
}

impl SpellDesign {
    pub fn new(effects: &[&str], cast: CastType) -> Self {
        Self {
            name: String::new(),
            effects: effects.iter().map(|s| s.to_string()).collect(),
            cast,
            modifiers: Vec::new(),
            stages: Vec::new(),
        }
    }

    pub fn named(mut self, name: &str) -> Self {
        self.name = name.into();
        self
    }

    pub fn with(mut self, m: Modifier) -> Self {
        self.modifiers.push(m);
        self
    }

    pub fn stage(mut self, trigger: Trigger, spell: SpellDesign) -> Self {
        self.stages.push(Stage { trigger, spell });
        self
    }

    /// Any dark ingredient makes the whole spell dark, stages included.
    pub fn is_dark(&self) -> bool {
        self.effects
            .iter()
            .filter_map(|k| effect(k))
            .any(|e| e.is_dark())
            || self.stages.iter().any(|s| s.spell.is_dark())
    }

    /// Every distinct effect key in the spell and its stages, plus the push a blast adds.
    pub fn all_effects(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        self.collect_effects(&mut out);
        out
    }

    fn collect_effects(&self, out: &mut Vec<String>) {
        for e in &self.effects {
            if !out.contains(e) {
                out.push(e.clone());
            }
        }
        let has_force = self.effects.iter().any(|k| {
            effect(k).is_some_and(|e| {
                matches!(
                    e.kind,
                    EffectKind::Force {
                        dir: ForceDir::Away,
                        ..
                    }
                )
            })
        });
        if !has_force
            && self
                .modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Blast { .. }))
            && !out.iter().any(|k| k == "push")
        {
            out.push("push".into());
        }
        for s in &self.stages {
            s.spell.collect_effects(out);
        }
    }

    /// Complexity: what the spell asks of the caster's mastery (see the module docs).
    pub fn complexity(&self) -> u32 {
        let dark = self.is_dark();
        let effects = self.all_effects().len().saturating_sub(1) as u32;
        effects + self.parts_complexity(dark)
    }

    fn parts_complexity(&self, dark: bool) -> u32 {
        let form = self.cast.area_like() as u32;
        let mods: u32 = self.modifiers.iter().map(|m| m.complexity(dark)).sum();
        let stages: u32 = self
            .stages
            .iter()
            .map(|s| s.trigger.complexity() + s.spell.parts_complexity(dark))
            .sum();
        form + mods + stages
    }

    /// What's wrong with it, as plain sentences (empty when it can be cast).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.effects.is_empty() {
            out.push("A spell needs at least one effect (fire, push, heal...).".to_string());
        }
        for k in &self.effects {
            if effect(k).is_none() {
                out.push(format!(
                    "No effect named \"{k}\"; magic.effects lists them."
                ));
            }
        }
        let dark = self.is_dark();
        for m in &self.modifiers {
            if m.dark_only() && !dark {
                out.push(format!("{} needs a dark ingredient.", m.name()));
            }
            if m.clean_only() && dark {
                out.push(format!("{} works only on clean spells.", m.name()));
            }
        }
        for s in &self.stages {
            out.extend(s.spell.problems());
        }
        out
    }

    pub fn has(&self, f: impl Fn(&Modifier) -> bool) -> bool {
        self.modifiers.iter().any(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master_volley() -> SpellDesign {
        SpellDesign::new(&["fire"], CastType::Bolt)
            .with(Modifier::Split { count: 5 })
            .with(Modifier::Seek)
            .with(Modifier::Blast { radius_m: 3.0 })
            .with(Modifier::Linger {
                secs: 10.0,
                radius_m: 2.5,
            })
    }

    #[test]
    fn the_master_fire_volley_costs_eleven() {
        let v = master_volley();
        assert_eq!(
            v.all_effects(),
            vec!["fire".to_string(), "push".to_string()]
        );
        assert_eq!(v.complexity(), 11);
        // Expert: three slightly homing fireballs, each exploding: inside 8.
        let expert = SpellDesign::new(&["fire"], CastType::Bolt)
            .with(Modifier::Split { count: 3 })
            .with(Modifier::Seek)
            .with(Modifier::Blast { radius_m: 2.5 });
        assert_eq!(expert.complexity(), 7);
        // A plain novice bolt costs nothing.
        assert_eq!(SpellDesign::default().complexity(), 0);
    }

    #[test]
    fn stages_and_areas_cost_complexity() {
        let meteor = SpellDesign::new(&["fire"], CastType::Rain)
            .with(Modifier::Seek)
            .with(Modifier::Blast { radius_m: 4.0 })
            .stage(
                Trigger::Impact,
                SpellDesign::new(&["fire"], CastType::Area)
                    .with(Modifier::Duration { factor: 4.0 }),
            );
        // push (1) + rain (1) + seek (2) + blast (2) + stage (1) + area (1) + duration (1).
        assert_eq!(meteor.complexity(), 9);
        let doom_on_kill = SpellDesign::new(&["blood"], CastType::Bolt).stage(
            Trigger::Kill,
            SpellDesign::new(&["blight", "shadow"], CastType::Chain),
        );
        // two more effects (2) + a kill condition (2).
        assert_eq!(doom_on_kill.complexity(), 4);
        assert!(doom_on_kill.is_dark());
    }

    #[test]
    fn dark_and_clean_only_modifiers_are_checked() {
        let bad = SpellDesign::new(&["fire"], CastType::Bolt).with(Modifier::BloodPrice);
        assert_eq!(bad.problems().len(), 1);
        let bad = SpellDesign::new(&["blood"], CastType::Bolt).with(Modifier::Subdue);
        assert_eq!(bad.problems().len(), 1);
        let ok = SpellDesign::new(&["blood"], CastType::Bolt)
            .with(Modifier::Lifesteal { share: 0.3 })
            .with(Modifier::BloodPrice);
        assert!(ok.problems().is_empty());
        assert!(
            !SpellDesign::new(&["nope"], CastType::Bolt)
                .problems()
                .is_empty()
        );
        // Safeguard is cheap on clean spells and dear on dark ones.
        let clean = SpellDesign::new(&["fire"], CastType::Area).with(Modifier::Safeguard);
        let dark = SpellDesign::new(&["blight"], CastType::Area).with(Modifier::Safeguard);
        assert_eq!(dark.complexity(), clean.complexity() + 1);
    }

    #[test]
    fn designs_round_trip_as_json() {
        let v = master_volley().named("Volley");
        let json = serde_json::to_string(&v).unwrap();
        let back: SpellDesign = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
        let typed: SpellDesign = serde_json::from_str(
            r#"{"effects": ["frost"], "cast": "self", "modifiers": [{"kind": "seek"}]}"#,
        )
        .unwrap();
        assert_eq!(typed.cast, CastType::OnSelf);
        assert_eq!(typed.modifiers, vec![Modifier::Seek]);
    }
}
