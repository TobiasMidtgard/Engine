//! Real water for a landscape that already has its lake basin and stream beds (the showcase
//! valley): a lake body over the ground, and a flowing region along each stream from its spring
//! to the lake, so waterfalls, rapids and the lake's inflow come from the simulation. A jetty
//! with moored boats and buoys can stand on the shore.

use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;

use super::DemoView;
use super::scenery::{Jetty, spawn_buoy, spawn_jetty, spawn_motorboat, spawn_rowboat};
use crate::body::{WaterBed, WaterBody, WaterOptics};
use crate::buoyancy::Tether;
use crate::flow::{FlowBed, FlowFill, FlowRegion, FlowSource};

/// A stream laid into the landscape.
#[derive(Debug, Clone, Copy)]
pub struct ValleyStream<'a> {
    /// Its course (world x, z), spring first, ending in the lake.
    pub path: &'a [[f64; 2]],
    /// Channel width, meters.
    pub width_m: f32,
    /// Water the spring gives, m³/s.
    pub flow_m3_s: f32,
}

/// A landscape lake and the streams that feed it.
#[derive(Clone, Copy)]
pub struct Valley<'a> {
    /// Middle of the lake, at its still surface.
    pub lake_center: DVec3,
    /// Reaches past the shore everywhere (the terrain hides the water under it).
    pub lake_radius: f32,
    pub streams: &'a [ValleyStream<'a>],
    /// Ground height at world (x, z): finds the shore and stands the jetty's piles.
    pub ground: &'a dyn Fn(f64, f64) -> f64,
    /// Where a jetty stands, as a bearing from the lake's middle (radians from +x toward +z):
    /// it runs from the bank there out over the water, with a rowboat and a motorboat moored
    /// alongside and buoys beyond its end.
    pub jetty_bearing: Option<f64>,
}

/// Length of the valley jetty, meters.
pub const JETTY_LENGTH: f32 = 18.0;

/// Cell size of a stream's flow grid, meters: fine enough for a 5 m channel to be several
/// cells wide, coarse enough for a few hundred meters of stream to cost little.
pub const STREAM_CELL: f32 = 0.75;
/// Priming springs go this far apart down a stream's course, meters.
const PRIMING_STEP: f64 = 25.0;

/// Fill `valley` with water: the lake as a body over the ground, each stream as a flowing
/// region that already runs into the lake when the world starts.
pub fn spawn_valley(commands: &mut Commands, valley: &Valley) {
    commands.spawn((
        Name::new("Valley lake"),
        Transform::from_translation(valley.lake_center),
        WaterBody::lake(valley.lake_radius)
            .with_bed(WaterBed::Ground)
            .with_optics(WaterOptics::clear_lake()),
    ));
    for (k, stream) in valley.streams.iter().enumerate() {
        if let Some((corner, region)) = stream_region(stream, valley.lake_center.y) {
            commands.spawn((
                Name::new(format!("Valley stream {k}")),
                Transform::from_translation(corner),
                region,
            ));
        }
    }
    if let Some(jetty) = valley.jetty_bearing.and_then(|b| valley_jetty(valley, b)) {
        spawn_jetty(commands, &jetty);
        moor_boats(commands, &jetty, valley.ground);
    }
}

/// The jetty on the bank at `bearing`: from the first point out from the lake's middle that
/// stands above the water, back out over the lake. `None` if no bank is found.
pub fn valley_jetty<'a>(valley: &Valley<'a>, bearing: f64) -> Option<Jetty<'a>> {
    let level = valley.lake_center.y;
    let middle = DVec2::new(valley.lake_center.x, valley.lake_center.z);
    let out = DVec2::new(bearing.cos(), bearing.sin());
    let ground = |p: DVec2| (valley.ground)(p.x, p.y);
    let mut r = 0.0;
    while ground(middle + out * r) < level + 0.3 {
        r += 0.5;
        if r > valley.lake_radius as f64 {
            return None;
        }
    }
    let shore = middle + out * r;
    // Boards a hand above the bank at the shore end, well clear of the waves further out.
    let deck = (level + 0.65).max(ground(shore) + 0.08);
    Some(Jetty {
        shore: DVec3::new(shore.x, deck, shore.y),
        dir: -out,
        length: JETTY_LENGTH,
        water_level: level,
        bed: valley.ground,
        seed: 0x7E77,
    })
}

/// Close-ups of the valley's jetty for tours (eye and target relative to the lake's middle):
/// from the bank looking out along the deck with a boat moored either side, and from the water
/// beside the rowboat. Empty without a jetty.
pub fn valley_views(valley: &Valley) -> Vec<DemoView> {
    let Some(jetty) = valley.jetty_bearing.and_then(|b| valley_jetty(valley, b)) else {
        return Vec::new();
    };
    let level = valley.lake_center.y;
    let along = jetty.length as f64;
    let rel = |p: DVec3, y: f64| (DVec3::new(p.x, y, p.z) - valley.lake_center).to_array();
    let bank = jetty.point(-5.0, -4.5);
    let bank_y = ((valley.ground)(bank.x, bank.z) + 1.8).max(jetty.shore.y + 2.8);
    vec![
        DemoView {
            key: "lake_jetty",
            name: "Jetty with a moored rowboat and motorboat",
            eye: rel(bank, bank_y),
            target: rel(jetty.point(along - 4.0, 1.0), level + 0.3),
            hour: 16.5,
            warmup_secs: 4.0,
        },
        DemoView {
            key: "lake_boats",
            name: "Rowboat riding the lake's waves at the jetty",
            eye: rel(jetty.point(along + 4.0, 8.0), level + 1.6),
            target: rel(jetty.point(along - 3.5, 1.5), level + 0.4),
            hour: 16.5,
            warmup_secs: 4.0,
        },
    ]
}

/// A rowboat and a motorboat tied up either side of the jetty's end, and three buoys beyond.
fn moor_boats(commands: &mut Commands, jetty: &Jetty, ground: &dyn Fn(f64, f64) -> f64) {
    let level = jetty.water_level;
    let on_water = |p: DVec3| DVec3::new(p.x, level, p.z);
    let along = jetty.length as f64;
    let d = jetty.out();
    let yaw = (-d.y).atan2(d.x) as f32;
    spawn_rowboat(
        commands,
        on_water(jetty.point(along - 2.5, 2.4)),
        yaw,
        jetty.point(along - 2.5, 0.9),
    );
    let motorboat = spawn_motorboat(
        commands,
        on_water(jetty.point(along - 7.0, -3.0)),
        yaw + std::f32::consts::PI,
    );
    commands.entity(motorboat).insert(Tether {
        anchor: jetty.point(along - 7.0, -0.9),
        length_m: 3.0,
        stiffness: 1400.0,
    });
    for k in 0..3 {
        let p = on_water(jetty.point(along + 7.0 + k as f64 * 5.0, (k as f64 - 1.0) * 6.0));
        let bottom = DVec3::new(p.x, ground(p.x, p.z), p.z);
        spawn_buoy(commands, p, bottom, k);
    }
}

/// The flow region covering `stream`'s course: its corner (at height `base`) and settings.
pub fn stream_region(stream: &ValleyStream, base: f64) -> Option<(DVec3, FlowRegion)> {
    let first = stream.path.first()?;
    let margin = stream.width_m as f64 + 6.0;
    let (mut lo, mut hi) = (DVec2::from_array(*first), DVec2::from_array(*first));
    for p in stream.path {
        lo = lo.min(DVec2::from_array(*p));
        hi = hi.max(DVec2::from_array(*p));
    }
    lo -= DVec2::splat(margin);
    hi += DVec2::splat(margin);
    let cell = STREAM_CELL as f64;
    let cells = ((hi - lo) / cell).ceil();
    let length: f64 = stream
        .path
        .windows(2)
        .map(|w| DVec2::from_array(w[0]).distance(DVec2::from_array(w[1])))
        .sum();
    let spring = DVec2::from_array(*first) - lo;
    // Pools along the course (a plunge pool, hollows in the bed) start full, as the running
    // stream leaves them; seeds that aren't in a hollow fill nothing.
    let mut fills = Vec::new();
    for w in stream.path.windows(2) {
        let (a, b) = (DVec2::from_array(w[0]), DVec2::from_array(w[1]));
        let steps = (a.distance(b) / 2.0).ceil().max(1.0) as usize;
        for k in 0..steps {
            let p = a.lerp(b, k as f64 / steps as f64) - lo;
            fills.push(FlowFill::to_spill([p.x as f32, p.y as f32]));
        }
    }
    // While it primes, springs every 25 m down the course fill it all at once.
    let mut priming = Vec::new();
    let mut along = 0.0;
    for w in stream.path.windows(2) {
        let (a, b) = (DVec2::from_array(w[0]), DVec2::from_array(w[1]));
        let len = a.distance(b);
        let mut next = (along / PRIMING_STEP).floor() * PRIMING_STEP + PRIMING_STEP;
        while next < along + len {
            let p = a.lerp(b, (next - along) / len) - lo;
            priming.push(FlowSource {
                at: [p.x as f32, p.y as f32],
                radius_m: (stream.width_m * 0.4).max(STREAM_CELL),
                rate_m3_s: stream.flow_m3_s,
            });
            next += PRIMING_STEP;
        }
        along += len;
    }
    Some((
        DVec3::new(lo.x, base, lo.y),
        FlowRegion {
            cells: [cells.x as u32, cells.y as u32],
            cell_m: STREAM_CELL,
            bed: FlowBed::Ground,
            sources: vec![FlowSource {
                at: [spring.x as f32, spring.y as f32],
                radius_m: (stream.width_m * 0.4).max(STREAM_CELL),
                rate_m3_s: stream.flow_m3_s,
            }],
            fills,
            priming,
            // The banks keep the water in; the lake (a body over the region) takes it at the end.
            open_edges: false,
            friction: 0.35,
            // Long enough for the water to fill the course and run into the lake.
            prewarm_s: (length as f32 * 0.9).clamp(30.0, 400.0),
            optics: WaterOptics::stream(),
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_region_covers_its_course() {
        let path = [[262.0, -440.0], [250.0, -400.0], [236.0, -235.0]];
        let s = ValleyStream {
            path: &path,
            width_m: 5.0,
            flow_m3_s: 0.4,
        };
        let (corner, region) = stream_region(&s, 0.0).expect("region");
        let size = DVec2::new(
            region.cells[0] as f64 * region.cell_m as f64,
            region.cells[1] as f64 * region.cell_m as f64,
        );
        for p in path {
            let local = DVec2::from_array(p) - DVec2::new(corner.x, corner.z);
            assert!(
                local.min_element() > 5.0 && (size - local).min_element() > 5.0,
                "{p:?}"
            );
        }
        // The spring is where the course starts.
        let src = region.sources[0].at;
        assert!((corner.x + src[0] as f64 - 262.0).abs() < 1e-3);
        assert!((corner.z + src[1] as f64 + 440.0).abs() < 1e-3);
        // About 210 m of course: time for the primed spring to fill it.
        assert!(region.prewarm_s > 150.0);
        assert!(region.cells[0] * region.cells[1] < 40_000);
        // Hollows along the course start full.
        assert!(region.fills.iter().all(|f| f.to_spill));
        assert!(region.fills.len() > 100);
    }
}
