//! Turning signed-distance fields into meshes: narrow-band surface nets (only cells near the
//! surface are evaluated), vertices projected onto the surface with normals from the field
//! gradient, then split into parts (paint, trim, glass openings…) with their own UVs.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::sdf::gradient;

/// A multiply-rotate hasher for small integer keys (edge and vertex ids): much cheaper than
/// the default SipHash, and the keys are not attacker-controlled.
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

/// A hash map keyed by ids.
pub type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

/// A field the mesher can sample from several threads.
pub type Field<'a> = &'a (dyn Fn(Vec3) -> f32 + Sync);

/// Positions, normals and triangles straight out of the mesher.
#[derive(Debug, Clone, Default)]
pub struct RawMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub indices: Vec<u32>,
}

/// Run `f(i, &mut out[i])` for every element, spread over the machine's cores.
pub fn par_fill<T: Send>(out: &mut [T], f: impl Fn(usize, &mut T) + Sync) {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    if out.len() < 256 || threads == 1 {
        for (i, v) in out.iter_mut().enumerate() {
            f(i, v);
        }
        return;
    }
    let chunk = out.len().div_ceil(threads * 4);
    let f = &f;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let chunks: Vec<(usize, &mut [T])> = out
        .chunks_mut(chunk)
        .enumerate()
        .map(|(i, c)| (i * chunk, c))
        .collect();
    let chunks = std::sync::Mutex::new(chunks.into_iter().map(Some).collect::<Vec<_>>());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let item = {
                        let mut c = chunks.lock().unwrap();
                        if i >= c.len() {
                            break;
                        }
                        c[i].take()
                    };
                    if let Some((start, slice)) = item {
                        for (k, v) in slice.iter_mut().enumerate() {
                            f(start + k, v);
                        }
                    }
                }
            });
        }
    });
}

/// Mesh the zero set of `f` inside `[min, max]` with cells of `h` meters.
///
/// `mirror_x`: the field is symmetric about x = 0; only x ≥ 0 is sampled and the result is
/// exactly symmetric. `lipschitz`: how much faster than 1 m/m the field may change (1 for a
/// true distance); cells farther than that bound from the surface are skipped.
pub fn surface_nets(
    f: Field,
    min: Vec3,
    max: Vec3,
    h: f32,
    mirror_x: bool,
    lipschitz: f32,
) -> RawMesh {
    let (nx, x0) = if mirror_x {
        let half = ((max.x.abs().max(min.x.abs()) / h) + 0.5).ceil() as usize + 1;
        (half * 2, -(half as f32 - 0.5) * h)
    } else {
        (((max.x - min.x) / h).ceil() as usize + 1, min.x)
    };
    let ny = ((max.y - min.y) / h).ceil() as usize + 1;
    let nz = ((max.z - min.z) / h).ceil() as usize + 1;
    let origin = Vec3::new(x0, min.y, min.z);
    let node_pos =
        |i: usize, j: usize, k: usize| origin + Vec3::new(i as f32, j as f32, k as f32) * h;
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
    let half_x = nx / 2;
    let mirror_i = |i: usize| {
        if mirror_x && i < half_x {
            nx - 1 - i
        } else {
            i
        }
    };

    // Coarse pass: one sample per block corner.
    const B: usize = 4;
    let bx = (nx - 1).div_ceil(B);
    let by = (ny - 1).div_ceil(B);
    let bz = (nz - 1).div_ceil(B);
    let cnode = |a: usize, n: usize| (a * B).min(n - 1);
    let (cx, cy, cz) = (bx + 1, by + 1, bz + 1);
    let mut coarse = vec![0.0f32; cx * cy * cz];
    {
        let ci = |a: usize, b: usize, c: usize| (c * cy + b) * cx + a;
        // Sample only the x ≥ 0 half when mirrored, then copy.
        let mut list: Vec<(usize, Vec3)> = Vec::new();
        for c in 0..cz {
            for b in 0..cy {
                for a in 0..cx {
                    let i = cnode(a, nx);
                    if mirror_x && i < half_x {
                        continue;
                    }
                    list.push((ci(a, b, c), node_pos(i, cnode(b, ny), cnode(c, nz))));
                }
            }
        }
        let mut vals = vec![0.0f32; list.len()];
        par_fill(&mut vals, |n, v| *v = f(list[n].1));
        for (n, (at, _)) in list.iter().enumerate() {
            coarse[*at] = vals[n];
        }
        if mirror_x {
            for c in 0..cz {
                for b in 0..cy {
                    for a in 0..cx {
                        let i = cnode(a, nx);
                        if i < half_x {
                            // Find the coarse node holding the mirror position (may not be a
                            // coarse node itself: sample the field directly then).
                            let mi = nx - 1 - i;
                            let p = node_pos(mi, cnode(b, ny), cnode(c, nz));
                            coarse[ci(a, b, c)] = if mi % B == 0 || mi == nx - 1 {
                                coarse[ci(if mi == nx - 1 { cx - 1 } else { mi / B }, b, c)]
                            } else {
                                f(p)
                            };
                        }
                    }
                }
            }
        }
        // Mark fine nodes inside blocks the surface may pass through.
        let threshold = (B as f32) * h * 0.87 * lipschitz + h;
        let mut need = vec![false; nx * ny * nz];
        let mut sign = vec![1.0f32; bx * by * bz];
        for c in 0..bz {
            for b in 0..by {
                for a in 0..bx {
                    let mut lo = f32::MAX;
                    let mut pos = false;
                    let mut neg = false;
                    for (da, db, dc) in [
                        (0, 0, 0),
                        (1, 0, 0),
                        (0, 1, 0),
                        (1, 1, 0),
                        (0, 0, 1),
                        (1, 0, 1),
                        (0, 1, 1),
                        (1, 1, 1),
                    ] {
                        let v = coarse[ci(a + da, b + db, c + dc)];
                        lo = lo.min(v.abs());
                        pos |= v >= 0.0;
                        neg |= v < 0.0;
                    }
                    sign[(c * by + b) * bx + a] = if neg { -1.0 } else { 1.0 };
                    if lo <= threshold || (pos && neg) {
                        for k in cnode(c, nz)..=cnode(c + 1, nz) {
                            for j in cnode(b, ny)..=cnode(b + 1, ny) {
                                for i in cnode(a, nx)..=cnode(a + 1, nx) {
                                    need[idx(mirror_i(i), j, k)] = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Fine pass over the marked nodes.
        let list: Vec<u32> = (0..need.len() as u32)
            .filter(|&n| need[n as usize])
            .collect();
        let mut vals = vec![0.0f32; list.len()];
        par_fill(&mut vals, |n, v| {
            let id = list[n] as usize;
            let i = id % nx;
            let j = (id / nx) % ny;
            let k = id / (nx * ny);
            *v = f(node_pos(i, j, k));
        });
        let mut grid = vec![f32::NAN; nx * ny * nz];
        for (n, &id) in list.iter().enumerate() {
            grid[id as usize] = vals[n];
        }
        drop(list);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let id = idx(i, j, k);
                    let mi = mirror_i(i);
                    if mi != i {
                        grid[id] = grid[idx(mi, j, k)];
                    }
                    if grid[id].is_nan() {
                        let (a, b, c) = (
                            (mi / B).min(bx - 1),
                            (j / B).min(by - 1),
                            (k / B).min(bz - 1),
                        );
                        grid[id] = sign[(c * by + b) * bx + a] * threshold * 2.0;
                    }
                }
            }
        }
        mesh_grid(f, &grid, nx, ny, nz, origin, h, mirror_x)
    }
}

#[allow(clippy::too_many_arguments)]
fn mesh_grid(
    f: Field,
    grid: &[f32],
    nx: usize,
    ny: usize,
    nz: usize,
    origin: Vec3,
    h: f32,
    mirror_x: bool,
) -> RawMesh {
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cidx = |i: usize, j: usize, k: usize| (k * cy + j) * cx + i;
    let half_x = nx / 2;
    // Active cells (corners of mixed sign).
    let mut cells: Vec<u32> = Vec::new();
    for k in 0..cz {
        for j in 0..cy {
            for i in 0..cx {
                if mirror_x && i + 1 < half_x {
                    continue;
                }
                let v0 = grid[idx(i, j, k)] < 0.0;
                let mut mixed = false;
                for (a, b, c) in [
                    (1, 0, 0),
                    (0, 1, 0),
                    (1, 1, 0),
                    (0, 0, 1),
                    (1, 0, 1),
                    (0, 1, 1),
                    (1, 1, 1),
                ] {
                    if (grid[idx(i + a, j + b, k + c)] < 0.0) != v0 {
                        mixed = true;
                        break;
                    }
                }
                if mixed {
                    cells.push(cidx(i, j, k) as u32);
                }
            }
        }
    }
    let eps = (h * 0.12).max(0.0008);
    let mut verts: Vec<(Vec3, Vec3)> = vec![(Vec3::ZERO, Vec3::ZERO); cells.len()];
    par_fill(&mut verts, |n, out| {
        let c = cells[n] as usize;
        let i = c % cx;
        let j = (c / cx) % cy;
        let k = c / (cx * cy);
        let mut sum = Vec3::ZERO;
        let mut count = 0.0;
        type Corner = (usize, usize, usize);
        const EDGES: [(Corner, Corner); 12] = [
            ((0, 0, 0), (1, 0, 0)),
            ((0, 1, 0), (1, 1, 0)),
            ((0, 0, 1), (1, 0, 1)),
            ((0, 1, 1), (1, 1, 1)),
            ((0, 0, 0), (0, 1, 0)),
            ((1, 0, 0), (1, 1, 0)),
            ((0, 0, 1), (0, 1, 1)),
            ((1, 0, 1), (1, 1, 1)),
            ((0, 0, 0), (0, 0, 1)),
            ((1, 0, 0), (1, 0, 1)),
            ((0, 1, 0), (0, 1, 1)),
            ((1, 1, 0), (1, 1, 1)),
        ];
        for (a, b) in EDGES {
            let va = grid[idx(i + a.0, j + a.1, k + a.2)];
            let vb = grid[idx(i + b.0, j + b.1, k + b.2)];
            if (va < 0.0) != (vb < 0.0) {
                let t = va / (va - vb);
                let pa = Vec3::new(a.0 as f32, a.1 as f32, a.2 as f32);
                let pb = Vec3::new(b.0 as f32, b.1 as f32, b.2 as f32);
                sum += pa + (pb - pa) * t;
                count += 1.0;
            }
        }
        let cell_min = origin + Vec3::new(i as f32, j as f32, k as f32) * h;
        let mut p = cell_min + sum / count * h;
        // One Newton step onto the surface (direction from the cell's corner values), kept
        // near the cell.
        let d = f(p);
        let v = |a: usize, b: usize, c: usize| grid[idx(i + a, j + b, k + c)];
        let g = Vec3::new(
            v(1, 0, 0) - v(0, 0, 0) + v(1, 1, 0) - v(0, 1, 0) + v(1, 0, 1) - v(0, 0, 1)
                + v(1, 1, 1)
                - v(0, 1, 1),
            v(0, 1, 0) - v(0, 0, 0) + v(1, 1, 0) - v(1, 0, 0) + v(0, 1, 1) - v(0, 0, 1)
                + v(1, 1, 1)
                - v(1, 0, 1),
            v(0, 0, 1) - v(0, 0, 0) + v(1, 0, 1) - v(1, 0, 0) + v(0, 1, 1) - v(0, 1, 0)
                + v(1, 1, 1)
                - v(1, 1, 0),
        ) / (4.0 * h);
        let gl = g.length_squared();
        if gl > 1e-8 {
            let q = p - g * (d / gl);
            let lo = cell_min - Vec3::splat(0.5 * h);
            let hi = cell_min + Vec3::splat(1.5 * h);
            p = q.clamp(lo, hi);
        }
        let mut nrm = gradient(f, p, eps).normalize_or(g.normalize_or(Vec3::Y));
        if mirror_x && i + 1 == half_x {
            p.x = 0.0;
            nrm.x = 0.0;
            nrm = nrm.normalize_or(Vec3::Y);
        }
        *out = (p, nrm);
    });
    let mut cell_vert = vec![u32::MAX; cx * cy * cz];
    let mut mesh = RawMesh::default();
    for (n, &c) in cells.iter().enumerate() {
        cell_vert[c as usize] = mesh.positions.len() as u32;
        mesh.positions.push(verts[n].0);
        mesh.normals.push(verts[n].1);
    }
    if mirror_x {
        // Left half: mirror images of the right-half cells.
        for (n, &c) in cells.iter().enumerate() {
            let c = c as usize;
            let i = c % cx;
            if i + 1 == half_x {
                continue;
            }
            let j = (c / cx) % cy;
            let k = c / (cx * cy);
            let mi = cx - 1 - i;
            let (p, nr) = verts[n];
            cell_vert[cidx(mi, j, k)] = mesh.positions.len() as u32;
            mesh.positions.push(Vec3::new(-p.x, p.y, p.z));
            mesh.normals.push(Vec3::new(-nr.x, nr.y, nr.z));
        }
    }
    // Quads across every edge whose ends differ in sign.
    let push_quad = |m: &mut RawMesh, q: [u32; 4]| {
        if q.contains(&u32::MAX) {
            return;
        }
        let p = |v: u32| m.positions[v as usize];
        let d02 = (p(q[0]) - p(q[2])).length_squared();
        let d13 = (p(q[1]) - p(q[3])).length_squared();
        if d02 <= d13 {
            m.indices
                .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        } else {
            m.indices
                .extend_from_slice(&[q[0], q[1], q[3], q[1], q[2], q[3]]);
        }
    };
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let inside = grid[idx(i, j, k)] < 0.0;
                // X edge.
                if i + 1 < nx && j >= 1 && k >= 1 && j < cy && k < cz {
                    let other = grid[idx(i + 1, j, k)] < 0.0;
                    if inside != other {
                        let q = [
                            cell_vert[cidx(i, j - 1, k - 1)],
                            cell_vert[cidx(i, j, k - 1)],
                            cell_vert[cidx(i, j, k)],
                            cell_vert[cidx(i, j - 1, k)],
                        ];
                        push_quad(&mut mesh, if inside { q } else { [q[3], q[2], q[1], q[0]] });
                    }
                }
                // Y edge.
                if j + 1 < ny && i >= 1 && k >= 1 && i < cx && k < cz {
                    let other = grid[idx(i, j + 1, k)] < 0.0;
                    if inside != other {
                        let q = [
                            cell_vert[cidx(i - 1, j, k - 1)],
                            cell_vert[cidx(i - 1, j, k)],
                            cell_vert[cidx(i, j, k)],
                            cell_vert[cidx(i, j, k - 1)],
                        ];
                        push_quad(&mut mesh, if inside { q } else { [q[3], q[2], q[1], q[0]] });
                    }
                }
                // Z edge.
                if k + 1 < nz && i >= 1 && j >= 1 && i < cx && j < cy {
                    let other = grid[idx(i, j, k + 1)] < 0.0;
                    if inside != other {
                        let q = [
                            cell_vert[cidx(i - 1, j - 1, k)],
                            cell_vert[cidx(i, j - 1, k)],
                            cell_vert[cidx(i, j, k)],
                            cell_vert[cidx(i - 1, j, k)],
                        ];
                        push_quad(&mut mesh, if inside { q } else { [q[3], q[2], q[1], q[0]] });
                    }
                }
            }
        }
    }
    mesh
}

/// Refine where the surface bends sharply: every edge whose end normals differ by more than
/// `angle_deg` and whose midpoint is more than `min_err` meters off the surface is split
/// there, the new vertex projected onto the field (conforming 1→2/3/4 splits, no
/// T-junctions). `levels` rounds. Tight fillets and creases come out smooth while flat
/// panels keep their coarse triangles.
pub fn refine(raw: &RawMesh, f: Field, levels: usize, angle_deg: f32, min_err: f32) -> RawMesh {
    let mut m = raw.clone();
    let cos_t = angle_deg.to_radians().cos();
    for _ in 0..levels {
        let mut split: IdMap<(u32, u32), u32> = IdMap::default();
        let mut pending: Vec<(u32, u32)> = Vec::new();
        for t in m.indices.as_chunks::<3>().0.iter() {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let k = (a.min(b), a.max(b));
                if split.contains_key(&k) {
                    continue;
                }
                let na = m.normals[a as usize];
                let nb = m.normals[b as usize];
                if na.dot(nb) < cos_t {
                    split.insert(k, u32::MAX);
                    pending.push(k);
                }
            }
        }
        // Only where the chord is visibly off the surface.
        let mut err = vec![0.0f32; pending.len()];
        {
            let pos = &m.positions;
            par_fill(&mut err, |i, e| {
                let (a, b) = pending[i];
                *e = f((pos[a as usize] + pos[b as usize]) * 0.5).abs();
            });
        }
        for (k, e) in pending.iter().zip(&err) {
            if *e <= min_err {
                split.remove(k);
            }
        }
        let pending: Vec<(u32, u32)> = pending
            .iter()
            .zip(&err)
            .filter(|(_, e)| **e > min_err)
            .map(|(k, _)| *k)
            .collect();
        if pending.is_empty() {
            break;
        }
        // New vertices: midpoints pulled onto the surface.
        let mut newv: Vec<(Vec3, Vec3)> = vec![(Vec3::ZERO, Vec3::ZERO); pending.len()];
        {
            let pos = &m.positions;
            let nrm = &m.normals;
            par_fill(&mut newv, |i, out| {
                let (a, b) = pending[i];
                let (pa, pb) = (pos[a as usize], pos[b as usize]);
                let len = (pb - pa).length();
                let mut p = (pa + pb) * 0.5;
                let eps = (len * 0.1).clamp(0.0005, 0.003);
                for _ in 0..3 {
                    let d = f(p);
                    if d.abs() < 2e-5 {
                        break;
                    }
                    let g = gradient(f, p, eps);
                    let gl = g.length_squared();
                    if gl < 1e-8 {
                        break;
                    }
                    let step = g * (d / gl);
                    // Never move farther than the edge is long (stay near the edge).
                    p -= step.clamp_length_max(len * 0.6);
                }
                let n = gradient(f, p, eps)
                    .normalize_or((nrm[a as usize] + nrm[b as usize]).normalize_or(Vec3::Y));
                *out = (p, n);
            });
        }
        for (i, k) in pending.iter().enumerate() {
            let id = m.positions.len() as u32;
            m.positions.push(newv[i].0);
            m.normals.push(newv[i].1);
            split.insert(*k, id);
        }
        let tris = std::mem::take(&mut m.indices);
        let mut out = Vec::with_capacity(tris.len() * 2);
        let mid = |a: u32, b: u32| split.get(&(a.min(b), a.max(b))).copied();
        for t in tris.as_chunks::<3>().0.iter() {
            let (a, b, c) = (t[0], t[1], t[2]);
            let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
            match (ab, bc, ca) {
                (None, None, None) => out.extend_from_slice(t),
                (Some(x), None, None) => out.extend_from_slice(&[a, x, c, x, b, c]),
                (None, Some(x), None) => out.extend_from_slice(&[a, b, x, a, x, c]),
                (None, None, Some(x)) => out.extend_from_slice(&[a, b, x, x, b, c]),
                (Some(x), Some(y), None) => {
                    out.extend_from_slice(&[x, b, y]);
                    quad(&m.positions, &mut out, a, x, y, c);
                }
                (None, Some(x), Some(y)) => {
                    out.extend_from_slice(&[y, x, c]);
                    quad(&m.positions, &mut out, a, b, x, y);
                }
                (Some(x), None, Some(y)) => {
                    out.extend_from_slice(&[a, x, y]);
                    quad(&m.positions, &mut out, x, b, c, y);
                }
                (Some(x), Some(y), Some(z)) => {
                    out.extend_from_slice(&[a, x, z, x, b, y, z, y, c, x, y, z]);
                }
            }
        }
        m.indices = out;
    }
    m
}

/// Simplify a closed mesh by half-edge collapses ordered by quadric error (Garland–Heckbert),
/// keeping every surviving vertex where the mesher put it (on the surface, with its field
/// normal). A collapse is refused when it would leave any triangle more than `max_dev_deg`
/// away from its vertex normals (that keeps curvature and silhouettes), fold a triangle,
/// make an edge longer than `max_edge`. `weight(p)` scales the tolerances per vertex: 0 locks
/// it (material boundaries that get cut later), 1 is normal, more lets hidden surfaces
/// (wheel-well liners, pocket floors) go coarser. Collapses stop once the RMS plane distance would pass
/// `tol` meters. Flat panels end up with large triangles, creases keep their density.
///
/// `mirror_x`: the mesh is symmetric about x = 0 with no triangle crossing it; only the
/// x ≥ 0 half is simplified (its seam kept) and mirrored, so the result stays symmetric.
pub fn decimate(
    raw: &RawMesh,
    tol: f32,
    max_dev_deg: f32,
    max_edge: f32,
    mirror_x: bool,
    weight: impl Fn(Vec3) -> f32 + Sync,
) -> RawMesh {
    const SEAM: f32 = 1e-4;
    if !mirror_x {
        return decimate_core(raw, tol, max_dev_deg, max_edge, &weight);
    }
    let mut half = RawMesh::default();
    let mut remap = vec![u32::MAX; raw.positions.len()];
    for t in raw.indices.as_chunks::<3>().0.iter() {
        let c = (raw.positions[t[0] as usize]
            + raw.positions[t[1] as usize]
            + raw.positions[t[2] as usize])
            / 3.0;
        if c.x < 0.0 {
            continue;
        }
        for &i in t {
            if remap[i as usize] == u32::MAX {
                remap[i as usize] = half.positions.len() as u32;
                half.positions.push(raw.positions[i as usize]);
                half.normals.push(raw.normals[i as usize]);
            }
            half.indices.push(remap[i as usize]);
        }
    }
    let lock = |p: Vec3| if p.x.abs() < SEAM { 0.0 } else { weight(p) };
    let mut out = decimate_halves(&half, tol, max_dev_deg, max_edge, &lock);
    let n = out.positions.len();
    let mut map = vec![0u32; n];
    for (i, m) in map.iter_mut().enumerate() {
        let (p, nr) = (out.positions[i], out.normals[i]);
        if p.x.abs() < SEAM {
            *m = i as u32;
        } else {
            *m = out.positions.len() as u32;
            out.positions.push(Vec3::new(-p.x, p.y, p.z));
            out.normals.push(Vec3::new(-nr.x, nr.y, nr.z));
        }
    }
    let count = out.indices.len();
    for k in (0..count).step_by(3) {
        let t = [out.indices[k], out.indices[k + 1], out.indices[k + 2]];
        out.indices.extend_from_slice(&[
            map[t[0] as usize],
            map[t[2] as usize],
            map[t[1] as usize],
        ]);
    }
    out
}

/// Decimate the front and back halves (split at the median triangle) on two threads, the
/// vertices they share locked so the halves still meet, then weld them back together.
fn decimate_halves(
    raw: &RawMesh,
    tol: f32,
    max_dev_deg: f32,
    max_edge: f32,
    weight: &(dyn Fn(Vec3) -> f32 + Sync),
) -> RawMesh {
    let nt = raw.indices.len() / 3;
    if nt < 20_000 {
        return decimate_core(raw, tol, max_dev_deg, max_edge, weight);
    }
    let pos = &raw.positions;
    let cz: Vec<f32> = raw
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| pos[t[0] as usize].z + pos[t[1] as usize].z + pos[t[2] as usize].z)
        .collect();
    let mut sorted = cz.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let zs = sorted[nt / 2];
    // Which halves use each vertex (bit 0 front, bit 1 back).
    let mut used = vec![0u8; pos.len()];
    for (t, &c) in raw.indices.as_chunks::<3>().0.iter().zip(&cz) {
        let side = if c < zs { 1 } else { 2 };
        for &i in t {
            used[i as usize] |= side;
        }
    }
    let piece = |side: u8| -> RawMesh {
        let mut m = RawMesh::default();
        let mut remap = vec![u32::MAX; pos.len()];
        for (t, &c) in raw.indices.as_chunks::<3>().0.iter().zip(&cz) {
            if (c < zs) != (side == 1) {
                continue;
            }
            for &i in t {
                if remap[i as usize] == u32::MAX {
                    remap[i as usize] = m.positions.len() as u32;
                    m.positions.push(pos[i as usize]);
                    m.normals.push(raw.normals[i as usize]);
                }
                m.indices.push(remap[i as usize]);
            }
        }
        m
    };
    let bits = |p: Vec3| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits());
    let seam: IdMap<(u32, u32, u32), ()> = (0..pos.len())
        .filter(|&i| used[i] == 3)
        .map(|i| (bits(pos[i]), ()))
        .collect();
    let lock = |p: Vec3| {
        if seam.contains_key(&bits(p)) {
            0.0
        } else {
            weight(p)
        }
    };
    let (front, back) = (piece(1), piece(2));
    let (front, back) = std::thread::scope(|s| {
        let f = s.spawn(|| decimate_core(&front, tol, max_dev_deg, max_edge, &lock));
        let b = decimate_core(&back, tol, max_dev_deg, max_edge, &lock);
        (f.join().expect("decimation thread"), b)
    });
    // Weld: seam vertices of the back half reuse the front half's.
    let mut out = front;
    let mut at: IdMap<(u32, u32, u32), u32> = IdMap::default();
    for (i, p) in out.positions.iter().enumerate() {
        if seam.contains_key(&bits(*p)) {
            at.insert(bits(*p), i as u32);
        }
    }
    let map: Vec<u32> = back
        .positions
        .iter()
        .zip(&back.normals)
        .map(|(p, n)| match at.get(&bits(*p)) {
            Some(&i) => i,
            None => {
                out.positions.push(*p);
                out.normals.push(*n);
                out.positions.len() as u32 - 1
            }
        })
        .collect();
    out.indices
        .extend(back.indices.iter().map(|&i| map[i as usize]));
    out
}

fn decimate_core(
    raw: &RawMesh,
    tol: f32,
    max_dev_deg: f32,
    max_edge: f32,
    weight: &(dyn Fn(Vec3) -> f32 + Sync),
) -> RawMesh {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let nv = raw.positions.len();
    let pos = &raw.positions;
    let nrm = &raw.normals;
    let mut faces: Vec<[u32; 3]> = raw
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let mut alive_face = vec![true; faces.len()];
    // Quadrics (upper triangle of the symmetric 4x4) and plane counts per vertex.
    let mut quad = vec![[0.0f64; 10]; nv];
    let mut planes = vec![0u32; nv];
    let mut vf: Vec<Vec<u32>> = vec![Vec::with_capacity(7); nv];
    for (fi, f) in faces.iter().enumerate() {
        for &v in f {
            vf[v as usize].push(fi as u32);
        }
        let [a, b, c] = f.map(|i| pos[i as usize]);
        let n = (b - a).cross(c - a);
        if n.length_squared() < 1e-16 {
            continue;
        }
        let n = n.normalize().as_dvec3();
        let d = -n.dot(a.as_dvec3());
        let q = [
            n.x * n.x,
            n.x * n.y,
            n.x * n.z,
            n.x * d,
            n.y * n.y,
            n.y * n.z,
            n.y * d,
            n.z * n.z,
            n.z * d,
            d * d,
        ];
        for &v in f {
            let qv = &mut quad[v as usize];
            for k in 0..10 {
                qv[k] += q[k];
            }
            planes[v as usize] += 1;
        }
    }
    let mut wt = vec![0.0f32; nv];
    par_fill(&mut wt, |i, l| *l = weight(pos[i]));
    let tol2 = (tol as f64) * (tol as f64);
    let cos_dev = |w: f32| (max_dev_deg * w.max(1.0)).min(60.0).to_radians().cos();
    let max_e2 = max_edge * max_edge;
    let mut stamp = vec![0u32; nv];
    let mut alive_v = vec![true; nv];
    // Heap keys: cost (f32 bits, ordered for non-negative values) above an entry slot.
    let mut heap: BinaryHeap<Reverse<u64>> = BinaryHeap::with_capacity(faces.len() * 3);
    let mut entries: Vec<[u32; 4]> = Vec::with_capacity(faces.len() * 3);
    let cost_of = |quad: &[[f64; 10]], planes: &[u32], a: u32, b: u32| -> Option<(f32, u32, u32)> {
        let (qa, qb) = (&quad[a as usize], &quad[b as usize]);
        let n = (planes[a as usize] + planes[b as usize]).max(1) as f64;
        let mut best: Option<(f32, u32, u32)> = None;
        for (from, to) in [(a, b), (b, a)] {
            let w = wt[from as usize];
            if w <= 0.0 {
                continue;
            }
            let p = pos[to as usize];
            let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
            let q = |k: usize| qa[k] + qb[k];
            let e = q(0) * x * x
                + 2.0 * q(1) * x * y
                + 2.0 * q(2) * x * z
                + 2.0 * q(3) * x
                + q(4) * y * y
                + 2.0 * q(5) * y * z
                + 2.0 * q(6) * y
                + q(7) * z * z
                + 2.0 * q(8) * z
                + q(9);
            let cost = e.max(0.0) / n;
            if cost <= tol2 * (w * w) as f64 && best.is_none_or(|(c, _, _)| (cost as f32) < c) {
                best = Some((cost as f32, from, to));
            }
        }
        best
    };
    fn push(
        heap: &mut BinaryHeap<Reverse<u64>>,
        entries: &mut Vec<[u32; 4]>,
        stamp: &[u32],
        c: Option<(f32, u32, u32)>,
    ) {
        if let Some((cost, from, to)) = c {
            let slot = entries.len() as u64;
            entries.push([from, to, stamp[from as usize], stamp[to as usize]]);
            heap.push(Reverse(((cost.to_bits() as u64) << 32) | slot));
        }
    }
    for f in &faces {
        for e in 0..3 {
            let (a, b) = (f[e], f[(e + 1) % 3]);
            if a < b {
                push(
                    &mut heap,
                    &mut entries,
                    &stamp,
                    cost_of(&quad, &planes, a, b),
                );
            }
        }
    }
    let mut ring: Vec<u32> = Vec::with_capacity(16);
    let mut shared: Vec<u32> = Vec::with_capacity(4);
    while let Some(Reverse(key)) = heap.pop() {
        let [from, to, sf, st] = entries[(key & 0xffff_ffff) as usize];
        let (u, v) = (from as usize, to as usize);
        if !alive_v[u] || !alive_v[v] || stamp[u] != sf || stamp[v] != st {
            continue;
        }
        vf[u].retain(|&f| alive_face[f as usize]);
        vf[v].retain(|&f| alive_face[f as usize]);
        shared.clear();
        shared.extend(
            vf[u]
                .iter()
                .copied()
                .filter(|&f| faces[f as usize].contains(&to)),
        );
        if shared.len() != 2 {
            continue;
        }
        // Link condition: u and v share exactly the two neighbors of the shared faces.
        ring.clear();
        for &f in &vf[u] {
            ring.extend(
                faces[f as usize]
                    .iter()
                    .copied()
                    .filter(|&w| w != from && w != to),
            );
        }
        ring.sort_unstable();
        ring.dedup();
        let mut common = 0;
        for &f in &vf[v] {
            for &w in &faces[f as usize] {
                if w != to && w != from && ring.binary_search(&w).is_ok() {
                    common += 1;
                }
            }
        }
        // Each common neighbor shows up in two faces of v's fan.
        if common != 4 {
            continue;
        }
        // Check the faces that move.
        let pv = pos[v];
        let nv_ = nrm[v];
        let cos_t = cos_dev(wt[u]);
        let mut ok = true;
        for &f in &vf[u] {
            let t = faces[f as usize];
            if t.contains(&to) {
                continue;
            }
            let old = t.map(|i| pos[i as usize]);
            let new = t.map(|i| if i == from { pv } else { pos[i as usize] });
            let n_old = (old[1] - old[0]).cross(old[2] - old[0]);
            let n_new = (new[1] - new[0]).cross(new[2] - new[0]);
            let l2 = n_new.length_squared();
            if l2 < 1e-14 || n_new.dot(n_old) <= 0.0 {
                ok = false;
                break;
            }
            let n_new = n_new / l2.sqrt();
            for k in 0..3 {
                let vn = if t[k] == from {
                    nv_
                } else {
                    nrm[t[k] as usize]
                };
                if n_new.dot(vn) < cos_t || (new[(k + 1) % 3] - new[k]).length_squared() > max_e2 {
                    ok = false;
                }
            }
            if !ok {
                break;
            }
        }
        if !ok {
            continue;
        }
        // Collapse u into v.
        for &f in &shared {
            alive_face[f as usize] = false;
        }
        let moved = std::mem::take(&mut vf[u]);
        for &f in &moved {
            if !alive_face[f as usize] {
                continue;
            }
            for i in faces[f as usize].iter_mut() {
                if *i == from {
                    *i = to;
                }
            }
            vf[v].push(f);
        }
        vf[v].retain(|&f| alive_face[f as usize]);
        alive_v[u] = false;
        let qu = quad[u];
        for k in 0..10 {
            quad[v][k] += qu[k];
        }
        planes[v] += planes[u];
        stamp[v] += 1;
        // Edges around v get new costs.
        ring.clear();
        for &f in &vf[v] {
            ring.extend(faces[f as usize].iter().copied().filter(|&w| w != to));
        }
        ring.sort_unstable();
        ring.dedup();
        for &w in &ring {
            push(
                &mut heap,
                &mut entries,
                &stamp,
                cost_of(&quad, &planes, to, w),
            );
        }
    }
    // Compact.
    let mut remap = vec![u32::MAX; nv];
    let mut out = RawMesh::default();
    for (fi, f) in faces.iter().enumerate() {
        if !alive_face[fi] {
            continue;
        }
        for &i in f {
            if remap[i as usize] == u32::MAX {
                remap[i as usize] = out.positions.len() as u32;
                out.positions.push(pos[i as usize]);
                out.normals.push(nrm[i as usize]);
            }
            out.indices.push(remap[i as usize]);
        }
    }
    out
}

/// Two triangles for the quad a-b-c-d (counter-clockwise), split along the shorter diagonal.
fn quad(pos: &[Vec3], out: &mut Vec<u32>, a: u32, b: u32, c: u32, d: u32) {
    let p = |i: u32| pos[i as usize];
    if (p(a) - p(c)).length_squared() <= (p(b) - p(d)).length_squared() {
        out.extend_from_slice(&[a, b, c, a, c, d]);
    } else {
        out.extend_from_slice(&[a, b, d, b, c, d]);
    }
}

/// A mesh cut along several fields, with every field's value at every vertex (zero on the
/// cut lines), so triangles can be classified without evaluating the fields again.
pub struct CutMesh {
    pub raw: RawMesh,
    pub vals: Vec<Vec<f32>>,
}

impl CutMesh {
    /// Field `k` on triangle `t`: negative inside, positive outside (cut triangles never
    /// straddle a boundary, so the sum of the corner values has the triangle's sign).
    pub fn side(&self, k: usize, t: [u32; 3]) -> f32 {
        let v = &self.vals[k];
        v[t[0] as usize] + v[t[1] as usize] + v[t[2] as usize]
    }
}

/// Split triangles along the zero set of each field in turn, so no triangle straddles a
/// boundary: material edges and window holes then follow smooth curves instead of the
/// mesher's staircase. New vertices sit on the original triangles (normals interpolated).
pub fn cut(raw: &RawMesh, fields: &[&(dyn Fn(Vec3) -> f32 + Sync)]) -> CutMesh {
    let mut m = raw.clone();
    let mut all: Vec<Vec<f32>> = Vec::new();
    for (fi, f) in fields.iter().enumerate() {
        let mut vals = vec![0.0f32; m.positions.len()];
        let pos = &m.positions;
        par_fill(&mut vals, |i, v| *v = f(pos[i]));
        let mut cache: IdMap<(u32, u32), u32> = IdMap::default();
        let mut out = Vec::with_capacity(m.indices.len() + m.indices.len() / 8);
        let tris = std::mem::take(&mut m.indices);
        for t in tris.as_chunks::<3>().0.iter() {
            let neg = [
                vals[t[0] as usize] < 0.0,
                vals[t[1] as usize] < 0.0,
                vals[t[2] as usize] < 0.0,
            ];
            if neg[0] == neg[1] && neg[1] == neg[2] {
                out.extend_from_slice(t);
                continue;
            }
            // Rotate so vertex 0 is the one alone on its side.
            let lone = if neg[1] == neg[2] {
                0
            } else if neg[0] == neg[2] {
                1
            } else {
                2
            };
            let p = t[lone];
            let q = t[(lone + 1) % 3];
            let r = t[(lone + 2) % 3];
            let mut mid = |a: u32, b: u32| -> u32 {
                let k = (a.min(b), a.max(b));
                if let Some(&v) = cache.get(&k) {
                    return v;
                }
                let (va, vb) = (vals[a as usize], vals[b as usize]);
                let s = (va / (va - vb)).clamp(0.0, 1.0);
                let (a, b) = (a as usize, b as usize);
                let pa = m.positions[a];
                let pb = m.positions[b];
                let na = m.normals[a];
                let nb = m.normals[b];
                m.positions.push(pa.lerp(pb, s));
                m.normals.push(na.lerp(nb, s).normalize_or(na));
                vals.push(0.0);
                for earlier in all.iter_mut().take(fi) {
                    let v = earlier[a] + (earlier[b] - earlier[a]) * s;
                    earlier.push(v);
                }
                let id = (m.positions.len() - 1) as u32;
                cache.insert(k, id);
                id
            };
            let m1 = mid(p, q);
            let m2 = mid(p, r);
            out.extend_from_slice(&[p, m1, m2, m1, q, r, m1, r, m2]);
        }
        m.indices = out;
        all.push(vals);
    }
    CutMesh { raw: m, vals: all }
}

/// Ambient occlusion from the field itself: how much nearby geometry crowds the normal.
/// 1 = open, toward 0 in creases. `reach` is the farthest probe, meters.
pub fn field_ao(f: Field, p: Vec3, n: Vec3, reach: f32) -> f32 {
    let mut occ = 0.0;
    let mut w = 1.0;
    for i in 1..=4 {
        let t = reach * i as f32 / 4.0;
        let d = f(p + n * t);
        occ += w * (t - d).max(0.0) / t;
        w *= 0.6;
    }
    (1.0 - occ * 0.55).clamp(0.0, 1.0)
}

/// What one triangle becomes: which output mesh, and a UV mode the vertex function sees
/// (vertices are split where the mode changes, e.g. between planar projections).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assign {
    pub mesh: usize,
    pub mode: u8,
}

/// Split a raw mesh into `outputs` meshes. `classify(centroid, face_normal, corners)` picks
/// each triangle's output (`None` drops it); `vertex(mode, index, position, normal)` makes
/// the final vertex (UV, color).
pub fn split(
    raw: &RawMesh,
    outputs: usize,
    classify: impl Fn(Vec3, Vec3, [u32; 3]) -> Option<Assign>,
    vertex: impl Fn(Assign, usize, Vec3, Vec3) -> Vertex,
) -> Vec<Mesh> {
    let mut out: Vec<Mesh> = (0..outputs).map(|_| Mesh::default()).collect();
    let mut maps: Vec<IdMap<u64, u32>> = (0..outputs).map(|_| IdMap::default()).collect();
    for t in raw.indices.as_chunks::<3>().0.iter() {
        let p = [
            raw.positions[t[0] as usize],
            raw.positions[t[1] as usize],
            raw.positions[t[2] as usize],
        ];
        let fnrm = (p[1] - p[0]).cross(p[2] - p[0]);
        if fnrm.length_squared() < 1e-14 {
            continue;
        }
        let centroid = (p[0] + p[1] + p[2]) / 3.0;
        let Some(a) = classify(centroid, fnrm.normalize(), [t[0], t[1], t[2]]) else {
            continue;
        };
        let m = &mut out[a.mesh];
        let map = &mut maps[a.mesh];
        for &vi in t {
            let key = ((vi as u64) << 8) | a.mode as u64;
            let id = *map.entry(key).or_insert_with(|| {
                let v = vertex(
                    a,
                    vi as usize,
                    raw.positions[vi as usize],
                    raw.normals[vi as usize],
                );
                m.vertices.push(v);
                (m.vertices.len() - 1) as u32
            });
            m.indices.push(id);
        }
    }
    out
}

/// A plain vertex with a gray `ao` baked into the vertex color.
pub fn vtx(p: Vec3, n: Vec3, uv: Vec2, ao: f32) -> Vertex {
    Vertex::new(p, n, uv).with_color(Vec4::new(ao, ao, ao, 1.0))
}

/// Reverse every triangle and normal (an inside-out surface, like a cabin seen from within).
pub fn flip(raw: &mut RawMesh) {
    for t in raw.indices.as_chunks_mut::<3>().0.iter_mut() {
        t.swap(1, 2);
    }
    for n in &mut raw.normals {
        *n = -*n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_meshes_closed_on_the_surface() {
        let sphere = |p: Vec3| (p - Vec3::new(0.0, 0.1, 0.0)).length() - 0.5;
        for mirror in [false, true] {
            let m = surface_nets(
                &sphere,
                Vec3::splat(-0.7),
                Vec3::splat(0.7),
                0.05,
                mirror,
                1.0,
            );
            assert!(m.indices.len() > 300, "{}", m.indices.len());
            for (p, n) in m.positions.iter().zip(&m.normals) {
                let r = (*p - Vec3::new(0.0, 0.1, 0.0)).length();
                assert!((r - 0.5).abs() < 0.01, "{p} r={r}");
                let radial = (*p - Vec3::new(0.0, 0.1, 0.0)).normalize();
                assert!(n.dot(radial) > 0.95, "normal {n} at {p}");
            }
            // Triangles face outward.
            for t in m.indices.as_chunks::<3>().0.iter() {
                let [a, b, c] = [0, 1, 2].map(|i| m.positions[t[i] as usize]);
                let fnrm = (b - a).cross(c - a);
                assert!(fnrm.dot(a - Vec3::new(0.0, 0.1, 0.0)) > 0.0);
            }
            // Closed: every edge used twice.
            let mut edges = HashMap::new();
            for t in m.indices.as_chunks::<3>().0.iter() {
                for e in 0..3 {
                    let (a, b) = (t[e], t[(e + 1) % 3]);
                    *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
            assert!(
                edges.values().all(|&c| c == 2),
                "mirror {mirror}: open edges"
            );
        }
    }
}
