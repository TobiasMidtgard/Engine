//! Boulders: an icosphere displaced by 3D noise, cut by a few planes (cleaved faces),
//! flattened underneath, with smooth normals and box-projected UVs (vertices split per
//! projection axis, so the rock texture never stretches). Moss and dirt live in the vertex
//! colors: green on top, darker near the ground.

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use crate::noise::{Rng, fbm3, smoothstep32 as smoothstep};
use crate::texgen::ROCK_TILE_M;

/// An icosphere of unit radius with `subdivisions` levels (positions only, CCW triangles).
fn icosphere(subdivisions: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut next = Vec::with_capacity(f.len() * 4);
        let mut midpoint = |a: u32, b: u32, v: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                v.push(((v[a as usize] + v[b as usize]) * 0.5).normalize());
                v.len() as u32 - 1
            })
        };
        for [a, b, c] in f {
            let ab = midpoint(a, b, &mut v);
            let bc = midpoint(b, c, &mut v);
            let ca = midpoint(c, a, &mut v);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = next;
    }
    (v, f)
}

/// Shape of a boulder.
#[derive(Clone, Copy, Debug)]
pub struct Boulder {
    pub seed: u64,
    /// Size along x, y, z before noise, m.
    pub size: Vec3,
    /// Icosphere subdivisions (3 = 1280 triangles, 4 = 5120).
    pub detail: u32,
    /// Moss on top, 0..1.
    pub moss: f32,
}

/// Build a boulder mesh with its base around y = 0 (the lowest 15% is below ground).
pub fn boulder(b: &Boulder) -> Mesh {
    let mut rng = Rng::new(b.seed);
    let (dirs, faces) = icosphere(b.detail);
    let seed = rng.next_u32();
    // Cleavage planes: (normal, distance); a rock is the noisy sphere minus what's beyond.
    let planes: Vec<(Vec3, f32)> = (0..rng.range(4.0, 8.0) as usize)
        .map(|_| {
            let n = Vec3::new(rng.signed(), rng.signed() * 0.6 + 0.2, rng.signed()).normalize();
            (n, rng.range(0.62, 0.85))
        })
        .collect();
    let mut pos: Vec<Vec3> = dirs
        .iter()
        .map(|&d| {
            let p = d * 1.3;
            let mut r = 1.0
                + 0.22 * fbm3(p.x + 3.1, p.y, p.z, 4, 0.5, seed)
                + 0.05 * fbm3(p.x * 4.0, p.y * 4.0, p.z * 4.0, 3, 0.5, seed + 1);
            for (n, dist) in &planes {
                let c = d.dot(*n);
                if c > 0.05 {
                    r = r.min(
                        dist / c + 0.03 * fbm3(p.x * 6.0, p.y * 6.0, p.z * 6.0, 2, 0.5, seed + 2),
                    );
                }
            }
            let mut q = d * r * b.size * 0.5;
            // Flat, slightly buried base.
            let floor = -b.size.y * 0.32;
            if q.y < floor {
                q.y = floor + (q.y - floor) * 0.15;
            }
            q
        })
        .collect();
    // Seat it: the lowest 15% of its height below y = 0.
    let min_y = pos.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let max_y = pos.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let lift = -min_y - (max_y - min_y) * 0.15;
    for p in &mut pos {
        p.y += lift;
    }
    // Smooth normals.
    let mut normals = vec![Vec3::ZERO; pos.len()];
    for f in &faces {
        let [a, b2, c] = [f[0] as usize, f[1] as usize, f[2] as usize];
        let n = (pos[b2] - pos[a]).cross(pos[c] - pos[a]);
        normals[a] += n;
        normals[b2] += n;
        normals[c] += n;
    }
    for n in &mut normals {
        *n = n.normalize_or(Vec3::Y);
    }
    // Box-projected UVs: one vertex per (position, axis).
    let mut mesh = Mesh::default();
    let mut map: HashMap<(u32, u8), u32> = HashMap::new();
    let height = max_y - min_y;
    for f in &faces {
        let fnrm = (pos[f[1] as usize] - pos[f[0] as usize])
            .cross(pos[f[2] as usize] - pos[f[0] as usize]);
        let a = fnrm.abs();
        let axis: u8 = if a.x >= a.y && a.x >= a.z {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        };
        for &vi in f {
            let id = *map.entry((vi, axis)).or_insert_with(|| {
                let p = pos[vi as usize];
                let n = normals[vi as usize];
                let uv = match axis {
                    0 => Vec2::new(p.z, -p.y),
                    1 => Vec2::new(p.x, p.z),
                    _ => Vec2::new(p.x, -p.y),
                } / ROCK_TILE_M
                    + Vec2::new(b.seed as f32 * 0.37, b.seed as f32 * 0.61);
                // Moss on top, damp darker rock near the ground.
                let moss_noise =
                    fbm3(p.x * 1.5, p.y * 1.5, p.z * 1.5, 3, 0.5, seed + 9) * 0.5 + 0.5;
                let moss = b.moss * smoothstep(0.45, 0.9, n.y) * smoothstep(0.35, 0.6, moss_noise);
                let ground = smoothstep(0.0, height * 0.35, p.y);
                let ao = 0.55 + 0.45 * ground;
                let c = Vec3::ONE.lerp(Vec3::new(0.42, 0.62, 0.22), moss) * ao;
                mesh.vertices
                    .push(Vertex::new(p, n, uv).with_color(Vec4::new(c.x, c.y, c.z, 1.0)));
                mesh.vertices.len() as u32 - 1
            });
            mesh.indices.push(id);
        }
    }
    mesh
}
