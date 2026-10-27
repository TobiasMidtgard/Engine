//! Smoke systems: volumes, sources, everything that pushes smoke around, and the step.

use engine_core::math::{DVec2, DVec3, UVec3, Vec3};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{
    FocusPoints, Ground, ProjectileTrace, SimulationRange, SmokeEmission, WeatherState, WindField,
    WorldImpulse,
};

use std::sync::{Mutex, mpsc};

use engine_core::tasks::AsyncComputeTaskPool;

use crate::domain::{Change, Domain, StepParams};
use crate::settings::SmokeSettings;

/// A box where smoke is simulated, its `Transform` at the center of its floor. Smoke released
/// inside it stays in it; elsewhere the module makes volumes on its own
/// (`SmokeSettings::auto_size_m`). Place one over a battlefield, a burning town or a valley
/// that fills with smoke.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeVolume {
    /// Size, meters (x, y, z).
    pub size: [f32; 3],
    /// Voxel size, meters (smaller is sharper and costlier: 0.25 for a room, 1–2 for a valley).
    pub cell_m: f32,
}

impl Default for SmokeVolume {
    fn default() -> Self {
        Self {
            size: [32.0, 16.0, 32.0],
            cell_m: 0.5,
        }
    }
}

/// Something that keeps releasing smoke: a smoke grenade, a chimney, a steam vent, a flare.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeSource {
    /// kg per second (a smoke grenade ≈ 0.05, a chimney ≈ 0.01).
    pub rate_kg_s: f32,
    /// Radius of the release, m.
    pub radius: f32,
    /// Gas temperature, °C (hot smoke rises; 20 °C stays low and spreads).
    pub temperature_c: f32,
    /// Release velocity (world axes), m/s.
    pub velocity: [f32; 3],
    /// Smoke color (linear albedo): white [0.9, 0.9, 0.9], soot [0.05, 0.05, 0.05], red signal
    /// smoke [0.85, 0.12, 0.1].
    pub color: [f32; 3],
    /// Gas released with the smoke, m³/s: pushes the air out so the cloud billows (a smoke
    /// grenade's cloud grows to several meters in seconds; a chimney's barely does).
    pub gas_m3_s: f32,
    /// Seconds it runs for (0 = forever); it stops by itself after that.
    pub duration_secs: f32,
    /// Seconds it has run.
    pub age_secs: f32,
    pub active: bool,
}

impl Default for SmokeSource {
    fn default() -> Self {
        Self {
            rate_kg_s: 0.05,
            radius: 0.4,
            temperature_c: 60.0,
            velocity: [0.0, 1.5, 0.0],
            color: [0.85, 0.85, 0.85],
            gas_m3_s: 0.0,
            duration_secs: 0.0,
            age_secs: 0.0,
            active: true,
        }
    }
}

impl SmokeSource {
    /// A smoke grenade's worth: `secs` of thick, slightly warm smoke.
    pub fn grenade(color: [f32; 3], secs: f32) -> Self {
        Self {
            rate_kg_s: 0.08,
            radius: 0.35,
            temperature_c: 45.0,
            velocity: [0.0, 1.0, 0.0],
            color,
            gas_m3_s: 15.0,
            duration_secs: secs,
            ..Default::default()
        }
    }
}

/// Something that moves through smoke and stirs it: a car, a character, a helicopter's body.
/// The smoke inside its ball moves with it and gets pushed out of the way.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeObstacle {
    pub radius: f32,
}

impl Default for SmokeObstacle {
    fn default() -> Self {
        Self { radius: 1.0 }
    }
}

/// A fan, vent or rotor downwash that blows air (and the smoke in it) along the entity's
/// forward direction (local -Z). It also stirs the shared `WindField`, so grass and particles
/// nearby feel it too.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeFan {
    /// Air speed at the fan, m/s.
    pub speed: f32,
    /// Radius of the jet, m.
    pub radius: f32,
    /// How far the jet reaches, m.
    pub reach: f32,
    pub on: bool,
}

impl Default for SmokeFan {
    fn default() -> Self {
        Self {
            speed: 8.0,
            radius: 1.5,
            reach: 12.0,
            on: true,
        }
    }
}

/// Where an obstacle was last step (to know how fast it moves).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SmokeObstacleTrack {
    last: Option<DVec3>,
}

impl SmokeObstacleTrack {
    /// Where it was, in a new reference frame, so a world shift doesn't look like a dash.
    pub(crate) fn shift(&mut self, s: &engine_world::WorldShift) {
        self.last = self.last.map(|p| s.point(p));
    }
}

/// One simulated volume.
#[derive(Debug, Clone)]
pub struct SmokeDomain {
    pub id: u32,
    /// The `SmokeVolume` entity, or `None` for volumes made around released smoke.
    pub owner: Option<Entity>,
    pub domain: Domain,
    /// Seconds with no smoke in it.
    pub empty_secs: f32,
    /// Far from every focus: it keeps its smoke as it was and doesn't step until someone
    /// comes back.
    pub resting: bool,
    /// Steps of the simulation clock per step of this volume: 2 while its smoke fills more
    /// than [`BIG_VOLUME_VOXELS`] (a meadow fire's plume), so it costs half as much. Its
    /// steps are twice as long; semi-Lagrangian smoke takes that in its stride.
    pub stride: u32,
    /// Seconds of simulation clock since it last stepped.
    pub owed_secs: f32,
    /// Its step running on a background task, if one is.
    pub background: Background,
}

/// A volume's step running on a background task: it steps a copy of the air, and the copy
/// replaces the air when it is done, with what was done to the air meanwhile done to it again.
#[derive(Default)]
pub struct Background {
    job: Option<Mutex<mpsc::Receiver<Domain>>>,
    /// Changes made to the air since the job started.
    pending: Vec<Change>,
    /// The air before the last step landed: memory for the next job's copy.
    spare: Option<Domain>,
}

impl Background {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }
}

/// A copy of a volume doesn't take its running step with it.
impl Clone for Background {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for Background {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Background")
            .field("running", &self.running())
            .field("pending", &self.pending.len())
            .finish()
    }
}

/// Active voxels above which a volume steps at most every other tick.
pub const BIG_VOLUME_VOXELS: u32 = 40_000;

/// Most ticks of the simulation clock one step of a volume may cover.
pub const MAX_STRIDE: u32 = 4;

/// How far smoke may move in one step, voxels (see [`Domain::steady_secs`]).
pub const STEP_CELLS: f32 = 1.0;

impl SmokeDomain {
    /// Ticks (of `tick` seconds) until this volume's next step: as many as its smoke takes to
    /// move a voxel, up to [`MAX_STRIDE`], so slow smoke, drifting dust and fog step a few
    /// times a second and a fast plume or a blast every tick (the renderer cross-fades between
    /// steps); a big volume at least every other tick, so it costs half as much. Steps grow
    /// longer one tick at a time, so fresh smoke that is about to speed up (a hot puff that
    /// has not risen yet) gets a few short steps first.
    pub fn next_stride(&self, tick: f32) -> u32 {
        let fit = (self.domain.steady_secs(STEP_CELLS) / tick.max(1e-4)).floor();
        let least = if self.domain.active_voxels > BIG_VOLUME_VOXELS {
            2
        } else {
            1
        };
        (fit.min(MAX_STRIDE as f32) as u32)
            .min(self.stride + 1)
            .max(least)
    }

    /// How far it is toward its next step (0..1), for drawing smoothly between steps;
    /// `clock` is the simulation clock's own progress.
    pub fn progress(&self, clock: f32, settings: &SmokeSettings) -> f32 {
        let step = 1.0 / settings.sim_hz.clamp(1.0, 120.0);
        ((self.owed_secs + clock * step) / (self.stride.max(1) as f32 * step)).clamp(0.0, 1.0)
    }

    /// Do `c` to the air, and again to the step running in the background when it lands.
    pub fn change(&mut self, c: Change) {
        self.domain.apply(&c);
        if self.background.running() {
            self.background.pending.push(c);
        }
    }

    /// Forget the step running in the background, if one is: the air stays as it shows and
    /// that step is skipped. Anything that changes the air other than by
    /// [`change`](Self::change) (a world shift, a new floor, fog, a step on the spot) does this
    /// first. It never waits: a system waiting on a worker that waits on the compute threads
    /// can hold them all up.
    pub fn drop_running(&mut self) {
        self.background.job = None;
        self.background.pending.clear();
    }

    /// Put the background step in place if it is done (or `wait` for it); true when none
    /// runs any more.
    fn land(&mut self, wait: bool) -> bool {
        let Some(job) = self.background.job.as_mut() else {
            return true;
        };
        let rx = job.get_mut().unwrap_or_else(|e| e.into_inner());
        let stepped = if wait {
            rx.recv().ok()
        } else {
            match rx.try_recv() {
                Ok(d) => Some(d),
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => None,
            }
        };
        self.background.job = None;
        let pending = std::mem::take(&mut self.background.pending);
        // A step that died leaves the air as it is.
        if let Some(mut stepped) = stepped {
            for c in &pending {
                stepped.apply(c);
            }
            stepped.version = stepped.version.max(self.domain.version) + 1;
            stepped.resampled = self.domain.resampled;
            self.background.spare = Some(std::mem::replace(&mut self.domain, stepped));
        }
        true
    }

    /// Step a copy of the air in the wind sampled now, on `pool`; it lands at a later
    /// [`land`](Self::land).
    fn start(
        &mut self,
        pool: &AsyncComputeTaskPool,
        params: StepParams,
        wind: &(dyn Fn(DVec3) -> Vec3 + Sync),
    ) {
        let mut work = match self.background.spare.take() {
            Some(mut w) => {
                w.copy_from(&self.domain);
                w
            }
            None => self.domain.clone(),
        };
        work.sample_wind(wind);
        let (tx, rx) = mpsc::channel();
        pool.spawn(async move {
            work.step_sampled(&params);
            let _ = tx.send(work);
        })
        .detach();
        self.background.job = Some(Mutex::new(rx));
    }
}

/// Every smoke volume.
#[derive(Resource, Debug, Clone, Default)]
pub struct SmokeDomains {
    pub list: Vec<SmokeDomain>,
    next_id: u32,
}

impl SmokeDomains {
    /// The volume that holds a point, if any: explicit volumes first, and of those the one
    /// with the finest cells, so a small fine box inside a big coarse one (a campfire's wisp
    /// in a meadow's air) gets the smoke released in it.
    pub fn find(&self, p: DVec3) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, d) in self.list.iter().enumerate() {
            if d.owner.is_some()
                && d.domain.contains(p)
                && best.is_none_or(|b| d.domain.cell < self.list[b].domain.cell)
            {
                best = Some(i);
            }
        }
        best.or_else(|| self.list.iter().position(|d| d.domain.contains(p)))
    }

    pub fn add(&mut self, owner: Option<Entity>, domain: Domain) -> usize {
        self.next_id += 1;
        self.list.push(SmokeDomain {
            id: self.next_id,
            owner,
            domain,
            empty_secs: 0.0,
            resting: false,
            stride: 1,
            owed_secs: 0.0,
            background: Background::default(),
        });
        self.list.len() - 1
    }

    /// Smoke density at a point, kg/m³.
    pub fn density(&self, p: DVec3) -> f32 {
        self.find(p)
            .map(|i| self.list[i].domain.sample(p).density)
            .unwrap_or(0.0)
    }

    /// Fraction of light that gets from `a` to `b` through every volume (1 = clear).
    pub fn transmittance(&self, a: DVec3, b: DVec3, extinction: f32) -> f32 {
        self.list
            .iter()
            .map(|d| d.domain.transmittance(a, b, extinction))
            .product()
    }

    /// Smoke in all volumes, kg.
    pub fn mass_kg(&self) -> f32 {
        self.list.iter().map(|d| d.domain.mass_kg).sum()
    }
}

/// A bullet's path, kept a moment so the renderer can draw the sharp tunnel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tunnel {
    pub from: DVec3,
    pub to: DVec3,
    pub radius: f32,
    pub age: f32,
}

/// Recent bullet tunnels.
#[derive(Resource, Debug, Clone, Default)]
pub struct SmokeTunnels {
    pub list: Vec<Tunnel>,
}

/// Asked of the smoke module through its API.
#[derive(Debug, Clone, Copy)]
pub enum SmokeCommand {
    Emit(SmokeEmission, Vec3),
    Clear,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct SmokeCommands(pub Vec<SmokeCommand>);

#[derive(Resource, Debug, Clone, Default)]
pub struct SmokeClock {
    acc: f32,
}

impl SmokeClock {
    /// How far world time is toward the next simulation step (0..1), for drawing smoothly
    /// between steps.
    pub fn progress(&self, settings: &SmokeSettings) -> f32 {
        (self.acc * settings.sim_hz.clamp(1.0, 120.0)).clamp(0.0, 1.0)
    }
}

/// Albedo of smoke `darkness` (0 white steam .. 1 black soot).
pub fn albedo_of(darkness: f32) -> Vec3 {
    let a = 0.92 + (0.06 - 0.92) * darkness.clamp(0.0, 1.0);
    // Wood smoke is a touch warm when thin, soot is neutral.
    Vec3::new(a, a * 0.985, a * 0.96)
}

fn ground_floor(ground: &Ground) -> impl Fn(f64, f64) -> Option<f64> + Sync + '_ {
    move |x, z| ground.height(DVec3::new(x, 1.0e5, z))
}

/// Keep volumes in sync with `SmokeVolume` components (made again when one moves or changes;
/// one a world shift moved along with its entity is kept), and measure the ground again under
/// volumes the world turned.
pub fn sync_volumes(
    q: Query<(Entity, &SmokeVolume, &GlobalTransform)>,
    mut removed: RemovedComponents<SmokeVolume>,
    mut domains: ResMut<SmokeDomains>,
    ground: Res<Ground>,
) {
    for e in removed.read() {
        domains.list.retain(|d| d.owner != Some(e));
    }
    for d in &mut domains.list {
        if d.domain.floor_stale {
            d.drop_running();
            d.domain.set_floor(ground_floor(&ground));
            d.domain.floor_stale = false;
        }
    }
    for (e, v, gt) in &q {
        let cell = v.cell_m.max(0.05);
        let dims = (Vec3::from(v.size) / cell)
            .ceil()
            .as_uvec3()
            .clamp(UVec3::splat(4), UVec3::splat(256));
        let size = dims.as_dvec3() * cell as f64;
        let base = gt.translation();
        let origin = DVec3::new(base.x - size.x * 0.5, base.y, base.z - size.z * 0.5);
        let same = domains.list.iter().any(|d| {
            d.owner == Some(e)
                && d.domain.dims == dims
                && d.domain.cell == cell
                && d.domain.origin.distance_squared(origin) < 1e-6
        });
        if same {
            continue;
        }
        domains.list.retain(|d| d.owner != Some(e));
        let mut domain = Domain::new(origin, cell, dims);
        domain.set_floor(ground_floor(&ground));
        domains.add(Some(e), domain);
    }
}

/// Smoke from emissions, sources and the API, into the volumes (making volumes as needed).
#[allow(clippy::too_many_arguments)]
pub fn release(
    time: Res<Time>,
    settings: Res<SmokeSettings>,
    mut emissions: MessageReader<SmokeEmission>,
    mut commands_queue: ResMut<SmokeCommands>,
    mut sources: Query<(&GlobalTransform, &mut SmokeSource)>,
    mut domains: ResMut<SmokeDomains>,
    weather: Res<WeatherState>,
    ground: Res<Ground>,
    (focus, range): (Res<FocusPoints>, Option<Res<SimulationRange>>),
) {
    let mut list: Vec<Puff> = Vec::new();
    for e in emissions.read() {
        list.push((*e, albedo_of(e.darkness), 0.0));
    }
    for c in std::mem::take(&mut commands_queue.0) {
        match c {
            SmokeCommand::Emit(e, albedo) => list.push((e, albedo, 0.0)),
            SmokeCommand::Clear => {
                for d in &mut domains.list {
                    d.change(Change::Clear);
                }
            }
        }
    }
    let dt = time.delta_secs();
    for (gt, mut s) in &mut sources {
        if let Some(puff) = run_source(&mut s, gt.translation(), dt) {
            list.push(puff);
        }
    }
    // Smoke released where the world rests (far from every focus) is let go: a volume made
    // for it there would never step, and making one costs a ground query per column.
    if let Some(range) = range
        && !focus.points.is_empty()
    {
        let far = range.active_m * (1.0 + range.hysteresis.max(0.0));
        list.retain(|(e, ..)| focus.distance(e.position) <= far);
    }
    if settings.enabled {
        deposit(
            &mut domains,
            &settings,
            &ground,
            weather.temperature_c,
            list,
        );
    }
}

/// Released smoke: what, its color, and the gas released with it (m³).
type Puff = (SmokeEmission, Vec3, f32);

/// Age a source by `dt` and return what it released.
fn run_source(s: &mut SmokeSource, at: DVec3, dt: f32) -> Option<Puff> {
    if !s.active || dt <= 0.0 {
        return None;
    }
    s.age_secs += dt;
    if s.duration_secs > 0.0 && s.age_secs > s.duration_secs {
        s.active = false;
        return None;
    }
    // Grenades sputter out at the end.
    let fade = if s.duration_secs > 0.0 {
        (1.0 - (s.age_secs / s.duration_secs).powi(4)).max(0.0)
    } else {
        1.0
    };
    Some((
        SmokeEmission {
            position: at,
            velocity: Vec3::from(s.velocity),
            radius: s.radius,
            mass_kg: s.rate_kg_s * dt * fade,
            temperature_c: s.temperature_c,
            darkness: 0.0,
        },
        Vec3::from(s.color),
        s.gas_m3_s * dt * fade,
    ))
}

/// Put released smoke into the volume holding it, making a volume around it if none does.
fn deposit(
    domains: &mut SmokeDomains,
    settings: &SmokeSettings,
    ground: &Ground,
    ambient_c: f32,
    list: Vec<Puff>,
) {
    for (e, albedo, gas) in list {
        if e.mass_kg <= 0.0 && gas <= 0.0 {
            continue;
        }
        let i = match domains.find(e.position) {
            Some(i) => i,
            None => {
                if domains.list.len() >= settings.max_volumes as usize {
                    continue;
                }
                domains.add(None, auto_domain(settings, e.position, ground))
            }
        };
        let d = &mut domains.list[i];
        d.change(Change::Splat {
            at: e.position,
            radius: e.radius,
            mass_kg: e.mass_kg,
            heat_c: (e.temperature_c - ambient_c).max(0.0),
            vel: e.velocity,
            albedo,
        });
        d.change(Change::Inflate {
            at: e.position,
            radius: e.radius,
            gas_m3: gas,
        });
    }
}

/// Size of the wind's eddies that smoke feels, m. Near the ground the biggest eddies are
/// about as big as their height; tens of meters is what tears and sways a fire's plume.
const EDDY_M: f64 = 14.0;

/// The wind at `p` with its eddies: the steady wind (`WindField::sample`) plus gusts that
/// change its direction too, about `turbulence` × its speed across and along it and half that
/// up and down (the air near the ground is never steady). The eddy pattern drifts with the
/// wind and slowly changes as it goes, so a plume meanders, puffs and spreads.
pub fn turbulent_wind(wind: &WindField, p: DVec3, turbulence: f32, time: f64) -> Vec3 {
    let steady = wind.sample(p);
    let speed = wind.prevailing.length();
    if turbulence <= 0.0 || speed < 0.05 {
        return steady;
    }
    let q = (p - wind.gust_origin - wind.prevailing.as_dvec3() * time) / EDDY_M;
    let n = |seed: u64| {
        let a = engine_core::math::noise::perlin3(q.x, q.y, q.z + time * 0.04, seed);
        let b = engine_core::math::noise::perlin3(
            q.x * 2.3 + 17.0,
            q.y * 2.3,
            q.z * 2.3 + time * 0.1,
            seed + 1,
        );
        a + 0.45 * b
    };
    // Perlin noise's spread is about a third of its range.
    let eddy = Vec3::new(n(0xEDD1), 0.5 * n(0xEDD3), n(0xEDD5)) * (3.0 * speed * turbulence);
    steady + eddy
}

/// Run `secs` of sources and simulation at once (no bullets or blasts), e.g. so a demo
/// starts with its smoke already built up. Volumes must exist (`SmokeDomains`).
pub fn fast_forward(world: &mut World, secs: f32) {
    let settings = world.resource::<SmokeSettings>().clone();
    if !settings.enabled || secs <= 0.0 {
        return;
    }
    let dt = 1.0 / settings.sim_hz.clamp(1.0, 120.0);
    let ambient = world.resource::<WeatherState>().temperature_c;
    let params = settings.step_params(dt, ambient);
    for d in &mut world.resource_mut::<SmokeDomains>().list {
        d.drop_running();
    }
    let mut q = world.query::<(&GlobalTransform, &mut SmokeSource)>();
    let start = world.resource::<WindField>().time as f64;
    for i in 0..(secs / dt).ceil() as u32 {
        let puffs: Vec<_> = q
            .iter_mut(world)
            .filter_map(|(gt, mut s)| run_source(&mut s, gt.translation(), dt))
            .collect();
        world.resource_scope(|world, mut domains: Mut<SmokeDomains>| {
            let ground = world.resource::<Ground>();
            deposit(&mut domains, &settings, ground, ambient, puffs);
            let wind = world.resource::<WindField>();
            let time = start + i as f64 * dt as f64;
            let wind_fn = |p: DVec3| turbulent_wind(wind, p, settings.turbulence, time);
            for d in &mut domains.list {
                d.domain.step(&params, &wind_fn);
            }
        });
    }
}

/// A volume around released smoke, snapped to a world grid so neighbors line up.
fn auto_domain(settings: &SmokeSettings, p: DVec3, ground: &Ground) -> Domain {
    let cell = settings.auto_cell_m.max(0.1);
    let size = Vec3::from(settings.auto_size_m).max(Vec3::splat(cell * 4.0));
    let dims = (size / cell).ceil().as_uvec3();
    let size = dims.as_dvec3() * cell as f64;
    let snap = |v: f64, s: f64| (v / s).floor() * s;
    let ground_y = ground.height(p).unwrap_or(p.y).min(p.y);
    let origin = DVec3::new(
        snap(p.x, size.x),
        ground_y - (cell as f64 * 2.0).max(1.0),
        snap(p.z, size.z),
    );
    let mut d = Domain::new(origin, cell, dims);
    d.set_floor(ground_floor(ground));
    d
}

/// Bullets, blasts and moving things push the smoke.
#[allow(clippy::too_many_arguments)]
pub fn disturb(
    settings: Res<SmokeSettings>,
    mut bullets: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut obstacles: Query<(&GlobalTransform, &SmokeObstacle, &mut SmokeObstacleTrack)>,
    fans: Query<(&GlobalTransform, &SmokeFan)>,
    time: Res<Time>,
    mut domains: ResMut<SmokeDomains>,
    mut tunnels: ResMut<SmokeTunnels>,
    mut wind: ResMut<WindField>,
) {
    for b in bullets.read() {
        let mut hit = false;
        for d in &mut domains.list {
            if d.domain.clip(b.from, b.to).is_some() && d.domain.mass_kg > 0.0 {
                d.change(Change::Carve {
                    from: b.from,
                    to: b.to,
                    radius: settings.tunnel_radius_m.max(b.radius),
                    clear: settings.bullet_clear,
                    speed: b.speed,
                });
                hit = true;
            }
        }
        if hit {
            tunnels.list.push(Tunnel {
                from: b.from,
                to: b.to,
                radius: settings.tunnel_radius_m.max(b.radius),
                age: 0.0,
            });
        }
    }
    for i in impulses.read() {
        if i.strength <= 0.0 {
            continue;
        }
        for d in &mut domains.list {
            let c = d.domain.center();
            let half = d.domain.size() * 0.5 + DVec3::splat(i.radius as f64);
            let o = (i.center - c).abs();
            if o.x < half.x && o.y < half.y && o.z < half.z {
                d.change(Change::Blast {
                    at: i.center,
                    radius: i.radius,
                    strength: i.strength,
                });
            }
        }
    }
    let dt = time.delta_secs();
    for (gt, fan) in &fans {
        if !fan.on || fan.speed == 0.0 || dt <= 0.0 {
            continue;
        }
        let from = gt.translation();
        let dir = gt.rotation() * Vec3::NEG_Z;
        let reach = fan.reach.max(0.1);
        // The shared wind decays at about 1.5/s; this keeps it near the fan's speed.
        let mid = from + (dir * reach * 0.5).as_dvec3();
        wind.add_gust(mid, reach * 0.5, fan.speed * 1.5 * dt, Some(dir));
        for d in &mut domains.list {
            let c = d.domain.center();
            let half = d.domain.size() * 0.5 + DVec3::splat(reach as f64);
            let o = (mid - c).abs();
            if o.x < half.x && o.y < half.y && o.z < half.z {
                d.change(Change::Jet {
                    from,
                    dir,
                    length: reach,
                    radius: fan.radius,
                    speed: fan.speed,
                    dt,
                });
            }
        }
    }
    for (gt, o, mut track) in &mut obstacles {
        let p = gt.translation();
        let vel = match track.last {
            Some(last) if dt > 0.0 => ((p - last) / dt as f64).as_vec3(),
            _ => Vec3::ZERO,
        };
        track.last = Some(p);
        if vel.length_squared() < 0.01 {
            continue;
        }
        for d in &mut domains.list {
            if d.domain.contains(p) && d.domain.mass_kg > 0.0 {
                d.change(Change::Obstacle {
                    at: p,
                    radius: o.radius,
                    vel,
                });
            }
        }
    }
}

/// Give obstacles their tracker.
pub fn track_obstacles(
    mut commands: Commands,
    q: Query<Entity, (With<SmokeObstacle>, Without<SmokeObstacleTrack>)>,
) {
    for e in &q {
        commands.entity(e).insert(SmokeObstacleTrack::default());
    }
}

/// Advance every volume with smoke in it near a focus at `sim_hz` of world time.
#[allow(clippy::too_many_arguments)]
pub fn step(
    time: Res<Time>,
    settings: Res<SmokeSettings>,
    mut clock: ResMut<SmokeClock>,
    mut domains: ResMut<SmokeDomains>,
    wind: Res<WindField>,
    weather: Res<WeatherState>,
    focus: Res<FocusPoints>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut diagnostics: ResMut<Diagnostics>,
    range: Option<Res<SimulationRange>>,
) {
    if !settings.enabled {
        return;
    }
    let step = 1.0 / settings.sim_hz.clamp(1.0, 120.0);
    clock.acc += time.delta_secs();
    if clock.acc < step {
        return;
    }
    let dt = clock.acc.min(step * 3.0);
    clock.acc = 0.0;
    let mut points = focus.points.clone();
    if points.is_empty() {
        points.extend(
            cameras
                .iter()
                .filter(|(c, _)| c.active)
                .map(|(_, t)| t.translation()),
        );
    }
    // Volumes step within the shared simulation range (or the smoke's own radius if that is
    // smaller) and rest beyond it and its margin, so ones at the edge don't flicker.
    let (active_m, hysteresis) =
        range.map_or((f64::INFINITY, 0.25), |r| (r.active_m, r.hysteresis));
    let active_m = active_m.min(settings.sim_radius_m as f64);
    let wind_fn = |p: DVec3| turbulent_wind(&wind, p, settings.turbulence, wind.time as f64);
    let pool = AsyncComputeTaskPool::try_get().filter(|_| settings.background);
    let started = std::time::Instant::now();
    let mut stepped = 0;
    let mut waiting = 0;
    for d in &mut domains.list {
        // A step done in the background since the last tick shows now.
        let ready = d.land(false);
        let c = d.domain.center();
        let size = d.domain.size();
        let reach = 0.5 * DVec2::new(size.x, size.z).length();
        let gap = points
            .iter()
            .map(|f| DVec2::new(f.x - c.x, f.z - c.z).length() - reach)
            .fold(f64::INFINITY, f64::min);
        if points.is_empty() || gap <= active_m {
            d.resting = false;
        } else if gap > active_m * (1.0 + hysteresis) {
            d.resting = true;
        }
        if d.resting {
            continue;
        }
        if d.domain.is_idle() {
            d.empty_secs += dt;
            d.stride = 1;
            continue;
        }
        d.empty_secs = 0.0;
        d.owed_secs += dt;
        if d.owed_secs < dt * (d.stride as f32 - 0.5) {
            continue;
        }
        if !ready {
            // Its last step is still running: this one is owed, up to a few ticks' worth.
            d.owed_secs = d.owed_secs.min(step * 3.0 * d.stride as f32);
            waiting += 1;
            continue;
        }
        let params = settings.step_params(d.owed_secs, weather.temperature_c);
        d.owed_secs = 0.0;
        d.stride = d.next_stride(step);
        match pool {
            // Fog settles every frame, so it steps on the spot.
            Some(pool) if !d.domain.fog => d.start(pool, params, &wind_fn),
            _ => d.domain.step(&params, &wind_fn),
        }
        stepped += 1;
    }
    // Volumes made around released smoke go away once their smoke has gone.
    domains
        .list
        .retain(|d| d.owner.is_some() || d.empty_secs < 10.0);
    diagnostics.set("smoke.volumes", domains.list.len() as f64);
    diagnostics.set("smoke.stepped", stepped as f64);
    diagnostics.set("smoke.waiting", waiting as f64);
    diagnostics.set("smoke.mass_kg", domains.mass_kg() as f64);
    diagnostics.set("smoke.step_ms", started.elapsed().as_secs_f64() * 1000.0);
}

/// Age bullet tunnels (every frame, world time).
pub fn age_tunnels(
    time: Res<Time>,
    settings: Res<SmokeSettings>,
    mut tunnels: ResMut<SmokeTunnels>,
) {
    let dt = time.delta_secs();
    for t in &mut tunnels.list {
        t.age += dt;
    }
    let life = settings.tunnel_secs.max(0.05);
    tunnels.list.retain(|t| t.age < life);
    if tunnels.list.len() > 32 {
        let n = tunnels.list.len() - 32;
        tunnels.list.drain(..n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A step in the background lands as the same step on the spot would have, with what was
    /// done to the air while it ran done again on top.
    /// Smoke drifting slowly steps a few times a second, working up to it; a fast plume and
    /// a blast step every tick.
    #[test]
    fn slow_smoke_takes_longer_steps() {
        engine_core::tasks::init_task_pools();
        let mut domain = Domain::new(DVec3::new(-8.0, 0.0, -8.0), 0.5, UVec3::new(32, 32, 32));
        domain.set_floor(|_, _| Some(0.0));
        let mut domains = SmokeDomains::default();
        let i = domains.add(None, domain);
        let d = &mut domains.list[i];
        let puff = |speed: f32, heat_c: f32| Change::Splat {
            at: DVec3::new(0.0, 4.0, 0.0),
            radius: 1.0,
            mass_kg: 0.2,
            heat_c,
            vel: Vec3::new(speed, 0.0, 0.0),
            albedo: Vec3::splat(0.5),
        };
        let mut params = SmokeSettings::default().step_params(0.05, 15.0);
        params.buoyancy = 0.0;
        let tick = 0.05;
        // Cool dust drifting at 1.5 m/s in a calm: half a meter a step takes a tick of 1/3.
        d.change(puff(1.5, 0.0));
        let calm = |_: DVec3| Vec3::new(1.5, 0.0, 0.0);
        let mut strides = Vec::new();
        for _ in 0..5 {
            d.domain.step(&params, &calm);
            d.stride = d.next_stride(tick);
            strides.push(d.stride);
        }
        assert_eq!(
            strides,
            [2, 3, 4, 4, 4],
            "steps grow a tick at a time to the most"
        );

        // A fast plume steps every tick.
        let gale = |_: DVec3| Vec3::new(12.0, 0.0, 0.0);
        d.change(puff(12.0, 0.0));
        d.domain.step(&params, &gale);
        d.domain.step(&params, &gale);
        assert_eq!(d.next_stride(tick), 1);

        // So does air a blast is pushing out.
        d.change(Change::Inflate {
            at: DVec3::new(0.0, 4.0, 0.0),
            radius: 1.0,
            gas_m3: 2.0,
        });
        assert_eq!(d.next_stride(tick), 1);
    }

    #[test]
    fn a_background_step_lands_with_what_was_done_meanwhile() {
        engine_core::tasks::init_task_pools();
        let pool = AsyncComputeTaskPool::get();
        let mut domain = Domain::new(DVec3::new(-8.0, 0.0, -8.0), 0.5, UVec3::new(32, 32, 32));
        domain.set_floor(|_, _| Some(0.0));
        let mut domains = SmokeDomains::default();
        let i = domains.add(None, domain);
        let d = &mut domains.list[i];
        let puff = |x: f64| Change::Splat {
            at: DVec3::new(x, 2.0, 0.0),
            radius: 1.0,
            mass_kg: 0.2,
            heat_c: 300.0,
            vel: Vec3::Y,
            albedo: Vec3::splat(0.5),
        };
        d.change(puff(-2.0));
        let mut on_the_spot = d.domain.clone();
        let params = SmokeSettings::default().step_params(0.05, 15.0);
        let wind = |_: DVec3| Vec3::new(2.0, 0.0, 0.0);
        d.start(pool, params, &wind);
        // The air shows as it was until the step lands, plus what is done to it.
        d.change(puff(3.0));
        assert!(d.background.running());
        let before = d.domain.version;
        d.land(true);
        assert!(!d.background.running());
        assert!(d.domain.version > before);
        on_the_spot.step(&params, &wind);
        on_the_spot.apply(&puff(3.0));
        assert!((d.domain.mass_kg - on_the_spot.mass_kg).abs() < 1e-4);
        let worst = d
            .domain
            .voxels
            .iter()
            .zip(&on_the_spot.voxels)
            .map(|(a, b)| (a.density - b.density).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }
}
