//! The fire showcase: a dry meadow that burns downwind and grows back, again and again.
//!
//! What to look for:
//!
//! - The flame front runs across the grass with the wind (it is lit on the upwind edge, whichever
//!   way the weather blows), slower on its flanks and barely creeping back upwind, leaving
//!   black stubble and glowing coals behind it.
//! - The gravel road stops it (unless an ember jumps across), and so does the soaked patch.
//! - Trees catch from the grass: trunk first, then the crown goes up, chars and shrinks.
//! - The wooden shed and the hay bales in its path burn down; the bales across the road don't.
//! - Embers drift downwind, smoke rises from everything that burns, the fire lights the
//!   scene and pulls air up into its plume.
//! - The campfire by the path burns forever and never lights the meadow.
//! - After the meadow has regrown, the next fire starts at the upwind edge.
//!
//! The fire is already a little way across the meadow when the demo starts
//! (`FireDemo::warmup_secs`) so there is something to see straight away. Fires that escape
//! the mown border are put out (`FireDemo::bounds`), so the rest of the map is safe.
//! `fire.demo {"at_secs": 54}` starts a fresh fire that far along, which is how a camera tour
//! catches each view's moment: each view jumps to `DemoView::at_secs` and then watches for
//! `DemoView::warmup_secs` while the smoke rises. A jump puts everything the old fire left
//! back (grass, props, embers, its random sequence, the wind it jumped in), so a view looks
//! much the same whichever view came before it, once its smoke is cleared too (`smoke.clear`).

use engine_assets::Assets;
use engine_core::math::{DQuat, DVec2, DVec3, Mat4, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, MeshRenderer};
use engine_world::{
    Combustible, FocusPoints, Ground, SurfaceMaterials, Water, WeatherState, WindField,
};

use crate::embers::Embers;
use crate::field::FireField;
use crate::fuel::{FuelPatch, PatchShape};
use crate::grass::{GrassBurn, GrassMeadow};
use crate::ground::{GroundKind, GroundPaint, PaintedGround};
use crate::objects::{Burning, Charring, ContainedFire, FlameShape};
use crate::props::{
    Foliage, SHED_ROOF_TILT, SHED_ROOF_Y, TreeParams, campfire_stones, campfire_wood, grow_tree,
    hay_bale, prop_materials, shed_roof, shed_wall, shed_wall_frame,
};
use crate::settings::FireSettings;
use crate::sim::{self, CharBase, FireClock, FirePatches, WorldFuel};

/// Half extents of the meadow (m).
const MEADOW: [f32; 2] = [28.0, 18.0];
/// Where the gravel road crosses the meadow (X, m) and its half width.
const ROAD_X: f32 = 14.0;
const ROAD_HALF: f32 = 1.5;
/// Width of the mown border around it (m).
const BORDER: f32 = 4.0;
/// Half extents of the ground the demo covers (m).
pub const FOOTPRINT: [f32; 2] = [MEADOW[0] + BORDER, MEADOW[1] + BORDER];

/// A camera spot for looking at the demo: eye and target relative to the demo's origin, the
/// hour of day it looks best at (the showcase's sky: latitude 35°, the March equinox), and how
/// far into a fire it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    /// Seconds since the fire was lit; `fire.demo {"at_secs": ..}` jumps there.
    pub at_secs: f32,
    /// Seconds to watch after the jump before the picture: the jump fast-forwards only the fire,
    /// so its smoke, embers and lights build up while this runs. The picture shows the fire
    /// `at_secs + warmup_secs` along.
    pub warmup_secs: f32,
}

/// How long a view watches its fire before the picture: long enough for the smoke to rise
/// through its 40 m box and drift across the meadow in a 5 m/s wind. (The front's view, which
/// stands downwind, watches less, or the smoke would bury the front.)
pub const WATCH_SECS: f32 = 20.0;

/// Suggested viewpoints (camera bookmarks and screenshot-tour stops).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "fire_front",
        name: "Grass fire front",
        eye: [6.0, 2.4, -20.0],
        target: [-4.0, 1.2, -4.0],
        hour: 17.5,
        at_secs: 32.0,
        warmup_secs: 3.0,
    },
    DemoView {
        key: "fire_trees",
        name: "Trees and shed burning",
        eye: [22.0, 4.0, -14.0],
        target: [2.0, 3.0, 4.0],
        hour: 17.85,
        at_secs: 60.0,
        warmup_secs: WATCH_SECS,
    },
    DemoView {
        key: "fire_campfire",
        name: "Campfire at dusk",
        eye: [-19.4, 1.3, -18.0],
        target: [-22.5, 0.35, -21.1],
        hour: 17.95,
        at_secs: 10.0,
        warmup_secs: WATCH_SECS,
    },
    DemoView {
        key: "fire_plume",
        name: "Smoke plume over the meadow",
        eye: [-58.0, 9.0, -52.0],
        target: [12.0, 12.0, 6.0],
        hour: 15.5,
        at_secs: 25.0,
        warmup_secs: WATCH_SECS,
    },
    DemoView {
        key: "fire_night",
        name: "Meadow fire at night",
        eye: [17.0, 2.0, -12.0],
        target: [0.0, 2.0, 4.0],
        hour: 22.5,
        at_secs: 35.0,
        warmup_secs: WATCH_SECS,
    },
];

/// A burning-meadow demo, centered on its `Transform`. Tune it live through the AI bridge.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FireDemo {
    /// How far into its life the first fire already is when the demo starts, seconds.
    pub warmup_secs: f32,
    /// Seconds from one fire to the next; the meadow is reset and relit.
    pub cycle_secs: f32,
    /// Fires are kept inside this rectangle (half extents, m).
    pub bounds: [f32; 2],
    /// Where each fire starts: a line between two points (m, relative to the demo).
    pub line: [[f32; 2]; 2],
    /// Light each fire along the edge of the meadow (west of the road) that faces the wind
    /// blowing at the time, so it runs across the meadow whichever way the weather blows;
    /// `line` is used in still air or when this is off.
    pub upwind: bool,
}

impl Default for FireDemo {
    fn default() -> Self {
        Self {
            warmup_secs: 50.0,
            cycle_secs: 300.0,
            bounds: FOOTPRINT,
            line: [
                [-MEADOW[0] + 2.0, -MEADOW[1] + 3.0],
                [-MEADOW[0] + 2.0, MEADOW[1] - 3.0],
            ],
            upwind: true,
        }
    }
}

/// Where a demo is in its loop.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct FireDemoState {
    frames: u32,
    started: bool,
    /// Seconds since the current fire was lit.
    pub since: f32,
    /// Fires lit so far.
    pub cycles: u32,
    /// Start a fresh fire this many seconds along at the next update (`fire.demo`).
    pub jump_to: Option<f32>,
}

/// An object the demo puts back as it was when the meadow resets.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoObject {
    pub demo: Entity,
    pub initial: Combustible,
}

/// Build the fire demo around `origin`. It needs about 64 × 44 m of open ground.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let mut rng = Rng::new(0xF12E);
    let demo = commands
        .spawn((
            Name::new("Fire demo"),
            Transform::from_translation(origin),
            FireDemo::default(),
            FireDemoState::default(),
        ))
        .id();
    let at = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    let place = |x: f64, z: f64| Transform::from_translation(origin + DVec3::new(x, 0.0, z));

    // Fuel: dry grass in the meadow, a mown border, a gravel road and a soaked patch.
    commands.spawn((
        Name::new("Fire demo: dry meadow"),
        place(0.0, 0.0),
        FuelPatch::grass(PatchShape::Rect {
            half_extents: MEADOW,
        })
        .with_load(0.4)
        .with_moisture(0.05)
        .with_regrow(110.0)
        .with_priority(20),
    ));
    commands.spawn((
        Name::new("Fire demo: mown border"),
        place(0.0, 0.0),
        FuelPatch::firebreak(PatchShape::Rect {
            half_extents: [MEADOW[0] + BORDER, MEADOW[1] + BORDER],
        })
        .with_priority(10),
    ));
    let road_x = ROAD_X as f64;
    commands.spawn((
        Name::new("Fire demo: gravel road"),
        place(road_x, 0.0),
        FuelPatch::firebreak(PatchShape::Rect {
            half_extents: [ROAD_HALF, MEADOW[1] + BORDER],
        })
        .with_priority(30),
    ));
    let wet = (-4.0, -11.0, [5.0f32, 3.5f32]);
    commands.spawn((
        Name::new("Fire demo: soaked patch"),
        place(wet.0, wet.1),
        FuelPatch::grass(PatchShape::Rect {
            half_extents: wet.2,
        })
        .with_load(0.5)
        .with_moisture(0.6)
        .with_regrow(110.0)
        .with_priority(30),
    ));
    let camp = (-22.0, -20.5);
    commands.spawn((
        Name::new("Fire demo: campfire pad"),
        place(camp.0, camp.1),
        FuelPatch::firebreak(PatchShape::Disc { radius: 2.5 }).with_priority(30),
    ));

    // Ground: meadow litter, a dusty mown border, the gravel road with its wheel ruts and
    // shoulders, the soaked patch and the campfire's pad, blended with ragged edges.
    let rect = |hx: f32, hz: f32| PatchShape::Rect {
        half_extents: [hx, hz],
    };
    let road_len = MEADOW[1] + BORDER + 2.0;
    let rx = road_x as f32;
    commands.spawn((
        Name::new("Fire demo: ground"),
        place(0.0, 0.0),
        PaintedGround {
            half_extents: [FOOTPRINT[0] + 2.0, FOOTPRINT[1] + 2.0],
            base: GroundKind::Dirt,
            edge_m: 2.5,
            paints: vec![
                GroundPaint::new(GroundKind::Litter, [0.0, 0.0], rect(MEADOW[0], MEADOW[1]))
                    .with_edge(1.4, 0.7),
                GroundPaint::new(GroundKind::Dirt, [rx, 0.0], rect(2.3, road_len))
                    .with_edge(0.9, 0.45),
                GroundPaint::new(GroundKind::Gravel, [rx, 0.0], rect(1.45, road_len))
                    .with_edge(0.5, 0.22),
                GroundPaint::new(GroundKind::Dirt, [rx - 0.75, 0.0], rect(0.22, road_len))
                    .with_edge(0.35, 0.08)
                    .with_strength(0.55),
                GroundPaint::new(GroundKind::Dirt, [rx + 0.75, 0.0], rect(0.22, road_len))
                    .with_edge(0.35, 0.08)
                    .with_strength(0.55),
                GroundPaint::new(
                    GroundKind::Mud,
                    [wet.0 as f32, wet.1 as f32],
                    rect(wet.2[0], wet.2[1]),
                )
                .with_edge(1.2, 0.6),
                GroundPaint::new(
                    GroundKind::Dirt,
                    [camp.0 as f32, camp.1 as f32],
                    PatchShape::Disc { radius: 2.4 },
                )
                .with_edge(0.9, 0.45),
                GroundPaint::new(
                    GroundKind::Ash,
                    [camp.0 as f32, camp.1 as f32],
                    PatchShape::Disc { radius: 1.25 },
                )
                .with_edge(0.6, 0.25)
                .with_strength(0.45),
                GroundPaint::new(
                    GroundKind::Ash,
                    [camp.0 as f32, camp.1 as f32],
                    PatchShape::Disc { radius: 0.62 },
                )
                .with_edge(0.25, 0.1),
            ],
            ..Default::default()
        },
    ));

    // Grass: a dense meadow of blades that burns with the fire field (lush in the soaked
    // patch, none on the road or the campfire pad).
    commands.spawn((
        Name::new("Fire demo: grass"),
        place(0.0, 0.0),
        GrassMeadow {
            half_extents: MEADOW,
            seed: 0xF12E,
            ..Default::default()
        },
    ));

    let object = |c: Combustible| DemoObject { demo, initial: c };
    // Props get their meshes and textures once the world has asset stores (it has none in
    // headless tests); see `dress_props`.
    let mut props: Vec<(Entity, Prop)> = Vec::new();

    // Trees: the trunk catches from the grass, then the crown. Each is grown from its seed:
    // the trunk entity carries the wood (trunk, limbs and twigs), the crown entity the leaves.
    for (i, (x, z)) in [(-9.0, 9.0), (-3.0, 13.0), (3.0, 7.5), (8.0, 12.5)]
        .into_iter()
        .enumerate()
    {
        let scale = rng.range(0.85, 1.15);
        let trunk_h = 4.2 * scale;
        let crown_r = 2.3 * scale;
        let crown_y = trunk_h + crown_r * 0.55;
        let tree = TreeParams {
            height: trunk_h + crown_r * 1.55,
            crown_base: trunk_h - crown_r * 0.8,
            crown_radius: crown_r * 1.15,
            trunk_radius: 0.26 * scale,
            seed: 0x7EE + i as u64 * 31,
        };
        let yaw = Quat::from_rotation_y(Rng::new(tree.seed).range(0.0, std::f32::consts::TAU));
        let trunk = Combustible {
            fuel_kg: 60.0,
            moisture: 0.35,
            ignition_c: 320.0,
            ..Default::default()
        };
        let trunk_entity = commands
            .spawn((
                Name::new(format!("Fire demo: tree {i} trunk")),
                Transform::from_translation(at(x, trunk_h as f64 * 0.5, z)).with_rotation(yaw),
                trunk,
                object(trunk),
                FlameShape {
                    radius: 0.5,
                    height: trunk_h,
                    offset: [0.0, -trunk_h * 0.5, 0.0],
                },
                Charring::default(),
            ))
            .id();
        props.push((
            trunk_entity,
            Prop::TreeWood {
                tree,
                foot: -trunk_h * 0.5,
            },
        ));
        let crown = Combustible {
            fuel_kg: 40.0,
            moisture: 0.3,
            ignition_c: 300.0,
            ..Default::default()
        };
        let crown_entity = commands
            .spawn((
                Name::new(format!("Fire demo: tree {i} crown")),
                Transform::from_translation(at(x, crown_y as f64, z)).with_rotation(yaw),
                crown,
                object(crown),
                FlameShape {
                    radius: crown_r,
                    height: crown_r * 2.2,
                    offset: [0.0, -crown_r * 0.9, 0.0],
                },
                Charring {
                    shrink: 0.3,
                    char_color: [0.03, 0.025, 0.02],
                    ..Default::default()
                },
                Foliage,
            ))
            .id();
        props.push((
            crown_entity,
            Prop::TreeLeaves {
                tree,
                foot: -crown_y,
            },
        ));
    }

    // A timber shed in the fire's path: four board walls (the front one with a door) and a
    // corrugated iron roof, each burning on its own.
    let (sx, sz) = (4.0, -3.0);
    let wall = Combustible {
        fuel_kg: 45.0,
        moisture: 0.12,
        ignition_c: 300.0,
        ..Default::default()
    };
    for i in 0..4 {
        let (center, ..) = shed_wall_frame(i);
        let (dx, dz) = (center.x as f64, center.z as f64);
        let e = commands
            .spawn((
                Name::new(format!("Fire demo: shed wall {i}")),
                Transform::from_translation(at(sx + dx, 1.2, sz + dz)),
                wall,
                object(wall),
                FlameShape {
                    radius: 1.3,
                    height: 2.8,
                    offset: [0.0, -1.2, 0.0],
                },
                Charring::default(),
            ))
            .id();
        props.push((e, Prop::ShedWall(i)));
    }
    let roof = Combustible {
        fuel_kg: 40.0,
        ..wall
    };
    let e = commands
        .spawn((
            Name::new("Fire demo: shed roof"),
            Transform::from_translation(at(sx, SHED_ROOF_Y as f64, sz))
                .with_rotation(Quat::from_rotation_x(SHED_ROOF_TILT)),
            roof,
            object(roof),
            FlameShape {
                radius: 1.8,
                height: 1.8,
                offset: [0.0, 0.0, 0.0],
            },
            Charring::default(),
        ))
        .id();
    props.push((e, Prop::ShedRoof));

    // Round hay bales: some in the fire's path, some safe across the road. (Their origin is
    // at their foot, so they slump onto the ground as they burn down.)
    let bale = Combustible {
        fuel_kg: 25.0,
        moisture: 0.08,
        ignition_c: 260.0,
        ..Default::default()
    };
    for (i, (x, z)) in [
        (8.0, -11.0),
        (9.8, -9.6),
        (8.4, -8.0),
        (20.0, 3.0),
        (21.8, 4.2),
        (20.4, 5.8),
    ]
    .into_iter()
    .enumerate()
    {
        let e = commands
            .spawn((
                Name::new(format!("Fire demo: hay bale {i}")),
                Transform::from_translation(at(x, 0.0, z))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, std::f32::consts::PI))),
                bale,
                object(bale),
                FlameShape {
                    radius: 0.8,
                    height: 1.8,
                    offset: [0.0, 0.0, 0.0],
                },
                Charring {
                    shrink: 0.6,
                    // Loose straw: its outer layer is black within seconds of catching.
                    char_secs: 5.0,
                    ..Default::default()
                },
            ))
            .id();
        props.push((e, Prop::Bale(i as u32 % 3)));
    }

    // The campfire: a ring of fieldstones around split logs and charcoal, tended forever.
    let stones = commands
        .spawn((
            Name::new("Fire demo: campfire stones"),
            Transform::from_translation(at(camp.0, 0.0, camp.1)),
        ))
        .id();
    props.push((stones, Prop::Stones));
    let wood = commands
        .spawn((
            Name::new("Fire demo: campfire wood"),
            Transform::from_translation(at(camp.0, 0.0, camp.1)),
        ))
        .id();
    props.push((wood, Prop::Firewood));
    commands.queue(move |world: &mut World| dress_props(world, &props));
    commands.spawn((
        Name::new("Fire demo: campfire"),
        Transform::from_translation(at(camp.0, 0.05, camp.1)),
        Combustible {
            fuel_kg: 0.4,
            moisture: 0.05,
            ignition_c: 250.0,
            ..Default::default()
        },
        FlameShape {
            radius: 0.35,
            height: 1.1,
            offset: [0.0; 3],
        },
        ContainedFire { perpetual: true },
    ));
}

/// What a demo prop entity looks like (see [`dress_props`]).
#[derive(Debug, Clone, Copy)]
enum Prop {
    /// Trunk and limbs, the entity `foot` m above the tree's foot.
    TreeWood {
        tree: TreeParams,
        foot: f32,
    },
    /// The leaves, the entity `foot` m above the tree's foot.
    TreeLeaves {
        tree: TreeParams,
        foot: f32,
    },
    ShedWall(usize),
    ShedRoof,
    Bale(u32),
    Stones,
    Firewood,
}

/// Give the demo's props their meshes and materials (generated once and shared by key).
/// Does nothing without asset stores (headless tests).
fn dress_props(world: &mut World, props: &[(Entity, Prop)]) {
    if !world.contains_resource::<Assets<Image>>() || !world.contains_resource::<Assets<Mesh>>() {
        return;
    }
    let mats =
        world.resource_scope(|_, mut images: Mut<Assets<Image>>| prop_materials(&mut images));
    let mut renderers = Vec::with_capacity(props.len());
    {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        let mut keyed =
            |key: String, make: &dyn Fn() -> Mesh| meshes.get_or_insert_keyed(&key, make);
        for &(entity, prop) in props {
            let (mesh, material) = match prop {
                Prop::TreeWood { tree, foot } | Prop::TreeLeaves { tree, foot } => {
                    let wood = matches!(prop, Prop::TreeWood { .. });
                    let part = if wood { "wood" } else { "leaves" };
                    let key = format!(
                        "fire/tree/{}/{}/{part}",
                        tree.seed,
                        (tree.height * 100.0).round()
                    );
                    let mesh = keyed(key, &|| {
                        let grown = grow_tree(&tree);
                        let (mut m, y) = if wood {
                            (grown.wood, foot)
                        } else {
                            (grown.leaves, grown.crown_center.y + foot)
                        };
                        m.transform(&Mat4::from_translation(Vec3::new(0.0, y, 0.0)));
                        m
                    });
                    (mesh, if wood { mats.bark } else { mats.leaves })
                }
                Prop::ShedWall(i) => {
                    let mesh = keyed(format!("fire/shed/wall{i}"), &|| {
                        let (center, ..) = shed_wall_frame(i);
                        let mut m = shed_wall(i);
                        m.transform(&Mat4::from_translation(-(center + Vec3::Y * 1.2)));
                        m
                    });
                    (mesh, mats.planks)
                }
                Prop::ShedRoof => (keyed("fire/shed/roof".into(), &shed_roof), mats.metal),
                Prop::Bale(v) => (
                    keyed(format!("fire/bale/{v}"), &|| hay_bale(0xBA1E + v)),
                    mats.straw,
                ),
                Prop::Stones => (
                    keyed("fire/campfire/stones".into(), &|| {
                        campfire_stones(0.8, 13, 0x570E)
                    }),
                    mats.granite,
                ),
                Prop::Firewood => (
                    keyed("fire/campfire/wood".into(), &|| campfire_wood(0x10C5)),
                    mats.firewood,
                ),
            };
            renderers.push((entity, MeshRenderer::new(mesh, material)));
        }
    }
    for (entity, renderer) in renderers {
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.insert(renderer);
        }
    }
}

/// Where a demo is: its origin and how it is turned. The layout is built along the world axes,
/// but a world shift can turn the demo with the world.
#[derive(Debug, Clone, Copy)]
struct Place {
    origin: DVec3,
    rotation: DQuat,
}

impl Place {
    fn of(t: &GlobalTransform) -> Self {
        Self {
            origin: t.translation(),
            rotation: t.rotation().as_dquat(),
        }
    }

    /// World position of a point given relative to the demo (X/Z on its ground).
    fn world(&self, x: f64, z: f64) -> DVec3 {
        self.origin + self.rotation * DVec3::new(x, 0.0, z)
    }

    /// Demo-relative X/Z of a world point.
    fn local(&self, p: DVec3) -> DVec2 {
        let l = self.rotation.inverse() * (p - self.origin);
        DVec2::new(l.x, l.z)
    }
}

/// Run every [`FireDemo`]: first fire (fast-forwarded by `warmup_secs`), containment and the
/// reset-and-relight loop. (The grass burns down and grows back with the field on its own:
/// [`crate::grass::update_grass`].)
pub fn run_demos(world: &mut World) {
    let mut q = world.query::<(Entity, &GlobalTransform, &FireDemo, &FireDemoState)>();
    let demos: Vec<(Entity, Place, FireDemo, FireDemoState)> = q
        .iter(world)
        .map(|(e, t, d, s)| (e, Place::of(t), d.clone(), *s))
        .collect();
    if demos.is_empty() {
        return;
    }
    let dt = world.resource::<Time>().delta_secs();
    for (entity, place, demo, mut state) in demos {
        state.frames += 1;
        // Wait for transforms to settle so objects are where they belong.
        if state.frames < 3 {
            world.entity_mut(entity).insert(state);
            continue;
        }
        if let Some(at) = state.jump_to.take() {
            if state.started {
                reset(world, entity, place, &demo);
            }
            state.started = true;
            state.cycles += 1;
            world.resource_mut::<FireClock>().reset_for_demo();
            light(world, place, &demo);
            if at > 0.0 {
                fast_forward(world, place.origin, reach(&demo), at);
            }
            state.since = at;
            let field = world.resource::<FireField>();
            let wind = world.get_resource::<WeatherState>().map(|w| w.wind);
            tracing::info!(
                "fire demo: a fresh fire {at:.0} s along, {:.0} m² burning, wind {wind:?}",
                field.stats.burning_m2
            );
        } else if !state.started {
            state.started = true;
            state.cycles = 1;
            light(world, place, &demo);
            if demo.warmup_secs > 0.0 {
                fast_forward(world, place.origin, reach(&demo), demo.warmup_secs);
            }
            state.since = demo.warmup_secs;
        } else {
            // A demo far from everyone rests with its fire: its cycle waits too, so no one
            // pays for relighting a meadow they can't see.
            if world.resource::<FireClock>().awake.contains(place.origin) {
                state.since += dt;
            }
            if state.since >= demo.cycle_secs {
                reset(world, entity, place, &demo);
                light(world, place, &demo);
                state.since = 0.0;
                state.cycles += 1;
            }
        }
        contain(world, place, &demo);
        world.entity_mut(entity).insert(state);
    }
}

/// Where a fire starts (demo-relative X/Z): `demo.line`, or with `demo.upwind` the edge of the
/// meadow west of the road that faces the prevailing wind.
fn ignition_line(world: &World, place: Place, demo: &FireDemo) -> [[f32; 2]; 2] {
    let wind = world
        .get_resource::<WeatherState>()
        .map_or(Vec3::ZERO, |w| w.wind);
    let w = (place.rotation.inverse() * wind.as_dvec3()).as_vec3();
    if !demo.upwind || w.x.hypot(w.z) < 0.5 {
        return demo.line;
    }
    // The meadow's burnable part, a little in from its edges.
    let (west, east) = (-MEADOW[0] + 2.0, ROAD_X - ROAD_HALF - 2.0);
    let (south, north) = (-MEADOW[1] + 3.0, MEADOW[1] - 3.0);
    if w.x.abs() >= w.z.abs() {
        let x = if w.x > 0.0 { west } else { east };
        [[x, south], [x, north]]
    } else {
        let z = if w.z > 0.0 { south } else { north };
        [[west, z], [east, z]]
    }
}

fn light(world: &mut World, place: Place, demo: &FireDemo) {
    let line = ignition_line(world, place, demo);
    let [a, b] = line.map(|p| place.world(p[0] as f64, p[1] as f64));
    let n = (a.distance(b) / 1.0).ceil().max(1.0) as usize;
    let mut commands = world.resource_mut::<sim::FireCommands>();
    for i in 0..=n {
        commands.0.push(sim::FireCommand::Ignite {
            at: a.lerp(b, i as f64 / n as f64),
            radius: 0.8,
            temperature_c: 900.0,
        });
    }
}

/// Have every demo start a fresh fire `at_secs` along (clamped to its cycle) at the next
/// update; returns how many demos there are.
pub fn jump_to(world: &mut World, at_secs: f32) -> usize {
    let mut q = world.query::<(&FireDemo, &mut FireDemoState)>();
    let mut n = 0;
    for (demo, mut state) in q.iter_mut(world) {
        state.jump_to = Some(at_secs.clamp(0.0, (demo.cycle_secs - 1.0).max(0.0)));
        n += 1;
    }
    n
}

/// How far from a demo's center its fires reach: its bounds plus the margin [`contain`] allows
/// escaped embers.
fn reach(demo: &FireDemo) -> f64 {
    Vec2::from(demo.bounds).length() as f64 + CONTAIN_MARGIN_M
}

/// Run the fire within `radius` of `origin` alone for `secs` of simulated time, right now
/// (fires elsewhere, such as a burning building across the valley, are left where they are).
fn fast_forward(world: &mut World, origin: DVec3, radius: f64, secs: f32) {
    let hz = world.resource::<FireSettings>().sim_hz.clamp(1.0, 240.0);
    let steps = (secs * hz).round() as usize;
    let mut schedule = Schedule::default();
    schedule.add_systems(
        (
            sim::index_patches,
            sim::handle_ignition,
            sim::step_field,
            sim::step_objects,
        )
            .chain(),
    );
    let focus = std::mem::replace(
        &mut world.resource_mut::<FocusPoints>().points,
        vec![origin],
    );
    let sim_radius = std::mem::replace(
        &mut world.resource_mut::<FireSettings>().sim_radius_m,
        radius as f32,
    );
    {
        let mut clock = world.resource_mut::<FireClock>();
        clock.fast_forward = true;
        clock.scope = Some((origin, radius));
    }
    // The wind the fire sees while it jumps ahead: the prevailing wind with its gusts from a
    // fixed start, and none of the old fire's updrafts, so the jump burns the same way whenever
    // it is made. The world's own wind is put back afterwards.
    let wind = world.get_resource::<WindField>().cloned();
    if let Some(mut w) = world.get_resource_mut::<WindField>() {
        w.time = 0.0;
        w.local.data.fill(Vec3::ZERO);
    }
    for _ in 0..steps {
        schedule.run(world);
        if let Some(mut w) = world.get_resource_mut::<WindField>() {
            w.time += 1.0 / hz;
        }
    }
    if let Some(wind) = wind {
        world.insert_resource(wind);
    }
    {
        let mut clock = world.resource_mut::<FireClock>();
        clock.fast_forward = false;
        clock.scope = None;
    }
    world.resource_mut::<FireSettings>().sim_radius_m = sim_radius;
    world.resource_mut::<FocusPoints>().points = focus;
}

/// Put the meadow and its objects back as they were.
fn reset(world: &mut World, demo_entity: Entity, place: Place, demo: &FireDemo) {
    let mut q = world.query::<(Entity, &DemoObject)>();
    let objects: Vec<(Entity, Combustible)> = q
        .iter(world)
        .filter(|(_, o)| o.demo == demo_entity)
        .map(|(e, o)| (e, o.initial))
        .collect();
    for (e, initial) in objects {
        let mut entity = world.entity_mut(e);
        entity.insert(initial);
        entity.remove::<Burning>();
        if let Some(base) = entity.get::<CharBase>().copied() {
            if let Some(mut r) = entity.get_mut::<MeshRenderer>() {
                r.material = engine_render::MaterialRef::Inline(base.material);
            }
            if let Some(mut t) = entity.get_mut::<Transform>() {
                t.scale = base.scale;
            }
        }
    }
    // The grass forgets the old fire (its blades would otherwise take a minute to stand up
    // again and turn green, and keep the old fire's glow for a while); the next update rebuilds
    // the map from the reset field.
    let mut meadows = world.query_filtered::<(Entity, &GlobalTransform), With<GrassBurn>>();
    let meadows: Vec<Entity> = meadows
        .iter(world)
        .filter(|(_, t)| t.translation().distance(place.origin) <= reach(demo))
        .map(|(e, _)| e)
        .collect();
    for e in meadows {
        world.entity_mut(e).remove::<GrassBurn>();
    }
    // Embers still flying from the old fire would light spot fires on the fresh meadow, and its
    // lights would flicker on for a frame.
    let near = |p: DVec3| p.distance(place.origin) <= reach(demo);
    let mut embers = world.resource_mut::<Embers>();
    embers.list.retain(|e| !near(e.position));
    embers.landings.retain(|(p, _)| !near(*p));
    world
        .resource_mut::<sim::FireLights>()
        .targets
        .retain(|t| !near(t.position));
    let half = DVec2::new(demo.bounds[0] as f64, demo.bounds[1] as f64);
    let center = DVec2::new(place.origin.x, place.origin.z);
    // Turned or not, the bounds fit in this square.
    let reach = DVec2::splat(half.length());
    world.resource_scope(|world, mut field: Mut<FireField>| {
        let settings = world.resource::<FireSettings>();
        let mut fuel = WorldFuel {
            ground: world.resource::<Ground>(),
            water: world.resource::<Water>(),
            materials: world.resource::<SurfaceMaterials>(),
            patches: &world.resource::<FirePatches>().list,
            ground_fuel: settings.ground_fuel,
        };
        let inside = |p: DVec3| {
            let d = place.local(DVec3::new(p.x, place.origin.y, p.z));
            d.x.abs() <= half.x && d.y.abs() <= half.y
        };
        field.reset_area(center - reach, center + reach, &inside, &mut fuel);
    });
}

/// How far past its bounds a demo puts out fires that escaped it, meters.
const CONTAIN_MARGIN_M: f64 = 40.0;

/// Put out anything burning just outside the demo (embers that jumped the border).
fn contain(world: &mut World, place: Place, demo: &FireDemo) {
    let half = Vec2::from(demo.bounds).as_dvec2();
    let margin = CONTAIN_MARGIN_M;
    let mut field = world.resource_mut::<FireField>();
    let field = &mut *field;
    let cell = field.cell_m as f64;
    let tile_size = crate::field::TILE as f64 * cell;
    // Half a tile's diagonal: a tile is entirely within this of its center, however turned.
    let r = tile_size * std::f64::consts::FRAC_1_SQRT_2;
    let field_origin = field.origin;
    for tile in &mut field.tiles {
        if tile.burning == 0 && !tile.active {
            continue;
        }
        let o = field_origin + (tile.key * crate::field::TILE).as_dvec2() * cell;
        let c = o + DVec2::splat(tile_size * 0.5);
        let mid = place.local(DVec3::new(c.x, place.origin.y, c.y));
        // Only tiles near this demo, and only ones reaching past its bounds.
        if mid.x.abs() - r > half.x + margin || mid.y.abs() - r > half.y + margin {
            continue;
        }
        if mid.x.abs() + r <= half.x && mid.y.abs() + r <= half.y {
            continue;
        }
        let mut doused = false;
        for (i, c) in tile.cells.iter_mut().enumerate() {
            if c.burn <= 0.0 && c.glow <= 0.0 && c.heat <= 0.0 {
                continue;
            }
            let x = (i as i32 % crate::field::TILE) as f64;
            let y = (i as i32 / crate::field::TILE) as f64;
            let w = o + DVec2::new(x + 0.5, y + 0.5) * cell;
            let p = place.local(DVec3::new(w.x, place.origin.y, w.y));
            if p.x.abs() > half.x || p.y.abs() > half.y {
                c.burn = 0.0;
                c.glow = 0.0;
                c.heat = 0.0;
                c.temp = 0.0;
                c.hrr = 0.0;
                doused = true;
            }
        }
        if doused {
            tile.version += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FirePlugin;
    use engine_world::BurnState;

    /// The demo in a `wind` after `fire.demo` started a fresh fire `at` seconds along.
    fn jumped(at: f32, wind: Vec3) -> (App, DVec3) {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(FirePlugin);
        app.world.resource_mut::<engine_world::WeatherState>().wind = wind;
        let origin = DVec3::new(100.0, 0.0, -50.0);
        {
            let mut commands = app.world.commands();
            spawn_demo(&mut commands, origin);
        }
        app.world.flush();
        app.update_n(5);
        let r = engine_core::registry::call_api(
            &mut app.world,
            "fire.demo",
            serde_json::json!({ "at_secs": at }),
        )
        .unwrap();
        assert_eq!(r["demos"], 1);
        app.update_n(2);
        (app, origin)
    }

    #[test]
    fn fire_demo_starts_a_fresh_fire_that_far_along() {
        use crate::field::CellState::*;
        // Just lit: the meadow ahead of the line is untouched.
        let (mut app, origin) = jumped(0.0, Vec3::new(3.0, 0.0, 1.0));
        let state = |app: &mut App, x: f64| {
            app.world
                .resource::<FireField>()
                .cell_at(origin + DVec3::new(x, 0.0, 0.0))
                .map(|c| c.state())
        };
        assert_eq!(state(&mut app, -8.0), Some(Intact));
        let mut q = app.world.query::<&FireDemoState>();
        let s = *q.single(&app.world).unwrap();
        assert!(s.since < 0.2 && s.cycles == 2, "{s:?}");
        // A minute along, the front has crossed it.
        let (mut app, _) = jumped(60.0, Vec3::new(3.0, 0.0, 1.0));
        assert!(
            matches!(state(&mut app, -8.0), Some(Burning | Smoldering | Burnt)),
            "{:?}",
            state(&mut app, -8.0)
        );
        // Jumping back resets the meadow: burnt ground is grass again.
        engine_core::registry::call_api(
            &mut app.world,
            "fire.demo",
            serde_json::json!({ "at_secs": 0 }),
        )
        .unwrap();
        app.update_n(2);
        assert_eq!(state(&mut app, -8.0), Some(Intact));
    }

    #[test]
    fn a_fresh_fire_looks_the_same_whatever_burnt_before() {
        use crate::field::CellState;
        let wind = Vec3::new(3.0, 0.0, 1.0);
        // What the meadow shows: each cell's state, how many embers fly and how burnt the
        // grass blades look.
        let look = |app: &mut App, origin: DVec3| {
            let field = app.world.resource::<FireField>();
            let mut counts = [0usize; 5];
            for x in -40..=40 {
                for z in -30..=30 {
                    let p = origin + DVec3::new(x as f64, 0.0, z as f64);
                    if let Some(c) = field.cell_at(p) {
                        counts[c.state() as usize] += 1;
                    }
                }
            }
            let embers = app.world.resource::<Embers>().list.len();
            let mut q = app.world.query::<&GrassBurn>();
            let (n, stubble) = q.iter(&app.world).fold((0, 0), |(n, s), g| {
                let short = g.cells.iter().filter(|c| c.height < 0.5).count();
                (n + g.cells.len(), s + short)
            });
            (counts, embers, n, stubble)
        };
        let (mut fresh, origin) = jumped(30.0, wind);
        let want = look(&mut fresh, origin);
        // A fire well along, with embers in the air and the grass burnt, then the same jump.
        let (mut after, _) = jumped(90.0, wind);
        after.update_n(60);
        assert!(!after.world.resource::<Embers>().list.is_empty());
        engine_core::registry::call_api(
            &mut after.world,
            "fire.demo",
            serde_json::json!({ "at_secs": 30.0 }),
        )
        .unwrap();
        after.update_n(2);
        let got = look(&mut after, origin);
        // (Intact and bare cells are left out: the bigger fire made the field cover more
        // ground, which is all intact grass again.)
        assert_eq!(got.0[2..], want.0[2..], "cells burning, smoldering, burnt");
        assert_eq!(got.1, want.1, "embers");
        assert_eq!((got.2, got.3), (want.2, want.3), "grass cells, short ones");
        assert!(want.0[CellState::Burning as usize] > 0);
    }

    #[test]
    fn the_fire_runs_across_the_meadow_whichever_way_the_wind_blows() {
        use crate::field::CellState::*;
        for wind in [
            Vec3::new(-3.0, 0.0, 0.8),
            Vec3::new(0.8, 0.0, 3.0),
            Vec3::new(-0.5, 0.0, -3.0),
        ] {
            let (app, origin) = jumped(60.0, wind);
            let mid = app
                .world
                .resource::<FireField>()
                .cell_at(origin + DVec3::new(0.0, 0.0, 0.0))
                .map(|c| c.state());
            assert!(
                matches!(mid, Some(Burning | Smoldering | Burnt)),
                "wind {wind}: mid-meadow {mid:?}"
            );
        }
    }

    #[test]
    fn demo_burns_the_meadow_but_not_past_the_road() {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(FirePlugin);
        app.world.resource_mut::<engine_world::WeatherState>().wind = Vec3::new(3.0, 0.0, 1.0);
        let origin = DVec3::new(100.0, 0.0, -50.0);
        {
            let mut commands = app.world.commands();
            spawn_demo(&mut commands, origin);
        }
        app.world.flush();
        // Warm-up happens on the third frame, then a minute of live fire.
        for _ in 0..(60 * 30) {
            app.update();
        }
        let field = app.world.resource::<FireField>();
        let state = |x: f64, z: f64| {
            field
                .cell_at(origin + DVec3::new(x, 0.0, z))
                .map(|c| c.state())
        };
        use crate::field::CellState::*;
        // Burnt behind the ignition line, burning or burnt mid-meadow.
        assert!(
            matches!(state(-20.0, 0.0), Some(Burnt | Smoldering)),
            "{:?}",
            state(-20.0, 0.0)
        );
        // The soaked patch survives.
        assert_eq!(state(-4.0, -11.0), Some(Intact));
        // Across the road the grass is untouched.
        assert!(
            matches!(state(22.0, -10.0), None | Some(Intact)),
            "{:?}",
            state(22.0, -10.0)
        );
        // Nothing burns outside the border.
        assert!(matches!(state(-40.0, 0.0), None | Some(Intact | Bare)));
        // The campfire burns.
        let mut q = app.world.query::<(&Name, &Combustible)>();
        let campfire = q
            .iter(&app.world)
            .find(|(n, _)| n.as_str() == "Fire demo: campfire")
            .unwrap()
            .1;
        assert_eq!(campfire.state, BurnState::Burning);
        // At least one tree trunk caught.
        let trees = q
            .iter(&app.world)
            .filter(|(n, c)| n.as_str().contains("trunk") && c.state != BurnState::Intact)
            .count();
        assert!(trees > 0, "no tree caught");
    }

    #[test]
    fn demo_follows_a_turned_world() {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(FirePlugin);
        let origin = DVec3::new(100.0, 0.0, -50.0);
        {
            let mut commands = app.world.commands();
            spawn_demo(&mut commands, origin);
        }
        app.world.flush();
        let mut demos = app.world.query::<&mut FireDemo>();
        demos.single_mut(&mut app.world).unwrap().warmup_secs = 0.0;
        app.update_n(5);
        // A quarter turn: the meadow's long side now runs along world Z.
        let s = engine_world::WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            translation: DVec3::new(500.5, 0.0, -300.25),
            ..engine_world::WorldShift::IDENTITY
        };
        app.world.write_message(s);
        app.update();
        // Light the far end of the meadow (inside the bounds) and the grass beyond the border.
        let at = |x: f64, z: f64| s.point(origin + DVec3::new(x, 0.0, z));
        let (inside, outside) = (at(25.0, 0.0), at(0.0, 30.0));
        app.world
            .resource_mut::<sim::FireCommands>()
            .0
            .extend([inside, outside].map(|at| sim::FireCommand::Ignite {
                at,
                radius: 1.0,
                temperature_c: 900.0,
            }));
        app.update_n(3);
        let field = app.world.resource::<FireField>();
        let burning = |p: DVec3| field.cell_at(p).is_some_and(|c| c.is_burning());
        assert!(burning(inside), "the meadow was put out");
        assert!(
            !burning(outside),
            "a fire outside the border was left burning"
        );
    }
}
