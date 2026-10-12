//! Steel targets: shot-up silhouette plates on T-post stands with deflectors, and round gongs
//! hanging on chains from A-frames. Plates use the target atlas (one of four shot patterns per
//! plate).

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::firing;
use super::ground::PAD_TOP;
use super::kit::{self, at, tr};

/// Map a plate's (x, y) (meters from its center) into quadrant `q` of the target atlas.
fn atlas_uv(m: &mut Mesh, q: u32) {
    let (qx, qy) = ((q % 2) as f32 * 0.5, (q / 2 % 2) as f32 * 0.5);
    for v in &mut m.vertices {
        let (x, y) = (v.position[0], v.position[1]);
        v.uv = [
            qx + (x / 1.1 + 0.5).clamp(0.0, 1.0) * 0.5,
            qy + (0.5 - y / 1.1).clamp(0.0, 1.0) * 0.5,
        ];
    }
}

fn silhouette() -> Vec<Vec2> {
    let pts = [
        (-0.2, -0.5),
        (0.2, -0.5),
        (0.275, -0.42),
        (0.275, 0.1),
        (0.16, 0.235),
        (0.072, 0.245),
        (0.105, 0.29),
        (0.105, 0.45),
        (0.068, 0.5),
        (-0.068, 0.5),
        (-0.105, 0.45),
        (-0.105, 0.29),
        (-0.072, 0.245),
        (-0.16, 0.235),
        (-0.275, 0.1),
        (-0.275, -0.42),
    ];
    let pts: Vec<Vec2> = pts.iter().map(|&(x, y)| Vec2::new(x, y)).collect();
    kit::fillet(&pts, 0.014, 3)
}

/// A square steel tube from `a` to `b`.
fn bar(a: Vec3, b: Vec3, w: f32) -> Mesh {
    let d = b - a;
    kit::placed(
        kit::rounded_box(Vec3::new(w, d.length(), w), w * 0.12, 1, 0.3),
        tr(
            (a + b) * 0.5,
            Quat::from_rotation_arc(Vec3::Y, d.normalize()),
        ),
    )
}

/// A silhouette target with its center at `c` (range space).
fn silhouette_target(parts: &mut Parts, c: Vec3, q: u32, rng: &mut Rng) {
    let tilt = Quat::from_rotation_x(-0.22);
    let mut plate = kit::extrude(&silhouette(), -0.0064, 0.0064, 30.0, 0.002);
    atlas_uv(&mut plate, q);
    parts.add_at(art::TARGET, plate, tr(c, tilt));
    // Mounting bracket behind the plate and the post.
    let post_z = c.z + 0.09;
    let top = c.y - 0.12;
    parts.add_at(
        art::DARK,
        kit::rounded_box(Vec3::new(0.12, 0.26, 0.012), 0.004, 1, 0.1),
        tr(c + tilt * Vec3::new(0.0, -0.1, 0.012), tilt),
    );
    parts.add(
        art::DARK,
        bar(
            Vec3::new(c.x, top, post_z),
            c + tilt * Vec3::new(0.0, -0.1, 0.02),
            0.05,
        ),
    );
    parts.add(
        art::DARK,
        bar(
            Vec3::new(c.x, PAD_TOP + 0.04, post_z),
            Vec3::new(c.x, top + 0.03, post_z),
            0.065,
        ),
    );
    // H-shaped base.
    for s in [-1.0f32, 1.0] {
        parts.add(
            art::DARK,
            bar(
                Vec3::new(c.x + s * 0.32, PAD_TOP + 0.035, post_z - 0.55),
                Vec3::new(c.x + s * 0.32, PAD_TOP + 0.035, post_z + 0.55),
                0.07,
            ),
        );
    }
    parts.add(
        art::DARK,
        bar(
            Vec3::new(c.x - 0.36, PAD_TOP + 0.035, post_z),
            Vec3::new(c.x + 0.36, PAD_TOP + 0.035, post_z),
            0.07,
        ),
    );
    // Deflector: a steel plate leaning back in front of the base.
    let defl = kit::rounded_box(Vec3::new(0.8, 0.42, 0.012), 0.004, 1, 0.2);
    parts.add_at(
        art::TARGET,
        {
            let mut m = defl;
            atlas_uv(&mut m, (q + 1) % 4);
            m
        },
        tr(
            Vec3::new(c.x, PAD_TOP + 0.17, post_z - 0.62),
            Quat::from_rotation_x(0.75),
        ),
    );
    // Sandbags weighing the feet down.
    for s in [-1.0f32, 1.0] {
        firing::loose_bag(
            parts,
            Vec3::new(c.x + s * 0.32, PAD_TOP + 0.05, post_z + 0.35),
            FRAC_PI_2 + rng.range(-0.2, 0.2),
            rng.next_u64(),
        );
    }
}

/// A chain of `n` links hanging from `top` straight down.
fn chain(parts: &mut Parts, top: Vec3, n: u32) {
    for k in 0..n {
        let link = kit::ring(0.012, 0.0028, 12, 5);
        let oval = engine_core::math::Mat4::from_scale(Vec3::new(1.0, 1.0, 1.7));
        let rot = if k % 2 == 0 {
            Quat::from_rotation_x(FRAC_PI_2)
        } else {
            Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_y(FRAC_PI_2)
        };
        let m = kit::placed(
            link,
            at(top - Vec3::Y * (k as f32 * 0.03 + 0.02)) * kit::tr(Vec3::ZERO, rot) * oval,
        );
        parts.add(art::RUST, m);
    }
}

/// A round gong with its center at `c`, hanging from an A-frame.
fn gong(parts: &mut Parts, c: Vec3, q: u32) {
    let disc = [
        Vec2::new(0.0, -0.0064),
        Vec2::new(0.296, -0.0064),
        Vec2::new(0.3, -0.004),
        Vec2::new(0.3, 0.004),
        Vec2::new(0.296, 0.0064),
        Vec2::new(0.0, 0.0064),
    ];
    let mut plate = kit::placed(
        kit::lathe(&disc, 64, 35.0, false),
        tr(Vec3::ZERO, Quat::from_rotation_x(FRAC_PI_2)),
    );
    atlas_uv(&mut plate, q);
    parts.add_at(art::TARGET, plate, tr(c, Quat::from_rotation_x(-0.05)));
    let bar_y = c.y + 0.72;
    for s in [-1.0f32, 1.0] {
        let x = c.x + s * 0.8;
        let apex = Vec3::new(x, bar_y + 0.05, c.z + 0.02);
        for dz in [-0.48f32, 0.48] {
            parts.add(
                art::DARK,
                bar(Vec3::new(x, PAD_TOP + 0.03, c.z + dz), apex, 0.05),
            );
        }
        parts.add(
            art::DARK,
            bar(
                Vec3::new(x, PAD_TOP + 0.03, c.z - 0.52),
                Vec3::new(x, PAD_TOP + 0.03, c.z + 0.52),
                0.06,
            ),
        );
        firing::loose_bag(
            parts,
            Vec3::new(x, PAD_TOP + 0.06, c.z + 0.3),
            FRAC_PI_2,
            (q as u64) * 31 + (s > 0.0) as u64,
        );
        // Shackle on the plate and the chain up to the bar.
        let hole = c + Vec3::new(s * 0.12, 0.245, 0.0);
        parts.add_at(
            art::RUST,
            kit::ring(0.016, 0.004, 14, 6),
            tr(hole + Vec3::Y * 0.01, Quat::from_rotation_z(FRAC_PI_2)),
        );
        chain(parts, Vec3::new(c.x + s * 0.12, bar_y - 0.02, c.z), 15);
    }
    parts.add(
        art::RUST,
        kit::rod(
            Vec3::new(c.x - 0.86, bar_y, c.z + 0.02),
            Vec3::new(c.x + 0.86, bar_y, c.z + 0.02),
            0.03,
            16,
        ),
    );
}

/// The targets at `xs` (range space), centers at height `y`, at depth `z`.
pub fn targets(parts: &mut Parts, xs: &[f32], y: f32, z: f32) {
    let mut rng = Rng::new(0x7A_6E7);
    for (i, &x) in xs.iter().enumerate() {
        let c = Vec3::new(x, y, z);
        if i % 2 == 0 {
            silhouette_target(parts, c, i as u32 % 4, &mut rng);
        } else {
            gong(parts, c, i as u32 % 4);
        }
    }
}
