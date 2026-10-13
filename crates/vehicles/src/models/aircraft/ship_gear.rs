//! The spaceplane's retracting landing gear: legs that swing forward and up into their wells
//! and the doors that shut over them.
//!
//! Every part is built in the body frame in one pose (legs down, doors shut) and turned by a
//! [`GearFold`] as the gear position runs between 1 (down) and 0 (stowed): the doors swing
//! over [`DOOR_SPAN`] and the legs over [`LEG_SPAN`], so the doors open before the legs come
//! down and shut after they are in.

use engine_core::math::{Quat, Vec2, Vec3};
use engine_render::{Mesh, Vertex};

use crate::visuals::GearFold;

use super::ship_build::{MAIN_AXLE, MAIN_WELL, NOSE_AXLE, NOSE_WELL};
use super::spaceplane::Hull;
use super::toolkit::*;

/// Leg trunnions inside the hull (right main gear), aft of the axles: the legs rake forward
/// and swing forward and up.
pub const NOSE_PIVOT: Vec3 = Vec3::new(0.0, -1.0, -7.05);
pub const MAIN_PIVOT: Vec3 = Vec3::new(2.2, -0.8, 4.6);
/// The main legs swing about an axis turned a little, so the wheels tuck in from the chine.
const MAIN_AXIS: Vec3 = Vec3::new(0.985, 0.0, -0.174);
/// Axle heights when stowed: the tires clear the belly skin.
const NOSE_STOW_Y: f32 = -0.85;
const MAIN_STOW_Y: f32 = -0.72;
/// Gear positions over which the doors and then the legs move.
pub const DOOR_SPAN: (f32, f32) = (0.0, 0.22);
pub const LEG_SPAN: (f32, f32) = (0.22, 1.0);
/// The doors' skin: how far outside the belly and how thick, m.
const DOOR_OFFSET: f32 = 0.006;
const DOOR_THICK: f32 = 0.022;

/// The turn about `axis` through `pivot` that lifts `axle` to height `y` (radians).
fn stow_angle(pivot: Vec3, axle: Vec3, axis: Vec3, y: f32) -> f32 {
    let lift = |a: f32| (pivot + Quat::from_axis_angle(axis, a) * (axle - pivot)).y;
    let (mut lo, mut hi) = (0.0f32, 3.0f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if lift(mid) < y {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// How the nose leg and the right main leg fold (their wheels with them); the left main is
/// the right one [`mirrored`](GearFold::mirrored).
pub fn leg_folds() -> (GearFold, GearFold) {
    let fold = |pivot: Vec3, axle: Vec3, axis: Vec3, y: f32| GearFold {
        pivot,
        axis,
        angles: (stow_angle(pivot, axle, axis, y), 0.0),
        span: LEG_SPAN,
    };
    (
        fold(NOSE_PIVOT, NOSE_AXLE, Vec3::X, NOSE_STOW_Y),
        fold(MAIN_PIVOT, MAIN_AXLE, MAIN_AXIS.normalize(), MAIN_STOW_Y),
    )
}

/// A gear door: hinged along `x = hinge` over the well's length `z`, shutting across to
/// `x = edge`; open, it hangs from the hinge toward `hang` (across the hinge line).
#[derive(Clone, Copy, Debug)]
pub struct Door {
    pub hinge: f32,
    pub edge: f32,
    pub z: (f32, f32),
    pub hang: Vec3,
}

/// The left and right nose doors, meeting on the centerline, and the right main door,
/// hinged inboard.
pub fn doors() -> [Door; 3] {
    let (c, half) = NOSE_WELL;
    let nose_z = (c.y - half.y + 0.02, c.y + half.y - 0.02);
    let (m, mh) = MAIN_WELL;
    [
        Door {
            hinge: c.x - half.x,
            edge: c.x - 0.005,
            z: nose_z,
            hang: Vec3::NEG_Y,
        },
        Door {
            hinge: c.x + half.x,
            edge: c.x + 0.005,
            z: nose_z,
            hang: Vec3::NEG_Y,
        },
        // Curved to the belly, so it hangs tilted in to clear the tires.
        Door {
            hinge: m.x - mh.x,
            edge: m.x + mh.x,
            z: (m.y - mh.y + 0.02, m.y + mh.y - 0.02),
            hang: Vec3::new(-0.3, -1.0, 0.0),
        },
    ]
}

/// The signed turn about `axis` from direction `from` to direction `to` (both projected
/// across the axis), radians.
fn turn(axis: Vec3, from: Vec3, to: Vec3) -> f32 {
    let f = (from - axis * from.dot(axis)).normalize();
    let t = (to - axis * to.dot(axis)).normalize();
    axis.dot(f.cross(t)).atan2(f.dot(t))
}

/// How door `d` swings: shut when the gear is stowed, hanging straight down when it is out.
pub fn door_fold(h: &Hull, d: &Door) -> GearFold {
    let belly = |x: f32, z: f32| h.exit(Vec3::new(x, -1.0, z), Vec3::NEG_Y).0;
    let a = belly(d.hinge, d.z.0);
    let b = belly(d.hinge, d.z.1);
    let axis = (b - a).normalize();
    let zm = 0.5 * (d.z.0 + d.z.1);
    let s = (d.edge - d.hinge).signum();
    let shut = belly(d.hinge + 0.1 * s, zm) - belly(d.hinge, zm);
    GearFold {
        pivot: a,
        axis,
        angles: (0.0, turn(axis, shut, d.hang)),
        span: DOOR_SPAN,
    }
}

/// Every folding part but the wheels, as (mesh name, material, fold); [`meshes`] builds them.
pub fn parts(h: &Hull) -> Vec<(&'static str, &'static str, GearFold)> {
    let (nose, main) = leg_folds();
    let [nl, nr, m] = doors().map(|d| door_fold(h, &d));
    vec![
        ("gear_nose", "gear", nose),
        ("gear_nose_chrome", "chrome", nose),
        ("gear_main_r", "gear", main),
        ("gear_main_r_chrome", "chrome", main),
        ("gear_main_l", "gear", main.mirrored()),
        ("gear_main_l_chrome", "chrome", main.mirrored()),
        ("door_nose_l", "door", nl),
        ("door_nose_l_inner", "gear", nl),
        ("door_nose_r", "door", nr),
        ("door_nose_r_inner", "gear", nr),
        ("door_main_r", "door", m),
        ("door_main_r_inner", "gear", m),
        ("door_main_l", "door", m.mirrored()),
        ("door_main_l_inner", "gear", m.mirrored()),
    ]
}

/// The meshes named by [`parts`], legs down and doors shut. `underside(x, z)` is the height
/// of the ship's lower skin (hull or wing root) the shut doors lie on.
pub fn meshes(underside: &dyn Fn(f32, f32) -> f32) -> Vec<(&'static str, Mesh)> {
    let (nose, main) = leg_folds();
    let (nose_leg, nose_chrome) = folding_leg(NOSE_PIVOT, NOSE_AXLE, nose.axis, 0.07);
    let (main_leg, main_chrome) = folding_leg(MAIN_PIVOT, MAIN_AXLE, main.axis, 0.09);
    let [nl, nr, m] = doors().map(|d| door_meshes(&d, underside));
    vec![
        ("gear_nose", nose_leg),
        ("gear_nose_chrome", nose_chrome),
        ("gear_main_l", mirrored_x(&main_leg)),
        ("gear_main_l_chrome", mirrored_x(&main_chrome)),
        ("gear_main_r", main_leg),
        ("gear_main_r_chrome", main_chrome),
        ("door_nose_l", nl.0),
        ("door_nose_l_inner", nl.1),
        ("door_nose_r", nr.0),
        ("door_nose_r_inner", nr.1),
        ("door_main_l", mirrored_x(&m.0)),
        ("door_main_l_inner", mirrored_x(&m.1)),
        ("door_main_r", m.0),
        ("door_main_r_inner", m.1),
    ]
}

/// A leg from its trunnion at `pivot` down to `axle`: the outer cylinder, the collar at the
/// gland, the trunnion across the fold `axis` with two arms down to the cylinder, and the
/// torque links (paint); the piston (chrome).
pub fn folding_leg(pivot: Vec3, axle: Vec3, axis: Vec3, r: f32) -> (Mesh, Mesh) {
    let mut paint = Mesh::default();
    let mut chrome = Mesh::default();
    let along = (axle - pivot).normalize();
    let mid = pivot.lerp(axle, 0.55);
    paint.merge(&tube(&[pivot, mid], |_| r, 18, true));
    chrome.merge(&tube(&[mid, axle - along * 0.02], |_| r * 0.72, 16, true));
    paint.merge(&tube(
        &[mid - along * 0.03, mid + along * 0.03],
        |_| r * 1.12,
        18,
        true,
    ));
    let w = 0.3;
    paint.merge(&tube(
        &[pivot - axis * w, pivot + axis * w],
        |_| r * 0.7,
        14,
        true,
    ));
    let knee = pivot.lerp(mid, 0.6);
    for s in [-1.0f32, 1.0] {
        paint.merge(&tube(
            &[pivot + axis * (w * 0.85 * s), knee],
            |_| r * 0.38,
            10,
            true,
        ));
    }
    // Torque links on the front of the strut (two arms).
    let f = axis.cross(along).normalize() * (r * 1.6);
    let k = mid.lerp(axle, 0.5) + f;
    paint.merge(&tube(&[mid + f * 0.6, k], |_| 0.022, 8, true));
    paint.merge(&tube(
        &[k, axle - along * 0.10 + f * 0.6],
        |_| 0.022,
        8,
        true,
    ));
    (paint, chrome)
}

/// Door `d` shut on the `underside`: (tiled outer skin and edges, the inside with its hinge
/// tube and stiffeners).
pub fn door_meshes(d: &Door, underside: &dyn Fn(f32, f32) -> f32) -> (Mesh, Mesh) {
    let (nz, nx) = (8, 10);
    let s = (d.edge - d.hinge).signum();
    let outer: Vec<Vec<Vec3>> = (0..nz)
        .map(|i| {
            let z = d.z.0 + (d.z.1 - d.z.0) * i as f32 / (nz - 1) as f32;
            (0..nx)
                .map(|j| {
                    let x = d.hinge + (d.edge - d.hinge) * j as f32 / (nx - 1) as f32;
                    Vec3::new(x, underside(x, z) - DOOR_OFFSET, z)
                })
                .collect()
        })
        .collect();
    let up = Vec3::Y * DOOR_THICK;
    let inner: Vec<Vec<Vec3>> = outer
        .iter()
        .map(|row| row.iter().map(|p| *p + up).collect())
        .collect();
    // Grid faces point along (across × along) = −Y·s: down for a door shutting toward +x.
    let flip = s < 0.0;
    let mut skin = grid(&outer, false, flip, |i, j| {
        let p = outer[i][j];
        Vec2::new(p.x, p.z)
    });
    // Edges, facing out of the door.
    let center = outer[nz / 2][nx / 2];
    let mut ring: Vec<Vec3> = Vec::new();
    ring.extend((0..nx).map(|j| outer[0][j]));
    ring.extend((1..nz).map(|i| outer[i][nx - 1]));
    ring.extend((0..nx - 1).rev().map(|j| outer[nz - 1][j]));
    ring.extend((1..nz - 1).rev().map(|i| outer[i][0]));
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let mut out = (b - a).cross(Vec3::Y).normalize_or_zero();
        if out.dot((a + b) * 0.5 - center) < 0.0 {
            out = -out;
        }
        wall(&mut skin, a, b, up, out);
    }
    let mut inside = grid(&inner, false, !flip, |_, _| Vec2::ZERO);
    let a = outer[0][0] + up;
    let b = outer[nz - 1][0] + up;
    inside.merge(&tube(&[a, b], |_| 0.02, 10, true));
    for i in [1, nz / 2, nz - 2] {
        let rib: Vec<Vec3> = inner[i].iter().map(|p| *p + Vec3::Y * 0.012).collect();
        inside.merge(&tube(&rib, |_| 0.012, 6, true));
    }
    (skin, inside)
}

/// The height of the ship's lowest skin over a region, from its drawn meshes (vertical rays
/// against their triangles, binned on a coarse grid).
pub struct Underside {
    tris: Vec<[Vec3; 3]>,
    cells: Vec<Vec<u32>>,
    lo: Vec2,
    n: (usize, usize),
}

impl Underside {
    const CELL: f32 = 0.25;

    /// Index the triangles of `meshes` over the plan-view box `lo`..`hi` (x, z).
    pub fn new(meshes: &[&Mesh], lo: Vec2, hi: Vec2) -> Self {
        let n = (
            ((hi.x - lo.x) / Self::CELL).ceil() as usize,
            ((hi.y - lo.y) / Self::CELL).ceil() as usize,
        );
        let cell = |x: f32, z: f32| {
            (
                (((x - lo.x) / Self::CELL).max(0.0) as usize).min(n.0 - 1),
                (((z - lo.y) / Self::CELL).max(0.0) as usize).min(n.1 - 1),
            )
        };
        let mut tris = Vec::new();
        let mut cells = vec![Vec::new(); n.0 * n.1];
        for m in meshes {
            for t in m.indices.chunks(3) {
                let p = [0, 1, 2].map(|k| Vec3::from(m.vertices[t[k] as usize].position));
                let (a, b) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
                if b.x < lo.x || a.x > hi.x || b.z < lo.y || a.z > hi.y {
                    continue;
                }
                let id = tris.len() as u32;
                tris.push(p);
                let (c0, c1) = (cell(a.x, a.z), cell(b.x, b.z));
                for cz in c0.1..=c1.1 {
                    for cx in c0.0..=c1.0 {
                        cells[cz * n.0 + cx].push(id);
                    }
                }
            }
        }
        Self { tris, cells, lo, n }
    }

    /// The lowest skin height at (x, z); −1.6 (the belly) where nothing is over it.
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let cx = ((x - self.lo.x) / Self::CELL).floor();
        let cz = ((z - self.lo.y) / Self::CELL).floor();
        if cx < 0.0 || cz < 0.0 || cx as usize >= self.n.0 || cz as usize >= self.n.1 {
            return -1.6;
        }
        let mut low = f32::MAX;
        for &t in &self.cells[cz as usize * self.n.0 + cx as usize] {
            let [a, b, c] = self.tris[t as usize];
            let d = (b.x - a.x) * (c.z - a.z) - (c.x - a.x) * (b.z - a.z);
            if d.abs() < 1e-12 {
                continue;
            }
            let u = ((x - a.x) * (c.z - a.z) - (c.x - a.x) * (z - a.z)) / d;
            let v = ((b.x - a.x) * (z - a.z) - (x - a.x) * (b.z - a.z)) / d;
            if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                low = low.min(a.y + u * (b.y - a.y) + v * (c.y - a.y));
            }
        }
        if low == f32::MAX { -1.6 } else { low }
    }
}

/// A quad from edge `a`–`b` up by `up`, facing `out`.
fn wall(m: &mut Mesh, a: Vec3, b: Vec3, up: Vec3, out: Vec3) {
    let base = m.vertices.len() as u32;
    for p in [a, b, b + up, a + up] {
        m.vertices.push(Vertex::new(p, out, Vec2::new(p.x, p.z)));
    }
    let facing = (b - a).cross(up).dot(out) > 0.0;
    let idx: [u32; 6] = if facing {
        [0, 1, 2, 0, 2, 3]
    } else {
        [0, 2, 1, 0, 3, 2]
    };
    m.indices.extend(idx.iter().map(|i| base + i));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::spaceplane_wheels;

    /// Every wheel swings in through its well's opening and ends up inside the skin.
    #[test]
    fn the_wheels_fold_through_their_wells_into_the_hull() {
        let h = Hull::new();
        let belly = |x: f32, z: f32| h.exit(Vec3::new(x, -1.0, z), Vec3::NEG_Y).0.y;
        let (nose, main) = leg_folds();
        for (name, fold, axle, r, half_w, well) in [
            ("nose", nose, NOSE_AXLE, 0.5, 0.27, NOSE_WELL),
            ("main", main, MAIN_AXLE, 0.6, 0.32, MAIN_WELL),
        ] {
            let (c, half) = well;
            let (mut fwd, mut aft) = (f32::MAX, f32::MIN);
            let (mut lo_x, mut hi_x) = (f32::MAX, f32::MIN);
            for k in 0..=200 {
                let g = k as f32 / 200.0;
                let p = fold.apply(g, axle);
                let skin = belly(p.x, p.z);
                // The tire's chord where it crosses the skin.
                let dy = (skin - p.y).abs();
                if dy < r {
                    let chord = (r * r - dy * dy).sqrt();
                    fwd = fwd.min(p.z - chord);
                    aft = aft.max(p.z + chord);
                    lo_x = lo_x.min(p.x - half_w);
                    hi_x = hi_x.max(p.x + half_w);
                }
            }
            eprintln!("{name}: crosses the skin over z {fwd:.2}..{aft:.2}, x {lo_x:.2}..{hi_x:.2}");
            assert!(
                fwd > c.y - half.y && aft < c.y + half.y,
                "{name} clips the well's ends"
            );
            assert!(
                lo_x > c.x - half.x - 0.01 && hi_x < c.x + half.x + 0.01,
                "{name} clips its sides"
            );
            let stowed = fold.apply(0.0, axle);
            let skin = belly(stowed.x, stowed.z);
            eprintln!("{name}: stowed axle {stowed:?}, skin {skin:.2}");
            assert!(
                stowed.y - r > skin + 0.05,
                "{name} wheel pokes out when stowed"
            );
        }
        // The wheels the legs carry are the ones the physics model rolls on.
        let w = spaceplane_wheels();
        assert!((w.wheels[0].mount.z - NOSE_AXLE.z).abs() < 1e-3);
        assert!((w.wheels[2].mount.x - MAIN_AXLE.x).abs() < 1e-3);
    }

    /// Shut doors lie on the belly; open ones hang down beside the wheels.
    #[test]
    fn the_doors_shut_flush_and_hang_clear_when_open() {
        let h = Hull::new();
        for d in doors() {
            let fold = door_fold(&h, &d);
            // A point near the far edge (the hull ends just past the main door's edge).
            let x = d.hinge + 0.9 * (d.edge - d.hinge);
            let tip = h.exit(Vec3::new(x, -1.0, fold.pivot.z), Vec3::NEG_Y).0;
            let open = fold.apply(1.0, tip);
            eprintln!(
                "door {d:?}: opens {:.0}°, edge hangs at {open:?}",
                fold.angles.1.to_degrees()
            );
            assert!(
                fold.angles.1.abs() > 70f32.to_radians()
                    && fold.angles.1.abs() < 115f32.to_radians()
            );
            assert!(
                open.y < fold.pivot.y - (d.edge - d.hinge).abs() * 0.8,
                "door doesn't hang down"
            );
            let toward_wheels = (open.x - d.hinge) * (d.edge - d.hinge).signum();
            assert!(
                toward_wheels < 0.06,
                "the open door leans into the wheel well"
            );
        }
    }
}
