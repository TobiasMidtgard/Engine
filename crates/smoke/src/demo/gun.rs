//! The heavy machine gun at the firing line: a pintle-mounted, belt-fed gun with a perforated
//! cooling jacket, conical flash hider, riveted receiver, spade grips, gun shield, ammo can
//! with a belt of cartridges feeding it, and spent brass on the ground.
//!
//! Gun space: the muzzle is at the origin, the barrel points along +Z (downrange), +Y is up
//! and +X is the gunner's left.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::kit::{self, at, tr};

/// Rotation that turns a lathe's +Y axis into +Z.
fn along_z() -> Quat {
    Quat::from_rotation_x(FRAC_PI_2)
}

/// A lathe along +Z starting at z0.
fn zlathe(profile: &[Vec2], segs: u32, crease: f32, z0: f32, center: Vec2) -> Mesh {
    kit::placed(
        kit::lathe(profile, segs, crease, false),
        tr(Vec3::new(center.x, center.y, z0), along_z()),
    )
}

/// Radius of the perforated jacket: its circumference holds exactly 26 holes of the 10 mm
/// grid in the jacket texture, so the pattern wraps without a seam.
const JACKET_R: f32 = 0.26 / TAU;

/// One cartridge (case and bullet) along +Z, case head at the origin. `fired` leaves an
/// empty, open case.
pub fn cartridge(fired: bool) -> (Mesh, Mesh) {
    let case = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0098, 0.0),
        Vec2::new(0.0102, 0.0012),
        Vec2::new(0.0102, 0.003),
        Vec2::new(0.0088, 0.0042),
        Vec2::new(0.0088, 0.0062),
        Vec2::new(0.0103, 0.0072),
        Vec2::new(0.0098, 0.078),
        Vec2::new(0.0078, 0.088),
        Vec2::new(0.0068, 0.090),
        Vec2::new(0.0068, 0.099),
    ];
    let mut prof = case.to_vec();
    if fired {
        prof.push(Vec2::new(0.006, 0.099));
        prof.push(Vec2::new(0.006, 0.09));
        prof.push(Vec2::new(0.0, 0.088));
    } else {
        prof.push(Vec2::new(0.0, 0.099));
    }
    let case = zlathe(&prof, 14, 35.0, 0.0, Vec2::ZERO);
    let bullet = if fired {
        Mesh::default()
    } else {
        let mut p = vec![Vec2::new(0.0, 0.0), Vec2::new(0.0065, 0.0)];
        for i in 0..=8 {
            let t = i as f32 / 8.0;
            // Tangent ogive from the shank to the point.
            let r = 0.0065 * (1.0 - t * t).sqrt().max(0.0);
            p.push(Vec2::new(r.max(0.0004), 0.018 + t * 0.040));
        }
        p.push(Vec2::new(0.0, 0.0585));
        zlathe(&p, 14, 40.0, 0.097, Vec2::ZERO)
    };
    (case, bullet)
}

fn gun_parts() -> Parts {
    let mut g = Parts::default();
    let gm = art::GUNMETAL;

    // Flash hider: a cone with a hollow mouth.
    let hider = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0235, 0.0),
        Vec2::new(0.0262, 0.004),
        Vec2::new(0.0262, 0.012),
        Vec2::new(0.0245, 0.016),
        Vec2::new(0.0305, 0.092),
        Vec2::new(0.0305, 0.102),
        Vec2::new(0.027, 0.106),
        Vec2::new(0.0225, 0.106),
        Vec2::new(0.0115, 0.055),
        Vec2::new(0.0065, 0.05),
        Vec2::new(0.0, 0.05),
    ];
    g.add(gm, zlathe(&hider, 28, 30.0, -0.106, Vec2::ZERO));
    // Barrel inside the jacket, slightly tapered.
    let barrel = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0245, 0.0),
        Vec2::new(0.0225, 1.10),
        Vec2::new(0.0, 1.10),
    ];
    g.add(gm, zlathe(&barrel, 20, 30.0, -1.2, Vec2::ZERO));
    // Perforated cooling jacket and its collars.
    let jacket = [Vec2::new(JACKET_R, 0.0), Vec2::new(JACKET_R, 0.76)];
    g.add(
        art::JACKET,
        kit::placed(
            kit::lathe(&jacket, 32, 30.0, false),
            tr(Vec3::new(0.0, 0.0, -1.065), along_z()),
        ),
    );
    let collar = |r: f32, len: f32| {
        vec![
            Vec2::new(0.022, 0.0),
            Vec2::new(r - 0.004, 0.0),
            Vec2::new(r, 0.004),
            Vec2::new(r, len - 0.006),
            Vec2::new(r - 0.006, len),
            Vec2::new(0.022, len),
        ]
    };
    g.add(
        gm,
        zlathe(&collar(0.0475, 0.035), 32, 30.0, -0.315, Vec2::ZERO),
    );
    g.add(
        gm,
        zlathe(&collar(0.0505, 0.05), 32, 30.0, -1.09, Vec2::ZERO),
    );
    // Barrel support at the front of the receiver, with the front sight.
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.118, 0.135, 0.17), 0.012, 3, 0.1),
            at(Vec3::new(0.0, -0.012, -1.165)),
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.006, 0.045, 0.02), 0.002, 1, 0.1),
            at(Vec3::new(0.0, 0.075, -1.12)),
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.04, 0.012, 0.035), 0.004, 2, 0.1),
            at(Vec3::new(0.0, 0.058, -1.12)),
        ),
    );

    // Receiver: box with raised, riveted side plates.
    let rc = Vec3::new(0.0, -0.025, -1.6);
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.124, 0.17, 0.72), 0.01, 3, 0.1),
            at(rc),
        ),
    );
    for side in [-1.0f32, 1.0] {
        g.add(
            gm,
            kit::placed(
                kit::rounded_box(Vec3::new(0.008, 0.13, 0.56), 0.003, 2, 0.1),
                at(rc + Vec3::new(side * 0.063, -0.005, 0.02)),
            ),
        );
        for row in [0.045f32, -0.07] {
            for k in 0..12 {
                let z = rc.z - 0.25 + k as f32 * 0.045;
                g.add(
                    gm,
                    kit::placed(
                        kit::rivet(0.0055),
                        tr(
                            Vec3::new(side * 0.067, rc.y + row, z),
                            Quat::from_rotation_z(-side * FRAC_PI_2),
                        ),
                    ),
                );
            }
        }
    }
    // Trunnion block under the front.
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.15, 0.06, 0.1), 0.008, 2, 0.1),
            at(Vec3::new(0.0, -0.125, -1.32)),
        ),
    );
    // Top cover with hinge and latch.
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.13, 0.032, 0.42), 0.008, 2, 0.1),
            at(Vec3::new(0.0, 0.074, -1.46)),
        ),
    );
    g.add(
        gm,
        kit::rod(
            Vec3::new(-0.07, 0.072, -1.255),
            Vec3::new(0.07, 0.072, -1.255),
            0.011,
            12,
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.05, 0.02, 0.03), 0.005, 2, 0.1),
            at(Vec3::new(0.0, 0.093, -1.66)),
        ),
    );
    // Feedway on the left, where the belt goes in.
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.05, 0.07, 0.15), 0.006, 2, 0.1),
            at(Vec3::new(0.083, 0.03, -1.42)),
        ),
    );
    // Rear sight: base, leaf and aperture.
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.045, 0.02, 0.06), 0.004, 2, 0.1),
            at(Vec3::new(0.0, 0.07, -1.8)),
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.034, 0.07, 0.005), 0.002, 1, 0.1),
            tr(Vec3::new(0.0, 0.11, -1.795), Quat::from_rotation_x(-0.15)),
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::ring(0.007, 0.0022, 16, 6),
            tr(
                Vec3::new(0.0, 0.135, -1.792),
                Quat::from_rotation_x(FRAC_PI_2 - 0.15),
            ),
        ),
    );
    // Charging handle on the right.
    g.add(
        gm,
        kit::rod(
            Vec3::new(-0.06, -0.02, -1.52),
            Vec3::new(-0.135, -0.02, -1.52),
            0.0075,
            10,
        ),
    );
    let knob = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.016, 0.0),
        Vec2::new(0.019, 0.006),
        Vec2::new(0.019, 0.028),
        Vec2::new(0.016, 0.034),
        Vec2::new(0.0, 0.034),
    ];
    g.add(
        art::RUBBER,
        kit::placed(
            kit::lathe(&knob, 16, 35.0, false),
            tr(
                Vec3::new(-0.135, -0.02, -1.52),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        ),
    );
    // Backplate, buffer housing, spade grips and butterfly trigger.
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.15, 0.2, 0.035), 0.01, 3, 0.1),
            at(Vec3::new(0.0, -0.03, -1.975)),
        ),
    );
    // Buffer housing, pointing back.
    let buffer = {
        let p = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.032, 0.0),
            Vec2::new(0.032, 0.035),
            Vec2::new(0.026, 0.045),
            Vec2::new(0.0, 0.045),
        ];
        kit::placed(
            kit::lathe(&p, 20, 35.0, false),
            tr(
                Vec3::new(0.0, -0.03, -1.99),
                Quat::from_rotation_x(-FRAC_PI_2),
            ),
        )
    };
    g.add(gm, buffer);
    for side in [-1.0f32, 1.0] {
        let a = Vec3::new(side * 0.058, -0.035, -1.985);
        let b = Vec3::new(side * 0.082, -0.115, -2.165);
        g.add(gm, kit::rod(a, b, 0.012, 12));
        // Rubber grip sleeve with finger ribs.
        let len = 0.12;
        let mut prof = vec![Vec2::new(0.0, 0.0), Vec2::new(0.019, 0.0)];
        for k in 0..=12 {
            let t = k as f32 / 12.0;
            let r = 0.0205 + 0.0018 * (t * PI * 8.0).sin().abs() - 0.002 * (2.0 * t - 1.0).powi(2);
            prof.push(Vec2::new(r, 0.006 + t * (len - 0.012)));
        }
        prof.push(Vec2::new(0.019, len));
        prof.push(Vec2::new(0.0, len));
        let dir = (b - a).normalize();
        g.add(
            art::RUBBER,
            kit::placed(
                kit::lathe(&prof, 16, 40.0, false),
                tr(a + dir * 0.05, kit::y_to(dir)),
            ),
        );
    }
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.07, 0.03, 0.012), 0.005, 2, 0.1),
            tr(Vec3::new(0.0, -0.05, -2.005), Quat::from_rotation_x(0.35)),
        ),
    );

    // Cradle and pintle (olive drab).
    let od = art::OLIVE;
    for side in [-1.0f32, 1.0] {
        g.add(
            od,
            kit::placed(
                kit::rounded_box(Vec3::new(0.012, 0.13, 0.24), 0.004, 2, 0.1),
                at(Vec3::new(side * 0.082, -0.17, -1.38)),
            ),
        );
    }
    g.add(
        od,
        kit::placed(
            kit::rounded_box(Vec3::new(0.176, 0.016, 0.24), 0.005, 2, 0.1),
            at(Vec3::new(0.0, -0.236, -1.38)),
        ),
    );
    g.add(
        gm,
        kit::rod(
            Vec3::new(-0.1, -0.13, -1.32),
            Vec3::new(0.1, -0.13, -1.32),
            0.011,
            12,
        ),
    );
    for side in [-1.0f32, 1.0] {
        g.add(
            gm,
            kit::placed(
                kit::hex_bolt(0.016),
                tr(
                    Vec3::new(side * 0.1, -0.13, -1.32),
                    Quat::from_rotation_z(-side * FRAC_PI_2),
                ),
            ),
        );
    }
    let ground = super::ground::PAD_TOP - super::MUZZLE[1] as f32;
    let post_z = -1.42;
    let pintle = [
        Vec2::new(0.0, ground),
        Vec2::new(0.32, ground),
        Vec2::new(0.322, ground + 0.004),
        Vec2::new(0.322, ground + 0.026),
        Vec2::new(0.3175, ground + 0.03),
        Vec2::new(0.065, ground + 0.03),
        Vec2::new(0.056, ground + 0.045),
        Vec2::new(0.056, -0.43),
        Vec2::new(0.062, -0.425),
        Vec2::new(0.062, -0.405),
        Vec2::new(0.048, -0.4),
        Vec2::new(0.048, -0.33),
        Vec2::new(0.03, -0.325),
        Vec2::new(0.028, -0.245),
        Vec2::new(0.0, -0.245),
    ];
    g.add(
        od,
        kit::placed(
            kit::lathe(&pintle, 32, 35.0, false),
            at(Vec3::new(0.0, 0.0, post_z)),
        ),
    );
    // Gussets and anchor bolts on the base plate.
    let gusset = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.2, 0.0),
        Vec2::new(0.2, 0.025),
        Vec2::new(0.0, 0.22),
    ];
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2 + PI / 4.0;
        let m = kit::extrude(&gusset, -0.006, 0.006, 30.0, 0.002);
        g.add(
            od,
            kit::placed(
                m,
                tr(
                    Vec3::new(0.0, ground + 0.03, post_z)
                        + Vec3::new(a.cos(), 0.0, -a.sin()) * 0.05,
                    Quat::from_rotation_y(a),
                ),
            ),
        );
    }
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        g.add(
            art::GALV,
            kit::placed(
                kit::hex_bolt(0.02),
                at(Vec3::new(
                    a.cos() * 0.26,
                    ground + 0.03,
                    post_z + a.sin() * 0.26,
                )),
            ),
        );
    }
    // Traverse lock lever on the socket.
    g.add(
        gm,
        kit::rod(
            Vec3::new(0.05, -0.37, post_z),
            Vec3::new(0.16, -0.33, post_z + 0.03),
            0.007,
            8,
        ),
    );
    g.add(
        art::RUBBER,
        kit::placed(
            kit::rounded_box(Vec3::new(0.05, 0.022, 0.022), 0.009, 2, 0.1),
            at(Vec3::new(0.175, -0.326, post_z + 0.034)),
        ),
    );

    // Gun shield: a center plate with a notch for the barrel and two angled wings.
    let notch_r = 0.06;
    let center = vec![
        Vec2::new(-0.33, -0.34),
        Vec2::new(0.33, -0.34),
        Vec2::new(0.33, 0.2),
        Vec2::new(0.27, 0.26),
        Vec2::new(notch_r, 0.26),
        Vec2::new(notch_r, -0.03),
        Vec2::new(-notch_r, -0.03),
        Vec2::new(-notch_r, 0.26),
        Vec2::new(-0.27, 0.26),
        Vec2::new(-0.33, 0.2),
    ];
    let radii = [0.01, 0.01, 0.02, 0.02, 0.01, 0.055, 0.055, 0.01, 0.02, 0.02];
    let plate = kit::fillet_each(&center, &radii, 5);
    let shield_z = -1.03;
    g.add(
        od,
        kit::placed(
            kit::extrude(&plate, -0.0045, 0.0045, 30.0, 0.0015),
            at(Vec3::new(0.0, 0.0, shield_z)),
        ),
    );
    // Rolled stiffening lip round the plate.
    let rim: Vec<Vec3> = kit::dedupe(&plate)
        .iter()
        .map(|q| Vec3::new(q.x, q.y, 0.0))
        .collect();
    g.add(
        od,
        kit::placed(
            kit::tube(&rim, 0.0055, 8, true),
            at(Vec3::new(0.0, 0.0, shield_z)),
        ),
    );
    let wing = kit::fillet(
        &[
            Vec2::new(0.0, -0.34),
            Vec2::new(0.25, -0.34),
            Vec2::new(0.25, 0.12),
            Vec2::new(0.0, 0.2),
        ],
        0.015,
        4,
    );
    for side in [-1.0f32, 1.0] {
        let w: Vec<Vec2> = wing.iter().map(|p| Vec2::new(p.x * side, p.y)).collect();
        let m = kit::extrude(&w, -0.0045, 0.0045, 30.0, 0.0015);
        let wing_t = tr(
            Vec3::new(side * 0.325, 0.0, shield_z - 0.004),
            Quat::from_rotation_y(side * 0.62),
        );
        g.add(od, kit::placed(m, wing_t));
        let rim: Vec<Vec3> = kit::dedupe(&w)
            .iter()
            .map(|q| Vec3::new(q.x, q.y, 0.0))
            .collect();
        g.add(od, kit::placed(kit::tube(&rim, 0.0055, 8, true), wing_t));
        // Hinge knuckles and bolts along the joint.
        for k in 0..4 {
            let y = -0.29 + k as f32 * 0.14;
            g.add(
                od,
                kit::placed(
                    kit::lathe(
                        &[
                            Vec2::new(0.0, 0.0),
                            Vec2::new(0.011, 0.0),
                            Vec2::new(0.011, 0.05),
                            Vec2::new(0.0, 0.05),
                        ],
                        12,
                        40.0,
                        false,
                    ),
                    at(Vec3::new(side * 0.333, y, shield_z - 0.012)),
                ),
            );
        }
        for k in 0..3 {
            let y = -0.25 + k as f32 * 0.2;
            g.add(
                gm,
                kit::placed(
                    kit::hex_bolt(0.012),
                    tr(
                        Vec3::new(side * 0.25, y, shield_z + 0.0045),
                        Quat::from_rotation_x(FRAC_PI_2),
                    ),
                ),
            );
        }
        // Brace from the shield to the cradle.
        g.add(
            od,
            kit::rod(
                Vec3::new(side * 0.2, -0.3, shield_z - 0.01),
                Vec3::new(side * 0.085, -0.2, -1.3),
                0.009,
                8,
            ),
        );
    }

    // Ammo can on its tray on the left, lid open, the belt feeding up into the gun.
    let can_c = Vec3::new(0.215, -0.21, -1.42);
    let can = Vec3::new(0.155, 0.19, 0.305);
    g.add(
        od,
        kit::placed(kit::rounded_box(can, 0.012, 3, 0.1), at(can_c)),
    );
    // Hollow look: a dark well inside the open top.
    g.add(
        art::RUBBER,
        kit::placed(
            kit::rounded_box(
                Vec3::new(can.x - 0.012, 0.004, can.z - 0.012),
                0.002,
                1,
                0.1,
            ),
            at(can_c + Vec3::new(0.0, can.y * 0.5 + 0.0005, 0.0)),
        ),
    );
    for side in [-1.0f32, 1.0] {
        for y in [-0.045f32, 0.035] {
            g.add(
                od,
                kit::placed(
                    kit::rounded_box(Vec3::new(0.006, 0.014, can.z - 0.06), 0.003, 2, 0.1),
                    at(can_c + Vec3::new(side * can.x * 0.5, y, 0.0)),
                ),
            );
        }
    }
    // Open lid hinged on the outer edge, with its handle and latch.
    let hinge = can_c + Vec3::new(can.x * 0.5, can.y * 0.5, 0.0);
    let lid_rot = Quat::from_rotation_z(-2.1);
    let lid = kit::rounded_box(
        Vec3::new(can.x + 0.006, 0.026, can.z + 0.006),
        0.008,
        2,
        0.1,
    );
    g.add(
        od,
        kit::placed(
            lid,
            tr(
                hinge + lid_rot * Vec3::new(-can.x * 0.5, 0.013, 0.0),
                lid_rot,
            ),
        ),
    );
    let handle: Vec<Vec3> = (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            Vec3::new(-0.045 + 0.09 * t, 0.03 + 0.012 * (t * PI).sin(), 0.0)
        })
        .collect();
    let handle = kit::tube(&handle, 0.005, 8, false);
    g.add(
        gm,
        kit::placed(
            handle,
            tr(
                hinge + lid_rot * Vec3::new(-can.x * 0.5, 0.0, 0.0),
                lid_rot * Quat::from_rotation_y(FRAC_PI_2),
            ),
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.05, 0.08, 0.012), 0.004, 2, 0.1),
            at(can_c + Vec3::new(0.0, 0.03, -can.z * 0.5 - 0.006)),
        ),
    );
    // Tray and its arm.
    g.add(
        od,
        kit::placed(
            kit::rounded_box(Vec3::new(0.175, 0.01, 0.33), 0.003, 1, 0.1),
            at(can_c + Vec3::new(0.0, -can.y * 0.5 - 0.006, 0.0)),
        ),
    );
    g.add(
        od,
        kit::placed(
            kit::rounded_box(Vec3::new(0.008, 0.06, 0.33), 0.003, 1, 0.1),
            at(can_c + Vec3::new(can.x * 0.5 + 0.008, -can.y * 0.5 + 0.02, 0.0)),
        ),
    );
    g.add(
        od,
        kit::rod(
            can_c + Vec3::new(-can.x * 0.5, -can.y * 0.5 - 0.01, 0.0),
            Vec3::new(0.08, -0.26, -1.38),
            0.012,
            10,
        ),
    );

    // The belt: rounds side by side along an arc from the can up into the feedway.
    let (case, bullet) = cartridge(false);
    let belt_path = |t: f32| -> Vec3 {
        // From inside the can (t = 0) over the lip to the feedway (t = 1).
        let p0 = Vec3::new(can_c.x - 0.01, can_c.y + 0.05, 0.0);
        let p1 = Vec3::new(can_c.x - 0.02, can_c.y + 0.2, 0.0);
        let p2 = Vec3::new(0.14, 0.06, 0.0);
        let p3 = Vec3::new(0.105, 0.032, 0.0);
        let u = 1.0 - t;
        p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t)
    };
    // Even spacing along the curve.
    let mut samples = vec![belt_path(0.0)];
    let mut lens = vec![0.0f32];
    for i in 1..=200 {
        let p = belt_path(i as f32 / 200.0);
        lens.push(lens[i - 1] + (p - samples[i - 1]).length());
        samples.push(p);
    }
    let total = lens[200];
    let pitch = 0.0254;
    let count = (total / pitch) as usize;
    for k in 0..count {
        let s = total - k as f32 * pitch - 0.012;
        let i = lens.partition_point(|l| *l < s).clamp(1, 200);
        let p = samples[i];
        let tangent = (samples[i] - samples[i - 1]).normalize_or(Vec3::X);
        // Rounds lie along Z; the belt's links turn with the path.
        let roll = tangent.y.atan2(tangent.x);
        let t = tr(Vec3::new(p.x, p.y, -1.49), Quat::from_rotation_z(roll));
        g.add(art::BRASS, kit::placed(case.clone(), t));
        g.add(art::COPPER, kit::placed(bullet.clone(), t));
        // Link: a clip around the case and a loop to the next round.
        let link = kit::placed(
            kit::ring(0.0113, 0.0016, 14, 5),
            tr(Vec3::new(0.0, 0.0, 0.03), along_z()),
        );
        g.add(gm, kit::placed(link.clone(), t));
        g.add(
            gm,
            kit::placed(kit::placed(link, at(Vec3::new(0.0, 0.0, 0.035))), t),
        );
    }

    // Carrying handle clamped on the jacket.
    let jr = JACKET_R + 0.004;
    g.add(
        gm,
        kit::placed(
            kit::ring(jr + 0.004, 0.006, 32, 8),
            tr(Vec3::new(0.0, 0.0, -0.95), along_z()),
        ),
    );
    g.add(
        gm,
        kit::placed(
            kit::ring(jr + 0.004, 0.006, 32, 8),
            tr(Vec3::new(0.0, 0.0, -0.71), along_z()),
        ),
    );
    let lean = Vec3::new(0.45, 1.0, 0.0).normalize();
    let handle = [
        Vec3::new(0.0, 0.0, -0.95) + lean * (jr + 0.008),
        Vec3::new(0.0, 0.0, -0.94) + lean * (jr + 0.07),
        Vec3::new(0.0, 0.0, -0.91) + lean * (jr + 0.095),
        Vec3::new(0.0, 0.0, -0.75) + lean * (jr + 0.095),
        Vec3::new(0.0, 0.0, -0.72) + lean * (jr + 0.07),
        Vec3::new(0.0, 0.0, -0.71) + lean * (jr + 0.008),
    ];
    g.add(gm, kit::tube(&handle, 0.008, 10, false));
    g.add(
        art::RUBBER,
        kit::placed(
            kit::lathe(
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.013, 0.0),
                    Vec2::new(0.0145, 0.01),
                    Vec2::new(0.0145, 0.1),
                    Vec2::new(0.013, 0.11),
                    Vec2::new(0.0, 0.11),
                ],
                14,
                40.0,
                false,
            ),
            tr(Vec3::new(0.0, 0.0, -0.885) + lean * (jr + 0.095), along_z()),
        ),
    );

    // Traversing and elevating mechanism: an arm back from the pintle head, a screw up to the
    // receiver, and its handwheel.
    let te_z = -1.86;
    g.add(
        od,
        kit::placed(
            kit::rounded_box(Vec3::new(0.035, 0.03, 0.47), 0.006, 2, 0.1),
            at(Vec3::new(0.0, -0.3, -1.63)),
        ),
    );
    let screw = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.016, 0.0),
        Vec2::new(0.016, 0.06),
        Vec2::new(0.011, 0.065),
        Vec2::new(0.011, 0.17),
        Vec2::new(0.0, 0.17),
    ];
    g.add(
        gm,
        kit::placed(
            kit::lathe(&screw, 16, 35.0, false),
            at(Vec3::new(0.0, -0.315, te_z)),
        ),
    );
    for k in 0..12 {
        g.add(
            gm,
            kit::placed(
                kit::ring(0.0118, 0.0016, 16, 4),
                at(Vec3::new(0.0, -0.245 + k as f32 * 0.0085, te_z)),
            ),
        );
    }
    g.add(
        gm,
        kit::placed(
            kit::rounded_box(Vec3::new(0.05, 0.03, 0.045), 0.006, 2, 0.1),
            at(Vec3::new(0.0, -0.125, te_z)),
        ),
    );
    let wheel_y = -0.27;
    g.add(
        gm,
        kit::placed(
            kit::ring(0.055, 0.0055, 32, 8),
            at(Vec3::new(0.0, wheel_y, te_z)),
        ),
    );
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2 + PI / 4.0;
        g.add(
            gm,
            kit::rod(
                Vec3::new(0.0, wheel_y, te_z),
                Vec3::new(a.cos() * 0.052, wheel_y, te_z + a.sin() * 0.052),
                0.004,
                6,
            ),
        );
    }
    g.add(
        art::RUBBER,
        kit::rod(
            Vec3::new(0.055, wheel_y, te_z),
            Vec3::new(0.055, wheel_y - 0.04, te_z),
            0.008,
            10,
        ),
    );

    // Grime: undersides darker.
    g.shade(|_, n| Vec3::splat(1.0 - n.y.min(0.0).abs() * 0.25));
    g
}

/// The gun (in gun space) and spent brass around its base.
pub fn gun() -> Parts {
    let mut g = gun_parts();
    // Spent cases and links, thrown down and to the right.
    let ground = super::ground::PAD_TOP - super::MUZZLE[1] as f32;
    let (case, _) = cartridge(true);
    let mut rng = Rng::new(0xB_A55);
    for _ in 0..90 {
        let r = rng.range(0.0, 1.0).powf(0.7) * 1.3;
        let a = rng.range(-PI, PI);
        let p = Vec2::new(-0.35 + a.cos() * r * 0.9, -1.55 + a.sin() * r * 0.8);
        if (p - Vec2::new(0.0, -1.42)).length() < 0.34 {
            continue;
        }
        let t = tr(
            Vec3::new(p.x, ground + 0.0101, p.y),
            Quat::from_rotation_y(rng.range(-PI, PI)),
        );
        // Center the case on its length.
        let m = kit::placed(case.clone(), t * kit::at(Vec3::new(0.0, 0.0, -0.05)));
        let tone = rng.range(0.75, 1.05);
        g.add(art::BRASS, kit::tint(m, Vec3::splat(tone)));
    }
    for _ in 0..25 {
        let p = Vec2::new(rng.range(-0.9, 0.2), rng.range(-2.3, -1.0));
        let t = tr(
            Vec3::new(p.x, ground + 0.003, p.y),
            Quat::from_rotation_y(rng.range(-PI, PI)) * along_z(),
        );
        g.add(
            art::GUNMETAL,
            kit::placed(kit::ring(0.0113, 0.0016, 12, 5), t),
        );
    }
    g
}
