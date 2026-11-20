//! Trade lines: every trade route drawn when the view is out over the islands or the whole
//! globe. A sea route is a racing line on the water along the lane its ships really sail
//! (round headlands, out through the gates and over the ocean): a dark lane edged in the
//! route's colour, and chevrons along it pointing the way it runs, green where the ships run at
//! full speed, amber and red where they ease off for a turn or a harbour. Each way of a
//! route keeps to its own side, as traffic does. Air routes fly straight, so they stay arcs
//! over the sea. The routes screen paints each route's chip from [`route_color`], so a chip
//! and its line agree.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use engine::prelude::*;
use engine::render::{Mesh, NoMeshLod, NoVirtualGeometry, Vertex};

use crate::camera::StrategyCam;
use crate::fleet::Routes;
use crate::globe::{self, Frame, RADIUS_M};
use crate::grid;
use crate::sim::Building;

/// Whether trade lines show (the `isle.view.trade_lines` action and the map's toggle).
#[derive(Resource)]
pub struct TradeLines {
    pub show: bool,
    drawn: Vec<(Entity, Handle<Mesh>)>,
    mats: HashMap<usize, Handle<StandardMaterial>>,
    key: u64,
}

impl Default for TradeLines {
    fn default() -> Self {
        Self {
            show: true,
            drawn: Vec::new(),
            mats: HashMap::new(),
            key: 0,
        }
    }
}

/// Lines show from this camera distance out (a group of islands in view), meters.
pub const SHOW_FROM: f32 = 1_500.0;

/// Eight hues in a fixed order, checked to stay apart for colour-blind eyes (sRGB, 0 to 1).
const PALETTE: [u32; 8] = [
    0x2a78d6, 0xeb6834, 0x1baf7a, 0xeda100, 0xe87ba4, 0x008300, 0x6250d6, 0xe34948,
];

/// The colour of route `i` (its index in [`Routes`]), sRGB 0 to 1. After eight routes the
/// hues come round again.
pub fn route_color(i: usize) -> [f32; 3] {
    let c = PALETTE[i % PALETTE.len()];
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Planet-local points of one leg's arc from `a` to `b` (map points), lifted in the middle by
/// a share of its length so it reads against the sea from far out. `lift` spreads routes that
/// share a leg.
fn arc(a: DVec3, b: DVec3, lift: f64) -> Vec<DVec3> {
    let (pa, pb) = (globe::point(a), globe::point(b));
    let (da, db) = (pa.normalize(), pb.normalize());
    let theta = da.dot(db).clamp(-1.0, 1.0).acos();
    let len = theta * RADIUS_M;
    if len < 1.0 {
        return Vec::new();
    }
    let h = (len * 0.06 * lift).clamp(15.0, 5_000.0);
    let n = ((len / 40.0) as usize).clamp(16, 128);
    (0..=n)
        .map(|k| {
            let t = k as f64 / n as f64;
            // Slerp the direction; the height of the ends plus the arch.
            let d = if theta < 1e-6 {
                da
            } else {
                (da * ((1.0 - t) * theta).sin() + db * (t * theta).sin()) / theta.sin()
            };
            let ground = pa.length() * (1.0 - t) + pb.length() * t;
            d.normalize() * (ground + 6.0 + h * 4.0 * t * (1.0 - t))
        })
        .collect()
}

/// A flat ribbon `w` wide along `pts`, facing away from the planet's middle, with an arrow
/// half way along; positions relative to `o`.
fn ribbon(
    pts: &[DVec3],
    w: f64,
    o: DVec3,
    color: [f32; 4],
    vs: &mut Vec<Vertex>,
    is: &mut Vec<u32>,
) {
    if pts.len() < 2 {
        return;
    }
    let side_at = |k: usize| {
        let up = pts[k].normalize();
        let t = (pts[(k + 1).min(pts.len() - 1)] - pts[k.saturating_sub(1)]).normalize_or_zero();
        (t.cross(up).normalize_or_zero(), up, t)
    };
    let vert = |p: DVec3, up: DVec3| {
        Vertex::new((p - o).as_vec3(), up.as_vec3(), Vec2::ZERO).with_color(Vec4::from(color))
    };
    let base = vs.len() as u32;
    for (k, p) in pts.iter().enumerate() {
        let (s, up, _) = side_at(k);
        vs.push(vert(*p - s * w * 0.5, up));
        vs.push(vert(*p + s * w * 0.5, up));
        if k > 0 {
            let i = base + 2 * k as u32;
            is.extend_from_slice(&[i - 2, i - 1, i, i - 1, i + 1, i]);
        }
    }
    // The arrow: a chevron three widths long at the middle, pointing the way the route runs.
    let m = pts.len() / 2;
    let (s, up, t) = side_at(m);
    let tip = pts[m] + t * w * 3.0 + up * 0.5;
    let back = pts[m] - t * w * 1.0 + up * 0.5;
    let i = vs.len() as u32;
    vs.push(vert(tip, up));
    vs.push(vert(back - s * w * 1.6, up));
    vs.push(vert(back + s * w * 1.6, up));
    is.extend_from_slice(&[i, i + 1, i + 2]);
}

/// Map points of the lane a ship sails from mooring `a` to mooring `b`, one list per sheet of
/// the map, smoothed into the curve its helm makes of the charted corners; `None` where the
/// sea does not join them.
fn sea_path(chart: &mut crate::fleet::SeaChart, a: DVec2, b: DVec2) -> Option<Vec<Vec<DVec3>>> {
    let legs = chart.voyage(a, b)?;
    Some(legs.iter().map(|l| chaikin(l, 3)).collect())
}

/// Corner cutting (Chaikin): each pass replaces every corner by two points a quarter of the
/// way along its sides, keeping the ends.
fn chaikin(pts: &[DVec3], passes: u32) -> Vec<DVec3> {
    let mut p = pts.to_vec();
    for _ in 0..passes {
        if p.len() < 3 {
            break;
        }
        let mut q = Vec::with_capacity(p.len() * 2);
        q.push(p[0]);
        for w in p.windows(2) {
            q.push(w[0] * 0.75 + w[1] * 0.25);
            q.push(w[0] * 0.25 + w[1] * 0.75);
        }
        q.push(*p.last().unwrap());
        p = q;
    }
    p
}

/// Points every `step` meters along `pts` (map points), ends kept.
fn resample(pts: &[DVec3], step: f64) -> Vec<DVec3> {
    let mut out = Vec::new();
    if pts.len() < 2 {
        return pts.to_vec();
    }
    out.push(pts[0]);
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let seg = (w[1] - w[0]).length();
        let mut d = step - carry;
        while d < seg {
            out.push(w[0] + (w[1] - w[0]) * (d / seg));
            d += step;
        }
        carry = (carry + seg) % step;
    }
    out.push(*pts.last().unwrap());
    out
}

/// How fast a ship goes at each point of a lane, 0 to 1 of its top speed: slow where the lane
/// bends hard and over the last and first stretch by a harbour, gathering and losing way
/// gently between (a racing line's braking and throttle).
pub fn speed_profile(pts: &[DVec3], harbour_m: f64) -> Vec<f32> {
    let n = pts.len();
    if n < 2 {
        return vec![1.0; n];
    }
    let mut along = vec![0.0f64; n];
    for k in 1..n {
        along[k] = along[k - 1] + (pts[k] - pts[k - 1]).length();
    }
    let total = along[n - 1];
    let flat = |v: DVec3| DVec2::new(v.x, v.z);
    let mut v: Vec<f32> = (0..n)
        .map(|k| {
            // Bend: the turn over about 60 m either side, radians per meter.
            let back = (0..k)
                .rev()
                .find(|&j| along[k] - along[j] >= 60.0)
                .unwrap_or(0);
            let fwd = (k + 1..n)
                .find(|&j| along[j] - along[k] >= 60.0)
                .unwrap_or(n - 1);
            let (d0, d1) = (
                flat(pts[k] - pts[back]).normalize_or_zero(),
                flat(pts[fwd] - pts[k]).normalize_or_zero(),
            );
            let span = (along[fwd] - along[back]).max(1.0);
            let bend = d0.dot(d1).clamp(-1.0, 1.0).acos() / span;
            // A turn of about 100 m radius halves the speed.
            let turn = (1.0 / (1.0 + bend * 100.0)) as f32;
            let ends = (along[k].min(total - along[k]) / harbour_m).clamp(0.25, 1.0) as f32;
            turn.min(ends)
        })
        .collect();
    // Way is gathered and lost over a distance, not at once.
    let ease = |v: &mut Vec<f32>, order: &mut dyn Iterator<Item = usize>, prev: &mut usize| {
        for k in order {
            let gap = (along[k] - along[*prev]).abs() as f32;
            v[k] = v[k].min(v[*prev] + gap / 150.0);
            *prev = k;
        }
    };
    let mut prev = 0;
    ease(&mut v, &mut (1..n), &mut prev);
    let mut prev = n - 1;
    ease(&mut v, &mut (0..n - 1).rev(), &mut prev);
    v
}

/// Green at full speed, amber at half, red at a crawl (sRGB 0 to 1).
fn pace_color(f: f32) -> [f32; 3] {
    let (green, amber, red) = ([0.16, 0.78, 0.38], [0.96, 0.68, 0.05], [0.89, 0.22, 0.16]);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    let t = ((f - 0.25) / 0.75).clamp(0.0, 1.0);
    if t > 0.5 {
        mix(amber, green, (t - 0.5) * 2.0)
    } else {
        mix(red, amber, t * 2.0)
    }
}

/// A racing line along map points `pts` (one sheet of a lane), `w` wide and `side` widths to
/// the right of its way: a dark lane with bright edges in `edge` and a chevron every 1.6
/// widths coloured by the pace there. Positions relative to planet point `o`.
#[allow(clippy::too_many_arguments)]
fn racing_line(
    pts: &[DVec3],
    pace: &[f32],
    w: f64,
    side: f64,
    o: DVec3,
    edge: [f32; 4],
    vs: &mut Vec<Vertex>,
    is: &mut Vec<u32>,
) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let lin = |c: [f32; 3]| [linear(c[0]), linear(c[1]), linear(c[2]), 1.0];
    // Planet points just over the sea, each with its way, side and up.
    let at: Vec<(DVec3, DVec3, DVec3, DVec3)> = (0..n)
        .map(|k| {
            let p = globe::point(DVec3::new(pts[k].x, LIFT_M, pts[k].z));
            let up = p.normalize();
            let a = globe::point(pts[k.saturating_sub(1)]);
            let b = globe::point(pts[(k + 1).min(n - 1)]);
            let t = (b - a).normalize_or_zero();
            let s = t.cross(up).normalize_or_zero();
            (p + s * side * w, t, s, up)
        })
        .collect();
    let vert = |p: DVec3, up: DVec3, c: [f32; 4]| {
        Vertex::new((p - o).as_vec3(), up.as_vec3(), Vec2::ZERO).with_color(Vec4::from(c))
    };
    // The lane: a band in a dark shade of the route's colour, with bright edges a sixth of
    // its width at either side, so it reads as a road across the sea at any zoom.
    let dark = [edge[0] * 0.28, edge[1] * 0.28, edge[2] * 0.28, 1.0];
    for (from, to, c) in [(-0.5, -0.33, edge), (-0.33, 0.33, dark), (0.33, 0.5, edge)] {
        let base = vs.len() as u32;
        for (k, (p, _, s, up)) in at.iter().enumerate() {
            vs.push(vert(*p + *s * from * w, *up, c));
            vs.push(vert(*p + *s * to * w, *up, c));
            if k > 0 {
                let i = base + 2 * k as u32;
                is.extend_from_slice(&[i - 2, i - 1, i, i - 1, i + 1, i]);
            }
        }
    }
    // The chevrons: every 1.6 widths, a V pointing the way, in the colour of the pace.
    let mut next = w;
    let mut run = 0.0;
    for k in 1..n {
        run += (at[k].0 - at[k - 1].0).length();
        if run < next || k + 1 == n {
            continue;
        }
        next = run + w * 1.6;
        let (p, t, s, up) = at[k];
        let c = lin(pace_color(pace[k]));
        let lift = up * 0.05;
        for arm in [-1.0, 1.0] {
            let tip = p + t * w * 0.3 + lift;
            let out = p - t * w * 0.3 + s * arm * w * 0.3 + lift;
            let th = t * w * 0.2;
            let i = vs.len() as u32;
            vs.push(vert(tip, up, c));
            vs.push(vert(tip - th, up, c));
            vs.push(vert(out, up, c));
            vs.push(vert(out - th, up, c));
            is.extend_from_slice(&[i, i + 1, i + 2, i + 1, i + 3, i + 2]);
        }
    }
}

/// How many ribbon widths a racing line is across.
const RACING_W: f64 = 2.4;

/// How far over the sea the racing lines lie, meters (above the swell).
const LIFT_M: f64 = 1.5;

/// Where each stop is on the map (its building's middle) and where ships moor there, by
/// building id.
fn stop_points(world: &mut World) -> HashMap<u32, (DVec3, DVec2)> {
    let mut q = world.query::<&Building>();
    q.iter(world)
        .map(|b| {
            let c = grid::footprint_center(b.kind, b.origin, b.rot);
            (
                b.id,
                (DVec3::new(c.x, b.base_y, c.y), crate::fleet::mooring(b).0),
            )
        })
        .collect()
}

/// Draw, redraw or hide the lines (each frame: cheap unless the routes or the zoom changed).
pub fn update(world: &mut World) {
    if !world.contains_resource::<TradeLines>() {
        return;
    }
    let dist = {
        let mut q = world.query::<&StrategyCam>();
        q.iter(world).next().map_or(0.0, |c| c.dist)
    };
    let show = world.resource::<TradeLines>().show && dist >= SHOW_FROM;
    let routes = world.resource::<Routes>().0.clone();
    let points = stop_points(world);
    // The width keeps a few pixels on screen: about 1/240 of the distance, in steps.
    let step = if show {
        ((dist as f64).ln() / 0.12).round() as i64
    } else {
        -1
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    step.hash(&mut h);
    for r in &routes {
        for s in &r.stops {
            s.at.hash(&mut h);
            points
                .get(&s.at)
                .map(|p| p.0.to_array().map(f64::to_bits))
                .hash(&mut h);
        }
        r.air.hash(&mut h);
        0xffu8.hash(&mut h);
    }
    let key = h.finish();
    let mut lines = std::mem::take(&mut *world.resource_mut::<TradeLines>());
    let frame = *world.resource::<Frame>();
    if key != lines.key {
        lines.key = key;
        for (e, handle) in lines.drawn.drain(..) {
            world.despawn(e);
            world.resource_mut::<Assets<Mesh>>().remove(handle);
        }
        if show {
            let w = (step as f64 * 0.12).exp() / 240.0;
            // Legs drawn so far: a second route on the same leg keeps further out (or, in
            // the air, a little higher).
            let mut legs: HashMap<(u32, u32), u32> = HashMap::new();
            let mut chart = world
                .remove_resource::<crate::fleet::SeaChart>()
                .unwrap_or_default();
            for (ri, r) in routes.iter().enumerate() {
                // (The player's routes: a rival's lanes are its own business.)
                if r.company != crate::sim::PLAYER {
                    continue;
                }
                let stops: Vec<(u32, DVec3, DVec2)> = r
                    .stops
                    .iter()
                    .filter_map(|s| points.get(&s.at).map(|p| (s.at, p.0, p.1)))
                    .collect();
                if stops.len() < 2 {
                    continue;
                }
                let o = globe::point(stops[0].1);
                let [cr, cg, cb] = route_color(ri);
                let color = [linear(cr), linear(cg), linear(cb), 1.0];
                let (mut vs, mut is) = (Vec::new(), Vec::new());
                // A loop through the stops: with two stops, there and back, each way on its
                // own side of the lane.
                let n = if stops.len() == 2 && r.air {
                    1
                } else {
                    stops.len()
                };
                for k in 0..n {
                    let (a, b) = (stops[k], stops[(k + 1) % stops.len()]);
                    let leg = (a.0.min(b.0), a.0.max(b.0));
                    let seen = legs.entry(leg).or_default();
                    let lane_no = *seen;
                    *seen += 1;
                    let path = if r.air {
                        None
                    } else {
                        sea_path(&mut chart, a.2, b.2)
                    };
                    match path {
                        Some(sheets) => {
                            // One pace for the whole way, harbour to harbour.
                            let spaced: Vec<Vec<DVec3>> = sheets
                                .iter()
                                .map(|l| resample(l, (w * 0.5).max(2.0)))
                                .collect();
                            let all: Vec<DVec3> = spaced.iter().flatten().copied().collect();
                            let pace = speed_profile(&all, 90.0);
                            let mut from = 0;
                            for l in &spaced {
                                let side = 0.6 + 1.1 * (lane_no / 2) as f64;
                                racing_line(
                                    l,
                                    &pace[from..from + l.len()],
                                    w * RACING_W,
                                    side,
                                    o,
                                    color,
                                    &mut vs,
                                    &mut is,
                                );
                                from += l.len();
                            }
                        }
                        None => {
                            let lift = 1.0 + 0.35 * lane_no as f64 + if r.air { 1.0 } else { 0.0 };
                            ribbon(&arc(a.1, b.1, lift), w, o, color, &mut vs, &mut is);
                        }
                    }
                }
                if is.is_empty() {
                    continue;
                }
                let mat = *lines.mats.entry(ri % PALETTE.len()).or_insert_with(|| {
                    world
                        .resource_mut::<Assets<StandardMaterial>>()
                        .add(StandardMaterial {
                            base_color: [1.0; 4],
                            unlit: true,
                            double_sided: true,
                            // (Counts as glowing: never swapped for a far-away stand-in card.)
                            emissive_strength: 1.0,
                            ..Default::default()
                        })
                });
                let handle = world.resource_mut::<Assets<Mesh>>().add(Mesh::new(vs, is));
                let mut mr = MeshRenderer::new(handle, mat);
                mr.cast_shadows = false;
                let e = world
                    .spawn((
                        Name::new(format!("Trade line: {}", r.name)),
                        Transform::from_translation(frame.world(o))
                            .with_rotation(frame.rot.as_quat()),
                        mr,
                        NoVirtualGeometry,
                        NoMeshLod,
                        LineOrigin(o),
                    ))
                    .id();
                lines.drawn.push((e, handle));
            }
            world.insert_resource(chart);
        }
    }
    // Keep the lines on the planet as the world frame shifts.
    for (e, _) in &lines.drawn {
        let o = world.get::<LineOrigin>(*e).map(|l| l.0);
        if let (Some(o), Some(mut t)) = (o, world.get_mut::<Transform>(*e)) {
            *t = Transform::from_translation(frame.world(o)).with_rotation(frame.rot.as_quat());
        }
    }
    world.insert_resource(lines);
}

/// The planet-local point a line's mesh is measured from.
#[derive(Component, Clone, Copy)]
struct LineOrigin(DVec3);

/// `isle.view.trade_lines`: toggle, or `on` to set.
pub fn set(world: &mut World, on: Option<bool>) -> bool {
    let mut l = world.resource_mut::<TradeLines>();
    l.show = on.unwrap_or(!l.show);
    l.show
}

pub fn register(app: &mut App) {
    app.init_resource::<TradeLines>().register_api(
        "isle.view.trade_lines",
        "Show or hide the trade lines drawn over the sea when the view is out over the islands or the globe (no `on`: toggle).",
        r#"{"on": true}"#,
        |world, args| {
            let on = set(world, args["on"].as_bool());
            Ok(serde_json::json!({ "show": on }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arc_starts_and_ends_at_its_harbours_and_rises_between() {
        let a = DVec3::new(0.0, 0.0, 0.0);
        let b = DVec3::new(2350.0, 0.0, -1250.0);
        let pts = arc(a, b, 1.0);
        assert!(pts.len() >= 17);
        let r = |p: DVec3| p.length() - RADIUS_M;
        assert!((r(pts[0]) - 6.0).abs() < 0.5 && (r(*pts.last().unwrap()) - 6.0).abs() < 0.5);
        // The middle stands well above the sea, by a share of the leg's length.
        let top = r(pts[pts.len() / 2]);
        assert!(top > 100.0 && top < 400.0, "{top}");
        // A second route on the same leg rides higher.
        assert!(r(arc(a, b, 1.35)[pts.len() / 2]) > top + 30.0);
    }

    #[test]
    fn a_racing_line_slows_for_bends_and_harbours() {
        // A straight run, a right-angle turn, another straight run.
        let mut pts: Vec<DVec3> = (0..=100)
            .map(|k| DVec3::new(k as f64 * 5.0, 0.0, 0.0))
            .collect();
        pts.extend((1..=100).map(|k| DVec3::new(500.0, 0.0, k as f64 * 5.0)));
        let pace = speed_profile(&pts, 90.0);
        let at = |x: usize| pace[x];
        // Slow at both harbours and at the corner, full speed on the straights.
        assert!(at(0) < 0.3 && at(pace.len() - 1) < 0.3);
        assert!(at(100) < 0.6, "corner {}", at(100));
        assert!(at(50) > 0.95 && at(150) > 0.95);
        // Gathering way is gradual: no jump between neighbours.
        assert!(pace.windows(2).all(|w| (w[1] - w[0]).abs() < 0.05));
        assert!(
            pace_color(1.0)[1] > 0.7 && pace_color(0.25)[0] > 0.8,
            "green when fast, red when slow"
        );
    }

    #[test]
    fn corner_cutting_keeps_the_ends_and_rounds_the_corner() {
        let pts = [
            DVec3::ZERO,
            DVec3::new(100.0, 0.0, 0.0),
            DVec3::new(100.0, 0.0, 100.0),
        ];
        let c = chaikin(&pts, 3);
        assert_eq!(c[0], pts[0]);
        assert_eq!(*c.last().unwrap(), pts[2]);
        // No point stays on the sharp corner.
        assert!(c.iter().all(|p| (*p - pts[1]).length() > 5.0));
        let r = resample(&c, 10.0);
        assert!(r.windows(2).all(|w| (w[1] - w[0]).length() <= 10.0 + 1e-6));
    }

    #[test]
    fn route_colours_follow_a_fixed_order() {
        assert_eq!(route_color(0), route_color(8));
        assert_ne!(route_color(0), route_color(1));
        let [r, g, b] = route_color(1);
        assert!(r > g && g > b, "slot 2 is orange");
    }
}
