//! Photographed leaves for the leaf atlases: the single-leaf atlas
//! ([`super::textures::hero_leaf_textures`]) and every drawn blade of the spray and mass
//! atlases (`Canvas::scanned_blade`).
//!
//! The CC0 leaf sets fetched by `cargo run -p fetch-assets -- scans` (assets/manifests/
//! scans.toml) are sheets of real leaves scanned flat on a light table, upright with the
//! petiole down, coverage in alpha and an OpenGL normal map beside them. A sheet is split into
//! its leaves once (connected regions of its alpha); each single-leaf cell, and each blade a
//! spray would draw, then takes one leaf, scaled and turned to its frame and recolored to the
//! painted leaf color, so a scan
//! changes the leaf's shape, veins, holes and blemishes but not the crown's color or season.
//! Without the files (CI does not fetch them) the painter draws the leaves as before.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use engine_core::math::{Vec2, Vec3};
use image::imageops::{self, FilterType};
use image::{RgbImage, Rgba32FImage};

use super::bark::{fetched_path, scans_on, srgb_to_linear};
use super::textures::LeafKind;

/// A sheet of scanned leaves, split into single leaves.
pub(crate) struct LeafScan {
    leaves: Vec<ScanLeaf>,
    /// Mean linear color of the leaves' covered texels.
    mean: Vec3,
    /// Mean brightest channel of the covered texels (see `keep_hue`).
    mean_value: f32,
    /// Recolor by channel (keeping the scan's own color variation) rather than by brightness
    /// only (for sheets photographed in another season's colors). Brightness is the largest
    /// channel, not luminance, so red and green patches of an autumn leaf come out alike and
    /// only veins, blemishes and shading show.
    keep_hue: bool,
}

struct ScanLeaf {
    /// Linear color premultiplied by coverage, cropped to the leaf.
    color: Rgba32FImage,
    normal: RgbImage,
    /// `color` and `normal` halved again and again (from half size down to about 16 pixels
    /// tall), for leaves stamped small into sprays.
    mips: Vec<(Rgba32FImage, RgbImage)>,
    /// Row (pixels, from the top) where the blade ends and the petiole begins.
    blade_y: f32,
    /// Column (pixels) of the leaf's middle: its coverage centroid.
    mid_x: f32,
}

impl ScanLeaf {
    fn new(color: Rgba32FImage, normal: RgbImage) -> Self {
        let (w, h) = color.dimensions();
        // The blade ends at the lowest row wider than a fifth of the widest one.
        let widths: Vec<u32> = (0..h)
            .map(|y| (0..w).filter(|&x| color.get_pixel(x, y).0[3] > 0.5).count() as u32)
            .collect();
        let widest = widths.iter().copied().max().unwrap_or(1);
        let blade_y = widths
            .iter()
            .rposition(|&n| n * 5 > widest)
            .map_or(h as f32, |y| y as f32 + 1.0);
        let (mut sx, mut sa) = (0.0, 0.0);
        for (x, _, p) in color.enumerate_pixels() {
            sx += x as f32 * p.0[3];
            sa += p.0[3];
        }
        let mid_x = if sa > 0.0 {
            sx / sa + 0.5
        } else {
            w as f32 * 0.5
        };
        let mut mips = Vec::new();
        let (mut mw, mut mh) = (w / 2, h / 2);
        while mh >= 16 && mw >= 4 {
            mips.push((
                imageops::resize(&color, mw, mh, FilterType::Triangle),
                imageops::resize(&normal, mw, mh, FilterType::Triangle),
            ));
            (mw, mh) = (mw / 2, mh / 2);
        }
        Self {
            color,
            normal,
            mips,
            blade_y,
            mid_x,
        }
    }
}

/// The fetched sheet a leaf kind's single leaves come from, and whether its colors are kept.
fn sheet(kind: LeafKind) -> Option<(&'static str, bool)> {
    match kind {
        LeafKind::Lobed => Some(("oak_leaves", true)),
        // Oval, finely toothed leaves with straight side veins (beech, hornbeam): beech's own,
        // and close to hazel's rounder ones.
        LeafKind::Oval | LeafKind::Broad => Some(("broadleaf_light", true)),
        // The maple sheet (scanned in autumn) is not used: its red, gold and green patches
        // differ in brightness too, so even recolored by brightness the leaves came out
        // blotched dark and white. The painted maple leaves stay. `keep_hue: false` is kept for
        // a sheet with even brightness in another season's colors.
        _ => None,
    }
}

/// Loaded sheets by folder (`None`: missing or unreadable).
type Sheets = HashMap<&'static str, Option<Arc<LeafScan>>>;

/// The scanned leaves for `kind`, loaded once; `None` when the kind has no sheet, scans are
/// off or the files were not fetched.
pub(crate) fn leaf_scan(kind: LeafKind) -> Option<Arc<LeafScan>> {
    static CACHE: LazyLock<Mutex<Sheets>> = LazyLock::new(Default::default);
    let (folder, keep_hue) = sheet(kind).filter(|_| scans_on())?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(folder)
        .or_insert_with(|| load(folder, keep_hue).map(Arc::new))
        .clone()
}

fn load(folder: &str, keep_hue: bool) -> Option<LeafScan> {
    let path = |suffix: &str, ext: &str| {
        fetched_path(&format!("scans/leaves/{folder}/{folder}_{suffix}_2k.{ext}"))
    };
    let open = |p: std::path::PathBuf| match image::open(&p) {
        Ok(img) => Some(img),
        Err(e) => {
            tracing::warn!("foliage: leaf scan {}: {e}", p.display());
            None
        }
    };
    let diff = open(path("diff", "png")?)?.to_rgba8();
    let nor = open(path("nor_gl", "jpg")?)?.to_rgb8();
    if nor.dimensions() != diff.dimensions() {
        tracing::warn!("foliage: leaf scan {folder}: normal map size differs");
        return None;
    }
    let (w, h) = diff.dimensions();
    // Split into leaves on a coarse grid (each cell the max coverage of its block, so thin
    // petioles stay joined to their blades).
    const BLOCK: u32 = 4;
    let (gw, gh) = (w.div_ceil(BLOCK), h.div_ceil(BLOCK));
    let mut grid = vec![false; (gw * gh) as usize];
    for (x, y, p) in diff.enumerate_pixels() {
        if p.0[3] > 40 {
            grid[((y / BLOCK) * gw + x / BLOCK) as usize] = true;
        }
    }
    let mut label = vec![u32::MAX; grid.len()];
    let mut boxes: Vec<[u32; 5]> = Vec::new(); // x0, y0, x1, y1 (grid, inclusive), area
    for start in 0..grid.len() {
        if !grid[start] || label[start] != u32::MAX {
            continue;
        }
        let id = boxes.len() as u32;
        let (sx, sy) = (start as u32 % gw, start as u32 / gw);
        let mut b = [sx, sy, sx, sy, 0];
        let mut stack = vec![start];
        label[start] = id;
        while let Some(i) = stack.pop() {
            let (x, y) = (i as u32 % gw, i as u32 / gw);
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y), b[4] + 1];
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= gw as i32 || ny >= gh as i32 {
                    continue;
                }
                let j = (ny as u32 * gw + nx as u32) as usize;
                if grid[j] && label[j] == u32::MAX {
                    label[j] = id;
                    stack.push(j);
                }
            }
        }
        boxes.push(b);
    }
    // Leaves, not specks: at least a fifth of the largest region.
    let largest = boxes.iter().map(|b| b[4]).max()?;
    let mut leaves = Vec::new();
    let mut sum = Vec3::ZERO;
    let mut value = 0.0;
    let mut covered = 0.0;
    for (id, b) in boxes.iter().enumerate() {
        if b[4] * 5 < largest {
            continue;
        }
        let x0 = b[0] * BLOCK;
        let y0 = b[1] * BLOCK;
        let x1 = ((b[2] + 1) * BLOCK).min(w);
        let y1 = ((b[3] + 1) * BLOCK).min(h);
        let mut color = Rgba32FImage::new(x1 - x0, y1 - y0);
        for (x, y, out) in color.enumerate_pixels_mut() {
            let (px, py) = (x0 + x, y0 + y);
            // Only this leaf's region: a neighbor's tip reaching into the box stays out.
            if label[((py / BLOCK) * gw + px / BLOCK) as usize] != id as u32 {
                continue;
            }
            let d = diff.get_pixel(px, py).0;
            let a = d[3] as f32 / 255.0;
            let c = Vec3::new(
                srgb_to_linear(d[0] as f32 / 255.0),
                srgb_to_linear(d[1] as f32 / 255.0),
                srgb_to_linear(d[2] as f32 / 255.0),
            );
            sum += c * a;
            value += c.max_element() * a;
            covered += a;
            *out = image::Rgba([c.x * a, c.y * a, c.z * a, a]);
        }
        let normal = imageops::crop_imm(&nor, x0, y0, x1 - x0, y1 - y0).to_image();
        leaves.push((b[1], b[0], ScanLeaf::new(color, normal)));
    }
    // Reading order, so a sheet's layout maps to cells the same way every run.
    leaves.sort_by_key(|(y, x, _)| (*y / 64, *x));
    let leaves: Vec<ScanLeaf> = leaves.into_iter().map(|(_, _, l)| l).collect();
    if leaves.is_empty() || covered <= 0.0 {
        return None;
    }
    tracing::info!("foliage: leaf scan {folder}: {} leaves", leaves.len());
    Some(LeafScan {
        leaves,
        mean: sum / covered,
        mean_value: value / covered,
        keep_hue,
    })
}

impl LeafScan {
    /// How many leaves the sheet holds.
    pub(crate) fn count(&self) -> usize {
        self.leaves.len()
    }

    /// Premultiplied scan color `c` recolored so the sheet's mean comes out `tint`.
    pub(crate) fn recolor(&self, c: Vec3, tint: Vec3) -> Vec3 {
        if self.keep_hue {
            c * tint / self.mean.max(Vec3::splat(1e-4))
        } else {
            tint * (c.max_element() / self.mean_value.max(1e-4))
        }
    }

    /// Leaf `index` sampled in its blade's frame: `u` from the blade's base (0) to its tip
    /// (1), `v` across it in blade lengths (positive to the leaf's right as scanned), for a
    /// blade drawn `blade_px` pixels long. Returns the premultiplied linear color (coverage
    /// in alpha) and the scan's tangent normal (x right, y toward the tip), or `None` off
    /// the blade (the petiole included: sprays paint their own).
    pub(crate) fn sample(
        &self,
        index: usize,
        u: f32,
        v: f32,
        blade_px: f32,
    ) -> Option<([f32; 4], Vec3)> {
        let leaf = &self.leaves[index % self.leaves.len()];
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        // The level whose blade is the smallest still at least `blade_px` long.
        let mut level = (&leaf.color, &leaf.normal, 1.0f32);
        for (k, (c, n)) in leaf.mips.iter().enumerate() {
            let scale = 0.5f32.powi(k as i32 + 1);
            if leaf.blade_y * scale < blade_px {
                break;
            }
            level = (c, n, scale);
        }
        let (color, normal, scale) = level;
        let (w, h) = color.dimensions();
        let x = (leaf.mid_x + v * leaf.blade_y) * scale - 0.5;
        let y = leaf.blade_y * (1.0 - u) * scale - 0.5;
        if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
            return None;
        }
        // Bilinear, transparent outside.
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let mut c = [0.0f32; 4];
        for (dx, dy, wgt) in [
            (0, 0, (1.0 - fx) * (1.0 - fy)),
            (1, 0, fx * (1.0 - fy)),
            (0, 1, (1.0 - fx) * fy),
            (1, 1, fx * fy),
        ] {
            let (sx, sy) = (x0 as i32 + dx, y0 as i32 + dy);
            if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
                continue;
            }
            let p = color.get_pixel(sx as u32, sy as u32).0;
            for i in 0..4 {
                c[i] += p[i] * wgt;
            }
        }
        if c[3] <= 1e-3 {
            return None;
        }
        let (nx, ny) = (
            (x.round() as i32).clamp(0, w as i32 - 1) as u32,
            (y.round() as i32).clamp(0, h as i32 - 1) as u32,
        );
        let q = normal.get_pixel(nx, ny).0;
        let f = |c: u8| c as f32 / 255.0 * 2.0 - 1.0;
        Some((c, Vec3::new(f(q[0]), f(q[1]), f(q[2]))))
    }

    /// Leaf `index` painted into a `size`² cell: premultiplied linear color and tangent
    /// normals (x right, y up), row by row from the top. The leaf is scaled to fit `rect`
    /// (cell units, y down) with its petiole at the rect's bottom center, recolored so its
    /// mean is `tint`, and mirrored left to right when `mirror` (an underside).
    pub(crate) fn paint(
        &self,
        index: usize,
        size: u32,
        rect: (Vec2, Vec2),
        tint: [f32; 3],
        mirror: bool,
    ) -> (Vec<[f32; 4]>, Vec<[f32; 3]>) {
        let leaf = &self.leaves[index % self.leaves.len()];
        let (lw, lh) = leaf.color.dimensions();
        let (min, max) = rect;
        let fit = ((max.x - min.x) / lw as f32).min((max.y - min.y) / lh as f32) * size as f32;
        let tw = ((lw as f32 * fit).round() as u32).clamp(1, size);
        let th = ((lh as f32 * fit).round() as u32).clamp(1, size);
        let color = imageops::resize(&leaf.color, tw, th, FilterType::Triangle);
        let normal = imageops::resize(&leaf.normal, tw, th, FilterType::Triangle);
        let ox = ((size as f32 * 0.5 - tw as f32 * 0.5).round() as i32).max(0) as u32;
        let oy = ((max.y * size as f32).round() as i32 - th as i32).max(0) as u32;

        let tint = Vec3::from(tint);
        let n = (size * size) as usize;
        let mut px = vec![[0.0f32; 4]; n];
        let mut nrm = vec![[0.0f32, 0.0, 1.0]; n];
        for y in 0..th.min(size - oy) {
            for x in 0..tw.min(size - ox) {
                let sx = if mirror { tw - 1 - x } else { x };
                let p = color.get_pixel(sx, y).0;
                if p[3] <= 0.0 {
                    continue;
                }
                let c = self.recolor(Vec3::new(p[0], p[1], p[2]), tint);
                let i = ((oy + y) * size + ox + x) as usize;
                px[i] = [c.x, c.y, c.z, p[3].min(1.0)];
                let q = normal.get_pixel(sx, y).0;
                let v = |c: u8| c as f32 / 255.0 * 2.0 - 1.0;
                let nx = if mirror { -v(q[0]) } else { v(q[0]) };
                let nv = Vec3::new(nx, v(q[1]), v(q[2])).normalize_or(Vec3::Z);
                nrm[i] = nv.into();
            }
        }
        (px, nrm)
    }
}
