//! The physics contract: what every physics module implements and what everything else uses.
//!
//! Rigid-body physics is a replaceable module. Gameplay, vehicles, cloth, destruction,
//! movement, animation (ragdolls) and AI never name a physics crate; they use only the types
//! here:
//!
//! * **Components you put on entities**: [`RigidBody`], [`Collider`] (or a [`FitCollider`] to
//!   have one fitted to a model's meshes), [`Velocity`], [`ExternalForce`], [`Joint`],
//!   [`CharacterController`] (+ [`CharacterMove`] input).
//! * **Components the backend writes back**: `Transform` and [`Velocity`] of dynamic bodies,
//!   [`MassProperties`], [`CharacterState`].
//! * **Messages**: [`ApplyImpulse`] in, [`ContactEvent`] out.
//! * **Queries**: [`PhysicsQueries`] (raycast, shapecast, overlap, closest point), usable from
//!   any system at any time.
//!
//! A backend (the shipped `engine_physics`, or one you write) is a plugin that:
//! 1. calls [`claim_backend`] in `build` (two backends in one app is an error),
//! 2. simulates in `FixedUpdate` / `SimSet::Physics` using `Res<Time>` (so time dilation and
//!    slow motion work), advancing each body by its own step (see "Local time" below),
//!    reading the components above and writing the outputs,
//! 3. replaces [`PhysicsQueries`] with its own implementation,
//! 4. follows world shifts (floating origin, planet frames): in `WorldShiftSet::Apply` it
//!    moves its own state (bodies, statics, islands) by the [`crate::WorldShift`], as a
//!    teleport rather than motion (kinematic bodies get no velocity from it), and by
//!    `WorldShiftSet::Rebuild` its query snapshot answers in the new frame,
//! 5. passes [`conformance`]: `engine_world::physics::conformance::assert_all(|app| { app.add_plugins(MyPhysics); })`.
//!
//! Swapping physics is then a one-line change in the game's `main`.
//!
//! Positions are `f64` world meters (`Transform`), so a backend must keep precision far from
//! the origin (for example by simulating in local islands around each `SimulationFocus`).
//!
//! **Local time.** Every body lives in its own seconds: each fixed step the backend advances
//! a body by `time.delta_for(local)` (`Time::delta_for` with the entity's `LocalTimeScale`, if
//! any), for motion, gravity, forces and damping alike. Without a `LocalTimeScale` that is the
//! world step. So a car inside a slow-motion bubble (`LocalTimeScale { factor: 0.3, .. }`)
//! moves, falls and spins at 0.3 speed while the rest of the world runs normally, and a body
//! with `LocalTimeScale::unaffected_by_dilation(1.0)` keeps full speed while `TimeDilation`
//! slows everything else. [`Velocity`] is in the body's own seconds, like characters'
//! [`CharacterState::velocity`]; multiply by `delta_for(local) / delta_secs()` for world
//! seconds. Contacts between bodies on different clocks are resolved with those world-second
//! velocities. A backend whose solver takes one step for all bodies can do this by scaling a
//! body's velocity by its time factor `s` for the step (and dividing it back after) and its
//! gravity and forces by `s²`.

use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;

use crate::physical::SurfaceId;

// ---------------------------------------------------------------- bodies and colliders

/// How a body moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BodyKind {
    /// Simulated: gravity, forces, collisions.
    #[default]
    Dynamic,
    /// Moved by gameplay (set `Transform` or `Velocity`); pushes dynamic bodies, isn't pushed.
    Kinematic,
    /// Never moves (buildings, terrain).
    Static,
}

/// Makes an entity a physics body. Put a [`Collider`] on it (or on its children) too.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RigidBody {
    pub kind: BodyKind,
    /// Total mass, kg. `None` = from collider volumes and densities.
    pub mass_kg: Option<f32>,
    /// Center of mass in the body's local space, meters. `None` = from the colliders. Set it
    /// to a design's balance point (a plane's CG, a car's low center of mass).
    pub center_of_mass: Option<Vec3>,
    /// Principal moments of inertia about the center of mass, along the body's local axes,
    /// kg·m². `None` = from the colliders.
    pub inertia: Option<Vec3>,
    /// Fraction of linear velocity lost per second (air drag stand-in), 0..1.
    pub linear_damping: f32,
    pub angular_damping: f32,
    /// Multiplies `engine_world::Gravity` for this body (0 = floats, 1 = normal).
    pub gravity_scale: f32,
    /// Continuous collision detection for fast bodies (bullets, cars at speed).
    pub ccd: bool,
    pub can_sleep: bool,
    /// Lock rotation around world X/Y/Z (characters, upright vehicles).
    pub lock_rotation: [bool; 3],
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            mass_kg: None,
            center_of_mass: None,
            inertia: None,
            linear_damping: 0.0,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            ccd: false,
            can_sleep: true,
            lock_rotation: [false; 3],
        }
    }
}

impl RigidBody {
    pub fn dynamic() -> Self {
        Self::default()
    }

    pub fn kinematic() -> Self {
        Self {
            kind: BodyKind::Kinematic,
            ..Self::default()
        }
    }

    pub fn fixed() -> Self {
        Self {
            kind: BodyKind::Static,
            ..Self::default()
        }
    }

    pub fn with_mass(mut self, kg: f32) -> Self {
        self.mass_kg = Some(kg);
        self
    }

    /// Designed mass properties instead of the ones the colliders imply: mass (kg), center
    /// of mass (body-local meters) and principal moments of inertia (kg·m²).
    pub fn with_mass_properties(mut self, kg: f32, center_of_mass: Vec3, inertia: Vec3) -> Self {
        self.mass_kg = Some(kg);
        self.center_of_mass = Some(center_of_mass);
        self.inertia = Some(inertia);
        self
    }
}

/// Collision shape in the collider's local space (meters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ColliderShape {
    Sphere {
        radius: f32,
    },
    /// Box with half extents.
    Cuboid {
        half_extents: Vec3,
    },
    /// Capsule along local Y: total height = 2 × (half_height + radius).
    Capsule {
        radius: f32,
        half_height: f32,
    },
    /// Cylinder along local Y.
    Cylinder {
        radius: f32,
        half_height: f32,
    },
    /// Convex hull of points.
    ConvexHull {
        points: Vec<Vec3>,
    },
    /// Triangle mesh (static geometry; dynamic bodies should use hulls or compounds).
    TriMesh {
        vertices: Vec<Vec3>,
        indices: Vec<[u32; 3]>,
    },
    /// Height grid centered on the collider, `rows × cols` samples spanning `size` (x, z).
    Heightfield {
        heights: Vec<f32>,
        rows: u32,
        cols: u32,
        size: [f32; 2],
    },
    /// Several shapes with local offsets.
    Compound(Vec<(Vec3, Quat, ColliderShape)>),
}

impl Default for ColliderShape {
    fn default() -> Self {
        ColliderShape::Sphere { radius: 0.5 }
    }
}

/// Which groups a collider belongs to and which it collides with (bit masks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollisionLayers {
    pub member: u32,
    pub collides_with: u32,
}

impl Default for CollisionLayers {
    fn default() -> Self {
        Self {
            member: 1,
            collides_with: u32::MAX,
        }
    }
}

impl CollisionLayers {
    pub fn interacts(&self, other: &CollisionLayers) -> bool {
        self.member & other.collides_with != 0 && other.member & self.collides_with != 0
    }
}

/// A collision shape. On a [`RigidBody`] entity or a child of one; alone it is static.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Collider {
    pub shape: ColliderShape,
    /// Friction, restitution and sounds come from this surface material.
    pub surface: SurfaceId,
    /// kg/m³, used when the body has no explicit mass (water 1000, wood 600, steel 7800).
    pub density: f32,
    /// Reports overlaps ([`ContactEvent`]) but doesn't push anything (triggers, pickups).
    pub sensor: bool,
    pub layers: CollisionLayers,
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            shape: ColliderShape::default(),
            surface: SurfaceId::default(),
            density: 1000.0,
            sensor: false,
            layers: CollisionLayers::default(),
        }
    }
}

impl Collider {
    pub fn sphere(radius: f32) -> Self {
        Self {
            shape: ColliderShape::Sphere { radius },
            ..Self::default()
        }
    }

    pub fn cuboid(half_extents: Vec3) -> Self {
        Self {
            shape: ColliderShape::Cuboid { half_extents },
            ..Self::default()
        }
    }

    pub fn capsule(radius: f32, half_height: f32) -> Self {
        Self {
            shape: ColliderShape::Capsule {
                radius,
                half_height,
            },
            ..Self::default()
        }
    }

    pub fn with_surface(mut self, surface: SurfaceId) -> Self {
        self.surface = surface;
        self
    }

    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }
}

/// How a [`FitCollider`] wraps a model, as Unreal's collision presets (simple box, Auto Convex
/// Collision, complex as simple) and Unity's `MeshCollider` (exact, or `convex`) do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Fit {
    /// One box around the whole model. Cheapest.
    Box,
    /// One convex hull (hollows and doorways filled in).
    Hull,
    /// Convex pieces (convex decomposition), so hollows, arches and hulls stay open. Works on
    /// moving bodies.
    Parts,
    /// The exact triangles (the default: scenery that never moves). A dynamic body gets
    /// [`Fit::Parts`] instead.
    #[default]
    Mesh,
}

/// Fit a [`Collider`] to this model's meshes once they have loaded, for scenery and props
/// built from art that has no collision shape of its own (buildings, piers, boats). The
/// backend puts the collider on this entity, in its space, with `surface` and `density`;
/// an entity without a [`RigidBody`] gets [`RigidBody::fixed`] (scenery). Meshes named for
/// collision (Unreal's `UBX_` / `USP_` / `UCP_` / `UCX_` prefixes, Godot's `-colonly` /
/// `-convcolonly` suffixes) win over the fit and are hidden. The backend removes this
/// component once the collider is in.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FitCollider {
    pub fit: Fit,
    pub surface: SurfaceId,
    /// kg/m³, as [`Collider::density`].
    pub density: f32,
}

impl Default for FitCollider {
    fn default() -> Self {
        Self {
            fit: Fit::Mesh,
            surface: SurfaceId::default(),
            density: Collider::default().density,
        }
    }
}

impl FitCollider {
    pub fn new(fit: Fit) -> Self {
        Self {
            fit,
            ..Default::default()
        }
    }

    pub fn with_surface(mut self, surface: SurfaceId) -> Self {
        self.surface = surface;
        self
    }

    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }
}

/// Linear (m/s) and angular (rad/s, world axes) velocity, in the body's own seconds (see
/// "Local time" in the [module docs](self)). Gameplay may write it; the backend writes it back
/// for dynamic bodies every step.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Velocity {
    pub linear: Vec3,
    pub angular: Vec3,
}

/// Force (N) and torque (N·m) applied every step until changed. World axes; the force acts
/// at `at` (world point) if given, else at the center of mass.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalForce {
    pub force: Vec3,
    pub torque: Vec3,
    pub at: Option<DVec3>,
}

/// Written by the backend on every dynamic body: the mass properties it simulates with. They
/// are the [`RigidBody`] overrides where set, otherwise derived from the colliders.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MassProperties {
    pub mass_kg: f32,
    /// Center of mass in the body's local space.
    pub center_of_mass: Vec3,
    /// Principal moments of inertia, kg·m².
    pub inertia: Vec3,
}

/// Velocity of a point on a body (for wheels, cloth coupling, impact speeds).
pub fn point_velocity(v: &Velocity, center_of_mass_world: DVec3, point: DVec3) -> Vec3 {
    v.linear + v.angular.cross((point - center_of_mass_world).as_vec3())
}

// ---------------------------------------------------------------- joints

/// Kind of connection between two bodies. Anchors are in each body's local space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JointKind {
    /// Welded together.
    Fixed,
    /// Ball and socket (ragdoll shoulders, ropes of rigid links), optional cone limit.
    Spherical { cone_limit: Option<f32> },
    /// Hinge around a local axis (doors, knees, wheels), optional angle limits and motor.
    Revolute {
        axis: Vec3,
        limits: Option<[f32; 2]>,
        motor: Option<JointMotor>,
    },
    /// Slides along a local axis (pistons, suspension), optional distance limits and motor.
    Prismatic {
        axis: Vec3,
        limits: Option<[f32; 2]>,
        motor: Option<JointMotor>,
    },
    /// Keeps a distance range (ropes, tethers): `[min, max]` meters.
    Distance { limits: [f32; 2] },
    /// Spring-damper between the anchors (suspension, soft attachments).
    Spring {
        rest_length: f32,
        stiffness: f32,
        damping: f32,
    },
}

/// Drives a joint toward a target velocity or position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JointMotor {
    /// rad/s or m/s.
    pub target_velocity: f32,
    /// rad or m; `None` = velocity motor only.
    pub target_position: Option<f32>,
    pub stiffness: f32,
    pub damping: f32,
    /// Maximum force/torque, N or N·m.
    pub max_force: f32,
}

/// A joint. Lives on its own entity (or on `body_b`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub kind: JointKind,
    pub body_a: Entity,
    /// `None` = attached to the world at `anchor_b` (world position).
    pub body_b: Option<Entity>,
    pub anchor_a: Vec3,
    pub anchor_b: DVec3,
    /// Breaks (and the backend despawns the joint) above this force, N. `None` = unbreakable.
    pub break_force: Option<f32>,
    pub collide_connected: bool,
}

// ---------------------------------------------------------------- character controller

/// Kinematic character body (players, NPCs). `engine_movement` builds wall-running, sliding
/// and dashing on top of it by writing [`CharacterMove`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterController {
    pub radius: f32,
    /// Total standing height, meters.
    pub height: f32,
    /// Highest step climbed without jumping, meters.
    pub step_height: f32,
    /// Steepest walkable slope, degrees.
    pub max_slope_deg: f32,
    /// Stick to the ground when walking down slopes and stairs up to this distance, meters.
    pub snap_distance: f32,
    pub mass_kg: f32,
    /// Push dynamic bodies the character walks into.
    pub push_bodies: bool,
    pub layers: CollisionLayers,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            radius: 0.35,
            height: 1.8,
            step_height: 0.35,
            max_slope_deg: 50.0,
            snap_distance: 0.3,
            mass_kg: 80.0,
            push_bodies: true,
            layers: CollisionLayers::default(),
        }
    }
}

/// Desired motion for a [`CharacterController`] this step: displacement velocity in m/s
/// (world). Gravity is not applied by the controller; the movement module adds it.
///
/// Seconds are the character's own: each fixed step the backend moves it by
/// `velocity × time.delta_for(LocalTimeScale)` (`Time::delta_for` with the entity's
/// `LocalTimeScale`, if any). So a character with `LocalTimeScale::unaffected_by_dilation(1.0)`
/// keeps walking at full speed while `TimeDilation` slows the world, and one with factor 0.5
/// moves at half speed. [`CharacterState::velocity`] is in the same seconds.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterMove {
    pub velocity: Vec3,
}

/// Written by the backend after moving a character.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterState {
    pub grounded: bool,
    pub ground_normal: Vec3,
    #[serde(skip)]
    pub ground_entity: Option<Entity>,
    pub ground_surface: Option<SurfaceId>,
    /// Velocity actually achieved after collisions, m/s in the character's own seconds (see
    /// [`CharacterMove`]).
    pub velocity: Vec3,
    /// Hit a wall this step (normal of the steepest contact), for wall-running.
    pub wall_normal: Option<Vec3>,
    /// Hit a ceiling this step.
    pub ceiling: bool,
}

// ---------------------------------------------------------------- messages

/// Instant change of momentum, N·s (explosions, hits, cloth/soft-body coupling).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ApplyImpulse {
    pub entity: Entity,
    pub impulse: Vec3,
    /// World point it acts at; `None` = center of mass (no spin).
    pub at: Option<DVec3>,
    /// Extra angular impulse, N·m·s.
    pub angular: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactPhase {
    Started,
    Ended,
}

/// Two colliders started or stopped touching. `impulse` is the total normal impulse of the
/// contact this step (N·s; 0 for sensors and `Ended`): impact sounds, damage, footsteps.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ContactEvent {
    pub phase: ContactPhase,
    pub a: Entity,
    pub b: Entity,
    pub point: DVec3,
    /// From `a` toward `b`.
    pub normal: Vec3,
    pub impulse: f32,
    pub sensor: bool,
}

// ---------------------------------------------------------------- queries

/// Filters for scene queries.
#[derive(Debug, Clone, Default)]
pub struct QueryFilter {
    /// Only colliders whose `member` bits intersect this (0 = everything).
    pub layers: u32,
    pub exclude: Vec<Entity>,
    pub include_sensors: bool,
}

impl QueryFilter {
    pub fn excluding(entity: Entity) -> Self {
        Self {
            exclude: vec![entity],
            ..Self::default()
        }
    }

    pub fn accepts(&self, entity: Entity, layers: &CollisionLayers, sensor: bool) -> bool {
        (self.layers == 0 || self.layers & layers.member != 0)
            && (!sensor || self.include_sensors)
            && !self.exclude.contains(&entity)
    }
}

/// A query hit. `entity` is the collider's entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub entity: Entity,
    pub point: DVec3,
    pub normal: Vec3,
    /// Distance along the ray/cast, meters.
    pub distance: f32,
    pub surface: SurfaceId,
}

/// Scene queries against the current physics state. Implementations must be safe to call
/// from any system, any thread, at any time (they read a snapshot updated each step and
/// after each world shift).
pub trait PhysicsQuery: Send + Sync + 'static {
    /// First hit along a ray. `dir` is normalized.
    fn raycast(
        &self,
        origin: DVec3,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit>;
    /// First hit when sweeping `shape` (rotated by `rotation`) from `origin` along `dir`.
    fn shapecast(
        &self,
        shape: &ColliderShape,
        origin: DVec3,
        rotation: Quat,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit>;
    /// Every collider overlapping `shape` at `position`; appended to `out`.
    fn overlap(
        &self,
        shape: &ColliderShape,
        position: DVec3,
        rotation: Quat,
        filter: &QueryFilter,
        out: &mut Vec<Entity>,
    );
    /// Closest point on any collider within `max_distance` of `point` (cloth/particle
    /// collision). `distance` is negative when `point` is inside.
    fn closest_point(&self, point: DVec3, max_distance: f32, filter: &QueryFilter) -> Option<Hit>;
}

/// The active physics queries. Defaults to an empty world (every query misses) until a
/// physics backend installs itself.
#[derive(Resource, Clone)]
pub struct PhysicsQueries(pub Arc<dyn PhysicsQuery>);

struct NoPhysics;

impl PhysicsQuery for NoPhysics {
    fn raycast(&self, _: DVec3, _: Vec3, _: f32, _: &QueryFilter) -> Option<Hit> {
        None
    }
    fn shapecast(
        &self,
        _: &ColliderShape,
        _: DVec3,
        _: Quat,
        _: Vec3,
        _: f32,
        _: &QueryFilter,
    ) -> Option<Hit> {
        None
    }
    fn overlap(&self, _: &ColliderShape, _: DVec3, _: Quat, _: &QueryFilter, _: &mut Vec<Entity>) {}
    fn closest_point(&self, _: DVec3, _: f32, _: &QueryFilter) -> Option<Hit> {
        None
    }
}

impl Default for PhysicsQueries {
    fn default() -> Self {
        Self(Arc::new(NoPhysics))
    }
}

impl std::ops::Deref for PhysicsQueries {
    type Target = dyn PhysicsQuery;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

/// Which physics backend is installed (set by [`claim_backend`]).
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct PhysicsBackend {
    pub name: String,
}

/// Call from a physics plugin's `build`. Panics with a clear message if another backend is
/// already installed, so swapping backends can't silently run two at once.
pub fn claim_backend(app: &mut App, name: &str) {
    if let Some(existing) = app.world.get_resource::<PhysicsBackend>() {
        panic!(
            "Two physics backends: \"{}\" is installed and \"{name}\" tried to install too. \
             Add only one physics plugin.",
            existing.name
        );
    }
    app.insert_resource(PhysicsBackend {
        name: name.to_string(),
    });
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<PhysicsQueries>()
        .add_message::<ApplyImpulse>()
        .add_message::<ContactEvent>()
        .register_component::<RigidBody>(
            "Physics body: dynamic/kinematic/static, mass (kg), damping, gravity scale, CCD.",
        )
        .register_component::<Collider>(
            "Collision shape (meters), surface material, density (kg/m³), sensor flag, layers.",
        )
        .register_component::<FitCollider>(
            "Fit a collider to the model's meshes once they load: box, hull, parts or mesh.",
        )
        .register_component::<Velocity>("Linear (m/s) and angular (rad/s) velocity.")
        .register_component::<ExternalForce>("Continuous force (N) and torque (N·m).")
        .register_component::<MassProperties>(
            "Mass (kg), center of mass, inertia (written by physics).",
        )
        .register_component::<CharacterController>(
            "Kinematic character capsule: radius, height, step height, max slope (deg).",
        )
        .register_component::<CharacterMove>("Desired character velocity this step (m/s).")
        .register_component::<CharacterState>(
            "Character result: grounded, ground normal/surface, velocity, wall contact.",
        );
}

pub mod conformance;
