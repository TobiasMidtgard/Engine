//! Aim where you look (aimed-melee 3.2): the body part under the camera's centre (the
//! reticle) is what the strike goes for. Each frame, for every fighter with [`LookAim`] (or
//! the player's [`Interactor`]) that has a target in its
//! [`StrikeTargets`](engine_world::animation::StrikeTargets), the active camera's centre ray is
//! tested against the target's zone capsules ([`crate::body`]):
//!
//! 1. A part the ray goes through, or passes within `CombatSettings::look_snap_m` of, is
//!    picked, with the point on it (the target's model space).
//! 2. Looking well off the body picks the chest.
//! 3. A new pick waits until the old one has been held `look_hold_secs`, so the aim doesn't
//!    flicker between thigh and knee.
//! 4. The pick goes into `StrikeTargets.zone` and `.point`, which the animation module swings
//!    at and the rules judge.

use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::Interactor;
use engine_world::animation::{SkeletonPose, StrikeTargets};
use engine_world::combat::BodyZone;

use crate::CombatSettings;
use crate::resolve::{capsules_of, to_model};

/// Aim this fighter's strikes where the camera looks. The player's
/// [`Interactor`] aims this way without it; `enabled: false` turns it off for one.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookAim {
    pub enabled: bool,
    /// The part picked now, and the point on it (the target's model space).
    pub zone: Option<BodyZone>,
    pub point: Option<Vec3>,
    /// Whether the ray is on the body (false: off it, the chest picked).
    pub on_body: bool,
    /// World time the pick was made, s.
    pub since: f64,
}

impl Default for LookAim {
    fn default() -> Self {
        Self {
            enabled: true,
            zone: None,
            point: None,
            on_body: false,
            since: f64::NEG_INFINITY,
        }
    }
}

/// The camera that draws the screen: the active one with the highest order.
pub fn screen_camera<'a>(
    cameras: impl Iterator<Item = (&'a Camera, &'a GlobalTransform)>,
) -> Option<&'a GlobalTransform> {
    cameras
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t)
}

type Aimer<'a> = (
    &'a GlobalTransform,
    &'a mut StrikeTargets,
    Option<&'a mut LookAim>,
);

/// Pick the looked-at part for every look-aiming fighter (Update, after cameras move).
#[allow(clippy::type_complexity)]
pub(crate) fn look_aim(
    time: Res<Time>,
    settings: Res<CombatSettings>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut aimers: Query<Aimer, Or<(With<LookAim>, With<Interactor>)>>,
    targets: Query<(&GlobalTransform, Option<&SkeletonPose>), Without<LookAim>>,
    mut commands: Commands,
    players: Query<Entity, (With<Interactor>, With<StrikeTargets>, Without<LookAim>)>,
) {
    // The player's body aims by look from its first frame with a target.
    for e in &players {
        commands.entity(e).try_insert(LookAim::default());
    }
    let Some(cam) = screen_camera(cameras.iter()) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let eye = cam.translation();
    let dir = cam.rotation() * Vec3::NEG_Z;
    for (gt, mut strike, look) in &mut aimers {
        let Some(mut look) = look else { continue };
        if !look.enabled {
            continue;
        }
        let Some(target) = strike.target else {
            if look.zone.is_some() {
                *look = LookAim::default();
            }
            continue;
        };
        let Ok((tgt, pose)) = targets.get(target) else {
            continue;
        };
        if gt.translation().distance(tgt.translation()) > settings.look_range_m as f64 {
            continue;
        }
        let caps = capsules_of(pose);
        let rot = tgt.rotation().inverse();
        let o = to_model(tgt, eye);
        let d = (rot * dir).normalize_or(Vec3::NEG_Z);
        let (zone, point, on_body) = match caps.pick(o, d, settings.look_snap_m) {
            Some((z, p)) => (z, p, true),
            None => (
                BodyZone::Chest,
                caps.center(BodyZone::Chest).unwrap_or(Vec3::Y * 1.3),
                false,
            ),
        };
        let held = now - look.since < settings.look_hold_secs as f64;
        if look.zone != Some(zone) && !(held && look.zone.is_some()) {
            look.zone = Some(zone);
            look.since = now;
        }
        if look.zone == Some(zone) {
            look.point = Some(point);
            look.on_body = on_body;
        }
        if strike.zone != look.zone {
            strike.zone = look.zone;
        }
        if strike.point != look.point {
            strike.point = look.point;
        }
    }
}
