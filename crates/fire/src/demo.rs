//! The fire showcase: a dry meadow that burns downwind and grows back, again and again.
//!
//! What to look for:
//!
//! - The flame front runs across the grass with the wind, slower on its flanks and barely
//!   creeping back upwind, leaving black stubble and glowing coals behind it.
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

use engine_assets::Assets;
use engine_core::math::{DQuat, DVec2, DVec3, Mat4, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, MeshRenderer};
use engine_world::{Combustible, FocusPoints, Ground, SurfaceMaterials, Water};

use crate::field::FireField;
use crate::fuel::{FuelPatch, PatchShape};
use crate::grass::GrassMeadow;
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
/// Width of the mown border around it (m).
const BORDER: f32 = 4.0;
/// Half extents of the ground the demo covers (m).
pub const FOOTPRINT: [f32; 2] = [MEADOW[0] + BORDER, MEADOW[1] + BORDER];

/// A camera spot for looking at the demo: eye and target relative to the demo's origin, and
/// the hour of day it looks best at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
}

/// Suggested viewpoints (camera bookmarks and screenshot-tour stops).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "fire_front",
        name: "Grass fire front",
        eye: [6.0, 2.4, -20.0],
        target: [-4.0, 1.2, -4.0],
        hour: 18.6,
    },
    DemoView {
        key: "fire_trees",
        name: "Trees and shed burning",
        eye: [22.0, 4.0, -14.0],
        target: [2.0, 3.0, 4.0],
        hour: 18.9,
    },
    DemoView {
        key: "fire_campfire",
        name: "Campfire at dusk",
        eye: [-19.4, 1.3, -18.0],
        target: [-22.5, 0.35, -21.1],
        hour: 19.15,
    },
    DemoView {
        key: "fire_plume",
        name: "Smoke plume over the meadow",
        eye: [-58.0, 9.0, -52.0],
        target: [12.0, 12.0, 6.0],
        hour: 15.5,
    },
    DemoView {
        key: "fire_night",
        name: "Meadow fire at night",
        eye: [17.0, 2.0, -12.0],
        target: [0.0, 2.0, 4.0],
        hour: 22.5,
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
    let road_x = 14.0;
    commands.spawn((
        Name::new("Fire demo: gravel road"),
        place(road_x, 0.0),
        FuelPatch::firebreak(PatchShape::Rect {
            half_extents: [1.5, MEADOW[1] + BORDER],
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
        if !state.started {
            state.started = true;
            state.cycles = 1;
            light(world, place, &demo);
            if demo.warmup_secs > 0.0 {
                fast_forward(world, place.origin, demo.warmup_secs);
            }
            state.since = demo.warmup_secs;
        } else {
            state.since += dt;
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

fn light(world: &mut World, place: Place, demo: &FireDemo) {
    let [a, b] = demo.line.map(|p| place.world(p[0] as f64, p[1] as f64));
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

/// Run the fire alone for `secs` of simulated time, right now.
fn fast_forward(world: &mut World, origin: DVec3, secs: f32) {
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
    world.resource_mut::<FireClock>().fast_forward = true;
    for _ in 0..steps {
        schedule.run(world);
    }
    world.resource_mut::<FireClock>().fast_forward = false;
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

/// Put out anything burning just outside the demo (embers that jumped the border).
fn contain(world: &mut World, place: Place, demo: &FireDemo) {
    let half = Vec2::from(demo.bounds).as_dvec2();
    let margin = 40.0;
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
