//! Procedural meshes for the showcase scenery: trees, bushes, grass and flower patches,
//! houses and lamp posts. Everything is built from code (no asset files) with vertex colors, so
//! a few shared materials draw the whole valley in a handful of instanced batches.

use engine::core::math::noise::{fbm3, perlin3};
use engine::core::math::smoothstep;
use engine::prelude::*;
use engine::render::Vertex;

/// Mesh under construction with helpers for boxes, tubes and blobs.
#[derive(Default)]
pub struct Builder {
    pub mesh: Mesh,
}

fn v(p: Vec3, n: Vec3, c: Vec3) -> Vertex {
    Vertex::new(p, n, Vec2::new(p.x + p.z, p.y)).with_color(c.extend(1.0))
}

impl Builder {
    pub fn finish(self) -> Mesh {
        self.mesh
    }

    fn base(&self) -> u32 {
        self.mesh.vertices.len() as u32
    }

    /// Flat-shaded quad (corners counter-clockwise seen from the front).
    pub fn quad(&mut self, p: [Vec3; 4], c: Vec3) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let b = self.base();
        for q in p {
            self.mesh.vertices.push(v(q, n, c));
        }
        self.mesh
            .indices
            .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }

    /// Flat-shaded triangle (counter-clockwise seen from the front).
    pub fn tri(&mut self, p: [Vec3; 3], c: Vec3) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let b = self.base();
        for q in p {
            self.mesh.vertices.push(v(q, n, c));
        }
        self.mesh.indices.extend_from_slice(&[b, b + 1, b + 2]);
    }

    /// Box with half extents `h`, rotated by `rot` around its center.
    pub fn cuboid(&mut self, center: Vec3, h: Vec3, rot: Quat, c: Vec3) {
        let corner = |sx: f32, sy: f32, sz: f32| center + rot * (h * Vec3::new(sx, sy, sz));
        let faces = [
            // +X
            [
                corner(1., -1., 1.),
                corner(1., -1., -1.),
                corner(1., 1., -1.),
                corner(1., 1., 1.),
            ],
            // -X
            [
                corner(-1., -1., -1.),
                corner(-1., -1., 1.),
                corner(-1., 1., 1.),
                corner(-1., 1., -1.),
            ],
            // +Y
            [
                corner(-1., 1., 1.),
                corner(1., 1., 1.),
                corner(1., 1., -1.),
                corner(-1., 1., -1.),
            ],
            // -Y
            [
                corner(-1., -1., -1.),
                corner(1., -1., -1.),
                corner(1., -1., 1.),
                corner(-1., -1., 1.),
            ],
            // +Z
            [
                corner(-1., -1., 1.),
                corner(1., -1., 1.),
                corner(1., 1., 1.),
                corner(-1., 1., 1.),
            ],
            // -Z
            [
                corner(1., -1., -1.),
                corner(-1., -1., -1.),
                corner(-1., 1., -1.),
                corner(1., 1., -1.),
            ],
        ];
        for f in faces {
            self.quad(f, c);
        }
    }

    /// Axis-aligned box from min to max corner.
    pub fn aabb(&mut self, min: Vec3, max: Vec3, c: Vec3) {
        self.cuboid((min + max) * 0.5, (max - min) * 0.5, Quat::IDENTITY, c);
    }

    /// Smooth tapered tube from `a` (radius `ra`) to `b` (radius `rb`), colors per end.
    #[allow(clippy::too_many_arguments)]
    pub fn tube(
        &mut self,
        a: Vec3,
        b: Vec3,
        ra: f32,
        rb: f32,
        sides: u32,
        ca: Vec3,
        cb: Vec3,
        cap: bool,
    ) {
        let axis = (b - a).normalize_or(Vec3::Y);
        let side = if axis.y.abs() < 0.95 {
            Vec3::Y
        } else {
            Vec3::X
        };
        let u = axis.cross(side).normalize();
        let w = axis.cross(u);
        let base = self.base();
        let slope = (ra - rb) / (b - a).length().max(1e-4);
        for i in 0..=sides {
            let t = i as f32 / sides as f32 * TAU;
            let dir = u * t.cos() + w * t.sin();
            let n = (dir + axis * slope).normalize();
            let uv_x = i as f32 / sides as f32;
            self.mesh.vertices.push(
                Vertex::new(a + dir * ra, n, Vec2::new(uv_x, 0.0)).with_color(ca.extend(1.0)),
            );
            self.mesh.vertices.push(
                Vertex::new(b + dir * rb, n, Vec2::new(uv_x, 1.0)).with_color(cb.extend(1.0)),
            );
        }
        for i in 0..sides {
            let k = base + i * 2;
            self.mesh
                .indices
                .extend_from_slice(&[k, k + 2, k + 1, k + 1, k + 2, k + 3]);
        }
        if cap && rb > 0.0 {
            let center = self.base();
            self.mesh.vertices.push(v(b, axis, cb));
            for i in 0..=sides {
                let t = i as f32 / sides as f32 * TAU;
                self.mesh
                    .vertices
                    .push(v(b + (u * t.cos() + w * t.sin()) * rb, axis, cb));
            }
            for i in 0..sides {
                self.mesh
                    .indices
                    .extend_from_slice(&[center, center + 1 + i, center + 2 + i]);
            }
        }
    }
}

/// Unit icosphere with shared vertices.
pub fn icosphere(subdivisions: u32) -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut verts: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
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
        let mut cache = std::collections::HashMap::new();
        let mut mid = |a: u32, b: u32, verts: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                verts.push(((verts[a as usize] + verts[b as usize]) * 0.5).normalize());
                verts.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = mid(a, b, &mut verts);
            let bc = mid(b, c, &mut verts);
            let ca = mid(c, a, &mut verts);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    (verts, tris.into_iter().flatten().collect())
}

/// A lumpy blob of leaves. `normal_center` bends normals outward from the whole crown so the
/// canopy shades as one soft volume instead of many balls.
#[allow(clippy::too_many_arguments)]
fn leaf_blob(
    b: &mut Builder,
    center: Vec3,
    radius: Vec3,
    seed: u64,
    color: Vec3,
    crown_center: Vec3,
    crown_radius: f32,
    subdiv: u32,
) {
    let (verts, idx) = icosphere(subdiv);
    let base = b.base();
    for d in &verts {
        let n = fbm3(
            d.x as f64 * 1.7,
            d.y as f64 * 1.7,
            d.z as f64 * 1.7,
            seed,
            3,
            2.0,
            0.5,
        );
        let p = center + *d * radius * (1.0 + 0.32 * n);
        let outward = (p - crown_center).normalize_or(Vec3::Y);
        let normal = (*d * 0.35 + outward * 0.65).normalize();
        // Baked occlusion: darker inside and underneath the crown.
        let depth = ((p - crown_center).length() / crown_radius).clamp(0.0, 1.0);
        let ao = 0.35 + 0.65 * smoothstep(0.1, 1.0, depth * 0.7 + (outward.y * 0.5 + 0.5) * 0.45);
        let tint = 1.0
            + 0.18
                * perlin3(
                    p.x as f64 * 0.7,
                    p.y as f64 * 0.7,
                    p.z as f64 * 0.7,
                    seed + 3,
                );
        b.mesh.vertices.push(
            Vertex::new(p, normal, Vec2::new(d.x, d.y)).with_color((color * ao * tint).extend(1.0)),
        );
    }
    b.mesh.indices.extend(idx.iter().map(|i| i + base));
}

/// Kinds of tree in the valley.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKind {
    Oak,
    Spruce,
    Birch,
    Bush,
}

/// One tree: trunk (bark material) and crown (leaf material) meshes, with the crown's height
/// for placement.
pub struct TreeMeshes {
    pub trunk: Mesh,
    pub crown: Mesh,
}

fn bark_color(rng: &mut Rng) -> Vec3 {
    Vec3::new(0.085, 0.065, 0.045) * rng.range(0.8, 1.2)
}

/// Branch recursively; returns the tips where leaf clumps go.
#[allow(clippy::too_many_arguments)]
fn branch(
    b: &mut Builder,
    rng: &mut Rng,
    start: Vec3,
    dir: Vec3,
    length: f32,
    radius: f32,
    depth: u32,
    color: Vec3,
    tips: &mut Vec<(Vec3, f32)>,
) {
    let end = start + dir * length;
    let end_r = radius * 0.62;
    b.tube(
        start,
        end,
        radius,
        end_r,
        if depth == 0 { 9 } else { 5 },
        color,
        color,
        false,
    );
    if depth >= 2 {
        tips.push((end, length));
        return;
    }
    let children = if depth == 0 {
        rng.range_i32(3, 6)
    } else {
        rng.range_i32(2, 4)
    };
    let phase = rng.range(0.0, TAU);
    for i in 0..children {
        let a = phase + i as f32 / children as f32 * TAU + rng.range(-0.3, 0.3);
        let tilt = rng.range(0.45, 0.95) - depth as f32 * 0.05;
        let side = Vec3::new(a.cos(), 0.0, a.sin());
        let child_dir = (dir * tilt.cos() + side * tilt.sin() + Vec3::Y * 0.25).normalize();
        let from = start + dir * length * rng.range(0.7, 1.0);
        let child_len = length * rng.range(0.55, 0.75);
        branch(
            b,
            rng,
            from,
            child_dir,
            child_len,
            end_r * 0.8,
            depth + 1,
            color,
            tips,
        );
    }
}

/// Build a tree (deterministic per seed).
pub fn tree(kind: TreeKind, seed: u64) -> TreeMeshes {
    let mut rng = Rng::new(seed);
    let mut trunk = Builder::default();
    let mut crown = Builder::default();
    match kind {
        TreeKind::Oak => {
            let bark = bark_color(&mut rng);
            let h = rng.range(3.2, 4.5);
            let r = rng.range(0.28, 0.42);
            let lean = Vec3::new(rng.range(-0.08, 0.08), 1.0, rng.range(-0.08, 0.08)).normalize();
            // Flared root base.
            trunk.tube(
                Vec3::new(0.0, -0.3, 0.0),
                Vec3::new(0.0, 0.6, 0.0),
                r * 1.6,
                r,
                9,
                bark * 0.8,
                bark,
                false,
            );
            let mut tips = Vec::new();
            branch(
                &mut trunk,
                &mut rng,
                Vec3::new(0.0, 0.5, 0.0),
                lean,
                h,
                r,
                0,
                bark,
                &mut tips,
            );
            let green = [
                Vec3::new(0.05, 0.11, 0.025),
                Vec3::new(0.07, 0.13, 0.03),
                Vec3::new(0.085, 0.13, 0.025),
            ][rng.range_i32(0, 3) as usize];
            let (min, max) = tips.iter().fold(
                (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |(a, b), (p, _)| (a.min(*p), b.max(*p)),
            );
            let crown_center = (min + max) * 0.5 + Vec3::Y * 0.6;
            let crown_radius = ((max - min).length() * 0.5).max(2.5) + 1.5;
            for (i, (p, len)) in tips.iter().enumerate() {
                let s = rng.range(1.3, 1.9) + len * 0.25;
                let c = green * rng.range(0.85, 1.15);
                leaf_blob(
                    &mut crown,
                    *p + Vec3::Y * 0.3,
                    Vec3::new(s * 1.15, s * 0.8, s * 1.15),
                    seed * 31 + i as u64,
                    c,
                    crown_center,
                    crown_radius,
                    1,
                );
            }
            // Fill the middle of the crown.
            leaf_blob(
                &mut crown,
                crown_center,
                Vec3::splat(crown_radius * 0.55),
                seed * 7,
                green * 0.8,
                crown_center,
                crown_radius,
                1,
            );
        }
        TreeKind::Birch => {
            let h = rng.range(9.0, 13.0);
            let white = Vec3::new(0.62, 0.6, 0.55);
            let segs = 8;
            let lean = Vec3::new(rng.range(-0.1, 0.1), 1.0, rng.range(-0.1, 0.1)).normalize();
            for i in 0..segs {
                let t0 = i as f32 / segs as f32;
                let t1 = (i + 1) as f32 / segs as f32;
                let bend = |t: f32| lean * h * t + Vec3::new((t * 3.0).sin() * 0.15, 0.0, 0.0);
                let dark = if rng.chance(0.4) { 0.25 } else { 1.0 };
                trunk.tube(
                    bend(t0),
                    bend(t1),
                    0.17 * (1.0 - t0 * 0.7),
                    0.17 * (1.0 - t1 * 0.7),
                    7,
                    white * dark,
                    white,
                    false,
                );
            }
            let green = Vec3::new(0.12, 0.17, 0.04) * rng.range(0.9, 1.1);
            let crown_center = lean * h * 0.72;
            let crown_radius = h * 0.32;
            for i in 0..9 {
                let t = rng.range(0.45, 1.0);
                let a = rng.range(0.0, TAU);
                let off = Vec3::new(a.cos(), 0.0, a.sin()) * rng.range(0.5, 1.8) * (1.2 - t);
                let p = lean * h * t + off;
                let s = rng.range(1.0, 1.6);
                trunk.tube(
                    lean * h * (t - 0.12),
                    p,
                    0.05,
                    0.02,
                    4,
                    white * 0.5,
                    white * 0.5,
                    false,
                );
                leaf_blob(
                    &mut crown,
                    p,
                    Vec3::new(s, s * 1.2, s),
                    seed * 13 + i,
                    green,
                    crown_center,
                    crown_radius,
                    1,
                );
            }
        }
        TreeKind::Spruce => {
            let h = rng.range(13.0, 22.0);
            let bark = bark_color(&mut rng) * 0.8;
            trunk.tube(
                Vec3::new(0.0, -0.3, 0.0),
                Vec3::new(0.0, h, 0.0),
                h * 0.022,
                0.03,
                7,
                bark,
                bark,
                false,
            );
            let green = Vec3::new(0.025, 0.06, 0.03) * rng.range(0.85, 1.15);
            let base_y = rng.range(1.2, 2.5);
            let tiers = (h / 1.6) as u32;
            let max_r = h * rng.range(0.2, 0.26);
            for t in 0..tiers {
                let f = t as f32 / tiers as f32;
                let y = base_y + (h - base_y) * f;
                let r = max_r * (1.0 - f).powf(0.9) + 0.25;
                let droop = r * 0.45;
                let top = y + r * 0.55 + 0.4;
                let k = 9 + (r * 2.0) as u32;
                let phase = rng.range(0.0, TAU);
                let crown_center = Vec3::new(0.0, y, 0.0);
                // A star-shaped skirt: tips at r, notches at 0.6r, apex up the trunk, closed
                // underneath.
                let base = crown.base();
                crown.mesh.vertices.push(
                    Vertex::new(Vec3::new(0.0, top, 0.0), Vec3::Y, Vec2::ZERO)
                        .with_color((green * 1.15).extend(1.0)),
                );
                crown.mesh.vertices.push(
                    Vertex::new(Vec3::new(0.0, y - droop * 0.4, 0.0), -Vec3::Y, Vec2::ZERO)
                        .with_color((green * 0.3).extend(1.0)),
                );
                for i in 0..k * 2 {
                    let a = phase + i as f32 / (k * 2) as f32 * TAU;
                    let tip = i % 2 == 0;
                    let rr = if tip {
                        r * rng.range(0.9, 1.1)
                    } else {
                        r * 0.6
                    };
                    let yy = y - if tip { droop } else { droop * 0.6 };
                    let p = Vec3::new(a.cos() * rr, yy, a.sin() * rr);
                    let n = ((p - crown_center).normalize() * 0.6 + Vec3::Y * 0.5).normalize();
                    let shade = if tip { 1.0 } else { 0.7 } * (0.6 + 0.4 * f);
                    crown.mesh.vertices.push(
                        Vertex::new(p, n, Vec2::new(a, 0.0))
                            .with_color((green * shade * 1.2).extend(1.0)),
                    );
                }
                let ring = k * 2;
                for i in 0..ring {
                    let a = base + 2 + i;
                    let b2 = base + 2 + (i + 1) % ring;
                    // Top (apex → ring, counter-clockwise from above).
                    crown.mesh.indices.extend_from_slice(&[base, b2, a]);
                    // Underside.
                    crown.mesh.indices.extend_from_slice(&[base + 1, a, b2]);
                }
            }
        }
        TreeKind::Bush => {
            let green = Vec3::new(0.05, 0.1, 0.03) * rng.range(0.8, 1.2);
            let n = rng.range_i32(3, 6);
            let crown_center = Vec3::new(0.0, 0.6, 0.0);
            for i in 0..n {
                let a = rng.range(0.0, TAU);
                let d = rng.range(0.2, 0.9);
                let s = rng.range(0.6, 1.0);
                leaf_blob(
                    &mut crown,
                    Vec3::new(a.cos() * d, s * 0.6, a.sin() * d),
                    Vec3::new(s, s * 0.75, s),
                    seed * 17 + i as u64,
                    green,
                    crown_center,
                    1.6,
                    1,
                );
            }
        }
    }
    TreeMeshes {
        trunk: trunk.finish(),
        crown: crown.finish(),
    }
}

/// Cheap stand-in for a tree far away: one blob or cone, about 1/20 of the triangles.
pub fn tree_far(kind: TreeKind, seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = Builder::default();
    match kind {
        TreeKind::Spruce => {
            let h = rng.range(13.0, 22.0);
            let green = Vec3::new(0.025, 0.06, 0.03) * 1.1;
            b.tube(
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, h, 0.0),
                h * 0.23,
                0.05,
                8,
                green * 0.6,
                green,
                false,
            );
        }
        TreeKind::Birch => {
            let c = Vec3::new(0.12, 0.17, 0.04);
            leaf_blob(
                &mut b,
                Vec3::new(0.0, 7.5, 0.0),
                Vec3::new(2.2, 3.5, 2.2),
                seed,
                c,
                Vec3::new(0.0, 7.5, 0.0),
                3.5,
                0,
            );
            b.tube(
                Vec3::ZERO,
                Vec3::new(0.0, 5.0, 0.0),
                0.17,
                0.1,
                4,
                Vec3::splat(0.6),
                Vec3::splat(0.6),
                false,
            );
        }
        TreeKind::Oak | TreeKind::Bush => {
            let c = Vec3::new(0.065, 0.12, 0.028);
            let y = if kind == TreeKind::Oak { 6.5 } else { 0.8 };
            let r = if kind == TreeKind::Oak { 4.0 } else { 1.2 };
            leaf_blob(
                &mut b,
                Vec3::new(0.0, y, 0.0),
                Vec3::new(r, r * 0.8, r),
                seed,
                c,
                Vec3::new(0.0, y, 0.0),
                r,
                0,
            );
            if kind == TreeKind::Oak {
                let bark = Vec3::new(0.08, 0.06, 0.04);
                b.tube(
                    Vec3::ZERO,
                    Vec3::new(0.0, y, 0.0),
                    0.35,
                    0.2,
                    5,
                    bark,
                    bark,
                    false,
                );
            }
        }
    }
    b.finish()
}

/// A square patch of grass blades (`size` m across, centered, base at y = 0). Blades are
/// single tapered triangles, dark at the root and sunlit at the tip.
pub fn grass_patch(seed: u64, size: f32, blades: u32, dry: f32) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut mesh = Mesh::default();
    for _ in 0..blades {
        let x = rng.range(-0.5, 0.5) * size;
        let z = rng.range(-0.5, 0.5) * size;
        let h = rng.range(0.2, 0.5) * if rng.chance(0.08) { 1.6 } else { 1.0 };
        let w = rng.range(0.04, 0.075);
        let a = rng.range(0.0, TAU);
        let lean = rng.range(0.05, 0.35);
        let side = Vec3::new(a.cos(), 0.0, a.sin());
        let fwd = Vec3::new(-a.sin(), 0.0, a.cos());
        let base = Vec3::new(x, 0.0, z);
        let tip = base + Vec3::Y * h + fwd * h * lean;
        let n = (fwd * -0.3 + Vec3::Y).normalize();
        let lush = Vec3::new(0.06, 0.15, 0.03);
        let gold = Vec3::new(0.2, 0.19, 0.06);
        let col = lush.lerp(gold, (dry + rng.range(-0.25, 0.25)).clamp(0.0, 1.0));
        let root = col * 0.8;
        let i = mesh.vertices.len() as u32;
        mesh.vertices.push(
            Vertex::new(base - side * w, n, Vec2::new(0.0, 0.0)).with_color(root.extend(1.0)),
        );
        mesh.vertices.push(
            Vertex::new(base + side * w, n, Vec2::new(1.0, 0.0)).with_color(root.extend(1.0)),
        );
        mesh.vertices
            .push(Vertex::new(tip, n, Vec2::new(0.5, 1.0)).with_color((col * 1.4).extend(1.0)));
        mesh.indices.extend_from_slice(&[i, i + 1, i + 2]);
    }
    mesh
}

/// Wildflowers: thin stems with small colored heads, scattered over a patch.
pub fn flower_patch(seed: u64, size: f32, count: u32, color: Vec3) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = Builder::default();
    for _ in 0..count {
        let x = rng.range(-0.5, 0.5) * size;
        let z = rng.range(-0.5, 0.5) * size;
        let h = rng.range(0.25, 0.5);
        let stem = Vec3::new(0.05, 0.1, 0.03);
        b.tube(
            Vec3::new(x, 0.0, z),
            Vec3::new(x, h, z),
            0.008,
            0.006,
            3,
            stem,
            stem,
            false,
        );
        let c = color * rng.range(0.85, 1.15);
        let r = rng.range(0.03, 0.05);
        let p = Vec3::new(x, h, z);
        // A small flat star of petals.
        for k in 0..5 {
            let a = k as f32 / 5.0 * TAU + rng.range(0.0, 0.5);
            let d = Vec3::new(a.cos(), 0.15, a.sin());
            let e = Vec3::new((a + 0.6).cos(), 0.15, (a + 0.6).sin());
            b.tri([p, p + e * r * 1.8, p + d * r * 1.8], c);
        }
    }
    b.finish()
}

/// Parts of a house, one mesh per material.
pub struct HouseMeshes {
    pub walls: Mesh,
    pub wood: Mesh,
    pub roof: Mesh,
    pub windows: Mesh,
    /// Footprint half extents (x, z) for placement.
    pub half: Vec2,
}

/// A half-timbered village house, door facing +Z, origin at the ground under its center.
pub fn house(seed: u64) -> HouseMeshes {
    let mut rng = Rng::new(seed);
    let w = rng.range(6.0, 8.5);
    let d = rng.range(8.0, 11.0);
    let floors = if rng.chance(0.6) { 2 } else { 1 };
    let floor_h = 3.0;
    let wall_h = floors as f32 * floor_h;
    let pitch = rng.range(0.7, 0.95);
    let overhang = 0.5;
    let plaster = [
        Vec3::new(0.62, 0.58, 0.5),
        Vec3::new(0.66, 0.6, 0.45),
        Vec3::new(0.55, 0.52, 0.47),
        Vec3::new(0.6, 0.48, 0.38),
    ][rng.range_i32(0, 4) as usize];
    let stone = Vec3::new(0.2, 0.19, 0.18);
    let timber = Vec3::new(0.07, 0.045, 0.03);
    let roof_c = [
        Vec3::new(0.25, 0.08, 0.05),
        Vec3::new(0.12, 0.11, 0.12),
        Vec3::new(0.2, 0.1, 0.06),
    ][rng.range_i32(0, 3) as usize];

    let mut walls = Builder::default();
    let mut wood = Builder::default();
    let mut roof = Builder::default();
    let mut windows = Builder::default();
    let (hw, hd) = (w * 0.5, d * 0.5);
    // Stone plinth and plaster body.
    walls.aabb(
        Vec3::new(-hw - 0.1, -1.0, -hd - 0.1),
        Vec3::new(hw + 0.1, 0.7, hd + 0.1),
        stone,
    );
    walls.aabb(Vec3::new(-hw, 0.7, -hd), Vec3::new(hw, wall_h, hd), plaster);
    // Gable ends (triangles above the walls on ±Z).
    let ridge = wall_h + hw * pitch;
    for (z, flip) in [(hd, false), (-hd, true)] {
        let a = Vec3::new(-hw, wall_h, z);
        let b = Vec3::new(hw, wall_h, z);
        let c = Vec3::new(0.0, ridge, z);
        if flip {
            walls.tri([b, a, c], plaster);
        } else {
            walls.tri([a, b, c], plaster);
        }
        // Gable timbers.
        let out = if flip { -0.06 } else { 0.06 };
        let rot_l = Quat::from_rotation_z(pitch.atan());
        let rot_r = Quat::from_rotation_z(-pitch.atan());
        let len = (hw * hw + (hw * pitch).powi(2)).sqrt();
        wood.cuboid(
            Vec3::new(-hw * 0.5, wall_h + hw * pitch * 0.5, z + out),
            Vec3::new(len * 0.5, 0.1, 0.06),
            rot_l,
            timber,
        );
        wood.cuboid(
            Vec3::new(hw * 0.5, wall_h + hw * pitch * 0.5, z + out),
            Vec3::new(len * 0.5, 0.1, 0.06),
            rot_r,
            timber,
        );
        wood.cuboid(
            Vec3::new(0.0, wall_h + hw * pitch * 0.45, z + out),
            Vec3::new(0.09, hw * pitch * 0.45, 0.06),
            Quat::IDENTITY,
            timber,
        );
    }
    // Timber frame: corner posts, floor beams, braces.
    for (x, z) in [(-hw, -hd), (hw, -hd), (-hw, hd), (hw, hd)] {
        wood.aabb(
            Vec3::new(x - 0.12, 0.7, z - 0.12),
            Vec3::new(x + 0.12, wall_h, z + 0.12),
            timber,
        );
    }
    for f in 1..=floors {
        let y = f as f32 * floor_h;
        wood.aabb(
            Vec3::new(-hw - 0.08, y - 0.2, hd - 0.02),
            Vec3::new(hw + 0.08, y, hd + 0.08),
            timber,
        );
        wood.aabb(
            Vec3::new(-hw - 0.08, y - 0.2, -hd - 0.08),
            Vec3::new(hw + 0.08, y, -hd + 0.02),
            timber,
        );
        wood.aabb(
            Vec3::new(hw - 0.02, y - 0.2, -hd - 0.08),
            Vec3::new(hw + 0.08, y, hd + 0.08),
            timber,
        );
        wood.aabb(
            Vec3::new(-hw - 0.08, y - 0.2, -hd - 0.08),
            Vec3::new(-hw + 0.02, y, hd + 0.08),
            timber,
        );
    }
    // Windows on all four sides, a door at the front.
    let glass = Vec3::new(1.0, 1.0, 1.0);
    let window =
        |cx: f32, cy: f32, z_or_x: f32, side: u8, wood: &mut Builder, windows: &mut Builder| {
            let (ww, wh) = (0.55, 0.7);
            match side {
                0 | 1 => {
                    let s = if side == 0 { 1.0 } else { -1.0 };
                    windows.aabb(
                        Vec3::new(cx - ww, cy - wh, z_or_x - 0.02),
                        Vec3::new(cx + ww, cy + wh, z_or_x + 0.04 * s),
                        glass,
                    );
                    wood.aabb(
                        Vec3::new(cx - ww - 0.1, cy - wh - 0.12, z_or_x - 0.02),
                        Vec3::new(cx + ww + 0.1, cy - wh, z_or_x + 0.12 * s),
                        timber,
                    );
                    wood.aabb(
                        Vec3::new(cx - 0.04, cy - wh, z_or_x - 0.02),
                        Vec3::new(cx + 0.04, cy + wh, z_or_x + 0.06 * s),
                        timber,
                    );
                }
                _ => {
                    let s = if side == 2 { 1.0 } else { -1.0 };
                    windows.aabb(
                        Vec3::new(z_or_x - 0.02, cy - wh, cx - ww),
                        Vec3::new(z_or_x + 0.04 * s, cy + wh, cx + ww),
                        glass,
                    );
                    wood.aabb(
                        Vec3::new(z_or_x - 0.02, cy - wh - 0.12, cx - ww - 0.1),
                        Vec3::new(z_or_x + 0.12 * s, cy - wh, cx + ww + 0.1),
                        timber,
                    );
                }
            }
        };
    for f in 0..floors {
        let cy = 0.7 + f as f32 * floor_h + 1.4;
        let nx = ((w - 2.0) / 2.2) as i32;
        for i in 0..nx.max(1) {
            let cx = -hw + 1.6 + i as f32 * ((w - 3.2) / (nx - 1).max(1) as f32);
            if f == 0 && cx.abs() < 1.2 {
                continue;
            }
            window(cx, cy, hd, 0, &mut wood, &mut windows);
            window(cx, cy, -hd, 1, &mut wood, &mut windows);
        }
        let nz = ((d - 2.0) / 2.6) as i32;
        for i in 0..nz.max(1) {
            let cz = -hd + 1.8 + i as f32 * ((d - 3.6) / (nz - 1).max(1) as f32);
            window(cz, cy, hw, 2, &mut wood, &mut windows);
            window(cz, cy, -hw, 3, &mut wood, &mut windows);
        }
    }
    wood.aabb(
        Vec3::new(-0.6, 0.0, hd - 0.02),
        Vec3::new(0.6, 2.3, hd + 0.1),
        timber * 1.5,
    );
    wood.aabb(
        Vec3::new(-0.9, 0.0, hd),
        Vec3::new(0.9, 0.25, hd + 0.9),
        stone,
    );
    // Roof: two slabs from the ridge past the eaves.
    let slope_len = ((hw + overhang).powi(2) + ((hw + overhang) * pitch).powi(2)).sqrt();
    let ang = pitch.atan();
    for s in [-1.0f32, 1.0] {
        let rot = Quat::from_rotation_z(-s * ang);
        let center = Vec3::new(
            s * (hw + overhang) * 0.5,
            ridge - (hw + overhang) * pitch * 0.5 + 0.12,
            0.0,
        );
        roof.cuboid(
            center,
            Vec3::new(slope_len * 0.5 + 0.1, 0.12, hd + overhang),
            rot,
            roof_c,
        );
        // Tile rows: thin ridges across the slope for texture.
        let rows = (slope_len / 0.45) as i32;
        for r in 0..rows {
            let t = (r as f32 + 0.5) / rows as f32 - 0.5;
            let p = center + rot * Vec3::new(t * slope_len, 0.14, 0.0);
            let shade = roof_c * (0.8 + 0.4 * ((r * 7 % 5) as f32 / 5.0));
            roof.cuboid(p, Vec3::new(0.05, 0.03, hd + overhang), rot, shade);
        }
    }
    roof.aabb(
        Vec3::new(-0.2, ridge - 0.1, -hd - overhang),
        Vec3::new(0.2, ridge + 0.25, hd + overhang),
        roof_c * 0.7,
    );
    // Chimney.
    let cx = rng.range(-hw * 0.5, hw * 0.5);
    let cz = rng.range(-hd * 0.5, hd * 0.5);
    walls.aabb(
        Vec3::new(cx - 0.4, wall_h, cz - 0.4),
        Vec3::new(cx + 0.4, ridge + 1.2, cz + 0.4),
        stone * 1.2,
    );
    HouseMeshes {
        walls: walls.finish(),
        wood: wood.finish(),
        roof: roof.finish(),
        windows: windows.finish(),
        half: Vec2::new(hw + overhang, hd + overhang),
    }
}

/// A wrought-iron street lamp, 4 m tall; the lantern's glass is the second mesh.
pub fn lamp_post() -> (Mesh, Mesh) {
    let mut iron = Builder::default();
    let mut glass = Builder::default();
    let c = Vec3::splat(0.03);
    iron.tube(
        Vec3::ZERO,
        Vec3::new(0.0, 0.4, 0.0),
        0.16,
        0.1,
        8,
        c,
        c,
        false,
    );
    iron.tube(
        Vec3::new(0.0, 0.4, 0.0),
        Vec3::new(0.0, 3.6, 0.0),
        0.06,
        0.045,
        8,
        c,
        c,
        false,
    );
    iron.aabb(
        Vec3::new(-0.22, 3.55, -0.22),
        Vec3::new(0.22, 3.62, 0.22),
        c,
    );
    iron.tube(
        Vec3::new(0.0, 4.15, 0.0),
        Vec3::new(0.0, 4.45, 0.0),
        0.28,
        0.02,
        8,
        c,
        c,
        true,
    );
    glass.aabb(
        Vec3::new(-0.17, 3.62, -0.17),
        Vec3::new(0.17, 4.15, 0.17),
        Vec3::ONE,
    );
    (iron.finish(), glass.finish())
}

/// Tileable value noise in about [0, 1] on a lattice that repeats every `period` cells.
fn tile_noise(x: f32, y: f32, period: i64, seed: u64) -> f32 {
    use engine::core::math::hash01;
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let h = |i: i64, j: i64| hash01(i.rem_euclid(period), j.rem_euclid(period), 0, seed);
    let a = h(xi, yi) + (h(xi + 1, yi) - h(xi, yi)) * sx;
    let b = h(xi, yi + 1) + (h(xi + 1, yi + 1) - h(xi, yi + 1)) * sx;
    a + (b - a) * sy
}

/// Tileable fractal noise over `u, v` in [0, 1), about [0, 1].
fn tile_fbm(u: f32, v: f32, base: i64, octaves: u32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for k in 0..octaves {
        let p = base << k;
        sum += amp * tile_noise(u * p as f32, v * p as f32, p, seed + k as u64);
        norm += amp;
        amp *= 0.55;
    }
    sum / norm
}

/// Ground detail textures that tile: a brightness map (multiplied with the terrain's vertex
/// colors) and a matching normal map of small bumps, clumps and pebbles.
pub fn ground_detail(size: u32) -> (Image, Image) {
    let n = size as usize;
    let mut height = vec![0.0f32; n * n];
    let mut albedo = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let clumps = tile_fbm(u, v, 8, 5, 11);
            let grain = tile_fbm(u, v, 64, 2, 12);
            let h = clumps * 0.7 + grain * 0.3;
            height[y * n + x] = h;
            let b = (0.62 + 0.42 * clumps + 0.12 * (grain - 0.5)).clamp(0.0, 1.0);
            let c = (b * 255.0) as u8;
            albedo.extend_from_slice(&[c, c, c, 255]);
        }
    }
    let mut normals = Vec::with_capacity(n * n * 4);
    let at = |x: isize, y: isize| {
        height[(y.rem_euclid(n as isize) as usize) * n + x.rem_euclid(n as isize) as usize]
    };
    for y in 0..n as isize {
        for x in 0..n as isize {
            let dx = (at(x + 1, y) - at(x - 1, y)) * 6.0;
            let dy = (at(x, y + 1) - at(x, y - 1)) * 6.0;
            let nrm = Vec3::new(-dx, dy, 1.0).normalize();
            let e = |c: f32| ((c * 0.5 + 0.5) * 255.0) as u8;
            normals.extend_from_slice(&[e(nrm.x), e(nrm.y), e(nrm.z), 255]);
        }
    }
    (
        Image::new(size, size, albedo, false),
        Image::new(size, size, normals, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_are_valid() {
        let check = |m: &Mesh, name: &str| {
            assert!(!m.indices.is_empty(), "{name} is empty");
            assert!(
                m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
                "{name} has bad indices"
            );
            assert!(
                m.vertices
                    .iter()
                    .all(|v| v.position.iter().all(|c| c.is_finite())),
                "{name} has NaNs"
            );
        };
        for kind in [TreeKind::Oak, TreeKind::Spruce, TreeKind::Birch] {
            let t = tree(kind, 3);
            check(&t.trunk, "trunk");
            check(&t.crown, "crown");
            check(&tree_far(kind, 3), "far tree");
        }
        check(&tree(TreeKind::Bush, 1).crown, "bush");
        check(&grass_patch(1, 4.0, 300, 0.3), "grass");
        check(&flower_patch(1, 4.0, 20, Vec3::X), "flowers");
        let h = house(9);
        check(&h.walls, "walls");
        check(&h.roof, "roof");
        check(&h.windows, "windows");
        let (a, b) = lamp_post();
        check(&a, "lamp");
        check(&b, "lantern");
    }

    #[test]
    fn spruce_skirts_face_up() {
        let t = tree(TreeKind::Spruce, 4);
        let m = &t.crown;
        let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
        let tri = &m.indices[0..3];
        let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
        assert!(n.y > 0.0);
    }
}
