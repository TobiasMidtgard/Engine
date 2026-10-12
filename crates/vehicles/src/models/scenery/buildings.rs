//! Structures shared by the airfield and the spaceport: portal-frame hangars with ribbed
//! cladding and sliding doors, a control tower with a glazed cab, a fire station with its
//! crash tender, a terminal, floodlight masts, windsocks, sign boards and painted text.
//!
//! Every function draws into [`Parts`] in zone-local coordinates; [`Place`] positions a
//! structure's own frame (x across, y up, z back from its front face) in the zone.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Quat, Vec2, Vec3, Vec4};

use super::kit::{Builder, gray, hex};
use super::noise::rand1;
use super::{Parts, Surface, font, tex};

/// Where a structure stands: zone-local origin and yaw about +Y.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub o: Vec3,
    pub r: Quat,
}

impl Place {
    pub fn new(o: Vec3, yaw: f32) -> Self {
        Self {
            o,
            r: Quat::from_rotation_y(yaw),
        }
    }

    /// Zone-local position of a point in the structure's frame.
    pub fn p(&self, x: f32, y: f32, z: f32) -> Vec3 {
        self.o + self.r * Vec3::new(x, y, z)
    }

    /// Zone-local direction of a vector in the structure's frame.
    pub fn v(&self, v: Vec3) -> Vec3 {
        self.r * v
    }
}

// ---------------------------------------------------------------- primitives

/// A vertical wall panel from `p0` to `p1` (seen from the side its normal faces, `p0` is on
/// the left), between heights `y0` and `y1` above the points. UVs: u = meters along the
/// wall's direction (world-aligned so neighbors match), v = height. The bottom edge is
/// darkened by `ao` (0..1), a cheap contact shadow.
pub fn panel(b: &mut Builder, p0: Vec3, p1: Vec3, y0: f32, y1: f32, ao: f32) {
    let d = p1 - p0;
    let len = Vec3::new(d.x, 0.0, d.z).length();
    if len < 1e-4 {
        return;
    }
    let dir = Vec3::new(d.x, 0.0, d.z) / len;
    let n = dir.cross(Vec3::Y);
    let u0 = p0.dot(dir);
    let c = b.color;
    let dark = Vec4::new(c.x * (1.0 - ao), c.y * (1.0 - ao), c.z * (1.0 - ao), c.w);
    let i0 = b.vertex_c(p0 + Vec3::Y * y0, n, Vec2::new(u0, p0.y + y0), dark);
    let i1 = b.vertex_c(p1 + Vec3::Y * y0, n, Vec2::new(u0 + len, p1.y + y0), dark);
    let i2 = b.vertex_c(p1 + Vec3::Y * y1, n, Vec2::new(u0 + len, p1.y + y1), c);
    let i3 = b.vertex_c(p0 + Vec3::Y * y1, n, Vec2::new(u0, p0.y + y1), c);
    b.quad_idx(i0, i1, i2, i3);
}

/// A flat horizontal quad (ring order) facing up or down, UVs = zone (x, z).
pub fn flat(b: &mut Builder, p: [Vec3; 4], up: bool) {
    let n = if up { Vec3::Y } else { Vec3::NEG_Y };
    b.quad_n(p, p.map(|q| Vec2::new(q.x, q.z)), n);
}

/// A rectangle on the ground plane at height `y`: centered on `c` (x, z used), `along` its
/// long axis (unit, horizontal), `hl` × `hw` half sizes. UVs = zone (x, z).
pub fn rect(b: &mut Builder, c: Vec3, along: Vec3, hl: f32, hw: f32, y: f32) {
    let a = Vec3::new(along.x, 0.0, along.z).normalize_or(Vec3::Z);
    let s = Vec3::new(-a.z, 0.0, a.x);
    let c = Vec3::new(c.x, y, c.z);
    flat(
        b,
        [
            c - a * hl - s * hw,
            c + a * hl - s * hw,
            c + a * hl + s * hw,
            c - a * hl + s * hw,
        ],
        true,
    );
}

/// A strip on the ground between two points, `w` wide, at height `y`.
pub fn line(b: &mut Builder, p0: Vec3, p1: Vec3, w: f32, y: f32) {
    let d = p1 - p0;
    let len = Vec3::new(d.x, 0.0, d.z).length();
    if len < 1e-4 {
        return;
    }
    rect(b, (p0 + p1) * 0.5, d, len * 0.5, w * 0.5, y);
}

/// A dashed strip: `dash` painted, `gap` bare, starting with paint at `p0`.
pub fn dashed(b: &mut Builder, p0: Vec3, p1: Vec3, w: f32, y: f32, dash: f32, gap: f32) {
    let len = (p1 - p0).length();
    let dir = (p1 - p0) / len.max(1e-6);
    let mut t = 0.0;
    while t < len {
        let e = (t + dash).min(len);
        line(b, p0 + dir * t, p0 + dir * e, w, y);
        t += dash + gap;
    }
}

/// Glyph quads for `text` on a plane, centered at `c`, reading along `right` with glyph
/// tops toward `up` (unit vectors in the plane), facing `n`; glyphs `h` tall and `w` times
/// the font's width. Use with [`Surface::Glyphs`].
#[allow(clippy::too_many_arguments)]
pub fn text(b: &mut Builder, c: Vec3, right: Vec3, up: Vec3, n: Vec3, txt: &str, h: f32, w: f32) {
    let unit = h / font::HEIGHT;
    let ux = unit * w;
    let count = txt.chars().count() as f32;
    let total = (count - 1.0) * font::ADVANCE * ux + font::WIDTH * ux;
    for (i, ch) in txt.chars().enumerate() {
        let Some(uv) = tex::glyph_uv(ch) else {
            continue;
        };
        let x = -total * 0.5 + i as f32 * font::ADVANCE * ux + font::WIDTH * 0.5 * ux;
        let center = c + right * x;
        let (hw, hh) = (3.0 * ux, 4.0 * unit);
        let p = [
            center - right * hw - up * hh,
            center + right * hw - up * hh,
            center + right * hw + up * hh,
            center - right * hw + up * hh,
        ];
        let t = [
            Vec2::new(uv[0], uv[3]),
            Vec2::new(uv[2], uv[3]),
            Vec2::new(uv[2], uv[1]),
            Vec2::new(uv[0], uv[1]),
        ];
        b.quad_n(p, t, n);
    }
}

/// Width of `txt` drawn by [`text`] at height `h` and width factor `w`.
pub fn text_width(txt: &str, h: f32, w: f32) -> f32 {
    let ux = h / font::HEIGHT * w;
    let count = txt.chars().count() as f32;
    (count - 1.0) * font::ADVANCE * ux + font::WIDTH * ux
}

/// A board from the sign atlas on a vertical quad: bottom-left `p0`, bottom-right `p1` as
/// seen by the reader, `h` tall, facing the reader.
pub fn board(b: &mut Builder, p0: Vec3, p1: Vec3, h: f32, index: usize) {
    let uv = tex::board_uv(index);
    let n = (p1 - p0).cross(Vec3::Y).normalize_or(Vec3::Z);
    b.quad_n(
        [p0, p1, p1 + Vec3::Y * h, p0 + Vec3::Y * h],
        [
            Vec2::new(uv[0], uv[3]),
            Vec2::new(uv[2], uv[3]),
            Vec2::new(uv[2], uv[1]),
            Vec2::new(uv[0], uv[1]),
        ],
        n,
    );
}

/// A cylinder seen from inside (normals in, winding reversed): for the inside of a sock or
/// a pipe mouth.
pub fn inner_cylinder(b: &mut Builder, a: Vec3, c: Vec3, ra: f32, rc: f32, seg: u32) {
    let mut t = Builder::new();
    t.color = b.color;
    t.cylinder(a, c, ra, rc, seg, false);
    let mut m = t.finish();
    for v in &mut m.vertices {
        v.normal = [-v.normal[0], -v.normal[1], -v.normal[2]];
    }
    for tri in m.indices.chunks_mut(3) {
        tri.swap(1, 2);
    }
    b.mesh.merge(&m);
}

/// A railing between points: posts every `spacing`, a top rail, a knee rail and a toe board.
pub fn railing(parts: &mut Parts, pts: &[Vec3], h: f32, spacing: f32) {
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    for w in pts.windows(2) {
        let (a, c) = (w[0], w[1]);
        let len = (c - a).length();
        let n = (len / spacing).ceil().max(1.0) as u32;
        for k in 0..=n {
            let p = a.lerp(c, k as f32 / n as f32);
            g.cylinder(p, p + Vec3::Y * h, 0.024, 0.024, 6, false);
        }
        g.cylinder(a + Vec3::Y * h, c + Vec3::Y * h, 0.025, 0.025, 6, false);
        g.cylinder(
            a + Vec3::Y * h * 0.5,
            c + Vec3::Y * h * 0.5,
            0.018,
            0.018,
            6,
            false,
        );
        g.beam(a + Vec3::Y * 0.06, c + Vec3::Y * 0.06, 0.01, 0.12, 0.0);
    }
}

// ---------------------------------------------------------------- hangar

/// A portal-frame hangar.
#[derive(Clone, Copy, Debug)]
pub struct Hangar {
    /// Width across the doors (x) and depth (z), meters.
    pub w: f32,
    pub d: f32,
    /// Eave height and the roof's rise to the ridge.
    pub eave: f32,
    pub rise: f32,
    /// Door opening height.
    pub door_h: f32,
    /// How far each door group is open (0 closed, 1 fully stacked): left, right as seen
    /// from outside.
    pub open: (f32, f32),
    /// Wall cladding, door and trim colors (linear).
    pub wall: Vec4,
    pub door: Vec4,
    pub trim: Vec4,
    /// Sponsor or operator board over the doors (sign atlas index).
    pub board: Option<usize>,
    /// Floor slab top above the ground.
    pub floor: f32,
}

/// Draw a hangar whose doors face −z of `at` (the front face at z = 0, back at z = `d`).
/// Returns zone-local positions for interior lights.
pub fn hangar(parts: &mut Parts, at: Place, h: &Hangar) -> Vec<Vec3> {
    let hw = h.w * 0.5;
    let (d, eave, ridge) = (h.d, h.eave, h.eave + h.rise);
    let jamb = 0.8f32;
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let mut lights = Vec::new();

    // Floor slab with a chamfered edge, an epoxy floor inside and the door track.
    {
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.95));
        c.chamfer_box(
            p(0.0, (h.floor - 0.3) * 0.5, d * 0.5 - 1.0),
            Vec3::new(hw + 0.4, (h.floor + 0.3) * 0.5, d * 0.5 + 1.4),
            0.05,
            at.r,
        );
        // Plinth: a cast concrete dado along the walls.
        for (a, b) in [
            (p(-hw, h.floor, d), p(-hw, h.floor, 0.0)),
            (p(hw, h.floor, 0.0), p(hw, h.floor, d)),
            (p(hw, h.floor, d), p(-hw, h.floor, d)),
        ] {
            let mid = (a + b) * 0.5;
            let len = (b - a).length();
            let dir = (b - a) / len;
            let rot = facing_z(dir);
            c.chamfer_box(
                mid + Vec3::Y * 0.3,
                Vec3::new(0.12, 0.3, len * 0.5 + 0.12),
                0.02,
                rot,
            );
        }
        let f = parts.get(Surface::Painted);
        f.color(hex(0x8d958f));
        flat(
            f,
            [
                p(-hw + 0.2, h.floor + 0.004, 0.3),
                p(hw - 0.2, h.floor + 0.004, 0.3),
                p(hw - 0.2, h.floor + 0.004, d - 0.2),
                p(-hw + 0.2, h.floor + 0.004, d - 0.2),
            ],
            true,
        );
        // Yellow safety line inside the door line.
        let paint = parts.get(Surface::Plastic);
        paint.color(hex(0xd8a70e));
        flat(
            paint,
            [
                p(-hw + 0.3, h.floor + 0.007, 1.2),
                p(hw - 0.3, h.floor + 0.007, 1.2),
                p(hw - 0.3, h.floor + 0.007, 1.35),
                p(-hw + 0.3, h.floor + 0.007, 1.35),
            ],
            true,
        );
    }

    // Steel portal frames, purlins and side rails (seen through the open doors).
    let bays = (d / 6.0).round().max(2.0) as u32;
    {
        let s = parts.get(Surface::Painted);
        s.color(h.trim);
        for k in 0..=bays {
            let z = 0.35 + (d - 0.7) * k as f32 / bays as f32;
            for sx in [-1.0f32, 1.0] {
                let x = sx * (hw - 0.35);
                s.beam(p(x, h.floor, z), p(x, eave - 0.2, z), 0.3, 0.5, 0.01);
                s.beam(p(x, eave - 0.6, z), p(0.0, ridge - 0.5, z), 0.22, 0.6, 0.01);
                // Haunch.
                s.beam(
                    p(x, eave - 1.6, z),
                    p(x * 0.86, eave - 0.3, z),
                    0.2,
                    0.25,
                    0.0,
                );
            }
        }
        let purlins = 7;
        for i in 0..=purlins {
            let t = i as f32 / purlins as f32;
            for sx in [-1.0f32, 1.0] {
                let x = sx * (hw - 0.4) * (1.0 - t);
                let y = mix(eave - 0.25, ridge - 0.25, t);
                s.beam(p(x, y, 0.2), p(x, y, d - 0.2), 0.1, 0.18, 0.0);
            }
        }
        // Door head beam.
        s.beam(
            p(-hw, h.door_h + 0.35, 0.15),
            p(hw, h.door_h + 0.35, 0.15),
            0.4,
            0.7,
            0.02,
        );
    }

    // Walls: ribbed cladding outside, liner panels inside.
    {
        let w = parts.get(Surface::Cladding);
        w.color(h.wall);
        let y0 = h.floor + 0.6;
        let top = eave - h.floor;
        // Side walls (outer faces), back wall, and the front above and beside the doors.
        panel(w, p(-hw, h.floor, 0.0), p(-hw, h.floor, d), 0.6, top, 0.15);
        panel(w, p(hw, h.floor, d), p(hw, h.floor, 0.0), 0.6, top, 0.15);
        panel(w, p(-hw, h.floor, d), p(hw, h.floor, d), 0.6, top, 0.15);
        panel(
            w,
            p(-hw + jamb, h.floor, 0.0),
            p(-hw, h.floor, 0.0),
            0.6,
            top,
            0.15,
        );
        panel(
            w,
            p(hw, h.floor, 0.0),
            p(hw - jamb, h.floor, 0.0),
            0.6,
            top,
            0.15,
        );
        panel(
            w,
            p(hw - jamb, h.floor, 0.0),
            p(-hw + jamb, h.floor, 0.0),
            h.door_h - h.floor + 0.7,
            top,
            0.0,
        );
        // Gables front and back.
        for (z, flip) in [(0.0f32, false), (d, true)] {
            let (a, c) = if flip {
                (p(-hw, eave, z), p(hw, eave, z))
            } else {
                (p(hw, eave, z), p(-hw, eave, z))
            };
            let top_p = p(0.0, ridge, z);
            let n = (c - a).cross(Vec3::Y).normalize();
            let dir = (c - a).normalize();
            let uv = |q: Vec3| Vec2::new(q.dot(dir), q.y);
            let i0 = w.vertex(a, n, uv(a));
            let i1 = w.vertex(c, n, uv(c));
            let i2 = w.vertex(top_p, n, uv(top_p));
            w.tri_idx(i0, i1, i2);
        }
        let _ = y0;
        // Liner inside (light gray), only where it can be seen through the doors.
        let l = parts.get(Surface::Painted);
        l.color(gray(0.62));
        let inset = 0.12;
        panel(
            l,
            p(-hw + inset, h.floor, d - inset),
            p(-hw + inset, h.floor, 0.0),
            0.0,
            top,
            0.35,
        );
        panel(
            l,
            p(hw - inset, h.floor, 0.0),
            p(hw - inset, h.floor, d - inset),
            0.0,
            top,
            0.35,
        );
        panel(
            l,
            p(hw - inset, h.floor, d - inset),
            p(-hw + inset, h.floor, d - inset),
            0.0,
            top,
            0.35,
        );
        // Inside of the gables and the door header.
        for z in [inset, d - inset] {
            let (a, c) = if z < 1.0 {
                (p(-hw, eave, z), p(hw, eave, z))
            } else {
                (p(hw, eave, z), p(-hw, eave, z))
            };
            let top_p = p(0.0, ridge, z);
            let n = (c - a).cross(Vec3::Y).normalize();
            let i0 = l.vertex(a, n, Vec2::ZERO);
            let i1 = l.vertex(c, n, Vec2::new(1.0, 0.0));
            let i2 = l.vertex(top_p, n, Vec2::new(0.5, 1.0));
            l.tri_idx(i0, i1, i2);
        }
        panel(
            l,
            p(-hw + jamb, h.floor, inset),
            p(hw - jamb, h.floor, inset),
            h.door_h - h.floor + 0.7,
            top,
            0.0,
        );
        // Roof underside.
        let under = |sx: f32| -> [Vec3; 4] {
            [
                p(sx * (hw - inset), eave - 0.05, 0.0),
                p(sx * (hw - inset), eave - 0.05, d),
                p(0.0, ridge - 0.05, d),
                p(0.0, ridge - 0.05, 0.0),
            ]
        };
        for sx in [-1.0f32, 1.0] {
            let q = under(sx);
            let n = (q[1] - q[0]).cross(q[3] - q[0]).normalize();
            let n = if n.y > 0.0 { -n } else { n };
            l.color(gray(0.45));
            l.quad_n(q, q.map(|v| Vec2::new(v.x, v.z)), n);
        }
    }

    // Roof: ribbed sheets down each slope, ridge cap, fascia, gutters, roof lights.
    {
        let over = 0.45f32;
        let slope = h.rise / hw;
        let r = parts.get(Surface::Cladding);
        r.color(gray(0.8));
        for sx in [-1.0f32, 1.0] {
            let e0 = p(sx * (hw + over), eave - over * slope, -over);
            let e1 = p(sx * (hw + over), eave - over * slope, d + over);
            let r1 = p(0.0, ridge, d + over);
            let r0 = p(0.0, ridge, -over);
            let span = (r0 - e0).length();
            let n = (e1 - e0).cross(r0 - e0).normalize();
            let n = if n.y < 0.0 { -n } else { n };
            r.quad_n(
                [e0, e1, r1, r0],
                [
                    Vec2::new(-over, 0.0),
                    Vec2::new(d + over, 0.0),
                    Vec2::new(d + over, span),
                    Vec2::new(-over, span),
                ],
                n,
            );
        }
        let t = parts.get(Surface::Painted);
        t.color(h.trim);
        t.beam(
            p(0.0, ridge + 0.05, -over - 0.05),
            p(0.0, ridge + 0.05, d + over + 0.05),
            0.5,
            0.12,
            0.02,
        );
        for sx in [-1.0f32, 1.0] {
            // Barge boards along the gables.
            for z in [-over - 0.02, d + over + 0.02] {
                t.beam(
                    p(sx * (hw + over), eave - over * slope - 0.1, z),
                    p(0.0, ridge - 0.1, z),
                    0.06,
                    0.32,
                    0.0,
                );
            }
            // Gutter and downpipes.
            let gx = sx * (hw + over + 0.1);
            t.beam(
                p(gx, eave - over * slope - 0.15, -over),
                p(gx, eave - over * slope - 0.15, d + over),
                0.22,
                0.2,
                0.02,
            );
            for z in [0.6, d * 0.5, d - 0.6] {
                let top_p = p(gx, eave - over * slope - 0.2, z);
                let wall_p = p(sx * (hw + 0.1), h.floor + 0.15, z);
                t.cylinder(
                    Vec3::new(wall_p.x, wall_p.y, wall_p.z),
                    Vec3::new(wall_p.x, top_p.y - 0.4, wall_p.z),
                    0.055,
                    0.055,
                    8,
                    false,
                );
                t.cylinder(
                    Vec3::new(wall_p.x, top_p.y - 0.4, wall_p.z),
                    top_p,
                    0.055,
                    0.055,
                    8,
                    false,
                );
            }
        }
        // Corner flashings.
        for (x, z) in [(-hw, 0.0), (hw, 0.0), (-hw, d), (hw, d)] {
            t.chamfer_box(
                p(x, (h.floor + eave) * 0.5, z),
                Vec3::new(0.09, (eave - h.floor) * 0.5, 0.09),
                0.01,
                at.r,
            );
        }
        // Translucent roof lights: strips on each slope.
        let rl = parts.get(Surface::Plastic);
        rl.color(Vec4::new(0.75, 0.78, 0.74, 1.0));
        for sx in [-1.0f32, 1.0] {
            for k in 0..3 {
                let z0 = d * (0.15 + k as f32 * 0.3);
                let z1 = z0 + d * 0.1;
                let (t0, t1) = (0.35f32, 0.7f32);
                let q = |t: f32, z: f32| {
                    let x = sx * hw * (1.0 - t);
                    p(x, eave + h.rise * t + 0.04, z)
                };
                let quad = [q(t0, z0), q(t0, z1), q(t1, z1), q(t1, z0)];
                let n = (quad[1] - quad[0]).cross(quad[3] - quad[0]).normalize();
                let n = if n.y < 0.0 { -n } else { n };
                rl.quad_n(quad, quad.map(|v| Vec2::new(v.x, v.z)), n);
            }
        }
        // Obstruction lights on the ridge ends.
        let red = parts.get(Surface::LensRed);
        red.color(Vec4::ONE);
        for z in [-over, d + over] {
            red.sphere(p(0.0, ridge + 0.32, z), 0.12, 10, 1.0);
        }
        let t = parts.get(Surface::Painted);
        t.color(gray(0.1));
        for z in [-over, d + over] {
            t.cylinder(
                p(0.0, ridge + 0.1, z),
                p(0.0, ridge + 0.22, z),
                0.08,
                0.08,
                8,
                true,
            );
        }
    }

    // Sliding doors: six leaves on three tracks, opening from the middle.
    {
        let opening = h.w - 2.0 * jamb;
        let leaves = 6usize;
        let lw = opening / leaves as f32 + 0.12;
        let dh = h.door_h - h.floor;
        // Fully open, every leaf of a group stands beside the building, past the jamb.
        let reach = hw + lw + 0.4;
        let r = parts.get(Surface::Galvanized);
        r.color(Vec4::ONE);
        // Floor tracks and the top guide run out to the door stacks.
        for z in [-0.18f32, -0.36, -0.54] {
            r.beam(
                p(-reach, h.floor + 0.015, z),
                p(reach, h.floor + 0.015, z),
                0.08,
                0.03,
                0.0,
            );
        }
        let g = parts.get(Surface::Painted);
        g.color(h.trim);
        g.beam(
            p(-reach, h.door_h + 0.25, -0.36),
            p(reach, h.door_h + 0.25, -0.36),
            0.62,
            0.3,
            0.02,
        );
        // Door-stack posts beyond the walls, braced back to the building.
        for sx in [-1.0f32, 1.0] {
            let x = sx * reach;
            g.beam(
                p(x, h.floor, -0.36),
                p(x, h.door_h + 0.25, -0.36),
                0.2,
                0.2,
                0.01,
            );
            g.beam(
                p(x, h.door_h + 0.1, -0.36),
                p(sx * hw, h.door_h + 0.1, 0.0),
                0.12,
                0.12,
                0.0,
            );
        }
        for i in 0..leaves {
            let left = i < leaves / 2;
            let closed_x = -hw + jamb + (i as f32 + 0.5) * (opening / leaves as f32);
            // k = 0 is the leaf next to the jamb (outermost track).
            let k = if left { i } else { leaves - 1 - i };
            let track = [-0.18f32, -0.36, -0.54][k.min(2)];
            let sx = if left { -1.0 } else { 1.0 };
            let stack_x = sx * (hw + lw * 0.5 + 0.2);
            let f = if left { h.open.0 } else { h.open.1 };
            let x = mix(closed_x, stack_x, f.clamp(0.0, 1.0));
            let c = p(x, h.floor + dh * 0.5 + 0.04, track);
            let w = parts.get(Surface::Cladding);
            w.color(h.door);
            w.chamfer_box(c, Vec3::new(lw * 0.5, dh * 0.5 - 0.04, 0.06), 0.02, at.r);
            let g = parts.get(Surface::Painted);
            g.color(h.trim);
            // Bottom rail, top rail and stiles.
            g.chamfer_box(
                p(x, h.floor + 0.2, track - 0.065),
                Vec3::new(lw * 0.5, 0.2, 0.02),
                0.01,
                at.r,
            );
            g.chamfer_box(
                p(x, h.door_h - 0.12, track - 0.065),
                Vec3::new(lw * 0.5, 0.12, 0.02),
                0.01,
                at.r,
            );
            for sx in [-1.0f32, 1.0] {
                g.chamfer_box(
                    p(
                        x + sx * (lw * 0.5 - 0.06),
                        h.floor + dh * 0.5,
                        track - 0.065,
                    ),
                    Vec3::new(0.06, dh * 0.5, 0.02),
                    0.01,
                    at.r,
                );
            }
            // A wicket door in the middle leaf on the left.
            if i == 2 {
                g.color(gray(0.18));
                g.chamfer_box(
                    p(x, h.floor + 1.05, track - 0.07),
                    Vec3::new(0.48, 1.05, 0.015),
                    0.01,
                    at.r,
                );
                let gl = parts.get(Surface::Glass);
                gl.color(Vec4::ONE);
                let wy = h.floor + 1.55;
                gl.quad_n(
                    [
                        p(x - 0.2, wy, track - 0.09),
                        p(x + 0.2, wy, track - 0.09),
                        p(x + 0.2, wy + 0.4, track - 0.09),
                        p(x - 0.2, wy + 0.4, track - 0.09),
                    ],
                    [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
                    at.v(Vec3::NEG_Z),
                );
            }
        }
    }

    // Personnel door and windows on the side walls, high-level vents.
    {
        for sx in [-1.0f32, 1.0] {
            let x = sx * (hw + 0.02);
            let n = at.v(Vec3::X * sx);
            let g = parts.get(Surface::Painted);
            g.color(gray(0.2));
            let zc = d * 0.3;
            let dc = p(x, h.floor + 1.05, zc);
            let side = at.v(Vec3::Z);
            g.quad_n(
                [
                    dc - side * 0.5 - Vec3::Y * 1.05,
                    dc + side * 0.5 - Vec3::Y * 1.05,
                    dc + side * 0.5 + Vec3::Y * 1.05,
                    dc - side * 0.5 + Vec3::Y * 1.05,
                ],
                [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
                n,
            );
            g.color(h.trim);
            // Canopy over the door.
            g.chamfer_box(
                dc + n * 0.45 + Vec3::Y * 1.3,
                Vec3::new(0.45, 0.04, 0.8),
                0.01,
                at.r,
            );
            // A row of windows.
            let wy = h.floor + 2.2;
            for k in 0..3 {
                let z = d * 0.5 + k as f32 * 2.4;
                if z + 0.8 > d - 0.5 {
                    continue;
                }
                let gl = parts.get(Surface::Glass);
                gl.color(Vec4::ONE);
                let c = p(x + sx * 0.01, wy + 0.5, z);
                gl.quad_n(
                    [
                        c - side * 0.8 - Vec3::Y * 0.5,
                        c + side * 0.8 - Vec3::Y * 0.5,
                        c + side * 0.8 + Vec3::Y * 0.5,
                        c - side * 0.8 + Vec3::Y * 0.5,
                    ],
                    [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
                    n,
                );
                let f = parts.get(Surface::Painted);
                f.color(gray(0.25));
                for (dy, hh, dz, hz) in [
                    (0.55, 0.04, 0.0, 0.84),
                    (-0.55, 0.06, 0.0, 0.86),
                    (0.0, 0.5, 0.82, 0.04),
                    (0.0, 0.5, -0.82, 0.04),
                ] {
                    f.chamfer_box(
                        c + Vec3::Y * dy + side * dz + n * 0.02,
                        Vec3::new(0.04, hh, hz),
                        0.0,
                        at.r,
                    );
                }
                // Dark room behind the glass.
                let i = parts.get(Surface::Interior);
                i.color(Vec4::ONE);
                i.quad_n(
                    [
                        c - side * 0.8 - Vec3::Y * 0.5 - n * 0.06,
                        c + side * 0.8 - Vec3::Y * 0.5 - n * 0.06,
                        c + side * 0.8 + Vec3::Y * 0.5 - n * 0.06,
                        c - side * 0.8 + Vec3::Y * 0.5 - n * 0.06,
                    ],
                    [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
                    n,
                );
            }
        }
    }

    // Board over the doors.
    if let Some(index) = h.board {
        let bw = (h.w * 0.4).min(16.0);
        let y = h.door_h + 0.9;
        let bh = ((eave - y) * 0.85).min(bw / 4.0);
        let s = parts.get(Surface::Signs);
        s.color(Vec4::ONE);
        board(s, p(bw * 0.5, y, -0.06), p(-bw * 0.5, y, -0.06), bh, index);
    }

    // Inside: high-bay lamps, cabinets along the back, a workbench.
    {
        let l = parts.get(Surface::Lamp);
        l.color(Vec4::ONE);
        for row in [-0.3f32, 0.3] {
            for k in 0..3 {
                let z = d * (0.2 + 0.3 * k as f32);
                let c = p(row * h.w, eave - 0.6, z);
                l.cylinder(c, c - Vec3::Y * 0.05, 0.22, 0.22, 12, true);
                lights.push(c - Vec3::Y * 0.3);
            }
        }
        let g = parts.get(Surface::Painted);
        g.color(gray(0.12));
        for row in [-0.3f32, 0.3] {
            for k in 0..3 {
                let z = d * (0.2 + 0.3 * k as f32);
                let c = p(row * h.w, eave - 0.6, z);
                g.cylinder(c + Vec3::Y * 0.05, c + Vec3::Y * 0.3, 0.25, 0.12, 12, false);
                g.cylinder(c + Vec3::Y * 0.3, c + Vec3::Y * 1.0, 0.01, 0.01, 4, false);
            }
        }
        let n_cab = ((h.w - 4.0) / 1.2) as i32;
        for k in 0..n_cab {
            let x = -hw + 2.0 + k as f32 * 1.2;
            if (x).abs() < 3.0 {
                continue;
            }
            let red = rand1(k, 7) > 0.5;
            g.color(if red { hex(0xa3271c) } else { hex(0x2b4a73) });
            g.chamfer_box(
                p(x, h.floor + 0.9, d - 0.55),
                Vec3::new(0.55, 0.9, 0.3),
                0.02,
                at.r,
            );
        }
        g.color(gray(0.3));
        g.chamfer_box(
            p(0.0, h.floor + 0.45, d - 1.0),
            Vec3::new(2.0, 0.45, 0.4),
            0.02,
            at.r,
        );
    }
    lights
}

// ---------------------------------------------------------------- control tower

/// A control tower: a two-storey base building, a square concrete shaft with a stair window,
/// an equipment floor and an eight-sided cab with outward-leaning glass, a catwalk, and roof
/// masts. `cab_floor` is the cab's floor height; the base building extends along +x.
pub fn tower(parts: &mut Parts, at: Place, cab_floor: f32) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let h = cab_floor;
    // Base building: 12 × 9 m, two storeys, ribbon windows.
    {
        let c = parts.get(Surface::Concrete);
        c.color(gray(1.0));
        c.chamfer_box(p(5.0, 0.15, 0.0), Vec3::new(7.4, 0.15, 5.0), 0.04, at.r);
        let (x0, x1, z0, z1, top) = (-1.0f32, 11.0f32, -4.5f32, 4.5f32, 7.2f32);
        let corners = [
            p(x0, 0.3, z0),
            p(x1, 0.3, z0),
            p(x1, 0.3, z1),
            p(x0, 0.3, z1),
        ];
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            // Outward = right of travel for this clockwise-from-above ring, so swap.
            panel(c, b, a, 0.0, 0.9, 0.25);
            panel(c, b, a, 2.3, 4.2, 0.0);
            panel(c, b, a, 5.6, top, 0.0);
        }
        let roof = [
            p(x0, top + 0.3, z0),
            p(x1, top + 0.3, z0),
            p(x1, top + 0.3, z1),
            p(x0, top + 0.3, z1),
        ];
        flat(c, roof, true);
        // Parapet coping.
        let g = parts.get(Surface::Painted);
        g.color(gray(0.3));
        for k in 0..4 {
            let (a, b) = (roof[k], roof[(k + 1) % 4]);
            g.beam(a + Vec3::Y * 0.1, b + Vec3::Y * 0.1, 0.3, 0.2, 0.01);
        }
        // Ribbon windows with mullions and dark rooms behind.
        for k in 0..4 {
            let (a, b) = (corners[(k + 1) % 4], corners[k]);
            for (y0, y1) in [(0.9f32, 2.3f32), (4.2, 5.6)] {
                glazing(parts, a, b, y0, y1, 1.5, 0.12);
            }
        }
        // Entrance canopy and door.
        let g = parts.get(Surface::Painted);
        g.color(gray(0.22));
        g.chamfer_box(p(6.0, 2.6, -5.4), Vec3::new(2.2, 0.08, 1.0), 0.02, at.r);
        // Rooftop plant.
        g.color(gray(0.55));
        g.chamfer_box(p(8.5, top + 1.0, 2.0), Vec3::new(1.2, 0.7, 1.0), 0.04, at.r);
        g.chamfer_box(
            p(8.5, top + 0.9, -2.0),
            Vec3::new(0.8, 0.6, 0.8),
            0.04,
            at.r,
        );
    }
    // Shaft.
    let sh = 2.3f32;
    {
        let corners = [
            p(-sh, 0.3, -sh),
            p(sh, 0.3, -sh),
            p(sh, 0.3, sh),
            p(-sh, 0.3, sh),
        ];
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            let c = parts.get(Surface::Concrete);
            c.color(gray(0.97));
            if k == 3 {
                // The stair window runs up the west face.
                let m0 = a.lerp(b, 0.35);
                let m1 = a.lerp(b, 0.65);
                panel(c, b, m1, 0.0, h - 3.3, 0.25);
                panel(c, m0, a, 0.0, h - 3.3, 0.25);
                panel(c, m1, m0, 0.0, 1.0, 0.25);
                glazing(parts, m1, m0, 1.0, h - 3.6, 0.9, 0.1);
                let c = parts.get(Surface::Concrete);
                panel(c, m1, m0, h - 3.6, h - 3.3, 0.0);
            } else {
                panel(c, b, a, 0.0, h - 3.3, 0.25);
            }
        }
        // Horizontal bands (formwork joints) every 3 m.
        let g = parts.get(Surface::Painted);
        g.color(gray(0.5));
        let mut y = 3.0;
        while y < h - 3.5 {
            for k in 0..4 {
                let (a, b) = (corners[k], corners[(k + 1) % 4]);
                g.beam(a + Vec3::Y * y, b + Vec3::Y * y, 0.03, 0.05, 0.0);
            }
            y += 3.0;
        }
    }
    // Equipment floor: a wider box under the cab with louvres.
    {
        let e = 3.2f32;
        let c = parts.get(Surface::Cladding);
        c.color(hex(0xc9ccce));
        let corners = [
            p(-e, h - 3.0, -e),
            p(e, h - 3.0, -e),
            p(e, h - 3.0, e),
            p(-e, h - 3.0, e),
        ];
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            panel(c, b, a, 0.0, 3.0, 0.0);
        }
        let g = parts.get(Surface::Painted);
        g.color(gray(0.25));
        flat(
            g,
            [
                p(-e, h - 3.0, -e),
                p(e, h - 3.0, -e),
                p(e, h - 3.0, e),
                p(-e, h - 3.0, e),
            ],
            false,
        );
        for k in 0..4 {
            let (a, b) = (corners[(k + 1) % 4], corners[k]);
            glazing(parts, a.lerp(b, 0.2), a.lerp(b, 0.45), 1.0, 2.2, 0.8, 0.08);
            let g = parts.get(Surface::Painted);
            g.color(gray(0.3));
            g.beam(a + Vec3::Y * 0.02, b + Vec3::Y * 0.02, 0.12, 0.08, 0.0);
            g.beam(a + Vec3::Y * 2.98, b + Vec3::Y * 2.98, 0.12, 0.08, 0.0);
        }
    }
    // Cab: eight sides, glass leaning out 15°, catwalk, roof.
    {
        let r0 = 3.9f32; // apothem at the sill
        let lean = 15f32.to_radians();
        let sill = 1.0f32;
        let glass_h = 3.0f32;
        let r1 = r0 + glass_h * lean.tan();
        let corner = |r: f32, k: usize, y: f32| -> Vec3 {
            let a = (k as f32 + 0.5) * TAU / 8.0;
            let rc = r / (PI / 8.0).cos();
            p(a.cos() * rc, y, a.sin() * rc)
        };
        // Floor slab and catwalk deck.
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.9));
        let ring = |r: f32, y: f32| -> Vec<Vec3> { (0..8).map(|k| corner(r, k, y)).collect() };
        let deck_r = r0 + 1.25;
        let top = ring(deck_r, h + 0.0);
        let bot = ring(deck_r, h - 0.35);
        for k in 0..8 {
            let (a, b) = (top[k], top[(k + 1) % 8]);
            let (a2, b2) = (bot[k], bot[(k + 1) % 8]);
            let n = ((a + b) * 0.5 - p(0.0, h, 0.0)).with_y(0.0).normalize();
            c.quad_n([a2, b2, b, a], [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y], n);
            let center = p(0.0, h, 0.0);
            let i0 = c.vertex(center, Vec3::Y, Vec2::ZERO);
            let i1 = c.vertex(a, Vec3::Y, Vec2::new(a.x, a.z));
            let i2 = c.vertex(b, Vec3::Y, Vec2::new(b.x, b.z));
            c.tri_idx(i0, i1, i2);
            let center = p(0.0, h - 0.35, 0.0);
            let i0 = c.vertex(center, Vec3::NEG_Y, Vec2::ZERO);
            let i1 = c.vertex(a2, Vec3::NEG_Y, Vec2::new(a2.x, a2.z));
            let i2 = c.vertex(b2, Vec3::NEG_Y, Vec2::new(b2.x, b2.z));
            c.tri_idx(i0, i1, i2);
        }
        // Railing round the catwalk.
        let mut rail: Vec<Vec3> = ring(deck_r - 0.08, h).into_iter().collect();
        rail.push(rail[0]);
        railing(parts, &rail, 1.1, 1.4);
        // Sill wall.
        let s = parts.get(Surface::Painted);
        s.color(gray(0.16));
        let lo = ring(r0, h);
        let hi = ring(r0, h + sill);
        for k in 0..8 {
            let (a, b) = (lo[k], lo[(k + 1) % 8]);
            let (a1, b1) = (hi[k], hi[(k + 1) % 8]);
            let n = ((a + b) * 0.5 - p(0.0, h, 0.0)).with_y(0.0).normalize();
            s.quad_n([a, b, b1, a1], [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y], n);
        }
        // Glass, mullions, and the consoles inside.
        let g0 = ring(r0, h + sill);
        let g1 = ring(r1, h + sill + glass_h);
        let gl = parts.get(Surface::Glass);
        gl.color(Vec4::ONE);
        for k in 0..8 {
            let (a, b) = (g0[k], g0[(k + 1) % 8]);
            let (a1, b1) = (g1[k], g1[(k + 1) % 8]);
            let n = (b - a).cross(a1 - a).normalize();
            let out = ((a + b) * 0.5 - p(0.0, h, 0.0)).with_y(0.0).normalize();
            let n = if n.dot(out) < 0.0 { -n } else { n };
            gl.quad_n([a, b, b1, a1], [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y], n);
        }
        let m = parts.get(Surface::Painted);
        m.color(gray(0.12));
        for k in 0..8 {
            m.beam(g0[k], g1[k], 0.12, 0.12, 0.0);
            m.beam(g0[k], g0[(k + 1) % 8], 0.1, 0.1, 0.0);
            m.beam(g1[k], g1[(k + 1) % 8], 0.1, 0.14, 0.0);
        }
        let con = parts.get(Surface::Interior);
        con.color(Vec4::ONE);
        for k in 0..8 {
            if k == 4 {
                continue; // stair hatch side
            }
            let a = (k as f32 + 1.0) * TAU / 8.0;
            let c = p(a.cos() * (r0 - 0.6), h + 0.5, a.sin() * (r0 - 0.6));
            let yaw = -a + PI * 0.5;
            con.chamfer_box(
                c,
                Vec3::new(1.2, 0.5, 0.35),
                0.03,
                at.r * Quat::from_rotation_y(yaw),
            );
        }
        let scr = parts.get(Surface::Screen);
        scr.color(Vec4::new(0.25, 0.35, 0.4, 1.0));
        for k in 0..8 {
            if k == 4 {
                continue;
            }
            let a = (k as f32 + 1.0) * TAU / 8.0;
            let c = p(a.cos() * (r0 - 0.85), h + 1.2, a.sin() * (r0 - 0.85));
            let yaw = -a + PI * 0.5;
            scr.chamfer_box(
                c,
                Vec3::new(0.5, 0.18, 0.03),
                0.0,
                at.r * Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-0.25),
            );
        }
        // Ceiling and roof slab with overhang.
        let ceil = parts.get(Surface::Painted);
        ceil.color(gray(0.7));
        let cr = ring(r1, h + sill + glass_h);
        for k in 0..8 {
            let center = p(0.0, h + sill + glass_h, 0.0);
            let i0 = ceil.vertex(center, Vec3::NEG_Y, Vec2::ZERO);
            let i1 = ceil.vertex(cr[k], Vec3::NEG_Y, Vec2::X);
            let i2 = ceil.vertex(cr[(k + 1) % 8], Vec3::NEG_Y, Vec2::Y);
            ceil.tri_idx(i0, i1, i2);
        }
        let roof_y = h + sill + glass_h;
        let c = parts.get(Surface::Painted);
        c.color(gray(0.82));
        let rt = ring(r1 + 0.7, roof_y + 0.55);
        let rb = ring(r1 + 0.7, roof_y);
        for k in 0..8 {
            let (a, b) = (rt[k], rt[(k + 1) % 8]);
            let (a2, b2) = (rb[k], rb[(k + 1) % 8]);
            let n = ((a + b) * 0.5 - p(0.0, roof_y, 0.0))
                .with_y(0.0)
                .normalize();
            c.quad_n([a2, b2, b, a], [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y], n);
            let center = p(0.0, roof_y + 0.65, 0.0);
            let i0 = c.vertex(center, Vec3::Y, Vec2::ZERO);
            let i1 = c.vertex(a, Vec3::Y, Vec2::X);
            let i2 = c.vertex(b, Vec3::Y, Vec2::Y);
            c.tri_idx(i0, i1, i2);
            let center = p(0.0, roof_y, 0.0);
            let i0 = c.vertex(center, Vec3::NEG_Y, Vec2::ZERO);
            let i1 = c.vertex(a2, Vec3::NEG_Y, Vec2::X);
            let i2 = c.vertex(b2, Vec3::NEG_Y, Vec2::Y);
            c.tri_idx(i0, i1, i2);
        }
        // Roof masts: antenna with obstruction light, a radome, a wind sensor.
        let top_y = roof_y + 0.6;
        let g = parts.get(Surface::Galvanized);
        g.color(Vec4::ONE);
        g.cylinder(
            p(0.8, top_y, 0.5),
            p(0.8, top_y + 6.5, 0.5),
            0.07,
            0.04,
            8,
            false,
        );
        for k in 0..3 {
            let y = top_y + 2.0 + k as f32 * 1.6;
            g.cylinder(p(0.8, y, 0.5), p(1.6, y, 0.5), 0.02, 0.02, 4, false);
        }
        g.cylinder(
            p(-1.5, top_y, -1.0),
            p(-1.5, top_y + 3.0, -1.0),
            0.05,
            0.05,
            8,
            false,
        );
        g.cylinder(
            p(-1.5, top_y + 3.0, -1.0),
            p(-0.6, top_y + 3.0, -1.0),
            0.02,
            0.02,
            4,
            false,
        );
        let w = parts.get(Surface::Plastic);
        w.color(gray(0.85));
        w.sphere(p(-1.4, top_y + 0.9, 1.6), 0.8, 16, 1.0);
        w.cylinder(
            p(-1.4, top_y, 1.6),
            p(-1.4, top_y + 0.4, 1.6),
            0.4,
            0.4,
            12,
            false,
        );
        w.cylinder(
            p(-0.6, top_y + 3.0, -1.0),
            p(-0.6, top_y + 3.2, -1.0),
            0.06,
            0.02,
            8,
            true,
        );
        let red = parts.get(Surface::LensRed);
        red.color(Vec4::ONE);
        red.sphere(p(0.8, top_y + 6.65, 0.5), 0.12, 10, 1.0);
    }
}

/// A run of windows from `a` to `b` (seen from outside, `a` on the left) between heights
/// `y0` and `y1`: glass, aluminium mullions every `spacing` meters, a sill, and a dark room
/// behind. `inset` is how far the glass sits behind the wall face.
pub fn glazing(parts: &mut Parts, a: Vec3, b: Vec3, y0: f32, y1: f32, spacing: f32, inset: f32) {
    let len = (b - a).length();
    if len < 0.1 {
        return;
    }
    let dir = (b - a) / len;
    let n = dir.cross(Vec3::Y).normalize();
    let a_in = a - n * inset;
    let b_in = b - n * inset;
    let gl = parts.get(Surface::Glass);
    gl.color(Vec4::ONE);
    gl.quad_n(
        [
            a_in + Vec3::Y * y0,
            b_in + Vec3::Y * y0,
            b_in + Vec3::Y * y1,
            a_in + Vec3::Y * y1,
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        n,
    );
    let room = parts.get(Surface::Interior);
    room.color(Vec4::ONE);
    let depth = 0.8;
    room.quad_n(
        [
            a_in - n * depth + Vec3::Y * y0,
            b_in - n * depth + Vec3::Y * y0,
            b_in - n * depth + Vec3::Y * y1,
            a_in - n * depth + Vec3::Y * y1,
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        n,
    );
    // Reveals: the wall's thickness around the opening.
    let c = parts.get(Surface::Painted);
    c.color(gray(0.5));
    for (p0, p1, nn) in [(a, a_in, dir), (b_in, b, -dir)] {
        c.quad_n(
            [
                p0 + Vec3::Y * y0,
                p1 + Vec3::Y * y0,
                p1 + Vec3::Y * y1,
                p0 + Vec3::Y * y1,
            ],
            [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            nn,
        );
    }
    c.quad_n(
        [
            a + Vec3::Y * y1,
            b + Vec3::Y * y1,
            b_in + Vec3::Y * y1,
            a_in + Vec3::Y * y1,
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        Vec3::NEG_Y,
    );
    let m = parts.get(Surface::Painted);
    m.color(gray(0.18));
    let count = (len / spacing).round().max(1.0) as u32;
    for k in 0..=count {
        let p = a_in.lerp(b_in, k as f32 / count as f32) + n * 0.02;
        m.beam(p + Vec3::Y * y0, p + Vec3::Y * y1, 0.06, 0.08, 0.0);
    }
    m.beam(
        a_in + n * 0.02 + Vec3::Y * y1,
        b_in + n * 0.02 + Vec3::Y * y1,
        0.08,
        0.06,
        0.0,
    );
    // Sill sloping out.
    m.color(gray(0.6));
    m.beam(
        a + Vec3::Y * (y0 - 0.03),
        b + Vec3::Y * (y0 - 0.03),
        inset * 2.0 + 0.08,
        0.05,
        0.0,
    );
}

// ---------------------------------------------------------------- airport buildings

/// Outer walls of a box footprint (`c` = four corners at ground level, in the same order as
/// a hangar's front-left, front-right, back-right, back-left) between `y0` and `y1`; `skip`
/// leaves out edge k (from corner k to k + 1).
fn walls(b: &mut Builder, c: [Vec3; 4], y0: f32, y1: f32, ao: f32, skip: Option<usize>) {
    for k in 0..4 {
        if skip == Some(k) {
            continue;
        }
        panel(b, c[(k + 1) % 4], c[k], y0, y1, ao);
    }
}

/// A flat roof over `c` at `y` with a parapet coping round it.
fn flat_roof(parts: &mut Parts, c: [Vec3; 4], y: f32, roof: Vec4, coping: Vec4) {
    let r = parts.get(Surface::Painted);
    r.color(roof);
    flat(r, c.map(|q| q + Vec3::Y * y), true);
    r.color(coping);
    for k in 0..4 {
        let (a, b) = (c[k] + Vec3::Y * y, c[(k + 1) % 4] + Vec3::Y * y);
        let d = (b - a).normalize_or(Vec3::X);
        // Overlap the corners so the coping closes.
        r.beam(a - d * 0.15, b + d * 0.15, 0.3, 0.45, 0.02);
    }
}

/// A wall from `a` to `b` (seen from outside, `a` on the left) from the ground to `top`,
/// with window rows (`rows`: y0, y1) punched through it along the stretches `runs`
/// (fractions of its length).
#[allow(clippy::too_many_arguments)]
fn punched_wall(
    parts: &mut Parts,
    surface: Surface,
    wall: Vec4,
    a: Vec3,
    b: Vec3,
    top: f32,
    rows: &[(f32, f32)],
    runs: &[(f32, f32)],
) {
    let mut y = 0.0;
    for &(y0, y1) in rows {
        let c = parts.get(surface);
        c.color(wall);
        panel(c, a, b, y, y0, if y == 0.0 { 0.15 } else { 0.0 });
        let mut t = 0.0;
        for &(t0, t1) in runs {
            if t0 > t {
                panel(c, a.lerp(b, t), a.lerp(b, t0), y0, y1, 0.0);
            }
            t = t1;
        }
        if t < 1.0 {
            panel(c, a.lerp(b, t), b, y0, y1, 0.0);
        }
        for &(t0, t1) in runs {
            glazing(parts, a.lerp(b, t0), a.lerp(b, t1), y0, y1, 1.2, 0.1);
        }
        y = y1;
    }
    let c = parts.get(surface);
    c.color(wall);
    panel(c, a, b, y, top, 0.0);
}

/// A roller shutter between frame x `xa` and `xb` at depth `z`, covering heights `y0`..`y1`,
/// facing −z: horizontal slats, a bottom rail and guide rails.
#[allow(clippy::too_many_arguments)]
fn shutter(parts: &mut Parts, at: Place, xa: f32, xb: f32, z: f32, y0: f32, y1: f32, color: Vec4) {
    let g = parts.get(Surface::Painted);
    g.color(gray(0.2));
    for x in [xa, xb] {
        g.beam(
            at.p(x, 0.0, z - 0.02),
            at.p(x, y1 + 0.3, z - 0.02),
            0.12,
            0.12,
            0.01,
        );
    }
    if y1 - y0 < 0.05 {
        return;
    }
    g.beam(
        at.p(xa, y0 + 0.06, z - 0.03),
        at.p(xb, y0 + 0.06, z - 0.03),
        0.06,
        0.12,
        0.01,
    );
    let s = parts.get(Surface::Cladding);
    s.color(color);
    let n = at.v(Vec3::NEG_Z);
    let p = [
        at.p(xa, y0, z),
        at.p(xb, y0, z),
        at.p(xb, y1, z),
        at.p(xa, y1, z),
    ];
    // Ribs run across: u = height, v = width.
    let uv = [
        Vec2::new(y0 * 1.6, xa),
        Vec2::new(y0 * 1.6, xb),
        Vec2::new(y1 * 1.6, xb),
        Vec2::new(y1 * 1.6, xa),
    ];
    s.quad_n(p, uv, n);
}

/// A vertical quad in a structure's frame between frame points `a` and `b` (bottom corners)
/// and heights `y0`..`y1`, facing `n` (frame direction).
#[allow(clippy::too_many_arguments)]
fn upright(b: &mut Builder, at: Place, a: Vec2, c: Vec2, y0: f32, y1: f32, n: Vec3) {
    b.quad_n(
        [
            at.p(a.x, y0, a.y),
            at.p(c.x, y0, c.y),
            at.p(c.x, y1, c.y),
            at.p(a.x, y1, a.y),
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        at.v(n),
    );
}

/// An airport rescue and fire-fighting station: a three-bay appliance hall with roller
/// shutters (the left bay open, a crash tender inside), a two-storey crew wing with ribbon
/// windows, a hose-drying tower, a red fascia with lettering and rooftop plant. The bays
/// face −z of `at`; the hall spans x = −10.5..10.5 and z = 0..17, the crew wing
/// x = 10.5..19.5, z = 0..12, the tower x = −14.5..−10.5 at the back.
pub fn fire_station(parts: &mut Parts, at: Place) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let bay = 7.0f32;
    let hw = bay * 1.5;
    let d = 17.0f32;
    let hall_h = 7.6f32;
    let (door_w, door_h) = (5.2f32, 5.4f32);
    let floor = 0.15f32;
    let (wing_w, wing_d, wing_h) = (9.0f32, 12.0f32, 7.2f32);
    let red = hex(0xa3141a);
    // Floor slab.
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.96));
    c.chamfer_box(
        p(wing_w * 0.5 - 2.0, floor * 0.5, d * 0.5),
        Vec3::new(hw + wing_w * 0.5 + 2.3, floor * 0.5, d * 0.5 + 0.3),
        0.03,
        at.r,
    );
    // Hall: clad sides and back, concrete front piers and lintels.
    let hall = [
        p(-hw, floor, 0.0),
        p(hw, floor, 0.0),
        p(hw, floor, d),
        p(-hw, floor, d),
    ];
    let cl = parts.get(Surface::Cladding);
    cl.color(hex(0xd3d6d8));
    panel(cl, hall[3], hall[2], 0.0, hall_h, 0.2);
    panel(cl, hall[0], hall[3], 0.0, hall_h, 0.2);
    panel(cl, p(hw, floor, d), p(hw, floor, wing_d), 0.0, hall_h, 0.2);
    panel(
        cl,
        p(hw, floor, wing_d),
        p(hw, floor, 0.0),
        wing_h,
        hall_h,
        0.0,
    );
    let mut edges = vec![-hw];
    for k in 0..3 {
        let xc = -hw + (k as f32 + 0.5) * bay;
        edges.push(xc - door_w * 0.5);
        edges.push(xc + door_w * 0.5);
    }
    edges.push(hw);
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.93));
    for pair in edges.chunks(2) {
        let (xa, xb) = (pair[0], pair[1]);
        panel(c, p(xb, floor, 0.0), p(xa, floor, 0.0), 0.0, hall_h, 0.2);
    }
    let depth = 0.35f32;
    for k in 0..3 {
        let xc = -hw + (k as f32 + 0.5) * bay;
        let (xa, xb) = (xc - door_w * 0.5, xc + door_w * 0.5);
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.93));
        panel(c, p(xb, floor, 0.0), p(xa, floor, 0.0), door_h, hall_h, 0.0);
        // Reveals and the soffit back to the shutter line.
        let r = parts.get(Surface::Painted);
        r.color(gray(0.55));
        let top = floor + door_h;
        upright(
            r,
            at,
            Vec2::new(xa, 0.0),
            Vec2::new(xa, depth),
            floor,
            top,
            Vec3::X,
        );
        upright(
            r,
            at,
            Vec2::new(xb, 0.0),
            Vec2::new(xb, depth),
            floor,
            top,
            Vec3::NEG_X,
        );
        r.quad_n(
            [
                p(xa, top, 0.0),
                p(xb, top, 0.0),
                p(xb, top, depth),
                p(xa, top, depth),
            ],
            [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            Vec3::NEG_Y,
        );
        // Yellow and black bollards guard each opening.
        let b = parts.get(Surface::Painted);
        for x in [xa - 0.35, xb + 0.35] {
            b.color(hex(0xe0a812));
            b.cylinder(
                p(x, floor, -0.35),
                p(x, floor + 1.1, -0.35),
                0.11,
                0.11,
                12,
                true,
            );
            b.color(gray(0.05));
            for y in [0.35f32, 0.75] {
                let (y0, y1) = (floor + y, floor + y + 0.12);
                b.cylinder(p(x, y0, -0.35), p(x, y1, -0.35), 0.113, 0.113, 12, false);
            }
        }
        // Shutters: the left bay open (rolled up), the others closed with a vision panel.
        let open = if k == 0 { 0.96 } else { 0.0 };
        let y0 = floor + open * door_h;
        shutter(
            parts,
            at,
            xa + 0.05,
            xb - 0.05,
            depth,
            y0,
            top,
            hex(0xe9ebec),
        );
        if open < 0.5 {
            let gl = parts.get(Surface::Glass);
            gl.color(Vec4::ONE);
            let y = floor + 2.4;
            let z = depth - 0.01;
            upright(
                gl,
                at,
                Vec2::new(xb - 0.3, z),
                Vec2::new(xa + 0.3, z),
                y,
                y + 0.35,
                Vec3::NEG_Z,
            );
        }
    }
    // Inside the hall: a dim interior box, a painted floor with bay lines and strip lights.
    {
        let i = parts.get(Surface::Interior);
        i.color(gray(1.6));
        let e = 0.2f32;
        let y1 = hall_h - 0.4;
        let (x0, x1, z0, z1) = (-hw + e, hw - e, depth + 0.05, d - e);
        upright(
            i,
            at,
            Vec2::new(x0, z1),
            Vec2::new(x1, z1),
            floor,
            y1,
            Vec3::NEG_Z,
        );
        upright(
            i,
            at,
            Vec2::new(x0, z0),
            Vec2::new(x0, z1),
            floor,
            y1,
            Vec3::X,
        );
        upright(
            i,
            at,
            Vec2::new(x1, z0),
            Vec2::new(x1, z1),
            floor,
            y1,
            Vec3::NEG_X,
        );
        i.quad_n(
            [p(x0, y1, z0), p(x1, y1, z0), p(x1, y1, z1), p(x0, y1, z1)],
            [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            Vec3::NEG_Y,
        );
        let f = parts.get(Surface::Painted);
        f.color(hex(0x5a6066));
        let y = floor + 0.005;
        flat(
            f,
            [p(x0, y, z0), p(x1, y, z0), p(x1, y, z1), p(x0, y, z1)],
            true,
        );
        f.color(hex(0xe0a812));
        for k in 1..3 {
            let x = -hw + k as f32 * bay;
            f.beam(
                p(x, floor + 0.008, 0.6),
                p(x, floor + 0.008, d - 0.6),
                0.1,
                0.004,
                0.0,
            );
        }
        let l = parts.get(Surface::Lamp);
        l.color(gray(0.5));
        for k in 0..3 {
            let x = -hw + (k as f32 + 0.5) * bay;
            for z in [4.0f32, 9.0, 14.0] {
                l.chamfer_box(p(x, y1 - 0.06, z), Vec3::new(0.08, 0.04, 1.2), 0.0, at.r);
            }
        }
    }
    crash_tender(
        parts,
        Place {
            o: p(-hw + 0.5 * bay, floor, 1.2),
            r: at.r,
        },
    );
    // Red fascia across the hall front with the station's lettering.
    let g = parts.get(Surface::Painted);
    g.color(red);
    g.chamfer_box(
        p(0.0, hall_h - 0.55, -0.12),
        Vec3::new(hw + 0.12, 0.85, 0.12),
        0.02,
        at.r,
    );
    let t = parts.get(Surface::Glyphs);
    t.color(gray(0.95));
    // Read from the front, text runs toward −x.
    let (right, front) = (at.v(Vec3::NEG_X), at.v(Vec3::NEG_Z));
    text(
        t,
        p(0.0, hall_h - 0.55, -0.245),
        right,
        Vec3::Y,
        front,
        "FIRE  RESCUE",
        0.9,
        1.0,
    );
    flat_roof(
        parts,
        hall.map(|q| q - Vec3::Y * floor),
        hall_h + floor,
        gray(0.32),
        gray(0.62),
    );
    // Crew wing: two storeys of ribbon windows on cladding, entrance with a canopy.
    let wing = [
        p(hw, floor, 0.0),
        p(hw + wing_w, floor, 0.0),
        p(hw + wing_w, floor, wing_d),
        p(hw, floor, wing_d),
    ];
    let wall = hex(0xe6e3dc);
    let cl = parts.get(Surface::Cladding);
    cl.color(wall);
    for (y0, y1, ao) in [
        (0.0f32, 0.9f32, 0.2f32),
        (2.4, 4.5, 0.0),
        (6.0, wing_h, 0.0),
    ] {
        walls(cl, wing, y0, y1, ao, Some(3));
    }
    for k in 0..3 {
        let (a, b) = (wing[(k + 1) % 4], wing[k]);
        for (y0, y1) in [(0.9f32, 2.4f32), (4.5, 6.0)] {
            if k == 0 && y0 < 1.0 {
                // Ground floor front: windows, the entrance at the outer end.
                let m = a.lerp(b, 0.62);
                glazing(parts, m, b, y0, y1, 1.5, 0.12);
                let cl = parts.get(Surface::Cladding);
                cl.color(wall);
                panel(cl, a, m, y0, y1, 0.0);
                continue;
            }
            glazing(parts, a, b, y0, y1, 1.5, 0.12);
        }
    }
    {
        let x = hw + wing_w * 0.82;
        let gl = parts.get(Surface::Glass);
        gl.color(Vec4::ONE);
        let z = -0.02;
        upright(
            gl,
            at,
            Vec2::new(x + 0.8, z),
            Vec2::new(x - 0.8, z),
            floor,
            floor + 2.3,
            Vec3::NEG_Z,
        );
        let g = parts.get(Surface::Painted);
        g.color(gray(0.2));
        for dx in [-0.8f32, 0.0, 0.8] {
            g.beam(
                p(x + dx, floor, -0.04),
                p(x + dx, floor + 2.35, -0.04),
                0.07,
                0.07,
                0.0,
            );
        }
        g.beam(
            p(x - 0.85, floor + 2.35, -0.04),
            p(x + 0.85, floor + 2.35, -0.04),
            0.08,
            0.08,
            0.0,
        );
        g.color(gray(0.3));
        g.chamfer_box(
            p(x, floor + 2.9, -1.0),
            Vec3::new(1.6, 0.08, 1.0),
            0.02,
            at.r,
        );
    }
    flat_roof(
        parts,
        wing.map(|q| q - Vec3::Y * floor),
        wing_h + floor,
        gray(0.32),
        gray(0.62),
    );
    // Hose-drying tower at the back left: concrete with slit windows, red crown.
    {
        let (x0, x1, z0, z1, top) = (-hw - 4.0, -hw, d - 4.2, d, 14.5f32);
        let tw = [
            p(x0, floor, z0),
            p(x1, floor, z0),
            p(x1, floor, z1),
            p(x0, floor, z1),
        ];
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.9));
        // Back and left side plain, the right side above the hall, slit windows in front.
        panel(c, tw[3], tw[2], 0.0, top - 1.2, 0.2);
        panel(c, tw[0], tw[3], 0.0, top - 1.2, 0.2);
        panel(c, tw[2], tw[1], hall_h, top - 1.2, 0.0);
        let rows = [(3.0f32, 4.8f32), (6.5, 8.3), (10.0, 11.8)];
        let (front_l, front_r) = (tw[1], tw[0]);
        let slit = [(0.42f32, 0.58f32)];
        punched_wall(
            parts,
            Surface::Concrete,
            gray(0.9),
            front_l,
            front_r,
            top - 1.2,
            &rows,
            &slit,
        );
        let g = parts.get(Surface::Painted);
        g.color(red);
        walls(g, tw, top - 1.2, top, 0.0, None);
        flat_roof(
            parts,
            tw.map(|q| q - Vec3::Y * floor),
            top + floor,
            gray(0.3),
            gray(0.6),
        );
        let g = parts.get(Surface::Galvanized);
        g.color(Vec4::ONE);
        let m = p(x0 + 1.0, top + floor + 0.45, z1 - 1.0);
        g.cylinder(m, m + Vec3::Y * 4.0, 0.05, 0.03, 8, false);
        let r = parts.get(Surface::LensRed);
        r.color(Vec4::ONE);
        r.sphere(m + Vec3::Y * 4.1, 0.1, 10, 1.0);
    }
    // Rooftop plant and a siren on the hall roof.
    let roof = hall_h + floor;
    let g = parts.get(Surface::Painted);
    for (x, z) in [(-4.0f32, 11.0f32), (3.0, 12.5)] {
        g.color(gray(0.6));
        g.chamfer_box(p(x, roof + 0.75, z), Vec3::new(1.4, 0.55, 0.9), 0.04, at.r);
        g.color(gray(0.2));
        g.cylinder(
            p(x, roof + 1.3, z),
            p(x, roof + 1.36, z),
            0.55,
            0.55,
            16,
            true,
        );
    }
    let s = parts.get(Surface::Galvanized);
    s.color(Vec4::ONE);
    s.cylinder(
        p(6.0, roof + 0.45, 3.0),
        p(6.0, roof + 2.6, 3.0),
        0.04,
        0.04,
        8,
        false,
    );
    s.cylinder(
        p(6.0, roof + 2.6, 3.0),
        p(6.0, roof + 2.6, 2.6),
        0.12,
        0.2,
        12,
        true,
    );
}

/// A 6×6 airport crash tender facing −z of `at` (front bumper at z = 0): lime-yellow body
/// with a white stripe, glazed cab, roof and bumper monitors, light bar, six wheels.
pub fn crash_tender(parts: &mut Parts, at: Place) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let body = hex(0xb4c812);
    let b = parts.get(Surface::Painted);
    b.color(body);
    // Cab and body.
    b.chamfer_box(p(0.0, 2.05, 1.5), Vec3::new(1.25, 1.15, 1.3), 0.12, at.r);
    b.chamfer_box(p(0.0, 2.15, 6.6), Vec3::new(1.25, 1.25, 3.7), 0.08, at.r);
    // Lower skirt and bumper.
    b.color(gray(0.12));
    b.chamfer_box(p(0.0, 0.85, 5.2), Vec3::new(1.2, 0.3, 5.0), 0.03, at.r);
    b.chamfer_box(p(0.0, 0.85, 0.15), Vec3::new(1.25, 0.22, 0.16), 0.04, at.r);
    // White stripe and red reflective band.
    b.color(gray(0.92));
    b.chamfer_box(p(0.0, 1.55, 6.6), Vec3::new(1.262, 0.12, 3.6), 0.0, at.r);
    b.color(hex(0xc01818));
    b.chamfer_box(p(0.0, 1.3, 6.6), Vec3::new(1.262, 0.08, 3.6), 0.0, at.r);
    // Equipment lockers: shutter seams.
    b.color(body * 0.8);
    for k in 0..4 {
        let z = 3.6 + k as f32 * 1.75;
        for s in [-1.0f32, 1.0] {
            b.chamfer_box(
                p(s * 1.262, 2.4, z),
                Vec3::new(0.006, 0.9, 0.025),
                0.0,
                at.r,
            );
        }
    }
    // Cab glass: windscreen and side windows.
    let gl = parts.get(Surface::Glass);
    gl.color(Vec4::ONE);
    gl.quad_n(
        [
            p(1.1, 2.1, 0.19),
            p(-1.1, 2.1, 0.19),
            p(-1.05, 3.05, 0.32),
            p(1.05, 3.05, 0.32),
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        at.v(Vec3::new(0.0, 0.13, -1.0).normalize()),
    );
    for s in [-1.0f32, 1.0] {
        let x = s * 1.256;
        upright(
            gl,
            at,
            Vec2::new(x, 0.5),
            Vec2::new(x, 2.5),
            2.1,
            3.0,
            Vec3::X * s,
        );
    }
    // Light bar, headlights.
    let r = parts.get(Surface::LensOff);
    r.color(Vec4::ONE);
    r.chamfer_box(p(0.0, 3.27, 0.8), Vec3::new(0.85, 0.07, 0.14), 0.03, at.r);
    let w = parts.get(Surface::LensClear);
    w.color(gray(0.6));
    for s in [-0.85f32, 0.85] {
        w.chamfer_box(p(s, 1.15, 0.0), Vec3::new(0.16, 0.09, 0.02), 0.01, at.r);
    }
    // Roof and bumper monitors.
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    g.cylinder(p(0.0, 3.4, 4.0), p(0.0, 3.75, 4.0), 0.22, 0.18, 12, true);
    g.cylinder(p(0.0, 3.85, 3.6), p(0.0, 3.85, 2.3), 0.09, 0.06, 10, true);
    g.cylinder(p(0.0, 3.75, 4.0), p(0.0, 3.9, 3.6), 0.1, 0.1, 8, false);
    g.cylinder(p(0.0, 1.1, -0.05), p(0.0, 1.1, -0.6), 0.06, 0.045, 8, true);
    // Ladder rails along the roof.
    for s in [-0.9f32, 0.9] {
        g.beam(p(s, 3.45, 4.8), p(s, 3.45, 10.0), 0.05, 0.05, 0.0);
    }
    // Wheels: front axle and a tandem at the back.
    for z in [1.9f32, 7.4, 9.0] {
        for s in [-1.0f32, 1.0] {
            let t = parts.get(Surface::Rubber);
            t.color(Vec4::ONE);
            let c = p(s * 1.0, 0.62, z);
            let axis = at.v(Vec3::X) * s;
            t.cylinder(c - axis * 0.3, c + axis * 0.3, 0.62, 0.62, 20, true);
            let h = parts.get(Surface::Painted);
            h.color(gray(0.75));
            h.cylinder(c + axis * 0.3, c + axis * 0.31, 0.32, 0.32, 12, true);
        }
    }
}

/// A two-storey general aviation terminal facing −z of `at`, `w` wide and `d` deep: a
/// glazed front with mullions and slab bands, clad sides and back with windows, a canopy on
/// slender columns, the operator's lettering on the roof band, rooftop plant behind a
/// screen.
pub fn terminal(parts: &mut Parts, at: Place, w: f32, d: f32, name: &str) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let hw = w * 0.5;
    let floor = 0.15f32;
    let top = 8.2f32;
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.95));
    c.chamfer_box(
        p(0.0, floor * 0.5, (d - 2.8) * 0.5),
        Vec3::new(hw + 0.4, floor * 0.5, (d + 2.8) * 0.5),
        0.03,
        at.r,
    );
    let ring = [
        p(-hw, floor, 0.0),
        p(hw, floor, 0.0),
        p(hw, floor, d),
        p(-hw, floor, d),
    ];
    // Sides and back: cladding with windows.
    let wall = hex(0x9aa3a8);
    let rows = [(1.0f32, 3.0f32), (4.6, 6.6)];
    for k in 1..4 {
        let (a, b) = (ring[(k + 1) % 4], ring[k]);
        let runs: &[(f32, f32)] = if k == 2 {
            &[(0.1, 0.4), (0.6, 0.9)]
        } else {
            &[(0.2, 0.8)]
        };
        punched_wall(parts, Surface::Cladding, wall, a, b, top, &rows, runs);
    }
    // Front: two storeys of curtain wall between a slab band and the roof band.
    let (a, b) = (ring[1], ring[0]);
    let cl = parts.get(Surface::Cladding);
    cl.color(wall);
    panel(cl, a, b, 0.0, 0.25, 0.2);
    glazing(parts, a, b, 0.25, 3.7, 1.5, 0.06);
    glazing(parts, a, b, 4.3, 7.4, 1.5, 0.06);
    let g = parts.get(Surface::Painted);
    g.color(gray(0.2));
    panel(g, a, b, 3.7, 4.3, 0.0);
    panel(g, a, b, 7.4, top, 0.0);
    // Doors in the middle of the front.
    let gl = parts.get(Surface::Glass);
    gl.color(Vec4::ONE);
    for dx in [-1.6f32, 0.0] {
        let z = -0.05;
        upright(
            gl,
            at,
            Vec2::new(dx + 1.6, z),
            Vec2::new(dx, z),
            floor,
            floor + 2.4,
            Vec3::NEG_Z,
        );
    }
    let g = parts.get(Surface::Painted);
    g.color(gray(0.16));
    for dx in [-1.6f32, 0.0, 1.6] {
        g.beam(
            p(dx, floor, -0.07),
            p(dx, floor + 2.45, -0.07),
            0.08,
            0.08,
            0.0,
        );
    }
    g.beam(
        p(-1.65, floor + 2.45, -0.07),
        p(1.65, floor + 2.45, -0.07),
        0.1,
        0.1,
        0.0,
    );
    // Canopy over the front: a deep slab on round columns, lettering on its fascia.
    let cd = 2.6f32;
    let cy = 4.0f32;
    let cw = hw * 0.7;
    g.color(gray(0.88));
    g.chamfer_box(
        p(0.0, cy + 0.2, -cd * 0.5),
        Vec3::new(cw, 0.2, cd * 0.5),
        0.03,
        at.r,
    );
    g.color(gray(0.2));
    g.chamfer_box(
        p(0.0, cy + 0.3, -cd),
        Vec3::new(cw + 0.03, 0.32, 0.06),
        0.01,
        at.r,
    );
    let col = parts.get(Surface::Galvanized);
    col.color(gray(0.85));
    let n = 5;
    for k in 0..n {
        let x = mix(-cw + 0.6, cw - 0.6, k as f32 / (n - 1) as f32);
        col.cylinder(
            p(x, floor, -cd + 0.4),
            p(x, cy, -cd + 0.4),
            0.12,
            0.12,
            12,
            false,
        );
    }
    // Read from the front, text runs toward −x.
    let (right, front) = (at.v(Vec3::NEG_X), at.v(Vec3::NEG_Z));
    let t = parts.get(Surface::Glyphs);
    t.color(gray(0.95));
    text(
        t,
        p(0.0, cy + 0.3, -cd - 0.065),
        right,
        Vec3::Y,
        front,
        "TERMINAL",
        0.42,
        1.0,
    );
    // Big lettering on the roof band.
    text(
        t,
        p(0.0, (7.4 + top) * 0.5, -0.01),
        right,
        Vec3::Y,
        front,
        name,
        0.55,
        1.0,
    );
    flat_roof(
        parts,
        ring.map(|q| q - Vec3::Y * floor),
        top + floor,
        gray(0.3),
        gray(0.55),
    );
    // Rooftop plant behind a screen.
    let roof = top + floor;
    let g = parts.get(Surface::Painted);
    g.color(gray(0.62));
    g.chamfer_box(
        p(-hw * 0.4, roof + 0.8, d * 0.6),
        Vec3::new(2.2, 0.6, 1.4),
        0.04,
        at.r,
    );
    g.chamfer_box(
        p(hw * 0.35, roof + 0.6, d * 0.55),
        Vec3::new(1.2, 0.45, 1.0),
        0.04,
        at.r,
    );
    let cl = parts.get(Surface::Cladding);
    cl.color(gray(0.5));
    let (sx, sz) = (-hw * 0.4, d * 0.6);
    let sc = [
        p(sx - 3.0, roof, sz - 2.2),
        p(sx + 3.0, roof, sz - 2.2),
        p(sx + 3.0, roof, sz + 2.2),
        p(sx - 3.0, roof, sz + 2.2),
    ];
    walls(cl, sc, 0.0, 1.8, 0.0, None);
}

// ---------------------------------------------------------------- masts and windsocks

/// A floodlight mast `h` tall at `base`, its head of `heads` lamps aimed down toward
/// `target`. Returns the lamp positions.
pub fn floodlight_mast(
    parts: &mut Parts,
    base: Vec3,
    h: f32,
    target: Vec3,
    heads: u32,
) -> Vec<Vec3> {
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    // Footing and base plate.
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.9));
    c.cylinder(
        base - Vec3::Y * 0.2,
        base + Vec3::Y * 0.35,
        0.75,
        0.7,
        16,
        true,
    );
    let g = parts.get(Surface::Galvanized);
    g.cylinder(
        base + Vec3::Y * 0.35,
        base + Vec3::Y * 0.4,
        0.5,
        0.5,
        8,
        true,
    );
    for k in 0..8 {
        let a = k as f32 / 8.0 * TAU;
        let q = base + Vec3::new(a.cos() * 0.4, 0.4, a.sin() * 0.4);
        g.cylinder(q, q + Vec3::Y * 0.08, 0.03, 0.03, 6, true);
    }
    // Tapered octagonal pole in three sections with slip joints.
    let sections = [
        (0.4f32, 0.0f32, 0.3f32),
        (0.27, 0.4, 0.22),
        (0.2, 0.7, 0.16),
    ];
    for (r0, t0, r1) in sections {
        let t1 = (t0 + 0.42).min(1.0);
        let y0 = 0.4 + (h - 0.4) * t0;
        let y1 = 0.4 + (h - 0.4) * t1;
        g.cylinder(base + Vec3::Y * y0, base + Vec3::Y * y1, r0, r1, 8, false);
    }
    // Head frame and lamps.
    let top = base + Vec3::Y * h;
    let to = Vec3::new(target.x - base.x, 0.0, target.z - base.z).normalize_or(Vec3::X);
    let side = Vec3::new(-to.z, 0.0, to.x);
    g.beam(top - side * 1.5, top + side * 1.5, 0.12, 0.12, 0.0);
    g.beam(
        top - side * 1.5 + Vec3::Y * 1.0,
        top + side * 1.5 + Vec3::Y * 1.0,
        0.12,
        0.12,
        0.0,
    );
    for s in [-1.5f32, 0.0, 1.5] {
        g.beam(
            top + side * s,
            top + side * s + Vec3::Y * 1.0,
            0.1,
            0.1,
            0.0,
        );
    }
    // Service platform with railing.
    let deck = top - Vec3::Y * 0.9;
    g.chamfer_box(
        deck + to * 0.2,
        Vec3::new(1.7, 0.04, 0.6),
        0.0,
        facing_z(to),
    );
    let rail = [
        deck - side * 1.7 - to * 0.4,
        deck - side * 1.7 + to * 0.8,
        deck + side * 1.7 + to * 0.8,
        deck + side * 1.7 - to * 0.4,
    ];
    railing(parts, &rail, 1.0, 1.2);
    let drop = (top.y - target.y).max(1.0);
    let reach = Vec3::new(target.x - top.x, 0.0, target.z - top.z).length();
    let tilt = drop.atan2(reach);
    let mut lamps = Vec::new();
    let per_row = heads.div_ceil(2);
    for k in 0..heads {
        let row = k / per_row;
        let col = k % per_row;
        let x = (col as f32 - (per_row as f32 - 1.0) * 0.5) * 0.95;
        let y = row as f32 * 1.0 + 0.1;
        let c = top + side * x + Vec3::Y * y + to * 0.35;
        let rot = facing_z(to) * Quat::from_rotation_x(tilt);
        let face = rot * Vec3::Z;
        let p = parts.get(Surface::Painted);
        p.color(gray(0.14));
        p.chamfer_box(c, Vec3::new(0.4, 0.3, 0.12), 0.03, rot);
        // Cooling fins at the back.
        for f in 0..4 {
            let fx = (f as f32 - 1.5) * 0.18;
            p.chamfer_box(
                c - face * 0.17 + rot * Vec3::new(fx, 0.0, 0.0),
                Vec3::new(0.015, 0.26, 0.05),
                0.0,
                rot,
            );
        }
        let l = parts.get(Surface::Lamp);
        l.color(Vec4::ONE);
        let f = c + face * 0.125;
        let (u, v) = (rot * Vec3::X, rot * Vec3::Y);
        l.quad_n(
            [
                f - u * 0.34 - v * 0.24,
                f + u * 0.34 - v * 0.24,
                f + u * 0.34 + v * 0.24,
                f - u * 0.34 + v * 0.24,
            ],
            [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            face,
        );
        lamps.push(f + face * 0.3);
    }
    // Access ladder up the pole.
    let g = parts.get(Surface::Galvanized);
    let lad = base - to * 0.45;
    for s in [-0.2f32, 0.2] {
        g.cylinder(
            lad + side * s + Vec3::Y * 2.5,
            lad + side * s + Vec3::Y * (h - 1.0),
            0.02,
            0.02,
            4,
            false,
        );
    }
    let mut y = 2.6;
    while y < h - 1.0 {
        g.cylinder(
            lad - side * 0.2 + Vec3::Y * y,
            lad + side * 0.2 + Vec3::Y * y,
            0.015,
            0.015,
            4,
            false,
        );
        y += 0.3;
    }
    lamps
}

/// A windsock on a 6 m mast at `base`, blowing toward `toward` (horizontal): five orange
/// and white bands, sagging a little.
pub fn windsock(parts: &mut Parts, base: Vec3, toward: Vec3) {
    let dir = Vec3::new(toward.x, 0.0, toward.z).normalize_or(Vec3::X);
    let mast_h = 6.0f32;
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.9));
    c.chamfer_box(
        base + Vec3::Y * 0.1,
        Vec3::new(0.6, 0.12, 0.6),
        0.03,
        Quat::IDENTITY,
    );
    let p = parts.get(Surface::Painted);
    // Mast in red and white bands.
    let bands = 6;
    for k in 0..bands {
        let y0 = 0.2 + (mast_h - 0.2) * k as f32 / bands as f32;
        let y1 = 0.2 + (mast_h - 0.2) * (k + 1) as f32 / bands as f32;
        p.color(if k % 2 == 0 { hex(0xc8321e) } else { gray(0.9) });
        p.cylinder(
            base + Vec3::Y * y0,
            base + Vec3::Y * y1,
            0.07,
            0.07,
            10,
            k == bands - 1,
        );
    }
    // Swivel and ring frame.
    let top = base + Vec3::Y * mast_h;
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    g.cylinder(top, top + Vec3::Y * 0.25, 0.06, 0.06, 8, true);
    let mouth_r = 0.45f32;
    let mouth = top + Vec3::Y * 0.1 + dir * 0.45;
    g.torus(mouth, dir, mouth_r, 0.018, 24, 6);
    g.beam(
        top + Vec3::Y * 0.1,
        mouth + Vec3::Y * mouth_r,
        0.03,
        0.03,
        0.0,
    );
    g.beam(
        top + Vec3::Y * 0.1,
        mouth - Vec3::Y * mouth_r,
        0.03,
        0.03,
        0.0,
    );
    // The sock: a frustum in five bands, the tail drooping.
    let len = 3.6f32;
    let sag = 0.22f32;
    let n = 5;
    for k in 0..n {
        let t0 = k as f32 / n as f32;
        let t1 = (k + 1) as f32 / n as f32;
        let pos = |t: f32| mouth + dir * (len * t) - Vec3::Y * (sag * len * t * t);
        let r = |t: f32| mix(mouth_r, mouth_r * 0.35, t);
        let col = if k % 2 == 0 {
            hex(0xe8590c)
        } else {
            gray(0.92)
        };
        let fab = parts.get(Surface::Plastic);
        fab.color(col);
        fab.cylinder(pos(t0), pos(t1), r(t0), r(t1), 16, false);
        fab.color(col * 0.6);
        inner_cylinder(fab, pos(t0), pos(t1), r(t0) - 0.005, r(t1) - 0.005, 16);
    }
}

/// An airfield-style sign on two legs: board `w` × `h` with its bottom `y` above the
/// ground, reading toward `facing` (horizontal unit). Both faces get the same board.
pub fn sign(parts: &mut Parts, center: Vec3, facing: Vec3, w: f32, h: f32, y: f32, index: usize) {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::Z);
    let right = Vec3::Y.cross(f).normalize();
    let g = parts.get(Surface::Painted);
    g.color(gray(0.1));
    let rot = facing_z(f);
    g.chamfer_box(
        center + Vec3::Y * (y + h * 0.5),
        Vec3::new(w * 0.5 + 0.06, h * 0.5 + 0.06, 0.08),
        0.02,
        rot,
    );
    let gal = parts.get(Surface::Galvanized);
    gal.color(Vec4::ONE);
    for s in [-0.35f32, 0.35] {
        let p = center + right * (w * s);
        gal.cylinder(p, p + Vec3::Y * (y + 0.1), 0.04, 0.04, 8, false);
        // Frangible coupling.
        gal.cylinder(p + Vec3::Y * 0.12, p + Vec3::Y * 0.2, 0.06, 0.06, 8, false);
    }
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.85));
    c.chamfer_box(
        center + Vec3::Y * 0.05,
        Vec3::new(w * 0.45, 0.08, 0.3),
        0.02,
        rot,
    );
    let s = parts.get(Surface::Signs);
    s.color(Vec4::ONE);
    for side in [1.0f32, -1.0] {
        let n = f * side;
        let r = right * side;
        let c0 = center + Vec3::Y * y + n * 0.085;
        board(s, c0 - r * (w * 0.5), c0 + r * (w * 0.5), h, index);
    }
}

/// Rotation about +Y taking +Z to the horizontal direction `dir`.
pub fn facing_z(dir: Vec3) -> Quat {
    Quat::from_rotation_y(dir.x.atan2(dir.z))
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
