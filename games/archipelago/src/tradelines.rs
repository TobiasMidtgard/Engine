//! Trade lines: every trade route drawn as a coloured arc from harbour to harbour, with an
//! arrow for the way it runs, when the view is out over the islands or the whole globe. The
//! routes screen paints each route's chip from [`route_color`], so a chip and its line agree.

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

/// Where each stop is on the map (its building's middle), by building id.
fn stop_points(world: &mut World) -> HashMap<u32, DVec3> {
    let mut q = world.query::<&Building>();
    q.iter(world)
        .map(|b| {
            let c = grid::footprint_center(b.kind, b.origin, b.rot);
            (b.id, DVec3::new(c.x, b.base_y, c.y))
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
                .map(|p| p.to_array().map(f64::to_bits))
                .hash(&mut h);
        }
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
            // Legs drawn so far, to lift a second route on the same leg a little higher.
            let mut legs: HashMap<(u32, u32), u32> = HashMap::new();
            for (ri, r) in routes.iter().enumerate() {
                let stops: Vec<(u32, DVec3)> = r
                    .stops
                    .iter()
                    .filter_map(|s| points.get(&s.at).map(|p| (s.at, *p)))
                    .collect();
                if stops.len() < 2 {
                    continue;
                }
                let o = globe::point(stops[0].1);
                let [cr, cg, cb] = route_color(ri);
                let color = [linear(cr), linear(cg), linear(cb), 1.0];
                let (mut vs, mut is) = (Vec::new(), Vec::new());
                // A loop through the stops; with two stops, there and back on one arc.
                let n = if stops.len() == 2 { 1 } else { stops.len() };
                for k in 0..n {
                    let (a, b) = (stops[k], stops[(k + 1) % stops.len()]);
                    let leg = (a.0.min(b.0), a.0.max(b.0));
                    let seen = legs.entry(leg).or_default();
                    let lift = 1.0 + 0.35 * *seen as f64 + if r.air { 1.0 } else { 0.0 };
                    *seen += 1;
                    ribbon(&arc(a.1, b.1, lift), w, o, color, &mut vs, &mut is);
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
    fn route_colours_follow_a_fixed_order() {
        assert_eq!(route_color(0), route_color(8));
        assert_ne!(route_color(0), route_color(1));
        let [r, g, b] = route_color(1);
        assert!(r > g && g > b, "slot 2 is orange");
    }
}
