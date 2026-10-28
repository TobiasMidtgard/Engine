//! Hair as strands: scalp hair, brows, lashes and beards, grown on the built skin.
//!
//! Every hair is a polyline of `points_per` points in model space (the character's own frame:
//! Y up, facing +Z), drawn by the shaders as a thin ribbon facing the camera. Roots are scattered
//! over the skin where a region mask says hair grows (scalp, brows, beard) or along the eyelid
//! margins (lashes), and each style decides where a hair heads from its root: brows lie along the
//! brow in a herringbone, lashes curl up and out, a short crop is combed back from the crown,
//! long hair falls under gravity over the head and shoulders. Long hair grows from *guides*:
//! nearby hairs follow one guide more closely toward their tips, which forms the clumps real
//! hair has. Hanging guides are then settled by a short simulation (gravity, bending, the skin
//! and the face) so they drape over the head and shoulders. Hairs never pass into the skin: each point is pushed back out over the nearest
//! skin vertex.
//!
//! Shading needs to know how deep in the hair each point sits (light reaching the inside of a
//! hairdo is dimmed by the hair around it): after growing, every point's occlusion is estimated
//! from a density grid of all points, marched outward from the head. A finer grid of the hair
//! and body ([`shadow_grid`]) goes to the GPU, where each light is marched through it, so the
//! hair a light reaches through the rest of the hairdo (or past the head) is dimmed.

use std::collections::HashMap;

use engine_core::math::{Quat, UVec3, Vec3};

use crate::appearance::{Appearance, AppearanceExt, BeardStyle, HairStyle, Rng};
use crate::mesh::{
    BROW_CENTER, BROW_HALF, BROW_INNER, BROW_LENGTH, Landmarks, MASKS, Mask, SkinMesh, lerp_table,
    smoothstep,
};

/// Scales every hair's width. Scalp hairs are drawn near their true width and count (about
/// 100-150 μm wide and 130-190 per cm², where a real scalp has 70 μm and ~150): hairs a few
/// times too thick and too few merge into sheets with hard edges, the look of a wig.
const HAIR_WIDTH: f32 = 1.0;

/// Which kind of hair a set holds (the shader colors and shades each its own way).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrandKind {
    Scalp = 0,
    Brows = 1,
    Lashes = 2,
    Beard = 3,
}

/// Hairs of one kind, all with the same number of points.
#[derive(Debug, Clone, PartialEq)]
pub struct StrandSet {
    pub kind: StrandKind,
    pub points_per: u32,
    /// Per point: model-space position (m) and radius (m).
    pub points: Vec<[f32; 4]>,
    /// Per point: occlusion (0 dark .. 1 open), the hair's random value (0..1, picks grey
    /// hairs and color variation), position along the hair (0 root .. 1 tip), and how fine
    /// the hair is (scalp hair: 0 full .. 1 the finest hairline hair).
    pub attrs: Vec<[f32; 4]>,
    /// Hair that moves (long and medium scalp hair): the guides it grew from, `points_per`
    /// model-space points each, root first. Handed to the hair simulation as
    /// `engine_world::HairGuides`; empty for hair that holds its shape.
    pub guides: Vec<Vec3>,
    /// Per hair, the guides it moves with. Empty without guides.
    pub follow: Vec<Follow>,
}

/// A [`StrandSet`] packed for the GPU ([`StrandSet::packed`]).
#[derive(Debug, Clone, Default)]
pub struct PackedStrands {
    /// Per point: x and y, then z and radius, 16-bit unorms.
    pub points: Vec<[u32; 2]>,
    /// Per point: occlusion and the hair's random value, 16-bit unorms.
    pub attrs: Vec<u32>,
    /// The box the positions are in (corner; w: the largest radius) and its size.
    pub box_min: [f32; 4],
    pub box_size: [f32; 4],
}

/// The guides one hair moves with: up to three, nearest root first, and how much of each
/// guide's motion it takes (summing to 1). Blending neighbors keeps a lock from splitting
/// into thin strips where two guides swing apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    /// Guide indices (`u32::MAX`: none).
    pub guides: [u32; 3],
    pub weights: [f32; 3],
}

impl Follow {
    pub const NONE: Follow = Follow {
        guides: [u32::MAX; 3],
        weights: [0.0; 3],
    };
}

impl StrandSet {
    fn new(kind: StrandKind, points_per: u32) -> Self {
        Self {
            kind,
            points_per,
            points: Vec::new(),
            attrs: Vec::new(),
            guides: Vec::new(),
            follow: Vec::new(),
        }
    }

    pub fn strand_count(&self) -> usize {
        self.points.len() / self.points_per.max(1) as usize
    }

    /// Guides of hair that moves (see [`Self::guides`]).
    pub fn guide_count(&self) -> usize {
        self.guides.len() / self.points_per.max(1) as usize
    }

    /// The set as the GPU reads it, a third of the bytes: each point's position as 16-bit
    /// unorms in the set's box (under 0.02 mm steps for a head of hair) and its radius as one of
    /// the largest; occlusion and the hair's random value as 16-bit unorms (all the shader reads
    /// of `attrs`). WGSL unpacks them with `unpack2x16unorm` (strand_data.wgsl).
    pub fn packed(&self) -> PackedStrands {
        let unorm = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u32;
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut radius = 0.0f32;
        for p in &self.points {
            lo = lo.min(Vec3::new(p[0], p[1], p[2]));
            hi = hi.max(Vec3::new(p[0], p[1], p[2]));
            radius = radius.max(p[3]);
        }
        if self.points.is_empty() {
            (lo, hi) = (Vec3::ZERO, Vec3::ZERO);
        }
        let size = (hi - lo).max(Vec3::splat(1e-6));
        let points = self
            .points
            .iter()
            .map(|p| {
                let t = (Vec3::new(p[0], p[1], p[2]) - lo) / size;
                let r = if radius > 0.0 { p[3] / radius } else { 0.0 };
                [unorm(t.x) | unorm(t.y) << 16, unorm(t.z) | unorm(r) << 16]
            })
            .collect();
        let attrs = self
            .attrs
            .iter()
            .map(|a| unorm(a[0]) | unorm(a[1]) << 16)
            .collect();
        PackedStrands {
            points,
            attrs,
            box_min: [lo.x, lo.y, lo.z, radius],
            box_size: [size.x, size.y, size.z, 0.0],
        }
    }

    fn push(&mut self, pts: &[Vec3], root_radius: f32, tip_radius: f32, random: f32) {
        debug_assert_eq!(pts.len(), self.points_per as usize);
        let n = pts.len().max(2) - 1;
        for (i, p) in pts.iter().enumerate() {
            let t = i as f32 / n as f32;
            let r = root_radius + (tip_radius - root_radius) * t;
            self.points.push([p.x, p.y, p.z, r]);
            self.attrs.push([1.0, random, t, 0.0]);
        }
    }
}

/// A point on the skin: position, normal, rest position and masks.
#[derive(Debug, Clone, Copy)]
struct SkinPoint {
    p: Vec3,
    n: Vec3,
    rest: Vec3,
    mask: f32,
}

/// Area- and mask-weighted random points on the skin.
struct Sampler<'a> {
    skin: &'a SkinMesh,
    cdf: Vec<f32>,
    tris: Vec<u32>,
    channel: usize,
}

impl<'a> Sampler<'a> {
    fn new(skin: &'a SkinMesh, channel: Mask, weight: impl Fn(Vec3, f32) -> f32) -> Self {
        let mut cdf = Vec::new();
        let mut tris = Vec::new();
        let mut total = 0.0;
        let c = channel as usize;
        for (t, tri) in skin.indices.as_chunks::<3>().0.iter().enumerate() {
            let (a, b, cc) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let m = (skin.masks[a * MASKS + c]
                + skin.masks[b * MASKS + c]
                + skin.masks[cc * MASKS + c])
                / 3.0;
            if m <= 0.02 {
                continue;
            }
            let rest = (skin.rest[a] + skin.rest[b] + skin.rest[cc]) / 3.0;
            let area = (skin.positions[b] - skin.positions[a])
                .cross(skin.positions[cc] - skin.positions[a])
                .length()
                * 0.5;
            let w = area * weight(rest, m);
            if w <= 0.0 {
                continue;
            }
            total += w;
            cdf.push(total);
            tris.push(t as u32);
        }
        Self {
            skin,
            cdf,
            tris,
            channel: c,
        }
    }

    /// Covered area weight (m² times the weight function).
    fn total(&self) -> f32 {
        self.cdf.last().copied().unwrap_or(0.0)
    }

    fn sample(&self, r: &mut Rng) -> Option<SkinPoint> {
        let total = self.total();
        if total <= 0.0 {
            return None;
        }
        let x = r.f32() * total;
        let i = self.cdf.partition_point(|c| *c < x).min(self.cdf.len() - 1);
        let t = self.tris[i] as usize;
        let tri = &self.skin.indices[t * 3..t * 3 + 3];
        let (mut u, mut v) = (r.f32(), r.f32());
        if u + v > 1.0 {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        let w = 1.0 - u - v;
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let s = self.skin;
        let mix3 = |x: &[Vec3]| x[a] * w + x[b] * u + x[c] * v;
        let m = |i: usize| s.masks[i * MASKS + self.channel];
        Some(SkinPoint {
            p: mix3(&s.positions),
            n: mix3(&s.normals).normalize_or(Vec3::Z),
            rest: mix3(&s.rest),
            mask: m(a) * w + m(b) * u + m(c) * v,
        })
    }
}

/// Nearest skin vertex lookups (a 1 cm hash grid), to keep hairs out of the skin.
pub struct SkinCollider<'a> {
    skin: &'a SkinMesh,
    cells: HashMap<[i32; 3], Vec<u32>>,
    cell: f32,
}

impl<'a> SkinCollider<'a> {
    pub fn new(skin: &'a SkinMesh) -> Self {
        let cell = 0.01;
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (i, p) in skin.positions.iter().enumerate() {
            cells.entry(Self::key(*p, cell)).or_default().push(i as u32);
        }
        Self { skin, cells, cell }
    }

    fn key(p: Vec3, cell: f32) -> [i32; 3] {
        [
            (p.x / cell).floor() as i32,
            (p.y / cell).floor() as i32,
            (p.z / cell).floor() as i32,
        ]
    }

    /// Nearest skin vertex within 2 cells: (index, distance).
    pub fn nearest(&self, p: Vec3) -> Option<(usize, f32)> {
        let k = Self::key(p, self.cell);
        let mut best: Option<(usize, f32)> = None;
        for dx in -2..=2 {
            for dy in -2..=2 {
                for dz in -2..=2 {
                    let Some(list) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &i in list {
                        let d = (self.skin.positions[i as usize] - p).length_squared();
                        if best.is_none_or(|(_, b)| d < b) {
                            best = Some((i as usize, d));
                        }
                    }
                }
            }
        }
        best.map(|(i, d)| (i, d.sqrt()))
    }

    /// `p` moved out to at least `offset` m above the skin under it.
    pub fn push_out(&self, p: Vec3, offset: f32) -> Vec3 {
        let Some((i, _)) = self.nearest(p) else {
            return p;
        };
        let n = self.skin.normals[i];
        let h = (p - self.skin.positions[i]).dot(n);
        if h < offset { p + n * (offset - h) } else { p }
    }
}

/// Every hair of an appearance on its built skin.
///
/// `lm` are the landmarks in model space, `rest` the base mesh's (rest-space region math).
pub fn grow_all(
    a: &Appearance,
    skin: &SkinMesh,
    margins: &[[Vec<Vec3>; 2]; 2],
    lm: &Landmarks,
    rest: &Landmarks,
) -> Vec<StrandSet> {
    let collider = SkinCollider::new(skin);
    let mut out = Vec::new();
    if let Some(s) = grow_scalp(a, skin, &collider, lm, rest) {
        out.push(s);
    }
    out.push(grow_brows(a, skin, rest));
    out.push(grow_lashes(a, margins, lm));
    if let Some(s) = grow_beard(a, skin, &collider) {
        out.push(s);
    }
    out.retain(|s| !s.points.is_empty());
    for s in &mut out {
        occlude(s, lm.head);
    }
    out
}

/// A unit vector in the tangent plane of `n` closest to `d` (or any tangent if `d` ∥ `n`).
fn tangent(d: Vec3, n: Vec3) -> Vec3 {
    let t = d - n * d.dot(n);
    t.try_normalize()
        .unwrap_or_else(|| n.any_orthonormal_vector())
}

// Brows as transplant surgeons and groomers map them: the head's hairs stand nearly upright and
// short, the body's lie almost flat along the brow in a herringbone (lower rows rising, upper
// rows falling, meeting in a ridge), and the tail's turn down toward the temple, the edges
// feathered. Every hair curves over into the brow's flow as it grows rather than standing
// straight: straight hairs at their own random angles read as scratches.
fn grow_brows(a: &Appearance, skin: &SkinMesh, rest: &Landmarks) -> StrandSet {
    const N: u32 = 5;
    let mut set = StrandSet::new(StrandKind::Brows, N);
    let sampler = Sampler::new(skin, Mask::Brows, |_, m| m * m);
    let thick = a.brows.thickness.clamp(0.0, 1.0);
    // Real brows hold ~250 hairs per cm²; a fraction reads as full once drawn.
    let count = ((sampler.total() * 1e4) * (130.0 + 180.0 * thick)) as usize;
    let mut r = Rng::new(a.seed ^ 0xB120);
    let center = |u: f32| lerp_table(&BROW_CENTER, u);
    for _ in 0..count {
        let Some(sp) = sampler.sample(&mut r) else {
            break;
        };
        // None under the brow ridge, facing down into the eye socket: laid along the skin
        // there they head back into the socket, a dark line under the brow.
        if r.f32() < smoothstep(-0.15, -0.4, sp.n.y) {
            continue;
        }
        let side = sp.rest.x.signum();
        let eye = if side > 0.0 {
            rest.eyes[0]
        } else {
            rest.eyes[1]
        };
        let u = ((sp.rest.x * side - (eye.x * side - BROW_INNER)) / BROW_LENGTH).clamp(0.0, 1.0);
        let v = ((sp.rest.y - eye.y - center(u)) / lerp_table(&BROW_HALF, u)).clamp(-1.3, 1.3);
        // The arch's own slope, rising to its peak and falling after.
        let slope = ((center(u + 0.02) - center(u - 0.02)) / (0.04 * BROW_LENGTH)).atan();
        let head = 1.0 - smoothstep(0.08, 0.3, u);
        let tail = smoothstep(0.65, 1.0, u);
        // Angles above the lateral direction.
        let herringbone = (0.12 - 0.4 * (v - 0.2)) * (1.0 - 0.6 * u);
        let body = slope + herringbone - 0.15 * tail;
        let start = head * (1.2 - 0.1 * v) + (1.0 - head) * body + r.normal() * 0.08;
        let flow = slope - 0.05 - 0.15 * tail;
        let bend = 0.35 - 0.1 * head;
        let len = lerp_table(
            &[(0.0, 0.0045), (0.3, 0.0075), (0.6, 0.007), (1.0, 0.0045)],
            u,
        ) * r.range(0.75, 1.1)
            * (0.85 + 0.3 * thick)
            * (1.0 - 0.35 * head * v.max(0.0));
        let lift = r.range(0.05, 0.14);
        let lateral = Vec3::X * side;
        let step = len / (N - 1) as f32;
        let mut pts = [Vec3::ZERO; N as usize];
        let mut along = sp.p;
        for (i, p) in pts.iter_mut().enumerate() {
            let t = i as f32 / (N - 1) as f32;
            // Out of the skin at the root, then lying down along it.
            let h = len * lift * (t * 2.0).min(1.0) * (1.0 - 0.6 * t) + 0.0002;
            *p = along + sp.n * h;
            let angle = start + (flow - start) * bend * (t + 0.5 / (N - 1) as f32);
            along += tangent(lateral * angle.cos() + Vec3::Y * angle.sin(), sp.n) * step;
        }
        set.push(&pts, 0.00008 * HAIR_WIDTH, 0.00003 * HAIR_WIDTH, r.f32());
    }
    set
}

// Lashes as anatomy and groomers have them (Aumond and Bitton, "The eyelash follicle features
// and anomalies: A review", J. Optometry 2018): 90-160 on the upper lid in 5-6 rows and 75-80
// on the lower in 3-4, up to about 12 mm long, every one curved, and each at its own point of
// a growth cycle of months, so lengths vary. They gather into clumps whose tips meet, as
// MetaHuman's groomed lashes and XGen's clump modifier have them; a single row of evenly
// spaced, equal hairs reads as a comb.
fn grow_lashes(a: &Appearance, margins: &[[Vec<Vec3>; 2]; 2], lm: &Landmarks) -> StrandSet {
    const N: u32 = 6;
    let mut set = StrandSet::new(StrandKind::Lashes, N);
    let mut r = Rng::new(a.seed ^ 0x1A54);
    for (side, lids) in margins.iter().enumerate() {
        let eye = lm.eyes[side];
        let sx = if side == 0 { 1.0 } else { -1.0 };
        for (li, lid) in lids.iter().enumerate() {
            if lid.len() < 2 {
                continue;
            }
            let upper = li == 0;
            let lengths: Vec<f32> = lid.windows(2).map(|w| (w[1] - w[0]).length()).collect();
            let total: f32 = lengths.iter().sum();
            // A point along the margin `s` meters from its first point.
            let at = |mut s: f32| {
                for (k, l) in lengths.iter().enumerate() {
                    if s <= *l {
                        return lid[k] + (lid[k + 1] - lid[k]) * (s / l.max(1e-6));
                    }
                    s -= l;
                }
                lid[lid.len() - 1]
            };
            let across = |p: Vec3| {
                ((p.x - lid[0].x) * sx / ((lid[lid.len() - 1].x - lid[0].x) * sx).max(1e-4))
                    .clamp(0.0, 1.0)
            };
            // The lash from `root`, `len` long: out from the eye and forward, fanning outward
            // toward the outer corner, curling away from the eye (up on the upper lid, down on
            // the lower), most near the tip, so seen from the front they spread like rays.
            let lash = |root: Vec3, u: f32, len: f32| {
                let out = (root - eye).normalize_or(Vec3::Z);
                let up = if upper { 1.0 } else { -1.0 };
                let fan = Vec3::new(sx * (u - 0.35) * 0.6, 0.0, 0.0);
                let d = (out * 0.5 + Vec3::Z * 0.9 + fan).normalize();
                let away = (Vec3::new(out.x, 0.0, 0.0) + fan + Vec3::Y * up * 0.8).normalize();
                let curl = if upper { 0.55 } else { 0.25 };
                let mut pts = [Vec3::ZERO; N as usize];
                for (k, p) in pts.iter_mut().enumerate() {
                    let t = k as f32 / (N - 1) as f32;
                    *p = root + d * (len * t) + away * (len * curl * t * t);
                }
                pts
            };
            let peak = if upper { 0.0092 } else { 0.0048 };
            // Longest a little outside the middle, short at the corners.
            let shape = |u: f32| (1.0 - ((u - 0.6) / 0.62).powi(2)).max(0.12);
            let count = if upper { 130 } else { 60 };
            // Clumps of about five; fewer, looser ones on the lower lid.
            let clumps = if upper { 26 } else { 15 };
            let pull = if upper { 0.45 } else { 0.3 };
            let guides: Vec<[Vec3; N as usize]> = (0..clumps)
                .map(|k| {
                    let s = (k as f32 + 0.5 + r.range(-0.25, 0.25)) / clumps as f32 * total;
                    let root = at(s);
                    let u = across(root);
                    lash(root, u, peak * shape(u) * r.range(0.85, 1.05))
                })
                .collect();
            for _ in 0..count {
                let s = r.f32() * total;
                let mut root = at(s);
                let u = across(root);
                // Some lashes are still growing.
                let grown = if r.f32() < 0.15 {
                    r.range(0.35, 0.7)
                } else {
                    r.range(0.75, 1.1)
                };
                // Rows across the lid margin, from its front edge back (about a millimetre).
                let out = (root - eye).normalize_or(Vec3::Z);
                root += out * 0.0002 - Vec3::Z * (r.f32() * if upper { 0.0008 } else { 0.0005 });
                let mut pts = lash(root, u, peak * shape(u) * grown);
                // Toward its clump's tip, more toward the tip and less for short ones.
                let k = ((s / total * clumps as f32) as usize).min(clumps - 1);
                let guide = &guides[k];
                let w = pull * grown.min(1.0) * r.range(0.6, 1.0);
                for (i, p) in pts.iter_mut().enumerate().skip(1) {
                    let t = i as f32 / (N - 1) as f32;
                    let target = root + (guide[i] - guide[0]) * grown;
                    *p = p.lerp(target, w * t * t);
                }
                set.push(&pts, 0.00008, 0.000025, r.f32());
            }
        }
    }
    set
}

fn grow_beard(a: &Appearance, skin: &SkinMesh, collider: &SkinCollider) -> Option<StrandSet> {
    let (len, per_cm2, n) = match a.beard_style() {
        BeardStyle::None => return None,
        // Stubble is many fine hairs (a few hundred per cm² on a man's cheek): at a few
        // dozen thick ones it reads as dirt.
        BeardStyle::Stubble => (0.0014, 140.0, 3u32),
        // Beard hairs are coarser than scalp hair, about 100-150 um (and as many per cm² as
        // the scalp's thinner areas): drawn twice as thick at half the count they read as
        // steel wool.
        BeardStyle::Short => (0.009, 95.0, 6),
        BeardStyle::Full => (0.03, 95.0, 9),
        BeardStyle::Goatee => (0.014, 95.0, 7),
    };
    let goatee = a.beard_style() == BeardStyle::Goatee;
    let stubble = a.beard_style() == BeardStyle::Stubble;
    let sampler = Sampler::new(skin, Mask::Beard, |rest, m| {
        if goatee && rest.x.abs() > 0.024 {
            0.0
        } else {
            m
        }
    });
    let density = a.beard.density.clamp(0.0, 1.0);
    let count = (sampler.total() * 1e4 * per_cm2 * (0.3 + 0.7 * density)) as usize;
    let mut set = StrandSet::new(StrandKind::Beard, n);
    let mut r = Rng::new(a.seed ^ 0xBEA2D);
    let mut pts = vec![Vec3::ZERO; n as usize];
    for _ in 0..count {
        let Some(sp) = sampler.sample(&mut r) else {
            break;
        };
        // Patchy where the mask fades (cheek line, neck); stubble fades out sooner, without
        // stray hairs beyond.
        let edge = if stubble {
            (sp.mask - 0.3) / 0.7
        } else {
            sp.mask.powf(0.7)
        };
        if r.f32() > edge {
            continue;
        }
        // Moustache hairs fan down and out from the middle; the rest grow down the face and
        // under the jaw lie toward the throat.
        let above_lips = sp.rest.y > 0.668 && sp.rest.x.abs() < 0.03;
        // (Moustache hairs are trimmed short of the lips: grown as long as the beard and
        // fanned out, one crossed the cheek.)
        let down = if above_lips {
            Vec3::new(sp.rest.x.signum() * 0.35, -1.0, 0.1)
        } else {
            Vec3::new(sp.rest.x.signum() * 0.12, -1.0, 0.25)
        };
        let d = tangent(down, sp.n);
        let l = f32::min(len, if above_lips { 0.014 } else { f32::MAX }) * r.range(0.6, 1.2);
        let lift = r.range(0.25, 0.55);
        let mut dir = (d + sp.n * lift).normalize();
        let seg = l / (n - 1) as f32;
        pts[0] = sp.p;
        for k in 1..n as usize {
            let t = k as f32 / (n - 1) as f32;
            // Curl a little and droop as it grows.
            dir = (dir
                + Vec3::new(r.normal(), r.normal(), r.normal()) * 0.18
                + Vec3::NEG_Y * (0.25 * t))
                .normalize();
            let p = pts[k - 1] + dir * seg;
            pts[k] = collider.push_out(p, 0.0006 + l * 0.15 * t);
        }
        let root = if stubble { 0.00005 } else { 0.000065 };
        set.push(&pts, root * HAIR_WIDTH, 0.00004 * HAIR_WIDTH, r.f32());
    }
    Some(set)
}

/// Style parameters of scalp hair.
struct Groom {
    /// Hair length, m.
    length: f32,
    points: u32,
    /// Hairs per cm² of scalp.
    density: f32,
    /// How strongly gravity bends the hair (0 stiff .. 1 limp).
    gravity: f32,
    /// How far the hair stands off the scalp at its tip, m.
    loft: f32,
    /// How closely hairs follow their clump toward the tip.
    clump: f32,
    root_radius: f32,
    /// Where a bun sits, from the head's center (m; zero without one).
    bun: Vec3,
}

/// The hairline, irregular as a real one is: hair-restoration surgeons rebuild a slow wander of
/// a few millimetres and a finer zigzag, slightly different on each side, because a smooth,
/// even line reads as a wig (Bernstein and Rassman, "Follicular Unit Hair Transplantation").
struct RaggedHairline {
    recession: f32,
    phases: [f32; 4],
}

impl RaggedHairline {
    fn new(seed: u64, recession: f32) -> Self {
        let mut r = Rng::new(seed ^ 0x4A1E);
        Self {
            recession,
            phases: std::array::from_fn(|_| r.f32() * std::f32::consts::TAU),
        }
    }

    /// Height of the hairline over the head's center (m) at `theta` around it (0 in front).
    fn at(&self, theta: f32) -> f32 {
        let p = self.phases;
        crate::mesh::hairline(theta, self.recession)
            + 0.002 * (theta * 4.0 + p[0]).sin()
            + 0.0012 * (theta * 9.0 + p[1]).sin()
            + 0.0007 * (theta * 37.0 + p[2]).sin()
            + 0.0004 * (theta * 71.0 + p[3]).sin()
    }
}

fn grow_scalp(
    a: &Appearance,
    skin: &SkinMesh,
    collider: &SkinCollider,
    lm: &Landmarks,
    rest: &Landmarks,
) -> Option<StrandSet> {
    let k = a.hair.length.clamp(0.5, 1.5);
    let g = match a.hair_style() {
        HairStyle::Bald => return None,
        HairStyle::Buzz => Groom {
            length: 0.006 * k,
            points: 5,
            density: 150.0,
            gravity: 0.0,
            loft: 0.0015,
            clump: 0.0,
            root_radius: 0.00007,
            bun: Vec3::ZERO,
        },
        HairStyle::Short => Groom {
            length: 0.055 * k,
            // Curly hair needs the points for its turns.
            points: 10 + (12.0 * a.hair.curl.clamp(0.0, 1.0)) as u32,
            density: 130.0,
            gravity: 0.25,
            loft: 0.012,
            clump: 0.55,
            root_radius: 0.00006,
            bun: Vec3::ZERO,
        },
        HairStyle::Medium => Groom {
            length: 0.2 * k,
            points: 18,
            density: 170.0,
            gravity: 1.0,
            loft: 0.016,
            clump: 0.6,
            root_radius: 0.00006,
            bun: Vec3::ZERO,
        },
        HairStyle::Long => Groom {
            length: 0.42 * k,
            points: 32,
            density: 190.0,
            gravity: 1.0,
            loft: 0.012,
            clump: 0.75,
            root_radius: 0.00006,
            bun: Vec3::ZERO,
        },
        HairStyle::Bun => Groom {
            length: 0.3,
            points: 20,
            density: 130.0,
            gravity: 0.0,
            loft: 0.004,
            clump: 0.6,
            root_radius: 0.00006,
            bun: Vec3::ZERO,
        },
    };
    let head = rest.head;
    let mut g = g;
    if a.hair_style() == HairStyle::Bun {
        g.bun = bun_seat(skin, head);
    }
    let recession = a.hair.recession.clamp(0.0, 1.0);
    let ragged = RaggedHairline::new(a.seed, recession);
    let sampler = Sampler::new(skin, Mask::Scalp, |rest, m| {
        // A receded hairline: thinner front and temples (and the crown at the far end).
        let d = rest - head;
        let theta = d.x.atan2(d.z);
        let line = ragged.at(theta);
        // A soft hairline: single hairs thin out over most of a centimetre (and grow finer,
        // below) before the full density.
        let edge = ((d.y - line) / 0.008).clamp(0.0, 1.0).powf(0.7);
        let crown = 1.0
            - recession
                * recession
                * 0.7
                * smooth_bump(Vec3::new(d.x, d.y - 0.1, d.z + 0.03).length(), 0.05);
        m * m * edge * crown
    });
    let total = (sampler.total() * 1e4 * g.density) as usize;
    if total == 0 {
        return None;
    }
    let mut r = Rng::new(a.seed ^ 0x5CA1);
    let mut set = StrandSet::new(StrandKind::Scalp, g.points);
    // Guides first: one per clump, grown in full; then the hairs that follow them.
    // Long hair gathers into fewer, bigger locks.
    let per_guide = if g.gravity > 0.5 { 125 } else { 65 };
    let guides_n = if g.clump > 0.0 {
        (total / per_guide).clamp(60, 1500)
    } else {
        0
    };
    let mut guides: Vec<(SkinPoint, Vec<Vec3>)> = Vec::with_capacity(guides_n);
    let mut gr = Rng::new(a.seed ^ 0x6D1E);
    let curl = a.hair.curl.clamp(0.0, 1.0);
    for _ in 0..guides_n {
        let Some(sp) = sampler.sample(&mut gr) else {
            break;
        };
        let mut pts = grow_hair(a, &g, sp, collider, lm, rest, &mut gr, 1.0);
        if g.gravity > 0.5 {
            settle(&mut pts, sp, collider, lm, rest);
            // Hanging hair waves as whole locks, shorter and deeper the curlier it is.
            // Neighboring locks wave nearly in step (as real waves set across the hair), with
            // enough difference that they catch the light apart; waves of their own would
            // read as separate ribbons.
            let around = (sp.rest - rest.head).x.atan2((sp.rest - rest.head).z);
            let phase = around * 1.5 + (sp.rest.y - rest.head.y) * 20.0 + gr.range(-0.9, 0.9);
            let wavelength = (0.2 - 0.13 * curl) * gr.range(0.9, 1.1);
            let waves = g.length / wavelength;
            let amp = gr.range(0.003, 0.008) + 0.02 * curl;
            // Mostly toward and away from the head (which turns the lock's highlight), a
            // little sideways.
            let out = (sp.p - lm.head).normalize_or(Vec3::Z);
            let side = out.cross(Vec3::Y).normalize_or(Vec3::X);
            let tilt = gr.range(-0.3, 0.3);
            let dir = (out + side * tilt).normalize();
            let last = pts.len() - 1;
            for (k, p) in pts.iter_mut().enumerate() {
                let t = k as f32 / last as f32;
                let wave = (phase + t * waves * std::f32::consts::TAU).sin() - phase.sin();
                *p += dir * (amp * t.min(0.5) * 2.0 * wave);
            }
        }
        guides.push((sp, pts));
    }
    // A lock from the front of the part that settled low across the forehead (nothing holds it
    // back there) hangs over the face as a stray rope below the swept hair: leave it out, and
    // its hairs join the locks beside it.
    if g.gravity > 0.5 {
        guides.retain(|(_, pts)| !pts.iter().skip(2).any(|p| over_forehead(*p, lm)));
    }
    let guide_grid = GuideGrid::new(&guides);
    // A hair takes its shape (and its motion) only from guides on its own side of the part:
    // a hair following a guide from across it is combed the other way, and the part zigzags
    // along the edges of the guides' patches instead of running straight.
    let parted = matches!(a.hair_style(), HairStyle::Medium | HairStyle::Long);
    let part_side = |rest: Vec3| {
        let d = rest - head;
        (parted && d.z > PART_END_Z).then_some(d.x > PART_X)
    };
    // Long and medium hair moves: every hair with the guide nearest its root.
    let moves = g.gravity > 0.5 && !guides.is_empty();
    for _ in 0..total {
        let Some(sp) = sampler.sample(&mut r) else {
            break;
        };
        let side = part_side(sp.rest);
        let same_side = |g: &SkinPoint| side.is_none_or(|s| part_side(g.rest) != Some(!s));
        // Hairs at the hairline are short and fine (vellus), as on a real forehead.
        let d = sp.rest - head;
        let line = ragged.at(d.x.atan2(d.z));
        let fine = ((d.y - line) / 0.01).clamp(0.0, 1.0);
        // Hairs grow for different spans, so lengths vary (by a third on short hair, where it
        // breaks up the even pile of a fur hat; less on long hair, whose tips thin out).
        let spread = if g.gravity > 0.5 { 0.78 } else { 0.7 };
        let mut scale = r.range(spread, 1.08) * (0.3 + 0.7 * fine);
        // Hanging hair from the sideburn in front of the ear would fall across the cheek: there
        // it stays short, as a real sideburn's fine hairs do under the long hair over them.
        if g.gravity > 0.5 {
            let t = d.x.atan2(d.z).abs();
            let ramp = |a: f32, b: f32, x: f32| ((x - a) / (b - a)).clamp(0.0, 1.0);
            let sideburn =
                ramp(0.85, 0.95, t) * (1.0 - ramp(1.2, 1.3, t)) * (1.0 - ramp(0.02, 0.035, d.y));
            scale *= 1.0 - 0.9 * sideburn;
        }
        let thin = 0.55 + 0.45 * fine;
        // A few hairs stray from the groom: flyaways.
        // (Not from the front hairline, where they would cross the forehead.)
        // (Nor from the front of hanging hair, where one falls across the face.)
        let at_front = fine < 1.0 || (d.z > 0.03 && d.y < 0.07) || (g.gravity > 0.5 && d.z > 0.0);
        let flyaway = g.points > 5 && !at_front && r.f32() < 0.015;
        // Hanging hair also holds short hairs, broken or growing back (hairs shed and regrow
        // all the time, a few percent of them at any length; 4% here), which stand a little off the
        // combed surface over the top and back: the fine halo real long hair shows against the
        // light, where a groom of only full-length hairs has the clean edge of a wig.
        let broken = g.gravity > 0.5
            && !flyaway
            && fine >= 1.0
            && d.y > 0.03
            && d.z < 0.03
            && r.f32() < 0.04;
        if broken {
            scale *= r.range(0.1, 0.4);
        }
        let mut pts = grow_hair(a, &g, sp, collider, lm, rest, &mut r, scale);
        // No two hairs grow quite the same way (groomers' "crossover"): turned a little about
        // the scalp's normal, most at the hairline, where combed-back hair otherwise lines up
        // like the teeth of a comb.
        if g.points > 5 {
            let turn = Quat::from_axis_angle(sp.n, r.normal() * (0.06 + 0.16 * (1.0 - fine)));
            let root = pts[0];
            for p in pts.iter_mut().skip(1) {
                *p = root + turn * (*p - root);
            }
        }
        // The strand of hairs this one grows with (see the locks below).
        // (Curls coil in wider strands.)
        let strand = strand_center(
            sp.rest,
            if g.gravity > 0.5 {
                0.004
            } else {
                0.003 + 0.004 * curl
            },
        );
        let mut lock_tone = None;
        // How far out in its lock the hair is (0 on the guide .. 1 at the lock's edge).
        let mut lock_edge = 0.0;
        // Follow the nearest guide if it is close (a lone hair far from any guide stays free).
        let reach = if g.gravity > 0.5 { 0.025 } else { 0.012 };
        let near = |gi: &usize| (guides[*gi].0.p - sp.p).length() < reach;
        let lock = guide_grid
            .nearest(sp.p, &guides, |i| same_side(&guides[i].0))
            .filter(near);
        // Beside the part with only locks from across it near: left free, it would hang
        // unsettled across the forehead; one hair fewer doesn't show.
        if !flyaway
            && lock.is_none()
            && guide_grid
                .nearest(sp.p, &guides, |_| true)
                .is_some_and(|gi| near(&gi))
        {
            continue;
        }
        // Nor does a hanging hair at the front with no lock near: it would hang unsettled
        // over the face.
        if !flyaway && lock.is_none() && g.gravity > 0.5 && d.z > 0.03 {
            continue;
        }
        if !flyaway && let Some(gi) = lock {
            let (gsp, gpts) = &guides[gi];
            let off = sp.p - gsp.p;
            let c = g.clump * r.range(if g.gravity > 0.5 { 0.8 } else { 0.6 }, 1.0);
            // Within its lock, a hair gathers with the hairs rooted a few millimetres around it
            // into a strand of its own (MetaHuman's secondary clumps): one clump level makes
            // each lock a smooth sheet, and the strands give it the breakup of real hair. Some
            // hairs stay out of any strand, and strands gather more or less tightly.
            let to_strand = strand - sp.rest;
            let to_strand = to_strand - sp.n * to_strand.dot(sp.n);
            let tight = if r.f32() < 0.08 {
                0.0
            } else {
                0.75 * (0.7 + 0.6 * hash01(strand * 1000.0))
            };
            // The hairs of a lock share most of its tone, and a strand some of its own.
            lock_tone = Some(0.6 * hash01(gsp.rest) + 0.4 * hash01(strand * 1000.0 + Vec3::X));
            lock_edge = (off.length() / 0.025).min(1.0);
            // A shorter hair follows its lock only as far as it reaches.
            let reach = scale.min(1.0);
            let last = pts.len() - 1;
            for (k, p) in pts.iter_mut().enumerate() {
                let t = k as f32 / last as f32;
                let own = off + to_strand * (tight * t.sqrt());
                let follow = along(gpts, t * reach) + own * (1.0 - c * t.powf(0.8));
                // Hanging hair takes its lock's shape all along (its own wander would cross
                // its neighbors' and look like straw); short hair only toward the tip.
                let w = if g.gravity > 0.5 {
                    (0.9 + 0.1 * t).min(1.0)
                } else {
                    (c * 1.2 * t.sqrt()).min(1.0)
                };
                *p = *p + (follow - *p) * w;
            }
        }
        if broken {
            // Standing off the hair over it, more toward the tip.
            let out = (sp.p - lm.head).normalize_or(Vec3::Y);
            let lift = r.range(0.003, 0.01);
            let last = pts.len() - 1;
            for (k, p) in pts.iter_mut().enumerate() {
                let t = k as f32 / last as f32;
                *p += out * (lift * t * t);
            }
        }
        if flyaway {
            // Lifted off the hair and curving away, more toward the tip.
            let out = (sp.p - lm.head).normalize_or(Vec3::Y);
            let aside = Vec3::new(r.normal(), r.normal(), r.normal()) * 0.6;
            let lift = r.range(0.003, 0.009) + g.length * 0.05;
            let last = pts.len() - 1;
            for (k, p) in pts.iter_mut().enumerate() {
                let t = k as f32 / last as f32;
                *p += (out + aside) * (lift * t * t);
            }
        }
        // Curls: a helix around the hair, growing from the root (tight coils on short hair).
        // Hanging hair waves with its lock (above), so its own coil is small. Anything longer
        // than a buzz turns once per wavelength of its curl, as hair is graded by the size of
        // its curve (Loussouarn et al. 2007: waves centimetres long, coils under one), and with
        // at least eight points a turn: a coarser helix zigzags, and the zigzags of
        // neighboring hairs cross like straw (slightly wavy short hair had two turns on ten
        // points).
        if curl > 0.02 && g.points > 4 {
            let hanging = g.gravity > 0.5;
            let mut radius = (0.002 + 0.01 * curl) * (g.length / 0.05).min(1.0);
            let mut turns = 2.0 + 10.0 * curl * (g.length / 0.2).min(2.0);
            if g.points > 5 {
                let wavelength = 0.12 - 0.108 * curl;
                turns = (len_of(&pts) / wavelength).min((g.points - 1) as f32 / 8.0);
            }
            if hanging {
                radius *= 0.3;
            }
            // The hairs of a strand curl in step, and so coil together into ringlets; each
            // hair curling its own way makes a frizz like fur (Houdini and MetaHuman grooms
            // share the curl's phase within a clump). Strands differ a little in size.
            let phase =
                hash01(strand * 1000.0 + Vec3::Z) * std::f32::consts::TAU + r.normal() * 0.25;
            radius *= 0.8 + 0.4 * hash01(strand * 1000.0 + Vec3::Y);
            let last = pts.len() - 1;
            let base = pts.clone();
            for k in 1..pts.len() {
                let t = k as f32 / last as f32;
                let dir = (base[k] - base[k - 1]).normalize_or(Vec3::NEG_Y);
                // (A frame that turns smoothly along the hair, and alike on neighboring hairs, so
                // a strand's hairs coil in step: an arbitrary one flipped from point to point and
                // zigzagged short curls into wool.)
                let side = if hanging || g.points > 5 {
                    dir.cross(base[k] - lm.head)
                        .normalize_or(dir.any_orthonormal_vector())
                } else {
                    dir.any_orthonormal_vector()
                };
                let up = dir.cross(side);
                let a = phase + t * turns * std::f32::consts::TAU;
                pts[k] += (side * a.cos() + up * a.sin()) * (radius * t.min(0.3) / 0.3);
            }
        }
        // Keep out of the skin (and off the face) after clumping and curls.
        let last = pts.len() - 1;
        let mut face_side = None;
        let jitter = hash01(sp.rest);
        let loft = loft_at(a, &g, d);
        // A hanging hair crossing the middle of the forehead low, below the swept hair, shows
        // alone against the skin as a stray line: leave it out (the hair above it covers). (The
        // narrow side's hair, swept away from the middle, stays.)
        let stray = |p: &Vec3| over_forehead(*p, lm) && (*p - lm.head).x.abs() < PART_X;
        if g.gravity > 0.5 && !flyaway && pts.iter().skip(3).any(stray) {
            continue;
        }
        for (k, p) in pts.iter_mut().enumerate().skip(1) {
            let t = k as f32 / last as f32;
            if g.gravity > 0.5 {
                *p = keep_off_face(
                    keep_off_forehead(*p, jitter, lm),
                    &mut face_side,
                    jitter,
                    lm,
                );
            }
            *p = collider.push_out(*p, 0.0008 + loft * t.min(0.4) / 0.4 * 0.6 * tuck(a, t));
        }
        // A point pushed out of the thin ear one way while its neighbours went the other way
        // spikes the hair out by centimetres: take it back to between them (inside the ear's
        // edge, it doesn't show).
        for k in 1..last {
            let mid = (pts[k - 1] + pts[k + 1]) * 0.5;
            if (pts[k] - mid).length() > 0.012 {
                pts[k] = mid;
            }
        }
        // Hairs taper to a fine tip.
        set.push(
            &pts,
            g.root_radius * HAIR_WIDTH * thin,
            g.root_radius * HAIR_WIDTH * thin * 0.25,
            // Hanging locks show their own tone; short or slicked hair mixes across its clumps,
            // and tones shared by whole clumps read as stripes.
            lock_tone.map_or(r.f32(), |l| {
                let w = if g.gravity > 0.5 { 0.6 } else { 0.25 };
                w * l + (1.0 - w) * r.f32()
            }),
        );
        // Fine hairline hairs are short, so they don't sit in their own root shadow.
        let n = set.attrs.len();
        for attr in &mut set.attrs[n - g.points as usize..] {
            attr[3] = 1.0 - fine;
            // Hanging locks part toward their tips, and the hair seen between them is
            // deeper in shadow: darken each lock's edge.
            if g.gravity > 0.5 {
                attr[0] = 1.0 - 0.25 * lock_edge * lock_edge * attr[2].sqrt();
            }
        }
        if moves {
            set.follow
                .push(guide_grid.follow(sp.p, &guides, |i| same_side(&guides[i].0)));
        }
    }
    if moves {
        set.guides = guides.into_iter().flat_map(|(_, pts)| pts).collect();
    }
    Some(set)
}

/// Hanging hair settled under gravity: the grown guide is a first guess (lifted off the head),
/// and a short simulation lets it fall and drape over the scalp, the ears and the shoulders the
/// way hair does, instead of standing out where the walk left it. The guide is a chain of
/// points with fixed lengths that resists bending (stiffer at the root, where hair leaves the
/// scalp at its own angle), collides with the skin, and keeps off the face. Hair has no room
/// to lie in the scalp, so each guide rests on a layer of its own: hair from the crown over the
/// hair from lower down, as it is combed.
fn settle(
    pts: &mut [Vec3],
    sp: SkinPoint,
    collider: &SkinCollider,
    lm: &Landmarks,
    rest: &Landmarks,
) {
    const STEPS: usize = 120;
    const ITERATIONS: usize = 4;
    // m per step²: how far gravity pulls each step.
    const GRAVITY: f32 = 0.0006;
    let n = pts.len();
    if n < 4 {
        return;
    }
    let seg: Vec<f32> = pts.windows(2).map(|w| (w[1] - w[0]).length()).collect();
    // How high the root sits on the head: higher roots lie on top.
    let d = sp.rest - rest.head;
    let layer = ((d.y + 0.04) / 0.14).clamp(0.0, 1.0);
    let lie = |k: usize| {
        let t = k as f32 / (n - 1) as f32;
        0.0008 + (0.0015 + 0.0065 * layer) * (t / 0.15).min(1.0)
    };
    let jitter = hash01(sp.rest);
    let mut face_side = Some(if d.x > PART_X { 1.0 } else { -1.0 });
    let grown = pts.to_vec();
    let mut prev = pts.to_vec();
    // Points resting on the skin hold there by friction, so combed hair stays combed where it
    // lies on the head and only what hangs free falls.
    let mut touching = vec![false; n];
    for _ in 0..STEPS {
        // Move, damped (hair in air settles quickly).
        let moving = pts[2..].iter_mut().zip(&mut prev[2..]).zip(&touching[2..]);
        for ((p, last), touch) in moving {
            let (keep, pull) = if *touch { (0.2, 0.25) } else { (0.75, 1.0) };
            let v = (*p - *last) * keep;
            *last = *p;
            *p += v + Vec3::new(0.0, -GRAVITY * pull, 0.0);
        }
        for it in 0..ITERATIONS {
            // Bending: each point toward the middle of its neighbors (stiffer near the root).
            for k in 2..n - 1 {
                let t = k as f32 / (n - 1) as f32;
                let stiff = 0.05 + 0.3 * (1.0 - t / 0.25).max(0.0);
                let mid = (pts[k - 1] + pts[k + 1]) * 0.5;
                pts[k] += (mid - pts[k]) * stiff;
            }
            // Lengths: from the root out, so the root stays put.
            for k in 1..n {
                let dir = (pts[k] - pts[k - 1]).normalize_or(Vec3::NEG_Y);
                pts[k] = pts[k - 1] + dir * seg[k - 1];
            }
            // The skin (and the face kept clear, eased so the chain doesn't jump), once the
            // chain has its shape.
            if it + 1 < ITERATIONS {
                continue;
            }
            for k in 2..n {
                let aside = keep_off_face(pts[k], &mut face_side, jitter, lm);
                let p = pts[k] + (aside - pts[k]) * 0.35;
                let out = collider.push_out(p, lie(k));
                touching[k] = (out - p).length_squared() > 0.0;
                pts[k] = out;
            }
        }
    }
    // A guide that crumpled (caught between the face and the skin) keeps its grown shape.
    let crumpled = pts.windows(3).any(|w| (w[1] - w[0]).dot(w[2] - w[1]) < 0.0);
    if crumpled {
        pts.copy_from_slice(&grown);
    }
}

/// The guide whose root is nearest `p`, searching all of them.
fn nearest_guide(p: Vec3, guides: &[(SkinPoint, Vec<Vec3>)]) -> Option<usize> {
    guides
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1.0.p - p)
                .length_squared()
                .total_cmp(&(b.1.0.p - p).length_squared())
        })
        .map(|(i, _)| i)
}

/// The part of long hair: on the character's left, 2.5 cm off center.
pub(crate) const PART_X: f32 = 0.025;
/// The part runs back from the hairline to here (rest z from the head's center, m); behind it
/// the hair falls straight down.
const PART_END_Z: f32 = -0.03;

/// Hair that would hang in front of the face is moved aside to frame it: easing out over the
/// forehead (so a hair bends aside instead of jumping), then beside the eyes and cheeks and back
/// into the side curtain rather than in front of the face. A hair goes aside on one side
/// (`side`, kept from point to point, picked where it first needs moving if not given), so it
/// never jumps across; `jitter` (0..1) spreads the hairs that were moved so they don't line up.
/// Over the forehead hair stays up at the hairline (swept to the side along it, then down past
/// the temple): a lock lying across the brow reads as a band, not a fringe. For hair already
/// grown (lifting while growing would send it out sideways).
/// Whether `p` hangs in front of the forehead, below where swept hair lies on it (the floor of
/// `keep_off_forehead`).
fn over_forehead(p: Vec3, lm: &Landmarks) -> bool {
    let d = p - lm.head;
    d.z > 0.06 && d.x.abs() < 0.04 && d.y > -0.01 && d.y < 0.05
}

fn keep_off_forehead(p: Vec3, jitter: f32, lm: &Landmarks) -> Vec3 {
    let d = p - lm.head;
    let floor = 0.05 + 0.006 * jitter;
    if d.z > 0.06 && d.x.abs() < 0.06 && d.y > -0.01 && d.y < floor {
        let t = 1.0 - ((d.x.abs() - 0.045) / 0.015).clamp(0.0, 1.0);
        return Vec3::new(p.x, lm.head.y + d.y + (floor - d.y) * t, p.z);
    }
    p
}

fn keep_off_face(p: Vec3, side: &mut Option<f32>, jitter: f32, lm: &Landmarks) -> Vec3 {
    let d = p - lm.head;
    // In front of the ears, from the forehead down to the shoulders.
    if d.y > 0.06 || d.y < -0.26 || d.z < 0.025 {
        return p;
    }
    let t = ((0.06 - d.y) / 0.06).clamp(0.0, 1.0);
    let below = ((d.y + 0.26) / 0.06).clamp(0.0, 1.0);
    let fade = t * t * (3.0 - 2.0 * t) * below;
    let half = (0.083 + 0.014 * jitter) * fade;
    let s = *side.get_or_insert(if d.x < 0.0 { -1.0 } else { 1.0 });
    // Fading out over a centimetre beside the face: a point pulled back while its neighbours
    // just outside weren't kinked the hair into a spike (rungs of a ladder down the cheek).
    let near = ((half + 0.012 - d.x * s) / 0.012).clamp(0.0, 1.0);
    if near <= 0.0 {
        return p;
    }
    // From the eyes down, also back toward the ears.
    let back = ((0.01 - d.y) / 0.03).clamp(0.0, 1.0) * near;
    let z = p.z.min(lm.head.z + 0.035 + 0.03 * jitter);
    let x = if d.x * s < half {
        lm.head.x + s * half
    } else {
        p.x
    };
    Vec3::new(x, p.y, p.z + (z - p.z) * back)
}

/// The length of a polyline.
fn len_of(pts: &[Vec3]) -> f32 {
    pts.windows(2).map(|w| (w[1] - w[0]).length()).sum()
}

/// A stable 0..1 value from a position (so a hair's guide pass and final pass agree).
/// How much of its style's length a hair at `d` (from the head's center, rest) has: short hair
/// is cut, not grown evenly, shorter over the sides and nape (graduated up to the weight at the
/// back of the head) and longest on top and at the front, as a barber's cut. Other styles are
/// even.
fn cut_length(a: &Appearance, d: Vec3) -> f32 {
    if a.hair_style() != HairStyle::Short {
        return 1.0;
    }
    let top = smoothstep(-0.03, 0.08, d.y);
    (0.45 + 0.55 * top) * (1.0 + 0.1 * smoothstep(0.0, 0.08, d.z))
}

/// How far the hair at `d` (from the head's center, rest) stands off the scalp. Short hair is
/// set to a shape, full on top and close over the sides and nape, its surface wandering a
/// little over a few centimetres (curly hair has more volume); a constant height everywhere
/// reads as a helmet or a fur hat. Other styles stand off evenly.
fn loft_at(a: &Appearance, g: &Groom, d: Vec3) -> f32 {
    if a.hair_style() != HairStyle::Short {
        return g.loft;
    }
    let curl = a.hair.curl.clamp(0.0, 1.0);
    let top = smoothstep(-0.02, 0.09, d.y);
    let wander = 1.0
        + 0.1
            * ((d.x * 140.0 + 1.3).sin() * (d.z * 120.0 + 0.7).sin()
                + 0.5 * (d.y * 170.0 + 2.1).sin());
    g.loft * (1.0 + curl) * (0.3 + 0.7 * top) * wander
}

/// Short hair's tips tuck in under the hair beyond them, as shingles lie (the hair's surface
/// is its mid-lengths, not a pile of tips).
fn tuck(a: &Appearance, t: f32) -> f32 {
    if a.hair_style() == HairStyle::Short {
        1.0 - 0.35 * smoothstep(0.7, 1.0, t)
    } else {
        1.0
    }
}

/// Where the strand of the hair rooted at `rest` gathers: the nearest of points jittered
/// about a grid of `cell` (so strands are irregular patches about a cell across).
fn strand_center(rest: Vec3, cell: f32) -> Vec3 {
    let c = (rest / cell).floor();
    let mut best = (f32::MAX, rest);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let k = c + Vec3::new(dx as f32, dy as f32, dz as f32);
                let jitter = Vec3::new(
                    hash01(k + Vec3::new(0.13, 0.0, 0.0)),
                    hash01(k + Vec3::new(0.0, 0.29, 0.0)),
                    hash01(k + Vec3::new(0.0, 0.0, 0.47)),
                );
                let q = (k + 0.15 + 0.7 * jitter) * cell;
                let d = q.distance_squared(rest);
                if d < best.0 {
                    best = (d, q);
                }
            }
        }
    }
    best.1
}

/// The point at `u` (0 root .. 1 tip) along a hair, between its points.
fn along(pts: &[Vec3], u: f32) -> Vec3 {
    let x = u.clamp(0.0, 1.0) * (pts.len() - 1) as f32;
    let k = (x as usize).min(pts.len() - 2);
    pts[k].lerp(pts[k + 1], x - k as f32)
}

fn hash01(p: Vec3) -> f32 {
    ((p.x * 12.9898 + p.y * 78.233 + p.z * 37.719).sin() * 43758.547)
        .fract()
        .abs()
}

fn smooth_bump(d: f32, r: f32) -> f32 {
    let x = (1.0 - d / r).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Grow one hair from its root, following the style's flow, gravity and the head's shape.
#[allow(clippy::too_many_arguments)]
fn grow_hair(
    a: &Appearance,
    g: &Groom,
    sp: SkinPoint,
    collider: &SkinCollider,
    lm: &Landmarks,
    rest: &Landmarks,
    r: &mut Rng,
    length_scale: f32,
) -> Vec<Vec3> {
    let n = g.points as usize;
    let d = sp.rest - rest.head;
    let side = if d.x > PART_X { 1.0 } else { -1.0 };
    let crown = Vec3::new(0.012, 0.085, -0.055);
    let flow = match a.hair_style() {
        HairStyle::Buzz | HairStyle::Short => {
            // Away from the crown whorl, front hair swept back and toward the part's far side.
            let from_crown = Vec3::new(d.x - crown.x, 0.0, d.z - crown.z);
            let front = ((d.z - 0.0) / 0.06).clamp(0.0, 1.0);
            let sweep = Vec3::new(-0.6, 0.15, -1.0).normalize();
            // Over the sides and back the hair lies down the head.
            let down = ((0.07 - d.y) / 0.05).clamp(0.0, 1.0) * 0.8;
            from_crown.normalize_or(Vec3::NEG_Z) * (1.0 - front)
                + sweep * front
                + Vec3::NEG_Y * down
        }
        HairStyle::Medium | HairStyle::Long => {
            // Swept back from the face and away from the part at the front and over the top;
            // straight down behind the crown (the part ends there).
            let front = ((d.z - PART_END_Z) / 0.07).clamp(0.0, 1.0);
            // On the part's narrow side, hairline hair goes more to the side than back, so the
            // front of the part stays covered (the wide side would sweep across the forehead).
            let hairline = ((d.z - 0.02) / 0.04).clamp(0.0, 1.0) * f32::max(side, 0.0);
            let swept =
                Vec3::new(side * 0.9, -0.2, -0.65).lerp(Vec3::new(side, -0.3, -0.3), hairline);
            let down = Vec3::new(d.x * 3.0, -1.0, -0.35);
            // Sideburn hair, low in front of the ear, grows down, not back toward the ear.
            let sideburn = ((0.035 - d.y) / 0.03).clamp(0.0, 1.0);
            (swept * front + down * (1.0 - front)).lerp(Vec3::new(0.0, -1.0, -0.25), sideburn)
        }
        HairStyle::Bun => {
            // Pulled toward the bun at the back of the head.
            (g.bun - d).normalize_or(Vec3::NEG_Z)
        }
        HairStyle::Bald => Vec3::NEG_Y,
    };
    let len = g.length * length_scale * cut_length(a, d);
    let seg = len / (n - 1) as f32;
    let mut pts = Vec::with_capacity(n);
    pts.push(sp.p);
    // Out of the scalp at a low angle along the flow (long hair lies flatter, so a part stays
    // a thin line).
    // Next to the part, hair lifts off the scalp before falling over it, so the part stays
    // narrow instead of opening into a bare strip.
    let near_part = if matches!(a.hair_style(), HairStyle::Medium | HairStyle::Long) {
        (1.0 - ((d.x - PART_X).abs() / 0.025)).clamp(0.0, 1.0)
            * ((d.y - 0.03) / 0.03).clamp(0.0, 1.0)
            // Not at the front hairline, where lifted hair would fall onto the forehead.
            * ((0.045 - d.z) / 0.035).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let curl = a.hair.curl.clamp(0.0, 1.0);
    // Long hair lifts off the crown a little before it falls (volume at the roots).
    let mut rise = if g.gravity > 0.5 { 0.26 } else { 0.24 } + 0.4 * near_part;
    let mut along = tangent(flow, sp.n);
    if matches!(a.hair_style(), HairStyle::Buzz | HairStyle::Short) {
        // Coily hair isn't combed flat: it stands up and points every way. Short hair that is
        // only wavy or curly is still combed and set (each hair pointing its own way made a
        // frizz like fur), lifting more on top, where a set cut has its volume.
        let coily = if a.hair_style() == HairStyle::Buzz {
            curl
        } else {
            smoothstep(0.6, 1.0, curl)
        };
        let any = tangent(Vec3::new(r.normal(), r.normal(), r.normal()), sp.n);
        along = (along * (1.0 - 0.85 * coily) + any * coily).normalize_or(along);
        rise += 0.5 * coily + 0.3 * (curl - coily) * smoothstep(-0.02, 0.09, d.y);
    }
    let mut dir = (along + sp.n * rise).normalize();
    let bun_center = lm.head + g.bun;
    // Hair is moved off the face toward the side it is combed to.
    let mut face_side = Some(side);
    let jitter = hash01(sp.rest);
    // Hanging hair wanders smoothly (a random walk of its direction) rather than kinking at
    // every point, which made long hair look frizzy.
    let mut drift = Vec3::ZERO;
    // How far this hair stands off the head (one value per hair: a new one at every point
    // pushed hanging hair in and out into a crimp).
    // (Short hair lies on its set shape: nearly the same height for every hair.)
    let lift = if a.hair_style() == HairStyle::Short {
        r.range(0.85, 1.0)
    } else {
        r.range(0.4, 1.0)
    };
    let loft = loft_at(a, g, d);
    for k in 1..n {
        let t = k as f32 / (n - 1) as f32;
        let p_prev = pts[k - 1];
        let next_dir;
        if a.hair_style() == HairStyle::Bun {
            // Along the scalp toward the bun, then wound around it.
            let to_bun = bun_center - p_prev;
            if to_bun.length() < 0.045 {
                let axis = Vec3::Z;
                let around = axis.cross(p_prev - bun_center).normalize_or(Vec3::X);
                next_dir = (around + (bun_center - p_prev).normalize_or(Vec3::Z) * 0.3).normalize();
            } else {
                next_dir = (dir * 0.6 + to_bun.normalize() * 0.4).normalize();
            }
        } else {
            // Gravity pulls down more as the hair leaves the scalp.
            let pull = Vec3::NEG_Y * (g.gravity * (0.15 + 0.6 * t));
            let kick = Vec3::new(r.normal(), r.normal(), r.normal());
            // (Short hair too: a new kick at every point zigzagged each hair, and a head of
            // zigzagging hairs reads as wool.)
            let wander = if g.gravity > 0.5 {
                drift = drift * 0.85 + kick * 0.012;
                drift
            } else if g.points > 5 {
                drift = drift * 0.8 + kick * 0.012;
                drift
            } else {
                kick * (0.015 + 0.025 * g.gravity)
            };
            next_dir = (dir + pull + flow.normalize_or(Vec3::NEG_Y) * 0.05 + wander).normalize();
        }
        let mut p = p_prev + next_dir * seg;
        if g.gravity > 0.5 {
            p = keep_off_face(p, &mut face_side, jitter, lm);
        }
        let offset = 0.0008 + loft * (t / 0.4).min(1.0) * lift * tuck(a, t);
        p = collider.push_out(p, offset);
        // Hair that fell past the shoulders hangs straight on.
        dir = (p - p_prev).normalize_or(next_dir);
        pts.push(p);
    }
    pts
}

/// Where a bun sits (from the head's center, rest space): on the back of the skull, high on
/// the occiput, pinned against it as a real one is, not floating behind the head.
fn bun_seat(skin: &SkinMesh, head: Vec3) -> Vec3 {
    const HEIGHT: f32 = 0.05;
    let back = skin
        .rest
        .iter()
        .map(|r| *r - head)
        .filter(|d| d.x.abs() < 0.012 && (d.y - HEIGHT).abs() < 0.012 && d.z < 0.0)
        .map(|d| d.z)
        .fold(f32::MAX, f32::min);
    let back = if back == f32::MAX { -0.1 } else { back };
    Vec3::new(0.0, HEIGHT, back - 0.018)
}

/// A coarse grid of guide roots for nearest-guide lookups.
struct GuideGrid {
    cells: HashMap<[i32; 3], Vec<u32>>,
}

impl GuideGrid {
    const CELL: f32 = 0.02;

    fn new(guides: &[(SkinPoint, Vec<Vec3>)]) -> Self {
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (i, (sp, _)) in guides.iter().enumerate() {
            cells.entry(Self::key(sp.p)).or_default().push(i as u32);
        }
        Self { cells }
    }

    fn key(p: Vec3) -> [i32; 3] {
        (p / Self::CELL).floor().as_ivec3().to_array()
    }

    /// The three guides nearest a root, weighted by closeness: a hair at a guide's root
    /// takes nearly all its motion, one between guides takes their average.
    fn follow(
        &self,
        p: Vec3,
        guides: &[(SkinPoint, Vec<Vec3>)],
        allowed: impl Fn(usize) -> bool,
    ) -> Follow {
        /// Within this distance of a root (m), the root's guide dominates.
        const NEAR: f32 = 0.004;
        let k = Self::key(p);
        let mut best = [(u32::MAX, f32::MAX); 3];
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &i in list.iter().filter(|&&i| allowed(i as usize)) {
                        let d = (guides[i as usize].0.p - p).length_squared();
                        if d < best[2].1 {
                            best[2] = (i, d);
                            best.sort_by(|a, b| a.1.total_cmp(&b.1));
                        }
                    }
                }
            }
        }
        if best[0].0 == u32::MAX {
            return match nearest_guide(p, guides) {
                Some(i) => Follow {
                    guides: [i as u32, u32::MAX, u32::MAX],
                    weights: [1.0, 0.0, 0.0],
                },
                None => Follow::NONE,
            };
        }
        let w = best.map(|(i, d)| {
            if i == u32::MAX {
                0.0
            } else {
                1.0 / (d + NEAR * NEAR)
            }
        });
        let sum: f32 = w.iter().sum();
        Follow {
            guides: best.map(|(i, _)| i),
            weights: w.map(|w| w / sum),
        }
    }

    fn nearest(
        &self,
        p: Vec3,
        guides: &[(SkinPoint, Vec<Vec3>)],
        allowed: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let k = Self::key(p);
        let mut best: Option<(usize, f32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &i in list.iter().filter(|&&i| allowed(i as usize)) {
                        let d = (guides[i as usize].0.p - p).length_squared();
                        if best.is_none_or(|(_, b)| d < b) {
                            best = Some((i as usize, d));
                        }
                    }
                }
            }
        }
        best.map(|(i, _)| i)
    }
}

/// Occlusion of every point from the hair around it: a density grid (2.5 mm cells) of all
/// points, summed along the way out from the head center (light into a hairdo comes mostly
/// from outside it).
fn occlude(set: &mut StrandSet, head: Vec3) {
    const CELL: f32 = 0.0025;
    let mut density: HashMap<[i32; 3], f32> = HashMap::new();
    let key = |p: Vec3| (p / CELL).floor().as_ivec3().to_array();
    for p in &set.points {
        let w = p[3] / 0.00005;
        *density.entry(key(Vec3::new(p[0], p[1], p[2]))).or_default() += w;
    }
    let per_cell = match set.kind {
        StrandKind::Scalp => 0.17,
        StrandKind::Beard => 0.15,
        _ => 0.05,
    };
    for (p, attr) in set.points.iter().zip(set.attrs.iter_mut()) {
        let p = Vec3::new(p[0], p[1], p[2]);
        let out = (p - head).normalize_or(Vec3::Y);
        let mut sum = 0.0;
        for step in 1..12 {
            let q = p + out * (CELL * step as f32);
            sum += density.get(&key(q)).copied().unwrap_or(0.0);
        }
        // Near the root the scalp itself shadows too.
        let t = attr[2];
        let root = 0.55 + 0.45 * ((t / 0.25).min(1.0)).max(attr[3]);
        // (Times the shade the strand was grown with: the gaps between locks.)
        attr[0] *= (-sum * per_cell).exp().max(0.08) * root;
    }
}

/// Grid cell of the hair shadow density (m).
const SHADOW_CELL: f32 = 0.0025;
/// How far the shader marches toward a light (cells); the grid keeps this margin around the
/// hair.
pub const SHADOW_REACH: usize = 24;
/// Optical depth of one cell inside the skin (the body is opaque); the most a cell holds.
pub const BODY_TAU: f32 = 4.0;
/// Hair is more opaque than its fibers' bare cross-section: drawn hairs are still a little fewer
/// than real ones (see `HAIR_WIDTH`), and light crossing a fiber is mostly turned away.
const HAIR_OPACITY: f32 = 2.2;

/// How much hair and body fills the space around the hair: per 2.5 mm cell, the optical depth
/// of crossing its hair and of crossing its body, each as a share of [`BODY_TAU`] in a byte
/// (two bytes a cell, x fastest, then y, then z). Uploaded as a 3D texture; the strand shader
/// marches each light through both, the skin shader through the hair only (the skin shades
/// itself).
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowGrid {
    /// The grid's low corner (model space, m).
    pub lo: Vec3,
    pub cell: f32,
    pub dims: [u32; 3],
    pub density: Vec<u8>,
}

impl ShadowGrid {
    /// Where the grid sits, for the shaders: low corner and cell size, then cells along x, y,
    /// z (`GpuCharacter::hair_grid`).
    pub fn placement(&self) -> [[f32; 4]; 2] {
        let [x, y, z] = self.dims.map(|d| d as f32);
        [[self.lo.x, self.lo.y, self.lo.z, self.cell], [x, y, z, 0.0]]
    }

    /// Optical depth from `p` toward `dir` (unit), marched as the shader does (from a cell out,
    /// one cell a step, nearest cell).
    pub fn depth_toward(&self, p: Vec3, dir: Vec3) -> f32 {
        let mut tau = 0.0;
        for step in 1..=SHADOW_REACH {
            let q = (p + dir * (self.cell * step as f32) - self.lo) / self.cell;
            let c = q.floor();
            if c.cmplt(Vec3::ZERO).any() || c.cmpge(UVec3::from(self.dims).as_vec3()).any() {
                break;
            }
            let [x, y, z] = [c.x as usize, c.y as usize, c.z as usize];
            let [nx, ny, _] = self.dims.map(|d| d as usize);
            let at = 2 * (x + nx * (y + ny * z));
            tau += (self.density[at] as f32 + self.density[at + 1] as f32) / 255.0 * BODY_TAU;
        }
        tau
    }
}

/// The density of the scalp hair and beard (their fibers splatted into 2.5 mm cells) and of the
/// body (the skin as a 2 cm opaque shell), around the hair with a margin of the shader's reach.
/// `None` without hair that shades itself. Like the deep opacity maps of Unreal's grooms, but
/// one grid serves every light, in any direction.
pub fn shadow_grid(sets: &[StrandSet], skin: &SkinMesh) -> Option<ShadowGrid> {
    let casts = |s: &&StrandSet| matches!(s.kind, StrandKind::Scalp | StrandKind::Beard);
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in sets.iter().filter(casts).flat_map(|s| &s.points) {
        let p = Vec3::new(p[0], p[1], p[2]);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    if lo.x > hi.x {
        return None;
    }
    let margin = SHADOW_CELL * (SHADOW_REACH + 1) as f32;
    lo -= Vec3::splat(margin);
    hi += Vec3::splat(margin);
    let dims = ((hi - lo) / SHADOW_CELL).ceil().as_uvec3().max(UVec3::ONE);
    let [nx, ny, nz] = dims.to_array().map(|d| d as usize);
    let at = |c: [usize; 3]| c[0] + nx * (c[1] + ny * c[2]);
    let cell = |p: Vec3| -> Option<[usize; 3]> {
        let c = ((p - lo) / SHADOW_CELL).floor();
        (c.cmpge(Vec3::ZERO).all() && c.cmplt(dims.as_vec3()).all()).then_some([
            c.x as usize,
            c.y as usize,
            c.z as usize,
        ])
    };
    let mut density = vec![0.0f32; nx * ny * nz];
    let mut body = vec![false; nx * ny * nz];
    // Fibers: a hair of radius r crossing a cell along length l blocks about 2 r l of the
    // cell's face.
    let per_area = HAIR_OPACITY / (SHADOW_CELL * SHADOW_CELL);
    for set in sets.iter().filter(casts) {
        let n = set.points_per.max(1) as usize;
        for hair in set.points.chunks_exact(n) {
            for w in hair.windows(2) {
                let a = Vec3::new(w[0][0], w[0][1], w[0][2]);
                let b = Vec3::new(w[1][0], w[1][1], w[1][2]);
                let len = a.distance(b);
                let steps = (len / (SHADOW_CELL * 0.5)).ceil().max(1.0) as usize;
                let r = (w[0][3] + w[1][3]) * 0.5;
                let tau = 2.0 * r * (len / steps as f32) * per_area;
                for k in 0..steps {
                    let q = a.lerp(b, (k as f32 + 0.5) / steps as f32);
                    if let Some(c) = cell(q) {
                        density[at(c)] += tau;
                    }
                }
            }
        }
    }
    // The body: a shell under the skin, from a cell below it (roots sit just above) to 2 cm in,
    // over every triangle near the hair, sampled finer than the cells so it has no holes.
    let near = |p: Vec3| p.cmpge(lo).all() && p.cmple(hi).all();
    let mut shell = |p: Vec3, n: Vec3| {
        let mut depth = SHADOW_CELL;
        while depth <= 0.02 {
            if let Some(c) = cell(p - n * depth) {
                body[at(c)] = true;
            }
            depth += SHADOW_CELL * 0.5;
        }
    };
    for t in skin.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| i as usize);
        let (Some(pa), Some(pb), Some(pc)) = (
            skin.positions.get(a),
            skin.positions.get(b),
            skin.positions.get(c),
        ) else {
            continue;
        };
        if !(near(*pa) || near(*pb) || near(*pc)) {
            continue;
        }
        let n = (skin.normals[a] + skin.normals[b] + skin.normals[c]).normalize_or_zero();
        let edge = pa.distance(*pb).max(pb.distance(*pc)).max(pc.distance(*pa));
        let k = (edge / (SHADOW_CELL * 0.5)).ceil().max(1.0) as usize;
        for i in 0..=k {
            for j in 0..=k - i {
                let (u, v) = (i as f32 / k as f32, j as f32 / k as f32);
                shell(*pa + (*pb - *pa) * u + (*pc - *pa) * v, n);
            }
        }
    }
    Some(ShadowGrid {
        lo,
        cell: SHADOW_CELL,
        dims: dims.to_array(),
        density: density
            .into_iter()
            .zip(body)
            .flat_map(|(t, body)| {
                [
                    ((t / BODY_TAU).clamp(0.0, 1.0) * 255.0).round() as u8,
                    if body { 255 } else { 0 },
                ]
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hairs_blend_their_nearest_guides() {
        let guide = |x: f32| {
            let p = Vec3::new(x, 0.7, 0.0);
            (
                SkinPoint {
                    p,
                    n: Vec3::Y,
                    rest: p,
                    mask: 1.0,
                },
                vec![p],
            )
        };
        let guides = vec![guide(0.0), guide(0.01), guide(0.02), guide(0.2)];
        let grid = GuideGrid::new(&guides);
        // At a guide's root: nearly all of its motion.
        let at = grid.follow(Vec3::new(0.0, 0.7, 0.0), &guides, |_| true);
        assert_eq!(at.guides[0], 0);
        assert!(at.weights[0] > 0.7, "{at:?}");
        // Halfway between two guides: an even share of both, and the weights sum to 1.
        let mid = grid.follow(Vec3::new(0.005, 0.7, 0.0), &guides, |_| true);
        assert!((mid.weights[0] - mid.weights[1]).abs() < 1e-4, "{mid:?}");
        assert!((mid.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // Far from every guide but one: that one alone.
        let far = grid.follow(Vec3::new(0.3, 0.7, 0.0), &guides, |_| true);
        assert_eq!(far.guides[0], 3);
        assert!((far.weights[0] - 1.0).abs() < 1e-5, "{far:?}");
    }

    /// Triangles over an n × n grid of vertices (row-major).
    fn grid_triangles(n: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for y in 0..n - 1 {
            for z in 0..n - 1 {
                let i = y * n + z;
                out.extend([i, i + 1, i + n, i + 1, i + n + 1, i + n]);
            }
        }
        out
    }

    fn wall() -> SkinMesh {
        // A patch of skin at x = 10 cm facing -X (toward the hair).
        SkinMesh {
            positions: (0..20)
                .flat_map(|y| {
                    (0..20).map(move |z| Vec3::new(0.1, y as f32 * 0.005, z as f32 * 0.005))
                })
                .collect(),
            normals: vec![-Vec3::X; 400],
            indices: grid_triangles(20),
            ..Default::default()
        }
    }

    #[test]
    fn hair_shades_the_hair_behind_it_and_the_body_shades_all() {
        // A 4 cm slab of vertical hairs (x 0..4 cm, z 0..4 cm), 1 mm apart, 10 cm long.
        let mut s = StrandSet::new(StrandKind::Scalp, 3);
        for i in 0..40 {
            for j in 0..40 {
                let (x, z) = (i as f32 * 0.001, j as f32 * 0.001);
                let pts = [0.0, 0.05, 0.1].map(|y| Vec3::new(x, y, z));
                s.push(&pts, 0.00005, 0.00005, 0.5);
            }
        }
        let grid = shadow_grid(&[s], &wall()).unwrap();
        let at = |x: f32| Vec3::new(x, 0.05, 0.02);
        // On the -X face, light from -X is barely dimmed; from +X it crosses the slab.
        let open = grid.depth_toward(at(0.0), -Vec3::X);
        let through = grid.depth_toward(at(0.0), Vec3::X);
        assert!(open < 0.1 && through > 2.0, "{open} {through}");
        // Deeper in, more hair lies toward -X; from above, little.
        assert!(grid.depth_toward(at(0.03), -Vec3::X) > grid.depth_toward(at(0.01), -Vec3::X));
        assert!(grid.depth_toward(Vec3::new(0.02, 0.098, 0.02), Vec3::Y) < 0.5);
        // Brows alone cast nothing.
        let mut brows = StrandSet::new(StrandKind::Brows, 3);
        brows.push(&[Vec3::ZERO, Vec3::X, Vec3::Y], 1.0, 0.0, 0.5);
        assert!(shadow_grid(&[brows], &wall()).is_none());
        // The shader marches the same reach and scale.
        let shader = include_str!("shaders/common.wgsl");
        assert!(shader.contains(&format!("const SHADOW_REACH: u32 = {SHADOW_REACH}u;")));
        assert!(shader.contains(&format!("const SHADOW_BODY_TAU: f32 = {BODY_TAU:?};")));
    }

    #[test]
    fn hair_near_the_body_is_shaded_by_it() {
        let mut s = StrandSet::new(StrandKind::Scalp, 2);
        s.push(
            &[Vec3::new(0.09, 0.05, 0.05), Vec3::new(0.09, 0.06, 0.05)],
            0.00005,
            0.00005,
            0.5,
        );
        let grid = shadow_grid(&[s], &wall()).unwrap();
        let p = Vec3::new(0.09, 0.055, 0.05);
        // Light from +X is behind the skin; from -X open.
        assert!(grid.depth_toward(p, Vec3::X) > 4.0);
        assert!(grid.depth_toward(p, -Vec3::X) < 0.1);
    }

    #[test]
    fn sets_count_their_strands() {
        let mut s = StrandSet::new(StrandKind::Brows, 3);
        s.push(&[Vec3::ZERO, Vec3::X, Vec3::Y], 1.0, 0.0, 0.5);
        assert_eq!(s.strand_count(), 1);
        assert_eq!(s.points[2][3], 0.0);
        assert_eq!(s.attrs[1][2], 0.5);
    }

    #[test]
    fn packing_keeps_points_and_attributes() {
        let mut s = StrandSet::new(StrandKind::Scalp, 2);
        s.push(
            &[Vec3::new(-0.1, 0.2, 0.05), Vec3::new(0.15, 0.4, -0.1)],
            1e-4,
            4e-5,
            0.25,
        );
        s.attrs[1][0] = 0.5;
        let p = s.packed();
        let unpack = |v: u32| [(v & 0xffff) as f32 / 65535.0, (v >> 16) as f32 / 65535.0];
        for (i, q) in p.points.iter().enumerate() {
            let [x, y] = unpack(q[0]);
            let [z, r] = unpack(q[1]);
            let at =
                Vec3::from_slice(&p.box_min) + Vec3::new(x, y, z) * Vec3::from_slice(&p.box_size);
            let want = Vec3::new(s.points[i][0], s.points[i][1], s.points[i][2]);
            assert!(at.distance(want) < 1e-5, "{at} {want}");
            assert!((r * p.box_min[3] - s.points[i][3]).abs() < 1e-8);
        }
        let [occ, random] = unpack(p.attrs[1]);
        assert!((occ - 0.5).abs() < 1e-4 && (random - 0.25).abs() < 1e-4);
        assert_eq!(unpack(p.attrs[0])[0], 1.0);
    }

    #[test]
    fn tangents_lie_in_the_plane() {
        let n = Vec3::new(0.0, 0.0, 1.0);
        let t = tangent(Vec3::new(1.0, 1.0, 5.0), n);
        assert!(t.dot(n).abs() < 1e-6);
        assert!((t.length() - 1.0).abs() < 1e-5);
        let any = tangent(n, n);
        assert!(any.dot(n).abs() < 1e-6);
    }
}
