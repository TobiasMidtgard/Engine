//! Colliders that aren't simple shapes: triangle meshes baked into a narrow-band signed
//! distance grid, and height grids sampled as they are.
//!
//! A [`DistanceField`] holds real signed distances only within `band` of the surface; past
//! that it reports no contact, so a particle far inside a statue isn't flung across it. The
//! sign comes from angle-weighted pseudo-normals (Bærentzen and Aanæs), which stays right at
//! edges and corners where face normals disagree. Grid points beyond the band are inside when
//! no path of far points connects them to the grid's border; an open mesh (a sheet, a terrain
//! skin) therefore counts as a surface with its back side inside, near the surface only.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use engine_core::math::Vec3;
use engine_core::prelude::Entity;
use engine_world::physics::ColliderShape;

use crate::collide::LocalShape;

/// Finest grid spacing a mesh is baked at, m.
pub const MIN_CELL: f32 = 0.01;
/// Cells along a mesh's longest side before the spacing grows.
const CELLS_ALONG: f32 = 160.0;
/// Most grid points one field may hold (16 MB of distances).
const MAX_POINTS: usize = 4 << 20;
/// Narrowest band of real distances, m: thicker than any cloth or flesh particle.
const MIN_BAND: f32 = 0.04;
/// How deep under a height grid a point still counts as touching it, m. Deeper points are
/// somewhere else (a cave below the terrain), not under the ground.
const HEIGHT_DEPTH: f32 = 0.5;
/// Grid step meshes are baked at, m (coarser for big meshes).
pub const MESH_CELL: f32 = 0.015;
/// Meshes with up to this many triangles bake right away; bigger ones on a thread of their
/// own, colliding once they're done.
const BAKE_INLINE: usize = 4096;
/// A mesh bakes once a soft body comes this close to it (m), so it's ready on arrival.
pub const BAKE_REACH: f64 = 10.0;

/// Sweeps (fixed steps) a baked mesh is kept unused: a minute at 60 Hz.
pub const KEEP_SWEEPS: u32 = 3600;

/// Baked grids of the mesh and height-grid colliders near soft bodies, per collider part.
/// Meshes far from every soft body are never baked; a changed or rescaled collider bakes
/// again, and parts not seen for [`KEEP_SWEEPS`] steps are dropped.
#[derive(Default)]
pub struct FieldCache {
    slots: HashMap<(Entity, u32), Slot>,
}

struct Slot {
    scale: Vec3,
    used: bool,
    /// Sweeps since it was last asked for.
    idle: u32,
    /// Mesh center (collider space) and radius around it.
    bounds: (Vec3, f32),
    state: SlotState,
}

enum SlotState {
    Idle,
    Baking(JoinHandle<Option<DistanceField>>),
    Ready(LocalShape),
    Failed,
}

impl FieldCache {
    /// The grid shape for collider part `key` (a mesh or height grid), or `None` while it
    /// isn't baked. `changed` says the collider changed since the last step; `near` says
    /// whether a sphere (collider-space center, radius) is close enough to a soft body to
    /// bake for.
    pub fn shape(
        &mut self,
        key: (Entity, u32),
        shape: &ColliderShape,
        scale: Vec3,
        changed: bool,
        near: impl Fn(Vec3, f32) -> bool,
    ) -> Option<LocalShape> {
        let slot = self.slots.entry(key).or_insert_with(|| Slot {
            scale,
            used: true,
            idle: 0,
            bounds: (Vec3::ZERO, -1.0),
            state: SlotState::Idle,
        });
        slot.used = true;
        if changed || slot.scale != scale {
            *slot = Slot {
                scale,
                used: true,
                idle: 0,
                bounds: (Vec3::ZERO, -1.0),
                state: SlotState::Idle,
            };
        }
        match &mut slot.state {
            SlotState::Ready(s) => return Some(s.clone()),
            SlotState::Failed => return None,
            SlotState::Baking(job) => {
                if !job.is_finished() {
                    return None;
                }
                let SlotState::Baking(job) = std::mem::replace(&mut slot.state, SlotState::Failed)
                else {
                    unreachable!()
                };
                if let Ok(Some(f)) = job.join() {
                    slot.state = SlotState::Ready(LocalShape::Field(Arc::new(f)));
                }
                return slot.ready();
            }
            SlotState::Idle => {}
        }
        match shape {
            ColliderShape::Heightfield {
                heights,
                rows,
                cols,
                size,
            } => {
                slot.state = match HeightGrid::new(heights, *rows, *cols, *size, scale) {
                    Some(g) => SlotState::Ready(LocalShape::Height(Arc::new(g))),
                    None => SlotState::Failed,
                };
            }
            ColliderShape::TriMesh { vertices, indices } => {
                if slot.bounds.1 < 0.0 {
                    slot.bounds = mesh_bounds(vertices, scale);
                }
                if indices.is_empty() || !near(slot.bounds.0, slot.bounds.1) {
                    return None;
                }
                if indices.len() <= BAKE_INLINE {
                    slot.state = match DistanceField::bake(vertices, indices, scale, MESH_CELL) {
                        Some(f) => SlotState::Ready(LocalShape::Field(Arc::new(f))),
                        None => SlotState::Failed,
                    };
                } else {
                    let (v, i) = (vertices.clone(), indices.clone());
                    slot.state = SlotState::Baking(std::thread::spawn(move || {
                        DistanceField::bake(&v, &i, scale, MESH_CELL)
                    }));
                }
            }
            _ => slot.state = SlotState::Failed,
        }
        slot.ready()
    }

    /// Drop the parts not asked for in the last [`KEEP_SWEEPS`] sweeps. Kept a while, so a
    /// body that rests or swings out of reach for a moment (the unveiling sheet lifted off
    /// its statue) finds the mesh still baked when it comes back, instead of passing through
    /// it while it bakes again.
    pub fn sweep(&mut self) {
        self.slots.retain(|_, s| {
            s.idle = if std::mem::take(&mut s.used) {
                0
            } else {
                s.idle + 1
            };
            s.idle < KEEP_SWEEPS
        });
    }

    /// Where part `key` is, if it is being baked: its center (collider space) and radius.
    pub fn baking(&self, key: (Entity, u32)) -> Option<(Vec3, f32)> {
        self.slots
            .get(&key)
            .filter(|s| matches!(s.state, SlotState::Baking(_)))
            .map(|s| s.bounds)
    }

    /// Meshes baked and waiting to be baked.
    pub fn counts(&self) -> (usize, usize) {
        self.slots.values().fold((0, 0), |(r, b), s| match s.state {
            SlotState::Ready(LocalShape::Field(_)) => (r + 1, b),
            SlotState::Baking(_) => (r, b + 1),
            _ => (r, b),
        })
    }
}

impl Slot {
    fn ready(&self) -> Option<LocalShape> {
        match &self.state {
            SlotState::Ready(s) => Some(s.clone()),
            _ => None,
        }
    }
}

fn mesh_bounds(vertices: &[Vec3], scale: Vec3) -> (Vec3, f32) {
    if vertices.is_empty() {
        return (Vec3::ZERO, 0.0);
    }
    let (lo, hi) = vertices.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), v| (lo.min(*v * scale), hi.max(*v * scale)),
    );
    ((lo + hi) * 0.5, (hi - lo).length() * 0.5)
}

/// A triangle mesh baked into signed distances on a regular grid, in collider space.
#[derive(Debug, Clone)]
pub struct DistanceField {
    /// Position of grid point (0, 0, 0).
    origin: Vec3,
    cell: f32,
    inv_cell: f32,
    dims: [usize; 3],
    /// Signed distance at each grid point, x fastest, clamped to ±`band`.
    dist: Vec<f32>,
    band: f32,
    /// Farthest the mesh reaches from the collider's origin.
    bound: f32,
    triangles: usize,
}

impl DistanceField {
    /// Bake `vertices` (scaled by `scale`) and `indices` at about `cell` meters per grid step
    /// (coarser for big meshes). `None` for a mesh without area.
    pub fn bake(vertices: &[Vec3], indices: &[[u32; 3]], scale: Vec3, cell: f32) -> Option<Self> {
        let mesh = Welded::new(vertices, indices, scale)?;
        let (lo, hi) = mesh.bounds();
        let extent = hi - lo;
        let mut cell = cell.max(MIN_CELL).max(extent.max_element() / CELLS_ALONG);
        let (band, pad, dims) = loop {
            let band = (3.0 * cell).max(MIN_BAND);
            let pad = band + cell;
            let n = ((extent + Vec3::splat(2.0 * pad)) / cell).ceil();
            let dims = [n.x as usize + 1, n.y as usize + 1, n.z as usize + 1];
            if dims[0] * dims[1] * dims[2] <= MAX_POINTS {
                break (band, pad, dims);
            }
            cell *= 1.25;
        };
        let origin = lo - Vec3::splat(pad);
        let mut dist = vec![f32::INFINITY; dims[0] * dims[1] * dims[2]];
        mesh.fill(&mut dist, origin, cell, dims, band);
        sign_far_points(&mut dist, dims, band);
        let bound = corners(lo, hi)
            .iter()
            .map(|c| c.length())
            .fold(0.0, f32::max);
        Some(Self {
            origin,
            cell,
            inv_cell: 1.0 / cell,
            dims,
            dist,
            band,
            bound,
            triangles: mesh.tris.len(),
        })
    }

    /// Grid spacing, m.
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// How far from the surface distances are real, m.
    pub fn band(&self) -> f32 {
        self.band
    }

    /// Grid points along x, y and z.
    pub fn dims(&self) -> [usize; 3] {
        self.dims
    }

    /// Triangles baked in (after welding and dropping slivers).
    pub fn triangles(&self) -> usize {
        self.triangles
    }

    /// Radius of a sphere around the collider's origin that holds the mesh.
    pub fn bounding_radius(&self) -> f32 {
        self.bound
    }

    /// Signed distance from `p` (collider space) to the mesh and the outward normal there, or
    /// `None` outside the grid or further than the band from the surface.
    pub fn sample(&self, p: Vec3) -> Option<(f32, Vec3)> {
        let g = (p - self.origin) * self.inv_cell;
        let [nx, ny, nz] = self.dims;
        if g.x < 0.0
            || g.y < 0.0
            || g.z < 0.0
            || g.x > (nx - 1) as f32
            || g.y > (ny - 1) as f32
            || g.z > (nz - 1) as f32
        {
            return None;
        }
        let i = (g.x as usize).min(nx - 2);
        let j = (g.y as usize).min(ny - 2);
        let k = (g.z as usize).min(nz - 2);
        let (fx, fy, fz) = (g.x - i as f32, g.y - j as f32, g.z - k as f32);
        let at =
            |di: usize, dj: usize, dk: usize| self.dist[(i + di) + nx * ((j + dj) + ny * (k + dk))];
        let c = [
            at(0, 0, 0),
            at(1, 0, 0),
            at(0, 1, 0),
            at(1, 1, 0),
            at(0, 0, 1),
            at(1, 0, 1),
            at(0, 1, 1),
            at(1, 1, 1),
        ];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        // Along x first, then y, then z.
        let x00 = lerp(c[0], c[1], fx);
        let x10 = lerp(c[2], c[3], fx);
        let x01 = lerp(c[4], c[5], fx);
        let x11 = lerp(c[6], c[7], fx);
        let y0 = lerp(x00, x10, fy);
        let y1 = lerp(x01, x11, fy);
        let d = lerp(y0, y1, fz);
        if d.abs() > self.band * 0.95 {
            return None;
        }
        let gx = lerp(
            lerp(c[1] - c[0], c[3] - c[2], fy),
            lerp(c[5] - c[4], c[7] - c[6], fy),
            fz,
        );
        let gy = lerp(x10 - x00, x11 - x01, fz);
        let gz = y1 - y0;
        let n = Vec3::new(gx, gy, gz).normalize_or_zero();
        (n != Vec3::ZERO).then_some((d, n))
    }
}

/// A height grid as the physics contract stores it: `rows × cols` heights, row-major, rows
/// along Z and columns along X, centered on the collider and spanning `size` (x, z).
#[derive(Debug, Clone)]
pub struct HeightGrid {
    heights: Vec<f32>,
    rows: usize,
    cols: usize,
    half: [f32; 2],
    step: [f32; 2],
    bound: f32,
}

impl HeightGrid {
    /// `None` when the heights don't fill the grid.
    pub fn new(heights: &[f32], rows: u32, cols: u32, size: [f32; 2], scale: Vec3) -> Option<Self> {
        let (rows, cols) = (rows as usize, cols as usize);
        if rows < 2 || cols < 2 || heights.len() != rows * cols {
            return None;
        }
        let s = scale.abs();
        let heights: Vec<f32> = heights.iter().map(|h| h * s.y).collect();
        let half = [size[0] * s.x * 0.5, size[1] * s.z * 0.5];
        let top = heights.iter().fold(0.0f32, |m, h| m.max(h.abs()));
        Some(Self {
            heights,
            rows,
            cols,
            half,
            step: [
                2.0 * half[0] / (cols - 1) as f32,
                2.0 * half[1] / (rows - 1) as f32,
            ],
            bound: (half[0] * half[0] + half[1] * half[1] + top * top).sqrt(),
        })
    }

    /// Radius of a sphere around the collider's origin that holds the grid.
    pub fn bounding_radius(&self) -> f32 {
        self.bound
    }

    /// Height above or below the surface (along its normal) and the normal, or `None` beside
    /// the grid or deep under it.
    pub fn sample(&self, p: Vec3) -> Option<(f32, Vec3)> {
        let u = (p.x + self.half[0]) / self.step[0];
        let v = (p.z + self.half[1]) / self.step[1];
        if u < 0.0 || v < 0.0 || u > (self.cols - 1) as f32 || v > (self.rows - 1) as f32 {
            return None;
        }
        let c = (u as usize).min(self.cols - 2);
        let r = (v as usize).min(self.rows - 2);
        let (fu, fv) = (u - c as f32, v - r as f32);
        let h = |r: usize, c: usize| self.heights[r * self.cols + c];
        let (h00, h01, h10, h11) = (h(r, c), h(r, c + 1), h(r + 1, c), h(r + 1, c + 1));
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let height = lerp(lerp(h00, h01, fu), lerp(h10, h11, fu), fv);
        let dx = lerp(h01 - h00, h11 - h10, fv) / self.step[0];
        let dz = lerp(h10 - h00, h11 - h01, fu) / self.step[1];
        let n = Vec3::new(-dx, 1.0, -dz).normalize();
        let d = (p.y - height) * n.y;
        (d > -HEIGHT_DEPTH).then_some((d, n))
    }
}

fn corners(lo: Vec3, hi: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    })
}

/// A mesh with duplicate vertices merged (seams split for UVs would break the pseudo-normals)
/// and the normals the sign test needs.
struct Welded {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    /// Angle-weighted normal per vertex.
    vertex_normals: Vec<Vec3>,
    /// Per triangle: unit face normal, then the normals of edges ab, bc and ca.
    tri_normals: Vec<[Vec3; 4]>,
}

impl Welded {
    fn new(vertices: &[Vec3], indices: &[[u32; 3]], scale: Vec3) -> Option<Self> {
        let mut verts = Vec::new();
        let mut remap = vec![u32::MAX; vertices.len()];
        let mut seen: HashMap<[i32; 3], u32> = HashMap::new();
        let used = |i: u32| (i as usize) < vertices.len();
        for tri in indices {
            for &i in tri {
                if !used(i) || remap[i as usize] != u32::MAX {
                    continue;
                }
                let p = vertices[i as usize] * scale;
                let key = (p * 1.0e4).round().to_array().map(|v| v as i32);
                remap[i as usize] = *seen.entry(key).or_insert_with(|| {
                    verts.push(p);
                    verts.len() as u32 - 1
                });
            }
        }
        let mirrored = scale.x * scale.y * scale.z < 0.0;
        let mut tris = Vec::with_capacity(indices.len());
        let mut face = Vec::with_capacity(indices.len());
        for tri in indices {
            if !tri.iter().all(|&i| used(i)) {
                continue;
            }
            let mut t = tri.map(|i| remap[i as usize]);
            // A mirroring scale turns the triangles inside out.
            if mirrored {
                t.swap(1, 2);
            }
            let [a, b, c] = t.map(|i| verts[i as usize]);
            let n = (b - a).cross(c - a);
            // Slivers have no reliable normal.
            if n.length_squared() < 1e-14 {
                continue;
            }
            tris.push(t);
            face.push(n.normalize());
        }
        if tris.is_empty() {
            return None;
        }
        let mut vertex_normals = vec![Vec3::ZERO; verts.len()];
        let mut edges: HashMap<(u32, u32), Vec3> = HashMap::new();
        for (t, n) in tris.iter().zip(&face) {
            for k in 0..3 {
                let (i, j, l) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                let p = verts[i as usize];
                let angle = (verts[j as usize] - p).angle_between(verts[l as usize] - p);
                vertex_normals[i as usize] += *n * angle;
                *edges.entry((i.min(j), i.max(j))).or_default() += *n;
            }
        }
        let edge = |i: u32, j: u32| edges[&(i.min(j), i.max(j))];
        let tri_normals = tris
            .iter()
            .zip(&face)
            .map(|(t, n)| [*n, edge(t[0], t[1]), edge(t[1], t[2]), edge(t[2], t[0])])
            .collect();
        Some(Self {
            verts,
            tris,
            vertex_normals,
            tri_normals,
        })
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for t in &self.tris {
            for &i in t {
                lo = lo.min(self.verts[i as usize]);
                hi = hi.max(self.verts[i as usize]);
            }
        }
        (lo, hi)
    }

    /// Signed distances at every grid point within `band` of a triangle (others stay
    /// infinite), in z slabs on all cores.
    fn fill(&self, dist: &mut [f32], origin: Vec3, cell: f32, dims: [usize; 3], band: f32) {
        let [nx, ny, nz] = dims;
        let reach = band + cell;
        // Each triangle's grid box, inclusive.
        let boxes: Vec<([usize; 3], [usize; 3])> = self
            .tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.verts[i as usize]);
                let lo = (a.min(b).min(c) - Vec3::splat(reach) - origin) / cell;
                let hi = (a.max(b).max(c) + Vec3::splat(reach) - origin) / cell;
                let lo = lo.ceil().max(Vec3::ZERO);
                let hi = hi.floor();
                (
                    [lo.x as usize, lo.y as usize, lo.z as usize],
                    [
                        (hi.x as usize).min(nx - 1),
                        (hi.y as usize).min(ny - 1),
                        (hi.z as usize).min(nz - 1),
                    ],
                )
            })
            .collect();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(nz);
        let per = nz.div_ceil(threads);
        std::thread::scope(|s| {
            for (slab, out) in dist.chunks_mut(per * nx * ny).enumerate() {
                let boxes = &boxes;
                s.spawn(move || {
                    let z0 = slab * per;
                    let z1 = (z0 + per).min(nz);
                    for (t, (lo, hi)) in boxes.iter().enumerate() {
                        if hi[2] < z0 || lo[2] >= z1 {
                            continue;
                        }
                        let [a, b, c] = self.tris[t].map(|i| self.verts[i as usize]);
                        for k in lo[2].max(z0)..=hi[2].min(z1 - 1) {
                            for j in lo[1]..=hi[1] {
                                let row = nx * (j + ny * (k - z0));
                                for i in lo[0]..=hi[0] {
                                    let p = origin + Vec3::new(i as f32, j as f32, k as f32) * cell;
                                    let (q, region) = closest_on_triangle(p, a, b, c);
                                    let d = p.distance(q);
                                    let slot = &mut out[row + i];
                                    if d >= slot.abs() {
                                        continue;
                                    }
                                    let n = self.pseudo_normal(t, region);
                                    *slot = if (p - q).dot(n) < 0.0 { -d } else { d };
                                }
                            }
                        }
                    }
                });
            }
        });
    }

    fn pseudo_normal(&self, t: usize, region: Region) -> Vec3 {
        let n = &self.tri_normals[t];
        match region {
            Region::Face => n[0],
            Region::Vertex(k) => self.vertex_normals[self.tris[t][k as usize] as usize],
            Region::Edge(k) => n[1 + k as usize],
        }
    }
}

/// Which feature of a triangle a closest point lies on. Edges are numbered ab, bc, ca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Face,
    Vertex(u8),
    Edge(u8),
}

/// Closest point to `p` on triangle `abc` and the feature it's on (Ericson, Real-Time
/// Collision Detection, 5.1.5).
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, Region) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, Region::Vertex(0));
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, Region::Vertex(1));
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, Region::Edge(0));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, Region::Vertex(2));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, Region::Edge(2));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, Region::Edge(1));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, Region::Face)
}

/// Points no triangle reached become ±`band`: outside when far points connect them to the
/// grid's border, inside otherwise. Near points are clamped to the band.
fn sign_far_points(dist: &mut [f32], dims: [usize; 3], band: f32) {
    let [nx, ny, nz] = dims;
    let mut outside = vec![false; dist.len()];
    let mut stack = Vec::new();
    let idx = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let border =
                    i == 0 || j == 0 || k == 0 || i == nx - 1 || j == ny - 1 || k == nz - 1;
                let n = idx(i, j, k);
                if border && dist[n].is_infinite() && !outside[n] {
                    outside[n] = true;
                    stack.push((i, j, k));
                }
            }
        }
    }
    while let Some((i, j, k)) = stack.pop() {
        let mut visit = |i: usize, j: usize, k: usize| {
            let n = idx(i, j, k);
            if !outside[n] && dist[n].is_infinite() {
                outside[n] = true;
                stack.push((i, j, k));
            }
        };
        if i > 0 {
            visit(i - 1, j, k);
        }
        if i + 1 < nx {
            visit(i + 1, j, k);
        }
        if j > 0 {
            visit(i, j - 1, k);
        }
        if j + 1 < ny {
            visit(i, j + 1, k);
        }
        if k > 0 {
            visit(i, j, k - 1);
        }
        if k + 1 < nz {
            visit(i, j, k + 1);
        }
    }
    for (d, out) in dist.iter_mut().zip(&outside) {
        *d = if d.is_infinite() {
            if *out { band } else { -band }
        } else {
            d.clamp(-band, band)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube (side 1, centered) with each face split into its own four vertices, as an
    /// exported mesh with hard edges would be.
    pub(crate) fn split_cube() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut v = Vec::new();
        let mut t = Vec::new();
        for axis in 0..3 {
            for side in [-1.0f32, 1.0] {
                let n =
                    Vec3::from_array(std::array::from_fn(|k| if k == axis { side } else { 0.0 }));
                let u =
                    Vec3::from_array(std::array::from_fn(|k| (k == (axis + 1) % 3) as u8 as f32));
                let w = n.cross(u);
                let base = v.len() as u32;
                for (su, sw) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    v.push((n + u * su + w * sw) * 0.5);
                }
                t.push([base, base + 1, base + 2]);
                t.push([base, base + 2, base + 3]);
            }
        }
        (v, t)
    }

    #[test]
    fn baked_cube_matches_the_exact_box_distance() {
        let (v, t) = split_cube();
        let f = DistanceField::bake(&v, &t, Vec3::ONE, 0.02).unwrap();
        // Faces are wound outward.
        let exact = crate::collide::LocalShape::Cuboid {
            half: Vec3::splat(0.5),
        };
        let mut checked = 0;
        for k in 0..2000 {
            // Points all around the surface, edges and corners included.
            let s = k as f32;
            let p = Vec3::new((s * 0.731).sin(), (s * 1.37).cos(), (s * 2.11 + 0.4).sin()) * 0.62;
            let (want, _) = exact.sdf(p);
            if want.abs() > f.band() * 0.8 {
                continue;
            }
            let (got, n) = f.sample(p).expect("inside the band");
            assert!(
                (got - want).abs() < f.cell() * 0.75,
                "at {p}: {got} vs {want}"
            );
            if want > 0.005 {
                assert!(n.dot(p) > 0.0, "normal {n} points into the cube at {p}");
            }
            checked += 1;
        }
        assert!(checked > 300, "only {checked} points in the band");
        // Deep inside and far outside report nothing.
        assert!(f.sample(Vec3::ZERO).is_none());
        assert!(f.sample(Vec3::new(0.0, 0.62, 0.0)).is_none());
        assert!(f.sample(Vec3::splat(5.0)).is_none());
        assert!((f.bounding_radius() - 0.75f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn an_open_sheet_is_solid_only_just_under_its_face() {
        // A 2 × 2 m sheet at y = 0 facing up.
        let v = vec![
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 1.0),
        ];
        let t = vec![[0, 2, 1], [0, 3, 2]];
        let f = DistanceField::bake(&v, &t, Vec3::ONE, 0.02).unwrap();
        let (d, n) = f.sample(Vec3::new(0.3, 0.03, -0.2)).unwrap();
        assert!((d - 0.03).abs() < 0.005 && n.y > 0.99, "{d} {n}");
        let (d, _) = f.sample(Vec3::new(0.3, -0.02, -0.2)).unwrap();
        assert!((d + 0.02).abs() < 0.005, "{d}");
        assert!(f.sample(Vec3::new(0.3, -0.5, -0.2)).is_none());
    }

    #[test]
    fn height_grid_follows_a_slope() {
        // 3 × 3 heights rising 1 m per meter along +X over 4 × 4 m.
        let h: Vec<f32> = (0..9).map(|i| (i % 3) as f32 * 2.0 - 2.0).collect();
        let g = HeightGrid::new(&h, 3, 3, [4.0, 4.0], Vec3::ONE).unwrap();
        let (d, n) = g.sample(Vec3::new(0.5, 1.5, 0.3)).unwrap();
        let expect_n = Vec3::new(-1.0, 1.0, 0.0).normalize();
        assert!(n.abs_diff_eq(expect_n, 1e-4), "{n}");
        assert!((d - 1.0 * expect_n.y).abs() < 1e-4, "{d}");
        assert!(g.sample(Vec3::new(3.0, 0.0, 0.0)).is_none());
        assert!(g.sample(Vec3::new(0.0, -2.0, 0.0)).is_none());
    }
}
