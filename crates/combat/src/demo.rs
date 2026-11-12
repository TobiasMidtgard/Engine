//! The combat yard: pairs of fighters (bodies the animation module builds and swings, on
//! `BodyRequest` + `Loadout`) trading aimed strikes so the rules can be watched part by part.
//! Each pair's attacker presses strikes at a list of body parts; the yard carries every body
//! by its strikes' steps and pushes (as a game's mover does), lays out a body whose head came
//! off or whose leg gave out, and starts a pair over when its list is done.
//!
//! | Pair | Shows |
//! |---|---|
//! | `aim` | a sword master at a shield fighter: head, neck, arm, knee, chest stab, thigh, wrist, belly; the shield fighter covers its head and neck |
//! | `hamstring` | cuts at the backs of the knees of a fighter turned away: the legs give out |
//! | `sever` | a great-sword master at a beginner: the sword arm until it drops the weapon, then the neck |
//! | `rookie` | a beginner at a master: blows land beside their part or are caught on the arm |
//! | `look` | an attacker aiming where the camera looks (`LookAim`): the part under the screen centre is outlined and struck |

use engine_core::prelude::*;
use engine_world::animation::{BodyRequest, Ragdoll, StrikeTargets};
use engine_world::combat::{
    AttackWindow, BodyInjuries, BodyZone, CombatSkills, ItemClass, Knockback, Loadout,
};
use engine_world::events::AttackKind;
use engine_world::locomotion::MoveIntent;

use crate::CombatSettings;
use crate::look::LookAim;
use crate::rules::Armor;

/// A tour stop of the yard: `eye` and `target` relative to the yard's middle.
#[derive(Debug, Clone, Copy)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup: f32,
}

/// Where each pair stands, along X from the yard's middle (m).
pub const PAIRS: [(&str, f64); 5] = [
    ("look", -8.0),
    ("aim", -4.0),
    ("hamstring", 0.0),
    ("sever", 4.0),
    ("rookie", 8.0),
];

/// The tour's stops, one per pair (side views at chest height, 1440p framing).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "combat_look",
        name: "Combat: the strike goes where you look (the outlined part)",
        eye: [-9.3, 1.55, 2.3],
        target: [-7.4, 0.9, 0.0],
        hour: 15.0,
        warmup: 6.0,
    },
    DemoView {
        key: "combat_aim",
        name: "Combat: aimed strikes at every part; the shield fighter covers head and neck",
        eye: [-3.6, 1.35, 3.6],
        target: [-4.0, 1.0, 0.0],
        hour: 15.0,
        warmup: 14.0,
    },
    DemoView {
        key: "combat_hamstring",
        name: "Combat: a hamstring cut from behind, the leg gives out",
        eye: [0.6, 1.3, 3.9],
        target: [0.15, 0.75, 0.0],
        hour: 15.0,
        warmup: 6.0,
    },
    DemoView {
        key: "combat_sever",
        name: "Combat: a great sword takes the arm, then the head",
        eye: [4.4, 1.4, 3.6],
        target: [4.0, 1.0, 0.0],
        hour: 15.0,
        warmup: 10.0,
    },
    DemoView {
        key: "combat_rookie",
        name: "Combat: a beginner against a master: blows land beside their part or on the arm",
        eye: [8.4, 1.4, 3.6],
        target: [8.0, 1.0, 0.0],
        hour: 15.0,
        warmup: 10.0,
    },
];

/// A yard body: where it stands, which way it faces, what it starts with.
#[derive(Component, Debug, Clone, Copy)]
pub struct YardBody {
    pub home: DVec3,
    pub yaw_deg: f32,
    pub kit: Loadout,
    /// World time it was laid out (head off), s.
    pub down_since: Option<f64>,
}

/// An attacker's list: strike `zones` in turn every `every_secs`, heavy where `heavy` says,
/// then wait `rest_secs` and start the pair over.
#[derive(Component, Debug, Clone)]
pub struct Sparring {
    pub foe: Entity,
    pub zones: Vec<(BodyZone, bool)>,
    pub every_secs: f64,
    pub rest_secs: f64,
    pub next_at: f64,
    pub i: usize,
}

fn skills(class: ItemClass, s: f32) -> CombatSkills {
    CombatSkills {
        weapons: [(class, s), (ItemClass::Shield, s)].into_iter().collect(),
        off_hand: s * 0.7,
    }
}

fn kit(main: ItemClass, off: ItemClass) -> Loadout {
    Loadout {
        main,
        off,
        two_handed: main.needs_both_hands(),
    }
}

/// Spawn the yard around `origin` (the zone's middle, on its pad).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    use BodyZone::*;
    type Pair = (
        &'static str,
        (Loadout, f32),
        (Loadout, f32, f32),
        bool,
        Vec<(BodyZone, bool)>,
        f64,
    );
    let sword = ItemClass::OneHandedBlade;
    let pairs: Vec<Pair> = vec![
        (
            "look",
            (kit(sword, ItemClass::None), 0.9),
            (kit(sword, ItemClass::None), 0.4, 0.0),
            false,
            vec![(Chest, false)],
            2.0,
        ),
        (
            "aim",
            (kit(sword, ItemClass::None), 0.9),
            (kit(sword, ItemClass::Shield), 0.6, 0.0),
            false,
            vec![
                (Head, false),
                (Neck, false),
                (UpperArmL, false),
                (KneeL, false),
                (Chest, false),
                (ThighR, false),
                (ForearmR, false),
                (Belly, false),
            ],
            1.7,
        ),
        (
            "hamstring",
            (kit(sword, ItemClass::None), 0.95),
            (kit(sword, ItemClass::None), 0.3, 0.0),
            true,
            vec![(KneeR, false), (KneeR, false)],
            1.8,
        ),
        (
            "sever",
            (kit(ItemClass::TwoHandedBlade, ItemClass::None), 1.0),
            (kit(sword, ItemClass::None), 0.15, 0.0),
            false,
            vec![
                (ForearmR, false),
                (ForearmR, true),
                (ForearmR, true),
                (Neck, true),
            ],
            2.2,
        ),
        (
            "rookie",
            (kit(sword, ItemClass::None), 0.1),
            (kit(sword, ItemClass::None), 1.0, 0.3),
            false,
            vec![(Head, false), (Neck, false), (Chest, false), (KneeR, false)],
            1.8,
        ),
    ];
    for (key, (akit, askill), (dkit, dskill, armor), away, zones, every) in pairs {
        let x = PAIRS.iter().find(|p| p.0 == key).map_or(0.0, |p| p.1);
        let mid = origin + DVec3::new(x, 0.0, 0.0);
        let seed = 7100 + (x as i64 + 20) as u64 * 3;
        let body = |commands: &mut Commands,
                    at: DVec3,
                    yaw: f32,
                    seed: u64,
                    loadout: Loadout,
                    skill: f32,
                    armor: f32| {
            let t = Transform::from_translation(at)
                .with_rotation(Quat::from_rotation_y(yaw.to_radians()));
            commands
                .spawn((
                    t,
                    GlobalTransform::from(t),
                    BodyRequest::human(seed),
                    loadout,
                    skills(loadout.main, skill),
                    MoveIntent::default(),
                    StrikeTargets::default(),
                    Armor::all(armor),
                    BodyInjuries::default(),
                    Ragdoll { weight: 0.0 },
                    YardBody {
                        home: at,
                        yaw_deg: yaw,
                        kit: loadout,
                        down_since: None,
                    },
                ))
                .id()
        };
        let a = body(
            commands,
            mid + DVec3::new(-0.75, 0.0, 0.0),
            -90.0,
            seed,
            akit,
            askill,
            0.0,
        );
        let d = body(
            commands,
            mid + DVec3::new(0.75, 0.0, 0.0),
            if away { -90.0 } else { 90.0 },
            seed + 1,
            dkit,
            dskill,
            armor,
        );
        commands.entity(a).insert((
            StrikeTargets {
                target: Some(d),
                foes: vec![d],
                ..Default::default()
            },
            Sparring {
                foe: d,
                zones,
                every_secs: every,
                rest_secs: 3.0,
                next_at: 2.0,
                i: 0,
            },
        ));
        if key == "look" {
            commands.entity(a).insert(LookAim::default());
        }
        if !away {
            commands.entity(d).insert(StrikeTargets {
                target: Some(a),
                foes: vec![a],
                ..Default::default()
            });
        }
    }
}

/// Press each attacker's next strike when it is due (Update, in `UpdateSet::Logic`, before the
/// animation module reads intent).
pub(crate) fn spar(
    time: Res<Time>,
    mut attackers: Query<(
        &mut Sparring,
        &mut MoveIntent,
        &mut StrikeTargets,
        Has<LookAim>,
    )>,
    mut bodies: Query<(&YardBody, &mut BodyInjuries, &mut Loadout), Without<Sparring>>,
    mut own: Query<(&YardBody, &mut BodyInjuries, &mut Loadout), With<Sparring>>,
) {
    let now = time.elapsed_secs_f64();
    for (mut s, mut intent, mut strike, looks) in &mut attackers {
        if intent.attack.is_some() {
            intent.attack = None;
        }
        if now < s.next_at {
            continue;
        }
        if s.i >= s.zones.len() {
            // The list is done: start the pair over, wounds healed and weapons back.
            if let Ok((y, mut hurt, mut l)) = bodies.get_mut(s.foe) {
                *hurt = BodyInjuries::default();
                *l = y.kit;
            }
            s.i = 0;
            continue;
        }
        let (zone, heavy) = s.zones[s.i];
        if !looks {
            strike.zone = Some(zone);
        }
        intent.attack = Some(if heavy {
            AttackKind::Dash
        } else {
            AttackKind::Melee
        });
        s.i += 1;
        s.next_at = now
            + if s.i >= s.zones.len() {
                s.every_secs + s.rest_secs
            } else {
                s.every_secs
            };
    }
    // Attackers never stay hurt (the yard only judges what they do).
    for (y, mut hurt, mut l) in &mut own {
        if hurt.zones.iter().any(|z| z.wound > 0.0) {
            *hurt = BodyInjuries::default();
        }
        if *l != y.kit {
            *l = y.kit;
        }
    }
}

/// Carry every yard body by its strike's step and the blows' pushes, drawing it back to its
/// mark; lay out a body whose neck was cut through, and stand it up again 4 s later.
#[allow(clippy::type_complexity)]
pub(crate) fn carry(
    time: Res<Time>,
    mut q: Query<(
        &mut YardBody,
        &mut Transform,
        Option<&AttackWindow>,
        Option<&Knockback>,
        &mut BodyInjuries,
        &mut Ragdoll,
        &mut Loadout,
    )>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    for (mut y, mut t, window, push, mut hurt, mut ragdoll, mut loadout) in &mut q {
        // Down: the head off, or a leg given out (the yard plays what a game would do
        // with those wounds: lay the body out, and stand it up again for the next round).
        let legs = [BodyZone::KneeL, BodyZone::KneeR]
            .iter()
            .any(|k| hurt.get(*k).wound >= crate::rules::LEG_GIVES_OUT);
        let headless = hurt.get(BodyZone::Neck).severed || legs;
        match (headless, y.down_since) {
            (true, None) => {
                y.down_since = Some(now);
                ragdoll.weight = 1.0;
            }
            (_, Some(since)) if now - since > 4.0 => {
                y.down_since = None;
                ragdoll.weight = 0.0;
                *hurt = BodyInjuries::default();
                *loadout = y.kit;
                *t = Transform::from_translation(y.home)
                    .with_rotation(Quat::from_rotation_y(y.yaw_deg.to_radians()));
                continue;
            }
            _ => {}
        }
        if ragdoll.weight > 0.5 {
            continue;
        }
        let mut step = window.map_or(Vec3::ZERO, |w| w.warp_delta);
        step += push.map_or(Vec3::ZERO, |k| k.velocity) * dt;
        let back = (y.home - t.translation).with_y(0.0).as_vec3() * (1.5 * dt).min(1.0);
        let next = t.translation + (step.with_y(0.0) + back).as_dvec3();
        if next != t.translation {
            t.translation = next;
        }
    }
}

pub(crate) fn register(app: &mut App) {
    app.add_systems(Update, (spar, carry).chain().in_set(UpdateSet::Logic));
    app.register_api(
        "combat.demo",
        "Spawn the combat yard (five pairs of fighters trading aimed strikes) around a point, \
         and turn the wound view on.",
        r#"{"at": [0, 0, 0]}"#,
        |world, args| {
            let at = args
                .get("at")
                .and_then(|v| v.as_array())
                .map(|a| {
                    let f = |i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0);
                    DVec3::new(f(0), f(1), f(2))
                })
                .unwrap_or(DVec3::ZERO);
            let mut queue = bevy_ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            spawn_demo(&mut commands, at);
            queue.apply(world);
            world.resource_mut::<CombatSettings>().show_wounds = true;
            Ok(serde_json::json!({ "pairs": PAIRS.len() }))
        },
    );
}
