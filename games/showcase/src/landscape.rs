//! The showcase landscape: a green valley ringed by snowy mountains, with a flat pad for every
//! zone, dirt roads between them, a lake basin and a hill the cave is dug into.
//!
//! [`Landscape::height`] is the one source of truth for the ground. The terrain mesh, every
//! tree, rock and building, and the [`Ground`](engine::world::physical::Ground) query that
//! modules use (footsteps, foliage, AI) all read it, so things sit on the ground everywhere.
//!
//! When `engine_terrain` ships heightmap terrain, the showcase hands it this height function and
//! drops the mesh built here.

use engine::core::math::noise::{fbm2, perlin2, ridged2};
use engine::core::math::smoothstep;
use engine::prelude::*;
use engine::render::Vertex;
use engine::world::SurfaceId;
use engine::world::physical::{
    Deformation, GroundQuery, GroundSample, SurfaceMaterial, SurfaceMaterials,
};

/// Planet radius used to curve the far terrain, the same Earth the valley sits on (m).
pub const PLANET_RADIUS: f64 = 6_371_000.0;
/// The terrain mesh reaches this far from the plaza (m).
pub const TERRAIN_RADIUS: f64 = 24_000.0;
/// Height of the lake's water surface (m).
pub const LAKE_LEVEL: f64 = 0.0;
/// Snow starts this high (m).
const SNOW_LINE: f64 = 950.0;

/// A flat area: a zone, a runway, a building lot.
#[derive(Debug, Clone, Copy)]
pub struct Pad {
    pub a: DVec2,
    /// Second end for a capsule (runway); `a` for a disc.
    pub b: DVec2,
    pub radius: f64,
    /// Width of the blend back to the natural ground (m).
    pub falloff: f64,
    pub height: f64,
}

impl Pad {
    pub fn disc(center: DVec3, radius: f64) -> Self {
        let c = DVec2::new(center.x, center.z);
        Self {
            a: c,
            b: c,
            radius,
            falloff: 30.0,
            height: center.y,
        }
    }

    fn distance(&self, p: DVec2) -> f64 {
        distance_to_segment(p, self.a, self.b)
    }

    fn weight(&self, p: DVec2) -> f64 {
        let d = self.distance(p);
        1.0 - smooth(self.radius, self.radius + self.falloff, d)
    }
}

/// A dirt road (Catmull–Rom smoothed through `points`).
#[derive(Debug, Clone)]
pub struct Road {
    pub points: Vec<DVec2>,
    pub width: f64,
    /// Paved (cobblestone) instead of dirt.
    pub paved: bool,
    bounds: (DVec2, DVec2),
}

impl Road {
    pub fn new(control: &[[f64; 2]], width: f64, paved: bool) -> Self {
        let ctrl: Vec<DVec2> = control.iter().map(|p| DVec2::new(p[0], p[1])).collect();
        let mut points = Vec::new();
        for i in 0..ctrl.len().saturating_sub(1) {
            let p0 = ctrl[i.saturating_sub(1)];
            let p1 = ctrl[i];
            let p2 = ctrl[i + 1];
            let p3 = ctrl[(i + 2).min(ctrl.len() - 1)];
            let steps = ((p2 - p1).length() / 8.0).ceil().max(1.0) as usize;
            for s in 0..steps {
                let t = s as f64 / steps as f64;
                points.push(catmull_rom(p0, p1, p2, p3, t));
            }
        }
        if let Some(last) = ctrl.last() {
            points.push(*last);
        }
        let margin = width + 20.0;
        let min = points.iter().fold(DVec2::splat(f64::MAX), |m, p| m.min(*p)) - margin;
        let max = points.iter().fold(DVec2::splat(f64::MIN), |m, p| m.max(*p)) + margin;
        Self {
            points,
            width,
            paved,
            bounds: (min, max),
        }
    }

    /// Distance to the centerline and the closest point on it.
    pub fn closest(&self, p: DVec2) -> Option<(f64, DVec2)> {
        if p.x < self.bounds.0.x
            || p.y < self.bounds.0.y
            || p.x > self.bounds.1.x
            || p.y > self.bounds.1.y
        {
            return None;
        }
        let mut best: Option<(f64, DVec2)> = None;
        for w in self.points.windows(2) {
            let q = closest_on_segment(p, w[0], w[1]);
            let d = (p - q).length();
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, q));
            }
        }
        best
    }
}

/// The hill the mushroom cave is dug into: a mound with a box-shaped hole for the cave.
#[derive(Debug, Clone, Copy)]
pub struct CaveHill {
    /// Top of the hill (x, z).
    pub center: DVec2,
    /// Middle of the cave inside (x, z); the mouth faces −Z, toward the plaza.
    pub mouth: DVec2,
    /// Half size of the cave's inside (x, z), m. Inside it the ground is the cave floor (y = 0).
    pub hole: DVec2,
    pub hill_height: f64,
    pub hill_radius: f64,
}

/// A lake basin: water at [`LAKE_LEVEL`] out to about `radius`, a sandy shore, a deep middle.
#[derive(Debug, Clone, Copy)]
pub struct Lake {
    pub center: DVec2,
    pub radius: f64,
    pub depth: f64,
}

/// What kind of ground a spot is (colors and footstep surfaces).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    Grass,
    Forest,
    Dirt,
    Cobble,
    Sand,
    Rock,
    Snow,
    LakeBed,
    Mud,
    Gravel,
    Wood,
    Metal,
    Puddle,
    Ice,
    Concrete,
}

/// Number of [`Cover`] kinds.
pub const COVER_KINDS: usize = 15;

/// Every [`Cover`] kind, in order.
pub const COVERS: [Cover; COVER_KINDS] = [
    Cover::Grass,
    Cover::Forest,
    Cover::Dirt,
    Cover::Cobble,
    Cover::Sand,
    Cover::Rock,
    Cover::Snow,
    Cover::LakeBed,
    Cover::Mud,
    Cover::Gravel,
    Cover::Wood,
    Cover::Metal,
    Cover::Puddle,
    Cover::Ice,
    Cover::Concrete,
];

impl Cover {
    /// Footstep surface name in [`SurfaceMaterials`].
    pub fn surface_name(self) -> &'static str {
        match self {
            Cover::Grass => "grass",
            Cover::Forest => "leaves",
            Cover::Dirt => "dirt",
            Cover::Cobble => "rock",
            Cover::Sand => "sand",
            Cover::Rock => "rock",
            Cover::Snow => "snow",
            Cover::LakeBed => "mud",
            Cover::Mud => "mud",
            Cover::Gravel => "gravel",
            Cover::Wood => "wood",
            Cover::Metal => "metal",
            Cover::Puddle => "water",
            Cover::Ice => "ice",
            Cover::Concrete => "concrete",
        }
    }

    /// Ground color for surface patches painted into the terrain (linear).
    fn patch_color(self) -> Option<Vec3> {
        match self {
            Cover::Mud => Some(Vec3::new(0.07, 0.05, 0.035)),
            Cover::Snow => Some(Vec3::new(0.8, 0.83, 0.88)),
            Cover::Sand => Some(Vec3::new(0.45, 0.38, 0.25)),
            Cover::Gravel => Some(Vec3::new(0.2, 0.19, 0.18)),
            Cover::Dirt => Some(Vec3::new(0.17, 0.12, 0.07)),
            Cover::Ice => Some(Vec3::new(0.55, 0.65, 0.72)),
            Cover::Concrete => Some(Vec3::new(0.3, 0.29, 0.27)),
            Cover::Wood => Some(Vec3::new(0.2, 0.12, 0.06)),
            Cover::Metal => Some(Vec3::new(0.16, 0.16, 0.17)),
            Cover::Puddle => Some(Vec3::new(0.05, 0.04, 0.03)),
            Cover::Forest => Some(Vec3::new(0.12, 0.07, 0.03)),
            _ => None,
        }
    }
}

/// A rock step in the land: the ground on one side of the line from `a` to `b` (the side of
/// `(-d.y, d.x)` for `d` = a → b) rises by `height`, as a cliff at the line, and stays up for
/// `reach` metres before easing back into the hills.
#[derive(Debug, Clone, Copy)]
pub struct Scarp {
    pub a: DVec2,
    pub b: DVec2,
    pub height: f64,
    pub reach: f64,
    /// Distance along the line where a stream falls over it (set by
    /// [`Landscape::lay_streams`]). There the step is sharp, just behind the worn rock face,
    /// so the water leaves the lip and falls clear instead of sliding down a slope.
    pub notch: Option<f64>,
}

impl Scarp {
    /// Distance along the line from `a`, and signed distance from it (positive on the raised
    /// side). The line wobbles a few metres so the cliff isn't ruler-straight.
    pub fn frame(&self, p: DVec2, seed: u64) -> (f64, f64) {
        let d = (self.b - self.a).normalize();
        let rel = p - self.a;
        let along = rel.dot(d);
        let wobble = fbm2(along / 35.0, 0.5, seed + 31, 3, 2.0, 0.5) as f64 * 6.0;
        (along, rel.dot(DVec2::new(-d.y, d.x)) - wobble)
    }

    /// How much the land rises at `p` (m).
    pub fn rise(&self, p: DVec2, seed: u64) -> f64 {
        let (along, side) = self.frame(p, seed);
        let len = (self.b - self.a).length();
        let ends = smooth(-5.0, 40.0, along) * smooth(-5.0, 40.0, len - along);
        if ends <= 0.0 || side < -2.0 {
            return 0.0;
        }
        // The cliff's rock stands in front of a 3.5 m wide step; at the falls its face is worn
        // back (`rocks::face_at`), and the step narrows to sit just behind it.
        let notch = self
            .notch
            .map_or(0.0, |u| smooth(14.0, 5.0, (along - u).abs()));
        let face = smooth(-1.5 + 1.1 * notch, 2.0 - 0.9 * notch, side);
        let back = 1.0 - smooth(self.reach * 0.6, self.reach * 1.4, side);
        self.height * face * back * ends
    }
}

/// A stream from a spring to the lake. Its bed only ever runs downhill, so where the land drops
/// away (over a [`Scarp`]) the water falls; the bed is carved into the ground with steep banks.
#[derive(Debug, Clone)]
pub struct Stream {
    /// Centerline, one point per metre.
    pub path: Vec<DVec2>,
    pub width: f64,
    /// Depth of the bed below the ground it runs through (m).
    pub depth: f64,
    /// Bed height at each path point (filled in by [`Landscape::lay_streams`]).
    pub bed: Vec<f64>,
    /// The plunge pool under the falls: center and bed height.
    pub pool: Option<(DVec2, f64)>,
    bounds: (DVec2, DVec2),
}

impl Stream {
    pub fn new(control: &[[f64; 2]], width: f64, depth: f64) -> Self {
        let smooth_line = Road::new(control, width, false).points;
        let mut path = vec![smooth_line[0]];
        for w in smooth_line.windows(2) {
            let n = (w[1] - w[0]).length().ceil().max(1.0) as usize;
            for k in 1..=n {
                path.push(w[0].lerp(w[1], k as f64 / n as f64));
            }
        }
        let margin = width + 25.0;
        let min = path.iter().fold(DVec2::splat(f64::MAX), |m, p| m.min(*p)) - margin;
        let max = path.iter().fold(DVec2::splat(f64::MIN), |m, p| m.max(*p)) + margin;
        Self {
            bed: vec![0.0; path.len()],
            path,
            width,
            depth,
            pool: None,
            bounds: (min, max),
        }
    }

    /// Distance to the centerline and the bed height at the closest point.
    pub fn closest(&self, p: DVec2) -> Option<(f64, f64)> {
        if p.x < self.bounds.0.x
            || p.y < self.bounds.0.y
            || p.x > self.bounds.1.x
            || p.y > self.bounds.1.y
        {
            return None;
        }
        let mut best: Option<(f64, f64)> = None;
        for (i, w) in self.path.windows(2).enumerate() {
            let q = closest_on_segment(p, w[0], w[1]);
            let d = (p - q).length();
            if best.is_none_or(|(bd, _)| d < bd) {
                let t = (q - w[0]).length() / (w[1] - w[0]).length().max(1e-9);
                best = Some((d, self.bed[i] + (self.bed[i + 1] - self.bed[i]) * t));
            }
        }
        best
    }

    /// The waterfall: where the bed drops the most over 4 m. Returns the path index at the top
    /// and at the bottom of the drop.
    pub fn falls(&self) -> Option<(usize, usize)> {
        (0..self.bed.len().saturating_sub(4))
            .map(|i| (i, i + 4))
            .max_by(|a, b| {
                let da = self.bed[a.0] - self.bed[a.1];
                let db = self.bed[b.0] - self.bed[b.1];
                da.total_cmp(&db)
            })
            .filter(|&(i, j)| self.bed[i] - self.bed[j] > 2.0)
    }
}

/// The whole landscape description.
#[derive(Debug, Clone)]
pub struct Landscape {
    pub seed: u64,
    pub pads: Vec<Pad>,
    pub roads: Vec<Road>,
    pub lake: Lake,
    pub cave: CaveHill,
    /// Woodland centers on the hills (x, z, radius) beyond the forest zone.
    pub woods: Vec<(DVec2, f64)>,
    /// Rectangles (min x/z, max x/z) with their own ground: the footprint lab's mud, snow and
    /// sand.
    pub patches: Vec<(DVec2, DVec2, Cover)>,
    /// Ground laid out by module demos (the footstep trail): the cover at (x, z), or `None`
    /// to leave the landscape's own. Checked before `patches`.
    pub painters: Vec<fn(f64, f64) -> Option<Cover>>,
    /// Farm fields on the valley floor: grid cell and crop (see [`Landscape::plant_fields`]).
    pub fields: Vec<((i32, i32), Crop)>,
    /// Rock steps (the cliff the waterfall drops over).
    pub scarps: Vec<Scarp>,
    /// Streams down to the lake (lay them out with [`Landscape::lay_streams`]).
    pub streams: Vec<Stream>,
}

/// What grows on a farm field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crop {
    Wheat,
    Barley,
    Plowed,
    Young,
    Pasture,
    Lavender,
}

impl Crop {
    fn color(self) -> Vec3 {
        match self {
            Crop::Wheat => Vec3::new(0.36, 0.27, 0.09),
            Crop::Barley => Vec3::new(0.27, 0.25, 0.1),
            Crop::Plowed => Vec3::new(0.11, 0.075, 0.045),
            Crop::Young => Vec3::new(0.09, 0.2, 0.035),
            Crop::Pasture => Vec3::new(0.08, 0.17, 0.035),
            Crop::Lavender => Vec3::new(0.17, 0.11, 0.24),
        }
    }
}

/// Field grid: cells of 64 × 44 m, turned 0.35 rad.
const FIELD_SIZE: DVec2 = DVec2::new(64.0, 44.0);
const FIELD_ANGLE: f64 = 0.35;

/// Field grid cell of a point and the distance to the cell's edge (m).
pub fn field_cell(x: f64, z: f64) -> ((i32, i32), f64) {
    let (s, c) = FIELD_ANGLE.sin_cos();
    let u = x * c + z * s;
    let v = -x * s + z * c;
    let fu = u / FIELD_SIZE.x;
    let fv = v / FIELD_SIZE.y;
    let (iu, iv) = (fu.floor(), fv.floor());
    let du = (fu - iu).min(1.0 - (fu - iu)) * FIELD_SIZE.x;
    let dv = (fv - iv).min(1.0 - (fv - iv)) * FIELD_SIZE.y;
    ((iu as i32, iv as i32), du.min(dv))
}

/// World-space center and corners of a field cell (x, z).
pub fn field_corners(cell: (i32, i32)) -> [DVec2; 4] {
    let (s, c) = FIELD_ANGLE.sin_cos();
    let to_world = |u: f64, v: f64| DVec2::new(u * c - v * s, u * s + v * c);
    let (u0, v0) = (cell.0 as f64 * FIELD_SIZE.x, cell.1 as f64 * FIELD_SIZE.y);
    [
        to_world(u0, v0),
        to_world(u0 + FIELD_SIZE.x, v0),
        to_world(u0 + FIELD_SIZE.x, v0 + FIELD_SIZE.y),
        to_world(u0, v0 + FIELD_SIZE.y),
    ]
}

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    smoothstep(e0 as f32, e1 as f32, x as f32) as f64
}

fn catmull_rom(p0: DVec2, p1: DVec2, p2: DVec2, p3: DVec2, t: f64) -> DVec2 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

fn closest_on_segment(p: DVec2, a: DVec2, b: DVec2) -> DVec2 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-9 {
        return a;
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    a + ab * t
}

fn distance_to_segment(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    (p - closest_on_segment(p, a, b)).length()
}

impl Landscape {
    /// Rolling hills near the plaza, foothills further out and a mountain range from about
    /// 2 km, highest (2.5 km) to the north-west. No pads, roads or lake.
    pub fn natural(&self, x: f64, z: f64) -> f64 {
        let s = self.seed;
        let r = (x * x + z * z).sqrt();
        // Valley floor: gentle near the plaza, hillier toward the edges.
        let hills = fbm2(x / 420.0, z / 420.0, s, 5, 2.0, 0.5) as f64 * 16.0 + 4.0;
        let valley = smooth(200.0, 1300.0, r);
        let mut h = hills * (0.4 + 0.9 * valley);
        // Foothills.
        let foot = (fbm2(x / 1100.0, z / 1100.0, s + 1, 5, 2.0, 0.5) as f64 * 0.5 + 0.5).powf(1.4);
        h += foot * 260.0 * smooth(900.0, 2600.0, r);
        // Mountains: ridged noise, stronger in some directions so the skyline varies.
        let angle = z.atan2(x);
        let dir_bias =
            0.55 + 0.45 * perlin2(angle.cos() * 1.3 + 7.0, angle.sin() * 1.3 + 3.0, s + 5) as f64;
        let ridge = ridged2(x / 3800.0, z / 3800.0, s + 2, 7) as f64;
        let mountains = ridge.powf(1.6) * 3200.0 * dir_bias.max(0.15);
        h += mountains * smooth(1700.0, 5200.0, r);
        // Flatten toward the edge of the mesh so it meets the planet's ground color.
        h * (1.0 - smooth(TERRAIN_RADIUS * 0.7, TERRAIN_RADIUS * 0.98, r))
    }

    /// Ground height without roads (roads sample this at their centerline).
    fn base_height(&self, x: f64, z: f64) -> f64 {
        self.shaped_height(x, z, true)
    }

    /// The cave hill's outside surface, carried on over the cave (where the ground itself is
    /// the cave floor). The cave's roof follows it.
    pub fn cave_roof_height(&self, x: f64, z: f64) -> f64 {
        self.shaped_height(x, z, false) - (x * x + z * z) / (2.0 * PLANET_RADIUS)
    }

    /// Height before roads and curvature. `carve` digs the cave and the cut in front of it.
    fn shaped_height(&self, x: f64, z: f64, carve: bool) -> f64 {
        let p = DVec2::new(x, z);
        let mut h = self.natural(x, z);
        for scarp in &self.scarps {
            h += scarp.rise(p, self.seed);
        }
        for pad in &self.pads {
            let w = pad.weight(p);
            if w > 0.0 {
                h += (pad.height - h) * w;
            }
        }
        // The cave hill: a mound whose middle is the cave (floor at 0), with a cut in front of
        // the mouth so the approach stays flat.
        let c = self.cave;
        let d = (p - c.center).length();
        if d < c.hill_radius {
            let local = p - c.mouth;
            let rel = local.abs() - c.hole;
            let outside = rel.max(DVec2::ZERO).length();
            let mut mound = c.hill_height * (1.0 - smooth(0.0, c.hill_radius, d)).powf(0.7);
            if carve && local.y < -c.hole.y {
                mound *= smooth(6.0, 18.0, local.x.abs());
            }
            let rough = fbm2(x / 9.0, z / 9.0, self.seed + 9, 3, 2.0, 0.5) as f64 * 1.6;
            let hill = mound + rough * smooth(0.0, 4.0, mound);
            if !carve {
                h = h.max(hill);
            } else if rel.x < 0.0 && rel.y < 0.0 {
                h = 0.0;
            } else {
                // A steep rock face around the mouth. Elsewhere the hill runs right up to the
                // cave and its roof carries the slope on over it.
                let toward_front = smooth(c.hole.y - 6.0, c.hole.y, -local.y);
                let wall = 1.0 - (1.0 - smooth(0.0, 6.0, outside)) * toward_front;
                h = h.max(hill * wall);
            }
        }
        // Lake basin.
        let l = self.lake;
        let d = (p - l.center).length();
        if d < l.radius * 2.6 {
            let t = d / (l.radius * self.lake_wobble(x, z));
            let shore = 0.9;
            if t < 1.25 {
                let bed = shore - (l.depth + shore) * (1.0 - t * t).max(0.0).powf(0.8);
                let blend = smooth(1.0, 1.25, t);
                h = bed * (1.0 - blend) + h.max(shore) * blend;
            } else {
                // Keep the land around the lake above the water so it can't spill.
                h = h.max(shore * (1.0 - smooth(1.8, 2.1, t)));
            }
        }
        h
    }

    /// Road influence at `p`: (weight 0..1 of the road's flat cross-section, road height,
    /// paved).
    fn road_at(&self, p: DVec2) -> Option<(f64, f64, bool, f64)> {
        let mut best: Option<(f64, f64, bool, f64)> = None;
        for road in &self.roads {
            if let Some((d, q)) = road.closest(p) {
                let half = road.width * 0.5;
                let w = 1.0 - smooth(half, half + 7.0, d);
                if w > 0.0 && best.is_none_or(|b| w > b.0) {
                    let edge = 1.0 - smooth(half - 1.0, half + 0.8, d);
                    best = Some((w, self.base_height(q.x, q.y), road.paved, edge));
                }
            }
        }
        best
    }

    /// Ground height at world X/Z (m), including the planet's curvature.
    pub fn height(&self, x: f64, z: f64) -> f64 {
        self.flat_height(x, z) - (x * x + z * z) / (2.0 * PLANET_RADIUS)
    }

    /// Ground height at world X/Z (m) as if the world were flat: what the planet terrain bends
    /// onto the sphere (it comes out at [`height`](Self::height)).
    pub fn flat_height(&self, x: f64, z: f64) -> f64 {
        self.carve_streams(DVec2::new(x, z), self.uncarved_height(x, z))
    }

    /// Height with roads but before streams cut into it (and without curvature).
    fn uncarved_height(&self, x: f64, z: f64) -> f64 {
        let mut h = self.base_height(x, z);
        if let Some((w, rh, _, _)) = self.road_at(DVec2::new(x, z)) {
            h += (rh - h) * w;
        }
        h
    }

    fn carve_streams(&self, p: DVec2, mut h: f64) -> f64 {
        for st in &self.streams {
            if let Some((d, bed)) = st.closest(p) {
                let half = st.width * 0.5;
                if d < half + 20.0 {
                    h = h.min(bed + (d - half).max(0.0) * 1.5);
                }
            }
            if let Some((c, bed)) = st.pool {
                let d = (p - c).length();
                if d < 25.0 {
                    // Only in front of the cliff: the pool doesn't eat back into the lip.
                    let behind = self
                        .scarps
                        .iter()
                        .map(|sc| smooth(-0.6, 0.4, sc.frame(p, self.seed).1))
                        .fold(0.0, f64::max);
                    h = h.min(bed + (d - 4.5).max(0.0) * 0.8 + behind * 40.0);
                }
            }
        }
        h
    }

    /// Lay each stream's bed: follow the ground down from the spring (shallow at first), never
    /// climbing, and dig a plunge pool under the falls.
    pub fn lay_streams(&mut self) {
        let mut streams = std::mem::take(&mut self.streams);
        let lay = |land: &Self, st: &mut Stream| {
            let mut low = f64::MAX;
            for (i, p) in st.path.iter().enumerate() {
                let dig = st.depth * (0.2 + 0.8 * smooth(0.0, 30.0, i as f64));
                low = low.min(land.uncarved_height(p.x, p.y) - dig);
                st.bed[i] = low;
            }
        };
        // Find where each stream falls over a scarp, notch the scarp there, and lay the bed
        // again over the sharper step.
        for st in &mut streams {
            lay(self, st);
            let Some((top, _)) = st.falls() else {
                continue;
            };
            for sc in &mut self.scarps {
                let (u, s) = sc.frame(st.path[top], self.seed);
                if (0.0..=(sc.b - sc.a).length()).contains(&u) && s.abs() < 8.0 {
                    sc.notch = Some(u);
                }
            }
        }
        for st in &mut streams {
            lay(self, st);
            if let Some((_, bottom)) = st.falls() {
                // A few metres downstream of the drop, where the water lands.
                let j = (bottom + 3).min(st.path.len() - 1);
                st.pool = Some((st.path[j], st.bed[j] - 1.8));
            }
        }
        self.streams = streams;
    }

    /// Distance to the nearest stream's centerline, and its water surface height there.
    pub fn stream_at(&self, x: f64, z: f64) -> Option<(f64, f64)> {
        let p = DVec2::new(x, z);
        self.streams
            .iter()
            .filter_map(|st| st.closest(p).map(|(d, bed)| (d, bed + st.depth * 0.55)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Surface normal from central differences with step `e` (m).
    pub fn normal(&self, x: f64, z: f64, e: f64) -> Vec3 {
        let hx = self.height(x + e, z) - self.height(x - e, z);
        let hz = self.height(x, z + e) - self.height(x, z - e);
        Vec3::new((-hx) as f32, (2.0 * e) as f32, (-hz) as f32).normalize()
    }

    /// 0..1: how much of the zone/runway pads covers this spot.
    pub fn pad_weight(&self, x: f64, z: f64) -> f64 {
        let p = DVec2::new(x, z);
        self.pads
            .iter()
            .map(|pad| pad.weight(p))
            .fold(0.0, f64::max)
    }

    /// 0..1 road or stream coverage (1 on the road surface or in the water): scenery keeps
    /// off it.
    pub fn road_weight(&self, x: f64, z: f64) -> f64 {
        let road = self.road_at(DVec2::new(x, z)).map_or(0.0, |r| r.3);
        let stream = self.streams.iter().fold(0.0, |w: f64, st| {
            st.closest(DVec2::new(x, z)).map_or(w, |(d, _)| {
                w.max(1.0 - smooth(st.width * 0.5, st.width * 0.5 + 3.0, d))
            })
        });
        road.max(stream)
    }

    /// 0..1 woodland density (the forest zone and the woods on the hills).
    pub fn woodland(&self, x: f64, z: f64) -> f64 {
        let s = self.seed;
        // Warp the domain so wood edges wander instead of forming circles.
        let warp = DVec2::new(
            fbm2(x / 300.0, z / 300.0, s + 30, 3, 2.0, 0.5) as f64,
            fbm2(x / 300.0 + 5.2, z / 300.0 + 1.3, s + 31, 3, 2.0, 0.5) as f64,
        ) * 130.0;
        let q = DVec2::new(x, z) + warp;
        let mut bonus: f64 = 0.0;
        for (c, r) in &self.woods {
            let d = (q - *c).length();
            bonus = bonus.max(1.0 - smooth(r * 0.35, r * 1.1, d));
        }
        let n = fbm2(q.x / 260.0, q.y / 260.0, s + 11, 4, 2.0, 0.5) as f64;
        // More woods on the slopes toward the mountains than on the valley floor.
        let r = (x * x + z * z).sqrt();
        let slopes = smooth(450.0, 1400.0, r) * 0.3;
        let v = n + bonus * 0.75 + slopes - 0.18;
        smooth(0.0, 0.28, v)
    }

    /// The farm field at this spot, if any.
    pub fn field_at(&self, x: f64, z: f64) -> Option<(Crop, f64)> {
        let (cell, edge) = field_cell(x, z);
        self.fields
            .iter()
            .find(|f| f.0 == cell)
            .map(|f| (f.1, edge))
    }

    /// Water depth below the lake surface (≤ 0 means dry ground).
    pub fn lake_depth(&self, x: f64, z: f64) -> f64 {
        LAKE_LEVEL - self.height(x, z)
    }

    /// What covers the ground here.
    pub fn cover(&self, x: f64, z: f64, h: f64, normal: Vec3) -> Cover {
        if let Some(c) = self.patch_at(x, z) {
            return c;
        }
        if self.in_cave(x, z) {
            return Cover::Rock;
        }
        if self.stream_at(x, z).is_some_and(|(d, _)| d < 3.0) {
            return Cover::Gravel;
        }
        let slope = 1.0 - normal.y as f64;
        if h > SNOW_LINE + perlin2(x / 300.0, z / 300.0, self.seed + 6) as f64 * 150.0
            && slope < 0.45
        {
            return Cover::Snow;
        }
        if slope > 0.32 {
            return Cover::Rock;
        }
        if h < LAKE_LEVEL - 0.15 {
            return Cover::LakeBed;
        }
        if let Some((_, _, paved, edge)) = self.road_at(DVec2::new(x, z))
            && edge > 0.5
        {
            return if paved { Cover::Cobble } else { Cover::Dirt };
        }
        if self.lake_t(x, z) < 1.08 {
            return Cover::Sand;
        }
        if let Some((crop, edge)) = self.field_at(x, z)
            && edge > 2.0
        {
            return if crop == Crop::Plowed {
                Cover::Dirt
            } else {
                Cover::Grass
            };
        }
        if self.woodland(x, z) > 0.55 {
            return Cover::Forest;
        }
        Cover::Grass
    }

    /// Choose which grid cells become farm fields: flat open ground on the valley floor, clear
    /// of zones, roads, the lake and the woods.
    pub fn plant_fields(&mut self) {
        let mut rng = Rng::new(self.seed + 40);
        let mut fields = Vec::new();
        let n = (1000.0 / FIELD_SIZE.y) as i32;
        for iu in -n..n {
            for iv in -n..n {
                let corners = field_corners((iu, iv));
                let center = (corners[0] + corners[2]) * 0.5;
                let r = center.length();
                if !(110.0..900.0).contains(&r) {
                    continue;
                }
                let mut samples: Vec<DVec2> = corners.to_vec();
                samples.push(center);
                for k in 0..4 {
                    samples.push((corners[k] + corners[(k + 1) % 4]) * 0.5);
                }
                let heights: Vec<f64> = samples.iter().map(|p| self.natural(p.x, p.y)).collect();
                let relief = heights.iter().cloned().fold(f64::MIN, f64::max)
                    - heights.iter().cloned().fold(f64::MAX, f64::min);
                let clear = samples.iter().all(|p| {
                    self.pad_weight(p.x, p.y) < 0.05
                        && self.road_weight(p.x, p.y) < 0.5
                        && self.woodland(p.x, p.y) < 0.35
                        && (*p - self.lake.center).length() > self.lake.radius * 1.5
                        && (*p - self.cave.center).length() > self.cave.hill_radius
                });
                if !clear || relief > 7.0 || rng.chance(0.3) {
                    continue;
                }
                let crop = match rng.f32() {
                    r if r < 0.28 => Crop::Wheat,
                    r if r < 0.42 => Crop::Barley,
                    r if r < 0.58 => Crop::Plowed,
                    r if r < 0.74 => Crop::Young,
                    r if r < 0.94 => Crop::Pasture,
                    _ => Crop::Lavender,
                };
                fields.push(((iu, iv), crop));
            }
        }
        self.fields = fields;
    }

    /// Irregular shoreline: the lake's radius scale at this spot.
    fn lake_wobble(&self, x: f64, z: f64) -> f64 {
        1.0 + 0.16 * perlin2(x / 70.0, z / 70.0, self.seed + 4) as f64
            + 0.07 * perlin2(x / 23.0, z / 23.0, self.seed + 5) as f64
    }

    /// Distance from the lake's middle in shoreline units (1 = the shore).
    fn lake_t(&self, x: f64, z: f64) -> f64 {
        let d = (DVec2::new(x, z) - self.lake.center).length();
        d / (self.lake.radius * self.lake_wobble(x, z))
    }

    /// Inside the cave (under the hill).
    pub fn in_cave(&self, x: f64, z: f64) -> bool {
        let rel = (DVec2::new(x, z) - self.cave.mouth).abs() - self.cave.hole;
        rel.x < 0.0 && rel.y < 0.0
    }

    fn patch_at(&self, x: f64, z: f64) -> Option<Cover> {
        self.painters
            .iter()
            .find_map(|paint| paint(x, z))
            .or_else(|| {
                self.patches
                    .iter()
                    .find(|(min, max, _)| x >= min.x && z >= min.y && x <= max.x && z <= max.y)
                    .map(|p| p.2)
            })
    }

    /// Linear albedo of the ground here (vertex color of the terrain mesh).
    pub fn color(&self, x: f64, z: f64, h: f64, normal: Vec3) -> Vec3 {
        self.paint(x, z, h, normal, false)
    }

    /// Color of the cave roof's top: the hillside, as if there were no cave below.
    pub fn roof_color(&self, x: f64, z: f64, h: f64, normal: Vec3) -> Vec3 {
        self.paint(x, z, h, normal, true)
    }

    fn paint(&self, x: f64, z: f64, h: f64, normal: Vec3, roofed: bool) -> Vec3 {
        let s = self.seed;
        let n1 = fbm2(x / 55.0, z / 55.0, s + 20, 4, 2.0, 0.5) * 0.5 + 0.5;
        let n2 = fbm2(x / 9.0, z / 9.0, s + 21, 3, 2.0, 0.5) * 0.5 + 0.5;
        let slope = (1.0 - normal.y).clamp(0.0, 1.0);
        let lush = Vec3::new(0.062, 0.15, 0.03);
        let dry = Vec3::new(0.16, 0.17, 0.055);
        let alpine = Vec3::new(0.11, 0.115, 0.06);
        let forest = Vec3::new(0.05, 0.07, 0.025);
        let dirt = Vec3::new(0.17, 0.12, 0.07);
        let cobble = Vec3::new(0.25, 0.23, 0.21);
        let sand = Vec3::new(0.42, 0.36, 0.24);
        let rock = Vec3::new(0.19, 0.185, 0.175);
        let rock_dark = Vec3::new(0.09, 0.085, 0.08);
        let snow = Vec3::new(0.82, 0.85, 0.9);

        let grass = lush.lerp(dry, (n1 * 0.8 + n2 * 0.25 - 0.25).clamp(0.0, 1.0));
        let altitude = smooth(250.0, 800.0, h) as f32;
        let mut c = grass.lerp(alpine, altitude);
        // Darker under the woods (the canopy above is instanced trees nearby, painted far away).
        let wood = self.woodland(x, z) as f32;
        c = c.lerp(forest, smoothstep(0.35, 0.8, wood) * (1.0 - altitude));
        // Rock on steep slopes.
        let rocky = smoothstep(0.18, 0.4, slope);
        let rock_c = rock.lerp(rock_dark, n2 * 0.7);
        c = c.lerp(rock_c, rocky);
        // Snow above the snow line, sliding off steep faces.
        let snow_h = SNOW_LINE as f32 + perlin2(x / 300.0, z / 300.0, s + 6) * 150.0;
        let snowy = smoothstep(snow_h - 60.0, snow_h + 60.0, h as f32)
            * (1.0 - smoothstep(0.35, 0.6, slope));
        c = c.lerp(snow, snowy);
        // Lake shore and bed.
        let p = DVec2::new(x, z);
        if (p - self.lake.center).length() < self.lake.radius * 1.6 {
            let beach = 1.0 - smoothstep(1.02, 1.12, self.lake_t(x, z) as f32);
            c = c.lerp(sand.lerp(dirt, n2 * 0.5), beach);
            let wet = 1.0 - smoothstep(-2.0, 0.1, (h - LAKE_LEVEL) as f32);
            c = c.lerp(Vec3::new(0.08, 0.07, 0.05), wet);
        }
        // Farm fields with a grassy margin.
        if let Some((crop, edge)) = self.field_at(x, z) {
            let shade = 0.85 + 0.3 * perlin2(x / 23.0, z / 23.0, s + 41) as f32;
            let mut fc = crop.color() * shade;
            if crop == Crop::Plowed {
                // Furrows.
                let (sn, cs) = FIELD_ANGLE.sin_cos();
                let along = (-x * sn + z * cs) as f32;
                fc *= 0.8 + 0.25 * (along * std::f32::consts::TAU / 4.0).sin();
            }
            c = c.lerp(fc, smoothstep(1.5, 3.0, edge as f32));
        }
        // Roads (they run into the cave, not over it).
        if let Some((_, _, paved, edge)) = self.road_at(p).filter(|_| !roofed) {
            let road = if paved {
                cobble * (0.8 + 0.4 * n2)
            } else {
                Vec3::new(0.24, 0.19, 0.13) * (0.85 + 0.3 * n2)
            };
            c = c.lerp(road, edge as f32);
        }
        if let Some(pc) = self.patch_at(x, z).and_then(Cover::patch_color) {
            c = pc;
        }
        // Wet stones in the stream bed.
        if self.stream_at(x, z).is_some_and(|(d, _)| d < 3.0) {
            c = Vec3::new(0.085, 0.08, 0.065).lerp(Vec3::new(0.13, 0.12, 0.1), n2);
        }
        if !roofed && self.in_cave(x, z) {
            c = Vec3::new(0.13, 0.12, 0.105).lerp(Vec3::new(0.08, 0.12, 0.12), n1);
        }
        // Small-scale brightness variation everywhere.
        c * (0.85 + 0.3 * n2)
    }

    /// What covers the ground at (x, z), from the height and slope there.
    pub fn cover_at(&self, x: f64, z: f64) -> Cover {
        let h = self.height(x, z);
        let n = self.normal(x, z, 0.5);
        self.cover(x, z, h, n)
    }
}

/// Seed of the showcase landscape. Fixed (not the engine seed) so viewpoints and screenshots
/// stay comparable between runs.
pub const LANDSCAPE_SEED: u64 = 7;

/// The showcase landscape, built once.
pub fn shared() -> &'static std::sync::Arc<Landscape> {
    static LAND: std::sync::OnceLock<std::sync::Arc<Landscape>> = std::sync::OnceLock::new();
    LAND.get_or_init(|| std::sync::Arc::new(crate::zones::landscape(LANDSCAPE_SEED)))
}

/// The landscape as the world's ground (footsteps, foliage, AI and physics demos read it).
pub struct LandscapeGround {
    pub land: std::sync::Arc<Landscape>,
    /// Footstep surface ids by [`Cover`].
    pub surface_ids: [SurfaceId; COVER_KINDS],
}

impl LandscapeGround {
    /// The ground for `land`, with surface ids from the world's material table. Registers the
    /// materials the landscape uses that the default table lacks (gravel), so the ids don't
    /// depend on which demo happens to register them first.
    pub fn new(land: std::sync::Arc<Landscape>, materials: &mut SurfaceMaterials) -> Self {
        if materials.id("gravel").is_none() {
            materials.add(gravel());
        }
        let fallback = materials.id("default").unwrap_or_default();
        let surface_ids = COVERS.map(|c| materials.id(c.surface_name()).unwrap_or(fallback));
        Self { land, surface_ids }
    }
}

/// Loose stones: firm but shifting underfoot, crunchy.
fn gravel() -> SurfaceMaterial {
    SurfaceMaterial {
        name: "gravel".into(),
        friction: 0.7,
        density: 1_800.0,
        absorbency: 0.15,
        deformation: Deformation::Granular { depth_m: 0.01 },
        footstep_sound: "footstep.gravel".into(),
        impact_effect: "impact.gravel".into(),
        ..Default::default()
    }
}

impl GroundQuery for LandscapeGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let l = &self.land;
        Some(GroundSample {
            height: l.height(p.x, p.z),
            normal: l.normal(p.x, p.z, 0.5),
            surface: self.surface_ids[l.cover_at(p.x, p.z) as usize],
        })
    }
}

/// Radii of the terrain mesh rings: 1 m apart near the plaza, then growing so cells stay
/// roughly square (cell width = radius × 2π / segments).
fn ring_radii(segments: u32) -> Vec<f64> {
    let mut radii = vec![0.0];
    let k = std::f64::consts::TAU / segments as f64;
    let mut r = 0.0;
    while r < TERRAIN_RADIUS {
        r += (r * k).max(1.0);
        radii.push(r.min(TERRAIN_RADIUS));
    }
    radii
}

/// Build the terrain as polar chunks (sectors × radial bands), each a mesh with its own local
/// origin so far chunks keep f32 precision. Returns (origin, mesh) pairs.
pub fn build_terrain(land: &Landscape) -> Vec<(DVec3, Mesh)> {
    build_terrain_within(land, TERRAIN_RADIUS)
}

/// [`build_terrain`] out to `max_radius` only (tests use a small one).
pub fn build_terrain_within(land: &Landscape, max_radius: f64) -> Vec<(DVec3, Mesh)> {
    const SEGMENTS: u32 = 512;
    const SECTORS: u32 = 16;
    let mut radii = ring_radii(SEGMENTS);
    radii.retain(|&r| r <= max_radius);
    // Band boundaries by ring index (each band a few hundred rings or less).
    let band_edges = [
        0.0,
        120.0,
        400.0,
        1000.0,
        2500.0,
        6000.0,
        TERRAIN_RADIUS + 1.0,
    ];
    let mut bands: Vec<(usize, usize)> = Vec::new();
    for w in band_edges.windows(2) {
        let Some(i0) = radii.iter().position(|&r| r >= w[0]) else {
            continue;
        };
        let i1 = radii
            .iter()
            .rposition(|&r| r <= w[1])
            .unwrap_or(radii.len() - 1);
        if i1 > i0 {
            bands.push((i0, i1));
        }
    }
    // Make bands share their boundary ring.
    for i in 1..bands.len() {
        bands[i].0 = bands[i - 1].1;
    }

    // Sample each ring vertex once (shared between chunks).
    let seg_per_sector = SEGMENTS / SECTORS;
    let mut out = Vec::new();
    for &(i0, i1) in &bands {
        for sector in 0..SECTORS {
            let j0 = sector * seg_per_sector;
            let j1 = j0 + seg_per_sector;
            let mid_r = (radii[i0] + radii[i1]) * 0.5;
            let mid_a = (j0 + j1) as f64 * 0.5 / SEGMENTS as f64 * std::f64::consts::TAU;
            let origin = DVec3::new(mid_a.cos() * mid_r, 0.0, mid_a.sin() * mid_r);
            let cols = (j1 - j0 + 1) as usize;
            let mut mesh = Mesh::default();
            for (ii, &r) in radii[i0..=i1].iter().enumerate() {
                let cell = (r * std::f64::consts::TAU / SEGMENTS as f64).max(1.0);
                for j in j0..=j1 {
                    let a = j as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
                    let (x, z) = (a.cos() * r, a.sin() * r);
                    let h = land.height(x, z);
                    let n = land.normal(x, z, (cell * 0.5).max(0.4));
                    let c = land.color(x, z, h, n);
                    let pos = (DVec3::new(x, h, z) - origin).as_vec3();
                    let _ = ii;
                    mesh.vertices.push(
                        Vertex::new(pos, n, Vec2::new((x / 6.0) as f32, (z / 6.0) as f32))
                            .with_color(c.extend(1.0)),
                    );
                }
            }
            let rows = i1 - i0 + 1;
            for row in 0..rows - 1 {
                for col in 0..cols - 1 {
                    let a = (row * cols + col) as u32;
                    let b = a + 1;
                    let c = a + cols as u32;
                    let d = c + 1;
                    // Counter-clockwise seen from above (+Y): angle grows toward +Z.
                    mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
                }
            }
            out.push((origin, mesh));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_land() -> Landscape {
        crate::zones::landscape(7)
    }

    #[test]
    fn zones_are_flat_at_their_pad_height() {
        let land = test_land();
        for z in crate::zones::ZONES.iter().filter(|z| z.key != "lake") {
            let h = land.height(z.center.x, z.center.z);
            assert!(
                (h - z.center.y).abs() < 0.05,
                "{} is at {h} m, its pad is {} m",
                z.key,
                z.center.y
            );
        }
    }

    #[test]
    fn mountains_rise_far_away_and_lake_holds_water() {
        let land = test_land();
        let peak = (0..64)
            .map(|i| {
                let a = i as f64 / 64.0 * std::f64::consts::TAU;
                land.height(a.cos() * 6000.0, a.sin() * 6000.0)
            })
            .fold(f64::MIN, f64::max);
        assert!(peak > 800.0, "highest point 6 km out is only {peak} m");
        let l = land.lake;
        assert!(land.lake_depth(l.center.x, l.center.y) > 3.0);
    }

    #[test]
    fn stream_runs_downhill_over_the_falls_into_the_lake() {
        let land = test_land();
        let st = &land.streams[0];
        assert!(st.bed.windows(2).all(|w| w[1] <= w[0]), "the bed climbs");
        let (top, bottom) = st.falls().expect("the stream has a waterfall");
        let drop = st.bed[top] - st.bed[bottom];
        assert!(drop > 5.0, "the falls drop only {drop} m");
        let end = *st.path.last().unwrap();
        assert!(
            land.lake_depth(end.x, end.y) > 0.5,
            "the stream ends in the lake"
        );
    }

    #[test]
    fn ground_reports_the_footstep_trail_surfaces() {
        let land = test_land();
        let mut table = SurfaceMaterials::default();
        let ground = LandscapeGround::new(std::sync::Arc::new(land), &mut table);
        let at = |x: f64, z: f64| {
            let s = ground.sample(DVec3::new(x, 0.0, z)).unwrap().surface;
            table.get(s).name.clone()
        };
        // The trail's first straight runs along x at z = 150 - 7: grass, mud, snow.
        assert_eq!(at(-12.0, 143.0), "grass");
        assert_eq!(at(0.0, 143.0), "mud");
        assert_eq!(at(12.0, 143.0), "snow");
        // The far bend starts with ice, then gravel (registered by the ground itself), and the
        // second straight starts with boards.
        assert_eq!(at(22.95, 145.05), "ice");
        assert_eq!(at(22.95, 154.95), "gravel");
        assert_eq!(at(14.0, 157.0), "wood");
        assert_ne!(at(0.0, 150.0), "wood", "inside the loop is not the trail");
    }

    #[test]
    fn terrain_mesh_triangles_face_up() {
        let land = test_land();
        let chunks = build_terrain_within(&land, 60.0);
        assert_eq!(chunks.len(), 16);
        for (_, mesh) in &chunks {
            let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
            for tri in mesh.indices.chunks(3) {
                let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
                // The innermost ring sits at the center, so its triangles are degenerate.
                if n.length() > 1e-6 {
                    assert!(
                        n.y > 0.0,
                        "terrain triangles must wind counter-clockwise from above"
                    );
                }
            }
        }
    }

    #[test]
    fn the_falls_drop_at_the_cliff_face() {
        // The ground keeps the lip's height right up to the rock's worn face and drops there,
        // so the water module's flow falls clear of the rock instead of sliding down a slope
        // inside it; the plunge pool stays in front of the cliff.
        let land = shared();
        let st = &land.streams[0];
        let sc = &land.scarps[0];
        let (top, bottom) = st.falls().expect("the stream falls over the cliff");
        assert!(sc.notch.is_some());
        let lip = land.height(st.path[top - 12].x, st.path[top - 12].y);
        for p in &st.path[top - 20..bottom + 8] {
            let (_, side) = sc.frame(*p, land.seed);
            let h = land.height(p.x, p.y);
            if side > 1.3 {
                assert!(
                    h > lip - 0.3,
                    "lip cut back at side {side:.2}: {h:.2} < {lip:.2}"
                );
            }
            if side < -1.0 {
                assert!(
                    h < lip - 0.75 * sc.height,
                    "no drop at side {side:.2}: {h:.2}"
                );
            }
        }
    }
}
