//! The water in the renderer's ray scene ([`RenderFeature::ray_scene`]): one flat stand-in per
//! body at its still surface, so that probe light and world reflections meet the water. A
//! disc for a round body, a square for a rectangular one, and for an endless sea a square
//! out past the far reach that moves only when the camera crosses a cell of a coarse grid.
//!
//! [`RenderFeature::ray_scene`]: engine_render::features::RenderFeature::ray_scene

use std::collections::HashSet;

use engine_core::math::{DAffine3, DQuat, DVec2, DVec3};
use engine_render::{
    RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RaySphere, RayView,
};

use crate::body::{WaterExtent, WaterOptics};
use crate::state::BodyState;

/// The endless sea's square snaps its centre to this grid, m, and reaches one cell past the
/// far reach, so that its place changes only when the camera crosses a cell: a still
/// instance that moves has its structure built again.
pub(crate) const SEA_CELL_M: f64 = 64.0;

/// The geometry every round body's disc shares.
const DISC: RayKey = RayKey(0, 0);
/// The geometry every square shares (a rectangular body, the endless sea).
const SQUARE: RayKey = RayKey(1, 0);

/// A body as the ray scene needs it, taken from the main view's prepare.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RayBody {
    /// `Entity::to_bits` of the body: its instance's name.
    pub id: u64,
    /// World centre at the still surface.
    pub center: DVec3,
    pub yaw: f32,
    pub extent: WaterExtent,
    pub albedo: [f32; 3],
    pub roughness: f32,
}

impl RayBody {
    pub(crate) fn of(b: &BodyState) -> Self {
        Self {
            id: b.entity.to_bits(),
            center: b.center,
            yaw: b.yaw,
            extent: b.body.extent,
            albedo: deep_albedo(&b.body.optics),
            roughness: b.body.optics.roughness.clamp(0.0, 1.0),
        }
    }
}

/// What deep water of these optics sends back of the light that falls into it, per band: the
/// irradiance reflectance of an optically deep body, R ≈ 0.33 b_b / (a + b_b) (Gordon, Brown
/// & Jacobs 1975; Morel & Prieur 1977). The surface's own reflection is not in it: a hit adds
/// Fresnel itself.
pub(crate) fn deep_albedo(o: &WaterOptics) -> [f32; 3] {
    std::array::from_fn(|i| {
        let bb = (o.scattering * o.scatter_color[i] * o.backscatter).max(0.0);
        let a = o.absorption[i].max(0.0);
        if a + bb <= 0.0 {
            0.0
        } else {
            (0.33 * bb / (a + bb)).clamp(0.0, 0.95)
        }
    })
}

/// One instance to place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub id: RayKey,
    pub geometry: RayKey,
    pub to_world: DAffine3,
    pub sphere: RaySphere,
    pub look: RayLook,
}

/// Where `b`'s stand-in stands as seen from `eye`, or `None` beyond `far_reach`.
pub(crate) fn placement(b: &RayBody, eye: DVec3, far_reach: f32) -> Option<Placement> {
    let far = far_reach.max(0.0) as f64;
    let rise = (b.center.y - eye.y).abs();
    let flat = |near: f64| near.hypot(rise) <= far;
    let c = DVec2::new(b.center.x, b.center.z);
    let e = DVec2::new(eye.x, eye.z);
    let (geometry, to_world, sphere) = match b.extent {
        WaterExtent::Circle { radius } => {
            let r = radius.max(0.0) as f64;
            if r <= 0.0 || !flat(((e - c).length() - r).max(0.0)) {
                return None;
            }
            let to_world = DAffine3::from_scale_rotation_translation(
                DVec3::new(r, 1.0, r),
                DQuat::IDENTITY,
                b.center,
            );
            (
                DISC,
                to_world,
                RaySphere {
                    center: b.center,
                    radius: r as f32,
                },
            )
        }
        WaterExtent::Rect { half_size } => {
            let h = DVec2::new(half_size[0].max(0.0) as f64, half_size[1].max(0.0) as f64);
            if h.x <= 0.0 || h.y <= 0.0 {
                return None;
            }
            // The eye in the body's frame (as `BodyState::local`).
            let (s, co) = (b.yaw as f64).sin_cos();
            let d = e - c;
            let local = DVec2::new(co * d.x - s * d.y, s * d.x + co * d.y);
            if !flat((local.abs() - h).max(DVec2::ZERO).length()) {
                return None;
            }
            let to_world = DAffine3::from_scale_rotation_translation(
                DVec3::new(h.x, 1.0, h.y),
                DQuat::from_rotation_y(b.yaw as f64),
                b.center,
            );
            (
                SQUARE,
                to_world,
                RaySphere {
                    center: b.center,
                    radius: h.length() as f32,
                },
            )
        }
        WaterExtent::Infinite => {
            if !flat(0.0) || far <= 0.0 {
                return None;
            }
            let snap = |v: f64| (v / SEA_CELL_M).round() * SEA_CELL_M;
            let center = DVec3::new(snap(eye.x), b.center.y, snap(eye.z));
            let half = far + SEA_CELL_M;
            let to_world = DAffine3::from_scale_rotation_translation(
                DVec3::new(half, 1.0, half),
                DQuat::IDENTITY,
                center,
            );
            let radius = (half * std::f64::consts::SQRT_2) as f32;
            (SQUARE, to_world, RaySphere { center, radius })
        }
    };
    Some(Placement {
        id: RayKey(b.id, 0),
        geometry,
        to_world,
        sphere,
        look: RayLook {
            albedo: b.albedo,
            roughness: b.roughness,
            ..RayLook::default()
        },
    })
}

/// What the water holds in the ray scene between frames.
#[derive(Default)]
pub(crate) struct WaterRays {
    /// The bodies of the main view's last prepare.
    pub bodies: Vec<RayBody>,
    /// Instances placed or parked in the scene.
    placed: HashSet<RayKey>,
    /// Ids of this frame's bodies (kept to save an allocation a frame).
    seen: HashSet<RayKey>,
}

impl WaterRays {
    /// Places every body within the far reach (seen by the camera or not: probes and
    /// reflections look behind it too) and takes out those that left.
    pub(crate) fn fill(&mut self, view: &RayView, out: &mut RaySceneOut<'_>) {
        let class = RayClass::Water;
        let mut ready = [false; 2];
        self.seen.clear();
        for b in &self.bodies {
            let Some(p) = placement(b, view.eye, view.far_reach) else {
                continue;
            };
            let g = usize::from(p.geometry == SQUARE);
            if !ready[g] {
                let source = if g == 1 {
                    RaySource::UnitSquare
                } else {
                    RaySource::UnitDisc
                };
                ready[g] = matches!(
                    out.geometry(p.geometry, 1, class.opaque(), source),
                    RayGeometryState::Ready | RayGeometryState::Older
                );
                if !ready[g] {
                    // Building (the frame's allowance is spent) or refused: asked again next
                    // frame, when every body is placed again.
                    continue;
                }
            }
            // Bodies hold still (a world shift moves them with the scene's own instances,
            // which then stand as placed).
            out.place(p.id, p.geometry, &p.to_world, p.sphere, class, 0, &p.look);
            self.seen.insert(p.id);
        }
        for id in self.placed.difference(&self.seen) {
            out.remove(*id);
        }
        std::mem::swap(&mut self.placed, &mut self.seen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(extent: WaterExtent) -> RayBody {
        RayBody {
            id: 7,
            center: DVec3::new(100.0, 12.0, -40.0),
            yaw: 0.0,
            extent,
            albedo: [0.01, 0.03, 0.05],
            roughness: 0.1,
        }
    }

    #[test]
    fn a_round_body_is_its_disc() {
        let b = body(WaterExtent::Circle { radius: 25.0 });
        let p = placement(&b, DVec3::new(100.0, 14.0, -40.0), 384.0).unwrap();
        assert_eq!(p.geometry, DISC);
        // The unit disc's rim lands on the shore, at the surface.
        let rim = p.to_world.transform_point3(DVec3::new(1.0, 0.0, 0.0));
        assert!((rim - DVec3::new(125.0, 12.0, -40.0)).length() < 1e-9);
        assert_eq!(p.sphere.radius, 25.0);
        assert_eq!(p.look.albedo, b.albedo);
        // Beyond the far reach it is left out; the reach counts from the shore.
        assert!(placement(&b, DVec3::new(100.0 + 25.0 + 383.0, 12.0, -40.0), 384.0).is_some());
        assert!(placement(&b, DVec3::new(100.0 + 25.0 + 385.0, 12.0, -40.0), 384.0).is_none());
    }

    #[test]
    fn a_rectangle_turns_with_its_yaw() {
        let mut b = body(WaterExtent::Rect {
            half_size: [30.0, 10.0],
        });
        b.yaw = 0.7;
        let p = placement(&b, b.center, 384.0).unwrap();
        assert_eq!(p.geometry, SQUARE);
        // A corner of the square is a corner of the body: inside the extent as the body's
        // own frame sees it, and on its edge.
        let corner = p.to_world.transform_point3(DVec3::new(1.0, 0.0, 1.0));
        let (s, c) = b.yaw.sin_cos();
        let d = (corner - b.center).as_vec3();
        let local = (c * d.x - s * d.z, s * d.x + c * d.z);
        assert!((local.0 - 30.0).abs() < 1e-3 && (local.1 - 10.0).abs() < 1e-3);
        assert!((corner.y - 12.0).abs() < 1e-9);
    }

    #[test]
    fn the_sea_moves_only_a_cell_at_a_time() {
        let b = body(WaterExtent::Infinite);
        let at = |x: f64| placement(&b, DVec3::new(x, 20.0, 5.0), 384.0).unwrap();
        // The camera's small moves leave it where it stands.
        assert_eq!(at(10.0).to_world, at(30.0).to_world);
        assert_ne!(at(10.0).to_world, at(10.0 + SEA_CELL_M).to_world);
        // It reaches past the far reach on every side, wherever the eye is within its cell.
        let p = at(31.9);
        let edge = p.to_world.transform_point3(DVec3::new(-1.0, 0.0, 0.0)).x;
        assert!(31.9 - edge >= 384.0);
        assert_eq!(p.to_world.translation.y, 12.0);
    }

    #[test]
    fn deep_water_takes_its_colour_from_what_is_in_it() {
        let clear = deep_albedo(&WaterOptics::clear_lake());
        // Blue-green over red, and dark: deep water sends back little.
        assert!(clear[2] > clear[0] && clear[1] > clear[0], "{clear:?}");
        assert!(clear.iter().all(|c| (0.0..0.2).contains(c)), "{clear:?}");
    }
}
