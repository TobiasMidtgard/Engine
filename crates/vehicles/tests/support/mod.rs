//! A small rigid-body solver for the vehicle tests, written only against the
//! `engine_world::physics` contract, so these tests drive the vehicles exactly as a real
//! backend would: the vehicles write `ExternalForce` and raycast through `PhysicsQueries`,
//! this solver integrates bodies (f64 positions, gravity bodies, full 3D rotation) and
//! resolves contacts between dynamic boxes and static boxes, cylinders and spheres.
//!
//! It is a stand-in until the physics module lands, not a backend: no dynamic–dynamic
//! contacts, corner contacts only, shapecasts are raycasts of the shape's center.

#![allow(dead_code)]

pub mod shots;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use engine_core::math::{DVec3, Mat3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::physics::*;
use engine_world::{Gravity, SurfaceId, SurfaceMaterials, WorldPlugin};

pub const DT: f64 = 1.0 / 60.0;

/// The stand-in solver: add it like any physics backend.
pub struct StandInPhysics {
    pub substeps: u32,
}

impl Default for StandInPhysics {
    fn default() -> Self {
        Self { substeps: 4 }
    }
}

#[derive(Resource)]
struct Settings {
    substeps: u32,
}

/// Impulses received since the last step.
#[derive(Resource, Default)]
struct PendingImpulses(Vec<ApplyImpulse>);

/// Contacts found this step, sent as `ContactEvent`s.
#[derive(Resource, Default)]
struct OutgoingContacts(Vec<ContactEvent>);

/// Pairs (dynamic body, static collider) touching last step.
#[derive(Resource, Default)]
struct Touching(HashSet<(Entity, Entity)>);

impl Plugin for StandInPhysics {
    fn build(&self, app: &mut App) {
        claim_backend(app, "vehicles test stand-in");
        app.insert_resource(Settings {
            substeps: self.substeps.max(1),
        })
        .init_resource::<PendingImpulses>()
        .init_resource::<OutgoingContacts>()
        .init_resource::<Touching>()
        .add_systems(FixedPreUpdate, snapshot_system)
        .add_systems(
            FixedUpdate,
            (collect_impulses, step_system, send_contacts)
                .chain()
                .in_set(SimSet::Physics),
        );
    }
}

/// An app with the world, the stand-in solver and deterministic 60 Hz time.
pub fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(StandInPhysics::default());
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app
}

/// Advance `seconds` of simulated time.
pub fn run_for(app: &mut App, seconds: f64) {
    let frames = (seconds / DT).round() as usize;
    for _ in 0..frames {
        app.update();
    }
}

/// A static box: `half` extents, top face at `top` when unrotated.
pub fn static_box(
    app: &mut App,
    center: DVec3,
    half: Vec3,
    rotation: Quat,
    surface: &str,
) -> Entity {
    let surface = app
        .world
        .resource::<SurfaceMaterials>()
        .id_or_default(surface);
    app.world
        .spawn((
            Transform::from_translation(center).with_rotation(rotation),
            RigidBody::fixed(),
            Collider::cuboid(half).with_surface(surface),
        ))
        .id()
}

/// Flat ground: a big static slab whose top is at `top`.
pub fn ground(app: &mut App, center: DVec3, half_size: f32, surface: &str) -> Entity {
    static_box(
        app,
        center - DVec3::Y * 1.0,
        Vec3::new(half_size, 1.0, half_size),
        Quat::IDENTITY,
        surface,
    )
}

// ---------------------------------------------------------------- shapes

#[derive(Clone, Copy, Debug)]
enum PrimKind {
    Box { half: Vec3 },
    Cylinder { radius: f32, half_height: f32 },
    Sphere { radius: f32 },
}

#[derive(Clone, Copy, Debug)]
struct Prim {
    entity: Entity,
    center: DVec3,
    rotation: Quat,
    kind: PrimKind,
    surface: SurfaceId,
    layers: CollisionLayers,
    sensor: bool,
}

fn expand(shape: &ColliderShape, offset: Vec3, rot: Quat, out: &mut Vec<(Vec3, Quat, PrimKind)>) {
    match shape {
        ColliderShape::Cuboid { half_extents } => out.push((
            offset,
            rot,
            PrimKind::Box {
                half: *half_extents,
            },
        )),
        ColliderShape::Sphere { radius } => {
            out.push((offset, rot, PrimKind::Sphere { radius: *radius }))
        }
        ColliderShape::Cylinder {
            radius,
            half_height,
        } => out.push((
            offset,
            rot,
            PrimKind::Cylinder {
                radius: *radius,
                half_height: *half_height,
            },
        )),
        ColliderShape::Capsule {
            radius,
            half_height,
        } => out.push((
            offset,
            rot,
            PrimKind::Box {
                half: Vec3::new(*radius, half_height + radius, *radius),
            },
        )),
        ColliderShape::Compound(parts) => {
            for (o, r, s) in parts {
                expand(s, offset + rot * *o, rot * *r, out);
            }
        }
        ColliderShape::ConvexHull { points } => {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in points {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            out.push((
                offset + rot * ((lo + hi) * 0.5),
                rot,
                PrimKind::Box {
                    half: (hi - lo) * 0.5,
                },
            ));
        }
        ColliderShape::TriMesh { .. } | ColliderShape::Heightfield { .. } => {}
    }
}

fn volume(kind: &PrimKind) -> f32 {
    match *kind {
        PrimKind::Box { half } => 8.0 * half.x * half.y * half.z,
        PrimKind::Cylinder {
            radius,
            half_height,
        } => std::f32::consts::PI * radius * radius * 2.0 * half_height,
        PrimKind::Sphere { radius } => 4.0 / 3.0 * std::f32::consts::PI * radius.powi(3),
    }
}

/// Inertia tensor of a solid primitive about its own center, in its own axes, per kg.
fn unit_inertia(kind: &PrimKind) -> Vec3 {
    match *kind {
        PrimKind::Box { half } => {
            let s = half * 2.0;
            Vec3::new(
                s.y * s.y + s.z * s.z,
                s.x * s.x + s.z * s.z,
                s.x * s.x + s.y * s.y,
            ) / 12.0
        }
        PrimKind::Cylinder {
            radius,
            half_height,
        } => {
            let h = half_height * 2.0;
            let side = (3.0 * radius * radius + h * h) / 12.0;
            Vec3::new(side, radius * radius * 0.5, side)
        }
        PrimKind::Sphere { radius } => Vec3::splat(0.4 * radius * radius),
    }
}

/// Mass, center of mass and principal (diagonal) inertia from collider volumes, unless the
/// body overrides them.
fn mass_properties(
    prims: &[(Vec3, Quat, PrimKind)],
    body: &RigidBody,
    density: f32,
) -> MassProperties {
    let total_volume: f32 = prims.iter().map(|p| volume(&p.2)).sum::<f32>().max(1e-6);
    let mass = body.mass_kg.unwrap_or(total_volume * density).max(1e-3);
    let com = body.center_of_mass.unwrap_or_else(|| {
        prims
            .iter()
            .map(|(o, _, k)| *o * volume(k))
            .fold(Vec3::ZERO, |a, b| a + b)
            / total_volume
    });
    let mut tensor = Mat3::ZERO;
    for (o, r, k) in prims {
        let m = mass * volume(k) / total_volume;
        let local = Mat3::from_diagonal(unit_inertia(k) * m);
        let rm = Mat3::from_quat(*r);
        let rotated = rm * local * rm.transpose();
        let d = *o - com;
        let parallel = (Mat3::IDENTITY * d.length_squared() - outer(d, d)) * m;
        tensor += rotated + parallel;
    }
    let inertia =
        body.inertia
            .unwrap_or(Vec3::new(tensor.x_axis.x, tensor.y_axis.y, tensor.z_axis.z));
    MassProperties {
        mass_kg: mass,
        center_of_mass: com,
        inertia: inertia.max(Vec3::splat(1e-3)),
    }
}

fn outer(a: Vec3, b: Vec3) -> Mat3 {
    Mat3::from_cols(a * b.x, a * b.y, a * b.z)
}

// ---------------------------------------------------------------- queries

struct Snapshot {
    prims: Vec<Prim>,
}

fn ray_prim(p: &Prim, origin: DVec3, dir: Vec3, max: f32) -> Option<(f32, Vec3)> {
    let inv = p.rotation.inverse();
    let o = inv * (origin - p.center).as_vec3();
    let d = inv * dir;
    let hit = match p.kind {
        PrimKind::Box { half } => {
            let mut t0 = f32::MIN;
            let mut t1 = f32::MAX;
            let mut n0 = Vec3::ZERO;
            for i in 0..3 {
                if d[i].abs() < 1e-9 {
                    if o[i].abs() > half[i] {
                        return None;
                    }
                    continue;
                }
                let a = (-half[i] - o[i]) / d[i];
                let b = (half[i] - o[i]) / d[i];
                let (near, far, sign) = if a < b { (a, b, -1.0) } else { (b, a, 1.0) };
                if near > t0 {
                    t0 = near;
                    n0 = Vec3::ZERO;
                    n0[i] = sign;
                }
                t1 = t1.min(far);
            }
            if t0 > t1 || t1 < 0.0 {
                return None;
            }
            if t0 < 0.0 {
                // Inside: report the start.
                return Some((0.0, -dir));
            }
            (t0, n0)
        }
        PrimKind::Sphere { radius } => {
            let b = o.dot(d);
            let c = o.length_squared() - radius * radius;
            let disc = b * b - c;
            if disc < 0.0 {
                return None;
            }
            let t = -b - disc.sqrt();
            if t < 0.0 {
                return if c < 0.0 { Some((0.0, -d)) } else { None };
            }
            (t, (o + d * t) / radius)
        }
        PrimKind::Cylinder {
            radius,
            half_height,
        } => {
            let mut best: Option<(f32, Vec3)> = None;
            // Side.
            let a = d.x * d.x + d.z * d.z;
            if a > 1e-12 {
                let b = o.x * d.x + o.z * d.z;
                let c = o.x * o.x + o.z * o.z - radius * radius;
                let disc = b * b - a * c;
                if disc >= 0.0 {
                    let t = (-b - disc.sqrt()) / a;
                    let y = o.y + d.y * t;
                    if t >= 0.0 && y.abs() <= half_height {
                        let q = o + d * t;
                        best = Some((t, Vec3::new(q.x, 0.0, q.z) / radius));
                    }
                }
            }
            // Caps.
            if d.y.abs() > 1e-9 {
                for s in [-1.0f32, 1.0] {
                    let t = (s * half_height - o.y) / d.y;
                    let q = o + d * t;
                    if t >= 0.0
                        && q.x * q.x + q.z * q.z <= radius * radius
                        && best.is_none_or(|b| t < b.0)
                    {
                        best = Some((t, Vec3::new(0.0, s, 0.0)));
                    }
                }
            }
            best?
        }
    };
    (hit.0 <= max).then(|| (hit.0, p.rotation * hit.1))
}

impl PhysicsQuery for Snapshot {
    fn raycast(
        &self,
        origin: DVec3,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for p in &self.prims {
            if !filter.accepts(p.entity, &p.layers, p.sensor) {
                continue;
            }
            if let Some((t, n)) = ray_prim(p, origin, dir, max_distance)
                && best.is_none_or(|b| t < b.distance)
            {
                best = Some(Hit {
                    entity: p.entity,
                    point: origin + (dir * t).as_dvec3(),
                    normal: n,
                    distance: t,
                    surface: p.surface,
                });
            }
        }
        best
    }

    fn shapecast(
        &self,
        _shape: &ColliderShape,
        origin: DVec3,
        _rotation: Quat,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        self.raycast(origin, dir, max_distance, filter)
    }

    fn overlap(
        &self,
        _shape: &ColliderShape,
        position: DVec3,
        _rotation: Quat,
        filter: &QueryFilter,
        out: &mut Vec<Entity>,
    ) {
        for p in &self.prims {
            if filter.accepts(p.entity, &p.layers, p.sensor)
                && signed_distance(p, position).is_some_and(|(s, _)| s <= 0.0)
            {
                out.push(p.entity);
            }
        }
    }

    fn closest_point(&self, point: DVec3, max_distance: f32, filter: &QueryFilter) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for p in &self.prims {
            if !filter.accepts(p.entity, &p.layers, p.sensor) {
                continue;
            }
            if let Some((s, n)) = signed_distance(p, point)
                && s <= max_distance
                && best.is_none_or(|b| s < b.distance)
            {
                best = Some(Hit {
                    entity: p.entity,
                    point: point - (n * s).as_dvec3(),
                    normal: n,
                    distance: s,
                    surface: p.surface,
                });
            }
        }
        best
    }
}

/// Signed distance from `point` to the nearest face of `p` and that face's outward normal
/// (world). `None` near edges and corners (outside along more than one axis), where a single
/// face normal would be wrong.
fn signed_distance(p: &Prim, point: DVec3) -> Option<(f32, Vec3)> {
    let q = p.rotation.inverse() * (point - p.center).as_vec3();
    match p.kind {
        PrimKind::Box { half } => {
            let s = q.abs() - half;
            let outside = (s.x > 0.0) as u8 + (s.y > 0.0) as u8 + (s.z > 0.0) as u8;
            if outside > 1 {
                return None;
            }
            let i = if s.x >= s.y && s.x >= s.z {
                0
            } else if s.y >= s.z {
                1
            } else {
                2
            };
            let mut n = Vec3::ZERO;
            n[i] = q[i].signum();
            Some((s[i], p.rotation * n))
        }
        PrimKind::Sphere { radius } => {
            let l = q.length();
            Some((l - radius, p.rotation * (q / l.max(1e-6))))
        }
        PrimKind::Cylinder {
            radius,
            half_height,
        } => {
            let r = (q.x * q.x + q.z * q.z).sqrt();
            let s_axial = q.y.abs() - half_height;
            let s_radial = r - radius;
            if s_axial > 0.0 && s_radial > 0.0 {
                return None;
            }
            if s_axial >= s_radial {
                Some((s_axial, p.rotation * Vec3::new(0.0, q.y.signum(), 0.0)))
            } else {
                Some((
                    s_radial,
                    p.rotation * (Vec3::new(q.x, 0.0, q.z) / r.max(1e-6)),
                ))
            }
        }
    }
}

fn snapshot_system(world: &mut World) {
    let mut prims = Vec::new();
    let mut parts = Vec::new();
    let mut q = world.query::<(Entity, &Transform, &Collider)>();
    for (e, t, c) in q.iter(world) {
        parts.clear();
        expand(&c.shape, Vec3::ZERO, Quat::IDENTITY, &mut parts);
        for (o, r, kind) in &parts {
            prims.push(Prim {
                entity: e,
                center: t.translation + (t.rotation * *o).as_dvec3(),
                rotation: t.rotation * *r,
                kind: *kind,
                surface: c.surface,
                layers: c.layers,
                sensor: c.sensor,
            });
        }
    }
    world.insert_resource(PhysicsQueries(Arc::new(Snapshot { prims })));
}

// ---------------------------------------------------------------- dynamics

fn collect_impulses(mut pending: ResMut<PendingImpulses>, mut reader: MessageReader<ApplyImpulse>) {
    pending.0.extend(reader.read().copied());
}

fn send_contacts(mut out: ResMut<OutgoingContacts>, mut writer: MessageWriter<ContactEvent>) {
    for c in out.0.drain(..) {
        writer.write(c);
    }
}

struct Body {
    entity: Entity,
    com: DVec3,
    rotation: Quat,
    com_local: Vec3,
    v: Vec3,
    w: Vec3,
    mass: f32,
    inertia: Vec3,
    force: Vec3,
    torque: Vec3,
    gravity_scale: f32,
    linear_damping: f32,
    angular_damping: f32,
    /// Contact points to test: box corners in body space.
    points: Vec<Vec3>,
    friction: f32,
    restitution: f32,
    /// Own seconds per world second (`LocalTimeScale`). Velocities are scaled by it for the
    /// step and forces and gravity by its square, as the contract describes.
    time_scale: f32,
    /// The body's own-seconds velocity on entry, kept for a frozen body (scale 0).
    own_velocity: Velocity,
}

impl Body {
    fn inv_inertia_world(&self, x: Vec3) -> Vec3 {
        let local = self.rotation.inverse() * x;
        self.rotation * (local / self.inertia)
    }

    fn point_velocity(&self, r: Vec3) -> Vec3 {
        self.v + self.w.cross(r)
    }

    fn apply(&mut self, impulse: Vec3, r: Vec3) {
        self.v += impulse / self.mass;
        self.w += self.inv_inertia_world(r.cross(impulse));
    }

    fn effective_mass(&self, r: Vec3, n: Vec3) -> f32 {
        let rn = r.cross(n);
        1.0 / (1.0 / self.mass + self.inv_inertia_world(rn).cross(r).dot(n))
    }
}

struct Contact {
    body: usize,
    other: Entity,
    r: Vec3,
    n: Vec3,
    separation: f32,
    mu: f32,
    bounce: f32,
    jn: f32,
    jt: Vec3,
}

fn step_system(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs_f64();
    if dt <= 0.0 {
        return;
    }
    let substeps = world.resource::<Settings>().substeps;
    let gravity = world.resource::<Gravity>().clone();
    let materials = world.resource::<SurfaceMaterials>().clone();
    let impulses = std::mem::take(&mut world.resource_mut::<PendingImpulses>().0);

    // Static colliders.
    let mut statics = Vec::new();
    let mut parts = Vec::new();
    {
        let mut q = world.query::<(Entity, &Transform, &Collider, Option<&RigidBody>)>();
        for (e, t, c, b) in q.iter(world) {
            if b.is_some_and(|b| b.kind == BodyKind::Dynamic) || c.sensor {
                continue;
            }
            parts.clear();
            expand(&c.shape, Vec3::ZERO, Quat::IDENTITY, &mut parts);
            for (o, r, kind) in &parts {
                statics.push(Prim {
                    entity: e,
                    center: t.translation + (t.rotation * *o).as_dvec3(),
                    rotation: t.rotation * *r,
                    kind: *kind,
                    surface: c.surface,
                    layers: c.layers,
                    sensor: c.sensor,
                });
            }
        }
    }

    // Dynamic bodies.
    let mut bodies: Vec<Body> = Vec::new();
    let mut props: Vec<MassProperties> = Vec::new();
    {
        let mut q = world.query::<(
            Entity,
            &Transform,
            &RigidBody,
            &Collider,
            Option<&Velocity>,
            Option<&ExternalForce>,
            Option<&LocalTimeScale>,
        )>();
        for (e, t, b, c, v, f, local) in q.iter(world) {
            if b.kind != BodyKind::Dynamic {
                continue;
            }
            let s = (world.resource::<Time>().delta_for(local) as f64 / dt) as f32;
            parts.clear();
            expand(&c.shape, Vec3::ZERO, Quat::IDENTITY, &mut parts);
            let mp = mass_properties(&parts, b, c.density);
            let com = t.translation + (t.rotation * mp.center_of_mass).as_dvec3();
            let own_velocity = v.copied().unwrap_or_default();
            let v = Velocity {
                linear: own_velocity.linear * s,
                angular: own_velocity.angular * s,
            };
            let (mut force, mut torque) = (Vec3::ZERO, Vec3::ZERO);
            if let Some(f) = f {
                force = f.force;
                torque = f.torque;
                if let Some(at) = f.at {
                    torque += (at - com).as_vec3().cross(f.force);
                }
            }
            let mut points = Vec::new();
            for (o, r, kind) in &parts {
                if let PrimKind::Box { half } = kind {
                    for i in 0..8 {
                        let s = Vec3::new(
                            if i & 1 == 0 { -1.0 } else { 1.0 },
                            if i & 2 == 0 { -1.0 } else { 1.0 },
                            if i & 4 == 0 { -1.0 } else { 1.0 },
                        );
                        points.push(*o + *r * (*half * s) - mp.center_of_mass);
                    }
                }
            }
            let mat = materials.get(c.surface);
            bodies.push(Body {
                entity: e,
                com,
                rotation: t.rotation,
                com_local: mp.center_of_mass,
                v: v.linear,
                w: v.angular,
                mass: mp.mass_kg,
                inertia: mp.inertia,
                force,
                torque,
                gravity_scale: b.gravity_scale,
                linear_damping: b.linear_damping,
                angular_damping: b.angular_damping,
                points,
                friction: mat.friction,
                restitution: mat.restitution,
                time_scale: s,
                own_velocity,
            });
            props.push(mp);
        }
    }
    let index: HashMap<Entity, usize> = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| (b.entity, i))
        .collect();
    for imp in impulses {
        if let Some(&i) = index.get(&imp.entity) {
            let b = &mut bodies[i];
            let r = imp.at.map_or(Vec3::ZERO, |p| (p - b.com).as_vec3());
            b.apply(imp.impulse, r);
            b.w += b.inv_inertia_world(imp.angular);
        }
    }

    let h = (dt / substeps as f64) as f32;
    let mut contact_impulse: HashMap<(Entity, Entity), (f32, DVec3, Vec3)> = HashMap::new();
    let mut contacts: Vec<Contact> = Vec::new();
    for _ in 0..substeps {
        // Forces.
        for b in &mut bodies {
            let s2 = b.time_scale * b.time_scale;
            let g = gravity.at(b.com) * b.gravity_scale;
            b.v += (b.force / b.mass + g) * s2 * h;
            let tau = b.torque * s2;
            b.w += b.inv_inertia_world(tau) * h;
            b.v *= (1.0 - b.linear_damping * b.time_scale * h).max(0.0);
            b.w *= (1.0 - b.angular_damping * b.time_scale * h).max(0.0);
        }
        // Contacts: every corner near a static face.
        contacts.clear();
        for (bi, b) in bodies.iter().enumerate() {
            for &p_local in &b.points {
                let r = b.rotation * p_local;
                let p = b.com + r.as_dvec3();
                let vp = b.point_velocity(r);
                let reach = 0.02 + vp.length() * h * 2.0;
                let mut best: Option<(f32, Vec3, &Prim)> = None;
                for s in &statics {
                    if !s.layers.interacts(&CollisionLayers::default()) {
                        continue;
                    }
                    if let Some((sep, n)) = signed_distance(s, p)
                        && sep < reach
                        && sep > -0.5
                        && best.is_none_or(|bst| sep < bst.0)
                    {
                        best = Some((sep, n, s));
                    }
                }
                if let Some((sep, n, s)) = best {
                    let m = materials.get(s.surface);
                    contacts.push(Contact {
                        body: bi,
                        other: s.entity,
                        r,
                        n,
                        separation: sep,
                        mu: (b.friction * m.friction).sqrt(),
                        bounce: b.restitution.max(m.restitution).min(0.5),
                        jn: 0.0,
                        jt: Vec3::ZERO,
                    });
                }
            }
        }
        // Relative approach speeds before solving, for restitution.
        let approach: Vec<f32> = contacts
            .iter()
            .map(|c| bodies[c.body].point_velocity(c.r).dot(c.n))
            .collect();
        for _ in 0..12 {
            for (ci, c) in contacts.iter_mut().enumerate() {
                let b = &mut bodies[c.body];
                let vp = b.point_velocity(c.r);
                let vn = vp.dot(c.n);
                // Speculative: allowed to close the gap this substep; pushed out if inside.
                let target = if c.separation > 0.0 {
                    -c.separation / h
                } else {
                    let bounce = if approach[ci] < -1.5 {
                        -approach[ci] * c.bounce
                    } else {
                        0.0
                    };
                    bounce.max(0.25 * (-c.separation - 0.005).max(0.0) / h)
                };
                let m = b.effective_mass(c.r, c.n);
                let dj = (target - vn) * m;
                let jn = (c.jn + dj).max(0.0);
                let dj = jn - c.jn;
                c.jn = jn;
                b.apply(c.n * dj, c.r);
                // Friction.
                let vp = b.point_velocity(c.r);
                let vt = vp - c.n * vp.dot(c.n);
                let speed = vt.length();
                if speed > 1e-6 {
                    let t = vt / speed;
                    let mt = b.effective_mass(c.r, t);
                    let want = c.jt - t * speed * mt;
                    let max = c.mu * c.jn;
                    let jt = want.clamp_length_max(max);
                    let dj = jt - c.jt;
                    c.jt = jt;
                    b.apply(dj, c.r);
                }
            }
        }
        for c in &contacts {
            if c.jn > 0.0 {
                let b = &bodies[c.body];
                let e = contact_impulse.entry((b.entity, c.other)).or_insert((
                    0.0,
                    b.com + c.r.as_dvec3(),
                    c.n,
                ));
                e.0 += c.jn;
            }
        }
        // Integrate.
        for b in &mut bodies {
            if b.time_scale <= 1e-6 {
                continue;
            }
            b.com += (b.v * h).as_dvec3();
            let angle = b.w.length() * h;
            if angle > 1e-9 {
                b.rotation =
                    (Quat::from_axis_angle(b.w / b.w.length(), angle) * b.rotation).normalize();
            }
        }
    }

    // Contact events.
    let mut touching = std::mem::take(&mut world.resource_mut::<Touching>().0);
    let mut now = HashSet::new();
    let mut events = Vec::new();
    for (&(a, other), &(impulse, point, n)) in &contact_impulse {
        now.insert((a, other));
        if !touching.contains(&(a, other)) {
            events.push(ContactEvent {
                phase: ContactPhase::Started,
                a: other,
                b: a,
                point,
                normal: n,
                impulse,
                sensor: false,
            });
        }
    }
    for &(a, other) in touching.difference(&now) {
        events.push(ContactEvent {
            phase: ContactPhase::Ended,
            a: other,
            b: a,
            point: DVec3::ZERO,
            normal: Vec3::ZERO,
            impulse: 0.0,
            sensor: false,
        });
    }
    touching = now;
    world.resource_mut::<Touching>().0 = touching;
    world.resource_mut::<OutgoingContacts>().0.extend(events);

    // Write back.
    for (b, mp) in bodies.iter().zip(props) {
        let origin = b.com - (b.rotation * b.com_local).as_dvec3();
        let mut e = world.entity_mut(b.entity);
        if let Some(mut t) = e.get_mut::<Transform>() {
            t.translation = origin;
            t.rotation = b.rotation;
        }
        let velocity = if b.time_scale > 1e-6 {
            Velocity {
                linear: b.v / b.time_scale,
                angular: b.w / b.time_scale,
            }
        } else {
            b.own_velocity
        };
        e.insert((velocity, mp));
    }
}

// ---------------------------------------------------------------- vehicle helpers

/// The physics app plus the vehicles module.
pub fn vehicle_app() -> App {
    let mut app = physics_app();
    app.add_plugins(engine_vehicles::VehiclesPlugin);
    // Tests build every surface from colliders; the flat ground query fallback (y = 0)
    // would otherwise cut through their slopes.
    app.world
        .resource_mut::<engine_vehicles::VehicleSettings>()
        .use_ground_query = false;
    app
}

/// Spawn through `Commands` (as a game would) and apply.
pub fn spawn_with(app: &mut App, f: impl FnOnce(&mut Commands) -> Entity) -> Entity {
    let e = {
        let mut commands = app.world.commands();
        f(&mut commands)
    };
    app.world.flush();
    e
}

pub fn telemetry(app: &App, e: Entity) -> engine_vehicles::VehicleTelemetry {
    *app.world
        .get::<engine_vehicles::VehicleTelemetry>(e)
        .expect("telemetry")
}

pub fn transform(app: &App, e: Entity) -> Transform {
    *app.world.get::<Transform>(e).expect("transform")
}

pub fn velocity(app: &App, e: Entity) -> Velocity {
    app.world.get::<Velocity>(e).copied().unwrap_or_default()
}

pub fn set_velocity(app: &mut App, e: Entity, linear: Vec3) {
    app.world.entity_mut(e).insert(Velocity {
        linear,
        angular: Vec3::ZERO,
    });
}

pub fn controls(app: &mut App, e: Entity, f: impl FnOnce(&mut engine_world::VehicleControls)) {
    let mut c = app
        .world
        .get_mut::<engine_world::VehicleControls>(e)
        .expect("controls");
    f(&mut c);
}

/// Run until `done` returns true or `max_seconds` pass; returns the seconds taken.
pub fn run_until(
    app: &mut App,
    max_seconds: f64,
    mut done: impl FnMut(&mut App) -> bool,
) -> Option<f64> {
    let frames = (max_seconds / DT).round() as usize;
    for i in 0..frames {
        app.update();
        if done(app) {
            return Some((i + 1) as f64 * DT);
        }
    }
    None
}

/// No wind (the default world has a 3 m/s breeze).
pub fn calm(app: &mut App) {
    let mut weather = app.world.resource_mut::<engine_world::WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    let mut w = app.world.resource_mut::<engine_world::WindField>();
    w.prevailing = Vec3::ZERO;
    w.gustiness = 0.0;
}
