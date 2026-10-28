//! Felling trees by hand: grown trees near the player offer "Chop" (an
//! `engine_world::Interactable`), and each press of the interact key is one axe blow, sent as a
//! `DamageEvent` so the felling code ([`crate::felling`]) cuts the tree and sound or effects
//! modules can answer the hit. A grown tree takes a handful of blows, a big one more; the
//! prompt shows how far the cut has gone, and the tree falls away from the player, as in Far
//! Cry, Red Dead Redemption 2 or Valheim (aim at a trunk, swing until it tips, step aside).
//!
//! Scenery trees (near woods, a valley's painted woods) are outside the simulation, so the
//! first blow turns one into a plant of the same species, size and genes where it stands, and
//! remembers the spot so near woods don't plant it again.

use bevy_ecs::message::MessageCursor;
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{DamageEvent, DamageKind, Interact, Interactable, Interactor};

use crate::felling::fell_threshold;
use crate::plant::{Plant, PlantStage};
use crate::scenery_tree::SceneryTree;
use crate::species::PlantLibrary;

/// How trees offer themselves to be chopped by the player.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HandFelling {
    /// Off: no tree offers "Chop".
    pub enabled: bool,
    /// Trees whose foot is within this distance of the player offer it, meters.
    pub offer_radius_m: f32,
    /// Blows a grown tree (size 1) takes; smaller and bigger trees take fewer or more.
    pub blows: f32,
    /// Most blows any tree takes.
    pub max_blows: u32,
    /// The prompt's verb.
    pub verb: String,
}

impl Default for HandFelling {
    fn default() -> Self {
        Self {
            enabled: true,
            offer_radius_m: 8.0,
            blows: 5.0,
            max_blows: 12,
            verb: "Chop".into(),
        }
    }
}

impl HandFelling {
    /// Blows a tree of `size` takes.
    pub fn blows_for(&self, size: f32) -> u32 {
        ((self.blows * size.max(0.2).powf(1.5)).round() as u32).clamp(2, self.max_blows.max(2))
    }
}

/// On a tree offering "Chop" (its [`Interactable`] was put there by hand felling, which takes
/// it off again when the player walks away or the tree is down).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct OffersChop;

/// Where scenery trees were turned into plants by a first blow (world meters, the tree's foot),
/// so whatever plants scenery trees (near woods) leaves those spots alone.
#[derive(Resource, Debug, Clone, Default)]
pub struct ChoppedSpots {
    pub spots: Vec<DVec3>,
}

impl ChoppedSpots {
    /// Whether a tree at `foot` was chopped (within half a meter).
    pub fn contains(&self, foot: DVec3) -> bool {
        self.spots.iter().any(|s| s.distance_squared(foot) < 0.25)
    }
}

impl engine_world::FollowsWorldShift for ChoppedSpots {
    fn follow(&mut self, shift: &engine_world::WorldShift) {
        for s in &mut self.spots {
            *s = shift.point(*s);
        }
    }
}

/// Height on the trunk the player aims at, meters above the foot.
const AIM_HEIGHT_M: f32 = 1.1;

/// Where the player is: the body that uses things, or the active camera without one.
fn player_position(world: &mut World) -> Option<DVec3> {
    let mut q = world.query_filtered::<&GlobalTransform, With<Interactor>>();
    if let Some(t) = q.iter(world).next() {
        return Some(t.translation());
    }
    let mut cams = world.query::<(&Camera, &GlobalTransform)>();
    cams.iter(world)
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
}

/// Put "Chop" on grown trees near the player and take it off the rest; keep each prompt showing
/// how far its cut has gone.
pub fn offer_chops(world: &mut World) {
    let Some(settings) = world.get_resource::<HandFelling>().cloned() else {
        return;
    };
    let player = if settings.enabled {
        player_position(world)
    } else {
        None
    };
    let Some(library) = world.get_resource::<PlantLibrary>() else {
        return;
    };
    let woody: Vec<bool> = library.species.iter().map(|sp| sp.is_tree()).collect();
    let near = |p: DVec3| {
        player.is_some_and(|me| {
            let d = p - me;
            d.x * d.x + d.z * d.z < (settings.offer_radius_m as f64).powi(2) && d.y.abs() < 20.0
        })
    };
    let is_tree = |species: usize| woody.get(species).copied().unwrap_or(false);

    // Plants: grown trees standing (alive, dead or burnt), not stumps.
    let mut add = Vec::new();
    let mut remove = Vec::new();
    let mut verbs = Vec::new();
    let mut q = world.query::<(Entity, &Plant, &GlobalTransform, Has<OffersChop>)>();
    for (e, plant, gt, offered) in q.iter(world) {
        let can = is_tree(plant.species.0 as usize)
            && plant.size >= 0.2
            && !matches!(plant.stage, PlantStage::Stump { .. })
            && near(gt.translation());
        if can && !offered {
            add.push((e, plant.size));
        } else if !can && offered {
            remove.push(e);
        } else if can {
            verbs.push((e, plant.species, plant.size, plant.chop_damage));
        }
    }
    let mut q = world.query::<(Entity, &SceneryTree, &GlobalTransform, Has<OffersChop>)>();
    for (e, tree, gt, offered) in q.iter(world) {
        let can = is_tree(tree.species.0 as usize) && tree.size >= 0.2 && near(gt.translation());
        if can && !offered {
            add.push((e, tree.size));
        } else if !can && offered {
            remove.push(e);
        }
    }

    for (e, size) in add {
        let reach = 1.8 + 0.4 * size;
        world.entity_mut(e).insert((
            Interactable::new(settings.verb.clone())
                .reach(reach)
                .at(Vec3::Y * AIM_HEIGHT_M),
            OffersChop,
        ));
    }
    for e in remove {
        world.entity_mut(e).remove::<(Interactable, OffersChop)>();
    }
    for (e, species, size, damage) in verbs {
        let threshold = {
            let library = world.resource::<PlantLibrary>();
            fell_threshold(library.get(species), size)
        };
        let verb = if damage > 0.0 {
            let blows = settings.blows_for(size);
            let done = ((damage / threshold) * blows as f32).round() as u32;
            format!("{} ({}/{})", settings.verb, done.min(blows - 1), blows)
        } else {
            settings.verb.clone()
        };
        if let Some(mut i) = world.get_mut::<Interactable>(e)
            && i.verb != verb
        {
            i.verb = verb;
        }
    }
}

/// Each press of the interact key on a tree offering "Chop" is one axe blow.
pub fn chop_on_interact(world: &mut World, mut cursor: Local<MessageCursor<Interact>>) {
    let presses: Vec<Interact> = match world.get_resource::<Messages<Interact>>() {
        Some(m) => cursor.read(m).copied().collect(),
        None => return,
    };
    if presses.is_empty() {
        return;
    }
    let settings = world
        .get_resource::<HandFelling>()
        .cloned()
        .unwrap_or_default();
    for press in presses {
        if !world
            .get_entity(press.target)
            .is_ok_and(|e| e.contains::<OffersChop>())
        {
            continue;
        }
        let Some(target) = as_plant(world, press.target) else {
            continue;
        };
        let (Some(plant), Some(foot)) = (
            world.get::<Plant>(target).cloned(),
            world.get::<Transform>(target).map(|t| t.translation),
        ) else {
            continue;
        };
        let Some(library) = world.get_resource::<PlantLibrary>() else {
            return;
        };
        let threshold = fell_threshold(library.get(plant.species), plant.size);
        let blows = settings.blows_for(plant.size) as f32;
        // A touch over a share, so float sums never leave the last blow short.
        let amount = threshold / blows * 1.001;
        let mut away = (foot - press.from).as_vec3();
        away.y = 0.0;
        let away = away.normalize_or(Vec3::X);
        world.write_message(DamageEvent {
            target,
            amount,
            kind: DamageKind::Slash,
            point: foot + DVec3::Y * AIM_HEIGHT_M as f64 - (away * 0.3).as_dvec3(),
            impulse: away * 40.0,
            source: Some(press.actor),
        });
    }
}

/// The plant a pressed tree stands for: itself, or for a scenery tree a plant put in its place.
fn as_plant(world: &mut World, target: Entity) -> Option<Entity> {
    if world.get::<Plant>(target).is_some() {
        return Some(target);
    }
    let tree = *world.get::<SceneryTree>(target)?;
    // Near woods trees are children of their cell: the plant stands in the world.
    let gt = world.get::<GlobalTransform>(target)?;
    let (rotation, foot) = (gt.rotation(), gt.translation());
    world.despawn(target);
    world
        .get_resource_or_insert_with(ChoppedSpots::default)
        .spots
        .push(foot);
    let mut plant = Plant::new(tree.species, tree.size, tree.genes);
    if let Some(library) = world.get_resource::<PlantLibrary>() {
        plant.age_years = plant.size * library.get(tree.species).years_to_mature;
    }
    let e = crate::ecology::spawn_sim_plant(world, crate::sim::SimPlant::new(foot, plant), None);
    let mut entity = world.entity_mut(e);
    if let Some(mut t) = entity.get_mut::<Transform>() {
        t.rotation = rotation;
    }
    entity.insert((
        Interactable::new("Chop").at(Vec3::Y * AIM_HEIGHT_M),
        OffersChop,
    ));
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::felling::FallingTree;
    use crate::plant::Genes;

    fn test_app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        app
    }

    fn press(app: &mut App, actor: Entity, target: Entity) {
        app.world.write_message(Interact {
            actor,
            target,
            from: DVec3::new(0.0, 0.0, 4.0),
        });
        app.update();
    }

    #[test]
    fn a_tree_by_the_player_falls_after_its_blows_and_away_from_them() {
        let mut app = test_app();
        let oak = PlantLibrary::default().id("oak").unwrap();
        let tree = app
            .world
            .spawn((
                Transform::from_xyz(3.0, 0.0, 4.0),
                Plant::new(oak, 1.0, Genes::default()),
            ))
            .id();
        let far = app
            .world
            .spawn((
                Transform::from_xyz(40.0, 0.0, 4.0),
                Plant::new(oak, 1.0, Genes::default()),
            ))
            .id();
        let player = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, 4.0), Interactor))
            .id();
        // A frame for the transforms to reach GlobalTransform, one to offer.
        app.update();
        app.update();
        assert_eq!(app.world.get::<Interactable>(tree).unwrap().verb, "Chop");
        assert!(app.world.get::<Interactable>(far).is_none());

        let blows = HandFelling::default().blows_for(1.0);
        for b in 1..blows {
            press(&mut app, player, tree);
            app.update();
            let verb = &app.world.get::<Interactable>(tree).unwrap().verb;
            assert_eq!(verb, &format!("Chop ({b}/{blows})"));
        }
        press(&mut app, player, tree);
        app.update();
        let mut q = app.world.query::<&FallingTree>();
        let falling: Vec<&FallingTree> = q.iter(&app.world).collect();
        assert_eq!(falling.len(), 1, "felled after {blows} blows");
        // It falls away from the player (who stood at -x).
        app.update();
        let plant = app.world.get::<Plant>(tree).unwrap();
        assert!(matches!(plant.stage, PlantStage::Stump { .. }));
        assert!(
            app.world.get::<Interactable>(tree).is_none(),
            "a stump offers nothing"
        );
        let mut q = app.world.query::<&crate::felling::LogPose>();
        let yaw = q.iter(&app.world).next().unwrap().yaw;
        assert!(yaw.cos() > 0.9, "fell toward +x, yaw {yaw}");
    }

    #[test]
    fn chopping_a_scenery_tree_makes_it_a_plant_that_stays_chopped() {
        let mut app = test_app();
        let library = PlantLibrary::default();
        let tree = SceneryTree::new(&library, "oak", 7).unwrap();
        let scenery = app
            .world
            .spawn((Transform::from_xyz(2.0, 0.0, 4.0), tree))
            .id();
        let player = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, 4.0), Interactor))
            .id();
        app.update();
        app.update();
        assert!(app.world.get::<OffersChop>(scenery).is_some());
        press(&mut app, player, scenery);
        app.update();
        assert!(app.world.get_entity(scenery).is_err());
        let mut q = app.world.query::<(Entity, &Plant, &Transform)>();
        let (plant, p, t) = q.iter(&app.world).next().expect("a plant in its place");
        assert_eq!(p.species, tree.species);
        assert!(t.translation.distance(DVec3::new(2.0, 0.0, 4.0)) < 1e-6);
        assert!(p.chop_damage > 0.0, "the first blow counted");
        assert!(app.world.get::<OffersChop>(plant).is_some());
        assert!(
            app.world
                .resource::<ChoppedSpots>()
                .contains(DVec3::new(2.0, 0.0, 4.0))
        );
    }
}
