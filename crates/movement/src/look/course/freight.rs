//! The freight station: standard-gauge track on crushed-stone ballast between two buffer stops,
//! a precast loading platform with granite coping beside it, and lamp posts behind it. A box
//! car ([`build_car`]) shuttles along the track: an oxide-red covered wagon with a sliding
//! door, pressed steel ends, a ribbed roof, ladders, buffers and screw couplings, on two
//! bogies with coil springs. The platform's top and the car's roof and sides are drawn on
//! their collider faces; the car's ladders and grab handles stand a few centimeters proud.

use super::*;

/// Top of the rail heads (relative to the demo origin).
const RAIL_TOP: f32 = 0.475;
/// Distance from the track's center line to each rail's.
const RAIL_C: f32 = 0.7535;
/// Wheel tread radius.
const WHEEL_R: f32 = 0.42;
/// Where the track ends (x).
const TRACK_X: [f32; 2] = [-29.9, 2.4];
/// The buffer stops' head faces (x): a little past the car's buffers at each end of its run.
const STOP_X: [f32; 2] = [-27.05, -0.45];
/// Buffer centers: height above the rail tops and distance from the track's center line.
const BUFFER_H: f32 = 1.06;
const BUFFER_Z: f32 = 0.875;
/// How far the car's buffer heads reach past its collider's ends.
const BUFFER_REACH: f32 = 0.62;
/// Lamp posts behind the platform (x); [`COURSE_LIGHTS`] has a light under each head.
const LAMP_X: [f32; 3] = [-22.0, -14.0, -6.0];
/// The lamps' heads: height and how far over the platform they reach (z).
const LAMP_HEAD: (f32, f32) = (6.1, 25.6);
/// Bogie centers along the car (x).
const BOGIE_X: [f32; 2] = [-4.2, 4.2];

/// The track, platform and lamps: (group, material, casts shadows), spawned at the demo origin.
pub(super) const TRACK_SET: &[(&str, &str, bool)] = &[
    ("freight", BALLAST, false),
    ("freight", CONCRETE, true),
    ("freight", PANEL, true),
    ("freight", STONE, true),
    ("freight", RUST, true),
    ("freight", ALU, true),
    ("freight", STEEL, true),
    ("freight", STEEL_DARK, true),
    ("freight", STEEL_YELLOW, true),
    ("freight", STEEL_OLIVE, true),
    ("freight", HAZARD, true),
    ("freight", RUBBER, true),
    ("freight", GLOW_WHITE, false),
    ("freight", GLOW_RED, false),
    ("freight", PAINT, false),
];

/// The car: spawned as children of the moving car body (see [`dress_freight_car`]).
pub(super) const CAR_SET: &[(&str, &str, bool)] = &[
    ("freight_car", OXIDE, true),
    ("freight_car", ROOF_GREY, true),
    ("freight_car", STEEL_DARK, true),
    ("freight_car", RUST, true),
    ("freight_car", ALU, true),
    ("freight_car", STEEL, true),
    ("freight_car", STEEL_YELLOW, true),
    ("freight_car", RUBBER, true),
    ("freight_car", PAINT, false),
];

// ---------------------------------------------------------------------------- helpers

/// A plain box with no bevel (small parts: 12 triangles).
fn cube(min: Vec3, max: Vec3) -> M {
    slab(min, max, 0.0, 100.0)
}

/// Spins a profile of (radius, height) points around +Y in `segments` steps, with a hard edge
/// at every profile point. Each profile segment faces to its right (go up the outside, then in
/// across the top). UVs in meters: U around, V along the profile.
fn revolve(profile: &[Vec2], segments: u32) -> M {
    let mut m = M::new();
    let mut v = 0.0;
    for w in profile.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = b - a;
        let len = d.length();
        if len < 1e-6 {
            continue;
        }
        let n2 = Vec2::new(d.y, -d.x) / len;
        let base = m.vertices.len() as u32;
        for k in 0..=segments {
            let t = k as f32 / segments as f32 * TAU;
            let dir = v3(t.cos(), 0.0, t.sin());
            let normal = dir * n2.x + Vec3::Y * n2.y;
            m.vertex(dir * a.x + Vec3::Y * a.y, normal, Vec2::new(t * a.x, v));
            m.vertex(
                dir * b.x + Vec3::Y * b.y,
                normal,
                Vec2::new(t * b.x, v + len),
            );
        }
        for k in 0..segments {
            let i = base + 2 * k;
            m.quad(i, i + 1, i + 3, i + 2);
        }
        v += len;
    }
    m.remove_degenerate();
    m
}

/// A closed cylinder of `radius` from `a` to `b`.
fn cylinder(a: Vec3, b: Vec3, radius: f32, segments: u32) -> M {
    let len = a.distance(b);
    let c = revolve(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(radius, 0.0),
            Vec2::new(radius, len),
            Vec2::new(0.0, len),
        ],
        segments,
    );
    c.transformed(rt(Quat::from_rotation_arc(Vec3::Y, (b - a).normalize()), a))
}

/// A coil spring standing on `foot`, `height` tall.
fn spring(foot: Vec3, height: f32, radius: f32, wire: f32, turns: f32) -> M {
    let n = (turns * 10.0).ceil() as usize;
    let path: Vec<Vec3> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let a = t * turns * TAU;
            let y = wire + (height - 2.0 * wire) * t;
            foot + v3(a.cos() * radius, y, a.sin() * radius)
        })
        .collect();
    pipe(&path, wire, 5)
}

/// A side buffer from its mounting face (x = 0) to its head (x = `len`), at y = z = 0:
/// (dark flange and casing, bright plunger and head).
fn buffer(len: f32, head_r: f32) -> (M, M) {
    let mut dark = M::new();
    let mut bright = M::new();
    dark.append(
        &slab(v3(0.0, -0.17, -0.17), v3(0.035, 0.17, 0.17), 0.01, 100.0),
        Mat4::IDENTITY,
    );
    bolts(
        &mut dark,
        &[
            v3(0.035, -0.13, -0.13),
            v3(0.035, -0.13, 0.13),
            v3(0.035, 0.13, -0.13),
            v3(0.035, 0.13, 0.13),
        ],
        Vec3::X,
        0.014,
    );
    let to_x = Quat::from_rotation_z(-FRAC_PI_2);
    let casing = len * 0.55;
    dark.append(
        &revolve(
            &[
                Vec2::new(0.0, 0.035),
                Vec2::new(0.125, 0.035),
                Vec2::new(0.125, 0.07),
                Vec2::new(0.112, 0.085),
                Vec2::new(0.112, casing - 0.025),
                Vec2::new(0.122, casing - 0.02),
                Vec2::new(0.122, casing),
                Vec2::new(0.0, casing),
            ],
            20,
        ),
        rt(to_x, Vec3::ZERO),
    );
    bright.append(
        &revolve(
            &[
                Vec2::new(0.0, casing),
                Vec2::new(0.092, casing),
                Vec2::new(0.092, len - 0.045),
                Vec2::new(head_r - 0.01, len - 0.045),
                Vec2::new(head_r, len - 0.035),
                Vec2::new(head_r, len - 0.014),
                Vec2::new(head_r - 0.03, len - 0.003),
                Vec2::new(0.0, len),
            ],
            24,
        ),
        rt(to_x, Vec3::ZERO),
    );
    (dark, bright)
}

/// Bays from `a` to `b` about `bay` meters long: the joints.
fn joints(a: f32, b: f32, bay: f32) -> Vec<f32> {
    let n = ((b - a) / bay).round().max(1.0) as usize;
    (0..=n).map(|i| a + (b - a) * i as f32 / n as f32).collect()
}

// ---------------------------------------------------------------------------- the track

pub(super) fn build_track() -> Kit {
    let tz = layout::FREIGHT_TRACK_Z as f32;
    let mut kit = Kit::default();
    bed(&mut kit, tz);
    track(&mut kit, tz);
    for (face, dir) in [(STOP_X[0], 1.0), (STOP_X[1], -1.0)] {
        buffer_stop(&mut kit, face, dir, tz);
    }
    platform(&mut kit);
    lamps(&mut kit);
    kit
}

/// The ballast bed: the crib between the rails a little below the sleeper tops, flat
/// shoulders sloping into the ground, lumpy, stained with rust along the rails and darker
/// where it is damp at the toe.
fn bed(kit: &mut Kit, tz: f32) {
    let [x0, x1] = TRACK_X;
    let height = |x: f32, s: f32| -> f32 {
        let a = s.abs();
        let crib = 0.3 - 0.035 * smoothstep(1.45, 1.25, a);
        let shoulder = 0.3 - (a - 1.9).max(0.0) / 1.5;
        let end = (x0 + 0.7 - x).max(x - x1 + 0.7).max(0.0);
        let h = crib.min(shoulder).min(0.3 - end / 1.5);
        let lumps = 0.014 * pn(x * 2.7, s * 2.7, 61) + 0.007 * pn(x * 7.3, s * 7.3, 62);
        (h + lumps * smoothstep(-0.04, 0.1, h)).max(-0.04)
    };
    let (nx, ns) = (((x1 - x0) / 0.2).ceil() as usize, 52usize);
    let (s0, s1) = (-2.6f32, 2.6f32);
    let mut m = M::new();
    for j in 0..=ns {
        let s = s0 + (s1 - s0) * j as f32 / ns as f32;
        for i in 0..=nx {
            let x = x0 + (x1 - x0) * i as f32 / nx as f32;
            let y = height(x, s);
            let e = 0.05;
            let n = v3(
                height(x - e, s) - height(x + e, s),
                2.0 * e,
                height(x, s - e) - height(x, s + e),
            );
            let k = m.vertex(v3(x, y, tz + s), n, Vec2::new(x, tz + s));
            // Brake dust and rust along the rails, oil between them, damp at the toe.
            let near_rail = (1.0 - ((s.abs() - RAIL_C).abs() / 0.35).min(1.0)).powi(2);
            let rust = Vec3::ONE.lerp(v3(0.8, 0.62, 0.5), 0.75 * near_rail);
            let oil = 1.0 - 0.14 * smoothstep(0.6, 0.15, s.abs());
            let damp = 0.82 + 0.18 * smoothstep(-0.02, 0.12, y);
            let blot = 0.92 + 0.1 * pn(x * 0.35, s * 0.6, 63);
            let c = rust * (oil * damp * blot);
            m.vertices[k as usize].color = [c.x, c.y, c.z, 1.0];
        }
    }
    let row = nx as u32 + 1;
    for j in 0..ns as u32 {
        for i in 0..nx as u32 {
            let a = j * row + i;
            m.quad(a, a + row, a + row + 1, a + 1);
        }
    }
    kit.add(BALLAST, m);
}

/// Concrete sleepers every 0.6 m with rail pads, cast shoulders and spring clips, and two
/// rusty rails with a bright running band on their heads, joined once by fishplates.
fn track(kit: &mut Kit, tz: f32) {
    let [x0, x1] = TRACK_X;
    let mut concrete = M::new();
    let mut rust = M::new();
    let mut dark = M::new();
    let mut bright = M::new();

    // One sleeper with its fastenings, centered on x = 0.
    let mut sleeper = M::new();
    for side in [-1.0f32, 1.0] {
        sleeper.append(
            &slab(
                v3(-0.13, 0.12, side * 0.36),
                v3(0.13, 0.3, side * 1.3),
                0.018,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    sleeper.append(
        &slab(v3(-0.11, 0.12, -0.36), v3(0.11, 0.272, 0.36), 0.015, 100.0),
        Mat4::IDENTITY,
    );
    // Weathered grey-brown, darker below and stained by the rail seats.
    sleeper.tint(Vec4::new(0.74, 0.7, 0.64, 1.0));
    sleeper.shade(|p, _| {
        let seat = (-(((p.z.abs() - RAIL_C) / 0.2).powi(2))).exp();
        (0.65 + 0.35 * smoothstep(0.12, 0.3, p.y)) * (1.0 - 0.25 * seat)
    });
    let mut pads = M::new();
    let mut shoulders = M::new();
    let mut clips = M::new();
    for rail in [-RAIL_C, RAIL_C] {
        pads.append(
            &cube(v3(-0.1, 0.3, rail - 0.085), v3(0.1, 0.315, rail + 0.085)),
            Mat4::IDENTITY,
        );
        for side in [-1.0f32, 1.0] {
            let z = rail + side * 0.098;
            shoulders.append(
                &cube(v3(-0.05, 0.29, z - 0.02), v3(0.05, 0.345, z + 0.02)),
                Mat4::IDENTITY,
            );
            // The clip's loop over its shoulder and its toe pressing on the rail foot.
            let toe = rail + side * 0.06;
            let path: Vec<Vec3> = (0..=4)
                .map(|k| {
                    let a = k as f32 / 4.0 * PI;
                    v3(-0.042 * a.cos(), 0.335 + 0.024 * a.sin(), z + side * 0.006)
                })
                .collect();
            clips.append(&pipe(&path, 0.0075, 5), Mat4::IDENTITY);
            clips.append(
                &pipe(
                    &[
                        v3(0.0, 0.359, z + side * 0.006),
                        v3(0.0, 0.352, (z + toe) * 0.5),
                        v3(0.0, 0.334, toe),
                    ],
                    0.0075,
                    5,
                ),
                Mat4::IDENTITY,
            );
        }
    }
    let n = ((x1 - x0 - 1.4) / 0.6).floor() as usize;
    let first = (x0 + x1) * 0.5 - n as f32 * 0.3;
    for i in 0..=n {
        let x = first + i as f32 * 0.6;
        // A slight wander, as laid by hand.
        let wobble = 0.012 * pn(x * 3.1, 0.5, 64);
        let to = rt(
            Quat::from_rotation_y(0.01 * pn(x * 2.3, 1.5, 65)),
            v3(x + wobble, 0.0, tz),
        );
        concrete.append(&sleeper, to);
        dark.append(&pads, to);
        rust.append(&shoulders, to);
        dark.append(&clips, to);
    }

    // Rails: foot, web and head, a running band where the wheels polish them.
    let (ra, rb) = (x0 + 0.25, x1 - 0.25);
    for rail in [-RAIL_C, RAIL_C] {
        let z = tz + rail;
        rust.append(
            &slab(
                v3(ra, 0.315, z - 0.07),
                v3(rb, 0.33, z + 0.07),
                0.004,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        rust.append(
            &slab(
                v3(ra, 0.33, z - 0.0085),
                v3(rb, 0.44, z + 0.0085),
                0.002,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        rust.append(
            &slab(
                v3(ra, 0.438, z - 0.036),
                v3(rb, RAIL_TOP, z + 0.036),
                0.008,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        bright.append(
            &cube(
                v3(ra + 0.02, RAIL_TOP - 0.001, z - 0.022),
                v3(rb - 0.02, RAIL_TOP + 0.0008, z + 0.022),
            ),
            Mat4::IDENTITY,
        );
        // A joint with fishplates and bolts.
        let jx = -13.7;
        for side in [-1.0f32, 1.0] {
            let face = z + side * 0.03;
            dark.append(
                &slab(
                    v3(jx - 0.4, 0.345, z + side * 0.0085),
                    v3(jx + 0.4, 0.43, face),
                    0.004,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
            let pts: Vec<Vec3> = [-0.3f32, -0.1, 0.1, 0.3]
                .iter()
                .map(|&dx| v3(jx + dx, 0.3875, face))
                .collect();
            bolts(&mut dark, &pts, Vec3::Z * side, 0.012);
        }
    }
    rust.shade(|p, _| 0.8 + 0.2 * smoothstep(0.3, 0.47, p.y));
    kit.add(CONCRETE, concrete);
    kit.add(RUST, rust);
    kit.add(STEEL_DARK, dark);
    kit.add(ALU, bright);
}

/// A buffer stop at the end of the track: a hazard-faced beam carrying two buffers at the car's
/// buffer height, on legs and raking struts clamped to the rails, with a red lamp on top.
/// `face` is the buffer heads' x and `dir` the way they face (±1).
fn buffer_stop(kit: &mut Kit, face: f32, dir: f32, tz: f32) {
    let y = RAIL_TOP + BUFFER_H;
    let reach = 0.5;
    let front = face - dir * reach;
    let back = front - dir * 0.3;
    let (xa, xb) = (front.min(back), front.max(back));
    let mut yellow = M::new();
    let mut hazard = M::new();
    let mut dark = M::new();
    let mut rust = M::new();
    let mut bright = M::new();
    let mut red = M::new();
    yellow.append(
        &slab(
            v3(xa, y - 0.27, tz - 1.25),
            v3(xb, y + 0.27, tz + 1.25),
            0.02,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // The striped face, between the buffers' flanges.
    let skin = front + dir * 0.004;
    hazard.append(
        &cube(
            v3(front.min(skin), y - 0.24, tz - 1.2),
            v3(front.max(skin), y + 0.24, tz + 1.2),
        ),
        Mat4::IDENTITY,
    );
    let turn = if dir > 0.0 {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_y(PI)
    };
    let (b_dark, b_bright) = buffer(reach, 0.23);
    for s in [-BUFFER_Z, BUFFER_Z] {
        let to = rt(turn, v3(front + dir * 0.004, y, tz + s));
        dark.append(&b_dark, to);
        bright.append(&b_bright, to);
    }
    for s in [-RAIL_C, RAIL_C] {
        let z = tz + s;
        // Front legs down to the rail, struts raking back to it.
        dark.append(
            &slab(
                v3(xa + 0.04, RAIL_TOP + 0.03, z - 0.07),
                v3(xb - 0.04, y - 0.26, z + 0.07),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let a = v3(back, y - 0.12, z);
        let b = v3(back - dir * 1.5, RAIL_TOP + 0.06, z);
        let d = b - a;
        dark.append(
            &rbox(
                (a + b) * 0.5,
                v3(d.length(), 0.16, 0.12),
                Quat::from_rotation_arc(Vec3::X, d.normalize()),
                0.012,
            ),
            Mat4::IDENTITY,
        );
        // Clamps around the rail head at both feet.
        for x in [(xa + xb) * 0.5, b.x + dir * 0.05] {
            rust.append(
                &slab(
                    v3(x - 0.16, 0.33, z - 0.11),
                    v3(x + 0.16, RAIL_TOP + 0.05, z + 0.11),
                    0.01,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
            bolts(
                &mut rust,
                &[
                    v3(x - 0.1, RAIL_TOP + 0.05, z - 0.07),
                    v3(x + 0.1, RAIL_TOP + 0.05, z - 0.07),
                    v3(x - 0.1, RAIL_TOP + 0.05, z + 0.07),
                    v3(x + 0.1, RAIL_TOP + 0.05, z + 0.07),
                ],
                Vec3::Y,
                0.014,
            );
        }
    }
    // Cross tie between the struts' feet.
    let tx = back - dir * 1.4;
    dark.append(
        &cube(
            v3(tx - 0.06, RAIL_TOP + 0.04, tz - RAIL_C),
            v3(tx + 0.06, RAIL_TOP + 0.16, tz + RAIL_C),
        ),
        Mat4::IDENTITY,
    );
    // The lamp: a housing on the beam with a red lens toward the track.
    let lx = (xa + xb) * 0.5;
    dark.append(
        &slab(
            v3(lx - 0.1, y + 0.27, tz - 0.1),
            v3(lx + 0.1, y + 0.5, tz + 0.1),
            0.015,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    red.append(
        &cylinder(
            v3(lx + dir * 0.1, y + 0.385, tz),
            v3(lx + dir * 0.125, y + 0.385, tz),
            0.07,
            16,
        ),
        Mat4::IDENTITY,
    );
    yellow.shade(|p, _| 0.8 + 0.2 * smoothstep(y - 0.3, y + 0.3, p.y));
    kit.add(STEEL_YELLOW, yellow);
    kit.add(HAZARD, hazard);
    kit.add(STEEL_DARK, dark);
    kit.add(RUST, rust);
    kit.add(ALU, bright);
    kit.add(GLOW_RED, red);
}

/// The loading platform: precast deck slabs in 3 m bays behind granite coping, walls of precast
/// panels, rubber dock bumpers facing the track, paint (the station number, a yellow line, MIND
/// THE GAP), and a stack of cases at its back edge.
fn platform(kit: &mut Kit) {
    let (p0, p1) = bounds(layout::FREIGHT_PLATFORM);
    let top = p1.y;
    let edge = p1.z;
    let cope = edge - 0.6;
    let g = 0.003;
    let gap = |v: f32, end: f32| if (v - end).abs() < 1e-3 { 0.0 } else { g };
    let bays = joints(p0.x, p1.x, 3.0);
    let mid = (p0.x + p1.x) * 0.5;

    let mut concrete = M::new();
    for w in bays.windows(2) {
        let (a, b) = (w[0] + gap(w[0], p0.x), w[1] - gap(w[1], p1.x));
        concrete.append(
            &slab(v3(a, top - 0.22, p0.z), v3(b, top, cope - g), 0.008, 0.6),
            Mat4::IDENTITY,
        );
    }
    // Worn where people walk, darker at the back edge.
    concrete.shade(|p, _| {
        let lane = (-((p.z - (cope - 0.9)) / 0.8).powi(2)).exp();
        let back = smoothstep(p0.z, p0.z + 0.5, p.z);
        let blot = 0.9 + 0.1 * pn(p.x * 0.3, p.z * 0.3, 66);
        blot * (0.86 + 0.14 * back) * (1.0 - 0.08 * lane)
    });
    kit.add(CONCRETE, concrete);

    let mut stone = M::new();
    let mut x = p0.x;
    while x < p1.x - 1e-3 {
        let x2 = (x + 1.0).min(p1.x);
        stone.append(
            &slab(
                v3(x + gap(x, p0.x), top - 0.15, cope),
                v3(x2 - gap(x2, p1.x), top, edge + 0.04),
                0.012,
                0.5,
            ),
            Mat4::IDENTITY,
        );
        x = x2;
    }
    stone.shade(|p, n| {
        let nose = if n.z > 0.5 { 0.85 } else { 1.0 };
        nose * (0.9 + 0.1 * pn(p.x * 0.8, p.z, 67))
    });
    kit.add(STONE, stone);

    // Walls: the face toward the track recessed under the coping's nose, the back, the ends.
    let mut panels = M::new();
    for w in bays.windows(2) {
        let (a, b) = (w[0] + gap(w[0], p0.x), w[1] - gap(w[1], p1.x));
        let mut front = slab(
            v3(a, -0.05, edge - 0.25),
            v3(b, top - 0.15, edge - 0.02),
            0.006,
            0.5,
        );
        panel_uvs(&mut front, Vec3::X, a, b - a);
        panels.append(&front, Mat4::IDENTITY);
        let mut back = slab(
            v3(a, -0.05, p0.z),
            v3(b, top - 0.22, p0.z + 0.25),
            0.006,
            0.5,
        );
        panel_uvs(&mut back, Vec3::X, a, b - a);
        panels.append(&back, Mat4::IDENTITY);
    }
    for (xa, xb) in [(p0.x, p0.x + 0.25), (p1.x - 0.25, p1.x)] {
        let mut end = slab(
            v3(xa, -0.05, p0.z + 0.25),
            v3(xb, top - 0.22, edge - 0.25),
            0.006,
            0.5,
        );
        panel_uvs(&mut end, Vec3::Z, p0.z, edge - p0.z);
        panels.append(&end, Mat4::IDENTITY);
    }
    wall_grime(&mut panels, 0.0, top, 68, |p| p.x + p.z);
    kit.add(PANEL, panels);

    // Dock bumpers on steel plates, one per bay.
    let mut rubber = M::new();
    let mut dark = M::new();
    for w in bays.windows(2) {
        let xc = (w[0] + w[1]) * 0.5;
        dark.append(
            &slab(
                v3(xc - 0.2, 0.3, edge - 0.02),
                v3(xc + 0.2, 0.8, edge + 0.012),
                0.006,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        bolts(
            &mut dark,
            &[
                v3(xc - 0.16, 0.34, edge + 0.012),
                v3(xc + 0.16, 0.34, edge + 0.012),
                v3(xc - 0.16, 0.76, edge + 0.012),
                v3(xc + 0.16, 0.76, edge + 0.012),
            ],
            Vec3::Z,
            0.014,
        );
        rubber.append(
            &slab(
                v3(xc - 0.13, 0.38, edge + 0.012),
                v3(xc + 0.13, 0.72, edge + 0.12),
                0.02,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(RUBBER, rubber);

    // Cases stacked at the back edge, out of the way.
    let mut olive = M::new();
    let mut guards = M::new();
    for (size, pos, yaw) in [
        (v3(1.2, 0.6, 0.8), v3(-3.4, top, 24.05), 0.04f32),
        (v3(0.9, 0.5, 0.6), v3(-3.45, top + 0.6, 24.05), -0.1),
        (v3(1.0, 0.7, 0.7), v3(-4.9, top, 24.0), 0.12),
    ] {
        let (body, guard) = case(size);
        let to = rt(Quat::from_rotation_y(yaw), pos);
        olive.append(&body, to);
        guards.append(&guard, to);
    }
    kit.add(STEEL_OLIVE, olive);
    kit.add(RUBBER, guards);
    kit.add(STEEL_DARK, dark);

    // Paint, read by someone on the platform facing the track.
    let mut paint = M::new();
    let y = top + 0.0025;
    let yellow = paint_color(YELLOW_PAINT);
    let white = paint_color(WHITE_PAINT);
    let orange = paint_color(ORANGE_PAINT);
    p_line(
        &mut paint,
        v3(mid, y, cope + 0.12),
        Vec3::X,
        p1.x - p0.x - 0.3,
        0.1,
        yellow,
    );
    for x in [-22.5f32, -16.5, -10.5, -4.5] {
        stencil(
            &mut paint,
            PAINT_MIND,
            v3(x, y, edge - 0.25),
            Vec3::NEG_X,
            Vec3::Z,
            0.14,
            white,
        );
    }
    // The yellow band along the coping's nose.
    paint_box(
        &mut paint,
        v3(mid, top - 0.045, edge + 0.0405),
        Vec3::X,
        Vec3::Y,
        Vec2::new(p1.x - p0.x - 0.1, 0.05),
        yellow,
    );
    // The station number big near the start, and chevrons along the run.
    stencil(
        &mut paint,
        PAINT_08,
        v3(-19.6, y, 24.75),
        Vec3::NEG_X,
        Vec3::Z,
        1.0,
        orange,
    );
    stencil(
        &mut paint,
        PAINT_RIDE,
        v3(-22.3, y, 24.45),
        Vec3::NEG_X,
        Vec3::Z,
        0.42,
        orange,
    );
    for x in [-15.5f32, -12.0] {
        stencil(
            &mut paint,
            9,
            v3(x, y, 25.9),
            Vec3::X,
            Vec3::NEG_Z,
            0.32,
            white,
        );
    }
    kit.add(PAINT, paint);
}

/// Lamp posts on the ground behind the platform, their heads reaching over it.
fn lamps(kit: &mut Kit) {
    let (p0, _) = bounds(layout::FREIGHT_PLATFORM);
    let z = p0.z - 0.55;
    let (hy, hz) = LAMP_HEAD;
    let mut concrete = M::new();
    let mut steel = M::new();
    let mut dark = M::new();
    let mut alu = M::new();
    let mut white = M::new();
    for x in LAMP_X {
        concrete.append(
            &revolve(
                &[
                    Vec2::new(0.0, -0.3),
                    Vec2::new(0.32, -0.3),
                    Vec2::new(0.32, 0.18),
                    Vec2::new(0.27, 0.23),
                    Vec2::new(0.0, 0.23),
                ],
                16,
            ),
            at(x, 0.0, z),
        );
        dark.append(
            &cube(v3(x - 0.16, 0.23, z - 0.16), v3(x + 0.16, 0.25, z + 0.16)),
            Mat4::IDENTITY,
        );
        bolts(
            &mut dark,
            &[
                v3(x - 0.12, 0.25, z - 0.12),
                v3(x + 0.12, 0.25, z - 0.12),
                v3(x - 0.12, 0.25, z + 0.12),
                v3(x + 0.12, 0.25, z + 0.12),
            ],
            Vec3::Y,
            0.014,
        );
        steel.append(
            &revolve(
                &[
                    Vec2::new(0.0, 0.25),
                    Vec2::new(0.085, 0.25),
                    Vec2::new(0.085, 0.4),
                    Vec2::new(0.06, hy - 0.2),
                    Vec2::new(0.0, hy - 0.2),
                ],
                16,
            ),
            at(x, 0.0, z),
        );
        // The arm curving out over the platform.
        let arm: Vec<Vec3> = (0..=8)
            .map(|i| {
                let t = i as f32 / 8.0;
                let a = t * FRAC_PI_2;
                v3(
                    x,
                    hy - 0.45 + 0.4 * a.sin(),
                    z + (hz - 0.35 - z) * (1.0 - a.cos()),
                )
            })
            .collect();
        steel.append(&pipe(&arm, 0.045, 10), Mat4::IDENTITY);
        // Luminaire: a slim housing with a lens underneath.
        alu.append(
            &slab(
                v3(x - 0.17, hy - 0.08, hz - 0.45),
                v3(x + 0.17, hy + 0.04, hz + 0.4),
                0.025,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        white.append(
            &cube(
                v3(x - 0.13, hy - 0.086, hz - 0.38),
                v3(x + 0.13, hy - 0.08, hz + 0.33),
            ),
            Mat4::IDENTITY,
        );
        // Junction box and conduit.
        dark.append(
            &slab(
                v3(x - 0.1, 1.0, z - 0.17),
                v3(x + 0.1, 1.35, z - 0.06),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &rod(v3(x, 0.23, z - 0.12), v3(x, 1.0, z - 0.12), 0.018, 6),
            Mat4::IDENTITY,
        );
    }
    steel.shade(|p, _| 0.8 + 0.2 * smoothstep(0.0, 2.0, p.y));
    kit.add(CONCRETE, concrete);
    kit.add(STEEL, steel);
    kit.add(STEEL_DARK, dark);
    kit.add(ALU, alu);
    kit.add(GLOW_WHITE, white);
}

// ---------------------------------------------------------------------------- the car

/// Heights in the car's frame (origin at its collider's center).
struct Car {
    /// Half the collider's size.
    half: Vec3,
    /// The rail tops, the axle centers, the floor's top, the sole bars' bottom and the eaves.
    rail: f32,
    axle: f32,
    floor: f32,
    sill: f32,
    eave: f32,
}

/// Exterior posts along each side (x), clear of the door.
const POSTS: [f32; 6] = [-4.6, -3.35, -2.15, 2.15, 3.35, 4.6];
/// Half the door opening's width.
const DOOR: f32 = 1.62;

/// The box car in its own frame: x along the track, origin at the collider's center. Its
/// sides (posts, door, sole bars) and roof are the collider's faces.
pub(super) fn build_car() -> Kit {
    let half = Vec3::from(layout::FREIGHT_CAR_SIZE) * 0.5;
    let ground = half.y - layout::FREIGHT_ROOF as f32;
    let rail = ground + RAIL_TOP;
    let c = Car {
        half,
        rail,
        axle: rail + WHEEL_R,
        floor: rail + 1.25,
        sill: rail + 0.85,
        eave: half.y - 0.14,
    };
    let mut kit = Kit::default();
    sides(&mut kit, &c);
    roof(&mut kit, &c);
    ends(&mut kit, &c);
    underframe(&mut kit, &c);
    for bx in BOGIE_X {
        bogie(&mut kit, &c, bx);
    }
    kit
}

/// Both sides: sheet, posts, sole bar, top chord, the sliding door on its tracks, ladders,
/// steps and stencils.
fn sides(kit: &mut Kit, c: &Car) {
    let (l, w) = (c.half.x, c.half.z);
    let mut body = M::new();
    let mut dark = M::new();
    let mut yellow = M::new();
    let mut alu = M::new();
    let door_top = c.eave - 0.24;
    // The −Z side, mirrored to +Z below.
    body.append(
        &slab(
            v3(-l + 0.08, c.floor - 0.02, -w + 0.04),
            v3(l - 0.08, c.eave, -w + 0.06),
            0.003,
            0.4,
        ),
        Mat4::IDENTITY,
    );
    body.append(
        &slab(
            v3(-l, c.eave - 0.09, -w),
            v3(l, c.eave + 0.02, -w + 0.08),
            0.012,
            0.5,
        ),
        Mat4::IDENTITY,
    );
    for x in POSTS {
        body.append(
            &cube(
                v3(x - 0.095, c.floor, -w + 0.035),
                v3(x + 0.095, c.eave - 0.09, -w + 0.045),
            ),
            Mat4::IDENTITY,
        );
        body.append(
            &slab(
                v3(x - 0.055, c.floor - 0.01, -w),
                v3(x + 0.055, c.eave - 0.09, -w + 0.045),
                0.014,
                0.5,
            ),
            Mat4::IDENTITY,
        );
    }
    // Door posts and the door: ribbed panel with stiles and rails.
    for x in [-DOOR - 0.07, DOOR + 0.07] {
        body.append(
            &slab(
                v3(x - 0.06, c.floor - 0.01, -w + 0.012),
                v3(x + 0.06, c.eave - 0.09, -w + 0.045),
                0.012,
                0.5,
            ),
            Mat4::IDENTITY,
        );
    }
    let dh = door_top - c.floor - 0.06;
    body.append(
        &corrugated(2.0 * DOOR - 0.14, dh - 0.12, 0.29, 0.022),
        at(-DOOR + 0.07, c.floor + 0.12, -w),
    );
    for (a, b) in [(-DOOR, -DOOR + 0.07), (DOOR - 0.07, DOOR)] {
        body.append(
            &slab(
                v3(a, c.floor + 0.06, -w),
                v3(b, door_top, -w + 0.03),
                0.006,
                0.5,
            ),
            Mat4::IDENTITY,
        );
    }
    for (y0, y1) in [
        (c.floor + 0.06, c.floor + 0.12),
        (door_top - 0.12, door_top),
    ] {
        body.append(
            &slab(v3(-DOOR, y0, -w), v3(DOOR, y1, -w + 0.03), 0.006, 0.5),
            Mat4::IDENTITY,
        );
    }
    // Door tracks above and below, rollers, handles, the latch.
    dark.append(
        &slab(
            v3(-3.4, door_top + 0.02, -w + 0.005),
            v3(3.4, door_top + 0.13, -w + 0.05),
            0.008,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &slab(
            v3(-3.4, c.floor - 0.07, -w + 0.005),
            v3(3.4, c.floor - 0.01, -w + 0.04),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for x in [-DOOR + 0.35, DOOR - 0.35] {
        dark.append(
            &cylinder(
                v3(x, door_top + 0.06, -w + 0.004),
                v3(x, door_top + 0.06, -w + 0.03),
                0.065,
                14,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &cube(
                v3(x - 0.05, door_top - 0.03, -w + 0.004),
                v3(x + 0.05, door_top + 0.06, -w + 0.02),
            ),
            Mat4::IDENTITY,
        );
    }
    for x in [-DOOR + 0.12, DOOR - 0.12] {
        dark.append(
            &pipe(
                &[
                    v3(x, c.floor + 0.55, -w + 0.002),
                    v3(x, c.floor + 0.58, -w - 0.03),
                    v3(x, c.floor + 1.35, -w - 0.03),
                    v3(x, c.floor + 1.38, -w + 0.002),
                ],
                0.014,
                8,
            ),
            Mat4::IDENTITY,
        );
    }
    let ly = c.floor + 0.95;
    dark.append(
        &slab(
            v3(DOOR - 0.03, ly - 0.06, -w - 0.012),
            v3(DOOR + 0.22, ly + 0.06, -w + 0.01),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &rod(
            v3(DOOR + 0.1, ly - 0.35, -w - 0.022),
            v3(DOOR + 0.1, ly + 0.2, -w - 0.022),
            0.012,
            6,
        ),
        Mat4::IDENTITY,
    );
    alu.append(
        &cube(
            v3(DOOR + 0.04, ly - 0.02, -w - 0.03),
            v3(DOOR + 0.07, ly + 0.02, -w - 0.012),
        ),
        Mat4::IDENTITY,
    );
    // Sole bar: a channel along the bottom, its web the collider face.
    dark.append(
        &slab(v3(-l, c.sill, -w), v3(l, c.floor, -w + 0.1), 0.01, 0.5),
        Mat4::IDENTITY,
    );
    dark.append(
        &cube(v3(-l, c.sill, -w + 0.1), v3(l, c.sill + 0.03, -w + 0.22)),
        Mat4::IDENTITY,
    );
    // Ladders near both ends, and stirrup steps under them.
    for x in [-l + 0.55, l - 0.55] {
        for dx in [-0.22f32, 0.19] {
            yellow.append(
                &cube(
                    v3(x + dx, c.sill + 0.05, -w - 0.06),
                    v3(x + dx + 0.03, c.eave - 0.15, -w),
                ),
                Mat4::IDENTITY,
            );
        }
        let mut y = c.floor + 0.12;
        while y < c.eave - 0.2 {
            yellow.append(
                &rod(
                    v3(x - 0.2, y, -w - 0.045),
                    v3(x + 0.2, y, -w - 0.045),
                    0.011,
                    6,
                ),
                Mat4::IDENTITY,
            );
            y += 0.34;
        }
        yellow.append(
            &pipe(
                &[
                    v3(x - 0.2, c.sill + 0.02, -w + 0.03),
                    v3(x - 0.2, c.sill - 0.36, -w + 0.03),
                    v3(x + 0.2, c.sill - 0.36, -w + 0.03),
                    v3(x + 0.2, c.sill + 0.02, -w + 0.03),
                ],
                0.014,
                6,
            ),
            Mat4::IDENTITY,
        );
    }
    let mirror = Mat4::from_scale(v3(1.0, 1.0, -1.0));
    for m in [&mut body, &mut dark, &mut yellow, &mut alu] {
        let other = m.transformed(mirror);
        m.append(&other, Mat4::IDENTITY);
    }
    // Weathering: grime at the foot, streaks from the eaves, blotches.
    let (floor, eave) = (c.floor, c.eave);
    body.shade(move |p, _| {
        let along = p.x * 1.7 + p.z * 0.3;
        let foot = 0.7 + 0.3 * smoothstep(floor - 0.1, floor + 0.7, p.y);
        let col = pn(along, 0.5, 71) * 0.5 + 0.5;
        let streak =
            (-(eave - p.y).max(0.0) / (0.3 + 1.2 * col)).exp() * smoothstep(0.45, 0.8, col);
        let blot = 1.0 + 0.1 * pn(p.x * 0.5, p.y * 0.6, 72);
        foot * (1.0 - 0.3 * streak) * blot
    });
    dark.shade(|p, _| 0.85 + 0.15 * pn(p.x * 0.9, p.y, 73));
    kit.add(OXIDE, body);
    kit.add(STEEL_DARK, dark);
    kit.add(STEEL_YELLOW, yellow);
    kit.add(ALU, alu);

    // Stencils on each side, read from that side: reporting mark, data panel.
    let mut paint = M::new();
    let white = paint_color(WHITE_PAINT) * Vec4::new(0.9, 0.9, 0.88, 1.0);
    for side in [-1.0f32, 1.0] {
        let z = side * (w - 0.04 + 0.0006);
        let normal = Vec3::Z * side;
        let right = Vec3::Y.cross(normal);
        let px = (POSTS[1] + POSTS[2]) * 0.5 * side;
        stencil(
            &mut paint,
            PAINT_MARK,
            v3(px, c.floor + 1.32, z),
            right,
            Vec3::Y,
            0.105,
            white,
        );
        paint_box(
            &mut paint,
            v3(px, c.floor + 1.12, z),
            right,
            Vec3::Y,
            Vec2::new(0.82, 0.018),
            white,
        );
        let dx = (POSTS[4] + POSTS[5]) * 0.5 * side;
        for (k, wdt) in [0.5f32, 0.38, 0.44].into_iter().enumerate() {
            paint_box(
                &mut paint,
                v3(dx, c.floor + 0.5 - k as f32 * 0.07, z),
                right,
                Vec3::Y,
                Vec2::new(wdt, 0.035),
                white,
            );
        }
    }
    kit.add(PAINT, paint);
}

/// The roof: a sheet whose top is the collider's, low seam caps across it, rounded eaves.
fn roof(kit: &mut Kit, c: &Car) {
    let (l, w, top) = (c.half.x, c.half.z, c.half.y);
    let mut body = M::new();
    body.append(
        &slab(
            v3(-l - 0.02, c.eave + 0.02, -w),
            v3(l + 0.02, top, w),
            0.035,
            0.5,
        ),
        Mat4::IDENTITY,
    );
    let mut x = -l + 0.6;
    while x < l - 0.5 {
        body.append(
            &slab(
                v3(x - 0.03, top - 0.004, -w + 0.07),
                v3(x + 0.03, top + 0.008, w - 0.07),
                0.004,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        x += 0.9;
    }
    // Sun-faded and dusty on top, dark streaks running off the edges.
    body.shade(|p, n| {
        let up = if n.y > 0.5 { 1.08 } else { 0.85 };
        let dust = 1.0 + 0.1 * pn(p.x * 0.7, p.z * 1.3, 74);
        let edge = smoothstep(w - 0.5, w, p.z.abs());
        up * dust * (1.0 - 0.2 * edge)
    });
    kit.add(ROOF_GREY, body);
}

/// Both ends: pressed sheet with horizontal ribs, corner posts, the headstock with its buffers,
/// draw hook and screw coupling, brake hoses, and a brake wheel at the +X end.
fn ends(kit: &mut Kit, c: &Car) {
    let (l, w) = (c.half.x, c.half.z);
    let mut body = M::new();
    let mut dark = M::new();
    let mut rust = M::new();
    let mut bright = M::new();
    let mut rubber = M::new();
    let mut yellow = M::new();
    let by = c.rail + BUFFER_H;
    // The +X end, turned around for the −X one.
    body.append(
        &slab(
            v3(l - 0.03, c.floor - 0.02, -w + 0.06),
            v3(l, c.eave, w - 0.06),
            0.003,
            0.4,
        ),
        Mat4::IDENTITY,
    );
    let ribs = 5;
    for k in 0..ribs {
        let y = c.floor + 0.3 + (c.eave - c.floor - 0.55) * k as f32 / (ribs - 1) as f32;
        body.append(
            &slab(
                v3(l - 0.01, y - 0.1, -w + 0.16),
                v3(l + 0.045, y + 0.1, w - 0.16),
                0.032,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    for s in [-1.0f32, 1.0] {
        body.append(
            &slab(
                v3(l - 0.1, c.sill + 0.05, s * w),
                v3(l + 0.05, c.eave + 0.02, s * (w - 0.12)),
                0.01,
                0.5,
            ),
            Mat4::IDENTITY,
        );
    }
    // Headstock across the end at buffer height.
    let hs = 0.14;
    dark.append(
        &slab(
            v3(l, c.sill - 0.03, -w + 0.04),
            v3(l + hs, c.floor + 0.02, w - 0.04),
            0.012,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    let (b_dark, b_bright) = buffer(BUFFER_REACH - hs, 0.22);
    for s in [-BUFFER_Z, BUFFER_Z] {
        let to = at(l + hs, by, s);
        dark.append(&b_dark, to);
        bright.append(&b_bright, to);
    }
    // Draw hook, its guide, and the screw coupling hanging from it.
    let hx = l + hs;
    rust.append(
        &slab(
            v3(hx, by - 0.07, -0.035),
            v3(hx + 0.3, by + 0.07, 0.035),
            0.012,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    rust.append(
        &pipe(
            &[
                v3(hx + 0.28, by + 0.01, 0.0),
                v3(hx + 0.36, by + 0.06, 0.0),
                v3(hx + 0.4, by + 0.0, 0.0),
                v3(hx + 0.37, by - 0.07, 0.0),
            ],
            0.03,
            8,
        ),
        Mat4::IDENTITY,
    );
    let link = |s: f32| -> Vec<Vec3> {
        vec![
            v3(hx + 0.27, by - 0.02, s * 0.06),
            v3(hx + 0.33, by - 0.22, s * 0.09),
            v3(hx + 0.38, by - 0.45, s * 0.09),
        ]
    };
    for s in [-1.0f32, 1.0] {
        rust.append(&pipe(&link(s), 0.016, 6), Mat4::IDENTITY);
    }
    rust.append(
        &cylinder(
            v3(hx + 0.38, by - 0.45, -0.1),
            v3(hx + 0.38, by - 0.45, 0.1),
            0.035,
            10,
        ),
        Mat4::IDENTITY,
    );
    rust.append(
        &rod(
            v3(hx + 0.38, by - 0.45, 0.0),
            v3(hx + 0.42, by - 0.72, 0.0),
            0.028,
            8,
        ),
        Mat4::IDENTITY,
    );
    rust.append(
        &pipe(
            &[
                v3(hx + 0.42, by - 0.72, -0.1),
                v3(hx + 0.44, by - 0.86, -0.08),
                v3(hx + 0.44, by - 0.86, 0.08),
                v3(hx + 0.42, by - 0.72, 0.1),
            ],
            0.018,
            6,
        ),
        Mat4::IDENTITY,
    );
    // Brake hoses hanging off the headstock with their coupling heads.
    for s in [-0.36f32, 0.36] {
        let path = [
            v3(hx, by - 0.12, s),
            v3(hx + 0.12, by - 0.3, s),
            v3(hx + 0.2, by - 0.48, s * 0.95),
            v3(hx + 0.24, by - 0.52, s * 0.9),
        ];
        rubber.append(&pipe(&path, 0.024, 8), Mat4::IDENTITY);
        dark.append(
            &slab(
                path[3] + v3(-0.02, -0.06, -0.035),
                path[3] + v3(0.07, 0.03, 0.035),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    // A grab iron and a step at each corner.
    for s in [-1.0f32, 1.0] {
        yellow.append(
            &pipe(
                &[
                    v3(l + 0.005, c.floor + 0.2, s * (w - 0.3)),
                    v3(l + 0.07, c.floor + 0.22, s * (w - 0.3)),
                    v3(l + 0.07, c.floor + 1.0, s * (w - 0.3)),
                    v3(l + 0.005, c.floor + 1.02, s * (w - 0.3)),
                ],
                0.013,
                6,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &slab(
                v3(l + 0.02, c.sill - 0.42, s * (w - 0.42)),
                v3(l + 0.26, c.sill - 0.39, s * (w - 0.1)),
                0.006,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &cube(
                v3(l + 0.02, c.sill - 0.42, s * (w - 0.12)),
                v3(l + 0.05, c.sill + 0.02, s * (w - 0.1)),
            ),
            Mat4::IDENTITY,
        );
    }
    let turn = Mat4::from_rotation_y(PI);
    for m in [
        &mut body,
        &mut dark,
        &mut rust,
        &mut bright,
        &mut rubber,
        &mut yellow,
    ] {
        let other = m.transformed(turn);
        m.append(&other, Mat4::IDENTITY);
    }
    // The hand brake wheel high on the +X end, with its column and gearbox.
    let wc = v3(l + 0.12, c.floor + 1.55, w - 0.55);
    let rim: Vec<Vec3> = (0..=24)
        .map(|i| {
            let a = i as f32 / 24.0 * TAU;
            wc + v3(0.0, a.cos() * 0.22, a.sin() * 0.22)
        })
        .collect();
    dark.append(&pipe(&rim, 0.014, 6), Mat4::IDENTITY);
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU + 0.4;
        dark.append(
            &rod(wc, wc + v3(0.0, a.cos() * 0.21, a.sin() * 0.21), 0.01, 5),
            Mat4::IDENTITY,
        );
    }
    dark.append(
        &cylinder(wc - v3(0.06, 0.0, 0.0), wc + v3(0.02, 0.0, 0.0), 0.035, 10),
        Mat4::IDENTITY,
    );
    dark.append(
        &slab(
            v3(l + 0.01, wc.y - 0.12, wc.z - 0.1),
            v3(l + 0.08, wc.y + 0.1, wc.z + 0.1),
            0.01,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &rod(
            v3(l + 0.06, wc.y - 0.12, wc.z),
            v3(l + 0.06, c.floor, wc.z),
            0.016,
            6,
        ),
        Mat4::IDENTITY,
    );
    let (floor, eave) = (c.floor, c.eave);
    body.shade(move |p, _| {
        let foot = 0.72 + 0.28 * smoothstep(floor - 0.1, floor + 0.6, p.y);
        let col = pn(p.z * 2.1, 3.5, 75) * 0.5 + 0.5;
        let streak = (-(eave - p.y).max(0.0) / (0.3 + col)).exp() * smoothstep(0.5, 0.85, col);
        foot * (1.0 - 0.25 * streak)
    });
    kit.add(OXIDE, body);
    kit.add(STEEL_DARK, dark);
    kit.add(RUST, rust);
    kit.add(ALU, bright);
    kit.add(RUBBER, rubber);
    kit.add(STEEL_YELLOW, yellow);
}

/// Under the floor: center sill, cross bearers, body bolsters, brake cylinder, reservoir,
/// distributor and the brake pipe.
fn underframe(kit: &mut Kit, c: &Car) {
    let (l, w) = (c.half.x, c.half.z);
    let mut dark = M::new();
    let mut steel = M::new();
    dark.append(
        &slab(
            v3(-l + 0.1, c.floor - 0.05, -w + 0.06),
            v3(l - 0.1, c.floor, w - 0.06),
            0.004,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &slab(
            v3(-l, c.sill - 0.05, -0.18),
            v3(l, c.floor - 0.05, 0.18),
            0.01,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for x in [-3.0f32, -1.5, 0.0, 1.5, 3.0] {
        dark.append(
            &cube(
                v3(x - 0.05, c.sill + 0.06, -w + 0.1),
                v3(x + 0.05, c.floor - 0.05, w - 0.1),
            ),
            Mat4::IDENTITY,
        );
    }
    for bx in BOGIE_X {
        dark.append(
            &slab(
                v3(bx - 0.2, c.sill - 0.08, -w + 0.1),
                v3(bx + 0.2, c.floor - 0.05, w - 0.1),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    let y = c.sill - 0.14;
    steel.append(
        &cylinder(v3(0.3, y, -0.62), v3(1.8, y, -0.62), 0.21, 18),
        Mat4::IDENTITY,
    );
    for x in [0.32f32, 1.78] {
        steel.append(
            &cylinder(v3(x - 0.01, y, -0.62), v3(x + 0.01, y, -0.62), 0.215, 18),
            Mat4::IDENTITY,
        );
    }
    steel.append(
        &cylinder(v3(-1.6, y, 0.55), v3(-1.0, y, 0.55), 0.16, 18),
        Mat4::IDENTITY,
    );
    for z in [-0.62f32, 0.55] {
        for x in [-1.3f32, 1.05] {
            dark.append(
                &cube(
                    v3(x - 0.04, c.sill - 0.02, z - 0.25),
                    v3(x + 0.04, c.floor - 0.05, z + 0.25),
                ),
                Mat4::IDENTITY,
            );
        }
    }
    dark.append(
        &slab(
            v3(-0.5, c.sill - 0.3, 0.35),
            v3(-0.1, c.sill + 0.05, 0.7),
            0.015,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // Pull rods from the cylinder's levers out to both bogies.
    for z in [0.48f32, 0.62] {
        dark.append(
            &rod(v3(-1.6, y, z), v3(BOGIE_X[0] + 1.3, y, z), 0.016, 6),
            Mat4::IDENTITY,
        );
        dark.append(
            &rod(v3(-1.0, y, z), v3(BOGIE_X[1] - 1.3, y, z), 0.016, 6),
            Mat4::IDENTITY,
        );
    }
    // The brake pipe along the sill, out to the hoses at both ends.
    dark.append(
        &pipe(
            &[
                v3(-l - 0.05, c.sill + 0.08, 0.36),
                v3(-l + 0.3, c.sill + 0.02, 0.3),
                v3(l - 0.3, c.sill + 0.02, 0.3),
                v3(l + 0.05, c.sill + 0.08, 0.36),
            ],
            0.02,
            6,
        ),
        Mat4::IDENTITY,
    );
    steel.shade(|p, _| 0.6 + 0.15 * pn(p.x * 2.0, p.z * 2.0, 76));
    kit.add(STEEL_DARK, dark);
    kit.add(STEEL, steel);
}

/// A two-axle bogie at `bx`: wheelsets with bright treads, axle boxes riding on coil springs
/// under a welded frame, a bolster, and brake blocks on every wheel.
fn bogie(kit: &mut Kit, c: &Car, bx: f32) {
    let mut dark = M::new();
    let mut rust = M::new();
    let mut bright = M::new();
    let ax = c.axle;
    // One wheel on the +Z rail, its flange face toward the center.
    let wheel_body = revolve(
        &[
            Vec2::new(0.0, -0.06),
            Vec2::new(0.11, -0.06),
            Vec2::new(0.11, -0.01),
            Vec2::new(0.36, 0.0),
            Vec2::new(0.446, 0.0),
            Vec2::new(0.452, 0.012),
            Vec2::new(0.44, 0.03),
            Vec2::new(0.425, 0.04),
        ],
        32,
    );
    let tread = revolve(
        &[
            Vec2::new(0.425, 0.04),
            Vec2::new(0.418, 0.132),
            Vec2::new(0.408, 0.14),
        ],
        32,
    );
    let wheel_face = revolve(
        &[
            Vec2::new(0.408, 0.14),
            Vec2::new(0.37, 0.14),
            Vec2::new(0.36, 0.11),
            Vec2::new(0.13, 0.09),
            Vec2::new(0.12, 0.17),
            Vec2::new(0.0, 0.17),
        ],
        32,
    );
    let to_z = Quat::from_rotation_x(FRAC_PI_2);
    let z0 = RAIL_C - 0.068;
    for dx in [-0.9f32, 0.9] {
        let x = bx + dx;
        for side in [-1.0f32, 1.0] {
            let to = Mat4::from_scale(v3(1.0, 1.0, side)) * rt(to_z, v3(x, ax, z0));
            rust.append(&wheel_body, to);
            rust.append(&wheel_face, to);
            bright.append(&tread, to);
            // Axle box with its cover, two springs on its lugs, the brake block.
            let bz = side * 0.99;
            dark.append(
                &cylinder(v3(x, ax, side * 0.86), v3(x, ax, side * 1.07), 0.115, 16),
                Mat4::IDENTITY,
            );
            rust.append(
                &cylinder(v3(x, ax, side * 1.07), v3(x, ax, side * 1.09), 0.09, 16),
                Mat4::IDENTITY,
            );
            dark.append(
                &slab(
                    v3(x - 0.24, ax + 0.02, bz - 0.08),
                    v3(x + 0.24, ax + 0.06, bz + 0.08),
                    0.008,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
            for sx in [-0.17f32, 0.17] {
                rust.append(
                    &spring(v3(x + sx, ax + 0.06, bz), 0.12, 0.05, 0.012, 3.5),
                    Mat4::IDENTITY,
                );
            }
            let toward = -dx.signum();
            let shoe_x = x + toward * (WHEEL_R + 0.04);
            dark.append(
                &slab(
                    v3(shoe_x - 0.04, ax - 0.16, side * (RAIL_C - 0.05)),
                    v3(shoe_x + 0.04, ax + 0.16, side * (RAIL_C + 0.05)),
                    0.008,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
        // The axle and the brake beam across.
        rust.append(
            &cylinder(v3(x, ax, -0.86), v3(x, ax, 0.86), 0.075, 14),
            Mat4::IDENTITY,
        );
        let shoe_x = x - dx.signum() * (WHEEL_R + 0.06);
        dark.append(
            &rod(v3(shoe_x, ax, -RAIL_C), v3(shoe_x, ax, RAIL_C), 0.03, 8),
            Mat4::IDENTITY,
        );
    }
    // The welded frame on the springs: side members, end and center transoms, the bolster.
    let fy = (ax + 0.18, ax + 0.36);
    for side in [-1.0f32, 1.0] {
        let (za, zb) = (side * 0.93, side * 1.06);
        dark.append(
            &slab(
                v3(bx - 1.3, fy.0, za.min(zb)),
                v3(bx + 1.3, fy.1, za.max(zb)),
                0.015,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        for dx in [-0.9f32, 0.9] {
            // Guards hugging the axle box.
            for gx in [-0.27f32, 0.27] {
                dark.append(
                    &cube(
                        v3(bx + dx + gx - 0.02, ax - 0.12, side * 0.93),
                        v3(bx + dx + gx + 0.02, fy.0, side * 1.06),
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
    }
    for tx in [bx - 1.25, bx + 1.25] {
        dark.append(
            &slab(
                v3(tx - 0.06, fy.0 + 0.02, -0.93),
                v3(tx + 0.06, fy.1 - 0.04, 0.93),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    dark.append(
        &slab(
            v3(bx - 0.24, fy.0 - 0.05, -0.93),
            v3(bx + 0.24, c.sill - 0.08, 0.93),
            0.015,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &cylinder(
            v3(bx, c.sill - 0.08, 0.0),
            v3(bx, c.sill - 0.02, 0.0),
            0.2,
            16,
        ),
        Mat4::IDENTITY,
    );
    dark.shade(|p, _| 0.75 + 0.25 * pn(p.x * 1.7, p.y * 2.0 + p.z, 77).abs());
    kit.add(STEEL_DARK, dark);
    kit.add(RUST, rust);
    kit.add(ALU, bright);
}
