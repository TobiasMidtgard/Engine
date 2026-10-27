//! Buoyancy from each body's real shape.
//!
//! A collider is a closed triangle mesh (built once per shape and cached). Every step the
//! water's surface is sampled on a small grid over the collider, and the water's pressure
//! (ρ g × depth) is integrated over the mesh's wetted triangles, clipped at the waterline. The
//! sum is Archimedes' lift acting at the true center of buoyancy: a log floats lying down, a
//! plank rights itself, a ball floats as deep as a sphere does, a cube of half-density wood
//! rolls onto an edge as a real one does, and a hull pitches over a swell shorter than itself.
//!
//! Drag stays the module's per-body model (`step::prepare_forces`), scaled by the fraction of
//! the volume under water. Shapes without a mesh here (triangle meshes, heightfields, rounded
//! shapes) keep the older estimate from the fraction of their bounding box under water.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::{DVec3, Mat3, Vec3};
use engine_world::Water;
use rapier3d::math::Pose;
use rapier3d::parry::shape::Shape;
use rapier3d::prelude::SharedShape;

/// Water density, kg/m³.
pub(crate) const WATER_DENSITY: f32 = 1000.0;

/// Subdivisions for curved shapes: 16 around, 8 from pole to pole (256 triangles a ball).
const AROUND: u32 = 16;
const ALONG: u32 = 8;

/// A closed, outward-facing triangle mesh of a collider, in the collider's own frame.
pub(crate) struct Hull {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    /// Enclosed volume, m³ (the shape's own volume: curved meshes are scaled to it).
    pub volume: f32,
}

impl Hull {
    /// The mesh of `shape`, or `None` for shapes floated by their bounding box.
    pub fn of(shape: &dyn Shape) -> Option<Hull> {
        let (verts, tris, curved) = mesh(shape)?;
        let mut hull = Hull {
            verts,
            tris,
            volume: 0.0,
        };
        let (volume, centroid) = hull.volume_and_centroid();
        if volume.abs() < 1e-9 {
            return None;
        }
        if volume < 0.0 {
            for t in &mut hull.tris {
                t.swap(1, 2);
            }
        }
        hull.volume = volume.abs();
        // A tessellated ball or cylinder is a little smaller than the real one; scale it up
        // so it displaces exactly the shape's volume when under water.
        let real = shape.mass_properties(1.0).mass();
        if curved && real > 0.0 {
            let k = (real / hull.volume).cbrt();
            for v in &mut hull.verts {
                *v = centroid + (*v - centroid) * k;
            }
            hull.volume = real;
        }
        Some(hull)
    }

    /// Signed volume and centroid (tetrahedra from the origin).
    fn volume_and_centroid(&self) -> (f32, Vec3) {
        let (mut vol, mut moment) = (0.0, Vec3::ZERO);
        for t in &self.tris {
            let [a, b, c] = t.map(|i| self.verts[i as usize]);
            let v = a.dot(b.cross(c)) / 6.0;
            vol += v;
            moment += (a + b + c) * (v / 4.0);
        }
        let centroid = if vol.abs() > 1e-12 {
            moment / vol
        } else {
            Vec3::ZERO
        };
        (vol, centroid)
    }

    /// The water's push on this mesh placed at `pose` in `surface`, under gravity `g` (m/s²),
    /// for a body moving at `vel` (relative to the water) and turning at `spin` about `com`:
    /// the force, its torque about `com`, and the volume of water displaced.
    pub fn immerse(
        &self,
        pose: &Pose,
        surface: &Surface,
        g: f32,
        com: Vec3,
        vel: Vec3,
        spin: Vec3,
    ) -> Wetted {
        let rho_g = WATER_DENSITY * g;
        // Vertices relative to the center of mass, and their depth under the surface.
        let mut placed = Vec::with_capacity(self.verts.len());
        for v in &self.verts {
            let p = pose.transform_point(*v);
            placed.push((p - com, surface.height(p.x, p.z) - p.y));
        }
        let mut out = Wetted::default();
        // Wetted bottom as seen from below: its area, first and second moments of r × up.
        let (mut base, mut base_arm, mut base_inertia) = (0.0, Vec3::ZERO, Mat3::ZERO);
        let mut add = |q: [Vec3; 3], d: [f32; 3]| {
            // Pressure grows linearly with depth over the triangle: ∫d dA = A Σd/3, and
            // ∫d x dA = A/12 (Σ dᵢxᵢ + Σd Σx). `area_n` is the outward normal times the area.
            let area_n = (q[1] - q[0]).cross(q[2] - q[0]) * 0.5;
            let s = d[0] + d[1] + d[2];
            out.force += -area_n * (rho_g * s / 3.0);
            // Torque about the center of mass: ∫ x × dF = (∫d x dA) × (−ρ g n).
            let first = q[0] * d[0] + q[1] * d[1] + q[2] * d[2] + (q[0] + q[1] + q[2]) * s;
            out.torque += (first / 12.0).cross(-area_n * rho_g);
            if area_n.y < 0.0 {
                // r × up is linear in r, so over the triangle ∫ (r × up) dA = A c × up and
                // ∫ (r × up)(r × up)ᵀ dA = A/12 (Σ aᵢaᵢᵀ + Σa Σaᵀ) with aᵢ = qᵢ × up.
                let a = -area_n.y;
                let arms = q.map(|p| p.cross(Vec3::Y));
                let sum = arms[0] + arms[1] + arms[2];
                let outer = |v: Vec3| Mat3::from_cols(v * v.x, v * v.y, v * v.z);
                base += a;
                base_arm += sum * (a / 3.0);
                base_inertia +=
                    (outer(arms[0]) + outer(arms[1]) + outer(arms[2]) + outer(sum)) * (a / 12.0);
            }
        };
        for t in &self.tris {
            let [(a, da), (b, db), (c, dc)] = t.map(|i| placed[i as usize]);
            clip_under([a, b, c], [da, db, dc], &mut add);
        }
        out.volume = (out.force.y / rho_g.max(1e-9)).max(0.0);
        if base > 1e-6 && out.volume > 0.0 {
            // Bobbing, rolling and pitching send waves away, which damps them within a few
            // periods: each patch of the bottom resists moving up or down with
            // ρ √(g D) per m² (D the mean draft), half the critical damping of heave.
            let draft = out.volume / base;
            let b = WATER_DENSITY * (g * draft).sqrt();
            // A patch at r moves up at vel.y + spin · (r × up).
            let up_speed = vel.y * base + spin.dot(base_arm);
            out.force.y -= b * up_speed;
            out.torque -= (base_arm * vel.y + base_inertia * spin) * b;
        }
        out
    }
}

/// What the water does to a hull this step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Wetted {
    pub force: Vec3,
    pub torque: Vec3,
    /// Displaced water, m³.
    pub volume: f32,
}

/// Call `under` with the part of triangle `q` (depths `d`, positive under water) that is under
/// the water, as one or two triangles in the same winding.
fn clip_under(q: [Vec3; 3], d: [f32; 3], under: &mut impl FnMut([Vec3; 3], [f32; 3])) {
    let wet = d.map(|x| x > 0.0);
    match wet.iter().filter(|w| **w).count() {
        0 => {}
        3 => under(q, d),
        n => {
            // Rotate so the odd one out comes first, keeping the winding.
            let odd = (0..3).find(|&i| wet[i] == (n == 1)).unwrap();
            let (i, j, k) = (odd, (odd + 1) % 3, (odd + 2) % 3);
            let cut = |a: usize, b: usize| q[a] + (q[b] - q[a]) * (d[a] / (d[a] - d[b]));
            let (pij, pik) = (cut(i, j), cut(i, k));
            if n == 1 {
                // Only `i` is under: the tip of the triangle.
                under([q[i], pij, pik], [d[i], 0.0, 0.0]);
            } else {
                // Only `i` is dry: the quad j, k, pik, pij under water, as two triangles.
                under([pij, q[j], q[k]], [0.0, d[j], d[k]]);
                under([pij, q[k], pik], [0.0, d[k], 0.0]);
            }
        }
    }
}

/// Mesh of a shape in its own frame, and whether it approximates a curved surface.
fn mesh(shape: &dyn Shape) -> Option<(Vec<Vec3>, Vec<[u32; 3]>, bool)> {
    if let Some(s) = shape.as_cuboid() {
        let (v, t) = s.to_trimesh();
        Some((v, t, false))
    } else if let Some(s) = shape.as_ball() {
        let (v, t) = s.to_trimesh(AROUND, ALONG);
        Some((v, t, true))
    } else if let Some(s) = shape.as_capsule() {
        let (v, t) = s.to_trimesh(AROUND, ALONG);
        Some((v, t, true))
    } else if let Some(s) = shape.as_cylinder() {
        let (v, t) = s.to_trimesh(AROUND * 2);
        Some((v, t, true))
    } else if let Some(s) = shape.as_cone() {
        let (v, t) = s.to_trimesh(AROUND * 2);
        Some((v, t, true))
    } else if let Some(s) = shape.as_convex_polyhedron() {
        let (v, t) = s.to_trimesh();
        Some((v, t, false))
    } else if let Some(s) = shape.as_compound() {
        // Each part closed on its own (and already scaled to its volume); overlaps count
        // twice, as they do for mass.
        let (mut verts, mut tris) = (Vec::new(), Vec::new());
        for (pose, part) in s.shapes() {
            let hull = Hull::of(&**part)?;
            let base = verts.len() as u32;
            verts.extend(hull.verts.iter().map(|v| pose.transform_point(*v)));
            tris.extend(hull.tris.iter().map(|t| t.map(|i| i + base)));
        }
        Some((verts, tris, false))
    } else {
        None
    }
}

/// Hulls by shape, built on first use. Holds a clone of each shape so its address can't be
/// reused while cached; shapes nothing else uses any more are dropped now and then.
#[derive(Default)]
pub(crate) struct HullCache {
    hulls: HashMap<usize, (SharedShape, Option<Arc<Hull>>)>,
    prune_at: usize,
}

impl HullCache {
    pub fn get(&mut self, shape: &SharedShape) -> Option<Arc<Hull>> {
        let key = Arc::as_ptr(&shape.0) as *const () as usize;
        if self.hulls.len() >= self.prune_at.max(64) {
            self.hulls.retain(|_, (s, _)| Arc::strong_count(&s.0) > 1);
            self.prune_at = self.hulls.len() * 2;
        }
        self.hulls
            .entry(key)
            .or_insert_with(|| (shape.clone(), Hull::of(&*shape.0).map(Arc::new)))
            .1
            .clone()
    }
}

/// The water surface over a patch: heights on a grid (region-local y) and the mean current.
pub(crate) struct Surface {
    x0: f32,
    z0: f32,
    step_x: f32,
    step_z: f32,
    nodes: usize,
    heights: Vec<f32>,
    /// Highest point of the patch, region-local.
    pub top: f32,
    pub current: Vec3,
}

/// Grid spacing over a collider: one sample every 2 m, at most a 5 × 5 grid.
const SAMPLE_SPACING: f32 = 2.0;
pub(crate) const MAX_CELLS: usize = 4;
/// The grid's cells across past the full level of detail ([`crate::lod`]): its corners only,
/// a plane through the water there. Each sample asks the water module for its waves and
/// ripples, and a pixel there spans 15 cm or more.
pub(crate) const FAR_CELLS: usize = 1;

impl Surface {
    /// The water over the box `mins`..`maxs` (region-local, region at `anchor`), sampled on a
    /// grid of at most `max_cells` across, or `None` where there is no water under it.
    pub fn sample(
        water: &Water,
        anchor: DVec3,
        mins: Vec3,
        maxs: Vec3,
        max_cells: usize,
    ) -> Option<Surface> {
        let span = (maxs.x - mins.x).max(maxs.z - mins.z);
        let cells = ((span / SAMPLE_SPACING).ceil() as usize).clamp(1, max_cells.max(1));
        let nodes = cells + 1;
        let (step_x, step_z) = (
            (maxs.x - mins.x) / cells as f32,
            (maxs.z - mins.z) / cells as f32,
        );
        let y = 0.5 * (mins.y + maxs.y);
        let center =
            anchor + Vec3::new(0.5 * (mins.x + maxs.x), y, 0.5 * (mins.z + maxs.z)).as_dvec3();
        let middle = water.sample(center)?;
        let fallback = (middle.surface_height - anchor.y) as f32;
        let mut heights = Vec::with_capacity(nodes * nodes);
        let mut current = Vec3::ZERO;
        let mut found = 0.0;
        for iz in 0..nodes {
            for ix in 0..nodes {
                let p = Vec3::new(mins.x + step_x * ix as f32, y, mins.z + step_z * iz as f32);
                let h = match water.sample(anchor + p.as_dvec3()) {
                    Some(s) => {
                        current += s.velocity;
                        found += 1.0;
                        (s.surface_height - anchor.y) as f32
                    }
                    // Past the water's edge: as high as the water in the middle.
                    None => fallback,
                };
                heights.push(h);
            }
        }
        let top = heights.iter().copied().fold(f32::MIN, f32::max);
        Some(Surface {
            x0: mins.x,
            z0: mins.z,
            step_x,
            step_z,
            nodes,
            heights,
            top,
            current: if found > 0.0 {
                current / found
            } else {
                middle.velocity
            },
        })
    }

    /// Surface height at `x`, `z` (region-local), bilinear between samples.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let last = (self.nodes - 1) as f32;
        let fx = ((x - self.x0) / self.step_x.max(1e-6)).clamp(0.0, last);
        let fz = ((z - self.z0) / self.step_z.max(1e-6)).clamp(0.0, last);
        let (ix, iz) = (
            (fx as usize).min(self.nodes - 2),
            (fz as usize).min(self.nodes - 2),
        );
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let at = |i: usize, j: usize| self.heights[j * self.nodes + i];
        let near = at(ix, iz) + (at(ix + 1, iz) - at(ix, iz)) * tx;
        let far = at(ix, iz + 1) + (at(ix + 1, iz + 1) - at(ix, iz + 1)) * tx;
        near + (far - near) * tz
    }

    /// A flat surface at height `h` (tests).
    #[cfg(test)]
    pub fn flat(h: f32) -> Surface {
        Surface {
            x0: -1e6,
            z0: -1e6,
            step_x: 2e6,
            step_z: 2e6,
            nodes: 2,
            heights: vec![h; 4],
            top: h,
            current: Vec3::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapier3d::parry::shape::{Ball, Capsule, Cuboid};

    const G: f32 = 9.81;
    const RHO_G: f32 = WATER_DENSITY * G;

    #[test]
    fn hull_volumes_match_the_shapes() {
        let shapes: Vec<Box<dyn Shape>> = vec![
            Box::new(Cuboid::new(Vec3::new(0.5, 0.2, 1.0))),
            Box::new(Ball::new(0.4)),
            Box::new(Capsule::new_y(0.6, 0.25)),
        ];
        for s in shapes {
            let hull = Hull::of(&*s).unwrap();
            let real = s.mass_properties(1.0).mass();
            assert!(
                (hull.volume - real).abs() < 1e-4 * real.max(1.0),
                "{} vs {real}",
                hull.volume
            );
            let (v, _) = hull.volume_and_centroid();
            assert!(
                (v - real).abs() < 0.002 * real,
                "outward mesh volume {v} vs {real}"
            );
        }
    }

    /// Fully under water, any shape gets exactly ρ g V, straight up, through its centroid.
    #[test]
    fn submerged_hulls_get_archimedes_lift() {
        let hull = Hull::of(&Cuboid::new(Vec3::new(0.5, 0.25, 1.0))).unwrap();
        let pose = Pose::from_parts(
            Vec3::new(1.0, -3.0, 2.0),
            engine_core::math::Quat::from_rotation_z(0.4)
                * engine_core::math::Quat::from_rotation_x(0.3),
        );
        let w = hull.immerse(
            &pose,
            &Surface::flat(0.0),
            G,
            pose.translation,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!((w.force - Vec3::Y * RHO_G).length() < 1.0, "{w:?}");
        assert!(w.torque.length() < 0.5, "{w:?}");
        assert!((w.volume - 1.0).abs() < 1e-3);
    }

    /// Half under water, a box tipped about a long axis is pushed back level.
    #[test]
    fn tipped_boxes_are_righted() {
        let hull = Hull::of(&Cuboid::new(Vec3::new(1.0, 0.1, 0.3))).unwrap();
        let pose = Pose::from_parts(Vec3::ZERO, engine_core::math::Quat::from_rotation_x(0.3));
        let w = hull.immerse(
            &pose,
            &Surface::flat(0.0),
            G,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        // Rolled +x: the righting torque turns it back (−x).
        assert!(w.torque.x < -10.0, "{w:?}");
        // Its center on the waterline: half its 0.24 m³ under.
        assert!((w.volume - 0.12).abs() < 0.005, "{w:?}");
    }

    /// A ball sunk to its center displaces half its volume; a quarter of the way up from the
    /// bottom, the volume of a spherical cap.
    #[test]
    fn balls_displace_spherical_caps() {
        let r = 0.5;
        let hull = Hull::of(&Ball::new(r)).unwrap();
        let full = 4.0 / 3.0 * std::f32::consts::PI * r * r * r;
        for (h, want) in [
            (r, 0.5 * full),
            (
                0.5 * r,
                std::f32::consts::PI * (0.5 * r).powi(2) * (3.0 * r - 0.5 * r) / 3.0,
            ),
        ] {
            let pose = Pose::from_translation(Vec3::new(0.0, r - h, 0.0));
            let w = hull.immerse(
                &pose,
                &Surface::flat(0.0),
                G,
                pose.translation,
                Vec3::ZERO,
                Vec3::ZERO,
            );
            assert!(
                (w.volume - want).abs() < 0.02 * full,
                "draft {h}: {} vs {want}",
                w.volume
            );
        }
    }
}
