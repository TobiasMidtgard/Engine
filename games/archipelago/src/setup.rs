//! The new-game setup page, opened by New game on the title: up to three rival houses, each
//! led by a character with its own temper and way of playing at a level of its own, and the
//! game's rules (economy, military, the world, restrictions, cheats).
//!
//! Set sail starts the game in view at once: the rules are applied and the houses set sail
//! (the clock has stood still since the world was made). A rule that shapes the world as it
//! is made (the economy's first purse and cargo, resources, space), or other houses than a
//! world made with `--rival-picks` has, starts the game again with the choice as flags
//! (`--no-title --rival-picks … --rules …`). The last choice is kept for the next game in
//! `%APPDATA%\Brightsund\setup.json` (`BRIGHTSUND_SETUP` names another file).

use std::path::PathBuf;

use engine::prelude::*;
use engine_ui::color::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Screen;
use engine_ui::screens;
use engine_ui::style::{Align, Corners, Direction, Ease, Enter, Fill, Justify, Pick, StylePatch};
use engine_ui::text::{DISPLAY_FONT, HUMANIST_FONT};
use engine_ui::theme::ClassStyle;
use serde::{Deserialize, Serialize};

use crate::brain::Level;
use crate::characters::{self, RivalPicks};
use crate::rules::{self, Rules};

/// The setup page's screen.
pub const SETUP: &str = "fv_setup";
/// Rival seats on the page.
pub const SEATS: usize = 3;

/// The page's choice, and what the world in view was made with.
#[derive(Resource, Clone, Debug)]
pub struct Setup {
    /// Each seat's character (an index into `characters::ALL`) and level, or empty.
    pub seats: [Option<(usize, Level)>; SEATS],
    pub rules: Rules,
    /// The rules the world in view was made with.
    made_rules: Rules,
    /// The houses the world in view was made with (`--rival-picks`).
    made_houses: Vec<(usize, Level)>,
}

impl Setup {
    /// The houses chosen, seat by seat.
    pub fn picks(&self) -> Vec<(usize, Level)> {
        self.seats.iter().flatten().copied().collect()
    }

    /// Whether Set sail must start the game again: a rule read as the world is made has
    /// changed, or the world already has other houses.
    pub fn needs_restart(&self) -> bool {
        let (a, b) = (&self.made_rules, &self.rules);
        a.economy != b.economy
            || a.resources != b.resources
            || a.space != b.space
            || (!self.made_houses.is_empty() && self.made_houses != self.picks())
    }

    /// The start flags for this choice, past the title.
    pub fn flags(&self) -> Vec<String> {
        flags_for(&self.picks(), &self.rules)
    }
}

/// The start flags for a game with houses `picks` and `rules`, past the title.
pub fn flags_for(picks: &[(usize, Level)], rules: &Rules) -> Vec<String> {
    let mut out = vec!["--no-title".to_string()];
    if !picks.is_empty() {
        out.push("--rival-picks".to_string());
        out.push(RivalPicks(picks.to_vec()).flag());
    }
    let rules = rules.flag();
    if !rules.is_empty() {
        out.push("--rules".to_string());
        out.push(rules);
    }
    out
}

/// The flags that start the game in play again as it was set up (the Esc menu's New game).
pub fn again(world: &World) -> Vec<String> {
    if let Some(s) = world.get_resource::<Setup>() {
        return s.flags();
    }
    let rules = rules::of(world);
    if let Some(p) = world.get_resource::<RivalPicks>() {
        return flags_for(&p.0, &rules);
    }
    let mut out = flags_for(&[], &rules);
    if let Some(r) = world.get_resource::<crate::brain::RivalSetup>()
        && r.count > 0
    {
        out.extend([
            "--rivals".to_string(),
            r.count.to_string(),
            "--rival-level".to_string(),
            r.level.name().to_lowercase(),
        ]);
    }
    out
}

/// Set sail in the world in view: the chosen rules, then the chosen houses (a world made
/// with houses keeps them).
pub fn begin(world: &mut World) {
    let Some(s) = world.get_resource::<Setup>().cloned() else {
        return;
    };
    rules::apply(world, s.rules);
    let picks = s.picks();
    if s.made_houses.is_empty() && !picks.is_empty() {
        world.insert_resource(RivalPicks(picks.clone()));
        crate::brain::start_houses(world, &picks);
        crate::build::refresh(world);
        world.insert_resource(crate::brain::RivalSetup {
            count: picks.len(),
            level: picks[0].1,
        });
    }
    let made = Setup {
        made_rules: s.rules,
        made_houses: picks,
        ..s
    };
    world.insert_resource(made);
}

/// What a rival at `level` is like, before its character shapes it.
pub fn level_note(level: Level) -> &'static str {
    match level {
        Level::Deckhand => "slow and forgiving, with short chains",
        Level::Mate => "takes its time",
        Level::Captain => "a fair match",
        Level::Commodore => "quick and sharp",
        Level::Admiral => "plays to win and misses nothing",
    }
}

/// How character `ci` plays at `level`, in a line: the level's note, then what the character
/// makes of it (how many islands it holds, whether it races you, buys abroad, and whether it
/// starts in your waters, which only the `first` house can).
pub fn seat_note(ci: usize, level: Level, first: bool, rules: &Rules) -> String {
    let k = characters::shape(level.knobs(), &characters::ALL[ci].traits);
    let mut facts = vec![match k.isles {
        1 => "keeps to one island".to_string(),
        n if n >= 99 => "takes every island it can".to_string(),
        n => format!("up to {n} islands"),
    }];
    if k.race {
        facts.push("races you for the best ones".into());
    }
    if k.abroad {
        facts.push("buys at other harbours".into());
    }
    if first && rules.rival_near(level) {
        facts.push("starts in your waters".into());
    }
    format!(
        "{}: {}; {}.",
        level.name(),
        level_note(level),
        facts.join(", ")
    )
}

/// The houses a new game offers when nothing else says: the first three characters, at
/// Captain.
pub fn default_seats() -> [Option<(usize, Level)>; SEATS] {
    std::array::from_fn(|i| (i < characters::ALL.len()).then_some((i, Level::Captain)))
}

// ------------------------------------------------------------------------------ kept choice

/// The last setup, as the flags' own words.
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
struct Kept {
    #[serde(default)]
    rival_picks: String,
    #[serde(default)]
    rules: String,
}

fn kept_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BRIGHTSUND_SETUP") {
        return Some(PathBuf::from(p));
    }
    if let Ok(a) = std::env::var("APPDATA") {
        return Some(PathBuf::from(a).join("Brightsund").join("setup.json"));
    }
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok()?;
    Some(base.join("brightsund").join("setup.json"))
}

fn load_kept() -> Option<Kept> {
    let text = std::fs::read_to_string(kept_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_kept(s: &Setup) {
    let Some(path) = kept_path() else {
        return;
    };
    let kept = Kept {
        rival_picks: RivalPicks(s.picks()).flag(),
        rules: s.rules.flag(),
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&kept)
        && let Err(e) = std::fs::write(&path, text)
    {
        engine::core::log::warn!("setup: {}: {e}", path.display());
    }
}

/// Rules from their flag words (`economy=hard,military=ruthless`) over `base` (words it
/// does not know are left out).
fn rules_from(text: &str, base: Rules) -> Rules {
    let mut r = base;
    for pair in text.split(',').filter(|p| !p.is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            let _ = r.set(k.trim(), v.trim());
        }
    }
    r
}

/// A choice's word as the page shows it: `free_traders` → "Free traders", `on` → "On".
fn shown(word: &str) -> String {
    let w = word.replace('_', " ");
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The words for how a character plays, from its traits (only what stands out).
fn trait_words(t: &characters::Traits) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut say = |v: f32, hi: &'static str, lo: Option<&'static str>, lo_at: f32| {
        if v >= 1.35 {
            out.push(hi);
        } else if let Some(lo) = lo
            && v <= lo_at
        {
            out.push(lo);
        }
    };
    say(t.expand, "Explorer", Some("Homebody"), 0.7);
    say(t.trade, "Trader", Some("Keeps to itself"), 0.6);
    say(t.aggression, "Warlike", Some("Peaceable"), 0.5);
    say(t.ambition, "Ambitious", Some("Content"), 0.7);
    say(t.thrift, "Miser", Some("Spendthrift"), 0.7);
    say(t.gifts, "Won by gifts", Some("Not for sale"), 0.6);
    say(t.grudge, "Bears grudges", Some("Forgiving"), 0.6);
    out
}

/// Before the world is made (title runs with no `--rules`): the last game's rules, so a
/// setup kept from before needs no restart.
pub fn apply_kept_rules(world: &mut World) {
    if let Some(k) = load_kept() {
        let r = rules_from(&k.rules, rules::of(world));
        rules::apply(world, r);
    }
}

/// The page's choice when the title opens: the flags' houses and rules when the run was
/// given them, else the last game's, else three Captains and the normal rules.
pub fn init(world: &mut World, flagged_houses: bool) {
    init_with(world, flagged_houses, load_kept());
}

fn init_with(world: &mut World, flagged_houses: bool, kept: Option<Kept>) {
    let made_rules = rules::of(world);
    let made_houses = if flagged_houses {
        world
            .get_resource::<RivalPicks>()
            .map(|p| p.0.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut seats = default_seats();
    if !made_houses.is_empty() {
        seats = std::array::from_fn(|i| made_houses.get(i).copied());
    } else if let Some(k) = kept
        && let Ok(p) = RivalPicks::parse(&k.rival_picks, Level::Captain)
    {
        seats = std::array::from_fn(|i| p.0.get(i).copied());
    }
    world.insert_resource(Setup {
        seats,
        rules: made_rules,
        made_rules,
        made_houses,
    });
}

// ------------------------------------------------------------------------------ actions

/// What a setup action did.
pub enum Done {
    /// Nothing for the title to do.
    Nothing,
    /// Back to the title.
    Back,
    /// Set sail in the world in view.
    Start,
    /// Start the game again with these flags.
    Restart(Vec<String>),
}

/// Handle `fv.setup.*`, `fv.seat.*` and `fv.rule:*` (None: not a setup action).
pub fn act(world: &mut World, a: &str) -> Option<Done> {
    if !a.starts_with("fv.setup") && !a.starts_with("fv.seat") && !a.starts_with("fv.rule") {
        return None;
    }
    if !world.contains_resource::<Setup>() {
        init(world, false);
    }
    let n = characters::ALL.len();
    let mut s = world.resource::<Setup>().clone();
    let taken = |s: &Setup, skip: usize| -> Vec<usize> {
        s.seats
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != skip)
            .filter_map(|(_, x)| x.map(|(c, _)| c))
            .collect()
    };
    // The next character, `step` along, that no other seat holds.
    let next_free = |s: &Setup, seat: usize, from: usize, step: isize| -> Option<usize> {
        let taken = taken(s, seat);
        (1..=n as isize)
            .map(|k| (from as isize + step * k).rem_euclid(n as isize) as usize)
            .find(|c| !taken.contains(c))
    };
    let seat_of = |rest: &str| rest.parse::<usize>().ok().filter(|i| *i < SEATS);
    let done = match a {
        "fv.setup.open" => {
            open(world);
            return Some(Done::Nothing);
        }
        "fv.setup.back" => {
            screens::hide(world, SETUP, false);
            return Some(Done::Back);
        }
        "fv.setup.defaults" => {
            s.seats = default_seats();
            s.rules = Rules::default();
            Done::Nothing
        }
        "fv.setup.start" => {
            save_kept(&s);
            screens::hide(world, SETUP, false);
            return Some(if s.needs_restart() {
                Done::Restart(s.flags())
            } else {
                Done::Start
            });
        }
        _ => {
            if let Some(i) = a.strip_prefix("fv.seat.next:").and_then(seat_of) {
                let from = s.seats[i].map_or(n - 1, |x| x.0);
                let level = s.seats[i].map_or(Level::Captain, |x| x.1);
                s.seats[i] = next_free(&s, i, from, 1).map(|c| (c, level));
            } else if let Some(i) = a.strip_prefix("fv.seat.prev:").and_then(seat_of) {
                let from = s.seats[i].map_or(0, |x| x.0);
                let level = s.seats[i].map_or(Level::Captain, |x| x.1);
                s.seats[i] = next_free(&s, i, from, -1).map(|c| (c, level));
            } else if let Some(i) = a.strip_prefix("fv.seat.clear:").and_then(seat_of) {
                s.seats[i] = None;
            } else if let Some(rest) = a.strip_prefix("fv.seat.level:") {
                if let Some((i, l)) = rest.split_once(':')
                    && let (Some(i), Some(l)) = (seat_of(i), Level::from_name(l))
                    && let Some(x) = s.seats[i].as_mut()
                {
                    x.1 = l;
                }
            } else if let Some((k, v)) = a.strip_prefix("fv.rule:").and_then(|r| r.split_once('='))
            {
                let _ = s.rules.set(k, v);
            }
            Done::Nothing
        }
    };
    world.insert_resource(s.clone());
    refresh(world, &s);
    Some(done)
}

/// Put the page up.
pub fn open(world: &mut World) {
    if !world.contains_resource::<Setup>() {
        init(world, false);
    }
    install_classes(world);
    let s = world.resource::<Setup>().clone();
    let screen = Screen {
        key: SETUP.into(),
        modal: true,
        layer: 40,
        closable: false,
        ..Default::default()
    };
    screens::hide(world, SETUP, true);
    if let Err(e) = screens::spawn_screen(world, screen, &page(&s)) {
        engine::core::log::warn!("setup: {e}");
    }
}

/// Show the choice anew (the page stays, its body is built again).
fn refresh(world: &mut World, s: &Setup) {
    if let Some(e) = engine_ui::input::find_id(world, "fv_setup_body") {
        let _ = engine_ui::el::replace_children(world, e, &body(s));
    }
}

/// Whether the page is up.
pub fn is_open(world: &mut World) -> bool {
    screens::find_screen(world, SETUP).is_some()
}

// ------------------------------------------------------------------------------ the page

fn install_classes(world: &mut World) {
    let Some(mut theme) = world.get_resource_mut::<engine_ui::theme::Theme>() else {
        return;
    };
    // A choice on the parchment: an ink rim, brass when picked.
    let chip = StylePatch::default()
        .padding([3.0, 11.0, 4.0, 11.0])
        .radius(Corners::all(4.0))
        .border(1.0, Color::hexa(0x6a533970))
        .background(Fill::Solid(Color::hexa(0xfff8e630)))
        .color(Colors::ink_dim())
        .font_family(HUMANIST_FONT)
        .font_weight(700)
        .font_size(15.5)
        .transition(0.12)
        .ease(Ease::Out);
    theme.classes.insert(
        "fv_pchip".to_string(),
        ClassStyle {
            base: chip,
            hover: StylePatch::default()
                .border_color(Colors::ink())
                .color(Colors::ink()),
            focused: StylePatch::default().border_color(Colors::ink()),
            selected: StylePatch::default()
                .background(Fill::vertical(Color::hex(0xf2d692), Color::hex(0xc5953f)))
                .border_color(Colors::brass_dark())
                .color(Colors::ink()),
            ..Default::default()
        },
    );
    // A small square button on the parchment (‹ › ×).
    theme.classes.insert(
        "fv_pbtn".to_string(),
        ClassStyle {
            base: StylePatch::default()
                .size(34.0, 34.0)
                .radius(Corners::all(17.0))
                .border(1.0, Color::hexa(0x6a533990))
                .background(Fill::vertical(Color::hex(0xfbf3dd), Color::hex(0xe6d5ae)))
                .justify(Justify::Center)
                .align_items(Align::Center)
                .color(Colors::ink())
                .font_family(HUMANIST_FONT)
                .font_weight(700)
                .font_size(18.0)
                .transition(0.12),
            hover: StylePatch::default().border_color(Colors::brass_dark()),
            pressed: StylePatch::default().translate([0.0, 1.0]),
            ..Default::default()
        },
    );
}

fn heading(text: &str) -> El {
    El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .align_items(Align::Center)
                .gap(12.0)
                .margin([4.0, 0.0, 2.0, 0.0])
                .pick(Pick::Ignore)
        })
        .child(
            El::text(text.to_uppercase())
                .class("ink_label")
                .text_style(|t| {
                    let mut t = t.nowrap();
                    t.letter_spacing = 0.16;
                    t
                })
                .with_style(|s| s.font_size(15.0))
                .pick_ignore(),
        )
        .child(El::new().with_style(|s| {
            s.height(1.0)
                .grow(1.0)
                .background(Color::hexa(0x6a533960))
                .pick(Pick::Ignore)
        }))
}

fn pchip(label: &str, action: String, on: bool) -> El {
    El::new()
        .class("fv_pchip")
        .on_click(action)
        .selected(on)
        .child(El::text(label).text_style(|t| t.nowrap()).pick_ignore())
}

fn pbtn(label: &str, action: String, tip: &str) -> El {
    El::new()
        .class("fv_pbtn")
        .on_click(action)
        .tooltip(tip, "")
        .child(El::text(label).text_style(|t| t.nowrap()).pick_ignore())
}

fn ink(text: impl Into<String>, size: f32) -> El {
    El::text(text)
        .text_style(|t| t.line_height(1.34))
        .with_style(move |s| s.font_size(size).color(Colors::ink()).shrink(1.0))
        .pick_ignore()
}

fn dim(text: impl Into<String>, size: f32) -> El {
    El::text(text)
        .text_style(|t| t.line_height(1.3))
        .with_style(move |s| s.font_size(size).color(Colors::ink_dim()).shrink(1.0))
        .pick_ignore()
}

/// A seat: the character who leads the house, how it plays and its level; or an empty seat.
fn seat(i: usize, pick: Option<(usize, Level)>, first: bool, rules: &Rules) -> El {
    let card = El::new().with_style(|s| {
        s.direction(Direction::Column)
            .gap(8.0)
            .padding([14.0, 16.0, 14.0, 16.0])
            .radius(Corners::all(6.0))
            .border(1.0, Color::hexa(0x6a533950))
            .background(Fill::Solid(Color::hexa(0x6a53390f)))
    });
    let Some((ci, level)) = pick else {
        return card
            .with_style(|s| {
                s.direction(Direction::Row)
                    .align_items(Align::Center)
                    .gap(14.0)
            })
            .child(El::new().with_style(|s| {
                s.size(64.0, 64.0)
                    .shrink(0.0)
                    .radius(Corners::all(32.0))
                    .border(1.5, Color::hexa(0x6a533960))
                    .pick(Pick::Ignore)
            }))
            .child(
                El::new()
                    .with_style(|s| {
                        s.direction(Direction::Column)
                            .gap(2.0)
                            .grow(1.0)
                            .pick(Pick::Ignore)
                    })
                    .child(ink(format!("Seat {}: empty", i + 1), 20.0))
                    .child(dim("No rival house sails from this seat.", 15.0)),
            )
            .child(pchip("Add a rival", format!("fv.seat.next:{i}"), false));
    };
    let c = &characters::ALL[ci];
    let colour = Color::hex(c.colour);
    let emblem = format!("isle_emblem_{}", c.emblem);
    let name = El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(10.0)
                .align_items(Align::End)
                .pick(Pick::Ignore)
        })
        .child(
            El::text(c.leader)
                .text_style(|t| t.nowrap())
                .with_style(|s| {
                    s.font_family(DISPLAY_FONT)
                        .font_weight(700)
                        .font_size(25.0)
                        .color(Colors::ink())
                })
                .pick_ignore(),
        )
        .child(
            El::text(c.house)
                .text_style(|t| t.nowrap().bold())
                .with_style(move |s| s.font_size(16.0).color(colour).margin([0.0, 0.0, 3.0, 0.0]))
                .pick_ignore(),
        );
    let traits = El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(6.0)
                .wrap(true)
                .pick(Pick::Ignore)
        })
        .children(
            trait_words(&c.traits)
                .into_iter()
                .chain(
                    (characters::shape(level.knobs(), &c.traits).top == crate::defs::Tier::Masters)
                        .then_some("Stops at Masters"),
                )
                .map(|t| {
                    El::new()
                        .with_style(|s| {
                            s.padding([1.0, 8.0, 2.0, 8.0])
                                .radius(Corners::all(9.0))
                                .background(Fill::Solid(Color::hexa(0x6a53391c)))
                                .pick(Pick::Ignore)
                        })
                        .child(
                            El::text(t)
                                .text_style(|t| t.nowrap().bold())
                                .with_style(|s| s.font_size(13.5).color(Colors::ink_dim()))
                                .pick_ignore(),
                        )
                }),
        );
    let info = El::new()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(3.0)
                .grow(1.0)
                .shrink(1.0)
                .pick(Pick::Ignore)
        })
        .child(name)
        .child(
            El::text(c.temperament)
                .text_style(|t| t.nowrap().italic())
                .with_style(|s| s.font_size(15.5).color(Colors::ink_dim()))
                .pick_ignore(),
        )
        .child(ink(c.style, 15.5))
        .child(traits);
    let turn = El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(6.0)
                .shrink(0.0)
                .align_items(Align::Start)
        })
        .child(pbtn(
            "‹",
            format!("fv.seat.prev:{i}"),
            "The character before",
        ))
        .child(pbtn("›", format!("fv.seat.next:{i}"), "The next character"))
        .child(pbtn(
            "×",
            format!("fv.seat.clear:{i}"),
            "Leave this seat empty",
        ));
    let levels = El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(5.0)
                .align_items(Align::Center)
                .margin([2.0, 0.0, 0.0, 78.0])
        })
        .children(Level::ALL.iter().map(|l| {
            pchip(
                l.name(),
                format!("fv.seat.level:{i}:{}", l.name().to_lowercase()),
                *l == level,
            )
        }));
    let note = El::text(seat_note(ci, level, first, rules))
        .text_style(|t| t.line_height(1.25))
        .with_style(|s| {
            s.font_size(14.0)
                .color(Colors::ink_dim())
                .margin([0.0, 0.0, 0.0, 78.0])
                .shrink(1.0)
        })
        .pick_ignore();
    card.child(
        El::new()
            .with_style(|s| {
                s.direction(Direction::Row)
                    .gap(14.0)
                    .align_items(Align::Start)
            })
            .child(brass::badge(&emblem, 64.0).with_style(|s| s.shrink(0.0)))
            .child(info)
            .child(turn),
    )
    .child(levels)
    .child(note)
}

/// One rule: its name, its choices, and what the chosen one does.
fn rule_row(opt: &rules::Opt, r: &Rules) -> El {
    let now = r.value(opt.key);
    let chips = El::new()
        .with_style(|s| s.direction(Direction::Row).gap(5.0).wrap(true).grow(1.0))
        .children(opt.choices.iter().map(|value| {
            pchip(
                &shown(value),
                format!("fv.rule:{}={value}", opt.key),
                now.as_str() == *value,
            )
        }));
    let help = opt
        .choices
        .iter()
        .position(|v| now.as_str() == *v)
        .and_then(|i| opt.help.get(i))
        .copied()
        .unwrap_or("");
    El::new()
        .with_style(|s| s.direction(Direction::Column).gap(3.0))
        .child(
            El::new()
                .with_style(|s| {
                    s.direction(Direction::Row)
                        .gap(10.0)
                        .align_items(Align::Center)
                })
                .child(
                    El::text(opt.label)
                        .text_style(|t| t.nowrap().bold())
                        .with_style(|s| {
                            s.width(150.0)
                                .shrink(0.0)
                                .font_size(16.0)
                                .color(Colors::ink())
                        })
                        .pick_ignore(),
                )
                .child(chips),
        )
        .child(dim(help, 14.5).with_style(|s| s.margin([0.0, 0.0, 0.0, 160.0])))
}

/// The page's body: the seats on the left, the rules by group on the right.
fn body(s: &Setup) -> Vec<El> {
    let mut left = El::new()
        .with_style(|st| {
            st.direction(Direction::Column)
                .gap(10.0)
                .width(700.0)
                .shrink(0.0)
        })
        .child(heading("Rival houses"));
    let first = s.seats.iter().position(|p| p.is_some());
    for (i, p) in s.seats.iter().enumerate() {
        left = left.child(seat(i, *p, first == Some(i), &s.rules));
    }
    let n = s.picks().len();
    left = left.child(dim(
        match n {
            0 => "The sea is yours alone.".to_string(),
            1 => "One house sails against you.".to_string(),
            k => format!("{k} houses sail against you, each its own way."),
        },
        15.0,
    ));
    let mut right = El::new().with_style(|st| {
        st.direction(Direction::Column)
            .gap(8.0)
            .width(560.0)
            .shrink(0.0)
    });
    let mut groups: Vec<&str> = Vec::new();
    for o in rules::OPTIONS.iter() {
        if !groups.contains(&o.group) {
            groups.push(o.group);
        }
    }
    for g in groups {
        right = right.child(heading(g));
        for o in rules::OPTIONS.iter().filter(|o| o.group == g) {
            right = right.child(rule_row(o, &s.rules));
        }
    }
    if s.needs_restart() {
        right = right.child(
            dim(
                "These rules shape the world as it is made: Set sail starts the game again with them.",
                14.5,
            )
            .with_style(|st| st.margin([6.0, 0.0, 0.0, 0.0])),
        );
    }
    vec![
        El::new()
            .with_style(|st| {
                st.direction(Direction::Row)
                    .gap(44.0)
                    .align_items(Align::Start)
            })
            .child(left)
            .child(right),
    ]
}

fn page(s: &Setup) -> El {
    let key = crate::title::key;
    let back = El::new()
        .class("brass_button")
        .id("fv_setup_back")
        .on_click("fv.setup.back")
        .child(El::text("Back").pick_ignore())
        .child(key("Esc", true));
    let defaults = El::new()
        .class("brass_button")
        .id("fv_setup_defaults")
        .on_click("fv.setup.defaults")
        .child(El::text("Defaults").pick_ignore());
    let sail = El::new()
        .class("brass_button")
        .id("fv_setup_start")
        .on_click("fv.setup.start")
        .child(El::text("Set sail").pick_ignore())
        .child(key("Enter", true));
    let head = El::new()
        .with_style(|st| {
            st.direction(Direction::Row)
                .align_items(Align::Center)
                .gap(14.0)
        })
        .child(
            El::new()
                .with_style(|st| st.direction(Direction::Column).gap(2.0).pick(Pick::Ignore))
                .child(brass::label("NEW GAME", true))
                .child(
                    El::text("Your rivals and the rules")
                        .class("ink_title")
                        .text_style(|t| t.nowrap())
                        .with_style(|st| st.font_size(40.0).font_weight(900))
                        .pick_ignore(),
                ),
        )
        .child(El::new().with_style(|st| st.grow(1.0).pick(Pick::Ignore)))
        .child(defaults)
        .child(back)
        .child(sail);
    let card = brass::parchment()
        .with_style(|st| {
            st.direction(Direction::Column)
                .width(1400.0)
                .padding([28.0, 44.0, 32.0, 44.0])
                .gap(14.0)
                .enter(Enter::rise(0.3, 22.0).ease(Ease::Out))
        })
        .child(head)
        .child(El::new().id("fv_setup_body").children(body(s)));
    El::new()
        .class("screen center")
        .child(El::new().with_style(|_| engine_ui::widgets::scrim(0.72)))
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Bounty, Hardship, Menace};

    fn world() -> World {
        let mut w = World::new();
        w.insert_resource(crate::sim::Economy::default());
        init_with(&mut w, false, None);
        w
    }

    fn seats(w: &World) -> Vec<Option<usize>> {
        w.resource::<Setup>()
            .seats
            .iter()
            .map(|x| x.map(|(c, _)| c))
            .collect()
    }

    /// Three Captains to begin with; turning a seat skips the characters the other seats
    /// hold, and an empty seat takes the first one free.
    #[test]
    fn seats_turn_through_the_characters_no_two_alike() {
        let mut w = world();
        assert_eq!(seats(&w), [Some(0), Some(1), Some(2)]);
        assert!(
            w.resource::<Setup>()
                .seats
                .iter()
                .flatten()
                .all(|x| x.1 == Level::Captain)
        );
        act(&mut w, "fv.seat.next:0");
        assert_eq!(seats(&w), [Some(3), Some(1), Some(2)]);
        act(&mut w, "fv.seat.prev:0");
        act(&mut w, "fv.seat.prev:0");
        // Back past 0 to the last character (1 and 2 are taken).
        assert_eq!(seats(&w)[0], Some(characters::ALL.len() - 1));
        act(&mut w, "fv.seat.clear:1");
        assert_eq!(seats(&w)[1], None);
        act(&mut w, "fv.seat.next:1");
        assert_eq!(seats(&w)[1], Some(0));
        act(&mut w, "fv.seat.level:2:admiral");
        assert_eq!(w.resource::<Setup>().seats[2], Some((2, Level::Admiral)));
    }

    /// The economy, resources and space are read as the world is made: changing one means a
    /// restart, with the choice as flags; the military does not.
    #[test]
    fn world_shaping_rules_restart_with_the_choice_as_flags() {
        let mut w = world();
        act(&mut w, "fv.rule:military=ruthless");
        let s = w.resource::<Setup>().clone();
        assert_eq!(s.rules.military, Menace::Ruthless);
        assert!(!s.needs_restart());
        act(&mut w, "fv.rule:economy=hard");
        act(&mut w, "fv.seat.level:0:admiral");
        let s = w.resource::<Setup>().clone();
        assert_eq!(s.rules.economy, Hardship::Hard);
        assert!(s.needs_restart());
        let f = s.flags();
        assert_eq!(f[0], "--no-title");
        let picks = f.iter().position(|x| x == "--rival-picks").unwrap();
        let back = RivalPicks::parse(&f[picks + 1], Level::Captain).unwrap();
        assert_eq!(back.0, s.picks());
        assert_eq!(back.0[0].1, Level::Admiral);
        let rules = f.iter().position(|x| x == "--rules").unwrap();
        let r = rules_from(&f[rules + 1], Rules::default());
        assert_eq!(r.economy, Hardship::Hard);
        assert_eq!(r.military, Menace::Ruthless);
        assert_eq!(r.resources, Bounty::Normal);
        act(&mut w, "fv.setup.defaults");
        assert!(!w.resource::<Setup>().needs_restart());
    }

    /// A kept setup comes back as the page's choice.
    #[test]
    fn a_kept_setup_comes_back() {
        let mut w = World::new();
        w.insert_resource(crate::sim::Economy::default());
        let kept = Kept {
            rival_picks: format!("{}:mate", characters::ALL[5].key),
            rules: String::new(),
        };
        init_with(&mut w, false, Some(kept));
        assert_eq!(
            w.resource::<Setup>().seats,
            [Some((5, Level::Mate)), None, None]
        );
    }

    /// A seat's line counts the islands its character holds at that level, not the level's
    /// bare cap, and only the first house can start in your waters.
    #[test]
    fn a_seat_note_reads_as_the_character_plays() {
        let rules = Rules::default();
        let vane = characters::find("vane").unwrap();
        let quill = characters::find("quill").unwrap();
        let v = seat_note(vane, Level::Captain, true, &rules);
        assert!(
            v.starts_with("Captain: a fair match; up to 5 islands"),
            "{v}"
        );
        let q = seat_note(quill, Level::Captain, true, &rules);
        assert!(q.contains("up to 2 islands"), "{q}");
        let near = seat_note(vane, Level::Commodore, true, &rules);
        assert!(near.ends_with("starts in your waters."), "{near}");
        let far = seat_note(vane, Level::Commodore, false, &rules);
        assert!(!far.contains("your waters"), "{far}");
        for ci in 0..characters::ALL.len() {
            for l in Level::ALL {
                let n = seat_note(ci, l, true, &rules);
                assert!(!n.contains("99"), "{n}");
            }
        }
    }

    /// Every character has words for how it plays, and every rule's choice shows a name and
    /// a line on what it does.
    #[test]
    fn every_character_and_rule_reads_on_the_page() {
        for c in characters::ALL.iter() {
            assert!(
                !trait_words(&c.traits).is_empty(),
                "{} has no trait words",
                c.leader
            );
        }
        for o in rules::OPTIONS.iter() {
            assert_eq!(o.choices.len(), o.help.len(), "{}", o.key);
            for v in o.choices {
                assert!(!shown(v).is_empty());
            }
        }
        assert_eq!(shown("free_traders"), "Free traders");
        assert_eq!(shown("on"), "On");
    }
}
