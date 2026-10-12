//! The per-frame UI pass: resolve styles (theme classes, states, transitions), inherit text
//! settings, bind and shape text, lay out with flexbox/grid (taffy), place world anchors, and
//! write every node's [`ComputedNode`] plus the draw order.

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use engine_core::math::{DVec3, Mat3, Vec2};
use engine_core::prelude::{GlobalTransform, Time, Window};
use engine_render::{Camera, Projection};
use serde::{Deserialize, Serialize};
use taffy::{AvailableSpace, NodeId, TaffyTree};

use crate::color::Color;
use crate::data::{self, UiData};
use crate::node::*;
use crate::style::{Display, Overflow, Style};
use crate::text::{Fonts, TextLayouts, TextSpec};
use crate::theme::{ClassStyle, Theme};

/// UI scaling and switches.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Player-facing UI scale multiplier.
    pub scale: f32,
    /// Layouts are designed for this window height (logical px = physical px at this height);
    /// 0 = use the OS scale factor instead.
    pub reference_height: f32,
    /// Never scale below this.
    pub min_scale: f32,
    /// Draw the UI at all.
    pub enabled: bool,
    /// Outline every node (layout debugging).
    pub debug_outlines: bool,
    /// Hover delay before tooltips, real seconds.
    pub tooltip_delay: f32,
    /// Release game actions (`ActionState`) while a modal screen is open, so the player
    /// doesn't walk or shoot while shopping.
    pub block_game_input: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            scale: 1.0,
            reference_height: 1080.0,
            min_scale: 0.5,
            enabled: true,
            debug_outlines: false,
            tooltip_delay: 0.35,
            block_game_input: true,
        }
    }
}

/// The drawing surface this frame: size in physical px and physical px per logical px.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct UiViewport {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Default for UiViewport {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            scale: 1.0,
        }
    }
}

impl UiViewport {
    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height)
    }
}

/// Visible nodes in draw order (back to front), and the top-level nodes in layer order.
#[derive(Resource, Debug, Default, Clone)]
pub struct UiDrawOrder {
    pub nodes: Vec<Entity>,
    pub roots: Vec<Entity>,
}

/// Taffy tree mirroring the UI entities.
#[derive(Resource)]
pub struct UiLayout {
    tree: TaffyTree<Entity>,
    nodes: HashMap<Entity, NodeId>,
    viewports: HashMap<Entity, NodeId>,
    theme: Option<Theme>,
    scale: f32,
}

impl Default for UiLayout {
    fn default() -> Self {
        Self {
            tree: TaffyTree::new(),
            nodes: HashMap::new(),
            viewports: HashMap::new(),
            theme: None,
            scale: 0.0,
        }
    }
}

// SAFETY: taffy packs lengths into a tagged `*const ()`, which only ever holds a pointer for
// `calc()` values. The UI never creates calc values, so the tree holds plain numbers and is
// only touched by the exclusive layout system.
unsafe impl Send for UiLayout {}
unsafe impl Sync for UiLayout {}

impl UiLayout {
    /// Taffy node count (diagnostics).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Effective style of `node` in its current interaction state.
pub fn resolve_style(theme: &Theme, node: &Node, it: &Interaction) -> Style {
    let mut s = Style::default();
    let classes: Vec<&ClassStyle> = node.classes.iter().filter_map(|c| theme.class(c)).collect();
    for c in &classes {
        c.base.apply(&mut s);
    }
    node.style.apply(&mut s);
    let mut state = |pick: fn(&ClassStyle) -> &crate::style::StylePatch,
                     inline: &crate::style::StylePatch| {
        for c in &classes {
            pick(c).apply(&mut s);
        }
        inline.apply(&mut s);
    };
    if it.hovered && !it.disabled {
        state(|c| &c.hover, &node.hover);
    }
    if it.focused {
        state(|c| &c.focused, &node.focused);
    }
    if it.selected {
        state(|c| &c.selected, &node.selected);
    }
    if it.pressed && !it.disabled {
        state(|c| &c.pressed, &node.pressed);
    }
    if it.disabled {
        state(|c| &c.disabled, &node.disabled);
    }
    s
}

/// Ease-out for open/close animations.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn viewport(world: &World) -> UiViewport {
    let window = world.resource::<Window>();
    let settings = world.resource::<UiSettings>();
    let (w, h) = (window.width.max(1) as f32, window.height.max(1) as f32);
    let base = if settings.reference_height > 0.0 {
        h / settings.reference_height
    } else {
        window.scale_factor as f32
    };
    UiViewport {
        width: w,
        height: h,
        scale: (base * settings.scale).max(settings.min_scale * settings.scale),
    }
}

fn node_children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<Children>(e)
        .map(|c| c.iter().filter(|c| world.get::<Node>(*c).is_some()).collect())
        .unwrap_or_default()
}

/// Top-level nodes (no parent node), sorted by screen layer, then spawn order.
fn roots(world: &mut World) -> Vec<Entity> {
    let mut q = world.query_filtered::<(Entity, Option<&ChildOf>, Option<&Screen>), With<Node>>();
    let mut roots: Vec<(i32, Entity)> = q
        .iter(world)
        .filter(|(_, parent, _)| match parent {
            None => true,
            Some(p) => world.get::<Node>(p.parent()).is_none(),
        })
        .map(|(e, _, s)| (s.map(|s| s.layer).unwrap_or(0), e))
        .collect();
    roots.sort_by_key(|(layer, e)| (*layer, e.index()));
    roots.into_iter().map(|(_, e)| e).collect()
}

/// Advance screen open/close animations; despawn screens that finished closing.
fn animate_screens(world: &mut World, dt: f32) {
    let mut done = Vec::new();
    let mut q = world.query::<(Entity, &mut Screen)>();
    for (e, mut s) in q.iter_mut(world) {
        if s.closing {
            s.presence -= dt / 0.16;
            if s.presence <= 0.0 {
                done.push(e);
            }
        } else if s.presence < 1.0 {
            s.presence = (s.presence + dt / 0.28).min(1.0);
        }
    }
    for e in done {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
}

/// Resolve styles and step transitions.
fn resolve_styles(world: &mut World, dt: f32, theme_changed: bool) {
    world.resource_scope(|world, theme: Mut<Theme>| {
        let mut q = world.query::<(Ref<Node>, Ref<Interaction>, &mut ComputedNode)>();
        for (node, it, mut c) in q.iter_mut(world) {
            if !c.resolved || theme_changed || node.is_changed() || it.is_changed() {
                c.target = resolve_style(&theme, &node, &it);
            }
            if !c.resolved || c.target.transition <= 0.0 {
                if !c.resolved || c.style != c.target {
                    c.style = c.target.clone();
                }
                c.resolved = true;
            } else if !c.style.visual_eq(&c.target) || c.style != c.target {
                let k = 1.0 - (-dt * 4.0 / c.target.transition).exp();
                let next = c.style.lerp_visual(&c.target, k);
                // Snap when close to avoid endless tiny updates.
                let close = (next.opacity - c.target.opacity).abs() < 0.002
                    && (next.scale - c.target.scale).abs() < 0.0005
                    && (next.translate[0] - c.target.translate[0]).abs() < 0.05
                    && (next.translate[1] - c.target.translate[1]).abs() < 0.05;
                let snap = close
                    && (k > 0.5 || next.background.approx_eq(&c.target.background, 0.004));
                c.style = if snap { c.target.clone() } else { next };
            }
        }
    });
}

/// Text spec for a node, from its `Text` and inherited settings (physical px).
fn text_spec(text: &Text, resolved: String, inherited: &Inherited, scale: f32) -> TextSpec {
    let size = if text.size > 0.0 {
        text.size
    } else {
        inherited.font_size
    };
    TextSpec {
        text: resolved,
        size: size * scale,
        weight: if text.weight > 0 {
            text.weight
        } else {
            inherited.font_weight
        },
        italic: text.italic,
        color: text.color.unwrap_or(inherited.color),
        align: text.align,
        line_height: text.line_height,
        letter_spacing: text.letter_spacing,
        uppercase: text.uppercase,
        rich: text.rich,
        tabular: text.tabular,
        wrap: text.wrap,
        family: None,
    }
}

#[derive(Clone, Copy)]
struct Inherited {
    color: Color,
    font_size: f32,
    font_weight: u16,
}

/// The whole UI pass. Runs in `PostUpdate` after transforms propagate.
pub fn ui_layout_system(world: &mut World) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let vp = viewport(world);
    if world.get_resource::<UiViewport>() != Some(&vp) {
        world.insert_resource(vp);
    }
    animate_screens(world, dt);

    let mut layout = world.remove_resource::<UiLayout>().unwrap_or_default();
    let theme_changed = layout.theme.as_ref() != Some(world.resource::<Theme>());
    if theme_changed {
        layout.theme = Some(world.resource::<Theme>().clone());
    }
    let scale_changed = layout.scale != vp.scale;
    layout.scale = vp.scale;
    resolve_styles(world, dt, theme_changed);

    let roots = roots(world);
    let mut fonts = world.remove_resource::<Fonts>().unwrap_or_default();
    let mut texts = world.remove_resource::<TextLayouts>().unwrap_or_default();

    // Pass 1: inheritance, text specs and taffy sync (depth first).
    let theme = layout.theme.clone().unwrap_or_default();
    let base = Inherited {
        color: theme.palette.text,
        font_size: theme.font_size,
        font_weight: 400,
    };
    let mut alive: Vec<Entity> = Vec::with_capacity(layout.nodes.len());
    {
        let mut stack: Vec<(Entity, Inherited)> = roots.iter().rev().map(|e| (*e, base)).collect();
        while let Some((e, inherited)) = stack.pop() {
            alive.push(e);
            let Some(c) = world.get::<ComputedNode>(e) else {
                continue;
            };
            let style = c.style.clone();
            let own = Inherited {
                color: if style.color.a > 0.0 {
                    style.color
                } else {
                    inherited.color
                },
                font_size: if style.font_size > 0.0 {
                    style.font_size
                } else {
                    inherited.font_size
                },
                font_weight: if style.font_weight > 0 {
                    style.font_weight
                } else {
                    inherited.font_weight
                },
            };
            if let Some(mut c) = world.get_mut::<ComputedNode>(e) {
                c.color = own.color;
                c.font_size = own.font_size;
                c.font_weight = own.font_weight;
            }

            // Text: bind, build the spec, reshape when it changed.
            let mut text_changed = false;
            let is_text = if let Some(text) = world.get::<Text>(e) {
                let resolved = if data::has_bindings(&text.text) {
                    data::resolve(&text.text, world.resource::<UiData>(), Some(world))
                } else {
                    text.text.clone()
                };
                let spec = text_spec(text, resolved, &own, vp.scale);
                text_changed = texts.sync(&mut fonts.system, e, &spec);
                true
            } else {
                false
            };
            let is_icon = world.get::<Icon>(e).is_some();

            // Taffy node.
            let mut tstyle = style.to_taffy(vp.scale);
            if is_icon && style.width == crate::style::Val::Auto && style.height == crate::style::Val::Auto {
                let px = own.font_size * 1.25 * vp.scale;
                tstyle.size = taffy::Size {
                    width: taffy::Dimension::length(px),
                    height: taffy::Dimension::length(px),
                };
            }
            let node = match layout.nodes.get(&e) {
                Some(n) => *n,
                None => {
                    let n = layout
                        .tree
                        .new_leaf_with_context(tstyle.clone(), e)
                        .expect("taffy node");
                    layout.nodes.insert(e, n);
                    n
                }
            };
            if layout.tree.style(node).ok() != Some(&tstyle) {
                let _ = layout.tree.set_style(node, tstyle);
            }
            if is_text && (text_changed || scale_changed) {
                let _ = layout.tree.mark_dirty(node);
            }

            let children = node_children(world, e);
            for child in children.iter().rev() {
                stack.push((*child, own));
            }
            let child_nodes: Vec<NodeId> = children
                .iter()
                .filter_map(|c| match layout.nodes.get(c) {
                    Some(n) => Some(*n),
                    None => {
                        let n = layout
                            .tree
                            .new_leaf_with_context(taffy::Style::default(), *c)
                            .ok()?;
                        layout.nodes.insert(*c, n);
                        Some(n)
                    }
                })
                .collect();
            if layout.tree.children(node).ok().as_deref() != Some(child_nodes.as_slice()) {
                let _ = layout.tree.set_children(node, &child_nodes);
            }
        }
    }

    // Forget nodes whose entities are gone.
    {
        let alive_set: std::collections::HashSet<Entity> = alive.iter().copied().collect();
        let stale: Vec<Entity> = layout
            .nodes
            .keys()
            .filter(|e| !alive_set.contains(e))
            .copied()
            .collect();
        for e in stale {
            if let Some(n) = layout.nodes.remove(&e) {
                let _ = layout.tree.remove(n);
            }
            texts.entries.remove(&e);
        }
        let stale_vp: Vec<Entity> = layout
            .viewports
            .keys()
            .filter(|e| !roots.contains(e))
            .copied()
            .collect();
        for e in stale_vp {
            if let Some(n) = layout.viewports.remove(&e) {
                let _ = layout.tree.remove(n);
            }
        }
    }

    // Pass 2: compute layout per root inside a viewport-sized container.
    let vp_style = taffy::Style {
        size: taffy::Size {
            width: taffy::Dimension::length(vp.width),
            height: taffy::Dimension::length(vp.height),
        },
        ..Default::default()
    };
    for root in &roots {
        let Some(&node) = layout.nodes.get(root) else {
            continue;
        };
        let container = match layout.viewports.get(root) {
            Some(c) => *c,
            None => {
                let c = layout
                    .tree
                    .new_with_children(vp_style.clone(), &[node])
                    .expect("viewport node");
                layout.viewports.insert(*root, c);
                c
            }
        };
        if layout.tree.style(container).ok() != Some(&vp_style) {
            let _ = layout.tree.set_style(container, vp_style.clone());
        }
        if layout.tree.children(container).ok().as_deref() != Some(&[node][..]) {
            let _ = layout.tree.set_children(container, &[node]);
        }
        let system = &mut fonts.system;
        let texts_ref = &mut texts;
        let _ = layout.tree.compute_layout_with_measure(
            container,
            taffy::Size {
                width: AvailableSpace::Definite(vp.width),
                height: AvailableSpace::Definite(vp.height),
            },
            |inputs, _id, ctx, style| {
                taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, available| {
                    let Some(entity) = ctx.map(|e| *e) else {
                        return taffy::Size::ZERO;
                    };
                    let Some(entry) = texts_ref.entries.get_mut(&entity) else {
                        return taffy::Size::ZERO;
                    };
                    let width = known.width.or(match available.width {
                        AvailableSpace::Definite(w) => Some(w),
                        AvailableSpace::MinContent => Some(0.0),
                        AvailableSpace::MaxContent => None,
                    });
                    let [w, h] = entry.measure(system, width);
                    taffy::Size {
                        width: known.width.unwrap_or(w),
                        height: known.height.unwrap_or(h),
                    }
                })
            },
        );
    }

    // Pass 3: absolute boxes, transforms, clipping, draw order.
    let anchors = project_anchors(world, &vp);
    let mut order = UiDrawOrder::default();
    let mut counter = 0u32;
    struct Frame {
        entity: Entity,
        origin: Vec2,
        scale: f32,
        offset: Vec2,
        opacity: f32,
        clip: Rect,
    }
    for root in &roots {
        let (presence, is_screen) = world
            .get::<Screen>(*root)
            .map(|s| (ease_out(s.presence), true))
            .unwrap_or((1.0, false));
        let mut root_offset = Vec2::ZERO;
        let mut root_opacity = presence;
        if let Some((center, fade, on)) = anchors.get(root).copied() {
            if let (Some(&node), true) = (layout.nodes.get(root), on) {
                let l = layout.tree.layout(node).map(|l| l.size).unwrap_or_default();
                root_offset = center - Vec2::new(l.width, l.height) * 0.5;
            } else {
                root_opacity = 0.0;
            }
            root_opacity *= fade;
        }
        let screen_scale = if is_screen { 0.985 + 0.015 * presence } else { 1.0 };
        let screen_shift = if is_screen {
            (1.0 - presence) * 14.0 * vp.scale
        } else {
            0.0
        };
        let mut deferred: Vec<(i32, Frame)> = Vec::new();
        let mut stack = vec![Frame {
            entity: *root,
            origin: root_offset + Vec2::new(0.0, screen_shift),
            scale: 1.0,
            offset: Vec2::ZERO,
            opacity: root_opacity,
            clip: vp.rect(),
        }];
        let mut first = true;
        loop {
            let Some(f) = stack.pop().or_else(|| {
                if deferred.is_empty() {
                    return None;
                }
                deferred.sort_by_key(|(z, _)| -*z);
                deferred.pop().map(|(_, f)| f)
            }) else {
                break;
            };
            let e = f.entity;
            let Some(&node) = layout.nodes.get(&e) else {
                continue;
            };
            let Ok(l) = layout.tree.layout(node).cloned() else {
                continue;
            };
            let Some(style) = world.get::<ComputedNode>(e).map(|c| c.style.clone()) else {
                continue;
            };
            if style.display == Display::None {
                if let Some(mut c) = world.get_mut::<ComputedNode>(e) {
                    c.visible = false;
                }
                continue;
            }
            // Untransformed box in the parent's coordinate system (physical px).
            let pos = f.origin + Vec2::new(l.location.x, l.location.y);
            let size = Vec2::new(l.size.width, l.size.height);
            let mut own_scale = style.scale;
            if first && is_screen {
                own_scale *= screen_scale;
            }
            first = false;
            // Own transform: scale about the center, then translate (logical px → physical).
            let center = pos + size * 0.5;
            let translate = Vec2::from(style.translate) * vp.scale;
            let s_total = f.scale * own_scale;
            let own_offset = center * (1.0 - own_scale) + translate;
            let offset_total = own_offset * f.scale + f.offset;
            let screen_min = pos * s_total + offset_total;
            let rect = Rect::new(screen_min.x, screen_min.y, size.x * s_total, size.y * s_total);
            let opacity = f.opacity * style.opacity;
            let mut clip = f.clip;
            if style.overflow != Overflow::Visible {
                clip = clip.intersect(&rect);
            }
            let visible = opacity > 0.001 && !rect.intersect(&f.clip).is_empty();

            // Scrolling.
            let mut scroll = Vec2::ZERO;
            if style.overflow == Overflow::Scroll {
                let content = content_extent(&l);
                let max = (content - size).max(Vec2::ZERO);
                if let Some(mut s) = world.get_mut::<ScrollState>(e) {
                    s.target = s.target.clamp(Vec2::ZERO, max);
                    let k = 1.0 - (-dt * 18.0).exp();
                    let delta = (s.target - s.offset) * k;
                    s.offset += delta;
                    if (s.offset - s.target).length() < 0.25 {
                        s.offset = s.target;
                    }
                    s.offset = s.offset.clamp(Vec2::ZERO, max);
                    scroll = s.offset;
                } else {
                    world.entity_mut(e).insert(ScrollState::default());
                }
            }

            if let Some(mut c) = world.get_mut::<ComputedNode>(e) {
                c.rect = rect;
                c.clip = f.clip;
                c.opacity = opacity;
                c.scale = s_total;
                c.order = counter;
                c.visible = visible;
                c.content_size = content_extent(&l);
                c.layout_size = size;
            }
            counter += 1;
            order.nodes.push(e);

            // Shape text at its final content width.
            if let Some(entry) = texts.entries.get_mut(&e) {
                let w = l.size.width
                    - l.padding.left
                    - l.padding.right
                    - l.border.left
                    - l.border.right;
                entry.shape(&mut fonts.system, Some(w.max(1.0)));
            }

            let children = node_children(world, e);
            let child_origin = pos - scroll;
            for child in children.iter().rev() {
                let frame = Frame {
                    entity: *child,
                    origin: child_origin,
                    scale: s_total,
                    offset: offset_total,
                    opacity,
                    clip,
                };
                let z = world
                    .get::<ComputedNode>(*child)
                    .map(|c| c.style.z_index)
                    .unwrap_or(0);
                if z > 0 {
                    deferred.push((
                        z,
                        Frame {
                            clip: vp.rect(),
                            ..frame
                        },
                    ));
                } else {
                    stack.push(frame);
                }
            }
        }
        order.roots.push(*root);
    }
    // Nodes not reached this frame (under display:none) are invisible.
    {
        let reached: std::collections::HashSet<Entity> = order.nodes.iter().copied().collect();
        let mut q = world.query::<(Entity, &mut ComputedNode)>();
        for (e, mut c) in q.iter_mut(world) {
            if !reached.contains(&e) && c.visible {
                c.visible = false;
            }
        }
    }

    fonts.end_frame();
    world.insert_resource(order);
    world.insert_resource(fonts);
    world.insert_resource(texts);
    world.insert_resource(layout);
}

/// Size of a node's content measured like its border box (for scroll limits).
fn content_extent(l: &taffy::Layout) -> Vec2 {
    let o = l.scrollable_overflow_rect;
    Vec2::new(
        (o.right + l.border.left + l.border.right + l.padding.right).max(l.size.width),
        (o.bottom + l.border.top + l.border.bottom + l.padding.bottom).max(l.size.height),
    )
}

/// The active camera with the highest `order`: world position, rotation, vertical fov
/// (radians, 0 = orthographic) and orthographic height.
pub fn active_camera(world: &mut World) -> Option<(DVec3, engine_core::math::Quat, f32, f32)> {
    let mut q = world.query::<(&Camera, &GlobalTransform)>();
    q.iter(world)
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(c, t)| {
            let (fov, h) = match c.projection {
                Projection::Perspective { fov_y_deg, .. } => (fov_y_deg.to_radians(), 0.0),
                Projection::Orthographic { height, .. } => (0.0, height),
            };
            (t.translation(), t.rotation(), fov, h)
        })
}

/// Project a world point to the viewport (physical px). Returns the screen point, distance
/// along the view axis and whether it is in front of the camera.
pub fn project(
    camera: (DVec3, engine_core::math::Quat, f32, f32),
    vp: &UiViewport,
    point: DVec3,
) -> (Vec2, f32, bool) {
    let (eye, rot, fov, ortho_h) = camera;
    let rel = (point - eye).as_vec3();
    let view = Mat3::from_quat(rot.inverse()) * rel;
    let depth = -view.z;
    let aspect = vp.width / vp.height;
    let (nx, ny) = if fov > 0.0 {
        let t = (fov * 0.5).tan();
        let d = depth.abs().max(1e-4);
        (view.x / (d * t * aspect), view.y / (d * t))
    } else {
        let half = ortho_h.max(1e-3) * 0.5;
        (view.x / (half * aspect), view.y / half)
    };
    let screen = Vec2::new((nx * 0.5 + 0.5) * vp.width, (0.5 - ny * 0.5) * vp.height);
    (screen, depth, depth > 0.0)
}

/// Screen position (center), fade and visibility of every anchored root; also writes the
/// anchors' distance/on-screen fields.
fn project_anchors(world: &mut World, vp: &UiViewport) -> HashMap<Entity, (Vec2, f32, bool)> {
    let mut out = HashMap::new();
    let Some(camera) = active_camera(world) else {
        return out;
    };
    let mut targets = Vec::new();
    {
        let mut q = world.query::<(Entity, &WorldAnchor)>();
        for (e, a) in q.iter(world) {
            let base = match a.entity {
                Some(t) => match world.get::<GlobalTransform>(t) {
                    Some(g) => g.translation(),
                    None => continue,
                },
                None => a.position,
            };
            targets.push((e, base + a.offset.as_dvec3()));
        }
    }
    let margin = 40.0 * vp.scale;
    for (e, point) in targets {
        let (mut screen, _, front) = project(camera, vp, point);
        let distance = (point - camera.0).length() as f32;
        let inside = front
            && screen.x >= 0.0
            && screen.y >= 0.0
            && screen.x <= vp.width
            && screen.y <= vp.height;
        let Some(mut a) = world.get_mut::<WorldAnchor>(e) else {
            continue;
        };
        a.distance = distance;
        a.on_screen = inside;
        let mut shown = inside;
        if !inside && a.clamp_to_edges {
            let c = Vec2::new(vp.width, vp.height) * 0.5;
            let mut d = screen - c;
            if !front {
                d = -d;
                if d.length_squared() < 1.0 {
                    d = Vec2::new(0.0, 1.0);
                }
            }
            let half = c - Vec2::splat(margin);
            let k = (d.x.abs() / half.x).max(d.y.abs() / half.y).max(1e-6);
            screen = c + d / k;
            a.edge_direction = d.normalize_or_zero();
            shown = true;
        } else {
            a.edge_direction = Vec2::ZERO;
        }
        let [near, far] = a.fade_distance;
        let fade = if far > near && far > 0.0 {
            1.0 - ((distance - near) / (far - near)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        out.insert(e, (screen, fade, shown));
    }
    out
}

/// Whether a node or any ancestor node has `Display::None` or a closing screen.
pub fn is_effectively_hidden(world: &World, mut e: Entity) -> bool {
    loop {
        if let Some(c) = world.get::<ComputedNode>(e)
            && (!c.visible || c.style.display == Display::None)
        {
            return true;
        }
        if let Some(s) = world.get::<Screen>(e)
            && s.closing
        {
            return true;
        }
        match world.get::<ChildOf>(e) {
            Some(p) if world.get::<Node>(p.parent()).is_some() => e = p.parent(),
            _ => return false,
        }
    }
}

/// Position helper for absolutely placed popups: keep `rect` inside the viewport.
pub fn keep_inside(rect: Rect, vp: &UiViewport, margin: f32) -> Vec2 {
    let x = rect.x.clamp(margin, (vp.width - rect.w - margin).max(margin));
    let y = rect.y.clamp(margin, (vp.height - rect.h - margin).max(margin));
    Vec2::new(x, y)
}
