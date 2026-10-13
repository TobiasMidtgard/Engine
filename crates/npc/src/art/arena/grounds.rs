//! The lived-in arena floor, and what lies and stands on it.
//!
//! The sand is two layers on one polar grid. Trampled sand lies wherever fighters, carts and
//! attendants go; freshly raked sand (concentric lanes of tine grooves) covers the rest a few
//! millimeters higher, cut out by an alpha mask: the layer's vertex alpha falls toward the
//! trampled ground and the raked texture's alpha (clumps and stray footprints) decides the
//! edge per pixel, so it tears like sand walked over instead of following the triangles. Dust
//! banks against the curb (deepest on the windward walls, swept clear at the gates) and
//! around the column plinths; carts have cut ruts at the gates and bodies were dragged toward
//! them; water slopped from the troughs darkens the sand in soft decals, with a few puddles.
//! Straw, broken arrows and splinters lie where they fell, and stone troughs, water casks and
//! weapon racks stand along the walls (decoration only: no colliders, behind the fighting
//! floor's edge).
//!
//! Coordinates are the arena's: meters from its origin, `q = (x, z)` on the floor, angles
//! from +X toward +Z as `demo` lays out the stands.

use std::hash::{Hash, Hasher};

use super::*;
use crate::art::mesh::{bake_par_alpha, smooth_normals};

/// The raked layer lies this far above the trampled sand.
const RAKE_LIFT: f32 = 0.005;
/// Damp-sand decals float this far above the sand (and the raked layer).
const DECAL_LIFT: f32 = 0.009;
/// The walls facing this way (radians) hold the deepest drifts: the wind comes over the
/// stands opposite and drops its dust against them.
const WIND: f32 = 1.05;
/// Distance from the center to the curb's inner faces.
const APOTHEM: f32 = 40.25;
/// The gates, as angles.
const GATES: [f32; 4] = [0.0, FRAC_PI_2, PI, PI + FRAC_PI_2];
/// Raked layer coverage below which a vertex is plain trampled sand.
const TORN: f32 = 0.49;

fn polar(a: f32, rho: f32) -> Vec2 {
    Vec2::new(a.cos(), a.sin()) * rho
}

/// Distance from the center to the curb at angle `a` (the floor is a 36-gon).
fn edge(a: f32) -> f32 {
    let s = TAU / 36.0;
    APOTHEM / (a - (a / s).round() * s).cos()
}

/// Angle of `q`, 0..TAU.
fn angle(q: Vec2) -> f32 {
    q.y.atan2(q.x).rem_euclid(TAU)
}

/// `a - b` wrapped into −π..π.
fn turn(a: f32, b: f32) -> f32 {
    (a - b + PI).rem_euclid(TAU) - PI
}

/// 1 in a gate's opening, fading out over the next few degrees.
fn gate(a: f32) -> f32 {
    GATES
        .iter()
        .map(|&g| 1.0 - smooth(0.06, 0.11, turn(a, g).abs()))
        .fold(0.0, f32::max)
}

fn seg_dist(q: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((q - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    q.distance(a + ab * t)
}

/// A smooth curve through `points` (Catmull-Rom), sampled about every `step` meters.
fn curve(points: &[Vec2], step: f32) -> Vec<Vec2> {
    let n = points.len();
    let mut out = vec![points[0]];
    for i in 0..n - 1 {
        let (p0, p1) = (points[i.saturating_sub(1)], points[i]);
        let (p2, p3) = (points[i + 1], points[(i + 2).min(n - 1)]);
        let k = (p1.distance(p2) / step).ceil().max(1.0) as usize;
        for j in 1..=k {
            let t = j as f32 / k as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    out
}

/// `path` moved sideways by `by` meters (to the left of its direction).
fn offset(path: &[Vec2], by: f32) -> Vec<Vec2> {
    let n = path.len();
    (0..n)
        .map(|i| {
            let d = (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec2::X);
            path[i] + Vec2::new(-d.y, d.x) * by
        })
        .collect()
}

fn v3(q: Vec2, y: f32) -> Vec3 {
    Vec3::new(q.x, y, q.y)
}

/// A triangle with its own (flat) normal, turned to face away from `inside`.
fn facet(m: &mut Mesh, p: [Vec3; 3], inside: Vec3, color: Vec4, uv: impl Fn(Vec3) -> Vec2) {
    let mut p = p;
    let mut n = (p[1] - p[0]).cross(p[2] - p[0]);
    if n.dot((p[0] + p[1] + p[2]) / 3.0 - inside) < 0.0 {
        p.swap(1, 2);
        n = -n;
    }
    let n = n.normalize_or(Vec3::Y);
    let base = m.vertices.len() as u32;
    for v in p {
        m.vertices.push(Vertex::new(v, n, uv(v)).with_color(color));
    }
    m.indices.extend_from_slice(&[base, base + 1, base + 2]);
}

/// A footprint on the floor (a collider or a prop): center, local X direction, half extents.
#[derive(Debug, Clone, Copy)]
struct Block {
    at: Vec2,
    x: Vec2,
    half: Vec2,
    wood: bool,
}

impl Block {
    fn local(&self, q: Vec2) -> Vec2 {
        let d = q - self.at;
        Vec2::new(d.dot(self.x), d.dot(Vec2::new(-self.x.y, self.x.x)))
    }

    fn inside(&self, q: Vec2, margin: f32) -> bool {
        let l = self.local(q).abs();
        l.x < self.half.x + margin && l.y < self.half.y + margin
    }

    fn world(&self, l: Vec2) -> Vec2 {
        self.at + self.x * l.x + Vec2::new(-self.x.y, self.x.x) * l.y
    }
}

/// A prop along the wall, by kind and variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prop {
    Trough(u32),
    Cask(u32),
    Rack(u32),
}

/// Where a prop stands: its middle on the floor, and the angle (as for the stands) its back
/// faces, away from the center.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub prop: Prop,
    pub at: Vec2,
    pub facing: f32,
}

/// Where the floor is trampled, wet and littered, and what stands along the walls: planned
/// from the layout, then read by every builder here.
#[derive(Debug, Clone, Default)]
pub struct Grounds {
    /// Trampled spots: center, radius, strength.
    spots: Vec<(Vec2, f32, f32)>,
    /// Trampled paths: from, to, half width, strength.
    paths: Vec<(Vec2, Vec2, f32, f32)>,
    columns: Vec<Vec2>,
    /// Cover walls and crate stacks (arrows and splinters gather around them).
    cover: Vec<Block>,
    /// Footprints litter keeps out of.
    solids: Vec<Block>,
    /// Discs litter keeps out of: center, radius.
    clear: Vec<(Vec2, f32)>,
    /// The duel ring's curb: center, radius.
    ring: (Vec2, f32),
    pub props: Vec<Placed>,
    /// Cart wheel ruts: path and depth.
    ruts: Vec<(Vec<Vec2>, f32)>,
    /// Body drag marks.
    drags: Vec<Vec<Vec2>>,
    /// Damp sand: center, radius, strength.
    damp: Vec<(Vec2, f32, f32)>,
    /// Standing water: center, radius.
    puddles: Vec<(Vec2, f32)>,
    /// Straw: center, radius, stalks.
    straw: Vec<(Vec2, f32, u32)>,
    /// Identifies the plan (meshes built from it are cached per plan).
    pub tag: u64,
}

impl Grounds {
    /// Plan the floor for an arena laid out by `demo::spawn_demo` (`layout`'s pieces are in
    /// `world`).
    pub fn plan(world: &World, layout: &ArenaLayout) -> Grounds {
        let mut g = Grounds::default();
        let o = layout.origin;
        let rel = |p: DVec3| {
            let d = (p - o).as_vec3();
            Vec2::new(d.x, d.z)
        };
        for &(c, r) in &layout.trample {
            g.spots.push((c, r, 1.0));
        }
        for &(e, piece) in &layout.pieces {
            let (Some(t), Some(c)) = (world.get::<Transform>(e), world.get::<Collider>(e)) else {
                continue;
            };
            let at = rel(t.translation);
            let x = t.rotation * Vec3::X;
            let half = match c.shape {
                ColliderShape::Cuboid { half_extents } => Vec2::new(half_extents.x, half_extents.z),
                ColliderShape::Cylinder { radius, .. } => Vec2::splat(radius),
                _ => continue,
            };
            let block = Block {
                at,
                x: Vec2::new(x.x, x.z).normalize_or(Vec2::X),
                half,
                wood: matches!(piece, Piece::TimberWall | Piece::Crates(_)),
            };
            match piece {
                Piece::Podium => {}
                Piece::Pillar(_) => {
                    g.columns.push(at);
                    g.spots.push((at, 2.5, 0.8));
                    g.clear.push((at, 1.3));
                }
                Piece::Barrel(_) => g.clear.push((at, 0.45)),
                Piece::StoneWall(_) | Piece::TimberWall | Piece::Crates(_) => {
                    g.cover.push(block);
                    g.solids.push(block);
                }
                _ => g.solids.push(block),
            }
        }
        for &b in &layout.braziers {
            g.clear.push((rel(b), 0.75));
            g.spots.push((rel(b), 2.0, 0.75));
        }
        for &m in &layout.mats {
            // The sparring mats (3.6 × 2.6 m).
            g.solids.push(Block {
                at: rel(m),
                x: Vec2::X,
                half: Vec2::new(1.85, 1.35),
                wood: false,
            });
        }
        if let (Some(&first), Some(&last)) = (layout.mats.first(), layout.mats.last()) {
            g.paths.push((rel(first), rel(last), 1.7, 0.9));
            // Straw knocked out of the sparring posts (0.85 m from each mat's middle) lies
            // round the mats.
            for &m in &layout.mats {
                g.straw.push((rel(m) - Vec2::X * 0.85, 2.2, 70));
            }
        }
        g.ring = (rel(layout.duel_ring), 5.0);
        let mut rng = Rng::new(7070);
        // Trails worn between neighboring cover, columns and the squads' spots: fighters run
        // from one to the next.
        let mut nodes: Vec<Vec2> = g.cover.iter().map(|b| b.at).collect();
        nodes.extend(&g.columns);
        nodes.extend([Vec2::ZERO, Vec2::new(0.0, 17.0), Vec2::new(0.0, -16.0)]);
        let mut links: Vec<(usize, usize)> = Vec::new();
        for i in 0..nodes.len() {
            let mut near: Vec<(f32, usize)> = (0..nodes.len())
                .filter(|&j| j != i)
                .map(|j| (nodes[i].distance(nodes[j]), j))
                .filter(|&(d, _)| d < 13.0)
                .collect();
            near.sort_by(|a, b| a.0.total_cmp(&b.0));
            for &(_, j) in near.iter().take(2) {
                let l = (i.min(j), i.max(j));
                if !links.contains(&l) {
                    links.push(l);
                }
            }
        }
        for (i, j) in links {
            let (a, b) = (nodes[i], nodes[j]);
            let d = (b - a).normalize_or(Vec2::X);
            let bow = Vec2::new(-d.y, d.x) * rng.signed() * 0.12 * a.distance(b);
            let trail = curve(&[a + d * 1.2, (a + b) * 0.5 + bow, b - d * 1.2], 1.5);
            for w in trail.windows(2) {
                g.paths.push((w[0], w[1], 0.6, 0.72));
            }
        }
        // Gates: trampled aprons and approaches, cart ruts, straw dropped from the carts.
        for (k, &ga) in GATES.iter().enumerate() {
            let inward = -polar(ga, 1.0);
            let side = Vec2::new(-inward.y, inward.x);
            g.paths.push((polar(ga, 40.5), polar(ga, 27.5), 2.8, 1.0));
            g.spots.push((polar(ga, 38.8), 4.2, 1.0));
            // The east gate opens onto the sparring lane: its carts turn sooner.
            let (len, swing) = if k == 0 { (8.0, 3.0) } else { (12.0, 6.5) };
            for (bend, depth) in [(1.0f32, 0.022f32), (-0.75, 0.014)] {
                let lat = rng.signed() * 0.5;
                let p0 = polar(ga, 40.4) + side * lat;
                let mid = curve(
                    &[
                        p0,
                        p0 + inward * (len * 0.35),
                        p0 + inward * (len * 0.7) + side * (bend * swing * 0.3),
                        p0 + inward * len + side * (bend * swing),
                    ],
                    0.4,
                );
                for w in [-0.68, 0.68] {
                    g.ruts.push((offset(&mid, w), depth));
                }
                for s in mid.chunks(5) {
                    g.paths.push((s[0], s[s.len() - 1], 1.3, 0.85));
                }
            }
            let at = polar(ga, 37.6) + side * rng.signed() * 1.5;
            g.straw.push((at, 2.4, 170));
        }
        // Bodies dragged out of the fights toward the gates: from the duel ring to the south
        // gate, from the southwest cover round the wall-run wall to the west gate, and from
        // the middle up to the north gate.
        for pts in [
            vec![
                Vec2::new(8.2, -27.4),
                Vec2::new(5.5, -31.0),
                Vec2::new(2.6, -35.5),
                Vec2::new(1.0, -40.4),
            ],
            vec![
                Vec2::new(-7.0, -9.0),
                Vec2::new(-14.0, -13.5),
                Vec2::new(-21.0, -17.0),
                Vec2::new(-31.0, -9.5),
                Vec2::new(-40.4, -1.5),
            ],
            vec![
                Vec2::new(-3.0, 4.0),
                Vec2::new(-4.5, 14.0),
                Vec2::new(-1.5, 30.0),
                Vec2::new(0.0, 40.4),
            ],
        ] {
            let path = curve(&pts, 0.4);
            for s in path.chunks(5) {
                g.paths.push((s[0], s[s.len() - 1], 0.75, 0.8));
            }
            g.drags.push(path);
        }
        // Troughs with a water cask at one end (away from the nearest gate), the sand dark
        // and puddled where water slopped over, drips trailing off toward the middle.
        let (rho_t, rho_c) = (39.25f32, 39.45f32);
        for (k, deg) in [38.0f32, 142.0, 218.0, 322.0].into_iter().enumerate() {
            let a = deg.to_radians();
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let at = polar(a, rho_t);
            g.props.push(Placed {
                prop: Prop::Trough(k as u32 % 2),
                at,
                facing: a + rng.signed() * 0.02,
            });
            let tangent = Vec2::new(a.sin(), -a.cos());
            g.solids.push(Block {
                at,
                x: tangent,
                half: Vec2::new(1.0, 0.38),
                wood: false,
            });
            let ca = a + side * 0.054;
            let cask = polar(ca, rho_c);
            g.props.push(Placed {
                prop: Prop::Cask(k as u32 % 2),
                at: cask,
                facing: rng.range(0.0, TAU),
            });
            g.clear.push((cask, 0.42));
            let spill = a - side * 0.02;
            g.spots.push((polar(a, rho_t - 1.3), 2.1, 0.95));
            g.spots.push((polar(ca, rho_c - 0.9), 1.2, 0.8));
            g.paths.push((
                polar(a, rho_t - 1.0),
                polar(a + side * 0.06, 27.0),
                0.9,
                0.62,
            ));
            g.damp.push((polar(a, rho_t - 1.0), 1.75, 0.7));
            g.damp.push((polar(spill, rho_t - 0.75), 1.15, 0.85));
            g.damp.push((polar(ca, rho_c - 0.5), 0.8, 0.65));
            g.puddles.push((polar(spill, rho_t - 0.95), 0.5));
            g.puddles
                .push((polar(a + side * 0.012, rho_t - 1.45), 0.28));
            // Drips from a carried bucket.
            let mut p = polar(a + side * 0.008, rho_t - 1.9);
            let mut dir = -polar(a + side * 0.1, 1.0);
            for i in 0..12 {
                let r = 0.17 - 0.008 * i as f32;
                g.damp.push((p, r * rng.range(0.7, 1.2), 0.75));
                dir = Vec2::from_angle(rng.signed() * 0.25).rotate(dir);
                p += dir * rng.range(0.35, 0.6);
            }
            g.straw.push((polar(a, rho_t - 0.9), 1.0, 70));
        }
        // Weapon racks.
        for (k, deg) in [57.0f32, 123.0, 203.0, 337.0].into_iter().enumerate() {
            let a = deg.to_radians();
            let at = polar(a, 39.35);
            g.props.push(Placed {
                prop: Prop::Rack(k as u32 % 2),
                at,
                facing: a,
            });
            g.solids.push(Block {
                at,
                x: Vec2::new(a.sin(), -a.cos()),
                half: Vec2::new(1.0, 0.42),
                wood: true,
            });
            g.spots.push((polar(a, 38.0), 1.9, 0.9));
            g.straw.push((polar(a, 38.5), 1.2, 110));
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{g:?}").hash(&mut h);
        g.tag = h.finish();
        g
    }

    fn warp(q: Vec2) -> f32 {
        let p = v3(q, 0.0);
        0.22 * vn3(p * 0.55, 931) + 0.08 * vn3(p * 1.7, 932)
    }

    /// How trampled the sand is at `q`: 0 (left as raked) .. 1.
    pub fn trample(&self, q: Vec2) -> f32 {
        let w = Self::warp(q);
        let fall = |d: f32| {
            let d = (d + w).max(0.0);
            (1.0 - d * d).max(0.0)
        };
        let mut t = 0.0f32;
        for &(c, r, s) in &self.spots {
            let d = q.distance(c) / r;
            if d < 1.4 {
                t = t.max(s * fall(d));
            }
        }
        for &(a, b, hw, s) in &self.paths {
            let d = seg_dist(q, a, b) / hw;
            if d < 1.4 {
                t = t.max(s * fall(d));
            }
        }
        // The rakes stop short of the curb, where the dust drifts.
        let s = edge(angle(q)) - q.length();
        t.max(1.0 - smooth(0.7, 1.6, s + 0.5 * w)).min(1.0)
    }

    /// The raked layer's vertex alpha for trampling `t`: exactly 1 where it is untouched,
    /// below [`TORN`] where only trampled sand shows.
    fn rake(t: f32) -> f32 {
        1.0 - 0.55 * smooth(0.22, 0.62, t)
    }

    /// Dust banked against the curb, meters.
    fn wall_drift(q: Vec2) -> f32 {
        let a = angle(q);
        let s = edge(a) - q.length();
        if s > 2.6 {
            return 0.0;
        }
        let along = Vec3::new(a * 40.0 * 0.16, 0.0, 3.1);
        let n1 = vn3(along, 941);
        let n2 = vn3(along * 2.7, 942);
        let wind = 0.5 + 0.5 * (a - WIND).cos();
        let width = (0.7 + 1.5 * wind) * (0.8 + 0.3 * n1);
        let height = (0.022 + 0.05 * wind) * (0.85 + 0.25 * n2 + 0.1 * n1) * (1.0 - gate(a));
        let x = (s / width).clamp(0.0, 1.0);
        height * (1.0 - x) * (1.0 - x)
    }

    /// Height of the trampled sand at `q` (trampling `t`).
    fn height(&self, q: Vec2, t: f32) -> f32 {
        let p = v3(q, 0.0);
        0.017 + 0.004 * vn3(p * 0.3, 901) + 0.0008 * vn3(p * 0.7, 902) - 0.002 * t
            + Self::wall_drift(q)
    }

    /// Height of the trampled sand at `q`.
    pub fn ground(&self, q: Vec2) -> f32 {
        self.height(q, self.trample(q))
    }

    /// Where litter lies at `q`: on whatever sand may show there (raked or trampled), with
    /// a hair to spare for the floor's triangles cutting across the curved sand.
    fn top(&self, q: Vec2) -> f32 {
        let t = self.trample(q);
        self.height(q, t)
            + 0.003
            + if Self::rake(t) >= TORN {
                RAKE_LIFT
            } else {
                0.0
            }
    }

    /// Vertex tint of the sand: darker where trampled, paler where dust drifted.
    fn tint(q: Vec2, t: f32, drift: f32) -> Vec4 {
        let p = v3(q, 0.0);
        let macro_n = vn3(p * 0.12, 911) * 0.6 + vn3(p * 0.37, 912) * 0.4;
        let k = (1.0 + 0.06 * macro_n) * (1.0 - 0.14 * t);
        let c = Vec3::new(k, k * (0.995 - 0.01 * t), k * (0.99 - 0.025 * t));
        c.lerp(Vec3::new(1.08, 1.07, 1.05), smooth(0.0, 0.05, drift) * 0.85)
            .extend(1.0)
    }

    /// Whether litter may lie at `q`: inside the curb and clear of colliders, props, mats,
    /// braziers and the ring's curb.
    fn free(&self, q: Vec2, margin: f32) -> bool {
        q.length() < 39.7
            && !self.solids.iter().any(|b| b.inside(q, margin))
            && !self.clear.iter().any(|&(c, r)| q.distance(c) < r + margin)
            && (q.distance(self.ring.0) - self.ring.1).abs() > 0.3 + margin
    }

    // ---------------------------------------------------------------------------- sand

    /// The floor inside the curb: trampled sand (texture u, v = meters) and the raked layer
    /// over it (u around the arena, v outward, so the tine grooves run in circles).
    pub fn floor_meshes(&self) -> (Mesh, Mesh) {
        // Rings ~0.69 m apart, finer where dust drifts against the curb; points ~0.62 m
        // apart (multiples of 8, the raked sectors), 360 near the curb (the 36-gon's corners).
        let mut ds = steps(0.0, 37.0, 54);
        ds.extend(steps(37.4, APOTHEM, 14));
        let counts: Vec<usize> = ds
            .iter()
            .map(|&d| {
                if d == 0.0 {
                    1
                } else if d >= 33.0 {
                    360
                } else {
                    (((TAU * d / 0.62) / 8.0).ceil() as usize * 8).clamp(16, 360)
                }
            })
            .collect();
        // Vertices: position, angle, distance, trample.
        struct Pt {
            q: Vec2,
            a: f32,
            rho: f32,
            t: f32,
            y: f32,
        }
        let rings: Vec<Vec<Pt>> = par_map(ds.len(), |i| {
            (0..counts[i])
                .map(|k| {
                    let a = k as f32 / counts[i] as f32 * TAU;
                    let rho = ds[i] / APOTHEM * edge(a);
                    let q = polar(a, rho);
                    let t = self.trample(q);
                    Pt {
                        q,
                        a,
                        rho,
                        t,
                        y: self.height(q, t),
                    }
                })
                .collect()
        });
        let mut starts = Vec::with_capacity(rings.len());
        let mut pts: Vec<&Pt> = Vec::new();
        for r in &rings {
            starts.push(pts.len() as u32);
            pts.extend(r.iter());
        }
        // Zip neighboring rings into triangles (counter-clockwise from above).
        let mut tris: Vec<[u32; 3]> = Vec::new();
        for i in 0..rings.len() - 1 {
            let (na, nb) = (counts[i], counts[i + 1]);
            let (sa, sb) = (starts[i], starts[i + 1]);
            let ia = |k: usize| sa + (k % na) as u32;
            let ib = |k: usize| sb + (k % nb) as u32;
            let (mut ka, mut kb) = (0usize, 0usize);
            while ka < na || kb < nb {
                let next_a = (ka + 1) as f32 / na as f32;
                let next_b = (kb + 1) as f32 / nb as f32;
                let t = if kb >= nb || (ka < na && na > 1 && next_a <= next_b) {
                    ka += 1;
                    [ia(ka - 1), ia(ka), ib(kb)]
                } else {
                    kb += 1;
                    [ia(ka), ib(kb), ib(kb - 1)]
                };
                if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                    tris.push(t);
                }
                if na == 1 {
                    ka = na;
                }
            }
        }
        let pos = |i: u32| {
            let p = pts[i as usize];
            v3(p.q, p.y)
        };
        for t in &mut tris {
            let n = (pos(t[1]) - pos(t[0])).cross(pos(t[2]) - pos(t[0]));
            if n.y < 0.0 {
                t.swap(1, 2);
            }
        }
        let mut normals = vec![Vec3::ZERO; pts.len()];
        for t in &tris {
            let n = (pos(t[1]) - pos(t[0])).cross(pos(t[2]) - pos(t[0]));
            for &i in t {
                normals[i as usize] += n;
            }
        }
        let alpha: Vec<f32> = pts.iter().map(|p| Self::rake(p.t)).collect();
        let (mut base, mut raked) = (Mesh::default(), Mesh::default());
        let mut base_ids: HashMap<u32, u32> = HashMap::new();
        let mut raked_ids: HashMap<(u32, u32), u32> = HashMap::new();
        let sector = TAU / 8.0;
        for t in &tris {
            let a = t.map(|i| alpha[i as usize]);
            if a.iter().any(|&x| x < 1.0) {
                for &i in t {
                    let id = *base_ids.entry(i).or_insert_with(|| {
                        let p = pts[i as usize];
                        base.vertices.push(
                            Vertex::new(
                                v3(p.q, p.y),
                                normals[i as usize].normalize_or(Vec3::Y),
                                p.q,
                            )
                            .with_color(Self::tint(
                                p.q,
                                p.t,
                                Self::wall_drift(p.q),
                            )),
                        );
                        base.vertices.len() as u32 - 1
                    });
                    base.indices.push(id);
                }
            }
            if a.iter().any(|&x| x >= TORN) {
                let c = (pts[t[0] as usize].q + pts[t[1] as usize].q + pts[t[2] as usize].q) / 3.0;
                let s = (angle(c) / sector).floor().min(7.0) as u32;
                let mid = (s as f32 + 0.5) * sector;
                for &i in t {
                    let id = *raked_ids.entry((i, s)).or_insert_with(|| {
                        let p = pts[i as usize];
                        let u = turn(p.a, mid) * p.rho + s as f32 * 1.37;
                        let pv = v3(p.q, 0.0);
                        let k = 1.04 + 0.05 * vn3(pv * 0.12, 911) + 0.02 * vn3(pv * 0.5, 913);
                        raked.vertices.push(
                            Vertex::new(
                                v3(p.q, p.y + RAKE_LIFT),
                                normals[i as usize].normalize_or(Vec3::Y),
                                Vec2::new(u, p.rho),
                            )
                            .with_color(Vec4::new(
                                k,
                                k,
                                k * 0.99,
                                alpha[i as usize],
                            )),
                        );
                        raked.vertices.len() as u32 - 1
                    });
                    raked.indices.push(id);
                }
            }
        }
        (base, raked)
    }

    /// Dust drifted round the column plinths (radius 1.14 m), deeper on the windward side,
    /// its foot tucked under the sand.
    pub fn drift_mesh(&self) -> Mesh {
        let mut m = Mesh::default();
        let (na, nr) = (40usize, 8usize);
        for (k, &c) in self.columns.iter().enumerate() {
            let seed = 950 + k as u32;
            let base = m.vertices.len() as u32;
            for i in 0..na {
                let th = i as f32 / na as f32 * TAU;
                let dir = Vec2::from_angle(th);
                let n = vn3(Vec3::new(dir.x * 1.3, k as f32 * 3.7, dir.y * 1.3), seed);
                let n2 = vn3(
                    Vec3::new(dir.x * 3.1, k as f32 * 1.9, dir.y * 3.1),
                    seed + 50,
                );
                let wind = 0.5 + 0.5 * turn(th, WIND + PI).cos();
                let width = (0.45 + 0.85 * wind) * (0.85 + 0.35 * n);
                let height = (0.03 + 0.065 * wind) * (0.8 + 0.35 * n2);
                for j in 0..=nr {
                    let (r, h) = match j {
                        0 => (1.06, height),
                        _ if j == nr => (1.14 + width, -0.012),
                        _ => {
                            let x = (j - 1) as f32 / (nr - 1) as f32;
                            (1.14 + width * x, height * (1.0 - x) * (1.0 - x))
                        }
                    };
                    let q = c + dir * r;
                    let t = self.trample(q);
                    let y = self.height(q, t) + h;
                    m.vertices
                        .push(Vertex::new(v3(q, y), Vec3::Y, q).with_color(Self::tint(
                            q,
                            t,
                            h.max(0.0) + Self::wall_drift(q),
                        )));
                }
            }
            let w = (nr + 1) as u32;
            for i in 0..na as u32 {
                let i2 = (i + 1) % na as u32;
                for j in 0..nr as u32 {
                    let (a, b) = (base + i * w + j, base + i * w + j + 1);
                    let (c2, d) = (base + i2 * w + j + 1, base + i2 * w + j);
                    // Angle grows toward +Z: (outward, around) winds counter-clockwise from
                    // above when listed a, d, c, b.
                    m.indices.extend_from_slice(&[a, d, c2, a, c2, b]);
                }
            }
        }
        fix_up(&mut m);
        smooth_normals(&mut m);
        m
    }

    /// Ribbons of sand laid over the floor along `path`, shaped by `profile` (offset, height
    /// at full depth, shade), their edges tucked under the floor and their ends sinking into
    /// it.
    fn ribbon(&self, m: &mut Mesh, path: &[Vec2], profile: &[(f32, f32, f32)], depth: f32) {
        let n = path.len();
        if n < 2 {
            return;
        }
        let mut len = vec![0.0f32];
        for w in path.windows(2) {
            len.push(len[len.len() - 1] + w[0].distance(w[1]));
        }
        let total = len[n - 1].max(1e-3);
        let base = m.vertices.len() as u32;
        let tuck = -0.012;
        for i in 0..n {
            let d = (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec2::X);
            let side = Vec2::new(-d.y, d.x);
            let s = len[i] / total;
            let f = smooth(0.0, 0.08, s) * (1.0 - smooth(0.55, 1.0, s));
            let wob = vn3(v3(path[i], 0.0) * 1.3, 961) * 0.25 + 0.75;
            for &(off, h, shade) in profile {
                let q = path[i] + side * off;
                let t = self.trample(q);
                let h = if h <= tuck { tuck } else { h * depth * wob };
                let y = self.height(q, t) + tuck + (h - tuck) * f;
                let k = 1.0 + (shade - 1.0) * f;
                let c = Self::tint(q, t, Self::wall_drift(q)) * Vec4::new(k, k, k, 1.0);
                m.vertices
                    .push(Vertex::new(v3(q, y), Vec3::Y, q).with_color(c));
            }
        }
        let w = profile.len() as u32;
        for i in 0..n as u32 - 1 {
            for j in 0..w - 1 {
                let a = base + i * w + j;
                let (b, c, dd) = (a + 1, a + w + 1, a + w);
                m.indices.extend_from_slice(&[a, b, c, a, c, dd]);
            }
        }
    }

    /// Cart ruts at the gates and drag marks toward them, in the trampled sand's material.
    pub fn track_mesh(&self) -> Mesh {
        let mut m = Mesh::default();
        // A wheel's rut: sand pushed up either side of a narrow, compacted groove (heights
        // at 1 = 1 m of depth scale).
        let rut = [
            (-0.21, -1.0, 1.0),
            (-0.14, 0.75, 1.06),
            (-0.08, 1.0, 1.04),
            (-0.045, 0.35, 0.8),
            (0.0, 0.15, 0.74),
            (0.045, 0.35, 0.8),
            (0.08, 1.0, 1.04),
            (0.14, 0.75, 1.06),
            (0.21, -1.0, 1.0),
        ];
        for (path, depth) in &self.ruts {
            self.ribbon(&mut m, path, &rut, *depth);
        }
        // A dragged body: two heel furrows and a smeared middle.
        let drag = [
            (-0.34, -1.0, 1.0),
            (-0.27, 0.8, 1.04),
            (-0.2, 0.35, 0.86),
            (-0.14, 0.85, 1.02),
            (-0.07, 0.6, 0.94),
            (0.0, 0.5, 0.92),
            (0.07, 0.6, 0.94),
            (0.14, 0.85, 1.02),
            (0.2, 0.35, 0.86),
            (0.27, 0.8, 1.04),
            (0.34, -1.0, 1.0),
        ];
        for path in &self.drags {
            self.ribbon(&mut m, path, &drag, 0.01);
        }
        fix_up(&mut m);
        smooth_normals(&mut m);
        m
    }

    /// Damp sand (soft decals, alpha in the vertex color) and puddles (flat water).
    pub fn wet_meshes(&self) -> (Mesh, Mesh) {
        let mut damp = Mesh::default();
        for (k, &(c, r, strength)) in self.damp.iter().enumerate() {
            let (na, nr) = if r > 0.5 { (36usize, 6usize) } else { (12, 2) };
            let seed = 970 + k as u32;
            let base = damp.vertices.len() as u32;
            let at = |q: Vec2| v3(q, self.ground(q) + DECAL_LIFT);
            damp.vertices.push(
                Vertex::new(at(c), Vec3::Y, c).with_color(Vec4::new(1.0, 1.0, 1.0, strength)),
            );
            for i in 0..na {
                let th = i as f32 / na as f32 * TAU;
                let dir = Vec2::from_angle(th);
                let reach = r
                    * (1.0
                        + 0.28 * vn3(Vec3::new(dir.x * 1.2, 0.0, dir.y * 1.2), seed)
                        + 0.1 * vn3(Vec3::new(dir.x * 3.5, 0.0, dir.y * 3.5), seed + 1));
                for j in 1..=nr {
                    let s = j as f32 / nr as f32;
                    let q = c + dir * reach * s;
                    let n = vn3(v3(q, 0.0) * 2.3, seed + 2);
                    let a = strength * (1.0 - s * s).powf(1.5) * (0.8 + 0.2 * n);
                    damp.vertices.push(
                        Vertex::new(at(q), Vec3::Y, q).with_color(Vec4::new(1.0, 1.0, 1.0, a)),
                    );
                }
            }
            let ring = |i: usize, j: usize| base + 1 + ((i % na) * nr + j - 1) as u32;
            for i in 0..na {
                damp.indices
                    .extend_from_slice(&[base, ring(i + 1, 1), ring(i, 1)]);
                for j in 1..nr {
                    let (a, b) = (ring(i, j), ring(i, j + 1));
                    let (c2, d) = (ring(i + 1, j + 1), ring(i + 1, j));
                    damp.indices.extend_from_slice(&[a, d, c2, a, c2, b]);
                }
            }
        }
        fix_up(&mut damp);
        smooth_normals(&mut damp);
        let mut puddles = Mesh::default();
        for (k, &(c, r)) in self.puddles.iter().enumerate() {
            let seed = 990 + k as u32;
            let n = 28;
            let outline: Vec<(Vec2, f32)> = (0..n)
                .map(|i| {
                    let dir = Vec2::from_angle(i as f32 / n as f32 * TAU);
                    let reach = r
                        * (1.0
                            + 0.3 * vn3(Vec3::new(dir.x * 1.4, 0.0, dir.y * 1.4), seed)
                            + 0.12 * vn3(Vec3::new(dir.x * 4.0, 0.0, dir.y * 4.0), seed + 1));
                    (dir, reach)
                })
                .collect();
            // Water finds its level just over the highest sand along its edge (and over the
            // damp sand decal round it).
            let level = outline
                .iter()
                .map(|&(dir, reach)| self.ground(c + dir * reach))
                .fold(self.ground(c), f32::max)
                + DECAL_LIFT
                + 0.003;
            let base = puddles.vertices.len() as u32;
            // Muddy water over wet sand: the bottom shows through, lighter toward the rim.
            let mud = Vec4::new(5.3, 3.1, 2.0, 1.0);
            let rim = Vec4::new(10.0, 6.0, 4.0, 1.0);
            puddles
                .vertices
                .push(Vertex::new(v3(c, level), Vec3::Y, c).with_color(mud));
            for &(dir, reach) in &outline {
                for (s, col) in [(0.75, mud.lerp(rim, 0.35)), (1.0, rim)] {
                    let q = c + dir * (reach * s);
                    puddles
                        .vertices
                        .push(Vertex::new(v3(q, level), Vec3::Y, q).with_color(col));
                }
            }
            for i in 0..n as u32 {
                let (a, b) = (base + 1 + 2 * i, base + 1 + 2 * ((i + 1) % n as u32));
                puddles
                    .indices
                    .extend_from_slice(&[base, b, a, a, b, b + 1, a, b + 1, a + 1]);
            }
        }
        fix_up(&mut puddles);
        (damp, puddles)
    }

    // -------------------------------------------------------------------------- litter

    /// Straw, broken arrows and splinters. Returns (straw tufts, fiber: fletching and stalks
    /// afloat, wood, iron).
    pub fn litter_meshes(&self) -> (Mesh, Mesh, Mesh, Mesh) {
        let mut rng = Rng::new(8080);
        let (mut cards, mut fiber, mut wood, mut iron) = (
            Mesh::default(),
            Mesh::default(),
            Mesh::default(),
            Mesh::default(),
        );
        let card = |m: &mut Mesh, rng: &mut Rng, q: Vec2, size: f32, kind: u32| {
            let tone = Vec3::new(1.0, rng.range(0.97, 1.02), rng.range(0.93, 1.03))
                * rng.range(0.86, 1.08);
            straw_card(m, self, q, rng.range(0.0, TAU), size, kind, tone);
        };
        // Straw: thick tufts and bale flakes where it was carried or knocked loose, looser
        // and older straw kicked round them.
        for &(c, r, n) in &self.straw {
            let mut left = n.div_ceil(5);
            for _ in 0..left * 6 {
                if left == 0 {
                    break;
                }
                let f = rng.f();
                let q = c + Vec2::from_angle(rng.range(0.0, TAU)) * (r * f.sqrt());
                if !self.free(q, 0.1) {
                    continue;
                }
                let kind = match (f < 0.4, rng.f()) {
                    (true, x) if x < 0.55 => 0,
                    (true, _) => 2,
                    (false, x) if x < 0.7 => 1,
                    _ => 3,
                };
                let size = rng.range(0.5, 0.72);
                card(&mut cards, &mut rng, q, size, kind);
                left -= 1;
            }
        }
        // Stray wisps over the trampled ground.
        let mut placed = 0;
        for _ in 0..8000 {
            if placed >= 220 {
                break;
            }
            let q = Vec2::from_angle(rng.range(0.0, TAU)) * (38.5 * rng.f().sqrt());
            if self.trample(q) < 0.5 || !self.free(q, 0.1) {
                continue;
            }
            let kind = if rng.f() < 0.55 { 1 } else { 3 };
            let size = rng.range(0.42, 0.62);
            card(&mut cards, &mut rng, q, size, kind);
            placed += 1;
        }
        // A few stalks afloat in the troughs.
        for p in &self.props {
            if let Prop::Trough(_) = p.prop {
                let y = self.ground(p.at) + TROUGH_WATER + 0.002;
                let b = Block {
                    at: p.at,
                    x: Vec2::new(p.facing.sin(), -p.facing.cos()),
                    half: Vec2::ONE,
                    wood: false,
                };
                for _ in 0..5 {
                    let q = b.world(Vec2::new(rng.signed() * 0.65, rng.signed() * 0.12));
                    straw(&mut fiber, &mut rng, q, y);
                }
            }
        }
        // Arrows round the cover (shot at whoever hid there) and over the fighting ground.
        let mut spots: Vec<(Vec2, f32, u32)> = self
            .cover
            .iter()
            .map(|b| (b.at, b.half.max_element() + 2.6, 8))
            .collect();
        spots.extend(
            self.spots
                .iter()
                .filter(|s| s.2 >= 1.0 && s.1 >= 4.0 && s.0.length() < 30.0)
                .map(|&(c, r, _)| (c, r * 0.8, 7)),
        );
        for (c, r, n) in spots {
            let mut left = n;
            for _ in 0..n * 8 {
                if left == 0 {
                    break;
                }
                let q = c + Vec2::from_angle(rng.range(0.0, TAU)) * (r * rng.f().sqrt());
                if !self.free(q, 0.12) {
                    continue;
                }
                arrow(&mut fiber, &mut wood, &mut iron, &mut rng, q, self.top(q));
                left -= 1;
            }
        }
        // Splinters and broken boards round the timber barricades and crates.
        for b in self.cover.iter().filter(|b| b.wood) {
            let mut left = 34;
            for _ in 0..300 {
                if left == 0 {
                    break;
                }
                let l = Vec2::new(
                    rng.signed() * (b.half.x + 0.9),
                    rng.signed() * (b.half.y + 0.9),
                );
                let q = b.world(l);
                if !self.free(q, 0.02) {
                    continue;
                }
                let near = 1.0 - smooth(0.0, 0.9, (l.abs() - b.half).max_element());
                if rng.f() > 0.25 + 0.75 * near {
                    continue;
                }
                let size = if rng.f() < 0.12 { 2.5 } else { 1.0 };
                shard(&mut wood, &mut rng, q, self.top(q), size);
                left -= 1;
            }
        }
        fix_up(&mut cards);
        (cards, fiber, wood, iron)
    }
}

/// Turn every triangle of a floor-hugging mesh to face up.
fn fix_up(m: &mut Mesh) {
    let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
    let flips: Vec<usize> = m
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, t)| (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).y < 0.0)
        .map(|(k, _)| k)
        .collect();
    for k in flips {
        m.indices.swap(k * 3 + 1, k * 3 + 2);
    }
}

/// One straw stalk lying at `q` (sand at height `y`): a thin, slightly bent strip, one end
/// propped on others.
fn straw(m: &mut Mesh, rng: &mut Rng, q: Vec2, y: f32) {
    let len = rng.range(0.07, 0.3);
    let w = rng.range(0.006, 0.011);
    let dir = Vec2::from_angle(rng.range(0.0, TAU));
    let side = Vec2::new(-dir.y, dir.x);
    let lift = rng.range(0.0, 0.012);
    let bow = rng.signed() * 0.012;
    let v = rng.range(0.75, 1.15);
    let old = rng.f();
    let c = mix(
        Vec3::new(0.82, 0.6, 0.2),
        Vec3::new(0.42, 0.37, 0.25),
        old * old * old,
    ) * v;
    let pts = [q, q + dir * (len * 0.5) + side * bow, q + dir * len];
    let ys = [y + 0.0015, y + 0.002 + lift * 0.4, y + 0.002 + lift];
    let base = m.vertices.len() as u32;
    for k in 0..3 {
        for s in [-1.0f32, 1.0] {
            let p = pts[k] + side * (s * w * 0.5);
            m.vertices.push(
                Vertex::new(v3(p, ys[k]), Vec3::Y, Vec2::new(s, k as f32))
                    .with_color((c * (0.9 + 0.08 * k as f32)).extend(1.0)),
            );
        }
    }
    for k in 0..2 {
        let a = base + k * 2;
        m.indices
            .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
}

/// An arrow shaft from `a` to `b` (5 sides), optionally splintered at `b`.
fn shaft(wood: &mut Mesh, a: Vec3, b: Vec3, color: Vec4, broken: bool, rng: &mut Rng) {
    let r = 0.0045;
    let mut s = tube(&[a, b], &|_| r, 5, false);
    if broken {
        let d = (b - a).normalize_or(Vec3::X);
        let kink = d.any_orthonormal_vector() * rng.signed() * 0.003;
        s.merge(&tube(
            &[b, b + d * rng.range(0.012, 0.03) + kink],
            &|t| r * (1.0 - t),
            5,
            false,
        ));
    }
    swap_uv(&mut s);
    offset_uv(&mut s, Vec2::new(rng.range(0.0, 2.0), rng.range(0.0, 2.0)));
    tint(&mut s, color);
    wood.merge(&s);
}

/// Three feather vanes at the nock `n` of a shaft pointing along `d`.
fn fletching(fiber: &mut Mesh, n: Vec3, d: Vec3, color: Vec3, rng: &mut Rng) {
    let u = d.any_orthonormal_vector();
    let roll = rng.range(0.0, TAU);
    for k in 0..3 {
        let r = Quat::from_axis_angle(d, roll + k as f32 * TAU / 3.0) * u;
        let tone = if k == 0 { 0.75 } else { 1.0 };
        let c = (color * tone * rng.range(0.9, 1.1)).extend(1.0);
        let p = [
            n + d * 0.015 + r * 0.004,
            n + d * 0.13 + r * 0.004,
            n + d * 0.105 + r * 0.016,
            n + d * 0.03 + r * 0.018,
        ];
        let inside = n + d * 0.07 + r * 0.01 - d.cross(r) * 0.01;
        facet(fiber, [p[0], p[1], p[2]], inside, c, |_| Vec2::ZERO);
        facet(fiber, [p[0], p[2], p[3]], inside, c, |_| Vec2::ZERO);
    }
}

/// A forged bodkin head with its base at `a`, pointing along `d`.
fn head(iron: &mut Mesh, a: Vec3, d: Vec3) {
    let u = d.any_orthonormal_vector() * 0.0065;
    let v = d.cross(u);
    let tip = a + d * 0.045;
    let corners = [a + u, a + v, a - u, a - v];
    let inside = a + d * 0.015;
    for k in 0..4 {
        facet(
            iron,
            [corners[k], corners[(k + 1) % 4], tip],
            inside,
            Vec4::ONE,
            |p| Vec2::new(p.x + p.z, p.y),
        );
    }
}

/// An arrow, whole or in pieces, on the sand at `q` (height `y`): stuck where it landed,
/// snapped and trodden flat, or lying whole.
fn arrow(fiber: &mut Mesh, wood: &mut Mesh, iron: &mut Mesh, rng: &mut Rng, q: Vec2, y: f32) {
    let yaw = Vec2::from_angle(rng.range(0.0, TAU));
    let flat = v3(yaw, 0.0);
    let ash = Vec4::new(2.2, 2.05, 1.8, 1.0) * rng.range(0.85, 1.1);
    let feather = [
        Vec3::new(0.78, 0.76, 0.7),
        Vec3::new(0.22, 0.2, 0.18),
        Vec3::new(0.5, 0.38, 0.26),
        Vec3::new(0.62, 0.6, 0.55),
    ][(rng.f() * 4.0) as usize % 4];
    let kind = rng.f();
    let ground = v3(q, y);
    if kind < 0.4 {
        // Stuck in the sand, tail up toward where it came from.
        let pitch = rng.range(0.35, 0.8);
        let d = (flat * pitch.cos() + Vec3::Y * pitch.sin()).normalize();
        let snapped = rng.f() < 0.35;
        let len = if snapped {
            rng.range(0.15, 0.3)
        } else {
            rng.range(0.38, 0.6)
        };
        let a = ground - d * 0.03;
        let b = ground + d * len;
        shaft(wood, a, b, ash, snapped, rng);
        if !snapped {
            fletching(fiber, b, -d, feather, rng);
        }
    } else if kind < 0.8 {
        // A snapped piece, trodden flat: the fletched end, the head end or a middle bit.
        let len = rng.range(0.12, 0.42);
        let lift = Vec3::Y * 0.0045;
        let tilt = Vec3::Y * rng.range(0.0, 0.015);
        let a = ground + lift;
        let b = ground + flat * len + lift + tilt;
        let d = (b - a).normalize();
        let end = rng.f();
        shaft(wood, a, b, ash, true, rng);
        if end < 0.5 {
            fletching(fiber, a, d, feather, rng);
        } else if end < 0.8 {
            head(iron, a, -d);
        }
    } else {
        // Whole, lying where it skidded.
        let a = ground + Vec3::Y * 0.0045;
        let b = a + flat * 0.7 + Vec3::Y * rng.range(0.0, 0.012);
        let d = (b - a).normalize();
        shaft(wood, a, b, ash, false, rng);
        fletching(fiber, a, d, feather, rng);
        head(iron, b, d);
    }
}

/// A splinter (or, at `size` 2.5, a chunk of broken board) lying at `q`.
fn shard(wood: &mut Mesh, rng: &mut Rng, q: Vec2, y: f32, size: f32) {
    let len = rng.range(0.03, 0.16) * size;
    let w = len * rng.range(0.12, 0.3);
    let th = w * rng.range(0.25, 0.5);
    let dir2 = Vec2::from_angle(rng.range(0.0, TAU));
    let dir = v3(dir2, 0.0);
    let side = Vec3::new(-dir2.y, 0.0, dir2.x);
    let base = v3(q, y + 0.001);
    let t1 = base + dir * len + Vec3::Y * rng.range(0.0, 0.01);
    let mid = base + dir * (len * rng.range(0.3, 0.6));
    let ring = [
        mid + side * (w * 0.5),
        mid + Vec3::Y * th,
        mid - side * (w * 0.5),
    ];
    let fresh = rng.f() < 0.6;
    let c = if fresh {
        Vec4::new(2.3, 2.05, 1.7, 1.0)
    } else {
        Vec4::new(1.1, 1.05, 1.0, 1.0)
    } * rng.range(0.85, 1.1);
    let inside = mid + Vec3::Y * (th * 0.3);
    let off = Vec2::new(rng.range(0.0, 2.0), rng.range(0.0, 2.0));
    let uv = |p: Vec3| Vec2::new((p - base).dot(dir), (p - base).dot(side) + p.y) + off;
    for k in 0..2 {
        facet(wood, [base, ring[k], ring[k + 1]], inside, c, uv);
        facet(wood, [t1, ring[k + 1], ring[k]], inside, c, uv);
    }
}

// ================================================================================ textures

/// Freshly raked sand (3 m repeat, grooves along u): lanes of tine grooves a rake pass wide,
/// sand pushed up where passes meet, wavering lines, skipped tines and dragged pebbles. The
/// alpha tears the raked layer open toward trampled ground: clumps, and a few tracks of
/// footprints that show only where the layer's vertex alpha is below one.
pub fn raked_set() -> TextureSet {
    let size = span::RAKED;
    let marks = marks(size, 505, 16, 6, 0);
    let cs = size / MARK_CELLS as f32;
    let lanes = 5;
    let tines = 14.0;
    let px = 512.0;
    bake_par_alpha(512, size, |uv| {
        let m = uv * size;
        let lane_f = uv.y * lanes as f32;
        let li = (lane_f.floor() as i32).rem_euclid(lanes);
        let lt = lane_f - lane_f.floor();
        let ls = li as u32 * 7 + 3;
        // The rake wavers along its pass; each pass starts its tines somewhere else.
        let wob = fb(Vec2::new(uv.x * 3.0, li as f32 * 1.7), (3, 0), 2, 600 + ls) * 0.12
            + pn(Vec2::new(uv.x * 23.0, li as f32), (23, 0), 610 + ls) * 0.05;
        let g = lt * tines + wob + r2(li, 0, 611) * 0.9;
        let gi = g.floor();
        let tine = gi as i32 + li * 31;
        // Tines bent a little out of line, some worn shorter than others.
        let gf = g - gi + (r2(tine, 1, 612) - 0.5) * 0.24;
        let width = 0.2 + 0.08 * r2(tine, 2, 613);
        let groove = (-((gf - 0.5) / width).powi(2)).exp();
        let lumps = 0.55 + 0.45 * fb(Vec2::new(uv.x * 40.0, g * 0.5), (40, 0), 2, 614);
        let crest = (-(gf.min(1.0 - gf) / 0.17).powi(2)).exp() * lumps;
        // Now and then a tine skips or rides up.
        let skip = smooth(
            0.3,
            0.55,
            pn(Vec2::new(uv.x * 5.0, tine as f32 * 0.61), (5, 0), 620),
        );
        let depth = 0.0027 * (0.6 + 0.6 * r2(tine, 3, 615)) * (1.0 - 0.75 * skip);
        // Where passes meet: a low ridge, the grooves fading into it.
        let e = lt.min(1.0 - lt) * size / lanes as f32;
        let ridge = (-(e / 0.025).powi(2)).exp();
        let fade = smooth(0.0, 0.05, e);
        let low = fb(uv * 4.0, (4, 4), 3, 630);
        let fine = fb(uv * 60.0, (60, 60), 2, 631);
        let grains = fb(uv * 160.0, (160, 160), 1, 635);
        let (ix, iy) = ((uv.x * px) as i32, (uv.y * px) as i32);
        let speck = rand3(ix, iy, 0, 632);
        // Pebbles the tines dragged, each at the end of the furrow it ploughed.
        let pc = cell(uv * 30.0, (30, 30), 633);
        let pebble = if pc.id < 0.06 {
            let r = 0.12 + 0.12 * pc.id / 0.06;
            let bump = 1.0 - smooth(r * 0.5, r, pc.d1);
            let furrow = if pc.rel.x < 0.0 && pc.rel.x > -1.2 {
                (-(pc.rel.y / (r * 0.6)).powi(2)).exp() * smooth(-1.2, -0.2, pc.rel.x)
            } else {
                0.0
            };
            (bump, furrow)
        } else {
            (0.0, 0.0)
        };
        let height = -depth * groove * fade
            + 0.0011 * crest * fade
            + 0.0022 * ridge
            + 0.0012 * low
            + 0.0003 * fine
            + 0.00035 * grains
            + 0.0002 * (speck - 0.5)
            + 0.003 * pebble.0
            - 0.0018 * pebble.1;
        let mut a = mix(hex(0xcdb185), hex(0xdcc49b), smooth(-0.3, 0.6, low));
        a *= 1.0 - 0.1 * groove * fade + 0.04 * crest * fade + 0.05 * ridge;
        a *= 0.95 + 0.1 * rand3(ix / 2, iy / 2, 1, 634);
        // Each pass of the rake turned the sand a little differently.
        a *= 0.95 + 0.1 * r2(li, 5, 616);
        if speck > 0.988 {
            a = mix(a, hex(0x6b5a48), 0.6);
        } else if speck < 0.008 {
            a = mix(a, hex(0xf2ead8), 0.6);
        }
        a = mix(a, hex(0x7a5d48) * (0.7 + speck * 0.6), pebble.0 * 0.85);
        a *= 1.0 - 0.08 * pebble.1;
        // The tear: clumps kicked up, and footprints walked through the lanes.
        let (cx, cy) = (
            (m.x / cs).floor() as i32 % MARK_CELLS,
            (m.y / cs).floor() as i32 % MARK_CELLS,
        );
        let mut print = 0.0f32;
        for &i in &marks.grid[(cy.max(0) * MARK_CELLS + cx.max(0)) as usize] {
            let mk = &marks.marks[i as usize];
            if mk.kind == 1 {
                continue;
            }
            let (_, inside, _) = mark_effect(mk, wrap_delta(m - mk.at, size));
            print = print.max(smooth(0.2, 0.6, inside));
        }
        // Prints tear through first (vertex alpha just under one), clumps further in.
        let clumps = smooth(-0.35, 0.55, fb(uv * 18.0, (18, 18), 3, 640));
        let alpha = 1.0 - 0.5 * print.max(clumps * 0.8);
        (
            Texel {
                albedo: a,
                height,
                roughness: (0.97 - 0.15 * pebble.0).clamp(0.5, 1.0),
                metallic: 0.0,
                occlusion: (1.0 - 0.25 * groove * fade - 0.1 * pebble.1).clamp(0.5, 1.0),
            },
            alpha,
        )
    })
}

// ================================================================================== straw

/// Meters the straw atlas spans; each quadrant is one tuft card.
const STRAW_SPAN: f32 = 1.2;
/// Radius (atlas units) a tuft keeps inside its quadrant's middle, clear of its neighbors
/// down the mip chain.
const TUFT: f32 = 0.22;

/// A stalk in the straw atlas (atlas units, 0..1).
struct Stalk {
    pts: [Vec2; 3],
    /// Half its width.
    half: f32,
    color: Vec3,
    /// 0 round, 1 split open and pressed flat.
    flat: f32,
    /// Length in meters, and where along it the first node sits (0..1 of a node spacing).
    len: f32,
    node: f32,
    age: f32,
}

impl Stalk {
    /// Distance from `p` to the stalk's middle line (signed: + left of it) and how far along
    /// it the nearest point is (0..1).
    fn at(&self, p: Vec2) -> (f32, f32) {
        let mut best = (f32::MAX, 0.0);
        for k in 0..2 {
            let (a, b) = (self.pts[k], self.pts[k + 1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            let r = p - (a + ab * t);
            let d = r.length();
            if d < best.0.abs() {
                let side = if ab.perp_dot(r) >= 0.0 { 1.0 } else { -1.0 };
                best = (d * side, (k as f32 + t) * 0.5);
            }
        }
        best
    }

    /// Half width at `t` along it: the ends taper a little.
    fn half_at(&self, t: f32) -> f32 {
        self.half * (1.0 - 0.3 * smooth(0.82, 1.0, (2.0 * t - 1.0).abs()))
    }
}

/// The straw the atlas holds, bottom to top. Quadrants: a thick tuft, a loose scatter, a
/// flake dropped off a bale (stalks lying roughly parallel) and old straw, broken short and
/// trodden flat. Chaff (bits a centimeter or three long) lies among each.
fn stalks() -> Vec<Stalk> {
    let mut rng = Rng::new(7070);
    let mut out = Vec::new();
    let per_m = 1.0 / STRAW_SPAN;
    for k in 0..4 {
        let center = Vec2::new((k % 2) as f32 * 0.5 + 0.25, (k / 2) as f32 * 0.5 + 0.25);
        let (count, chaff) = [(80, 50), (26, 18), (56, 24), (40, 70)][k];
        let axis = Vec2::from_angle(rng.range(0.0, TAU));
        let across = Vec2::new(-axis.y, axis.x);
        for i in 0..count + chaff {
            let bit = i >= count;
            let (mid, dir, len) = match k {
                0 => (
                    center
                        + Vec2::from_angle(rng.range(0.0, TAU)) * (TUFT * 0.6 * rng.f().powf(0.7)),
                    Vec2::from_angle(rng.range(0.0, TAU)),
                    rng.range(0.06, 0.24),
                ),
                1 => (
                    center + Vec2::from_angle(rng.range(0.0, TAU)) * (TUFT * 0.8 * rng.f().sqrt()),
                    Vec2::from_angle(rng.range(0.0, TAU)),
                    rng.range(0.1, 0.28),
                ),
                2 => (
                    center
                        + across * (rng.signed() * 0.1 * (1.0 + 0.3 * rng.f()))
                        + axis * (rng.signed() * 0.07),
                    Vec2::from_angle(axis.to_angle() + rng.signed() * 0.4),
                    rng.range(0.1, 0.26),
                ),
                _ => (
                    center + Vec2::from_angle(rng.range(0.0, TAU)) * (TUFT * 0.85 * rng.f().sqrt()),
                    Vec2::from_angle(rng.range(0.0, TAU)),
                    rng.range(0.04, 0.15),
                ),
            };
            let (dir, mut len) = if bit {
                (
                    Vec2::from_angle(rng.range(0.0, TAU)),
                    rng.range(0.008, 0.035),
                )
            } else {
                (dir, len)
            };
            let bow = rng.signed() * 0.08;
            let old = match k {
                3 => 0.45 + 0.55 * rng.f(),
                _ => rng.f().powi(3) * 0.8,
            };
            let flat = if k == 3 || rng.f() < 0.15 {
                rng.range(0.5, 1.0)
            } else {
                0.0
            };
            let width = if bit {
                rng.range(0.004, 0.007)
            } else {
                rng.range(0.006, 0.009) * (1.0 + 0.45 * flat)
            };
            // Shorten the stalk until it fits its tuft (or drop it).
            let mut pts = None;
            for _ in 0..5 {
                let l = len * per_m;
                let side = Vec2::new(-dir.y, dir.x) * (bow * l);
                let p = [mid - dir * (l * 0.5), mid + side, mid + dir * (l * 0.5)];
                if p.iter().all(|p| p.distance(center) < TUFT) {
                    pts = Some(p);
                    break;
                }
                len *= 0.7;
            }
            let Some(pts) = pts else { continue };
            let fresh = mix(hex(0xebc977), hex(0xf7ecc8), rng.f().powi(2));
            // Weathered straw bleaches toward a silvery beige.
            let aged = mix(hex(0xd8bf8c), hex(0xcbc0a6), rng.f());
            out.push(Stalk {
                pts,
                half: width * 0.5 * per_m,
                color: mix(fresh, aged, old) * rng.range(0.86, 1.08),
                flat,
                len,
                node: rng.f(),
                age: old,
            });
        }
    }
    out
}

/// Straw tufts on transparent ground (alpha cut at 0.5), four to the atlas; the alpha keeps
/// its coverage down the mip chain so distant tufts don't thin away.
pub fn straw_set() -> TextureSet {
    const SIZE: u32 = 1024;
    const GRID: usize = 128;
    let stalks = stalks();
    let px = 1.0 / SIZE as f32;
    let per_m = 1.0 / STRAW_SPAN;
    // Shade reaches this far past a stalk's edge onto the straw beneath.
    let reach = 0.004 * per_m;
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); GRID * GRID];
    // Each stalk's bounds with its shade, to skip it cheaply.
    let mut boxes = Vec::with_capacity(stalks.len());
    for (i, s) in stalks.iter().enumerate() {
        let pad = s.half + reach + 2.0 * px;
        let lo = s.pts.iter().fold(Vec2::splat(f32::MAX), |m, p| m.min(*p)) - pad;
        let hi = s.pts.iter().fold(Vec2::splat(f32::MIN), |m, p| m.max(*p)) + pad;
        boxes.push((lo, hi));
        let cell = |v: f32| ((v * GRID as f32) as usize).min(GRID - 1);
        for y in cell(lo.y.max(0.0))..=cell(hi.y) {
            for x in cell(lo.x.max(0.0))..=cell(hi.x) {
                grid[y * GRID + x].push(i as u32);
            }
        }
    }
    let average = hex(0xc8ae7c);
    let mut set = bake_par_alpha(SIZE, STRAW_SPAN, |uv| {
        let c = (
            ((uv.x * GRID as f32) as usize).min(GRID - 1),
            ((uv.y * GRID as f32) as usize).min(GRID - 1),
        );
        let list = &grid[c.1 * GRID + c.0];
        // The topmost stalk under this texel.
        let mut top: Option<(usize, f32, f32, f32)> = None;
        let mut alpha = 0.0f32;
        let outside = |i: u32| {
            let (lo, hi) = boxes[i as usize];
            uv.cmplt(lo).any() || uv.cmpgt(hi).any()
        };
        for &i in list {
            if outside(i) {
                continue;
            }
            let s = &stalks[i as usize];
            let (d, t) = s.at(uv);
            let h = s.half_at(t);
            let cover = smooth(h + 0.7 * px, h - 0.7 * px, d.abs());
            if cover > 0.0 {
                alpha = alpha.max(cover);
                top = Some((i as usize, d, t, h));
            }
        }
        let Some((i, d, t, h)) = top else {
            return (
                Texel {
                    albedo: average,
                    height: 0.0,
                    roughness: 0.6,
                    metallic: 0.0,
                    occlusion: 1.0,
                },
                0.0,
            );
        };
        let s = &stalks[i];
        let x = (d / h).clamp(-1.0, 1.0);
        let round = (1.0 - x * x).max(0.0).sqrt();
        let along = t * s.len;
        // Nodes every ten-odd centimeters: a darker, swollen band.
        let n = along / 0.11 + s.node;
        let node = (-((n - n.round()) * 0.11 / 0.0025).powi(2)).exp();
        // Fibers running its length; the broken ends frayed and darker.
        let fiber = pn(
            Vec2::new(x * 3.0 + i as f32 * 7.3, along * 70.0),
            (0, 0),
            650,
        );
        let end = 1.0 - smooth(0.0, 0.006, along.min(s.len - along));
        let mut a = s.color * (0.92 + 0.16 * fiber);
        // Split stalks show their paler, matte inner face.
        a = mix(
            a,
            hex(0xeedfb4) * (0.85 + 0.15 * (1.0 - s.age)),
            s.flat * 0.55 * round,
        );
        a *= 1.0 - 0.25 * node;
        a *= 1.0 - 0.25 * end;
        // Rounded edges fall off into shade.
        a *= 0.62 + 0.38 * round.powf(0.6 - 0.4 * s.flat);
        // Shade from stalks lying across it.
        let mut occlusion = 1.0f32;
        for &j in list.iter().filter(|&&j| j as usize > i && !outside(j)) {
            let o = &stalks[j as usize];
            let (dj, tj) = o.at(uv);
            let gap = dj.abs() - o.half_at(tj);
            occlusion = occlusion.min(0.55 + 0.45 * smooth(0.0, reach, gap));
        }
        let rank = i as f32 / stalks.len() as f32;
        let thick = h / per_m * (0.45 - 0.3 * s.flat);
        let height = 0.002 * rank + thick * round + 0.0006 * node - 0.0004 * end;
        (
            Texel {
                albedo: a * (0.7 + 0.3 * occlusion),
                height,
                roughness: (0.6 + 0.2 * (1.0 - round) + 0.15 * s.age + 0.1 * s.flat).min(0.95),
                metallic: 0.0,
                occlusion,
            },
            alpha,
        )
    });
    keep_coverage(&mut set.albedo, 0.5);
    set
}

/// Scale each mip level's alpha so as many texels pass the mask's `cut` as at full size
/// (box-filtered alpha otherwise thins fine cut-outs away in the distance).
fn keep_coverage(image: &mut Image, cut: f32) {
    let cut = (cut * 255.0) as u32;
    let covered = |px: &[u8], scale: u32| {
        px.as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] as u32 * scale >= cut * 256)
            .count() as f32
            / (px.len() / 4).max(1) as f32
    };
    let want = covered(&image.data, 256);
    for level in image.mips.iter_mut() {
        // Scale in 1/256ths: the smallest that covers as much as level 0.
        let (mut lo, mut hi) = (256u32, 256 * 16);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if covered(level, mid) >= want {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        for p in level.as_chunks_mut::<4>().0 {
            p[3] = (p[3] as u32 * lo / 256).min(255) as u8;
        }
    }
}

/// A card of straw (one tuft of the atlas, `kind` 0..4) at `q`, turned by `turn` and
/// `size` meters across, laid over the sand on a 3×3 grid so it follows drifts and ruts.
fn straw_card(m: &mut Mesh, g: &Grounds, q: Vec2, turn: f32, size: f32, kind: u32, tone: Vec3) {
    let x = Vec2::from_angle(turn) * (size * 0.5);
    let z = Vec2::new(-x.y, x.x);
    let corner = Vec2::new((kind % 2) as f32 * 0.5, (kind / 2) as f32 * 0.5);
    let base = m.vertices.len() as u32;
    for j in 0..3 {
        for i in 0..3 {
            let (u, v) = (i as f32 - 1.0, j as f32 - 1.0);
            let p = q + x * u + z * v;
            // Over the raked layer even where it is torn: no stalk may sink into it.
            let y = g.ground(p) + 0.003 + RAKE_LIFT + 0.0025;
            m.vertices.push(
                Vertex::new(
                    v3(p, y),
                    Vec3::Y,
                    corner + Vec2::new(i as f32, j as f32) * 0.25,
                )
                .with_color(tone.extend(1.0)),
            );
        }
    }
    for j in 0..2 {
        for i in 0..2 {
            let a = base + j * 3 + i;
            m.indices
                .extend_from_slice(&[a, a + 3, a + 1, a + 1, a + 3, a + 4]);
        }
    }
}

// ================================================================================== props

/// Water level in a trough above its base, meters.
pub const TROUGH_WATER: f32 = 0.47;

/// A stone water trough (origin on the floor at its middle, long side along X, back toward
/// +Z): dressed slabs on a bottom stone, iron cramps across the joints, water a few
/// centimeters below the rim and a dark tide line. Returns (stone, iron, water).
pub fn trough_meshes(seed: u64) -> (Mesh, Mesh, Mesh) {
    let mut ms = Mason::new(seed, Tone::Sand);
    ms.wobble = 0.002;
    let (hx, hz, top, wall) = (0.95f32, 0.34f32, 0.56f32, 0.11f32);
    let face = sb(0.03).step(0.08).skip(NEG_Y);
    ms.block(
        Vec3::new(-hx - 0.03, -0.2, -hz - 0.03),
        Vec3::new(hx + 0.03, 0.1, hz + 0.03),
        sb(0.03).skip(NEG_Y),
    );
    for s in [-1.0f32, 1.0] {
        let (z0, z1) = if s < 0.0 {
            (-hz, -hz + wall)
        } else {
            (hz - wall, hz)
        };
        ms.block(Vec3::new(-hx, 0.1, z0), Vec3::new(hx, top, z1), face);
        let (x0, x1) = if s < 0.0 {
            (-hx, -hx + wall)
        } else {
            (hx - wall, hx)
        };
        ms.block(
            Vec3::new(x0, 0.1, -hz + wall),
            Vec3::new(x1, top - 0.006, hz - wall),
            face,
        );
    }
    let (ix, iz) = (hx - wall + 0.01, hz - wall + 0.01);
    for v in &mut ms.mesh.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let inside = p.x.abs() < ix && p.z.abs() < iz && p.y > 0.12;
        let mut k = Vec3::ONE;
        if inside {
            // Tide line: algae and lime where the water stands, splashed stone above it.
            let tide = smooth(TROUGH_WATER - 0.03, TROUGH_WATER, p.y)
                * (1.0 - smooth(TROUGH_WATER + 0.02, TROUGH_WATER + 0.07, p.y));
            k *= Vec3::ONE.lerp(Vec3::new(0.45, 0.5, 0.36), tide);
            k *= 0.88 + 0.12 * smooth(TROUGH_WATER, top, p.y);
        } else {
            // Damp and grimy near the sand; tops worn pale by hands and buckets.
            k *= 0.68 + 0.32 * smooth(0.0, 0.22, p.y);
            if n.y > 0.7 && p.y > top - 0.03 {
                k *= 1.07;
            }
        }
        let c = Vec4::from(v.color);
        v.color = (c.truncate() * k).extend(c.w).to_array();
    }
    let mut iron = Mesh::default();
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let mut b = rounded_box(Vec3::new(0.02, 0.005, 0.045), BoxOpts::round(0.004));
        offset_uv(&mut b, Vec2::new(sx * 0.3, sz * 0.7));
        put(
            &mut iron,
            &b,
            Vec3::new(sx * (hx - wall * 0.5), top + 0.002, sz * (hz - wall)),
        );
    }
    let (wx, wz) = (hx - wall + 0.03, hz - wall + 0.03);
    let water = Mesh::new(
        [(-wx, -wz), (wx, -wz), (wx, wz), (-wx, wz)]
            .iter()
            .map(|&(x, z)| Vertex::new(Vec3::new(x, TROUGH_WATER, z), Vec3::Y, Vec2::new(x, z)))
            .collect(),
        vec![0, 2, 1, 0, 3, 2],
    );
    (ms.mesh, iron, water)
}

/// A coopered water cask standing open (origin on the floor at its middle): bellied oak
/// staves with dark joints, three iron hoops, water near the brim. Returns (wood, iron,
/// water).
pub fn cask_meshes(seed: u64) -> (Mesh, Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let (h, re, rb) = (0.92f32, 0.29f32, 0.335f32);
    let radius = move |y: f32| {
        let t = (y / h).clamp(0.0, 1.0) * 2.0 - 1.0;
        re + (rb - re) * (1.0 - t * t)
    };
    let staves = 16;
    let offsets: Vec<f32> = (0..staves).map(|_| rng.signed() * 0.0018).collect();
    let tints: Vec<f32> = (0..staves).map(|_| rng.range(0.78, 1.02)).collect();
    let outside: Vec<Vec2> = (0..=9)
        .map(|k| {
            let y = -0.04 + (h + 0.04) * k as f32 / 9.0;
            Vec2::new(radius(y), y)
        })
        .collect();
    let (rt, wall) = (radius(h), 0.024);
    let rim = [Vec2::new(rt, h), Vec2::new(rt - wall, h)];
    let inner = [
        Vec2::new(rt - wall, h),
        Vec2::new(rt - wall - 0.004, h - 0.1),
    ];
    let off2 = offsets.clone();
    let stave = move |a: f32, _y: f32, r: f32| {
        let f = a / TAU * staves as f32;
        let i = (f.floor() as usize) % staves;
        let fr = f - f.floor();
        let joint = (-(fr.min(1.0 - fr) / 0.06).powi(2)).exp();
        r - 0.003 * joint + off2[i]
    };
    let mut wood = lathe(&[&outside, &rim, &inner], 48, rb, &stave);
    swap_uv(&mut wood);
    for v in &mut wood.vertices {
        let p = Vec3::from(v.position);
        let a = (-p.z).atan2(p.x).rem_euclid(TAU);
        let f = a / TAU * staves as f32;
        let fr = f - f.floor();
        let joint = (-(fr.min(1.0 - fr) / 0.08).powi(2)).exp();
        let k = tints[(f.floor() as usize) % staves] * (1.0 - 0.4 * joint);
        // Wet and dark at the foot, bleached toward the rim.
        let k = k * (0.75 + 0.25 * smooth(0.0, 0.25, p.y)) * (1.0 + 0.08 * smooth(0.6, h, p.y));
        v.color = [k, k * 0.97, k * 0.93, 1.0];
    }
    let mut iron = Mesh::default();
    for y0 in [0.07f32, 0.4, 0.78] {
        let y1 = y0 + 0.045;
        let band = [
            Vec2::new(radius(y0) - 0.001, y0),
            Vec2::new(radius(y0) + 0.004, y0 + 0.004),
            Vec2::new(radius(y1) + 0.004, y1 - 0.004),
            Vec2::new(radius(y1) - 0.001, y1),
        ];
        let mut b = lathe(&[&band], 48, rb, &|_, _, r| r);
        offset_uv(&mut b, Vec2::new(0.0, y0 * 3.0));
        iron.merge(&b);
    }
    let rw = rt - wall + 0.006;
    let mut water = Mesh::default();
    water.vertices.push(Vertex::new(
        Vec3::new(0.0, h - 0.06, 0.0),
        Vec3::Y,
        Vec2::ZERO,
    ));
    for k in 0..24 {
        let a = k as f32 / 24.0 * TAU;
        let p = Vec3::new(a.cos() * rw, h - 0.06, a.sin() * rw);
        water
            .vertices
            .push(Vertex::new(p, Vec3::Y, Vec2::new(p.x, p.z)));
        water
            .indices
            .extend_from_slice(&[0, 1 + (k + 1) % 24, 1 + k]);
    }
    fix_up(&mut water);
    (wood, iron, water)
}

/// A weapon rack (origin on the floor at its middle, back toward +Z, 1.9 m wide): two
/// posts on feet, a top rail and a drilled foot board, spears and wooden practice swords
/// leaning in it and round shields propped against it. Returns (wood, iron, leather).
pub fn rack_meshes(seed: u64) -> (Mesh, Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let (mut wood, mut iron, mut leather) = (Mesh::default(), Mesh::default(), Mesh::default());
    for s in [-1.0f32, 1.0] {
        let x = s * 0.88;
        put(
            &mut wood,
            &post(&mut rng, Vec3::new(0.09, 1.52, 0.09), 0.012),
            Vec3::new(x, 0.73, 0.14),
        );
        put(
            &mut wood,
            &beam_z(&mut rng, Vec3::new(0.1, 0.08, 0.64), 0.012),
            Vec3::new(x, 0.01, 0.06),
        );
        // A brace from the foot up the back of the post.
        let mut brace = beam_z(&mut rng, Vec3::new(0.05, 0.05, 0.42), 0.008);
        brace = transformed(&brace, Mat4::from_rotation_x(-0.75));
        put(&mut wood, &brace, Vec3::new(x, 0.2, 0.28));
    }
    put(
        &mut wood,
        &plank(&mut rng, Vec3::new(1.92, 0.08, 0.08), 0.012),
        Vec3::new(0.0, 1.34, 0.14),
    );
    put(
        &mut wood,
        &plank(&mut rng, Vec3::new(1.78, 0.05, 0.26), 0.01),
        Vec3::new(0.0, 0.24, 0.02),
    );
    // Pegs on the top rail.
    for k in 0..5 {
        let x = -0.7 + 0.35 * k as f32;
        let mut peg = tube(
            &[Vec3::new(x, 1.33, 0.1), Vec3::new(x, 1.35, -0.02)],
            &|_| 0.012,
            6,
            true,
        );
        tint(&mut peg, Vec4::new(0.8, 0.75, 0.7, 1.0));
        wood.merge(&peg);
    }
    let variant = seed % 2;
    // Spears standing in the foot board, leaning on the top rail.
    let spears: &[f32] = if variant == 0 {
        &[-0.62, -0.38, -0.14, 0.52]
    } else {
        &[-0.55, 0.2, 0.42, 0.66]
    };
    for &x in spears {
        let foot = Vec3::new(x, 0.0, -0.17);
        let top = Vec3::new(x + rng.signed() * 0.05, 2.12 + rng.signed() * 0.06, 0.17);
        let d = (top - foot).normalize();
        let mut s = tube(&[foot, top], &|t| 0.016 - 0.003 * t, 6, false);
        swap_uv(&mut s);
        offset_uv(&mut s, Vec2::new(rng.range(0.0, 2.0), 0.0));
        let v = rng.range(0.8, 1.0);
        tint(&mut s, Vec4::new(v, v * 0.95, v * 0.88, 1.0));
        wood.merge(&s);
        // Socket and leaf-shaped head; a ferrule at the butt.
        iron.merge(&tube(
            &[top - d * 0.02, top + d * 0.08],
            &|t| 0.017 - 0.006 * t,
            6,
            false,
        ));
        let u = d.cross(Vec3::Z).normalize_or(Vec3::X);
        let w = d.cross(u);
        let base = top + d * 0.08;
        let widest = base + d * 0.07;
        let tip = base + d * 0.26;
        let ring = [
            widest + u * 0.032,
            widest + w * 0.006,
            widest - u * 0.032,
            widest - w * 0.006,
        ];
        for k in 0..4 {
            let (a, b) = (ring[k], ring[(k + 1) % 4]);
            facet(&mut iron, [base, b, a], widest, Vec4::ONE, |p| {
                Vec2::new(p.x + p.z, p.y)
            });
            facet(&mut iron, [tip, a, b], widest, Vec4::ONE, |p| {
                Vec2::new(p.x + p.z, p.y)
            });
        }
        iron.merge(&tube(
            &[foot - d * 0.01, foot + d * 0.06],
            &|_| 0.017,
            6,
            true,
        ));
    }
    // Wooden practice swords, point down, hilts resting on the top rail.
    let swords: &[f32] = if variant == 0 {
        &[0.1, 0.28]
    } else {
        &[-0.3, -0.12, 0.02]
    };
    for &x in swords {
        let lean = Quat::from_rotation_x(0.16) * Quat::from_rotation_z(rng.signed() * 0.06);
        let at = Vec3::new(x, 0.0, -0.06);
        let place = |m: &Mesh, local: Vec3| {
            transformed(m, Mat4::from_rotation_translation(lean, at + lean * local))
        };
        let blade = plank(&mut rng, Vec3::new(0.62, 0.052, 0.02), 0.008);
        let blade = transformed(&blade, Mat4::from_rotation_z(FRAC_PI_2));
        wood.merge(&place(&blade, Vec3::new(0.0, 0.33, 0.0)));
        let guard = plank(&mut rng, Vec3::new(0.13, 0.03, 0.045), 0.008);
        wood.merge(&place(&guard, Vec3::new(0.0, 0.655, 0.0)));
        let mut grip = tube(
            &[Vec3::new(0.0, 0.67, 0.0), Vec3::new(0.0, 0.79, 0.0)],
            &|_| 0.016,
            6,
            false,
        );
        offset_uv(&mut grip, Vec2::new(rng.range(0.0, 1.0), 0.0));
        leather.merge(&place(&grip, Vec3::ZERO));
        let pommel = rounded_box(Vec3::new(0.026, 0.022, 0.026), BoxOpts::round(0.014));
        iron.merge(&place(&pommel, Vec3::new(0.0, 0.81, 0.0)));
    }
    // Round shields propped against the foot of the rack, faces out.
    let shields: &[(f32, f32)] = if variant == 0 {
        &[(-0.45, -0.1), (0.5, 0.15)]
    } else {
        &[(0.0, 0.05)]
    };
    for &(x, spin) in shields {
        let (paint, edge) = if rng.f() < 0.5 {
            (Vec3::new(0.8, 0.6, 0.4), Vec3::new(0.42, 0.33, 0.26))
        } else {
            (Vec3::new(0.48, 0.55, 0.42), Vec3::new(0.85, 0.75, 0.55))
        };
        let (shield, boss, rim) = shield_meshes(paint, edge, rng.range(0.0, TAU));
        let m = Mat4::from_translation(Vec3::new(x, 0.43, -0.3))
            * Mat4::from_rotation_z(spin)
            * Mat4::from_rotation_x(-1.3);
        wood.merge(&transformed(&shield, m));
        iron.merge(&transformed(&boss, m));
        leather.merge(&transformed(&rim, m));
    }
    (wood, iron, leather)
}

/// A round shield lying face up (radius 0.42 m): planks painted in quarters, an iron boss
/// and a rawhide rim. Returns (wood, iron, leather).
fn shield_meshes(paint: Vec3, edge: Vec3, rot: f32) -> (Mesh, Mesh, Mesh) {
    let back = [Vec2::new(0.0, -0.012), Vec2::new(0.42, -0.012)];
    let side = [Vec2::new(0.42, -0.012), Vec2::new(0.42, 0.0)];
    let face = [
        Vec2::new(0.42, 0.0),
        Vec2::new(0.4, 0.01),
        Vec2::new(0.3, 0.024),
        Vec2::new(0.15, 0.034),
        Vec2::new(0.0, 0.038),
    ];
    let mut wood = lathe(&[&back, &side, &face], 28, 0.42, &|_, _, r| r);
    for v in &mut wood.vertices {
        let p = Vec3::from(v.position);
        let a = ((-p.z).atan2(p.x) + rot).rem_euclid(TAU);
        let quarter = ((a / FRAC_PI_2) as u32).is_multiple_of(2);
        let r = Vec2::new(p.x, p.z).length();
        let c = if r > 0.36 {
            edge
        } else if quarter {
            paint
        } else {
            paint * 0.55
        };
        let worn = 0.85 + 0.15 * vn3(p * 9.0, 77);
        v.color = (c * worn).extend(1.0).to_array();
    }
    let boss_p = [
        Vec2::new(0.1, 0.028),
        Vec2::new(0.1, 0.048),
        Vec2::new(0.085, 0.072),
        Vec2::new(0.05, 0.09),
        Vec2::new(0.0, 0.096),
    ];
    let boss = lathe(&[&boss_p], 20, 0.1, &|_, _, r| r);
    let rim_p = [
        Vec2::new(0.405, -0.016),
        Vec2::new(0.43, -0.012),
        Vec2::new(0.432, 0.002),
        Vec2::new(0.41, 0.014),
        Vec2::new(0.385, 0.015),
    ];
    let rim = lathe(&[&rim_p], 28, 0.42, &|_, _, r| r);
    (wood, boss, rim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Grounds {
        Grounds {
            spots: vec![(Vec2::ZERO, 7.0, 1.0), (Vec2::new(12.0, -23.0), 4.6, 1.0)],
            paths: vec![(Vec2::new(0.0, 40.5), Vec2::new(0.0, 27.5), 2.8, 1.0)],
            columns: vec![Vec2::new(16.6, 6.9)],
            puddles: vec![(Vec2::new(-30.0, 23.0), 0.5)],
            tag: 1,
            ..Default::default()
        }
    }

    #[test]
    fn the_raked_layer_is_whole_where_untouched_and_gone_where_trampled() {
        assert_eq!(Grounds::rake(0.0), 1.0);
        assert_eq!(Grounds::rake(0.2), 1.0);
        assert!(Grounds::rake(0.62) < TORN && Grounds::rake(1.0) < TORN);
        // The mask cuts at alpha 0.5: where the vertex alpha is 1 no texel may tear a hole
        // (there is no trampled sand underneath), below 0.5 nothing of it shows.
        let set = raked_set();
        let min = set.albedo.data.chunks(4).map(|p| p[3]).min().unwrap();
        assert!(min >= 128, "raked alpha {min} would tear untouched sand");
        assert!(
            set.albedo.data.chunks(4).any(|p| p[3] < 160),
            "the raked alpha has footprints to tear"
        );
    }

    #[test]
    fn straw_tufts_stay_on_their_cards_and_keep_their_coverage() {
        let set = straw_set();
        let img = &set.albedo;
        let n = img.width as usize;
        let cover = |px: &[u8]| {
            px.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128).count() as f32
                / (px.len() / 4) as f32
        };
        let full = cover(&img.data);
        assert!(
            (0.08..0.6).contains(&full),
            "straw covers {full} of the atlas"
        );
        // No stalk reaches a card's edge (each quadrant is one card).
        let half = n / 2;
        let edge = |v: usize| (v % half).min(half - 1 - v % half);
        for (i, p) in img.data.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (i % n, i / n);
            if edge(x).min(edge(y)) < n / 64 {
                assert!(p[3] < 128, "straw at the card's edge ({x}, {y})");
            }
        }
        // Distant tufts don't thin away.
        for (k, m) in img.mips.iter().take(4).enumerate() {
            let c = cover(m);
            assert!(c >= full * 0.9, "mip {} covers {c} of {full}", k + 1);
        }
    }

    #[test]
    fn floor_layers_cover_the_floor_and_face_up() {
        let g = plan();
        let (base, raked) = g.floor_meshes();
        let area = |m: &Mesh| -> f32 {
            m.indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| {
                    let p = t.map(|i| Vec3::from(m.vertices[i as usize].position));
                    let n = (p[1] - p[0]).cross(p[2] - p[0]);
                    assert!(n.y > 0.0, "floor triangle faces down");
                    n.y * 0.5
                })
                .sum()
        };
        let (a, b) = (area(&base), area(&raked));
        // The 36-gon inside the curb.
        let floor = 36.0 * APOTHEM * APOTHEM * (PI / 36.0).tan();
        assert!(
            a + b >= floor * 0.999,
            "layers cover {a} + {b} of {floor} m²"
        );
        // Each layer covers only its part (they overlap where the raked layer tears).
        assert!(
            a < floor * 0.6 && b < floor * 0.97,
            "partial layers: {a}, {b}"
        );
        // Raked vertices hold their coverage in alpha, the trampled ones are opaque.
        assert!(raked.vertices.iter().all(|v| v.color[3] >= 0.4));
        assert!(base.vertices.iter().all(|v| v.color[3] == 1.0));
        // Trampled at the middle (no raked layer), raked out in the open.
        let near = |m: &Mesh, q: Vec2| {
            m.vertices
                .iter()
                .any(|v| Vec2::new(v.position[0], v.position[2]).distance(q) < 0.8)
        };
        assert!(!near(&raked, Vec2::ZERO) && near(&base, Vec2::ZERO));
        let open = Vec2::new(-25.0, -10.0);
        assert!(near(&raked, open) && !near(&base, open));
    }

    #[test]
    fn drifts_and_tracks_hug_the_sand() {
        let mut g = plan();
        let path = [
            Vec2::new(0.0, 40.4),
            Vec2::new(0.0, 32.0),
            Vec2::new(3.0, 28.0),
        ];
        g.ruts.push((curve(&path, 0.4), 0.022));
        for m in [g.drift_mesh(), g.track_mesh()] {
            assert!(m.triangle_count() > 100);
            for v in &m.vertices {
                let q = Vec2::new(v.position[0], v.position[2]);
                let dy = v.position[1] - g.ground(q);
                assert!((-0.013..0.15).contains(&dy), "{dy} m off the sand at {q}");
            }
        }
    }
}
