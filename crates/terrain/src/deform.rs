//! Footprints, paw prints, hoof marks, tire tracks and flattened grass.
//!
//! A high-resolution grid (4 cm cells, ~80 m across) follows the simulation focus. Every
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

/// Prints the shader draws sharper than the grid (a ring the cells point into).
pub const SLOTS: usize = 16_384;

/// Farthest a wheel's print is joined to its last one, meters.
const ROLL_GAP_M: f64 = 3.0;
/// Wheels remembered for joining ruts.
const ROLLING: usize = 64;

/// A print's shape, for the shader to draw its outline and tread at pixel scale (the grid's
/// 4 cm cells only carry its depth: a sole, a lug or a tire's chevrons are finer than that).
#[derive(Debug, Clone, Copy)]
pub struct PrintSlot {
    /// World x/z of the print's center.
    pub center: DVec2,
    /// Along the print (yaw 0 = −Z).
    pub forward: Vec2,
    /// Half width and half length, meters.
    pub half: Vec2,
    pub contact: ContactKind,
    pub kind: CellKind,
    /// Depth pressed, meters, and the rim's share of it.
    pub depth: f32,
    pub rim: f32,
    /// The grid's depth at the center right after pressing: the shader scales the print by
    /// how much of that is left (sand softening, snow filling in, ground dug away).
    pub center_depth: f32,
}

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
///
/// The window's cells are stored with wraparound addressing, like a geometry clipmap's
/// (Asirvatham and Hoppe, GPU Gems 2, ch. 2): window cell (x, y) lives at storage cell
/// ((x + base.x) mod size, (y + base.y) mod size). Moving the window only moves `base` and
/// clears the strips that come into view, instead of copying every cell and sending the whole
/// grid to the GPU again. The arrays and the tile flags are indexed by storage cell; the
/// methods take window cells.
#[derive(Resource, Debug, Clone)]
pub struct DeformationMap {
    /// World x/z of window cell (0, 0)'s corner.
    pub origin: DVec2,
    pub cell: f32,
    pub size: u32,
    /// Storage cell of window cell (0, 0).
    pub base: [u32; 2],
    /// Depth pressed in, meters (negative = raised rim).
    pub depth: Vec<f32>,
    /// Grass bent flat, 0..1.
    pub flatten: Vec<f32>,
    pub kind: Vec<CellKind>,
    /// Storage tiles with depth or bent grass in them (for recovery).
    pub active: Vec<bool>,
    /// Storage tiles that may hold anything (cleared when the window leaves them).
    touched: Vec<bool>,
    /// Storage tiles changed since the last upload.
    dirty: Vec<bool>,
    /// This grid's number: a new grid (other settings) starts from a clean texture.
    pub epoch: u64,
    /// Bumped when the window moved (print shapes are kept relative to its corner).
    pub generation: u64,
    /// Prints stamped since startup.
    pub stamps: u64,
    /// Recent prints (newest last), re-applied when the grid moves over them.
    pub history: VecDeque<StampRecord>,
    /// The two newest prints reaching each cell (slot + 1, 0 = none).
    pub prints: Vec<[u16; 2]>,
    /// Print shapes the cells point into (up to [`SLOTS`]).
    pub slots: Vec<PrintSlot>,
    /// Next slot to fill.
    pub next_slot: usize,
    /// Slots changed since the last upload (first, last exclusive).
    pub slots_dirty: Option<(usize, usize)>,
    /// Where wheels last pressed (whose, world x/z), to join their prints into ruts.
    rolling: Vec<(Option<Entity>, DVec2)>,
}

impl DeformationMap {
    pub fn new(cell: f32, size: u32) -> Self {
        static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = (size * size) as usize;
        let tiles = (size.div_ceil(TILE) * size.div_ceil(TILE)) as usize;
        Self {
            origin: DVec2::splat(-(size as f64) * cell as f64 * 0.5),
            cell,
            size,
            base: [0, 0],
            depth: vec![0.0; n],
            flatten: vec![0.0; n],
            kind: vec![CellKind::None; n],
            active: vec![false; tiles],
            touched: vec![false; tiles],
            dirty: vec![false; tiles],
            epoch: EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            generation: 1,
            stamps: 0,
            history: VecDeque::new(),
            prints: vec![[0; 2]; n],
            slots: Vec::new(),
            next_slot: 0,
            slots_dirty: None,
            rolling: Vec::new(),
        }
    }

    /// World width covered, meters.
    pub fn extent(&self) -> f64 {
        self.size as f64 * self.cell as f64
    }

    /// Storage index of window cell (x, y).
    fn index(&self, x: u32, y: u32) -> usize {
        let wrap = |v: u32, b: u32| {
            let s = v + b;
            if s >= self.size { s - self.size } else { s }
        };
        (wrap(y, self.base[1]) * self.size + wrap(x, self.base[0])) as usize
    }

    /// Storage ranges (at most two) of window cells `a..b` along an axis whose base is `base`.
    fn storage_ranges(&self, a: u32, b: u32, base: u32) -> impl Iterator<Item = (u32, u32)> {
        let n = self.size;
        let s = (a + base) % n;
        let len = b - a;
        let first = (s, (s + len).min(n));
        let second = (s + len > n).then_some((0, s + len - n));
        std::iter::once(first).chain(second).filter(|(a, b)| a < b)
    }

    /// Storage rectangles of the window rectangle (x0, y0)..(x1, y1).
    fn storage_rects(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<[u32; 4]> {
        let mut out = Vec::with_capacity(4);
        for (sy0, sy1) in self.storage_ranges(y0, y1, self.base[1]) {
            for (sx0, sx1) in self.storage_ranges(x0, x1, self.base[0]) {
                out.push([sx0, sy0, sx1, sy1]);
            }
        }
        out
    }

    fn tiles_per_side(&self) -> u32 {
        self.size.div_ceil(TILE)
    }

    /// Whether anything changed since the last [`take_dirty`](Self::take_dirty).
    pub fn is_dirty(&self) -> bool {
        self.dirty.iter().any(|&d| d)
    }

    /// The storage rectangles (x0, y0, x1, y1, exclusive) changed since the last call: runs of
    /// changed tiles along each row of tiles.
    pub fn take_dirty(&mut self) -> Vec<[u32; 4]> {
        let tiles = self.tiles_per_side();
        let mut out = Vec::new();
        for ty in 0..tiles {
            let mut tx = 0;
            while tx < tiles {
                if !self.dirty[(ty * tiles + tx) as usize] {
                    tx += 1;
                    continue;
                }
                let start = tx;
                while tx < tiles && self.dirty[(ty * tiles + tx) as usize] {
                    self.dirty[(ty * tiles + tx) as usize] = false;
                    tx += 1;
                }
                out.push([
                    start * TILE,
                    ty * TILE,
                    (tx * TILE).min(self.size),
                    ((ty + 1) * TILE).min(self.size),
                ]);
            }
        }
        out
    }

    /// Mark every tile that may hold something as changed (a new texture must get them all).
    pub fn mark_all_dirty(&mut self) {
        for (d, &t) in self.dirty.iter_mut().zip(&self.touched) {
            *d |= t;
        }
    }

    /// Clear the window rectangle (x0, y0)..(x1, y1): only the tiles that hold something.
    fn clear(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        let tiles = self.tiles_per_side();
        let n = self.size;
        for [sx0, sy0, sx1, sy1] in self.storage_rects(x0, y0, x1, y1) {
            for ty in sy0 / TILE..sy1.div_ceil(TILE) {
                for tx in sx0 / TILE..sx1.div_ceil(TILE) {
                    let t = (ty * tiles + tx) as usize;
                    if !self.touched[t] {
                        continue;
                    }
                    let (cx0, cy0) = ((tx * TILE).max(sx0), (ty * TILE).max(sy0));
                    let (cx1, cy1) = (((tx + 1) * TILE).min(sx1), ((ty + 1) * TILE).min(sy1));
                    for y in cy0..cy1 {
                        let row = (y * n) as usize;
                        let (a, b) = (row + cx0 as usize, row + cx1 as usize);
                        self.depth[a..b].fill(0.0);
                        self.flatten[a..b].fill(0.0);
                        self.kind[a..b].fill(CellKind::None);
                        self.prints[a..b].fill([0; 2]);
                    }
                    self.dirty[t] = true;
                    // The whole tile is empty now.
                    let whole = cx0 == tx * TILE
                        && cy0 == ty * TILE
                        && cx1 == ((tx + 1) * TILE).min(n)
                        && cy1 == ((ty + 1) * TILE).min(n);
                    if whole {
                        self.touched[t] = false;
                        self.active[t] = false;
                    }
                }
            }
        }
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

    /// The window rectangle (x0, y0)..(x1, y1) changed: its tiles hold something now.
    fn mark(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        let tiles = self.tiles_per_side();
        for [sx0, sy0, sx1, sy1] in self.storage_rects(x0, y0, x1, y1) {
            for ty in sy0 / TILE..sy1.div_ceil(TILE) {
                for tx in sx0 / TILE..sx1.div_ceil(TILE) {
                    let t = (ty * tiles + tx) as usize;
                    self.active[t] = true;
                    self.touched[t] = true;
                    self.dirty[t] = true;
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
        // Farthest a print reaches from its middle, meters (a lying body's rim).
        const REACH_M: f64 = 2.0;
        let kept = (
            self.origin + REACH_M,
            self.origin + DVec2::splat(self.extent() - REACH_M),
        );
        if let Some(cleared) = self.shift_to(focus) {
            let history = std::mem::take(&mut self.history);
            let (lo, hi) = (self.origin, self.origin + DVec2::splat(self.extent()));
            for r in &history {
                let p = r.contact.position;
                if p.x < lo.x || p.z < lo.y || p.x > hi.x || p.z > hi.y {
                    continue;
                }
                // Prints wholly inside the old window are still there (and may have recovered
                // since: pressing them again would undo that).
                let inside_old =
                    p.x > kept.0.x && p.z > kept.0.y && p.x < kept.1.x && p.z < kept.1.y;
                if !cleared && inside_old {
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
            // The cells and the print shapes (kept relative to the window's corner) stay as
            // they are: only where the window is changes.
            let t = DVec2::new(s.translation.x, s.translation.z);
            self.origin += t;
            for r in &mut self.history {
                r.contact.follow(s);
            }
            for p in &mut self.slots {
                p.center += t;
            }
            for r in &mut self.rolling {
                r.1 += t;
            }
        } else {
            self.clear(0, 0, self.size, self.size);
            self.history.clear();
            self.rolling.clear();
            self.generation += 1;
        }
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
                        self.prints[i] = [0; 2];
                    }
                }
            }
            self.mark(x0, y0, x1, y1);
        }
        let at = DVec2::new(center.x, center.z);
        self.history
            .retain(|h| DVec2::new(h.contact.position.x, h.contact.position.z).distance(at) > r);
    }

    /// Move the window over `focus` if needed: `None` if it stayed, else whether it jumped so
    /// far that nothing was kept.
    fn shift_to(&mut self, focus: DVec3) -> Option<bool> {
        let half = self.extent() * 0.5;
        let center = self.origin + DVec2::splat(half);
        let d = DVec2::new(focus.x, focus.z) - center;
        if d.abs().max_element() < half * 0.5 {
            return None;
        }
        let shift_x = (d.x / self.cell as f64).round() as i64;
        let shift_y = (d.y / self.cell as f64).round() as i64;
        let n = self.size as i64;
        self.origin += DVec2::new(shift_x as f64, shift_y as f64) * self.cell as f64;
        self.generation += 1;
        if shift_x.abs() >= n || shift_y.abs() >= n {
            self.clear(0, 0, self.size, self.size);
            return Some(true);
        }
        // Window cell (x, y) is old window cell (x + shift): only the base moves.
        self.base = [
            (self.base[0] as i64 + shift_x).rem_euclid(n) as u32,
            (self.base[1] as i64 + shift_y).rem_euclid(n) as u32,
        ];
        // The cells that came into view hold what left it.
        let (size, sx, sy) = (
            self.size,
            shift_x.unsigned_abs() as u32,
            shift_y.unsigned_abs() as u32,
        );
        match shift_x.signum() {
            1 => self.clear(size - sx, 0, size, size),
            -1 => self.clear(0, 0, sx, size),
            _ => {}
        }
        match shift_y.signum() {
            1 => self.clear(0, size - sy, size, size),
            -1 => self.clear(0, 0, size, sy),
            _ => {}
        }
        Some(false)
    }

    /// Press a print into the grid. `depth` meters, `rim` share of the depth pushed up around
    /// it, `flatten` 0..1 grass bending, `shape` the print's outline.
    pub fn stamp(&mut self, c: &GroundContact, kind: CellKind, depth: f32, rim: f32, flatten: f32) {
        let (w, l) = contact_size(c);
        // A rolling wheel touches the ground all the way from its last print: stretch this one
        // back to it, so the rut is unbroken however far it rolled between frames.
        let (rolled, l) = self.roll_back(c, w, l);
        let c = &rolled;
        let reach = (w.max(l) * 0.5 * if flatten > 0.0 { 1.8 } else { RIM_END + 0.1 }) as f64 + 0.1;
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
        // Pressed prints get a slot the shader draws sharp; cells within a cell of its rim
        // point at it. (Bent grass is drawn by the grass itself.)
        let slot = (depth > 0.0 && kind != CellKind::Grass).then(|| {
            let s = self.next_slot;
            self.next_slot = (s + 1) % SLOTS;
            let p = PrintSlot {
                center: DVec2::new(c.position.x, c.position.z),
                forward: fwd,
                half: Vec2::new(w, l) * 0.5,
                contact: c.kind,
                kind,
                depth,
                rim,
                center_depth: 0.0,
            };
            if s < self.slots.len() {
                self.slots[s] = p;
            } else {
                self.slots.push(p);
            }
            s
        });
        // How far (in the shape's units) a cell's center can be from the farthest point of
        // the cell.
        let margin = self.cell * 0.71 * 2.5 / (w.min(l) * 0.5);
        for y in y0..y1 {
            for x in x0..x1 {
                let world = self.origin + (DVec2::new(x as f64, y as f64) + 0.5) * self.cell as f64;
                let rel = (world - DVec2::new(c.position.x, c.position.z)).as_vec2();
                let u = rel.dot(side) / (w * 0.5);
                let v = rel.dot(fwd) / (l * 0.5);
                let d = shape_distance(c.kind, u, v);
                let (inside, edge) = shape_weights(d);
                let i = self.index(x, y);
                if let Some(s) = slot
                    && d - margin < RIM_END
                {
                    let id = s as u16 + 1;
                    let p = &mut self.prints[i];
                    if p[0] != id {
                        *p = [id, p[0]];
                    }
                }
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
        if let Some(s) = slot {
            self.slots[s].center_depth = self.sample(c.position).0;
            self.slots_dirty = Some(match self.slots_dirty {
                None => (s, s + 1),
                Some((a, b)) => (a.min(s), b.max(s + 1)),
            });
        }
    }

    /// For a wheel or track: this contact stretched back along its way to the same wheel's
    /// last print (the nearest one of the same entity behind it, in line with it, within
    /// [`ROLL_GAP_M`]), and its new length. Anything else stays as it is.
    fn roll_back(&mut self, c: &GroundContact, w: f32, l: f32) -> (GroundContact, f32) {
        if !matches!(c.kind, ContactKind::Wheel | ContactKind::Track) {
            return (*c, l);
        }
        let at = DVec2::new(c.position.x, c.position.z);
        let fwd = DVec2::new(-(c.yaw as f64).sin(), -(c.yaw as f64).cos());
        let mut best: Option<(usize, f64)> = None;
        for (i, r) in self.rolling.iter().enumerate() {
            if r.0 != c.entity {
                continue;
            }
            let back = at - r.1;
            let along = back.dot(fwd);
            let across = (back - fwd * along).length();
            if along > 0.0
                && along <= ROLL_GAP_M
                && across < w as f64 * 0.5
                && best.is_none_or(|b| along < b.1)
            {
                best = Some((i, along));
            }
        }
        let out = match best {
            Some((i, along)) => {
                self.rolling.swap_remove(i);
                let mut s = *c;
                s.position.x -= fwd.x * along * 0.5;
                s.position.z -= fwd.y * along * 0.5;
                (s, l + along as f32)
            }
            None => (*c, l),
        };
        if self.rolling.len() >= ROLLING {
            self.rolling.remove(0);
        }
        self.rolling.push((c.entity, at));
        out
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
        let tiles = self.tiles_per_side();
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
                // (Storage cells: recovery doesn't care where they are in the window.)
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * self.size + x) as usize;
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
                self.dirty[t] |= changed;
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

/// Where the rim ends, in [`shape_distance`] units.
const RIM_END: f32 = 1.7;

/// Distance-like measure from a print's middle in print coordinates where the print spans
/// u, v ∈ [−1, 1] (v forward): below 1 inside, 1 at its wall. Matches `print_distance` in
/// terrain.wgsl.
fn shape_distance(kind: ContactKind, u: f32, v: f32) -> f32 {
    match kind {
        ContactKind::Foot => {
            // A sole: the forefoot widest, the heel a little narrower, joined by a waist.
            let heel = ellipse(u, v, 0.0, -0.6, 0.82, 0.42);
            let fore = ellipse(u, v, 0.05, 0.35, 1.0, 0.64);
            let arch = ellipse(u, v, 0.08, -0.12, 0.74, 0.52);
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
    }
}

/// (inside 0..1 with soft walls, rim weight 0..1 just outside) at a [`shape_distance`].
fn shape_weights(d: f32) -> (f32, f32) {
    let inside = 1.0 - smooth(0.82, 1.0, d);
    let edge = if d >= 1.0 {
        1.0 - smooth(1.0, RIM_END, d)
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
        assert!(map.is_dirty());
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
        map.take_dirty();
        map.erase(DVec3::new(0.0, 0.0, 0.0), 1.0);
        assert_eq!(peak(&map, 0.0, 0.0), 0.0);
        assert!(peak(&map, 3.0, 0.0) > 0.01, "prints outside stay");
        assert!(map.is_dirty());
        assert_eq!(map.history.len(), 1);
    }

    /// Slots (+1) pointed at by the cell under a world point.
    fn ids_at(map: &DeformationMap, x: f64, z: f64) -> [u16; 2] {
        let g = (DVec2::new(x, z) - map.origin) / map.cell as f64;
        map.prints[map.index(g.x as u32, g.y as u32)]
    }

    #[test]
    fn cells_point_at_the_prints_that_reach_them() {
        let mut map = DeformationMap::new(0.04, 256);
        let s = DeformationSettings::default();
        let (k, d, rim, f) = response(Deform::Plastic { depth_m: 0.01 }, 80.0, 0.0, 0.0, &s);
        map.stamp(&contact(0.0, 0.0, ContactKind::Foot), k, d, rim, f);
        map.stamp(&contact(0.2, 0.0, ContactKind::Foot), k, d, rim, f);
        assert_eq!(ids_at(&map, 0.0, 0.05), [1, 0]);
        assert_eq!(ids_at(&map, 0.2, 0.05), [2, 0]);
        assert_eq!(ids_at(&map, 1.0, 0.0), [0, 0], "far from both");
        // Between the feet both rims reach: the newest comes first.
        assert_eq!(ids_at(&map, 0.1, 0.0), [2, 1]);
        let p = map.slots[0];
        assert!(p.center_depth > 0.005 && p.depth == d && p.half.y > 0.1);
        assert_eq!(map.slots_dirty, Some((0, 2)));
        // Grass is drawn by the grass: no slot.
        map.stamp(
            &contact(3.0, 0.0, ContactKind::Foot),
            CellKind::Grass,
            0.004,
            0.0,
            1.0,
        );
        assert_eq!(ids_at(&map, 3.0, 0.05), [0, 0]);
        // Moving the grid carries the pointers along; digging clears them.
        let before = ids_at(&map, 0.0, 0.05);
        map.follow(DVec3::new(4.0, 0.0, 0.0), 0.0, 20.0, 240.0);
        assert_eq!(ids_at(&map, 0.0, 0.05), before);
        map.erase(DVec3::ZERO, 0.3);
        assert_eq!(ids_at(&map, 0.0, 0.05), [0, 0]);
    }

    #[test]
    fn a_rolling_wheel_leaves_an_unbroken_rut() {
        let mut map = DeformationMap::new(0.04, 256);
        // A wheel rolling toward −Z (yaw 0), pressing every 0.8 m: longer than its 0.3 m print.
        for i in 0..5 {
            let mut c = contact(0.0, 2.0 - i as f64 * 0.8, ContactKind::Wheel);
            c.size = [0.2, 0.3];
            map.stamp(&c, CellKind::Plastic, 0.05, 0.3, 0.0);
        }
        // Between the presses the rut is as deep as under them.
        for z in [1.6, 0.8, 0.0, -0.8] {
            assert!(
                peak(&map, 0.0, z) > 0.04,
                "gap at z {z}: {}",
                peak(&map, 0.0, z)
            );
        }
        // A wheel beside it is another wheel: no rut across.
        let mut c = contact(1.0, -2.0, ContactKind::Wheel);
        c.size = [0.2, 0.3];
        map.stamp(&c, CellKind::Plastic, 0.05, 0.3, 0.0);
        assert!(peak(&map, 1.0, -1.0) < 0.01);
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

    /// The window wraps around in its arrays: walking far keeps the prints in view where they
    /// were, clears what it leaves, and marks every changed cell for the GPU's copy (kept in
    /// step here by applying only the changed rectangles), a few tiles per move rather than the
    /// whole grid.
    #[test]
    fn the_window_wraps_around_and_sends_only_what_changed() {
        let mut map = DeformationMap::new(0.04, 256);
        let mut gpu = vec![0.0f32; map.depth.len()];
        let mut sync = |map: &mut DeformationMap| {
            let mut sent = 0;
            for [x0, y0, x1, y1] in map.take_dirty() {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * map.size + x) as usize;
                        gpu[i] = map.depth[i];
                        sent += 1;
                    }
                }
            }
            assert_eq!(gpu, map.depth, "the GPU's copy fell behind");
            sent
        };
        assert_eq!(sync(&mut map), 0, "an empty grid sends nothing");
        // A walk east, then north, then back west, pressing (unremembered) prints.
        let mut at = DVec2::ZERO;
        let mut pressed = Vec::new();
        for (step, dir) in [(80, DVec2::X), (60, DVec2::Y), (90, -DVec2::X)] {
            for _ in 0..step {
                at += dir * 0.25;
                map.follow(DVec3::new(at.x, 0.0, at.y), 0.0, 20.0, 240.0);
                map.stamp(
                    &contact(at.x, at.y, ContactKind::Hoof),
                    CellKind::Plastic,
                    0.05,
                    0.3,
                    0.0,
                );
                pressed.push(at);
                let sent = sync(&mut map);
                assert!(sent <= 256 * 256 / 4, "{sent} cells sent for one move");
            }
        }
        assert!(map.base != [0, 0], "the window wrapped");
        let (lo, hi) = (map.origin, map.origin + DVec2::splat(map.extent()));
        let inside = |p: DVec2| p.cmpgt(lo + 0.5).all() && p.cmplt(hi - 0.5).all();
        let (mut kept, mut gone) = (0, 0);
        for p in &pressed {
            if inside(*p) {
                assert!(peak(&map, p.x, p.y) > 0.03, "print at {p} lost");
                kept += 1;
            } else {
                gone += 1;
            }
        }
        assert!(kept > 10 && gone > 10, "{kept} kept, {gone} gone");
        // Back where the walk started, nothing comes back from cells the window reused (these
        // prints weren't remembered).
        map.follow(DVec3::ZERO, 0.0, 20.0, 240.0);
        map.follow(DVec3::ZERO, 0.0, 20.0, 240.0);
        sync(&mut map);
        for p in pressed.iter().filter(|p| p.length() < 3.0) {
            assert_eq!(peak(&map, p.x, p.y), 0.0, "stale print at {p}");
        }
        // A floating-origin move changes nothing in the cells.
        map.follow_shift(&WorldShift::translate(DVec3::new(1_000.0, 0.0, -500.0)));
        assert_eq!(sync(&mut map), 0);
    }
}
