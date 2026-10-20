//! The [`ForestCover`](engine_world::ForestCover) query for terrains: how wooded the ground is
//! at any point, by the rules the terrain shader paints its woods with (`terrain.wgsl`, from
//! "Woods: the forest floor up close" to the material weights), so trees planted from it stand
//! where the ground shows forest.
//!
//! The shader works per pixel from per-vertex climate; this works per point from the same
//! fields at full detail (no distance fades, level ground's curvature). Its noise is a port of
//! the shader's: `hash13`, `pnoise3` and `tile_fbm` on world positions tiled every 4096 m, in
//! `f32`.

use engine_core::math::DVec3;
use engine_world::ForestSample;

use crate::chunk::{Sample, patches, variety};
use crate::ground::GroundTerrain;

/// Smallest relief (m) a forest lookup reads: woods stand on the lay of the land, so slopes
/// are measured over this distance (not a boulder's flank).
const DETAIL_M: f64 = 4.0;

/// The paint layer of woods' floor ("leaves", see [`crate::edits::PAINT_NAMES`]).
const LEAVES: u8 = 10;

/// Dave Hoskins' hash without sine (`lib_types.wgsl`).
fn hash13(p: [f32; 3]) -> f32 {
    let mut p3 = p.map(|v| fract(v * 0.1031));
    let d = p3[0] * (p3[2] + 31.32) + p3[1] * (p3[1] + 31.32) + p3[2] * (p3[0] + 31.32);
    p3 = p3.map(|v| v + d);
    fract((p3[0] + p3[1]) * p3[2])
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Value noise whose lattice repeats every `period` cells (`pnoise3` in the shader).
fn pnoise3(p: [f32; 3], period: f32) -> f32 {
    let i = p.map(f32::floor);
    let u = std::array::from_fn::<f32, 3, _>(|k| {
        let f = p[k] - i[k];
        f * f * (3.0 - 2.0 * f)
    });
    let i0 = i.map(|v| v - period * (v / period).floor());
    let i1 = i0.map(|v| (v + 1.0) - period * ((v + 1.0) / period).floor());
    let h = |x: f32, y: f32, z: f32| hash13([x, y, z]);
    let a = mix(
        mix(h(i0[0], i0[1], i0[2]), h(i1[0], i0[1], i0[2]), u[0]),
        mix(h(i0[0], i1[1], i0[2]), h(i1[0], i1[1], i0[2]), u[0]),
        u[1],
    );
    let b = mix(
        mix(h(i0[0], i0[1], i1[2]), h(i1[0], i0[1], i1[2]), u[0]),
        mix(h(i0[0], i1[1], i1[2]), h(i1[0], i1[1], i1[2]), u[0]),
        u[1],
    );
    mix(a, b, u[2])
}

/// Fractal sum of [`pnoise3`] from `freq` cycles per meter, about 0..1 (`tile_fbm`).
fn tile_fbm(tp: [f32; 3], freq: f32, octaves: i32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut f = freq;
    for i in 0..octaves {
        let o = i as f32 * 17.0;
        sum += amp * pnoise3(tp.map(|v| v * f + o), 4096.0 * f);
        f *= 2.0;
        amp *= 0.5;
    }
    sum / (1.0 - 2.0 * amp)
}

/// The shader's tiling coordinates for a world point: the position modulo the 4096 m tile
/// (`tiling_pos`). Every pattern used here repeats with the tile, so any tile origin works.
fn tiling_pos(p: DVec3) -> [f32; 3] {
    p.to_array().map(|v| v.rem_euclid(4096.0) as f32)
}

/// Woods from a terrain sample at full detail: canopy cover and needle share, both 0..1.
/// `slope` gives 1 − cos of the ground's slope angle (asked only where woods might grow: it
/// costs two more terrain samples), `tp` is the tiling position.
pub(crate) fn woods(
    s: &Sample,
    variety: f32,
    patches: f32,
    slope: impl FnOnce() -> f32,
    rock_slope: f32,
    tp: [f32; 3],
) -> ForestSample {
    let moisture = s.moisture;
    let temp_c = s.temperature_c;
    let altitude = s.height as f32;
    let macro_v = tile_fbm(tp, 1.0 / 64.0, 3);
    let mid_v = tile_fbm(tp, 1.0 / 8.0, 3);
    let patch_v = tile_fbm(tp, 1.0 / 32.0, 3);
    let meadow_v = tile_fbm(tp, 1.0 / 512.0, 3);

    // Natural woods on the moist side of the patches, and gallery woods along river banks.
    let forest_edge = (mid_v - 0.5) * 0.08 + (patch_v - 0.5) * 0.06 + (macro_v - 0.5) * 0.12;
    let woodland = smoothstep(
        0.40,
        0.52,
        moisture * 0.6 + (0.5 - patches) * 0.9 + (variety - 0.5) * 0.4 + forest_edge,
    ) * smoothstep(-6.0, 4.0, temp_c)
        * (1.0 - smoothstep(1800.0, 2600.0, altitude + (variety - 0.5) * 600.0));
    let gallery = smoothstep(
        0.3,
        0.65,
        s.banks + (patch_v - 0.5) * 0.25 + (mid_v - 0.5) * 0.15,
    ) * smoothstep(-6.0, 4.0, temp_c)
        * (1.0 - smoothstep(1800.0, 2600.0, altitude));
    let veg = smoothstep(-8.0, 6.0, temp_c);
    let mut density = woodland.max(gallery).min(veg);
    let desert = (1.0 - smoothstep(0.08, 0.22, moisture + (variety - 0.5) * 0.1))
        * smoothstep(12.0, 22.0, temp_c);
    // Painted ground (a level's paths, fields and yards) takes the place of the natural
    // ground, and painted leaves are a level's woods; snow and mud lie on top.
    let (painted, leaves) = match s.paint {
        (l, w) if l >= 3 => (saturate(w), if l == LEAVES { saturate(w) } else { 0.0 }),
        _ => (0.0, 0.0),
    };
    density *= 1.0 - painted;
    // Rivers and lakes.
    density *= 1.0 - smoothstep(0.3, 0.6, s.water);
    // Needle trees in the cold: up mountains and toward the poles, mixed woods in between.
    let conifer = smoothstep(
        11.0,
        3.0,
        temp_c + (variety - 0.5) * 5.0 + (macro_v - 0.5) * 3.0 + (moisture - 0.5) * 2.0,
    );
    if density * (1.0 - desert) <= 0.0 {
        return ForestSample {
            density: leaves,
            conifer,
        };
    }

    // No woods on beaches, in deserts, on scree or on rock.
    let slope = slope();
    let nu = (1.0 - slope).max(0.05);
    let shore_tan = (1.0 - nu * nu).max(0.0).sqrt() / nu;
    let beach_w = mix(40.0, 160.0, saturate((meadow_v - 0.35) * 2.5));
    let beach_top = (shore_tan * beach_w).clamp(0.8, 5.0);
    let beach_h = altitude + (mid_v - 0.5) * 0.5 * beach_top;
    let beach = (1.0 - smoothstep(beach_top * 0.55, beach_top * 1.15, beach_h))
        * if altitude >= -1.0 { 1.0 } else { 0.0 };
    density *= 1.0 - saturate(beach + desert);
    let veg_hold =
        0.03 + 0.06 * (1.0 - smoothstep(1400.0, 2600.0, altitude + (variety - 0.5) * 600.0));
    let outcrop = (macro_v - 0.5) * 0.4 + (mid_v - 0.5) * 0.12;
    let rock_slope = rock_slope + veg_hold + outcrop;
    let alpine = smoothstep(2100.0, 3000.0, altitude + (variety - 0.5) * 800.0);
    let rock = saturate(smoothstep(rock_slope - 0.08, rock_slope + 0.05, slope) + alpine * 0.8);
    let scree = (1.0 - rock)
        * saturate(smoothstep(rock_slope - 0.14, rock_slope - 0.04, slope) * 0.7 + alpine * 0.6);
    density *= (1.0 - scree) * (1.0 - rock);
    ForestSample {
        density: saturate(density + leaves),
        conifer,
    }
}

impl GroundTerrain {
    /// Woods on the ground below (or above) a world point; `None` on worlds without plants,
    /// under the sea and where the ground is open.
    pub fn forest(&self, p: DVec3) -> Option<ForestSample> {
        if !self.palette.vegetation {
            return None;
        }
        let local = self.rotation.inverse() * (p - self.position);
        let (dir, up, base) = self.source.map.locate(local)?;
        let s = self.source.sample(base, up, dir, DETAIL_M);
        if self.source.shape.ocean && s.height < 0.0 {
            return None;
        }
        let point_local = base + up * s.height;
        let slope = || {
            (1.0 - self
                .local_normal(base, up, point_local, DETAIL_M, DETAIL_M)
                .dot(up)) as f32
        };
        let spacing = 1.0;
        let w = woods(
            &s,
            variety(&self.source, dir, spacing),
            patches(&self.source, dir, spacing),
            slope,
            self.palette.rock_slope,
            tiling_pos(self.position + self.rotation * point_local),
        );
        (w.density > 0.0).then_some(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules above are a copy of the shader's: when one changes, the other must too.
    #[test]
    fn the_shader_still_paints_woods_by_the_rules_ported_here() {
        let wgsl = include_str!("terrain.wgsl");
        let lib = include_str!("../../render/src/shaders/lib_types.wgsl");
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let (wgsl, lib) = (squash(wgsl), squash(lib));
        for line in [
            "let forest_edge = (mid_v - 0.5) * 0.08 * clump_fade + (patch_v - 0.5) * 0.06 + (macro_v - 0.5) * 0.12 * macro_fade;",
            "smoothstep(0.40 - 0.1 * far - 0.12 * orbit, 0.52 + 0.1 * far + 0.15 * orbit, moisture * 0.6 + (0.5 - patches) * 0.9 + (variety - 0.5) * 0.4 + forest_edge + gully * 0.12 - crest * 0.12",
            "* smoothstep(-6.0, 4.0, temp_c) * (1.0 - smoothstep(1800.0, 2600.0, altitude + (variety - 0.5) * 600.0));",
            "* smoothstep(0.3, 0.65, in.water.z + (patch_v - 0.5) * 0.25 + (mid_v - 0.5) * 0.15 * clump_fade)",
            "let veg = vegetation * smoothstep(-8.0, 6.0, temp_c);",
            "let beach_top = clamp(shore_tan * beach_w, 0.8, 5.0);",
            "let desert = vegetation * (1.0 - smoothstep(0.08, 0.22, moisture + (variety - 0.5) * 0.1",
            "let alpine = smoothstep(2100.0, 3000.0, altitude + (variety - 0.5) * 800.0) * vegetation;",
            "w[M_FOREST] = min(woods, veg);",
            "for (var i = 0u; i < MAT_COUNT; i++) { w[i] *= 1.0 - paint_base; }",
            "case 10u: { return M_FOREST; }",
            "let macro_v = tile_fbm(tp, 1.0 / 64.0, 3);",
            "let mid_v = tile_fbm(tp, 1.0 / 8.0, 3);",
            "let patch_v = tile_fbm(tp, 1.0 / 32.0, 3);",
            "let meadow_v = tile_fbm(tp, 1.0 / 512.0, 3);",
            "sum += amp * pnoise3(tp * f + f32(i) * 17.0, 4096.0 * f);",
            "let i1 = (i0 + 1.0) - period * floor((i0 + 1.0) / period);",
        ] {
            assert!(wgsl.contains(&squash(line)), "terrain.wgsl changed: {line}");
        }
        for line in [
            "var p3 = fract(p * 0.1031);",
            "p3 += dot(p3, p3.zyx + 31.32);",
            "return fract((p3.x + p3.y) * p3.z);",
        ] {
            assert!(lib.contains(&squash(line)), "hash13 changed: {line}");
        }
    }

    #[test]
    fn noise_is_in_range_and_tiles_every_4096_m() {
        for k in 0..200 {
            let p = [k as f32 * 37.3, k as f32 * 3.1, 4000.0 - k as f32 * 19.7];
            let n = tile_fbm(p, 1.0 / 32.0, 3);
            assert!((-0.01..=1.01).contains(&n), "{n}");
            let h = hash13(p);
            assert!((0.0..1.0).contains(&h));
        }
        let a = tiling_pos(DVec3::new(-10.0, 5.0, 8192.0 + 3.0));
        assert_eq!(a, [4086.0, 5.0, 3.0]);
        // Across the tile seam the pattern runs on smoothly.
        let near = |x: f32| tile_fbm([x, 10.0, 10.0], 1.0 / 8.0, 3);
        assert!((near(4095.9) - near(0.0)).abs() < 0.02);
    }

    fn sample(moisture: f32, temp: f32, height: f64) -> Sample {
        Sample {
            height,
            moisture,
            temperature_c: temp,
            mare: 0.0,
            water: 0.0,
            lake: 0.0,
            banks: 0.0,
            paint: (0, 0.0),
            tint: [0.0; 4],
        }
    }

    #[test]
    fn woods_grow_where_it_is_moist_and_mild_and_give_way_to_cold_heights_and_cliffs() {
        let tp = [100.0, 0.0, 200.0];
        let wet = woods(&sample(0.9, 10.0, 300.0), 0.6, 0.2, || 0.0, 0.3, tp);
        assert!(wet.density > 0.9, "{wet:?}");
        let dry = woods(&sample(0.1, 20.0, 300.0), 0.4, 0.8, || 0.0, 0.3, tp);
        assert_eq!(dry.density, 0.0);
        let high = woods(&sample(0.9, 2.0, 3200.0), 0.6, 0.2, || 0.0, 0.3, tp);
        assert_eq!(high.density, 0.0);
        let cliff = woods(&sample(0.9, 10.0, 300.0), 0.6, 0.2, || 0.9, 0.3, tp);
        assert_eq!(cliff.density, 0.0);
        let beach = woods(&sample(0.9, 10.0, 0.2), 0.6, 0.2, || 0.0, 0.3, tp);
        assert_eq!(beach.density, 0.0);
        let mut road = sample(0.9, 10.0, 300.0);
        road.paint = (9, 1.0);
        assert_eq!(woods(&road, 0.6, 0.2, || 0.0, 0.3, tp).density, 0.0);
        // A level's painted woods are woods, also where the land is too dry or steep for them.
        let mut painted = sample(0.1, 20.0, 300.0);
        painted.paint = (LEAVES, 0.8);
        assert_eq!(woods(&painted, 0.4, 0.8, || 0.9, 0.3, tp).density, 0.8);
        let mut banks = sample(0.2, 14.0, 300.0);
        banks.banks = 0.9;
        assert!(woods(&banks, 0.4, 0.8, || 0.0, 0.3, tp).density > 0.8);
        // Needles in the cold, leaves in the warm.
        let cold = woods(&sample(0.9, 1.0, 1500.0), 0.5, 0.2, || 0.0, 0.3, tp);
        let warm = woods(&sample(0.9, 16.0, 100.0), 0.5, 0.2, || 0.0, 0.3, tp);
        assert!(
            cold.conifer > 0.9 && warm.conifer < 0.1,
            "{cold:?} {warm:?}"
        );
    }
}
