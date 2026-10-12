//! Range clutter that makes it look used: power cables from the fans to a junction box, red
//! range flags on poles, and wooden ammunition crates and steel ammo cans behind the gun.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::ground::{PAD_TOP, wnoise};
use super::kit::{self, at, tr};

fn catmull(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    (p1 * 2.0
        + (p2 - p0) * t
        + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2
        + (p1 * 3.0 - p0 - p2 * 3.0 + p3) * t3)
        * 0.5
}

/// A smooth path through `pts` (Catmull-Rom), about `step` apart.
fn smooth(pts: &[Vec3], step: f32) -> Vec<Vec3> {
    let mut path = Vec::new();
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let n = ((p2 - p1).length() / step).ceil().max(1.0) as u32;
        for k in 0..n {
            path.push(catmull(p0, p1, p2, p3, k as f32 / n as f32));
        }
    }
    path.push(pts[pts.len() - 1]);
    path
}

/// A rubber cable through `pts` (smoothed), radius `r`.
pub fn cable(parts: &mut Parts, pts: &[Vec3], r: f32) {
    parts.add(art::RUBBER, kit::tube(&smooth(pts, 0.06), r, 8, false));
}

/// Power cables from each fan's motor (fan hub at (`fan_x`, `fan_y`, z) for z in `fan_z`,
/// motors toward −x) down to the pad and along its edge to a junction box at `box_at`.
pub fn fan_cables(parts: &mut Parts, fan_x: f32, fan_y: f32, fan_z: &[f32], box_at: Vec2) {
    let r = 0.011;
    let lie = PAD_TOP + r;
    for (i, &z) in fan_z.iter().enumerate() {
        let lane_x = box_at.x - 0.05 + i as f32 * 0.07;
        let seed = 900 + i as u32 * 7;
        let wig = |a: f32| wnoise(a, i as f32 * 3.1, 1.3, seed) * 0.06;
        // From the terminal box on the motor, hanging down behind the fan.
        let mut pts = vec![
            Vec3::new(fan_x - 0.5, fan_y + 0.2, z),
            Vec3::new(fan_x - 0.62, fan_y + 0.1, z + 0.02),
            Vec3::new(fan_x - 0.82, fan_y - 0.9, z + 0.12),
            Vec3::new(fan_x - 1.0, lie + 0.05, z + 0.22),
            Vec3::new(fan_x - 1.15, lie, z + 0.3),
        ];
        // Along the pad's edge toward the box, lying loose.
        let mut s = z - 0.4;
        while s > box_at.y + 0.9 {
            pts.push(Vec3::new(lane_x + wig(s), lie, s));
            s -= 1.6;
        }
        pts.push(Vec3::new(lane_x, lie, box_at.y + 0.5));
        pts.push(Vec3::new(
            box_at.x + (i as f32 - 1.0) * 0.08,
            lie + 0.02,
            box_at.y + 0.18,
        ));
        pts.push(Vec3::new(
            box_at.x + (i as f32 - 1.0) * 0.08,
            PAD_TOP + 0.45,
            box_at.y + 0.1,
        ));
        pts.push(Vec3::new(
            box_at.x + (i as f32 - 1.0) * 0.08,
            PAD_TOP + 0.55,
            box_at.y + 0.1,
        ));
        cable(parts, &pts, r);
    }
    junction_box(parts, box_at);
    // The feed: a thicker cable from the box off the pad into the ground.
    cable(
        parts,
        &[
            Vec3::new(box_at.x, PAD_TOP + 0.55, box_at.y - 0.1),
            Vec3::new(box_at.x, PAD_TOP + 0.3, box_at.y - 0.12),
            Vec3::new(box_at.x - 0.1, PAD_TOP + 0.02, box_at.y - 0.3),
            Vec3::new(box_at.x - 0.6, PAD_TOP + 0.015, box_at.y - 0.45),
            Vec3::new(box_at.x - 1.4, 0.0, box_at.y - 0.6),
            Vec3::new(box_at.x - 2.2, -0.12, box_at.y - 0.7),
        ],
        0.018,
    );
}

/// A weatherproof junction box on two posts, its door facing the range (+x).
fn junction_box(parts: &mut Parts, c: Vec2) {
    let base = Vec3::new(c.x, PAD_TOP, c.y);
    for dz in [-0.22f32, 0.22] {
        parts.add(
            art::GALV,
            kit::placed(
                kit::rounded_box(Vec3::new(0.05, 1.1, 0.05), 0.006, 2, 0.3),
                at(base + Vec3::new(0.0, 0.55, dz)),
            ),
        );
        parts.add(
            art::GALV,
            kit::placed(
                kit::rounded_box(Vec3::new(0.16, 0.008, 0.16), 0.003, 1, 0.1),
                at(base + Vec3::new(0.0, 0.004, dz)),
            ),
        );
    }
    let body = Vec3::new(0.2, 0.5, 0.42);
    let bc = base + Vec3::new(0.06, 0.82, 0.0);
    parts.add(
        art::GALV,
        kit::placed(kit::rounded_box(body, 0.012, 2, 0.2), at(bc)),
    );
    // Door with a rain lip, hinges and a latch, and a yellow warning label.
    parts.add(
        art::GALV,
        kit::placed(
            kit::rounded_box(Vec3::new(0.012, 0.47, 0.39), 0.004, 1, 0.2),
            at(bc + Vec3::new(body.x * 0.5 + 0.006, 0.0, 0.0)),
        ),
    );
    parts.add(
        art::GALV,
        kit::placed(
            kit::rounded_box(Vec3::new(0.24, 0.012, 0.46), 0.004, 1, 0.2),
            at(bc + Vec3::new(0.0, body.y * 0.5 + 0.006, 0.0)),
        ),
    );
    for y in [-0.15f32, 0.15] {
        parts.add(
            art::DARK,
            kit::rod(
                bc + Vec3::new(body.x * 0.5 + 0.012, y - 0.03, -0.2),
                bc + Vec3::new(body.x * 0.5 + 0.012, y + 0.03, -0.2),
                0.008,
                8,
            ),
        );
    }
    parts.add(
        art::DARK,
        kit::placed(
            kit::rounded_box(Vec3::new(0.025, 0.06, 0.025), 0.006, 1, 0.1),
            at(bc + Vec3::new(body.x * 0.5 + 0.02, 0.0, 0.17)),
        ),
    );
    let label = [
        Vec2::new(-0.05, -0.043),
        Vec2::new(0.05, -0.043),
        Vec2::new(0.0, 0.043),
    ];
    let label = kit::extrude(&kit::fillet(&label, 0.006, 2), 0.0, 0.002, 30.0, 0.0);
    parts.add(
        art::CART,
        kit::placed(
            label,
            tr(
                bc + Vec3::new(body.x * 0.5 + 0.012, 0.12, 0.0),
                Quat::from_rotation_y(FRAC_PI_2),
            ),
        ),
    );
    // Cable glands underneath.
    for k in 0..4 {
        let z = -0.15 + k as f32 * 0.1;
        parts.add(
            art::RUBBER,
            kit::rod(
                bc + Vec3::new(0.0, -body.y * 0.5, z),
                bc + Vec3::new(0.0, -body.y * 0.5 - 0.035, z),
                0.018,
                10,
            ),
        );
    }
}

/// A red range flag on a pole at `p` (on the pad), flying toward +x.
pub fn flag(parts: &mut Parts, p: Vec2, seed: u32) {
    let base = Vec3::new(p.x, PAD_TOP, p.y);
    let h = 5.2;
    // Concrete foot, galvanized pole, ball finial.
    parts.add(
        art::PRECAST,
        kit::placed(
            kit::rounded_box(Vec3::new(0.5, 0.32, 0.5), 0.03, 2, 0.3),
            at(base + Vec3::Y * 0.16),
        ),
    );
    parts.add(
        art::GALV,
        kit::rod(base + Vec3::Y * 0.3, base + Vec3::Y * h, 0.03, 16),
    );
    let ball = kit::lathe(
        &(0..=8)
            .map(|i| {
                let a = -FRAC_PI_2 + i as f32 / 8.0 * PI;
                Vec2::new(a.cos().max(0.0) * 0.05, a.sin() * 0.05 + 0.05)
            })
            .collect::<Vec<_>>(),
        16,
        60.0,
        false,
    );
    parts.add(art::GALV, kit::placed(ball, at(base + Vec3::Y * h)));
    // Halyard and the flag: a rippling sheet that sags toward its fly end.
    let top = base + Vec3::Y * (h - 0.12);
    let (fl, fh) = (1.5f32, 1.0f32);
    let sheet = kit::surface(28, 14, false, |u, v| {
        let x = u * fl;
        let wave = (u * TAU * 1.3 - 0.6 + v * 0.8).sin() * 0.11 * u.powf(0.8)
            + wnoise(x, v, 0.4, seed) * 0.03 * u;
        let sag = u * u * 0.14 * (0.4 + v);
        (
            top + Vec3::new(x + 0.035, -v * fh - sag, wave),
            Vec2::new(x, v * fh),
        )
    });
    parts.add(art::FLAG, sheet);
    parts.add(
        art::RUBBER,
        kit::rod(
            top + Vec3::new(0.035, 0.05, 0.0),
            base + Vec3::new(0.035, 1.2, 0.0),
            0.003,
            6,
        ),
    );
    parts.add(
        art::GALV,
        kit::placed(
            kit::rounded_box(Vec3::new(0.03, 0.12, 0.02), 0.005, 1, 0.1),
            at(base + Vec3::new(0.04, 1.2, 0.0)),
        ),
    );
}

/// One wooden ammunition crate along +x, bottom at y = 0: planked sides, end battens, a
/// lid with battens and rope handles.
pub fn crate_box(rng: &mut Rng) -> Parts {
    let mut p = Parts::default();
    let (l, w, h) = (0.82f32, 0.42f32, 0.36f32);
    let t = 0.02;
    let tint = Vec3::new(0.9, 0.96, 0.7) * rng.range(0.9, 1.12);
    let mut wood = Mesh::default();
    let mut plank = |size: Vec3, c: Vec3, rng: &mut Rng| {
        let m = kit::rounded_box(size, 0.004, 1, 0.3);
        let m = kit::offset_uv(m, Vec2::new(rng.range(0.0, 1.2), rng.range(0.0, 1.2)));
        let tone = rng.range(0.85, 1.12);
        wood.merge(&kit::tint(kit::placed(m, at(c)), Vec3::splat(tone)));
    };
    let planks = 3;
    let ph = (h - 0.03) / planks as f32;
    for s in [-1.0f32, 1.0] {
        for k in 0..planks {
            let y = 0.015 + ph * (k as f32 + 0.5);
            plank(
                Vec3::new(l, ph - 0.004, t),
                Vec3::new(0.0, y, s * (w - t) * 0.5),
                rng,
            );
        }
        // End panel and battens.
        plank(
            Vec3::new(t, h - 0.03, w - 2.0 * t),
            Vec3::new(s * (l - t) * 0.5, h * 0.5, 0.0),
            rng,
        );
        for y in [0.07f32, h - 0.07] {
            plank(
                Vec3::new(0.02, 0.06, w + 0.01),
                Vec3::new(s * (l * 0.5 + 0.01), y, 0.0),
                rng,
            );
        }
    }
    plank(
        Vec3::new(l - 0.01, 0.02, w - 0.01),
        Vec3::new(0.0, 0.01, 0.0),
        rng,
    );
    for k in 0..2 {
        let z = (k as f32 - 0.5) * (w * 0.5);
        plank(
            Vec3::new(l, 0.022, w * 0.5 - 0.004),
            Vec3::new(0.0, h - 0.011, z),
            rng,
        );
    }
    for x in [-0.28f32, 0.28] {
        plank(Vec3::new(0.06, 0.02, w), Vec3::new(x, h + 0.01, 0.0), rng);
    }
    p.add(art::TIMBER, kit::tint(wood, tint));
    // Rope handles at the ends.
    for s in [-1.0f32, 1.0] {
        let path: Vec<Vec3> = (0..=10)
            .map(|i| {
                let a = i as f32 / 10.0 * PI;
                Vec3::new(
                    s * (l * 0.5 + 0.025 + a.sin() * 0.03),
                    h * 0.6 - a.sin() * 0.03,
                    a.cos() * 0.09,
                )
            })
            .collect();
        p.add(
            art::SANDBAG,
            kit::tint(kit::tube(&path, 0.007, 6, false), Vec3::new(0.8, 0.75, 0.6)),
        );
    }
    // Steel straps over the lid.
    for x in [-0.36f32, 0.36] {
        let strap = kit::rounded_box(Vec3::new(0.022, h + 0.004, w + 0.006), 0.001, 1, 0.2);
        p.add(
            art::DARK,
            kit::placed(strap, at(Vec3::new(x, h * 0.5 + 0.002, 0.0))),
        );
    }
    p
}

/// A closed steel ammunition can (30 × 15 × 19 cm) along +x, bottom at y = 0.
pub fn ammo_can() -> Parts {
    let mut p = Parts::default();
    let size = Vec3::new(0.305, 0.17, 0.155);
    p.add(
        art::OLIVE,
        kit::placed(
            kit::rounded_box(size, 0.01, 2, 0.1),
            at(Vec3::Y * (size.y * 0.5)),
        ),
    );
    // Lid with a raised rim, the folding handle and the end latch.
    p.add(
        art::OLIVE,
        kit::placed(
            kit::rounded_box(
                Vec3::new(size.x + 0.008, 0.025, size.z + 0.008),
                0.008,
                2,
                0.1,
            ),
            at(Vec3::Y * (size.y + 0.008)),
        ),
    );
    for z in [-0.045f32, 0.045] {
        p.add(
            art::OLIVE,
            kit::placed(
                kit::rounded_box(Vec3::new(size.x - 0.04, 0.006, 0.012), 0.003, 1, 0.1),
                at(Vec3::new(0.0, size.y + 0.022, z)),
            ),
        );
    }
    let handle: Vec<Vec3> = (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            Vec3::new(
                -0.05 + 0.1 * t,
                size.y + 0.026 + 0.004 * (t * PI).sin(),
                0.0,
            )
        })
        .collect();
    p.add(art::GUNMETAL, kit::tube(&handle, 0.0045, 8, false));
    p.add(
        art::DARK,
        kit::placed(
            kit::rounded_box(Vec3::new(0.012, 0.09, 0.05), 0.004, 2, 0.1),
            at(Vec3::new(size.x * 0.5 + 0.006, size.y - 0.03, 0.0)),
        ),
    );
    p
}

/// Crates and cans stacked behind the gun pit, around `c` (on the pad).
pub fn ammo_dump(parts: &mut Parts, c: Vec2, seed: u64) {
    let mut rng = Rng::new(seed);
    let floor = Vec3::new(c.x, PAD_TOP, c.y);
    // Two crates side by side, one on top across them, one open-lidded can stack beside.
    let spots = [
        (Vec3::new(0.0, 0.0, 0.0), 0.05f32),
        (Vec3::new(0.0, 0.0, 0.47), -0.04),
        (Vec3::new(0.05, 0.38, 0.24), 0.12),
    ];
    for (o, yaw) in spots {
        let b = crate_box(&mut rng);
        parts.absorb(b, tr(floor + o, Quat::from_rotation_y(yaw)));
    }
    let can = ammo_can();
    let cans = [
        (Vec3::new(0.75, 0.0, 0.05), 0.1f32),
        (Vec3::new(0.75, 0.0, 0.24), 0.05),
        (Vec3::new(0.77, 0.205, 0.14), -0.15),
        (Vec3::new(0.62, 0.0, -0.45), 1.2),
    ];
    for (o, yaw) in cans {
        parts.absorb(
            can.clone(),
            tr(floor + o, Quat::from_rotation_y(yaw + FRAC_PI_2)),
        );
    }
}
