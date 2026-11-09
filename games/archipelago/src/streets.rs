//! Laid streets: in town every road cell is a cobbled street with kerbstones, out in the
//! country an earth lane with wheel ruts. The surfaces follow the ground and are rebuilt in
//! 16 × 16 cell chunks, only where roads changed.

use std::collections::{HashMap, HashSet};

use engine::prelude::*;
use engine::render::{Mesh, Vertex};

use crate::art::{Mat, hexc};
use crate::defs::{CELL_M, Kind};
use crate::draw::Art;
use crate::grid::{self, Grid};
use crate::island;
use crate::sim::Building;

const CHUNK: i32 = 16;
/// Half the cobbled carriageway's width (a 4 m cell leaves a little verge).
const HALF: f32 = 1.75;
const KERB_W: f32 = 0.28;
const KERB_H: f32 = 0.14;
/// How far surfaces sit above the analytic ground (the terrain mesh is a little coarser).
const LIFT: f32 = 0.07;

/// Street meshes by chunk, and the chunks waiting to be rebuilt.
#[derive(Resource, Default)]
pub struct Streets {
    parts: HashMap<IVec2, Vec<Entity>>,
    dirty: HashSet<IVec2>,
    /// Rebuild everything (after the start is laid out).
    all: bool,
}

/// A road cell changed: its chunk (and a neighbour's, at a chunk edge) needs redrawing.
pub fn mark(world: &mut World, c: IVec2) {
    if let Some(mut s) = world.get_resource_mut::<Streets>() {
        for d in [IVec2::ZERO, IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            s.dirty.insert((c + d).div_euclid(IVec2::splat(CHUNK)));
        }
    }
}

/// Redraw every street (towns may have changed which cells count as in town).
pub fn mark_all(world: &mut World) {
    if let Some(mut s) = world.get_resource_mut::<Streets>() {
        s.all = true;
    }
}

/// Places that make the roads round them town streets: markets and harbours.
fn town_spots(world: &mut World) -> Vec<(DVec2, f64)> {
    let mut q = world.query::<&Building>();
    q.iter(world)
        .filter_map(|b| {
            let r = match b.kind {
                Kind::Market => 120.0,
                Kind::Harbour => 70.0,
                Kind::Chapel => 60.0,
                _ => return None,
            };
            Some((grid::footprint_center(b.kind, b.origin, b.rot), r))
        })
        .collect()
}

/// Rebuild the street meshes of chunks whose roads changed.
pub fn redraw_streets(world: &mut World) {
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    let chunks: Vec<IVec2> = {
        let Some(mut s) = world.get_resource_mut::<Streets>() else {
            return;
        };
        if s.all {
            s.all = false;
            s.dirty.clear();
            let mut all: HashSet<IVec2> = s.parts.keys().copied().collect();
            for c in &world.resource::<Grid>().roads {
                all.insert(c.div_euclid(IVec2::splat(CHUNK)));
            }
            all.into_iter().collect()
        } else if s.dirty.is_empty() {
            return;
        } else {
            s.dirty.drain().collect()
        }
    };
    let roads = world.resource::<Grid>().roads.clone();
    let towns = town_spots(world);
    let built: Vec<Vec<(Mat, Mesh)>> =
        engine_ui::demo::art::par_map(chunks.len(), |i| chunk_meshes(chunks[i], &roads, &towns));
    for (chunk, meshes) in chunks.into_iter().zip(built) {
        let old = world
            .resource_mut::<Streets>()
            .parts
            .remove(&chunk)
            .unwrap_or_default();
        for e in old {
            world.despawn(e);
        }
        let mut ents = Vec::new();
        for (mat, mesh) in meshes {
            if mesh.indices.is_empty() {
                continue;
            }
            let h = world.resource_mut::<Assets<Mesh>>().add(mesh);
            let mut r = MeshRenderer::new(h, art.mats.get(mat));
            r.cast_shadows = false;
            ents.push(
                world
                    .spawn((
                        Name::new(format!("streets {chunk}")),
                        Transform::default(),
                        r,
                    ))
                    .id(),
            );
        }
        if !ents.is_empty() {
            world.resource_mut::<Streets>().parts.insert(chunk, ents);
        }
    }
}

/// A patch of surface over the ground: a grid of vertices about every meter, each at the
/// ground's height plus `lift`, UVs in world meters.
struct Builder {
    meshes: HashMap<Mat, Mesh>,
}

impl Builder {
    fn mesh(&mut self, m: Mat) -> &mut Mesh {
        self.meshes
            .entry(m)
            .or_insert_with(|| Mesh::new(Vec::new(), Vec::new()))
    }

    /// Axis-aligned world rectangle `(x0, z0)..(x1, z1)`.
    #[allow(clippy::too_many_arguments)]
    fn rect(&mut self, m: Mat, x0: f32, z0: f32, x1: f32, z1: f32, lift: f32, col: Vec4) {
        let nx = ((x1 - x0) / 1.0).ceil().max(1.0) as usize;
        let nz = ((z1 - z0) / 1.0).ceil().max(1.0) as usize;
        let mut hs = vec![0.0f32; (nx + 1) * (nz + 1)];
        for j in 0..=nz {
            for i in 0..=nx {
                let x = x0 + (x1 - x0) * i as f32 / nx as f32;
                let z = z0 + (z1 - z0) * j as f32 / nz as f32;
                hs[j * (nx + 1) + i] = island::height(x as f64, z as f64) as f32 + lift;
            }
        }
        let (dx, dz) = ((x1 - x0) / nx as f32, (z1 - z0) / nz as f32);
        let h = |i: usize, j: usize| hs[j.min(nz) * (nx + 1) + i.min(nx)];
        let mesh = self.mesh(m);
        let base = mesh.vertices.len() as u32;
        for j in 0..=nz {
            for i in 0..=nx {
                let x = x0 + dx * i as f32;
                let z = z0 + dz * j as f32;
                let (il, ir) = (i.saturating_sub(1), i + 1);
                let (jl, jr) = (j.saturating_sub(1), j + 1);
                let gx = (h(ir, j) - h(il, j)) / (dx * (ir.min(nx) - il) as f32);
                let gz = (h(i, jr) - h(i, jl)) / (dz * (jr.min(nz) - jl) as f32);
                let n = Vec3::new(-gx, 1.0, -gz).normalize();
                mesh.vertices.push(Vertex {
                    position: [x, h(i, j), z],
                    normal: n.to_array(),
                    uv: [x, z],
                    color: col.to_array(),
                });
            }
        }
        let w = (nx + 1) as u32;
        for j in 0..nz as u32 {
            for i in 0..nx as u32 {
                let a = base + j * w + i;
                // Counter-clockwise seen from above (+Y): a, a+w, a+w+1 then a, a+w+1, a+1.
                mesh.indices
                    .extend_from_slice(&[a, a + w, a + w + 1, a, a + w + 1, a + 1]);
            }
        }
    }

    /// A kerb along an axis-aligned rectangle: a raised top and faces down into the ground on
    /// both long sides.
    fn kerb(&mut self, x0: f32, z0: f32, x1: f32, z1: f32, col: Vec4) {
        self.rect(Mat::Stone, x0, z0, x1, z1, KERB_H, col);
        // Faces on both long sides, from the kerb top down below the ground.
        let along_x = (x1 - x0) > (z1 - z0);
        let n = ((if along_x { x1 - x0 } else { z1 - z0 }) / 1.0)
            .ceil()
            .max(1.0) as usize;
        for side in [-1.0f32, 1.0] {
            let face = if along_x {
                Vec2::new(0.0, side)
            } else {
                Vec2::new(side, 0.0)
            };
            let mesh = self.mesh(Mat::Stone);
            for k in 0..n {
                let t0 = k as f32 / n as f32;
                let t1 = (k + 1) as f32 / n as f32;
                let (p0, p1) = if along_x {
                    let z = if side > 0.0 { z1 } else { z0 };
                    (
                        Vec2::new(x0 + (x1 - x0) * t0, z),
                        Vec2::new(x0 + (x1 - x0) * t1, z),
                    )
                } else {
                    let x = if side > 0.0 { x1 } else { x0 };
                    (
                        Vec2::new(x, z0 + (z1 - z0) * t0),
                        Vec2::new(x, z0 + (z1 - z0) * t1),
                    )
                };
                let g0 = island::height(p0.x as f64, p0.y as f64) as f32;
                let g1 = island::height(p1.x as f64, p1.y as f64) as f32;
                let quad = [
                    Vec3::new(p0.x, g0 - 0.15, p0.y),
                    Vec3::new(p1.x, g1 - 0.15, p1.y),
                    Vec3::new(p1.x, g1 + KERB_H, p1.y),
                    Vec3::new(p0.x, g0 + KERB_H, p0.y),
                ];
                let nrm = Vec3::new(face.x, 0.0, face.y);
                let base = mesh.vertices.len() as u32;
                for p in &quad {
                    let along = if along_x { p.x } else { p.z };
                    mesh.vertices.push(Vertex {
                        position: p.to_array(),
                        normal: nrm.to_array(),
                        uv: [along, p.y],
                        color: (col * 0.85).with_w(1.0).to_array(),
                    });
                }
                // Wind so the face looks out along `face`.
                let e = quad[1] - quad[0];
                let up = quad[3] - quad[0];
                if e.cross(up).dot(nrm) >= 0.0 {
                    mesh.indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                } else {
                    mesh.indices.extend_from_slice(&[
                        base,
                        base + 2,
                        base + 1,
                        base,
                        base + 3,
                        base + 2,
                    ]);
                }
            }
        }
    }
}

fn hash01(c: IVec2, k: u32) -> f32 {
    let mut x = (c.x as u32).wrapping_mul(0x9E37_79B1) ^ (c.y as u32).wrapping_mul(0x85EB_CA77) ^ k;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// The street surfaces of one chunk, by material.
fn chunk_meshes(chunk: IVec2, roads: &HashSet<IVec2>, towns: &[(DVec2, f64)]) -> Vec<(Mat, Mesh)> {
    let mut b = Builder {
        meshes: HashMap::new(),
    };
    let cobble = hexc(0xb8b0a4);
    let kerb = hexc(0xc8c4bc);
    let rut = hexc(0x6a5640);
    let lo = chunk * CHUNK;
    let half_cell = CELL_M as f32 * 0.5;
    for z in lo.y..lo.y + CHUNK {
        for x in lo.x..lo.x + CHUNK {
            let c = IVec2::new(x, z);
            if !roads.contains(&c) {
                continue;
            }
            let m = grid::cell_center(c);
            let (cx, cz) = (m.x as f32, m.y as f32);
            let link =
                [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y].map(|d| roads.contains(&(c + d)));
            let in_town = towns.iter().any(|(p, r)| (*p - m).length() < *r);
            if in_town {
                // Cobbles: a slightly different stone tone per cell, like relaid patches.
                let t = 0.9 + 0.14 * hash01(c, 1);
                let col = Vec4::new(cobble.x * t, cobble.y * t, cobble.z * t, 1.0);
                b.rect(
                    Mat::Cobble,
                    cx - HALF,
                    cz - HALF,
                    cx + HALF,
                    cz + HALF,
                    LIFT,
                    col,
                );
                // Arms out to linked neighbours, kerbs along whatever is not linked.
                let arm = |b: &mut Builder, d: usize| match d {
                    0 => b.rect(
                        Mat::Cobble,
                        cx + HALF,
                        cz - HALF,
                        cx + half_cell,
                        cz + HALF,
                        LIFT,
                        col,
                    ),
                    1 => b.rect(
                        Mat::Cobble,
                        cx - half_cell,
                        cz - HALF,
                        cx - HALF,
                        cz + HALF,
                        LIFT,
                        col,
                    ),
                    2 => b.rect(
                        Mat::Cobble,
                        cx - HALF,
                        cz + HALF,
                        cx + HALF,
                        cz + half_cell,
                        LIFT,
                        col,
                    ),
                    _ => b.rect(
                        Mat::Cobble,
                        cx - HALF,
                        cz - half_cell,
                        cx + HALF,
                        cz - HALF,
                        LIFT,
                        col,
                    ),
                };
                for (d, linked) in link.iter().enumerate() {
                    if *linked {
                        arm(&mut b, d);
                        // Kerbs along both sides of the arm.
                        match d {
                            0 => {
                                b.kerb(
                                    cx + HALF,
                                    cz - HALF - KERB_W,
                                    cx + half_cell,
                                    cz - HALF,
                                    kerb,
                                );
                                b.kerb(
                                    cx + HALF,
                                    cz + HALF,
                                    cx + half_cell,
                                    cz + HALF + KERB_W,
                                    kerb,
                                );
                            }
                            1 => {
                                b.kerb(
                                    cx - half_cell,
                                    cz - HALF - KERB_W,
                                    cx - HALF,
                                    cz - HALF,
                                    kerb,
                                );
                                b.kerb(
                                    cx - half_cell,
                                    cz + HALF,
                                    cx - HALF,
                                    cz + HALF + KERB_W,
                                    kerb,
                                );
                            }
                            2 => {
                                b.kerb(
                                    cx - HALF - KERB_W,
                                    cz + HALF,
                                    cx - HALF,
                                    cz + half_cell,
                                    kerb,
                                );
                                b.kerb(
                                    cx + HALF,
                                    cz + HALF,
                                    cx + HALF + KERB_W,
                                    cz + half_cell,
                                    kerb,
                                );
                            }
                            _ => {
                                b.kerb(
                                    cx - HALF - KERB_W,
                                    cz - half_cell,
                                    cx - HALF,
                                    cz - HALF,
                                    kerb,
                                );
                                b.kerb(
                                    cx + HALF,
                                    cz - half_cell,
                                    cx + HALF + KERB_W,
                                    cz - HALF,
                                    kerb,
                                );
                            }
                        }
                    } else {
                        match d {
                            0 => b.kerb(
                                cx + HALF,
                                cz - HALF - KERB_W,
                                cx + HALF + KERB_W,
                                cz + HALF + KERB_W,
                                kerb,
                            ),
                            1 => b.kerb(
                                cx - HALF - KERB_W,
                                cz - HALF - KERB_W,
                                cx - HALF,
                                cz + HALF + KERB_W,
                                kerb,
                            ),
                            2 => b.kerb(cx - HALF, cz + HALF, cx + HALF, cz + HALF + KERB_W, kerb),
                            _ => b.kerb(cx - HALF, cz - HALF - KERB_W, cx + HALF, cz - HALF, kerb),
                        }
                    }
                }
            } else {
                // An earth lane: two wheel ruts along each linked direction, through the middle.
                let col = Vec4::new(rut.x, rut.y, rut.z, 1.0) * (0.9 + 0.2 * hash01(c, 2));
                let col = col.with_w(1.0);
                let (w, gap) = (0.32f32, 0.72f32);
                let along_x = link[0] || link[1];
                let along_z = link[2] || link[3];
                if along_x || !along_z {
                    let x0 = if link[1] { cx - half_cell } else { cx - gap };
                    let x1 = if link[0] { cx + half_cell } else { cx + gap };
                    for s in [-1.0f32, 1.0] {
                        let z = cz + s * gap;
                        b.rect(
                            Mat::Stone,
                            x0,
                            z - w * 0.5,
                            x1,
                            z + w * 0.5,
                            LIFT * 0.6,
                            col,
                        );
                    }
                }
                if along_z {
                    let z0 = if link[3] { cz - half_cell } else { cz - gap };
                    let z1 = if link[2] { cz + half_cell } else { cz + gap };
                    for s in [-1.0f32, 1.0] {
                        let x = cx + s * gap;
                        b.rect(
                            Mat::Stone,
                            x - w * 0.5,
                            z0,
                            x + w * 0.5,
                            z1,
                            LIFT * 0.6,
                            col,
                        );
                    }
                }
            }
        }
    }
    b.meshes.into_iter().collect()
}
