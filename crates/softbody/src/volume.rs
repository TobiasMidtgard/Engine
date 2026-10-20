//! Soft volumes: jelly, slimes, creature bellies, cushions, soft tires.
//!
//! A [`SoftVolume`] is a coarse lattice of particles held together by springs and pulled back
//! toward its rest shape by shape matching, which gives jelly its wobble and its spring back.
//! A finer, smooth render mesh is embedded in the lattice and follows it. Plasticity lets
//! dents stay (clay, dough, crumpled metal). The volume rests on and pushes physics colliders
//! both ways: it stacks on crates, and a crate swinging into it knocks it over.

use std::collections::HashMap;

use engine_core::prelude::*;

use crate::body::{Embedding, SoftBodySim, SoftKind, Surface};
use crate::settings::{SimSettings, SoftMaterial};
use crate::solver::{ShapeMatch, Sim};

/// The rest shape of a [`SoftVolume`], centered on the entity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum VolumeShape {
    /// Box of `size` (meters) with corners rounded by `bevel`.
    Box { size: [f32; 3], bevel: f32 },
    /// Ball of `radius` meters.
    Sphere { radius: f32 },
}

impl Default for VolumeShape {
    fn default() -> Self {
        VolumeShape::Box {
            size: [1.0; 3],
            bevel: 0.08,
        }
    }
}

/// A soft, squishy solid. Add it to an entity with a `Transform`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftVolume {
    pub shape: VolumeShape,
    /// Lattice particles along the longest side (3–8). More = finer wobble, more cost.
    pub resolution: u32,
    pub mass_kg: f32,
    /// How hard it springs back to its shape: compliance of the pull, m/N
    /// (jelly 1e-3, rubber 1e-5, 0 = rigid).
    pub shape_compliance: f32,
    /// Compliance of the springs between neighboring particles, m/N (local firmness).
    pub lattice_compliance: f32,
    /// Rate at which deformation past `yield_m` becomes permanent, 1/s (0 = always springs
    /// back; dough 2, clay 10).
    pub plasticity: f32,
    pub yield_m: f32,
    /// Render mesh subdivisions per face.
    pub detail: u32,
    pub sim: SimSettings,
    pub material: SoftMaterial,
}

impl Default for SoftVolume {
    fn default() -> Self {
        Self {
            shape: VolumeShape::default(),
            resolution: 5,
            mass_kg: 20.0,
            shape_compliance: 5e-4,
            lattice_compliance: 2e-4,
            plasticity: 0.0,
            yield_m: 0.05,
            detail: 12,
            sim: SimSettings {
                substeps: 12,
                damping: 0.6,
                friction: 0.7,
                thickness: 0.04,
                wind_scale: 0.0,
                // Mostly water itself: it hangs in water and never soaks.
                buoyancy: 0.95,
                water_drag: 1.5,
                soak: 0.0,
                ..SimSettings::default()
            },
            material: SoftMaterial::hex(0x5fd38d)
                .with_roughness(0.12)
                .with_subsurface(0.85)
                .with_sheen(0.0)
                .with_wrinkles(0.0)
                .with_alpha(0.82),
        }
    }
}

impl SoftVolume {
    /// A jelly cube of edge `size` meters.
    pub fn jelly(size: f32, material: SoftMaterial) -> Self {
        Self {
            shape: VolumeShape::Box {
                size: [size; 3],
                bevel: size * 0.12,
            },
            mass_kg: 1000.0 * size * size * size * 0.9,
            material,
            ..Self::default()
        }
    }
}

/// Build the simulation for `spec` at `transform`.
pub fn build(spec: &SoftVolume, transform: &GlobalTransform) -> Result<SoftBodySim, String> {
    let origin = transform.translation();
    let rot = transform.rotation();
    let scale = transform.scale();
    let r = spec.sim.thickness.max(0.005);
    let res = spec.resolution.clamp(2, 12) as f32;
    let (half, sphere) = match spec.shape {
        VolumeShape::Box { size, .. } => (Vec3::from(size) * 0.5 * scale, None),
        VolumeShape::Sphere { radius } => {
            let rr = radius * scale.max_element();
            (Vec3::splat(rr), Some(rr))
        }
    };
    let inner = (half - Vec3::splat(r)).max(Vec3::splat(0.01));
    let longest = inner.max_element() * 2.0;
    let spacing = longest / (res - 1.0);
    let dims = ((inner * 2.0 / spacing).round().as_uvec3() + UVec3::ONE).max(UVec3::splat(2));
    let step = inner * 2.0 / (dims - UVec3::ONE).as_vec3();

    let mut sim = Sim::new(origin);
    // Lattice in rest space (centered); `slot` maps grid cells to particles.
    let mut slot = vec![u32::MAX; (dims.x * dims.y * dims.z) as usize];
    let gid = |x: u32, y: u32, z: u32| (x + dims.x * (y + dims.y * z)) as usize;
    let mut rest = Vec::new();
    for z in 0..dims.z {
        for y in 0..dims.y {
            for x in 0..dims.x {
                let p = -inner + step * UVec3::new(x, y, z).as_vec3();
                if let Some(sr) = sphere
                    && p.length() > sr - r + step.x * 0.3
                {
                    continue;
                }
                slot[gid(x, y, z)] = rest.len() as u32;
                rest.push(p);
            }
        }
    }
    if rest.len() < 4 {
        return Err("Soft volume is too small for its resolution.".into());
    }
    let mass = spec.mass_kg.max(0.01) / rest.len() as f32;
    for p in &rest {
        sim.add_particle(rot * *p, mass);
    }
    // Springs to the 13 forward neighbors (edges, face and body diagonals).
    for z in 0..dims.z {
        for y in 0..dims.y {
            for x in 0..dims.x {
                let a = slot[gid(x, y, z)];
                if a == u32::MAX {
                    continue;
                }
                for (dx, dy, dz) in NEIGHBORS {
                    let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    if nx < 0 || ny < 0 || nz < 0 {
                        continue;
                    }
                    let (nx, ny, nz) = (nx as u32, ny as u32, nz as u32);
                    if nx >= dims.x || ny >= dims.y || nz >= dims.z {
                        continue;
                    }
                    let b = slot[gid(nx, ny, nz)];
                    if b != u32::MAX {
                        sim.add_distance(a, b, spec.lattice_compliance, u32::MAX);
                    }
                }
            }
        }
    }
    let com = rest.iter().copied().sum::<Vec3>() / rest.len() as f32;
    sim.shapes.push(ShapeMatch {
        particles: (0..rest.len() as u32).collect(),
        rest: rest.iter().map(|p| *p - com).collect(),
        compliance: spec.shape_compliance,
        plasticity: spec.plasticity,
        yield_m: spec.yield_m,
        rotation: rot,
    });

    // Render mesh in rest space, embedded in the lattice.
    let detail = spec.detail.clamp(2, 48);
    let (verts, triangles) = match spec.shape {
        VolumeShape::Box { bevel, .. } => rounded_box(half, bevel.min(half.min_element()), detail),
        VolumeShape::Sphere { .. } => {
            let (v, t) = rounded_box(half, half.min_element(), detail);
            (v, t)
        }
    };
    let full_grid = sphere.is_none();
    let mut embed = Embedding::default();
    for v in &verts {
        let (ids, w) = if full_grid {
            trilinear(*v, inner, step, dims, &slot)
        } else {
            nearest(*v, &rest)
        };
        let base: Vec3 = ids
            .iter()
            .zip(&w)
            .map(|(i, w)| rest[*i as usize] * *w)
            .sum();
        embed.offsets.push(*v - base);
        embed.particles.push(ids);
        embed.weights.push(w);
    }
    let uv = verts
        .iter()
        .map(|v| {
            let n = *v / half;
            Vec2::new(n.x * 0.5 + 0.5 + n.z * 0.25, 0.5 - n.y * 0.5)
        })
        .collect();
    sim.params.radius = r;
    let mut body = SoftBodySim::new(SoftKind::Volume, sim, spec.sim, spec.material);
    body.surface = Some(Surface {
        triangles,
        uv,
        embed: Some(embed),
        rest_inv: Vec::new(),
        uv_scale: Vec2::ONE,
    });
    Ok(body)
}

const NEIGHBORS: [(i32, i32, i32); 13] = [
    (1, 0, 0),
    (0, 1, 0),
    (0, 0, 1),
    (1, 1, 0),
    (1, -1, 0),
    (1, 0, 1),
    (1, 0, -1),
    (0, 1, 1),
    (0, 1, -1),
    (1, 1, 1),
    (1, 1, -1),
    (1, -1, 1),
    (1, -1, -1),
];

/// Trilinear weights of the lattice cell containing `p` (clamped into the lattice).
fn trilinear(p: Vec3, inner: Vec3, step: Vec3, dims: UVec3, slot: &[u32]) -> ([u32; 8], [f32; 8]) {
    let g = ((p + inner) / step).clamp(Vec3::ZERO, (dims - UVec3::ONE).as_vec3());
    let c = g.floor().as_uvec3().min(dims - UVec3::splat(2));
    let f = (g - c.as_vec3()).clamp(Vec3::ZERO, Vec3::ONE);
    let mut ids = [0u32; 8];
    let mut w = [0f32; 8];
    for k in 0..8u32 {
        let (dx, dy, dz) = (k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let cell = c + UVec3::new(dx, dy, dz);
        ids[k as usize] = slot[(cell.x + dims.x * (cell.y + dims.y * cell.z)) as usize];
        let wx = if dx == 1 { f.x } else { 1.0 - f.x };
        let wy = if dy == 1 { f.y } else { 1.0 - f.y };
        let wz = if dz == 1 { f.z } else { 1.0 - f.z };
        w[k as usize] = wx * wy * wz;
    }
    (ids, w)
}

/// Inverse-distance weights of the 8 nearest particles.
fn nearest(p: Vec3, rest: &[Vec3]) -> ([u32; 8], [f32; 8]) {
    let mut best: Vec<(f32, u32)> = rest
        .iter()
        .enumerate()
        .map(|(i, q)| (q.distance_squared(p), i as u32))
        .collect();
    best.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut ids = [0u32; 8];
    let mut w = [0f32; 8];
    let mut sum = 0.0;
    for k in 0..8 {
        let (d2, i) = best[k.min(best.len() - 1)];
        ids[k] = i;
        w[k] = 1.0 / (d2 + 1e-4);
        sum += w[k];
    }
    for x in &mut w {
        *x /= sum;
    }
    (ids, w)
}

/// A rounded box mesh with welded vertices (no seams at the edges). `bevel` = half extent
/// on every axis makes a sphere.
pub fn rounded_box(half: Vec3, bevel: f32, detail: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let core = (half - Vec3::splat(bevel)).max(Vec3::ZERO);
    let mut verts: Vec<Vec3> = Vec::new();
    let mut index: HashMap<[i32; 3], u32> = HashMap::new();
    let mut tris = Vec::new();
    let n = detail as i32;
    // Faces as (normal axis, u axis, v axis) with right-handed orientation.
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    for (normal, u_axis, v_axis) in faces {
        let mut ids = vec![0u32; ((n + 1) * (n + 1)) as usize];
        for j in 0..=n {
            for i in 0..=n {
                // Integer lattice key so shared edge vertices weld exactly.
                let key_v =
                    normal * n as f32 + u_axis * (2 * i - n) as f32 + v_axis * (2 * j - n) as f32;
                let key = [key_v.x as i32, key_v.y as i32, key_v.z as i32];
                let id = *index.entry(key).or_insert_with(|| {
                    let cube = key_v / n as f32; // in [-1, 1]
                    // Spread vertices evenly over the rounded surface.
                    let p = cube * half;
                    let clamped = p.clamp(-core, core);
                    let dir = (p - clamped).normalize_or(normal);
                    verts.push(clamped + dir * bevel);
                    (verts.len() - 1) as u32
                });
                ids[(j * (n + 1) + i) as usize] = id;
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = ids[(j * (n + 1) + i) as usize];
                let b = ids[(j * (n + 1) + i + 1) as usize];
                let c = ids[((j + 1) * (n + 1) + i) as usize];
                let d = ids[((j + 1) * (n + 1) + i + 1) as usize];
                tris.push([a, b, d]);
                tris.push([a, d, c]);
            }
        }
    }
    (verts, tris)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_box_is_closed_and_outward() {
        let (v, t) = rounded_box(Vec3::new(0.5, 0.3, 0.4), 0.1, 6);
        // Welded: V - E + F = 2 for a closed genus-0 surface.
        let mut edges = std::collections::HashSet::new();
        for tri in &t {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                edges.insert((a.min(b), a.max(b)));
            }
        }
        assert_eq!(v.len() as i64 - edges.len() as i64 + t.len() as i64, 2);
        for tri in &t {
            let [a, b, c] = tri.map(|i| v[i as usize]);
            let n = (b - a).cross(c - a);
            assert!(n.dot((a + b + c) / 3.0) > 0.0, "inward triangle");
        }
    }

    #[test]
    fn jelly_rests_on_the_ground_and_keeps_its_volume() {
        let spec = SoftVolume::jelly(0.8, SoftMaterial::default());
        let mut body = build(
            &spec,
            &GlobalTransform::from(Transform::from_xyz(0.0, 1.0, 0.0)),
        )
        .unwrap();
        body.sim.ground = vec![[0.0, 1.0, 0.0, 1.0]]; // y = -1 in body space = world 0
        // Dropped from 36 cm, it wobbles for a few seconds before it comes to rest.
        for _ in 0..480 {
            body.sim.step(1.0 / 60.0, &[]);
        }
        let (lo, hi) = body.sim.bounds();
        let height = hi.y - lo.y + 2.0 * body.sim.params.radius;
        assert!(lo.y > -1.0, "sank: {lo}");
        assert!((height - 0.8).abs() < 0.12, "height {height}");
        assert!(
            body.sim.kinetic_energy() < 0.5,
            "{}",
            body.sim.kinetic_energy()
        );
    }
}
