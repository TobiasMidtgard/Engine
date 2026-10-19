//! The zip line station: a launch gantry on the grapple tower's roof, the line itself (drawn by
//! [`crate::look::zipline`] from the demo's [`Zipline`](crate::zipline::Zipline)) and a
//! landing gate on its own pad, 26 m away and ten meters lower.
//!
//! - **Launch gantry**: two orange wide-flange posts on bolted, gusseted base plates either
//!   side of the run-up lane, an I-beam crossbeam (stiffened over the posts and the anchor)
//!   with the line's anchor lug, shackle, turnbuckle and rope clips under its middle, knee
//!   braces, and two back-stays down to the roof. Lane lines and chevrons on the roof lead to a
//!   gap in the railing.
//! - **Landing gate**: the same frame, its posts padded in hazard sleeves, with the line's
//!   other end under the crossbeam, a brake spring and rubber stop on the cable before it, a
//!   backlit "09 ZIP" sign on top and back-stays out to concrete deadmen past the gate. Riders
//!   let go before the spring and drop through.
//! - **Pad**: saw-cut concrete with hazard edges, a strip of rubber landing mats along the line
//!   and its paint.
//!
//! Everything is built in each end's own frame (X down the line, Z across it, Y up, the anchor
//! over the origin) and turned into the course's.

use super::*;

/// Wide-flange posts (as wide as deep) and the I-beam crossbeam (flange width, depth), m.
const POST: f32 = 0.2;
const BEAM: Vec2 = Vec2::new(0.18, 0.26);
/// Flange and web thickness of the rolled sections, m.
const FLANGE: f32 = 0.016;
const WEB: f32 = 0.01;
/// Post cap plates, m thick.
const CAP: f32 = 0.016;

/// One end of the line, in its own frame.
struct End {
    /// Course transform of the end's frame (origin on the floor under the anchor).
    to_course: Mat4,
    /// The anchor's height above the floor.
    anchor: f32,
    /// From the line to each post.
    half: f32,
    /// Top of the posts above the floor.
    top: f32,
    /// The cable's direction at the anchor, out along the line (local).
    cable: Vec3,
}

fn ends() -> (End, End) {
    let (along, _) = layout::zip_axes();
    let along = DVec3::from_array(along).as_vec3();
    let q = Quat::from_rotation_arc(Vec3::X, along);
    let from = DVec3::from_array(layout::ZIP_FROM).as_vec3();
    let to = DVec3::from_array(layout::ZIP_TO).as_vec3();
    let roof = layout::GRAPPLE_TOP as f32;
    let floor = (layout::ZIP_PAD.center[1] + layout::ZIP_PAD.size[1] as f64 * 0.5) as f32;
    // The cable leaves each anchor along its sag curve's tangent.
    let run = Vec2::new(to.x - from.x, to.z - from.z).length();
    let drop = to.y - from.y;
    let sag = 4.0 * layout::ZIP_SAG;
    let top = End {
        to_course: rt(q, v3(from.x, roof, from.z)),
        anchor: from.y - roof,
        half: layout::ZIP_GANTRY_HALF as f32,
        top: layout::ZIP_GANTRY_TOP as f32 - roof,
        cable: v3(run, drop - sag, 0.0).normalize(),
    };
    let bottom = End {
        to_course: rt(q, v3(to.x, floor, to.z)),
        anchor: to.y - floor,
        half: layout::ZIP_GATE_HALF as f32,
        top: layout::ZIP_GATE_TOP as f32 - floor,
        cable: -v3(run, drop + sag, 0.0).normalize(),
    };
    (top, bottom)
}

pub(super) fn build() -> Kit {
    let (top, bottom) = ends();
    let mut kit = Kit::default();
    let mut parts = Parts::default();
    frame(&mut parts, &top, false);
    stays(&mut parts, &top, -2.3, 1.5, false);
    frame(&mut parts, &bottom, true);
    stays(&mut parts, &bottom, 2.7, 2.1, true);
    brake(&mut parts, &bottom);
    launch_lane(&mut parts);
    landing_pad(&mut parts, &bottom);
    gate_sign(&mut parts, &bottom);
    for (mesh, end) in [(&mut parts.top, &top), (&mut parts.bottom, &bottom)] {
        for (mat, m) in mesh.drain() {
            kit.add(mat, m.transformed(end.to_course));
        }
    }
    for (mat, m) in parts.course.drain() {
        kit.add(mat, m);
    }
    kit
}

/// Meshes by material, for each end's frame and for the course's.
#[derive(Default)]
struct Parts {
    top: Group,
    bottom: Group,
    course: Group,
}

impl Parts {
    fn at(&mut self, gate: bool) -> &mut Group {
        if gate {
            &mut self.bottom
        } else {
            &mut self.top
        }
    }
}

#[derive(Default)]
struct Group(Vec<(&'static str, M)>);

impl Group {
    fn add(&mut self, material: &'static str, m: M) {
        match self.0.iter_mut().find(|(k, _)| *k == material) {
            Some((_, all)) => {
                all.append(&m, Mat4::IDENTITY);
            }
            None => self.0.push((material, m)),
        }
    }

    fn drain(&mut self) -> Vec<(&'static str, M)> {
        std::mem::take(&mut self.0)
    }
}

/// A rolled I-section from y = 0 to `len`: flanges `width` wide (X) and [`FLANGE`] thick at
/// z = ±`depth`/2, joined by a [`WEB`] web.
fn i_section(len: f32, width: f32, depth: f32) -> M {
    let mut m = M::new();
    for side in [-1.0f32, 1.0] {
        let z = side * (depth - FLANGE) * 0.5;
        m.append(
            &slab(
                v3(-width * 0.5, 0.0, z - FLANGE * 0.5),
                v3(width * 0.5, len, z + FLANGE * 0.5),
                0.003,
                0.5,
            ),
            Mat4::IDENTITY,
        );
    }
    // The web runs a hair into the flanges so no seam shows.
    let inner = depth * 0.5 - FLANGE + 0.001;
    m.append(
        &slab(
            v3(-WEB * 0.5, 0.0, -inner),
            v3(WEB * 0.5, len, inner),
            0.002,
            0.5,
        ),
        Mat4::IDENTITY,
    );
    m
}

/// Stiffener plates in both channels of an I-section whose web is the plane x = 0, at `z`
/// across `y0..y1` (between its flanges), reaching out to `half_width`.
fn stiffeners(m: &mut M, z: f32, y0: f32, y1: f32, half_width: f32) {
    for side in [-1.0f32, 1.0] {
        let (x0, x1) = (side * WEB * 0.5, side * (half_width - 0.006));
        m.append(
            &slab(v3(x0, y0, z - 0.005), v3(x1, y1, z + 0.005), 0.0015, 100.0),
            Mat4::IDENTITY,
        );
    }
}

/// A box-section member from `a` to `b` (local), `size` across (x: width, y: depth), its
/// width side kept as level as the member allows.
fn member(a: Vec3, b: Vec3, size: Vec2, bevel: f32) -> M {
    let d = b - a;
    let len = d.length();
    let dir = d / len.max(1e-5);
    // Box along +Y, turned onto the member.
    let q = Quat::from_rotation_arc(Vec3::Y, dir);
    let mut m = chamfer_grid(v3(size.x, len, size.y), bevel, 0.6);
    m.planar_uvs(1.0);
    m.transformed(rt(q, (a + b) * 0.5))
}

/// Posts, base plates, crossbeam, knee braces and the anchor hardware of one end.
fn frame(parts: &mut Parts, end: &End, gate: bool) {
    let g = parts.at(gate);
    let mut orange = M::new();
    let mut dark = M::new();
    let mut steel = M::new();
    let mut hazard = M::new();
    let (h, top) = (end.half, end.top);
    let beam_lo = top - BEAM.y;
    for side in [-1.0f32, 1.0] {
        let z = side * h;
        // A wide-flange post (web across the line, flanges facing along the beam) on a base
        // plate with four bolts and gussets, capped under the beam.
        let post_top = beam_lo - CAP;
        orange.append(
            &i_section(post_top - 0.025, POST, POST),
            Mat4::from_translation(v3(0.0, 0.025, z)),
        );
        // Level stiffeners in both channels where the knee brace meets the flange.
        let inner = POST * 0.5 - FLANGE + 0.001;
        for x in [-1.0f32, 1.0] {
            let y = beam_lo - 0.55;
            orange.append(
                &slab(
                    v3(x * WEB * 0.5, y - 0.005, z - inner),
                    v3(x * (POST * 0.5 - 0.006), y + 0.005, z + inner),
                    0.0015,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
        orange.append(
            &slab(
                v3(-0.12, post_top, z - 0.12),
                v3(0.12, beam_lo, z + 0.12),
                0.003,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        // Bolted up through the cap into the beam's bottom flange.
        let cap_bolts: Vec<Vec3> = [
            (-0.085, -0.085),
            (0.085, -0.085),
            (-0.085, 0.085),
            (0.085, 0.085),
        ]
        .iter()
        .map(|&(x, dz)| v3(x, post_top, z + dz))
        .collect();
        bolts(&mut steel, &cap_bolts, Vec3::NEG_Y, 0.01);
        dark.append(
            &slab(
                v3(-0.2, 0.0, z - 0.2),
                v3(0.2, 0.025, z + 0.2),
                0.004,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let corners = [(-0.15, -0.15), (0.15, -0.15), (-0.15, 0.15), (0.15, 0.15)];
        let pts: Vec<Vec3> = corners
            .iter()
            .map(|&(x, dz)| v3(x, 0.025, z + dz))
            .collect();
        bolts(&mut steel, &pts, Vec3::Y, 0.012);
        for (gx, gz) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            // A triangular gusset on each flange's outer face and, in the channels, out from
            // the web past the flange edges.
            let (o, out) = if gx != 0.0 {
                (
                    v3(gx * WEB * 0.5, 0.025, z),
                    v3(gx, 0.0, 0.0) * (POST * 0.5 + 0.1),
                )
            } else {
                (v3(0.0, 0.025, z + gz * POST * 0.5), v3(0.0, 0.0, gz) * 0.12)
            };
            let across = v3(gz, 0.0, gx).abs() * 0.006;
            let mut gus = M::extrude(
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(1.0, 0.0),
                    Vec2::new(0.0, 1.0),
                ],
                1.0,
                0.0,
            );
            gus.planar_uvs(1.0);
            // Unit triangle (x out, y up) scaled onto the face.
            let x_axis = out;
            let y_axis = Vec3::Y * if gx != 0.0 { 0.24 } else { 0.16 };
            let mut z_axis = if across.x > 0.0 {
                Vec3::X * 0.012
            } else {
                Vec3::Z * 0.012
            };
            // Keep the triangle's winding (a mirrored gusset would face inward).
            if x_axis.cross(y_axis).dot(z_axis) < 0.0 {
                z_axis = -z_axis;
            }
            let m = Mat4::from_cols(
                x_axis.extend(0.0),
                y_axis.extend(0.0),
                z_axis.extend(0.0),
                (o - z_axis * 0.5).extend(1.0),
            );
            orange.append(&gus, m);
        }
        // Knee braces into the beam.
        let knee = v3(0.0, beam_lo - 0.55, z - side * POST * 0.5);
        let on_beam = v3(0.0, beam_lo, z - side * 0.6);
        orange.append(
            &member(knee, on_beam, Vec2::splat(0.08), 0.004),
            Mat4::IDENTITY,
        );
        if gate {
            // Impact pads round the posts, at body height.
            hazard.append(
                &slab(
                    v3(-0.16, 0.45, z - 0.16),
                    v3(0.16, 2.1, z + 0.16),
                    0.03,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
            for y in [0.45f32, 2.1] {
                dark.append(
                    &slab(
                        v3(-0.17, y - 0.02, z - 0.17),
                        v3(0.17, y + 0.02, z + 0.17),
                        0.005,
                        100.0,
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
    }
    // The crossbeam: an I-beam (web upright) with end plates, stiffened over each post and
    // over the anchor.
    let reach = h + 0.22;
    orange.append(
        &i_section(2.0 * reach, BEAM.x, BEAM.y),
        rt(
            Quat::from_rotation_x(FRAC_PI_2),
            v3(0.0, beam_lo + BEAM.y * 0.5, -reach),
        ),
    );
    let (web_lo, web_hi) = (beam_lo + FLANGE - 0.001, top - FLANGE + 0.001);
    for z in [-h, 0.0, h] {
        stiffeners(&mut orange, z, web_lo, web_hi, BEAM.x * 0.5);
    }
    for side in [-1.0f32, 1.0] {
        orange.append(
            &slab(
                v3(-BEAM.x * 0.5 - 0.01, beam_lo - 0.01, side * reach - 0.006),
                v3(BEAM.x * 0.5 + 0.01, top + 0.01, side * reach + 0.006),
                0.002,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    orange.shade(|p, n| {
        // Grime runs down the members; their tops are dusty.
        let dust = if n.y > 0.5 { 0.9 } else { 1.0 };
        let foot = 0.75 + 0.25 * smoothstep(0.0, 0.5, p.y);
        dust * foot * (0.94 + 0.06 * pn(p.y * 3.0, p.z * 3.0, 91))
    });
    anchor(&mut steel, &mut dark, end, beam_lo);
    g.add(STEEL_ORANGE, orange);
    g.add(STEEL_DARK, dark);
    g.add(ALU, steel);
    if !hazard.is_empty() {
        g.add(HAZARD, hazard);
    }
}

/// The cable's end: a lug under the beam, a shackle, a turnbuckle out along the cable and
/// three rope clips on it.
fn anchor(steel: &mut M, dark: &mut M, end: &End, beam_lo: f32) {
    let a = v3(0.0, end.anchor, 0.0);
    let dir = end.cable;
    // Lug: a plate welded under the beam, along the line, with a hole for the pin.
    dark.append(
        &slab(
            v3(-0.07, a.y + 0.02, -0.01),
            v3(0.07, beam_lo, 0.01),
            0.003,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // Shackle: a bow round the pin, pin across.
    let bow: Vec<Vec3> = (0..=10)
        .map(|i| {
            let t = std::f32::consts::PI * (i as f32 / 10.0);
            a + v3(0.0, 0.05 - 0.05 * t.sin(), 0.03 * t.cos())
        })
        .collect();
    steel.append(&pipe(&bow, 0.008, 8), Mat4::IDENTITY);
    steel.append(
        &rod(a + v3(0.0, 0.05, -0.04), a + v3(0.0, 0.05, 0.04), 0.009, 8),
        Mat4::IDENTITY,
    );
    // Turnbuckle along the cable: an open body between two eyes, hex nuts at its ends.
    let q = Quat::from_rotation_arc(Vec3::Y, dir);
    let body = M::lathe(
        &[
            Vec2::new(0.008, 0.0),
            Vec2::new(0.012, 0.03),
            Vec2::new(0.012, 0.09),
            Vec2::new(0.021, 0.1),
            Vec2::new(0.021, 0.3),
            Vec2::new(0.012, 0.31),
            Vec2::new(0.012, 0.38),
            Vec2::new(0.008, 0.42),
        ],
        12,
    );
    steel.append(&body, rt(q, a));
    for y in [0.09f32, 0.31] {
        let nut = M::lathe(&[Vec2::new(0.024, -0.012), Vec2::new(0.024, 0.012)], 6).flat_shaded();
        steel.append(&nut, rt(q, a + dir * y));
    }
    // Rope clips: U-bolts round the doubled cable on saddles.
    for k in 0..3 {
        let c = a + dir * (0.52 + k as f32 * 0.13);
        steel.append(&rbox(c, v3(0.045, 0.03, 0.035), q, 0.004), Mat4::IDENTITY);
    }
    // The tail of the rope folded back along the cable.
    steel.append(
        &rod(
            a + dir * 0.44,
            a + dir * 0.86 + v3(0.0, 0.0, 0.016),
            0.009,
            8,
        ),
        Mat4::IDENTITY,
    );
}

/// The landing end's brake: a coil spring threaded on the cable short of the gate, a steel
/// collar clamping its gate end to the cable and a rubber stop on its uphill end. Riders let go
/// before it ([`layout::ZIP_END_CLEAR`]), so it only reads as the line's last resort.
fn brake(parts: &mut Parts, end: &End) {
    const FROM: f32 = 0.9;
    const TO: f32 = 2.5;
    const COILS: f32 = 18.0;
    const RADIUS: f32 = 0.05;
    let g = parts.at(true);
    let a = v3(0.0, end.anchor, 0.0);
    let dir = end.cable;
    let side = dir.cross(Vec3::Z).normalize();
    let up = Vec3::Z;
    let n = (COILS * 12.0) as usize;
    let helix: Vec<Vec3> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let ang = t * COILS * std::f32::consts::TAU;
            a + dir * (FROM + (TO - FROM) * t) + (side * ang.cos() + up * ang.sin()) * RADIUS
        })
        .collect();
    let mut steel = M::new();
    steel.append(&pipe(&helix, 0.0065, 6), Mat4::IDENTITY);
    let q = Quat::from_rotation_arc(Vec3::Y, dir);
    // The collar, two half-shells bolted round the cable.
    let collar = M::lathe(
        &[
            Vec2::new(0.02, -0.03),
            Vec2::new(0.064, -0.03),
            Vec2::new(0.068, -0.02),
            Vec2::new(0.068, 0.02),
            Vec2::new(0.064, 0.03),
            Vec2::new(0.02, 0.03),
        ],
        16,
    );
    steel.append(&collar, rt(q, a + dir * (FROM - 0.03)));
    for s in [-1.0f32, 1.0] {
        steel.append(
            &rbox(
                a + dir * (FROM - 0.03) + side * (s * 0.074),
                v3(0.022, 0.05, 0.03),
                q,
                0.003,
            ),
            Mat4::IDENTITY,
        );
    }
    g.add(ALU, steel);
    // The stop: a rubber cylinder the rider's glove would meet, with a steel washer.
    let stop = M::lathe(
        &[
            Vec2::new(0.016, 0.0),
            Vec2::new(0.07, 0.0),
            Vec2::new(0.078, 0.012),
            Vec2::new(0.078, 0.17),
            Vec2::new(0.07, 0.18),
            Vec2::new(0.016, 0.18),
        ],
        18,
    );
    let mut rubber = M::new();
    rubber.append(&stop, rt(q, a + dir * (TO + 0.012)));
    g.add(RUBBER, rubber);
    let mut dark = M::new();
    dark.append(
        &M::lathe(
            &[
                Vec2::new(0.016, 0.0),
                Vec2::new(0.072, 0.0),
                Vec2::new(0.072, 0.012),
                Vec2::new(0.016, 0.012),
            ],
            18,
        ),
        rt(q, a + dir * TO),
    );
    g.add(STEEL_DARK, dark);
}

/// Back-stays from the post tops out to anchors on the floor at `out` along the line and
/// `wide` across it, each with a turnbuckle; at the gate they end in concrete deadmen.
fn stays(parts: &mut Parts, end: &End, out: f32, wide: f32, gate: bool) {
    let g = parts.at(gate);
    let mut steel = M::new();
    let mut dark = M::new();
    let mut concrete = M::new();
    for side in [-1.0f32, 1.0] {
        let from = v3(out.signum() * 0.1, end.top - 0.12, side * end.half);
        let to = v3(out, 0.12, side * wide);
        let d = (to - from).normalize();
        steel.append(&rod(from, to, 0.012, 8), Mat4::IDENTITY);
        let q = Quat::from_rotation_arc(Vec3::Y, d);
        let mid = from.lerp(to, 0.62);
        steel.append(
            &M::lathe(
                &[
                    Vec2::new(0.012, 0.0),
                    Vec2::new(0.019, 0.04),
                    Vec2::new(0.019, 0.24),
                    Vec2::new(0.012, 0.28),
                ],
                10,
            ),
            rt(q, mid),
        );
        // Anchor lug on its plate (or deadman).
        if gate {
            let mut block = slab(
                v3(out - 0.35, -0.25, side * wide - 0.35),
                v3(out + 0.35, 0.1, side * wide + 0.35),
                0.03,
                0.4,
            );
            block.shade(|p, _| 0.82 + 0.18 * smoothstep(-0.1, 0.1, p.y));
            concrete.append(&block, Mat4::IDENTITY);
        }
        dark.append(
            &slab(
                v3(out - 0.12, 0.0, side * wide - 0.09),
                v3(out + 0.12, 0.02, side * wide + 0.09),
                0.003,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &slab(
                v3(out - 0.05, 0.02, side * wide - 0.008),
                v3(out + 0.05, 0.13, side * wide + 0.008),
                0.002,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let pts = [
            v3(out - 0.09, 0.02, side * wide - 0.06),
            v3(out + 0.09, 0.02, side * wide - 0.06),
            v3(out - 0.09, 0.02, side * wide + 0.06),
            v3(out + 0.09, 0.02, side * wide + 0.06),
        ];
        bolts(&mut steel, &pts, Vec3::Y, 0.01);
    }
    g.add(ALU, steel);
    g.add(STEEL_DARK, dark);
    if !concrete.is_empty() {
        g.add(CONCRETE, concrete);
    }
}

/// On the roof: lane lines from the gantry to the edge, chevrons down the line and its number
/// behind the gantry.
fn launch_lane(parts: &mut Parts) {
    let g = parts.at(false);
    let mut paint = M::new();
    let y = 0.0025;
    let yellow = paint_color(YELLOW_PAINT);
    let white = paint_color(WHITE_PAINT);
    // From the gantry to the roof's edge (where the lane crosses the tower's +X face).
    let (_, t1) = bounds(layout::GRAPPLE_TOWER);
    let (along, _) = layout::zip_axes();
    let from = DVec3::from_array(layout::ZIP_FROM).as_vec3();
    let edge = (t1.x - from.x) / along[0] as f32;
    let start = -0.15;
    let len = edge - start - 0.2;
    for side in [-1.0f32, 1.0] {
        p_line(
            &mut paint,
            v3(start + len * 0.5, y, side * 0.7),
            Vec3::X,
            len,
            0.08,
            yellow,
        );
    }
    // Chevrons point down the line; the number reads from behind the gantry (decals face
    // up when right × up does).
    for k in 0..2 {
        stencil(
            &mut paint,
            9,
            v3(0.75 + k as f32 * 1.1, y, 0.0),
            Vec3::X,
            Vec3::NEG_Z,
            0.3,
            white,
        );
    }
    stencil(
        &mut paint,
        PAINT_09,
        v3(-0.75, y, 0.0),
        Vec3::Z,
        Vec3::X,
        0.42,
        paint_color(ORANGE_PAINT),
    );
    g.add(PAINT, paint);
}

/// The landing pad: saw-cut bays, hazard edges, rubber mats along the line past the gate and
/// its paint.
fn landing_pad(parts: &mut Parts, end: &End) {
    let (p0, p1) = bounds(layout::ZIP_PAD);
    let top = p1.y;
    let mut concrete = M::new();
    let g = 0.003;
    let xs = [p0.x, p0.x + 3.5, p0.x + 7.0, p1.x];
    let zs = [p0.z, p0.z + 3.5, p0.z + 7.0, p1.z];
    let to_local = end.to_course.inverse();
    for xw in xs.windows(2) {
        for zw in zs.windows(2) {
            let ga = |v: f32, edge: f32| if (v - edge).abs() < 1e-3 { 0.0 } else { g };
            let min = v3(xw[0] + ga(xw[0], p0.x), -0.3, zw[0] + ga(zw[0], p0.z));
            let max = v3(xw[1] - ga(xw[1], p1.x), top, zw[1] - ga(zw[1], p1.z));
            concrete.append(&slab(min, max, 0.008, 0.6), Mat4::IDENTITY);
        }
    }
    concrete.shade(|p, _| {
        let l = to_local.transform_point3(p);
        // Scuffed where riders land and skid, darker round the posts.
        let land = (-((l.x - 1.6) / 2.2).powi(2) - (l.z / 0.9).powi(2)).exp();
        let posts = [-1.0f32, 1.0]
            .iter()
            .map(|s| {
                let d = Vec2::new(l.x, l.z - s * layout::ZIP_GATE_HALF as f32).length();
                0.4 * (-d / 0.35).exp()
            })
            .sum::<f32>();
        (0.9 + 0.1 * pn(p.x * 0.3, p.z * 0.3, 77)) * (1.0 - 0.14 * land) * (1.0 - posts)
    });
    parts.course.add(CONCRETE, concrete);

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
    parts.course.add(HAZARD, hz);

    // Rubber mats from the gate to past where riders come down, butted in a strip.
    let gate = parts.at(true);
    let mut rubber = M::new();
    for k in 0..4 {
        let x0 = -0.6 + k as f32 * 1.6;
        rubber.append(
            &slab(
                v3(x0 + 0.01, -0.01, -1.05),
                v3(x0 + 1.59, 0.006, 1.05),
                0.004,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    rubber.shade(|p, _| 0.85 + 0.15 * pn(p.x * 1.7, p.z * 1.7, 78));
    gate.add(RUBBER, rubber);
    let mut paint = M::new();
    let y = 0.0025;
    let white = paint_color(WHITE_PAINT);
    let yellow = paint_color(YELLOW_PAINT);
    // The landing box round the mats, and the line's name before the gate.
    for (c, d, len) in [
        (v3(2.6, y, -1.2), Vec3::X, 6.6),
        (v3(2.6, y, 1.2), Vec3::X, 6.6),
        (v3(-0.7, y, 0.0), Vec3::Z, 2.48),
        (v3(5.9, y, 0.0), Vec3::Z, 2.48),
    ] {
        p_line(&mut paint, c, d, len, 0.08, yellow);
    }
    stencil(
        &mut paint,
        PAINT_ZIP,
        v3(-2.4, y, 0.0),
        Vec3::Z,
        Vec3::X,
        0.38,
        white,
    );
    stencil(
        &mut paint,
        PAINT_09,
        v3(-3.5, y, 0.0),
        Vec3::Z,
        Vec3::X,
        0.5,
        paint_color(ORANGE_PAINT),
    );
    gate.add(PAINT, paint);
}

/// A backlit "09 ZIP" sign standing on the gate's crossbeam, readable from both sides.
fn gate_sign(parts: &mut Parts, end: &End) {
    let g = parts.at(true);
    let mut alu = M::new();
    let mut sign = M::new();
    let (_, _, aspect) = sign_rect(SIGN_ZIP);
    let sh = 0.42;
    let sw = sh * aspect;
    let c = v3(0.0, end.top + 0.08 + sh * 0.5, 0.0);
    alu.append(
        &slab(
            v3(-0.05, c.y - sh * 0.5 - 0.03, -sw * 0.5 - 0.03),
            v3(0.05, c.y + sh * 0.5 + 0.03, sw * 0.5 + 0.03),
            0.008,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for z in [-sw * 0.3, sw * 0.3] {
        alu.append(
            &slab(
                v3(-0.03, end.top, z - 0.02),
                v3(0.03, c.y - sh * 0.5 - 0.03, z + 0.02),
                0.003,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    for side in [-1.0f32, 1.0] {
        let normal = Vec3::X * side;
        let right = Vec3::Y.cross(normal).normalize();
        sign_face(&mut sign, SIGN_ZIP, c + normal * 0.052, right, Vec3::Y, sh);
    }
    g.add(ALU, alu);
    g.add(SIGN, sign);
}
