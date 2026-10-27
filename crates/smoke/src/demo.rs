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

use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use engine_assets::Assets;
use engine_core::math::{DQuat, DVec3, Quat, Rng, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::{Image, Mesh, MeshRenderer, Shape, StandardMaterial};
use engine_world::{LoadingBudget, ProjectileTrace, SmokeEmission, WorldImpulse};

use crate::settings::SmokeSettings;
use crate::sim::{self, SmokeFan, SmokeSource};

mod art;
mod build;
mod extras;
mod fan;
mod firing;
mod grass;
mod ground;
mod gun;
mod kit;
mod targets;
mod tex;
mod track;
mod vent;

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
        eye: [-1.2, 2.9, -23.3],
        target: [0.6, 1.5, 5.0],
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
        eye: [-24.0, 4.5, -14.0],
        target: [-4.0, 1.8, 2.0],
        hour: 12.5,
        warmup_secs: 15.5,
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
            fans: [22.0, 36.0],
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
    /// Seconds of the round's warm-up still to run (spread over a loading screen's frames).
    warming: f32,
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

/// A fan's rotor: spins up while its fan is on and coasts down when it is off.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoRotor {
    /// The fan (its `SmokeFan::on` drives the rotor).
    pub fan: Entity,
    /// Spin, radians per second.
    pub speed: f32,
    /// Angle about the fan's axis, radians.
    pub angle: f32,
}

impl Default for DemoRotor {
    fn default() -> Self {
        Self {
            fan: Entity::PLACEHOLDER,
            speed: 0.0,
            angle: 0.0,
        }
    }
}

/// A wheelset of the cart: turns as the cart moves.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoWheel {
    pub cart: Entity,
    /// Wheel radius, m.
    pub radius: f32,
    /// Angle about the axle, radians.
    pub angle: f32,
    /// How far along its track the cart was when last seen, m (None before the first
    /// frame). Measured along the track, so a world shift doesn't turn the wheels.
    pub last_along: Option<f64>,
}

impl Default for DemoWheel {
    fn default() -> Self {
        Self {
            cart: Entity::PLACEHOLDER,
            radius: 0.3,
            angle: 0.0,
            last_along: None,
        }
    }
}

/// Top speed of the fan rotors, rad/s (about 290 rpm).
const ROTOR_SPEED: f32 = 30.0;

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

/// Build the smoke range around `origin`. It needs about 45 × 52 m of flat ground (the pad is
/// 38 × 44 m; the berm and its spill reach a little past the far end and the sides).
///
/// The props are procedural meshes and textures registered once by key (`smoke/range/…`).
/// The first time, they are baked on a worker (about a second of work) and the range is built
/// a few frames later, when they are done; [`loading`] says whether that is still to come.
/// Without a renderer only the gameplay entities are made, at once.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| {
        if let Some(mut waiting) = world.get_resource_mut::<RangeArt>() {
            waiting.origins.push(origin);
            return;
        }
        let renders =
            world.contains_resource::<Assets<Image>>() && world.contains_resource::<Assets<Mesh>>();
        match AsyncComputeTaskPool::try_get() {
            Some(pool) if renders && !art::made(world) => {
                let slot = Arc::new(Mutex::new(None));
                let out = slot.clone();
                pool.spawn(async move {
                    let baked = build::bake();
                    if let Ok(mut s) = out.lock() {
                        *s = Some(baked);
                    }
                })
                .detach();
                world.insert_resource(RangeArt {
                    baked: slot,
                    origins: vec![origin],
                });
            }
            _ => build::build(world, origin),
        }
    });
}

/// The range's art baking on a worker, and where to build a range once it is done.
#[derive(Resource)]
pub struct RangeArt {
    baked: Arc<Mutex<Option<art::Baked>>>,
    origins: Vec<DVec3>,
}

/// True while a range asked for with [`spawn_demo`] waits for its art: what a loading screen
/// waits on.
pub fn loading(world: &World) -> bool {
    world.contains_resource::<RangeArt>() || world.contains_resource::<Warming>()
}

/// Present while a range's warm-up is still being spread over loading-screen frames.
#[derive(Resource)]
struct Warming;

/// Build the waiting ranges once their art is baked.
pub fn land_range_art(world: &mut World) {
    let Some(art) = world.get_resource::<RangeArt>() else {
        return;
    };
    let baked = match art.baked.lock() {
        Ok(mut b) => b.take(),
        Err(_) => None,
    };
    // A worker that died leaves the slot empty for good: build on the spot instead.
    if baked.is_none() && Arc::strong_count(&art.baked) > 1 {
        return;
    }
    let Some(waiting) = world.remove_resource::<RangeArt>() else {
        return;
    };
    let started = std::time::Instant::now();
    if let Some(baked) = baked {
        art::land(world, baked);
    }
    for origin in waiting.origins {
        build::build(world, origin);
    }
    tracing::info!(
        "smoke range: built in {:.0} ms on the frame",
        started.elapsed().as_secs_f64() * 1000.0
    );
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

/// Where a demo is, from its transform.
fn place_of(t: &GlobalTransform) -> Place {
    Place {
        origin: t.translation(),
        rotation: t.rotation().as_dquat(),
    }
}

/// Seconds into the round, whether the turret is in a burst at `t`.
fn in_burst(demo: &SmokeDemo, t: f32) -> bool {
    t >= demo.shooting[0]
        && t < demo.shooting[1]
        && demo.fire_rate > 0.0
        && (t - demo.shooting[0]) % 1.6 < 1.0
}

/// Whether the fans should run at `t` seconds into the round.
fn fans_on(demo: &SmokeDemo, t: f32) -> bool {
    t >= demo.fans[0] && t < demo.fans[1]
}

/// Whether a range's air is there to run in (transforms settled, its volume made) and awake.
fn volume_at(domains: &sim::SmokeDomains, middle: DVec3) -> Option<&sim::SmokeDomain> {
    domains
        .list
        .iter()
        .find(|d| d.owner.is_some() && d.domain.contains(middle))
}

/// Every frame, as a normal system: count each [`SmokeDemo`]'s frames and time its round. A
/// range whose air rests (no one near) waits with it. [`run_demos`] runs only when one has
/// something to do this frame ([`demos_due`]).
pub fn tick_demos(
    time: Res<Time>,
    domains: Res<sim::SmokeDomains>,
    mut q: Query<(&GlobalTransform, &SmokeDemo, &mut SmokeDemoState)>,
) {
    let dt = time.delta_secs();
    for (t, demo, mut state) in &mut q {
        // A restart counts its own frames.
        if state.restart {
            continue;
        }
        state.frames += 1;
        if !state.started || state.warming > 0.0 {
            continue;
        }
        let middle = place_of(t).world(DVec3::Y);
        if volume_at(&domains, middle).is_some_and(|d| d.resting) {
            continue;
        }
        state.since += dt;
        if !in_burst(demo, state.since) {
            state.shot_clock = 0.0;
        }
    }
}

/// Run condition for [`run_demos`]: a demo restarts, starts, warms up, starts its next
/// round, shoots, blasts or switches its fans this frame.
pub fn demos_due(
    domains: Res<sim::SmokeDomains>,
    demos: Query<(Entity, &GlobalTransform, &SmokeDemo, &SmokeDemoState)>,
    fans: Query<(&DemoFan, &SmokeFan)>,
) -> bool {
    demos.iter().any(|(e, t, demo, s)| {
        let middle = place_of(t).world(DVec3::Y);
        if s.restart || s.warming > 0.0 {
            return true;
        }
        if !s.started {
            return s.frames >= 3 && volume_at(&domains, middle).is_some();
        }
        let at = s.since;
        let fans_wrong = fans
            .iter()
            .any(|(f, fan)| f.demo == e && fan.on != fans_on(demo, at));
        at >= demo.cycle_secs
            || in_burst(demo, at)
            || (at >= demo.blast_at && !s.blasted)
            || fans_wrong
    })
}

/// Lose spent tracer lines.
pub fn age_tracers(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut Tracer)>) {
    let dt = time.delta_secs();
    for (e, mut t) in &mut q {
        t.life -= dt;
        if t.life <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

/// Run each [`SmokeDemo`] that has something to do this frame ([`demos_due`]): restarts,
/// first rounds and warm-ups, grenades, the turret's shots, the blast and the fans;
/// [`tick_demos`] times them. It needs the whole world, so it is gated: a `&mut World` system
/// that runs every frame stops every other system while it runs.
pub fn run_demos(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let mut q = world.query::<(Entity, &GlobalTransform, &SmokeDemo, &SmokeDemoState)>();
    let demos: Vec<(Entity, Place, SmokeDemo, SmokeDemoState)> = q
        .iter(world)
        .map(|(e, t, d, s)| (e, place_of(t), d.clone(), *s))
        .collect();
    for (entity, place, demo, mut state) in demos {
        let middle = place.world(DVec3::Y);
        if state.restart {
            clear_demo(world, middle);
            state = SmokeDemoState {
                frames: 3,
                ..Default::default()
            };
        }
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
                state.warming = demo.warmup_secs.max(0.0);
                state.since = demo.warmup_secs;
                warm_up(world, &mut state.warming);
            }
            world.entity_mut(entity).insert(state);
            continue;
        }
        if state.warming > 0.0 {
            warm_up(world, &mut state.warming);
            world.entity_mut(entity).insert(state);
            continue;
        }
        if state.since >= demo.cycle_secs {
            state.since = 0.0;
            state.rounds += 1;
            state.blasted = false;
            pop_grenades(world, entity);
        }
        let t = state.since;
        // Shooting: one-second bursts with short pauses, sweeping across the targets.
        if in_burst(&demo, t) {
            let burst = ((t - demo.shooting[0]) / 1.6) as usize;
            state.shot_clock += dt;
            let gap = 1.0 / demo.fire_rate;
            while state.shot_clock >= gap {
                state.shot_clock -= gap;
                state.shots += 1;
                let target = AIM_X[(burst + state.rounds as usize) % AIM_X.len()];
                shoot(world, place, target, state.shots);
            }
        }
        if t >= demo.blast_at && !state.blasted {
            state.blasted = true;
            blast(world, place);
        }
        let fans_on = fans_on(&demo, t);
        let mut fans = world.query::<(&DemoFan, &mut SmokeFan)>();
        for (f, mut fan) in fans.iter_mut(world) {
            if f.demo == entity && fan.on != fans_on {
                fan.on = fans_on;
            }
        }
        world.entity_mut(entity).insert(state);
    }
    let mut q = world.query::<&SmokeDemoState>();
    let warming = q.iter(world).any(|s| s.warming > 0.0);
    if warming != world.contains_resource::<Warming>() {
        if warming {
            world.insert_resource(Warming);
        } else {
            world.remove_resource::<Warming>();
        }
    }
}

/// Run a round's warm-up: all at once in play, or behind a loading screen as many steps as fit
/// in its frame budget (`LoadingBudget`), the rest on the next frames. A whole warm-up is a few
/// hundred steps of the range's air: about 0.3 s in one frame on a fast PC.
fn warm_up(world: &mut World, warming: &mut f32) {
    let Some(budget) = world.get_resource::<LoadingBudget>().map(|b| b.budget_ms) else {
        sim::fast_forward(world, *warming);
        *warming = 0.0;
        return;
    };
    let step = 1.0 / world.resource::<SmokeSettings>().sim_hz.clamp(1.0, 120.0);
    let started = std::time::Instant::now();
    // At least one step a frame, so a tiny budget still gets there.
    loop {
        let secs = step.min(*warming);
        sim::fast_forward(world, secs);
        *warming -= secs;
        if *warming <= 1e-4 || started.elapsed().as_secs_f64() * 1000.0 >= budget {
            break;
        }
    }
    if *warming <= 1e-4 {
        *warming = 0.0;
    }
}

/// Clear the smoke and bullet tunnels in the volume around `middle` (the demo's).
fn clear_demo(world: &mut World, middle: DVec3) {
    let mut cleared = Vec::new();
    for d in world.resource_mut::<sim::SmokeDomains>().list.iter_mut() {
        if d.owner.is_some() && d.domain.contains(middle) {
            d.change(crate::domain::Change::Clear);
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

/// Drive the carts back and forth along their tracks.
pub fn drive_carts(time: Res<Time>, mut q: Query<(&mut DemoCart, &mut Transform)>) {
    let dt = time.delta_secs();
    for (mut cart, mut t) in &mut q {
        let span = cart.to.distance(cart.from).max(0.1);
        cart.phase = (cart.phase + dt * cart.speed / span as f32) % 2.0;
        // There and back, easing at the ends.
        t.translation = cart.from.lerp(cart.to, cart_along(&cart) / span);
    }
}

/// Turn the cart's wheelsets by how far the cart moved.
pub fn turn_wheels(carts: Query<&DemoCart>, mut q: Query<(&mut DemoWheel, &mut Transform)>) {
    for (mut w, mut t) in &mut q {
        let Ok(cart) = carts.get(w.cart) else {
            continue;
        };
        let s = cart_along(cart);
        let ds = s - w.last_along.unwrap_or(s);
        w.last_along = Some(s);
        // Rolling toward `to` (+x on the range) turns the wheel clockwise seen from +z.
        w.angle =
            (w.angle - (ds / w.radius.max(0.01) as f64) as f32).rem_euclid(std::f32::consts::TAU);
        t.rotation = Quat::from_rotation_z(w.angle);
    }
}

/// How far the cart is from the `from` end of its track, m.
fn cart_along(cart: &DemoCart) -> f64 {
    let s = 0.5 - 0.5 * (cart.phase * PI).cos();
    s as f64 * cart.to.distance(cart.from)
}

/// Spin fan rotors up while their fan is on and let them coast down when it is off.
pub fn spin_rotors(
    time: Res<Time>,
    fans: Query<&SmokeFan>,
    mut q: Query<(&mut DemoRotor, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (mut r, mut t) in &mut q {
        let running = fans.get(r.fan).is_ok_and(|f| f.on);
        let (target, secs) = if running {
            (ROTOR_SPEED, 1.6)
        } else {
            (0.0, 4.0)
        };
        r.speed += (target - r.speed) * (1.0 - (-dt / secs).exp());
        if r.speed < 0.01 && !running {
            r.speed = 0.0;
        }
        r.angle = (r.angle + r.speed * dt).rem_euclid(std::f32::consts::TAU);
        t.rotation = Quat::from_rotation_z(r.angle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{SmokeDomains, SmokeTunnels, SmokeVolume};
    use crate::{SmokePlugin, SmokeSettings};
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
        // Steps on the spot, so the smoke is where the test counts on it being.
        app.world.resource_mut::<SmokeSettings>().background = false;
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
        let fans = SmokeDemo::default().fans;
        while state(&mut app).since < fans[0] {
            app.update();
        }
        let before = centroid(&app);
        while state(&mut app).since < (fans[0] + 10.0).min(fans[1] - 0.5) {
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

    /// Behind a loading screen the round's warm-up runs a budget's worth a frame (here one
    /// step), not all in the first frame, and play starts where it would have.
    #[test]
    fn behind_a_loading_screen_the_warm_up_is_spread_over_frames() {
        engine_core::tasks::init_task_pools();
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 20.0,
            });
        app.add_plugins(SmokePlugin);
        app.world.resource_mut::<SmokeSettings>().background = false;
        app.world.insert_resource(LoadingBudget { budget_ms: 0.0 });
        {
            let mut commands = app.world.commands();
            spawn_demo(&mut commands, DVec3::ZERO);
        }
        app.world.flush();
        let mut volumes = app.world.query::<&mut SmokeVolume>();
        for mut v in volumes.iter_mut(&mut app.world) {
            v.cell_m = 1.0;
        }
        let state = |app: &mut App| {
            let mut q = app.world.query::<&SmokeDemoState>();
            *q.single(&app.world).unwrap()
        };
        app.update_n(4);
        let warmup = SmokeDemo::default().warmup_secs;
        let s = state(&mut app);
        assert!(s.started && s.warming > 0.0 && s.warming < warmup, "{s:?}");
        assert_eq!(s.since, warmup);
        assert!(loading(&app.world));
        let mut frames = 0;
        while state(&mut app).warming > 0.0 {
            app.update();
            frames += 1;
            assert!(frames < 1000);
        }
        assert!(frames > 100, "{frames}");
        assert!(!loading(&app.world));
        assert_eq!(state(&mut app).since, warmup);
        assert!(app.world.resource::<SmokeDomains>().mass_kg() > 1.0);
    }
}
