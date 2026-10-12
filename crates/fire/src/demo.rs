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

use engine_core::math::{DVec2, DVec3, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::{Combustible, FocusPoints, Ground, SurfaceMaterials, Water};

use crate::field::FireField;
use crate::fuel::{FuelPatch, PatchShape};
use crate::objects::{Burning, Charring, ContainedFire, FlameShape};
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
        hour: 19.6,
    },
    DemoView {
        key: "fire_campfire",
        name: "Campfire at dusk",
        eye: [-27.5, 1.5, -25.5],
        target: [-22.0, 0.6, -20.5],
        hour: 20.4,
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
        eye: [-40.0, 14.0, -50.0],
        target: [0.0, 0.0, 0.0],
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

/// A grass blade that burns down to stubble and grows back with the fire field's fuel.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoGrass {
    pub height: f32,
    pub ground_y: f64,
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

    // Ground: dirt border, gravel road and pad, dark wet patch.
    let dirt = StandardMaterial::hex(0x6b5843).with_roughness(1.0);
    let gravel = StandardMaterial::hex(0x8d877c).with_roughness(0.95);
    let soaked = StandardMaterial::hex(0x2f4423).with_roughness(0.22);
    let (hx, hz) = (MEADOW[0] as f64, MEADOW[1] as f64);
    let b = BORDER as f64;
    for (x, z, sx, sz) in [
        (0.0, hz + b * 0.5, 2.0 * (hx + b), b),
        (0.0, -hz - b * 0.5, 2.0 * (hx + b), b),
        (hx + b * 0.5, 0.0, b, 2.0 * hz),
        (-hx - b * 0.5, 0.0, b, 2.0 * hz),
    ] {
        commands.spawn((
            Transform::from_translation(at(x, 0.01, z)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [sx as f32, 0.02, sz as f32],
                },
                dirt,
            ),
        ));
    }
    commands.spawn((
        Transform::from_translation(at(road_x, 0.02, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 0.04, 2.0 * (MEADOW[1] + BORDER)],
            },
            gravel,
        ),
    ));
    commands.spawn((
        Transform::from_translation(at(camp.0, 0.02, camp.1)),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 2.5,
                height: 0.04,
                segments: 32,
            },
            gravel,
        ),
    ));
    commands.spawn((
        Transform::from_translation(at(wet.0, 0.015, wet.1)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [wet.2[0] * 2.0, 0.02, wet.2[1] * 2.0],
            },
            soaked,
        ),
    ));

    // Grass blades: dry and straw-colored in the meadow, lush in the soaked patch.
    let dry = [
        StandardMaterial::hex(0x9c9548),
        StandardMaterial::hex(0xb3a35e),
        StandardMaterial::hex(0x7f8a3e),
    ]
    .map(|m| {
        m.with_roughness(0.85)
            .with_subsurface(0.45)
            .with_wind_sway(0.22)
    });
    let lush = StandardMaterial::hex(0x3f7a2c)
        .with_roughness(0.6)
        .with_subsurface(0.5)
        .with_wind_sway(0.2);
    let blades = (MEADOW[0] * MEADOW[1] * 4.0 * 1.6) as usize;
    for i in 0..blades {
        let x = rng.range(-MEADOW[0], MEADOW[0]) as f64;
        let z = rng.range(-MEADOW[1], MEADOW[1]) as f64;
        if (x - road_x).abs() < 1.7 || DVec2::new(x - camp.0, z - camp.1).length() < 2.7 {
            continue;
        }
        let in_wet = (x - wet.0).abs() < wet.2[0] as f64 && (z - wet.1).abs() < wet.2[1] as f64;
        let h = rng.range(0.45, 1.0) * if in_wet { 0.8 } else { 1.0 };
        let material = if in_wet { lush } else { dry[i % dry.len()] };
        commands.spawn((
            Transform::from_translation(at(x, h as f64 * 0.5, z)).with_rotation(
                Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU))
                    * Quat::from_rotation_z(rng.range(-0.2, 0.2)),
            ),
            MeshRenderer::new(
                Shape::Cone {
                    radius: 0.05,
                    height: h,
                    segments: 4,
                },
                material,
            ),
            DemoGrass {
                height: h,
                ground_y: origin.y,
            },
        ));
    }

    let object = |c: Combustible| DemoObject { demo, initial: c };

    // Trees: the trunk catches from the grass, then the crown.
    let bark = StandardMaterial::hex(0x5a4030).with_roughness(0.9);
    let leaves = [0x3f6b2a, 0x557f2c, 0x4a6f26].map(|hex| {
        StandardMaterial::hex(hex)
            .with_roughness(0.7)
            .with_subsurface(0.45)
            .with_wind_sway(0.3)
    });
    for (i, (x, z)) in [(-9.0, 9.0), (-3.0, 13.0), (3.0, 7.5), (8.0, 12.5)]
        .into_iter()
        .enumerate()
    {
        let scale = rng.range(0.85, 1.15);
        let trunk_h = 4.2 * scale;
        let crown_r = 2.3 * scale;
        let trunk = Combustible {
            fuel_kg: 60.0,
            moisture: 0.35,
            ignition_c: 320.0,
            ..Default::default()
        };
        commands.spawn((
            Name::new(format!("Fire demo: tree {i} trunk")),
            Transform::from_translation(at(x, trunk_h as f64 * 0.5, z)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.22 * scale,
                    height: trunk_h,
                    segments: 10,
                },
                bark,
            ),
            trunk,
            object(trunk),
            FlameShape {
                radius: 0.5,
                height: trunk_h,
                offset: [0.0, -trunk_h * 0.5, 0.0],
            },
            Charring::default(),
        ));
        let crown = Combustible {
            fuel_kg: 40.0,
            moisture: 0.3,
            ignition_c: 300.0,
            ..Default::default()
        };
        let crown_y = trunk_h as f64 + crown_r as f64 * 0.55;
        commands.spawn((
            Name::new(format!("Fire demo: tree {i} crown")),
            Transform::from_translation(at(x, crown_y, z)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: crown_r,
                    segments: 20,
                },
                leaves[i % leaves.len()],
            ),
            crown,
            object(crown),
            FlameShape {
                radius: crown_r,
                height: crown_r * 2.2,
                offset: [0.0, -crown_r * 0.9, 0.0],
            },
            Charring {
                shrink: 0.55,
                char_color: [0.03, 0.025, 0.02],
            },
        ));
    }

    // A wooden shed in the fire's path: four walls and a roof that burn on their own.
    let wood = StandardMaterial::hex(0x8a6239).with_roughness(0.8);
    let (sx, sz) = (4.0, -3.0);
    let wall = Combustible {
        fuel_kg: 45.0,
        moisture: 0.12,
        ignition_c: 300.0,
        ..Default::default()
    };
    for (i, (dx, dz, size)) in [
        (0.0, -1.5, [3.0, 2.4, 0.12]),
        (0.0, 1.5, [3.0, 2.4, 0.12]),
        (-1.5, 0.0, [0.12, 2.4, 3.0]),
        (1.5, 0.0, [0.12, 2.4, 3.0]),
    ]
    .into_iter()
    .enumerate()
    {
        commands.spawn((
            Name::new(format!("Fire demo: shed wall {i}")),
            Transform::from_translation(at(sx + dx, 1.2, sz + dz)),
            MeshRenderer::new(Shape::Cuboid { size }, wood),
            wall,
            object(wall),
            FlameShape {
                radius: 1.3,
                height: 2.8,
                offset: [0.0, -1.2, 0.0],
            },
            Charring::default(),
        ));
    }
    let roof = Combustible {
        fuel_kg: 40.0,
        ..wall
    };
    commands.spawn((
        Name::new("Fire demo: shed roof"),
        Transform::from_translation(at(sx, 2.46, sz)).with_rotation(Quat::from_rotation_x(0.08)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.5, 0.12, 3.6],
            },
            wood,
        ),
        roof,
        object(roof),
        FlameShape {
            radius: 1.8,
            height: 1.8,
            offset: [0.0, 0.0, 0.0],
        },
        Charring::default(),
    ));

    // Hay bales: some in the fire's path, some safe across the road.
    let straw = StandardMaterial::hex(0xc9ad5a).with_roughness(0.95);
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
        commands.spawn((
            Name::new(format!("Fire demo: hay bale {i}")),
            Transform::from_translation(at(x, 0.6, z)).with_rotation(
                Quat::from_rotation_y(rng.range(0.0, std::f32::consts::PI))
                    * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            ),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.6,
                    height: 1.3,
                    segments: 16,
                },
                straw,
            ),
            bale,
            object(bale),
            FlameShape {
                radius: 0.8,
                height: 1.8,
                offset: [0.0, -0.6, 0.0],
            },
            Charring {
                shrink: 0.6,
                ..Default::default()
            },
        ));
    }

    // The campfire: a ring of stones around a few logs, tended forever.
    let stone = StandardMaterial::hex(0x77726b).with_roughness(0.85);
    let char_log = StandardMaterial::hex(0x2a1e16)
        .with_roughness(0.9)
        .with_emissive([1.0, 0.3, 0.06], 30.0);
    for k in 0..10 {
        let a = k as f64 / 10.0 * std::f64::consts::TAU;
        let r = rng.range(0.15, 0.22);
        commands.spawn((
            Transform::from_translation(at(
                camp.0 + a.cos() * 0.75,
                r as f64 * 0.6,
                camp.1 + a.sin() * 0.75,
            ))
            .with_scale(Vec3::new(1.0, 0.7, 1.0)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: r,
                    segments: 10,
                },
                stone,
            ),
        ));
    }
    for k in 0..4 {
        let yaw = k as f32 * std::f32::consts::FRAC_PI_2 + 0.4;
        let o = Quat::from_rotation_y(yaw) * Vec3::new(0.22, 0.0, 0.0);
        commands.spawn((
            Transform::from_translation(at(camp.0 + o.x as f64, 0.28, camp.1 + o.z as f64))
                .with_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_z(0.75)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.06,
                    height: 0.8,
                    segments: 8,
                },
                char_log,
            ),
        ));
    }
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

/// Run every [`FireDemo`]: first fire (fast-forwarded by `warmup_secs`), containment, grass
/// that burns down and grows back, and the reset-and-relight loop.
pub fn run_demos(world: &mut World) {
    let mut q = world.query::<(Entity, &GlobalTransform, &FireDemo, &FireDemoState)>();
    let demos: Vec<(Entity, DVec3, FireDemo, FireDemoState)> = q
        .iter(world)
        .map(|(e, t, d, s)| (e, t.translation(), d.clone(), *s))
        .collect();
    if demos.is_empty() {
        return;
    }
    let dt = world.resource::<Time>().delta_secs();
    for (entity, origin, demo, mut state) in demos {
        state.frames += 1;
        // Wait for transforms to settle so objects are where they belong.
        if state.frames < 3 {
            world.entity_mut(entity).insert(state);
            continue;
        }
        if !state.started {
            state.started = true;
            state.cycles = 1;
            light(world, origin, &demo);
            if demo.warmup_secs > 0.0 {
                fast_forward(world, origin, demo.warmup_secs);
            }
            state.since = demo.warmup_secs;
        } else {
            state.since += dt;
            if state.since >= demo.cycle_secs {
                reset(world, entity, origin, &demo);
                light(world, origin, &demo);
                state.since = 0.0;
                state.cycles += 1;
            }
        }
        contain(world, origin, &demo);
        world.entity_mut(entity).insert(state);
    }
    grow_grass(world);
}

fn light(world: &mut World, origin: DVec3, demo: &FireDemo) {
    let [a, b] = demo
        .line
        .map(|p| origin + DVec3::new(p[0] as f64, 0.0, p[1] as f64));
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
fn reset(world: &mut World, demo_entity: Entity, origin: DVec3, demo: &FireDemo) {
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
    let center = DVec2::new(origin.x, origin.z);
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
            let d = DVec2::new(p.x, p.z) - center;
            d.x.abs() <= half.x && d.y.abs() <= half.y
        };
        field.reset_area(center - half, center + half, &inside, &mut fuel);
    });
}

/// Put out anything burning just outside the demo (embers that jumped the border).
fn contain(world: &mut World, origin: DVec3, demo: &FireDemo) {
    let half = Vec2::from(demo.bounds).as_dvec2();
    let margin = 40.0;
    let center = DVec2::new(origin.x, origin.z);
    let mut field = world.resource_mut::<FireField>();
    let cell = field.cell_m as f64;
    let tile_size = crate::field::TILE as f64 * cell;
    for tile in &mut field.tiles {
        if tile.burning == 0 && !tile.active {
            continue;
        }
        let o = tile.key.as_dvec2() * tile_size;
        let lo = o - center;
        let hi = lo + DVec2::splat(tile_size);
        // Only tiles near this demo, and only ones reaching past its bounds.
        if hi.x < -half.x - margin
            || lo.x > half.x + margin
            || hi.y < -half.y - margin
            || lo.y > half.y + margin
        {
            continue;
        }
        if lo.x >= -half.x && hi.x <= half.x && lo.y >= -half.y && hi.y <= half.y {
            continue;
        }
        let mut doused = false;
        for (i, c) in tile.cells.iter_mut().enumerate() {
            if c.burn <= 0.0 && c.glow <= 0.0 && c.heat <= 0.0 {
                continue;
            }
            let x = (i as i32 % crate::field::TILE) as f64;
            let y = (i as i32 / crate::field::TILE) as f64;
            let p = o + DVec2::new(x + 0.5, y + 0.5) * cell - center;
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

/// Burnt grass shrinks to stubble; regrown grass stands up again.
fn grow_grass(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let mut q = world.query::<(&DemoGrass, &mut Transform)>();
    world.resource_scope(|world, field: Mut<FireField>| {
        if field.tiles.is_empty() {
            return;
        }
        for (grass, mut t) in q.iter_mut(world) {
            let Some(c) = field.cell_at(t.translation) else {
                continue;
            };
            if c.load <= 0.0 {
                continue;
            }
            let left = (c.fuel / c.load).clamp(0.0, 1.0);
            let target = if c.state() == crate::field::CellState::Intact {
                left
            } else {
                0.12 + 0.88 * left * left
            };
            let s = t.scale.y;
            // Burning grass collapses fast; regrowth springs up over a few seconds.
            let rate = if target < s { 3.0 } else { 0.5 };
            let next = s + (target - s) * (rate * dt).min(1.0);
            if (next - s).abs() > 0.002 {
                t.scale.y = next;
                t.translation.y = grass.ground_y + (grass.height * next) as f64 * 0.5;
            }
        }
    });
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
}
