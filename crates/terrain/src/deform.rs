//! Footprints, paw prints, hoof marks, tire tracks and flattened grass.
//!
//! A high-resolution grid (8 cm cells, ~120 m across) follows the simulation focus. Every
//! [`GroundContact`](engine_world::GroundContact) stamps its shape into it, shaped by the
//! surface material and the weather: deep prints with pushed-up rims in mud, compacted prints
//! in snow (as deep as the snow), soft prints in sand, bent blades in grass and nothing on
//! rock. Wet soil prints deeper. Grass springs back, falling snow fills prints in and blowing
//! sand softens them. The terrain shader turns the grid into shaded relief.

use std::collections::VecDeque;

use engine_core::math::{DVec2, DVec3, Vec2};
use engine_core::prelude::*;
use engine_world::{
    ContactKind, Deformation as Deform, FollowsWorldShift, GroundContact, WorldShift,
};

/// What kind of ground a cell was pressed into (decides how it recovers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum CellKind {
    #[default]
    None = 0,
    /// Mud, wet dirt: stays.
    Plastic = 1,
    /// Snow: fills in while it snows.
    Snow = 2,
    /// Sand: slowly softens.
    Granular = 3,
    /// Grass: springs back.
    Grass = 4,
}

/// Settings for footprints and tracks.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeformationSettings {
    pub enabled: bool,
    /// Grid cell size, meters (prints need ≤ 0.1).
    pub cell_m: f32,
    /// Cells per side.
    pub size: u32,
    /// Mud/snow rim height as a share of the print depth.
    pub rim: f32,
    /// Seconds for blown sand to soften a print by half.
    pub sand_half_life_s: f32,
    /// Snow depth assumed on snow-covered terrain when the weather reports none, meters.
    pub default_snow_m: f32,
}

impl Default for DeformationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cell_m: 0.04,
            size: 2048,
            rim: 0.3,
            sand_half_life_s: 240.0,
            default_snow_m: 0.25,
        }
    }
}

/// Side of a recovery tile in cells.
pub const TILE: u32 = 32;

/// Prints remembered so they come back when the grid moves over them again.
pub const HISTORY: usize = 40_000;

/// A remembered print.
#[derive(Debug, Clone, Copy)]
pub struct StampRecord {
    pub contact: GroundContact,
    pub kind: CellKind,
    pub depth: f32,
    pub rim: f32,
    pub flatten: f32,
    /// World time it was made, seconds.
    pub time: f64,
}

/// The deformation grid around the focus.
#[derive(Resource, Debug, Clone)]
pub struct DeformationMap {
    /// World x/z of cell (0, 0)'s corner.
    pub origin: DVec2,
    pub cell: f32,
    pub size: u32,
    /// Depth pressed in, meters (negative = raised rim).
    pub depth: Vec<f32>,
    /// Grass bent flat, 0..1.
    pub flatten: Vec<f32>,
    pub kind: Vec<CellKind>,
    /// Tiles with something in them (for recovery).
    pub active: Vec<bool>,
    /// Cell rectangle changed since the last upload: (x0, y0, x1, y1) exclusive.
    pub dirty: Option<(u32, u32, u32, u32)>,
    /// Bumped when the whole grid moved (re-upload everything).
    pub generation: u64,
    /// Prints stamped since startup.
    pub stamps: u64,
    /// Recent prints (newest last), re-applied when the grid moves over them.
    pub history: VecDeque<StampRecord>,
}

impl DeformationMap {
    pub fn new(cell: f32, size: u32) -> Self {
        let n = (size * size) as usize;
        let tiles = size.div_ceil(TILE);
        Self {
            origin: DVec2::splat(-(size as f64) * cell as f64 * 0.5),
            cell,
            size,
            depth: vec![0.0; n],
            flatten: vec![0.0; n],
            kind: vec![CellKind::None; n],
            active: vec![false; (tiles * tiles) as usize],
            dirty: None,
            generation: 1,
            stamps: 0,
            history: VecDeque::new(),
        }
    }

    /// World width covered, meters.
    pub fn extent(&self) -> f64 {
        self.size as f64 * self.cell as f64
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y * self.size + x) as usize
    }

    /// Depth (m) and flattening at a world point (bilinear), zero outside.
    pub fn sample(&self, p: DVec3) -> (f32, f32) {
        let g = (DVec2::new(p.x, p.z) - self.origin) / self.cell as f64 - 0.5;
        if g.x < 0.0 || g.y < 0.0 || g.x >= (self.size - 1) as f64 || g.y >= (self.size - 1) as f64
        {
            return (0.0, 0.0);
        }
        let (x, y) = (g.x as u32, g.y as u32);
        let (fx, fy) = ((g.x - x as f64) as f32, (g.y - y as f64) as f32);
        let at = |v: &Vec<f32>, dx: u32, dy: u32| v[self.index(x + dx, y + dy)];
        let bil = |v: &Vec<f32>| {
            let a = at(v, 0, 0) + (at(v, 1, 0) - at(v, 0, 0)) * fx;
            let b = at(v, 0, 1) + (at(v, 1, 1) - at(v, 0, 1)) * fx;
            a + (b - a) * fy
        };
        (bil(&self.depth), bil(&self.flatten))
    }

    fn mark(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        self.dirty = Some(match self.dirty {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
        let tiles = self.size.div_ceil(TILE);
        for ty in y0 / TILE..=(y1.saturating_sub(1)) / TILE {
            for tx in x0 / TILE..=(x1.saturating_sub(1)) / TILE {
                if tx < tiles && ty < tiles {
                    self.active[(ty * tiles + tx) as usize] = true;
                }
            }
        }
    }

    /// Press a print and remember it (see [`DeformationMap::stamp`]); `now` is world time.
    pub fn record(
        &mut self,
        c: &GroundContact,
        kind: CellKind,
        depth: f32,
        rim: f32,
        flatten: f32,
        now: f64,
    ) {
        self.stamp(c, kind, depth, rim, flatten);
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(StampRecord {
            contact: *c,
            kind,
            depth,
            rim,
            flatten,
            time: now,
        });
    }

    /// Re-center on `focus` when it has moved more than a quarter of the extent, keeping the
    /// prints that stay inside and re-pressing remembered prints that come into view (aged by
    /// `now`: grass has sprung back, sand has softened).
    pub fn follow(&mut self, focus: DVec3, now: f64, grass_recover_s: f32, sand_half_life_s: f32) {
        if self.shift_to(focus) {
            let history = std::mem::take(&mut self.history);
            let (lo, hi) = (self.origin, self.origin + DVec2::splat(self.extent()));
            for r in &history {
                let p = r.contact.position;
                if p.x < lo.x || p.z < lo.y || p.x > hi.x || p.z > hi.y {
                    continue;
                }
                let age = (now - r.time).max(0.0) as f32;
                let flatten = (r.flatten - age / grass_recover_s.max(0.1)).max(0.0);
                let depth = match r.kind {
                    CellKind::Granular => r.depth * 0.5f32.powf(age / sand_half_life_s.max(1.0)),
                    _ => r.depth,
                };
                if depth > 1e-4 || flatten > 0.0 {
                    let stamps = self.stamps;
                    self.stamp(&r.contact, r.kind, depth, r.rim, flatten);
                    self.stamps = stamps;
                }
            }
            self.history = history;
        }
    }

    /// Keep the prints on the ground when the world re-anchors. A plain move carries the grid
    /// and the remembered prints along. A turn (another planet's frame, or the world starting
    /// or stopping to turn with its planet, both far above the ground) clears them.
    pub fn follow_shift(&mut self, s: &WorldShift) {
        if s.is_translation_only() {
            self.origin += DVec2::new(s.translation.x, s.translation.z);
            for r in &mut self.history {
                r.contact.follow(s);
            }
        } else {
            self.depth.iter_mut().for_each(|d| *d = 0.0);
            self.flatten.iter_mut().for_each(|f| *f = 0.0);
            self.kind.iter_mut().for_each(|k| *k = CellKind::None);
            self.active.iter_mut().for_each(|a| *a = false);
            self.history.clear();
        }
        self.dirty = Some((0, 0, self.size, self.size));
        self.generation += 1;
    }

    /// Clear the prints within `radius` of `center` (the ground there was dug, raised or
    /// smoothed, so they are gone) and forget them.
    pub fn erase(&mut self, center: DVec3, radius: f32) {
        let r = radius as f64 + self.cell as f64;
        let c = (DVec2::new(center.x, center.z) - self.origin) / self.cell as f64;
        let rc = r / self.cell as f64;
        let lo = (c - rc).floor().max(DVec2::ZERO);
        let hi = (c + rc).ceil().min(DVec2::splat(self.size as f64));
        if lo.x < hi.x && lo.y < hi.y {
            let (x0, y0, x1, y1) = (lo.x as u32, lo.y as u32, hi.x as u32, hi.y as u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = DVec2::new(x as f64 + 0.5, y as f64 + 0.5);
                    if p.distance(c) <= rc {
                        let i = self.index(x, y);
                        self.depth[i] = 0.0;
                        self.flatten[i] = 0.0;
                        self.kind[i] = CellKind::None;
                    }
                }
            }
            self.mark(x0, y0, x1, y1);
        }
        let at = DVec2::new(center.x, center.z);
        self.history
            .retain(|h| DVec2::new(h.contact.position.x, h.contact.position.z).distance(at) > r);
    }

    /// Move the window over `focus` if needed. Returns whether it moved.
    fn shift_to(&mut self, focus: DVec3) -> bool {
        let half = self.extent() * 0.5;
        let center = self.origin + DVec2::splat(half);
        let d = DVec2::new(focus.x, focus.z) - center;
        if d.abs().max_element() < half * 0.5 {
            return false;
        }
        let shift_x = (d.x / self.cell as f64).round() as i64;
        let shift_y = (d.y / self.cell as f64).round() as i64;
        let n = self.size as i64;
        let mut depth = vec![0.0; self.depth.len()];
        let mut flatten = vec![0.0; self.flatten.len()];
        let mut kind = vec![CellKind::None; self.kind.len()];
        for y in 0..n {
            let sy = y + shift_y;
            if sy < 0 || sy >= n {
                continue;
            }
            for x in 0..n {
                let sx = x + shift_x;
                if sx < 0 || sx >= n {
                    continue;
                }
                let (di, si) = ((y * n + x) as usize, (sy * n + sx) as usize);
                depth[di] = self.depth[si];
                flatten[di] = self.flatten[si];
                kind[di] = self.kind[si];
            }
        }
        self.depth = depth;
        self.flatten = flatten;
        self.kind = kind;
        self.origin += DVec2::new(shift_x as f64, shift_y as f64) * self.cell as f64;
        self.active.iter_mut().for_each(|a| *a = true);
        self.dirty = Some((0, 0, self.size, self.size));
        self.generation += 1;
        true
    }

    /// Press a print into the grid. `depth` meters, `rim` share of the depth pushed up around
    /// it, `flatten` 0..1 grass bending, `shape` the print's outline.
    pub fn stamp(&mut self, c: &GroundContact, kind: CellKind, depth: f32, rim: f32, flatten: f32) {
        let (w, l) = contact_size(c);
        let reach = (w.max(l) * 0.5 * if flatten > 0.0 { 1.8 } else { 1.4 }) as f64 + 0.1;
        let lo = (DVec2::new(c.position.x, c.position.z) - reach - self.origin) / self.cell as f64;
        let hi = (DVec2::new(c.position.x, c.position.z) + reach - self.origin) / self.cell as f64;
        let x0 = lo.x.floor().max(0.0) as u32;
        let y0 = lo.y.floor().max(0.0) as u32;
        let x1 = (hi.x.ceil().max(0.0) as u32).min(self.size);
        let y1 = (hi.y.ceil().max(0.0) as u32).min(self.size);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        // Print frame: `fwd` along the long axis (yaw 0 = −Z, like `Transform::forward`).
        let fwd = Vec2::new(-c.yaw.sin(), -c.yaw.cos());
        let side = Vec2::new(fwd.y, -fwd.x);
        for y in y0..y1 {
            for x in x0..x1 {
                let world = self.origin + (DVec2::new(x as f64, y as f64) + 0.5) * self.cell as f64;
                let rel = (world - DVec2::new(c.position.x, c.position.z)).as_vec2();
                let u = rel.dot(side) / (w * 0.5);
                let v = rel.dot(fwd) / (l * 0.5);
                let (inside, edge) = print_shape(c.kind, u, v);
                let i = self.index(x, y);
                if depth > 0.0 {
                    if inside > 0.0 {
                        let tread = tread(c.kind, u, v, l);
                        let d = depth * inside * (1.0 - 0.12 * tread);
                        if d > self.depth[i] {
                            self.depth[i] = d;
                        }
                    } else if edge > 0.0 && rim > 0.0 {
                        let r = -depth * rim * edge;
                        // Only raise ground that wasn't pressed already.
                        if self.depth[i] <= 0.0 && r < self.depth[i] {
                            self.depth[i] = r;
                        }
                    }
                }
                if flatten > 0.0 {
                    let f = flatten * (inside.max(edge * 0.6));
                    self.flatten[i] = self.flatten[i].max(f);
                }
                if inside > 0.0 || edge > 0.0 {
                    self.kind[i] = kind;
                }
            }
        }
        self.mark(x0, y0, x1, y1);
        self.stamps += 1;
    }

    /// Let prints recover over `dt` seconds. `snowfall_m_s`: new snow depth per second;
    /// `grass_recover_s`: seconds for bent grass to stand up again.
    pub fn recover(
        &mut self,
        dt: f32,
        snowfall_m_s: f32,
        grass_recover_s: f32,
        sand_half_life_s: f32,
    ) {
        let tiles = self.size.div_ceil(TILE);
        let sand_k = 0.5f32.powf(dt / sand_half_life_s.max(1.0));
        let grass_step = dt / grass_recover_s.max(0.1);
        let snow_step = snowfall_m_s * dt;
        for ty in 0..tiles {
            for tx in 0..tiles {
                let t = (ty * tiles + tx) as usize;
                if !self.active[t] {
                    continue;
                }
                let mut any = false;
                let mut changed = false;
                let (x0, y0) = (tx * TILE, ty * TILE);
                let (x1, y1) = ((x0 + TILE).min(self.size), (y0 + TILE).min(self.size));
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = self.index(x, y);
                        if self.flatten[i] > 0.0 {
                            self.flatten[i] = (self.flatten[i] - grass_step).max(0.0);
                            changed = true;
                        }
                        let d = self.depth[i];
                        if d != 0.0 {
                            let nd = match self.kind[i] {
                                CellKind::Snow if snow_step > 0.0 => {
                                    if d > 0.0 {
                                        (d - snow_step).max(0.0)
                                    } else {
                                        (d + snow_step).min(0.0)
                                    }
                                }
                                CellKind::Granular => {
                                    let v = d * sand_k;
                                    if v.abs() < 1e-4 { 0.0 } else { v }
                                }
                                _ => d,
                            };
                            if nd != d {
                                self.depth[i] = nd;
                                changed = true;
                            }
                        }
                        any |= self.depth[i] != 0.0 || self.flatten[i] > 0.0;
                    }
                }
                self.active[t] = any;
                if changed {
                    self.mark(x0, y0, x1, y1);
                }
            }
        }
    }
}

/// (width, length) of a contact with sensible defaults per kind.
fn contact_size(c: &GroundContact) -> (f32, f32) {
    let (dw, dl) = match c.kind {
        ContactKind::Foot => (0.11, 0.29),
        ContactKind::Paw => (0.09, 0.10),
        ContactKind::Hoof => (0.12, 0.13),
        ContactKind::Wheel => (0.22, 0.30),
        ContactKind::Track => (0.5, 0.5),
        ContactKind::Body => (0.5, 1.6),
    };
    let w = if c.size[0] > 0.0 { c.size[0] } else { dw };
    let l = if c.size[1] > 0.0 { c.size[1] } else { dl };
    (w, l)
}

fn ellipse(u: f32, v: f32, cu: f32, cv: f32, ru: f32, rv: f32) -> f32 {
    let d = ((u - cu) / ru).powi(2) + ((v - cv) / rv).powi(2);
    d.sqrt()
}

/// (inside 0..1 with soft walls, rim weight 0..1 just outside) in print coordinates where
/// the print spans u, v ∈ [−1, 1] (v forward).
fn print_shape(kind: ContactKind, u: f32, v: f32) -> (f32, f32) {
    let d = match kind {
        ContactKind::Foot => {
            // Heel and forefoot pads joined by a narrower arch.
            let heel = ellipse(u, v, 0.0, -0.62, 0.78, 0.4);
            let fore = ellipse(u, v, 0.08, 0.32, 0.95, 0.68);
            let arch = ellipse(u, v, 0.18, -0.15, 0.55, 0.45);
            heel.min(fore).min(arch)
        }
        ContactKind::Paw => {
            let pad = ellipse(u, v, 0.0, -0.35, 0.6, 0.45);
            let mut toes = f32::MAX;
            for (cu, cv) in [(-0.62, 0.35), (-0.22, 0.62), (0.22, 0.62), (0.62, 0.35)] {
                toes = toes.min(ellipse(u, v, cu, cv, 0.24, 0.26));
            }
            pad.min(toes)
        }
        ContactKind::Hoof => {
            // A U-shaped wall with the frog notch at the back.
            let outer = ellipse(u, v, 0.0, 0.0, 1.0, 1.0);
            let notch = ellipse(u, v, 0.0, -0.75, 0.3, 0.55);
            if notch < 1.0 {
                1.0 + (1.0 - notch) * 0.5
            } else {
                outer
            }
        }
        ContactKind::Wheel | ContactKind::Track => {
            // Rounded rectangle (superellipse).
            (u.abs().powi(6) + v.abs().powi(6)).powf(1.0 / 6.0)
        }
        ContactKind::Body => ellipse(u, v, 0.0, 0.0, 1.0, 1.0),
    };
    let inside = 1.0 - smooth(0.82, 1.0, d);
    let edge = if d >= 1.0 {
        1.0 - smooth(1.0, 1.45, d)
    } else {
        0.0
    };
    (inside, edge * smooth(0.9, 1.08, d))
}

/// Tread pattern 0..1 (sole lugs, tire chevrons).
fn tread(kind: ContactKind, u: f32, v: f32, length: f32) -> f32 {
    match kind {
        ContactKind::Foot => {
            let lug = ((v * 9.0).sin() * (u * 4.0 + v * 2.0).cos()).abs();
            smooth(0.4, 0.7, lug)
        }
        ContactKind::Wheel => {
            // Chevrons every ~5 cm along the contact.
            let p = v * length * 20.0 + u.abs() * 2.5;
            smooth(0.3, 0.6, (p * std::f32::consts::PI).sin().abs())
        }
        ContactKind::Track => {
            let p = v * length * 6.0;
            smooth(0.6, 0.8, (p * std::f32::consts::PI).sin().abs())
        }
        _ => 0.0,
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    engine_core::math::smoothstep(e0, e1, x)
}

/// How a contact deforms ground, given the surface's deformation and conditions.
/// Returns (kind, depth m, rim share, grass flatten).
pub fn response(
    surface: Deform,
    load_kg: f32,
    wetness: f32,
    snow_m: f32,
    settings: &DeformationSettings,
) -> (CellKind, f32, f32, f32) {
    let pressure = (load_kg.max(1.0) / 75.0).sqrt().clamp(0.25, 4.0);
    // Snow on top of anything takes the print.
    if snow_m > 0.02 {
        let depth = (0.12 * pressure).min(snow_m * 0.9);
        return (CellKind::Snow, depth, settings.rim * 0.4, 0.0);
    }
    match surface {
        Deform::None => (CellKind::None, 0.0, 0.0, 0.0),
        Deform::Plastic { depth_m } => {
            // Wet soil is softer: dry dirt barely marks, soaked dirt prints like mud.
            let soft = 0.6 + 2.4 * wetness.clamp(0.0, 1.0);
            let depth = (depth_m * pressure * soft).min(0.25);
            (CellKind::Plastic, depth, settings.rim, 0.0)
        }
        Deform::Snow => {
            let depth = (0.12 * pressure).min(settings.default_snow_m * 0.9);
            (CellKind::Snow, depth, settings.rim * 0.4, 0.0)
        }
        Deform::Granular { depth_m } => (
            CellKind::Granular,
            (depth_m * pressure).min(0.2),
            settings.rim * 1.2,
            0.0,
        ),
        Deform::Flatten { .. } => {
            // Grass bends; wet ground under it takes a faint print too.
            let depth = 0.004 * pressure + 0.02 * wetness * pressure;
            (CellKind::Grass, depth, 0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(x: f64, z: f64, kind: ContactKind) -> GroundContact {
        GroundContact {
            position: DVec3::new(x, 0.0, z),
            normal: engine_core::math::Vec3::Y,
            size: [0.0, 0.0],
            yaw: 0.0,
            load_kg: 80.0,
            speed: 1.4,
            kind,
            surface: None,
            entity: None,
        }
    }

    #[test]
    fn mud_print_has_depth_and_rim() {
        let mut map = DeformationMap::new(0.04, 256);
        let s = DeformationSettings::default();
        let (k, d, rim, f) = response(Deform::Plastic { depth_m: 0.06 }, 80.0, 0.3, 0.0, &s);
        map.stamp(&contact(0.0, 0.0, ContactKind::Foot), k, d, rim, f);
        let (center, _) = map.sample(DVec3::new(0.0, 0.0, 0.08));
        assert!(center > 0.03, "{center}");
        // Just beside the foot the ground is pushed up.
        let (beside, _) = map.sample(DVec3::new(0.075, 0.0, 0.08));
        assert!(beside < 0.0, "{beside}");
        assert!(map.dirty.is_some());
    }

    #[test]
    fn prints_stay_on_the_ground_when_the_world_moves() {
        let mut map = DeformationMap::new(0.04, 256);
        let s = DeformationSettings::default();
        let (k, d, rim, f) = response(Deform::Plastic { depth_m: 0.06 }, 80.0, 0.3, 0.0, &s);
        let c = contact(0.0, 0.0, ContactKind::Foot);
        map.record(&c, k, d, rim, f, 0.0);
        let before = map.sample(DVec3::new(0.0, 0.0, 0.08)).0;
        // A floating-origin move of 1.5 km east and 3 m down: the print moves with the ground.
        let shift = WorldShift::translate(DVec3::new(1_500.0, -3.0, 0.0));
        map.follow_shift(&shift);
        let after = map.sample(DVec3::new(1_500.0, -3.0, 0.08)).0;
        assert_eq!(before, after);
        assert_eq!(map.history[0].contact.position.x, 1_500.0);
        // A turn (another planet's frame) clears the grid.
        let turn = WorldShift {
            rotation: engine_core::math::DQuat::from_rotation_x(0.3),
            ..WorldShift::IDENTITY
        };
        map.follow_shift(&turn);
        assert!(map.depth.iter().all(|&d| d == 0.0) && map.history.is_empty());
    }

    #[test]
    fn snow_print_limited_by_snow_and_fills_in() {
        let mut map = DeformationMap::new(0.04, 256);
        let s = DeformationSettings::default();
        let (k, d, rim, f) = response(Deform::Plastic { depth_m: 0.06 }, 80.0, 0.0, 0.05, &s);
        assert_eq!(k, CellKind::Snow);
        assert!(d <= 0.05);
        map.stamp(&contact(0.0, 0.0, ContactKind::Foot), k, d, rim, f);
        let before = map.sample(DVec3::new(0.0, 0.0, 0.1)).0;
        map.recover(10.0, 0.002, 20.0, 240.0);
        let after = map.sample(DVec3::new(0.0, 0.0, 0.1)).0;
        assert!(after < before, "{before} -> {after}");
    }

    #[test]
    fn grass_springs_back_and_rock_takes_nothing() {
        let mut map = DeformationMap::new(0.04, 256);
        let s = DeformationSettings::default();
        let (k, d, rim, f) = response(Deform::Flatten { recover_secs: 20.0 }, 80.0, 0.0, 0.0, &s);
        map.stamp(&contact(1.0, 1.0, ContactKind::Paw), k, d, rim, f);
        assert!(map.sample(DVec3::new(1.0, 0.0, 1.0)).1 > 0.5);
        for _ in 0..30 {
            map.recover(1.0, 0.0, 20.0, 240.0);
        }
        assert_eq!(map.sample(DVec3::new(1.0, 0.0, 1.0)).1, 0.0);
        let (_, d, _, f) = response(Deform::None, 80.0, 1.0, 0.0, &s);
        assert_eq!((d, f), (0.0, 0.0));
    }

    /// Deepest point within 10 cm of (x, z).
    fn peak(map: &DeformationMap, x: f64, z: f64) -> f32 {
        let mut best = 0.0f32;
        for j in -5..=5 {
            for i in -5..=5 {
                let p = DVec3::new(x + i as f64 * 0.02, 0.0, z + j as f64 * 0.02);
                best = best.max(map.sample(p).0);
            }
        }
        best
    }

    #[test]
    fn edits_wipe_the_prints_under_them() {
        let mut map = DeformationMap::new(0.05, 256);
        for x in [0.0, 3.0] {
            let c = contact(x, 0.0, ContactKind::Foot);
            map.record(&c, CellKind::Plastic, 0.03, 0.005, 0.0, 0.0);
        }
        assert!(peak(&map, 0.0, 0.0) > 0.01 && peak(&map, 3.0, 0.0) > 0.01);
        map.dirty = None;
        map.erase(DVec3::new(0.0, 0.0, 0.0), 1.0);
        assert_eq!(peak(&map, 0.0, 0.0), 0.0);
        assert!(peak(&map, 3.0, 0.0) > 0.01, "prints outside stay");
        assert!(map.dirty.is_some());
        assert_eq!(map.history.len(), 1);
    }

    #[test]
    fn grid_follows_focus_and_keeps_prints() {
        let mut map = DeformationMap::new(0.04, 256);
        map.stamp(
            &contact(3.0, 0.0, ContactKind::Hoof),
            CellKind::Plastic,
            0.05,
            0.3,
            0.0,
        );
        let before = peak(&map, 3.0, 0.0);
        assert!(before > 0.03, "{before}");
        map.follow(DVec3::new(4.0, 0.0, 0.0), 0.0, 20.0, 240.0);
        let after = peak(&map, 3.0, 0.0);
        assert!((before - after).abs() < 1e-3, "{before} vs {after}");
    }

    #[test]
    fn remembered_prints_come_back() {
        let mut map = DeformationMap::new(0.04, 256);
        map.record(
            &contact(3.0, 0.0, ContactKind::Foot),
            CellKind::Plastic,
            0.05,
            0.3,
            0.0,
            0.0,
        );
        assert!(peak(&map, 3.0, 0.0) > 0.03);
        // Walk far away (the print leaves the window), then come back.
        map.follow(DVec3::new(500.0, 0.0, 0.0), 1.0, 20.0, 240.0);
        assert_eq!(peak(&map, 3.0, 0.0), 0.0);
        map.follow(DVec3::new(0.0, 0.0, 0.0), 2.0, 20.0, 240.0);
        assert!(peak(&map, 3.0, 0.0) > 0.03);
    }
}
