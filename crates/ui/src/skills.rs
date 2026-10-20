//! The skill tree: three branches of talents (Combat, Tech, Survival) drawn as lit paths,
//! learned with skill points, each with ranks, prerequisites and a level gate.
//!
//! ```ignore
//! ui::show(world, "skills", &json!({"select": "c_riposte"}))?;
//! ```
//!
//! The tree lives in the [`SkillTree`] resource (`skills.tree` replaces it, `skills.learn`
//! spends a point, `skills.grant` gives points, `skills.reset` refunds them). Learning writes
//! `skills.<id>` (the rank) and `player.skill_points` to [`UiData`], so the HUD and game code
//! can read them, and sends `skill.learned:<id>`.

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_input::{ButtonInput, GamepadButton, KeyCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::draw::{DrawCtx, Painters, flag};
use crate::el::{self, El};
use crate::input::{UiEvent, UiInputState};
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Pick, Position, pct};
use crate::theme::{Palette, Theme};
use crate::toast::{self, ToastKind};
use crate::widgets;

/// What a skill is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillKind {
    #[default]
    Passive,
    Active,
    Ultimate,
}

impl SkillKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Passive => "Passive",
            Self::Active => "Active ability",
            Self::Ultimate => "Ultimate",
        }
    }
}

/// One branch of the tree.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// One line under the name.
    pub motto: String,
    pub color: Option<Color>,
}

/// One talent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Index into [`SkillTree::branches`].
    pub branch: usize,
    /// Row from the top, 0..=4.
    pub tier: u32,
    /// Sideways place in its branch: -1 left, 0 middle, 1 right.
    pub col: f32,
    pub kind: SkillKind,
    pub rank: u32,
    pub max_rank: u32,
    /// Points per rank.
    pub cost: u32,
    /// Ids that need at least one rank first.
    pub requires: Vec<String>,
    /// Player level needed.
    pub level: u32,
    pub description: String,
    /// What each rank gives ("+8% melee damage"), one line per rank.
    pub ranks: Vec<String>,
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            icon: String::new(),
            branch: 0,
            tier: 0,
            col: 0.0,
            kind: SkillKind::Passive,
            rank: 0,
            max_rank: 1,
            cost: 1,
            requires: Vec::new(),
            level: 1,
            description: String::new(),
            ranks: Vec::new(),
        }
    }
}

/// Where a skill stands for the player right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillState {
    Maxed,
    Learned,
    /// Can be learned (or ranked up) now.
    Available,
    /// Prerequisites met but not enough points.
    Affordable,
    /// Waiting on a prerequisite or the player's level.
    Locked,
}

/// The whole tree and the points to spend.
#[derive(Resource, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillTree {
    pub branches: Vec<Branch>,
    pub skills: Vec<Skill>,
    pub points: u32,
}

impl SkillTree {
    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Skill> {
        self.skills.iter_mut().find(|s| s.id == id)
    }

    pub fn spent(&self) -> u32 {
        self.skills.iter().map(|s| s.rank * s.cost).sum()
    }

    pub fn spent_in(&self, branch: usize) -> u32 {
        self.skills
            .iter()
            .filter(|s| s.branch == branch)
            .map(|s| s.rank * s.cost)
            .sum()
    }

    /// Why `id` can't take another rank now, or `None` if it can.
    pub fn blocker(&self, id: &str, level: u32) -> Option<String> {
        let Some(s) = self.get(id) else {
            return Some(format!("There is no skill \"{id}\"."));
        };
        if s.rank >= s.max_rank {
            return Some(format!("{} is at its highest rank.", s.name));
        }
        if let Some(r) = s
            .requires
            .iter()
            .find(|r| self.get(r).is_none_or(|r| r.rank == 0))
        {
            let name = self.get(r).map_or(r.as_str(), |r| r.name.as_str());
            return Some(format!("Learn {name} first."));
        }
        if level < s.level {
            return Some(format!("Needs level {}.", s.level));
        }
        if self.points < s.cost {
            return Some(if s.cost == 1 {
                "You need a skill point.".into()
            } else {
                format!("You need {} skill points.", s.cost)
            });
        }
        None
    }

    pub fn state(&self, id: &str, level: u32) -> SkillState {
        let Some(s) = self.get(id) else {
            return SkillState::Locked;
        };
        if s.rank >= s.max_rank {
            return SkillState::Maxed;
        }
        let reqs = s
            .requires
            .iter()
            .all(|r| self.get(r).is_some_and(|r| r.rank > 0));
        let open = reqs && level >= s.level;
        match (open, self.points >= s.cost, s.rank > 0) {
            (true, true, _) => SkillState::Available,
            (_, _, true) => SkillState::Learned,
            (true, false, false) => SkillState::Affordable,
            _ => SkillState::Locked,
        }
    }

    pub fn color(&self, branch: usize, p: &Palette) -> Color {
        self.branches
            .get(branch)
            .and_then(|b| b.color)
            .unwrap_or(p.accent)
    }
}

fn player_level(world: &World) -> u32 {
    world
        .get_resource::<UiData>()
        .and_then(|d| d.get_f64("player.level"))
        .unwrap_or(1.0) as u32
}

fn sync_data(world: &mut World) {
    let Some(tree) = world.get_resource::<SkillTree>().cloned() else {
        return;
    };
    if let Some(mut data) = world.get_resource_mut::<UiData>() {
        if data.get_f64("player.skill_points") != Some(tree.points as f64) {
            data.set("player.skill_points", tree.points);
        }
        for s in &tree.skills {
            let path = format!("skills.{}", s.id);
            if data.get_f64(&path) != Some(s.rank as f64) {
                data.set(&path, s.rank);
            }
        }
    }
}

/// Spend points on one rank of `id`. Returns the new rank.
pub fn learn(world: &mut World, id: &str) -> Result<u32, String> {
    let level = player_level(world);
    let mut tree = world
        .get_resource::<SkillTree>()
        .cloned()
        .ok_or("There is no skill tree.")?;
    if let Some(why) = tree.blocker(id, level) {
        return Err(why);
    }
    let s = tree.get_mut(id).ok_or("no skill")?;
    s.rank += 1;
    let (rank, max, name, icon, cost) =
        (s.rank, s.max_rank, s.name.clone(), s.icon.clone(), s.cost);
    tree.points -= cost;
    world.insert_resource(tree);
    sync_data(world);
    crate::input::dispatch(world, &format!("skill.learned:{id}"));
    let text = if max > 1 {
        format!("[b]{name}[/b] · rank {rank} of {max}")
    } else {
        format!("[b]{name}[/b]")
    };
    toast::notify(world, ToastKind::Good, &icon, "Skill learned", &text, 2.4);
    Ok(rank)
}

/// Refund every point.
pub fn reset(world: &mut World) -> u32 {
    let Some(mut tree) = world.get_resource::<SkillTree>().cloned() else {
        return 0;
    };
    let refund = tree.spent();
    tree.points += refund;
    for s in &mut tree.skills {
        s.rank = 0;
    }
    world.insert_resource(tree);
    sync_data(world);
    if refund > 0 {
        toast::notify(
            world,
            ToastKind::Info,
            "ui_sort",
            "Skills reset",
            &format!("{refund} points back to spend"),
            2.4,
        );
    }
    refund
}

// -------------------------------------------------------------------------------- layout

/// The tree canvas, logical px.
const TREE_W: f32 = 880.0;
const TREE_H: f32 = 640.0;
const NODE: f32 = 68.0;
const ULT: f32 = 84.0;

/// A node's centre on the canvas (logical px).
fn node_pos(s: &Skill, branches: usize) -> Vec2 {
    let n = branches.max(1) as f32;
    let col_w = TREE_W / n;
    let x = col_w * (s.branch as f32 + 0.5) + s.col * col_w * 0.29;
    let y = 112.0 + s.tier as f32 * 112.0;
    Vec2::new(x, y)
}

fn node_size(s: &Skill) -> f32 {
    if s.kind == SkillKind::Ultimate {
        ULT
    } else {
        NODE
    }
}

/// The canvas's state (on the node with `paint: "skill_tree"`).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct SkillCanvas {
    pub tree: SkillTree,
    pub level: u32,
    pub hovered: Option<String>,
    pub selected: Option<String>,
    /// Real seconds, for the pulse on learnable nodes.
    pub time: f32,
}

fn paint_tree(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(v) = world.get::<SkillCanvas>(e).cloned() else {
        return;
    };
    let p = world.resource::<Theme>().palette.clone();
    let px = ctx.px;
    let o = Vec2::new(ctx.rect.x, ctx.rect.y);
    let at = |q: Vec2| o + q * px;
    let nb = v.tree.branches.len();
    let pulse = 0.5 + 0.5 * (v.time * 3.2).sin();

    // Branch light: a soft column of the branch color behind each path.
    for (b, _) in v.tree.branches.iter().enumerate() {
        let col = v.tree.color(b, &p);
        let x = TREE_W / nb.max(1) as f32 * (b as f32 + 0.5);
        let spent = v.tree.spent_in(b) as f32;
        let k = 0.05 + (spent / 12.0).min(1.0) * 0.07;
        for (i, y) in [150.0, 330.0, 510.0].iter().enumerate() {
            ctx.glow(
                at(Vec2::new(x, *y)),
                (170.0 - i as f32 * 12.0) * px,
                col.with_alpha(k),
                1.4,
            );
        }
    }

    // Links: dim when locked, the branch color once both ends are learned, and a brighter
    // dashed line toward what can be learned next.
    for s in &v.tree.skills {
        let to = at(node_pos(s, nb));
        let state = v.tree.state(&s.id, v.level);
        for r in &s.requires {
            let Some(req) = v.tree.get(r) else {
                continue;
            };
            let from = at(node_pos(req, nb));
            let col = v.tree.color(s.branch, &p);
            let d = (to - from).normalize_or_zero();
            let a = from + d * node_size(req) * 0.5 * px;
            let b = to - d * node_size(s) * 0.5 * px;
            let lit = req.rank > 0 && s.rank > 0;
            let next = req.rank > 0 && s.rank == 0 && state == SkillState::Available;
            ctx.line(a, b, 6.0 * px, Color::hexa(0x04070dd0), 0.0);
            if lit {
                ctx.line(a, b, 8.0 * px, col.with_alpha(0.18), 0.0);
                ctx.line(a, b, 3.0 * px, col.lerp(Color::WHITE, 0.25), 0.0);
            } else if next {
                ctx.line(
                    a,
                    b,
                    2.5 * px,
                    col.with_alpha(0.45 + 0.35 * pulse),
                    10.0 * px,
                );
            } else {
                ctx.line(a, b, 2.0 * px, Color::WHITE.with_alpha(0.1), 0.0);
            }
        }
    }

    // Nodes.
    for s in &v.tree.skills {
        let c = at(node_pos(s, nb));
        let size = node_size(s) * px;
        let r = size * 0.5;
        let col = v.tree.color(s.branch, &p);
        let state = v.tree.state(&s.id, v.level);
        let hovered = v.hovered.as_deref() == Some(s.id.as_str());
        let selected = v.selected.as_deref() == Some(s.id.as_str());
        let learned = s.rank > 0;
        let ult = s.kind == SkillKind::Ultimate;
        let rect = Rect::new(c.x - r, c.y - r, size, size);
        // Glow under learned and learnable nodes.
        if learned {
            ctx.glow(c, r * 2.1, col.with_alpha(0.26), 1.6);
        } else if state == SkillState::Available {
            ctx.glow(c, r * 1.9, col.with_alpha(0.12 + 0.12 * pulse), 1.8);
        }
        if selected {
            ctx.ring(
                c,
                Vec2::splat(r + 9.0 * px),
                2.0 * px,
                Color::WHITE.with_alpha(0.85),
                0.0,
                std::f32::consts::TAU,
                0.0,
            );
        }
        // The disc: dark, lit with the branch color once learned.
        let (inner, outer) = if learned {
            (
                col.lerp(Color::hexa(0x0b111cff), 0.55),
                Color::hexa(0x0b111cff),
            )
        } else {
            (Color::hexa(0x1a2232ff), Color::hexa(0x0a0f18ff))
        };
        ctx.rect_fill(
            rect,
            Fill::Radial { inner, outer },
            [r; 4],
            0.0,
            Color::TRANSPARENT,
        );
        // Rim.
        let (rim, rim_w) = match state {
            SkillState::Maxed => (p.accent_light, 3.0),
            SkillState::Learned => (col.lerp(Color::WHITE, 0.2), 2.5),
            SkillState::Available => (col.with_alpha(0.6 + 0.4 * pulse), 2.5),
            SkillState::Affordable => (col.with_alpha(0.45), 2.0),
            SkillState::Locked => (Color::WHITE.with_alpha(0.16), 1.5),
        };
        let rim = if hovered {
            rim.lerp(Color::WHITE, 0.35)
        } else {
            rim
        };
        ctx.ring(
            c,
            Vec2::splat(r - rim_w * 0.5 * px),
            rim_w * px,
            rim,
            0.0,
            std::f32::consts::TAU,
            0.0,
        );
        if ult {
            // The capstone wears a second, finer ring.
            ctx.ring(
                c,
                Vec2::splat(r + 5.0 * px),
                1.2 * px,
                col.with_alpha(if learned { 0.8 } else { 0.3 }),
                0.0,
                std::f32::consts::TAU,
                24.0,
            );
        }
        // Icon, grayed when locked.
        let is = size * if ult { 0.6 } else { 0.58 };
        let n0 = ctx.list.instances.len();
        let dim = match state {
            SkillState::Locked => 0.45,
            SkillState::Affordable => 0.7,
            _ => 1.0,
        };
        ctx.faded(dim, |ctx| {
            ctx.icon(
                &s.icon,
                Rect::new(c.x - is * 0.5, c.y - is * 0.5, is, is),
                Color::WHITE,
            );
        });
        if state == SkillState::Locked {
            for inst in &mut ctx.list.instances[n0..] {
                inst.kind[1] |= flag::DESATURATE;
            }
        }
        // Rank pips under the node.
        if s.max_rank > 1 {
            let n = s.max_rank as f32;
            let w = 9.0 * px;
            let gap = 4.0 * px;
            let total = n * w + (n - 1.0) * gap;
            let y = c.y + r + 8.0 * px;
            for k in 0..s.max_rank {
                let x = c.x - total * 0.5 + k as f32 * (w + gap);
                let on = k < s.rank;
                ctx.rect(
                    Rect::new(x, y, w, 4.0 * px),
                    if on {
                        col.lerp(Color::WHITE, 0.3)
                    } else {
                        Color::WHITE.with_alpha(0.14)
                    },
                    2.0 * px,
                    0.0,
                    Color::TRANSPARENT,
                );
            }
        }
        // A lock over level-gated nodes.
        if state == SkillState::Locked
            && s.requires
                .iter()
                .all(|r| v.tree.get(r).is_some_and(|r| r.rank > 0))
            && v.level < s.level
        {
            let l = 22.0 * px;
            let q = c + Vec2::new(r * 0.62, r * 0.62);
            ctx.rect(
                Rect::new(q.x - l * 0.5, q.y - l * 0.5, l, l),
                Color::hexa(0x0a0f18f0),
                l * 0.5,
                1.0 * px,
                Color::WHITE.with_alpha(0.25),
            );
            ctx.icon(
                "ui_lock",
                Rect::new(q.x - l * 0.32, q.y - l * 0.32, l * 0.64, l * 0.64),
                p.text_dim,
            );
        }
        // The level a gated node waits on, in a pill under it.
        if v.level < s.level && s.rank == 0 {
            let label = format!("LV {}", s.level);
            let y = c.y + r + 14.0 * px;
            let w = (label.len() as f32 * 7.0 + 16.0) * px;
            let h = 18.0 * px;
            ctx.rect(
                Rect::new(c.x - w * 0.5, y - h * 0.5, w, h),
                Color::hexa(0x0a0f18e8),
                h * 0.5,
                1.0 * px,
                col.with_alpha(0.45),
            );
            ctx.text(
                &label,
                11.0,
                700,
                col.lerp(Color::WHITE, 0.35).with_alpha(0.9),
                Vec2::new(c.x, y),
                Vec2::splat(0.5),
            );
        }
    }
}

pub fn add_painters(p: &mut Painters) {
    p.add("skill_tree", paint_tree);
}

// -------------------------------------------------------------------------------- screen

/// The open skills screen (on its root).
#[derive(Component, Debug, Clone, Default)]
pub struct SkillsScreen {
    pub selected: Option<String>,
    pub snapshot: Option<SkillTree>,
}

fn branch_header(tree: &SkillTree, b: usize, p: &Palette) -> El {
    let br = &tree.branches[b];
    let col = tree.color(b, p);
    let w = TREE_W / tree.branches.len().max(1) as f32;
    El::new()
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(w * b as f32)
                .top(8.0)
                .width(w)
                .align_items(Align::Center)
                .gap(2.0)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0))
                .child(
                    El::icon(br.icon.clone())
                        .icon_tint(col)
                        .with_style(|s| s.size(18.0, 18.0))
                        .pick_ignore(),
                )
                .child(
                    El::text(br.name.to_uppercase())
                        .text_style(|t| t.bold().nowrap().shadow())
                        .with_style(|s| s.font_size(15.0).color(col.lerp(Color::WHITE, 0.25))),
                ),
        )
        .child(
            El::text(format!("{}  ·  {} spent", br.motto, tree.spent_in(b)))
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| s.font_size(12.0).font_weight(600).color(p.text_faint)),
        )
}

fn tree_panel(tree: &SkillTree, p: &Palette) -> El {
    let nb = tree.branches.len();
    let mut canvas = El::new()
        .id("skills_canvas")
        .paint("skill_tree")
        .with_style(|s| s.width(TREE_W).height(TREE_H).pick(Pick::Ignore));
    // An invisible button over each node: clicks, focus, gamepad navigation, tooltips.
    for s in &tree.skills {
        let c = node_pos(s, nb);
        let size = node_size(s) + 8.0;
        canvas = canvas.child(
            El::new()
                .id(format!("skill:{}", s.id))
                .on_click(format!("skills.select:{}", s.id))
                .focusable(true)
                .tooltip(s.name.clone(), s.description.clone())
                .with_style(|st| {
                    st.position(Position::Absolute)
                        .left(c.x - size * 0.5)
                        .top(c.y - size * 0.5)
                        .size(size, size)
                        .radius(Corners::all(999.0))
                        .pick(Pick::Block)
                }),
        );
    }
    for b in 0..nb {
        canvas = canvas.child(branch_header(tree, b, p));
    }
    El::new()
        .class("panel")
        .id("skills_tree")
        .with_style(|s| s.padding([8.0, 8.0]).align_items(Align::Center))
        .child(canvas)
}

fn requirement_row(p: &Palette, ok: bool, text: String) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(10.0))
        .child(
            El::icon(if ok { "ui_check" } else { "ui_lock" })
                .icon_tint(if ok { p.good } else { p.text_faint })
                .with_style(|s| s.size(16.0, 16.0))
                .pick_ignore(),
        )
        .child(El::text(text).text_style(|t| t.nowrap()).with_style(|s| {
            s.font_size(14.0)
                .color(if ok { p.text_dim } else { p.text_faint })
        }))
}

fn details(world: &World, st: &SkillsScreen) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let tree = world
        .get_resource::<SkillTree>()
        .cloned()
        .unwrap_or_default();
    let level = player_level(world);
    let panel = El::new()
        .class("panel")
        .id("skills_details")
        .with_style(|s| s.width(380.0).gap(14.0));
    let Some(s) = st.selected.as_deref().and_then(|id| tree.get(id)) else {
        return panel.child(
            El::new()
                .class("col center")
                .with_style(|s| s.grow(1.0))
                .child(El::icon("spellbook").with_style(|s| s.size(72.0, 72.0).opacity(0.5)))
                .child(El::text("Pick a skill to read about it.").class("dim")),
        );
    };
    let col = tree.color(s.branch, p);
    let state = tree.state(&s.id, level);
    let branch = tree
        .branches
        .get(s.branch)
        .map(|b| b.name.clone())
        .unwrap_or_default();

    let head = El::new()
        .class("row")
        .with_style(|s| s.gap(16.0))
        .child(
            El::new()
                .with_style(|st| {
                    st.size(72.0, 72.0)
                        .shrink(0.0)
                        .radius(Corners::all(999.0))
                        .center()
                        .background(Fill::Radial {
                            inner: col.lerp(Color::hexa(0x0b111cff), 0.5),
                            outer: Color::hexa(0x0b111cff),
                        })
                        .border(2.0, col.lerp(Color::WHITE, 0.2))
                })
                .child(
                    El::icon(s.icon.clone())
                        .with_style(|s| s.size(44.0, 44.0))
                        .pick_ignore(),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(4.0).grow(1.0))
                .child(
                    El::text(format!(
                        "{}  ·  {}",
                        branch.to_uppercase(),
                        s.kind.label().to_uppercase()
                    ))
                    .text_style(|t| t.bold().nowrap())
                    .with_style(|st| st.font_size(11.0).color(col.lerp(Color::WHITE, 0.25))),
                )
                .child(
                    El::text(s.name.clone())
                        .class("title")
                        .text_style(|t| t.nowrap()),
                )
                .child(
                    El::text(if s.max_rank > 1 {
                        format!("Rank {} of {}", s.rank, s.max_rank)
                    } else if s.rank > 0 {
                        "Learned".to_string()
                    } else {
                        "Not learned".to_string()
                    })
                    .text_style(|t| t.nowrap().tabular())
                    .with_style(|st| {
                        st.font_size(13.0).font_weight(600).color(if s.rank > 0 {
                            p.accent_light
                        } else {
                            p.text_faint
                        })
                    }),
                ),
        );

    let mut ranks = El::new().class("col").with_style(|s| s.gap(6.0));
    for (i, line) in s.ranks.iter().enumerate() {
        let k = i as u32 + 1;
        let (now, next) = (k <= s.rank, k == s.rank + 1);
        ranks = ranks.child(
            El::new()
                .class("row")
                .with_style(|st| {
                    st.gap(10.0)
                        .padding([7.0, 10.0])
                        .radius(Corners::all(8.0))
                        .background(if next {
                            col.with_alpha(0.12)
                        } else {
                            Color::TRANSPARENT
                        })
                        .border(
                            1.0,
                            if next {
                                col.with_alpha(0.35)
                            } else {
                                Color::TRANSPARENT
                            },
                        )
                })
                .child(
                    El::text(if s.max_rank > 1 {
                        format!("{k}")
                    } else {
                        "•".into()
                    })
                    .text_style(|t| t.bold().nowrap().tabular())
                    .with_style(|st| {
                        st.font_size(12.0).width(14.0).color(if now {
                            p.accent_light
                        } else {
                            p.text_faint
                        })
                    }),
                )
                .child(El::text(line.clone()).with_style(|st| {
                    st.font_size(14.0).grow(1.0).color(if now || next {
                        p.text
                    } else {
                        p.text_faint
                    })
                })),
        );
    }

    let mut reqs = El::new().class("col").with_style(|s| s.gap(6.0));
    for r in &s.requires {
        let (name, ok) = tree
            .get(r)
            .map_or((r.clone(), false), |r| (r.name.clone(), r.rank > 0));
        reqs = reqs.child(requirement_row(p, ok, name));
    }
    if s.level > 1 {
        reqs = reqs.child(requirement_row(
            p,
            level >= s.level,
            format!("Level {}", s.level),
        ));
    }
    let has_reqs = !s.requires.is_empty() || s.level > 1;

    let blocker = tree.blocker(&s.id, level);
    let label = match state {
        SkillState::Maxed => "Highest rank".to_string(),
        _ if s.rank > 0 => format!("Rank up  ·  {}", points_text(s.cost)),
        _ => format!("Learn  ·  {}", points_text(s.cost)),
    };
    let learn = El::button(label, "skills.learn")
        .class(if blocker.is_none() { "primary" } else { "" })
        .id("skills_learn")
        .disabled(blocker.is_some())
        .with_style(|s| s.width(pct(100.0)).height(48.0));

    panel
        .child(head)
        .child(widgets::divider())
        .child(
            El::text(s.description.clone())
                .class("body")
                .with_style(|st| st.color(p.text_dim)),
        )
        .child_opt((!s.ranks.is_empty()).then(|| {
            El::new()
                .class("col")
                .with_style(|s| s.gap(8.0))
                .child(El::text("RANKS").class("label_caps"))
                .child(ranks)
        }))
        .child_opt(has_reqs.then(|| {
            El::new()
                .class("col")
                .with_style(|s| s.gap(8.0))
                .child(El::text("REQUIRES").class("label_caps"))
                .child(reqs)
        }))
        .child(El::new().class("spacer"))
        .child_opt(blocker.filter(|_| state != SkillState::Maxed).map(|why| {
            El::text(why)
                .text_style(|t| t.nowrap())
                .with_style(|st| st.font_size(13.0).color(p.warn))
        }))
        .child(learn)
}

fn points_text(n: u32) -> String {
    if n == 1 {
        "1 point".into()
    } else {
        format!("{n} points")
    }
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
        .child(widgets::key_hint("Space", "X", "Learn"))
        .child(widgets::key_hint("R", "Y", "Reset"))
        .child(widgets::key_hint("Esc", "B", "Close"))
}

fn build(world: &World, st: &SkillsScreen) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let tree = world
        .get_resource::<SkillTree>()
        .cloned()
        .unwrap_or_default();
    let level = player_level(world);
    let points = tree.points;
    let top = El::new()
        .class("row")
        .id("skills_top")
        .with_style(|s| s.gap(16.0).width(1290.0).padding([0.0, 4.0]))
        .child(
            El::new().class("header_icon").child(
                El::icon("spellbook")
                    .with_style(|s| s.size(26.0, 26.0))
                    .pick_ignore(),
            ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0))
                .child(El::text("Skills").class("title").text_style(|t| t.shadow()))
                .child(
                    El::text(format!("Level {level}  ·  {} points spent", tree.spent()))
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| s.font_size(13.0).font_weight(600).color(p.info)),
                ),
        )
        .child(El::new().class("spacer"))
        .child(
            El::new()
                .class("chip row")
                .id("skills_points")
                .with_style(|s| {
                    s.gap(8.0)
                        .padding([8.0, 16.0])
                        .background(if points > 0 {
                            p.accent.with_alpha(0.16)
                        } else {
                            Color::WHITE.with_alpha(0.05)
                        })
                        .border(
                            1.0,
                            if points > 0 {
                                p.accent.with_alpha(0.6)
                            } else {
                                p.line
                            },
                        )
                })
                .child(
                    El::icon("star")
                        .with_style(|s| s.size(18.0, 18.0))
                        .pick_ignore(),
                )
                .child(
                    El::text(format!("{points}"))
                        .text_style(|t| t.bold().tabular().nowrap())
                        .with_style(|s| {
                            s.font_size(18.0).color(if points > 0 {
                                p.accent_light
                            } else {
                                p.text_dim
                            })
                        }),
                )
                .child(
                    El::text(if points == 1 {
                        "skill point"
                    } else {
                        "skill points"
                    })
                    .text_style(|t| t.nowrap())
                    .with_style(|s| s.font_size(13.0).color(p.text_dim)),
                ),
        )
        .child(
            El::button("Reset", "skills.reset")
                .class("ghost")
                .id("skills_reset")
                .disabled(tree.spent() == 0),
        )
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
                .with_style(|s| {
                    s.gap(18.0)
                        .align_items(Align::Stretch)
                        .height(TREE_H + 16.0)
                })
                .child(tree_panel(&tree, p))
                .child(details(world, st)),
        )
        .child(hints())
}

fn fill(world: &mut World, root: Entity, st: &SkillsScreen) -> Result<(), String> {
    let content = build(world, st);
    el::spawn(world, Some(root), &content)?;
    let tree = world
        .get_resource::<SkillTree>()
        .cloned()
        .unwrap_or_default();
    let level = player_level(world);
    if let Some(canvas) = screens::find_in(world, root, "skills_canvas") {
        world.entity_mut(canvas).insert(SkillCanvas {
            tree,
            level,
            hovered: None,
            selected: st.selected.clone(),
            time: 0.0,
        });
    }
    Ok(())
}

pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let tree = world
        .get_resource::<SkillTree>()
        .cloned()
        .unwrap_or_default();
    if tree.skills.is_empty() {
        return Err("The skill tree is empty (fill it with skills.tree).".into());
    }
    let level = player_level(world);
    let selected = args
        .get("select")
        .and_then(Value::as_str)
        .filter(|id| tree.get(id).is_some())
        .map(str::to_string)
        // Open on something worth reading: the first skill you can learn, else the first.
        .or_else(|| {
            tree.skills
                .iter()
                .find(|s| tree.state(&s.id, level) == SkillState::Available)
                .or(tree.skills.first())
                .map(|s| s.id.clone())
        });
    let mut st = SkillsScreen {
        selected,
        snapshot: None,
    };
    let el = El::new()
        .class("screen")
        .child(El::new().with_style(|_| widgets::scrim(0.74)));
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
    st.snapshot = world.get_resource::<SkillTree>().cloned();
    world.entity_mut(root).insert(st);
    Ok(root)
}

fn rebuild(world: &mut World, root: Entity) {
    let Some(st) = world.get::<SkillsScreen>(root).cloned() else {
        return;
    };
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
        tracing::warn!("skills: {e}");
    }
    let snap = world.get_resource::<SkillTree>().cloned();
    if let Some(mut s) = world.get_mut::<SkillsScreen>(root) {
        s.snapshot = snap;
    }
}

/// Skill actions from anywhere, the screen's input, hover and the pulse.
pub fn skills_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let mut reset_now = false;
    for ev in &events {
        if let UiEvent::Action { action, .. } = ev {
            if let Some(id) = action.strip_prefix("skills.learn:") {
                if let Err(e) = learn(world, id) {
                    toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                }
            } else if action == "confirm:skills.reset_now" {
                reset_now = true;
            }
        }
    }
    if reset_now {
        reset(world);
    }
    let mut q = world.query::<(Entity, &SkillsScreen, &Screen)>();
    let open: Vec<(Entity, SkillsScreen)> = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, _)| (e, st.clone()))
        .collect();
    for (root, mut st) in open {
        let before = st.selected.clone();
        let mut learn_selected = false;
        let mut ask_reset = false;
        for ev in &events {
            if let UiEvent::Action { action, entity, .. } = ev {
                if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                    continue;
                }
                if let Some(id) = action.strip_prefix("skills.select:") {
                    st.selected = Some(id.to_string());
                } else if action == "skills.learn" {
                    learn_selected = true;
                } else if action == "skills.reset" {
                    ask_reset = true;
                }
            }
        }
        let on_top = screens::open_screens(world)
            .into_iter()
            .find(|(.., modal)| *modal)
            .is_some_and(|(_, e, ..)| e == root);
        if on_top {
            let keys = world.get_resource::<ButtonInput<KeyCode>>();
            let pad = world.get_resource::<ButtonInput<GamepadButton>>();
            if keys.is_some_and(|k| k.just_pressed(KeyCode::Space))
                || pad.is_some_and(|b| b.just_pressed(GamepadButton::West))
            {
                learn_selected = true;
            }
            if keys.is_some_and(|k| k.just_pressed(KeyCode::R))
                || pad.is_some_and(|b| b.just_pressed(GamepadButton::North))
            {
                ask_reset = true;
            }
        }
        // Focus moving with keys or a pad selects too.
        let focus = world
            .get_resource::<UiInputState>()
            .and_then(|s| s.focused)
            .filter(|e| screens::is_descendant(world, *e, root))
            .and_then(|e| world.get::<UiId>(e))
            .and_then(|id| id.0.strip_prefix("skill:").map(str::to_string));
        if let Some(id) = focus
            && world
                .get_resource::<UiInputState>()
                .is_some_and(|s| s.mode == crate::input::NavMode::Directional)
        {
            st.selected = Some(id);
        }
        if learn_selected
            && let Some(id) = st.selected.clone()
            && let Err(e) = learn(world, &id)
        {
            toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
        }
        if ask_reset
            && world
                .get_resource::<SkillTree>()
                .is_some_and(|t| t.spent() > 0)
        {
            let refund = world
                .get_resource::<SkillTree>()
                .map_or(0, SkillTree::spent);
            let r = screens::show(
                world,
                "confirm",
                &json!({
                    "title": "Reset every skill?",
                    "text": format!("All {refund} points come back to spend again."),
                    "confirm": "Reset",
                    "action": "skills.reset_now",
                    "danger": true
                }),
            );
            if let Err(e) = r {
                toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
            }
        }
        if let Some(mut s) = world.get_mut::<SkillsScreen>(root) {
            s.selected = st.selected.clone();
        }
        let tree = world.get_resource::<SkillTree>().cloned();
        let stale = world
            .get::<SkillsScreen>(root)
            .is_some_and(|s| s.snapshot != tree);
        if stale || st.selected != before {
            rebuild(world, root);
        }
        // Hover, selection and the pulse on the canvas.
        let hovered = world
            .get_resource::<UiInputState>()
            .and_then(|s| s.hovered)
            .and_then(|e| world.get::<UiId>(e))
            .and_then(|id| id.0.strip_prefix("skill:").map(str::to_string));
        let dt = world.resource::<Time>().real_delta_secs().min(0.1);
        if let Some(canvas) = screens::find_in(world, root, "skills_canvas")
            && let Some(mut c) = world.get_mut::<SkillCanvas>(canvas)
        {
            c.time += dt;
            if c.hovered != hovered {
                c.hovered = hovered;
            }
            if c.selected != st.selected {
                c.selected = st.selected.clone();
            }
        }
    }
    sync_data(world);
}

// ---------------------------------------------------------------------------------- demo

/// The demo tree: Combat, Tech and Survival, seven skills each, a few learned.
pub fn demo_tree() -> SkillTree {
    let sk = |id: &str,
              name: &str,
              icon: &str,
              branch: usize,
              tier: u32,
              col: f32,
              max_rank: u32,
              requires: &[&str],
              description: &str,
              ranks: &[&str]| Skill {
        id: id.into(),
        name: name.into(),
        icon: icon.into(),
        branch,
        tier,
        col,
        max_rank,
        requires: requires.iter().map(|s| s.to_string()).collect(),
        description: description.into(),
        ranks: ranks.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let active = |mut s: Skill| {
        s.kind = SkillKind::Active;
        s
    };
    let ult = |mut s: Skill, level: u32| {
        s.kind = SkillKind::Ultimate;
        s.level = level;
        s.cost = 2;
        s
    };
    let ranked = |mut s: Skill, rank: u32| {
        s.rank = rank;
        s
    };
    SkillTree {
        points: 3,
        branches: vec![
            Branch {
                id: "combat".into(),
                name: "Combat".into(),
                icon: "sword".into(),
                motto: "Blade and nerve".into(),
                color: Some(Color::hexa(0xf0644cff)),
            },
            Branch {
                id: "tech".into(),
                name: "Tech".into(),
                icon: "circuit".into(),
                motto: "Rails, drones, time".into(),
                color: Some(Color::hexa(0x4cc3f0ff)),
            },
            Branch {
                id: "survival".into(),
                name: "Survival".into(),
                icon: "herb".into(),
                motto: "Live off the land".into(),
                color: Some(Color::hexa(0x6fd36aff)),
            },
        ],
        skills: vec![
            // Combat.
            ranked(
                sk(
                    "c_edge",
                    "Keen Edge",
                    "sword",
                    0,
                    0,
                    0.0,
                    3,
                    &[],
                    "Blades bite deeper. Every rank sharpens all melee weapons.",
                    &[
                        "+8% melee damage",
                        "+16% melee damage",
                        "+24% melee damage, and hits stagger light foes",
                    ],
                ),
                3,
            ),
            ranked(
                sk(
                    "c_riposte",
                    "Riposte",
                    "ability_slash",
                    0,
                    1,
                    -1.0,
                    1,
                    &["c_edge"],
                    "Parry a blow, then strike within 0.6 s for a guaranteed critical hit.",
                    &["Critical counter after a parry"],
                ),
                1,
            ),
            ranked(
                sk(
                    "c_momentum",
                    "Momentum",
                    "ability_dash",
                    0,
                    1,
                    1.0,
                    2,
                    &["c_edge"],
                    "Each hit in a combo speeds up the next.",
                    &[
                        "+5% attack speed per hit, up to 5 stacks",
                        "Stacks last twice as long",
                    ],
                ),
                1,
            ),
            ranked(
                sk(
                    "c_bulwark",
                    "Bulwark",
                    "shield",
                    0,
                    2,
                    -1.0,
                    2,
                    &["c_riposte"],
                    "Hold the line. Blocking costs less and pushes back.",
                    &[
                        "Blocking drains 30% less stamina",
                        "A perfect block knocks the attacker back",
                    ],
                ),
                1,
            ),
            sk(
                "c_cleave",
                "Cleave",
                "greatsword",
                0,
                2,
                1.0,
                2,
                &["c_momentum"],
                "Heavy swings carry through to everyone in the arc.",
                &[
                    "Heavy attacks hit up to 3 foes",
                    "Cleaved foes bleed for 4 s",
                ],
            ),
            active(sk(
                "c_whirl",
                "Whirlwind",
                "ability_lightning",
                0,
                3,
                0.0,
                1,
                &["c_bulwark", "c_cleave"],
                "Spin through a crowd, three hits on everything in reach. 12 s cooldown.",
                &["Unlocks Whirlwind"],
            )),
            ult(
                sk(
                    "c_ult",
                    "Thousand Cuts",
                    "ability_time_slow",
                    0,
                    4,
                    0.0,
                    1,
                    &["c_whirl"],
                    "Time slows around you and every strike lands twice for 6 s.",
                    &["Unlocks Thousand Cuts"],
                ),
                20,
            ),
            // Tech.
            ranked(
                sk(
                    "t_steady",
                    "Steady Hands",
                    "rifle",
                    1,
                    0,
                    0.0,
                    3,
                    &[],
                    "Aim holds still under pressure.",
                    &[
                        "-10% weapon sway",
                        "-20% weapon sway",
                        "-30% sway, and no flinch when hit while aiming",
                    ],
                ),
                2,
            ),
            sk(
                "t_overclock",
                "Overclock",
                "fuel_cell",
                1,
                1,
                -1.0,
                2,
                &["t_steady"],
                "Run hot. Charged weapons fill faster.",
                &["Weapons charge 15% faster", "Weapons charge 30% faster"],
            ),
            ranked(
                sk(
                    "t_ammo",
                    "Deep Pockets",
                    "pistol",
                    1,
                    1,
                    1.0,
                    3,
                    &["t_steady"],
                    "Room for more rounds in every pouch.",
                    &[
                        "+12 reserve ammo",
                        "+24 reserve ammo",
                        "+36 reserve ammo, reloads 10% faster",
                    ],
                ),
                1,
            ),
            active(sk(
                "t_drone",
                "Seeker Drone",
                "compass",
                1,
                2,
                -1.0,
                1,
                &["t_overclock"],
                "A drone marks every foe within 40 m for 10 s. 30 s cooldown.",
                &["Unlocks Seeker Drone"],
            )),
            sk(
                "t_cluster",
                "Cluster Charge",
                "grenade",
                1,
                2,
                1.0,
                2,
                &["t_ammo"],
                "Grenades break into bomblets.",
                &["Grenades split into 3", "Bomblets stun for 1 s"],
            ),
            sk(
                "t_focus",
                "Bullet Focus",
                "ability_time_slow",
                1,
                3,
                0.0,
                2,
                &["t_drone", "t_cluster"],
                "Aiming down sights in the air slows time around you.",
                &[
                    "Time runs at half speed while aiming mid-air",
                    "Lasts 2 s longer and recharges on kills",
                ],
            ),
            ult(
                sk(
                    "t_ult",
                    "Rail Storm",
                    "ability_lightning",
                    1,
                    4,
                    0.0,
                    1,
                    &["t_focus"],
                    "Call a line of rail strikes down a 30 m path.",
                    &["Unlocks Rail Storm"],
                ),
                22,
            ),
            // Survival.
            ranked(
                sk(
                    "s_vigor",
                    "Vigor",
                    "heart",
                    2,
                    0,
                    0.0,
                    3,
                    &[],
                    "Tougher in body and quicker to mend.",
                    &[
                        "+10 max health",
                        "+20 max health",
                        "+30 max health, and slow regeneration out of combat",
                    ],
                ),
                2,
            ),
            sk(
                "s_forager",
                "Forager",
                "mushroom_glow",
                2,
                1,
                -1.0,
                2,
                &["s_vigor"],
                "Know what grows where.",
                &[
                    "Gather twice as many herbs",
                    "Rare plants show on the minimap",
                ],
            ),
            ranked(
                sk(
                    "s_wind",
                    "Second Wind",
                    "stamina",
                    2,
                    1,
                    1.0,
                    2,
                    &["s_vigor"],
                    "Catch your breath faster.",
                    &[
                        "Stamina refills 20% faster",
                        "A dodge at low stamina costs nothing",
                    ],
                ),
                1,
            ),
            sk(
                "s_medic",
                "Field Medic",
                "medkit",
                2,
                2,
                -1.0,
                2,
                &["s_forager"],
                "Patch wounds properly.",
                &[
                    "Medkits heal 40% more",
                    "Medkits also cure poison and bleeding",
                ],
            ),
            sk(
                "s_shadow",
                "Shadowstep",
                "cloak",
                2,
                2,
                1.0,
                2,
                &["s_wind"],
                "Move unseen.",
                &[
                    "Crouched, you are 30% harder to spot",
                    "Silent landings from any height",
                ],
            ),
            active(sk(
                "s_ward",
                "Warding Bubble",
                "ability_shield_bubble",
                2,
                3,
                0.0,
                1,
                &["s_medic", "s_shadow"],
                "A bubble blocks shots from outside for 5 s. 40 s cooldown.",
                &["Unlocks Warding Bubble"],
            )),
            ult(
                sk(
                    "s_ult",
                    "Phoenix",
                    "ability_fireball",
                    2,
                    4,
                    0.0,
                    1,
                    &["s_ward"],
                    "Once every five minutes, a lethal hit leaves you standing at 40% health.",
                    &["Unlocks Phoenix"],
                ),
                18,
            ),
        ],
    }
}

/// Put the demo tree in place (unless a game already did).
pub fn add_demo_tree(world: &mut World) {
    if world
        .get_resource::<SkillTree>()
        .is_none_or(|t| t.skills.is_empty())
    {
        world.insert_resource(demo_tree());
    }
    sync_data(world);
}

/// `skills.*` APIs.
pub fn register_api(app: &mut engine_core::prelude::App) {
    app.register_api(
        "skills.tree",
        "Get the skill tree (no args) or replace it (pass {branches, skills, points}). A skill: {id, name, icon, branch (index), tier (row 0-4), col (-1, 0, 1), kind (passive, active, ultimate), rank, max_rank, cost, requires [ids], level, description, ranks [text per rank]}.",
        r#"{"points": 2, "branches": [{"id": "combat", "name": "Combat", "icon": "sword"}], "skills": [{"id": "edge", "name": "Keen Edge", "icon": "sword", "max_rank": 3, "ranks": ["+8% melee damage", "+16%", "+24%"]}]}"#,
        |world, args| {
            if args.get("skills").is_some() {
                let tree: SkillTree = serde_json::from_value(args)
                    .map_err(|e| format!("That isn't a skill tree: {e}."))?;
                let n = tree.skills.len();
                world.insert_resource(tree);
                sync_data(world);
                return Ok(json!({"ok": true, "skills": n}));
            }
            let tree = world.get_resource::<SkillTree>().cloned().unwrap_or_default();
            serde_json::to_value(&tree).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "skills.learn",
        "Spend points on the next rank of a skill: {id}. Fails with the reason (missing prerequisite, level, points, highest rank).",
        r#"{"id": "c_cleave"}"#,
        |world, args| {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Pass the skill's \"id\".")?
                .to_string();
            let rank = learn(world, &id)?;
            let points = world.get_resource::<SkillTree>().map_or(0, |t| t.points);
            Ok(json!({"ok": true, "rank": rank, "points": points}))
        },
    );
    app.register_api(
        "skills.grant",
        "Give skill points: {points} (default 1), e.g. on a level up.",
        r#"{"points": 1}"#,
        |world, args| {
            let n = args.get("points").and_then(Value::as_u64).unwrap_or(1) as u32;
            let mut tree = world
                .get_resource::<SkillTree>()
                .cloned()
                .unwrap_or_default();
            tree.points += n;
            let points = tree.points;
            world.insert_resource(tree);
            sync_data(world);
            toast::notify(
                world,
                ToastKind::Good,
                "star",
                if n == 1 {
                    "Skill point"
                } else {
                    "Skill points"
                },
                &format!("[b]{points}[/b] to spend in Skills"),
                2.6,
            );
            Ok(json!({"ok": true, "points": points}))
        },
    );
    app.register_api(
        "skills.reset",
        "Refund every spent point (no confirmation).",
        "{}",
        |world, _| {
            let refund = reset(world);
            Ok(json!({"ok": true, "refunded": refund}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prerequisites_level_and_points_gate_learning() {
        let mut t = demo_tree();
        // Cleave needs Momentum (learned): available with points.
        assert_eq!(t.state("c_cleave", 14), SkillState::Available);
        // Whirlwind needs Cleave too.
        assert_eq!(t.state("c_whirl", 14), SkillState::Locked);
        assert!(t.blocker("c_whirl", 14).unwrap().contains("Cleave"));
        // Keen Edge is maxed.
        assert_eq!(t.state("c_edge", 14), SkillState::Maxed);
        // Phoenix waits on level 18 even with its path learned.
        for id in ["s_forager", "s_medic", "s_ward"] {
            t.get_mut(id).unwrap().rank = 1;
        }
        assert!(t.blocker("s_ult", 14).unwrap().contains("level 18"));
        assert_eq!(t.blocker("s_ult", 18), None);
        // No points: learnable skills wait.
        t.points = 0;
        assert_eq!(t.state("c_cleave", 14), SkillState::Affordable);
        assert_eq!(t.state("c_momentum", 14), SkillState::Learned);
    }

    #[test]
    fn nodes_sit_inside_the_canvas_without_overlapping() {
        let t = demo_tree();
        let nb = t.branches.len();
        for a in &t.skills {
            let p = node_pos(a, nb);
            let r = node_size(a) * 0.5;
            assert!(p.x - r > 0.0 && p.x + r < TREE_W, "{} off the sides", a.id);
            assert!(
                p.y - r > 50.0 && p.y + r + 24.0 < TREE_H,
                "{} off the ends",
                a.id
            );
            for b in &t.skills {
                if a.id != b.id {
                    assert!(
                        p.distance(node_pos(b, nb)) > NODE + 10.0,
                        "{} and {}",
                        a.id,
                        b.id
                    );
                }
            }
        }
    }
}
