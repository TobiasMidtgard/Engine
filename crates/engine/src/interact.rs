//! Picks what the player would use and sends the `interact` action to it
//! ([`engine_world::interact`]).

use engine_core::prelude::*;
use engine_input::ActionState;
use engine_render::Camera;
use engine_world::interact::{Interact, InteractTarget, Interactable, Interactor, Using, pick};

/// Adds the picking and the `interact` action to [`Interact`] messages. The F3 panel's font
/// draws the prompt (`engine_debug`) until a UI takes it over.
pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InteractTarget>()
            .add_message::<Interact>()
            .add_systems(Update, (pick_target_system, interact_system).chain());
    }
}

/// `Update`: [`InteractTarget`], from the actors (the [`Interactor`]s, or the active camera
/// when there are none) and the enabled [`Interactable`]s.
pub fn pick_target_system(
    actors: Query<(Entity, &GlobalTransform, Option<&Using>), With<Interactor>>,
    cameras: Query<(Entity, &Camera, &GlobalTransform)>,
    things: Query<(Entity, &Interactable, &GlobalTransform)>,
    mut target: ResMut<InteractTarget>,
) {
    let view = cameras
        .iter()
        .filter(|(_, c, _)| c.active)
        .max_by_key(|(_, c, _)| c.order);
    let eye = view.map(|(_, _, g)| (g.translation(), g.forward()));
    let mut actors: Vec<(Entity, DVec3, Option<Entity>)> = actors
        .iter()
        .map(|(e, g, using)| (e, g.translation(), using.map(|u| u.target)))
        .collect();
    if actors.is_empty()
        && let Some((e, _, g)) = view
    {
        actors.push((e, g.translation(), None));
    }
    let mut best: Option<(Entity, Entity, f32)> = None;
    for &(actor, at, using) in &actors {
        // Busy with something: it stays the target, at any distance.
        if let Some(busy) = using.filter(|t| things.contains(*t)) {
            let d = things
                .get(busy)
                .map_or(0.0, |(_, _, g)| g.translation().distance(at) as f32);
            best = Some((actor, busy, d));
            break;
        }
        let candidates = things
            .iter()
            .filter(|(_, i, _)| i.enabled)
            .map(|(e, i, g)| {
                let point = g.0.transform_point3(i.offset.as_dvec3());
                (e, point, i.reach_m)
            });
        if let Some((e, d)) = pick(at, eye, candidates)
            && best.is_none_or(|b| d < b.2)
        {
            best = Some((actor, e, d));
        }
    }
    let next = match best {
        Some((actor, e, d)) => InteractTarget {
            actor: Some(actor),
            target: Some(e),
            verb: things
                .get(e)
                .map(|(_, i, _)| i.verb.clone())
                .unwrap_or_default(),
            distance_m: d,
        },
        None => InteractTarget::default(),
    };
    if *target != next {
        *target = next;
    }
}

/// `Update`: the `interact` action (E, gamepad West) sends [`Interact`] to the target.
pub fn interact_system(
    actions: Option<Res<ActionState>>,
    target: Res<InteractTarget>,
    places: Query<&GlobalTransform>,
    mut out: MessageWriter<Interact>,
) {
    if !actions.is_some_and(|a| a.just_pressed("interact")) {
        return;
    }
    let (Some(actor), Some(thing)) = (target.actor, target.target) else {
        return;
    };
    out.write(Interact {
        actor,
        target: thing,
        from: places.get(actor).map_or(DVec3::ZERO, |g| g.translation()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::interact::Interactable;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(engine_input::InputPlugin)
            .add_plugins(InteractPlugin);
        app
    }

    fn sent(app: &App) -> Vec<Interact> {
        let messages = app.world.resource::<Messages<Interact>>();
        messages.get_cursor().read(messages).copied().collect()
    }

    #[test]
    fn the_interact_action_reaches_the_thing_in_reach() {
        let mut app = app();
        let player = app
            .world
            .spawn((Interactor, GlobalTransform::default()))
            .id();
        let car = app
            .world
            .spawn((
                Interactable::new("Drive").reach(3.0),
                GlobalTransform::from(Transform::from_xyz(0.0, 0.0, -2.0)),
            ))
            .id();
        app.world.spawn((
            Interactable::new("Fire"),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, -30.0)),
        ));
        app.update();
        let target = app.world.resource::<InteractTarget>().clone();
        assert_eq!(target.target, Some(car));
        assert_eq!(target.verb, "Drive");
        assert!(sent(&app).is_empty());

        app.world.resource_mut::<ActionState>().tap("interact", 1);
        app.update();
        let got = sent(&app);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!((got[0].actor, got[0].target), (player, car));

        // Driving it, far away: it stays the target, so the next press gets out.
        app.world.entity_mut(player).insert((
            Using { target: car },
            GlobalTransform::from(Transform::from_xyz(500.0, 0.0, 0.0)),
        ));
        app.update();
        assert_eq!(app.world.resource::<InteractTarget>().target, Some(car));
    }
}
