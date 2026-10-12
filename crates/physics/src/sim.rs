//! The simulation behind the contract: Rapier worlds ("regions") anchored at f64 positions.
//!
//! # Regions
//!
//! Rapier works in `f32`, which is millimeter-precise only within a few kilometers of its
//! origin. The engine's worlds span light-minutes, so every moving thing lives in a
//! [`Region`]: a separate Rapier world whose local origin is an `f64` anchor near the bodies in
//! it. A crate on the Moon and a car on Earth are in different regions and both simulate with
//! full precision.
//!
//! * A moving body joins the nearest region within [`PhysicsSettings::region_radius_m`] / 2 of
//!   it, or starts a new one there.
//! * Each step a region re-centers its anchor on its moving bodies when they drift away from it,
//!   bodies that wander out of its radius (with everything jointed to them) move to another
//!   region, and two regions whose moving bodies come close merge so they can collide.
//! * Static colliders are copied into every region they could touch (Rapier shares the shape
//!   data, so copies are cheap). A static collider far from every region gets a region of its
//!   own so scene queries still find it.
//! * Regions far from every `SimulationFocus` can be frozen
//!   ([`PhysicsSettings::freeze_distance_m`]).

use std::collections::{BTreeMap, HashMap};

use engine_core::ecs::entity::{EntityHashMap, EntityHashSet};
use engine_core::math::{DVec3, Mat3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::SurfaceId;
use engine_world::physics::{
    BodyKind, CharacterController, Collider, ContactEvent, ContactPhase, Joint, JointKind,
    RigidBody, Velocity,
};
use parking_lot::Mutex;
use rapier3d::prelude::{
    ActiveCollisionTypes, ActiveEvents, ColliderBuilder, ColliderHandle, CollisionEvent,
    CollisionEventFlags, ContactPair, EventHandler, ImpulseJointHandle, LockedAxes,
    MassProperties as RapierMassProperties, PhysicsWorld, Pose, RigidBodyBuilder, RigidBodyHandle,
    RigidBodySet, RigidBodyType, SharedShape, SoftBodySet, SoftBodyTearEvent,
};

use crate::convert;
use crate::settings::PhysicsSettings;

/// Index of a region in [`Simulation::regions`].
pub type RegionId = u32;

/// One Rapier world with its `f64` anchor. See the [module docs](self).
pub struct Region {
    /// World position of the Rapier origin.
    pub anchor: DVec3,
    pub world: PhysicsWorld,
    /// A fixed body at the local origin: the "world" end of joints attached to nothing.
    pub ground: RigidBodyHandle,
    /// Dynamic and kinematic bodies and characters simulated here.
    pub movers: u32,
    /// Static colliders whose home this region is (it must exist so queries find them).
    pub home_statics: u32,
    /// Static colliders copied into this region.
    pub statics: EntityHashSet,
    /// Too far from every focus to simulate this step.
    pub frozen: bool,
    /// Colliders changed while frozen: refresh the query structures before the next query.
    pub needs_refresh: bool,
    /// Collision events from Rapier, drained after every substep.
    pub collector: Collector,
    /// Local bounds of every collider here, refreshed after each step (scene-query culling).
    pub bounds: Option<(Vec3, Vec3)>,
    /// Colliders added or moved since the last step, not yet in the query tree.
    pub fresh: Vec<ColliderHandle>,
    /// Everything moved (the region was re-anchored): refresh the whole query tree.
    pub stale_all: bool,
}

impl Region {
    fn new(anchor: DVec3, settings: &PhysicsSettings) -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = Vec3::ZERO;
        settings.apply_to(&mut world.integration_parameters);
        let ground = world.insert_body(RigidBodyBuilder::fixed());
        Self {
            anchor,
            world,
            ground,
            movers: 0,
            home_statics: 0,
            statics: EntityHashSet::default(),
            frozen: false,
            needs_refresh: true,
            collector: Collector::default(),
            bounds: None,
            fresh: Vec::new(),
            stale_all: false,
        }
    }

    /// World position → this region's local `f32` coordinates.
    pub fn local(&self, p: DVec3) -> Vec3 {
        (p - self.anchor).as_vec3()
    }

    /// Local coordinates → world position.
    pub fn global(&self, v: Vec3) -> DVec3 {
        self.anchor + v.as_dvec3()
    }

    /// World-space bounds of everything in this region (from the broad phase), if any.
    pub fn world_bounds(&self) -> Option<(DVec3, DVec3)> {
        self.bounds
            .map(|(lo, hi)| (self.global(lo), self.global(hi)))
    }

    /// Recompute [`Self::bounds`] from the colliders.
    /// Put colliders added or moved since the last step into the query tree now (the step
    /// would only do it after characters looked for the ground under them).
    pub fn refresh_queries(&mut self) {
        let w = &mut self.world;
        let params = w.integration_parameters;
        let handles: Vec<ColliderHandle> = if self.stale_all {
            w.colliders.iter().map(|(h, _)| h).collect()
        } else {
            std::mem::take(&mut self.fresh)
        };
        for h in handles {
            if let Some(c) = w.colliders.get(h) {
                let aabb = c.compute_broad_phase_aabb(&params, &w.bodies);
                w.broad_phase.set_aabb(&params, h, aabb);
            }
        }
        self.queries_refreshed();
    }

    /// The query tree is up to date (after a step or a refresh).
    pub fn queries_refreshed(&mut self) {
        self.fresh.clear();
        self.stale_all = false;
    }

    pub fn update_bounds(&mut self) {
        self.bounds = self.world.colliders.iter().fold(None, |acc, (_, c)| {
            let a = c.compute_aabb();
            Some(match acc {
                None => (a.mins, a.maxs),
                Some((lo, hi)) => (lo.min(a.mins), hi.max(a.maxs)),
            })
        });
    }
}

/// Collects Rapier collision events during a step.
#[derive(Default)]
pub struct Collector(pub Mutex<Vec<CollisionEvent>>);

impl EventHandler for Collector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &rapier3d::prelude::ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        self.0.lock().push(event);
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &RigidBodySet,
        _colliders: &rapier3d::prelude::ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: f32,
    ) {
    }

    fn handle_soft_body_tear_event(&self, _soft_bodies: &SoftBodySet, _event: &SoftBodyTearEvent) {}
}

/// A dynamic or kinematic body.
pub struct BodyEntry {
    pub region: RegionId,
    pub handle: RigidBodyHandle,
    pub kind: BodyKind,
    pub desc: RigidBody,
    /// Parent entity in the ECS hierarchy (bodies are normally roots).
    pub parent: Option<Entity>,
    /// The `Transform` this backend last wrote (local to `parent`), to tell gameplay
    /// teleports apart from our own writes.
    pub written: Option<(DVec3, Quat)>,
    /// The `Velocity` this backend last wrote.
    pub written_velocity: Option<Velocity>,
    /// Gameplay force (N) and torque (N·m), world axes, and the world point the force acts at.
    pub external: (Vec3, Vec3, Option<DVec3>),
    /// This body's time rate relative to the world (from `LocalTimeScale`).
    pub time_factor: f32,
    /// Forces currently set on the Rapier body (to know when they must be reset).
    pub forces_set: bool,
    /// Moved by gameplay through `Velocity` (kinematic) rather than `Transform`.
    pub kinematic_by_velocity: bool,
    /// Was asleep after the last step (its Transform needs no write).
    pub was_sleeping: bool,
    /// Mass properties need writing back.
    pub mass_dirty: bool,
}

/// A collider: attached to a body, or static (copied into every region it can touch).
pub struct ColliderEntry {
    /// Owning dynamic/kinematic body, `None` for static colliders.
    pub body: Option<Entity>,
    pub desc: Collider,
    pub shape: SharedShape,
    /// Static: world pose and bounding radius around it.
    pub world_pose: (DVec3, Quat),
    pub radius: f64,
    /// Static: the region that must exist for it (its home).
    pub home: Option<RegionId>,
    /// Rapier colliders for it: one for attached colliders, one per region for statics.
    pub handles: Vec<(RegionId, ColliderHandle)>,
}

/// A joint between two bodies (or a body and the world).
pub struct JointEntry {
    pub desc: Joint,
    /// Region and Rapier handle once created.
    pub created: Option<(RegionId, ImpulseJointHandle)>,
}

/// A kinematic character capsule.
pub struct CharacterEntry {
    pub region: RegionId,
    pub body: RigidBodyHandle,
    pub collider: ColliderHandle,
    pub desc: CharacterController,
    /// Translation this backend last wrote.
    pub written: Option<DVec3>,
    /// The moving body the character stood on before this step, and that body's pose then.
    pub platform: Option<(RigidBodyHandle, Pose)>,
    /// Desired velocity this step and its time step (set before moving).
    pub desired: Vec3,
    /// The character's own time step (`Time::delta_for` its `LocalTimeScale`).
    pub dt: f32,
    /// The world's time step, for the platform under the character.
    pub world_dt: f32,
    pub state: engine_world::physics::CharacterState,
}

/// What the step produced for one body, to write back into the ECS.
pub struct BodyWrite {
    pub entity: Entity,
    pub translation: DVec3,
    pub rotation: Quat,
    pub velocity: Velocity,
}

/// A contact that started with a real impact (for footsteps, dents, noise).
#[derive(Debug, Clone, Copy)]
pub struct Impact {
    pub event: ContactEvent,
    /// The moving body that hit something, its mass and the other side's surface.
    pub body: Entity,
    pub mass: f32,
    pub other_static: bool,
    pub other_surface: SurfaceId,
    /// Contact normal pointing from the other side toward the mover.
    pub normal: Vec3,
    pub tangent_speed: f32,
    pub size: [f32; 2],
}

/// Contact bookkeeping: Rapier reports pairs as soon as they are close (speculative contacts);
/// the contract wants "started touching". A pair is pending until the solver pushes it apart
/// or it actually touches, then `Started` is sent with that step's impulse.
#[derive(Default)]
pub struct Contacts {
    /// Close pairs not yet touching: key → (region, handle1, handle2).
    pub pending: BTreeMap<(Entity, Entity), (RegionId, ColliderHandle, ColliderHandle)>,
    /// Touching pairs we reported: key → (a, b, sensor).
    pub touching: BTreeMap<(Entity, Entity), (Entity, Entity, bool)>,
    /// Pairs to re-check after a collider was rebuilt or moved between regions.
    pub recheck: Vec<(Entity, Entity)>,
    pub out: Vec<ContactEvent>,
    pub impacts: Vec<Impact>,
}

fn pair_key(a: Entity, b: Entity) -> (Entity, Entity) {
    if a < b { (a, b) } else { (b, a) }
}

/// Everything the backend knows. Shared (behind a lock) with [`crate::queries`].
#[derive(Default)]
pub struct Simulation {
    pub regions: Vec<Option<Region>>,
    pub bodies: EntityHashMap<BodyEntry>,
    pub colliders: EntityHashMap<ColliderEntry>,
    pub joints: EntityHashMap<JointEntry>,
    pub characters: EntityHashMap<CharacterEntry>,
    pub contacts: Contacts,
    pub settings: PhysicsSettings,
    /// Friction and restitution per surface material.
    pub materials: MaterialCache,
    /// Joints that broke this step (to despawn).
    pub broken_joints: Vec<Entity>,
    /// Body writes produced by the last step.
    pub writes: Vec<BodyWrite>,
    /// Length of the last substep, seconds (turns impulses into forces for debug drawing).
    pub last_sub_dt: f32,
}

/// Friction, restitution and extra rolling resistance (soft ground) of each `SurfaceMaterials`
/// entry, copied when the table changes.
#[derive(Clone, Default)]
pub struct MaterialCache(pub Vec<(f32, f32, f32)>);

impl MaterialCache {
    fn entry(&self, id: SurfaceId) -> (f32, f32, f32) {
        self.0
            .get(id.0 as usize)
            .or_else(|| self.0.first())
            .copied()
            .unwrap_or((0.7, 0.1, 0.0))
    }

    pub fn get(&self, id: SurfaceId) -> (f32, f32) {
        let (f, r, _) = self.entry(id);
        (f, r)
    }

    /// Rolling resistance a surface adds on top of `PhysicsSettings::rolling_resistance`.
    pub fn rolling(&self, id: SurfaceId) -> f32 {
        self.entry(id).2
    }
}

/// Contact distance under which a close pair counts as touching, meters.
const TOUCH_DISTANCE: f32 = 0.01;

impl Simulation {
    // ------------------------------------------------------------------ regions

    pub fn region(&self, id: RegionId) -> &Region {
        self.regions[id as usize].as_ref().expect("live region")
    }

    pub fn region_mut(&mut self, id: RegionId) -> &mut Region {
        self.regions[id as usize].as_mut().expect("live region")
    }

    pub fn live_regions(&self) -> impl Iterator<Item = (RegionId, &Region)> {
        self.regions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i as RegionId, r)))
    }

    fn live_region_ids(&self) -> Vec<RegionId> {
        self.live_regions().map(|(i, _)| i).collect()
    }

    /// Nearest region whose anchor is within `max` meters of `p`.
    fn nearest_region(&self, p: DVec3, max: f64) -> Option<RegionId> {
        self.live_regions()
            .map(|(id, r)| (id, r.anchor.distance(p)))
            .filter(|(_, d)| *d <= max)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// The region a new mover at `p` belongs to (created if needed).
    pub fn region_for_mover(&mut self, p: DVec3) -> RegionId {
        let join = self.settings.region_radius_m * 0.5;
        self.nearest_region(p, join)
            .unwrap_or_else(|| self.create_region(p))
    }

    fn create_region(&mut self, anchor: DVec3) -> RegionId {
        let region = Region::new(anchor, &self.settings);
        let id = match self.regions.iter().position(Option::is_none) {
            Some(i) => {
                self.regions[i] = Some(region);
                i as RegionId
            }
            None => {
                self.regions.push(Some(region));
                (self.regions.len() - 1) as RegionId
            }
        };
        // Copy in every static collider it can touch.
        let reach = self.static_reach();
        let statics: Vec<Entity> = self
            .colliders
            .iter()
            .filter(|(_, c)| {
                c.body.is_none() && c.world_pose.0.distance(anchor) <= c.radius + reach
            })
            .map(|(e, _)| *e)
            .collect();
        for e in statics {
            self.insert_static_copy(e, id);
        }
        id
    }

    fn static_reach(&self) -> f64 {
        self.settings.region_radius_m * 1.25 + 50.0
    }

    /// Remove regions that have nothing left to simulate or answer.
    pub fn prune_regions(&mut self) {
        for id in self.live_region_ids() {
            let r = self.region(id);
            if r.movers == 0 && r.home_statics == 0 {
                let statics: Vec<Entity> = r.statics.iter().copied().collect();
                for e in statics {
                    if let Some(c) = self.colliders.get_mut(&e) {
                        c.handles.retain(|(rid, _)| *rid != id);
                    }
                }
                self.regions[id as usize] = None;
            }
        }
    }

    // ------------------------------------------------------------------ bodies

    /// Create (or recreate) the Rapier body for `entity` at world pose `pose`.
    #[allow(clippy::too_many_arguments)]
    pub fn create_body(
        &mut self,
        entity: Entity,
        desc: &RigidBody,
        pose: (DVec3, Quat),
        velocity: Velocity,
        parent: Option<Entity>,
    ) {
        self.remove_body(entity);
        let region_id = self.region_for_mover(pose.0);
        let region = self.region_mut(region_id);
        let local = region.local(pose.0);
        let kind = match desc.kind {
            BodyKind::Kinematic => RigidBodyType::KinematicPositionBased,
            _ => RigidBodyType::Dynamic,
        };
        let builder = RigidBodyBuilder::new(kind)
            .pose(Pose::from_parts(local, pose.1))
            .linvel(velocity.linear)
            .angvel(velocity.angular)
            .user_data(entity.to_bits() as u128);
        let handle = region.world.bodies.insert(body_params(builder, desc));
        region.movers += 1;
        self.bodies.insert(
            entity,
            BodyEntry {
                region: region_id,
                handle,
                kind: desc.kind,
                desc: desc.clone(),
                parent,
                written: None,
                written_velocity: None,
                external: (Vec3::ZERO, Vec3::ZERO, None),
                time_factor: 1.0,
                forces_set: false,
                kinematic_by_velocity: false,
                was_sleeping: false,
                mass_dirty: true,
            },
        );
    }

    /// Update damping, gravity scale, CCD, sleeping and locks without recreating the body.
    pub fn update_body_params(&mut self, entity: Entity, desc: &RigidBody) {
        let Some(entry) = self.bodies.get_mut(&entity) else {
            return;
        };
        entry.desc = desc.clone();
        let (region, handle) = (entry.region, entry.handle);
        let rb = &mut self.region_mut(region).world.bodies[handle];
        rb.set_linear_damping(convert::damping(desc.linear_damping));
        rb.set_angular_damping(convert::damping(desc.angular_damping));
        rb.set_gravity_scale(desc.gravity_scale, true);
        rb.enable_ccd(desc.ccd);
        rb.activation_mut().normalized_linear_threshold = if desc.can_sleep { 0.4 } else { -1.0 };
        rb.set_locked_axes(locked_axes(desc.lock_rotation), true);
        self.fix_mass(entity);
    }

    /// Remove a body; its colliders are left without a body (the caller re-resolves them).
    pub fn remove_body(&mut self, entity: Entity) -> Vec<Entity> {
        let Some(entry) = self.bodies.remove(&entity) else {
            return Vec::new();
        };
        let attached: Vec<Entity> = self
            .colliders
            .iter()
            .filter(|(_, c)| c.body == Some(entity))
            .map(|(e, _)| *e)
            .collect();
        for e in &attached {
            self.remove_collider_handles(*e);
            if let Some(c) = self.colliders.get_mut(e) {
                c.body = None;
            }
        }
        // Joints on it are dropped by Rapier with the body; mark them for re-creation.
        for j in self.joints.values_mut() {
            if let Some((rid, _)) = j.created
                && rid == entry.region
                && (j.desc.body_a == entity || j.desc.body_b == Some(entity))
            {
                j.created = None;
            }
        }
        let region = self.region_mut(entry.region);
        region.world.remove_body(entry.handle);
        region.movers = region.movers.saturating_sub(1);
        attached
    }

    /// Give a body the mass properties its `RigidBody` asks for: `mass_kg` alone scales the
    /// collider densities (so the mass spreads as the shapes do); a center of mass or inertia
    /// override replaces the colliders' contribution with exactly the designed values.
    pub fn fix_mass(&mut self, entity: Entity) {
        let Some(entry) = self.bodies.get_mut(&entity) else {
            return;
        };
        entry.mass_dirty = true;
        let (region, handle) = (entry.region, entry.handle);
        let desc = entry.desc.clone();
        // Start from the densities the colliders ask for (an earlier override may have scaled
        // or zeroed them).
        let densities: Vec<(ColliderHandle, f32)> = self
            .colliders
            .values()
            .filter(|c| c.body == Some(entity))
            .filter_map(|c| {
                let h = c.handles.iter().find(|(r, _)| *r == region)?.1;
                Some((
                    h,
                    if c.desc.sensor {
                        0.0
                    } else {
                        c.desc.density.max(0.0)
                    },
                ))
            })
            .collect();
        let world = &mut self.region_mut(region).world;
        for (h, d) in &densities {
            if let Some(c) = world.colliders.get_mut(*h) {
                c.set_density(*d);
            }
        }
        let Some(rb) = world.bodies.get_mut(handle) else {
            return;
        };
        rb.set_additional_mass(0.0, true);
        let handles: Vec<ColliderHandle> = rb.colliders().to_vec();
        let solid = || {
            handles
                .iter()
                .filter_map(|h| world.colliders.get(*h))
                .filter(|c| !c.is_sensor())
        };
        let mass = desc.mass_kg.filter(|m| *m > 0.0);
        if desc.center_of_mass.is_none() && desc.inertia.is_none() {
            let Some(target) = mass else {
                return;
            };
            let total: f32 = solid().map(|c| c.mass()).sum();
            if total > 1e-9 {
                for h in &handles {
                    if let Some(c) = world.colliders.get_mut(*h)
                        && !c.is_sensor()
                    {
                        let d = c.density();
                        c.set_density(d * target / total);
                    }
                }
            } else if let Some(rb) = world.bodies.get_mut(handle) {
                rb.set_additional_mass(target, true);
            }
            return;
        }
        // Designed mass properties: what the colliders imply, overridden where given.
        let derived = solid()
            .map(|c| {
                let local = c.position_wrt_parent().copied().unwrap_or_default();
                c.mass_properties().transform_by(&local)
            })
            .fold(RapierMassProperties::default(), |a, b| a + b);
        let m = mass.unwrap_or(derived.mass()).max(1e-6);
        let com = desc.center_of_mass.unwrap_or(derived.local_com);
        let props = match desc.inertia {
            Some(i) => RapierMassProperties::new(com, m, i.max(Vec3::splat(1e-6))),
            None => {
                // The shapes' inertia, scaled to the designed mass and taken about the
                // designed center of mass (parallel axes).
                let scale = if derived.mass() > 1e-9 {
                    m / derived.mass()
                } else {
                    1.0
                };
                let d = com - derived.local_com;
                let shifted = derived.reconstruct_inertia_matrix() * scale
                    + (Mat3::IDENTITY * d.length_squared()
                        - Mat3::from_cols(d * d.x, d * d.y, d * d.z))
                        * m;
                RapierMassProperties::with_inertia_matrix(com, m, shifted)
            }
        };
        for h in &handles {
            if let Some(c) = world.colliders.get_mut(*h) {
                c.set_density(0.0);
            }
        }
        if let Some(rb) = world.bodies.get_mut(handle) {
            rb.set_additional_mass_properties(props, true);
        }
    }

    // ------------------------------------------------------------------ colliders

    fn collider_builder(
        entity: Entity,
        desc: &Collider,
        shape: SharedShape,
        pose: Pose,
        friction: f32,
        restitution: f32,
    ) -> ColliderBuilder {
        let mut b = ColliderBuilder::new(shape)
            .position(pose)
            .friction(friction)
            .restitution(restitution)
            .density(desc.density.max(0.0))
            .sensor(desc.sensor)
            .collision_groups(convert::groups(&desc.layers))
            .user_data(convert::pack_user_data(entity, desc.surface, desc.sensor))
            .active_events(ActiveEvents::COLLISION_EVENTS);
        if desc.sensor {
            b = b
                .density(0.0)
                .active_collision_types(ActiveCollisionTypes::all());
        }
        b
    }

    /// Insert or replace a collider attached to `body` at `local_pose` (relative to the body).
    #[allow(clippy::too_many_arguments)]
    pub fn attach_collider(
        &mut self,
        entity: Entity,
        body: Entity,
        desc: &Collider,
        shape: SharedShape,
        local_pose: Pose,
        friction: f32,
        restitution: f32,
    ) {
        self.remove_collider(entity, false);
        let Some(b) = self.bodies.get(&body) else {
            return;
        };
        let (region_id, handle) = (b.region, b.handle);
        let builder = Self::collider_builder(
            entity,
            desc,
            shape.clone(),
            local_pose,
            friction,
            restitution,
        );
        let region = self.region_mut(region_id);
        let world = &mut region.world;
        let ch = world
            .colliders
            .insert_with_parent(builder, handle, &mut world.bodies);
        region.fresh.push(ch);
        self.colliders.insert(
            entity,
            ColliderEntry {
                body: Some(body),
                desc: desc.clone(),
                shape,
                world_pose: (DVec3::ZERO, Quat::IDENTITY),
                radius: 0.0,
                home: None,
                handles: vec![(region_id, ch)],
            },
        );
        self.fix_mass(body);
    }

    /// Insert or replace a static collider at world pose `pose`.
    pub fn add_static(
        &mut self,
        entity: Entity,
        desc: &Collider,
        shape: SharedShape,
        pose: (DVec3, Quat),
    ) {
        self.remove_collider(entity, false);
        let aabb = shape.compute_local_aabb();
        let radius = (aabb.center().length() + aabb.half_extents().length()) as f64;
        self.colliders.insert(
            entity,
            ColliderEntry {
                body: None,
                desc: desc.clone(),
                shape,
                world_pose: pose,
                radius,
                home: None,
                handles: Vec::new(),
            },
        );
        self.place_static(entity);
    }

    /// (Re)compute which regions a static collider is copied into, creating a home region if
    /// none is near.
    pub fn place_static(&mut self, entity: Entity) {
        let Some(c) = self.colliders.get(&entity) else {
            return;
        };
        let (center, radius) = (c.world_pose.0, c.radius);
        let reach = self.static_reach();
        let wanted: Vec<RegionId> = self
            .live_regions()
            .filter(|(_, r)| r.anchor.distance(center) <= radius + reach)
            .map(|(id, _)| id)
            .collect();
        self.remove_collider_handles(entity);
        for id in &wanted {
            self.insert_static_copy(entity, *id);
        }
        let home_ok = self.colliders[&entity]
            .home
            .is_some_and(|h| self.regions.get(h as usize).is_some_and(|r| r.is_some()));
        if !home_ok {
            let home = match self.nearest_region(center, self.settings.region_radius_m) {
                Some(id) => id,
                None => self.create_region(center),
            };
            self.colliders.get_mut(&entity).unwrap().home = Some(home);
            self.region_mut(home).home_statics += 1;
            if !self.colliders[&entity]
                .handles
                .iter()
                .any(|(r, _)| *r == home)
            {
                self.insert_static_copy(entity, home);
            }
        }
    }

    fn insert_static_copy(&mut self, entity: Entity, region_id: RegionId) {
        let materials = self.materials.clone();
        let Some(c) = self.colliders.get(&entity) else {
            return;
        };
        if c.handles.iter().any(|(r, _)| *r == region_id) {
            return;
        }
        let (friction, restitution) = materials.get(c.desc.surface);
        let region = self.regions[region_id as usize].as_mut().expect("live");
        let pose = Pose::from_parts(region.local(c.world_pose.0), c.world_pose.1);
        let builder = Self::collider_builder(
            entity,
            &c.desc,
            c.shape.clone(),
            pose,
            friction,
            restitution,
        );
        let h = region.world.colliders.insert(builder);
        region.statics.insert(entity);
        region.needs_refresh = true;
        region.fresh.push(h);
        self.colliders
            .get_mut(&entity)
            .unwrap()
            .handles
            .push((region_id, h));
    }

    fn remove_collider_handles(&mut self, entity: Entity) {
        let Some(c) = self.colliders.get_mut(&entity) else {
            return;
        };
        let handles = std::mem::take(&mut c.handles);
        for (rid, h) in handles {
            if let Some(Some(region)) = self.regions.get_mut(rid as usize) {
                region.world.remove_collider(h);
                region.statics.remove(&entity);
                region.needs_refresh = true;
            }
        }
    }

    /// Forget a collider. With `user_removed`, contacts it had end now (the game removed it);
    /// otherwise it is being rebuilt and its contacts are re-checked after the next step.
    pub fn remove_collider(&mut self, entity: Entity, user_removed: bool) {
        if !self.colliders.contains_key(&entity) {
            return;
        }
        self.remove_collider_handles(entity);
        let c = self.colliders.remove(&entity).unwrap();
        if let Some(home) = c.home
            && let Some(Some(r)) = self.regions.get_mut(home as usize)
        {
            r.home_statics = r.home_statics.saturating_sub(1);
        }
        if let Some(body) = c.body {
            self.fix_mass(body);
        }
        self.contacts
            .pending
            .retain(|k, _| k.0 != entity && k.1 != entity);
        let pairs: Vec<(Entity, Entity)> = self
            .contacts
            .touching
            .keys()
            .filter(|k| k.0 == entity || k.1 == entity)
            .copied()
            .collect();
        if user_removed {
            for k in pairs {
                let (a, b, sensor) = self.contacts.touching.remove(&k).unwrap();
                self.contacts.out.push(ended(a, b, sensor));
            }
        } else {
            self.contacts.recheck.extend(pairs);
        }
    }

    /// Update a static collider's pose after gameplay moved it.
    pub fn move_static(&mut self, entity: Entity, pose: (DVec3, Quat)) {
        let Some(c) = self.colliders.get_mut(&entity) else {
            return;
        };
        c.world_pose = pose;
        let handles = c.handles.clone();
        let reach = self.static_reach();
        let mut out_of_reach = false;
        for (rid, h) in handles {
            let region = self.region_mut(rid);
            if region.anchor.distance(pose.0) > reach + 1.0e4 {
                out_of_reach = true;
            }
            let local = Pose::from_parts(region.local(pose.0), pose.1);
            if let Some(col) = region.world.colliders.get_mut(h) {
                col.set_position(local);
            }
            region.needs_refresh = true;
            region.fresh.push(h);
        }
        if out_of_reach {
            self.place_static(entity);
        }
    }

    /// Re-apply friction/restitution after the surface material table changed.
    pub fn refresh_materials(&mut self) {
        let materials = self.materials.clone();
        let updates: Vec<(RegionId, ColliderHandle, f32, f32)> = self
            .colliders
            .values()
            .flat_map(|c| {
                let (f, r) = materials.get(c.desc.surface);
                c.handles.iter().map(move |(rid, h)| (*rid, *h, f, r))
            })
            .collect();
        for (rid, h, f, r) in updates {
            if let Some(col) = self.region_mut(rid).world.colliders.get_mut(h) {
                col.set_friction(f);
                col.set_restitution(r);
            }
        }
    }

    // ------------------------------------------------------------------ joints

    /// Try to create a joint in Rapier. Returns false if a body it needs doesn't exist yet.
    ///
    /// `static_pose` gives the world pose of entities that aren't simulated bodies (static
    /// bodies and plain entities a joint is pinned to).
    pub fn create_joint(
        &mut self,
        entity: Entity,
        static_pose: &dyn Fn(Entity) -> Option<(DVec3, Quat)>,
    ) -> bool {
        let Some(entry) = self.joints.get(&entity) else {
            return true;
        };
        if entry.created.is_some() {
            return true;
        }
        let desc = entry.desc.clone();
        let side_a = if self.bodies.contains_key(&desc.body_a) {
            JointSide::Body(desc.body_a)
        } else if let Some(p) = static_pose(desc.body_a) {
            JointSide::World(p.0 + (p.1 * desc.anchor_a).as_dvec3())
        } else {
            return false;
        };
        let side_b = match desc.body_b {
            None => JointSide::World(desc.anchor_b),
            Some(b) if self.bodies.contains_key(&b) => JointSide::Body(b),
            Some(b) => match static_pose(b) {
                Some(p) => JointSide::World(p.0 + (p.1 * desc.anchor_b.as_vec3()).as_dvec3()),
                None => return false,
            },
        };
        // Both bodies must share a region.
        if let (JointSide::Body(a), JointSide::Body(b)) = (side_a, side_b) {
            let (ra, rb) = (self.bodies[&a].region, self.bodies[&b].region);
            if ra != rb {
                self.merge_regions(ra, rb);
            }
        }
        let region_id = match (side_a, side_b) {
            (JointSide::Body(a), _) => self.bodies[&a].region,
            (_, JointSide::Body(b)) => self.bodies[&b].region,
            (JointSide::World(p), JointSide::World(_)) => self.region_for_mover(p),
        };
        let (h1, anchor1, rot1) = self.joint_side(region_id, side_a, desc.anchor_a);
        let (h2, anchor2, rot2) = self.joint_side(region_id, side_b, desc.anchor_b.as_vec3());
        // The joint axis is given in body A's space; express it in B's space too so the joint
        // starts relaxed.
        let axis = match &desc.kind {
            JointKind::Revolute { axis, .. } | JointKind::Prismatic { axis, .. } => {
                axis.try_normalize().unwrap_or(Vec3::X)
            }
            _ => Vec3::X,
        };
        let frames = convert::JointFrames {
            anchor1,
            anchor2,
            axis1: axis,
            axis2: rot2.inverse() * (rot1 * axis),
        };
        let joint = convert::joint(&desc.kind, frames, desc.collide_connected);
        let region = self.region_mut(region_id);
        let handle = region.world.impulse_joints.insert(h1, h2, joint, true);
        self.joints.get_mut(&entity).unwrap().created = Some((region_id, handle));
        true
    }

    /// Rapier body, local anchor and body rotation for one side of a joint. For a body side,
    /// `anchor` is already in the body's space; for a world side, the anchor is the world point
    /// itself, pinned to the region's ground body.
    fn joint_side(
        &self,
        region_id: RegionId,
        side: JointSide,
        anchor: Vec3,
    ) -> (RigidBodyHandle, Vec3, Quat) {
        let region = self.region(region_id);
        match side {
            JointSide::Body(e) => {
                let b = &self.bodies[&e];
                (b.handle, anchor, *region.world.bodies[b.handle].rotation())
            }
            JointSide::World(p) => (region.ground, region.local(p), Quat::IDENTITY),
        }
    }

    pub fn remove_joint(&mut self, entity: Entity) {
        if let Some(j) = self.joints.remove(&entity)
            && let Some((rid, h)) = j.created
            && let Some(Some(region)) = self.regions.get_mut(rid as usize)
        {
            region.world.impulse_joints.remove(h, true);
        }
    }

    // ------------------------------------------------------------------ characters

    pub fn create_character(&mut self, entity: Entity, desc: &CharacterController, at: DVec3) {
        let half = (desc.height * 0.5 - desc.radius).max(0.0);
        // A changed controller (crouching, sliding) keeps its body, platform and state; only
        // the capsule changes.
        if let Some(c) = self.characters.get_mut(&entity) {
            c.desc = *desc;
            let (rid, collider) = (c.region, c.collider);
            if let Some(col) = self.region_mut(rid).world.colliders.get_mut(collider) {
                col.set_shape(SharedShape::capsule_y(half, desc.radius.max(0.01)));
                col.set_collision_groups(convert::groups(&desc.layers));
            }
            return;
        }
        let region_id = self.region_for_mover(at);
        let region = self.region_mut(region_id);
        let local = region.local(at);
        let body = region.world.bodies.insert(
            RigidBodyBuilder::kinematic_position_based()
                .translation(local)
                .user_data(entity.to_bits() as u128),
        );
        let collider = ColliderBuilder::capsule_y(half, desc.radius.max(0.01))
            // Characters move only by their own sweeps (the controller), but bodies hitting
            // them bounce off as off any kinematic body: crates land on heads, barrels stop.
            .collision_groups(convert::groups(&desc.layers))
            .user_data(convert::pack_user_data(entity, SurfaceId(15), false))
            .density(0.0);
        let world = &mut region.world;
        let collider = world
            .colliders
            .insert_with_parent(collider, body, &mut world.bodies);
        region.fresh.push(collider);
        region.movers += 1;
        self.characters.insert(
            entity,
            CharacterEntry {
                region: region_id,
                body,
                collider,
                desc: *desc,
                written: None,
                platform: None,
                desired: Vec3::ZERO,
                dt: 0.0,
                world_dt: 0.0,
                state: Default::default(),
            },
        );
    }

    pub fn remove_character(&mut self, entity: Entity) {
        if let Some(c) = self.characters.remove(&entity) {
            let region = self.region_mut(c.region);
            region.world.remove_body(c.body);
            region.movers = region.movers.saturating_sub(1);
        }
    }

    // ------------------------------------------------------------------ region upkeep

    /// World position of every mover in a region (bodies and characters).
    fn mover_positions(&self, id: RegionId) -> Vec<DVec3> {
        let region = self.region(id);
        let bodies = self
            .bodies
            .values()
            .filter(|b| b.region == id)
            .filter_map(|b| region.world.bodies.get(b.handle))
            .map(|rb| region.global(rb.translation()));
        let chars = self
            .characters
            .values()
            .filter(|c| c.region == id)
            .filter_map(|c| region.world.bodies.get(c.body))
            .map(|rb| region.global(rb.translation()));
        bodies.chain(chars).collect()
    }

    /// Re-center, split and merge regions so every mover stays near its region's anchor and
    /// movers that could touch share a region. Also freezes regions far from every focus.
    pub fn maintain_regions(&mut self, foci: &[DVec3], max_speed_step: f64) {
        let radius = self.settings.region_radius_m;
        // Freeze far regions.
        let freeze = self.settings.freeze_distance_m;
        for r in self.regions.iter_mut().flatten() {
            r.frozen = match freeze {
                Some(d) if !foci.is_empty() => {
                    foci.iter().all(|f| f.distance(r.anchor) > d + radius)
                }
                _ => false,
            };
        }

        let mut bounds: Vec<(RegionId, DVec3, DVec3)> = Vec::new();
        for id in self.live_region_ids() {
            if self.region(id).movers == 0 || self.region(id).frozen {
                continue;
            }
            let positions = self.mover_positions(id);
            if positions.is_empty() {
                continue;
            }
            let (mut lo, mut hi) = (positions[0], positions[0]);
            for p in &positions {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            let center = (lo + hi) * 0.5;
            // Re-center when the movers drifted away from the anchor.
            if center.distance(self.region(id).anchor) > radius * 0.25 {
                self.rebase(id, center);
            }
            // Movers far outside the radius leave for another region.
            let anchor = self.region(id).anchor;
            let strays: Vec<Entity> = self
                .bodies
                .iter()
                .filter(|(_, b)| b.region == id)
                .filter_map(|(e, b)| {
                    let rb = self.region(id).world.bodies.get(b.handle)?;
                    let p = self.region(id).global(rb.translation());
                    (p.distance(anchor) > radius * 1.25).then_some(*e)
                })
                .collect();
            for e in strays {
                if self.bodies.get(&e).is_some_and(|b| b.region == id) {
                    let p = self.body_world_position(e);
                    let target = match self.nearest_region(p, radius * 0.5) {
                        Some(t) if t != id => t,
                        _ => self.create_region(p),
                    };
                    self.migrate_group(e, target);
                }
            }
            let margin = 20.0 + max_speed_step * 4.0;
            bounds.push((id, lo - DVec3::splat(margin), hi + DVec3::splat(margin)));
        }

        // Merge regions whose movers could touch (if the result still fits).
        for i in 0..bounds.len() {
            for j in (i + 1)..bounds.len() {
                let (a, alo, ahi) = bounds[i];
                let (b, blo, bhi) = bounds[j];
                let overlap = alo.cmple(bhi).all() && blo.cmple(ahi).all();
                let span = (ahi.max(bhi) - alo.min(blo)).max_element();
                let live = |s: &Self, id: RegionId| {
                    s.regions.get(id as usize).is_some_and(|r| r.is_some())
                };
                if overlap && span < radius * 2.0 && live(self, a) && live(self, b) {
                    let (big, small) = if self.region(a).movers >= self.region(b).movers {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    self.merge_regions(big, small);
                }
            }
        }
        self.prune_regions();
    }

    fn body_world_position(&self, e: Entity) -> DVec3 {
        let b = &self.bodies[&e];
        let region = self.region(b.region);
        region
            .world
            .bodies
            .get(b.handle)
            .map(|rb| region.global(rb.translation()))
            .unwrap_or(region.anchor)
    }

    /// Move a region's anchor to `new_anchor`, shifting everything in it.
    fn rebase(&mut self, id: RegionId, new_anchor: DVec3) {
        let delta = {
            let region = self.region_mut(id);
            let delta = (new_anchor - region.anchor).as_vec3();
            region.anchor = new_anchor;
            let handles: Vec<RigidBodyHandle> =
                region.world.bodies.iter().map(|(h, _)| h).collect();
            for h in handles {
                let rb = &mut region.world.bodies[h];
                let mut pose = *rb.position();
                pose.translation -= delta;
                let sleeping = rb.is_sleeping();
                rb.set_position(pose, false);
                if rb.is_kinematic() {
                    rb.set_next_kinematic_position(pose);
                }
                if sleeping {
                    rb.sleep();
                }
            }
            // Attached colliders follow their bodies only when the region steps; move them now
            // so queries before then (characters looking for the ground) find them.
            let world = &mut region.world;
            for (_, col) in world.colliders.iter_mut() {
                if let Some(parent) = col.parent()
                    && let Some(rb) = world.bodies.get(parent)
                    && let Some(rel) = col.position_wrt_parent()
                {
                    col.set_position(*rb.position() * *rel);
                }
            }
            region.needs_refresh = true;
            region.stale_all = true;
            delta
        };
        let _ = delta;
        // Static copies: re-place the ones in this region (some may now be out of reach, new
        // ones in reach).
        let statics: Vec<Entity> = self.region(id).statics.iter().copied().collect();
        for e in statics {
            let Some(c) = self.colliders.get(&e) else {
                continue;
            };
            let pose = c.world_pose;
            let handle = c.handles.iter().find(|(r, _)| *r == id).map(|(_, h)| *h);
            let region = self.region_mut(id);
            let local = region.local(pose.0);
            if let Some(h) = handle
                && let Some(col) = region.world.colliders.get_mut(h)
            {
                col.set_position(Pose::from_parts(local, pose.1));
            }
        }
        let reach = self.static_reach();
        let anchor = self.region(id).anchor;
        let add: Vec<Entity> = self
            .colliders
            .iter()
            .filter(|(_, c)| c.body.is_none())
            .filter(|(_, c)| c.world_pose.0.distance(anchor) <= c.radius + reach)
            .filter(|(_, c)| !c.handles.iter().any(|(r, _)| *r == id))
            .map(|(e, _)| *e)
            .collect();
        for e in add {
            self.insert_static_copy(e, id);
        }
    }

    /// Every body connected to `start` through joints (including itself).
    fn joint_group(&self, start: Entity) -> Vec<Entity> {
        let mut group = vec![start];
        let mut seen = EntityHashSet::default();
        seen.insert(start);
        let mut i = 0;
        while i < group.len() {
            let e = group[i];
            for j in self.joints.values() {
                let other = if j.desc.body_a == e {
                    j.desc.body_b
                } else if j.desc.body_b == Some(e) {
                    Some(j.desc.body_a)
                } else {
                    None
                };
                if let Some(o) = other
                    && self.bodies.contains_key(&o)
                    && seen.insert(o)
                {
                    group.push(o);
                }
            }
            i += 1;
        }
        group
    }

    /// Move `start` and everything jointed to it into region `target`.
    fn migrate_group(&mut self, start: Entity, target: RegionId) {
        let group = self.joint_group(start);
        self.migrate_bodies(&group, target);
    }

    /// Move every mover of `small` into `big`.
    pub fn merge_regions(&mut self, big: RegionId, small: RegionId) {
        if big == small {
            return;
        }
        let bodies: Vec<Entity> = self
            .bodies
            .iter()
            .filter(|(_, b)| b.region == small)
            .map(|(e, _)| *e)
            .collect();
        self.migrate_bodies(&bodies, big);
        let chars: Vec<Entity> = self
            .characters
            .iter()
            .filter(|(_, c)| c.region == small)
            .map(|(e, _)| *e)
            .collect();
        for e in chars {
            self.migrate_character(e, big);
        }
    }

    fn migrate_character(&mut self, e: Entity, target: RegionId) {
        let Some(c) = self.characters.get(&e) else {
            return;
        };
        let (from, body, collider) = (c.region, c.body, c.collider);
        if from == target {
            return;
        }
        let (rb, col, offset) = {
            let src = self.region_mut(from);
            let col = src.world.colliders.remove(
                collider,
                &mut src.world.islands,
                &mut src.world.bodies,
                &mut src.world.soft_bodies,
                false,
            );
            let rb = src.world.remove_body(body);
            src.movers = src.movers.saturating_sub(1);
            (rb, col, src.anchor)
        };
        let (Some(mut rb), Some(col)) = (rb, col) else {
            return;
        };
        let dst = self.region_mut(target);
        let shift = (offset - dst.anchor).as_vec3();
        let mut pose = *rb.position();
        pose.translation += shift;
        rb.set_position(pose, true);
        let nb = dst.world.bodies.insert(rb);
        let world = &mut dst.world;
        let nc = world
            .colliders
            .insert_with_parent(col, nb, &mut world.bodies);
        dst.fresh.push(nc);
        dst.movers += 1;
        let c = self.characters.get_mut(&e).unwrap();
        c.region = target;
        c.body = nb;
        c.collider = nc;
    }

    /// Move bodies (with their colliders and the joints among them) into region `target`.
    fn migrate_bodies(&mut self, entities: &[Entity], target: RegionId) {
        // Joints touching these bodies are recreated in the target region.
        let set: EntityHashSet = entities.iter().copied().collect();
        for j in self.joints.values_mut() {
            if (set.contains(&j.desc.body_a) || j.desc.body_b.is_some_and(|b| set.contains(&b)))
                && let Some((rid, h)) = j.created.take()
                && let Some(Some(r)) = self.regions.get_mut(rid as usize)
            {
                r.world.impulse_joints.remove(h, false);
            }
        }
        for &e in entities {
            let Some(b) = self.bodies.get(&e) else {
                continue;
            };
            let (from, handle) = (b.region, b.handle);
            if from == target {
                continue;
            }
            let collider_entities: Vec<Entity> = self
                .colliders
                .iter()
                .filter(|(_, c)| c.body == Some(e))
                .map(|(ce, _)| *ce)
                .collect();
            // Take the Rapier objects out of the old region.
            let (rb, cols, from_anchor) = {
                let src = self.region_mut(from);
                let mut cols = Vec::new();
                for ce in &collider_entities {
                    // (handled below via the entry handles)
                    let _ = ce;
                }
                let rb_cols: Vec<ColliderHandle> = src
                    .world
                    .bodies
                    .get(handle)
                    .map(|rb| rb.colliders().to_vec())
                    .unwrap_or_default();
                for h in rb_cols {
                    if let Some(col) = src.world.colliders.remove(
                        h,
                        &mut src.world.islands,
                        &mut src.world.bodies,
                        &mut src.world.soft_bodies,
                        false,
                    ) {
                        cols.push((h, col));
                    }
                }
                let rb = src.world.remove_body(handle);
                src.movers = src.movers.saturating_sub(1);
                (rb, cols, src.anchor)
            };
            let Some(mut rb) = rb else {
                continue;
            };
            let dst = self.region_mut(target);
            let shift = (from_anchor - dst.anchor).as_vec3();
            let mut pose = *rb.position();
            pose.translation += shift;
            rb.set_position(pose, false);
            let nb = dst.world.bodies.insert(rb);
            let mut new_handles: HashMap<ColliderHandle, ColliderHandle> = HashMap::new();
            for (old, col) in cols {
                let world = &mut dst.world;
                let nc = world
                    .colliders
                    .insert_with_parent(col, nb, &mut world.bodies);
                dst.fresh.push(nc);
                new_handles.insert(old, nc);
            }
            dst.movers += 1;
            let entry = self.bodies.get_mut(&e).unwrap();
            entry.region = target;
            entry.handle = nb;
            entry.forces_set = true;
            for ce in collider_entities {
                if let Some(c) = self.colliders.get_mut(&ce) {
                    for (rid, h) in c.handles.iter_mut() {
                        *rid = target;
                        if let Some(n) = new_handles.get(h) {
                            *h = *n;
                        }
                    }
                }
                let pairs: Vec<(Entity, Entity)> = self
                    .contacts
                    .touching
                    .keys()
                    .filter(|k| k.0 == ce || k.1 == ce)
                    .copied()
                    .collect();
                self.contacts.recheck.extend(pairs);
                self.contacts.pending.retain(|k, _| k.0 != ce && k.1 != ce);
            }
        }
    }

    // ------------------------------------------------------------------ contacts

    /// Turn Rapier's collision events from the last substep into contract events.
    pub fn process_contacts(&mut self, region_id: RegionId, sub_dt: f32) {
        let mut events: Vec<CollisionEvent> =
            std::mem::take(&mut *self.region(region_id).collector.0.lock());
        // The narrow phase runs on several threads: sort so results don't depend on timing.
        events.sort_by_key(|ev| {
            let (a, b) = (ev.collider1(), ev.collider2());
            (a.into_raw_parts(), b.into_raw_parts())
        });
        for ev in events {
            let (h1, h2, flags, started) = match ev {
                CollisionEvent::Started(a, b, f) => (a, b, f, true),
                CollisionEvent::Stopped(a, b, f) => (a, b, f, false),
            };
            if flags.contains(CollisionEventFlags::REMOVED) {
                // Removals are reported (or re-checked) when we remove the collider.
                continue;
            }
            let region = self.region(region_id);
            let (Some(c1), Some(c2)) = (
                region.world.colliders.get(h1),
                region.world.colliders.get(h2),
            ) else {
                continue;
            };
            let (e1, e2) = (
                convert::entity_of(c1.user_data),
                convert::entity_of(c2.user_data),
            );
            let key = pair_key(e1, e2);
            let sensor = flags.contains(CollisionEventFlags::SENSOR);
            if started {
                if self.contacts.touching.contains_key(&key) {
                    continue;
                }
                if sensor {
                    let point = region.global(c2.translation());
                    self.contacts.touching.insert(key, (e1, e2, true));
                    self.contacts.out.push(ContactEvent {
                        phase: ContactPhase::Started,
                        a: e1,
                        b: e2,
                        point,
                        normal: Vec3::ZERO,
                        impulse: 0.0,
                        sensor: true,
                    });
                } else {
                    self.contacts.pending.insert(key, (region_id, h1, h2));
                }
            } else {
                self.contacts.pending.remove(&key);
                if let Some((a, b, s)) = self.contacts.touching.remove(&key) {
                    self.contacts.out.push(ended(a, b, s));
                }
            }
        }
        self.confirm_pending(region_id, sub_dt);
    }

    /// Promote close pairs that now actually touch (or were pushed apart) to `Started`.
    fn confirm_pending(&mut self, region_id: RegionId, _sub_dt: f32) {
        let keys: Vec<((Entity, Entity), ColliderHandle, ColliderHandle)> = self
            .contacts
            .pending
            .iter()
            .filter(|(_, (r, _, _))| *r == region_id)
            .map(|(k, (_, h1, h2))| (*k, *h1, *h2))
            .collect();
        for (key, h1, h2) in keys {
            let region = self.region(region_id);
            let Some(pair) = region.world.narrow_phase.contact_pair(h1, h2) else {
                self.contacts.pending.remove(&key);
                continue;
            };
            let impulse = pair.total_impulse_magnitude();
            let mut deepest: Option<(f32, Vec3, Vec3)> = None; // (dist, world point, normal)
            for m in pair.manifolds() {
                for c in &m.points {
                    if deepest.is_none_or(|d| c.dist < d.0) {
                        let c1 = &region.world.colliders[pair.collider1];
                        let p = c1.position().transform_point(c.local_p1);
                        deepest = Some((c.dist, p, m.data.normal));
                    }
                }
            }
            let Some((dist, point, normal)) = deepest else {
                continue;
            };
            if impulse <= 1.0e-6 && dist > TOUCH_DISTANCE {
                continue;
            }
            let c1 = &region.world.colliders[pair.collider1];
            let c2 = &region.world.colliders[pair.collider2];
            let (a, b) = (
                convert::entity_of(c1.user_data),
                convert::entity_of(c2.user_data),
            );
            let event = ContactEvent {
                phase: ContactPhase::Started,
                a,
                b,
                point: region.global(point),
                normal,
                impulse,
                sensor: false,
            };
            // Impact info for footsteps/dents/noise: the dynamic side that hit something.
            let impact = self.impact_for(region_id, pair.collider1, pair.collider2, event);
            self.contacts.pending.remove(&key);
            self.contacts.touching.insert(key, (a, b, false));
            self.contacts.out.push(event);
            if let Some(i) = impact {
                self.contacts.impacts.push(i);
            }
        }
    }

    fn impact_for(
        &self,
        region_id: RegionId,
        h1: ColliderHandle,
        h2: ColliderHandle,
        event: ContactEvent,
    ) -> Option<Impact> {
        let world = &self.region(region_id).world;
        let side = |h: ColliderHandle| {
            let c = &world.colliders[h];
            let body = c.parent().and_then(|p| world.bodies.get(p));
            (c, body.filter(|b| b.is_dynamic()))
        };
        let (c1, b1) = side(h1);
        let (c2, b2) = side(h2);
        // Prefer the dynamic body hitting something static.
        let (mover_c, mover_b, other_c, other_dynamic, normal_sign) = match (b1, b2) {
            (Some(b), None) => (c1, b, c2, false, -1.0),
            (None, Some(b)) => (c2, b, c1, false, 1.0),
            (Some(x), Some(y)) if x.mass() <= y.mass() => (c1, x, c2, true, -1.0),
            (Some(_), Some(y)) => (c2, y, c1, true, 1.0),
            _ => return None,
        };
        let mass = mover_b.mass();
        let n = event.normal * normal_sign; // from the other side toward the mover
        let v = mover_b.linvel();
        let tangent = v - n * v.dot(n);
        let ext = mover_c.compute_aabb().half_extents() * 2.0;
        Some(Impact {
            event,
            body: convert::entity_of(mover_b.user_data),
            mass,
            other_static: !other_dynamic,
            other_surface: convert::surface_of(other_c.user_data),
            normal: n,
            tangent_speed: tangent.length(),
            size: [ext.x.min(ext.z).max(0.05), ext.x.max(ext.z).max(0.05)],
        })
    }

    /// After a rebuild or a move between regions, end contacts that no longer exist.
    pub fn recheck_contacts(&mut self) {
        let keys = std::mem::take(&mut self.contacts.recheck);
        for key in keys {
            if !self.contacts.touching.contains_key(&key) {
                continue;
            }
            let still = (|| {
                let c1 = self.colliders.get(&key.0)?;
                let c2 = self.colliders.get(&key.1)?;
                for (r1, h1) in &c1.handles {
                    for (r2, h2) in &c2.handles {
                        if r1 == r2 {
                            let pair = self.region(*r1).world.narrow_phase.contact_pair(*h1, *h2);
                            if pair.is_some_and(|p| p.has_any_active_contact())
                                || self
                                    .region(*r1)
                                    .world
                                    .narrow_phase
                                    .intersection_pair(*h1, *h2)
                                    == Some(true)
                            {
                                return Some(true);
                            }
                        }
                    }
                }
                Some(false)
            })()
            .unwrap_or(false);
            if !still {
                let (a, b, s) = self.contacts.touching.remove(&key).unwrap();
                self.contacts.out.push(ended(a, b, s));
            }
        }
    }
}

/// One end of a joint: a simulated body, or a fixed point in the world.
#[derive(Clone, Copy)]
enum JointSide {
    Body(Entity),
    World(DVec3),
}

fn ended(a: Entity, b: Entity, sensor: bool) -> ContactEvent {
    ContactEvent {
        phase: ContactPhase::Ended,
        a,
        b,
        point: DVec3::ZERO,
        normal: Vec3::ZERO,
        impulse: 0.0,
        sensor,
    }
}

fn locked_axes(lock: [bool; 3]) -> LockedAxes {
    let mut axes = LockedAxes::empty();
    if lock[0] {
        axes |= LockedAxes::ROTATION_LOCKED_X;
    }
    if lock[1] {
        axes |= LockedAxes::ROTATION_LOCKED_Y;
    }
    if lock[2] {
        axes |= LockedAxes::ROTATION_LOCKED_Z;
    }
    axes
}

fn body_params(builder: RigidBodyBuilder, desc: &RigidBody) -> RigidBodyBuilder {
    builder
        .linear_damping(convert::damping(desc.linear_damping))
        .angular_damping(convert::damping(desc.angular_damping))
        .gravity_scale(desc.gravity_scale)
        .ccd_enabled(desc.ccd)
        .can_sleep(desc.can_sleep)
        .locked_axes(locked_axes(desc.lock_rotation))
}
