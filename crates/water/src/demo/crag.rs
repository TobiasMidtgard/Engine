//! The crag across the tarn from the outlet: a rock massif rising out of a cove, with a
//! crevice cut down its face. A spring in a basin near the top spills down the crevice in
//! tiers: a free fall onto a ledge pool, white rapids down a slab, a curtain that meets the
//! face below its brink and slides down it, and a last free fall into the cove.

use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use engine_core::math::noise;
use engine_core::prelude::*;

use super::{LAKE_LEVEL, mix, shore_radius, smooth};
use crate::flow::{FlowBed, FlowFill, FlowRegion, FlowSource};

/// From the tarn's center out to the crag (local x, z), and across it.
pub const CRAG: Vec2 = Vec2::new(-FRAC_1_SQRT_2, -FRAC_1_SQRT_2);
const SIDE: Vec2 = Vec2::new(FRAC_1_SQRT_2, -FRAC_1_SQRT_2);
/// Turn of the cascade's flow grid about +Y: its x runs up the crevice ([`CRAG`]), its z
/// across ([`SIDE`]).
const YAW: f32 = 0.75 * PI;

/// The crevice's bed: height (above the ground plane) at distance `u` up from the foot of
/// the crag (see [`frame`]), straight between these points. Pools sit on the ledges behind
/// low rims.
const PROFILE: [(f32, f32); 27] = [
    // The cove: a bay of the tarn at the foot of the crag.
    (-6.0, 3.0),
    (-1.0, 3.3),
    (1.38, 3.9),
    // A last free fall of 5 m from the lowest ledge's pool into the cove.
    (1.5, 9.05),
    (1.8, 9.05),
    (2.1, 8.8),
    (2.65, 8.8),
    (2.95, 9.05),
    // A face at 55° under a short sheer brink: the curtain off the pool above meets it and
    // slides down it.
    (4.75, 11.6),
    (4.85, 12.2),
    (5.25, 12.25),
    (5.5, 12.0),
    (6.0, 12.0),
    (6.3, 12.3),
    // A slab at 37°: white rapids.
    (10.4, 15.3),
    (10.65, 15.35),
    (10.9, 15.12),
    (11.25, 15.12),
    (11.4, 15.3),
    // A sheer 3.7 m drop from the spring's basin.
    (11.5, 19.05),
    (11.75, 19.05),
    (12.0, 18.8),
    (13.8, 18.8),
    (14.3, 19.3),
    // Its back wall, and the top of the crag.
    (15.2, 20.6),
    (17.5, 23.0),
    (60.0, 23.0),
];

/// The ledge pools and the spring's basin, `u` along the crevice.
const POOLS: [f32; 4] = [2.4, 5.75, 11.05, 12.9];
/// The spring, `u` along the crevice, and what it gives, m³/s.
const SPRING_AT: f32 = 12.9;
const SPRING_RATE: f32 = 0.2;

/// The cascade's flow grid: `u` from −3 to 17 m, 4 m to each side, 25 cm cells.
const GRID_FROM: Vec2 = Vec2::new(-3.0, -4.0);
const GRID_CELLS: [u32; 2] = [80, 32];
const GRID_CELL: f32 = 0.25;

/// The square (local x, z) round the whole crag that has a finer ground mesh of its own
/// (`demo::crag_mesh`), on the hill mesh's half-meter grid.
/// Its edge lies where the hill is gentle again: on the crag's steep faces the two meshes would
/// sample its color bands differently and show a seam.
pub const PATCH: (Vec2, Vec2) = (Vec2::new(-58.5, -58.5), Vec2::new(-6.5, -6.5));

/// Where the crag meets the tarn's shore, meters out from the tarn's center.
pub fn foot() -> f32 {
    shore_radius(CRAG.y.atan2(CRAG.x)) + 2.5
}

/// Local `x`, `z` as (`u` up the crevice from the crag's foot, `a` across it).
pub fn frame(x: f32, z: f32) -> (f32, f32) {
    let p = Vec2::new(x, z);
    (p.dot(CRAG) - foot(), p.dot(SIDE))
}

/// Local x, z of the point `u` up the crevice and `a` across it.
pub fn point(u: f32, a: f32) -> Vec2 {
    CRAG * (foot() + u) + SIDE * a
}

/// The crevice's bed at `u` (see [`PROFILE`]).
fn bed(u: f32) -> f32 {
    let k = PROFILE.partition_point(|p| p.0 <= u);
    if k == 0 {
        return PROFILE[0].1;
    }
    if k == PROFILE.len() {
        return PROFILE[k - 1].1;
    }
    let ((u0, h0), (u1, h1)) = (PROFILE[k - 1], PROFILE[k]);
    mix(h0, h1, (u - u0) / (u1 - u0))
}

/// The crevice's floor at `u` along it and `a` across: [`bed`], but no brink is a level weir.
/// Each lip runs ragged and aslant across the crevice, and is worn down in the middle, so
/// the water gathers into a tongue there. (Not the brink over the 55° face: water any faster
/// over it would clear the face instead of sliding down it.)
fn floor(u: f32, a: f32) -> f32 {
    let shaped = 1.0 - smooth(3.3, 3.8, u) * (1.0 - smooth(6.0, 6.6, u));
    let ragged = 0.3 * noise::perlin2(a as f64 * 0.8, u as f64 * 0.21, 0xC4AC) + 0.08 * a;
    let worn = 0.2 * (1.0 - (a / half_width(u)).powi(2)).max(0.0);
    bed(u + ragged * shaped) - worn * shaped
}

/// Half the width of the crevice's floor: a bay at the cove, a wider basin round the spring.
fn half_width(u: f32) -> f32 {
    let course = 1.0 + 0.25 * (u * 0.8 + 0.5).sin() + 0.25 * smooth(11.5, 13.0, u);
    mix(course, 3.2, smooth(1.4, -1.5, u))
}

/// The massif's crest line at `u`: a cliff out of the cove, then up to the summit ridge.
fn massif(u: f32) -> f32 {
    if u < 1.5 {
        mix(LAKE_LEVEL - 1.0, 10.8, smooth(-0.6, 1.5, u))
    } else {
        10.8 + 12.7 * ((u - 1.5) / 12.5).clamp(0.0, 1.0).powf(0.75) + 0.8 * smooth(14.0, 20.0, u)
    }
}

/// The crag raised on the hill's height `h` at local `x`, `z`, with the crevice cut into it.
pub fn shape(x: f32, z: f32, h: f32) -> f32 {
    let (u, a) = frame(x, z);
    if !(-8.0..42.0).contains(&u) || a.abs() > 30.0 {
        return h;
    }
    let n = noise::fbm2(x as f64 * 0.11, z as f64 * 0.11, 0xC4A6, 4, 2.0, 0.5);
    let fine = noise::fbm2(x as f64 * 0.6, z as f64 * 0.6, 0xC4A7, 3, 2.0, 0.5);
    let ribs = ridged(x as f64 * 0.07, z as f64 * 0.07, 0xC4A8, 4);
    // The massif: rough rock with ribs and gullies, two summits flanking the crevice,
    // falling away steeply at the sides and more gently behind, standing straight out of the
    // tarn at the front. (Gentler relief low down, so the crevice keeps its walls there.)
    let relief = smooth(1.0, 6.0, u);
    let summits = 5.5
        * (-((a.abs() - 6.5) / 4.5).powi(2)).exp()
        * smooth(3.0, 11.0, u)
        * (1.0 - smooth(16.0, 28.0, u));
    let top = massif(u)
        + n * mix(0.5, 1.6, relief)
        + (ribs - 0.45) * 4.5 * relief
        + summits
        + fine * 0.35;
    // Ledges come and go along the face: where the beds are jointed and blocks have fallen,
    // the face runs on unstepped.
    let ledges = 0.35
        + 0.65
            * smooth(
                -0.25,
                0.25,
                noise::perlin2(x as f64 * 0.09, z as f64 * 0.09, 0xC4AA),
            );
    let top = mix(top, strata(x, z, top), 0.85 * relief * ledges);
    let sides = 1.0 - smooth(8.0 + n * 3.0, 22.0 + n * 3.0 + ribs * 4.0, a.abs());
    let back = 1.0 - smooth(17.0, 36.0, u + ribs * 4.0);
    let front = smooth(-1.2, 0.3, u + n * 0.6);
    let mut out = mix(h, h.max(top), sides * back * front);
    // The crevice: a stepped bed between walls rising at ~73° (rounded into the floor),
    // layered like the rest of the crag above the floor.
    let w = a.abs() - half_width(u);
    let walls = floor(u, a)
        + 0.5 * (3.2 * w + (10.24 * w * w + 0.04).sqrt())
        + fine * 0.3 * smooth(0.0, 1.0, w);
    let walls = mix(walls, strata(x, z, walls), 0.5 * smooth(0.4, 1.2, w));
    out = out.min(walls);
    out
}

/// Mean thickness of the crag's rock beds, m.
const BED: f32 = 1.7;
/// How much the beds rise per meter across the crevice (about 10°): their ledges run aslant
/// over the faces rather than round them like contour lines.
const DIP: f32 = 0.18;

/// Where height `h` at local `x`, `z` lies in the crag's beds: whole numbers are the partings
/// between beds. The beds dip, undulate and range from about 1 to 4 m thick.
pub fn bed_coord(x: f32, z: f32, h: f32) -> f32 {
    let (_, a) = frame(x, z);
    let n = noise::fbm2(x as f64 * 0.11, z as f64 * 0.11, 0xC4A6, 4, 2.0, 0.5);
    let swell = noise::fbm2(x as f64 * 0.04, z as f64 * 0.04, 0xC4A9, 2, 2.0, 0.5);
    let h = h + DIP * a;
    // A warp of the height with a slope under 1/BED keeps the beds in order: thinner where it
    // climbs, thicker where it falls.
    h / BED + 0.22 * (h * TAU / (BED * 2.3) + swell * 2.0).sin() + n * 0.4
}

/// `h` stepped into the crag's beds (see [`bed_coord`]): each bed a steep riser over a ledge,
/// so faces read as layered rock rather than smooth slopes.
fn strata(x: f32, z: f32, h: f32) -> f32 {
    let t = bed_coord(x, z, h);
    let stepped = t.floor() + smooth(0.45, 0.8, t.fract());
    // Back from bed coordinates to meters over the bed's local thickness.
    let per_m = (bed_coord(x, z, h + 0.05) - t) / 0.05;
    h + (stepped - t) / per_m.max(0.1)
}

/// Whether the ground at local `x`, `z`, height `h` with up-component `up` of its normal is
/// bare rock of the crag (faces, risers and the wet crevice) rather than soil and turf on its
/// ledges and summits.
pub fn is_cliff(x: f32, z: f32, h: f32, up: f32) -> bool {
    if presence(x, z) < 0.3 {
        return false;
    }
    let ragged = noise::fbm2(x as f64 * 0.7, z as f64 * 0.7, 0xC4AB, 3, 2.0, 0.5);
    up < 0.86 + ragged * 0.12 || wetness(x, z, h) > 0.3
}

/// How much of the crag (rather than the hill) is at local `x`, `z`, 0..1.
pub fn presence(x: f32, z: f32) -> f32 {
    let (u, a) = frame(x, z);
    smooth(-1.5, 0.5, u) * (1.0 - smooth(14.0, 24.0, a.abs())) * (1.0 - smooth(22.0, 36.0, u))
}

/// How wet the rock is at local `x`, `z` with ground height `h` (0..1): the crevice's floor
/// and the foot of its walls, where the spray reaches.
pub fn wetness(x: f32, z: f32, h: f32) -> f32 {
    let (u, a) = frame(x, z);
    if !(-3.0..15.0).contains(&u) {
        return 0.0;
    }
    let over = h - floor(u, a);
    (1.0 - smooth(0.2, 1.5, over)) * (1.0 - smooth(0.4, 1.5, a.abs() - half_width(u)))
}

/// Ridged multifractal noise, 0..1: sharp crests along the zero lines of Perlin noise (rock
/// ribs and the gullies between them).
fn ridged(x: f64, z: f64, seed: u64, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for k in 0..octaves {
        let r = 1.0 - noise::perlin2(x * f, z * f, seed + k as u64).abs();
        sum += r * r * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

/// The cascade's flow region, on the crag at `origin` (the area's center on the ground).
/// `height` is the area's ground.
pub fn cascade(origin: DVec3, height: impl Fn(f32, f32) -> f32) -> (Transform, FlowRegion) {
    let [nu, na] = GRID_CELLS;
    let mut bed = Vec::with_capacity((nu * na) as usize);
    for j in 0..na {
        for i in 0..nu {
            let local = GRID_FROM + (Vec2::new(i as f32, j as f32) + 0.5) * GRID_CELL;
            let p = point(local.x, local.y);
            bed.push(height(p.x, p.y));
        }
    }
    let at = |u: f32| (Vec2::new(u, 0.0) - GRID_FROM).to_array();
    let corner = point(GRID_FROM.x, GRID_FROM.y);
    (
        Transform::from_translation(origin + DVec3::new(corner.x as f64, 0.0, corner.y as f64))
            .with_rotation(Quat::from_rotation_y(YAW)),
        FlowRegion {
            cells: GRID_CELLS,
            cell_m: GRID_CELL,
            bed: FlowBed::Heights(bed),
            open_edges: true,
            friction: 0.5,
            sources: vec![FlowSource {
                at: at(SPRING_AT),
                radius_m: 0.6,
                rate_m3_s: SPRING_RATE,
            }],
            fills: POOLS.iter().map(|u| FlowFill::to_spill(at(*u))).collect(),
            prewarm_s: 25.0,
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::FlowSim;

    #[test]
    fn the_crevice_is_cut_into_the_crag() {
        for u in [2.0, 5.5, 8.0, 11.0, 13.0] {
            let floor = point(u, 0.0);
            let h = super::super::tarn_height(floor.x, floor.y);
            let want = super::floor(u, 0.0);
            assert!((h - want).abs() < 0.15, "floor at {u}: {h} vs {want}");
            // Walls a meter or more high on both sides.
            for side in [-1.0, 1.0] {
                let wall = point(u, side * (half_width(u) + 1.2));
                let hw = super::super::tarn_height(wall.x, wall.y);
                assert!(hw > h + 1.0, "wall at {u}, {side}: {hw} over a {h} floor");
            }
        }
        // All of it is in the finer mesh.
        for (u, a) in [(-3.5, -5.5), (-3.5, 5.5), (17.5, -5.5), (17.5, 5.5)] {
            let p = point(u, a);
            assert!(p.cmpgt(PATCH.0).all() && p.cmplt(PATCH.1).all(), "{p}");
        }
        // The cove is part of the tarn.
        let cove = point(0.0, 0.0);
        assert!(super::super::tarn_height(cove.x, cove.y) < LAKE_LEVEL - 1.0);
    }

    #[test]
    fn the_spring_cascades_down_the_crevice_in_tiers() {
        let (transform, region) = cascade(DVec3::ZERO, super::super::tarn_height);
        let FlowBed::Heights(bed) = region.bed.clone() else {
            unreachable!()
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, transform.translation, &region, bed);
        sim.yaw = YAW;
        for _ in 0..30 * 25 {
            sim.step(1.0 / 30.0);
        }
        // A lip at the top of each drop too steep to run down: the basin's sheer drop, the
        // 55° face, the last fall into the cove.
        let lips = sim.lips(0.5);
        let tier = |from: f32, to: f32| {
            lips.iter()
                .filter(|l| {
                    let (u, _) = frame(l.top.x as f32, l.top.z as f32);
                    (from..to).contains(&u)
                })
                .map(|l| l.q * l.width)
                .sum::<f32>()
        };
        for (from, to) in [(11.0, 12.0), (4.6, 5.6), (1.2, 2.0)] {
            let q = tier(from, to);
            assert!(
                q > SPRING_RATE * 0.4,
                "{q} m³/s over the drop at {from}..{to} m"
            );
        }
        // Water over the 55° face meets it and slides down it to the pool below.
        let face: Vec<_> = lips
            .iter()
            .filter(|l| (4.6..5.6).contains(&frame(l.top.x as f32, l.top.z as f32).0))
            .collect();
        for lip in face {
            let path = sim.fall_path(lip, 16);
            assert!(path.iter().any(|p| p.rock != Vec3::ZERO), "{path:?}");
            let end = path.last().unwrap();
            let (u, _) = frame(end.pos.x as f32, end.pos.z as f32);
            assert!((1.9..3.1).contains(&u), "lands {u} m up");
        }
        // The rapids between run fast.
        let rapids = point(8.3, 0.0);
        let (_, depth, v) = sim
            .sample(DVec3::new(rapids.x as f64, 0.0, rapids.y as f64))
            .expect("water on the rapids");
        assert!(
            depth < 0.2 && v.length() > 1.5,
            "{depth} m deep at {} m/s",
            v.length()
        );
    }
}
