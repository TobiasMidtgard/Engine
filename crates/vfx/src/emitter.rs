//! Playing effects: the [`ParticleEffect`] component, particle pool, spawn scheduling and the
//! per-frame hand-off to the GPU ([`ParticleFrame`]).
//!
//! Particles live on the GPU. The CPU only decides *how many* particles each emitter spawns
//! this frame (rates, bursts, distance traveled) and *where* in the pool they go, so the
//! simulation never needs a readback. Each emitter owns a ring of slots in one shared pool;
//! new particles overwrite the oldest, and the ring is returned to the pool once its last
//! particle has died.
//!
//! Effects follow world time: slow motion (`TimeDilation`) slows spawning and motion alike,
//! and an entity's [`LocalTimeScale`] speeds up or freezes its own effects.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::PointLight;
use engine_world::Env;

use crate::effect::{EffectDesc, EmitterDesc};
use crate::gpu::GpuEmitter;
use crate::library::EffectLibrary;
use crate::trail::Trail;

/// Plays an effect from the [`EffectLibrary`] at this entity (it follows the entity).
///
/// ```ignore
/// commands.spawn((
///     Transform::from_xyz(0.0, 1.5, 0.0),
///     ParticleEffect::new("spell.fireball"),
/// ));
/// // A one-shot effect that removes its entity when done:
/// commands.spawn((Transform::from_translation(hit), ParticleEffect::once("impact.sparks")));
/// ```
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct ParticleEffect {
    /// Effect name in the library (`"spell.fireball"`). Changing it restarts the effect.
    pub effect: String,
    /// Emitting. Setting it back to true after the effect finished plays it again.
    pub playing: bool,
    /// Size multiplier (sizes, shapes, speeds and swirls scale together).
    pub scale: f32,
    /// Particle count multiplier (density).
    pub rate: f32,
    /// Glow and light multiplier.
    pub intensity: f32,
    /// Despawn the entity once the effect has finished and its last particle died.
    pub despawn_when_done: bool,
    /// Random seed (0 = pick one).
    pub seed: u32,
}

impl Default for ParticleEffect {
    fn default() -> Self {
        Self {
            effect: String::new(),
            playing: true,
            scale: 1.0,
            rate: 1.0,
            intensity: 1.0,
            despawn_when_done: false,
            seed: 0,
        }
    }
}

impl ParticleEffect {
    /// Play `effect` here until stopped.
    pub fn new(effect: impl Into<String>) -> Self {
        Self {
            effect: effect.into(),
            ..Default::default()
        }
    }

    /// Play `effect` once and despawn the entity when it is done (impacts, splashes).
    pub fn once(effect: impl Into<String>) -> Self {
        Self {
            effect: effect.into(),
            despawn_when_done: true,
            ..Default::default()
        }
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    pub fn with_rate(mut self, rate: f32) -> Self {
        self.rate = rate;
        self
    }

    pub fn with_intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity;
        self
    }
}

/// Marks the point light the plugin spawned for an effect.
#[derive(Component, Debug, Clone, Copy)]
pub struct VfxLight;

/// Tuning for the whole particle system.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VfxSettings {
    /// Most particles alive at once across every effect (GPU memory: 64 bytes each).
    pub max_particles: u32,
    /// Global particle count multiplier (quality setting: 0.5 on low-end devices).
    pub spawn_scale: f32,
    /// Effects farther than this from the camera are not drawn, meters.
    pub draw_distance: f32,
    /// Fade particles where they meet scene geometry.
    pub soft_particles: bool,
    /// Draw heat haze (`haze` sprites bend the scene behind them).
    pub heat_haze: bool,
    /// Colliding particles also hit what the camera sees (walls, props, characters), using
    /// last frame's depth; off screen only the ground plane collides.
    pub scene_collision: bool,
    /// Draw particles at all (they keep simulating).
    pub enabled: bool,
    /// How much glow follows the camera's exposure: 1 is physical (a torch is a dim smudge
    /// by day and a white blaze at night), 0 looks the same in any light. Glow values are
    /// authored for a bright overcast day (EV100 14) and drift from there by this weight.
    pub glow_exposure: f32,
}

impl Default for VfxSettings {
    fn default() -> Self {
        Self {
            max_particles: 1 << 21,
            spawn_scale: 1.0,
            draw_distance: 5000.0,
            soft_particles: true,
            heat_haze: true,
            scene_collision: true,
            enabled: true,
            glow_exposure: 0.15,
        }
    }
}

/// A range of slots in the particle pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoolRange {
    pub base: u32,
    pub len: u32,
}

impl PoolRange {
    pub fn end(&self) -> u32 {
        self.base + self.len
    }
}

/// First-fit allocator of particle slots. The GPU buffer grows to `high_water`.
#[derive(Debug, Clone, Default)]
pub struct ParticlePool {
    /// Free ranges below `high_water`, sorted by base, never adjacent.
    free: Vec<PoolRange>,
    high_water: u32,
}

impl ParticlePool {
    /// Allocate `len` slots, or `None` if that would pass `limit`.
    pub fn alloc(&mut self, len: u32, limit: u32) -> Option<PoolRange> {
        if len == 0 {
            return None;
        }
        if let Some(i) = self.free.iter().position(|r| r.len >= len) {
            let r = self.free[i];
            if r.len == len {
                self.free.remove(i);
            } else {
                self.free[i] = PoolRange {
                    base: r.base + len,
                    len: r.len - len,
                };
            }
            return Some(PoolRange { base: r.base, len });
        }
        // Grow, reusing a free range that ends at the top.
        let (base, reuse) = match self.free.last() {
            Some(last) if last.end() == self.high_water => (last.base, true),
            _ => (self.high_water, false),
        };
        if base.checked_add(len)? > limit {
            return None;
        }
        if reuse {
            self.free.pop();
        }
        self.high_water = base + len;
        Some(PoolRange { base, len })
    }

    /// Return slots to the pool.
    pub fn free(&mut self, range: PoolRange) {
        if range.len == 0 {
            return;
        }
        let i = self.free.partition_point(|r| r.base < range.base);
        self.free.insert(i, range);
        // Merge with neighbors.
        if i + 1 < self.free.len() && self.free[i].end() == self.free[i + 1].base {
            self.free[i].len += self.free[i + 1].len;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].end() == self.free[i].base {
            self.free[i - 1].len += self.free[i].len;
            self.free.remove(i);
        }
        // Shrink the top.
        if let Some(last) = self.free.last()
            && last.end() == self.high_water
        {
            self.high_water = last.base;
            self.free.pop();
        }
    }

    /// Slots in use or below the highest allocation.
    pub fn high_water(&self) -> u32 {
        self.high_water
    }

    /// Slots currently allocated.
    pub fn used(&self) -> u32 {
        self.high_water - self.free.iter().map(|r| r.len).sum::<u32>()
    }
}

/// New particles for one emitter in one step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnRun {
    pub count: u32,
    /// Ring position of the first new particle.
    pub ring_start: u32,
    /// Step length, seconds (particles are spread evenly over it).
    pub dt: f32,
    /// Extra seconds to simulate after the step (prewarm).
    pub pre_age: f32,
    /// Emitter position at the start and end of the step.
    pub from: DVec3,
    pub to: DVec3,
    pub seed: u32,
}

/// One emitter's data for this frame, for the render feature.
#[derive(Debug, Clone)]
pub struct FrameEmitter {
    /// Parameters; camera-relative fields are filled in by the render feature.
    pub gpu: GpuEmitter,
    pub origin: DVec3,
    pub prev_origin: DVec3,
    /// Collision plane: a point on it and its normal.
    pub plane: Option<(DVec3, Vec3)>,
    pub range: PoolRange,
    pub runs: Vec<SpawnRun>,
    /// Bounding radius around `origin` for culling, meters.
    pub reach: f32,
}

/// Everything the GPU needs this frame. Rebuilt every frame by [`tick_effects`].
#[derive(Resource, Debug, Default)]
pub struct ParticleFrame {
    /// `Time::frame_count` when it was built (the renderer simulates each frame once).
    pub frame: u64,
    pub emitters: Vec<FrameEmitter>,
    /// Ranges allocated this frame: cleared on the GPU before use.
    pub cleared: Vec<PoolRange>,
    pub pool_high_water: u32,
    pub pool_limit: u32,
}

/// Counters for `vfx.stats` and diagnostics.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct VfxStats {
    pub effects: u32,
    pub fading: u32,
    pub emitters: u32,
    pub spawned_this_frame: u32,
    pub pool_used: u32,
    pub pool_high_water: u32,
    /// Emitters that could not get room in the pool (raise `VfxSettings::max_particles`).
    pub starved: u32,
}

struct EmitterRun {
    range: Option<PoolRange>,
    head: u32,
    rate_acc: f32,
    dist_acc: f32,
    /// Effect age when it last spawned (`None` = never).
    last_emit: Option<f32>,
    baked: Box<GpuEmitter>,
    runs: Vec<SpawnRun>,
}

struct Instance {
    entity: Option<Entity>,
    name: String,
    desc: Arc<EffectDesc>,
    version: u64,
    scale: f32,
    rate: f32,
    intensity: f32,
    age: f32,
    pos: DVec3,
    prev_pos: DVec3,
    rot: Quat,
    vel: Vec3,
    playing: bool,
    /// Age when emission ended (stopped or ran out of duration).
    stopped_at: Option<f32>,
    emitters: Vec<EmitterRun>,
    seed: u32,
    light: Option<Entity>,
    local: Option<LocalTimeScale>,
    despawn_when_done: bool,
    started: bool,
    finished: bool,
    plane: Option<(DVec3, Vec3)>,
    gravity: Vec3,
    wind: Vec3,
    /// This frame's time step for the effect, seconds.
    frame_dt: f32,
}

/// Runtime state of every playing effect (managed by the plugin).
#[derive(Resource, Default)]
pub struct VfxRuntime {
    instances: HashMap<Entity, Instance>,
    /// Effects whose entity is gone; they stop emitting and fade out.
    orphans: Vec<Instance>,
    pool: ParticlePool,
    next_seed: u32,
    missing: HashSet<String>,
    pub stats: VfxStats,
}

impl VfxRuntime {
    /// Pool bookkeeping (read-only).
    pub fn pool(&self) -> &ParticlePool {
        &self.pool
    }

    /// Names of effects that were asked for but are not in the library.
    pub fn missing_effects(&self) -> impl Iterator<Item = &str> {
        self.missing.iter().map(String::as_str)
    }

    /// Move every effect's world-space state into a new reference frame.
    pub fn shift(&mut self, s: &engine_world::WorldShift) {
        for inst in self.instances.values_mut().chain(self.orphans.iter_mut()) {
            inst.pos = s.point(inst.pos);
            inst.prev_pos = s.point(inst.prev_pos);
            inst.rot = s.orientation(inst.rot);
            inst.vel = s.velocity_at(inst.pos, inst.vel);
            inst.plane = inst.plane.map(|(p, n)| (s.point(p), s.direction(n)));
            inst.gravity = s.direction(inst.gravity);
            inst.wind = s.direction(inst.wind);
        }
    }
}

/// Prewarm step, seconds.
const PREWARM_STEP: f32 = 1.0 / 20.0;
/// Particles that would outlive a stopped effect by more than this (magic circles, mist)
/// fade out over this many seconds instead.
const STOP_FADE_SECS: f32 = 1.0;

impl Instance {
    fn new(
        entity: Entity,
        effect: &ParticleEffect,
        desc: Arc<EffectDesc>,
        version: u64,
        seed: u32,
    ) -> Self {
        let mut inst = Instance {
            entity: Some(entity),
            name: effect.effect.clone(),
            desc,
            version,
            scale: effect.scale,
            rate: effect.rate,
            intensity: effect.intensity,
            age: 0.0,
            pos: DVec3::ZERO,
            prev_pos: DVec3::ZERO,
            rot: Quat::IDENTITY,
            vel: Vec3::ZERO,
            playing: effect.playing,
            stopped_at: None,
            emitters: Vec::new(),
            seed: if effect.seed != 0 { effect.seed } else { seed },
            light: None,
            local: None,
            despawn_when_done: effect.despawn_when_done,
            started: false,
            finished: false,
            plane: None,
            gravity: Vec3::ZERO,
            wind: Vec3::ZERO,
            frame_dt: 0.0,
        };
        inst.rebake();
        inst
    }

    fn rebake(&mut self) {
        self.emitters = self
            .desc
            .emitters
            .iter()
            .map(|e| EmitterRun {
                range: None,
                head: 0,
                rate_acc: 0.0,
                dist_acc: 0.0,
                last_emit: None,
                baked: Box::new(GpuEmitter::bake(e, self.scale)),
                runs: Vec::new(),
            })
            .collect();
    }

    fn free_ranges(&mut self, pool: &mut ParticlePool) {
        for e in &mut self.emitters {
            if let Some(r) = e.range.take() {
                pool.free(r);
            }
        }
    }

    /// Restart from the beginning (replay, or the effect definition changed).
    fn restart(&mut self, pool: &mut ParticlePool) {
        self.free_ranges(pool);
        self.age = 0.0;
        self.stopped_at = None;
        self.started = false;
        self.finished = false;
        self.rebake();
    }

    fn emitting_now(&self) -> bool {
        self.playing && self.stopped_at.is_none()
    }

    /// Advance by `dt` seconds of this effect's time, scheduling spawns.
    fn step(&mut self, dt: f32, from: DVec3, to: DVec3, pre_age: f32, rate_scale: f32) {
        let a0 = self.age;
        let a1 = a0 + dt;
        let effect_end = self.desc.duration.unwrap_or(f32::INFINITY);
        let emitting = self.emitting_now();
        let moved = from.distance(to) as f32;
        let rate_mul = self.rate.max(0.0) * rate_scale;
        for (i, run) in self.emitters.iter_mut().enumerate() {
            let desc = &self.desc.emitters[i];
            let Some(range) = run.range else {
                continue;
            };
            let start = desc.delay.max(0.0);
            let end = desc
                .duration
                .map(|d| start + d.max(0.0))
                .unwrap_or(f32::INFINITY)
                .min(effect_end);
            let mut n = 0u32;
            if emitting && a1 > start && a0 < end {
                let overlap = a1.min(end) - a0.max(start);
                run.rate_acc += desc.rate * rate_mul * overlap.max(0.0);
                run.dist_acc += desc.rate_per_meter * rate_mul * moved;
                n += bursts_between(desc, start, end, a0, a1, rate_mul);
            }
            let whole = (run.rate_acc + run.dist_acc).floor();
            if whole >= 1.0 {
                let take_rate = run.rate_acc.floor();
                run.rate_acc -= take_rate;
                run.dist_acc -= whole - take_rate;
                n = n.saturating_add(whole as u32);
            }
            if n == 0 {
                continue;
            }
            let n = n.min(range.len);
            let seed = self
                .seed
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add((i as u32).wrapping_mul(0x85EB_CA6B))
                .wrapping_add((run.head).wrapping_mul(0xC2B2_AE35))
                .wrapping_add((a1 * 1000.0) as u32);
            run.runs.push(SpawnRun {
                count: n,
                ring_start: run.head,
                dt,
                pre_age,
                from,
                to,
                seed,
            });
            run.head = (run.head + n) % range.len;
            run.last_emit = Some(a1);
        }
        self.age = a1;
        if self.stopped_at.is_none() {
            // Stopping takes effect from the start of the step that saw it.
            if !self.playing {
                self.stopped_at = Some(a0);
            } else if a1 >= effect_end {
                self.stopped_at = Some(effect_end);
            }
        }
    }

    /// True once emission has ended and every particle has died.
    fn all_dead(&self) -> bool {
        if self.stopped_at.is_none() {
            return false;
        }
        let light_done = match (self.desc.light, self.stopped_at) {
            (Some(l), Some(s)) => self.age - s >= l.fade_secs,
            _ => true,
        };
        light_done && (0..self.emitters.len()).all(|i| !self.has_particles(i))
    }

    /// Whether emitter `i` may still have live particles.
    fn has_particles(&self, i: usize) -> bool {
        let Some(t) = self.emitters[i].last_emit else {
            return false;
        };
        let life = self.desc.emitters[i].max_lifetime();
        if self.age - t >= life {
            return false;
        }
        match self.stopped_at {
            Some(s) if life > STOP_FADE_SECS * 1.5 => self.age - s < STOP_FADE_SECS,
            _ => true,
        }
    }

    /// Opacity of emitter `i`'s particles: long-lived ones fade out once the effect stops.
    fn stop_fade(&self, i: usize) -> f32 {
        match self.stopped_at {
            Some(s) if self.desc.emitters[i].max_lifetime() > STOP_FADE_SECS * 1.5 => {
                (1.0 - (self.age - s) / STOP_FADE_SECS).clamp(0.0, 1.0)
            }
            _ => 1.0,
        }
    }

    /// Emitters that still have (or just got) particles.
    fn live_emitters(&self) -> impl Iterator<Item = (usize, &EmitterRun)> {
        self.emitters.iter().enumerate().filter(move |(i, e)| {
            e.range.is_some() && (!e.runs.is_empty() || self.has_particles(*i))
        })
    }

    /// Brightness of the light now: flicker and fade-out.
    fn light_factor(&self) -> f32 {
        let Some(light) = self.desc.light else {
            return 0.0;
        };
        let fade = match self.stopped_at {
            Some(s) if light.fade_secs > 0.0 => {
                1.0 - ((self.age - s) / light.fade_secs).clamp(0.0, 1.0)
            }
            Some(_) => 0.0,
            None => 1.0,
        };
        let n =
            engine_core::math::noise::perlin2(self.age as f64 * 11.0, self.seed as f64 * 0.37, 7);
        let flicker = (1.0 + light.flicker * n * 1.4).max(0.0);
        fade * flicker * self.intensity.max(0.0)
    }
}

/// Number of burst particles that fire in effect-age window `[a0, a1)`.
fn bursts_between(
    desc: &EmitterDesc,
    start: f32,
    end: f32,
    a0: f32,
    a1: f32,
    rate_mul: f32,
) -> u32 {
    let mut n = 0u32;
    for b in &desc.bursts {
        let first = start + b.time.max(0.0);
        let count = (b.count as f32 * rate_mul).round() as u32;
        if b.repeat <= 0.0 {
            if first >= a0 && first < a1 && first < end.max(start + 1e-6) {
                n += count;
            }
            continue;
        }
        let k0 = ((a0 - first) / b.repeat).ceil().max(0.0) as u64;
        let mut k = k0;
        loop {
            if b.cycles > 0 && k >= b.cycles as u64 {
                break;
            }
            let t = first + k as f32 * b.repeat;
            if t >= a1 || t >= end {
                break;
            }
            if t >= a0 {
                n += count;
            }
            k += 1;
        }
    }
    n
}

/// The effects [`tick_effects`] advances (point lights it spawned are excluded).
type EffectQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ParticleEffect,
        &'static GlobalTransform,
        Option<&'static LocalTimeScale>,
        Has<Trail>,
    ),
    Without<VfxLight>,
>;

/// Advance every playing effect, schedule spawns and build the [`ParticleFrame`].
#[allow(clippy::too_many_arguments)]
pub fn tick_effects(
    mut commands: Commands,
    time: Res<Time>,
    library: Res<EffectLibrary>,
    settings: Res<VfxSettings>,
    env: Env,
    mut runtime: ResMut<VfxRuntime>,
    mut frame: ResMut<ParticleFrame>,
    effects: EffectQuery,
    mut lights: Query<(&mut Transform, &mut GlobalTransform, &mut PointLight), With<VfxLight>>,
    mut diagnostics: Option<ResMut<Diagnostics>>,
) {
    let runtime = &mut *runtime;
    let limit = settings.max_particles.max(1024);
    let rate_scale = settings.spawn_scale.max(0.0);
    frame.frame = time.frame_count();
    frame.emitters.clear();
    frame.cleared.clear();
    let mut stats = VfxStats::default();
    let mut seen: HashSet<Entity> = HashSet::with_capacity(runtime.instances.len());

    for (entity, effect, gt, local, has_trail) in &effects {
        seen.insert(entity);
        let Some((desc, version)) = library.get_versioned(&effect.effect) else {
            if runtime.missing.insert(effect.effect.clone()) {
                engine_core::log::warn!(
                    "vfx: no effect named \"{}\" (entity {entity}); see vfx.effects for the list",
                    effect.effect
                );
            }
            if let Some(mut inst) = runtime.instances.remove(&entity) {
                inst.entity = None;
                inst.playing = false;
                runtime.orphans.push(inst);
            }
            continue;
        };
        runtime.next_seed = runtime.next_seed.wrapping_add(0x6D2B_79F5);
        let seed = runtime.next_seed;
        let inst = runtime
            .instances
            .entry(entity)
            .or_insert_with(|| Instance::new(entity, effect, desc.clone(), version, seed));
        // Changes that need a restart.
        if inst.name != effect.effect || inst.version != version {
            inst.name = effect.effect.clone();
            inst.desc = desc.clone();
            inst.version = version;
            inst.restart(&mut runtime.pool);
        }
        if inst.scale != effect.scale {
            inst.scale = effect.scale;
            for (i, run) in inst.emitters.iter_mut().enumerate() {
                *run.baked = GpuEmitter::bake(&inst.desc.emitters[i], inst.scale);
            }
        }
        if effect.playing && !inst.playing && inst.stopped_at.is_some() {
            // Replay.
            inst.restart(&mut runtime.pool);
        }
        inst.playing = effect.playing;
        inst.rate = effect.rate;
        inst.intensity = effect.intensity;
        inst.despawn_when_done = effect.despawn_when_done;
        inst.local = local.copied();

        let dt = time.delta_for(local);
        let pos = gt.translation();
        inst.rot = gt.rotation();
        if !inst.started {
            inst.pos = pos;
            inst.prev_pos = pos;
            // Trails from the definition are attached to the entity.
            if let Some(trail) = &inst.desc.trail
                && !has_trail
            {
                commands.entity(entity).insert(trail.clone());
            }
        } else {
            inst.prev_pos = inst.pos;
            inst.pos = pos;
        }
        if dt > 0.0 {
            inst.vel = ((inst.pos - inst.prev_pos) / dt as f64).as_vec3();
        }
        inst.gravity = env.gravity(pos);
        inst.wind = env.wind(pos);
        inst.plane = collision_plane(&env, pos);
        tick_instance(
            inst,
            dt,
            &mut runtime.pool,
            limit,
            rate_scale,
            &mut frame.cleared,
            &mut stats,
        );
    }

    // Effects whose entity is gone (or lost its ParticleEffect) stop emitting and fade out.
    let gone: Vec<Entity> = runtime
        .instances
        .keys()
        .filter(|e| !seen.contains(e))
        .copied()
        .collect();
    for e in gone {
        if let Some(mut inst) = runtime.instances.remove(&e) {
            inst.entity = None;
            inst.playing = false;
            runtime.orphans.push(inst);
        }
    }
    for inst in &mut runtime.orphans {
        let dt = time.delta_for(inst.local.as_ref());
        inst.prev_pos = inst.pos;
        tick_instance(
            inst,
            dt,
            &mut runtime.pool,
            limit,
            rate_scale,
            &mut frame.cleared,
            &mut stats,
        );
    }

    // Lights, finished effects and the GPU frame.
    let mut finished_entities = Vec::new();
    for inst in runtime
        .instances
        .values_mut()
        .chain(runtime.orphans.iter_mut())
    {
        update_light(&mut commands, inst, &mut lights);
        if inst.finished {
            continue;
        }
        if inst.started && inst.all_dead() {
            inst.finished = true;
            inst.free_ranges(&mut runtime.pool);
            if let Some(l) = inst.light.take() {
                commands.entity(l).try_despawn();
            }
            if let Some(e) = inst.entity
                && inst.despawn_when_done
            {
                finished_entities.push(e);
            }
            continue;
        }
        if inst.entity.is_some() {
            stats.effects += 1;
        } else {
            stats.fading += 1;
        }
        push_frame_emitters(inst, &mut frame.emitters);
    }
    runtime.orphans.retain(|i| !i.finished);
    for e in finished_entities {
        runtime.instances.remove(&e);
        commands.entity(e).try_despawn();
    }

    stats.emitters = frame.emitters.len() as u32;
    stats.pool_used = runtime.pool.used();
    stats.pool_high_water = runtime.pool.high_water();
    frame.pool_high_water = runtime.pool.high_water();
    frame.pool_limit = limit;
    runtime.stats = stats;
    if let Some(d) = diagnostics.as_deref_mut() {
        d.set("vfx.effects", stats.effects as f64);
        d.set("vfx.emitters", stats.emitters as f64);
        d.set("vfx.spawned", stats.spawned_this_frame as f64);
        d.set("vfx.pool_used", stats.pool_used as f64);
        d.set("vfx.starved", stats.starved as f64);
    }
}

fn collision_plane(env: &Env, pos: DVec3) -> Option<(DVec3, Vec3)> {
    let ground = env.ground(pos);
    let water = env.water(pos);
    match (ground, water) {
        (Some(g), Some(w)) if w.surface_height > g.height => {
            Some((DVec3::new(pos.x, w.surface_height, pos.z), w.normal))
        }
        (Some(g), _) => Some((DVec3::new(pos.x, g.height, pos.z), g.normal)),
        (None, Some(w)) => Some((DVec3::new(pos.x, w.surface_height, pos.z), w.normal)),
        (None, None) => None,
    }
}

fn tick_instance(
    inst: &mut Instance,
    dt: f32,
    pool: &mut ParticlePool,
    limit: u32,
    rate_scale: f32,
    cleared: &mut Vec<crate::emitter::PoolRange>,
    stats: &mut VfxStats,
) {
    for run in &mut inst.emitters {
        run.runs.clear();
    }
    inst.frame_dt = dt;
    if inst.finished {
        return;
    }
    if !inst.started {
        inst.started = true;
        // Room in the pool for every layer.
        let rate_mul = inst.rate.max(0.0) * rate_scale;
        for (i, run) in inst.emitters.iter_mut().enumerate() {
            let cap = inst.desc.emitters[i].capacity(rate_mul.max(0.01));
            match pool.alloc(cap, limit) {
                Some(r) => {
                    cleared.push(r);
                    run.range = Some(r);
                }
                None => stats.starved += 1,
            }
        }
        // Prewarm: simulate the effect as if it started `prewarm` seconds ago.
        let prewarm = inst.desc.prewarm;
        if prewarm > 0.0 && inst.playing {
            let steps = (prewarm / PREWARM_STEP).ceil().clamp(1.0, 600.0) as u32;
            let h = prewarm / steps as f32;
            for k in 0..steps {
                let pre_age = prewarm - (k + 1) as f32 * h;
                inst.step(h, inst.pos, inst.pos, pre_age, rate_scale);
            }
        }
    }
    let (from, to) = (inst.prev_pos, inst.pos);
    inst.step(dt, from, to, 0.0, rate_scale);
    for run in &inst.emitters {
        stats.spawned_this_frame += run.runs.iter().map(|r| r.count).sum::<u32>();
    }
}

fn push_frame_emitters(inst: &Instance, out: &mut Vec<FrameEmitter>) {
    for (i, run) in inst.live_emitters() {
        let desc = &inst.desc.emitters[i];
        let mut gpu = *run.baked;
        gpu.origin[3] = inst.frame_dt;
        let r = inst.rot;
        gpu.rot = [r.x, r.y, r.z, r.w];
        gpu.velocity[0] = inst.vel.x;
        gpu.velocity[1] = inst.vel.y;
        gpu.velocity[2] = inst.vel.z;
        gpu.set_forces(inst.gravity.to_array(), desc, inst.scale);
        let air = inst.wind * desc.wind.clamp(0.0, 4.0);
        gpu.wind[0] = air.x;
        gpu.wind[1] = air.y;
        gpu.wind[2] = air.z;
        gpu.shade[0] = desc.emissive.max(0.0) * inst.intensity.max(0.0);
        gpu.look[2] = 1.0;
        gpu.look[3] = inst.stop_fade(i);
        let reach = desc.reach() * inst.scale.max(0.01) + inst.vel.length() * desc.max_lifetime();
        out.push(FrameEmitter {
            gpu,
            origin: inst.pos,
            prev_origin: inst.prev_pos,
            plane: inst.plane,
            range: run.range.expect("live emitters have a range"),
            runs: run.runs.clone(),
            reach,
        });
    }
}

fn update_light(
    commands: &mut Commands,
    inst: &mut Instance,
    lights: &mut Query<(&mut Transform, &mut GlobalTransform, &mut PointLight), With<VfxLight>>,
) {
    let Some(desc) = inst.desc.light else {
        return;
    };
    if inst.finished {
        return;
    }
    let pos = inst.pos + (inst.rot * Vec3::from(desc.offset)).as_dvec3();
    let k = inst.light_factor();
    let light = PointLight {
        color: desc.color,
        intensity_lm: desc.intensity_lm * k,
        range: desc.range * inst.scale.max(0.1).sqrt(),
        radius: 0.1 * inst.scale.max(0.1),
    };
    match inst.light.and_then(|e| lights.get_mut(e).ok()) {
        Some((mut t, mut g, mut l)) => {
            t.translation = pos;
            *g = GlobalTransform::from_translation_rotation(pos, Quat::IDENTITY);
            *l = light;
        }
        None if inst.light.is_none() && inst.started => {
            let t = Transform::from_translation(pos);
            inst.light = Some(
                commands
                    .spawn((t, GlobalTransform::from(t), light, VfxLight))
                    .id(),
            );
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::{Burst, EffectLight};

    #[test]
    fn pool_allocates_frees_and_merges() {
        let mut pool = ParticlePool::default();
        let a = pool.alloc(100, 1000).unwrap();
        let b = pool.alloc(200, 1000).unwrap();
        let c = pool.alloc(50, 1000).unwrap();
        assert_eq!((a.base, b.base, c.base), (0, 100, 300));
        assert_eq!(pool.high_water(), 350);
        assert!(pool.alloc(700, 1000).is_none(), "over the limit");
        pool.free(b);
        assert_eq!(pool.used(), 150);
        // First fit reuses the hole.
        let d = pool.alloc(150, 1000).unwrap();
        assert_eq!(d.base, 100);
        pool.free(c);
        assert_eq!(pool.high_water(), 250, "top shrinks");
        pool.free(a);
        pool.free(d);
        assert_eq!(pool.high_water(), 0);
        assert_eq!(pool.used(), 0);
        // A free range at the top is reused when growing.
        let e = pool.alloc(10, 1000).unwrap();
        let f = pool.alloc(10, 1000).unwrap();
        pool.free(e);
        let g = pool.alloc(30, 1000).unwrap();
        assert_eq!(g.base, 20);
        pool.free(f);
        let h = pool.alloc(20, 1000).unwrap();
        assert_eq!(h.base, 0, "merged hole of 20 at the bottom");
    }

    fn desc(emitter: EmitterDesc, duration: Option<f32>) -> Arc<EffectDesc> {
        Arc::new(EffectDesc {
            name: "t".into(),
            duration,
            emitters: vec![emitter],
            ..Default::default()
        })
    }

    fn instance(d: Arc<EffectDesc>) -> (Instance, ParticlePool) {
        let mut pool = ParticlePool::default();
        let mut inst = Instance::new(
            Entity::from_raw_u32(1).unwrap(),
            &ParticleEffect::new("t"),
            d,
            0,
            1,
        );
        let mut cleared = Vec::new();
        let mut stats = VfxStats::default();
        tick_instance(
            &mut inst,
            0.0,
            &mut pool,
            1 << 20,
            1.0,
            &mut cleared,
            &mut stats,
        );
        assert_eq!(cleared.len(), 1);
        (inst, pool)
    }

    fn spawned(inst: &Instance) -> u32 {
        inst.emitters[0].runs.iter().map(|r| r.count).sum()
    }

    #[test]
    fn rate_spawns_over_time_and_slow_motion_slows_it() {
        let d = desc(
            EmitterDesc {
                rate: 100.0,
                ..Default::default()
            },
            None,
        );
        let (mut inst, mut pool) = instance(d.clone());
        let mut total = 0;
        let mut stats = VfxStats::default();
        for _ in 0..60 {
            tick_instance(
                &mut inst,
                1.0 / 60.0,
                &mut pool,
                1 << 20,
                1.0,
                &mut Vec::new(),
                &mut stats,
            );
            total += spawned(&inst);
        }
        assert!((99..=101).contains(&total), "{total}");
        // At 10% speed the same real time spawns a tenth as many.
        let (mut slow, mut pool) = instance(d);
        let mut total = 0;
        for _ in 0..60 {
            tick_instance(
                &mut slow,
                0.1 / 60.0,
                &mut pool,
                1 << 20,
                1.0,
                &mut Vec::new(),
                &mut stats,
            );
            total += spawned(&slow);
        }
        assert!((9..=11).contains(&total), "{total}");
    }

    #[test]
    fn bursts_repeat_and_effect_finishes() {
        let d = desc(
            EmitterDesc {
                bursts: vec![Burst {
                    time: 0.0,
                    count: 20,
                    repeat: 0.25,
                    cycles: 3,
                }],
                lifetime: [0.5, 0.5],
                ..Default::default()
            },
            Some(1.0),
        );
        let (mut inst, mut pool) = instance(d);
        let mut stats = VfxStats::default();
        let mut total = 0;
        let mut ticks = 0;
        while !inst.all_dead() && ticks < 200 {
            tick_instance(
                &mut inst,
                0.05,
                &mut pool,
                1 << 20,
                1.0,
                &mut Vec::new(),
                &mut stats,
            );
            total += spawned(&inst);
            ticks += 1;
        }
        assert_eq!(total, 60);
        // Last burst at 0.5 s + 0.5 s lifetime, and emission ends at 1.0 s.
        assert!((20..=22).contains(&ticks), "{ticks}");
    }

    #[test]
    fn distance_spawning_follows_movement() {
        let d = desc(
            EmitterDesc {
                rate_per_meter: 10.0,
                ..Default::default()
            },
            None,
        );
        let (mut inst, mut pool) = instance(d);
        let mut stats = VfxStats::default();
        let mut total = 0;
        for i in 1..=10 {
            inst.prev_pos = inst.pos;
            inst.pos = DVec3::new(i as f64 * 0.5, 0.0, 0.0);
            tick_instance(
                &mut inst,
                0.1,
                &mut pool,
                1 << 20,
                1.0,
                &mut Vec::new(),
                &mut stats,
            );
            total += spawned(&inst);
        }
        assert_eq!(total, 50);
        let run = inst.emitters[0].runs[0];
        assert_eq!(run.from, DVec3::new(4.5, 0.0, 0.0));
        assert_eq!(run.to, DVec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn prewarm_spawns_aged_particles() {
        let d = Arc::new(EffectDesc {
            name: "t".into(),
            prewarm: 1.0,
            emitters: vec![EmitterDesc {
                rate: 50.0,
                lifetime: [2.0, 2.0],
                ..Default::default()
            }],
            ..Default::default()
        });
        let (inst, _) = instance(d);
        let runs = &inst.emitters[0].runs;
        let total: u32 = runs.iter().map(|r| r.count).sum();
        assert!((49..=51).contains(&total), "{total}");
        assert!(runs.first().unwrap().pre_age > 0.9);
        assert!(runs.last().unwrap().pre_age.abs() < 1e-5);
        assert!((inst.age - 1.0).abs() < 1e-4);
    }

    #[test]
    fn ring_wraps_and_never_spawns_more_than_capacity() {
        let d = desc(
            EmitterDesc {
                bursts: vec![Burst {
                    count: 1000,
                    ..Default::default()
                }],
                max_particles: 64,
                ..Default::default()
            },
            Some(0.1),
        );
        let (mut inst, mut pool) = instance(d);
        let mut stats = VfxStats::default();
        tick_instance(
            &mut inst,
            0.05,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert_eq!(spawned(&inst), 64);
        assert_eq!(inst.emitters[0].head, 0);
    }

    #[test]
    fn long_lived_particles_fade_out_when_stopped() {
        let d = desc(
            EmitterDesc {
                bursts: vec![Burst {
                    count: 1,
                    ..Default::default()
                }],
                lifetime: [30.0, 30.0],
                ..Default::default()
            },
            None,
        );
        let (mut inst, mut pool) = instance(d);
        let mut stats = VfxStats::default();
        tick_instance(
            &mut inst,
            0.1,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert_eq!(inst.stop_fade(0), 1.0);
        inst.playing = false;
        tick_instance(
            &mut inst,
            0.5,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert!((inst.stop_fade(0) - 0.5).abs() < 1e-4);
        assert!(!inst.all_dead());
        tick_instance(
            &mut inst,
            0.6,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert!(
            inst.all_dead(),
            "gone a second after stopping, not after 30 s"
        );
    }

    #[test]
    fn stopping_fades_the_light_then_finishes() {
        let d = Arc::new(EffectDesc {
            name: "t".into(),
            emitters: vec![EmitterDesc {
                rate: 10.0,
                lifetime: [0.2, 0.2],
                ..Default::default()
            }],
            light: Some(EffectLight {
                fade_secs: 0.5,
                flicker: 0.0,
                ..Default::default()
            }),
            ..Default::default()
        });
        let (mut inst, mut pool) = instance(d);
        let mut stats = VfxStats::default();
        tick_instance(
            &mut inst,
            0.1,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert!((inst.light_factor() - 1.0).abs() < 1e-5);
        inst.playing = false;
        tick_instance(
            &mut inst,
            0.25,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert!(inst.light_factor() < 0.6 && inst.light_factor() > 0.0);
        assert!(!inst.all_dead());
        tick_instance(
            &mut inst,
            0.3,
            &mut pool,
            1 << 20,
            1.0,
            &mut Vec::new(),
            &mut stats,
        );
        assert!(inst.all_dead());
    }
}
