//! A classic gaff-rigged clinker sailing dinghy, built procedurally: a 4.2 m lapstrake hull of
//! six strakes a side over steamed oak frames, with keelson, floorboards, thwarts, stern
//! sheets, a centerboard trunk and board, varnished mahogany trim, bronze fittings, a solid
//! spruce mast and its standing rigging ([`BoatModels::hull`]); the swinging rig (boom, gaff,
//! mast hoops, halyards) and the rudder with its tiller are separate models
//! ([`BoatModels::rig`], [`BoatModels::rudder`]) so the caller trims the sail and steers by
//! rotating them about +Y. The sail is not modeled: it is simulated cloth, and its corners
//! are in [`dims`].
//!
//! Boat frame: bow toward −Z, +X starboard, +Y up, origin at the designed waterline,
//! amidships, on the centerline. Every number the cloth, physics and steering rely on is in
//! [`dims`].
//!
//! The hull is lofted the way a builder lines off a clinker boat. Four boundary curves (keel
//! rabbet, stem rabbet, sheer, transom) bound a surface whose sections are shaped by their
//! deadrise and flare; strake lines divide each section's girth in fixed proportions, so the
//! plank ends spread evenly up the stem. Each plank is a real board: its lower edge laps over
//! the plank below by a land, so its outer face steps out by the plank thickness there (fading
//! to flush at the stem, where the planks are gained into the rabbet), with a rounded edge
//! that catches the light and shows wear.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::OnceLock;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::{Mesh, Vertex};

use super::kit::{Mat, Model, ModelBuilder, ensure_materials, register_mesh};
use super::meshkit::*;
use super::props::{board, rnd};
use super::texgen::noise3;
use dims::*;

/// The dinghy's fixed dimensions, in meters (boat frame unless noted: bow toward −Z, +X
/// starboard, +Y up, origin at the designed waterline, amidships, on the centerline). The
/// sail cloth, physics and steering are built around these numbers.
pub mod dims {
    // ---- Hull

    /// Stem face at the stemhead: the forward end of the hull.
    pub const STEM_Z: f32 = -2.15;
    /// Outer face of the upright transom: the after end of the hull.
    pub const TRANSOM_Z: f32 = 2.05;
    /// Nominal length overall as specified. The hull itself spans `STEM_Z..TRANSOM_Z`
    /// (4.2 m); the rudder reaches 0.4 m past the transom.
    pub const LOA: f32 = 4.3;
    /// Greatest beam over the planking, at the sheer.
    pub const BEAM: f32 = 1.62;
    /// Station of the greatest beam.
    pub const BEAM_Z: f32 = 0.2;
    /// Beam over the planking at the transom's top corners.
    pub const TRANSOM_BEAM: f32 = 1.17;
    /// Bottom of the keel at its lowest, amidships.
    pub const KEEL_Y: f32 = -0.20;
    /// Bottom of the keel at the forefoot, where it sweeps up into the stem.
    pub const FOREFOOT_Y: f32 = -0.06;
    /// Station of the forefoot.
    pub const FOREFOOT_Z: f32 = -1.90;
    /// Bottom of the keel at the transom.
    pub const KEEL_TRANSOM_Y: f32 = -0.12;
    /// Top of the sheer strake at its lowest, amidships.
    pub const SHEER_Y: f32 = 0.42;
    /// Sheer height where it meets the stem.
    pub const SHEER_STEM_Y: f32 = 0.62;
    /// Sheer height at the transom's top corners.
    pub const SHEER_TRANSOM_Y: f32 = 0.48;
    /// Strakes (planks) a side.
    pub const STRAKES: usize = 6;
    /// Center of the white boot-top stripe; antifouling covers the hull below the stripe,
    /// from the waterline (y = 0) down.
    pub const BOOT_TOP_Y: f32 = 0.02;
    /// Height of the boot-top stripe.
    pub const BOOT_TOP_WIDTH: f32 = 0.04;

    // ---- Mast and standing rigging (hull model)

    /// Station of the mast axis (x = 0).
    pub const MAST_Z: f32 = -1.20;
    /// Mast heel, standing in its step on the keelson.
    pub const MAST_FOOT_Y: f32 = -0.05;
    /// Top of the mast (its truck).
    pub const MAST_TOP_Y: f32 = 5.0;
    /// Mast diameter at deck (sheer) level; parallel up to 1.6 m, then tapering.
    pub const MAST_DIA_DECK: f32 = 0.085;
    /// Mast diameter at the top.
    pub const MAST_DIA_TOP: f32 = 0.055;
    /// Height of the masthead sheaves the halyards run over (their axles, athwartships).
    pub const MASTHEAD_SHEAVE_Y: f32 = 4.86;
    /// Height where the shrouds leave the mast (the hounds).
    pub const SHROUD_MAST_Y: f32 = 4.3;
    /// Starboard chainplate pin, where the shroud ends (port: x mirrored).
    pub const CHAINPLATE: [f32; 3] = [0.78, 0.42, -0.95];
    /// Height where the forestay leaves the mast.
    pub const FORESTAY_MAST_Y: f32 = 4.4;
    /// Stemhead fitting pin, where the forestay ends.
    pub const STEMHEAD: [f32; 3] = [0.0, 0.60, -2.12];

    // ---- Interior

    /// Center of the mid thwart's straight part (it bulges forward around the mast).
    pub const MID_THWART_Z: f32 = -0.98;
    /// Center of the aft thwart.
    pub const AFT_THWART_Z: f32 = 0.55;
    /// Top of the thwarts and stern-sheet benches.
    pub const THWART_Y: f32 = 0.24;
    /// Fore and aft ends of the centerboard trunk.
    pub const TRUNK_Z: [f32; 2] = [-0.6, 0.3];
    /// Lowest point of the centerboard (fully down).
    pub const BOARD_BOTTOM_Y: f32 = -0.95;

    // ---- Rig model (its own frame: origin on the mast axis at y = 0, boat axes; rotate it
    // about +Y to trim)

    /// Gooseneck: the boom's forward end.
    pub const GOOSENECK: [f32; 3] = [0.0, 1.05, 0.06];
    /// The boom's after end.
    pub const BOOM_END: [f32; 3] = [0.0, 1.08, 2.95];
    /// Greatest boom diameter (it tapers toward both ends).
    pub const BOOM_DIA: f32 = 0.065;
    /// Gaff jaws: the gaff's forward end, hugging the mast.
    pub const GAFF_JAWS: [f32; 3] = [0.0, 3.45, 0.06];
    /// Gaff peak: its after end.
    pub const GAFF_PEAK: [f32; 3] = [0.0, 4.77, 1.94];
    /// Angle of the gaff above horizontal, degrees.
    pub const GAFF_ANGLE_DEG: f32 = 35.0;
    /// Heights of the lowest and highest mast hoop.
    pub const HOOPS_Y: [f32; 2] = [1.15, 3.4];
    /// Inner radius of the mast hoops.
    pub const HOOP_INNER_R: f32 = 0.05;
    /// Sail corners (rig frame): throat, peak, clew, tack.
    pub const SAIL_THROAT: [f32; 3] = [0.0, 3.40, 0.10];
    /// Sail peak corner (rig frame).
    pub const SAIL_PEAK: [f32; 3] = [0.0, 4.70, 1.90];
    /// Sail clew corner (rig frame).
    pub const SAIL_CLEW: [f32; 3] = [0.0, 1.13, 2.85];
    /// Sail tack corner (rig frame).
    pub const SAIL_TACK: [f32; 3] = [0.0, 1.12, 0.10];

    // ---- Rudder model (its own frame: origin on the hinge axis at (0, 0, RUDDER_Z), boat
    // axes; rotate it about +Y to steer)

    /// Station of the rudder's hinge axis (just aft of the transom).
    pub const RUDDER_Z: f32 = 2.08;
    /// Lowest point of the rudder blade.
    pub const RUDDER_BOTTOM_Y: f32 = -0.75;
    /// Widest chord of the rudder blade.
    pub const RUDDER_CHORD: f32 = 0.35;
    /// Forward end of the tiller (rudder frame).
    pub const TILLER_END: [f32; 3] = [0.0, 0.55, -0.85];
    /// Heights of the gudgeons on the transom (the pintles rest on them).
    pub const GUDGEON_Y: [f32; 2] = [-0.02, 0.36];

    // ---- Preview stand

    /// Height of the waterline origin above the ground when the hull sits on the stand of
    /// [`super::boat_stand`].
    pub const STAND_DWL_Y: f32 = 1.05;
}

/// Plank thickness.
const T: f32 = 0.011;
/// Width of a plank land (where a plank laps over the one below).
const LAND: f32 = 0.022;
/// Half width of the rabbet, where the garboards land on the keel and stem.
const RABBET_X: f32 = 0.016;
/// Depth of the keel below its rabbet, and of the stem face ahead of its rabbet.
const KEEL_DEPTH: f32 = 0.045;
const STEM_DEPTH: f32 = 0.04;
/// Top of the stem timber, above the sheer.
const STEM_TOP_Y: f32 = 0.645;
/// Forward rake of the stem (meters forward per meter up).
const STEM_RAKE: f32 = 0.18;
/// Steamed frames: molded depth (off the planking) and sided width.
const RIB_D: f32 = 0.016;
const RIB_W: f32 = 0.026;
/// Relative widths of the strakes, garboard first.
const STRAKE_WEIGHTS: [f32; STRAKES] = [1.12, 1.0, 0.94, 0.94, 1.0, 1.04];
const TRANSOM_T: f32 = 0.022;
const THWART_T: f32 = 0.024;
const RISER_T: f32 = 0.016;
/// Gunwale cap (on the sheer strake): thickness.
const CAP_T: f32 = 0.019;
/// Plank rows along the hull.
const HULL_ROWS: usize = 96;

// ---------------------------------------------------------------------------------------
// Public API

/// The dinghy's three models; see the module docs for their frames.
#[derive(Debug, Clone)]
pub struct BoatModels {
    /// Hull with interior, mast and standing rigging (boat frame).
    pub hull: Model,
    /// Boom, gaff, hoops and halyards (rig frame: origin on the mast axis at y = 0; place it
    /// at `(0, 0, MAST_Z)` and rotate about +Y to trim).
    pub rig: Model,
    /// Rudder blade, pintles and tiller (rudder frame: origin on the hinge axis; place it at
    /// `(0, 0, RUDDER_Z)` and rotate about +Y to steer).
    pub rudder: Model,
}

/// The dinghy's models. Their meshes are registered under asset keys the first time a world
/// needs them; later calls only check that and return the same models.
pub fn boat_models(commands: &mut Commands) -> BoatModels {
    static MODELS: OnceLock<BoatModels> = OnceLock::new();
    commands.queue(ensure_materials);
    if let Some(models) = MODELS.get() {
        let probe = models.hull.keys().next().unwrap_or_default().to_string();
        commands.queue(move |world: &mut World| {
            let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
                return;
            };
            if meshes.loaded_by_key(&probe).is_none() {
                for (key, mesh) in build_boat().1 {
                    meshes.get_or_insert_keyed(&key, || mesh);
                }
            }
        });
        return models.clone();
    }
    let (models, meshes) = build_boat();
    for (key, mesh) in meshes {
        register_mesh(commands, key, mesh);
    }
    MODELS.get_or_init(|| models).clone()
}

/// Points on the outer hull shell from the keel up to the sheer (both sides, stem and
/// transom included), for a convex-hull collider. Boat frame.
pub fn hull_points() -> Vec<Vec3> {
    let hull = Hull::get();
    let mut pts = Vec::new();
    // Stem face and keel bottom on the centerline.
    for y in [STEM_TOP_Y, 0.3, 0.0] {
        pts.push(Vec3::new(0.0, y, STEM_Z + (STEM_TOP_Y - y) * STEM_RAKE));
    }
    pts.push(Vec3::new(0.0, FOREFOOT_Y, FOREFOOT_Z));
    for z in [-1.5, -0.8, 0.0, 0.8, 1.5, TRANSOM_Z] {
        pts.push(Vec3::new(0.0, keel_bottom(z), z));
    }
    let max_beam_u = hull.u_at_sheer_z(BEAM_Z);
    for u in [0.03, 0.12, 0.3, max_beam_u, 0.8, 1.0] {
        let row = hull.row(u);
        for f in [0.2, 0.55, 1.0] {
            let v = row.v_at(row.girth() * f);
            let p = hull.point(u, v) + hull.normal(u, v) * if f < 1.0 { 1.5 * T } else { T };
            pts.push(p);
            pts.push(Vec3::new(-p.x, p.y, p.z));
        }
    }
    pts
}

/// Cribbing and shores to stand the hull on, in their own frame: ground at y = 0, the boat's
/// origin goes at `(0, STAND_DWL_Y, 0)` (high enough that the centerboard, down, and the
/// rudder clear the ground).
pub fn boat_stand(commands: &mut Commands) -> Model {
    commands.queue(ensure_materials);
    let hull = Hull::get();
    let mut mb = ModelBuilder::new("boat_stand").grounded();
    let h = STAND_DWL_Y;
    let mut seed = 9100;
    let mut next = || {
        seed += 1;
        seed
    };
    // Under the keel: cribs of timbers laid crosswise in pairs, a pine wedge driven in on
    // top, and a block along the keel.
    const LAYER: f32 = 0.11;
    const WEDGE: f32 = 0.035;
    for z in [-0.85f32, 0.95] {
        let top = h + keel_bottom(z);
        let layers = ((top - WEDGE - 0.05) / LAYER).floor() as usize;
        for k in 0..layers {
            let y = LAYER * (k as f32 + 0.5);
            let jitter = (rnd(next(), 0) - 0.5) * 0.03;
            for s in [-1.0f32, 1.0] {
                let (c, size) = if k % 2 == 0 {
                    (
                        Vec3::new(jitter, y, z + s * 0.2),
                        Vec3::new(0.72, LAYER - 0.002, 0.13),
                    )
                } else {
                    (
                        Vec3::new(s * 0.22, y, z + jitter),
                        Vec3::new(0.13, LAYER - 0.002, 0.6),
                    )
                };
                mb.add(Mat::Timber, board_at_seed(c, size, next()));
            }
        }
        let base = LAYER * layers as f32;
        let wedge = extrude(
            &[
                Vec2::new(-0.22, 0.0),
                Vec2::new(0.22, 0.0),
                Vec2::new(0.22, 0.01),
                Vec2::new(-0.22, WEDGE),
            ],
            0.13,
            0.002,
            20.0,
        );
        mb.add(
            Mat::Pine,
            place(wedge, Vec3::new(0.0, base, z), Quat::IDENTITY),
        );
        let block = top - base - WEDGE + 0.004;
        mb.add(
            Mat::Pine,
            board_at_seed(
                Vec3::new(0.0, base + WEDGE + block * 0.5 - 0.004, z),
                Vec3::new(0.15, block, 0.42),
                next(),
            ),
        );
    }
    // Shores under the bilges, each on a foot block.
    for z in [-0.45f32, 1.05] {
        for side in [1.0f32, -1.0] {
            let (p, n) = hull.side(z, 0.06, -2.0 * T);
            let contact =
                Vec3::new(p.x * side, p.y + h, p.z) + Vec3::new(n.x * side, n.y, n.z) * 0.02;
            let foot = Vec3::new(side * (p.x + 0.5), 0.06, z);
            mb.add(
                Mat::Timber,
                board_at_seed(foot - Vec3::Y * 0.03, Vec3::new(0.3, 0.06, 0.2), next()),
            );
            let dir = contact - foot;
            mb.add(
                Mat::Timber,
                place(
                    board(Vec3::new(0.07, dir.length(), 0.07), next()),
                    foot + dir * 0.5,
                    super::props::frame_rot(dir, Vec3::Z),
                ),
            );
        }
    }
    mb.build(commands)
}

fn board_at_seed(c: Vec3, size: Vec3, seed: u32) -> Mesh {
    place(board(size, seed), c, Quat::IDENTITY)
}

/// The three models and their meshes (not registered).
fn build_boat() -> (BoatModels, Vec<(String, Mesh)>) {
    let start = std::time::Instant::now();
    let (hull, mut meshes) = hull_model().finish();
    let (rig, rig_meshes) = rig_model().finish();
    let (rudder, rudder_meshes) = rudder_model().finish();
    meshes.extend(rig_meshes);
    meshes.extend(rudder_meshes);
    let triangles: usize = meshes.iter().map(|(_, m)| m.triangle_count()).sum();
    engine_core::log::info!(
        "softbody yard: built the dinghy ({triangles} triangles) in {:.2} s",
        start.elapsed().as_secs_f64()
    );
    (BoatModels { hull, rig, rudder }, meshes)
}

// ---------------------------------------------------------------------------------------
// Hull lines

/// Height of the keel bottom along the hull (aft of the forefoot), with its rocker.
fn keel_bottom(z: f32) -> f32 {
    const LOW: f32 = -0.15;
    if z < LOW {
        KEEL_Y + (FOREFOOT_Y - KEEL_Y) * ((LOW - z) / (LOW - FOREFOOT_Z)).powi(2)
    } else {
        KEEL_Y + (KEEL_TRANSOM_Y - KEEL_Y) * ((z - LOW) / (TRANSOM_Z - LOW)).powi(2)
    }
}

/// Deadrise of the bottom at the keel (radians) at hull parameter `u` (0 stem, 1 transom):
/// a deep V forward easing to a flat run aft.
fn deadrise(u: f32) -> f32 {
    (10.0 + 50.0 * (-u / 0.16).exp() - 2.0 * u).to_radians()
}

/// Flare of the topsides at the sheer (radians from vertical).
fn flare(u: f32) -> f32 {
    (16.0 + 22.0 * (1.0 - u).powi(2)).to_radians()
}

fn bezier(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let s = 1.0 - t;
    s * s * s * a + 3.0 * s * s * t * b + 3.0 * s * t * t * c + t * t * t * d
}

/// Half-breadth (beyond the rabbet) at height fraction `v` of a section `w` wide and `h`
/// tall: a cubic leaving the keel at the deadrise and reaching the sheer at the flare.
fn section_x(dr: f32, fl: f32, w: f32, h: f32, v: f32) -> f32 {
    if w <= 1e-6 {
        return 0.0;
    }
    let l = (w * w + h * h).sqrt();
    let a = (0.5 * l).min(0.75 * w / dr.cos().max(0.05));
    let b = (0.38 * l).min(0.6 * w / fl.sin().max(0.05));
    let p1 = Vec2::new(a * dr.cos(), a * dr.sin());
    let p2 = Vec2::new(w - b * fl.sin(), h - b * fl.cos());
    let target = v * h;
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..26 {
        let t = 0.5 * (lo + hi);
        if bezier(0.0, p1.y, p2.y, h, t) < target {
            lo = t;
        } else {
            hi = t;
        }
    }
    bezier(0.0, p1.x, p2.x, w, 0.5 * (lo + hi))
}

/// The lofted hull: its profile (stem and keel) and the surface through the planking's
/// inner land line (the "mean" surface the planks are placed on).
struct Hull {
    /// Stem face and keel bottom as (z, y), from the stem top aft to the transom.
    outer: Vec<Vec2>,
    /// The rabbet (where the planking lands), same order.
    rabbet: Vec<Vec2>,
    /// Index of the forefoot in both: the stem above it, the keel aft of it.
    forefoot: usize,
    /// Rabbet at the forefoot (z, y), and its z where it meets the sheer.
    z_f: f32,
    y_f: f32,
    z_h: f32,
}

impl Hull {
    fn get() -> &'static Hull {
        static HULL: OnceLock<Hull> = OnceLock::new();
        HULL.get_or_init(Hull::new)
    }

    fn new() -> Hull {
        let stem_z = |y: f32| STEM_Z + (STEM_TOP_Y - y) * STEM_RAKE;
        let mut outer = Vec::new();
        let low = 0.10;
        for k in 0..=12 {
            let y = STEM_TOP_Y + (low - STEM_TOP_Y) * k as f32 / 12.0;
            outer.push(Vec2::new(stem_z(y), y));
        }
        let stem_end = outer.len() - 1;
        // The forefoot: a cubic from the raked stem into the keel's rocker, tangent to both.
        let p0 = outer[stem_end];
        let t0 = Vec2::new(STEM_RAKE, -1.0).normalize();
        let p3 = Vec2::new(FOREFOOT_Z, FOREFOOT_Y);
        let slope = (keel_bottom(FOREFOOT_Z + 1e-3) - keel_bottom(FOREFOOT_Z)) / 1e-3;
        let t3 = Vec2::new(1.0, slope).normalize();
        let (p1, p2) = (p0 + t0 * 0.09, p3 - t3 * 0.12);
        for k in 1..=16 {
            let t = k as f32 / 16.0;
            outer.push(Vec2::new(
                bezier(p0.x, p1.x, p2.x, p3.x, t),
                bezier(p0.y, p1.y, p2.y, p3.y, t),
            ));
        }
        let forefoot = outer.len() - 1;
        for k in 1..=64 {
            let z = FOREFOOT_Z + (TRANSOM_Z - FOREFOOT_Z) * k as f32 / 64.0;
            outer.push(Vec2::new(z, keel_bottom(z)));
        }
        let n = outer.len();
        let rabbet = (0..n)
            .map(|i| {
                let t = (outer[(i + 1).min(n - 1)] - outer[i.saturating_sub(1)]).normalize();
                let inward = Vec2::new(-t.y, t.x);
                let depth = STEM_DEPTH
                    + (KEEL_DEPTH - STEM_DEPTH)
                        * smoothstep(stem_end as f32, forefoot as f32, i as f32);
                outer[i] + inward * depth
            })
            .collect::<Vec<_>>();
        let mut hull = Hull {
            z_f: rabbet[forefoot].x,
            y_f: rabbet[forefoot].y,
            outer,
            rabbet,
            forefoot,
            z_h: 0.0,
        };
        hull.z_h = hull.stem_rabbet_z(SHEER_STEM_Y);
        hull
    }

    /// The stem rabbet's z at height `y`.
    fn stem_rabbet_z(&self, y: f32) -> f32 {
        let r = &self.rabbet[..=self.forefoot];
        if y >= r[0].y {
            return r[0].x;
        }
        for w in r.windows(2) {
            if y <= w[0].y && y >= w[1].y {
                let t = (w[0].y - y) / (w[0].y - w[1].y).max(1e-9);
                return w[0].x + (w[1].x - w[0].x) * t;
            }
        }
        r[r.len() - 1].x
    }

    /// The keel rabbet's height at `z` (aft of the forefoot).
    fn keel_rabbet_y(&self, z: f32) -> f32 {
        let r = &self.rabbet[self.forefoot..];
        if z <= r[0].x {
            return r[0].y;
        }
        for w in r.windows(2) {
            if z >= w[0].x && z <= w[1].x {
                let t = (z - w[0].x) / (w[1].x - w[0].x).max(1e-9);
                return w[0].y + (w[1].y - w[0].y) * t;
            }
        }
        r[r.len() - 1].y
    }

    /// Sheer height (top of the sheer strake) at `z`.
    fn sheer_y(&self, z: f32) -> f32 {
        const LOW: f32 = 0.15;
        if z < LOW {
            let s = ((LOW - z) / (LOW - self.z_h)).clamp(0.0, 1.0);
            SHEER_Y + (SHEER_STEM_Y - SHEER_Y) * s * s
        } else {
            let s = (z - LOW) / (TRANSOM_Z - LOW);
            SHEER_Y + (SHEER_TRANSOM_Y - SHEER_Y) * s * s
        }
    }

    /// Half-breadth over the planking at the sheer at `z`: a full, round bow, the greatest
    /// beam at `BEAM_Z`, quarters easing in to the transom.
    fn sheer_half_breadth(&self, z: f32) -> f32 {
        let half = BEAM * 0.5;
        if z < BEAM_Z {
            let s = ((z - self.z_h) / (BEAM_Z - self.z_h)).clamp(0.0, 1.0);
            RABBET_X + (half - RABBET_X) * (1.0 - (1.0 - s).powf(3.2))
        } else {
            let s = ((z - BEAM_Z) / (TRANSOM_Z - BEAM_Z)).clamp(0.0, 1.0);
            half - (half - TRANSOM_BEAM * 0.5) * s * s
        }
    }

    /// Keel and sheer stations of the section at `u`: the bow sections lean (their keel
    /// end trails aft along the curved stem), the rest are upright.
    fn frame(&self, u: f32) -> (f32, f32) {
        let zs = self.z_h + (TRANSOM_Z - self.z_h) * u;
        (zs + (self.z_f - self.z_h) * (1.0 - u).powi(6), zs)
    }

    /// `u` of the section whose sheer is at `z`.
    fn u_at_sheer_z(&self, z: f32) -> f32 {
        ((z - self.z_h) / (TRANSOM_Z - self.z_h)).clamp(0.0, 1.0)
    }

    /// The mean surface at `u` (0 stem, 1 transom) and `v` (0 keel rabbet, 1 sheer, linear
    /// in height).
    fn point(&self, u: f32, v: f32) -> Vec3 {
        let (zk, zs) = self.frame(u);
        let yk = self.keel_rabbet_y(zk);
        let ys = self.sheer_y(zs);
        let y = yk + (ys - yk) * v;
        // Coons blend: at u = 0 the section is the stem rabbet itself.
        let y_stem = self.y_f + (SHEER_STEM_Y - self.y_f) * v;
        let chord = self.z_f + (self.z_h - self.z_f) * v;
        let z = zk + (zs - zk) * v + (self.stem_rabbet_z(y_stem) - chord) * (1.0 - u).powi(6);
        let fl = flare(u);
        let w = (self.sheer_half_breadth(zs) - T * fl.cos() - RABBET_X).max(0.0);
        Vec3::new(RABBET_X + section_x(deadrise(u), fl, w, ys - yk, v), y, z)
    }

    /// Outward unit normal of the mean surface (starboard side).
    fn normal(&self, u: f32, v: f32) -> Vec3 {
        let e = 1e-3;
        let pu = self.point((u + e).min(1.0), v) - self.point((u - e).max(0.0), v);
        let pv = self.point(u, (v + e).min(1.0)) - self.point(u, (v - e).max(0.0));
        pv.cross(pu).normalize_or(Vec3::X)
    }

    /// Girth table of the section at `u`.
    fn row(&self, u: f32) -> Row {
        const N: usize = 160;
        let v: Vec<f32> = (0..=N).map(|j| (j as f32 / N as f32).powf(1.7)).collect();
        let mut s = vec![0.0f32; N + 1];
        let mut prev = self.point(u, 0.0);
        for j in 1..=N {
            let p = self.point(u, v[j]);
            s[j] = s[j - 1] + p.distance(prev);
            prev = p;
        }
        Row { u, v, s }
    }

    /// `u` where the surface at height fraction `v` reaches station `z`.
    fn u_at(&self, z: f32, v: f32) -> f32 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if self.point(mid, v).z < z {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    /// The hull side at station `z` and height `y`, moved inward by `inset` (negative:
    /// outward) along the surface normal; returns the point and the outward normal.
    fn side(&self, z: f32, y: f32, inset: f32) -> (Vec3, Vec3) {
        let v_of = |u: f32| {
            let (zk, zs) = self.frame(u);
            let (yk, ys) = (self.keel_rabbet_y(zk), self.sheer_y(zs));
            ((y - yk) / (ys - yk)).clamp(0.0, 1.0)
        };
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if self.point(mid, v_of(mid)).z < z {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let u = 0.5 * (lo + hi);
        let v = v_of(u);
        let n = self.normal(u, v);
        (self.point(u, v) - n * inset, n)
    }

    /// Height of the frames' inner face over the bottom at station `z`, `x` off the
    /// centerline.
    fn bottom_y(&self, z: f32, x: f32) -> f32 {
        let u = self.u_at(z, 0.08);
        let n = 96;
        let mut prev = (0.0f32, self.point(u, 0.0));
        for j in 1..=n {
            let v = (j as f32 / n as f32).powf(1.7);
            let p = self.point(u, v);
            if p.x >= x {
                let t = ((x - prev.1.x) / (p.x - prev.1.x).max(1e-6)).clamp(0.0, 1.0);
                let v = prev.0 + (v - prev.0) * t;
                let q = self.point(u, v);
                return q.y - self.normal(u, v).y * RIB_D;
            }
            prev = (v, p);
        }
        prev.1.y
    }
}

/// A section's girth table: arc length `s` (meters from the keel rabbet) at samples of `v`.
struct Row {
    u: f32,
    v: Vec<f32>,
    s: Vec<f32>,
}

impl Row {
    fn girth(&self) -> f32 {
        *self.s.last().unwrap_or(&0.0)
    }

    /// `v` at girth `s`.
    fn v_at(&self, s: f32) -> f32 {
        let s = s.clamp(0.0, self.girth());
        let j = self
            .s
            .partition_point(|&x| x < s)
            .clamp(1, self.s.len() - 1);
        let (s0, s1) = (self.s[j - 1], self.s[j]);
        let t = ((s - s0) / (s1 - s0).max(1e-9)).clamp(0.0, 1.0);
        self.v[j - 1] + (self.v[j] - self.v[j - 1]) * t
    }
}

/// Cumulative girth fractions of the strake lines (0 at the keel, 1 at the sheer).
fn strake_lines() -> [f32; STRAKES + 1] {
    let total: f32 = STRAKE_WEIGHTS.iter().sum();
    let mut c = [0.0f32; STRAKES + 1];
    for i in 0..STRAKES {
        c[i + 1] = c[i] + STRAKE_WEIGHTS[i] / total;
    }
    c
}

/// Plank rows along the hull, denser toward the bow where the lines curve hardest.
fn hull_rows(hull: &Hull) -> Vec<Row> {
    (0..=HULL_ROWS)
        .map(|k| hull.row((k as f32 / HULL_ROWS as f32).powf(1.35)))
        .collect()
}

/// Waterline (top of the antifouling) at station `z`: level, with a hint of sheer toward
/// the ends so it reads parallel to the water.
fn waterline(z: f32) -> f32 {
    BOOT_TOP_Y - BOOT_TOP_WIDTH * 0.5 + 0.012 * (z / 2.1).powi(2)
}

// ---------------------------------------------------------------------------------------
// Mesh helpers

/// Points, texture coordinates, normal hints and brightness of a grid surface being built
/// row by row.
#[derive(Default)]
struct GridAcc {
    pts: Vec<Vec<Vec3>>,
    uvs: Vec<Vec<Vec2>>,
    hints: Vec<Vec<Vec3>>,
    shade: Vec<Vec<f32>>,
}

impl GridAcc {
    fn mesh(&self, ring: bool) -> Mesh {
        let mut m = grid(&self.pts, &self.uvs, ring, |i, j| self.hints[i][j]);
        let nc = self.pts.first().map_or(0, Vec::len);
        if !self.shade.is_empty() {
            for (k, v) in m.vertices.iter_mut().enumerate() {
                let s = self.shade[k / nc][k % nc];
                v.color = [s, s, s, 1.0];
            }
        }
        m
    }
}

/// A smooth surface through `pts[row][col]`: normals from the grid's tangents, turned to
/// agree with `outward(row, col)`, and every triangle wound to face its normals. `ring`:
/// the last column repeats the first (a closed loop with a texture seam).
fn grid(
    pts: &[Vec<Vec3>],
    uvs: &[Vec<Vec2>],
    ring: bool,
    outward: impl Fn(usize, usize) -> Vec3,
) -> Mesh {
    let nr = pts.len();
    let nc = pts.first().map_or(0, Vec::len);
    let mut m = Mesh::default();
    if nr < 2 || nc < 2 {
        return m;
    }
    for i in 0..nr {
        for j in 0..nc {
            let a = pts[(i + 1).min(nr - 1)][j] - pts[i.saturating_sub(1)][j];
            let (jm, jp) = if ring {
                (
                    if j == 0 { nc - 2 } else { j - 1 },
                    if j == nc - 1 { 1 } else { j + 1 },
                )
            } else {
                (j.saturating_sub(1), (j + 1).min(nc - 1))
            };
            let b = pts[i][jp] - pts[i][jm];
            let hint = outward(i, j);
            let mut n = b.cross(a);
            if n.length_squared() < 1e-16 {
                n = hint;
            }
            let mut n = n.normalize_or(Vec3::Y);
            if n.dot(hint) < 0.0 {
                n = -n;
            }
            m.vertices.push(Vertex::new(pts[i][j], n, uvs[i][j]));
        }
    }
    for i in 0..nr - 1 {
        for j in 0..nc - 1 {
            let a = (i * nc + j) as u32;
            let b = a + 1;
            let d = a + nc as u32;
            let c = d + 1;
            facing(&mut m, a, b, c);
            facing(&mut m, a, c, d);
        }
    }
    m
}

/// Append triangle `a b c` wound to agree with its vertices' normals; drop it if degenerate.
fn facing(m: &mut Mesh, a: u32, b: u32, c: u32) {
    let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
    let n: Vec3 = [a, b, c]
        .iter()
        .map(|&i| Vec3::from(m.vertices[i as usize].normal))
        .sum();
    let g = (p(b) - p(a)).cross(p(c) - p(a));
    if g.length_squared() < 1e-18 {
        return;
    }
    if g.dot(n) >= 0.0 {
        m.indices.extend_from_slice(&[a, b, c]);
    } else {
        m.indices.extend_from_slice(&[a, c, b]);
    }
}

fn lerp_vertex(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let l3 = |x: [f32; 3], y: [f32; 3]| Vec3::from(x).lerp(Vec3::from(y), t);
    let mut v = Vertex::new(
        l3(a.position, b.position),
        l3(a.normal, b.normal).normalize_or(Vec3::from(a.normal)),
        Vec2::from(a.uv).lerp(Vec2::from(b.uv), t),
    );
    v.color = Vec4::from(a.color).lerp(Vec4::from(b.color), t).to_array();
    v
}

/// Cut `m` where `f` (a field over positions, interpolated linearly along edges) crosses
/// zero: returns the parts where `f < 0` and `f >= 0`, with shared vertices kept shared.
fn split(m: &Mesh, f: impl Fn(Vec3) -> f32) -> (Mesh, Mesh) {
    struct Out {
        mesh: Mesh,
        map: Vec<u32>,
        edges: HashMap<(u32, u32), u32>,
    }
    impl Out {
        fn vert(&mut self, src: &Mesh, i: u32) -> u32 {
            if self.map[i as usize] == u32::MAX {
                self.map[i as usize] = self.mesh.vertices.len() as u32;
                self.mesh.vertices.push(src.vertices[i as usize]);
            }
            self.map[i as usize]
        }
        fn edge(&mut self, src: &Mesh, vals: &[f32], a: u32, b: u32) -> u32 {
            let key = (a.min(b), a.max(b));
            if let Some(&i) = self.edges.get(&key) {
                return i;
            }
            let (lo, hi) = key;
            let (fa, fb) = (vals[lo as usize], vals[hi as usize]);
            let t = (fa / (fa - fb)).clamp(0.0, 1.0);
            let v = lerp_vertex(&src.vertices[lo as usize], &src.vertices[hi as usize], t);
            let i = self.mesh.vertices.len() as u32;
            self.mesh.vertices.push(v);
            self.edges.insert(key, i);
            i
        }
    }
    let vals: Vec<f32> = m
        .vertices
        .iter()
        .map(|v| f(Vec3::from(v.position)))
        .collect();
    let new_out = || Out {
        mesh: Mesh::default(),
        map: vec![u32::MAX; m.vertices.len()],
        edges: HashMap::new(),
    };
    let (mut neg, mut pos) = (new_out(), new_out());
    for t in m.indices.as_chunks::<3>().0 {
        let inside = |i: u32| vals[i as usize] >= 0.0;
        for (out, keep) in [(&mut neg, false), (&mut pos, true)] {
            let mut poly = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if inside(a) == keep {
                    poly.push(out.vert(m, a));
                }
                if inside(a) != inside(b) {
                    poly.push(out.edge(m, &vals, a, b));
                }
            }
            for k in 1..poly.len().saturating_sub(1) {
                out.mesh
                    .indices
                    .extend_from_slice(&[poly[0], poly[k], poly[k + 1]]);
            }
        }
    }
    (neg.mesh, pos.mesh)
}

/// Add a surface painted by height: antifouling below the waterline, the white boot-top
/// stripe, and `above` over it. `dz` is the boat-frame station of the mesh's origin.
fn add_painted_at(mb: &mut ModelBuilder, mesh: Mesh, above: Mat, dz: f32) {
    let (below, rest) = split(&mesh, |p| p.y - waterline(p.z + dz));
    let (stripe, top) = split(&rest, |p| p.y - waterline(p.z + dz) - BOOT_TOP_WIDTH);
    mb.add(Mat::Antifoul, below);
    mb.add(Mat::BootTop, stripe);
    mb.add(above, top);
}

/// [`add_painted_at`] for a mesh in the boat frame.
fn add_painted(mb: &mut ModelBuilder, mesh: Mesh, above: Mat) {
    add_painted_at(mb, mesh, above, 0.0);
}

/// Sweep closed profiles along `path`. `profile(i)` gives ring `i` as (side, up) points,
/// counter-clockwise; `up[i]` orients it (side = up × tangent). Texture u runs along the
/// path and v around the profile, both in meters. `caps` closes both ends.
fn sweep(path: &[Vec3], up: &[Vec3], profile: impl Fn(usize) -> Vec<Vec2>, caps: bool) -> Mesh {
    let n = path.len();
    let mut acc = GridAcc::default();
    let mut along = 0.0;
    let mut frames = Vec::with_capacity(n);
    for i in 0..n {
        let t = (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec3::Z);
        let mut u = up[i] - t * up[i].dot(t);
        if u.length_squared() < 1e-10 {
            u = t.any_orthonormal_vector();
        }
        let u = u.normalize();
        let s = u.cross(t);
        if i > 0 {
            along += path[i].distance(path[i - 1]);
        }
        let prof = profile(i);
        let c = prof.iter().copied().sum::<Vec2>() / prof.len().max(1) as f32;
        let center = path[i] + s * c.x + u * c.y;
        let mut pts = Vec::with_capacity(prof.len() + 1);
        let mut uvs = Vec::with_capacity(prof.len() + 1);
        let mut hints = Vec::with_capacity(prof.len() + 1);
        let mut around = 0.0;
        for k in 0..=prof.len() {
            let q = prof[k % prof.len()];
            if k > 0 {
                around += q.distance(prof[k - 1]);
            }
            let p = path[i] + s * q.x + u * q.y;
            pts.push(p);
            uvs.push(Vec2::new(along, around));
            hints.push(p - center);
        }
        acc.pts.push(pts);
        acc.uvs.push(uvs);
        acc.hints.push(hints);
        frames.push((t, s, u, center));
    }
    let mut m = acc.mesh(true);
    if caps && n >= 2 {
        for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
            let (t, s, u, center) = frames[i];
            let nrm = t * sign;
            let prof = profile(i);
            let base = m.vertices.len() as u32;
            m.vertices.push(Vertex::new(center, nrm, Vec2::ZERO));
            for q in &prof {
                let p = path[i] + s * q.x + u * q.y;
                m.vertices.push(Vertex::new(p, nrm, *q));
            }
            let k = prof.len() as u32;
            for j in 0..k {
                facing(&mut m, base, base + 1 + j, base + 1 + (j + 1) % k);
            }
        }
    }
    m
}

/// A counter-clockwise rounded rectangle spanning `a0..a1` × `b0..b1` (corner radius `r`,
/// `segs` segments per corner).
fn rrect(a0: f32, a1: f32, b0: f32, b1: f32, r: f32, segs: u32) -> Vec<Vec2> {
    let r = r.min(0.49 * (a1 - a0)).min(0.49 * (b1 - b0)).max(0.0);
    let corners = [
        (Vec2::new(a1 - r, b0 + r), -FRAC_PI_2),
        (Vec2::new(a1 - r, b1 - r), 0.0),
        (Vec2::new(a0 + r, b1 - r), FRAC_PI_2),
        (Vec2::new(a0 + r, b0 + r), PI),
    ];
    let mut out = Vec::new();
    for (c, start) in corners {
        for k in 0..=segs {
            let a = start + FRAC_PI_2 * k as f32 / segs as f32;
            out.push(c + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    out
}

/// A circle of radius `r`, counter-clockwise.
fn circle(r: f32, n: u32) -> Vec<Vec2> {
    (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * TAU;
            Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

/// A round spar from `a` to `b` with `radius(t)` (t = 0..1 along it) and rounded ends
/// (`ends` = rounding radius as a fraction of the end radius, per end; 0 = flat cap).
fn spar(
    a: Vec3,
    b: Vec3,
    radius: impl Fn(f32) -> f32,
    rings: usize,
    sides: u32,
    ends: (f32, f32),
) -> Mesh {
    let len = a.distance(b);
    let dir = (b - a) / len.max(1e-6);
    // (distance along, radius factor) samples, with quarter-circle ends.
    let mut st: Vec<(f32, f32)> = Vec::new();
    let e0 = ends.0 * radius(0.0);
    let e1 = ends.1 * radius(1.0);
    if e0 > 0.0 {
        for k in 0..3 {
            let th = k as f32 / 3.0 * FRAC_PI_2;
            st.push((e0 * (1.0 - th.sin()), th.sin().max(0.02)));
        }
    }
    for k in 0..=rings {
        st.push((e0 + (len - e0 - e1) * k as f32 / rings as f32, 1.0));
    }
    if e1 > 0.0 {
        for k in (0..3).rev() {
            let th = k as f32 / 3.0 * FRAC_PI_2;
            st.push((len - e1 * (1.0 - th.sin()), th.sin().max(0.02)));
        }
    }
    let path: Vec<Vec3> = st.iter().map(|&(d, _)| a + dir * d).collect();
    let up = vec![dir.any_orthonormal_vector(); path.len()];
    let circ = circle(1.0, sides);
    sweep(
        &path,
        &up,
        |i| {
            let (d, f) = st[i];
            let r = radius((d / len).clamp(0.0, 1.0)) * f;
            circ.iter().map(|&p| p * r).collect()
        },
        e0 <= 0.0 || e1 <= 0.0,
    )
}

/// Normalized NACA 00xx half-thickness (1 at its thickest, 0 at both edges).
fn naca(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    (0.2969 * x.sqrt() - 0.126 * x - 0.3516 * x * x + 0.2843 * x * x * x - 0.1036 * x * x * x * x)
        / 0.1
}

/// Half-thickness of a section at chord fraction `x` (about 1 at its thickest): the NACA
/// foil, blended by `flat` toward a flat-sided board with rounded edges.
fn foil_shape(x: f32, flat: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let board = (x / 0.06).sqrt().min(1.0) * ((1.0 - x) / 0.06).sqrt().min(1.0);
    naca(x) * (1.0 - flat) + board * flat
}

/// A blade with a foil section: `stations` (leading edge, trailing edge, greatest
/// thickness, flatness for [`foil_shape`]) from root to tip; its faces point along ±`side`.
/// `cap_root` closes the root.
fn foil(stations: &[(Vec3, Vec3, f32, f32)], side: Vec3, chord_n: usize, cap_root: bool) -> Mesh {
    let xs: Vec<f32> = (0..chord_n)
        .map(|k| 0.5 - 0.5 * (PI * k as f32 / (chord_n - 1) as f32).cos())
        .collect();
    let mut acc = GridAcc::default();
    let mut span = 0.0;
    for (i, &(le, te, th, flat)) in stations.iter().enumerate() {
        if i > 0 {
            let (ple, pte, _, _) = stations[i - 1];
            span += (le + te).distance(ple + pte) * 0.5;
        }
        let chord = te - le;
        let (mut pts, mut uvs, mut hints) = (Vec::new(), Vec::new(), Vec::new());
        let order = (0..chord_n)
            .rev()
            .map(|k| (k, 1.0f32))
            .chain((1..chord_n).map(|k| (k, -1.0f32)));
        for (k, sgn) in order {
            let x = xs[k];
            let p = le + chord * x + side * (sgn * 0.5 * th * foil_shape(x, flat));
            pts.push(p);
            uvs.push(Vec2::new(span, chord.length() * x * sgn));
            hints.push(if k == 0 {
                -chord
            } else if k == chord_n - 1 {
                chord
            } else {
                side * sgn
            });
        }
        acc.pts.push(pts);
        acc.uvs.push(uvs);
        acc.hints.push(hints);
    }
    let mut m = acc.mesh(false);
    if cap_root && stations.len() > 1 {
        let nrm = (stations[0].0 - stations[1].0).normalize_or(Vec3::Y);
        let ring = &acc.pts[0];
        let base = m.vertices.len() as u32;
        for p in ring {
            m.vertices
                .push(Vertex::new(*p, nrm, Vec2::new(p.z + p.y, p.x)));
        }
        // The ring runs trailing edge → leading edge on one face and back on the other:
        // stitch the two faces together.
        let k = ring.len() as u32;
        for j in 0..k / 2 {
            let (a, b) = (base + j, base + j + 1);
            let (c, d) = (base + k - 1 - j, base + k - 2 - j);
            if b < d {
                facing(&mut m, a, b, d);
                facing(&mut m, a, d, c);
            } else if b == d {
                facing(&mut m, a, b, c);
            }
        }
    }
    m
}

/// Make a polygon counter-clockwise.
fn ccw(mut poly: Vec<Vec2>) -> Vec<Vec2> {
    let area: f32 = (0..poly.len())
        .map(|i| poly[i].perp_dot(poly[(i + 1) % poly.len()]))
        .sum();
    if area < 0.0 {
        poly.reverse();
    }
    poly
}

/// A board cut to the plan outline `poly` (x, z), `thick` meters thick with its top at
/// `top`; texture u runs along x.
fn slab(poly: Vec<Vec2>, top: f32, thick: f32, bevel: f32) -> Mesh {
    let m = extrude(&ccw(poly), thick, bevel, 35.0);
    xf(
        &m,
        Mat4::from_rotation_translation(
            Quat::from_rotation_x(FRAC_PI_2),
            Vec3::new(0.0, top - thick * 0.5, 0.0),
        ),
    )
}

/// A plate cut to the side-view outline `poly` (z, y), `thick` meters thick, centered on
/// `x`.
fn side_slab(poly: Vec<Vec2>, x: f32, thick: f32, bevel: f32) -> Mesh {
    let m = extrude(&ccw(poly), thick, bevel, 35.0);
    xf(
        &m,
        Mat4::from_rotation_translation(Quat::from_rotation_y(-FRAC_PI_2), Vec3::new(x, 0.0, 0.0)),
    )
}

/// Swap texture u and v (so the grain runs the other way).
fn swap_uv(mut m: Mesh) -> Mesh {
    for v in &mut m.vertices {
        v.uv = [v.uv[1], v.uv[0]];
    }
    m
}

/// A cast horn cleat `len` long along X, standing on y = 0.
fn horn_cleat(len: f32) -> Mesh {
    let h = len * 0.32;
    let path = smooth_path(
        &[
            Vec3::new(-0.5 * len, h * 0.72, 0.0),
            Vec3::new(-0.3 * len, h * 0.92, 0.0),
            Vec3::new(0.0, h, 0.0),
            Vec3::new(0.3 * len, h * 0.92, 0.0),
            Vec3::new(0.5 * len, h * 0.72, 0.0),
        ],
        6,
    );
    let n = path.len();
    let up = vec![Vec3::Y; n];
    let r = len * 0.075;
    let mut m = sweep(
        &path,
        &up,
        |i| {
            let t = i as f32 / (n - 1) as f32;
            circle(r * (1.0 - 0.45 * (2.0 * t - 1.0).powi(2)), 10)
        },
        true,
    );
    for x in [-0.17 * len, 0.17 * len] {
        m.merge(&super::props::turned(
            &[
                (0.0, 0.0),
                (r * 1.5, 0.0),
                (r * 1.3, h * 0.25),
                (r * 0.95, h * 0.85),
                (0.0, h * 0.9),
            ],
            10,
            Vec3::new(x, 0.0, 0.0),
        ));
    }
    m.merge(&place(
        rounded_box(
            Vec3::new(len * 0.5, h * 0.12, r * 2.4),
            h * 0.04,
            1,
            1.0,
            BoxUv::PLANAR,
        ),
        Vec3::Y * h * 0.06,
        Quat::IDENTITY,
    ));
    m
}

/// A bronze rigging screw (turnbuckle) from `a` along `dir`, `len` long.
fn turnbuckle(a: Vec3, dir: Vec3, len: f32) -> Mesh {
    let r = 0.0085;
    let p = [
        (0.0, 0.0),
        (0.004, 0.0),
        (0.004, 0.15 * len),
        (r * 0.8, 0.22 * len),
        (r, 0.26 * len),
        (r, 0.74 * len),
        (r * 0.8, 0.78 * len),
        (0.004, 0.85 * len),
        (0.004, len),
        (0.0, len),
    ];
    let m = super::props::turned(&p, 12, Vec3::ZERO);
    place(m, a, Quat::from_rotation_arc(Vec3::Y, dir.normalize()))
}

/// A ring (torus) of `major` radius around `axis` at `c`.
fn ring_at(c: Vec3, axis: Vec3, major: f32, minor: f32) -> Mesh {
    place(
        torus(major, minor, 20, 8),
        c,
        Quat::from_rotation_arc(Vec3::Y, axis.normalize()),
    )
}

/// Domed bolt heads at `points` on a surface facing `normal`.
fn bolts(points: &[Vec3], normal: Vec3, r: f32) -> Mesh {
    super::props::bolt_heads(points, normal, r)
}

/// A rope (laid-rope texture) through `points`, radius `r`.
fn rope(points: &[Vec3], r: f32) -> Mesh {
    super::props::rope_line(points, r)
}

// ---------------------------------------------------------------------------------------
// Hull model

/// Mast radius at height `y`: parallel up to 1.6 m, then an easing taper to the top.
fn mast_radius(y: f32) -> f32 {
    let (r0, r1) = (MAST_DIA_DECK * 0.5, MAST_DIA_TOP * 0.5);
    let t = ((y - 1.6) / (MAST_TOP_Y - 1.6)).clamp(0.0, 1.0);
    r0 - (r0 - r1) * t.powf(1.6)
}

/// Station of the halyard cleats on the mid thwart's bulge, ahead of the mast.
const CLEAT_Z: f32 = -1.32;

fn hull_model() -> ModelBuilder {
    let hull = Hull::get();
    let rows = hull_rows(hull);
    let mut mb = ModelBuilder::new("boat_hull");
    planking(&mut mb, hull, &rows);
    keel_and_stem(&mut mb, hull);
    transom(&mut mb, hull);
    frames(&mut mb, hull);
    bottom(&mut mb, hull);
    gunwales(&mut mb, hull);
    thwarts(&mut mb, hull);
    trunk(&mut mb, hull);
    mast(&mut mb);
    rigging(&mut mb, hull);
    fittings(&mut mb, hull);
    oars(&mut mb);
    mb
}

/// One strake's cross-section at a row, as (girth, offset) pairs: the outer surface from the
/// lower edge's inner corner round its rounded outer corner and up the face, and the inner
/// face from the top down. `lines` are the strake lines' girths; `tau` scales the land (0 at
/// the stem, where the planks are gained flush, 1 elsewhere).
fn strake_profile(i: usize, lines: &[f32], tau: f32) -> (Vec<Vec2>, Vec<Vec2>) {
    let (si, sj) = (lines[i], lines[i + 1]);
    // A hair of step stays at the stem so overlapping faces never coincide.
    let step = tau.max(0.08) * T;
    let w = (LAND * tau.max(0.3)).min(0.3 * (sj - si));
    let lo = if i == 0 { si } else { si - w };
    let m = sj - w;
    let d_in = |x: f32| {
        if x <= si {
            step
        } else if x >= m {
            0.0
        } else {
            step * (1.0 - (x - si) / (m - si))
        }
    };
    let d_out = |x: f32| d_in(x) + T;
    let r = 0.0035;
    let top = d_out(lo);
    let mut outer = vec![Vec2::new(lo, d_in(lo)), Vec2::new(lo, top - r)];
    for k in 1..=2 {
        let a = PI - FRAC_PI_2 * k as f32 / 3.0;
        outer.push(Vec2::new(lo + r + r * a.cos(), top - r + r * a.sin()));
    }
    outer.push(Vec2::new(lo + r, top));
    let face = if i == 0 {
        [0.25 * m, 0.5 * m, 0.75 * m, m, sj]
    } else {
        [si, si + (m - si) / 3.0, si + 2.0 * (m - si) / 3.0, m, sj]
    };
    outer.extend(face.iter().map(|&x| Vec2::new(x, d_out(x))));
    let inner_s: Vec<f32> = if i == 0 {
        vec![sj, m, 0.75 * m, 0.5 * m, 0.25 * m, lo]
    } else {
        vec![sj, m, face[2], face[1], si, lo]
    };
    let inner = inner_s.iter().map(|&x| Vec2::new(x, d_in(x))).collect();
    (outer, inner)
}

/// Place a strake cross-section on the hull: points, outward hints (from the profile's
/// left-hand normal: `up` along the section, the surface normal across it) and meter UVs.
fn place_profile(
    prof: &[Vec2],
    eval: &mut impl FnMut(f32) -> (Vec3, Vec3),
    up: Vec3,
) -> (Vec<Vec3>, Vec<Vec3>, Vec<Vec2>) {
    let n = prof.len();
    let (mut pts, mut hints, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..n {
        let (m, nn) = eval(prof[j].x);
        let p = m + nn * prof[j].y;
        let t = prof[(j + 1).min(n - 1)] - prof[j.saturating_sub(1)];
        let left = Vec2::new(-t.y, t.x).normalize_or(Vec2::Y);
        pts.push(p);
        hints.push(up * left.x + nn * left.y);
        uvs.push(Vec2::new(p.z, prof[j].x + prof[j].y));
    }
    (pts, hints, uvs)
}

/// The clinker planking, both sides: painted outside (antifouling, boot-top, topsides; the
/// plank lands worn lighter in patches), cream inside, plank ends showing at the transom.
fn planking(mb: &mut ModelBuilder, hull: &Hull, rows: &[Row]) {
    // Paint wear on the outer columns: the land edge and its rounded corner.
    const EDGE_WEAR: [f32; 6] = [0.0, 0.5, 1.2, 1.25, 0.7, 0.15];
    // Occlusion in the crevice under each land: the plank's lower edge, and the face of the
    // plank below where it disappears under it.
    const LAND_AO: [f32; 10] = [0.5, 0.8, 1.0, 1.0, 1.0, 1.0, 1.0, 0.97, 0.66, 0.6];
    let fr = strake_lines();
    let mut outside = Mesh::default();
    let mut inside = Mesh::default();
    for i in 0..STRAKES {
        let (mut out, mut inn, mut edge) =
            (GridAcc::default(), GridAcc::default(), GridAcc::default());
        let mut end: Option<(Vec<Vec2>, Vec<Vec3>)> = None;
        for (k, row) in rows.iter().enumerate() {
            let g = row.girth();
            let lines: Vec<f32> = fr.iter().map(|c| c * g).collect();
            let tau = smoothstep(0.0, 0.12, row.u);
            let (outer, inner) = strake_profile(i, &lines, tau);
            let mut cache: Vec<(f32, Vec3, Vec3)> = Vec::new();
            let mut eval = |s: f32| {
                if let Some(&(_, p, n)) = cache.iter().find(|c| c.0 == s) {
                    return (p, n);
                }
                let v = row.v_at(s);
                let r = (hull.point(row.u, v), hull.normal(row.u, v));
                cache.push((s, r.0, r.1));
                r
            };
            let hi = eval(lines[i + 1]).0;
            let up = (hi - eval(outer[0].x).0).normalize_or(Vec3::Y);
            let wear = smoothstep(
                0.5,
                0.8,
                0.5 + 0.5 * noise3(Vec3::new(hi.z * 2.3, i as f32 * 5.3, 1.7), 91),
            );
            let (pts, hints, uvs) = place_profile(&outer, &mut eval, up);
            let shade = (0..pts.len())
                .map(|j| {
                    (1.0 + wear * EDGE_WEAR.get(j).copied().unwrap_or(0.0))
                        * LAND_AO.get(j).copied().unwrap_or(1.0)
                })
                .collect();
            let out_pts = pts.clone();
            out.pts.push(pts);
            out.hints.push(hints);
            out.uvs.push(uvs);
            out.shade.push(shade);
            let (pts, hints, uvs) = place_profile(&inner, &mut eval, up);
            // Bilge dirt low down inside.
            let shade = pts
                .iter()
                .map(|p| 0.78 + 0.22 * smoothstep(-0.12, 0.16, p.y))
                .collect();
            if k == rows.len() - 1 {
                let mut poly = outer.clone();
                poly.extend_from_slice(&inner[..inner.len() - 1]);
                let mut p3 = out_pts;
                p3.extend_from_slice(&pts[..pts.len() - 1]);
                end = Some((poly, p3));
            }
            inn.pts.push(pts);
            inn.hints.push(hints);
            inn.uvs.push(uvs);
            inn.shade.push(shade);
            let top = [Vec2::new(lines[i + 1], T), Vec2::new(lines[i + 1], 0.0)];
            let (pts, hints, uvs) = place_profile(&top, &mut eval, up);
            edge.pts.push(pts);
            edge.hints.push(hints);
            edge.uvs.push(uvs);
        }
        outside.merge(&out.mesh(false));
        inside.merge(&inn.mesh(false));
        inside.merge(&edge.mesh(false));
        // The plank's end grain at the transom.
        if let Some((mut poly, mut p3)) = end {
            let area: f32 = (0..poly.len())
                .map(|j| poly[j].perp_dot(poly[(j + 1) % poly.len()]))
                .sum();
            if area < 0.0 {
                poly.reverse();
                p3.reverse();
            }
            let mut cap = Mesh::default();
            for p in &p3 {
                cap.vertices
                    .push(Vertex::new(*p, Vec3::Z, Vec2::new(p.x, p.y)));
            }
            for t in triangulate(&poly) {
                facing(&mut cap, t[0] as u32, t[1] as u32, t[2] as u32);
            }
            outside.merge(&cap);
        }
    }
    add_painted(
        mb,
        join([outside.clone(), mirror_x(outside)]),
        Mat::HullPaint,
    );
    mb.add(Mat::InteriorPaint, join([inside.clone(), mirror_x(inside)]));
}

/// The outer keel and stem (one timber with a narrow, rounded cutwater), the apron and hog
/// inside, and a bronze stem band.
fn keel_and_stem(mb: &mut ModelBuilder, hull: &Hull) {
    let n = hull.outer.len();
    let path: Vec<Vec3> = hull
        .outer
        .iter()
        .map(|p| Vec3::new(0.0, p.y, p.x))
        .collect();
    let inward: Vec<Vec3> = (0..n)
        .map(|i| {
            let t = (hull.outer[(i + 1).min(n - 1)] - hull.outer[i.saturating_sub(1)]).normalize();
            Vec3::new(0.0, t.x, -t.y)
        })
        .collect();
    let ff = hull.forefoot as f32;
    let keel = sweep(
        &path,
        &inward,
        |i| {
            let k = smoothstep(ff - 8.0, ff, i as f32);
            let depth = hull.outer[i].distance(hull.rabbet[i]) + 0.012;
            let hw = 0.026 + (0.023 - 0.026) * k;
            let fw = 0.009 + (0.017 - 0.009) * k;
            (0..18)
                .map(|j| {
                    let th = j as f32 / 18.0 * TAU;
                    let (s, c) = th.sin_cos();
                    let a = hw * c.signum() * c.abs().powf(0.4);
                    let b = depth * 0.5 * (1.0 + s.signum() * s.abs().powf(0.4));
                    Vec2::new(
                        a * (fw / hw + (1.0 - fw / hw) * smoothstep(0.0, depth * 0.5, b)),
                        b,
                    )
                })
                .collect()
        },
        true,
    );
    add_painted(mb, keel, Mat::HullPaint);
    let rabbet: Vec<Vec3> = hull
        .rabbet
        .iter()
        .map(|p| Vec3::new(0.0, p.y, p.x))
        .collect();
    mb.add(
        Mat::InteriorPaint,
        sweep(
            &rabbet,
            &inward,
            |_| rrect(-0.028, 0.028, -0.01, 0.032, 0.006, 2),
            true,
        ),
    );
    // Bronze band up the cutwater, round the forefoot.
    let end = (hull.forefoot + 10).min(n - 1);
    mb.add(
        Mat::Bronze,
        sweep(
            &path[1..end],
            &inward[1..end],
            |_| rrect(-0.005, 0.005, -0.003, 0.002, 0.0025, 2),
            true,
        ),
    );
}

/// The transom: a varnished mahogany board inside the plank ends, its top crowned.
fn transom(mb: &mut ModelBuilder, hull: &Hull) {
    let row = hull.row(1.0);
    let g = row.girth();
    let star: Vec<Vec2> = (0..=40)
        .map(|k| {
            let v = row.v_at(g * k as f32 / 40.0);
            let p = hull.point(1.0, v) + hull.normal(1.0, v) * T;
            Vec2::new(p.x, p.y)
        })
        .collect();
    let corner = star[star.len() - 1];
    let mut poly = star.clone();
    for k in 1..20 {
        let x = corner.x * (1.0 - 2.0 * k as f32 / 20.0);
        poly.push(Vec2::new(
            x,
            corner.y + 0.028 * (1.0 - (x / corner.x).powi(2)),
        ));
    }
    poly.extend(star.iter().rev().map(|p| Vec2::new(-p.x, p.y)));
    let m = place(
        extrude(&ccw(poly), TRANSOM_T, 0.003, 40.0),
        Vec3::new(0.0, 0.0, TRANSOM_Z - TRANSOM_T * 0.5),
        Quat::IDENTITY,
    );
    let (inner, outer) = split(&m, |p| p.z - (TRANSOM_Z - 0.004));
    mb.add(Mat::Varnish, inner);
    add_painted(mb, outer, Mat::Varnish);
}

/// Steamed oak frames from keel to gunwale, every 19 cm.
fn frames(mb: &mut ModelBuilder, hull: &Hull) {
    let mut m = Mesh::default();
    let count = 20;
    for k in 0..count {
        let z = -1.80 + (1.88 + 1.80) * k as f32 / (count - 1) as f32;
        let u = hull.u_at_sheer_z(z);
        let row = hull.row(u);
        let g = row.girth();
        let n = 18;
        let (mut path, mut ups) = (Vec::new(), Vec::new());
        for j in 0..=n {
            let s = 0.035 + (g - 0.03 - 0.035) * j as f32 / n as f32;
            let v = row.v_at(s);
            let nn = hull.normal(u, v);
            path.push(hull.point(u, v) - nn * (RIB_D * 0.5));
            ups.push(nn);
        }
        m.merge(&sweep(
            &path,
            &ups,
            |_| {
                rrect(
                    -RIB_W * 0.5,
                    RIB_W * 0.5,
                    -RIB_D * 0.5,
                    RIB_D * 0.5,
                    0.005,
                    1,
                )
            },
            true,
        ));
    }
    mb.add(Mat::VarnishLight, join([m.clone(), mirror_x(m)]));
}

/// Keelson, mast step and slatted floorboards.
fn bottom(mb: &mut ModelBuilder, hull: &Hull) {
    let kson = |z: f32| hull.keel_rabbet_y(z) + RIB_D + 0.015;
    let path: Vec<Vec3> = (0..=48)
        .map(|k| {
            let z = -1.62 + (1.98 + 1.62) * k as f32 / 48.0;
            Vec3::new(0.0, kson(z), z)
        })
        .collect();
    mb.add(
        Mat::VarnishLight,
        sweep(
            &path,
            &vec![Vec3::Y; path.len()],
            |_| rrect(-0.036, 0.036, -0.015, 0.015, 0.005, 2),
            true,
        ),
    );
    // Mast step: an oak block on the keelson with the heel's mortise.
    let (top, base) = (-0.025, kson(MAST_Z) + 0.01);
    mb.add(
        Mat::VarnishLight,
        place(
            rounded_box(
                Vec3::new(0.11, top - base, 0.32),
                0.008,
                2,
                0.1,
                BoxUv::PLANAR,
            ),
            Vec3::new(0.0, (top + base) * 0.5, MAST_Z),
            Quat::IDENTITY,
        ),
    );
    // Floorboards: slats lying on the frames, each following the bottom under it.
    let slats = [
        (0.05f32, 0.145f32),
        (0.16, 0.255),
        (0.27, 0.365),
        (0.38, 0.475),
    ];
    let mut fb = Mesh::default();
    for (k, &(x0, x1)) in slats.iter().enumerate() {
        let low = |z: f32| hull.bottom_y(z, x1) < -0.03 && z > -1.03 && z < 1.76;
        let zs: Vec<f32> = (0..=110).map(|j| -1.1 + 2.9 * j as f32 / 110.0).collect();
        let Some(za) = zs.iter().copied().find(|&z| low(z)) else {
            continue;
        };
        let Some(zb) = zs.iter().rev().copied().find(|&z| low(z)) else {
            continue;
        };
        let n = ((zb - za) / 0.08).ceil().max(2.0) as usize;
        let rings: Vec<(Vec3, Vec<Vec2>)> = (0..=n)
            .map(|j| {
                let z = za + (zb - za) * j as f32 / n as f32;
                let (y0, y1) = (hull.bottom_y(z, x0) + 0.003, hull.bottom_y(z, x1) + 0.003);
                let c = Vec2::new((x0 + x1) * 0.5, (y0 + y1) * 0.5);
                let ex = Vec2::new(x1 - x0, y1 - y0).normalize();
                let ey = Vec2::new(-ex.y, ex.x);
                let (bl, br) = (Vec2::new(x0, y0) - c, Vec2::new(x1, y1) - c);
                let (tl, tr) = (bl + ey * 0.016, br + ey * 0.016);
                let ch = 0.003;
                let prof = vec![
                    bl + ex * ch,
                    br - ex * ch,
                    br + ey * ch,
                    tr - ey * ch,
                    tr - ex * ch,
                    tl + ex * ch,
                    tl - ey * ch,
                    bl + ey * ch,
                ];
                (Vec3::new(c.x, c.y, z), prof)
            })
            .collect();
        let path: Vec<Vec3> = rings.iter().map(|r| r.0).collect();
        let mut slat = sweep(
            &path,
            &vec![Vec3::Y; path.len()],
            |i| rings[i].1.clone(),
            true,
        );
        // The weathered-pine atlas: long grain along the slat.
        let (off, flip) = (
            Vec2::new(rnd(700 + k as u32, 0) * 0.2, rnd(700 + k as u32, 1) * 2.0),
            rnd(700 + k as u32, 2) > 0.5,
        );
        for v in &mut slat.vertices {
            let p = Vec3::from(v.position);
            v.uv = wood_uv((p.x - x0).clamp(0.0, x1 - x0), p.z, x1 - x0, off, flip).to_array();
        }
        fb.merge(&slat);
    }
    mb.add(Mat::Pine, join([fb.clone(), mirror_x(fb)]));
}

/// Gunwale cap, rubrail and inwale along the sheer, breasthook and quarter knees.
fn gunwales(mb: &mut ModelBuilder, hull: &Hull) {
    let sweep_sheer = |z0: f32, z1: f32, n: usize, at: &dyn Fn(f32) -> (Vec3, Vec3)| {
        let (u0, u1) = (hull.u_at_sheer_z(z0), hull.u_at_sheer_z(z1));
        let mut path = Vec::new();
        let mut ups = Vec::new();
        for k in 0..=n {
            let (p, up) = at(u0 + (u1 - u0) * k as f32 / n as f32);
            path.push(p);
            ups.push(up);
        }
        (path, ups)
    };
    let mut trim = Mesh::default();
    // Cap: sits on the sheer strake and frame heads, overhanging outboard a little.
    let (path, ups) = sweep_sheer(-1.78, TRANSOM_Z, 72, &|u| {
        (hull.point(u, 1.0) + hull.normal(u, 1.0) * T, Vec3::Y)
    });
    trim.merge(&sweep(
        &path,
        &ups,
        |_| rrect(-0.058, 0.007, 0.0, CAP_T, 0.006, 2),
        true,
    ));
    // Rubrail: half-round, just under the cap.
    let rub: Vec<Vec2> = (0..12)
        .map(|k| {
            let th = k as f32 / 12.0 * TAU;
            Vec2::new(0.0135 * th.cos(), (0.0075 + 0.0095 * th.sin()).max(0.0))
        })
        .collect();
    let (path, ups) = sweep_sheer(-2.07, TRANSOM_Z, 80, &|u| {
        let row = hull.row(u);
        let v = row.v_at(row.girth() - 0.026);
        let n = hull.normal(u, v);
        (hull.point(u, v) + n * T, n)
    });
    trim.merge(&sweep(&path, &ups, |_| rub.clone(), true));
    // Inwale: inside the frame heads.
    let (path, ups) = sweep_sheer(-1.85, 2.02, 64, &|u| {
        let row = hull.row(u);
        let v = row.v_at(row.girth() - 0.022);
        let n = hull.normal(u, v);
        (hull.point(u, v) - n * (RIB_D + 0.0105), -n)
    });
    trim.merge(&sweep(
        &path,
        &ups,
        |_| rrect(-0.016, 0.016, -0.0105, 0.0105, 0.003, 2),
        true,
    ));
    mb.add(Mat::Varnish, join([trim.clone(), mirror_x(trim)]));

    // Breasthook: joins the gunwales and stem, flush with the cap, rising with the sheer.
    let z_aft = -1.80f32;
    let z_tip = hull.z_h + 0.014;
    let edge = |z: f32| (hull.sheer_half_breadth(z) + 0.006).max(0.012);
    let u_aft = hull.u_at_sheer_z(z_aft);
    let top_aft = hull.point(u_aft, 1.0).y + hull.normal(u_aft, 1.0).y * T + CAP_T + 0.001;
    let rise = (hull.sheer_y(z_tip) - hull.sheer_y(z_aft)) / (z_aft - z_tip);
    let mut poly = Vec::new();
    for k in 0..=12 {
        let z = z_aft + (z_tip - z_aft) * k as f32 / 12.0;
        poly.push(Vec2::new(edge(z), z - z_aft));
    }
    for k in (0..=12).rev() {
        let z = z_aft + (z_tip - z_aft) * k as f32 / 12.0;
        poly.push(Vec2::new(-edge(z), z - z_aft));
    }
    let xe = edge(z_aft);
    for k in 1..14 {
        let x = -xe + 2.0 * xe * k as f32 / 14.0;
        poly.push(Vec2::new(x, -0.09 * (1.0 - (x / xe).powi(2))));
    }
    let hook = slab(poly, 0.0, 0.04, 0.004);
    let tilt = Mat4::from_rotation_translation(
        Quat::from_rotation_x(rise.atan()),
        Vec3::new(0.0, top_aft, z_aft),
    );
    mb.add(Mat::Varnish, xf(&hook, tilt));
    let cleat = place(
        horn_cleat(0.1),
        Vec3::new(0.0, 0.0, -0.14),
        Quat::from_rotation_y(FRAC_PI_2),
    );
    mb.add(Mat::Bronze, xf(&cleat, tilt));

    // Quarter knees under the cap at the transom corners.
    let inner_x = |z: f32| hull.sheer_half_breadth(z) - T - RIB_D - 0.025;
    let (za, zc) = (1.80f32, TRANSOM_Z - TRANSOM_T - 0.002);
    let mut poly: Vec<Vec2> = (0..=8)
        .map(|k| {
            let z = za + (zc - za) * k as f32 / 8.0;
            Vec2::new(inner_x(z), z)
        })
        .collect();
    let corner = Vec2::new(inner_x(zc), zc);
    let a = Vec2::new(corner.x - 0.21, zc);
    let b = Vec2::new(inner_x(za), za);
    poly.push(a);
    for k in 1..10 {
        let t = k as f32 / 10.0;
        let p = a.lerp(b, t);
        poly.push(p + (corner - a.lerp(b, 0.5)) * 0.4 * (PI * t).sin());
    }
    let zm = (za + zc) * 0.5;
    let um = hull.u_at_sheer_z(zm);
    let top = hull.point(um, 1.0).y + hull.normal(um, 1.0).y * T - 0.0015;
    let slope = (hull.sheer_y(zc) - hull.sheer_y(za)) / (zc - za);
    let poly: Vec<Vec2> = poly.iter().map(|p| Vec2::new(p.x, p.y - zm)).collect();
    let knee = xf(
        &slab(poly, 0.0, 0.032, 0.004),
        Mat4::from_rotation_translation(
            Quat::from_rotation_x(-slope.atan()),
            Vec3::new(0.0, top, zm),
        ),
    );
    mb.add(Mat::Varnish, join([knee.clone(), mirror_x(knee)]));
}

/// Mid thwart (round the mast, with its partner and hanging knees), aft thwart, stern-sheet
/// benches, the risers they sit on, and the halyard cleats.
fn thwarts(mb: &mut ModelBuilder, hull: &Hull) {
    let seat_bottom = THWART_Y - THWART_T;
    let ends = |z: f32| hull.side(z, seat_bottom, RIB_D + RISER_T + 0.003).0.x;
    let mut wood = Mesh::default();
    // Mid thwart: a round lobe forward carries it round the mast, filleted into its edge.
    let (zf, za) = (MID_THWART_Z - 0.12, MID_THWART_Z + 0.12);
    // The front edge from x = -half to +half, with a rounded-square lobe `a` wide (half) and
    // `b` deep round the mast; returns the points and the fillet radius at each.
    let front = |half: f32, a: f32, b: f32, fillet: f32| {
        let mut pts = vec![(Vec2::new(-half, zf), 0.0)];
        let e = 2.0 / 2.6;
        for k in 0..=32 {
            let phi = PI * (1.0 - k as f32 / 32.0);
            let (s, c) = phi.sin_cos();
            let f = if k == 0 || k == 32 { fillet } else { 0.0 };
            pts.push((
                Vec2::new(a * c.signum() * c.abs().powf(e), zf - b * s.abs().powf(e)),
                f,
            ));
        }
        pts.push((Vec2::new(half, zf), 0.0));
        pts
    };
    let mut outline = Vec::new();
    for k in 0..=4 {
        let z = zf + (za - zf) * k as f32 / 4.0;
        outline.push((Vec2::new(ends(z), z), 0.0));
    }
    for k in (0..=4).rev() {
        let z = zf + (za - zf) * k as f32 / 4.0;
        outline.push((Vec2::new(-ends(z), z), 0.0));
    }
    let xe = ends(zf);
    let lobe = front(xe, 0.15, 0.25, 0.03);
    outline.extend_from_slice(&lobe[1..lobe.len() - 1]);
    let rounded = |o: &[(Vec2, f32)]| {
        let pts: Vec<Vec2> = o.iter().map(|p| p.0).collect();
        let pts = ccw(pts);
        let flip = pts[0] != o[0].0;
        let n = o.len();
        round_corners(&pts, |i| o[if flip { n - 1 - i } else { i }].1, 4)
    };
    wood.merge(&slab(rounded(&outline), THWART_Y, THWART_T, 0.004));
    // Mast partner: a doubler under the lobe, spreading the mast's load into the thwart.
    let mut partner = front(0.2, 0.132, 0.232, 0.025);
    partner.extend_from_slice(&[
        (Vec2::new(0.2, zf + 0.06), 0.0),
        (Vec2::new(-0.2, zf + 0.06), 0.0),
    ]);
    wood.merge(&slab(rounded(&partner), seat_bottom, 0.03, 0.004));
    // Hanging knees under the mid thwart's ends.
    let mut knees = Vec::new();
    let ys = [0.05f32, 0.09, 0.13, 0.17, seat_bottom];
    for &y in &ys {
        knees.push(Vec2::new(
            hull.side(MID_THWART_Z, y, RIB_D + RISER_T + 0.002).0.x,
            y,
        ));
    }
    let tip = knees[knees.len() - 1];
    let (a, b) = (Vec2::new(tip.x - 0.17, seat_bottom), knees[0]);
    let corner = Vec2::new(tip.x, seat_bottom);
    knees.push(a);
    for k in 1..10 {
        let t = k as f32 / 10.0;
        knees.push(a.lerp(b, t) + (corner - a.lerp(b, 0.5)) * 0.45 * (PI * t).sin());
    }
    let knee = place(
        extrude(&ccw(knees), 0.028, 0.004, 35.0),
        Vec3::new(0.0, 0.0, MID_THWART_Z),
        Quat::IDENTITY,
    );
    wood.merge(&knee);
    wood.merge(&mirror_x(knee));
    // Aft thwart.
    let (zf, za) = (AFT_THWART_Z - 0.11, AFT_THWART_Z + 0.11);
    let mut poly = Vec::new();
    for k in 0..=4 {
        let z = zf + (za - zf) * k as f32 / 4.0;
        poly.push(Vec2::new(ends(z), z));
    }
    for k in (0..=4).rev() {
        let z = zf + (za - zf) * k as f32 / 4.0;
        poly.push(Vec2::new(-ends(z), z));
    }
    wood.merge(&slab(poly, THWART_Y, THWART_T, 0.004));
    // Stern sheets: a bench down each side and a seat across the transom.
    let (zb0, zb1) = (za, 1.665f32);
    let mut bench = Vec::new();
    for k in 0..=10 {
        let z = zb0 + (zb1 - zb0) * k as f32 / 10.0;
        bench.push(Vec2::new(ends(z), z));
    }
    for k in (0..=10).rev() {
        let z = zb0 + (zb1 - zb0) * k as f32 / 10.0;
        bench.push(Vec2::new(ends(z) - 0.2, z));
    }
    let side_bench = swap_uv(slab(bench, THWART_Y, THWART_T, 0.004));
    wood.merge(&side_bench);
    wood.merge(&mirror_x(side_bench));
    for (z0, z1) in [(1.67f32, 1.835f32), (1.85, 2.02)] {
        let mut poly = Vec::new();
        for k in 0..=3 {
            let z = z0 + (z1 - z0) * k as f32 / 3.0;
            poly.push(Vec2::new(ends(z), z));
        }
        for k in (0..=3).rev() {
            let z = z0 + (z1 - z0) * k as f32 / 3.0;
            poly.push(Vec2::new(-ends(z), z));
        }
        wood.merge(&slab(poly, THWART_Y, THWART_T, 0.004));
    }
    // Legs under the benches' inner edges.
    for z in [0.95f32, 1.45] {
        let x = ends(z) - 0.17;
        let y0 = hull.bottom_y(z, x);
        let leg = place(
            rounded_box(
                Vec3::new(0.035, seat_bottom - y0, 0.035),
                0.005,
                2,
                0.1,
                BoxUv::PLANAR,
            ),
            Vec3::new(x, (seat_bottom + y0) * 0.5, z),
            Quat::IDENTITY,
        );
        wood.merge(&leg);
        wood.merge(&mirror_x(leg));
    }
    mb.add(Mat::Varnish, wood);
    // Risers along the frames, carrying the thwarts and benches.
    let n = 40;
    let (mut path, mut ups) = (Vec::new(), Vec::new());
    for k in 0..=n {
        let z = -1.45 + (1.98 + 1.45) * k as f32 / n as f32;
        let (p, nn) = hull.side(z, seat_bottom - 0.03, RIB_D + RISER_T * 0.5);
        path.push(p);
        ups.push(nn);
    }
    let riser = sweep(
        &path,
        &ups,
        |_| rrect(-0.03, 0.03, -RISER_T * 0.5, RISER_T * 0.5, 0.003, 2),
        true,
    );
    mb.add(Mat::VarnishLight, join([riser.clone(), mirror_x(riser)]));
    // Mast collar where the mast passes the thwart, and the halyard cleats beside it.
    mb.add(
        Mat::Bronze,
        super::props::turned(
            &[
                (0.0425, THWART_Y),
                (0.056, THWART_Y),
                (0.058, THWART_Y + 0.004),
                (0.052, THWART_Y + 0.016),
                (0.0425, THWART_Y + 0.016),
            ],
            28,
            Vec3::new(0.0, 0.0, MAST_Z),
        ),
    );
    for x in [-0.06f32, 0.06] {
        mb.add(
            Mat::Bronze,
            place(
                horn_cleat(0.09),
                Vec3::new(x, THWART_Y, CLEAT_Z),
                Quat::IDENTITY,
            ),
        );
    }
}

/// The centerboard trunk (painted sides, varnished headledges and cap) and the board, down.
fn trunk(mb: &mut ModelBuilder, hull: &Hull) {
    let (z0, z1) = (TRUNK_Z[0], TRUNK_Z[1]);
    let base = |z: f32| hull.keel_rabbet_y(z) + RIB_D + 0.026;
    let top = 0.20f32;
    let mut poly: Vec<Vec2> = (0..=8)
        .map(|k| {
            let z = z0 + (z1 - z0) * k as f32 / 8.0;
            Vec2::new(z, base(z))
        })
        .collect();
    poly.extend_from_slice(&[Vec2::new(z1, top), Vec2::new(z0, top)]);
    for x in [0.0215f32, -0.0215] {
        mb.add(Mat::InteriorPaint, side_slab(poly.clone(), x, 0.016, 0.002));
    }
    for z in [z0 + 0.02, z1 - 0.02] {
        let b = base(z) - 0.004;
        mb.add(
            Mat::Varnish,
            place(
                rounded_box(
                    Vec3::new(0.064, top + 0.006 - b, 0.04),
                    0.004,
                    2,
                    0.2,
                    BoxUv::PLANAR,
                ),
                Vec3::new(0.0, (top + 0.006 + b) * 0.5, z),
                Quat::IDENTITY,
            ),
        );
    }
    mb.add(
        Mat::Varnish,
        place(
            swap_uv(rounded_box(
                Vec3::new(0.082, 0.02, z1 - z0 + 0.02),
                0.006,
                2,
                0.3,
                BoxUv::PLANAR,
            )),
            Vec3::new(0.0, top + 0.016, (z0 + z1) * 0.5),
            Quat::IDENTITY,
        ),
    );
    let pivot = Vec3::new(0.0, base(-0.53) + 0.035, -0.53);
    mb.add_small(
        Mat::Bronze,
        bolts(&[pivot + Vec3::X * 0.0295], Vec3::X, 0.011),
    );
    mb.add_small(
        Mat::Bronze,
        bolts(&[pivot - Vec3::X * 0.0295], Vec3::NEG_X, 0.011),
    );
    // The board: a foil, raked a little aft, with a rounded tip.
    let n = 28;
    let st: Vec<(Vec3, Vec3, f32, f32)> = (0..=n)
        .map(|k| {
            let s = 1.0 - (1.0 - k as f32 / n as f32).powf(1.6);
            let y = -0.06 + (BOARD_BOTTOM_Y + 0.06) * s;
            let (le, te) = (-0.565 + 0.09 * s, -0.17 - 0.035 * s);
            let r0 = 0.86;
            let kt = if s > r0 {
                (1.0 - ((s - r0) / (1.0 - r0)).powi(2)).max(0.0).sqrt()
            } else {
                1.0
            };
            let mid = (le + te) * 0.5 + 0.03 * (1.0 - kt);
            let half = (te - le) * 0.5 * kt;
            (
                Vec3::new(0.0, y, mid - half),
                Vec3::new(0.0, y, mid + half),
                0.022 * (0.35 + 0.65 * kt),
                0.0,
            )
        })
        .collect();
    mb.add(Mat::Antifoul, foil(&st, Vec3::X, 14, false));
}

/// The mast: a solid spruce spar with its truck, masthead sheave box and hounds band.
fn mast(mb: &mut ModelBuilder) {
    let at = |y: f32| Vec3::new(0.0, y, MAST_Z);
    let top = MAST_TOP_Y - 0.02;
    let len = top - MAST_FOOT_Y;
    mb.add(
        Mat::VarnishLight,
        spar(
            at(MAST_FOOT_Y),
            at(top),
            |t| mast_radius(MAST_FOOT_Y + len * t),
            64,
            24,
            (0.0, 0.0),
        ),
    );
    mb.add(
        Mat::VarnishLight,
        super::props::turned_wood(
            &[
                (0.0, top - 0.002),
                (0.031, top - 0.002),
                (0.034, top + 0.006),
                (0.034, top + 0.013),
                (0.029, MAST_TOP_Y - 0.002),
                (0.0, MAST_TOP_Y),
            ],
            24,
            at(0.0),
            811,
        ),
    );
    // Masthead sheave box: bronze cheeks, two sheaves through a mortise, the pin.
    let ys = MASTHEAD_SHEAVE_Y;
    let rs = mast_radius(ys);
    for x in [-1.0f32, 1.0] {
        mb.add(
            Mat::Bronze,
            place(
                rounded_box(Vec3::new(0.004, 0.12, 0.05), 0.0015, 1, 0.2, BoxUv::PLANAR),
                Vec3::new(x * (rs + 0.002), ys, MAST_Z),
                Quat::IDENTITY,
            ),
        );
        mb.add_small(
            Mat::Bronze,
            bolts(
                &[Vec3::new(x * (rs + 0.004), ys, MAST_Z)],
                Vec3::X * x,
                0.008,
            ),
        );
        mb.add_small(
            Mat::Bronze,
            bolts(
                &[
                    Vec3::new(x * (rs + 0.004), ys + 0.05, MAST_Z),
                    Vec3::new(x * (rs + 0.004), ys - 0.05, MAST_Z),
                ],
                Vec3::X * x,
                0.005,
            ),
        );
    }
    let sheave = super::props::turned(
        &[
            (0.0, -0.0042),
            (0.033, -0.0042),
            (0.0345, -0.0025),
            (0.031, 0.0),
            (0.0345, 0.0025),
            (0.033, 0.0042),
            (0.0, 0.0042),
        ],
        28,
        Vec3::ZERO,
    );
    for x in [-0.0055f32, 0.0055] {
        mb.add(
            Mat::Bronze,
            place(
                sheave.clone(),
                Vec3::new(x, ys, MAST_Z),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
    }
    for z in [-1.0f32, 1.0] {
        mb.add(
            Mat::BlackSteel,
            place(
                rounded_box(Vec3::new(0.024, 0.085, 0.006), 0.002, 1, 0.2, BoxUv::PLANAR),
                Vec3::new(0.0, ys, MAST_Z + z * (rs - 0.002)),
                Quat::IDENTITY,
            ),
        );
    }
    // Hounds band carrying the shroud and forestay tangs.
    let rh = mast_radius(SHROUD_MAST_Y + 0.05);
    mb.add(
        Mat::Bronze,
        super::props::turned(
            &[
                (rh - 0.001, SHROUD_MAST_Y - 0.03),
                (rh + 0.002, SHROUD_MAST_Y - 0.03),
                (rh + 0.003, SHROUD_MAST_Y - 0.027),
                (rh + 0.003, FORESTAY_MAST_Y + 0.027),
                (rh + 0.002, FORESTAY_MAST_Y + 0.03),
                (rh - 0.001, FORESTAY_MAST_Y + 0.03),
            ],
            28,
            at(0.0),
        ),
    );
}

/// Shrouds and forestay with their tangs, rigging screws and toggles, the chainplates, the
/// stemhead fitting, and the halyard falls down to their cleats.
fn rigging(mb: &mut ModelBuilder, hull: &Hull) {
    let wire_r = 0.0028;
    let mut wire = Mesh::default();
    let mut bronze = Mesh::default();
    let mut small = Mesh::default();
    let tang = |p0: Vec3, dir: Vec3, bronze: &mut Mesh| {
        let plate_n = dir.cross(Vec3::Y).normalize_or(Vec3::Z);
        bronze.merge(&place(
            rounded_box(
                Vec3::new(0.004, 0.075, 0.024),
                0.0015,
                1,
                0.2,
                BoxUv::PLANAR,
            ),
            p0 + dir * 0.03,
            super::props::frame_rot(dir, plate_n),
        ));
        p0 + dir * 0.06
    };
    // Wire from a tang to a pin, with a rigging screw and toggle at the pin.
    let stay = |p0: Vec3, pin: Vec3, bronze: &mut Mesh, wire: &mut Mesh| {
        let dir = (pin - p0).normalize();
        let top = tang(p0, dir, bronze);
        let up = -dir;
        bronze.merge(&place(
            rounded_box(Vec3::new(0.012, 0.04, 0.008), 0.003, 1, 0.2, BoxUv::PLANAR),
            pin + up * 0.02,
            super::props::frame_rot(up, Vec3::X),
        ));
        bronze.merge(&turnbuckle(pin + up * 0.04, up, 0.14));
        wire.merge(&tube(
            &[top, pin + up * 0.175],
            |_| wire_r,
            8,
            false,
            true,
            1.0,
        ));
    };
    let rm = mast_radius(SHROUD_MAST_Y);
    for side in [1.0f32, -1.0] {
        let pin = Vec3::new(CHAINPLATE[0] * side, CHAINPLATE[1], CHAINPLATE[2]);
        let p0 = Vec3::new(side * (rm + 0.005), SHROUD_MAST_Y + 0.025, MAST_Z);
        stay(p0, pin, &mut bronze, &mut wire);
    }
    let rf = mast_radius(FORESTAY_MAST_Y);
    let stemhead = Vec3::from(STEMHEAD);
    stay(
        Vec3::new(0.0, FORESTAY_MAST_Y + 0.02, MAST_Z - rf - 0.005),
        stemhead,
        &mut bronze,
        &mut wire,
    );
    // Chainplates: straps bolted down the topsides, an eye bent out under the rubrail.
    for side in [1.0f32, -1.0] {
        let z = CHAINPLATE[2];
        let (mut path, mut ups) = (Vec::new(), Vec::new());
        for k in 0..=6 {
            let y = 0.22 + (0.385 - 0.22) * k as f32 / 6.0;
            let (p, n) = hull.side(z, y, -(1.6 * T + 0.003));
            path.push(Vec3::new(p.x * side, p.y, p.z));
            ups.push(Vec3::new(n.x * side, n.y, n.z));
        }
        let strap = sweep(
            &path,
            &ups,
            |_| rrect(-0.014, 0.014, -0.003, 0.003, 0.002, 1),
            true,
        );
        bronze.merge(&strap);
        let pin = Vec3::new(CHAINPLATE[0] * side, CHAINPLATE[1], CHAINPLATE[2]);
        let top = path[path.len() - 1];
        let n_top = ups[ups.len() - 1];
        let knuckle = smooth_path(&[top, top + n_top * 0.022 + Vec3::Y * 0.012, pin], 4);
        bronze.merge(&tube(&knuckle, |_| 0.0055, 10, false, true, 1.0));
        bronze.merge(&ring_at(pin, Vec3::Z, 0.008, 0.0035));
        let heads: Vec<Vec3> = [0.24f32, 0.29, 0.34]
            .iter()
            .map(|&y| {
                let (p, n) = hull.side(z, y, -(1.6 * T + 0.006));
                Vec3::new(p.x * side, p.y, p.z) + Vec3::new(n.x * side, n.y, n.z) * 0.0
            })
            .collect();
        small.merge(&bolts(&heads, Vec3::new(side, 0.0, 0.0), 0.006));
    }
    // Stemhead fitting: cheeks either side of the stem head, a cap, the pin.
    for x in [-1.0f32, 1.0] {
        bronze.merge(&place(
            rounded_box(Vec3::new(0.004, 0.07, 0.06), 0.0015, 1, 0.2, BoxUv::PLANAR),
            Vec3::new(x * 0.0285, 0.61, -2.112),
            Quat::IDENTITY,
        ));
    }
    small.merge(&bolts(&[stemhead + Vec3::X * 0.0305], Vec3::X, 0.006));
    small.merge(&bolts(&[stemhead - Vec3::X * 0.0305], Vec3::NEG_X, 0.006));
    bronze.merge(&place(
        rounded_box(Vec3::new(0.06, 0.006, 0.085), 0.002, 1, 0.2, BoxUv::PLANAR),
        Vec3::new(0.0, STEM_TOP_Y + 0.003, -2.108),
        Quat::IDENTITY,
    ));
    mb.add(Mat::Galvanized, wire);
    mb.add(Mat::Bronze, bronze);
    mb.add_small(Mat::Bronze, small);
    // Halyard falls from the sheaves' fore side down to the cleats by the mast, with a
    // couple of turns made fast.
    let mut ropes = Mesh::default();
    for x in [-1.0f32, 1.0] {
        let start = Vec3::new(x * 0.0055, MASTHEAD_SHEAVE_Y, MAST_Z - 0.0345);
        let end = Vec3::new(x * 0.03, THWART_Y + 0.024, CLEAT_Z);
        ropes.merge(&rope(&[start, end], 0.004));
        for (k, tilt) in [0.35f32, -0.35].iter().enumerate() {
            ropes.merge(&place(
                torus(0.03, 0.004, 18, 6),
                Vec3::new(x * 0.06, THWART_Y + 0.021 + k as f32 * 0.006, CLEAT_Z),
                Quat::from_rotation_y(*tilt) * Quat::from_rotation_x(0.1),
            ));
        }
    }
    mb.add(Mat::Rope, ropes);
}

/// A ring of rectangular, softened section (a band, a hoop) around +Y, centered on y = 0.
fn band(r_in: f32, r_out: f32, h: f32, segments: u32) -> Mesh {
    let mut p = rrect(
        r_in,
        r_out,
        -0.5 * h,
        0.5 * h,
        ((r_out - r_in) * 0.4).min(0.003),
        1,
    );
    p.push(p[0]);
    lathe(&p, segments, LatheUv::meters(r_out))
}

/// Rudder gudgeons on the transom, the oarlocks on their pads, and the bow eye.
fn fittings(mb: &mut ModelBuilder, hull: &Hull) {
    let mut bronze = Mesh::default();
    let mut small = Mesh::default();
    // Gudgeons: an eye on the hinge axis, a neck back to a strap across the transom.
    for &yg in &GUDGEON_Y {
        bronze.merge(&super::props::turned(
            &[
                (0.0065, yg - 0.009),
                (0.0115, yg - 0.009),
                (0.013, yg - 0.0075),
                (0.013, yg + 0.0075),
                (0.0115, yg + 0.009),
                (0.0065, yg + 0.009),
                (0.0065, yg - 0.009),
            ],
            20,
            Vec3::new(0.0, 0.0, RUDDER_Z),
        ));
        let (z0, z1) = (TRANSOM_Z + 0.003, RUDDER_Z - 0.008);
        bronze.merge(&place(
            rounded_box(
                Vec3::new(0.016, 0.014, z1 - z0),
                0.003,
                1,
                0.2,
                BoxUv::PLANAR,
            ),
            Vec3::new(0.0, yg, 0.5 * (z0 + z1)),
            Quat::IDENTITY,
        ));
        bronze.merge(&place(
            rounded_box(Vec3::new(0.10, 0.026, 0.005), 0.002, 1, 0.2, BoxUv::PLANAR),
            Vec3::new(0.0, yg, TRANSOM_Z + 0.0025),
            Quat::IDENTITY,
        ));
        small.merge(&bolts(
            &[
                Vec3::new(0.036, yg, TRANSOM_Z + 0.005),
                Vec3::new(-0.036, yg, TRANSOM_Z + 0.005),
            ],
            Vec3::Z,
            0.0045,
        ));
    }

    // Oarlocks: a varnished pad on the gunwale cap, a bronze socket, the rowlock in it.
    let z = -0.62f32;
    let u = hull.u_at_sheer_z(z);
    let edge = hull.point(u, 1.0) + hull.normal(u, 1.0) * T;
    let ahead = hull.point(hull.u_at_sheer_z(z + 0.05), 1.0);
    let behind = hull.point(hull.u_at_sheer_z(z - 0.05), 1.0);
    let d = ahead - behind;
    let frame = Mat4::from_rotation_translation(
        Quat::from_rotation_y((d.x / d.z).atan()) * Quat::from_rotation_x(-(d.y / d.z).atan()),
        Vec3::new(edge.x - 0.0255, edge.y + CAP_T, z),
    );
    let mut pad = slab(rrect(-0.026, 0.026, -0.1, 0.1, 0.024, 4), 0.02, 0.02, 0.004);
    pad = swap_uv(pad);
    let mut lock = Mesh::default();
    lock.merge(&place(
        rounded_box(
            Vec3::new(0.032, 0.004, 0.064),
            0.0015,
            1,
            0.2,
            BoxUv::PLANAR,
        ),
        Vec3::new(0.0, 0.022, 0.0),
        Quat::IDENTITY,
    ));
    lock.merge(&super::props::turned(
        &[
            (0.0072, 0.024),
            (0.0125, 0.024),
            (0.0125, 0.027),
            (0.0105, 0.036),
            (0.0072, 0.036),
            (0.0072, 0.024),
        ],
        16,
        Vec3::ZERO,
    ));
    lock.merge(&super::props::turned(
        &[
            (0.0, 0.03),
            (0.0062, 0.03),
            (0.0062, 0.036),
            (0.0095, 0.0365),
            (0.0095, 0.041),
            (0.0062, 0.043),
            (0.0062, 0.05),
            (0.0, 0.05),
        ],
        16,
        Vec3::ZERO,
    ));
    let mut horns = Mesh::default();
    let (ru, cy) = (0.03f32, 0.048 + 0.03);
    let mut pts = vec![Vec3::new(0.0, cy + 0.016, -ru - 0.006)];
    for k in 0..=14 {
        let th = -PI + PI * k as f32 / 14.0;
        pts.push(Vec3::new(0.0, cy + ru * th.sin(), ru * th.cos()));
    }
    pts.push(Vec3::new(0.0, cy + 0.016, ru + 0.006));
    let pts = smooth_path(&pts, 3);
    horns.merge(&tube(&pts, |_| 0.0055, 12, false, true, 1.0));
    for end in [pts[0], pts[pts.len() - 1]] {
        horns.merge(&place(sphere(0.0058, 12), end, Quat::IDENTITY));
    }
    lock.merge(&horns);
    let pad = xf(&pad, frame);
    let lock = xf(&lock, frame);
    mb.add(Mat::Varnish, join([pad.clone(), mirror_x(pad)]));
    bronze.merge(&lock);
    bronze.merge(&mirror_x(lock));

    // Bow eye through the stem, just above the waterline.
    let y = 0.16f32;
    let face = Vec3::new(0.0, y, STEM_Z + (STEM_TOP_Y - y) * STEM_RAKE);
    let fwd = Vec3::new(0.0, -STEM_RAKE, -1.0).normalize();
    bronze.merge(&place(
        super::props::turned(
            &[
                (0.0, 0.0),
                (0.012, 0.0),
                (0.012, 0.003),
                (0.009, 0.009),
                (0.0, 0.0105),
            ],
            16,
            Vec3::ZERO,
        ),
        face + fwd * 0.002,
        Quat::from_rotation_arc(Vec3::Y, fwd),
    ));
    bronze.merge(&ring_at(face + fwd * 0.024, Vec3::X, 0.0125, 0.0036));
    mb.add(Mat::Bronze, bronze);
    mb.add_small(Mat::Bronze, small);
}

/// Length of the oars.
const OAR_LEN: f32 = 2.2;
/// Where the oar's blade begins (meters from the handle end).
const OAR_BLADE: f32 = 1.62;

/// Oar loom radius `d` meters from the handle end: a slim grip, the full loom through the
/// rowlock, tapering toward the blade.
fn oar_radius(d: f32) -> f32 {
    if d < 0.17 {
        0.0155 + (0.021 - 0.0155) * smoothstep(0.12, 0.17, d)
    } else {
        0.021 - (0.021 - 0.0165) * smoothstep(0.95, OAR_BLADE, d)
    }
}

/// Oar blade thickness (at its spine) `t` of the way from its root to its tip.
fn oar_blade_thickness(t: f32) -> f32 {
    0.033 - (0.033 - 0.012) * smoothstep(0.0, 0.6, t)
}

/// One oar along +Z from its handle end at the origin, the blade flat (wide along X):
/// (varnished spruce, leather, painted tip).
fn oar_mesh() -> (Mesh, Mesh, Mesh) {
    let at = |d: f32| Vec3::Z * d;
    let shaft_end = OAR_BLADE + 0.04;
    let mut wood = spar(
        at(0.0),
        at(shaft_end),
        |t| oar_radius(t * shaft_end),
        44,
        14,
        (0.5, 0.0),
    );
    // Blade: from round at the root to a thin lens with a spine, rounded at the tip.
    let blade_len = OAR_LEN - OAR_BLADE;
    let mut st: Vec<(f32, f32)> = (0..30).map(|k| (k as f32 / 30.0 * 0.9, 1.0)).collect();
    for k in 0..=8 {
        let phi = FRAC_PI_2 * k as f32 / 8.0;
        st.push((0.9 + 0.1 * phi.sin(), phi.cos().max(0.12)));
    }
    let path: Vec<Vec3> = st
        .iter()
        .map(|&(t, _)| at(OAR_BLADE + t * blade_len))
        .collect();
    let blade = sweep(
        &path,
        &vec![Vec3::Y; path.len()],
        |i| {
            let (t, f) = st[i];
            let w = (0.033 + (0.145 - 0.033) * smoothstep(0.0, 0.5, t)) * f;
            let th = oar_blade_thickness(t) * (0.4 + 0.6 * f);
            let spine = 0.55 * smoothstep(0.0, 0.25, t);
            (0..16)
                .map(|k| {
                    let (s, c) = (k as f32 / 16.0 * TAU).sin_cos();
                    Vec2::new(
                        0.5 * w * c,
                        0.5 * th * s * (1.0 - spine * c.abs().powf(1.6)),
                    )
                })
                .collect()
        },
        true,
    );
    let (blade, tip) = split(&blade, |p| p.z - (OAR_LEN - 0.075));
    wood.merge(&blade);
    // Leather where it works in the rowlock, with a button to stop it slipping out.
    let mut leather = spar(at(0.55), at(0.78), |_| 0.0232, 12, 16, (0.12, 0.12));
    leather.merge(&spar(at(0.545), at(0.565), |_| 0.0275, 2, 16, (0.25, 0.25)));
    (wood, leather, tip)
}

/// A pair of oars stowed fore and aft on the thwarts, blades aft and flat.
fn oars(mb: &mut ModelBuilder) {
    let (wood, leather, tip) = oar_mesh();
    // Resting on the mid thwart by the loom and on the aft thwart by the blade.
    let (z0, d_mid, d_aft) = (-1.30f32, MID_THWART_Z + 1.30, AFT_THWART_Z + 1.30);
    let y_mid = THWART_Y + oar_radius(d_mid) + 0.0005;
    let y_aft =
        THWART_Y + 0.5 * oar_blade_thickness((d_aft - OAR_BLADE) / (OAR_LEN - OAR_BLADE)) + 0.0005;
    let pitch = ((y_mid - y_aft) / (d_aft - d_mid)).atan();
    let y0 = y_mid + d_mid * pitch.sin();
    for (x, yaw) in [(0.40f32, 0.006f32), (-0.42, -0.004)] {
        let t = Mat4::from_rotation_translation(
            Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch),
            Vec3::new(x, y0, z0),
        );
        mb.add(Mat::VarnishLight, xf(&wood, t));
        mb.add(Mat::Antifoul, xf(&leather, t));
        mb.add(Mat::HullPaint, xf(&tip, t));
    }
}

// ---------------------------------------------------------------------------------------
// Rig model

/// Boom radius at `t` (0 gooseneck, 1 end): swelling to its greatest a little aft of the
/// middle, where the mainsheet pulls.
fn boom_radius(t: f32) -> f32 {
    let r_max = BOOM_DIA * 0.5;
    if t < 0.55 {
        0.029 + (r_max - 0.029) * (FRAC_PI_2 * t / 0.55).sin()
    } else {
        r_max - (r_max - 0.025) * ((t - 0.55) / 0.45).powf(1.5)
    }
}

/// Gaff radius at `t` (0 jaws, 1 peak).
fn gaff_radius(t: f32) -> f32 {
    0.027 - 0.0095 * smoothstep(0.15, 1.0, t)
}

/// The rig, in its own frame (origin on the mast axis at y = 0).
fn rig_model() -> ModelBuilder {
    let mut mb = ModelBuilder::new("boat_rig");
    boom(&mut mb);
    gaff(&mut mb);
    hoops(&mut mb);
    mb
}

/// The boom with its gooseneck, mainsheet bail and outhaul fittings.
fn boom(mb: &mut ModelBuilder) {
    let (a, b) = (Vec3::from(GOOSENECK), Vec3::from(BOOM_END));
    let dir = (b - a).normalize();
    let along = Quat::from_rotation_arc(Vec3::Y, dir);
    let at = |t: f32| a + (b - a) * t;
    mb.add(
        Mat::VarnishLight,
        spar(a, b, boom_radius, 56, 20, (0.0, 0.7)),
    );
    let mut bronze = Mesh::default();
    let mut small = Mesh::default();
    // Gooseneck: a band round the mast, ears with a vertical pin, a tongue on the boom's
    // ferrule.
    let y = a.y;
    let rm = mast_radius(y);
    bronze.merge(&place(
        band(rm - 0.0003, rm + 0.0055, 0.04, 32),
        Vec3::Y * y,
        Quat::IDENTITY,
    ));
    for dy in [-0.011f32, 0.011] {
        bronze.merge(&place(
            rounded_box(Vec3::new(0.022, 0.006, 0.026), 0.002, 1, 0.2, BoxUv::PLANAR),
            Vec3::new(0.0, y + dy, 0.054),
            Quat::IDENTITY,
        ));
    }
    bronze.merge(&super::props::turned(
        &[
            (0.0, y - 0.016),
            (0.0055, y - 0.016),
            (0.0055, y - 0.0135),
            (0.0035, y - 0.0135),
            (0.0035, y + 0.0135),
            (0.0055, y + 0.0135),
            (0.0055, y + 0.016),
            (0.0, y + 0.017),
        ],
        12,
        Vec3::new(0.0, 0.0, 0.058),
    ));
    bronze.merge(&place(
        rounded_box(
            Vec3::new(0.014, 0.0145, 0.032),
            0.003,
            1,
            0.2,
            BoxUv::PLANAR,
        ),
        Vec3::new(0.0, y, 0.069),
        Quat::IDENTITY,
    ));
    let r0 = boom_radius(0.0);
    bronze.merge(&place(
        band(r0 - 0.0003, r0 + 0.0025, 0.07, 28),
        a + dir * 0.035,
        along,
    ));
    // Mainsheet bail: a strap under the boom with an eye.
    let t_bail = 0.9;
    let rb = boom_radius(t_bail) + 0.004;
    let p = at(t_bail);
    let side = Vec3::X;
    let down = side.cross(dir).normalize();
    let arc: Vec<Vec3> = (0..=12)
        .map(|k| {
            let th = PI * k as f32 / 12.0;
            p + side * (rb * th.cos()) + down * (rb * th.sin())
        })
        .collect();
    bronze.merge(&tube(&arc, |_| 0.0035, 10, false, true, 1.0));
    for s in [-1.0f32, 1.0] {
        bronze.merge(&place(
            rounded_box(Vec3::new(0.004, 0.012, 0.03), 0.0015, 1, 0.2, BoxUv::PLANAR),
            p + side * (s * (rb - 0.002)) - down * 0.006,
            Quat::IDENTITY,
        ));
        small.merge(&bolts(
            &[p + side * (s * (rb + 0.0)) - down * 0.006],
            side * s,
            0.003,
        ));
    }
    bronze.merge(&ring_at(p + down * (rb + 0.011), dir, 0.009, 0.0028));
    // Outhaul: an end band with an eye on top.
    let r_end = boom_radius(0.982);
    bronze.merge(&place(
        band(r_end - 0.0003, r_end + 0.0028, 0.022, 28),
        at(0.982),
        along,
    ));
    bronze.merge(&ring_at(
        at(0.986) - down * (r_end + 0.0028 + 0.0075),
        side,
        0.0075,
        0.0025,
    ));
    mb.add(Mat::Bronze, bronze);
    mb.add_small(Mat::Bronze, small);
}

/// The gaff: spar, jaws (cheeks, block and parrel with its beads), bands for the peak
/// halyard's span, and the halyards up to the masthead.
fn gaff(mb: &mut ModelBuilder) {
    let (j, p) = (Vec3::from(GAFF_JAWS), Vec3::from(GAFF_PEAK));
    let len = j.distance(p);
    let a = (p - j) / len;
    // Square to the gaff, upward.
    let nrm = a.cross(Vec3::X);
    let rot = Quat::from_mat3(&Mat3::from_cols(Vec3::X, nrm, a));
    let along = Quat::from_rotation_arc(Vec3::Y, a);
    let gp = |t: f32, q: f32| j + a * t + nrm * q;
    let s0 = 0.15f32;
    mb.add(
        Mat::VarnishLight,
        spar(
            gp(s0, 0.0),
            p,
            |t| gaff_radius((s0 + t * (len - s0)) / len),
            60,
            20,
            (0.0, 0.6),
        ),
    );
    // Jaws: a block between two shaped cheeks that hug the mast.
    let mut jaws = Mesh::default();
    jaws.merge(&place(
        rounded_box(Vec3::new(0.084, 0.054, 0.2), 0.005, 2, 0.2, BoxUv::PLANAR),
        gp(0.1, 0.0),
        rot,
    ));
    let g2 = |t: f32, q: f32| {
        let v = gp(t, q);
        Vec2::new(v.z, v.y)
    };
    let poly = vec![
        g2(0.25, 0.01),
        g2(0.16, 0.03),
        g2(0.06, 0.034),
        g2(0.0, 0.034),
        Vec2::new(0.0, j.y + 0.034),
        Vec2::new(-0.028, j.y + 0.03),
        Vec2::new(-0.044, j.y + 0.016),
        Vec2::new(-0.046, j.y - 0.002),
        Vec2::new(-0.034, j.y - 0.016),
        Vec2::new(0.0, j.y - 0.022),
        Vec2::new(0.03, j.y - 0.022),
        g2(0.04, -0.03),
        g2(0.13, -0.029),
        g2(0.25, -0.01),
    ];
    let poly = round_corners(&ccw(poly), |_| 0.008, 3);
    let mut small = Mesh::default();
    for x in [-0.049f32, 0.049] {
        jaws.merge(&side_slab(poly.clone(), x, 0.013, 0.002));
        let heads: Vec<Vec3> = [0.12f32, 0.2]
            .iter()
            .map(|&t| gp(t, 0.0) + Vec3::X * (x + x.signum() * 0.0065))
            .collect();
        small.merge(&bolts(&heads, Vec3::X * x.signum(), 0.005));
    }
    mb.add(Mat::Varnish, jaws);
    // Parrel: a line round the front of the mast from cheek to cheek, threaded with beads.
    let (yp, rp) = (j.y + 0.002, 0.047f32);
    let parrel: Vec<Vec3> = (0..=20)
        .map(|k| {
            let phi = (-100.0 + 200.0 * k as f32 / 20.0).to_radians();
            Vec3::new(phi.sin() * rp, yp, -phi.cos() * rp)
        })
        .collect();
    mb.add(Mat::Rope, rope(&parrel, 0.0035));
    let beads = join((-2..=2).map(|k| {
        let phi = (30.0 * k as f32).to_radians();
        place(
            sphere(0.0095, 14),
            Vec3::new(phi.sin() * rp, yp, -phi.cos() * rp),
            Quat::IDENTITY,
        )
    }));
    mb.add(Mat::VarnishLight, beads);

    let mut bronze = Mesh::default();
    // Throat eye on top of the jaws.
    let eye = gp(0.07, 0.027 + 0.0095);
    bronze.merge(&place(
        rounded_box(Vec3::new(0.02, 0.004, 0.04), 0.0015, 1, 0.2, BoxUv::PLANAR),
        gp(0.07, 0.029),
        rot,
    ));
    bronze.merge(&ring_at(eye, Vec3::X, 0.0075, 0.0025));
    // Bands for the span, and one at the peak.
    let band_at = |d: f32, bronze: &mut Mesh| {
        let r = gaff_radius(d / len);
        bronze.merge(&place(
            band(r - 0.0003, r + 0.0025, 0.018, 24),
            gp(d, 0.0),
            along,
        ));
        r + 0.0025
    };
    let mut span_eyes = Vec::new();
    for d in [0.95f32, 1.95] {
        let r = band_at(d, &mut bronze);
        let e = gp(d, r + 0.0075);
        bronze.merge(&ring_at(e, Vec3::X, 0.0072, 0.0022));
        span_eyes.push(gp(d, r + 0.0125));
    }
    band_at(len - 0.045, &mut bronze);
    // Halyards from the masthead sheaves (their after side) to the throat eye and to a
    // block riding on the span.
    let sheave = |x: f32| Vec3::new(x, MASTHEAD_SHEAVE_Y, 0.0345);
    let block = gp(1.38, 0.15);
    let mut ropes = Mesh::default();
    ropes.merge(&rope(&[sheave(0.0055), eye + nrm * 0.008], 0.004));
    let hal_dir = (sheave(-0.0055) - block).normalize();
    ropes.merge(&rope(&[sheave(-0.0055), block + hal_dir * 0.045], 0.004));
    let block_bottom = block - hal_dir * 0.02;
    ropes.merge(&rope(&[span_eyes[0], block_bottom, span_eyes[1]], 0.0035));
    mb.add(Mat::Rope, ropes);
    mb.add(
        Mat::VarnishLight,
        place(
            rounded_box(Vec3::new(0.024, 0.06, 0.042), 0.008, 2, 0.2, BoxUv::PLANAR),
            block + hal_dir * 0.008,
            super::props::frame_rot(hal_dir, Vec3::X),
        ),
    );
    bronze.merge(&ring_at(block + hal_dir * 0.042, Vec3::X, 0.007, 0.0025));
    small.merge(&bolts(
        &[block + hal_dir * 0.006 + Vec3::X * 0.012],
        Vec3::X,
        0.005,
    ));
    small.merge(&bolts(
        &[block + hal_dir * 0.006 - Vec3::X * 0.012],
        Vec3::NEG_X,
        0.005,
    ));
    mb.add(Mat::Bronze, bronze);
    mb.add_small(Mat::Bronze, small);
}

/// Steamed ash mast hoops, hanging on the mast where the sail's luff is seized to them:
/// each with a riveted lap on its forward side.
fn hoops(mb: &mut ModelBuilder) {
    let n = 9;
    let (ri, h) = (HOOP_INNER_R, 0.024f32);
    let ro = ri + 0.0075;
    let hoop = band(ri, ro, h, 36);
    let lap_path: Vec<Vec3> = (0..=8)
        .map(|k| {
            let phi = -0.3 + 0.6 * k as f32 / 8.0;
            Vec3::new(phi.sin(), 0.0, -phi.cos()) * (ro + 0.0013)
        })
        .collect();
    let lap = sweep(
        &lap_path,
        &vec![Vec3::Y; lap_path.len()],
        |i| {
            let f = 1.0 - 0.6 * (i as f32 / 8.0).powi(4);
            rrect(
                -0.0014 * f,
                0.0014 * f,
                -0.5 * h + 0.001,
                0.5 * h - 0.001,
                0.0008,
                1,
            )
        },
        true,
    );
    let rivets: Vec<Vec3> = [-0.12f32, 0.12]
        .iter()
        .map(|&phi| Vec3::new(phi.sin(), 0.0, -phi.cos()) * (ro + 0.0027))
        .collect();
    let mut wood = Mesh::default();
    let mut copper = Mesh::default();
    for k in 0..n {
        let y = HOOPS_Y[0] + (HOOPS_Y[1] - HOOPS_Y[0]) * k as f32 / (n - 1) as f32;
        let dz = 0.9 * (ri - mast_radius(y));
        let tilt = if k % 2 == 0 { 1.5f32 } else { -1.5 }.to_radians();
        let rot = Quat::from_rotation_x(tilt) * Quat::from_rotation_z(0.4 * tilt);
        let t = Mat4::from_rotation_translation(rot, Vec3::new(0.0, y, dz));
        // Spin the hoop itself so the grain differs from hoop to hoop.
        let spin = Quat::from_rotation_y(rnd(4400 + k as u32, 0) * TAU);
        wood.merge(&xf(&place(hoop.clone(), Vec3::ZERO, spin), t));
        wood.merge(&xf(&lap, t));
        for r in &rivets {
            copper.merge(&xf(&bolts(&[*r], r.normalize(), 0.0028), t));
        }
    }
    mb.add(Mat::VarnishLight, wood);
    mb.add_small(Mat::Bronze, copper);
}

// ---------------------------------------------------------------------------------------
// Rudder model

/// Top of the rudder blade (rudder frame); the cheeks and tiller stand above it.
const RUDDER_TOP_Y: f32 = 0.555;

/// The rudder's section at height `y` (rudder frame): leading and trailing edge (z),
/// greatest thickness, and how flat-sided it is (1 at the head, 0 for the foil below).
fn rudder_section(y: f32) -> (f32, f32, f32, f32) {
    let le = 0.022;
    let te = 0.17 + (le + RUDDER_CHORD - 0.17) * smoothstep(0.42, -0.12, y);
    let s = ((RUDDER_TOP_Y - y) / (RUDDER_TOP_Y - RUDDER_BOTTOM_Y)).clamp(0.0, 1.0);
    let th = 0.03 + (0.018 - 0.03) * s;
    // The bottom rounds off below `yr`, sweeping aft.
    let yr = -0.52;
    let (le, te, th) = if y < yr {
        let kt = (1.0 - ((yr - y) / (yr - RUDDER_BOTTOM_Y)).powi(2))
            .max(0.0)
            .sqrt();
        let mid = 0.5 * (le + te) + 0.07 * (1.0 - kt);
        let half = 0.5 * (te - le) * kt;
        (mid - half, mid + half, th * kt.sqrt())
    } else {
        (le, te, th)
    };
    (le, te, th, smoothstep(0.2, 0.36, y))
}

/// The rudder in its own frame (origin on the hinge axis): blade, cheeks, tiller, pintles.
fn rudder_model() -> ModelBuilder {
    let mut mb = ModelBuilder::new("boat_rudder");
    let n = 46;
    let st: Vec<(Vec3, Vec3, f32, f32)> = (0..=n)
        .map(|k| {
            let s = 1.0 - (1.0 - k as f32 / n as f32).powf(1.4);
            let y = RUDDER_TOP_Y + (RUDDER_BOTTOM_Y - RUDDER_TOP_Y) * s;
            let (le, te, th, flat) = rudder_section(y);
            (Vec3::new(0.0, y, le), Vec3::new(0.0, y, te), th, flat)
        })
        .collect();
    add_painted_at(
        &mut mb,
        foil(&st, Vec3::X, 18, true),
        Mat::Varnish,
        RUDDER_Z,
    );

    // Cheeks either side of the head, through-bolted with the tiller between them.
    let half_top = 0.5 * rudder_section(RUDDER_TOP_Y).2;
    let cheek = vec![
        Vec2::new(0.026, 0.36),
        Vec2::new(0.08, 0.375),
        Vec2::new(0.13, 0.41),
        Vec2::new(0.165, 0.46),
        Vec2::new(0.171, 0.615),
        Vec2::new(0.158, 0.635),
        Vec2::new(0.03, 0.635),
        Vec2::new(0.022, 0.62),
    ];
    let cheek = round_corners(&ccw(cheek), |_| 0.01, 3);
    let mut small = Mesh::default();
    for s in [-1.0f32, 1.0] {
        let x = s * (half_top + 0.006);
        mb.add(Mat::Varnish, side_slab(cheek.clone(), x, 0.012, 0.002));
        let heads: Vec<Vec3> = [
            (0.593f32, 0.07f32),
            (0.593, 0.13),
            (0.44, 0.07),
            (0.5, 0.13),
        ]
        .iter()
        .map(|&(y, z)| Vec3::new(x + s * 0.006, y, z))
        .collect();
        small.merge(&bolts(&heads, Vec3::X * s, 0.0045));
    }

    // Tiller: from between the cheeks forward over the transom into the stern sheets.
    let path = smooth_path(
        &[
            Vec3::new(0.0, 0.593, 0.165),
            Vec3::new(0.0, 0.593, 0.0),
            Vec3::new(0.0, 0.594, -0.25),
            Vec3::new(0.0, 0.583, -0.55),
            Vec3::from(TILLER_END),
        ],
        10,
    );
    let mut dist = vec![0.0f32];
    for w in path.windows(2) {
        dist.push(dist[dist.len() - 1] + w[0].distance(w[1]));
    }
    let total = dist[dist.len() - 1];
    let tiller = sweep(
        &path,
        &vec![Vec3::Y; path.len()],
        |i| {
            let d = dist[i];
            let h = 0.034 + 0.036 * (1.0 - smoothstep(0.17, 0.5, d));
            let w = 2.0 * half_top - 0.001 - 0.005 * smoothstep(0.17, 1.0, d);
            let end = ((d - (total - 0.035)) / 0.035).clamp(0.0, 1.0);
            let f = (1.0 - end * end).sqrt().max(0.2);
            rrect(
                -0.5 * w * f,
                0.5 * w * f,
                -0.5 * h * f,
                0.5 * h * f,
                0.008 * f,
                3,
            )
        },
        true,
    );
    mb.add(Mat::VarnishLight, tiller);

    // Pintles: a knuckle above each gudgeon, its pin down through the gudgeon's eye, a neck
    // to a strap wrapped round the blade's leading edge.
    let mut bronze = Mesh::default();
    for &yg in &GUDGEON_Y {
        let yp = yg + 0.02;
        bronze.merge(&super::props::turned(
            &[
                (0.0, yp - 0.0105),
                (0.0115, yp - 0.0105),
                (0.012, yp - 0.009),
                (0.012, yp + 0.009),
                (0.0105, yp + 0.0105),
                (0.0, yp + 0.0105),
            ],
            20,
            Vec3::ZERO,
        ));
        bronze.merge(&super::props::turned(
            &[
                (0.0, yg - 0.014),
                (0.0045, yg - 0.014),
                (0.0055, yg - 0.012),
                (0.0055, yp - 0.0105),
                (0.0, yp - 0.0105),
            ],
            12,
            Vec3::ZERO,
        ));
        bronze.merge(&place(
            rounded_box(Vec3::new(0.012, 0.02, 0.018), 0.003, 1, 0.2, BoxUv::PLANAR),
            Vec3::new(0.0, yp, 0.012),
            Quat::IDENTITY,
        ));
        let (le, te, th, flat) = rudder_section(yp);
        let chord = te - le;
        let cmax = ((0.13 - le) / chord).min(0.6);
        let m = 14;
        let mut outline = Vec::new();
        for k in (0..=m).rev() {
            let c = cmax * (1.0 - (FRAC_PI_2 * k as f32 / m as f32).cos());
            outline.push(Vec2::new(0.5 * th * foil_shape(c, flat), le + chord * c));
        }
        for k in 1..=m {
            let c = cmax * (1.0 - (FRAC_PI_2 * k as f32 / m as f32).cos());
            outline.push(Vec2::new(-0.5 * th * foil_shape(c, flat), le + chord * c));
        }
        let nn = outline.len();
        let offset: Vec<(Vec3, Vec3)> = (0..nn)
            .map(|i| {
                let t = outline[(i + 1).min(nn - 1)] - outline[i.saturating_sub(1)];
                let out = Vec2::new(-t.y, t.x).normalize_or(Vec2::X);
                let q = outline[i] + out * 0.0025;
                (Vec3::new(q.x, yp, q.y), Vec3::new(out.x, 0.0, out.y))
            })
            .collect();
        let path: Vec<Vec3> = offset.iter().map(|o| o.0).collect();
        bronze.merge(&sweep(
            &path,
            &vec![Vec3::Y; path.len()],
            |_| rrect(-0.002, 0.002, -0.011, 0.011, 0.0015, 1),
            true,
        ));
        for &i in &[2usize, nn - 3] {
            let (q, out) = offset[i];
            small.merge(&bolts(&[q + out * 0.002], out, 0.0035));
        }
    }
    mb.add(Mat::Bronze, bronze);
    mb.add_small(Mat::Bronze, small);
    mb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_mesh(key: &str, m: &Mesh) {
        assert!(
            !m.indices.is_empty() && m.indices.len().is_multiple_of(3),
            "{key}: no triangles"
        );
        assert!(
            m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
            "{key}: index out of range"
        );
        for v in &m.vertices {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            assert!(
                p.is_finite() && p.abs().max_element() < 6.0,
                "{key}: position {p}"
            );
            assert!(Vec2::from(v.uv).is_finite(), "{key}: uv");
            assert!((n.length() - 1.0).abs() < 1e-3, "{key}: normal {n}");
        }
    }

    /// The three models register valid meshes within the triangle budget.
    #[test]
    fn boat_meshes_are_valid() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        let models = {
            let mut commands = world.commands();
            boat_models(&mut commands)
        };
        world.flush();
        let meshes = world.resource::<Assets<Mesh>>();
        let mut total = 0;
        for (name, model) in [
            ("hull", &models.hull),
            ("rig", &models.rig),
            ("rudder", &models.rudder),
        ] {
            let mut tris = 0;
            for key in model.keys() {
                let m = meshes
                    .get(meshes.loaded_by_key(key).expect(key))
                    .expect(key);
                check_mesh(key, m);
                println!("  {key}: {}", m.triangle_count());
                tris += m.triangle_count();
            }
            println!("{name}: {tris} triangles");
            assert!(tris > 0, "{name}");
            total += tris;
        }
        assert!(
            (60_000..=150_000).contains(&total),
            "{total} triangles in all"
        );
        // A second call reuses the models.
        let again = {
            let mut commands = world.commands();
            boat_models(&mut commands)
        };
        world.flush();
        assert_eq!(
            again.hull.keys().collect::<Vec<_>>(),
            models.hull.keys().collect::<Vec<_>>()
        );
    }

    /// The collider points span the hull's dimensions.
    #[test]
    fn hull_points_span_the_hull() {
        let pts = hull_points();
        assert!((30..=60).contains(&pts.len()), "{} points", pts.len());
        let min = pts.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
        let max = pts.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
        assert!((min.z - STEM_Z).abs() < 0.02, "stem {}", min.z);
        assert!((max.z - TRANSOM_Z).abs() < 0.02, "transom {}", max.z);
        assert!((max.x - BEAM * 0.5).abs() < 0.015, "beam {}", max.x);
        assert!((min.x + BEAM * 0.5).abs() < 0.015, "beam {}", min.x);
        assert!((min.y - KEEL_Y).abs() < 0.015, "keel {}", min.y);
        assert!(
            max.y > SHEER_STEM_Y - 0.01 && max.y < STEM_TOP_Y + 0.01,
            "stemhead {}",
            max.y
        );
    }

    /// The boom, gaff and their fittings stay clear of the sail's quad (side view).
    #[test]
    fn rig_stays_clear_of_the_sail() {
        let (model, meshes) = rig_model().finish();
        assert!(model.keys().count() > 0);
        let quad = [SAIL_TACK, SAIL_CLEW, SAIL_PEAK, SAIL_THROAT].map(|c| Vec2::new(c[2], c[1]));
        // Signed distance to the quad's edges (positive inside, counter-clockwise quad).
        let inside = |p: Vec2| {
            (0..4)
                .map(|i| {
                    let (a, b) = (quad[i], quad[(i + 1) % 4]);
                    let e = (b - a).normalize();
                    e.perp_dot(p - a)
                })
                .fold(f32::MAX, f32::min)
        };
        for (key, m) in &meshes {
            for v in &m.vertices {
                let p = Vec3::from(v.position);
                let d = inside(Vec2::new(p.z, p.y));
                assert!(d < -0.01, "{key}: vertex {p} is {d} m inside the sail");
            }
        }
    }

    /// The hull is as wide as its beam and reaches its sheer heights.
    #[test]
    fn hull_lines_match_the_dimensions() {
        let hull = Hull::get();
        let u = hull.u_at_sheer_z(BEAM_Z);
        let p = hull.point(u, 1.0) + hull.normal(u, 1.0) * T;
        assert!((p.x - BEAM * 0.5).abs() < 0.01, "beam {}", p.x);
        assert!((hull.sheer_y(0.0) - SHEER_Y).abs() < 0.01);
        assert!((hull.sheer_y(TRANSOM_Z) - SHEER_TRANSOM_Y).abs() < 1e-4);
        assert!((keel_bottom(0.0) - KEEL_Y).abs() < 0.01);
        assert!((keel_bottom(TRANSOM_Z) - KEEL_TRANSOM_Y).abs() < 1e-4);
        assert!((keel_bottom(FOREFOOT_Z) - FOREFOOT_Y).abs() < 1e-4);
    }
}
