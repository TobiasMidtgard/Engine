//! The grapple station: a nine-meter precast tower nobody climbs without the hook. A glowing
//! anchor plate under its top edge is the target and painted chevrons lead the eye up to it.
//! Railings guard only its sides: the front edge is where the rope pulls a pilot over, and the
//! +X side opens where the zip line ([`super::zip_station`]) leaves the roof. Saw-cut pad bays with start, take-off and landing
//! paint, a numbered backlit sign (in [`build_signs`]), a floodlight on a pole, a door, a
//! downpipe and roof gear.

use super::*;

/// Height of the joint between the tower's two lifts of precast panels.
const LIFT: f32 = 4.6;
/// Rail heights above the roof.
const RAIL_Y: [f32; 2] = [0.55, 1.07];
/// The floodlight's head, aimed at the target (relative to the demo origin).
const FLOOD_HEAD: [f32; 3] = [3.55, 6.7, 19.3];

/// Joints splitting `a..b` into bays of about `bay` meters.
fn bays(a: f32, b: f32, bay: f32) -> Vec<f32> {
    let n = ((b - a) / bay).round().max(1.0) as usize;
    (0..=n).map(|i| a + (b - a) * i as f32 / n as f32).collect()
}

pub(super) fn build() -> Kit {
    let (p0, p1) = bounds(layout::GRAPPLE_PAD);
    let (t0, t1) = bounds(layout::GRAPPLE_TOWER);
    let mut kit = Kit::default();
    pad(&mut kit, p0, p1, t0, t1);
    tower(&mut kit, p1.y, t0, t1);
    roof(&mut kit, t0, t1);
    target(&mut kit, t0);
    floodlight(&mut kit, p1.y);
    kit
}

/// The pad in saw-cut bays (joints in line with the tower's faces), hazard-banded sides, and
/// paint: the start, arrows to the take-off line, and the landing box behind the tower.
fn pad(kit: &mut Kit, p0: Vec3, p1: Vec3, t0: Vec3, t1: Vec3) {
    let top = p1.y;
    let take = layout::GRAPPLE_TAKEOFF_Z as f32;
    let mid = (t0.x + t1.x) * 0.5;
    let land_z = (t1.z + p1.z) * 0.5;
    let xs = [p0.x, t0.x, mid, t1.x, p1.x];
    let mut zs = bays(p0.z, t0.z, 3.3);
    zs.extend(bays(t1.z, p1.z, 3.0));
    let g = 0.003;
    let mut concrete = M::new();
    for xw in xs.windows(2) {
        for zw in zs.windows(2) {
            let ga = |v: f32, edge: f32| if (v - edge).abs() < 1e-3 { 0.0 } else { g };
            let min = v3(xw[0] + ga(xw[0], p0.x), -0.3, zw[0] + ga(zw[0], p0.z));
            let max = v3(xw[1] - ga(xw[1], p1.x), top, zw[1] - ga(zw[1], p1.z));
            concrete.append(&slab(min, max, 0.008, 0.7), Mat4::IDENTITY);
        }
    }
    concrete.shade(|p, _| {
        let q = Vec2::new(p.x, p.z);
        let d = rect_dist(q, Vec2::new(t0.x, t0.z), Vec2::new(t1.x, t1.z));
        let ao = 1.0 - 0.45 * (-d / 0.4).exp();
        // Scuffed where pilots push off and where they land.
        let push = (-((p.z - take) / 0.7).powi(2) - ((p.x - mid) / 1.4).powi(2)).exp();
        let land = (-(Vec2::new(p.x - mid, p.z - land_z).length() / 1.3).powi(2)).exp();
        let blot = 0.9 + 0.1 * pn(p.x * 0.25, p.z * 0.25, 17);
        blot * ao * (1.0 - 0.12 * push - 0.14 * land)
    });
    kit.add(CONCRETE, concrete);

    let mut hz = M::new();
    let t = 0.004;
    for (min, max) in [
        (v3(p0.x, -0.02, p0.z - t), v3(p1.x, top - 0.012, p0.z)),
        (v3(p0.x, -0.02, p1.z), v3(p1.x, top - 0.012, p1.z + t)),
        (v3(p0.x - t, -0.02, p0.z), v3(p0.x, top - 0.012, p1.z)),
        (v3(p1.x, -0.02, p0.z), v3(p1.x + t, top - 0.012, p1.z)),
    ] {
        hz.append(&slab(min, max, 0.0, 100.0), Mat4::IDENTITY);
    }
    kit.add(HAZARD, hz);

    let mut p = M::new();
    let y = top + 0.0025;
    let yellow = paint_color(YELLOW_PAINT);
    let white = paint_color(WHITE_PAINT);
    let orange = paint_color(ORANGE_PAINT);
    // Lane edges from the start to the tower's foot.
    for x in [mid - 1.6, mid + 1.6] {
        let (za, zb) = (p0.z + 0.4, t0.z - 0.3);
        p_line(
            &mut p,
            v3(x, y, (za + zb) * 0.5),
            Vec3::Z,
            zb - za,
            0.06,
            white,
        );
    }
    // The start (read by a pilot heading +Z), arrows, and the take-off line.
    let start = layout::GRAPPLE_START[2] as f32;
    stencil(
        &mut p,
        6,
        v3(mid, y, start - 0.55),
        Vec3::NEG_X,
        Vec3::Z,
        0.34,
        white,
    );
    p_line(&mut p, v3(mid, y, start), Vec3::X, 3.2, 0.06, white);
    stencil(
        &mut p,
        9,
        v3(mid, y, (start + take) * 0.5),
        Vec3::Z,
        Vec3::X,
        0.42,
        white,
    );
    p_line(&mut p, v3(mid, y, take), Vec3::X, 3.2, 0.1, yellow);
    stencil(
        &mut p,
        8,
        v3(mid, y, take - 0.45),
        Vec3::NEG_X,
        Vec3::Z,
        0.3,
        yellow,
    );
    // The station number big on the approach, for the camera over the course.
    stencil(
        &mut p,
        10,
        v3(mid, y, take + 2.6),
        Vec3::NEG_X,
        Vec3::Z,
        1.6,
        orange,
    );
    stencil(
        &mut p,
        11,
        v3(mid, y, take + 4.2),
        Vec3::NEG_X,
        Vec3::Z,
        0.34,
        white,
    );
    // Landing box behind the tower.
    let land = v3(mid, y, land_z);
    for (dx, dz, len, dir) in [
        (0.0, -1.3, 3.0, Vec3::X),
        (0.0, 1.3, 3.0, Vec3::X),
        (-1.5, 0.0, 2.68, Vec3::Z),
        (1.5, 0.0, 2.68, Vec3::Z),
    ] {
        p_line(&mut p, land + v3(dx, 0.0, dz), dir, len, 0.08, white);
    }
    stencil(&mut p, 9, land, Vec3::Z, Vec3::X, 0.45, white);
    kit.add(PAINT, p);
}

/// The tower: a concrete core and top slab, two lifts of precast panels, steel nosing and
/// corner guards, side railings, a door and a downpipe at the back, and painted numbers.
fn tower(kit: &mut Kit, base: f32, t0: Vec3, t1: Vec3) {
    let top = t1.y;
    let mid = (t0.x + t1.x) * 0.5;
    let target = DVec3::from_array(layout::GRAPPLE_TARGET).as_vec3();
    let th = 0.2;
    let g = 0.005;
    let mut concrete = slab(
        v3(t0.x + 0.1, base, t0.z + 0.1),
        v3(t1.x - 0.1, top - 0.12, t1.z - 0.1),
        0.0,
        2.0,
    );
    concrete.append(
        &slab(v3(t0.x, top - 0.12, t0.z), v3(t1.x, top, t1.z), 0.012, 0.5),
        Mat4::IDENTITY,
    );
    concrete.shade(|p, n| {
        // Worn down the lane pilots cross.
        let worn = (-((p.x - mid) / 1.2).powi(2)).exp();
        let top_face = if n.y > 0.5 { 0.93 - 0.12 * worn } else { 1.0 };
        top_face * (0.92 + 0.08 * pn(p.x * 0.6, p.z * 0.6, 420))
    });
    kit.add(CONCRETE, concrete);

    // Panels: three across the front and back, two along each side, in two lifts.
    let ytop = top - 0.12;
    let lifts = [(base, LIFT - g, 0.0), (LIFT + g, ytop, 1.13)];
    let inset = 0.07;
    let mut panels = M::new();
    let mut panel = |min: Vec3, max: Vec3, axis: Vec3, start: f32, w: f32, v_off: f32| {
        let mut p = slab(min, max, 0.02, 0.3);
        panel_uvs(&mut p, axis, start, w);
        for v in &mut p.vertices {
            v.uv[1] += v_off;
        }
        panels.append(&p, Mat4::IDENTITY);
    };
    for (y0, y1, v_off) in lifts {
        for (x_out, x_in) in [(t0.x, t0.x + th), (t1.x, t1.x - th)] {
            let (za, zb) = (t0.z + inset, t1.z - inset);
            let w = (zb - za) / 2.0;
            for i in 0..2 {
                let z0 = za + i as f32 * w;
                panel(
                    v3(x_out.min(x_in), y0, z0 + g),
                    v3(x_out.max(x_in), y1, z0 + w - g),
                    Vec3::Z,
                    z0,
                    w,
                    v_off,
                );
            }
        }
        for (z_out, z_in) in [(t0.z, t0.z + th), (t1.z, t1.z - th)] {
            let (xa, xb) = (t0.x + inset, t1.x - inset);
            let w = (xb - xa) / 3.0;
            for i in 0..3 {
                let x0 = xa + i as f32 * w;
                panel(
                    v3(x0 + g, y0, z_out.min(z_in)),
                    v3(x0 + w - g, y1, z_out.max(z_in)),
                    Vec3::X,
                    x0,
                    w,
                    v_off,
                );
            }
        }
    }
    wall_grime(&mut panels, base, ytop, 421, |p| p.x + p.z);
    // Boot scuffs on the front face under the target, where reeled-in pilots kick the wall.
    panels.shade(|p, n| {
        if n.z > -0.7 {
            return 1.0;
        }
        let d = Vec2::new((p.x - target.x) / 1.1, (p.y - (target.y - 1.6)) / 1.3);
        let streak = 0.6 + 0.4 * pn(p.x * 3.0, p.y * 0.8, 422);
        1.0 - 0.3 * (-d.length_squared()).exp() * streak
    });
    kit.add(PANEL, panels);

    let mut dark = M::new();
    let mut yellow = M::new();
    let mut steel = M::new();
    // Nosing angles: dark on the front and back edges, yellow on the sides; corner guards.
    let a = 0.07;
    let s = 0.006;
    dark.append(
        &bx(v3(t0.x - s, top - a, t0.z - s), v3(t1.x + s, top, t0.z)),
        Mat4::IDENTITY,
    );
    dark.append(
        &bx(v3(t0.x - s, top - a, t1.z), v3(t1.x + s, top, t1.z + s)),
        Mat4::IDENTITY,
    );
    for (xa, xb) in [(t0.x - s, t0.x), (t1.x, t1.x + s)] {
        yellow.append(
            &bx(v3(xa, top - a, t0.z), v3(xb, top, t1.z)),
            Mat4::IDENTITY,
        );
    }
    for (x, z) in [(t0.x, t0.z), (t1.x, t0.z), (t0.x, t1.z), (t1.x, t1.z)] {
        let sx = if x == t0.x { -1.0 } else { 1.0 };
        let sz = if z == t0.z { -1.0 } else { 1.0 };
        let leg = 0.11;
        dark.append(
            &bx(
                v3(x.min(x + sx * s), base, z.min(z - sz * leg)),
                v3(x.max(x + sx * s), top - a, z.max(z - sz * leg)),
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &bx(
                v3(x.min(x - sx * leg), base, z.min(z + sz * s)),
                v3(x.max(x - sx * leg), top - a, z.max(z + sz * s)),
            ),
            Mat4::IDENTITY,
        );
    }
    // Railings down both sides, returning a little onto the front and back edges at the
    // corners; the middle of the front and back stays open, and so does the stretch of the +X
    // side the zip line leaves over.
    let off = 0.05;
    let (gap0, gap1) = zip_gap(t1);
    for (x_edge, inward) in [(t0.x - off, 1.0f32), (t1.x + off, -1.0)] {
        let ret = x_edge + inward * 1.0;
        let runs: Vec<Vec<Vec3>> = if inward > 0.0 {
            vec![vec![
                v3(ret, top, t0.z - off),
                v3(x_edge, top, t0.z - off),
                v3(x_edge, top, t1.z + off),
                v3(ret, top, t1.z + off),
            ]]
        } else {
            vec![
                vec![
                    v3(ret, top, t0.z - off),
                    v3(x_edge, top, t0.z - off),
                    v3(x_edge, top, gap0),
                ],
                vec![
                    v3(x_edge, top, gap1),
                    v3(x_edge, top, t1.z + off),
                    v3(ret, top, t1.z + off),
                ],
            ]
        };
        for path in runs {
            railing(&mut yellow, &mut dark, &path, t0, t1);
        }
    }

    // A door in the back face, with a canopy and a lamp.
    let mut orange = M::new();
    let mut alu = M::new();
    let mut white = M::new();
    let (dx, dz) = (t1.x - 1.6, t1.z);
    let frame = [
        (
            v3(dx - 0.62, base, dz),
            v3(dx - 0.52, base + 2.36, dz + 0.06),
        ),
        (
            v3(dx + 0.52, base, dz),
            v3(dx + 0.62, base + 2.36, dz + 0.06),
        ),
        (
            v3(dx - 0.62, base + 2.26, dz),
            v3(dx + 0.62, base + 2.36, dz + 0.06),
        ),
    ];
    for (min, max) in frame {
        dark.append(&bx(min, max), Mat4::IDENTITY);
    }
    orange.append(
        &slab(
            v3(dx - 0.52, base + 0.01, dz),
            v3(dx + 0.52, base + 2.26, dz + 0.04),
            0.008,
            0.4,
        ),
        Mat4::IDENTITY,
    );
    for (y0, y1) in [(0.25, 0.75), (1.5, 2.0)] {
        orange.append(
            &bx(
                v3(dx - 0.4, base + y0, dz + 0.04),
                v3(dx + 0.4, base + y1, dz + 0.05),
            ),
            Mat4::IDENTITY,
        );
    }
    alu.append(
        &rod(
            v3(dx + 0.38, base + 1.0, dz + 0.1),
            v3(dx + 0.38, base + 1.15, dz + 0.1),
            0.014,
            8,
        ),
        Mat4::IDENTITY,
    );
    alu.append(
        &bx(
            v3(dx + 0.35, base + 0.98, dz + 0.04),
            v3(dx + 0.41, base + 1.17, dz + 0.06),
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &slab(
            v3(dx - 0.8, base + 2.5, dz),
            v3(dx + 0.8, base + 2.54, dz + 0.7),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for xx in [dx - 0.7, dx + 0.7] {
        dark.append(
            &pipe(
                &[v3(xx, base + 2.15, dz), v3(xx, base + 2.5, dz + 0.66)],
                0.015,
                6,
            ),
            Mat4::IDENTITY,
        );
    }
    dark.append(
        &bx(
            v3(dx - 0.1, base + 2.36, dz),
            v3(dx + 0.1, base + 2.46, dz + 0.14),
        ),
        Mat4::IDENTITY,
    );
    white.append(
        &bx(
            v3(dx - 0.08, base + 2.355, dz + 0.02),
            v3(dx + 0.08, base + 2.36, dz + 0.13),
        ),
        Mat4::IDENTITY,
    );
    kit.add(
        TREAD,
        slab(
            v3(dx - 0.8, base, dz),
            v3(dx + 0.8, base + 0.012, dz + 1.0),
            0.004,
            100.0,
        ),
    );

    // A downpipe from a scupper at the back-east corner, bracketed to the side face.
    let px = t1.x + 0.11;
    let pz = t1.z - 0.45;
    steel.append(
        &slab(
            v3(t1.x, top - 0.45, pz - 0.16),
            v3(t1.x + 0.2, top - 0.2, pz + 0.16),
            0.01,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    steel.append(
        &pipe(
            &[
                v3(px, top - 0.45, pz),
                v3(px, base + 0.35, pz),
                v3(px, base + 0.16, pz + 0.08),
                v3(px, base + 0.12, pz + 0.3),
            ],
            0.055,
            12,
        ),
        Mat4::IDENTITY,
    );
    let mut y = base + 1.0;
    while y < top - 0.6 {
        dark.append(
            &bx(v3(t1.x, y, pz - 0.075), v3(px + 0.07, y + 0.05, pz + 0.075)),
            Mat4::IDENTITY,
        );
        y += 1.6;
    }
    // A cable from the floodlight runs along the front face's foot to a junction box.
    let mut cable = M::new();
    let flood = Vec3::from(FLOOD_HEAD);
    cable.append(
        &pipe(
            &[
                v3(flood.x + 0.06, base + 0.3, flood.z + 0.06),
                v3(flood.x + 0.32, base + 0.06, flood.z + 0.3),
                v3(t0.x - 0.06, base + 0.06, t0.z - 1.0),
                v3(t0.x - 0.04, base + 0.06, t0.z - 0.06),
                v3(t0.x + 0.6, base + 0.06, t0.z - 0.04),
                v3(t0.x + 0.6, base + 0.5, t0.z - 0.04),
            ],
            0.014,
            6,
        ),
        Mat4::IDENTITY,
    );
    kit.add(CABLE, cable);
    steel.append(
        &slab(
            v3(t0.x + 0.3, base + 0.5, t0.z - 0.16),
            v3(t0.x + 0.95, base + 1.3, t0.z),
            0.012,
            0.3,
        ),
        Mat4::IDENTITY,
    );
    for k in 0..5 {
        let y = base + 0.9 + k as f32 * 0.05;
        dark.append(
            &bx(
                v3(t0.x + 0.4, y, t0.z - 0.166),
                v3(t0.x + 0.85, y + 0.022, t0.z - 0.158),
            ),
            Mat4::IDENTITY,
        );
    }

    // Station number and name on the front (seen from the course) and the east side.
    let mut paint = M::new();
    let orange_paint = paint_color(ORANGE_PAINT);
    let white_paint = paint_color(WHITE_PAINT);
    let fz = t0.z - 0.004;
    stencil(
        &mut paint,
        10,
        v3(t0.x + 1.9, 3.2, fz),
        Vec3::NEG_X,
        Vec3::Y,
        1.6,
        orange_paint,
    );
    stencil(
        &mut paint,
        11,
        v3(t0.x + 1.9, 1.95, fz),
        Vec3::NEG_X,
        Vec3::Y,
        0.3,
        white_paint,
    );
    stencil(
        &mut paint,
        10,
        v3(t1.x + 0.004, 3.2, t1.z - 1.9),
        Vec3::NEG_Z,
        Vec3::Y,
        1.6,
        orange_paint,
    );
    // Cyan guide lights in the front face's two upper panel joints frame the way up, and
    // chevrons between them point at the target.
    let mut lights = M::new();
    let w = (t1.x - t0.x - 2.0 * inset) / 3.0;
    for jx in [t0.x + inset + w, t1.x - inset - w] {
        let (y0, y1) = (LIFT + 0.25, top - 0.2);
        alu.append(
            &bx(
                v3(jx - 0.026, y0 - 0.04, t0.z - 0.012),
                v3(jx + 0.026, y1 + 0.04, t0.z),
            ),
            Mat4::IDENTITY,
        );
        lights.append(
            &bx(
                v3(jx - 0.013, y0, t0.z - 0.016),
                v3(jx + 0.013, y1, t0.z - 0.012),
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(GLOW_CYAN, lights);
    // Chevrons up the front face to the target.
    for (y, c) in [(6.1, YELLOW_PAINT), (7.4, ORANGE_PAINT)] {
        let c = paint_color(c);
        stencil(
            &mut paint,
            9,
            v3(target.x, y, fz),
            Vec3::Y,
            Vec3::X,
            0.34,
            c,
        );
    }
    kit.add(PAINT, paint);
    kit.add(STEEL_DARK, dark);
    kit.add(STEEL_YELLOW, yellow);
    kit.add(STEEL, steel);
    kit.add(STEEL_ORANGE, orange);
    kit.add(ALU, alu);
    kit.add(GLOW_WHITE, white);
}

/// Where the railing on the tower's +X side opens for the zip line (z from, to): 2 m round
/// where the line crosses that edge.
fn zip_gap(t1: Vec3) -> (f32, f32) {
    let (along, _) = layout::zip_axes();
    let from = DVec3::from_array(layout::ZIP_FROM);
    let z = from.z + along[2] * (t1.x as f64 - from.x) / along[0];
    (z as f32 - 1.0, z as f32 + 1.0)
}

/// A guard rail along `path` (on the roof, just outside its edge): posts bracketed to the
/// faces, two rails bent round the corners and a toe board.
fn railing(yellow: &mut M, dark: &mut M, path: &[Vec3], t0: Vec3, t1: Vec3) {
    let top = t1.y;
    let center = (t0 + t1) * 0.5;
    let mut posts = vec![path[0]];
    for w in path.windows(2) {
        let n = (w[0].distance(w[1]) / 1.5).ceil().max(1.0) as usize;
        posts.extend((1..=n).map(|k| w[0].lerp(w[1], k as f32 / n as f32)));
    }
    for &p in &posts {
        yellow.append(
            &bx(
                v3(p.x - 0.025, top - 0.35, p.z - 0.025),
                v3(p.x + 0.025, top + RAIL_Y[1], p.z + 0.025),
            ),
            Mat4::IDENTITY,
        );
        // Brackets back to the nearest face.
        let foot = v3(p.x.clamp(t0.x, t1.x), p.y, p.z.clamp(t0.z, t1.z));
        let across = Vec3::Y.cross((foot - p).normalize_or(Vec3::X)).abs();
        for y in [-0.25, -0.08] {
            let (a, b) = (p + Vec3::Y * y, foot + Vec3::Y * y);
            let pad = across * 0.04 + Vec3::Y * 0.04;
            dark.append(&bx(a.min(b) - pad, a.max(b) + pad), Mat4::IDENTITY);
        }
    }
    // Rails, with the corners eased so the tube doesn't pinch.
    let mut eased = vec![path[0]];
    for i in 1..path.len() - 1 {
        let (a, b, c) = (path[i - 1], path[i], path[i + 1]);
        eased.push(b + (a - b).normalize() * 0.09);
        eased.push(b + (c - b).normalize() * 0.09);
    }
    eased.push(path[path.len() - 1]);
    for (k, &dy) in RAIL_Y.iter().enumerate() {
        let r = if k == 1 { 0.026 } else { 0.019 };
        let line: Vec<Vec3> = eased.iter().map(|&p| p + Vec3::Y * dy).collect();
        yellow.append(&pipe(&line, r, 10), Mat4::IDENTITY);
    }
    // Toe boards on the roof, just inside the posts.
    for w in path.windows(2) {
        let along = (w[1] - w[0]).normalize();
        let mut n = Vec3::Y.cross(along);
        if n.dot(center - w[0]) < 0.0 {
            n = -n;
        }
        let (a, b) = (w[0] + n * 0.06, w[1] + n * 0.06);
        let th = n.abs() * 0.004;
        yellow.append(
            &bx(a.min(b) - th, a.max(b) + th + Vec3::Y * 0.1),
            Mat4::IDENTITY,
        );
    }
}

/// Roof gear out of the crossing lane, and hazard strips on the edges pilots come over and
/// drop off.
fn roof(kit: &mut Kit, t0: Vec3, t1: Vec3) {
    let top = t1.y;
    let mut steel = M::new();
    let mut dark = M::new();
    steel.append(
        &slab(
            v3(t0.x + 0.6, top, t1.z - 1.5),
            v3(t0.x + 1.6, top + 0.22, t1.z - 0.6),
            0.02,
            0.5,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &bx(
            v3(t0.x + 0.58, top + 0.22, t1.z - 1.52),
            v3(t0.x + 1.62, top + 0.25, t1.z - 0.58),
        ),
        Mat4::IDENTITY,
    );
    steel.append(
        &pipe(
            &[
                v3(t0.x + 1.0, top, t1.z - 2.6),
                v3(t0.x + 1.0, top + 0.7, t1.z - 2.6),
                v3(t0.x + 1.0, top + 0.86, t1.z - 2.48),
                v3(t0.x + 1.0, top + 0.82, t1.z - 2.3),
            ],
            0.09,
            14,
        ),
        Mat4::IDENTITY,
    );
    // An air-handling box on rubber feet (on the −X side: the zip line's gantry stands on the
    // +X side).
    let (ax, az) = (t0.x + 1.6, t0.z + 1.7);
    steel.append(
        &slab(
            v3(ax - 0.55, top + 0.08, az - 0.4),
            v3(ax + 0.55, top + 0.78, az + 0.4),
            0.02,
            0.4,
        ),
        Mat4::IDENTITY,
    );
    for k in 0..7 {
        let x = ax - 0.42 + k as f32 * 0.14;
        dark.append(
            &bx(
                v3(x, top + 0.2, az - 0.405),
                v3(x + 0.06, top + 0.68, az - 0.398),
            ),
            Mat4::IDENTITY,
        );
    }
    for (dx, dz) in [(-0.45, -0.3), (0.45, -0.3), (-0.45, 0.3), (0.45, 0.3)] {
        dark.append(
            &bx(
                v3(ax + dx - 0.06, top, az + dz - 0.06),
                v3(ax + dx + 0.06, top + 0.08, az + dz + 0.06),
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(STEEL, steel);
    kit.add(STEEL_DARK, dark);
    let mut hz = M::new();
    for (z0, z1) in [(t0.z, t0.z + 0.3), (t1.z - 0.3, t1.z)] {
        hz.append(
            &slab(v3(t0.x, top, z0), v3(t1.x, top + 0.002, z1), 0.0, 100.0),
            Mat4::IDENTITY,
        );
    }
    // And on the +X edge where the zip line leaves.
    let (gap0, gap1) = zip_gap(t1);
    hz.append(
        &slab(
            v3(t1.x - 0.3, top, gap0),
            v3(t1.x, top + 0.002, gap1),
            0.0,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    kit.add(HAZARD, hz);
    // Chevrons in the middle of the roof point the way on: down the zip line.
    let (along, across) = layout::zip_axes();
    let (along, across) = (
        DVec3::from_array(along).as_vec3(),
        DVec3::from_array(across).as_vec3(),
    );
    let mut paint = M::new();
    stencil(
        &mut paint,
        9,
        v3((t0.x + t1.x) * 0.5, top + 0.003, (t0.z + t1.z) * 0.5),
        along,
        -across,
        0.4,
        paint_color(WHITE_PAINT),
    );
    kit.add(PAINT, paint);
}

/// The anchor plate the hook bites: a bolted steel plate in a hazard frame, a boss with a
/// glowing ring and a dark socket at the aim point.
fn target(kit: &mut Kit, t0: Vec3) {
    let c = DVec3::from_array(layout::GRAPPLE_TARGET).as_vec3();
    let face = t0.z;
    let out = Quat::from_rotation_x(-FRAC_PI_2); // +Y → −Z, out of the face
    let (hw, hh) = (0.38, 0.23);
    let mut dark = M::new();
    let mut steel = M::new();
    let mut glow = M::new();
    let mut hz = M::new();
    dark.append(
        &slab(
            v3(c.x - hw, c.y - hh, face - 0.025),
            v3(c.x + hw, c.y + hh, face),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    let corners: Vec<Vec3> = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .iter()
        .map(|(sx, sy)| v3(c.x + sx * (hw - 0.06), c.y + sy * (hh - 0.06), face - 0.025))
        .collect();
    bolts(&mut steel, &corners, Vec3::NEG_Z, 0.016);
    // A hazard frame painted round the plate (clear of the nosing above).
    let b = 0.04;
    for (min, max) in [
        (v3(-hw - b, -hh - b, 0.0), v3(hw + b, -hh, 0.0)),
        (v3(-hw - b, hh, 0.0), v3(hw + b, hh + b, 0.0)),
        (v3(-hw - b, -hh, 0.0), v3(-hw, hh, 0.0)),
        (v3(hw, -hh, 0.0), v3(hw + b, hh, 0.0)),
    ] {
        let lift = v3(c.x, c.y, face - 0.003);
        hz.append(
            &slab(min + lift, max + lift + Vec3::Z * 0.003, 0.0, 100.0),
            Mat4::IDENTITY,
        );
    }
    // The boss, its glowing ring, the socket.
    let at_face = |dz: f32| Mat4::from_rotation_translation(out, v3(c.x, c.y, face - dz));
    steel.append(
        &M::lathe(
            &[
                Vec2::new(0.19, 0.0),
                Vec2::new(0.19, 0.03),
                Vec2::new(0.175, 0.045),
                Vec2::new(0.0001, 0.045),
            ],
            32,
        ),
        at_face(0.025),
    );
    glow.append(
        &M::lathe(
            &[
                Vec2::new(0.11, 0.0),
                Vec2::new(0.11, 0.051),
                Vec2::new(0.145, 0.051),
                Vec2::new(0.145, 0.0),
            ],
            32,
        ),
        at_face(0.025),
    );
    dark.append(
        &M::lathe(
            &[
                Vec2::new(0.055, 0.0),
                Vec2::new(0.055, 0.05),
                Vec2::new(0.044, 0.055),
                Vec2::new(0.0001, 0.055),
            ],
            20,
        ),
        at_face(0.025),
    );
    kit.add(STEEL_DARK, dark);
    kit.add(STEEL, steel);
    kit.add(GLOW_CYAN, glow);
    kit.add(HAZARD, hz);
}

/// A floodlight on a pole at the pad's front corner, aimed up at the target.
fn floodlight(kit: &mut Kit, base: f32) {
    let head = Vec3::from(FLOOD_HEAD);
    let (px, pz) = (head.x, head.z);
    let foot = base + 0.27;
    let pole_top = head.y - 0.42;
    let mut steel = M::new();
    let mut dark = M::new();
    let mut white = M::new();
    let mut red = M::new();
    let mut concrete = M::new();
    concrete.append(
        &slab(
            v3(px - 0.3, base, pz - 0.3),
            v3(px + 0.3, base + 0.25, pz + 0.3),
            0.025,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    steel.append(
        &slab(
            v3(px - 0.17, base + 0.25, pz - 0.17),
            v3(px + 0.17, foot, pz + 0.17),
            0.005,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    let anchors: Vec<Vec3> = [(-0.12, -0.12), (0.12, -0.12), (-0.12, 0.12), (0.12, 0.12)]
        .iter()
        .map(|(a, b)| v3(px + a, foot, pz + b))
        .collect();
    bolts(&mut dark, &anchors, Vec3::Y, 0.014);
    steel.append(
        &M::lathe(
            &[
                Vec2::new(0.07, 0.0),
                Vec2::new(0.06, pole_top - foot - 0.1),
                Vec2::new(0.05, pole_top - foot),
            ],
            14,
        ),
        at(px, foot, pz),
    );
    // The head looks along its local −Z, at the target, held in a yoke.
    let target = DVec3::from_array(layout::GRAPPLE_TARGET).as_vec3();
    let dir = (target - head).normalize();
    let aim = Quat::from_rotation_y((-dir.x).atan2(-dir.z)) * Quat::from_rotation_x(dir.y.asin());
    dark.append(&rbox(head, v3(0.5, 0.32, 0.14), aim, 0.014), Mat4::IDENTITY);
    for k in 0..6 {
        let off = aim * v3(-0.2 + k as f32 * 0.08, 0.0, 0.09);
        dark.append(
            &rbox(head + off, v3(0.012, 0.28, 0.05), aim, 0.0),
            Mat4::IDENTITY,
        );
    }
    white.append(
        &rbox(
            head + aim * v3(0.0, 0.0, -0.072),
            v3(0.44, 0.26, 0.006),
            aim,
            0.0,
        ),
        Mat4::IDENTITY,
    );
    let ears = [-1.0f32, 1.0].map(|side| head + aim * v3(side * 0.275, 0.0, 0.0));
    let low = pole_top + 0.08;
    dark.append(
        &pipe(
            &[
                ears[0],
                v3(ears[0].x, low, ears[0].z),
                v3(ears[1].x, low, ears[1].z),
                ears[1],
            ],
            0.016,
            8,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &rod(v3(px, pole_top - 0.02, pz), v3(px, low, pz), 0.03, 10),
        Mat4::IDENTITY,
    );
    red.append(
        &M::lathe(
            &[
                Vec2::new(0.04, 0.0),
                Vec2::new(0.04, 0.03),
                Vec2::new(0.027, 0.065),
                Vec2::new(0.0001, 0.075),
            ],
            12,
        ),
        at(head.x, head.y + 0.16, head.z),
    );
    kit.add(CONCRETE, concrete);
    kit.add(STEEL, steel);
    kit.add(STEEL_DARK, dark);
    kit.add(GLOW_WHITE, white);
    kit.add(GLOW_RED, red);
}
