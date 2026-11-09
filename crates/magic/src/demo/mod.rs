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
//! | `spells_player` | the player's spell bar over the shoulder: charge, place, chain, channel, with the HUD hotbar and mana ([`player`]) |
//! | `spells_summon` | summons and tethers: a stone golem and a lava golem walking to dummies and punching them, frost and storm elementals shooting, a wisp healing a wounded mage, a tether hauling a dummy in, then one snapping |
//! | `spells_dead` | the dead rise: three necromancers raise skeleton warriors, a storm skeletal mage and a fallen student as a thrall against three students' golem, fire elemental and fire bolts |
//! | `spells_ward` | wards you can see: a warden's dome over three students stops a fire barrage until it shatters, a frost shell takes fire double, a storm shell shocks the skeletons that strike it, then a ward wall goes up ([`crate::wards`]) |
//! | `spells_portal` | doors through space: a warden opens a portal on the floor and its other end down the hall, fires bolts through it into a dummy beyond, a storm beam bends through it, a push shoves a crate through, and a mage blinks off the dais twice and recalls back along its path ([`portal`], [`crate::space`]) |
//! | `spells_kinesis` | telekinesis: a stone torn out of the floor and launched (then a burning one), a barrel and a crate lifted and thrown, a bolt caught in the air and thrown on, a loose dummy hurled down the hall, and a ring of torn stones that stops bolts and is flung at a dummy ([`kinesis`], [`crate::kinesis`]) |
//! | `spells_shield` | shields you hold: one under a fire barrage cracks, flickers and shatters (its holder staggers), then regrows after a second; another raised just in time sends bolts back at the battle mage who cast them, then holds a fire shield into a flame jet ([`shield`], [`crate::wards`]) |
//! | `spells_cataclysm` | Cataclysm: a burning meteor on a dummy among crates (its ring, the fall, the blast, a burning crater that cools), a Master's swarm of five, and fault lines that run down a lane throwing its dummies, then burst again ([`cataclysm`], [`crate::cataclysm`]) |
//! | `spells_minds` | minds: necromancers raise a golem and skeletons; students answer with three decoys the skeletons cut into glass, a mirror image on the dais, a terror wave that sends the dead running, a frenzy that turns them on each other, and a command that turns the golem on its own side ([`minds`], [`crate::minds`]) |
//! | `spells_craft` | the Spellcrafter at the altar: an Expert builds a returning frost glaive to 8 of 8, seek is refused, the crystal casts each change at a dummy, then it goes on key 3 and is cast from the bar ([`craft`]) |
//!
//! The mages wear hooded robes coloured by mastery (the character contract's `Appearance`).
//!
//! The mages only cast while a camera is within 160 m, so the hall costs nothing elsewhere.

pub mod art;
pub mod cataclysm;
pub mod craft;
pub mod duel;
pub mod kinesis;
pub mod minds;
pub mod player;
pub mod portal;
pub mod shield;

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
use crate::summon::{Minion, Rubble};
use crate::undead::Thrall;
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
    DemoView {
        key: "spells_player",
        name: "The player's spell bar: charge, place, chain, channel, with the HUD",
        // Over the weaver's right shoulder, the crosshair on the dummy its fireball is for.
        eye: [1.6, 2.2, 7.3],
        target: [-3.0, 1.4, -11.0],
        hour: 16.9,
        // The flame jet held on the dummy, a pillar standing, a frost wall up, two slots
        // cooling and the mana bar draining.
        warmup: 6.0,
    },
    DemoView {
        key: "spells_dead",
        name: "The dead rise: skeletons, a skeletal mage and a raised student against a golem and a fire elemental",
        // From the far end of the floor, looking back at the clash: the dead coming in from
        // the left (the necromancers' plinths), the golem and the elemental meeting them from
        // the right, the raised student at the left edge, the plinths beyond.
        eye: [1.5, 3.6, -7.0],
        target: [1.5, 0.7, 4.0],
        hour: 16.6,
        // Skeletons up and running, the first blows and bolts landing.
        warmup: 5.0,
    },
    DemoView {
        key: "spells_summon",
        name: "Summons and tethers: golems, elementals, a wisp, a chain haul and a snap",
        // From the east end, three-quarters onto the lava golem: its dummy stands beside it,
        // not in front, the stone golem behind, the elementals hovering in at the left and
        // the chain haul and the dais beyond.
        eye: [9.5, 3.0, -8.0],
        target: [0.0, 1.6, -1.5],
        hour: 16.6,
        // Both golems at their dummies mid-punch, the elementals firing, the chain hauling.
        warmup: 5.6,
    },
    DemoView {
        key: "spells_craft",
        name: "The Spellcrafter: a returning frost glaive built to an Expert's budget, cast at a dummy and put on key 3",
        // Behind the altar's right, the screen on the left: the weaver at the altar's back
        // left just past the screen's edge, the crystal in the middle, the dummy 9 m down the
        // hall on the right.
        eye: [2.2, 2.9, 14.2],
        target: [-3.4, 1.0, 3.9],
        hour: 16.9,
        // The glaive at 8 of 8, seek just refused in red, the crystal's glaives in flight.
        warmup: 10.9,
    },
    DemoView {
        key: "spells_ward",
        name: "Wards: a dome over three students under a fire barrage, frost and storm shells, then a ward wall",
        // From the hall floor in front of the altar: the fire students' line of fire on the
        // left, the dome over the three students filling the right, its near face where the
        // bolts burst, then the ward wall slanting across beside the altar.
        eye: [2.5, 4.0, -1.5],
        target: [-4.5, 2.2, 9.0],
        hour: 16.6,
        // The dome cracked once, flares on its face, a skeleton at a shell.
        warmup: 4.6,
    },
    DemoView {
        key: "spells_portal",
        name: "Portals: bolts, a beam and a crate through a door in space; a blink and a recall",
        // Over the warden's line of fire, just in front of it and 1.3 m above: its bolts fly
        // in under the camera into the near end 7 m away, and through the opening you see them
        // strike the dummy beyond the far end, in the middle of the view. The far end from
        // behind down the hall, the blinker's path between the two.
        eye: [-8.2, 3.4, 8.3],
        target: [-2.5, 1.2, -1.5],
        hour: 16.6,
        // A bolt in flight out of the far end, the blinker mid-path.
        warmup: 4.3,
    },
    DemoView {
        key: "spells_kinesis",
        name: "Telekinesis: stones torn from the floor, a crate thrown, a bolt caught, a ring of stones",
        eye: [1.8, 7.4, 17.0],
        target: [1.8, 0.2, -2.5],
        hour: 16.6,
        warmup: 4.4,
    },
    DemoView {
        key: "spells_shield",
        name: "Held shields: a barrage cracks and shatters one, another sends bolts back",
        // Over the two battle mages' shoulders, looking back at the holders' shields.
        eye: [6.0, 3.3, 2.6],
        target: [6.0, 1.8, 9.0],
        hour: 16.6,
        // The barrage has cracked lane 4's shield; lane 3's second bolt is on its way back.
        warmup: 5.0,
    },
    DemoView {
        key: "spells_cataclysm",
        name: "Cataclysm: a burning meteor, a swarm of five and fault lines that throw dummies",
        // From over the floor before the dais' right end, across the lanes: the meteors come
        // down out of the sky on their slant from the right, the fault line runs across the
        // view from the left.
        eye: [13.0, 5.5, 9.0],
        target: [-1.0, 5.0, -8.0],
        hour: 16.6,
        // Lane 3's meteor has struck: its crater burning, the swarm's rings pulsing far off.
        warmup: 4.6,
    },
    DemoView {
        key: "spells_minds",
        name: "Minds: decoys the skeletons cut into glass, a terror wave, a frenzy and a commanded golem",
        // From the far end of the floor, as spells_dead, looking back at the dais: the dead
        // coming in from the left, the decoys on the floor before the students' plinths,
        // lane 2's mirror image on the dais behind them; far enough left that the dead the
        // terror wave scatters stay in the picture (the probe in tests/spells.rs checks), and
        // the four late dead climb out mid-floor, around the golem, close to the camera.
        eye: [7.0, 3.9, -6.5],
        target: [5.0, 0.9, 5.0],
        hour: 16.6,
        // The skeletons at the decoys, the mirror image standing on the dais.
        warmup: 6.0,
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
    Player,
    Summon,
    Dead,
    Craft,
    Ward,
    Portal,
    Kinesis,
    Shield,
    Cataclysm,
    Minds,
}

impl Program {
    pub const ALL: [Program; 19] = [
        Program::Mastery,
        Program::Elements,
        Program::Combos,
        Program::Forms,
        Program::Meet,
        Program::Force,
        Program::Dark,
        Program::Duel,
        Program::Shaping,
        Program::Player,
        Program::Summon,
        Program::Dead,
        Program::Craft,
        Program::Ward,
        Program::Portal,
        Program::Kinesis,
        Program::Shield,
        Program::Cataclysm,
        Program::Minds,
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
            Program::Player => "spells_player",
            Program::Summon => "spells_summon",
            Program::Dead => "spells_dead",
            Program::Craft => "spells_craft",
            Program::Ward => "spells_ward",
            Program::Portal => "spells_portal",
            Program::Kinesis => "spells_kinesis",
            Program::Shield => "spells_shield",
            Program::Cataclysm => "spells_cataclysm",
            Program::Minds => "spells_minds",
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
            Program::Player => player::PERIOD,
            // Summons last 10.8 s: each period they fall as rubble and rise again.
            Program::Summon => 12.0,
            // The dead last 9.6 s: the bones and the ash are gone before the next rising.
            Program::Dead => 13.0,
            Program::Craft => craft::PERIOD,
            // The dome stands about 6 s under the barrage, the wall goes up after it, and both
            // are gone before the next period's dome.
            Program::Ward => 15.0,
            // The pair stands 15 s and is gone before the next period opens another.
            Program::Portal => 16.0,
            // The ring is flung by 15.5 s; its stones and the thrown props reset each period.
            Program::Kinesis => 16.0,
            // Both shields are down and the battle mages stand fresh each period.
            Program::Shield => 16.0,
            // Every crater has sunk and every crack closed before the next period's.
            Program::Cataclysm => cataclysm::PERIOD,
            // The dead last 21.6 s and the command on the golem 12 s; the restart clears the rest.
            Program::Minds => minds::PERIOD,
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
    /// The ground at (x, z) from the hall's center.
    Floor(f64, f64),
    /// The nearest creature of another side, else the lane's focus.
    Foe,
    /// The hall mage on that lane.
    Mage(usize),
    /// The nearest golem of another side, else the lane's far dummy.
    Golem,
    /// A point (x, y, z) from the hall's center.
    At(f64, f64, f64),
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
        Program::Summon => {
            let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
                at,
                design: Some(design),
                aim,
            };
            let casts = |casts: Vec<Cast>| Lane {
                casts,
                ..lane(SpellDesign::default())
            };
            // Gone before the next period's rise.
            let summon = |fx: &[&str], name: &str| {
                d(fx, CastType::Summon)
                    .with(Modifier::Duration { factor: 0.18 })
                    .named(name)
            };
            vec![
                // A wisp: it flies to the wounded mage of the third lane.
                // (Its lane fires no stagger, and a cast at 0.0 waits a whole period.)
                casts(vec![cast(0.1, summon(&["heal"], "Wisp"), AimAt::Lane(7.0))]),
                // A chain haul drags the dummy in; then a storm tether holds it until a push
                // throws it past the break.
                casts(vec![
                    cast(
                        2.0,
                        d(&["pull"], CastType::Tether).named("Chain haul"),
                        AimAt::Focus,
                    ),
                    cast(
                        7.2,
                        d(&["storm"], CastType::Tether).named("Storm tether"),
                        AimAt::Focus,
                    ),
                    cast(8.6, d(&["push"], CastType::Bolt), AimAt::Focus),
                ]),
                casts(vec![cast(
                    0.0,
                    summon(&["stone"], "Stone golem"),
                    AimAt::Lane(0.0),
                )]),
                casts(vec![cast(
                    0.0,
                    summon(&["fire", "stone"], "Lava golem"),
                    AimAt::Lane(0.0),
                )]),
                casts(vec![cast(
                    0.0,
                    summon(&["frost"], "Frost elemental"),
                    AimAt::Lane(6.0),
                )]),
                casts(vec![cast(
                    0.0,
                    summon(&["storm"], "Storm elemental"),
                    AimAt::Lane(6.0),
                )]),
            ]
        }
        Program::Dead => {
            let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
                at,
                design: Some(design),
                aim,
            };
            // Every lane casts as an Ascended (no fizzles to spoil the show) and at one level,
            // so neither army sweeps the floor at once; the robes say who is who (students in
            // green and blue, necromancers in black: `dead_robe`).
            let lane_of = |casts: Vec<Cast>| Lane {
                mastery: Mastery::Ascended1,
                level: 60.0,
                known: None,
                casts,
            };
            let summon = |fx: &[&str], name: &str| {
                d(fx, CastType::Summon)
                    .with(Modifier::Duration { factor: 0.16 })
                    .named(name)
            };
            let skeleton = || summon(&["raise"], "Raise skeleton");
            // Seeking, to find a running skeleton; safeguarded, to fly past the golem beside
            // it and never burst on it.
            let bolt = || {
                d(&["fire"], CastType::Bolt)
                    .with(Modifier::Seek)
                    .with(Modifier::Safeguard)
                    .named("Fire bolt")
            };
            // The battle keeps to the middle of the floor, 12 m across, in the camera's view.
            vec![
                lane_of(vec![cast(
                    0.1,
                    summon(&["stone"], "Stone golem"),
                    AimAt::Floor(-4.5, 1.5),
                )]),
                lane_of(vec![cast(
                    0.2,
                    summon(&["fire"], "Fire elemental"),
                    AimAt::Floor(-3.5, 4.5),
                )]),
                // Fire bolts into whatever the dead raise.
                lane_of(
                    [2.6, 4.4, 6.2, 8.0]
                        .into_iter()
                        .map(|at| cast(at, bolt(), AimAt::Foe))
                        .collect(),
                ),
                // Three skeleton warriors each at once, and more climbing out mid-fight to
                // take the place of the fallen.
                lane_of(vec![
                    cast(0.2, skeleton(), AimAt::Floor(2.5, 2.5)),
                    cast(0.8, skeleton(), AimAt::Floor(3.5, 0.5)),
                    cast(1.4, skeleton(), AimAt::Floor(4.0, 3.0)),
                    cast(4.6, skeleton(), AimAt::Floor(1.5, 1.0)),
                    cast(6.6, skeleton(), AimAt::Floor(2.0, 3.5)),
                ]),
                lane_of(vec![
                    cast(0.2, skeleton(), AimAt::Floor(6.0, 0.0)),
                    cast(0.8, skeleton(), AimAt::Floor(5.5, 1.5)),
                    cast(1.4, skeleton(), AimAt::Floor(4.5, 0.5)),
                    cast(5.4, skeleton(), AimAt::Floor(3.5, 0.0)),
                    cast(7.4, skeleton(), AimAt::Floor(5.0, 1.5)),
                ]),
                // A storm skeletal mage (a frost one would steam the fire elemental into a cloud
                // that hides the fight, and freeze the golem to shatter it in a few blows), then
                // the fallen student stands for the dead.
                lane_of(vec![
                    cast(
                        0.2,
                        summon(&["raise", "storm"], "Skeletal mage"),
                        AimAt::Floor(8.5, 6.5),
                    ),
                    cast(
                        1.4,
                        skeleton().named("Raise the fallen"),
                        AimAt::Floor(FALLEN[0], FALLEN[1]),
                    ),
                ]),
            ]
        }
        Program::Ward => {
            let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
                at,
                design: Some(design),
                aim,
            };
            // As in spells_dead: everyone Ascended (no fizzles) at one level, students on the
            // left plinths, necromancers on the right.
            let lane_of = |casts: Vec<Cast>| Lane {
                mastery: Mastery::Ascended1,
                level: 60.0,
                known: None,
                casts,
            };
            let shell = |fx: &[&str], name: &str| d(fx, CastType::OnSelf).named(name);
            let fire_bolt = || {
                d(&["fire"], CastType::Bolt)
                    .with(Modifier::Seek)
                    .with(Modifier::Safeguard)
                    .named("Fire bolt")
            };
            // Fire at the students: on the dome while it stands, then on the wall.
            let barrage = |from: f32, every: f32, at_lane: usize| -> Vec<Cast> {
                (0..)
                    .map(|i| from + i as f32 * every)
                    .take_while(|&at| at < WARD_BARRAGE_END)
                    .map(|at| cast(at, fire_bolt(), AimAt::Mage(at_lane)))
                    .collect()
            };
            vec![
                // A frost shell: the barrage's fire costs it double, so it breaks first.
                lane_of(vec![
                    cast(0.4, shell(&["ward", "frost"], "Frost ward"), AimAt::Focus),
                    cast(
                        4.5,
                        d(&["frost"], CastType::Bolt).named("Frost bolt"),
                        AimAt::Foe,
                    ),
                    cast(
                        9.5,
                        d(&["frost"], CastType::Bolt).named("Frost bolt"),
                        AimAt::Foe,
                    ),
                ]),
                // The warden: a dome over the three of them, then a ward wall once it falls.
                lane_of(vec![
                    cast(
                        0.1,
                        d(&["ward"], CastType::Area)
                            .with(Modifier::Size { factor: 1.6 })
                            .named("Ward dome"),
                        AimAt::Floor(WARD_DOME[0], WARD_DOME[1]),
                    ),
                    cast(
                        WARD_WALL_AT,
                        // Three panels, the length of the dais' depth.
                        d(&["ward"], CastType::Wall)
                            .with(Modifier::Size { factor: 0.53 })
                            .named("Ward wall"),
                        AimAt::Floor(WARD_WALL[0], WARD_WALL[1]),
                    ),
                ]),
                // A storm shell, nearest the dead: what strikes it is shocked.
                lane_of(vec![
                    cast(0.6, shell(&["ward", "storm"], "Storm ward"), AimAt::Focus),
                    cast(
                        5.5,
                        d(&["storm"], CastType::Bolt).named("Storm bolt"),
                        AimAt::Foe,
                    ),
                ]),
                // Two fire students (level 42: bolts about 0.4 of a level-60 one), so the dome
                // takes a dozen flares before it shatters, around 7 s.
                Lane {
                    level: 42.0,
                    ..lane_of(barrage(1.0, 0.9, 1))
                },
                Lane {
                    level: 42.0,
                    ..lane_of(barrage(1.45, 0.9, 0))
                },
                // Skeletons that walk through the dome and strike the shells.
                lane_of(vec![
                    cast(
                        0.3,
                        d(&["raise"], CastType::Summon)
                            .with(Modifier::Duration { factor: 0.2 })
                            .named("Raise skeleton"),
                        AimAt::Floor(1.0, 6.5),
                    ),
                    cast(
                        0.9,
                        d(&["raise"], CastType::Summon)
                            .with(Modifier::Duration { factor: 0.2 })
                            .named("Raise skeleton"),
                        AimAt::Floor(1.5, 9.0),
                    ),
                ]),
            ]
        }
        Program::Portal => portal::lanes(),
        Program::Kinesis => kinesis::lanes(),
        Program::Shield => shield::lanes(),
        Program::Cataclysm => cataclysm::lanes(),
        Program::Minds => minds::lanes(),
        // The hall's mages watch while the weaver works its spell bar (player::script_weaver)
        // or the Spellcrafter (craft::script_crafter).
        Program::Player | Program::Craft => (0..LANES.len())
            .map(|_| Lane {
                casts: Vec::new(),
                ..lane(SpellDesign::default())
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

/// The necromancers' side in `spells_dead` (the students keep [`duel::STUDENTS`]).
pub const NECROMANCERS: u32 = 3;
/// Where the fallen student lies in `spells_dead`, in front of the last lane (x, z).
/// More than 3 m from every plain raise of lanes 3 and 4, so only lane 5 raises it.
pub const FALLEN: [f64; 2] = [7.5, 4.5];

/// `spells_ward`: the dome over the students' plinths (x, z), the ward wall the warden raises
/// once it falls, when, and when the necromancers' barrage stops.
pub const WARD_DOME: [f64; 2] = [-9.0, 9.0];
/// On the dais between the storm student and the altar (which stands at x 0).
pub const WARD_WALL: [f64; 2] = [-1.3, 10.0];
pub const WARD_WALL_AT: f32 = 8.2;
pub const WARD_BARRAGE_END: f32 = 13.0;

/// Programs with two armies: students on the left plinths, necromancers on the right.
fn two_sides(program: Program) -> bool {
    matches!(program, Program::Dead | Program::Ward | Program::Minds)
}

/// The robes `spells_dead`'s students wear (green, blue, green), whatever they cast as.
fn dead_robe(program: Program, lane: usize) -> Option<Mastery> {
    (two_sides(program) && lane < 3).then_some(if lane == 1 {
        Mastery::Apprentice
    } else {
        Mastery::Adept
    })
}

/// The fallen student of `spells_dead`: a body on the floor (down, limp), a fire bolt in its
/// grimoire for when it is raised against its classmates.
pub fn spawn_fallen(commands: &mut Commands, origin: DVec3) -> Entity {
    let at = origin + DVec3::new(FALLEN[0], FLOOR, FALLEN[1]);
    let e = spawn_mage(commands, at, 207, Mastery::Adept);
    commands.entity(e).insert((
        Name::new("Fallen student"),
        Transform::from_translation(at).with_rotation(Quat::from_rotation_y(-0.9)),
        MagicCaster {
            learns: false,
            side: duel::STUDENTS,
            seed: 3107,
            grimoire: vec![KnownSpell {
                design: d(&["fire"], CastType::Bolt).named("Fire bolt"),
                growth: None,
            }],
            ..MagicCaster::new(34.0, Mastery::Adept)
        },
        mage_look(7, Mastery::Adept, None),
        Vitality {
            health: 0.0,
            revive_secs: 0.0,
            ..Vitality::new(300.0)
        },
        Ragdoll { weight: 1.0 },
        ProgramProp,
    ));
    e
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
    spawn_altar(commands, origin);
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
            With<Minion>,
            With<Rubble>,
            With<crate::kinesis::Grip>,
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
    // Every period of spells_ward starts with no ward standing.
    if !restart && hall.program == Program::Ward {
        commands.queue(crate::wards::clear);
    }
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
        // A fresh floor: the last program's scorch, frost and blood marks go (vfx's marks), and
        // its ground fires and glowing coals go out. No water, so the floor stays dry for the
        // next program's fire. Without vfx or fire the calls just fail.
        commands.queue(crate::wards::clear);
        commands.queue(crate::space::clear);
        commands.queue(crate::cataclysm::clear);
        commands.queue(crate::minds::clear);
        commands.queue(move |world: &mut World| {
            let center = [origin.x, origin.y, origin.z - 3.0];
            let _ = engine_core::registry::call_api(
                world,
                "vfx.clear_marks",
                json!({"center": center, "radius_m": 26.0}),
            );
            let _ = engine_core::registry::call_api(
                world,
                "fire.extinguish",
                json!({"at": center, "radius": 26.0, "water_kg_m2": 0.0}),
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
        // The blinker starts every period of spells_portal on its plinth, whatever its recall
        // left.
        if !restart
            && program == Program::Portal
            && m.lane == portal::BLINKER
            && let Some(home) = duel::student_home(origin, m)
        {
            tf.translation = home;
            tf.rotation = Quat::IDENTITY;
        }
        if restart {
            // Back on the plinth (a duel's blinks move them), whole and unafflicted.
            if let Some(home) = duel::student_home(origin, m) {
                tf.translation = home;
                tf.rotation = Quat::IDENTITY;
            }
            // At the altar the mage on the plinth beside it steps aside along the dais, out of
            // the over-the-shoulder view.
            if program == Program::Craft && m.lane == 3 {
                tf.translation = origin + DVec3::new(LANES[3] + 2.8, DAIS_TOP, MAGE_Z);
            }
            commands.entity(e).remove::<(
                Afflictions,
                engine_world::Statuses,
                LocalTimeScale,
                duel::Duelist,
                Ragdoll,
            )>();
            c.side = if two_sides(program) && m.lane >= 3 {
                NECROMANCERS
            } else {
                duel::STUDENTS
            };
            if program == Program::Duel {
                commands.entity(e).insert(duel::student(&mut c, m.lane));
            } else {
                commands.entity(e).insert(Vitality::new(300.0));
            }
        }
        // The stone golem's summoner starts each period hurt, for the first lane's wisp to fly
        // over and heal (in the camera's view, where its own lane is far off).
        if program == Program::Summon && m.lane == 2 {
            commands.entity(e).insert(Vitality {
                health: 105.0,
                ..Vitality::new(300.0)
            });
        }
        let side = (program == Program::Duel).then_some([0.1, 0.35, 0.9]);
        let necromancer = two_sides(program) && m.lane >= 3;
        let robe_tier = dead_robe(program, m.lane).unwrap_or(l.mastery);
        let want = if necromancer {
            necromancer_look(m.lane)
        } else {
            mage_look(m.lane, robe_tier, side)
        };
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
        let robe = if necromancer {
            NECRO_ROBE
        } else {
            robe_color(robe_tier)
        };
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
        // The duel's floor is clear of them, and the summons' far end too (its camera stands
        // there).
        tf.translation = if matches!(
            program,
            Program::Duel
                | Program::Summon
                | Program::Dead
                | Program::Ward
                | Program::Portal
                | Program::Shield
                | Program::Minds
        ) {
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
    if program == Program::Summon {
        // Loose dummies for the golems to knock about and the tethers to haul. The
        // elementals float out to the nearest, the lava golem's, so frost and lightning meet
        // its fire there.
        for (lane, z) in [(1, -2.0), (2, -5.0), (3, -5.0)] {
            let dummy = spawn_dummy(
                &mut commands,
                origin + DVec3::new(LANES[lane], FLOOR, z),
                lane,
                true,
                true,
            );
            // Upright as a person keeps upright when shoved.
            commands
                .entity(dummy)
                .insert(engine_world::physics::RigidBody {
                    kind: engine_world::physics::BodyKind::Dynamic,
                    mass_kg: Some(45.0),
                    lock_rotation: [true, false, true],
                    ..Default::default()
                });
        }
    }
    if program == Program::Dead {
        spawn_fallen(&mut commands, origin);
    }
    if program == Program::Portal {
        portal::spawn_props(&mut commands, origin);
    }
    if program == Program::Kinesis {
        kinesis::spawn_props(&mut commands, origin);
    }
    if program == Program::Shield {
        shield::spawn_props(&mut commands, origin);
    }
    if program == Program::Cataclysm {
        cataclysm::spawn_props(&mut commands, origin);
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
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn run_hall(
    time: Res<Time>,
    hall: Option<ResMut<Hall>>,
    mages: Query<(Entity, &HallMage, &MagicCaster)>,
    targets: Query<(Entity, &HallTarget, Has<ProgramProp>)>,
    cams: Query<(&Camera, &GlobalTransform)>,
    (mage_at, creatures, thralls): (
        Query<&GlobalTransform, With<HallMage>>,
        Query<(Entity, &Minion, &GlobalTransform)>,
        Query<(Entity, &Thrall, &GlobalTransform)>,
    ),
    mut casts: MessageWriter<CastRequest>,
    mut commands: Commands,
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
    // The last program's ground fires go out at the restart, but hits still on their way to the
    // fire module (and its coals) can light the floor again just after: douse it twice more
    // before any program's first spell lands (1.1 s at the earliest).
    if [0.5, 1.0].iter().any(|&at| t0 < at && t1 >= at) {
        let center = hall.origin + DVec3::new(0.0, 0.0, -3.0);
        commands.queue(move |world: &mut World| {
            let _ = engine_core::registry::call_api(
                world,
                "fire.extinguish",
                json!({"at": [center.x, center.y, center.z], "radius": 26.0, "water_kg_m2": 0.0}),
            );
        });
    }
    if hall.program == Program::Force && hall.cycle != cycle && hall.cycle % 2 == 0 {
        hall.reset_props = true;
    }
    if matches!(
        hall.program,
        Program::Shaping
            | Program::Summon
            | Program::Dead
            | Program::Ward
            | Program::Portal
            | Program::Kinesis
            | Program::Shield
            | Program::Cataclysm
            | Program::Minds
    ) && hall.cycle != cycle
    {
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
                AimAt::Floor(x, z) => Aim::Point(hall.origin + DVec3::new(x, FLOOR, z)),
                AimAt::At(x, y, z) => Aim::Point(hall.origin + DVec3::new(x, y, z)),
                AimAt::Mage(lane) => mages
                    .iter()
                    .find(|(_, m, _)| m.lane == lane)
                    .map(|(e, _, _)| Aim::Entity(e))
                    .unwrap_or(Aim::Point(
                        hall.origin
                            + DVec3::new(LANES.get(lane).copied().unwrap_or(0.0), FLOOR, MAGE_Z),
                    )),
                AimAt::Golem | AimAt::Foe => {
                    let me = mage_at
                        .get(e)
                        .map(|g| g.translation())
                        .unwrap_or(hall.origin);
                    creatures
                        .iter()
                        // Not one still climbing out of the floor (it is under it).
                        .filter(|(_, mn, _)| {
                            mn.cast.side != caster.side
                                && !matches!(mn.act, crate::summon::Act::Rise { .. })
                                && (c.aim != AimAt::Golem
                                    || mn.kind == crate::summon::MinionKind::Golem)
                        })
                        .map(|(c, _, g)| (c, g.translation()))
                        .chain(
                            thralls
                                .iter()
                                .filter(|(_, t, _)| t.cast.side != caster.side)
                                .map(|(c, _, g)| (c, g.translation())),
                        )
                        .min_by(|a, b| a.1.distance(me).total_cmp(&b.1.distance(me)))
                        .map(|(c, _)| Aim::Entity(c))
                        .unwrap_or(Aim::Point(
                            hall.origin + DVec3::new(LANES[m.lane], FLOOR, DUMMY_Z),
                        ))
                }
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
                gathered: false,
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
        (
            run_hall,
            (duel::duel_footwork, duel::run_duel).chain(),
            player::script_weaver,
            craft::script_crafter.after(run_hall),
            shield::run_foes.after(run_hall),
        )
            .in_set(SimSet::Gameplay)
            .before(crate::MagicSet::Cast),
    )
    .add_systems(
        Update,
        (
            setup_program,
            player::keep_weaver,
            player::weaver_quest,
            hide_stand_ins,
            duel::limp_when_down,
            duel_readout.before(show_readout),
            show_readout,
        ),
    )
    .register_api(
        "magic.demo",
        "Run a Weaving hall program: spells_mastery, spells_elements, spells_combos, \
         spells_forms, spells_meet, spells_force, spells_dark, spells_duel, spells_shaping, \
         spells_player, spells_summon, spells_dead, spells_ward, spells_portal, \
         spells_kinesis, spells_shield or spells_craft. \
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
    fn only_the_last_lane_raises_the_fallen_student() {
        // A plain raise within 3 m of a body down raises it (undead::CORPSE_M): every other
        // lane's skeletons must climb out farther off.
        for (i, l) in lanes(Program::Dead, 0).iter().enumerate() {
            for c in &l.casts {
                let plain = c
                    .design
                    .as_ref()
                    .is_some_and(|d| d.effects == ["raise"] && d.cast == CastType::Summon);
                let AimAt::Floor(x, z) = c.aim else { continue };
                let far = ((x - FALLEN[0]).powi(2) + (z - FALLEN[1]).powi(2)).sqrt();
                if plain && i != 5 {
                    assert!(
                        far > crate::undead::CORPSE_M as f64 + 0.5,
                        "lane {i}: {far}"
                    );
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
