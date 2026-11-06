//! Spells in a headless world with a small physics stand-in (spheres for bodies, a flat
//! ground): bolts hit and burn, frost freezes, fire on chill makes steam, chains hop, fields
//! tick, spells meet, force throws, blink moves.

use std::sync::{Arc, RwLock};

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_magic::*;
use engine_world::physics::{
    ApplyImpulse, BodyKind, Collider, ColliderShape, Hit, PhysicsQueries, PhysicsQuery,
    QueryFilter, RigidBody,
};
use engine_world::{
    DamageEvent, Died, FreezeRequest, IgniteRequest, SpellCue, SpellLook, SpellMark, SpellRole,
    Statuses, WorldPlugin,
};
use serde_json::json;

/// Bodies as spheres (center, radius), and the ground at y = 0.
#[derive(Default)]
struct Scene {
    bodies: RwLock<Vec<(Entity, DVec3, f32)>>,
    ground: RwLock<Option<Entity>>,
}

impl Scene {
    fn sweep(
        &self,
        origin: DVec3,
        dir: Vec3,
        max: f32,
        r: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let d = dir.as_dvec3();
        for &(e, c, radius) in self.bodies.read().unwrap().iter() {
            if filter.exclude.contains(&e) {
                continue;
            }
            let rr = (radius + r) as f64;
            let oc = origin - c;
            let b = oc.dot(d);
            let cc = oc.length_squared() - rr * rr;
            let disc = b * b - cc;
            if disc < 0.0 {
                continue;
            }
            let t = -b - disc.sqrt();
            let t = if t < 0.0 && cc < 0.0 { 0.0 } else { t };
            if t < 0.0 || t > max as f64 {
                continue;
            }
            if best.as_ref().is_none_or(|h| (t as f32) < h.distance) {
                let p = origin + d * t;
                best = Some(Hit {
                    entity: e,
                    point: p,
                    normal: (p - c).as_vec3().normalize_or(Vec3::Y),
                    distance: t as f32,
                    surface: Default::default(),
                });
            }
        }
        if let Some(g) = *self.ground.read().unwrap()
            && dir.y < -1e-4
        {
            let t = ((origin.y - r as f64) / -(dir.y as f64)).max(0.0);
            if t <= max as f64 && best.as_ref().is_none_or(|h| (t as f32) < h.distance) {
                let p = origin + d * t - DVec3::Y * r as f64;
                best = Some(Hit {
                    entity: g,
                    point: DVec3::new(p.x, 0.0, p.z),
                    normal: Vec3::Y,
                    distance: t as f32,
                    surface: Default::default(),
                });
            }
        }
        best
    }
}

struct Stand(Arc<Scene>);

impl PhysicsQuery for Stand {
    fn raycast(&self, origin: DVec3, dir: Vec3, max: f32, filter: &QueryFilter) -> Option<Hit> {
        self.0.sweep(origin, dir, max, 0.0, filter)
    }
    fn shapecast(
        &self,
        shape: &ColliderShape,
        origin: DVec3,
        _: Quat,
        dir: Vec3,
        max: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let r = match shape {
            ColliderShape::Sphere { radius } => *radius,
            _ => 0.2,
        };
        self.0.sweep(origin, dir, max, r, filter)
    }
    fn overlap(&self, _: &ColliderShape, _: DVec3, _: Quat, _: &QueryFilter, _: &mut Vec<Entity>) {}
    fn closest_point(&self, _: DVec3, _: f32, _: &QueryFilter) -> Option<Hit> {
        None
    }
}

/// A body the stand-in physics sees, as a sphere this high above its feet.
#[derive(Component)]
struct Body(f32);

#[derive(Resource, Default)]
struct Seen {
    hits: Vec<SpellHit>,
    met: Vec<SpellsMet>,
    damage: Vec<DamageEvent>,
    impulses: Vec<ApplyImpulse>,
    ignites: usize,
    cues: Vec<SpellCue>,
    freezes: Vec<FreezeRequest>,
    died: Vec<Died>,
}

fn snapshot(scene: Res<SceneRes>, q: Query<(Entity, &GlobalTransform, &Body)>) {
    let mut b = scene.0.bodies.write().unwrap();
    b.clear();
    for (e, g, body) in &q {
        b.push((e, g.translation() + DVec3::Y * 1.0, body.0));
    }
}

#[derive(Resource)]
struct SceneRes(Arc<Scene>);

#[allow(clippy::too_many_arguments)]
fn collect(
    mut seen: ResMut<Seen>,
    mut hits: MessageReader<SpellHit>,
    mut met: MessageReader<SpellsMet>,
    mut damage: MessageReader<DamageEvent>,
    mut impulses: MessageReader<ApplyImpulse>,
    mut ignites: MessageReader<IgniteRequest>,
    mut cues: MessageReader<SpellCue>,
    mut freezes: MessageReader<FreezeRequest>,
    mut died: MessageReader<Died>,
) {
    seen.cues.extend(cues.read().cloned());
    seen.freezes.extend(freezes.read().copied());
    seen.died.extend(died.read().copied());
    seen.hits.extend(hits.read().cloned());
    seen.met.extend(met.read().copied());
    seen.damage.extend(damage.read().cloned());
    seen.impulses.extend(impulses.read().copied());
    seen.ignites += ignites.read().count();
}

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, MagicPlugin));
    let scene = Arc::new(Scene::default());
    app.world
        .insert_resource(PhysicsQueries(Arc::new(Stand(scene.clone()))));
    let ground = app.world.spawn(Name::new("Ground")).id();
    *scene.ground.write().unwrap() = Some(ground);
    app.world.insert_resource(SceneRes(scene));
    app.world.init_resource::<Seen>();
    app.add_systems(FixedUpdate, snapshot.in_set(SimSet::Physics));
    app.add_systems(FixedUpdate, collect.after(MagicSet::Flush));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn mage(app: &mut App, at: DVec3, mastery: Mastery) -> Entity {
    app.world
        .spawn((
            Name::new("Mage"),
            Transform::from_translation(at),
            MagicCaster {
                mana_regen: 0.0,
                ..MagicCaster::new(40.0, mastery)
            },
            Vitality::new(100.0),
        ))
        .id()
}

fn dummy(app: &mut App, at: DVec3) -> Entity {
    app.world
        .spawn((
            Name::new("Dummy"),
            Transform::from_translation(at),
            Vitality {
                revive_secs: 0.0,
                ..Vitality::new(500.0)
            },
            Body(0.5),
        ))
        .id()
}

fn run(app: &mut App, secs: f32) {
    for _ in 0..(secs * 60.0).round() as usize {
        app.update();
    }
}

fn cast(app: &mut App, caster: Entity, design: SpellDesign, aim: Aim) {
    app.world.write_message(CastRequest {
        caster,
        spell: SpellChoice::Design(design),
        aim,
        charge: 0.0,
        gathered: false,
    });
}

fn hits_on(app: &App, e: Entity) -> Vec<SpellHit> {
    app.world
        .resource::<Seen>()
        .hits
        .iter()
        .filter(|h| h.target == Some(e))
        .cloned()
        .collect()
}

#[test]
fn a_fire_bolt_hits_a_dummy_and_burns_it() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -12.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 2.0);
    let hits = hits_on(&app, d);
    assert!(!hits.is_empty(), "the bolt hits");
    assert!(hits[0].damage > 0.0);
    let health = app.world.get::<Vitality>(d).unwrap().health;
    assert!(health < 500.0, "{health}");
    let a = app.world.get::<Afflictions>(d).expect("afflicted");
    assert!(a.has(Status::Burning) || a.has_aura(Element::Fire));
    let seen = app.world.resource::<Seen>();
    assert_eq!(
        seen.ignites, 0,
        "a hit on a body that won't burn lights no ground under it"
    );
    assert!(seen.damage.iter().any(|x| x.target == d));
    // The caster paid for it.
    let c = app.world.get::<MagicCaster>(me).unwrap();
    assert!(c.mana < c.max_mana);
    // A bolt into the ground asks the world to catch fire there.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Point(DVec3::new(5.0, 0.0, -8.0)),
    );
    run(&mut app, 2.0);
    assert!(
        app.world.resource::<Seen>().ignites > 0,
        "fire on the ground asks the world to ignite"
    );
}

/// The spell looks on the world's entities, by role.
fn looks_with(app: &mut App, role: SpellRole) -> Vec<(Entity, SpellLook)> {
    let mut q = app.world.query::<(Entity, &SpellLook)>();
    q.iter(&app.world)
        .filter(|(_, l)| l.role == role)
        .map(|(e, l)| (e, l.clone()))
        .collect()
}

#[test]
fn spells_speak_the_engine_contracts() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = app
        .world
        .spawn((
            Name::new("Frail dummy"),
            Transform::from_translation(DVec3::new(0.0, 0.0, -12.0)),
            Vitality::new(5.0),
            Body(0.5),
        ))
        .id();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    // Gathering in the hand, then a bolt in flight that carries its look.
    app.update();
    app.update();
    assert!(
        looks_with(&mut app, SpellRole::Charge)
            .iter()
            .any(|(e, l)| *e == me && l.look == "fire")
    );
    let mut flying = None;
    for _ in 0..90 {
        app.update();
        if let Some(b) = looks_with(&mut app, SpellRole::Bolt).pop() {
            flying = Some(b);
            break;
        }
    }
    let (bolt, look) = flying.expect("the bolt carries a SpellLook");
    assert_eq!(look.look, "fire");
    assert!(look.power_j > 1.0e3 && look.scale > 0.0);
    run(&mut app, 2.0);
    assert!(
        app.world.get::<SpellLook>(bolt).is_none(),
        "a spent bolt's look comes off"
    );
    assert!(looks_with(&mut app, SpellRole::Charge).is_empty());
    let seen = app.world.resource::<Seen>();
    assert!(
        seen.cues
            .iter()
            .any(|c| matches!(c, SpellCue::Impact { look, .. } if look == "fire")),
        "an impact cue"
    );
    // It went down once, and the world was told who did it.
    let deaths: Vec<_> = seen.died.iter().filter(|x| x.entity == d).collect();
    assert_eq!(deaths.len(), 1);
    assert_eq!(deaths[0].killer, Some(me));
    // Its afflictions are on it for everyone else.
    assert!(app.world.get::<Statuses>(d).is_some());
    // Frost on the ground asks the water to freeze and leaves a frost mark.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Bolt),
        Aim::Point(DVec3::new(4.0, 0.0, -9.0)),
    );
    run(&mut app, 2.0);
    let seen = app.world.resource::<Seen>();
    assert!(!seen.freezes.is_empty(), "frost asks water to freeze");
    assert!(seen.cues.iter().any(|c| matches!(
        c,
        SpellCue::Mark {
            kind: SpellMark::Frost,
            ..
        }
    )));
}

#[test]
fn frost_five_times_freezes() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    run(&mut app, 0.1);
    for _ in 0..5 {
        cast(
            &mut app,
            me,
            SpellDesign::new(&["frost"], CastType::Bolt),
            Aim::Entity(d),
        );
        run(&mut app, 0.8);
    }
    let a = app.world.get::<Afflictions>(d).expect("afflicted");
    assert!(
        a.has(Status::Frozen),
        "chill {} frozen {}",
        a.chill,
        a.frozen
    );
    let scale = app
        .world
        .get::<LocalTimeScale>(d)
        .expect("frozen bodies run slow");
    assert!(scale.factor < 0.05);
    // Everyone else sees it frozen.
    assert!(app.world.get::<Statuses>(d).is_some_and(|s| s.frozen));
    // A status look rides on it.
    let mut q = app.world.query::<&runtime::FollowLook>();
    assert!(
        q.iter(&app.world)
            .any(|f| f.owner == d && f.status == Status::Frozen)
    );
}

#[test]
fn fire_on_a_chilled_dummy_makes_steam() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 0.8);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 0.8);
    let hits = hits_on(&app, d);
    assert!(
        hits.iter().any(|h| h.reaction == Some("steam")),
        "{:?}",
        hits.iter().map(|h| h.reaction).collect::<Vec<_>>()
    );
    // A steam cloud stays.
    let mut q = app.world.query::<&runtime::Field>();
    assert!(
        q.iter(&app.world)
            .any(|f| f.cast.spell.weave.key == "steam")
    );
}

#[test]
fn a_split_volley_shares_its_energy() {
    let single = SpellDesign::new(&["fire"], CastType::Bolt);
    let volley = single.clone().with(Modifier::Split { count: 5 });
    let ctx = CastContext::new(Mastery::Master, Power::default());
    let one = compile(&single, ctx).unwrap();
    let five = compile(&volley, ctx).unwrap();
    let total = five.payload.damage * five.carrier.count as f32;
    assert_eq!(five.carrier.count, 5);
    assert!(
        total > one.payload.damage && total < one.payload.damage * 1.5,
        "{total} vs {}",
        one.payload.damage
    );
}

#[test]
fn a_chain_hops_to_three_dummies() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let ds: Vec<Entity> = (0..4)
        .map(|i| dummy(&mut app, DVec3::new(i as f64 * 4.0, 0.0, -10.0)))
        .collect();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["storm"], CastType::Chain),
        Aim::Entity(ds[0]),
    );
    run(&mut app, 1.5);
    let hit = ds.iter().filter(|d| !hits_on(&app, **d).is_empty()).count();
    assert!(hit >= 3, "{hit} dummies hit");
}

#[test]
fn an_aura_ticks_on_what_stands_in_it() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(1.5, 0.0, 0.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Aura),
        Aim::Direction(Vec3::NEG_Z),
    );
    run(&mut app, 3.0);
    let n = hits_on(&app, d).len();
    assert!(n >= 4, "{n} ticks");
    // Its own caster is spared.
    assert!(hits_on(&app, me).is_empty());
}

#[test]
fn a_fire_bolt_through_a_frost_wall_bursts_into_steam() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -20.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Wall),
        Aim::Point(DVec3::new(0.0, 0.0, -9.0)),
    );
    run(&mut app, 1.2);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 1.5);
    let met = &app.world.resource::<Seen>().met;
    assert!(met.iter().any(|m| m.recipe == "steam"), "{met:?}");
    // The wall turned to steam.
    let mut q = app.world.query::<&runtime::Field>();
    assert!(
        q.iter(&app.world)
            .any(|f| f.cast.spell.weave.key == "steam" && f.wall.is_some())
    );
}

#[test]
fn force_throws_a_crate_away() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let crate_e = app
        .world
        .spawn((
            Name::new("Crate"),
            Transform::from_translation(DVec3::new(0.0, 0.0, -8.0)),
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some(30.0),
                ..Default::default()
            },
            Body(0.5),
        ))
        .id();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["push"], CastType::Bolt),
        Aim::Entity(crate_e),
    );
    run(&mut app, 1.0);
    let imp = app
        .world
        .resource::<Seen>()
        .impulses
        .iter()
        .find(|i| i.entity == crate_e)
        .copied()
        .expect("an impulse on the crate");
    assert!(imp.impulse.z < -10.0, "{:?}", imp.impulse);
}

#[test]
fn blink_moves_the_caster() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["blink"], CastType::OnSelf),
        Aim::Direction(Vec3::NEG_Z),
    );
    run(&mut app, 1.0);
    let p = app.world.get::<Transform>(me).unwrap().translation;
    assert!(p.z < -2.5, "{p}");
}

#[test]
fn the_api_crafts_combines_and_casts() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    let w = call_api(
        &mut app.world,
        "magic.combine",
        json!({"effects": ["frost", "fire"]}),
    )
    .unwrap();
    assert_eq!(w["name"], "Steam");
    let c = call_api(
        &mut app.world,
        "magic.craft",
        json!({"design": {"effects": ["fire"], "cast": "bolt", "modifiers": [
            {"kind": "split", "count": 5}, {"kind": "seek"}, {"kind": "blast", "radius_m": 3.0},
            {"kind": "linger", "secs": 8.0, "radius_m": 2.0}]}, "mastery": "master"}),
    )
    .unwrap();
    assert_eq!(c["spell"]["complexity"], 11, "{c}");
    let refused = call_api(
        &mut app.world,
        "magic.craft",
        json!({"design": {"effects": ["fire"], "cast": "bolt", "modifiers": [{"kind": "split", "count": 5}]}, "mastery": "novice"}),
    )
    .unwrap();
    assert_eq!(refused["ok"], false);
    run(&mut app, 0.1);
    call_api(
        &mut app.world,
        "magic.cast",
        json!({"caster": {"id": me.to_bits()}, "effects": ["storm"], "cast": "bolt", "target": {"id": d.to_bits()}}),
    )
    .unwrap();
    run(&mut app, 1.0);
    assert!(!hits_on(&app, d).is_empty());
    let info = call_api(
        &mut app.world,
        "magic.caster",
        json!({"entity": {"id": me.to_bits()}}),
    )
    .unwrap();
    assert!(
        info["caster"]["mana"].as_f64().unwrap() < info["caster"]["max_mana"].as_f64().unwrap()
    );
}

/// The stones standing in the world: kind, base and how high their foot stands now (the rock
/// is centered on its transform).
fn stones(app: &mut App) -> Vec<(Entity, stone::StoneBody, f64)> {
    let mut q = app.world.query::<(Entity, &stone::StoneBody, &Transform)>();
    q.iter(&app.world)
        .map(|(e, s, t)| (e, s.clone(), t.translation.y - s.height as f64 / 2.0))
        .collect()
}

#[test]
fn a_stone_wall_stands_as_solid_blocks_and_sinks_with_its_wall() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["stone"], CastType::Wall).with(Modifier::Duration { factor: 0.4 }),
        Aim::Point(DVec3::new(0.0, 0.0, -8.0)),
    );
    run(&mut app, 1.2);
    let wall = stones(&mut app);
    assert!(wall.len() >= 3, "a block per piece: {}", wall.len());
    for (e, s, y) in &wall {
        assert_eq!(s.kind, stone::StoneKind::Wall);
        assert!(s.height > 2.0, "head-high: {}", s.height);
        assert!(y.abs() < 0.05, "stood up out of the ground: {y}");
        let body = app.world.get::<RigidBody>(*e).expect("a physics body");
        assert_eq!(body.kind, BodyKind::Kinematic);
        assert!(app.world.get::<Collider>(*e).is_some(), "solid");
    }
    // The rock is the look: no particle wall on its pieces.
    assert!(looks_with(&mut app, SpellRole::Wall).is_empty());
    // The wall lasts 3.2 s; its stones sink with it and are gone.
    run(&mut app, 6.0);
    assert!(stones(&mut app).is_empty());
}

#[test]
fn a_stone_bolt_raises_a_step_and_grips_a_body_until_fire_cracks_it() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    run(&mut app, 0.1);
    // Into the floor: a step.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["stone"], CastType::Bolt),
        Aim::Point(DVec3::new(4.0, 0.0, -6.0)),
    );
    run(&mut app, 1.5);
    let steps: Vec<_> = stones(&mut app)
        .into_iter()
        .filter(|(_, s, _)| s.kind == stone::StoneKind::Block)
        .collect();
    assert_eq!(steps.len(), 1);
    assert!((steps[0].1.base - DVec3::new(4.0, 0.0, -6.0)).length() < 0.8);
    // Into the dummy: stone closes around its feet and roots it.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["stone"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 1.5);
    assert!(app.world.get::<Statuses>(d).is_some_and(|s| s.rooted));
    let grips = stones(&mut app)
        .into_iter()
        .filter(|(_, s, _)| s.kind == stone::StoneKind::Grip)
        .count();
    assert_eq!(grips, 4, "four rocks around the feet");
    // Fire cracks it early.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 1.5);
    assert!(app.world.get::<Statuses>(d).is_some_and(|s| !s.rooted));
    run(&mut app, 1.0);
    assert!(
        !stones(&mut app)
            .iter()
            .any(|(_, s, _)| s.kind == stone::StoneKind::Grip),
        "the grip's rocks sank"
    );
}

#[test]
fn a_pillar_rises_under_a_loose_body_and_lifts_it_instead_of_a_field() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let crate_ = app
        .world
        .spawn((
            Name::new("Crate"),
            Transform::from_translation(DVec3::new(0.0, 0.0, -9.0)),
            RigidBody::dynamic(),
            Body(0.5),
        ))
        .id();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["lift", "stone"], CastType::Area),
        Aim::Point(DVec3::new(0.0, 0.0, -9.0)),
    );
    run(&mut app, 3.0);
    let pillars: Vec<_> = stones(&mut app)
        .into_iter()
        .filter(|(_, s, _)| s.kind == stone::StoneKind::Pillar)
        .collect();
    assert_eq!(pillars.len(), 1);
    let (pe, p, y) = &pillars[0];
    assert!(
        app.world.get::<Collider>(*pe).is_some(),
        "solid once grounded"
    );
    assert!(
        p.base.y.abs() < 0.05,
        "on the floor, not on the crate: {}",
        p.base
    );
    assert!((3.0..=6.5).contains(&p.height), "{}", p.height);
    assert!(y.abs() < 0.05, "risen: {y}");
    // The stone does the lifting: no lifting field pushing the crate.
    let seen = app.world.resource::<Seen>();
    assert!(!seen.impulses.iter().any(|i| i.entity == crate_));
    let mut fields = app.world.query::<&engine_magic::runtime::Field>();
    assert_eq!(fields.iter(&app.world).count(), 0);
}

#[test]
fn a_status_look_grows_with_the_status() {
    let low = Statuses {
        burning: 0.1,
        ..Default::default()
    };
    let high = Statuses {
        burning: 1.0,
        ..Default::default()
    };
    let small = runtime::status_scale(Status::Burning, &low);
    let big = runtime::status_scale(Status::Burning, &high);
    assert!(small < 0.65 && (big - 1.2).abs() < 1e-4, "{small} {big}");
    assert_eq!(runtime::status_scale(Status::Frozen, &high), 1.0);
}

/// A weaver with the starter spell bar, driven by script (as a film or an AI would).
fn weaver(app: &mut App, at: DVec3) -> Entity {
    let mut c = MagicCaster {
        mana_regen: 0.0,
        ..MagicCaster::new(40.0, Mastery::Adept)
    };
    for (d, g) in engine_magic::player::starter_spells() {
        c = c.learn(d, g);
    }
    app.world
        .spawn((
            Name::new("Weaver"),
            Transform::from_translation(at),
            c,
            Vitality::new(100.0),
            engine_magic::player::SpellBar::default(),
            engine_magic::player::CastInput::default(),
            engine_magic::player::ScriptedInput,
        ))
        .id()
}

fn aim_input(
    app: &mut App,
    who: Entity,
    from: DVec3,
    at: DVec3,
    f: impl FnOnce(&mut engine_magic::player::CastInput),
) {
    let mut input = app
        .world
        .get_mut::<engine_magic::player::CastInput>(who)
        .unwrap();
    input.ray = Some((from, (at - from).as_vec3().normalize()));
    f(&mut input);
}

#[test]
fn the_spell_bar_charges_on_hold_and_pays_only_on_release() {
    use engine_magic::player::SpellBar;
    let mut app = app();
    let me = weaver(&mut app, DVec3::ZERO);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -14.0));
    run(&mut app, 0.1);
    let eye = DVec3::new(0.5, 2.0, 3.0);
    let target = DVec3::new(0.0, 1.0, -14.0);
    let full = app.world.get::<MagicCaster>(me).unwrap().mana;
    // Hold the fireball (slot 1): nothing is paid or cast while it charges.
    aim_input(&mut app, me, eye, target, |i| i.press(0));
    run(&mut app, 1.3);
    assert_eq!(app.world.get::<MagicCaster>(me).unwrap().mana, full);
    let bar = app.world.get::<SpellBar>(me).unwrap();
    assert!(bar.held.is_some(), "still held");
    assert_eq!(bar.cooldown[0], 0.0);
    // The camera's sweep finds the dummy under the crosshair.
    let p = bar.aim_point.expect("aimed");
    assert!(p.z < -12.0 && p.z > -14.5, "{p:?}");
    // Let go: it flies at full charge, costs mana and the slot cools down.
    aim_input(&mut app, me, eye, target, |i| i.release(0));
    run(&mut app, 2.0);
    let c = app.world.get::<MagicCaster>(me).unwrap();
    assert!(c.mana < full, "paid on release");
    let bar = app.world.get::<SpellBar>(me).unwrap();
    assert!(bar.held.is_none());
    assert!(bar.cooldown_total[0] > 0.3, "{:?}", bar.cooldown_total);
    let hits = hits_on(&app, d);
    assert!(
        !hits.is_empty(),
        "the charged fireball lands where the crosshair was"
    );
    // A slot pressed while cooling down flashes and casts nothing.
    let spent = app.world.get::<MagicCaster>(me).unwrap().mana;
    let mut bar = app.world.get_mut::<SpellBar>(me).unwrap();
    bar.cooldown[3] = 5.0;
    aim_input(&mut app, me, eye, target, |i| i.press(3));
    run(&mut app, 0.05);
    assert!(
        app.world.get::<SpellBar>(me).unwrap().flash[3] > 0.0,
        "it flashes"
    );
    aim_input(&mut app, me, eye, target, |i| i.release(3));
    run(&mut app, 0.5);
    assert_eq!(app.world.get::<MagicCaster>(me).unwrap().mana, spent);
}

#[test]
fn a_channelled_beam_lasts_while_the_key_is_held() {
    use engine_magic::runtime::BeamCast;
    let mut app = app();
    let me = weaver(&mut app, DVec3::ZERO);
    dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    run(&mut app, 0.1);
    let eye = DVec3::new(0.5, 2.0, 3.0);
    let target = DVec3::new(0.0, 1.0, -10.0);
    let beams = |app: &mut App| app.world.query::<&BeamCast>().iter(&app.world).count();
    // Slot 6 is the flame jet.
    aim_input(&mut app, me, eye, target, |i| i.press(5));
    run(&mut app, 1.0);
    assert_eq!(beams(&mut app), 1, "the jet is out");
    let natural = app
        .world
        .query::<&BeamCast>()
        .iter(&app.world)
        .next()
        .unwrap()
        .duration;
    run(&mut app, natural + 1.5);
    assert_eq!(beams(&mut app), 1, "held past its own {natural} s");
    let before = app.world.get::<MagicCaster>(me).unwrap().mana;
    run(&mut app, 0.5);
    assert!(
        app.world.get::<MagicCaster>(me).unwrap().mana < before,
        "a channel drains mana"
    );
    // Its look is drawn again while held, so it doesn't vanish at its own duration.
    let drawn = app
        .world
        .resource::<Seen>()
        .cues
        .iter()
        .filter(|c| matches!(c, SpellCue::Beam { .. }))
        .count();
    assert!(drawn >= 3, "beam drawn {drawn} times");
    aim_input(&mut app, me, eye, target, |i| i.release(5));
    run(&mut app, 0.6);
    assert_eq!(beams(&mut app), 0, "let go, it ends");
}

#[test]
fn a_press_while_the_hands_are_busy_waits_for_them() {
    use engine_magic::player::SpellBar;
    let mut app = app();
    let me = weaver(&mut app, DVec3::ZERO);
    dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    run(&mut app, 0.1);
    let eye = DVec3::new(0.5, 2.0, 3.0);
    let target = DVec3::new(0.0, 1.0, -10.0);
    // Chain lightning (slot 4), then the stone grip (slot 7) near the end of the lightning's
    // gathering: the grip goes as soon as the lightning leaves.
    aim_input(&mut app, me, eye, target, |i| i.press(3));
    run(&mut app, 0.05);
    aim_input(&mut app, me, eye, target, |i| i.release(3));
    run(&mut app, 0.3);
    aim_input(&mut app, me, eye, target, |i| i.press(6));
    run(&mut app, 0.05);
    aim_input(&mut app, me, eye, target, |i| i.release(6));
    assert!(
        app.world.get::<Casting>(me).is_some(),
        "the lightning gathers"
    );
    let bar = app.world.get::<SpellBar>(me).unwrap();
    assert!(bar.buffered.is_some(), "the grip waits");
    assert_eq!(bar.cooldown_total[6], 0.0);
    run(&mut app, 1.0);
    let bar = app.world.get::<SpellBar>(me).unwrap();
    assert!(
        bar.cooldown_total[6] > 0.0,
        "the grip went after the lightning"
    );
    assert_eq!(bar.flash[6], 0.0, "and never flashed");
}
