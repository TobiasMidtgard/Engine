//! Boats as physics bodies: solid to walk in, bump and beach, floated by the physics module.
//!
//! As Just Cause 3 and Crest do, a boat is a dynamic rigid body whose buoyancy is pressure over
//! its submerged hull (engine_physics integrates that over the collider's triangles, through
//! the `Water` query). Its collider is built from the drawn model: one convex piece for the
//! hull below the floor (what displaces the water and what the floor stands on), thin convex
//! pieces for the sides above it (so the player stands inside, between the gunwales), and the
//! outboard on its own. The pieces ride on the hull's real shape, so the boat floats on its
//! drawn waterline and heels and pitches as the model would.
//!
//! A [`Boat`] carries its collider and mass until a physics backend takes it over: then it gets
//! a dynamic body, its mooring becomes a rope (a distance joint to the world) and the water
//! module's own float ([`Buoyancy`], [`Floating`], [`Tether`]) is dropped, so it isn't lifted
//! twice. Without a physics backend (tools, the water module's own tests) it keeps floating on
//! those, as before.
//!
//! The physics module floats a body by the water's pressure on its collider, and an open boat's
//! collider is open: the air inside the hull, between its sides and below the waterline,
//! displaces water too, but no collider closes it (the crew stands in it). Without it a boat
//! floats as if swamped, its floor under water. [`boat_air_system`] adds that lift, ρ g times
//! the air under the waterline, slice by slice along the hull, at each slice's own center, so
//! it also rights the boat when it heels and trims it when it pitches; it is lost as the boat
//! sinks past its gunwales, as a swamped boat loses it.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{
    Collider, ColliderShape, ExternalForce, Joint, JointKind, PhysicsBackend, RigidBody, Velocity,
};
use engine_world::{Gravity, Water};

use crate::buoyancy::{Buoyancy, Floating, Tether};
use crate::demo::props::Part;

/// A boat that becomes a physics body once a physics backend is installed.
#[derive(Component, Debug, Clone)]
pub struct Boat {
    /// Collision shape in the boat's own space (see [`hull_collider`]).
    pub hull: Collider,
    pub mass_kg: f32,
    /// Center of mass, boat space.
    pub center_of_mass: Vec3,
    /// Principal moments of inertia, kg·m².
    pub inertia: Vec3,
    /// Tied to the world: (point on the boat, boat space; world anchor; rope length m).
    pub mooring: Option<(Vec3, DVec3, f32)>,
    /// The open hull's inside, whose air keeps it afloat ([`boat_air_system`]).
    pub interior: Option<Interior>,
    /// Force (N) and torque (N·m) the boat's crew or engine puts on it, world axes: set it
    /// each step to drive the boat (it goes into its [`ExternalForce`] with the air's lift).
    pub drive: (Vec3, Vec3),
}

/// The inside of an open hull, above its floor (see [`hull_collider`]): slices along it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Interior {
    /// Height of the floor and of the gunwales, boat space.
    pub floor_y: f32,
    pub top_y: f32,
    /// Per slice: middle x, length, half its width inside at the floor and at the gunwales.
    pub slices: Vec<[f32; 4]>,
}

impl Interior {
    /// The air's lift for water standing `depth[side]` meters over the floor at each slice's
    /// port (`0`, +z) and starboard (`1`) halves: (m³ displaced, its center) per half, boat
    /// space.
    fn displaced(&self, slice: usize, side: usize, depth: f32) -> (f32, Vec3) {
        let [x, dx, floor, top] = self.slices[slice];
        let h = (self.top_y - self.floor_y).max(1e-3);
        let d = depth.clamp(0.0, h);
        // Each side's cross-section widens from `floor` to `top` up the sides.
        let area = d * floor + (top - floor) * d * d / (2.0 * h);
        let sign = if side == 0 { 1.0 } else { -1.0 };
        let z = sign * 0.5 * (floor + (top - floor) * d / (2.0 * h));
        (area * dx, Vec3::new(x, self.floor_y + d * 0.5, z))
    }
}

impl Boat {
    /// A boat of `mass_kg` with `hull`; inertia as a hull-shaped solid of the hull's extents,
    /// its center of mass `ballast` meters above the hull's lowest point.
    pub fn new(hull: Collider, mass_kg: f32, ballast: f32) -> Self {
        let (lo, hi) = bounds(&hull.shape).unwrap_or((Vec3::splat(-0.5), Vec3::splat(0.5)));
        let size = hi - lo;
        // A boat's mass sits in its shell, between a solid box (1/12) and a hollow one.
        let k = mass_kg / 9.0;
        let inertia = Vec3::new(
            k * (size.y * size.y + size.z * size.z),
            k * (size.x * size.x + size.z * size.z),
            k * (size.x * size.x + size.y * size.y),
        );
        Self {
            hull,
            mass_kg,
            center_of_mass: Vec3::new((lo.x + hi.x) * 0.5, lo.y + ballast, 0.0),
            inertia,
            mooring: None,
            interior: None,
            drive: (Vec3::ZERO, Vec3::ZERO),
        }
    }

    /// The open hull's inside (from [`hull_collider`]), whose air keeps it afloat. The center
    /// of mass moves over the middle of the floor's plan (about where the hull's buoyancy
    /// acts), as a boat is loaded to float level.
    pub fn with_interior(mut self, interior: Interior) -> Self {
        let (area, moment) = interior
            .slices
            .iter()
            .fold((0.0, 0.0), |(a, m), [x, dx, floor, _]| {
                (a + floor * dx, m + floor * dx * x)
            });
        if area > 1e-4 {
            self.center_of_mass.x = moment / area;
        }
        self.interior = Some(interior);
        self
    }

    /// Tie `at` (boat space) to `anchor` (world) with a rope of `length_m`.
    pub fn moored(mut self, at: Vec3, anchor: DVec3, length_m: f32) -> Self {
        self.mooring = Some((at, anchor, length_m));
        self
    }
}

/// Make boats physics bodies once a backend is installed (see the module docs).
pub fn boat_bodies_system(
    mut commands: Commands,
    backend: Option<Res<PhysicsBackend>>,
    boats: Query<(Entity, &Boat), Without<RigidBody>>,
) {
    if backend.is_none() {
        return;
    }
    for (e, boat) in &boats {
        commands
            .entity(e)
            .insert((
                RigidBody::dynamic().with_mass_properties(
                    boat.mass_kg,
                    boat.center_of_mass,
                    boat.inertia,
                ),
                boat.hull.clone(),
                Velocity::default(),
                ExternalForce::default(),
            ))
            .remove::<(Buoyancy, Floating, Tether)>();
        if let Some((at, anchor, length)) = boat.mooring {
            commands.spawn((
                Name::new("Mooring line"),
                Joint {
                    kind: JointKind::Distance {
                        limits: [0.0, length],
                    },
                    body_a: e,
                    body_b: None,
                    anchor_a: at,
                    anchor_b: anchor,
                    break_force: None,
                    collide_connected: false,
                },
                ChildOf(e),
            ));
        }
    }
}

/// Water density, kg/m³.
const RHO: f32 = 1000.0;

/// Lift from the air inside open hulls, plus the boats' [`Boat::drive`], into their
/// [`ExternalForce`] (see the module docs). Runs before the physics step.
pub fn boat_air_system(
    water: Option<Res<Water>>,
    gravity: Option<Res<Gravity>>,
    mut boats: Query<(&Boat, &Transform, &mut ExternalForce), With<RigidBody>>,
) {
    for (boat, tf, mut push) in &mut boats {
        let (mut force, mut torque) = boat.drive;
        if let (Some(water), Some(inside)) = (&water, &boat.interior) {
            let com = tf.translation + (tf.rotation * boat.center_of_mass).as_dvec3();
            let g = gravity
                .as_ref()
                .map_or(Vec3::new(0.0, -9.81, 0.0), |g| g.at(com));
            let up_world = -g.normalize_or(Vec3::NEG_Y);
            let boat_up = tf.rotation * Vec3::Y;
            for k in 0..inside.slices.len() {
                let [x, _, floor, _] = inside.slices[k];
                for side in 0..2 {
                    let sign = if side == 0 { 1.0 } else { -1.0 };
                    // The floor at the middle of this half, and the water's surface over it.
                    let at = Vec3::new(x, inside.floor_y, sign * floor * 0.5);
                    let p = tf.translation + (tf.rotation * at).as_dvec3();
                    let Some(s) = water.0.sample(p) else {
                        continue;
                    };
                    let over = (s.surface_height - p.y) as f32;
                    if over <= 0.0 {
                        continue;
                    }
                    // Measured up the boat's sides (it heels and pitches).
                    let depth = over / boat_up.dot(up_world).max(0.2);
                    let (volume, center) = inside.displaced(k, side, depth);
                    let lift = up_world * (RHO * g.length() * volume);
                    let r = (tf.translation + (tf.rotation * center).as_dvec3() - com).as_vec3();
                    force += lift;
                    torque += r.cross(lift);
                }
            }
        }
        if push.force != force || push.torque != torque || push.at.is_some() {
            *push = ExternalForce {
                force,
                torque,
                at: None,
            };
        }
    }
}

/// How a boat model is cut into collision pieces.
#[derive(Debug, Clone, Copy)]
pub struct HullCut {
    /// Height of the floor the crew stands on (boat space): the hull below it is one solid.
    pub floor_y: f32,
    /// The transom's x: anything aft of it (an outboard) is a piece of its own.
    pub transom_x: f32,
    /// How thick the sides are taken inward from the outside of the model, meters.
    pub wall: f32,
}

/// A collider for a boat drawn with `parts` (boat space, +x forward): the hull below
/// `cut.floor_y` as one convex piece, each side above it in convex segments along the
/// length, the bow and transom ends, and whatever hangs aft of the transom. Also the open
/// space inside it, between the sides above the floor ([`Interior`]).
pub fn hull_collider(parts: &[Part], cut: HullCut) -> (Collider, Interior) {
    let points: Vec<Vec3> = parts
        .iter()
        .flat_map(|p| p.mesh.vertices.iter().map(|v| Vec3::from(v.position)))
        .collect();
    let (lo, hi) = points.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    );
    let hull_lo = lo.x.max(cut.transom_x);
    // The outside of the model at each station (x bin) on each side, above the floor.
    const BINS: usize = 48;
    let bin = |x: f32| {
        (((x - hull_lo) / (hi.x - hull_lo).max(1e-3) * BINS as f32) as usize).min(BINS - 1)
    };
    let mut outer = [[0.0f32; BINS]; 2];
    // ...and at the floor's own height.
    let mut at_floor = [[0.0f32; BINS]; 2];
    for p in &points {
        if p.x >= hull_lo && p.y > cut.floor_y {
            let side = (p.z < 0.0) as usize;
            let o = &mut outer[side][bin(p.x)];
            *o = o.max(p.z.abs());
        }
        if p.x >= hull_lo && (p.y - cut.floor_y).abs() < 0.03 {
            let side = (p.z < 0.0) as usize;
            let o = &mut at_floor[side][bin(p.x)];
            *o = o.max(p.z.abs());
        }
    }
    let mut below = Vec::new();
    let mut aft = Vec::new();
    // Sides in segments along the length: [side][segment].
    const SEGMENTS: usize = 6;
    let mut sides: [Vec<Vec<Vec3>>; 2] = [vec![Vec::new(); SEGMENTS], vec![Vec::new(); SEGMENTS]];
    let mut ends: [Vec<Vec3>; 2] = [Vec::new(), Vec::new()];
    let end_band = 0.12;
    // The gunwales' height along the hull (a windscreen or an engine isn't a side).
    let mut wall_top = [f32::NAN; BINS];
    for &p in &points {
        if p.x < cut.transom_x - 0.01 {
            aft.push(p);
            continue;
        }
        if p.y <= cut.floor_y {
            below.push(p);
            continue;
        }
        let side = (p.z < 0.0) as usize;
        let b = bin(p.x);
        if p.z.abs() >= outer[side][b] - cut.wall {
            let seg = (b * SEGMENTS / BINS).min(SEGMENTS - 1);
            sides[side][seg].push(p);
            wall_top[b] = wall_top[b].max(p.y);
        }
        if p.x < hull_lo + end_band {
            ends[0].push(p);
        } else if p.x > hi.x - end_band {
            ends[1].push(p);
        }
    }
    // Segments share their boundary stations, so neighbors meet without a gap.
    for side in &mut sides {
        for k in 1..SEGMENTS {
            let (a, b) = side.split_at_mut(k);
            let edge: Vec<Vec3> = b[0]
                .iter()
                .copied()
                .filter(|p| bin(p.x) == k * BINS / SEGMENTS)
                .collect();
            a[k - 1].extend(edge);
        }
    }
    let mut pieces = Vec::new();
    let mut push = |pts: Vec<Vec3>| {
        let pts = thin(pts);
        if pts.len() >= 4 {
            pieces.push((
                Vec3::ZERO,
                Quat::IDENTITY,
                ColliderShape::ConvexHull { points: pts },
            ));
        }
    };
    // The floor's own level closes the solid below it, so it is flat where the crew stands.
    let mut floor = below;
    for (side, stations) in at_floor.iter().enumerate() {
        let sign = if side == 0 { 1.0 } else { -1.0 };
        for (b, &z) in stations.iter().enumerate() {
            if z > 0.0 {
                let x = hull_lo + (b as f32 + 0.5) / BINS as f32 * (hi.x - hull_lo);
                floor.push(Vec3::new(x, cut.floor_y, sign * z));
            }
        }
    }
    let floor: Vec<Vec3> = floor
        .into_iter()
        .map(|p| Vec3::new(p.x, p.y.min(cut.floor_y), p.z))
        .collect();
    push(floor);
    for side in sides {
        for seg in side {
            push(seg);
        }
    }
    for end in ends {
        push(end);
    }
    push(aft);
    // The inside: slices of a few stations each, as wide as the sides leave it.
    const PER_SLICE: usize = 4;
    let station = (hi.x - hull_lo) / BINS as f32;
    let slices = (0..BINS / PER_SLICE)
        .filter_map(|k| {
            let bins = k * PER_SLICE..(k + 1) * PER_SLICE;
            let mean = |table: &[[f32; BINS]; 2]| {
                let v: Vec<f32> = bins
                    .clone()
                    .flat_map(|b| [table[0][b], table[1][b]])
                    .filter(|w| *w > 0.0)
                    .collect();
                (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
            };
            let floor = (mean(&at_floor)? - cut.wall).max(0.0);
            let top = (mean(&outer)? - cut.wall).max(floor);
            let x = hull_lo + (k * PER_SLICE) as f32 * station + 0.5 * PER_SLICE as f32 * station;
            (floor > 0.02).then_some([x, station * PER_SLICE as f32, floor, top])
        })
        .collect();
    (
        Collider {
            shape: ColliderShape::Compound(pieces),
            ..Collider::default()
        },
        Interior {
            floor_y: cut.floor_y,
            top_y: median(&wall_top).unwrap_or(hi.y),
            slices,
        },
    )
}

/// The middle of the numbers in `v` that aren't NaN.
fn median(v: &[f32]) -> Option<f32> {
    let mut v: Vec<f32> = v.iter().copied().filter(|x| !x.is_nan()).collect();
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
}

/// Points rounded to a 2 cm grid, one per cell: hulls of a few hundred points, not thousands.
fn thin(points: Vec<Vec3>) -> Vec<Vec3> {
    let mut seen = std::collections::HashSet::new();
    points
        .into_iter()
        .filter(|p| seen.insert((p * 50.0).round().as_ivec3().to_array()))
        .collect()
}

/// Bounds of a shape's points (hulls and compounds of hulls and boxes).
fn bounds(shape: &ColliderShape) -> Option<(Vec3, Vec3)> {
    match shape {
        ColliderShape::ConvexHull { points } => points.iter().fold(None, |acc, p| {
            Some(acc.map_or((*p, *p), |(lo, hi): (Vec3, Vec3)| (lo.min(*p), hi.max(*p))))
        }),
        ColliderShape::Cuboid { half_extents } => Some((-*half_extents, *half_extents)),
        ColliderShape::Compound(parts) => parts.iter().fold(None, |acc, (at, _, s)| {
            let Some((lo, hi)) = bounds(s) else {
                return acc;
            };
            let (lo, hi) = (lo + *at, hi + *at);
            Some(acc.map_or((lo, hi), |(a, b): (Vec3, Vec3)| (a.min(lo), b.max(hi))))
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bevy_ecs::system::RunSystemOnce;
    use engine_world::{WaterQuery, WaterSample};

    use super::*;

    /// Still water with its surface at y = 0.
    struct Pond;

    impl WaterQuery for Pond {
        fn sample(&self, _p: DVec3) -> Option<WaterSample> {
            Some(WaterSample {
                surface_height: 0.0,
                depth: 10.0,
                velocity: Vec3::ZERO,
                normal: Vec3::Y,
            })
        }
    }

    /// A boat 3 m long, 1.2 m wide inside, its floor at -0.2 and gunwales at 0.4.
    fn boat() -> Boat {
        let slices = (0..6)
            .map(|k| [-1.25 + k as f32 * 0.5, 0.5, 0.5, 0.6])
            .collect();
        Boat::new(Collider::cuboid(Vec3::new(1.5, 0.3, 0.6)), 140.0, 0.1).with_interior(Interior {
            floor_y: -0.2,
            top_y: 0.4,
            slices,
        })
    }

    fn lift(rotation: Quat, y: f64) -> ExternalForce {
        let mut world = World::new();
        world.insert_resource(Water(Arc::new(Pond)));
        let e = world
            .spawn((
                boat(),
                Transform::from_translation(DVec3::new(0.0, y, 0.0)).with_rotation(rotation),
                RigidBody::dynamic(),
                ExternalForce::default(),
            ))
            .id();
        world.run_system_once(boat_air_system).unwrap();
        *world.get::<ExternalForce>(e).unwrap()
    }

    #[test]
    fn the_air_in_an_open_hull_lifts_it_and_rights_it() {
        // Floor 0.2 m under the water: 3 m × 1.1 m (mean width) × 0.2 m of air ≈ 0.66 m³.
        let level = lift(Quat::IDENTITY, 0.0);
        let tonnes = level.force.y / 9.81 / 1000.0;
        assert!((0.55..0.75).contains(&tonnes), "{level:?}");
        assert!(level.torque.length() < 1.0, "{level:?}");
        // Deeper in the water, more lift, up to the gunwales and no more.
        assert!(lift(Quat::IDENTITY, -0.2).force.y > level.force.y * 1.5);
        let swamped = lift(Quat::IDENTITY, -2.0).force.y;
        assert!((swamped - lift(Quat::IDENTITY, -1.0).force.y).abs() < 1.0);
        // Heeled to starboard (-z down), the air on that side pushes it back upright.
        let heeled = lift(Quat::from_rotation_x(-0.15), 0.0);
        assert!(heeled.torque.x > 50.0, "{heeled:?}");
        // Out of the water: nothing.
        assert_eq!(lift(Quat::IDENTITY, 1.0).force, Vec3::ZERO);
    }
}
