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

use engine_core::math::{UVec3, Vec3};

use crate::appearance::{Appearance, AppearanceExt, BeardStyle, HairStyle, Rng};
use crate::mesh::{Landmarks, MASKS, Mask, SkinMesh};

/// Drawn hairs are fewer than real ones (tens per cm² where a scalp has ~150), so each stands
/// for a few and is drawn that much wider; this scales every hair's width.
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

fn grow_brows(a: &Appearance, skin: &SkinMesh, rest: &Landmarks) -> StrandSet {
    const N: u32 = 5;
    let mut set = StrandSet::new(StrandKind::Brows, N);
    let sampler = Sampler::new(skin, Mask::Brows, |_, m| m * m);
    let thick = a.brows.thickness.clamp(0.0, 1.0);
    // Real brows hold ~250 hairs per cm²; a fraction reads as full once drawn.
    let count = ((sampler.total() * 1e4) * (90.0 + 130.0 * thick)) as usize;
    let mut r = Rng::new(a.seed ^ 0xB120);
    for _ in 0..count {
        let Some(sp) = sampler.sample(&mut r) else {
            break;
        };
        let side = sp.rest.x.signum();
        let eye = if side > 0.0 {
            rest.eyes[0]
        } else {
            rest.eyes[1]
        };
        let u = ((sp.rest.x * side - (eye.x * side - 0.017)) / 0.041).clamp(0.0, 1.0);
        let center = eye.y + 0.0165 + 0.004 * (1.0 - ((u - 0.6) / 0.6).powi(2)).max(0.0);
        let v = ((sp.rest.y - center) / 0.005).clamp(-1.5, 1.5);
        // Angle of the hair above the lateral direction: inner hairs stand up, the rest lie
        // along the brow, lower hairs rising and upper ones falling into a ridge.
        let inner = 1.0 - (u / 0.22).clamp(0.0, 1.0);
        let along = 0.35 - 0.45 * v - 0.25 * u;
        let angle = inner * 1.25 + (1.0 - inner) * along + r.normal() * 0.12;
        let lateral = Vec3::X * side;
        let d = tangent(lateral * angle.cos() + Vec3::Y * angle.sin(), sp.n);
        let len = (0.0072 - 0.0026 * u) * r.range(0.7, 1.1) * (0.85 + 0.3 * thick);
        let lift = r.range(0.05, 0.14);
        let mut pts = [Vec3::ZERO; N as usize];
        for (i, p) in pts.iter_mut().enumerate() {
            let t = i as f32 / (N - 1) as f32;
            // Out of the skin at the root, then lying down along it.
            let h = len * lift * (t * 2.0).min(1.0) * (1.0 - 0.6 * t) + 0.0002;
            *p = sp.p + d * (len * t) + sp.n * h;
        }
        set.push(&pts, 0.00008 * HAIR_WIDTH, 0.00003 * HAIR_WIDTH, r.f32());
    }
    set
}

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
            let count = if upper { 130 } else { 60 };
            for _ in 0..count {
                // A point along the margin, inner corner (u 0) to outer (u 1).
                let mut s = r.f32() * total;
                let mut root = lid[lid.len() - 1];
                for (k, l) in lengths.iter().enumerate() {
                    if s <= *l {
                        root = lid[k] + (lid[k + 1] - lid[k]) * (s / l.max(1e-6));
                        break;
                    }
                    s -= l;
                }
                let u = ((root.x - lid[0].x) * sx
                    / ((lid[lid.len() - 1].x - lid[0].x) * sx).max(1e-4))
                .clamp(0.0, 1.0);
                // Out from the eye, forward, and curling up (upper) or down (lower), fanning
                // outward toward the outer corner.
                let out = (root - eye).normalize_or(Vec3::Z);
                let up = if upper { 1.0 } else { -1.0 };
                let fan = Vec3::new(sx * (u - 0.35) * 0.6, 0.0, 0.0);
                let d = (out * 0.5 + Vec3::Z * 0.9 + fan).normalize();
                let peak = if upper { 0.0088 } else { 0.0042 };
                // Longest a little outside the middle, short at the corners.
                let shape = (1.0 - ((u - 0.6) / 0.62).powi(2)).max(0.12);
                let len = peak * shape * r.range(0.75, 1.1);
                let curl = if upper { 0.4 } else { 0.2 };
                let mut pts = [Vec3::ZERO; N as usize];
                for (k, p) in pts.iter_mut().enumerate() {
                    let t = k as f32 / (N - 1) as f32;
                    *p = root + out * 0.0003 + d * (len * t) + Vec3::Y * (up * len * curl * t * t);
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
        BeardStyle::Short => (0.009, 60.0, 6),
        BeardStyle::Full => (0.03, 60.0, 9),
        BeardStyle::Goatee => (0.014, 60.0, 7),
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
        let down = if above_lips {
            Vec3::new(sp.rest.x.signum() * 0.6, -1.0, 0.1)
        } else {
            Vec3::new(sp.rest.x.signum() * 0.12, -1.0, 0.25)
        };
        let d = tangent(down, sp.n);
        let l = len * r.range(0.6, 1.2);
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
        let root = if stubble { 0.00005 } else { 0.00009 };
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
            density: 110.0,
            gravity: 0.0,
            loft: 0.0015,
            clump: 0.0,
            root_radius: 0.00009,
        },
        HairStyle::Short => Groom {
            length: 0.055 * k,
            points: 10,
            density: 80.0,
            gravity: 0.25,
            loft: 0.009,
            clump: 0.55,
            root_radius: 0.00011,
        },
        HairStyle::Medium => Groom {
            length: 0.2 * k,
            points: 18,
            density: 120.0,
            gravity: 1.0,
            loft: 0.016,
            clump: 0.6,
            root_radius: 0.00012,
        },
        HairStyle::Long => Groom {
            length: 0.42 * k,
            points: 32,
            density: 140.0,
            gravity: 1.0,
            loft: 0.012,
            clump: 0.75,
            root_radius: 0.00012,
        },
        HairStyle::Bun => Groom {
            length: 0.3,
            points: 20,
            density: 80.0,
            gravity: 0.0,
            loft: 0.004,
            clump: 0.6,
            root_radius: 0.00012,
        },
    };
    let head = rest.head;
    let recession = a.hair.recession.clamp(0.0, 1.0);
    let sampler = Sampler::new(skin, Mask::Scalp, |rest, m| {
        // A receded hairline: thinner front and temples (and the crown at the far end).
        let d = rest - head;
        let theta = d.x.atan2(d.z);
        let line = crate::mesh::hairline(theta, recession);
        // A soft hairline: hairs thin out over a centimetre (and grow finer, below).
        let edge = ((d.y - line) / 0.012).clamp(0.0, 1.0).powf(0.7);
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
    let per_guide = if g.gravity > 0.5 { 90 } else { 40 };
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
    let guide_grid = GuideGrid::new(&guides);
    // Long and medium hair moves: every hair with the guide nearest its root.
    let moves = g.gravity > 0.5 && !guides.is_empty();
    for _ in 0..total {
        let Some(sp) = sampler.sample(&mut r) else {
            break;
        };
        // Hairs at the hairline are short and fine (vellus), as on a real forehead.
        let d = sp.rest - head;
        let line = crate::mesh::hairline(d.x.atan2(d.z), recession);
        let fine = ((d.y - line) / 0.015).clamp(0.0, 1.0);
        let scale = r.range(0.85, 1.1) * (0.3 + 0.7 * fine);
        let thin = 0.55 + 0.45 * fine;
        // A few hairs stray from the groom: flyaways.
        // (Not from the front hairline, where they would cross the forehead.)
        let at_front = fine < 1.0 || (d.z > 0.03 && d.y < 0.07);
        let flyaway = g.points > 5 && !at_front && r.f32() < 0.015;
        let mut pts = grow_hair(a, &g, sp, collider, lm, rest, &mut r, scale);
        let mut lock_tone = None;
        // How far out in its lock the hair is (0 on the guide .. 1 at the lock's edge).
        let mut lock_edge = 0.0;
        // Follow the nearest guide if it is close (a lone hair far from any guide stays free).
        if !flyaway
            && let Some(gi) = guide_grid.nearest(sp.p, &guides)
            && (guides[gi].0.p - sp.p).length() < if g.gravity > 0.5 { 0.025 } else { 0.012 }
        {
            let (gsp, gpts) = &guides[gi];
            let off = sp.p - gsp.p;
            let c = g.clump * r.range(if g.gravity > 0.5 { 0.8 } else { 0.6 }, 1.0);
            // The hairs of a lock share most of its tone.
            lock_tone = Some(hash01(gsp.rest));
            lock_edge = (off.length() / 0.025).min(1.0);
            let last = pts.len() - 1;
            for (k, p) in pts.iter_mut().enumerate() {
                let t = k as f32 / last as f32;
                let follow = gpts[k] + off * (1.0 - c * t.powf(0.8));
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
        // Hanging hair waves with its lock (above), so its own coil is small and at least
        // eight points a turn: a coarser helix zigzags, and the zigzags of neighboring hairs
        // cross like straw.
        if curl > 0.02 && g.points > 4 {
            let hanging = g.gravity > 0.5;
            let mut radius = (0.002 + 0.01 * curl) * (g.length / 0.05).min(1.0);
            let mut turns = 2.0 + 10.0 * curl * (g.length / 0.2).min(2.0);
            if hanging {
                radius *= 0.3;
                turns = turns.min((g.points - 1) as f32 / 8.0);
            }
            let phase = r.f32() * std::f32::consts::TAU;
            let last = pts.len() - 1;
            let base = pts.clone();
            for k in 1..pts.len() {
                let t = k as f32 / last as f32;
                let dir = (base[k] - base[k - 1]).normalize_or(Vec3::NEG_Y);
                // (A frame that turns smoothly along the hair for hanging hair.)
                let side = if hanging {
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
            *p = collider.push_out(*p, 0.0008 + g.loft * t.min(0.4) / 0.4 * 0.6);
        }
        // Hairs taper to a fine tip.
        set.push(
            &pts,
            g.root_radius * HAIR_WIDTH * thin,
            g.root_radius * HAIR_WIDTH * thin * 0.35,
            lock_tone.map_or(r.f32(), |l| 0.6 * l + 0.4 * r.f32()),
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
            set.follow.push(guide_grid.follow(sp.p, &guides));
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
const PART_X: f32 = 0.025;

/// Hair that would hang in front of the face is moved aside to frame it: easing out over the
/// forehead (so a hair bends aside instead of jumping), then beside the eyes and cheeks and back
/// into the side curtain rather than in front of the face. A hair goes aside on one side
/// (`side`, kept from point to point, picked where it first needs moving if not given), so it
/// never jumps across; `jitter` (0..1) spreads the hairs that were moved so they don't line up.
/// Over the forehead hair stays up at the hairline (swept to the side along it, then down past
/// the temple): a lock lying across the brow reads as a band, not a fringe. For hair already
/// grown (lifting while growing would send it out sideways).
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
    if d.x * s >= half {
        return p;
    }
    // From the eyes down, also back toward the ears.
    let back = ((0.01 - d.y) / 0.03).clamp(0.0, 1.0);
    let z = p.z.min(lm.head.z + 0.035 + 0.03 * jitter);
    Vec3::new(lm.head.x + s * half, p.y, p.z + (z - p.z) * back)
}

/// A stable 0..1 value from a position (so a hair's guide pass and final pass agree).
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
            let front = ((d.z + 0.03) / 0.07).clamp(0.0, 1.0);
            // On the part's narrow side, hairline hair goes more to the side than back, so the
            // front of the part stays covered (the wide side would sweep across the forehead).
            let hairline = ((d.z - 0.02) / 0.04).clamp(0.0, 1.0) * f32::max(side, 0.0);
            let swept =
                Vec3::new(side * 0.9, -0.2, -0.65).lerp(Vec3::new(side, -0.3, -0.3), hairline);
            let down = Vec3::new(d.x * 3.0, -1.0, -0.35);
            swept * front + down * (1.0 - front)
        }
        HairStyle::Bun => {
            // Pulled toward the bun at the back of the head.
            let bun = Vec3::new(0.0, 0.055, -0.115);
            (bun - d).normalize_or(Vec3::NEG_Z)
        }
        HairStyle::Bald => Vec3::NEG_Y,
    };
    let len = g.length * length_scale;
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
        // Coily hair isn't combed flat: it stands up and points every way.
        let any = tangent(Vec3::new(r.normal(), r.normal(), r.normal()), sp.n);
        along = (along * (1.0 - 0.85 * curl) + any * curl).normalize_or(along);
        rise += 0.5 * curl;
    }
    let mut dir = (along + sp.n * rise).normalize();
    let bun_center = lm.head + Vec3::new(0.0, 0.06, -0.125);
    // Hair is moved off the face toward the side it is combed to.
    let mut face_side = Some(side);
    let jitter = hash01(sp.rest);
    // Hanging hair wanders smoothly (a random walk of its direction) rather than kinking at
    // every point, which made long hair look frizzy.
    let mut drift = Vec3::ZERO;
    // How far this hair stands off the head (one value per hair: a new one at every point
    // pushed hanging hair in and out into a crimp).
    let lift = r.range(0.4, 1.0);
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
            let wander = if g.gravity > 0.5 {
                drift = drift * 0.85 + kick * 0.012;
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
        let offset = 0.0008 + g.loft * (t / 0.4).min(1.0) * lift;
        p = collider.push_out(p, offset);
        // Hair that fell past the shoulders hangs straight on.
        dir = (p - p_prev).normalize_or(next_dir);
        pts.push(p);
    }
    pts
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
    fn follow(&self, p: Vec3, guides: &[(SkinPoint, Vec<Vec3>)]) -> Follow {
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
                    for &i in list {
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

    fn nearest(&self, p: Vec3, guides: &[(SkinPoint, Vec<Vec3>)]) -> Option<usize> {
        let k = Self::key(p);
        let mut best: Option<(usize, f32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &i in list {
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

/// Occlusion of every point from the hair around it: a density grid (4 mm cells) of all
/// points, summed along the way out from the head center (light into a hairdo comes mostly
/// from outside it).
fn occlude(set: &mut StrandSet, head: Vec3) {
    const CELL: f32 = 0.004;
    let mut density: HashMap<[i32; 3], f32> = HashMap::new();
    let key = |p: Vec3| (p / CELL).floor().as_ivec3().to_array();
    for p in &set.points {
        let w = p[3] / 0.00005;
        *density.entry(key(Vec3::new(p[0], p[1], p[2]))).or_default() += w;
    }
    let per_cell = match set.kind {
        StrandKind::Scalp => 0.045,
        StrandKind::Beard => 0.06,
        _ => 0.02,
    };
    for (p, attr) in set.points.iter().zip(set.attrs.iter_mut()) {
        let p = Vec3::new(p[0], p[1], p[2]);
        let out = (p - head).normalize_or(Vec3::Y);
        let mut sum = 0.0;
        for step in 1..8 {
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
const SHADOW_CELL: f32 = 0.004;
/// How far the shader marches toward a light (cells); the grid keeps this margin around the
/// hair.
pub const SHADOW_REACH: usize = 16;
/// Optical depth of one cell inside the skin (the body is opaque); the most a cell holds.
pub const BODY_TAU: f32 = 4.0;
/// Hair is more opaque than its fibers' bare cross-section: drawn hairs stand for a few real
/// ones (see `HAIR_WIDTH`), and light crossing a fiber is mostly turned away.
const HAIR_OPACITY: f32 = 1.5;

/// How much hair and body fills the space around the hair: per 4 mm cell, the optical depth
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

/// The density of the scalp hair and beard (their fibers splatted into 4 mm cells) and of the
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
        let at = grid.follow(Vec3::new(0.0, 0.7, 0.0), &guides);
        assert_eq!(at.guides[0], 0);
        assert!(at.weights[0] > 0.7, "{at:?}");
        // Halfway between two guides: an even share of both, and the weights sum to 1.
        let mid = grid.follow(Vec3::new(0.005, 0.7, 0.0), &guides);
        assert!((mid.weights[0] - mid.weights[1]).abs() < 1e-4, "{mid:?}");
        assert!((mid.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // Far from every guide but one: that one alone.
        let far = grid.follow(Vec3::new(0.3, 0.7, 0.0), &guides);
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
    fn tangents_lie_in_the_plane() {
        let n = Vec3::new(0.0, 0.0, 1.0);
        let t = tangent(Vec3::new(1.0, 1.0, 5.0), n);
        assert!(t.dot(n).abs() < 1e-6);
        assert!((t.length() - 1.0).abs() < 1e-5);
        let any = tangent(n, n);
        assert!(any.dot(n).abs() < 1e-6);
    }
}
