//! What the physics sees, drawn over the scene with `engine_debug`: collider wireframes colored
//! by state, contact points with force arrows, joints colored by stress, regions, centers of mass
//! and velocities. Off by default; switch layers on with `PhysicsDebug` (code, data,
//! `resource.set`) or the `physics.debug` API. Draws only when a `DebugPlugin` is present.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_debug::color::{self, Color};
use engine_debug::{DebugDraw, DepthMode};
use engine_render::Camera;
use engine_world::FocusPoints;
use rapier3d::prelude::{
    Collider as RapierCollider, ColliderHandle, Pose, RigidBody as RapierBody, Shape,
};

use crate::SharedSimulation;
use crate::sim::{JointHandle, Simulation};

/// Which physics debug layers to draw. A registered resource: patch it by name, or use
/// `physics.debug {"colliders": true}`.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhysicsDebug {
    /// Collider wireframes: green awake, blue-gray asleep, yellow kinematic, gray static,
    /// magenta sensors, cyan characters.
    pub colliders: bool,
    /// Touching contact points, with an arrow along the normal that grows with the force.
    pub contacts: bool,
    /// Joint anchors joined by a line: orange, turning red as a breakable joint nears its
    /// break force (or when the anchors pull apart).
    pub joints: bool,
    /// Region anchors and their radius (purple, gray when frozen).
    pub regions: bool,
    /// Centers of mass of dynamic bodies, as small axes.
    pub centers_of_mass: bool,
    /// Velocity arrows: where each awake body will be in 0.25 s.
    pub velocities: bool,
    /// Only draw within this distance of the primary focus (or the camera), meters.
    pub radius_m: f64,
    /// Draw over the scene instead of hiding lines behind geometry.
    pub on_top: bool,
}

impl Default for PhysicsDebug {
    fn default() -> Self {
        Self {
            colliders: false,
            contacts: false,
            joints: false,
            regions: false,
            centers_of_mass: false,
            velocities: false,
            radius_m: 150.0,
            on_top: false,
        }
    }
}

impl PhysicsDebug {
    /// Every layer on or off.
    pub fn set_all(&mut self, on: bool) {
        self.colliders = on;
        self.contacts = on;
        self.joints = on;
        self.regions = on;
        self.centers_of_mass = on;
        self.velocities = on;
    }

    pub fn any(&self) -> bool {
        self.colliders
            || self.contacts
            || self.joints
            || self.regions
            || self.centers_of_mass
            || self.velocities
    }
}

const AWAKE: Color = color::LIME;
const ASLEEP: Color = [0.35, 0.45, 0.6, 1.0];
const KINEMATIC: Color = color::YELLOW;
const STATIC: Color = [0.55, 0.55, 0.55, 0.7];
const SENSOR: Color = color::MAGENTA;
const CHARACTER: Color = color::CYAN;
/// Above this many triangles a mesh or heightfield is drawn as its bounding box.
const MAX_MESH_TRIANGLES: usize = 4000;

/// Draw the enabled layers (runs in `PostUpdate`, after the step wrote its results).
pub(crate) fn draw_physics_debug(
    settings: Res<PhysicsDebug>,
    draw: Option<ResMut<DebugDraw>>,
    shared: Res<SharedSimulation>,
    focus: Option<Res<FocusPoints>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    let Some(mut draw) = draw else {
        return;
    };
    if !settings.any() {
        return;
    }
    let center = focus
        .as_deref()
        .and_then(|f| f.points.first().copied())
        .or_else(|| {
            cameras
                .iter()
                .filter(|(c, _)| c.active)
                .max_by_key(|(c, _)| c.order)
                .map(|(_, t)| t.translation())
        })
        .unwrap_or(DVec3::ZERO);
    let sim = shared.0.read();
    let mut pen = Pen {
        draw: &mut draw,
        depth: if settings.on_top {
            DepthMode::OnTop
        } else {
            DepthMode::Tested
        },
        center,
        radius: settings.radius_m.max(0.0),
    };
    pen.everything(&sim, &settings);
}

struct Pen<'a> {
    draw: &'a mut DebugDraw,
    depth: DepthMode,
    center: DVec3,
    radius: f64,
}

impl Pen<'_> {
    fn near(&self, p: DVec3, extent: f64) -> bool {
        p.distance(self.center) <= self.radius + extent
    }

    fn everything(&mut self, sim: &Simulation, s: &PhysicsDebug) {
        let sub_dt = sim.last_sub_dt.max(1e-6);
        for (rid, region) in sim.live_regions() {
            let to_world = |v: Vec3| region.global(v);
            let world = &region.world;
            if s.regions {
                let c = if region.frozen {
                    color::GRAY
                } else {
                    color::PURPLE
                };
                let r = sim.settings.region_radius_m as f32;
                if self.near(region.anchor, r as f64) {
                    self.draw
                        .cross(region.anchor, 2.0, c)
                        .depth(DepthMode::OnTop);
                    self.draw
                        .circle(region.anchor, Vec3::Y, r, c)
                        .depth(self.depth);
                }
            }
            if s.colliders {
                let characters: Vec<ColliderHandle> = sim
                    .characters
                    .values()
                    .filter(|c| c.region == rid)
                    .map(|c| c.collider)
                    .collect();
                for (h, col) in world.colliders.iter() {
                    let body = col.parent().and_then(|h| world.bodies.get(h));
                    let pos = to_world(col.translation());
                    let extent = col.compute_aabb().half_extents().length() as f64;
                    if !self.near(pos, extent) {
                        continue;
                    }
                    let c = collider_color(col, body, characters.contains(&h));
                    self.shape(col.shape(), pos, col.rotation(), c);
                }
            }
            if s.contacts {
                for pair in world.narrow_phase.contact_pairs() {
                    if !pair.has_any_active_contact() {
                        continue;
                    }
                    let Some(c1) = world.colliders.get(pair.collider1) else {
                        continue;
                    };
                    // Impulses live in the manifolds the solver saw (merged clusters on
                    // terrain, meshes and compounds).
                    for m in pair.solver_manifolds() {
                        for p in &m.points {
                            if p.data.impulse <= 0.0 {
                                continue;
                            }
                            let at = to_world(c1.position().transform_point(p.local_p1));
                            if !self.near(at, 0.0) {
                                continue;
                            }
                            let force = p.data.impulse / sub_dt;
                            // 1 N ≈ 5 cm, 1 kN ≈ 30 cm, 1 MN ≈ 60 cm.
                            let len = (0.05 + 0.083 * force.max(0.0).log10().max(0.0)).min(1.0);
                            let c = color::ramp((force.max(1.0).log10() / 5.0).clamp(0.0, 1.0));
                            self.draw.point(at, c).depth(self.depth);
                            self.draw
                                .arrow(at, at + (m.data.normal * len).as_dvec3(), c)
                                .depth(self.depth);
                        }
                    }
                }
            }
            if s.centers_of_mass || s.velocities {
                for (h, rb) in world.bodies.iter() {
                    if h == region.ground || !rb.is_dynamic() {
                        continue;
                    }
                    let com = to_world(rb.center_of_mass());
                    if !self.near(com, 0.0) {
                        continue;
                    }
                    if s.centers_of_mass {
                        self.draw
                            .axes_at(com, *rb.rotation(), 0.15)
                            .depth(DepthMode::OnTop);
                    }
                    if s.velocities && !rb.is_sleeping() && rb.linvel().length() > 0.05 {
                        let to = com + (rb.linvel() * 0.25).as_dvec3();
                        self.draw.arrow(com, to, color::WHITE).depth(self.depth);
                    }
                }
            }
        }
        if s.joints {
            self.joints(sim, sub_dt);
        }
    }

    fn joints(&mut self, sim: &Simulation, sub_dt: f32) {
        let iterations = sim.settings.solver_iterations.clamp(1, 64) as f32;
        for entry in sim.joints.values() {
            let Some((rid, h)) = entry.created else {
                continue;
            };
            let region = sim.region(rid);
            let world = &region.world;
            // Both bodies, both attachment frames, and the constraint impulse if it has one.
            let (h1, f1, h2, f2, impulses) = match h {
                JointHandle::Impulse(h) => {
                    let Some(joint) = world.impulse_joints.get(h) else {
                        continue;
                    };
                    (
                        joint.body1(),
                        joint.data.local_frame1,
                        joint.body2(),
                        joint.data.local_frame2,
                        Some(joint.impulses),
                    )
                }
                JointHandle::Articulated { link, .. } => {
                    let Some((mb, id)) = world.multibody_joints.get(link) else {
                        continue;
                    };
                    let child = mb.link(id).unwrap();
                    let Some(parent) = child.parent_id().and_then(|p| mb.link(p)) else {
                        continue;
                    };
                    (
                        parent.rigid_body_handle(),
                        child.joint.data.local_frame1,
                        child.rigid_body_handle(),
                        child.joint.data.local_frame2,
                        None,
                    )
                }
            };
            let (Some(b1), Some(b2)) = (world.bodies.get(h1), world.bodies.get(h2)) else {
                continue;
            };
            let a = region.global(*b1.position() * f1.translation);
            let b = region.global(*b2.position() * f2.translation);
            if !self.near(a, 0.0) {
                continue;
            }
            let force = impulses.map_or(0.0, |i| {
                Vec3::new(i[0], i[1], i[2]).length() * iterations / sub_dt
            });
            let stress = match entry.desc.break_force {
                Some(limit) if limit > 0.0 => (force / limit).clamp(0.0, 1.0),
                _ => 0.0,
            };
            let gap = (a.distance(b) / 0.02).clamp(0.0, 1.0) as f32;
            let c = color::mix(color::ORANGE, color::RED, stress.max(gap));
            self.draw.cross(a, 0.08, c).depth(DepthMode::OnTop);
            self.draw.cross(b, 0.08, c).depth(DepthMode::OnTop);
            self.draw.line(a, b, c).depth(DepthMode::OnTop);
            if h1 == region.ground || h2 == region.ground {
                // Pinned to the world: mark the world side.
                self.draw
                    .circle(b, Vec3::Y, 0.12, c)
                    .depth(DepthMode::OnTop);
            }
        }
    }

    fn shape(&mut self, shape: &dyn Shape, pos: DVec3, rot: Quat, c: Color) {
        let place = |v: Vec3| pos + (rot * v).as_dvec3();
        let depth = self.depth;
        if let Some(b) = shape.as_cuboid() {
            self.draw
                .oriented_box(pos, b.half_extents, rot, c)
                .depth(depth);
        } else if let Some(b) = shape.as_round_cuboid() {
            let half = b.inner_shape.half_extents + Vec3::splat(b.border_radius);
            self.draw.oriented_box(pos, half, rot, c).depth(depth);
        } else if let Some(s) = shape.as_ball() {
            self.draw.sphere(pos, s.radius, c).depth(depth);
        } else if let Some(cap) = shape.as_capsule() {
            let (a, b) = (place(cap.segment.a), place(cap.segment.b));
            self.draw.capsule(a, b, cap.radius, c).depth(depth);
        } else if let Some(cy) = shape.as_cylinder() {
            let h = Vec3::Y * cy.half_height;
            self.draw
                .cylinder(place(-h), place(h), cy.radius, c)
                .depth(depth);
        } else if let Some(cy) = shape.as_round_cylinder() {
            let s = &cy.inner_shape;
            let h = Vec3::Y * (s.half_height + cy.border_radius);
            self.draw
                .cylinder(place(-h), place(h), s.radius + cy.border_radius, c)
                .depth(depth);
        } else if let Some(cone) = shape.as_cone() {
            let (base, tip) = (
                place(-Vec3::Y * cone.half_height),
                place(Vec3::Y * cone.half_height),
            );
            self.draw
                .circle(base, rot * Vec3::Y, cone.radius, c)
                .depth(depth);
            for k in 0..4 {
                let a = std::f32::consts::FRAC_PI_2 * k as f32;
                let rim =
                    Vec3::new(a.cos(), 0.0, a.sin()) * cone.radius - Vec3::Y * cone.half_height;
                self.draw.line(place(rim), tip, c).depth(depth);
            }
        } else if let Some(hull) = shape.as_convex_polyhedron() {
            let points = hull.points();
            for e in hull.edges() {
                let [i, j] = e.vertices;
                self.draw
                    .line(place(points[i as usize]), place(points[j as usize]), c)
                    .depth(depth);
            }
        } else if let Some(mesh) = shape
            .as_trimesh()
            .filter(|m| m.indices().len() <= MAX_MESH_TRIANGLES)
        {
            let v = mesh.vertices();
            for t in mesh.indices() {
                let [a, b, cc] = t.map(|i| place(v[i as usize]));
                self.draw.polygon([a, b, cc], c).depth(depth);
            }
        } else if let Some(compound) = shape.as_compound() {
            for (local, part) in compound.shapes() {
                let p = pos + (rot * local.translation).as_dvec3();
                self.shape(part.as_ref(), p, rot * local.rotation, c);
            }
        } else {
            // Heightfields, big meshes and anything else: their bounding box.
            let aabb = shape.compute_aabb(&Pose::from_parts(Vec3::ZERO, rot));
            self.draw
                .aabb(pos + aabb.mins.as_dvec3(), pos + aabb.maxs.as_dvec3(), c)
                .depth(depth);
        }
    }
}

fn collider_color(col: &RapierCollider, body: Option<&RapierBody>, character: bool) -> Color {
    if col.is_sensor() {
        return SENSOR;
    }
    if character {
        return CHARACTER;
    }
    match body {
        None => STATIC,
        Some(b) if b.is_fixed() => STATIC,
        Some(b) if b.is_kinematic() => KINEMATIC,
        Some(b) if b.is_sleeping() => ASLEEP,
        Some(_) => AWAKE,
    }
}
