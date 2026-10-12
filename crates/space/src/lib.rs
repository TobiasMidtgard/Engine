#![doc = include_str!("../README.md")]

pub mod demo;
pub mod look;
pub mod orbit;
pub mod system;

use std::collections::HashMap;

use engine_core::math::{DMat3, DQuat, DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_render::prelude::{Atmosphere, DirectionalLight};
use engine_world::physics::Velocity;
use engine_world::{
    Atmospheres, FocusPoints, Gravity, GravityBody, SimulationFocus, WorldShift, WorldShiftSet,
};

pub use look::{BodiesFeature, BodyHaze, BodyImpostor, BodyLook, BodyRings, LookBakes};
pub use orbit::{AU, DAY, Orbit};

/// What kind of body (for maps and defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BodyKind {
    Star,
    #[default]
    Planet,
    DwarfPlanet,
    Moon,
}

/// Air around a body (drives the renderer's `Atmosphere` when it is the nearest one).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyAtmosphere {
    pub thickness_m: f32,
    pub rayleigh_scale_height_m: f32,
    pub mie_scale_height_m: f32,
    pub mie_g: f32,
    /// Surface albedo seen from above (linear RGB).
    pub ground_color: [f32; 3],
}

impl Default for BodyAtmosphere {
    fn default() -> Self {
        Self {
            thickness_m: 100_000.0,
            rayleigh_scale_height_m: 8_000.0,
            mie_scale_height_m: 1_200.0,
            mie_g: 0.8,
            ground_color: [0.12, 0.15, 0.09],
        }
    }
}

/// The air a body holds, for flight, drag, sound and weather (written to engine_world's
/// [`Atmospheres`], where `Env::air` reads it). Gas giants give it at the 1 bar level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyAir {
    /// Density at the surface, kg/m³ (Earth 1.225, Mars 0.020, Venus 65; 0 = airless).
    pub surface_density: f32,
    /// Altitude over which density falls by e, meters.
    pub scale_height_m: f32,
    /// No air above this altitude, meters.
    pub top_m: f32,
    /// Mean surface temperature, °C.
    pub temperature_c: f32,
}

impl BodyAir {
    /// No air at all (the Moon, Mercury).
    pub const NONE: BodyAir = BodyAir {
        surface_density: 0.0,
        scale_height_m: 1.0,
        top_m: 0.0,
        temperature_c: 0.0,
    };
}

impl Default for BodyAir {
    fn default() -> Self {
        Self::NONE
    }
}

/// A star, planet or moon. Its `Transform` is driven by the space module: bodies follow their
/// orbits on rails and spin about their axes, and the anchor body (see [`SpaceFrame`]) stays
/// fixed in the world. Put a `PlanetTerrain` (engine_terrain) on the same entity to give it a
/// surface.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct CelestialBody {
    pub name: String,
    pub kind: BodyKind,
    /// Gravitational parameter G·M, m³/s².
    pub mu: f64,
    /// Mean surface radius, meters.
    pub radius_m: f64,
    /// Body this one orbits (by name); `None` for the root (usually the star).
    pub parent: Option<String>,
    pub orbit: Orbit,
    /// Sidereal rotation period, seconds (0 = does not spin).
    pub rotation_period_s: f64,
    /// Tilt of the spin axis against the orbit plane, degrees.
    pub axial_tilt_deg: f64,
    /// Spin angle at the epoch, degrees.
    pub rotation_at_epoch_deg: f64,
    /// Always shows the same face to its parent (like the Moon).
    pub tidally_locked: bool,
    pub atmosphere: Option<BodyAtmosphere>,
    /// Physical air. `None`: airless, unless the body has an [`atmosphere`](Self::atmosphere),
    /// in which case it gets engine_world's weather-driven standard (Earth) atmosphere.
    pub air: Option<BodyAir>,
    /// Radiated power, watts (stars; the Sun ≈ 3.828e26).
    pub luminosity_w: f64,
    /// Color for maps and far-away dots (linear RGB).
    pub color: [f32; 3],
    /// Surface pattern of the far view.
    pub look: BodyLook,
    /// Rings in the equatorial plane.
    pub rings: Option<BodyRings>,
    /// Haze or thin air seen from afar (a glowing rim, a softer terminator). Bodies whose
    /// `atmosphere` the renderer is drawing use that instead.
    pub haze: Option<BodyHaze>,
    /// Polar flattening, (equatorial − polar) / equatorial radius (Saturn ≈ 0.098). The far
    /// view is drawn as this spheroid with `radius_m` as its mean radius.
    pub flattening: f64,
}

impl Default for CelestialBody {
    fn default() -> Self {
        Self {
            name: "Body".into(),
            kind: BodyKind::Planet,
            mu: 3.986_004_418e14,
            radius_m: 6_371_000.0,
            parent: None,
            orbit: Orbit::default(),
            rotation_period_s: 0.0,
            axial_tilt_deg: 0.0,
            rotation_at_epoch_deg: 0.0,
            tidally_locked: false,
            atmosphere: None,
            air: None,
            luminosity_w: 0.0,
            color: [0.5, 0.5, 0.5],
            look: BodyLook::Rocky,
            rings: None,
            haze: None,
            flattening: 0.0,
        }
    }
}

/// The space clock: seconds since the epoch and the time warp.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpaceClock {
    /// Seconds since the epoch (orbits are given at 0).
    pub seconds: f64,
    /// Space seconds per world second (1 = real time; 1000 makes a Moon month pass in 40 min).
    pub warp: f64,
    pub paused: bool,
}

impl Default for SpaceClock {
    fn default() -> Self {
        Self {
            seconds: 0.0,
            warp: 1.0,
            paused: false,
        }
    }
}

/// How bodies map into the world. The anchor body is fixed in the world and turns with it:
/// its `home` spot is at the world origin with +Y up, north toward −Z and east toward +X (the
/// same placement as `PlanetTerrain::home_transform`). Everything else moves around it.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpaceFrame {
    /// Name of the anchor body ("" = the first planet).
    pub anchor: String,
    pub home_lat_deg: f64,
    pub home_lon_deg: f64,
    /// Height of the home spot above the body's radius, meters. Match the terrain pad's
    /// `height_m` (60 m by default) so the ground is at y = 0.
    pub home_height_m: f64,
    /// Re-anchor on the body whose sphere of influence the focus enters or leaves, so that
    /// flying to another planet ends in that planet's frame, and stop/start turning with the
    /// anchor when the focus climbs out of / descends into its low space.
    pub auto_switch: bool,
    /// The world turns with the anchor (its surface stands still). Off high above it: the
    /// world then keeps a fixed orientation in space and the anchor spins in it, so orbits and
    /// interplanetary flight see no huge frame velocities.
    pub rotating: bool,
    /// The anchor's orientation (x, y, z, w) when the world stopped turning.
    pub frozen_orientation: [f64; 4],
}

impl Default for SpaceFrame {
    fn default() -> Self {
        Self {
            anchor: String::new(),
            home_lat_deg: 40.0,
            home_lon_deg: 0.0,
            home_height_m: 60.0,
            auto_switch: true,
            rotating: true,
            frozen_orientation: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// What the space module drives.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpaceSettings {
    /// Write `Gravity::bodies` (and zero `Gravity::uniform`).
    pub drive_gravity: bool,
    /// Set the renderer's `Atmosphere` to the nearest body with air.
    pub drive_atmosphere: bool,
    /// Write engine_world's `Atmospheres::bodies` with every planet's and moon's air (airless
    /// ones too, so the Moon has none), for flight, drag, sound and weather.
    pub drive_air: bool,
    /// Point lights marked [`StarLight`] at their star with the right illuminance.
    pub drive_starlight: bool,
    /// Luminous efficacy of starlight, lumens per watt (sunlight ≈ 93).
    pub lumens_per_watt: f64,
    /// The world turns with the anchor below this altitude, as a fraction of its radius (plus
    /// the top of its atmosphere).
    pub rotating_altitude_fraction: f64,
    /// Draw a textured sphere (and rings) for every body that is not a star; a terrain on the
    /// body covers it up close.
    pub draw_bodies: bool,
    /// Far bodies stay at least this wide (half-angle, radians) so they show as dots.
    pub min_apparent_radius_rad: f64,
}

impl Default for SpaceSettings {
    fn default() -> Self {
        Self {
            drive_gravity: true,
            drive_atmosphere: true,
            drive_air: true,
            drive_starlight: true,
            lumens_per_watt: 93.0,
            rotating_altitude_fraction: 0.15,
            draw_bodies: true,
            min_apparent_radius_rad: 7e-4,
        }
    }
}

/// Put on a `DirectionalLight` to have it shine from a star (empty name = the brightest).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StarLight {
    pub star: String,
}

/// Where a body is right now (computed every frame).
#[derive(Debug, Clone)]
pub struct BodyState {
    pub entity: Entity,
    pub name: String,
    pub kind: BodyKind,
    pub radius_m: f64,
    pub mu: f64,
    pub parent: Option<usize>,
    /// Position and velocity in the inertial (star-centered, non-rotating) frame.
    pub inertial_position: DVec3,
    pub inertial_velocity: DVec3,
    /// Body-local (north = +Y) to inertial rotation, and spin (rad/s, inertial axes).
    pub orientation: DQuat,
    pub spin: DVec3,
    /// In the world (the anchor's frame).
    pub world_position: DVec3,
    pub world_rotation: DQuat,
    /// Sphere of influence radius, meters (infinite for the root).
    pub soi_m: f64,
}

/// Every body's current state and the world ↔ inertial mapping. Read it for maps and HUDs.
#[derive(Resource, Debug, Clone, Default)]
pub struct SpaceState {
    pub bodies: Vec<BodyState>,
    /// Index of the anchor in `bodies`.
    pub anchor: Option<usize>,
    /// world = rotation · inertial + offset.
    pub world_from_inertial: (DQuat, DVec3),
    /// Anchor switches since startup.
    pub switches: u64,
}

impl SpaceState {
    pub fn body(&self, name: &str) -> Option<&BodyState> {
        self.bodies
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
    }

    pub fn to_world(&self, inertial: DVec3) -> DVec3 {
        let (r, o) = self.world_from_inertial;
        r * inertial + o
    }

    pub fn to_inertial(&self, world: DVec3) -> DVec3 {
        let (r, o) = self.world_from_inertial;
        r.inverse() * (world - o)
    }

    /// The body whose sphere of influence most tightly holds an inertial point.
    pub fn dominant(&self, inertial: DVec3) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, b) in self.bodies.iter().enumerate() {
            if b.kind == BodyKind::Star && b.parent.is_some() {
                continue;
            }
            let d = (inertial - b.inertial_position).length();
            if d < b.soi_m && best.is_none_or(|(_, s)| b.soi_m < s) {
                best = Some((i, b.soi_m));
            }
        }
        best.map(|(i, _)| i)
    }

    /// World point on a body's surface at latitude/longitude and height above its radius.
    pub fn surface_point(
        &self,
        name: &str,
        lat_deg: f64,
        lon_deg: f64,
        height_m: f64,
    ) -> Option<DVec3> {
        let b = self.body(name)?;
        let local = dir_from_lat_lon(lat_deg, lon_deg) * (b.radius_m + height_m);
        Some(b.world_position + b.world_rotation * local)
    }

    /// Latitude, longitude and height above the radius of a world point over a body.
    pub fn lat_lon_height(&self, name: &str, world: DVec3) -> Option<(f64, f64, f64)> {
        let b = self.body(name)?;
        let local = b.world_rotation.inverse() * (world - b.world_position);
        let r = local.length();
        let d = local / r.max(1e-9);
        Some((
            d.y.clamp(-1.0, 1.0).asin().to_degrees(),
            d.x.atan2(d.z).to_degrees(),
            r - b.radius_m,
        ))
    }
}

/// Unit direction for latitude/longitude on a body (+Y north, longitude 0 toward +Z, east
/// toward +X). Matches engine_terrain.
pub fn dir_from_lat_lon(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    DVec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos())
}

/// Rotation that puts `lat`/`lon` of a body on top (+Y) with north toward −Z and east toward
/// +X. Matches `engine_terrain::home_rotation`.
pub fn home_rotation(lat_deg: f64, lon_deg: f64) -> DQuat {
    let up = dir_from_lat_lon(lat_deg, lon_deg);
    let east = DVec3::Y.cross(up).try_normalize().unwrap_or(DVec3::X);
    let north = up.cross(east);
    let local = DMat3::from_cols(east, up, north);
    let world = DMat3::from_cols(DVec3::X, DVec3::Y, DVec3::NEG_Z);
    DQuat::from_mat3(&(world * local.transpose())).normalize()
}

/// The home rotation as a `Transform` stores it (f32), back in f64. Using exactly this keeps
/// the home spot on the origin to the millimeter (see `PlanetTerrain::home_transform`).
fn stored_home(lat_deg: f64, lon_deg: f64) -> (Quat, DQuat) {
    let q = home_rotation(lat_deg, lon_deg).as_quat();
    (q, q.as_dquat().normalize())
}

/// Orbits, spin, the world frame, gravity, atmosphere and starlight.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpaceClock>()
            .init_resource::<look::LookBakes>()
            .init_resource::<SpaceFrame>()
            .init_resource::<SpaceSettings>()
            .init_resource::<SpaceState>()
            .register_component::<CelestialBody>(
                "A star, planet or moon on rails: mass (mu), radius, orbit around its parent, \
                 spin, atmosphere. Its Transform is driven by the space module.",
            )
            .register_component::<StarLight>(
                "Makes a DirectionalLight shine from a star (direction and lux follow it).",
            )
            .register_resource::<SpaceClock>(
                "Space time: seconds since the epoch, time warp (1 = real time), paused.",
            )
            .register_resource::<SpaceFrame>(
                "The anchor body that stays fixed in the world and the home spot at the world \
                 origin (latitude/longitude, degrees). auto_switch re-anchors on arrival.",
            )
            .register_resource::<SpaceSettings>(
                "What the space module drives: gravity, atmosphere, starlight, far-view spheres.",
            )
            .add_systems(FixedUpdate, advance_clock.in_set(SimSet::Environment))
            .init_resource::<PendingTravel>()
            .add_systems(PostUpdate, reanchor.in_set(WorldShiftSet::Write))
            .add_systems(
                PostUpdate,
                (look::spawn_impostors, update_space, look::scale_impostors)
                    .chain()
                    .after(WorldShiftSet::Apply)
                    .before(WorldShiftSet::Rebuild),
            );
        app.add_render_feature(BodiesFeature::default());
        register_apis(app);
    }
}

fn advance_clock(time: Res<Time>, mut clock: ResMut<SpaceClock>) {
    if !clock.paused {
        clock.seconds += time.delta_secs_f64() * clock.warp;
    }
}

/// Inertial states of every body at `t` (parents before children).
fn propagate(bodies: &[(Entity, CelestialBody)], t: f64) -> Vec<BodyState> {
    let index: HashMap<&str, usize> = bodies
        .iter()
        .enumerate()
        .map(|(i, (_, b))| (b.name.as_str(), i))
        .collect();
    let parent_of = |i: usize| -> Option<usize> {
        bodies[i]
            .1
            .parent
            .as_deref()
            .and_then(|p| index.get(p).copied())
            .filter(|&p| p != i)
    };
    let mut states: Vec<Option<BodyState>> = vec![None; bodies.len()];
    fn solve(
        i: usize,
        bodies: &[(Entity, CelestialBody)],
        parent_of: &dyn Fn(usize) -> Option<usize>,
        states: &mut Vec<Option<BodyState>>,
        t: f64,
        depth: u32,
    ) {
        if states[i].is_some() {
            return;
        }
        let (entity, b) = &bodies[i];
        let parent = if depth < 16 { parent_of(i) } else { None };
        let (pos, vel, soi) = match parent {
            Some(p) => {
                solve(p, bodies, parent_of, states, t, depth + 1);
                let ps = states[p].as_ref().expect("parent state");
                // Two-body motion: the orbit's period uses both masses.
                let (r, v) = b.orbit.state_at(ps.mu + b.mu, t);
                let soi = b.orbit.sphere_of_influence(b.mu, ps.mu);
                (ps.inertial_position + r, ps.inertial_velocity + v, soi)
            }
            None => (DVec3::ZERO, DVec3::ZERO, f64::INFINITY),
        };
        let tilt = DQuat::from_rotation_x(b.axial_tilt_deg.to_radians());
        let (angle, rate) = match (b.tidally_locked, parent) {
            (true, Some(p)) => {
                let ps = states[p].as_ref().expect("parent state");
                let to_parent = tilt.inverse() * (ps.inertial_position - pos);
                let n = b.orbit.mean_motion(ps.mu + b.mu);
                (to_parent.x.atan2(to_parent.z), n)
            }
            _ if b.rotation_period_s != 0.0 => {
                let rate = std::f64::consts::TAU / b.rotation_period_s;
                (b.rotation_at_epoch_deg.to_radians() + rate * t, rate)
            }
            _ => (b.rotation_at_epoch_deg.to_radians(), 0.0),
        };
        let angle = angle.rem_euclid(std::f64::consts::TAU);
        let orientation = (tilt * DQuat::from_rotation_y(angle)).normalize();
        states[i] = Some(BodyState {
            entity: *entity,
            name: b.name.clone(),
            kind: b.kind,
            radius_m: b.radius_m,
            mu: b.mu,
            parent,
            inertial_position: pos,
            inertial_velocity: vel,
            orientation,
            spin: tilt * DVec3::Y * rate,
            world_position: pos,
            world_rotation: orientation,
            soi_m: soi,
        });
    }
    for i in 0..bodies.len() {
        solve(i, bodies, &parent_of, &mut states, t, 0);
    }
    states.into_iter().map(|s| s.expect("state")).collect()
}

/// The world ↔ inertial mapping for an anchor and home spot.
fn frame_for(anchor: &BodyState, frame: &SpaceFrame) -> (DQuat, DVec3) {
    let (_, home) = stored_home(frame.home_lat_deg, frame.home_lon_deg);
    let rot = (home * frame_orientation(anchor, frame).inverse()).normalize();
    let pad = dir_from_lat_lon(frame.home_lat_deg, frame.home_lon_deg)
        * (anchor.radius_m + frame.home_height_m);
    let t = -(home * pad);
    (rot, t - rot * anchor.inertial_position)
}

/// The anchor orientation the world follows: the live one when turning, else the frozen one.
fn frame_orientation(anchor: &BodyState, frame: &SpaceFrame) -> DQuat {
    if frame.rotating {
        anchor.orientation
    } else {
        DQuat::from_array(frame.frozen_orientation).normalize()
    }
}

/// A frame: anchor state, whether it turns, and the world ← inertial mapping.
struct FrameRef<'a> {
    anchor: &'a BodyState,
    rotating: bool,
    map: (DQuat, DVec3),
}

impl FrameRef<'_> {
    /// Inertial velocity of a point at rest in this frame.
    fn velocity_at(&self, inertial: DVec3) -> DVec3 {
        let spin = if self.rotating {
            self.anchor.spin
        } else {
            DVec3::ZERO
        };
        self.anchor.inertial_velocity + spin.cross(inertial - self.anchor.inertial_position)
    }

    fn spin(&self) -> DVec3 {
        if self.rotating {
            self.anchor.spin
        } else {
            DVec3::ZERO
        }
    }
}

/// The world shift that re-expresses everything from one frame in another, keeping true
/// positions and motion: things at rest in the old frame move with its velocity and spin as
/// seen from the new one.
fn frame_shift(from: &FrameRef, to: &FrameRef) -> WorldShift {
    let (old, new) = (from.map, to.map);
    let rotation = (new.0 * old.0.inverse()).normalize();
    // The old world origin, in inertial space.
    let origin = old.0.inverse() * -old.1;
    WorldShift {
        rotation,
        translation: new.1 - rotation * old.1,
        velocity: new.0 * (from.velocity_at(origin) - to.velocity_at(origin)),
        angular_velocity: new.0 * (from.spin() - to.spin()),
    }
}

/// The anchor body's index: by name, else the first planet, else the first body.
fn anchor_index(states: &[BodyState], frame: &SpaceFrame) -> usize {
    states
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(&frame.anchor))
        .or_else(|| states.iter().position(|b| b.kind == BodyKind::Planet))
        .unwrap_or(0)
}

/// Every body and its definition.
fn collect_bodies(world: &mut World) -> Vec<(Entity, CelestialBody)> {
    let mut q = world.query::<(Entity, &CelestialBody)>();
    q.iter(world).map(|(e, b)| (e, b.clone())).collect()
}

/// A trip asked for with [`travel_to`], made at the next re-anchoring step.
#[derive(Resource, Debug, Clone, Default)]
struct PendingTravel {
    trip: Option<(String, f64, f64)>,
    /// Where the focus lands (world, new frame) once the world has shifted.
    land: Option<DVec3>,
}

/// Before the world shifts (in [`WorldShiftSet::Write`]): decide whether the world
/// re-anchors this frame, on arrival at another body, on climbing out of or descending into
/// a body's low space (the world stops or starts turning with it), or for a trip asked for
/// with [`travel_to`]. If so, write the [`WorldShift`] that takes everything into the new
/// frame; engine_world and every module move their state with it.
pub fn reanchor(world: &mut World) {
    let bodies = collect_bodies(world);
    if bodies.is_empty() {
        return;
    }
    let t = world.resource::<SpaceClock>().seconds;
    let states = propagate(&bodies, t);
    let frame = world.resource::<SpaceFrame>().clone();
    let settings = world.resource::<SpaceSettings>().clone();
    let anchor = anchor_index(&states, &frame);
    let current = frame_for(&states[anchor], &frame);
    let find = |name: &str| {
        states
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    };

    let trip = world
        .get_resource_mut::<PendingTravel>()
        .and_then(|mut p| p.trip.take());
    let mut next_frame = None;
    if let Some((name, lat, lon)) = trip {
        if let Some(target) = find(&name) {
            next_frame = Some(SpaceFrame {
                anchor: states[target].name.clone(),
                home_lat_deg: lat,
                home_lon_deg: lon,
                home_height_m: 0.0,
                rotating: true,
                ..frame.clone()
            });
        }
    } else if frame.auto_switch
        && let Some(f) = focus_position(world)
    {
        // Arrival at another body: re-anchor there with home under the focus.
        let inertial = current.0.inverse() * (f - current.1);
        let probe = SpaceState {
            bodies: states.clone(),
            ..Default::default()
        };
        let dist = |i: usize| (inertial - states[i].inertial_position).length();
        let mut target = anchor;
        if let Some(d) = probe
            .dominant(inertial)
            .filter(|&d| states[d].kind != BodyKind::Star)
            && d != anchor
            && (dist(d) < states[d].soi_m * 0.98 || dist(anchor) > states[anchor].soi_m * 1.02)
        {
            target = d;
        }
        let air = bodies[target]
            .1
            .atmosphere
            .map_or(0.0, |a| a.thickness_m as f64);
        let limit = states[target].radius_m * settings.rotating_altitude_fraction + air;
        let altitude = dist(target) - states[target].radius_m;
        let rotating = if target != anchor || !frame.rotating {
            altitude < limit * 0.95
        } else {
            altitude < limit * 1.05
        };
        if target != anchor || rotating != frame.rotating {
            let mut next = SpaceFrame {
                anchor: states[target].name.clone(),
                rotating,
                ..frame.clone()
            };
            if target != anchor || rotating {
                // Home right under the focus.
                let local = states[target].orientation.inverse()
                    * (inertial - states[target].inertial_position);
                let dir = local.normalize_or(DVec3::Y);
                next.home_lat_deg = dir.y.clamp(-1.0, 1.0).asin().to_degrees();
                next.home_lon_deg = dir.x.atan2(dir.z).to_degrees();
                next.home_height_m = 0.0;
            }
            if !rotating {
                next.frozen_orientation = states[target].orientation.to_array();
            }
            next_frame = Some(next);
        }
    }

    let Some(next_frame) = next_frame else {
        return;
    };
    let target = anchor_index(&states, &next_frame);
    let shift = frame_shift(
        &FrameRef {
            anchor: &states[anchor],
            rotating: frame.rotating,
            map: current,
        },
        &FrameRef {
            anchor: &states[target],
            rotating: next_frame.rotating,
            map: frame_for(&states[target], &next_frame),
        },
    );
    world.write_message(shift);
    *world.resource_mut::<SpaceFrame>() = next_frame;
    world.resource_mut::<SpaceState>().switches += 1;
}

/// Once the world has shifted (after [`WorldShiftSet::Apply`]): move bodies along their orbits
/// with the anchor fixed, land a [`travel_to`] trip, and drive gravity, the air, the sky and
/// starlight.
pub fn update_space(world: &mut World) {
    let bodies = collect_bodies(world);
    if bodies.is_empty() {
        return;
    }
    let t = world.resource::<SpaceClock>().seconds;
    let mut states = propagate(&bodies, t);
    let frame = world.resource::<SpaceFrame>().clone();
    let settings = world.resource::<SpaceSettings>().clone();
    let anchor = anchor_index(&states, &frame);
    let current = frame_for(&states[anchor], &frame);

    // A trip's focus lands above its spot, at rest.
    if let Some(spot) = world
        .get_resource_mut::<PendingTravel>()
        .and_then(|mut p| p.land.take())
    {
        let mut q = world
            .query_filtered::<(&mut Transform, Option<&mut Velocity>), With<SimulationFocus>>();
        for (mut tf, vel) in q.iter_mut(world) {
            tf.translation = spot;
            if let Some(mut v) = vel {
                v.linear = Vec3::ZERO;
                v.angular = Vec3::ZERO;
            }
        }
    }
    let focus = focus_position(world);

    // World placement of every body.
    let (home_q, _) = stored_home(frame.home_lat_deg, frame.home_lon_deg);
    for (i, s) in states.iter_mut().enumerate() {
        if i == anchor && frame.rotating {
            let pad = dir_from_lat_lon(frame.home_lat_deg, frame.home_lon_deg)
                * (s.radius_m + frame.home_height_m);
            let home = home_q.as_dquat().normalize();
            s.world_rotation = home;
            s.world_position = -(home * pad);
        } else {
            s.world_position = current.0 * s.inertial_position + current.1;
            s.world_rotation = (current.0 * s.orientation).normalize();
        }
    }
    for (i, s) in states.iter().enumerate() {
        if let Some(mut tf) = world.get_mut::<Transform>(s.entity) {
            tf.translation = s.world_position;
            tf.rotation = if i == anchor && frame.rotating {
                home_q
            } else {
                s.world_rotation.as_quat()
            };
        }
    }

    // Gravity: the anchor, plus every non-star body whose reach (1.5× its sphere of
    // influence) holds the focus. Stars are left out: the anchor falls around them with us.
    if settings.drive_gravity {
        let f = focus.unwrap_or(DVec3::ZERO);
        let list: Vec<GravityBody> = states
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                *i == anchor
                    || (s.kind != BodyKind::Star && (s.world_position - f).length() < s.soi_m * 1.5)
            })
            .map(|(_, s)| GravityBody {
                center: s.world_position,
                mu: s.mu,
                radius: s.radius_m,
            })
            .collect();
        let mut g = world.resource_mut::<Gravity>();
        g.uniform = Vec3::ZERO;
        g.bodies = list;
    }

    // Physical air for every body (they are matched by μ, so the list does not depend on
    // where they are). Bodies with a sky but no `air` keep the weather-driven standard one.
    if settings.drive_air && world.contains_resource::<Atmospheres>() {
        let list: Vec<engine_world::BodyAtmosphere> = bodies
            .iter()
            .filter(|(_, b)| b.kind != BodyKind::Star)
            .filter_map(|(_, b)| {
                let air = match (b.air, b.atmosphere) {
                    (Some(air), _) => air,
                    (None, Some(_)) => return None,
                    (None, None) => BodyAir::NONE,
                };
                Some(engine_world::BodyAtmosphere {
                    mu: b.mu,
                    sea_level_density: air.surface_density,
                    scale_height_m: air.scale_height_m.max(1.0),
                    top_m: air.top_m,
                    temperature_c: air.temperature_c,
                })
            })
            .collect();
        if world.resource::<Atmospheres>().bodies != list {
            world.resource_mut::<Atmospheres>().bodies = list;
        }
    }

    // Atmosphere: the body with air whose air is closest to the focus.
    if settings.drive_atmosphere && world.contains_resource::<Atmosphere>() {
        let f = focus.unwrap_or(DVec3::ZERO);
        let best = bodies
            .iter()
            .enumerate()
            .filter_map(|(i, (_, b))| b.atmosphere.map(|a| (i, a)))
            .min_by(|(a, aa), (b, ba)| {
                let da = (states[*a].world_position - f).length()
                    - states[*a].radius_m
                    - aa.thickness_m as f64;
                let db = (states[*b].world_position - f).length()
                    - states[*b].radius_m
                    - ba.thickness_m as f64;
                da.total_cmp(&db)
            });
        let mut atmo = world.resource_mut::<Atmosphere>();
        match best {
            Some((i, a)) => {
                atmo.enabled = true;
                atmo.planet_center = states[i].world_position;
                atmo.planet_radius = states[i].radius_m;
                atmo.thickness = a.thickness_m;
                atmo.rayleigh_scale_height = a.rayleigh_scale_height_m;
                atmo.mie_scale_height = a.mie_scale_height_m;
                atmo.mie_g = a.mie_g;
                atmo.ground_color = a.ground_color;
            }
            None => atmo.enabled = false,
        }
    }

    // Starlight.
    if settings.drive_starlight {
        let f = focus.unwrap_or(DVec3::ZERO);
        let stars: Vec<(String, DVec3, f64)> = bodies
            .iter()
            .zip(&states)
            .filter(|((_, b), _)| b.luminosity_w > 0.0)
            .map(|((_, b), s)| (b.name.clone(), s.world_position, b.luminosity_w))
            .collect();
        let mut lights = world.query::<(&StarLight, &mut Transform, &mut DirectionalLight)>();
        for (star, mut tf, mut light) in lights.iter_mut(world) {
            let lux = |p: DVec3, l: f64| {
                l / (4.0 * std::f64::consts::PI * (p - f).length_squared().max(1.0))
                    * settings.lumens_per_watt
            };
            let pick = stars
                .iter()
                .filter(|s| star.star.is_empty() || s.0.eq_ignore_ascii_case(&star.star))
                .max_by(|a, b| lux(a.1, a.2).total_cmp(&lux(b.1, b.2)));
            if let Some((_, p, l)) = pick {
                let dir = (*p - f).normalize_or(DVec3::Y);
                let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
                *tf = Transform::default().looking_at(-dir, up);
                light.illuminance = lux(*p, *l) as f32;
            }
        }
    }

    let mut state = world.resource_mut::<SpaceState>();
    state.bodies = states;
    state.anchor = Some(anchor);
    state.world_from_inertial = current;
}

/// The primary focus right now (current `Transform` for top-level entities, so a focus moved
/// this frame counts before transforms propagate).
fn focus_position(world: &mut World) -> Option<DVec3> {
    let mut q = world.query::<(&SimulationFocus, &Transform, &GlobalTransform, Has<ChildOf>)>();
    q.iter(world)
        .max_by_key(|(f, ..)| f.priority)
        .map(|(_, tf, gt, child)| {
            if child {
                gt.translation()
            } else {
                tf.translation
            }
        })
        .or_else(|| {
            world
                .get_resource::<FocusPoints>()
                .filter(|f| !f.points.is_empty())
                .map(|f| f.primary())
        })
}

/// Put the space module's anchor on `body` with `lat`/`lon` at the world origin, and move every
/// `SimulationFocus` entity to `altitude_m` above that spot (a quick trip, for tours and
/// tools). Other entities keep their true positions. The trip is made in the next
/// `PostUpdate`, through a [`WorldShift`] like any re-anchoring; the focus's new position is
/// returned.
pub fn travel_to(
    world: &mut World,
    body: &str,
    lat_deg: f64,
    lon_deg: f64,
    altitude_m: f64,
) -> Result<DVec3, String> {
    let names: Vec<String> = collect_bodies(world)
        .into_iter()
        .map(|(_, b)| b.name)
        .collect();
    let Some(name) = names.iter().find(|n| n.eq_ignore_ascii_case(body)) else {
        return Err(format!(
            "No body named \"{body}\". Known: {}.",
            names.join(", ")
        ));
    };
    let spot = DVec3::new(0.0, altitude_m, 0.0);
    let mut pending = world.get_resource_or_insert_with(PendingTravel::default);
    pending.trip = Some((name.clone(), lat_deg, lon_deg));
    pending.land = Some(spot);
    Ok(spot)
}

fn parse_vec3(v: Option<&serde_json::Value>) -> Option<DVec3> {
    let a = v?.as_array()?;
    Some(DVec3::new(
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ))
}

fn register_apis(app: &mut App) {
    app.register_api(
        "space.bodies",
        "Stars, planets and moons: where each is in the world, its distance from a point (default \
         the focus), the altitude over it, which one anchors the world and the space time.",
        r#"{"from": [0, 2, 0]}"#,
        |world, args| {
            let from = parse_vec3(args.get("from")).unwrap_or_else(|| {
                world
                    .get_resource::<FocusPoints>()
                    .filter(|f| !f.points.is_empty())
                    .map(|f| f.primary())
                    .unwrap_or(DVec3::ZERO)
            });
            let state = world.resource::<SpaceState>();
            let clock = world.resource::<SpaceClock>();
            let anchor = state.anchor.map(|a| state.bodies[a].name.clone());
            let list: Vec<_> = state
                .bodies
                .iter()
                .map(|b| {
                    let d = (b.world_position - from).length();
                    serde_json::json!({
                        "name": b.name,
                        "kind": format!("{:?}", b.kind),
                        "world_position": b.world_position.to_array(),
                        "distance_m": d,
                        "altitude_m": d - b.radius_m,
                        "radius_m": b.radius_m,
                        "sphere_of_influence_m": if b.soi_m.is_finite() { Some(b.soi_m) } else { None },
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "anchor": anchor,
                "space_seconds": clock.seconds,
                "space_days": clock.seconds / DAY,
                "warp": clock.warp,
                "bodies": list,
            }))
        },
    )
    .register_api(
        "space.warp",
        "Set the time warp (space seconds per world second; 1 = real time) and optionally jump \
         the clock to `days` since the epoch.",
        r#"{"warp": 1000.0, "days": null}"#,
        |world, args| {
            let mut clock = world.resource_mut::<SpaceClock>();
            if let Some(w) = args.get("warp").and_then(|v| v.as_f64()) {
                if !(0.0..=1.0e9).contains(&w) {
                    return Err("warp must be between 0 and 1e9".into());
                }
                clock.warp = w;
            }
            if let Some(d) = args.get("days").and_then(|v| v.as_f64()) {
                clock.seconds = d * DAY;
            }
            Ok(serde_json::json!({ "warp": clock.warp, "space_days": clock.seconds / DAY }))
        },
    )
    .register_api(
        "space.travel",
        "Go to a body: it becomes the world's anchor with lat/lon (degrees) at the world origin, \
         and the focus (camera/player) is placed `altitude` meters above that spot.",
        r#"{"body": "Moon", "lat": 0.0, "lon": 0.0, "altitude": 2.0}"#,
        |world, args| {
            let body = args
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or("Pass \"body\": a body name, e.g. \"Moon\".")?
                .to_string();
            let lat = args.get("lat").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let lon = args.get("lon").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let alt = args.get("altitude").and_then(|v| v.as_f64()).unwrap_or(2.0);
            let at = travel_to(world, &body, lat, lon, alt)?;
            Ok(serde_json::json!({ "anchor": body, "focus_at": at.to_array() }))
        },
    );
}

#[cfg(test)]
mod tests;
