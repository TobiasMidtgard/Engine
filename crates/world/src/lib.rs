//! # engine_world — how simulations talk to each other
//!
//! Weather, water, fire, smoke, foliage, terrain, destruction, vehicles and AI never call each
//! other directly. They meet here:
//!
//! * **Fields** ([`environment`]): wind, weather, ground wetness/snow/scorch, heat. One owner
//!   writes each; anyone reads. Sample them through [`Env`], a system parameter.
//! * **Queries** ([`physical`]): [`Gravity`], [`Ground`], [`GroundWear`], [`Water`] and
//!   [`ForestCover`] answer "what is at this point" for any implementation (heightmap, voxel,
//!   planet; worn paths; lake, ocean, river; painted woods), and [`Obscurance`] answers "how
//!   much smoke, fog or dust is between these two points".
//! * **Air** ([`air`]): density, pressure, temperature, speed of sound and wind anywhere,
//!   per planet (`Env::air`).
//! * **Materials** ([`SurfaceMaterials`]): one table of friction, fuel, deformation, sounds.
//! * **Events** ([`events`]): projectiles, impulses, ground contacts, ignition, damage, noise.
//! * **Focus** ([`SimulationFocus`]): where detailed simulation happens (the player, the camera).
//! * **Using things** ([`interact`]): [`Interactable`] on what the player can use (E), and the
//!   [`Interact`] message its owner acts on.
//! * **World shifts** ([`shift`]): when the world is re-anchored (floating origin, a new
//!   planet's frame), [`WorldShift`] says how; this crate moves everything the contracts hold
//!   and modules move their own caches.
//! * **Combustible** ([`Combustible`]): anything that can burn, declared by its owner (foliage,
//!   props, buildings) and simulated by the fire module.
//! * **Module contracts** for the systems other modules build on, so each module can be ripped
//!   out and replaced: [`physics`] (bodies, colliders, joints, character controller, queries,
//!   plus a [`physics::conformance`] suite every backend must pass), [`animation`] (pose,
//!   parameters, triggers, events, ragdoll, body requests), [`character`] (a person's look,
//!   appearance requests, face controls), [`locomotion`] (move intent and
//!   state, vehicle controls, ladders) and [`terrain`] (edits and change notifications).
//!
//! Units everywhere: meters, kilograms, seconds, °C, m/s.

pub mod air;
pub mod animation;
pub mod character;
pub mod environment;
pub mod events;
pub mod grid;
pub mod interact;
pub mod locomotion;
pub mod physical;
pub mod physics;
pub mod scalability;
pub mod shift;
pub mod terrain;

pub use air::{AirSample, Atmospheres, BodyAtmosphere};
pub use character::{
    Appearance, AppearanceBodies, AppearanceRequest, BodyFit, CharacterQuality, FaceControls,
    HairGuide, HairGuides, HairGuidesPosed, HandControls, Outfit, draws_appearances,
};
pub use environment::*;
pub use events::*;
pub use grid::{Blend, Grid2, Grid3};
pub use interact::{Interact, InteractTarget, Interactable, Interactor, Using};
pub use locomotion::{
    ClimbLine, Climbable, EngineKind, EngineState, Exhaust, ExhaustEffects, ExhaustKind, Exhausts,
    LocomotionMode, LocomotionState, MoveIntent, VehicleControls, WheelContact, WheelContacts,
};
pub use physical::*;
pub use scalability::{QualityPreset, Scalability};
pub use shift::{FollowsWorldShift, SurfaceMoved, WorldShift, WorldShiftAppExt, WorldShiftSet};
pub use terrain::{EditOp, EditShape, TerrainChanged, TerrainEdit};

use engine_core::math::{DVec2, DVec3, Vec3};
use engine_core::prelude::*;

/// Marks an entity that detailed simulation should follow (player, active camera).
/// Field grids re-center on the first focus each frame.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SimulationFocus {
    /// Higher priority wins when choosing the primary focus.
    pub priority: i32,
}

/// World positions of all foci, primary first. Updated every frame after transforms settle.
#[derive(Resource, Debug, Clone, Default)]
pub struct FocusPoints {
    pub points: Vec<DVec3>,
}

impl FocusPoints {
    /// Primary focus, or the origin if there is none.
    pub fn primary(&self) -> DVec3 {
        self.points.first().copied().unwrap_or(DVec3::ZERO)
    }

    /// Distance from `p` to the nearest focus.
    pub fn distance(&self, p: DVec3) -> f64 {
        self.points
            .iter()
            .map(|f| f.distance(p))
            .fold(f64::INFINITY, f64::min)
    }
}

/// How far from the nearest focus the world simulates in full. Beyond it, modules let their
/// things rest: bodies sleep, NPCs stop thinking, cloth, fire, smoke and debris stop stepping
/// (or step rarely), so a big world costs what the player can see, not everything in it.
/// Each module decides what resting means for it; use [`SimulationRange::is_active`] to wake
/// and [`SimulationRange::is_resting`] to put to sleep, so things near the edge don't flicker.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SimulationRange {
    /// Full simulation within this distance (meters) of a focus.
    pub active_m: f64,
    /// Things rest only beyond `active_m × (1 + hysteresis)`.
    pub hysteresis: f64,
}

impl Default for SimulationRange {
    fn default() -> Self {
        Self {
            active_m: 150.0,
            hysteresis: 0.25,
        }
    }
}

impl SimulationRange {
    /// Within the active range of a focus (always true when there is no focus, so tests and
    /// tools without a camera simulate everything).
    pub fn is_active(&self, focus: &FocusPoints, p: DVec3) -> bool {
        focus.points.is_empty() || focus.distance(p) <= self.active_m
    }

    /// Far enough from every focus to rest (never true when there is no focus).
    pub fn is_resting(&self, focus: &FocusPoints, p: DVec3) -> bool {
        !focus.points.is_empty() && focus.distance(p) > self.active_m * (1.0 + self.hysteresis)
    }

    /// Like [`is_active`](Self::is_active) for something `radius` meters across, such as a
    /// demo area or a cloth: active while any part of it is in range.
    pub fn is_active_sphere(&self, focus: &FocusPoints, center: DVec3, radius: f64) -> bool {
        focus.points.is_empty() || focus.distance(center) - radius.max(0.0) <= self.active_m
    }
}

/// Present while a loading screen covers the game (the game inserts it, and removes it when play
/// can start). Nobody watches frame times then, so modules that spread first-use work over frames
/// (planting, mesh and texture builds, uploads) may spend up to `budget_ms` a frame instead of
/// their in-play budget: `world.get_resource::<LoadingBudget>()`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoadingBudget {
    /// Frame-thread time a module may spend per frame on its loading work (ms).
    pub budget_ms: f64,
}

impl Default for LoadingBudget {
    fn default() -> Self {
        Self { budget_ms: 100.0 }
    }
}

/// Burn state of a [`Combustible`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BurnState {
    #[default]
    Intact,
    Smoldering,
    Burning,
    Burnt,
}

/// Something that can catch fire. Owners (foliage, props, buildings) add it; the fire module
/// heats, ignites, burns and extinguishes it, and owners react to `state`
/// (e.g. foliage swaps a burnt tree mesh, destruction collapses a burnt beam).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Combustible {
    /// Fuel left, kg.
    pub fuel_kg: f32,
    /// Moisture 0..1 (wet things resist ignition and smoke more).
    pub moisture: f32,
    /// Ignition temperature, °C.
    pub ignition_c: f32,
    /// Current temperature, °C.
    pub temperature_c: f32,
    pub state: BurnState,
}

impl Default for Combustible {
    fn default() -> Self {
        Self {
            fuel_kg: 5.0,
            moisture: 0.2,
            ignition_c: 300.0,
            temperature_c: 15.0,
            state: BurnState::Intact,
        }
    }
}

/// Convenience bundle of every environment resource for simulation systems.
///
/// ```ignore
/// fn drift(env: Env, mut q: Query<(&mut Transform, &Drifter)>) {
///     for (mut t, _) in &mut q {
///         let wind = env.wind(t.translation);
///         // …
///     }
/// }
/// ```
#[derive(SystemParam)]
pub struct Env<'w> {
    pub weather: Res<'w, WeatherState>,
    pub wind_field: Res<'w, WindField>,
    pub surface: Res<'w, SurfaceLayers>,
    pub heat: Res<'w, HeatField>,
    pub gravity: Res<'w, Gravity>,
    pub atmospheres: Res<'w, Atmospheres>,
    pub ground_query: Res<'w, Ground>,
    pub water_query: Res<'w, Water>,
    pub obscurance: Res<'w, Obscurance>,
    pub materials: Res<'w, SurfaceMaterials>,
    pub focus: Res<'w, FocusPoints>,
}

impl Env<'_> {
    pub fn wind(&self, p: DVec3) -> Vec3 {
        self.wind_field.sample(p)
    }

    /// Air temperature at `p` including altitude and nearby fires, °C.
    pub fn temperature(&self, p: DVec3) -> f32 {
        self.weather.temperature_at_altitude(p.y) + self.heat.sample(p)
    }

    /// Air at `p`: density, pressure, temperature, speed of sound, wind and altitude.
    pub fn air(&self, p: DVec3) -> AirSample {
        self.atmospheres
            .sample(&self.weather, &self.gravity, &self.wind_field, p)
    }

    pub fn gravity(&self, p: DVec3) -> Vec3 {
        self.gravity.at(p)
    }

    pub fn up(&self, p: DVec3) -> Vec3 {
        self.gravity.up_at(p)
    }

    pub fn ground(&self, p: DVec3) -> Option<GroundSample> {
        self.ground_query.sample(p)
    }

    pub fn water(&self, p: DVec3) -> Option<WaterSample> {
        self.water_query.sample(p)
    }

    /// Fraction of light that gets from `from` to `to` through smoke, fog and dust.
    pub fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        self.obscurance.transmittance(from, to)
    }

    pub fn surface_state(&self, p: DVec3) -> SurfaceCell {
        self.surface.sample(p)
    }

    pub fn material(&self, id: SurfaceId) -> &SurfaceMaterial {
        self.materials.get(id)
    }
}

/// Installs the shared world resources, events and focus tracking.
pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        physics::register(app);
        animation::register(app);
        character::register(app);
        locomotion::register(app);
        terrain::register(app);
        shift::register(app);
        interact::register(app);
        app.init_resource::<WeatherState>()
            .init_resource::<WindField>()
            .init_resource::<WindOverride>()
            .init_resource::<SurfaceLayers>()
            .init_resource::<HeatField>()
            .init_resource::<Gravity>()
            .init_resource::<Atmospheres>()
            .init_resource::<Ground>()
            .init_resource::<Water>()
            .init_resource::<ForestCover>()
            .init_resource::<GroundWear>()
            .init_resource::<Obscurance>()
            .init_resource::<SurfaceMaterials>()
            .init_resource::<FocusPoints>()
            .init_resource::<SimulationRange>()
            .init_resource::<Scalability>()
            .add_message::<ProjectileTrace>()
            .add_message::<WorldImpulse>()
            .add_message::<GroundContact>()
            .add_message::<IgniteRequest>()
            .add_message::<DamageEvent>()
            .add_message::<NoiseEvent>()
            .add_message::<SmokeEmission>()
            .add_message::<WaterSplash>()
            .add_message::<WaterDisturbance>()
            .add_message::<AttackEvent>()
            .add_message::<LightningStrike>()
            .register_component::<SimulationFocus>(
                "Detailed simulation (fields, LOD) follows this entity. Put it on the player.",
            )
            .register_component::<Combustible>(
                "Can catch fire: fuel, moisture, ignition temperature, burn state.",
            )
            .register_resource::<WeatherState>(
                "Regional weather: temperature, humidity, clouds, precipitation, wind, fog, \
                 time of day, sun direction.",
            )
            .register_resource::<WindOverride>(
                "Tester's wind: speed_m_s (m/s at 10 m, null = the weather's) sets the \
                 prevailing wind that sway, smoke, fire and cloth feel. F3 panel: PageUp/PageDown.",
            )
            .register_resource::<Gravity>("Uniform gravity plus attracting bodies (planets).")
            .register_resource::<Scalability>(
                "Quality: preset (Low, Medium, High, Ultra) and factors relative to High: \
                 render_scale, shadows, view_distance, density, effects, simulation_m.",
            )
            .register_resource::<SimulationRange>(
                "Meters from the nearest focus that simulate in full; beyond, modules rest.",
            )
            .register_resource::<Atmospheres>(
                "Air per planet (matched by gravitational parameter μ): surface density, scale \
                 height, top, temperature. Unlisted bodies use the weather's standard atmosphere.",
            )
            .register_resource::<SurfaceMaterials>(
                "Surface material table: friction, fuel, deformation, sounds.",
            )
            .add_systems(PreUpdate, scalability::apply_scalability)
            .add_systems(
                PostUpdate,
                (update_focus_system, recenter_fields_system)
                    .chain()
                    .in_set(UpdateSet::AfterPropagate),
            )
            .add_systems(
                FixedUpdate,
                (sync_wind_system, apply_impulses_to_wind_system)
                    .chain()
                    .in_set(SimSet::Environment),
            );
        shift::register_messages(app);
    }
}

fn update_focus_system(
    mut focus: ResMut<FocusPoints>,
    q: Query<(&GlobalTransform, &SimulationFocus)>,
) {
    let mut items: Vec<(i32, DVec3)> = q
        .iter()
        .map(|(g, f)| (f.priority, g.translation()))
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    focus.points.clear();
    focus.points.extend(items.into_iter().map(|(_, p)| p));
}

fn recenter_fields_system(
    focus: Res<FocusPoints>,
    mut surface: ResMut<SurfaceLayers>,
    mut heat: ResMut<HeatField>,
    mut wind: ResMut<WindField>,
) {
    if focus.points.is_empty() {
        return;
    }
    let p = focus.primary();
    let xz = DVec2::new(p.x, p.z);
    // Only shift when the focus has moved a few cells, to keep the copy cost rare.
    let needs = |origin: DVec2, w: u32, cell: f32| {
        let center = origin + DVec2::splat(w as f64 * cell as f64 * 0.5);
        (center - xz).abs().max_element() > (w as f64 * cell as f64) * 0.125
    };
    if needs(surface.grid.origin, surface.grid.width, surface.grid.cell) {
        surface.grid.recenter(xz);
    }
    if needs(heat.grid.origin, heat.grid.width, heat.grid.cell) {
        heat.grid.recenter(xz);
    }
    let l = &wind.local;
    let size = DVec3::new(l.dims[0] as f64, l.dims[1] as f64, l.dims[2] as f64) * l.cell as f64;
    let center = l.origin + size * 0.5;
    if (center - p).abs().max_element() > size.x * 0.125 {
        let cell = l.cell as f64;
        let origin = p - size * 0.5;
        let snapped = (origin / cell).floor() * cell;
        // Wind disturbances are short-lived: simply reset the grid at the new location.
        let dims = l.dims;
        let c = l.cell;
        wind.local = Grid3::new(snapped, c, dims, Vec3::ZERO);
    }
}

fn sync_wind_system(
    time: Res<Time>,
    weather: Res<WeatherState>,
    test: Option<Res<WindOverride>>,
    mut wind: ResMut<WindField>,
) {
    wind.prevailing = test.map_or(weather.wind, |t| t.apply(weather.wind));
    wind.gustiness = weather.gustiness;
    wind.time += time.delta_secs();
    wind.decay(time.delta_secs(), 1.5);
}

fn apply_impulses_to_wind_system(
    mut impulses: MessageReader<WorldImpulse>,
    mut wind: ResMut<WindField>,
) {
    for i in impulses.read() {
        if i.strength > 0.0 {
            wind.add_gust(i.center, i.radius, i.strength, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planet_gravity_points_to_center() {
        let g = Gravity {
            uniform: Vec3::ZERO,
            bodies: vec![GravityBody {
                center: DVec3::ZERO,
                mu: 3.986e14,
                radius: 6.371e6,
            }],
        };
        let a = g.at(DVec3::new(0.0, 6.371e6, 0.0));
        assert!((a.y + 9.82).abs() < 0.05, "{a:?}");
        assert!(
            g.up_at(DVec3::new(6.371e6, 0.0, 0.0))
                .abs_diff_eq(Vec3::X, 1e-5)
        );
    }

    #[test]
    fn plugin_installs_and_runs() {
        let mut app = App::new();
        app.add_plugins(WorldPlugin);
        app.world.spawn((
            Transform::from_xyz(500.0, 0.0, 0.0),
            SimulationFocus::default(),
        ));
        app.update_n(2);
        let focus = app.world.resource::<FocusPoints>();
        assert_eq!(focus.points.len(), 1);
        let surface = app.world.resource::<SurfaceLayers>();
        assert!(surface.grid.contains(DVec3::new(500.0, 0.0, 0.0)));
    }

    #[test]
    fn simulation_range_wakes_near_and_rests_far() {
        let range = SimulationRange::default();
        let none = FocusPoints::default();
        assert!(range.is_active(&none, DVec3::splat(1e6)));
        assert!(!range.is_resting(&none, DVec3::splat(1e6)));
        let focus = FocusPoints {
            points: vec![DVec3::ZERO],
        };
        assert!(range.is_active(&focus, DVec3::new(100.0, 0.0, 0.0)));
        // Between the two edges: neither wakes nor rests, so it keeps its current state.
        let edge = DVec3::new(170.0, 0.0, 0.0);
        assert!(!range.is_active(&focus, edge) && !range.is_resting(&focus, edge));
        assert!(range.is_resting(&focus, DVec3::new(400.0, 0.0, 0.0)));
        assert!(range.is_active_sphere(&focus, DVec3::new(180.0, 0.0, 0.0), 40.0));
    }

    #[test]
    #[should_panic(expected = "Two physics backends")]
    fn two_physics_backends_are_rejected() {
        let mut app = App::new();
        physics::claim_backend(&mut app, "first");
        physics::claim_backend(&mut app, "second");
    }

    #[test]
    fn contracts_are_registered() {
        let mut app = App::new();
        app.add_plugins(WorldPlugin);
        let woods = app.world.resource::<ForestCover>();
        assert_eq!(woods.sample(DVec3::new(10.0, 0.0, -20.0)), None);
        let wear = app.world.resource::<GroundWear>();
        assert_eq!(wear.wear(DVec3::new(10.0, 0.0, -20.0)), 0.0);
        assert_eq!(app.world.resource::<SimulationRange>().active_m, 150.0);
        app.insert_resource(Scalability::preset(QualityPreset::Low));
        app.update();
        assert_eq!(app.world.resource::<SimulationRange>().active_m, 80.0);
        let registry = app.world.resource::<TypeRegistry>();
        for name in [
            "RigidBody",
            "Collider",
            "CharacterController",
            "MoveIntent",
            "AnimationParams",
            "BodyRequest",
            "Climbable",
            "Exhausts",
            "Appearance",
            "AppearanceRequest",
            "FaceControls",
            "CharacterQuality",
            "HairGuides",
            "HairGuidesPosed",
            "HandControls",
            "BodyFit",
        ] {
            assert!(
                registry.component(name).is_some(),
                "{name} is not registered"
            );
        }
        // Without a backend every query misses instead of failing.
        let q = app.world.resource::<physics::PhysicsQueries>();
        assert!(
            q.raycast(
                DVec3::ZERO,
                Vec3::NEG_Y,
                10.0,
                &physics::QueryFilter::default()
            )
            .is_none()
        );
        // Without smoke the air is clear.
        let o = app.world.resource::<Obscurance>();
        assert_eq!(
            o.transmittance(DVec3::ZERO, DVec3::new(0.0, 0.0, 50.0)),
            1.0
        );
    }
}
