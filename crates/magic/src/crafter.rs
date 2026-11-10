//! The Spellcrafter (engine-design/magic/round-7-plan.md; magic-system.md section 5,
//! "Spellcrafting"): an altar where a caster builds a spell in four steps, sees what it costs,
//! watches it fly at a practice dummy, learns it and puts it on a key.
//!
//! * [`Crafter`] is the open screen's state: whose it is, the design being built, the step
//!   shown and the last thing refused. [`apply`] runs one `magic.craft.*` action on it and
//!   [`layout`] draws it as the UI module's element JSON, so both work without the UI (tests,
//!   `magic.crafter`, an AI picking parts).
//! * The order is Ars Nouveau's: effects first (1 to 3, woven into one through the combination
//!   table), then one cast type, then modifiers, then power. As in Oblivion's altar, a caster
//!   only gets effects of subclasses they have skill in (Apprentice or better; their best
//!   subclass always). Every part shows its complexity, and a part that would take the spell
//!   past the caster's budget is dimmed and refused in red, unless Overcharge is on.
//! * The screen is a `layout` screen of the UI module (`ui.show`); its buttons send
//!   `magic.craft.*` actions that [`run_crafter`] reads back from `ui.events`, so no UI code
//!   knows about magic and the magic module needs no UI crate.
//! * An [`Altar`] (an `Interactable`, "Weave a spell", E) opens it for whoever uses it. The altar
//!   is a caster too: its crystal casts the design at its practice dummy every
//!   [`PREVIEW_EVERY`] s, free, at the weaver's level and mastery, so a change shows in the
//!   world a moment later. No other game in the round's research previews a spell in the world
//!   while it is built.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_render::{MaterialRef, MeshRenderer, PointLight};
use engine_world::interact::Interact;
use serde_json::{Value, json};

use crate::caster::{Aim, CastRequest, KnownSpell, MagicCaster, SpellChoice, Vitality};
use crate::combine::weave;
use crate::compile::Spell;
use crate::design::{CastType, Modifier, SpellDesign};
use crate::effects::{EFFECTS, EffectKind, effect};
use crate::player::{HudBar, SLOTS, SpellBar};
use crate::schools::{Mastery, Subclass};
use crate::summon::{MinionKind, kind_of};

/// The UI screen's key.
pub const SCREEN: &str = "spellcrafter";
/// Effects one spell may weave.
pub const MAX_EFFECTS: usize = 3;
/// Seconds between the crystal's preview casts.
pub const PREVIEW_EVERY: f32 = 2.5;
/// Seconds from opening to the crystal's first cast: a moment to read the screen.
pub const FIRST_PREVIEW: f32 = 2.0;
/// Seconds from a change to the preview that shows it.
pub const PREVIEW_AFTER_CHANGE: f32 = 0.35;
/// Every action the screen sends starts with this.
pub const ACTION: &str = "magic.craft.";
/// Where an unused altar keeps its dummy: under the floor, where no spell looks for it.
pub const DUMMY_AWAY_M: f64 = 40.0;
/// After the screen closes, the dummy stays up while the weaver is this close to the altar, m.
pub const STAY_M: f32 = 6.0;
/// The crystal's color while no one is at the altar, linear RGB.
pub const IDLE_GLOW: [f32; 3] = [0.45, 0.4, 1.0];

/// The four steps (magic-system.md section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    #[default]
    Effects,
    Cast,
    Modifiers,
    Power,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Effects, Step::Cast, Step::Modifiers, Step::Power];

    pub fn key(self) -> &'static str {
        match self {
            Step::Effects => "effects",
            Step::Cast => "cast",
            Step::Modifiers => "modifiers",
            Step::Power => "power",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Step::Effects => "Effects",
            Step::Cast => "Cast type",
            Step::Modifiers => "Modifiers",
            Step::Power => "Power",
        }
    }

    pub fn from_key(key: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// The line under the readout: what the last action did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Note {
    pub text: String,
    /// Done (green) or refused (red).
    pub ok: bool,
}

/// The Spellcrafter's state. One screen, one weaver at a time.
#[derive(Resource, Debug, Clone, Default)]
pub struct Crafter {
    pub open: bool,
    /// Whose spell it is: their skill sets the parts and the budget, their grimoire and bar take
    /// it.
    pub weaver: Option<Entity>,
    /// The altar it was opened at (its crystal previews the design).
    pub altar: Option<Entity>,
    pub design: SpellDesign,
    pub step: Step,
    pub note: Option<Note>,
    /// The complexity a refused part would have made; the budget pips show it in red.
    pub refused: Option<u32>,
    /// The grimoire spell the design was learned as, until it changes.
    pub learned: Option<usize>,
    /// Seconds to the crystal's next preview.
    pub preview_in: f32,
    /// Preview casts so far.
    pub previews: u32,
    /// Still in the pause from opening to the first cast: changes don't hurry it, so the
    /// first preview shows a spell the weaver has had a moment to choose.
    pub warmup: bool,
    /// Open at the next update: weaver and altar (an `Interact` at an altar, `magic.crafter`).
    pub request: Option<(Entity, Option<Entity>)>,
    /// `ui.events` are read up to here.
    pub seq: u64,
    /// The layout on screen now.
    pub shown: Option<Value>,
}

/// An altar that opens the Spellcrafter. Put it with an `Interactable` (the verb is the
/// prompt) and a [`MagicCaster`] (its crystal: `hand` is where the crystal is).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Altar {
    /// The practice dummy the crystal casts at while the altar is in use.
    pub dummy: Option<Entity>,
    /// Where the dummy stands while the altar is in use (it waits under the floor otherwise).
    pub stand: DVec3,
}

/// A mesh or light that glows in the color of the spell being woven at `altar`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AltarGlow {
    pub altar: Entity,
    /// Emissive strength (meshes) or lumens (lights) at full glow.
    pub strength: f32,
}

/// The crystal's caster: endless mana, no learning, never a fizzle (the readout gives the
/// chances; the preview shows the spell itself), the weaver's skill copied in before each
/// preview.
pub fn crystal_caster(hand: Vec3) -> MagicCaster {
    MagicCaster {
        mana: 1.0e9,
        max_mana: 1.0e9,
        mana_regen: 1.0e9,
        learns: false,
        steady: true,
        hand,
        seed: 777,
        ..Default::default()
    }
}

// ---------------------------------------------------------------- the palette

/// One modifier on the screen: the value it starts at, and the range of its count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub key: &'static str,
    pub group: &'static str,
    pub default: Modifier,
    /// Smallest and largest count (split, pierce, chain...).
    pub counts: Option<(u32, u32)>,
    pub blurb: &'static str,
}

const fn part(
    key: &'static str,
    group: &'static str,
    default: Modifier,
    counts: Option<(u32, u32)>,
    blurb: &'static str,
) -> Part {
    Part {
        key,
        group,
        default,
        counts,
        blurb,
    }
}

/// The modifier groups, in screen order.
pub const GROUPS: [&str; 4] = ["Shape", "On impact", "Tune", "Conduct"];

/// The modifiers step's palette. Charge and Overcharge are the power step's.
pub const PALETTE: &[Part] = &[
    part(
        "split",
        "Shape",
        Modifier::Split { count: 3 },
        Some((2, 7)),
        "Several at once, in a fan.",
    ),
    part(
        "fork",
        "Shape",
        Modifier::Fork { count: 2 },
        Some((2, 5)),
        "Splits into more when it hits.",
    ),
    part(
        "chain",
        "Shape",
        Modifier::Chain { jumps: 3 },
        Some((1, 8)),
        "Jumps on to further targets.",
    ),
    part(
        "pierce",
        "Shape",
        Modifier::Pierce { count: 3 },
        Some((1, 6)),
        "Passes through this many.",
    ),
    part(
        "bounce",
        "Shape",
        Modifier::Bounce { count: 2 },
        Some((1, 5)),
        "Ricochets off walls.",
    ),
    part(
        "boomerang",
        "Shape",
        Modifier::Boomerang,
        None,
        "Flies out and back, hitting on both passes.",
    ),
    part("seek", "Shape", Modifier::Seek, None, "Homes in on foes."),
    part(
        "rend",
        "Shape",
        Modifier::Rend,
        None,
        "Lodges in the target and stacks.",
    ),
    part(
        "blast",
        "On impact",
        Modifier::Blast { radius_m: 2.5 },
        None,
        "Explodes where it lands.",
    ),
    part(
        "linger",
        "On impact",
        Modifier::Linger {
            secs: 4.0,
            radius_m: 2.5,
        },
        None,
        "Leaves a field where it lands.",
    ),
    part(
        "stacking",
        "On impact",
        Modifier::Stacking { threshold: 5 },
        Some((2, 10)),
        "Each hit stacks; at the count the status bursts.",
    ),
    part(
        "echo",
        "On impact",
        Modifier::Echo,
        None,
        "Casts itself again a moment later.",
    ),
    part(
        "delay",
        "On impact",
        Modifier::Delay { secs: 1.0 },
        None,
        "Waits a fuse before it goes.",
    ),
    part(
        "speed",
        "Tune",
        Modifier::Speed { factor: 1.5 },
        None,
        "Flies half again as fast.",
    ),
    part(
        "range",
        "Tune",
        Modifier::Range { factor: 1.5 },
        None,
        "Reaches half again as far.",
    ),
    part(
        "size",
        "Tune",
        Modifier::Size { factor: 1.6 },
        None,
        "Bigger missile or area.",
    ),
    part(
        "duration",
        "Tune",
        Modifier::Duration { factor: 2.0 },
        None,
        "Lasts twice as long.",
    ),
    part(
        "safeguard",
        "Conduct",
        Modifier::Safeguard,
        None,
        "Never harms you or your friends.",
    ),
    part(
        "silent",
        "Conduct",
        Modifier::Silent,
        None,
        "No sound or light when cast.",
    ),
    part(
        "subdue",
        "Conduct",
        Modifier::Subdue,
        None,
        "Knocks out instead of killing (clean spells).",
    ),
    part(
        "lifesteal",
        "Conduct",
        Modifier::Lifesteal { share: 0.3 },
        None,
        "A third of the harm heals you (dark spells).",
    ),
    part(
        "blood_price",
        "Conduct",
        Modifier::BloodPrice,
        None,
        "Paid in health, not mana (dark spells).",
    ),
];

/// The power step's two.
pub const CHARGE: Part = part(
    "charge",
    "Power",
    Modifier::Charge { max_secs: 1.0 },
    None,
    "Hold the key to grow it, up to double power at a full charge.",
);
pub const OVERCHARGE: Part = part(
    "overcharge",
    "Power",
    Modifier::Overcharge,
    None,
    "Go past the budget. The farther past, the likelier it backfires.",
);

/// A palette part by key (the modifiers' and the power step's).
pub fn part_of(key: &str) -> Option<&'static Part> {
    PALETTE
        .iter()
        .chain([&CHARGE, &OVERCHARGE])
        .find(|p| p.key == key)
}

fn same_kind(a: &Modifier, b: &Modifier) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// A modifier's count, for the ones that have one.
pub fn count_of(m: &Modifier) -> Option<u32> {
    match *m {
        Modifier::Split { count }
        | Modifier::Fork { count }
        | Modifier::Pierce { count }
        | Modifier::Bounce { count } => Some(count),
        Modifier::Chain { jumps } => Some(jumps),
        Modifier::Stacking { threshold } => Some(threshold),
        _ => None,
    }
}

fn with_count(m: Modifier, n: u32) -> Modifier {
    match m {
        Modifier::Split { .. } => Modifier::Split { count: n },
        Modifier::Fork { .. } => Modifier::Fork { count: n },
        Modifier::Pierce { .. } => Modifier::Pierce { count: n },
        Modifier::Bounce { .. } => Modifier::Bounce { count: n },
        Modifier::Chain { .. } => Modifier::Chain { jumps: n },
        Modifier::Stacking { .. } => Modifier::Stacking { threshold: n },
        m => m,
    }
}

/// The design's modifier of this part's kind.
pub fn find(design: &SpellDesign, p: &Part) -> Option<Modifier> {
    design
        .modifiers
        .iter()
        .find(|m| same_kind(m, &p.default))
        .copied()
}

/// The palette parts this design may carry (no lifesteal on a clean spell, no subdue on a dark
/// one).
pub fn fits_design(p: &Part, design: &SpellDesign) -> bool {
    let dark = design.is_dark();
    !(p.default.dark_only() && !dark || p.default.clean_only() && dark)
}

/// The cast types on the screen.
pub fn cast_types() -> impl Iterator<Item = CastType> {
    CastType::ALL.into_iter().filter(|c| c.runs())
}

// ---------------------------------------------------------------- rules

/// The subclasses whose effects a caster may weave: Apprentice or better, and always their
/// best one (a Novice still has one effect to cast).
pub fn open_subclasses(caster: &MagicCaster) -> Vec<Subclass> {
    let best = Subclass::ALL
        .into_iter()
        .max_by_key(|s| (caster.mastery(*s), std::cmp::Reverse(*s as u8)))
        .unwrap_or(Subclass::Evocation);
    Subclass::ALL
        .into_iter()
        .filter(|s| *s == best || caster.mastery(*s) >= Mastery::Apprentice)
        .collect()
}

/// What the screen starts with: a plain bolt of the first effect of the caster's best subclass.
pub fn starting_design(caster: &MagicCaster) -> SpellDesign {
    let first = open_subclasses(caster)
        .into_iter()
        .max_by_key(|s| (caster.mastery(*s), std::cmp::Reverse(*s as u8)))
        .and_then(|s| EFFECTS.iter().find(|e| e.subclass == s))
        .map(|e| e.key)
        .unwrap_or("fire");
    SpellDesign::new(&[first], CastType::Bolt)
}

/// "an Expert", "a Master".
pub fn with_article(m: Mastery) -> String {
    let name = m.name();
    let an = name.starts_with(['A', 'E', 'I', 'O', 'U']);
    format!("{} {name}", if an { "an" } else { "a" })
}

/// The design's complexity, the budget of the mastery it draws on, and that mastery.
pub fn budget_of(design: &SpellDesign, caster: &MagicCaster) -> (u32, u32, Mastery) {
    let m = caster.mastery(MagicCaster::subclass_of(design));
    (design.complexity(), m.budget(), m)
}

/// Complexity row by row: effects, cast type, modifiers, power. They add up to the design's.
pub fn rows(design: &SpellDesign) -> [u32; 4] {
    let dark = design.is_dark();
    let mut distinct = design.effects.clone();
    distinct.sort();
    distinct.dedup();
    let effects = distinct.len().saturating_sub(1) as u32;
    let cast = design.cast.area_like() as u32;
    let power: u32 = design
        .modifiers
        .iter()
        .filter(|m| matches!(m, Modifier::Charge { .. } | Modifier::Overcharge))
        .map(|m| m.complexity(dark))
        .sum();
    let total = design.complexity();
    [
        effects,
        cast,
        total.saturating_sub(effects + cast + power),
        power,
    ]
}

/// The spell's name from its weave and form: "Frost glaive", "Steam flask", "Fire volley",
/// "Magma golem".
pub fn auto_name(design: &SpellDesign, spell: Option<&Spell>) -> String {
    let Some(w) = weave(&design.effects) else {
        return "Unwoven".into();
    };
    let split = design
        .modifiers
        .iter()
        .filter_map(|m| match m {
            Modifier::Split { count } => Some(*count),
            _ => None,
        })
        .max()
        .unwrap_or(1);
    let many = split > 1;
    let boomerang = design.has(|m| matches!(m, Modifier::Boomerang));
    let elements = w
        .elements
        .first()
        .map(|e| e.name())
        .unwrap_or(w.name.as_str());
    let noun = match design.cast {
        CastType::Summon => {
            return match spell.and_then(kind_of) {
                Some(MinionKind::Skeleton) => "Skeleton warriors".into(),
                Some(MinionKind::SkeletalMage) => format!("{elements} skeletal mage"),
                Some(MinionKind::Golem) if design.effects.iter().any(|k| k == "raise") => {
                    "Bone golem".into()
                }
                Some(MinionKind::Golem) if w.elements.is_empty() => "Stone golem".into(),
                Some(MinionKind::Golem) => format!("{} golem", w.name),
                Some(MinionKind::Elemental) => format!("{} elemental", w.name),
                Some(MinionKind::Wisp) => format!("{} wisp", w.name),
                Some(MinionKind::Imp) => "Imps".into(),
                Some(MinionKind::Hellhound) => "Hellhounds".into(),
                None => format!("{} familiar", w.name),
            };
        }
        _ if boomerang => {
            if many {
                "glaives"
            } else {
                "glaive"
            }
        }
        CastType::Bolt | CastType::Lob if many => "volley",
        CastType::Touch => "touch",
        CastType::Imbue => "blade",
        CastType::Bolt => "bolt",
        CastType::Lob => "flask",
        CastType::Beam => "beam",
        CastType::Cone => "spray",
        CastType::Wave => "wave",
        CastType::Wall => "wall",
        CastType::Area => "field",
        CastType::Aura => "aura",
        CastType::Rune => "rune",
        CastType::Orbit => "orbs",
        CastType::Tether => "tether",
        CastType::Chain => "chain",
        CastType::Rain => "rain",
        CastType::OnSelf => "",
    };
    // The Shaping list names its own forms ("Pillar").
    let lower = w.name.to_lowercase();
    if noun.is_empty() || lower.ends_with(noun) || w.name == "Pillar" {
        return w.name.clone();
    }
    format!("{} {noun}", w.name)
}

/// The two designs are the same spell (names aside).
pub fn same_parts(a: &SpellDesign, b: &SpellDesign) -> bool {
    let set = |d: &SpellDesign| {
        let mut v = d.effects.clone();
        v.sort();
        v.dedup();
        v
    };
    set(a) == set(b) && a.cast == b.cast && a.modifiers == b.modifiers && a.stages == b.stages
}

/// The preview can't show it from the crystal: it moves or works on its caster.
pub fn preview_blocked(design: &SpellDesign) -> Option<&'static str> {
    if design
        .effects
        .iter()
        .filter_map(|k| effect(k))
        .any(|e| matches!(e.kind, EffectKind::Blink { .. }))
    {
        return Some("A blink moves its caster: try it from the bar.");
    }
    if design.cast == CastType::OnSelf {
        return Some("A spell on yourself shows on you: try it from the bar.");
    }
    None
}

impl Crafter {
    fn refuse(&mut self, text: String, would: Option<u32>) {
        self.note = Some(Note { text, ok: false });
        self.refused = would;
    }

    fn done(&mut self, text: String) {
        self.note = Some(Note { text, ok: true });
        self.refused = None;
    }

    /// Take `next` if the caster can hold it; refuse in words otherwise. `what` names the part.
    fn change(&mut self, caster: &MagicCaster, mut next: SpellDesign, what: &str) -> bool {
        // Parts that no longer fit come off by themselves (subdue once blood is woven in).
        let mut dropped = Vec::new();
        let dark = is_dark(&next.effects);
        next.modifiers.retain(|m| {
            let ok = !(m.dark_only() && !dark || m.clean_only() && dark);
            if !ok {
                dropped.push(m.name());
            }
            ok
        });
        let (now, ..) = budget_of(&self.design, caster);
        let (cx, budget, m) = budget_of(&next, caster);
        let over = next.has(|m| matches!(m, Modifier::Overcharge));
        if cx > budget && !over && cx > now {
            self.refuse(
                format!(
                    "{what} would make it {cx}, past {}'s {budget}. Take a part off, or turn on Overcharge to risk it.",
                    with_article(m)
                ),
                Some(cx),
            );
            return false;
        }
        if let Err(e) = caster.compile(&next, 0.0)
            && !(cx > budget && !over)
        {
            self.refuse(e.to_string(), None);
            return false;
        }
        self.design = next;
        self.learned = None;
        if !self.warmup {
            self.preview_in = self.preview_in.min(PREVIEW_AFTER_CHANGE);
        }
        if dropped.is_empty() {
            self.note = None;
            self.refused = None;
        } else {
            self.done(format!(
                "{} came off: it no longer fits.",
                dropped.join(", ")
            ));
        }
        true
    }
}

fn is_dark(effects: &[String]) -> bool {
    effects
        .iter()
        .filter_map(|k| effect(k))
        .any(|e| e.is_dark())
}

/// Learn the design into the caster's grimoire (once: the same parts again are the same
/// spell), named from its weave. Returns the grimoire index.
pub fn learn(design: &SpellDesign, caster: &mut MagicCaster) -> Result<usize, String> {
    let spell = caster.compile(design, 0.0).map_err(|e| e.to_string())?;
    if let Some(i) = caster
        .grimoire
        .iter()
        .position(|k| same_parts(&k.design, design))
    {
        return Ok(i);
    }
    let base = auto_name(design, Some(&spell));
    let mut name = base.clone();
    let mut n = 1;
    while caster.grimoire.iter().any(|k| k.design.name == name) {
        n += 1;
        name = format!("{base} {}", roman(n));
    }
    let mut d = design.clone();
    d.name = name;
    caster.grimoire.push(KnownSpell {
        design: d,
        growth: None,
    });
    Ok(caster.grimoire.len() - 1)
}

fn roman(n: usize) -> &'static str {
    ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX"]
        .get(n)
        .copied()
        .unwrap_or("X")
}

/// Run one action (`magic.craft.mod:seek`, or without the prefix). Returns whether the screen
/// should close.
pub fn apply(
    c: &mut Crafter,
    caster: &mut MagicCaster,
    bar: Option<&mut SpellBar>,
    action: &str,
) -> bool {
    let action = action.strip_prefix(ACTION).unwrap_or(action);
    let (verb, arg) = action.split_once(':').unwrap_or((action, ""));
    match verb {
        "close" => return true,
        "step" => {
            if let Some(s) = Step::from_key(arg) {
                c.step = s;
            }
        }
        "clear" => {
            let d = starting_design(caster);
            c.change(caster, d, "Starting over");
            c.step = Step::Effects;
        }
        "effect" => {
            let Some(def) = effect(arg) else {
                c.refuse(format!("No effect named \"{arg}\"."), None);
                return false;
            };
            let mut next = c.design.clone();
            if let Some(i) = next.effects.iter().position(|k| k == arg) {
                if next.effects.len() == 1 {
                    c.refuse(
                        "A spell needs one effect: pick another before taking this off.".into(),
                        None,
                    );
                    return false;
                }
                next.effects.remove(i);
                c.change(caster, next, def.name);
            } else {
                if !open_subclasses(caster).contains(&def.subclass) {
                    c.refuse(
                        format!("{} needs Apprentice in {}.", def.name, def.subclass.name()),
                        None,
                    );
                    return false;
                }
                if next.effects.len() >= MAX_EFFECTS {
                    c.refuse(
                        format!(
                            "Three effects at most: take one off before adding {}.",
                            def.name
                        ),
                        None,
                    );
                    return false;
                }
                next.effects.push(arg.to_string());
                c.change(caster, next, def.name);
            }
        }
        "cast" => {
            let Some(cast) = cast_types().find(|t| t.name().eq_ignore_ascii_case(arg)) else {
                c.refuse(format!("No cast type named \"{arg}\"."), None);
                return false;
            };
            let mut next = c.design.clone();
            next.cast = cast;
            c.change(caster, next, cast.name());
        }
        "mod" | "charge" | "overcharge" => {
            let key = if verb == "mod" { arg } else { verb };
            let Some(p) = part_of(key) else {
                c.refuse(format!("No modifier named \"{key}\"."), None);
                return false;
            };
            let mut next = c.design.clone();
            if let Some(i) = next.modifiers.iter().position(|m| same_kind(m, &p.default)) {
                next.modifiers.remove(i);
                if p.key == "overcharge" {
                    let (cx, budget, m) = budget_of(&next, caster);
                    if cx > budget {
                        c.refuse(
                            format!(
                                "At {cx}, past {}'s {budget}, it needs Overcharge: take parts off first.",
                                with_article(m)
                            ),
                            None,
                        );
                        return false;
                    }
                }
            } else {
                if !fits_design(p, &next) {
                    c.refuse(
                        format!(
                            "{} works only on {} spells.",
                            p.default.name(),
                            if p.default.dark_only() {
                                "dark"
                            } else {
                                "clean"
                            }
                        ),
                        None,
                    );
                    return false;
                }
                next.modifiers.push(p.default);
            }
            c.change(caster, next, p.default.name());
        }
        "more" | "less" => {
            let Some(p) = part_of(arg) else {
                return false;
            };
            let Some((lo, hi)) = p.counts else {
                return false;
            };
            let mut next = c.design.clone();
            let Some(m) = next.modifiers.iter_mut().find(|m| same_kind(m, &p.default)) else {
                return false;
            };
            let n = count_of(m).unwrap_or(lo);
            let n2 = if verb == "more" {
                n + 1
            } else {
                n.saturating_sub(1)
            };
            if !(lo..=hi).contains(&n2) {
                c.refuse(
                    format!("{} goes from {lo} to {hi}.", p.default.name()),
                    None,
                );
                return false;
            }
            *m = with_count(*m, n2);
            c.change(caster, next, &format!("{} {n2}", p.default.name()));
        }
        "learn" => match learn(&c.design, caster) {
            Ok(i) => {
                c.learned = Some(i);
                c.done(format!(
                    "Learned: {} is in your grimoire.",
                    caster.grimoire[i].design.name
                ));
            }
            Err(e) => c.refuse(e, None),
        },
        "key" => {
            let slot = arg.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
            let Some(slot) = slot.filter(|s| *s < SLOTS) else {
                c.refuse(format!("Keys go from 1 to {SLOTS}."), None);
                return false;
            };
            let Some(bar) = bar else {
                c.refuse("No spell bar to put it on.".into(), None);
                return false;
            };
            match learn(&c.design, caster) {
                Ok(i) => {
                    c.learned = Some(i);
                    if bar.slots.len() < SLOTS {
                        bar.slots.resize(SLOTS, None);
                    }
                    bar.slots[slot] = Some(i);
                    if let Some(cd) = bar.cooldown.get_mut(slot) {
                        *cd = 0.0;
                    }
                    c.done(format!(
                        "{} is on key {}.",
                        caster.grimoire[i].design.name,
                        slot + 1
                    ));
                }
                Err(e) => c.refuse(e, None),
            }
        }
        _ => c.refuse(format!("The Spellcrafter has no \"{action}\"."), None),
    }
    false
}

// ---------------------------------------------------------------- the screen

/// The node id of the button that sends `action` (`magic.craft.mod:seek` is `craft_mod_seek`).
pub fn id_of(action: &str) -> String {
    let a = action.strip_prefix(ACTION).unwrap_or(action);
    format!("craft_{}", a.replace([':', '.'], "_"))
}

/// The chosen card: an opaque dark gold fill with a gold edge. The theme's own selected card is
/// see-through over its glow, which read as a solid gold slab with the caption lost in it.
fn selected_card() -> Value {
    json!({"background": "#33291a", "border_color": "#e8b65acc"})
}

fn act(a: &str) -> String {
    format!("{ACTION}{a}")
}

/// A linear color as the UI's sRGB hex, brightened so a dark weave still reads on the panel.
pub fn hex(c: [f32; 3], lift: f32) -> String {
    let max = c[0].max(c[1]).max(c[2]).max(1e-4);
    let ch = |x: f32| {
        let v = (x / max).clamp(0.0, 1.0).powf(1.0 / 2.2);
        let v = v + (1.0 - v) * lift;
        (v * 255.0).round() as u8
    };
    format!("#{:02x}{:02x}{:02x}", ch(c[0]), ch(c[1]), ch(c[2]))
}

const GOOD: &str = "#7ee08a";
const BAD: &str = "#ff6b6b";

fn button(action: &str, text: &str, selected: bool, extra: Value) -> Value {
    let mut b = json!({
        "id": id_of(action),
        "class": "button",
        "text": text,
        "on_click": act(action),
        "selected": selected,
        "style": {"padding": [7, 12], "font_size": 14},
    });
    if let (Some(o), Some(e)) = (b.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            if k == "style" {
                if let (Some(s), Some(v)) = (
                    o.get_mut("style").and_then(Value::as_object_mut),
                    v.as_object(),
                ) {
                    for (sk, sv) in v {
                        s.insert(sk.clone(), sv.clone());
                    }
                }
            } else {
                o.insert(k.clone(), v.clone());
            }
        }
    }
    b
}

fn caption(text: &str) -> Value {
    json!({"class": "caption", "text": text.to_uppercase()})
}

fn cost_tag(n: u32, over: bool) -> String {
    let color = if over { BAD } else { "#e8c070" };
    format!("[color={color}]+{n}[/color]")
}

/// The four recipe rows, each with its complexity on the right.
fn recipe(c: &Crafter) -> Value {
    let d = &c.design;
    let r = rows(d);
    let fx: Vec<&str> = d
        .effects
        .iter()
        .map(|k| effect(k).map(|e| e.name).unwrap_or(k))
        .collect();
    let mods: Vec<String> = d
        .modifiers
        .iter()
        .filter(|m| !matches!(m, Modifier::Charge { .. } | Modifier::Overcharge))
        .map(|m| match count_of(m) {
            Some(n) => format!("{} {n}", m.name()),
            None => m.name().to_string(),
        })
        .collect();
    let power: Vec<&str> = d
        .modifiers
        .iter()
        .filter(|m| matches!(m, Modifier::Charge { .. } | Modifier::Overcharge))
        .map(|m| m.name())
        .collect();
    let values = [
        fx.join(" + "),
        d.cast.name().to_string(),
        if mods.is_empty() {
            "none".into()
        } else {
            mods.join(", ")
        },
        if power.is_empty() {
            "plain".into()
        } else {
            power.join(", ")
        },
    ];
    let cards: Vec<Value> = Step::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "id": id_of(&format!("step:{}", s.key())),
                "class": "card",
                "selected_style": selected_card(),
                "on_click": act(&format!("step:{}", s.key())),
                "selected": c.step == *s,
                "style": {"direction": "row", "align_items": "center", "padding": [9, 12], "gap": 12},
                "children": [
                    {"class": "kbd", "text": (i + 1).to_string()},
                    {"class": "col", "style": {"gap": 2, "grow": 1, "shrink": 1}, "children": [
                        caption(s.title()),
                        {"text": values[i], "text_style": {"size": 16, "weight": 600, "wrap": true}},
                    ]},
                    {"class": "chip", "text": format!("+{}", r[i]), "style": {"font_size": 14}},
                ],
            })
        })
        .collect();
    json!({"class": "col", "style": {"gap": 6}, "children": cards})
}

fn effects_picker(c: &Crafter, caster: &MagicCaster) -> Value {
    let open = open_subclasses(caster);
    let groups: Vec<Value> = open
        .iter()
        .map(|s| {
            let buttons: Vec<Value> = EFFECTS
                .iter()
                .filter(|e| e.subclass == *s)
                .map(|e| {
                    let on = c.design.effects.iter().any(|k| k == e.key);
                    button(
                        &format!("effect:{}", e.key),
                        e.name,
                        on,
                        json!({"tooltip": {"title": e.name, "text": e.description}}),
                    )
                })
                .collect();
            json!({"class": "col", "style": {"gap": 5}, "children": [
                caption(&format!("{} · {}", s.name(), caster.mastery(*s).name())),
                {"class": "row", "style": {"wrap": true, "gap": 6}, "children": buttons},
            ]})
        })
        .collect();
    json!({"class": "col", "style": {"gap": 8}, "children": [
        {"class": "body", "text": "Up to three. Together they weave into one spell: fire and frost make steam."},
        {"style": {"display": "grid", "grid_columns": 2, "gap": 10}, "children": groups},
    ]})
}

fn cast_picker(c: &Crafter, caster: &MagicCaster) -> Value {
    let (now, budget, _) = budget_of(&c.design, caster);
    let over_ok = c.design.has(|m| matches!(m, Modifier::Overcharge));
    let buttons: Vec<Value> = cast_types()
        .map(|t| {
            let extra = t.area_like() as u32;
            let mut d = c.design.clone();
            d.cast = t;
            let over = d.complexity() > budget && d.complexity() > now && !over_ok;
            let text = if extra > 0 {
                format!("{}  {}", t.name(), cost_tag(extra, over))
            } else {
                t.name().to_string()
            };
            button(
                &format!("cast:{}", t.name().to_lowercase()),
                &text,
                c.design.cast == t,
                if over {
                    json!({"style": {"opacity": 0.45}})
                } else {
                    json!({})
                },
            )
        })
        .collect();
    json!({"class": "col", "style": {"gap": 8}, "children": [
        {"class": "body", "text": "How it leaves your hand. Areas cost one more."},
        {"style": {"display": "grid", "grid_columns": 4, "gap": 6}, "children": buttons},
    ]})
}

fn modifier_chip(c: &Crafter, caster: &MagicCaster, p: &Part) -> Value {
    let (now, budget, _) = budget_of(&c.design, caster);
    let over_ok = c.design.has(|m| matches!(m, Modifier::Overcharge));
    let dark = c.design.is_dark();
    let have = find(&c.design, p);
    let cost = have.unwrap_or(p.default).complexity(dark);
    let mut d = c.design.clone();
    if have.is_none() {
        d.modifiers.push(p.default);
    }
    let over = have.is_none() && d.complexity() > budget && d.complexity() > now && !over_ok;
    let label = match have.and_then(|m| count_of(&m)).or(count_of(&p.default)) {
        Some(n) if have.is_some() => format!("{} {n}", p.default.name()),
        _ => p.default.name().to_string(),
    };
    let text = format!("{label}  {}", cost_tag(cost, over));
    let mut extra = json!({"tooltip": {"title": p.default.name(), "text": p.blurb}});
    if over {
        extra["style"] = json!({"opacity": 0.45, "border_color": "#ff6b6b80"});
    }
    let chip = button(&format!("mod:{}", p.key), &text, have.is_some(), extra);
    if have.is_none() || p.counts.is_none() {
        return chip;
    }
    // A count steps with − and +.
    let step = |verb: &str, glyph: &str| {
        json!({
            "id": id_of(&format!("{verb}:{}", p.key)),
            "class": "button",
            "text": glyph,
            "on_click": act(&format!("{verb}:{}", p.key)),
            "style": {"padding": [7, 9], "font_size": 15},
        })
    };
    json!({"class": "row", "style": {"gap": 3}, "children": [step("less", "−"), chip, step("more", "+")]})
}

fn modifiers_picker(c: &Crafter, caster: &MagicCaster) -> Value {
    let groups: Vec<Value> = GROUPS
        .iter()
        .map(|g| {
            let chips: Vec<Value> = PALETTE
                .iter()
                .filter(|p| p.group == *g && fits_design(p, &c.design))
                .map(|p| modifier_chip(c, caster, p))
                .collect();
            json!({"class": "col", "style": {"gap": 5}, "children": [
                caption(g),
                {"class": "row", "style": {"wrap": true, "gap": 6}, "children": chips},
            ]})
        })
        .collect();
    json!({"class": "col", "style": {"gap": 9}, "children": groups})
}

fn power_picker(c: &Crafter, caster: &MagicCaster) -> Value {
    let cards: Vec<Value> = [&CHARGE, &OVERCHARGE]
        .into_iter()
        .map(|p| {
            let on = find(&c.design, p).is_some();
            json!({
                "id": id_of(&format!("mod:{}", p.key)),
                "class": "card",
                "selected_style": selected_card(),
                "on_click": act(&format!("mod:{}", p.key)),
                "selected": on,
                "style": {"direction": "column", "gap": 4, "grow": 1, "basis": 0},
                "children": [
                    {"class": "row", "children": [
                        {"text": p.default.name(), "text_style": {"size": 17, "weight": 700}},
                        {"class": "spacer"},
                        {"class": "chip", "text": if on { "On" } else { "Off" }},
                    ]},
                    {"class": "body", "text": p.blurb, "style": {"font_size": 13}},
                    caption(&format!("+{}", p.default.complexity(c.design.is_dark()))),
                ],
            })
        })
        .collect();
    let p = caster.power();
    json!({"class": "col", "style": {"gap": 8}, "children": [
        {"class": "body", "text": format!(
            "Your level sets its energy: {} a cast at level {:.0}.",
            joules(p.energy_j),
            caster.level
        )},
        {"class": "row", "style": {"gap": 10, "align_items": "stretch"}, "children": cards},
    ]})
}

fn joules(j: f64) -> String {
    match j {
        j if j >= 1e9 => format!("{:.1} GJ", j / 1e9),
        j if j >= 1e6 => format!("{:.1} MJ", j / 1e6),
        j if j >= 1e3 => format!("{:.1} kJ", j / 1e3),
        j => format!("{j:.0} J"),
    }
}

/// The budget as pips, one per point: spent in the accent, past the budget red, and the points
/// a refused part would have taken outlined in red.
fn pips(cx: u32, budget: u32, refused: Option<u32>) -> Value {
    let n = budget.max(cx).max(refused.unwrap_or(0)).max(1);
    let pips: Vec<Value> = (1..=n)
        .map(|i| {
            let (bg, border) = if i <= cx && i <= budget {
                ("#e8b65a", "#ffd88a")
            } else if i <= cx {
                (BAD, "#ff9a9a")
            } else if refused.is_some_and(|r| i <= r) {
                ("#ff6b6b33", BAD)
            } else {
                ("#ffffff10", "#ffffff30")
            };
            let mut s = json!({"grow": 1, "basis": 0, "height": 14, "radius": 4,
                "background": bg, "border_width": 1.5, "border_color": border});
            if i == budget && n > budget {
                // The budget's edge.
                s["margin"] = json!([0, 6, 0, 0]);
            }
            json!({"style": s})
        })
        .collect();
    json!({"class": "row", "style": {"gap": 4}, "children": pips})
}

fn stats(spell: &Spell) -> Value {
    let mut chips = vec![
        format!("Mana {:.0}", spell.cost.mana),
        format!("Cast {:.2} s", spell.cast_secs),
        format!("Fizzle {:.0}%", spell.fizzle * 100.0),
        format!("Backfire {:.0}%", spell.backfire * 100.0),
    ];
    if spell.cost.health > 0.0 {
        chips.insert(1, format!("Health {:.0}", spell.cost.health));
    }
    if spell.dark {
        chips.push(format!("Corruption +{:.1}", spell.cost.corruption));
    }
    let chips: Vec<Value> = chips
        .into_iter()
        .map(|t| json!({"class": "chip", "text": t, "style": {"font_size": 13}}))
        .collect();
    json!({"class": "row", "style": {"wrap": true, "gap": 6}, "children": chips})
}

/// The whole screen, as the UI module's element JSON.
pub fn layout(c: &Crafter, caster: &MagicCaster, bar: Option<&SpellBar>) -> Value {
    let compiled = caster.compile(&c.design, 0.0);
    let spell = compiled.as_ref().ok();
    let w = weave(&c.design.effects);
    let color = w.as_ref().map(|w| w.color).unwrap_or([0.8, 0.8, 0.8]);
    let name = match c.learned.and_then(|i| caster.grimoire.get(i)) {
        Some(k) => k.design.name.clone(),
        None => auto_name(&c.design, spell.map(|s| s.as_ref())),
    };
    let description = w
        .as_ref()
        .map(|w| w.description.clone())
        .unwrap_or_default();
    let sub = MagicCaster::subclass_of(&c.design);
    let (cx, budget, mastery) = budget_of(&c.design, caster);
    let picker = match c.step {
        Step::Effects => effects_picker(c, caster),
        Step::Cast => cast_picker(c, caster),
        Step::Modifiers => modifiers_picker(c, caster),
        Step::Power => power_picker(c, caster),
    };
    let header = json!({"class": "row", "style": {"gap": 14, "align_items": "center"}, "children": [
        {"style": {"width": 34, "height": 34, "radius": 17, "shrink": 0,
            "background": {"radial": {"inner": hex(color, 0.55), "outer": hex(color, 0.0)}},
            "border_width": 1.5, "border_color": hex(color, 0.35),
            "shadow": {"color": format!("{}aa", hex(color, 0.2)), "blur": 18}}},
        {"class": "col", "style": {"gap": 2, "grow": 1, "shrink": 1}, "children": [
            caption(&format!("Spellcrafter · {} · {}", sub.name(), mastery.name())),
            {"id": "craft_name", "class": "title", "text": name, "style": {"color": hex(color, 0.25)}},
            {"class": "body", "text": description, "text_style": {"size": 14, "wrap": true}},
        ]},
    ]});
    let over = cx > budget;
    let readout_line = format!(
        "Complexity [b]{cx}[/b] of {}'s {budget}",
        with_article(mastery)
    );
    let mut readout = vec![
        json!({"class": "row", "children": [
            {"id": "craft_complexity", "class": "heading", "text": readout_line,
                "style": {"color": if over { BAD } else { "#f2efe6" }}},
            {"class": "spacer"},
            caption(if c.design.has(|m| matches!(m, Modifier::Overcharge)) { "Overcharged" } else { "Budget" }),
        ]}),
        pips(cx, budget, c.refused),
    ];
    match &compiled {
        Ok(s) => readout.push(stats(s)),
        Err(e) => {
            readout.push(json!({"class": "body", "text": e.to_string(), "style": {"color": BAD}}))
        }
    }
    let preview = match preview_blocked(&c.design) {
        Some(why) => why.to_string(),
        None if c.altar.is_some() => {
            format!("The crystal casts it at the dummy every {PREVIEW_EVERY} s.")
        }
        None => String::new(),
    };
    if !preview.is_empty() {
        readout.push(json!({"class": "caption", "text": preview, "style": {"font_weight": 600}}));
    }
    if let Some(n) = &c.note {
        readout.push(json!({"id": "craft_note", "text": n.text, "text_style": {
            "size": 15, "weight": 700, "wrap": true, "color": if n.ok { GOOD } else { BAD }}}));
    }
    // Keys: the bar's spell on each (tooltip), the one this design is on highlighted.
    let on_key = |slot: usize| -> bool {
        c.learned.is_some() && bar.and_then(|b| b.slots.get(slot).copied().flatten()) == c.learned
    };
    let keys: Vec<Value> = (0..SLOTS)
        .map(|i| {
            let now = bar
                .and_then(|b| b.slots.get(i).copied().flatten())
                .and_then(|g| caster.grimoire.get(g))
                .map(|k| k.design.name.clone())
                .unwrap_or_else(|| "empty".into());
            json!({
                "id": id_of(&format!("key:{}", i + 1)),
                "class": "slot",
                "text": (i + 1).to_string(),
                "on_click": act(&format!("key:{}", i + 1)),
                "selected": on_key(i),
                "tooltip": {"title": format!("Key {}", i + 1), "text": now},
                "style": {"width": 38, "height": 38, "font_size": 16, "font_weight": 700},
            })
        })
        .collect();
    let can_learn = spell.is_some();
    let actions = json!({"class": "col", "style": {"gap": 8}, "children": [
        {"class": "row", "style": {"gap": 10}, "children": [
            {"id": id_of("learn"), "class": "button primary", "text": if c.learned.is_some() { "Learned" } else { "Learn" },
                "on_click": act("learn"), "disabled": !can_learn, "selected": c.learned.is_some()},
            {"id": id_of("clear"), "class": "button ghost", "text": "Start over", "on_click": act("clear")},
            {"class": "spacer"},
            {"id": id_of("close"), "class": "button ghost", "on_click": act("close"), "children": [
                {"text": "Close"}, {"class": "kbd", "text": "Esc"}]},
        ]},
        {"class": "row", "style": {"gap": 6}, "children":
            std::iter::once(caption("Put on key")).chain(keys).collect::<Vec<_>>()},
    ]});
    json!({
        "id": "craft_panel",
        "class": "panel",
        "style": {"position": "absolute", "left": 28, "top": 28, "width": 700, "gap": 14, "padding": 22},
        "children": [
            header,
            {"class": "divider"},
            recipe(c),
            {"class": "glass", "style": {"padding": 14, "direction": "column"}, "children": [picker]},
            {"class": "divider"},
            {"class": "col", "style": {"gap": 8}, "children": readout},
            actions,
        ],
    })
}

/// Machine-readable state, for `magic.crafter`.
pub fn state(c: &Crafter, caster: Option<&MagicCaster>) -> Value {
    let Some(caster) = caster else {
        return json!({"open": c.open});
    };
    let spell = caster.compile(&c.design, 0.0);
    let (cx, budget, mastery) = budget_of(&c.design, caster);
    json!({
        "open": c.open,
        "weaver": c.weaver.map(|e| e.to_bits()),
        "altar": c.altar.map(|e| e.to_bits()),
        "step": c.step,
        "design": c.design,
        "name": auto_name(&c.design, spell.as_ref().ok().map(|s| s.as_ref())),
        "complexity": cx,
        "budget": budget,
        "mastery": mastery,
        "rows": rows(&c.design),
        "spell": spell.as_ref().ok().map(|s| json!({
            "mana": s.cost.mana, "health": s.cost.health, "corruption": s.cost.corruption,
            "cast_secs": s.cast_secs, "fizzle": s.fizzle, "backfire": s.backfire,
            "summary": s.summary(), "color": s.weave.color,
        })),
        "error": spell.as_ref().err().map(|e| e.to_string()),
        "note": c.note,
        "learned": c.learned,
        "previews": c.previews,
        "subclasses": open_subclasses(caster).iter().map(|s| s.name()).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------- systems

/// Ask the crafter open for `weaver` (at `altar`, whose crystal previews), on the next update.
pub fn request_open(world: &mut World, weaver: Entity, altar: Option<Entity>) {
    world.get_resource_or_insert_with(Crafter::default).request = Some((weaver, altar));
}

/// `Update`: E at an altar opens the crafter for whoever pressed it (or the player's caster).
pub fn open_at_altars(
    mut uses: MessageReader<Interact>,
    altars: Query<(), With<Altar>>,
    casters: Query<(), With<MagicCaster>>,
    players: Query<Entity, (With<HudBar>, With<MagicCaster>)>,
    mut crafter: ResMut<Crafter>,
) {
    for u in uses.read() {
        if !altars.contains(u.target) || crafter.open {
            continue;
        }
        let weaver = if casters.contains(u.actor) {
            Some(u.actor)
        } else {
            players.iter().next()
        };
        if let Some(w) = weaver {
            crafter.request = Some((w, Some(u.target)));
        }
    }
}

fn call(world: &mut World, api: &str, args: Value) -> Result<Value, String> {
    call_api(world, api, args)
}

fn open(world: &mut World, c: &mut Crafter, weaver: Entity, altar: Option<Entity>) {
    let Some(caster) = world.get::<MagicCaster>(weaver) else {
        return;
    };
    c.open = true;
    c.weaver = Some(weaver);
    c.altar = altar.filter(|a| world.get::<Altar>(*a).is_some());
    c.design = starting_design(caster);
    c.step = Step::Effects;
    c.note = None;
    c.refused = None;
    c.learned = None;
    c.preview_in = FIRST_PREVIEW;
    c.warmup = true;
    c.shown = None;
    // Only actions from now on.
    c.seq = call(world, "ui.events", json!({"since": u64::MAX}))
        .ok()
        .and_then(|v| v.get("next").and_then(Value::as_u64))
        .unwrap_or(0);
}

fn close(world: &mut World, c: &mut Crafter) {
    if c.shown.take().is_some() {
        let _ = call(world, "ui.hide", json!({"screen": SCREEN}));
    }
    c.open = false;
    c.request = None;
}

/// Run one action on the open crafter as if its button were pressed, with the weaver's caster
/// and bar. Returns false when the crafter is closed.
pub fn act_now(world: &mut World, action: &str) -> bool {
    let Some(mut c) = world.remove_resource::<Crafter>() else {
        return false;
    };
    let ok = c.open;
    if ok {
        run_action(world, &mut c, action);
    }
    world.insert_resource(c);
    ok
}

fn run_action(world: &mut World, c: &mut Crafter, action: &str) {
    let Some(w) = c.weaver else { return };
    let Some(mut caster) = world.get::<MagicCaster>(w).cloned() else {
        return;
    };
    let mut bar = world.get::<SpellBar>(w).cloned();
    let closes = apply(c, &mut caster, bar.as_mut(), action);
    if let Some(mut m) = world.get_mut::<MagicCaster>(w)
        && *m != caster
    {
        *m = caster;
    }
    if let Some(b) = bar
        && let Some(mut m) = world.get_mut::<SpellBar>(w)
        && *m != b
    {
        *m = b;
    }
    if closes {
        close(world, c);
    }
}

/// Press a crafter button as the player would: through the UI (`ui.click`) when its screen is
/// up, else straight to the crafter (tests, no UI).
pub fn press(world: &mut World, action: &str) {
    let id = id_of(action);
    if call(world, "ui.click", json!({"id": id})).is_err() {
        act_now(world, action);
    }
}

/// `Update` (exclusive): open on request, read the screen's actions, redraw it when it
/// changed, keep the cursor free, and let the crystal cast the design.
pub fn run_crafter(world: &mut World) {
    let Some(mut c) = world.remove_resource::<Crafter>() else {
        return;
    };
    if let Some((weaver, altar)) = c.request.take() {
        if c.open {
            close(world, &mut c);
        }
        open(world, &mut c, weaver, altar);
    }
    if c.open {
        tick_open(world, &mut c);
    }
    world.insert_resource(c);
}

fn tick_open(world: &mut World, c: &mut Crafter) {
    let Some(w) = c.weaver.filter(|w| world.get::<MagicCaster>(*w).is_some()) else {
        close(world, c);
        return;
    };
    // The screen's buttons, and Esc (the UI closes the screen).
    if let Ok(v) = call(world, "ui.events", json!({"since": c.seq})) {
        c.seq = v.get("next").and_then(Value::as_u64).unwrap_or(c.seq);
        let events = v
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for e in events {
            match e.get("type").and_then(Value::as_str) {
                Some("action") => {
                    if let Some(a) = e.get("action").and_then(Value::as_str)
                        && a.starts_with(ACTION)
                    {
                        run_action(world, c, a);
                        if !c.open {
                            return;
                        }
                    }
                }
                Some("screen_closed")
                    if e.get("screen").and_then(Value::as_str) == Some(SCREEN) =>
                {
                    c.shown = None;
                    close(world, c);
                    return;
                }
                _ => {}
            }
        }
    }
    // Redraw when anything on it changed.
    let Some(caster) = world.get::<MagicCaster>(w) else {
        return;
    };
    let view = layout(c, caster, world.get::<SpellBar>(w));
    if c.shown.as_ref() != Some(&view) {
        let shown = call(
            world,
            "ui.show",
            json!({"screen": "layout", "key": SCREEN, "layout": view, "modal": true, "layer": 30}),
        );
        match shown {
            Ok(_) => c.shown = Some(view),
            // No UI: the crafter still runs (tests, the API).
            Err(e) => {
                if c.shown.is_none() {
                    tracing::debug!("spellcrafter: no screen ({e})");
                }
                c.shown = Some(view);
            }
        }
    }
    preview(world, c, w);
}

/// The crystal casts the design at the dummy every [`PREVIEW_EVERY`] s.
fn preview(world: &mut World, c: &mut Crafter, weaver: Entity) {
    let Some(altar) = c.altar else { return };
    let dt = world.resource::<Time>().delta_secs();
    c.preview_in -= dt;
    if c.preview_in > 0.0 {
        return;
    }
    c.preview_in = PREVIEW_EVERY;
    c.warmup = false;
    if preview_blocked(&c.design).is_some() {
        return;
    }
    let (Some(a), Some(w)) = (
        world.get::<Altar>(altar).copied(),
        world.get::<MagicCaster>(weaver).cloned(),
    ) else {
        return;
    };
    // At the weaver's level and mastery, on the weaver's side.
    if let Some(mut crystal) = world.get_mut::<MagicCaster>(altar) {
        crystal.level = w.level;
        crystal.power_scale = w.power_scale;
        crystal.skills = w.skills.clone();
        crystal.god = w.god;
        crystal.side = w.side;
        crystal.mana = crystal.max_mana;
    } else {
        return;
    }
    let aim = match a.dummy.filter(|d| world.get_entity(*d).is_ok()) {
        Some(d) => Aim::Entity(d),
        None => Aim::Point(a.stand + DVec3::Y * 1.1),
    };
    c.previews += 1;
    world.write_message(CastRequest {
        caster: altar,
        spell: SpellChoice::Design(c.design.clone()),
        aim,
        charge: 0.0,
        gathered: false,
        off_hand: false,
    });
}

/// `Update`: an altar's dummy stands while the altar is in use, and after the screen closes
/// while its weaver stays within [`STAY_M`] (to try the new spell from the bar), and waits
/// under the floor otherwise; its crystal glows in the color of the spell being woven.
#[allow(clippy::type_complexity)]
pub fn tend_altars(
    crafter: Res<Crafter>,
    altars: Query<(Entity, &Altar)>,
    mut dummies: Query<(&mut Transform, Option<&mut Vitality>), Without<Altar>>,
    places: Query<&GlobalTransform>,
    mut glows: Query<(
        &AltarGlow,
        Option<&mut MeshRenderer>,
        Option<&mut PointLight>,
    )>,
    mut last: Local<Option<(Option<Entity>, [u32; 3])>>,
) {
    let stays = crafter.altar.filter(|a| {
        let at = |e: Option<Entity>| e.and_then(|e| places.get(e).ok()).map(|g| g.translation());
        match (at(Some(*a)), at(crafter.weaver)) {
            (Some(a), Some(w)) => a.distance(w) < STAY_M as f64,
            _ => false,
        }
    });
    let in_use = if crafter.open { crafter.altar } else { stays };
    for (e, altar) in &altars {
        let Some(d) = altar.dummy else { continue };
        let Ok((mut tf, vit)) = dummies.get_mut(d) else {
            continue;
        };
        let want = if in_use == Some(e) {
            altar.stand
        } else {
            altar.stand - DVec3::Y * DUMMY_AWAY_M
        };
        if tf.translation.distance(want) > 0.01 {
            let rising = want.y > tf.translation.y;
            tf.translation = want;
            if rising && let Some(mut v) = vit {
                v.health = v.max;
                v.down_secs = 0.0;
            }
        }
    }
    let color = weave(&crafter.design.effects)
        .map(|w| w.color)
        .unwrap_or(IDLE_GLOW);
    let key = (in_use, color.map(|c| (c * 1000.0) as u32));
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    for (g, mesh, light) in &mut glows {
        let c = if in_use == Some(g.altar) {
            color
        } else {
            IDLE_GLOW
        };
        if let Some(mut m) = mesh
            && let MaterialRef::Inline(mat) = &mut m.material
        {
            mat.emissive = c;
            mat.emissive_strength = g.strength;
            mat.base_color = [c[0] * 0.3, c[1] * 0.3, c[2] * 0.3, 1.0];
        }
        if let Some(mut l) = light {
            l.color = c;
            l.intensity_lm = g.strength;
        }
    }
}

/// Keep the mouse free while the crafter is open (play mode captures it every frame;
/// `Last` runs after that).
pub fn free_cursor(crafter: Res<Crafter>, window: Option<ResMut<Window>>) {
    if !crafter.open || crafter.shown.is_none() {
        return;
    }
    if let Some(mut w) = window
        && w.cursor_mode != CursorMode::Normal
    {
        w.cursor_mode = CursorMode::Normal;
    }
}

/// `magic.crafter`.
pub fn register(app: &mut App) {
    app.register_api(
        "magic.crafter",
        "The Spellcrafter: build a spell in four steps (effects, cast type, modifiers, power), \
         see its complexity against the weaver's budget, learn it and put it on a key. \
         `open` with a `weaver` (and an `altar` whose crystal previews it), `do` one action or a \
         list (effect:frost, cast:bolt, mod:boomerang, more:pierce, less:split, step:modifiers, \
         charge, overcharge, learn, key:3, clear, close). Returns the state.",
        r#"{"open": true, "weaver": "Weaver", "do": ["effect:frost", "effect:fire", "mod:boomerang", "learn", "key:3"]}"#,
        |world, args| {
            if args.get("open").and_then(Value::as_bool) == Some(true) {
                let weaver =
                    engine_core::registry::resolve_entity(world, args.get("weaver").unwrap_or(&Value::Null))?;
                if world.get::<MagicCaster>(weaver).is_none() {
                    return Err("That weaver can't cast (no MagicCaster).".into());
                }
                let altar = match args.get("altar") {
                    Some(a) if !a.is_null() => Some(engine_core::registry::resolve_entity(world, a)?),
                    _ => None,
                };
                let mut c = world.remove_resource::<Crafter>().unwrap_or_default();
                if c.open {
                    close(world, &mut c);
                }
                open(world, &mut c, weaver, altar);
                world.insert_resource(c);
            }
            let actions: Vec<String> = match args.get("do") {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                _ => Vec::new(),
            };
            for a in actions {
                if !act_now(world, &a) {
                    return Err("The Spellcrafter isn't open: pass \"open\": true and a \"weaver\".".into());
                }
            }
            let c = world.get_resource::<Crafter>().cloned().unwrap_or_default();
            let caster = c.weaver.and_then(|w| world.get::<MagicCaster>(w)).cloned();
            Ok(state(&c, caster.as_ref()))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expert() -> MagicCaster {
        MagicCaster::new(60.0, Mastery::Expert)
    }

    fn glaive_steps() -> [&'static str; 9] {
        [
            "effect:frost",
            "effect:fire",
            "cast:bolt",
            "mod:boomerang",
            "mod:pierce",
            "mod:stacking",
            "mod:safeguard",
            "mod:split",
            "mod:speed",
        ]
    }

    #[test]
    fn a_change_hurries_the_preview_only_after_the_first_cast() {
        let mut caster = expert();
        let mut c = Crafter {
            design: starting_design(&caster),
            preview_in: FIRST_PREVIEW,
            warmup: true,
            ..Default::default()
        };
        // Frost in a moment after opening: the opening pause stands, so no steam bolt flies
        // before fire comes off again.
        apply(&mut c, &mut caster, None, "magic.craft.effect:frost");
        assert_eq!(c.preview_in, FIRST_PREVIEW);
        c.warmup = false;
        c.preview_in = PREVIEW_EVERY;
        apply(&mut c, &mut caster, None, "magic.craft.effect:fire");
        assert_eq!(c.preview_in, PREVIEW_AFTER_CHANGE);
    }

    #[test]
    fn the_frost_glaive_fills_an_experts_budget_and_one_more_part_is_refused() {
        let mut caster = expert();
        let mut c = Crafter {
            design: starting_design(&caster),
            ..Default::default()
        };
        assert_eq!(c.design.effects, vec!["fire".to_string()]);
        let mut climb = Vec::new();
        for a in glaive_steps() {
            assert!(!apply(&mut c, &mut caster, None, a), "{a}");
            assert!(c.note.is_none(), "{a}: {:?}", c.note);
            climb.push(c.design.complexity());
        }
        // Fire and frost weave into steam, then frost alone; boomerang +2, pierce, stacking
        // and safeguard +1 each, a split of three +2, speed +1.
        assert_eq!(climb, vec![1, 0, 0, 2, 3, 4, 5, 7, 8]);
        assert_eq!(budget_of(&c.design, &caster).1, 8);
        assert_eq!(auto_name(&c.design, None), "Frost glaives");
        let before = c.design.clone();
        apply(&mut c, &mut caster, None, "magic.craft.mod:seek");
        assert_eq!(c.design, before, "seek is refused");
        let note = c.note.clone().unwrap();
        assert!(
            !note.ok && note.text.contains("10") && note.text.contains("an Expert's 8"),
            "{}",
            note.text
        );
        assert_eq!(c.refused, Some(10));
        // Overcharge lets it through, and taking Overcharge off again is refused while past.
        apply(&mut c, &mut caster, None, "overcharge");
        apply(&mut c, &mut caster, None, "mod:seek");
        assert_eq!(c.design.complexity(), 10);
        apply(&mut c, &mut caster, None, "overcharge");
        assert!(c.note.as_ref().is_some_and(|n| !n.ok));
        assert!(caster.compile(&c.design, 0.0).unwrap().backfire > 0.0);
        // Removing a part always works.
        apply(&mut c, &mut caster, None, "mod:seek");
        assert_eq!(c.design.complexity(), 8);
    }

    #[test]
    fn learning_names_the_spell_and_a_key_takes_it() {
        let mut caster = expert();
        let mut bar = SpellBar {
            slots: vec![None; SLOTS],
            ..Default::default()
        };
        let mut c = Crafter {
            design: starting_design(&caster),
            ..Default::default()
        };
        for a in glaive_steps() {
            apply(&mut c, &mut caster, Some(&mut bar), a);
        }
        apply(&mut c, &mut caster, Some(&mut bar), "learn");
        assert_eq!(c.learned, Some(0));
        assert_eq!(caster.grimoire[0].design.name, "Frost glaives");
        apply(&mut c, &mut caster, Some(&mut bar), "key:3");
        assert_eq!(bar.slots[2], Some(0), "the same spell, not learned twice");
        assert_eq!(caster.grimoire.len(), 1);
        assert!(c.note.as_ref().unwrap().text.contains("key 3"));
        // A different spell with the same name gets a numeral.
        apply(&mut c, &mut caster, Some(&mut bar), "more:pierce");
        assert_eq!(c.learned, None, "a change unlearns the screen's spell");
        apply(&mut c, &mut caster, Some(&mut bar), "learn");
        assert_eq!(caster.grimoire[1].design.name, "Frost glaives II");
        apply(&mut c, &mut caster, Some(&mut bar), "mod:split");
        apply(&mut c, &mut caster, Some(&mut bar), "learn");
        assert_eq!(caster.grimoire[2].design.name, "Frost glaive");
        assert!(caster.compile(&caster.grown(2).unwrap(), 0.0).is_ok());
    }

    #[test]
    fn effects_follow_skill_and_dark_parts_follow_the_weave() {
        let mut caster = MagicCaster::new(30.0, Mastery::Novice);
        caster.skills.insert(
            Subclass::Evocation,
            crate::caster::Skill::at(Mastery::Adept),
        );
        caster.skills.insert(
            Subclass::Malefice,
            crate::caster::Skill::at(Mastery::Apprentice),
        );
        assert_eq!(
            open_subclasses(&caster),
            vec![Subclass::Evocation, Subclass::Malefice]
        );
        let mut c = Crafter {
            design: starting_design(&caster),
            ..Default::default()
        };
        apply(&mut c, &mut caster, None, "effect:heal");
        assert!(
            c.note
                .as_ref()
                .is_some_and(|n| n.text.contains("Apprentice in Vitalism"))
        );
        apply(&mut c, &mut caster, None, "mod:subdue");
        assert!(c.design.has(|m| matches!(m, Modifier::Subdue)));
        // Blood makes it dark: subdue comes off by itself, lifesteal fits now.
        apply(&mut c, &mut caster, None, "effect:blood");
        assert!(!c.design.has(|m| matches!(m, Modifier::Subdue)));
        assert!(
            c.note
                .as_ref()
                .is_some_and(|n| n.ok && n.text.contains("Subdue"))
        );
        assert!(fits_design(part_of("lifesteal").unwrap(), &c.design));
        // A fourth effect is refused; the last one can't come off.
        apply(&mut c, &mut caster, None, "effect:frost");
        apply(&mut c, &mut caster, None, "effect:storm");
        assert_eq!(c.design.effects.len(), 3);
        assert!(
            c.note
                .as_ref()
                .is_some_and(|n| !n.ok && n.text.contains("Three effects"))
        );
        for k in ["fire", "blood", "frost"] {
            apply(&mut c, &mut caster, None, &format!("effect:{k}"));
        }
        assert_eq!(c.design.effects, vec!["frost".to_string()]);
        apply(&mut c, &mut caster, None, "effect:frost");
        assert_eq!(c.design.effects, vec!["frost".to_string()]);
    }

    #[test]
    fn rows_add_up_and_names_follow_the_form() {
        let d = SpellDesign::new(&["fire", "frost"], CastType::Area)
            .with(Modifier::Blast { radius_m: 2.0 })
            .with(Modifier::Charge { max_secs: 1.0 });
        let r = rows(&d);
        assert_eq!(r.iter().sum::<u32>(), d.complexity());
        assert_eq!(r, [1, 1, 3, 1]);
        assert_eq!(
            auto_name(&SpellDesign::new(&["fire", "frost"], CastType::Lob), None),
            "Steam flask"
        );
        assert_eq!(
            auto_name(
                &SpellDesign::new(&["fire"], CastType::Bolt).with(Modifier::Split { count: 3 }),
                None
            ),
            "Fire volley"
        );
        assert_eq!(
            auto_name(&SpellDesign::new(&["lift", "stone"], CastType::Area), None),
            "Pillar"
        );
        let golem = SpellDesign::new(&["stone"], CastType::Summon);
        let s = expert().compile(&golem, 0.0).unwrap();
        assert_eq!(auto_name(&golem, Some(&s)), "Stone golem");
    }

    #[test]
    fn the_screen_lays_out_and_every_button_has_an_action() {
        let mut caster = expert();
        let mut c = Crafter {
            design: starting_design(&caster),
            altar: None,
            ..Default::default()
        };
        for a in glaive_steps() {
            apply(&mut c, &mut caster, None, a);
        }
        for s in Step::ALL {
            c.step = s;
            let v = layout(&c, &caster, None);
            let text = v.to_string();
            assert!(text.contains("Frost glaives"), "{s:?}");
            assert!(
                text.contains("Complexity [b]8[/b] of an Expert's 8"),
                "{s:?}"
            );
            // Every clickable node's id is the one `press` looks for.
            fn walk(v: &Value, out: &mut Vec<(String, String)>) {
                if let (Some(id), Some(a)) = (
                    v.get("id").and_then(Value::as_str),
                    v.get("on_click").and_then(Value::as_str),
                ) {
                    out.push((id.to_string(), a.to_string()));
                }
                for ch in v
                    .get("children")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    walk(ch, out);
                }
            }
            let mut buttons = Vec::new();
            walk(&v, &mut buttons);
            assert!(buttons.len() > 12, "{s:?}");
            for (id, a) in &buttons {
                assert_eq!(*id, id_of(a));
            }
            let mut ids: Vec<&String> = buttons.iter().map(|(i, _)| i).collect();
            ids.sort();
            let n = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), n, "ids are unique");
        }
        // The seek chip is dimmed: it would go past.
        c.step = Step::Modifiers;
        let v = layout(&c, &caster, None).to_string();
        assert!(v.contains("\"opacity\":0.45"));
    }

    #[test]
    fn the_hex_color_keeps_the_hue() {
        assert_eq!(hex([1.0, 0.0, 0.0], 0.0), "#ff0000");
        assert_eq!(hex([0.2, 0.2, 0.2], 0.0), "#ffffff");
        assert_eq!(hex([0.0, 0.0, 0.5], 0.5), "#8080ff");
    }
}
