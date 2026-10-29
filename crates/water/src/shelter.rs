//! Wind shelter over a lake: how much of the wind reaches the water at each point.
//!
//! A small lake is rarely rough all over. Wind stress over water recovers only some 40–60
//! obstacle heights downwind of a bank, a bluff or a tree line (Markfort et al. 2010, wind
//! sheltering of lakes), and short waves need a few meters of open water upwind to grow (fetch).
//! So a tarn in a bowl lies glassy in the lee of its upwind shore and of the crag, roughens
//! downwind, and the breeze touches the calm parts only in its strongest gusts (cat's paws):
//! the contrast of rough and mirror patches is what reads as a lake rather than a pool.
//!
//! [`ShelterMask`] holds that exposure (0 in full shelter .. 1 in the open) on a [`MASK_N`]²
//! grid over the lake nearest the camera, marched upwind over the ground once per wind
//! direction (rebuilt when it turns by more than [`REBUILD_DEG`]). The water shader scales the
//! wind's fine waves and ripples by it (`shelter_at` in water.wgsl); the swell, wakes and
//! ripples things make are left alone.

use engine_core::math::{DVec2, DVec3, Vec2};
use engine_core::prelude::*;
use engine_world::Ground;

use crate::body::WaterExtent;
use crate::state::{BodyState, SharedWater};

/// Cells per side of the mask.
pub const MASK_N: usize = 64;
/// Rebuild when the wind turns by more than this, degrees.
pub const REBUILD_DEG: f32 = 10.0;
/// Wind stress recovers over this many obstacle heights downwind (Markfort et al.: 40–60).
const RECOVERY_HEIGHTS: f64 = 50.0;
/// Short waves grow over the first few meters of open water: exposure rises as
/// 1 − e^(−fetch / this), meters.
const FETCH_GROWTH_M: f64 = 5.0;
/// How far upwind the march looks, meters.
const MARCH_M: f64 = 160.0;
/// Lakes larger than this (radius, meters) get no mask: the open middle of a big lake has its
/// full wind anyway.
const MAX_RADIUS_M: f32 = 400.0;

/// Exposure to the wind over one lake. See the module docs.
#[derive(Resource, Debug, Clone, Default)]
pub struct ShelterMask {
    /// The water body it covers.
    pub body: Option<Entity>,
    /// World x/z of the corner of cell (0, 0); cells run +x then +z.
    pub corner: DVec2,
    /// Cell size, meters.
    pub cell: f32,
    /// The wind direction it was marched for (unit, world x/z, toward where it blows).
    pub wind_dir: Vec2,
    /// The still surface's height it was made for, world y.
    pub level: f64,
    /// [`MASK_N`]² exposures, 0..1.
    pub exposure: Vec<f32>,
    /// Bumped on every rebuild (the renderer uploads it again).
    pub version: u64,
}

impl ShelterMask {
    /// Exposure at world x/z, bilinear; 1 outside the mask.
    pub fn at(&self, x: f64, z: f64) -> f32 {
        if self.exposure.len() != MASK_N * MASK_N || self.cell <= 0.0 {
            return 1.0;
        }
        let f = (DVec2::new(x, z) - self.corner) / self.cell as f64 - 0.5;
        if f.x < 0.0 || f.y < 0.0 || f.x > (MASK_N - 1) as f64 || f.y > (MASK_N - 1) as f64 {
            return 1.0;
        }
        let (i, j) = (f.x as usize, f.y as usize);
        let (i1, j1) = ((i + 1).min(MASK_N - 1), (j + 1).min(MASK_N - 1));
        let (tx, tz) = ((f.x - i as f64) as f32, (f.y - j as f64) as f32);
        let e = |i: usize, j: usize| self.exposure[j * MASK_N + i];
        let a = e(i, j) + (e(i1, j) - e(i, j)) * tx;
        let b = e(i, j1) + (e(i1, j1) - e(i, j1)) * tx;
        a + (b - a) * tz
    }
}

/// Exposure at `p` (world x/z, on the water at `level`) to wind blowing toward `wind_dir`:
/// march upwind over `height` (ground height at world x/z) for the nearest shore (fetch) and
/// the obstacle sheltering it most (`1 − distance / (50 · its height above the water)`).
pub fn exposure(p: DVec2, level: f64, wind_dir: Vec2, height: &dyn Fn(f64, f64) -> f64) -> f32 {
    let up = -wind_dir.as_dvec2().normalize_or(DVec2::X);
    let mut fetch = MARCH_M;
    let mut cover: f64 = 0.0;
    let mut s = 0.5;
    while s < MARCH_M {
        let q = p + up * s;
        let above = height(q.x, q.y) - level;
        if above > 0.02 {
            fetch = fetch.min(s);
            cover = cover.max(1.0 - s / (RECOVERY_HEIGHTS * above));
        }
        // Fine near the point, coarser far out, where only tall things still shelter it.
        s += (s * 0.08).clamp(0.5, 4.0);
    }
    let grown = 1.0 - (-fetch / FETCH_GROWTH_M).exp();
    ((1.0 - cover.clamp(0.0, 1.0)) * grown) as f32
}

/// Build the mask over `body` for wind toward `wind_dir`.
pub fn build(body: &BodyState, wind_dir: Vec2, ground: &Ground) -> ShelterMask {
    let radius = body.body.extent.bounding_radius().min(MAX_RADIUS_M);
    let cell = radius * 2.0 / MASK_N as f32;
    let corner = DVec2::new(body.center.x, body.center.z) - radius as f64;
    let level = body.center.y;
    let probe = level + 50.0;
    let height = |x: f64, z: f64| {
        ground
            .height(DVec3::new(x, probe, z))
            .unwrap_or(f64::NEG_INFINITY)
    };
    let mut exposure_grid = vec![1.0; MASK_N * MASK_N];
    engine_core::tasks::init_task_pools();
    engine_core::tasks::par_chunks_mut(&mut exposure_grid, MASK_N * 4, |k, chunk| {
        for (o, e) in chunk.iter_mut().enumerate() {
            let idx = k * MASK_N * 4 + o;
            let (i, j) = (idx % MASK_N, idx / MASK_N);
            let p = corner + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell as f64;
            *e = exposure(p, level, wind_dir, &height);
        }
    });
    ShelterMask {
        body: Some(body.entity),
        corner,
        cell,
        wind_dir,
        level,
        exposure: exposure_grid,
        version: 0,
    }
}

/// Keep the mask over the lake nearest the focus (else the camera) up to date with the wind.
pub fn shelter_system(
    settings: Option<Res<crate::body::WaterSettings>>,
    shared: Res<SharedWater>,
    focus: Res<engine_world::FocusPoints>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    mut mask: ResMut<ShelterMask>,
) {
    let camera = || {
        cameras
            .iter()
            .filter(|(c, _)| c.active)
            .max_by_key(|(c, _)| c.order)
            .map(|(_, g)| g.translation())
    };
    let on = settings.is_none_or(|s| s.wind_shelter);
    let Some(eye) = focus.points.first().copied().or_else(camera).filter(|_| on) else {
        if mask.body.is_some() {
            *mask = ShelterMask {
                version: mask.version + 1,
                ..Default::default()
            };
        }
        return;
    };
    let state = shared.0.read();
    let nearest = state
        .bodies
        .iter()
        .filter(|b| {
            b.body.visible
                && !matches!(b.body.extent, WaterExtent::Infinite)
                && b.body.extent.bounding_radius() <= MAX_RADIUS_M
        })
        .map(|b| {
            let d = DVec2::new(eye.x - b.center.x, eye.z - b.center.z).length()
                - b.body.extent.bounding_radius() as f64;
            (d, b)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((_, body)) = nearest else {
        if mask.body.is_some() {
            *mask = ShelterMask {
                version: mask.version + 1,
                ..Default::default()
            };
        }
        return;
    };
    let wind = Vec2::new(body.wind.x, body.wind.z);
    let Some(dir) = wind.try_normalize() else {
        return;
    };
    let radius = body.body.extent.bounding_radius().min(MAX_RADIUS_M);
    let same = mask.body == Some(body.entity)
        && (mask.level - body.center.y).abs() < 0.05
        && (mask.corner - (DVec2::new(body.center.x, body.center.z) - radius as f64)).length()
            < 0.5
        && mask.wind_dir.dot(dir) > REBUILD_DEG.to_radians().cos();
    if same {
        return;
    }
    let version = mask.version + 1;
    *mask = ShelterMask {
        version,
        ..build(body, dir, &state.ground)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lake at y = 0 from x = -30 to 30, banks 1.5 m high, and a 15 m crag beyond x = 30.
    fn ground(x: f64, _z: f64) -> f64 {
        if x > 34.0 {
            15.0
        } else if x.abs() > 30.0 {
            1.5
        } else {
            -3.0
        }
    }

    #[test]
    fn water_lies_calm_in_the_lee_and_roughens_downwind() {
        // Wind blowing toward -x: the crag is upwind.
        let wind = Vec2::NEG_X;
        let e = |x: f64| exposure(DVec2::new(x, 0.0), 0.0, wind, &ground);
        // Under the crag, sheltered: 15 m shelters some 750 m.
        assert!(e(28.0) < 0.05, "{}", e(28.0));
        assert!(e(-28.0) < 0.2, "{}", e(-28.0));
        // Wind blowing toward +x over the low bank: calm only near it, open farther out.
        let e = |x: f64| exposure(DVec2::new(x, 0.0), 0.0, Vec2::X, &ground);
        assert!(e(-29.0) < 0.2, "near the upwind bank {}", e(-29.0));
        assert!(e(25.0) > 0.5, "far across {}", e(25.0));
        assert!(e(25.0) > e(0.0) && e(0.0) > e(-25.0));
    }

    #[test]
    fn open_water_has_all_the_wind() {
        let flat = |_x: f64, _z: f64| -5.0;
        assert!((exposure(DVec2::ZERO, 0.0, Vec2::X, &flat) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_mask_reads_back_what_it_holds() {
        let mut mask = ShelterMask {
            corner: DVec2::new(-32.0, -32.0),
            cell: 1.0,
            exposure: vec![0.25; MASK_N * MASK_N],
            ..Default::default()
        };
        mask.exposure[MASK_N * 32 + 32] = 1.0;
        assert!((mask.at(0.5, 0.5) - 1.0).abs() < 1e-5);
        assert!((mask.at(-10.0, 3.0) - 0.25).abs() < 1e-5);
        // Outside: all the wind.
        assert_eq!(mask.at(100.0, 0.0), 1.0);
    }
}
