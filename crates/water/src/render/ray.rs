//! The water in the renderer's ray scene ([`RenderFeature::ray_scene`]): one flat stand-in per
//! body at its still surface, so that probe light and world reflections meet the water. A
//! disc for a round body, a square for a rectangular one, and for an endless sea a square
//! out past the far reach that moves only when the camera crosses a cell of a coarse grid.
//!
//! And the falls' white curtains, which the screen alone held before (a traced lake mirror
//! showed no fall where the drawn one showed its white column): per source of falls (a flow
//! region, the creeks' pour-overs, the storm's threads) one sheet down every lip's path, as
//! wide as its drawn curtain with the spray round it, white, casting no shadow
//! ([`RayClass::NoCast`]: probes and reflections see it). A flow works its falls out again five
//! times a second, mostly where they were: a source's sheets are built again only once a point
//! of them has moved [`CURTAIN_MOVE_M`], its lips change, or the world has shifted.
//!
//! [`RenderFeature::ray_scene`]: engine_render::features::RenderFeature::ray_scene

use std::collections::HashSet;
use std::sync::Arc;

use engine_core::math::{DAffine3, DQuat, DVec2, DVec3, Vec3};
use engine_core::prelude::Entity;
use engine_render::{
    RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RaySphere, RayView,
};

use crate::body::{WaterExtent, WaterOptics};
use crate::flow::Falls;
use crate::state::BodyState;

/// The endless sea's square snaps its centre to this grid, m, and reaches one cell past the
/// far reach, so that its place changes only when the camera crosses a cell: a still
/// instance that moves has its structure built again.
pub(crate) const SEA_CELL_M: f64 = 64.0;

/// The geometry every round body's disc shares.
const DISC: RayKey = RayKey(0, 0);
/// The geometry every square shares (a rectangular body, the endless sea).
const SQUARE: RayKey = RayKey(1, 0);

/// The name of a flow region's curtains, its geometry's and its instance's: its entity's bits
/// (a body's stand-in is `RayKey(bits, 0)`).
pub(crate) fn flow_falls(region: Entity) -> RayKey {
    RayKey(region.to_bits(), 1)
}
/// The creeks' pour-overs' curtains ([`crate::creeks::CreekRibbons::falls`]).
pub(crate) const CREEK_FALLS: RayKey = RayKey(0, 2);
/// The storm's threads down the cliffs ([`crate::runoff::StormRunoff::falls`]).
pub(crate) const STORM_FALLS: RayKey = RayKey(0, 3);
/// A source's curtains are built again once a point of them has moved this far from where it
/// was built, meters: a few pixels at a hundred meters, in a reflection of the turbulent white
/// of a fall.
pub(crate) const CURTAIN_MOVE_M: f32 = 0.25;
/// Below this radius a curtain is left out, meters, as anything that casts no shadow is
/// ([`RayView::caster_reach`]).
const LEAST_RADIUS_M: f32 = 0.3;
/// Most lips one source's curtains hold: four times as many as are drawn of every source
/// together ([`super::MAX_LIPS`], the nearest), more than any stream has. Past it the first in
/// the falls' order are kept, not the nearest.
const CURTAIN_MOST_LIPS: usize = 4 * super::MAX_LIPS;
/// What the falls' froth reflects of the light falling on it: `fs_falls` lights it 0.9 times
/// its streaks' 0.7 to 1.15 (water.wgsl), about 0.8 on the whole.
const FROTH_ALBEDO: f32 = 0.8;

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

/// The curtains of one source of falls as the ray scene is asked to hold them ([`curtain`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Curtain {
    /// The first lip's top (world): the positions are relative to it, and the instance
    /// stands there.
    pub origin: DVec3,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// Round every point (world).
    pub sphere: RaySphere,
}

impl Curtain {
    /// Whether `other` is this curtain moved by no more than `m` at any point: the same
    /// sheets, each point within `m` of where it stood.
    pub(crate) fn holds(&self, other: &Curtain, m: f32) -> bool {
        if self.positions.len() != other.positions.len() || self.indices != other.indices {
            return false;
        }
        let by = (other.origin - self.origin).as_vec3();
        self.positions
            .iter()
            .zip(&other.positions)
            .all(|(a, b)| (Vec3::from(*b) + by - Vec3::from(*a)).length_squared() <= m * m)
    }
}

/// The curtains of `falls` as the rays meet them: down each lip's path from its brink to its
/// foot, a sheet as wide as its drawn curtain with the spray round it (`fall_strip`'s core,
/// spreading as it falls and over the rock it slides down, [`super::sheet_half`], and the
/// aerated water and spray it spreads into on each side, [`super::jet_spread`]: the drawn
/// mirror reflects the spray's veil too), two points across it at each point of the path.
/// None without a lip with a path; at most [`CURTAIN_MOST_LIPS`] lips.
pub(crate) fn curtain(falls: &Falls) -> Option<Curtain> {
    let origin = falls.lips.first()?.top;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for (lip, path) in falls.lips.iter().zip(&falls.paths).take(CURTAIN_MOST_LIPS) {
        if path.len() < 2 {
            continue;
        }
        let across = Vec3::new(-lip.dir.y, 0.0, lip.dir.x);
        let last = (path.len() - 1) as f32;
        let first = positions.len() as u32;
        let mut spread = 0.0f32;
        for (k, point) in path.iter().enumerate() {
            if point.rock != Vec3::ZERO {
                spread += 0.06;
            }
            let drop = (lip.top.y - point.pos.y) as f32;
            let half =
                super::sheet_half(lip, k as f32 / last, spread) + super::jet_spread(lip, drop);
            let at = (point.pos - origin).as_vec3();
            for side in [-1.0f32, 1.0] {
                let p = at + across * (half * side);
                positions.push(p.to_array());
                let w = origin + p.as_dvec3();
                lo = lo.min(w);
                hi = hi.max(w);
            }
        }
        for k in 0..path.len() as u32 - 1 {
            let (a, b) = (first + 2 * k, first + 2 * k + 2);
            indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    if indices.is_empty() {
        return None;
    }
    Some(Curtain {
        origin,
        positions,
        indices,
        sphere: RaySphere {
            center: (lo + hi) * 0.5,
            radius: ((hi - lo).length() * 0.5) as f32,
        },
    })
}

/// Whether a thing round `sphere` that casts no shadow belongs in the scene `view` asks for:
/// from [`LEAST_RADIUS_M`] in radius within the caster reach, and past it to the far reach where
/// it is also at least [`RayView::cut`] of its distance across.
pub(crate) fn in_reach(sphere: &RaySphere, view: &RayView) -> bool {
    let d = sphere.distance(view.eye);
    sphere.radius >= LEAST_RADIUS_M
        && (d <= view.caster_reach || (d <= view.far_reach && sphere.radius >= view.cut * d))
}

/// The colour of the falls' froth with the streams `mud` muddy (`Freshet::mud`, 0 to 1): white,
/// light beige in a flood, as `flood_froth` (water.wgsl) tints the drawn curtains.
pub(crate) fn froth(mud: f32) -> [f32; 3] {
    let k = 0.4 * mud.clamp(0.0, 1.0);
    let flood = [0.92, 0.8, 0.64];
    std::array::from_fn(|i| FROTH_ALBEDO * (1.0 + (flood[i] - 1.0) * k))
}

/// One source's falls in the ray scene.
struct Falling {
    /// Its geometry's and its instance's name.
    id: RayKey,
    /// The falls last taken: held, so that no new falls can come at their address.
    from: Arc<Falls>,
    curtain: Curtain,
    /// Its geometry's version (a number no build had before).
    version: u64,
    /// `WaterState::shifts` when the curtain was built: once the world has shifted since, the
    /// scene has moved its instance along and its own world points are of the frame before.
    shifts: u64,
    /// Among this frame's sources.
    seen: bool,
}

/// What the water holds in the ray scene between frames.
#[derive(Default)]
pub(crate) struct WaterRays {
    /// The bodies of the main view's last prepare.
    pub bodies: Vec<RayBody>,
    /// The falls of the main view's last prepare ([`WaterRays::take_falls`]).
    falls: Vec<Falling>,
    /// `WaterState::shifts` at the main view's last prepare.
    shifts: u64,
    /// The last version a curtain was built at.
    version: u64,
    /// The falls' look (their froth's colour, [`froth`]).
    froth: [f32; 3],
    /// Instances placed or parked in the scene.
    placed: HashSet<RayKey>,
    /// Ids of this frame's bodies and curtains (kept to save an allocation a frame).
    seen: HashSet<RayKey>,
}

impl WaterRays {
    /// The falls the main view draws this frame: each source's name and falls (none while the
    /// scene is off), the world shifts followed so far (`WaterState::shifts`) and the froth's
    /// colour ([`froth`]). A source whose falls are new is built again where they moved by
    /// more than [`CURTAIN_MOVE_M`] at any point, a lip came or went (or a path's length
    /// changed), or the world has shifted since; one that is gone, or has no lip left, leaves.
    /// Falls not new since a shift (a creek's or the storm's: a shift does not move them) keep
    /// their instance where the scene moved it. New falls after a shift are taken as laid in
    /// the new frame, though a creek's or the storm's may still come from the frame before for
    /// a while (laid from rims and jobs a shift does not move) and stand off by the shift until
    /// then, as their drawn curtains do.
    pub(crate) fn take_falls<'a>(
        &mut self,
        sources: impl IntoIterator<Item = (RayKey, &'a Arc<Falls>)>,
        shifts: u64,
        froth: [f32; 3],
    ) {
        self.shifts = shifts;
        self.froth = froth;
        for f in &mut self.falls {
            f.seen = false;
        }
        for (id, falls) in sources {
            let at = self.falls.iter().position(|f| f.id == id);
            if let Some(f) = at.map(|i| &mut self.falls[i]) {
                if Arc::ptr_eq(&f.from, falls) {
                    f.seen = true;
                    continue;
                }
                f.from = falls.clone();
                let Some(c) = curtain(falls) else {
                    continue;
                };
                f.seen = true;
                if f.shifts == shifts && f.curtain.holds(&c, CURTAIN_MOVE_M) {
                    continue;
                }
                self.version += 1;
                (f.curtain, f.version, f.shifts) = (c, self.version, shifts);
            } else if let Some(c) = curtain(falls) {
                self.version += 1;
                self.falls.push(Falling {
                    id,
                    from: falls.clone(),
                    curtain: c,
                    version: self.version,
                    shifts,
                    seen: true,
                });
            }
        }
        self.falls.retain(|f| f.seen);
    }

    /// Places every body within the far reach (seen by the camera or not: probes and
    /// reflections look behind it too) and the falls' curtains in reach, and takes out those
    /// that left.
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
        let look = RayLook {
            albedo: self.froth,
            roughness: 1.0,
            double_sided: true,
            ..RayLook::default()
        };
        for f in &self.falls {
            let placed = self.placed.contains(&f.id);
            // Built before a world shift, from falls not new since: the scene has moved it
            // along with the world, and it stands so until they are new.
            if f.shifts != self.shifts || !in_reach(&f.curtain.sphere, view) {
                if f.shifts != self.shifts && placed {
                    self.seen.insert(f.id);
                }
                continue;
            }
            let source = RaySource::Cpu {
                positions: &f.curtain.positions,
                indices: &f.curtain.indices,
            };
            match out.geometry(f.id, f.version, RayClass::NoCast.opaque(), source) {
                RayGeometryState::Ready => {
                    let to_world = DAffine3::from_translation(f.curtain.origin);
                    let sphere = f.curtain.sphere;
                    out.place(f.id, f.id, &to_world, sphere, RayClass::NoCast, 0, &look);
                    self.seen.insert(f.id);
                }
                // An older version serves (or this one waits for the frame's allowance): its
                // instance stands where it was placed, moved along by a shift since, until
                // this one is built.
                RayGeometryState::Older | RayGeometryState::Building => {
                    if placed {
                        self.seen.insert(f.id);
                    }
                }
                RayGeometryState::Refused => {}
            }
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

    /// A brink of `n` lips 0.5 m apart along +z from (10, 9, 0), moved by `by`, each falling
    /// free 8.4 m off it along +x (its arc as `FlowSim::fall_path` gives it, evenly in time
    /// here).
    fn brink(n: usize, by: DVec3) -> Arc<Falls> {
        use crate::flow::{FALL_POINTS, FallPoint, Lip};
        let t_end = (2.0 * 8.4f32 / 9.81).sqrt();
        let mut falls = Falls::default();
        for k in 0..n {
            let lip = Lip {
                top: DVec3::new(10.0, 9.0, 0.5 * k as f64) + by,
                dir: engine_core::math::Vec2::X,
                speed: 2.0,
                q: 0.2,
                drop: 8.4,
                width: 0.5,
                launch: 0.0,
                turbulence: 0.18,
            };
            let path = (0..=FALL_POINTS)
                .map(|j| {
                    let t = t_end * j as f32 / FALL_POINTS as f32;
                    FallPoint {
                        pos: lip.top
                            + DVec3::new(2.0 * t as f64, -0.5 * 9.81 * (t * t) as f64, 0.0),
                        vel: Vec3::new(2.0, -9.81 * t, 0.0),
                        t,
                        rock: Vec3::ZERO,
                    }
                })
                .collect();
            falls.lips.push(lip);
            falls.paths.push(path);
            falls.sides.push([k > 0, k + 1 < n]);
        }
        Arc::new(falls)
    }

    #[test]
    fn a_curtain_hangs_down_each_lips_path() {
        let f = brink(2, DVec3::ZERO);
        let c = curtain(&f).unwrap();
        let points = crate::flow::FALL_POINTS + 1;
        assert_eq!(c.origin, f.lips[0].top);
        // Two points across the sheet at each point of each path, two triangles between rows.
        assert_eq!(c.positions.len(), 2 * points * 2);
        assert_eq!(c.indices.len(), 2 * (points - 1) * 6);
        assert!(c.indices.iter().all(|&i| (i as usize) < c.positions.len()));
        let at = |i: usize| c.origin + Vec3::from(c.positions[i]).as_dvec3();
        // At its brink the sheet spans 0.66 of its lip's width to each side of the lip's middle,
        // across the way it falls (+z here); at its foot 0.96 and the spray round the jet, as
        // far as a jet this size throws it (`spray_reach`, 0.95 m): 1.43 m.
        assert!((at(0) - DVec3::new(10.0, 9.0, -0.33)).length() < 1e-5);
        assert!((at(1) - DVec3::new(10.0, 9.0, 0.33)).length() < 1e-5);
        let foot = f.paths[0].last().unwrap().pos;
        let half = 0.48 + super::super::jet_spread(&f.lips[0], 8.4) as f64;
        assert!((half - 1.4287).abs() < 1e-3, "{half}");
        assert!((at(2 * points - 2) - (foot - DVec3::Z * half)).length() < 1e-4);
        assert!((at(2 * points - 1) - (foot + DVec3::Z * half)).length() < 1e-4);
        // The next lip's sheet hangs from its own brink, overlapping this one's.
        assert!((at(2 * points) - DVec3::new(10.0, 9.0, 0.5 - 0.33)).length() < 1e-5);
        for i in 0..c.positions.len() {
            assert!(at(i).distance(c.sphere.center) <= c.sphere.radius as f64 + 1e-4);
        }
        // A lip without a path is left out; falls without a lip have no curtain.
        let mut short = (*f).clone();
        short.paths[1].truncate(1);
        assert_eq!(curtain(&short).unwrap().positions.len(), 2 * points);
        assert!(curtain(&Falls::default()).is_none());
    }

    #[test]
    fn a_curtain_is_built_again_only_once_it_moves() {
        let mut rays = WaterRays::default();
        let id = RayKey(5, 1);
        let white = froth(0.0);
        let version = |r: &WaterRays| r.falls.iter().find(|f| f.id == id).map(|f| f.version);
        let a = brink(3, DVec3::ZERO);
        rays.take_falls([(id, &a)], 0, white);
        let v1 = version(&rays).unwrap();
        // The same falls again, then new ones a few centimeters off (a flow works its falls
        // out again five times a second): as built.
        rays.take_falls([(id, &a)], 0, white);
        let b = brink(3, DVec3::new(0.1, 0.0, 0.0));
        rays.take_falls([(id, &b)], 0, white);
        assert_eq!(version(&rays), Some(v1));
        assert_eq!(rays.falls[0].curtain, curtain(&a).unwrap());
        // Moved as much again and more, it is built again: a move counts from where it was
        // built, not from the falls before.
        let c = brink(3, DVec3::new(0.3, 0.0, 0.0));
        rays.take_falls([(id, &c)], 0, white);
        let v2 = version(&rays).unwrap();
        assert!(v2 > v1);
        assert_eq!(rays.falls[0].curtain, curtain(&c).unwrap());
        // A lip more, where the others stand.
        let d = brink(4, DVec3::new(0.3, 0.0, 0.0));
        rays.take_falls([(id, &d)], 0, white);
        let v3 = version(&rays).unwrap();
        assert!(v3 > v2);
        // The world shifts: falls not yet laid again stand as the scene moved them, and new
        // ones are built in the new frame even where they hold the same points.
        rays.take_falls([(id, &d)], 1, white);
        assert_eq!(version(&rays), Some(v3));
        assert!(rays.falls[0].shifts != rays.shifts);
        let e = brink(4, DVec3::new(0.3, 0.0, 0.0));
        rays.take_falls([(id, &e)], 1, white);
        let v4 = version(&rays).unwrap();
        assert!(v4 > v3);
        assert_eq!(rays.falls[0].shifts, rays.shifts);
        // Falls with no lip left leave, and come back at a version no build had (the scene may
        // still hold the last); a source no longer given leaves.
        rays.take_falls([(id, &Arc::new(Falls::default()))], 1, white);
        assert_eq!(version(&rays), None);
        rays.take_falls([(id, &a)], 1, white);
        assert!(version(&rays).unwrap() > v4);
        rays.take_falls(std::iter::empty(), 1, white);
        assert!(rays.falls.is_empty());
    }

    #[test]
    fn a_curtain_is_in_the_scene_as_far_as_what_casts_no_shadow() {
        let view = RayView {
            eye: DVec3::ZERO,
            frame: 0,
            epoch: 0,
            shifted: 0,
            caster_reach: 160.0,
            far_reach: 384.0,
            cut: 0.0175,
            ground_patches: 0,
        };
        let at = |d: f64, radius: f32| {
            let center = DVec3::new(d + radius as f64, 0.0, 0.0);
            in_reach(&RaySphere { center, radius }, &view)
        };
        // Within the caster reach from 0.3 m in radius; past it, where it is a cut of its
        // distance across: a 20 m fall to the far reach, a thread down a cliff not so far.
        assert!(at(150.0, 0.5));
        assert!(!at(150.0, 0.2));
        // Near, where its distance's cut is less, a thinner one is still left out.
        assert!(!at(8.0, 0.2));
        assert!(at(380.0, 10.0));
        assert!(!at(300.0, 5.0));
        assert!(!at(390.0, 10.0));
    }

    #[test]
    fn froth_is_white_and_light_beige_in_a_flood() {
        assert_eq!(froth(0.0), [FROTH_ALBEDO; 3]);
        let f = froth(1.0);
        assert!(
            f[0] > f[1] && f[1] > f[2] && f[2] > 0.8 * FROTH_ALBEDO,
            "{f:?}"
        );
    }

    #[test]
    fn deep_water_takes_its_colour_from_what_is_in_it() {
        let clear = deep_albedo(&WaterOptics::clear_lake());
        // Blue-green over red, and dark: deep water sends back little.
        assert!(clear[2] > clear[0] && clear[1] > clear[0], "{clear:?}");
        assert!(clear.iter().all(|c| (0.0..0.2).contains(c)), "{clear:?}");
    }
}
