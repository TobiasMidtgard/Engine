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
    wards: Vec<wards::WardStruck>,
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
    mut struck: MessageReader<wards::WardStruck>,
) {
    seen.wards.extend(struck.read().copied());
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
fn the_altar_previews_a_crafted_spell_and_the_bar_casts_it() {
    use engine_magic::crafter::{self, Altar, Crafter};
    use engine_magic::player::SpellBar;
    let mut app = app();
    let me = weaver(&mut app, DVec3::new(-1.0, 0.0, 1.0));
    {
        // An Expert everywhere: a budget of 8.
        let mut c = app.world.get_mut::<MagicCaster>(me).unwrap();
        let grimoire = std::mem::take(&mut c.grimoire);
        *c = MagicCaster {
            grimoire,
            mana_regen: 0.0,
            ..MagicCaster::new(60.0, Mastery::Expert)
        };
    }
    let stand = DVec3::new(0.0, 0.0, -9.0);
    let d = dummy(&mut app, stand - DVec3::Y * 40.0);
    let altar = app
        .world
        .spawn((
            Name::new("Altar"),
            Transform::default(),
            Altar {
                dummy: Some(d),
                stand,
            },
            crafter::crystal_caster(Vec3::new(0.0, 1.4, 0.0)),
        ))
        .id();
    run(&mut app, 0.1);
    let full = app.world.get::<MagicCaster>(me).unwrap().mana;
    crafter::request_open(&mut app.world, me, Some(altar));
    run(&mut app, 0.05);
    assert!(app.world.resource::<Crafter>().open);
    let at = app.world.get::<Transform>(d).unwrap().translation;
    assert!(
        at.distance(stand) < 0.01,
        "the dummy stands while the altar is in use: {at:?}"
    );
    for a in [
        "effect:frost",
        "effect:fire",
        "step:modifiers",
        "mod:boomerang",
        "mod:pierce",
        "mod:stacking",
        "mod:safeguard",
        "mod:split",
        "mod:speed",
    ] {
        crafter::press(&mut app.world, a);
    }
    crafter::press(&mut app.world, "mod:seek");
    {
        let c = app.world.resource::<Crafter>();
        assert_eq!(c.design.complexity(), 8);
        assert!(c.note.as_ref().is_some_and(|n| !n.ok), "seek is refused");
    }
    // The crystal casts it at the dummy: the glaives pass through it and come back through it.
    // (The first cast waits out the opening pause; the middle glaive flies its full range
    // before it turns.)
    run(&mut app, crafter::FIRST_PREVIEW + 4.0);
    assert!(app.world.resource::<Crafter>().previews >= 1);
    let hits: Vec<SpellHit> = hits_on(&app, d)
        .into_iter()
        .filter(|h| h.caster == Some(altar))
        .collect();
    assert!(hits.len() >= 2, "out and back: {}", hits.len());
    assert!(
        hits.iter()
            .all(|h| h.spell.element() == Some(Element::Frost))
    );
    assert_eq!(
        app.world.get::<MagicCaster>(me).unwrap().mana,
        full,
        "the preview is free"
    );
    // Learn it, put it on key 3, close: the dummy goes back under the floor.
    crafter::press(&mut app.world, "learn");
    crafter::press(&mut app.world, "key:3");
    let g = app.world.get::<SpellBar>(me).unwrap().slots[2].expect("on key 3");
    assert_eq!(
        app.world.get::<MagicCaster>(me).unwrap().grimoire[g]
            .design
            .name,
        "Frost glaives"
    );
    crafter::press(&mut app.world, "close");
    run(&mut app, 0.05);
    assert!(!app.world.resource::<Crafter>().open);
    let at = app.world.get::<Transform>(d).unwrap().translation;
    assert!(
        at.distance(stand) < 0.01,
        "it stays while the weaver is at the altar"
    );
    // Key 3 casts it.
    let eye = DVec3::new(-0.5, 2.0, 4.0);
    aim_input(&mut app, me, eye, DVec3::new(0.0, 1.0, -9.0), |i| {
        i.press(2)
    });
    run(&mut app, 0.05);
    aim_input(&mut app, me, eye, DVec3::new(0.0, 1.0, -9.0), |i| {
        i.release(2)
    });
    run(&mut app, 2.5);
    assert!(
        app.world.get::<MagicCaster>(me).unwrap().mana < full,
        "paid"
    );
    assert!(
        hits_on(&app, d).iter().any(|h| h.caster == Some(me)),
        "the weaver's glaives land"
    );
    // Walking off puts the dummy back under the floor.
    app.world.get_mut::<Transform>(me).unwrap().translation = DVec3::new(20.0, 0.0, 20.0);
    run(&mut app, 0.1);
    assert!(app.world.get::<Transform>(d).unwrap().translation.y < -30.0);
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

fn minions(app: &mut App) -> Vec<(Entity, summon::Minion)> {
    let mut q = app.world.query::<(Entity, &summon::Minion)>();
    q.iter(&app.world).map(|(e, m)| (e, m.clone())).collect()
}

fn rubble(app: &mut App) -> usize {
    let mut q = app.world.query::<&summon::Rubble>();
    q.iter(&app.world).count()
}

#[test]
fn a_summoned_golem_rises_walks_to_a_dummy_and_punches_it() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -4.0));
    run(&mut app, 0.1);
    let golem = SpellDesign::new(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.15 })
        .named("Stone golem");
    cast(&mut app, me, golem, Aim::Point(DVec3::ZERO));
    run(&mut app, 1.0);
    // Out of the floor it comes.
    let found = minions(&mut app);
    assert_eq!(found.len(), 1);
    let (g, m) = found[0].clone();
    assert_eq!(m.kind, summon::MinionKind::Golem);
    assert!(app.world.get::<Transform>(g).unwrap().translation.y < -0.5);
    assert_eq!(m.parts.len(), summon::GOLEM_PARTS);
    run(&mut app, 6.0);
    // It walked to the dummy and hit it, and never its summoner.
    let at = app.world.get::<Transform>(g).unwrap().translation;
    assert!(at.z < -1.0, "it walked: {at}");
    assert!(at.y.abs() < 0.05, "it stands on the floor: {at}");
    let seen = app.world.resource::<Seen>();
    let blows: Vec<_> = seen.damage.iter().filter(|x| x.source == Some(g)).collect();
    assert!(!blows.is_empty(), "no punch landed");
    assert!(blows.iter().all(|b| b.target == d));
    assert!(
        blows
            .iter()
            .all(|b| b.amount > 0.0 && b.impulse.length() > 0.0)
    );
    // Nine seconds (0.15 of a minute) after it came it falls apart into rubble.
    run(&mut app, 3.5);
    assert!(minions(&mut app).is_empty());
    assert!(rubble(&mut app) >= 15, "{}", rubble(&mut app));
}

#[test]
fn a_second_summon_of_a_kind_replaces_the_first() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    run(&mut app, 0.1);
    let golem = SpellDesign::new(&["stone"], CastType::Summon);
    cast(
        &mut app,
        me,
        golem.clone(),
        Aim::Point(DVec3::new(-2.0, 0.0, 0.0)),
    );
    run(&mut app, 1.5);
    let first = minions(&mut app)[0].0;
    cast(&mut app, me, golem, Aim::Point(DVec3::new(2.0, 0.0, 0.0)));
    run(&mut app, 1.5);
    let now = minions(&mut app);
    assert_eq!(now.len(), 1, "an Adept keeps one golem");
    assert_ne!(now[0].0, first);
    assert!(rubble(&mut app) > 0, "the first fell apart");
}

#[test]
fn a_frost_elemental_shoots_foes_and_spares_friends() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    let friend = mage(&mut app, DVec3::new(-3.0, 0.0, 5.0), Mastery::Adept);
    app.world.entity_mut(friend).insert(Body(0.5));
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -6.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Summon),
        Aim::Point(DVec3::new(2.0, 0.0, 4.0)),
    );
    run(&mut app, 6.0);
    let found = minions(&mut app);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.kind, summon::MinionKind::Elemental);
    let e = found[0].0;
    let at = app.world.get::<Transform>(e).unwrap().translation;
    assert!(at.y > 1.4, "it floats: {at}");
    // It floated out toward the dummy to fight it from about 7 m.
    let gap = (at - DVec3::new(0.0, at.y, -6.0)).length();
    assert!((5.0..9.5).contains(&gap), "{gap} m from the dummy at {at}");
    let shots: Vec<_> = hits_on(&app, d)
        .into_iter()
        .filter(|h| h.caster == Some(e))
        .collect();
    assert!(shots.len() >= 2, "{} shots", shots.len());
    assert!(
        shots
            .iter()
            .all(|h| h.spell.element() == Some(Element::Frost))
    );
    assert!(hits_on(&app, friend).is_empty());
    assert!(hits_on(&app, me).is_empty());
}

#[test]
fn a_wisp_heals_its_wounded_summoner() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    run(&mut app, 0.1);
    app.world.get_mut::<Vitality>(me).unwrap().health = 30.0;
    cast(
        &mut app,
        me,
        SpellDesign::new(&["heal"], CastType::Summon),
        Aim::Point(DVec3::new(1.0, 0.0, 3.0)),
    );
    run(&mut app, 5.0);
    let found = minions(&mut app);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.kind, summon::MinionKind::Wisp);
    let health = app.world.get::<Vitality>(me).unwrap().health;
    assert!(health > 70.0, "{health}");
}

fn tethers(app: &mut App) -> Vec<tether::TetherLink> {
    let mut q = app.world.query::<&runtime::BeamCast>();
    q.iter(&app.world).filter_map(|b| b.tether).collect()
}

#[test]
fn a_tether_ticks_on_its_body_and_snaps_when_it_is_thrown_past_the_break() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Adept);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -8.0));
    let other = dummy(&mut app, DVec3::new(3.0, 0.0, -4.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["storm"], CastType::Tether),
        Aim::Entity(d),
    );
    run(&mut app, 1.8);
    let links = tethers(&mut app);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, d);
    assert!(links[0].strain < 0.3, "slack at 8 m: {}", links[0].strain);
    let ticks = hits_on(&app, d).len();
    assert!(ticks >= 4, "{ticks} ticks");
    // Something walking across the line is hit too.
    app.world.get_mut::<Transform>(other).unwrap().translation = DVec3::new(0.0, 0.0, -4.0);
    run(&mut app, 0.6);
    assert!(!hits_on(&app, other).is_empty(), "the crossing body");
    // Thrown past 1.3 times its range, it snaps and stops.
    app.world.get_mut::<Transform>(d).unwrap().translation = DVec3::new(0.0, 0.0, -30.0);
    run(&mut app, 0.2);
    assert!(tethers(&mut app).is_empty(), "it snapped");
    let after = hits_on(&app, d).len();
    run(&mut app, 1.0);
    assert_eq!(hits_on(&app, d).len(), after);
}

#[test]
fn a_tether_with_nothing_to_hold_fizzles_and_one_held_to_the_end_lands() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Adept);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -6.0));
    run(&mut app, 0.1);
    // Aimed at empty floor off to the side: nothing to link.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Tether),
        Aim::Point(DVec3::new(9.0, 0.0, 2.0)),
    );
    run(&mut app, 1.0);
    assert!(tethers(&mut app).is_empty());
    assert!(hits_on(&app, d).is_empty());
    // On the dummy for its whole time: ticks, then one landing hit.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Tether),
        Aim::Entity(d),
    );
    run(&mut app, 2.5);
    // Its frost builds up on the body as it holds.
    let st = app.world.get::<Statuses>(d).copied().unwrap_or_default();
    assert!(st.frozen || st.chill >= 2, "{st:?}");
    run(&mut app, 3.0);
    assert!(tethers(&mut app).is_empty(), "over after 4 s");
    let hits = hits_on(&app, d);
    assert!(hits.len() >= 12, "{}", hits.len());
}

#[test]
fn the_summon_hall_raises_every_creature_in_its_first_period() {
    let mut app = app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Summon;
        hall.restart = true;
    }
    run(&mut app, 5.6);
    let found = minions(&mut app);
    let count = |k: summon::MinionKind| found.iter().filter(|(_, m)| m.kind == k).count();
    assert_eq!(count(summon::MinionKind::Golem), 2);
    assert_eq!(count(summon::MinionKind::Elemental), 2);
    assert_eq!(count(summon::MinionKind::Wisp), 1, "the first lane's wisp");
    // It flew over to the stone golem's wounded summoner and healed it.
    let mut mages = app
        .world
        .query::<(&demo::art::HallMage, &Vitality, &GlobalTransform)>();
    let (health, at) = mages
        .iter(&app.world)
        .find(|(m, ..)| m.lane == 2)
        .map(|(_, v, g)| (v.health, g.translation()))
        .unwrap();
    assert!(health > 140.0, "healed from 105: {health}");
    let wisp = found
        .iter()
        .find(|(_, m)| m.kind == summon::MinionKind::Wisp)
        .unwrap()
        .0;
    let w = app.world.get::<Transform>(wisp).unwrap().translation;
    assert!((w - at).length() < 4.0, "the wisp at {w}, the mage at {at}");
    // The elementals floated out to the lava golem's dummy, at (3, 0, -5), and fight it there.
    for (e, m) in &found {
        if m.kind != summon::MinionKind::Elemental {
            continue;
        }
        let at = app.world.get::<Transform>(*e).unwrap().translation;
        let gap = (at - DVec3::new(3.0, at.y, -5.0)).length();
        assert!(gap < 8.0, "{gap} m off the dummy, at {at}");
        let t = m
            .target
            .and_then(|t| app.world.get::<Name>(t))
            .map(|n| n.to_string());
        assert_eq!(t.as_deref(), Some("Practice dummy 4"));
    }
}

fn raise() -> SpellDesign {
    SpellDesign::new(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.15 })
        .named("Raise skeleton")
}

fn named(app: &mut App, name: &str) -> Vec<Entity> {
    let mut q = app.world.query::<(Entity, &Name)>();
    q.iter(&app.world)
        .filter(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
        .collect()
}

#[test]
fn skeletons_climb_out_by_the_handful_and_cut_a_dummy() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -4.0));
    run(&mut app, 0.1);
    for x in [-2.0, 0.0, 2.0, 4.0] {
        cast(&mut app, me, raise(), Aim::Point(DVec3::new(x, 0.0, 0.0)));
        run(&mut app, 0.8);
    }
    // An Adept keeps three (Diablo II's skill-scaled count): the first crumbled into bones
    // when the fourth climbed out.
    let found = minions(&mut app);
    assert_eq!(
        found.len(),
        3,
        "{:?}",
        found.iter().map(|f| f.1.kind).collect::<Vec<_>>()
    );
    for (_, m) in &found {
        assert_eq!(m.kind, summon::MinionKind::Skeleton);
        assert_eq!(m.parts.len(), undead::SKELETON_PARTS);
    }
    assert!(rubble(&mut app) >= 20, "bones: {}", rubble(&mut app));
    run(&mut app, 4.0);
    // They ran to the dummy and cut it, never their raiser.
    let ids: Vec<Entity> = found.iter().map(|f| f.0).collect();
    let seen = app.world.resource::<Seen>();
    let blows: Vec<_> = seen
        .damage
        .iter()
        .filter(|x| x.source.is_some_and(|s| ids.contains(&s)))
        .collect();
    assert!(blows.len() >= 3, "{} cuts", blows.len());
    assert!(blows.iter().all(|b| b.target == d && b.amount > 0.0));
    // Around it, not stacked on one spot.
    let at: Vec<DVec3> = ids
        .iter()
        .map(|e| app.world.get::<Transform>(*e).unwrap().translation)
        .collect();
    for (i, a) in at.iter().enumerate() {
        assert!(
            a.distance(DVec3::new(0.0, 0.0, -4.0)) < 2.2,
            "near the dummy: {a}"
        );
        assert!(a.y.abs() < 0.05, "on the floor: {a}");
        for b in &at[i + 1..] {
            assert!(a.distance(*b) > 0.4, "{a} and {b} overlap");
        }
    }
}

#[test]
fn a_skeleton_walks_through_a_fallen_rock_not_onto_it() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    dummy(&mut app, DVec3::new(0.0, 0.0, -6.0));
    // A fallen golem's rock lies across the way, its top 1.6 m up.
    app.world.spawn((
        Name::new("Rock"),
        Transform::from_translation(DVec3::new(0.0, 0.0, -1.5)),
        Body(0.6),
        summon::Rubble {
            age: 0.0,
            life: 60.0,
        },
    ));
    run(&mut app, 0.1);
    cast(&mut app, me, raise(), Aim::Point(DVec3::new(0.0, 0.0, 1.5)));
    let mut top = 0.0f64;
    let mut nearest = f64::MAX;
    for _ in 0..50 {
        run(&mut app, 0.1);
        for (_, m) in minions(&mut app) {
            top = top.max(m.pos.y);
            nearest = nearest.min(m.pos.distance(DVec3::new(0.0, 0.0, -6.0)));
        }
    }
    assert!(nearest < 2.0, "it reached the dummy: {nearest}");
    assert!(top < 0.05, "it climbed onto the rock: {top}");
}

#[test]
fn a_raise_on_a_fallen_body_stands_it_up_to_fight_and_it_falls_to_ash() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    // A fire mage of another side, fallen, and a dummy beyond.
    let body = app
        .world
        .spawn((
            Name::new("Fallen"),
            Transform::from_xyz(1.0, 0.0, 0.0),
            MagicCaster {
                learns: false,
                side: 2,
                grimoire: vec![KnownSpell {
                    design: SpellDesign::new(&["fire"], CastType::Bolt),
                    growth: None,
                }],
                ..MagicCaster::new(30.0, Mastery::Adept)
            },
            Vitality {
                health: 0.0,
                ..Vitality::new(200.0)
            },
            engine_world::animation::Ragdoll { weight: 1.0 },
        ))
        .id();
    let d = dummy(&mut app, DVec3::new(0.5, 0.0, -8.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        raise().named("Raise the fallen"),
        Aim::Point(DVec3::new(0.0, 0.0, 0.5)),
    );
    run(&mut app, 1.0);
    // It stands for its raiser at 60% health, and no skeleton came instead.
    let v = *app.world.get::<Vitality>(body).unwrap();
    assert!((v.health - 120.0).abs() < 1.0, "{}", v.health);
    let side = app.world.get::<MagicCaster>(me).unwrap().side;
    assert_eq!(app.world.get::<MagicCaster>(body).unwrap().side, side);
    assert!(app.world.get::<undead::Thrall>(body).is_some());
    assert_eq!(
        app.world
            .get::<engine_world::animation::Ragdoll>(body)
            .map(|r| r.weight),
        Some(0.0)
    );
    assert!(minions(&mut app).is_empty());
    run(&mut app, 4.0);
    // It cast its own fire bolt at the dummy.
    let shots: Vec<_> = hits_on(&app, d)
        .into_iter()
        .filter(|h| h.caster == Some(body))
        .collect();
    assert!(!shots.is_empty(), "the thrall never cast");
    assert!(shots[0].spell.element() == Some(Element::Fire));
    assert!(hits_on(&app, me).is_empty(), "it spared its raiser");
    // Killed again, it collapses into ash.
    app.world.get_mut::<Vitality>(body).unwrap().health = 0.0;
    run(&mut app, 0.2);
    assert!(app.world.get_entity(body).is_err(), "the body is gone");
    assert_eq!(named(&mut app, "Ash heap").len(), 1);
}

#[test]
fn creatures_of_two_sides_fight_each_other() {
    let mut app = app();
    let a = mage(&mut app, DVec3::new(-6.0, 0.0, 8.0), Mastery::Adept);
    let b = mage(&mut app, DVec3::new(6.0, 0.0, 8.0), Mastery::Master);
    app.world.get_mut::<MagicCaster>(a).unwrap().side = 1;
    app.world.get_mut::<MagicCaster>(b).unwrap().side = 2;
    run(&mut app, 0.1);
    let golem = SpellDesign::new(&["stone"], CastType::Summon).named("Stone golem");
    cast(&mut app, a, golem, Aim::Point(DVec3::new(-1.5, 0.0, 0.0)));
    for x in [1.5, 2.5] {
        cast(&mut app, b, raise(), Aim::Point(DVec3::new(x, 0.0, 0.5)));
        run(&mut app, 0.7);
    }
    run(&mut app, 6.0);
    let found = minions(&mut app);
    let kind = |e: Entity| found.iter().find(|f| f.0 == e).map(|f| f.1.kind);
    let seen = app.world.resource::<Seen>();
    let golem_blows = seen
        .damage
        .iter()
        .filter(|x| x.source.and_then(kind) == Some(summon::MinionKind::Golem))
        .count();
    let cuts_on_golem = seen
        .damage
        .iter()
        .filter(|x| {
            x.source.and_then(kind) == Some(summon::MinionKind::Skeleton)
                && kind(x.target) == Some(summon::MinionKind::Golem)
        })
        .count();
    assert!(golem_blows > 0, "the golem never struck");
    assert!(cuts_on_golem > 0, "no skeleton cut the golem");
    // The golem only ever struck the dead, which were all raised beside it.
    assert!(seen.damage.iter().all(|x| x.target != a && x.target != b));
}

#[test]
fn the_dead_hall_raises_two_armies_that_meet() {
    let mut app = app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Dead;
        hall.restart = true;
    }
    run(&mut app, 3.0);
    let found = minions(&mut app);
    let count = |k: summon::MinionKind| found.iter().filter(|(_, m)| m.kind == k).count();
    assert_eq!(count(summon::MinionKind::Skeleton), 6, "lanes 3 and 4");
    assert_eq!(count(summon::MinionKind::SkeletalMage), 1, "lane 5");
    assert_eq!(count(summon::MinionKind::Golem), 1, "lane 0");
    assert_eq!(count(summon::MinionKind::Elemental), 1, "lane 1");
    // The fallen student stands for the necromancers.
    let fallen = named(&mut app, "Fallen student");
    assert_eq!(fallen.len(), 1);
    assert!(app.world.get::<undead::Thrall>(fallen[0]).is_some());
    assert_eq!(
        app.world.get::<MagicCaster>(fallen[0]).unwrap().side,
        demo::NECROMANCERS
    );
    // Bodies for the stand-in physics (the engine's are their colliders).
    for (e, _) in &found {
        app.world.entity_mut(*e).insert(Body(0.9));
    }
    app.world.entity_mut(fallen[0]).insert(Body(0.9));
    run(&mut app, 5.0);
    // The armies met: the dead struck the living's creatures, and were struck by them.
    let living: Vec<Entity> = found
        .iter()
        .filter(|f| f.1.cast.side != demo::NECROMANCERS)
        .map(|f| f.0)
        .collect();
    let dead: Vec<Entity> = found
        .iter()
        .filter(|f| f.1.cast.side == demo::NECROMANCERS)
        .map(|f| f.0)
        .chain(fallen.iter().copied())
        .collect();
    let mages: Vec<Entity> = {
        let mut q = app.world.query::<(Entity, &demo::art::HallMage)>();
        q.iter(&app.world)
            .filter(|(_, m)| m.lane == 2)
            .map(|(e, _)| e)
            .collect()
    };
    let seen = app.world.resource::<Seen>();
    let on = |from: &[Entity], to: &[Entity]| {
        seen.damage
            .iter()
            .filter(|x| x.source.is_some_and(|s| from.contains(&s)) && to.contains(&x.target))
            .count()
    };
    let struck_by_dead = on(&dead, &living);
    assert!(
        struck_by_dead > 0,
        "the dead never reached the living's creatures"
    );
    assert!(
        on(&living, &dead) > 0,
        "the living's creatures never struck the dead"
    );
    let bolts = seen
        .hits
        .iter()
        .filter(|h| h.caster == Some(mages[0]) && h.target.is_some_and(|t| dead.contains(&t)))
        .count();
    assert!(bolts > 0, "lane 2's fire bolts never found the dead");
}

fn barriers(app: &mut App) -> Vec<(Entity, wards::Barrier)> {
    let mut q = app.world.query::<(Entity, &wards::Barrier)>();
    let mut v: Vec<_> = q.iter(&app.world).map(|(e, b)| (e, b.clone())).collect();
    v.sort_by_key(|(e, _)| *e);
    v
}

fn ward_of(app: &App, e: Entity) -> (f32, f32) {
    app.world
        .get::<status::Afflictions>(e)
        .map_or((0.0, 0.0), |a| (a.ward, a.ward_lock))
}

fn shells(app: &mut App) -> usize {
    let mut q = app.world.query::<&wards::WardShell>();
    q.iter(&app.world).count()
}

#[test]
fn a_frost_shell_soaks_frost_breaks_under_fire_and_locks_out() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 0.0), Mastery::Expert);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, 14.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 1;
    app.world.entity_mut(me).insert(Body(0.5));
    app.world.entity_mut(foe).insert(Body(0.5));
    cast(
        &mut app,
        me,
        SpellDesign::new(&["ward", "frost"], CastType::OnSelf),
        Aim::Direction(Vec3::Y),
    );
    run(&mut app, 1.0);
    let (pool, _) = ward_of(&app, me);
    assert!(pool > 0.0, "the shell is up");
    assert_eq!(shells(&mut app), 1);
    let health = app.world.get::<Vitality>(me).unwrap().health;
    // Frost on a frost ward: soaked, no chill, no harm.
    cast(
        &mut app,
        foe,
        SpellDesign::new(&["frost"], CastType::Bolt),
        Aim::Entity(me),
    );
    run(&mut app, 1.2);
    assert_eq!(ward_of(&app, me).0, pool, "frost soaked");
    assert_eq!(app.world.get::<Vitality>(me).unwrap().health, health);
    let aff = app.world.get::<status::Afflictions>(me).unwrap();
    assert_eq!(aff.chill, 0.0, "the ward took the frost");
    let seen = &app.world.resource::<Seen>().wards;
    assert!(
        seen.iter().any(|w| w.ward == me && w.taken == 0.0),
        "the soak still flares"
    );
    // Fire costs it double. A deep pool first, so one bolt's cost can be read...
    let fire_bolt = |app: &mut App| {
        cast(
            app,
            foe,
            SpellDesign::new(&["fire"], CastType::Bolt),
            Aim::Entity(me),
        );
        run(app, 1.2);
    };
    let deepen = |app: &mut App, pool: f32| {
        let mut aff = app.world.get_mut::<status::Afflictions>(me).unwrap();
        aff.ward = pool;
        aff.ward_full = pool;
        aff.ward_secs = wards::SHELL_SECS;
    };
    deepen(&mut app, 1.0e4);
    fire_bolt(&mut app);
    let fire_cost = 1.0e4 - ward_of(&app, me).0;
    assert!(fire_cost > 0.0);
    // ...then a thin one: the next bolt breaks it.
    deepen(&mut app, 1.0);
    fire_bolt(&mut app);
    let (left, lock) = ward_of(&app, me);
    assert_eq!(left, 0.0, "fire broke the frost ward");
    assert!(lock > 0.0, "a broken ward locks out");
    let seen = &app.world.resource::<Seen>().wards;
    assert!(seen.iter().any(|w| w.ward == me && w.broke));
    run(&mut app, wards::FADE_SECS + 0.2);
    assert_eq!(shells(&mut app), 0, "the shell shattered");
    // Raised again at once: the lock holds it off.
    cast(
        &mut app,
        me,
        SpellDesign::new(&["ward", "frost"], CastType::OnSelf),
        Aim::Direction(Vec3::Y),
    );
    run(&mut app, 0.8);
    assert_eq!(ward_of(&app, me).0, 0.0);
    // A plain ward takes the same bolt at its face value: half the frost ward's cost.
    run(&mut app, wards::LOCK_SECS);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["ward"], CastType::OnSelf),
        Aim::Direction(Vec3::Y),
    );
    run(&mut app, 0.8);
    assert!(ward_of(&app, me).0 > 0.0, "up again once the lock ran out");
    deepen(&mut app, 1.0e4);
    fire_bolt(&mut app);
    let plain_cost = 1.0e4 - ward_of(&app, me).0;
    assert!(
        (fire_cost / plain_cost - 2.0).abs() < 0.05,
        "{fire_cost} vs {plain_cost}"
    );
}

#[test]
fn a_dome_stops_enemy_bolts_lets_friends_through_and_shatters() {
    let mut app = app();
    let warden = mage(&mut app, DVec3::new(0.0, 0.0, 0.0), Mastery::Expert);
    let friend = mage(&mut app, DVec3::new(3.0, 0.0, 16.0), Mastery::Expert);
    let foe = mage(&mut app, DVec3::new(-3.0, 0.0, 16.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 1;
    for e in [warden, friend, foe] {
        app.world.entity_mut(e).insert(Body(0.5));
    }
    let inside = dummy(&mut app, DVec3::new(1.5, 0.0, -1.0));
    cast(
        &mut app,
        warden,
        SpellDesign::new(&["ward"], CastType::Area),
        Aim::Point(DVec3::ZERO),
    );
    run(&mut app, 0.6);
    let domes = barriers(&mut app);
    assert_eq!(domes.len(), 1);
    let (dome, b) = domes[0].clone();
    assert!(matches!(b.kind, wards::BarrierKind::Dome { .. }));
    // The foe's bolt bursts on the dome's face; the warden is untouched.
    cast(
        &mut app,
        foe,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(warden),
    );
    run(&mut app, 1.2);
    assert!(hits_on(&app, warden).is_empty(), "the dome took it");
    let w = app.world.get::<wards::Barrier>(dome).unwrap();
    assert!(w.pool < w.full);
    // The friend's bolt flies through to the dummy inside.
    cast(
        &mut app,
        friend,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(inside),
    );
    run(&mut app, 1.2);
    assert_eq!(hits_on(&app, inside).len(), 1, "friends' spells pass");
    // Bolts until it breaks; then they reach the warden.
    let mut volleys = 0;
    while app.world.get::<wards::Barrier>(dome).is_some() && volleys < 40 {
        cast(
            &mut app,
            foe,
            SpellDesign::new(&["fire"], CastType::Bolt),
            Aim::Entity(warden),
        );
        run(&mut app, 0.6);
        volleys += 1;
    }
    assert!(app.world.get::<wards::Barrier>(dome).is_none(), "it broke");
    assert!(
        app.world
            .resource::<Seen>()
            .wards
            .iter()
            .any(|w| w.ward == dome && w.broke)
    );
    cast(
        &mut app,
        foe,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(warden),
    );
    run(&mut app, 1.2);
    assert!(!hits_on(&app, warden).is_empty(), "nothing in the way now");
}

#[test]
fn a_ward_wall_stands_in_panels_that_break_one_at_a_time() {
    let mut app = app();
    let warden = mage(&mut app, DVec3::new(0.0, 0.0, 0.0), Mastery::Expert);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, 16.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 1;
    for e in [warden, foe] {
        app.world.entity_mut(e).insert(Body(0.5));
    }
    cast(
        &mut app,
        warden,
        SpellDesign::new(&["ward"], CastType::Wall),
        Aim::Point(DVec3::new(0.0, 0.0, 6.0)),
    );
    run(&mut app, 0.6);
    let panels = barriers(&mut app);
    assert!(panels.len() >= 4, "{}", panels.len());
    for _ in 0..30 {
        if barriers(&mut app).len() < panels.len() {
            break;
        }
        cast(
            &mut app,
            foe,
            SpellDesign::new(&["fire"], CastType::Bolt),
            Aim::Entity(warden),
        );
        run(&mut app, 0.6);
    }
    let left = barriers(&mut app);
    assert_eq!(left.len(), panels.len() - 1, "one panel broke");
    assert!(
        left.iter().all(|(_, b)| b.pool == b.full),
        "the rest are whole"
    );
}

#[test]
fn a_storm_ward_shocks_whoever_strikes_it_from_close() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 0.0), Mastery::Expert);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, 1.8), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 1;
    for e in [me, foe] {
        app.world.entity_mut(e).insert(Body(0.5));
    }
    cast(
        &mut app,
        me,
        SpellDesign::new(&["ward", "storm"], CastType::OnSelf),
        Aim::Direction(Vec3::Y),
    );
    run(&mut app, 1.0);
    assert!(ward_of(&app, me).0 > 0.0);
    cast(
        &mut app,
        foe,
        SpellDesign::new(&["fire"], CastType::Touch),
        Aim::Entity(me),
    );
    run(&mut app, 1.0);
    let back = hits_on(&app, foe);
    assert!(
        back.iter()
            .any(|h| h.caster == Some(me) && h.spell.element() == Some(schools::Element::Storm)),
        "the ward answered with lightning: {back:?}"
    );
}

#[test]
fn the_ward_hall_holds_a_dome_until_it_shatters_then_raises_a_wall() {
    let mut app = app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Ward;
        hall.restart = true;
    }
    run(&mut app, 3.0);
    let up = barriers(&mut app);
    assert_eq!(up.len(), 1, "the warden's dome");
    let (dome, b) = up[0].clone();
    assert!(b.pool < b.full, "the barrage is landing on it");
    let mut broke_at = None;
    let mut t = 3.0;
    while t < demo::WARD_WALL_AT {
        run(&mut app, 0.25);
        t += 0.25;
        if broke_at.is_none() && app.world.get::<wards::Barrier>(dome).is_none() {
            broke_at = Some(t);
        }
    }
    let broke_at = broke_at.expect("the dome broke before the wall goes up");
    assert!(broke_at > 4.0, "it held a while: {broke_at}");
    run(&mut app, 1.0);
    let panels = barriers(&mut app);
    assert_eq!(panels.len(), 3, "the ward wall's three panels");
    // All of them within the dais' depth (z 7.5 to 12.5), across the line of fire. (The
    // stand-in physics has no dais; in the hall they stand on its top.)
    for (_, b) in &panels {
        let wards::BarrierKind::Panel { at, .. } = b.kind else {
            panic!("a panel");
        };
        assert!((7.5..12.5).contains(&at.z), "{at}");
    }
    // Both shells went up.
    let shelled = {
        let mut q = app
            .world
            .query::<(&demo::art::HallMage, &status::Afflictions)>();
        q.iter(&app.world)
            .filter(|(m, a)| m.lane != 1 && a.ward_full > 0.0)
            .count()
    };
    assert_eq!(shelled, 2, "frost and storm shells");
}

// ------------------------------------------------------------------------------- space

/// The portal ends now: (entity, gate, transform).
fn ends(app: &mut App) -> Vec<(Entity, space::Gate, Transform)> {
    let mut q = app.world.query::<(Entity, &space::Gate, &Transform)>();
    let mut v: Vec<_> = q
        .iter(&app.world)
        .map(|(e, g, t)| (e, g.clone(), *t))
        .collect();
    v.sort_by_key(|(_, g, _)| g.end);
    v
}

fn portal(secs: f32) -> SpellDesign {
    SpellDesign::new(&["portal"], CastType::Area).with(Modifier::Duration {
        factor: secs / 20.0,
    })
}

/// A mage at the origin opens a pair: its near end at `a`, its far end at `b` (on the ground).
fn open_pair(app: &mut App, me: Entity, a: DVec3, b: DVec3) -> (Entity, Entity) {
    cast(app, me, portal(20.0), Aim::Point(a));
    run(app, 1.5);
    cast(app, me, portal(20.0), Aim::Point(b));
    run(app, 1.5);
    let e = ends(app);
    assert_eq!(e.len(), 2, "two ends");
    assert!(
        e.iter().all(|(_, g, _)| g.open() && g.exit.is_some()),
        "linked and open"
    );
    (e[0].0, e[1].0)
}

/// Bodies the test moves by their velocity (the stand-in physics moves nothing).
fn drift(time: Res<Time>, mut q: Query<(&mut Transform, &engine_world::physics::Velocity)>) {
    for (mut t, v) in &mut q {
        t.translation += (v.linear * time.delta_secs()).as_dvec3();
    }
}

#[test]
fn a_portal_opens_one_end_then_the_other_and_they_face_on_through() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        portal(20.0),
        Aim::Point(DVec3::new(0.0, 0.0, -5.0)),
    );
    run(&mut app, 1.5);
    let e = ends(&mut app);
    assert_eq!(e.len(), 1);
    let (near, g, t) = e[0].clone();
    assert!(g.exit.is_none(), "alone until its other end opens");
    // Standing on the ground, facing the caster.
    assert!((t.translation.y - 1.2).abs() < 0.05, "{}", t.translation);
    assert!((t.rotation * Vec3::Z).dot(Vec3::Z) > 0.99);
    cast(
        &mut app,
        me,
        portal(20.0),
        Aim::Point(DVec3::new(10.0, 0.0, -5.0)),
    );
    run(&mut app, 1.5);
    let e = ends(&mut app);
    assert_eq!(e.len(), 2);
    let (far, g2, t2) = e[1].clone();
    assert_eq!(g2.exit, Some(near));
    assert_eq!(e[0].1.exit, Some(far));
    // The far end faces away from the caster.
    let away = Vec3::new(10.0, 0.0, -5.0).normalize();
    assert!((t2.rotation * Vec3::Z).dot(away) > 0.99);
    // The renderer draws the view through each.
    let p = app
        .world
        .get::<engine_render::Portal>(near)
        .expect("a view");
    assert_eq!(p.exit, Some(far));
    let api = call_api(&mut app.world, "magic.portals", json!({})).unwrap();
    assert_eq!(api["ends"].as_array().unwrap().len(), 2, "{api}");
}

#[test]
fn a_bolt_into_one_end_hits_a_dummy_beyond_the_other() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    open_pair(
        &mut app,
        me,
        DVec3::new(0.0, 0.0, -6.0),
        DVec3::new(10.0, 0.0, -5.0),
    );
    // Beyond the far end, on its axis; nothing on the way to it but the door.
    let away = DVec3::new(10.0, 0.0, -5.0).normalize();
    let d = dummy(&mut app, DVec3::new(10.0, 0.0, -5.0) + away * 6.0);
    // And one straight behind the near end, which must not be hit.
    let behind = dummy(&mut app, DVec3::new(0.0, 0.0, -12.0));
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Point(DVec3::new(0.0, 1.1, -6.0)),
    );
    run(&mut app, 2.0);
    assert!(
        !hits_on(&app, d).is_empty(),
        "the bolt came out of the far end"
    );
    assert!(
        hits_on(&app, behind).is_empty(),
        "nothing goes past the near end"
    );
}

#[test]
fn a_beam_bends_through_a_pair() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    open_pair(
        &mut app,
        me,
        DVec3::new(0.0, 0.0, -6.0),
        DVec3::new(10.0, 0.0, -5.0),
    );
    let away = DVec3::new(10.0, 0.0, -5.0).normalize();
    let d = dummy(&mut app, DVec3::new(10.0, 0.0, -5.0) + away * 5.0);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["storm"], CastType::Beam),
        Aim::Point(DVec3::new(0.0, 1.1, -6.0)),
    );
    run(&mut app, 1.2);
    let beam_end = {
        let mut q = app.world.query::<&runtime::BeamCast>();
        q.iter(&app.world).next().map(|b| b.end).expect("a beam")
    };
    assert!(
        beam_end.distance(DVec3::new(10.0, 1.0, -5.0) + away * 5.0) < 1.0,
        "its end is on the dummy beyond the far end: {beam_end}"
    );
    run(&mut app, 2.0);
    assert!(hits_on(&app, d).len() >= 3, "it ticks on it");
}

#[test]
fn a_shoved_body_comes_out_turned_and_does_not_go_straight_back() {
    let mut app = app();
    app.add_systems(FixedUpdate, drift.in_set(SimSet::Physics));
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    let (_, far) = open_pair(
        &mut app,
        me,
        DVec3::new(0.0, 0.0, -6.0),
        DVec3::new(10.0, 0.0, -5.0),
    );
    let crate_e = app
        .world
        .spawn((
            Name::new("Crate"),
            Transform::from_translation(DVec3::new(0.0, 0.4, -4.0)),
            RigidBody {
                kind: BodyKind::Dynamic,
                ..Default::default()
            },
            engine_world::physics::Velocity {
                linear: Vec3::new(0.0, 0.0, -4.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    run(&mut app, 1.0);
    let away = Vec3::new(10.0, 0.0, -5.0).normalize();
    let t = *app.world.get::<Transform>(crate_e).unwrap();
    let v = *app
        .world
        .get::<engine_world::physics::Velocity>(crate_e)
        .unwrap();
    let far_at = app.world.get::<Transform>(far).unwrap().translation;
    assert!(
        t.translation.distance(far_at) < 4.0,
        "it came out of the far end: {}",
        t.translation
    );
    assert!(
        (v.linear.normalize().dot(away) - 1.0).abs() < 1e-3
            && (v.linear.length() - 4.0).abs() < 1e-3,
        "its velocity turned with the pair: {}",
        v.linear
    );
    assert!(
        (t.rotation * Vec3::NEG_Z).dot(away) > 0.99,
        "and so did its facing"
    );
    run(&mut app, 2.0);
    let p = app.world.get::<Transform>(crate_e).unwrap().translation;
    assert!(
        (p - far_at).dot(away.as_dvec3()) > 6.0,
        "it went on out of the far end: {p}"
    );
}

#[test]
fn an_end_alone_lets_nothing_through() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        portal(20.0),
        Aim::Point(DVec3::new(0.0, 0.0, -6.0)),
    );
    run(&mut app, 1.5);
    let behind = dummy(&mut app, DVec3::new(0.0, 0.0, -12.0));
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Point(DVec3::new(0.0, 1.1, -12.0)),
    );
    run(&mut app, 2.0);
    assert!(hits_on(&app, behind).is_empty(), "the membrane stopped it");
    let seen = app.world.resource::<Seen>();
    assert!(
        seen.hits
            .iter()
            .any(|h| h.target.is_none() && (h.point.z + 6.0).abs() < 0.5),
        "it burst on the membrane"
    );
}

#[test]
fn a_pair_closes_when_its_time_is_up_and_when_a_third_end_opens() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    let (a, b) = open_pair(
        &mut app,
        me,
        DVec3::new(0.0, 0.0, -6.0),
        DVec3::new(10.0, 0.0, -5.0),
    );
    // A third end: the old pair closes, the new end waits alone.
    cast(
        &mut app,
        me,
        portal(20.0),
        Aim::Point(DVec3::new(-6.0, 0.0, -6.0)),
    );
    run(&mut app, 1.5);
    let e = ends(&mut app);
    assert_eq!(e.len(), 1, "only the new end");
    assert!(e[0].0 != a && e[0].0 != b && e[0].1.exit.is_none());
    // A short-lived pair closes on its own.
    let mut app = self::app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        portal(3.0),
        Aim::Point(DVec3::new(0.0, 0.0, -6.0)),
    );
    run(&mut app, 1.0);
    cast(
        &mut app,
        me,
        portal(3.0),
        Aim::Point(DVec3::new(8.0, 0.0, -6.0)),
    );
    run(&mut app, 1.0);
    assert_eq!(ends(&mut app).len(), 2);
    run(&mut app, 3.0);
    assert!(ends(&mut app).is_empty(), "both ends closed");
}

#[test]
fn a_blink_stops_short_of_a_wall_and_steps_onto_a_ledge_it_aims_at() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    // A wall: a big boulder 4.5 m ahead.
    app.world.spawn((
        Name::new("Boulder"),
        Transform::from_translation(DVec3::new(0.0, 0.0, -6.0)),
        Body(1.5),
    ));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["blink"], CastType::OnSelf),
        Aim::Point(DVec3::new(0.0, 0.0, -10.0)),
    );
    run(&mut app, 1.0);
    let p = app.world.get::<Transform>(me).unwrap().translation;
    assert!(p.z < -2.5 && p.z > -4.5, "short of the boulder: {p}");
    assert!(p.y.abs() < 0.05, "on the ground: {p}");

    // A ledge: a low rock whose top is 2.4 m up, aimed at.
    let mut app = self::app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    app.world.spawn((
        Name::new("Ledge"),
        Transform::from_translation(DVec3::new(0.0, -0.2, -6.0)),
        Body(1.6),
    ));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["blink"], CastType::OnSelf),
        Aim::Point(DVec3::new(0.0, 2.4, -6.0)),
    );
    run(&mut app, 0.05);
    let api = call_api(&mut app.world, "magic.portals", json!({})).unwrap();
    run(&mut app, 1.0);
    let p = app.world.get::<Transform>(me).unwrap().translation;
    assert!(
        p.y > 2.0 && (p.z + 6.0).abs() < 0.6,
        "on top of it: {p} ({api})"
    );
}

#[test]
fn a_recall_rewinds_three_seconds_of_its_path_and_gives_back_its_health() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    run(&mut app, 0.5);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["blink"], CastType::OnSelf),
        Aim::Point(DVec3::new(0.0, 0.0, -6.0)),
    );
    run(&mut app, 1.0);
    let there = app.world.get::<Transform>(me).unwrap().translation;
    assert!(there.z < -4.0, "{there}");
    app.world.get_mut::<Vitality>(me).unwrap().health = 40.0;
    run(&mut app, 0.5);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["recall"], CastType::OnSelf),
        Aim::Direction(Vec3::NEG_Z),
    );
    let mut steps = 0;
    while app.world.get::<space::Rewinding>(me).is_none() {
        run(&mut app, 1.0 / 60.0);
        steps += 1;
        assert!(steps < 120, "it rewinds");
    }
    run(&mut app, 0.6);
    let back = app.world.get::<Transform>(me).unwrap().translation;
    assert!(
        back.distance(DVec3::ZERO) < 0.3,
        "back where it was: {back}"
    );
    let v = app.world.get::<Vitality>(me).unwrap();
    assert!(v.health > 99.0, "its health back: {}", v.health);
    assert!(app.world.get::<space::Rewinding>(me).is_none());
}

#[test]
fn nothing_touches_a_body_rewinding() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -12.0));
    app.world.entity_mut(d).insert(space::Rewinding {
        path: vec![
            (DVec3::new(0.0, 0.0, -12.0), Quat::IDENTITY),
            (DVec3::new(0.0, 0.0, -12.0), Quat::IDENTITY),
        ],
        t: 0.0,
        secs: 30.0,
        health: None,
    });
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 2.0);
    assert!(hits_on(&app, d).is_empty());
    assert_eq!(app.world.get::<Vitality>(d).unwrap().health, 500.0);
}

#[test]
fn the_portal_hall_opens_its_pair_and_sends_bolts_through() {
    let mut app = app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Portal;
        hall.restart = true;
    }
    run(&mut app, 0.2);
    // The stand-in physics sees only bodies it is told of: the program's dummies.
    let props: Vec<Entity> = {
        let mut q = app
            .world
            .query_filtered::<Entity, (With<demo::art::HallTarget>, With<demo::art::ProgramProp>)>(
            );
        q.iter(&app.world).collect()
    };
    for e in props {
        app.world.entity_mut(e).insert(Body(0.45));
    }
    // The program starts 1.2 s into its period (the last program's smoke settles first).
    run(&mut app, 3.0);
    let e = ends(&mut app);
    assert_eq!(e.len(), 2, "the warden's pair");
    assert!(e.iter().all(|(_, g, _)| g.exit.is_some()));
    // Where the hall's layout says (the stand-in ground is at 0, the hall floor at 0.2).
    let (near, far) = demo::portal::frames();
    for ((_, _, t), f) in e.iter().zip([near, far]) {
        let flat = |p: DVec3| DVec3::new(p.x, 0.0, p.z);
        assert!(
            flat(t.translation).distance(flat(f.at)) < 0.1,
            "{}",
            t.translation
        );
        assert!((t.rotation.as_dquat() * DVec3::Z).dot(f.rot * DVec3::Z) > 0.999);
    }
    run(&mut app, 4.0);
    // The warden's bolts land on the dummy beyond the far end.
    let [x, z] = demo::portal::exit_spot(demo::portal::WARDEN, demo::portal::DUMMY_M[0].1);
    let target = {
        let mut q = app
            .world
            .query::<(Entity, &Transform, &demo::art::HallTarget)>();
        q.iter(&app.world)
            .filter(|(_, t, _)| {
                DVec3::new(t.translation.x - x, 0.0, t.translation.z - z).length() < 0.3
            })
            .map(|(e, _, _)| e)
            .next()
            .expect("the warden's dummy")
    };
    assert!(!hits_on(&app, target).is_empty(), "bolts through the door");
}

// ------------------------------------------------------------------------------ kinesis

/// Turns on the stand-in's motion: dynamic bodies move by their velocity under gravity and
/// impulses, and stop on the ground.
#[derive(Resource)]
struct Integrate;

fn integrate(
    time: Res<Time>,
    gravity: Res<engine_world::Gravity>,
    mut q: Query<(
        &mut Transform,
        &RigidBody,
        &mut engine_world::physics::Velocity,
    )>,
    mut impulses: MessageReader<ApplyImpulse>,
) {
    let dt = time.delta_secs();
    for imp in impulses.read() {
        if let Ok((_, rb, mut v)) = q.get_mut(imp.entity) {
            v.linear += imp.impulse / rb.mass_kg.unwrap_or(60.0);
        }
    }
    for (mut t, rb, mut v) in &mut q {
        if rb.kind != BodyKind::Dynamic {
            continue;
        }
        v.linear += gravity.at(t.translation) * rb.gravity_scale * dt;
        t.translation += (v.linear * dt).as_dvec3();
        if t.translation.y < 0.0 {
            t.translation.y = 0.0;
            v.linear.y = v.linear.y.max(0.0);
            v.linear *= 1.0 - (8.0 * dt).min(1.0);
        }
    }
}

fn moving_app() -> App {
    let mut app = app();
    app.world.insert_resource(Integrate);
    app.add_systems(
        FixedUpdate,
        integrate
            .run_if(resource_exists::<Integrate>)
            .in_set(SimSet::Physics),
    );
    app
}

fn loose_crate(app: &mut App, at: DVec3, kg: f32) -> Entity {
    app.world
        .spawn((
            Name::new("Crate"),
            Transform::from_translation(at),
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some(kg),
                ..Default::default()
            },
            Collider {
                shape: ColliderShape::Cuboid {
                    half_extents: Vec3::splat(0.375),
                },
                density: 500.0,
                ..Default::default()
            },
            engine_world::physics::Velocity::default(),
        ))
        .id()
}

fn kinesis(app: &mut App) -> serde_json::Value {
    call_api(&mut app.world, "magic.kinesis", json!({})).unwrap()
}

fn grip() -> SpellDesign {
    SpellDesign::new(&["grip"], CastType::Bolt)
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

#[test]
fn a_grip_lifts_a_crate_to_the_hand_and_throws_it_at_a_dummy() {
    let mut app = moving_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let crate_e = loose_crate(&mut app, DVec3::new(0.3, 0.375, -5.0), 16.0);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -14.0));
    run(&mut app, 0.1);
    cast(&mut app, me, grip(), Aim::Entity(d));
    run(&mut app, 1.4);
    let k = kinesis(&mut app);
    let held = &k["grips"][0]["held"][0];
    assert_eq!(held["id"], json!(crate_e.to_bits()), "{k}");
    assert_eq!(held["kind"], json!("body"));
    // It floats before the hand: up off the floor, a couple of metres out.
    let p = pos(&app, crate_e);
    assert!(p.y > 1.0 && p.z < -1.0 && p.z > -3.5, "held at {p:?}");
    assert!(app.world.get::<kinesis::Gripped>(crate_e).is_some());
    // The time is up: it flies at the dummy and hits it by its weight and speed.
    let flat = |app: &App| {
        let v = app
            .world
            .get::<engine_world::physics::Velocity>(crate_e)
            .unwrap()
            .linear;
        Vec3::new(v.x, 0.0, v.z).length()
    };
    let mut t = 0.0;
    let (hit, before) = loop {
        let before = flat(&app);
        run(&mut app, 0.05);
        t += 0.05;
        if let Some(h) = app
            .world
            .resource::<Seen>()
            .damage
            .iter()
            .find(|h| h.target == d && h.source == Some(me))
        {
            break (h.clone(), before);
        }
        assert!(t < 2.0, "the crate never hit the dummy");
    };
    assert!(hit.amount > 20.0 && hit.impulse.z < 0.0, "{hit:?}");
    // The blow took most of its speed: it drops at the dummy rather than flying on.
    let after = flat(&app);
    assert!(
        before > 20.0 && after < 0.5 * before,
        "{before} m/s, then {after} m/s"
    );
    run(&mut app, 2.0);
    assert!(app.world.get::<kinesis::Gripped>(crate_e).is_none());
    assert_eq!(
        app.world.get::<RigidBody>(crate_e).unwrap().gravity_scale,
        1.0,
        "gravity back"
    );
}

#[test]
fn a_crate_too_heavy_to_lift_drags_along_the_floor() {
    let mut app = moving_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Apprentice);
    app.world.get_mut::<MagicCaster>(me).unwrap().level = 3.0;
    let crate_e = loose_crate(&mut app, DVec3::new(0.0, 0.0, -6.0), 80.0);
    run(&mut app, 0.1);
    cast(&mut app, me, grip(), Aim::Entity(crate_e));
    run(&mut app, 1.6);
    let k = kinesis(&mut app);
    assert_eq!(k["grips"][0]["held"][0]["too_heavy"], json!(true), "{k}");
    let p = pos(&app, crate_e);
    assert!(p.y < 0.2, "never lifted: {p:?}");
    assert!(p.z > -5.5, "dragged toward the caster: {p:?}");
}

#[test]
fn a_grip_catches_a_bolt_and_throws_it_back() {
    let mut app = moving_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, -24.0), Mastery::Ascended1);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 2;
    app.world.entity_mut(foe).insert(Body(0.5));
    run(&mut app, 0.1);
    cast(
        &mut app,
        foe,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(me),
    );
    // The grip comes up while the bolt is on its way.
    run(&mut app, 0.45);
    cast(&mut app, me, grip(), Aim::Entity(foe));
    run(&mut app, 1.0);
    let k = kinesis(&mut app);
    assert_eq!(k["grips"][0]["held"][0]["kind"], json!("missile"), "{k}");
    assert!(
        app.world
            .resource::<Seen>()
            .damage
            .iter()
            .all(|h| h.target != me),
        "caught before it struck"
    );
    // Held, then thrown back: it is the gripper's now and burns its first caster.
    run(&mut app, 3.0);
    let back = hits_on(&app, foe);
    assert!(!back.is_empty(), "the bolt went back");
    let mut q = app.world.query::<&runtime::Missile>();
    assert!(
        q.iter(&app.world)
            .all(|m| m.cast.caster == Some(me) || m.spent.is_some())
    );
}

#[test]
fn with_nothing_in_reach_a_grip_tears_a_stone_out_of_the_ground() {
    let mut app = moving_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -20.0));
    run(&mut app, 0.1);
    cast(&mut app, me, grip(), Aim::Entity(d));
    run(&mut app, 0.8);
    let mut q = app.world.query_filtered::<Entity, With<kinesis::Torn>>();
    let stone = q.iter(&app.world).next().expect("a torn stone");
    assert!(
        app.world
            .resource::<Seen>()
            .cues
            .iter()
            .any(|c| matches!(c, SpellCue::Mark { .. })),
        "it leaves a scar"
    );
    run(&mut app, 0.7);
    let p = pos(&app, stone);
    assert!(p.y > 0.9, "it rises to the hand: {p:?}");
    run(&mut app, 2.5);
    assert!(
        app.world
            .resource::<Seen>()
            .damage
            .iter()
            .any(|h| h.target == d && h.source == Some(me)),
        "and flies at the dummy"
    );
}

#[test]
fn a_ring_of_torn_stones_stops_a_bolt_and_flies_at_the_aim() {
    let mut app = moving_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, -14.0), Mastery::Ascended1);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 2;
    run(&mut app, 0.1);
    let ring =
        SpellDesign::new(&["grip"], CastType::Orbit).with(Modifier::Duration { factor: 0.5 });
    cast(&mut app, me, ring, Aim::Entity(foe));
    run(&mut app, 1.0);
    let k = kinesis(&mut app);
    assert_eq!(k["grips"][0]["ring"], json!(true));
    let stones = k["grips"][0]["held"].as_array().unwrap().len();
    assert!(stones >= 5, "{k}");
    // Bolts at the ring's middle stop on its stones.
    for _ in 0..3 {
        cast(
            &mut app,
            foe,
            SpellDesign::new(&["fire"], CastType::Bolt),
            Aim::Point(DVec3::new(0.0, 1.2, 0.0)),
        );
        run(&mut app, 0.7);
    }
    assert!(
        app.world
            .resource::<Seen>()
            .damage
            .iter()
            .all(|h| h.target != me),
        "the ring took them"
    );
    // Its time up, it flies at the foe.
    run(&mut app, 4.0);
    assert!(
        app.world
            .resource::<Seen>()
            .damage
            .iter()
            .any(|h| h.target == foe && h.source == Some(me)),
        "stones flung at the aim"
    );
}

#[test]
fn a_grip_lifts_a_creature_helpless_and_flings_it() {
    let mut app = moving_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let wolf = app
        .world
        .spawn((
            Name::new("Wolf"),
            Transform::from_translation(DVec3::new(0.0, 0.0, -6.0)),
            Vitality::new(300.0),
            engine_world::physics::CharacterController::default(),
            MagicCaster {
                side: 3,
                ..MagicCaster::new(5.0, Mastery::Novice)
            },
        ))
        .id();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        grip(),
        Aim::Point(DVec3::new(0.0, 0.0, -16.0)),
    );
    run(&mut app, 1.2);
    let p = pos(&app, wolf);
    assert!(p.y > 0.4, "lifted off its feet: {p:?}");
    assert!(
        app.world.get::<Statuses>(wolf).is_some_and(|s| s.rooted),
        "helpless while held"
    );
    run(&mut app, 3.0);
    assert!(pos(&app, wolf).z < -7.0, "flung toward the aim");
    assert!(
        app.world
            .resource::<Seen>()
            .damage
            .iter()
            .any(|h| h.target == wolf && h.kind == engine_world::DamageKind::Fall),
        "it lands hard"
    );
}

#[test]
fn the_spell_bar_holds_a_grip_while_the_key_is_down_and_throws_on_release() {
    let mut app = moving_app();
    let me = weaver(&mut app, DVec3::ZERO);
    let crate_e = loose_crate(&mut app, DVec3::new(0.0, 0.375, -4.0), 16.0);
    run(&mut app, 0.1);
    let eye = DVec3::new(0.5, 2.0, 3.0);
    let at = DVec3::new(0.0, 0.4, -4.0);
    // Slot 7 is telekinesis.
    aim_input(&mut app, me, eye, at, |i| i.press(6));
    run(&mut app, 5.0);
    let k = kinesis(&mut app);
    assert_eq!(
        k["grips"][0]["held"][0]["thrown"],
        json!(false),
        "held past its own time: {k}"
    );
    assert!(pos(&app, crate_e).y > 0.8);
    // Aim off to the side and let go: it flies that way.
    let side = DVec3::new(-12.0, 1.0, -4.0);
    aim_input(&mut app, me, eye, side, |i| i.release(6));
    run(&mut app, 0.6);
    let p = pos(&app, crate_e);
    assert!(p.x < -2.0, "thrown where the crosshair was: {p:?}");
}

/// A warden at the origin and a foe 16 m out along +z, both with bodies.
fn shield_duel(app: &mut App) -> (Entity, Entity) {
    let warden = mage(app, DVec3::new(0.0, 0.0, 0.0), Mastery::Expert);
    let foe = mage(app, DVec3::new(0.0, 0.0, 16.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 1;
    for e in [warden, foe] {
        app.world.entity_mut(e).insert(Body(0.5));
    }
    (warden, foe)
}

fn shield() -> SpellDesign {
    SpellDesign::new(&["ward"], CastType::Cone).named("Shield")
}

fn bolt() -> SpellDesign {
    SpellDesign::new(&["fire"], CastType::Bolt)
}

#[test]
fn a_held_shield_stops_bolts_at_its_face_and_turns_with_its_aim() {
    let mut app = app();
    let (warden, foe) = shield_duel(&mut app);
    cast(&mut app, warden, shield(), Aim::Entity(foe));
    run(&mut app, 0.6);
    let held = barriers(&mut app);
    assert_eq!(held.len(), 1);
    let (e, b) = held[0].clone();
    let wards::BarrierKind::Shield { at, normal, .. } = b.kind else {
        panic!("a ward cast as a cone is a held shield: {:?}", b.kind);
    };
    assert!(normal.z > 0.95, "it faces the foe: {normal}");
    assert!(
        (at.z - wards::SHIELD_AHEAD as f64).abs() < 0.1 && at.y > 1.0,
        "{at}"
    );
    // Past its first moment a bolt only bursts on its face.
    cast(&mut app, foe, bolt(), Aim::Entity(warden));
    run(&mut app, 1.2);
    assert!(hits_on(&app, warden).is_empty(), "the shield took it");
    assert!(hits_on(&app, foe).is_empty(), "and didn't send it back");
    let b = app.world.get::<wards::Barrier>(e).unwrap();
    assert!(b.pool < b.full);
    let seen = &app.world.resource::<Seen>().wards;
    assert!(
        seen.iter()
            .any(|w| w.ward == e && w.taken > 0.0 && !w.perfect)
    );
    // Turned away (aimed behind), it no longer covers the warden from the foe.
    app.world.get_mut::<wards::HeldShield>(e).unwrap().aim =
        Aim::Point(DVec3::new(0.0, 1.3, -20.0));
    run(&mut app, 0.1);
    let b = app.world.get::<wards::Barrier>(e).unwrap();
    let wards::BarrierKind::Shield { normal, .. } = b.kind else {
        unreachable!()
    };
    assert!(normal.z < -0.95, "it turned with the aim: {normal}");
    cast(&mut app, foe, bolt(), Aim::Entity(warden));
    run(&mut app, 1.2);
    assert!(
        !hits_on(&app, warden).is_empty(),
        "from behind it gets through"
    );
}

#[test]
fn a_perfect_block_sends_the_bolt_back_at_whoever_cast_it() {
    let mut app = app();
    let (warden, foe) = shield_duel(&mut app);
    let up = app
        .world
        .get::<MagicCaster>(warden)
        .unwrap()
        .compile(&shield(), 0.0)
        .unwrap()
        .cast_secs;
    assert!(up < 0.25, "a shield comes up fast: {up}");
    cast(&mut app, foe, bolt(), Aim::Entity(warden));
    // Raise it so it stands just before the bolt arrives.
    let mut raised = false;
    for _ in 0..120 {
        app.update();
        let mut q = app.world.query::<(&runtime::Missile, &runtime::SpellPos)>();
        let near = q.iter(&app.world).any(|(m, p)| {
            m.spent.is_none() && p.pos.z - 1.1 < m.vel.length() as f64 * (up + 0.1) as f64
        });
        if near {
            cast(&mut app, warden, shield(), Aim::Entity(foe));
            raised = true;
            break;
        }
    }
    assert!(raised);
    run(&mut app, 1.5);
    assert!(hits_on(&app, warden).is_empty(), "blocked");
    let back = hits_on(&app, foe);
    assert!(!back.is_empty(), "it flew back and hit the foe");
    assert_eq!(back[0].caster, Some(warden), "as the warden's");
    let seen = &app.world.resource::<Seen>().wards;
    assert!(seen.iter().any(|w| w.perfect && w.taken == 0.0));
    let (_, b) = barriers(&mut app)[0].clone();
    assert_eq!(b.pool, b.full, "a perfect block costs nothing");
}

#[test]
fn a_broken_shield_staggers_its_holder_locks_it_out_and_a_held_one_regrows() {
    let mut app = app();
    let (warden, foe) = shield_duel(&mut app);
    cast(
        &mut app,
        warden,
        shield().with(Modifier::Duration { factor: 6.0 }),
        Aim::Entity(foe),
    );
    run(&mut app, 0.6);
    let (e, _) = barriers(&mut app)[0].clone();
    // Two bolts, then quiet: it regrows to full.
    for _ in 0..2 {
        cast(&mut app, foe, bolt(), Aim::Entity(warden));
        run(&mut app, 0.6);
    }
    run(&mut app, 0.6);
    let b = app.world.get::<wards::Barrier>(e).unwrap().clone();
    assert!(b.pool < b.full, "hit");
    let low = b.pool;
    run(&mut app, wards::REGROW_AFTER + 0.2);
    let b = app.world.get::<wards::Barrier>(e).unwrap().clone();
    assert!(
        b.pool > low,
        "regrowing once nothing hits it: {low} -> {}",
        b.pool
    );
    run(&mut app, 1.0 / wards::REGROW_RATE);
    let b = app.world.get::<wards::Barrier>(e).unwrap().clone();
    assert_eq!(b.pool, b.full, "whole again");
    // Bolts until it breaks: the warden staggers and is locked out.
    let mut volleys = 0;
    while app.world.get::<wards::Barrier>(e).is_some() && volleys < 30 {
        cast(&mut app, foe, bolt(), Aim::Entity(warden));
        run(&mut app, 0.3);
        volleys += 1;
    }
    assert!(app.world.get::<wards::Barrier>(e).is_none(), "it broke");
    let a = app.world.get::<status::Afflictions>(warden).unwrap();
    assert!(a.ward_lock > 0.0, "locked out");
    assert!(a.staggered > 0.0, "staggered for a moment");
    assert!(a.time_scale() < 0.5, "slowed while it staggers");
    // Raised again while locked out, it sputters.
    cast(&mut app, warden, shield(), Aim::Entity(foe));
    run(&mut app, 0.5);
    assert!(barriers(&mut app).is_empty(), "no shield while locked out");
    run(&mut app, wards::LOCK_SECS);
    cast(&mut app, warden, shield(), Aim::Entity(foe));
    run(&mut app, 0.5);
    assert_eq!(barriers(&mut app).len(), 1, "up again after the lockout");
}

#[test]
fn a_shield_on_the_spell_bar_stays_up_while_the_key_is_down() {
    let mut app = app();
    let me = weaver(&mut app, DVec3::ZERO);
    app.world.get_mut::<MagicCaster>(me).unwrap().mana = 1.0e4;
    run(&mut app, 0.1);
    let eye = DVec3::new(0.5, 2.0, 3.0);
    let target = DVec3::new(0.0, 1.0, -14.0);
    // Key 5 is the starter Shield: held far past the spell's own 4 s.
    aim_input(&mut app, me, eye, target, |i| i.press(4));
    run(&mut app, 6.0);
    let held = barriers(&mut app);
    assert_eq!(held.len(), 1, "up while the key is down");
    assert!(held[0].1.stands());
    let wards::BarrierKind::Shield { normal, .. } = held[0].1.kind else {
        panic!("{:?}", held[0].1.kind);
    };
    assert!(normal.z < -0.9, "turned to the crosshair: {normal}");
    assert!(
        app.world.get::<MagicCaster>(me).unwrap().mana < 1.0e4,
        "it drains mana"
    );
    aim_input(&mut app, me, eye, target, |i| i.release(4));
    run(&mut app, 0.6);
    assert!(
        barriers(&mut app).is_empty(),
        "lowered when the key comes up"
    );
}
