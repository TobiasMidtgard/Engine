//! Quests and the journal: a [`QuestLog`] of quests (plain JSON: who gave them, where they
//! lead, objectives and rewards) and the `journal` screen that lists them.
//!
//! The tracked quest drives the HUD's quest tracker (`quest.*` in [`UiData`]) and puts the
//! map waypoint on its current objective's place, so the compass, minimap and world markers
//! point there. Steps after the current one stay hidden until it is done, so a quest reveals
//! itself as it goes.
//!
//! Dialogue choices and buttons change quests with actions: `quest.accept:<id>`,
//! `quest.track:<id>`, `quest.advance:<id>` (finishes the current step),
//! `quest.complete:<id>` and `quest.fail:<id>`. AI agents use the `quest.*` API.
//!
//! ```ignore
//! ui::show(world, "journal", &json!({"select": "silent_relay"}))?;
//! call_api(world, "quest.progress", json!({"id": "glass_and_glow", "add": 1}))?;
//! ```

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_input::{ButtonInput, GamepadButton, KeyCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::el::{self, El};
use crate::input::UiEvent;
use crate::inventory::{self, group, item_slot};
use crate::items::*;
use crate::map::{MapData, MapItem, MapView};
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Overflow, Pick, Position, Shadow, pct};
use crate::theme::{Palette, Theme};
use crate::toast::{self, ToastKind};
use crate::widgets;

// ----------------------------------------------------------------------------------- data

/// What sort of quest it is (sets its color, icon and group in the journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestKind {
    #[default]
    Main,
    Side,
    Contract,
}

impl QuestKind {
    pub const ALL: [QuestKind; 3] = [QuestKind::Main, QuestKind::Side, QuestKind::Contract];

    pub fn label(self) -> &'static str {
        match self {
            QuestKind::Main => "Main story",
            QuestKind::Side => "Side quest",
            QuestKind::Contract => "Contract",
        }
    }

    /// Heading of the journal group.
    pub fn group(self) -> &'static str {
        match self {
            QuestKind::Main => "MAIN STORY",
            QuestKind::Side => "SIDE QUESTS",
            QuestKind::Contract => "CONTRACTS",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            QuestKind::Main => "marker_quest",
            QuestKind::Side => "quest_side",
            QuestKind::Contract => "quest_contract",
        }
    }

    pub fn color(self, p: &Palette) -> Color {
        match self {
            QuestKind::Main => p.accent,
            QuestKind::Side => p.info,
            QuestKind::Contract => Color::hex(0xff7a5c),
        }
    }
}

/// Where a quest stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestState {
    /// Known but not taken yet (hidden from the journal until `quest.accept`).
    Offered,
    #[default]
    Active,
    Completed,
    Failed,
}

/// One step of a quest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Objective {
    pub text: String,
    /// How many are needed (1 for a plain step) and how many are done.
    pub count: u32,
    pub progress: u32,
    /// Optional steps don't hold the quest back.
    pub optional: bool,
    /// The map place (a [`Poi`](crate::map::Poi) id) this step happens at.
    pub place: String,
}

impl Default for Objective {
    fn default() -> Self {
        Self {
            text: String::new(),
            count: 1,
            progress: 0,
            optional: false,
            place: String::new(),
        }
    }
}

impl Objective {
    pub fn done(&self) -> bool {
        self.progress >= self.count.max(1)
    }
}

/// What finishing a quest gives.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rewards {
    pub xp: u32,
    pub gold: u64,
    pub items: Vec<ItemStack>,
}

/// A quest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Quest {
    pub id: String,
    pub title: String,
    pub kind: QuestKind,
    pub state: QuestState,
    /// Recommended level.
    pub level: u32,
    /// Who gave it: name, title and portrait icon (`portrait:merchant`).
    pub giver: String,
    pub giver_title: String,
    pub giver_portrait: String,
    /// Where it takes place, e.g. "Veyra · Northern Ridge".
    pub region: String,
    pub summary: String,
    pub objectives: Vec<Objective>,
    pub rewards: Rewards,
    /// Not looked at in the journal yet.
    pub new: bool,
}

impl Quest {
    /// The step being worked on: the first unfinished required objective.
    pub fn current(&self) -> Option<usize> {
        self.objectives
            .iter()
            .position(|o| !o.optional && !o.done())
    }

    /// How many objectives the journal shows: every one up to the current step while the
    /// quest is active, all of them once it is finished.
    pub fn visible(&self) -> usize {
        match (self.state, self.current()) {
            (QuestState::Active | QuestState::Offered, Some(i)) => i + 1,
            _ => self.objectives.len(),
        }
    }

    /// Required steps done and in total.
    pub fn steps(&self) -> (u32, u32) {
        let req = self.objectives.iter().filter(|o| !o.optional);
        let total = req.clone().count() as u32;
        (req.filter(|o| o.done()).count() as u32, total)
    }

    /// The place the quest leads to now: the current step's, else the latest shown step's.
    pub fn place(&self) -> Option<&str> {
        let shown = &self.objectives[..self.visible()];
        if let Some(i) = self.current()
            && !shown[i].place.is_empty()
        {
            return Some(&shown[i].place);
        }
        shown
            .iter()
            .rev()
            .find(|o| !o.place.is_empty())
            .map(|o| o.place.as_str())
    }

    /// What the HUD's quest tracker shows (`quest.*`): the current step, and its count when
    /// it has one, else the steps done.
    pub fn hud(&self) -> Value {
        let cur = self.current();
        let step = cur
            .map(|i| self.objectives[i].text.clone())
            .unwrap_or_else(|| "Complete".into());
        let (progress, max) = match cur.map(|i| &self.objectives[i]) {
            Some(o) if o.count > 1 => (o.progress, o.count),
            _ => self.steps(),
        };
        json!({
            "id": self.id,
            "title": self.title,
            "kind": self.kind.label(),
            "step": step,
            "progress": progress,
            "progress_max": max,
            "region": self.region,
        })
    }
}

/// Every quest the player knows, and which one is tracked.
#[derive(Resource, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QuestLog {
    pub quests: Vec<Quest>,
    pub tracked: Option<String>,
}

impl QuestLog {
    pub fn get(&self, id: &str) -> Option<&Quest> {
        self.quests.iter().find(|q| q.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Quest> {
        self.quests.iter_mut().find(|q| q.id == id)
    }

    /// The tracked quest, while it is active.
    pub fn tracked_quest(&self) -> Option<&Quest> {
        self.tracked
            .as_deref()
            .and_then(|id| self.get(id))
            .filter(|q| q.state == QuestState::Active)
    }

    /// Quests the journal lists on a tab (active, or finished), main story first.
    pub fn listed(&self, finished: bool) -> Vec<&Quest> {
        let mut v: Vec<&Quest> = self
            .quests
            .iter()
            .filter(|q| match q.state {
                QuestState::Offered => false,
                QuestState::Active => !finished,
                QuestState::Completed | QuestState::Failed => finished,
            })
            .collect();
        v.sort_by_key(|q| q.kind as u8);
        v
    }
}

fn need<'a>(log: &'a mut QuestLog, id: &str) -> Result<&'a mut Quest, String> {
    let known: Vec<String> = log.quests.iter().map(|q| q.id.clone()).collect();
    log.get_mut(id).ok_or_else(|| {
        format!(
            "There is no quest \"{id}\". Known quests: {}.",
            known.join(", ")
        )
    })
}

fn with_log<R>(
    world: &mut World,
    f: impl FnOnce(&mut QuestLog) -> Result<R, String>,
) -> Result<R, String> {
    let mut log = world
        .get_resource_mut::<QuestLog>()
        .ok_or("The UI plugin isn't running (no QuestLog).")?;
    f(&mut log)
}

/// Take an offered quest (it appears in the journal, marked new, and is tracked when nothing
/// else is).
pub fn accept(world: &mut World, id: &str) -> Result<(), String> {
    let title = with_log(world, |log| {
        let q = need(log, id)?;
        if q.state != QuestState::Offered {
            return Ok(None);
        }
        q.state = QuestState::Active;
        q.new = true;
        let title = q.title.clone();
        if log.tracked_quest().is_none() {
            log.tracked = Some(id.to_string());
        }
        Ok(Some(title))
    })?;
    if let Some(t) = title {
        toast::notify(
            world,
            ToastKind::Good,
            "marker_quest",
            "New quest",
            &format!("[b]{t}[/b]"),
            3.0,
        );
    }
    Ok(())
}

/// Track a quest (or nothing): the HUD tracker shows it and the waypoint follows it.
pub fn track(world: &mut World, id: Option<&str>) -> Result<(), String> {
    let title = with_log(world, |log| match id {
        Some(id) => {
            let q = need(log, id)?;
            if q.state != QuestState::Active {
                return Err(format!(
                    "\"{}\" isn't active, so it can't be tracked.",
                    q.title
                ));
            }
            let t = q.title.clone();
            log.tracked = Some(id.to_string());
            Ok(Some(t))
        }
        None => {
            log.tracked = None;
            Ok(None)
        }
    })?;
    match title {
        Some(t) => toast::notify(
            world,
            ToastKind::Info,
            "marker_waypoint",
            "Tracking",
            &format!("[b]{t}[/b]"),
            2.4,
        ),
        None => toast::notify(
            world,
            ToastKind::Info,
            "marker_quest",
            "Not tracking a quest",
            "",
            2.0,
        ),
    }
    Ok(())
}

/// Track `id`, or stop tracking it when it already is.
pub fn toggle_track(world: &mut World, id: &str) -> Result<(), String> {
    let tracked = world
        .get_resource::<QuestLog>()
        .is_some_and(|l| l.tracked.as_deref() == Some(id));
    track(world, (!tracked).then_some(id))
}

/// Set an objective's progress (`objective` index, default the current step) to `set`, or
/// add `add` to it. Finishing the last required step completes the quest. Returns the
/// objective after the change.
pub fn progress(
    world: &mut World,
    id: &str,
    objective: Option<usize>,
    set: Option<u32>,
    add: i64,
) -> Result<Objective, String> {
    let (obj, newly_done, finished) = with_log(world, |log| {
        let q = need(log, id)?;
        if q.state != QuestState::Active {
            return Err(format!("\"{}\" isn't active.", q.title));
        }
        let i = match objective {
            Some(i) if i < q.objectives.len() => i,
            Some(i) => {
                return Err(format!(
                    "\"{}\" has {} objectives (0-{}), not {i}.",
                    q.title,
                    q.objectives.len(),
                    q.objectives.len().saturating_sub(1)
                ));
            }
            None => q.current().ok_or("Every step is done already.")?,
        };
        let o = &mut q.objectives[i];
        let was = o.done();
        let count = o.count.max(1);
        o.progress = set
            .unwrap_or_else(|| (o.progress as i64 + add).clamp(0, count as i64) as u32)
            .min(count);
        let obj = o.clone();
        Ok((obj.clone(), !was && obj.done(), q.current().is_none()))
    })?;
    if newly_done && !finished {
        toast::notify(
            world,
            ToastKind::Good,
            "ui_check",
            "Objective complete",
            &obj.text,
            2.8,
        );
    }
    if finished {
        complete(world, id, false)?;
    }
    Ok(obj)
}

/// Finish the current step.
pub fn advance(world: &mut World, id: &str) -> Result<(), String> {
    progress(world, id, None, Some(u32::MAX), 0).map(|_| ())
}

/// Complete (or fail) a quest. Completing gives its rewards to the player (gold and items to
/// the Player's inventory, XP to `player.xp`). A tracked quest hands tracking to the next
/// active one.
pub fn complete(world: &mut World, id: &str, failed: bool) -> Result<(), String> {
    let q = with_log(world, |log| {
        let q = need(log, id)?;
        if matches!(q.state, QuestState::Completed | QuestState::Failed) {
            return Ok(None);
        }
        q.state = if failed {
            QuestState::Failed
        } else {
            QuestState::Completed
        };
        if !failed {
            for o in q.objectives.iter_mut().filter(|o| !o.optional) {
                o.progress = o.count.max(1);
            }
        }
        let q = q.clone();
        if log.tracked.as_deref() == Some(id) {
            log.tracked = log
                .listed(false)
                .iter()
                .find(|n| n.kind == q.kind)
                .or(log.listed(false).first())
                .map(|n| n.id.clone());
        }
        Ok(Some(q))
    })?;
    let Some(q) = q else {
        return Ok(());
    };
    if failed {
        toast::notify(
            world,
            ToastKind::Bad,
            q.kind.icon(),
            "Quest failed",
            &format!("[b]{}[/b]", q.title),
            3.2,
        );
        return Ok(());
    }
    let r = &q.rewards;
    if let Ok(owner) = inventory::find_owner(world, &json!({})) {
        let db = world.resource::<ItemDatabase>().clone();
        if let Some(mut inv) = world.get_mut::<Inventory>(owner) {
            inv.gold += r.gold;
            for s in &r.items {
                inv.add(&db, &s.item, s.count);
            }
        }
    }
    if r.xp > 0
        && let Some(mut data) = world.get_resource_mut::<UiData>()
        && let Some(xp) = data.get_f64("player.xp")
    {
        data.set("player.xp", xp + r.xp as f64);
    }
    let mut got = vec![format!("[b]{}[/b]", q.title)];
    if r.xp > 0 {
        got.push(format!("+{} XP", group(r.xp as u64)));
    }
    if r.gold > 0 {
        got.push(format!("+{} gold", group(r.gold)));
    }
    toast::notify(
        world,
        ToastKind::Loot,
        q.kind.icon(),
        "Quest complete",
        &got.join("  ·  "),
        4.0,
    );
    Ok(())
}

// --------------------------------------------------------------------------------- sync

/// What was last written to the HUD data and the waypoint.
#[derive(Default)]
pub struct Synced {
    log: Option<QuestLog>,
    hud: Option<Value>,
    waypoint: Option<(String, Option<usize>)>,
}

/// Mirror the tracked quest into `quest.*` and point the waypoint at its current place.
/// Only changes are written, so a game that fills `quest.*` itself (without a QuestLog) is
/// left alone.
fn sync(world: &mut World, s: &mut Synced) {
    let Some(log) = world.get_resource::<QuestLog>() else {
        return;
    };
    if s.log.as_ref() == Some(log) {
        return;
    }
    let log = log.clone();
    let tracked = log.tracked_quest();
    let hud = tracked.map(Quest::hud);
    if hud != s.hud
        && let Some(mut data) = world.get_resource_mut::<UiData>()
    {
        match &hud {
            Some(Value::Object(v)) => {
                for (k, val) in v {
                    data.set(&format!("quest.{k}"), val.clone());
                }
            }
            _ => {
                for k in ["id", "title", "kind", "step", "region"] {
                    data.set(&format!("quest.{k}"), "");
                }
                data.set("quest.progress", 0);
                data.set("quest.progress_max", 0);
            }
        }
    }
    s.hud = hud;
    let key = tracked.map(|q| (q.id.clone(), q.current()));
    if key != s.waypoint {
        if let Some(place) = tracked.and_then(Quest::place)
            && let Some(mut map) = world.get_resource_mut::<MapData>()
        {
            crate::map::set_waypoint(&mut map, place);
        }
        s.waypoint = key;
    }
    s.log = Some(log);
}

// ------------------------------------------------------------------------------- screen

/// State of the open journal (on its root).
#[derive(Component, Debug, Clone, Default)]
pub struct JournalScreen {
    /// 0 = active quests, 1 = finished ones.
    pub tab: usize,
    pub selected: Option<String>,
    pub snapshot: Option<QuestLog>,
}

fn player_level(world: &World) -> u32 {
    world
        .get_resource::<UiData>()
        .and_then(|d| d.get_f64("player.level"))
        .unwrap_or(1.0) as u32
}

/// Distance from the player to a place on their planet, as text.
fn distance_to(map: &MapData, place: &str) -> Option<String> {
    let (pl, poi) = map.poi(place)?;
    if pl.id != map.player.planet {
        return Some(pl.name.clone());
    }
    let (d, _) = crate::map::great_circle(map.player.lon, map.player.lat, poi.lon, poi.lat);
    let m = d * pl.radius_km * 1000.0;
    Some(if m < 100.0 {
        "You are here".into()
    } else {
        crate::hud::format_distance(m)
    })
}

fn kind_tile(p: &Palette, kind: QuestKind, size: f32, dim: bool) -> El {
    let c = kind.color(p);
    El::new()
        .with_style(|s| {
            s.size(size, size)
                .shrink(0.0)
                .center()
                .radius(Corners::all(size * 0.26))
                .background(Fill::Radial {
                    inner: c.fade(if dim { 0.12 } else { 0.3 }),
                    outer: c.fade(0.04),
                })
                .border(1.0, c.fade(if dim { 0.25 } else { 0.55 }))
        })
        .child(
            El::icon(kind.icon())
                .with_style(|s| {
                    s.size(size * 0.56, size * 0.56)
                        .opacity(if dim { 0.55 } else { 1.0 })
                })
                .pick_ignore(),
        )
        .pick_ignore()
}

fn row(theme: &Theme, map: &MapData, level: u32, q: &Quest, selected: bool, tracked: bool) -> El {
    let p = &theme.palette;
    let finished = q.state != QuestState::Active;
    let place = q
        .place()
        .and_then(|id| map.poi(id).map(|(_, poi)| poi.name.clone()));
    let sub = match q.state {
        QuestState::Completed => "Completed".to_string(),
        QuestState::Failed => "Failed".to_string(),
        _ => match (place, q.place().and_then(|id| distance_to(map, id))) {
            (Some(name), Some(d)) => format!("{name}  ·  {d}"),
            (Some(name), None) => name,
            _ => q.region.clone(),
        },
    };
    let sub_color = match q.state {
        QuestState::Completed => p.good.fade(0.85),
        QuestState::Failed => p.bad,
        _ if selected => p.text_dim,
        _ => p.text_faint,
    };
    let hard = !finished && q.level > level + 2;
    let mut right = El::new().class("row").with_style(|s| s.gap(8.0));
    if hard {
        right = right.child(
            El::text(format!("Lv {}", q.level))
                .class("chip")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.color(p.warn).border_color(p.warn.fade(0.5)))
                .tooltip(
                    "",
                    format!("Recommended level {} (you are {level})", q.level),
                ),
        );
    }
    if tracked {
        right = right.child(
            El::icon("marker_waypoint")
                .with_style(|s| s.size(18.0, 18.0))
                .pick_ignore(),
        );
    }
    if q.new {
        right = right.child(
            El::new()
                .with_style(|s| {
                    s.size(8.0, 8.0)
                        .radius(Corners::all(4.0))
                        .bg(p.accent_light)
                        .shadow(Shadow::glow(p.accent.fade(0.8), 8.0))
                })
                .pick_ignore(),
        );
    }
    El::new()
        .class("list_item")
        .id(format!("quest:{}", q.id))
        .on_click(format!("journal.select:{}", q.id))
        .selected(selected)
        .tooltip(
            "",
            if finished {
                q.title.clone()
            } else {
                format!(
                    "[b]{}[/b]\n[color=#8a94a8]Right click (X) to {}[/color]",
                    q.title,
                    if tracked { "stop tracking" } else { "track" }
                )
            },
        )
        .with_style(|s| s.padding([9.0, 12.0]))
        .child(kind_tile(p, q.kind, 40.0, finished))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0))
                .pick_ignore()
                .child(
                    El::text(q.title.clone())
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|s| {
                            s.font_size(16.0)
                                .color(if finished { p.text_dim } else { p.text })
                        }),
                )
                .child(
                    El::text(sub)
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(13.0).font_weight(600).color(sub_color)),
                ),
        )
        .child(right.pick_ignore())
}

fn list(world: &World, st: &JournalScreen) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
    let log = world
        .get_resource::<QuestLog>()
        .cloned()
        .unwrap_or_default();
    let level = player_level(world);
    let quests = log.listed(st.tab == 1);
    let mut col = El::new().class("col").with_style(|s| s.gap(4.0));
    for kind in QuestKind::ALL {
        let group: Vec<&&Quest> = quests.iter().filter(|q| q.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        let c = kind.color(p);
        col = col.child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).padding([10.0, 6.0, 4.0, 6.0]))
                .child(El::new().with_style(|s| {
                    s.size(7.0, 7.0)
                        .radius(Corners::all(1.5))
                        .rotate_deg(45.0)
                        .bg(c)
                }))
                .child(
                    El::text(kind.group())
                        .class("label_caps")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.color(c.lerp(p.text, 0.25))),
                )
                .child(
                    El::new()
                        .class("divider")
                        .with_style(|s| s.grow(1.0).width(pct(10.0))),
                )
                .child(El::text(group.len().to_string()).class("caption")),
        );
        for q in group {
            col = col.child(row(
                theme,
                &map,
                level,
                q,
                st.selected.as_deref() == Some(&q.id),
                log.tracked.as_deref() == Some(&q.id),
            ));
        }
    }
    if quests.is_empty() {
        col = col.child(
            El::text(if st.tab == 1 {
                "No finished quests yet."
            } else {
                "No quests. Talk to people."
            })
            .class("dim")
            .with_style(|s| s.padding(12.0)),
        );
    }
    El::new()
        .class("panel")
        .id("journal_list")
        .with_style(|s| s.width(430.0).padding([14.0, 12.0]))
        .child(
            El::new()
                .class("scroll")
                .id("journal_scroll")
                .with_style(|s| s.grow(1.0).padding([0.0, 4.0, 0.0, 0.0]))
                .child(col),
        )
}

fn objective_row(p: &Palette, q: &Quest, i: usize, o: &Objective, map: &MapData) -> El {
    let current = q.state == QuestState::Active && q.current() == Some(i);
    let failed = q.state == QuestState::Failed && !o.done();
    let done = o.done();
    let mark = if done {
        El::new()
            .with_style(|s| {
                s.size(20.0, 20.0)
                    .shrink(0.0)
                    .center()
                    .radius(Corners::all(10.0))
                    .background(p.good.fade(0.18))
                    .border(1.5, p.good.fade(0.85))
            })
            .child(
                El::icon("ui_check")
                    .icon_tint(p.good)
                    .with_style(|s| s.size(11.0, 11.0)),
            )
    } else if failed {
        El::new()
            .with_style(|s| {
                s.size(20.0, 20.0)
                    .shrink(0.0)
                    .center()
                    .radius(Corners::all(10.0))
                    .background(p.bad.fade(0.16))
                    .border(1.5, p.bad.fade(0.8))
            })
            .child(
                El::icon("ui_close")
                    .icon_tint(p.bad)
                    .with_style(|s| s.size(9.0, 9.0)),
            )
    } else if current {
        El::new()
            .with_style(|s| s.size(20.0, 20.0).shrink(0.0).center())
            .child(El::new().with_style(|s| {
                s.size(11.0, 11.0)
                    .radius(Corners::all(2.0))
                    .rotate_deg(45.0)
                    .background(Fill::vertical(p.accent_light, p.accent))
                    .shadow(Shadow::glow(p.accent.fade(0.7), 10.0))
            }))
    } else {
        El::new().with_style(|s| {
            s.size(20.0, 20.0)
                .shrink(0.0)
                .radius(Corners::all(10.0))
                .border(1.5, p.text_faint.fade(0.8))
        })
    };
    let text_color = if current {
        p.text
    } else if done {
        p.text_faint
    } else {
        p.text_dim
    };
    let mut text = El::new()
        .class("row")
        .with_style(|s| s.gap(8.0).grow(1.0).shrink(1.0))
        .child(
            El::text(o.text.clone())
                .text_style(|t| t.line_height(1.3))
                .with_style(|s| {
                    s.font_size(15.0)
                        .font_weight(if current { 600 } else { 400 })
                        .color(text_color)
                        .shrink(1.0)
                }),
        );
    if o.optional {
        text = text.child(
            El::text("Optional")
                .class("chip")
                .text_style(|t| t.nowrap()),
        );
    }
    let mut row = El::new()
        .class("row")
        .id(format!("objective:{i}"))
        .with_style(|s| {
            let s = s.gap(12.0).padding([6.0, 10.0]).radius(Corners::all(9.0));
            if current {
                s.background(Fill::Linear {
                    from: p.accent.fade(0.14),
                    to: p.accent.fade(0.02),
                    angle_deg: 0.0,
                })
                .border(1.0, p.accent.fade(0.32))
            } else {
                s.border(1.0, Color::TRANSPARENT)
            }
        })
        .child(mark)
        .child(text);
    if o.count > 1 {
        let frac = o.progress as f32 / o.count as f32;
        row = row.child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0))
                .child(
                    El::new()
                        .class("bar_track")
                        .with_style(|s| s.width(84.0).height(8.0))
                        .paint("bar")
                        .attach(
                            "Bar",
                            &widgets::Bar {
                                value: frac,
                                color: if done {
                                    p.good.scale(0.7)
                                } else {
                                    p.accent_dark
                                },
                                color2: if done { p.good } else { p.accent },
                                segments: o.count.min(12),
                                ..Default::default()
                            },
                        ),
                )
                .child(
                    El::text(format!("{} / {}", o.progress.min(o.count), o.count))
                        .text_style(|t| t.tabular().nowrap())
                        .with_style(|s| {
                            s.font_size(13.0)
                                .font_weight(700)
                                .color(if done { p.good } else { p.text_dim })
                                .min_width(40.0)
                        }),
                ),
        );
    }
    if let Some((_, poi)) = (!o.place.is_empty()).then(|| map.poi(&o.place)).flatten() {
        row = row.child(
            El::new()
                .class("icon_button")
                .id(format!("objective_map:{i}"))
                .on_click(format!("journal.map:{}", o.place))
                .tooltip("", format!("Show [b]{}[/b] on the map", poi.name))
                .with_style(|s| s.size(28.0, 28.0).padding(5.0).shrink(0.0))
                .child(
                    El::icon(poi.kind.icon())
                        .with_style(|s| s.size(18.0, 18.0))
                        .pick_ignore(),
                ),
        );
    }
    row
}

fn reward_tile(p: &Palette, icon: &str, value: String, label: &str) -> El {
    El::new()
        .class("row")
        .with_style(|s| {
            s.gap(10.0)
                .padding([6.0, 16.0, 6.0, 10.0])
                .height(50.0)
                .radius(Corners::all(10.0))
                .background(Fill::Radial {
                    inner: p.accent.fade(0.12),
                    outer: p.well,
                })
                .border(1.0, p.accent.fade(0.3))
        })
        .child(
            El::icon(icon)
                .with_style(|s| s.size(26.0, 26.0))
                .pick_ignore(),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text(value)
                        .text_style(|t| t.tabular().nowrap())
                        .with_style(|s| s.font_size(17.0).font_weight(700).color(p.accent_light)),
                )
                .child(
                    El::text(label)
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(12.0).font_weight(600).color(p.text_dim)),
                ),
        )
}

fn details(world: &World, st: &JournalScreen) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let db = world.resource::<ItemDatabase>();
    let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
    let log = world
        .get_resource::<QuestLog>()
        .cloned()
        .unwrap_or_default();
    let panel = El::new()
        .class("panel")
        .id("journal_details")
        .with_style(|s| s.width(792.0));
    let Some(q) = st.selected.as_deref().and_then(|id| log.get(id)) else {
        return panel.child(
            El::new()
                .class("col center")
                .with_style(|s| s.grow(1.0))
                .child(El::icon("scroll").with_style(|s| s.size(72.0, 72.0).opacity(0.5)))
                .child(El::text("Pick a quest to read about it.").class("dim")),
        );
    };
    let c = q.kind.color(p);
    let tracked = log.tracked.as_deref() == Some(&q.id);
    let level = player_level(world);

    // The place, live from the map, with its name and distance over the corner.
    let active = q.state == QuestState::Active;
    let place = q
        .place()
        .and_then(|id| map.poi(id).map(|(_, poi)| (id, poi.clone())));
    let mut banner = El::new().id("journal_banner").with_style(|s| {
        s.width(pct(100.0))
            .height(150.0)
            .shrink(0.0)
            .radius(Corners::all(12.0))
            .overflow(Overflow::Hidden)
            .background(Fill::vertical(c.fade(0.22), p.well))
            .border(1.0, c.fade(0.35))
    });
    match &place {
        Some((id, poi)) => {
            banner = banner
                .child(
                    El::new()
                        .id("journal_map")
                        .paint("map")
                        .with_style(|s| s.fill_parent().pick(Pick::Ignore)),
                )
                .child(
                    El::new()
                        .class("hud_panel row")
                        .id("journal_place")
                        .with_style(|s| {
                            s.position(Position::Absolute)
                                .left(12.0)
                                .bottom(12.0)
                                .gap(10.0)
                                .padding([7.0, 16.0, 7.0, 10.0])
                                .radius(Corners::all(12.0))
                        })
                        .child(
                            El::icon(poi.kind.icon())
                                .with_style(|s| s.size(20.0, 20.0))
                                .pick_ignore(),
                        )
                        .child(
                            El::new()
                                .class("col")
                                .with_style(|s| s.gap(1.0))
                                .child(
                                    El::new()
                                        .class("row")
                                        .with_style(|s| s.gap(10.0))
                                        .child(
                                            El::text(poi.name.clone())
                                                .text_style(|t| t.semibold().nowrap().shadow())
                                                .with_style(|s| s.font_size(15.0).color(p.text)),
                                        )
                                        .child_opt(distance_to(&map, id).filter(|_| active).map(
                                            |d| {
                                                El::text(d)
                                                    .text_style(|t| t.tabular().nowrap().shadow())
                                                    .with_style(|s| {
                                                        s.font_size(14.0)
                                                            .font_weight(700)
                                                            .color(p.accent_light)
                                                    })
                                            },
                                        )),
                                )
                                .child_opt((!q.region.is_empty()).then(|| {
                                    El::text(q.region.clone())
                                        .text_style(|t| t.nowrap().shadow())
                                        .with_style(|s| {
                                            s.font_size(12.0).font_weight(600).color(p.text_dim)
                                        })
                                })),
                        ),
                );
        }
        None => {
            banner = banner.with_style(|s| s.center()).child(
                El::icon(q.kind.icon())
                    .with_style(|s| s.size(84.0, 84.0).opacity(0.85))
                    .pick_ignore(),
            );
        }
    }

    // Title, kind and level, and who gave it.
    let state_chip = match q.state {
        QuestState::Completed => Some(("Completed", p.good)),
        QuestState::Failed => Some(("Failed", p.bad)),
        _ if tracked => Some(("Tracked", p.accent_light)),
        _ => None,
    };
    let level_color = if q.state == QuestState::Active && q.level > level + 2 {
        p.warn
    } else {
        p.text_faint
    };
    let titles = El::new()
        .class("col")
        .with_style(|s| s.gap(4.0).grow(1.0).shrink(1.0))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0))
                .child(
                    El::text(q.kind.label().to_uppercase())
                        .class("label_caps")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.color(c)),
                )
                .child_opt((q.level > 0).then(|| {
                    El::text(format!("LEVEL {}", q.level))
                        .class("label_caps")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.color(level_color))
                }))
                .child_opt(state_chip.map(|(t, col)| {
                    El::text(t)
                        .class("chip")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| {
                            s.color(col)
                                .border_color(col.fade(0.5))
                                .background(col.fade(0.1))
                        })
                })),
        )
        .child(El::text(q.title.clone()).class("title"));
    let portrait = if q.giver_portrait.is_empty() {
        El::icon("marker_npc").with_style(|s| s.size(30.0, 30.0))
    } else {
        El::icon(q.giver_portrait.clone())
            .with_style(|s| s.size(58.0, 72.0).margin([18.0, 0.0, 0.0, 0.0]))
    };
    let giver = (!q.giver.is_empty()).then(|| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(12.0).align_items(Align::Center))
            .child(
                El::new()
                    .class("col")
                    .with_style(|s| s.gap(2.0).align_items(Align::End))
                    .child(
                        El::text("GIVEN BY")
                            .class("label_caps")
                            .text_style(|t| t.nowrap()),
                    )
                    .child(
                        El::text(q.giver.clone())
                            .text_style(|t| t.semibold().nowrap())
                            .with_style(|s| s.font_size(15.0).color(p.text)),
                    )
                    .child_opt((!q.giver_title.is_empty()).then(|| {
                        El::text(q.giver_title.clone())
                            .class("caption")
                            .text_style(|t| t.nowrap())
                    })),
            )
            .child(
                El::new()
                    .with_style(|s| {
                        s.size(56.0, 56.0)
                            .shrink(0.0)
                            .radius(Corners::all(28.0))
                            .overflow(Overflow::Hidden)
                            .background(Fill::Radial {
                                inner: c.fade(0.35),
                                outer: Color::hex(0x10141e),
                            })
                            .border(2.0, c.fade(0.6))
                            .center()
                    })
                    .child(portrait.pick_ignore()),
            )
    });
    let head = El::new()
        .class("row")
        .with_style(|s| s.gap(16.0).align_items(Align::Start))
        .child(titles)
        .child_opt(giver);

    // Summary, objectives and rewards scroll together.
    let mut objectives = El::new().class("col").with_style(|s| s.gap(4.0));
    for (i, o) in q.objectives.iter().enumerate().take(q.visible()) {
        objectives = objectives.child(objective_row(p, q, i, o, &map));
    }
    let r = &q.rewards;
    let mut rewards = El::new().class("row").with_style(|s| s.gap(8.0));
    if r.xp > 0 {
        rewards = rewards.child(reward_tile(p, "star", group(r.xp as u64), "Experience"));
    }
    if r.gold > 0 {
        rewards = rewards.child(reward_tile(p, "coin", group(r.gold), "Gold"));
    }
    for s in &r.items {
        rewards = rewards.child(item_slot(theme, db, Some(s), 50.0));
    }
    let has_rewards = r.xp > 0 || r.gold > 0 || !r.items.is_empty();
    let caps = |t: &str| {
        El::text(t.to_string())
            .class("label_caps")
            .text_style(|t| t.nowrap())
            .with_style(|s| s.margin([4.0, 0.0, 0.0, 2.0]))
    };
    let (done, total) = q.steps();
    let body = El::new()
        .class("scroll")
        .id("journal_body")
        .with_style(|s| s.grow(1.0).gap(12.0).padding([0.0, 6.0, 0.0, 0.0]))
        .child(
            El::text(q.summary.clone())
                .class("body")
                .text_style(|t| t.line_height(1.45)),
        )
        .child(
            El::new()
                .class("row")
                .child(caps("OBJECTIVES"))
                .child(El::new().class("spacer"))
                .child(
                    El::text(format!("{done} of {total} steps"))
                        .class("caption")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.margin([4.0, 12.0, 0.0, 0.0])),
                ),
        )
        .child(objectives);

    // Rewards on the left, track and map buttons on the right.
    let mut foot = El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).align_items(Align::End));
    if has_rewards {
        foot = foot.child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(6.0))
                .child(caps("REWARDS"))
                .child(rewards),
        );
    }
    foot = foot.child(El::new().class("spacer"));
    if q.state == QuestState::Active {
        foot = foot.child(
            El::button(
                if tracked {
                    "Stop tracking"
                } else {
                    "Track quest"
                },
                "journal.track",
            )
            .id("journal_track")
            .class(if tracked { "ghost" } else { "primary" })
            .with_style(|s| s.min_width(170.0)),
        );
    }
    if place.is_some() {
        foot = foot.child(
            El::button("Show on map", "journal.map")
                .id("journal_show_map")
                .with_style(|s| s.min_width(150.0)),
        );
    }

    panel
        .child(banner)
        .child(head)
        .child(widgets::divider())
        .child(body)
        .child(foot)
}

fn hints() -> El {
    El::new()
        .class("hud_panel row")
        .with_style(|s| {
            s.gap(22.0)
                .padding([10.0, 18.0])
                .radius(Corners::all(999.0))
        })
        .child(widgets::key_hint("LMB", "A", "Select"))
        .child(widgets::key_hint("T", "X", "Track"))
        .child(widgets::key_hint("M", "Y", "Show on map"))
        .child(widgets::key_hint2(
            ["Q", "E"],
            ["LB", "RB"],
            "Active / Finished",
        ))
        .child(widgets::key_hint("Esc", "B", "Close"))
}

fn build(world: &World, st: &JournalScreen) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let log = world
        .get_resource::<QuestLog>()
        .cloned()
        .unwrap_or_default();
    let (active, finished) = (log.listed(false).len(), log.listed(true).len());
    let top = El::new()
        .class("row")
        .id("journal_top")
        .with_style(|s| s.gap(16.0).width(1240.0).padding([0.0, 4.0]))
        .child(
            El::new().class("header_icon").child(
                El::icon("marker_quest")
                    .with_style(|s| s.size(26.0, 26.0))
                    .pick_ignore(),
            ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0))
                .child(
                    El::text("Journal")
                        .class("title")
                        .text_style(|t| t.shadow()),
                )
                .child(
                    El::text(format!("{active} active  ·  {finished} finished"))
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| s.font_size(13.0).font_weight(600).color(p.info)),
                ),
        )
        .child(El::new().class("spacer"))
        .child(widgets::tabs(
            &["Active", "Finished"],
            st.tab,
            "journal.tab",
        ))
        .child(widgets::close_button());
    El::new()
        .with_style(|s| {
            s.fill_parent()
                .column()
                .center()
                .gap(16.0)
                .pick(Pick::Ignore)
        })
        .child(top)
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(18.0).align_items(Align::Stretch).height(720.0))
                .child(list(world, st))
                .child(details(world, st)),
        )
        .child(hints())
}

/// The banner's map: the surface around the quest's place, that place picked.
fn banner_view(map: &MapData, place: &str) -> Option<MapView> {
    let (pl, poi) = map.poi(place)?;
    let system = map
        .planet(&pl.id)
        .map(|(s, _)| s.id.clone())
        .unwrap_or_default();
    Some(MapView {
        zoom: -0.25,
        target: -0.25,
        system,
        planet: pl.id.clone(),
        lon: poi.lon,
        lat: poi.lat,
        selected: Some(MapItem::Poi(place.to_string())),
        idle: 0.0,
        ..Default::default()
    })
}

/// Spawn the journal's content under `root` (after the scrim) and give the banner its view.
fn fill(world: &mut World, root: Entity, st: &JournalScreen) -> Result<(), String> {
    let content = build(world, st);
    el::spawn(world, Some(root), &content)?;
    let log = world
        .get_resource::<QuestLog>()
        .cloned()
        .unwrap_or_default();
    let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
    if let Some(view) = st
        .selected
        .as_deref()
        .and_then(|id| log.get(id))
        .and_then(Quest::place)
        .and_then(|place| banner_view(&map, place))
        && let Some(canvas) = screens::find_in(world, root, "journal_map")
    {
        world.entity_mut(canvas).insert(view);
    }
    Ok(())
}

fn first_listed(log: &QuestLog, tab: usize) -> Option<String> {
    let listed = log.listed(tab == 1);
    // The tracked quest when it is on this tab, else the first one.
    listed
        .iter()
        .find(|q| log.tracked.as_deref() == Some(&q.id))
        .or(listed.first())
        .map(|q| q.id.clone())
}

fn mark_seen(world: &mut World, id: Option<&str>) {
    if let Some(id) = id
        && let Some(mut log) = world.get_resource_mut::<QuestLog>()
        && log.get(id).is_some_and(|q| q.new)
        && let Some(q) = log.get_mut(id)
    {
        q.new = false;
    }
}

pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let log = world
        .get_resource::<QuestLog>()
        .cloned()
        .unwrap_or_default();
    let selected = args
        .get("select")
        .and_then(Value::as_str)
        .filter(|id| log.get(id).is_some())
        .map(str::to_string);
    let tab = match args.get("tab") {
        Some(Value::String(t)) => (t == "finished") as usize,
        Some(v) => v.as_u64().unwrap_or(0).min(1) as usize,
        // The tab the selected quest is on.
        None => selected
            .as_deref()
            .and_then(|id| log.get(id))
            .is_some_and(|q| matches!(q.state, QuestState::Completed | QuestState::Failed))
            as usize,
    };
    let selected = selected.or_else(|| first_listed(&log, tab));
    mark_seen(world, selected.as_deref());
    let mut st = JournalScreen {
        tab,
        selected,
        snapshot: None,
    };
    let el = El::new()
        .class("screen")
        .child(El::new().with_style(|_| widgets::scrim(0.72)));
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 20,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    fill(world, root, &st)?;
    st.snapshot = world.get_resource::<QuestLog>().cloned();
    world.entity_mut(root).insert(st);
    Ok(root)
}

fn rebuild(world: &mut World, root: Entity) {
    let Some(st) = world.get::<JournalScreen>(root).cloned() else {
        return;
    };
    let keep = |world: &World, id: &str| {
        screens::find_in(world, root, id).and_then(|e| world.get::<ScrollState>(e).copied())
    };
    let list_scroll = keep(world, "journal_scroll");
    let children: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for c in children.iter().skip(1) {
        if let Ok(e) = world.get_entity_mut(*c) {
            e.despawn();
        }
    }
    if let Err(e) = fill(world, root, &st) {
        tracing::warn!("journal: {e}");
    }
    if let (Some(s), Some(e)) = (list_scroll, screens::find_in(world, root, "journal_scroll")) {
        world.entity_mut(e).insert(s);
    }
    let snap = world.get_resource::<QuestLog>().cloned();
    if let Some(mut s) = world.get_mut::<JournalScreen>(root) {
        s.snapshot = snap;
    }
}

fn open_map_at(world: &mut World, place: &str) {
    let r = screens::show(
        world,
        "map",
        &json!({"zoom": 0.3, "focus": true, "select": {"type": "poi", "id": place}}),
    );
    if let Err(e) = r {
        toast::notify(world, ToastKind::Bad, "", "", &e, 2.6);
    }
}

/// Quest actions from anywhere, the journal's input, and keeping the HUD and waypoint on the
/// tracked quest.
pub fn quests_system(
    world: &mut World,
    mut seen: Local<MessageCursor<UiEvent>>,
    mut synced: Local<Synced>,
) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    for ev in &events {
        if let UiEvent::Action { action, .. } = ev
            && let Some((verb, id)) = action
                .strip_prefix("quest.")
                .and_then(|s| s.split_once(':'))
        {
            let r = match verb {
                "accept" => accept(world, id),
                "track" => track(world, Some(id)),
                "advance" => advance(world, id),
                "complete" => complete(world, id, false),
                "fail" => complete(world, id, true),
                _ => Ok(()),
            };
            if let Err(e) = r {
                tracing::warn!("{action}: {e}");
            }
        }
    }
    journal_input(world, &events);
    sync(world, &mut synced);
    let log = world.get_resource::<QuestLog>().cloned();
    let mut q = world.query::<(Entity, &JournalScreen, &Screen)>();
    let stale: Vec<Entity> = q
        .iter(world)
        .filter(|(_, st, s)| !s.closing && st.snapshot != log)
        .map(|(e, ..)| e)
        .collect();
    for root in stale {
        rebuild(world, root);
    }
}

fn journal_input(world: &mut World, events: &[UiEvent]) {
    let mut q = world.query::<(Entity, &JournalScreen, &Screen)>();
    let open: Vec<(Entity, JournalScreen, String)> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, s)| (e, st.clone(), s.key.clone()))
        .collect();
    for (root, mut st, key) in open {
        let log = world
            .get_resource::<QuestLog>()
            .cloned()
            .unwrap_or_default();
        let before = (st.tab, st.selected.clone());
        let mut map_at: Option<String> = None;
        let mut toggle: Option<String> = None;
        for ev in events {
            match ev {
                UiEvent::Action {
                    action,
                    alt,
                    entity,
                    ..
                } => {
                    if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                        continue;
                    }
                    if let Some(id) = action.strip_prefix("journal.select:") {
                        if *alt {
                            toggle = Some(id.to_string());
                        } else {
                            st.selected = Some(id.to_string());
                        }
                    } else if let Some(t) = action
                        .strip_prefix("journal.tab:")
                        .and_then(|t| t.parse::<usize>().ok())
                    {
                        st.tab = t.min(1);
                    } else if action == "journal.track" {
                        toggle = st.selected.clone();
                    } else if action == "journal.map" {
                        map_at = st
                            .selected
                            .as_deref()
                            .and_then(|id| log.get(id))
                            .and_then(Quest::place)
                            .map(str::to_string);
                    } else if let Some(place) = action.strip_prefix("journal.map:") {
                        map_at = Some(place.to_string());
                    }
                }
                UiEvent::Tab { screen, .. } if *screen == key => st.tab = 1 - st.tab.min(1),
                _ => {}
            }
        }
        // Hotkeys while the journal is on top: T (or X on a pad, through the row) tracks, M
        // or Y shows the place on the map.
        let on_top = screens::open_screens(world)
            .into_iter()
            .find(|(k, ..)| !matches!(k.as_str(), "toasts" | "tooltip" | "drag"))
            .is_some_and(|(_, e, ..)| e == root);
        if on_top {
            let keys = world.get_resource::<ButtonInput<KeyCode>>();
            let pad = world.get_resource::<ButtonInput<GamepadButton>>();
            if keys.is_some_and(|k| k.just_pressed(KeyCode::T)) {
                toggle = st.selected.clone();
            }
            if keys.is_some_and(|k| k.just_pressed(KeyCode::M))
                || pad.is_some_and(|b| b.just_pressed(GamepadButton::North))
            {
                map_at = st
                    .selected
                    .as_deref()
                    .and_then(|id| log.get(id))
                    .and_then(Quest::place)
                    .map(str::to_string);
            }
        }
        if st.tab != before.0 {
            st.selected = first_listed(&log, st.tab);
        }
        if let Some(id) = toggle
            && let Err(e) = toggle_track(world, &id)
        {
            toast::notify(world, ToastKind::Bad, "", "", &e, 2.6);
        }
        let changed = (st.tab, st.selected.clone()) != before;
        if changed {
            mark_seen(world, st.selected.as_deref());
        }
        if let Some(mut s) = world.get_mut::<JournalScreen>(root) {
            s.tab = st.tab;
            s.selected = st.selected.clone();
        }
        if changed {
            rebuild(world, root);
        }
        if let Some(place) = map_at {
            open_map_at(world, &place);
        }
    }
}

// --------------------------------------------------------------------------------- demo

/// The demo quests: the Silent Relay (tracked) and what else is going on around Hollow
/// Reach, plus a few finished ones.
pub fn demo_quests() -> QuestLog {
    let o = |text: &str, place: &str, progress: u32, count: u32| Objective {
        text: text.into(),
        count,
        progress,
        optional: false,
        place: place.into(),
    };
    let opt = |text: &str, place: &str| Objective {
        optional: true,
        ..o(text, place, 0, 1)
    };
    let items = |list: &[(&str, u32)]| -> Vec<ItemStack> {
        list.iter().map(|(id, n)| ItemStack::new(id, *n)).collect()
    };
    let quests = vec![
        Quest {
            id: "silent_relay".into(),
            title: "The Silent Relay".into(),
            kind: QuestKind::Main,
            level: 14,
            giver: "Old Brannoc".into(),
            giver_title: "Trader of Hollow Reach".into(),
            giver_portrait: "portrait:merchant".into(),
            region: "Veyra  ·  Northern Ridge".into(),
            summary: "The relay on the northern ridge went silent nine days ago, though its lights still burn. The last crew that climbed up came back quiet, and every lamp in Hollow Reach flickers when the wind turns north. Brannoc would rather a stranger found out why.".into(),
            objectives: vec![
                o("Ask Old Brannoc about the relay", "hollow_reach", 1, 1),
                o("Buy a fusion fuel cell for the relay's backup generator", "hollow_reach", 1, 1),
                opt("Look over the ridge from Ridge Lookout", "ridge_lookout"),
                o("Reach the relay tower on Veyra's northern ridge", "relay_tower", 0, 1),
                o("Find what is drawing power from the relay", "relay_tower", 0, 1),
            ],
            rewards: Rewards { xp: 1_200, gold: 350, items: items(&[("amulet", 1), ("medkit", 2)]) },
            ..Default::default()
        },
        Quest {
            id: "glass_and_glow".into(),
            title: "Glass and Glow".into(),
            kind: QuestKind::Side,
            level: 12,
            giver: "Quill".into(),
            giver_title: "Cartographer of the Drift".into(),
            giver_portrait: "portrait:alien".into(),
            region: "Veyra  ·  Glass Caves".into(),
            summary: "Quill says the crystals growing in the Glass Caves hum at the same pitch as the silent relay. Three of them, unbroken, and it could chart where the signal is going.".into(),
            objectives: vec![
                o("Collect resonant crystals in the Glass Caves", "glass_caves", 2, 3),
                o("Bring the crystals to Quill", "hollow_reach", 0, 1),
            ],
            rewards: Rewards { xp: 600, gold: 0, items: items(&[("map_scroll", 1), ("compass", 1)]) },
            ..Default::default()
        },
        Quest {
            id: "fuel_for_rumors".into(),
            title: "Rumors for Fuel".into(),
            kind: QuestKind::Side,
            level: 13,
            giver: "Ostra Vale".into(),
            giver_title: "Miller of Ostra Mill".into(),
            region: "Veyra  ·  Ostra Mill".into(),
            summary: "Ostra Vale trades in rumors, and the going rate is fuel. She says she knows who climbed the relay before you, and what they carried down.".into(),
            objectives: vec![
                o("Carry two fusion fuel cells", "", 2, 2),
                o("Trade them to Ostra Vale at the mill", "old_mill", 0, 1),
            ],
            rewards: Rewards { xp: 350, gold: 0, items: items(&[("key", 1)]) },
            ..Default::default()
        },
        Quest {
            id: "lookout_log".into(),
            title: "The Lookout's Log".into(),
            kind: QuestKind::Side,
            level: 10,
            giver: "Veyra Survey Office".into(),
            region: "Veyra  ·  Ridge Lookout".into(),
            summary: "The survey station on the ridge stopped filing reports the same day the relay went quiet. The office wants the station's log back, and a look at whoever was keeping it.".into(),
            objectives: vec![o("Recover the survey log from Ridge Lookout", "ridge_lookout", 0, 1)],
            rewards: Rewards { xp: 250, gold: 120, items: Vec::new() },
            new: true,
            ..Default::default()
        },
        Quest {
            id: "storm_salvage".into(),
            title: "Storm Salvage".into(),
            kind: QuestKind::Contract,
            level: 18,
            giver: "Caldera Salvage Guild".into(),
            region: "Veyra  ·  The Scar".into(),
            summary: "Four ships went down in the storm fields this season. The guild pays per flight recorder and asks no questions about the rest of the wreck.".into(),
            objectives: vec![o("Recover flight recorders from the Scar", "scar", 1, 4)],
            rewards: Rewards { xp: 900, gold: 1_600, items: items(&[("circuit", 2)]) },
            ..Default::default()
        },
        Quest {
            id: "salt_run".into(),
            title: "Salt Run".into(),
            kind: QuestKind::Contract,
            level: 9,
            giver: "Saltmarsh Camp".into(),
            region: "Veyra  ·  Saltmarsh Flats".into(),
            summary: "The salt harvesters on the southern flats are out of medicine, and the next supply ship is a month away.".into(),
            objectives: vec![o("Deliver field medkits to Saltmarsh Camp", "south_camp", 0, 3)],
            rewards: Rewards { xp: 180, gold: 240, items: Vec::new() },
            ..Default::default()
        },
        Quest {
            id: "landfall".into(),
            title: "Landfall".into(),
            kind: QuestKind::Main,
            state: QuestState::Completed,
            level: 12,
            giver: "Harbormaster Ines Kade".into(),
            giver_title: "Caldera Port".into(),
            region: "Veyra  ·  Caldera Port".into(),
            summary: "You brought the Kestrel down inside a dead volcano with one engine and most of a hull. The harbormaster stamped your papers and pointed you north.".into(),
            objectives: vec![
                o("Bring the Kestrel down at Caldera Port", "caldera_port", 1, 1),
                o("Clear customs with the harbormaster", "caldera_port", 1, 1),
                o("Find a ride north to Hollow Reach", "hollow_reach", 1, 1),
            ],
            rewards: Rewards { xp: 400, gold: 150, items: Vec::new() },
            ..Default::default()
        },
        Quest {
            id: "traders_welcome".into(),
            title: "A Trader's Welcome".into(),
            kind: QuestKind::Side,
            state: QuestState::Completed,
            level: 12,
            giver: "Old Brannoc".into(),
            giver_title: "Trader of Hollow Reach".into(),
            giver_portrait: "portrait:merchant".into(),
            region: "Veyra  ·  Hollow Reach".into(),
            summary: "Nobody trades with a stranger in Hollow Reach until they have shared a drink at the ale corner.".into(),
            objectives: vec![o("Share a drink with Old Brannoc", "hollow_reach", 1, 1)],
            rewards: Rewards { xp: 100, gold: 0, items: items(&[("bread", 2)]) },
            ..Default::default()
        },
        Quest {
            id: "night_courier".into(),
            title: "Night Courier".into(),
            kind: QuestKind::Contract,
            state: QuestState::Failed,
            level: 11,
            giver: "Lumen Post".into(),
            region: "Veyra  ·  Lumen".into(),
            summary: "A sealed letter for the capital, to arrive before dawn. The storm over the Scar had other plans.".into(),
            objectives: vec![o("Deliver the sealed letter to Lumen before dawn", "lumen", 0, 1)],
            rewards: Rewards { xp: 0, gold: 500, items: Vec::new() },
            ..Default::default()
        },
    ];
    QuestLog {
        quests,
        tracked: Some("silent_relay".into()),
    }
}

/// Install the demo quests unless quests are already loaded.
pub fn add_demo_quests(world: &mut World) {
    let empty = world
        .get_resource::<QuestLog>()
        .is_none_or(|l| l.quests.is_empty());
    if empty {
        world.insert_resource(demo_quests());
    }
}

// ----------------------------------------------------------------------------------- API

fn id_arg(args: &Value) -> Result<&str, String> {
    args.get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Pass the quest's \"id\".".to_string())
}

/// `quest.*` APIs.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "quest.log",
        "Get every quest and the tracked one (no args), or replace the log (pass {quests, tracked}). A quest: {id, title, kind (main, side, contract), state (offered, active, completed, failed), level, giver, giver_title, giver_portrait, region, summary, objectives [{text, count, progress, optional, place (map place id)}], rewards {xp, gold, items [{item, count}]}}.",
        r#"{"tracked": "silent_relay", "quests": [{"id": "silent_relay", "title": "The Silent Relay", "objectives": [{"text": "Reach the relay tower", "place": "relay_tower"}]}]}"#,
        |world, args| {
            if args.get("quests").is_some() {
                let log: QuestLog = serde_json::from_value(args)
                    .map_err(|e| format!("That isn't a quest log: {e}."))?;
                let n = log.quests.len();
                world.insert_resource(log);
                return Ok(json!({"ok": true, "quests": n}));
            }
            let log = world.get_resource::<QuestLog>().cloned().unwrap_or_default();
            serde_json::to_value(&log).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "quest.add",
        "Add a quest (or replace the one with the same id). Same fields as in quest.log; `track: true` tracks it.",
        r#"{"id": "lost_drone", "title": "The Lost Drone", "kind": "side", "region": "Veyra · Glass Caves", "summary": "A survey drone went down near the caves.", "objectives": [{"text": "Find the drone", "place": "glass_caves"}], "rewards": {"xp": 200}, "track": true}"#,
        |world, args| {
            let q: Quest = serde_json::from_value(args.clone())
                .map_err(|e| format!("That isn't a quest: {e}."))?;
            if q.id.is_empty() {
                return Err("Give the quest an \"id\".".into());
            }
            let id = q.id.clone();
            with_log(world, |log| {
                match log.quests.iter_mut().find(|o| o.id == id) {
                    Some(old) => *old = q,
                    None => log.quests.push(q),
                }
                Ok(())
            })?;
            if args.get("track").and_then(Value::as_bool) == Some(true) {
                track(world, Some(&id))?;
            }
            Ok(json!({"ok": true, "id": id}))
        },
    );
    app.register_api(
        "quest.progress",
        "Move an objective on: {id, objective (index, default the current step), progress (set) or add (default 1)}. Finishing the last required step completes the quest and gives its rewards.",
        r#"{"id": "glass_and_glow", "add": 1}"#,
        |world, args| {
            let id = id_arg(&args)?.to_string();
            let objective = args
                .get("objective")
                .and_then(Value::as_u64)
                .map(|i| i as usize);
            let set = args
                .get("progress")
                .and_then(Value::as_u64)
                .map(|n| n as u32);
            let add = args.get("add").and_then(Value::as_i64).unwrap_or(1);
            let o = progress(world, &id, objective, set, add)?;
            let state = world
                .get_resource::<QuestLog>()
                .and_then(|l| l.get(&id))
                .map(|q| q.state);
            Ok(json!({"objective": o, "done": o.done(), "quest_state": state}))
        },
    );
    app.register_api(
        "quest.track",
        "Track a quest ({id}) or nothing ({id: null}). The HUD tracker shows it and the waypoint moves to its current place.",
        r#"{"id": "storm_salvage"}"#,
        |world, args| {
            let id = args.get("id").and_then(Value::as_str).map(str::to_string);
            track(world, id.as_deref())?;
            Ok(json!({"ok": true, "tracked": id}))
        },
    );
    app.register_api(
        "quest.accept",
        "Take an offered quest ({id}): it shows in the journal and is tracked when nothing else is.",
        r#"{"id": "lost_drone"}"#,
        |world, args| {
            accept(world, id_arg(&args)?)?;
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "quest.complete",
        "Complete a quest ({id}) and give its rewards, or fail it ({id, failed: true}).",
        r#"{"id": "salt_run", "failed": false}"#,
        |world, args| {
            let failed = args.get("failed").and_then(Value::as_bool).unwrap_or(false);
            complete(world, id_arg(&args)?, failed)?;
            Ok(json!({"ok": true}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_steps_stay_hidden_and_optional_ones_do_not_hold_the_quest() {
        let log = demo_quests();
        let q = log.get("silent_relay").unwrap();
        // Two done, an optional one skipped, the tower is current, the last is hidden.
        assert_eq!(q.current(), Some(3));
        assert_eq!(q.visible(), 4);
        assert_eq!(q.steps(), (2, 4));
        assert_eq!(q.place(), Some("relay_tower"));
        let hud = q.hud();
        assert_eq!(
            hud["step"],
            "Reach the relay tower on Veyra's northern ridge"
        );
        assert_eq!(
            (hud["progress"].as_u64(), hud["progress_max"].as_u64()),
            (Some(2), Some(4))
        );
    }

    #[test]
    fn a_counted_step_shows_its_count_on_the_hud() {
        let log = demo_quests();
        let hud = log.get("glass_and_glow").unwrap().hud();
        assert_eq!(
            (hud["progress"].as_u64(), hud["progress_max"].as_u64()),
            (Some(2), Some(3))
        );
    }

    #[test]
    fn a_step_without_a_place_leads_to_the_last_place_shown() {
        let mut q = demo_quests().get("fuel_for_rumors").unwrap().clone();
        q.objectives[1].place.clear();
        assert_eq!(q.place(), None);
        q.objectives[0].place = "old_mill".into();
        assert_eq!(q.place(), Some("old_mill"));
    }

    #[test]
    fn the_log_round_trips_through_json() {
        let log = demo_quests();
        let v = serde_json::to_value(&log).unwrap();
        assert_eq!(v["quests"][0]["kind"], "main");
        let back: QuestLog = serde_json::from_value(v).unwrap();
        assert_eq!(back, log);
        // Missing fields take their defaults.
        let q: Quest =
            serde_json::from_value(json!({"id": "x", "objectives": [{"text": "Go"}]})).unwrap();
        assert_eq!(q.state, QuestState::Active);
        assert_eq!(q.objectives[0].count, 1);
    }
}
