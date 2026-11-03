//! Model and scenery builds made on a worker ([`engine_core::off_frame`]), so spawning a car,
//! a plane or a zone's scenery doesn't hold the frame while its meshes and textures are made.
//!
//! - [`after_assets`]: a model kit (its keyed meshes, images and materials) is built once on
//!   a worker however many vehicles of the kind wait for it; each vehicle's drawn parts are
//!   spawned once it is in.
//! - [`at_anchor`]: a zone's scenery is built on a worker and placed where its anchor stands
//!   then, so a world shift during the build moves it too.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use engine_assets::Assets;
use engine_core::math::DVec3;
use engine_core::off_frame::off_frame;
use engine_core::prelude::*;
use engine_render::{Image, Mesh, StandardMaterial};

/// Whether this world draws (has the renderer's asset stores): models are only made for one.
pub(crate) fn draws(world: &World) -> bool {
    world.contains_resource::<Assets<Mesh>>()
        && world.contains_resource::<Assets<Image>>()
        && world.contains_resource::<Assets<StandardMaterial>>()
}

/// Kits being built, by guard key. A kit's entry lives as long as its job: a job dropped with
/// its cell lets the next vehicle build the kit again.
#[derive(Resource, Default)]
struct Building(HashMap<String, Weak<()>>);

/// Run `then` once the model whose mesh `guard` is registered is in: at once if it is (or
/// nothing draws here), else after `build` has run on a worker and `install` put it in. Kits
/// already being built aren't built again; a vehicle whose kit was queued in another
/// streamed cell, and isn't in yet when its own turn comes, builds it in place.
pub(crate) fn after_assets<A: Send + 'static>(
    world: &mut World,
    guard: String,
    build: fn() -> A,
    install: fn(&mut World, A),
    then: impl FnOnce(&mut World) + Send + 'static,
) {
    let ready = |world: &World, guard: &str| {
        !draws(world) || world.resource::<Assets<Mesh>>().by_key(guard).is_some()
    };
    if ready(world, &guard) {
        then(world);
        return;
    }
    world.init_resource::<Building>();
    let building = world
        .resource::<Building>()
        .0
        .get(&guard)
        .is_some_and(|w| w.strong_count() > 0);
    if !building {
        let token = Arc::new(());
        world
            .resource_mut::<Building>()
            .0
            .insert(guard.clone(), Arc::downgrade(&token));
        let g = guard.clone();
        off_frame(
            world,
            format!("vehicles kit {guard}"),
            build,
            move |world, kit| {
                drop(token);
                if !ready(world, &g) {
                    install(world, kit);
                }
            },
        );
    }
    off_frame(
        world,
        format!("vehicles parts {guard}"),
        || (),
        move |world, ()| {
            if !ready(world, &guard) {
                install(world, build());
            }
            then(world);
        },
    );
}

/// Build `make()` on a worker, then `place` it at `at` moved by any world shift since: an
/// anchor entity stands at `at` meanwhile and is replaced by what `place` spawns. Nothing
/// happens in a world that doesn't draw.
pub(crate) fn at_anchor<T: Send + 'static>(
    world: &mut World,
    name: &str,
    at: DVec3,
    make: impl FnOnce() -> T + Send + 'static,
    place: impl FnOnce(&mut World, DVec3, T) + Send + 'static,
) {
    if !draws(world) {
        return;
    }
    let anchor = world
        .spawn((
            Transform::from_translation(at),
            Name::new(format!("{name} (building)")),
        ))
        .id();
    off_frame(world, name.to_string(), make, move |world, made| {
        let Some(now) = world.get::<Transform>(anchor).map(|t| t.translation) else {
            return;
        };
        world.despawn(anchor);
        place(world, now - at, made);
    });
}
