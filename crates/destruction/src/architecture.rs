//! Fortress architecture: procedural medieval masonry for the siege range.
//!
//! Every piece is built from individually cut stones laid on the same grid as the stone
//! texture ([`BLOCK_W`] × [`COURSE_H`], courses from y = 0, odd courses offset by half a block,
//! UV in meters with v = −y on walls), so painted joints and modeled joints line up. Geometry
//! carries what a texture cannot: silhouettes (battered plinths, corbel tables, merlons,
//! roofs), chamfered stone arrises with recessed V joints, worn and chipped corners, missing
//! and proud stones, deep reveals around arrow slits and gates. Vertex colors carry per-stone
//! tint, weathering (ground grime, rain streaks, lichen) and ambient occlusion baked by ray
//! casting against the piece itself and the ground, so the meshes read well even with plain
//! materials.
//!
//! Each builder returns one [`ArchPart`] per [`DestructibleMaterial`] (stone, wood, metal,
//! rock, dirt), with the piece's base centered at the local origin, y up. Sizes are meters.

use std::f32::consts::{PI, TAU};

use engine_core::math::noise::{fbm3, perlin3};
use engine_core::math::{Quat, Rng, Vec2, Vec3, hash01};
use engine_render::Vertex;

use crate::material::DestructibleMaterial;
use crate::meshing::{MeshBuilder, box_uv};

/// Length of one cut stone along a course, meters (the stone texture's block pitch).
pub const BLOCK_W: f32 = 0.9;
/// Height of one masonry course, meters (the stone texture's course pitch).
pub const COURSE_H: f32 = 0.45;
/// Depth of the recessed mortar joints and width of the chamfer on stone arrises, meters.
const JOINT: f32 = 0.024;

// ---------------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------------

/// The share of an architecture piece drawn with one material. Spawn one mesh per part with
/// the material's textured look; vertex colors (linear RGB, multiplied with the material)
/// carry per-stone tint, weathering and baked ambient occlusion.
#[derive(Debug, Clone)]
pub struct ArchPart {
    pub material: DestructibleMaterial,
    pub mesh: MeshBuilder,
}

/// How a tower ends at the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TowerTop {
    /// Projecting parapet on stepped corbels with machicolation slots, merlons and crenels.
    #[default]
    Crenellated,
    /// Conical (round) or pyramidal (square) roof of slate on a corbelled cornice.
    SlateRoof,
    /// Same roof shape in weathered wood shingles.
    ShingleRoof,
    /// Crenellated parapet with a covered timber gallery (hoarding) built out around it.
    Hoarding,
}

/// Total triangles over all parts.
pub fn triangle_count(parts: &[ArchPart]) -> usize {
    parts.iter().map(|p| p.mesh.indices.len() / 3).sum()
}

/// A straight curtain wall: battered plinth with a weathering string course, ashlar faces,
/// a corbel table carrying a projecting machicolated parapet with merlons, crenels and arrow
/// slits, a paved wall walk and (for walls 16 m and longer) a stone stair against the inner
/// face. `length` runs along X centered at the origin, the outer face looks toward −Z,
/// `height` is the top of the merlons, `thickness` the wall body above the plinth (3 m is
/// typical; the plinth and parapet project beyond it). About 900 triangles per meter.
pub fn curtain_wall(length: f32, height: f32, thickness: f32, seed: u64) -> Vec<ArchPart> {
    let length = length.max(4.0);
    let height = height.max(4.0);
    let thickness = thickness.clamp(1.2, 6.0);
    let mut k = Kit::new(seed);
    let fr = Frame::new(Vec3::new(0.0, 0.0, -thickness * 0.5), Vec3::X);
    let spec = WallSpec::new(length, height, thickness);
    k.curtain(&fr, &spec, true);
    k.finish()
}

/// A round tower with a crenellated, machicolated top: see [`round_tower_with`].
pub fn round_tower(radius: f32, height: f32, seed: u64) -> Vec<ArchPart> {
    round_tower_with(radius, height, TowerTop::Crenellated, seed)
}

/// A round tower centered on the origin: battered base with a roll string course, ashlar
/// shaft with arrow slits (and windows under a roof), an impact scar or two, and the chosen
/// `top`. `radius` is the shaft above the base (the foot stands out about 1 m more).
/// `height` is the top of the merlons; under a roof, the top of the corbelled cornice (the
/// cone rises about 2.3 × its radius above); with a hoarding, the gallery's eaves are about
/// there (its roof rises about 1.5 × its radius above).
pub fn round_tower_with(radius: f32, height: f32, top: TowerTop, seed: u64) -> Vec<ArchPart> {
    let mut k = Kit::new(seed);
    k.round_tower(
        Vec3::ZERO,
        radius.clamp(2.0, 12.0),
        height.max(6.0),
        top,
        1.0,
    );
    k.finish()
}

/// A square tower centered on the origin, `size` wide above its battered base, with arrow
/// slits and a machicolated crenellated parapet whose top (merlons) is at `height`.
pub fn square_tower(size: f32, height: f32, seed: u64) -> Vec<ArchPart> {
    let mut k = Kit::new(seed);
    k.square_tower(Vec3::ZERO, size.clamp(4.0, 20.0), height.max(6.0));
    k.finish()
}

/// A straight stone stair rising toward +X to a landing at `height` (its top step ends at
/// x = 0), `width` deep along +Z from z = 0, with a coursed support wall under its open
/// side. Its back is meant to stand against a wall face at z = 0 looking toward +Z (the
/// stair of [`curtain_wall`] is the same piece).
pub fn stone_stairs(height: f32, width: f32, seed: u64) -> Vec<ArchPart> {
    let mut k = Kit::new(seed);
    let fr = Frame::new(Vec3::ZERO, Vec3::X);
    k.stair_flight(
        &fr,
        0.0,
        height.clamp(0.45, 20.0),
        0.0,
        width.clamp(0.6, 4.0),
    );
    k.finish()
}

/// A banner pole with a cross bar and a swallow-tailed heraldic banner (red and gold, or
/// blue and gold), its foot at the origin, `height` tall. The cloth is the plaster part
/// (tinted by vertex colors).
pub fn banner_pole(height: f32, red: bool, seed: u64) -> Vec<ArchPart> {
    let mut k = Kit::new(seed);
    k.banner(Vec3::ZERO, height.max(3.5), red);
    k.finish()
}

/// A breached stretch of curtain wall, 3 m thick, in the same frame as [`curtain_wall`]
/// (`length` along X centered on the origin, outer face toward −Z). The plinth and lowest
/// courses stand intact; above them the facing breaks away course by course along the stone
/// joints down to a breach near the middle, exposing the rubble core between the two ashlar
/// skins, with loosened and broken stones on the break. Spoil heaps of earth, rubble and
/// fallen blocks lie against both faces. `height` is the top of the stubs at the ends.
pub fn ruined_wall(length: f32, height: f32, seed: u64) -> Vec<ArchPart> {
    let mut k = Kit::new(seed);
    k.ruined_wall(length.max(6.0), height.max(4.0), 3.0);
    k.finish()
}

// ---------------------------------------------------------------------------------------------
// Small math helpers
// ---------------------------------------------------------------------------------------------

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Round a height to the nearest course line.
fn snap(y: f32) -> f32 {
    (y / COURSE_H).round() * COURSE_H
}

fn mulc(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Horizontal joint offset of texture row `r` (rows are [`COURSE_H`] tall in v). With v = −y
/// on walls, row −1 is course 0 (no offset) and row −2 is course 1 (half a block).
fn row_offset(r: i64) -> f32 {
    if r.rem_euclid(2) == 0 {
        BLOCK_W * 0.5
    } else {
        0.0
    }
}

/// Split `[lo, hi]` at `off + k * period`, merging pieces thinner than `min` into their
/// neighbor.
fn spans(lo: f32, hi: f32, period: f32, off: f32, min: f32) -> Vec<(f32, f32)> {
    let mut cuts = vec![lo];
    let mut k = ((lo - off) / period).floor() + 1.0;
    loop {
        let x = off + k * period;
        if x >= hi - 1e-4 {
            break;
        }
        if x > lo + 1e-4 {
            cuts.push(x);
        }
        k += 1.0;
    }
    cuts.push(hi);
    if cuts.len() > 2 && cuts[1] - cuts[0] < min {
        cuts.remove(1);
    }
    let n = cuts.len();
    if n > 2 && cuts[n - 1] - cuts[n - 2] < min {
        cuts.remove(n - 2);
    }
    cuts.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Vertex with explicit UV.
fn vert(mb: &mut MeshBuilder, p: Vec3, n: Vec3, uv: Vec2, c: [f32; 3]) -> u32 {
    let i = mb.vertices.len() as u32;
    mb.vertices.push(Vertex {
        position: p.to_array(),
        normal: n.to_array(),
        uv: uv.to_array(),
        color: [c[0], c[1], c[2], 1.0],
    });
    i
}

/// Flat-shaded quad (any winding) whose front faces `out`; per-corner UV and color.
fn quad(mb: &mut MeshBuilder, p: [Vec3; 4], uv: [Vec2; 4], c: [[f32; 3]; 4], out: Vec3) {
    let n = (p[1] - p[0]).cross(p[2] - p[0]) + (p[2] - p[0]).cross(p[3] - p[0]);
    if n.length_squared() < 1e-14 || !n.is_finite() {
        return;
    }
    let flip = n.dot(out) < 0.0;
    let n = if flip { -n } else { n }.normalize();
    let base = mb.vertices.len() as u32;
    for k in 0..4 {
        vert(mb, p[k], n, uv[k], c[k]);
    }
    if flip {
        mb.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    } else {
        mb.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Quad with per-vertex normals (soft-shaded stone faces), wound to face the normals;
/// `alt` picks the other diagonal.
fn quad_n(
    mb: &mut MeshBuilder,
    p: [Vec3; 4],
    n: [Vec3; 4],
    uv: [Vec2; 4],
    c: [[f32; 3]; 4],
    alt: bool,
) {
    let g = (p[1] - p[0]).cross(p[2] - p[0]) + (p[2] - p[0]).cross(p[3] - p[0]);
    if g.length_squared() < 1e-14 || !g.is_finite() {
        return;
    }
    let rev = g.dot(n[0] + n[1] + n[2] + n[3]) < 0.0;
    let base = mb.vertices.len() as u32;
    for k in 0..4 {
        vert(mb, p[k], n[k], uv[k], c[k]);
    }
    let tris: [[u32; 3]; 2] = if alt {
        [[0, 1, 3], [1, 2, 3]]
    } else {
        [[0, 1, 2], [0, 2, 3]]
    };
    for t in tris {
        if rev {
            mb.indices
                .extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
        } else {
            mb.indices
                .extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
        }
    }
}

/// Flat triangle whose front faces `out`.
fn tri(mb: &mut MeshBuilder, p: [Vec3; 3], uv: [Vec2; 3], c: [[f32; 3]; 3], out: Vec3) {
    let n = (p[1] - p[0]).cross(p[2] - p[0]);
    if n.length_squared() < 1e-14 || !n.is_finite() {
        return;
    }
    let flip = n.dot(out) < 0.0;
    let n = if flip { -n } else { n }.normalize();
    let base = mb.vertices.len() as u32;
    for k in 0..3 {
        vert(mb, p[k], n, uv[k], c[k]);
    }
    if flip {
        mb.indices.extend_from_slice(&[base, base + 2, base + 1]);
    } else {
        mb.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
}

// ---------------------------------------------------------------------------------------------
// Chamfered hexahedra: corbels, copings, string courses, voussoirs, steps, loose stones, beams
// ---------------------------------------------------------------------------------------------

/// Machicolation console: per stone from the top (height m, reach at its top and bottom as
/// fractions of the projection). The lowest stone dies back into the wall.
const CORBEL: [(f32, f32, f32); 3] = [(0.3, 1.0, 0.86), (0.33, 0.76, 0.5), (0.37, 0.42, 0.05)];

/// Corner `i` of a hexahedron has bits x (1), y (2), z (4).
const HEX_FACES: [[usize; 4]; 6] = [
    [0, 4, 6, 2], // -X
    [1, 3, 7, 5], // +X
    [0, 1, 5, 4], // -Y
    [2, 6, 7, 3], // +Y
    [0, 2, 3, 1], // -Z
    [4, 5, 7, 6], // +Z
];
/// Face bits for [`hexa`]'s `hidden` mask.
const NX: u8 = 1;
const PX: u8 = 2;
const NY: u8 = 4;
const PY: u8 = 8;
const NZ: u8 = 16;
const PZ: u8 = 32;

/// How a hexahedron's faces get texture coordinates.
#[derive(Clone, Copy)]
enum Uvm {
    /// Box projection of the mesh-space position (stones laid on the texture grid).
    World,
    /// Box projection in the stone's own frame, centered on one texture block (loose stones).
    Local(Vec3, Quat),
}

fn hexa_uv(m: Uvm, p: Vec3, n: Vec3) -> Vec2 {
    match m {
        Uvm::World => box_uv(p, n),
        Uvm::Local(c, q) => {
            let inv = q.inverse();
            box_uv(inv * (p - c), inv * n) + Vec2::new(0.45, 0.675)
        }
    }
}

/// Corners of a box (`c` center, `h` half extents) turned by `q`, in hexahedron order.
fn box_corners(c: Vec3, h: Vec3, q: Quat) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        let s = Vec3::new(
            if i & 1 == 0 { -h.x } else { h.x },
            if i & 2 == 0 { -h.y } else { h.y },
            if i & 4 == 0 { -h.z } else { h.z },
        );
        c + q * s
    })
}

/// A hexahedron (8 corners, planar-ish faces) with every arris chamfered by `ch`. Faces in the
/// `hidden` mask are not drawn (they touch other stones) but their chamfers are, which makes
/// the recessed V joint between neighbors.
fn hexa(mb: &mut MeshBuilder, p: [Vec3; 8], hidden: u8, ch: f32, col: [f32; 3], uvm: Uvm) {
    hexa_e(mb, p, hidden, 0, ch, col, uvm);
}

/// [`hexa`] where faces in `embedded` sit inside other masonry: no face, no chamfers around it.
fn hexa_e(
    mb: &mut MeshBuilder,
    p: [Vec3; 8],
    hidden: u8,
    embedded: u8,
    ch: f32,
    col: [f32; 3],
    uvm: Uvm,
) {
    let hidden = hidden | embedded;
    let emb = |f: usize| embedded & (1 << f) != 0;
    let center = p.iter().copied().sum::<Vec3>() / 8.0;
    // Inset corner of every face.
    let mut inset = [[Vec3::ZERO; 8]; 6];
    let mut normals = [Vec3::ZERO; 6];
    for (f, face) in HEX_FACES.iter().enumerate() {
        let q = face.map(|i| p[i]);
        let mut n = Vec3::ZERO;
        for k in 0..4 {
            n += q[k].cross(q[(k + 1) % 4]);
        }
        let n = n.normalize_or(Vec3::Y);
        normals[f] = n;
        for k in 0..4 {
            let prev = q[(k + 3) % 4];
            let next = q[(k + 1) % 4];
            let a = n.cross(q[k] - prev).normalize_or_zero();
            let b = n.cross(next - q[k]).normalize_or_zero();
            let d = (1.0 + a.dot(b)).max(0.3);
            inset[f][face[k]] = q[k] + (a + b) * (ch / d);
        }
    }
    let shown = |f: usize| hidden & (1 << f) == 0;
    let dark = mulc(col, 0.78);
    let colf = |f: usize| if shown(f) { col } else { dark };
    for (f, face) in HEX_FACES.iter().enumerate() {
        if !shown(f) {
            continue;
        }
        let q = face.map(|i| inset[f][i]);
        let uv = q.map(|x| hexa_uv(uvm, x, normals[f]));
        quad(mb, q, uv, [col; 4], normals[f]);
    }
    // Edge chamfers.
    for f in 0..6 {
        for k in 0..4 {
            let a = HEX_FACES[f][k];
            let b = HEX_FACES[f][(k + 1) % 4];
            let Some(g) =
                (0..6).find(|&g| g != f && HEX_FACES[g].contains(&a) && HEX_FACES[g].contains(&b))
            else {
                continue;
            };
            if g < f || (!shown(f) && !shown(g)) || emb(f) || emb(g) {
                continue;
            }
            let q = [inset[f][a], inset[f][b], inset[g][b], inset[g][a]];
            let out = normals[f] + normals[g];
            let uv = q.map(|x| hexa_uv(uvm, x, out.normalize_or(Vec3::Y)));
            quad(mb, q, uv, [colf(f), colf(f), colf(g), colf(g)], out);
        }
    }
    // Corner triangles.
    for v in 0..8 {
        let fs: Vec<usize> = (0..6).filter(|&f| HEX_FACES[f].contains(&v)).collect();
        if fs.iter().all(|&f| !shown(f)) || fs.iter().any(|&f| emb(f)) {
            continue;
        }
        let q = [inset[fs[0]][v], inset[fs[1]][v], inset[fs[2]][v]];
        let out = p[v] - center;
        let uv = q.map(|x| hexa_uv(uvm, x, out.normalize_or(Vec3::Y)));
        tri(mb, q, uv, [colf(fs[0]), colf(fs[1]), colf(fs[2])], out);
    }
}

// ---------------------------------------------------------------------------------------------
// Surfaces and the ashlar tiler
// ---------------------------------------------------------------------------------------------

/// Maps texture coordinates (u, v), meters, to the nominal masonry surface.
trait Surf {
    fn pos(&self, u: f32, v: f32) -> Vec3;
    /// Outward unit normal.
    fn normal(&self, u: f32, v: f32) -> Vec3;
}

/// Plane `o + u du + v dv`.
#[derive(Clone, Copy, Debug)]
struct Plane {
    o: Vec3,
    du: Vec3,
    dv: Vec3,
    n: Vec3,
}

impl Plane {
    /// Vertical face with outward horizontal normal `n` through `p`, UV like `box_uv`
    /// (v = −y, u along `Y × n`), measured from the world origin so stones meet the texture
    /// grid.
    fn vertical(p: Vec3, n: Vec3) -> Self {
        let n = Vec3::new(n.x, 0.0, n.z).normalize();
        let o = n * p.dot(n);
        Self {
            o: Vec3::new(o.x, 0.0, o.z),
            du: Vec3::Y.cross(n),
            dv: -Vec3::Y,
            n,
        }
    }

    /// Horizontal face at height `y` facing up (`up`) or down; u = x, v = ±z like `box_uv`.
    fn horizontal(y: f32, up: bool) -> Self {
        Self {
            o: Vec3::new(0.0, y, 0.0),
            du: Vec3::X,
            dv: if up { Vec3::Z } else { -Vec3::Z },
            n: if up { Vec3::Y } else { -Vec3::Y },
        }
    }

    /// Vertical face leaning back: at y = 0 it stands `batter` meters further out than at
    /// `h` (a battered plinth). `p` is a point on the upper edge.
    fn battered(p: Vec3, n: Vec3, batter: f32, h: f32) -> Self {
        let mut s = Self::vertical(p, n);
        s.o += s.n * batter;
        s.dv = Vec3::new(0.0, -1.0, 0.0) + s.n * (batter / h);
        s.n = (s.n + Vec3::Y * (batter / h)).normalize();
        s
    }

    fn uv(&self, p: Vec3) -> Vec2 {
        let d = p - self.o;
        Vec2::new(
            d.dot(self.du) / self.du.length_squared(),
            d.dot(self.dv) / self.dv.length_squared(),
        )
    }

    /// The UV rectangle spanned by two 3D corners.
    fn rect(&self, a: Vec3, b: Vec3) -> Rect {
        let (a, b) = (self.uv(a), self.uv(b));
        Rect::new(a.x, a.y, b.x, b.y)
    }
}

impl Surf for Plane {
    fn pos(&self, u: f32, v: f32) -> Vec3 {
        self.o + self.du * u + self.dv * v
    }
    fn normal(&self, _u: f32, _v: f32) -> Vec3 {
        self.n
    }
}

/// Plane whose u axis widens toward the bottom (battered tower faces meet at the corners).
struct Trapezoid {
    plane: Plane,
    /// u of the face's center line.
    uc: f32,
    /// Half width at the top (y = `h`) and at y = 0, around u = 0.
    half_top: f32,
    half_bottom: f32,
    h: f32,
}

impl Surf for Trapezoid {
    fn pos(&self, u: f32, v: f32) -> Vec3 {
        let y = -v;
        let t = (y / self.h).clamp(0.0, 1.0);
        let half = self.half_bottom + (self.half_top - self.half_bottom) * t;
        self.plane
            .pos(self.uc + (u - self.uc) * half / self.half_top, v)
    }
    fn normal(&self, _u: f32, _v: f32) -> Vec3 {
        self.plane.n
    }
}

/// Vertical cylinder or cone frustum around `c` (radius `r0` at `y0`, `r1` at `y1`), u = arc
/// length at `k` meters per radian, v = −y.
#[derive(Clone, Copy, Debug)]
struct Cyl {
    c: Vec3,
    y0: f32,
    r0: f32,
    y1: f32,
    r1: f32,
    k: f32,
    inward: bool,
}

impl Cyl {
    fn new(c: Vec3, r: f32, k: f32) -> Self {
        Self {
            c,
            y0: 0.0,
            r0: r,
            y1: 1.0,
            r1: r,
            k,
            inward: false,
        }
    }
    fn radius(&self, y: f32) -> f32 {
        self.r0 + (self.r1 - self.r0) * (y - self.y0) / (self.y1 - self.y0)
    }
}

impl Surf for Cyl {
    fn pos(&self, u: f32, v: f32) -> Vec3 {
        let y = -v;
        let th = u / self.k;
        let r = self.radius(y);
        self.c + Vec3::new(r * th.cos(), y, r * th.sin())
    }
    fn normal(&self, u: f32, _v: f32) -> Vec3 {
        let th = u / self.k;
        let slope = (self.r0 - self.r1) / (self.y1 - self.y0);
        let n = Vec3::new(th.cos(), slope, th.sin()).normalize();
        if self.inward {
            -Vec3::new(n.x, -n.y, n.z)
        } else {
            n
        }
    }
}

/// Horizontal annulus around `c` at height `y`: u = arc length at radius `r` (`k` = u per
/// radian), v = radial distance from `r`.
#[derive(Clone, Copy, Debug)]
struct Ring {
    c: Vec3,
    y: f32,
    r: f32,
    k: f32,
    up: bool,
}

impl Surf for Ring {
    fn pos(&self, u: f32, v: f32) -> Vec3 {
        let th = u / self.k;
        let r = self.r + v;
        self.c + Vec3::new(r * th.cos(), self.y, r * th.sin())
    }
    fn normal(&self, _u: f32, _v: f32) -> Vec3 {
        if self.up { Vec3::Y } else { -Vec3::Y }
    }
}

/// Horizontal paving at height `y` clamped to radius `rmax` around `c` (tower platforms).
struct Disk {
    c: Vec3,
    y: f32,
    rmax: f32,
}

impl Surf for Disk {
    fn pos(&self, u: f32, v: f32) -> Vec3 {
        let mut d = Vec2::new(u - self.c.x, v - self.c.z);
        if d.length() > self.rmax {
            d = d.normalize() * self.rmax;
        }
        Vec3::new(self.c.x + d.x, self.y, self.c.z + d.y)
    }
    fn normal(&self, _u: f32, _v: f32) -> Vec3 {
        Vec3::Y
    }
}

/// How a face's stones end along one side.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Edge {
    /// Mortar joint or a seam hidden by other masonry: the arris is chamfered down to the joint.
    Groove,
    /// Convex corner with a face whose outward normal is the vector: both chamfer to one line.
    Corner(Vec3),
    /// Convex corner with a curved face whose normal points away from (`x`, `z`) (sign 1) or
    /// toward it (−1), horizontally.
    Radial(f32, f32, f32),
    /// The same stone continues (split around an opening).
    Flush,
    /// The rim of an opening whose chamfered frame is drawn by [`Kit::opening`].
    Rim,
}

impl Edge {
    fn bevelled(self) -> bool {
        !matches!(self, Edge::Flush | Edge::Rim)
    }
    /// The neighbor's outward normal at `p` for corner edges.
    fn corner_normal(self, p: Vec3) -> Option<Vec3> {
        match self {
            Edge::Corner(m) => Some(m),
            Edge::Radial(x, z, s) => {
                Some(Vec3::new(p.x - x, 0.0, p.z - z).normalize_or(Vec3::X) * s)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

impl Rect {
    fn new(u0: f32, v0: f32, u1: f32, v1: f32) -> Self {
        Self {
            u0: u0.min(u1),
            v0: v0.min(v1),
            u1: u0.max(u1),
            v1: v0.max(v1),
        }
    }
    fn w(&self) -> f32 {
        self.u1 - self.u0
    }
    fn h(&self) -> f32 {
        self.v1 - self.v0
    }
    fn overlaps(&self, o: &Rect) -> bool {
        self.u0 < o.u1 - 1e-4
            && o.u0 < self.u1 - 1e-4
            && self.v0 < o.v1 - 1e-4
            && o.v0 < self.v1 - 1e-4
    }
    fn grow(&self, d: f32) -> Rect {
        Rect::new(self.u0 - d, self.v0 - d, self.u1 + d, self.v1 + d)
    }
    fn shift_u(&self, d: f32) -> Rect {
        Rect::new(self.u0 + d, self.v0, self.u1 + d, self.v1)
    }
}

/// An opening in a face: cells next to it get `e` on the hole's (u0, u1, v0, v1) sides.
#[derive(Clone, Copy, Debug)]
struct Hole {
    r: Rect,
    e: [Edge; 4],
}

impl Hole {
    fn rim(r: Rect) -> Self {
        Self {
            r,
            e: [Edge::Rim; 4],
        }
    }
}

/// Look of a tiled face.
#[derive(Clone, Copy, Debug)]
struct Style {
    /// Random depth of each stone's face, meters.
    relief: f32,
    /// Chance a stone corner is chipped (0..1).
    wear: f32,
    /// Chance a stone is missing (fallen out); a few more stand proud.
    missing: f32,
    /// Per-stone brightness variation (0..1).
    var: f32,
    tint: [f32; 3],
    /// Closed ring of this many stones per course (cylinders), else bounded by the rect.
    wrap: Option<usize>,
    /// Course height in v (paving uses rows of [`COURSE_H`] too).
    joint: f32,
    /// Only the bed joints are modeled and stones are two texture blocks long, meeting flush
    /// (cheaper, for faces seen from inside or above; the texture paints the other joints).
    coarse: bool,
}

impl Style {
    fn ashlar() -> Self {
        Self {
            relief: 0.014,
            wear: 0.2,
            missing: 0.004,
            var: 0.09,
            tint: [1.0, 1.0, 1.0],
            wrap: None,
            joint: JOINT,
            coarse: false,
        }
    }
    fn coarse(mut self) -> Self {
        self.coarse = true;
        self
    }
    fn worn(mut self, wear: f32) -> Self {
        self.wear = wear;
        self
    }
    fn missing(mut self, m: f32) -> Self {
        self.missing = m;
        self
    }
    fn wrap(mut self, n: usize) -> Self {
        self.wrap = Some(n);
        self
    }
}

/// Directions of a rect's (u0, u1, v0, v1) sides, outward, in 3D.
fn side_dirs(s: &dyn Surf, r: &Rect) -> [Vec3; 4] {
    let (um, vm) = ((r.u0 + r.u1) * 0.5, (r.v0 + r.v1) * 0.5);
    let e = 0.02;
    [
        (s.pos(r.u0, vm) - s.pos(r.u0 + e, vm)).normalize_or_zero(),
        (s.pos(r.u1, vm) - s.pos(r.u1 - e, vm)).normalize_or_zero(),
        (s.pos(um, r.v0) - s.pos(um, r.v0 + e)).normalize_or_zero(),
        (s.pos(um, r.v1) - s.pos(um, r.v1 - e)).normalize_or_zero(),
    ]
}

/// Split cell `c` around `hole` into up to 8 cells.
fn split_cell(c: Rect, ce: [Edge; 4], hole: &Hole) -> Vec<(Rect, [Edge; 4])> {
    if !c.overlaps(&hole.r) {
        return vec![(c, ce)];
    }
    let us = [c.u0, hole.r.u0.max(c.u0), hole.r.u1.min(c.u1), c.u1];
    let vs = [c.v0, hole.r.v0.max(c.v0), hole.r.v1.min(c.v1), c.v1];
    let mut out = Vec::new();
    for iv in 0..3 {
        for iu in 0..3 {
            if iu == 1 && iv == 1 {
                continue;
            }
            let r = Rect::new(us[iu], vs[iv], us[iu + 1], vs[iv + 1]);
            if r.w() < 1e-4 || r.h() < 1e-4 {
                continue;
            }
            let e0 = match iu {
                0 => ce[0],
                1 => Edge::Flush,
                _ if iv == 1 => hole.e[1],
                _ => Edge::Flush,
            };
            let e1 = match iu {
                2 => ce[1],
                1 => Edge::Flush,
                _ if iv == 1 => hole.e[0],
                _ => Edge::Flush,
            };
            let e2 = match iv {
                0 => ce[2],
                1 => Edge::Flush,
                _ if iu == 1 => hole.e[3],
                _ => Edge::Flush,
            };
            let e3 = match iv {
                2 => ce[3],
                1 => Edge::Flush,
                _ if iu == 1 => hole.e[2],
                _ => Edge::Flush,
            };
            out.push((r, [e0, e1, e2, e3]));
        }
    }
    out
}

/// Where a cell's corner meets the joint/corner line.
fn boundary_point(p: Vec3, n: Vec3, ea: Edge, eb: Edge, ch: f32, depth: f32) -> Vec3 {
    let mut m = Vec3::ZERO;
    let mut corner = false;
    for e in [ea, eb] {
        if let Some(cm) = e.corner_normal(p) {
            m += cm;
            corner = true;
        }
    }
    if corner {
        p - (n + m) * (ch * 0.5)
    } else if ea.bevelled() || eb.bevelled() {
        p - n * ch
    } else {
        p + n * depth
    }
}

/// An opening through or into a face.
#[derive(Clone, Copy, Debug)]
struct Opening {
    /// The clear opening at the face, UV meters.
    r: Rect,
    /// Width of the chamfer around it.
    frame: f32,
    /// Depth of the reveal before its back wall (blind openings).
    depth: f32,
    /// How much the reveal widens per meter of depth on each side (splayed embrasures).
    splay: f32,
}

// ---------------------------------------------------------------------------------------------
// The kit: material builders, stones, faces, finishing
// ---------------------------------------------------------------------------------------------

struct Kit {
    seed: u64,
    salt: u64,
    rng: Rng,
    stone: MeshBuilder,
    wood: MeshBuilder,
    metal: MeshBuilder,
    rock: MeshBuilder,
    dirt: MeshBuilder,
    /// Painted cloth (banners).
    plaster: MeshBuilder,
    /// Points water runs down from (spouts, slits): dark streaks below them (point, width).
    streaks: Vec<(Vec3, f32)>,
    /// Siege damage: impact scars (center, outward face normal, radius) smash the stones
    /// around them and soot the face.
    scars: Vec<(Vec3, Vec3, f32)>,
    /// Boxes buried inside other masonry: stones entirely inside are not built.
    cull: Vec<(Vec3, Vec3)>,
}

impl Kit {
    fn new(seed: u64) -> Self {
        Self {
            seed,
            salt: 0,
            rng: Rng::new(seed ^ 0x5eed_a5c4),
            stone: MeshBuilder::new(),
            wood: MeshBuilder::new(),
            metal: MeshBuilder::new(),
            rock: MeshBuilder::new(),
            dirt: MeshBuilder::new(),
            plaster: MeshBuilder::new(),
            streaks: Vec::new(),
            scars: Vec::new(),
            cull: Vec::new(),
        }
    }

    /// Is every point inside one of the cull boxes?
    fn culled(&self, pts: &[Vec3]) -> bool {
        self.cull.iter().any(|(lo, hi)| {
            pts.iter()
                .all(|p| p.cmpgt(*lo + 0.02).all() && p.cmplt(*hi - 0.02).all())
        })
    }

    fn next_salt(&mut self) -> u64 {
        self.salt += 1;
        self.salt
    }

    fn hash(&self, a: i64, b: i64, salt: u64) -> f32 {
        hash01(a, b, salt as i64, self.seed)
    }

    /// Random stone tint around `base`.
    fn stone_tint(&mut self, var: f32) -> [f32; 3] {
        let lum = 1.0 + (self.rng.f32() - 0.5) * 2.0 * var;
        let warm = (self.rng.f32() - 0.5) * var * 0.7;
        [lum * (1.0 + warm), lum, lum * (1.0 - warm)]
    }

    /// A chamfered stone in the stone mesh.
    fn stone_hexa(&mut self, p: [Vec3; 8], hidden: u8, ch: f32, uvm: Uvm) {
        self.stone_hexa_e(p, hidden, 0, ch, uvm);
    }

    /// A chamfered stone partly built into other masonry (`embedded` faces).
    fn stone_hexa_e(&mut self, p: [Vec3; 8], hidden: u8, embedded: u8, ch: f32, uvm: Uvm) {
        if self.culled(&p) {
            return;
        }
        let t = self.stone_tint(0.08);
        hexa_e(&mut self.stone, p, hidden, embedded, ch, t, uvm);
    }

    /// Axis-aligned chamfered stone between `lo` and `hi` in a frame.
    fn stone_box(&mut self, fr: &Frame, lo: Vec3, hi: Vec3, hidden: u8, ch: f32) {
        let p = fr.box_corners(lo, hi);
        self.stone_hexa(p, fr.map_hidden(hidden), ch, Uvm::World);
    }

    /// Tile a face with cut stones laid on the texture grid. `edge` picks each side's kind
    /// from its outward 3D direction.
    fn face(
        &mut self,
        s: &dyn Surf,
        r: Rect,
        edge: impl Fn(Vec3) -> Edge,
        holes: &[Hole],
        st: Style,
    ) {
        if r.w() < 1e-3 || r.h() < 1e-3 {
            return;
        }
        let dirs = side_dirs(s, &r);
        let fe = dirs.map(&edge);
        self.face_e(s, r, fe, holes, st);
    }

    /// [`Kit::face`] with explicit (u0, u1, v0, v1) side kinds.
    fn face_e(&mut self, s: &dyn Surf, r: Rect, fe: [Edge; 4], holes: &[Hole], st: Style) {
        if r.w() < 1e-3 || r.h() < 1e-3 {
            return;
        }
        let salt = self.next_salt();
        let period = st.wrap.map(|n| n as f32 * BLOCK_W);
        for (va, vb) in spans(r.v0, r.v1, COURSE_H, 0.0, 0.13) {
            let row = (((va + vb) * 0.5) / COURSE_H).floor() as i64;
            let off = row_offset(row);
            // Coarse faces lay two texture blocks per modeled stone.
            let bw = if st.coarse { BLOCK_W * 2.0 } else { BLOCK_W };
            let blocks: Vec<(f32, f32)> = match st.wrap {
                Some(n) if st.coarse && n % 2 == 0 => (0..n / 2)
                    .map(|j| (off + j as f32 * bw, off + (j + 1) as f32 * bw))
                    .collect(),
                Some(n) => (0..n)
                    .map(|j| (off + j as f32 * BLOCK_W, off + (j + 1) as f32 * BLOCK_W))
                    .collect(),
                None => spans(r.u0, r.u1, bw, off, 0.22),
            };
            for (ua, ub) in blocks {
                let b = Rect::new(ua, va, ub, vb);
                let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
                let head = if st.coarse { Edge::Flush } else { Edge::Groove };
                let be = [
                    if st.wrap.is_none() && near(ua, r.u0) {
                        fe[0]
                    } else {
                        head
                    },
                    if st.wrap.is_none() && near(ub, r.u1) {
                        fe[1]
                    } else {
                        head
                    },
                    if near(va, r.v0) { fe[2] } else { Edge::Groove },
                    if near(vb, r.v1) { fe[3] } else { Edge::Groove },
                ];
                let jid = (ua / BLOCK_W * 2.0).round() as i64;
                self.block(s, b, be, holes, &st, period, row, jid, salt);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        s: &dyn Surf,
        b: Rect,
        be: [Edge; 4],
        holes: &[Hole],
        st: &Style,
        period: Option<f32>,
        row: i64,
        jid: i64,
        salt: u64,
    ) {
        if !self.cull.is_empty() {
            let pts = [
                s.pos(b.u0, b.v0),
                s.pos(b.u1, b.v0),
                s.pos(b.u1, b.v1),
                s.pos(b.u0, b.v1),
            ];
            if self.culled(&pts) {
                return;
            }
        }
        let seed = self.seed;
        let h = |k: i64| hash01(row, jid * 32 + k, salt as i64, seed);
        let mut cells = vec![(b, be)];
        let mut touched = false;
        for hole in holes {
            let shifts: &[f32] = match period {
                Some(p) => &[-p, 0.0, p],
                None => &[0.0],
            };
            for &sh in shifts {
                let hr = Hole {
                    r: hole.r.shift_u(sh),
                    e: hole.e,
                };
                if !hr.r.overlaps(&b) {
                    continue;
                }
                touched = true;
                cells = cells
                    .into_iter()
                    .flat_map(|(c, ce)| split_cell(c, ce, &hr))
                    .collect();
            }
        }
        let lum = 1.0 + (h(3) - 0.5) * 2.0 * st.var;
        let warm = (h(4) - 0.5) * st.var * 0.7;
        let tint = [
            st.tint[0] * lum * (1.0 + warm),
            st.tint[1] * lum,
            st.tint[2] * lum * (1.0 - warm),
        ];
        let plain = be.iter().all(|e| *e == Edge::Groove);
        let bc = ((b.u0 + b.u1) * 0.5, (b.v0 + b.v1) * 0.5);
        // Inside an impact scar (0 at its rim, 1 at the center).
        let bp = s.pos(bc.0, bc.1);
        let scar = self
            .scars
            .iter()
            .map(|&(c, _, r)| 1.0 - (bp - c).length() / r)
            .fold(0.0f32, f32::max);
        if !touched && plain && b.w() > 0.3 && scar > 0.0 && h(25) < 0.25 + scar * 1.6 {
            let fresh = mix3(tint, [1.12, 1.1, 1.06], 0.6);
            let depth = 0.12 + 0.3 * scar;
            self.missing_block(s, b, st.joint, fresh, [h(6), h(7), h(8), h(9)], depth);
            return;
        }
        if !touched && plain && h(5) < st.missing && b.w() > 0.5 {
            self.missing_block(s, b, st.joint, tint, [h(6), h(7), h(8), h(9)], 0.18);
            return;
        }
        let flat = touched || be.iter().any(|e| !e.bevelled());
        let mut plane = if flat {
            [0.0; 3]
        } else {
            [
                (h(0) - 0.45) * 2.0 * st.relief,
                (h(1) - 0.5) * st.relief * 1.5 / b.w(),
                (h(2) - 0.5) * st.relief * 1.5 / b.h(),
            ]
        };
        if !flat && h(10) < st.missing * 3.0 {
            plane[0] += 0.025 + 0.035 * h(11);
        }
        // Stones around a scar are shaken loose: driven in, askew and battered.
        if !flat && scar > 0.0 {
            plane[0] -= (0.015 + 0.05 * h(26)) * (scar * 3.0).min(1.0);
            plane[1] *= 1.0 + 4.0 * scar;
            plane[2] *= 1.0 + 4.0 * scar;
        }
        let wear = if scar > 0.0 {
            st.wear.max(0.9)
        } else {
            st.wear
        };
        let chips: [f32; 4] = std::array::from_fn(|k| {
            if h(12 + k as i64) < wear {
                0.02 + if scar > 0.0 { 0.1 } else { 0.075 } * h(16 + k as i64).powi(2)
            } else {
                0.0
            }
        });
        for (c, ce) in cells {
            cell(
                &mut self.stone,
                s,
                c,
                ce,
                plane,
                bc,
                &b,
                chips,
                tint,
                st.joint,
                [h(20), h(21), h(22), h(23), h(24)],
            );
        }
    }

    /// A fallen-out stone: a ragged recess about `depth` deep into the rubble core.
    fn missing_block(
        &mut self,
        s: &dyn Surf,
        b: Rect,
        ch: f32,
        tint: [f32; 3],
        hs: [f32; 4],
        depth: f32,
    ) {
        let cu = [b.u0, b.u1, b.u1, b.u0];
        let cv = [b.v0, b.v0, b.v1, b.v1];
        let su = [1.0, -1.0, -1.0, 1.0];
        let sv = [1.0, 1.0, -1.0, -1.0];
        let mut bp = [Vec3::ZERO; 4];
        let mut kp = [Vec3::ZERO; 4];
        let mut uvb = [Vec2::ZERO; 4];
        let mut uvk = [Vec2::ZERO; 4];
        for k in 0..4 {
            let n = s.normal(cu[k], cv[k]);
            bp[k] = s.pos(cu[k], cv[k]) - n * ch;
            uvb[k] = Vec2::new(cu[k], cv[k]);
            let (iu, iv) = (
                cu[k] + su[k] * 0.06 * hs[k],
                cv[k] + sv[k] * 0.04 * hs[(k + 1) % 4],
            );
            let d = depth * (1.0 + 1.1 * hs[(k + 2) % 4]);
            kp[k] = s.pos(iu, iv) - s.normal(iu, iv) * d;
            uvk[k] = Vec2::new(iu, iv);
        }
        let center = (bp.iter().copied().sum::<Vec3>() + kp.iter().copied().sum::<Vec3>()) / 8.0;
        let dark = mulc(tint, 0.55);
        let mb = &mut self.stone;
        for (a, bb) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
            let q = [bp[a], bp[bb], kp[bb], kp[a]];
            let mid = (bp[a] + bp[bb]) * 0.5;
            quad(
                mb,
                q,
                [uvb[a], uvb[bb], uvk[bb], uvk[a]],
                [tint, tint, dark, dark],
                center - mid,
            );
        }
        let n = s.normal((b.u0 + b.u1) * 0.5, (b.v0 + b.v1) * 0.5);
        quad(mb, kp, uvk, [dark; 4], n);
    }

    /// The chamfered frame and reveal of an opening; returns the hole to cut from the face.
    /// `through` connects the reveal to an opening in another face instead of a back wall.
    fn opening(
        &mut self,
        s: &dyn Surf,
        o: &Opening,
        through: Option<(&dyn Surf, &Opening)>,
    ) -> Hole {
        let tint = self.stone_tint(0.05);
        let (rim, inner, uvi) = frame_points(s, o);
        let uvr = [
            Vec2::new(o.r.u0 - o.frame, o.r.v0 - o.frame),
            Vec2::new(o.r.u1 + o.frame, o.r.v0 - o.frame),
            Vec2::new(o.r.u1 + o.frame, o.r.v1 + o.frame),
            Vec2::new(o.r.u0 - o.frame, o.r.v1 + o.frame),
        ];
        let c = s.pos((o.r.u0 + o.r.u1) * 0.5, (o.r.v0 + o.r.v1) * 0.5);
        let n = s.normal((o.r.u0 + o.r.u1) * 0.5, (o.r.v0 + o.r.v1) * 0.5);
        let mb = &mut self.stone;
        if o.frame > 1e-4 {
            for k in 0..4 {
                let k2 = (k + 1) % 4;
                let mid = (rim[k] + rim[k2]) * 0.5;
                let out = n + (c - mid).normalize_or_zero();
                quad(
                    mb,
                    [rim[k], rim[k2], inner[k2], inner[k]],
                    [uvr[k], uvr[k2], uvi[k2], uvi[k]],
                    [tint; 4],
                    out,
                );
            }
        }
        let dark = mulc(tint, 0.6);
        let deep = mulc(tint, 0.3);
        match through {
            None => {
                let back = Rect::new(
                    o.r.u0 - o.splay * o.depth,
                    o.r.v0 - o.splay * o.depth,
                    o.r.u1 + o.splay * o.depth,
                    o.r.v1 + o.splay * o.depth,
                );
                let cu = [back.u0, back.u1, back.u1, back.u0];
                let cv = [back.v0, back.v0, back.v1, back.v1];
                let kp: [Vec3; 4] =
                    std::array::from_fn(|k| s.pos(cu[k], cv[k]) - s.normal(cu[k], cv[k]) * o.depth);
                let uvk: [Vec2; 4] = std::array::from_fn(|k| Vec2::new(cu[k], cv[k]));
                let axis = c - n * o.depth * 0.5;
                for k in 0..4 {
                    let k2 = (k + 1) % 4;
                    let mid = (inner[k] + inner[k2] + kp[k] + kp[k2]) * 0.25;
                    quad(
                        mb,
                        [inner[k], inner[k2], kp[k2], kp[k]],
                        [uvi[k], uvi[k2], uvk[k2], uvk[k]],
                        [dark, dark, deep, deep],
                        axis - mid,
                    );
                }
                quad(mb, kp, uvk, [deep; 4], n);
            }
            Some((s2, o2)) => {
                let (rim2, inner2, uvi2) = frame_points(s2, o2);
                let c2 = s2.pos((o2.r.u0 + o2.r.u1) * 0.5, (o2.r.v0 + o2.r.v1) * 0.5);
                let n2 = s2.normal((o2.r.u0 + o2.r.u1) * 0.5, (o2.r.v0 + o2.r.v1) * 0.5);
                if o2.frame > 1e-4 {
                    for k in 0..4 {
                        let k2 = (k + 1) % 4;
                        let mid = (rim2[k] + rim2[k2]) * 0.5;
                        quad(
                            mb,
                            [rim2[k], rim2[k2], inner2[k2], inner2[k]],
                            [uvi2[k], uvi2[k2], uvi2[k2], uvi2[k]],
                            [tint; 4],
                            n2 + (c2 - mid).normalize_or_zero(),
                        );
                    }
                }
                // Match corners across the wall by their position along the face.
                let flat = |p: Vec3| p - n * p.dot(n);
                let m: [usize; 4] = std::array::from_fn(|k| {
                    (0..4)
                        .min_by(|&a, &b| {
                            (flat(inner2[a]) - flat(inner[k]))
                                .length()
                                .total_cmp(&(flat(inner2[b]) - flat(inner[k])).length())
                        })
                        .unwrap_or(k)
                });
                let axis = (c + c2) * 0.5;
                for k in 0..4 {
                    let k2 = (k + 1) % 4;
                    let mid = (inner[k] + inner[k2] + inner2[m[k]] + inner2[m[k2]]) * 0.25;
                    quad(
                        mb,
                        [inner[k], inner[k2], inner2[m[k2]], inner2[m[k]]],
                        [uvi[k], uvi[k2], uvi2[m[k2]], uvi2[m[k]]],
                        [dark; 4],
                        axis - mid,
                    );
                }
            }
        }
        Hole::rim(o.r.grow(o.frame))
    }

    /// Collect the parts after weathering, noise displacement and ambient occlusion.
    fn finish(mut self) -> Vec<ArchPart> {
        let seed = self.seed;
        let streaks = std::mem::take(&mut self.streaks);
        let scars = std::mem::take(&mut self.scars);
        // Weather the masonry.
        for v in &mut self.stone.vertices {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            let w = weather(p, n, seed, &streaks, &scars);
            v.color = [v.color[0] * w[0], v.color[1] * w[1], v.color[2] * w[2], 1.0];
        }
        let mut parts = vec![
            (DestructibleMaterial::Stone, self.stone),
            (DestructibleMaterial::Wood, self.wood),
            (DestructibleMaterial::Metal, self.metal),
            (DestructibleMaterial::Rock, self.rock),
            (DestructibleMaterial::Dirt, self.dirt),
            (DestructibleMaterial::Plaster, self.plaster),
        ];
        parts.retain(|(_, m)| !m.is_empty());
        for (_, m) in &mut parts {
            for v in &mut m.vertices {
                v.position = displace(Vec3::from(v.position), seed).to_array();
            }
        }
        bake_ao(&mut parts);
        parts
            .into_iter()
            .map(|(material, mesh)| ArchPart { material, mesh })
            .collect()
    }
}

/// Rim (outer, at the face) and inner (after the chamfer) corners of an opening.
fn frame_points(s: &dyn Surf, o: &Opening) -> ([Vec3; 4], [Vec3; 4], [Vec2; 4]) {
    let g = o.r.grow(o.frame);
    let cu = [g.u0, g.u1, g.u1, g.u0];
    let cv = [g.v0, g.v0, g.v1, g.v1];
    let iu = [o.r.u0, o.r.u1, o.r.u1, o.r.u0];
    let iv = [o.r.v0, o.r.v0, o.r.v1, o.r.v1];
    let rim = std::array::from_fn(|k| s.pos(cu[k], cv[k]));
    let inner = std::array::from_fn(|k| s.pos(iu[k], iv[k]) - s.normal(iu[k], iv[k]) * o.frame);
    let uv = std::array::from_fn(|k| Vec2::new(iu[k], iv[k]));
    (rim, inner, uv)
}

/// Emit one stone (or the part of one around an opening): a face with chamfered arrises
/// sloping down into the joints.
#[allow(clippy::too_many_arguments)]
fn cell(
    mb: &mut MeshBuilder,
    s: &dyn Surf,
    c: Rect,
    e: [Edge; 4],
    plane: [f32; 3],
    bc: (f32, f32),
    blk: &Rect,
    chips: [f32; 4],
    tint: [f32; 3],
    ch: f32,
    rnd: [f32; 5],
) {
    let cu = [c.u0, c.u1, c.u1, c.u0];
    let cv = [c.v0, c.v0, c.v1, c.v1];
    // Corner k touches u-edge ue[k] and v-edge ve[k].
    let ue = [0usize, 1, 1, 0];
    let ve = [2usize, 2, 3, 3];
    let su = [1.0f32, -1.0, -1.0, 1.0];
    let sv = [1.0f32, 1.0, -1.0, -1.0];
    // Arrises wear unevenly: each side of a stone gets its own chamfer width.
    let ins = |i: usize| {
        if e[i].bevelled() {
            ch * (0.55 + 0.75 * rnd[i])
        } else {
            0.0
        }
    };
    let mut iu = [ins(0), ins(1)];
    let mut iv = [ins(2), ins(3)];
    let fit = |i: &mut [f32; 2], w: f32| {
        let s = i[0] + i[1];
        if s > w * 0.5 {
            let k = w * 0.5 / s;
            i[0] *= k;
            i[1] *= k;
        }
    };
    fit(&mut iu, c.w());
    fit(&mut iv, c.h());
    let depth = |u: f32, v: f32| plane[0] + plane[1] * (u - bc.0) + plane[2] * (v - bc.1);
    let blk_corner = |u: f32, v: f32| -> Option<usize> {
        let bu = [blk.u0, blk.u1, blk.u1, blk.u0];
        let bv = [blk.v0, blk.v0, blk.v1, blk.v1];
        (0..4).find(|&k| (bu[k] - u).abs() < 1e-4 && (bv[k] - v).abs() < 1e-4)
    };
    let mut f = [Vec3::ZERO; 4];
    let mut fuv = [Vec2::ZERO; 4];
    for k in 0..4 {
        let (ea, eb) = (e[ue[k]], e[ve[k]]);
        let chip = match blk_corner(cu[k], cv[k]) {
            Some(bk) if ea.bevelled() && eb.bevelled() => {
                chips[bk].min(c.w() * 0.3).min(c.h() * 0.3)
            }
            _ => 0.0,
        };
        let iuk = if ue[k] == 0 { iu[0] } else { iu[1] };
        let ivk = if ve[k] == 2 { iv[0] } else { iv[1] };
        let u = cu[k] + su[k] * (iuk + chip);
        let v = cv[k] + sv[k] * (ivk + chip);
        let n = s.normal(u, v);
        f[k] = s.pos(u, v) + n * (depth(u, v) - chip * 0.45);
        fuv[k] = Vec2::new(u, v);
    }
    let mut bp = [Vec3::ZERO; 4];
    let mut rim = [Vec3::ZERO; 4];
    let buv: [Vec2; 4] = std::array::from_fn(|k| Vec2::new(cu[k], cv[k]));
    for k in 0..4 {
        let p = s.pos(cu[k], cv[k]);
        let n = s.normal(cu[k], cv[k]);
        let d = depth(cu[k], cv[k]);
        bp[k] = boundary_point(p, n, e[ue[k]], e[ve[k]], ch, d);
        rim[k] = p + n * d;
    }
    let um = (c.u0 + c.u1) * 0.5;
    let vm = (c.v0 + c.v1) * 0.5;
    let nc = s.normal(um, vm);
    let center = s.pos(um, vm);
    let joint = mulc(tint, 0.8);
    // Soft stone face: normals lean toward the chamfered sides, and on
    // curved walls follow the curvature.
    let gn = (f[1] - f[0]).cross(f[2] - f[0]) + (f[2] - f[0]).cross(f[3] - f[0]);
    let nf = if gn.dot(nc) < 0.0 { -gn } else { gn }.normalize_or(nc);
    let dirs = side_dirs(s, &c);
    let nrm: [Vec3; 4] = std::array::from_fn(|k| {
        let mut t = Vec3::ZERO;
        if e[ue[k]].bevelled() {
            t += dirs[ue[k]];
        }
        if e[ve[k]].bevelled() {
            t += dirs[ve[k]];
        }
        (nf + (s.normal(fuv[k].x, fuv[k].y) - nc) + t * 0.04).normalize_or(nf)
    });
    quad_n(mb, f, nrm, fuv, [tint; 4], rnd[4] < 0.5);
    // Edge k: (corner a, corner b).
    let edges = [(3usize, 0usize), (1, 2), (0, 1), (2, 3)];
    for (i, &(a, b)) in edges.iter().enumerate() {
        if !e[i].bevelled() {
            continue;
        }
        let mid = (bp[a] + bp[b]) * 0.5;
        let out = nc + (mid - center).normalize_or_zero();
        quad(
            mb,
            [f[a], f[b], bp[b], bp[a]],
            [fuv[a], fuv[b], buv[b], buv[a]],
            [tint, tint, joint, joint],
            out,
        );
    }
    // Close joints where they run into an opening's frame.
    for k in 0..4 {
        let (ea, eb) = (e[ue[k]], e[ve[k]]);
        let (rim_e, joint_e) = if ea == Edge::Rim && eb.bevelled() {
            (ue[k], ve[k])
        } else if eb == Edge::Rim && ea.bevelled() {
            (ve[k], ue[k])
        } else {
            continue;
        };
        let _ = joint_e;
        let out = match rim_e {
            0 => s.pos(c.u0, vm) - s.pos(c.u0 + 0.01, vm),
            1 => s.pos(c.u1, vm) - s.pos(c.u1 - 0.01, vm),
            2 => s.pos(um, c.v0) - s.pos(um, c.v0 + 0.01),
            _ => s.pos(um, c.v1) - s.pos(um, c.v1 - 0.01),
        };
        tri(
            mb,
            [f[k], rim[k], bp[k]],
            [fuv[k], buv[k], buv[k]],
            [tint, tint, joint],
            out,
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Weathering, displacement, ambient occlusion
// ---------------------------------------------------------------------------------------------

/// Color multiplier for weathered masonry at `p` with normal `n`.
fn weather(
    p: Vec3,
    n: Vec3,
    seed: u64,
    streaks: &[(Vec3, f32)],
    scars: &[(Vec3, Vec3, f32)],
) -> [f32; 3] {
    let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
    let f = |sx: f64, sy: f64, salt: u64, oct: u32| {
        fbm3(x * sx, y * sy, z * sx, seed ^ salt, oct, 2.0, 0.55)
    };
    // Broad tonal patches over many stones (bleached vs. stained), warm and cool.
    let mut c = mulc([1.0; 3], 1.0 + 0.3 * f(0.07, 0.1, 5, 3));
    let warm = f(0.05, 0.05, 6, 2);
    c = [c[0] * (1.0 + 0.1 * warm), c[1], c[2] * (1.0 - 0.12 * warm)];
    // Rising damp and splash grime at the foot: darker and greener, ragged upper edge.
    let g = 1.0 - smoothstep(0.0, 2.8, p.y + 0.9 * f(0.45, 0.6, 11, 3));
    c = mulc(c, 1.0 - 0.5 * g);
    c = mix3(c, [0.7, 0.8, 0.55], 0.45 * g);
    // Vertical rain streaks on walls: long, uneven, grey.
    if n.y.abs() < 0.6 {
        let st = fbm3(
            x * 3.0 + z * 0.5,
            y * 0.08,
            z * 3.0 - x * 0.5,
            seed ^ 23,
            3,
            2.1,
            0.55,
        );
        c = mix3(c, [0.55, 0.55, 0.53], smoothstep(0.0, 0.45, st) * 0.4);
    }
    // Dark streaks below spouts, slits and crenels.
    for &(q, w) in streaks {
        let dy = q.y - p.y;
        if !(0.0..=9.0).contains(&dy) {
            continue;
        }
        let dh = Vec2::new(p.x - q.x, p.z - q.z).length();
        let wobble = 1.0 + 0.35 * perlin3(x * 3.0, y * 0.7, z * 3.0, seed ^ 59);
        let wid = w * (1.0 + dy * 0.12) * wobble;
        if dh < wid {
            let k = (1.0 - dh / wid).powf(0.5) * (-dy / 5.5).exp() * 0.75;
            c = mix3(c, [0.3, 0.3, 0.27], k);
        }
    }
    // Lichen crusts, mostly on upward faces, and dark moss in damp spots.
    let l = smoothstep(0.3, 0.7, f(0.3, 0.3, 37, 4) + 0.3 * n.y.max(0.0));
    c = mix3(c, [0.9, 0.93, 0.68], 0.42 * l);
    let m = smoothstep(0.35, 0.75, f(0.6, 0.6, 43, 3) + 0.25 * g);
    c = mix3(c, [0.42, 0.46, 0.34], 0.45 * m);
    // Soot and powder burn around impact scars, ragged and smeared upward.
    for &(q, _, r) in scars {
        let d = p - q;
        let reach = r * (2.1 + 0.6 * perlin3(x * 2.1, y * 2.1, z * 2.1, seed ^ 61));
        let dist = Vec3::new(d.x, d.y.min(0.0) + d.y.max(0.0) * 0.6, d.z).length();
        if dist < reach {
            let k = (1.0 - dist / reach).powf(1.1) * 0.7;
            c = mix3(c, [0.24, 0.22, 0.2], k);
        }
    }
    let sp = perlin3(x * 7.0, y * 7.0, z * 7.0, seed ^ 41) * 0.06;
    mulc(c, 1.0 + sp)
}

/// Gentle low-frequency displacement so no edge is perfectly straight (≤ 2.5 cm).
fn displace(p: Vec3, seed: u64) -> Vec3 {
    let s = 0.12;
    let (x, y, z) = (p.x as f64 * s, p.y as f64 * s, p.z as f64 * s);
    let d = Vec3::new(
        fbm3(x, y, z, seed ^ 101, 3, 2.2, 0.5),
        0.4 * fbm3(x, y, z, seed ^ 202, 3, 2.2, 0.5),
        fbm3(x, y, z, seed ^ 303, 3, 2.2, 0.5),
    );
    p + d * 0.024
}

#[derive(Clone, Copy)]
struct BvhNode {
    lo: Vec3,
    hi: Vec3,
    /// Leaf: first triangle; inner: left child (right is `a + 1`).
    a: u32,
    /// Triangles in a leaf, 0 for inner nodes.
    n: u32,
}

/// Bounding volume hierarchy over triangles for occlusion rays.
struct Bvh {
    nodes: Vec<BvhNode>,
    tris: Vec<[Vec3; 3]>,
}

impl Bvh {
    fn new(tris: Vec<[Vec3; 3]>) -> Self {
        let cent: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut idx: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = vec![BvhNode {
            lo: Vec3::ZERO,
            hi: Vec3::ZERO,
            a: 0,
            n: 0,
        }];
        let mut stack = vec![(0usize, 0usize, tris.len())];
        while let Some((ni, s, e)) = stack.pop() {
            let mut lo = Vec3::splat(f32::MAX);
            let mut hi = Vec3::splat(f32::MIN);
            let mut clo = lo;
            let mut chi = hi;
            for &i in &idx[s..e] {
                for v in tris[i as usize] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                clo = clo.min(cent[i as usize]);
                chi = chi.max(cent[i as usize]);
            }
            nodes[ni].lo = lo;
            nodes[ni].hi = hi;
            if e - s <= 4 {
                nodes[ni].a = s as u32;
                nodes[ni].n = (e - s) as u32;
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
            idx[s..e].select_nth_unstable_by(mid - s, |&a, &b| {
                cent[a as usize][axis].total_cmp(&cent[b as usize][axis])
            });
            let left = nodes.len();
            nodes.push(nodes[ni]);
            nodes.push(nodes[ni]);
            nodes[ni].a = left as u32;
            nodes[ni].n = 0;
            stack.push((left, s, mid));
            stack.push((left + 1, mid, e));
        }
        let tris = idx.iter().map(|&i| tris[i as usize]).collect();
        Self { nodes, tris }
    }

    /// Does the ray from `o` along unit `d` hit anything closer than `tmax`?
    fn occluded(&self, o: Vec3, d: Vec3, tmax: f32) -> bool {
        if self.tris.is_empty() {
            return false;
        }
        let inv = Vec3::new(
            if d.x.abs() > 1e-9 {
                1.0 / d.x
            } else {
                1e30f32.copysign(d.x)
            },
            if d.y.abs() > 1e-9 {
                1.0 / d.y
            } else {
                1e30f32.copysign(d.y)
            },
            if d.z.abs() > 1e-9 {
                1.0 / d.z
            } else {
                1e30f32.copysign(d.z)
            },
        );
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
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
            if node.n > 0 {
                for t in &self.tris[node.a as usize..(node.a + node.n) as usize] {
                    if ray_tri(o, d, t, tmax) {
                        return true;
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = node.a;
                stack[sp + 1] = node.a + 1;
                sp += 2;
            }
        }
        false
    }
}

fn ray_tri(o: Vec3, d: Vec3, t: &[Vec3; 3], tmax: f32) -> bool {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-10 {
        return false;
    }
    let inv = 1.0 / det;
    let s = o - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let tt = e2.dot(q) * inv;
    tt > 1e-4 && tt < tmax
}

/// Bake ambient occlusion (the piece itself plus the ground at y = 0) into vertex colors.
fn bake_ao(parts: &mut [(DestructibleMaterial, MeshBuilder)]) {
    let mut tris = Vec::new();
    for (_, m) in parts.iter() {
        for t in m.indices.as_chunks::<3>().0 {
            tris.push(t.map(|i| Vec3::from(m.vertices[i as usize].position)));
        }
    }
    let bvh = Bvh::new(tris);
    // Cosine-weighted hemisphere directions (Hammersley), z up.
    const NEAR: usize = 10;
    const FAR: usize = 6;
    let dirs: Vec<Vec3> = (0..NEAR + FAR)
        .map(|i| {
            let n = if i < NEAR { NEAR } else { FAR };
            let j = if i < NEAR { i } else { i - NEAR };
            let u = (j as f32 + 0.5) / n as f32;
            let v = ((j as u32).reverse_bits() as f32) / 4_294_967_296.0;
            let r = u.sqrt();
            let a = TAU * (v + if i < NEAR { 0.0 } else { 0.37 });
            Vec3::new(r * a.cos(), r * a.sin(), (1.0 - u).max(0.0).sqrt())
        })
        .collect();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    for (_, m) in parts.iter_mut() {
        let chunk = m.vertices.len().div_ceil(threads).max(256);
        std::thread::scope(|sc| {
            for vs in m.vertices.chunks_mut(chunk) {
                let bvh = &bvh;
                let dirs = &dirs;
                sc.spawn(move || {
                    for v in vs {
                        let n = Vec3::from(v.normal).normalize_or(Vec3::Y);
                        let p = Vec3::from(v.position) + n * 0.012;
                        let t = n.any_orthonormal_vector();
                        let b = n.cross(t);
                        let rot = hash01(
                            (p.x * 97.0) as i64,
                            (p.y * 89.0) as i64,
                            (p.z * 83.0) as i64,
                            7,
                        ) * TAU;
                        let (sr, cr) = rot.sin_cos();
                        let mut near = 0.0;
                        let mut far = 0.0;
                        for (i, d) in dirs.iter().enumerate() {
                            let (x, y) = (d.x * cr - d.y * sr, d.x * sr + d.y * cr);
                            let w = (t * x + b * y + n * d.z).normalize();
                            let tmax = if i < NEAR { 1.3 } else { 7.0 };
                            let hit =
                                (w.y < -1e-4 && p.y / -w.y < tmax) || bvh.occluded(p, w, tmax);
                            if hit {
                                if i < NEAR {
                                    near += 1.0;
                                } else {
                                    far += 1.0;
                                }
                            }
                        }
                        let occ = 0.62 * near / NEAR as f32 + 0.38 * far / FAR as f32;
                        let ao = (1.0 - occ).clamp(0.0, 1.0);
                        let k = 0.3 + 0.7 * ao.powf(1.2);
                        for c in &mut v.color[..3] {
                            *c *= k;
                        }
                    }
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Frames for straight runs of wall
// ---------------------------------------------------------------------------------------------

/// A straight run: local x along the run, y up, z into the wall from the outer face at z = 0.
#[derive(Clone, Copy, Debug)]
struct Frame {
    o: Vec3,
    x: Vec3,
    z: Vec3,
}

impl Frame {
    /// `x` is the run direction (horizontal); the outer face looks toward `x × Y`... i.e.
    /// for `x = +X` the outer face looks toward −Z.
    fn new(o: Vec3, x: Vec3) -> Self {
        let x = Vec3::new(x.x, 0.0, x.z).normalize();
        Self {
            o,
            x,
            z: Vec3::Y.cross(x).normalize() * -1.0,
        }
    }
    fn p(&self, x: f32, y: f32, z: f32) -> Vec3 {
        self.o + self.x * x + Vec3::Y * y + self.z * z
    }
    /// Plane of a vertical face at local depth `z` whose outward normal is local `n`.
    fn vplane(&self, z: f32, outward_local: Vec3) -> Plane {
        let n = self.x * outward_local.x + self.z * outward_local.z;
        Plane::vertical(self.p(0.0, 0.0, z), n)
    }
    /// Plane of a face at local `x` looking along local ±x.
    fn end_plane(&self, x: f32, sign: f32) -> Plane {
        Plane::vertical(self.p(x, 0.0, 0.0), self.x * sign)
    }
    fn dir(&self, l: Vec3) -> Vec3 {
        self.x * l.x + Vec3::Y * l.y + self.z * l.z
    }
    fn box_corners(&self, lo: Vec3, hi: Vec3) -> [Vec3; 8] {
        std::array::from_fn(|i| {
            self.p(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            )
        })
    }
    /// Hidden-face mask stays in local terms (box corners follow the frame).
    fn map_hidden(&self, h: u8) -> u8 {
        h
    }
    /// Local direction of a world direction.
    fn local(&self, d: Vec3) -> Vec3 {
        Vec3::new(d.dot(self.x), d.y, d.dot(self.z))
    }
}

// ---------------------------------------------------------------------------------------------
// Curtain wall
// ---------------------------------------------------------------------------------------------

/// Dimensions of a straight wall run (local frame meters).
#[derive(Clone, Copy, Debug)]
struct WallSpec {
    len: f32,
    h: f32,
    t: f32,
    /// Plinth top and how far its foot stands out.
    plinth: f32,
    batter: f32,
    /// Wall-walk level, crenel sill, merlon top below the coping.
    walk: f32,
    sill: f32,
    merlon: f32,
    /// Parapet projection beyond the face (on corbels) and thickness.
    proj: f32,
    tp: f32,
}

impl WallSpec {
    fn new(len: f32, h: f32, t: f32) -> Self {
        let plinth = snap((h * 0.25).clamp(0.9, 2.7));
        let walk = snap(h - 2.2).max(plinth + 1.35);
        let sill = walk + 0.9;
        Self {
            len,
            h,
            t,
            plinth,
            batter: 0.32 * plinth + 0.08,
            walk,
            sill,
            merlon: h - 0.16,
            proj: 0.42,
            tp: 0.62,
        }
    }
    /// Local z of the parapet's outer and inner faces.
    fn parapet_z(&self) -> (f32, f32) {
        (-self.proj, -self.proj + self.tp)
    }
    /// Underside of the projecting parapet.
    fn soffit(&self) -> f32 {
        self.walk - 0.45
    }
}

impl Kit {
    /// Mark a projectile impact on a face at `c` (outward normal `n`, radius `r`); the
    /// stones broken out of it lie at the foot of the wall.
    fn scar(&mut self, c: Vec3, n: Vec3, r: f32, foot: f32) {
        self.scars.push((c, n, r));
        let side = Vec3::Y.cross(n).normalize_or(Vec3::X);
        for _ in 0..(3.0 + r * 3.0) as usize {
            let out = foot + 0.25 + 1.6 * self.rng.f32();
            let along = (self.rng.f32() - 0.5) * 2.4 * r;
            let sz = 0.12 + 0.28 * self.rng.f32().powi(2);
            let size = Vec3::new(sz * (1.0 + 0.6 * self.rng.f32()), sz * 0.7, sz);
            let p = Vec3::new(c.x, 0.0, c.z) + n * out + side * along + Vec3::Y * size.y * 0.3;
            let q = Quat::from_rotation_y(self.rng.f32() * TAU)
                * Quat::from_rotation_x((self.rng.f32() - 0.5) * 0.6);
            self.rubble_lump(p, size, q);
        }
    }

    /// A full curtain wall run in `fr` (x from −len/2 to len/2). `stairs` adds the inner stair.
    fn curtain(&mut self, fr: &Frame, w: &WallSpec, stairs: bool) {
        let x0 = -w.len * 0.5;
        let x1 = w.len * 0.5;
        let st = Style::ashlar();
        let ends = |_d: Vec3| Edge::Groove;
        // Siege damage: about one impact per 12 m of outer face.
        for _ in 0..(w.len / 12.0).round() as usize {
            let x = x0 + 2.0 + self.rng.f32() * (w.len - 4.0).max(0.0);
            let y = w.plinth + 1.0 + self.rng.f32() * (w.soffit() - w.plinth - 1.8).max(0.0);
            let r = 0.7 + 0.5 * self.rng.f32();
            self.scar(fr.p(x, y, 0.0), -fr.z, r, w.batter);
        }
        // Battered plinth.
        let pl = Plane::battered(fr.p(0.0, w.plinth, 0.0), -fr.z, w.batter, w.plinth);
        let r = pl.rect(fr.p(x0, w.plinth, 0.0), fr.p(x1, 0.0, -w.batter));
        let putlogs = self.putlog_holes(&pl, r, 0.9, 1.5, &[]);
        self.face(&pl, r, ends, &putlogs, st.worn(0.22).missing(0.006));
        self.string_course(fr, x0, x1, w.plinth, false);
        // Outer face up to the corbel table.
        let soff = w.soffit();
        let outer = fr.vplane(0.0, -Vec3::Z);
        let r = outer.rect(fr.p(x0, w.plinth, 0.0), fr.p(x1, soff, 0.0));
        let mut holes = self.wall_slits(fr, &outer, w, x0, x1);
        let putlogs = self.putlog_holes(&outer, r, w.plinth + 1.2, soff - 1.3, &holes);
        holes.extend(putlogs);
        self.face(&outer, r, ends, &holes, st.missing(0.005));
        // Inner face and wall walk.
        let inner = fr.vplane(w.t, Vec3::Z);
        let r = inner.rect(fr.p(x0, 0.0, w.t), fr.p(x1, w.walk, w.t));
        let holes: Vec<Hole> = Vec::new();
        let up = Vec3::Y;
        self.face(
            &inner,
            r,
            move |d| {
                if d.y > 0.7 {
                    Edge::Corner(up)
                } else {
                    Edge::Groove
                }
            },
            &holes,
            st.coarse(),
        );
        let (_, zpi) = w.parapet_z();
        let walk = Plane::horizontal(w.walk, true);
        let r = walk.rect(fr.p(x0, w.walk, zpi - 0.08), fr.p(x1, w.walk, w.t));
        let inz = fr.z;
        self.face(
            &walk,
            r,
            move |d| {
                if d.dot(inz) > 0.7 {
                    Edge::Corner(inz)
                } else {
                    Edge::Groove
                }
            },
            &[],
            st.worn(0.3).coarse(),
        );
        // End faces: one plain quad each (usually buried in a tower or the next run; the
        // texture paints the joints).
        for (x, s) in [(x0, -1.0f32), (x1, 1.0)] {
            let q = [
                fr.p(x, 0.0, 0.0),
                fr.p(x, 0.0, w.t),
                fr.p(x, w.walk, w.t),
                fr.p(x, w.walk, 0.0),
            ];
            let uv = q.map(|p| box_uv(p, fr.x * s));
            quad(&mut self.stone, q, uv, [[1.0; 3]; 4], fr.x * s);
        }
        self.parapet(fr, w, x0, x1, (x0, x1), true, false);
        if stairs && w.len >= 16.0 {
            let x_top = x1 - 6.0;
            self.stair_flight(fr, x_top, w.walk, w.t, 1.35);
        }
    }

    /// Weathering string course along the top of the plinth.
    fn string_course(&mut self, fr: &Frame, x0: f32, x1: f32, y: f32, open_ends: bool) {
        let pieces = spans(x0, x1, BLOCK_W * 2.0, 0.0, 0.5);
        let last = pieces.len().saturating_sub(1);
        for (i, (a, b)) in pieces.into_iter().enumerate() {
            // Cross-section: bottom front, top front, top back (sloped weathering), bottom back.
            let lo = Vec3::new(a, y - 0.24, -0.17);
            let p: [Vec3; 8] = std::array::from_fn(|i| {
                let x = if i & 1 == 0 { a } else { b };
                let top = i & 2 != 0;
                let back = i & 4 != 0;
                let (yy, zz) = match (top, back) {
                    (false, false) => (lo.y, -0.17),
                    (true, false) => (y - 0.05, -0.17),
                    (true, true) => (y + 0.16, 0.12),
                    (false, true) => (lo.y, 0.12),
                };
                fr.p(x, yy, zz)
            });
            let mut hidden = NX | PX;
            if open_ends && i == 0 {
                hidden &= !NX;
            }
            if open_ends && i == last {
                hidden &= !PX;
            }
            self.stone_hexa_e(p, hidden, PZ, JOINT * 1.4, Uvm::World);
        }
    }

    /// Sparse square scaffold (putlog) holes on a face, in rows between `y0` and `y1`.
    fn putlog_holes(
        &mut self,
        s: &dyn Surf,
        r: Rect,
        y0: f32,
        y1: f32,
        avoid: &[Hole],
    ) -> Vec<Hole> {
        let mut holes = Vec::new();
        let mut y = snap(y0) + 0.135;
        let salt = self.next_salt();
        let mut row = 0;
        while y < y1 {
            let mut u = r.u0 + 1.2 + self.hash(row, 0, salt) * 1.0;
            let mut j = 0;
            while u < r.u1 - 1.0 {
                if self.hash(row, j + 1, salt) < 0.4 {
                    let v = -y;
                    let o = Opening {
                        r: Rect::new(u - 0.075, v - 0.08, u + 0.075, v + 0.08),
                        frame: 0.0,
                        depth: 0.32,
                        splay: 0.0,
                    };
                    let clear = !avoid.iter().any(|h| h.r.overlaps(&o.r.grow(0.35)));
                    if clear && o.r.v0 > r.v0 + 0.2 && o.r.v1 < r.v1 - 0.2 {
                        holes.push(self.opening(s, &o, None));
                    }
                }
                u += 1.8 * (1.0 + (self.hash(row, j + 7, salt) * 2.0).floor());
                j += 1;
            }
            y += COURSE_H * 3.0;
            row += 1;
        }
        holes
    }

    /// Arrow slits low in the wall body (lighting casemates behind).
    fn wall_slits(&mut self, _fr: &Frame, s: &Plane, w: &WallSpec, x0: f32, x1: f32) -> Vec<Hole> {
        let mut holes = Vec::new();
        let n = ((x1 - x0) / 9.0).floor() as i32;
        let y = w.plinth + 1.35;
        for i in 0..n {
            let x = x0 + (i as f32 + 0.5) * (x1 - x0) / n as f32;
            let u = s.uv(_fr.p(x, 0.0, 0.0)).x;
            let o = Opening {
                r: Rect::new(u - 0.07, -(y + 1.25), u + 0.07, -y),
                frame: 0.06,
                depth: 0.9,
                splay: 0.35,
            };
            holes.push(self.opening(s, &o, None));
            self.streaks.push((s.pos(u, -y), 0.2));
        }
        holes
    }

    /// Corbel table, projecting parapet with machicolations, merlons, crenels and slits.
    fn parapet(
        &mut self,
        fr: &Frame,
        w: &WallSpec,
        x0: f32,
        x1: f32,
        soffit_x: (f32, f32),
        corbels: bool,
        open_ends: bool,
    ) {
        let st = Style::ashlar();
        let (zpo, zpi) = w.parapet_z();
        let soff = w.soffit();
        // Merlon layout: merlons at both ends.
        let crenel = 0.8;
        let n = (((x1 - x0) - 2.0) / 2.9).round().max(1.0) as usize;
        let mw = ((x1 - x0) - n as f32 * crenel) / (n + 1) as f32;
        let pitch = mw + crenel;
        // Corbels under every merlon and crenel center, machicolation slots between.
        let mut cx = Vec::new();
        let mut c = x0 + mw * 0.5;
        while c < soffit_x.0 + 0.3 {
            c += pitch * 0.5;
        }
        while c < soffit_x.1 - 0.2 {
            cx.push(c);
            c += pitch * 0.5;
        }
        if corbels {
            for &c in &cx {
                self.corbel(fr, c, soff, w.proj);
            }
        }
        // Soffit with machicolation slots.
        let sp = Plane::horizontal(soff, false);
        let r = sp.rect(fr.p(soffit_x.0, soff, zpo), fr.p(soffit_x.1, soff, 0.08));
        let mut holes = Vec::new();
        let pairs = if corbels {
            cx.windows(2).collect()
        } else {
            Vec::new()
        };
        for pair in pairs {
            let (a, b) = (pair[0] + 0.3, pair[1] - 0.3);
            if b - a < 0.25 {
                continue;
            }
            let pa = sp.uv(fr.p(a, soff, zpo + 0.1));
            let pb = sp.uv(fr.p(b, soff, -0.06));
            let o = Opening {
                r: Rect::new(pa.x, pa.y, pb.x, pb.y),
                frame: 0.03,
                depth: 0.55,
                splay: 0.0,
            };
            holes.push(self.opening(&sp, &o, None));
        }
        let out = -fr.z;
        let xd = fr.x;
        if corbels {
            self.face(
                &sp,
                r,
                move |d| {
                    if d.dot(out) > 0.7 {
                        Edge::Corner(out)
                    } else if open_ends && d.dot(xd).abs() > 0.7 {
                        Edge::Corner(d)
                    } else {
                        Edge::Groove
                    }
                },
                &holes,
                st.coarse(),
            );
        }
        // Parapet body: outer face from the soffit to the sill, inner face from the walk.
        let po = fr.vplane(zpo, -Vec3::Z);
        let r = po.rect(fr.p(x0, soff, zpo), fr.p(x1, w.sill, zpo));
        let down = -Vec3::Y;
        // Spouts draining the walk.
        let mut holes = Vec::new();
        let ns = ((x1 - x0) / 10.0).floor().max(1.0) as i32;
        for i in 0..ns {
            let x = x0 + (i as f32 + 0.5) * (x1 - x0) / ns as f32 + 0.45;
            let y = w.walk + 0.12;
            let p0 = fr.p(x - 0.13, y, zpo - 0.55);
            let p1 = fr.p(x + 0.13, y + 0.2, zpo + 0.3);
            self.stone_box(fr, fr.local(p0 - fr.o), fr.local(p1 - fr.o), PZ, 0.025);
            self.streaks.push((fr.p(x, y, zpo - 0.5), 0.28));
            let uv = po.uv(fr.p(x, y + 0.1, zpo));
            holes.push(Hole::rim(Rect::new(
                uv.x - 0.15,
                uv.y - 0.12,
                uv.x + 0.15,
                uv.y + 0.12,
            )));
        }
        self.face(
            &po,
            r,
            move |d| {
                if d.y < -0.7 && corbels {
                    Edge::Corner(down)
                } else if open_ends && d.dot(xd).abs() > 0.7 {
                    Edge::Corner(d)
                } else {
                    Edge::Groove
                }
            },
            &holes,
            st.worn(0.25),
        );
        let pi = fr.vplane(zpi, Vec3::Z);
        let r = pi.rect(fr.p(x0, w.walk, zpi), fr.p(x1, w.sill, zpi));
        self.face(&pi, r, |_| Edge::Groove, &[], st.coarse());
        if open_ends {
            // The parapet ends at a tower corner: faces looking along the run.
            let zd = fr.z;
            for (x, sg) in [(x0, -1.0f32), (x1, 1.0)] {
                let ep = fr.end_plane(x, sg);
                let bottom = if corbels { soff } else { w.walk };
                let r = ep.rect(fr.p(x, bottom, zpo), fr.p(x, w.sill, zpi));
                self.face(
                    &ep,
                    r,
                    move |d| {
                        if d.dot(zd) < -0.7 {
                            Edge::Corner(-zd)
                        } else if d.y < -0.7 && corbels {
                            Edge::Corner(-Vec3::Y)
                        } else {
                            Edge::Groove
                        }
                    },
                    &[],
                    st,
                );
            }
        }
        // Crenel sills and merlons; on the fighting fronts a few merlons are shot down and
        // a few crenels hang timber shutters.
        let broken: Vec<bool> = (0..=n)
            .map(|i| corbels && i > 0 && i < n && self.rng.f32() < 0.12)
            .collect();
        for i in 0..=n {
            let a = x0 + i as f32 * pitch;
            let b = a + mw;
            self.merlon(
                fr,
                w,
                [a, b],
                if corbels {
                    i % 2 == 1 || n < 3
                } else {
                    i % 4 == 2
                },
                i == 0 && !open_ends,
                i == n && !open_ends,
                broken[i],
            );
            if i < n && corbels && !broken[i] && !broken[i + 1] && self.rng.f32() < 0.2 {
                self.crenel_shutter(fr, w, b, b + crenel);
            }
            if i < n {
                // Sloped sill coping in the crenel.
                let (ca, cb) = (b - 0.06, b + crenel + 0.06);
                let p: [Vec3; 8] = std::array::from_fn(|k| {
                    let x = if k & 1 == 0 { ca } else { cb };
                    let top = k & 2 != 0;
                    let back = k & 4 != 0;
                    let z = if back { zpi + 0.05 } else { zpo - 0.06 };
                    let y = match (top, back) {
                        (false, _) => w.sill - 0.02,
                        (true, false) => w.sill + 0.1,
                        (true, true) => w.sill + 0.17,
                    };
                    fr.p(x, y, z)
                });
                self.stone_hexa(p, NX | PX | NY, 0.03, Uvm::World);
                self.streaks
                    .push((fr.p((ca + cb) * 0.5, w.sill, zpo - 0.06), 0.45));
            }
        }
    }

    /// One merlon from the sill to the coping; `slit` pierces it with an arrow slit. A
    /// `broken` merlon is shot down to a ragged stump a course or two above the sill.
    #[allow(clippy::too_many_arguments)]
    fn merlon(
        &mut self,
        fr: &Frame,
        w: &WallSpec,
        [a, b]: [f32; 2],
        slit: bool,
        first: bool,
        last: bool,
        broken: bool,
    ) {
        let (zpo, zpi) = w.parapet_z();
        let st = Style::ashlar().worn(0.35).missing(0.0);
        let front = fr.vplane(zpo, -Vec3::Z);
        let back = fr.vplane(zpi, Vec3::Z);
        let xd = fr.x;
        let zd = fr.z;
        let top = if broken {
            let courses = 1.0 + (self.rng.f32() * 2.0).floor();
            (snap(w.sill) + COURSE_H * courses).min(w.merlon - 0.3)
        } else {
            w.merlon
        };
        let corner_x = move |d: Vec3| {
            if d.dot(xd) > 0.7 {
                Edge::Corner(xd)
            } else if d.dot(xd) < -0.7 {
                Edge::Corner(-xd)
            } else if broken && d.y > 0.7 {
                Edge::Corner(Vec3::Y)
            } else {
                Edge::Groove
            }
        };
        let rf = front.rect(fr.p(a, w.sill, zpo), fr.p(b, top, zpo));
        let rb = back.rect(fr.p(a, w.sill, zpi), fr.p(b, top, zpi));
        let mut hf = Vec::new();
        let mut hb = Vec::new();
        if slit && !broken {
            let m = (a + b) * 0.5;
            let uf = front.uv(fr.p(m, w.sill + 0.2, zpo));
            let ub = back.uv(fr.p(m, w.sill + 0.06, zpi));
            let top_f = w.merlon - 0.2 - (w.sill + 0.2);
            let top_b = w.merlon - 0.1 - (w.sill + 0.06);
            let of = Opening {
                r: Rect::new(uf.x - 0.055, uf.y - top_f, uf.x + 0.055, uf.y),
                frame: 0.05,
                depth: 0.0,
                splay: 0.0,
            };
            let ob = Opening {
                r: Rect::new(ub.x - 0.28, ub.y - top_b, ub.x + 0.28, ub.y),
                frame: 0.04,
                depth: 0.0,
                splay: 0.0,
            };
            hf.push(self.opening(&front, &of, Some((&back, &ob))));
            hb.push(Hole::rim(ob.r.grow(ob.frame)));
            self.streaks.push((fr.p(m, w.sill + 0.25, zpo), 0.12));
        }
        self.face(&front, rf, corner_x, &hf, st);
        self.face(&back, rb, corner_x, &hb, st.coarse());
        for (x, s, skip) in [(a, -1.0f32, first), (b, 1.0, last)] {
            if skip {
                continue;
            }
            let side = fr.end_plane(x, s);
            let r = side.rect(fr.p(x, w.sill, zpo), fr.p(x, top, zpi));
            self.face(
                &side,
                r,
                move |d| {
                    if d.dot(zd) > 0.7 {
                        Edge::Corner(zd)
                    } else if d.dot(zd) < -0.7 {
                        Edge::Corner(-zd)
                    } else if broken && d.y > 0.7 {
                        Edge::Corner(Vec3::Y)
                    } else {
                        Edge::Groove
                    }
                },
                &[],
                st,
            );
        }
        if broken {
            // The shattered top, a few fragments left on it and a block fallen on the walk.
            let tp = Plane::horizontal(top, true);
            let r = tp.rect(fr.p(a, top, zpo), fr.p(b, top, zpi));
            self.face(&tp, r, Edge::Corner, &[], st.worn(0.9));
            for _ in 0..3 {
                let x = a + 0.2 + self.rng.f32() * (b - a - 0.4);
                let z = zpo + 0.15 + self.rng.f32() * (zpi - zpo - 0.3);
                let sz = 0.15 + 0.15 * self.rng.f32();
                let q = Quat::from_rotation_y(self.rng.f32() * TAU);
                self.rubble_lump(
                    fr.p(x, top + sz * 0.2, z),
                    Vec3::new(sz * 1.3, sz * 0.7, sz),
                    q,
                );
            }
            let c = fr.p(
                (a + b) * 0.5 + (self.rng.f32() - 0.5),
                w.walk + 0.2,
                zpi + 0.5 + self.rng.f32() * 0.8,
            );
            let q = Quat::from_rotation_y(self.rng.f32() * TAU) * Quat::from_rotation_x(0.12);
            let p = box_corners(c, Vec3::new(BLOCK_W * 0.4, COURSE_H * 0.5, 0.26), q);
            self.stone_hexa(p, 0, 0.04, Uvm::Local(c, q));
            return;
        }
        // Coping, weathered to throw water onto the walk.
        let p: [Vec3; 8] = std::array::from_fn(|k| {
            let x = if k & 1 == 0 { a - 0.05 } else { b + 0.05 };
            let back = k & 4 != 0;
            let z = if back { zpi + 0.06 } else { zpo - 0.06 };
            let y = if k & 2 == 0 {
                w.merlon - 0.01
            } else if back {
                w.h - 0.06
            } else {
                w.h
            };
            fr.p(x, y, z)
        });
        self.stone_hexa(p, NY, 0.03, Uvm::World);
    }

    /// A timber shutter hung in the crenel between `a` and `b` from an iron bar under the
    /// copings, propped open at the foot.
    fn crenel_shutter(&mut self, fr: &Frame, w: &WallSpec, a: f32, b: f32) {
        let (zpo, _) = w.parapet_z();
        let hinge = w.merlon - 0.12;
        let hgt = hinge - w.sill - 0.14;
        let open = 0.2 + 0.45 * self.rng.f32();
        let z0 = zpo - 0.03;
        let down = Vec3::new(0.0, -open.cos(), -open.sin());
        let across = Vec3::new(0.0, -open.sin(), open.cos());
        let at = |x: f32, s: f32, k: f32| fr.p(x, hinge, z0) + fr.dir(down * s + across * k);
        let (sa, sb) = (a + 0.05, b - 0.05);
        let board: [Vec3; 8] = std::array::from_fn(|i| {
            at(
                if i & 1 == 0 { sa } else { sb },
                if i & 2 == 0 { hgt } else { 0.0 },
                if i & 4 == 0 { -0.05 } else { 0.0 },
            )
        });
        let col = wood_tint(&mut self.rng);
        hexa(&mut self.wood, board, 0, 0.012, col, Uvm::World);
        for s in [0.25, 0.75] {
            let s = s * hgt;
            let batten: [Vec3; 8] = std::array::from_fn(|i| {
                at(
                    if i & 1 == 0 { sa + 0.04 } else { sb - 0.04 },
                    if i & 2 == 0 { s + 0.05 } else { s - 0.05 },
                    if i & 4 == 0 { 0.0 } else { 0.035 },
                )
            });
            let col = wood_tint(&mut self.rng);
            hexa(&mut self.wood, batten, NZ, 0.0, col, Uvm::World);
        }
        let bar = beam_corners(
            fr.p(a - 0.06, hinge + 0.03, z0 + 0.02),
            fr.p(b + 0.06, hinge + 0.03, z0 + 0.02),
            Vec3::Y,
            0.035,
            0.035,
        );
        hexa(&mut self.metal, bar, 0, 0.0, [0.8; 3], Uvm::World);
    }

    /// Three stepped corbel stones under the projecting parapet at local `x`.
    fn corbel(&mut self, fr: &Frame, x: f32, top: f32, proj: f32) {
        let mut y1 = top;
        for (t, &(hgt, rt, rb)) in CORBEL.iter().enumerate() {
            let y0 = y1 - hgt;
            let hw = 0.22 - 0.015 * t as f32;
            let p: [Vec3; 8] = std::array::from_fn(|k| {
                let xx = if k & 1 == 0 { x - hw } else { x + hw };
                let up = k & 2 != 0;
                let z = if k & 4 != 0 {
                    0.3
                } else if up {
                    -proj * rt
                } else {
                    -proj * rb
                };
                fr.p(xx, if up { y1 } else { y0 }, z)
            });
            let (hidden, embedded) = if t == 0 { (PY, PZ) } else { (0, PZ | PY) };
            self.stone_hexa_e(p, hidden, embedded, 0.03, Uvm::World);
            y1 = y0;
        }
    }

    /// Straight stair against the inner face rising toward +x, arriving at `top` at `x_top`.
    fn stair_flight(&mut self, fr: &Frame, x_top: f32, top: f32, z_wall: f32, width: f32) {
        let rise = COURSE_H * 0.5;
        let tread = 0.3;
        let steps = (top / rise).round() as usize;
        let x_b = x_top - steps as f32 * tread;
        let zf = z_wall + width;
        // Support wall face under the flight; rows are cut back under the steps.
        let side = fr.vplane(zf, Vec3::Z);
        let r = side.rect(fr.p(x_b, 0.0, zf), fr.p(x_top, top - 0.9, zf));
        let mut holes = Vec::new();
        let rows = (top / COURSE_H).ceil() as i32;
        for k in 0..rows {
            let cut = x_b + (2 * k + 4) as f32 * tread;
            let ya = k as f32 * COURSE_H;
            let pa = side.uv(fr.p(x_b - 1.0, ya, zf));
            let pb = side.uv(fr.p(cut.min(x_top), ya + COURSE_H, zf));
            holes.push(Hole {
                r: Rect::new(pa.x, pa.y, pb.x, pb.y),
                e: [Edge::Groove; 4],
            });
        }
        let xd = fr.x;
        self.face(
            &side,
            r,
            move |d| {
                if d.dot(xd) > 0.7 {
                    Edge::Corner(xd)
                } else {
                    Edge::Groove
                }
            },
            &holes,
            Style::ashlar().coarse(),
        );
        let endp = fr.end_plane(x_top, 1.0);
        let r = endp.rect(fr.p(x_top, 0.0, z_wall), fr.p(x_top, top, zf));
        let inz = fr.z;
        self.face(
            &endp,
            r,
            move |d| {
                if d.dot(inz) > 0.7 {
                    Edge::Corner(inz)
                } else {
                    Edge::Groove
                }
            },
            &[],
            Style::ashlar(),
        );
        for i in 0..steps {
            let y1 = (i + 1) as f32 * rise;
            let y0 = (y1 - 4.0 * rise).max(0.0);
            let xa = x_b + i as f32 * tread - 0.04;
            let xb = if i + 1 == steps {
                x_top
            } else {
                xa + tread + 0.04
            };
            let lo = Vec3::new(xa, y0, z_wall - 0.05);
            let hi = Vec3::new(xb, y1, zf + 0.035);
            let p = fr.box_corners(lo, hi);
            self.stone_hexa_e(p, PX, NZ | NY, 0.03, Uvm::World);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Towers, roofs, timber work
// ---------------------------------------------------------------------------------------------

/// Corners of an annular-sector stone: x bit = angle (`th`), y bit = height (`y`), z bit =
/// radius (`r[0]` outer, `r[1]` inner); `dy(i)` lifts single corners.
fn sector(c: Vec3, th: [f32; 2], y: [f32; 2], r: [f32; 2], dy: impl Fn(usize) -> f32) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        let t = th[i & 1];
        let rr = r[(i >> 2) & 1];
        c + Vec3::new(rr * t.cos(), y[(i >> 1) & 1] + dy(i), rr * t.sin())
    })
}

/// Corners of a square member from `a` to `b`, `w` wide and `h` tall (h measured along the
/// direction closest to `up`).
fn beam_corners(a: Vec3, b: Vec3, up: Vec3, w: f32, h: f32) -> [Vec3; 8] {
    let d = (b - a).normalize_or(Vec3::X);
    let side = d.cross(up).normalize_or(d.any_orthonormal_vector());
    let up2 = side.cross(d).normalize_or(Vec3::Y);
    std::array::from_fn(|i| {
        let base = if i & 1 == 0 { a } else { b };
        let y = if i & 2 == 0 { -h * 0.5 } else { h * 0.5 };
        let z = if i & 4 == 0 { -w * 0.5 } else { w * 0.5 };
        base + up2 * y + side * z
    })
}

/// Weathered timber color.
fn wood_tint(rng: &mut Rng) -> [f32; 3] {
    let l = 1.0 + rng.range(-0.16, 0.1);
    let g = rng.range(0.0, 0.12);
    [
        l * (1.0 - g * 0.3),
        l * (1.0 - g * 0.15),
        l * (1.0 + g * 0.1),
    ]
}

impl Kit {
    /// Rounded weathering course around a tower at the top of its battered base.
    fn ring_course(&mut self, c: Vec3, r: f32, y: f32, n: usize) {
        let d = TAU / n as f32;
        for j in 0..n {
            let th = [j as f32 * d, (j + 1) as f32 * d];
            let p = sector(c, th, [y - 0.24, y - 0.05], [r + 0.16, r - 0.12], |i| {
                if i & 6 == 6 { 0.21 } else { 0.0 }
            });
            self.stone_hexa_e(p, NX | PX, PZ, JOINT * 1.4, Uvm::World);
        }
    }

    fn round_tower(&mut self, c: Vec3, r: f32, h: f32, top: TowerTop, batter_scale: f32) {
        let st = Style::ashlar();
        let n = ((TAU * r / BLOCK_W).round() as usize).max(8);
        let k = n as f32 * BLOCK_W / TAU;
        let full = n as f32 * BLOCK_W;
        let yb = snap((h * 0.2).clamp(1.35, 3.15));
        let batter = (0.26 * yb + 0.2) * batter_scale;
        let base = Cyl {
            c,
            y0: 0.0,
            r0: r + batter,
            y1: yb,
            r1: r,
            k,
            inward: false,
        };
        let rect = Rect::new(0.0, -yb, full, 0.0);
        let holes = self.putlog_holes(&base, rect, 0.9, yb - 0.3, &[]);
        self.face_e(
            &base,
            rect,
            [Edge::Groove; 4],
            &holes,
            st.wrap(n).worn(0.22).missing(0.006),
        );
        self.ring_course(c, r, yb, n);
        let walk = snap(h - 2.2);
        let roofed = matches!(top, TowerTop::SlateRoof | TowerTop::ShingleRoof);
        let proj = if top == TowerTop::Crenellated {
            0.42
        } else {
            0.0
        };
        let shaft_top = if roofed {
            h - 0.3
        } else if proj > 0.0 {
            walk - 0.45
        } else {
            walk + 0.9
        };
        let shaft = Cyl::new(c, r, k);
        let rect = Rect::new(0.0, -shaft_top, full, -yb);
        let mut holes = Vec::new();
        let phase = self.rng.f32() * TAU;
        let levels = (((shaft_top - yb - 2.2) / 3.6).floor() as i32).max(1);
        for l in 0..levels {
            let y = yb + 1.3 + l as f32 * 3.6;
            let window = roofed && l == levels - 1 && l > 0;
            for i in 0..4 {
                let th = phase + (i as f32 + 0.5 * (l % 2) as f32) * TAU / 4.0;
                let u = (th * k).rem_euclid(full);
                let o = if window {
                    Opening {
                        r: Rect::new(u - 0.3, -(y + 1.15), u + 0.3, -y),
                        frame: 0.08,
                        depth: 0.45,
                        splay: 0.0,
                    }
                } else {
                    Opening {
                        r: Rect::new(u - 0.065, -(y + 1.3), u + 0.065, -y),
                        frame: 0.06,
                        depth: 0.85,
                        splay: 0.3,
                    }
                };
                holes.push(self.opening(&shaft, &o, None));
                self.streaks
                    .push((shaft.pos(u, -y), if window { 0.4 } else { 0.18 }));
            }
        }
        let beams: Vec<f32> = if top == TowerTop::Hoarding {
            let nb = ((TAU * r / 1.8).round() as usize).max(8);
            (0..nb)
                .map(|j| (j as f32 + 0.5) * TAU / nb as f32)
                .collect()
        } else {
            Vec::new()
        };
        let beam_y = walk - 0.6;
        for &th in &beams {
            let u = th * k;
            let o = Opening {
                r: Rect::new(u - 0.13, -(beam_y + 0.14), u + 0.13, -(beam_y - 0.14)),
                frame: 0.0,
                depth: 0.4,
                splay: 0.0,
            };
            holes.push(self.opening(&shaft, &o, None));
        }
        if self.rng.f32() < 0.7 {
            let th = self.rng.f32() * TAU;
            let rad = Vec3::new(th.cos(), 0.0, th.sin());
            let y = yb + 1.2 + self.rng.f32() * (shaft_top - yb - 2.4).max(0.0);
            let rs = 0.65 + 0.4 * self.rng.f32();
            self.scar(c + rad * r + Vec3::Y * y, rad, rs, batter);
        }
        self.face_e(&shaft, rect, [Edge::Groove; 4], &holes, st.wrap(n));
        match top {
            TowerTop::Crenellated => self.round_parapet(c, r, h, walk, 0.42, true),
            TowerTop::Hoarding => {
                self.round_parapet(c, r, h, walk, 0.0, false);
                self.hoarding(c, r, &beams, beam_y);
            }
            TowerTop::SlateRoof | TowerTop::ShingleRoof => {
                self.cornice(c, r, h - 0.12);
                let re = r + 0.62;
                self.cone_roof(
                    c,
                    r - 0.3,
                    re,
                    h - 0.02,
                    2.3 * re,
                    top == TowerTop::SlateRoof,
                );
            }
        }
    }

    /// Stepped corbel under a round tower's projecting parapet at angle `th`.
    fn corbel_radial(&mut self, c: Vec3, r: f32, th: f32, top: f32, proj: f32, scale: f32) {
        let mut y1 = top;
        for (t, &(hgt, rt, rb)) in CORBEL.iter().enumerate() {
            let y0 = y1 - hgt * scale;
            let hw = (0.22 - 0.015 * t as f32) * scale.sqrt() / r;
            let p: [Vec3; 8] = std::array::from_fn(|k| {
                let a = if k & 1 == 0 { th - hw } else { th + hw };
                let up = k & 2 != 0;
                let rr = if k & 4 != 0 {
                    r - 0.3
                } else if up {
                    r + proj * rt
                } else {
                    r + proj * rb
                };
                c + Vec3::new(rr * a.cos(), if up { y1 } else { y0 }, rr * a.sin())
            });
            let (hidden, embedded) = if t == 0 { (PY, PZ) } else { (0, PZ | PY) };
            self.stone_hexa_e(p, hidden, embedded, 0.03 * scale.sqrt(), Uvm::World);
            y1 = y0;
        }
    }

    /// Round parapet: optional corbel table and machicolations (`proj` > 0), merlons and
    /// crenel sills (or, without `merlons`, a coped breastwork), the inner face and the paved
    /// platform.
    fn round_parapet(&mut self, c: Vec3, r: f32, h: f32, walk: f32, proj: f32, merlons: bool) {
        let st = Style::ashlar();
        let sill = walk + 0.9;
        let merlon = h - 0.16;
        let soffit = walk - 0.45;
        let ro = r + proj;
        let ri = ro - 0.6;
        let ring_n = |rad: f32| ((TAU * rad / BLOCK_W).round() as usize).max(8);
        let (no, ni) = (ring_n(ro), ring_n(ri));
        let (ko, ki) = (no as f32 * BLOCK_W / TAU, ni as f32 * BLOCK_W / TAU);
        if proj > 0.0 {
            let nc = ((TAU * (r + 0.1) / 1.6).round() as usize).max(8);
            let dth = TAU / nc as f32;
            for j in 0..nc {
                self.corbel_radial(c, r, j as f32 * dth, soffit, proj, 1.0);
            }
            let nr = ring_n(r + proj * 0.5);
            let kr = nr as f32 * BLOCK_W / TAU;
            let ring = Ring {
                c,
                y: soffit,
                r,
                k: kr,
                up: false,
            };
            let mut holes = Vec::new();
            for j in 0..nc {
                let a = j as f32 * dth + 0.3 / r;
                let b = (j + 1) as f32 * dth - 0.3 / r;
                if (b - a) * r < 0.25 {
                    continue;
                }
                let o = Opening {
                    r: Rect::new(a * kr, 0.07, b * kr, proj - 0.1),
                    frame: 0.03,
                    depth: 0.55,
                    splay: 0.0,
                };
                holes.push(self.opening(&ring, &o, None));
            }
            self.face_e(
                &ring,
                Rect::new(0.0, 0.0, nr as f32 * BLOCK_W, proj),
                [
                    Edge::Groove,
                    Edge::Groove,
                    Edge::Groove,
                    Edge::Radial(c.x, c.z, 1.0),
                ],
                &holes,
                st.wrap(nr).coarse(),
            );
            let po = Cyl::new(c, ro, ko);
            self.face_e(
                &po,
                Rect::new(0.0, -sill, no as f32 * BLOCK_W, -soffit),
                [
                    Edge::Groove,
                    Edge::Groove,
                    Edge::Groove,
                    Edge::Corner(-Vec3::Y),
                ],
                &[],
                st.wrap(no).worn(0.25),
            );
        }
        let pi = Cyl {
            inward: true,
            ..Cyl::new(c, ri, ki)
        };
        self.face_e(
            &pi,
            Rect::new(0.0, -sill, ni as f32 * BLOCK_W, -walk),
            [Edge::Groove; 4],
            &[],
            st.wrap(ni).coarse(),
        );
        let floor = Disk {
            c,
            y: walk,
            rmax: ri + 0.12,
        };
        let e = ri + 0.3;
        self.face_e(
            &floor,
            Rect::new(c.x - e, c.z - e, c.x + e, c.z + e),
            [Edge::Groove; 4],
            &[],
            st.worn(0.3).coarse(),
        );
        if !merlons {
            let ns = ((TAU * ro / 1.8).round() as usize).max(6);
            for j in 0..ns {
                let th = [j as f32 * TAU / ns as f32, (j + 1) as f32 * TAU / ns as f32];
                let p = sector(
                    c,
                    th,
                    [sill - 0.02, sill + 0.1],
                    [ro + 0.06, ri - 0.05],
                    |i| {
                        if i & 6 == 6 { 0.07 } else { 0.0 }
                    },
                );
                self.stone_hexa(p, NX | PX | NY, 0.03, Uvm::World);
            }
            return;
        }
        let nm = ((TAU * ro / 2.9).round() as usize).max(4);
        let alpha = TAU / nm as f32;
        let cren = 0.8 / ro;
        let ph = self.rng.f32() * alpha;
        for i in 0..nm {
            let ta = ph + i as f32 * alpha + cren * 0.5;
            let tb = ph + (i + 1) as f32 * alpha - cren * 0.5;
            self.round_merlon(
                c,
                [ro, ri],
                [ko, ki],
                [ta, tb],
                [sill, merlon, h],
                i % 2 == 0,
            );
            let (a, b) = (tb - 0.06 / ro, tb + cren + 0.06 / ro);
            let p = sector(
                c,
                [a, b],
                [sill - 0.02, sill + 0.1],
                [ro + 0.06, ri - 0.05],
                |i| {
                    if i & 6 == 6 { 0.07 } else { 0.0 }
                },
            );
            self.stone_hexa(p, NX | PX | NY, 0.03, Uvm::World);
            let m = (a + b) * 0.5;
            self.streaks.push((
                c + Vec3::new((ro + 0.06) * m.cos(), sill, (ro + 0.06) * m.sin()),
                0.4,
            ));
        }
    }

    fn round_merlon(
        &mut self,
        c: Vec3,
        [ro, ri]: [f32; 2],
        [ko, ki]: [f32; 2],
        [ta, tb]: [f32; 2],
        [sill, merlon, h]: [f32; 3],
        slit: bool,
    ) {
        let st = Style::ashlar().worn(0.35).missing(0.0);
        let front = Cyl::new(c, ro, ko);
        let back = Cyl {
            inward: true,
            ..Cyl::new(c, ri, ki)
        };
        let na = Vec3::new(ta.sin(), 0.0, -ta.cos());
        let nb = Vec3::new(-tb.sin(), 0.0, tb.cos());
        let rf = Rect::new(ta * ko, -merlon, tb * ko, -sill);
        let rb = Rect::new(ta * ki, -merlon, tb * ki, -sill);
        let mut hf = Vec::new();
        let mut hb = Vec::new();
        if slit {
            let m = (ta + tb) * 0.5;
            let hgt_f = merlon - 0.2 - (sill + 0.2);
            let hgt_b = merlon - 0.1 - (sill + 0.06);
            let of = Opening {
                r: Rect::new(
                    m * ko - 0.055,
                    -(sill + 0.2 + hgt_f),
                    m * ko + 0.055,
                    -(sill + 0.2),
                ),
                frame: 0.05,
                depth: 0.0,
                splay: 0.0,
            };
            let ob = Opening {
                r: Rect::new(
                    m * ki - 0.26,
                    -(sill + 0.06 + hgt_b),
                    m * ki + 0.26,
                    -(sill + 0.06),
                ),
                frame: 0.04,
                depth: 0.0,
                splay: 0.0,
            };
            hf.push(self.opening(&front, &of, Some((&back, &ob))));
            hb.push(Hole::rim(ob.r.grow(ob.frame)));
        }
        let e = [
            Edge::Corner(na),
            Edge::Corner(nb),
            Edge::Groove,
            Edge::Groove,
        ];
        self.face_e(&front, rf, e, &hf, st);
        self.face_e(&back, rb, e, &hb, st.coarse());
        for (t, n) in [(ta, na), (tb, nb)] {
            let rad = Vec3::new(t.cos(), 0.0, t.sin());
            let side = Plane::vertical(c + rad * ro, n);
            let r = side.rect(
                c + rad * ro + Vec3::Y * sill,
                c + rad * ri + Vec3::Y * merlon,
            );
            self.face(
                &side,
                r,
                move |d| {
                    if d.dot(rad) > 0.7 {
                        Edge::Corner(rad)
                    } else if d.dot(rad) < -0.7 {
                        Edge::Corner(-rad)
                    } else {
                        Edge::Groove
                    }
                },
                &[],
                st,
            );
        }
        let segs = ((tb - ta) * ro).ceil().max(1.0) as usize;
        for sg in 0..segs {
            let a =
                ta + (tb - ta) * sg as f32 / segs as f32 - if sg == 0 { 0.05 / ro } else { 0.0 };
            let b = ta
                + (tb - ta) * (sg + 1) as f32 / segs as f32
                + if sg + 1 == segs { 0.05 / ro } else { 0.0 };
            let p = sector(
                c,
                [a, b],
                [merlon - 0.01, h - 0.06],
                [ro + 0.05, ri - 0.05],
                |i| {
                    if i & 6 == 6 { 0.06 } else { 0.0 }
                },
            );
            let mut hid = NY;
            if sg > 0 {
                hid |= NX;
            }
            if sg + 1 < segs {
                hid |= PX;
            }
            self.stone_hexa(p, hid, 0.03, Uvm::World);
        }
    }

    /// Corbelled cornice carrying a roof at the top of a round tower.
    fn cornice(&mut self, c: Vec3, r: f32, h: f32) {
        let n = ((TAU * r / 1.35).round() as usize).max(8);
        for j in 0..n {
            self.corbel_radial(c, r, j as f32 * TAU / n as f32, h - 0.2, 0.3, 0.55);
        }
        let nb = ((TAU * r / BLOCK_W).round() as usize).max(8);
        let d = TAU / nb as f32;
        for j in 0..nb {
            let p = sector(
                c,
                [j as f32 * d, (j + 1) as f32 * d],
                [h - 0.2, h + 0.12],
                [r + 0.32, r - 0.3],
                |_| 0.0,
            );
            self.stone_hexa(p, NX | PX | PZ | PY, 0.03, Uvm::World);
        }
    }

    /// Conical roof of overlapping slates (rock) or shingles (wood) from radius `re` at height
    /// `ye` rising `ah`, over a wall of radius `r_wall`, with a lead cap and an iron finial.
    fn cone_roof(&mut self, c: Vec3, r_wall: f32, re: f32, ye: f32, ah: f32, slate: bool) {
        let l = (re * re + ah * ah).sqrt();
        let (ex, w, thick) = if slate {
            (0.3, 0.42, 0.024)
        } else {
            (0.32, 0.4, 0.032)
        };
        let cone = |th: f32, s: f32, lift: f32| -> (Vec3, Vec3) {
            let rr = re * (1.0 - s / l);
            let n = Vec3::new(ah * th.cos(), re, ah * th.sin()).normalize();
            (
                c + Vec3::new(rr * th.cos(), ye + ah * s / l, rr * th.sin()) + n * lift,
                n,
            )
        };
        let mut rng = self.rng.fork(77);
        let mb = if slate {
            &mut self.rock
        } else {
            &mut self.wood
        };
        // Dark underlay (boards seen through gaps) and the eave soffit.
        let segs = 48;
        let rings = 8;
        let dark = [0.2, 0.19, 0.18];
        for i in 0..rings {
            let s0 = l * i as f32 / rings as f32;
            let s1 = l * (i + 1) as f32 / rings as f32;
            for j in 0..segs {
                let t0 = j as f32 * TAU / segs as f32;
                let t1 = (j + 1) as f32 * TAU / segs as f32;
                let (a, n) = cone(t0, s0, 0.0);
                let (b, _) = cone(t1, s0, 0.0);
                let (cc, _) = cone(t1, s1, 0.0);
                let (d, _) = cone(t0, s1, 0.0);
                if i + 1 == rings {
                    tri(mb, [a, b, cc], [Vec2::ZERO; 3], [dark; 3], n);
                } else {
                    quad(mb, [a, b, cc, d], [Vec2::ZERO; 4], [dark; 4], n);
                }
            }
        }
        for j in 0..segs {
            let t0 = j as f32 * TAU / segs as f32;
            let t1 = (j + 1) as f32 * TAU / segs as f32;
            let at = |t: f32, rr: f32| c + Vec3::new(rr * t.cos(), ye - 0.01, rr * t.sin());
            let q = [at(t0, r_wall), at(t1, r_wall), at(t1, re), at(t0, re)];
            let uv = q.map(|p| Vec2::new(p.x, p.z));
            quad(mb, q, uv, [mulc(dark, 1.8); 4], -Vec3::Y);
        }
        let rows = ((l - 0.9) / ex).floor().max(1.0) as usize;
        for i in 0..rows {
            let s0 = i as f32 * ex;
            let s1 = s0 + ex + 0.1;
            let rr = re * (1.0 - s0 / l);
            let m = ((TAU * rr / w).round() as usize).max(6);
            let ph = if i % 2 == 0 { 0.0 } else { 0.5 } + rng.range(-0.08, 0.08);
            let dt = TAU / m as f32;
            let gap = 0.007 / rr.max(0.3);
            for j in 0..m {
                if rng.chance(0.004) {
                    continue;
                }
                let ta = (j as f32 + ph) * dt + gap;
                let tb = (j as f32 + 1.0 + ph) * dt - gap;
                let s0j = s0 + rng.range(-0.025, 0.02);
                let lift = thick * 1.7 + rng.range(-0.008, 0.012);
                let lift2 = thick * 1.7 + rng.range(-0.008, 0.012);
                let (p0, n) = cone(ta, s0j, lift);
                let (p1, _) = cone(tb, s0j, lift2);
                let (p2, _) = cone(tb, s1, thick * 0.4);
                let (p3, _) = cone(ta, s1, thick * 0.4);
                let lum = 1.0 + rng.range(-0.18, 0.12);
                let hue = rng.range(-0.06, 0.06);
                let col = if slate {
                    [lum * (1.0 - hue), lum, lum * (1.0 + hue * 1.5)]
                } else {
                    let g = rng.range(0.0, 0.25);
                    [
                        lum * (1.0 - g * 0.25),
                        lum * (1.0 - g * 0.12),
                        lum * (1.0 + g * 0.05),
                    ]
                };
                let uv = [
                    Vec2::new(ta * rr, s0),
                    Vec2::new(tb * rr, s0),
                    Vec2::new(tb * rr, s1),
                    Vec2::new(ta * rr, s1),
                ];
                quad(mb, [p0, p1, p2, p3], uv, [col; 4], n);
            }
        }
        // Lead cap over the top rows and the finial.
        let s_cap = rows as f32 * ex - 0.05;
        let lead = [0.75, 0.78, 0.8];
        let mbm = &mut self.metal;
        let n_cap = 16;
        for j in 0..n_cap {
            let t0 = j as f32 * TAU / n_cap as f32;
            let t1 = (j + 1) as f32 * TAU / n_cap as f32;
            let (a, n) = cone(t0, s_cap, thick * 2.5);
            let (b, _) = cone(t1, s_cap, thick * 2.5);
            let (apex, _) = cone(t0, l, 0.0);
            tri(mbm, [a, b, apex], [Vec2::ZERO; 3], [lead; 3], n);
            let (a2, _) = cone(t0, s_cap, 0.0);
            let (b2, _) = cone(t1, s_cap, 0.0);
            quad(
                mbm,
                [a2, b2, b, a],
                [Vec2::ZERO; 4],
                [lead; 4],
                -n + Vec3::NEG_Y,
            );
        }
        self.finial(c + Vec3::Y * (ye + ah - 0.25), 2.4);
    }

    /// Iron spike with a ball and a weathervane pennant.
    fn finial(&mut self, base: Vec3, height: f32) {
        let mb = &mut self.metal;
        let col = [0.9, 0.9, 0.9];
        let top = base + Vec3::Y * height;
        hexa(
            mb,
            beam_corners(base, top, Vec3::X, 0.05, 0.05),
            0,
            0.012,
            col,
            Uvm::World,
        );
        for (y, s) in [(0.45, 0.1), (0.62, 0.06)] {
            let b = base + Vec3::Y * (height * y);
            hexa(
                mb,
                box_corners(b, Vec3::splat(s), Quat::from_rotation_y(0.4)),
                0,
                s * 0.45,
                col,
                Uvm::World,
            );
        }
        let f0 = top - Vec3::Y * 0.12;
        let p: [Vec3; 8] = std::array::from_fn(|i| {
            let x = if i & 1 == 0 { 0.0 } else { 0.72 };
            let tip = i & 1 != 0;
            let y = match (i & 2 != 0, tip) {
                (false, false) => -0.36,
                (true, false) => 0.0,
                (false, true) => -0.24,
                (true, true) => -0.1,
            };
            let z = if i & 4 == 0 { -0.006 } else { 0.006 };
            f0 + Vec3::new(x, y, z)
        });
        hexa(mb, p, 0, 0.003, col, Uvm::World);
    }

    /// Timber hoarding around a round tower: beams through the wall at `beam_y`, braces,
    /// floor, a plank wall with shuttered openings and a shingle roof over it all.
    fn hoarding(&mut self, c: Vec3, r: f32, beams: &[f32], beam_y: f32) {
        let out = 1.5;
        let ro = r + out;
        let fy = beam_y + 0.14 + 0.07;
        let wall_h = 2.35;
        let mut rng = self.rng.fork(91);
        let mb = &mut self.wood;
        for &th in beams {
            let rad = Vec3::new(th.cos(), 0.0, th.sin());
            let tan = Vec3::new(-th.sin(), 0.0, th.cos());
            let a = c + rad * (r - 0.35) + Vec3::Y * beam_y;
            let b = c + rad * (ro + 0.2) + Vec3::Y * beam_y;
            hexa(
                mb,
                beam_corners(a, b, Vec3::Y, 0.2, 0.24),
                0,
                0.02,
                wood_tint(&mut rng),
                Uvm::World,
            );
            let s0 = c + rad * (ro - 0.3) + Vec3::Y * (beam_y - 0.12);
            let s1 = c + rad * (r - 0.05) + Vec3::Y * (beam_y - 1.7);
            hexa(
                mb,
                beam_corners(s1, s0, tan, 0.16, 0.16),
                0,
                0.02,
                wood_tint(&mut rng),
                Uvm::World,
            );
            let p0 = c + rad * (ro - 0.12) + Vec3::Y * (fy - 0.04);
            hexa(
                mb,
                beam_corners(p0, p0 + Vec3::Y * wall_h, rad, 0.16, 0.16),
                0,
                0.02,
                wood_tint(&mut rng),
                Uvm::World,
            );
        }
        // Floor boards.
        let nf = ((TAU * (r + out * 0.5) / 0.3).round() as usize).max(8);
        for j in 0..nf {
            let gap = 0.006 / r;
            let th = [
                j as f32 * TAU / nf as f32 + gap,
                (j + 1) as f32 * TAU / nf as f32 - gap,
            ];
            let p = sector(c, th, [fy - 0.06, fy], [ro, r - 0.05], |_| 0.0);
            hexa(mb, p, PZ | NX | PX, 0.0, wood_tint(&mut rng), Uvm::World);
        }
        // Sill and head plates between the posts.
        let nb = beams.len();
        for j in 0..nb {
            let t0 = beams[j];
            let t1 = if j + 1 < nb {
                beams[j + 1]
            } else {
                beams[0] + TAU
            };
            for (y0, y1) in [(fy, fy + 0.16), (fy + wall_h - 0.18, fy + wall_h)] {
                let p = sector(c, [t0, t1], [y0, y1], [ro - 0.04, ro - 0.2], |_| 0.0);
                hexa(mb, p, NX | PX, 0.0, wood_tint(&mut rng), Uvm::World);
            }
        }
        // Plank wall with openings and propped shutters.
        let np = ((TAU * ro / 0.3).round() as usize).max(12);
        let dt = TAU / np as f32;
        for j in 0..np {
            let gap = 0.005 / ro;
            let th = [j as f32 * dt + gap, (j + 1) as f32 * dt - gap];
            let bay = j % 9;
            let window = (j / 9) % 2 == 1 && (2..=5).contains(&bay);
            let col = wood_tint(&mut rng);
            let jag = rng.range(-0.04, 0.03);
            let pieces: &[(f32, f32)] = if window {
                &[(fy - 0.18, fy + 0.95), (fy + 1.6, fy + wall_h + 0.04)]
            } else {
                &[(fy - 0.18, fy + wall_h + 0.04)]
            };
            for &(y0, y1) in pieces {
                let p = sector(c, th, [y0 + jag * 0.5, y1], [ro + 0.04, ro], |_| 0.0);
                hexa(mb, p, PY | NX | PX, 0.0, col, Uvm::World);
            }
            if window && bay == 2 {
                // Shutter hinged at the head of the opening, propped open outward.
                let t0 = th[0];
                let t1 = t0 + 4.0 * dt;
                let tm = (t0 + t1) * 0.5;
                let rad = Vec3::new(tm.cos(), 0.0, tm.sin());
                let hinge = fy + 1.6;
                let open = 0.9f32;
                let p: [Vec3; 8] = std::array::from_fn(|i| {
                    let t = if i & 1 == 0 { t0 } else { t1 };
                    let along = if i & 2 == 0 { 0.65 } else { 0.0 };
                    let thick = if i & 4 == 0 { 0.05 } else { 0.0 };
                    let base = c + Vec3::new((ro + 0.05) * t.cos(), hinge, (ro + 0.05) * t.sin());
                    base + rad * (along * open.sin() + thick) - Vec3::Y * (along * open.cos())
                });
                hexa(mb, p, 0, 0.0, wood_tint(&mut rng), Uvm::World);
            }
        }
        let re = ro + 0.35;
        self.cone_roof(c, r - 0.3, re, fy + wall_h, 1.45 * re, false);
    }

    /// A square tower with battered base, string course, slits and a machicolated
    /// crenellated parapet whose corners belong to the south and north runs.
    fn square_tower(&mut self, c: Vec3, size: f32, h: f32) {
        let st = Style::ashlar();
        let hs = size * 0.5;
        let yb = snap((h * 0.2).clamp(1.35, 3.15));
        let batter = 0.26 * yb + 0.2;
        let mut w = WallSpec::new(size, h, size);
        w.plinth = yb;
        w.batter = batter;
        let soffit = w.soffit();
        let sides = [
            (Vec3::new(0.0, 0.0, -hs), Vec3::X),
            (Vec3::new(hs, 0.0, 0.0), Vec3::Z),
            (Vec3::new(0.0, 0.0, hs), -Vec3::X),
            (Vec3::new(-hs, 0.0, 0.0), -Vec3::Z),
        ];
        let frames: Vec<Frame> = sides.iter().map(|&(o, x)| Frame::new(c + o, x)).collect();
        let tilted = |fr: &Frame| Plane::battered(fr.p(0.0, yb, 0.0), -fr.z, batter, yb).n;
        for (i, fr) in frames.iter().enumerate() {
            let prev = frames[(i + 3) % 4];
            let next = frames[(i + 1) % 4];
            let xd = fr.x;
            let (np, nn) = (tilted(&prev), tilted(&next));
            let pl = Plane::battered(fr.p(0.0, yb, 0.0), -fr.z, batter, yb);
            let uc = pl.uv(fr.p(0.0, yb, 0.0)).x;
            let tz = Trapezoid {
                plane: pl,
                uc,
                half_top: hs,
                half_bottom: hs + batter,
                h: yb,
            };
            let r = Rect::new(uc - hs, -yb, uc + hs, 0.0);
            let corner = move |d: Vec3| {
                if d.dot(xd) > 0.7 {
                    Edge::Corner(nn)
                } else if d.dot(xd) < -0.7 {
                    Edge::Corner(np)
                } else {
                    Edge::Groove
                }
            };
            let holes = self.putlog_holes(&tz, r, 0.9, yb - 0.3, &[]);
            self.face(&tz, r, corner, &holes, st.worn(0.22).missing(0.006));
            if i % 2 == 0 {
                self.string_course(fr, -hs - 0.17, hs + 0.17, yb, true);
            } else {
                self.string_course(fr, -hs + 0.12, hs - 0.12, yb, false);
            }
            // Shaft.
            let sp = fr.vplane(0.0, -Vec3::Z);
            let r = sp.rect(fr.p(-hs, yb, 0.0), fr.p(hs, soffit, 0.0));
            let mut holes = Vec::new();
            let levels = (((soffit - yb - 2.2) / 3.6).floor() as i32).max(1);
            for l in 0..levels {
                let y = yb + 1.3 + l as f32 * 3.6;
                for xs in [-0.25f32, 0.25] {
                    let x = xs * size + if l % 2 == 1 { 0.45 } else { 0.0 };
                    let u = sp.uv(fr.p(x, y, 0.0)).x;
                    let o = Opening {
                        r: Rect::new(u - 0.065, -(y + 1.3), u + 0.065, -y),
                        frame: 0.06,
                        depth: 0.85,
                        splay: 0.3,
                    };
                    holes.push(self.opening(&sp, &o, None));
                    self.streaks.push((fr.p(x, y, 0.0), 0.18));
                }
            }
            let putlogs = self.putlog_holes(&sp, r, yb + 1.0, soffit - 1.4, &holes);
            holes.extend(putlogs);
            let (op, on) = (-prev.z, -next.z);
            self.face(
                &sp,
                r,
                move |d| {
                    if d.dot(xd) > 0.7 {
                        Edge::Corner(on)
                    } else if d.dot(xd) < -0.7 {
                        Edge::Corner(op)
                    } else {
                        Edge::Groove
                    }
                },
                &holes,
                st,
            );
            let (_, zpi) = w.parapet_z();
            let reach = hs + w.proj;
            if i % 2 == 0 {
                self.parapet(fr, &w, -reach, reach, (-reach, reach), true, true);
            } else {
                let inner = reach - w.tp;
                self.parapet(fr, &w, -inner, inner, (-hs, hs), true, false);
            }
            let _ = zpi;
        }
        // Platform.
        let inner = hs + w.proj - w.tp + 0.08;
        let fl = Plane::horizontal(w.walk, true);
        let r = fl.rect(
            c + Vec3::new(-inner, w.walk, -inner),
            c + Vec3::new(inner, w.walk, inner),
        );
        self.face(&fl, r, |_| Edge::Groove, &[], st.worn(0.3).coarse());
    }
}

// ---------------------------------------------------------------------------------------------
// Gatehouse
// ---------------------------------------------------------------------------------------------

/// Pointed (two-centred) arch profile, or round when `rho == s`: half span `s`, springing
/// height `ys`, arc radius `rho`.
#[derive(Clone, Copy, Debug)]
struct Arch {
    s: f32,
    ys: f32,
    rho: f32,
}

impl Arch {
    /// Angle of each arc at the apex.
    fn phi_apex(&self) -> f32 {
        ((self.rho - self.s) / self.rho).clamp(-1.0, 1.0).acos()
    }
    fn apex(&self) -> f32 {
        self.ys + self.rho * self.phi_apex().sin()
    }
    /// Center x of the left (`side` −1) or right (+1) arc.
    fn cx(&self, side: f32) -> f32 {
        -side * (self.rho - self.s)
    }
    /// Point at angle `phi` up the left or right arc, `d` beyond the intrados.
    fn at(&self, side: f32, phi: f32, d: f32) -> Vec2 {
        let r = self.rho + d;
        Vec2::new(
            self.cx(side) + side * r * phi.cos(),
            self.ys + r * phi.sin(),
        )
    }
    /// Half width of the opening at height `y`.
    fn half_width(&self, y: f32) -> f32 {
        if y <= self.ys {
            return self.s;
        }
        let h = y - self.ys;
        let top = self.rho * self.phi_apex().sin();
        if h >= top {
            return 0.0;
        }
        (self.rho * self.rho - h * h).sqrt() - (self.rho - self.s)
    }
    /// Arc length of one side.
    fn side_len(&self) -> f32 {
        self.rho * self.phi_apex()
    }
    /// Point and normal (toward the opening) at arc length `t` from the left springing.
    fn profile(&self, t: f32) -> (Vec2, Vec2) {
        let la = self.side_len();
        let t = t.clamp(0.0, 2.0 * la);
        let (side, phi) = if t <= la {
            (-1.0, t / self.rho)
        } else {
            (1.0, (2.0 * la - t) / self.rho)
        };
        let p = self.at(side, phi, 0.0);
        let n = (Vec2::new(self.cx(side), self.ys) - p).normalize_or(-Vec2::Y);
        (p, n)
    }
}

/// The intrados of a barrel vault along z: u = z, v = arc length over the profile.
struct Vault {
    a: Arch,
    x0: f32,
}

impl Surf for Vault {
    fn pos(&self, u: f32, v: f32) -> Vec3 {
        let (p, _) = self.a.profile(v);
        Vec3::new(self.x0 + p.x, p.y, u)
    }
    fn normal(&self, _u: f32, v: f32) -> Vec3 {
        let (_, n) = self.a.profile(v);
        Vec3::new(n.x, n.y, 0.0)
    }
}

/// A hole whose sides are convex corners with the faces lining it.
fn corner_hole(s: &dyn Surf, r: Rect) -> Hole {
    let d = side_dirs(s, &r);
    Hole {
        r,
        e: d.map(|x| Edge::Corner(-x)),
    }
}

/// A twin-towered gatehouse: `width` across the flanking towers (above their bases),
/// `height` to the tops of the tower merlons, `depth` the length of the gate passage. The
/// gate looks toward −Z through a pointed arch with voussoirs and keystone, a half-raised
/// iron portcullis and heavy oak doors (one ajar) under a vault with murder holes.
pub fn gatehouse(width: f32, height: f32, depth: f32, seed: u64) -> Vec<ArchPart> {
    let mut k = Kit::new(seed);
    k.gatehouse(
        width.clamp(10.0, 40.0),
        height.max(9.0),
        depth.clamp(5.0, 30.0),
    );
    k.finish()
}

impl Kit {
    fn gatehouse(&mut self, w: f32, h: f32, d: f32) {
        let st = Style::ashlar();
        let s = (w * 0.11).clamp(1.5, 2.2);
        let rt = (w * 0.5 - s) * 0.5 - 0.15;
        let xt = w * 0.5 - rt;
        let zt = -d * 0.5 - 1.2;
        let hc = h - 2.7;
        let mut spec = WallSpec::new(w, hc, d);
        spec.plinth = 0.0;
        let walk = spec.walk;
        let soff = spec.soffit();
        let arch = Arch {
            s,
            ys: snap(2.7),
            rho: s * 1.45,
        };
        let ring = 0.55;
        let zf = -d * 0.5;
        let zr = d * 0.5;
        let zg = zf + ring + 0.25;
        // Flanking towers; what is buried in the block is skipped.
        self.cull = vec![
            (
                Vec3::new(s + 0.3, -1.0, zf + 0.04),
                Vec3::new(w, walk - 0.5, zr + 1.0),
            ),
            (
                Vec3::new(-w, -1.0, zf + 0.04),
                Vec3::new(-s - 0.3, walk - 0.5, zr + 1.0),
            ),
        ];
        for sx in [-1.0f32, 1.0] {
            self.round_tower(
                Vec3::new(sx * xt, 0.0, zt),
                rt,
                h,
                TowerTop::Crenellated,
                0.4,
            );
        }
        self.cull.clear();
        // Front and rear faces with the arch openings.
        for (z, n) in [(zf, -Vec3::Z), (zr, Vec3::Z)] {
            let face = Plane::vertical(Vec3::new(0.0, 0.0, z), n);
            let r = face.rect(Vec3::new(-w * 0.5, 0.0, z), Vec3::new(w * 0.5, soff, z));
            let mut holes = self.arch_holes(&arch, &face, z);
            if n.z < 0.0 {
                // Slits flanking the arch and an armorial panel above it.
                let y = arch.apex() + 1.6;
                for sx in [-1.0f32, 1.0] {
                    let u = face.uv(Vec3::new(sx * (s + 0.55), 0.0, z)).x;
                    let o = Opening {
                        r: Rect::new(u - 0.06, -(y + 1.2), u + 0.06, -y),
                        frame: 0.06,
                        depth: 0.9,
                        splay: 0.3,
                    };
                    holes.push(self.opening(&face, &o, None));
                }
                holes.push(self.armorial_panel(&face, Vec3::new(0.0, arch.apex() + 1.75, z)));
            }
            self.face(&face, r, |_| Edge::Groove, &holes, st);
            self.voussoirs(&arch, z, n.z, ring);
        }
        // Side faces of the block.
        for sx in [-1.0f32, 1.0] {
            let face = Plane::vertical(Vec3::new(sx * w * 0.5, 0.0, 0.0), Vec3::X * sx);
            let r = face.rect(
                Vec3::new(sx * w * 0.5, 0.0, zf),
                Vec3::new(sx * w * 0.5, soff, zr),
            );
            self.face(&face, r, |_| Edge::Groove, &[], st.coarse());
        }
        // Parapets: machicolated over the gate, plain elsewhere.
        let front = Frame::new(Vec3::new(0.0, 0.0, zf), Vec3::X);
        self.parapet(
            &front,
            &spec,
            -w * 0.5,
            w * 0.5,
            (-w * 0.5, w * 0.5),
            true,
            false,
        );
        let mut flush = spec;
        flush.proj = 0.0;
        let back = Frame::new(Vec3::new(0.0, 0.0, zr), -Vec3::X);
        self.parapet(
            &back,
            &flush,
            -w * 0.5,
            w * 0.5,
            (-w * 0.5, w * 0.5),
            false,
            false,
        );
        let (_, zpi) = spec.parapet_z();
        for sx in [-1.0f32, 1.0] {
            let fr = Frame::new(Vec3::new(sx * w * 0.5, 0.0, 0.0), Vec3::Z * sx);
            let (a, b) = (zf + zpi, zr - flush.tp);
            let (a, b) = if sx > 0.0 { (a, b) } else { (-b, -a) };
            self.parapet(&fr, &flush, a, b, (a, b), false, false);
        }
        let fl = Plane::horizontal(walk, true);
        let r = fl.rect(
            Vec3::new(-w * 0.5 + flush.tp - 0.08, walk, zf + zpi - 0.08),
            Vec3::new(w * 0.5 - flush.tp + 0.08, walk, zr - flush.tp + 0.08),
        );
        self.face(&fl, r, |_| Edge::Groove, &[], st.worn(0.3).coarse());
        self.gate_passage(&arch, zf, zr, zg, ring);
        self.portcullis(&arch, zg);
        self.gate_doors(&arch, zf + ring + 1.25);
        for sx in [-1.0f32, 1.0] {
            let base = Vec3::new(sx * xt, h - 2.2 - 0.06, zt);
            self.banner(base + Vec3::new(-sx * 0.6, 0.0, 0.4), 5.5, sx > 0.0);
        }
    }

    /// Per-course holes for an arch in a face at `z`: below the springing the jambs are
    /// corners with the passage walls; above, the coursing stops under the voussoirs.
    fn arch_holes(&self, a: &Arch, face: &Plane, z: f32) -> Vec<Hole> {
        let mut holes = Vec::new();
        let apex = a.apex();
        let mut k = 0;
        loop {
            let ya = k as f32 * COURSE_H;
            let yb = ya + COURSE_H;
            if ya >= apex - 0.05 {
                break;
            }
            if yb <= a.ys + 1e-3 {
                let r = face.rect(Vec3::new(-a.s, ya, z), Vec3::new(a.s, yb, z));
                holes.push(corner_hole(face, r));
            } else {
                let hw = a.half_width(ya.max(a.ys));
                let r = face.rect(Vec3::new(-hw, ya, z), Vec3::new(hw, yb, z));
                holes.push(Hole {
                    r,
                    e: [Edge::Groove; 4],
                });
            }
            k += 1;
        }
        holes
    }

    /// The arch ring at the face `z` (`dir` −1 front, +1 rear): voussoirs with a stepped
    /// extrados, a proud keystone and impost blocks.
    fn voussoirs(&mut self, a: &Arch, z: f32, dir: f32, ring: f32) {
        let proud = 0.035;
        let (z_out, z_in) = (z + dir * proud, z - dir * ring);
        let la = a.side_len();
        let key = 0.24 / a.rho;
        let phi_a = a.phi_apex();
        let n = ((la - 0.24) / 0.44).round().max(3.0) as usize;
        let hidden_back = if dir < 0.0 { PZ } else { NZ };
        for side in [-1.0f32, 1.0] {
            for i in 0..n {
                let p0 = (phi_a - key) * i as f32 / n as f32;
                let p1 = (phi_a - key) * (i + 1) as f32 / n as f32;
                let dv = if i % 2 == 0 { 0.62 } else { 0.48 };
                let p: [Vec3; 8] = std::array::from_fn(|k| {
                    let phi = if k & 1 == 0 { p0 } else { p1 };
                    let q = a.at(side, phi, if k & 2 == 0 { 0.0 } else { dv });
                    Vec3::new(q.x, q.y, if k & 4 == 0 { z_out } else { z_in })
                });
                let hidden = NX | PX | PY | hidden_back;
                self.stone_hexa(p, hidden, 0.03, Uvm::World);
            }
            // Impost: a moulded block the arch springs from.
            let x = side * a.s;
            let p: [Vec3; 8] = std::array::from_fn(|k| {
                let xx = if k & 1 == 0 {
                    x - side * 0.1
                } else {
                    x + side * 0.45
                };
                let up = k & 2 != 0;
                let y = if up { a.ys } else { a.ys - 0.3 };
                let zz = if k & 4 == 0 {
                    z + dir * (proud + 0.05)
                } else {
                    z_in
                };
                // Undercut: the bottom front edge is cut back.
                let xx = if !up && k & 1 == 0 {
                    x - side * 0.02
                } else {
                    xx
                };
                Vec3::new(xx, y, zz)
            });
            self.stone_hexa_e(
                p,
                hidden_back,
                if side < 0.0 { NX } else { PX },
                0.03,
                Uvm::World,
            );
        }
        // Keystone spanning the apex.
        let p: [Vec3; 8] = std::array::from_fn(|k| {
            let side = if k & 1 == 0 { -1.0 } else { 1.0 };
            let q = a.at(side, phi_a - key, if k & 2 == 0 { 0.0 } else { 0.78 });
            let zz = if k & 4 == 0 { z_out + dir * 0.03 } else { z_in };
            Vec3::new(q.x, q.y, zz)
        });
        self.stone_hexa(p, PY | hidden_back, 0.035, Uvm::World);
    }

    /// Passage walls with the portcullis groove, the vault with its slot and murder holes,
    /// and the paving.
    fn gate_passage(&mut self, a: &Arch, zf: f32, zr: f32, zg: f32, ring: f32) {
        let st = Style::ashlar();
        let gw = 0.11;
        let gd = 0.17;
        for side in [-1.0f32, 1.0] {
            let n = Vec3::X * -side;
            let x = side * a.s;
            let face = Plane::vertical(Vec3::new(x, 0.0, 0.0), n);
            let r = face.rect(Vec3::new(x, 0.0, zf), Vec3::new(x, a.ys, zr));
            let mut holes = Vec::new();
            let mut y = 0.0;
            while y < a.ys - 1e-3 {
                let r = face.rect(
                    Vec3::new(x, y, zg - gw),
                    Vec3::new(x, y + COURSE_H, zg + gw),
                );
                holes.push(corner_hole(&face, r));
                y += COURSE_H;
            }
            self.face(
                &face,
                r,
                |d| {
                    if d.z < -0.7 {
                        Edge::Corner(-Vec3::Z)
                    } else if d.z > 0.7 {
                        Edge::Corner(Vec3::Z)
                    } else {
                        Edge::Groove
                    }
                },
                &holes,
                st,
            );
            // The groove itself.
            let half = JOINT * 0.5;
            let dark = [0.35, 0.34, 0.32];
            let mb = &mut self.stone;
            for (zz, m) in [(zg - gw, Vec3::Z), (zg + gw, -Vec3::Z)] {
                let lip = |y: f32| Vec3::new(x, y, zz) - (n + m) * half;
                let deep = |y: f32| Vec3::new(x - n.x * gd, y, zz);
                quad(
                    mb,
                    [lip(0.0), lip(a.ys), deep(a.ys), deep(0.0)],
                    [
                        Vec2::new(zz, 0.0),
                        Vec2::new(zz, -a.ys),
                        Vec2::new(zz + gd, -a.ys),
                        Vec2::new(zz + gd, 0.0),
                    ],
                    [[0.8; 3], [0.8; 3], dark, dark],
                    m,
                );
            }
            let back = |y: f32, zz: f32| Vec3::new(x - n.x * gd, y, zz);
            quad(
                mb,
                [
                    back(0.0, zg - gw),
                    back(0.0, zg + gw),
                    back(a.ys, zg + gw),
                    back(a.ys, zg - gw),
                ],
                [
                    Vec2::ZERO,
                    Vec2::X * 0.22,
                    Vec2::new(0.22, -a.ys),
                    Vec2::new(0.0, -a.ys),
                ],
                [dark; 4],
                n,
            );
        }
        // Vault between the two arch rings.
        let vault = Vault { a: *a, x0: 0.0 };
        let total = 2.0 * a.side_len();
        let r = Rect::new(zf + ring, 0.0, zr - ring, total);
        let mut holes = vec![corner_hole(
            &vault,
            Rect::new(zg - gw, -0.5, zg + gw, total + 0.5),
        )];
        for zm in [-0.9f32, 0.9] {
            let zc = (zf + zr) * 0.5 + zm;
            let o = Opening {
                r: Rect::new(zc - 0.17, total * 0.5 - 0.17, zc + 0.17, total * 0.5 + 0.17),
                frame: 0.04,
                depth: 0.9,
                splay: 0.0,
            };
            holes.push(self.opening(&vault, &o, None));
        }
        self.face_e(&vault, r, [Edge::Groove; 4], &holes, st.missing(0.0));
        // Portcullis slot above the vault: two walls and a top.
        let top = a.apex() + 0.9;
        let dark = [0.25, 0.24, 0.22];
        let segs = 16;
        let mb = &mut self.stone;
        for (zz, m) in [(zg - gw, Vec3::Z), (zg + gw, -Vec3::Z)] {
            for i in 0..segs {
                let (p0, _) = a.profile(total * i as f32 / segs as f32);
                let (p1, _) = a.profile(total * (i + 1) as f32 / segs as f32);
                let q = [
                    Vec3::new(p0.x, p0.y, zz),
                    Vec3::new(p1.x, p1.y, zz),
                    Vec3::new(p1.x, top, zz),
                    Vec3::new(p0.x, top, zz),
                ];
                let uv = q.map(|p| Vec2::new(p.x, -p.y));
                quad(mb, q, uv, [dark; 4], m);
            }
            let q = [
                Vec3::new(-a.s - gd, 0.0, zz),
                Vec3::new(-a.s, 0.0, zz),
                Vec3::new(-a.s, top, zz),
                Vec3::new(-a.s - gd, top, zz),
            ];
            let _ = q;
        }
        let q = [
            Vec3::new(-a.s - gd, top, zg - gw),
            Vec3::new(a.s + gd, top, zg - gw),
            Vec3::new(a.s + gd, top, zg + gw),
            Vec3::new(-a.s - gd, top, zg + gw),
        ];
        quad(mb, q, [Vec2::ZERO; 4], [dark; 4], -Vec3::Y);
        // Worn paving through the gate and out into the approach.
        let fl = Plane::horizontal(0.035, true);
        let r = fl.rect(
            Vec3::new(-a.s, 0.035, zf - 3.2),
            Vec3::new(a.s, 0.035, zr + 1.0),
        );
        self.face(&fl, r, |_| Edge::Groove, &[], st.worn(0.45).coarse());
    }

    /// Iron portcullis in its slot at `zg`, raised so its spiked foot hangs at mid height.
    fn portcullis(&mut self, a: &Arch, zg: f32) {
        let bottom = a.ys - 0.3;
        let top = a.apex() + 0.8;
        let half = a.s + 0.1;
        let mb = &mut self.metal;
        let col = [1.0; 3];
        let nv = ((2.0 * half) / 0.3).round() as usize;
        for i in 0..=nv {
            let x = -half + 2.0 * half * i as f32 / nv as f32;
            let p0 = Vec3::new(x, bottom, zg);
            hexa(
                mb,
                beam_corners(p0, Vec3::new(x, top, zg), Vec3::Z, 0.08, 0.08),
                0,
                0.012,
                col,
                Uvm::World,
            );
            // Spike.
            let tip = Vec3::new(x, bottom - 0.3, zg);
            let p: [Vec3; 8] = std::array::from_fn(|k| {
                if k & 2 == 0 {
                    tip
                } else {
                    p0 + Vec3::new(
                        if k & 1 == 0 { -0.04 } else { 0.04 },
                        0.0,
                        if k & 4 == 0 { -0.04 } else { 0.04 },
                    )
                }
            });
            hexa(mb, p, PY, 0.0, col, Uvm::World);
        }
        let mut y = bottom + 0.12;
        while y < top - 0.2 {
            let a0 = Vec3::new(-half - 0.04, y, zg + 0.07);
            let a1 = Vec3::new(half + 0.04, y, zg + 0.07);
            hexa(
                mb,
                beam_corners(a0, a1, Vec3::Y, 0.06, 0.08),
                0,
                0.01,
                col,
                Uvm::World,
            );
            y += 0.45;
        }
    }

    /// Two oak doors under the vault at `z`: the left one shut, the right one ajar.
    fn gate_doors(&mut self, a: &Arch, z: f32) {
        let mut rng = self.rng.fork(55);
        for (side, angle) in [(-1.0f32, 0.0f32), (1.0, 0.5)] {
            let hinge = Vec3::new(side * (a.s - 0.03), 0.04, z);
            let closed = Vec3::X * -side;
            let rot = Quat::from_rotation_y(side * angle);
            let along = rot * closed;
            let thick = rot * Vec3::Z;
            let wleaf = a.s - 0.04;
            let at = |s: f32, y: f32, t: f32| hinge + along * s + Vec3::Y * y + thick * t;
            let height = |s: f32| a.half_width_inv(side * (a.s - 0.03 - s)) - 0.06;
            // Planks.
            let mut s0 = 0.0;
            let mb = &mut self.wood;
            while s0 < wleaf - 0.02 {
                let s1 = (s0 + rng.range(0.2, 0.27)).min(wleaf);
                let (h0, h1) = (height(s0), height(s1));
                let p: [Vec3; 8] = std::array::from_fn(|k| {
                    let s = if k & 1 == 0 { s0 + 0.004 } else { s1 - 0.004 };
                    let y = if k & 2 == 0 {
                        0.0
                    } else if k & 1 == 0 {
                        h0
                    } else {
                        h1
                    };
                    at(s, y, if k & 4 == 0 { -0.05 } else { 0.05 })
                });
                let col = wood_tint(&mut rng);
                hexa(mb, p, NY, 0.008, mulc(col, 0.8), Uvm::World);
                s0 = s1;
            }
            // Battens and a brace on the inner side.
            let ys = [0.45, 1.55, 2.6];
            for &y in &ys {
                let p0 = at(0.06, y, 0.08);
                let p1 = at(wleaf - 0.06, y, 0.08);
                hexa(
                    mb,
                    beam_corners(p0, p1, Vec3::Y, 0.06, 0.2),
                    0,
                    0.012,
                    wood_tint(&mut rng),
                    Uvm::World,
                );
            }
            let b0 = at(0.15, ys[0] + 0.1, 0.08);
            let b1 = at(wleaf - 0.2, ys[1] - 0.1, 0.08);
            hexa(
                mb,
                beam_corners(b0, b1, thick, 0.06, 0.16),
                0,
                0.012,
                wood_tint(&mut rng),
                Uvm::World,
            );
            // Iron strap hinges with studs on the outer face.
            let mbm = &mut self.metal;
            let iron = [0.85, 0.85, 0.85];
            for &y in &ys {
                let len = wleaf * 0.82;
                let p0 = at(-0.02, y, -0.055);
                let p1 = at(len, y, -0.055);
                hexa(
                    mbm,
                    beam_corners(p0, p1, Vec3::Y, 0.014, 0.085),
                    0,
                    0.004,
                    iron,
                    Uvm::World,
                );
                let mut s = 0.12;
                while s < len - 0.05 {
                    let c = at(s, y, -0.066);
                    hexa(
                        mbm,
                        box_corners(c, Vec3::new(0.022, 0.022, 0.012), rot),
                        PZ,
                        0.0,
                        iron,
                        Uvm::World,
                    );
                    s += 0.24;
                }
                // Hinge pin block on the jamb.
                let pin = hinge + Vec3::Y * y - thick * 0.03 + Vec3::X * side * 0.05;
                hexa(
                    mbm,
                    box_corners(pin, Vec3::new(0.05, 0.07, 0.05), Quat::IDENTITY),
                    0,
                    0.01,
                    iron,
                    Uvm::World,
                );
            }
            // Ring handle on the shut leaf.
            if angle == 0.0 {
                let c = at(wleaf - 0.3, 1.15, -0.06);
                hexa(
                    mbm,
                    box_corners(c, Vec3::new(0.05, 0.05, 0.015), rot),
                    0,
                    0.006,
                    iron,
                    Uvm::World,
                );
                let r0 = c + Vec3::new(0.0, -0.05, -0.02);
                hexa(
                    mbm,
                    beam_corners(r0, r0 - Vec3::Y * 0.16, Vec3::Z, 0.02, 0.02),
                    0,
                    0.004,
                    iron,
                    Uvm::World,
                );
            }
        }
    }

    /// A carved armorial panel: a framed recess with a proud shield. Returns the hole.
    fn armorial_panel(&mut self, face: &Plane, c: Vec3) -> Hole {
        let uv = face.uv(c);
        let o = Opening {
            r: Rect::new(uv.x - 0.5, uv.y - 0.62, uv.x + 0.5, uv.y + 0.62),
            frame: 0.07,
            depth: 0.12,
            splay: 0.0,
        };
        let hole = self.opening(face, &o, None);
        let back = c - face.n * 0.12;
        let du = face.du;
        let p: [Vec3; 8] = std::array::from_fn(|k| {
            let x = if k & 1 == 0 { -0.36 } else { 0.36 };
            let (x, y) = if k & 2 != 0 {
                (x, 0.42)
            } else {
                (x * 0.15, -0.5)
            };
            let t = if k & 4 == 0 { 0.09 } else { 0.0 };
            back + du * x + Vec3::Y * y + face.n * t
        });
        let t = self.stone_tint(0.04);
        hexa(
            &mut self.stone,
            p,
            0,
            0.025,
            mix3(t, [1.05, 0.62, 0.5], 0.5),
            Uvm::World,
        );
        hole
    }

    /// A pole with an iron finial and a long hanging banner (cloth is drawn as plaster).
    fn banner(&mut self, base: Vec3, height: f32, red: bool) {
        let mut rng = self.rng.fork(13);
        let top = base + Vec3::Y * height;
        hexa(
            &mut self.wood,
            beam_corners(base, top, Vec3::X, 0.09, 0.09),
            0,
            0.015,
            wood_tint(&mut rng),
            Uvm::World,
        );
        hexa(
            &mut self.metal,
            box_corners(
                top + Vec3::Y * 0.05,
                Vec3::splat(0.07),
                Quat::from_rotation_y(0.7),
            ),
            0,
            0.03,
            [0.9; 3],
            Uvm::World,
        );
        // Cross bar and the banner hanging from it, rippling.
        let bar0 = top - Vec3::Y * 0.25;
        let dir = Vec3::new(0.94, 0.0, -0.34);
        hexa(
            &mut self.wood,
            beam_corners(bar0, bar0 + dir * 1.25, Vec3::Y, 0.05, 0.05),
            0,
            0.01,
            wood_tint(&mut rng),
            Uvm::World,
        );
        let len = 2.8;
        let wid = 1.15;
        let col = if red {
            [0.62, 0.09, 0.07]
        } else {
            [0.08, 0.16, 0.45]
        };
        let gold = [0.85, 0.62, 0.18];
        let nx = 6;
        let ny = 10;
        let side = dir.cross(Vec3::Y).normalize();
        let pt = |i: usize, j: usize| -> Vec3 {
            let a = i as f32 / nx as f32;
            let b = j as f32 / ny as f32;
            let wave = (a * 7.0 + b * 3.0).sin() * 0.06 * (0.3 + b) + b * b * 0.35;
            // Swallow-tailed foot.
            let tail = if b > 0.85 {
                (1.0 - (a - 0.5).abs() * 2.0) * (b - 0.85) * 2.2
            } else {
                0.0
            };
            bar0 + dir * (0.08 + a * wid) - Vec3::Y * (b * len - tail) + side * wave
        };
        let mb = &mut self.plaster;
        for i in 0..nx {
            for j in 0..ny {
                let q = [pt(i, j), pt(i + 1, j), pt(i + 1, j + 1), pt(i, j + 1)];
                let a = (i as f32 + 0.5) / nx as f32;
                let b = (j as f32 + 0.5) / ny as f32;
                let stripe = (a - 0.5).abs() < 0.09 || b < 0.08;
                let c = if stripe { gold } else { col };
                let n = (q[1] - q[0]).cross(q[3] - q[0]).normalize_or(side);
                let uv = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
                quad(mb, q, uv, [c; 4], n);
                quad(mb, [q[3], q[2], q[1], q[0]], uv, [mulc(c, 0.8); 4], -n);
            }
        }
    }
}

impl Arch {
    /// Height of the intrados above `x` (the springing height outside the span).
    fn half_width_inv(&self, x: f32) -> f32 {
        let x = x.abs().min(self.s);
        let side = 1.0;
        let cx = self.cx(side);
        let r = self.rho;
        let dx = x - cx;
        if dx.abs() >= r {
            return self.ys;
        }
        self.ys + (r * r - dx * dx).sqrt()
    }
}

// ---------------------------------------------------------------------------------------------
// Ruined wall
// ---------------------------------------------------------------------------------------------

/// One facing stone of a ruined wall skin, as laid in its course.
#[derive(Clone, Copy, Debug)]
struct SkinStone {
    a: f32,
    b: f32,
    /// Still standing.
    on: bool,
    /// Knocked askew on the break.
    loose: bool,
    /// Broken back at its right (+) or left (−) end by this much at the top.
    cut: f32,
}

impl SkinStone {
    /// Does this stone sit squarely at `x` (standing, not loosened, not broken away there)?
    fn holds(&self, x: f32) -> bool {
        self.on
            && !self.loose
            && x > self.a
            && x < self.b
            && !(self.cut > 0.0 && x > self.b - self.cut)
            && !(self.cut < 0.0 && x < self.a - self.cut)
    }
}

/// Is `x` squarely backed by a stone of `row`?
fn row_holds(row: Option<&Vec<SkinStone>>, x: f32) -> bool {
    row.is_some_and(|r| r.iter().any(|q| q.holds(x)))
}

impl Kit {
    /// A breached wall run of thickness `t` (outer face at z = −t/2, x from −len/2 to len/2).
    fn ruined_wall(&mut self, len: f32, h: f32, t: f32) {
        let (x0, x1) = (-len * 0.5, len * 0.5);
        let half = len * 0.5;
        let fr = Frame::new(Vec3::new(0.0, 0.0, -t * 0.5), Vec3::X);
        // Plinth and batter of the wall as built (about a tenth taller than what is left).
        let w = WallSpec::new(len, h / 0.9, t);
        let st = Style::ashlar();
        let seed = self.seed;
        // Course lines; the last course ends exactly at `h`.
        let nc = ((h / COURSE_H).round() as usize).max(4);
        let yc = move |k: usize| if k >= nc { h } else { k as f32 * COURSE_H };
        let k0 = ((w.plinth / COURSE_H).round() as usize + 1).min(nc - 2);
        let y0 = yc(k0);
        // The break: a breach near the middle, stepping up to full height at the ends.
        let xc = (self.rng.f32() - 0.5) * 0.2 * len;
        let floor = y0 + COURSE_H * 1.3;
        let prof = move |x: f32, side: usize| -> f32 {
            let s = (x - xc).abs() / half;
            let base = floor + (h - floor) * smoothstep(0.13, 0.8, s);
            let n = fbm3(
                x as f64 * 0.42,
                side as f64 * 7.3,
                0.5,
                seed ^ 0x6a11,
                3,
                2.0,
                0.5,
            ) * 1.4
                + perlin3(x as f64 * 1.4, side as f64 * 3.1, 2.5, seed ^ 0x6a12) * 0.7;
            let y = (base + n - side as f32 * 0.4).max(floor - 0.35);
            let end = smoothstep(half - 1.9, half - 0.8, x.abs());
            y + (h - y) * end
        };
        // Which facing stones still stand: through the profile, and only on support.
        let salt = self.next_salt();
        let mut skins: [Vec<Vec<SkinStone>>; 2] = [Vec::new(), Vec::new()];
        for (side, skin) in skins.iter_mut().enumerate() {
            let ss = (salt * 4 + side as u64) as i64;
            for k in k0..nc {
                let off = row_offset(-(k as i64) - 1);
                let yb = yc(k + 1);
                let mut row: Vec<SkinStone> = spans(x0, x1, BLOCK_W, off, 0.22)
                    .into_iter()
                    .map(|(a, b)| {
                        let j = ((a + b) / BLOCK_W).round() as i64;
                        let hk = |i: i64| hash01(k as i64, j * 8 + i, ss, seed);
                        let end = a < x0 + 1e-3 || b > x1 - 1e-3;
                        let jit = (hk(0) - 0.5) * 0.9;
                        let stands = prof((a + b) * 0.5, side) + jit >= yb - 0.12;
                        let on = end || k == k0 || (stands && hk(1) > 0.025);
                        SkinStone {
                            a,
                            b,
                            on,
                            loose: false,
                            cut: 0.0,
                        }
                    })
                    .collect();
                if let Some(below) = skin.last() {
                    for s in &mut row {
                        s.on &= below
                            .iter()
                            .any(|q| q.on && q.b.min(s.b) - q.a.max(s.a) > 0.2);
                    }
                }
                skin.push(row);
            }
            // Stones left uncovered on the break: some knocked askew, some broken back.
            for ki in 1..skin.len() {
                let (lower, upper) = skin.split_at_mut(ki + 1);
                let row = &mut lower[ki];
                let above = upper.first();
                let n = row.len();
                for i in 0..n {
                    let s = row[i];
                    if !s.on || s.a < x0 + 1e-3 || s.b > x1 - 1e-3 {
                        continue;
                    }
                    let covered = above.is_some_and(|r| {
                        r.iter().any(|q| q.on && q.b.min(s.b) - q.a.max(s.a) > 0.2)
                    });
                    if covered {
                        continue;
                    }
                    let hk = |q: i64| hash01((k0 + ki) as i64, i as i64 * 8 + q, ss + 2, seed);
                    let left_off = i > 0 && !row[i - 1].on;
                    let right_off = i + 1 < n && !row[i + 1].on;
                    if (left_off || right_off) && hk(3) < 0.4 && s.b - s.a > 0.6 {
                        let c = 0.16 + 0.3 * hk(4);
                        row[i].cut = if right_off && (!left_off || hk(5) < 0.5) {
                            c
                        } else {
                            -c
                        };
                    } else if above.is_some() && hk(2) < 0.4 {
                        row[i].loose = true;
                    }
                }
            }
        }
        // Rubble core between the skins. It was laid in lifts with the facing, so its broken
        // top steps with the stones still standing in both skins, roughened.
        let zc = [0.5f32, t - 0.5];
        let skins = &skins;
        let standing = move |side: usize, x: f32| -> f32 {
            skins[side]
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    row.iter()
                        .any(|q| q.on && x >= q.a - 1e-3 && x <= q.b + 1e-3)
                })
                .map(|(ki, _)| yc(k0 + ki + 1))
                .fold(y0, f32::max)
        };
        let core = move |x: f32, z: f32| -> f32 {
            let wz = ((z - zc[0]) / (zc[1] - zc[0])).clamp(0.0, 1.0);
            let f = |side: usize| {
                (standing(side, x - 0.16) + standing(side, x) + standing(side, x + 0.16)) / 3.0
            };
            let (fo, fi) = (f(0), f(1));
            let base = fo + (fi - fo) * wz - 0.2;
            let hump = 0.1 * (wz * PI).sin();
            let n = fbm3(
                x as f64 * 0.9,
                z as f64 * 0.9,
                3.7,
                seed ^ 0x77c0,
                3,
                2.0,
                0.5,
            ) * 0.22
                + fbm3(
                    x as f64 * 3.1,
                    z as f64 * 3.1,
                    1.3,
                    seed ^ 0x77c1,
                    2,
                    2.0,
                    0.5,
                ) * 0.1;
            let y = (base + hump + n).clamp(y0 + 0.12, h - 0.04);
            let end = smoothstep(half - 1.3, half - 0.6, x.abs());
            y + (h - 0.04 - y) * end
        };
        // Where the core's side shows, it bulges a little out of the line of the facing.
        let bulge = move |x: f32, y: f32, side: usize| -> f32 {
            let n = perlin3(
                x as f64 * 2.3,
                y as f64 * 2.3,
                side as f64 * 5.1,
                seed ^ 0xb1d6,
            );
            (0.05 + 0.07 * n) * if side == 0 { -1.0 } else { 1.0 }
        };

        // Intact lower work: battered plinth, string course, first courses, end faces.
        let pl = Plane::battered(fr.p(0.0, w.plinth, 0.0), -fr.z, w.batter, w.plinth);
        let r = pl.rect(fr.p(x0, w.plinth, 0.0), fr.p(x1, 0.0, -w.batter));
        let putlogs = self.putlog_holes(&pl, r, 0.9, 1.5, &[]);
        self.face(
            &pl,
            r,
            |_| Edge::Groove,
            &putlogs,
            st.worn(0.35).missing(0.012),
        );
        self.string_course(&fr, x0, x1, w.plinth, false);
        let outer = fr.vplane(0.0, -Vec3::Z);
        let r = outer.rect(fr.p(x0, w.plinth, 0.0), fr.p(x1, y0, 0.0));
        self.face(&outer, r, |_| Edge::Groove, &[], st.worn(0.4));
        let inner = fr.vplane(t, Vec3::Z);
        let r = inner.rect(fr.p(x0, 0.0, t), fr.p(x1, y0, t));
        self.face(
            &inner,
            r,
            |_| Edge::Groove,
            &[],
            st.worn(0.35).missing(0.012),
        );
        for (x, s) in [(x0, -1.0f32), (x1, 1.0)] {
            let ep = fr.end_plane(x, s);
            let r = ep.rect(fr.p(x, 0.0, 0.0), fr.p(x, h, t));
            self.face(&ep, r, |_| Edge::Groove, &[], st.missing(0.0));
        }

        // The standing facing stones, one chamfered block each.
        for (side, skin) in skins.iter().enumerate() {
            let outer_side = side == 0;
            let (back, out_z) = if outer_side { (PZ, -1.0) } else { (NZ, 1.0) };
            for (ki, row) in skin.iter().enumerate() {
                let k = k0 + ki;
                let (ya, yb) = (yc(k), yc(k + 1));
                let below = if ki > 0 { skin.get(ki - 1) } else { None };
                let above = skin.get(ki + 1);
                for (i, s) in row.iter().enumerate() {
                    if !s.on {
                        continue;
                    }
                    let hs = |q: i64| {
                        hash01(
                            k as i64,
                            i as i64 * 8 + q,
                            (salt * 4 + 2 + side as u64) as i64,
                            seed,
                        )
                    };
                    let d = 0.5 + 0.28 * hs(0);
                    let (za, zb) = if outer_side { (0.0, d) } else { (t - d, t) };
                    let mut p = fr.box_corners(Vec3::new(s.a, ya, za), Vec3::new(s.b, yb, zb));
                    if s.cut != 0.0 {
                        // A slanted fracture: more is gone at the top and toward the core.
                        let right = s.cut > 0.0;
                        for (ci, c) in p.iter_mut().enumerate() {
                            if (ci & 1 == 1) != right {
                                continue;
                            }
                            let top = ci & 2 != 0;
                            let deep = (ci & 4 != 0) == outer_side;
                            let m = s.cut.abs()
                                * if top { 1.0 } else { 0.3 + 0.25 * hs(1) }
                                * if deep { 1.2 } else { 1.0 };
                            *c -= fr.x * if right { m } else { -m };
                        }
                    }
                    let mut hidden = 0;
                    let mut embedded = 0;
                    if s.loose {
                        // Rocked about its bottom and slid out a little.
                        let pivot = fr.p((s.a + s.b) * 0.5, ya, (za + zb) * 0.5);
                        let q = Quat::from_axis_angle(fr.x, (hs(2) - 0.5) * 0.09)
                            * Quat::from_axis_angle(Vec3::Y, (hs(3) - 0.5) * 0.07);
                        let shift = fr.z * out_z * (0.012 + 0.05 * hs(4))
                            + fr.x * (hs(5) - 0.5) * 0.06
                            + Vec3::Y * 0.004;
                        for c in &mut p {
                            *c = pivot + q * (*c - pivot) + shift;
                        }
                    } else {
                        let left = i > 0 && {
                            let q = row[i - 1];
                            q.on && !q.loose && q.cut <= 0.0
                        };
                        let right = i + 1 < row.len() && {
                            let q = row[i + 1];
                            q.on && !q.loose && q.cut >= 0.0
                        };
                        if s.cut >= 0.0 && (left || s.a < x0 + 1e-3) {
                            hidden |= NX;
                        }
                        if s.cut <= 0.0 && (right || s.b > x1 - 1e-3) {
                            hidden |= PX;
                        }
                        let (lx, rx) = (s.a + 0.22, s.b - 0.22);
                        if ki == 0 || (row_holds(below, lx) && row_holds(below, rx)) {
                            hidden |= NY;
                        }
                        if s.cut == 0.0 && row_holds(above, lx) && row_holds(above, rx) {
                            hidden |= PY;
                        }
                        let core_lo = [s.a, (s.a + s.b) * 0.5, s.b]
                            .map(|x| core(x, zc[side]))
                            .into_iter()
                            .fold(f32::MAX, f32::min);
                        if core_lo > yb + 0.02 {
                            embedded = back;
                        }
                    }
                    self.stone_hexa_e(p, hidden, embedded, JOINT * 1.3, Uvm::World);
                }
            }
        }

        // The core: lumpy top between the skins and its sides where facing stones are gone.
        let mut xs: Vec<f32> = spans(x0, x1, 0.3, 0.0, 0.1).iter().map(|s| s.0).collect();
        xs.push(x1);
        let nz = 8;
        let core_pt = |x: f32, j: usize| -> Vec3 {
            let z = zc[0] + (zc[1] - zc[0]) * j as f32 / nz as f32;
            let y = core(x, z);
            let dz = match j {
                0 => bulge(x, y, 0),
                _ if j == nz => bulge(x, y, 1),
                _ => 0.0,
            };
            fr.p(x, y, z + dz)
        };
        // Rubble and mortar: darker than the dressed faces, mapped inside one texture block
        // so no ashlar joints show on it.
        let rubble = |p: Vec3| -> [f32; 3] {
            let n = perlin3(
                p.x as f64 * 1.7,
                p.y as f64 * 1.7,
                p.z as f64 * 1.7,
                seed ^ 0x2c,
            );
            let k = 0.6 + 0.12 * n;
            [k, k * 0.96, k * 0.9]
        };
        let rubble_uv = |p: Vec3| -> Vec2 {
            Vec2::new(
                0.45 + 0.36 * (0.9 * p.x + 0.5 * p.z + 0.3 * p.y).sin(),
                0.675 + 0.17 * (0.8 * p.z - 0.6 * p.x + 0.4 * p.y).sin(),
            )
        };
        for xw in xs.windows(2) {
            for j in 0..nz {
                let q = [
                    core_pt(xw[0], j),
                    core_pt(xw[1], j),
                    core_pt(xw[1], j + 1),
                    core_pt(xw[0], j + 1),
                ];
                quad(&mut self.stone, q, q.map(rubble_uv), q.map(rubble), Vec3::Y);
            }
        }
        for (side, skin) in skins.iter().enumerate() {
            let z = zc[side];
            for xw in xs.windows(2) {
                let xm = (xw[0] + xw[1]) * 0.5;
                let tops = [core(xw[0], z), core(xw[1], z)];
                for (ki, row) in skin.iter().enumerate() {
                    let (ya, yb) = (yc(k0 + ki), yc(k0 + ki + 1));
                    if tops[0] <= ya && tops[1] <= ya {
                        break;
                    }
                    if row_holds(Some(row), xm) {
                        continue;
                    }
                    let pt = |x: f32, y: f32| fr.p(x, y, z + bulge(x, y, side));
                    let (ta, tb) = (tops[0].min(yb).max(ya), tops[1].min(yb).max(ya));
                    let q = [pt(xw[0], ya), pt(xw[1], ya), pt(xw[1], tb), pt(xw[0], ta)];
                    let out = fr.z * if side == 0 { -1.0 } else { 1.0 };
                    quad(&mut self.stone, q, q.map(rubble_uv), q.map(rubble), out);
                }
            }
        }
        // The core's rubble: rough stones bedded in the mortar all over its broken top and
        // standing out of its exposed sides.
        let pitch = 0.36;
        let nxl = (len / pitch) as usize;
        let nzl = ((zc[1] - zc[0]) / pitch).round().max(1.0) as usize;
        for i in 0..nxl {
            for j in 0..nzl {
                let x = x0 + (i as f32 + 0.2 + 0.6 * self.rng.f32()) * len / nxl as f32;
                let z =
                    zc[0] + (j as f32 + 0.2 + 0.6 * self.rng.f32()) * (zc[1] - zc[0]) / nzl as f32;
                let sz = 0.2 + 0.2 * self.rng.f32();
                let size = Vec3::new(
                    sz * (0.9 + 0.5 * self.rng.f32()),
                    sz * 0.6,
                    sz * (0.9 + 0.5 * self.rng.f32()),
                );
                let y = core(x, z).min(h - 0.06 - size.y * 0.75);
                let q = Quat::from_rotation_y(self.rng.f32() * TAU)
                    * Quat::from_rotation_x((self.rng.f32() - 0.5) * 0.5);
                self.rubble_lump(fr.p(x, y, z), size, q);
            }
        }
        for (side, skin) in skins.iter().enumerate() {
            let z = zc[side];
            for xw in xs.windows(2) {
                let xm = (xw[0] + xw[1]) * 0.5;
                let top = core(xm, z);
                for (ki, row) in skin.iter().enumerate() {
                    let ym = (yc(k0 + ki) + yc(k0 + ki + 1)) * 0.5;
                    if ym > top - 0.12 {
                        break;
                    }
                    if row_holds(Some(row), xm) || self.rng.f32() < 0.45 {
                        continue;
                    }
                    let sz = 0.2 + 0.15 * self.rng.f32();
                    let size = Vec3::new(sz * 1.2, sz * 0.7, sz);
                    let q = Quat::from_rotation_y(self.rng.f32() * TAU);
                    let out = if side == 0 { -0.02 } else { 0.02 };
                    self.rubble_lump(fr.p(xm, ym, z + out + bulge(xm, ym, side)), size, q);
                }
            }
        }

        // Spoil heaps of fallen masonry on both sides of the breach.
        let hm = (0.42 * (h - floor)).clamp(0.8, 3.0);
        let reach = hm * 2.3 + 1.5;
        let mound = move |x: f32, dist: f32, side: usize| -> f32 {
            let e = 1.0 - smoothstep(0.06 * len, 0.44 * len, (x - xc).abs());
            let fall = (1.0 - (dist / reach).clamp(0.0, 1.0)).powf(1.6);
            let n = fbm3(
                x as f64 * 0.55,
                dist as f64 * 0.55,
                side as f64 * 3.1 + 0.5,
                seed ^ 0x3b0d,
                3,
                2.0,
                0.5,
            );
            let k = if side == 0 { 1.0 } else { 0.8 };
            hm * k * e * fall + n * 0.4 * (e * fall).sqrt() - 0.12
        };
        let to_z = move |dist: f32, side: usize| if side == 0 { -dist } else { t + dist };
        let nd = 18;
        let dists: Vec<f32> = (0..=nd)
            .map(|j| -0.45 + (reach + 0.45) * (j as f32 / nd as f32).powf(1.25))
            .collect();
        let mut gx: Vec<f32> = spans(x0, x1, 0.4, 0.0, 0.1).iter().map(|s| s.0).collect();
        gx.push(x1);
        for side in 0..2 {
            let ht = |x: f32, d: f32| mound(x, d, side).max(-0.1);
            let pt = |x: f32, d: f32| fr.p(x, ht(x, d), to_z(d, side));
            let nrm = |x: f32, d: f32| {
                let e = 0.12;
                let dx = (ht(x + e, d) - ht(x - e, d)) / (2.0 * e);
                let dd = (ht(x, d + e) - ht(x, d - e)) / (2.0 * e);
                let dz = if side == 0 { -dd } else { dd };
                fr.dir(Vec3::new(-dx, 1.0, -dz)).normalize()
            };
            let col = |x: f32, d: f32| -> [f32; 3] {
                let p = (x as f64, d as f64, side as f64 * 9.0);
                let patch = fbm3(p.0 * 0.8, p.1 * 0.8, p.2, seed ^ 0x51, 3, 2.0, 0.5);
                let mut c = mulc([1.1, 1.07, 1.0], 0.9 + 0.3 * patch);
                // Lime dust and stone grit near the wall, grass creeping in at the toe.
                let dust = 1.0 - smoothstep(0.0, reach * 0.7, d);
                c = mix3(c, [1.3, 1.27, 1.2], 0.6 * dust);
                let green = smoothstep(0.45, 0.9, d / reach + 0.3 * patch);
                mix3(c, [0.62, 0.78, 0.42], 0.6 * green)
            };
            for xw in gx.windows(2) {
                for dw in dists.windows(2) {
                    let c = [
                        (xw[0], dw[0]),
                        (xw[1], dw[0]),
                        (xw[1], dw[1]),
                        (xw[0], dw[1]),
                    ];
                    if c.iter().all(|&(x, d)| ht(x, d) <= -0.09) {
                        continue;
                    }
                    let p = c.map(|(x, d)| pt(x, d));
                    let n = c.map(|(x, d)| nrm(x, d));
                    let uv = p.map(|p| Vec2::new(p.x, p.z));
                    let cc = c.map(|(x, d)| col(x, d));
                    quad_n(
                        &mut self.dirt,
                        p,
                        n,
                        uv,
                        cc,
                        (xw[0] / 0.4).round() as i64 % 2 == 0,
                    );
                }
            }
            // Rubble and fallen ashlar on the heap.
            let lumps = if side == 0 { 150 } else { 110 };
            for i in 0..lumps + 22 {
                let block = i >= lumps;
                let x = (xc + (self.rng.f32() + self.rng.f32() - 1.0) * 0.42 * len)
                    .clamp(x0 + 0.7, x1 - 0.7);
                let far = block && self.rng.f32() < 0.2;
                let d = if far {
                    reach * (0.85 + 0.4 * self.rng.f32())
                } else {
                    0.3 + reach * 0.9 * self.rng.f32().powf(1.3)
                };
                let y = ht(x, d);
                if y < -0.02 && !far {
                    continue;
                }
                let n = if far { Vec3::Y } else { nrm(x, d) };
                let tilt = Quat::from_rotation_arc(Vec3::Y, n.lerp(Vec3::Y, 0.35).normalize());
                let q = tilt
                    * Quat::from_rotation_y(self.rng.f32() * TAU)
                    * Quat::from_rotation_x((self.rng.f32() - 0.5) * 0.5)
                    * Quat::from_rotation_z((self.rng.f32() - 0.5) * 0.5);
                let base = fr.p(x, y.max(0.0), to_z(d, side));
                if block {
                    let r = self.rng.f32();
                    let size = if r < 0.55 {
                        Vec3::new(BLOCK_W, COURSE_H, 0.5 + 0.2 * self.rng.f32())
                    } else {
                        Vec3::new(
                            0.35 + 0.35 * self.rng.f32(),
                            COURSE_H,
                            0.45 + 0.2 * self.rng.f32(),
                        )
                    };
                    let c = base + n * (size.y * 0.5 - 0.1);
                    let mut p = box_corners(c, size * 0.5, q);
                    for x in &mut p {
                        *x += Vec3::new(
                            self.rng.f32() - 0.5,
                            self.rng.f32() - 0.5,
                            self.rng.f32() - 0.5,
                        ) * 0.03;
                    }
                    self.stone_hexa_e(p, 0, 0, 0.04, Uvm::Local(c, q));
                } else {
                    let sz = 0.12 + 0.38 * self.rng.f32().powi(2);
                    let size = Vec3::new(
                        sz * (1.0 + 0.8 * self.rng.f32()),
                        sz * 0.7,
                        sz * (1.0 + 0.8 * self.rng.f32()),
                    );
                    self.rubble_lump(base + n * (size.y * 0.15), size, q);
                }
            }
        }
    }

    /// An irregular broken lump of masonry: a box with jittered corners.
    fn rubble_lump(&mut self, c: Vec3, size: Vec3, q: Quat) {
        let j = size * 0.24;
        let mut p = box_corners(c, size * 0.5, q);
        for x in &mut p {
            let r = Vec3::new(self.rng.f32(), self.rng.f32(), self.rng.f32()) * 2.0 - 1.0;
            *x += q * (r * j);
        }
        // Small lumps keep sharp arrises (their chamfers would be sub-centimeter anyway).
        let ch = if size.min_element() < 0.2 {
            0.0
        } else {
            0.16 * size.min_element()
        };
        let lum = 0.7 + 0.35 * self.rng.f32();
        let col = [lum, lum * 0.97, lum * 0.92];
        hexa(&mut self.stone, p, 0, ch, col, Uvm::Local(c, q));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(parts: &[ArchPart]) {
        assert!(!parts.is_empty());
        let mut vol = 0.0f64;
        for p in parts {
            assert!(!p.mesh.indices.is_empty());
            for v in &p.mesh.vertices {
                assert!(v.position.iter().all(|x| x.is_finite()), "NaN position");
                assert!(v.normal.iter().all(|x| x.is_finite()), "NaN normal");
                assert!(
                    v.color.iter().all(|x| x.is_finite() && *x >= 0.0),
                    "bad color"
                );
            }
            for t in p.mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| Vec3::from(p.mesh.vertices[i as usize].position));
                vol += a.dot(b.cross(c)) as f64 / 6.0;
            }
        }
        assert!(vol > 0.0, "faces point inward (signed volume {vol})");
    }

    fn bounds(parts: &[ArchPart]) -> (Vec3, Vec3) {
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

    fn stone(parts: &[ArchPart]) -> &MeshBuilder {
        &parts
            .iter()
            .find(|p| p.material == DestructibleMaterial::Stone)
            .expect("stone part")
            .mesh
    }

    /// Median of `f` over the stone vertices that pass `keep` (position, normal).
    fn median(
        parts: &[ArchPart],
        keep: impl Fn(Vec3, Vec3) -> bool,
        f: impl Fn(Vec3) -> f32,
    ) -> f32 {
        let mut v: Vec<f32> = stone(parts)
            .vertices
            .iter()
            .map(|v| (Vec3::from(v.position), Vec3::from(v.normal)))
            .filter(|&(p, n)| keep(p, n))
            .map(|(p, _)| f(p))
            .collect();
        assert!(v.len() > 8, "too few samples ({})", v.len());
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    }

    /// Extent along x of the stone vertices between heights `y0` and `y1`.
    fn x_extent(parts: &[ArchPart], y0: f32, y1: f32) -> f32 {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for v in &stone(parts).vertices {
            let p = Vec3::from(v.position);
            if (y0..y1).contains(&p.y) {
                lo = lo.min(p.x);
                hi = hi.max(p.x);
            }
        }
        hi - lo
    }

    fn close(what: &str, got: f32, want: f32) {
        assert!(
            (got - want).abs() <= 0.05,
            "{what}: {got} (want {want} ± 0.05)"
        );
    }

    #[test]
    fn curtain_wall_builds_within_budget_and_size() {
        let parts = curtain_wall(30.0, 9.0, 3.0, 7);
        check(&parts);
        let tris = triangle_count(&parts);
        eprintln!("curtain wall 30 m: {tris} triangles");
        assert!((15_000..=40_000).contains(&tris), "{tris} triangles");
        close("height", bounds(&parts).1.y, 9.0);
        close("length", x_extent(&parts, 3.0, 5.0), 30.0);
        // Body thickness at mid-height, away from the stair: outer to inner face.
        let band = |p: Vec3| (3.2..4.8).contains(&p.y) && p.x < -10.0;
        let zo = median(&parts, |p, n| band(p) && n.z < -0.95, |p| p.z);
        let zi = median(&parts, |p, n| band(p) && n.z > 0.95, |p| p.z);
        close("thickness", zi - zo, 3.0);
    }

    #[test]
    fn round_towers_build_within_budget_and_size() {
        for (i, top) in [
            TowerTop::Crenellated,
            TowerTop::SlateRoof,
            TowerTop::ShingleRoof,
            TowerTop::Hoarding,
        ]
        .into_iter()
        .enumerate()
        {
            let parts = round_tower_with(5.0, 13.5, top, i as u64);
            check(&parts);
            let tris = triangle_count(&parts);
            eprintln!("round tower r 5 m, 13.5 m, {top:?}: {tris} triangles");
            assert!(tris <= 30_000, "{top:?}: {tris} triangles");
            let r = median(
                &parts,
                |p, n| {
                    let rad = Vec3::new(p.x, 0.0, p.z).normalize_or_zero();
                    (4.0..6.0).contains(&p.y) && n.dot(rad) > 0.95
                },
                |p| Vec3::new(p.x, 0.0, p.z).length(),
            );
            close("radius", r, 5.0);
            if top != TowerTop::Hoarding {
                let top_y = stone(&parts)
                    .vertices
                    .iter()
                    .map(|v| v.position[1])
                    .fold(f32::MIN, f32::max);
                close("height", top_y, 13.5);
            }
        }
    }

    #[test]
    fn square_tower_builds_within_budget_and_size() {
        let parts = square_tower(9.0, 15.0, 3);
        check(&parts);
        let tris = triangle_count(&parts);
        eprintln!("square tower 9 m, 15 m: {tris} triangles");
        assert!(tris <= 30_000, "{tris} triangles");
        close("height", bounds(&parts).1.y, 15.0);
        let half = median(
            &parts,
            |p, n| (5.0..7.0).contains(&p.y) && n.x.abs() > 0.95 && p.z.abs() < 3.5,
            |p| p.x.abs(),
        );
        close("size", half * 2.0, 9.0);
    }

    #[test]
    fn gatehouse_builds_within_budget_and_size() {
        let (w, h, d) = (16.0, 12.5, 10.0);
        let parts = gatehouse(w, h, d, 4);
        check(&parts);
        let tris = triangle_count(&parts);
        eprintln!("gatehouse {w} × {h} × {d} m: {tris} triangles");
        assert!(tris <= 50_000, "{tris} triangles");
        let top_y = stone(&parts)
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        close("height", top_y, h);
        let half = median(
            &parts,
            |p, n| (5.0..6.0).contains(&p.y) && n.x.abs() > 0.95 && p.x.abs() > w * 0.5 - 0.3,
            |p| p.x.abs(),
        );
        close("width", half * 2.0, w);
        // Faces just above the arch ring, front and back.
        let band = |p: Vec3| p.x.abs() < 1.6 && (5.75..6.25).contains(&p.y);
        let zf = median(
            &parts,
            |p, n| band(p) && n.z < -0.95 && p.z < -d * 0.4,
            |p| p.z,
        );
        let zr = median(
            &parts,
            |p, n| band(p) && n.z > 0.95 && p.z > d * 0.4,
            |p| p.z,
        );
        close("depth", zr - zf, d);
    }

    #[test]
    fn ruined_wall_builds_within_budget_and_size() {
        let parts = ruined_wall(15.0, 8.0, 5);
        check(&parts);
        let tris = triangle_count(&parts);
        eprintln!("ruined wall 15 m: {tris} triangles");
        assert!(tris <= 25_000, "{tris} triangles");
        close("height", bounds(&parts).1.y, 8.0);
        close("length", x_extent(&parts, 0.6, 1.2), 15.0);
    }

    #[test]
    fn small_pieces_build() {
        let stairs = stone_stairs(6.75, 1.35, 1);
        check(&stairs);
        close("stair height", bounds(&stairs).1.y, 6.75);
        let banner = banner_pole(5.5, true, 2);
        assert!(
            banner
                .iter()
                .any(|p| p.material == DestructibleMaterial::Plaster)
        );
        for p in &banner {
            assert!(
                p.mesh
                    .vertices
                    .iter()
                    .all(|v| v.position.iter().all(|x| x.is_finite()))
            );
        }
    }

    #[test]
    fn fortress_fits_the_budget() {
        let mut tris = 0;
        for i in 0..4 {
            tris += triangle_count(&curtain_wall(30.0, 9.0, 3.0, i));
        }
        tris += triangle_count(&round_tower_with(5.0, 14.0, TowerTop::SlateRoof, 5));
        tris += triangle_count(&round_tower_with(5.0, 13.5, TowerTop::Crenellated, 6));
        tris += triangle_count(&round_tower_with(5.0, 13.0, TowerTop::Hoarding, 7));
        tris += triangle_count(&square_tower(9.0, 15.0, 8));
        tris += triangle_count(&gatehouse(16.0, 12.5, 10.0, 9));
        eprintln!("fortress: {tris} triangles");
        assert!(tris < 250_000, "{tris} triangles");
    }
}
