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
    blasts: Vec<engine_world::WorldImpulse>,
    minds: Vec<engine_world::MindEffect>,
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
    mut blasts: MessageReader<engine_world::WorldImpulse>,
    mut minds: MessageReader<engine_world::MindEffect>,
) {
    seen.minds.extend(minds.read().copied());
    seen.blasts.extend(blasts.read().copied());
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
        off_hand: false,
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
fn a_safeguarded_spells_steam_spares_its_side() {
    let mut app = app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Ascended1);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    // A friend (the same side) right beside the dummy, in the cloud.
    let friend = mage(&mut app, DVec3::new(1.2, 0.0, -10.0), Mastery::Novice);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["frost"], CastType::Bolt).with(Modifier::Safeguard),
        Aim::Entity(d),
    );
    run(&mut app, 0.8);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt).with(Modifier::Safeguard),
        Aim::Entity(d),
    );
    run(&mut app, 3.0);
    assert!(
        hits_on(&app, d).iter().any(|h| h.reaction == Some("steam")),
        "it still makes steam"
    );
    let mut q = app.world.query::<&runtime::Field>();
    assert!(
        q.iter(&app.world)
            .any(|f| f.cast.spell.weave.key == "steam" && f.cast.spell.safeguard),
        "the cloud keeps the safeguard"
    );
    assert!(
        hits_on(&app, friend).iter().all(|h| h.damage <= 0.0),
        "the friend in the cloud is spared"
    );
    assert!(app.world.get::<Vitality>(friend).unwrap().health >= 100.0);
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
fn a_rains_drops_burst_on_the_ground_and_splash_round_it() {
    // An Ascended's rain covers 9 to 14 m: its drops once swept as spheres that wide, so they burst high
    // over the dummies and cost a millisecond of physics a step each.
    let mut app = app();
    let me = app
        .world
        .spawn((
            Name::new("Ascended"),
            Transform::default(),
            MagicCaster {
                mana_regen: 0.0,
                ..MagicCaster::new(88.0, Mastery::Ascended1)
            },
            Vitality::new(100.0),
        ))
        .id();
    let aim = DVec3::new(0.0, 0.0, -16.0);
    let ds: Vec<Entity> = (0..7)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 6.0;
            let r = if i == 0 { 0.0 } else { 3.0 };
            dummy(&mut app, aim + DVec3::new(a.cos() * r, 0.0, a.sin() * r))
        })
        .collect();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Rain),
        Aim::Point(aim),
    );
    let (mut rain_r, mut drop_r, mut count) = (0.0f32, 0.0f32, 0);
    let mut landed: Vec<(Entity, f64)> = Vec::new();
    for _ in 0..300 {
        app.update();
        let mut rains = app.world.query::<&runtime::Rainfall>();
        for r in rains.iter(&app.world) {
            rain_r = r.cast.spell.carrier.radius;
            count = r.cast.spell.carrier.count as usize;
        }
        let mut drops = app
            .world
            .query::<(Entity, &runtime::Missile, &runtime::SpellPos)>();
        for (e, m, sp) in drops.iter(&app.world) {
            drop_r = m.cast.spell.carrier.radius;
            if m.spent.is_some() && !landed.iter().any(|(l, _)| *l == e) {
                landed.push((e, sp.pos.y));
            }
        }
    }
    assert!(rain_r > 8.0, "an Ascended's rain covers {rain_r} m");
    assert!(
        (drop_r - rain_r * runtime::RAIN_DROP_SHARE).abs() < 1e-4 && drop_r < 1.0,
        "a drop is {drop_r} m across a {rain_r} m rain"
    );
    assert!(
        count >= 10 && landed.len() == count,
        "{} of {count} drops landed",
        landed.len()
    );
    let high = landed.iter().map(|(_, y)| *y).fold(0.0, f64::max);
    assert!(high < 2.5, "a drop burst {high:.1} m up");
    let hit = ds.iter().filter(|d| !hits_on(&app, **d).is_empty()).count();
    assert!(hit >= 4, "{hit} of 7 dummies under the rain hit");
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
    let shows = |app: &App| *app.world.get::<Statuses>(warden).unwrap();
    assert!(
        shows(&app).shielding && !shows(&app).staggered,
        "it shows it holds a shield up"
    );
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
    run(&mut app, 0.1);
    assert!(
        shows(&app).staggered && !shows(&app).shielding,
        "and shows it staggers"
    );
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

#[test]
fn the_shield_hall_breaks_one_shield_and_sends_bolts_back_from_the_other() {
    let mut app = app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Shield;
        hall.restart = true;
    }
    let mut most = 0;
    for _ in 0..64 {
        run(&mut app, 0.25);
        most = most.max(app.world.entities().len());
    }
    let seen = &app.world.resource::<Seen>().wards;
    let perfect = seen.iter().filter(|w| w.perfect).count();
    let broke = seen.iter().filter(|w| w.broke).count();
    let soaked = seen.iter().filter(|w| w.taken == 0.0 && !w.perfect).count();
    println!(
        "entities at most {most}, now {}; perfect {perfect}, broke {broke}, soaked {soaked}, struck {}",
        app.world.entities().len(),
        seen.len()
    );
    for w in seen.iter().filter(|w| w.taken > 0.0) {
        println!(
            "ward {:?} taken {} share {} broke {}",
            w.ward, w.taken, w.share, w.broke
        );
    }
    assert_eq!(perfect, 2, "lane 3's two perfect blocks");
    assert!(broke >= 1, "lane 4's shield broke");
    assert!(soaked > 5, "the fire shield soaked the flame jet");
    assert!(most < 4000, "{most}");
}

fn cataclysm_now(app: &mut App) -> serde_json::Value {
    call_api(&mut app.world, "magic.cataclysm", json!({})).unwrap()
}

fn damage_to(app: &App, e: Entity) -> f32 {
    app.world
        .resource::<Seen>()
        .damage
        .iter()
        .filter(|d| d.target == e)
        .map(|d| d.amount)
        .sum()
}

fn named_count(app: &mut App, name: &str) -> usize {
    let mut q = app.world.query::<&Name>();
    q.iter(&app.world).filter(|n| n.as_str() == name).count()
}

fn meteor() -> SpellDesign {
    SpellDesign::new(&["stone"], CastType::Rain).named("Meteor")
}

/// Step until `done` (or `secs` pass); the seconds it took.
fn run_until(app: &mut App, secs: f32, mut done: impl FnMut(&mut App) -> bool) -> Option<f32> {
    for i in 0..(secs * 60.0) as usize {
        app.update();
        if done(app) {
            return Some(i as f32 / 60.0);
        }
    }
    None
}

#[test]
fn a_meteor_falls_on_its_ring_and_blasts_friend_and_foe_into_a_crater() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 22.0), Mastery::Expert);
    // A friend of the caster's beside the dummy it aims at: Cataclysm spares no one.
    let friend = mage(&mut app, DVec3::new(1.6, 0.0, 0.8), Mastery::Adept);
    app.world.entity_mut(friend).insert(Body(0.5));
    let foe = dummy(&mut app, DVec3::new(0.0, 0.0, 0.0));
    let edge = dummy(&mut app, DVec3::new(-3.0, 0.0, 0.0));
    let far = dummy(&mut app, DVec3::new(14.0, 0.0, 0.0));
    let barrel = app
        .world
        .spawn((
            Name::new("Barrel"),
            Transform::from_translation(DVec3::new(0.0, 0.0, -2.2)),
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some(40.0),
                ..Default::default()
            },
        ))
        .id();
    run(&mut app, 0.1);
    cast(&mut app, me, meteor(), Aim::Entity(foe));
    // The ring is on the ground at the dummy's feet before the rock comes down.
    let seen_at = run_until(&mut app, 3.0, |a| {
        cataclysm_now(a)["meteors"].as_array().unwrap().len() == 1
    })
    .expect("a meteor in the sky");
    let now = cataclysm_now(&mut app);
    let m = &now["meteors"][0];
    let lands: Vec<f64> = serde_json::from_value(m["lands_at"].clone()).unwrap();
    assert!(
        lands[0].abs() < 0.1 && lands[1].abs() < 0.05 && lands[2].abs() < 0.1,
        "{m}"
    );
    let eta = m["eta_s"].as_f64().unwrap();
    assert!((1.2..1.7).contains(&eta), "it falls for 1.2-1.7 s: {eta}");
    let at: Vec<f64> = serde_json::from_value(m["at"].clone()).unwrap();
    assert!(
        at[1] > 45.0 && at[2] < -15.0,
        "from high, beyond the target: {at:?}"
    );
    let size = m["size_m"].as_f64().unwrap();
    let blast = m["blast_m"].as_f64().unwrap();
    assert!(
        (0.7..1.6).contains(&size) && blast > 2.0 * size,
        "{size} {blast}"
    );
    assert_eq!(named_count(&mut app, "Meteor mark"), 1);
    assert!(hits_on(&app, foe).is_empty(), "nothing before it lands");
    // It lands.
    let landed = run_until(&mut app, 2.0, |a| {
        !cataclysm_now(a)["craters"].as_array().unwrap().is_empty()
    })
    .expect("a crater");
    println!("seen at {seen_at:.2}s, landed {landed:.2}s later; size {size}, blast {blast}");
    assert!((landed as f64 - eta).abs() < 0.15, "{landed} {eta}");
    run(&mut app, 0.1);
    // Friend and foe were struck, the dummy in the middle hardest; the far one not at all.
    let (hit_foe, hit_edge, hit_friend) = (
        damage_to(&app, foe),
        damage_to(&app, edge),
        damage_to(&app, friend),
    );
    println!("damage: foe {hit_foe}, edge {hit_edge}, friend {hit_friend}");
    assert!(hit_foe > 0.0 && hit_friend > 0.0 && hit_edge > 0.0);
    assert!(hit_foe > hit_edge * 1.3, "falloff: {hit_foe} {hit_edge}");
    assert!(hits_on(&app, far).is_empty() && damage_to(&app, far) == 0.0);
    // Thrown off their feet near the middle, not at the edge.
    assert!(app.world.get::<cataclysm::Knocked>(foe).is_some());
    assert!(app.world.get::<cataclysm::Knocked>(edge).is_none());
    // The barrel flies out and up.
    let imp = app
        .world
        .resource::<Seen>()
        .impulses
        .iter()
        .find(|i| i.entity == barrel)
        .copied()
        .expect("the barrel is thrown");
    assert!(
        imp.impulse.y > 100.0 && imp.impulse.z < -50.0,
        "{:?}",
        imp.impulse
    );
    // The world felt it: a blast with energy enough to break things.
    let w = app
        .world
        .resource::<Seen>()
        .blasts
        .iter()
        .find(|b| b.source == Some(me))
        .copied()
        .expect("a WorldImpulse");
    assert!(w.energy_j > 1.0e5 && w.radius > blast as f32, "{w:?}");
    // The crater: a rim of rock, the boulder in it, hot debris; the ring is gone.
    let c = &cataclysm_now(&mut app)["craters"][0];
    assert!(c["heat"].as_f64().unwrap() > 0.9, "{c}");
    assert!(named_count(&mut app, "Crater rim") >= 9);
    assert!(named_count(&mut app, "Meteor debris") >= 5);
    run(&mut app, 0.5);
    assert_eq!(named_count(&mut app, "Meteor mark"), 0);
    // The dummy gets up again; the boulder cools over ten seconds.
    run(&mut app, 1.2);
    assert!(app.world.get::<cataclysm::Knocked>(foe).is_none());
    run(&mut app, 9.0);
    let c = &cataclysm_now(&mut app)["craters"][0];
    assert!(c["heat"].as_f64().unwrap() < 0.05, "{c}");
}

#[test]
fn a_safeguarded_meteor_spares_friends() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 22.0), Mastery::Expert);
    let friend = mage(&mut app, DVec3::new(1.0, 0.0, 0.0), Mastery::Adept);
    app.world.entity_mut(friend).insert(Body(0.5));
    let foe = dummy(&mut app, DVec3::new(-1.0, 0.0, 0.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        meteor().with(Modifier::Safeguard),
        Aim::Point(DVec3::ZERO),
    );
    run(&mut app, 3.0);
    assert!(damage_to(&app, foe) > 0.0);
    assert_eq!(damage_to(&app, friend), 0.0);
}

#[test]
fn a_masters_swarm_drops_five_meteors_one_after_another() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 24.0), Mastery::Master);
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        meteor().with(Modifier::Split { count: 5 }),
        Aim::Point(DVec3::ZERO),
    );
    let mut most = 0;
    let mut etas = Vec::new();
    for _ in 0..(5.0 * 60.0) as usize {
        app.update();
        let now = cataclysm_now(&mut app);
        let ms = now["meteors"].as_array().unwrap();
        most = most.max(ms.len());
        if ms.len() == 5 && etas.is_empty() {
            etas = ms.iter().map(|m| m["eta_s"].as_f64().unwrap()).collect();
        }
    }
    let now = cataclysm_now(&mut app);
    let craters = now["craters"].as_array().unwrap();
    println!(
        "most in the sky {most}; etas {etas:?}; craters {}",
        craters.len()
    );
    assert_eq!(craters.len(), 5, "five landed");
    assert_eq!(most, 5, "all five were in the sky at once");
    etas.sort_by(f64::total_cmp);
    assert!(etas[4] - etas[0] > 1.2, "one after another: {etas:?}");
    // Spread round the aim, not on one spot.
    let spots: Vec<Vec<f64>> = craters
        .iter()
        .map(|c| serde_json::from_value(c["at"].clone()).unwrap())
        .collect();
    let apart = spots
        .iter()
        .flat_map(|a| spots.iter().map(move |b| (a[0] - b[0]).hypot(a[2] - b[2])))
        .fold(0.0, f64::max);
    assert!(apart > 2.5, "{spots:?}");
}

#[test]
fn a_ward_takes_a_meteor_and_nothing_under_it_is_hurt() {
    let mut app = app();
    let warden = mage(&mut app, DVec3::new(0.0, 0.0, 0.0), Mastery::Master);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, 24.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 1;
    let inside = dummy(&mut app, DVec3::new(1.0, 0.0, 0.5));
    // The dummy is the warden's (it would be spared by the dome otherwise).
    cast(
        &mut app,
        warden,
        SpellDesign::new(&["ward"], CastType::Area),
        Aim::Point(DVec3::ZERO),
    );
    run(&mut app, 0.6);
    assert_eq!(barriers(&mut app).len(), 1);
    cast(
        &mut app,
        foe,
        meteor(),
        Aim::Point(DVec3::new(0.5, 0.0, 0.0)),
    );
    run(&mut app, 3.0);
    let seen = app.world.resource::<Seen>();
    assert!(seen.wards.iter().any(|w| w.barrier), "the dome was struck");
    assert_eq!(damage_to(&app, warden), 0.0);
    assert_eq!(damage_to(&app, inside), 0.0);
    assert!(
        cataclysm_now(&mut app)["craters"]
            .as_array()
            .unwrap()
            .is_empty(),
        "it burst on the dome: no crater under it"
    );
}

#[test]
fn a_fault_line_runs_throws_who_stands_on_it_and_shakes_again() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 0.0), Mastery::Adept);
    let on = dummy(&mut app, DVec3::new(0.2, 0.0, -7.0));
    let late = dummy(&mut app, DVec3::new(-0.2, 0.0, -15.0));
    let off = dummy(&mut app, DVec3::new(4.0, 0.0, -7.0));
    let crate_e = app
        .world
        .spawn((
            Name::new("Crate"),
            Transform::from_translation(DVec3::new(0.0, 0.0, -11.0)),
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some(30.0),
                ..Default::default()
            },
        ))
        .id();
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["stone"], CastType::Wave).named("Fault line"),
        Aim::Point(DVec3::new(0.0, 0.0, -20.0)),
    );
    let started = run_until(&mut app, 2.0, |a| {
        !cataclysm_now(a)["faults"].as_array().unwrap().is_empty()
    })
    .expect("a crack");
    let f = cataclysm_now(&mut app)["faults"][0].clone();
    let len = f["length_m"].as_f64().unwrap();
    assert!((16.0..19.0).contains(&len), "it runs its whole length: {f}");
    let end: Vec<f64> = serde_json::from_value(f["end"].clone()).unwrap();
    assert!(
        end[2] < -15.0 && end[0].abs() < 1.0,
        "toward the aim: {end:?}"
    );
    // The front runs: it reaches the first dummy before the second.
    let t_on = run_until(&mut app, 2.0, |a| !hits_on(a, on).is_empty()).expect("on is hit");
    assert!(hits_on(&app, late).is_empty(), "not yet");
    assert!(
        app.world.get::<cataclysm::Knocked>(on).is_some(),
        "thrown down"
    );
    let t_late = run_until(&mut app, 2.0, |a| !hits_on(a, late).is_empty()).expect("late is hit");
    println!("crack at {started:.2}s; on +{t_on:.2}s, late +{t_late:.2}s; {f}");
    assert!((0.35..0.9).contains(&t_late), "14 m/s: 8 m in {t_late}");
    assert_eq!(hits_on(&app, on).len(), 1, "once as the crack passes");
    assert!(
        app.world
            .resource::<Seen>()
            .impulses
            .iter()
            .any(|i| i.entity == crate_e && i.impulse.y > 0.0),
        "the crate is tossed"
    );
    // The aftershock bursts along the whole line and hits both again.
    run(&mut app, 1.3);
    assert!(cataclysm_now(&mut app)["faults"][0]["aftershock"] == json!(true));
    assert_eq!(hits_on(&app, on).len(), 2);
    assert_eq!(hits_on(&app, late).len(), 2);
    assert!(hits_on(&app, off).is_empty(), "4 m off the line");
    assert!(hits_on(&app, me).is_empty(), "not its caster");
    assert!(named_count(&mut app, "Fault slab") >= 30);
    // It closes and is gone.
    run(&mut app, 4.0);
    assert!(
        cataclysm_now(&mut app)["faults"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(named_count(&mut app, "Fault slab"), 0);
}

#[test]
fn the_cataclysm_hall_drops_a_meteor_a_swarm_and_runs_two_fault_lines() {
    let mut app = app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Cataclysm;
        hall.restart = true;
    }
    let mut landed: Vec<(f32, f64, f64)> = Vec::new();
    let mut faults_seen: Vec<(f32, u64)> = Vec::new();
    let mut shocks: Vec<(f32, u64)> = Vec::new();
    let mut most = 0;
    let mut craters_before = 0;
    let steps = (demo::cataclysm::PERIOD * 60.0) as usize;
    for i in 0..steps {
        app.update();
        let t = i as f32 / 60.0;
        let now = cataclysm_now(&mut app);
        let craters = now["craters"].as_array().unwrap();
        if craters.len() > craters_before {
            for c in &craters[craters_before..] {
                landed.push((
                    t,
                    c["at"][0].as_f64().unwrap(),
                    c["at"][2].as_f64().unwrap(),
                ));
            }
        }
        craters_before = craters.len();
        for f in now["faults"].as_array().unwrap() {
            let id = f["id"].as_u64().unwrap();
            if !faults_seen.iter().any(|(_, x)| *x == id) {
                faults_seen.push((t, id));
                println!("fault at {t:.2}: {f}");
            }
            if f["aftershock"] == json!(true) && !shocks.iter().any(|(_, x)| *x == id) {
                shocks.push((t, id));
            }
        }
        most = most.max(app.world.entities().len());
    }
    for (t, x, z) in &landed {
        println!("crater at {t:.2}s ({x:.1}, {z:.1})");
    }
    println!(
        "faults {faults_seen:?}; aftershocks {shocks:?}; entities at most {most}, now {}",
        app.world.entities().len()
    );
    let lane = |x: f64| demo::art::LANES.iter().position(|l| (l - x).abs() <= 3.0);
    assert_eq!(landed.iter().filter(|c| lane(c.1) == Some(3)).count(), 1);
    assert_eq!(landed.iter().filter(|c| lane(c.1) == Some(4)).count(), 5);
    assert_eq!(faults_seen.len(), 2);
    assert_eq!(shocks.len(), 2);
    // Everything is gone again by the period's end.
    let now = cataclysm_now(&mut app);
    println!("at the end: {now}");
    assert!(now["faults"].as_array().unwrap().is_empty());
    assert!(most < 4000, "{most}");
}

// ------------------------------------------------------------------------------- minds

/// A copy or creature that has just come into the world without a stand-in body.
type NewCreature = (
    Or<(Added<minds::Phantasm>, Added<summon::Minion>)>,
    Without<Body>,
);

/// Copies and creatures are spheres to the stand-in physics, so bolts find them.
fn bodies_for_creatures(mut commands: Commands, q: Query<Entity, NewCreature>) {
    for e in &q {
        commands.entity(e).insert(Body(0.5));
    }
}

fn minds_app() -> App {
    let mut a = app();
    a.add_systems(FixedUpdate, bodies_for_creatures.in_set(SimSet::Physics));
    a
}

fn copies(app: &mut App) -> Vec<(Entity, minds::Phantasm, DVec3)> {
    let mut q = app.world.query::<(Entity, &minds::Phantasm, &Transform)>();
    let mut v: Vec<_> = q
        .iter(&app.world)
        .map(|(e, p, t)| (e, p.clone(), t.translation))
        .collect();
    v.sort_by_key(|c| c.0);
    v
}

/// A student (side 1) and a necromancer (side 2) facing each other down the z axis.
fn two_casters(app: &mut App, student_z: f64, necro_z: f64) -> (Entity, Entity) {
    let me = mage(app, DVec3::new(0.0, 0.0, student_z), Mastery::Expert);
    let necro = mage(app, DVec3::new(0.0, 0.0, necro_z), Mastery::Master);
    app.world.get_mut::<MagicCaster>(me).unwrap().side = 1;
    app.world.get_mut::<MagicCaster>(necro).unwrap().side = 2;
    app.world.entity_mut(necro).insert(Body(0.5));
    app.world.entity_mut(me).insert(Body(0.5));
    (me, necro)
}

#[test]
fn a_mirror_image_steps_copies_out_to_its_sides_that_break_at_a_blow() {
    let mut app = minds_app();
    app.world
        .init_resource::<engine_assets::Assets<engine_render::Mesh>>();
    let (me, foe) = two_casters(&mut app, 0.0, -14.0);
    let look = engine_world::character::Appearance {
        seed: 77,
        ..Default::default()
    };
    app.world
        .entity_mut(me)
        .insert((look.clone(), engine_world::animation::BodyRequest::human(5)));
    run(&mut app, 0.1);
    let mirror = SpellDesign::new(&["phantasm"], CastType::OnSelf).named("Mirror image");
    cast(
        &mut app,
        me,
        mirror,
        Aim::Point(DVec3::new(0.0, 0.0, -10.0)),
    );
    run(&mut app, 2.0);
    let found = copies(&mut app);
    // An Expert makes three (the design's three copies), each the caster to look at.
    assert_eq!(found.len(), 3);
    let mut left = 0;
    let mut right = 0;
    for (e, p, at) in &found {
        assert!(p.around && p.owner == Some(me));
        assert_eq!(
            app.world.get::<engine_world::character::Appearance>(*e),
            Some(&look)
        );
        assert_eq!(app.world.get::<MagicCaster>(*e).unwrap().side, 1);
        let d = at.distance(DVec3::ZERO);
        assert!((1.6..3.4).contains(&d), "{at}");
        if at.x < -0.5 {
            left += 1;
        }
        if at.x > 0.5 {
            right += 1;
        }
    }
    assert!(left >= 1 && right >= 1, "to both sides: {left} {right}");
    // They throw glamour bolts at the foe, as the caster would, and do it no harm.
    run(&mut app, 3.0);
    let glamours = hits_on(&app, foe);
    assert!(!glamours.is_empty(), "no glamour reached the foe");
    let seen = app.world.resource::<Seen>();
    assert!(
        seen.damage
            .iter()
            .all(|d| d.target != foe || d.amount == 0.0)
    );
    // A fire bolt at the one nearest the foe breaks it; the others stand.
    let first = copies(&mut app)
        .into_iter()
        .min_by(|a, b| a.2.z.total_cmp(&b.2.z))
        .unwrap()
        .0;
    let fire = SpellDesign::new(&["fire"], CastType::Bolt).named("Fire bolt");
    cast(&mut app, foe, fire, Aim::Entity(first));
    // It breaks into glowing glass.
    let mut most_glass = 0;
    for _ in 0..90 {
        app.update();
        let mut q = app.world.query_filtered::<(), With<wards::WardShard>>();
        most_glass = most_glass.max(q.iter(&app.world).count());
    }
    assert_eq!(most_glass, minds::GLASS_SHARDS as usize, "glass shards");
    let left_now = copies(&mut app);
    assert_eq!(left_now.len(), 2, "one broke");
    assert!(left_now.iter().all(|c| c.0 != first));
    assert!(app.world.get_entity(first).is_err());
    // The api lists them.
    let v = call_api(&mut app.world, "magic.minds", json!({})).unwrap();
    assert_eq!(v["copies"].as_array().unwrap().len(), 2);
    assert_eq!(v["copies"][0]["kind"], "mirror_image");
}

#[test]
fn a_skeleton_goes_for_a_decoy_before_the_mage_who_made_it() {
    let mut app = minds_app();
    let (me, necro) = two_casters(&mut app, 6.0, -8.0);
    run(&mut app, 0.1);
    cast(
        &mut app,
        necro,
        raise(),
        Aim::Point(DVec3::new(0.0, 0.0, -2.0)),
    );
    run(&mut app, 2.2);
    let skeleton = minions(&mut app)[0].0;
    let decoy = SpellDesign::new(&["phantasm"], CastType::Summon).named("Decoy");
    cast(&mut app, me, decoy, Aim::Point(DVec3::new(4.0, 0.0, 1.0)));
    run(&mut app, 1.0);
    let found = copies(&mut app);
    assert_eq!(found.len(), 1);
    assert!(!found[0].1.around);
    let copy = found[0].0;
    let target = |app: &mut App| {
        minions(app)
            .into_iter()
            .find(|m| m.0 == skeleton)
            .and_then(|m| m.1.target)
    };
    assert_eq!(
        target(&mut app),
        Some(copy),
        "the skeleton turned to the copy"
    );
    // It cuts the copy down (into glass), then goes for the mage.
    run(&mut app, 6.0);
    assert!(copies(&mut app).is_empty(), "the copy still stands");
    let seen = app.world.resource::<Seen>();
    let first_cut_on_me = seen
        .damage
        .iter()
        .position(|d| d.target == me && d.source == Some(skeleton));
    let cut_copy = seen
        .damage
        .iter()
        .position(|d| d.target == copy && d.source == Some(skeleton));
    assert!(cut_copy.is_some(), "the skeleton never cut the copy");
    assert!(first_cut_on_me.is_none_or(|m| m > cut_copy.unwrap()));
}

#[test]
fn fear_sends_a_skeleton_running_from_its_caster_until_it_wears_off() {
    let mut app = minds_app();
    let (me, necro) = two_casters(&mut app, 8.0, -8.0);
    run(&mut app, 0.1);
    let raise = SpellDesign::new(&["raise"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.5 })
        .named("Raise skeleton");
    cast(
        &mut app,
        necro,
        raise,
        Aim::Point(DVec3::new(0.0, 0.0, 2.0)),
    );
    run(&mut app, 2.2);
    let skeleton = minions(&mut app)[0].0;
    let pos = |app: &App| app.world.get::<Transform>(skeleton).unwrap().translation;
    let fear = SpellDesign::new(&["fear"], CastType::Bolt).named("Fear");
    cast(&mut app, me, fear, Aim::Entity(skeleton));
    run(&mut app, 0.8);
    let aff = app.world.get::<status::Afflictions>(skeleton).unwrap();
    assert!(aff.dread > 1.0, "dread {}", aff.dread);
    let from = aff.dread_from.expect("it knows what it fears");
    assert!(from.distance(DVec3::new(0.0, 0.0, 8.0)) < 0.5, "{from}");
    let scared_at = pos(&app).distance(DVec3::new(0.0, 0.0, 8.0));
    run(&mut app, 2.0);
    let fled_to = pos(&app).distance(DVec3::new(0.0, 0.0, 8.0));
    assert!(fled_to > scared_at + 2.5, "{scared_at} -> {fled_to}");
    let m = minions(&mut app)
        .into_iter()
        .find(|m| m.0 == skeleton)
        .unwrap();
    assert!(m.1.target.is_none());
    // It wears off and the skeleton comes back to fight.
    run(&mut app, 3.0);
    let m = minions(&mut app)
        .into_iter()
        .find(|m| m.0 == skeleton)
        .unwrap();
    assert_eq!(m.1.target, Some(me));
    run(&mut app, 3.0);
    assert!(pos(&app).distance(DVec3::new(0.0, 0.0, 8.0)) < fled_to - 2.0);
}

#[test]
fn a_commanded_golem_fights_its_own_side_then_goes_back() {
    let mut app = minds_app();
    let (me, necro) = two_casters(&mut app, 10.0, -8.0);
    run(&mut app, 0.1);
    let golem = SpellDesign::new(&["stone"], CastType::Summon).named("Stone golem");
    cast(
        &mut app,
        necro,
        golem,
        Aim::Point(DVec3::new(-1.5, 0.0, -1.0)),
    );
    run(&mut app, 0.6);
    cast(
        &mut app,
        necro,
        raise(),
        Aim::Point(DVec3::new(1.5, 0.0, -1.0)),
    );
    run(&mut app, 2.4);
    let found = minions(&mut app);
    let g = found
        .iter()
        .find(|m| m.1.kind == summon::MinionKind::Golem)
        .unwrap()
        .0;
    let s = found
        .iter()
        .find(|m| m.1.kind == summon::MinionKind::Skeleton)
        .unwrap()
        .0;
    let command = SpellDesign::new(&["command"], CastType::Bolt).named("Command");
    cast(&mut app, me, command, Aim::Entity(g));
    run(&mut app, 1.0);
    let held = app
        .world
        .get::<minds::Commanded>(g)
        .expect("the golem is held");
    assert_eq!(held.by, Some(me));
    assert!(!held.wrath && held.thread.len() == minds::THREAD_PIECES);
    assert_eq!(app.world.get::<MagicCaster>(g).unwrap().side, 1);
    run(&mut app, 4.0);
    let seen = app.world.resource::<Seen>();
    let blows = seen
        .damage
        .iter()
        .filter(|d| d.source == Some(g) && d.target == s)
        .count();
    assert!(blows > 0, "the golem never struck the skeleton");
    assert!(
        seen.damage
            .iter()
            .all(|d| d.source != Some(g) || d.target != me)
    );
    // An Expert's hold lasts 8 s: then it is the necromancer's again.
    run(&mut app, 4.5);
    assert!(app.world.get::<minds::Commanded>(g).is_none());
    assert_eq!(app.world.get::<MagicCaster>(g).unwrap().side, 2);
    let m = minions(&mut app).into_iter().find(|m| m.0 == g).unwrap();
    assert_eq!(m.1.cast.side, 2);
    assert_eq!(m.1.cast.caster, Some(necro));
    let mut threads = app.world.query::<&minds::ThreadPiece>();
    assert_eq!(threads.iter(&app.world).count(), 0);
}

#[test]
fn wrath_turns_skeletons_on_each_other() {
    let mut app = minds_app();
    let (me, necro) = two_casters(&mut app, 30.0, -8.0);
    run(&mut app, 0.1);
    for x in [-0.8, 0.8] {
        cast(
            &mut app,
            necro,
            raise(),
            Aim::Point(DVec3::new(x, 0.0, 0.0)),
        );
        run(&mut app, 0.7);
    }
    run(&mut app, 1.6);
    let ids: Vec<Entity> = minions(&mut app).into_iter().map(|m| m.0).collect();
    assert_eq!(ids.len(), 2);
    let wrath = SpellDesign::new(&["wrath"], CastType::Area).named("Frenzy");
    cast(&mut app, me, wrath, Aim::Point(DVec3::new(0.0, 0.0, 0.0)));
    run(&mut app, 3.5);
    for e in &ids {
        if let Some(h) = app.world.get::<minds::Commanded>(*e) {
            assert!(h.wrath && h.by.is_none());
        }
    }
    let seen = app.world.resource::<Seen>();
    let at_each_other = seen
        .damage
        .iter()
        .filter(|d| {
            d.source.is_some_and(|s| ids.contains(&s)) && ids.contains(&d.target) && d.amount > 0.0
        })
        .count();
    assert!(at_each_other > 0, "they never struck each other");
}

#[test]
fn a_fire_decoy_bursts_into_fire_on_the_skeleton_that_breaks_it() {
    let mut app = minds_app();
    let (me, necro) = two_casters(&mut app, 8.0, -8.0);
    run(&mut app, 0.1);
    cast(
        &mut app,
        necro,
        raise(),
        Aim::Point(DVec3::new(0.0, 0.0, -2.0)),
    );
    run(&mut app, 2.2);
    let skeleton = minions(&mut app)[0].0;
    let decoy = SpellDesign::new(&["phantasm", "fire"], CastType::Summon).named("Burning decoy");
    cast(&mut app, me, decoy, Aim::Point(DVec3::new(0.0, 0.0, 3.0)));
    run(&mut app, 6.0);
    assert!(copies(&mut app).is_empty(), "the decoy still stands");
    let seen = app.world.resource::<Seen>();
    let burnt = seen
        .damage
        .iter()
        .any(|d| d.target == skeleton && d.source == Some(me) && d.amount > 0.0);
    assert!(burnt, "the copy's fire never reached the skeleton");
    assert!(
        minions(&mut app)
            .iter()
            .all(|m| m.1.kind != summon::MinionKind::Elemental)
    );
}

#[test]
fn a_veiled_mage_fades_out_and_shows_itself_when_it_strikes() {
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Expert);
    app.world.entity_mut(me).insert(Body(0.5));
    let d = dummy(&mut app, DVec3::ZERO);
    run(&mut app, 0.1);
    let veil = SpellDesign::new(&["veil"], CastType::OnSelf).named("Veil");
    cast(&mut app, me, veil, Aim::Entity(me));
    run(&mut app, 1.0);
    let shown = |app: &App| app.world.get::<Statuses>(me).map_or(0.0, |s| s.veil);
    assert_eq!(shown(&app), 1.0, "hidden");
    run(&mut app, 2.0);
    assert_eq!(shown(&app), 1.0, "and stays hidden");
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 1.2);
    assert!(!hits_on(&app, d).is_empty());
    let a = app.world.get::<status::Afflictions>(me).unwrap();
    assert_eq!(a.veiled, 0.0, "the bolt broke the veil");
    run(&mut app, status::VEIL_OUT);
    assert_eq!(shown(&app), 0.0, "seen again");
}

#[test]
fn people_are_told_to_fight_for_the_commanders_faction_or_strike_at_anyone() {
    use engine_world::{Faction, MindKind};
    let mut app = app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Master);
    app.world.entity_mut(me).insert(Faction(7));
    let loner = mage(&mut app, DVec3::new(4.0, 0.0, 6.0), Mastery::Master);
    let guard = dummy(&mut app, DVec3::new(0.0, 0.0, 0.0));
    let other = dummy(&mut app, DVec3::new(4.0, 0.0, 0.0));
    run(&mut app, 0.1);
    let command = || SpellDesign::new(&["command"], CastType::Bolt).named("Command");
    cast(&mut app, me, command(), Aim::Entity(guard));
    cast(&mut app, loner, command(), Aim::Entity(other));
    run(&mut app, 1.0);
    let told = |app: &App, t: Entity| -> Vec<engine_world::MindEffect> {
        let seen = app.world.resource::<Seen>();
        seen.minds
            .iter()
            .filter(|m| m.target == t)
            .copied()
            .collect()
    };
    let g = told(&app, guard);
    assert_eq!(g.len(), 1, "told once: {g:?}");
    assert_eq!(g[0].kind, MindKind::Command { side: 7 });
    assert!(g[0].secs >= 8.0 && g[0].source == Some(me), "{g:?}");
    assert!(
        told(&app, other).is_empty(),
        "a caster with no faction has no side to give a person"
    );
    let wrath = SpellDesign::new(&["wrath"], CastType::Bolt).named("Frenzy");
    cast(&mut app, loner, wrath, Aim::Entity(other));
    run(&mut app, 1.0);
    let o = told(&app, other);
    assert_eq!(o.len(), 1, "{o:?}");
    assert_eq!(o[0].kind, MindKind::Frenzy);
    assert!(o[0].secs >= 4.0, "{o:?}");
}

#[test]
fn the_minds_hall_fools_scares_maddens_and_turns_the_dead() {
    let mut app = minds_app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Minds;
        hall.restart = true;
    }
    let mut most_copies = 0;
    let mut decoys_seen: Vec<Entity> = Vec::new();
    let mut mirrors_seen: Vec<Entity> = Vec::new();
    let mut most_terrified = 0;
    let mut held_golem_at: Option<f32> = None;
    let mut wrath_at: Option<f32> = None;
    let mut first_terror: Option<f32> = None;
    let mut kinds: std::collections::HashMap<Entity, summon::MinionKind> = Default::default();
    // The stop's camera (a 55° tall view, 16:9): how often the fight leaves the picture.
    let view = demo::VIEWS
        .iter()
        .find(|v| v.key == "spells_minds")
        .unwrap();
    let eye = DVec3::from(view.eye);
    let fwd = (DVec3::from(view.target) - eye).normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let tan_y = (55f64.to_radians() / 2.0).tan();
    let in_view = |p: DVec3| {
        let d = p + DVec3::Y * 0.9 - eye;
        let z = d.dot(fwd);
        z > 0.5
            && (d.dot(right) / (z * tan_y * 16.0 / 9.0)).abs() < 0.85
            && (d.dot(up) / (z * tan_y)).abs() < 0.85
    };
    let (mut looked, mut missed) = (0usize, 0usize);
    let steps = (demo::minds::PERIOD * 60.0) as usize;
    for i in 0..steps {
        app.update();
        let t = i as f32 / 60.0;
        let now = call_api(&mut app.world, "magic.minds", json!({})).unwrap();
        let cs = now["copies"].as_array().unwrap();
        most_copies = most_copies.max(cs.len());
        for c in cs {
            let e = Entity::from_bits(c["id"].as_u64().unwrap());
            let list = if c["kind"] == "decoy" {
                &mut decoys_seen
            } else {
                &mut mirrors_seen
            };
            if !list.contains(&e) {
                list.push(e);
                println!("{t:.2}: {} at {}", c["kind"], c["at"]);
            }
        }
        for (e, m) in minions(&mut app) {
            kinds.entry(e).or_insert(m.kind);
            if i % 15 == 0 && t > 1.0 {
                looked += 1;
                if !in_view(m.pos) {
                    missed += 1;
                    println!(
                        "  {t:.2} out of the picture: {e:?} {:?} at {:.1}",
                        m.kind, m.pos
                    );
                }
            }
            if i % 60 == 0 {
                println!(
                    "  {t:.0}s {e:?} {:?} at ({:.1}, {:.1}) side {} target {:?}",
                    m.kind, m.pos.x, m.pos.z, m.cast.side, m.target
                );
            }
        }
        let terrified = now["terrified"].as_array().unwrap().len();
        if terrified > 0 && first_terror.is_none() {
            first_terror = Some(t);
        }
        most_terrified = most_terrified.max(terrified);
        for h in now["held"].as_array().unwrap() {
            if h["wrath"] == json!(true) {
                wrath_at.get_or_insert(t);
            } else {
                held_golem_at.get_or_insert(t);
            }
        }
    }
    let kind = |e: Entity| kinds.get(&e).copied();
    let seen = app.world.resource::<Seen>();
    let cut_decoys = decoys_seen
        .iter()
        .filter(|d| {
            seen.damage
                .iter()
                .any(|x| x.target == **d && x.amount > 0.0)
        })
        .count();
    let skel_on_skel = seen
        .damage
        .iter()
        .filter(|x| {
            x.source.and_then(kind) == Some(summon::MinionKind::Skeleton)
                && kind(x.target) == Some(summon::MinionKind::Skeleton)
        })
        .count();
    let golem_on_skel = seen
        .damage
        .iter()
        .filter(|x| {
            x.source.and_then(kind) == Some(summon::MinionKind::Golem)
                && kind(x.target) == Some(summon::MinionKind::Skeleton)
        })
        .count();
    println!("out of the picture {missed} of {looked} looks");
    println!(
        "copies at most {most_copies}; decoys {} ({cut_decoys} cut), mirrors {}; terror from \
         {first_terror:?}, most terrified {most_terrified}; wrath at {wrath_at:?}; command at \
         {held_golem_at:?}; skeleton on skeleton {skel_on_skel}, golem on skeleton {golem_on_skel}",
        decoys_seen.len(),
        mirrors_seen.len()
    );
    assert_eq!(decoys_seen.len(), 3);
    assert_eq!(mirrors_seen.len(), 4);
    assert!(cut_decoys >= 2, "the dead cut {cut_decoys} decoys");
    assert!(most_terrified >= 3, "{most_terrified} ran");
    assert!(wrath_at.is_some() && held_golem_at.is_some());
    assert!(skel_on_skel > 0, "the frenzied never struck each other");
    assert!(
        golem_on_skel > 0,
        "the commanded golem never struck a skeleton"
    );
    assert!(
        missed * 20 < looked,
        "the fight left the picture {missed} of {looked} looks"
    );
}

// ------------------------------------------------------------------------------- blades

use engine_world::DamageKind;
use engine_world::combat::{ItemClass, Loadout};

/// A spirit blade that has just come into the world is a sphere to the stand-in physics.
type NewSpirit = (Added<blades::Blade>, With<Vitality>, Without<Body>);

fn bodies_for_blades(mut commands: Commands, q: Query<Entity, NewSpirit>) {
    for e in &q {
        commands.entity(e).insert(Body(0.3));
    }
}

fn blade_app() -> App {
    let mut a = minds_app();
    a.world
        .init_resource::<engine_assets::Assets<engine_render::Mesh>>();
    a.add_systems(FixedUpdate, bodies_for_blades.in_set(SimSet::Physics));
    a
}

fn blades_now(app: &mut App) -> Vec<(Entity, blades::Blade)> {
    let mut q = app.world.query::<(Entity, &blades::Blade)>();
    let mut v: Vec<_> = q.iter(&app.world).map(|(e, b)| (e, b.clone())).collect();
    v.sort_by_key(|b| b.0);
    v
}

fn shards(app: &mut App) -> usize {
    let mut q = app.world.query_filtered::<(), With<wards::WardShard>>();
    q.iter(&app.world).count()
}

fn blade(cast: CastType) -> SpellDesign {
    SpellDesign::new(&["blade"], cast).named("Blade")
}

#[test]
fn a_phalanx_stands_over_its_casters_head_and_looses_at_a_foe_that_comes_near() {
    let mut app = blade_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let foe = dummy(&mut app, DVec3::new(0.0, 0.0, -30.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        blade(CastType::Orbit),
        Aim::Point(DVec3::new(0.0, 0.0, -10.0)),
    );
    run(&mut app, 2.0);
    let found = blades_now(&mut app);
    // An Expert's phalanx: three blades in an arch over the head, waiting.
    assert_eq!(found.len(), 3);
    let mut xs: Vec<f64> = found.iter().map(|b| b.1.pos.x).collect();
    xs.sort_by(f64::total_cmp);
    assert!(xs[2] - xs[0] > 0.6, "spread along the arch: {xs:?}");
    for (_, b) in &found {
        assert_eq!(b.kind(), "phalanx");
        assert!((1.8..3.2).contains(&b.pos.y), "{}", b.pos);
        assert!(b.pos.x.abs() < 1.4 && b.pos.z.abs() < 1.2, "{}", b.pos);
        assert!(b.left() > 20.0);
    }
    assert!(hits_on(&app, foe).is_empty(), "a foe 30 m off draws none");
    // The foe comes within reach: the blades fly at it one after another and break on it.
    app.world.get_mut::<Transform>(foe).unwrap().translation = DVec3::new(0.0, 0.0, -6.0);
    let mut most_shards = 0;
    let mut loosed_at = Vec::new();
    for i in 0..150 {
        app.update();
        most_shards = most_shards.max(shards(&mut app));
        let n = blades_now(&mut app).len();
        if loosed_at.len() < 3 - n {
            loosed_at.push(i);
        }
    }
    assert!(blades_now(&mut app).is_empty(), "all three loosed");
    assert!(
        loosed_at.windows(2).all(|w| w[1] - w[0] >= 18),
        "in turn, not at once: {loosed_at:?}"
    );
    let cuts = hits_on(&app, foe);
    assert_eq!(cuts.len(), 3, "each blade struck once");
    let seen = app.world.resource::<Seen>();
    assert!(
        seen.damage
            .iter()
            .filter(|d| d.target == foe)
            .all(|d| d.kind == DamageKind::Slash && d.amount > 0.0 && d.source == Some(me))
    );
    assert!(
        most_shards >= blades::BREAK_SHARDS as usize,
        "{most_shards}"
    );
}

#[test]
fn a_glintblade_flies_point_first_and_breaks_into_shards_on_what_it_hits() {
    let mut app = blade_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let foe = dummy(&mut app, DVec3::new(2.0, 0.0, -12.0));
    run(&mut app, 0.1);
    let fire = SpellDesign::new(&["blade", "fire"], CastType::Bolt).named("Fire glintblade");
    cast(&mut app, me, fire, Aim::Entity(foe));
    let mut drawn = 0;
    let mut most_shards = 0;
    for _ in 0..120 {
        app.update();
        let mut q = app
            .world
            .query::<(&blades::BladeRide, &runtime::Missile, &Transform)>();
        for (r, m, t) in q.iter(&app.world) {
            if r.drawn.is_some() && m.spent.is_none() && m.vel.length() > 1.0 {
                drawn += 1;
                // Its +Y (the blade's point) leads along its flight.
                let up = t.rotation * Vec3::Y;
                assert!(up.dot(m.vel.normalize()) > 0.95, "point first");
                assert!(r.length > 0.8 && r.color[0] > r.color[2], "a fire blade");
            }
        }
        most_shards = most_shards.max(shards(&mut app));
    }
    assert!(drawn > 5, "the sword was drawn on the bolt");
    let cuts = hits_on(&app, foe);
    assert_eq!(cuts.len(), 1);
    assert!(cuts[0].damage > 0.0);
    assert!(
        most_shards >= blades::BREAK_SHARDS as usize,
        "{most_shards}"
    );
    // A fire blade cuts with fire: it sets the foe burning.
    let aff = app.world.get::<status::Afflictions>(foe).unwrap();
    assert!(
        aff.burning.active() || aff.gauges.iter().any(|g| g.element == Element::Fire),
        "{aff:?}"
    );
}

#[test]
fn a_bladefall_drops_blades_down_its_line_that_cut_and_stick() {
    let mut app = blade_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let near = dummy(&mut app, DVec3::new(0.0, 0.0, -4.6));
    let aside = dummy(&mut app, DVec3::new(7.0, 0.0, -8.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        blade(CastType::Rain),
        Aim::Point(DVec3::new(0.0, 0.0, -12.0)),
    );
    let mut most = 0;
    let mut stuck_most = 0;
    let mut grounds = Vec::new();
    for _ in 0..150 {
        app.update();
        let now = blades_now(&mut app);
        most = most.max(now.len());
        let stuck = now
            .iter()
            .filter(|b| matches!(b.1.flight, blades::Flight::Stuck { .. }))
            .count();
        stuck_most = stuck_most.max(stuck);
        for (_, b) in &now {
            if let blades::Flight::Falling { ground } = b.flight {
                grounds.push(ground);
                // Point first, straight down.
                assert!((b.rot * Vec3::Y).y < -0.95);
            }
        }
    }
    assert_eq!(most, 10, "ten blades");
    assert!(stuck_most >= 8, "they stand in the floor: {stuck_most}");
    // Down the line from the caster to the aim, a little either side.
    assert!(
        grounds
            .iter()
            .all(|g| g.x.abs() < 1.6 && (-12.6..-1.5).contains(&g.z))
    );
    let zs: Vec<f64> = grounds.iter().map(|g| g.z).collect();
    let span =
        zs.iter().cloned().fold(f64::MIN, f64::max) - zs.iter().cloned().fold(f64::MAX, f64::min);
    assert!(span > 7.0, "{span}");
    assert!(
        !hits_on(&app, near).is_empty(),
        "the dummy on the line was cut"
    );
    assert!(hits_on(&app, aside).is_empty(), "the one off it wasn't");
    // They fade.
    run(&mut app, 4.0);
    assert!(blades_now(&mut app).is_empty());
}

#[test]
fn spirit_blades_fight_skeletons_beside_their_caster_and_break_when_cut_down() {
    let mut app = blade_app();
    let (me, necro) = two_casters(&mut app, 6.0, -14.0);
    run(&mut app, 0.1);
    let spirits = SpellDesign::new(&["blade"], CastType::Summon)
        .with(Modifier::Split { count: 2 })
        .named("Spirit blades");
    cast(&mut app, me, spirits, Aim::Point(DVec3::new(0.0, 0.0, 3.0)));
    run(&mut app, 1.0);
    let found = blades_now(&mut app);
    assert_eq!(found.len(), 2);
    for (e, b) in &found {
        assert_eq!(b.kind(), "spirit");
        assert!(b.pos.distance(DVec3::new(0.0, 1.5, 6.0)) < 2.2, "{}", b.pos);
        assert_eq!(app.world.get::<MagicCaster>(*e).unwrap().side, 1);
        assert!(app.world.get::<Vitality>(*e).is_some());
    }
    let ids: Vec<Entity> = found.iter().map(|b| b.0).collect();
    cast(
        &mut app,
        necro,
        raise(),
        Aim::Point(DVec3::new(0.0, 0.0, 0.0)),
    );
    run(&mut app, 2.2);
    let skeletons: Vec<Entity> = minions(&mut app).iter().map(|m| m.0).collect();
    assert!(!skeletons.is_empty());
    run(&mut app, 4.0);
    let cuts: Vec<DamageEvent> = app
        .world
        .resource::<Seen>()
        .damage
        .iter()
        .filter(|d| d.source.is_some_and(|s| ids.contains(&s)))
        .cloned()
        .collect();
    assert!(cuts.len() >= 3, "the spirit blades cut: {}", cuts.len());
    assert!(
        cuts.iter().any(|d| skeletons.contains(&d.target)),
        "they cut the dead"
    );
    assert!(cuts.iter().all(|d| d.target != me), "never their caster");
    // One cut down breaks into shards.
    let left = blades_now(&mut app);
    if let Some((e, _)) = left.first() {
        app.world.get_mut::<Vitality>(*e).unwrap().health = 0.0;
        let mut most = 0;
        for _ in 0..30 {
            app.update();
            most = most.max(shards(&mut app));
        }
        assert!(most >= blades::BREAK_SHARDS as usize);
        assert!(app.world.get_entity(*e).is_err());
    }
}

#[test]
fn a_bound_blade_puts_a_sword_in_the_hand_and_gives_the_hand_back() {
    let mut app = blade_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let before = Loadout {
        main: ItemClass::None,
        off: ItemClass::SpellFocus,
        two_handed: false,
    };
    app.world.entity_mut(me).insert(before);
    let foe = dummy(&mut app, DVec3::new(0.0, 0.0, -1.5));
    run(&mut app, 0.1);
    let bound = SpellDesign::new(&["blade", "frost"], CastType::OnSelf)
        .with(Modifier::Duration { factor: 0.2 })
        .named("Bound frost blade");
    cast(&mut app, me, bound, Aim::Point(DVec3::new(0.0, 0.0, -5.0)));
    run(&mut app, 1.0);
    let held = *app.world.get::<Loadout>(me).unwrap();
    assert_eq!(held.main, ItemClass::OneHandedBlade, "Combat sees a sword");
    assert_eq!(held.off, ItemClass::SpellFocus);
    assert!(!held.two_handed);
    let im = app.world.get::<blades::Imbued>(me).unwrap();
    assert!(im.from_blade);
    let found = blades_now(&mut app);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.kind(), "bound");
    let t = app.world.get::<Transform>(found[0].0).unwrap();
    assert!(
        t.translation.distance(DVec3::new(0.25, 1.0, -0.15)) < 0.6,
        "in the hand: {}",
        t.translation
    );
    // No spell was thrown at the foe.
    assert!(hits_on(&app, foe).is_empty());
    // A melee cut from the hand (Combat's, here written by hand) carries the blade's frost.
    let cut = |app: &mut App| {
        app.world.write_message(DamageEvent {
            target: foe,
            amount: 12.0,
            kind: DamageKind::Slash,
            point: DVec3::new(0.0, 1.1, -1.3),
            impulse: Vec3::NEG_Z * 40.0,
            source: Some(me),
        });
    };
    cut(&mut app);
    run(&mut app, 0.1);
    let ridden = hits_on(&app, foe);
    assert_eq!(ridden.len(), 1, "the blade's spell rode the cut");
    assert_eq!(ridden[0].caster, Some(me));
    // Another cut at once doesn't land it again; one after a breath does.
    cut(&mut app);
    run(&mut app, 0.1);
    assert_eq!(hits_on(&app, foe).len(), 1);
    run(&mut app, 0.4);
    cut(&mut app);
    run(&mut app, 0.1);
    assert_eq!(hits_on(&app, foe).len(), 2);
    // Its time ends: the hand is given back as it was.
    run(&mut app, 6.5);
    assert_eq!(app.world.get::<Loadout>(me), Some(&before));
    assert!(blades_now(&mut app).is_empty());
    assert!(app.world.get::<blades::Imbued>(me).is_none());
}

#[test]
fn a_bound_greatblade_is_two_handed_and_a_bare_hand_gets_no_loadout_after() {
    let mut app = blade_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    run(&mut app, 0.1);
    let great = SpellDesign::new(&["blade"], CastType::OnSelf)
        .with(Modifier::Size { factor: 2.5 })
        .with(Modifier::Duration { factor: 0.2 })
        .named("Bound greatblade");
    cast(&mut app, me, great, Aim::Point(DVec3::new(0.0, 0.0, -5.0)));
    run(&mut app, 1.0);
    let held = *app.world.get::<Loadout>(me).unwrap();
    assert_eq!(held.main, ItemClass::TwoHandedBlade);
    assert!(held.two_handed);
    assert!(blades_now(&mut app)[0].1.length >= 1.35);
    run(&mut app, 7.0);
    assert!(app.world.get::<Loadout>(me).is_none());
}

#[test]
fn an_imbue_sets_its_casters_creatures_alight_and_their_cuts_burn() {
    let mut app = blade_app();
    let (me, necro) = two_casters(&mut app, 5.0, -8.0);
    run(&mut app, 0.1);
    cast(
        &mut app,
        necro,
        raise(),
        Aim::Point(DVec3::new(0.0, 0.0, -1.0)),
    );
    run(&mut app, 2.2);
    let skeletons: Vec<Entity> = minions(&mut app).iter().map(|m| m.0).collect();
    assert!(!skeletons.is_empty());
    let imbue = SpellDesign::new(&["fire"], CastType::Imbue).named("Fire weapon");
    cast(
        &mut app,
        necro,
        imbue,
        Aim::Point(DVec3::new(0.0, 0.0, 0.0)),
    );
    run(&mut app, 0.8);
    for s in &skeletons {
        let im = app
            .world
            .get::<blades::Imbued>(*s)
            .expect("every creature of the caster near it is imbued");
        assert!(!im.from_blade && im.left() > 20.0);
    }
    assert!(
        app.world.get::<blades::Imbued>(necro).is_some(),
        "and its own hand"
    );
    // Their blades glow orange.
    let m = minions(&mut app)[0].1.clone();
    let mut glowing = 0;
    for p in &m.parts {
        if let Some(r) = app.world.get::<engine_render::MeshRenderer>(*p)
            && let engine_render::MaterialRef::Inline(mat) = &r.material
            && mat.emissive_strength >= blades::IMBUE_GLOW
            && mat.emissive[0] > mat.emissive[2]
        {
            glowing += 1;
        }
    }
    assert_eq!(glowing, 1, "the blade, not the bones");
    // A hand glows on the caster.
    let mut q = app.world.query::<&blades::HandGlow>();
    assert_eq!(q.iter(&app.world).filter(|h| h.owner == necro).count(), 1);
    // The skeletons cut the student and each cut lands the fire too.
    run(&mut app, 6.0);
    let fire_hits: Vec<SpellHit> = hits_on(&app, me)
        .into_iter()
        .filter(|h| h.spell.name == "Fire weapon")
        .collect();
    assert!(!fire_hits.is_empty(), "no cut burned");
    assert!(
        fire_hits
            .iter()
            .all(|h| h.caster.is_some_and(|c| skeletons.contains(&c)))
    );
    let aff = app.world.get::<status::Afflictions>(me).unwrap();
    assert!(
        aff.burning.active() || aff.gauges.iter().any(|g| g.element == Element::Fire),
        "{aff:?}"
    );
    // The api lists them.
    let v = call_api(&mut app.world, "magic.blades", json!({})).unwrap();
    assert!(v["imbued"].as_array().unwrap().len() > skeletons.len());
    // One falls: its blade goes on as plain bones on the floor, the fire out with it.
    let (e, m) = minions(&mut app)
        .into_iter()
        .find(|(e, _)| app.world.get::<blades::Imbued>(*e).is_some())
        .expect("an imbued skeleton still standing");
    app.world.get_mut::<Vitality>(e).unwrap().health = 0.0;
    run(&mut app, 0.3);
    assert!(app.world.get::<summon::Minion>(e).is_none(), "it fell");
    let still_burning = m
        .parts
        .iter()
        .filter(|p| {
            app.world
                .get::<engine_render::MeshRenderer>(**p)
                .is_some_and(|r| {
                    matches!(&r.material, engine_render::MaterialRef::Inline(mat)
                        if mat.emissive_strength >= blades::IMBUE_GLOW)
                })
        })
        .count();
    assert_eq!(still_burning, 0, "a fallen skeleton's blade stops burning");
}

#[test]
fn an_off_hand_cast_leaves_from_the_other_hand_and_a_charged_heavy_hits_harder() {
    // Where it leaves: the hand mirrored, or the left hand's bone of a posed body.
    let t = GlobalTransform::from(Transform::from_translation(DVec3::new(3.0, 0.0, 0.0)));
    let caster = MagicCaster::new(10.0, Mastery::Expert);
    let main = runtime::cast_hand(&t, &caster, None, None, false);
    let off = runtime::cast_hand(&t, &caster, None, None, true);
    assert!(main.x > 3.1 && off.x < 2.9, "{main} {off}");
    let focus = Loadout {
        main: ItemClass::OneHandedBlade,
        off: ItemClass::SpellFocus,
        two_handed: false,
    };
    assert_eq!(
        runtime::cast_hand(&t, &caster, None, Some(&focus), false),
        off,
        "a spell focus in the off hand casts from it"
    );
    let pose = engine_world::animation::SkeletonPose {
        names: vec!["hand_r".to_string(), "hand_l".to_string()].into(),
        parents: vec![None, None].into(),
        bones: vec![
            engine_world::animation::BonePose {
                translation: Vec3::new(0.3, 1.0, -0.2),
                ..Default::default()
            },
            engine_world::animation::BonePose {
                translation: Vec3::new(-0.35, 1.05, -0.3),
                ..Default::default()
            },
        ],
    };
    let at = runtime::cast_hand(&t, &caster, Some(&pose), None, true);
    assert!(at.distance(DVec3::new(2.65, 1.05, -0.3)) < 1e-4, "{at}");
    // A heavy attack held the off hand: its full charge doubles the bolt's power, and it goes
    // off as the heavy lands (gathered).
    let mut app = blade_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let a = dummy(&mut app, DVec3::new(-3.0, 0.0, -10.0));
    let b = dummy(&mut app, DVec3::new(3.0, 0.0, -10.0));
    run(&mut app, 0.1);
    let bolt = SpellDesign::new(&["frost"], CastType::Bolt).named("Frost bolt");
    cast(&mut app, me, bolt.clone(), Aim::Entity(a));
    run(&mut app, 2.0);
    app.world.write_message(CastRequest {
        caster: me,
        spell: SpellChoice::Design(bolt),
        aim: Aim::Entity(b),
        charge: 1.0,
        gathered: true,
        off_hand: true,
    });
    run(&mut app, 2.0);
    let first = |e: Entity| {
        app.world
            .resource::<Seen>()
            .damage
            .iter()
            .find(|d| d.target == e)
            .map_or(0.0, |d| d.amount)
    };
    let plain = first(a);
    let heavy = first(b);
    assert!(plain > 0.0);
    assert!(heavy > plain * 1.25, "{plain} -> {heavy}");
}

#[test]
fn the_blades_hall_burns_the_dead_and_answers_with_blades() {
    let mut app = blade_app();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Blades;
        hall.restart = true;
    }
    let view = demo::VIEWS
        .iter()
        .find(|v| v.key == "spells_blades")
        .unwrap();
    let eye = DVec3::from(view.eye);
    let fwd = (DVec3::from(view.target) - eye).normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let tan_y = (55f64.to_radians() / 2.0).tan();
    let in_view = |p: DVec3| {
        let d = p - eye;
        let z = d.dot(fwd);
        z > 0.5
            && (d.dot(right) / (z * tan_y * 16.0 / 9.0)).abs() < 0.9
            && (d.dot(up) / (z * tan_y)).abs() < 0.9
    };
    let (mut looked, mut missed) = (0usize, 0usize);
    let mut seen_blades: std::collections::HashMap<Entity, &'static str> = Default::default();
    let mut most_imbued = 0;
    let mut most_stuck = 0;
    let mut kinds: std::collections::HashMap<Entity, summon::MinionKind> = Default::default();
    let mut bound_at = None;
    let mut lowest = f32::MAX;
    let steps = (demo::blades::PERIOD * 60.0) as usize;
    for i in 0..steps {
        app.update();
        let t = i as f32 / 60.0;
        for (e, b) in blades_now(&mut app) {
            let k = b.kind();
            if seen_blades.insert(e, k).is_none() {
                print!("[{:.2} m] ", b.length);
                match b.flight {
                    blades::Flight::Waiting { ground, .. } => {
                        println!("{t:.2}: bladefall onto {:.1}", ground)
                    }
                    _ => println!("{t:.2}: {k} at {:.1}", b.pos),
                }
            }
            if k == "spirit" && i % 30 == 0 {
                println!(
                    "  {t:.2} spirit {e:?} at {:.1} target {:?}",
                    b.pos, b.target
                );
            }
            if k == "bound" {
                bound_at.get_or_insert(t);
            }
            if i % 15 == 0 && !matches!(b.flight, blades::Flight::Waiting { .. }) {
                looked += 1;
                if !in_view(b.pos) {
                    missed += 1;
                    println!("  {t:.2} blade out of the picture: {k} at {:.1}", b.pos);
                }
            }
        }
        if i % 30 == 0 {
            // The masters on the plinths stand the whole period.
            let mut q = app.world.query::<(&demo::art::HallMage, &Vitality)>();
            lowest = q.iter(&app.world).fold(lowest, |m, (_, v)| m.min(v.health));
        }
        let stuck = blades_now(&mut app)
            .iter()
            .filter(|b| matches!(b.1.flight, blades::Flight::Stuck { .. }))
            .count();
        most_stuck = most_stuck.max(stuck);
        let mut q = app.world.query::<&blades::Imbued>();
        most_imbued = most_imbued.max(q.iter(&app.world).count());
        for (e, m) in minions(&mut app) {
            kinds.entry(e).or_insert(m.kind);
            if i % 15 == 0 && t > 1.5 {
                looked += 1;
                if !in_view(m.pos + DVec3::Y * 0.9) {
                    missed += 1;
                    println!(
                        "  {t:.2} out of the picture: {e:?} {:?} at {:.1}",
                        m.kind, m.pos
                    );
                }
            }
            if i % 60 == 0 {
                println!(
                    "  {t:.0}s {e:?} {:?} at ({:.1}, {:.1}) target {:?}",
                    m.kind, m.pos.x, m.pos.z, m.target
                );
            }
        }
    }
    let masters: Vec<Entity> = {
        let mut q = app.world.query::<(Entity, &demo::art::HallMage)>();
        q.iter(&app.world).map(|(e, _)| e).collect()
    };
    let count = |k: &str| seen_blades.values().filter(|v| **v == k).count();
    let seen = app.world.resource::<Seen>();
    let spirits: Vec<Entity> = seen_blades
        .iter()
        .filter(|(_, k)| **k == "spirit")
        .map(|(e, _)| *e)
        .collect();
    let spirit_cuts = seen
        .damage
        .iter()
        .filter(|d| d.source.is_some_and(|s| spirits.contains(&s)))
        .count();
    // Blows that shove a master on its plinth (a body knocked off one goes limp, as the third
    // film showed): the golems never reach the plinths, and a skeleton seldom does.
    let shoves: Vec<Option<summon::MinionKind>> = seen
        .damage
        .iter()
        .filter(|d| masters.contains(&d.target) && d.impulse.length() > 1.0)
        .filter_map(|d| d.source.and_then(|s| kinds.get(&s).copied()).map(Some))
        .collect();
    let burning_cuts = seen
        .hits
        .iter()
        .filter(|h| h.spell.name == "Fire weapon" && h.caster.and_then(|c| kinds.get(&c)).is_some())
        .count();
    let blade_hits = seen
        .hits
        .iter()
        .filter(|h| h.spell.payload.blades > 0 && h.damage > 0.0)
        .count();
    println!("out of the picture {missed} of {looked} looks");
    println!(
        "phalanx {}, spirit {}, bound {}, bladefall {} (most stuck {most_stuck}); imbued at most \
         {most_imbued}; spirit cuts {spirit_cuts}, burning cuts {burning_cuts}, blade hits \
         {blade_hits}; bound at {bound_at:?}; lowest master {lowest}; shoves {}",
        count("phalanx"),
        count("spirit"),
        count("bound"),
        count("bladefall"),
        shoves.len(),
    );
    assert_eq!(count("phalanx"), 20, "four phalanxes of five");
    assert_eq!(count("spirit"), 2);
    assert_eq!(count("bound"), 1);
    assert_eq!(count("bladefall"), 60, "three bladefalls of twenty");
    assert!(most_stuck >= 15, "{most_stuck}");
    assert!(most_imbued >= 6, "{most_imbued}");
    assert!(spirit_cuts >= 4, "{spirit_cuts}");
    assert!(burning_cuts >= 2, "{burning_cuts}");
    assert!(blade_hits >= 10, "{blade_hits}");
    assert!(
        missed * 20 < looked,
        "the fight left the picture {missed} of {looked} looks"
    );
    assert!(
        lowest > 1000.0,
        "a master on a plinth nearly fell: {lowest}"
    );
    assert!(
        !shoves.contains(&Some(summon::MinionKind::Golem)) && shoves.len() <= 4,
        "{shoves:?}"
    );
}

// ------------------------------------------------------------------------------- pacts

/// The stand-in physics sees a hellgate as a big sphere round its middle.
fn bodies_for_gates(mut commands: Commands, q: Query<Entity, Added<demons::Hellgate>>) {
    for e in &q {
        commands.entity(e).insert(Body(1.3));
    }
}

fn pact_app() -> App {
    let mut a = minds_app();
    a.world
        .init_resource::<engine_assets::Assets<engine_render::Mesh>>();
    a.add_systems(FixedUpdate, bodies_for_gates.in_set(SimSet::Physics));
    a
}

fn chains_now(app: &mut App) -> Vec<(Entity, chains::Chain)> {
    let mut q = app.world.query::<(Entity, &chains::Chain)>();
    let mut v: Vec<_> = q.iter(&app.world).map(|(e, c)| (e, c.clone())).collect();
    v.sort_by_key(|c| c.0);
    v
}

fn of_kind(app: &mut App, kind: summon::MinionKind) -> Vec<(Entity, summon::Minion)> {
    minions(app)
        .into_iter()
        .filter(|m| m.1.kind == kind)
        .collect()
}

#[test]
fn chains_burst_from_the_floor_and_bind_a_golem_until_they_let_go() {
    let mut app = pact_app();
    let (me, warlock) = two_casters(&mut app, 4.0, -14.0);
    run(&mut app, 0.1);
    let golem = SpellDesign::new(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.3 })
        .named("Stone golem");
    cast(&mut app, me, golem, Aim::Point(DVec3::new(0.0, 0.0, 0.0)));
    run(&mut app, 2.5);
    let (g, _) = minions(&mut app)[0].clone();
    let bind = SpellDesign::new(&["chains"], CastType::Area).named("Binding chains");
    cast(&mut app, warlock, bind, Aim::Entity(g));
    run(&mut app, 1.2);
    // A binding holds it with four chains out of a sigil on the floor.
    let mut q = app.world.query::<&chains::Binding>();
    let binds: Vec<chains::Binding> = q.iter(&app.world).cloned().collect();
    assert_eq!(binds.len(), 1, "one binding");
    assert_eq!(binds[0].target, g);
    assert_eq!(binds[0].chains.len(), 4);
    assert!(binds[0].secs >= 1.5);
    assert!(app.world.get::<Statuses>(g).is_some_and(|s| s.rooted));
    let held_at = app.world.get::<summon::Minion>(g).unwrap().pos;
    let found = chains_now(&mut app);
    assert_eq!(found.len(), 4);
    for (_, c) in &found {
        // Each comes out of the floor about a metre from the golem and holds a fist or a foot.
        assert!(c.a_at.y.abs() < 0.05, "{}", c.a_at);
        let from =
            DVec3::new(c.a_at.x, 0.0, c.a_at.z).distance(DVec3::new(held_at.x, 0.0, held_at.z));
        assert!(from > 0.6 && from < 2.0, "{from}");
        assert!(c.points.last().unwrap().distance(c.b_at) < 0.01);
        assert!(c.links.len() >= 4);
    }
    // Two hold its fists, up off the floor; the links are drawn on the line.
    let high = found.iter().filter(|c| c.1.b_at.y > 0.6).count();
    assert!(high >= 2, "{high}");
    let (_, c) = &found[0];
    let link = app
        .world
        .get::<Transform>(c.links[c.links.len() / 2])
        .unwrap();
    assert!(link.scale.length() > 0.05);
    // While bound it doesn't move, though a foe stands in reach.
    run(&mut app, 1.0);
    let now = app.world.get::<summon::Minion>(g).unwrap().pos;
    assert!(now.distance(held_at) < 0.05, "{held_at} {now}");
    // Then the chains let go, fall and are gone, and the golem walks again.
    run(&mut app, 4.0);
    assert!(chains_now(&mut app).is_empty());
    let mut q = app.world.query::<&chains::Binding>();
    assert_eq!(q.iter(&app.world).count(), 0);
    assert!(app.world.get::<Statuses>(g).is_some_and(|s| !s.rooted));
}

#[test]
fn a_chain_hook_bites_a_golem_and_drags_it_to_its_caster() {
    let mut app = pact_app();
    let (me, warlock) = two_casters(&mut app, -3.0, 20.0);
    run(&mut app, 0.1);
    let golem = SpellDesign::new(&["stone"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.3 })
        .named("Stone golem");
    cast(&mut app, me, golem, Aim::Point(DVec3::new(0.0, 0.0, 0.0)));
    run(&mut app, 2.5);
    let (g, m) = minions(&mut app)[0].clone();
    let before = m.pos;
    let hook = SpellDesign::new(&["chains"], CastType::Bolt).named("Chain hook");
    cast(&mut app, warlock, hook, Aim::Entity(g));
    // While it flies the chain pays out behind it from the hand.
    let mut paying = false;
    let mut caught = false;
    for _ in 0..80 {
        app.update();
        let mut q = app.world.query::<&chains::Hook>();
        for h in q.iter(&app.world) {
            paying |= h.caught.is_none();
            caught |= h.caught == Some(g);
        }
    }
    assert!(paying, "no chain behind the bolt");
    assert!(caught, "the hook never bit");
    run(&mut app, 0.6);
    let after = app.world.get::<summon::Minion>(g).unwrap().pos;
    assert!(after.z - before.z > 6.0, "dragged: {before} {after}");
    // It comes to a stop before its caster, not on it.
    assert!(after.distance(DVec3::new(0.0, 0.0, 20.0)) > 1.4, "{after}");
    run(&mut app, 1.5);
    let mut q = app.world.query::<&chains::Hook>();
    assert_eq!(q.iter(&app.world).count(), 0);
    assert!(chains_now(&mut app).is_empty());
}

#[test]
fn fatal_bonds_chain_bodies_together_and_share_their_pain() {
    let mut app = pact_app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 8.0), Mastery::Master);
    let ds: Vec<Entity> = [-2.0, 0.0, 2.0]
        .iter()
        .map(|&x| dummy(&mut app, DVec3::new(x, 0.0, 0.0)))
        .collect();
    run(&mut app, 0.1);
    let bonds = SpellDesign::new(&["chains"], CastType::Chain).named("Fatal bonds");
    cast(&mut app, me, bonds, Aim::Entity(ds[1]));
    run(&mut app, 1.0);
    let mut q = app.world.query::<(Entity, &chains::Bond)>();
    let found: Vec<(Entity, chains::Bond)> =
        q.iter(&app.world).map(|(e, b)| (e, b.clone())).collect();
    assert_eq!(found.len(), 1, "one bond");
    let (be, bond) = found[0].clone();
    assert_eq!(bond.members.len(), 3, "{:?}", bond.members);
    assert_eq!(bond.chains.len(), 2);
    assert_eq!(chains_now(&mut app).len(), 2);
    // A fire bolt on one: the others take a quarter of it.
    app.world.resource_mut::<Seen>().damage.clear();
    let bolt = SpellDesign::new(&["fire"], CastType::Bolt).named("Firebolt");
    cast(&mut app, me, bolt, Aim::Entity(ds[0]));
    run(&mut app, 1.0);
    let seen = app.world.resource::<Seen>();
    let blow = seen
        .damage
        .iter()
        .find(|d| d.target == ds[0] && d.source == Some(me))
        .expect("the bolt struck")
        .amount;
    for &o in &ds[1..] {
        let shared: f32 = seen
            .damage
            .iter()
            .filter(|d| d.target == o && d.source == Some(be))
            .map(|d| d.amount)
            .sum();
        assert!(
            (shared - blow * chains::BOND_SHARE).abs() < blow * 0.02 + 0.01,
            "{shared} of {blow}"
        );
    }
    // Shared pain isn't shared again.
    assert!(seen.damage.iter().filter(|d| d.source == Some(be)).count() == 2);
}

#[test]
fn a_hellgate_opens_sends_hounds_then_imps_and_closes_when_broken() {
    let mut app = pact_app();
    let (me, warlock) = two_casters(&mut app, 4.0, -12.0);
    app.world.get_mut::<Vitality>(me).unwrap().max = 4000.0;
    app.world.get_mut::<Vitality>(me).unwrap().health = 4000.0;
    run(&mut app, 0.1);
    let gate = SpellDesign::new(&["demon"], CastType::Summon).named("Hellgate");
    cast(
        &mut app,
        warlock,
        gate,
        Aim::Point(DVec3::new(0.0, 0.0, -5.0)),
    );
    run(&mut app, 0.5);
    let mut q = app.world.query::<(Entity, &demons::Hellgate)>();
    let (ge, g) = q
        .iter(&app.world)
        .map(|(e, g)| (e, g.clone()))
        .next()
        .expect("a gate opened");
    assert!(g.at.distance(DVec3::new(0.0, 0.0, -5.0)) < 0.5);
    // It faces its caster's foe.
    assert!(g.facing.z > 0.9, "{}", g.facing);
    assert!(app.world.get::<Vitality>(ge).is_some());
    run(&mut app, 1.0);
    assert!(
        app.world.get::<demons::Hellgate>(ge).unwrap().open() > 0.95,
        "open after a second"
    );
    // Two hounds leap out first, onto the floor in front of it; then they spring at the
    // student.
    let mut pounced = false;
    let mut watch = |app: &mut App, secs: f32| {
        for _ in 0..(secs * 60.0) as usize {
            app.update();
            pounced |= of_kind(app, summon::MinionKind::Hellhound)
                .iter()
                .any(|h| matches!(h.1.act, summon::Act::Pounce { .. }) && h.1.attacks > 0);
        }
    };
    watch(&mut app, 1.6);
    let hounds = of_kind(&mut app, summon::MinionKind::Hellhound);
    assert_eq!(hounds.len(), 2);
    assert!(of_kind(&mut app, summon::MinionKind::Imp).is_empty());
    for (h, m) in &hounds {
        assert_eq!(m.parts.len(), demons::HOUND_PARTS);
        assert_eq!(app.world.get::<MagicCaster>(*h).unwrap().side, 2);
        assert!(m.pos.z > -4.0, "in front of the gate: {}", m.pos);
    }
    // Then imps, every few seconds.
    watch(&mut app, 3.0);
    let imps = of_kind(&mut app, summon::MinionKind::Imp);
    assert!(!imps.is_empty(), "no imp came");
    assert_eq!(imps[0].1.parts.len(), demons::IMP_PARTS);
    // The hounds pounced on the student and bite it, standing on the floor before it.
    watch(&mut app, 4.0);
    assert!(pounced, "no hound pounced");
    for (_, m) in of_kind(&mut app, summon::MinionKind::Hellhound) {
        assert!(
            m.pos.y.abs() < 0.05,
            "a hound stands on something: {}",
            m.pos
        );
        let gap = summon::flat_distance(m.pos, DVec3::new(0.0, 0.0, 4.0));
        assert!(gap > 0.6 && gap < 2.2, "{gap}");
    }
    let ids: Vec<Entity> = hounds.iter().map(|h| h.0).collect();
    let seen = app.world.resource::<Seen>();
    let bites: Vec<&DamageEvent> = seen
        .damage
        .iter()
        .filter(|d| d.source.is_some_and(|s| ids.contains(&s)))
        .collect();
    assert!(!bites.is_empty(), "no bite landed");
    assert!(
        bites.iter().all(|b| b.target == me),
        "{:?}",
        bites.iter().map(|b| (b.target, b.kind)).collect::<Vec<_>>()
    );
    // Teeth, and the fire in them.
    assert!(bites.iter().any(|b| b.kind == DamageKind::Pierce));
    // The imps' firebolts reach it too.
    let imp_ids: Vec<Entity> = of_kind(&mut app, summon::MinionKind::Imp)
        .iter()
        .map(|m| m.0)
        .collect();
    let seen = app.world.resource::<Seen>();
    assert!(
        seen.hits
            .iter()
            .any(|h| h.target == Some(me) && h.caster.is_some_and(|c| imp_ids.contains(&c))),
        "no firebolt from an imp struck"
    );
    // Broken, it closes and is gone; its demons stay.
    app.world.get_mut::<Vitality>(ge).unwrap().health = 0.0;
    run(&mut app, 1.0);
    assert!(app.world.get_entity(ge).is_err());
    assert!(!of_kind(&mut app, summon::MinionKind::Hellhound).is_empty());
}

#[test]
fn a_demonic_meteor_bursts_into_three_imps_that_fly_up() {
    let mut app = pact_app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 10.0), Mastery::Expert);
    run(&mut app, 0.1);
    let meteor = SpellDesign::new(&["demon"], CastType::Bolt).named("Demonic meteor");
    cast(&mut app, me, meteor, Aim::Point(DVec3::new(0.0, 0.0, 0.0)));
    let mut rock = false;
    let mut imps = Vec::new();
    for _ in 0..120 {
        app.update();
        let mut q = app.world.query::<&demons::MeteorRide>();
        rock |= q.iter(&app.world).any(|r| r.rock.is_some());
        imps = of_kind(&mut app, summon::MinionKind::Imp);
        if !imps.is_empty() {
            break;
        }
    }
    assert!(rock, "no rock rode the bolt");
    // Three leap out of the crater where it struck.
    assert_eq!(imps.len(), 3);
    for (_, m) in &imps {
        assert!(m.pos.distance(DVec3::new(0.0, 1.2, 0.0)) < 2.0, "{}", m.pos);
    }
    run(&mut app, 2.0);
    let imps = of_kind(&mut app, summon::MinionKind::Imp);
    assert_eq!(imps.len(), 3, "they last the pact's time");
    for (_, m) in imps {
        assert!(m.pos.y > 1.4, "it flies: {}", m.pos);
    }
}

#[test]
fn the_pacts_hall_opens_a_gate_binds_hooks_and_the_students_break_it() {
    let mut app = blade_app();
    app.add_systems(FixedUpdate, bodies_for_gates.in_set(SimSet::Physics));
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Pacts;
        hall.restart = true;
    }
    let view = demo::VIEWS
        .iter()
        .find(|v| v.key == "spells_pacts")
        .unwrap();
    let eye = DVec3::from(view.eye);
    let fwd = (DVec3::from(view.target) - eye).normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let tan_y = (55f64.to_radians() / 2.0).tan();
    let in_view = |p: DVec3| {
        let d = p - eye;
        let z = d.dot(fwd);
        z > 0.5
            && (d.dot(right) / (z * tan_y * 16.0 / 9.0)).abs() < 0.9
            && (d.dot(up) / (z * tan_y)).abs() < 0.9
    };
    let (mut looked, mut missed) = (0usize, 0usize);
    let mut kinds: std::collections::HashMap<Entity, summon::MinionKind> = Default::default();
    let mut gate_seen: Option<(Entity, f32, DVec3)> = None;
    let mut gate_gone = None;
    let mut bound: Vec<(f32, Entity)> = Vec::new();
    let mut hooked: Vec<(f32, Entity)> = Vec::new();
    let mut most_bonded = 0;
    let mut most_chains = 0;
    let mut lowest = f32::MAX;
    let mut fell: Vec<Entity> = Vec::new();
    let mut pounces = 0;
    let steps = (demo::pacts::PERIOD * 60.0) as usize;
    for i in 0..steps {
        app.update();
        let t = i as f32 / 60.0;
        let mut q = app.world.query::<(Entity, &demons::Hellgate, &Vitality)>();
        let gates: Vec<(Entity, demons::Hellgate, f32)> = q
            .iter(&app.world)
            .map(|(e, g, v)| (e, g.clone(), v.health))
            .collect();
        for (e, g, hp) in &gates {
            if gate_seen.is_none() {
                println!("{t:.2}: gate at {:.1} facing {:.2}", g.at, g.facing);
                gate_seen = Some((*e, t, g.at));
                assert!(
                    in_view(g.at + DVec3::Y * 1.5),
                    "the gate is out of the picture"
                );
            }
            if i % 60 == 0 {
                println!(
                    "  {t:.0}s gate health {hp:.0} hounds {} imps {}",
                    g.hounds, g.imps
                );
            }
        }
        if let Some((ge, ..)) = gate_seen
            && gate_gone.is_none()
            && app.world.get_entity(ge).is_err()
        {
            println!("{t:.2}: the gate is gone");
            gate_gone = Some(t);
        }
        let mut q = app.world.query::<&chains::Binding>();
        for b in q.iter(&app.world) {
            if !bound.iter().any(|(_, e)| *e == b.target)
                || bound
                    .last()
                    .is_some_and(|l| t - l.0 > 4.0 && l.1 == b.target)
            {
                println!("{t:.2}: bound {:?} {:?}", b.target, kinds.get(&b.target));
                bound.push((t, b.target));
            }
        }
        let mut q = app.world.query::<&chains::Hook>();
        for h in q.iter(&app.world) {
            if let Some(c) = h.caught
                && !hooked.iter().any(|(at, e)| *e == c && t - at < 2.0)
            {
                println!("{t:.2}: hooked {c:?} {:?}", kinds.get(&c));
                hooked.push((t, c));
            }
        }
        let mut q = app.world.query::<&chains::Bond>();
        for b in q.iter(&app.world) {
            if b.members.len() > most_bonded {
                println!(
                    "{t:.2}: bonded {:?}",
                    b.members
                        .iter()
                        .map(|m| (m, kinds.get(m)))
                        .collect::<Vec<_>>()
                );
            }
        }
        most_bonded = q
            .iter(&app.world)
            .fold(most_bonded, |m, b| m.max(b.members.len()));
        most_chains = most_chains.max(chains_now(&mut app).len());
        if i % 30 == 0 {
            let mut q = app.world.query::<(&demo::art::HallMage, &Vitality)>();
            lowest = q.iter(&app.world).fold(lowest, |m, (_, v)| m.min(v.health));
        }
        let now = minions(&mut app);
        for (e, m) in &now {
            if matches!(m.act, summon::Act::Pounce { t, .. } if t < 0.02) && m.attacks > 0 {
                println!("{t:.2}: {e:?} pounces from {:.1}", m.pos);
                pounces += 1;
            }
        }
        for (e, k) in &kinds {
            if !now.iter().any(|m| m.0 == *e) && !fell.contains(e) {
                println!("{t:.2}: {e:?} {k:?} falls");
                fell.push(*e);
            }
        }
        for (e, m) in now {
            if kinds.insert(e, m.kind).is_none() {
                println!("{t:.2}: {e:?} {:?} comes at {:.1}", m.kind, m.pos);
            }
            if i % 15 == 0 && t > 1.5 && t < 17.0 && m.age < m.life {
                looked += 1;
                if !in_view(m.pos + DVec3::Y * 0.6) {
                    missed += 1;
                    println!(
                        "  {t:.2} out of the picture: {e:?} {:?} at {:.1}",
                        m.kind, m.pos
                    );
                }
            }
            if i % 60 == 0 {
                println!(
                    "  {t:.0}s {e:?} {:?} at ({:.1}, {:.1}, {:.1}) target {:?} act {:?}",
                    m.kind, m.pos.x, m.pos.y, m.pos.z, m.target, m.act
                );
            }
        }
    }
    let count = |k: summon::MinionKind| kinds.values().filter(|v| **v == k).count();
    let seen = app.world.resource::<Seen>();
    let demons_ids: Vec<Entity> = kinds
        .iter()
        .filter(|(_, k)| k.demon())
        .map(|(e, _)| *e)
        .collect();
    let demon_blows = seen
        .damage
        .iter()
        .filter(|d| d.source.is_some_and(|s| demons_ids.contains(&s)))
        .count();
    let on_gate = gate_seen.map_or(0, |(ge, ..)| {
        seen.damage.iter().filter(|d| d.target == ge).count()
    });
    let mut by: std::collections::BTreeMap<String, f32> = Default::default();
    for h in seen.hits.iter().filter(|h| h.damage > 0.0) {
        let tk = h
            .target
            .and_then(|t| kinds.get(&t))
            .map(|k| format!("{k:?}"))
            .unwrap_or("-".into());
        let ck = h
            .caster
            .and_then(|c| kinds.get(&c))
            .map(|k| format!("{k:?}"))
            .unwrap_or(format!("{:?}", h.caster));
        *by.entry(format!("{tk} <- {} ({ck})", h.spell.name))
            .or_default() += h.damage;
    }
    for (k, v) in &by {
        println!("  {k}: {v:.0}");
    }
    let mut taken: std::collections::BTreeMap<String, f32> = Default::default();
    for d in &seen.damage {
        let Some(tk) = kinds.get(&d.target) else {
            continue;
        };
        let src = d
            .source
            .map(|s| {
                kinds
                    .get(&s)
                    .map(|k| format!("{k:?}"))
                    .unwrap_or(format!("{s:?}"))
            })
            .unwrap_or("-".into());
        *taken
            .entry(format!("{tk:?} {:?} <- {src} {:?}", d.target, d.kind))
            .or_default() += d.amount;
    }
    for (k, v) in &taken {
        println!("  dmg {k}: {v:.0}");
    }
    println!("out of the picture {missed} of {looked} looks");
    println!(
        "hounds {}, imps {}, gate gone {gate_gone:?}, blows on the gate {on_gate}, bound {bound:?}, \
         hooked {hooked:?}, most bonded {most_bonded}, most chains {most_chains}, demon blows \
         {demon_blows}, lowest master {lowest}",
        count(summon::MinionKind::Hellhound),
        count(summon::MinionKind::Imp),
    );
    assert!(gate_seen.is_some(), "no gate opened");
    assert_eq!(count(summon::MinionKind::Hellhound), 2);
    assert!(
        count(summon::MinionKind::Imp) >= 6,
        "the gate's imps and two meteors' worth"
    );
    assert!(
        gate_gone.is_some_and(|t| t < 19.0),
        "the students broke the gate: {gate_gone:?}"
    );
    assert!(bound.len() >= 2, "{bound:?}");
    // The chains hold the students' creatures, never the warlocks' own demons.
    assert!(
        bound.iter().all(|b| matches!(
            kinds.get(&b.1),
            Some(summon::MinionKind::Golem | summon::MinionKind::Elemental)
        )),
        "{bound:?}"
    );
    assert!(
        bound
            .iter()
            .filter(|b| kinds.get(&b.1) == Some(&summon::MinionKind::Golem))
            .count()
            >= 2
    );
    assert!(
        hooked
            .iter()
            .any(|h| kinds.get(&h.1) == Some(&summon::MinionKind::Elemental)),
        "{hooked:?}"
    );
    assert!(most_bonded >= 3, "{most_bonded}");
    assert!(demon_blows >= 10, "{demon_blows}");
    assert!(pounces >= 1, "no hound pounced");
    assert!(
        missed * 10 < looked,
        "the fight left the picture {missed} of {looked} looks"
    );
    assert!(
        lowest > 1000.0,
        "a master on a plinth nearly fell: {lowest}"
    );
    // At the end of the period what is left of the pact has fallen.
    let left: Vec<Entity> = minions(&mut app)
        .iter()
        .filter(|m| m.1.kind.demon())
        .map(|m| m.0)
        .collect();
    run(&mut app, 1.5);
    let now = minions(&mut app);
    assert!(
        left.iter().all(|e| !now.iter().any(|m| m.0 == *e)),
        "{left:?} of {:?}",
        now.iter().map(|m| (m.0, m.1.kind)).collect::<Vec<_>>()
    );
}

// ------------------------------------------------------------------------------- the wilds

fn tangles(app: &mut App) -> Vec<(Entity, wilds::Tangle)> {
    let mut q = app.world.query::<(Entity, &wilds::Tangle)>();
    let mut v: Vec<_> = q.iter(&app.world).map(|(e, t)| (e, t.clone())).collect();
    v.sort_by_key(|t| t.0);
    v
}

fn wild_roots(app: &mut App) -> Vec<(Entity, wilds::Root)> {
    let mut q = app.world.query::<(Entity, &wilds::Root)>();
    q.iter(&app.world).map(|(e, r)| (e, r.clone())).collect()
}

fn rooted(app: &App, e: Entity) -> bool {
    app.world.get::<Statuses>(e).is_some_and(|s| s.rooted)
}

fn held(app: &mut App, e: Entity) -> bool {
    tangles(app)
        .iter()
        .any(|t| t.1.target == e && t.1.done.is_none())
}

fn wild_app() -> App {
    let mut a = app();
    a.world
        .init_resource::<engine_assets::Assets<engine_render::Mesh>>();
    a.world
        .init_resource::<engine_assets::Assets<engine_render::Image>>();
    a
}

#[test]
fn roots_break_out_of_the_floor_coil_up_a_dummy_and_hold_it_until_they_slide_back() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -8.0));
    run(&mut app, 0.1);
    let entangle = SpellDesign::new(&["wilds"], CastType::Area).named("Entangle");
    cast(&mut app, me, entangle, Aim::Entity(d));
    run(&mut app, 1.6);
    let found = tangles(&mut app);
    assert_eq!(found.len(), 1, "one tangle");
    let (_, tg) = &found[0];
    assert_eq!(tg.target, d);
    assert_eq!(tg.roots.len(), 5, "two up each leg and one round both");
    assert!(tg.secs >= 2.0 && tg.secs <= 5.0, "{}", tg.secs);
    assert!(rooted(&app, d));
    for r in &tg.roots {
        let root = app.world.get::<wilds::Root>(*r).unwrap().clone();
        // It starts under the floor beside the dummy, is all the way out now, and reaches up.
        assert!(root.pts[0].y < 0.0, "{}", root.pts[0]);
        let from = root.pts[0].with_y(0.0).length();
        assert!(from > 0.3 && from < 1.6, "{from}");
        assert!((root.length_now() - root.total()).abs() < 0.01);
        let top = root.pts.iter().map(|p| p.y).fold(0.0, f32::max);
        assert!(top > 0.45, "{top}");
        // Its segments are drawn.
        let shown = root
            .segs
            .iter()
            .filter(|s| {
                app.world
                    .get::<Transform>(**s)
                    .is_some_and(|t| t.scale.min_element() > 0.01)
            })
            .count();
        assert!(
            shown * 10 >= root.segs.len() * 9,
            "{shown} of {}",
            root.segs.len()
        );
    }
    // Free roots writhe up across the area too.
    let mut q = app.world.query::<&wilds::Patch>();
    let patch = q.iter(&app.world).next().expect("a patch of roots").clone();
    assert!(patch.roots.len() >= 6, "{}", patch.roots.len());
    // A little damage as they bite.
    let seen = app.world.resource::<Seen>();
    assert!(seen.damage.iter().any(|x| x.target == d && x.amount > 0.0));
    // Then they let go and slide back into the floor, and the dummy is free.
    run(&mut app, 5.0);
    assert!(tangles(&mut app).is_empty());
    assert!(wild_roots(&mut app).is_empty());
    assert!(!rooted(&app, d));
}

#[test]
fn fire_burns_roots_off_a_held_body_at_once() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -8.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["wilds"], CastType::Area),
        Aim::Entity(d),
    );
    run(&mut app, 1.4);
    assert!(rooted(&app, d));
    let roots = tangles(&mut app)[0].1.roots.clone();
    cast(
        &mut app,
        me,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Entity(d),
    );
    run(&mut app, 0.9);
    assert!(!rooted(&app, d), "fire frees it");
    assert!(!held(&mut app, d));
    let burnt = roots
        .iter()
        .filter(|r| {
            app.world
                .get::<wilds::Root>(**r)
                .is_none_or(|r| r.burnt.is_some())
        })
        .count();
    assert_eq!(burnt, roots.len(), "every root caught");
    run(&mut app, 1.5);
    assert!(
        roots
            .iter()
            .all(|r| app.world.get::<wilds::Root>(*r).is_none())
    );
}

#[test]
fn a_seed_bursts_into_roots_that_hold_what_it_hit_and_those_beside_it() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -10.0));
    let near = dummy(&mut app, DVec3::new(1.3, 0.0, -10.0));
    let far = dummy(&mut app, DVec3::new(5.0, 0.0, -10.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["wilds"], CastType::Bolt).named("Thornvine"),
        Aim::Entity(d),
    );
    run(&mut app, 1.8);
    assert!(rooted(&app, d) && held(&mut app, d));
    assert!(
        rooted(&app, near) && held(&mut app, near),
        "the seed's roots reach 2 m"
    );
    assert!(!rooted(&app, far) && !held(&mut app, far));
}

#[test]
fn a_thorn_hedge_grows_along_a_wall_and_snags_and_scratches_what_pushes_through() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -8.0));
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["wilds"], CastType::Wall).named("Thorn wall"),
        Aim::Point(DVec3::new(0.0, 0.0, -8.0)),
    );
    run(&mut app, 0.6);
    let mut q = app.world.query::<&wilds::Hedge>();
    let hedge = q.iter(&app.world).next().expect("a hedge").clone();
    assert!(hedge.roots.len() >= 10, "{}", hedge.roots.len());
    // Brambles along the wall's line, about head high, with no wall look of its own.
    let mut tall = 0.0f32;
    let mut wide = 0.0f32;
    for r in &hedge.roots {
        let root = app.world.get::<wilds::Root>(*r).unwrap();
        for p in &root.pts {
            tall = tall.max(p.y);
            wide = wide.max(p.x.abs().max(p.z.abs()));
        }
    }
    assert!(tall > 1.4 && tall < 2.6, "{tall}");
    assert!(wide > 2.0, "{wide}");
    // Whoever stands in it is snagged most of the time and scratched each tick.
    let mut snagged = 0;
    for _ in 0..60 {
        run(&mut app, 1.0 / 30.0);
        if rooted(&app, d) {
            snagged += 1;
        }
    }
    assert!(snagged > 25 && snagged < 60, "{snagged} of 60");
    let seen = app.world.resource::<Seen>();
    let scratches: Vec<f32> = seen
        .damage
        .iter()
        .filter(|x| x.target == d)
        .map(|x| x.amount)
        .collect();
    assert!(scratches.len() >= 3, "{scratches:?}");
    assert!(
        scratches.iter().all(|a| *a > 0.0 && *a < 25.0),
        "{scratches:?}"
    );
    // When the wall's time is up the hedge goes with it.
    run(&mut app, 30.0);
    let mut q = app.world.query::<&wilds::Hedge>();
    assert_eq!(q.iter(&app.world).count(), 0);
    assert!(wild_roots(&mut app).is_empty());
}

#[test]
fn bark_skin_grows_over_its_caster_and_takes_two_fifths_off_magic() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    app.world.entity_mut(me).insert(Body(0.5));
    let plain = dummy(&mut app, DVec3::new(0.0, 0.0, -20.0));
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, -10.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 2;
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["wilds"], CastType::OnSelf).named("Bark skin"),
        Aim::Entity(me),
    );
    run(&mut app, 1.5);
    let mut q = app.world.query::<&wilds::BarkSkin>();
    let skin = q.iter(&app.world).next().expect("bark").clone();
    assert_eq!(skin.target, me);
    assert!(skin.plates.len() >= 20, "{}", skin.plates.len());
    assert!(skin.secs >= 15.0, "{}", skin.secs);
    let grown = skin
        .plates
        .iter()
        .filter(|p| {
            app.world
                .get::<Transform>(p.0)
                .is_some_and(|t| t.scale.min_element() > 0.9)
        })
        .count();
    assert_eq!(grown, skin.plates.len(), "all grown in");
    // The same frost bolt does 40% less to it than to a plain body.
    let bolt = SpellDesign::new(&["frost"], CastType::Bolt);
    cast(&mut app, foe, bolt.clone(), Aim::Entity(me));
    run(&mut app, 1.5);
    cast(&mut app, foe, bolt, Aim::Entity(plain));
    run(&mut app, 1.5);
    let seen = app.world.resource::<Seen>();
    let on = |e: Entity| -> f32 {
        seen.damage
            .iter()
            .filter(|x| x.target == e && x.source == Some(foe))
            .map(|x| x.amount)
            .sum()
    };
    let (barked, bare) = (on(me), on(plain));
    assert!(bare > 0.0);
    assert!(
        (barked / bare - (1.0 - wilds::BARK_SHARE)).abs() < 0.05,
        "{barked} of {bare}"
    );
    // When its time is up it cracks off and falls.
    let plates: Vec<Entity> = skin.plates.iter().map(|p| p.0).collect();
    run(&mut app, 20.0);
    let mut q = app.world.query::<&wilds::BarkSkin>();
    assert_eq!(q.iter(&app.world).count(), 0);
    assert!(plates.iter().all(|p| app.world.get_entity(*p).is_err()));
}

#[test]
fn a_summoned_treant_walks_to_a_dummy_and_its_slam_roots_it() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::new(0.0, 0.0, 6.0), Mastery::Adept);
    let d = dummy(&mut app, DVec3::new(0.0, 0.0, -4.0));
    run(&mut app, 0.1);
    let treant = SpellDesign::new(&["wilds"], CastType::Summon)
        .with(Modifier::Duration { factor: 0.4 })
        .named("Treant");
    cast(&mut app, me, treant, Aim::Point(DVec3::ZERO));
    run(&mut app, 1.0);
    let found = minions(&mut app);
    assert_eq!(found.len(), 1);
    let (t, m) = found[0].clone();
    assert_eq!(m.kind, summon::MinionKind::Treant);
    assert!(
        summon::body_height(m.kind, m.size) > 3.3,
        "{}",
        summon::body_height(m.kind, m.size)
    );
    let mut rooted_by_it = false;
    for _ in 0..24 {
        run(&mut app, 0.5);
        rooted_by_it |= held(&mut app, d);
    }
    let seen = app.world.resource::<Seen>();
    let blows: Vec<_> = seen.damage.iter().filter(|x| x.source == Some(t)).collect();
    assert!(blows.len() >= 3, "{}", blows.len());
    assert!(blows.iter().all(|b| b.target == d));
    assert!(rooted_by_it, "its slam roots what it strikes");
}

#[test]
fn fire_through_a_thorn_hedge_runs_along_it_and_burns_it_away() {
    let mut app = wild_app();
    let me = mage(&mut app, DVec3::ZERO, Mastery::Expert);
    let foe = mage(&mut app, DVec3::new(0.0, 0.0, -16.0), Mastery::Expert);
    app.world.get_mut::<MagicCaster>(foe).unwrap().side = 2;
    run(&mut app, 0.1);
    cast(
        &mut app,
        me,
        SpellDesign::new(&["wilds"], CastType::Wall),
        Aim::Point(DVec3::new(0.0, 0.0, -8.0)),
    );
    run(&mut app, 1.0);
    let mut q = app.world.query::<&wilds::Hedge>();
    let hedge = q.iter(&app.world).next().expect("a hedge").clone();
    cast(
        &mut app,
        foe,
        SpellDesign::new(&["fire"], CastType::Bolt),
        Aim::Point(DVec3::new(1.0, 1.0, 0.0)),
    );
    run(&mut app, 0.8);
    // Lit where it went in, the fire running out along it.
    let burning = hedge
        .roots
        .iter()
        .filter(|r| {
            app.world
                .get::<wilds::Root>(**r)
                .is_none_or(|r| r.burnt.is_some_and(|b| b > 0.0))
        })
        .count();
    let lit = hedge
        .roots
        .iter()
        .filter(|r| {
            app.world
                .get::<wilds::Root>(**r)
                .is_none_or(|r| r.burnt.is_some())
        })
        .count();
    assert_eq!(lit, hedge.roots.len(), "the whole hedge caught");
    assert!(burning > 0 && burning <= lit, "{burning} of {lit}");
    run(&mut app, 3.0);
    let mut q = app.world.query::<&wilds::Hedge>();
    assert_eq!(q.iter(&app.world).count(), 0);
    assert!(wild_roots(&mut app).is_empty());
}

#[test]
fn the_wilds_hall_holds_the_dead_in_roots_and_thorns_and_a_treant_smashes_them() {
    let mut app = blade_app();
    app.world
        .init_resource::<engine_assets::Assets<engine_render::Image>>();
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app.update();
    {
        let mut hall = app.world.resource_mut::<demo::Hall>();
        hall.program = demo::Program::Wilds;
        hall.restart = true;
    }
    let view = demo::VIEWS
        .iter()
        .find(|v| v.key == "spells_wilds")
        .unwrap();
    let eye = DVec3::from(view.eye);
    let fwd = (DVec3::from(view.target) - eye).normalize();
    let right = fwd.cross(DVec3::Y).normalize();
    let up = right.cross(fwd);
    let tan_y = (55f64.to_radians() / 2.0).tan();
    let in_view = |p: DVec3| {
        let d = p - eye;
        let z = d.dot(fwd);
        z > 0.5
            && (d.dot(right) / (z * tan_y * 16.0 / 9.0)).abs() < 0.9
            && (d.dot(up) / (z * tan_y)).abs() < 0.9
    };
    let mut kinds: std::collections::HashMap<Entity, summon::MinionKind> = Default::default();
    let mut held: Vec<(f32, Entity)> = Vec::new();
    let mut hedge_at: Option<(f32, DVec3)> = None;
    let mut hedge_burnt: Option<f32> = None;
    let mut barked: Option<f32> = None;
    let mut treant: Option<Entity> = None;
    let mut treant_blows = 0usize;
    let mut snags = 0usize;
    let mut fell: Vec<Entity> = Vec::new();
    let (mut looked, mut missed) = (0usize, 0usize);
    let steps = (demo::wilds::PERIOD * 60.0) as usize;
    for i in 0..steps {
        app.update();
        let t = i as f32 / 60.0;
        for (_, tg) in tangles(&mut app) {
            if tg.done.is_none() && !held.iter().any(|(at, e)| *e == tg.target && t - at < 3.0) {
                println!(
                    "{t:.2}: held {:?} {:?} {:?}",
                    tg.target,
                    kinds.get(&tg.target),
                    app.world.get::<Name>(tg.target)
                );
                held.push((t, tg.target));
            }
        }
        let mut q = app.world.query::<(Entity, &wilds::Hedge)>();
        let hedges: Vec<(Entity, wilds::Hedge)> =
            q.iter(&app.world).map(|(e, h)| (e, h.clone())).collect();
        for (e, h) in &hedges {
            if hedge_at.is_none() {
                let at = app.world.get::<Transform>(*e).unwrap().translation;
                println!("{t:.2}: hedge at {at:.1}, {} brambles", h.roots.len());
                assert!(in_view(at + DVec3::Y), "the hedge is out of the picture");
                hedge_at = Some((t, at));
            }
            if hedge_burnt.is_none()
                && h.roots.iter().any(|r| {
                    app.world
                        .get::<wilds::Root>(*r)
                        .is_some_and(|r| r.burnt.is_some())
                })
            {
                println!("{t:.2}: the hedge burns");
                hedge_burnt = Some(t);
            }
        }
        let mut q = app.world.query::<&wilds::BarkSkin>();
        if barked.is_none() && q.iter(&app.world).next().is_some() {
            println!("{t:.2}: bark");
            barked = Some(t);
        }
        let now = minions(&mut app);
        for (e, k) in &kinds {
            if !now.iter().any(|m| m.0 == *e) && !fell.contains(e) {
                println!("{t:.2}: {e:?} {k:?} falls");
                fell.push(*e);
            }
        }
        for (e, m) in &now {
            if kinds.insert(*e, m.kind).is_none() {
                println!("{t:.2}: {e:?} {:?} comes at {:.1}", m.kind, m.pos);
                if m.kind == summon::MinionKind::Treant {
                    treant = Some(*e);
                }
            }
            if i % 15 == 0 && t > 2.5 && t < 16.0 && m.age < m.life {
                looked += 1;
                if !in_view(m.pos + DVec3::Y * 0.6) {
                    missed += 1;
                    println!(
                        "  {t:.2}: {e:?} {:?} out of the picture at {:.1}",
                        m.kind, m.pos
                    );
                }
            }
            if i % 6 == 0
                && m.kind == summon::MinionKind::Skeleton
                && hedge_at.is_some_and(|(_, at)| {
                    DVec3::new(m.pos.x - at.x, 0.0, m.pos.z - at.z).length() < 4.5
                })
                && app.world.get::<Statuses>(*e).is_some_and(|s| s.rooted)
            {
                snags += 1;
            }
        }
    }
    if let Some(tr) = treant {
        let seen = app.world.resource::<Seen>();
        treant_blows = seen.damage.iter().filter(|x| x.source == Some(tr)).count();
    }
    println!(
        "held {} bodies, {snags} snag samples, treant blows {treant_blows}, missed {missed} of {looked}",
        held.len()
    );
    assert!(barked.is_some_and(|t| t < 1.5), "no bark");
    assert!(hedge_at.is_some_and(|h| h.0 < 2.5), "no hedge");
    assert!(
        hedge_burnt.is_some_and(|t| t > 10.0 && t < 13.0),
        "{hedge_burnt:?}"
    );
    assert!(treant.is_some(), "no treant");
    assert!(treant_blows >= 4, "{treant_blows}");
    let held_dead = held
        .iter()
        .filter(|(_, e)| kinds.get(e) == Some(&summon::MinionKind::Skeleton))
        .count();
    assert!(held_dead >= 6, "{held_dead}");
    assert!(
        !held
            .iter()
            .any(|(_, e)| kinds.get(e) == Some(&summon::MinionKind::Treant)),
        "the druids rooted their own treant"
    );
    assert!(snags >= 10, "{snags}");
    assert!(
        missed * 10 < looked,
        "the fight left the picture {missed} of {looked} looks"
    );
}
