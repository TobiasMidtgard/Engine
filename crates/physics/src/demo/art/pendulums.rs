//! Meshes for the pendulum stations: the pendulum wave's A-frame gantry and its bobs, and the
//! Newton's cradle on its plinth.
//!
//! As in [`super::structures`]: one mesh per material, in the local frame of the entity that
//! carries it, metric UVs, and vertex colors near white.

use std::f32::consts::{FRAC_PI_2, TAU};

use engine_core::math::{Quat, Vec3};
use engine_render::Mesh;

use super::kit::MeshBuilder;
use super::structures::{at, bar, fillet_path, grey, hash, hex_nut, plate, rot_at, washer};

/// How far below the crossbar's axis a wave bob's pivot (the eyes its wires hang from) is.
pub const WAVE_EYE_DROP: f32 = 0.075;

/// Half the spread of a wave bob's two wires where they meet the crossbar, along the bar.
pub const WAVE_WIRE_SPREAD: f32 = 0.09;

/// The pendulum wave's gantry.
pub struct WaveFrame {
    /// Safety-yellow painted steel: the A-frame legs, ties, apex plates and base plates.
    pub painted: Mesh,
    /// Bare steel: the crossbar, its saddles and nuts, and the eyes the bobs hang from.
    pub steel: Mesh,
    /// Cast concrete: the footings.
    pub concrete: Mesh,
}

/// The pendulum wave's gantry: a steel crossbar along X at `height` (its axis), carried by
/// an A-frame at each end (`x = ±half_span`) whose legs stand `splay` either side of the bar
/// along Z on concrete footings 0.2 m high. Under the bar hang a pair of eyes for each of
/// `bobs` (x = `first_x + pitch·k`, `±WAVE_WIRE_SPREAD` along the bar), their centers
/// [`WAVE_EYE_DROP`] below the bar's axis.
///
/// Frame: the ground under the middle of the bar, Y up.
pub fn wave_frame(
    height: f32,
    half_span: f32,
    splay: f32,
    bobs: usize,
    first_x: f32,
    pitch: f32,
) -> WaveFrame {
    let mut p = MeshBuilder::new();
    let mut s = MeshBuilder::new();
    let mut c = MeshBuilder::new();
    let foot_h = 0.2;
    let leg_r = 0.055;
    let bar_r = 0.045;
    for (ei, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
        let x = sx * half_span;
        let apex = Vec3::new(x, height - bar_r - 0.02, 0.0);
        for (li, sz) in [-1.0f32, 1.0].into_iter().enumerate() {
            let k = (ei * 2 + li) as u32;
            let foot = Vec3::new(x, foot_h + 0.02, sz * splay);
            c.colored(grey(0.92 + 0.07 * hash(k + 11)), |b| {
                plate(
                    b,
                    at(Vec3::new(x, foot_h * 0.5, sz * splay)),
                    Vec3::new(0.24, foot_h * 0.5, 0.24),
                    0.02,
                );
            });
            p.colored(grey(0.95 + 0.05 * hash(k + 3)), |b| {
                plate(
                    b,
                    at(Vec3::new(x, foot_h + 0.01, sz * splay)),
                    Vec3::new(0.16, 0.01, 0.16),
                    0.004,
                );
                bar(b, foot, apex + Vec3::new(0.0, -0.04, sz * 0.05), leg_r, 18);
            });
            // Anchor nuts on the base plate.
            for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let o = Vec3::new(x + dx * 0.11, foot_h + 0.02, sz * splay + dz * 0.11);
                s.colored(grey(0.9), |b| {
                    washer(b, at(o + Vec3::Y * 0.002), 0.008, 0.022, 0.004, 12);
                    hex_nut(b, at(o + Vec3::Y * 0.012), 0.024, 0.016);
                });
            }
        }
        p.colored(grey(0.97), |b| {
            // A tie between the legs, and the apex plate the bar's saddle sits on.
            let tie_y = 1.1;
            let tz = splay * (height - tie_y) / (height - foot_h);
            bar(
                b,
                Vec3::new(x, tie_y, -tz),
                Vec3::new(x, tie_y, tz),
                0.032,
                14,
            );
            plate(
                b,
                at(apex + Vec3::new(0.0, -0.06, 0.0)),
                Vec3::new(0.012, 0.14, 0.13),
                0.004,
            );
        });
        s.colored(grey(0.85), |b| {
            // The saddle: a U-bolt round the bar.
            let path: Vec<Vec3> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    Vec3::new(
                        x,
                        height + (bar_r + 0.008) * a.sin(),
                        (bar_r + 0.008) * a.cos(),
                    )
                })
                .collect();
            b.tube(&path, 0.007, 8, false, true);
            for sz in [-1.0f32, 1.0] {
                hex_nut(
                    b,
                    rot_at(
                        Quat::IDENTITY,
                        Vec3::new(x, height - 0.03, sz * (bar_r + 0.008)),
                    ),
                    0.02,
                    0.014,
                );
            }
        });
    }
    // The crossbar, a little past both frames, with end caps.
    let bar_half = half_span + 0.18;
    s.colored(grey(0.95), |b| {
        b.placed(
            rot_at(Quat::from_rotation_z(FRAC_PI_2), Vec3::Y * height),
            |b| {
                b.cylinder(bar_r, bar_half, 24, 0.008);
            },
        );
    });
    // The eyes under it.
    s.colored(grey(0.8), |b| {
        for k in 0..bobs {
            let x = first_x + pitch * k as f32;
            for sx in [-1.0f32, 1.0] {
                let ex = x + sx * WAVE_WIRE_SPREAD;
                let eye = Vec3::new(ex, height - WAVE_EYE_DROP, 0.0);
                bar(
                    b,
                    Vec3::new(ex, height - bar_r * 0.5, 0.0),
                    eye + Vec3::Y * 0.012,
                    0.004,
                    8,
                );
                let ring: Vec<Vec3> = (0..16)
                    .map(|i| {
                        let a = TAU * i as f32 / 16.0;
                        eye + Vec3::new(0.012 * a.cos(), 0.012 * a.sin(), 0.0)
                    })
                    .collect();
                b.tube(&ring, 0.0035, 6, true, false);
            }
        }
    });
    WaveFrame {
        painted: p.build(),
        steel: s.build(),
        concrete: c.build(),
    }
}

/// A hanging bob.
pub struct Bob {
    /// The ball.
    pub ball: Mesh,
    /// Its eye and wires.
    pub wire: Mesh,
}

/// A ball of `radius` hanging on two wires from points `±spread` along the hinge axis
/// `axis` (X or Z, unit), `length` above its center: a pendulum wave bob (`axis` X) or a
/// cradle ball (`axis` Z, the wires spread across the row).
///
/// Frame: the ball's center, Y up toward the pivot when it hangs still.
pub fn bob(radius: f32, length: f32, axis: Vec3, spread: f32) -> Bob {
    let mut ball = MeshBuilder::new();
    ball.sphere(radius, 10);
    let mut w = MeshBuilder::new();
    let top = Vec3::Y * radius;
    w.colored(grey(0.85), |b| {
        // A screw eye in the top of the ball.
        b.placed(at(top + Vec3::Y * 0.004), |b| {
            b.cylinder(radius * 0.12, 0.006, 12, 0.002)
        });
        let eye = top + Vec3::Y * (0.01 + radius * 0.08);
        let ring: Vec<Vec3> = (0..14)
            .map(|i| {
                let a = TAU * i as f32 / 14.0;
                eye + (axis * a.cos() + Vec3::Y * a.sin()) * radius * 0.08
            })
            .collect();
        b.tube(&ring, radius * 0.025, 6, true, false);
        for s in [-1.0f32, 1.0] {
            let end = Vec3::Y * length + axis * s * spread;
            bar(b, eye + Vec3::Y * radius * 0.08, end, 0.0035, 6);
        }
    });
    Bob {
        ball: ball.build(),
        wire: w.build(),
    }
}

/// The Newton's cradle on its plinth.
pub struct Cradle {
    /// Black lacquered base board, and the rubber feet under it.
    pub base: Mesh,
    /// Chrome: the two bow frames the balls hang from, and their foot flanges.
    pub chrome: Mesh,
}

/// A Newton's cradle's base and frames: a base board `2·half_x × 2·half_z` (the board 7 cm
/// thick) and two chrome bows along X at `z = ±bar_z`, whose top rails' axes run at `pivot_y`
/// (the balls' pivots), legs at `x = ±(half_x - 0.1)`.
///
/// Frame: the middle of the board's underside (the plinth top), Y up.
pub fn cradle(half_x: f32, half_z: f32, bar_z: f32, pivot_y: f32) -> Cradle {
    let mut base = MeshBuilder::new();
    let board = 0.07;
    base.colored(grey(1.0), |b| {
        plate(
            b,
            at(Vec3::Y * (board * 0.5 + 0.012)),
            Vec3::new(half_x, board * 0.5, half_z),
            0.018,
        );
    });
    base.colored(grey(0.35), |b| {
        for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            b.placed(
                at(Vec3::new(dx * (half_x - 0.08), 0.006, dz * (half_z - 0.08))),
                |b| b.cylinder(0.035, 0.006, 16, 0.003),
            );
        }
    });
    let mut chrome = MeshBuilder::new();
    let leg_x = half_x - 0.1;
    let top_y = board + 0.012;
    for sz in [-1.0f32, 1.0] {
        let z = sz * bar_z;
        let path = fillet_path(
            &[
                Vec3::new(-leg_x, top_y, z),
                Vec3::new(-leg_x, pivot_y, z),
                Vec3::new(leg_x, pivot_y, z),
                Vec3::new(leg_x, top_y, z),
            ],
            &[0.09, 0.09],
            8,
        );
        chrome.tube(&path, 0.016, 16, false, true);
        for sx in [-1.0f32, 1.0] {
            chrome.placed(at(Vec3::new(sx * leg_x, top_y + 0.008, z)), |b| {
                b.cylinder(0.04, 0.008, 20, 0.004)
            });
        }
    }
    Cradle {
        base: base.build(),
        chrome: chrome.build(),
    }
}
