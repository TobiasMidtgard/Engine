//! The vault lane: a saw-cut concrete pad south of the runway with three obstacles across it,
//! each low and shallow enough to vault at a sprint, and the paint and mats that go with them.
//!
//! - **Railing**: yellow steel posts on bolted base plates, a round top rail and mid rail,
//!   close-set balusters and a kick plate.
//! - **Barrier**: a precast concrete barrier (the props' profile) with lifting loops and
//!   reflective strips.
//! - **Cases**: two olive equipment cases strapped onto a pair of wooden pallets.
//! - **Pad**: saw-cut concrete with hazard edges, lane lines, a start line, chevrons before
//!   each obstacle, rubber landing mats past each, "10 VAULT" painted at the start and on a
//!   backlit sign beside it.
//!
//! Built straight in the course's frame from the demo's collider boxes.

use super::*;

pub(super) fn build() -> Kit {
    let mut kit = Kit::default();
    pad(&mut kit);
    railing(&mut kit);
    barrier_parts(&mut kit);
    cases(&mut kit);
    sign(&mut kit);
    kit
}

/// The top of the pad (the obstacles stand on it).
fn floor() -> f32 {
    bounds(layout::VAULT_PAD).1.y
}

/// Landing mats: where each vault comes down (x from, x to), from the demo's vaulter.
const MATS: [(f32, f32); 3] = [(-20.3, -17.9), (-14.6, -12.2), (-8.0, -5.6)];

fn pad(kit: &mut Kit) {
    let (p0, p1) = bounds(layout::VAULT_PAD);
    let top = p1.y;
    let mut concrete = M::new();
    let g = 0.003;
    // Saw cut into bays along the lane, and either side of a middle strip.
    let xs: Vec<f32> = (0..=8)
        .map(|i| p0.x + (p1.x - p0.x) * i as f32 / 8.0)
        .collect();
    let zs = [p0.z, 1.0, 3.0, p1.z];
    for xw in xs.windows(2) {
        for zw in zs.windows(2) {
            let ga = |v: f32, a: f32, b: f32| {
                if (v - a).abs() < 1e-3 || (v - b).abs() < 1e-3 {
                    0.0
                } else {
                    g
                }
            };
            let min = v3(
                xw[0] + ga(xw[0], p0.x, p1.x),
                -0.3,
                zw[0] + ga(zw[0], p0.z, p1.z),
            );
            let max = v3(
                xw[1] - ga(xw[1], p0.x, p1.x),
                top,
                zw[1] - ga(zw[1], p0.z, p1.z),
            );
            concrete.append(&slab(min, max, 0.008, 0.5), Mat4::IDENTITY);
        }
    }
    let mid = layout::VAULT_START[2] as f32;
    concrete.shade(|p, _| {
        // Worn down the running line, scuffed dark where feet take off and land.
        let line = (-((p.z - mid) / 0.7).powi(2)).exp();
        let landings = MATS
            .iter()
            .map(|&(a, b)| {
                let c = (a + b) * 0.5;
                (-((p.x - c) / 1.6).powi(2)).exp()
            })
            .sum::<f32>();
        (0.9 + 0.1 * pn(p.x * 0.35, p.z * 0.35, 91)) * (1.0 - 0.08 * line * (1.0 + landings))
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

    // Rubber mats where each vault lands, butted in pairs.
    let mut rubber = M::new();
    for (a, b) in MATS {
        let half = (b - a) * 0.5;
        for x0 in [a, a + half] {
            rubber.append(
                &slab(
                    v3(x0 + 0.01, top - 0.01, mid - 0.8),
                    v3(x0 + half - 0.01, top + 0.006, mid + 0.8),
                    0.004,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
    }
    rubber.shade(|p, _| 0.85 + 0.15 * pn(p.x * 1.7, p.z * 1.7, 92));
    kit.add(RUBBER, rubber);

    let mut paint = M::new();
    let y = top + 0.0025;
    let white = paint_color(WHITE_PAINT);
    let yellow = paint_color(YELLOW_PAINT);
    // Lane lines either side of the running line, a start line, the lane's number and name.
    let start = layout::VAULT_START[0] as f32 + 0.45;
    let len = p1.x - start - 0.3;
    for z in [mid - 1.1, mid + 1.1] {
        p_line(
            &mut paint,
            v3(start + len * 0.5, y, z),
            Vec3::X,
            len,
            0.08,
            white,
        );
    }
    p_line(&mut paint, v3(start, y, mid), Vec3::Z, 2.28, 0.1, white);
    stencil(
        &mut paint,
        PAINT_START,
        v3(start - 0.75, y, mid),
        Vec3::Z,
        Vec3::X,
        0.3,
        white,
    );
    stencil(
        &mut paint,
        PAINT_10,
        v3(start + 1.0, y, mid - 0.6),
        Vec3::Z,
        Vec3::X,
        0.4,
        paint_color(ORANGE_PAINT),
    );
    stencil(
        &mut paint,
        PAINT_VAULT,
        v3(start + 1.0, y, mid + 0.45),
        Vec3::Z,
        Vec3::X,
        0.24,
        white,
    );
    // Chevrons in the run-up to each obstacle, clear of the mats, and the mats' landing boxes.
    for face in [
        bounds(layout::VAULT_RAIL).0.x,
        bounds(layout::VAULT_BARRIER[1]).0.x,
        bounds(layout::VAULT_CASES).0.x,
    ] {
        stencil(
            &mut paint,
            PAINT_CHEVRONS,
            v3(face - 0.9, y, mid),
            Vec3::X,
            Vec3::NEG_Z,
            0.32,
            yellow,
        );
    }
    for (a, b) in MATS {
        for z in [mid - 0.9, mid + 0.9] {
            p_line(
                &mut paint,
                v3((a + b) * 0.5, y, z),
                Vec3::X,
                b - a + 0.2,
                0.06,
                yellow,
            );
        }
    }
    kit.add(PAINT, paint);
}

/// Post spacing and section sizes of the railing, m.
const RAIL_POST: f32 = 0.05;
const RAIL_TOP: f32 = 0.024;
const BALUSTER: f32 = 0.008;

fn railing(kit: &mut Kit) {
    let (b0, b1) = bounds(layout::VAULT_RAIL);
    let x = (b0.x + b1.x) * 0.5;
    let (base, top) = (b0.y, b1.y);
    let mut yellow = M::new();
    let mut alu = M::new();
    let rail_y = top - RAIL_TOP;
    let mid_y = base + 0.55;
    let kick = base + 0.15;
    let posts = [b0.z + 0.06, (b0.z + b1.z) * 0.5, b1.z - 0.06];
    for &z in &posts {
        // A square post on a bolted base plate, its foot welded to the plate.
        yellow.append(
            &slab(
                v3(x - RAIL_POST * 0.5, base + 0.012, z - RAIL_POST * 0.5),
                v3(x + RAIL_POST * 0.5, rail_y, z + RAIL_POST * 0.5),
                0.004,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        alu.append(
            &slab(
                v3(x - 0.075, base, z - 0.075),
                v3(x + 0.075, base + 0.012, z + 0.075),
                0.003,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let bolt_at: Vec<Vec3> = [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
            .iter()
            .map(|&(sx, sz)| v3(x + sx * 0.05, base + 0.012, z + sz * 0.05))
            .collect();
        bolts(&mut alu, &bolt_at, Vec3::Y, 0.008);
    }
    // Top rail and mid rail, run past the end posts and capped.
    let (z0, z1) = (b0.z + 0.02, b1.z - 0.02);
    yellow.append(
        &rod(v3(x, rail_y, z0), v3(x, rail_y, z1), RAIL_TOP, 14),
        Mat4::IDENTITY,
    );
    yellow.append(
        &rod(v3(x, mid_y, z0), v3(x, mid_y, z1), 0.02, 12),
        Mat4::IDENTITY,
    );
    // The kick plate along the foot.
    yellow.append(
        &slab(
            v3(x - 0.004, base + 0.02, posts[0]),
            v3(x + 0.004, kick, posts[2]),
            0.002,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // Balusters between the posts, from the kick plate to the top rail.
    for w in posts.windows(2) {
        let (a, b) = (w[0] + RAIL_POST, w[1] - RAIL_POST);
        let n = ((b - a) / 0.11).round().max(1.0) as usize;
        for i in 0..=n {
            let z = a + (b - a) * i as f32 / n as f32;
            yellow.append(
                &rod(
                    v3(x, kick, z),
                    v3(x, rail_y - RAIL_TOP * 0.8, z),
                    BALUSTER,
                    6,
                ),
                Mat4::IDENTITY,
            );
        }
    }
    yellow.shade(|p, _| {
        // Grime up from the foot; the top rail rubbed lighter where hands plant on it.
        let foot = 0.72 + 0.28 * smoothstep(base, base + 0.5, p.y);
        let rubbed = (-((p.z - 1.78) / 0.35).powi(2)).exp() * smoothstep(top - 0.06, top, p.y);
        foot * (1.0 + 0.12 * rubbed) * (0.94 + 0.06 * pn(p.z * 3.0, p.y * 3.0, 93))
    });
    kit.add(STEEL_YELLOW, yellow);
    kit.add(ALU, alu);
}

fn barrier_parts(kit: &mut Kit) {
    let (b0, b1) = bounds(layout::VAULT_BARRIER[0]);
    let c = v3((b0.x + b1.x) * 0.5, b0.y, (b0.z + b1.z) * 0.5);
    kit.add(CONCRETE, barrier().transformed(at(c.x, c.y, c.z)));
    let mut steel = M::new();
    // Lifting loops cast into the top, near each end.
    for dz in [-0.95f32, 0.95] {
        let path: Vec<Vec3> = (0..=8)
            .map(|i| {
                let a = PI * i as f32 / 8.0;
                c + v3(0.0, 0.79 + 0.055 * a.sin(), dz + 0.06 * a.cos())
            })
            .collect();
        steel.append(&pipe(&path, 0.008, 6), Mat4::IDENTITY);
    }
    steel.shade(|_, _| 0.7);
    kit.add(STEEL_DARK, steel);
    // Reflective strips along both faces, set into the slope where it is narrowest.
    let mut strips = M::new();
    for side in [-1.0f32, 1.0] {
        let x = c.x + side * 0.109;
        strips.append(
            &bx(
                v3(x.min(x + side * 0.012), c.y + 0.58, c.z - 1.3),
                v3(x.max(x + side * 0.012), c.y + 0.66, c.z + 1.3),
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(STEEL_YELLOW, strips);
}

/// Two pallets under two equipment cases, strapped down.
fn cases(kit: &mut Kit) {
    let (b0, b1) = bounds(layout::VAULT_CASES);
    let x = (b0.x + b1.x) * 0.5;
    let depth = b1.x - b0.x;
    let base = b0.y;
    let mut wood = M::new();
    let board = 0.022;
    let block = 0.078;
    let deck = base + 3.0 * board + block;
    let half = (b1.z - b0.z) * 0.25;
    let mut lid = 0.0;
    let mut olive = M::new();
    let mut rubber = M::new();
    for zc in [b0.z + half, b1.z - half] {
        let span = half - 0.02;
        // Bottom boards, blocks, stringer boards and the top deck.
        for dx in [-0.5f32, 0.0, 0.5] {
            let bx_ = x + dx * (depth - 0.1);
            wood.append(
                &slab(
                    v3(bx_ - 0.05, base, zc - span),
                    v3(bx_ + 0.05, base + board, zc + span),
                    0.003,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
            for dz in [-1.0f32, 0.0, 1.0] {
                let bz = zc + dz * (span - 0.05);
                wood.append(
                    &slab(
                        v3(bx_ - 0.05, base + board, bz - 0.05),
                        v3(bx_ + 0.05, base + board + block, bz + 0.05),
                        0.004,
                        100.0,
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
        for dz in [-1.0f32, 0.0, 1.0] {
            let bz = zc + dz * (span - 0.05);
            wood.append(
                &slab(
                    v3(x - depth * 0.5, base + board + block, bz - 0.05),
                    v3(x + depth * 0.5, deck - board, bz + 0.05),
                    0.003,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
        for i in 0..7 {
            let bx_ = x - depth * 0.5 + 0.05 + (depth - 0.1) * i as f32 / 6.0;
            wood.append(
                &slab(
                    v3(bx_ - 0.05, deck - board, zc - span),
                    v3(bx_ + 0.05, deck, zc + span),
                    0.003,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
        // The case: as wide as the pallet, its lid level with the collider's top.
        let size = v3(depth - 0.04, b1.y - deck + 0.01, 2.0 * span - 0.04);
        let (body, guards) = case(size);
        let to = at(x, deck, zc);
        olive.append(&body, to);
        rubber.append(&guards, to);
        lid = b1.y;
    }
    wood.shade(|p, _| 0.75 + 0.25 * (pn(p.x * 6.0, p.z * 0.8, 94) * 0.5 + 0.5));
    kit.add(PLANKS, wood);
    kit.add(STEEL_OLIVE, olive);
    kit.add(RUBBER, rubber);
    // Two ratchet straps over both cases, down their outer ribs to the pallets, between the
    // handles and the latches.
    let mut strap = M::new();
    let mut alu = M::new();
    let t = 0.004;
    // The cases' outer rib faces.
    let (z0, z1) = (b0.z + 0.028, b1.z - 0.028);
    for dx in [-0.22f32, 0.22] {
        let sx = x + dx;
        strap.append(
            &bx(v3(sx - 0.025, lid, z0 - t), v3(sx + 0.025, lid + t, z1 + t)),
            Mat4::IDENTITY,
        );
        for (z, out) in [(z0, -t), (z1, t)] {
            strap.append(
                &bx(
                    v3(sx - 0.025, deck - 0.02, z.min(z + out)),
                    v3(sx + 0.025, lid + t, z.max(z + out)),
                ),
                Mat4::IDENTITY,
            );
        }
        alu.append(
            &bx(
                v3(sx - 0.035, deck + 0.3, z1 + t),
                v3(sx + 0.035, deck + 0.42, z1 + t + 0.022),
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(STEEL_ORANGE, strap);
    kit.add(ALU, alu);
}

/// A backlit "10 VAULT" sign on two legs by the start, readable from the runway and the ring.
fn sign(kit: &mut Kit) {
    let (_, p1) = bounds(layout::VAULT_PAD);
    let mut alu = M::new();
    let mut face = M::new();
    let (_, _, aspect) = sign_rect(SIGN_VAULT);
    let sh = 0.42;
    let sw = sh * aspect;
    let x = layout::VAULT_START[0] as f32 + 1.2;
    let c = v3(x, floor() + 1.75, p1.z + 0.35);
    alu.append(
        &slab(
            v3(c.x - sw * 0.5 - 0.03, c.y - sh * 0.5 - 0.03, c.z - 0.05),
            v3(c.x + sw * 0.5 + 0.03, c.y + sh * 0.5 + 0.03, c.z + 0.05),
            0.008,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for dx in [-sw * 0.35, sw * 0.35] {
        alu.append(
            &slab(
                v3(c.x + dx - 0.025, 0.0, c.z - 0.025),
                v3(c.x + dx + 0.025, c.y - sh * 0.5 - 0.03, c.z + 0.025),
                0.003,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        alu.append(
            &slab(
                v3(c.x + dx - 0.09, 0.0, c.z - 0.09),
                v3(c.x + dx + 0.09, 0.015, c.z + 0.09),
                0.004,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    for side in [-1.0f32, 1.0] {
        let normal = Vec3::Z * side;
        let right = Vec3::Y.cross(normal).normalize();
        sign_face(
            &mut face,
            SIGN_VAULT,
            c + normal * 0.052,
            right,
            Vec3::Y,
            sh,
        );
    }
    kit.add(ALU, alu);
    kit.add(SIGN, face);
}
