//! Bounding volume hierarchies for the probes' rays: one per mesh over its triangles, and one
//! over the instances near the camera each frame. Binned surface area heuristic (Wald 2007),
//! flattened depth first so a node's left child follows it; `probe_gi.wgsl` walks them.

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

/// GPU layout of `BvhNode` in `probe_gi.wgsl`: bounds, and either the right child's index
/// (`count` 0, the left child is the next node) or the first of `count` items.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub(crate) struct Node {
    pub min: [f32; 3],
    pub right_or_first: u32,
    pub max: [f32; 3],
    pub count: u32,
}

/// Items per leaf at most.
const LEAF_ITEMS: usize = 4;
const BINS: usize = 12;
/// Below this depth splits halve by count along the longest axis, so no tree gets deeper than
/// about this plus log2(items): the shader's traversal stack holds `MAX_DEPTH` + 24 nodes.
pub(crate) const MAX_DEPTH: usize = 24;

#[derive(Clone, Copy)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    fn grow(&mut self, other: &Bounds) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    fn area(&self) -> f32 {
        let d = (self.max - self.min).max(Vec3::ZERO);
        d.x * d.y + d.y * d.z + d.z * d.x
    }
}

/// A hierarchy over `boxes` (min, max per item): nodes, and the item order its leaves refer to.
pub(crate) fn build(boxes: &[(Vec3, Vec3)]) -> (Vec<Node>, Vec<u32>) {
    let items: Vec<Bounds> = boxes
        .iter()
        .map(|&(min, max)| Bounds { min, max })
        .collect();
    let centers: Vec<Vec3> = items.iter().map(|b| (b.min + b.max) * 0.5).collect();
    let mut order: Vec<u32> = (0..boxes.len() as u32).collect();
    let mut nodes = Vec::with_capacity(boxes.len().max(1) * 2 / LEAF_ITEMS.max(1) + 1);
    if boxes.is_empty() {
        // One empty leaf that nothing hits.
        nodes.push(Node {
            min: [1.0; 3],
            right_or_first: 0,
            max: [-1.0; 3],
            count: 0,
        });
        return (nodes, order);
    }
    split(&items, &centers, &mut order, 0, boxes.len(), 0, &mut nodes);
    (nodes, order)
}

fn split(
    items: &[Bounds],
    centers: &[Vec3],
    order: &mut [u32],
    start: usize,
    end: usize,
    depth: usize,
    nodes: &mut Vec<Node>,
) {
    let mut bounds = Bounds::EMPTY;
    let mut center_bounds = Bounds::EMPTY;
    for &i in &order[start..end] {
        bounds.grow(&items[i as usize]);
        let c = centers[i as usize];
        center_bounds.grow(&Bounds { min: c, max: c });
    }
    let index = nodes.len();
    nodes.push(Node {
        min: bounds.min.to_array(),
        right_or_first: start as u32,
        max: bounds.max.to_array(),
        count: (end - start) as u32,
    });
    let n = end - start;
    if n <= LEAF_ITEMS {
        return;
    }
    // The best split along any axis by the surface area heuristic, binned.
    let extent = center_bounds.max - center_bounds.min;
    let mut best: Option<(f32, usize, usize)> = None;
    for axis in 0..3 {
        if depth >= MAX_DEPTH {
            break;
        }
        if extent[axis] <= 1e-9 {
            continue;
        }
        let mut bins = [(Bounds::EMPTY, 0usize); BINS];
        let scale = BINS as f32 / extent[axis];
        let bin_of =
            |c: Vec3| (((c[axis] - center_bounds.min[axis]) * scale) as usize).min(BINS - 1);
        for &i in &order[start..end] {
            let b = &mut bins[bin_of(centers[i as usize])];
            b.0.grow(&items[i as usize]);
            b.1 += 1;
        }
        // Costs of putting bins 0..=k on the left.
        let mut left_area = [0.0f32; BINS];
        let mut left_count = [0usize; BINS];
        let mut acc = Bounds::EMPTY;
        let mut count = 0;
        for k in 0..BINS {
            acc.grow(&bins[k].0);
            count += bins[k].1;
            left_area[k] = acc.area();
            left_count[k] = count;
        }
        let mut acc = Bounds::EMPTY;
        let mut count = 0;
        for k in (1..BINS).rev() {
            acc.grow(&bins[k].0);
            count += bins[k].1;
            let (l, r) = (left_count[k - 1], count);
            if l == 0 || r == 0 {
                continue;
            }
            let cost = left_area[k - 1] * l as f32 + acc.area() * r as f32;
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, k));
            }
        }
    }
    let mid = match best {
        Some((cost, axis, k)) if cost < bounds.area() * n as f32 => {
            let scale = BINS as f32 / extent[axis];
            let threshold = |i: &u32| {
                let c = centers[*i as usize];
                ((((c[axis] - center_bounds.min[axis]) * scale) as usize).min(BINS - 1)) < k
            };
            start + partition(&mut order[start..end], threshold)
        }
        // No split pays, all centers coincide, or the tree is deep: halve by count along the
        // longest axis.
        _ => {
            let axis = extent.max_position();
            order[start..end].select_nth_unstable_by(n / 2, |a, b| {
                centers[*a as usize][axis].total_cmp(&centers[*b as usize][axis])
            });
            start + n / 2
        }
    };
    if mid == start || mid == end {
        return;
    }
    nodes[index].count = 0;
    split(items, centers, order, start, mid, depth + 1, nodes);
    nodes[index].right_or_first = nodes.len() as u32;
    split(items, centers, order, mid, end, depth + 1, nodes);
}

/// Put the items `left` accepts first; returns how many.
fn partition(items: &mut [u32], left: impl Fn(&u32) -> bool) -> usize {
    let mut i = 0;
    for j in 0..items.len() {
        if left(&items[j]) {
            items.swap(i, j);
            i += 1;
        }
    }
    i
}

/// A mesh's hierarchy ready to upload: nodes and its triangles' corners (three `[x, y, z, 0]`
/// each) in leaf order.
pub(crate) struct MeshBvh {
    pub nodes: Vec<Node>,
    pub triangles: Vec<[f32; 4]>,
}

pub(crate) fn build_mesh(positions: &[[f32; 3]], indices: &[u32]) -> MeshBvh {
    let tris: Vec<[Vec3; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t.iter().all(|&i| (i as usize) < positions.len()))
        .map(|t| t.map(|i| Vec3::from(positions[i as usize])))
        .collect();
    let boxes: Vec<(Vec3, Vec3)> = tris
        .iter()
        .map(|t| (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2])))
        .collect();
    let (nodes, order) = build(&boxes);
    let mut triangles = Vec::with_capacity(order.len() * 3);
    for &i in &order {
        for p in tris[i as usize] {
            triangles.push([p.x, p.y, p.z, 0.0]);
        }
    }
    MeshBvh { nodes, triangles }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nearest hit along a ray, walking the hierarchy as the shader does.
    fn trace(bvh: &MeshBvh, origin: Vec3, dir: Vec3) -> Option<f32> {
        let inv = dir.recip();
        let mut stack = vec![0u32];
        let mut best: Option<f32> = None;
        while let Some(i) = stack.pop() {
            let n = bvh.nodes[i as usize];
            let t0 = (Vec3::from(n.min) - origin) * inv;
            let t1 = (Vec3::from(n.max) - origin) * inv;
            let near = t0.min(t1).max_element().max(0.0);
            let far = t0.max(t1).min_element().min(best.unwrap_or(f32::INFINITY));
            if near > far {
                continue;
            }
            if n.count > 0 {
                for t in n.right_or_first..n.right_or_first + n.count {
                    let c = &bvh.triangles[t as usize * 3..t as usize * 3 + 3];
                    let [a, b, c] = [c[0], c[1], c[2]].map(|p| Vec3::new(p[0], p[1], p[2]));
                    let (e1, e2) = (b - a, c - a);
                    let p = dir.cross(e2);
                    let det = e1.dot(p);
                    if det.abs() < 1e-12 {
                        continue;
                    }
                    let s = origin - a;
                    let u = s.dot(p) / det;
                    let q = s.cross(e1);
                    let v = dir.dot(q) / det;
                    let t = e2.dot(q) / det;
                    if u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0 && best.is_none_or(|b| t < b)
                    {
                        best = Some(t);
                    }
                }
            } else {
                stack.push(n.right_or_first);
                stack.push(i + 1);
            }
        }
        best
    }

    #[test]
    fn rays_find_the_nearest_triangle_of_a_sphere() {
        let mesh = crate::mesh::Shape::Sphere {
            radius: 1.0,
            segments: 32,
        }
        .mesh();
        let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
        let bvh = build_mesh(&positions, &mesh.indices);
        assert_eq!(bvh.triangles.len(), mesh.indices.len());
        // From 5 m away along several directions, the sphere is hit about 4 m out.
        for dir in [Vec3::X, Vec3::NEG_Y, Vec3::new(1.0, 2.0, -0.5).normalize()] {
            let t = trace(&bvh, -dir * 5.0, dir).expect("hit");
            assert!((t - 4.0).abs() < 0.01, "{dir}: {t}");
        }
        // Missing it.
        assert!(trace(&bvh, Vec3::new(0.0, 3.0, -5.0), Vec3::Z).is_none());
        // From inside, the far side 1 m away.
        let t = trace(&bvh, Vec3::ZERO, Vec3::Z).expect("hit from inside");
        assert!((t - 1.0).abs() < 0.01, "{t}");
        // Leaves stay small and the tree is reasonably shallow.
        assert!(bvh.nodes.iter().all(|n| n.count as usize <= LEAF_ITEMS));
    }

    #[test]
    fn empty_and_single_item_hierarchies() {
        let (nodes, order) = build(&[]);
        assert_eq!((nodes.len(), order.len()), (1, 0));
        let (nodes, order) = build(&[(Vec3::ZERO, Vec3::ONE)]);
        assert_eq!(nodes.len(), 1);
        assert_eq!((nodes[0].count, order), (1, vec![0]));
    }
}
