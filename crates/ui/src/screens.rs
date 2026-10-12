//! Screens: top-level UI (dialogue, shop, inventory, map, HUD, menus, custom layouts) opened
//! and closed by key.
//!
//! ```ignore
//! ui::show(world, "shop", &json!({"vendor": "Merchant"}))?;   // or the `ui.show` API
//! ui::hide(world, "shop", false);
//! ```
//!
//! A screen's builder spawns its tree and returns the root entity; [`show`] adds the
//! [`Screen`] component, so opening fades and slides the screen in and closing fades it out
//! before it is despawned. Games add their own with [`ScreenRegistry::add`], or open a
//! JSON layout with `ui.show {"screen": "layout", "key": "quest_log", "layout": {...}}`.

use std::collections::BTreeMap;

use bevy_ecs::message::Messages;
use bevy_ecs::prelude::*;
use serde_json::Value;

use crate::el::{self, El};
use crate::input::UiEvent;
use crate::node::{Node, Screen};

/// Builds a screen from JSON arguments and returns its root entity.
pub type ScreenBuilder = fn(&mut World, &Value) -> Result<Entity, String>;

/// One registered screen kind.
#[derive(Clone)]
pub struct ScreenKind {
    /// One-line description (shown to AI agents by `ui.screens`).
    pub doc: String,
    /// Example arguments for `ui.show`.
    pub args_example: String,
    pub build: ScreenBuilder,
}

/// Screen builders by key.
#[derive(Resource, Default, Clone)]
pub struct ScreenRegistry {
    pub kinds: BTreeMap<String, ScreenKind>,
}

impl ScreenRegistry {
    /// Register (or replace) a screen kind.
    pub fn add(&mut self, key: &str, doc: &str, args_example: &str, build: ScreenBuilder) {
        self.kinds.insert(
            key.to_string(),
            ScreenKind {
                doc: doc.to_string(),
                args_example: args_example.to_string(),
                build,
            },
        );
    }
}

fn send(world: &mut World, ev: UiEvent) {
    if let Some(mut m) = world.get_resource_mut::<Messages<UiEvent>>() {
        m.write(ev);
    }
}

/// The open (not closing) screen with this key.
pub fn find_screen(world: &mut World, key: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Screen)>();
    q.iter(world)
        .find(|(_, s)| s.key == key && !s.closing)
        .map(|(e, _)| e)
}

/// Open screens (not closing), top layer first: (key, entity, layer, modal).
pub fn open_screens(world: &mut World) -> Vec<(String, Entity, i32, bool)> {
    let mut q = world.query::<(Entity, &Screen)>();
    let mut v: Vec<(String, Entity, i32, bool)> = q
        .iter(world)
        .filter(|(_, s)| !s.closing)
        .map(|(e, s)| (s.key.clone(), e, s.layer, s.modal))
        .collect();
    v.sort_by_key(|(_, e, layer, _)| (-*layer, std::cmp::Reverse(e.index())));
    v
}

/// Open a screen of kind `kind` with `args`. Its key is `args.key` or the kind's name. A
/// screen with the same key that is already open is rebuilt in place (without replaying the
/// opening animation). `args.instant` skips the animation, `args.modal`/`args.layer`
/// override the defaults.
pub fn show(world: &mut World, kind: &str, args: &Value) -> Result<Entity, String> {
    let key = args.get("key").and_then(Value::as_str).unwrap_or(kind).to_string();
    let key = key.as_str();
    let registry = world.resource::<ScreenRegistry>();
    let Some(kind) = registry.kinds.get(kind).cloned() else {
        let known: Vec<&str> = registry.kinds.keys().map(String::as_str).collect();
        return Err(format!(
            "There is no screen \"{kind}\". Known screens: {}. Use \"layout\" with a \"layout\" tree for a custom one.",
            known.join(", ")
        ));
    };
    let previous = find_screen(world, key);
    let presence = previous
        .and_then(|e| world.get::<Screen>(e))
        .map(|s| s.presence)
        .unwrap_or(0.0);
    if let Some(e) = previous
        && let Ok(e) = world.get_entity_mut(e)
    {
        e.despawn();
    }
    let root = (kind.build)(world, args)?;
    let mut e = world.entity_mut(root);
    let mut screen = e.get::<Screen>().cloned().unwrap_or_default();
    screen.key = key.to_string();
    screen.closing = false;
    screen.presence = if args.get("instant").and_then(Value::as_bool) == Some(true) {
        1.0
    } else {
        presence
    };
    if let Some(m) = args.get("modal").and_then(Value::as_bool) {
        screen.modal = m;
    }
    if let Some(l) = args.get("layer").and_then(Value::as_i64) {
        screen.layer = l as i32;
    }
    e.insert(screen);
    if previous.is_none() {
        send(
            world,
            UiEvent::ScreenOpened {
                screen: key.to_string(),
            },
        );
    }
    Ok(root)
}

/// Start closing a screen entity (it fades out, then is despawned).
pub fn close_screen(world: &mut World, entity: Entity, instant: bool) {
    let Some(mut s) = world.get_mut::<Screen>(entity) else {
        return;
    };
    if s.closing {
        return;
    }
    s.closing = true;
    let key = s.key.clone();
    if instant && let Ok(e) = world.get_entity_mut(entity) {
        e.despawn();
    }
    send(world, UiEvent::ScreenClosed { screen: key });
}

/// Close the screen with this key. Returns whether one was open.
pub fn hide(world: &mut World, key: &str, instant: bool) -> bool {
    match find_screen(world, key) {
        Some(e) => {
            close_screen(world, e, instant);
            true
        }
        None => false,
    }
}

/// Close every screen except the HUD (and except `keep`).
pub fn hide_all(world: &mut World, keep: &[&str], instant: bool) -> usize {
    let open = open_screens(world);
    let mut n = 0;
    for (key, e, _, _) in open {
        if key == "hud" || keep.contains(&key.as_str()) {
            continue;
        }
        close_screen(world, e, instant);
        n += 1;
    }
    n
}

/// Spawn `el` as a screen root (helper for builders).
pub fn spawn_screen(world: &mut World, screen: Screen, el: &El) -> Result<el::Spawned, String> {
    let spawned = el::spawn(world, None, el)?;
    if let Some(root) = spawned.root {
        world.entity_mut(root).insert(screen);
    }
    Ok(spawned)
}

/// The built-in `layout` screen: any [`El`] tree from JSON.
pub fn build_layout(world: &mut World, args: &Value) -> Result<Entity, String> {
    let layout = args.get("layout").ok_or(
        "Pass the screen's tree in \"layout\", e.g. {\"layout\": {\"class\": \"panel\", \"children\": [{\"text\": \"Hi\"}]}}.",
    )?;
    let el: El = serde_json::from_value(layout.clone())
        .map_err(|e| format!("The layout doesn't match the UI element format: {e}."))?;
    let screen = Screen {
        modal: args.get("modal").and_then(Value::as_bool).unwrap_or(false),
        layer: args.get("layer").and_then(Value::as_i64).unwrap_or(10) as i32,
        ..Default::default()
    };
    // A full-screen, pointer-transparent root so the layout can be placed anywhere.
    let root = El::new()
        .with_style(|s| s.fill_parent().pick(crate::style::Pick::Ignore))
        .child(el);
    let spawned = spawn_screen(world, screen, &root)?;
    spawned.root.ok_or_else(|| "The layout is empty.".to_string())
}

/// Every node entity under (and including) `root`, depth first.
pub fn subtree(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if world.get::<Node>(e).is_none() {
            continue;
        }
        out.push(e);
        if let Some(c) = world.get::<Children>(e) {
            stack.extend(c.iter().rev());
        }
    }
    out
}

/// The node with [`UiId`](crate::node::UiId) `id` under `root`.
pub fn find_in(world: &World, root: Entity, id: &str) -> Option<Entity> {
    subtree(world, root)
        .into_iter()
        .find(|e| world.get::<crate::node::UiId>(*e).is_some_and(|i| i.0 == id))
}

/// The top-level node (screen root) that `e` belongs to.
pub fn root_of(world: &World, mut e: Entity) -> Entity {
    while let Some(p) = world.get::<ChildOf>(e) {
        if world.get::<Node>(p.parent()).is_none() {
            break;
        }
        e = p.parent();
    }
    e
}

/// Whether `e` is `ancestor` or below it.
pub fn is_descendant(world: &World, mut e: Entity, ancestor: Entity) -> bool {
    loop {
        if e == ancestor {
            return true;
        }
        match world.get::<ChildOf>(e) {
            Some(p) => e = p.parent(),
            None => return false,
        }
    }
}
