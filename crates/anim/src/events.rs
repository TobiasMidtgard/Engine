//! What animation tells the rest of the engine: clip events, footfalls, wheel contacts and
//! the noise they make.
//!
//! * Clip events become [`AnimationEvent`]s. Events named `footstep…` with a foot bone also
//!   become a [`GroundContact`] (footprints in mud and snow, bent grass, footstep sounds) and
//!   a quiet [`NoiseEvent`] (AI hearing); `impact…` events make a loud one.
//! * Procedural gaits report every footfall the same way, with the creature's weight.
//! * Vehicle rigs report each wheel's contact every frame (tire tracks).

use engine_core::prelude::*;
use engine_world::animation::AnimationEvent;
use engine_world::{ContactKind, Ground, GroundContact, NoiseEvent};

use crate::animator::Animator;
use crate::ik::FootIk;
use crate::procedural::gait::LeggedGait;
use crate::procedural::vehicle::{VehicleRig, wheel_contact};

/// Weight and footprint used for clip footsteps of characters without a gait.
const BIPED_KG: f32 = 75.0;
const BIPED_FOOT: [f32; 2] = [0.11, 0.28];

#[allow(clippy::type_complexity)]
pub(crate) fn emit_events(
    ground: Res<Ground>,
    mut anim_events: MessageWriter<AnimationEvent>,
    mut contacts: MessageWriter<GroundContact>,
    mut noises: MessageWriter<NoiseEvent>,
    mut q: Query<(
        Entity,
        &Animator,
        &GlobalTransform,
        Option<&FootIk>,
        Option<&mut LeggedGait>,
        Option<&mut VehicleRig>,
    )>,
) {
    for (entity, animator, gt, foot_ik, gait, vehicle) in &mut q {
        let yaw = GroundContact::yaw_of(gt.rotation() * Vec3::NEG_Z);
        for e in &animator.events {
            anim_events.write(AnimationEvent {
                entity,
                name: e.name.clone(),
                bone: e.bone.clone(),
            });
            let bone_pos = e
                .bone
                .as_deref()
                .and_then(|b| animator.skeleton.index_of(b))
                .map(|b| animator.bone_world(gt, b).0);
            if e.name.starts_with("footstep")
                && let Some(p) = bone_pos
            {
                let foot_height = foot_ik.map_or(0.08, |f| f.foot_height) as f64;
                let sample = ground.sample(p);
                let y = sample.map_or(p.y - foot_height, |s| {
                    s.height.max(p.y - foot_height - 0.05)
                });
                let position = DVec3::new(p.x, y, p.z);
                contacts.write(GroundContact {
                    position,
                    normal: sample.map_or(Vec3::Y, |s| s.normal),
                    size: BIPED_FOOT,
                    yaw,
                    load_kg: BIPED_KG,
                    speed: 0.0,
                    kind: ContactKind::Foot,
                    surface: sample.map(|s| s.surface),
                    entity: Some(entity),
                });
                noises.write(NoiseEvent {
                    position,
                    radius: 8.0,
                    source: Some(entity),
                });
            } else if e.name.starts_with("impact") {
                noises.write(NoiseEvent {
                    position: bone_pos.unwrap_or(gt.translation()),
                    radius: 30.0,
                    source: Some(entity),
                });
            }
        }
        if let Some(mut gait) = gait {
            let kinds: Vec<(ContactKind, [f32; 2])> = gait
                .legs
                .iter()
                .map(|l| (l.kind.contact(), l.foot_size))
                .collect();
            for f in gait.take_footfalls() {
                let (kind, size) = kinds.get(f.leg).copied().unwrap_or_default();
                contacts.write(GroundContact {
                    position: f.position,
                    normal: f.normal,
                    size,
                    yaw,
                    load_kg: f.load_kg,
                    speed: f.speed,
                    kind,
                    surface: Some(f.surface),
                    entity: Some(entity),
                });
                noises.write(NoiseEvent {
                    position: f.position,
                    radius: (4.0 + f.load_kg.sqrt() * 0.8).min(40.0),
                    source: Some(entity),
                });
            }
        }
        if let Some(mut vehicle) = vehicle {
            for c in vehicle.take_contacts() {
                contacts.write(wheel_contact(&c, entity));
            }
        }
    }
}
