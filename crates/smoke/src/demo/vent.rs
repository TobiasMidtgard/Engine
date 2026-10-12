//! The steam vent (a flanged riser on a concrete plinth, fed by a lagged pipe through a gate
//! valve with a red handwheel, with a pressure gauge) and the smoke grenades (spent canisters
//! lying on the pad, banded in their smoke's color, their levers and pins thrown nearby).

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::ground::PAD_TOP;
use super::kit::{self, at, tr};

/// A pipe flange (raised face, bolt circle) on +Y from y = 0, `t` thick.
fn flange(r: f32, pipe_r: f32, t: f32) -> Mesh {
    kit::lathe(
        &[
            Vec2::new(pipe_r, 0.0),
            Vec2::new(r - 0.004, 0.0),
            Vec2::new(r, 0.004),
            Vec2::new(r, t - 0.004),
            Vec2::new(r - 0.004, t),
            Vec2::new(pipe_r + 0.02, t),
            Vec2::new(pipe_r + 0.012, t + 0.03),
            Vec2::new(pipe_r, t + 0.04),
        ],
        40,
        35.0,
        false,
    )
}

/// A ring of `n` bolts at radius `r` on a flange face at height `y` (heads up or down).
fn bolt_circle(parts: &mut Parts, c: Vec3, r: f32, n: u32, up: bool, size: f32) {
    for k in 0..n {
        let a = (k as f32 + 0.5) / n as f32 * TAU;
        let rot = if up {
            Quat::IDENTITY
        } else {
            Quat::from_rotation_x(PI)
        };
        parts.add_at(
            art::GALV,
            kit::hex_bolt(size),
            tr(c + Vec3::new(a.cos() * r, 0.0, a.sin() * r), rot),
        );
    }
}

/// The vent with its outlet rim at `top` (range space).
pub fn vent(parts: &mut Parts, top: Vec3, pad_edge_x: f32) {
    let c = Vec3::new(top.x, 0.0, top.z);
    let plinth_top = PAD_TOP + 0.16;
    let mut plinth = kit::rounded_box(Vec3::new(0.9, 0.2, 0.9), 0.025, 3, 0.3);
    kit::ground_grime(&mut plinth, -0.1, 0.2, 0.7, Vec3::new(0.9, 0.85, 0.75));
    parts.add_at(art::PRECAST, plinth, at(c + Vec3::Y * (plinth_top - 0.1)));
    let pr = 0.0845;
    // Base flange bolted to the plinth.
    parts.add_at(
        art::RUST,
        flange(0.15, pr, 0.026),
        at(c + Vec3::Y * plinth_top),
    );
    bolt_circle(
        parts,
        c + Vec3::Y * (plinth_top + 0.026),
        0.12,
        8,
        true,
        0.016,
    );
    // Riser with a tee for the supply branch.
    let riser = [Vec2::new(pr, plinth_top), Vec2::new(pr, top.y - 0.12)];
    parts.add_at(art::RUST, kit::lathe(&riser, 40, 35.0, false), at(c));
    // Mid flange pair.
    let fy = top.y - 0.19;
    parts.add_at(art::RUST, flange(0.14, pr, 0.022), at(c + Vec3::Y * fy));
    parts.add_at(
        art::RUST,
        kit::placed(
            flange(0.14, pr, 0.022),
            tr(Vec3::ZERO, Quat::from_rotation_x(PI)),
        ),
        at(c + Vec3::Y * (fy + 0.046)),
    );
    bolt_circle(parts, c + Vec3::Y * (fy + 0.046), 0.112, 8, true, 0.014);
    bolt_circle(parts, c + Vec3::Y * fy, 0.112, 8, false, 0.014);
    // Bell mouth, sooty inside.
    let bell = [
        Vec2::new(pr, top.y - 0.12),
        Vec2::new(pr + 0.004, top.y - 0.06),
        Vec2::new(0.1, top.y - 0.02),
        Vec2::new(0.112, top.y - 0.004),
        Vec2::new(0.11, top.y),
        Vec2::new(0.1, top.y),
        Vec2::new(0.078, top.y - 0.04),
        Vec2::new(0.074, top.y - 0.12),
        Vec2::new(0.0, top.y - 0.14),
    ];
    let mut bell = kit::lathe(&bell, 48, 35.0, false);
    kit::shade(&mut bell, |p, n| {
        let radial = Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X);
        if n.dot(radial) < -0.2 || n.y > 0.9 {
            Vec3::splat(0.25)
        } else {
            Vec3::ONE
        }
    });
    parts.add_at(art::RUST, bell, at(c));
    // Supply branch toward the pad edge (+x), at the tee.
    let by = plinth_top + 0.13;
    let branch_r = 0.06;
    let along_x = Quat::from_rotation_z(-FRAC_PI_2);
    let pipe = |x0: f32, x1: f32, r: f32| {
        kit::placed(
            kit::lathe(&[Vec2::new(r, 0.0), Vec2::new(r, x1 - x0)], 32, 35.0, false),
            tr(Vec3::new(c.x + x0, by, c.z), along_x),
        )
    };
    parts.add(art::RUST, pipe(0.0, 0.25, branch_r));
    // Gate valve: flanged body, bonnet, stem and handwheel.
    let vx = 0.36;
    for (x, flip) in [(0.25f32, false), (0.47, true)] {
        let f = kit::placed(
            flange(0.11, branch_r, 0.02),
            tr(
                Vec3::ZERO,
                if flip {
                    Quat::from_rotation_z(FRAC_PI_2)
                } else {
                    along_x
                },
            ),
        );
        parts.add_at(art::DARK, f, at(Vec3::new(c.x + x, by, c.z)));
    }
    let body = [
        Vec2::new(0.0, -0.09),
        Vec2::new(0.07, -0.09),
        Vec2::new(0.1, -0.06),
        Vec2::new(0.11, 0.0),
        Vec2::new(0.1, 0.06),
        Vec2::new(0.065, 0.1),
        Vec2::new(0.065, 0.2),
        Vec2::new(0.085, 0.21),
        Vec2::new(0.085, 0.24),
        Vec2::new(0.03, 0.25),
        Vec2::new(0.0, 0.25),
    ];
    parts.add_at(
        art::DARK,
        kit::lathe(&body, 32, 35.0, false),
        at(Vec3::new(c.x + vx, by, c.z)),
    );
    parts.add_at(
        art::DARK,
        kit::lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.065, 0.0),
                Vec2::new(0.065, 0.06),
                Vec2::new(0.0, 0.06),
            ],
            32,
            35.0,
            false,
        ),
        tr(Vec3::new(c.x + vx - 0.12, by, c.z), along_x),
    );
    let stem_top = by + 0.42;
    parts.add(
        art::STEEL,
        kit::rod(
            Vec3::new(c.x + vx, by + 0.25, c.z),
            Vec3::new(c.x + vx, stem_top + 0.04, c.z),
            0.012,
            12,
        ),
    );
    // Handwheel: rim, five spokes, hub.
    let wheel_y = stem_top - 0.02;
    parts.add_at(
        art::RED,
        kit::ring(0.15, 0.012, 48, 10),
        at(Vec3::new(c.x + vx, wheel_y, c.z)),
    );
    for k in 0..5 {
        let a = k as f32 / 5.0 * TAU;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let path: Vec<Vec3> = (0..=6)
            .map(|i| {
                let t = i as f32 / 6.0;
                Vec3::new(c.x + vx, wheel_y + 0.03 * (1.0 - t), c.z) + d * (0.025 + 0.125 * t)
            })
            .collect();
        parts.add(art::RED, kit::tube(&path, 0.008, 8, false));
    }
    parts.add_at(
        art::RED,
        kit::lathe(
            &[
                Vec2::new(0.0, -0.01),
                Vec2::new(0.032, -0.01),
                Vec2::new(0.032, 0.04),
                Vec2::new(0.0, 0.04),
            ],
            24,
            35.0,
            false,
        ),
        at(Vec3::new(c.x + vx, wheel_y, c.z)),
    );
    parts.add_at(
        art::GALV,
        kit::lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.02, 0.0),
                Vec2::new(0.02, 0.02),
                Vec2::new(0.0, 0.02),
            ],
            6,
            30.0,
            true,
        ),
        at(Vec3::new(c.x + vx, wheel_y + 0.04, c.z)),
    );
    // Lagged pipe (aluminum cladding with bands) to the pad edge, then down into the ground.
    let clad_r = 0.13;
    let x0 = 0.6;
    let x1 = pad_edge_x - c.x - 0.25;
    parts.add(art::RUST, pipe(0.47, x0, branch_r));
    let mut clad = pipe(x0, x1, clad_r);
    clad = kit::offset_uv(clad, Vec2::ZERO);
    parts.add(art::GALV, clad);
    for cap in [x0, x1] {
        parts.add_at(
            art::GALV,
            kit::lathe(
                &[Vec2::new(branch_r, 0.0), Vec2::new(clad_r, 0.0)],
                32,
                35.0,
                false,
            ),
            tr(
                Vec3::new(c.x + cap, by, c.z),
                if cap == x0 {
                    along_x
                } else {
                    Quat::from_rotation_z(FRAC_PI_2)
                },
            ),
        );
    }
    let mut x = x0 + 0.45;
    while x < x1 - 0.1 {
        parts.add_at(
            art::GALV,
            kit::ring(clad_r + 0.002, 0.004, 40, 6),
            tr(
                Vec3::new(c.x + x, by, c.z),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
        x += 0.9;
    }
    // Pipe supports.
    let mut x = x0 + 0.9;
    while x < x1 - 0.3 {
        let foot = Vec3::new(c.x + x, PAD_TOP, c.z);
        parts.add_at(
            art::DARK,
            kit::rounded_box(Vec3::new(0.06, by - clad_r - PAD_TOP, 0.06), 0.006, 1, 0.2),
            at(foot + Vec3::Y * ((by - clad_r - PAD_TOP) * 0.5)),
        );
        parts.add_at(
            art::DARK,
            kit::rounded_box(Vec3::new(0.2, 0.012, 0.2), 0.003, 1, 0.2),
            at(foot + Vec3::Y * 0.006),
        );
        let saddle = [
            Vec2::new(-0.14, -clad_r - 0.02),
            Vec2::new(0.14, -clad_r - 0.02),
            Vec2::new(0.14, -clad_r * 0.4),
            Vec2::new(0.12, -clad_r * 0.4),
            Vec2::new(0.12, -clad_r - 0.005),
            Vec2::new(-0.12, -clad_r - 0.005),
            Vec2::new(-0.12, -clad_r * 0.4),
            Vec2::new(-0.14, -clad_r * 0.4),
        ];
        let s = kit::extrude(&saddle, -0.03, 0.03, 30.0, 0.002);
        parts.add_at(
            art::DARK,
            s,
            tr(
                Vec3::new(c.x + x, by, c.z),
                Quat::from_rotation_y(FRAC_PI_2),
            ),
        );
        x += 1.5;
    }
    // Elbow down into the ground at the end.
    let elbow: Vec<Vec3> = (0..=12)
        .map(|i| {
            let a = i as f32 / 12.0 * FRAC_PI_2;
            Vec3::new(c.x + x1 + a.sin() * 0.22, by - 0.22 + a.cos() * 0.22, c.z)
        })
        .chain(std::iter::once(Vec3::new(
            c.x + x1 + 0.22,
            PAD_TOP - 0.1,
            c.z,
        )))
        .collect();
    parts.add(art::GALV, kit::tube(&elbow, clad_r, 32, false));
    // Pressure gauge on a nipple facing the range.
    let gy = top.y - 0.07;
    let nip_a = Vec3::new(c.x - pr, gy, c.z);
    let nip_b = Vec3::new(c.x - pr - 0.09, gy, c.z);
    parts.add(art::STEEL, kit::rod(nip_a, nip_b, 0.01, 10));
    let face = Quat::from_rotation_z(FRAC_PI_2);
    parts.add_at(
        art::DARK,
        kit::lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.05, 0.0),
                Vec2::new(0.052, 0.006),
                Vec2::new(0.052, 0.03),
                Vec2::new(0.046, 0.034),
                Vec2::new(0.046, 0.03),
                Vec2::new(0.0, 0.03),
            ],
            32,
            35.0,
            false,
        ),
        tr(nip_b, face),
    );
    parts.add_at(
        art::LIGHT,
        kit::lathe(
            &[Vec2::new(0.046, 0.0285), Vec2::new(0.0, 0.0285)],
            32,
            35.0,
            false,
        ),
        tr(nip_b, face),
    );
    let needle = kit::rounded_box(Vec3::new(0.003, 0.034, 0.002), 0.0008, 1, 0.1);
    parts.add_at(
        art::RED,
        needle,
        tr(
            nip_b + Vec3::new(-0.0295, 0.006, 0.006),
            Quat::from_rotation_y(-FRAC_PI_2) * Quat::from_rotation_z(0.6),
        ),
    );
}

/// A spent smoke grenade lying on the pad, in its own space (resting on y = `floor`,
/// centered on the origin, lying along `yaw`), with its band and top in `color` (linear).
pub fn grenade(color: Vec3, floor: f32, seed: u64) -> Parts {
    let mut rng = Rng::new(seed);
    let mut p = Parts::default();
    let r = 0.0318;
    // Upright: body from y = 0 to 0.114, fuze above.
    let body = [
        Vec2::new(0.0, 0.0),
        Vec2::new(r - 0.003, 0.0),
        Vec2::new(r, 0.003),
        Vec2::new(r, 0.111),
        Vec2::new(r - 0.003, 0.114),
        Vec2::new(0.0, 0.114),
    ];
    let mut can = kit::lathe(&body, 28, 35.0, false);
    // Sooty top and bottom where the smoke came out.
    kit::shade(&mut can, |q, _| {
        let top = ((q.y - 0.095) / 0.02).clamp(0.0, 1.0);
        let bottom = (1.0 - q.y / 0.012).clamp(0.0, 1.0);
        Vec3::splat(1.0 - top * 0.65 - bottom * 0.5)
    });
    let band = [Vec2::new(r + 0.0004, 0.084), Vec2::new(r + 0.0004, 0.106)];
    let band = kit::tint(kit::lathe(&band, 28, 35.0, false), color);
    let fuze = [
        Vec2::new(0.0, 0.114),
        Vec2::new(0.013, 0.114),
        Vec2::new(0.012, 0.122),
        Vec2::new(0.016, 0.126),
        Vec2::new(0.016, 0.142),
        Vec2::new(0.012, 0.146),
        Vec2::new(0.0, 0.147),
    ];
    let mut fuze = kit::lathe(&fuze, 16, 35.0, false);
    kit::shade(&mut fuze, |_, _| Vec3::splat(0.35));
    // Emission holes on the top face.
    let mut holes = Mesh::default();
    for k in 0..4 {
        let a = k as f32 / 4.0 * TAU + 0.4;
        let hole = kit::lathe(
            &[Vec2::new(0.004, 0.0), Vec2::new(0.0, 0.0)],
            10,
            35.0,
            false,
        );
        holes.merge(&kit::placed(
            hole,
            at(Vec3::new(a.cos() * 0.021, 0.1143, a.sin() * 0.021)),
        ));
    }
    let lie = tr(
        Vec3::new(-0.074, floor + r, 0.0),
        Quat::from_rotation_z(-FRAC_PI_2) * Quat::from_rotation_y(rng.range(0.0, TAU)),
    );
    let yaw = Quat::from_rotation_y(rng.range(0.0, TAU));
    let place = at(Vec3::ZERO) * tr(Vec3::ZERO, yaw) * lie;
    p.add(art::OLIVE, kit::placed(can, place));
    p.add(art::LIGHT, kit::placed(band, place));
    p.add(art::DARK, kit::placed(fuze, place));
    p.add(art::RUBBER, kit::placed(holes, place));
    // The thrown lever and the pin's ring nearby.
    let spoon: Vec<Vec3> = (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            Vec3::new(t * 0.09, 0.004 + 0.012 * (t * PI).sin(), 0.0)
        })
        .collect();
    let sec = [
        Vec2::new(-0.0008, -0.006),
        Vec2::new(0.0008, -0.006),
        Vec2::new(0.0008, 0.006),
        Vec2::new(-0.0008, 0.006),
    ];
    let sec: Vec<Vec2> = sec.iter().map(|q| Vec2::new(q.y, q.x)).collect();
    let spoon = kit::sweep(&spoon, &sec, false, 40.0);
    let off = Vec3::new(rng.range(-0.4, 0.4), floor, rng.range(0.15, 0.4));
    p.add(
        art::GALV,
        kit::placed(spoon, tr(off, Quat::from_rotation_y(rng.range(0.0, TAU)))),
    );
    let ring_at = Vec3::new(rng.range(-0.3, 0.3), floor + 0.0016, rng.range(-0.4, -0.15));
    p.add(
        art::GALV,
        kit::placed(kit::ring(0.012, 0.0015, 16, 5), at(ring_at)),
    );
    p.add(
        art::GALV,
        kit::rod(
            ring_at + Vec3::new(0.012, 0.0, 0.0),
            ring_at + Vec3::new(0.05, 0.0, 0.01),
            0.0013,
            6,
        ),
    );
    p
}
