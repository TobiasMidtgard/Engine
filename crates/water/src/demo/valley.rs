//! Real water for a landscape that already has its lake basin and stream beds (the showcase
//! valley): a lake body over the ground, and a flowing region along each stream from its spring
//! to the lake, so waterfalls, rapids and the lake's inflow come from the simulation. A jetty
//! with moored boats and buoys can stand on the shore.

use engine_core::math::{DVec2, DVec3, Rng};
use engine_core::prelude::*;
use engine_world::physics::Collider;

use super::DemoView;
use super::scenery::{
    Jetty, merged, moor_motorboat, spawn_buoy, spawn_jetty, spawn_motorboat, spawn_prop,
    spawn_rowboat,
};
use crate::body::{WaterBed, WaterBody, WaterOptics};
use crate::flow::{FlowBed, FlowFill, FlowRegion, FlowRock, FlowSource};

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
        if let Some((corner, mut region)) =
            stream_region(stream, valley.lake_center.y, valley.ground)
        {
            let stones = stream_stones(stream, valley.ground);
            region.rocks = stones.iter().map(|s| s.rock(corner)).collect();
            spawn_stones(commands, &stones);
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

/// A stone laid in or along a valley stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamStone {
    /// Its middle, on the ground under it (world).
    pub at: DVec3,
    /// Meters across.
    pub size: f32,
    /// Its top over the ground under its middle, meters.
    pub height: f32,
    pub yaw: f32,
    /// Stretch of its rock shape (x, y, z), about 1.
    pub stretch: Vec3,
    /// Which of the demo's rock shapes it uses.
    pub shape: usize,
}

impl StreamStone {
    /// The stone in the stream's flow region whose corner is `corner`.
    pub fn rock(&self, corner: DVec3) -> FlowRock {
        FlowRock {
            at: [(self.at.x - corner.x) as f32, (self.at.z - corner.z) as f32],
            radius_m: self.radius(),
            height_m: self.height,
        }
    }

    /// Its radius in plan, meters.
    pub fn radius(&self) -> f32 {
        self.size * 0.45 * (self.stretch.x + self.stretch.z) * 0.5
    }
}

/// Where a mountain stream's bed drops this steeply (over 10 m) it runs as a cascade of
/// boulders, pools less than a channel width apart (Montgomery & Buffington 1997).
const CASCADE_SLOPE: f64 = 0.065;
/// From here to [`CASCADE_SLOPE`] it steps down over channel-spanning lines of stones, the
/// pools between them 1 to 4 widths apart; gentler, it runs over gravel and cobbles with a
/// boulder here and there (plane bed and pool-riffle reaches).
const STEP_POOL_SLOPE: f64 = 0.03;
/// Steps of a step-pool reach, channel widths apart.
const STEP_WIDTHS: f64 = 2.5;
/// A drop this big from one meter to the next is a fall: no stones on its brink or in the
/// first meters of its plunge pool.
const FALL_STEP_M: f64 = 0.5;

/// The stones of `stream` laid in the channel the landscape carved (`ground`), by the kind of
/// reach its bed's slope makes: cascades of boulders where it is steep, lines of stones
/// stepping across it where it is steeper than 3 %, a boulder here and there where it runs
/// gently, and boulders along both banks breaking up their edges all the way down.
pub fn stream_stones(stream: &ValleyStream, ground: &dyn Fn(f64, f64) -> f64) -> Vec<StreamStone> {
    let half = stream.width_m as f64 * 0.5;
    let reach = half + 4.0;
    // The channel's middle every meter down the course, smoothed (the lowest point near the
    // drawn course wanders by a cell).
    let mut raw = Vec::new();
    for w in stream.path.windows(2) {
        let (a, b) = (DVec2::from_array(w[0]), DVec2::from_array(w[1]));
        let steps = a.distance(b).ceil().max(1.0) as usize;
        for k in 0..steps {
            raw.push(in_channel(
                a.lerp(b, k as f64 / steps as f64),
                reach,
                ground,
            ));
        }
    }
    let n = raw.len();
    if n < 16 {
        return Vec::new();
    }
    let line: Vec<DVec2> = (0..n)
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(3), (i + 3).min(n - 1));
            raw[lo..=hi].iter().copied().sum::<DVec2>() / (hi - lo + 1) as f64
        })
        .collect();
    let bed: Vec<f64> = raw.iter().map(|p| ground(p.x, p.y)).collect();
    // Falls, by the meter they drop over.
    let falls: Vec<usize> = (0..n - 1)
        .filter(|&i| bed[i] - bed[i + 1] > FALL_STEP_M)
        .collect();
    let near_fall = |i: usize| falls.iter().any(|&f| i + 4 >= f && i <= f + 10);
    let seed = stream.path[0][0].to_bits() ^ stream.path[0][1].to_bits().rotate_left(17);
    let mut rng = Rng::new(seed ^ 0x570_4E5);
    let mut out = Vec::new();
    let mut place = |rng: &mut Rng, i: usize, across: f64, size: f32| {
        let i = i.min(n - 2);
        let dir = (line[i + 1] - line[i.saturating_sub(1)]).normalize_or_zero();
        let side = DVec2::new(-dir.y, dir.x);
        let p = line[i] + side * across;
        let stretch = Vec3::new(
            rng.range(0.85, 1.2),
            rng.range(0.75, 1.05),
            rng.range(0.85, 1.2),
        );
        let k = out.len();
        out.push(StreamStone {
            at: DVec3::new(p.x, ground(p.x, p.y), p.y),
            size,
            // A boulder stands about 0.42 of its size over where it is half buried.
            height: size * 0.42 * stretch.y,
            yaw: rng.range(0.0, std::f32::consts::TAU),
            stretch,
            shape: super::rock_for(size, k).0,
        });
    };
    // The banks, all the way down but for the spring and the mouth: a boulder every few
    // meters on one side or the other, half on the bed and half up the bank.
    let mut i = 3.0;
    while (i as usize) < n - 6 {
        let at = i as usize;
        if !near_fall(at) {
            let side = if rng.f32() < 0.5 { -1.0 } else { 1.0 };
            let size = rng.range(0.9, 1.8);
            let across = side * (half + rng.range(-0.3, 0.8) as f64);
            place(&mut rng, at, across, size);
        }
        i += rng.range(1.5, 3.0) as f64;
    }
    // In the channel, by reach.
    let mut i = 6.0;
    while (i as usize) < n - 8 {
        let at = i as usize;
        let (lo, hi) = (at.saturating_sub(5), (at + 5).min(n - 1));
        let slope = (bed[lo] - bed[hi]) / (hi - lo) as f64;
        if near_fall(at) {
            i += 1.0;
            continue;
        }
        if slope > CASCADE_SLOPE {
            for _ in 0..1 + (rng.f32() < 0.5) as usize {
                let size = rng.range(0.8, 1.5);
                let across = rng.range(-0.75, 0.75) as f64 * half;
                place(&mut rng, at, across, size);
            }
            i += rng.range(2.0, 3.5) as f64;
        } else if slope > STEP_POOL_SLOPE {
            // A step: stones shoulder to shoulder across the channel.
            let mut across = -half + 0.4;
            while across < half - 0.3 {
                let size = rng.range(0.7, 1.0);
                let jitter = rng.range(-0.1, 0.1) as f64;
                place(&mut rng, at, across + jitter, size);
                across += size as f64 * 0.95;
            }
            i += stream.width_m as f64 * STEP_WIDTHS * rng.range(0.8, 1.2) as f64;
        } else {
            let size = rng.range(0.8, 1.5);
            let across = rng.range(-0.7, 0.7) as f64 * half;
            place(&mut rng, at, across, size);
            if rng.f32() < 0.3 {
                let small = rng.range(0.4, 0.6);
                let beside =
                    across + rng.range(0.6, 1.0) as f64 * if across > 0.0 { -1.0 } else { 1.0 };
                place(&mut rng, at + 1, beside, small);
            }
            i += rng.range(4.0, 7.0) as f64;
        }
    }
    out
}

/// Stones in a merged mesh per stretch of stream (so stretches out of view are culled), each
/// with a sphere to stand on.
fn spawn_stones(commands: &mut Commands, stones: &[StreamStone]) {
    let rocks = super::art::rocks();
    for (k, chunk) in stones.chunks(16).enumerate() {
        let origin = chunk[0].at;
        let parts = merged(chunk.iter().map(|s| {
            let built = super::rock_for(s.size, s.shape).1;
            let m = Mat4::from_translation((s.at - origin).as_vec3() - Vec3::Y * s.size * 0.05)
                * Mat4::from_rotation_y(s.yaw)
                * Mat4::from_scale(s.stretch * (s.size / built));
            (rocks[s.shape].clone(), m)
        }));
        spawn_prop(
            commands,
            &format!("Stream stones {k}"),
            Transform::from_translation(origin),
            parts,
        );
        for s in chunk {
            let r = s.radius();
            commands.spawn((
                Name::new("Stream stone"),
                Transform::from_translation(s.at + DVec3::Y * (s.height - r) as f64),
                Collider::sphere(r),
            ));
        }
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
            quick_warmup: true,
        },
        DemoView {
            key: "lake_boats",
            name: "Rowboat riding the lake's waves at the jetty",
            eye: rel(jetty.point(along + 4.0, 8.0), level + 1.6),
            target: rel(jetty.point(along - 3.5, 1.5), level + 0.4),
            hour: 16.5,
            warmup_secs: 4.0,
            quick_warmup: true,
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
    moor_motorboat(
        commands,
        motorboat,
        jetty.point(along - 7.0, -0.9),
        3.6,
        1400.0,
    );
    for k in 0..3 {
        let p = on_water(jetty.point(along + 7.0 + k as f64 * 5.0, (k as f64 - 1.0) * 6.0));
        let bottom = DVec3::new(p.x, ground(p.x, p.z), p.z);
        spawn_buoy(commands, p, bottom, k);
    }
}

/// The lowest ground (`ground` at world x/z) within `reach` m of `p`, nearer spots preferred:
/// the bed of the channel the landscape carved there. The landscape lays its channel along a
/// smoothed curve through the course's points, which runs several meters off the straight
/// lines between them at the bends: springs and pools placed on those lines poured onto the
/// meadow beside the stream and flooded its hollows (playtest 2026-10-29 item 14).
fn in_channel(p: DVec2, reach: f64, ground: &dyn Fn(f64, f64) -> f64) -> DVec2 {
    let step = STREAM_CELL as f64;
    let n = (reach / step).ceil() as i64;
    let mut best = (ground(p.x, p.y), p);
    for j in -n..=n {
        for i in -n..=n {
            let q = p + DVec2::new(i as f64, j as f64) * step;
            let d = (q - p).length();
            if d > reach {
                continue;
            }
            let score = ground(q.x, q.y) + 0.15 * d;
            if score < best.0 {
                best = (score, q);
            }
        }
    }
    best.1
}

/// Banks this high over the bed hold a spring's water, meters.
const SPRING_BANK_M: f64 = 0.7;
/// The spring moves at most this far down its course looking for such banks, meters.
const SPRING_SEARCH_M: f64 = 40.0;

/// Where `stream`'s spring wells up: the head of its course, or, where the channel starts too
/// shallow there to hold its flow (it spilled over the meadow), the first point down the course
/// whose banks rise [`SPRING_BANK_M`] over the bed.
fn spring_at(
    stream: &ValleyStream,
    channel: &dyn Fn(DVec2) -> DVec2,
    ground: &dyn Fn(f64, f64) -> f64,
) -> DVec2 {
    let head = channel(DVec2::from_array(stream.path[0]));
    let mut along = 0.0;
    for w in stream.path.windows(2) {
        let (a, b) = (DVec2::from_array(w[0]), DVec2::from_array(w[1]));
        let len = a.distance(b);
        let across = (b - a).perp() / len.max(1e-6);
        let mut t = 0.0;
        while t < len && along + t <= SPRING_SEARCH_M {
            let c = channel(a.lerp(b, t / len));
            let bed = ground(c.x, c.y);
            let off = stream.width_m as f64 * 0.5 + 1.5;
            let bank = [-1.0, 1.0]
                .map(|side| {
                    let q = c + across * off * side;
                    ground(q.x, q.y)
                })
                .into_iter()
                .fold(f64::MAX, f64::min);
            if bank - bed >= SPRING_BANK_M {
                return c;
            }
            t += 1.0;
        }
        along += len;
    }
    head
}

/// The flow region covering `stream`'s course: its corner (at height `base`) and settings, its
/// spring, priming springs and pools placed in the channel the landscape carved (`ground`).
pub fn stream_region(
    stream: &ValleyStream,
    base: f64,
    ground: &dyn Fn(f64, f64) -> f64,
) -> Option<(DVec3, FlowRegion)> {
    let first = stream.path.first()?;
    let reach = stream.width_m as f64 * 0.5 + 4.0;
    let channel = |p: DVec2| in_channel(p, reach, ground);
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
    let spring = spring_at(stream, &channel, ground) - lo;
    // Pools along the course (a plunge pool, hollows in the bed) start full, as the running
    // stream leaves them; seeds that aren't in a hollow fill nothing.
    let mut fills = Vec::new();
    for w in stream.path.windows(2) {
        let (a, b) = (DVec2::from_array(w[0]), DVec2::from_array(w[1]));
        let steps = (a.distance(b) / 2.0).ceil().max(1.0) as usize;
        for k in 0..steps {
            let p = channel(a.lerp(b, k as f64 / steps as f64)) - lo;
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
            let p = channel(a.lerp(b, (next - along) / len)) - lo;
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
            // The banks keep the water in and the lake (a body over the region) takes it at the
            // end; what still gets out runs off rather than standing against an unseen wall.
            open_edges: true,
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
    fn stones_are_laid_by_reach_in_and_along_the_channel() {
        // Straight down +z: 100 m dropping 8 % (a cascade), then 100 m dropping 1 %.
        let path = [[0.0, 0.0], [0.0, 200.0]];
        let s = ValleyStream {
            path: &path,
            width_m: 5.0,
            flow_m3_s: 1.0,
        };
        let bed = |z: f64| {
            let z = z.clamp(0.0, 200.0);
            if z < 100.0 {
                -0.08 * z
            } else {
                -8.0 - 0.01 * (z - 100.0)
            }
        };
        let ground = |x: f64, z: f64| {
            bed(z)
                + if x.abs() < 2.5 {
                    0.0
                } else {
                    (x.abs() - 2.5) * 1.5
                }
        };
        let stones = stream_stones(&s, &ground);
        assert_eq!(stones, stream_stones(&s, &ground), "the same every time");
        let in_channel = |s: &&StreamStone| s.at.x.abs() < 2.0;
        let steep = stones
            .iter()
            .filter(in_channel)
            .filter(|s| s.at.z < 95.0)
            .count();
        let gentle = stones
            .iter()
            .filter(in_channel)
            .filter(|s| s.at.z > 105.0)
            .count();
        assert!(
            steep > 2 * gentle && gentle > 3,
            "steep {steep}, gentle {gentle}"
        );
        // Both banks get theirs.
        assert!(stones.iter().filter(|s| s.at.x > 2.0).count() > 10);
        assert!(stones.iter().filter(|s| s.at.x < -2.0).count() > 10);
        for st in &stones {
            assert!(st.at.x.abs() < 4.0, "off the channel: {:?}", st.at);
            assert!((st.at.y - ground(st.at.x, st.at.z)).abs() < 1e-9);
            assert!(
                st.at.z > 2.0 && st.at.z < 195.0,
                "spring or mouth: {:?}",
                st.at
            );
            // From cobbles under the water to boulders standing out of it.
            assert!(st.height > 0.1 && st.height < 0.9, "{}", st.height);
        }
        // And they stand in its flow region.
        let (corner, mut region) = stream_region(&s, 0.0, &ground).expect("region");
        region.rocks = stones.iter().map(|st| st.rock(corner)).collect();
        assert!(region.rocks.iter().all(|r| r.at[0] > 0.0 && r.at[1] > 0.0));
    }

    #[test]
    fn a_stream_region_covers_its_course() {
        let path = [[262.0, -440.0], [250.0, -400.0], [236.0, -235.0]];
        let s = ValleyStream {
            path: &path,
            width_m: 5.0,
            flow_m3_s: 0.4,
        };
        // A channel carved along the straight course, 1 m deep in flat ground.
        let ground = |x: f64, z: f64| {
            let p = DVec2::new(x, z);
            let d = path
                .windows(2)
                .map(|w| {
                    let (a, b) = (DVec2::from_array(w[0]), DVec2::from_array(w[1]));
                    let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
                    (p - a.lerp(b, t)).length()
                })
                .fold(f64::MAX, f64::min);
            if d < 2.5 { -1.0 } else { 0.0 }
        };
        let (corner, region) = stream_region(&s, 0.0, &ground).expect("region");
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

    #[test]
    fn a_spring_wells_up_where_the_channel_can_hold_it() {
        // Dug 0.2 m deep at the head, deepening to 1.2 m over the first 30 m.
        let path = [[0.0, 0.0], [0.0, 100.0]];
        let s = ValleyStream {
            path: &path,
            width_m: 5.0,
            flow_m3_s: 0.4,
        };
        let ground = |x: f64, z: f64| {
            if x.abs() < 2.5 {
                -(0.2 + (z / 30.0).clamp(0.0, 1.0))
            } else {
                0.0
            }
        };
        let (corner, region) = stream_region(&s, 0.0, &ground).expect("region");
        let z = corner.z + region.sources[0].at[1] as f64;
        assert!((14.0..=18.0).contains(&z), "{z}");
    }

    #[test]
    fn springs_and_pools_go_in_the_channel_not_the_meadow_beside_it() {
        // The landscape's channel runs 3 m east of the straight course (a smoothed bend).
        let path = [[0.0, 0.0], [0.0, 100.0]];
        let s = ValleyStream {
            path: &path,
            width_m: 5.0,
            flow_m3_s: 0.4,
        };
        let ground = |x: f64, _z: f64| if (x - 3.0).abs() < 2.0 { -1.0 } else { 0.0 };
        let (corner, region) = stream_region(&s, 0.0, &ground).expect("region");
        let world = |at: [f32; 2]| (corner.x + at[0] as f64, corner.z + at[1] as f64);
        let wet = |at: [f32; 2]| {
            let (x, z) = world(at);
            ground(x, z) < -0.5
        };
        assert!(
            wet(region.sources[0].at),
            "{:?}",
            world(region.sources[0].at)
        );
        assert!(region.priming.iter().all(|p| wet(p.at)));
        assert!(region.fills.iter().all(|f| wet(f.at)));
    }
}
