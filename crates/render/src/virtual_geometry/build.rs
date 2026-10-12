//! Building a [`ClusterDag`] from a mesh: clusters, groups, simplification, levels.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use engine_core::math::Vec3;

use super::simplify::simplify;
use super::{
    CLUSTER_MAX_VERTICES, CLUSTER_TRIANGLES, Cluster, ClusterDag, ClusterLevel, GROUP_SIZE,
    ROOT_ERROR, Sphere,
};
use crate::mesh::{Mesh, Vertex};

/// A group counts as simplified when it kept at most this share of its triangles; otherwise
/// its clusters carry over to the next level unchanged (and are grouped differently there).
const MAX_KEPT: f32 = 0.85;
/// Stop when a whole level removes less than this share of the triangles.
const MIN_LEVEL_REDUCTION: f32 = 0.01;
/// Safety stop (a mesh halves ~20 times from a million triangles to one cluster).
const MAX_LEVELS: u32 = 48;

/// A cluster while the DAG is being built.
struct Work {
    triangles: Vec<[u32; 3]>,
    level: u32,
    error: f32,
    lod_bounds: Sphere,
    parent_error: f32,
    parent_bounds: Sphere,
}

/// The mesh with identical vertices merged, its positions welded and degenerate triangles
/// removed.
struct Prepared {
    vertices: Vec<Vertex>,
    /// Position of each vertex.
    positions: Vec<Vec3>,
    /// Welded position id of each vertex (vertices at the same place share one).
    pos_id: Vec<u32>,
    position_count: usize,
    triangles: Vec<[u32; 3]>,
}

fn prepare(mesh: &Mesh) -> Prepared {
    let mut by_bits: HashMap<[u32; 12], u32> = HashMap::with_capacity(mesh.vertices.len());
    let mut remap = vec![u32::MAX; mesh.vertices.len()];
    let mut vertices: Vec<Vertex> = Vec::with_capacity(mesh.vertices.len());
    let mut triangles = Vec::with_capacity(mesh.indices.len() / 3);
    for tri in mesh.indices.as_chunks::<3>().0 {
        if tri.iter().any(|&i| i as usize >= mesh.vertices.len()) {
            continue;
        }
        let t = tri.map(|i| {
            let i = i as usize;
            if remap[i] == u32::MAX {
                let key: [u32; 12] = bytemuck::cast(mesh.vertices[i]);
                remap[i] = *by_bits.entry(key).or_insert_with(|| {
                    vertices.push(mesh.vertices[i]);
                    vertices.len() as u32 - 1
                });
            }
            remap[i]
        });
        triangles.push(t);
    }
    let positions: Vec<Vec3> = vertices.iter().map(|v| Vec3::from(v.position)).collect();
    let mut by_position: HashMap<[u32; 3], u32> = HashMap::with_capacity(vertices.len());
    let pos_id: Vec<u32> = vertices
        .iter()
        .map(|v| {
            let n = by_position.len() as u32;
            *by_position.entry(bytemuck::cast(v.position)).or_insert(n)
        })
        .collect();
    // Triangles with two corners at one place have no area and no well-defined edges.
    triangles.retain(|t| {
        let [a, b, c] = t.map(|i| pos_id[i as usize]);
        a != b && b != c && a != c
    });
    Prepared {
        vertices,
        positions,
        pos_id,
        position_count: by_position.len(),
        triangles,
    }
}

/// Build the cluster hierarchy of a mesh. Pure CPU work, deterministic, and slow for big
/// meshes (about a second per million triangles in release builds): the renderer runs it on a
/// background thread.
pub fn build(mesh: &Mesh) -> ClusterDag {
    let p = prepare(mesh);
    let mut work: Vec<Work> = split_triangles(&p.triangles, &p.positions)
        .into_iter()
        .map(|triangles| {
            let bounds = Sphere::around_points(corners(&triangles, &p.positions));
            Work {
                triangles,
                level: 0,
                error: 0.0,
                lod_bounds: bounds,
                parent_error: ROOT_ERROR,
                parent_bounds: bounds,
            }
        })
        .collect();

    let mut current: Vec<usize> = (0..work.len()).collect();
    let mut level = 0;
    let mut pos_group = vec![u32::MAX; p.position_count];
    let mut shared = vec![false; p.position_count];
    while current.len() > 1 && level < MAX_LEVELS {
        let groups = group_clusters(&current, &work, &p);
        // Positions used by more than one group: the group borders, which stay locked.
        pos_group.fill(u32::MAX);
        shared.fill(false);
        for (g, members) in groups.iter().enumerate() {
            for &m in members {
                for tri in &work[current[m as usize]].triangles {
                    for &v in tri {
                        let id = p.pos_id[v as usize] as usize;
                        if pos_group[id] == u32::MAX {
                            pos_group[id] = g as u32;
                        } else if pos_group[id] != g as u32 {
                            shared[id] = true;
                        }
                    }
                }
            }
        }
        let results = parallel_map(&groups, |members| {
            let tris: Vec<[u32; 3]> = members
                .iter()
                .flat_map(|&m| work[current[m as usize]].triangles.iter().copied())
                .collect();
            simplify_group(&tris, &p, &shared)
                .map(|(simplified, error)| (split_triangles(&simplified, &p.positions), error))
        });

        let before: usize = current.iter().map(|&c| work[c].triangles.len()).sum();
        let mut next = Vec::with_capacity(current.len() / 2 + 1);
        for (members, result) in groups.iter().zip(results) {
            let children: Vec<usize> = members.iter().map(|&m| current[m as usize]).collect();
            let Some((clusters, error)) = result else {
                next.extend(children);
                continue;
            };
            // Errors add up level by level (a bound on the distance to the original
            // surface), and the group's sphere encloses its children's: both only grow.
            let child_error = children.iter().map(|&c| work[c].error).fold(0.0, f32::max);
            let group_error = error + child_error;
            let group_bounds = Sphere::enclosing(children.iter().map(|&c| work[c].lod_bounds));
            for &c in &children {
                work[c].parent_error = group_error;
                work[c].parent_bounds = group_bounds;
            }
            for triangles in clusters {
                next.push(work.len());
                work.push(Work {
                    triangles,
                    level: level + 1,
                    error: group_error,
                    lod_bounds: group_bounds,
                    parent_error: ROOT_ERROR,
                    parent_bounds: group_bounds,
                });
            }
        }
        let after: usize = next.iter().map(|&c| work[c].triangles.len()).sum();
        let progressed = next.len() != current.len() || after != before;
        current = next;
        level += 1;
        if !progressed || (after as f32) > before as f32 * (1.0 - MIN_LEVEL_REDUCTION) {
            break;
        }
    }

    finish(p, work)
}

/// Pack the clusters, compute their bounds and normal cones, and index the levels.
fn finish(p: Prepared, mut work: Vec<Work>) -> ClusterDag {
    // Creation order is level order, except that clusters carried over keep their level:
    // sort stably by level so each level is contiguous.
    work.sort_by_key(|w| w.level);
    let mut dag = ClusterDag {
        source_triangles: p.triangles.len() as u32,
        ..Default::default()
    };
    let mut used = vec![u32::MAX; p.vertices.len()];
    let mut refs: Vec<u32> = Vec::with_capacity(CLUSTER_MAX_VERTICES);
    for w in &work {
        refs.clear();
        for tri in &w.triangles {
            refs.extend_from_slice(tri);
        }
        refs.sort_unstable();
        refs.dedup();
        debug_assert!(refs.len() <= CLUSTER_MAX_VERTICES);
        // Final vertex numbering in order of first use.
        let refs_final: Vec<u32> = refs
            .iter()
            .map(|&v| {
                if used[v as usize] == u32::MAX {
                    used[v as usize] = dag.vertices.len() as u32;
                    dag.vertices.push(p.vertices[v as usize]);
                }
                used[v as usize]
            })
            .collect();
        let index_offset = dag.indices.len() as u32;
        dag.indices.extend_from_slice(&refs_final);
        for tri in &w.triangles {
            let local = tri.map(|v| refs.binary_search(&v).unwrap_or(0) as u32);
            dag.indices
                .push(local[0] | (local[1] << 8) | (local[2] << 16));
        }
        let bounds = Sphere::around_points(refs.iter().map(|&v| p.positions[v as usize]));
        let (cone_axis, cone_cos) = normal_cone(&w.triangles, &p.positions);
        dag.clusters.push(Cluster {
            index_offset,
            vertex_count: refs.len() as u32,
            triangle_count: w.triangles.len() as u32,
            level: w.level,
            bounds,
            cone_axis,
            cone_cos,
            error: w.error,
            lod_bounds: w.lod_bounds,
            parent_error: w.parent_error,
            parent_bounds: w.parent_bounds,
        });
    }
    let mut first = 0;
    while first < dag.clusters.len() {
        let level = dag.clusters[first].level;
        let mut end = first;
        let (mut min_error, mut max_parent_error) = (f32::MAX, 0.0f32);
        while end < dag.clusters.len() && dag.clusters[end].level == level {
            min_error = min_error.min(dag.clusters[end].error);
            max_parent_error = max_parent_error.max(dag.clusters[end].parent_error);
            end += 1;
        }
        dag.levels.push(ClusterLevel {
            first: first as u32,
            count: (end - first) as u32,
            min_error,
            max_parent_error,
        });
        first = end;
    }
    dag.bounds = Sphere::enclosing(
        dag.clusters
            .iter()
            .flat_map(|c| [c.bounds, c.lod_bounds, c.parent_bounds]),
    );
    dag
}

fn corners<'a>(
    triangles: &'a [[u32; 3]],
    positions: &'a [Vec3],
) -> impl Iterator<Item = Vec3> + Clone + 'a {
    triangles
        .iter()
        .flat_map(move |t| t.iter().map(move |&v| positions[v as usize]))
}

/// Axis and cosine of the cone around every triangle normal (`cos <= 0`: no useful cone).
fn normal_cone(triangles: &[[u32; 3]], positions: &[Vec3]) -> (Vec3, f32) {
    let normal = |t: &[u32; 3]| {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        (b - a).cross(c - a)
    };
    let axis = triangles
        .iter()
        .map(normal)
        .sum::<Vec3>()
        .normalize_or_zero();
    if axis == Vec3::ZERO {
        return (Vec3::Z, -1.0);
    }
    let cos = triangles
        .iter()
        .filter_map(|t| {
            let n = normal(t);
            (n.length_squared() > 0.0).then(|| axis.dot(n.normalize()))
        })
        .fold(1.0, f32::min);
    (axis, cos)
}

/// Split triangles into spatially compact clusters of at most [`CLUSTER_TRIANGLES`]
/// triangles and [`CLUSTER_MAX_VERTICES`] vertices: recursive median splits along the longest
/// axis, sized so the clusters come out evenly filled.
fn split_triangles(triangles: &[[u32; 3]], positions: &[Vec3]) -> Vec<Vec<[u32; 3]>> {
    let centroids: Vec<Vec3> = triangles
        .iter()
        .map(|t| t.iter().map(|&v| positions[v as usize]).sum::<Vec3>() / 3.0)
        .collect();
    let mut order: Vec<u32> = (0..triangles.len() as u32).collect();
    let mut out = Vec::with_capacity(triangles.len() / (CLUSTER_TRIANGLES / 2) + 1);
    let mut scratch = Vec::new();
    split_recursive(&mut order, &centroids, triangles, &mut scratch, &mut out);
    out
}

fn split_recursive(
    items: &mut [u32],
    centroids: &[Vec3],
    triangles: &[[u32; 3]],
    scratch: &mut Vec<u32>,
    out: &mut Vec<Vec<[u32; 3]>>,
) {
    let n = items.len();
    if n == 0 {
        return;
    }
    if n <= CLUSTER_TRIANGLES {
        scratch.clear();
        for &t in items.iter() {
            scratch.extend_from_slice(&triangles[t as usize]);
        }
        scratch.sort_unstable();
        scratch.dedup();
        if scratch.len() <= CLUSTER_MAX_VERTICES || n == 1 {
            out.push(items.iter().map(|&t| triangles[t as usize]).collect());
            return;
        }
    }
    let k = n.div_ceil(CLUSTER_TRIANGLES).max(2);
    let left = ((n * (k / 2) + k / 2) / k).clamp(1, n - 1);
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for &t in items.iter() {
        lo = lo.min(centroids[t as usize]);
        hi = hi.max(centroids[t as usize]);
    }
    let e = hi - lo;
    let axis = if e.x >= e.y && e.x >= e.z {
        0
    } else if e.y >= e.z {
        1
    } else {
        2
    };
    items.select_nth_unstable_by(left, |&a, &b| {
        centroids[a as usize][axis].total_cmp(&centroids[b as usize][axis])
    });
    let (l, r) = items.split_at_mut(left);
    split_recursive(l, centroids, triangles, scratch, out);
    split_recursive(r, centroids, triangles, scratch, out);
}

/// Partition the clusters `current` (indices into `work`) into groups of about [`GROUP_SIZE`]
/// that share as many edges as possible. Returns indices into `current`.
///
/// Greedy graph growing on the cluster adjacency graph, weighted by shared edges: a group
/// starts at the free cluster with the fewest free neighbors (so no pockets are left behind)
/// and repeatedly takes the free neighbor most connected to it. Borders a level locked are
/// lines of short edges, which connect their two sides strongly, so the next level's groups
/// tend to straddle them and they get simplified after all.
fn group_clusters(current: &[usize], work: &[Work], p: &Prepared) -> Vec<Vec<u32>> {
    let n = current.len();
    if n <= GROUP_SIZE {
        return vec![(0..n as u32).collect()];
    }

    // Boundary edges of every cluster (by welded position), then clusters sharing them.
    let mut boundary: Vec<(u64, u32)> = Vec::new();
    let mut edges: Vec<u64> = Vec::new();
    for (i, &c) in current.iter().enumerate() {
        edges.clear();
        for tri in &work[c].triangles {
            for k in 0..3 {
                let a = p.pos_id[tri[k] as usize];
                let b = p.pos_id[tri[(k + 1) % 3] as usize];
                edges.push(((a.min(b) as u64) << 32) | a.max(b) as u64);
            }
        }
        edges.sort_unstable();
        let mut j = 0;
        while j < edges.len() {
            let mut k = j;
            while k < edges.len() && edges[k] == edges[j] {
                k += 1;
            }
            if k - j == 1 {
                boundary.push((edges[j], i as u32));
            }
            j = k;
        }
    }
    boundary.sort_unstable();
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let mut j = 0;
    while j < boundary.len() {
        let mut k = j;
        while k < boundary.len() && boundary[k].0 == boundary[j].0 {
            k += 1;
        }
        for a in j..k {
            for b in a + 1..k {
                let (x, y) = (boundary[a].1, boundary[b].1);
                if x != y {
                    pairs.push((x.min(y), x.max(y)));
                }
            }
        }
        j = k;
    }
    pairs.sort_unstable();
    let mut adjacency: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    let mut j = 0;
    while j < pairs.len() {
        let mut k = j;
        while k < pairs.len() && pairs[k] == pairs[j] {
            k += 1;
        }
        let (a, b) = pairs[j];
        let w = (k - j) as u32;
        adjacency[a as usize].push((b, w));
        adjacency[b as usize].push((a, w));
        j = k;
    }

    let centroid: Vec<Vec3> = current
        .iter()
        .map(|&c| {
            let t = &work[c].triangles;
            corners(t, &p.positions).sum::<Vec3>() / (t.len() * 3).max(1) as f32
        })
        .collect();
    let rank = morton_ranks(&centroid);

    let mut group_of = vec![u32::MAX; n];
    let mut free_neighbors: Vec<u32> = adjacency.iter().map(|a| a.len() as u32).collect();
    let mut heap: BinaryHeap<Reverse<(u32, u32, u32)>> = (0..n as u32)
        .map(|i| Reverse((free_neighbors[i as usize], rank[i as usize], i)))
        .collect();
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut frontier: Vec<(u32, u32)> = Vec::new();
    while let Some(Reverse((count, _, seed))) = heap.pop() {
        if group_of[seed as usize] != u32::MAX || count != free_neighbors[seed as usize] {
            continue; // assigned, or a stale entry
        }
        let g = groups.len() as u32;
        let mut members = Vec::with_capacity(GROUP_SIZE);
        frontier.clear();
        let mut next = Some(seed);
        while let Some(c) = next {
            group_of[c as usize] = g;
            members.push(c);
            frontier.retain(|&(f, _)| f != c);
            for &(nb, w) in &adjacency[c as usize] {
                if group_of[nb as usize] != u32::MAX {
                    continue;
                }
                free_neighbors[nb as usize] -= 1;
                heap.push(Reverse((
                    free_neighbors[nb as usize],
                    rank[nb as usize],
                    nb,
                )));
                match frontier.iter_mut().find(|(f, _)| *f == nb) {
                    Some(entry) => entry.1 += w,
                    None => frontier.push((nb, w)),
                }
            }
            if members.len() >= GROUP_SIZE {
                break;
            }
            let origin = centroid[seed as usize];
            next = frontier
                .iter()
                .max_by(|a, b| {
                    a.1.cmp(&b.1).then_with(|| {
                        let da = centroid[a.0 as usize].distance_squared(origin);
                        let db = centroid[b.0 as usize].distance_squared(origin);
                        db.total_cmp(&da)
                    })
                })
                .map(|&(c, _)| c);
        }
        groups.push(members);
    }

    // Fold small leftover groups into a neighbor (or the nearest group) with room.
    let max_size = GROUP_SIZE + GROUP_SIZE / 2;
    for g in 0..groups.len() {
        let size = groups[g].len();
        if size == 0 || size * 2 >= GROUP_SIZE {
            continue;
        }
        let mut links: HashMap<u32, u32> = HashMap::new();
        for &c in &groups[g] {
            for &(nb, w) in &adjacency[c as usize] {
                let h = group_of[nb as usize];
                if h != g as u32 && groups[h as usize].len() + size <= max_size {
                    *links.entry(h).or_insert(0) += w;
                }
            }
        }
        let target = links
            .into_iter()
            .max_by_key(|&(h, w)| (w, Reverse(h)))
            .map(|(h, _)| h)
            .or_else(|| {
                let here = centroid[groups[g][0] as usize];
                (0..groups.len())
                    .filter(|&h| h != g && !groups[h].is_empty())
                    .filter(|&h| groups[h].len() + size <= max_size)
                    .min_by(|&a, &b| {
                        let da = centroid[groups[a][0] as usize].distance_squared(here);
                        let db = centroid[groups[b][0] as usize].distance_squared(here);
                        da.total_cmp(&db)
                    })
                    .map(|h| h as u32)
            });
        if let Some(h) = target {
            let moved = std::mem::take(&mut groups[g]);
            for &c in &moved {
                group_of[c as usize] = h;
            }
            groups[h as usize].extend(moved);
        }
    }
    groups.retain(|g| !g.is_empty());
    groups
}

/// Rank of each point along a Z-order curve through their bounding box.
fn morton_ranks(points: &[Vec3]) -> Vec<u32> {
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for &q in points {
        lo = lo.min(q);
        hi = hi.max(q);
    }
    let size = (hi - lo).max(Vec3::splat(1e-12));
    let spread = |v: u32| {
        let mut x = v as u64 & 0x1f_ffff;
        x = (x | (x << 32)) & 0x1f_0000_0000_ffff;
        x = (x | (x << 16)) & 0x1f_0000_ff00_00ff;
        x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
        x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
        (x | (x << 2)) & 0x1249_2492_4924_9249
    };
    let codes: Vec<u64> = points
        .iter()
        .map(|&q| {
            let u = ((q - lo) / size * 2_097_151.0).as_uvec3();
            spread(u.x) | (spread(u.y) << 1) | (spread(u.z) << 2)
        })
        .collect();
    let mut order: Vec<u32> = (0..points.len() as u32).collect();
    order.sort_by_key(|&i| (codes[i as usize], i));
    let mut rank = vec![0; points.len()];
    for (r, &i) in order.iter().enumerate() {
        rank[i as usize] = r as u32;
    }
    rank
}

/// Simplify one group to half its triangles, keeping the positions it shares with other
/// groups (`shared`) in place. `None` when it can't be reduced enough.
fn simplify_group(
    triangles: &[[u32; 3]],
    p: &Prepared,
    shared: &[bool],
) -> Option<(Vec<[u32; 3]>, f32)> {
    let mut ids: Vec<u32> = triangles.iter().flatten().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let local = |v: u32| ids.binary_search(&v).unwrap_or(0) as u32;
    let positions: Vec<Vec3> = ids.iter().map(|&v| p.positions[v as usize]).collect();
    let locked: Vec<bool> = ids
        .iter()
        .map(|&v| shared[p.pos_id[v as usize] as usize])
        .collect();
    let local_tris: Vec<[u32; 3]> = triangles.iter().map(|t| t.map(local)).collect();
    let out = simplify(&positions, &local_tris, &locked, triangles.len() / 2);
    if out.triangles.len() as f32 > triangles.len() as f32 * MAX_KEPT {
        return None;
    }
    Some((
        out.triangles
            .iter()
            .map(|t| t.map(|i| ids[i as usize]))
            .collect(),
        out.error,
    ))
}

/// `f` over `items` on up to four threads, results in order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 4);
    if threads == 1 || items.len() < 16 {
        return items.iter().map(&f).collect();
    }
    let chunk = items.len().div_ceil(threads);
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(f).collect::<Vec<R>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::super::{ROOT_ERROR, projected_error};
    use super::*;
    use crate::mesh::icosphere;

    /// A closed, lumpy rock: a subdivided icosphere pushed in and out by smooth noise.
    pub(crate) fn rock(subdivisions: u32) -> Mesh {
        let mut m = icosphere(1.0, subdivisions);
        for v in &mut m.vertices {
            let p = Vec3::from(v.position);
            let bumps = 0.18 * (p.x * 3.1 + 0.4).sin() * (p.y * 2.7).cos()
                + 0.07 * (p.z * 7.3 + p.x * 5.1).sin()
                + 0.03 * (p.y * 17.0 - p.z * 13.0).sin();
            v.position = (p * (1.0 + bumps) * Vec3::new(1.3, 0.8, 1.0)).to_array();
        }
        m.compute_normals();
        m
    }

    /// Every edge (by welded position) is used exactly once in each direction: the
    /// triangles form a closed surface without cracks or overlaps.
    fn assert_watertight(dag: &ClusterDag, clusters: &[usize], what: &str) {
        let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
        let pid: Vec<u32> = dag
            .vertices
            .iter()
            .map(|v| {
                let n = ids.len() as u32;
                *ids.entry(bytemuck::cast(v.position)).or_insert(n)
            })
            .collect();
        let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
        for &c in clusters {
            for t in dag.triangles(c) {
                for k in 0..3 {
                    let e = (pid[t[k] as usize], pid[t[(k + 1) % 3] as usize]);
                    *directed.entry(e).or_insert(0) += 1;
                }
            }
        }
        let mut bad = 0;
        for (&(a, b), &count) in &directed {
            if count != 1 || directed.get(&(b, a)) != Some(&1) {
                bad += 1;
            }
        }
        assert_eq!(
            bad,
            0,
            "{what}: {bad} of {} edges are cracked or doubled",
            directed.len()
        );
    }

    fn area(dag: &ClusterDag, clusters: &[usize]) -> f32 {
        clusters
            .iter()
            .flat_map(|&c| dag.triangles(c))
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from(dag.vertices[i as usize].position));
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    #[test]
    fn clusters_respect_their_limits_and_bounds() {
        let mesh = rock(5);
        let dag = build(&mesh);
        assert_eq!(dag.source_triangles as usize, mesh.triangle_count());
        assert!(dag.levels.len() >= 5, "only {} levels", dag.levels.len());
        let leaves: u32 = dag
            .clusters
            .iter()
            .filter(|c| c.level == 0)
            .map(|c| c.triangle_count)
            .sum();
        assert_eq!(leaves, dag.source_triangles, "level 0 is the whole mesh");
        for (i, c) in dag.clusters.iter().enumerate() {
            assert!(
                c.triangle_count as usize <= CLUSTER_TRIANGLES,
                "cluster {i}: {c:?}"
            );
            assert!(
                c.vertex_count as usize <= CLUSTER_MAX_VERTICES,
                "cluster {i}: {c:?}"
            );
            assert!(c.triangle_count > 0);
            for t in dag.triangles(i) {
                let [a, b, cc] = t.map(|v| Vec3::from(dag.vertices[v as usize].position));
                for q in [a, b, cc] {
                    assert!(q.distance(c.bounds.center) <= c.bounds.radius * 1.0001 + 1e-6);
                }
                let n = (b - a).cross(cc - a);
                if n.length_squared() > 1e-20 && c.cone_cos > 0.0 {
                    assert!(
                        n.normalize().dot(c.cone_axis) >= c.cone_cos - 1e-4,
                        "cluster {i}: a triangle normal is outside its cone"
                    );
                }
            }
            assert!(dag.bounds.contains(&c.bounds) && dag.bounds.contains(&c.parent_bounds));
        }
        // Most leaves are full, and most clusters can be culled by their normal cone.
        let level0 = dag.clusters.iter().filter(|c| c.level == 0);
        let average = leaves as f32 / level0.clone().count() as f32;
        assert!(average > 90.0, "leaves hold {average} triangles on average");
        let coned = level0.clone().filter(|c| c.cone_cos > 0.5).count();
        assert!(coned * 10 > level0.count() * 8, "normal cones are too wide");
        let roots: Vec<usize> = dag.roots().collect();
        let root_tris: u32 = roots.iter().map(|&r| dag.clusters[r].triangle_count).sum();
        assert!(
            root_tris <= 512,
            "the coarsest cut still has {root_tris} triangles"
        );
    }

    #[test]
    fn errors_and_bounds_grow_toward_the_root() {
        let dag = build(&rock(5));
        for (i, c) in dag.clusters.iter().enumerate() {
            assert!(c.error >= 0.0);
            assert!(
                c.error <= c.parent_error,
                "cluster {i}: error {} above its parent's {}",
                c.error,
                c.parent_error
            );
            assert!(
                c.parent_bounds.contains(&c.lod_bounds),
                "cluster {i}: parent sphere {:?} doesn't contain {:?}",
                c.parent_bounds,
                c.lod_bounds
            );
            if c.level == 0 {
                assert_eq!(c.error, 0.0);
            } else {
                assert!(
                    c.error > 0.0,
                    "cluster {i} at level {} has no error",
                    c.level
                );
            }
            if c.parent_error < ROOT_ERROR {
                // Its parents exist: clusters whose own error and sphere are this parent's.
                let parents = dag
                    .clusters
                    .iter()
                    .filter(|p| p.error == c.parent_error && p.lod_bounds == c.parent_bounds)
                    .count();
                assert!(parents > 0, "cluster {i} has no parent cluster");
            }
        }
        for l in &dag.levels {
            let cl = &dag.clusters[l.first as usize..(l.first + l.count) as usize];
            assert!(
                cl.iter()
                    .all(|c| c.error >= l.min_error && c.parent_error <= l.max_parent_error)
            );
        }
    }

    #[test]
    fn every_level_covers_the_surface() {
        let mesh = rock(5);
        let dag = build(&mesh);
        let full = area(&dag, &dag.select(None, 1.0, 0.0));
        let mut thresholds: Vec<f32> = dag.clusters.iter().map(|c| c.error).collect();
        thresholds.sort_by(f32::total_cmp);
        thresholds.dedup();
        let mut previous = usize::MAX;
        let mut cuts = 0;
        for &t in thresholds.iter().step_by((thresholds.len() / 12).max(1)) {
            let cut = dag.select(None, 1.0, t);
            assert!(!cut.is_empty());
            assert_watertight(&dag, &cut, &format!("error threshold {t}"));
            let triangles: usize = cut
                .iter()
                .map(|&c| dag.clusters[c].triangle_count as usize)
                .sum();
            assert!(triangles <= previous, "a coarser cut has more triangles");
            previous = triangles;
            let a = area(&dag, &cut);
            assert!(
                (a - full).abs() < full * 0.25,
                "cut at {t} covers {a} of {full} m²"
            );
            cuts += 1;
        }
        assert!(cuts >= 5);
        // The finest cut is exactly the original mesh.
        let finest: u32 = dag
            .select(None, 1.0, 0.0)
            .iter()
            .map(|&c| dag.clusters[c].triangle_count)
            .sum();
        assert_eq!(finest, dag.source_triangles);
    }

    #[test]
    fn mixed_levels_do_not_crack() {
        // A rock ridge 16 m long: seen from near one end, fine clusters there, coarse ones
        // toward the far end.
        let mut ridge = rock(5);
        for v in &mut ridge.vertices {
            v.position[0] *= 6.0;
        }
        ridge.compute_normals();
        let dag = build(&ridge);
        for eye in [
            Vec3::new(10.0, 0.5, 0.0),
            Vec3::new(-6.0, 2.5, 2.5),
            Vec3::new(0.0, -1.0, 3.0),
        ] {
            let mut most_levels = 0;
            for threshold in [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0] {
                let cut = dag.select(Some(eye), 300.0, threshold);
                let levels: std::collections::BTreeSet<u32> =
                    cut.iter().map(|&c| dag.clusters[c].level).collect();
                most_levels = most_levels.max(levels.len());
                assert_watertight(&dag, &cut, &format!("{threshold} px from {eye}"));
                for &c in &cut {
                    let cl = &dag.clusters[c];
                    assert!(
                        projected_error(cl.error, cl.lod_bounds, Some(eye), 300.0) <= threshold
                    );
                }
            }
            assert!(
                most_levels >= 3,
                "never more than {most_levels} levels from {eye}"
            );
        }
    }

    #[test]
    fn open_meshes_and_seams_keep_their_borders() {
        // A plane (open border) and a UV sphere (seam meridian, poles).
        let plane = {
            let mut m = crate::mesh::plane(engine_core::math::Vec2::splat(4.0), 60);
            for v in &mut m.vertices {
                v.position[1] = 0.1 * (v.position[0] * 3.0).sin() * (v.position[2] * 2.0).cos();
            }
            m
        };
        let dag = build(&plane);
        assert!(dag.levels.len() >= 3);
        for t in [0.0, 1e-4, 1e-3, 1e-2, 1.0] {
            let cut = dag.select(None, 1.0, t);
            // The outline of the plane never changes, and inside it every edge is shared.
            let mut count: HashMap<(u32, u32), i32> = HashMap::new();
            for &c in &cut {
                for tri in dag.triangles(c) {
                    for k in 0..3 {
                        let (a, b) = (tri[k], tri[(k + 1) % 3]);
                        *count.entry((a, b)).or_insert(0) += 1;
                        *count.entry((b, a)).or_insert(0) -= 1;
                    }
                }
            }
            let open = count.values().filter(|&&c| c != 0).count();
            assert_eq!(
                open,
                61 * 4 * 2,
                "threshold {t}: the border changed ({open})"
            );
        }
        let sphere = crate::mesh::uv_sphere(1.0, 64);
        let dag = build(&sphere);
        let coarse = dag.select(None, 1.0, 0.05);
        let tris: u32 = coarse.iter().map(|&c| dag.clusters[c].triangle_count).sum();
        assert!(
            tris < dag.source_triangles / 2,
            "the sphere barely simplified ({tris})"
        );
    }

    #[test]
    fn levels_halve_the_triangles() {
        let dag = build(&rock(5));
        let triangles: Vec<u32> = dag
            .levels
            .iter()
            .map(|l| {
                dag.clusters[l.first as usize..(l.first + l.count) as usize]
                    .iter()
                    .map(|c| c.triangle_count)
                    .sum()
            })
            .collect();
        for pair in triangles.windows(2) {
            let ratio = pair[1] as f32 / pair[0] as f32;
            assert!(
                (0.3..=0.7).contains(&ratio),
                "levels hold {triangles:?} triangles"
            );
        }
        assert!(*triangles.last().unwrap() <= 2 * CLUSTER_TRIANGLES as u32);
    }
}
