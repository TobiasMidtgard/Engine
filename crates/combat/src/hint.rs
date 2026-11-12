//! Pictures of the rules: the zone hint (a thin outline round the part a look-aimed strike
//! would hit, on the target: `CombatSettings::hint`) and the wound view (every body's hurt
//! parts drawn yellow to red, severed ones black: `CombatSettings::show_wounds`). Both are
//! debug lines (engine_debug), drawn on top so the body doesn't hide them.

use engine_core::prelude::*;
use engine_debug::color::{rgba, with_alpha};
use engine_debug::{Color, DebugDraw};
use engine_world::animation::{SkeletonPose, StrikeTargets};
use engine_world::combat::BodyInjuries;

use crate::CombatSettings;
use crate::body::BodyCapsules;
use crate::look::LookAim;
use crate::resolve::{capsules_of, to_world};

/// The hint's colour: a warm white that reads on skin, cloth and steel.
pub const HINT: Color = rgba(1.0, 0.86, 0.55, 1.0);

fn draw_zone(
    debug: &mut DebugDraw,
    caps: &BodyCapsules,
    gt: &GlobalTransform,
    zone: engine_world::combat::BodyZone,
    grow: f32,
    color: Color,
) {
    for c in caps.of_zone(zone) {
        debug
            .capsule(to_world(gt, c.a), to_world(gt, c.b), c.radius * grow, color)
            .on_top();
    }
}

/// Mark the looked-at part on each look-aiming fighter's target: two thin rings round it,
/// facing the camera (a lock-on reticle on the part), and the part's outline faintly.
pub(crate) fn draw_hint(
    settings: Res<CombatSettings>,
    debug: Option<ResMut<DebugDraw>>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform)>,
    aimers: Query<(&LookAim, &StrikeTargets)>,
    bodies: Query<(&GlobalTransform, Option<&SkeletonPose>)>,
) {
    let Some(mut debug) = debug else { return };
    if !settings.hint {
        return;
    }
    let eye = crate::look::screen_camera(cameras.iter()).map(|c| c.translation());
    for (look, strike) in &aimers {
        let (Some(zone), Some(target)) = (look.zone, strike.target) else {
            continue;
        };
        if !look.enabled {
            continue;
        }
        let Ok((gt, pose)) = bodies.get(target) else {
            continue;
        };
        let caps = capsules_of(pose);
        let alpha = if look.on_body { 1.0 } else { 0.45 };
        let Some(mid) = caps.center(zone) else {
            continue;
        };
        let size = caps
            .of_zone(zone)
            .map(|c| c.a.distance(c.b) * 0.5 + c.radius)
            .fold(0.0f32, f32::max)
            .max(0.06);
        let at = to_world(gt, mid);
        let normal = eye.map_or(Vec3::Z, |e| (e - at).as_vec3().normalize_or(Vec3::Z));
        debug
            .circle(at, normal, size * 1.25, with_alpha(HINT, alpha))
            .on_top();
        debug
            .circle(
                at,
                normal,
                size * 1.25 + 0.012,
                with_alpha(HINT, alpha * 0.6),
            )
            .on_top();
        draw_zone(
            &mut debug,
            &caps,
            gt,
            zone,
            1.12,
            with_alpha(HINT, alpha * 0.35),
        );
    }
}

/// Where each judged blow landed, for 0.6 s: a small star, red for a clean hit, blue for one
/// caught on an arm or a thigh, grey for a glancing edge, black where it cut through.
pub(crate) fn draw_blows(
    settings: Res<CombatSettings>,
    debug: Option<ResMut<DebugDraw>>,
    log: Res<crate::CombatLog>,
    mut seen: Local<u64>,
) {
    let Some(mut debug) = debug else { return };
    let fresh = (log.total - *seen).min(log.records.len() as u64) as usize;
    *seen = log.total;
    if !settings.show_wounds || fresh == 0 {
        return;
    }
    for r in log.records.iter().rev().take(fresh) {
        if r.blocked {
            continue;
        }
        let v = &r.verdict;
        let color = if v.severed.is_some() {
            rgba(0.02, 0.02, 0.02, 1.0)
        } else if v.covered {
            rgba(0.25, 0.55, 1.0, 1.0)
        } else if v.glanced {
            rgba(0.7, 0.7, 0.7, 1.0)
        } else {
            rgba(1.0, 0.1, 0.05, 1.0)
        };
        debug
            .cross(r.world_point, 0.09, color)
            .duration(0.6)
            .on_top();
        debug
            .sphere(r.world_point, 0.035, color)
            .duration(0.6)
            .on_top();
    }
}

/// The wound colour: yellow for a scratch to deep red for a ruined part; black when severed.
pub fn wound_color(wound: f32, severed: bool) -> Color {
    if severed {
        return rgba(0.02, 0.02, 0.02, 1.0);
    }
    let w = wound.clamp(0.0, 1.0);
    rgba(1.0 - 0.35 * w, 0.85 * (1.0 - w), 0.1 * (1.0 - w), 1.0)
}

/// Draw every body's wounds on its parts.
pub(crate) fn draw_wounds(
    settings: Res<CombatSettings>,
    debug: Option<ResMut<DebugDraw>>,
    bodies: Query<(&GlobalTransform, Option<&SkeletonPose>, &BodyInjuries)>,
) {
    let Some(mut debug) = debug else { return };
    if !settings.show_wounds {
        return;
    }
    for (gt, pose, hurt) in &bodies {
        let caps = capsules_of(pose);
        for zone in engine_world::combat::BodyZone::ALL {
            let i = hurt.get(zone);
            if i.wound < 0.05 && !i.severed {
                continue;
            }
            // Bands every 2.5 cm round the part, so it reads as tinted at a distance.
            let color = wound_color(i.wound, i.severed);
            for c in caps.of_zone(zone) {
                let (a, b) = (to_world(gt, c.a), to_world(gt, c.b));
                let axis = (b - a).as_vec3();
                let n = ((axis.length() / 0.025).ceil() as usize).max(1);
                let normal = axis.normalize_or(Vec3::Y);
                for k in 0..=n {
                    let at = a + (b - a) * (k as f64 / n as f64);
                    debug.circle(at, normal, c.radius * 1.08, color).on_top();
                }
                debug.capsule(a, b, c.radius * 1.08, color).on_top();
            }
        }
    }
}
