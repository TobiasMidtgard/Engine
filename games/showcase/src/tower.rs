//! The spaceport's 600 m telecom and observation tower, built from code as real geometry.
//!
//! - **Foot**: a paved plaza and a podium of three two-storey buildings between the legs
//!   (curtain-wall lobbies with entrances and canopies, an office floor behind concrete fins,
//!   roof plant), board-formed concrete.
//! - **Shaft**: a slip-formed concrete core on three flared legs, a Y in plan about 54 m across
//!   at the ground, each leg with a recessed channel up its nose, shallow reveals up its sides
//!   and aviation lights; the legs taper into the core by 345 m. Glass elevators run up the front between two concrete fins.
//! - **Collar** at 344–377 m where the legs end: louvered plant floor between concrete piers, a
//!   balcony with microwave drums.
//! - **Pod** at 421–471 m: a ribbed soffit, outward-leaning observation glazing, a restaurant
//!   level, an open terrace with a railing under a deep overhang, a technical level with small
//!   windows, and a roof with dishes, a cleaning gantry, plant and a stair penthouse.
//! - **Sky deck** at 508–526 m on the slimmer upper shaft: a glazed ring and an open roof deck.
//! - **Mast** to 603 m in aviation orange and white: a steel lattice with platforms, sector
//!   antennas and dishes, then a stepped radome cylinder with dipole arrays, catwalks and a whip
//!   carrying the top beacon. Red obstruction lights mark every level.
//!
//! The tower is cut by height into sections, each its own entities (one per material) with a
//! near and a far mesh, so the switch follows the camera's distance to that part of the tower;
//! fine detail (railings, louvers, antennas, roof clutter) hides beyond a few hundred meters.
//! Large-scale weathering (rain streaks under the ledges, grime at the foot, pours of slightly
//! different tone) is in the vertex colors; the tiling textures carry the small scale: slip-form
//! concrete with pour joints, board-formed concrete, cladding cassettes, glazing with lit
//! interiors at night, painted steel. Meshes and textures build on worker threads ([`start`])
//! while the main thread grows the trees; the textures are only waited for at the end of setup
//! ([`TowerKit::finish_textures`]).
//!
//! Frame: Y up, meters, origin at the center of the tower's foot at ground level.

use crate::scenery::Lod;
use crate::texgen::{Px, TexSet, bake, bits01, fbm, hash, noise, smooth, srgb8, worley};
use engine::core::math::noise::{fbm3, perlin2};
use engine::prelude::*;
use engine::render::Vertex;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_3, PI, TAU};
use std::sync::Mutex;
use std::thread::JoinHandle;

// ===================================================================================== layout

/// Direction (radians around Y, from +X toward +Z) the front faces: the bay with the glass
/// elevators looks back at the spaceport's landing pad, which is at (-70, -60) from the tower.
const FRONT: f32 = -2.433;
/// Directions of the three legs: either side of the front bay and one at the back.
const LEGS: [f32; 3] = [FRONT + FRAC_PI_3, FRONT + PI, FRONT + 5.0 * FRAC_PI_3];
/// Where full turns of revolved surfaces start (behind the back leg).
const SEAM: f32 = FRONT + PI;

/// Height (meters) where the legs have tapered into the core.
const LEG_TOP: f32 = 345.0;
/// How far the legs reach from the axis at the ground, meters.
const LEG_REACH: f32 = 27.0;
/// Outer radius of the podium buildings, meters.
const PODIUM_R: f32 = 23.0;
/// Radius of the paved plaza, meters (the spaceport's level ground ends a little beyond it on
/// the far side).
const PLAZA_R: f32 = 30.0;

/// The mast: its foot on the sky deck's cap, its top (the beacon), and the seven paint bands
/// (orange, white, ..., orange) between.
const MAST_FOOT: f32 = 526.6;
const MAST_TOP: f32 = 603.0;
const BAND: f32 = (MAST_TOP - MAST_FOOT) / 7.0;
/// The lattice spans the three lowest bands, in panels of a quarter band.
const LATTICE_TOP: f32 = MAST_FOOT + 3.0 * BAND;
const PANELS: u32 = 12;

/// Meters covered by one repeat of each texture.
const CONCRETE_SPAN: f32 = 6.0;
const CAST_SPAN: f32 = 4.8;
const PANEL_SPAN: f32 = 6.0;
const STEEL_SPAN: f32 = 1.5;
/// Cells per side of the glazing atlas (each window shows one cell).
const CELLS: u32 = 4;

// Vertex colors (linear, multiplied with the textures).
const ORANGE: Vec3 = Vec3::new(0.7, 0.1, 0.018);
const WHITE: Vec3 = Vec3::new(0.85, 0.85, 0.83);
const DARK: Vec3 = Vec3::new(0.075, 0.08, 0.085);
const GREY: Vec3 = Vec3::new(0.36, 0.37, 0.38);
const CLAD: Vec3 = Vec3::new(0.88, 0.89, 0.9);
const PAVING: Vec3 = Vec3::new(0.66, 0.65, 0.63);
const ROOFING: Vec3 = Vec3::new(0.24, 0.24, 0.25);

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Unit vector in the ground plane at angle `a` (radians from +X toward +Z).
fn dir(a: f32) -> Vec3 {
    Vec3::new(a.cos(), 0.0, a.sin())
}

/// Radius of the concrete shaft at height `y`: tapering between the legs, slimmer again from
/// the collar to the pod.
fn core_r(y: f32) -> f32 {
    let lo = 10.0 - 1.9 * (y / 356.0).clamp(0.0, 1.0);
    let mid = lerp(8.1, 7.6, ((y - 362.0) / 64.0).clamp(0.0, 1.0));
    lerp(lo, mid, ((y - 356.0) / 6.0).clamp(0.0, 1.0))
}

/// Radius of the upper shaft between the pod and the sky deck.
fn core_hi(y: f32) -> f32 {
    lerp(5.6, 4.8, ((y - 466.0) / 46.0).clamp(0.0, 1.0))
}

/// Distance from the axis to a leg's nose at height `y` (flared at the foot, inside the core
/// by [`LEG_TOP`]).
fn leg_reach(y: f32) -> f32 {
    let t = (y / LEG_TOP).clamp(0.0, 1.0);
    core_r(y) - 0.6 + (LEG_REACH - 10.0 + 0.6) * (1.0 - t).powf(1.5)
}

/// Half a leg's thickness at height `y`.
fn leg_half(y: f32) -> f32 {
    lerp(4.8, 2.6, (y / LEG_TOP).clamp(0.0, 1.0))
}

/// Half-width of the mast's lattice at height `y`.
fn mast_half(y: f32) -> f32 {
    lerp(
        2.4,
        1.45,
        ((y - MAST_FOOT) / (LATTICE_TOP - MAST_FOOT)).clamp(0.0, 1.0),
    )
}

/// Aviation paint at height `y` on the mast: seven bands, orange at both ends.
fn band_color(y: f32) -> Vec3 {
    if ((y - MAST_FOOT) / BAND).floor() as i32 % 2 == 0 {
        ORANGE
    } else {
        WHITE
    }
}

fn hash_f(a: u32, b: u32) -> f32 {
    bits01(hash(a as i32, b as i32, 0x70e3), 8)
}

// ================================================================================ weathering

/// Slip-formed concrete: pours of slightly different tone, rain streaks that run longer and
/// darker under the ledges that shed water (collar, pod, sky deck, podium roof), grime and
/// splash at the foot, dirt on faces that look up.
fn concrete_tint(p: Vec3, n: Vec3) -> Vec3 {
    let y = p.y;
    let pour =
        perlin2(y as f64 / 7.0, 0.37, 41) * 0.035 + perlin2(y as f64 / 45.0, 5.1, 43) * 0.045;
    let under = |ledge: f32, reach: f32| {
        if y > ledge + 0.5 {
            0.0
        } else {
            smooth(ledge - reach, ledge, y)
        }
    };
    let shed = under(345.0, 50.0)
        .max(under(421.0, 75.0))
        .max(under(508.0, 35.0))
        .max(under(11.0, 6.0));
    let s = fbm3(
        p.x as f64 / 1.7,
        y as f64 / 28.0,
        p.z as f64 / 1.7,
        23,
        4,
        2.0,
        0.55,
    );
    let streak = smooth(-0.1, 0.4, s) * (0.5 + 0.5 * shed);
    let ground = smooth(2.5, 0.0, y) * 0.2 + smooth(30.0, 0.0, y) * 0.07;
    let up = smooth(0.5, 0.95, n.y) * 0.12;
    // Higher up the concrete is cleaner and a little bleached.
    let clean = smooth(60.0, 420.0, y) * 0.05;
    Vec3::new(0.82, 0.81, 0.78) * (1.0 + pour + clean) * (1.0 - 0.3 * streak - ground - up)
}

/// White cladding: faint streaks, soffits a little dirtier.
fn panel_tint(p: Vec3, n: Vec3) -> Vec3 {
    let s = fbm3(
        p.x as f64 / 1.6,
        p.y as f64 / 9.0,
        p.z as f64 / 1.6,
        57,
        2,
        2.0,
        0.5,
    );
    let down = smooth(-0.3, -0.9, n.y) * 0.1;
    CLAD * (1.0 - 0.07 * smooth(0.0, 0.6, s) - down)
}

/// Board-formed concrete of the podium: grime at the foot.
fn cast_tint(p: Vec3, _n: Vec3) -> Vec3 {
    Vec3::new(0.9, 0.88, 0.84) * (1.0 - smooth(1.2, -0.5, p.y) * 0.25)
}

// =============================================================================== mesh building

/// Texture coordinates in meters projected onto a flat face: U along the face horizontally, V
/// down the face (plan X and Z on faces that look up or down).
fn planar_uv(p: Vec3, n: Vec3) -> Vec2 {
    if n.y.abs() > 0.8 {
        Vec2::new(p.x, p.z)
    } else {
        let up = (Vec3::Y - n * n.y).normalize();
        let t = up.cross(n);
        Vec2::new(p.dot(t), -p.dot(up))
    }
}

/// A whole number of texture repeats around a full turn, so revolved surfaces have no seam.
fn snap(circumference: f32, span: f32) -> f32 {
    (circumference / span).round().max(1.0) * span
}

/// A mesh under construction. Triangles are wound to face the way their normals point.
#[derive(Default)]
struct Mb {
    mesh: Mesh,
}

impl Mb {
    fn vert(&mut self, p: Vec3, n: Vec3, uv: Vec2, c: Vec3) -> u32 {
        self.mesh
            .vertices
            .push(Vertex::new(p, n, uv).with_color(c.extend(1.0)));
        self.mesh.vertices.len() as u32 - 1
    }

    fn tri(&mut self, a: u32, b: u32, c: u32) {
        let v = &self.mesh.vertices;
        let p = |i: u32| Vec3::from(v[i as usize].position);
        let n = |i: u32| Vec3::from(v[i as usize].normal);
        let g = (p(b) - p(a)).cross(p(c) - p(a));
        if g.dot(n(a) + n(b) + n(c)) < 0.0 {
            self.mesh.indices.extend_from_slice(&[a, c, b]);
        } else {
            self.mesh.indices.extend_from_slice(&[a, b, c]);
        }
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    /// Flat convex polygon facing `n`.
    fn poly(&mut self, pts: &[Vec3], n: Vec3, c: Vec3) {
        if pts.len() < 3 {
            return;
        }
        let base = self.mesh.vertices.len() as u32;
        for p in pts {
            self.vert(*p, n, planar_uv(*p, n), c);
        }
        for i in 1..pts.len() as u32 - 1 {
            self.tri(base, base + i, base + i + 1);
        }
    }

    /// Flat quad with its own texture coordinates (corners in order around it).
    fn quad_uv(&mut self, p: [Vec3; 4], uv: [Vec2; 4], n: Vec3, c: Vec3) {
        let i: Vec<u32> = (0..4).map(|k| self.vert(p[k], n, uv[k], c)).collect();
        self.quad(i[0], i[1], i[2], i[3]);
    }

    /// Box around `center` with orthonormal axes `ax` and half extents `h` along them.
    fn cbox(&mut self, center: Vec3, ax: [Vec3; 3], h: Vec3, c: Vec3) {
        for i in 0..3 {
            let (j, k) = ((i + 1) % 3, (i + 2) % 3);
            for s in [-1.0, 1.0] {
                let n = ax[i] * s;
                let f = center + n * h[i];
                let (a, b) = (ax[j] * h[j], ax[k] * h[k]);
                self.poly(&[f - a - b, f + a - b, f + a + b, f - a + b], n, c);
            }
        }
    }

    /// Axis-aligned box from `min` to `max`.
    fn aabb(&mut self, min: Vec3, max: Vec3, c: Vec3) {
        self.cbox(
            (min + max) * 0.5,
            [Vec3::X, Vec3::Y, Vec3::Z],
            (max - min) * 0.5,
            c,
        );
    }

    /// A rectangular member from `a` to `b`: `half_h` along `up` (made square to the member),
    /// `half_w` across both.
    fn beam(&mut self, a: Vec3, b: Vec3, half_w: f32, half_h: f32, up: Vec3, c: Vec3) {
        let d = b - a;
        let len = d.length();
        if len < 1e-4 {
            return;
        }
        let x = d / len;
        let mut y = up - x * up.dot(x);
        if y.length_squared() < 1e-6 {
            y = x.any_orthonormal_vector();
        }
        let y = y.normalize();
        self.cbox(
            (a + b) * 0.5,
            [x, y, x.cross(y)],
            Vec3::new(len * 0.5, half_h, half_w),
            c,
        );
    }

    /// Smooth tapered tube from `a` (radius `ra`) to `b` (radius `rb`), optionally capped.
    #[allow(clippy::too_many_arguments)]
    fn tube(&mut self, a: Vec3, b: Vec3, ra: f32, rb: f32, sides: u32, c: Vec3, caps: bool) {
        let len = (b - a).length();
        if len < 1e-4 {
            return;
        }
        let axis = (b - a) / len;
        let u = axis.any_orthonormal_vector();
        let w = axis.cross(u);
        let slope = (ra - rb) / len;
        let base = self.mesh.vertices.len() as u32;
        let rm = (ra + rb) * 0.5;
        let ring = |t: f32| u * t.cos() + w * t.sin();
        for i in 0..=sides {
            let t = i as f32 / sides as f32 * TAU;
            let d = ring(t);
            let n = (d + axis * slope).normalize();
            self.vert(a + d * ra, n, Vec2::new(t * rm, -a.y), c);
            self.vert(b + d * rb, n, Vec2::new(t * rm, -a.y - len), c);
        }
        for i in 0..sides {
            let k = base + i * 2;
            self.quad(k, k + 2, k + 3, k + 1);
        }
        if caps {
            for (center, r, n) in [(a, ra, -axis), (b, rb, axis)] {
                if r > 0.0 {
                    let pts: Vec<Vec3> = (0..sides)
                        .map(|i| center + ring(i as f32 / sides as f32 * TAU) * r)
                        .collect();
                    self.poly(&pts, n, c);
                }
            }
        }
    }

    /// UV sphere (an octahedron-like blob with few segments).
    fn sphere(&mut self, center: Vec3, r: f32, rings: u32, segs: u32, c: Vec3) {
        let base = self.mesh.vertices.len() as u32;
        for i in 0..=rings {
            let phi = i as f32 / rings as f32 * PI;
            for j in 0..=segs {
                let th = j as f32 / segs as f32 * TAU;
                let n = Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
                self.vert(center + n * r, n, Vec2::new(th * r, -phi * r - center.y), c);
            }
        }
        for i in 0..rings {
            for j in 0..segs {
                let a = base + i * (segs + 1) + j;
                let b = a + segs + 1;
                self.quad(a, a + 1, b + 1, b);
            }
        }
    }

    /// Surface of revolution about the Y axis through the profile `pts` (radius, height), one
    /// smooth run, swept from `th.0` to `th.1` in `segs` steps. Walk the profile bottom-up
    /// around the outside of the solid: the outside is to its right. U is meters around (a
    /// whole number of `span` repeats on a full turn), V meters along the profile, downward.
    fn revolve(
        &mut self,
        pts: &[Vec2],
        th: (f32, f32),
        segs: u32,
        span: f32,
        col: &dyn Fn(Vec3, Vec3) -> Vec3,
    ) {
        let n = pts.len();
        if n < 2 || segs == 0 {
            return;
        }
        let seg_n = |i: usize| {
            let d = pts[i + 1] - pts[i];
            Vec2::new(d.y, -d.x).normalize_or(Vec2::X)
        };
        let pn: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = seg_n(i.saturating_sub(1).min(n - 2));
                let b = seg_n(i.min(n - 2));
                (a + b).normalize_or(b)
            })
            .collect();
        let rising = pts[n - 1].y >= pts[0].y;
        let mut s = 0.0;
        let v: Vec<f32> = (0..n)
            .map(|i| {
                if i > 0 {
                    s += (pts[i] - pts[i - 1]).length();
                }
                if rising {
                    -(pts[0].y + s)
                } else {
                    -(pts[0].y - s)
                }
            })
            .collect();
        let full = (th.1 - th.0).abs() >= TAU - 1e-3;
        let base = self.mesh.vertices.len() as u32;
        for j in 0..=segs {
            let f = j as f32 / segs as f32;
            let a = th.0 + (th.1 - th.0) * f;
            let (sn, cs) = a.sin_cos();
            for i in 0..n {
                let r = pts[i].x;
                let p = Vec3::new(r * cs, pts[i].y, r * sn);
                let nn = Vec3::new(pn[i].x * cs, pn[i].y, pn[i].x * sn);
                let u = if full { snap(TAU * r, span) * f } else { a * r };
                let c = col(p, nn);
                self.vert(p, nn, Vec2::new(u, v[i]), c);
            }
        }
        let n = n as u32;
        for j in 0..segs {
            for i in 0..n - 1 {
                let a = base + j * n + i;
                self.quad(a, a + 1, a + n + 1, a + n);
            }
        }
    }

    /// [`Mb::revolve`] with a crease at every profile point (flat bands and crisp edges).
    fn revolve_crisp(
        &mut self,
        pts: &[Vec2],
        th: (f32, f32),
        segs: u32,
        span: f32,
        col: &dyn Fn(Vec3, Vec3) -> Vec3,
    ) {
        for w in pts.windows(2) {
            self.revolve(w, th, segs, span, col);
        }
    }

    /// A prism lofted through horizontal outlines: `outline(y)` gives the outline at height
    /// `y` in the frame (`ex`, `ez`) as runs of points running counter-clockwise in that frame
    /// (the outside is to the right of each step), one ring per entry of `ys`. Normals are
    /// smooth along a run and creased between runs; every ring must have the same runs.
    fn loft(
        &mut self,
        ys: &[f32],
        ex: Vec3,
        ez: Vec3,
        outline: &dyn Fn(f32) -> Vec<Vec<Vec2>>,
        col: &dyn Fn(Vec3, Vec3) -> Vec3,
    ) {
        if ys.len() < 2 {
            return;
        }
        let rings: Vec<Vec<Vec<Vec2>>> = ys.iter().map(|y| outline(*y)).collect();
        let to3 = |q: Vec2, y: f32| ex * q.x + ez * q.y + Vec3::Y * y;
        // U runs on round the outline, separately in each ring.
        let mut offs = vec![0.0f32; ys.len()];
        for (k, first) in rings[0].iter().enumerate() {
            let m = first.len() as u32;
            if m < 2 {
                continue;
            }
            let base = self.mesh.vertices.len() as u32;
            for (j, y) in ys.iter().enumerate() {
                let run = &rings[j][k];
                let (jp, jn) = (j.saturating_sub(1), (j + 1).min(ys.len() - 1));
                let mut u = offs[j];
                for i in 0..run.len() {
                    let (ip, inx) = (i.saturating_sub(1), (i + 1).min(run.len() - 1));
                    let e = run[inx] - run[ip];
                    let out2 = Vec2::new(e.y, -e.x).normalize_or(Vec2::X);
                    let out = ex * out2.x + ez * out2.y;
                    let along = to3(run[inx], *y) - to3(run[ip], *y);
                    let up = to3(rings[jn][k][i], ys[jn]) - to3(rings[jp][k][i], ys[jp]);
                    let mut n = along.cross(up).normalize_or(out);
                    if n.dot(out) < 0.0 {
                        n = -n;
                    }
                    if i > 0 {
                        u += (run[i] - run[i - 1]).length();
                    }
                    let p = to3(run[i], *y);
                    let c = col(p, n);
                    self.vert(p, n, Vec2::new(u, -y), c);
                }
                offs[j] = u;
            }
            for j in 0..ys.len() as u32 - 1 {
                for i in 0..m - 1 {
                    let a = base + j * m + i;
                    self.quad(a, a + 1, a + m + 1, a + m);
                }
            }
        }
    }

    fn translate(&mut self, d: Vec3) {
        for v in &mut self.mesh.vertices {
            v.position = (Vec3::from(v.position) + d).to_array();
        }
    }
}

/// Heights from `y0` to `y1` about `step` apart (both ends included).
fn steps(y0: f32, y1: f32, step: f32) -> Vec<f32> {
    if y1 <= y0 {
        return Vec::new();
    }
    let n = ((y1 - y0) / step).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| y0 + (y1 - y0) * i as f32 / n as f32)
        .collect()
}

// ======================================================================================= parts

/// The tower's materials; each section has one mesh per material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Slip-formed concrete: the shaft, legs, collar, sky deck core.
    Concrete,
    /// Board-formed concrete: the podium, plaza paving and decks.
    Cast,
    /// White aluminum cladding cassettes: pods, soffits, radomes, antenna covers.
    Panel,
    /// Glazing (lit interiors at night).
    Glass,
    /// Painted steel: the mast in aviation orange and white, railings, mullions, plant.
    Steel,
    /// Red aviation obstruction lights.
    Light,
}

impl Part {
    pub const ALL: [Part; 6] = [
        Part::Concrete,
        Part::Cast,
        Part::Panel,
        Part::Glass,
        Part::Steel,
        Part::Light,
    ];
}

struct Parts([Mb; 6]);

impl Parts {
    fn new() -> Self {
        Self(std::array::from_fn(|_| Mb::default()))
    }

    fn get(&mut self, p: Part) -> &mut Mb {
        &mut self.0[p as usize]
    }

    fn finish(self, origin: Vec3) -> Vec<(Part, Mesh)> {
        self.0
            .into_iter()
            .zip(Part::ALL)
            .filter(|(m, _)| !m.mesh.indices.is_empty())
            .map(|(mut m, p)| {
                m.translate(-origin);
                (p, m.mesh)
            })
            .collect()
    }
}

/// A section under construction: near detail, its far stand-in, and fine detail that hides.
struct Sec {
    y: f32,
    near: Parts,
    far: Parts,
    fine: Parts,
}

impl Sec {
    fn new(y: f32) -> Self {
        Self {
            y,
            near: Parts::new(),
            far: Parts::new(),
            fine: Parts::new(),
        }
    }

    fn finish(self, switch: f32, fine_hide: f32) -> SectionMeshes {
        let o = Vec3::Y * self.y;
        SectionMeshes {
            y: self.y,
            switch,
            fine_hide,
            near: self.near.finish(o),
            far: self.far.finish(o),
            fine: self.fine.finish(o),
        }
    }
}

/// One height range of the tower: meshes per material in a frame whose origin is `y` meters up
/// the tower's axis.
pub struct SectionMeshes {
    pub y: f32,
    /// Distance (meters, to the section's origin) beyond which `far` replaces `near`.
    pub switch: f32,
    /// Distance beyond which `fine` hides.
    pub fine_hide: f32,
    pub near: Vec<(Part, Mesh)>,
    pub far: Vec<(Part, Mesh)>,
    pub fine: Vec<(Part, Mesh)>,
}

// ==================================================================================== features

/// Corners of cell `cell` of the glazing atlas for a pane (head at the top).
fn cell_uv(cell: u32) -> [Vec2; 4] {
    let c = Vec2::new((cell % CELLS) as f32, (cell / CELLS) as f32);
    let at = |a: f32, b: f32| (c + Vec2::new(0.04 + 0.92 * a, 0.04 + 0.92 * b)) / CELLS as f32;
    // Sill left, sill right, head right, head left.
    [at(0.0, 1.0), at(1.0, 1.0), at(1.0, 0.0), at(0.0, 0.0)]
}

/// Point at angle `th` on the profile point `q` (radius, height).
fn on(q: Vec2, th: f32) -> Vec3 {
    dir(th) * q.x + Vec3::Y * q.y
}

/// Glazing around the axis between the profile points `a` (sill) and `b` (head): `bays` flat
/// panes over `th0 .. th0 + sweep`, each showing the atlas cell `cell(bay)`. With `frames`,
/// dark mullions divide the panes and run along sill and head.
#[allow(clippy::too_many_arguments)]
fn glazing(
    parts: &mut Parts,
    a: Vec2,
    b: Vec2,
    th0: f32,
    sweep: f32,
    bays: u32,
    cell: &dyn Fn(u32) -> u32,
    frames: bool,
) {
    let d2 = b - a;
    let out2 = Vec2::new(d2.y, -d2.x).normalize();
    for i in 0..bays {
        let (t0, t1) = (
            th0 + sweep * i as f32 / bays as f32,
            th0 + sweep * (i + 1) as f32 / bays as f32,
        );
        let tm = (t0 + t1) * 0.5;
        let n = (dir(tm) * out2.x + Vec3::Y * out2.y).normalize();
        let p = [on(a, t0), on(a, t1), on(b, t1), on(b, t0)];
        parts
            .get(Part::Glass)
            .quad_uv(p, cell_uv(cell(i)), n, Vec3::ONE);
        if frames {
            let st = parts.get(Part::Steel);
            let lift = n * 0.04;
            st.beam(p[0] + lift, p[3] + lift, 0.06, 0.1, n, DARK);
            st.beam(p[0] + lift, p[1] + lift, 0.07, 0.1, n, DARK);
            st.beam(p[3] + lift, p[2] + lift, 0.07, 0.1, n, DARK);
            if i + 1 == bays && sweep < TAU - 1e-3 {
                st.beam(p[1] + lift, p[2] + lift, 0.06, 0.1, n, DARK);
            }
        }
    }
}

/// A circular railing at radius `r` on a floor at height `y`: posts every `post` meters, a
/// handrail and two rails below it.
fn ring_railing(m: &mut Mb, r: f32, y: f32, th0: f32, sweep: f32, post: f32) {
    let n = ((sweep * r / post).ceil() as u32).max(2);
    let pt = |i: u32| dir(th0 + sweep * i as f32 / n as f32) * r;
    for i in 0..=n {
        if sweep >= TAU - 1e-3 && i == n {
            break;
        }
        let q = pt(i) + Vec3::Y * y;
        m.aabb(
            q + Vec3::new(-0.035, 0.0, -0.035),
            q + Vec3::new(0.035, 1.1, 0.035),
            DARK,
        );
    }
    for i in 0..n {
        for h in [1.08, 0.62, 0.15] {
            let (a, b) = (pt(i) + Vec3::Y * (y + h), pt(i + 1) + Vec3::Y * (y + h));
            let hw = if h > 1.0 { 0.035 } else { 0.015 };
            m.beam(a, b, hw, hw, Vec3::Y, DARK);
        }
    }
}

/// A straight railing from `a` to `b` (on the floor), posts about `post` meters apart.
fn railing(m: &mut Mb, a: Vec3, b: Vec3, post: f32) {
    let n = (((b - a).length() / post).ceil() as u32).max(1);
    for i in 0..=n {
        let q = a.lerp(b, i as f32 / n as f32);
        m.aabb(
            q + Vec3::new(-0.03, 0.0, -0.03),
            q + Vec3::new(0.03, 1.1, 0.03),
            DARK,
        );
    }
    for h in [1.08, 0.55] {
        m.beam(
            a + Vec3::Y * h,
            b + Vec3::Y * h,
            0.025,
            0.025,
            Vec3::Y,
            DARK,
        );
    }
}

/// A red aviation light at `at` on a bracket reaching out along `out` from the structure.
/// The far version is a bigger lens, so it still covers a pixel at night from kilometers.
fn aviation_light(s: &mut Sec, at: Vec3, out: Vec3, beacon: bool) {
    let r = if beacon { 0.45 } else { 0.33 };
    let st = s.near.get(Part::Steel);
    st.tube(at - Vec3::Y * 0.3, at, r * 0.8, r * 0.7, 8, DARK, true);
    if out != Vec3::ZERO {
        st.beam(
            at - out * 0.7 - Vec3::Y * 0.2,
            at - Vec3::Y * 0.2,
            0.05,
            0.05,
            Vec3::Y,
            DARK,
        );
    }
    s.near
        .get(Part::Light)
        .sphere(at + Vec3::Y * r * 0.5, r, 4, 8, Vec3::ONE);
    s.far
        .get(Part::Light)
        .sphere(at, if beacon { 1.3 } else { 0.9 }, 2, 4, Vec3::ONE);
}

/// A microwave drum antenna: a white radome disc on a short drum, facing `facing`, on a stand
/// down to `floor`.
fn dish(m: &mut Parts, center: Vec3, facing: Vec3, r: f32, floor: f32) {
    let depth = r * 0.7;
    m.get(Part::Panel).tube(
        center - facing * depth * 0.5,
        center + facing * depth * 0.5,
        r,
        r * 0.97,
        20,
        CLAD,
        true,
    );
    m.get(Part::Steel).tube(
        center - facing * depth * 0.5,
        center - facing * (depth * 0.5 + 0.25),
        r * 0.6,
        r * 0.3,
        12,
        GREY,
        true,
    );
    let foot = Vec3::new(center.x, floor, center.z) - facing * depth * 0.3;
    m.get(Part::Steel).beam(
        foot,
        center - facing * depth * 0.3,
        0.08,
        0.08,
        facing,
        GREY,
    );
}

// ======================================================================================= shaft

/// The concrete core from `y0` to `y1` (radius [`core_r`]).
fn core(s: &mut Sec, y0: f32, y1: f32) {
    let prof = |step: f32| -> Vec<Vec2> {
        steps(y0, y1, step)
            .into_iter()
            .map(|y| Vec2::new(core_r(y), y))
            .collect()
    };
    s.near.get(Part::Concrete).revolve(
        &prof(4.0),
        (SEAM, SEAM + TAU),
        112,
        CONCRETE_SPAN,
        &concrete_tint,
    );
    s.far.get(Part::Concrete).revolve(
        &prof(30.0),
        (SEAM, SEAM + TAU),
        24,
        CONCRETE_SPAN,
        &concrete_tint,
    );
}

/// Each step of a polyline as its own run (creased corners) for [`Mb::loft`].
fn facets(pts: &[Vec2]) -> Vec<Vec<Vec2>> {
    pts.windows(2).map(|w| w.to_vec()).collect()
}

/// A leg's outline at height `y` (x out along the leg, z across): chamfered nose corners, a
/// recessed channel up the nose and two reveals up each side that close in on the nose as the
/// leg tapers. The inner end is buried in the core.
fn leg_outline(y: f32, far: bool) -> Vec<Vec<Vec2>> {
    let (e, w) = (leg_reach(y), leg_half(y));
    let u0 = core_r(y) * 0.5;
    if far {
        return facets(&[
            Vec2::new(u0, -w),
            Vec2::new(e, -w),
            Vec2::new(e, w),
            Vec2::new(u0, w),
        ]);
    }
    let (c, g, d) = (0.12 * w, 0.3 * w, 0.75);
    // The side from the core out to the nose chamfer, flat stretches subdivided so the
    // vertex-color weathering has somewhere to live.
    let xb = (core_r(y) * 0.95).min(e - c);
    let l = e - c - xb;
    let k = (l / 4.0).min(1.0);
    let (gw, dp) = (0.35 * k, 0.09 * k);
    let flat = |x0: f32, x1: f32, n: u32| -> Vec<Vec2> {
        (0..=n)
            .map(|i| Vec2::new(x0 + (x1 - x0) * i as f32 / n as f32, -w))
            .collect()
    };
    let (g2, g1) = (xb + 0.5 * l, xb + 0.82 * l);
    let mut side = vec![flat(u0, g2 - gw, 12)];
    // Shallow V reveals: broad enough to catch the light without shimmering.
    let reveal = |x: f32| {
        facets(&[
            Vec2::new(x - gw, -w),
            Vec2::new(x, -w + dp),
            Vec2::new(x + gw, -w),
        ])
    };
    side.extend(reveal(g2));
    side.push(flat(g2 + gw, g1 - gw, 4));
    side.extend(reveal(g1));
    side.push(flat(g1 + gw, e - c, 3));
    // The other side is the mirror image, walked the other way.
    let mut other: Vec<Vec<Vec2>> = side
        .iter()
        .rev()
        .map(|r| r.iter().rev().map(|p| Vec2::new(p.x, -p.y)).collect())
        .collect();
    side.extend(facets(&[
        Vec2::new(e - c, -w),
        Vec2::new(e, -w + c),
        Vec2::new(e, -g),
        Vec2::new(e - d, -g + 0.15),
        Vec2::new(e - d, g - 0.15),
        Vec2::new(e, g),
        Vec2::new(e, w - c),
        Vec2::new(e - c, w),
    ]));
    side.append(&mut other);
    side
}

/// The three legs from `y0` to `y1`, with lights on their noses at the given heights.
fn legs(s: &mut Sec, y0: f32, y1: f32, lights: &[f32]) {
    let (y0, y1) = (y0.max(-2.0), y1.min(LEG_TOP));
    for &a in &LEGS {
        let (ex, ez) = (dir(a), dir(a + FRAC_PI_2));
        s.near.get(Part::Concrete).loft(
            &steps(y0, y1, 2.5),
            ex,
            ez,
            &|y| leg_outline(y, false),
            &concrete_tint,
        );
        s.far.get(Part::Concrete).loft(
            &steps(y0, y1, 24.0),
            ex,
            ez,
            &|y| leg_outline(y, true),
            &concrete_tint,
        );
        // A cable tray up the channel.
        let (ya, yb) = (y0.max(11.0), y1.min(LEG_TOP - 40.0));
        if yb > ya {
            for k in steps(ya, yb, 12.0).windows(2) {
                let p = |y: f32| ex * (leg_reach(y) - 0.62) + Vec3::Y * y;
                s.fine
                    .get(Part::Steel)
                    .beam(p(k[0]), p(k[1]), 0.22, 0.07, ex, GREY);
            }
        }
        for &y in lights {
            aviation_light(s, ex * (leg_reach(y) - 0.3) + Vec3::Y * y, ex, false);
        }
        // Stair windows up the channel while the leg stands clear of the core, and a door at
        // its foot.
        let bottom = |y: f32| ex * (leg_reach(y) - 0.73) + Vec3::Y * y;
        let mut k = ((y0 - 2.0) / 4.2).ceil().max(3.0) as u32;
        while (k as f32 * 4.2 + 2.0) < y1.min(285.0) {
            let y = k as f32 * 4.2 + 2.0;
            let p = [
                bottom(y - 0.55) - ez * 0.35,
                bottom(y - 0.55) + ez * 0.35,
                bottom(y + 0.55) + ez * 0.35,
                bottom(y + 0.55) - ez * 0.35,
            ];
            let cell = if hash_f(k, (a * 10.0) as u32) < 0.25 {
                13
            } else {
                8 + k % 3
            };
            s.fine
                .get(Part::Glass)
                .quad_uv(p, cell_uv(cell), ex, Vec3::ONE);
            k += 1;
        }
        if y0 < 1.0 {
            s.near.get(Part::Steel).cbox(
                bottom(1.45),
                [ex, Vec3::Y, ez],
                Vec3::new(0.06, 1.37, 0.75),
                GREY * 0.55,
            );
        }
    }
}

/// The glass elevators up the front between concrete fins, from `y0` to `y1`, and the cabs
/// that are in that stretch.
fn elevators(s: &mut Sec, y0: f32, y1: f32) {
    let (y0, y1) = (y0.max(10.8), y1.min(425.0));
    if y1 <= y0 {
        return;
    }
    let (ex, ez) = (dir(FRONT), dir(FRONT + FRAC_PI_2));
    for (z, hw) in [(-2.75, 0.25), (0.0, 0.15), (2.75, 0.25)] {
        let fin = move |y: f32| -> Vec<Vec<Vec2>> {
            let (u0, u1) = (core_r(y) - 0.8, core_r(y) + 1.7);
            facets(&[
                Vec2::new(u0, z - hw),
                Vec2::new(u1, z - hw),
                Vec2::new(u1, z + hw),
                Vec2::new(u0, z + hw),
            ])
        };
        s.near
            .get(Part::Concrete)
            .loft(&steps(y0, y1, 6.0), ex, ez, &fin, &concrete_tint);
    }
    // Far: both shafts as one dark strip between concrete cheeks (thin fins would alias).
    let cheeks = |y: f32| -> Vec<Vec<Vec2>> {
        let (u0, u1) = (core_r(y) - 0.8, core_r(y) + 1.2);
        vec![
            vec![Vec2::new(u0, -3.0), Vec2::new(u1, -3.0)],
            vec![Vec2::new(u1, 3.0), Vec2::new(u0, 3.0)],
        ]
    };
    s.far
        .get(Part::Concrete)
        .loft(&steps(y0, y1, 30.0), ex, ez, &cheeks, &concrete_tint);
    // The shafts' back walls: glazing in 3 m bays, a light every fourth bay.
    let to3 = |u: f32, z: f32, y: f32| ex * u + ez * z + Vec3::Y * y;
    let bays = steps(y0, y1, 3.0);
    for w in bays.windows(2) {
        let gy = (w[0] / 3.0).round() as u32;
        for (lane, (z0, z1)) in [(-2.5f32, -0.15f32), (0.15, 2.5)].into_iter().enumerate() {
            let p = [
                to3(core_r(w[0]) + 0.25, z0, w[0]),
                to3(core_r(w[0]) + 0.25, z1, w[0]),
                to3(core_r(w[1]) + 0.25, z1, w[1]),
                to3(core_r(w[1]) + 0.25, z0, w[1]),
            ];
            let cell = if (gy + lane as u32 * 2).is_multiple_of(4) {
                13
            } else {
                8 + (gy + lane as u32) % 3
            };
            s.near
                .get(Part::Glass)
                .quad_uv(p, cell_uv(cell), ex, Vec3::ONE);
            s.fine
                .get(Part::Steel)
                .beam(p[0] + ex * 0.05, p[1] + ex * 0.05, 0.05, 0.06, ex, DARK);
        }
    }
    for w in steps(y0, y1, 12.0).windows(2) {
        let p = [
            to3(core_r(w[0]) + 1.2, -3.0, w[0]),
            to3(core_r(w[0]) + 1.2, 3.0, w[0]),
            to3(core_r(w[1]) + 1.2, 3.0, w[1]),
            to3(core_r(w[1]) + 1.2, -3.0, w[1]),
        ];
        s.far
            .get(Part::Glass)
            .quad_uv(p, cell_uv(10), ex, Vec3::ONE);
    }
    // Cabs: glass boxes lit inside, riding on the outside of the shaft.
    for (z, y) in [(1.325f32, 64.0f32), (-1.325, 251.0), (1.325, 388.0)] {
        if !(y0..y1 - 3.0).contains(&y) {
            continue;
        }
        let u = core_r(y) + 0.95;
        let c = to3(u, z, y + 1.5);
        let h = Vec3::new(0.6, 1.5, 1.0);
        let ax = [ex, Vec3::Y, ez];
        let st = s.near.get(Part::Steel);
        for sy in [-1.0, 1.0] {
            st.cbox(
                c + Vec3::Y * 1.45 * sy,
                ax,
                Vec3::new(0.64, 0.14, 1.04),
                WHITE,
            );
        }
        for (sx, sz) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            st.cbox(
                c + ex * sx * 0.58 + ez * sz * 0.98,
                ax,
                Vec3::new(0.06, 1.35, 0.06),
                WHITE,
            );
        }
        for (n, side) in [(ex, ez), (ez, -ex), (-ez, ex)] {
            let f = c + n * if n == ex { h.x } else { h.z };
            let hw = if n == ex { h.z } else { h.x };
            let p = [
                f - side * hw - Vec3::Y * 1.35,
                f + side * hw - Vec3::Y * 1.35,
                f + side * hw + Vec3::Y * 1.35,
                f - side * hw + Vec3::Y * 1.35,
            ];
            s.near
                .get(Part::Glass)
                .quad_uv(p, cell_uv(12), n, Vec3::ONE);
        }
        s.far.get(Part::Glass).quad_uv(
            [
                c + ex * h.x - ez * h.z - Vec3::Y * 1.4,
                c + ex * h.x + ez * h.z - Vec3::Y * 1.4,
                c + ex * h.x + ez * h.z + Vec3::Y * 1.4,
                c + ex * h.x - ez * h.z + Vec3::Y * 1.4,
            ],
            cell_uv(12),
            ex,
            Vec3::ONE,
        );
    }
}

// ======================================================================================== foot

/// The paved plaza and the three podium buildings between the legs.
fn foot(s: &mut Sec) {
    // Paving: rings with plan texture coordinates (the form-panel joints read as slab joints).
    let paving = |m: &mut Mb, radii: &[f32], segs: u32, y: f32| {
        for w in radii.windows(2) {
            let base = m.mesh.vertices.len() as u32;
            for j in 0..=segs {
                let a = SEAM + TAU * j as f32 / segs as f32;
                for r in [w[0], w[1]] {
                    let p = dir(a) * r + Vec3::Y * y;
                    // Darker by the legs and buildings, where feet and rain wear it.
                    let near_wall = smooth(PODIUM_R + 6.0, PODIUM_R, r) * 0.1;
                    m.vert(p, Vec3::Y, Vec2::new(p.x, p.z), PAVING * (1.0 - near_wall));
                }
            }
            for j in 0..segs {
                let a = base + j * 2;
                m.quad(a, a + 1, a + 3, a + 2);
            }
        }
    };
    paving(
        s.near.get(Part::Cast),
        &[9.0, 16.0, 22.0, 26.0, PLAZA_R],
        128,
        0.12,
    );
    paving(s.far.get(Part::Cast), &[9.0, PLAZA_R], 32, 0.12);
    let kerb = [
        Vec2::new(PLAZA_R + 0.3, -0.8),
        Vec2::new(PLAZA_R + 0.3, 0.16),
        Vec2::new(PLAZA_R, 0.16),
    ];
    let pave_tint = |_: Vec3, _: Vec3| PAVING * 0.92;
    s.near
        .get(Part::Cast)
        .revolve_crisp(&kerb, (SEAM, SEAM + TAU), 128, CAST_SPAN, &pave_tint);
    s.far
        .get(Part::Cast)
        .revolve_crisp(&kerb, (SEAM, SEAM + TAU), 32, CAST_SPAN, &pave_tint);
    // Bollards round the edge.
    for i in 0..36 {
        let a = SEAM + TAU * (i as f32 + 0.5) / 36.0;
        if LEGS
            .iter()
            .any(|l| (a - l).sin().abs() < 0.2 && (a - l).cos() > 0.0)
        {
            continue;
        }
        let p = dir(a) * (PLAZA_R - 1.0);
        s.fine
            .get(Part::Steel)
            .tube(p, p + Vec3::Y * 0.9, 0.11, 0.11, 10, GREY, true);
    }
    for la in LEGS {
        podium_building(s, la, la + 2.0 * FRAC_PI_3);
    }
}

/// One podium building in the bay between legs at angles `la` and `lb` (counter-clockwise).
fn podium_building(s: &mut Sec, la: f32, lb: f32) {
    let r = PODIUM_R;
    let off = leg_half(0.0) + 1.2;
    let bis = (la + lb) * 0.5;
    // Where the side walls (parallel to the legs, `off` meters from them) meet the arc.
    let t_out = (r * r - off * off).sqrt();
    let da = off.atan2(t_out);
    let (a0, a1) = (la + da, lb - da);
    let sweep = a1 - a0;
    let bays = (sweep * r / 1.8).round() as u32;
    let door = bays / 2;
    let span = (a0, a1);
    // --- the curtain-wall facade, revolved over the arc ---
    let n = s.near.get(Part::Cast);
    n.revolve_crisp(
        &[
            Vec2::new(r + 0.35, -1.5),
            Vec2::new(r + 0.35, 0.6),
            Vec2::new(r - 0.05, 0.6),
        ],
        span,
        48,
        CAST_SPAN,
        &cast_tint,
    );
    n.revolve_crisp(
        &[
            Vec2::new(r - 0.05, 5.0),
            Vec2::new(r + 0.25, 5.0),
            Vec2::new(r + 0.25, 6.3),
            Vec2::new(r - 0.45, 6.3),
        ],
        span,
        48,
        CAST_SPAN,
        &cast_tint,
    );
    n.revolve_crisp(
        &[
            Vec2::new(r - 0.45, 9.4),
            Vec2::new(r + 0.35, 9.4),
            Vec2::new(r + 0.35, 11.0),
            Vec2::new(r - 0.3, 11.0),
            Vec2::new(r - 0.3, 10.5),
        ],
        span,
        48,
        CAST_SPAN,
        &cast_tint,
    );
    let seed = (la * 100.0).abs() as u32;
    glazing(
        &mut s.near,
        Vec2::new(r - 0.05, 0.6),
        Vec2::new(r - 0.05, 5.0),
        a0,
        sweep,
        bays,
        &|i| {
            if i == door || i + 1 == door || hash_f(seed, i) < 0.7 {
                14
            } else {
                15
            }
        },
        true,
    );
    glazing(
        &mut s.near,
        Vec2::new(r - 0.45, 6.3),
        Vec2::new(r - 0.45, 9.4),
        a0,
        sweep,
        bays,
        &|i| {
            if hash_f(seed + 1, i) < 0.55 {
                15
            } else {
                8 + i % 3
            }
        },
        false,
    );
    // Transom over the doors and the shop fronts.
    for i in 0..bays {
        let (t0, t1) = (
            a0 + sweep * i as f32 / bays as f32,
            a0 + sweep * (i + 1) as f32 / bays as f32,
        );
        let q = |t: f32| dir(t) * (r + 0.02) + Vec3::Y * 3.7;
        s.near
            .get(Part::Steel)
            .beam(q(t0), q(t1), 0.06, 0.07, dir((t0 + t1) * 0.5), DARK);
        // Concrete fins shading the office floor.
        let f = dir(t0);
        s.near.get(Part::Cast).cbox(
            f * (r + 0.05) + Vec3::Y * 7.85,
            [f, Vec3::Y, dir(t0 + FRAC_PI_2)],
            Vec3::new(0.5, 1.55, 0.12),
            Vec3::new(0.9, 0.88, 0.84),
        );
    }
    // Far: the same building as a few bands.
    let f = s.far.get(Part::Cast);
    f.revolve_crisp(
        &[
            Vec2::new(r, -1.0),
            Vec2::new(r, 0.6),
            Vec2::new(r - 0.1, 0.6),
        ],
        span,
        10,
        CAST_SPAN,
        &cast_tint,
    );
    f.revolve_crisp(
        &[
            Vec2::new(r - 0.1, 5.0),
            Vec2::new(r, 5.0),
            Vec2::new(r, 6.3),
            Vec2::new(r - 0.1, 6.3),
        ],
        span,
        10,
        CAST_SPAN,
        &cast_tint,
    );
    f.revolve_crisp(
        &[
            Vec2::new(r - 0.1, 9.4),
            Vec2::new(r + 0.2, 9.4),
            Vec2::new(r + 0.2, 11.0),
            Vec2::new(r - 0.3, 11.0),
        ],
        span,
        10,
        CAST_SPAN,
        &cast_tint,
    );
    glazing(
        &mut s.far,
        Vec2::new(r - 0.1, 0.6),
        Vec2::new(r - 0.1, 5.0),
        a0,
        sweep,
        10,
        &|_| 14,
        false,
    );
    glazing(
        &mut s.far,
        Vec2::new(r - 0.1, 6.3),
        Vec2::new(r - 0.1, 9.4),
        a0,
        sweep,
        10,
        &|i| if i % 3 == 1 { 9 } else { 15 },
        false,
    );
    // --- side walls along the legs, and the roof ---
    let t_in = (core_r(0.0).powi(2) - off * off).max(0.0).sqrt() - 0.6;
    for (leg, sign) in [(la, 1.0f32), (lb, -1.0)] {
        let ex = dir(leg);
        let ez = dir(leg + FRAC_PI_2) * sign;
        let len = t_out + 0.4 - t_in;
        let center = ex * (t_in + len * 0.5) + ez * (off + 0.3) + Vec3::Y * 4.75;
        let h = Vec3::new(len * 0.5, 6.25, 0.3);
        s.near
            .get(Part::Cast)
            .cbox(center, [ex, Vec3::Y, ez], h, Vec3::new(0.9, 0.88, 0.84));
        s.far
            .get(Part::Cast)
            .cbox(center, [ex, Vec3::Y, ez], h, Vec3::new(0.9, 0.88, 0.84));
        // Tall slot windows in the side wall.
        for k in 0..3 {
            let t = t_in + len * (0.35 + 0.22 * k as f32);
            let c = ex * t + ez * (off - 0.02) + Vec3::Y * 4.2;
            let p = [
                c - ex * 0.35 - Vec3::Y * 3.0,
                c + ex * 0.35 - Vec3::Y * 3.0,
                c + ex * 0.35 + Vec3::Y * 3.0,
                c - ex * 0.35 + Vec3::Y * 3.0,
            ];
            s.near
                .get(Part::Glass)
                .quad_uv(p, cell_uv(14 + k % 2), -ez, Vec3::ONE);
        }
    }
    let mut roof = Vec::new();
    let side = |leg: f32, sign: f32, t: f32| {
        dir(leg) * t + dir(leg + FRAC_PI_2) * sign * (off + 0.6) + Vec3::Y * 10.5
    };
    roof.push(side(la, 1.0, t_in));
    let (ri, ra0) = (r - 0.3, la + (off + 0.6).atan2(t_out - 0.6));
    let ra1 = lb - (off + 0.6).atan2(t_out - 0.6);
    for i in 0..=16 {
        roof.push(dir(lerp(ra0, ra1, i as f32 / 16.0)) * ri + Vec3::Y * 10.5);
    }
    roof.push(side(lb, -1.0, t_in));
    s.near.get(Part::Cast).poly(&roof, Vec3::Y, ROOFING);
    s.far.get(Part::Cast).poly(&roof, Vec3::Y, ROOFING);
    // Roof plant: air handlers, a skylight.
    for (k, (t, w)) in [(0.35f32, 3.2f32), (0.62, 2.4)].into_iter().enumerate() {
        let c = dir(bis + (k as f32 - 0.5) * 0.5) * lerp(13.0, r - 3.0, t) + Vec3::Y * 10.5;
        s.near.get(Part::Steel).cbox(
            c + Vec3::Y * 0.9,
            [dir(bis), Vec3::Y, dir(bis + FRAC_PI_2)],
            Vec3::new(w * 0.5, 0.9, 1.0),
            GREY,
        );
        s.fine.get(Part::Steel).tube(
            c + Vec3::Y * 1.8,
            c + Vec3::Y * 2.0,
            0.55,
            0.55,
            12,
            DARK,
            true,
        );
    }
    let sk = dir(bis) * 15.5 + Vec3::Y * 10.5;
    s.near.get(Part::Glass).quad_uv(
        [
            sk - dir(bis + FRAC_PI_2) * 4.0 - dir(bis) * 1.0 + Vec3::Y * 0.4,
            sk + dir(bis + FRAC_PI_2) * 4.0 - dir(bis) * 1.0 + Vec3::Y * 0.4,
            sk + dir(bis + FRAC_PI_2) * 4.0 + dir(bis) * 1.0 + Vec3::Y * 0.4,
            sk - dir(bis + FRAC_PI_2) * 4.0 + dir(bis) * 1.0 + Vec3::Y * 0.4,
        ],
        cell_uv(9),
        Vec3::Y,
        Vec3::ONE,
    );
    s.near.get(Part::Cast).cbox(
        sk + Vec3::Y * 0.18,
        [dir(bis), Vec3::Y, dir(bis + FRAC_PI_2)],
        Vec3::new(1.15, 0.2, 4.15),
        Vec3::new(0.9, 0.88, 0.84),
    );
    // --- the entrance: canopy on two columns, steps ---
    let (td0, td1) = (
        a0 + sweep * (door - 1) as f32 / bays as f32,
        a0 + sweep * (door + 1) as f32 / bays as f32,
    );
    let tm = (td0 + td1) * 0.5;
    let (ex, ez) = (dir(tm), dir(tm + FRAC_PI_2));
    let front = ex * r;
    s.near.get(Part::Panel).cbox(
        front + ex * 2.2 + Vec3::Y * 4.75,
        [ex, Vec3::Y, ez],
        Vec3::new(2.4, 0.2, 4.6),
        CLAD,
    );
    s.far.get(Part::Cast).cbox(
        front + ex * 2.2 + Vec3::Y * 4.75,
        [ex, Vec3::Y, ez],
        Vec3::new(2.4, 0.2, 4.6),
        CLAD,
    );
    for side in [-1.0, 1.0] {
        let b = front + ex * 4.0 + ez * side * 3.8;
        s.near
            .get(Part::Steel)
            .tube(b, b + Vec3::Y * 4.55, 0.13, 0.13, 12, DARK, false);
    }
    s.near.get(Part::Cast).cbox(
        front + ex * 1.4 + Vec3::Y * 0.1,
        [ex, Vec3::Y, ez],
        Vec3::new(1.6, 0.1, 4.8),
        PAVING * 1.1,
    );
    // Door frames in the two door bays.
    for t in [td0, tm, td1] {
        let b = dir(t) * (r + 0.02);
        s.near.get(Part::Steel).beam(
            b + Vec3::Y * 0.6,
            b + Vec3::Y * 3.7,
            0.09,
            0.1,
            dir(t),
            DARK,
        );
    }
}

// ====================================================================================== collar

/// The plant collar where the legs end (344–377 m).
fn collar(s: &mut Sec) {
    let full = (SEAM, SEAM + TAU);
    let lip = [
        Vec2::new(11.8, 352.5),
        Vec2::new(11.8, 353.3),
        Vec2::new(11.0, 353.3),
    ];
    let balcony = [
        Vec2::new(11.0, 364.6),
        Vec2::new(13.0, 364.6),
        Vec2::new(13.0, 365.6),
    ];
    for (parts, segs) in [(&mut s.near, 112u32), (&mut s.far, 28)] {
        let c = parts.get(Part::Concrete);
        c.revolve(
            &[Vec2::new(core_r(344.0), 344.0), Vec2::new(11.8, 352.5)],
            full,
            segs,
            CONCRETE_SPAN,
            &concrete_tint,
        );
        c.revolve_crisp(&lip, full, segs, CONCRETE_SPAN, &concrete_tint);
        c.revolve_crisp(&balcony, full, segs, CONCRETE_SPAN, &concrete_tint);
        c.revolve(
            &[Vec2::new(10.6, 370.2), Vec2::new(core_r(377.0), 377.0)],
            full,
            segs,
            CONCRETE_SPAN,
            &concrete_tint,
        );
        parts.get(Part::Steel).revolve(
            &[Vec2::new(11.0, 353.3), Vec2::new(11.0, 364.6)],
            full,
            segs,
            STEEL_SPAN,
            &|_, _| GREY * 0.45,
        );
        parts.get(Part::Cast).revolve(
            &[Vec2::new(13.0, 365.6), Vec2::new(10.6, 365.6)],
            full,
            segs,
            CAST_SPAN,
            &|_, _| PAVING,
        );
        parts.get(Part::Panel).revolve(
            &[Vec2::new(10.6, 365.6), Vec2::new(10.6, 370.2)],
            full,
            segs,
            PANEL_SPAN,
            &panel_tint,
        );
    }
    // Piers between the louvers, louver blades, railing, microwave drums, lights.
    for i in 0..16 {
        let a = SEAM + TAU * i as f32 / 16.0;
        let (ex, ez) = (dir(a), dir(a + FRAC_PI_2));
        s.near.get(Part::Concrete).cbox(
            ex * 11.35 + Vec3::Y * 358.95,
            [ex, Vec3::Y, ez],
            Vec3::new(0.55, 5.65, 0.45),
            concrete_tint(ex * 11.0 + Vec3::Y * 359.0, ex),
        );
        let next = SEAM + TAU * (i as f32 + 1.0) / 16.0;
        for k in 0..18 {
            let y = 353.9 + k as f32 * 0.6;
            let (p, q) = (dir(a + 0.03) * 11.15, dir(next - 0.03) * 11.15);
            s.fine.get(Part::Steel).beam(
                p + Vec3::Y * y,
                q + Vec3::Y * y,
                0.02,
                0.12,
                Vec3::new(0.0, 1.0, 0.0) + dir((a + next) * 0.5) * 0.8,
                GREY * 0.7,
            );
        }
    }
    ring_railing(s.fine.get(Part::Steel), 12.85, 365.6, SEAM, TAU, 2.0);
    for k in 0..4 {
        let a = FRONT + FRAC_PI_2 * 0.5 + FRAC_PI_2 * k as f32;
        dish(
            &mut s.near,
            dir(a) * 11.9 + Vec3::Y * 367.7,
            dir(a),
            1.4,
            365.6,
        );
    }
    for &a in &LEGS {
        aviation_light(s, dir(a) * 12.85 + Vec3::Y * 366.0, dir(a), false);
    }
}

// ========================================================================================= pod

/// The main pod (421–471 m).
fn pod(s: &mut Sec) {
    let full = (SEAM, SEAM + TAU);
    let v = Vec2::new;
    let panel_runs: [&[Vec2]; 5] = [
        &[
            v(21.0, 436.0),
            v(21.6, 436.0),
            v(21.6, 437.4),
            v(21.2, 437.4),
        ],
        &[
            v(23.4, 443.2),
            v(23.9, 443.2),
            v(23.9, 444.6),
            v(23.0, 444.6),
        ],
        &[v(23.0, 450.0), v(25.4, 450.0), v(25.4, 451.2)],
        &[
            v(20.6, 455.2),
            v(22.8, 455.2),
            v(22.8, 456.6),
            v(20.2, 463.0),
            v(20.2, 464.0),
            v(19.6, 464.0),
            v(19.6, 463.4),
        ],
        &[v(19.6, 463.4), v(7.2, 466.4)],
    ];
    for (parts, segs) in [(&mut s.near, 160u32), (&mut s.far, 40)] {
        let p = parts.get(Part::Panel);
        p.revolve(
            &[v(7.4, 421.0), v(21.0, 436.0)],
            full,
            segs,
            PANEL_SPAN,
            &panel_tint,
        );
        for run in panel_runs {
            p.revolve_crisp(run, full, segs, PANEL_SPAN, &panel_tint);
        }
        parts.get(Part::Cast).revolve(
            &[v(25.4, 451.2), v(20.6, 451.2)],
            full,
            segs,
            CAST_SPAN,
            &|_, _| PAVING,
        );
        parts.get(Part::Concrete).revolve_crisp(
            &[v(7.2, 466.4), v(7.2, 470.6), v(5.6, 471.0)],
            full,
            segs / 2,
            CONCRETE_SPAN,
            &concrete_tint,
        );
    }
    // Glazing: observation (leaning out), restaurant, terrace; some rooms dark.
    let levels: [(Vec2, Vec2, u32, u32, f32); 3] = [
        (v(21.2, 437.4), v(23.4, 443.2), 72, 0, 0.08),
        (v(23.0, 444.6), v(23.0, 450.0), 72, 4, 0.15),
        (v(20.6, 451.2), v(20.6, 455.2), 64, 0, 0.1),
    ];
    for (k, &(a, b, bays, base, dark)) in levels.iter().enumerate() {
        let k = k as u32;
        let cell = move |i: u32| {
            let room = hash_f(k * 131 + i / 3, 7);
            if room < dark {
                8 + i % 3
            } else {
                base + (hash_f(k, i) * 4.0) as u32 % 4
            }
        };
        glazing(&mut s.near, a, b, SEAM, TAU, bays, &cell, true);
        glazing(
            &mut s.far,
            a,
            b,
            SEAM,
            TAU,
            40,
            &|i| cell(i * bays / 40),
            false,
        );
    }
    // Small windows in the technical level.
    let (wa, wb) = (v(22.8, 456.6), v(20.2, 463.0));
    let slope = (wb - wa).normalize();
    let wn2 = Vec2::new(slope.y, -slope.x);
    for i in 0..48 {
        let a = SEAM + TAU * (i as f32 + 0.5) / 48.0;
        let n = (dir(a) * wn2.x + Vec3::Y * wn2.y).normalize();
        let c = on(wa.lerp(wb, 0.47), a) + n * 0.03;
        let up = (dir(a) * slope.x + Vec3::Y * slope.y).normalize();
        let side = dir(a + FRAC_PI_2);
        let p = [
            c - side * 0.8 - up * 0.65,
            c + side * 0.8 - up * 0.65,
            c + side * 0.8 + up * 0.65,
            c - side * 0.8 + up * 0.65,
        ];
        let cell = if hash_f(i, 3) < 0.6 { 11 } else { 8 + i % 3 };
        s.near
            .get(Part::Glass)
            .quad_uv(p, cell_uv(cell), n, Vec3::ONE);
    }
    // Ribs under the soffit.
    let (ra, rb) = (v(7.4, 421.0), v(21.0, 436.0));
    let along = (rb - ra).normalize();
    let down2 = Vec2::new(along.y, -along.x);
    for i in 0..24 {
        let a = SEAM + TAU * (i as f32 + 0.5) / 24.0;
        let x = (dir(a) * along.x + Vec3::Y * along.y).normalize();
        let dn = (dir(a) * down2.x + Vec3::Y * down2.y).normalize();
        let mid = on((ra + rb) * 0.5, a) + dn * 0.35;
        let len = (rb - ra).length();
        s.near.get(Part::Panel).cbox(
            mid,
            [x, dn, dir(a + FRAC_PI_2)],
            Vec3::new(len * 0.5, 0.4, 0.2),
            CLAD * 0.95,
        );
    }
    // Terrace railing; aviation lights on the parapet.
    ring_railing(s.fine.get(Part::Steel), 25.15, 451.2, SEAM, TAU, 1.6);
    for i in 0..6 {
        let a = FRONT + TAU * i as f32 / 6.0;
        aviation_light(s, dir(a) * 19.9 + Vec3::Y * 464.3, Vec3::ZERO, false);
    }
    // Roof: microwave dishes, the cleaning gantry on its ring rail, plant, aerials.
    for k in 0..3 {
        let a = FRONT + 0.5 + TAU * k as f32 / 3.0;
        let r = 14.5;
        let floor = 463.4 + (19.6 - r) / (19.6 - 7.2) * 3.0;
        dish(
            &mut s.near,
            dir(a) * r + Vec3::Y * (floor + 1.6),
            dir(a),
            1.2,
            floor,
        );
    }
    let rail = |rr: f32| 463.4 + (19.6 - rr) / (19.6 - 7.2) * 3.0;
    for i in 0..64 {
        let (a0, a1) = (TAU * i as f32 / 64.0, TAU * (i + 1) as f32 / 64.0);
        let y = rail(17.5) + 0.15;
        s.fine.get(Part::Steel).beam(
            dir(a0) * 17.5 + Vec3::Y * y,
            dir(a1) * 17.5 + Vec3::Y * y,
            0.1,
            0.12,
            Vec3::Y,
            GREY,
        );
    }
    let ga = FRONT + 1.9;
    let (gx, gz) = (dir(ga), dir(ga + FRAC_PI_2));
    let st = s.near.get(Part::Steel);
    st.cbox(
        gx * 17.5 + Vec3::Y * (rail(17.5) + 0.9),
        [gx, Vec3::Y, gz],
        Vec3::new(1.2, 0.75, 1.4),
        WHITE * 0.9,
    );
    st.beam(
        gx * 10.0 + Vec3::Y * (rail(17.5) + 3.0),
        gx * 22.8 + Vec3::Y * (rail(17.5) + 3.0),
        0.35,
        0.35,
        Vec3::Y,
        WHITE * 0.9,
    );
    st.beam(
        gx * 17.5 + Vec3::Y * (rail(17.5) + 1.6),
        gx * 17.5 + Vec3::Y * (rail(17.5) + 2.7),
        0.3,
        0.3,
        gx,
        WHITE * 0.9,
    );
    st.cbox(
        gx * 22.6 + Vec3::Y * (rail(17.5) - 0.2),
        [gx, Vec3::Y, gz],
        Vec3::new(0.45, 0.5, 1.6),
        WHITE * 0.85,
    );
    for side in [-1.2, 1.2] {
        s.fine.get(Part::Steel).beam(
            gx * 22.6 + gz * side + Vec3::Y * (rail(17.5) + 0.3),
            gx * 22.6 + gz * side + Vec3::Y * (rail(17.5) + 2.7),
            0.02,
            0.02,
            gx,
            DARK,
        );
    }
    for k in 0..4 {
        let a = FRONT + PI + 0.35 * (k as f32 - 1.5);
        let r = 9.4;
        let c = dir(a) * r + Vec3::Y * (rail(r) + 0.7);
        s.near.get(Part::Steel).cbox(
            c,
            [dir(a), Vec3::Y, dir(a + FRAC_PI_2)],
            Vec3::new(1.0, 0.75, 1.3),
            GREY,
        );
    }
    for k in 0..2 {
        let p = dir(FRONT + 1.0 + PI * k as f32) * 6.2 + Vec3::Y * 470.8;
        s.fine
            .get(Part::Steel)
            .tube(p, p + Vec3::Y * 4.0, 0.06, 0.03, 6, GREY, false);
    }
}

// ==================================================================================== sky deck

/// The upper shaft (466–508 m) and the sky deck (508–526 m) with its open roof.
fn sky_deck(s: &mut Sec) {
    let full = (SEAM, SEAM + TAU);
    let v = Vec2::new;
    let shaft: Vec<Vec2> = steps(466.4, 509.0, 4.0)
        .into_iter()
        .map(|y| v(core_hi(y), y))
        .collect();
    for (parts, segs) in [(&mut s.near, 96u32), (&mut s.far, 24)] {
        let c = parts.get(Part::Concrete);
        c.revolve(&shaft, full, segs, CONCRETE_SPAN, &concrete_tint);
        c.revolve_crisp(
            &[
                v(4.3, 520.4),
                v(4.3, 525.8),
                v(4.7, 525.8),
                v(4.7, MAST_FOOT),
                v(0.0, MAST_FOOT),
            ],
            full,
            segs / 2,
            CONCRETE_SPAN,
            &concrete_tint,
        );
        let p = parts.get(Part::Panel);
        p.revolve(
            &[v(4.6, 508.0), v(9.6, 514.0)],
            full,
            segs,
            PANEL_SPAN,
            &panel_tint,
        );
        p.revolve_crisp(
            &[v(9.6, 514.0), v(9.9, 514.0), v(9.9, 515.0), v(9.5, 515.0)],
            full,
            segs,
            PANEL_SPAN,
            &panel_tint,
        );
        p.revolve_crisp(
            &[v(10.3, 519.2), v(10.9, 519.2), v(10.9, 520.4)],
            full,
            segs,
            PANEL_SPAN,
            &panel_tint,
        );
        parts.get(Part::Cast).revolve(
            &[v(10.9, 520.4), v(4.3, 520.4)],
            full,
            segs,
            CAST_SPAN,
            &|_, _| PAVING,
        );
    }
    let cell = |i: u32| {
        if hash_f(i / 2, 41) < 0.1 {
            9
        } else {
            (hash_f(i, 43) * 4.0) as u32 % 4
        }
    };
    glazing(
        &mut s.near,
        v(9.5, 515.0),
        v(10.3, 519.2),
        SEAM,
        TAU,
        30,
        &cell,
        true,
    );
    glazing(
        &mut s.far,
        v(9.5, 515.0),
        v(10.3, 519.2),
        SEAM,
        TAU,
        15,
        &|i| cell(i * 2),
        false,
    );
    // Ribs under the soffit, stair hut on the deck, railing, lights.
    let (ra, rb) = (v(4.6, 508.0), v(9.6, 514.0));
    let along = (rb - ra).normalize();
    let down2 = Vec2::new(along.y, -along.x);
    for i in 0..12 {
        let a = SEAM + TAU * (i as f32 + 0.5) / 12.0;
        let x = (dir(a) * along.x + Vec3::Y * along.y).normalize();
        let dn = (dir(a) * down2.x + Vec3::Y * down2.y).normalize();
        s.near.get(Part::Panel).cbox(
            on((ra + rb) * 0.5, a) + dn * 0.25,
            [x, dn, dir(a + FRAC_PI_2)],
            Vec3::new((rb - ra).length() * 0.5, 0.28, 0.15),
            CLAD * 0.95,
        );
    }
    let ha = FRONT + PI;
    s.near.get(Part::Cast).cbox(
        dir(ha) * 5.6 + Vec3::Y * 521.8,
        [dir(ha), Vec3::Y, dir(ha + FRAC_PI_2)],
        Vec3::new(1.6, 1.4, 1.4),
        Vec3::new(0.9, 0.88, 0.84),
    );
    s.near.get(Part::Glass).quad_uv(
        [
            dir(ha) * 7.21 - dir(ha + FRAC_PI_2) * 0.5 + Vec3::Y * 520.4,
            dir(ha) * 7.21 + dir(ha + FRAC_PI_2) * 0.5 + Vec3::Y * 520.4,
            dir(ha) * 7.21 + dir(ha + FRAC_PI_2) * 0.5 + Vec3::Y * 522.6,
            dir(ha) * 7.21 - dir(ha + FRAC_PI_2) * 0.5 + Vec3::Y * 522.6,
        ],
        cell_uv(14),
        dir(ha),
        Vec3::ONE,
    );
    ring_railing(s.fine.get(Part::Steel), 10.75, 520.4, SEAM, TAU, 1.5);
    for i in 0..4 {
        let a = FRONT + FRAC_PI_2 * 0.5 + FRAC_PI_2 * i as f32;
        aviation_light(s, dir(a) * 10.6 + Vec3::Y * 520.6, Vec3::ZERO, false);
    }
    for &a in &LEGS {
        aviation_light(s, dir(a) * 4.8 + Vec3::Y * 525.9, dir(a), false);
    }
}

// ======================================================================================== mast

/// The antenna mast (526.6–603 m).
fn mast(s: &mut Sec) {
    let yaw = FRONT + FRAC_PI_2 * 0.5;
    let (ax, az) = (dir(yaw), dir(yaw + FRAC_PI_2));
    let corner = |y: f32, i: usize| {
        let (sx, sz) = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)][i];
        (ax * sx + az * sz) * mast_half(y) + Vec3::Y * y
    };
    let panel_h = (LATTICE_TOP - MAST_FOOT) / PANELS as f32;
    let st = s.near.get(Part::Steel);
    st.cbox(
        Vec3::Y * (MAST_FOOT + 0.2),
        [ax, Vec3::Y, az],
        Vec3::new(2.95, 0.2, 2.95),
        GREY,
    );
    for k in 0..PANELS {
        let (y0, y1) = (
            MAST_FOOT + panel_h * k as f32,
            MAST_FOOT + panel_h * (k + 1) as f32,
        );
        let col = band_color((y0 + y1) * 0.5);
        for i in 0..4 {
            let r = lerp(0.2, 0.13, k as f32 / PANELS as f32);
            st.tube(corner(y0, i), corner(y1, i), r, r, 10, col, false);
            let j = (i + 1) % 4;
            // Horizontal at the panel's top, X-bracing on the face.
            let out = (corner(y1, i) + corner(y1, j)).with_y(0.0).normalize();
            st.beam(corner(y1, i), corner(y1, j), 0.07, 0.09, out, col);
            st.beam(corner(y0, i), corner(y1, j), 0.06, 0.06, out, col);
            st.beam(corner(y0, j), corner(y1, i), 0.06, 0.06, out, col);
        }
    }
    for i in 0..4 {
        let j = (i + 1) % 4;
        let out = (corner(MAST_FOOT, i) + corner(MAST_FOOT, j))
            .with_y(0.0)
            .normalize();
        st.beam(
            corner(MAST_FOOT + 0.4, i),
            corner(MAST_FOOT + 0.4, j),
            0.08,
            0.1,
            out,
            ORANGE,
        );
    }
    // The feeder trunk up the middle, cables to the antennas.
    st.tube(
        Vec3::Y * MAST_FOOT,
        Vec3::Y * LATTICE_TOP,
        0.55,
        0.45,
        14,
        GREY,
        false,
    );
    // Platforms: at two panel levels and on top of the lattice.
    let plat_ys = [
        MAST_FOOT + panel_h * 5.0,
        MAST_FOOT + panel_h * 9.0,
        LATTICE_TOP,
    ];
    for (k, &y) in plat_ys.iter().enumerate() {
        let h = mast_half(y) + if k == 2 { 0.5 } else { 1.0 };
        for parts in [&mut s.near, &mut s.far] {
            parts.get(Part::Steel).cbox(
                Vec3::Y * (y + 0.1),
                [ax, Vec3::Y, az],
                Vec3::new(h, 0.12, h),
                GREY * 0.55,
            );
        }
        let c = |sx: f32, sz: f32| (ax * sx + az * sz) * (h - 0.05) + Vec3::Y * (y + 0.22);
        let fine = s.fine.get(Part::Steel);
        let cs = [c(1.0, 1.0), c(-1.0, 1.0), c(-1.0, -1.0), c(1.0, -1.0)];
        for i in 0..4 {
            railing(fine, cs[i], cs[(i + 1) % 4], 1.4);
        }
        for c in cs {
            let out = c.with_y(0.0).normalize();
            aviation_light(s, c + Vec3::Y * 1.3, out * 0.2, false);
        }
        if k < 2 {
            // Sector antennas: three on each face (the lower platform's dishes take two faces),
            // on pipes off the platform's edge.
            for f in 0..4 {
                if k == 0 && f % 2 == 1 {
                    continue;
                }
                let (n, t) = match f {
                    0 => (ax, az),
                    1 => (az, -ax),
                    2 => (-ax, -az),
                    _ => (-az, ax),
                };
                for m in 0..3 {
                    let p = n * (h - 0.35) + t * (m as f32 - 1.0) * h * 0.6 + Vec3::Y * (y + 1.6);
                    let tilt = Quat::from_axis_angle(t, 0.05);
                    s.fine.get(Part::Panel).cbox(
                        p,
                        [n, tilt * Vec3::Y, t],
                        Vec3::new(0.08, 1.15, 0.17),
                        CLAD,
                    );
                    s.fine.get(Part::Steel).tube(
                        p - n * 0.2 - Vec3::Y * 1.4,
                        p - n * 0.2 + Vec3::Y * 1.4,
                        0.045,
                        0.045,
                        6,
                        GREY,
                        false,
                    );
                    // Feeder cable down to the trunk.
                    s.fine.get(Part::Steel).beam(
                        p - n * 0.2 - Vec3::Y * 1.2,
                        n * 0.5 + Vec3::Y * (y - 2.0),
                        0.025,
                        0.025,
                        Vec3::Y,
                        DARK,
                    );
                }
            }
        }
    }
    // Two drum dishes on the lower platform.
    for f in [az, -az] {
        let y = plat_ys[0];
        dish(
            &mut s.near,
            f * (mast_half(y) + 0.55) + Vec3::Y * (y + 1.3),
            f,
            0.85,
            y + 0.2,
        );
    }
    // The stepped radome cylinder, cut at the paint bands, with flanges and catwalks.
    let sections = [
        (LATTICE_TOP, 572.0, 1.05),
        (572.4, 584.5, 0.85),
        (584.9, 594.5, 0.62),
        (594.8, 599.5, 0.42),
    ];
    let mut prev_r: Option<(f32, f32)> = None;
    for &(y0, y1, r) in &sections {
        if let Some((py, pr)) = prev_r {
            // Conical step from the section below.
            for (parts, segs) in [(&mut s.near, 20u32), (&mut s.far, 8)] {
                parts.get(Part::Steel).revolve(
                    &[Vec2::new(pr, py), Vec2::new(r, y0)],
                    (SEAM, SEAM + TAU),
                    segs,
                    STEEL_SPAN,
                    &|p, _| band_color(p.y),
                );
            }
        }
        // Split at band boundaries so the paint changes crisply.
        let mut cuts = vec![y0];
        for b in 1..7 {
            let yb = MAST_FOOT + BAND * b as f32;
            if yb > y0 + 0.05 && yb < y1 - 0.05 {
                cuts.push(yb);
            }
        }
        cuts.push(y1);
        for w in cuts.windows(2) {
            let col = band_color((w[0] + w[1]) * 0.5);
            for (parts, segs) in [(&mut s.near, 20u32), (&mut s.far, 8)] {
                parts.get(Part::Steel).revolve(
                    &[Vec2::new(r, w[0]), Vec2::new(r, w[1])],
                    (SEAM, SEAM + TAU),
                    segs,
                    STEEL_SPAN,
                    &move |_, _| col,
                );
            }
        }
        // Flange at the top of the section.
        s.near.get(Part::Steel).tube(
            Vec3::Y * (y1 - 0.12),
            Vec3::Y * (y1 + 0.02),
            r + 0.07,
            r + 0.07,
            20,
            GREY,
            true,
        );
        prev_r = Some((y1, r));
    }
    // Catwalks on the steps.
    for &(y, r, w) in &[(572.4f32, 0.85f32, 1.05f32), (584.9, 0.62, 0.8)] {
        for (parts, segs) in [(&mut s.near, 24u32), (&mut s.far, 8)] {
            parts.get(Part::Steel).revolve_crisp(
                &[
                    Vec2::new(r, y - 0.15),
                    Vec2::new(r + w, y - 0.15),
                    Vec2::new(r + w, y),
                    Vec2::new(r, y),
                ],
                (SEAM, SEAM + TAU),
                segs,
                STEEL_SPAN,
                &|_, _| GREY * 0.55,
            );
        }
        ring_railing(s.fine.get(Part::Steel), r + w - 0.05, y, SEAM, TAU, 1.2);
        for k in 0..2 {
            let a = yaw + PI * k as f32;
            aviation_light(s, dir(a) * (r + w) + Vec3::Y * (y + 1.2), Vec3::ZERO, false);
        }
    }
    // Dipole arrays on the middle section.
    for col in 0..4 {
        let a = yaw + FRAC_PI_2 * col as f32;
        for row in 0..5 {
            let y = 574.6 + row as f32 * 1.9;
            let (n, t) = (dir(a), dir(a + FRAC_PI_2));
            let fine = s.fine.get(Part::Steel);
            fine.beam(
                n * 0.8 + Vec3::Y * y,
                n * 1.5 + Vec3::Y * y,
                0.035,
                0.035,
                Vec3::Y,
                GREY,
            );
            fine.beam(
                n * 1.5 - t * 0.6 + Vec3::Y * y,
                n * 1.5 + t * 0.6 + Vec3::Y * y,
                0.03,
                0.03,
                Vec3::Y,
                WHITE,
            );
        }
    }
    // Whip, finial and the top beacon.
    for (parts, sides) in [(&mut s.near, 10u32), (&mut s.far, 4)] {
        parts.get(Part::Steel).tube(
            Vec3::Y * 599.4,
            Vec3::Y * 602.3,
            0.14,
            0.07,
            sides,
            ORANGE,
            true,
        );
    }
    aviation_light(s, Vec3::Y * 602.7, Vec3::ZERO, true);
    s.fine.get(Part::Steel).tube(
        Vec3::Y * 603.0,
        Vec3::Y * 604.6,
        0.025,
        0.01,
        5,
        GREY,
        false,
    );
    // Far: the lattice as a banded square frustum, a touch darker (it is see-through).
    let f = s.far.get(Part::Steel);
    for b in 0..3 {
        let (y0, y1) = (
            MAST_FOOT + BAND * b as f32,
            MAST_FOOT + BAND * (b + 1) as f32,
        );
        let col = band_color((y0 + y1) * 0.5) * 0.8;
        for i in 0..4 {
            let j = (i + 1) % 4;
            let p = [corner(y0, i), corner(y0, j), corner(y1, j), corner(y1, i)];
            let n = (p[0] + p[1]).with_y(0.0).normalize();
            f.poly(&p, n, col);
        }
    }
}

// ===================================================================================== the set

/// Build every section's meshes (the work [`start`] does on a worker thread).
pub fn build() -> Vec<SectionMeshes> {
    let mut out = Vec::new();
    // Foot: plaza, podium, the flare of the legs.
    let mut s = Sec::new(20.0);
    foot(&mut s);
    core(&mut s, -2.0, 64.0);
    legs(&mut s, -2.0, 64.0, &[]);
    elevators(&mut s, 0.0, 64.0);
    out.push(s.finish(1100.0, 260.0));
    // Lower and upper shaft.
    let mut s = Sec::new(132.0);
    core(&mut s, 64.0, 200.0);
    legs(&mut s, 64.0, 200.0, &[118.0]);
    elevators(&mut s, 64.0, 200.0);
    out.push(s.finish(1200.0, 320.0));
    let mut s = Sec::new(273.0);
    core(&mut s, 200.0, 346.0);
    legs(&mut s, 200.0, 346.0, &[236.0]);
    elevators(&mut s, 200.0, 346.0);
    out.push(s.finish(1200.0, 320.0));
    // Collar and the shaft up to the pod.
    let mut s = Sec::new(385.0);
    core(&mut s, 346.0, 424.0);
    collar(&mut s);
    elevators(&mut s, 346.0, 425.0);
    out.push(s.finish(1200.0, 330.0));
    // Pod.
    let mut s = Sec::new(447.0);
    pod(&mut s);
    out.push(s.finish(1300.0, 420.0));
    // Upper shaft and sky deck.
    let mut s = Sec::new(497.0);
    sky_deck(&mut s);
    out.push(s.finish(1100.0, 320.0));
    // Mast: the lattice gives way to a solid stand-in early (thin members alias).
    let mut s = Sec::new(565.0);
    mast(&mut s);
    out.push(s.finish(380.0, 260.0));
    out
}

// ==================================================================================== textures

/// Slip-formed concrete: fine aggregate, faint horizontal drag lines, a pour joint every 3 m
/// with drips below it, bug holes, rain streaks. Covers [`CONCRETE_SPAN`].
fn concrete_tex(size: u32) -> TexSet {
    let s = 1000;
    bake(size, size, Vec2::splat(CONCRETE_SPAN), 0.5, |u, v| {
        let mottle = fbm(u, v, 6, 6, 4, 0.5, s);
        let broad = fbm(u, v, 2, 2, 3, 0.5, s + 1);
        let sand = noise(u * 420.0, v * 420.0, 420, 420, s + 2);
        let grit = noise(u * 160.0, v * 160.0, 160, 160, s + 3);
        let drag = noise(u * 4.0, v * 240.0, 4, 240, s + 4);
        // Pour joints at v = 0 and 0.5; `below` is meters under the joint above.
        let jv = (v * 2.0).fract();
        let below = jv * CONCRETE_SPAN * 0.5;
        let dj = below.min(CONCRETE_SPAN * 0.5 - below);
        let joint = smooth(0.014, 0.004, dj);
        let streak_n = fbm(u, v, 56, 2, 3, 0.55, s + 5);
        let drip = (-below / 1.1).exp();
        let streak = smooth(0.0, 0.5, streak_n) * (0.3 + 0.7 * drip);
        let c = worley(u, v, 96, 96, 1.0, s + 6);
        let hole = if c.id.is_multiple_of(5) {
            smooth(0.2, 0.08, c.f1)
        } else {
            0.0
        };
        let h = mottle * 0.0008 + drag * 0.00025 + grit * 0.0002 + sand * 0.0001
            - joint * 0.004
            - hole * 0.003;
        let albedo = Vec3::new(0.7, 0.69, 0.66)
            * (1.0 + 0.1 * mottle + 0.08 * broad + 0.04 * sand + 0.03 * drag)
            * (1.0 - 0.13 * streak)
            * (1.0 - 0.3 * joint)
            * (1.0 - 0.5 * hole);
        Px::new(
            albedo,
            h,
            0.88 + 0.05 * grit - 0.05 * streak,
            1.0 - 0.6 * hole - 0.3 * joint,
        )
    })
}

/// Board-formed concrete: 2.4 × 1.2 m form panels with their fins and a faint wood grain,
/// mottling, bug holes, streaks under the joints. Covers [`CAST_SPAN`].
fn cast_tex(size: u32) -> TexSet {
    let s = 1100;
    bake(size, size, Vec2::splat(CAST_SPAN), 0.5, |u, v| {
        let (pu, pv) = (u * 2.0, v * 4.0);
        let (iu, iv) = (pu.floor(), pv.floor());
        let (fu, fv) = (pu - iu, pv - iv);
        let id = hash(iu as i32, iv as i32, s);
        let edge = (fu.min(1.0 - fu) * 2.4).min(fv.min(1.0 - fv) * 1.2);
        let joint = smooth(0.007, 0.002, edge);
        let tone = bits01(id, 0) - 0.5;
        let grain = noise(u * 5.0 + bits01(id, 8) * 5.0, v * 160.0, 5, 160, s + 1);
        let mottle = fbm(u, v, 5, 5, 4, 0.5, s + 2);
        let sand = noise(u * 380.0, v * 380.0, 380, 380, s + 3);
        let streak_n = fbm(u, v, 40, 2, 3, 0.5, s + 4);
        let streak = smooth(0.05, 0.6, streak_n) * (0.4 + 0.6 * (-fv * 1.2 / 0.6).exp());
        let c = worley(u, v, 70, 70, 1.0, s + 5);
        let hole = if c.id.is_multiple_of(4) {
            smooth(0.16, 0.06, c.f1)
        } else {
            0.0
        };
        let h = mottle * 0.0006 + grain * 0.00025 + sand * 0.0001 + joint * 0.0015 - hole * 0.002;
        let albedo = Vec3::new(0.7, 0.68, 0.64)
            * (1.0 + 0.05 * tone + 0.06 * mottle + 0.03 * sand + 0.025 * grain)
            * (1.0 - 0.1 * streak)
            * (1.0 - 0.15 * joint)
            * (1.0 - 0.45 * hole);
        Px::new(
            albedo,
            h,
            0.85 + 0.05 * mottle,
            1.0 - 0.5 * hole - 0.2 * joint,
        )
    })
}

/// Aluminum cladding cassettes 1.5 × 1 m: open joints, rounded edges, a little oil-canning,
/// tone per cassette, dirt washing down from the joints. Covers [`PANEL_SPAN`].
fn panel_tex(size: u32) -> TexSet {
    let s = 1200;
    bake(size, size, Vec2::splat(PANEL_SPAN), 0.4, |u, v| {
        let (pu, pv) = (u * 4.0, v * 6.0);
        let (iu, iv) = (pu.floor(), pv.floor());
        let (fu, fv) = (pu - iu, pv - iv);
        let id = hash(iu as i32, iv as i32, s);
        let edge = (fu.min(1.0 - fu) * 1.5).min(fv.min(1.0 - fv) * 1.0);
        let joint = smooth(0.008, 0.005, edge);
        let round = smooth(0.02, 0.008, edge);
        let bulge = fu * (1.0 - fu) * fv * (1.0 - fv) * 16.0 * (0.5 + 0.5 * bits01(id, 16));
        let tone = bits01(id, 0) - 0.5;
        let mottle = fbm(u, v, 6, 6, 3, 0.5, s + 1);
        let streak_n = fbm(u, v, 64, 3, 3, 0.5, s + 2);
        let streak = smooth(0.1, 0.6, streak_n) * (-fv / 0.35).exp();
        let h = bulge * 0.001 - round * 0.0015 - joint * 0.006;
        let albedo = Vec3::new(0.86, 0.87, 0.88)
            * (1.0 + 0.035 * tone + 0.02 * mottle)
            * (1.0 - 0.12 * streak)
            * (1.0 - 0.8 * joint);
        Px::new(
            albedo,
            h,
            0.3 + 0.08 * bits01(id, 8) + 0.15 * streak + 0.03 * mottle,
            1.0 - 0.7 * joint,
        )
    })
}

/// Painted steel: orange-peel, chips down to primer and rust, streaks. Nearly white; vertex
/// colors paint it. Covers [`STEEL_SPAN`].
fn steel_tex(size: u32) -> TexSet {
    let s = 1300;
    bake(size, size, Vec2::splat(STEEL_SPAN), 0.3, |u, v| {
        let peel = noise(u * 150.0, v * 150.0, 150, 150, s);
        let wear =
            fbm(u, v, 6, 6, 5, 0.55, s + 1) + 0.25 * noise(u * 90.0, v * 90.0, 90, 90, s + 2);
        let chip = smooth(0.42, 0.47, wear);
        let rust = chip * smooth(0.5, 0.65, wear);
        let streak = smooth(0.1, 0.6, fbm(u, v, 24, 2, 3, 0.5, s + 3));
        let paint = Vec3::splat(0.92) * (1.0 + 0.015 * peel);
        let albedo = paint
            .lerp(Vec3::new(0.42, 0.4, 0.38), chip)
            .lerp(Vec3::new(0.3, 0.13, 0.05), rust)
            * (1.0 - 0.1 * streak);
        Px::new(
            albedo,
            peel * 0.00003 - chip * 0.00015,
            0.42 + 0.08 * streak + 0.35 * rust + 0.1 * chip,
            1.0 - 0.15 * chip,
        )
    })
}

/// A person against the light inside a window cell (1 inside the silhouette).
fn people(fu: f32, fv: f32, id: u32) -> f32 {
    let mut m: f32 = 0.0;
    for k in 0..2 {
        if bits01(id, 4 + k * 8) < 0.45 {
            continue;
        }
        let x = 0.15 + 0.7 * bits01(id, 8 + k * 8);
        let h = 0.85 + 0.15 * bits01(id, 12 + k * 5);
        let body = ((fu - x) / 0.07).powi(2) + ((fv - (1.0 - 0.22 * h)) / (0.24 * h)).powi(2);
        let head = ((fu - x) / 0.04).powi(2) + ((fv - (1.0 - 0.5 * h)) / 0.05).powi(2);
        m = m.max(smooth(1.1, 0.9, body)).max(smooth(1.1, 0.9, head));
    }
    m
}

/// What window cell `cell` of the glazing atlas shows at night (linear, about 0..1): `fu`
/// across the pane, `fv` down from its head.
fn interior(cell: u32, fu: f32, fv: f32) -> Vec3 {
    let id = hash(cell as i32, 5, 1407);
    let r = |k: u32| bits01(id, k);
    let spot =
        |x: f32, y: f32, rad: f32| (-((fu - x).powi(2) + (fv - y).powi(2)) / (rad * rad)).exp();
    match cell {
        // Observation deck: bright, warm-white, downlights along the ceiling, visitors.
        0..=3 => {
            let warm = Vec3::new(1.0, 0.8, 0.58).lerp(Vec3::new(1.0, 0.9, 0.78), r(0));
            let lights =
                smooth(0.25, 0.0, ((fu * 3.0 + r(8)).fract() - 0.5).abs()) * smooth(0.16, 0.03, fv);
            let room = 0.3 + 0.28 * (1.0 - fv) + 0.1 * smooth(0.85, 1.0, fv);
            warm * (room + 0.8 * lights) * (1.0 - 0.85 * people(fu, fv, id))
        }
        // Restaurant: dim amber, table lamps, a cove light along the ceiling.
        4..=7 => {
            let amber = Vec3::new(1.0, 0.56, 0.26);
            let lamps = spot(0.25 + 0.2 * r(0), 0.66, 0.07) + spot(0.6 + 0.25 * r(8), 0.64, 0.06);
            let cove = smooth(0.08, 0.02, fv) * 0.6;
            amber * (0.1 + 0.1 * (1.0 - fv) + 0.9 * lamps + cove) * (1.0 - 0.6 * people(fu, fv, id))
        }
        // Dark rooms, a corridor glow far inside.
        8..=10 => Vec3::new(0.008, 0.007, 0.006) * (1.0 + 2.0 * spot(0.3 + 0.4 * r(0), 0.5, 0.25)),
        // Technical floor: cool and dim, a few screens.
        11 => {
            let screen =
                smooth(0.06, 0.04, (fu - 0.35).abs()) * smooth(0.08, 0.05, (fv - 0.55).abs());
            Vec3::new(0.55, 0.7, 1.0) * (0.07 + 0.6 * screen)
        }
        // Elevator cab: evenly lit, a passenger or two.
        12 => Vec3::new(1.0, 0.9, 0.76) * (0.95 - 0.3 * fv) * (1.0 - 0.8 * people(fu, fv, id)),
        // Elevator shaft: dark, one fitting near the head.
        13 => {
            Vec3::new(1.0, 0.86, 0.7)
                * (0.004 + 1.2 * spot(0.5, 0.12, 0.07) + 0.08 * spot(0.5, 0.12, 0.3))
        }
        // Lobby: warm, double height, a row of pendant lights.
        14 => {
            let pend =
                smooth(0.25, 0.0, ((fu * 2.0 + 0.5).fract() - 0.5).abs()) * spot(0.5, 0.12, 0.6);
            Vec3::new(1.0, 0.76, 0.5)
                * (0.4 + 0.25 * (1.0 - fv) + 0.6 * pend)
                * (1.0 - 0.7 * people(fu, fv, id))
        }
        // Offices: cool white ceiling panels in rows.
        _ => {
            let rows = smooth(0.3, 0.1, ((fu * 2.0).fract() - 0.5).abs()) * smooth(0.14, 0.04, fv);
            Vec3::new(0.86, 0.93, 1.0) * (0.3 + 0.25 * (1.0 - fv) + 0.6 * rows)
        }
    }
}

/// Glazing: an atlas of [`CELLS`] × [`CELLS`] panes, a faint roller wave, grime at the edges,
/// and a matching emission map of what is behind each pane at night ([`interior`]).
fn glass_tex(size: u32) -> TexSet {
    let s = 1400;
    let mut set = bake(size, size, Vec2::splat(8.0), 0.0, |u, v| {
        let (cu, cv) = ((u * CELLS as f32).fract(), (v * CELLS as f32).fract());
        let wave = noise(u * 4.0, v * 48.0, 4, 48, s);
        let edge = smooth(0.1, 0.0, cu.min(1.0 - cu).min(cv.min(1.0 - cv)));
        let smudge = smooth(0.2, 0.8, fbm(u, v, 8, 8, 3, 0.5, s + 1)) * 0.08 + edge * 0.12;
        Px::new(Vec3::ONE, wave * 0.0002, 0.05 + smudge, 1.0)
    });
    let n = size as usize;
    let mut em = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (
                (x as f32 + 0.5) / size as f32,
                (y as f32 + 0.5) / size as f32,
            );
            let (cx, cy) = ((u * CELLS as f32) as u32, (v * CELLS as f32) as u32);
            // The pane uses the middle 92% of its cell.
            let fu = ((u * CELLS as f32 - cx as f32 - 0.04) / 0.92).clamp(0.0, 1.0);
            let fv = ((v * CELLS as f32 - cy as f32 - 0.04) / 0.92).clamp(0.0, 1.0);
            let c = interior(cy * CELLS + cx, fu, fv);
            em.extend_from_slice(&[srgb8(c.x), srgb8(c.y), srgb8(c.z), 255]);
        }
    }
    let mut emissive = Image::new(size, size, em, true);
    emissive.generate_mips();
    set.emissive = Some(emissive);
    set
}

/// Every tower texture.
pub struct TowerTextures {
    pub concrete: TexSet,
    pub cast: TexSet,
    pub panel: TexSet,
    pub glass: TexSet,
    pub steel: TexSet,
}

/// Generate the textures in parallel (`size` for the big ones, half for the rest).
pub fn textures(size: u32) -> TowerTextures {
    std::thread::scope(|sc| {
        let concrete = sc.spawn(|| concrete_tex(size));
        let panel = sc.spawn(|| panel_tex(size));
        let cast = sc.spawn(|| cast_tex(size / 2));
        let glass = sc.spawn(|| glass_tex(size / 2));
        let steel = steel_tex(size / 2);
        let j = |h: std::thread::ScopedJoinHandle<'_, TexSet>| h.join().expect("texture thread");
        TowerTextures {
            concrete: j(concrete),
            cast: j(cast),
            panel: j(panel),
            glass: j(glass),
            steel,
        }
    })
}

// ================================================================================= assets/spawn

/// The tower's meshes and materials, ready to spawn. The textures are still baking: the
/// materials point at reserved images until [`TowerKit::finish_textures`] fills them.
pub struct TowerKit {
    mats: [Handle<StandardMaterial>; 6],
    sections: Vec<SectionHandles>,
    pending: Mutex<Option<PendingTextures>>,
}

/// The texture bake and the reserved image slots it fills (albedo, normal and ORM of each
/// set in [`TowerTextures`] order, then the glass's emission map).
struct PendingTextures {
    job: JoinHandle<TowerTextures>,
    slots: Vec<Handle<Image>>,
}

struct SectionHandles {
    y: f32,
    switch: f32,
    fine_hide: f32,
    /// Near mesh per material, with the far mesh that replaces it (none: it hides instead).
    near: Vec<(Part, Handle<Mesh>, Option<Handle<Mesh>>)>,
    fine: Vec<(Part, Handle<Mesh>)>,
}

/// The tower being built on worker threads: meshes (quick) and textures (most of the work).
pub struct TowerJob {
    meshes: JoinHandle<(Vec<SectionMeshes>, f32)>,
    textures: JoinHandle<TowerTextures>,
}

/// Start building the tower's meshes and textures on worker threads.
pub fn start() -> TowerJob {
    TowerJob {
        meshes: std::thread::spawn(|| {
            let t0 = std::time::Instant::now();
            let sections = build();
            (sections, t0.elapsed().as_secs_f32())
        }),
        textures: std::thread::spawn(|| textures(1024)),
    }
}

impl TowerJob {
    /// True once the meshes are built ([`TowerJob::finish`] won't wait).
    pub fn is_ready(&self) -> bool {
        self.meshes.is_finished()
    }

    /// Wait for the meshes and add them as assets, with the materials. The textures keep
    /// baking; [`TowerKit::finish_textures`] collects them.
    pub fn finish(
        self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> TowerKit {
        let t0 = std::time::Instant::now();
        let (sections, built) = self.meshes.join().expect("tower mesh thread panicked");
        let tris = |v: &[(Part, Mesh)]| v.iter().map(|(_, m)| m.triangle_count()).sum::<usize>();
        let (near, far): (usize, usize) = sections.iter().fold((0, 0), |(n, f), s| {
            (n + tris(&s.near) + tris(&s.fine), f + tris(&s.far))
        });
        engine::core::log::info!(
            "tower: {near} triangles near, {far} far, meshes built in {built:.2} s (waited {:.2} s)",
            t0.elapsed().as_secs_f32()
        );
        let mut slots = Vec::new();
        let mut reserve = |set: &str| {
            let h = ["albedo", "normal", "orm"]
                .map(|map| images.reserve_keyed(&format!("showcase/tower/{set}/{map}")));
            slots.extend_from_slice(&h);
            h
        };
        let concrete = reserve("concrete");
        let cast = reserve("cast");
        let panel = reserve("panel");
        let glass = reserve("glass");
        let steel = reserve("steel");
        let glow = images.reserve_keyed("showcase/tower/glass/emissive");
        slots.push(glow);
        let textured = |set: &[Handle<Image>; 3], span: f32| {
            let mut m = StandardMaterial::color(1.0, 1.0, 1.0)
                .with_roughness(1.0)
                .with_base_color_texture(set[0])
                .with_normal_texture(set[1])
                .with_metallic_roughness_texture(set[2])
                .with_occlusion_texture(set[2])
                .with_uv_scale(1.0 / span, 1.0 / span);
            m.occlusion_strength = 1.0;
            m
        };
        let mut glass_mat = StandardMaterial::color(0.04, 0.05, 0.06)
            .with_roughness(1.0)
            .with_normal_texture(glass[1])
            .with_metallic_roughness_texture(glass[2])
            .with_emissive([1.0, 1.0, 1.0], 22.0);
        glass_mat.emissive_texture = Some(glow);
        glass_mat.reflectance = 0.6;
        glass_mat.receives_wetness = false;
        let mats = [
            materials.add(textured(&concrete, CONCRETE_SPAN)),
            materials.add(textured(&cast, CAST_SPAN)),
            materials.add(textured(&panel, PANEL_SPAN)),
            materials.add(glass_mat),
            materials.add(textured(&steel, STEEL_SPAN)),
            materials.add(
                StandardMaterial::hex(0x400404)
                    .with_roughness(0.25)
                    .with_emissive([1.0, 0.045, 0.012], 1500.0),
            ),
        ];
        let sections = sections
            .into_iter()
            .map(|s| {
                let mut far: Vec<(Part, Handle<Mesh>)> =
                    s.far.into_iter().map(|(p, m)| (p, meshes.add(m))).collect();
                SectionHandles {
                    y: s.y,
                    switch: s.switch,
                    fine_hide: s.fine_hide,
                    near: s
                        .near
                        .into_iter()
                        .map(|(p, m)| {
                            let f = far
                                .iter()
                                .position(|(q, _)| *q == p)
                                .map(|i| far.remove(i).1);
                            (p, meshes.add(m), f)
                        })
                        .collect(),
                    fine: s
                        .fine
                        .into_iter()
                        .map(|(p, m)| (p, meshes.add(m)))
                        .collect(),
                }
            })
            .collect();
        TowerKit {
            mats,
            sections,
            pending: Mutex::new(Some(PendingTextures {
                job: self.textures,
                slots,
            })),
        }
    }
}

impl TowerKit {
    /// Wait for the textures (they bake while the world spawns) and fill the reserved images.
    /// Call once the rest of the world is built; later calls do nothing.
    pub fn finish_textures(&self, images: &mut Assets<Image>) {
        let Some(p) = self.pending.lock().ok().and_then(|mut p| p.take()) else {
            return;
        };
        let t0 = std::time::Instant::now();
        let mut t = p.job.join().expect("tower texture thread panicked");
        let glow = t.glass.emissive.take().expect("glass has an emission map");
        let mut maps = Vec::new();
        for set in [t.concrete, t.cast, t.panel, t.glass, t.steel] {
            maps.extend([set.albedo, set.normal, set.orm]);
        }
        maps.push(glow);
        for (slot, image) in p.slots.into_iter().zip(maps) {
            images.set(slot, image);
        }
        engine::core::log::info!(
            "tower: textures ready (waited {:.2} s)",
            t0.elapsed().as_secs_f32()
        );
    }
}

/// Spawn the tower with its foot at `base` (ground level under the axis).
pub fn spawn(commands: &mut Commands, kit: &TowerKit, base: DVec3) {
    for s in &kit.sections {
        let tf = Transform::from_translation(base + DVec3::new(0.0, s.y as f64, 0.0));
        for &(part, near, far) in &s.near {
            let mut renderer = MeshRenderer::new(near, kit.mats[part as usize]);
            if part == Part::Light {
                renderer = renderer.without_shadows();
            }
            commands.spawn((
                tf,
                renderer,
                Lod {
                    near,
                    far,
                    switch: s.switch,
                    hide: f32::INFINITY,
                    far_shown: false,
                },
            ));
        }
        for &(part, fine) in &s.fine {
            commands.spawn((
                tf,
                MeshRenderer::new(fine, kit.mats[part as usize]),
                Lod::hide_beyond(fine, s.fine_hide),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &Mesh, name: &str) {
        assert!(!m.indices.is_empty(), "{name} is empty");
        assert!(m.indices.len().is_multiple_of(3), "{name}: not triangles");
        assert!(
            m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
            "{name} has bad indices"
        );
        assert!(
            m.vertices.iter().all(|v| v
                .position
                .iter()
                .chain(&v.normal)
                .chain(&v.uv)
                .chain(&v.color)
                .all(|c| c.is_finite())),
            "{name} has NaNs"
        );
    }

    #[test]
    fn tower_meshes_are_valid_and_within_budget() {
        let sections = build();
        let (mut near, mut far, mut top, mut lights) = (0, 0, 0.0f32, 0);
        for s in &sections {
            for (kind, list) in [("near", &s.near), ("far", &s.far), ("fine", &s.fine)] {
                for (part, m) in list {
                    check(m, &format!("section {} {kind} {part:?}", s.y));
                }
            }
            for (p, _) in &s.far {
                assert!(
                    s.near.iter().any(|(q, _)| q == p),
                    "section {}: far {p:?} has no near mesh to replace",
                    s.y
                );
            }
            near += s
                .near
                .iter()
                .chain(&s.fine)
                .map(|(_, m)| m.triangle_count())
                .sum::<usize>();
            far += s.far.iter().map(|(_, m)| m.triangle_count()).sum::<usize>();
            for (p, m) in &s.near {
                for v in &m.vertices {
                    top = top.max(v.position[1] + s.y);
                }
                if *p == Part::Light {
                    lights += m.vertices.len();
                }
            }
        }
        assert!((598.0..610.0).contains(&top), "top at {top} m");
        assert!(lights > 0, "no aviation lights");
        assert!(near < 600_000, "{near} triangles near");
        assert!(far < 30_000, "{far} triangles far");
        assert!(
            far * 10 < near,
            "far ({far}) should be much cheaper than near ({near})"
        );
    }

    #[test]
    fn faces_point_where_their_normals_say() {
        for s in build() {
            for (part, m) in s.near.iter().chain(&s.far).chain(&s.fine) {
                let mut bad = 0;
                for tri in m.indices.as_chunks::<3>().0 {
                    let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                    let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
                    if n.length_squared() < 1e-10 {
                        continue;
                    }
                    let vn: Vec3 = tri
                        .iter()
                        .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                        .sum();
                    if n.dot(vn) < 0.0 {
                        bad += 1;
                    }
                }
                assert!(
                    bad * 100 < m.indices.len() / 3,
                    "section {} {part:?}: {bad} of {} triangles wound backwards",
                    s.y,
                    m.indices.len() / 3
                );
            }
        }
    }

    #[test]
    fn proportions_are_believable() {
        // A footprint of about 50 m at the ground, a slim upper shaft, the pod up high.
        assert!((25.0..30.0).contains(&leg_reach(0.0)));
        assert!(leg_reach(LEG_TOP) < core_r(LEG_TOP));
        assert!(core_r(400.0) < 8.5 && core_hi(500.0) < 5.5);
        // Outward normals on the shaft point away from the axis.
        let sections = build();
        let (_, m) = sections[1]
            .near
            .iter()
            .find(|(p, _)| *p == Part::Concrete)
            .unwrap();
        let outward = m
            .vertices
            .iter()
            .filter(|v| {
                let p = Vec3::from(v.position);
                Vec3::from(v.normal).dot(Vec3::new(p.x, 0.0, p.z)) > 0.0
            })
            .count();
        assert!(outward * 10 > m.vertices.len() * 9);
    }

    #[test]
    fn stands_on_level_ground() {
        let land = crate::landscape::shared();
        let c = crate::zones::zone("spaceport").center + DVec3::new(70.0, 0.0, 60.0);
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        for i in 0..64 {
            for r in [5.0, 15.0, 25.0, PLAZA_R as f64] {
                let a = i as f64 / 64.0 * std::f64::consts::TAU;
                let h = land.height(c.x + a.cos() * r, c.z + a.sin() * r);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        // The paving is 0.12 m up and its kerb reaches 0.8 m down.
        assert!(
            hi - c.y < 0.11 && c.y - lo < 0.6,
            "ground under the plaza from {lo:.2} to {hi:.2} m"
        );
    }

    #[test]
    fn textures_are_sane() {
        let t = textures(32);
        for set in [&t.concrete, &t.cast, &t.panel, &t.glass, &t.steel] {
            assert!(set.albedo.srgb && !set.normal.srgb && !set.orm.srgb);
            assert!(set.normal.data.chunks(4).all(|p| p[2] > 128));
        }
        assert!(t.glass.emissive.is_some());
        // Lit cells glow, dark ones barely.
        assert!(interior(1, 0.5, 0.3).x > 0.2);
        assert!(interior(9, 0.5, 0.5).x < 0.05);
    }
}
