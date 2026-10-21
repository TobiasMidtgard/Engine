//! Two-person conversations: portraits on both sides that talk and blink, a typewriter text
//! box with the speaker's name, and numbered choices.
//!
//! A conversation is a [`DialogueTree`] (plain JSON): speakers, and nodes with a line of text
//! and either `choices` or a `next` node. Choices can carry an `action` (sent as a
//! [`UiEvent::Action`] when picked, so `"ui.show:shop"` opens the shop) and a `requires`
//! binding that hides them unless it is true.
//!
//! ```json
//! {"start": "hello",
//!  "speakers": {"mara": {"name": "Mara Voss", "portrait": "portrait:pilot", "side": "left"},
//!               "brannoc": {"name": "Old Brannoc", "portrait": "portrait:merchant", "side": "right"}},
//!  "nodes": {"hello": {"speaker": "brannoc", "text": "Back again?",
//!                      "choices": [{"text": "Show me your wares.", "action": "ui.show:shop"},
//!                                  {"text": "Just passing through."}]}}}
//! ```
//!
//! Open one with `ui.show {"screen": "dialogue", "tree": "merchant_intro"}` (a tree in the
//! [`DialogueLibrary`]) or pass the tree itself in `tree`. Enter / Space / A or a click
//! finishes the line or continues; 1-9 pick a choice.
//!
//! A choice with a `chance` (0..1) is a skill check: its tag shows the odds, and a failed
//! roll goes to `fail` instead of `next` (and sends `dialogue.check:<tag>:fail`, or `:pass`).
//! Choices already picked in this conversation are dimmed with a tick. H (or the Log hint)
//! shows what has been said so far.

use std::collections::BTreeMap;

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::Time;
use engine_input::{ButtonInput, KeyCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::el::{self, El};
use crate::icons::IconLibrary;
use crate::input::{self, UiEvent};
use crate::node::*;
use crate::screens;
use crate::style::{Align, Fill, Pick, Position, Shadow, pct};
use crate::theme::Theme;
use crate::widgets;

/// Which side of the screen a speaker stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Left,
    Right,
}

/// Someone in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Speaker {
    pub name: String,
    /// A line under the name ("Trader of Hollow Reach").
    pub title: String,
    /// Portrait icon. `<portrait>_talk` and `<portrait>_blink` are used for animation when
    /// they exist.
    pub portrait: String,
    pub color: Color,
    pub side: Side,
    /// The portrait art faces right (it is mirrored on the right side so speakers face
    /// each other).
    pub faces_right: bool,
}

impl Default for Speaker {
    fn default() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            portrait: String::new(),
            color: Color::hex(0xe8b65a),
            side: Side::Left,
            faces_right: true,
        }
    }
}

/// A choice the player can pick.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub text: String,
    /// Node to go to (none = end the conversation).
    pub next: Option<String>,
    /// Action sent when picked (e.g. `"ui.show:shop"`, `"quest.accept:relay"`).
    pub action: Option<String>,
    /// Only shown when this binding is truthy (`"quest.has_key"`, `"$Res.field"`).
    pub requires: Option<String>,
    /// A tag shown before the text: `"Persuade"`, `"Lie"`, `"Trade"`.
    pub tag: Option<String>,
    /// Odds of a skill check (0..1), shown with the tag; a failed roll goes to `fail`.
    pub chance: Option<f32>,
    /// Node after a failed check (none = end the conversation).
    pub fail: Option<String>,
}

/// One entry of the conversation log: a line someone said, or a choice the player made.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LogLine {
    /// Speaker key in the tree.
    pub speaker: String,
    /// Markup and `{bindings}` allowed.
    pub text: String,
    /// The player picked this (shown as a reply).
    pub choice: bool,
    /// A skill check's tag and whether it passed.
    pub check: Option<(String, bool)>,
}

/// One line of the conversation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogueNode {
    pub speaker: String,
    /// Markup and `{bindings}` allowed.
    pub text: String,
    pub choices: Vec<Choice>,
    /// Next node when there are no choices (none = end).
    pub next: Option<String>,
    /// Action sent when this line is shown.
    pub action: Option<String>,
}

/// A whole conversation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogueTree {
    pub start: String,
    pub speakers: BTreeMap<String, Speaker>,
    pub nodes: BTreeMap<String, DialogueNode>,
}

impl DialogueTree {
    /// Problems that would break the conversation (unknown speakers or nodes).
    pub fn validate(&self) -> Result<(), String> {
        if !self.nodes.contains_key(&self.start) {
            return Err(format!("The start node \"{}\" doesn't exist.", self.start));
        }
        for (id, n) in &self.nodes {
            if !self.speakers.contains_key(&n.speaker) {
                return Err(format!(
                    "Node \"{id}\" has an unknown speaker \"{}\".",
                    n.speaker
                ));
            }
            let nexts = n
                .next
                .iter()
                .chain(n.choices.iter().filter_map(|c| c.next.as_ref()));
            for next in nexts {
                if !self.nodes.contains_key(next) {
                    return Err(format!("Node \"{id}\" leads to a missing node \"{next}\"."));
                }
            }
        }
        Ok(())
    }
}

/// Conversations by id.
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogueLibrary {
    pub trees: BTreeMap<String, DialogueTree>,
}

/// State of an open conversation (on the screen root).
#[derive(Component, Debug, Clone)]
pub struct DialogueScreen {
    pub id: String,
    pub tree: DialogueTree,
    pub node: String,
    /// Characters revealed.
    pub reveal: f32,
    pub total: usize,
    /// Characters per second.
    pub speed: f32,
    pub pause: f32,
    pub talk: f32,
    pub talk_open: bool,
    pub blink: [f32; 2],
    pub choices_shown: bool,
    pub plain: Vec<char>,
    pub t: f32,
    /// Choices picked so far, as (node, index).
    pub seen: std::collections::BTreeSet<(String, usize)>,
    /// What has been said, oldest first.
    pub log: Vec<LogLine>,
    pub log_open: bool,
    /// Skill-check rolls so far (varies the next roll).
    pub rolls: u32,
}

impl DialogueScreen {
    fn typing(&self) -> bool {
        (self.reveal as usize) < self.total
    }
}

fn portrait_variant(icons: &IconLibrary, base: &str, suffix: &str) -> String {
    let name = format!("{base}_{suffix}");
    if icons.contains(&name) {
        name
    } else {
        base.to_string()
    }
}

fn visible_choices(world: &World, node: &DialogueNode) -> Vec<(usize, Choice)> {
    node.choices
        .iter()
        .enumerate()
        .filter(|(_, c)| match &c.requires {
            Some(path) => {
                crate::data::lookup_f64(world, path).is_some_and(|v| v != 0.0)
                    || world
                        .get_resource::<crate::data::UiData>()
                        .and_then(|d| d.get(path).cloned())
                        .is_some_and(|v| {
                            v.as_bool() == Some(true) || v.as_str().is_some_and(|s| !s.is_empty())
                        })
            }
            None => true,
        })
        .map(|(i, c)| (i, c.clone()))
        .collect()
}

fn portrait_el(sp: &Speaker, side: Side, active: bool, id: &str) -> El {
    let flip = (side == Side::Right) == sp.faces_right;
    let glow = sp.color;
    let mut col = El::new().id(format!("{id}_wrap")).with_style(|s| {
        let s = s
            .position(Position::Absolute)
            .bottom(0.0)
            .width(440.0)
            .height(550.0)
            .column()
            .align_items(Align::Center)
            .justify(crate::style::Justify::End)
            .pick(Pick::Ignore)
            .transition(0.25)
            // The listener steps back into shade rather than turning see-through.
            .opacity(if active { 1.0 } else { 0.88 })
            .scale(if active { 1.0 } else { 0.95 })
            .translate([0.0, if active { 0.0 } else { 12.0 }]);
        match side {
            Side::Left => s.left(30.0),
            Side::Right => s.right(30.0),
        }
    });
    // Soft colored light behind the speaker.
    col = col.child(El::new().with_style(|s| {
        s.position(Position::Absolute)
            .left(-60.0)
            .right(-60.0)
            .top(40.0)
            .bottom(-40.0)
            .background(Fill::Radial {
                inner: glow.fade(0.28),
                outer: glow.fade(0.0),
            })
            .pick(Pick::Ignore)
    }));
    col.child(
        El::icon(sp.portrait.clone())
            .id(id)
            .icon_flip(flip)
            .icon_tint(if active {
                Color::WHITE
            } else {
                Color::hex(0x8a92a4)
            })
            .with_style(|s| s.width(440.0).height(550.0).pick(Pick::Ignore)),
    )
}

/// Green for good odds, amber for even, red for long shots.
pub fn odds_color(chance: f32, p: &crate::theme::Palette) -> Color {
    if chance >= 0.67 {
        p.good
    } else if chance >= 0.34 {
        p.warn
    } else {
        p.bad
    }
}

/// Who answers when the player picks a choice: the speaker on the other side.
fn player_key(tree: &DialogueTree, node: &DialogueNode) -> String {
    let side = tree
        .speakers
        .get(&node.speaker)
        .map_or(Side::Left, |s| s.side);
    tree.speakers
        .iter()
        .find(|(_, s)| s.side != side)
        .map_or_else(|| node.speaker.clone(), |(k, _)| k.clone())
}

/// The last lines of the conversation, newest at the bottom, over the text box.
fn log_panel(st: &DialogueScreen, p: &crate::theme::Palette) -> El {
    const SHOWN: usize = 9;
    let start = st.log.len().saturating_sub(SHOWN);
    let mut list = El::new()
        .class("col")
        .id("dlg_log_list")
        .with_style(|s| s.gap(10.0));
    if st.log.is_empty() {
        list = list.child(El::text("Nothing said yet.").class("caption"));
    }
    for (n, line) in st.log[start..].iter().enumerate() {
        let sp = st
            .tree
            .speakers
            .get(&line.speaker)
            .cloned()
            .unwrap_or_default();
        // Older lines fade back.
        let age = (st.log.len() - start - 1 - n) as f32;
        let fade = (1.0 - age * 0.07).max(0.45);
        let mut head = El::new()
            .class("row")
            .with_style(|s| s.gap(8.0).align_items(Align::Center))
            .child(
                El::text(sp.name.clone())
                    .text_style(|t| t.bold().nowrap().color(sp.color.lerp(Color::WHITE, 0.2)))
                    .with_style(|s| s.font_size(14.0)),
            );
        if line.choice {
            head = head.child(El::text("you").class("caption").text_style(|t| t.nowrap()));
        }
        if let Some((tag, pass)) = &line.check {
            let c = if *pass { p.good } else { p.bad };
            head = head.child(
                El::text(format!(
                    "{}  ·  {}",
                    tag.to_uppercase(),
                    if *pass { "SUCCESS" } else { "FAILED" }
                ))
                .class("chip")
                .text_style(|t| t.bold().nowrap().color(c))
                .with_style(|s| s.font_size(11.0).border_color(c.fade(0.5))),
            );
        }
        list = list.child(
            El::new()
                .class("col")
                .with_style(|s| {
                    s.gap(3.0).opacity(fade).padding([
                        0.0,
                        0.0,
                        0.0,
                        if line.choice { 18.0 } else { 0.0 },
                    ])
                })
                .child(head)
                .child(
                    El::text(line.text.clone())
                        .text_style(|t| t.line_height(1.4))
                        .with_style(|s| {
                            s.font_size(16.0)
                                .color(if line.choice { p.text_dim } else { p.text })
                        }),
                ),
        );
    }
    El::new()
        .class("dialogue_box")
        .id("dlg_log")
        .with_style(|s| {
            s.position(Position::Absolute)
                .bottom(0.0)
                .left(0.0)
                .right(0.0)
                .gap(12.0)
                .padding([18.0, 26.0, 22.0, 26.0])
                .shadow(Shadow::new(Color::hexa(0x000000b0), 40.0, 12.0))
                .pick(Pick::Block)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(Align::Center))
                .child(
                    El::text("CONVERSATION LOG")
                        .text_style(|t| t.bold().nowrap().color(p.accent))
                        .with_style(|s| s.font_size(12.0)),
                )
                .child(El::new().class("spacer"))
                .child(widgets::key_hint("H", "Y", "Close").pick_ignore()),
        )
        .child(list)
}

fn build(world: &World, st: &DialogueScreen) -> El {
    let theme = world.resource::<Theme>().clone();
    let p = &theme.palette;
    let node = st.tree.nodes.get(&st.node).cloned().unwrap_or_default();
    let speaker = st
        .tree
        .speakers
        .get(&node.speaker)
        .cloned()
        .unwrap_or_default();
    let side = speaker.side;

    // Who stands where: the current speaker on their side, the most recent other speaker
    // opposite.
    let mut left: Option<(String, Speaker)> = None;
    let mut right: Option<(String, Speaker)> = None;
    for (k, s) in &st.tree.speakers {
        match s.side {
            Side::Left if left.is_none() || *k == node.speaker => {
                left = Some((k.clone(), s.clone()))
            }
            Side::Right if right.is_none() || *k == node.speaker => {
                right = Some((k.clone(), s.clone()))
            }
            _ => {}
        }
    }

    let mut root = El::new().with_style(|s| s.fill_parent().pick(Pick::Ignore));
    // Letterbox: a black bar at the top fading into the scene.
    root = root.child(El::new().with_style(|s| {
        s.position(Position::Absolute)
            .left(0.0)
            .right(0.0)
            .top(0.0)
            .height(46.0)
            .bg(Color::BLACK)
            .pick(Pick::Ignore)
    }));
    root = root.child(El::new().with_style(|s| {
        s.position(Position::Absolute)
            .left(0.0)
            .right(0.0)
            .top(46.0)
            .height(90.0)
            .background(Fill::Linear {
                from: Color::hexa(0x000000d8),
                to: Color::hexa(0x00000000),
                angle_deg: 90.0,
            })
            .pick(Pick::Ignore)
    }));
    if let Some((k, s)) = &left {
        root = root.child(portrait_el(
            s,
            Side::Left,
            *k == node.speaker,
            "dlg_portrait_left",
        ));
    }
    if let Some((k, s)) = &right {
        root = root.child(portrait_el(
            s,
            Side::Right,
            *k == node.speaker,
            "dlg_portrait_right",
        ));
    }
    // Cinematic shading over the lower screen (and the portraits' cut-off shoulders).
    root = root.child(El::new().with_style(|s| {
        s.position(Position::Absolute)
            .left(0.0)
            .right(0.0)
            .bottom(0.0)
            .height(560.0)
            .background(Fill::Linear {
                from: Color::hexa(0x04060a00),
                to: Color::hexa(0x04060af6),
                angle_deg: 90.0,
            })
            .pick(Pick::Block)
    }));

    // The text box.
    let accent = speaker.color;
    let name_row = El::new()
        .class("row")
        .with_style(|s| s.gap(12.0).align_items(Align::End))
        .child(
            El::text(speaker.name.clone())
                .text_style(|t| t.bold().color(accent.lerp(Color::WHITE, 0.2)).nowrap())
                .with_style(|s| s.font_size(24.0)),
        )
        .child_opt((!speaker.title.is_empty()).then(|| {
            El::text(speaker.title.clone())
                .class("caption")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.margin([0.0, 0.0, 3.0, 0.0]))
        }))
        .child(El::new().class("spacer"))
        .child_opt((!st.log_open).then(|| {
            El::new()
                .class("row")
                .id("dlg_log_hint")
                .on_click("dialogue.log")
                .focusable(false)
                .with_style(|s| s.gap(8.0).margin([0.0, 0.0, 3.0, 0.0]))
                .child(widgets::key_hint("H", "Y", "Log").pick_ignore())
        }));
    let text = El::text(node.text.clone())
        .id("dlg_text")
        .text_style(|t| t.line_height(1.5))
        .with_style(|s| {
            s.font_size(23.0)
                .color(p.text)
                .min_height(if node.choices.is_empty() { 70.0 } else { 0.0 })
        })
        .pick_ignore();
    let mut body = El::new()
        .class("col")
        .with_style(|s| s.gap(14.0))
        .child(name_row)
        .child(text);
    let choices = visible_choices(world, &node);
    if st.choices_shown && !choices.is_empty() {
        let mut list = El::new()
            .class("col")
            .id("dlg_choices")
            .with_style(|s| s.gap(6.0).margin([6.0, 0.0, 0.0, 0.0]));
        for (n, (i, c)) in choices.iter().enumerate() {
            let seen = st.seen.contains(&(st.node.clone(), *i));
            let mut row = El::new()
                .class("list_item choice")
                .id(format!("choice:{n}"))
                .on_click(format!("dialogue.choose:{i}"))
                .with_style(|s| {
                    s.opacity(0.0)
                        .translate([0.0, 8.0])
                        .transition(0.18 + n as f32 * 0.06)
                })
                .child(
                    El::text((n + 1).to_string())
                        .class("kbd")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                );
            match (&c.tag, c.chance) {
                (Some(tag), Some(chance)) => {
                    let col = odds_color(chance, p);
                    row = row.child(
                        El::text(format!("{}  {:.0}%", tag.to_uppercase(), chance * 100.0))
                            .class("chip")
                            .id(format!("choice_odds:{n}"))
                            .text_style(|t| t.bold().nowrap().color(col))
                            .with_style(|s| {
                                s.font_size(12.0)
                                    .border_color(col.fade(0.55))
                                    .background(col.fade(0.12))
                            })
                            .pick_ignore(),
                    );
                }
                (Some(tag), None) => {
                    row = row.child(
                        El::text(format!("[{tag}]"))
                            .text_style(|t| t.semibold().nowrap().color(p.info))
                            .with_style(|s| s.font_size(18.0))
                            .pick_ignore(),
                    );
                }
                _ => {}
            }
            let ends = c.next.is_none() && (c.chance.is_none() || c.fail.is_none());
            row = row.child(
                El::text(c.text.clone())
                    .text_style(|t| if seen { t.color(p.text_faint) } else { t })
                    .with_style(|s| s.font_size(19.0).grow(1.0).shrink(1.0))
                    .pick_ignore(),
            );
            if seen {
                row = row.child(
                    El::icon("ui_check")
                        .icon_tint(p.text_faint)
                        .with_style(|s| s.size(14.0, 14.0))
                        .pick_ignore(),
                );
            }
            if ends {
                row = row.child(El::text("End").class("caption").pick_ignore());
            }
            list = list.child(row);
        }
        body = body.child(list);
    }
    let cont = El::new()
        .class("row")
        .id("dlg_continue")
        .on_click("dialogue.next")
        .focusable(choices.is_empty() || !st.choices_shown)
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(26.0)
                .bottom(18.0)
                .gap(8.0)
                .opacity(if choices.is_empty() || !st.choices_shown {
                    1.0
                } else {
                    0.0
                })
                .transition(0.2)
        })
        .child(
            widgets::key_hint(
                "Space",
                "A",
                if node.next.is_some() || !choices.is_empty() {
                    "Continue"
                } else {
                    "End"
                },
            )
            .pick_ignore(),
        )
        .child(
            El::icon("ui_arrow_down")
                .id("dlg_arrow")
                .with_style(|s| s.size(14.0, 14.0).color(accent))
                .pick_ignore(),
        );
    let x_offset = match side {
        Side::Left => 24.0,
        Side::Right => -24.0,
    };
    let panel = El::new()
        .class("dialogue_box")
        .id("dlg_box")
        .on_click("dialogue.next")
        .focusable(false)
        .with_style(|s| {
            s.width(1040.0)
                .max_width(pct(70.0))
                .border_color(accent.fade(0.35))
                .shadow(Shadow::new(Color::hexa(0x000000a0), 40.0, 12.0))
                .translate([x_offset, 0.0])
                .transition(0.3)
        })
        .child(El::new().with_style(|s| {
            s.position(Position::Absolute)
                .top(-1.0)
                .left(40.0)
                .right(40.0)
                .height(2.0)
                .background(Fill::Linear {
                    from: accent.fade(0.0),
                    to: accent,
                    angle_deg: 0.0,
                })
                .pick(Pick::Ignore)
        }))
        .child(body)
        .child(cont);
    let panel = if st.log_open {
        El::new()
            .with_style(|s| s.width(1040.0).max_width(pct(70.0)).column())
            .child(
                El::new()
                    .with_style(|s| s.height(0.0))
                    .child(log_panel(st, p).with_style(|s| s.bottom(12.0))),
            )
            .child(panel.with_style(|s| s.max_width(pct(100.0))))
    } else {
        panel
    };
    root.child(
        El::new()
            .with_style(|s| {
                s.position(Position::Absolute)
                    .left(0.0)
                    .right(0.0)
                    .bottom(56.0)
                    .row()
                    .justify(crate::style::Justify::Center)
                    .pick(Pick::Ignore)
            })
            .child(panel),
    )
}

/// Characters a line reveals (markup stripped, bindings resolved).
fn plain_text(world: &World, text: &str) -> Vec<char> {
    let resolved = match world.get_resource::<crate::data::UiData>() {
        Some(d) if crate::data::has_bindings(text) => crate::data::resolve(text, d, Some(world)),
        _ => text.to_string(),
    };
    crate::text::strip_markup(&resolved)
        .chars()
        .filter(|c| *c != '\n')
        .collect()
}

/// Builder for `ui.show {"screen": "dialogue"}`.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let (id, tree) = match args.get("tree") {
        Some(Value::String(id)) => {
            let lib = world.resource::<DialogueLibrary>();
            let tree = lib.trees.get(id).cloned().ok_or_else(|| {
                let ids: Vec<&String> = lib.trees.keys().collect();
                format!("There is no conversation \"{id}\". Known: {ids:?}. Pass a whole tree in \"tree\" to play a new one.")
            })?;
            (id.clone(), tree)
        }
        Some(v @ Value::Object(_)) => {
            let tree: DialogueTree = serde_json::from_value(v.clone())
                .map_err(|e| format!("The conversation doesn't parse: {e}."))?;
            ("inline".to_string(), tree)
        }
        _ => {
            return Err("Pass \"tree\": a conversation id (dialogue.list) or a whole tree.".into());
        }
    };
    tree.validate()?;
    let node = args
        .get("node")
        .and_then(Value::as_str)
        .unwrap_or(&tree.start)
        .to_string();
    if !tree.nodes.contains_key(&node) {
        return Err(format!("The conversation has no node \"{node}\"."));
    }
    let mut st = DialogueScreen {
        id,
        tree,
        node,
        reveal: 0.0,
        total: 0,
        speed: args.get("speed").and_then(Value::as_f64).unwrap_or(52.0) as f32,
        pause: 0.35,
        talk: 0.0,
        talk_open: false,
        blink: [2.1, 3.4],
        choices_shown: false,
        plain: Vec::new(),
        t: 0.0,
        seen: Default::default(),
        log: Vec::new(),
        log_open: args.get("log").and_then(Value::as_bool).unwrap_or(false),
        rolls: 0,
    };
    let text = st.tree.nodes[&st.node].text.clone();
    st.plain = plain_text(world, &text);
    st.total = st.plain.len();
    st.log.push(LogLine {
        speaker: st.tree.nodes[&st.node].speaker.clone(),
        text,
        ..Default::default()
    });
    let el = El::new().class("screen").child(build(world, &st));
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 30,
            closable: false,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    world.entity_mut(root).insert(st);
    set_reveal(world, root, 0.0);
    if let Some(a) = world
        .get::<DialogueScreen>(root)
        .and_then(|s| s.tree.nodes[&s.node].action.clone())
    {
        input::dispatch(world, &a);
    }
    Ok(root)
}

fn rebuild(world: &mut World, root: Entity) {
    let Some(st) = world.get::<DialogueScreen>(root).cloned() else {
        return;
    };
    let el = build(world, &st);
    let _ = el::replace_children(world, root, &[el]);
    set_reveal(world, root, st.reveal);
}

fn set_reveal(world: &mut World, root: Entity, reveal: f32) {
    if let Some(t) = screens::find_in(world, root, "dlg_text")
        && let Some(mut text) = world.get_mut::<Text>(t)
    {
        let r = Some(reveal);
        if text.reveal != r {
            text.reveal = r;
        }
    }
}

/// Go to `next` (or end the conversation).
fn go(world: &mut World, root: Entity, next: Option<String>) {
    let Some(mut st) = world.get::<DialogueScreen>(root).cloned() else {
        return;
    };
    match next.filter(|n| st.tree.nodes.contains_key(n)) {
        Some(n) => {
            st.node = n;
            st.reveal = 0.0;
            st.pause = 0.15;
            st.choices_shown = false;
            let text = st.tree.nodes[&st.node].text.clone();
            st.plain = plain_text(world, &text);
            st.total = st.plain.len();
            st.log.push(LogLine {
                speaker: st.tree.nodes[&st.node].speaker.clone(),
                text,
                ..Default::default()
            });
            let action = st.tree.nodes[&st.node].action.clone();
            world.entity_mut(root).insert(st);
            rebuild(world, root);
            if let Some(a) = action {
                input::dispatch(world, &a);
            }
        }
        None => {
            let id = st.id.clone();
            screens::close_screen(world, root, false);
            input::dispatch(world, &format!("dialogue.ended:{id}"));
        }
    }
}

/// Pick choice `index` (of the node's full choice list).
pub fn choose(world: &mut World, root: Entity, index: usize) -> Result<(), String> {
    let st = world
        .get::<DialogueScreen>(root)
        .cloned()
        .ok_or("No conversation.")?;
    let node = st.tree.nodes.get(&st.node).cloned().unwrap_or_default();
    let visible = visible_choices(world, &node);
    let (_, c) = visible
        .into_iter()
        .find(|(i, _)| *i == index)
        .ok_or_else(|| format!("This line has no choice {index}."))?;
    let mut st = st;
    st.seen.insert((st.node.clone(), index));
    // A skill check rolls against its odds; a failure takes the `fail` branch.
    let passed = c.chance.map(|chance| {
        st.rolls += 1;
        roll(st.t, st.rolls) < chance
    });
    let tag = c.tag.clone().unwrap_or_else(|| "Check".into());
    st.log.push(LogLine {
        speaker: player_key(&st.tree, &node),
        text: c.text.clone(),
        choice: true,
        check: passed.map(|p| (tag.clone(), p)),
    });
    world.entity_mut(root).insert(st);
    if let Some(p) = passed {
        input::dispatch(
            world,
            &format!(
                "dialogue.check:{}:{}",
                tag.to_lowercase(),
                if p { "pass" } else { "fail" }
            ),
        );
    }
    if passed != Some(false)
        && let Some(a) = &c.action
    {
        input::dispatch(world, a);
    }
    let next = if passed == Some(false) {
        c.fail.clone()
    } else {
        c.next.clone()
    };
    go(world, root, next);
    Ok(())
}

/// A roll in 0..1 from the conversation clock and the roll count.
fn roll(t: f32, n: u32) -> f32 {
    let mut h = t.to_bits() ^ n.wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// Finish the line, or continue to the next one.
pub fn advance(world: &mut World, root: Entity) {
    let Some(mut st) = world.get::<DialogueScreen>(root).cloned() else {
        return;
    };
    if st.typing() {
        st.reveal = st.total as f32;
        world.entity_mut(root).insert(st);
        return;
    }
    let node = st.tree.nodes.get(&st.node).cloned().unwrap_or_default();
    if visible_choices(world, &node).is_empty() {
        go(world, root, node.next.clone());
    }
}

/// Show or hide the conversation log.
pub fn toggle_log(world: &mut World, root: Entity) {
    if let Some(mut st) = world.get_mut::<DialogueScreen>(root) {
        st.log_open = !st.log_open;
    }
    rebuild(world, root);
    if world
        .get::<DialogueScreen>(root)
        .is_some_and(|s| s.choices_shown)
    {
        reveal_choices(world, root);
    }
}

/// Let the choice rows slide in (they are built hidden).
fn reveal_choices(world: &mut World, root: Entity) {
    if let Some(list) = screens::find_in(world, root, "dlg_choices") {
        for c in screens::subtree(world, list).into_iter().skip(1) {
            if world.get::<OnClick>(c).is_some()
                && let Some(mut n) = world.get_mut::<Node>(c)
            {
                n.style.opacity = None;
                n.style.translate = None;
            }
        }
    }
}

/// Typewriter, portrait animation and input for open conversations.
pub fn dialogue_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let digits: Vec<usize> = world
        .get_resource::<ButtonInput<KeyCode>>()
        .map(|k| {
            use KeyCode::*;
            [
                Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
            ]
            .iter()
            .enumerate()
            .filter(|(_, d)| k.just_pressed(**d))
            .map(|(i, _)| i)
            .collect()
        })
        .unwrap_or_default();
    let log_key = world
        .get_resource::<ButtonInput<KeyCode>>()
        .is_some_and(|k| k.just_pressed(KeyCode::H));
    let mut q = world.query::<(Entity, &DialogueScreen, &Screen)>();
    let open: Vec<Entity> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, _, _)| e)
        .collect();
    for root in open {
        for ev in &events {
            if let UiEvent::Action { action, entity, .. } = ev {
                if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                    continue;
                }
                if action == "dialogue.log" {
                    toggle_log(world, root);
                } else if action == "dialogue.next" {
                    advance(world, root);
                } else if let Some(i) = action
                    .strip_prefix("dialogue.choose:")
                    .and_then(|i| i.parse().ok())
                    && let Err(e) = choose(world, root, i)
                {
                    tracing::warn!("{e}");
                }
            }
        }
        if world.get::<Screen>(root).is_none_or(|s| s.closing) {
            continue;
        }
        if log_key {
            toggle_log(world, root);
        }
        let Some(mut st) = world.get::<DialogueScreen>(root).cloned() else {
            continue;
        };
        // Number keys pick the n-th visible choice.
        if st.choices_shown
            && let Some(&n) = digits.first()
        {
            let node = st.tree.nodes.get(&st.node).cloned().unwrap_or_default();
            if let Some((i, _)) = visible_choices(world, &node).get(n) {
                let _ = choose(world, root, *i);
                continue;
            }
        }
        st.t += dt;
        // Typewriter with short pauses after punctuation.
        if st.typing() {
            if st.pause > 0.0 {
                st.pause -= dt;
            } else {
                let before = st.reveal as usize;
                st.reveal = (st.reveal + st.speed * dt).min(st.total as f32);
                let after = st.reveal as usize;
                if after > before
                    && let Some(c) = st.plain.get(after.saturating_sub(1))
                {
                    st.pause = match c {
                        '.' | '!' | '?' => 0.28,
                        ',' | ';' | ':' => 0.12,
                        '—' => 0.18,
                        _ => 0.0,
                    };
                }
            }
        }
        // Mouth flaps while letters appear; both speakers blink now and then.
        let speaking = st.typing() && st.pause <= 0.0;
        st.talk -= dt;
        if st.talk <= 0.0 {
            st.talk_open = speaking && !st.talk_open;
            st.talk = 0.07 + ((st.t * 13.7).sin() * 0.5 + 0.5) * 0.08;
        }
        for b in &mut st.blink {
            *b -= dt;
            if *b < -0.13 {
                *b = 2.4 + ((st.t * 7.3).sin() * 0.5 + 0.5) * 3.0;
            }
        }
        let show_choices = !st.typing() && !st.choices_shown;
        if show_choices {
            st.choices_shown = true;
        }
        let reveal = st.reveal;
        let node = st.tree.nodes.get(&st.node).cloned().unwrap_or_default();
        let speakers = st.tree.speakers.clone();
        let (talk_open, blink) = (st.talk_open, st.blink);
        world.entity_mut(root).insert(st);
        if show_choices && !node.choices.is_empty() {
            rebuild(world, root);
            reveal_choices(world, root);
        }
        set_reveal(world, root, reveal);
        // Portrait frames.
        let icons = world.resource::<IconLibrary>();
        let mut frames = Vec::new();
        for (side_id, side, b) in [
            ("dlg_portrait_left", Side::Left, blink[0]),
            ("dlg_portrait_right", Side::Right, blink[1]),
        ] {
            let Some((key, sp)) = speakers
                .iter()
                .filter(|(_, s)| s.side == side)
                .find(|(k, _)| **k == node.speaker)
                .or_else(|| speakers.iter().find(|(_, s)| s.side == side))
            else {
                continue;
            };
            let name = if b < 0.0 {
                portrait_variant(icons, &sp.portrait, "blink")
            } else if *key == node.speaker && talk_open {
                portrait_variant(icons, &sp.portrait, "talk")
            } else {
                sp.portrait.clone()
            };
            frames.push((side_id, name));
        }
        for (id, name) in frames {
            if let Some(e) = screens::find_in(world, root, id)
                && let Some(mut icon) = world.get_mut::<Icon>(e)
                && icon.name != name
            {
                icon.name = name;
            }
        }
        // Bob the continue arrow.
        let t = world.resource::<Time>().real_elapsed_secs_f64() as f32;
        if let Some(e) = screens::find_in(world, root, "dlg_arrow")
            && let Some(mut n) = world.get_mut::<Node>(e)
        {
            n.style.translate = Some([0.0, (t * 4.0).sin() * 3.0]);
        }
    }
}

fn open_dialogue(world: &mut World) -> Result<Entity, String> {
    let mut q = world.query::<(Entity, &DialogueScreen, &Screen)>();
    q.iter(world)
        .find(|(_, _, s)| !s.closing)
        .map(|(e, _, _)| e)
        .ok_or_else(|| "No conversation is open. Start one with ui.show {\"screen\": \"dialogue\", \"tree\": ...}.".into())
}

/// Register the `dialogue.*` API.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "dialogue.list",
        "Conversation ids in the DialogueLibrary.",
        "{}",
        |world, _| {
            Ok(json!(
                world
                    .resource::<DialogueLibrary>()
                    .trees
                    .keys()
                    .collect::<Vec<_>>()
            ))
        },
    );
    app.register_api(
        "dialogue.add",
        "Add (or replace) a conversation: {\"id\": \"guard\", \"tree\": {start, speakers, nodes}}.",
        r#"{"id": "guard", "tree": {"start": "a", "speakers": {"g": {"name": "Guard", "portrait": "portrait:pilot", "side": "right"}}, "nodes": {"a": {"speaker": "g", "text": "Halt!"}}}}"#,
        |world, args| {
            let id = args.get("id").and_then(Value::as_str).ok_or("Pass \"id\".")?.to_string();
            let tree: DialogueTree = serde_json::from_value(args.get("tree").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("The conversation doesn't parse: {e}."))?;
            tree.validate()?;
            world.resource_mut::<DialogueLibrary>().trees.insert(id, tree);
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "dialogue.state",
        "The open conversation: speaker, line, whether it is still typing, and the choices.",
        "{}",
        |world, _| {
            let root = open_dialogue(world)?;
            let st = world.get::<DialogueScreen>(root).unwrap().clone();
            let node = st.tree.nodes.get(&st.node).cloned().unwrap_or_default();
            let speaker = st.tree.speakers.get(&node.speaker).map(|s| s.name.clone());
            let choices: Vec<Value> = visible_choices(world, &node)
                .into_iter()
                .map(|(i, c)| json!({"index": i, "text": c.text, "ends": c.next.is_none(), "chance": c.chance, "seen": st.seen.contains(&(st.node.clone(), i))}))
                .collect();
            let log: Vec<Value> = st.log.iter().map(|l| json!({"speaker": l.speaker, "text": l.text, "choice": l.choice, "check": l.check})).collect();
            Ok(json!({"tree": st.id, "node": st.node, "speaker": speaker, "text": node.text, "typing": st.typing(), "choices": choices, "log": log, "log_open": st.log_open}))
        },
    );
    app.register_api(
        "dialogue.choose",
        "Pick a choice by index in the open conversation.",
        r#"{"index": 0}"#,
        |world, args| {
            let root = open_dialogue(world)?;
            let i = args
                .get("index")
                .and_then(Value::as_u64)
                .ok_or("Pass \"index\".")? as usize;
            choose(world, root, i)?;
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "dialogue.log",
        "Show or hide the conversation log (what has been said so far).",
        "{}",
        |world, _| {
            let root = open_dialogue(world)?;
            toggle_log(world, root);
            let open = world
                .get::<DialogueScreen>(root)
                .is_some_and(|s| s.log_open);
            Ok(json!({"open": open}))
        },
    );
    app.register_api(
        "dialogue.next",
        "Finish typing the line, or continue to the next one.",
        "{}",
        |world, _| {
            let root = open_dialogue(world)?;
            advance(world, root);
            Ok(json!({"ok": true}))
        },
    );
}

/// Add the demo conversations (`merchant_intro`, `quill_intro`).
pub fn add_demo_trees(world: &mut World) {
    if !world.contains_resource::<DialogueLibrary>() {
        world.insert_resource(DialogueLibrary::default());
    }
    let mara = Speaker {
        name: "Mara Voss".into(),
        title: "Courier pilot".into(),
        portrait: "portrait:pilot".into(),
        color: Color::hex(0x5fd0ff),
        side: Side::Left,
        faces_right: true,
    };
    let merchant: DialogueTree = serde_json::from_value(json!({
        "start": "hello",
        "speakers": {
            "mara": mara,
            "brannoc": {"name": "Old Brannoc", "title": "Trader of Hollow Reach", "portrait": "portrait:merchant",
                         "color": "#e8b65a", "side": "right", "faces_right": false}
        },
        "nodes": {
            "hello": {"speaker": "brannoc", "text": "Well now. A courier, out here? Hollow Reach doesn't get many visitors since the relay went [i]quiet[/i].", "next": "reply"},
            "reply": {"speaker": "mara", "text": "I've got [color=#5fd0ff]{quest.title}[/color] on my board and a ship that needs fuel. Which one of those can you help with?", "next": "offer"},
            "offer": {"speaker": "brannoc", "text": "Both, if your purse is as heavy as your boots. Fuel cells, rations, a rail rifle that only [b]mostly[/b] explodes. What'll it be?",
                      "choices": [
                          {"text": "Show me what you've got.", "tag": "Trade", "action": "ui.show:shop", "next": "browse"},
                          {"text": "Tell me about the relay tower.", "next": "relay"},
                          {"text": "That rifle. \"Mostly\"? Knock something off and I'll take it.", "tag": "Persuade", "chance": 0.64, "next": "rifle", "fail": "rifle_no"},
                          {"text": "Another time.", "next": null}
                      ]},
            "browse": {"speaker": "brannoc", "text": "Take your time. Don't touch the jar on the left.", "next": null},
            "relay": {"speaker": "brannoc", "text": "Went silent nine days back. Lights still on up on the northern ridge, but nobody answers. The last crew that went up came back... different.", "next": "relay2"},
            "relay2": {"speaker": "mara", "text": "Different how?", "next": "relay3"},
            "relay3": {"speaker": "brannoc", "text": "Quiet. Like the relay. Take a medkit, courier. Take two.",
                       "choices": [
                           {"text": "I'll take those medkits.", "tag": "Trade", "action": "ui.show:shop", "next": null},
                           {"text": "I've heard enough. Goodbye.", "next": null}
                       ]},
            "rifle_no": {"speaker": "brannoc", "text": "Ha! Nice try, courier. The price is the price. The flare's [i]free[/i], though.", "next": "offer"},
            "rifle": {"speaker": "brannoc", "text": "Ninety-six percent of the time it fires a slug at a tenth of light speed. The other four percent, it's a very expensive flare. For you? [color=#e8b65a]Ten percent off.[/color]", "next": "offer"}
        }
    }))
    .expect("demo dialogue");
    let quill: DialogueTree = serde_json::from_value(json!({
        "start": "a",
        "speakers": {
            "mara": mara,
            "quill": {"name": "Quill", "title": "Cartographer of the Drift", "portrait": "portrait:alien",
                       "color": "#9b8cff", "side": "right", "faces_right": false}
        },
        "nodes": {
            "a": {"speaker": "quill", "text": "Your star charts are [i]charming[/i], pilot. Flat. Static. As if the galaxy holds still for you.", "next": "b"},
            "b": {"speaker": "mara", "text": "They get me where I'm going.", "next": "c"},
            "c": {"speaker": "quill", "text": "Let me show you where you [b]could[/b] go.",
                  "choices": [
                      {"text": "Open the star map.", "action": "ui.show:map", "next": null},
                      {"text": "You wanted crystals from the Glass Caves.", "tag": "Quest", "action": "quest.track:glass_and_glow", "next": "crystals"},
                      {"text": "Maybe later.", "next": null}
                  ]},
            "crystals": {"speaker": "quill", "text": "Three, unbroken. They sing to the relay, and the relay sings to [i]something else[/i]. Bring them to me and I will draw you where.",
                         "choices": [
                             {"text": "Open my journal.", "action": "ui.show:journal", "next": null},
                             {"text": "I'm on it.", "next": null}
                         ]}
        }
    }))
    .expect("demo dialogue");
    let mut lib = world.resource_mut::<DialogueLibrary>();
    lib.trees.entry("merchant_intro".into()).or_insert(merchant);
    lib.trees.entry("quill_intro".into()).or_insert(quill);
}
