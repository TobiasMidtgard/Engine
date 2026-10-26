//! Levels of detail for meshes on the instanced path (those too small for virtual geometry:
//! props, wall stones, debris). Each dense mesh gets a chain of simplified index lists over its
//! own vertices, built once on a worker thread, and every view picks the coarsest level whose
//! error stays under a pixel (the camera) or a texel (shadow maps). Objects smaller than that
//! are left out of the view altogether.

use engine_core::math::Vec3;

use crate::mesh::Mesh;
use crate::virtual_geometry::simplify::simplify;

/// Meshes with fewer triangles get no levels (drawing them is cheap anyway).
pub const MIN_TRIANGLES: usize = 512;
/// Most levels below the mesh itself.
pub const MAX_LEVELS: usize = 6;
/// The chain stops at about this many triangles.
const FLOOR_TRIANGLES: usize = 48;
/// Each level aims at this share of the one above.
const STEP: f32 = 0.35;
/// A level that keeps more than this share of the one above isn't worth one.
const MIN_GAIN: f32 = 0.8;

/// One level of a mesh: its triangles as indices into the mesh's vertices, and how far its
/// surface strays from the full mesh (meters in mesh space).
#[derive(Debug, Clone)]
pub struct LodIndices {
    pub indices: Vec<u32>,
    pub error: f32,
}

/// The levels of `mesh`, finest first (empty for a mesh too small or too ragged to simplify).
pub fn build(mesh: &Mesh) -> Vec<LodIndices> {
    let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    if triangles.len() < MIN_TRIANGLES {
        return Vec::new();
    }
    let positions: Vec<Vec3> = mesh.vertices.iter().map(|v| v.position.into()).collect();
    // Split vertices (seams in normals or texture coordinates) stay put: the simplifier locks
    // vertices on edges that aren't shared by exactly two triangles.
    let locked = vec![false; positions.len()];
    let mut levels = Vec::new();
    let mut current = triangles;
    let mut error = 0.0f32;
    while levels.len() < MAX_LEVELS && current.len() > FLOOR_TRIANGLES * 2 {
        let target = ((current.len() as f32 * STEP) as usize).max(FLOOR_TRIANGLES);
        let out = simplify(&positions, &current, &locked, target);
        if out.triangles.is_empty() || out.triangles.len() as f32 > current.len() as f32 * MIN_GAIN
        {
            break;
        }
        // Errors add up along the chain (each level is measured against the one above).
        error += out.error;
        levels.push(LodIndices {
            indices: out.triangles.iter().flatten().copied().collect(),
            error,
        });
        current = out.triangles;
    }
    levels
}

/// The coarsest level (0 = the mesh itself) whose error, scaled by `scale`, stays within
/// `tolerance` meters. `errors` are the levels' errors, finest first.
pub fn pick(errors: impl Iterator<Item = f32>, scale: f32, tolerance: f32) -> u8 {
    let mut level = 0;
    for (i, e) in errors.enumerate() {
        if e * scale > tolerance {
            break;
        }
        level = i as u8 + 1;
    }
    level
}

/// Meters one pixel spans at `distance` from a perspective camera (`fov_y` radians, `height`
/// pixels); `None` for an orthographic view.
pub fn meters_per_pixel(fov_y: f32, height: u32, distance: f32) -> Option<f32> {
    (fov_y > 0.0).then(|| 2.0 * (fov_y * 0.5).tan() * distance.max(1e-3) / height.max(1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rock::rock;

    #[test]
    fn a_dense_stone_gets_a_chain_of_ever_coarser_levels() {
        let mesh = rock(Vec3::new(0.4, 0.25, 0.3), 7, 5);
        let full = mesh.indices.len() / 3;
        let levels = build(&mesh);
        assert!(levels.len() >= 3, "{} levels", levels.len());
        let mut last = full;
        let mut last_error = 0.0;
        for l in &levels {
            let n = l.indices.len() / 3;
            assert!(n < last, "{n} triangles after {last}");
            assert!(l.error >= last_error);
            assert!(
                l.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.vertices.len())
            );
            last = n;
            last_error = l.error;
        }
        let coarsest = levels.last().unwrap();
        assert!(
            coarsest.indices.len() / 3 <= full / 20,
            "{full} down to {}",
            coarsest.indices.len() / 3
        );
        // Still the stone: the coarsest level strays less than a quarter of its size, and the
        // first one less than a percent.
        assert!(coarsest.error < 0.1, "error {} m", coarsest.error);
        assert!(levels[0].error < 0.004, "error {} m", levels[0].error);
    }

    #[test]
    fn levels_are_picked_by_their_error_on_screen() {
        let errors = [0.001, 0.004, 0.02, 0.08];
        assert_eq!(pick(errors.into_iter(), 1.0, 0.0005), 0);
        assert_eq!(pick(errors.into_iter(), 1.0, 0.005), 2);
        assert_eq!(pick(errors.into_iter(), 2.0, 0.005), 1);
        assert_eq!(pick(errors.into_iter(), 1.0, 1.0), 4);
        let near = meters_per_pixel(60f32.to_radians(), 1440, 2.0).unwrap();
        let far = meters_per_pixel(60f32.to_radians(), 1440, 200.0).unwrap();
        assert!((far / near - 100.0).abs() < 1e-3);
        assert!(meters_per_pixel(0.0, 1440, 2.0).is_none());
    }
}
