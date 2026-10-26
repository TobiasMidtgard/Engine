//! Undo and redo. Every change the editor makes to the world is a [`Command`] applied through
//! [`History::apply`], which records how to reverse it.
//!
//! Component edits are stored as before/after JSON through the type registry, so any
//! registered component (from any module) is undoable without the editor knowing its type.
//! Repeated edits of the same thing in one drag or typing burst merge into one step.

use engine::core::registry::TypeRegistry;
use engine::prelude::*;
use serde_json::Value;

use crate::scene_ops::{self, Remap, SavedEntity};

/// One reversible change.
#[derive(Debug, Clone)]
pub enum Command {
    /// Set (or add, or remove with `None`) a registered component.
    Component {
        entity: Entity,
        name: String,
        before: Option<Value>,
        after: Option<Value>,
    },
    /// Set a registered resource.
    Resource {
        name: String,
        before: Value,
        after: Value,
    },
    /// Rename an entity (`None` removes the name).
    Rename {
        entity: Entity,
        before: Option<String>,
        after: Option<String>,
    },
    /// An entity that was created (undo removes it again).
    Spawned {
        saved: SavedEntity,
        parent: Option<Entity>,
    },
    /// An entity that was deleted (undo brings it back).
    Despawned {
        saved: SavedEntity,
        parent: Option<Entity>,
    },
    /// Moved under another parent (or to the root with `None`).
    Reparent {
        entity: Entity,
        before: Option<Entity>,
        after: Option<Entity>,
        /// Local transforms before and after, so the entity stays where it was in the world.
        before_local: Transform,
        after_local: Transform,
    },
    /// Several changes as one step (multi-selection edits, duplicate of many).
    Group(String, Vec<Command>),
}

impl Command {
    /// A short description for the History panel.
    pub fn describe(&self, world: &World) -> String {
        let who = |e: &Entity| scene_ops::label(world, *e);
        match self {
            Command::Component {
                entity,
                name,
                before,
                after,
            } => match (before, after) {
                (None, Some(_)) => format!("Add {name} to {}", who(entity)),
                (Some(_), None) => format!("Remove {name} from {}", who(entity)),
                _ => format!("Edit {name} of {}", who(entity)),
            },
            Command::Resource { name, .. } => format!("Edit {name}"),
            Command::Rename { after, .. } => {
                format!("Rename to {}", after.as_deref().unwrap_or("(none)"))
            }
            Command::Spawned { saved, .. } => {
                format!("Create {}", saved.doc.name.as_deref().unwrap_or("entity"))
            }
            Command::Despawned { saved, .. } => {
                format!("Delete {}", saved.doc.name.as_deref().unwrap_or("entity"))
            }
            Command::Reparent { entity, .. } => format!("Reparent {}", who(entity)),
            Command::Group(label, _) => label.clone(),
        }
    }

    /// Apply this command forward. Errors are plain sentences for the status bar.
    fn run(&self, world: &mut World, remap: &mut Remap) -> Result<(), String> {
        match self {
            Command::Component {
                entity,
                name,
                after,
                ..
            } => set_component(world, *entity, name, after.clone()),
            Command::Resource { name, after, .. } => set_resource(world, name, after.clone()),
            Command::Rename { entity, after, .. } => rename(world, *entity, after.clone()),
            Command::Spawned { saved, parent } => {
                scene_ops::restore_entity(world, saved, *parent, remap);
                Ok(())
            }
            Command::Despawned { saved, .. } => despawn(world, saved.old),
            Command::Reparent {
                entity,
                after,
                after_local,
                ..
            } => reparent(world, *entity, *after, *after_local),
            Command::Group(_, commands) => {
                for c in commands {
                    c.run(world, remap)?;
                }
                Ok(())
            }
        }
    }

    /// The command that reverses this one.
    fn inverse(&self) -> Command {
        match self.clone() {
            Command::Component {
                entity,
                name,
                before,
                after,
            } => Command::Component {
                entity,
                name,
                before: after,
                after: before,
            },
            Command::Resource {
                name,
                before,
                after,
            } => Command::Resource {
                name,
                before: after,
                after: before,
            },
            Command::Rename {
                entity,
                before,
                after,
            } => Command::Rename {
                entity,
                before: after,
                after: before,
            },
            Command::Spawned { saved, parent } => Command::Despawned { saved, parent },
            Command::Despawned { saved, parent } => Command::Spawned { saved, parent },
            Command::Reparent {
                entity,
                before,
                after,
                before_local,
                after_local,
            } => Command::Reparent {
                entity,
                before: after,
                after: before,
                before_local: after_local,
                after_local: before_local,
            },
            Command::Group(label, commands) => {
                Command::Group(label, commands.iter().rev().map(Command::inverse).collect())
            }
        }
    }

    /// Point every entity reference at its respawned id.
    fn remap(&mut self, map: &Remap) {
        let fix = |e: &mut Entity| {
            if let Some(new) = map.get(e) {
                *e = *new;
            }
        };
        fn fix_saved(saved: &mut SavedEntity, map: &Remap) {
            if let Some(new) = map.get(&saved.old) {
                saved.old = *new;
            }
            for c in &mut saved.children {
                fix_saved(c, map);
            }
        }
        match self {
            Command::Component { entity, .. } | Command::Rename { entity, .. } => fix(entity),
            Command::Resource { .. } => {}
            Command::Spawned { saved, parent } | Command::Despawned { saved, parent } => {
                fix_saved(saved, map);
                if let Some(p) = parent {
                    fix(p);
                }
            }
            Command::Reparent {
                entity,
                before,
                after,
                ..
            } => {
                fix(entity);
                if let Some(p) = before {
                    fix(p);
                }
                if let Some(p) = after {
                    fix(p);
                }
            }
            Command::Group(_, commands) => {
                for c in commands {
                    c.remap(map);
                }
            }
        }
    }

    /// The key edits merge on: the same component (or resource, or name) of the same entity.
    fn merge_key(&self) -> Option<(Option<Entity>, String)> {
        match self {
            Command::Component {
                entity,
                name,
                before: Some(_),
                after: Some(_),
            } => Some((Some(*entity), name.clone())),
            Command::Resource { name, .. } => Some((None, format!("resource {name}"))),
            Command::Rename { entity, .. } => Some((Some(*entity), "Name".into())),
            Command::Group(label, commands) if label.starts_with("Move") => Some((
                commands.first().and_then(|c| match c {
                    Command::Component { entity, .. } => Some(*entity),
                    _ => None,
                }),
                format!("{label} x{}", commands.len()),
            )),
            _ => None,
        }
    }

    /// Fold a later edit of the same thing into this one (keep our `before`, take its `after`).
    fn absorb(&mut self, later: Command) {
        match (self, later) {
            (
                Command::Component { after, .. },
                Command::Component {
                    after: later_after, ..
                },
            ) => *after = later_after,
            (
                Command::Resource { after, .. },
                Command::Resource {
                    after: later_after, ..
                },
            ) => *after = later_after,
            (
                Command::Rename { after, .. },
                Command::Rename {
                    after: later_after, ..
                },
            ) => *after = later_after,
            (Command::Group(_, mine), Command::Group(_, theirs)) => {
                for (m, t) in mine.iter_mut().zip(theirs) {
                    m.absorb(t);
                }
            }
            _ => {}
        }
    }
}

fn set_component(
    world: &mut World,
    entity: Entity,
    name: &str,
    value: Option<Value>,
) -> Result<(), String> {
    if world.get_entity(entity).is_err() {
        return Err(format!("entity {entity} no longer exists"));
    }
    let entry = world
        .resource::<TypeRegistry>()
        .component_or_err(name)?
        .clone();
    match value {
        Some(v) => (entry.insert)(world, entity, v),
        None => {
            (entry.remove)(world, entity);
            Ok(())
        }
    }
}

fn set_resource(world: &mut World, name: &str, value: Value) -> Result<(), String> {
    let entry = world
        .resource::<TypeRegistry>()
        .resource_or_err(name)?
        .clone();
    (entry.patch)(world, value)
}

fn rename(world: &mut World, entity: Entity, name: Option<String>) -> Result<(), String> {
    let mut e = world
        .get_entity_mut(entity)
        .map_err(|_| format!("entity {entity} no longer exists"))?;
    match name {
        Some(n) => {
            e.insert(Name::new(n));
        }
        None => {
            e.remove::<Name>();
        }
    }
    Ok(())
}

fn despawn(world: &mut World, entity: Entity) -> Result<(), String> {
    match world.get_entity_mut(entity) {
        Ok(e) => {
            e.despawn();
            Ok(())
        }
        Err(_) => Err(format!("entity {entity} no longer exists")),
    }
}

fn reparent(
    world: &mut World,
    entity: Entity,
    parent: Option<Entity>,
    local: Transform,
) -> Result<(), String> {
    let mut e = world
        .get_entity_mut(entity)
        .map_err(|_| format!("entity {entity} no longer exists"))?;
    match parent {
        Some(p) => {
            e.insert(ChildOf(p));
        }
        None => {
            e.remove::<ChildOf>();
        }
    }
    e.insert(local);
    Ok(())
}

/// Edits closer together than this (real seconds) merge into one undo step.
const MERGE_WINDOW_SECS: f64 = 0.8;

struct Entry {
    command: Command,
    label: String,
    at: f64,
    interaction: u64,
}

/// The undo and redo stacks.
#[derive(Default)]
pub struct History {
    done: Vec<Entry>,
    undone: Vec<Entry>,
    /// Bumped by the editor each time the mouse is released, so one drag is one step.
    pub interaction: u64,
    /// Whether anything changed since the scene was last saved or loaded.
    pub dirty: bool,
    /// Old → new ids of entities respawned by the last undo or redo.
    pub last_remap: Remap,
}

impl History {
    /// Apply a command now and record it. Edits of the same thing during one drag, or within
    /// [`MERGE_WINDOW_SECS`], become one step.
    pub fn apply(&mut self, world: &mut World, command: Command, now: f64) -> Result<(), String> {
        let mut remap = Remap::new();
        command.run(world, &mut remap)?;
        self.remap_all(&remap);
        let label = command.describe(world);
        self.undone.clear();
        self.dirty = true;
        if let (Some(last), Some(key)) = (self.done.last_mut(), command.merge_key())
            && last.command.merge_key() == Some(key)
            && (last.interaction == self.interaction || now - last.at < MERGE_WINDOW_SECS)
        {
            last.command.absorb(command);
            last.at = now;
            return Ok(());
        }
        self.done.push(Entry {
            command,
            label,
            at: now,
            interaction: self.interaction,
        });
        Ok(())
    }

    /// Record a change that was already made (spawns, where the editor builds the entity
    /// itself first).
    pub fn record(&mut self, world: &World, command: Command, now: f64) {
        let label = command.describe(world);
        self.undone.clear();
        self.dirty = true;
        self.done.push(Entry {
            command,
            label,
            at: now,
            interaction: self.interaction,
        });
    }

    /// Undo the last step. Returns its label.
    pub fn undo(&mut self, world: &mut World) -> Result<Option<String>, String> {
        let Some(entry) = self.done.pop() else {
            return Ok(None);
        };
        let label = entry.label.clone();
        let mut remap = Remap::new();
        let result = entry.command.inverse().run(world, &mut remap);
        self.undone.push(entry);
        self.remap_all(&remap);
        self.last_remap = remap;
        self.dirty = true;
        result.map(|_| Some(label))
    }

    /// Redo the last undone step. Returns its label.
    pub fn redo(&mut self, world: &mut World) -> Result<Option<String>, String> {
        let Some(entry) = self.undone.pop() else {
            return Ok(None);
        };
        let label = entry.label.clone();
        let mut remap = Remap::new();
        let result = entry.command.run(world, &mut remap);
        self.done.push(entry);
        self.remap_all(&remap);
        self.last_remap = remap;
        self.dirty = true;
        result.map(|_| Some(label))
    }

    /// Point every recorded command at respawned entities.
    pub fn remap_all(&mut self, map: &Remap) {
        if map.is_empty() {
            return;
        }
        for e in self.done.iter_mut().chain(self.undone.iter_mut()) {
            e.command.remap(map);
        }
    }

    /// Forget everything (after loading a scene).
    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
        self.dirty = false;
    }

    /// Labels of the done steps, oldest first, and of the undone steps, next redo first.
    pub fn labels(&self) -> (Vec<&str>, Vec<&str>) {
        (
            self.done.iter().map(|e| e.label.as_str()).collect(),
            self.undone.iter().rev().map(|e| e.label.as_str()).collect(),
        )
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_ops::{InScene, save_entity};
    use serde_json::json;

    fn world() -> App {
        let mut app = App::new();
        app.update();
        app
    }

    fn y(world: &World, e: Entity) -> f64 {
        world.get::<Transform>(e).unwrap().translation.y
    }

    fn move_to(e: Entity, from: f64, to: f64) -> Command {
        Command::Component {
            entity: e,
            name: "Transform".into(),
            before: Some(json!({"translation": [0.0, from, 0.0]})),
            after: Some(json!({"translation": [0.0, to, 0.0]})),
        }
    }

    #[test]
    fn component_edits_undo_and_redo() {
        let mut app = world();
        let w = &mut app.world;
        let e = w.spawn((InScene, Transform::default())).id();
        let mut h = History::default();
        h.apply(w, move_to(e, 0.0, 2.0), 0.0).unwrap();
        assert_eq!(y(w, e), 2.0);
        h.undo(w).unwrap();
        assert_eq!(y(w, e), 0.0);
        h.redo(w).unwrap();
        assert_eq!(y(w, e), 2.0);
    }

    #[test]
    fn one_drag_is_one_step() {
        let mut app = world();
        let w = &mut app.world;
        let e = w.spawn((InScene, Transform::default())).id();
        let mut h = History::default();
        for i in 1..=10 {
            h.apply(w, move_to(e, i as f64 - 1.0, i as f64), i as f64 * 5.0)
                .unwrap();
        }
        assert_eq!(h.labels().0.len(), 1);
        h.undo(w).unwrap();
        assert_eq!(y(w, e), 0.0);
        // A new interaction starts a new step even within the time window.
        h.redo(w).unwrap();
        h.interaction += 1;
        h.apply(w, move_to(e, 10.0, 11.0), 1000.0).unwrap();
        assert_eq!(h.labels().0.len(), 2);
    }

    #[test]
    fn delete_undo_respawns_and_later_edits_follow_the_new_id() {
        let mut app = world();
        let w = &mut app.world;
        let e = w
            .spawn((InScene, Name::new("Crate"), Transform::default()))
            .id();
        let mut h = History::default();
        h.apply(w, move_to(e, 0.0, 1.0), 0.0).unwrap();
        h.interaction += 1;
        let saved = save_entity(w, e);
        h.apply(
            w,
            Command::Despawned {
                saved,
                parent: None,
            },
            10.0,
        )
        .unwrap();
        assert!(w.get_entity(e).is_err());
        h.undo(w).unwrap(); // the crate is back with a new id
        let new = h.last_remap[&e];
        assert_eq!(y(w, new), 1.0);
        h.undo(w).unwrap(); // and the earlier move now applies to the new id
        assert_eq!(y(w, new), 0.0);
        h.redo(w).unwrap();
        h.redo(w).unwrap();
        assert!(w.get_entity(new).is_err());
    }

    #[test]
    fn add_and_remove_component() {
        let mut app = world();
        let w = &mut app.world;
        let e = w.spawn(InScene).id();
        let mut h = History::default();
        h.apply(
            w,
            Command::Component {
                entity: e,
                name: "Transform".into(),
                before: None,
                after: Some(json!({})),
            },
            0.0,
        )
        .unwrap();
        assert!(w.get::<Transform>(e).is_some());
        h.undo(w).unwrap();
        assert!(w.get::<Transform>(e).is_none());
    }
}
