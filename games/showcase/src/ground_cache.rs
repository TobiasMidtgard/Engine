//! The world's ground near the focus, baked into tiles off the frame and read from them.
//!
//! One sample of the game's ground at the fortress cost about 34 µs on the VM: the planet's own
//! shape under the valley for the height and both normal probes, then the landscape's height,
//! four-tap normal and cover for the surface. Debris, foot IK, NPC sight lines, fire, water and
//! weather all ask it, many times a frame. Engines read terrain from a baked heightfield instead
//! (Unity's `TerrainData.GetInterpolatedHeight`/`GetInterpolatedNormal` interpolate the stored
//! heightmap; Unreal's landscape collision is a heightfield baked per component). This does
//! the same for whatever ground is installed: tiles around each focus are baked on the async
//! pool from the ground itself, and read bilinearly (height and normal) or from the nearest
//! surface node; anywhere else, or before a tile is ready, the ground answers exactly.
//!
//! When the ground is replaced (terrain edits, a moved terrain, a world shift) the cache wraps
//! the new one; after edits it keeps the tiles the edits didn't touch.

use engine::prelude::*;
use engine::world::{
    FocusPoints, Gravity, Ground, GroundQuery, GroundSample, SurfaceId, TerrainChanged,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Height and normal nodes per tile side, less one.
const CELLS: usize = 64;
/// Spacing of the height and normal nodes, m.
const SPACING_M: f64 = 0.5;
/// Width of a tile, m.
const TILE_M: f64 = CELLS as f64 * SPACING_M;
/// Surface nodes are every `SURFACE_STEP` height nodes (the surface is looked up at the
/// nearest one; a full sample costs five times a height).
const SURFACE_STEP: usize = 2;
/// Tiles are baked out to this far around each focus, m.
const RADIUS_M: f64 = 64.0;
/// And forgotten beyond this.
const FORGET_M: f64 = 160.0;
/// Tiles baked at once (each is about 70 ms of one async thread).
const IN_FLIGHT: usize = 2;

/// One baked square of ground.
struct Tile {
    /// World x and z of its first node.
    x0: f64,
    z0: f64,
    /// Heights, `(CELLS + 1)²`, row by row along x.
    height: Box<[f64]>,
    normal: Box<[Vec3]>,
    surface: Box<[SurfaceId]>,
}

const N: usize = CELLS + 1;
const NS: usize = CELLS / SURFACE_STEP + 1;

impl Tile {
    /// Bake the tile at `key` from `ground` (`y` is the height hint passed to it). `None` where
    /// the ground has a hole in it: such a tile stays exact.
    fn bake(ground: &dyn GroundQuery, key: (i64, i64), y: f64) -> Option<Tile> {
        let (x0, z0) = (key.0 as f64 * TILE_M, key.1 as f64 * TILE_M);
        // Heights with a one-node border for the normals' central differences.
        let w = N + 2;
        let mut h = vec![0.0f64; w * w];
        for j in 0..w {
            for i in 0..w {
                let p = DVec3::new(
                    x0 + (i as f64 - 1.0) * SPACING_M,
                    y,
                    z0 + (j as f64 - 1.0) * SPACING_M,
                );
                h[j * w + i] = ground.height(p)?;
            }
        }
        let mut height = Vec::with_capacity(N * N);
        let mut normal = Vec::with_capacity(N * N);
        for j in 0..N {
            for i in 0..N {
                let at = |di: isize, dj: isize| {
                    h[(j as isize + 1 + dj) as usize * w + (i as isize + 1 + di) as usize]
                };
                height.push(at(0, 0));
                let dx = (at(1, 0) - at(-1, 0)) / (2.0 * SPACING_M);
                let dz = (at(0, 1) - at(0, -1)) / (2.0 * SPACING_M);
                normal.push(Vec3::new(-dx as f32, 1.0, -dz as f32).normalize());
            }
        }
        let mut surface = Vec::with_capacity(NS * NS);
        for j in 0..NS {
            for i in 0..NS {
                let (hi, hj) = (i * SURFACE_STEP, j * SURFACE_STEP);
                let p = DVec3::new(
                    x0 + hi as f64 * SPACING_M,
                    height[hj * N + hi],
                    z0 + hj as f64 * SPACING_M,
                );
                surface.push(ground.sample(p)?.surface);
            }
        }
        Some(Tile {
            x0,
            z0,
            height: height.into(),
            normal: normal.into(),
            surface: surface.into(),
        })
    }

    /// Node coordinates of `p` in the tile: indices of the cell's first corner and the
    /// fractions across it.
    fn cell(&self, p: DVec3) -> (usize, usize, f64, f64) {
        let gx = ((p.x - self.x0) / SPACING_M).clamp(0.0, CELLS as f64);
        let gz = ((p.z - self.z0) / SPACING_M).clamp(0.0, CELLS as f64);
        let (i, j) = ((gx as usize).min(CELLS - 1), (gz as usize).min(CELLS - 1));
        (i, j, gx - i as f64, gz - j as f64)
    }

    fn height(&self, p: DVec3) -> f64 {
        let (i, j, fx, fz) = self.cell(p);
        let h = |a: usize, b: usize| self.height[(j + b) * N + i + a];
        let lo = h(0, 0) + (h(1, 0) - h(0, 0)) * fx;
        let hi = h(0, 1) + (h(1, 1) - h(0, 1)) * fx;
        lo + (hi - lo) * fz
    }

    fn sample(&self, p: DVec3) -> GroundSample {
        let (i, j, fx, fz) = self.cell(p);
        let n = |a: usize, b: usize| self.normal[(j + b) * N + i + a];
        let (fx32, fz32) = (fx as f32, fz as f32);
        let normal = (n(0, 0) * (1.0 - fx32) * (1.0 - fz32)
            + n(1, 0) * fx32 * (1.0 - fz32)
            + n(0, 1) * (1.0 - fx32) * fz32
            + n(1, 1) * fx32 * fz32)
            .normalize_or(Vec3::Y);
        let si = ((i as f64 + fx) / SURFACE_STEP as f64).round() as usize;
        let sj = ((j as f64 + fz) / SURFACE_STEP as f64).round() as usize;
        GroundSample {
            height: self.height(p),
            normal,
            surface: self.surface[sj.min(NS - 1) * NS + si.min(NS - 1)],
        }
    }
}

fn key_of(x: f64, z: f64) -> (i64, i64) {
    ((x / TILE_M).floor() as i64, (z / TILE_M).floor() as i64)
}

/// The baked tiles, shared by the ground and its bake jobs.
#[derive(Default)]
struct Tiles {
    ready: RwLock<HashMap<(i64, i64), Arc<Tile>>>,
    pending: Mutex<HashSet<(i64, i64)>>,
    in_flight: AtomicUsize,
}

/// A ground that answers from baked tiles where it has them, and asks `inner` elsewhere.
pub struct TiledGround {
    inner: Arc<dyn GroundQuery>,
    tiles: Arc<Tiles>,
}

impl TiledGround {
    fn new(inner: Arc<dyn GroundQuery>) -> Self {
        Self {
            inner,
            tiles: Arc::default(),
        }
    }

    fn tile(&self, p: DVec3) -> Option<Arc<Tile>> {
        let ready = self.tiles.ready.read().unwrap_or_else(|e| e.into_inner());
        ready.get(&key_of(p.x, p.z)).cloned()
    }

    /// Bake the tile at `key` on this thread (tests, and the bake jobs).
    fn bake_now(&self, key: (i64, i64), y: f64) {
        if let Some(t) = Tile::bake(&*self.inner, key, y) {
            self.tiles
                .ready
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, Arc::new(t));
        }
    }
}

#[cfg(test)]
impl TiledGround {
    /// `inner` with every tile within `reach` of `at` baked.
    pub fn baked_for_tests(inner: Arc<dyn GroundQuery>, at: DVec3, reach: f64) -> Self {
        let tiled = Self::new(inner);
        let (lo, hi) = (
            key_of(at.x - reach, at.z - reach),
            key_of(at.x + reach, at.z + reach),
        );
        for a in lo.0..=hi.0 {
            for b in lo.1..=hi.1 {
                tiled.bake_now((a, b), at.y);
            }
        }
        tiled
    }
}

impl GroundQuery for TiledGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        match self.tile(p) {
            Some(t) => Some(t.sample(p)),
            None => self.inner.sample(p),
        }
    }

    fn height(&self, p: DVec3) -> Option<f64> {
        match self.tile(p) {
            Some(t) => Some(t.height(p)),
            None => self.inner.height(p),
        }
    }
}

/// The wrapper installed last.
#[derive(Default)]
pub struct Wrapped(Option<Arc<TiledGround>>);

/// Wrap a newly installed ground, then start baking the tiles the foci are missing and forget
/// the ones they left (PostUpdate, after the terrain installs its ground).
pub fn update(
    mut ground: ResMut<Ground>,
    focus: Res<FocusPoints>,
    gravity: Res<Gravity>,
    mut changes: MessageReader<TerrainChanged>,
    mut wrapped: Local<Wrapped>,
) {
    let changed: Vec<TerrainChanged> = changes.read().copied().collect();
    // (The same object: data pointers only, as vtables may differ between codegen units.)
    let ours = wrapped
        .0
        .as_ref()
        .is_some_and(|w| std::ptr::addr_eq(Arc::as_ptr(&ground.0), Arc::as_ptr(w)));
    if !ours {
        let fresh = Arc::new(TiledGround::new(ground.0.clone()));
        // Edits only: what they didn't touch is still what the old ground baked.
        if let Some(old) = &wrapped.0
            && !changed.is_empty()
        {
            let keep = old.tiles.ready.read().unwrap_or_else(|e| e.into_inner());
            let mut ready = fresh.tiles.ready.write().unwrap_or_else(|e| e.into_inner());
            for (key, tile) in keep.iter() {
                let (cx, cz) = (tile.x0 + TILE_M * 0.5, tile.z0 + TILE_M * 0.5);
                let touched = changed.iter().any(|c| {
                    let r = c.radius as f64 + TILE_M * 0.75;
                    (c.center.x - cx).abs() < r && (c.center.z - cz).abs() < r
                });
                if !touched {
                    ready.insert(*key, tile.clone());
                }
            }
        }
        wrapped.0 = Some(fresh.clone());
        ground.0 = fresh;
    }
    let Some(tiled) = &wrapped.0 else {
        return;
    };
    // A heightfield in world x and z: only where the world's up is +Y (the valley; not the
    // Moon seen from orbit).
    let foci: Vec<DVec3> = focus
        .points
        .iter()
        .copied()
        .filter(|p| gravity.up_at(*p).y > 0.9999)
        .collect();
    let tiles = &tiled.tiles;
    {
        let near = |key: &(i64, i64), reach: f64| {
            let (cx, cz) = ((key.0 as f64 + 0.5) * TILE_M, (key.1 as f64 + 0.5) * TILE_M);
            foci.iter()
                .any(|f| (f.x - cx).abs() < reach && (f.z - cz).abs() < reach)
        };
        let mut ready = tiles.ready.write().unwrap_or_else(|e| e.into_inner());
        ready.retain(|key, _| near(key, FORGET_M));
    }
    for f in &foci {
        if tiles.in_flight.load(Ordering::Relaxed) >= IN_FLIGHT {
            break;
        }
        // Nearest missing tile first.
        let r = (RADIUS_M / TILE_M).ceil() as i64;
        let home = key_of(f.x, f.z);
        let mut want: Vec<(i64, i64)> = (-r..=r)
            .flat_map(|a| (-r..=r).map(move |b| (home.0 + a, home.1 + b)))
            .collect();
        want.sort_by_key(|k| (k.0 - home.0).abs().max((k.1 - home.1).abs()));
        for key in want {
            if tiles.in_flight.load(Ordering::Relaxed) >= IN_FLIGHT {
                break;
            }
            if tiles
                .ready
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&key)
                || !tiles
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key)
            {
                continue;
            }
            tiles.in_flight.fetch_add(1, Ordering::Relaxed);
            let job = tiled.clone();
            let y = f.y;
            engine::core::tasks::AsyncComputeTaskPool::get()
                .spawn(async move {
                    job.bake_now(key, y);
                    job.tiles
                        .pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&key);
                    job.tiles.in_flight.fetch_sub(1, Ordering::Relaxed);
                })
                .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::landscape::{LandscapeGround, shared};

    /// The baked ground answers like the ground it bakes, at the fortress and the plaza: the
    /// landscape on its own (`--no-planet`) and the game's planet ground.
    #[test]
    fn tiles_answer_like_the_ground() {
        let mut table = engine::world::SurfaceMaterials::default();
        let landscape: Arc<dyn GroundQuery> =
            Arc::new(LandscapeGround::new(shared().clone(), &mut table));
        let (planet, _) = crate::planet::ground_for_tests();
        for (name, exact) in [("landscape", landscape), ("planet", planet)] {
            matches_within_tolerance(name, exact);
        }
    }

    fn matches_within_tolerance(name: &str, exact: Arc<dyn GroundQuery>) {
        let tiled = TiledGround::new(exact.clone());
        let mut points = Vec::new();
        for (cx, cz) in [(-300.0, 190.0), (0.0, 0.0)] {
            let home = key_of(cx, cz);
            for a in -1..=1 {
                for b in -1..=1 {
                    tiled.bake_now((home.0 + a, home.1 + b), 10.0);
                }
            }
            for i in 0..2000 {
                let t = i as f64 * 0.618_034;
                points.push(DVec3::new(
                    cx + (t.fract() - 0.5) * 2.0 * TILE_M,
                    10.0,
                    cz + ((t * 7.3).fract() - 0.5) * 2.0 * TILE_M,
                ));
            }
        }
        let mut dh = Vec::new();
        let mut angle = Vec::new();
        let mut same = 0;
        for p in &points {
            let (a, b) = (exact.sample(*p).unwrap(), tiled.sample(*p).unwrap());
            assert_eq!(tiled.height(*p), Some(b.height));
            dh.push((a.height - b.height).abs());
            angle.push(a.normal.angle_between(b.normal).to_degrees() as f64);
            same += (a.surface == b.surface) as usize;
        }
        let pct = |v: &mut Vec<f64>, q: f64| {
            v.sort_by(f64::total_cmp);
            v[((v.len() - 1) as f64 * q) as usize]
        };
        let (h99, hmax) = (pct(&mut dh, 0.99), pct(&mut dh, 1.0));
        let (a99, amax) = (pct(&mut angle, 0.99), pct(&mut angle, 1.0));
        let surface = same as f64 / points.len() as f64;
        println!(
            "{name}: height p99 {h99:.4} max {hmax:.4} m, normal p99 {a99:.2} max {amax:.2}°, \
             surface {:.1}%",
            surface * 100.0
        );
        // Measured 2026-11-06: 0.1 mm, 0.04° and 99.8% on both.
        assert!(hmax < 0.01, "{name}: height max {hmax} m");
        assert!(a99 < 1.0, "{name}: normal p99 {a99}°");
        assert!(surface > 0.99, "{name}: surface {surface}");
    }
}
