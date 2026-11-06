//! The Weaving hall: the magic module's showcase zone (round-1-plan.md, "Showcase").
//!
//! Six mages on plinths cast down six lanes at practice dummies, crates and barrels. One
//! program runs at a time, switched by `magic.demo` (the tour calls it on arriving at each
//! stop):
//!
//! | Program | What it shows |
//! |---|---|
//! | `spells_mastery` | the same Fire bolt grown from Novice to Ascended |
//! | `spells_elements` | the six elements and what they leave: burning, freezing, shock, bleeding, rot, dread |
//! | `spells_combos` | woven elements: Steam, Plasma, Tempest, Plague, Nightfeed, Wither (then Hellfire, Blood lightning, Aether...) |
//! | `spells_forms` | cast types: bolt, lob, beam, cone, chain, area (then aura, rune, orbit, rain, wave, wall) |
//! | `spells_meet` | spells that meet: bolts through walls of other elements |
//! | `spells_force` | force vectors: push, pull, lift, slam, thunderclap, shardstorm |
//! | `spells_dark` | the dark subclasses: blood, rot clouds, shadow orbs, a draining beam, plague, doom |
//! | `spells_duel` | six Adept students against three Ascended masters, chasing reactions and moving between casts ([`duel`]) |
//! | `spells_shaping` | stone that is really there: a wall bolts burst on, pillars lifting a dummy and a crate stack, a stone grip, steps, a frost wall beside a stone one |
//!
//! The mages wear hooded robes coloured by mastery (the character contract's `Appearance`).
//!
//! The mages only cast while a camera is within 160 m, so the hall costs nothing elsewhere.

pub mod art;
pub mod duel;

use engine_core::prelude::*;
use engine_render::{Camera, MeshRenderer};
use engine_world::Readouts;
use engine_world::animation::Ragdoll;
use engine_world::character::Appearance;
use serde_json::{Value, json};

use crate::caster::{Aim, CastRequest, KnownSpell, MagicCaster, SpellChoice, Vitality};
use crate::design::{CastType, Modifier, SpellDesign, Trigger};
use crate::growth::Growth;
use crate::hits::SpellHit;
use crate::runtime::{BeamCast, ChainArc, Field, Missile, Rainfall};
use crate::schools::Mastery;
use crate::status::Afflictions;
use art::*;

/// A viewpoint on the hall: eye and target relative to the zone's center, the hour, the
/// program it shows and how long to let it run before a screenshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup: f32,
}

/// The tour's stops; each runs the program named by its key.
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "spells_mastery",
        name: "Mastery ladder: one Fire bolt from Novice to Ascended",
        eye: [0.0, 9.0, 26.0],
        target: [0.0, 0.4, -9.0],
        hour: 17.9,
        warmup: 2.3,
    },
    DemoView {
        key: "spells_elements",
        name: "The six elements and what they leave behind",
        eye: [0.0, 7.0, 23.0],
        target: [0.0, 0.9, -11.0],
        hour: 17.6,
        warmup: 7.4,
    },
    DemoView {
        key: "spells_combos",
        name: "Woven elements: Steam, Plasma, Tempest, Plague, Nightfeed, Wither",
        eye: [-19.0, 5.0, 8.0],
        target: [2.0, 1.2, -6.0],
        // Afternoon: a low golden sun turned the steam cloud orange.
        hour: 15.5,
        warmup: 2.2,
    },
    DemoView {
        key: "spells_forms",
        name: "Cast types: bolt, lob, beam, cone, chain, area",
        eye: [21.0, 6.5, 4.0],
        target: [-2.0, 1.0, -4.0],
        hour: 18.1,
        warmup: 1.9,
    },
    DemoView {
        key: "spells_meet",
        name: "Spells that meet: bolts through walls of other elements",
        eye: [16.0, 4.0, 8.0],
        target: [-3.0, 1.3, -2.0],
        hour: 17.9,
        warmup: 1.75,
    },
    DemoView {
        key: "spells_force",
        name: "Force: push, pull, lift, slam, thunderclap, shardstorm",
        eye: [17.0, 4.2, 9.0],
        target: [-2.0, 1.0, -4.0],
        hour: 16.8,
        warmup: 1.7,
    },
    DemoView {
        key: "spells_dark",
        name: "Dark magic: blood, rot, shadow orbs, a draining beam, plague, doom",
        eye: [-16.0, 4.5, 13.0],
        target: [2.0, 1.2, -6.0],
        hour: 17.7,
        warmup: 3.0,
    },
    DemoView {
        key: "spells_duel",
        name: "A duel: six students against three masters, each chasing reactions",
        eye: [17.0, 6.5, -17.0],
        target: [-3.0, 1.4, 3.0],
        hour: 17.6,
        warmup: 6.0,
    },
    DemoView {
        key: "spells_shaping",
        name: "Shaping: a stone wall, pillars that lift, a stone grip, steps, frost beside stone",
        // From behind the dais, down the lanes, so no lane hides another.
        eye: [4.0, 6.5, 19.0],
        target: [0.0, 1.8, -4.0],
        hour: 16.9,
        // Pillars at the top of their climb, every step up, a fire bolt just bursting on
        // each stone wall.
        warmup: 3.1,
    },
];

/// What the hall is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Program {
    #[default]
    Mastery,
    Elements,
    Combos,
    Forms,
    Meet,
    Force,
    Dark,
    Duel,
    Shaping,
}

impl Program {
    pub const ALL: [Program; 9] = [
        Program::Mastery,
        Program::Elements,
        Program::Combos,
        Program::Forms,
        Program::Meet,
        Program::Force,
        Program::Dark,
        Program::Duel,
        Program::Shaping,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Program::Mastery => "spells_mastery",
            Program::Elements => "spells_elements",
            Program::Combos => "spells_combos",
            Program::Forms => "spells_forms",
            Program::Meet => "spells_meet",
            Program::Force => "spells_force",
            Program::Dark => "spells_dark",
            Program::Duel => "spells_duel",
            Program::Shaping => "spells_shaping",
        }
    }

    pub fn from_key(key: &str) -> Option<Program> {
        let key = key.strip_prefix("spells_").unwrap_or(key);
        Program::ALL.into_iter().find(|p| p.key().ends_with(key))
    }

    /// Seconds between a lane's casts.
    fn period(self) -> f32 {
        match self {
            Program::Mastery => 4.0,
            Program::Elements => 1.6,
            Program::Combos | Program::Forms => 3.5,
            Program::Meet => 6.0,
            Program::Force => 5.0,
            Program::Dark => 3.5,
            Program::Duel => 60.0,
            // Every stone is down again before the next period (pillars: 1.8 s up, 4.5 s
            // standing, 1.2 s down), so the props reset onto bare floor.
            Program::Shaping => 7.0,
        }
    }

    fn title(self) -> &'static str {
        VIEWS
            .iter()
            .find(|v| v.key == self.key())
            .map(|v| v.name)
            .unwrap_or("")
    }
}

/// Where a cast is aimed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum AimAt {
    /// The lane's focus target.
    Focus,
    /// The ground this far down the lane (z, from the hall's center).
    Lane(f64),
}

/// One cast in a lane's period.
#[derive(Debug, Clone)]
struct Cast {
    at: f32,
    /// None: the mage's grown grimoire spell.
    design: Option<SpellDesign>,
    aim: AimAt,
}

/// A lane's part in a program.
#[derive(Debug, Clone)]
struct Lane {
    mastery: Mastery,
    level: f32,
    known: Option<KnownSpell>,
    casts: Vec<Cast>,
}

fn d(fx: &[&str], cast: CastType) -> SpellDesign {
    SpellDesign::new(fx, cast)
}

fn lane(design: SpellDesign) -> Lane {
    Lane {
        mastery: Mastery::Ascended1,
        level: 62.0,
        known: None,
        casts: vec![Cast {
            at: 0.0,
            design: Some(design),
            aim: AimAt::Focus,
        }],
    }
}

/// The lanes of a program in its `cycle`th period (some programs alternate two sets).
fn lanes(program: Program, cycle: u64) -> Vec<Lane> {
    let alt = cycle % 2 == 1;
    match program {
        Program::Mastery => {
            let tiers = [
                (Mastery::Novice, 6.0),
                (Mastery::Apprentice, 20.0),
                (Mastery::Adept, 34.0),
                (Mastery::Expert, 48.0),
                (Mastery::Master, 66.0),
                (Mastery::Ascended1, 88.0),
            ];
            tiers
                .into_iter()
                .map(|(m, level)| Lane {
                    mastery: m,
                    level,
                    known: Some(KnownSpell {
                        design: d(&["fire"], CastType::Bolt).named("Fire bolt"),
                        growth: Some(Growth::Volley),
                    }),
                    casts: vec![Cast {
                        at: 0.0,
                        design: None,
                        aim: AimAt::Focus,
                    }],
                })
                .collect()
        }
        Program::Elements => ["fire", "frost", "storm", "blood", "blight", "shadow"]
            .into_iter()
            .map(|e| lane(d(&[e], CastType::Bolt)))
            .collect(),
        Program::Combos => {
            let set: [SpellDesign; 6] = if alt {
                [
                    d(&["blight", "fire"], CastType::Bolt).with(Modifier::Blast { radius_m: 3.0 }),
                    d(&["blood", "storm"], CastType::Chain),
                    d(&["fire", "frost", "storm"], CastType::Bolt),
                    d(&["blight", "blood", "shadow"], CastType::Lob),
                    d(&["blood", "frost"], CastType::Bolt).with(Modifier::Split { count: 3 }),
                    d(&["blight", "storm"], CastType::Rain),
                ]
            } else {
                [
                    d(&["fire", "frost"], CastType::Lob),
                    d(&["fire", "storm"], CastType::Beam),
                    d(&["frost", "storm"], CastType::Area),
                    d(&["blight", "blood"], CastType::Lob),
                    d(&["blood", "shadow"], CastType::Beam)
                        .with(Modifier::Lifesteal { share: 0.3 }),
                    d(&["blight", "shadow"], CastType::Wave),
                ]
            };
            set.into_iter().map(lane).collect()
        }
        Program::Forms => {
            let set: [SpellDesign; 6] = if alt {
                [
                    d(&["fire"], CastType::Aura).with(Modifier::Safeguard),
                    d(&["storm"], CastType::Rune),
                    d(&["frost"], CastType::Orbit),
                    d(&["fire"], CastType::Rain),
                    d(&["blight"], CastType::Wave),
                    d(&["frost"], CastType::Wall),
                ]
            } else {
                [
                    d(&["fire"], CastType::Bolt),
                    d(&["frost"], CastType::Lob),
                    d(&["storm"], CastType::Beam),
                    d(&["fire"], CastType::Cone),
                    d(&["storm"], CastType::Chain),
                    d(&["frost"], CastType::Area),
                ]
            };
            set.into_iter()
                .map(|s| {
                    let mut l = lane(s.clone());
                    // Runes and walls go halfway; cones and auras need the dummy near.
                    l.casts[0].aim = match s.cast {
                        CastType::Rune | CastType::Wall => AimAt::Lane(-6.0),
                        _ => AimAt::Focus,
                    };
                    l
                })
                .collect()
        }
        Program::Meet => {
            let wall = |e: &str| d(&[e], CastType::Wall).with(Modifier::Duration { factor: 0.6 });
            let pair = |w: &str, bolt: &str| Lane {
                mastery: Mastery::Ascended1,
                level: 62.0,
                known: None,
                casts: vec![
                    Cast {
                        at: 0.0,
                        design: Some(wall(w)),
                        aim: AimAt::Lane(-1.0),
                    },
                    Cast {
                        at: 1.2,
                        design: Some(d(&[bolt], CastType::Bolt)),
                        aim: AimAt::Focus,
                    },
                ],
            };
            let mut first = pair("frost", "fire");
            // ...and lightning into the steam it made: Aether.
            first.casts.push(Cast {
                at: 2.6,
                design: Some(d(&["storm"], CastType::Bolt)),
                aim: AimAt::Focus,
            });
            vec![
                first,
                pair("fire", "storm"),
                pair("frost", "storm"),
                pair("blight", "fire"),
                pair("blood", "storm"),
                pair("shadow", "blood"),
            ]
        }
        Program::Force => {
            let area = |fx: &[&str], dur: bool| {
                let s = d(fx, CastType::Area).with(Modifier::Size { factor: 0.6 });
                if dur {
                    s.with(Modifier::Duration { factor: 1.0 })
                } else {
                    s
                }
            };
            let one = |s: SpellDesign| lane(s);
            vec![
                one(d(&["push"], CastType::Bolt)),
                one(d(&["pull"], CastType::Bolt)),
                one(area(&["lift"], true)),
                Lane {
                    casts: vec![
                        Cast {
                            at: 0.0,
                            design: Some(area(&["lift"], false)),
                            aim: AimAt::Focus,
                        },
                        Cast {
                            at: 1.1,
                            design: Some(area(&["slam"], false)),
                            aim: AimAt::Focus,
                        },
                    ],
                    ..lane(SpellDesign::default())
                },
                one(area(&["push", "storm"], false)),
                one(d(&["frost", "push"], CastType::Cone)),
            ]
        }
        Program::Dark => {
            let doom = d(&["blight", "blood", "shadow"], CastType::Bolt).stage(
                Trigger::Kill,
                d(&["shadow"], CastType::Area).with(Modifier::Size { factor: 1.5 }),
            );
            [
                d(&["blood"], CastType::Bolt).with(Modifier::Split { count: 3 }),
                d(&["blight"], CastType::Lob).with(Modifier::Linger {
                    secs: 6.0,
                    radius_m: 2.5,
                }),
                d(&["shadow"], CastType::Orbit),
                d(&["blood", "shadow"], CastType::Beam).with(Modifier::Lifesteal { share: 0.4 }),
                d(&["blight", "blood"], CastType::Lob),
                doom,
            ]
            .into_iter()
            .map(lane)
            .collect()
        }
        Program::Shaping => {
            let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
                at,
                design: Some(design),
                aim,
            };
            let casts = |casts: Vec<Cast>| Lane {
                casts,
                ..lane(SpellDesign::default())
            };
            // Walls a lane wide and down before the next period; pillars and steps likewise.
            // A lane's casts are at least a cast time (0.32 s) apart, or the later is refused.
            let wall = |e: &str| {
                d(&[e], CastType::Wall)
                    .with(Modifier::Size { factor: 0.55 })
                    .with(Modifier::Duration { factor: 0.6 })
            };
            let pillar = |size: f32| {
                d(&["lift", "stone"], CastType::Area)
                    .with(Modifier::Duration { factor: 0.375 })
                    .with(Modifier::Size { factor: size })
                    .named("Pillar")
            };
            let step = d(&["stone"], CastType::Bolt)
                .with(Modifier::Duration { factor: 0.45 })
                .named("Stone block");
            let fire = || d(&["fire"], CastType::Bolt);
            vec![
                // Fire bolts burst on a stone wall's face.
                casts(vec![
                    cast(0.0, wall("stone").named("Stone wall"), AimAt::Lane(-4.0)),
                    cast(1.4, fire(), AimAt::Focus),
                    cast(2.2, fire(), AimAt::Focus),
                ]),
                // Light and stone: a fire bolt goes through the frost wall (as steam) and
                // bursts on the stone one behind it.
                casts(vec![
                    cast(0.0, wall("frost"), AimAt::Lane(-2.5)),
                    cast(0.5, wall("stone").named("Stone wall"), AimAt::Lane(-6.5)),
                    cast(2.0, fire(), AimAt::Focus),
                ]),
                // A pillar under a crate stack, broad enough to carry all three.
                casts(vec![cast(0.0, pillar(1.3), AimAt::Lane(-5.0))]),
                // Steps where stone bolts strike the floor, far first so no bolt meets a step.
                casts(vec![
                    cast(0.0, step.clone(), AimAt::Lane(-7.5)),
                    cast(0.4, step.clone(), AimAt::Lane(-4.5)),
                    cast(0.8, step, AimAt::Lane(-1.5)),
                ]),
                // A pillar lifting a dummy.
                casts(vec![cast(0.0, pillar(1.0), AimAt::Lane(-5.0))]),
                // A stone grip on a near dummy, cracked early by fire.
                casts(vec![
                    cast(
                        0.0,
                        d(&["stone"], CastType::Bolt).named("Stone grip"),
                        AimAt::Focus,
                    ),
                    cast(3.4, fire(), AimAt::Focus),
                ]),
            ]
        }
        // The duel's students cast by their own judgement (duel::run_duel), not on a clock.
        Program::Duel => (0..LANES.len())
            .map(|_| Lane {
                mastery: Mastery::Adept,
                level: 34.0,
                known: None,
                casts: Vec::new(),
            })
            .collect(),
    }
}

/// The hall's state.
#[derive(Resource, Debug, Clone)]
pub struct Hall {
    pub origin: DVec3,
    pub program: Program,
    /// Seconds into the program.
    pub t: f32,
    /// Set the program up on the next frame (props, mages, dummies).
    pub restart: bool,
    /// Each lane's last cast, for the readout.
    pub labels: Vec<String>,
    /// Whether a camera is near enough for the mages to cast.
    pub near: bool,
    /// Periods run.
    pub cycle: u64,
    /// Put the force program's crates and dummies back (every other period).
    pub reset_props: bool,
    /// The duel's last few reactions, shatters and downs, for the readout.
    pub events: Vec<String>,
}

impl Hall {
    pub fn new(origin: DVec3) -> Self {
        Self {
            origin,
            program: Program::Mastery,
            t: 0.0,
            restart: true,
            labels: vec![String::new(); LANES.len()],
            near: true,
            cycle: 0,
            reset_props: false,
            events: Vec::new(),
        }
    }
}

/// Lay out the hall at `origin` (the zone's center).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    spawn_terrace(commands, origin);
    for (i, x) in LANES.iter().enumerate() {
        spawn_mage(
            commands,
            origin + DVec3::new(*x, PLINTH_TOP, MAGE_Z),
            i,
            Mastery::Ascended1,
        );
        spawn_dummy(
            commands,
            origin + DVec3::new(*x, FLOOR, DUMMY_Z),
            i,
            true,
            false,
        );
    }
    commands.insert_resource(Hall::new(origin));
}

/// Set a program up: mages' tiers and spells, fresh dummies, the program's props.
#[allow(clippy::type_complexity)]
fn setup_program(
    mut commands: Commands,
    hall: Option<ResMut<Hall>>,
    mut mages: Query<
        (
            Entity,
            &HallMage,
            &mut MagicCaster,
            &Children,
            &mut Transform,
            Option<&mut Appearance>,
        ),
        Without<HallTarget>,
    >,
    props: Query<Entity, (With<ProgramProp>, Without<HallTarget>)>,
    mut stand_ins: Query<&mut MeshRenderer, With<StandIn>>,
    mut targets: Query<(
        Entity,
        &HallTarget,
        &mut Transform,
        Option<&mut Vitality>,
        Has<ProgramProp>,
    )>,
    carriers: Query<
        Entity,
        Or<(
            With<Missile>,
            With<Field>,
            With<BeamCast>,
            With<ChainArc>,
            With<Rainfall>,
        )>,
    >,
) {
    let Some(mut hall) = hall else {
        return;
    };
    if !hall.restart && !hall.reset_props {
        return;
    }
    let restart = hall.restart;
    hall.restart = false;
    hall.reset_props = false;
    let origin = hall.origin;
    // Fresh props: the force program's crates and loose dummies.
    for (e, _, _, _, prop) in &targets {
        if prop {
            commands.entity(e).despawn();
        }
    }
    // The duel's masters.
    for e in &props {
        commands.entity(e).despawn();
    }
    if restart {
        for e in &carriers {
            commands.entity(e).despawn();
        }
        // A fresh floor: the last program's scorch, frost and blood marks go (vfx's marks;
        // without vfx the call just fails).
        commands.queue(move |world: &mut World| {
            let _ = engine_core::registry::call_api(
                world,
                "vfx.clear_marks",
                json!({"center": [origin.x, origin.y, origin.z - 3.0], "radius_m": 26.0}),
            );
        });
        hall.t = 0.0;
        hall.cycle = 0;
        hall.labels = vec![String::new(); LANES.len()];
    }
    let program = hall.program;
    let plan = lanes(program, 0);
    for (e, m, mut c, children, mut tf, look) in &mut mages {
        let Some(l) = plan.get(m.lane) else { continue };
        if restart {
            // Back on the plinth (a duel's blinks move them), whole and unafflicted.
            if let Some(home) = duel::student_home(origin, m) {
                tf.translation = home;
                tf.rotation = Quat::IDENTITY;
            }
            commands.entity(e).remove::<(
                Afflictions,
                engine_world::Statuses,
                LocalTimeScale,
                duel::Duelist,
                Ragdoll,
            )>();
            c.side = duel::STUDENTS;
            if program == Program::Duel {
                commands.entity(e).insert(duel::student(&mut c, m.lane));
            } else {
                commands.entity(e).insert(Vitality::new(300.0));
            }
        }
        let side = (program == Program::Duel).then_some([0.1, 0.35, 0.9]);
        let want = mage_look(m.lane, l.mastery, side);
        match look {
            Some(mut a) if *a != want => *a = want,
            Some(_) => {}
            None => {
                commands.entity(e).insert(want);
            }
        }
        let fresh = MagicCaster::new(l.level, l.mastery);
        c.level = l.level;
        c.skills = fresh.skills;
        c.god = false;
        c.max_mana = 1.0e6;
        c.mana = 1.0e6;
        c.mana_regen = 1.0e5;
        c.grimoire = l.known.clone().into_iter().collect();
        c.selected = 0;
        let robe = robe_color(l.mastery);
        for child in children.iter() {
            if let Ok(mut r) = stand_ins.get_mut(child)
                && let engine_render::MaterialRef::Inline(mat) = &mut r.material
                && mat.base_color[0] < 0.6
            {
                mat.base_color = [robe[0], robe[1], robe[2], 1.0];
            }
        }
    }
    // The far dummies stand back up, unhurt and unafflicted; the duel's floor is clear of
    // them (stored under the hall, where no seeking bolt looks).
    for (e, t, mut tf, v, prop) in &mut targets {
        if prop || !restart {
            continue;
        }
        tf.translation = if program == Program::Duel {
            t.home - DVec3::Y * 40.0
        } else {
            t.home
        };
        if let Some(mut v) = v {
            v.health = v.max;
            v.down_secs = 0.0;
        }
        commands
            .entity(e)
            .remove::<(Afflictions, engine_world::Statuses, LocalTimeScale)>();
    }
    if restart && program == Program::Duel {
        hall.events.clear();
        duel::setup_duel(&mut commands, origin);
    }
    if program == Program::Shaping {
        // Lane 2: a crate stack on the pillar's spot; lane 4: a dummy on its own.
        let c = origin + DVec3::new(LANES[2], FLOOR, -5.0);
        for (k, off) in [(-0.4, 0.0), (0.4, 0.0), (0.0, 0.0)].iter().enumerate() {
            let y = if k == 2 { 1.15 } else { 0.38 };
            spawn_crate(
                &mut commands,
                c + DVec3::new(off.0, y, off.1),
                2,
                k == 2,
                false,
            );
        }
        let rider = spawn_dummy(
            &mut commands,
            origin + DVec3::new(LANES[4], FLOOR, -5.0),
            4,
            false,
            true,
        );
        // It keeps upright on the ride, as a person would (a capsule on its round end
        // toppled off the top).
        commands
            .entity(rider)
            .insert(engine_world::physics::RigidBody {
                kind: engine_world::physics::BodyKind::Dynamic,
                mass_kg: Some(45.0),
                lock_rotation: [true, false, true],
                ..Default::default()
            });
        // Lane 5: a standing dummy near the camera for the grip (the far one is small).
        let grip = spawn_dummy(
            &mut commands,
            origin + DVec3::new(LANES[5], FLOOR, -3.5),
            5,
            true,
            false,
        );
        commands.entity(grip).insert(ProgramProp);
    }
    if program == Program::Force {
        for (i, x) in LANES.iter().enumerate() {
            let base = origin + DVec3::new(*x, FLOOR, 0.0);
            match i {
                0 => {
                    spawn_dummy(
                        &mut commands,
                        base + DVec3::new(0.0, 0.0, -3.0),
                        i,
                        true,
                        true,
                    );
                }
                1 => {
                    spawn_dummy(
                        &mut commands,
                        base + DVec3::new(0.0, 0.0, -9.0),
                        i,
                        true,
                        true,
                    );
                }
                2 | 3 => {
                    let c = base + DVec3::new(0.0, 0.0, -5.0);
                    for (k, off) in [(-0.45, 0.0), (0.45, 0.0), (0.0, 0.0)].iter().enumerate() {
                        let y = if k == 2 { 1.15 } else { 0.38 };
                        spawn_crate(
                            &mut commands,
                            c + DVec3::new(off.0, y, off.1),
                            i,
                            k == 2,
                            false,
                        );
                    }
                }
                4 => {
                    let c = base + DVec3::new(0.0, 0.0, -5.0);
                    for k in 0..5 {
                        let a = k as f64 * std::f64::consts::TAU / 5.0;
                        spawn_crate(
                            &mut commands,
                            c + DVec3::new(a.cos() * 0.8, 0.46, a.sin() * 0.8),
                            i,
                            k == 0,
                            true,
                        );
                    }
                }
                _ => {
                    // Close to the mage: a cone reaches about 7 m.
                    let c = base + DVec3::new(0.0, 0.0, 4.0);
                    for k in 0..3 {
                        spawn_crate(
                            &mut commands,
                            c + DVec3::new(-0.8 + 0.8 * k as f64, 0.38, 0.0),
                            i,
                            k == 1,
                            false,
                        );
                    }
                }
            }
        }
    }
}

/// Cast each lane's spells on time while a camera is near.
#[allow(clippy::type_complexity)]
fn run_hall(
    time: Res<Time>,
    hall: Option<ResMut<Hall>>,
    mages: Query<(Entity, &HallMage, &MagicCaster)>,
    targets: Query<(Entity, &HallTarget, Has<ProgramProp>)>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut casts: MessageWriter<CastRequest>,
) {
    let Some(mut hall) = hall else {
        return;
    };
    hall.near = cams
        .iter()
        .any(|(c, g)| c.active && g.translation().distance(hall.origin) < 160.0)
        || cams.is_empty();
    if !hall.near || hall.restart {
        return;
    }
    let dt = time.delta_secs();
    let period = hall.program.period();
    let t0 = hall.t;
    hall.t += dt;
    let t1 = hall.t;
    let cycle = (t0 / period).floor() as u64;
    hall.cycle = (t1 / period).floor() as u64;
    if hall.program == Program::Force && hall.cycle != cycle && hall.cycle % 2 == 0 {
        hall.reset_props = true;
    }
    if hall.program == Program::Shaping && hall.cycle != cycle {
        hall.reset_props = true;
    }
    let plan = lanes(hall.program, cycle);
    for (e, m, caster) in &mages {
        let Some(l) = plan.get(m.lane) else { continue };
        let stagger = 0.13 * m.lane as f32;
        for c in &l.casts {
            // Fires when the lane's clock crosses `c.at` in this period.
            let at = c.at + stagger;
            let crossed = |a: f32, b: f32| {
                let (pa, pb) = (a - at, b - at);
                pb >= 0.0 && (pa < 0.0 || (pa / period).floor() != (pb / period).floor())
            };
            if !crossed(t0, t1) {
                continue;
            }
            let aim = match c.aim {
                // A program's own target first, else the lane's far dummy.
                AimAt::Focus => targets
                    .iter()
                    .filter(|(_, t, _)| t.lane == m.lane && t.focus)
                    .min_by_key(|(e, _, prop)| (!prop, *e))
                    .map(|(e, _, _)| Aim::Entity(e))
                    .unwrap_or(Aim::Point(
                        hall.origin + DVec3::new(LANES[m.lane], FLOOR, DUMMY_Z),
                    )),
                AimAt::Lane(z) => Aim::Point(hall.origin + DVec3::new(LANES[m.lane], FLOOR, z)),
            };
            let spell = match &c.design {
                Some(d) => SpellChoice::Design(d.clone()),
                None => SpellChoice::Selected,
            };
            let design = match &c.design {
                Some(d) => Some(d.clone()),
                None => caster.grown(caster.selected),
            };
            if let Some(design) = design {
                let tier = if hall.program == Program::Mastery {
                    format!("{} · ", l.mastery.name())
                } else {
                    String::new()
                };
                hall.labels[m.lane] = match caster.compile(&design, 0.0) {
                    Ok(s) => format!("{tier}{}", s.summary()),
                    Err(err) => format!("{tier}{err}"),
                };
            }
            casts.write(CastRequest {
                caster: e,
                spell,
                aim,
                charge: 0.0,
            });
        }
    }
}

fn show_readout(hall: Option<Res<Hall>>, readouts: Option<ResMut<Readouts>>) {
    let (Some(hall), Some(mut readouts)) = (hall, readouts) else {
        return;
    };
    if hall.near {
        write_readout(&mut readouts, hall.program.title(), &hall.labels);
    }
}

/// The duel's readout: who stands, and the reactions worth naming.
fn duel_readout(
    hall: Option<ResMut<Hall>>,
    q: Query<(&duel::Duelist, &Vitality)>,
    mut hits: MessageReader<SpellHit>,
) {
    let Some(mut hall) = hall else {
        hits.clear();
        return;
    };
    if hall.program != Program::Duel {
        hits.clear();
        return;
    }
    let mut n = (0, 0, 0, 0);
    for (d, v) in &q {
        let up = usize::from(!v.down());
        if d.team == duel::STUDENTS {
            n.0 += up;
            n.1 += 1;
        } else {
            n.2 += up;
            n.3 += 1;
        }
    }
    duel::duel_lines(&mut hall, n, &mut hits);
}

pub fn build(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (run_hall, (duel::duel_footwork, duel::run_duel).chain())
            .in_set(SimSet::Gameplay)
            .before(crate::MagicSet::Cast),
    )
    .add_systems(
        Update,
        (
            setup_program,
            hide_stand_ins,
            duel::limp_when_down,
            duel_readout.before(show_readout),
            show_readout,
        ),
    )
    .register_api(
        "magic.demo",
        "Run a Weaving hall program: spells_mastery, spells_elements, spells_combos, \
         spells_forms, spells_meet, spells_force, spells_dark, spells_duel or spells_shaping. \
         With no program, lists them.",
        r#"{"program": "spells_combos"}"#,
        |world, args| {
            let list: Vec<Value> = Program::ALL
                .iter()
                .map(|p| json!({"program": p.key(), "shows": p.title()}))
                .collect();
            let Some(key) = args.get("program").and_then(Value::as_str) else {
                return Ok(json!({ "programs": list }));
            };
            let p = Program::from_key(key).ok_or_else(|| {
                format!("No program \"{key}\"; magic.demo with no args lists them.")
            })?;
            let mut hall = world
                .get_resource_mut::<Hall>()
                .ok_or("The Weaving hall isn't loaded.")?;
            hall.program = p;
            hall.restart = true;
            Ok(json!({ "program": p.key() }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_program_fits_its_casters() {
        for p in Program::ALL {
            for cycle in 0..2 {
                for (i, l) in lanes(p, cycle).iter().enumerate() {
                    let mut c = MagicCaster::new(l.level, l.mastery);
                    c.grimoire = l.known.clone().into_iter().collect();
                    for cast in &l.casts {
                        let design = cast.design.clone().or_else(|| c.grown(0)).unwrap();
                        let s = c.compile(&design, 0.0);
                        assert!(s.is_ok(), "{p:?} cycle {cycle} lane {i}: {:?}", s.err());
                    }
                }
            }
        }
    }

    #[test]
    fn programs_round_trip_by_key() {
        for p in Program::ALL {
            assert_eq!(Program::from_key(p.key()), Some(p));
        }
        assert_eq!(Program::from_key("meet"), Some(Program::Meet));
        assert_eq!(VIEWS.len(), Program::ALL.len());
    }

    #[test]
    fn the_mastery_ladder_grows() {
        let ls = lanes(Program::Mastery, 0);
        let complexity: Vec<u32> = ls
            .iter()
            .map(|l| {
                let mut c = MagicCaster::new(l.level, l.mastery);
                c.grimoire = l.known.clone().into_iter().collect();
                c.grown(0).unwrap().complexity()
            })
            .collect();
        // Bolt to volley grows in parts; the ascended rain is a new form, not more parts.
        assert!(
            complexity[..5].windows(2).all(|w| w[0] < w[1]),
            "{complexity:?}"
        );
        assert_eq!(complexity[4], 11);
        let mut top = MagicCaster::new(ls[5].level, ls[5].mastery);
        top.grimoire = ls[5].known.clone().into_iter().collect();
        assert_eq!(top.grown(0).unwrap().cast, CastType::Rain);
    }
}
