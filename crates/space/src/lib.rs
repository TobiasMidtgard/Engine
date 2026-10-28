#![doc = include_str!("../README.md")]

pub mod conic;
pub mod demo;
pub mod look;
pub mod orbit;
pub mod rails;
pub mod sun;
pub mod system;

use std::collections::HashMap;

use engine_core::math::{DMat3, DQuat, DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_render::prelude::{Atmosphere, DirectionalLight};
use engine_world::physics::Velocity;
use engine_world::{
    Atmospheres, FocusPoints, Gravity, GravityBody, SimulationFocus, StreamFrame, SurfaceMoved,
    WorldShift, WorldShiftSet,
};

pub use conic::Conic;
pub use look::{BodiesFeature, BodyHaze, BodyImpostor, BodyLook, BodyRings, LookBakes};
pub use orbit::{AU, DAY, Orbit};
pub use rails::{NoRails, OnRails, RailsLog};

/// What kind of body (for maps and defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BodyKind {
    Star,
    #[default]
    Planet,
    DwarfPlanet,
    Moon,
}

/// Air around a body as it looks: the sky from its ground, the haze over it and the glow
/// around its limb (drives the renderer's `Atmosphere` when it is the nearest one). The
/// scattering coefficients are the renderer's (see `Atmosphere` there): [`Self::earth`] and
/// [`Self::mars`] are its presets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyAtmosphere {
    pub thickness_m: f32,
    pub rayleigh_scale_height_m: f32,
    pub mie_scale_height_m: f32,
    pub mie_g: f32,
    /// Surface albedo seen from above (linear RGB).
    pub ground_color: [f32; 3],
    /// Rayleigh scattering by the gas at the surface, per meter, RGB.
    pub rayleigh_scattering: [f32; 3],
    /// Mie scattering by haze or dust at the surface, per meter, RGB.
    pub mie_scattering: [f32; 3],
    /// Light the haze or dust absorbs at the surface, per meter, RGB.
    pub mie_absorption: [f32; 3],
    /// Ozone absorption at its densest (25 km up), per meter, RGB; 0 without ozone.
    pub ozone_absorption: [f32; 3],
    /// Added to `mie_g` per channel (a blue glow around the sun where it is larger in blue).
    pub mie_g_offset: [f32; 3],
    /// Scales the density of the whole air (1 = as given).
    pub density_scale: f32,
    /// Share (0..1) of the haze's sharp forward peak passed on as unscattered light (dust on
    /// Mars 1, Earth 0): see `Atmosphere::mie_forward_peak`.
    pub mie_forward_peak: f32,
}

impl Default for BodyAtmosphere {
    fn default() -> Self {
        Self::earth()
    }
}

impl BodyAtmosphere {
    /// Earth's air: a blue sky and red sunsets.
    pub fn earth() -> Self {
        Self::from_render(&Atmosphere::earth())
    }

    /// Mars' thin CO₂ under a veil of dust: a butterscotch sky by day, a blue glow around the
    /// setting sun.
    pub fn mars() -> Self {
        Self::from_render(&Atmosphere::mars())
    }

    /// The air of one of the renderer's atmosphere presets.
    pub fn from_render(a: &Atmosphere) -> Self {
        Self {
            thickness_m: a.thickness,
            rayleigh_scale_height_m: a.rayleigh_scale_height,
            mie_scale_height_m: a.mie_scale_height,
            mie_g: a.mie_g,
            ground_color: a.ground_color,
            rayleigh_scattering: a.rayleigh_scattering,
            mie_scattering: a.mie_scattering,
            mie_absorption: a.mie_absorption,
            ozone_absorption: a.ozone_absorption,
            mie_g_offset: a.mie_g_offset,
            density_scale: a.density_scale,
            mie_forward_peak: a.mie_forward_peak,
        }
    }

    /// Sets the renderer's air to this one (not where the planet is, nor whether it is on).
    pub fn apply(&self, a: &mut Atmosphere) {
        a.thickness = self.thickness_m;
        a.rayleigh_scale_height = self.rayleigh_scale_height_m;
        a.mie_scale_height = self.mie_scale_height_m;
        a.mie_g = self.mie_g;
        a.ground_color = self.ground_color;
        a.rayleigh_scattering = self.rayleigh_scattering;
        a.mie_scattering = self.mie_scattering;
        a.mie_absorption = self.mie_absorption;
        a.ozone_absorption = self.ozone_absorption;
        a.mie_g_offset = self.mie_g_offset;
        a.density_scale = self.density_scale;
        a.mie_forward_peak = self.mie_forward_peak;
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
    /// Tilt of the spin axis against the orbit plane, degrees. The north pole leans toward
    /// ecliptic longitude 90° (where the Sun stands at Earth's June solstice).
    pub axial_tilt_deg: f64,
    /// Spin angle at the epoch, degrees: longitude 0 (body +Z) faces right ascension
    /// (this − 90°) (Earth: Greenwich sidereal time + 90°).
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
    /// Turn of the world about +Y at the home spot, degrees: north points toward −Z at 0 and
    /// toward +Z at 180 (east toward +X / −X). Match the terrain pad's `heading_deg`.
    pub home_heading_deg: f64,
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
    /// In a fixed (not turning) frame: where the world origin is, meters from the home spot
    /// along the world axes. The origin follows the focus this way out in space (see
    /// [`SpaceSettings::recenter_distance_m`]), so positions near it stay fine however far
    /// from any body the flight goes. Zero in a turning frame.
    pub origin_m: [f64; 3],
}

impl Default for SpaceFrame {
    fn default() -> Self {
        Self {
            anchor: String::new(),
            home_lat_deg: 40.0,
            home_lon_deg: 0.0,
            home_height_m: 60.0,
            home_heading_deg: 0.0,
            auto_switch: true,
            rotating: true,
            frozen_orientation: [0.0, 0.0, 0.0, 1.0],
            origin_m: [0.0; 3],
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
    /// Point lights marked [`StarLight`] at their star with the right illuminance. The space
    /// module is then the day/night cycle: unless [`follow_world_clock`](Self::follow_world_clock)
    /// is on, it also writes engine_world's `WeatherState` sun direction, local time and
    /// season (see [`sun`]).
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
    /// Things on or near a body (below half the height where the world stops turning with it)
    /// stay where they are on it while the world is anchored elsewhere or stops turning, and
    /// are put back there, with their motion, when the world turns with that body again. See
    /// [`Parked`].
    pub keep_surfaces: bool,
    /// Follow the world's day/night clock (engine_world `WeatherState::time_of_day_h` and
    /// `day_of_year`, which the weather's sky runs) instead of the space clock's own time and
    /// warp: the clock is set so that the real sun stands at that local solar time and season
    /// over the home spot. For a game that keeps the weather's sun and its time controls;
    /// the Moon's phase and the planets are then lit from where that sun is. The weather's
    /// compass (north +Z) needs [`SpaceFrame::home_heading_deg`] 180 to match the planet's.
    pub follow_world_clock: bool,
    /// Above this time warp, free physics bodies out in space coast on their exact orbits
    /// instead of being simulated (see [`rails`]); physics takes them back below it.
    pub rails_above_warp: f64,
    /// From high up, a body's cloud tops are drawn as a layer over its terrain, except within
    /// this distance (meters along the ground) of the camera, where the weather draws its own
    /// clouds (0: everywhere).
    pub cloud_hole_m: f64,
    /// With [`SpaceFrame::auto_switch`], low over the turning anchor (below
    /// [`rehome_altitude_m`](Self::rehome_altitude_m)) and farther than this along the ground
    /// from the home spot, the home spot moves under the focus: the world origin follows it
    /// across the planet and +Y stays up, as KSP re-centres its floating origin.
    pub rehome_distance_m: f64,
    pub rehome_altitude_m: f64,
    /// With [`SpaceFrame::auto_switch`], coming back within this distance along the ground of
    /// the [`HomeFrame`]'s spot (turning with its anchor) puts the world back in that frame
    /// exactly, so places laid out in home coordinates are where they were.
    pub home_return_m: f64,
    /// With [`SpaceFrame::auto_switch`], in a fixed frame (out in space), the world origin
    /// moves to the focus once it is farther than this from it: a plain shift, as a floating
    /// origin (KSP keeps its within 500 m; physics here keeps its own regions, so the reach
    /// can be longer). Near the origin f64 keeps sub-millimeter positions even light years
    /// from the Sun.
    pub recenter_distance_m: f64,
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
            keep_surfaces: true,
            follow_world_clock: false,
            rails_above_warp: 1.5,
            cloud_hole_m: 60_000.0,
            rehome_distance_m: 40_000.0,
            rehome_altitude_m: 30_000.0,
            home_return_m: 30_000.0,
            recenter_distance_m: 20_000.0,
        }
    }
}

/// Put on a `DirectionalLight` to have it shine from a star (empty name = the brightest).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StarLight {
    pub star: String,
    /// The tint last put on the light's color: another star's color against the Sun's (white
    /// for the Sun), so the light's own color is the Sun's and the tint is taken off again.
    #[serde(skip)]
    pub tint: Option<[f32; 3]>,
}

/// What the world left behind on each body when it stopped turning with it (by body name):
/// put back where it was on that body when the world turns with it again. Trees, houses,
/// props, parked cars and NPCs of a valley would otherwise stay where the old frame mapped
/// them while the planet turns away beneath (the shift can't give things without a velocity
/// the body's spin), and come back kilometers off.
#[derive(Resource, Debug, Clone, Default)]
pub struct Parked {
    pub bodies: HashMap<String, Vec<ParkedEntity>>,
    /// Per parked body: where a point in today's world coordinates sits on that body
    /// (body-local), following every shift since it was parked. On the way back the world's
    /// kept points take it to the body's new pose as one [`SurfaceMoved`].
    kept: HashMap<String, WorldShift>,
    /// The body whose things go back once the world has shifted into its turning frame, and
    /// where that body sits in the world then.
    restore: Option<(String, (DVec3, DQuat))>,
}

/// One thing left on a body: pose and motion in body-local coordinates (+Y north).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParkedEntity {
    pub entity: Entity,
    pub position: DVec3,
    pub rotation: DQuat,
    pub velocity: DVec3,
    pub angular_velocity: DVec3,
}

/// The frame the world started in, for [`travel_home`].
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct HomeFrame(pub SpaceFrame);

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
    /// home = rotation · inertial + offset: the [`HomeFrame`] (the frame the world started
    /// in) right now, for places laid out in home coordinates.
    pub home_from_inertial: Option<(DQuat, DVec3)>,
    /// The world is in its [`HomeFrame`] (same anchor, home spot, turning).
    pub at_home: bool,
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

    /// A world point in home coordinates (where it is in the [`HomeFrame`]).
    pub fn to_home(&self, world: DVec3) -> DVec3 {
        match self.home_from_inertial {
            Some((r, o)) if !self.at_home => r * self.to_inertial(world) + o,
            _ => world,
        }
    }

    /// Where a point given in home coordinates (see [`Self::to_home`]) is in the world now.
    pub fn from_home(&self, home: DVec3) -> DVec3 {
        match self.home_from_inertial {
            Some((r, o)) if !self.at_home => self.to_world(r.inverse() * (home - o)),
            _ => home,
        }
    }

    /// Latitude and longitude on a body right under its star (noon there), degrees.
    pub fn subsolar(&self, name: &str) -> Option<(f64, f64)> {
        let b = self.body(name)?;
        let star = self.bodies.iter().find(|s| s.kind == BodyKind::Star)?;
        let d = (b.orientation.inverse() * (star.inertial_position - b.inertial_position))
            .normalize_or(DVec3::Z);
        Some((
            d.y.clamp(-1.0, 1.0).asin().to_degrees(),
            d.x.atan2(d.z).to_degrees(),
        ))
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

/// The frame's home rotation (turned by its heading) as a `Transform` stores it (f32), back in
/// f64. Using exactly this keeps the home spot on the origin to the millimeter (see
/// `PlanetTerrain::home_transform` and `FlatPad::home_rotation`).
fn stored_home(frame: &SpaceFrame) -> (Quat, DQuat) {
    let q = home_rotation(frame.home_lat_deg, frame.home_lon_deg);
    let q = (DQuat::from_rotation_y(frame.home_heading_deg.to_radians()) * q)
        .normalize()
        .as_quat();
    (q, q.as_dquat().normalize())
}

/// Share of its distance from the Sun within which another star pulls (its sphere of
/// influence; see `propagate`).
const STAR_REACH: f64 = 0.05;

/// Narrowest the sky draws the lit star's disc, degrees: about two pixels on a 1080p screen
/// (it keeps the star's light, so a far sun is a small bright point; much narrower, the sky's
/// f32 disc test can't hold it and the Sun vanishes).
const MIN_STAR_DISC_DEG: f64 = 0.12;

/// Orbits, spin, the world frame, gravity, atmosphere and starlight.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpaceClock>()
            .init_resource::<look::LookBakes>()
            .init_resource::<SpaceFrame>()
            .init_resource::<SpaceSettings>()
            .init_resource::<SpaceState>()
            .init_resource::<Parked>()
            .register_component::<CelestialBody>(
                "A star, planet or moon on rails: mass (mu), radius, orbit around its parent, \
                 spin, atmosphere. Its Transform is driven by the space module.",
            )
            .init_resource::<RailsLog>()
            .register_component::<NoRails>(
                "Keeps a physics body off the rails: it is not flown on its orbit through time \
                 warp (for things the game moves itself).",
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
            .add_systems(
                FixedUpdate,
                (advance_clock, sun::follow_world_clock)
                    .chain()
                    .in_set(SimSet::Environment),
            )
            .init_resource::<PendingTravel>()
            .add_systems(PostUpdate, reanchor.in_set(WorldShiftSet::Write))
            .add_systems(PostUpdate, follow_parked.in_set(WorldShiftSet::Apply))
            .add_systems(
                PostUpdate,
                (
                    look::spawn_impostors,
                    tag_stream_frames,
                    update_space,
                    rails::fly,
                    look::scale_impostors,
                )
                    .chain()
                    .after(WorldShiftSet::Apply)
                    .before(WorldShiftSet::Rebuild),
            );
        app.add_render_feature(BodiesFeature::default());
        register_apis(app);
    }
}

fn advance_clock(time: Res<Time>, settings: Res<SpaceSettings>, mut clock: ResMut<SpaceClock>) {
    if !clock.paused && !settings.follow_world_clock {
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
                let mut soi = b.orbit.sphere_of_influence(b.mu, ps.mu);
                if b.kind == BodyKind::Star {
                    // Another star light years off: its pull matters within a small share of
                    // the way (the Laplace formula, made for a light body, would reach past
                    // the Sun).
                    soi = soi.min(b.orbit.semi_major_axis_m * STAR_REACH);
                }
                (ps.inertial_position + r, ps.inertial_velocity + v, soi)
            }
            None => (DVec3::ZERO, DVec3::ZERO, f64::INFINITY),
        };
        // Rx(−tilt) leans +Y toward −Z, ecliptic longitude 90° (see `orbit`).
        let tilt = DQuat::from_rotation_x(-b.axial_tilt_deg.to_radians());
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
    let (_, home) = stored_home(frame);
    let rot = (home * frame_orientation(anchor, frame).inverse()).normalize();
    let pad = dir_from_lat_lon(frame.home_lat_deg, frame.home_lon_deg)
        * (anchor.radius_m + frame.home_height_m);
    let t = -(home * pad);
    let origin = if frame.rotating {
        DVec3::ZERO
    } else {
        DVec3::from_array(frame.origin_m)
    };
    (rot, t - rot * anchor.inertial_position - origin)
}

/// A new frame with its origin on an inertial point: fixed frames start centered on the
/// focus, turning ones on their home spot.
fn centered(mut next: SpaceFrame, anchor: &BodyState, inertial: DVec3) -> SpaceFrame {
    next.origin_m = [0.0; 3];
    if !next.rotating {
        let (r, o) = frame_for(anchor, &next);
        next.origin_m = (r * inertial + o).to_array();
    }
    next
}

/// The anchor orientation the world follows: the live one when turning, else the frozen one.
fn frame_orientation(anchor: &BodyState, frame: &SpaceFrame) -> DQuat {
    if frame.rotating {
        anchor.orientation
    } else {
        DQuat::from_array(frame.frozen_orientation).normalize()
    }
}

/// Where a body sits in the world (position, body-local to world rotation): the turning
/// anchor exactly as its `Transform` holds it (home spot on the origin, the stored f32
/// rotation), every other body through the world ← inertial `map`.
fn placement(
    s: &BodyState,
    turning_anchor: bool,
    frame: &SpaceFrame,
    map: (DQuat, DVec3),
) -> (DVec3, DQuat) {
    if turning_anchor {
        let (_, home) = stored_home(frame);
        let pad = dir_from_lat_lon(frame.home_lat_deg, frame.home_lon_deg)
            * (s.radius_m + frame.home_height_m);
        (-(home * pad), home)
    } else {
        (
            map.0 * s.inertial_position + map.1,
            (map.0 * s.orientation).normalize(),
        )
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

/// The frame the space module last put the world in: a different [`SpaceFrame`] was set by
/// the game, and becomes the [`HomeFrame`].
#[derive(Resource, Debug, Clone)]
struct WrittenFrame(SpaceFrame);

/// A trip asked for with [`travel_to`], made at the next re-anchoring step.
#[derive(Resource, Debug, Clone, Default)]
struct PendingTravel {
    /// The frame to switch to (a turning frame on the destination).
    trip: Option<SpaceFrame>,
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
    // The home frame is the one the world starts in, or the one a game last set itself.
    let edited = world
        .get_resource::<WrittenFrame>()
        .is_none_or(|w| w.0 != frame);
    if edited || !world.contains_resource::<HomeFrame>() {
        world.insert_resource(HomeFrame(SpaceFrame {
            anchor: states[anchor].name.clone(),
            rotating: true,
            ..frame.clone()
        }));
        world.insert_resource(WrittenFrame(frame.clone()));
    }
    let find = |name: &str| {
        states
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    };

    let trip = world
        .get_resource_mut::<PendingTravel>()
        .and_then(|mut p| p.trip.take());
    let mut next_frame = None;
    if let Some(trip) = trip {
        if let Some(target) = find(&trip.anchor) {
            next_frame = Some(SpaceFrame {
                anchor: states[target].name.clone(),
                rotating: true,
                ..trip
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
        // Out past every planet's sphere of influence the Sun (the root star) holds the focus:
        // the world re-anchors on it, in a fixed frame, and its pull comes back.
        if let Some(d) = probe
            .dominant(inertial)
            .filter(|&d| states[d].kind != BodyKind::Star || states[d].parent.is_none())
            && d != anchor
            && (dist(d) < states[d].soi_m * 0.98 || dist(anchor) > states[anchor].soi_m * 1.02)
        {
            target = d;
        }
        let limit = turning_limit(&states[target], &bodies[target].1, &settings);
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
            next_frame = Some(centered(next, &states[target], inertial));
        } else if !rotating && f.length() > settings.recenter_distance_m {
            // Out in space far from the origin: it moves to the focus (a plain shift).
            next_frame = Some(SpaceFrame {
                origin_m: (DVec3::from_array(frame.origin_m) + f).to_array(),
                ..frame.clone()
            });
        } else if rotating && altitude < settings.rehome_altitude_m {
            // Low over the anchor far from the home spot: home moves under the focus, so the
            // world origin stays near it and +Y stays up.
            let local = states[target].orientation.inverse()
                * (inertial - states[target].inertial_position);
            let dir = local.normalize_or(DVec3::Y);
            let home = dir_from_lat_lon(frame.home_lat_deg, frame.home_lon_deg);
            if dir.angle_between(home) * states[target].radius_m > settings.rehome_distance_m {
                next_frame = Some(SpaceFrame {
                    home_lat_deg: dir.y.clamp(-1.0, 1.0).asin().to_degrees(),
                    home_lon_deg: dir.x.atan2(dir.z).to_degrees(),
                    home_height_m: 0.0,
                    origin_m: [0.0; 3],
                    ..frame.clone()
                });
            }
        }
        // Back near where the world started, turning with that body: its home frame exactly,
        // so whatever is laid out in home coordinates is where it was.
        let home = world.get_resource::<HomeFrame>().map(|h| h.0.clone());
        let next = next_frame.as_ref().unwrap_or(&frame);
        if let Some(home) = home
            && next.rotating
            && !same_frame(&states, next, &home)
            && anchor_index(&states, next) == anchor_index(&states, &home)
        {
            let h = anchor_index(&states, &home);
            let local = states[h].orientation.inverse() * (inertial - states[h].inertial_position);
            let spot = dir_from_lat_lon(home.home_lat_deg, home.home_lon_deg);
            if local.normalize_or(DVec3::Y).angle_between(spot) * states[h].radius_m
                < settings.home_return_m
            {
                next_frame = Some(SpaceFrame {
                    auto_switch: frame.auto_switch,
                    origin_m: [0.0; 3],
                    ..home
                });
            }
        }
    }

    let Some(next_frame) = next_frame else {
        return;
    };
    let target = anchor_index(&states, &next_frame);
    if settings.keep_surfaces {
        if frame.rotating && !(next_frame.rotating && target == anchor) {
            let limit = turning_limit(&states[anchor], &bodies[anchor].1, &settings);
            let at = placement(&states[anchor], true, &frame, current);
            park(world, &states[anchor], at, limit * 0.5);
        }
        if next_frame.rotating {
            let t = &states[target];
            let at = placement(t, true, &next_frame, frame_for(t, &next_frame));
            world.resource_mut::<Parked>().restore = Some((t.name.clone(), at));
        }
    }
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
    // Coming back to a parked body: what modules keep in world points on it goes to where
    // the body is now, as its roots do in `unpark` (after this frame's shift).
    if let Some((name, at)) = world.resource::<Parked>().restore.clone()
        && let Some(kept) = world.resource_mut::<Parked>().kept.remove(&name)
        && let Some(body) = states.iter().find(|s| s.name == name)
    {
        let moved = inverse(&shift).then(&kept).then(&pose(at));
        world.write_message(SurfaceMoved {
            body: body.entity,
            shift: WorldShift {
                velocity: DVec3::ZERO,
                angular_velocity: DVec3::ZERO,
                ..moved
            },
        });
    }
    world.insert_resource(WrittenFrame(next_frame.clone()));
    *world.resource_mut::<SpaceFrame>() = next_frame;
    world.resource_mut::<SpaceState>().switches += 1;
}

/// Two frames put the world in the same place: same anchor, home spot and heading, both
/// turning or both fixed.
fn same_frame(states: &[BodyState], a: &SpaceFrame, b: &SpaceFrame) -> bool {
    anchor_index(states, a) == anchor_index(states, b)
        && a.rotating == b.rotating
        && a.home_lat_deg == b.home_lat_deg
        && a.home_lon_deg == b.home_lon_deg
        && a.home_height_m == b.home_height_m
        && a.home_heading_deg == b.home_heading_deg
        && (a.rotating || a.origin_m == b.origin_m)
}

/// Altitude below which the world turns with a body.
fn turning_limit(s: &BodyState, body: &CelestialBody, settings: &SpaceSettings) -> f64 {
    let air = body.atmosphere.map_or(0.0, |a| a.thickness_m as f64);
    s.radius_m * settings.rotating_altitude_fraction + air
}

/// Leave everything within `reach` (altitude, meters) of `body` on it: its body-local pose
/// and motion go into [`Parked`]. `at` is where the body sits in the world now (turning with
/// it, so motion relative to the world is motion relative to the body). Children follow
/// their parents; the focus, bodies and lights travel with the world.
fn park(world: &mut World, body: &BodyState, at: (DVec3, DQuat), reach: f64) {
    let (pos, rot) = at;
    let inv = rot.inverse();
    world
        .resource_mut::<Parked>()
        .kept
        .insert(body.name.clone(), inverse(&pose(at)));
    let mut q = world.query_filtered::<(Entity, &Transform, Option<&Velocity>), (
        Without<ChildOf>,
        Without<CelestialBody>,
        Without<SimulationFocus>,
        Without<DirectionalLight>,
        Without<OnRails>,
    )>();
    let left: Vec<ParkedEntity> = q
        .iter(world)
        .filter(|(_, tf, _)| (tf.translation - pos).length() - body.radius_m < reach)
        .map(|(entity, tf, v)| {
            let (lin, ang) = v.map_or((Vec3::ZERO, Vec3::ZERO), |v| (v.linear, v.angular));
            ParkedEntity {
                entity,
                position: inv * (tf.translation - pos),
                rotation: (inv * tf.rotation.as_dquat()).normalize(),
                velocity: inv * lin.as_dvec3(),
                angular_velocity: inv * ang.as_dvec3(),
            }
        })
        .collect();
    if left.is_empty() {
        return;
    }
    let mut parked = world.resource_mut::<Parked>();
    let list = parked.bodies.entry(body.name.clone()).or_default();
    list.retain(|p| !left.iter().any(|l| l.entity == p.entity));
    list.extend(left);
}

/// Every body is a frame streamed cells can ride ([`StreamFrame`], named after the body):
/// zones laid out on a planet stay on it while the world is anchored elsewhere.
fn tag_stream_frames(
    mut commands: Commands,
    bodies: Query<(Entity, &CelestialBody), Without<StreamFrame>>,
) {
    for (e, body) in &bodies {
        commands.entity(e).insert(StreamFrame(body.name.clone()));
    }
}

/// A body's pose (body-local to world) as a shift.
fn pose((position, rotation): (DVec3, DQuat)) -> WorldShift {
    WorldShift {
        rotation,
        ..WorldShift::translate(position)
    }
}

/// The shift that undoes `s`'s placement (its motion is dropped).
fn inverse(s: &WorldShift) -> WorldShift {
    let rotation = s.rotation.inverse();
    WorldShift {
        rotation,
        ..WorldShift::translate(-(rotation * s.translation))
    }
}

/// Every shift moves the world's points; the map from them to each parked body follows.
fn follow_parked(mut shifts: MessageReader<WorldShift>, mut parked: ResMut<Parked>) {
    for s in shifts.read() {
        let back = inverse(s);
        for kept in parked.kept.values_mut() {
            *kept = back.then(kept);
        }
    }
}

/// Put what was left on `body` back where it was on it (`at`: where the body sits in the
/// world now, turning with it).
fn unpark(world: &mut World, body: &str, at: (DVec3, DQuat)) {
    let Some(list) = world.resource_mut::<Parked>().bodies.remove(body) else {
        return;
    };
    let (pos, rot) = at;
    for p in list {
        let Ok(mut e) = world.get_entity_mut(p.entity) else {
            continue;
        };
        if let Some(mut tf) = e.get_mut::<Transform>() {
            tf.translation = pos + rot * p.position;
            tf.rotation = (rot * p.rotation).normalize().as_quat();
        }
        if let Some(mut v) = e.get_mut::<Velocity>() {
            v.linear = (rot * p.velocity).as_vec3();
            v.angular = (rot * p.angular_velocity).as_vec3();
        }
        if let Some(mut i) = e.get_mut::<TransformInterpolation>() {
            i.reset();
        }
    }
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
    let (home_q, _) = stored_home(&frame);
    for (i, s) in states.iter_mut().enumerate() {
        (s.world_position, s.world_rotation) =
            placement(s, i == anchor && frame.rotating, &frame, current);
    }
    // Back on a body the world left: its things go back where they were on it.
    let restore = world.resource_mut::<Parked>().restore.take();
    if let Some((name, at)) = restore
        && frame.rotating
        && states[anchor].name == name
    {
        unpark(world, &name, at);
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

    // Gravity: the anchor, plus every body whose reach (1.5× its sphere of influence) holds
    // the focus, pull. The root star and whatever the anchor orbits are left out: the anchor falls
    // around them with us (on the Moon, Earth's pull is only a tide). Out past the planets the
    // world anchors on the Sun, which then pulls.
    if settings.drive_gravity {
        let f = focus.unwrap_or(DVec3::ZERO);
        let mut around = Vec::new();
        let mut up = states[anchor].parent;
        while let Some(p) = up.filter(|p| !around.contains(p)) {
            around.push(p);
            up = states[p].parent;
        }
        let pulls = |i: usize, s: &BodyState| {
            i == anchor
                || (s.parent.is_some()
                    && !around.contains(&i)
                    && (s.world_position - f).length() < s.soi_m * 1.5)
        };
        // The rest follow with no pull (μ 0): where every body is and how big, for whatever
        // slows down or aims toward them (a cruise drive). Pulling bodies come first.
        let (pulling, rest): (Vec<_>, Vec<_>) =
            states.iter().enumerate().partition(|(i, s)| pulls(*i, s));
        let list: Vec<GravityBody> = pulling
            .iter()
            .map(|(_, s)| (s, s.mu))
            .chain(rest.iter().map(|(_, s)| (s, 0.0)))
            .map(|(s, mu)| GravityBody {
                center: s.world_position,
                mu,
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
                a.apply(&mut atmo);
            }
            None => atmo.enabled = false,
        }
    }

    // Starlight.
    if settings.drive_starlight {
        let f = focus.unwrap_or(DVec3::ZERO);
        let root_color = bodies
            .iter()
            .find(|(_, b)| b.kind == BodyKind::Star && b.parent.is_none())
            .map_or([1.0; 3], |(_, b)| b.color);
        let stars: Vec<(String, DVec3, f64, f64, [f32; 3])> = bodies
            .iter()
            .zip(&states)
            .filter(|((_, b), _)| b.luminosity_w > 0.0)
            .map(|((_, b), s)| {
                let rel: [f32; 3] = std::array::from_fn(|i| b.color[i] / root_color[i].max(1e-3));
                let luma = (0.2126 * rel[0] + 0.7152 * rel[1] + 0.0722 * rel[2]).max(1e-3);
                (
                    b.name.clone(),
                    s.world_position,
                    b.luminosity_w,
                    b.radius_m,
                    rel.map(|c| (c / luma).max(1e-3)),
                )
            })
            .collect();
        let mut sun_dir = None;
        let mut lights = world.query::<(&mut StarLight, &mut Transform, &mut DirectionalLight)>();
        for (mut star, mut tf, mut light) in lights.iter_mut(world) {
            let lux = |p: DVec3, l: f64| {
                l / (4.0 * std::f64::consts::PI * (p - f).length_squared().max(1.0))
                    * settings.lumens_per_watt
            };
            let pick = stars
                .iter()
                .filter(|s| star.star.is_empty() || s.0.eq_ignore_ascii_case(&star.star))
                .max_by(|a, b| lux(a.1, a.2).total_cmp(&lux(b.1, b.2)));
            if let Some((_, p, l, r, tint)) = pick {
                let dir = (*p - f).normalize_or(DVec3::Y);
                let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
                *tf = Transform::default().looking_at(-dir, up);
                light.illuminance = lux(*p, *l) as f32;
                // The sky draws the light's star as a disc this wide: its true size, shrinking
                // to a bright point (a few pixels, the same light) far out.
                let size = (2.0 * (r / (*p - f).length().max(*r)).asin()).to_degrees();
                light.angular_size_deg = size.max(MIN_STAR_DISC_DEG) as f32;
                // Another star's light has its color (Sirius blue-white, Proxima red).
                let old = star.tint.unwrap_or([1.0; 3]);
                if old != *tint {
                    light.color = std::array::from_fn(|i| light.color[i] / old[i] * tint[i]);
                    star.tint = Some(*tint);
                }
                sun_dir.get_or_insert(dir);
            }
        }
        // This is the day/night cycle: tell the weather, wildlife and sound where the sun is.
        if let Some(dir) = sun_dir
            && !settings.follow_world_clock
        {
            sun::write_weather_sun(world, &states, anchor, dir);
        }
    }

    let home_frame = world.get_resource::<HomeFrame>().map(|h| h.0.clone());
    let mut state = world.resource_mut::<SpaceState>();
    state.bodies = states;
    state.anchor = Some(anchor);
    state.world_from_inertial = current;
    state.home_from_inertial = home_frame.as_ref().map(|h| {
        let i = anchor_index(&state.bodies, h);
        frame_for(&state.bodies[i], h)
    });
    state.at_home = home_frame
        .as_ref()
        .is_none_or(|h| same_frame(&state.bodies, &frame, h));
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
/// tools). Other entities keep their true positions, and what is on the body left behind
/// stays there (see [`Parked`]). The trip is made in the next
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
    let trip = SpaceFrame {
        anchor: name.clone(),
        home_lat_deg: lat_deg,
        home_lon_deg: lon_deg,
        home_height_m: 0.0,
        ..world.resource::<SpaceFrame>().clone()
    };
    Ok(travel(world, trip, altitude_m))
}

/// Go back to the frame the world started in (its anchor and home spot, so the home level is
/// where it was), with the focus `altitude_m` above the home spot. Like [`travel_to`].
pub fn travel_home(world: &mut World, altitude_m: f64) -> DVec3 {
    let home = world
        .get_resource::<HomeFrame>()
        .map(|h| h.0.clone())
        .unwrap_or_else(|| world.resource::<SpaceFrame>().clone());
    let trip = SpaceFrame {
        auto_switch: world.resource::<SpaceFrame>().auto_switch,
        ..home
    };
    travel(world, trip, altitude_m)
}

/// Go back to the frame the world started in, with the focus at `at` in home coordinates
/// (e.g. a place laid out there), if the world is not in it already: `false` then, and
/// nothing changes. Like [`travel_home`].
pub fn travel_home_to(world: &mut World, at: DVec3) -> bool {
    if world.resource::<SpaceState>().at_home {
        return false;
    }
    let home = world
        .get_resource::<HomeFrame>()
        .map(|h| h.0.clone())
        .unwrap_or_else(|| world.resource::<SpaceFrame>().clone());
    let trip = SpaceFrame {
        auto_switch: world.resource::<SpaceFrame>().auto_switch,
        ..home
    };
    travel_landing(world, trip, at);
    true
}

fn travel(world: &mut World, trip: SpaceFrame, altitude_m: f64) -> DVec3 {
    travel_landing(world, trip, DVec3::new(0.0, altitude_m, 0.0))
}

fn travel_landing(world: &mut World, trip: SpaceFrame, spot: DVec3) -> DVec3 {
    let mut pending = world.get_resource_or_insert_with(PendingTravel::default);
    pending.trip = Some(trip);
    pending.land = Some(spot);
    spot
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
        "space.rails",
        "Ships coasting on their orbits through time warp: the body each falls around, its \
         altitude and speed over it, its lowest and highest points (null: it escapes) and \
         period, and how often coasting ships changed body or dropped the warp near a surface.",
        "{}",
        |world, _| {
            let names: Vec<(String, f64)> = world
                .resource::<SpaceState>()
                .bodies
                .iter()
                .map(|b| (b.name.clone(), b.radius_m))
                .collect();
            let radius = |n: &str| names.iter().find(|b| b.0 == n).map_or(0.0, |b| b.1);
            let mut q = world.query::<(Entity, &OnRails)>();
            let ships: Vec<_> = q
                .iter(world)
                .map(|(e, r)| {
                    let rad = radius(&r.around);
                    let apo = r.conic.apoapsis();
                    serde_json::json!({
                        "entity": e.to_bits(),
                        "around": r.around,
                        "altitude_m": r.conic.position.length() - rad,
                        "speed_m_s": r.conic.velocity.length(),
                        "periapsis_altitude_m": r.conic.periapsis() - rad,
                        "apoapsis_altitude_m": apo.is_finite().then_some(apo - rad),
                        "period_s": r.conic.period(),
                    })
                })
                .collect();
            let log = world.resource::<RailsLog>();
            Ok(serde_json::json!({
                "rate": log.rate,
                "ships": ships,
                "handoffs": log.handoffs,
                "warp_drops": log.warp_drops,
            }))
        },
    )
    .register_api(
        "space.travel",
        "Go to a body: it becomes the world's anchor with lat/lon (degrees) at the world origin, \
         and the focus (camera/player) is placed `altitude` meters above that spot. \
         {\"home\": true} goes back to where the world started.",
        r#"{"body": "Moon", "lat": 0.0, "lon": 0.0, "altitude": 2.0}"#,
        |world, args| {
            let alt = args.get("altitude").and_then(|v| v.as_f64()).unwrap_or(2.0);
            if args.get("home").and_then(|v| v.as_bool()) == Some(true) {
                let at = travel_home(world, alt);
                return Ok(serde_json::json!({ "anchor": "home", "focus_at": at.to_array() }));
            }
            let body = args
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or("Pass \"body\": a body name, e.g. \"Moon\".")?
                .to_string();
            let lat = args.get("lat").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let lon = args.get("lon").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let at = travel_to(world, &body, lat, lon, alt)?;
            Ok(serde_json::json!({ "anchor": body, "focus_at": at.to_array() }))
        },
    );
}

#[cfg(test)]
mod tests;
