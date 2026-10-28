//! Rust hot patching (cargo feature `hotpatch`): edit a system in the game's own crate while
//! the game runs and the change applies within a second or two, without a restart.
//!
//! It uses [subsecond](https://docs.rs/subsecond) through the Dioxus CLI, which watches the
//! sources, compiles only what changed, links it against the running program and sends the new
//! code's jump table; bevy_ecs (feature `hotpatching`) calls every system through that table.
//!
//! ```sh
//! cargo install dioxus-cli@0.7.10 --locked     # once (`dx`)
//! dx serve --hot-patch -p showcase --features hotpatch
//! ```
//!
//! What patches (tried with dx 0.7.10 on 2026-10-28): code in the game binary's crate
//! (`games/<game>/src`), in dev builds. A saved edit to a system, or to a function it calls, was
//! live about a second later and the game kept its state. Edits to engine crates (`crates/*`)
//! compile and "patch", but calls still reach the old code, so they need a restart, as do
//! changes to a struct, component or resource layout and a system's parameters. Luau scripts
//! reload in release builds too, and are the way to iterate on gameplay.
//!
//! Serve a game crate, not an engine crate's example: dx also recompiles every workspace crate
//! that depends on the one you edited, and stops ("Missing rustc args for replay") when one of
//! them isn't part of the program it serves.

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use bevy_ecs::{HotPatchChanges, HotPatched};
use dioxus_devtools::subsecond;
use engine_core::prelude::*;

/// Connects to `dx serve --hot-patch` and tells the schedules when new code arrived, so every
/// system picks it up (added by [`crate::ScriptPlugin`] with the `hotpatch` feature).
pub struct HotPatchPlugin;

/// Patches that arrived from the CLI's thread, taken on the main thread in `Last`.
#[derive(Resource)]
struct Patches(Mutex<Receiver<()>>);

impl Plugin for HotPatchPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx): (SyncSender<()>, Receiver<()>) = sync_channel(1);
        // Does nothing unless the game was started by `dx serve`.
        #[cfg(not(target_family = "wasm"))]
        dioxus_devtools::connect_subsecond();
        subsecond::register_handler(Arc::new(move || {
            let _ = tx.try_send(());
        }));
        app.add_message::<HotPatched>()
            .init_resource::<HotPatchChanges>()
            .insert_resource(Patches(Mutex::new(rx)))
            .add_systems(Last, take_patches);
    }
}

fn take_patches(world: &mut World) {
    let patched = world
        .resource::<Patches>()
        .0
        .lock()
        .is_ok_and(|rx| rx.try_recv().is_ok());
    if !patched {
        return;
    }
    world.write_message(HotPatched);
    world.resource_mut::<HotPatchChanges>().set_changed();
    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
        let n = d.get("script.rust_patches").unwrap_or(0.0);
        d.set("script.rust_patches", n + 1.0);
    }
    tracing::info!(target: "script", "Rust code hot patched");
}
