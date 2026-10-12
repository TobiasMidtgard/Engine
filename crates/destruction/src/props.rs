//! Detailed procedural props for the destruction range: crates, drums, sandbags, barriers,
//! pillars, beams, columns, timbers, roofs, pallets, scaffolding and fences.
//!
//! Every prop is built in meters with real-world proportions, Y up, centered on the origin
//! (the middle of its bounding box), and returned as one [`PropPart`] per
//! [`DestructibleMaterial`], so each part draws with that material's textured surface.
//!
//! - **Edges** are bevelled or chamfered everywhere (no razor edges); curved surfaces have
//!   smooth normals, sharp edges hard normals.
//! - **Vertex colors** are near white: the texture supplies the color. They carry per-board
//!   tint variation, weathering, and ambient occlusion baked by ray casting against the prop
//!   itself and the ground under it (dark gaps, crevices, contact shadows).
//! - **UVs** are in meters. On wood, `u` runs along the grain of every board.
//!
//! ```ignore
//! let parts = props::wooden_crate(Vec3::new(1.0, 0.8, 0.8), 7);
//! for part in parts {
//!     let mesh = meshes.add(part.mesh.build());
//!     commands.spawn((transform, MeshRenderer::new(mesh, surface[part.material.index()].clone())));
//! }
//! ```

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::noise::{fbm3, perlin3};
use engine_core::math::{Mat3, Rng, Vec2, Vec3, hash01, lerp, smoothstep};
use engine_render::Vertex;
use serde::{Deserialize, Serialize};

use crate::material::DestructibleMaterial;
use crate::meshing::{MeshBuilder, box_uv};

const WOOD: DestructibleMaterial = DestructibleMaterial::Wood;
const METAL: DestructibleMaterial = DestructibleMaterial::Metal;
const CONCRETE: DestructibleMaterial = DestructibleMaterial::Concrete;
const STONE: DestructibleMaterial = DestructibleMaterial::Stone;
const DIRT: DestructibleMaterial = DestructibleMaterial::Dirt;

// ============================================================================================
// Public API
// ============================================================================================

/// One material's share of a prop: draw `mesh` with the surface of `material`.
#[derive(Debug, Clone)]
pub struct PropPart {
    pub material: DestructibleMaterial,
    pub mesh: MeshBuilder,
}

/// Every prop this module builds, with its parameters (meters, radians), so scenes and AI
/// agents can name one as data: `{"wooden_crate": {"size": [1.0, 0.8, 0.8]}}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKind {
    WoodenCrate {
        size: [f32; 3],
    },
    MilitaryCrate {
        size: [f32; 3],
    },
    SteelDrum {
        dents: u32,
    },
    Sandbag,
    SandbagWall {
        length: f32,
        courses: u32,
    },
    JerseyBarrier,
    ConcretePillar {
        size: [f32; 3],
    },
    SteelIBeam {
        length: f32,
        depth: f32,
        end_plates: bool,
    },
    StoneColumn {
        height: f32,
        radius: f32,
    },
    Timber {
        size: [f32; 3],
    },
    /// A dressed ashlar block: chamfered arrises, tooled faces, chipped corners.
    StoneBlock {
        size: [f32; 3],
    },
    RoofTiles {
        width: f32,
        depth: f32,
        pitch: f32,
    },
    /// One flat roof panel: `size` X down the slope, Y thick, Z along the ridge; the
    /// shingle butts (the eave) at +X, or at −X when `eave_at_plus_x` is false.
    ShinglePanel {
        size: [f32; 3],
        eave_at_plus_x: bool,
    },
    Pallet,
    ScaffoldTower,
    ChainLinkFence {
        length: f32,
    },
}

impl PropKind {
    /// One of every prop with typical parameters.
    pub fn catalog() -> Vec<PropKind> {
        vec![
            PropKind::WoodenCrate {
                size: [1.0, 0.8, 0.8],
            },
            PropKind::MilitaryCrate {
                size: [1.0, 0.45, 0.5],
            },
            PropKind::SteelDrum { dents: 2 },
            PropKind::Sandbag,
            PropKind::SandbagWall {
                length: 2.5,
                courses: 4,
            },
            PropKind::JerseyBarrier,
            PropKind::ConcretePillar {
                size: [0.6, 3.0, 0.6],
            },
            PropKind::SteelIBeam {
                length: 4.0,
                depth: 0.3,
                end_plates: true,
            },
            PropKind::StoneColumn {
                height: 4.0,
                radius: 0.3,
            },
            PropKind::Timber {
                size: [3.0, 0.2, 0.2],
            },
            PropKind::StoneBlock {
                size: [1.2, 0.98, 1.2],
            },
            PropKind::RoofTiles {
                width: 4.0,
                depth: 3.4,
                pitch: 35f32.to_radians(),
            },
            PropKind::ShinglePanel {
                size: [2.6, 0.12, 4.6],
                eave_at_plus_x: true,
            },
            PropKind::Pallet,
            PropKind::ScaffoldTower,
            PropKind::ChainLinkFence { length: 5.0 },
        ]
    }

    /// Build the prop's meshes (deterministic per `seed`).
    pub fn build(&self, seed: u64) -> Vec<PropPart> {
        match *self {
            PropKind::WoodenCrate { size } => wooden_crate(Vec3::from(size), seed),
            PropKind::MilitaryCrate { size } => military_crate(Vec3::from(size), seed),
            PropKind::SteelDrum { dents } => steel_drum(dents, seed),
            PropKind::Sandbag => sandbag(seed),
            PropKind::SandbagWall { length, courses } => sandbag_wall(length, courses, seed),
            PropKind::JerseyBarrier => jersey_barrier(seed),
            PropKind::ConcretePillar { size } => concrete_pillar(Vec3::from(size), seed),
            PropKind::SteelIBeam {
                length,
                depth,
                end_plates,
            } => steel_i_beam(length, depth, end_plates),
            PropKind::StoneColumn { height, radius } => stone_column(height, radius, seed),
            PropKind::Timber { size } => timber(Vec3::from(size), seed),
            PropKind::StoneBlock { size } => stone_block(Vec3::from(size), seed),
            PropKind::RoofTiles {
                width,
                depth,
                pitch,
            } => roof_tiles(width, depth, pitch, seed),
            PropKind::ShinglePanel {
                size,
                eave_at_plus_x,
            } => shingle_panel(Vec3::from(size), eave_at_plus_x, seed),
            PropKind::Pallet => pallet(seed),
            PropKind::ScaffoldTower => scaffold_tower(seed),
            PropKind::ChainLinkFence { length } => chain_link_fence(length, seed),
        }
    }

    /// Size of the box the prop fills, meters (centered on the origin).
    pub fn size(&self) -> Vec3 {
        match *self {
            PropKind::WoodenCrate { size }
            | PropKind::MilitaryCrate { size }
            | PropKind::ConcretePillar { size }
            | PropKind::Timber { size }
            | PropKind::StoneBlock { size } => Vec3::from(size),
            PropKind::SteelDrum { .. } => Vec3::new(DRUM_DIAMETER, DRUM_HEIGHT, DRUM_DIAMETER),
            PropKind::Sandbag => Vec3::new(BAG_LEN, BAG_HEIGHT, BAG_WIDTH),
            PropKind::SandbagWall { length, courses } => sandbag_wall_size(length, courses),
            PropKind::JerseyBarrier => Vec3::new(3.0, 0.81, 0.6),
            PropKind::SteelIBeam {
                length,
                depth,
                end_plates,
            } => i_beam_size(length, depth, end_plates),
            PropKind::StoneColumn { height, radius } => stone_column_size(height, radius),
            PropKind::RoofTiles {
                width,
                depth,
                pitch,
            } => roof_size(width, depth, pitch),
            PropKind::ShinglePanel { size, .. } => Vec3::from(size),
            PropKind::Pallet => Vec3::new(1.2, 0.144, 0.8),
            PropKind::ScaffoldTower => SCAFFOLD_SIZE,
            PropKind::ChainLinkFence { length } => Vec3::new(length.max(1.0), 2.0, 0.1),
        }
    }
}

/// Axis-aligned bounds `(min, max)` of all parts, meters.
pub fn prop_bounds(parts: &[PropPart]) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in parts {
        for v in &p.mesh.vertices {
            lo = lo.min(Vec3::from(v.position));
            hi = hi.max(Vec3::from(v.position));
        }
    }
    (lo, hi)
}

/// Total triangle count of all parts.
pub fn prop_triangles(parts: &[PropPart]) -> usize {
    parts.iter().map(|p| p.mesh.indices.len() / 3).sum()
}

// ============================================================================================
// Geometry kernel
// ============================================================================================

/// Rigid placement: local → prop coordinates.
#[derive(Debug, Clone, Copy)]
struct Frame {
    rot: Mat3,
    pos: Vec3,
}

impl Frame {
    const ID: Frame = Frame {
        rot: Mat3::IDENTITY,
        pos: Vec3::ZERO,
    };

    fn at(pos: Vec3) -> Self {
        Self {
            rot: Mat3::IDENTITY,
            pos,
        }
    }

    fn new(pos: Vec3, rot: Mat3) -> Self {
        Self { rot, pos }
    }

    /// Local X along `x`, local Z along `z` (made perpendicular to `x`).
    fn xz(pos: Vec3, x: Vec3, z: Vec3) -> Self {
        let x = x.normalize();
        let z = (z - x * z.dot(x)).normalize_or(x.any_orthonormal_vector());
        Self {
            rot: Mat3::from_cols(x, z.cross(x), z),
            pos,
        }
    }

    /// Local X along `x`, local Y as close to `up` as possible.
    fn along(pos: Vec3, x: Vec3, up: Vec3) -> Self {
        let x = x.normalize();
        let z = x.cross(up);
        let z = if z.length_squared() < 1e-8 {
            x.any_orthonormal_vector()
        } else {
            z.normalize()
        };
        Self::xz(pos, x, z)
    }

    /// Local Y along `axis` (for lathes and bolts).
    fn up_along(pos: Vec3, axis: Vec3) -> Self {
        let y = axis.normalize();
        let x = y.any_orthonormal_vector();
        Self {
            rot: Mat3::from_cols(x, y, x.cross(y)),
            pos,
        }
    }

    fn p(&self, v: Vec3) -> Vec3 {
        self.rot * v + self.pos
    }

    fn d(&self, v: Vec3) -> Vec3 {
        self.rot * v
    }

    fn then(&self, local: Frame) -> Frame {
        Frame {
            rot: self.rot * local.rot,
            pos: self.p(local.pos),
        }
    }
}

/// Builds the parts of one prop.
struct Kit {
    parts: Vec<PropPart>,
}

/// Ambient occlusion bake settings: rays reach `reach` meters, `strength` 0..1 darkens fully
/// occluded vertices, `ground` adds the ground plane under the prop.
#[derive(Debug, Clone, Copy)]
struct Ao {
    reach: f32,
    strength: f32,
    ground: bool,
}

impl Kit {
    fn new() -> Self {
        Self { parts: Vec::new() }
    }

    fn mb(&mut self, material: DestructibleMaterial) -> &mut MeshBuilder {
        let i = match self.parts.iter().position(|p| p.material == material) {
            Some(i) => i,
            None => {
                self.parts.push(PropPart {
                    material,
                    mesh: MeshBuilder::new(),
                });
                self.parts.len() - 1
            }
        };
        &mut self.parts[i].mesh
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        prop_bounds(&self.parts)
    }

    fn translate(&mut self, d: Vec3) {
        for p in &mut self.parts {
            p.mesh.translate(d);
        }
    }

    /// Move the middle of the bounding box to the origin.
    fn center(&mut self) {
        let (lo, hi) = self.bounds();
        self.translate(-(lo + hi) * 0.5);
    }

    fn finish(mut self, ao: Ao) -> Vec<PropPart> {
        self.parts.retain(|p| !p.mesh.is_empty());
        let ground = ao.ground.then(|| self.bounds().0.y);
        bake_ao(&mut self.parts, ground, ao.reach, ao.strength);
        self.parts
    }
}

fn vert(b: &mut MeshBuilder, p: Vec3, n: Vec3, uv: Vec2, c: [f32; 3]) -> u32 {
    let i = b.vertices.len() as u32;
    b.vertices.push(Vertex {
        position: p.to_array(),
        normal: n.to_array(),
        uv: uv.to_array(),
        color: [c[0], c[1], c[2], 1.0],
    });
    i
}

fn vpos(b: &MeshBuilder, i: u32) -> Vec3 {
    Vec3::from(b.vertices[i as usize].position)
}

/// Quad `q` (two triangles), wound so its front faces along `n`.
fn quad_idx(b: &mut MeshBuilder, q: [u32; 4], n: Vec3) {
    let [a, c1, c2, c3] = q.map(|i| vpos(b, i));
    let g = (c1 - a).cross(c2 - a) + (c2 - a).cross(c3 - a);
    if g.dot(n) >= 0.0 {
        b.indices
            .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    } else {
        b.indices
            .extend_from_slice(&[q[0], q[2], q[1], q[0], q[3], q[2]]);
    }
}

/// Triangle wound so its front faces along `n`.
fn tri_idx(b: &mut MeshBuilder, t: [u32; 3], n: Vec3) {
    let [a, c1, c2] = t.map(|i| vpos(b, i));
    if (c1 - a).cross(c2 - a).dot(n) >= 0.0 {
        b.indices.extend_from_slice(&t);
    } else {
        b.indices.extend_from_slice(&[t[0], t[2], t[1]]);
    }
}

/// Multiply the colors of vertices `v0..` by `f(position, normal)`.
fn shade(b: &mut MeshBuilder, v0: usize, f: impl Fn(Vec3, Vec3) -> [f32; 3]) {
    for v in &mut b.vertices[v0..] {
        let k = f(Vec3::from(v.position), Vec3::from(v.normal));
        for (c, k) in v.color.iter_mut().zip(k) {
            *c *= k;
        }
    }
}

/// Move vertices `v0..` by `f(position)` (a function of position only, so vertices shared by
/// several faces move together and no cracks open).
fn deform(b: &mut MeshBuilder, v0: usize, f: impl Fn(Vec3) -> Vec3) {
    for v in &mut b.vertices[v0..] {
        v.position = f(Vec3::from(v.position)).to_array();
    }
}

/// Area-weighted smooth normals for vertices `v0..` from triangles `t0..` (index offsets),
/// merging vertices at the same position whose faces bend less than `crease_deg` (auto
/// smooth): UV seams and flat-built quads shade smoothly, sharp edges stay sharp.
fn auto_smooth(b: &mut MeshBuilder, v0: usize, t0: usize, crease_deg: f32) {
    let n = b.vertices.len() - v0;
    if n == 0 {
        return;
    }
    let mut acc = vec![Vec3::ZERO; n];
    for t in b.indices[t0..].as_chunks::<3>().0 {
        let [i, j, k] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let pa = Vec3::from(b.vertices[i].position);
        let pb = Vec3::from(b.vertices[j].position);
        let pc = Vec3::from(b.vertices[k].position);
        let f = (pb - pa).cross(pc - pa);
        for x in [i, j, k] {
            if x >= v0 {
                acc[x - v0] += f;
            }
        }
    }
    let mut groups: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for k in 0..n {
        let p = Vec3::from(b.vertices[v0 + k].position) * 2.0e4;
        groups
            .entry([p.x.round() as i32, p.y.round() as i32, p.z.round() as i32])
            .or_default()
            .push(k);
    }
    let cos = crease_deg.to_radians().cos();
    let dirs: Vec<Vec3> = acc.iter().map(|a| a.normalize_or_zero()).collect();
    for members in groups.values() {
        for &k in members {
            let mut sum = Vec3::ZERO;
            for &m in members {
                if dirs[k].dot(dirs[m]) >= cos {
                    sum += acc[m];
                }
            }
            let v = &mut b.vertices[v0 + k];
            v.normal = sum.normalize_or(Vec3::from(v.normal)).to_array();
        }
    }
}

/// Face bits of a box: +X, -X, +Y, -Y, +Z, -Z.
const PX: u8 = 1;
const NX: u8 = 2;
const PY: u8 = 4;
const NY: u8 = 8;
const PZ: u8 = 16;
const NZ: u8 = 32;
const ALL: u8 = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Rounded (smooth normals across the bevel).
    Round,
    /// 45° chamfer with hard normals.
    Chamfer,
}

/// A box with bevelled edges, `half` extents, local X along the grain.
#[derive(Debug, Clone, Copy)]
struct BoxSpec<'a> {
    half: Vec3,
    bevel: f32,
    edge: Edge,
    faces: u8,
    /// Extra grid lines per local axis (meters): vertices for baked occlusion and weathering.
    splits: [&'a [f32]; 3],
    uv: Vec2,
    tint: [f32; 3],
}

impl<'a> BoxSpec<'a> {
    fn new(half: Vec3, bevel: f32) -> Self {
        Self {
            half,
            bevel,
            edge: Edge::Round,
            faces: ALL,
            splits: [&[], &[], &[]],
            uv: Vec2::ZERO,
            tint: [1.0; 3],
        }
    }

    fn faces(mut self, f: u8) -> Self {
        self.faces = f;
        self
    }

    fn chamfer(mut self) -> Self {
        self.edge = Edge::Chamfer;
        self
    }

    fn splits(mut self, s: [&'a [f32]; 3]) -> Self {
        self.splits = s;
        self
    }

    fn tint(mut self, t: [f32; 3]) -> Self {
        self.tint = t;
        self
    }

    fn uv(mut self, o: Vec2) -> Self {
        self.uv = o;
        self
    }
}

/// Box with rounded or chamfered edges: every face is a grid whose outer rows wrap around the
/// bevel (so the bevel catches highlights and shades smoothly into the faces).
fn bevel_box(b: &mut MeshBuilder, f: &Frame, s: &BoxSpec) {
    let h = s.half.max(Vec3::splat(1e-4));
    let r = s.bevel.clamp(0.0, h.min_element() * 0.98);
    let inner = h - Vec3::splat(r);
    let lines: [Vec<f32>; 3] = std::array::from_fn(|a| {
        let mut v = vec![-h[a], h[a]];
        if r > 1e-6 {
            v.push(-inner[a]);
            v.push(inner[a]);
        }
        v.extend(
            s.splits[a]
                .iter()
                .copied()
                .filter(|x| x.abs() < inner[a] - 1e-4),
        );
        v.sort_by(f32::total_cmp);
        v.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
        v
    });
    for face in 0..6 {
        if s.faces & (1 << face) == 0 {
            continue;
        }
        let a = face / 2;
        let sg = if face % 2 == 0 { 1.0 } else { -1.0 };
        let (t1, t2) = if sg > 0.0 {
            ((a + 1) % 3, (a + 2) % 3)
        } else {
            ((a + 2) % 3, (a + 1) % 3)
        };
        let mut axis_n = Vec3::ZERO;
        axis_n[a] = sg;
        let map = |c1: f32, c2: f32| -> (Vec3, Vec3) {
            let mut q = Vec3::ZERO;
            q[a] = sg * h[a];
            q[t1] = c1;
            q[t2] = c2;
            let cl = q.clamp(-inner, inner);
            let d = q - cl;
            match s.edge {
                Edge::Round => {
                    let n = d.normalize_or(axis_n);
                    (cl + n * r, n)
                }
                Edge::Chamfer => {
                    let l1 = d.x.abs() + d.y.abs() + d.z.abs();
                    if l1 < 1e-9 {
                        (q, axis_n)
                    } else {
                        (cl + d * (r / l1), axis_n)
                    }
                }
            }
        };
        let (l1, l2) = (&lines[t1], &lines[t2]);
        let w = l1.len() as u32;
        let fn_ = f.d(axis_n);
        match s.edge {
            Edge::Round => {
                let base = b.vertices.len() as u32;
                for &c2 in l2 {
                    for &c1 in l1 {
                        let (p, n) = map(c1, c2);
                        vert(b, f.p(p), f.d(n), box_uv(p, axis_n) + s.uv, s.tint);
                    }
                }
                for j in 0..l2.len() as u32 - 1 {
                    for i in 0..w - 1 {
                        let i0 = base + j * w + i;
                        quad_idx(b, [i0, i0 + 1, i0 + 1 + w, i0 + w], fn_);
                    }
                }
            }
            Edge::Chamfer => {
                for j in 0..l2.len() - 1 {
                    for i in 0..l1.len() - 1 {
                        let p = [
                            map(l1[i], l2[j]).0,
                            map(l1[i + 1], l2[j]).0,
                            map(l1[i + 1], l2[j + 1]).0,
                            map(l1[i], l2[j + 1]).0,
                        ];
                        let n = (p[1] - p[0]).cross(p[2] - p[0]) + (p[2] - p[0]).cross(p[3] - p[0]);
                        if n.length_squared() < 1e-16 {
                            continue;
                        }
                        let n = n.normalize();
                        let idx =
                            p.map(|q| vert(b, f.p(q), f.d(n), box_uv(q, axis_n) + s.uv, s.tint));
                        quad_idx(b, idx, f.d(n));
                    }
                }
            }
        }
    }
}

/// Signed area of a closed 2D polygon (positive when counter-clockwise).
fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f32>() * 0.5
}

/// Ear-clipping triangulation of a simple polygon (either winding). Returns index triples.
fn triangulate(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = if signed_area(poly) >= 0.0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut out = Vec::new();
    let cross = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let mut guard = 0;
    while idx.len() > 3 && guard < 4 * n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 1e-12 {
                continue;
            }
            let blocked = idx.iter().any(|&j| {
                j != ia
                    && j != ib
                    && j != ic
                    && cross(a, b, poly[j]) > 1e-9
                    && cross(b, c, poly[j]) > 1e-9
                    && cross(c, a, poly[j]) > 1e-9
            });
            if blocked {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Only collinear or degenerate corners left: drop the flattest one.
            let m = idx.len();
            let k = (0..m)
                .min_by(|&x, &y| {
                    let f = |i: usize| {
                        cross(
                            poly[idx[(i + m - 1) % m]],
                            poly[idx[i]],
                            poly[idx[(i + 1) % m]],
                        )
                        .abs()
                    };
                    f(x).total_cmp(&f(y))
                })
                .unwrap_or(0);
            idx.remove(k);
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

/// Round every corner `i` of a closed polygon with radius `radii[i]` using `segs[i]` arc
/// segments (convex and concave corners alike).
fn fillet(pts: &[Vec2], radii: &[f32], segs: &[usize]) -> Vec<Vec2> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let p = pts[i];
        let a = pts[(i + n - 1) % n];
        let c = pts[(i + 1) % n];
        let (d0, d1) = ((p - a).normalize(), (c - p).normalize());
        let turn = d0.perp_dot(d1);
        let alpha = d0.dot(d1).clamp(-1.0, 1.0).acos();
        if radii[i] <= 1e-6 || alpha < 1e-3 || segs[i] == 0 {
            out.push(p);
            continue;
        }
        let half_tan = (alpha * 0.5).tan();
        let t = (radii[i] * half_tan)
            .min((p - a).length() * 0.49)
            .min((c - p).length() * 0.49);
        let r = t / half_tan;
        let t0 = p - d0 * t;
        let t1 = p + d1 * t;
        let side = if turn > 0.0 { d0.perp() } else { -d0.perp() };
        let center = t0 + side * r;
        let a0 = (t0 - center).to_angle();
        let a1 = (t1 - center).to_angle();
        let mut sweep = a1 - a0;
        if turn > 0.0 {
            while sweep < 0.0 {
                sweep += TAU;
            }
        } else {
            while sweep > 0.0 {
                sweep -= TAU;
            }
        }
        for k in 0..=segs[i] {
            let ang = a0 + sweep * k as f32 / segs[i] as f32;
            out.push(center + Vec2::from_angle(ang) * r);
        }
    }
    out
}

/// Rectangle `±half` with rounded corners (counter-clockwise from (-x, -y)); `radii` per
/// corner in the order (-x,-y), (+x,-y), (+x,+y), (-x,+y).
fn rounded_rect(half: Vec2, radii: [f32; 4], segs: usize) -> Vec<Vec2> {
    let corners = [
        Vec2::new(-half.x, -half.y),
        Vec2::new(half.x, -half.y),
        Vec2::new(half.x, half.y),
        Vec2::new(-half.x, half.y),
    ];
    let start = [PI, 1.5 * PI, 0.0, 0.5 * PI];
    let mut out = Vec::new();
    for c in 0..4 {
        let r = radii[c].min(half.x * 0.95).min(half.y * 0.95);
        if r <= 1e-6 {
            out.push(corners[c]);
            continue;
        }
        let center = corners[c] - corners[c].signum() * r;
        for k in 0..=segs {
            let a = start[c] + k as f32 / segs as f32 * FRAC_PI_2;
            out.push(center + Vec2::from_angle(a) * r);
        }
    }
    out
}

/// Octagon: rectangle `±half` with 45° chamfers of size `c` (counter-clockwise).
fn chamfered_rect(half: Vec2, c: f32) -> Vec<Vec2> {
    let (x, y) = (half.x, half.y);
    vec![
        Vec2::new(-x + c, -y),
        Vec2::new(x - c, -y),
        Vec2::new(x, -y + c),
        Vec2::new(x, y - c),
        Vec2::new(x - c, y),
        Vec2::new(-x + c, y),
        Vec2::new(-x, y - c),
        Vec2::new(-x, -y + c),
    ]
}

/// Sorted stations from `a` to `b` that include `must` and are at most `step` apart.
fn stations(a: f32, b: f32, must: &[f32], step: f32) -> Vec<f32> {
    let mut v = vec![a, b];
    v.extend(
        must.iter()
            .copied()
            .filter(|x| *x > a + 1e-4 && *x < b - 1e-4),
    );
    v.sort_by(f32::total_cmp);
    v.dedup_by(|x, y| (*x - *y).abs() < 1e-5);
    let mut out = vec![v[0]];
    for w in v.windows(2) {
        let n = ((w[1] - w[0]) / step).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(w[0] + (w[1] - w[0]) * k as f32 / n as f32);
        }
    }
    out
}

fn arc_lengths(p: &[Vec2]) -> Vec<f32> {
    let mut s = vec![0.0; p.len() + 1];
    for i in 0..p.len() {
        s[i + 1] = s[i] + p[i].distance(p[(i + 1) % p.len()]);
    }
    s
}

/// Loft through closed profiles with the same point count: ring `k` holds points `(z, y)` at
/// local `x = rings[k].0`. Built flat, then auto-smoothed below `smooth_deg`; `caps` closes
/// the first and last ring. UVs: `u` along X, `v` around the profile (meters).
fn loft(
    b: &mut MeshBuilder,
    f: &Frame,
    rings: &[(f32, &[Vec2])],
    smooth_deg: f32,
    caps: [bool; 2],
    tint: [f32; 3],
    uv: Vec2,
) {
    if rings.len() < 2 {
        return;
    }
    let v0 = b.vertices.len();
    let t0 = b.indices.len();
    let m = rings[0].1.len();
    let to3 = |x: f32, p: Vec2| Vec3::new(x, p.y, p.x);
    let ccw = signed_area(rings[0].1) >= 0.0;
    for w in rings.windows(2) {
        let (xa, pa) = w[0];
        let (xb, pb) = w[1];
        let (sa, sb) = (arc_lengths(pa), arc_lengths(pb));
        for i in 0..m {
            let j = (i + 1) % m;
            let e = pa[j] - pa[i];
            let n2 = if ccw {
                Vec2::new(e.y, -e.x)
            } else {
                Vec2::new(-e.y, e.x)
            };
            let n = f.d(Vec3::new(0.0, n2.y, n2.x).normalize_or(Vec3::Y));
            let q = [
                (to3(xa, pa[i]), Vec2::new(xa, -sa[i])),
                (to3(xa, pa[j]), Vec2::new(xa, -sa[i + 1])),
                (to3(xb, pb[j]), Vec2::new(xb, -sb[i + 1])),
                (to3(xb, pb[i]), Vec2::new(xb, -sb[i])),
            ];
            let idx = q.map(|(p, t)| vert(b, f.p(p), n, t + uv, tint));
            quad_idx(b, idx, n);
        }
    }
    auto_smooth(b, v0, t0, smooth_deg);
    for (k, on) in [(0, caps[0]), (rings.len() - 1, caps[1])] {
        if !on {
            continue;
        }
        let (x, p) = rings[k];
        let nl = if k == 0 { Vec3::NEG_X } else { Vec3::X };
        let n = f.d(nl);
        let base = b.vertices.len() as u32;
        for &q in p {
            let lp = to3(x, q);
            vert(b, f.p(lp), n, box_uv(lp, nl) + uv, tint);
        }
        for t in triangulate(p) {
            tri_idx(
                b,
                [base + t[0] as u32, base + t[1] as u32, base + t[2] as u32],
                n,
            );
        }
    }
}

/// Straight extrusion of one profile through `xs`.
fn extrude(
    b: &mut MeshBuilder,
    f: &Frame,
    prof: &[Vec2],
    xs: &[f32],
    smooth_deg: f32,
    caps: bool,
    tint: [f32; 3],
    uv: Vec2,
) {
    let rings: Vec<(f32, &[Vec2])> = xs.iter().map(|&x| (x, prof)).collect();
    loft(b, f, &rings, smooth_deg, [caps, caps], tint, uv);
}

/// Sawn board along local X through stations `xs`: cross-section `±half` (thickness along
/// local Z, width along local Y), long edges on the +Z side rounded by `r_front`, on the -Z
/// side by `r_back`; flat sawn ends when `caps`.
fn board(
    b: &mut MeshBuilder,
    f: &Frame,
    half: Vec2,
    xs: &[f32],
    r_front: f32,
    r_back: f32,
    caps: bool,
    tint: [f32; 3],
    uv: Vec2,
) {
    let prof = rounded_rect(half, [r_back, r_front, r_front, r_back], 2);
    extrude(b, f, &prof, xs, 50.0, caps, tint, uv);
}

#[derive(Debug, Clone, Copy)]
enum LatheUv {
    /// Wrapped around the axis: `u` = angle × radius (meters), `v` = profile length.
    Wrap(f32),
    /// Projected from above (lids, discs).
    Planar,
}

/// Surface of revolution around local Y. `prof` holds `(radius, height)` points from the
/// bottom around the outside to the top (counter-clockwise in the r–y half plane), so normals
/// face out. Corners sharper than `crease_deg` get hard normals.
fn lathe(
    b: &mut MeshBuilder,
    f: &Frame,
    prof: &[Vec2],
    segs: usize,
    crease_deg: f32,
    uv: LatheUv,
    tint: [f32; 3],
) {
    let n = prof.len();
    if n < 2 {
        return;
    }
    let seg_n: Vec<Vec2> = (0..n - 1)
        .map(|i| {
            let d = prof[i + 1] - prof[i];
            Vec2::new(d.y, -d.x).normalize_or(Vec2::X)
        })
        .collect();
    let cos_c = crease_deg.to_radians().cos();
    let mut s = vec![0.0; n];
    for i in 1..n {
        s[i] = s[i - 1] + prof[i].distance(prof[i - 1]);
    }
    let mut rings: Vec<(usize, Vec2)> = Vec::new();
    let mut before = vec![0usize; n];
    let mut after = vec![0usize; n];
    for i in 0..n {
        let nin = (i > 0).then(|| seg_n[i - 1]);
        let nout = (i + 1 < n).then(|| seg_n[i]);
        match (nin, nout) {
            (Some(a), Some(c)) if a.dot(c) < cos_c => {
                before[i] = rings.len();
                rings.push((i, a));
                after[i] = rings.len();
                rings.push((i, c));
            }
            (a, c) => {
                let nn = (a.unwrap_or(Vec2::ZERO) + c.unwrap_or(Vec2::ZERO)).normalize_or(Vec2::X);
                before[i] = rings.len();
                after[i] = rings.len();
                rings.push((i, nn));
            }
        }
    }
    let row = (segs + 1) as u32;
    let base = b.vertices.len() as u32;
    for &(i, n2) in &rings {
        let p = prof[i];
        for k in 0..=segs {
            let a = k as f32 / segs as f32 * TAU;
            let (sn, cs) = a.sin_cos();
            let pos = Vec3::new(p.x * cs, p.y, -p.x * sn);
            let nrm = Vec3::new(n2.x * cs, n2.y, -n2.x * sn);
            let t = match uv {
                LatheUv::Wrap(r) => Vec2::new(a * r, -s[i]),
                LatheUv::Planar => Vec2::new(pos.x, pos.z),
            };
            vert(b, f.p(pos), f.d(nrm), t, tint);
        }
    }
    for i in 0..n - 1 {
        let ra = base + after[i] as u32 * row;
        let rb = base + before[i + 1] as u32 * row;
        for k in 0..segs as u32 {
            let (a0, a1, b0, b1) = (ra + k, ra + k + 1, rb + k, rb + k + 1);
            if prof[i].x > 1e-6 {
                b.indices.extend_from_slice(&[a0, a1, b1]);
            }
            if prof[i + 1].x > 1e-6 {
                b.indices.extend_from_slice(&[a0, b1, b0]);
            }
        }
    }
}

/// Round tube along `path` with `sides` facets (smooth shading), optional flat end caps.
/// UVs: `u` around (meters), `v` along the path.
fn tube(b: &mut MeshBuilder, path: &[Vec3], radius: f32, sides: usize, caps: bool, tint: [f32; 3]) {
    let n = path.len();
    if n < 2 {
        return;
    }
    let tangent =
        |i: usize| (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec3::X);
    let mut nrm = tangent(0).any_orthonormal_vector();
    let v0 = b.vertices.len() as u32;
    let mut len = 0.0;
    let row = (sides + 1) as u32;
    for i in 0..n {
        let t = tangent(i);
        nrm = (nrm - t * nrm.dot(t)).normalize_or(t.any_orthonormal_vector());
        let bin = t.cross(nrm);
        if i > 0 {
            len += path[i].distance(path[i - 1]);
        }
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * TAU;
            let d = nrm * a.cos() + bin * a.sin();
            vert(b, path[i] + d * radius, d, Vec2::new(a * radius, len), tint);
        }
    }
    for i in 0..n as u32 - 1 {
        for k in 0..sides as u32 {
            let a = v0 + i * row + k;
            let c = a + row;
            let out = Vec3::from(b.vertices[a as usize].normal)
                + Vec3::from(b.vertices[(c + 1) as usize].normal);
            quad_idx(b, [a, a + 1, c + 1, c], out);
        }
    }
    if caps {
        for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
            let t = tangent(i) * sign;
            let ring = v0 + i as u32 * row;
            let c = vert(b, path[i], t, Vec2::ZERO, tint);
            let base = b.vertices.len() as u32;
            for k in 0..sides as u32 {
                let p = vpos(b, ring + k);
                vert(b, p, t, Vec2::new((p - path[i]).length(), 0.0), tint);
            }
            for k in 0..sides as u32 {
                tri_idx(b, [c, base + k, base + (k + 1) % sides as u32], t);
            }
        }
    }
}

/// Points on a circular arc around `center` in the plane of unit vectors `u`, `v`.
fn arc_path(center: Vec3, u: Vec3, v: Vec3, radius: f32, a0: f32, a1: f32, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / n as f32;
            center + (u * a.cos() + v * a.sin()) * radius
        })
        .collect()
}

/// Low domed nail or rivet head on a surface at `p` facing `n`.
fn nail(b: &mut MeshBuilder, p: Vec3, n: Vec3, r: f32, tint: [f32; 3]) {
    let (t1, t2) = n.any_orthonormal_pair();
    let c = vert(b, p + n * r * 0.4, n, box_uv(p, n), tint);
    let k0 = b.vertices.len() as u32;
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        let d = t1 * a.cos() + t2 * a.sin();
        let q = p + d * r - n * 0.0003;
        vert(b, q, (n + d * 0.9).normalize(), box_uv(q, n), tint);
    }
    for k in 0..6u32 {
        tri_idx(b, [c, k0 + k, k0 + (k + 1) % 6], n);
    }
}

/// Hexagon bolt head or nut with a chamfered top, standing on `p` along `axis`.
fn hex_head(b: &mut MeshBuilder, p: Vec3, axis: Vec3, r: f32, h: f32, tint: [f32; 3]) {
    let v0 = b.vertices.len();
    let t0 = b.indices.len();
    let prof = [
        Vec2::new(0.0, -0.0005),
        Vec2::new(r, -0.0005),
        Vec2::new(r, h * 0.8),
        Vec2::new(r * 0.8, h),
        Vec2::new(0.0, h),
    ];
    lathe(
        b,
        &Frame::up_along(p, axis),
        &prof,
        6,
        40.0,
        LatheUv::Planar,
        tint,
    );
    auto_smooth(b, v0, t0, 40.0);
}

fn n3(p: Vec3, freq: f32, seed: u64) -> f32 {
    let q = p.as_dvec3() * freq as f64;
    perlin3(q.x, q.y, q.z, seed)
}

fn fbm(p: Vec3, freq: f32, seed: u64, octaves: u32) -> f32 {
    let q = p.as_dvec3() * freq as f64;
    fbm3(q.x, q.y, q.z, seed, octaves, 2.0, 0.5)
}

fn mul(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

/// Per-board wood tint: brightness and warmth vary like boards from different trees.
fn wood_tint(rng: &mut Rng) -> [f32; 3] {
    let k = rng.range(0.78, 1.04);
    let warm = rng.range(-0.05, 0.05);
    [k * (1.0 + warm), k, k * (1.0 - warm * 1.6)]
}

/// Random texture offset so neighbouring boards do not repeat the same grain.
fn rng_uv(rng: &mut Rng) -> Vec2 {
    Vec2::new(rng.range(0.0, 5.0), rng.range(0.0, 5.0))
}

/// Darken vertices `v0..` whose normal faces along `dir` (insides seen through gaps).
fn darken_facing(b: &mut MeshBuilder, v0: usize, dir: Vec3, k: f32) {
    shade(
        b,
        v0,
        |_, n| if n.dot(dir) > 0.7 { [k; 3] } else { [1.0; 3] },
    );
}

// --------------------------------------------------------------------------------------------
// Ambient occlusion bake
// --------------------------------------------------------------------------------------------

const AO_RAYS: usize = 32;

#[derive(Debug, Clone, Copy, Default)]
struct BvhNode {
    lo: Vec3,
    hi: Vec3,
    first: u32,
    count: u32,
}

/// Bounding volume hierarchy over triangles for occlusion rays.
struct Bvh {
    nodes: Vec<BvhNode>,
    tris: Vec<[Vec3; 3]>,
}

impl Bvh {
    fn new(tris: Vec<[Vec3; 3]>) -> Self {
        let cent: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = vec![BvhNode::default()];
        let mut stack = vec![(0usize, 0usize, tris.len())];
        while let Some((ni, s, e)) = stack.pop() {
            let mut lo = Vec3::splat(f32::MAX);
            let mut hi = Vec3::splat(f32::MIN);
            let mut clo = lo;
            let mut chi = hi;
            for &i in &order[s..e] {
                for p in tris[i as usize] {
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
                clo = clo.min(cent[i as usize]);
                chi = chi.max(cent[i as usize]);
            }
            nodes[ni].lo = lo;
            nodes[ni].hi = hi;
            if e - s <= 4 {
                nodes[ni].first = s as u32;
                nodes[ni].count = (e - s) as u32;
                continue;
            }
            let ext = chi - clo;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let mid = (s + e) / 2;
            order[s..e].select_nth_unstable_by(mid - s, |a, b| {
                cent[*a as usize][axis].total_cmp(&cent[*b as usize][axis])
            });
            let left = nodes.len();
            nodes.push(BvhNode::default());
            nodes.push(BvhNode::default());
            nodes[ni].first = left as u32;
            nodes[ni].count = 0;
            stack.push((left, s, mid));
            stack.push((left + 1, mid, e));
        }
        let tris = order.iter().map(|&i| tris[i as usize]).collect();
        Self { nodes, tris }
    }

    /// Nearest hit closer than `tmax`: (distance, whether it hit the front side).
    fn cast(&self, o: Vec3, d: Vec3, mut tmax: f32) -> Option<(f32, bool)> {
        if self.tris.is_empty() {
            return None;
        }
        let inv = d.recip();
        let mut best = None;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = self.nodes[stack[sp] as usize];
            let t0 = (node.lo - o) * inv;
            let t1 = (node.hi - o) * inv;
            let tn = t0.min(t1).max_element().max(0.0);
            let tf = t0.max(t1).min_element().min(tmax);
            if tn > tf {
                continue;
            }
            if node.count > 0 {
                for t in &self.tris[node.first as usize..(node.first + node.count) as usize] {
                    let e1 = t[1] - t[0];
                    let e2 = t[2] - t[0];
                    let p = d.cross(e2);
                    let det = e1.dot(p);
                    if det.abs() < 1e-12 {
                        continue;
                    }
                    let id = 1.0 / det;
                    let s = o - t[0];
                    let u = s.dot(p) * id;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let q = s.cross(e1);
                    let v = d.dot(q) * id;
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let h = e2.dot(q) * id;
                    if h > 1e-5 && h < tmax {
                        tmax = h;
                        best = Some((h, det > 0.0));
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = node.first;
                stack[sp + 1] = node.first + 1;
                sp += 2;
            }
        }
        best
    }
}

/// Cosine-weighted hemisphere directions around +Z (Hammersley set).
fn hemisphere(n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|k| {
            let u1 = (k as f32 + 0.5) / n as f32;
            let u2 = (k as u32).reverse_bits() as f32 * 2.328_306_4e-10;
            let r = u1.sqrt();
            let phi = TAU * u2;
            Vec3::new(r * phi.cos(), r * phi.sin(), (1.0 - u1).max(0.0).sqrt())
        })
        .collect()
}

/// Ray-traced ambient occlusion from the prop itself (and the ground plane at `ground`),
/// multiplied into the vertex colors.
fn bake_ao(parts: &mut [PropPart], ground: Option<f32>, reach: f32, strength: f32) {
    let mut tris = Vec::new();
    for p in parts.iter() {
        let v = &p.mesh.vertices;
        for t in p.mesh.indices.as_chunks::<3>().0 {
            let a = Vec3::from(v[t[0] as usize].position);
            let b = Vec3::from(v[t[1] as usize].position);
            let c = Vec3::from(v[t[2] as usize].position);
            if (b - a).cross(c - a).length_squared() > 1e-16 {
                tris.push([a, b, c]);
            }
        }
    }
    let bvh = Bvh::new(tris);
    let dirs = hemisphere(AO_RAYS);
    for part in parts.iter_mut() {
        let mesh = &mut part.mesh;
        let n = mesh.vertices.len();
        let mut ao = vec![1.0f32; n];
        let mut buried = vec![false; n];
        for (i, v) in mesh.vertices.iter().enumerate() {
            let p = Vec3::from(v.position);
            let nrm = Vec3::from(v.normal).normalize_or(Vec3::Y);
            let (t1, t2) = nrm.any_orthonormal_pair();
            let spin = hash01(i as i64, n as i64, 0, 0xA0) * TAU;
            let (ss, cs) = spin.sin_cos();
            let o = p + nrm * 1.0e-3;
            let mut occ = 0.0;
            let mut back = 0;
            for d in &dirs {
                let (x, y) = (d.x * cs - d.y * ss, d.x * ss + d.y * cs);
                let dir = t1 * x + t2 * y + nrm * d.z;
                let mut hit = None;
                if let Some((t, front)) = bvh.cast(o, dir, reach) {
                    hit = Some(t);
                    if !front && t < 0.04 {
                        back += 1;
                    }
                }
                if let Some(g) = ground
                    && dir.y < -1e-3
                {
                    let t = (g - o.y) / dir.y;
                    if t >= 0.0 && t < hit.unwrap_or(reach) {
                        hit = Some(t);
                    }
                }
                if let Some(t) = hit {
                    let f = t / reach;
                    occ += 1.0 - f * f;
                }
            }
            ao[i] = 1.0 - occ / AO_RAYS as f32;
            buried[i] = back * 2 > AO_RAYS;
        }
        // Buried vertices (inside another piece) take the occlusion of their visible
        // neighbours, so a board's hidden end does not darken its visible middle.
        let mut acc = vec![(0.0f32, 0u32); n];
        for t in mesh.indices.as_chunks::<3>().0 {
            for (x, y) in [(0, 1), (1, 2), (2, 0), (1, 0), (2, 1), (0, 2)] {
                let (x, y) = (t[x] as usize, t[y] as usize);
                if buried[x] && !buried[y] {
                    acc[x].0 += ao[y];
                    acc[x].1 += 1;
                }
            }
        }
        for i in 0..n {
            if buried[i] {
                ao[i] = if acc[i].1 > 0 {
                    acc[i].0 / acc[i].1 as f32
                } else {
                    1.0
                };
            }
        }
        for (v, a) in mesh.vertices.iter_mut().zip(ao) {
            let k = 1.0 - strength * (1.0 - a.clamp(0.0, 1.0));
            for c in &mut v.color[..3] {
                *c *= k;
            }
        }
    }
}

// ============================================================================================
// Crates and pallets
// ============================================================================================

/// Classic framed wooden crate filling `size` (meters, centered): every face has a frame of
/// boards along its edges, panel boards with small gaps behind it, a diagonal brace and nail
/// heads (the bottom face is framed but plain).
pub fn wooden_crate(size: Vec3, seed: u64) -> Vec<PropPart> {
    let size = size.max(Vec3::splat(0.3));
    let h = size * 0.5;
    let mut rng = Rng::new(seed ^ 0x0C2A_7E01);
    let mut kit = Kit::new();
    let tf = 0.022;
    let tp = 0.018;
    let w = (size.min_element() * 0.1).clamp(0.055, 0.095);
    let side = Vec2::new(size.z - 2.0 * tf, size.y - 2.0 * tf);
    let side_panel = Vec2::new(h.z - tf - tp, h.y - tf - tp);
    let front = Vec2::new(size.x, size.y - 2.0 * tf);
    let front_panel = Vec2::new(h.x - tf, h.y - tf - tp);
    let lid = Vec2::new(size.x, size.z);
    let lid_panel = Vec2::new(h.x - tf, h.z - tf);
    let faces = [
        (Vec3::Z, Vec3::X, front, front_panel, true),
        (Vec3::NEG_Z, Vec3::NEG_X, front, front_panel, true),
        (Vec3::X, Vec3::NEG_Z, side, side_panel, true),
        (Vec3::NEG_X, Vec3::Z, side, side_panel, true),
        (Vec3::Y, Vec3::X, lid, lid_panel, true),
        (Vec3::NEG_Y, Vec3::X, lid, lid_panel, false),
    ];
    for (n, u, ext, panel, dressed) in faces {
        let v = n.cross(u);
        let face = Frame::new(n * n.abs().dot(h), Mat3::from_cols(u, v, n));
        crate_side(&mut kit, &face, ext, panel, tf, tp, w, dressed, &mut rng);
    }
    kit.finish(Ao {
        reach: 0.22,
        strength: 0.9,
        ground: true,
    })
}

/// One face of [`wooden_crate`] in face coordinates (X across, Y up the face, Z out).
fn crate_side(
    kit: &mut Kit,
    face: &Frame,
    ext: Vec2,
    panel: Vec2,
    tf: f32,
    tp: f32,
    w: f32,
    dressed: bool,
    rng: &mut Rng,
) {
    let (hw, hh) = (ext.x * 0.5, ext.y * 0.5);
    let (iu, iv) = (hw - w, hh - w);
    let r = 0.004;
    let nail_r = 0.0042;
    let iron = [0.55, 0.53, 0.5];
    let out = face.d(Vec3::Z);
    // Horizontal frame boards over the full width.
    for sv in [-1.0f32, 1.0] {
        let f = face.then(Frame::at(Vec3::new(0.0, sv * (hh - w * 0.5), -tf * 0.5)));
        let xs = stations(-hw, hw, &[-iu, iu], 0.3);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(tf * 0.5, w * 0.5),
            &xs,
            r,
            0.0,
            true,
            wood_tint(rng),
            rng_uv(rng),
        );
        if dressed {
            let count = ((2.0 * hw) / 0.2).ceil().max(2.0) as usize;
            for i in 0..count {
                let u = -hw + w * 0.5 + (2.0 * hw - w) * (i as f32 + 0.5) / count as f32;
                let p = Vec3::new(u, sv * (hh - w * 0.5) + rng.range(-0.008, 0.008), 0.0);
                nail(kit.mb(METAL), face.p(p), out, nail_r, iron);
            }
        }
    }
    // Vertical frame boards between them.
    for su in [-1.0f32, 1.0] {
        let f = face.then(Frame::new(
            Vec3::new(su * (hw - w * 0.5), 0.0, -tf * 0.5),
            Mat3::from_cols(Vec3::Y, Vec3::NEG_X, Vec3::Z),
        ));
        let xs = stations(-iv, iv, &[], 0.3);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(tf * 0.5, w * 0.5),
            &xs,
            r,
            0.0,
            false,
            wood_tint(rng),
            rng_uv(rng),
        );
    }
    // Panel boards with gaps, behind the frame.
    let span = 2.0 * panel.y;
    let gap = 0.005;
    let n = ((span + gap) / (0.135 + gap)).round().max(2.0) as usize;
    let widths: Vec<f32> = (0..n).map(|_| rng.range(0.85, 1.15)).collect();
    let k = (span - gap * (n - 1) as f32) / widths.iter().sum::<f32>();
    let xs = stations(-panel.x, panel.x, &[-iu, iu], 0.3);
    let mut v = -panel.y;
    for bw in widths.iter().map(|x| x * k) {
        let vc = v + bw * 0.5;
        v += bw + gap;
        let f = face.then(Frame::new(
            Vec3::new(0.0, vc, -tf - tp * 0.5 + rng.range(-0.0005, 0.0005)),
            Mat3::from_rotation_x(rng.range(-0.008, 0.008)),
        ));
        let b = kit.mb(WOOD);
        let v0 = b.vertices.len();
        board(
            b,
            &f,
            Vec2::new(tp * 0.5, bw * 0.5),
            &xs,
            0.003,
            0.0,
            false,
            wood_tint(rng),
            rng_uv(rng),
        );
        // The back faces the dark inside of the crate.
        darken_facing(b, v0, -out, 0.35);
        if dressed && vc.abs() < iv - 0.015 {
            for su in [-1.0f32, 1.0] {
                let p = Vec3::new(
                    su * (hw - w * 0.5) + rng.range(-0.008, 0.008),
                    vc + rng.range(-0.012, 0.012),
                    0.0,
                );
                nail(kit.mb(METAL), face.p(p), out, nail_r, iron);
            }
        }
    }
    // Diagonal brace between the frame corners, nailed along its length.
    if dressed {
        let flip = if rng.chance(0.5) { 1.0 } else { -1.0 };
        let d = Vec2::new(2.0 * iu, flip * 2.0 * iv);
        let len = d.length();
        let dir = Vec3::new(d.x / len, d.y / len, 0.0);
        let tb = tf - 0.003;
        let f = face.then(Frame::new(
            Vec3::new(0.0, 0.0, -tf + tb * 0.5),
            Mat3::from_cols(dir, Vec3::Z.cross(dir), Vec3::Z),
        ));
        let xs = stations(-len * 0.5, len * 0.5, &[], 0.25);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(tb * 0.5, w * 0.47),
            &xs,
            r,
            0.0,
            false,
            wood_tint(rng),
            rng_uv(rng),
        );
        for t in [-0.33f32, 0.0, 0.33] {
            let p = dir * t * len + Vec3::new(0.0, 0.0, -0.003);
            nail(kit.mb(METAL), face.p(p), out, nail_r, iron);
        }
    }
}

/// Military ammunition crate filling `size`: olive painted planks with worn edges on two skids,
/// a lid with end battens, pressed steel corner caps with rivets, two steel shipping bands
/// with crimp seals, folding handles with wooden grips on the ends, two latches on the front
/// and strap hinges on the back.
pub fn military_crate(size: Vec3, seed: u64) -> Vec<PropPart> {
    let size = size.max(Vec3::new(0.45, 0.25, 0.3));
    let h = size * 0.5;
    let mut rng = Rng::new(seed ^ 0x3A11_C0DE);
    let mut kit = Kit::new();
    let tc = 0.003;
    let tsk = 0.02;
    let tbat = 0.014;
    let tband = 0.0018;
    // Body box ±hb (walls, lid, bottom); skids below it, battens above, centered at the end.
    let hb = Vec3::new(h.x - 0.032, (size.y - tsk - tbat) * 0.5, h.z - 0.016);
    let tw = 0.02;
    let gap = 0.0035;
    let a = (size.min_element() * 0.17).clamp(0.05, 0.085);
    let lid_h = (hb.y * 0.45).clamp(0.06, 0.12);
    let y_lid = hb.y - lid_h;
    let olive = [0.64, 0.74, 0.47];
    let tint = |rng: &mut Rng| {
        let t = wood_tint(rng);
        let k = rng.range(0.94, 1.04);
        [
            (0.6 + 0.4 * t[0]) * olive[0] * k,
            (0.6 + 0.4 * t[1]) * olive[1] * k,
            (0.6 + 0.4 * t[2]) * olive[2] * k,
        ]
    };
    let steel = [0.72, 0.73, 0.7];
    let x_sk = hb.x - a - 0.045;
    let x_band = (hb.x * 0.32).min(x_sk - 0.055);
    let x_hw = hb.x * 0.55;
    let cap_lines = [-(hb.x - a), hb.x - a];
    // Planks of the four walls: body rows and the lid skirt, with a wider seam at the lid.
    let body_rows = ((y_lid + hb.y) / 0.14).round().max(1.0) as usize;
    let mut rows: Vec<(f32, f32)> = (0..body_rows)
        .map(|i| {
            let r = (y_lid + hb.y) / body_rows as f32;
            (-hb.y + r * i as f32, -hb.y + r * (i + 1) as f32 - gap)
        })
        .collect();
    rows.push((y_lid + 0.002, hb.y - tw));
    let walls = [
        (Vec3::Z, Vec3::X, hb.z, hb.x, true),
        (Vec3::NEG_Z, Vec3::NEG_X, hb.z, hb.x, true),
        (Vec3::X, Vec3::NEG_Z, hb.x, hb.z - tw, false),
        (Vec3::NEG_X, Vec3::Z, hb.x, hb.z - tw, false),
    ];
    for (n, u, dist, half_len, caps) in walls {
        let face = Frame::new(n * dist, Mat3::from_cols(u, n.cross(u), n));
        let lines: Vec<f32> = [-x_band, x_band, -x_hw, x_hw]
            .iter()
            .chain(&cap_lines)
            .map(|x| x.clamp(-half_len, half_len))
            .collect();
        let xs = stations(-half_len, half_len, &lines, 0.2);
        for &(y0, y1) in &rows {
            let f = face.then(Frame::new(
                Vec3::new(0.0, (y0 + y1) * 0.5, -tw * 0.5 + rng.range(-0.0006, 0.0006)),
                Mat3::from_rotation_x(rng.range(-0.004, 0.004)),
            ));
            let b = kit.mb(WOOD);
            let v0 = b.vertices.len();
            board(
                b,
                &f,
                Vec2::new(tw * 0.5, (y1 - y0) * 0.5),
                &xs,
                0.0045,
                0.002,
                caps,
                tint(&mut rng),
                rng_uv(&mut rng),
            );
            darken_facing(b, v0, -n, 0.35);
        }
    }
    // Lid boards, end battens across them, and the bottom.
    let lid_face = Frame::new(
        Vec3::Y * hb.y,
        Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y),
    );
    let nb = ((2.0 * hb.z) / 0.15).round().max(2.0) as usize;
    let bw = (2.0 * hb.z - gap * (nb - 1) as f32) / nb as f32;
    let x_bat = hb.x - a - 0.03;
    let xs = stations(
        -hb.x,
        hb.x,
        &[
            -(hb.x - a),
            hb.x - a,
            -x_band,
            x_band,
            -x_bat - 0.0225,
            -x_bat + 0.0225,
            x_bat - 0.0225,
            x_bat + 0.0225,
        ],
        0.2,
    );
    for i in 0..nb {
        let v = -hb.z + bw * (i as f32 + 0.5) + gap * i as f32;
        let f = lid_face.then(Frame::new(
            Vec3::new(0.0, v, -tw * 0.5),
            Mat3::from_rotation_x(rng.range(-0.004, 0.004)),
        ));
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(tw * 0.5, bw * 0.5),
            &xs,
            0.0045,
            0.0,
            true,
            tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    let iron = [0.5, 0.49, 0.46];
    for sx in [-1.0f32, 1.0] {
        let f = Frame::new(
            Vec3::new(sx * x_bat, hb.y + tbat * 0.5, 0.0),
            Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y),
        );
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(tbat * 0.5, 0.0225),
            &stations(-hb.z, hb.z, &[], 0.12),
            0.004,
            0.0,
            true,
            tint(&mut rng),
            rng_uv(&mut rng),
        );
        for k in 0..nb {
            let z = -hb.z + bw * (k as f32 + 0.5) + gap * k as f32;
            for dx in [-0.009f32, 0.009] {
                nail(
                    kit.mb(METAL),
                    Vec3::new(sx * x_bat + dx, hb.y + tbat, z + rng.range(-0.01, 0.01)),
                    Vec3::Y,
                    0.0038,
                    iron,
                );
            }
        }
    }
    let bottom = Frame::new(
        Vec3::Y * (-hb.y + tw * 0.5),
        Mat3::from_cols(Vec3::X, Vec3::Z, Vec3::NEG_Y),
    );
    board(
        kit.mb(WOOD),
        &bottom,
        Vec2::new(tw * 0.5, hb.z - tw),
        &[-hb.x + tw, hb.x - tw],
        0.0,
        0.0,
        true,
        tint(&mut rng),
        Vec2::ZERO,
    );
    // Two skids across the bottom.
    for sx in [-1.0f32, 1.0] {
        let f = Frame::new(
            Vec3::new(sx * x_sk, -hb.y - tsk * 0.5, 0.0),
            Mat3::from_cols(Vec3::NEG_Z, Vec3::X, Vec3::NEG_Y),
        );
        let b = kit.mb(WOOD);
        let v0 = b.vertices.len();
        board(
            b,
            &f,
            Vec2::new(tsk * 0.5, 0.03),
            &stations(-hb.z + 0.004, hb.z - 0.004, &[], 0.12),
            0.005,
            0.0015,
            true,
            tint(&mut rng),
            rng_uv(&mut rng),
        );
        shade(b, v0, |_, _| [0.8, 0.78, 0.74]);
    }
    // Paint: mottled, rubbed through to bare wood on worn edges and corners.
    {
        let b = kit.mb(WOOD);
        let bare = [0.95, 0.86, 0.72];
        for v in &mut b.vertices {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            let lum = v.color[0].max(v.color[1]);
            let edge = 1.0 - n.abs().max_element();
            let rub = smoothstep(0.06, 0.2, edge)
                * smoothstep(-0.15, 0.3, fbm(p, 7.0, seed ^ 0x5A1, 3))
                * smoothstep(0.3, 0.45, lum);
            let mottle = 1.0 + 0.09 * fbm(p, 2.5, seed ^ 0x77, 3);
            let c = mix3(
                mul([v.color[0], v.color[1], v.color[2]], mottle),
                bare,
                rub * 0.85,
            );
            v.color = [c[0], c[1], c[2], 1.0];
        }
    }
    // Steel shipping bands around front, lid, back and bottom, with crimp seals on top.
    for sx in [-1.0f32, 1.0] {
        let wb = 0.011;
        let outer = rounded_rect(Vec2::new(hb.z + tband, hb.y + tband), [0.0045; 4], 2);
        let inner = rounded_rect(Vec2::new(hb.z, hb.y), [0.0027; 4], 2);
        let b = kit.mb(METAL);
        let f = Frame::at(Vec3::X * sx * x_band);
        extrude(
            b,
            &f,
            &outer,
            &[-wb, -wb * 0.3, wb * 0.3, wb],
            40.0,
            false,
            steel,
            Vec2::ZERO,
        );
        for side in [-1.0f32, 1.0] {
            let nrm = Vec3::X * side;
            let m = outer.len();
            for i in 0..m {
                let j = (i + 1) % m;
                let q = [outer[i], outer[j], inner[j], inner[i]]
                    .map(|p| Vec3::new(sx * x_band + side * wb, p.y, p.x));
                let idx = q.map(|p| vert(b, p, nrm, Vec2::new(p.z, p.y), steel));
                quad_idx(b, idx, nrm);
            }
        }
        let zs = rng.range(-0.35, 0.35) * hb.z;
        bevel_box(
            b,
            &Frame::at(Vec3::new(sx * x_band, hb.y + tband + 0.0018, zs)),
            &BoxSpec::new(Vec3::new(wb + 0.0025, 0.0022, 0.016), 0.0012).tint(steel),
        );
        for dz in [-0.008f32, 0.0, 0.008] {
            bevel_box(
                b,
                &Frame::at(Vec3::new(sx * x_band, hb.y + tband + 0.0042, zs + dz)),
                &BoxSpec::new(Vec3::new(wb + 0.0026, 0.0006, 0.0018), 0.0004)
                    .faces(ALL & !NY)
                    .tint(mul(steel, 0.8)),
            );
        }
    }
    // Pressed steel corner caps with a rivet on each face.
    for sx in [-1.0f32, 1.0] {
        for sy in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                let s = Vec3::new(sx, sy, sz);
                let c = s * (hb + Vec3::splat(tc - a * 0.5));
                let faces = (if sx > 0.0 { PX } else { NX })
                    | (if sy > 0.0 { PY } else { NY })
                    | (if sz > 0.0 { PZ } else { NZ });
                let b = kit.mb(METAL);
                bevel_box(
                    b,
                    &Frame::at(c),
                    &BoxSpec::new(Vec3::splat(a * 0.5), tc * 0.9)
                        .faces(faces)
                        .tint(steel),
                );
                for axis in 0..3 {
                    let mut n = Vec3::ZERO;
                    n[axis] = s[axis];
                    let mut p = c;
                    p[axis] = s[axis] * (hb[axis] + tc);
                    nail(b, p, n, 0.0055, steel);
                }
            }
        }
    }
    // Folding handles on the ends: brackets, a steel bail and a wooden grip.
    let zb = (hb.z * 0.42).min(0.085);
    let yh = (y_lid - 0.05).min(hb.y * 0.35);
    for sx in [-1.0f32, 1.0] {
        let xf = sx * hb.x;
        for sz in [-1.0f32, 1.0] {
            bevel_box(
                kit.mb(METAL),
                &Frame::at(Vec3::new(xf + sx * 0.004, yh, sz * zb)),
                &BoxSpec::new(Vec3::new(0.004, 0.028, 0.017), 0.002)
                    .faces(ALL & if sx > 0.0 { !NX } else { !PX })
                    .tint(steel),
            );
            for dy in [-0.016f32, 0.016] {
                nail(
                    kit.mb(METAL),
                    Vec3::new(xf + sx * 0.008, yh + dy, sz * zb),
                    Vec3::X * sx,
                    0.004,
                    steel,
                );
            }
        }
        let xh = xf + sx * 0.014;
        let drop = 0.055;
        let rc = 0.018;
        let mut path = vec![Vec3::new(xh, yh, -zb)];
        path.extend(arc_path(
            Vec3::new(xh, yh - drop + rc, -zb + rc),
            Vec3::NEG_Z,
            Vec3::NEG_Y,
            rc,
            0.0,
            FRAC_PI_2,
            4,
        ));
        path.extend(arc_path(
            Vec3::new(xh, yh - drop + rc, zb - rc),
            Vec3::NEG_Y,
            Vec3::Z,
            rc,
            0.0,
            FRAC_PI_2,
            4,
        ));
        path.push(Vec3::new(xh, yh, zb));
        tube(kit.mb(METAL), &path, 0.0055, 8, true, steel);
        let grip = [
            Vec3::new(xh, yh - drop, -zb + rc + 0.008),
            Vec3::new(xh, yh - drop, zb - rc - 0.008),
        ];
        let b = kit.mb(WOOD);
        let v0 = b.vertices.len();
        tube(b, &grip, 0.012, 10, true, [0.62, 0.52, 0.42]);
        shade(b, v0, |_, n| [0.85 + 0.15 * n.y.max(0.0); 3]);
    }
    // Latches over the lid seam on the front.
    for sx in [-1.0f32, 1.0] {
        let x = sx * x_hw;
        let b = kit.mb(METAL);
        bevel_box(
            b,
            &Frame::at(Vec3::new(x, y_lid - 0.03, hb.z + 0.003)),
            &BoxSpec::new(Vec3::new(0.024, 0.022, 0.003), 0.0015)
                .faces(ALL & !NZ)
                .tint(steel),
        );
        bevel_box(
            b,
            &Frame::new(
                Vec3::new(x, y_lid + 0.006, hb.z + 0.007),
                Mat3::from_rotation_x(-0.08),
            ),
            &BoxSpec::new(Vec3::new(0.017, 0.032, 0.0035), 0.0015).tint(steel),
        );
        bevel_box(
            b,
            &Frame::at(Vec3::new(x, y_lid + 0.045, hb.z + 0.003)),
            &BoxSpec::new(Vec3::new(0.022, 0.012, 0.003), 0.0015)
                .faces(ALL & !NZ)
                .tint(steel),
        );
        tube(
            b,
            &[
                Vec3::new(x - 0.02, y_lid + 0.04, hb.z + 0.007),
                Vec3::new(x + 0.02, y_lid + 0.04, hb.z + 0.007),
            ],
            0.004,
            8,
            true,
            steel,
        );
        for dx in [-0.016f32, 0.016] {
            nail(
                b,
                Vec3::new(x + dx, y_lid - 0.03, hb.z + 0.006),
                Vec3::Z,
                0.0035,
                steel,
            );
        }
    }
    // Strap hinges over the lid seam on the back.
    for sx in [-1.0f32, 1.0] {
        let x = sx * x_hw;
        let b = kit.mb(METAL);
        let y = y_lid + 0.001;
        tube(
            b,
            &[
                Vec3::new(x - 0.03, y, -hb.z - 0.0062),
                Vec3::new(x + 0.03, y, -hb.z - 0.0062),
            ],
            0.0055,
            10,
            true,
            steel,
        );
        for (sy, len) in [(1.0f32, (lid_h - 0.02).min(0.06)), (-1.0, 0.07)] {
            bevel_box(
                b,
                &Frame::at(Vec3::new(x, y + sy * (0.004 + len * 0.5), -hb.z - 0.0018)),
                &BoxSpec::new(Vec3::new(0.028, len * 0.5, 0.0018), 0.0012)
                    .faces(ALL & !PZ)
                    .tint(steel),
            );
            for dx in [-0.014f32, 0.014] {
                nail(
                    b,
                    Vec3::new(x + dx, y + sy * (0.004 + len * 0.6), -hb.z - 0.0036),
                    Vec3::NEG_Z,
                    0.0038,
                    steel,
                );
            }
        }
    }
    kit.center();
    kit.finish(Ao {
        reach: 0.2,
        strength: 0.9,
        ground: true,
    })
}

/// EUR pallet (1.2 × 0.144 × 0.8 m): three bottom boards, nine blocks, three stringer boards
/// and five top deck boards with chamfered edges, nailed at every crossing.
pub fn pallet(seed: u64) -> Vec<PropPart> {
    let mut rng = Rng::new(seed ^ 0x9A11E7);
    let mut kit = Kit::new();
    let t = 0.022;
    let y0 = -0.072;
    let block_h = 0.078;
    let up = Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y);
    let down = Mat3::from_cols(Vec3::X, Vec3::Z, Vec3::NEG_Y);
    let across = Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y);
    let rows = [(-0.35f32, 0.1f32), (0.0, 0.145), (0.35, 0.1)];
    let cols = [-0.5275f32, 0.0, 0.5275];
    let col_edges: Vec<f32> = cols.iter().flat_map(|c| [c - 0.0725, c + 0.0725]).collect();
    let xs = stations(-0.6, 0.6, &col_edges, 0.2);
    for (zc, w) in rows {
        let f = Frame::new(Vec3::new(0.0, y0 + t * 0.5, zc), down);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(t * 0.5, w * 0.5),
            &xs,
            0.004,
            0.003,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    let yb = y0 + t + block_h * 0.5;
    for &xc in &cols {
        for (zc, w) in rows {
            let tint = mul(wood_tint(&mut rng), 0.95);
            bevel_box(
                kit.mb(WOOD),
                &Frame::at(Vec3::new(xc, yb, zc)),
                &BoxSpec::new(Vec3::new(0.0725, block_h * 0.5, w * 0.5), 0.005)
                    .faces(PX | NX | PZ | NZ)
                    .splits([&[], &[0.0], &[]])
                    .tint(tint)
                    .uv(rng_uv(&mut rng)),
            );
        }
    }
    let ys = y0 + t + block_h + t * 0.5;
    let row_edges: Vec<f32> = rows
        .iter()
        .flat_map(|(z, w)| [z - w * 0.5, z + w * 0.5])
        .collect();
    let zs = stations(-0.4, 0.4, &row_edges, 0.2);
    for &xc in &cols {
        let f = Frame::new(Vec3::new(xc, ys, 0.0), across);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(t * 0.5, 0.0725),
            &zs,
            0.003,
            0.002,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    let yt = ys + t;
    let deck = [
        (-0.3275f32, 0.145f32),
        (-0.16375, 0.1),
        (0.0, 0.145),
        (0.16375, 0.1),
        (0.3275, 0.145),
    ];
    for (zc, w) in deck {
        let f = Frame::new(Vec3::new(rng.range(-0.002, 0.002), yt, zc), up);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(t * 0.5, w * 0.5),
            &xs,
            0.005,
            0.002,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
        for &xc in &cols {
            for k in [-1.0f32, 1.0] {
                let p = Vec3::new(
                    xc + k * 0.035 + rng.range(-0.006, 0.006),
                    yt + t * 0.5,
                    zc - k * w * 0.22,
                );
                nail(kit.mb(METAL), p, Vec3::Y, 0.0042, [0.6, 0.58, 0.55]);
            }
        }
    }
    kit.finish(Ao {
        reach: 0.15,
        strength: 0.9,
        ground: true,
    })
}

/// Sawn post or beam that fills exactly the box `size` (it stands in for a structural box
/// piece). The longest axis runs along the grain; arrises are chamfered, faces slightly
/// uneven with a seasoning check, end grain darker.
pub fn timber(size: Vec3, seed: u64) -> Vec<PropPart> {
    let size = size.max(Vec3::splat(0.03));
    let mut rng = Rng::new(seed ^ 0x7173_BE55);
    let mut kit = Kit::new();
    let long = if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    };
    let rot = match long {
        0 => Mat3::IDENTITY,
        1 => Mat3::from_cols(Vec3::Y, Vec3::Z, Vec3::X),
        _ => Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y),
    };
    let half = (rot.transpose() * (size * 0.5)).abs();
    let c = (half.y.min(half.z) * 0.12).clamp(0.003, 0.012);
    let along = stations(-half.x, half.x, &[], 0.12);
    let crack_y = rng.range(-0.35, 0.35) * half.y;
    let crack_x = (
        rng.range(-0.9, -0.1) * half.x,
        rng.range(0.2, 0.95) * half.x,
    );
    let ys = [
        -half.y * 0.85,
        -half.y * 0.7,
        -half.y * 0.45,
        crack_y - 0.003,
        crack_y,
        crack_y + 0.003,
        half.y * 0.45,
        half.y * 0.7,
        half.y * 0.85,
    ];
    let zs = [
        -half.z * 0.85,
        -half.z * 0.7,
        -half.z * 0.45,
        0.0,
        half.z * 0.45,
        half.z * 0.7,
        half.z * 0.85,
    ];
    // Wane: one arris keeps the round outside of the log, widest towards one end.
    let wane_s = Vec2::new(
        if rng.chance(0.5) { 1.0 } else { -1.0 },
        if rng.chance(0.5) { 1.0 } else { -1.0 },
    );
    let wane_end = if rng.chance(0.5) { 1.0f32 } else { -1.0 };
    let hd = Vec2::new(half.y, half.z).length();
    let wane_r = move |x: f32| {
        let t = (x / half.x * wane_end + 1.0) * 0.5;
        hd * (0.74 + 0.36 * t * t)
    };
    let tint = wood_tint(&mut rng);
    let b = kit.mb(WOOD);
    let v0 = b.vertices.len();
    let t0 = b.indices.len();
    let f = Frame::new(Vec3::ZERO, rot);
    bevel_box(
        b,
        &f,
        &BoxSpec::new(half, c)
            .chamfer()
            .splits([&along, &ys, &zs])
            .tint(tint)
            .uv(rng_uv(&mut rng)),
    );
    let s = seed;
    deform(b, v0, |p| {
        let lp = rot.transpose() * p;
        let on_end = lp.x.abs() > half.x - 1e-4;
        let grain = Vec3::new(lp.x * 0.15, lp.y * 1.0, lp.z * 1.0);
        let mut amount = 0.0016 + 0.0018 * fbm(grain, 9.0, s, 3);
        if lp.z > half.z - 1e-4
            && (lp.y - crack_y).abs() < 1e-4
            && lp.x > crack_x.0
            && lp.x < crack_x.1
        {
            amount += 0.012
                * (PI * (lp.x - crack_x.0) / (crack_x.1 - crack_x.0))
                    .sin()
                    .powf(0.6);
        }
        // Dings along the arrises.
        let arris = (lp.y.abs() > half.y - c * 1.5) && (lp.z.abs() > half.z - c * 1.5);
        if arris {
            amount += 0.004 * smoothstep(0.2, 0.55, fbm(lp, 14.0, s ^ 0xD1, 2));
        }
        let dir = if on_end {
            Vec3::new(-lp.x.signum(), 0.0, 0.0)
        } else {
            -Vec3::new(0.0, lp.y, lp.z).normalize_or(Vec3::Y)
        };
        let amount = if on_end {
            0.0006 + 0.0006 * n3(lp, 60.0, s)
        } else {
            amount
        };
        let mut q = lp + dir * amount.max(0.0);
        let yz = Vec2::new(q.y, q.z);
        let rw = wane_r(q.x) * (1.0 + 0.02 * n3(lp, 18.0, s ^ 0x3E));
        if yz.y * wane_s.y > 0.0 && yz.x * wane_s.x > 0.0 && yz.length() > rw {
            let w = yz * (rw / yz.length());
            q.y = w.x;
            q.z = w.y;
        }
        rot * q
    });
    auto_smooth(b, v0, t0, 30.0);
    shade(b, v0, |p, _| {
        let lp = rot.transpose() * p;
        let k = 1.0 + 0.14 * fbm(Vec3::new(lp.x * 0.12, lp.y, lp.z), 10.0, s ^ 5, 3);
        let yz = Vec2::new(lp.y, lp.z);
        let wane =
            yz.y * wane_s.y > 0.0 && yz.x * wane_s.x > 0.0 && yz.length() > wane_r(lp.x) * 0.975;
        if lp.x.abs() > half.x - 0.0015 {
            [0.8 * k, 0.72 * k, 0.64 * k]
        } else if wane {
            let bark = 0.5 + 0.5 * smoothstep(-0.3, 0.3, fbm(lp, 25.0, s ^ 0xBA, 2));
            mix3(
                [0.66 * k, 0.56 * k, 0.46 * k],
                [0.86 * k, 0.8 * k, 0.7 * k],
                bark,
            )
        } else {
            [k; 3]
        }
    });
    kit.finish(Ao {
        reach: 0.12,
        strength: 0.7,
        ground: false,
    })
}

// ============================================================================================
// Steel drum
// ============================================================================================

const DRUM_DIAMETER: f32 = 0.585;
const DRUM_HEIGHT: f32 = 0.88;

/// 208 L (55 US gal) steel oil drum, 0.585 m × 0.88 m: rolled chimes, two rolling hoops and
/// shallow swages, recessed heads, 2" and ¾" bung caps on the lid; `dents` random dents.
pub fn steel_drum(dents: u32, seed: u64) -> Vec<PropPart> {
    let mut rng = Rng::new(seed ^ 0xD20208);
    let mut kit = Kit::new();
    let rb = 0.2865;
    let rm = DRUM_DIAMETER * 0.5;
    let hgt = DRUM_HEIGHT;
    let p = Vec2::new;
    let mut prof = vec![
        p(0.0, 0.012),
        p(0.262, 0.012),
        p(0.268, 0.0105),
        p(0.2725, 0.006),
        p(0.2765, 0.0015),
        p(0.2805, 0.0),
        p(0.2865, 0.0006),
        p(0.2905, 0.0028),
        p(rm, 0.0075),
        p(0.2918, 0.0125),
        p(0.2893, 0.0165),
        p(0.2872, 0.021),
    ];
    // (center height, outward bulge, half width, steep hoop?)
    let features = [
        (0.155f32, 0.0018f32, 0.009f32, false),
        (hgt / 3.0, rm - rb, 0.015, true),
        (0.44, 0.0018, 0.009, false),
        (2.0 * hgt / 3.0, rm - rb, 0.015, true),
        (0.725, 0.0018, 0.009, false),
    ];
    let bump = |y: f32| -> f32 {
        features
            .iter()
            .map(|&(c, a, w, hoop)| {
                let x = ((y - c) / w).abs();
                if x >= 1.0 {
                    0.0
                } else if hoop {
                    a * (1.0 - x * x).powf(1.5)
                } else {
                    a * (1.0 - x * x).powi(2)
                }
            })
            .sum()
    };
    let mut ys: Vec<f32> = stations(0.027, 0.853, &[], 0.075)
        .into_iter()
        .filter(|y| {
            features.iter().all(|f| (y - f.0).abs() > f.2 + 0.006) || *y < 0.03 || *y > 0.85
        })
        .collect();
    for &(c, _, w, hoop) in &features {
        let ks: &[f32] = if hoop {
            &[-1.0, -0.72, -0.45, -0.2, 0.0, 0.2, 0.45, 0.72, 1.0]
        } else {
            &[-1.0, -0.5, 0.0, 0.5, 1.0]
        };
        ys.extend(ks.iter().map(|k| c + k * w));
    }
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.002);
    prof.extend(ys.iter().map(|&y| p(rb + bump(y), y)));
    prof.extend([
        p(0.2872, 0.859),
        p(0.2893, 0.8635),
        p(0.2918, 0.8675),
        p(rm, 0.8725),
        p(0.2905, 0.8772),
        p(0.2865, 0.8794),
        p(0.2805, hgt),
        p(0.2765, 0.8785),
        p(0.2728, 0.874),
        p(0.2705, 0.869),
    ]);
    let segs = 36;
    let b = kit.mb(METAL);
    let v0 = b.vertices.len();
    let t0 = b.indices.len();
    lathe(
        b,
        &Frame::ID,
        &prof,
        segs,
        35.0,
        LatheUv::Wrap(rb),
        [1.0; 3],
    );
    if dents > 0 {
        let list: Vec<(f32, f32, f32, f32)> = (0..dents)
            .map(|_| {
                let mut y = rng.range(0.08, 0.8);
                if features.iter().any(|f| f.3 && (y - f.0).abs() < 0.05) {
                    y += 0.09;
                }
                (
                    rng.range(0.0, TAU),
                    y,
                    rng.range(0.05, 0.11),
                    rng.range(0.006, 0.016),
                )
            })
            .collect();
        deform(b, v0, |q| {
            if q.y < 0.03 || q.y > 0.85 {
                return q;
            }
            let r = Vec2::new(q.x, q.z).length();
            if r < 0.27 {
                return q;
            }
            let th = (-q.z).atan2(q.x);
            let mut d = 0.0;
            for &(a, y, size, depth) in &list {
                let mut da = (th - a).rem_euclid(TAU);
                if da > PI {
                    da -= TAU;
                }
                let dist2 = (da * rb).powi(2) + (q.y - y).powi(2);
                d += depth * (-dist2 / (size * size)).exp() * (1.0 + 0.4 * n3(q, 30.0, seed));
            }
            let radial = Vec3::new(q.x, 0.0, q.z) / r;
            q - radial * d
        });
        auto_smooth(b, v0, t0, 35.0);
    }
    let head = [
        p(0.2705, 0.869),
        p(0.262, 0.8685),
        p(0.236, 0.8685),
        p(0.232, 0.8705),
        p(0.226, 0.8715),
        p(0.0, 0.8715),
    ];
    lathe(b, &Frame::ID, &head, segs, 35.0, LatheUv::Planar, [1.0; 3]);
    let a0 = rng.range(0.0, TAU);
    for (a, flange, plug) in [(a0, 0.034f32, 0.024f32), (a0 + PI, 0.021, 0.014)] {
        let c = Vec3::new(0.195 * a.cos(), 0.8715, -0.195 * a.sin());
        lathe(
            b,
            &Frame::at(c),
            &[
                p(flange, -0.0005),
                p(flange, 0.0015),
                p(flange - 0.002, 0.0042),
                p(0.0, 0.0042),
            ],
            20,
            40.0,
            LatheUv::Planar,
            [0.92; 3],
        );
        hex_head(b, c + Vec3::Y * 0.004, Vec3::Y, plug, 0.009, [0.9; 3]);
    }
    let s = seed;
    shade(b, 0, |q, _| {
        let dirt = 0.78 + 0.22 * smoothstep(0.0, 0.2, q.y);
        let edge = if q.y < 0.03 || q.y > 0.85 { 1.0 } else { 0.3 };
        let rust = smoothstep(0.1, 0.45, fbm(q, 9.0, s, 3)) * edge;
        let c = mix3([dirt; 3], [0.6 * dirt, 0.42 * dirt, 0.3 * dirt], rust * 0.7);
        let k = 1.0 + 0.05 * n3(q, 14.0, s ^ 9);
        mul(c, k)
    });
    kit.translate(Vec3::new(0.0, -hgt * 0.5, 0.0));
    kit.finish(Ao {
        reach: 0.2,
        strength: 0.85,
        ground: true,
    })
}

// ============================================================================================
// Sandbags
// ============================================================================================

const BAG_LEN: f32 = 0.6;
const BAG_WIDTH: f32 = 0.35;
const BAG_HEIGHT: f32 = 0.15;
const COURSE: f32 = 0.115;
const WALL_BAG_HEIGHT: f32 = 0.14;

#[derive(Debug, Clone, Copy)]
struct Bag {
    len: f32,
    width: f32,
    height: f32,
    /// Tied neck and tuft at +X (else both ends are folded pillow ends).
    tied: bool,
    /// Sag over the joint of the course below, meters.
    dip: f32,
    rings: usize,
    around: usize,
    seed: u64,
}

/// A soft filled bag lying on its flat bottom, local X along its length.
fn bag_mesh(b: &mut MeshBuilder, f: &Frame, bag: &Bag) {
    let v0 = b.vertices.len();
    let t0 = b.indices.len();
    let (hl, hw, hh) = (bag.len * 0.5, bag.width * 0.5, bag.height * 0.5);
    let neck_d = 0.07;
    let r_neck = 0.026;
    let s = bag.seed;
    let nr = bag.rings;
    let xs: Vec<f32> = (0..nr)
        .map(|k| {
            let t = k as f32 / (nr - 1) as f32;
            let g = 0.4 * t + 0.6 * (1.0 - (PI * t).cos()) * 0.5;
            -hl + bag.len * g
        })
        .collect();
    let thetas: Vec<f32> = (0..=bag.around)
        .map(|k| {
            let psi = k as f32 / bag.around as f32 * TAU;
            psi - 0.3 * (2.0 * psi).sin()
        })
        .collect();
    // (half width, top height, bottom height, center height, gather 0..1, tuft 0..1)
    let section = |x: f32| -> (f32, f32, f32, f32, f32, f32) {
        let end = |d: f32| {
            let c = (d / 0.11).clamp(0.0, 1.0);
            (c, 1.0 - (1.0 - c).powf(2.4))
        };
        let (c0, e0) = end(x + hl);
        let mut w = hw * (0.9 + 0.1 * c0);
        let mut ht = hh * 1.1 * e0.powf(0.55);
        let mut hb = hh * 0.9 * e0.powf(0.4);
        let (mut gather, mut tuft) = (0.0, 0.0);
        let d1 = hl - x;
        if bag.tied {
            if d1 > neck_d {
                let g = 1.0 - smoothstep(neck_d, neck_d + 0.17, d1);
                let e = g.powf(1.3);
                w = lerp(w, r_neck, e);
                ht = lerp(ht, r_neck, e);
                hb = lerp(hb, r_neck, e);
                gather = g;
            } else {
                let t = 1.0 - d1 / neck_d;
                let r = r_neck * (0.8 + 1.0 * (t * PI * 0.8).sin()) * (1.0 - 0.85 * t.powi(8));
                w = r;
                ht = r;
                hb = r;
                gather = 1.0;
                tuft = t;
            }
        } else {
            let (c1, e1) = end(d1);
            w *= 0.9 + 0.1 * c1;
            ht *= e1.powf(0.55);
            hb *= e1.powf(0.4);
        }
        let yc = -hh + hb + gather * 0.032 - tuft * 0.012;
        (w, ht, hb, yc, gather, tuft)
    };
    let rows = (bag.around + 1) as u32;
    for &x in &xs {
        let (w, ht, hb, yc, gather, tuft) = section(x);
        let n = lerp(2.5, 2.0, gather);
        let mut arc = 0.0;
        let mut prev: Option<Vec3> = None;
        for &th in &thetas {
            let (sn, cs) = th.sin_cos();
            let seam = 0.004 * (1.0 - gather) * (-(sn / 0.09).powi(2)).exp() * cs.abs();
            let z = (w + seam) * cs.signum() * cs.abs().powf(2.0 / n);
            let y = if sn >= 0.0 {
                ht * sn.powf(2.0 / lerp(2.2, 2.0, gather))
            } else {
                -hb * (-sn).powf(2.0 / lerp(3.6, 2.0, gather))
            };
            let base = Vec3::new(x, 0.0, z);
            let lump = 1.0
                + 0.035 * fbm(base + Vec3::Y * y, 5.0, s, 3)
                + 0.012 * n3(base + Vec3::Y * y, 22.0, s ^ 3)
                + 0.08 * gather * (9.0 * th + 3.0 * n3(Vec3::new(x, 0.0, 0.0), 12.0, s)).sin()
                + 0.22 * tuft * (7.0 * th + 2.0).sin();
            // Fold creases where the cloth wrinkles over the fill.
            let cp = Vec3::new(x * 1.3 + z * 0.6, y * 1.5, z - x * 0.4);
            let ridge = (1.0 - n3(cp, 7.0, s ^ 0x9F).abs() * 2.2).max(0.0).powi(3);
            let lump = lump - 0.05 * ridge * (1.0 - gather) * smoothstep(-0.4, 0.3, sn);
            let mut q = Vec3::new(x, yc + y * lump, z * lump);
            q.y -= bag.dip * (-(x / 0.13).powi(2)).exp();
            q.y = q.y.max(-hh - bag.dip - 0.004);
            if let Some(pq) = prev {
                arc += q.distance(pq);
            }
            prev = Some(q);
            vert(
                b,
                f.p(q),
                f.d(Vec3::new(0.0, y, z).normalize_or(Vec3::Y)),
                Vec2::new(x, arc),
                [1.0; 3],
            );
        }
    }
    for i in 0..nr as u32 - 1 {
        for j in 0..bag.around as u32 {
            let a = v0 as u32 + i * rows + j;
            let c = a + rows;
            let n = Vec3::from(b.vertices[a as usize].normal)
                + Vec3::from(b.vertices[(c + 1) as usize].normal);
            quad_idx(b, [a, a + 1, c + 1, c], n);
        }
    }
    if bag.tied {
        // Close the tuft and tie a cord around the neck.
        let last = v0 as u32 + (nr as u32 - 1) * rows;
        let (_, _, _, yc, _, _) = section(hl);
        let tip = vert(
            b,
            f.p(Vec3::new(hl + 0.004, yc, 0.0)),
            f.d(Vec3::X),
            Vec2::new(hl, 0.0),
            [1.0; 3],
        );
        for j in 0..bag.around as u32 {
            tri_idx(b, [tip, last + j, last + j + 1], f.d(Vec3::X));
        }
    }
    auto_smooth(b, v0, t0, 75.0);
    if bag.tied {
        let x = hl - neck_d + 0.004;
        let (w, _, _, yc, _, _) = section(x);
        let ring = arc_path(
            Vec3::new(x, yc, 0.0),
            Vec3::Y,
            Vec3::Z,
            w * 0.9 + 0.003,
            0.0,
            TAU,
            20,
        );
        let ring: Vec<Vec3> = ring.iter().map(|q| f.p(*q)).collect();
        let v1 = b.vertices.len();
        tube(b, &ring, 0.0035, 6, false, [0.85, 0.8, 0.7]);
        let knot = Vec3::new(x, yc + w * 0.9 + 0.003, 0.0);
        for side in [-1.0f32, 1.0] {
            let path: Vec<Vec3> = (0..6)
                .map(|k| {
                    let t = k as f32 / 5.0;
                    knot + Vec3::new(
                        0.02 * t + 0.004 * side,
                        -0.045 * t * t,
                        side * (0.006 + 0.03 * t),
                    )
                })
                .map(|q| f.p(q))
                .collect();
            tube(b, &path, 0.003, 5, true, [0.85, 0.8, 0.7]);
        }
        shade(b, v1, |_, _| [0.9; 3]);
    }
    let tint = [
        1.0 + 0.04 * hash01(s as i64, 1, 0, 3),
        1.0,
        0.96 + 0.06 * hash01(s as i64, 2, 0, 3),
    ];
    shade(b, v0, |q, _| {
        let k = 0.9 + 0.1 * fbm(q, 4.0, s ^ 77, 2) + 0.04 * n3(q, 40.0, s);
        mul(tint, k)
    });
}

/// Filled sandbag (about 0.6 × 0.15 × 0.35 m, long along X): a soft pillow with side seams,
/// a sewn flat end and a tied, gathered end with a cord, lying on its flattened bottom.
pub fn sandbag(seed: u64) -> Vec<PropPart> {
    let mut kit = Kit::new();
    let bag = Bag {
        len: BAG_LEN,
        width: BAG_WIDTH - 0.01,
        height: BAG_HEIGHT,
        tied: true,
        dip: 0.0,
        rings: 40,
        around: 28,
        seed,
    };
    bag_mesh(kit.mb(DIRT), &Frame::ID, &bag);
    kit.center();
    kit.finish(Ao {
        reach: 0.15,
        strength: 0.8,
        ground: true,
    })
}

fn sandbag_wall_size(length: f32, courses: u32) -> Vec3 {
    let courses = courses.max(1);
    Vec3::new(
        length.max(BAG_LEN),
        COURSE * (courses - 1) as f32 + WALL_BAG_HEIGHT,
        BAG_WIDTH + 0.04,
    )
}

/// Sandbag wall `length` meters long (along X), `courses` high, bags in running bond with
/// random rotation and sag over the joints below; the ends step like a real stacked wall.
pub fn sandbag_wall(length: f32, courses: u32, seed: u64) -> Vec<PropPart> {
    let size = sandbag_wall_size(length, courses);
    let mut rng = Rng::new(seed ^ 0x5A4D_BA65);
    let mut kit = Kit::new();
    let n_even = (((size.x - BAG_LEN) / 0.55).round() as usize + 1).max(1);
    let pitch = if n_even > 1 {
        (size.x - BAG_LEN) / (n_even - 1) as f32
    } else {
        0.55
    };
    let x0 = -((n_even - 1) as f32) * pitch * 0.5;
    for k in 0..courses.max(1) {
        let odd = k % 2 == 1;
        let n = if odd {
            n_even.saturating_sub(1).max(1)
        } else {
            n_even
        };
        let shift = if odd && n_even > 1 { pitch * 0.5 } else { 0.0 };
        for i in 0..n {
            let len = BAG_LEN * rng.range(0.96, 1.0);
            let hgt = WALL_BAG_HEIGHT * rng.range(0.93, 1.0);
            let yaw = rng.range(-0.035, 0.035);
            let roll = rng.range(-0.025, 0.025);
            let flip = rng.chance(0.5);
            let x = (x0 + shift + i as f32 * pitch + rng.range(-0.012, 0.012))
                .clamp(-size.x * 0.5 + len * 0.5, size.x * 0.5 - len * 0.5);
            let y = -size.y * 0.5 + k as f32 * COURSE + hgt * 0.5;
            let z = rng.range(-0.01, 0.01);
            let mut rot = Mat3::from_rotation_y(yaw) * Mat3::from_rotation_x(roll);
            if flip {
                rot = Mat3::from_rotation_y(PI) * rot;
            }
            let end_bag = i == 0 || i + 1 == n;
            let bag = Bag {
                len,
                width: BAG_WIDTH * rng.range(0.95, 1.0),
                height: hgt,
                tied: end_bag && rng.chance(0.4),
                dip: if k > 0 { rng.range(0.008, 0.016) } else { 0.0 },
                rings: 26,
                around: 20,
                seed: seed ^ (k as u64 * 977 + i as u64 * 131 + 1),
            };
            bag_mesh(kit.mb(DIRT), &Frame::new(Vec3::new(x, y, z), rot), &bag);
        }
    }
    kit.finish(Ao {
        reach: 0.2,
        strength: 0.85,
        ground: true,
    })
}

// ============================================================================================
// Concrete
// ============================================================================================

/// Insert points where the closed polygon crosses height `y` (so lofts can be split there
/// without T-junctions).
fn insert_level(poly: &[Vec2], y: f32) -> Vec<Vec2> {
    let n = poly.len();
    let mut out = Vec::new();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        out.push(a);
        if (a.y - y) * (b.y - y) < 0.0 {
            let t = (y - a.y) / (b.y - a.y);
            out.push(a.lerp(b, t));
        }
    }
    out
}

/// Concrete F-shape safety barrier (3 m × 0.81 m × 0.6 m base, long along X): F-shape faces
/// with rounded slope breaks, chamfered ends, two forklift/drainage slots through the base and
/// steel pin loops at both ends.
pub fn jersey_barrier(seed: u64) -> Vec<PropPart> {
    let mut kit = Kit::new();
    let hx = 1.455;
    let slot_y = 0.1;
    let p = Vec2::new;
    let pts = [
        p(-0.3, 0.0),
        p(0.3, 0.0),
        p(0.3, 0.075),
        p(0.174, 0.255),
        p(0.1185, 0.81),
        p(-0.1185, 0.81),
        p(-0.174, 0.255),
        p(-0.3, 0.075),
    ];
    let radii = [0.008, 0.008, 0.025, 0.09, 0.03, 0.03, 0.09, 0.025];
    let segs = [1, 1, 2, 4, 3, 3, 4, 2];
    let mut full = insert_level(&fillet(&pts, &radii, &segs), slot_y);
    for y in [0.17, 0.36, 0.46, 0.56, 0.66, 0.74] {
        full = insert_level(&full, y);
    }
    let upper: Vec<Vec2> = full
        .iter()
        .copied()
        .filter(|q| q.y >= slot_y - 1e-5)
        .collect();
    let lower: Vec<Vec2> = full
        .iter()
        .copied()
        .filter(|q| q.y <= slot_y + 1e-5)
        .collect();
    // Inset ring for the chamfered ends.
    let c = 0.025;
    let m = full.len();
    let inset: Vec<Vec2> = (0..m)
        .map(|i| {
            let a = full[(i + m - 1) % m];
            let q = full[i];
            let b2 = full[(i + 1) % m];
            let n0 = (q - a).perp().normalize_or_zero();
            let n1 = (b2 - q).perp().normalize_or_zero();
            let inward = (n0 + n1).normalize_or(Vec2::Y);
            if q.y < 1e-4 {
                Vec2::new(q.x - q.x.signum() * c, q.y)
            } else {
                q + inward * c
            }
        })
        .collect();
    let slots = [(-0.97f32, -0.63f32), (0.63, 0.97)];
    let b = kit.mb(CONCRETE);
    let tint = [1.0; 3];
    let up = Mat3::IDENTITY;
    let f = Frame::new(Vec3::ZERO, up);
    // Chamfered end bands and end faces.
    loft(
        b,
        &f,
        &[(-hx, &inset), (-hx + c, &full)],
        40.0,
        [true, false],
        tint,
        Vec2::ZERO,
    );
    loft(
        b,
        &f,
        &[(hx - c, &full), (hx, &inset)],
        40.0,
        [false, true],
        tint,
        Vec2::ZERO,
    );
    let mut edges = vec![-hx + c];
    for &(s0, s1) in &slots {
        edges.push(s0);
        edges.push(s1);
    }
    edges.push(hx - c);
    for (k, w) in edges.windows(2).enumerate() {
        let in_slot = k % 2 == 1;
        let xs = stations(w[0], w[1], &[], 0.075);
        let prof: &[Vec2] = if in_slot { &upper } else { &full };
        let rings: Vec<(f32, &[Vec2])> = xs.iter().map(|&x| (x, prof)).collect();
        loft(b, &f, &rings, 40.0, [false, false], tint, Vec2::ZERO);
        if in_slot {
            // Slot side walls, facing into the slot.
            for (x, dir) in [(w[0], 1.0f32), (w[1], -1.0)] {
                let n = Vec3::X * dir;
                let base = b.vertices.len() as u32;
                for &q in &lower {
                    let lp = Vec3::new(x, q.y, q.x);
                    vert(b, lp, n, box_uv(lp, n), tint);
                }
                for t in triangulate(&lower) {
                    tri_idx(
                        b,
                        [base + t[0] as u32, base + t[1] as u32, base + t[2] as u32],
                        n,
                    );
                }
            }
        }
    }
    // Knocks along the top arrises and the ends, then road grime, rain streaks and scuffs.
    deform(b, 0, |q| {
        let top = smoothstep(0.72, 0.8, q.y) * smoothstep(0.05, 0.11, q.z.abs());
        let end =
            smoothstep(hx - 0.06, hx, q.x.abs()) * smoothstep(0.1, 0.25, q.z.abs() + q.y * 0.2);
        let e = top.max(end);
        if e <= 0.0 {
            return q;
        }
        let mask = smoothstep(0.18, 0.5, fbm(q, 7.0, seed ^ 0xC4, 3));
        let core = Vec3::new(q.x.clamp(-hx + 0.15, hx - 0.15), q.y.clamp(0.15, 0.6), 0.0);
        q + (core - q).normalize_or_zero() * 0.022 * e * mask
    });
    auto_smooth(b, 0, 0, 40.0);
    shade(b, 0, |q, n| {
        let dirt = 0.6 + 0.4 * smoothstep(0.0, 0.38, q.y + 0.06 * fbm(q, 3.0, seed ^ 5, 2));
        let streak = smoothstep(
            0.0,
            0.55,
            fbm(Vec3::new(q.x * 2.5, q.y * 0.25, q.z * 2.5), 2.5, seed, 3),
        ) * (1.0 - n.y.max(0.0));
        let scuff = smoothstep(
            0.3,
            0.65,
            fbm(Vec3::new(q.x * 0.4, q.y * 3.0, q.z), 6.0, seed ^ 9, 3),
        ) * (1.0 - smoothstep(0.2, 0.4, q.y));
        let blotch = smoothstep(0.2, 0.55, fbm(q, 1.5, seed ^ 13, 3));
        let k = (1.0 + 0.1 * fbm(q, 3.0, seed ^ 11, 3))
            * (1.0 - 0.18 * streak)
            * (1.0 - 0.2 * scuff)
            * (1.0 + 0.08 * blotch);
        [dirt * k, dirt * k * 0.985, dirt * k * 0.95]
    });
    // Steel pin loops (16 mm bar) protruding from both ends at interleaved heights.
    let rebar = [0.72, 0.62, 0.55];
    for (sx, heights) in [(-1.0f32, [0.22f32, 0.6]), (1.0, [0.3, 0.68])] {
        for y in heights {
            let base_x = sx * (hx - 0.02);
            let tip = sx * (hx + 0.006);
            let mut path = vec![Vec3::new(base_x, y, -0.028), Vec3::new(tip, y, -0.028)];
            path.extend(
                arc_path(
                    Vec3::new(tip, y, 0.0),
                    Vec3::NEG_Z,
                    Vec3::X * sx,
                    0.028,
                    0.0,
                    PI,
                    10,
                )
                .into_iter()
                .skip(1),
            );
            path.push(Vec3::new(base_x, y, 0.028));
            tube(kit.mb(METAL), &path, 0.008, 8, false, rebar);
        }
    }
    kit.translate(Vec3::new(0.0, -0.405, 0.0));
    kit.finish(Ao {
        reach: 0.3,
        strength: 0.75,
        ground: true,
    })
}

/// Flat rectangle in the plane through `o` (axes `u`, `v`, normal `u × v`) tiled on the grid
/// `us × vs`; cells centered on a point of `holes` get a round recessed form-tie hole with a
/// plastic plug at the bottom.
fn holed_face(
    b: &mut MeshBuilder,
    o: Vec3,
    u: Vec3,
    v: Vec3,
    us: &[f32],
    vs: &[f32],
    holes: &[Vec2],
    hr: f32,
    depth: f32,
    tint: [f32; 3],
) {
    let n = u.cross(v).normalize();
    let at = |q: Vec2| o + u * q.x + v * q.y;
    for j in 0..vs.len() - 1 {
        for i in 0..us.len() - 1 {
            let (u0, u1, v0, v1) = (us[i], us[i + 1], vs[j], vs[j + 1]);
            let center = Vec2::new((u0 + u1) * 0.5, (v0 + v1) * 0.5);
            if !holes.iter().any(|h| h.distance(center) < 1e-3) {
                let q = [
                    Vec2::new(u0, v0),
                    Vec2::new(u1, v0),
                    Vec2::new(u1, v1),
                    Vec2::new(u0, v1),
                ];
                let idx = q.map(|c| {
                    let p = at(c);
                    vert(b, p, n, box_uv(p, n), tint)
                });
                quad_idx(b, idx, n);
                continue;
            }
            let (hu, hv) = ((u1 - u0) * 0.5, (v1 - v0) * 0.5);
            let outer = [
                Vec2::new(hu, 0.0),
                Vec2::new(hu, hv),
                Vec2::new(0.0, hv),
                Vec2::new(-hu, hv),
                Vec2::new(-hu, 0.0),
                Vec2::new(-hu, -hv),
                Vec2::new(0.0, -hv),
                Vec2::new(hu, -hv),
            ];
            let circle: Vec<Vec2> = (0..8)
                .map(|k| Vec2::from_angle(k as f32 * PI / 4.0) * hr)
                .collect();
            let plug = mul(tint, 0.7);
            for k in 0..8 {
                let k1 = (k + 1) % 8;
                let q = [outer[k], outer[k1], circle[k1], circle[k]];
                let idx = q.map(|c| {
                    let p = at(center + c);
                    vert(b, p, n, box_uv(p, n), tint)
                });
                quad_idx(b, idx, n);
                let w = [k, k1].map(|i| {
                    let c = circle[i];
                    let p = at(center + c);
                    let nn = -(u * c.x + v * c.y).normalize();
                    (p, nn)
                });
                let idx = [
                    vert(b, w[0].0, w[0].1, box_uv(w[0].0, n), tint),
                    vert(b, w[1].0, w[1].1, box_uv(w[1].0, n), tint),
                    vert(b, w[1].0 - n * depth, w[1].1, box_uv(w[1].0, n), plug),
                    vert(b, w[0].0 - n * depth, w[0].1, box_uv(w[0].0, n), plug),
                ];
                quad_idx(b, idx, w[0].1 + w[1].1);
            }
            let c = vert(
                b,
                at(center) - n * depth * 0.8,
                n,
                box_uv(at(center), n),
                plug,
            );
            let base = b.vertices.len() as u32;
            for &cq in &circle {
                let p = at(center + cq) - n * depth;
                vert(b, p, n, box_uv(p, n), plug);
            }
            for k in 0..8u32 {
                tri_idx(b, [c, base + k, base + (k + 1) % 8], n);
            }
        }
    }
}

/// Cast-in-place square concrete column filling `size`: chamfered shaft with form-tie holes,
/// a plinth at the foot and a capital band with a sloped haunch at the top.
pub fn concrete_pillar(size: Vec3, seed: u64) -> Vec<PropPart> {
    let size = size.max(Vec3::new(0.25, 0.8, 0.25));
    let h = size * 0.5;
    let mut kit = Kit::new();
    let ph = (size.y * 0.07).clamp(0.12, 0.3);
    let chh = (size.y * 0.07).clamp(0.14, 0.3);
    let hh = 0.06;
    let sh = Vec2::new(h.x * 0.78, h.z * 0.78);
    let c = 0.022;
    let tint = [1.0; 3];
    let b = kit.mb(CONCRETE);
    let sx = stations(-h.x, h.x, &[], 0.15);
    let sz = stations(-h.z, h.z, &[], 0.15);
    bevel_box(
        b,
        &Frame::at(Vec3::new(0.0, -h.y + ph * 0.5, 0.0)),
        &BoxSpec::new(Vec3::new(h.x, ph * 0.5, h.z), c)
            .chamfer()
            .faces(ALL & !NY)
            .splits([&sx, &[], &sz]),
    );
    bevel_box(
        b,
        &Frame::at(Vec3::new(0.0, h.y - chh * 0.5, 0.0)),
        &BoxSpec::new(Vec3::new(h.x, chh * 0.5, h.z), c)
            .chamfer()
            .splits([&sx, &[], &sz]),
    );
    let y0 = -h.y + ph;
    let y1 = h.y - chh - hh;
    let vertical = Frame::new(Vec3::ZERO, Mat3::from_cols(Vec3::Y, Vec3::Z, Vec3::X));
    let top = sh + (Vec2::new(h.x, h.z) - sh) * 0.55;
    let shaft = chamfered_rect(sh, c);
    let haunch = chamfered_rect(top, c);
    loft(
        b,
        &vertical,
        &[(y1, &shaft), (y1 + hh, &haunch)],
        30.0,
        [false, false],
        tint,
        Vec2::ZERO,
    );
    // Shaft faces with form-tie holes on a grid.
    let hs = 0.04;
    let mut rows = Vec::new();
    let mut y = y0 + 0.35;
    while y + hs < y1 - 0.15 {
        rows.push(y);
        y += 0.6;
    }
    let mut vs_must = Vec::new();
    for &r in &rows {
        vs_must.push(r - hs);
        vs_must.push(r + hs);
    }
    // Formwork lifts every 1.2 m leave a joint line and a slightly different shade.
    let lift = 1.2;
    let mut joints = Vec::new();
    let mut jy = y0 + lift;
    while jy < y1 - 0.1 {
        if rows.iter().all(|r| (r - jy).abs() > hs + 0.03) {
            joints.push(jy);
            vs_must.extend([jy - 0.012, jy, jy + 0.012]);
        }
        jy += lift;
    }
    let vs = stations(y0, y1, &vs_must, 0.12);
    let faces = [
        (Vec3::Z, Vec3::X, sh.y, sh.x),
        (Vec3::X, Vec3::NEG_Z, sh.x, sh.y),
        (Vec3::NEG_Z, Vec3::NEG_X, sh.y, sh.x),
        (Vec3::NEG_X, Vec3::Z, sh.x, sh.y),
    ];
    for (n, u, dist, hu) in faces {
        let col = hu * 0.5;
        let us = stations(
            -(hu - c),
            hu - c,
            &[-col - hs, -col + hs, col - hs, col + hs],
            0.095,
        );
        let holes: Vec<Vec2> = rows
            .iter()
            .flat_map(|&r| [Vec2::new(-col, r), Vec2::new(col, r)])
            .collect();
        holed_face(b, n * dist, u, Vec3::Y, &us, &vs, &holes, 0.013, 0.02, tint);
    }
    for (cx, cz) in [(1.0f32, 1.0f32), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)] {
        let a = Vec3::new(cx * (sh.x - c), 0.0, cz * sh.y);
        let d = Vec3::new(cx * sh.x, 0.0, cz * (sh.y - c));
        let n = Vec3::new(cx, 0.0, cz).normalize();
        for w in vs.windows(2) {
            let q = [
                a + Vec3::Y * w[0],
                d + Vec3::Y * w[0],
                d + Vec3::Y * w[1],
                a + Vec3::Y * w[1],
            ];
            let idx = q.map(|p| vert(b, p, n, box_uv(p, n), tint));
            quad_idx(b, idx, n);
        }
    }
    shade(b, 0, |q, n| {
        let dirt = 0.62 + 0.38 * smoothstep(-h.y, -h.y + 0.6, q.y);
        let streak = smoothstep(
            0.0,
            0.6,
            fbm(Vec3::new(q.x * 2.0, q.y * 0.25, q.z * 2.0), 2.0, seed, 3),
        ) * (1.0 - n.y.max(0.0));
        let joint = joints
            .iter()
            .map(|j| (-((q.y - j) / 0.008).powi(2)).exp())
            .fold(0.0f32, f32::max);
        let li = ((q.y - y0) / lift).floor() as i64;
        let lift_k = 1.0 + 0.07 * (hash01(li, 3, 0, seed) - 0.5);
        let blotch = smoothstep(0.25, 0.6, fbm(q, 1.6, seed ^ 7, 3));
        let k = (1.0 + 0.12 * fbm(q, 2.5, seed ^ 3, 3))
            * (1.0 - 0.2 * streak)
            * dirt
            * (1.0 - 0.2 * joint)
            * lift_k
            * (1.0 + 0.1 * blotch);
        [k, k * 0.985, k * 0.955]
    });
    kit.finish(Ao {
        reach: 0.3,
        strength: 0.8,
        ground: true,
    })
}

// ============================================================================================
// Steel beam
// ============================================================================================

fn i_beam_dims(depth: f32) -> (f32, f32, f32, f32, f32) {
    let d = depth.clamp(0.08, 1.2);
    let bf = 0.5 * d;
    let tw = (0.024 * d).max(0.0045);
    let tf = (0.036 * d).max(0.0065);
    let r = (0.05 * d).max(0.006);
    (d, bf, tw, tf, r)
}

const PLATE_T: f32 = 0.02;

fn plate_extension(d: f32) -> f32 {
    (0.25 * d).clamp(0.05, 0.12)
}

fn i_beam_size(length: f32, depth: f32, end_plates: bool) -> Vec3 {
    let (d, bf, ..) = i_beam_dims(depth);
    if end_plates {
        Vec3::new(length.max(0.3), d + 2.0 * plate_extension(d), bf + 0.04)
    } else {
        Vec3::new(length.max(0.3), d, bf)
    }
}

/// Hot-rolled I-beam (IPE proportions: flange width ½ depth) `length` meters along X and
/// `depth` meters deep, with root fillets and rounded flange toes; `end_plates` adds welded,
/// extended end plates with eight bolts each.
pub fn steel_i_beam(length: f32, depth: f32, end_plates: bool) -> Vec<PropPart> {
    let len = length.max(0.3);
    let (d, bf, tw, tf, rr) = i_beam_dims(depth);
    let mut kit = Kit::new();
    let (hb, hd) = (bf * 0.5, d * 0.5);
    let p = Vec2::new;
    let pts = [
        p(-hb, -hd),
        p(hb, -hd),
        p(hb, -hd + tf),
        p(tw * 0.5, -hd + tf),
        p(tw * 0.5, hd - tf),
        p(hb, hd - tf),
        p(hb, hd),
        p(-hb, hd),
        p(-hb, hd - tf),
        p(-tw * 0.5, hd - tf),
        p(-tw * 0.5, -hd + tf),
        p(-hb, -hd + tf),
    ];
    let toe = tf * 0.3;
    let radii = [
        0.0015, 0.0015, toe, rr, rr, toe, 0.0015, 0.0015, toe, rr, rr, toe,
    ];
    let segs = [2, 2, 2, 4, 4, 2, 2, 2, 2, 4, 4, 2];
    let prof = fillet(&pts, &radii, &segs);
    let x_end = if end_plates {
        len * 0.5 - PLATE_T
    } else {
        len * 0.5
    };
    let xs = stations(-x_end, x_end, &[], 0.4);
    let steel = [0.95, 0.95, 0.96];
    let b = kit.mb(METAL);
    extrude(
        b,
        &Frame::ID,
        &prof,
        &xs,
        50.0,
        !end_plates,
        steel,
        Vec2::ZERO,
    );
    if end_plates {
        let ext = plate_extension(d);
        let half = Vec3::new(PLATE_T * 0.5, hd + ext, hb + 0.02);
        for sx in [-1.0f32, 1.0] {
            let xp = sx * (len * 0.5 - PLATE_T * 0.5);
            bevel_box(
                b,
                &Frame::at(Vec3::new(xp, 0.0, 0.0)),
                &BoxSpec::new(half, 0.002)
                    .splits([&[], &[-hd, hd], &[]])
                    .tint(steel),
            );
            let face = sx * (len * 0.5 - PLATE_T);
            let axis = Vec3::X * -sx;
            let bolts = [
                (hd + ext * 0.5, hb - 0.022),
                (hd + ext * 0.5, -(hb - 0.022)),
                (-(hd + ext * 0.5), hb - 0.022),
                (-(hd + ext * 0.5), -(hb - 0.022)),
                (hd * 0.45, tw * 0.5 + 0.034),
                (hd * 0.45, -(tw * 0.5 + 0.034)),
                (-hd * 0.45, tw * 0.5 + 0.034),
                (-hd * 0.45, -(tw * 0.5 + 0.034)),
            ];
            for (y, z) in bolts {
                let at = Vec3::new(face, y, z);
                lathe(
                    b,
                    &Frame::up_along(at, axis),
                    &[
                        p(0.0, -0.0005),
                        p(0.019, -0.0005),
                        p(0.019, 0.0025),
                        p(0.0175, 0.0035),
                        p(0.0, 0.0035),
                    ],
                    16,
                    40.0,
                    LatheUv::Planar,
                    [0.85; 3],
                );
                hex_head(b, at + axis * 0.0035, axis, 0.0175, 0.013, [0.8; 3]);
            }
            // Fillet welds where the beam meets the plate.
            let leg = 0.007;
            let weld = |b: &mut MeshBuilder, o: Vec3, t: Vec3, m: Vec3, l: f32| {
                let fr = Frame::xz(o, t, t.cross(m));
                let s_q = fr.rot.col(2).dot(axis).signum();
                let s_m = fr.rot.col(1).dot(m).signum();
                let w = [
                    p(0.0, 0.0),
                    p(s_q * leg, 0.0),
                    p(s_q * leg * 0.62, s_m * leg * 0.62),
                    p(0.0, s_m * leg),
                ];
                extrude(
                    b,
                    &fr,
                    &w,
                    &stations(-l * 0.5, l * 0.5, &[], 0.05),
                    60.0,
                    true,
                    [0.75; 3],
                    Vec2::ZERO,
                );
            };
            for sy in [-1.0f32, 1.0] {
                weld(b, Vec3::new(face, sy * hd, 0.0), Vec3::Z, Vec3::Y * sy, bf);
                for sz in [-1.0f32, 1.0] {
                    let zc = sz * (tw * 0.5 + rr + (hb - tw * 0.5 - rr) * 0.5);
                    weld(
                        b,
                        Vec3::new(face, sy * (hd - tf), zc),
                        Vec3::Z,
                        Vec3::Y * -sy,
                        hb - tw * 0.5 - rr,
                    );
                }
            }
            for sz in [-1.0f32, 1.0] {
                weld(
                    b,
                    Vec3::new(face, 0.0, sz * tw * 0.5),
                    Vec3::Y,
                    Vec3::Z * sz,
                    d - 2.0 * (tf + rr),
                );
            }
        }
    }
    shade(b, 0, |q, _| {
        let k = 1.0 + 0.06 * fbm(q, 3.0, 0x57EE1, 3);
        let rust = smoothstep(0.25, 0.6, fbm(q, 6.0, 0x2057, 3));
        mix3([k; 3], [0.62 * k, 0.45 * k, 0.33 * k], rust * 0.45)
    });
    kit.finish(Ao {
        reach: 0.15,
        strength: 0.85,
        ground: true,
    })
}

// ============================================================================================
// Stone column
// ============================================================================================

fn stone_column_size(height: f32, radius: f32) -> Vec3 {
    let r = radius.clamp(0.08, 2.0);
    let h = height.max(r * 4.0);
    Vec3::new(2.76 * r, h, 2.76 * r)
}

/// Classical stone column `height` meters tall with shaft radius `radius` at the foot: square
/// plinth, Attic base (two tori around a scotia), a 20-flute shaft with entasis and rounded
/// flute ends, Doric capital (astragal, annulets, echinus, abacus), weathered and chipped.
/// A dressed ashlar block filling `size` (centered): chamfered arrises, faces tooled with
/// shallow pitting, a few chipped corners and edges, and a per-block tint with darker pits.
pub fn stone_block(size: Vec3, seed: u64) -> Vec<PropPart> {
    let half = size.max(Vec3::splat(0.05)) * 0.5;
    let mut rng = Rng::new(seed ^ 0x570E_B10C);
    let mut kit = Kit::new();
    let c = (half.min_element() * 0.08).clamp(0.008, 0.03);
    let splits: [Vec<f32>; 3] = std::array::from_fn(|a| stations(-half[a], half[a], &[], 0.1));
    let b = kit.mb(STONE);
    let v0 = b.vertices.len();
    let t0 = b.indices.len();
    let tint = {
        let k = rng.range(0.86, 1.06);
        let warm = rng.range(-0.03, 0.03);
        [k * (1.0 + warm), k, k * (1.0 - warm)]
    };
    bevel_box(
        b,
        &Frame::ID,
        &BoxSpec::new(half, c)
            .chamfer()
            .splits([&splits[0], &splits[1], &splits[2]])
            .tint(tint)
            .uv(rng_uv(&mut rng)),
    );
    // Chipped corners: a few corners lose a bite of stone.
    let chips: Vec<(Vec3, f32)> = (0..rng.range(1.0, 4.0) as usize)
        .map(|_| {
            let corner = Vec3::new(
                if rng.chance(0.5) { 1.0 } else { -1.0 },
                if rng.chance(0.5) { 1.0 } else { -1.0 },
                if rng.chance(0.5) { 1.0 } else { -1.0 },
            ) * half;
            (corner, rng.range(0.04, 0.11))
        })
        .collect();
    let s = seed;
    deform(b, v0, |p| {
        // Tooled faces: shallow pitting and a gentle pillow toward the middle of each face.
        let inward = -(p / half).clamp(Vec3::splat(-1.0), Vec3::splat(1.0));
        let face = p.abs() / half;
        let depth = 0.004 * fbm(p, 9.0, s, 3).abs() + 0.0025 * n3(p, 30.0, s ^ 0x71);
        let pillow = 0.006 * (1.0 - face.max_element().powi(8));
        let mut q = p + inward.normalize_or(Vec3::Y) * (depth - pillow).max(-0.004);
        for (corner, r) in &chips {
            let d = (q - *corner).length();
            if d < *r {
                let bite = (*r - d) * (0.7 + 0.3 * n3(q, 25.0, s ^ 0xC4));
                q += (-*corner).normalize_or(Vec3::Y) * bite;
            }
        }
        q
    });
    auto_smooth(b, v0, t0, 35.0);
    shade(b, v0, |p, _| {
        let pit = smoothstep(0.25, 0.7, fbm(p, 9.0, s, 3).abs());
        let mottle = 1.0 + 0.1 * fbm(p, 2.5, s ^ 0x3A, 3);
        let chipped = chips
            .iter()
            .any(|(corner, r)| (p - *corner).length() < *r * 0.95);
        let k = mottle * (1.0 - 0.18 * pit) * if chipped { 1.12 } else { 1.0 };
        [k; 3]
    });
    kit.finish(Ao {
        reach: 0.2,
        strength: 0.7,
        ground: false,
    })
}

pub fn stone_column(height: f32, radius: f32, seed: u64) -> Vec<PropPart> {
    let size = stone_column_size(height, radius);
    let r = radius.clamp(0.08, 2.0);
    let hgt = size.y;
    let mut kit = Kit::new();
    let y_bot = -hgt * 0.5;
    let ph = 0.32 * r;
    let ps = 1.38 * r;
    let y0 = y_bot + ph;
    let base_h = 0.68 * r;
    let ab_h = 0.28 * r;
    let cap_h = 0.76 * r;
    let ys0 = y0 + base_h;
    let ys1 = hgt * 0.5 - cap_h;
    let r_top = 0.84 * r;
    let segs = 40;
    let b = kit.mb(STONE);
    // Plinth.
    let splits: Vec<f32> = stations(-ps, ps, &[], 0.1);
    bevel_box(
        b,
        &Frame::at(Vec3::new(0.0, y_bot + ph * 0.5, 0.0)),
        &BoxSpec::new(Vec3::new(ps, ph * 0.5, ps), 0.012 * r / 0.3)
            .faces(ALL & !NY)
            .splits([&splits, &[0.0], &splits]),
    );
    // Attic base.
    let base_fn = |t: f32| -> f32 {
        let circ = |t: f32, c: f32, rad: f32| (rad * rad - (t - c).powi(2)).max(0.0).sqrt();
        if t <= 0.2 {
            1.2 + circ(t, 0.1, 0.1)
        } else if t <= 0.24 {
            1.14
        } else if t <= 0.36 {
            1.14 - 0.8 * circ(t, 0.3, 0.06)
        } else if t <= 0.39 {
            1.14
        } else if t <= 0.53 {
            1.08 + circ(t, 0.46, 0.07)
        } else if t <= 0.56 {
            1.04
        } else {
            1.0 + 0.04 * (1.0 - (t - 0.56) / 0.12).powi(2)
        }
    };
    let ts = [
        0.0, 0.01, 0.03, 0.06, 0.1, 0.14, 0.17, 0.19, 0.2, 0.2001, 0.24, 0.2401, 0.255, 0.28, 0.3,
        0.32, 0.345, 0.36, 0.3601, 0.39, 0.3901, 0.40, 0.42, 0.46, 0.5, 0.52, 0.53, 0.5301, 0.56,
        0.5601, 0.6, 0.64, 0.68,
    ];
    let mut prof = vec![Vec2::new(0.0, y0)];
    prof.extend(ts.iter().map(|&t| Vec2::new(r * base_fn(t), y0 + t * r)));
    prof.push(Vec2::new(0.0, y0 + base_h));
    lathe(b, &Frame::ID, &prof, segs, 40.0, LatheUv::Wrap(r), [1.0; 3]);
    // Capital: astragal, annulets, echinus.
    let yc = ys1;
    let mut cap = vec![Vec2::new(0.0, yc), Vec2::new(r_top, yc)];
    for k in 0..=6 {
        let a = -FRAC_PI_2 + PI * k as f32 / 6.0;
        cap.push(Vec2::new(
            r_top + 0.012 * r + 0.03 * r * a.cos(),
            yc + 0.04 * r + 0.035 * r * a.sin(),
        ));
    }
    let mut y = yc + 0.08 * r;
    let mut rad = r_top + 0.01 * r;
    cap.push(Vec2::new(rad, y));
    for _ in 0..3 {
        cap.push(Vec2::new(rad + 0.015 * r, y));
        y += 0.02 * r;
        rad += 0.015 * r;
        cap.push(Vec2::new(rad, y));
    }
    // Echinus: a quadratic Bezier flaring out at ~30° and turning up under the abacus.
    let y_ab = hgt * 0.5 - ab_h;
    let (re0, re1, he) = (rad, 1.1 * r, y_ab - y);
    let p0 = Vec2::new(re0, y);
    let p1 = Vec2::new(re1, y + 0.45 * he);
    let p2 = Vec2::new(re1 - 0.012 * r, y_ab);
    for k in 1..=10 {
        let t = k as f32 / 10.0;
        cap.push(p0 * (1.0 - t) * (1.0 - t) + p1 * 2.0 * t * (1.0 - t) + p2 * t * t);
    }
    cap.push(Vec2::new(0.0, y_ab));
    lathe(b, &Frame::ID, &cap, segs, 40.0, LatheUv::Wrap(r), [1.0; 3]);
    let abacus = 1.16 * r;
    let asplit = stations(-abacus, abacus, &[], 0.1);
    bevel_box(
        b,
        &Frame::at(Vec3::new(0.0, y_ab + ab_h * 0.5, 0.0)),
        &BoxSpec::new(Vec3::new(abacus, ab_h * 0.5, abacus), 0.012 * r / 0.3).splits([
            &asplit,
            &[-0.25 * ab_h, 0.0, 0.25 * ab_h],
            &asplit,
        ]),
    );
    // Fluted shaft with entasis.
    let nfl = 20;
    let m = 5;
    let mut ring_ys = stations(ys0, ys1, &[], (ys1 - ys0) / 40.0);
    for k in [0.04f32, 0.1, 0.18, 0.28, 0.4] {
        ring_ys.push(ys0 + k * r);
        ring_ys.push(ys1 - k * r);
    }
    ring_ys.sort_by(f32::total_cmp);
    ring_ys.dedup_by(|a, b| (*a - *b).abs() < 0.01 * r);
    let delta = TAU / nfl as f32;
    let cols = (m + 1) as u32;
    for fl in 0..nfl {
        let first = b.vertices.len() as u32;
        for &y in &ring_ys {
            let t = (y - ys0) / (ys1 - ys0);
            let rr = r * (1.0 - 0.16 * t.powf(1.8));
            let fade = smoothstep(ys0, ys0 + 0.35 * r, y) * smoothstep(ys1, ys1 - 0.3 * r, y);
            let chord = 2.0 * rr * (delta * 0.5).sin();
            let depth = 0.06 * rr * fade;
            for j in 0..=m {
                let fr = j as f32 / m as f32;
                let x = (fr - 0.5) * chord;
                let off = if depth > 1e-6 {
                    let rc = (chord * chord * 0.25 + depth * depth) / (2.0 * depth);
                    (rc * rc - x * x).max(0.0).sqrt() - (rc - depth)
                } else {
                    0.0
                };
                let phi = (fl as f32 + fr) * delta;
                let rho = rr - off.max(0.0);
                let pos = Vec3::new(rho * phi.cos(), y, -rho * phi.sin());
                let nrm = Vec3::new(phi.cos(), 0.0, -phi.sin());
                vert(b, pos, nrm, Vec2::new(phi * r, -y), [1.0; 3]);
            }
        }
        for i in 0..ring_ys.len() as u32 - 1 {
            for j in 0..m as u32 {
                let a = first + i * cols + j;
                let c = a + cols;
                let n = Vec3::from(b.vertices[a as usize].normal);
                quad_idx(b, [a, a + 1, c + 1, c], n);
            }
        }
    }
    // Weathering: shallow erosion everywhere and a few chipped-out pockets.
    let k = r / 0.3;
    let s = seed;
    let delta_fl = TAU / nfl as f32;
    let (abacus_lo, plinth_hi) = (y_ab, y0);
    deform(b, 0, |q| {
        let radial = Vec3::new(q.x, 0.0, q.z);
        let dir = -radial.normalize_or(Vec3::X);
        let wear = 0.003 * k * (0.5 + 0.5 * fbm(q, 9.0 / k, s, 3));
        let chip = 0.02 * k * smoothstep(0.32, 0.62, fbm(q, 3.5 / k, s ^ 0xC1, 3));
        // Broken arrises between the flutes.
        let phi = (-q.z).atan2(q.x);
        let fr = (phi / delta_fl).rem_euclid(1.0);
        let arris = smoothstep(0.14, 0.0, fr.min(1.0 - fr))
            * smoothstep(ys0 + 0.2 * r, ys0 + 0.5 * r, q.y)
            * smoothstep(ys1 - 0.1 * r, ys1 - 0.4 * r, q.y);
        let nick = 0.012 * k * arris * smoothstep(0.1, 0.45, fbm(q, 6.0 / k, s ^ 0xA7, 3));
        // Knocked corners on the plinth and abacus.
        let top = q.y > abacus_lo - 1e-4;
        let (corner, cy) = if top || q.y < plinth_hi + 1e-4 {
            let (hx, cy, lift) = if top {
                (abacus, abacus_lo + ab_h * 0.5, 1.0)
            } else {
                (ps, y_bot + ph * 0.5, smoothstep(y_bot, plinth_hi, q.y))
            };
            let e = q.x.abs().max(q.z.abs()) / hx;
            let e2 = q.x.abs().min(q.z.abs()) / hx;
            let c = smoothstep(0.88, 0.99, e)
                * smoothstep(0.5, 0.95, e2)
                * smoothstep(0.1, 0.5, fbm(q, 4.0 / k, s ^ 0xB3, 3))
                * lift;
            (c, cy)
        } else {
            (0.0, 0.0)
        };
        q + dir * (wear + chip + nick) - radial.normalize_or(Vec3::X) * (0.03 * k * corner)
            + Vec3::Y * (cy - q.y).signum() * 0.014 * k * corner
    });
    auto_smooth(b, 0, 0, 40.0);
    shade(b, 0, |q, n| {
        let mottle = 1.0 + 0.16 * fbm(q, 2.0 / k, s ^ 1, 4);
        let streak = smoothstep(
            0.0,
            0.5,
            fbm(
                Vec3::new(q.x * 6.0, q.y * 0.3, q.z * 6.0),
                1.5 / k,
                s ^ 2,
                3,
            ),
        );
        // Rain runs dark below every ledge and leaves the tops clean.
        let low = smoothstep(y_bot, y_bot + hgt * 0.3, q.y);
        let up = n.y.max(0.0);
        let moss = (1.0 - smoothstep(y_bot, y_bot + ph + 0.5 * r, q.y))
            * smoothstep(-0.1, 0.4, fbm(q, 4.0, s ^ 4, 2));
        let warm = 0.05 * fbm(q, 0.7 / k, s ^ 5, 2);
        let v = mottle * (1.0 - 0.22 * streak * (1.0 - up)) * (0.72 + 0.28 * low);
        mix3(
            [v * (1.0 + warm), v * 0.98, v * (0.95 - warm)],
            [0.55 * v, 0.64 * v, 0.42 * v],
            moss * 0.7,
        )
    });
    kit.finish(Ao {
        reach: 0.25 * k,
        strength: 0.8,
        ground: true,
    })
}

// ============================================================================================
// Roof
// ============================================================================================

const BATTEN_T: f32 = 0.025;
const RAFTER_D: f32 = 0.14;
const SHINGLE_H: f32 = 0.035;

/// Down-slope length from the apex to the shingle eave edge.
fn roof_slope_len(depth: f32, pitch: f32) -> f32 {
    (depth * 0.5 - SHINGLE_H * pitch.sin()) / pitch.cos()
}

/// A flat roof panel (see [`PropKind::ShinglePanel`]): rafters every 0.6 m under board
/// sheathing under wooden shingle courses laid from the eave up.
pub fn shingle_panel(size: Vec3, eave_at_plus_x: bool, seed: u64) -> Vec<PropPart> {
    let (l, t, w) = (size.x.max(0.5), size.y.max(0.06), size.z.max(0.5));
    let mut rng = Rng::new(seed ^ 0x5419);
    let mut kit = Kit::new();
    let dn = if eave_at_plus_x { Vec3::X } else { -Vec3::X };
    let nn = Vec3::Y;
    let hw = w * 0.5;
    let e = 0.18; // shingle exposure
    // `sl` runs down the slope from the ridge end (0) to the eave (l); `h` up from the bottom.
    let at = |z: f32, sl: f32, h: f32| Vec3::Z * z + dn * (sl - l * 0.5) + nn * (h - t * 0.5);
    let b = kit.mb(WOOD);
    let rd = (t - 0.05).max(0.03);
    let n_raft = ((w - 0.1) / 0.6).ceil().max(1.0) as usize + 1;
    for i in 0..n_raft {
        let z = -(hw - 0.05) + (w - 0.1) * i as f32 / (n_raft - 1) as f32;
        let f = Frame::xz(at(z, l * 0.5, rd * 0.5), dn, Vec3::Z);
        board(
            b,
            &f,
            Vec2::new(0.025, rd * 0.5),
            &stations(-l * 0.5, l * 0.5, &[], 0.5),
            0.004,
            0.004,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    // Sheathing boards across the rafters.
    let mut sl = 0.1;
    while sl < l {
        let width = (l - sl + 0.1).min(0.2);
        let f = Frame::xz(at(0.0, sl - 0.1 + width * 0.5, rd + 0.01), Vec3::Z, nn);
        board(
            b,
            &f,
            Vec2::new(0.01, width * 0.5 - 0.003),
            &stations(-hw, hw, &[], 0.6),
            0.002,
            0.0,
            true,
            mul(wood_tint(&mut rng), 0.9),
            rng_uv(&mut rng),
        );
        sl += 0.2;
    }
    // Shingle courses from the eave up.
    let base_h = rd + 0.02;
    let mut k = 0;
    loop {
        let butt = l - k as f32 * e;
        if butt < 0.02 {
            break;
        }
        let top = (butt - e - 0.06).max(0.0);
        let lift = if k == 0 { 0.008 } else { 0.006 };
        let mut z = -hw - rng.range(0.0, 0.12);
        while z < hw {
            let width = rng.range(0.08, 0.24);
            let gap = rng.range(0.003, 0.008);
            let z0 = z.max(-hw);
            let z1 = (z + width).min(hw);
            z += width + gap;
            if z1 - z0 < 0.03 {
                continue;
            }
            let bj = (butt + rng.range(-0.008, 0.006)).min(l);
            let curl = rng.range(0.0, 0.004);
            let skew = rng.range(-0.004, 0.004);
            let t_b = rng.range(0.009, 0.013);
            let t_t = 0.003;
            let tint = {
                let t = wood_tint(&mut rng);
                let grey = rng.range(0.0, 0.35);
                mix3(t, [t[1] * 0.92; 3], grey)
            };
            let uv = rng_uv(&mut rng);
            let f01 = |sl: f32| ((sl - top) / (bj - top)).clamp(0.0, 1.0);
            let hb = |sl: f32| base_h + (lift + curl) * f01(sl);
            let th = |sl: f32| t_t + (t_b - t_t) * f01(sl);
            let rows = [top, (bj - e).max(top + 0.01), bj - 0.006, bj];
            let pt = |z: f32, sl: f32, up: bool| {
                let round_over = if sl >= bj - 1e-5 { 0.0025 } else { 0.0 };
                let hh = if up {
                    hb(sl) + th(sl) - round_over
                } else {
                    hb(sl)
                };
                at(z + skew * f01(sl), sl, hh)
            };
            // Top surface strips, the last one rounding over the butt.
            for wr in rows.windows(2) {
                let n1 = if wr[1] >= bj - 1e-5 {
                    (nn + dn).normalize()
                } else {
                    nn
                };
                let q = [
                    (pt(z0, wr[0], true), nn),
                    (pt(z1, wr[0], true), nn),
                    (pt(z1, wr[1], true), n1),
                    (pt(z0, wr[1], true), n1),
                ];
                let idx = q.map(|(v, n)| vert(b, v, n, Vec2::new(v.dot(dn), v.z) + uv, tint));
                quad_idx(b, idx, nn);
            }
            // Butt end.
            let q = [
                pt(z0, bj, false),
                pt(z1, bj, false),
                pt(z1, bj, true),
                pt(z0, bj, true),
            ];
            let idx = q.map(|v| vert(b, v, dn, Vec2::new(v.z, v.y) + uv, mul(tint, 0.85)));
            quad_idx(b, idx, dn);
            // Sides.
            for (ze, sgn) in [(z0, -1.0f32), (z1, 1.0)] {
                let q = [
                    pt(ze, top, false),
                    pt(ze, bj, false),
                    pt(ze, bj, true),
                    pt(ze, top, true),
                ];
                let n = Vec3::Z * sgn;
                let idx = q.map(|v| vert(b, v, n, Vec2::new(v.dot(dn), v.y) + uv, mul(tint, 0.85)));
                quad_idx(b, idx, n);
            }
        }
        k += 1;
    }
    kit.center();
    kit.finish(Ao {
        reach: 0.25,
        strength: 0.85,
        ground: false,
    })
}

fn roof_size(width: f32, depth: f32, pitch: f32) -> Vec3 {
    let (w, d, p) = roof_params(width, depth, pitch);
    let sl = roof_slope_len(d, p);
    let tail = sl - 0.06 + 0.022;
    let bottom = -tail * p.sin() - 0.15 * p.cos();
    let top = 0.01 * p.sin() + (BATTEN_T + 0.019 + 0.024) * p.cos();
    Vec3::new(w, top - bottom, d)
}

fn roof_params(width: f32, depth: f32, pitch: f32) -> (f32, f32, f32) {
    (
        width.max(1.0),
        depth.max(1.0),
        pitch.clamp(15f32.to_radians(), 60f32.to_radians()),
    )
}

/// Gable roof `width` meters along the ridge (X) and `depth` meters across (Z) at `pitch`
/// radians: rafters on a ridge beam, battens, overlapping hand-split wooden shingles in
/// staggered courses, a board ridge cap, barge boards on the gables and fascia boards.
pub fn roof_tiles(width: f32, depth: f32, pitch: f32, seed: u64) -> Vec<PropPart> {
    let (w, d, p) = roof_params(width, depth, pitch);
    let mut rng = Rng::new(seed ^ 0x500F);
    let mut kit = Kit::new();
    let sl_max = roof_slope_len(d, p);
    let hw = w * 0.5;
    let e = 0.18; // shingle exposure
    for s in [1.0f32, -1.0] {
        let dn = Vec3::new(0.0, -p.sin(), s * p.cos());
        let nn = Vec3::new(0.0, p.cos(), s * p.sin());
        let at = |x: f32, sl: f32, h: f32| Vec3::X * x + dn * sl + nn * h;
        let b = kit.mb(WOOD);
        // Rafters.
        let n_raft = ((w - 0.16) / 0.6).ceil().max(1.0) as usize + 1;
        let sl_r = sl_max - 0.06;
        for i in 0..n_raft {
            let x = -(hw - 0.08) + (w - 0.16) * i as f32 / (n_raft - 1) as f32;
            let f = Frame::xz(at(x, (sl_r - 0.025) * 0.5, -RAFTER_D * 0.5), dn, Vec3::X);
            let xs = stations(-(sl_r + 0.025) * 0.5, (sl_r + 0.025) * 0.5, &[], 0.5);
            board(
                b,
                &f,
                Vec2::new(0.025, RAFTER_D * 0.5),
                &xs,
                0.004,
                0.004,
                true,
                wood_tint(&mut rng),
                rng_uv(&mut rng),
            );
        }
        // Battens.
        let mut sl = 0.12;
        while sl < sl_max - 0.05 {
            let f = Frame::xz(at(0.0, sl, BATTEN_T * 0.5), Vec3::X, nn);
            board(
                b,
                &f,
                Vec2::new(BATTEN_T * 0.5, 0.025),
                &[-(hw - 0.03), hw - 0.03],
                0.0,
                0.0,
                true,
                mul(wood_tint(&mut rng), 0.9),
                rng_uv(&mut rng),
            );
            sl += e;
        }
        // Starter strip under the first course.
        let f = Frame::xz(at(0.0, sl_max - 0.12, BATTEN_T + 0.004), Vec3::X, nn);
        board(
            b,
            &f,
            Vec2::new(0.004, 0.11),
            &[-hw + 0.01, hw - 0.01],
            0.0,
            0.0,
            true,
            wood_tint(&mut rng),
            Vec2::ZERO,
        );
        // Fascia across the rafter tails, barge boards on the gables.
        let f = Frame::xz(at(0.0, sl_r + 0.011, -0.06), Vec3::X, dn);
        board(
            b,
            &f,
            Vec2::new(0.011, 0.09),
            &stations(-hw + 0.012, hw - 0.012, &[], 0.6),
            0.003,
            0.0,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
        for sx in [-1.0f32, 1.0] {
            let f = Frame::xz(
                at(sx * (hw - 0.0245), sl_max * 0.5 - 0.01, -0.05),
                dn,
                Vec3::X * sx,
            );
            board(
                b,
                &f,
                Vec2::new(0.0125, 0.1),
                &stations(-sl_max * 0.5 - 0.01, sl_max * 0.5 - 0.04, &[], 0.6),
                0.004,
                0.0,
                true,
                wood_tint(&mut rng),
                rng_uv(&mut rng),
            );
        }
        // Shingle courses from the eave up.
        let base_h = BATTEN_T;
        let mut k = 0;
        loop {
            let butt = sl_max - k as f32 * e;
            if butt < 0.02 {
                break;
            }
            let top = (butt - e - 0.06).max(-0.01);
            let lift = if k == 0 { 0.008 } else { 0.006 };
            let mut x = -hw - rng.range(0.0, 0.12);
            while x < hw {
                let width = rng.range(0.08, 0.24);
                let gap = rng.range(0.003, 0.008);
                let x0 = x.max(-hw);
                let x1 = (x + width).min(hw);
                x += width + gap;
                if x1 - x0 < 0.03 {
                    continue;
                }
                let bj = butt + rng.range(-0.008, 0.006);
                let curl = rng.range(0.0, 0.004);
                let skew = rng.range(-0.004, 0.004);
                let t_b = rng.range(0.009, 0.013);
                let t_t = 0.003;
                let tint = {
                    let t = wood_tint(&mut rng);
                    let grey = rng.range(0.0, 0.35);
                    mix3(t, [t[1] * 0.92; 3], grey)
                };
                let uv = rng_uv(&mut rng);
                let hb =
                    |sl: f32| base_h + (lift + curl) * ((sl - top) / (bj - top)).clamp(0.0, 1.0);
                let th = |sl: f32| t_t + (t_b - t_t) * ((sl - top) / (bj - top)).clamp(0.0, 1.0);
                let bev = 0.006;
                let rows = [top, (bj - e).max(top + 0.01), bj - bev, bj];
                let xsk = |x: f32, sl: f32| x + skew * (sl - top) / (bj - top);
                let pt = |x: f32, sl: f32, up: bool| {
                    let round_over = if sl >= bj - 1e-5 { 0.0025 } else { 0.0 };
                    let hh = if up {
                        hb(sl) + th(sl) - round_over
                    } else {
                        hb(sl)
                    };
                    at(xsk(x, sl), sl, hh)
                };
                // Top surface strips, the last one rounding over the butt.
                for wr in rows.windows(2) {
                    let n0 = nn;
                    let n1 = if wr[1] >= bj - 1e-5 {
                        (nn + dn).normalize()
                    } else {
                        nn
                    };
                    let q = [
                        (pt(x0, wr[0], true), n0),
                        (pt(x1, wr[0], true), n0),
                        (pt(x1, wr[1], true), n1),
                        (pt(x0, wr[1], true), n1),
                    ];
                    let idx = q.map(|(v, n)| vert(b, v, n, Vec2::new(v.dot(dn), v.x) + uv, tint));
                    quad_idx(b, idx, nn);
                }
                // Butt end.
                let q = [
                    pt(x0, bj, false),
                    pt(x1, bj, false),
                    pt(x1, bj, true),
                    pt(x0, bj, true),
                ];
                let idx = q.map(|v| vert(b, v, dn, Vec2::new(v.x, v.y) + uv, mul(tint, 0.85)));
                quad_idx(b, idx, dn);
                // Sides.
                for (xe, sgn) in [(x0, -1.0f32), (x1, 1.0)] {
                    let q = [
                        pt(xe, top, false),
                        pt(xe, bj, false),
                        pt(xe, bj, true),
                        pt(xe, top, true),
                    ];
                    let n = Vec3::X * sgn;
                    let idx = q.map(|v| vert(b, v, n, Vec2::new(v.z, v.y) + uv, mul(tint, 0.85)));
                    quad_idx(b, idx, n);
                }
            }
            k += 1;
        }
        // Ridge cap board on this side.
        let ridge_h = base_h + 0.006 + 0.013 + 0.0;
        let f = Frame::xz(at(0.0, 0.065 - s * 0.0, ridge_h + 0.012), Vec3::X, nn);
        board(
            b,
            &f,
            Vec2::new(0.012, 0.075),
            &stations(-hw, hw, &[], 0.5),
            0.004,
            0.0,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    // Ridge beam under the apex.
    let b = kit.mb(WOOD);
    let f = Frame::xz(Vec3::new(0.0, -0.03 - 0.11, 0.0), Vec3::X, Vec3::Z);
    board(
        b,
        &f,
        Vec2::new(0.025, 0.11),
        &stations(-hw + 0.02, hw - 0.02, &[], 0.6),
        0.004,
        0.004,
        true,
        wood_tint(&mut rng),
        rng_uv(&mut rng),
    );
    kit.center();
    kit.finish(Ao {
        reach: 0.25,
        strength: 0.85,
        ground: false,
    })
}

// ============================================================================================
// Scaffold and fence
// ============================================================================================

const SCAFFOLD_SIZE: Vec3 = Vec3::new(1.97, 2.0, 1.37);

/// Scaffold clamp (right-angle or swivel coupler) between tube axis points `a` and `b`.
fn coupler(
    b: &mut MeshBuilder,
    a: Vec3,
    axis_a: Vec3,
    c: Vec3,
    axis_c: Vec3,
    r: f32,
    tint: [f32; 3],
) {
    let link = c - a;
    let dist = link.length().max(1e-4);
    let dir = link / dist;
    for (p, ax) in [(a, axis_a), (c, axis_c)] {
        let prof = [
            Vec2::new(r + 0.0005, -0.021),
            Vec2::new(r + 0.0055, -0.017),
            Vec2::new(r + 0.0055, 0.017),
            Vec2::new(r + 0.0005, 0.021),
        ];
        lathe(
            b,
            &Frame::up_along(p, ax),
            &prof,
            8,
            50.0,
            LatheUv::Wrap(r),
            tint,
        );
        let side = ax.cross(dir).normalize_or(ax.any_orthonormal_vector());
        hex_head(b, p + side * (r + 0.004), side, 0.01, 0.012, mul(tint, 0.9));
    }
    let f = Frame::along((a + c) * 0.5, dir, axis_a);
    bevel_box(
        b,
        &f,
        &BoxSpec::new(
            Vec3::new((dist * 0.5 - r * 0.45).max(0.004), 0.017, 0.017),
            0.0,
        )
        .faces(PY | NY | PZ | NZ)
        .tint(tint),
    );
}

/// Straight scaffold tube from `a` to `b` with open (hollow) ends, split every 0.4 m.
fn scaffold_tube(b: &mut MeshBuilder, a: Vec3, c: Vec3, r: f32, tint: [f32; 3]) {
    let n = ((c - a).length() / 0.4).ceil().max(1.0) as usize;
    let path: Vec<Vec3> = (0..=n).map(|k| a.lerp(c, k as f32 / n as f32)).collect();
    let sides = 10;
    tube(b, &path, r, sides, false, tint);
    let t = (c - a).normalize_or(Vec3::X);
    let nrm = t.any_orthonormal_vector();
    let bin = t.cross(nrm);
    let wall = 0.0035;
    let depth = 0.035;
    let inside = mul(tint, 0.35);
    for (p, out) in [(a, -t), (c, t)] {
        let ring = |rad: f32, back: f32, k: u32| {
            let ang = k as f32 / sides as f32 * TAU;
            p - out * back + (nrm * ang.cos() + bin * ang.sin()) * rad
        };
        // Lip of the cut end.
        let base = b.vertices.len() as u32;
        for k in 0..sides as u32 {
            let (o, i) = (ring(r, 0.0, k), ring(r - wall, 0.0, k));
            vert(b, o, out, Vec2::new(k as f32 * r * 0.6, 0.0), tint);
            vert(
                b,
                i,
                out,
                Vec2::new(k as f32 * r * 0.6, wall),
                mul(tint, 0.8),
            );
        }
        let s = sides as u32;
        for k in 0..s {
            let k1 = (k + 1) % s;
            quad_idx(
                b,
                [
                    base + 2 * k,
                    base + 2 * k1,
                    base + 2 * k1 + 1,
                    base + 2 * k + 1,
                ],
                out,
            );
        }
        // Dark bore and the shadowed inside further down.
        let base = b.vertices.len() as u32;
        for k in 0..=s {
            let ang = k as f32 / sides as f32 * TAU;
            let inward = -(nrm * ang.cos() + bin * ang.sin());
            let uv = Vec2::new(ang * r, 0.0);
            vert(b, ring(r - wall, 0.0, k), inward, uv, mul(tint, 0.6));
            vert(
                b,
                ring(r - wall, depth, k),
                inward,
                uv + Vec2::Y * depth,
                inside,
            );
        }
        for k in 0..s {
            let q = [
                base + 2 * k,
                base + 2 * k + 2,
                base + 2 * k + 3,
                base + 2 * k + 1,
            ];
            let inward = Vec3::from(b.vertices[q[0] as usize].normal)
                + Vec3::from(b.vertices[q[1] as usize].normal);
            quad_idx(b, q, inward);
        }
        let center = vert(b, p - out * depth, out, Vec2::ZERO, mul(inside, 0.6));
        let base = b.vertices.len() as u32;
        for k in 0..s {
            vert(
                b,
                ring(r - wall, depth, k),
                out,
                Vec2::ZERO,
                mul(inside, 0.6),
            );
        }
        for k in 0..s {
            tri_idx(b, [center, base + k, base + (k + 1) % s], out);
        }
    }
}

/// Mobile tube-and-coupler scaffold tower about 2 m tall (1.8 × 1.2 m between standards):
/// base plates with screw jacks, standards, ledgers, transoms, diagonal braces, couplers at
/// every joint, a four-board platform with banded ends and toe boards.
pub fn scaffold_tower(seed: u64) -> Vec<PropPart> {
    let mut rng = Rng::new(seed ^ 0x5CAF_F01D);
    let mut kit = Kit::new();
    let r = 0.02415;
    let (sx, sz) = (0.85f32, 0.55f32);
    let off = 2.0 * r + 0.004;
    let levels = [0.3f32, 1.0, 1.5, 1.95];
    let top = 2.0;
    let galv = [0.92, 0.93, 0.95];
    let dark = [0.6, 0.6, 0.6];
    let m = kit.mb(METAL);
    for cx in [-1.0f32, 1.0] {
        for cz in [-1.0f32, 1.0] {
            let (x, z) = (cx * sx, cz * sz);
            bevel_box(
                m,
                &Frame::at(Vec3::new(x, 0.003, z)),
                &BoxSpec::new(Vec3::new(0.075, 0.003, 0.075), 0.002)
                    .faces(ALL & !NY)
                    .tint(galv),
            );
            for (dx, dz) in [
                (-0.05f32, -0.05f32),
                (0.05, -0.05),
                (0.05, 0.05),
                (-0.05, 0.05),
            ] {
                nail(m, Vec3::new(x + dx, 0.006, z + dz), Vec3::Y, 0.006, dark);
            }
            let v0 = m.vertices.len();
            tube(
                m,
                &[
                    Vec3::new(x, 0.006, z),
                    Vec3::new(x, 0.13, z),
                    Vec3::new(x, 0.2, z),
                ],
                0.017,
                10,
                false,
                dark,
            );
            shade(m, v0, |q, _| [0.85 + 0.15 * (q.y * 400.0).sin().abs(); 3]);
            hex_head(
                m,
                Vec3::new(x, 0.1, z),
                Vec3::Y,
                0.034,
                0.03,
                mul(galv, 0.85),
            );
            for k in [-1.0f32, 1.0] {
                tube(
                    m,
                    &[Vec3::new(x, 0.115, z), Vec3::new(x + k * 0.07, 0.115, z)],
                    0.006,
                    6,
                    true,
                    mul(galv, 0.85),
                );
            }
            scaffold_tube(m, Vec3::new(x, 0.14, z), Vec3::new(x, top, z), r, galv);
        }
    }
    for &y in &levels {
        for cz in [-1.0f32, 1.0] {
            let z = cz * (sz + off);
            scaffold_tube(m, Vec3::new(-0.95, y, z), Vec3::new(0.95, y, z), r, galv);
            for cx in [-1.0f32, 1.0] {
                coupler(
                    m,
                    Vec3::new(cx * sx, y, cz * sz),
                    Vec3::Y,
                    Vec3::new(cx * sx, y, z),
                    Vec3::X,
                    r,
                    galv,
                );
            }
        }
        let yt = y + off;
        if yt > top - 0.02 {
            continue;
        }
        for cx in [-1.0f32, 1.0] {
            let x = cx * (sx + off);
            scaffold_tube(m, Vec3::new(x, yt, -0.65), Vec3::new(x, yt, 0.65), r, galv);
            for cz in [-1.0f32, 1.0] {
                coupler(
                    m,
                    Vec3::new(cx * sx, yt, cz * sz),
                    Vec3::Y,
                    Vec3::new(x, yt, cz * sz),
                    Vec3::Z,
                    r,
                    galv,
                );
            }
        }
    }
    // Diagonal braces on all four sides of the bottom lift.
    for cz in [-1.0f32, 1.0] {
        let z = cz * (sz + 2.0 * off);
        let (a, c) = (Vec3::new(-sx, 0.4, z), Vec3::new(sx, 0.92, z));
        let d = (c - a).normalize();
        scaffold_tube(m, a - d * 0.07, c + d * 0.07, r, galv);
        coupler(m, Vec3::new(-sx, 0.4, cz * sz), Vec3::Y, a, d, r, galv);
        coupler(m, Vec3::new(sx, 0.92, cz * sz), Vec3::Y, c, d, r, galv);
    }
    for cx in [-1.0f32, 1.0] {
        let x = cx * (sx + 2.0 * off);
        let (a, c) = (Vec3::new(x, 0.45, -sz), Vec3::new(x, 0.9, sz));
        let d = (c - a).normalize();
        scaffold_tube(m, a - d * 0.06, c + d * 0.06, r, galv);
        coupler(m, Vec3::new(cx * sx, 0.45, -sz), Vec3::Y, a, d, r, galv);
        coupler(m, Vec3::new(cx * sx, 0.9, sz), Vec3::Y, c, d, r, galv);
    }
    // Platform boards on the transoms of the 1.0 m lift, with steel end bands.
    let y_board = 1.0 + off + r + 0.019;
    let up = Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y);
    for i in 0..4 {
        let z = -0.36 + 0.24 * i as f32 + rng.range(-0.004, 0.004);
        let f = Frame::new(Vec3::new(rng.range(-0.01, 0.01), y_board, z), up);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(0.019, 0.1125),
            &stations(-0.95, 0.95, &[-0.9, 0.9], 0.3),
            0.004,
            0.003,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
        for ex in [-1.0f32, 1.0] {
            bevel_box(
                kit.mb(METAL),
                &Frame::at(Vec3::new(f.pos.x + ex * 0.935, y_board, z)),
                &BoxSpec::new(Vec3::new(0.016, 0.0205, 0.1145), 0.002).tint(galv),
            );
        }
    }
    // Toe boards around the platform.
    let y_toe = y_board + 0.019 + 0.075;
    for cz in [-1.0f32, 1.0] {
        let f = Frame::xz(Vec3::new(0.0, y_toe, cz * 0.505), Vec3::X, Vec3::Z * cz);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(0.0125, 0.075),
            &stations(-0.82, 0.82, &[], 0.3),
            0.003,
            0.003,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    for cx in [-1.0f32, 1.0] {
        let f = Frame::xz(Vec3::new(cx * 0.81, y_toe, 0.0), Vec3::Z, Vec3::X * cx);
        board(
            kit.mb(WOOD),
            &f,
            Vec2::new(0.0125, 0.075),
            &stations(-0.49, 0.49, &[], 0.3),
            0.003,
            0.003,
            true,
            wood_tint(&mut rng),
            rng_uv(&mut rng),
        );
    }
    kit.center();
    kit.finish(Ao {
        reach: 0.2,
        strength: 0.8,
        ground: true,
    })
}

/// Galvanized chain-link fence `length` meters long (X), 2 m tall: terminal posts with dome
/// caps, line posts every ≤ 3 m with loop caps, a top rail, bottom tension wire, tension bars
/// and bands, wire ties, and 50 mm diamond mesh drawn as thin wires.
pub fn chain_link_fence(length: f32, seed: u64) -> Vec<PropPart> {
    let len = length.max(1.0);
    let mut rng = Rng::new(seed ^ 0xFE4CE);
    let mut kit = Kit::new();
    let rt = 0.0365;
    let rl = 0.03;
    let post_h = 1.95;
    let galv = [0.9, 0.91, 0.93];
    let bays = (len / 3.0).ceil().max(1.0) as usize;
    let (x0, x1) = (-len * 0.5 + rt + 0.004, len * 0.5 - rt - 0.004);
    let zf = rl + 0.004;
    let y_rail = post_h - 0.035;
    let m = kit.mb(METAL);
    for i in 0..=bays {
        let x = x0 + (x1 - x0) * i as f32 / bays as f32;
        let terminal = i == 0 || i == bays;
        let rp = if terminal { rt } else { rl };
        let top: f32 = if terminal { post_h } else { y_rail - 0.01 };
        let n = (top / 0.4).ceil() as usize;
        let path: Vec<Vec3> = (0..=n)
            .map(|k| Vec3::new(x, top * k as f32 / n as f32, 0.0))
            .collect();
        tube(m, &path, rp, 12, false, galv);
        if terminal {
            lathe(
                m,
                &Frame::at(Vec3::new(x, top, 0.0)),
                &[
                    Vec2::new(rp + 0.003, -0.035),
                    Vec2::new(rp + 0.003, 0.0),
                    Vec2::new(rp * 0.85, 0.016),
                    Vec2::new(rp * 0.45, 0.027),
                    Vec2::new(0.0, 0.03),
                ],
                16,
                50.0,
                LatheUv::Wrap(rp),
                galv,
            );
            let inward = if i == 0 { 1.0 } else { -1.0 };
            // Tension bar and bands holding the mesh.
            let xb = x + inward * (rp + 0.008);
            bevel_box(
                m,
                &Frame::at(Vec3::new(xb, 0.96, zf)),
                &BoxSpec::new(Vec3::new(0.009, 0.9, 0.0025), 0.0012).tint(galv),
            );
            for y in [0.15f32, 0.6, 1.1, 1.6] {
                lathe(
                    m,
                    &Frame::at(Vec3::new(x, y, 0.0)),
                    &[
                        Vec2::new(rp + 0.0005, -0.012),
                        Vec2::new(rp + 0.003, -0.01),
                        Vec2::new(rp + 0.003, 0.01),
                        Vec2::new(rp + 0.0005, 0.012),
                    ],
                    12,
                    50.0,
                    LatheUv::Wrap(rp),
                    galv,
                );
                bevel_box(
                    m,
                    &Frame::at(Vec3::new(x + inward * (rp + 0.004), y, zf * 0.6)),
                    &BoxSpec::new(Vec3::new(0.008, 0.011, 0.012), 0.002).tint(galv),
                );
                hex_head(
                    m,
                    Vec3::new(x + inward * (rp + 0.004), y, zf * 0.6 + 0.012),
                    Vec3::Z,
                    0.006,
                    0.006,
                    galv,
                );
            }
            // Rail end cup.
            lathe(
                m,
                &Frame::up_along(Vec3::new(x + inward * rp, y_rail, 0.0), Vec3::X * inward),
                &[
                    Vec2::new(0.026, -0.004),
                    Vec2::new(0.026, 0.03),
                    Vec2::new(0.024, 0.034),
                    Vec2::new(0.0, 0.034),
                ],
                12,
                50.0,
                LatheUv::Wrap(0.025),
                galv,
            );
        } else {
            // Loop cap carrying the top rail.
            lathe(
                m,
                &Frame::at(Vec3::new(x, top, 0.0)),
                &[
                    Vec2::new(rp + 0.003, -0.03),
                    Vec2::new(rp + 0.003, 0.004),
                    Vec2::new(rp * 0.6, 0.01),
                    Vec2::new(0.0, 0.012),
                ],
                12,
                50.0,
                LatheUv::Wrap(rp),
                galv,
            );
            let ring = arc_path(
                Vec3::new(x, y_rail, 0.0),
                Vec3::Y,
                Vec3::Z,
                0.027,
                0.0,
                TAU,
                16,
            );
            tube(m, &ring, 0.005, 6, false, galv);
            // Wire ties.
            let mut y = 0.3;
            while y < y_rail - 0.2 {
                let a0 = rng.range(-0.3, 0.3);
                let ring = arc_path(
                    Vec3::new(x, y, 0.0),
                    Vec3::X,
                    Vec3::Z,
                    rp + 0.002,
                    a0,
                    a0 + 5.6,
                    14,
                );
                tube(m, &ring, 0.0018, 4, true, [0.8; 3]);
                y += 0.38;
            }
        }
    }
    // Top rail and bottom tension wire.
    let rail_a = x0 + rt + 0.02;
    let rail_b = x1 - rt - 0.02;
    let n = ((rail_b - rail_a) / 0.5).ceil() as usize;
    let rail: Vec<Vec3> = (0..=n)
        .map(|k| Vec3::new(lerp(rail_a, rail_b, k as f32 / n as f32), y_rail, 0.0))
        .collect();
    tube(m, &rail, 0.021, 12, true, galv);
    let wire: Vec<Vec3> = rail.iter().map(|q| Vec3::new(q.x, 0.055, zf)).collect();
    tube(m, &wire, 0.002, 4, false, [0.8; 3]);
    // Diamond mesh: two families of 45° wires meeting at knuckled selvages.
    let pitch = 0.0707;
    let (xa, xb) = (x0 + rt + 0.01, x1 - rt - 0.01);
    let yb = 0.06;
    let yt = yb + ((2.0 * (y_rail - 0.04 - yb) / pitch).floor() * pitch * 0.5).max(pitch);
    let wire_t = mul(galv, 0.92);
    for fam in [1.0f32, -1.0] {
        let zw = zf + fam * 0.0015;
        let (c_min, c_max) = if fam > 0.0 {
            (xa - yt, xb - yb)
        } else {
            (xa + yb, xb + yt)
        };
        let c0 = if fam > 0.0 { xa - yb } else { xa + yb };
        let k0 = ((c_min - c0) / pitch).floor() as i32;
        let k1 = ((c_max - c0) / pitch).ceil() as i32;
        for k in k0..=k1 {
            let c = c0 + k as f32 * pitch;
            let (ylo, yhi) = if fam > 0.0 {
                (yb.max(xa - c), yt.min(xb - c))
            } else {
                (yb.max(c - xb), yt.min(c - xa))
            };
            if yhi - ylo < 0.005 {
                continue;
            }
            let xat = |y: f32| if fam > 0.0 { c + y } else { c - y };
            tube(
                m,
                &[Vec3::new(xat(ylo), ylo, zw), Vec3::new(xat(yhi), yhi, zw)],
                0.0018,
                3,
                false,
                wire_t,
            );
        }
    }
    let s = seed;
    shade(m, 0, |q, _| {
        let k = 1.0 + 0.06 * fbm(q, 2.0, s, 3);
        let dirt = 0.85 + 0.15 * smoothstep(0.0, 0.3, q.y);
        [k * dirt; 3]
    });
    kit.center();
    kit.finish(Ao {
        reach: 0.15,
        strength: 0.7,
        ground: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_build_clean_and_fit() {
        for (n, kind) in PropKind::catalog().into_iter().enumerate() {
            let parts = kind.build(7 + n as u64);
            assert!(!parts.is_empty(), "{kind:?} built nothing");
            let (lo, hi) = prop_bounds(&parts);
            let c = (lo + hi) * 0.5;
            let mut vol = 0.0f64;
            let (mut bad, mut total) = (0, 0);
            for p in &parts {
                for v in &p.mesh.vertices {
                    let mut all = v
                        .position
                        .iter()
                        .chain(&v.normal)
                        .chain(&v.uv)
                        .chain(&v.color);
                    assert!(all.all(|x| x.is_finite()), "{kind:?}: NaN in vertex");
                    assert!(
                        v.color[..3].iter().all(|&x| (0.0..=1.2).contains(&x)),
                        "{kind:?}: color {:?}",
                        v.color
                    );
                }
                for t in p.mesh.indices.as_chunks::<3>().0 {
                    let q = [t[0], t[1], t[2]]
                        .map(|i| Vec3::from(p.mesh.vertices[i as usize].position) - c);
                    vol += q[0].dot(q[1].cross(q[2])) as f64 / 6.0;
                    let g = (q[1] - q[0]).cross(q[2] - q[0]);
                    let n: Vec3 = t
                        .iter()
                        .map(|&i| Vec3::from(p.mesh.vertices[i as usize].normal))
                        .sum();
                    if g.length_squared() > 1e-14 {
                        total += 1;
                        if g.dot(n) < 0.0 {
                            bad += 1;
                        }
                    }
                }
            }
            assert!(
                vol > 0.0,
                "{kind:?}: signed volume {vol} (faces point inward)"
            );
            assert!(
                bad * 100 <= total,
                "{kind:?}: {bad} of {total} triangles disagree with their normals"
            );
            let size = kind.size();
            let tol = Vec3::splat(0.02);
            assert!(
                lo.cmpge(-size * 0.5 - tol).all() && hi.cmple(size * 0.5 + tol).all(),
                "{kind:?}: bounds {lo} .. {hi} exceed size {size}"
            );
            assert!(
                (hi - lo).cmpge(size - tol * 2.0).all(),
                "{kind:?}: extent {} does not fill size {size}",
                hi - lo
            );
            let tris = prop_triangles(&parts);
            let budget = match kind {
                PropKind::ScaffoldTower => 15_500,
                PropKind::StoneColumn { .. }
                | PropKind::RoofTiles { .. }
                | PropKind::ShinglePanel { .. }
                | PropKind::SandbagWall { .. } => 30_000,
                _ => 6_500,
            };
            println!("{kind:?}: {tris} triangles, extent {}", hi - lo);
            assert!(
                tris <= budget,
                "{kind:?}: {tris} triangles over budget {budget}"
            );
        }
    }

    #[test]
    fn timber_fills_its_box_on_any_axis() {
        for size in [
            Vec3::new(0.2, 2.5, 0.2),
            Vec3::new(0.15, 0.3, 4.0),
            Vec3::new(3.0, 0.1, 0.25),
        ] {
            let (lo, hi) = prop_bounds(&timber(size, 3));
            assert!(
                (hi - lo - size).abs().max_element() < 0.005,
                "{size}: {}",
                hi - lo
            );
        }
    }

    #[test]
    fn deterministic_per_seed() {
        let a = wooden_crate(Vec3::splat(0.8), 5);
        let b = wooden_crate(Vec3::splat(0.8), 5);
        assert_eq!(a[0].mesh.vertices, b[0].mesh.vertices);
    }

    #[test]
    fn triangulate_concave() {
        let l = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        let t = triangulate(&l);
        assert_eq!(t.len(), 4);
        let area: f32 = t
            .iter()
            .map(|t| (l[t[1]] - l[t[0]]).perp_dot(l[t[2]] - l[t[0]]) * 0.5)
            .sum();
        assert!((area - 3.0).abs() < 1e-5);
    }
}
