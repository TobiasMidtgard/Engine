//! The cart track and the cart: rails with a real rail profile on tie plates and spikes,
//! timber sleepers bedded in ballast, fishplated joints and buffer stops; and a tipping skip
//! wagon (rounded steel tub with a rolled rim, bands and a gravel load, on a channel-steel
//! chassis with leaf springs, axle boxes, buffers and couplings) on two turning wheelsets.
//!
//! Track parts are in range space; cart parts are in cart space (x along the track, z across,
//! y = 0 at the rail top, centered between the axles).

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::ground::{PAD_TOP, wfbm, wnoise};
use super::kit::{self, at, tr};
use super::tex::smoothstep;

/// Height of the rail tops, m.
pub const RAIL_TOP: f32 = 0.524;
/// Rail head centers either side of the track's center line, m.
pub const GAUGE_HALF: f32 = 0.7525;
/// Wheel radius at the tread, m.
pub const WHEEL_R: f32 = 0.3;
/// Wheelsets either side of the cart's center, m.
pub const AXLE_X: f32 = 0.7;

const BALLAST_TOP: f32 = PAD_TOP + 0.17;
const SLEEPER_TOP: f32 = 0.34;
const RAIL_BASE: f32 = 0.356;

fn rail_profile() -> Vec<Vec2> {
    let half = [
        (0.075, 0.0, 0.003),
        (0.075, 0.011, 0.003),
        (0.03, 0.025, 0.01),
        (0.0095, 0.04, 0.012),
        (0.0085, 0.11, 0.012),
        (0.014, 0.126, 0.008),
        (0.036, 0.133, 0.004),
        (0.037, 0.157, 0.006),
        (0.03, 0.168, 0.008),
    ];
    let mut pts = Vec::new();
    let mut radii = Vec::new();
    pts.push(Vec2::new(-0.075, 0.0));
    radii.push(0.003);
    for &(x, y, r) in &half {
        pts.push(Vec2::new(x, y));
        radii.push(r);
    }
    for &(x, y, r) in half.iter().rev() {
        if y == 0.0 {
            continue;
        }
        pts.push(Vec2::new(-x, y));
        radii.push(r);
    }
    // The loop now starts at (-0.075, 0), runs along the base to +x, up the right side,
    // across the head and down the left side back to (-0.075, 0.011).
    pts.dedup_by(|a, b| (*a - *b).length_squared() < 1e-12);
    radii.truncate(pts.len());
    kit::fillet_each(&pts, &radii, 3)
}

/// A rail along +X from x0 to x1 standing on y = 0, centered on z = 0.
fn rail(x0: f32, x1: f32) -> Mesh {
    let m = kit::extrude_ends(&rail_profile(), x0, x1, 40.0, 0.002, 0.002);
    // Extruded along +Z with the profile in (x, y): turn so +Z becomes +X.
    let m = kit::placed(m, tr(Vec3::ZERO, Quat::from_rotation_y(FRAC_PI_2)));
    // The rotation maps z to x and x to -z; flip the x range back.
    let mut m = m;
    for v in &mut m.vertices {
        v.uv = [v.uv[1], v.uv[0]];
    }
    m
}

/// The track: ballast bed, sleepers, tie plates and spikes, rails with joints, and buffer
/// stops, centered on z = `tz` from x = -`half` to `half`.
pub fn track(parts: &mut Parts, tz: f32, half: f32) {
    let mut rng = Rng::new(0x7_7AC);
    // Ballast: a lumpy trapezoid bed.
    let (bx, bz) = (half + 1.0, 2.0);
    let bed = kit::surface(
        (bx * 2.0 / 0.15) as u32,
        (bz * 2.0 / 0.1) as u32,
        false,
        |u, v| {
            // x from +bx to -bx so the bed faces up.
            let x = bx - u * bx * 2.0;
            let z = -bz + v * bz * 2.0;
            let edge = smoothstep(1.45, 2.0, z.abs()).max(smoothstep(half + 0.3, bx, x.abs()));
            let top =
                BALLAST_TOP + wnoise(x, z, 0.35, 401) * 0.012 + wnoise(x, z, 0.12, 403) * 0.006;
            let y = top + (PAD_TOP - 0.008 - top) * edge;
            let z_w = tz + z;
            (Vec3::new(x, y, z_w), Vec2::new(x, z_w))
        },
    );
    let mut bed = bed;
    kit::shade(&mut bed, |p, _| {
        // Oil and rust stains between the rails, dust at the edges.
        let between = 1.0 - smoothstep(0.6, 1.0, (p.z - tz).abs());
        let oil = smoothstep(0.1, 0.6, wfbm(p.x, p.z, 1.5, 409)) * between * 0.35;
        Vec3::ONE.lerp(Vec3::new(0.55, 0.45, 0.38), oil)
            * (0.92 + 0.08 * wnoise(p.x, p.z, 3.0, 411))
    });
    parts.add(art::GRAVEL, bed);

    // Sleepers with tie plates and spikes.
    let spacing = 0.62;
    let n = ((half * 2.0 - 0.6) / spacing) as i32;
    let x0 = -spacing * n as f32 / 2.0;
    for k in 0..=n {
        let x = x0 + spacing * k as f32 + rng.range(-0.025, 0.025);
        let yaw = rng.range(-0.02, 0.02);
        let sl = kit::rounded_box(Vec3::new(0.24, 0.16, 2.6), 0.014, 2, 0.4);
        let sl = kit::offset_uv(sl, Vec2::new(rng.range(0.0, 1.0), rng.range(0.0, 1.0)));
        let t = tr(
            Vec3::new(x, SLEEPER_TOP - 0.08 + rng.range(-0.006, 0.004), tz),
            Quat::from_rotation_y(yaw) * Quat::from_rotation_x(rng.range(-0.006, 0.006)),
        );
        let tone = rng.range(0.75, 1.15);
        let mut m = kit::tint(
            kit::placed(sl, t),
            Vec3::new(tone, tone * 0.97, tone * 0.92),
        );
        kit::ground_grime(&mut m, BALLAST_TOP, 0.06, 0.6, Vec3::new(0.85, 0.8, 0.72));
        parts.add(art::SLEEPER, m);
        for side in [-1.0f32, 1.0] {
            let rz = tz + side * GAUGE_HALF;
            let plate = kit::rounded_box(Vec3::new(0.19, 0.016, 0.36), 0.004, 1, 0.2);
            parts.add_at(art::RUST, plate, at(Vec3::new(x, SLEEPER_TOP + 0.008, rz)));
            for (dx, dz) in [(-0.05f32, -0.1f32), (0.05, 0.1), (0.05, -0.1), (-0.05, 0.1)] {
                let head = kit::rounded_box(Vec3::new(0.018, 0.022, 0.018), 0.004, 1, 0.1);
                let lip = kit::rounded_box(Vec3::new(0.018, 0.008, 0.036), 0.003, 1, 0.1);
                let base = Vec3::new(x + dx, SLEEPER_TOP + 0.016, rz + dz);
                parts.add_at(art::RUST, head, at(base + Vec3::new(0.0, 0.011, 0.0)));
                parts.add_at(
                    art::RUST,
                    lip,
                    at(base + Vec3::new(0.0, 0.018, -dz.signum() * 0.012)),
                );
            }
        }
    }

    // Rails in three lengths with fishplated joints.
    let joints = [-half * 0.33, half * 0.33];
    let mut cuts = vec![-half];
    cuts.extend_from_slice(&joints);
    cuts.push(half);
    for side in [-1.0f32, 1.0] {
        let rz = tz + side * GAUGE_HALF;
        for w in cuts.windows(2) {
            let (a, b) = (w[0] + 0.003, w[1] - 0.003);
            let mut r = kit::placed(rail(a, b), at(Vec3::new(0.0, RAIL_BASE, rz)));
            r = kit::offset_uv(r, Vec2::new(rng.f32(), 0.0));
            parts.add(art::RUST, r);
            // Polished running band on the head.
            let band = kit::rounded_box(Vec3::new(b - a, 0.0016, 0.046), 0.0006, 1, 2.0);
            parts.add_at(
                art::STEEL,
                band,
                at(Vec3::new((a + b) * 0.5, RAIL_BASE + 0.1683, rz)),
            );
        }
        for &jx in &joints {
            for s in [-1.0f32, 1.0] {
                let bar = kit::rounded_box(Vec3::new(0.62, 0.075, 0.018), 0.005, 2, 0.2);
                parts.add_at(
                    art::RUST,
                    bar,
                    at(Vec3::new(jx, RAIL_BASE + 0.075, rz + s * 0.019)),
                );
                for b in 0..4 {
                    let bx = jx - 0.225 + b as f32 * 0.15;
                    parts.add_at(
                        art::RUST,
                        kit::hex_bolt(0.016),
                        tr(
                            Vec3::new(bx, RAIL_BASE + 0.075, rz + s * 0.028),
                            Quat::from_rotation_x(s * FRAC_PI_2),
                        ),
                    );
                }
            }
        }
    }

    // Buffer stops at both ends.
    for end in [-1.0f32, 1.0] {
        let x = end * (half + 0.35);
        for side in [-1.0f32, 1.0] {
            let z = tz + side * GAUGE_HALF;
            // Posts made of rail.
            let post = kit::placed(
                rail(0.0, 0.95),
                tr(Vec3::new(x, RAIL_BASE, z), Quat::from_rotation_z(FRAC_PI_2)),
            );
            parts.add(art::RUST, post);
            // Raking strut back to the rail.
            let strut = rail(0.0, 1.25);
            let lean = 0.62;
            parts.add_at(
                art::RUST,
                strut,
                tr(
                    Vec3::new(x + end * 0.95, RAIL_BASE + 0.17, z),
                    Quat::from_rotation_z(if end > 0.0 { PI - lean } else { lean }),
                ),
            );
        }
        // Timber beam faced with a red and white board.
        let beam = kit::rounded_box(Vec3::new(0.3, 0.3, 2.3), 0.02, 2, 0.4);
        parts.add_at(
            art::TIMBER,
            beam,
            at(Vec3::new(x - end * 0.12, RAIL_TOP + 0.42, tz)),
        );
        for k in 0..5 {
            let z = tz - 0.92 + k as f32 * 0.46;
            let mat = if k % 2 == 0 { art::RED } else { art::LIGHT };
            let board = kit::rounded_box(Vec3::new(0.03, 0.26, 0.45), 0.006, 1, 0.2);
            parts.add_at(
                mat,
                board,
                at(Vec3::new(x - end * 0.285, RAIL_TOP + 0.42, z)),
            );
        }
    }
}

/// The cart's fixed parts: tub, load, chassis, axle boxes, springs, buffers, couplings.
pub fn cart_body() -> Parts {
    let mut p = Parts::default();
    // Tub cross-section (z, y): a half round bottom with flared sides.
    let (cy, r) = (1.38f32, 0.6f32);
    let (top_y, top_z) = (1.95f32, 0.86f32);
    let mut outer: Vec<Vec2> = Vec::new();
    outer.push(Vec2::new(-top_z, top_y));
    for i in 0..=24 {
        let a = PI + i as f32 / 24.0 * PI;
        outer.push(Vec2::new(a.cos() * r, cy + a.sin() * r));
    }
    outer.push(Vec2::new(top_z, top_y));
    let t = 0.007;
    let inner: Vec<Vec2> = outer
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let a = outer[i.saturating_sub(1)];
            let b = outer[(i + 1).min(outer.len() - 1)];
            let d = (b - a).normalize_or(Vec2::X);
            *q + Vec2::new(-d.y, d.x) * t
        })
        .collect();
    let mut shell = outer.clone();
    shell.extend(inner.iter().rev());
    let len = 2.1;
    let tub = kit::extrude_caps(&shell, -len / 2.0, len / 2.0, 30.0, 0.0, true, true);
    // Extruded along z with the section in (z, y): turn so the length runs along x.
    let along_x = |m: Mesh| kit::placed(m, tr(Vec3::ZERO, Quat::from_rotation_y(FRAC_PI_2)));
    // Grime: mud caked on the lower tub, rust weeping from under the rim.
    let mut tub = along_x(tub);
    let grime = |m: &mut Mesh| {
        kit::shade(m, |q, _| {
            let mud = smoothstep(1.25, 0.8, q.y);
            let weep = smoothstep(1.7, 1.93, q.y) * 0.5;
            Vec3::ONE
                .lerp(Vec3::new(0.55, 0.46, 0.36), mud * 0.8)
                .lerp(Vec3::new(0.62, 0.42, 0.3), weep)
        })
    };
    grime(&mut tub);
    p.add(art::CART, tub);
    // End plates.
    let mut plate = outer.clone();
    plate.dedup_by(|a, b| (*a - *b).length_squared() < 1e-10);
    for s in [-1.0f32, 1.0] {
        let m = kit::extrude(&plate, -0.005, 0.005, 30.0, 0.002);
        let mut m = kit::placed(along_x(m), at(Vec3::new(s * (len / 2.0 - 0.006), 0.0, 0.0)));
        grime(&mut m);
        p.add(art::CART, m);
    }
    // Rolled rim around the mouth.
    let mut rim = Vec::new();
    let (hx, hz) = (len / 2.0, top_z + 0.004);
    let rc = 0.05;
    for (cx, cz, a0) in [
        (hx - rc, hz - rc, 0.0f32),
        (-(hx - rc), hz - rc, FRAC_PI_2),
        (-(hx - rc), -(hz - rc), PI),
        (hx - rc, -(hz - rc), PI * 1.5),
    ] {
        for i in 0..=4 {
            let a = a0 + i as f32 / 4.0 * FRAC_PI_2;
            rim.push(Vec3::new(
                cx + a.cos() * rc,
                top_y + 0.01,
                cz + a.sin() * rc,
            ));
        }
    }
    rim.dedup_by(|a, b| (*a - *b).length_squared() < 1e-10);
    p.add(art::CART, kit::tube(&rim, 0.022, 10, true));
    // Bands around the tub.
    let band_sec = kit::fillet(
        &[
            Vec2::new(-0.03, -0.006),
            Vec2::new(0.03, -0.006),
            Vec2::new(0.03, 0.006),
            Vec2::new(-0.03, 0.006),
        ],
        0.004,
        2,
    );
    for bx in [-0.62f32, 0.0, 0.62] {
        let path: Vec<Vec3> = outer
            .iter()
            .map(|q| {
                let n = (*q - Vec2::new(0.0, cy)).normalize_or(Vec2::Y);
                let q = *q + n * 0.008;
                Vec3::new(bx, q.y - 0.03 * (q.y > top_y - 0.01) as i32 as f32, q.x)
            })
            .collect();
        p.add(art::CART, kit::sweep(&path, &band_sec, false, 40.0));
    }
    // Lifting eyes at the corners.
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let eye = kit::ring(0.035, 0.009, 16, 8);
        p.add(
            art::DARK,
            kit::placed(
                eye,
                tr(
                    Vec3::new(sx * (len / 2.0 + 0.03), top_y - 0.06, sz * 0.62),
                    Quat::from_rotation_z(FRAC_PI_2),
                ),
            ),
        );
    }
    // Gravel load heaped in the tub.
    let load = kit::surface(40, 28, false, |u, v| {
        let x = len / 2.0 - 0.02 - u * (len - 0.04);
        let z = -0.74 + v * 1.48;
        let mound = (1.0 - (x / 1.05).powi(4)) * (1.0 - (z / 0.75).powi(2)).max(0.0);
        let y =
            1.68 + mound * 0.36 + wnoise(x, z, 0.18, 421) * 0.025 + wnoise(x, z, 0.06, 423) * 0.008;
        (Vec3::new(x, y, z), Vec2::new(x * 0.7, z * 0.7))
    });
    p.add(art::GRAVEL, load);
    // Rocker plates under the tub.
    for rx in [-0.78f32, 0.78] {
        let mut rocker = Vec::new();
        for i in 0..=12 {
            let a = PI * 1.5 - 0.9 + i as f32 / 12.0 * 1.8;
            rocker.push(Vec2::new(a.cos() * (r + 0.01), cy + a.sin() * (r + 0.01)));
        }
        rocker.push(Vec2::new(0.42, 0.72));
        rocker.push(Vec2::new(-0.42, 0.72));
        let m = kit::extrude(&rocker, -0.012, 0.012, 35.0, 0.003);
        p.add(
            art::DARK,
            kit::placed(along_x(m), at(Vec3::new(rx, 0.0, 0.0))),
        );
    }

    // Chassis: channel-steel side members and end beams.
    let channel = |w: f32, h: f32| {
        kit::fillet(
            &[
                Vec2::new(0.0, -h / 2.0),
                Vec2::new(w, -h / 2.0),
                Vec2::new(w, -h / 2.0 + 0.011),
                Vec2::new(0.011, -h / 2.0 + 0.011),
                Vec2::new(0.011, h / 2.0 - 0.011),
                Vec2::new(w, h / 2.0 - 0.011),
                Vec2::new(w, h / 2.0),
                Vec2::new(0.0, h / 2.0),
            ],
            0.003,
            1,
        )
    };
    let frame_y = 0.56;
    for s in [-1.0f32, 1.0] {
        // Opening inward.
        let sec: Vec<Vec2> = channel(0.075, 0.2)
            .iter()
            .map(|q| Vec2::new(-q.x * s, q.y))
            .collect();
        let m = kit::extrude(&sec, -1.38, 1.38, 30.0, 0.003);
        p.add(
            art::DARK,
            kit::placed(along_x(m), at(Vec3::new(0.0, frame_y, s * 0.94))),
        );
    }
    for s in [-1.0f32, 1.0] {
        let sec: Vec<Vec2> = channel(0.08, 0.24)
            .iter()
            .map(|q| Vec2::new(-q.x * s, q.y))
            .collect();
        let m = kit::extrude(&sec, -1.02, 1.02, 30.0, 0.003);
        // Along z (no turn); opening inward along x.
        let m = kit::placed(
            m,
            tr(
                Vec3::new(s * 1.4, frame_y, 0.0),
                Quat::from_rotation_y(FRAC_PI_2),
            ),
        );
        p.add(art::DARK, m);
        // Buffers.
        for bz in [-0.62f32, 0.62] {
            let flange = kit::rounded_box(Vec3::new(0.02, 0.2, 0.2), 0.004, 1, 0.1);
            p.add(
                art::DARK,
                kit::placed(flange, at(Vec3::new(s * 1.41, frame_y, bz))),
            );
            let body = [
                Vec2::new(0.0, 0.0),
                Vec2::new(0.075, 0.0),
                Vec2::new(0.075, 0.15),
                Vec2::new(0.06, 0.16),
                Vec2::new(0.052, 0.16),
                Vec2::new(0.052, 0.24),
                Vec2::new(0.0, 0.24),
            ];
            let rot = Quat::from_rotation_z(-s * FRAC_PI_2);
            p.add(
                art::DARK,
                kit::placed(
                    kit::lathe(&body, 24, 35.0, false),
                    tr(Vec3::new(s * 1.42, frame_y, bz), rot),
                ),
            );
            let head = [
                Vec2::new(0.0, 0.0),
                Vec2::new(0.115, 0.0),
                Vec2::new(0.12, 0.006),
                Vec2::new(0.12, 0.02),
                Vec2::new(0.11, 0.028),
                Vec2::new(0.06, 0.032),
                Vec2::new(0.0, 0.033),
            ];
            p.add(
                art::STEEL,
                kit::placed(
                    kit::lathe(&head, 32, 30.0, false),
                    tr(Vec3::new(s * 1.66, frame_y, bz), rot),
                ),
            );
            for k in 0..4 {
                let a = k as f32 / 4.0 * TAU + PI / 4.0;
                p.add(
                    art::GALV,
                    kit::placed(
                        kit::hex_bolt(0.012),
                        tr(
                            Vec3::new(s * 1.42, frame_y + a.sin() * 0.075, bz + a.cos() * 0.075),
                            rot,
                        ),
                    ),
                );
            }
        }
        // Coupling hook and a short chain.
        let hook: Vec<Vec3> = (0..=12)
            .map(|i| {
                let t = i as f32 / 12.0;
                if t < 0.5 {
                    Vec3::new(s * (1.42 + 0.3 * t), frame_y - 0.02, 0.0)
                } else {
                    let a = (t - 0.5) * 2.0 * PI;
                    Vec3::new(
                        s * (1.57 + 0.04 * a.sin()),
                        frame_y - 0.06 + 0.04 * a.cos(),
                        0.0,
                    )
                }
            })
            .collect();
        p.add(art::RUST, kit::tube(&hook, 0.016, 10, false));
        for k in 0..3 {
            let link = kit::ring(0.035, 0.008, 16, 6);
            let rot = if k % 2 == 0 {
                Quat::from_rotation_x(FRAC_PI_2)
            } else {
                Quat::IDENTITY
            };
            // Oval links.
            let oval = Mat4::from_scale(Vec3::new(1.5, 1.0, 1.0));
            p.add(
                art::RUST,
                kit::placed(
                    link,
                    at(Vec3::new(
                        s * (1.62 + k as f32 * 0.075),
                        frame_y - 0.1 - k as f32 * 0.03,
                        0.0,
                    )) * oval
                        * Mat4::from_quat(rot),
                ),
            );
        }
    }
    // Cross members and cradle beams.
    for x in [-0.36f32, 0.36] {
        p.add(
            art::DARK,
            kit::placed(
                kit::rounded_box(Vec3::new(0.1, 0.12, 1.8), 0.008, 2, 0.3),
                at(Vec3::new(x, frame_y, 0.0)),
            ),
        );
    }
    for x in [-0.78f32, 0.78] {
        p.add(
            art::DARK,
            kit::placed(
                kit::rounded_box(Vec3::new(0.14, 0.08, 1.82), 0.008, 2, 0.3),
                at(Vec3::new(x, 0.68, 0.0)),
            ),
        );
    }
    // Axle boxes with horn guides, leaf springs on top.
    for ax in [-AXLE_X, AXLE_X] {
        for s in [-1.0f32, 1.0] {
            let z = s * 0.93;
            p.add(
                art::DARK,
                kit::placed(
                    kit::rounded_box(Vec3::new(0.2, 0.2, 0.15), 0.02, 3, 0.1),
                    at(Vec3::new(ax, WHEEL_R, z)),
                ),
            );
            let cap = [
                Vec2::new(0.0, 0.0),
                Vec2::new(0.06, 0.0),
                Vec2::new(0.06, 0.02),
                Vec2::new(0.045, 0.035),
                Vec2::new(0.0, 0.035),
            ];
            p.add(
                art::DARK,
                kit::placed(
                    kit::lathe(&cap, 20, 35.0, false),
                    tr(
                        Vec3::new(ax, WHEEL_R, z + s * 0.075),
                        Quat::from_rotation_x(s * FRAC_PI_2),
                    ),
                ),
            );
            for gx in [-0.125f32, 0.125] {
                p.add(
                    art::DARK,
                    kit::placed(
                        kit::rounded_box(Vec3::new(0.025, 0.3, 0.13), 0.005, 1, 0.2),
                        at(Vec3::new(ax + gx, WHEEL_R + 0.08, z)),
                    ),
                );
            }
            // Leaf spring: six leaves, each shorter, bowed.
            for leaf in 0..6 {
                let half = 0.42 - leaf as f32 * 0.06;
                let path: Vec<Vec3> = (0..=10)
                    .map(|i| {
                        let t = i as f32 / 10.0 * 2.0 - 1.0;
                        Vec3::new(
                            ax + t * half,
                            WHEEL_R + 0.13 + leaf as f32 * 0.011 - 0.035 * (1.0 - t * t),
                            z,
                        )
                    })
                    .collect();
                let sec = [
                    Vec2::new(-0.005, -0.03),
                    Vec2::new(0.005, -0.03),
                    Vec2::new(0.005, 0.03),
                    Vec2::new(-0.005, 0.03),
                ];
                let sec: Vec<Vec2> = sec.iter().map(|q| Vec2::new(q.y, q.x)).collect();
                p.add(
                    art::DARK,
                    kit::sweep(&path, &kit::fillet(&sec, 0.002, 1), false, 40.0),
                );
            }
            p.add(
                art::DARK,
                kit::placed(
                    kit::rounded_box(Vec3::new(0.07, 0.1, 0.075), 0.006, 2, 0.1),
                    at(Vec3::new(ax, WHEEL_R + 0.13, z)),
                ),
            );
        }
    }
    // Tipping lever at one end.
    p.add(
        art::RED,
        kit::rod(
            Vec3::new(1.12, 1.45, -0.55),
            Vec3::new(1.25, 1.9, -0.62),
            0.014,
            10,
        ),
    );
    p.add(
        art::RUBBER,
        kit::placed(
            kit::rounded_box(Vec3::new(0.04, 0.11, 0.04), 0.015, 2, 0.1),
            tr(Vec3::new(1.265, 1.97, -0.625), Quat::from_rotation_z(-0.28)),
        ),
    );
    p.shade(|_, n| Vec3::splat(1.0 - n.y.min(0.0).abs() * 0.25));
    p
}

/// One wheelset in its own space (the axle along z, center at the origin): two flanged,
/// dished wheels on an axle, with hub bolts. The treads are polished by the rails.
pub fn wheelset() -> Parts {
    let mut p = Parts::default();
    let wheel = [
        Vec2::new(0.06, -0.065),
        Vec2::new(0.10, -0.065),
        Vec2::new(0.112, -0.055),
        Vec2::new(0.125, -0.022),
        Vec2::new(0.24, -0.032),
        Vec2::new(0.256, -0.046),
        Vec2::new(0.272, -0.056),
        Vec2::new(0.3, -0.058),
        Vec2::new(0.324, -0.053),
        Vec2::new(0.333, -0.043),
        Vec2::new(0.331, -0.033),
        Vec2::new(0.312, -0.026),
        Vec2::new(0.302, -0.017),
        Vec2::new(0.301, 0.0),
        Vec2::new(0.2975, 0.045),
        Vec2::new(0.292, 0.052),
        Vec2::new(0.27, 0.052),
        Vec2::new(0.255, 0.04),
        Vec2::new(0.24, 0.002),
        Vec2::new(0.125, 0.012),
        Vec2::new(0.112, 0.045),
        Vec2::new(0.1, 0.06),
        Vec2::new(0.06, 0.06),
    ];
    let tread = [
        Vec2::new(0.3022, -0.018),
        Vec2::new(0.3016, 0.0),
        Vec2::new(0.2981, 0.044),
    ];
    for s in [-1.0f32, 1.0] {
        // Lathe axis +Y -> +Z for the +z wheel (flange inside), mirrored for the other.
        let rot = Quat::from_rotation_x(s * FRAC_PI_2);
        let t = tr(Vec3::new(0.0, 0.0, s * GAUGE_HALF), rot);
        p.add(art::RUST, kit::placed(kit::lathe_loop(&wheel, 64, 40.0), t));
        p.add(
            art::STEEL,
            kit::placed(kit::lathe(&tread, 64, 40.0, false), t),
        );
        for k in 0..6 {
            let a = k as f32 / 6.0 * TAU;
            let bolt = kit::hex_bolt(0.013);
            let pos = Vec3::new(a.cos() * 0.085, a.sin() * 0.085, s * (GAUGE_HALF + 0.06));
            p.add(art::DARK, kit::placed(bolt, tr(pos, rot)));
        }
    }
    p.add(
        art::DARK,
        kit::rod(
            Vec3::new(0.0, 0.0, -0.95),
            Vec3::new(0.0, 0.0, 0.95),
            0.055,
            20,
        ),
    );
    p
}
