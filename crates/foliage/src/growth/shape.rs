//! Drawing a developed plant at any age: every organ alive that day, each grown along its own
//! smooth curve (Hermite for lengths, logistic for fruit), so a plant drawn a moment later has
//! moved on a little and nothing pops in (organs start at size zero where they attach).
//!
//! Stems are tubes along their internodes (bending toward the light, wandering, sagging under
//! what they carry, runners lying along the ground); leaves are real blades, unfolding about the
//! midrib as they grow, toothed, drooping at the tip and yellowing with age; flowers swell from
//! a closed bud, open their petals and drop them; fruit swells from the flower's center and
//! ripens tip first, with seeds on the skin.

use engine_core::math::{Quat, Rng, Vec2, Vec3, Vec4, smoothstep};

use super::develop::{Organ, Skeleton};
use super::graph::{Axis, AxisKind, Flower, Fruit, Leaf, LeafKind, Node, PlantGraph, Tree};
use crate::meshgen::MeshBuilder;

const UP: Vec3 = Vec3::Y;

/// The meshes of a plant at one age, by material (stems, leaves and sepals, petals, fruit).
#[derive(Default)]
pub struct GrownMeshes {
    pub stems: MeshBuilder,
    pub leaves: MeshBuilder,
    pub petals: MeshBuilder,
    pub fruit: MeshBuilder,
}

impl GrownMeshes {
    pub fn triangles(&self) -> usize {
        (self.stems.indices.len()
            + self.leaves.indices.len()
            + self.petals.indices.len()
            + self.fruit.indices.len())
            / 3
    }
}

/// Smooth start and finish: 0 before 0, 1 after 1, no jump in size or speed at either end.
pub fn hermite(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// Logistic swelling, 0 at 0 and 1 at 1, fastest in the middle (fruit).
pub fn logistic(u: f32) -> f32 {
    let l = |x: f32| 1.0 / (1.0 + (-9.0 * (x - 0.5)).exp());
    ((l(u.clamp(0.0, 1.0)) - l(0.0)) / (l(1.0) - l(0.0))).clamp(0.0, 1.0)
}

/// An axis as grown on the day drawn: its node points with their direction, a normal carried
/// along it (for turning children around it) and radii.
#[derive(Debug, Clone, Default)]
struct Grown {
    points: Vec<Vec3>,
    tangents: Vec<Vec3>,
    normals: Vec<Vec3>,
    radii: Vec<f32>,
}

impl Grown {
    fn at(&self, node: u32) -> (Vec3, Vec3, Vec3, f32) {
        let i = (node as usize).min(self.points.len().saturating_sub(1));
        (
            self.points[i],
            self.tangents[i],
            self.normals[i],
            self.radii[i],
        )
    }
}

/// Any vector at right angles to `v`.
fn perpendicular(v: Vec3) -> Vec3 {
    let a = if v.x.abs() < 0.9 { Vec3::X } else { Vec3::Z };
    v.cross(a).normalize()
}

fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    if axis.length_squared() < 1e-12 {
        return v;
    }
    Quat::from_axis_angle(axis.normalize(), angle) * v
}

fn rgba(c: [f32; 3], a: f32) -> Vec4 {
    Vec4::new(c[0], c[1], c[2], a)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The plant drawn on day `t` (from 0, the seed). `detail` scales tube sides and blade rows
/// (1 near; 0.5 halves them).
pub fn grow_meshes(graph: &PlantGraph, sk: &Skeleton, t: f32, detail: f32) -> GrownMeshes {
    let mut out = GrownMeshes::default();
    let n = sk.organs.len();
    let node_of = |o: &Organ| graph.nodes.get(&sk.names[o.node]);
    // Shown: alive, on a shown parent.
    let mut shown = vec![false; n];
    for (i, o) in sk.organs.iter().enumerate() {
        let alive = o.birth <= t && t < o.death;
        shown[i] = alive && o.parent.is_none_or(|p| shown[p as usize]);
    }
    // What each organ carries (leaves, flowers and fruit at their size today), for sagging.
    let mut load = vec![0.0f32; n];
    for i in (0..n).rev() {
        if !shown[i] {
            continue;
        }
        let o = &sk.organs[i];
        let age = t - o.birth;
        load[i] += match node_of(o) {
            Some(Node::Fruit(f)) => 1.0 * logistic(age / f.swell_days.max(0.1)),
            Some(Node::Flower(f)) => 0.25 * hermite(age / f.bud_days.max(0.1)),
            Some(Node::Leaf(l)) => 0.12 * hermite(age / l.grow_days.max(0.1)),
            _ => 0.0,
        };
        if let Some(p) = o.parent {
            let l = load[i];
            load[p as usize] += l;
        }
    }

    let pipes = tree_radii(graph, sk, t);
    // A tree's thousands of leaves are drawn plainer (lobes and teeth in the texture alone).
    let leaf_detail = match graph.nodes.get(&graph.root) {
        Some(Node::Tree(_)) => detail.min(0.25),
        _ => detail,
    };

    let mut grown: Vec<Option<Grown>> = vec![None; n];
    for i in 0..n {
        if !shown[i] {
            continue;
        }
        let o = &sk.organs[i];
        let age = t - o.birth;
        // Where it grows from and which way.
        let (base, dir, side, surface) = match o.parent {
            None => (Vec3::ZERO, UP, Vec3::X, 0.0),
            Some(p) => {
                let Some(g) = &grown[p as usize] else {
                    continue;
                };
                let (pos, tangent, normal, radius) = g.at(o.at_node);
                let binormal = tangent.cross(normal);
                let side = normal * o.roll.cos() + binormal * o.roll.sin();
                if o.upright {
                    (pos, UP, side, 0.0)
                } else {
                    let dir = (tangent * o.angle.cos() + side * o.angle.sin()).normalize();
                    (pos, dir, side, radius)
                }
            }
        };
        let base = base + side * surface * 0.8;
        match node_of(o) {
            Some(Node::Axis(a)) => {
                let g = grow_axis(a, o, age, base, dir, side, load[i], detail, &mut out.stems);
                grown[i] = Some(g);
            }
            Some(Node::Tree(tr)) => {
                let g = grow_shoot(tr, o, t, base, dir, side, &pipes[i], detail, &mut out.stems);
                grown[i] = Some(g);
            }
            Some(Node::Leaf(l)) => leaf(l, o, age, base, dir, side, leaf_detail, &mut out),
            Some(Node::Flower(f)) => flower(f, o, age, base, dir, side, &mut out),
            Some(Node::Fruit(f)) => fruit(f, o, age, base, dir, side, &mut out),
            None => {}
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn grow_axis(
    a: &Axis,
    o: &Organ,
    age: f32,
    base: Vec3,
    dir: Vec3,
    side: Vec3,
    load: f32,
    detail: f32,
    stems: &mut MeshBuilder,
) -> Grown {
    let total = o.internodes.len() as u32;
    let laid = ((age / a.plastochron_days.max(1e-3)).floor() as u32 + 1).min(total);
    let mut g = Grown::default();
    let mut p = base;
    let mut d = dir.normalize();
    let mut normal = (side - d * side.dot(d)).normalize_or(perpendicular(d));
    let full: f32 = o.internodes.iter().sum::<f32>().max(1e-3);
    let sag = a.sag * (load / (1.0 + 0.5 * load)).min(1.5);
    let mut ages = Vec::with_capacity(laid as usize + 1);
    g.points.push(p);
    g.tangents.push(d);
    g.normals.push(normal);
    ages.push(age);
    for k in 0..laid {
        let node_age = age - k as f32 * a.plastochron_days;
        let full_len = o.internodes[k as usize];
        let len = full_len * hermite(node_age / a.internode_days.max(1e-3));
        // Bend: the node's own wander, toward the sky, toward horizontal, under the load.
        let (b0, b1) = o.bends[k as usize];
        let binormal = d.cross(normal);
        d = rotate(rotate(d, normal, b0), binormal, b1);
        let f = full_len.max(1e-4);
        d = (d + UP * (a.gravitropism * f)).normalize_or(d);
        if a.plagiotropism != 0.0 {
            let flat = Vec3::new(d.x, 0.0, d.z).normalize_or(side);
            d = (d + (flat - d) * (a.plagiotropism * f * 6.0).min(1.0)).normalize_or(d);
        }
        if sag > 0.0 {
            d = (d - UP * (sag * f / full * 2.0)).normalize_or(d);
        }
        // Carry the normal along (parallel transport).
        normal = (normal - d * normal.dot(d)).normalize_or(perpendicular(d));
        p += d * len;
        if a.kind == AxisKind::Stolon {
            // Runners lie along the ground once they reach it.
            p.y = p.y.max(0.004 * o.scale);
        }
        g.points.push(p);
        g.tangents.push(d);
        g.normals.push(normal);
        ages.push(node_age);
    }
    // Radii: thicker where more has grown above (the pipe model's stand-in), thin while young.
    let grown_nodes = laid as f32;
    let young = hermite(age / a.internode_days.max(1e-3));
    for k in 0..g.points.len() {
        let above = (grown_nodes - k as f32).max(0.0);
        g.radii.push(
            (a.tip_radius_m + a.thickening_m * above.powf(0.7)) * o.scale * (0.3 + 0.7 * young),
        );
    }
    // A rosette is a squat crown, not a stem.
    if a.kind == AxisKind::Rosette {
        let r = g.radii[0] * 2.5 + 0.004 * o.scale;
        let path = [base - UP * 0.004, base + UP * (0.01 * o.scale * young)];
        let color = rgba(mix3(a.color, a.wood_color, 0.6), 0.3);
        stems.tube(
            &path,
            &[r, r * 0.6],
            6,
            1.0,
            &|_, _| color,
            None,
            Some(&|_| color),
        );
        return g;
    }
    if g.points.len() >= 2 {
        let r0 = g.radii[0];
        let sides = if r0 > 0.03 {
            10
        } else if r0 > 0.006 {
            7
        } else {
            5
        };
        let sides = ((sides as f32 * detail).round() as u32).max(3);
        let colors: Vec<Vec4> = ages
            .iter()
            .enumerate()
            .map(|(k, &na)| {
                let woody = if a.lignify_days > 0.0 {
                    smoothstep(0.0, a.lignify_days, na)
                } else {
                    0.0
                };
                // Still near the ground, swaying higher up.
                let sway = (g.points[k].y / 0.25).clamp(0.15, 1.0);
                rgba(mix3(a.color, a.wood_color, woody), sway)
            })
            .collect();
        let radii = g.radii.clone();
        stems.tube(
            &g.points,
            &radii,
            sides,
            1.0,
            &|k, _| colors[k],
            None,
            Some(&|_| colors[colors.len() - 1]),
        );
    }
    g
}

/// Radii along every tree shoot on day `t` (empty for other organs), by the pipe model: a
/// node's cross section is the sum of the shoot tips it carries, each pipe `tip_radius_m`
/// across, so the trunk thickens year by year as the crown grows. Wood stays when a branch is
/// shed. A tip fades in over its first internode's growth, so nothing thickens in a jump.
fn tree_radii(graph: &PlantGraph, sk: &Skeleton, t: f32) -> Vec<Vec<f32>> {
    let n = sk.organs.len();
    let mut out = vec![Vec::new(); n];
    let Some(Node::Tree(tr)) = graph.nodes.get(&graph.root) else {
        return out;
    };
    let tree_node = sk.names.iter().position(|m| *m == graph.root);
    let is_shoot = |o: &Organ| Some(o.node) == tree_node;
    let grow = |o: &Organ| {
        o.node_days
            .first()
            .map_or(0.0, |&d| hermite((t - d) / tr.internode_days.max(1e-3)))
    };
    // Each shoot's pipes at its base, children first (they come later in the list).
    let mut base = vec![0.0f32; n];
    for i in (0..n).rev() {
        let o = &sk.organs[i];
        if !is_shoot(o) || o.birth > t {
            continue;
        }
        let laid = o.node_days.iter().filter(|&&d| d <= t).count();
        let mut at_node = vec![0.0f32; laid + 1];
        for &c in &sk.children[i] {
            let c = c as usize;
            if is_shoot(&sk.organs[c]) && sk.organs[c].birth <= t {
                let k = (sk.organs[c].at_node as usize).min(laid);
                at_node[k] += base[c];
            }
        }
        // From the tip down: the tip's own pipe, then each branch's pipes where it joins.
        let mut pipes = grow(o);
        let mut radii = vec![0.0f32; laid + 1];
        for k in (0..=laid).rev() {
            pipes += at_node[k];
            radii[k] = tr.tip_radius_m * pipes.max(1e-4).powf(1.0 / tr.pipe_exponent.max(1.0));
        }
        base[i] = pipes;
        out[i] = radii;
    }
    out
}

/// A tree shoot as grown on day `t`: its metamers in the directions they grew, each
/// lengthening after the day its node was laid.
#[allow(clippy::too_many_arguments)]
fn grow_shoot(
    tr: &Tree,
    o: &Organ,
    t: f32,
    base: Vec3,
    dir: Vec3,
    side: Vec3,
    radii: &[f32],
    detail: f32,
    stems: &mut MeshBuilder,
) -> Grown {
    let laid = o.node_days.iter().filter(|&&d| d <= t).count();
    let mut g = Grown::default();
    let mut p = base;
    let mut d = o.dirs.first().copied().unwrap_or(dir);
    let mut normal = (side - d * side.dot(d)).normalize_or(perpendicular(d));
    g.points.push(p);
    g.tangents.push(d);
    g.normals.push(normal);
    let mut ages = vec![t - o.node_days.first().copied().unwrap_or(o.birth)];
    for k in 0..laid {
        let node_age = t - o.node_days[k];
        d = o.dirs[k];
        normal = (normal - d * normal.dot(d)).normalize_or(perpendicular(d));
        p += d * o.internodes[k] * hermite(node_age / tr.internode_days.max(1e-3));
        g.points.push(p);
        g.tangents.push(d);
        g.normals.push(normal);
        ages.push(node_age);
    }
    g.radii = (0..g.points.len())
        .map(|k| radii.get(k).copied().unwrap_or(tr.tip_radius_m))
        .collect();
    if g.points.len() >= 2 {
        let r0 = g.radii[0];
        let sides = if r0 > 0.05 {
            12
        } else if r0 > 0.015 {
            8
        } else if r0 > 0.006 {
            5
        } else {
            3
        };
        let sides = ((sides as f32 * detail).round() as u32).max(3);
        let colors: Vec<Vec4> = ages
            .iter()
            .enumerate()
            .map(|(k, &na)| {
                let woody = smoothstep(0.0, tr.lignify_days.max(1.0), na);
                // Still at the trunk's foot, swaying in the crown.
                let sway = (g.points[k].y / 3.0).clamp(0.05, 1.0);
                rgba(mix3(tr.color, tr.wood_color, woody), sway)
            })
            .collect();
        stems.tube(
            &g.points,
            &g.radii,
            sides,
            1.0,
            &|k, _| colors[k],
            None,
            Some(&|_| colors[colors.len() - 1]),
        );
    }
    g
}

/// The outline of a blade: half-width (fraction of its widest) at `s` along it.
fn outline(l: &Leaf, s: f32) -> f32 {
    let widest = l.widest.clamp(0.1, 0.9);
    let p = 0.5f32.ln() / widest.ln();
    let w = (std::f32::consts::PI * s.clamp(0.0, 1.0).powf(p))
        .sin()
        .max(0.0);
    if l.teeth > 0 && s > 0.12 {
        let x = s * l.teeth as f32;
        let saw = x - x.floor();
        w * (1.0 - l.tooth_depth * saw * smoothstep(0.12, 0.3, s))
    } else {
        w
    }
}

/// One blade from `base` toward `dir`, `length` long, opened flat to `fold` (radians each half
/// stands up from flat), with `up` its upper side.
#[allow(clippy::too_many_arguments)]
fn blade(
    b: &mut MeshBuilder,
    l: &Leaf,
    base: Vec3,
    dir: Vec3,
    up: Vec3,
    length: f32,
    fold: f32,
    color: [f32; 3],
    detail: f32,
) {
    if length < 1e-4 {
        return;
    }
    // Rows along the blade: fewer on small blades, teeth cut in only at full detail (a far or
    // busy plant has its teeth in the texture alone).
    let small = (length / 0.12).clamp(0.3, 1.0);
    let teeth = if detail >= 0.75 {
        2 * l.teeth.min(8) as usize
    } else {
        0
    };
    let rows = ((14.0 * detail * small).round() as usize).max(3) + teeth;
    // Vertices each side of the midrib, plus the midrib (one: a tree's plain far leaves).
    let cols = if detail < 0.3 { 1usize } else { 2 };
    let side = dir.cross(up).normalize_or(perpendicular(dir));
    let up = side.cross(dir).normalize();
    let half_w = 0.5 * l.width * length;
    let first = b.vertices.len() as u32;
    for i in 0..=rows {
        let s = i as f32 / rows as f32;
        let w = outline(l, s) * half_w;
        // The midrib arches up a little then droops toward the tip.
        let mid = base + dir * (s * length) + up * (length * (0.08 * s * (1.0 - s)))
            - UP * (l.droop * 0.35 * length * s * s);
        for c in 0..=2 * cols {
            let x = c as f32 / cols as f32 - 1.0; // -1 .. 1
            let sign = x.signum();
            let lateral = side * (fold.cos() * sign) + up * fold.sin();
            // Each half curves gently (cupped), more toward its edge.
            let pos = mid + lateral * (x.abs() * w) + up * (0.12 * w * x * x);
            let n = (up * fold.cos() - side * (sign * fold.sin()) * 0.8 + up * 0.3).normalize();
            // Darker along the midrib, lighter at the edges, yellower toward the tip when young.
            let shade = 0.82 + 0.22 * x.abs();
            let c = Vec4::new(color[0] * shade, color[1] * shade, color[2] * shade, 1.0);
            b.vertex(pos, n, Vec2::new(x * 0.5 + 0.5, 1.0 - s), c);
        }
    }
    let row = (2 * cols + 1) as u32;
    for i in 0..rows as u32 {
        for c in 0..row - 1 {
            let a = first + i * row + c;
            b.tri(a, a + 1, a + row + 1);
            b.tri(a, a + row + 1, a + row);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn leaf(
    l: &Leaf,
    o: &Organ,
    age: f32,
    base: Vec3,
    dir: Vec3,
    side: Vec3,
    detail: f32,
    out: &mut GrownMeshes,
) {
    let u = age / l.grow_days.max(0.1);
    let g = hermite(u);
    if g < 0.01 {
        return;
    }
    let mut rng = Rng::new(o.seed);
    let s = o.scale * g;
    let petiole = rng.range(l.petiole_m.0, l.petiole_m.1.max(l.petiole_m.0)) * s;
    let blade_len = rng.range(l.blade_m.0, l.blade_m.1.max(l.blade_m.0)) * s;
    // Unfolds about the midrib early, then lies open.
    let open = smoothstep(0.0, l.unfold.max(0.05), u);
    let fold = (1.45 - 1.25 * open).max(0.12);
    // Yellows, then browns before it drops.
    let old = if l.senesce_days > 0.0 {
        smoothstep(l.senesce_days, l.senesce_days + l.fade_days.max(0.1), age)
    } else {
        0.0
    };
    let young = [0.17, 0.30, 0.05];
    let color = mix3(
        mix3(young, l.color, smoothstep(0.0, 0.7, u)),
        l.old_color,
        old,
    );
    // Petiole: arches out from the stem, bending over by `droop` (and more as it ages).
    let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(side);
    let droop = l.droop * (1.0 + 0.6 * old);
    let end_dir = (flat + UP * (0.5 - droop)).normalize();
    let mid_dir = (dir + end_dir).normalize();
    let mut path = vec![base];
    let mut radius = vec![l.petiole_radius_m * o.scale * (0.4 + 0.6 * g)];
    let segs = if detail >= 0.75 { 5 } else { 3 };
    for k in 1..=segs {
        let f = k as f32 / segs as f32;
        let d = if f < 0.5 {
            dir.lerp(mid_dir, f * 2.0)
        } else {
            mid_dir.lerp(end_dir, f * 2.0 - 1.0)
        };
        let p = path[k - 1] + d.normalize() * (petiole / segs as f32);
        path.push(p);
        radius.push(radius[0] * (1.0 - 0.35 * f));
    }
    let stalk = rgba(mix3([0.20, 0.28, 0.07], color, 0.3), 1.0);
    let sides = if detail >= 0.75 { 4 } else { 3 };
    // A short stalk on a plain leaf is too small to see.
    if petiole > 0.002 && (detail >= 0.75 || petiole > 0.012) {
        out.stems
            .tube(&path, &radius, sides, 1.0, &|_, _| stalk, None, None);
    }
    let tip = *path.last().unwrap();
    let up = (UP - end_dir * UP.dot(end_dir)).normalize_or(UP);
    let blades: Vec<(Vec3, f32, f32)> = match l.kind {
        LeafKind::Simple => vec![(end_dir, 1.0, 0.0)],
        LeafKind::Trifoliate => vec![
            (end_dir, 1.0, 0.0),
            (end_dir, 0.92, 0.95),
            (end_dir, 0.92, -0.95),
        ],
        LeafKind::Palmate(n) => {
            let n = n.max(3);
            (0..n)
                .map(|i| {
                    let x = i as f32 / (n - 1) as f32 * 2.0 - 1.0;
                    (end_dir, 1.0 - 0.35 * x.abs(), x * 1.3)
                })
                .collect()
        }
        LeafKind::Pinnate(n) => {
            let mut v = vec![(end_dir, 1.0, 0.0)];
            for _ in 0..n {
                v.push((end_dir, 0.8, 1.1));
                v.push((end_dir, 0.8, -1.1));
            }
            v
        }
    };
    let rachis = match l.kind {
        LeafKind::Pinnate(n) => blade_len * 0.55 * n as f32,
        _ => 0.0,
    };
    for (k, &(d0, scale, turn)) in blades.iter().enumerate() {
        let d = rotate(d0, up, turn * (0.55 + 0.45 * open)).normalize();
        let start = match l.kind {
            LeafKind::Pinnate(n) if k > 0 => {
                let pair = ((k - 1) / 2) as f32 / n.max(1) as f32;
                tip + end_dir * (rachis * pair)
            }
            LeafKind::Pinnate(_) => tip + end_dir * rachis,
            _ => tip,
        };
        let jitter = rng.range(0.92, 1.06);
        blade(
            &mut out.leaves,
            l,
            start,
            d,
            up,
            blade_len * scale * jitter,
            fold,
            color,
            detail,
        );
    }
    if rachis > 0.0 {
        out.stems.tube(
            &[tip, tip + end_dir * rachis],
            &[radius[segs] * 0.8, radius[segs] * 0.4],
            3,
            1.0,
            &|_, _| stalk,
            None,
            None,
        );
    }
}

/// A small pointed sepal or a rounded petal: a cupped strip from `base` along `dir`.
#[allow(clippy::too_many_arguments)]
fn petal(
    b: &mut MeshBuilder,
    base: Vec3,
    dir: Vec3,
    up: Vec3,
    length: f32,
    width: f32,
    cup: f32,
    pointed: bool,
    root_color: [f32; 3],
    color: [f32; 3],
) {
    if length < 1e-4 {
        return;
    }
    let side = dir.cross(up).normalize_or(perpendicular(dir));
    let up = side.cross(dir).normalize();
    let rows = 5;
    let first = b.vertices.len() as u32;
    for i in 0..=rows {
        let s = i as f32 / rows as f32;
        let w = if pointed {
            (std::f32::consts::PI * s.powf(0.7)).sin()
        } else {
            // Obovate: narrow claw at the base, round at the end.
            (s * 1.6).min(1.0).powf(0.6) * (1.0 - s.powf(4.0)).max(0.0).sqrt()
        } * 0.5
            * width;
        let mid = base + dir * (s * length) + up * (cup * length * s * s * 0.5);
        let c = mix3(root_color, color, smoothstep(0.0, 0.45, s));
        for x in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
            let pos = mid + side * (x * w) + up * (cup * w * x * x * 0.6);
            let n = (up - side * (x * cup * 0.8)).normalize();
            b.vertex(pos, n, Vec2::new(x * 0.5 + 0.5, s), rgba(c, 1.0));
        }
    }
    for i in 0..rows as u32 {
        for c in 0..4u32 {
            let a = first + i * 5 + c;
            b.tri(a, a + 1, a + 6);
            b.tri(a, a + 6, a + 5);
        }
    }
}

/// A small ball (flower centers, stamen tips).
fn ball(b: &mut MeshBuilder, center: Vec3, r: f32, color: Vec4) {
    if r < 1e-5 {
        return;
    }
    let first = b.vertices.len() as u32;
    let (rings, sides) = (4u32, 6u32);
    for i in 0..=rings {
        let phi = std::f32::consts::PI * i as f32 / rings as f32;
        for k in 0..=sides {
            let th = std::f32::consts::TAU * k as f32 / sides as f32;
            let n = Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
            b.vertex(center + n * r, n, Vec2::ZERO, color);
        }
    }
    for i in 0..rings {
        for k in 0..sides {
            let a = first + i * (sides + 1) + k;
            let c = a + sides + 1;
            b.tri(a, c, a + 1);
            b.tri(a + 1, c, c + 1);
        }
    }
}

/// Which way a flower or fruit faces: out from its stalk, turned up toward the light (a
/// flower) or hanging under its weight (a fruit, `hang` 0..1).
fn facing(dir: Vec3, hang: f32) -> Vec3 {
    let up_a_bit = (dir + UP * 0.6).normalize();
    up_a_bit
        .lerp(-UP, hang.clamp(0.0, 1.0) * 0.85)
        .normalize_or(dir)
}

#[allow(clippy::too_many_arguments)]
fn sepals(
    b: &mut MeshBuilder,
    base: Vec3,
    face: Vec3,
    count: u32,
    length: f32,
    spread: f32,
    color: [f32; 3],
    seed: u64,
) {
    let side0 = perpendicular(face);
    let mut rng = Rng::new(seed);
    for k in 0..count {
        let roll = k as f32 / count as f32 * std::f32::consts::TAU + rng.range(-0.15, 0.15);
        let out = rotate(side0, face, roll);
        let d = (face * spread.cos() + out * spread.sin()).normalize();
        let up = face;
        petal(
            b,
            base,
            d,
            up,
            length * rng.range(0.85, 1.1),
            length * 0.42,
            0.15,
            true,
            color,
            color,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn flower(
    f: &Flower,
    o: &Organ,
    age: f32,
    base: Vec3,
    dir: Vec3,
    _side: Vec3,
    out: &mut GrownMeshes,
) {
    let bud = hermite(age / f.bud_days.max(0.1));
    if bud < 0.01 {
        return;
    }
    let open = hermite((age - f.bud_days) / f.open_days.max(0.1));
    // Petals wither in the last day before they drop.
    let wilt = smoothstep(
        f.bloom_days * 0.6,
        f.bloom_days,
        age - f.bud_days - f.open_days,
    );
    let s = o.scale;
    let face = facing(dir, 0.15 * (1.0 - open));
    let (pl, pw) = (
        f.petal_m.0 * s * (0.35 + 0.65 * bud),
        f.petal_m.1 * s * (0.35 + 0.65 * bud),
    );
    // Sepals: hold the bud closed, then fold back.
    let sepal_spread = 0.25 + 1.45 * open;
    sepals(
        &mut out.leaves,
        base,
        face,
        f.sepals.max(3),
        pl * 0.75,
        sepal_spread,
        f.sepal_color,
        o.seed,
    );
    // Petals: tucked into the bud (greenish-white), then opening out flat-ish.
    let spread = 0.12 + (1.25 - 0.3 * f.cup) * open - 0.3 * wilt;
    let petal_color = mix3([0.55, 0.62, 0.40], f.petal_color, smoothstep(0.4, 1.0, bud));
    let petal_color = mix3(petal_color, [0.55, 0.48, 0.38], wilt * 0.6);
    let side0 = perpendicular(face);
    for k in 0..f.petals.max(1) {
        let roll = (k as f32 + 0.5) / f.petals as f32 * std::f32::consts::TAU;
        let outward = rotate(side0, face, roll);
        let d = (face * spread.cos() + outward * spread.sin()).normalize();
        petal(
            &mut out.petals,
            base + face * (0.001 * s),
            d,
            face,
            pl,
            pw,
            f.cup * (0.4 + 0.6 * open),
            false,
            mix3(f.petal_color, f.center_color, 0.25),
            petal_color,
        );
    }
    // The center: a dome of pistils ringed by stamens, once open.
    if open > 0.05 {
        let c = rgba(f.center_color, 1.0);
        ball(
            &mut out.petals,
            base + face * (0.002 * s),
            0.0035 * s * open,
            c,
        );
        let anther = rgba([0.85, 0.65, 0.05], 1.0);
        for k in 0..f.stamens {
            let roll = k as f32 / f.stamens.max(1) as f32 * std::f32::consts::TAU;
            let p = base + face * (0.003 * s) + rotate(side0, face, roll) * (0.0048 * s * open);
            ball(&mut out.petals, p, 0.0009 * s * open, anther);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fruit(
    f: &Fruit,
    o: &Organ,
    age: f32,
    base: Vec3,
    dir: Vec3,
    _side: Vec3,
    out: &mut GrownMeshes,
) {
    let swell = logistic(age / f.swell_days.max(0.1));
    let ripe = age / f.ripe_days.max(0.1);
    let s = o.scale * (0.12 + 0.88 * swell);
    let (len, wid) = (f.length_m * s, f.width_m * s);
    let face = facing(dir, 0.25 + 0.75 * swell);
    let side0 = perpendicular(face);
    let side1 = face.cross(side0);
    // The calyx stays on top, its sepals turned back.
    sepals(
        &mut out.leaves,
        base,
        face,
        10,
        (0.008 + 0.004 * swell) * o.scale,
        1.75,
        [0.10, 0.24, 0.05],
        o.seed ^ 0xca1,
    );
    let color_at = |v: f32| {
        // The tip ripens first.
        let r = (ripe + 0.18 * (v - 0.5)).clamp(0.0, 1.2);
        let mut c = f.colors.first().map_or([0.5; 3], |c| c.1);
        for w in f.colors.windows(2) {
            if r >= w[0].0 {
                let k = ((r - w[0].0) / (w[1].0 - w[0].0).max(1e-4)).clamp(0.0, 1.0);
                c = mix3(w[0].1, w[1].1, smoothstep(0.0, 1.0, k));
            }
        }
        c
    };
    // Lathe: round shoulders under the calyx, narrowing to a point.
    let profile = |v: f32| {
        let shoulder = (std::f32::consts::FRAC_PI_2 * (v / 0.22).min(1.0))
            .sin()
            .powf(0.6);
        shoulder * (1.0 - v * v).max(0.0).powf(0.35 + f.point)
    };
    let mut rng = Rng::new(o.seed);
    // Uneven pollination dents a berry here and there.
    let dents: Vec<(f32, f32, f32)> = (0..3)
        .map(|_| {
            (
                rng.range(0.0, std::f32::consts::TAU),
                rng.range(0.3, 0.8),
                rng.range(0.0, 0.12),
            )
        })
        .collect();
    let dent = |a: f32, v: f32| {
        dents.iter().fold(1.0, |acc, &(da, dv, depth)| {
            let d = ((a - da).sin().abs() * 2.0).min(1.0) + (v - dv).abs() * 3.0;
            acc - depth * (1.0 - d.min(1.0))
        })
    };
    let (rings, sides) = (12u32, 14u32);
    let first = out.fruit.vertices.len() as u32;
    let top = base + face * (0.002 * o.scale);
    for i in 0..=rings {
        let v = i as f32 / rings as f32;
        let r = 0.5 * wid * profile(v);
        let c = color_at(v);
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            let radial = side0 * a.cos() + side1 * a.sin();
            let rr = r * dent(a, v);
            let p = top + face * (v * len) + radial * rr;
            // Normal from the profile's slope.
            let slope = (0.5 * wid * (profile((v + 0.02).min(1.0)) - profile((v - 0.02).max(0.0))))
                / (0.04 * len.max(1e-5));
            let n = (radial - face * slope).normalize_or(radial);
            out.fruit
                .vertex(p, n, Vec2::new(k as f32 / sides as f32, v), rgba(c, 1.0));
        }
    }
    for i in 0..rings {
        for k in 0..sides {
            let a = first + i * (sides + 1) + k;
            let c = a + sides + 1;
            out.fruit.tri(a, c, a + 1);
            out.fruit.tri(a + 1, c, c + 1);
        }
    }
    // Seeds in a golden spiral over the skin, sitting in little pits.
    if f.seeds > 0 && swell > 0.05 {
        let seed_c = mix3(f.seed_color, [0.75, 0.55, 0.12], smoothstep(0.6, 1.0, ripe));
        let size = 0.0011 * o.scale * (0.6 + 0.4 * swell);
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        for k in 0..f.seeds {
            let v = 0.12 + 0.83 * (k as f32 + 0.5) / f.seeds as f32;
            let a = k as f32 * golden;
            let radial = side0 * a.cos() + side1 * a.sin();
            let r = 0.5 * wid * profile(v) * dent(a, v);
            let p = top + face * (v * len) + radial * (r + 0.0002);
            let along = face;
            let across = radial.cross(along).normalize();
            let c = rgba(seed_c, 1.0);
            let q = [
                p + along * size,
                p + across * (size * 0.6),
                p - along * size,
                p - across * (size * 0.6),
            ];
            let i0 = out.fruit.vertex(q[0], radial, Vec2::ZERO, c);
            let i1 = out.fruit.vertex(q[1], radial, Vec2::ZERO, c);
            let i2 = out.fruit.vertex(q[2], radial, Vec2::ZERO, c);
            let i3 = out.fruit.vertex(q[3], radial, Vec2::ZERO, c);
            out.fruit.tri(i0, i1, i2);
            out.fruit.tri(i0, i2, i3);
        }
    }
}
