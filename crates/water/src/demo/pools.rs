//! The row of pools: twelve deep concrete pools on a terrace of the tarn's rim, one per
//! [`WaterOptics::PRESETS`], clearest first: iceberg meltwater, ocean, tropical lagoon,
//! alpine lake, glacier lake, clear lake, stream, algal bloom, murky pond, peat bog, swamp
//! and a silty river. Each is 2.6 m deep over the same pale floor, so the colors compare.

use engine_core::math::{DVec3, Vec2};
use engine_core::prelude::*;

use super::props;
use super::scenery::spawn_prop;
use crate::body::{WaterBed, WaterBody, WaterOptics};

/// Radius of the arc the pools stand on, meters from the area's center.
pub const ARC_RADIUS: f32 = 36.0;
/// Angle of the first pool and the step between pools (radians, `atan2(z, x)`).
const FIRST: f32 = 268.0 * std::f32::consts::PI / 180.0;
const STEP: f32 = 8.0 * std::f32::consts::PI / 180.0;
/// Half the side of a pool's water, meters.
pub const HALF: f32 = 1.6;
/// Wall thickness and the paving round each pool, meters.
const WALL: f32 = 0.25;
const APRON: f32 = 0.8;
/// Water depth, and how far below the paving the water stands.
pub const DEPTH: f32 = 2.6;
const FREEBOARD: f32 = 0.15;
/// The terrace's ground level (meters above the area's center): flat under the row.
pub const TERRACE: f32 = 6.45;
/// The paving's top over the terrace.
const PAVING: f32 = 0.06;

/// Each pool: its center (local x, z), its yaw (its sides along and across the arc) and its
/// preset.
pub fn pools() -> impl Iterator<Item = (Vec2, f32, &'static str)> {
    WaterOptics::PRESETS.iter().enumerate().map(|(k, &name)| {
        let a = FIRST + STEP * k as f32;
        (Vec2::new(a.cos(), a.sin()) * ARC_RADIUS, -a, name)
    })
}

/// The pool around `p` (local x, z): its index and `p` in its own frame.
fn pool_at(p: Vec2) -> Option<(usize, Vec2)> {
    let reach = HALF + WALL + APRON;
    pools().enumerate().find_map(|(k, (c, yaw, _))| {
        let d = p - c;
        // Turned into the pool's frame (x radial, z along the arc); `yaw` is minus its angle.
        let (s, co) = (-yaw).sin_cos();
        let l = Vec2::new(co * d.x + s * d.y, -s * d.x + co * d.y);
        (l.x.abs().max(l.y.abs()) < reach).then_some((k, l))
    })
}

/// The ground at `p` (local x, z) with the terrace and the pools cut into `h` (the hill).
pub fn shape(x: f32, z: f32, h: f32) -> f32 {
    let p = Vec2::new(x, z);
    let r = p.length();
    let theta = z.atan2(x).rem_euclid(std::f32::consts::TAU);
    let first = FIRST - STEP * 0.75;
    let last = FIRST + STEP * (WaterOptics::PRESETS.len() as f32 - 0.25);
    // The terrace: flat along the arc, blended into the hill over a few meters.
    let edge = |lo: f32, hi: f32, v: f32, w: f32| {
        let s = |e0: f32, e1: f32, x: f32| {
            let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        s(lo - w, lo, v) * (1.0 - s(hi, hi + w, v))
    };
    let along = edge(first, last, theta, 0.08);
    let across = edge(ARC_RADIUS - 4.0, ARC_RADIUS + 4.0, r, 3.0);
    let mut h = h + (TERRACE - h) * along * across;
    if let Some((_, l)) = pool_at(p) {
        let m = l.x.abs().max(l.y.abs());
        h = if m < HALF {
            TERRACE - FREEBOARD - DEPTH
        } else {
            // Under the walls and paving (a little below their top, so the hill's mesh
            // never shows through them).
            TERRACE + PAVING - 0.02
        };
    }
    h
}

/// Whether the hill's mesh leaves out ground at `p`: inside a pool's walls (the pool's own
/// floor and walls close the hole; the paving round it covers the mesh's ragged edge).
pub fn hole(p: Vec2) -> bool {
    pool_at(p).is_some_and(|(_, l)| l.x.abs().max(l.y.abs()) < HALF + WALL)
}

/// Build the pools around the area's center `at` (on the ground).
pub fn spawn_pools(commands: &mut Commands, at: DVec3) {
    let liner = props::pool_liner(HALF, WALL, APRON, DEPTH + FREEBOARD, PAVING);
    for (c, yaw, name) in pools() {
        let ground = at + DVec3::new(c.x as f64, TERRACE as f64, c.y as f64);
        let turn = Quat::from_rotation_y(yaw);
        spawn_prop(
            commands,
            "Pool",
            Transform::from_translation(ground).with_rotation(turn),
            liner.clone(),
        );
        let optics = WaterOptics::preset(name).unwrap_or_default();
        commands.spawn((
            Name::new(format!("Pool: {name}")),
            Transform::from_translation(ground - DVec3::Y * FREEBOARD as f64).with_rotation(turn),
            WaterBody::pool(HALF, HALF, DEPTH)
                .with_bed(WaterBed::Flat { depth_m: DEPTH })
                .with_optics(optics),
        ));
    }
}
