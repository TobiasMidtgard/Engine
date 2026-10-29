//! Landmarks: the big rock formations that make a place on a planet memorable from the ground
//! (sandstone arches, fields of hoodoos, layered buttes, granite tors, ice spires).
//!
//! A heightfield cannot hold an arch or an overhanging caprock, so, as Peytavie et al. 2009
//! ("Arches") and Paris et al. 2019 ("Terrain Amplification with Implicit 3D Features") do,
//! the planet keeps its heightfield and the formations stand on it as separate 3D shapes.
//! Each is a signed distance function in its own frame (+Y up), built from the primitives of
//! those papers and Dey et al. 2017 (an arch is a sphere swept along a Bezier, a window arch a
//! fin with an opening worn through it), meshed with naive surface nets on a worker thread.
//!
//! The rock is bedded: soft beds recede and hard ones stand out, so hoodoos are ribbed and
//! waisted under a hard caprock and buttes step up in ledges over a talus apron. The beds
//! follow height above sea level across the whole planet, so a hoodoo's ribs line up with the
//! ledges of the butte behind it. Cliffs carry dark streaks of desert varnish.
//!
//! Formations are placed from a hash of the terrain's quadtree cells (about a kilometer
//! across), so the same ones come back wherever you return, and gather in badlands regions
//! tens of kilometers wide. They stand only where the land suits them: dry ground for arches,
//! hoodoos and buttes, uplands for tors, frozen ground for ice spires; never in the sea, a
//! river, on steep ground, a flat pad or the hand-made overlay. Each cell is one entity with
//! one mesh (ice in a second) and a static triangle-mesh collider while a focus is near.

use std::collections::{HashMap, HashSet};
use std::f64::consts::{PI, TAU};
use std::thread::JoinHandle;

use engine_assets::{Assets, Handle};
use engine_core::math::{DQuat, DVec2, DVec3};
use engine_core::prelude::*;
use engine_render::{
    MaterialPreset, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial, Vertex,
};
use engine_world::physics::{Collider, ColliderShape};
use engine_world::{FocusPoints, SurfaceMaterials};
use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkSource, Sample};
use crate::cube::{NodeKey, SurfaceMap, TerrainDomain};
use crate::edits::EditLayer;
use crate::noise::{cell_random, noise3};
use crate::{PlanetTerrain, Terrains};

/// Where formations stand and what they look like (see the module docs). Off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LandmarkField {
    /// Formations per km² on land that suits them (0 = none). They gather in badlands
    /// regions tens of kilometers across, about three times as thick there.
    pub per_km2: f64,
    /// How often each kind is chosen where it can stand (relative weights).
    pub arches: f64,
    pub hoodoos: f64,
    pub buttes: f64,
    pub tors: f64,
    pub ice_spires: f64,
    /// Arches, hoodoos and buttes stand only on ground drier than this (moisture 0..1).
    pub dry_below: f32,
    /// Tors stand on ground drier than this, at least `tors_above_m` above sea level.
    pub tors_below: f32,
    pub tors_above_m: f64,
    /// Ice spires stand only where it is colder than this, °C.
    pub ice_below_c: f32,
    /// Keep this far from flat pads and hand-made ground, meters.
    pub clearance_m: f64,
    /// Linear colors of soft beds, hard beds and the caprock.
    pub soft: [f32; 3],
    pub hard: [f32; 3],
    pub cap: [f32; 3],
    /// Linear colors of granite (tors) and of ice.
    pub granite: [f32; 3],
    pub ice: [f32; 3],
    /// Thickness of a bed of rock, meters.
    pub bed_m: f64,
    /// Formations at least 18 m tall are drawn, coarsely, out to this far, meters (at most
    /// the near reach of 6 km: only the near ones).
    pub far_m: f64,
    /// The stone's surface texture, tinted by the colors (none: plain).
    pub texture: Option<MaterialPreset>,
}

impl Default for LandmarkField {
    fn default() -> Self {
        Self {
            per_km2: 0.0,
            arches: 1.0,
            hoodoos: 1.2,
            buttes: 1.0,
            tors: 0.0,
            ice_spires: 0.0,
            dry_below: 0.3,
            tors_below: 0.75,
            tors_above_m: 300.0,
            ice_below_c: -10.0,
            clearance_m: 5_000.0,
            soft: [0.34, 0.12, 0.05],
            hard: [0.45, 0.25, 0.13],
            cap: [0.5, 0.43, 0.34],
            granite: [0.42, 0.4, 0.37],
            ice: [0.62, 0.74, 0.86],
            bed_m: 4.0,
            far_m: 30_000.0,
            texture: Some(MaterialPreset::Rock),
        }
    }
}

impl LandmarkField {
    /// Earth: red sandstone arches, hoodoos and buttes in the deserts (the Colorado Plateau's
    /// colors), granite tors on dry uplands.
    pub fn earth() -> Self {
        Self {
            per_km2: 0.35,
            dry_below: 0.22,
            tors: 0.5,
            ..Self::default()
        }
    }

    /// Mars: dusty layered buttes and mesas, hoodoos, now and then an arch, in red-brown
    /// beds under darker caprock.
    pub fn mars() -> Self {
        Self {
            per_km2: 0.3,
            arches: 0.35,
            hoodoos: 0.8,
            buttes: 1.4,
            soft: [0.27, 0.11, 0.05],
            hard: [0.33, 0.17, 0.09],
            cap: [0.2, 0.11, 0.07],
            bed_m: 5.0,
            ..Self::default()
        }
    }

    /// Granite tors on the uplands of a living world, sandstone where it is dry.
    pub fn terra() -> Self {
        Self {
            per_km2: 0.25,
            tors: 1.5,
            tors_above_m: 150.0,
            ..Self::default()
        }
    }

    /// Fields of ice spires on a frozen world.
    pub fn ice() -> Self {
        Self {
            per_km2: 0.4,
            arches: 0.0,
            hoodoos: 0.0,
            buttes: 0.0,
            ice_spires: 1.0,
            ice_below_c: 0.0,
            ..Self::default()
        }
    }

    /// The same field with its rock colors moved toward `color` by `amount` (0..1), keeping
    /// each one's brightness.
    pub fn tinted(mut self, color: [f32; 3], amount: f32) -> Self {
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let target = luma(color).max(1e-3);
        for c in [&mut self.soft, &mut self.hard, &mut self.cap] {
            let l = luma(*c);
            for i in 0..3 {
                c[i] += (color[i] / target * l - c[i]) * amount;
            }
        }
        self
    }
}

/// A kind of formation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LandmarkKind {
    /// A sandstone arch: a fin with an opening worn through it, or a free-standing span.
    Arch,
    /// A field of hoodoos: ribbed, waisted pillars under hard caps.
    Hoodoos,
    /// A layered butte: cliffs stepping up in ledges to a flat caprock, over a talus apron.
    Butte,
    /// A granite tor: stacked, rounded blocks.
    Tor,
    /// A field of ice spires.
    IceSpires,
}

/// One formation on a terrain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landmark {
    pub kind: LandmarkKind,
    /// Terrain-local point on the ground at its center, and the local up there.
    pub position: DVec3,
    pub up: DVec3,
    /// Height of its tallest part above the ground, and its width, meters.
    pub height_m: f64,
    pub width_m: f64,
    yaw: f64,
    seed: u64,
    /// Height of the ground at the center above sea level.
    ground_asl: f64,
}

impl Landmark {
    /// Terrain-local direction it shows best from (or from the opposite side): across an
    /// arch's opening.
    pub fn front(&self) -> DVec3 {
        self.rotation() * DVec3::Z
    }

    /// Rotation from the formation's frame (+Y up) to terrain-local space.
    fn rotation(&self) -> DQuat {
        DQuat::from_rotation_arc(DVec3::Y, self.up) * DQuat::from_rotation_y(self.yaw)
    }
}

/// The formations within `reach_m` of terrain-local point `local` on a spherical terrain,
/// nearest first (ignores edits: for tools and tests).
pub fn landmarks_near(terrain: &PlanetTerrain, local: DVec3, reach_m: f64) -> Vec<Landmark> {
    if terrain.landmarks.per_km2 <= 0.0 {
        return Vec::new();
    }
    let source = ChunkSource {
        shape: std::sync::Arc::new(terrain.shape.clone()),
        edits: std::sync::Arc::new(EditLayer::default()),
        map: SurfaceMap::new(TerrainDomain::Sphere, terrain.shape.radius_m),
        ocean_surface: terrain.ocean_surface,
    };
    let level = level_for(&source.map);
    let mut out: Vec<Landmark> = cells_around(&source.map, local, level, reach_m)
        .into_iter()
        .flat_map(|(_, key)| sites_in(&source, &terrain.landmarks, key, terrain.shape.seed))
        .filter(|l| (l.position - local).length() <= reach_m)
        .collect();
    out.sort_by(|a, b| {
        (a.position - local)
            .length()
            .total_cmp(&(b.position - local).length())
    });
    out
}

/// Cell size along the ground, meters.
const CELL_M: f64 = 1_000.0;
/// Formations are drawn out to this far from a focus, and collide this near.
const REACH_M: f64 = 6_000.0;
const COLLIDER_REACH_M: f64 = 1_500.0;
/// Cells being built at once.
const MAX_BUILDING: usize = 2;
/// Most vertices in one formation part and in one cell.
const MAX_PART_VERTICES: usize = 250_000;
const MAX_CELL_VERTICES: usize = 900_000;

/// The quadtree level whose nodes are about [`CELL_M`] across.
fn level_for(map: &SurfaceMap) -> u8 {
    (map.root_size() / CELL_M).log2().round().clamp(0.0, 24.0) as u8
}

/// The cells within `reach_m` of terrain-local point `local` (distance to the nearest edge,
/// cell).
fn cells_around(map: &SurfaceMap, local: DVec3, level: u8, reach_m: f64) -> Vec<(f64, NodeKey)> {
    let up = local.normalize_or(DVec3::Y);
    let side = up.any_orthonormal_vector();
    let fwd = up.cross(side);
    let size = map.node_size(level);
    // Half-cell steps: toward a cube face's edges its cells are down to half the nominal size
    // along the ground, and a full step would jump over some.
    let step = size * 0.5;
    let n = (reach_m / step).ceil() as i32 + 2;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dz in -n..=n {
        for dx in -n..=n {
            let q = local + (side * dx as f64 + fwd * dz as f64) * step;
            let Some(key) = map.node_at(q, level) else {
                continue;
            };
            if !seen.insert(key) {
                continue;
            }
            let (lo, s) = key.face_rect();
            let mid = map.position(key.face, lo.x + s * 0.5, lo.y + s * 0.5, 0.0);
            let d = (mid.normalize() * local.length() - local).length() - size * 0.7;
            if d <= reach_m {
                out.push((d.max(0.0), key));
            }
        }
    }
    out
}

/// How much a place is badlands (0..3): regions tens of kilometers across where formations
/// gather, with a few between them.
fn badlands(p: DVec3, seed: u64) -> f64 {
    let n = noise3(p / 40_000.0, seed ^ 0xBAD_1A4D) * 0.7 + noise3(p / 9_000.0, seed ^ 0x51) * 0.3;
    0.08 + 2.9 * smooth(-0.05, 0.35, n)
}

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Which kind of formation suits the ground (`roll` in 0..1), if any.
fn choose(field: &LandmarkField, s: &Sample, roll: f64) -> Option<LandmarkKind> {
    let dry = s.moisture < field.dry_below;
    let tor_ground = s.moisture < field.tors_below && s.height > field.tors_above_m;
    let weights = [
        (LandmarkKind::Arch, if dry { field.arches } else { 0.0 }),
        (LandmarkKind::Hoodoos, if dry { field.hoodoos } else { 0.0 }),
        (LandmarkKind::Butte, if dry { field.buttes } else { 0.0 }),
        (LandmarkKind::Tor, if tor_ground { field.tors } else { 0.0 }),
        (
            LandmarkKind::IceSpires,
            if s.temperature_c < field.ice_below_c {
                field.ice_spires
            } else {
                0.0
            },
        ),
    ];
    let total: f64 = weights.iter().map(|w| w.1).sum();
    if total <= 0.0 {
        return None;
    }
    let mut pick = roll * total;
    for (kind, w) in weights {
        if pick < w {
            return Some(kind);
        }
        pick -= w;
    }
    None
}

/// Height and width of a formation of `kind` (`u`, `v` in 0..1), meters.
fn size_of(kind: LandmarkKind, u: f64, v: f64) -> (f64, f64) {
    match kind {
        LandmarkKind::Arch => {
            let h = 16.0 + 26.0 * u;
            (h, h * (1.5 + 0.4 * v))
        }
        LandmarkKind::Hoodoos => (12.0 + 22.0 * u, 60.0 + 80.0 * v),
        LandmarkKind::Butte => {
            let r = 30.0 + 50.0 * v;
            ((r * (0.55 + 0.6 * u)).clamp(22.0, 85.0), r * 2.0)
        }
        LandmarkKind::Tor => {
            let h = 5.0 + 9.0 * u;
            (h, h * (1.2 + 0.6 * v))
        }
        LandmarkKind::IceSpires => (7.0 + 16.0 * u, 25.0 + 45.0 * v),
    }
}

/// The formations of one cell.
fn sites_in(source: &ChunkSource, field: &LandmarkField, key: NodeKey, seed: u64) -> Vec<Landmark> {
    let map = &source.map;
    if field.per_km2 <= 0.0 || !map.is_sphere() {
        return Vec::new();
    }
    let (lo, s) = key.face_rect();
    let size = map.node_size(key.level);
    let salt = ((key.face as u64) << 56) ^ ((key.level as u64) << 48) ^ 0x1A4D_3A4C;
    let rand = |i: u64, j: u64| cell_random(key.x as i64, key.y as i64, i as i64, seed, salt ^ j);
    let center = map.position(key.face, lo.x + s * 0.5, lo.y + s * 0.5, 0.0);
    let expected = field.per_km2 * size * size / 1e6 * badlands(center, seed);
    let count = expected.floor() as u64 + u64::from(rand(0, 1) < expected.fract());
    let shape = &source.shape;
    let ground = |local: DVec3| -> Option<(DVec3, DVec3, Sample)> {
        let (dir, up, base) = map.locate(local)?;
        let s = source.sample(base, up, dir, 0.0);
        Some((base + up * s.height, up, s))
    };
    let mut out: Vec<Landmark> = Vec::new();
    for i in 1..=count {
        let local = map.position(key.face, lo.x + s * rand(i, 2), lo.y + s * rand(i, 3), 0.0);
        let Some((point, up, sample)) = ground(local) else {
            continue;
        };
        if (shape.ocean && sample.height < 3.0) || sample.water > 0.05 || sample.lake > 0.0 {
            continue;
        }
        if shape.near_hand_made(up, field.clearance_m) {
            continue;
        }
        let Some(kind) = choose(field, &sample, rand(i, 4)) else {
            continue;
        };
        let (height_m, width_m) = size_of(kind, rand(i, 5), rand(i, 6));
        // Level enough ground under the footprint.
        let t1 = up.any_orthonormal_vector();
        let t2 = up.cross(t1);
        let (mut low, mut high) = (0.0f64, 0.0f64);
        for k in 0..8 {
            let a = k as f64 / 8.0 * TAU;
            let q = local + (t1 * a.cos() + t2 * a.sin()) * width_m * 0.5;
            if let Some((_, _, s)) = ground(q) {
                low = low.min(s.height - sample.height);
                high = high.max(s.height - sample.height);
            }
        }
        if high - low > 0.3 * width_m + 4.0 {
            continue;
        }
        // Not on top of another.
        if out
            .iter()
            .any(|o| (o.position - point).length() < (o.width_m + width_m) * 0.55)
        {
            continue;
        }
        out.push(Landmark {
            kind,
            position: point,
            up,
            height_m,
            width_m,
            yaw: rand(i, 7) * TAU,
            seed: (rand(i, 8) * 4_294_967_296.0) as u64,
            ground_asl: sample.height,
        });
    }
    out
}

// --- Shapes --------------------------------------------------------------------------------

/// What a point of a formation's surface is made of (for its color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stone {
    /// Bedded rock: soft and hard beds by height.
    Beds,
    Cap,
    Talus,
    Granite,
    Ice,
}

/// One meshed piece of a formation: a shape in a frame of its own (+Y up, its foot on the
/// ground at the origin).
#[derive(Debug, Clone)]
struct Part {
    /// Offset of its frame from the formation's (in the formation's frame).
    at: DVec3,
    shape: Shape,
    /// Bounds in its own frame and voxel size.
    min: DVec3,
    max: DVec3,
    voxel: f64,
    /// Where it is cut off below the ground (≤ 0).
    bottom: f64,
}

#[derive(Debug, Clone)]
enum Shape {
    Hoodoo {
        h: f64,
        r: f64,
        cap_r: f64,
        cap_t: f64,
        squash: f64,
    },
    /// A fin along X with an opening worn through it.
    Window {
        h: f64,
        len: f64,
        thick: f64,
        span: f64,
        hole_h: f64,
    },
    /// A free-standing span: a swept, tapered ribbon (points along the curve and radii).
    Span {
        curve: Vec<(DVec3, f64)>,
        r: f64,
    },
    Butte {
        h: f64,
        r: f64,
    },
    /// Joint blocks of granite, each rounded by weathering; `grain` sizes the lumps.
    Tor {
        blocks: Vec<TorBlock>,
        grain: f64,
    },
    /// A low mound of radius `r`, draped over the ground (heights on a grid spanning ±(r + 2)).
    Mound {
        h: f64,
        r: f64,
        ground: Vec<f64>,
    },
    Spire {
        h: f64,
        r: f64,
        lean: DVec2,
    },
}

/// What a shape needs to know about where it stands.
struct Ctx<'a> {
    field: &'a LandmarkField,
    /// Planet seed (the beds are the same all over the planet) and the formation's own.
    planet_seed: u64,
    seed: u64,
    /// Height above sea level of the part's origin, and its frame in terrain-local space.
    ground_asl: f64,
    frame_pos: DVec3,
    frame_rot: DQuat,
}

impl Ctx<'_> {
    /// Hardness (0..1) of the bed at point `p` of the part's frame.
    fn hardness(&self, p: DVec3) -> f64 {
        let w = self.frame_pos + self.frame_rot * p;
        // Beds warp gently over a few hundred meters.
        let y = self.ground_asl + p.y + 3.0 * noise3(w / 160.0, self.planet_seed ^ 0xBED5);
        let u = y / self.field.bed_m;
        let i = u.floor();
        let bed = |i: f64| {
            let r = cell_random(i as i64, 7, 3, self.planet_seed, 0xBED);
            // Sandstone or shale, little between.
            (r * 1.8 - 0.5).clamp(0.0, 1.0)
        };
        let t = smooth(0.82, 1.0, u - i);
        bed(i) + (bed(i + 1.0) - bed(i)) * t
    }

    fn noise(&self, p: DVec3, salt: u64) -> f64 {
        noise3(p, self.seed ^ salt)
    }
}

/// Polynomial smooth minimum and maximum (blend radius `k`).
fn smin(a: f64, b: f64, k: f64) -> f64 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

fn smax(a: f64, b: f64, k: f64) -> f64 {
    -smin(-a, -b, k)
}

/// Distance to a box of half extents `half` with edges rounded by `round`.
fn rounded_box(p: DVec3, half: DVec3, round: f64) -> f64 {
    let q = p.abs() - (half - DVec3::splat(round)).max(DVec3::ZERO);
    q.max(DVec3::ZERO).length() + q.max_element().min(0.0) - round
}

impl Part {
    /// Signed distance (approximate, never more than about the true distance) and stone.
    fn sdf(&self, p: DVec3, c: &Ctx) -> (f64, Stone) {
        let (d, stone) = match &self.shape {
            Shape::Hoodoo {
                h,
                r,
                cap_r,
                cap_t,
                squash,
            } => hoodoo(p, *h, *r, *cap_r, *cap_t, *squash, c),
            Shape::Window {
                h,
                len,
                thick,
                span,
                hole_h,
            } => window(p, *h, *len, *thick, *span, *hole_h, c),
            Shape::Span { curve, r } => span_arch(p, curve, *r, c),
            Shape::Butte { h, r } => butte(p, *h, *r, c),
            Shape::Tor { blocks, grain } => tor(p, blocks, *grain, c),
            Shape::Spire { h, r, lean } => spire(p, *h, *r, *lean, c),
            Shape::Mound { h, r, ground } => mound(p, *h, *r, ground, c),
        };
        (d.max(self.bottom - p.y), stone)
    }
}

fn hoodoo(p: DVec3, h: f64, r: f64, cap_r: f64, cap_t: f64, squash: f64, c: &Ctx) -> (f64, Stone) {
    let y = p.y;
    let t = (y / h).clamp(0.0, 1.0);
    let rq = DVec2::new(p.x, p.z * squash).length();
    let hard = c.hardness(p);
    // Waisted above the middle, thinner toward the top, ribbed by the beds.
    let waist = 1.0 - 0.3 * (-((t - 0.62) / 0.2).powi(2)).exp();
    let lump = 1.0
        + 0.2
            * c.noise(
                DVec3::new(p.x / (1.4 * r), p.y / (2.2 * r), p.z / (1.4 * r)),
                0x400D,
            )
        + 0.08 * c.noise(p / (0.5 * r), 0x400E);
    let r_y = r * (1.0 - 0.25 * t) * waist * (0.6 + 0.4 * hard) * lump;
    let column = ((rq - r_y) * 0.7).max(y - h);
    // The hard cap, a little wider than the top it shields.
    let yc = h + cap_t * 0.3;
    // Caps sit a little askew.
    let at = c.frame_pos / 5.0;
    let tilt = DVec2::new(c.noise(at, 0xCA1), c.noise(at, 0xCA2)) * 0.35;
    let cq = p - DVec3::new(0.0, yc + tilt.x * p.x + tilt.y * p.z, 0.0);
    let cap_lump = 1.0 + 0.2 * c.noise(cq / (cap_r * 0.8), 0xCA9);
    let cap = (DVec2::new(cq.x, cq.z * squash).length() - cap_r * cap_lump)
        .max(cq.y.abs() - cap_t * 0.5)
        * 0.7;
    // A talus cone at the foot.
    let talus_h = 0.16 * h;
    let r_t = r * 1.1 + (talus_h - y) * 1.5;
    let talus = ((rq - r_t) * 0.55 + 0.25 * r * c.noise(p / (0.8 * r), 0x7A1)).max(y - talus_h);
    let body = smin(column, talus, r * 0.35);
    let d = smin(body, cap, cap_t * 0.25);
    let stone = if cap < body {
        Stone::Cap
    } else if talus < column - 0.2 {
        Stone::Talus
    } else {
        Stone::Beds
    };
    (d, stone)
}

/// The low badland mound a field of hoodoos stands on: soft beds cut by gullies.
/// Samples per side of a mound's grid of ground heights.
const MOUND_GRID: usize = 13;

/// The ground under a mound at (x, z), from its grid (spanning ±`half` on both axes).
fn ground_grid(g: &[f64], half: f64, x: f64, z: f64) -> f64 {
    let n = MOUND_GRID - 1;
    let to = |v: f64| ((v / half + 1.0) * 0.5 * n as f64).clamp(0.0, n as f64 - 1e-9);
    let (u, v) = (to(x), to(z));
    let (i, j) = (u as usize, v as usize);
    let (fu, fv) = (u - i as f64, v - j as f64);
    let at = |i: usize, j: usize| g[j * MOUND_GRID + i];
    let top = at(i, j) + (at(i + 1, j) - at(i, j)) * fu;
    let bottom = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * fu;
    top + (bottom - top) * fv
}

fn mound(p: DVec3, h: f64, r: f64, ground: &[f64], c: &Ctx) -> (f64, Stone) {
    let flat = DVec3::new(p.x, 0.0, p.z);
    let rq = flat.length() * (1.0 + 0.15 * c.noise(flat / (r * 0.6), 0x30));
    let around = flat.normalize_or(DVec3::X) * (r / 7.0);
    let gully = (1.0 - c.noise(around + flat / (r * 1.5), 0x31).abs()).powi(4);
    let x = (rq / r).min(1.0);
    // Draped over the ground, so it never stands on a ledge of its own.
    let height = ground_grid(ground, r + 2.0, p.x, p.z)
        + h * (1.0 - x * x).powf(1.5) * (1.0 - 0.6 * gully)
        + 0.4 * c.noise(p / 3.0, 0x32)
        - 0.4
        // Its skirt sinks under the ground, finer than the grid, rather than ending in a lip.
        - 1.5 * x.powi(6)
        - (rq - r).max(0.0);
    ((p.y - height) * 0.6, Stone::Beds)
}

fn window(p: DVec3, h: f64, len: f64, thick: f64, span: f64, hole_h: f64, c: &Ctx) -> (f64, Stone) {
    let hard = c.hardness(p);
    // The fin: straight-sided below its middle, a rounded crest above.
    let (a, b) = (len * 0.5, h * 0.5);
    let ex = p.x.abs() / a;
    let ey = (p.y - b).max(0.0) / b;
    let profile = (ex.powf(3.0) + ey.powf(2.5)).powf(1.0 / 2.7);
    let lumps = 0.12 * c.noise(p / (thick * 1.5), 0xF1) + 0.04 * c.noise(p / (thick * 0.4), 0xF4);
    let d_xy = (profile - 1.0 - lumps) * a.min(b) * 0.6;
    let recess = (thick * 0.06).clamp(0.3, 1.0) * (1.0 - hard);
    let taper = 1.0 - 0.35 * (p.y / h).clamp(0.0, 1.0);
    let d_z =
        p.z.abs() - thick * 0.5 * taper * (1.0 + 0.25 * c.noise(p / (thick * 2.0), 0xF2)) + recess;
    let fin = smax(d_xy, d_z * 0.8, thick * 0.25);
    // The opening: worn through near the foot, its top rounded.
    let floor = 0.06 * h;
    let hc = floor + hole_h * 0.5;
    let hx = p.x / (span * 0.5);
    let hy = (p.y - hc) / (hole_h * 0.5);
    let wobble = 0.07 * c.noise(p / (span * 0.3), 0xF3);
    let d_hole = ((hx * hx + hy * hy).sqrt() - 1.0 - wobble) * (span * 0.5).min(hole_h * 0.5) * 0.6;
    // Below the opening's middle it is a straight-walled slot down to the ground.
    let d_hole = if p.y < hc {
        (hx.abs() - 1.0 - wobble) * span * 0.5 * 0.6
    } else {
        d_hole
    }
    .max(floor - p.y);
    let arch = smax(fin, -d_hole, thick * 0.15);
    // Talus along the foot of the fin.
    let talus_h = 0.08 * h;
    let qx = (p.x.abs() - a * 0.95).max(0.0);
    let qz = (p.z.abs() - thick * 0.5).max(0.0);
    let talus = (qx.hypot(qz) - (talus_h - p.y) * 1.5) * 0.55 + 0.4 * c.noise(p / 3.0, 0x7A2);
    let d = smin(arch, talus, 1.5);
    let stone = if talus < arch - 0.2 {
        Stone::Talus
    } else {
        Stone::Beds
    };
    (d, stone)
}

fn span_arch(p: DVec3, curve: &[(DVec3, f64)], r: f64, c: &Ctx) -> (f64, Stone) {
    // The ribbon is deeper (along Z) than it is thick.
    let q = DVec3::new(p.x, p.y, p.z / 1.6);
    let mut best = f64::MAX;
    for w in curve.windows(2) {
        let ((a, ra), (b, rb)) = (w[0], w[1]);
        let ab = b - a;
        let t = ((q - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        let d = (q - (a + ab * t)).length() - (ra + (rb - ra) * t);
        best = best.min(d);
    }
    let lumps = 0.18 * r * c.noise(p / (r * 1.2), 0x5A);
    let hard = c.hardness(p);
    let d = (best + lumps + 0.15 * r * (1.0 - hard)) * 0.6;
    (d, Stone::Beds)
}

fn butte(p: DVec3, h: f64, r: f64, c: &Ctx) -> (f64, Stone) {
    let rq = DVec2::new(p.x, p.z).length();
    let hard = c.hardness(p);
    // Lobes, spurs and bays around it (the same at every height: the cliffs stand straight),
    // and vertical joints fluting the walls.
    let flat = DVec3::new(p.x, 0.0, p.z);
    let lobes = 0.2 * c.noise(flat / (r * 0.9), 0xB1)
        + 0.08 * c.noise(flat / (r * 0.3), 0xB2)
        + 0.03 * c.noise(flat / (r * 0.1), 0xB6);
    let flutes = 0.9 * c.noise(DVec3::new(p.x / 6.0, p.y / 40.0, p.z / 6.0), 0xB3);
    let recess = (r * 0.045).clamp(1.0, 3.5) * (1.0 - hard);
    let r_y = r * (1.0 + lobes) - recess + flutes;
    let top = h + 0.8 * c.noise(flat / 30.0, 0xB4);
    let cliff = ((rq - r_y) * 0.8).max(p.y - top);
    // The caprock juts out a little over the cliff.
    let cap_t = (c.field.bed_m * 1.3).min(h * 0.2);
    let cap =
        (rq - (r * (1.0 + lobes) + 0.8)).max((p.y - (top - cap_t * 0.5)).abs() - cap_t * 0.5) * 0.8;
    // Talus apron up to about a third of its height at the angle of repose, cut by chutes
    // where rockfall runs down from notches in the cliff, and rubbly.
    let talus_h = 0.36 * h;
    let around = flat.normalize_or(DVec3::X) * (r / 9.0);
    let chute = (1.0
        - c.noise(around + DVec3::new(0.0, p.y / 60.0, 0.0), 0xB7)
            .abs())
    .powi(4);
    let r_t = r * (1.0 + lobes) * 0.97 + (talus_h - p.y) * 1.45 - chute * h * 0.07;
    let rubble = 0.8 * c.noise(p / 9.0, 0xB5) + 0.35 * c.noise(p / 2.5, 0xB8);
    let talus = ((rq - r_t) * 0.55 + rubble).max(p.y - talus_h * (1.0 - 0.3 * chute));
    let body = cliff.min(cap);
    let d = smin(body, talus, 3.0);
    let stone = if talus < body - 0.4 {
        Stone::Talus
    } else if cap <= cliff {
        Stone::Cap
    } else {
        Stone::Beds
    };
    (d, stone)
}

/// One block of a tor, between its joints.
#[derive(Debug, Clone, Copy)]
struct TorBlock {
    center: DVec3,
    half: DVec3,
    /// Turn about Y and how far its edges are worn round.
    yaw: f64,
    round: f64,
}

fn tor(p: DVec3, blocks: &[TorBlock], grain: f64, c: &Ctx) -> (f64, Stone) {
    let mut d = f64::MAX;
    for b in blocks {
        let q = p - b.center;
        // A block's rounded box lies inside its bounding sphere.
        if q.length() - b.half.length() > d {
            continue;
        }
        let q = DQuat::from_rotation_y(-b.yaw) * q;
        d = d.min(rounded_box(q, b.half, b.round));
    }
    // Granular, pitted surfaces.
    let lumps =
        grain * (0.1 * c.noise(p / (grain * 1.6), 0x70) + 0.035 * c.noise(p / (grain * 0.4), 0x71));
    ((d + lumps) * 0.85, Stone::Granite)
}

fn spire(p: DVec3, h: f64, r: f64, lean: DVec2, c: &Ctx) -> (f64, Stone) {
    let t = (p.y / h).clamp(0.0, 1.0);
    // Blades, all turned the same way along the sun's daily path (as penitentes are), their
    // crests wandering a little and their flanks fluted.
    let wander = DVec2::new(c.noise(p / (h * 0.5), 0x1C1), c.noise(p / (h * 0.5), 0x1C2)) * r * 0.3;
    let q = DVec2::new(p.x - lean.x * p.y, p.z - lean.y * p.y) - wander * t;
    let q = DVec2::new(q.x, q.y * 1.8);
    let angle = q.y.atan2(q.x);
    let flutes = 1.0 + 0.2 * (angle * 7.0 + 3.0 * c.noise(p / (r * 2.0), 0x1CE)).sin();
    let r_y = r * (1.0 - t).powf(0.8) * flutes + 0.12;
    let d = ((q.length() - r_y) * 0.45).max(p.y - h);
    (d, Stone::Ice)
}

/// The parts of a formation, in its frame. `ground(x, z)` is the ground's height under a point
/// of its frame, relative to its center.
fn parts_of(site: &Landmark, ground: &dyn Fn(f64, f64) -> f64) -> Vec<Part> {
    let rand = |j: u64| cell_random(site.seed as i64, 0, 0, 0x1A4D, j);
    let (h, w) = (site.height_m, site.width_m);
    // The lowest ground under a footprint of radius `r` around (x, z), relative to (x, z).
    let low_under = |x: f64, z: f64, r: f64| {
        let g0 = ground(x, z);
        (0..8)
            .map(|k| {
                let a = k as f64 / 8.0 * TAU;
                ground(x + a.cos() * r, z + a.sin() * r) - g0
            })
            .fold(0.0f64, f64::min)
    };
    let part = |at: DVec3, shape: Shape, half_xz: f64, top: f64, bottom: f64, cells: f64| {
        let margin = (half_xz.max(top - bottom) / cells) * 2.0;
        let voxel = ((2.0 * half_xz).max(top - bottom) / cells).max(0.1);
        Part {
            at,
            shape,
            min: DVec3::new(-half_xz - margin, bottom - margin, -half_xz - margin),
            max: DVec3::new(half_xz + margin, top + margin, half_xz + margin),
            voxel,
            bottom,
        }
    };
    match site.kind {
        LandmarkKind::Arch => {
            let bottom = low_under(0.0, 0.0, w * 0.5) - 1.5;
            if rand(1) < 0.65 {
                let span = w * (0.5 + 0.12 * rand(2));
                let thick = (h * (0.22 + 0.12 * rand(3))).clamp(4.0, 11.0);
                let hole_h = h * (0.55 + 0.15 * rand(4));
                // Out to the end of the talus along the fin.
                let half = (w * 0.5 + (0.08 * h - bottom) * 1.5 + 2.0).max(thick * 2.0);
                vec![part(
                    DVec3::ZERO,
                    Shape::Window {
                        h,
                        len: w,
                        thick,
                        span,
                        hole_h,
                    },
                    half,
                    h * 1.1,
                    bottom,
                    110.0,
                )]
            } else {
                // A free-standing span from foot to foot.
                let s = w * 0.55;
                let r = (h * (0.1 + 0.05 * rand(5))).max(1.2);
                let (p0, p3) = (
                    DVec3::new(-s * 0.5, bottom, 0.0),
                    DVec3::new(s * 0.5, bottom, 0.0),
                );
                let rise = (h - r) * 4.0 / 3.0 - bottom / 3.0;
                let (p1, p2) = (
                    DVec3::new(-s * 0.32, rise, 0.0),
                    DVec3::new(s * 0.32, rise, 0.0),
                );
                let curve = (0..=32)
                    .map(|i| {
                        let t = i as f64 / 32.0;
                        let u = 1.0 - t;
                        let pt = p0 * (u * u * u)
                            + p1 * (3.0 * u * u * t)
                            + p2 * (3.0 * u * t * t)
                            + p3 * (t * t * t);
                        // Stout legs and a thinner crown.
                        (pt, r * (1.3 - 0.45 * (PI * t).sin()))
                    })
                    .collect();
                vec![part(
                    DVec3::ZERO,
                    Shape::Span { curve, r },
                    s * 0.5 + r * 2.5,
                    h + r,
                    bottom,
                    100.0,
                )]
            }
        }
        LandmarkKind::Hoodoos => {
            let count = 8 + (rand(1) * 14.0) as u64;
            let mut placed: Vec<(f64, f64, f64)> = Vec::new();
            // They stand on the mound of soft rock they were cut from.
            let mound_h = 0.2 * h;
            let mound_bottom = low_under(0.0, 0.0, w * 0.5) - 1.0;
            let grid: Vec<f64> = {
                let half = w * 0.5 + 2.0;
                let n = MOUND_GRID - 1;
                let at = |k: usize| (k as f64 / n as f64 * 2.0 - 1.0) * half;
                (0..MOUND_GRID * MOUND_GRID)
                    .map(|k| ground(at(k % MOUND_GRID), at(k / MOUND_GRID)))
                    .collect()
            };
            let high = grid.iter().copied().fold(0.0f64, f64::max);
            let low = grid.iter().copied().fold(0.0f64, f64::min);
            let mut parts = vec![part(
                DVec3::ZERO,
                Shape::Mound {
                    h: mound_h,
                    r: w * 0.5,
                    ground: grid,
                },
                w * 0.5 + 2.0,
                high + mound_h + 2.0,
                mound_bottom.min(low - 1.5),
                70.0,
            )];
            for i in 0..count * 4 {
                if placed.len() as u64 >= count {
                    break;
                }
                let j = 10 + i * 8;
                let hh = h * (0.4 + 0.6 * rand(j));
                let r = hh * (0.15 + 0.09 * rand(j + 1));
                let (a, d) = (
                    rand(j + 2) * TAU,
                    (rand(j + 3)).sqrt() * (w * 0.42 - r * 2.0),
                );
                let (x, z) = (a.cos() * d, a.sin() * d);
                if placed
                    .iter()
                    .any(|&(px, pz, pr)| (px - x).hypot(pz - z) < (pr + r) * 1.45)
                {
                    continue;
                }
                placed.push((x, z, r));
                let cap_r = r * (0.7 + 0.3 * rand(j + 4));
                let cap_t = r * (0.45 + 0.3 * rand(j + 5));
                let talus_r = r * 1.1 + 0.16 * hh * 1.5;
                let bottom = low_under(x, z, talus_r) - 1.0;
                let half = talus_r + (-bottom) * 1.5 + r * 0.5;
                parts.push(part(
                    DVec3::new(x, ground(x, z), z),
                    Shape::Hoodoo {
                        h: hh,
                        r,
                        cap_r,
                        cap_t,
                        squash: 1.0 + 0.6 * rand(j + 6),
                    },
                    half,
                    hh + cap_t,
                    bottom,
                    56.0,
                ));
            }
            parts
        }
        LandmarkKind::Butte => {
            let r = w * 0.5;
            let half = r * 1.35 + 0.36 * h * 1.45 + 4.0;
            let bottom = low_under(0.0, 0.0, half) - 2.0;
            vec![part(
                DVec3::ZERO,
                Shape::Butte { h, r },
                half + (-bottom) * 1.45,
                h + 2.0,
                bottom,
                120.0,
            )]
        }
        LandmarkKind::Tor => {
            // Granite cut by two sets of upright joints and by sheeting joints parallel to
            // the ground, rotted along the joints and stripped down to its sound blocks:
            // stacks of rounded slabs, thinner toward the top, along a crest (Dartmoor
            // National Park, "Tors"; Linton 1955). Frost pried loose blocks downslope as
            // clitter.
            let jx = (w * (0.15 + 0.06 * rand(2))).clamp(1.4, 4.5);
            let jz = jx * (0.8 + 0.4 * rand(3));
            let (phase_x, phase_z) = (rand(4), rand(5));
            let grain = jx * 0.25;
            let stacks = 1 + (rand(1) * 3.0) as u64;
            let mut blocks = Vec::new();
            for s in 0..stacks {
                let j = 20 + s * 60;
                // The tallest in the middle, lower ones along the crest to either side.
                let side = if s % 2 == 1 { 1.0 } else { -1.0 };
                let (ox, oz) = if s == 0 {
                    (0.0, 0.0)
                } else {
                    (
                        side * w * (0.26 + 0.1 * rand(j)),
                        (rand(j + 1) - 0.5) * w * 0.15,
                    )
                };
                let sh = h * if s == 0 {
                    1.0
                } else {
                    0.4 + 0.35 * rand(j + 2)
                };
                let rx = w * if s == 0 {
                    0.22
                } else {
                    0.13 + 0.05 * rand(j + 3)
                };
                let rz = rx * (0.7 + 0.3 * rand(j + 4));
                let g0 = ground(ox, oz);
                let top = g0 + sh;
                let mut y = g0 + low_under(ox, oz, rx) - 0.6;
                for k in 0..16u64 {
                    if y > top - 0.35 {
                        break;
                    }
                    let jj = j + 6 + k * 9;
                    let frac = ((y - g0) / sh).clamp(0.0, 1.0);
                    // Sheets thin upward, as sheeting joints crowd toward the surface.
                    let t = ((h / 5.0) * (1.4 - 0.7 * frac) * (0.7 + 0.6 * rand(jj)))
                        .clamp(0.5, 3.0)
                        .min(top - y);
                    let yc = y + t * 0.5;
                    let fy = ((yc - g0) / sh).clamp(0.0, 1.0);
                    // A dome: the sheets narrow toward the top.
                    let e = (1.0 - fy.powf(2.5)).max(0.0).sqrt() * (0.85 + 0.3 * rand(jj + 1));
                    // Each sheet slid a little on the one below.
                    let slide = DVec2::new(rand(jj + 2) - 0.5, rand(jj + 3) - 0.5) * jx * 0.12;
                    let yaw = (rand(jj + 4) - 0.5) * 0.06;
                    let (nx, nz) = ((rx / jx).ceil() as i64 + 1, (rz / jz).ceil() as i64 + 1);
                    let before = blocks.len();
                    for iz in -nz..=nz {
                        for ix in -nx..=nx {
                            let cx = (ix as f64 + phase_x - 0.5) * jx;
                            let cz = (iz as f64 + phase_z - 0.5) * jz;
                            let r2 = (cx / (rx * e).max(0.01)).powi(2)
                                + (cz / (rz * e).max(0.01)).powi(2);
                            if r2 > 1.0 {
                                continue;
                            }
                            let rb = rand(jj + 5 + ((ix + 9) * 31 + iz + 9) as u64 * 3);
                            // Blocks near the top weathered away here and there.
                            if fy > 0.55 && rb < 0.18 {
                                continue;
                            }
                            let outer = r2.sqrt();
                            let half = DVec3::new(
                                jx * 0.5 - 0.04 * jx - 0.03,
                                t * 0.5 - 0.03,
                                jz * 0.5 - 0.04 * jz - 0.03,
                            );
                            // Worn roundest at the top and the outside; lower down the
                            // slabs keep fairly square edges.
                            let round = half.min_element()
                                * (0.2 + 0.35 * fy + 0.2 * outer + 0.15 * rb).min(0.75);
                            blocks.push(TorBlock {
                                center: DVec3::new(ox + cx + slide.x, yc, oz + cz + slide.y),
                                half,
                                yaw,
                                round,
                            });
                        }
                    }
                    // A sheet too narrow for the joints: one slab.
                    if blocks.len() == before && e > 0.2 {
                        let half = DVec3::new(
                            (rx * e).max(jx * 0.4),
                            t * 0.5 - 0.03,
                            (rz * e).max(jz * 0.4),
                        );
                        blocks.push(TorBlock {
                            center: DVec3::new(ox + slide.x, yc, oz + slide.y),
                            half,
                            yaw,
                            round: half.min_element() * 0.8,
                        });
                    }
                    y += t;
                }
                // Now and then a rounded boulder perched on top (a logan stone).
                if s == 0 && rand(j + 5) < 0.35 {
                    let r = jx * (0.35 + 0.15 * rand(j + 6));
                    let half = DVec3::new(r * 1.1, r * 0.7, r);
                    blocks.push(TorBlock {
                        center: DVec3::new(ox, y + half.y * 0.9, oz),
                        half,
                        yaw: rand(j + 7) * TAU,
                        round: half.min_element() * 0.9,
                    });
                }
            }
            let reach = blocks
                .iter()
                .map(|b| DVec2::new(b.center.x, b.center.z).length() + b.half.length())
                .fold(1.0f64, f64::max);
            let roof = blocks
                .iter()
                .map(|b| b.center.y + b.half.y)
                .fold(1.0f64, f64::max);
            let bottom = low_under(0.0, 0.0, reach) - 1.5;
            let mut parts = vec![part(
                DVec3::ZERO,
                Shape::Tor { blocks, grain },
                reach,
                roof + 0.5,
                bottom,
                110.0,
            )];
            // Clitter: blocks frost carried off the tor, half sunk in the ground around it.
            let count = 6 + (rand(8) * 14.0) as u64;
            for i in 0..count {
                let j = 400 + i * 7;
                let a = rand(j) * TAU;
                let d = reach * (0.75 + 0.9 * rand(j + 1));
                let (x, z) = (a.cos() * d, a.sin() * d);
                let r = jx * (0.18 + 0.32 * rand(j + 2));
                let half = DVec3::new(r * 1.2, r * (0.55 + 0.3 * rand(j + 3)), r);
                let bottom = low_under(x, z, r * 1.2) - 0.3;
                parts.push(part(
                    DVec3::new(x, ground(x, z), z),
                    Shape::Tor {
                        blocks: vec![TorBlock {
                            center: DVec3::new(0.0, half.y * 0.45, 0.0),
                            half,
                            yaw: rand(j + 4) * TAU,
                            round: half.min_element() * 0.75,
                        }],
                        grain,
                    },
                    r * 1.6,
                    half.y * 1.5,
                    bottom,
                    20.0,
                ));
            }
            parts
        }
        LandmarkKind::IceSpires => {
            let count = 10 + (rand(1) * 20.0) as u64;
            let mut placed: Vec<(f64, f64, f64)> = Vec::new();
            let mut parts = Vec::new();
            // They lean together, toward the low sun that carved them.
            let lean_a = rand(2) * TAU;
            for i in 0..count * 4 {
                if placed.len() as u64 >= count {
                    break;
                }
                let j = 10 + i * 6;
                let hh = h * (0.35 + 0.65 * rand(j));
                let r = hh * (0.16 + 0.08 * rand(j + 1));
                let (a, d) = (rand(j + 2) * TAU, rand(j + 3).sqrt() * (w * 0.5 - r));
                let (x, z) = (a.cos() * d, a.sin() * d);
                if placed
                    .iter()
                    .any(|&(px, pz, pr)| (px - x).hypot(pz - z) < (pr + r) * 0.9)
                {
                    continue;
                }
                placed.push((x, z, r));
                let lean = DVec2::new(lean_a.cos(), lean_a.sin()) * (0.08 + 0.1 * rand(j + 4));
                let bottom = low_under(x, z, r) - 0.8;
                let half = r * 1.3 + hh * lean.length() + (-bottom) * 0.5;
                parts.push(part(
                    DVec3::new(x, ground(x, z), z),
                    Shape::Spire { h: hh, r, lean },
                    half,
                    hh,
                    bottom,
                    50.0,
                ));
            }
            parts
        }
    }
}

// --- Meshing -------------------------------------------------------------------------------

/// Naive surface nets over the box `min..max` at spacing `h`: one vertex per cell the surface
/// crosses (the mean of its edge crossings), a quad per crossed grid edge. Space far from the
/// surface is skipped a block at a time (the distance at a block's center says no surface can
/// be inside). Triangles wind counter-clockwise seen from outside. `None` past `max_vertices`.
fn surface_nets(
    f: &dyn Fn(DVec3) -> f64,
    min: DVec3,
    max: DVec3,
    h: f64,
    max_vertices: usize,
) -> Option<(Vec<DVec3>, Vec<[u32; 3]>)> {
    let n = ((max - min) / h).ceil();
    let (nx, ny, nz) = (
        (n.x as usize + 1).max(2),
        (n.y as usize + 1).max(2),
        (n.z as usize + 1).max(2),
    );
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let at = |x: usize, y: usize, z: usize| min + DVec3::new(x as f64, y as f64, z as f64) * h;
    let mut v = vec![0f32; nx * ny * nz];
    let mut exact = vec![false; nx * ny * nz];
    let mut filled = vec![false; nx * ny * nz];
    const B: usize = 4;
    // Half the block's diagonal, with room for distances that are a little too long.
    let reach = 3f64.sqrt() * B as f64 * 0.5 * h * 1.5 + h;
    for bz in (0..nz - 1).step_by(B) {
        for by in (0..ny - 1).step_by(B) {
            for bx in (0..nx - 1).step_by(B) {
                let (x1, y1, z1) = (
                    (bx + B).min(nx - 1),
                    (by + B).min(ny - 1),
                    (bz + B).min(nz - 1),
                );
                let c = min
                    + DVec3::new(
                        (bx + x1) as f64 * 0.5,
                        (by + y1) as f64 * 0.5,
                        (bz + z1) as f64 * 0.5,
                    ) * h;
                let dc = f(c);
                let far = dc.abs() > reach;
                for z in bz..=z1 {
                    for y in by..=y1 {
                        for x in bx..=x1 {
                            let i = idx(x, y, z);
                            if far {
                                if !filled[i] {
                                    v[i] = dc as f32;
                                    filled[i] = true;
                                }
                            } else if !exact[i] {
                                v[i] = f(at(x, y, z)) as f32;
                                exact[i] = true;
                                filled[i] = true;
                            }
                        }
                    }
                }
            }
        }
    }
    // A vertex in every cell the surface crosses.
    let (cx, cy) = (nx - 1, ny - 1);
    let cidx = |x: usize, y: usize, z: usize| (z * cy + y) * cx + x;
    let mut cell_vertex = vec![u32::MAX; cx * cy * (nz - 1)];
    let corner =
        |k: usize| DVec3::new((k & 1) as f64, ((k >> 1) & 1) as f64, ((k >> 2) & 1) as f64);
    let edges: Vec<(usize, usize)> = (0..8)
        .flat_map(|a| [1, 2, 4].into_iter().map(move |bit| (a, a | bit)))
        .filter(|&(a, b)| a != b)
        .collect();
    let mut positions = Vec::new();
    for z in 0..nz - 1 {
        for y in 0..ny - 1 {
            for x in 0..nx - 1 {
                let mut d = [0f32; 8];
                let mut mask = 0u8;
                for (k, dk) in d.iter_mut().enumerate() {
                    *dk = v[idx(x + (k & 1), y + ((k >> 1) & 1), z + ((k >> 2) & 1))];
                    if *dk < 0.0 {
                        mask |= 1 << k;
                    }
                }
                if mask == 0 || mask == 255 {
                    continue;
                }
                let (mut sum, mut count) = (DVec3::ZERO, 0.0);
                for &(a, b) in &edges {
                    let (da, db) = (d[a], d[b]);
                    if (da < 0.0) != (db < 0.0) {
                        let t = (da / (da - db)) as f64;
                        sum += corner(a) + (corner(b) - corner(a)) * t;
                        count += 1.0;
                    }
                }
                if positions.len() >= max_vertices {
                    return None;
                }
                cell_vertex[cidx(x, y, z)] = positions.len() as u32;
                positions.push(min + (DVec3::new(x as f64, y as f64, z as f64) + sum / count) * h);
            }
        }
    }
    // A quad across every crossed grid edge, joining the four cells around it.
    let mut tris = Vec::new();
    let mut quad = |cells: [u32; 4], outward: DVec3, positions: &[DVec3]| {
        if cells.contains(&u32::MAX) {
            return;
        }
        let p = |i: u32| positions[i as usize];
        let [a, b, c, d] = cells;
        let pair = if (p(a) - p(c)).length_squared() <= (p(b) - p(d)).length_squared() {
            [[a, b, c], [a, c, d]]
        } else {
            [[a, b, d], [b, c, d]]
        };
        for mut t in pair {
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            if n.dot(outward) < 0.0 {
                t.swap(1, 2);
            }
            tris.push(t);
        }
    };
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let inside = v[idx(x, y, z)] < 0.0;
                let sign = if inside { 1.0 } else { -1.0 };
                // Along X: the four cells sharing the edge differ in y and z.
                if x + 1 < nx
                    && y >= 1
                    && z >= 1
                    && y < cy
                    && z < nz - 1
                    && inside != (v[idx(x + 1, y, z)] < 0.0)
                {
                    quad(
                        [
                            cell_vertex[cidx(x, y - 1, z - 1)],
                            cell_vertex[cidx(x, y, z - 1)],
                            cell_vertex[cidx(x, y, z)],
                            cell_vertex[cidx(x, y - 1, z)],
                        ],
                        DVec3::X * sign,
                        &positions,
                    );
                }
                if y + 1 < ny
                    && x >= 1
                    && z >= 1
                    && x < cx
                    && z < nz - 1
                    && inside != (v[idx(x, y + 1, z)] < 0.0)
                {
                    quad(
                        [
                            cell_vertex[cidx(x - 1, y, z - 1)],
                            cell_vertex[cidx(x, y, z - 1)],
                            cell_vertex[cidx(x, y, z)],
                            cell_vertex[cidx(x - 1, y, z)],
                        ],
                        DVec3::Y * sign,
                        &positions,
                    );
                }
                if z + 1 < nz
                    && x >= 1
                    && y >= 1
                    && x < cx
                    && y < cy
                    && inside != (v[idx(x, y, z + 1)] < 0.0)
                {
                    quad(
                        [
                            cell_vertex[cidx(x - 1, y - 1, z)],
                            cell_vertex[cidx(x, y - 1, z)],
                            cell_vertex[cidx(x, y, z)],
                            cell_vertex[cidx(x - 1, y, z)],
                        ],
                        DVec3::Z * sign,
                        &positions,
                    );
                }
            }
        }
    }
    Some((positions, tris))
}

/// Color of a point of a formation's surface (linear), `up` how much its normal faces up.
fn paint(stone: Stone, hard: f64, up: f64, w: DVec3, field: &LandmarkField, seed: u64) -> [f32; 3] {
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let scale = |c: [f32; 3], k: f32| [c[0] * k, c[1] * k, c[2] * k];
    let speck = 1.0 + 0.12 * noise3(w / 2.5, seed ^ 0x5EC) as f32;
    match stone {
        Stone::Beds | Stone::Cap => {
            let base = if stone == Stone::Cap {
                field.cap
            } else {
                mix(field.soft, field.hard, hard as f32)
            };
            // Desert varnish: dark streaks running down the steep faces.
            let streak = noise3(DVec3::new(w.x / 3.0, w.y / 45.0, w.z / 3.0), seed ^ 0xDA7) as f32;
            let steep = (1.0 - up.abs() as f32).clamp(0.0, 1.0);
            let varnish = 1.0 - 0.45 * steep * ((streak - 0.15) / 0.4).clamp(0.0, 1.0);
            scale(base, speck * varnish)
        }
        Stone::Talus => scale(mix(field.soft, field.hard, 0.3), 0.9 * speck),
        Stone::Granite => {
            // Coarse crystals, grey-green lichen on the tops, dark weathering streaks down
            // the sides.
            let crystals = 1.0 + 0.1 * noise3(w / 0.4, seed ^ 0x6A1) as f32;
            let lichen = ((noise3(w / 1.7, seed ^ 0x11C) as f32 - 0.05) / 0.3).clamp(0.0, 1.0)
                * (0.3 + 0.7 * up.max(0.0) as f32);
            let streak = noise3(DVec3::new(w.x / 1.2, w.y / 12.0, w.z / 1.2), seed ^ 0x57) as f32;
            let steep = (1.0 - up.abs() as f32).clamp(0.0, 1.0);
            let wet = 1.0 - 0.3 * steep * ((streak - 0.2) / 0.4).clamp(0.0, 1.0);
            let stone = scale(field.granite, speck * crystals * wet);
            mix(stone, [0.36, 0.38, 0.3], lichen * 0.55)
        }
        Stone::Ice => {
            // Bluer in the hollows, whiter where the sun and frost reach.
            let frost = (0.5 + 0.5 * up as f32).clamp(0.0, 1.0);
            mix(scale(field.ice, 0.8), [0.85, 0.88, 0.92], frost * 0.6)
        }
    }
}

/// A built cell: its formations, in the cell's tangent frame.
struct CellMesh {
    /// Terrain-local origin (on the ground at the cell's center) and rotation (+Y up).
    origin: DVec3,
    rotation: DQuat,
    rock: Option<Mesh>,
    ice: Option<Mesh>,
    /// Every triangle of both, for the collider.
    collider: Option<ColliderShape>,
}

/// Rock and ice vertices and triangle indices of a cell.
type Buffers = (Vec<Vertex>, Vec<u32>);

/// The ground's terrain-local point and up under terrain-local point `local`.
fn ground_at(source: &ChunkSource, local: DVec3) -> Option<(DVec3, DVec3)> {
    let (dir, up, base) = source.map.locate(local)?;
    Some((base + up * source.sample(base, up, dir, 0.0).height, up))
}

/// The ground's point and up at the middle of a quadtree cell, and the rotation from +Y to
/// that up: the frame a cell's meshes are built in.
fn cell_frame(source: &ChunkSource, key: NodeKey) -> (DVec3, DQuat) {
    let (lo, s) = key.face_rect();
    let center = source
        .map
        .position(key.face, lo.x + s * 0.5, lo.y + s * 0.5, 0.0);
    let (origin, up) = ground_at(source, center).unwrap_or((center, center.normalize()));
    (origin, DQuat::from_rotation_arc(DVec3::Y, up))
}

/// Mesh `sites` into rock and ice buffers in the frame (`origin`, `rotation`), with voxels
/// `coarse` times their full size.
fn mesh_sites(
    source: &ChunkSource,
    field: &LandmarkField,
    seed: u64,
    sites: &[Landmark],
    (origin, rotation): (DVec3, DQuat),
    coarse: f64,
) -> (Buffers, Buffers) {
    let inv = rotation.inverse();
    let mut rock: Buffers = (Vec::new(), Vec::new());
    let mut ice: Buffers = (Vec::new(), Vec::new());
    for site in sites {
        let site_rot = site.rotation();
        // Ground under a point of the formation's frame, relative to its center.
        let ground = |x: f64, z: f64| -> f64 {
            let q = site.position + site_rot * DVec3::new(x, 0.0, z);
            ground_at(source, q)
                .map(|(g, _)| (g - site.position).dot(site.up))
                .unwrap_or(0.0)
        };
        for mut part in parts_of(site, &ground) {
            if coarse > 1.0 {
                part.voxel *= coarse;
                part.min -= DVec3::splat(part.voxel * 2.0);
                part.max += DVec3::splat(part.voxel * 2.0);
            }
            let ctx = Ctx {
                field,
                planet_seed: seed,
                seed: site.seed,
                ground_asl: site.ground_asl + part.at.y,
                frame_pos: site.position + site_rot * part.at,
                frame_rot: site_rot,
            };
            let f = |p: DVec3| part.sdf(p, &ctx).0;
            let Some((positions, tris)) =
                surface_nets(&f, part.min, part.max, part.voxel, MAX_PART_VERTICES)
            else {
                continue;
            };
            let to_cell = inv * site_rot;
            let e = part.voxel * 0.5;
            let mut verts = Vec::with_capacity(positions.len());
            let mut is_ice = false;
            for &p in &positions {
                let grad = DVec3::new(
                    f(p + DVec3::X * e) - f(p - DVec3::X * e),
                    f(p + DVec3::Y * e) - f(p - DVec3::Y * e),
                    f(p + DVec3::Z * e) - f(p - DVec3::Z * e),
                )
                .normalize_or(DVec3::Y);
                let (_, stone) = part.sdf(p, &ctx);
                is_ice = stone == Stone::Ice;
                let w = ctx.frame_pos + site_rot * p;
                let color = paint(stone, ctx.hardness(p), grad.y, w, field, site.seed);
                let local = site.position + site_rot * (part.at + p);
                let at = (inv * (local - origin)).as_vec3();
                let n = (to_cell * grad).as_vec3();
                verts.push(
                    Vertex::new(at, n, Vec2::new(at.x * 0.25, at.z * 0.25))
                        .with_color(Vec4::new(color[0], color[1], color[2], 1.0)),
                );
            }
            let target = if is_ice { &mut ice } else { &mut rock };
            if target.0.len() + verts.len() > MAX_CELL_VERTICES {
                continue;
            }
            let start = target.0.len() as u32;
            target.0.extend(verts);
            target.1.extend(tris.iter().flatten().map(|i| start + i));
        }
    }
    (rock, ice)
}

fn to_mesh((v, i): Buffers) -> Option<Mesh> {
    (!v.is_empty()).then(|| Mesh::new(v, i))
}

/// Mesh every formation of one cell.
fn build_cell(source: &ChunkSource, field: &LandmarkField, key: NodeKey, seed: u64) -> CellMesh {
    let (origin, rotation) = cell_frame(source, key);
    let sites = sites_in(source, field, key, seed);
    let (rock, ice) = mesh_sites(source, field, seed, &sites, (origin, rotation), 1.0);
    let collider = (!rock.0.is_empty() || !ice.0.is_empty()).then(|| {
        let mut vertices: Vec<Vec3> = Vec::with_capacity(rock.0.len() + ice.0.len());
        let mut indices: Vec<[u32; 3]> = Vec::new();
        for (vs, is) in [&rock, &ice] {
            let start = vertices.len() as u32;
            vertices.extend(vs.iter().map(|v| Vec3::from(v.position)));
            indices.extend(
                is.chunks(3)
                    .map(|t| [start + t[0], start + t[1], start + t[2]]),
            );
        }
        ColliderShape::TriMesh { vertices, indices }
    });
    CellMesh {
        origin,
        rotation,
        rock: to_mesh(rock),
        ice: to_mesh(ice),
        collider,
    }
}

/// Formations at least this tall are drawn from afar.
const FAR_MIN_HEIGHT_M: f64 = 18.0;
/// Far cells are this many quadtree levels above the near ones (4 km across), drawn out to
/// [`LandmarkField::far_m`] with voxels [`FAR_COARSE`] times as big.
const FAR_LEVELS_UP: u8 = 2;
const FAR_COARSE: f64 = 4.0;
const MAX_FAR_BUILDING: usize = 1;

/// A far cell's big formations, coarsely meshed, grouped by the near cell each stands in (a
/// group hides while its near cell is drawn).
struct FarMesh {
    origin: DVec3,
    rotation: DQuat,
    groups: Vec<(NodeKey, Option<Mesh>, Option<Mesh>)>,
}

fn build_far_cell(source: &ChunkSource, field: &LandmarkField, key: NodeKey, seed: u64) -> FarMesh {
    let (origin, rotation) = cell_frame(source, key);
    let mut near = vec![key];
    for _ in 0..FAR_LEVELS_UP {
        near = near.iter().flat_map(|k| k.children()).collect();
    }
    let groups = near
        .into_iter()
        .filter_map(|k| {
            let big: Vec<Landmark> = sites_in(source, field, k, seed)
                .into_iter()
                .filter(|s| s.height_m >= FAR_MIN_HEIGHT_M)
                .collect();
            if big.is_empty() {
                return None;
            }
            let (rock, ice) = mesh_sites(source, field, seed, &big, (origin, rotation), FAR_COARSE);
            Some((k, to_mesh(rock), to_mesh(ice)))
        })
        .collect();
    FarMesh {
        origin,
        rotation,
        groups,
    }
}

// --- Streaming -----------------------------------------------------------------------------

/// One cell: terrain and quadtree node.
type CellKey = (Entity, NodeKey);

struct Cell {
    /// Drawn rock and ice, and the collider while a focus is near.
    entities: Vec<Entity>,
    meshes: Vec<Handle<Mesh>>,
    collider: Option<ColliderShape>,
    collider_entity: Option<Entity>,
    origin: DVec3,
    rotation: DQuat,
}

/// A far cell: one entity per group of formations, with the near cell it stands in and
/// whether it is shown.
struct FarCell {
    groups: Vec<FarGroup>,
    origin: DVec3,
    rotation: DQuat,
}

struct FarGroup {
    near: NodeKey,
    entities: Vec<Entity>,
    meshes: Vec<Handle<Mesh>>,
    shown: bool,
}

/// The landmark cells in the world.
#[derive(Resource, Default)]
pub struct LandmarkCells {
    cells: HashMap<CellKey, Cell>,
    building: HashMap<CellKey, JoinHandle<CellMesh>>,
    far: HashMap<CellKey, FarCell>,
    far_building: HashMap<CellKey, JoinHandle<FarMesh>>,
    terrains: HashMap<Entity, TerrainLandmarks>,
    /// Cells put in place since startup.
    pub placed: u64,
}

struct TerrainLandmarks {
    rock: Handle<StandardMaterial>,
    ice: Handle<StandardMaterial>,
    version: u64,
    pose: (DVec3, DQuat),
}

impl LandmarkCells {
    /// Cells with formations drawn.
    pub fn drawn(&self) -> usize {
        self.cells
            .values()
            .filter(|c| !c.entities.is_empty())
            .count()
    }

    /// Groups of far formations shown (their near cells not drawn yet).
    pub fn far_shown(&self) -> usize {
        self.far
            .values()
            .flat_map(|c| &c.groups)
            .filter(|g| g.shown)
            .count()
    }

    /// Nothing waiting to be built.
    pub fn is_settled(&self) -> bool {
        self.building.is_empty() && self.far_building.is_empty()
    }
}

fn rock_material(field: &LandmarkField) -> StandardMaterial {
    match field.texture {
        // The preset's albedo is about mid grey: the vertex colors carry the rock's own.
        Some(texture) => StandardMaterial {
            base_color: [2.2, 2.2, 2.2, 1.0],
            perceptual_roughness: 0.9,
            ..StandardMaterial::preset(texture).with_triplanar()
        },
        None => StandardMaterial {
            base_color: [1.0, 1.0, 1.0, 1.0],
            perceptual_roughness: 0.92,
            ..Default::default()
        },
    }
}

fn ice_material() -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0, 1.0, 1.0, 1.0],
        perceptual_roughness: 0.35,
        ..Default::default()
    }
}

fn place(pose: (DVec3, DQuat), origin: DVec3, rotation: DQuat) -> Transform {
    let (position, rot) = pose;
    Transform {
        translation: position + rot * origin,
        rotation: (rot * rotation).normalize().as_quat(),
        ..Default::default()
    }
}

fn drop_far(commands: &mut Commands, meshes: &mut Assets<Mesh>, cell: Option<FarCell>) {
    for g in cell.into_iter().flat_map(|c| c.groups) {
        for e in g.entities {
            commands.entity(e).despawn();
        }
        for h in g.meshes {
            meshes.remove(h);
        }
    }
}

fn drop_cell(commands: &mut Commands, meshes: &mut Assets<Mesh>, cell: Option<Cell>) {
    let Some(cell) = cell else {
        return;
    };
    for e in cell.entities.into_iter().chain(cell.collider_entity) {
        commands.entity(e).despawn();
    }
    for h in cell.meshes {
        meshes.remove(h);
    }
}

/// Keep the formations around every focus point near a terrain with a [`LandmarkField`]
/// drawn, and colliding near the focus.
#[allow(clippy::too_many_arguments)]
pub fn update_landmarks(
    mut commands: Commands,
    terrains: Res<Terrains>,
    focus: Res<FocusPoints>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    surfaces: Option<Res<SurfaceMaterials>>,
    mut state: ResMut<LandmarkCells>,
    mut renderers: Query<&mut MeshRenderer>,
    diagnostics: Option<ResMut<engine_core::diagnostics::Diagnostics>>,
) {
    let (Some(mut meshes), Some(mut materials)) = (meshes, materials) else {
        return;
    };
    let state = &mut *state;
    let mut wanted: Vec<(f64, CellKey)> = Vec::new();
    let mut wanted_far: Vec<(f64, CellKey)> = Vec::new();
    for (&e, t) in &terrains.entries {
        let field = &t.config.landmarks;
        if field.per_km2 <= 0.0 || !t.source.map.is_sphere() {
            continue;
        }
        let pose = (t.position, t.rotation);
        let entry = state.terrains.entry(e).or_insert_with(|| TerrainLandmarks {
            rock: materials.add(rock_material(field)),
            ice: materials.add(ice_material()),
            version: t.version,
            pose,
        });
        if entry.version != t.version {
            entry.version = t.version;
            if let Some(m) = materials.get_mut(entry.rock) {
                *m = rock_material(field);
            }
            let stale: Vec<CellKey> = state.cells.keys().filter(|k| k.0 == e).copied().collect();
            for k in stale {
                drop_cell(&mut commands, &mut meshes, state.cells.remove(&k));
            }
            state.building.retain(|k, _| k.0 != e);
            let stale: Vec<CellKey> = state.far.keys().filter(|k| k.0 == e).copied().collect();
            for k in stale {
                drop_far(&mut commands, &mut meshes, state.far.remove(&k));
            }
            state.far_building.retain(|k, _| k.0 != e);
        }
        if entry.pose != pose {
            entry.pose = pose;
            for (_, c) in state.cells.iter().filter(|(k, _)| k.0 == e) {
                let tf = place(pose, c.origin, c.rotation);
                for &entity in c.entities.iter().chain(&c.collider_entity) {
                    commands.entity(entity).insert(tf);
                }
            }
            for (_, c) in state.far.iter().filter(|(k, _)| k.0 == e) {
                let tf = place(pose, c.origin, c.rotation);
                for &entity in c.groups.iter().flat_map(|g| &g.entities) {
                    commands.entity(entity).insert(tf);
                }
            }
        }
        let level = level_for(&t.source.map);
        let far_level = level.saturating_sub(FAR_LEVELS_UP);
        for &p in &focus.points {
            let altitude = t.altitude_hint(p) - t.config.height_range().1;
            if altitude > field.far_m.max(REACH_M) {
                continue;
            }
            let local = t.to_local(p);
            if field.far_m > REACH_M {
                for (d, key) in cells_around(&t.source.map, local, far_level, field.far_m) {
                    wanted_far.push((d, (e, key)));
                }
            }
            if altitude > REACH_M {
                continue;
            }
            for (d, key) in cells_around(&t.source.map, local, level, REACH_M) {
                wanted.push((d, (e, key)));
            }
        }
    }
    state
        .terrains
        .retain(|e, _| terrains.entries.contains_key(e));
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Each wanted cell's distance from the nearest focus.
    let mut nearest: HashMap<CellKey, f64> = HashMap::new();
    for &(d, k) in &wanted {
        nearest.entry(k).or_insert(d);
    }
    let stale: Vec<CellKey> = state
        .cells
        .keys()
        .filter(|k| !nearest.contains_key(k))
        .copied()
        .collect();
    for k in stale {
        drop_cell(&mut commands, &mut meshes, state.cells.remove(&k));
    }
    // Put finished cells in place.
    let done: Vec<CellKey> = state
        .building
        .iter()
        .filter(|(_, h)| h.is_finished())
        .map(|(k, _)| *k)
        .collect();
    for k in done {
        let Some(handle) = state.building.remove(&k) else {
            continue;
        };
        let Ok(built) = handle.join() else {
            continue;
        };
        if !nearest.contains_key(&k) {
            continue;
        }
        let Some(terrain) = state.terrains.get(&k.0) else {
            continue;
        };
        let tf = place(terrain.pose, built.origin, built.rotation);
        let mut cell = Cell {
            entities: Vec::new(),
            meshes: Vec::new(),
            collider: built.collider,
            collider_entity: None,
            origin: built.origin,
            rotation: built.rotation,
        };
        for (mesh, material, name) in [
            (built.rock, terrain.rock, "Landmarks"),
            (built.ice, terrain.ice, "Ice spires"),
        ] {
            let Some(mesh) = mesh else {
                continue;
            };
            let h = meshes.add(mesh);
            cell.entities.push(
                commands
                    .spawn((
                        Name::new(name),
                        tf,
                        MeshRenderer {
                            mesh: MeshRef::Handle(h),
                            material: MaterialRef::Handle(material),
                            cast_shadows: true,
                            visible: true,
                        },
                    ))
                    .id(),
            );
            cell.meshes.push(h);
        }
        state.cells.insert(k, cell);
        state.placed += 1;
    }
    // Far cells: drop the unwanted, put finished ones in place, show each group while its
    // near cell is not drawn.
    wanted_far.sort_by(|a, b| a.0.total_cmp(&b.0));
    let far_wanted: HashSet<CellKey> = wanted_far.iter().map(|w| w.1).collect();
    let stale: Vec<CellKey> = state
        .far
        .keys()
        .filter(|k| !far_wanted.contains(k))
        .copied()
        .collect();
    for k in stale {
        drop_far(&mut commands, &mut meshes, state.far.remove(&k));
    }
    let done: Vec<CellKey> = state
        .far_building
        .iter()
        .filter(|(_, h)| h.is_finished())
        .map(|(k, _)| *k)
        .collect();
    for k in done {
        let Some(Ok(built)) = state.far_building.remove(&k).map(|h| h.join()) else {
            continue;
        };
        let Some(terrain) = state.terrains.get(&k.0).filter(|_| far_wanted.contains(&k)) else {
            continue;
        };
        let tf = place(terrain.pose, built.origin, built.rotation);
        let mut cell = FarCell {
            groups: Vec::new(),
            origin: built.origin,
            rotation: built.rotation,
        };
        for (near, rock, ice) in built.groups {
            let shown = !state.cells.contains_key(&(k.0, near));
            let mut group = FarGroup {
                near,
                entities: Vec::new(),
                meshes: Vec::new(),
                shown,
            };
            for (mesh, material) in [(rock, terrain.rock), (ice, terrain.ice)] {
                let Some(mesh) = mesh else {
                    continue;
                };
                let h = meshes.add(mesh);
                group.entities.push(
                    commands
                        .spawn((
                            Name::new("Far landmarks"),
                            tf,
                            MeshRenderer {
                                mesh: MeshRef::Handle(h),
                                material: MaterialRef::Handle(material),
                                cast_shadows: false,
                                visible: shown,
                            },
                        ))
                        .id(),
                );
                group.meshes.push(h);
            }
            cell.groups.push(group);
        }
        state.far.insert(k, cell);
    }
    for (k, cell) in state.far.iter_mut() {
        for g in &mut cell.groups {
            let shown = !state.cells.contains_key(&(k.0, g.near));
            if shown == g.shown {
                continue;
            }
            g.shown = shown;
            for &e in &g.entities {
                if let Ok(mut r) = renderers.get_mut(e) {
                    r.visible = shown;
                }
            }
        }
    }
    // Colliders on the cells near a focus.
    let surface = surfaces
        .map(|s| s.id_or_default("rock"))
        .unwrap_or_default();
    for (k, cell) in state.cells.iter_mut() {
        let near = nearest.get(k).is_some_and(|&d| d <= COLLIDER_REACH_M);
        match (near, cell.collider_entity, &cell.collider) {
            (true, None, Some(shape)) => {
                let Some(terrain) = state.terrains.get(&k.0) else {
                    continue;
                };
                cell.collider_entity = Some(
                    commands
                        .spawn((
                            Name::new("Landmark collider"),
                            place(terrain.pose, cell.origin, cell.rotation),
                            Collider {
                                shape: shape.clone(),
                                surface,
                                ..Default::default()
                            },
                        ))
                        .id(),
                );
            }
            (false, Some(e), _) => {
                commands.entity(e).despawn();
                cell.collider_entity = None;
            }
            _ => {}
        }
    }
    // Start the nearest missing cells.
    for (_, k) in &wanted {
        if state.building.len() >= MAX_BUILDING {
            break;
        }
        if state.cells.contains_key(k) || state.building.contains_key(k) {
            continue;
        }
        let Some(t) = terrains.entries.get(&k.0) else {
            continue;
        };
        let (source, field, key) = (t.source.clone(), t.config.landmarks.clone(), k.1);
        let seed = t.config.shape.seed;
        state.building.insert(
            *k,
            std::thread::spawn(move || build_cell(&source, &field, key, seed)),
        );
    }
    for (_, k) in &wanted_far {
        if state.far_building.len() >= MAX_FAR_BUILDING {
            break;
        }
        if state.far.contains_key(k) || state.far_building.contains_key(k) {
            continue;
        }
        let Some(t) = terrains.entries.get(&k.0) else {
            continue;
        };
        let (source, field, key) = (t.source.clone(), t.config.landmarks.clone(), k.1);
        let seed = t.config.shape.seed;
        state.far_building.insert(
            *k,
            std::thread::spawn(move || build_far_cell(&source, &field, key, seed)),
        );
    }
    if let Some(mut d) = diagnostics {
        d.set("terrain.landmark_far", state.far_shown() as f64);
        d.set("terrain.landmark_cells", state.drawn() as f64);
        d.set("terrain.landmark_builds", state.building.len() as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(field: &LandmarkField) -> Ctx<'_> {
        Ctx {
            field,
            planet_seed: 1,
            seed: 2,
            ground_asl: 1_000.0,
            frame_pos: DVec3::new(0.0, 3.4e6, 0.0),
            frame_rot: DQuat::IDENTITY,
        }
    }

    #[test]
    fn surface_nets_mesh_a_sphere_closed_and_outward() {
        let f = |p: DVec3| p.length() - 5.0;
        let (pos, tris) =
            surface_nets(&f, DVec3::splat(-7.0), DVec3::splat(7.0), 0.5, 100_000).unwrap();
        assert!(pos.len() > 300, "{}", pos.len());
        for p in &pos {
            assert!((p.length() - 5.0).abs() < 0.3, "{p:?}");
        }
        // Closed: every edge is shared by exactly two triangles.
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                // Facing out.
            }
            let p = |i: u32| pos[i as usize];
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            assert!(n.dot(p(t[0]) + p(t[1]) + p(t[2])) > 0.0);
        }
        assert!(edges.values().all(|&c| c == 2));
    }

    #[test]
    fn every_kind_of_formation_meshes_to_its_size() {
        let field = LandmarkField::earth();
        let c = ctx(&field);
        for (i, kind) in [
            LandmarkKind::Arch,
            LandmarkKind::Hoodoos,
            LandmarkKind::Butte,
            LandmarkKind::Tor,
            LandmarkKind::IceSpires,
        ]
        .into_iter()
        .enumerate()
        {
            for seed in 0..3u64 {
                let (height_m, width_m) = size_of(kind, 0.5, 0.5);
                let site = Landmark {
                    kind,
                    position: DVec3::new(0.0, 3.4e6, 0.0),
                    up: DVec3::Y,
                    height_m,
                    width_m,
                    yaw: 0.0,
                    seed: seed * 7 + i as u64,
                    ground_asl: 1_000.0,
                };
                let parts = parts_of(&site, &|_, _| 0.0);
                assert!(!parts.is_empty(), "{kind:?}");
                let mut top = f64::MIN;
                let mut verts = 0;
                for part in &parts {
                    let f = |p: DVec3| part.sdf(p, &c).0;
                    let (pos, tris) =
                        surface_nets(&f, part.min, part.max, part.voxel, MAX_PART_VERTICES)
                            .expect("within budget");
                    assert!(!tris.is_empty(), "{kind:?}");
                    verts += pos.len();
                    for p in &pos {
                        top = top.max(p.y + part.at.y);
                    }
                    // Nothing is cut off by its bounds: they lie outside it all round.
                    let (lo, hi) = (part.min, part.max);
                    let steps = 24;
                    for i in 0..=steps {
                        for j in 0..=steps {
                            let (u, v) = (i as f64 / steps as f64, j as f64 / steps as f64);
                            let x = lo.x + (hi.x - lo.x) * u;
                            let y = lo.y + (hi.y - lo.y) * v;
                            let z = lo.z + (hi.z - lo.z) * v;
                            let yy = lo.y + (hi.y - lo.y) * u;
                            for p in [
                                DVec3::new(x, y, lo.z),
                                DVec3::new(x, y, hi.z),
                                DVec3::new(lo.x, yy, z),
                                DVec3::new(hi.x, yy, z),
                                DVec3::new(x, hi.y, z),
                            ] {
                                assert!(f(p) > 0.0, "{kind:?} open at {p:?}");
                            }
                        }
                    }
                }
                assert!(
                    top > height_m * 0.6 && top < height_m * 1.4,
                    "{kind:?} {top} vs {height_m}"
                );
                assert!(verts < 400_000, "{kind:?} {verts}");
            }
        }
    }

    #[test]
    fn a_window_arch_has_an_opening_you_can_see_through() {
        let field = LandmarkField::earth();
        let c = ctx(&field);
        let part = Part {
            at: DVec3::ZERO,
            shape: Shape::Window {
                h: 30.0,
                len: 50.0,
                thick: 7.0,
                span: 26.0,
                hole_h: 18.0,
            },
            min: DVec3::splat(-40.0),
            max: DVec3::splat(40.0),
            voxel: 0.5,
            bottom: -2.0,
        };
        // Through the middle of the opening along Z: open; a pillar beside it: rock; above
        // the opening: the span.
        assert!(part.sdf(DVec3::new(0.0, 8.0, 0.0), &c).0 > 0.0);
        assert!(part.sdf(DVec3::new(19.0, 8.0, 0.0), &c).0 < 0.0);
        assert!(part.sdf(DVec3::new(0.0, 25.5, 0.0), &c).0 < 0.0);
    }

    #[test]
    fn beds_line_up_across_formations() {
        let field = LandmarkField::mars();
        let a = Ctx {
            frame_pos: DVec3::new(0.0, 3.4e6, 0.0),
            ..ctx(&field)
        };
        let b = Ctx {
            frame_pos: DVec3::new(30.0, 3.4e6, 0.0),
            ground_asl: 1_010.0,
            seed: 99,
            ..ctx(&field)
        };
        // The same height above sea level, 30 m apart: (nearly) the same bed.
        let mut same = 0;
        for i in 0..50 {
            let y = i as f64 * 1.3;
            if (a.hardness(DVec3::new(0.0, y + 10.0, 0.0)) - b.hardness(DVec3::new(0.0, y, 0.0)))
                .abs()
                < 0.2
            {
                same += 1;
            }
        }
        assert!(same >= 40, "{same}");
    }

    #[test]
    fn mars_has_formations_in_its_badlands_and_the_same_every_time() {
        let t = PlanetTerrain::mars(3);
        let r = t.shape.radius_m;
        let mut found = Vec::new();
        for lat in [-30.0, -10.0, 10.0, 30.0] {
            for lon in [0.0, 60.0, 120.0, 180.0, 240.0, 300.0] {
                let at = crate::dir_from_lat_lon(lat, lon) * r;
                found.extend(landmarks_near(&t, at, 4_000.0));
            }
        }
        assert!(found.len() >= 3, "{} formations", found.len());
        let first = found[0];
        let again = landmarks_near(&t, first.position, 10.0);
        assert_eq!(again.first().map(|l| l.position), Some(first.position));
        // On the ground.
        let source = crate::TerrainRuntime::new(&t, Default::default(), 1).source;
        let (dir, up, base) = source.map.locate(first.position).unwrap();
        let g = source.sample(base, up, dir, 0.0).height;
        assert!(((first.position.length() - r) - g).abs() < 0.5);
    }

    #[test]
    fn a_mars_cell_builds_in_good_time_with_a_collider() {
        let t = PlanetTerrain::mars(3);
        let r = t.shape.radius_m;
        let source = crate::TerrainRuntime::new(&t, Default::default(), 1).source;
        let level = level_for(&source.map);
        let mut built = None;
        'search: for lat in [-30.0, -10.0, 10.0, 30.0] {
            for lon in [0.0, 60.0, 120.0, 180.0, 240.0, 300.0] {
                let at = crate::dir_from_lat_lon(lat, lon) * r;
                for (_, key) in cells_around(&source.map, at, level, 4_000.0) {
                    if !sites_in(&source, &t.landmarks, key, t.shape.seed).is_empty() {
                        let start = std::time::Instant::now();
                        let cell = build_cell(&source, &t.landmarks, key, t.shape.seed);
                        built = Some((cell, start.elapsed()));
                        break 'search;
                    }
                }
            }
        }
        let (cell, took) = built.expect("a cell with formations");
        assert!(cell.rock.is_some());
        assert!(matches!(cell.collider, Some(ColliderShape::TriMesh { .. })));
        assert!(took.as_secs_f64() < 8.0, "{took:?}");
    }

    #[test]
    fn a_tor_is_sheets_of_jointed_blocks_with_clitter_around() {
        for seed in 0..4u64 {
            let (height_m, width_m) = size_of(LandmarkKind::Tor, 0.6, 0.5);
            let site = Landmark {
                kind: LandmarkKind::Tor,
                position: DVec3::new(0.0, 3.4e6, 0.0),
                up: DVec3::Y,
                height_m,
                width_m,
                yaw: 0.0,
                seed,
                ground_asl: 400.0,
            };
            let parts = parts_of(&site, &|_, _| 0.0);
            let Shape::Tor { blocks, .. } = &parts[0].shape else {
                panic!("a tor");
            };
            let mut levels: Vec<i64> = blocks
                .iter()
                .map(|b| (b.center.y * 10.0).round() as i64)
                .collect();
            levels.sort_unstable();
            levels.dedup();
            assert!(blocks.len() >= 6, "{} blocks", blocks.len());
            assert!(levels.len() >= 4, "{} sheets", levels.len());
            // Joint blocks side by side in the lower sheets, not one slab.
            let lowest = levels[1];
            let across = blocks
                .iter()
                .filter(|b| (b.center.y * 10.0).round() as i64 == lowest)
                .count();
            assert!(across >= 2, "{across} blocks across");
            assert!(parts.len() >= 7, "{} clitter boulders", parts.len() - 1);
        }
    }

    #[test]
    fn big_formations_show_from_afar_coarsely() {
        let t = PlanetTerrain::mars(3);
        let r = t.shape.radius_m;
        let source = crate::TerrainRuntime::new(&t, Default::default(), 1).source;
        let far_level = level_for(&source.map) - FAR_LEVELS_UP;
        let mut found = None;
        'search: for lat in [-30.0, -10.0, 10.0, 30.0] {
            for lon in [0.0, 60.0, 120.0, 180.0, 240.0, 300.0] {
                let at = crate::dir_from_lat_lon(lat, lon) * r;
                for (_, key) in cells_around(&source.map, at, far_level, 8_000.0) {
                    let far = build_far_cell(&source, &t.landmarks, key, t.shape.seed);
                    if let Some((near, Some(rock), _)) =
                        far.groups.iter().find(|g| g.1.is_some()).cloned()
                    {
                        found = Some((key, near, rock));
                        break 'search;
                    }
                }
            }
        }
        let (key, near, far_rock) = found.expect("a far cell with a big formation");
        // The group's near cell lies inside the far cell.
        let mut up = near;
        for _ in 0..FAR_LEVELS_UP {
            up = up.parent().unwrap();
        }
        assert_eq!(up, key);
        let close = build_cell(&source, &t.landmarks, near, t.shape.seed);
        let close = close.rock.expect("the near cell has it too");
        assert!(
            far_rock.vertices.len() * 6 < close.vertices.len(),
            "far {} vs near {}",
            far_rock.vertices.len(),
            close.vertices.len()
        );
    }

    #[test]
    fn nothing_stands_on_the_home_pad() {
        let pad = crate::FlatPad::default();
        let t = PlanetTerrain {
            landmarks: LandmarkField {
                per_km2: 50.0,
                dry_below: 2.0,
                ..LandmarkField::earth()
            },
            ..PlanetTerrain::earthlike(7).with_pad(pad)
        };
        let at = pad.direction() * t.shape.radius_m;
        let near = landmarks_near(&t, at, 3_000.0);
        assert!(near.is_empty(), "{}", near.len());
    }

    #[test]
    fn cells_stay_light_enough_to_stream() {
        let t = PlanetTerrain::mars(1);
        let r = t.shape.radius_m;
        let source = crate::TerrainRuntime::new(&t, Default::default(), 1).source;
        let level = level_for(&source.map);
        let mut most = 0;
        for (lat, lon) in [(-39.0, 18.0), (0.0, 90.0), (30.0, 270.0)] {
            let at = crate::dir_from_lat_lon(lat, lon) * r;
            for (_, key) in cells_around(&source.map, at, level, 2_000.0) {
                if sites_in(&source, &t.landmarks, key, t.shape.seed).is_empty() {
                    continue;
                }
                let cell = build_cell(&source, &t.landmarks, key, t.shape.seed);
                most = most.max(cell.rock.map_or(0, |m| m.vertices.len()));
            }
        }
        assert!(most > 0 && most < 200_000, "{most} vertices");
    }

    #[test]
    fn no_cell_near_a_point_is_skipped_toward_a_face_edge() {
        let map = SurfaceMap::new(TerrainDomain::Sphere, 3_389_500.0);
        let level = level_for(&map);
        // Points across a face, out to near its edge where cells are smallest.
        for a in [-0.95, -0.55, 0.0, 0.4, 0.9] {
            for b in [-0.9, 0.0, 0.7] {
                let at = map.position(4, a, b, 0.0);
                let around = cells_around(&map, at, level, 1_000.0);
                // Every point within 900 m lies in a listed cell.
                let up = at.normalize();
                let side = up.any_orthonormal_vector();
                let fwd = up.cross(side);
                for k in 0..36 {
                    let ang = k as f64 / 36.0 * TAU;
                    for d in [100.0, 450.0, 900.0] {
                        let q = at + (side * ang.cos() + fwd * ang.sin()) * d;
                        let key = map.node_at(q, level).unwrap();
                        assert!(around.iter().any(|c| c.1 == key), "{a} {b} {k} {d}");
                    }
                }
            }
        }
    }
}
