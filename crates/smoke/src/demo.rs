//! The smoke showcase: a firing range that fills with smoke, again and again.
//!
//! What to look for:
//!
//! - Smoke grenades pop: a thick white screen in the middle of the range and red, green and
//!   violet signal smoke by the targets. Warm smoke rises a little, spreads and drifts with
//!   the wind, and the colors mix where the clouds meet.
//! - The turret fires bursts through the white screen at the targets: every bullet punches a
//!   clean tunnel that you can see through for a moment, then the smoke flows back in.
//! - A blast in the middle of the cloud blows a hole in it and leaves a dark, hot puff that
//!   rises out of it.
//! - The fans along the side wall switch on and push the cloud across the range.
//! - The cart on its track drives through the smoke and stirs it.
//! - The steam vent sends up a hot white plume that bends with the wind.
//! - The sun lights the smoke and casts shadows through it; the back of the cloud is darker.
//!
//! The first round is already `SmokeDemo::warmup_secs` along when the demo starts, so the
//! smoke is there straight away.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{DQuat, DVec3, Quat, Rng, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::{ProjectileTrace, SmokeEmission, WorldImpulse};

use crate::sim::{self, SmokeFan, SmokeObstacle, SmokeSource, SmokeVolume};

/// Half extents of the range's concrete pad (m): it fits a 60 m-wide flat area.
pub const FOOTPRINT: [f32; 2] = [19.0, 22.0];

/// A camera spot for looking at the demo: eye and target relative to the demo's origin, the
/// hour of day it looks best at, and how long after a restart (`smoke.demo` with
/// `{"restart": true}`) its moment comes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup_secs: f32,
}

/// Suggested viewpoints (camera bookmarks and screenshot-tour stops).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "smoke_bullets",
        name: "Bullets through a smoke screen",
        eye: [0.3, 2.3, -20.6],
        target: [0.0, 1.7, 8.0],
        hour: 10.5,
        warmup_secs: 3.5,
    },
    DemoView {
        key: "smoke_range",
        name: "Smoke range",
        eye: [-27.0, 13.0, -30.0],
        target: [1.0, 2.0, 4.0],
        hour: 15.0,
        warmup_secs: 3.5,
    },
    DemoView {
        key: "smoke_signal",
        name: "Signal smoke",
        eye: [-16.0, 2.2, 8.0],
        target: [4.0, 3.2, 13.0],
        hour: 9.5,
        warmup_secs: 1.0,
    },
    DemoView {
        key: "smoke_fans",
        name: "Fans blowing smoke",
        eye: [26.0, 7.0, -14.0],
        target: [0.0, 2.0, 3.0],
        hour: 12.5,
        warmup_secs: 25.5,
    },
];

/// The smoke-range demo, centered on its `Transform`. Tune it live through the AI bridge.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeDemo {
    /// Seconds per round: grenades, shooting, a blast, fans.
    pub cycle_secs: f32,
    /// How far into the first round the demo starts, seconds.
    pub warmup_secs: f32,
    /// Rounds per second while the turret fires.
    pub fire_rate: f32,
    /// When the turret fires, seconds into the round (from, to).
    pub shooting: [f32; 2],
    /// When the blast goes off, seconds into the round.
    pub blast_at: f32,
    /// When the fans run, seconds into the round (from, to).
    pub fans: [f32; 2],
}

impl Default for SmokeDemo {
    fn default() -> Self {
        Self {
            cycle_secs: 50.0,
            warmup_secs: 9.0,
            fire_rate: 9.0,
            shooting: [8.0, 18.0],
            blast_at: 20.0,
            fans: [27.0, 40.0],
        }
    }
}

/// Where a demo is in its loop.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SmokeDemoState {
    frames: u32,
    started: bool,
    /// Seconds into the current round.
    pub since: f32,
    /// Rounds so far.
    pub rounds: u32,
    shot_clock: f32,
    shots: u32,
    blasted: bool,
    restart: bool,
}

impl SmokeDemoState {
    /// Start the round over on the next frame: the demo's smoke is cleared and the grenades
    /// pop again, `SmokeDemo::warmup_secs` along.
    pub fn restart(&mut self) {
        self.restart = true;
    }
}

/// A grenade the demo pops again every round.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoGrenade {
    pub demo: Entity,
    pub fresh: SmokeSource,
}

/// A fan the demo switches on and off.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoFan {
    pub demo: Entity,
}

/// The cart that drives back and forth through the smoke.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoCart {
    /// Track ends: where the cart is at each end, world position.
    pub from: DVec3,
    pub to: DVec3,
    /// m/s.
    pub speed: f32,
    pub phase: f32,
}

/// A tracer: a short-lived glowing streak along a bullet's path.
#[derive(Component, Debug, Clone, Copy)]
pub struct Tracer {
    pub life: f32,
}

/// Where the turret's shots come from and go, relative to the demo.
const MUZZLE: [f64; 3] = [0.0, 1.55, -18.2];
const TARGET_Z: f64 = 16.0;
const TARGET_Y: f64 = 1.8;
const TARGETS_X: [f64; 5] = [-12.0, -6.0, 0.0, 6.0, 12.0];
/// Where each burst aims (target X): through the thick of the white screen.
const AIM_X: [f64; 5] = [0.0, 3.0, -3.0, 1.5, -1.5];
const BLAST: [f64; 3] = [0.0, 0.5, 1.0];

/// Build the smoke range around `origin`. It needs about 38 × 44 m of flat ground.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let demo = commands
        .spawn((
            Name::new("Smoke demo"),
            Transform::from_translation(origin),
            SmokeDemo::default(),
            SmokeDemoState::default(),
        ))
        .id();
    let at = |x: f64, y: f64, z: f64| Transform::from_translation(origin + DVec3::new(x, y, z));
    let [hx, hz] = FOOTPRINT;

    // The air over the range: 0.5 m voxels, 14 m high.
    commands.spawn((
        Name::new("Smoke demo: air"),
        at(0.0, 0.0, 0.0),
        SmokeVolume {
            size: [hx * 2.0, 14.0, hz * 2.0],
            cell_m: 0.5,
        },
    ));

    let concrete = StandardMaterial::hex(0x8a8781).with_roughness(0.92);
    let dark_concrete = StandardMaterial::hex(0x5f5d59).with_roughness(0.95);
    let metal = StandardMaterial::hex(0x3d4247)
        .with_roughness(0.45)
        .with_metallic(0.8);
    let olive = StandardMaterial::hex(0x4d5a2e).with_roughness(0.6);
    let wood = StandardMaterial::hex(0x6e4e33).with_roughness(0.85);
    let paper = StandardMaterial::hex(0xe8e4d8).with_roughness(0.9);
    let red = StandardMaterial::hex(0xb3302a).with_roughness(0.7);
    let cart_paint = StandardMaterial::hex(0xc9772b).with_roughness(0.55);
    let mesh = |shape: Shape, material: StandardMaterial| MeshRenderer::new(shape, material);
    let cuboid = |x: f32, y: f32, z: f32| Shape::Cuboid { size: [x, y, z] };
    let cylinder = |radius: f32, height: f32| Shape::Cylinder {
        radius,
        height,
        segments: 24,
    };

    // Pad, firing wall with the turret, back wall.
    commands.spawn((
        at(0.0, 0.05, 0.0),
        mesh(cuboid(hx * 2.0, 0.1, hz * 2.0), concrete),
    ));
    commands.spawn((
        at(0.0, 0.55, -19.5),
        mesh(cuboid(26.0, 1.0, 0.8), dark_concrete),
    ));
    commands.spawn((
        at(0.0, 3.0, hz as f64 - 0.5),
        mesh(cuboid(hx * 2.0, 6.0, 1.0), dark_concrete),
    ));
    let [mx, my, mz] = MUZZLE;
    commands.spawn((
        Name::new("Smoke demo: turret"),
        at(mx, my - 0.35, mz - 1.3),
        mesh(cylinder(0.45, 0.5), metal),
    ));
    commands.spawn((at(mx, my, mz - 1.0), mesh(cuboid(0.5, 0.35, 0.9), olive)));
    commands.spawn((
        at(mx, my, mz - 0.35).with_rotation(Quat::from_rotation_x(FRAC_PI_2)),
        mesh(cylinder(0.05, 0.9), metal),
    ));

    // Targets.
    for x in TARGETS_X {
        commands.spawn((
            at(x, 0.9, TARGET_Z + 0.1),
            mesh(cuboid(0.12, 1.8, 0.12), wood),
        ));
        commands.spawn((
            at(x, TARGET_Y, TARGET_Z).with_rotation(Quat::from_rotation_x(FRAC_PI_2)),
            mesh(cylinder(0.6, 0.05), paper),
        ));
        commands.spawn((
            at(x, TARGET_Y, TARGET_Z - 0.03).with_rotation(Quat::from_rotation_x(FRAC_PI_2)),
            mesh(cylinder(0.25, 0.04), red),
        ));
    }

    // Grenades: the white screen and the signal colors by the targets.
    let grenades = [
        ([-2.5, 0.15, 1.0], {
            let mut g = SmokeSource::grenade([0.92, 0.92, 0.9], 24.0);
            g.rate_kg_s = 0.12;
            g.radius = 0.6;
            g.temperature_c = 32.0;
            g.gas_m3_s = 22.0;
            g
        }),
        (
            [-9.0, 0.15, 12.0],
            warm(SmokeSource::grenade([0.85, 0.12, 0.08], 28.0)),
        ),
        (
            [0.0, 0.15, 13.5],
            warm(SmokeSource::grenade([0.45, 0.16, 0.7], 28.0)),
        ),
        (
            [9.0, 0.15, 12.0],
            warm(SmokeSource::grenade([0.12, 0.7, 0.18], 28.0)),
        ),
    ];
    for (i, (p, source)) in grenades.into_iter().enumerate() {
        let mut idle = source;
        idle.active = false;
        commands.spawn((
            Name::new(format!("Smoke demo: grenade {i}")),
            at(p[0], p[1], p[2]),
            mesh(cylinder(0.06, 0.22), olive),
            idle,
            DemoGrenade {
                demo,
                fresh: source,
            },
        ));
    }

    // Steam vent: always on.
    commands.spawn((
        Name::new("Smoke demo: steam vent"),
        at(14.0, 0.6, -10.0),
        mesh(cylinder(0.25, 1.1), metal),
        SmokeSource {
            rate_kg_s: 0.012,
            radius: 0.25,
            temperature_c: 100.0,
            velocity: [0.0, 4.0, 0.0],
            color: [0.97, 0.97, 0.97],
            ..Default::default()
        },
    ));

    // Fans along the west side, blowing east across the range.
    for z in [-6.0, 2.0, 10.0] {
        commands.spawn((at(-17.6, 1.0, z), mesh(cuboid(0.3, 2.0, 0.3), metal)));
        commands.spawn((
            Name::new("Smoke demo: fan"),
            at(-17.4, 2.4, z).with_rotation(Quat::from_rotation_y(-FRAC_PI_2)),
            mesh(
                Shape::Torus {
                    radius: 1.2,
                    tube: 0.12,
                    segments: 24,
                },
                metal,
            ),
            SmokeFan {
                speed: 9.0,
                radius: 2.4,
                reach: 26.0,
                on: false,
            },
            DemoFan { demo },
        ));
    }

    // The cart and its rails.
    let (rail_z, rail_from, rail_to) = (5.5, -15.0, 15.0);
    for dz in [-0.7, 0.7] {
        commands.spawn((
            at(0.0, 0.14, rail_z + dz),
            mesh(cuboid((rail_to - rail_from) as f32 + 3.0, 0.08, 0.1), metal),
        ));
    }
    commands.spawn((
        Name::new("Smoke demo: cart"),
        at(rail_from, 1.0, rail_z),
        mesh(cuboid(3.2, 1.6, 2.0), cart_paint),
        SmokeObstacle { radius: 1.8 },
        DemoCart {
            from: origin + DVec3::new(rail_from, 1.0, rail_z),
            to: origin + DVec3::new(rail_to, 1.0, rail_z),
            speed: 3.5,
            phase: 0.0,
        },
    ));
}

/// Signal smoke burns hotter (it rises and spreads more).
fn warm(mut s: SmokeSource) -> SmokeSource {
    s.temperature_c = 70.0;
    s.rate_kg_s = 0.05;
    s.radius = 0.4;
    s
}

/// Where a demo is: its origin and how it is turned. The range is built along the world
/// axes, but a world shift can turn it with the world.
#[derive(Debug, Clone, Copy)]
struct Place {
    origin: DVec3,
    rotation: DQuat,
}

impl Place {
    /// World position of a point given relative to the demo.
    fn world(&self, local: DVec3) -> DVec3 {
        self.origin + self.rotation * local
    }

    /// A direction given relative to the demo, in world axes.
    fn direction(&self, local: Vec3) -> Vec3 {
        (self.rotation * local.as_dvec3()).as_vec3()
    }
}

/// Drive every smoke demo: rounds of grenades, shooting, a blast and fans; the cart; tracers.
pub fn run_demos(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    age_tracers(world, dt);
    drive_carts(world, dt);
    let mut q = world.query::<(Entity, &GlobalTransform, &SmokeDemo, &SmokeDemoState)>();
    let demos: Vec<(Entity, Place, SmokeDemo, SmokeDemoState)> = q
        .iter(world)
        .map(|(e, t, d, s)| {
            let place = Place {
                origin: t.translation(),
                rotation: t.rotation().as_dquat(),
            };
            (e, place, d.clone(), *s)
        })
        .collect();
    for (entity, place, demo, mut state) in demos {
        let middle = place.world(DVec3::Y);
        if state.restart {
            clear_demo(world, middle);
            state = SmokeDemoState {
                frames: 2,
                ..Default::default()
            };
        }
        state.frames += 1;
        // Wait until transforms have settled and the volume exists.
        let ready = state.frames >= 3
            && world
                .resource::<sim::SmokeDomains>()
                .list
                .iter()
                .any(|d| d.owner.is_some() && d.domain.contains(middle));
        if !state.started {
            if ready {
                state.started = true;
                state.rounds = 1;
                pop_grenades(world, entity);
                if demo.warmup_secs > 0.0 {
                    sim::fast_forward(world, demo.warmup_secs);
                }
                state.since = demo.warmup_secs;
            }
            world.entity_mut(entity).insert(state);
            continue;
        }
        state.since += dt;
        if state.since >= demo.cycle_secs {
            state.since = 0.0;
            state.rounds += 1;
            state.blasted = false;
            pop_grenades(world, entity);
        }
        let t = state.since;
        // Shooting: one-second bursts with short pauses, sweeping across the targets.
        if t >= demo.shooting[0] && t < demo.shooting[1] && demo.fire_rate > 0.0 {
            let burst = ((t - demo.shooting[0]) / 1.6) as usize;
            let in_burst = (t - demo.shooting[0]) % 1.6 < 1.0;
            if in_burst {
                state.shot_clock += dt;
                let gap = 1.0 / demo.fire_rate;
                while state.shot_clock >= gap {
                    state.shot_clock -= gap;
                    state.shots += 1;
                    let target = AIM_X[(burst + state.rounds as usize) % AIM_X.len()];
                    shoot(world, place, target, state.shots);
                }
            } else {
                state.shot_clock = 0.0;
            }
        }
        if t >= demo.blast_at && !state.blasted {
            state.blasted = true;
            blast(world, place);
        }
        let fans_on = t >= demo.fans[0] && t < demo.fans[1];
        let mut fans = world.query::<(&DemoFan, &mut SmokeFan)>();
        for (f, mut fan) in fans.iter_mut(world) {
            if f.demo == entity && fan.on != fans_on {
                fan.on = fans_on;
            }
        }
        world.entity_mut(entity).insert(state);
    }
}

/// Clear the smoke and bullet tunnels in the volume around `middle` (the demo's).
fn clear_demo(world: &mut World, middle: DVec3) {
    let mut cleared = Vec::new();
    for d in world.resource_mut::<sim::SmokeDomains>().list.iter_mut() {
        if d.owner.is_some() && d.domain.contains(middle) {
            d.domain.clear();
            cleared.push((d.domain.origin, d.domain.origin + d.domain.size()));
        }
    }
    let inside = |p: DVec3| {
        cleared
            .iter()
            .any(|(lo, hi)| p.cmpge(*lo).all() && p.cmplt(*hi).all())
    };
    world
        .resource_mut::<sim::SmokeTunnels>()
        .list
        .retain(|t| !inside(t.from) && !inside(t.to));
}

fn pop_grenades(world: &mut World, demo: Entity) {
    let mut q = world.query::<(&DemoGrenade, &mut SmokeSource)>();
    for (g, mut s) in q.iter_mut(world) {
        if g.demo == demo {
            *s = g.fresh;
        }
    }
}

fn shoot(world: &mut World, place: Place, target_x: f64, n: u32) {
    let mut rng = Rng::new(0x5_0E + n as u64 * 7919);
    let from = place.world(DVec3::from_array(MUZZLE));
    let to = place.world(DVec3::new(
        target_x + rng.range(-0.5, 0.5) as f64,
        TARGET_Y + rng.range(-0.4, 0.4) as f64,
        TARGET_Z,
    ));
    world.write_message(ProjectileTrace {
        from,
        to,
        radius: 0.012,
        speed: 900.0,
        shooter: None,
    });
    // A tracer streak part-way along the path (a bullet crosses the range in 1/25 s).
    let dir = (to - from).normalize();
    let len = 4.0;
    let along = rng.range(0.25, 0.75) as f64 * (from.distance(to) - len);
    let mid = from + dir * (along + len * 0.5);
    let rotation = Quat::from_rotation_arc(Vec3::Z, dir.as_vec3());
    world.spawn((
        Transform::from_translation(mid).with_rotation(rotation),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.035, 0.035, len as f32],
            },
            StandardMaterial::hex(0xffb347).with_emissive([1.0, 0.55, 0.2], 9000.0),
        )
        .without_shadows(),
        Tracer { life: 0.05 },
    ));
}

fn blast(world: &mut World, place: Place) {
    let center = place.world(DVec3::from_array(BLAST));
    world.write_message(WorldImpulse {
        center,
        radius: 7.0,
        strength: 24.0,
        energy_j: 1.5e6,
        heat_c: 0.0,
        source: None,
    });
    world.write_message(SmokeEmission {
        position: center + place.direction(Vec3::Y).as_dvec3(),
        velocity: place.direction(Vec3::new(0.0, 3.0, 0.0)),
        radius: 2.0,
        mass_kg: 0.35,
        temperature_c: 260.0,
        darkness: 0.85,
    });
}

fn age_tracers(world: &mut World, dt: f32) {
    let mut q = world.query::<(Entity, &mut Tracer)>();
    let mut dead = Vec::new();
    for (e, mut t) in q.iter_mut(world) {
        t.life -= dt;
        if t.life <= 0.0 {
            dead.push(e);
        }
    }
    for e in dead {
        world.despawn(e);
    }
}

fn drive_carts(world: &mut World, dt: f32) {
    let mut q = world.query::<(&mut DemoCart, &mut Transform)>();
    for (mut cart, mut t) in q.iter_mut(world) {
        let span = cart.to.distance(cart.from).max(0.1) as f32;
        cart.phase = (cart.phase + dt * cart.speed / span) % 2.0;
        // There and back, easing at the ends.
        let s = 0.5 - 0.5 * (cart.phase * PI).cos();
        t.translation = cart.from.lerp(cart.to, s as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SmokePlugin;
    use crate::sim::{SmokeDomains, SmokeTunnels};
    use serde_json::json;

    #[test]
    fn the_range_fills_with_smoke_that_bullets_cut_and_fans_push() {
        engine_core::tasks::init_task_pools();
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 20.0,
            });
        app.add_plugins(SmokePlugin);
        // No wind, so only the fans move the cloud sideways.
        app.world.resource_mut::<engine_world::WeatherState>().wind = Vec3::ZERO;
        let origin = DVec3::new(100.0, 0.0, 50.0);
        {
            let mut commands = app.world.commands();
            spawn_demo(&mut commands, origin);
        }
        app.world.flush();
        // Coarser air than the showcase's, to keep the test quick.
        let mut volumes = app.world.query::<&mut SmokeVolume>();
        for mut v in volumes.iter_mut(&mut app.world) {
            v.cell_m = 1.0;
        }
        app.update_n(4);
        let screen = origin + DVec3::new(0.0, 1.5, 1.0);
        let state = |app: &mut App| {
            let mut q = app.world.query::<&SmokeDemoState>();
            *q.single(&app.world).unwrap()
        };
        assert!(state(&mut app).started);
        // Warmed up: the white screen is already thick.
        let domains = app.world.resource::<SmokeDomains>();
        assert_eq!(domains.list.len(), 1);
        let ext = 3000.0;
        let across = |a: f64, b: f64| {
            (
                origin + DVec3::new(a, 1.5, 1.0),
                origin + DVec3::new(b, 1.5, 1.0),
            )
        };
        let (a, b) = across(-8.0, 8.0);
        assert!(
            domains.transmittance(a, b, ext) < 0.05,
            "screen too thin: {} (mass {} kg, at screen {})",
            domains.transmittance(a, b, ext),
            domains.mass_kg(),
            domains.density(screen)
        );
        assert!(domains.density(screen) > 0.0);
        // Shooting starts: tunnels appear.
        let mut tunnels = 0;
        while state(&mut app).since < 14.0 {
            app.update();
            tunnels = tunnels.max(app.world.resource::<SmokeTunnels>().list.len());
        }
        assert!(tunnels > 3, "{tunnels}");
        // Fans push the cloud east.
        let centroid = |app: &App| {
            let d = &app.world.resource::<SmokeDomains>().list[0].domain;
            let mut sum = 0.0;
            let mut total = 0.0;
            for z in 0..d.dims.z {
                for y in 0..d.dims.y {
                    for x in 0..d.dims.x {
                        let m = d.voxels[d.index(x, y, z)].density as f64;
                        sum += d.voxel_center(x, y, z).x * m;
                        total += m;
                    }
                }
            }
            sum / total.max(1e-12)
        };
        while state(&mut app).since < 27.0 {
            app.update();
        }
        let before = centroid(&app);
        while state(&mut app).since < 37.0 {
            app.update();
        }
        let after = centroid(&app);
        assert!(after > before + 1.0, "{before} -> {after}");
        let mut q = app.world.query::<&SmokeFan>();
        assert!(q.iter(&app.world).all(|f| f.on));
        // A restart (what the screenshot tour does on arrival) starts the round over.
        engine_core::registry::call_api(&mut app.world, "smoke.demo", json!({"restart": true}))
            .unwrap();
        app.update_n(2);
        let s = state(&mut app);
        assert!(s.since < 10.0 && s.rounds == 1, "{s:?}");
        let mut q = app.world.query::<&SmokeFan>();
        assert!(q.iter(&app.world).all(|f| !f.on));
    }
}
