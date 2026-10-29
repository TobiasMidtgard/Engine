//! The lake mirror: a calm lake or pool near the camera reflects through a planar reflection
//! (`engine_render::PlanarReflection`), as Unreal's planar reflections and HDRP's planar probe do
//! for flat water. Screen-space reflections lose whatever leaves the screen (the crag above the
//! top edge, a boat's underside, the far shore when the camera tilts down); the mirror's view
//! has it all.
//!
//! One mirror at a time, on the nearest body flat enough for one plane to stand for its surface
//! (a tarn's waves are centimeters; an ocean's swell is not, and keeps its screen-space
//! reflections), while the camera is above it. The water shader samples the picture along each
//! pixel's reflected ray (`mirror_reflection` in water.wgsl).

use engine_core::math::DVec2;
use engine_core::prelude::*;
use engine_render::PlanarReflection;

use crate::body::{WaterExtent, WaterSettings};
use crate::state::{BodyState, SharedWater};

/// Waves taller than this (meters, the sum of the body's wave amplitudes) are too much for one
/// plane: the body keeps screen-space reflections.
const MAX_WAVES_M: f32 = 0.25;
/// Bodies farther than this (meters from their edge) get no mirror.
const REACH_M: f64 = 400.0;

/// The mirror entity and the body it reflects for.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LakeMirror {
    pub entity: Option<Entity>,
    /// The water body's entity.
    pub body: Option<Entity>,
}

/// Whether one plane can stand for the body's surface, and how far the camera is from it
/// (meters from its edge), with the camera at `eye`.
pub(crate) fn mirror_range(b: &BodyState, eye: DVec3) -> Option<f64> {
    if !b.body.visible || matches!(b.body.extent, WaterExtent::Infinite) {
        return None;
    }
    let waves: f32 = b.waves.waves.iter().map(|w| w.amp).sum();
    if waves > MAX_WAVES_M || eye.y < b.center.y + 0.05 {
        return None;
    }
    let d = DVec2::new(eye.x - b.center.x, eye.z - b.center.z).length()
        - b.body.extent.bounding_radius() as f64;
    (d < REACH_M).then_some(d.max(0.0))
}

/// Put the mirror on the nearest body that can have one (Extract, before the renderer places
/// the mirror's view), or take it away.
pub(crate) fn lake_mirror_system(world: &mut World) {
    let on = world
        .get_resource::<WaterSettings>()
        .is_none_or(|s| s.mirror_reflections);
    let resolution = world
        .get_resource::<WaterSettings>()
        .map_or(0.5, |s| s.mirror_resolution);
    let eye = engine_render::frame::active_camera(world).map(|(_, _, gt)| gt.translation());
    let best = match (on, eye, world.get_resource::<SharedWater>()) {
        (true, Some(eye), Some(water)) => {
            let state = water.0.read();
            state
                .bodies
                .iter()
                .filter_map(|b| mirror_range(b, eye).map(|d| (d, b)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, b)| (b.entity, b.center, b.body.extent.bounding_radius()))
        }
        _ => None,
    };
    let mut mirror = world
        .get_resource::<LakeMirror>()
        .copied()
        .unwrap_or_default();
    match best {
        Some((body, center, radius)) => {
            let at = GlobalTransform::from_translation_rotation(center, Quat::IDENTITY);
            let reflection =
                PlanarReflection::new(Vec2::splat(radius * 2.0)).with_resolution(resolution);
            let entity = mirror
                .entity
                .filter(|e| world.get_entity(*e).is_ok())
                .unwrap_or_else(|| world.spawn(Name::new("Lake mirror")).id());
            world
                .entity_mut(entity)
                .insert((reflection, at.to_transform(), at));
            mirror = LakeMirror {
                entity: Some(entity),
                body: Some(body),
            };
        }
        None => {
            if let Some(e) = mirror.entity.take() {
                world.despawn(e);
            }
            mirror.body = None;
        }
    }
    world.insert_resource(mirror);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::WaterBody;
    use crate::waves::{Wave, WaveSet};

    fn body(extent: WaterExtent, amp: f32) -> BodyState {
        BodyState {
            entity: Entity::PLACEHOLDER,
            center: DVec3::new(0.0, 10.0, 0.0),
            yaw: 0.0,
            body: WaterBody {
                extent,
                ..WaterBody::default()
            },
            waves: WaveSet {
                waves: vec![Wave {
                    amp,
                    ..Wave::default()
                }],
            },
            mean_depth: 3.0,
            wind: Vec3::ZERO,
        }
    }

    #[test]
    fn calm_lakes_seen_from_above_get_a_mirror_and_oceans_do_not() {
        let lake = body(WaterExtent::Circle { radius: 30.0 }, 0.02);
        assert_eq!(mirror_range(&lake, DVec3::new(10.0, 12.0, 0.0)), Some(0.0));
        assert_eq!(mirror_range(&lake, DVec3::new(80.0, 12.0, 0.0)), Some(50.0));
        // From under the water, or too far: none.
        assert_eq!(mirror_range(&lake, DVec3::new(0.0, 9.0, 0.0)), None);
        assert_eq!(mirror_range(&lake, DVec3::new(1000.0, 12.0, 0.0)), None);
        // A stormy lake and the ocean keep screen-space reflections.
        assert_eq!(
            mirror_range(
                &body(WaterExtent::Circle { radius: 30.0 }, 0.6),
                DVec3::new(0.0, 12.0, 0.0)
            ),
            None
        );
        assert_eq!(
            mirror_range(
                &body(WaterExtent::Infinite, 0.02),
                DVec3::new(0.0, 12.0, 0.0)
            ),
            None
        );
    }
}
