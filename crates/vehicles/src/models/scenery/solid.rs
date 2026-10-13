//! The scenery's collision shapes. They are laid out from the same plans as the meshes (the
//! oval, the building dimensions, the placement constants) but need no renderer, so
//! simulation-only apps and tests collide with the same barriers, walls and buildings the
//! game draws: a car that leaves the track ends up in the gravel or against the Armco, not
//! inside the grandstand.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{Collider, ColliderShape, RigidBody};
use engine_world::{Deformation, SurfaceMaterial, SurfaceMaterials};

/// One static shape in zone-local coordinates.
#[derive(Clone, Debug)]
pub struct Solid {
    pub center: Vec3,
    pub rotation: Quat,
    pub shape: ColliderShape,
    /// Surface material by name ([`SurfaceMaterials`]; the scenery's own are added on spawn).
    pub surface: &'static str,
}

impl Solid {
    /// A box of half extents `half` at `center`, turned by `rotation`.
    pub fn cuboid(center: Vec3, half: Vec3, rotation: Quat, surface: &'static str) -> Self {
        Self {
            center,
            rotation,
            shape: ColliderShape::Cuboid { half_extents: half },
            surface,
        }
    }

    /// An upright cylinder standing on `base`.
    pub fn post(base: Vec3, radius: f32, height: f32, surface: &'static str) -> Self {
        Self {
            center: base + Vec3::Y * (height * 0.5),
            rotation: Quat::IDENTITY,
            shape: ColliderShape::Cylinder {
                radius,
                half_height: height * 0.5,
            },
            surface,
        }
    }

    pub fn ball(center: Vec3, radius: f32, surface: &'static str) -> Self {
        Self {
            center,
            rotation: Quat::IDENTITY,
            shape: ColliderShape::Sphere { radius },
            surface,
        }
    }

    /// A square beam `half` thick from `a` to `b` (any direction).
    pub fn beam(a: Vec3, b: Vec3, half: f32, surface: &'static str) -> Self {
        let along = b - a;
        let len = along.length().max(1e-3);
        Self::cuboid(
            (a + b) * 0.5,
            Vec3::new(half, half, len * 0.5),
            Quat::from_rotation_arc(Vec3::Z, along / len),
            surface,
        )
    }

    /// A vertical wall from `a` to `b` (their heights ignored), `thick` thick, between
    /// heights `y0` and `y1`.
    pub fn wall(a: Vec3, b: Vec3, thick: f32, y0: f32, y1: f32, surface: &'static str) -> Self {
        let along = (b - a).with_y(0.0);
        let len = along.length().max(1e-3);
        let mid = (a + b) * 0.5;
        Self::cuboid(
            Vec3::new(mid.x, (y0 + y1) * 0.5, mid.z),
            Vec3::new(thick * 0.5, (y1 - y0) * 0.5, len * 0.5),
            Quat::from_rotation_y(along.x.atan2(along.z)),
            surface,
        )
    }

    /// The convex hull of zone-local `points`.
    pub fn hull(points: &[Vec3], surface: &'static str) -> Self {
        let center = points.iter().copied().sum::<Vec3>() / points.len().max(1) as f32;
        Self {
            center,
            rotation: Quat::IDENTITY,
            shape: ColliderShape::ConvexHull {
                points: points.iter().map(|p| *p - center).collect(),
            },
            surface,
        }
    }

    /// Zone-local corners of the shape's bounding box (exact for boxes and hulls).
    pub fn corners(&self) -> Vec<Vec3> {
        let local: Vec<Vec3> = match &self.shape {
            ColliderShape::Cuboid { half_extents: h } => (0..8)
                .map(|k| {
                    let s = |bit: u32| if k & (1 << bit) == 0 { -1.0 } else { 1.0 };
                    Vec3::new(h.x * s(0), h.y * s(1), h.z * s(2))
                })
                .collect(),
            ColliderShape::ConvexHull { points } => points.clone(),
            ColliderShape::Cylinder {
                radius,
                half_height,
            } => (0..16)
                .map(|k| {
                    let a = (k % 8) as f32 / 8.0 * std::f32::consts::TAU;
                    let r = radius / (std::f32::consts::PI / 8.0).cos();
                    let y = if k < 8 { -half_height } else { *half_height };
                    Vec3::new(a.cos() * r, y, a.sin() * r)
                })
                .collect(),
            ColliderShape::Capsule {
                radius,
                half_height,
            } => [-1.0f32, 1.0]
                .iter()
                .flat_map(|&sy| {
                    [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sz)| {
                        Vec3::new(sx * radius, sy * (half_height + radius), sz * radius)
                    })
                })
                .collect(),
            ColliderShape::Sphere { radius } => [-1.0f32, 1.0]
                .iter()
                .flat_map(|&sy| {
                    [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                        .map(|(sx, sz)| Vec3::new(sx, sy, sz) * *radius)
                })
                .collect(),
            _ => vec![Vec3::ZERO],
        };
        local
            .into_iter()
            .map(|p| self.center + self.rotation * p)
            .collect()
    }
}

/// The scenery's own surfaces, added to [`SurfaceMaterials`] when missing: a gravel trap's
/// deep loose pea gravel (it slows a car hard by burying its tires) and the bolted tire
/// stacks in front of the barriers (soft, they don't throw a car back).
fn own_surface(name: &str) -> Option<SurfaceMaterial> {
    match name {
        "gravel_trap" => Some(SurfaceMaterial {
            name: name.into(),
            friction: 0.55,
            restitution: 0.0,
            density: 1_700.0,
            absorbency: 0.2,
            deformation: Deformation::Granular { depth_m: 0.2 },
            footstep_sound: "footstep.gravel".into(),
            impact_effect: "impact.gravel".into(),
            ..Default::default()
        }),
        "tire_wall" => Some(SurfaceMaterial {
            name: name.into(),
            friction: 0.9,
            restitution: 0.05,
            density: 600.0,
            absorbency: 0.0,
            footstep_sound: "footstep.rubber".into(),
            impact_effect: "impact.rubber".into(),
            ..Default::default()
        }),
        _ => None,
    }
}

/// One shape of a compound: offset, rotation, shape.
type Part = (Vec3, Quat, ColliderShape);

/// Make `solids` of a zone whose local origin sits at `origin`, turned by `rotation`: one
/// fixed body per surface holding them all as a compound (`Scenery solids: <zone>/<surface>`).
pub fn spawn(
    commands: &mut Commands,
    origin: DVec3,
    rotation: Quat,
    zone: &str,
    solids: Vec<Solid>,
) {
    let mut by_surface: Vec<(&'static str, Vec<Part>)> = Vec::new();
    for s in solids {
        let part = (s.center, s.rotation, s.shape);
        match by_surface.iter_mut().find(|(name, _)| *name == s.surface) {
            Some((_, parts)) => parts.push(part),
            None => by_surface.push((s.surface, vec![part])),
        }
    }
    for (surface, parts) in by_surface {
        let id = commands
            .spawn((
                Transform::from_translation(origin).with_rotation(rotation),
                RigidBody::fixed(),
                Name::new(format!("Scenery solids: {zone}/{surface}")),
            ))
            .id();
        commands.queue(move |world: &mut World| {
            let mut table = world.get_resource_or_init::<SurfaceMaterials>();
            let surface = match (table.id(surface), own_surface(surface)) {
                (Some(id), _) => id,
                (None, Some(m)) => table.add(m),
                (None, None) => table.id_or_default(surface),
            };
            if let Ok(mut e) = world.get_entity_mut(id) {
                e.insert(Collider {
                    shape: ColliderShape::Compound(parts),
                    surface,
                    ..Default::default()
                });
            }
        });
    }
}
