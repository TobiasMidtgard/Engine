//! Zip line cables: a galvanized wire rope from anchor to anchor, sagging between them and
//! pulled down into a shallow V under each rider's fist, with sparks streaming off a fast
//! rider's glove. Drawn for every [`Zipline`], whoever spawned it; nothing here moves anyone
//! (see [`crate::zipline`]).
//!
//! The cable is a chain of [`SEGMENTS`] stretched one-meter pieces re-laid every frame, so it
//! meets each rider's hand as drawn (between fixed steps).

use std::f32::consts::TAU;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_core::transform::TransformInterpolation;
use engine_render::{MeshRenderer, StandardMaterial};
use engine_world::Gravity;
use engine_world::physics::CharacterController;

use super::effects::{EffectSettings, Effects};
use super::rig::KNUCKLE_L;
use super::{Library, MeshBuilder};
use crate::demo::Puppet;
use crate::mover::Mover;
use crate::zipline::{self, Cable, Zipline};

/// Pieces of cable per line.
pub const SEGMENTS: usize = 64;
/// Cable radius, m (a 22 mm wire rope).
const CABLE_RADIUS: f32 = 0.011;
/// Pitch of the strands' lay along the cable, m.
const LAY: f32 = 0.16;

const CABLE_MESH: &str = "zip/cable";
const CABLE_MATERIAL: &str = "zip/cable";

/// Registers the cable piece and its material.
pub(crate) fn generate(lib: &mut Library) {
    // One meter along +Y. Six strands twisted around the core read as a helical ripple in the
    // shading (stretched with the piece, close enough for pieces about a meter long).
    let mut cable = MeshBuilder::tube(&[Vec3::ZERO, Vec3::Y], CABLE_RADIUS, 12);
    cable.shade(|p, n| {
        let a = n.x.atan2(n.z);
        let strand = (6.0 * a + p.y * TAU * 6.0 / LAY).cos();
        0.8 + 0.2 * strand * strand
    });
    lib.mesh(CABLE_MESH, cable);
    lib.material(
        CABLE_MATERIAL,
        StandardMaterial::hex(0x9a9d9f)
            .with_metallic(1.0)
            .with_roughness(0.38),
    );
}

/// The cable pieces of one line (on the line's entity).
#[derive(Component, Debug, Clone)]
pub struct ZipCable {
    segments: Vec<Entity>,
}

/// A piece of a line's cable (despawned with the line).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct ZipPart {
    line: Entity,
}

/// Where a body is drawn this frame (between its last two fixed steps).
fn visual(t: &Transform, interp: Option<&TransformInterpolation>, alpha: f32) -> Transform {
    interp
        .and_then(|i| i.previous)
        .map_or(*t, |p| p.lerp(t, alpha))
}

/// A rider's hand on a line: where along it (`s`) and how far off the unloaded cable the hand
/// pulls it there.
#[derive(Debug, Clone, Copy)]
struct Load {
    s: f32,
    pull: DVec3,
}

/// Points along the cable: the sag between the anchors, with each load pulling it into a
/// shallow V that passes exactly through the hand (a grid point is moved under every hand).
pub fn cable_points(cable: &Cable, loads: &[(f32, DVec3)]) -> [DVec3; SEGMENTS + 1] {
    let mut s: [f32; SEGMENTS + 1] = std::array::from_fn(|i| i as f32 / SEGMENTS as f32);
    let loads: Vec<Load> = loads
        .iter()
        .filter(|(at, _)| *at > 0.0 && *at < 1.0)
        .map(|&(at, hand)| {
            let i = ((at * SEGMENTS as f32).round() as usize).clamp(1, SEGMENTS - 1);
            s[i] = at;
            Load {
                s: at,
                pull: hand - cable.at(at),
            }
        })
        .collect();
    std::array::from_fn(|i| {
        let u = s[i];
        let mut p = cable.at(u);
        for l in &loads {
            // A tent from the anchors up to the hand.
            let k = if u <= l.s {
                u / l.s.max(1e-4)
            } else {
                (1.0 - u) / (1.0 - l.s).max(1e-4)
            };
            p += l.pull * k as f64;
        }
        p
    })
}

/// Gives every new [`Zipline`] its cable pieces, lays every cable along its anchors and
/// riders, and throws sparks off fast riders' gloves (`Update`, after the puppets are posed).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn draw_ziplines(
    mut commands: Commands,
    time: Res<Time>,
    fixed: Res<FixedTime>,
    gravity: Res<Gravity>,
    settings: Option<Res<EffectSettings>>,
    mut fx: Option<ResMut<Effects>>,
    lines: Query<(Entity, &Transform, &Zipline, Option<&ZipCable>)>,
    riders: Query<(
        &Mover,
        &Transform,
        Option<&TransformInterpolation>,
        &CharacterController,
        Option<&Puppet>,
    )>,
    mut parts: Query<&mut Transform, (With<ZipPart>, Without<Zipline>, Without<Mover>)>,
    mut loads: Local<Vec<(Entity, f32, DVec3, Vec3, f32)>>,
) {
    let alpha = fixed.alpha();
    // Every rider's hand as drawn.
    loads.clear();
    for (m, t, interp, cc, puppet) in &riders {
        let Some(z) = m.zip else {
            continue;
        };
        let Some(line) = z.line else {
            continue;
        };
        let body = visual(t, interp, alpha);
        let hand = match puppet {
            Some(p) => {
                let h = p.hand_l();
                let fist = h.pos + h.rot * (KNUCKLE_L * 0.7);
                body.translation + (body.rotation * fist).as_dvec3()
            }
            None => zipline::hand(body.translation, m.up, cc.height),
        };
        loads.push((line, z.s, hand, z.heading, z.speed));
    }

    for (line, t, z, cable_parts) in &lines {
        let Some(cable_parts) = cable_parts else {
            let segments = (0..SEGMENTS)
                .map(|_| {
                    commands
                        .spawn((
                            Name::new("Zip line cable"),
                            Transform::from_translation(t.translation),
                            MeshRenderer::new(
                                super::mesh(CABLE_MESH),
                                super::material(CABLE_MATERIAL),
                            ),
                            ZipPart { line },
                        ))
                        .id()
                })
                .collect();
            commands.entity(line).insert(ZipCable { segments });
            continue;
        };
        let cable = Cable::new(t, z, gravity.up_at(t.translation));
        let on: Vec<(f32, DVec3)> = loads
            .iter()
            .filter(|l| l.0 == line)
            .map(|l| (l.1, l.2))
            .collect();
        let points = cable_points(&cable, &on);
        for (i, &e) in cable_parts.segments.iter().enumerate() {
            let (a, b) = (points[i], points[i + 1]);
            let d = (b - a).as_vec3();
            let len = d.length().max(1e-4);
            if let Ok(mut pt) = parts.get_mut(e) {
                pt.translation = a;
                pt.rotation = Quat::from_rotation_arc(Vec3::Y, d / len);
                pt.scale = Vec3::new(1.0, len, 1.0);
            }
        }
    }

    // Sparks off the glove: more and longer the faster the ride.
    let (Some(fx), Some(settings)) = (fx.as_deref_mut(), settings.as_deref()) else {
        return;
    };
    let dt = time.delta_secs();
    for &(_, _, hand, heading, speed) in loads.iter() {
        fx.zip_sparks(hand, heading, speed, dt, settings);
    }
}

/// Despawns the cable pieces of lines that are gone (`Update`).
pub(crate) fn drop_orphan_cables(
    mut commands: Commands,
    parts: Query<(Entity, &ZipPart)>,
    lines: Query<(), With<Zipline>>,
) {
    for (e, part) in &parts {
        if !lines.contains(part.line) {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cable_runs_through_a_riders_hand() {
        let cable = Cable {
            a: DVec3::new(0.0, 10.0, 0.0),
            b: DVec3::new(30.0, 2.0, 0.0),
            up: Vec3::Y,
            sag: 0.4,
            end_clear: 1.5,
            exit_hop: 2.0,
        };
        let free = cable_points(&cable, &[]);
        assert_eq!(free[0], cable.a);
        assert_eq!(free[SEGMENTS], cable.b);
        let hand = cable.grip(0.37) + DVec3::new(0.0, -0.05, 0.04);
        let loaded = cable_points(&cable, &[(0.37, hand)]);
        assert!(loaded.iter().any(|p| (*p - hand).length() < 1e-6));
        // The anchors stay put, and the cable everywhere hangs at or below the free one.
        assert_eq!(loaded[0], cable.a);
        assert_eq!(loaded[SEGMENTS], cable.b);
        for (l, f) in loaded.iter().zip(free.iter()) {
            assert!(l.y <= f.y + 1e-6);
        }
    }
}
