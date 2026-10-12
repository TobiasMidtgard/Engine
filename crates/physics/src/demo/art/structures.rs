//! Meshes for the proving ground's static structures and machines: the stairs, the
//! wrecking-ball gantry, the seesaw, the paddle wheel and its bin, the pile bin, the
//! turntable, the plank bridge and the yard's slab and curb.
//!
//! Every function returns finished meshes, one per material, in the local frame of the
//! entity that carries them (each function says which). Surfaces that bodies touch match the
//! demo's colliders to within a centimeter; details nothing collides with (rails, bolts,
//! braces, bearing blocks) may stand outside them. UVs are metric (1 UV unit = 1 m) and run
//! along each part's length, so wood grain follows the boards. Vertex colors carry small
//! tint changes between parts (planks, pours, plates) and a little baked occlusion in
//! inside corners; they multiply the material's color, so they stay near white.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::kit::{MeshBuilder, round_corner};

// ---------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------

/// Deterministic hash of an integer to 0..1.
fn hash(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

fn hash2(a: u32, b: u32) -> f32 {
    hash(a.wrapping_mul(0x9e37_79b9) ^ b.wrapping_add(0x7f4a_7c15))
}

/// Smooth 1D value noise in 0..1.
fn noise1(x: f32, seed: u32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let t = f * f * (3.0 - 2.0 * f);
    let a = hash2(i as i32 as u32, seed);
    let b = hash2((i as i32 + 1) as u32, seed);
    a + (b - a) * t
}

/// A grey vertex tint (multiplies the material color).
fn grey(v: f32) -> Vec4 {
    Vec4::new(v, v, v, 1.0)
}

/// A colored vertex tint.
fn tint(r: f32, g: f32, b: f32) -> Vec4 {
    Vec4::new(r, g, b, 1.0)
}

fn at(t: Vec3) -> Mat4 {
    Mat4::from_translation(t)
}

fn rot_at(q: Quat, t: Vec3) -> Mat4 {
    Mat4::from_rotation_translation(q, t)
}

/// A grid of `rows × cols` points with a color per point, joined into quads.
fn grid_colored(
    b: &mut MeshBuilder,
    rows: usize,
    cols: usize,
    mut point: impl FnMut(usize, usize) -> (Vec3, Vec3, Vec2, Vec4),
) {
    let base = b.mesh.vertices.len() as u32;
    let old = b.color;
    for r in 0..rows {
        for c in 0..cols {
            let (p, n, uv, color) = point(r, c);
            b.color = color;
            b.vertex(p, n, uv);
        }
    }
    b.color = old;
    let idx = |r: usize, c: usize| base + (r * cols + c) as u32;
    for r in 0..rows.saturating_sub(1) {
        for c in 0..cols.saturating_sub(1) {
            b.quad(idx(r, c), idx(r, c + 1), idx(r + 1, c + 1), idx(r + 1, c));
        }
    }
}

/// A polygon with each corner rounded by its own radius (`steps` segments per corner).
fn rounded_polygon(pts: &[Vec2], radii: &[f32], steps: u32) -> Vec<Vec2> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let p = pts[i];
        let prev = pts[(i + n - 1) % n];
        let next = pts[(i + 1) % n];
        let r = radii[i.min(radii.len() - 1)];
        out.extend(round_corner(
            p,
            (prev - p).normalize_or(Vec2::X),
            (next - p).normalize_or(Vec2::X),
            r,
            steps,
        ));
    }
    out
}

/// A polyline with its inner corners rounded (`radius` per corner, clamped to fit).
fn fillet_path(pts: &[Vec3], radii: &[f32], steps: u32) -> Vec<Vec3> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    for i in 1..n - 1 {
        let (p, a, c) = (pts[i], pts[i - 1], pts[i + 1]);
        let (da, dc) = ((a - p), (c - p));
        let (la, lc) = (da.length(), dc.length());
        let (ua, uc) = (da / la, dc / lc);
        let cos = ua.dot(uc).clamp(-1.0, 1.0);
        let angle = cos.acos(); // interior angle at the corner
        if angle > PI - 1e-3 {
            out.push(p);
            continue;
        }
        let r = radii[(i - 1).min(radii.len() - 1)];
        // Tangent distance from the corner for a circular fillet of radius r.
        let t = (r / (angle * 0.5).tan()).min(la * 0.49).min(lc * 0.49);
        let r = t * (angle * 0.5).tan();
        let bis = (ua + uc).normalize();
        let center = p + bis * (r / (angle * 0.5).sin());
        let (s, e) = (p + ua * t, p + uc * t);
        let (vs, ve) = (s - center, e - center);
        let sweep = vs.angle_between(ve);
        let axis = vs.cross(ve).normalize_or(Vec3::X);
        let steps = steps.max(1);
        for k in 0..=steps {
            let q = Quat::from_axis_angle(axis, sweep * k as f32 / steps as f32);
            out.push(center + q * vs);
        }
    }
    out.push(pts[n - 1]);
    out
}

/// Sweep a closed cross-section along a path (parallel-transported frames, so it never
/// twists). `section` lists `(point, normal)` in the frame's (side, up) plane; a hard corner
/// is two entries at the same point with different normals. `up` sets the frame's up at the
/// path's start. U runs along the path in meters, V around the section. Open sweeps get
/// flat end caps (convex sections only).
fn sweep(
    b: &mut MeshBuilder,
    path: &[Vec3],
    closed: bool,
    section: &[(Vec2, Vec2)],
    up: Vec3,
    caps: bool,
) {
    let n = path.len();
    if n < 2 || section.len() < 2 {
        return;
    }
    let tangent = |i: usize| -> Vec3 {
        let prev = if i > 0 {
            path[i] - path[i - 1]
        } else if closed {
            path[0] - path[n - 1]
        } else {
            Vec3::ZERO
        };
        let next = if i + 1 < n {
            path[i + 1] - path[i]
        } else if closed {
            path[0] - path[n - 1]
        } else {
            Vec3::ZERO
        };
        (prev.normalize_or(Vec3::ZERO) + next.normalize_or(Vec3::ZERO)).normalize_or(Vec3::Y)
    };
    let t0 = tangent(0);
    let mut u = up.reject_from(t0).normalize_or(t0.any_orthonormal_vector());
    let mut prev_t = t0;
    let count = if closed { n + 1 } else { n };
    let mut frames = Vec::with_capacity(count);
    let mut along = 0.0;
    for k in 0..count {
        let i = k % n;
        let t = tangent(i);
        let q = Quat::from_rotation_arc(prev_t, t);
        u = (q * u).reject_from(t).normalize_or(u);
        prev_t = t;
        if k > 0 {
            along += path[i].distance(path[(k - 1) % n]);
        }
        let s = u.cross(t);
        frames.push((path[i], t, s, u, along));
    }
    let mut v_of = vec![0.0f32; section.len()];
    for j in 1..section.len() {
        v_of[j] = v_of[j - 1] + section[j].0.distance(section[j - 1].0);
    }
    let color = b.color;
    grid_colored(b, frames.len(), section.len(), |r, c| {
        let (p, _, s, u, along) = frames[r];
        let (q, nn) = section[c];
        (
            p + s * q.x + u * q.y,
            (s * nn.x + u * nn.y).normalize_or(u),
            Vec2::new(along, v_of[c]),
            color,
        )
    });
    if caps && !closed {
        for (fi, sign) in [(0usize, -1.0f32), (frames.len() - 1, 1.0)] {
            let (p, t, s, u, _) = frames[fi];
            let face = t * sign;
            let center = section.iter().map(|x| x.0).sum::<Vec2>() / section.len() as f32;
            let ci = b.vertex(p + s * center.x + u * center.y, face, center);
            let rim: Vec<u32> = section
                .iter()
                .map(|(q, _)| b.vertex(p + s * q.x + u * q.y, face, *q))
                .collect();
            for k in 0..rim.len() {
                b.tri(ci, rim[k], rim[(k + 1) % rim.len()]);
            }
        }
    }
}

/// A rectangle cross-section `w × h` (side × up) with corners rounded by `r` in `steps`,
/// as `(point, normal)` pairs for [`sweep`] (smooth around the rounds, hard nowhere).
fn rounded_rect_section(w: f32, h: f32, r: f32, steps: u32) -> Vec<(Vec2, Vec2)> {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let r = r.min(hw).min(hh);
    let mut out = Vec::new();
    for (cx, cy, a0) in [
        (hw - r, hh - r, 0.0f32),
        (-hw + r, hh - r, FRAC_PI_2),
        (-hw + r, -hh + r, PI),
        (hw - r, -hh + r, 1.5 * PI),
    ] {
        for k in 0..=steps {
            let a = a0 + FRAC_PI_2 * k as f32 / steps as f32;
            let n = Vec2::new(a.cos(), a.sin());
            out.push((Vec2::new(cx, cy) + n * r, n));
        }
    }
    out.push(out[0]);
    out
}

/// A hex prism standing on the XZ plane around +Y, `af` across flats and `h` tall, its top
/// edges chamfered; no bottom face (it always sits on something). Placed by `m`.
fn hex_body(b: &mut MeshBuilder, m: Mat4, af: f32, h: f32) {
    let r = af * 0.5 / (PI / 6.0).cos();
    let c = h * 0.2;
    let ri = r - c / (PI / 6.0).cos();
    let corner = |k: usize, rad: f32, y: f32| {
        let a = (k % 6) as f32 / 6.0 * TAU;
        Vec3::new(rad * a.cos(), y, rad * a.sin())
    };
    b.placed(m, |b| {
        for k in 0..6 {
            let mid = (k as f32 + 0.5) / 6.0 * TAU;
            let n = Vec3::new(mid.cos(), 0.0, mid.sin());
            let nc = (n + Vec3::Y).normalize();
            let (u0, u1) = (k as f32 * r, (k + 1) as f32 * r);
            let a0 = b.vertex(corner(k, r, 0.0), n, Vec2::new(u0, 0.0));
            let a1 = b.vertex(corner(k + 1, r, 0.0), n, Vec2::new(u1, 0.0));
            let b1 = b.vertex(corner(k + 1, r, h - c), n, Vec2::new(u1, h - c));
            let b0 = b.vertex(corner(k, r, h - c), n, Vec2::new(u0, h - c));
            b.quad(a0, a1, b1, b0);
            let c0 = b.vertex(corner(k, r, h - c), nc, Vec2::new(u0, h - c));
            let c1 = b.vertex(corner(k + 1, r, h - c), nc, Vec2::new(u1, h - c));
            let d1 = b.vertex(corner(k + 1, ri, h), nc, Vec2::new(u1, h));
            let d0 = b.vertex(corner(k, ri, h), nc, Vec2::new(u0, h));
            b.quad(c0, c1, d1, d0);
        }
        let top: Vec<u32> = (0..6)
            .map(|k| {
                let q = corner(k, ri, h);
                b.vertex(q, Vec3::Y, Vec2::new(q.x, q.z))
            })
            .collect();
        for k in 1..5 {
            b.tri(top[0], top[k], top[k + 1]);
        }
    });
}

/// A hex nut sitting on the XZ plane around +Y at the origin, `af` across flats, `h` tall,
/// with the bolt's end poking out of it, placed by `m`.
fn hex_nut(b: &mut MeshBuilder, m: Mat4, af: f32, h: f32) {
    hex_body(b, m, af, h);
    // The threaded end of the bolt, rounded off.
    let rb = af * 0.32;
    let big = af > 0.035;
    let mut stud = vec![Vec2::new(rb, h - 0.001), Vec2::new(rb, h + rb * 0.7)];
    if big {
        stud.push(Vec2::new(rb * 0.6, h + rb * 1.1));
    }
    stud.push(Vec2::new(0.0, h + rb * 1.15));
    b.placed(m, |b| b.lathe(&stud, if big { 10 } else { 6 }, 50.0));
}

/// A domed bolt head (carriage bolt or rivet) on the XZ plane around +Y, placed by `m`.
fn dome_head(b: &mut MeshBuilder, m: Mat4, r: f32, h: f32, sides: u32) {
    let mut p = vec![Vec2::new(r, -0.002)];
    p.extend((0..=3).map(|k| {
        let a = k as f32 / 3.0 * FRAC_PI_2;
        Vec2::new(r * a.cos(), h * a.sin())
    }));
    b.placed(m, |b| b.lathe(&p, sides, 60.0));
}

/// A short pin or knuckle along Y, `r` round and `hh` half long, its rims chamfered.
fn knuckle(b: &mut MeshBuilder, m: Mat4, r: f32, hh: f32, sides: u32) {
    let c = (r * 0.12).min(hh * 0.3);
    let p = [
        Vec2::new(0.0, -hh),
        Vec2::new(r - c, -hh),
        Vec2::new(r, -hh + c),
        Vec2::new(r, hh - c),
        Vec2::new(r - c, hh),
        Vec2::new(0.0, hh),
    ];
    b.placed(m, |b| b.lathe(&p, sides, 40.0));
}

/// A flat washer on the XZ plane around +Y, placed by `m` (top and rim only: it sits on
/// something and a nut covers its hole).
fn washer(b: &mut MeshBuilder, m: Mat4, r_in: f32, r_out: f32, t: f32, sides: u32) {
    let p = [
        Vec2::new(r_out, -0.001),
        Vec2::new(r_out, t),
        Vec2::new(r_in, t),
        Vec2::new(r_in, t * 0.5),
    ];
    b.placed(m, |b| b.lathe(&p, sides, 40.0));
}

/// A weld bead: a thin rounded seam from `a` to `c`.
fn weld(b: &mut MeshBuilder, a: Vec3, c: Vec3, r: f32) {
    b.tube(&[a, c], r, 5, false, true);
}

/// An I-beam (or H column) along Z from `-len/2` to `len/2`: `depth` along Y, `width`
/// along X, flange thickness `tf`, web thickness `tw`, with rolled root fillets and
/// slightly rounded flange edges.
fn i_beam(b: &mut MeshBuilder, len: f32, depth: f32, width: f32, tf: f32, tw: f32) {
    let (w, h, t) = (width * 0.5, depth * 0.5, tw * 0.5);
    let pts = [
        Vec2::new(-w, -h),
        Vec2::new(w, -h),
        Vec2::new(w, -h + tf),
        Vec2::new(t, -h + tf),
        Vec2::new(t, h - tf),
        Vec2::new(w, h - tf),
        Vec2::new(w, h),
        Vec2::new(-w, h),
        Vec2::new(-w, h - tf),
        Vec2::new(-t, h - tf),
        Vec2::new(-t, -h + tf),
        Vec2::new(-w, -h + tf),
    ];
    let edge = tf * 0.18;
    let root = tf * 0.9;
    let radii = [
        edge, edge, edge, root, root, edge, edge, edge, edge, root, root, edge,
    ];
    let outline = rounded_polygon(&pts, &radii, 3);
    b.extrude(&outline, len, 0.004, 50.0);
}

/// A plate: a box with softly rounded edges, `half` extents.
fn plate(b: &mut MeshBuilder, m: Mat4, half: Vec3, r: f32) {
    b.placed(m, |b| b.rounded_box(half, r, 1));
}

/// A straight round bar between two points.
fn bar(b: &mut MeshBuilder, a: Vec3, c: Vec3, r: f32, sides: u32) {
    b.tube(&[a, c], r, sides, false, true);
}

// ---------------------------------------------------------------------------------------
// 1. Stairs
// ---------------------------------------------------------------------------------------

/// The concrete stairs: a flight with worn, chipped nosings between cheek walls, and
/// painted steel handrails on posts.
pub struct Stairs {
    /// Cast concrete: the flight, landing, cheek walls (with V-joints) and their precast
    /// coping.
    pub concrete: Mesh,
    /// Painted steel: handrails, posts, base plates and their anchor nuts.
    pub steel: Mesh,
}

/// Thickness of the stairs' cheek walls, meters (they stand outside the steps' colliders).
pub const STAIR_CHEEK: f32 = 0.25;

/// The ragdoll stairs: `steps` steps of `rise` × `run` meters, `width` wide, climbing toward
/// -Z to a `landing` meters deep landing (the demo: 0.22, 0.32, 14, 6.0, 3.5).
///
/// Frame: the foot of the flight. The first riser's bottom edge runs along X at y = 0,
/// z = 0, centered on x = 0; step `i`'s tread is the box top at y = rise·(i+1) between
/// z = -run·i and -run·(i+1); the landing top (y = rise·steps) runs from z = -run·steps to
/// -(run·steps + landing), where its back face drops to the ground. Steps span
/// x = ±width/2 exactly (the step colliders); the cheek walls ([`STAIR_CHEEK`] thick) and
/// handrails stand just outside that, the coping overhanging the steps by 2 cm, 5 cm above
/// the nosing line.
pub fn stairs(rise: f32, run: f32, steps: usize, width: f32, landing: f32) -> Stairs {
    let mut concrete = MeshBuilder::new();
    let mut steel = MeshBuilder::new();
    stair_flight(&mut concrete, rise, run, steps, width, landing);
    let hw = width * 0.5;
    // Cheek walls follow the nosing line, their precast coping's top 0.12 m above it, then
    // run level along the landing. Profiles live in (a, y), a = -z.
    let cheek_h = 0.12;
    let k = rise / run;
    let a_back = run * steps as f32 + landing;
    let y_top = rise * steps as f32 + cheek_h;
    let a_kink = run * (steps as f32 - 1.0);
    let a_front = -0.06;
    let wall_y = move |a: f32| (rise + cheek_h + k * a).min(y_top);
    let coping_t = 0.07;
    let top_line: Vec<Vec2> = fillet_path(
        &[
            Vec3::new(a_front - 0.03, wall_y(a_front - 0.03), 0.0),
            Vec3::new(a_kink, y_top, 0.0),
            Vec3::new(a_back + 0.03, y_top, 0.0),
        ],
        &[0.35],
        6,
    )
    .iter()
    .map(|p| p.truncate())
    .collect();
    let under_line: Vec<Vec2> = (0..top_line.len())
        .map(|i| {
            let (a, c) = (
                top_line[i.saturating_sub(1)],
                top_line[(i + 1).min(top_line.len() - 1)],
            );
            let t = (c - a).normalize();
            top_line[i] - Vec2::new(-t.y, t.x) * coping_t
        })
        .collect();
    let joints = [-0.09, 0.86, 2.15, 3.375, 4.625, 5.9, 7.03, a_back + 0.03];
    let wall_joints = [a_front, 2.15, 4.625, a_back];
    let raised: Vec<Vec2> = under_line.iter().map(|p| *p + Vec2::Y * 0.004).collect();
    for (side, seed) in [(-1.0f32, 3u32), (1.0, 7)] {
        let x = side * (hw + STAIR_CHEEK * 0.5);
        let place = |b: &mut MeshBuilder, dx: f32, o: &[Vec2], depth: f32, bevel: f32| {
            b.placed(
                rot_at(
                    Quat::from_rotation_y(FRAC_PI_2),
                    Vec3::new(x + dx, 0.0, 0.0),
                ),
                |b| b.extrude(o, depth, bevel, 30.0),
            )
        };
        // Wall pieces with V-joints between them, their tops hidden under the coping.
        for w in 0..wall_joints.len() - 1 {
            let a0 = wall_joints[w] + if w > 0 { 0.006 } else { 0.0 };
            let a1 = wall_joints[w + 1]
                - if w + 2 < wall_joints.len() {
                    0.006
                } else {
                    0.0
                };
            let mut o = vec![Vec2::new(a0, 0.0), Vec2::new(a1, 0.0)];
            o.extend(cut_line(&raised, a0, a1).into_iter().rev());
            let shade = 0.96 + 0.05 * hash(seed + w as u32);
            concrete.colored(grey(shade), |b| place(b, 0.0, &o, STAIR_CHEEK, 0.012));
        }
        // Precast coping, 2 cm proud inside and 3 cm outside, in pieces between the posts.
        for j in 0..joints.len() - 1 {
            let a0 = joints[j] + if j > 0 { 0.004 } else { 0.0 };
            let a1 = joints[j + 1] - if j + 2 < joints.len() { 0.004 } else { 0.0 };
            let mut o = cut_line(&under_line, a0, a1);
            o.extend(cut_line(&top_line, a0, a1).into_iter().rev());
            let shade = 0.93 + 0.08 * hash(seed * 31 + j as u32);
            concrete.colored(grey(shade), |b| {
                place(b, side * 0.005, &o, STAIR_CHEEK + 0.05, 0.01)
            });
        }
    }
    stair_rails(&mut steel, hw, wall_y, a_kink, a_back);
    Stairs {
        concrete: concrete.build(),
        steel: steel.build(),
    }
}

/// A polyline (points in increasing x) cut to `x0..x1`: its ends interpolated there, its
/// points strictly between kept (dropping any within 4 mm of the cuts).
fn cut_line(line: &[Vec2], x0: f32, x1: f32) -> Vec<Vec2> {
    let mut o = vec![Vec2::new(x0, interp_y(line, x0))];
    o.extend(line.iter().filter(|p| p.x > x0 + 0.004 && p.x < x1 - 0.004));
    o.push(Vec2::new(x1, interp_y(line, x1)));
    o
}

/// The height of a polyline (points in increasing x) at `x`, extended past its ends.
fn interp_y(line: &[Vec2], x: f32) -> f32 {
    let n = line.len();
    let i = line.partition_point(|p| p.x < x).clamp(1, n - 1);
    let (a, c) = (line[i - 1], line[i]);
    let t = (x - a.x) / (c.x - a.x).max(1e-6);
    a.y + (c.y - a.y) * t
}

/// The flight's visible surfaces (risers, nosings, treads, landing, back face) swept across
/// X, with chipped nosings and baked occlusion in the inside corners. Normals come from the
/// final shape so chips and wear shade correctly.
fn stair_flight(
    out: &mut MeshBuilder,
    rise: f32,
    run: f32,
    steps: usize,
    width: f32,
    landing: f32,
) {
    let hw = width * 0.5;
    let base_cols = (width / 0.25).ceil() as usize;
    let mut f = MeshBuilder::new();
    let ao_dark = 0.74;
    let a_back = run * steps as f32 + landing;
    let mut v_base = 0.0f32;
    for i in 0..steps {
        let seed = 101 + i as u32 * 17;
        let (a0, a1) = (run * i as f32, run * (i + 1) as f32);
        let (y0, y1) = (rise * i as f32, rise * (i + 1) as f32);
        let last = i + 1 == steps;
        // Chips: a few broken stretches of nosing per step.
        let chips: Vec<(f32, f32, f32)> = (0..3)
            .filter(|&k| hash2(seed, k) < 0.75)
            .map(|k| {
                let cx = -hw + 0.25 + (width - 0.5) * hash2(seed, 10 + k);
                let half = 0.05 + 0.14 * hash2(seed, 20 + k);
                let depth = 0.012 + 0.03 * hash2(seed, 30 + k);
                (cx, half, depth)
            })
            .collect();
        // Columns: 25 cm apart, 3.5 cm inside the chips.
        let mut xs: Vec<f32> = (0..=base_cols)
            .map(|c| -hw + width * c as f32 / base_cols as f32)
            .collect();
        for &(cx, half, _) in &chips {
            let n = (2.0 * half / 0.035).ceil() as usize;
            xs.extend(
                (0..=n)
                    .map(|k| cx - half + 2.0 * half * k as f32 / n as f32)
                    .filter(|x| x.abs() < hw - 0.02),
            );
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut kept: Vec<f32> = Vec::new();
        for x in xs {
            match kept.last() {
                Some(&l) if x - l < 0.012 => {
                    // Keep the flight's ends exact.
                    if (x - hw).abs() < 1e-6 {
                        *kept.last_mut().unwrap() = x;
                    }
                }
                _ => kept.push(x),
            }
        }
        let xs = kept;
        let cols = xs.len();
        let radius = |x: f32| -> (f32, f32) {
            let mut c = 0.011 + 0.006 * noise1(x * 2.3, seed);
            let mut chip = 0.0f32;
            for &(cx, half, depth) in &chips {
                let t = (x - cx) / half;
                if t.abs() < 1.0 {
                    let bump = (1.0 - t * t).powi(2);
                    c += depth * bump;
                    chip = chip.max(bump);
                }
            }
            (c, chip)
        };
        let shade = 0.95 + 0.07 * hash2(seed, 99);
        // Profile across one column: (a, y), nominal normal (a, y), occlusion.
        let profile = |x: f32| -> Vec<(Vec2, Vec2, f32)> {
            let (c, chip) = radius(x);
            let rough = (noise1(x * 17.0, seed + 5) - 0.5) * chip;
            let mut p = Vec::new();
            p.push((Vec2::new(a0, y0), Vec2::new(-1.0, 0.0), ao_dark));
            p.push((Vec2::new(a0, y0 + 0.06), Vec2::new(-1.0, 0.0), 0.97));
            p.push((Vec2::new(a0, y1 - c), Vec2::new(-1.0, 0.0), 1.0));
            let center = Vec2::new(a0 + c, y1 - c);
            for (k, deg) in [150.0f32, 120.0].into_iter().enumerate() {
                let a = deg.to_radians();
                let n = Vec2::new(a.cos(), a.sin());
                // Broken edges are flatter and uneven.
                let inset = 1.0 - chip * (0.28 + 0.2 * rough * if k == 0 { 1.0 } else { -1.0 });
                p.push((center + n * c * inset, n, 1.0 - 0.1 * chip));
            }
            p.push((Vec2::new(a0 + c, y1), Vec2::new(0.0, 1.0), 1.0));
            if last {
                let cb = 0.02;
                p.push((Vec2::new(a0 + 1.2, y1), Vec2::Y, 1.0));
                p.push((Vec2::new(a_back - cb, y1), Vec2::Y, 1.0));
                let back = Vec2::new(a_back - cb, y1 - cb);
                for deg in [60.0f32, 30.0] {
                    let a = deg.to_radians();
                    let n = Vec2::new(a.cos(), a.sin());
                    p.push((back + n * cb, n, 1.0));
                }
                p.push((Vec2::new(a_back, y1 - cb), Vec2::X, 1.0));
                p.push((Vec2::new(a_back, 0.06), Vec2::X, 0.97));
                p.push((Vec2::new(a_back, 0.0), Vec2::X, ao_dark + 0.06));
            } else {
                // A shallow wear dip in the middle of the tread, a few millimeters deep.
                let dip = 0.003 * (1.0 - (x / hw).powi(2)) * (0.6 + 0.8 * noise1(x * 0.7, seed));
                p.push((
                    Vec2::new(0.5 * (a0 + c + a1 - 0.06), y1 - dip),
                    Vec2::Y,
                    1.0,
                ));
                p.push((Vec2::new(a1 - 0.06, y1), Vec2::Y, 0.97));
                p.push((Vec2::new(a1, y1), Vec2::Y, ao_dark));
            }
            p
        };
        let columns: Vec<Vec<(Vec2, Vec2, f32)>> = xs.iter().map(|&x| profile(x)).collect();
        // V: nominal arc length along the profile (continuous over the whole flight).
        let nominal = profile(1e3);
        let mut vs = vec![v_base];
        for w in nominal.windows(2) {
            vs.push(vs.last().unwrap() + w[0].0.distance(w[1].0));
        }
        v_base = *vs.last().unwrap();
        let rows = columns[0].len();
        grid_colored(&mut f, rows, cols, |r, c| {
            let (q, n, ao) = columns[c][r];
            let x = xs[c];
            (
                Vec3::new(x, q.y, -q.x),
                Vec3::new(0.0, n.y, -n.x),
                Vec2::new(x, vs[r]),
                grey(shade * ao),
            )
        });
    }
    f.mesh.compute_normals();
    out.mesh.merge(&f.mesh);
}

/// Painted steel handrails: one continuous top rail and mid rail running up one side, along
/// the landing's back and down the other side, on posts with bolted base plates.
fn stair_rails(
    b: &mut MeshBuilder,
    hw: f32,
    wall_y: impl Fn(f32) -> f32,
    a_kink: f32,
    a_back: f32,
) {
    let x_rail = hw + STAIR_CHEEK * 0.5;
    let a_corner = a_back - STAIR_CHEEK * 0.5;
    let y_flat = wall_y(a_back);
    let (top, mid) = (0.92, 0.47);
    let (r_top, r_mid, r_post) = (0.0245, 0.017, 0.0245);
    let a0 = 0.22;
    let rail = |h: f32, from_post: bool| -> Vec<Vec3> {
        let mut pts = Vec::new();
        let side = |x: f32, pts: &mut Vec<Vec3>, up: bool| {
            let mut s = vec![
                Vec3::new(x, wall_y(a0) + h, -a0),
                Vec3::new(x, y_flat + h, -a_kink),
                Vec3::new(x, y_flat + h, -a_corner),
            ];
            if !up {
                s.reverse();
            }
            pts.extend(s);
        };
        if from_post {
            pts.push(Vec3::new(-x_rail, wall_y(a0) + 0.01, -a0));
        }
        side(-x_rail, &mut pts, true);
        side(x_rail, &mut pts, false);
        if from_post {
            pts.push(Vec3::new(x_rail, wall_y(a0) + 0.01, -a0));
        }
        pts
    };
    let top_pts = rail(top, true);
    let radii = [0.11, 0.45, 0.16, 0.16, 0.45, 0.11];
    let path = fillet_path(&top_pts, &radii, 6);
    b.tube(&path, r_top, 12, false, true);
    let mid_pts = rail(mid, false);
    let path = fillet_path(&mid_pts, &[0.45, 0.2, 0.2, 0.45], 6);
    b.tube(&path, r_mid, 10, false, true);

    // Posts with sloped or level base plates and anchor nuts.
    let slope = (wall_y(1.0) - wall_y(0.0)).atan2(1.0);
    let mut posts: Vec<(Vec3, f32)> = Vec::new(); // (foot on the surface, plate tilt)
    for s in [-1.0f32, 1.0] {
        let x = s * x_rail;
        for a in [a0, 1.5, 2.8, 3.95] {
            posts.push((Vec3::new(x, wall_y(a), -a), slope));
        }
        for a in [5.3, 6.5, a_corner - 0.3] {
            posts.push((Vec3::new(x, y_flat, -a), 0.0));
        }
    }
    let landing_y = y_flat - 0.12;
    for x in [-2.0f32, 0.0, 2.0] {
        posts.push((Vec3::new(x, landing_y, -a_corner), 0.0));
    }
    for (i, (foot, tilt)) in posts.into_iter().enumerate() {
        let a = -foot.z;
        let top_y = if foot.y < landing_y + 0.01 && (foot.x.abs() < hw) {
            y_flat + top
        } else {
            wall_y(a) + top
        };
        let is_end = (a - a0).abs() < 1e-3;
        if !is_end {
            bar(
                b,
                foot - Vec3::Y * 0.01,
                Vec3::new(foot.x, top_y, foot.z),
                r_post,
                12,
            );
        }
        let q = Quat::from_rotation_x(tilt);
        let up = q * Vec3::Y;
        let pt = 0.012;
        let center = foot + up * (pt * 0.5);
        b.colored(grey(0.96 + 0.04 * hash(i as u32 + 40)), |b| {
            plate(b, rot_at(q, center), Vec3::new(0.07, pt * 0.5, 0.07), 0.004);
            for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = center + q * Vec3::new(dx * 0.047, pt * 0.5, dz * 0.047);
                hex_nut(b, rot_at(q, p), 0.017, 0.009);
            }
            // Weld around the post foot.
            let ring: Vec<Vec3> = (0..10)
                .map(|k| {
                    let ang = k as f32 / 10.0 * TAU;
                    let d = Vec3::new(ang.cos(), 0.0, ang.sin()) * (r_post + 0.002);
                    // Sit the ring on the (possibly sloped) plate.
                    let p = foot + up * pt + d;
                    let lift = -(p - (foot + up * pt)).dot(up) / up.y;
                    p + Vec3::Y * (lift + 0.002)
                })
                .collect();
            b.tube(&ring, 0.004, 5, true, false);
        });
    }
}

// ---------------------------------------------------------------------------------------
// Prisms with patches (walls, curbs, panels)
// ---------------------------------------------------------------------------------------

/// One point of a prism's cross-section (see [`prism`]).
#[derive(Clone, Copy, Debug)]
struct Sec {
    p: Vec2,
    n: Vec2,
    /// Vertex shade (baked occlusion, 1 = none).
    shade: f32,
    /// Which output gets the strip from this point to the next.
    out: usize,
}

fn sec(p: Vec2, n: Vec2) -> Sec {
    Sec {
        p,
        n: n.normalize(),
        shade: 1.0,
        out: 0,
    }
}

/// A quarter round from `from` (normal `n0`) to `to` (normal `n1`) around `center`, as
/// section points (excluding the start, including the end).
fn sec_round(center: Vec2, r: f32, a0: f32, a1: f32, steps: u32) -> Vec<Sec> {
    (1..=steps)
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / steps as f32;
            let n = Vec2::new(a.cos(), a.sin());
            sec(center + n * r, n)
        })
        .collect()
}

/// Revolve a section of (radius, height) points around +Y with `sides` steps, like the
/// kit's lathe but with each point's own normal and shade (hard edges are two points in one
/// place). U runs around at the largest radius in meters, V along the section.
fn revolve(b: &mut MeshBuilder, section: &[Sec], sides: u32) {
    let r_max = section.iter().map(|q| q.p.x).fold(0.0f32, f32::max);
    let mut vs = vec![0.0f32];
    for w in section.windows(2) {
        vs.push(vs.last().unwrap() + w[0].p.distance(w[1].p));
    }
    let color = b.color;
    grid_colored(b, section.len(), sides as usize + 1, |r, c| {
        let a = c as f32 / sides as f32 * TAU;
        let (sn, co) = a.sin_cos();
        let q = &section[r];
        (
            Vec3::new(q.p.x * co, q.p.y, -q.p.x * sn),
            Vec3::new(q.n.x * co, q.n.y, -q.n.x * sn),
            Vec2::new(a * r_max, vs[r]),
            color * grey(q.shade),
        )
    });
}

/// Vertex indices of a prism's body grid: `[output][ring][point]`.
type PrismIndex = Vec<Vec<Vec<Option<u32>>>>;

/// A prism along +Z: the cross-section `section` (XY, open or closed) swept through the
/// body rings at `rings` (ascending z), with 45° end chamfers (`chamfer[0]` before the first
/// ring, `chamfer[1]` after the last) and flat end caps (the section must be convex; an open
/// section's cap closes it with a straight edge). Each strip goes to `outs[section[j].out]`;
/// `skip(j, i)` leaves out the strip from point `j` between rings `i` and `i + 1` (for
/// patches built with the returned indices). U runs along Z in meters, V along the section
/// (from 0 at each output's first point).
#[allow(clippy::too_many_arguments)]
fn prism(
    outs: &mut [&mut MeshBuilder],
    section: &[Sec],
    closed: bool,
    rings: &[f32],
    chamfer: [f32; 2],
    caps: [bool; 2],
    color: Vec4,
    skip: &dyn Fn(usize, usize) -> bool,
) -> PrismIndex {
    let np = section.len();
    let segs = if closed { np } else { np - 1 };
    let seg_out = |j: usize| section[j].out;
    let used = |o: usize, j: usize| -> bool {
        (j < segs && seg_out(j) == o)
            || (j > 0 && seg_out(j - 1) == o)
            || (closed && j == 0 && seg_out(np - 1) == o)
    };
    let mut vs = vec![0.0f32; np];
    for j in 1..np {
        vs[j] = vs[j - 1] + section[j].p.distance(section[j - 1].p);
    }
    let mut v_off = vec![f32::MAX; outs.len()];
    for (j, &v) in vs.iter().enumerate() {
        for (o, off) in v_off.iter_mut().enumerate() {
            if used(o, j) {
                *off = off.min(v);
            }
        }
    }
    // Inset (mitered) section for the end chamfers.
    let inset = |c: f32| -> Vec<Vec2> {
        (0..np)
            .map(|j| {
                let p = section[j].p;
                let mut ns: Vec<Vec2> = section
                    .iter()
                    .filter(|s| s.p.distance(p) < 1e-6)
                    .map(|s| s.n)
                    .collect();
                ns.dedup_by(|a, b| a.distance(*b) < 1e-4);
                let m = if ns.len() >= 2 {
                    let (a, b) = (ns[0], ns[ns.len() - 1]);
                    (a + b) / (1.0 + a.dot(b)).max(0.2)
                } else {
                    ns[0]
                };
                p - m * c
            })
            .collect()
    };
    let col = |j: usize| color * grey(section[j].shade);
    let mut index: PrismIndex = vec![vec![vec![None; np]; rings.len()]; outs.len()];
    for (o, out) in outs.iter_mut().enumerate() {
        let old = out.color;
        // Body rings.
        for (r, &z) in rings.iter().enumerate() {
            for j in 0..np {
                if used(o, j) {
                    let s = section[j];
                    out.color = col(j);
                    index[o][r][j] = Some(out.vertex(
                        s.p.extend(z),
                        s.n.extend(0.0),
                        Vec2::new(z, vs[j] - v_off[o]),
                    ));
                }
            }
        }
        for r in 0..rings.len().saturating_sub(1) {
            for j in 0..segs {
                let k = (j + 1) % np;
                if seg_out(j) != o || skip(j, r) {
                    continue;
                }
                let (a, b, c, d) = (
                    index[o][r][j].unwrap(),
                    index[o][r][k].unwrap(),
                    index[o][r + 1][k].unwrap(),
                    index[o][r + 1][j].unwrap(),
                );
                out.quad(a, b, c, d);
            }
        }
        // End chamfers.
        for (end, (z_body, sign)) in [(rings[0], -1.0f32), (*rings.last().unwrap(), 1.0)]
            .into_iter()
            .enumerate()
        {
            let c = chamfer[end];
            if c <= 0.0 {
                continue;
            }
            let ins = inset(c);
            let z_cap = z_body + sign * c;
            let mut outer = vec![None; np];
            let mut inner = vec![None; np];
            for j in 0..np {
                if used(o, j) {
                    let s = section[j];
                    let n = (s.n.extend(0.0) + Vec3::Z * sign).normalize();
                    out.color = col(j);
                    outer[j] = Some(out.vertex(
                        s.p.extend(z_body),
                        n,
                        Vec2::new(z_body, vs[j] - v_off[o]),
                    ));
                    inner[j] = Some(out.vertex(
                        ins[j].extend(z_cap),
                        n,
                        Vec2::new(z_cap, vs[j] - v_off[o]),
                    ));
                }
            }
            for j in 0..segs {
                let k = (j + 1) % np;
                if seg_out(j) != o {
                    continue;
                }
                out.quad(
                    outer[j].unwrap(),
                    outer[k].unwrap(),
                    inner[k].unwrap(),
                    inner[j].unwrap(),
                );
            }
        }
        out.color = old;
    }
    // Flat end caps (into the first output).
    let out = &mut outs[0];
    let old = out.color;
    out.color = color;
    for (end, (z_body, sign)) in [(rings[0], -1.0f32), (*rings.last().unwrap(), 1.0)]
        .into_iter()
        .enumerate()
    {
        if !caps[end] {
            continue;
        }
        let pts = inset(chamfer[end]);
        let mut uniq: Vec<Vec2> = Vec::new();
        for p in pts {
            if uniq.last().is_none_or(|q| q.distance(p) > 1e-6) {
                uniq.push(p);
            }
        }
        if uniq.len() > 2 && uniq[0].distance(*uniq.last().unwrap()) < 1e-6 {
            uniq.pop();
        }
        let z = z_body + sign * chamfer[end];
        let n = Vec3::Z * sign;
        let center = uniq.iter().copied().sum::<Vec2>() / uniq.len() as f32;
        let ci = out.vertex(center.extend(z), n, center);
        let rim: Vec<u32> = uniq
            .iter()
            .map(|p| out.vertex(p.extend(z), n, *p))
            .collect();
        for k in 0..rim.len() {
            out.tri(ci, rim[k], rim[(k + 1) % rim.len()]);
        }
    }
    out.color = old;
    index
}

/// Fill a flat rectangular cell (four vertex indices around it, on a face with normal `n`)
/// with a round recess `r` wide and `depth` deep at its center: a lifting-anchor pocket or
/// a form-tie hole. The face is joined to the cell's corners only, so no T-junctions.
fn recess_patch(
    b: &mut MeshBuilder,
    corners: [u32; 4],
    n: Vec3,
    r: f32,
    depth: f32,
    sides: u32,
    inner_shade: f32,
) -> Mat4 {
    let pos = |b: &MeshBuilder, i: u32| Vec3::from(b.mesh.vertices[i as usize].position);
    let uvs = |b: &MeshBuilder, i: u32| Vec2::from(b.mesh.vertices[i as usize].uv);
    let p: Vec<Vec3> = corners.iter().map(|&i| pos(b, i)).collect();
    let uv: Vec<Vec2> = corners.iter().map(|&i| uvs(b, i)).collect();
    let cols: Vec<Vec4> = corners
        .iter()
        .map(|&i| Vec4::from(b.mesh.vertices[i as usize].color))
        .collect();
    let center = (p[0] + p[1] + p[2] + p[3]) * 0.25;
    let m = rot_at(Quat::from_rotation_arc(Vec3::Y, n), center);
    let inv = m.inverse();
    let (e1, e2) = (p[1] - p[0], p[3] - p[0]);
    let uv_at = |q: Vec3| {
        let d = q - p[0];
        uv[0]
            + (uv[1] - uv[0]) * (d.dot(e1) / e1.length_squared())
            + (uv[3] - uv[0]) * (d.dot(e2) / e2.length_squared())
    };
    let ang = |q: Vec3| {
        let l = inv.transform_point3(q);
        (-l.z).atan2(l.x)
    };
    let corner_ang: Vec<f32> = p.iter().map(|&q| ang(q)).collect();
    let owner = |a: f32| -> usize {
        let mut best = (0, f32::MAX);
        for (k, &ca) in corner_ang.iter().enumerate() {
            let d = (a - ca + PI).rem_euclid(TAU) - PI;
            if d.abs() < best.1 {
                best = (k, d.abs());
            }
        }
        best.0
    };
    let old = b.color;
    b.color = (cols[0] + cols[1] + cols[2] + cols[3]) * 0.25;
    let ring: Vec<(u32, usize)> = (0..sides)
        .map(|c| {
            let a = c as f32 / sides as f32 * TAU;
            let (s, co) = a.sin_cos();
            let q = m.transform_point3(Vec3::new(r * co, 0.0, -r * s));
            (b.vertex(q, n, uv_at(q)), owner(a))
        })
        .collect();
    for k in 0..ring.len() {
        let (hi, ci) = ring[k];
        let (hj, cj) = ring[(k + 1) % ring.len()];
        b.tri(corners[ci], hi, hj);
        if ci != cj {
            b.tri(corners[ci], hj, corners[cj]);
        }
    }
    b.color = old * grey(inner_shade);
    let dish: Vec<Vec2> = if sides > 10 {
        vec![
            Vec2::new(r, 0.0),
            Vec2::new(r * 0.82, -depth * 0.55),
            Vec2::new(r * 0.45, -depth),
            Vec2::new(0.0, -depth),
        ]
    } else {
        vec![
            Vec2::new(r, 0.0),
            Vec2::new(r * 0.6, -depth),
            Vec2::new(0.0, -depth),
        ]
    };
    b.placed(m, |b| b.lathe(&dish, sides, 70.0));
    b.color = old;
    m
}

/// A hex bolt head (no stud) on the XZ plane around +Y, placed by `m`.
fn hex_head(b: &mut MeshBuilder, m: Mat4, af: f32, h: f32) {
    hex_body(b, m, af, h);
}

/// A rotation taking +Y to `dir`.
fn y_to(dir: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::Y, dir.normalize())
}

/// A frame (as a matrix) with its columns `x`, `y` and `z = x × y` at `t`.
fn frame(y: Vec3, z: Vec3, t: Vec3) -> Mat4 {
    let (y, z) = (y.normalize(), z.normalize());
    let x = y.cross(z);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0))
}

/// Extrude a flat plate outline (XY) `t` thick and place it with `m`.
fn plate_outline(b: &mut MeshBuilder, m: Mat4, outline: &[Vec2], t: f32, bevel: f32) {
    b.placed(m, |b| b.extrude(outline, t, bevel, 35.0));
}

// ---------------------------------------------------------------------------------------
// 2. Wrecking-ball gantry
// ---------------------------------------------------------------------------------------

/// The wrecking ball's portal frame.
pub struct Gantry {
    /// Painted steel (safety yellow): columns, crossbeam, plates, stiffeners, knee braces,
    /// gussets and the pivot hanger.
    pub painted: Mesh,
    /// Bare forged steel: anchor rods, nuts, bolts and the hanger pin.
    pub steel: Mesh,
    /// Cast concrete: footing plinths and grout pads.
    pub concrete: Mesh,
}

/// The wrecking-ball gantry: two H columns `post_spacing` apart (centers, along Z) under an
/// I-beam whose underside is 5 cm above the chain's pivot (the demo: pivot 7.6 m, posts 4.8 m
/// apart, so the beam spans y 7.65..8.15 and z ±2.65, the columns fill x ±0.23, z ±2.4 ±0.25).
///
/// Frame: the demo's `base`: the ground under the pivot, Y up, the frame in the YZ plane.
/// The pivot hanger's pin runs along Z through `(0, pivot_height, 0)`; the chain swings in
/// the XY plane between its lugs (inner faces at z = ±0.085). Footings stand 0.18 m proud.
pub fn gantry(pivot_height: f32, post_spacing: f32) -> Gantry {
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let mut c = MeshBuilder::new();
    let s2 = post_spacing * 0.5;
    let (col_d, col_w, col_tf, col_tw) = (0.5, 0.46, 0.032, 0.02);
    let (bm_d, bm_w, bm_tf, bm_tw) = (0.5, 0.40, 0.028, 0.016);
    let y_bot = pivot_height + 0.05;
    let y_top = y_bot + bm_d;
    let cap_t = 0.03;
    let y_col_top = y_bot - cap_t;
    let (plinth_h, grout, base_t) = (0.18, 0.025, 0.04);
    let y_base = plinth_h + grout;
    let y_col_bot = y_base + base_t;
    let half_len = s2 + col_d * 0.5;
    let paint = |k: u32| grey(0.95 + 0.06 * hash(k));

    for (ci, side) in [-1.0f32, 1.0].into_iter().enumerate() {
        let z = side * s2;
        let k = ci as u32 * 31;
        c.colored(grey(0.94 + 0.05 * hash(k + 1)), |b| {
            plate(
                b,
                at(Vec3::new(0.0, plinth_h * 0.5, z)),
                Vec3::new(0.5, plinth_h * 0.5, 0.56),
                0.025,
            );
        });
        c.colored(grey(0.82), |b| {
            plate(
                b,
                at(Vec3::new(0.0, plinth_h + grout * 0.5, z)),
                Vec3::new(0.37, grout * 0.5, 0.43),
                0.01,
            );
        });
        p.colored(paint(k + 2), |b| {
            plate(
                b,
                at(Vec3::new(0.0, y_base + base_t * 0.5, z)),
                Vec3::new(0.34, base_t * 0.5, 0.4),
                0.006,
            );
            b.placed(
                rot_at(
                    Quat::from_rotation_x(-FRAC_PI_2),
                    Vec3::new(0.0, (y_col_bot + y_col_top) * 0.5, z),
                ),
                |b| i_beam(b, y_col_top - y_col_bot, col_d, col_w, col_tf, col_tw),
            );
            plate(
                b,
                at(Vec3::new(0.0, y_col_top + cap_t * 0.5, z)),
                Vec3::new(0.25, cap_t * 0.5, 0.275),
                0.005,
            );
            // Base stiffeners on the outer faces of both flanges.
            let stiff = rounded_polygon(
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.13, 0.0),
                    Vec2::new(0.13, 0.06),
                    Vec2::new(0.02, 0.32),
                    Vec2::new(0.0, 0.32),
                ],
                &[0.0, 0.0, 0.01, 0.01, 0.0],
                2,
            );
            for sz in [-1.0f32, 1.0] {
                let q = Quat::from_rotation_y(-sz * FRAC_PI_2);
                for sx in [-1.0f32, 1.0] {
                    let o = Vec3::new(sx * 0.12, y_col_bot, z + sz * col_d * 0.5);
                    plate_outline(b, rot_at(q, o), &stiff, 0.016, 0.003);
                    for dx in [-1.0f32, 1.0] {
                        let x = sx * 0.12 + dx * 0.011;
                        weld(
                            b,
                            Vec3::new(x, y_col_bot + 0.005, o.z + sz * 0.004),
                            Vec3::new(x, y_col_bot + 0.3, o.z + sz * 0.004),
                            0.005,
                        );
                        weld(
                            b,
                            Vec3::new(x, y_col_bot + 0.005, o.z + sz * 0.005),
                            Vec3::new(x, y_col_bot + 0.005, o.z + sz * 0.125),
                            0.005,
                        );
                    }
                }
                // Flange-to-plate fillet welds.
                let fz = z + sz * (col_d * 0.5 + 0.004);
                weld(
                    b,
                    Vec3::new(-col_w * 0.5, y_col_bot + 0.004, fz),
                    Vec3::new(col_w * 0.5, y_col_bot + 0.004, fz),
                    0.006,
                );
            }
        });
        // Anchor rods: plate washers, nuts and rod ends.
        for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let o = Vec3::new(dx * 0.27, y_base + base_t, z + dz * 0.32);
            p.colored(paint(k + 3), |b| {
                plate(
                    b,
                    at(o + Vec3::Y * 0.006),
                    Vec3::new(0.04, 0.006, 0.04),
                    0.003,
                )
            });
            s.colored(grey(0.9 + 0.1 * hash(k + 7 + dx as u32)), |b| {
                hex_nut(b, at(o + Vec3::Y * 0.012), 0.046, 0.03);
            });
        }
    }
    // Crossbeam, end plates and web stiffeners.
    p.colored(paint(90), |b| {
        b.placed(at(Vec3::new(0.0, y_bot + bm_d * 0.5, 0.0)), |b| {
            i_beam(b, 2.0 * half_len - 0.04, bm_d, bm_w, bm_tf, bm_tw)
        });
        for side in [-1.0f32, 1.0] {
            plate(
                b,
                at(Vec3::new(0.0, y_bot + bm_d * 0.5, side * (half_len - 0.01))),
                Vec3::new(0.215, bm_d * 0.5 + 0.005, 0.01),
                0.004,
            );
        }
        let (x0, x1) = (bm_tw * 0.5, bm_w * 0.5 - 0.012);
        let (y0, y1) = (y_bot + bm_tf, y_top - bm_tf);
        let snipe = 0.022;
        let stiff = [
            Vec2::new(x0, y0 + snipe),
            Vec2::new(x0 + snipe, y0),
            Vec2::new(x1, y0),
            Vec2::new(x1, y1),
            Vec2::new(x0 + snipe, y1),
            Vec2::new(x0, y1 - snipe),
        ];
        let mirrored: Vec<Vec2> = stiff.iter().map(|v| Vec2::new(-v.x, v.y)).collect();
        let mut zs = vec![0.0f32];
        for side in [-1.0f32, 1.0] {
            for f in [-1.0f32, 1.0] {
                zs.push(side * s2 + f * (col_d * 0.5 - col_tf * 0.5));
            }
        }
        for z in zs {
            for o in [&stiff[..], &mirrored[..]] {
                plate_outline(b, at(Vec3::new(0.0, 0.0, z)), o, 0.014, 0.002);
            }
        }
    });
    // Beam-to-column bolts: nuts on the bottom flange, heads under the cap plate.
    for side in [-1.0f32, 1.0] {
        for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let (x, z) = (dx * 0.13, side * s2 + dz * 0.16);
            hex_nut(&mut s, at(Vec3::new(x, y_bot + bm_tf, z)), 0.036, 0.022);
            hex_head(
                &mut s,
                rot_at(Quat::from_rotation_x(PI), Vec3::new(x, y_col_top, z)),
                0.036,
                0.018,
            );
        }
    }
    // Knee braces: double angles on gusset plates, between each column and the beam.
    for side in [-1.0f32, 1.0] {
        knee_brace(&mut p, &mut s, side, s2 - col_d * 0.5, y_bot, 1.2);
    }
    // The pivot hanger: a bolted top plate with two lugs and a pin through the pivot.
    p.colored(paint(77), |b| {
        plate(
            b,
            at(Vec3::new(0.0, y_bot - 0.0125, 0.0)),
            Vec3::new(0.17, 0.0125, 0.185),
            0.004,
        );
        let top = y_bot - 0.025;
        let mut lug = vec![Vec2::new(-0.12, top), Vec2::new(0.12, top)];
        for k in 0..=12 {
            let a = -PI * k as f32 / 12.0;
            lug.push(Vec2::new(0.12 * a.cos(), pivot_height + 0.12 * a.sin()));
        }
        for z in [-0.1f32, 0.1] {
            plate_outline(b, at(Vec3::new(0.0, 0.0, z)), &lug, 0.03, 0.004);
            // Reinforcing boss around the pin hole.
            let ring = [
                Vec2::new(0.07, -0.001),
                Vec2::new(0.07, 0.012),
                Vec2::new(0.036, 0.012),
                Vec2::new(0.036, 0.006),
            ];
            let q = y_to(Vec3::Z * z.signum());
            b.placed(
                rot_at(q, Vec3::new(0.0, pivot_height, z + z.signum() * 0.015)),
                |b| b.lathe(&ring, 20, 40.0),
            );
            for dz in [-1.0f32, 1.0] {
                let wz = z + dz * 0.019;
                weld(
                    b,
                    Vec3::new(-0.12, top - 0.004, wz),
                    Vec3::new(0.12, top - 0.004, wz),
                    0.005,
                );
            }
        }
    });
    s.placed(
        rot_at(
            Quat::from_rotation_x(FRAC_PI_2),
            Vec3::new(0.0, pivot_height, 0.0),
        ),
        |b| b.cylinder(0.035, 0.165, 18, 0.004),
    );
    let head = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.056, 0.0),
        Vec2::new(0.056, 0.012),
        Vec2::new(0.04, 0.018),
        Vec2::new(0.0, 0.018),
    ];
    s.placed(
        rot_at(y_to(Vec3::Z), Vec3::new(0.0, pivot_height, 0.127)),
        |b| b.lathe(&head, 20, 40.0),
    );
    hex_nut(
        &mut s,
        rot_at(y_to(-Vec3::Z), Vec3::new(0.0, pivot_height, -0.127)),
        0.065,
        0.032,
    );
    for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (x, z) = (dx * 0.12, dz * 0.15);
        hex_nut(&mut s, at(Vec3::new(x, y_bot + bm_tf, z)), 0.036, 0.022);
        hex_head(
            &mut s,
            rot_at(Quat::from_rotation_x(PI), Vec3::new(x, y_bot - 0.025, z)),
            0.036,
            0.018,
        );
    }
    Gantry {
        painted: p.build(),
        steel: s.build(),
        concrete: c.build(),
    }
}

/// One knee brace in the YZ plane on the `side` (±1) of the gantry: from the column's inner
/// flange face (`face_z` from the center) `leg` below the beam's underside `y_beam`, to the
/// beam's underside `leg` in from the column.
fn knee_brace(
    p: &mut MeshBuilder,
    s: &mut MeshBuilder,
    side: f32,
    face_z: f32,
    y_beam: f32,
    leg: f32,
) {
    // Work in (zz, y) with zz = side · z, so the column face is at zz = face_z.
    let corner = Vec2::new(face_z, y_beam);
    let w1 = corner - Vec2::new(0.0, leg);
    let w2 = corner - Vec2::new(leg, 0.0);
    let d = (w2 - w1).normalize();
    let pp = Vec2::new(-d.y, d.x) * -1.0; // perpendicular, up and out
    let len = w1.distance(w2);
    let g = 0.018;
    // Gusset on the column (relative to w1), and its mirror image on the beam.
    let q1 = [
        Vec2::new(0.0, -0.1),
        d * 0.55 - pp * 0.11,
        d * 0.55 + pp * 0.11,
        Vec2::new(0.0, 0.42),
    ];
    let col_g: Vec<Vec2> = q1.iter().map(|q| w1 + *q).collect();
    let beam_g: Vec<Vec2> = q1
        .iter()
        .map(|q| corner + Vec2::new(q.y - leg, q.x))
        .collect();
    let to3 = |v: Vec2, x: f32| Vec3::new(x, v.y, side * v.x);
    let rot = Quat::from_rotation_y(-side * FRAC_PI_2);
    p.colored(grey(0.97), |b| {
        for gp in [&col_g, &beam_g] {
            let r = rounded_polygon(gp, &[0.0, 0.012, 0.012, 0.0], 2);
            plate_outline(b, rot_at(rot, Vec3::ZERO), &r, g, 0.003);
        }
        // Welds along the gussets' fixed edges.
        for x in [-g * 0.5 - 0.004, g * 0.5 + 0.004] {
            weld(
                b,
                to3(col_g[0] - Vec2::new(0.004, 0.0), x),
                to3(col_g[3] - Vec2::new(0.004, 0.0), x),
                0.005,
            );
            weld(
                b,
                to3(beam_g[0] - Vec2::new(0.0, 0.004), x),
                to3(beam_g[3] - Vec2::new(0.0, 0.004), x),
                0.005,
            );
        }
        // Two angles back to back on either side of the gussets.
        let (l, t) = (0.09f32, 0.009f32);
        let angle = |sx: f32| -> Vec<Vec2> {
            let pts = [
                Vec2::new(g * 0.5, -l * 0.5),
                Vec2::new(g * 0.5 + l, -l * 0.5),
                Vec2::new(g * 0.5 + l, -l * 0.5 + t),
                Vec2::new(g * 0.5 + t, -l * 0.5 + t),
                Vec2::new(g * 0.5 + t, l * 0.5),
                Vec2::new(g * 0.5, l * 0.5),
            ];
            let r = rounded_polygon(&pts, &[0.0, 0.002, 0.003, 0.008, 0.003, 0.002], 2);
            r.into_iter().map(|v| Vec2::new(sx * v.x, v.y)).collect()
        };
        let (s0, s1) = (0.12, len - 0.12);
        let mid = w1 + d * ((s0 + s1) * 0.5);
        let up3 = Vec3::new(0.0, pp.y, side * pp.x);
        let dir3 = Vec3::new(0.0, d.y, side * d.x);
        let m = frame(up3, dir3, to3(mid, 0.0));
        let xa = up3.cross(dir3).normalize();
        for sx in [-1.0f32, 1.0] {
            b.placed(m, |b| b.extrude(&angle(sx), s1 - s0, 0.002, 50.0));
        }
        // Bolts through angles and gusset at both ends.
        for dist in [0.22f32, 0.4, len - 0.4, len - 0.22] {
            let c = to3(w1 + d * dist, 0.0);
            let off = xa * (g * 0.5 + t);
            hex_head(s, frame(xa, dir3, c + off), 0.03, 0.016);
            hex_nut(s, frame(-xa, dir3, c - off), 0.03, 0.018);
        }
    });
}

// ---------------------------------------------------------------------------------------
// 3. Seesaw
// ---------------------------------------------------------------------------------------

/// The seesaw's plank.
pub struct SeesawPlank {
    /// Weathered timber: four glulam boards side by side (five lamellae each, toned apart by
    /// vertex color), grain along X.
    pub timber: Mesh,
    /// Painted steel: let-in straps, end shoes, hinge cheek plates and the hinge sleeve.
    pub painted: Mesh,
    /// Bare forged steel: through-bolts, nuts and washers.
    pub steel: Mesh,
    /// Black rubber: the bumper pads under both ends (any dark rough material).
    pub rubber: Mesh,
}

/// Timber tint for board `k` of a family `seed`: weathered boards differ a little.
fn board_tint(seed: u32, k: u32) -> Vec4 {
    let v = 0.86 + 0.18 * hash2(seed, k);
    let warm = 0.97 + 0.06 * hash2(seed, k + 1000);
    tint(v * warm, v, v * (2.0 - warm) * 0.98)
}

/// The seesaw plank, `half` its collider's half extents (the demo: 4.0 × 0.1 × 0.6).
///
/// Frame: the plank's body (the collider's center). The top (y = +half.y) is flat timber and
/// let-in steel; the hinge sleeve runs along Z with its axis on the plank's underside at
/// `(0, -half.y, 0)`, the revolute joint's anchor, and hangs 6 cm below it. Steel end shoes
/// cover both ends, rubber bumpers sit flush in the underside at both ends.
pub fn seesaw_plank(half: Vec3) -> SeesawPlank {
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let mut wood = MeshBuilder::new();
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let mut rubber = MeshBuilder::new();
    let shoe = hx - 0.2; // where the end shoes begin
    let pad = hx - 0.65; // where the rubber bumpers begin
    let let_in = 0.006;
    let straps = [-hx * 0.65, -hx * 0.25, hx * 0.25, hx * 0.65];
    let strap_w = 0.06;
    let groove_r = 0.062;
    let end = hx - 0.006;
    // Glued-laminated boards: five lamellae each, the bottom one cut by the hinge groove and
    // stopping short of the rubber pads, the top one let in for the straps and shoes. The
    // glue lines are flush (the lamellae differ in tone only): the outer lamellae carry the
    // board's eased edges and reach `ease` into their neighbours, burying the chamfers that
    // would otherwise groove the glue lines.
    let lam = 5;
    let ease = 0.004;
    let t = 2.0 * hy / lam as f32;
    let pad_h = t;
    let notch = |o: &mut Vec<Vec2>, x: f32, y: f32, depth: f32| {
        let w = strap_w * 0.5 + 0.002;
        let d = depth.signum();
        o.extend([
            Vec2::new(x - w * d, y),
            Vec2::new(x - w * d, y + depth),
            Vec2::new(x + w * d, y + depth),
            Vec2::new(x + w * d, y),
        ]);
    };
    let arc = |a0: f32, a1: f32, steps: u32| -> Vec<Vec2> {
        (0..=steps)
            .map(|k| {
                let a = a0 + (a1 - a0) * k as f32 / steps as f32;
                Vec2::new(groove_r * a.cos(), -hy + groove_r * a.sin())
            })
            .collect()
    };
    let a_cut = (t / groove_r).asin();
    let mut lamellae: Vec<(Vec<Vec2>, f32)> = Vec::new();
    // Bottom lamella, right of the groove (the left one is its mirror).
    let mut l0 = vec![Vec2::new(groove_r, -hy)];
    for &x in straps.iter().filter(|x| **x > 0.0) {
        notch(&mut l0, x, -hy, 0.005);
    }
    let x_cut = groove_r * a_cut.cos();
    l0.extend([
        Vec2::new(pad, -hy),
        Vec2::new(pad, -hy + t + ease),
        Vec2::new(x_cut + 0.012, -hy + t + ease),
        Vec2::new(x_cut + 0.012, -hy + t),
    ]);
    let mut up = arc(a_cut, 0.0, 3);
    up.pop();
    l0.extend(up);
    lamellae.push((
        l0.iter().rev().map(|q| Vec2::new(-q.x, q.y)).collect(),
        ease,
    ));
    lamellae.push((l0, ease));
    // Second lamella, over the top of the groove.
    let mut l1 = vec![Vec2::new(-end, -hy + t)];
    l1.extend(arc(PI - a_cut, a_cut, 8));
    l1.extend([
        Vec2::new(end, -hy + t),
        Vec2::new(end, -hy + 2.0 * t),
        Vec2::new(-end, -hy + 2.0 * t),
    ]);
    lamellae.push((l1, 0.0));
    for k in 2..lam - 1 {
        let (y0, y1) = (-hy + k as f32 * t, -hy + (k + 1) as f32 * t);
        let o = vec![
            Vec2::new(-end, y0),
            Vec2::new(end, y0),
            Vec2::new(end, y1),
            Vec2::new(-end, y1),
        ];
        lamellae.push((o, 0.0));
    }
    let mut top = vec![
        Vec2::new(-end, hy - t - ease),
        Vec2::new(end, hy - t - ease),
        Vec2::new(end, hy - let_in),
        Vec2::new(shoe, hy - let_in),
        Vec2::new(shoe, hy),
    ];
    for &x in straps.iter().rev() {
        notch(&mut top, x, hy, -0.005);
    }
    top.extend([
        Vec2::new(-shoe, hy),
        Vec2::new(-shoe, hy - let_in),
        Vec2::new(-end, hy - let_in),
    ]);
    lamellae.push((top, ease));
    let gap = 0.004;
    let bw = (2.0 * hz - 3.0 * gap) / 4.0;
    for k in 0..4u32 {
        let z = -hz + bw * 0.5 + k as f32 * (bw + gap);
        for (j, (o, bevel)) in lamellae.iter().enumerate() {
            // Lamellae differ in tone, as glulam does.
            let tone = board_tint(11, k) * grey(0.88 + 0.24 * hash2(k * 13 + 5, j as u32));
            wood.colored(tone, |b| {
                b.placed(at(Vec3::new(0.0, 0.0, z)), |b| {
                    b.extrude(o, bw, *bevel, 30.0)
                });
            });
        }
    }
    // Straps wrapped around the plank in their let-in notches.
    let strap_sec = rounded_rect_section(0.005, strap_w, 0.002, 1);
    for (i, &x) in straps.iter().enumerate() {
        let (y, z) = (hy - 0.0025, hz + 0.0025);
        let loop_pts = [
            Vec3::new(x, y, -z),
            Vec3::new(x, y, z),
            Vec3::new(x, -y, z),
            Vec3::new(x, -y, -z),
        ];
        let mut closed: Vec<Vec3> = loop_pts.to_vec();
        closed.push(loop_pts[0]);
        closed.push(loop_pts[1]);
        let path = fillet_path(&closed, &[0.008], 3);
        // Drop the unrounded start and end: the rest closes on itself.
        let ring: Vec<Vec3> = path[1..path.len() - 1].to_vec();
        p.colored(grey(0.9 + 0.08 * hash(i as u32 + 300)), |b| {
            sweep(b, &ring, true, &strap_sec, Vec3::X, false)
        });
    }
    // End shoes: a let-in top plate, an end plate and tapered side cheeks, bolted.
    let len = hx - shoe;
    let cheek_pts = [
        Vec2::new(shoe, hy),
        Vec2::new(hx, hy),
        Vec2::new(hx, -hy),
        Vec2::new(hx - 0.08, -hy),
        Vec2::new(shoe, hy - 0.075),
    ];
    let cheek_r = rounded_polygon(&cheek_pts, &[0.004, 0.002, 0.002, 0.03, 0.02], 3);
    for sx in [-1.0f32, 1.0] {
        p.colored(grey(0.93), |b| {
            plate(
                b,
                at(Vec3::new(sx * (shoe + len * 0.5), hy - let_in * 0.5, 0.0)),
                Vec3::new(len * 0.5, let_in * 0.5, hz + 0.004),
                0.002,
            );
            plate(
                b,
                at(Vec3::new(sx * (hx - 0.003), 0.0, 0.0)),
                Vec3::new(0.003, hy, hz + 0.004),
                0.002,
            );
            let mut outline: Vec<Vec2> = cheek_r.iter().map(|q| Vec2::new(q.x * sx, q.y)).collect();
            if sx < 0.0 {
                outline.reverse();
            }
            for sz in [-1.0f32, 1.0] {
                plate_outline(
                    b,
                    at(Vec3::new(0.0, 0.0, sz * (hz + 0.003))),
                    &outline,
                    0.006,
                    0.0015,
                );
            }
        });
        for k in 0..3 {
            let z = -hz + 0.2 + k as f32 * (2.0 * hz - 0.4) / 2.0;
            dome_head(
                &mut s,
                at(Vec3::new(sx * (hx - len * 0.45), hy, z)),
                0.012,
                0.0035,
                8,
            );
        }
        for sz in [-1.0f32, 1.0] {
            let m = rot_at(
                y_to(Vec3::Z * sz),
                Vec3::new(sx * (hx - 0.05), 0.035, sz * (hz + 0.006)),
            );
            dome_head(&mut s, m, 0.012, 0.005, 8);
        }
        rubber.colored(grey(1.0), |b| {
            let len = end - pad;
            plate(
                b,
                at(Vec3::new(sx * (pad + len * 0.5), -hy + pad_h * 0.5, 0.0)),
                Vec3::new(len * 0.5, pad_h * 0.5, hz - 0.002),
                0.012,
            );
        });
    }
    // Through-bolts across the plank: domed heads on +Z, washers and nuts on -Z.
    for x in [-hx * 0.85, -hx * 0.45, -0.55, 0.55, hx * 0.45, hx * 0.85] {
        dome_head(
            &mut s,
            rot_at(y_to(Vec3::Z), Vec3::new(x, 0.0, hz)),
            0.017,
            0.007,
            10,
        );
        washer(
            &mut s,
            rot_at(y_to(-Vec3::Z), Vec3::new(x, 0.0, -hz)),
            0.012,
            0.022,
            0.003,
            10,
        );
        hex_nut(
            &mut s,
            rot_at(y_to(-Vec3::Z), Vec3::new(x, 0.0, -hz - 0.003)),
            0.026,
            0.014,
        );
    }
    // The hinge: a sleeve let into the underside, held by cheek plates bolted through.
    let sleeve = [
        Vec2::new(0.042, -hz - 0.06),
        Vec2::new(0.06, -hz - 0.06),
        Vec2::new(0.06, hz + 0.06),
        Vec2::new(0.042, hz + 0.06),
        Vec2::new(0.042, -hz - 0.06),
    ];
    p.colored(grey(0.95), |b| {
        b.placed(
            rot_at(Quat::from_rotation_x(FRAC_PI_2), Vec3::new(0.0, -hy, 0.0)),
            |b| b.lathe(&sleeve, 24, 40.0),
        );
        let mut cheek = vec![
            Vec2::new(-0.28, hy - 0.02),
            Vec2::new(0.28, hy - 0.02),
            Vec2::new(0.28, -hy + 0.06),
        ];
        for k in 0..=10 {
            let a = (-10.0f32).to_radians() - (160.0f32).to_radians() * k as f32 / 10.0;
            cheek.push(Vec2::new(0.105 * a.cos(), -hy + 0.105 * a.sin()));
        }
        cheek.push(Vec2::new(-0.28, -hy + 0.06));
        let mut radii = vec![0.0f32; cheek.len()];
        let n = radii.len();
        radii[0] = 0.03;
        radii[1] = 0.03;
        radii[2] = 0.03;
        radii[n - 1] = 0.03;
        let cheek = rounded_polygon(&cheek, &radii, 3);
        for sz in [-1.0f32, 1.0] {
            plate_outline(
                b,
                at(Vec3::new(0.0, 0.0, sz * (hz + 0.006))),
                &cheek,
                0.012,
                0.002,
            );
        }
    });
    for (x, y) in [(-0.2f32, 0.03f32), (0.2, 0.03), (-0.2, -0.04), (0.2, -0.04)] {
        dome_head(
            &mut s,
            rot_at(y_to(Vec3::Z), Vec3::new(x, y, hz + 0.012)),
            0.016,
            0.007,
            10,
        );
        hex_nut(
            &mut s,
            rot_at(y_to(-Vec3::Z), Vec3::new(x, y, -hz - 0.012)),
            0.026,
            0.014,
        );
    }
    SeesawPlank {
        timber: wood.build(),
        painted: p.build(),
        steel: s.build(),
        rubber: rubber.build(),
    }
}

/// The seesaw's stand.
pub struct SeesawStand {
    /// Painted steel: two A-frame trestles with ties, foot plates and pillow-block bearings.
    pub painted: Mesh,
    /// Bare forged steel: the axle, its collars, anchor nuts and bearing bolts.
    pub steel: Mesh,
}

/// The seesaw's stand, `half` its collider's half extents (the demo: 0.3 × 0.4 × 0.6).
///
/// Frame: the stand block's center, so the ground is at y = -half.y and the axle's center
/// (the hinge) at y = +half.y, running along Z. Two A-frame trestles stand just outside the
/// plank's width at z = ±(half.z + 0.1), holding the axle in pillow blocks; everything else
/// stays inside the collider.
pub fn seesaw_stand(half: Vec3) -> SeesawStand {
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let h = half.y * 2.0;
    let axle_y = h;
    let zf = half.z + 0.1;
    let foot_x = half.x;
    let apex_y = axle_y - 0.135;
    let leg_top = Vec2::new(0.05, apex_y - 0.008);
    let leg_x = |y: f32| foot_x + (leg_top.x - foot_x) * (y - 0.012) / (leg_top.y - 0.012);
    let rhs = |b: &mut MeshBuilder, a: Vec3, c: Vec3, w: f32, up: Vec3| {
        let d = c - a;
        let m = frame(up.reject_from(d), d, (a + c) * 0.5);
        b.placed(m, |b| {
            b.rounded_box(Vec3::new(w * 0.5, w * 0.5, d.length() * 0.5), 0.007, 1)
        });
    };
    for (fi, z) in [-zf, zf].into_iter().enumerate() {
        p.colored(grey(0.94 + 0.05 * hash(fi as u32 + 50)), |b| {
            for sx in [-1.0f32, 1.0] {
                let foot = Vec3::new(sx * foot_x, 0.016, z);
                let top = Vec3::new(sx * leg_top.x, leg_top.y, z);
                rhs(b, foot, top, 0.07, Vec3::Z);
                plate(
                    b,
                    at(Vec3::new(sx * foot_x, 0.008, z)),
                    Vec3::new(0.1, 0.008, 0.08),
                    0.003,
                );
                weld(
                    b,
                    foot + Vec3::new(-0.04, -0.003, 0.038),
                    foot + Vec3::new(0.04, -0.003, 0.038),
                    0.005,
                );
                weld(
                    b,
                    foot + Vec3::new(-0.04, -0.003, -0.038),
                    foot + Vec3::new(0.04, -0.003, -0.038),
                    0.005,
                );
                // Longitudinal tie between the trestles, low down.
                if fi == 0 {
                    let y = 0.13;
                    rhs(
                        b,
                        Vec3::new(sx * leg_x(y), y, -zf + 0.03),
                        Vec3::new(sx * leg_x(y), y, zf - 0.03),
                        0.05,
                        Vec3::Y,
                    );
                }
            }
            let y = 0.33;
            rhs(
                b,
                Vec3::new(-leg_x(y) + 0.02, y, z),
                Vec3::new(leg_x(y) - 0.02, y, z),
                0.05,
                Vec3::Y,
            );
            plate(
                b,
                at(Vec3::new(0.0, apex_y, z)),
                Vec3::new(0.15, 0.008, 0.05),
                0.003,
            );
            // The pillow block: a cast housing around the axle.
            let base = apex_y + 0.008;
            let mut o = vec![
                Vec2::new(-0.14, base),
                Vec2::new(0.14, base),
                Vec2::new(0.14, base + 0.026),
                Vec2::new(0.078, base + 0.026),
                Vec2::new(0.078, axle_y),
            ];
            for k in 1..18 {
                let a = PI * k as f32 / 18.0;
                o.push(Vec2::new(0.078 * a.cos(), axle_y + 0.078 * a.sin()));
            }
            o.extend([
                Vec2::new(-0.078, axle_y),
                Vec2::new(-0.078, base + 0.026),
                Vec2::new(-0.14, base + 0.026),
            ]);
            let mut radii = vec![0.0f32; o.len()];
            let n = o.len();
            radii[0] = 0.006;
            radii[1] = 0.006;
            radii[2] = 0.004;
            radii[3] = 0.012;
            radii[n - 2] = 0.012;
            radii[n - 1] = 0.004;
            let o = rounded_polygon(&o, &radii, 2);
            plate_outline(b, at(Vec3::new(0.0, 0.0, z)), &o, 0.065, 0.005);
        });
        // Top tie between the trestles under the axle.
        if fi == 0 {
            p.colored(grey(0.95), |b| {
                rhs(
                    b,
                    Vec3::new(0.0, apex_y - 0.04, -zf),
                    Vec3::new(0.0, apex_y - 0.04, zf),
                    0.06,
                    Vec3::Y,
                )
            });
        }
        // Bearing bolts, grease nipple, foot anchors.
        for sx in [-1.0f32, 1.0] {
            hex_nut(
                &mut s,
                at(Vec3::new(sx * 0.112, apex_y + 0.034, z)),
                0.026,
                0.014,
            );
            for dz in [-0.05f32, 0.05] {
                hex_nut(
                    &mut s,
                    at(Vec3::new(sx * (foot_x + 0.06), 0.016, z + dz)),
                    0.024,
                    0.014,
                );
            }
        }
        let nipple = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.007, 0.0),
            Vec2::new(0.007, 0.012),
            Vec2::new(0.005, 0.014),
            Vec2::new(0.0045, 0.022),
            Vec2::new(0.0, 0.024),
        ];
        s.placed(at(Vec3::new(0.0, axle_y + 0.076, z)), |b| {
            b.lathe(&nipple, 8, 50.0)
        });
    }
    // The axle with collars outside the bearings.
    s.placed(
        rot_at(
            Quat::from_rotation_x(FRAC_PI_2),
            Vec3::new(0.0, axle_y, 0.0),
        ),
        |b| b.cylinder(0.04, zf + 0.1, 20, 0.006),
    );
    for sz in [-1.0f32, 1.0] {
        let collar = [
            Vec2::new(0.04, -0.016),
            Vec2::new(0.058, -0.016),
            Vec2::new(0.06, -0.012),
            Vec2::new(0.06, 0.012),
            Vec2::new(0.058, 0.016),
            Vec2::new(0.04, 0.016),
        ];
        s.placed(
            rot_at(
                Quat::from_rotation_x(FRAC_PI_2),
                Vec3::new(0.0, axle_y, sz * (zf + 0.052)),
            ),
            |b| b.lathe(&collar, 20, 40.0),
        );
        hex_head(
            &mut s,
            rot_at(
                y_to(Vec3::Y),
                Vec3::new(0.0, axle_y + 0.058, sz * (zf + 0.052)),
            ),
            0.014,
            0.01,
        );
    }
    let shift = at(Vec3::new(0.0, -half.y, 0.0));
    let mut out_p = p.build();
    out_p.transform(&shift);
    let mut out_s = s.build();
    out_s.transform(&shift);
    SeesawStand {
        painted: out_p,
        steel: out_s,
    }
}

// ---------------------------------------------------------------------------------------
// 4. Paddle wheel
// ---------------------------------------------------------------------------------------

/// Where the paddle wheel's arms sit along its axle (X), meters from the hub's center.
pub const PADDLE_ARMS: [f32; 3] = [-1.9, 0.0, 1.9];

/// The paddle wheel's hub.
pub struct PaddleHub {
    /// Painted steel: the drum, its end flanges and the arm rings.
    pub painted: Mesh,
    /// Bare forged steel: stub shafts, flange bolts and weld seams.
    pub steel: Mesh,
}

/// The paddle wheel's hub: a steel drum of `radius` (the axle collider's, 0.3 m) running
/// `half_length` (2.7 m) each way along X, with end flanges, a ring pair at each of
/// [`PADDLE_ARMS`] for the paddles' arms, and stub shafts out to x = ±(half_length + 0.72)
/// for the bearings on the bin walls ([`paddle_bin`]).
///
/// Frame: the wheel's body (the hub's center), axle along X; the paddles sit at +Y, +Z, -Y
/// and -Z. Put it on the wheel entity itself (or a child with an identity transform), not on
/// the demo's rotated cylinder child.
pub fn paddle_hub(radius: f32, half_length: f32) -> PaddleHub {
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let to_x = Quat::from_rotation_z(-FRAC_PI_2);
    let hl = half_length - 0.05;
    let drum = [
        Vec2::new(0.0, -hl),
        Vec2::new(radius - 0.012, -hl),
        Vec2::new(radius - 0.003, -hl + 0.003),
        Vec2::new(radius, -hl + 0.012),
        Vec2::new(radius, hl - 0.012),
        Vec2::new(radius - 0.003, hl - 0.003),
        Vec2::new(radius - 0.012, hl),
        Vec2::new(0.0, hl),
    ];
    p.colored(grey(0.96), |b| {
        b.placed(rot_at(to_x, Vec3::ZERO), |b| b.lathe(&drum, 40, 40.0));
        let flange = [
            Vec2::new(0.0, -0.025),
            Vec2::new(0.494, -0.025),
            Vec2::new(0.5, -0.019),
            Vec2::new(0.5, 0.019),
            Vec2::new(0.494, 0.025),
            Vec2::new(0.0, 0.025),
        ];
        for sx in [-1.0f32, 1.0] {
            b.placed(rot_at(to_x, Vec3::new(sx * (hl + 0.025), 0.0, 0.0)), |b| {
                b.lathe(&flange, 40, 40.0)
            });
            let boss = [
                Vec2::new(0.1, 0.0),
                Vec2::new(0.17, 0.0),
                Vec2::new(0.17, 0.05),
                Vec2::new(0.16, 0.06),
                Vec2::new(0.1, 0.06),
            ];
            b.placed(
                rot_at(y_to(Vec3::X * sx), Vec3::new(sx * (hl + 0.05), 0.0, 0.0)),
                |b| b.lathe(&boss, 32, 40.0),
            );
        }
        let ring = [
            Vec2::new(radius - 0.01, -0.0125),
            Vec2::new(0.416, -0.0125),
            Vec2::new(0.42, -0.0085),
            Vec2::new(0.42, 0.0085),
            Vec2::new(0.416, 0.0125),
            Vec2::new(radius - 0.01, 0.0125),
        ];
        for &xa in &PADDLE_ARMS {
            for dx in [-0.0525f32, 0.0525] {
                b.placed(rot_at(to_x, Vec3::new(xa + dx, 0.0, 0.0)), |b| {
                    b.lathe(&ring, 32, 40.0)
                });
            }
        }
    });
    // Weld seams: two around the drum and one along it.
    s.colored(grey(0.75), |b| {
        for x in [-0.95f32, 0.95] {
            let pts: Vec<Vec3> = (0..36)
                .map(|k| {
                    let a = k as f32 / 36.0 * TAU;
                    Vec3::new(x, a.cos() * radius, a.sin() * radius)
                })
                .collect();
            b.tube(&pts, 0.0045, 5, true, false);
        }
        let a = 0.6f32;
        weld(
            b,
            Vec3::new(-hl + 0.02, a.cos() * radius, a.sin() * radius),
            Vec3::new(hl - 0.02, a.cos() * radius, a.sin() * radius),
            0.0045,
        );
    });
    for sx in [-1.0f32, 1.0] {
        let len = 0.72 - 0.05;
        knuckle(
            &mut s,
            rot_at(to_x, Vec3::new(sx * (half_length + len * 0.5), 0.0, 0.0)),
            0.1,
            len * 0.5,
            24,
        );
        for k in 0..12 {
            let a = (k as f32 + 0.5) / 12.0 * TAU;
            let o = Vec3::new(sx * (hl + 0.05), a.cos() * 0.43, a.sin() * 0.43);
            hex_head(&mut s, rot_at(y_to(Vec3::X * sx), o), 0.034, 0.02);
        }
    }
    PaddleHub {
        painted: p.build(),
        steel: s.build(),
    }
}

/// One paddle of the wheel.
pub struct PaddleBlade {
    /// Weathered timber: five boards, grain along X.
    pub timber: Mesh,
    /// Painted steel: the arms behind the boards and the edge cap.
    pub painted: Mesh,
    /// Bare forged steel: carriage bolts, nuts and the arms' root bolts.
    pub steel: Mesh,
}

/// One paddle, `half` its collider's half extents (the demo: 2.6 × 0.75 × 0.05), with its
/// center `center_radius` (1.05 m) from the axle.
///
/// Frame: the paddle's entity (the collider's center): X along the axle, Y radially outward
/// (the drum's surface at y = -half.y), Z through the blade; +Z is the leading face (the
/// wheel turns +X, which moves +Z forward), the arms are on the trailing (-Z) side. All four
/// paddles use the same mesh.
pub fn paddle_blade(half: Vec3, center_radius: f32) -> PaddleBlade {
    let mut w = MeshBuilder::new();
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let cap = 0.006;
    let gap = 0.005;
    let arm_t = 0.035;
    let board_z0 = -hz + arm_t;
    let hb = (2.0 * hy - cap - 4.0 * gap) / 5.0;
    let board_center = |k: usize| -hy + hb * 0.5 + k as f32 * (hb + gap);
    for k in 0..5 {
        w.colored(board_tint(23, k as u32), |b| {
            plate(
                b,
                at(Vec3::new(0.0, board_center(k), (board_z0 + hz) * 0.5)),
                Vec3::new(hx, hb * 0.5, (hz - board_z0) * 0.5),
                0.008,
            )
        });
    }
    let root = -hy - 0.02;
    let arm_top = hy - 0.05;
    p.colored(grey(0.95), |b| {
        for &xa in &PADDLE_ARMS {
            plate(
                b,
                at(Vec3::new(xa, (root + arm_top) * 0.5, -hz + arm_t * 0.5)),
                Vec3::new(0.04, (arm_top - root) * 0.5, arm_t * 0.5),
                0.005,
            );
        }
        // Edge cap: a channel over the outer edge.
        let zc = (board_z0 - 0.004 + hz + 0.004) * 0.5;
        plate(
            b,
            at(Vec3::new(0.0, hy - cap * 0.5, zc)),
            Vec3::new(hx, cap * 0.5, (hz + 0.004 - board_z0 + 0.004) * 0.5),
            0.002,
        );
        for (z, _) in [(hz + 0.002, 1.0f32), (board_z0 - 0.002, -1.0)] {
            plate(
                b,
                at(Vec3::new(0.0, hy - 0.026, z)),
                Vec3::new(hx, 0.025, 0.002),
                0.0015,
            );
        }
    });
    for &xa in &PADDLE_ARMS {
        for k in 0..5 {
            let y = board_center(k);
            dome_head(
                &mut s,
                rot_at(y_to(Vec3::Z), Vec3::new(xa, y, hz)),
                0.013,
                0.005,
                8,
            );
            hex_nut(
                &mut s,
                rot_at(y_to(-Vec3::Z), Vec3::new(xa, y, -hz)),
                0.022,
                0.008,
            );
        }
        // Root bolt through the hub's ring pair.
        let y = 0.37 - center_radius;
        hex_head(
            &mut s,
            rot_at(y_to(Vec3::X), Vec3::new(xa + 0.065, y, -hz + arm_t * 0.5)),
            0.03,
            0.016,
        );
        hex_nut(
            &mut s,
            rot_at(y_to(-Vec3::X), Vec3::new(xa - 0.065, y, -hz + arm_t * 0.5)),
            0.03,
            0.02,
        );
    }
    PaddleBlade {
        timber: w.build(),
        painted: p.build(),
        steel: s.build(),
    }
}

// ---------------------------------------------------------------------------------------
// 5. Bins
// ---------------------------------------------------------------------------------------

/// A concrete bin (the pile bin or the paddle wheel's).
pub struct Bin {
    /// Concrete: the walls (and the paddle bin's bearing pedestals).
    pub concrete: Mesh,
    /// Painted steel: cap channels and joint plates, or the bearing pillow blocks.
    pub painted: Mesh,
    /// Bare forged steel: lifting anchors, bolts and nuts.
    pub steel: Mesh,
}

/// A wall along Z (wall-local: centered on x = 0, ground at y = 0) for [`pile_bin`] and
/// [`paddle_bin`], built into `b` (concrete) with anchor heads into `s`. `ties` puts form-tie
/// holes on both faces at the given heights every `tie_step` meters; `anchors` puts lifting
/// anchor pockets in the top at the given z.
#[allow(clippy::too_many_arguments)]
fn bin_wall(
    b: &mut MeshBuilder,
    s: &mut MeshBuilder,
    m: Mat4,
    t: f32,
    h: f32,
    z: [f32; 2],
    chamfer: [f32; 2],
    anchors: &[f32],
    ties: &[f32],
    tie_step: f32,
    seed: u32,
) {
    let r = 0.022;
    let cell = 0.07;
    let acell = 0.22;
    let mut section = Vec::new();
    // Up the +X face, over the rounded top, down the -X face.
    let face = |x: f32, nx: f32, rows: &mut Vec<Sec>, up: bool| {
        let mut ys = vec![0.0f32, 0.16];
        for &ty in ties {
            ys.push(ty - cell * 0.5);
            ys.push(ty + cell * 0.5);
        }
        ys.push(h - r);
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if !up {
            ys.reverse();
        }
        for y in ys {
            let mut q = sec(Vec2::new(x, y), Vec2::new(nx, 0.0));
            q.shade = if y < 0.01 { 0.8 } else { 1.0 };
            rows.push(q);
        }
    };
    face(t * 0.5, 1.0, &mut section, true);
    section.extend(sec_round(
        Vec2::new(t * 0.5 - r, h - r),
        r,
        0.0,
        FRAC_PI_2,
        3,
    ));
    let top_j = section.len() - 1;
    section.push(sec(Vec2::new(-t * 0.5 + r, h), Vec2::Y));
    section.extend(sec_round(
        Vec2::new(-t * 0.5 + r, h - r),
        r,
        FRAC_PI_2,
        PI,
        3,
    ));
    section.pop(); // the round's end is the face's first point
    face(-t * 0.5, -1.0, &mut section, false);
    // Rings: ends, anchor cells, tie columns.
    let (z0, z1) = (z[0] + chamfer[0], z[1] - chamfer[1]);
    let mut rings = vec![z0, z1];
    for &za in anchors {
        rings.extend([za - acell * 0.5, za + acell * 0.5]);
    }
    let mut tie_cols = Vec::new();
    if !ties.is_empty() {
        let len = z1 - z0;
        let n = ((len - 0.3) / tie_step).floor().max(0.0) as usize;
        let start = z0 + (len - n as f32 * tie_step) * 0.5;
        for k in 0..=n {
            let zt = start + k as f32 * tie_step;
            tie_cols.push(zt);
            rings.extend([zt - cell * 0.5, zt + cell * 0.5]);
        }
    }
    rings.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let ring_of = |zv: f32| rings.iter().position(|q| (q - zv).abs() < 1e-5).unwrap();
    let anchor_rings: Vec<usize> = anchors
        .iter()
        .map(|&za| ring_of(za - acell * 0.5))
        .collect();
    let tie_rings: Vec<usize> = tie_cols
        .iter()
        .map(|&zt| ring_of(zt - cell * 0.5))
        .collect();
    let tie_segs: Vec<usize> = (0..section.len() - 1)
        .filter(|&j| {
            let (a, c) = (section[j].p, section[j + 1].p);
            (a.x - c.x).abs() < 1e-6
                && ties.iter().any(|&ty| {
                    (a.y.min(c.y) - (ty - cell * 0.5)).abs() < 1e-5
                        && (a.y.max(c.y) - (ty + cell * 0.5)).abs() < 1e-5
                })
        })
        .collect();
    let shade = 0.94 + 0.07 * hash(seed);
    let skip = |j: usize, i: usize| {
        (j == top_j && anchor_rings.contains(&i))
            || (tie_segs.contains(&j) && tie_rings.contains(&i))
    };
    b.placed(m, |b| {
        let idx = prism(
            &mut [&mut *b],
            &section,
            false,
            &rings,
            chamfer,
            [true, true],
            grey(shade),
            &skip,
        );
        let corner = |i: usize, j: usize| {
            [
                idx[0][i][j].unwrap(),
                idx[0][i][j + 1].unwrap(),
                idx[0][i + 1][j + 1].unwrap(),
                idx[0][i + 1][j].unwrap(),
            ]
        };
        for &i in &anchor_rings {
            let mr = recess_patch(b, corner(i, top_j), Vec3::Y, 0.05, 0.045, 16, 0.6);
            // The anchor's spherical head at the bottom of the pocket.
            let head = [
                Vec2::new(0.0, -0.045),
                Vec2::new(0.011, -0.045),
                Vec2::new(0.011, -0.03),
                Vec2::new(0.02, -0.028),
                Vec2::new(0.02, -0.02),
                Vec2::new(0.0, -0.018),
            ];
            s.placed(m * mr, |s| s.lathe(&head, 12, 40.0));
        }
        for &j in &tie_segs {
            let nx = section[j].n.x;
            for &i in &tie_rings {
                recess_patch(b, corner(i, j), Vec3::X * nx, 0.017, 0.022, 8, 0.55);
            }
        }
    });
}

/// The pile bin: precast concrete wall panels with chamfered edges and joints, lifting
/// anchor pockets in their tops, a painted steel cap channel, and bolted joint plates
/// outside. `inner_half` is the inside's half size (the demo: 4.0 × 4.0), walls `thickness`
/// (0.3) thick and `height` (1.6) tall.
///
/// Frame: the bin's center on the ground. The side walls (x = ±(inner.x + t/2)) run the full
/// length z = ±(inner.y + t); the end walls (z = ±(inner.y + t/2)) fit between them, as the
/// demo's four colliders do. The cap channels stand 6 mm proud of the walls' faces and top.
pub fn pile_bin(inner_half: Vec2, thickness: f32, height: f32) -> Bin {
    let mut c = MeshBuilder::new();
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let t = thickness;
    let joint = 0.006;
    let mut seed = 400;
    // (wall-local transform, z range, outer side +1/-1 in wall-local x)
    let mut walls: Vec<(Mat4, [f32; 2], f32)> = Vec::new();
    for sx in [-1.0f32, 1.0] {
        let m = at(Vec3::new(sx * (inner_half.x + t * 0.5), 0.0, 0.0));
        let l = inner_half.y + t;
        walls.push((m, [-l, -joint * 0.5], sx));
        walls.push((m, [joint * 0.5, l], sx));
    }
    for sz in [-1.0f32, 1.0] {
        let m = rot_at(
            Quat::from_rotation_y(FRAC_PI_2),
            Vec3::new(0.0, 0.0, sz * (inner_half.y + t * 0.5)),
        );
        let l = inner_half.x - 0.003;
        // Wall-local x maps to world -z here.
        walls.push((m, [-l, -joint * 0.5], -sz));
        walls.push((m, [joint * 0.5, l], -sz));
    }
    for (m, z, outer) in walls {
        seed += 1;
        let len = z[1] - z[0];
        let anchors = [z[0] + len * 0.25, z[0] + len * 0.75];
        bin_wall(
            &mut c,
            &mut s,
            m,
            t,
            height,
            z,
            [0.015, 0.015],
            &anchors,
            &[],
            1.0,
            seed,
        );
        // Cap channel in three pieces around the anchor pockets.
        let (ct, leg) = (0.006, 0.07);
        let (hx, h) = (t * 0.5, height);
        let u = [
            Vec2::new(-hx - ct, h - leg),
            Vec2::new(-hx, h - leg),
            Vec2::new(-hx, h),
            Vec2::new(hx, h),
            Vec2::new(hx, h - leg),
            Vec2::new(hx + ct, h - leg),
            Vec2::new(hx + ct, h + ct),
            Vec2::new(-hx - ct, h + ct),
        ];
        let cuts = [
            z[0] + 0.01,
            anchors[0] - 0.16,
            anchors[0] + 0.16,
            anchors[1] - 0.16,
            anchors[1] + 0.16,
            z[1] - 0.01,
        ];
        p.colored(grey(0.92 + 0.06 * hash(seed + 50)), |b| {
            for w in cuts.chunks(2) {
                let (a, e) = (w[0], w[1]);
                b.placed(m * at(Vec3::new(0.0, 0.0, (a + e) * 0.5)), |b| {
                    b.extrude(&u, e - a, 0.0025, 30.0)
                });
            }
        });
        // Joint plates on the outside where two panels meet (once per joint).
        if z[0] > 0.0 && z[0] < 0.01 {
            let x = outer * (t * 0.5 + 0.006);
            for y in [0.4f32, height - 0.4] {
                p.colored(grey(0.9), |b| {
                    plate(
                        b,
                        m * at(Vec3::new(x, y, 0.0)),
                        Vec3::new(0.006, 0.09, 0.16),
                        0.002,
                    );
                });
                {
                    for (dy, dz) in [(-0.05f32, -0.1f32), (0.05, -0.1), (-0.05, 0.1), (0.05, 0.1)] {
                        let o = Vec3::new(x + outer * 0.006, y + dy, dz);
                        hex_head(&mut s, m * rot_at(y_to(Vec3::X * outer), o), 0.026, 0.014);
                    }
                }
            }
        }
    }
    Bin {
        concrete: c.build(),
        painted: p.build(),
        steel: s.build(),
    }
}

/// The paddle wheel's bin: cast-in-place concrete walls with chamfered edges and form-tie
/// holes, and concrete bearing pedestals on the side walls carrying pillow blocks for the
/// hub's stub shafts at `axle_height`. Sizes as in [`pile_bin`] (the demo: inner 3.0 × 2.5,
/// 0.3 thick, 1.4 tall, axle at 2.0 m).
///
/// Frame: the bin's center on the ground; the wheel's axle runs along X at
/// `(·, axle_height, 0)`.
pub fn paddle_bin(inner_half: Vec2, thickness: f32, height: f32, axle_height: f32) -> Bin {
    let mut c = MeshBuilder::new();
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let t = thickness;
    let ties = [height * 0.27, height * 0.73];
    for (k, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
        let m = at(Vec3::new(sx * (inner_half.x + t * 0.5), 0.0, 0.0));
        let l = inner_half.y + t;
        bin_wall(
            &mut c,
            &mut s,
            m,
            t,
            height,
            [-l, l],
            [0.02, 0.02],
            &[],
            &ties,
            0.62,
            600 + k as u32,
        );
    }
    for (k, sz) in [-1.0f32, 1.0].into_iter().enumerate() {
        let m = rot_at(
            Quat::from_rotation_y(FRAC_PI_2),
            Vec3::new(0.0, 0.0, sz * (inner_half.y + t * 0.5)),
        );
        let l = inner_half.x;
        bin_wall(
            &mut c,
            &mut s,
            m,
            t,
            height,
            [-l, l],
            [0.0, 0.0],
            &[],
            &ties,
            0.62,
            610 + k as u32,
        );
    }
    // Bearing pedestals and pillow blocks.
    let shaft_r = 0.1;
    let hr = 0.165;
    let base = axle_height - hr * 0.75;
    let sole = 0.02;
    let ped_top = base - sole;
    for sx in [-1.0f32, 1.0] {
        let x = sx * (inner_half.x + t * 0.5);
        let ph = ped_top - height;
        c.colored(grey(0.97), |b| {
            plate(
                b,
                at(Vec3::new(x, height + ph * 0.5, 0.0)),
                Vec3::new(t * 0.5 + 0.02, ph * 0.5, 0.26),
                0.02,
            );
        });
        p.colored(grey(0.93), |b| {
            plate(
                b,
                at(Vec3::new(x, ped_top + sole * 0.5, 0.0)),
                Vec3::new(0.12, sole * 0.5, 0.37),
                0.003,
            );
            let mut o = vec![
                Vec2::new(-0.3, base),
                Vec2::new(0.3, base),
                Vec2::new(0.3, base + 0.035),
                Vec2::new(hr, base + 0.035),
                Vec2::new(hr, axle_height),
            ];
            for k in 1..20 {
                let a = PI * k as f32 / 20.0;
                o.push(Vec2::new(hr * a.cos(), axle_height + hr * a.sin()));
            }
            o.extend([
                Vec2::new(-hr, axle_height),
                Vec2::new(-hr, base + 0.035),
                Vec2::new(-0.3, base + 0.035),
            ]);
            let n = o.len();
            let mut radii = vec![0.0f32; n];
            radii[0] = 0.008;
            radii[1] = 0.008;
            radii[2] = 0.006;
            radii[3] = 0.02;
            radii[n - 2] = 0.02;
            radii[n - 1] = 0.006;
            let o = rounded_polygon(&o, &radii, 2);
            plate_outline(
                b,
                rot_at(Quat::from_rotation_y(FRAC_PI_2), Vec3::new(x, 0.0, 0.0)),
                &o,
                0.17,
                0.006,
            );
            // Seals where the shaft leaves the housing.
            let seal = [
                Vec2::new(shaft_r, -0.012),
                Vec2::new(shaft_r + 0.03, -0.012),
                Vec2::new(shaft_r + 0.03, 0.012),
                Vec2::new(shaft_r, 0.012),
            ];
            for dx in [-0.09f32, 0.09] {
                b.placed(
                    rot_at(
                        Quat::from_rotation_z(-FRAC_PI_2),
                        Vec3::new(x + dx, axle_height, 0.0),
                    ),
                    |b| b.lathe(&seal, 24, 40.0),
                );
            }
        });
        for dz in [-0.24f32, 0.24] {
            for dx in [-0.05f32, 0.05] {
                hex_nut(&mut s, at(Vec3::new(x + dx, base + 0.035, dz)), 0.034, 0.02);
            }
        }
        // Holding-down bolts through the sole plate.
        for dz in [-0.335f32, 0.335] {
            hex_nut(&mut s, at(Vec3::new(x, ped_top + sole, dz)), 0.03, 0.018);
        }
    }
    Bin {
        concrete: c.build(),
        painted: p.build(),
        steel: s.build(),
    }
}

/// A flush, low button head (countersunk bolt or rivet) on the XZ plane around +Y.
fn button(b: &mut MeshBuilder, m: Mat4, r: f32, h: f32, sides: u32) {
    let prof = [
        Vec2::new(r, -0.001),
        Vec2::new(r, h * 0.3),
        Vec2::new(r * 0.55, h),
        Vec2::new(0.0, h),
    ];
    b.placed(m, |b| b.lathe(&prof, sides, 60.0));
}

// ---------------------------------------------------------------------------------------
// 6. Turntable
// ---------------------------------------------------------------------------------------

/// The turntable.
pub struct Turntable {
    /// Diamond plate: eight deck plates with small gaps, each with its pattern (U) running
    /// along its own center line.
    pub deck: Mesh,
    /// Painted steel: the rim band (a channel with a recessed web) and the floor under the
    /// deck's seams (vertex-tinted dark).
    pub painted: Mesh,
    /// Bare forged steel: the center hub cap and every bolt head.
    pub steel: Mesh,
}

/// The turntable: a disc of `radius` (4 m) and `height` (0.3 m).
///
/// Frame: the table's body (the disc's center), Y up, top at y = +height/2. The deck plates,
/// rim and hub cap are flat at the top (bolt heads stand 1.5 mm proud at most), the rim's
/// outside is the collider's cylinder.
pub fn turntable(radius: f32, height: f32) -> Turntable {
    let mut deck = MeshBuilder::new();
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let h2 = height * 0.5;
    let rim_w = 0.1;
    let (r_in, r_out) = (0.42f32, radius - rim_w);
    let plate_t = 0.008;
    let e = 0.0025;
    let d = 0.003;
    let sectors = 8usize;
    let (ncol, nrad) = (16usize, 8usize);
    for k in 0..sectors {
        let th0 = k as f32 / sectors as f32 * TAU;
        let th1 = (k + 1) as f32 / sectors as f32 * TAU;
        let mid = (th0 + th1) * 0.5;
        let dir = Vec2::new(mid.cos(), mid.sin());
        let perp = Vec2::new(-dir.y, dir.x);
        // (radius, level, radial side): level 0 = underside, 1 = bevel, 2 = top.
        let mut rows: Vec<(f32, u8, f32)> = vec![(r_in, 0, -1.0), (r_in, 1, -1.0)];
        for i in 0..=nrad {
            rows.push((
                r_in + e + (r_out - r_in - 2.0 * e) * i as f32 / nrad as f32,
                2,
                0.0,
            ));
        }
        rows.extend([(r_out, 1, 1.0), (r_out, 0, 1.0)]);
        // (fraction or edge offset, level, edge side)
        let mut cols: Vec<(f32, u8, f32)> = vec![(d, 0, -1.0), (d, 1, -1.0)];
        for c in 0..=ncol {
            cols.push((c as f32 / ncol as f32, 2, 0.0));
        }
        cols.extend([(d, 1, 1.0), (d, 0, 1.0)]);
        let shade = grey(0.93 + 0.08 * hash(k as u32 + 70));
        grid_colored(&mut deck, rows.len(), cols.len(), |ri, ci| {
            let (r, rl, rs) = rows[ri];
            let (f, cl, cs) = cols[ci];
            let level = rl.min(cl);
            let ang = if cs < 0.0 {
                th0 + (f / r).asin()
            } else if cs > 0.0 {
                th1 - (f / r).asin()
            } else {
                let a0 = th0 + ((d + e) / r).asin();
                let a1 = th1 - ((d + e) / r).asin();
                a0 + (a1 - a0) * f
            };
            let y = match level {
                0 => h2 - plate_t,
                1 => h2 - e,
                _ => h2,
            };
            let q = Vec2::new(r * ang.cos(), r * ang.sin());
            let n = if level == 2 {
                Vec3::Y
            } else {
                let radial = Vec2::new(ang.cos(), ang.sin()) * rs;
                let edge = if cs < 0.0 {
                    Vec2::new(th0.sin(), -th0.cos())
                } else if cs > 0.0 {
                    Vec2::new(-th1.sin(), th1.cos())
                } else {
                    Vec2::ZERO
                };
                let o = (radial + edge).normalize_or(Vec2::X);
                Vec3::new(o.x, 0.0, o.y)
            };
            (
                Vec3::new(q.x, y, q.y),
                n,
                Vec2::new(q.dot(dir), q.dot(perp)),
                shade,
            )
        });
    }
    // The floor under the seams, dark.
    p.colored(grey(0.22), |b| {
        b.lathe(
            &[
                Vec2::new(r_out + 0.004, h2 - 0.014),
                Vec2::new(r_in - 0.03, h2 - 0.014),
            ],
            96,
            40.0,
        )
    });
    // The rim: a rolled channel, its web recessed between the flanges and stiffened by ribs.
    let rr = radius;
    let web = 0.03;
    let shaded = |p: Vec2, n: Vec2, shade: f32| Sec { shade, ..sec(p, n) };
    let mut prof = vec![sec(Vec2::new(rr - 0.008, -h2), -Vec2::Y)];
    prof.extend(sec_round(
        Vec2::new(rr - 0.008, -h2 + 0.008),
        0.008,
        -FRAC_PI_2,
        0.0,
        2,
    ));
    prof.push(sec(Vec2::new(rr, -0.103), Vec2::X));
    prof.extend(sec_round(
        Vec2::new(rr - 0.003, -0.103),
        0.003,
        0.0,
        FRAC_PI_2,
        2,
    ));
    prof.extend([
        shaded(Vec2::new(rr - web, -0.1), Vec2::Y, 0.6),
        shaded(Vec2::new(rr - web, -0.1), Vec2::X, 0.55),
        shaded(Vec2::new(rr - web, -0.07), Vec2::X, 0.8),
        shaded(Vec2::new(rr - web, 0.07), Vec2::X, 0.8),
        shaded(Vec2::new(rr - web, 0.1), Vec2::X, 0.5),
        shaded(Vec2::new(rr - web, 0.1), -Vec2::Y, 0.5),
        shaded(Vec2::new(rr - 0.003, 0.1), -Vec2::Y, 0.8),
    ]);
    prof.extend(sec_round(
        Vec2::new(rr - 0.003, 0.103),
        0.003,
        -FRAC_PI_2,
        0.0,
        2,
    ));
    prof.push(sec(Vec2::new(rr, h2 - 0.012), Vec2::X));
    prof.extend(sec_round(
        Vec2::new(rr - 0.012, h2 - 0.012),
        0.012,
        0.0,
        FRAC_PI_2,
        3,
    ));
    prof.push(sec(Vec2::new(r_out + 0.003, h2), Vec2::Y));
    prof.extend(sec_round(
        Vec2::new(r_out + 0.003, h2 - 0.003),
        0.003,
        FRAC_PI_2,
        PI,
        2,
    ));
    prof.push(shaded(Vec2::new(r_out, h2 - 0.014), -Vec2::X, 0.4));
    p.colored(grey(0.97), |b| revolve(b, &prof, 96));
    for k in 0..32 {
        let a = (k as f32 + 0.5) / 32.0 * TAU;
        let radial = Vec3::new(a.cos(), 0.0, a.sin());
        dome_head(
            &mut s,
            rot_at(y_to(radial), radial * (rr - web)),
            0.011,
            0.0055,
            8,
        );
    }
    p.colored(grey(0.93), |b| {
        for k in 0..16 {
            let a = k as f32 / 16.0 * TAU;
            let m = rot_at(
                Quat::from_rotation_y(-a),
                Vec3::new(a.cos(), 0.0, a.sin()) * (rr - web * 0.5 - 0.002),
            );
            plate(b, m, Vec3::new(web * 0.5 + 0.002, 0.1, 0.005), 0.002);
        }
    });
    // Countersunk bolts along each plate's seams.
    for k in 0..sectors {
        let th0 = k as f32 / sectors as f32 * TAU;
        let th1 = (k + 1) as f32 / sectors as f32 * TAU;
        for r in [1.0f32, 2.0, 3.0, 3.72] {
            for (th, side) in [(th0, 1.0f32), (th1, -1.0)] {
                let a = th + side * (0.04 / r);
                button(
                    &mut s,
                    at(Vec3::new(r * a.cos(), h2, r * a.sin())),
                    0.01,
                    0.0015,
                    6,
                );
            }
        }
    }
    // The hub cap, flush, with its own ring of bolts.
    let cap = [
        Vec2::new(0.0, h2 - 0.012),
        Vec2::new(0.4, h2 - 0.012),
        Vec2::new(0.4, h2 - 0.003),
        Vec2::new(0.397, h2),
        Vec2::new(0.085, h2),
        Vec2::new(0.07, h2 + 0.004),
        Vec2::new(0.0, h2 + 0.005),
    ];
    s.lathe(&cap, 48, 40.0);
    for k in 0..8 {
        let a = (k as f32 + 0.5) / 8.0 * TAU;
        button(
            &mut s,
            at(Vec3::new(0.3 * a.cos(), h2, 0.3 * a.sin())),
            0.012,
            0.0015,
            8,
        );
    }
    Turntable {
        deck: deck.build(),
        painted: p.build(),
        steel: s.build(),
    }
}

// ---------------------------------------------------------------------------------------
// 7. Bridge
// ---------------------------------------------------------------------------------------

/// One plank of the bridge.
pub struct BridgePlank {
    /// Weathered timber: three boards across the deck, grain along X.
    pub timber: Mesh,
    /// Bare forged steel: the two cables under the boards, their hinge knuckles and the
    /// U-bolts clamping each board.
    pub steel: Mesh,
}

/// Radius of the bridge's cables, meters.
pub const BRIDGE_CABLE: f32 = 0.016;

/// One bridge plank, `half` its collider's half extents (the demo: 1.0 × 0.06 × ~0.41);
/// `index` varies the boards from plank to plank.
///
/// Frame: the plank's body (the collider's center): X across the deck, Z along it, the
/// hinges' axes along X through `(·, 0, ±half.z)`. The cables run along Z at x = ±half.x,
/// y = 0, ending exactly on the hinge axes, so neighbours' cables meet however the deck
/// sags; a knuckle at each hinge (the -Z end's outer half, the +Z end's inner half) hides
/// the kink. Boards are flush with the top (y = +half.y) and overhang the cables by 10 cm.
pub fn bridge_plank(half: Vec3, index: usize) -> BridgePlank {
    let mut w = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let rc = BRIDGE_CABLE;
    let (end_gap, gap) = (0.01, 0.02);
    let bw = (2.0 * hz - 2.0 * end_gap - 2.0 * gap) / 3.0;
    let seed = 900 + index as u32 * 7;
    let bt = hy - rc;
    let mut centers = Vec::new();
    for k in 0..3 {
        let zc =
            -hz + end_gap + bw * 0.5 + k as f32 * (bw + gap) + (hash2(seed, k + 10) - 0.5) * 0.006;
        let yaw = (hash2(seed, k + 20) - 0.5) * 0.012;
        let len = hx + 0.08 + 0.04 * hash2(seed, k + 30);
        let shift = (hash2(seed, k + 40) - 0.5) * 0.03;
        centers.push(zc);
        w.colored(board_tint(seed, k), |b| {
            plate(
                b,
                rot_at(
                    Quat::from_rotation_y(yaw),
                    Vec3::new(shift, hy - bt * 0.5, zc),
                ),
                Vec3::new(len, bt * 0.5, bw * 0.5),
                0.006,
            )
        });
    }
    let to_x = Quat::from_rotation_z(-FRAC_PI_2);
    for sx in [-1.0f32, 1.0] {
        let x = sx * hx;
        s.colored(grey(0.85), |b| {
            b.tube(
                &[Vec3::new(x, 0.0, -hz), Vec3::new(x, 0.0, hz)],
                rc,
                8,
                false,
                false,
            );
        });
        s.colored(grey(0.95), |b| {
            // Knuckles: outer half at -Z, inner half at +Z.
            for (z, dx) in [(-hz, 0.0225f32), (hz, -0.0225)] {
                knuckle(
                    b,
                    rot_at(to_x, Vec3::new(x + sx * dx, 0.0, z)),
                    0.03,
                    0.0225,
                    10,
                );
            }
            dome_head(
                b,
                rot_at(y_to(Vec3::X * sx), Vec3::new(x + sx * 0.045, 0.0, -hz)),
                0.014,
                0.006,
                8,
            );
            dome_head(
                b,
                rot_at(y_to(-Vec3::X * sx), Vec3::new(x - sx * 0.045, 0.0, hz)),
                0.014,
                0.006,
                8,
            );
            // U-bolts round the cable, through each board, with nuts on top.
            let ru = 0.024;
            for &zc in &centers {
                let mut path = vec![Vec3::new(x + ru, rc + 0.006, zc)];
                for k in 0..=4 {
                    let a = -PI * k as f32 / 4.0;
                    path.push(Vec3::new(x + ru * a.cos(), ru * a.sin(), zc));
                }
                path.push(Vec3::new(x - ru, rc + 0.006, zc));
                b.tube(&path, 0.005, 5, false, false);
                for dx in [-ru, ru] {
                    hex_body(b, at(Vec3::new(x + dx, hy, zc)), 0.014, 0.005);
                }
            }
        });
    }
    BridgePlank {
        timber: w.build(),
        steel: s.build(),
    }
}

/// One of the bridge's towers.
pub struct BridgeTower {
    /// Weathered timber: corner posts, cladding, the cap beam and the gate above the deck.
    pub timber: Mesh,
    /// Bare forged steel: corner angles, bands, cable anchors, backstays, turnbuckles, bolts.
    pub steel: Mesh,
    /// Cast concrete: the plinth and the backstays' ground anchors.
    pub concrete: Mesh,
}

/// A bridge tower, `half` its collider's half extents (the demo: 1.3 × 2.25 × 0.3), with
/// the deck on its `deck_side` (+1: the deck is toward +Z, the near tower at -Z; -1 for the
/// far tower).
///
/// Frame: the tower block's center: ground at y = -half.y, its top (flush timber) at
/// +half.y. The deck's end hinges sit at `(±1.0, half.y, deck_side · half.z)`, where steel
/// anchors hold the cables' knuckles; backstays run from the anchors over the cap beam to
/// concrete ground anchors 2.6 m behind the tower. A timber gate stands 1.45 m above the
/// cap. Everything outside the collider is render-only.
pub fn bridge_tower(half: Vec3, deck_side: f32) -> BridgeTower {
    let mut w = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let mut c = MeshBuilder::new();
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let yg = -hy;
    let plinth = 0.15;
    let cap_h = 0.2;
    let post = 0.3;
    let y0 = yg + plinth;
    let y1 = hy - cap_h;
    let seed = if deck_side > 0.0 { 1200 } else { 1300 };
    c.colored(grey(0.95), |b| {
        plate(
            b,
            at(Vec3::new(0.0, yg + plinth * 0.5, 0.0)),
            Vec3::new(hx + 0.07, plinth * 0.5, hz + 0.07),
            0.02,
        )
    });
    // Corner posts (doubled), cladding and the cap beam.
    for (k, (sx, sz)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        w.colored(board_tint(seed, k as u32), |b| {
            plate(
                b,
                at(Vec3::new(
                    sx * (hx - post * 0.5),
                    (y0 + y1) * 0.5,
                    sz * post * 0.5,
                )),
                Vec3::new(post * 0.5, (y1 - y0) * 0.5, post * 0.5),
                0.014,
            )
        });
    }
    let n_boards = 18;
    let bgap = 0.012;
    let bh = ((y1 - y0) - (n_boards - 1) as f32 * bgap) / n_boards as f32;
    // Their ends butt into the posts, so they are open sweeps.
    let board = rounded_rect_section(0.05, bh, 0.008, 2);
    let lx = hx - post - 0.002;
    for sz in [-1.0f32, 1.0] {
        for k in 0..n_boards {
            let y = y0 + bh * 0.5 + k as f32 * (bh + bgap);
            let z = sz * (hz - 0.025);
            w.colored(
                board_tint(seed + if sz > 0.0 { 7 } else { 5 }, k as u32),
                |b| {
                    sweep(
                        b,
                        &[Vec3::new(-lx, y, z), Vec3::new(lx, y, z)],
                        false,
                        &board,
                        Vec3::Y,
                        false,
                    )
                },
            );
        }
    }
    let cap_front = hz - 0.05;
    let cap_back = -hz - 0.06;
    w.colored(board_tint(seed, 9), |b| {
        plate(
            b,
            at(Vec3::new(
                0.0,
                hy - cap_h * 0.5,
                (cap_front + cap_back) * 0.5,
            )),
            Vec3::new(hx + 0.08, cap_h * 0.5, (cap_front - cap_back) * 0.5),
            0.012,
        )
    });
    // The gate above the deck: two posts, a lintel, knee braces.
    let gate_h = 1.25;
    let gp = 0.26;
    w.colored(board_tint(seed, 11), |b| {
        for sx in [-1.0f32, 1.0] {
            plate(
                b,
                at(Vec3::new(sx * (hx - post * 0.5), hy + gate_h * 0.5, -0.05)),
                Vec3::new(gp * 0.5, gate_h * 0.5, gp * 0.5),
                0.012,
            );
            let a = Vec3::new(sx * (hx - post * 0.5 - gp * 0.5), hy + gate_h - 0.42, -0.05);
            let e2 = Vec3::new(sx * (hx - post * 0.5 - gp * 0.5 - 0.42), hy + gate_h, -0.05);
            let dvec = e2 - a;
            let m = frame(Vec3::Z, dvec, (a + e2) * 0.5);
            b.placed(m, |b| {
                b.rounded_box(Vec3::new(0.06, 0.06, dvec.length() * 0.5 + 0.03), 0.008, 1)
            });
        }
        plate(
            b,
            at(Vec3::new(0.0, hy + gate_h + 0.11, -0.05)),
            Vec3::new(hx + 0.12, 0.11, 0.14),
            0.012,
        );
    });
    // Steel: corner angles, bands, anchors, backstays.
    let ang = |sx: f32, sz: f32| -> Vec<Vec2> {
        // In (x, -z): legs along the two faces meeting at the corner.
        let (cx, cz) = (sx * hx, sz * hz);
        let t = 0.008;
        let l = 0.09;
        let pts = [
            Vec2::new(cx - sx * l, -cz),
            Vec2::new(cx - sx * l, -(cz + sz * t)),
            Vec2::new(cx + sx * t, -(cz + sz * t)),
            Vec2::new(cx + sx * t, -(cz - sz * l)),
            Vec2::new(cx, -(cz - sz * l)),
            Vec2::new(cx, -cz),
        ];
        rounded_polygon(&pts, &[0.0, 0.002, 0.004, 0.002, 0.0, 0.0], 2)
    };
    for (k, (sx, sz)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        s.colored(grey(0.85 + 0.1 * hash(seed + k as u32)), |b| {
            b.placed(
                rot_at(
                    Quat::from_rotation_x(-FRAC_PI_2),
                    Vec3::new(0.0, (y0 + y1) * 0.5, 0.0),
                ),
                |b| b.extrude(&ang(sx, sz), y1 - y0 - 0.02, 0.002, 50.0),
            );
            for j in 0..4 {
                let y = y0 + 0.3 + j as f32 * (y1 - y0 - 0.6) / 3.0;
                dome_head(
                    b,
                    rot_at(
                        y_to(Vec3::X * sx),
                        Vec3::new(sx * (hx + 0.008), y, sz * (hz - 0.045)),
                    ),
                    0.012,
                    0.005,
                    8,
                );
                dome_head(
                    b,
                    rot_at(
                        y_to(Vec3::Z * sz),
                        Vec3::new(sx * (hx - 0.045), y, sz * (hz + 0.008)),
                    ),
                    0.012,
                    0.005,
                    8,
                );
            }
        });
    }
    let band = rounded_rect_section(0.006, 0.08, 0.002, 1);
    for y in [y0 + 1.0, y0 + 2.8] {
        let (bx, bz) = (hx + 0.011, hz + 0.011);
        let pts = [
            Vec3::new(-bx, y, -bz),
            Vec3::new(bx, y, -bz),
            Vec3::new(bx, y, bz),
            Vec3::new(-bx, y, bz),
            Vec3::new(-bx, y, -bz),
            Vec3::new(bx, y, -bz),
        ];
        let path = fillet_path(&pts, &[0.012], 3);
        let ring: Vec<Vec3> = path[1..path.len() - 1].to_vec();
        s.colored(grey(0.8), |b| sweep(b, &ring, true, &band, Vec3::Y, false));
    }
    let to_x = Quat::from_rotation_z(-FRAC_PI_2);
    for sx in [-1.0f32, 1.0] {
        let x = sx * 1.0;
        s.colored(grey(0.9), |b| {
            // Anchor bracket: base plate on the cap, front plate, lug and the knuckle's inner half.
            plate(
                b,
                at(Vec3::new(
                    x,
                    hy + 0.008,
                    (cap_front - 0.28 + cap_front) * 0.5,
                )),
                Vec3::new(0.09, 0.008, 0.14),
                0.003,
            );
            plate(
                b,
                at(Vec3::new(x, hy - 0.09, cap_front + 0.008)),
                Vec3::new(0.09, 0.105, 0.008),
                0.003,
            );
            let mut lug = vec![
                Vec2::new(cap_front + 0.016, hy - 0.045),
                Vec2::new(hz, hy - 0.045),
            ];
            for k in 1..12 {
                let a = -FRAC_PI_2 + PI * k as f32 / 12.0;
                lug.push(Vec2::new(hz + 0.045 * a.cos(), hy + 0.045 * a.sin()));
            }
            lug.extend([
                Vec2::new(hz, hy + 0.045),
                Vec2::new(cap_front + 0.016, hy + 0.045),
            ]);
            let lx = x - sx * (0.0225 * 2.0 + 0.012);
            plate_outline(
                b,
                rot_at(Quat::from_rotation_y(-FRAC_PI_2), Vec3::new(lx, 0.0, 0.0)),
                &lug,
                0.02,
                0.003,
            );
            knuckle(
                b,
                rot_at(to_x, Vec3::new(x - sx * 0.0225, hy, hz)),
                0.03,
                0.0225,
                10,
            );
            for (dz, dx) in [
                (-0.05f32, -0.055f32),
                (-0.05, 0.055),
                (-0.22, -0.055),
                (-0.22, 0.055),
            ] {
                hex_nut(
                    b,
                    at(Vec3::new(x + dx, hy + 0.016, cap_front + dz)),
                    0.024,
                    0.012,
                );
            }
            // Backstay: from the bracket's tail over a saddle on the cap's back edge, down to
            // a ground anchor behind the tower.
            let tail = Vec3::new(x, hy + 0.016 + BRIDGE_CABLE, cap_front - 0.27);
            let saddle = Vec3::new(x, hy + 0.016 + BRIDGE_CABLE, cap_back + 0.02);
            let anchor = Vec3::new(x, yg + 0.42, -2.6);
            let path = fillet_path(&[tail, saddle, anchor], &[0.18], 8);
            b.tube(&path, BRIDGE_CABLE, 8, false, true);
            plate(
                b,
                at(Vec3::new(x, hy + 0.004, cap_back + 0.06)),
                Vec3::new(0.05, 0.004, 0.06),
                0.002,
            );
            // A turnbuckle partway down.
            let dirv = (anchor - saddle).normalize();
            let tb = saddle.lerp(anchor, 0.55);
            let body = [
                Vec2::new(0.0, -0.14),
                Vec2::new(0.012, -0.14),
                Vec2::new(0.024, -0.1),
                Vec2::new(0.024, 0.1),
                Vec2::new(0.012, 0.14),
                Vec2::new(0.0, 0.14),
            ];
            b.placed(rot_at(y_to(dirv), tb), |b| b.lathe(&body, 10, 30.0));
            // Shackle and eye plate on the ground anchor.
            b.placed(
                rot_at(
                    y_to(dirv) * Quat::from_rotation_y(FRAC_PI_2),
                    anchor + dirv * 0.03,
                ),
                |b| b.chain_link(0.11, 0.06, 0.011, 8, 6),
            );
            plate(
                b,
                at(Vec3::new(x, yg + 0.3 + 0.006, -2.6)),
                Vec3::new(0.12, 0.006, 0.12),
                0.003,
            );
            plate(
                b,
                at(Vec3::new(x, yg + 0.36, -2.6)),
                Vec3::new(0.01, 0.06, 0.05),
                0.003,
            );
        });
        c.colored(grey(0.93), |b| {
            plate(
                b,
                at(Vec3::new(x, yg + 0.15, -2.6)),
                Vec3::new(0.35, 0.15, 0.3),
                0.025,
            )
        });
    }
    let mut out = BridgeTower {
        timber: w.build(),
        steel: s.build(),
        concrete: c.build(),
    };
    if deck_side < 0.0 {
        let flip = Mat4::from_rotation_y(PI);
        for m in [&mut out.timber, &mut out.steel, &mut out.concrete] {
            m.transform(&flip);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// 8. Yard: slab and curb
// ---------------------------------------------------------------------------------------

/// The yard's slab and curb.
pub struct Yard {
    /// Concrete slab (`concrete_slab`, its 6 m joints): the pad's top in 6 m panels with
    /// small saw-cut joints at x, z = 3 + 6k, and its sides. UV = (x + 3, z + 3) meters, so a
    /// texture tiling every 6 m has its joints on the geometry's.
    pub slab: Mesh,
    /// Cast concrete: the precast curb segments (3 m, with joints).
    pub curb: Mesh,
    /// The curb's inner face, upper band and rounded top edge, for `hazard_stripes`: U along
    /// the curb in meters, V from 0 at the top edge downward (the band is 0.45 m).
    pub hazard: Mesh,
}

/// The yard: a `size` (70 × 50 m) slab, top at y = 0 and `depth` (0.5 m) thick, and a curb
/// `curb_height` (0.8) tall and `curb_thickness` (0.4) thick along its four sides, as the
/// demo's colliders (the curbs along ±Z span the full width, the ones along ±X fit between).
///
/// Frame: the yard's origin, the center of the slab's top. The curb's inner faces are flat
/// and match the colliders; its top edges are rounded (8 cm inside, 4 cm outside).
pub fn yard(size: [f32; 2], depth: f32, curb_height: f32, curb_thickness: f32) -> Yard {
    let mut slab = MeshBuilder::new();
    let (hx, hz) = (size[0] * 0.5, size[1] * 0.5);
    let joints = |h: f32| -> Vec<f32> {
        let mut v = vec![-h];
        let mut k = ((-h - 3.0) / 6.0).ceil() as i32;
        loop {
            let x = 3.0 + 6.0 * k as f32;
            if x >= h - 0.5 {
                break;
            }
            if x > -h + 0.5 {
                v.push(x);
            }
            k += 1;
        }
        v.push(h);
        v
    };
    let (jx, jz) = (joints(hx), joints(hz));
    let e = 0.004;
    let outer_e = 0.02;
    for (ix, wx) in jx.windows(2).enumerate() {
        for (iz, wz) in jz.windows(2).enumerate() {
            let (x0, x1, z0, z1) = (wx[0], wx[1], wz[0], wz[1]);
            let ex0 = if ix == 0 { outer_e } else { e };
            let ex1 = if ix + 2 == jx.len() { outer_e } else { e };
            let ez0 = if iz == 0 { outer_e } else { e };
            let ez1 = if iz + 2 == jz.len() { outer_e } else { e };
            let axis = |a: f32, b: f32, ea: f32, eb: f32| -> Vec<(f32, f32, f32)> {
                // (coordinate, edge depth, outward sign)
                let n = ((b - a) / 1.5).ceil().max(1.0) as usize;
                let mut v = vec![(a, ea, -1.0)];
                for k in 0..=n {
                    v.push((a + ea + (b - a - ea - eb) * k as f32 / n as f32, 0.0, 0.0));
                }
                v.push((b, eb, 1.0));
                v
            };
            let xs = axis(x0, x1, ex0, ex1);
            let zs = axis(z0, z1, ez0, ez1);
            let pk = (ix * 31 + iz * 17) as u32;
            let shade = 0.93 + 0.08 * hash(pk + 5000);
            let warm = 0.985 + 0.03 * hash(pk + 6000);
            grid_colored(&mut slab, zs.len(), xs.len(), |r, c| {
                let (x, ex, sx) = xs[c];
                let (z, ez, sz) = zs[r];
                let y = -ex.max(ez);
                let n = if sx != 0.0 || sz != 0.0 {
                    Vec3::new(sx, 0.0, sz).normalize()
                } else {
                    Vec3::Y
                };
                let mottle =
                    0.94 + 0.12 * noise1(x * 0.37 + z * 0.11, 77) * noise1(z * 0.41 - x * 0.07, 78);
                (
                    Vec3::new(x, y, z),
                    n,
                    Vec2::new(x + 3.0, z + 3.0),
                    tint(
                        shade * warm * mottle,
                        shade * mottle,
                        shade * mottle * (2.0 - warm),
                    ),
                )
            });
            // The slab's sides along the yard's edge, under its outer chamfer.
            let mut side = |pts: Vec<Vec3>, n: Vec3| {
                grid_colored(&mut slab, 2, pts.len(), |r, c| {
                    let p = pts[c];
                    let y = if r == 0 { p.y } else { -depth };
                    let u = p.x * n.z.abs() + p.z * n.x.abs();
                    (
                        Vec3::new(p.x, y, p.z),
                        n,
                        Vec2::new(u, y),
                        grey(if r == 0 { 0.95 } else { 0.7 }),
                    )
                });
            };
            let ring_x: Vec<Vec3> = xs
                .iter()
                .map(|&(x, ..)| Vec3::new(x, -outer_e, 0.0))
                .collect();
            let ring_z: Vec<Vec3> = zs
                .iter()
                .map(|&(z, ..)| Vec3::new(0.0, -outer_e, z))
                .collect();
            if iz == 0 {
                side(
                    ring_x
                        .iter()
                        .map(|p| Vec3::new(p.x, -outer_e, z0))
                        .collect(),
                    -Vec3::Z,
                );
            }
            if iz + 2 == jz.len() {
                side(
                    ring_x
                        .iter()
                        .map(|p| Vec3::new(p.x, -outer_e, z1))
                        .collect(),
                    Vec3::Z,
                );
            }
            if ix == 0 {
                side(
                    ring_z
                        .iter()
                        .map(|p| Vec3::new(x0, -outer_e, p.z))
                        .collect(),
                    -Vec3::X,
                );
            }
            if ix + 2 == jx.len() {
                side(
                    ring_z
                        .iter()
                        .map(|p| Vec3::new(x1, -outer_e, p.z))
                        .collect(),
                    Vec3::X,
                );
            }
        }
    }
    // The curb: precast segments about 3 m long.
    let mut curb = MeshBuilder::new();
    let mut hazard = MeshBuilder::new();
    let (h, t) = (curb_height, curb_thickness);
    let (ri, ro) = (0.08f32, 0.04f32);
    let band = 0.45;
    let mut section = vec![
        sec(Vec2::new(t * 0.5, 0.0), Vec2::X),
        sec(Vec2::new(t * 0.5, 0.16), Vec2::X),
        sec(Vec2::new(t * 0.5, h - ro), Vec2::X),
    ];
    section[0].shade = 0.78;
    section.extend(sec_round(
        Vec2::new(t * 0.5 - ro, h - ro),
        ro,
        0.0,
        FRAC_PI_2,
        3,
    ));
    let mut top_end = sec(Vec2::new(-t * 0.5 + ri, h), Vec2::Y);
    top_end.out = 1;
    section.push(top_end);
    let mut round = sec_round(Vec2::new(-t * 0.5 + ri, h - ri), ri, FRAC_PI_2, PI, 4);
    for q in &mut round {
        q.out = 1;
    }
    section.extend(round);
    let mut band_end = sec(Vec2::new(-t * 0.5, h - band), -Vec2::X);
    band_end.out = 0;
    section.push(band_end);
    section.push(sec(Vec2::new(-t * 0.5, 0.16), -Vec2::X));
    let mut foot = sec(Vec2::new(-t * 0.5, 0.0), -Vec2::X);
    foot.shade = 0.78;
    section.push(foot);
    // (frame, length): local Z along the curb, local -X toward the yard.
    let runs = [
        (
            rot_at(
                Quat::from_rotation_y(-FRAC_PI_2),
                Vec3::new(0.0, 0.0, hz - t * 0.5),
            ),
            2.0 * hx,
        ),
        (
            rot_at(
                Quat::from_rotation_y(FRAC_PI_2),
                Vec3::new(0.0, 0.0, -hz + t * 0.5),
            ),
            2.0 * hx,
        ),
        (at(Vec3::new(hx - t * 0.5, 0.0, 0.0)), 2.0 * (hz - t)),
        (
            rot_at(
                Quat::from_rotation_y(PI),
                Vec3::new(-hx + t * 0.5, 0.0, 0.0),
            ),
            2.0 * (hz - t),
        ),
    ];
    let mut seg_id = 0u32;
    for (m, len) in runs {
        let n = (len / 3.0).round().max(1.0) as usize;
        let seg = len / n as f32;
        for k in 0..n {
            seg_id += 1;
            let z0 = -len * 0.5 + k as f32 * seg + 0.005;
            let z1 = z0 + seg - 0.01;
            let c = 0.015;
            let shade = grey(0.93 + 0.08 * hash(seg_id + 7000));
            let mut cb = MeshBuilder::new();
            let mut hb = MeshBuilder::new();
            prism(
                &mut [&mut cb, &mut hb],
                &section,
                false,
                &[z0 + c, z1 - c],
                [c, c],
                [true, true],
                shade,
                &|_, _| false,
            );
            curb.append(&cb.mesh, m);
            // Hazard paint is the same on every segment: no concrete tint.
            for v in &mut hb.mesh.vertices {
                v.color = [1.0; 4];
            }
            hazard.append(&hb.mesh, m);
        }
    }
    Yard {
        slab: slab.build(),
        curb: curb.build(),
        hazard: hazard.build(),
    }
}
