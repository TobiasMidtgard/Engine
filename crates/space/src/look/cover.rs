//! The planet's cloud cover where the weather happens, so the weather's clouds and the cloud
//! tops seen from high up are one field (Horizon Zero Dawn's Nubis and Flight Simulator drive
//! every cloud from one weather map). The far view's baked cloud map is sampled on a grid in
//! the world's X/Z plane around the focus; the weather builds its clouds on that cover.

use std::sync::Arc;

use engine_core::math::{DVec2, DVec3};
use engine_render::Image;

/// A body's baked cloud cover (the R channel of its aux map, equirectangular, unorm8).
#[derive(Debug, Clone)]
pub struct CloudChannel {
    pub width: u32,
    pub height: u32,
    pub cover: Arc<[u8]>,
}

impl CloudChannel {
    /// The cover channel of an RGBA8 aux map.
    pub fn from_aux(aux: &Image) -> Self {
        Self {
            width: aux.width,
            height: aux.height,
            cover: aux.data.as_chunks::<4>().0.iter().map(|t| t[0]).collect(),
        }
    }

    /// Cover 0..1 at body-local unit direction `d` (+Y north, longitude 0 at +Z, east +X),
    /// with the clouds turned east by `drift` (a share of a turn, as the shell's `sun.w`).
    /// Bilinear, wrapping in longitude.
    pub fn at(&self, d: DVec3, drift: f64) -> f32 {
        let lon = d.x.atan2(d.z);
        let lat = d.y.clamp(-1.0, 1.0).asin();
        let u = lon / std::f64::consts::TAU + 0.5 + drift;
        let v = 0.5 - lat / std::f64::consts::PI;
        let (w, h) = (self.width as i64, self.height as i64);
        let x = u * w as f64 - 0.5;
        let y = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let texel = |i: i64, j: i64| {
            let i = i.rem_euclid(w);
            let j = j.clamp(0, h - 1);
            self.cover[(j * w + i) as usize] as f32 / 255.0
        };
        let (i, j) = (x0 as i64, y0 as i64);
        let top = texel(i, j) * (1.0 - fx) + texel(i + 1, j) * fx;
        let bottom = texel(i, j + 1) * (1.0 - fx) + texel(i + 1, j + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// The cover on a `size`² grid of `cell_m` cells in the world's X/Z plane, cell (0, 0)'s corner
/// at `origin`, rows along +Z (unorm8), for a body centered at world `center` with body axes
/// `axes` (world directions of its +X, +Y and +Z).
pub fn cover_grid(
    channel: &CloudChannel,
    center: DVec3,
    axes: [DVec3; 3],
    drift: f64,
    origin: DVec2,
    size: u32,
    cell_m: f64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size) as usize);
    for j in 0..size {
        for i in 0..size {
            let p = origin + (DVec2::new(i as f64, j as f64) + 0.5) * cell_m;
            let d = (DVec3::new(p.x, 0.0, p.y) - center).normalize_or(DVec3::Y);
            let local = DVec3::new(d.dot(axes[0]), d.dot(axes[1]), d.dot(axes[2]));
            let c = channel.at(local, drift);
            out.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

/// How far the clouds over `focus` (world, m) have moved in the world's X/Z plane after the
/// drift grew by `turn` (a share of a turn, wrapped to ±½), for a body centered at `center`
/// with `axes` and equatorial radius `r_eq` (m). Sampled at longitude + drift, the clouds move
/// toward smaller longitudes as the drift grows: west, by the turn's share of the circle at
/// the focus's latitude.
pub fn drift_shift(center: DVec3, axes: [DVec3; 3], r_eq: f64, focus: DVec3, turn: f64) -> DVec2 {
    let turn = (turn + 0.5).rem_euclid(1.0) - 0.5;
    let up = (focus - center).normalize_or(DVec3::Y);
    let local = DVec3::new(up.dot(axes[0]), up.dot(axes[1]), up.dot(axes[2]));
    let lon = local.x.atan2(local.z);
    let east = axes[0] * lon.cos() - axes[2] * lon.sin();
    let lat_cos = (1.0 - local.y * local.y).max(0.0).sqrt();
    let m = east * (-turn * std::f64::consts::TAU * r_eq * lat_cos);
    DVec2::new(m.x, m.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cloud over the longitudes a quarter turn east of 0 (u 0.5..0.75), clear elsewhere.
    fn band() -> CloudChannel {
        let (w, h) = (256u32, 128u32);
        let cover = (0..h)
            .flat_map(|_| (0..w).map(move |x| if (128..192).contains(&x) { 255 } else { 0 }))
            .collect();
        CloudChannel {
            width: w,
            height: h,
            cover,
        }
    }

    #[test]
    fn the_grid_reads_the_body_map_under_each_cell() {
        let r = 6_371_000.0;
        // Home on the equator at longitude 0: body +Z is world up, north is world −Z.
        let center = DVec3::new(0.0, -r, 0.0);
        let axes = [DVec3::X, DVec3::NEG_Z, DVec3::Y];
        let ch = band();
        let local = |x: f64, z: f64| {
            let d = (DVec3::new(x, 0.0, z) - center).normalize();
            DVec3::new(d.dot(axes[0]), d.dot(axes[1]), d.dot(axes[2]))
        };
        // East of home is cloudy, west is clear.
        assert!(ch.at(local(200_000.0, 0.0), 0.0) > 0.99);
        assert!(ch.at(local(-200_000.0, 0.0), 0.0) < 0.01);
        // Drifted a quarter turn east, the band has moved off home's east side.
        assert!(ch.at(local(200_000.0, 0.0), 0.25) < 0.01);
        // Drifting moves the band's edge (at home) by `drift_shift`: what showed just east of
        // home now shows that far away.
        let turn = 0.002;
        let moved = drift_shift(center, axes, r, DVec3::ZERO, turn);
        assert!(moved.x < -70_000.0 && moved.y.abs() < 1.0, "{moved}");
        let before = ch.at(local(30_000.0, 0.0), 0.0);
        let after = ch.at(local(30_000.0 + moved.x, 0.0), turn);
        assert!((before - after).abs() < 0.01, "{before} vs {after}");
        // The grid matches, cell by cell, with rows along +Z.
        let grid = cover_grid(
            &ch,
            center,
            axes,
            0.0,
            DVec2::new(-1_024_000.0, -4_000.0),
            4,
            512_000.0,
        );
        assert_eq!(grid.len(), 16);
        for row in grid.chunks(4) {
            assert_eq!(row[0], 0);
            assert_eq!(row[1], 0);
            assert_eq!(row[2], 255);
            assert_eq!(row[3], 255);
        }
    }
}
