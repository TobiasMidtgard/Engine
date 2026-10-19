//! The engine test stand outside the court's north-east rim: a rocket motor clamped
//! horizontally in an iron cradle, its forward end bolted to a massive stone thrust block, on a
//! basalt pad. The motor fires along +X.

use engine_core::math::{Quat, Rng, Vec3};
use engine_render::Mesh;

use super::super::meshes::metal::{band, bar, rivet};
use super::super::meshes::{Outline, Profile, block, colored, moved, revolve, slab, tube};
use super::super::{Prop, Surface};
use super::dress;

/// Height of the motor's axis.
pub const AXIS_Y: f32 = 1.25;
/// Where the nozzle's exit plane is, along +X.
pub const EXIT_X: f32 = PLATE_X + BELL_END;
/// Radius of the nozzle's exit.
pub const EXIT_RADIUS: f32 = 0.335;

/// Front face of the thrust plate, where the motor's forward end sits.
const PLATE_X: f32 = -1.82;
/// Top of the pad.
const PAD: f32 = 0.18;
/// Motor case radius and length, then where its bell starts and ends (from the plate).
const CASE_R: f32 = 0.4;
const CASE_LEN: f32 = 2.3;
const THROAT: f32 = 2.72;
const BELL_END: f32 = 3.62;

/// Turn a mesh built along +Y (from the plate) onto the motor's axis.
fn on_axis(mesh: Mesh) -> Mesh {
    moved(
        mesh,
        Vec3::new(PLATE_X, AXIS_Y, 0.0),
        Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
    )
}

/// The stand: pad, thrust block, cradle, feed lines and the motor with its bell nozzle.
pub fn engine_stand() -> Prop {
    let mut stone = Mesh::default();
    let mut dark = Mesh::default();
    let mut iron = Mesh::default();
    let mut bronze = Mesh::default();
    let mut ember = Mesh::default();

    // A basalt pad under it all, and the thrust block at the back: a stepped granite mass the
    // motor pushes against, with an ember-rune band.
    let mut pad = slab(Outline::rect(2.9, 1.9), -0.05, PAD, 0.04);
    for v in &mut pad.vertices {
        v.position[0] -= 0.45;
    }
    dress(&mut pad, 61, 1.2);
    dark.merge(&pad);
    let back = -3.05;
    let mut base = moved(
        block(Vec3::new(1.35, 0.5, 3.0), 0.04),
        Vec3::new(back + 0.62, PAD + 0.25, 0.0),
        Quat::IDENTITY,
    );
    dress(&mut base, 62, 1.3);
    stone.merge(&base);
    let mut mass = moved(
        block(Vec3::new(1.1, 2.3, 2.6), 0.05),
        Vec3::new(back + 0.55, PAD + 0.5 + 1.15, 0.0),
        Quat::IDENTITY,
    );
    dress(&mut mass, 63, 1.4);
    stone.merge(&mass);
    let mut cap = moved(
        block(Vec3::new(1.3, 0.16, 2.8), 0.03),
        Vec3::new(back + 0.55, PAD + 2.88, 0.0),
        Quat::IDENTITY,
    );
    dress(&mut cap, 64, 1.0);
    dark.merge(&cap);
    let front = back + 1.1;
    for y in [PAD + 0.62, PAD + 2.62] {
        ember.merge(&moved(
            block(Vec3::new(0.012, 0.05, 2.3), 0.004),
            Vec3::new(front + 0.004, y, 0.0),
            Quat::IDENTITY,
        ));
    }

    // The thrust plate bolted to the block's face.
    let plate_t = PLATE_X - front;
    iron.merge(&moved(
        block(Vec3::new(plate_t, 1.3, 1.3), 0.015),
        Vec3::new(front + plate_t * 0.5, AXIS_Y, 0.0),
        Quat::IDENTITY,
    ));
    let face = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
    for k in 0..12 {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        let at = Vec3::new(PLATE_X, AXIS_Y + a.sin() * 0.56, a.cos() * 0.56);
        iron.merge(&moved(rivet(0.03), at, face));
    }

    // The motor: a forward flange, the case and its converging aft end; then the bell nozzle,
    // its outside heat-tinted and its inside sooty down to a dark throat.
    let case = Profile::new()
        .corner(0.0, 0.0)
        .corner(CASE_R + 0.07, 0.0)
        .corner(CASE_R + 0.07, 0.07)
        .corner(CASE_R, 0.07)
        .to(CASE_R, CASE_LEN)
        .to(CASE_R - 0.02, CASE_LEN + 0.1)
        .to(0.3, CASE_LEN + 0.24)
        .to(0.2, CASE_LEN + 0.36)
        .to(0.16, THROAT - 0.02)
        .corner(0.155, THROAT);
    iron.merge(&on_axis(revolve(&case, 48)));
    let bell_out = Profile::new()
        .corner(0.155, THROAT)
        .to(0.18, 2.82)
        .to(0.235, 3.0)
        .to(0.285, 3.22)
        .to(0.32, 3.45)
        .corner(EXIT_RADIUS, BELL_END)
        .corner(EXIT_RADIUS - 0.014, BELL_END);
    iron.merge(&on_axis(colored(
        revolve(&bell_out, 48),
        Vec3::new(0.62, 0.55, 0.62),
    )));
    let bell_in = Profile::new()
        .corner(EXIT_RADIUS - 0.014, BELL_END)
        .to(0.304, 3.45)
        .to(0.271, 3.22)
        .to(0.221, 3.0)
        .to(0.166, 2.82)
        .corner(0.141, THROAT + 0.01)
        .corner(0.0, THROAT + 0.01);
    dark.merge(&on_axis(colored(revolve(&bell_in, 48), Vec3::splat(0.25))));

    // Bronze joint rings with rivets around the case.
    let mut rng = Rng::new(7);
    for y in [0.55f32, 1.25, 1.95] {
        bronze.merge(&on_axis(moved(
            band(CASE_R, 0.09, 0.022, 48),
            Vec3::Y * y,
            Quat::IDENTITY,
        )));
        for k in 0..14 {
            let a = (k as f32 + rng.range(-0.05, 0.05)) / 14.0 * std::f32::consts::TAU;
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            let at = out * (CASE_R + 0.022) + Vec3::Y * y;
            bronze.merge(&on_axis(moved(
                rivet(0.012),
                at,
                Quat::from_rotation_arc(Vec3::Y, out),
            )));
        }
    }

    // The cradle: two iron saddles clamping the case, each on an A-frame of square bars.
    for y in [0.85f32, 1.95] {
        let x = PLATE_X + y + 0.18;
        iron.merge(&on_axis(moved(
            band(CASE_R, 0.14, 0.03, 48),
            Vec3::Y * (y + 0.18),
            Quat::IDENTITY,
        )));
        for side in [-1.0f32, 1.0] {
            iron.merge(&bar(
                &[
                    Vec3::new(x, PAD, side * 0.82),
                    Vec3::new(x, AXIS_Y - 0.21, side * 0.35),
                ],
                0.045,
                0.0,
            ));
            iron.merge(&moved(
                block(Vec3::new(0.24, 0.04, 0.24), 0.008),
                Vec3::new(x, PAD + 0.02, side * 0.82),
                Quat::IDENTITY,
            ));
        }
        iron.merge(&bar(
            &[
                Vec3::new(x, PAD + 0.32, -0.7),
                Vec3::new(x, PAD + 0.32, 0.7),
            ],
            0.035,
            0.0,
        ));
        iron.merge(&moved(
            block(Vec3::new(0.16, 0.42, 0.5), 0.01),
            Vec3::new(x, AXIS_Y - CASE_R - 0.16, 0.0),
            Quat::IDENTITY,
        ));
    }

    // Two feed lines from the top of the block down to the motor's forward end.
    for side in [-1.0f32, 1.0] {
        let path: Vec<Vec3> = [
            (front - 0.3, PAD + 2.96, 0.62),
            (front - 0.3, PAD + 3.1, 0.62),
            (front + 0.05, PAD + 3.12, 0.6),
            (front + 0.3, PAD + 2.9, 0.52),
            (PLATE_X + 0.35, AXIS_Y + 0.9, 0.38),
            (PLATE_X + 0.38, AXIS_Y + 0.55, 0.26),
            (
                PLATE_X + 0.38,
                AXIS_Y + (CASE_R * CASE_R - 0.04).sqrt() - 0.02,
                0.2,
            ),
        ]
        .iter()
        .map(|&(x, y, z)| Vec3::new(x, y, z * side))
        .collect();
        bronze.merge(&tube(&path, |_| 0.045, 12, true));
        for (i, at) in [(0usize, path[1]), (1, path[path.len() - 1])] {
            let dir = if i == 0 {
                Vec3::Y
            } else {
                (path[path.len() - 1] - path[path.len() - 2]).normalize()
            };
            bronze.merge(&moved(
                band(0.045, 0.05, 0.02, 16),
                at - dir * 0.03,
                Quat::from_rotation_arc(Vec3::Y, dir),
            ));
        }
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, stone);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::Iron, iron);
    prop.push(Surface::Bronze, bronze);
    prop.push(Surface::RuneEmber, ember);
    prop
}
