//! Quadtree nodes on a cube-sphere (or one flat square) and the mapping from node
//! coordinates to the planet surface.
//!
//! A sphere is split into the six faces of a cube; each face is a quadtree of square
//! patches. Face coordinates `(a, b)` in `[-1, 1]²` map to a direction with the tangent
//! warp `tan(a·π/4)`, which keeps patch areas within ~1.4× of each other across a face.

use engine_core::math::{DQuat, DVec2, DVec3};
use engine_core::prelude::*;

use crate::shape::dir_from_lat_lon;

/// Which surface a terrain covers.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum TerrainDomain {
    /// A whole planet around the entity's position (six-face quadtree).
    #[default]
    Sphere,
    /// A flat square `size_m` across centered on the entity (local +Y up). Its relief comes
    /// from the planet shape around `lat_deg`/`lon_deg`, so pads and climate work the same.
    Flat {
        size_m: f64,
        lat_deg: f64,
        lon_deg: f64,
    },
}

/// One patch of the quadtree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeKey {
    pub face: u8,
    pub level: u8,
    pub x: u32,
    pub y: u32,
}

impl NodeKey {
    pub fn root(face: u8) -> Self {
        Self {
            face,
            level: 0,
            x: 0,
            y: 0,
        }
    }

    /// The four children (x-major: (0,0), (1,0), (0,1), (1,1)).
    pub fn children(&self) -> [NodeKey; 4] {
        let c = |dx: u32, dy: u32| NodeKey {
            face: self.face,
            level: self.level + 1,
            x: self.x * 2 + dx,
            y: self.y * 2 + dy,
        };
        [c(0, 0), c(1, 0), c(0, 1), c(1, 1)]
    }

    pub fn parent(&self) -> Option<NodeKey> {
        (self.level > 0).then(|| NodeKey {
            face: self.face,
            level: self.level - 1,
            x: self.x / 2,
            y: self.y / 2,
        })
    }

    /// Lower corner and edge length of the patch in face coordinates (`[-1, 1]²`).
    pub fn face_rect(&self) -> (DVec2, f64) {
        let size = 2.0 / (1u64 << self.level) as f64;
        (
            DVec2::new(-1.0 + self.x as f64 * size, -1.0 + self.y as f64 * size),
            size,
        )
    }
}

/// Face frames: outward normal, `a` axis, `b` axis (a × b = normal, so triangles wound
/// counter-clockwise in (a, b) face outward).
const FACES: [(DVec3, DVec3, DVec3); 6] = [
    (DVec3::X, DVec3::NEG_Z, DVec3::Y),
    (DVec3::NEG_X, DVec3::Z, DVec3::Y),
    (DVec3::Y, DVec3::X, DVec3::NEG_Z),
    (DVec3::NEG_Y, DVec3::X, DVec3::Z),
    (DVec3::Z, DVec3::X, DVec3::Y),
    (DVec3::NEG_Z, DVec3::NEG_X, DVec3::Y),
];

/// Maps quadtree face coordinates to the surface for one terrain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMap {
    pub domain: TerrainDomain,
    /// Base radius of the shape (sea level), meters.
    pub radius: f64,
    /// For flat terrain: rotation from the flat square's frame onto the shape's sphere.
    flat_rotation: DQuat,
}

impl SurfaceMap {
    pub fn new(domain: TerrainDomain, radius: f64) -> Self {
        let flat_rotation = match domain {
            TerrainDomain::Sphere => DQuat::IDENTITY,
            TerrainDomain::Flat {
                lat_deg, lon_deg, ..
            } => DQuat::from_rotation_arc(DVec3::Y, dir_from_lat_lon(lat_deg, lon_deg)),
        };
        Self {
            domain,
            radius,
            flat_rotation,
        }
    }

    /// Number of quadtree roots.
    pub fn roots(&self) -> u8 {
        match self.domain {
            TerrainDomain::Sphere => 6,
            TerrainDomain::Flat { .. } => 1,
        }
    }

    pub fn is_sphere(&self) -> bool {
        matches!(self.domain, TerrainDomain::Sphere)
    }

    /// Edge length of a root patch along the surface, meters.
    pub fn root_size(&self) -> f64 {
        match self.domain {
            TerrainDomain::Sphere => self.radius * std::f64::consts::FRAC_PI_2,
            TerrainDomain::Flat { size_m, .. } => size_m,
        }
    }

    /// Edge length of a patch at `level` along the surface, meters.
    pub fn node_size(&self, level: u8) -> f64 {
        self.root_size() / (1u64 << level) as f64
    }

    /// For face coordinates: (base point on the surface at height 0 in terrain-local space,
    /// local up, the unit direction to sample the shape with).
    pub fn base(&self, face: u8, a: f64, b: f64) -> (DVec3, DVec3, DVec3) {
        match self.domain {
            TerrainDomain::Sphere => {
                let (n, u, v) = FACES[face as usize % 6];
                let ta = (a * std::f64::consts::FRAC_PI_4).tan();
                let tb = (b * std::f64::consts::FRAC_PI_4).tan();
                let dir = (n + u * ta + v * tb).normalize();
                (dir * self.radius, dir, dir)
            }
            TerrainDomain::Flat { size_m, .. } => {
                let x = a * size_m * 0.5;
                let z = -b * size_m * 0.5;
                let sample_dir = self.flat_rotation * DVec3::new(x, self.radius, z).normalize();
                (DVec3::new(x, 0.0, z), DVec3::Y, sample_dir)
            }
        }
    }

    /// Terrain-local position for a surface point displaced by `height` along its up.
    pub fn position(&self, face: u8, a: f64, b: f64, height: f64) -> DVec3 {
        let (base, up, _) = self.base(face, a, b);
        base + up * height
    }

    /// Inverse mapping for a terrain-local point: (unit sampling direction, local up, the base
    /// point on the surface below it). `None` for flat terrain outside its square.
    pub fn locate(&self, local: DVec3) -> Option<(DVec3, DVec3, DVec3)> {
        match self.domain {
            TerrainDomain::Sphere => {
                let dir = local.try_normalize()?;
                Some((dir, dir, dir * self.radius))
            }
            TerrainDomain::Flat { size_m, .. } => {
                let half = size_m * 0.5;
                if local.x.abs() > half || local.z.abs() > half {
                    return None;
                }
                let sample_dir =
                    self.flat_rotation * DVec3::new(local.x, self.radius, local.z).normalize();
                Some((sample_dir, DVec3::Y, DVec3::new(local.x, 0.0, local.z)))
            }
        }
    }

    /// The node at `level` containing a terrain-local point (sphere: by direction).
    pub fn node_at(&self, local: DVec3, level: u8) -> Option<NodeKey> {
        let (face, a, b) = match self.domain {
            TerrainDomain::Sphere => {
                let d = local.try_normalize()?;
                let (face, (n, u, v)) = FACES
                    .iter()
                    .enumerate()
                    .max_by(|x, y| d.dot(x.1.0).total_cmp(&d.dot(y.1.0)))?;
                let k = d.dot(*n);
                let ta = d.dot(*u) / k;
                let tb = d.dot(*v) / k;
                (
                    face as u8,
                    ta.atan() / std::f64::consts::FRAC_PI_4,
                    tb.atan() / std::f64::consts::FRAC_PI_4,
                )
            }
            TerrainDomain::Flat { size_m, .. } => {
                let a = local.x / (size_m * 0.5);
                let b = -local.z / (size_m * 0.5);
                if a.abs() > 1.0 || b.abs() > 1.0 {
                    return None;
                }
                (0, a, b)
            }
        };
        let n = (1u64 << level) as f64;
        let to_cell = |t: f64| (((t + 1.0) * 0.5 * n).floor() as i64).clamp(0, n as i64 - 1) as u32;
        Some(NodeKey {
            face,
            level,
            x: to_cell(a),
            y: to_cell(b),
        })
    }
}

/// Rotation that puts the point at `lat_deg`/`lon_deg` of a planet on top (+Y), with its
/// north toward −Z and east toward +X. Put it on the planet entity's `Transform` (with the
/// planet center at `(0, -radius, 0)`) and that spot becomes the world origin.
pub fn home_rotation(lat_deg: f64, lon_deg: f64) -> DQuat {
    let up = dir_from_lat_lon(lat_deg, lon_deg);
    let east = DVec3::Y.cross(up).try_normalize().unwrap_or(DVec3::X);
    let north = up.cross(east);
    // Local basis (east, up, north) -> world (X, Y, -Z).
    let local = engine_core::math::DMat3::from_cols(east, up, north);
    let world = engine_core::math::DMat3::from_cols(DVec3::X, DVec3::Y, DVec3::NEG_Z);
    DQuat::from_mat3(&(world * local.transpose())).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_cover_the_sphere_and_round_trip() {
        let map = SurfaceMap::new(TerrainDomain::Sphere, 1000.0);
        for face in 0..6u8 {
            for (a, b) in [(0.0, 0.0), (0.7, -0.3), (-0.99, 0.99)] {
                let p = map.position(face, a, b, 0.0);
                assert!((p.length() - 1000.0).abs() < 1e-9);
                let key = map.node_at(p, 4).unwrap();
                assert_eq!(key.face, face, "{a} {b}");
                let (lo, size) = key.face_rect();
                assert!(a >= lo.x - 1e-9 && a <= lo.x + size + 1e-9);
                assert!(b >= lo.y - 1e-9 && b <= lo.y + size + 1e-9);
            }
        }
    }

    #[test]
    fn faces_wind_outward() {
        let map = SurfaceMap::new(TerrainDomain::Sphere, 1.0);
        for face in 0..6u8 {
            let p0 = map.position(face, 0.0, 0.0, 0.0);
            let pa = map.position(face, 0.01, 0.0, 0.0);
            let pb = map.position(face, 0.0, 0.01, 0.0);
            assert!((pa - p0).cross(pb - p0).dot(p0) > 0.0, "face {face}");
        }
        let flat = SurfaceMap::new(
            TerrainDomain::Flat {
                size_m: 100.0,
                lat_deg: 40.0,
                lon_deg: 0.0,
            },
            1000.0,
        );
        let p0 = flat.position(0, 0.0, 0.0, 0.0);
        let pa = flat.position(0, 0.1, 0.0, 0.0);
        let pb = flat.position(0, 0.0, 0.1, 0.0);
        assert!((pa - p0).cross(pb - p0).y > 0.0);
    }

    #[test]
    fn home_rotation_puts_home_on_top() {
        let q = home_rotation(40.0, 25.0);
        let up = q * dir_from_lat_lon(40.0, 25.0);
        assert!((up - DVec3::Y).length() < 1e-9);
        // North (toward the pole) points to -Z.
        let toward_pole = q * dir_from_lat_lon(41.0, 25.0) - up;
        assert!(toward_pole.z < 0.0 && toward_pole.x.abs() < 1e-9);
    }

    #[test]
    fn children_tile_the_parent() {
        let k = NodeKey {
            face: 2,
            level: 3,
            x: 5,
            y: 1,
        };
        let (lo, size) = k.face_rect();
        for c in k.children() {
            let (clo, csize) = c.face_rect();
            assert!((csize - size * 0.5).abs() < 1e-12);
            assert!(clo.x >= lo.x - 1e-12 && clo.y >= lo.y - 1e-12);
            assert_eq!(c.parent(), Some(k));
        }
    }
}
