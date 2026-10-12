//! NPCs and animals follow a `WorldShift`: in the frame the world is re-anchored, what they
//! remember, where they are going, their paths and the tactical world move with it, and
//! afterwards the fight and the herd carry on in the new frame as if nothing happened.

use engine_core::math::DQuat;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::world_model::Obstacle;
use engine_npc::*;
use engine_world::locomotion::{LocomotionState, MoveIntent};
use engine_world::physics::Collider;
use engine_world::{
    Ground, NoiseEvent, SimulationFocus, Water, WorldPlugin, WorldShift, WorldShiftSet,
};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    app.add_systems(PostUpdate, shift_when_ready.in_set(WorldShiftSet::Write));
    app
}

const WALL: DVec3 = DVec3::new(4.0, 1.0, -8.0);
const HERD: DVec3 = DVec3::new(0.0, 0.0, 160.0);

/// A rifleman 22 m from a player standing in the open, a wall to take cover behind, and a
/// deer herd grazing out of earshot of the gunfire.
struct Scene {
    npc: Entity,
    player: Entity,
    deer: Vec<Entity>,
}

fn scene(app: &mut App) -> Scene {
    let npc = app
        .world
        .spawn((
            Transform::from_translation(DVec3::ZERO).looking_to(Vec3::NEG_Z, Vec3::Y),
            Npc {
                home: Some(DVec3::new(1.0, 0.0, 2.0)),
                ..Default::default()
            },
            Faction::ENEMY,
            NpcSkills::veteran(),
            NpcWeapon::rifle(),
        ))
        .id();
    let player = app
        .world
        .spawn((
            Transform::from_xyz(2.0, 0.0, -22.0).looking_to(Vec3::Z, Vec3::Y),
            Faction::PLAYER,
            LocomotionState::default(),
            SimulationFocus::default(),
        ))
        .id();
    app.world.spawn((
        Transform::from_translation(WALL),
        Collider::cuboid(Vec3::new(2.5, 1.0, 0.3)),
    ));
    let deer = (0..4)
        .map(|k| {
            let a = k as f64 / 4.0 * std::f64::consts::TAU;
            app.world
                .spawn((
                    Transform::from_translation(
                        HERD + DVec3::new(a.cos() * 2.5, 0.0, a.sin() * 2.5),
                    ),
                    Animal {
                        home: Some(HERD),
                        ..Animal::new(Species::Deer)
                    },
                    Herd(1),
                ))
                .id()
        })
        .collect();
    Scene { npc, player, deer }
}

trait LookingTo {
    fn looking_to(self, dir: Vec3, up: Vec3) -> Self;
}
impl LookingTo for Transform {
    fn looking_to(mut self, dir: Vec3, up: Vec3) -> Self {
        self.look_to(dir, up);
        self
    }
}

// ------------------------------------------------------------------------------------------
// Shifting mid-action
// ------------------------------------------------------------------------------------------

/// Shifts to write (in this order, in one frame) once the rifleman is on its way somewhere and
/// has seen the player (and, if asked, a deer is running), and everything as it was just
/// before them.
#[derive(Resource)]
struct Plan {
    shifts: Vec<WorldShift>,
    npc: Entity,
    player: Entity,
    deer: Vec<Entity>,
    wait_for_flight: bool,
    before: Option<Before>,
}

#[derive(Clone)]
struct Before {
    npc: Npc,
    brain: NpcBrain,
    memory: NpcMemory,
    nav: NavAgent,
    combat: NpcCombat,
    deer: Vec<(Animal, AnimalMind, NavAgent)>,
    obstacles: Vec<Obstacle>,
    cover: Vec<CoverPoint>,
    generation: u64,
}

/// Runs in `WorldShiftSet::Write`, after this frame's fixed steps: everything it records is
/// exactly what `Apply` gets to move.
fn shift_when_ready(world: &mut World) {
    let Some(plan) = world.get_resource::<Plan>() else {
        return;
    };
    if plan.before.is_some() {
        return;
    }
    let (npc, player) = (plan.npc, plan.player);
    let moving = world.get::<NpcBrain>(npc).unwrap().destination.is_some()
        && !world.get::<NavAgent>(npc).unwrap().path.is_empty();
    let seen = world
        .get::<NpcMemory>(npc)
        .unwrap()
        .get(player)
        .is_some_and(|k| k.history.len() >= 2);
    let running = !plan.wait_for_flight
        || plan
            .deer
            .iter()
            .any(|&d| world.get::<AnimalMind>(d).unwrap().state == AnimalState::Flee);
    if !(moving && seen && running) {
        return;
    }
    let tactical = world.resource::<TacticalWorld>();
    let before = Before {
        npc: world.get::<Npc>(npc).unwrap().clone(),
        brain: world.get::<NpcBrain>(npc).unwrap().clone(),
        memory: world.get::<NpcMemory>(npc).unwrap().clone(),
        nav: world.get::<NavAgent>(npc).unwrap().clone(),
        combat: world.get::<NpcCombat>(npc).unwrap().clone(),
        deer: plan
            .deer
            .iter()
            .map(|&d| {
                (
                    world.get::<Animal>(d).unwrap().clone(),
                    world.get::<AnimalMind>(d).unwrap().clone(),
                    world.get::<NavAgent>(d).unwrap().clone(),
                )
            })
            .collect(),
        obstacles: tactical.obstacles.clone(),
        cover: tactical.cover.clone(),
        generation: tactical.generation,
    };
    let shifts = plan.shifts.clone();
    world.resource_mut::<Plan>().before = Some(before);
    for s in shifts {
        world.write_message(s);
    }
}

/// Run until the plan's shifts went out (the frame they did has applied them). `each` runs
/// after every frame before that.
fn run_until_shifted(app: &mut App, mut each: impl FnMut(&mut App)) -> Before {
    for _ in 0..60 * 15 {
        app.update();
        if let Some(before) = app.world.resource::<Plan>().before.clone() {
            return before;
        }
        each(app);
    }
    panic!("the fight never got going");
}

// ------------------------------------------------------------------------------------------
// Expected values: each shift applied in turn
// ------------------------------------------------------------------------------------------

fn point(shifts: &[WorldShift], p: DVec3) -> DVec3 {
    shifts.iter().fold(p, |p, s| s.point(p))
}

fn direction(shifts: &[WorldShift], v: Vec3) -> Vec3 {
    shifts.iter().fold(v, |v, s| s.direction(v))
}

/// The new-frame velocity of a body that moved at `v` at `p` (old frame).
fn velocity(shifts: &[WorldShift], p: DVec3, v: Vec3) -> Vec3 {
    let (mut p, mut v) = (p, v);
    for s in shifts {
        p = s.point(p);
        v = s.velocity_at(p, v);
    }
    v
}

#[track_caller]
fn moved(what: &str, shifts: &[WorldShift], before: DVec3, after: DVec3) {
    let want = point(shifts, before);
    assert!(
        after.distance(want) < 1e-6,
        "{what}: {after}, expected {want} (was {before})"
    );
}

#[track_caller]
fn moved_opt(what: &str, shifts: &[WorldShift], before: Option<DVec3>, after: Option<DVec3>) {
    assert_eq!(
        before.is_some(),
        after.is_some(),
        "{what}: {before:?} → {after:?}"
    );
    if let (Some(b), Some(a)) = (before, after) {
        moved(what, shifts, b, a);
    }
}

#[track_caller]
fn same_vec(what: &str, got: Vec3, want: Vec3) {
    assert!(
        (got - want).length() < 1e-4,
        "{what}: {got}, expected {want}"
    );
}

/// Everything the NPC module keeps in world space moved exactly with the world.
fn assert_all_moved(app: &App, scene: &Scene, shifts: &[WorldShift], before: &Before) {
    let w = &app.world;
    let npc = w.get::<Npc>(scene.npc).unwrap();
    moved_opt("Npc::home", shifts, before.npc.home, npc.home);

    let brain = w.get::<NpcBrain>(scene.npc).unwrap();
    let b = &before.brain;
    moved_opt("brain home", shifts, b.home, brain.home);
    moved_opt("destination", shifts, b.destination, brain.destination);
    moved_opt("high ground", shifts, b.high_ground, brain.high_ground);
    assert_eq!(brain.spots.len(), b.spots.len());
    for (s, was) in brain.spots.iter().zip(&b.spots) {
        moved("candidate spot", shifts, was.position, s.position);
    }

    let memory = w.get::<NpcMemory>(scene.npc).unwrap();
    assert_eq!(memory.known.len(), before.memory.known.len());
    for was in &before.memory.known {
        let k = memory.get(was.entity).expect("still remembered");
        moved(
            "last known position",
            shifts,
            was.last_position,
            k.last_position,
        );
        same_vec(
            "last known velocity",
            k.last_velocity,
            velocity(shifts, was.last_position, was.last_velocity),
        );
        assert_eq!(k.history.len(), was.history.len());
        for (h, old) in k.history.iter().zip(&was.history) {
            moved("snapshot position", shifts, old.position, h.position);
            same_vec(
                "snapshot velocity",
                h.velocity,
                velocity(shifts, old.position, old.velocity),
            );
            same_vec("snapshot facing", h.forward, direction(shifts, old.forward));
        }
    }
    moved_opt(
        "disturbance",
        shifts,
        before.memory.stimulus.map(|s| s.position),
        memory.stimulus.map(|s| s.position),
    );

    let nav = w.get::<NavAgent>(scene.npc).unwrap();
    assert_eq!(nav.path.len(), before.nav.path.len());
    for (p, was) in nav.path.iter().zip(&before.nav.path) {
        moved("waypoint", shifts, *was, *p);
    }
    moved_opt("path goal", shifts, before.nav.goal, nav.goal);

    if let AttackPhase::Windup { aim: was, .. } = before.combat.phase {
        let AttackPhase::Windup { aim, .. } = w.get::<NpcCombat>(scene.npc).unwrap().phase else {
            panic!("the shot in progress was dropped");
        };
        moved("aim point", shifts, was, aim);
    }

    for (&e, (animal, mind, agent)) in scene.deer.iter().zip(&before.deer) {
        let a = w.get::<Animal>(e).unwrap();
        moved_opt("Animal::home", shifts, animal.home, a.home);
        let m = w.get::<AnimalMind>(e).unwrap();
        moved_opt("deer home", shifts, mind.home, m.home);
        moved_opt("deer destination", shifts, mind.destination, m.destination);
        moved_opt("deer threat", shifts, mind.threat_pos, m.threat_pos);
        moved_opt("deer hurt from", shifts, mind.hurt_from, m.hurt_from);
        moved_opt(
            "deer heard",
            shifts,
            mind.heard.map(|h| h.0),
            m.heard.map(|h| h.0),
        );
        assert_eq!(mind.water.is_some(), m.water.is_some());
        moved_opt(
            "deer water",
            shifts,
            mind.water.flatten(),
            m.water.flatten(),
        );
        same_vec("deer flight", m.flee_dir, direction(shifts, mind.flee_dir));
        let nav = w.get::<NavAgent>(e).unwrap();
        assert_eq!(nav.path.len(), agent.path.len());
        for (p, was) in nav.path.iter().zip(&agent.path) {
            moved("deer waypoint", shifts, *was, *p);
        }
        moved_opt("deer path goal", shifts, agent.goal, nav.goal);
    }

    let tactical = w.resource::<TacticalWorld>();
    assert_eq!(tactical.obstacles.len(), before.obstacles.len());
    for (o, was) in tactical.obstacles.iter().zip(&before.obstacles) {
        moved("obstacle", shifts, was.center, o.center);
    }
    assert_eq!(tactical.cover.len(), before.cover.len());
    for (c, was) in tactical.cover.iter().zip(&before.cover) {
        moved("cover point", shifts, was.position, c.position);
        same_vec("cover facing", c.normal, direction(shifts, was.normal));
    }
    assert!(
        tactical.generation > before.generation,
        "navigation caches must see the change"
    );
}

/// After the shift: the rifleman still knows where the player is, looks at it, aims at it
/// and stays in the fight, and its path leads around the fight, not back to the old frame.
/// Returns the frames it spent aiming at the player and the shots it fired.
fn fight_on(app: &mut App, scene: &Scene, secs: f32) -> (u32, u32) {
    let mut aims = 0;
    let shots = app.world.get::<NpcCombat>(scene.npc).unwrap().stats.shots;
    for _ in 0..(secs * 60.0) as usize {
        app.update();
        let w = &app.world;
        let player = w.get::<Transform>(scene.player).unwrap().translation;
        let at = w.get::<Transform>(scene.npc).unwrap().translation;
        let brain = w.get::<NpcBrain>(scene.npc).unwrap();
        assert_eq!(brain.target, Some(scene.player), "lost its target");
        let k = w
            .get::<NpcMemory>(scene.npc)
            .unwrap()
            .get(scene.player)
            .unwrap();
        assert!(
            k.last_position.distance(player) < 0.5,
            "remembers the player at {} but it stands at {player}",
            k.last_position
        );
        assert!(at.distance(player) < 35.0, "wandered off to {at}");
        if let Some(d) = brain.destination {
            assert!(d.distance(player) < 35.0, "heading for {d}");
        }
        for p in &w.get::<NavAgent>(scene.npc).unwrap().path {
            assert!(p.distance(at) < 30.0, "a waypoint at {p} from {at}");
        }
        let to = Vec3::new((player.x - at.x) as f32, 0.0, (player.z - at.z) as f32).normalize();
        if let Some(look) = w.get::<MoveIntent>(scene.npc).unwrap().look {
            assert!(
                look.dot(to) > 0.97,
                "looks along {look}, the player is along {to}"
            );
        }
        if let AttackPhase::Windup {
            target: Some(t),
            aim,
            ..
        } = w.get::<NpcCombat>(scene.npc).unwrap().phase
        {
            assert_eq!(t, scene.player);
            let chest = player + DVec3::Y * 1.15;
            assert!(
                aim.distance(chest) < 1.0,
                "aims at {aim}, the player is at {player}"
            );
            aims += 1;
        }
    }
    let fired = app.world.get::<NpcCombat>(scene.npc).unwrap().stats.shots - shots;
    (aims, fired)
}

/// The herd lives around its home in the new frame.
fn herd_stays_home(app: &mut App, scene: &Scene, home: DVec3, secs: f32, reach: f64) {
    for _ in 0..(secs * 60.0) as usize {
        app.update();
    }
    for &d in &scene.deer {
        let p = app.world.get::<Transform>(d).unwrap().translation;
        assert!(p.distance(home) < reach, "a deer is at {p}, home is {home}");
        if let Some(dest) = app.world.get::<AnimalMind>(d).unwrap().destination {
            assert!(dest.distance(home) < reach, "a deer heads for {dest}");
        }
    }
}

// ------------------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------------------

#[test]
fn a_fight_and_a_herd_follow_a_floating_origin_move() {
    let mut app = app();
    let scene = scene(&mut app);
    let shift = WorldShift::translate(DVec3::new(-250_000.0, 0.0, 175_000.0));
    app.world.insert_resource(Plan {
        shifts: vec![shift],
        npc: scene.npc,
        player: scene.player,
        deer: scene.deer.clone(),
        wait_for_flight: false,
        before: None,
    });
    // Something clatters off to the side before the fight: a disturbance it remembers.
    app.world.write_message(NoiseEvent {
        position: DVec3::new(-6.0, 0.5, -4.0),
        radius: 20.0,
        source: None,
    });
    let before = run_until_shifted(&mut app, |_| {});
    assert!(before.brain.destination.is_some() && !before.nav.path.is_empty());
    assert!(!before.brain.spots.is_empty() && before.memory.stimulus.is_some());
    assert!(before.deer.iter().all(|(_, m, _)| m.home.is_some()));
    assert_all_moved(&app, &scene, &[shift], &before);

    let (aims, fired) = fight_on(&mut app, &scene, 4.0);
    assert!(
        aims > 0 && fired > 0,
        "it should keep shooting ({aims} aimed frames)"
    );
    // Long enough to wander, graze and wander again around the home in the new frame.
    herd_stays_home(&mut app, &scene, shift.point(HERD), 30.0, 40.0);
}

#[test]
fn a_fight_and_a_fleeing_herd_follow_a_turned_frame() {
    let mut app = app();
    let scene = scene(&mut app);
    // Two shifts in one frame, applied in order: a small move, then a turn about the vertical
    // into a frame the old one drifts and spins in.
    let shifts = [
        WorldShift::translate(DVec3::new(3.0, 0.0, -2.0)),
        WorldShift {
            rotation: DQuat::from_rotation_y(1.1),
            translation: DVec3::new(-5.0, 0.0, 4.0),
            velocity: DVec3::new(0.8, 0.0, -0.6),
            angular_velocity: DVec3::new(0.0, 0.02, 0.0),
        },
    ];
    app.world.insert_resource(Plan {
        shifts: shifts.to_vec(),
        npc: scene.npc,
        player: scene.player,
        deer: scene.deer.clone(),
        wait_for_flight: true,
        before: None,
    });
    // Startle the herd once the rifleman is on the move, then shift while the deer run.
    let mut startled = false;
    let before = run_until_shifted(&mut app, |app| {
        let moving = !app
            .world
            .get::<NavAgent>(scene.npc)
            .unwrap()
            .path
            .is_empty();
        if moving && !startled {
            startled = true;
            app.world.write_message(NoiseEvent {
                position: HERD + DVec3::new(0.0, 1.0, -12.0),
                radius: 60.0,
                source: None,
            });
        }
    });
    assert!(before.brain.destination.is_some() && !before.nav.path.is_empty());
    assert!(
        before
            .deer
            .iter()
            .any(|(_, m, _)| m.threat_pos.is_some() && m.flee_dir.length() > 0.5)
    );
    assert_all_moved(&app, &scene, &shifts, &before);
    // The player's own movement code would set its velocity again: it stands still.
    app.world
        .get_mut::<LocomotionState>(scene.player)
        .unwrap()
        .velocity = Vec3::ZERO;

    // A turned world is also rebuilt from the (turned) colliders on the next step: the wall is
    // where the world took it.
    let generation = app.world.resource::<TacticalWorld>().generation;
    let (mut aims, mut fired) = fight_on(&mut app, &scene, 1.0 / 60.0);
    let tactical = app.world.resource::<TacticalWorld>();
    assert!(tactical.generation > generation, "rebuilt after the turn");
    assert_eq!(tactical.obstacles.len(), 1);
    let wall = point(&shifts, WALL);
    assert!(tactical.obstacles[0].center.distance(wall) < 1e-4);
    assert_eq!(tactical.cover.len(), before.cover.len());

    // The deer keep running away from what scared them, where it is in the new frame.
    let threats: Vec<Option<DVec3>> = before
        .deer
        .iter()
        .map(|(_, m, _)| m.threat_pos.map(|p| point(&shifts, p)))
        .collect();
    let deer = |app: &App| -> Vec<(DVec3, AnimalState)> {
        scene
            .deer
            .iter()
            .map(|&d| {
                let p = app.world.get::<Transform>(d).unwrap().translation;
                (p, app.world.get::<AnimalMind>(d).unwrap().state)
            })
            .collect()
    };
    let start = deer(&app);
    let (a, f) = fight_on(&mut app, &scene, 0.5);
    (aims, fired) = (aims + a, fired + f);
    let mut running = 0;
    for ((threat, (from, was)), (to, now)) in threats.iter().zip(start).zip(deer(&app)) {
        let Some(threat) = threat else { continue };
        if was != AnimalState::Flee || now != AnimalState::Flee {
            continue;
        }
        running += 1;
        assert!(
            to.distance(*threat) > from.distance(*threat) + 1.5,
            "a fleeing deer went from {from} to {to}, the threat is at {threat}"
        );
    }
    assert!(running > 0, "no deer kept running");

    let (a, f) = fight_on(&mut app, &scene, 3.5);
    (aims, fired) = (aims + a, fired + f);
    assert!(
        aims > 0 && fired > 0,
        "it should keep shooting ({aims} aimed frames)"
    );
    herd_stays_home(&mut app, &scene, point(&shifts, HERD), 30.0, 65.0);
}

#[test]
fn the_nav_grid_is_rebuilt_where_the_world_moved() {
    let mut app = app();
    app.world.spawn((
        Transform::from_translation(WALL),
        Collider::cuboid(Vec3::new(2.5, 1.0, 0.3)),
    ));
    app.update();
    // A small move, so the grid's old chunks cover where the wall ends up.
    let shift = WorldShift::translate(DVec3::new(-9.3, 0.0, 4.6));
    let wall = shift.point(WALL);
    let (from, to) = (
        DVec3::new(wall.x, 0.0, wall.z + 3.0),
        DVec3::new(wall.x, 0.0, wall.z - 3.0),
    );
    // Free ground before the shift (this builds the chunks there).
    assert!(path_length(&mut app, from, to) < 6.5);
    app.world.write_message(shift);
    app.update();
    let tactical = app.world.resource::<TacticalWorld>();
    assert!(tactical.obstacles[0].center.distance(wall) < 1e-9);
    assert!(
        tactical
            .cover
            .iter()
            .all(|c| c.position.distance(wall) < 5.0),
        "cover moved with the wall"
    );
    assert!(
        path_length(&mut app, from, to) > 7.0,
        "the path should go around the wall"
    );
}

/// Length of the path the nav grid finds from `from` to `to` (fails if there is none).
fn path_length(app: &mut App, from: DVec3, to: DVec3) -> f64 {
    app.world.resource_scope(|w, mut grid: Mut<NavGrid>| {
        let mut ctx = grid.ctx(
            w.resource::<TacticalWorld>(),
            w.resource::<Ground>(),
            w.resource::<Water>(),
        );
        let mut path = Vec::new();
        let status = ctx.find_path(from, to, &mut |_| 0.0, &mut path);
        assert_eq!(status, PathStatus::Complete, "{path:?}");
        let mut len = 0.0;
        let mut p = from;
        for &q in &path {
            len += p.distance(q);
            p = q;
        }
        len
    })
}
