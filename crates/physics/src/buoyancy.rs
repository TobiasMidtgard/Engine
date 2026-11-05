//! Buoyancy from each body's real shape.
//!
//! A collider is a closed triangle mesh (built once per shape and cached). Every step the
//! water's surface is sampled on a small grid over the collider, and the water's pressure
//! (ρ g × depth) is integrated over the mesh's wetted triangles, clipped at the waterline. The
//! sum is Archimedes' lift acting at the true center of buoyancy: a log floats lying down, a
//! plank rights itself, a ball floats as deep as a sphere does, a cube of half-density wood
//! rolls onto an edge as a real one does, and a hull pitches over a swell shorter than itself.
//!
//! Drag stays the module's per-body model (`step::prepare_forces`), scaled by the fraction of
//! the volume under water. Shapes without a mesh here (triangle meshes, heightfields, rounded
//! shapes) keep the older estimate from the fraction of their bounding box under water.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::{DVec3, Mat3, Vec3};
use engine_world::Water;
use rapier3d::math::Pose;
use rapier3d::parry::shape::Shape;
use rapier3d::prelude::SharedShape;

/// Water density, kg/m³.
pub(crate) const WATER_DENSITY: f32 = 1000.0;

/// Subdivisions for curved shapes: 16 around, 8 from pole to pole (256 triangles a ball).
const AROUND: u32 = 16;
const ALONG: u32 = 8;

/// A closed, outward-facing triangle mesh of a collider, in the collider's own frame.
pub(crate) struct Hull {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    /// Enclosed volume, m³ (the shape's own volume: curved meshes are scaled to it).
    pub volume: f32,
    /// The same hull with fewer triangles, for bodies far from every focus ([`Hull::at`]).
    coarse: Option<Box<Hull>>,
    /// A finely detailed hull with a few hundred triangles, for bodies near a focus.
    near: Option<Box<Hull>>,
}

/// Hulls with more triangles than this get a coarse copy for far bodies.
const COARSE_ABOVE: usize = 160;
/// Cells across the coarse copy's vertex grid, along each axis.
const COARSE_CELLS: f32 = 5.0;
/// Hulls with more triangles than this float near a focus on a copy merged on a
/// [`NEAR_CELLS`]-across grid. A boat's render-detail hull (about 1,300 triangles at the lake)
/// was pressed on triangle by triangle every substep, most of the lake stop's buoyancy time
/// (round 19); boat buoyancy is usually worked out on a simplified hull of a few hundred
/// triangles (Kerner, "Water interaction model for boats in video games", 2015).
const NEAR_ABOVE: usize = 512;
/// Cells across the near copy's vertex grid, along each axis: a 5 m boat's cells are 0.5 m
/// long, its 1 m depth's 10 cm.
const NEAR_CELLS: f32 = 10.0;

impl Hull {
    /// The mesh of `shape`, or `None` for shapes floated by their bounding box.
    pub fn of(shape: &dyn Shape) -> Option<Hull> {
        let (verts, tris, curved) = mesh(shape)?;
        let mut hull = Hull {
            verts,
            tris,
            volume: 0.0,
            coarse: None,
            near: None,
        };
        let (volume, centroid) = hull.volume_and_centroid();
        if volume.abs() < 1e-9 {
            return None;
        }
        if volume < 0.0 {
            for t in &mut hull.tris {
                t.swap(1, 2);
            }
        }
        hull.volume = volume.abs();
        // A tessellated ball or cylinder is a little smaller than the real one; scale it up
        // so it displaces exactly the shape's volume when under water.
        let real = shape.mass_properties(1.0).mass();
        if curved && real > 0.0 {
            let k = (real / hull.volume).cbrt();
            for v in &mut hull.verts {
                *v = centroid + (*v - centroid) * k;
            }
            hull.volume = real;
        }
        if hull.tris.len() > COARSE_ABOVE {
            hull.coarse = hull.decimated(COARSE_CELLS).map(Box::new);
        }
        if hull.tris.len() > NEAR_ABOVE {
            hull.near = hull.decimated(NEAR_CELLS).map(Box::new);
        }
        Some(hull)
    }

    /// This hull's copy for the water looked at on `cells` across: the coarse one on less
    /// than the full grid (`cells` below [`MAX_CELLS`]: the body is far from every focus;
    /// pressing on every triangle of a boat's hull was most of the buoyancy's time at the
    /// plaza, for boats 300 m away on the lake), else the near one ([`NEAR_ABOVE`]), else
    /// this.
    pub fn at(&self, cells: usize) -> &Hull {
        match (&self.coarse, &self.near) {
            (Some(c), _) if cells < MAX_CELLS => c,
            (_, Some(n)) => n,
            _ => self,
        }
    }

    /// The hull with its vertices merged on a grid `across` cells along each axis (vertex
    /// clustering, Rossignac and Borrel 1993): triangles that collapse are dropped, and the
    /// rest scaled about the centroid to the same volume, so it floats as deep. `None` if too
    /// little is left.
    fn decimated(&self, across: f32) -> Option<Hull> {
        let (lo, hi) = self.verts.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), v| (lo.min(*v), hi.max(*v)),
        );
        let cell = ((hi - lo) / across).max(Vec3::splat(1e-4));
        let mut index: HashMap<(i32, i32, i32), u32> = HashMap::new();
        let (mut sums, mut counts): (Vec<Vec3>, Vec<f32>) = (Vec::new(), Vec::new());
        let map: Vec<u32> = self
            .verts
            .iter()
            .map(|v| {
                let k = ((*v - lo) / cell).floor().min(Vec3::splat(across - 1.0));
                let key = (k.x as i32, k.y as i32, k.z as i32);
                let i = *index.entry(key).or_insert_with(|| {
                    sums.push(Vec3::ZERO);
                    counts.push(0.0);
                    (sums.len() - 1) as u32
                });
                sums[i as usize] += *v;
                counts[i as usize] += 1.0;
                i
            })
            .collect();
        let verts: Vec<Vec3> = sums.iter().zip(&counts).map(|(s, n)| *s / *n).collect();
        let mut seen = std::collections::HashSet::new();
        let tris: Vec<[u32; 3]> = self
            .tris
            .iter()
            .map(|t| t.map(|i| map[i as usize]))
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .filter(|t| {
                let mut k = *t;
                k.sort_unstable();
                seen.insert(k)
            })
            .collect();
        if tris.len() < 4 || tris.len() >= self.tris.len() {
            return None;
        }
        let mut coarse = Hull {
            verts,
            tris,
            volume: 0.0,
            coarse: None,
            near: None,
        };
        let (volume, centroid) = coarse.volume_and_centroid();
        if volume < self.volume * 0.5 {
            return None;
        }
        let k = (self.volume / volume).cbrt();
        for v in &mut coarse.verts {
            *v = centroid + (*v - centroid) * k;
        }
        coarse.volume = self.volume;
        Some(coarse)
    }

    /// Signed volume and centroid (tetrahedra from the origin).
    fn volume_and_centroid(&self) -> (f32, Vec3) {
        let (mut vol, mut moment) = (0.0, Vec3::ZERO);
        for t in &self.tris {
            let [a, b, c] = t.map(|i| self.verts[i as usize]);
            let v = a.dot(b.cross(c)) / 6.0;
            vol += v;
            moment += (a + b + c) * (v / 4.0);
        }
        let centroid = if vol.abs() > 1e-12 {
            moment / vol
        } else {
            Vec3::ZERO
        };
        (vol, centroid)
    }

    /// The water's push on this mesh placed at `pose` in `surface`, under gravity `g` (m/s²),
    /// for a body moving at `vel` (relative to the water) and turning at `spin` about `com`:
    /// the force, its torque about `com`, and the volume of water displaced.
    pub fn immerse(
        &self,
        pose: &Pose,
        surface: &Surface,
        g: f32,
        com: Vec3,
        vel: Vec3,
        spin: Vec3,
    ) -> Wetted {
        let rho_g = WATER_DENSITY * g;
        // Vertices relative to the center of mass, and their depth under the surface.
        let mut placed = Vec::with_capacity(self.verts.len());
        for v in &self.verts {
            let p = pose.transform_point(*v);
            placed.push((p - com, surface.height(p.x, p.z) - p.y));
        }
        let mut out = Wetted::default();
        // Wetted bottom as seen from below: its area, first and second moments of r × up.
        let (mut base, mut base_arm, mut base_inertia) = (0.0, Vec3::ZERO, Mat3::ZERO);
        let mut add = |q: [Vec3; 3], d: [f32; 3]| {
            // Pressure grows linearly with depth over the triangle: ∫d dA = A Σd/3, and
            // ∫d x dA = A/12 (Σ dᵢxᵢ + Σd Σx). `area_n` is the outward normal times the area.
            let area_n = (q[1] - q[0]).cross(q[2] - q[0]) * 0.5;
            let s = d[0] + d[1] + d[2];
            out.force += -area_n * (rho_g * s / 3.0);
            // Torque about the center of mass: ∫ x × dF = (∫d x dA) × (−ρ g n).
            let first = q[0] * d[0] + q[1] * d[1] + q[2] * d[2] + (q[0] + q[1] + q[2]) * s;
            out.torque += (first / 12.0).cross(-area_n * rho_g);
            if area_n.y < 0.0 {
                // r × up is linear in r, so over the triangle ∫ (r × up) dA = A c × up and
                // ∫ (r × up)(r × up)ᵀ dA = A/12 (Σ aᵢaᵢᵀ + Σa Σaᵀ) with aᵢ = qᵢ × up.
                let a = -area_n.y;
                let arms = q.map(|p| p.cross(Vec3::Y));
                let sum = arms[0] + arms[1] + arms[2];
                let outer = |v: Vec3| Mat3::from_cols(v * v.x, v * v.y, v * v.z);
                base += a;
                base_arm += sum * (a / 3.0);
                base_inertia +=
                    (outer(arms[0]) + outer(arms[1]) + outer(arms[2]) + outer(sum)) * (a / 12.0);
            }
        };
        for t in &self.tris {
            let [(a, da), (b, db), (c, dc)] = t.map(|i| placed[i as usize]);
            clip_under([a, b, c], [da, db, dc], &mut add);
        }
        out.volume = (out.force.y / rho_g.max(1e-9)).max(0.0);
        if base > 1e-6 && out.volume > 0.0 {
            // Bobbing, rolling and pitching send waves away, which damps them within a few
            // periods: each patch of the bottom resists moving up or down with
            // ρ √(g D) per m² (D the mean draft), half the critical damping of heave.
            let draft = out.volume / base;
            let b = WATER_DENSITY * (g * draft).sqrt();
            // A patch at r moves up at vel.y + spin · (r × up).
            let up_speed = vel.y * base + spin.dot(base_arm);
            out.force.y -= b * up_speed;
            out.torque -= (base_arm * vel.y + base_inertia * spin) * b;
        }
        out
    }
}

/// What the water does to a hull this step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Wetted {
    pub force: Vec3,
    pub torque: Vec3,
    /// Displaced water, m³.
    pub volume: f32,
}

/// Call `under` with the part of triangle `q` (depths `d`, positive under water) that is under
/// the water, as one or two triangles in the same winding.
fn clip_under(q: [Vec3; 3], d: [f32; 3], under: &mut impl FnMut([Vec3; 3], [f32; 3])) {
    let wet = d.map(|x| x > 0.0);
    match wet.iter().filter(|w| **w).count() {
        0 => {}
        3 => under(q, d),
        n => {
            // Rotate so the odd one out comes first, keeping the winding.
            let odd = (0..3).find(|&i| wet[i] == (n == 1)).unwrap();
            let (i, j, k) = (odd, (odd + 1) % 3, (odd + 2) % 3);
            let cut = |a: usize, b: usize| q[a] + (q[b] - q[a]) * (d[a] / (d[a] - d[b]));
            let (pij, pik) = (cut(i, j), cut(i, k));
            if n == 1 {
                // Only `i` is under: the tip of the triangle.
                under([q[i], pij, pik], [d[i], 0.0, 0.0]);
            } else {
                // Only `i` is dry: the quad j, k, pik, pij under water, as two triangles.
                under([pij, q[j], q[k]], [0.0, d[j], d[k]]);
                under([pij, q[k], pik], [0.0, d[k], 0.0]);
            }
        }
    }
}

/// Mesh of a shape in its own frame, and whether it approximates a curved surface.
fn mesh(shape: &dyn Shape) -> Option<(Vec<Vec3>, Vec<[u32; 3]>, bool)> {
    if let Some(s) = shape.as_cuboid() {
        let (v, t) = s.to_trimesh();
        Some((v, t, false))
    } else if let Some(s) = shape.as_ball() {
        let (v, t) = s.to_trimesh(AROUND, ALONG);
        Some((v, t, true))
    } else if let Some(s) = shape.as_capsule() {
        let (v, t) = s.to_trimesh(AROUND, ALONG);
        Some((v, t, true))
    } else if let Some(s) = shape.as_cylinder() {
        let (v, t) = s.to_trimesh(AROUND * 2);
        Some((v, t, true))
    } else if let Some(s) = shape.as_cone() {
        let (v, t) = s.to_trimesh(AROUND * 2);
        Some((v, t, true))
    } else if let Some(s) = shape.as_convex_polyhedron() {
        let (v, t) = s.to_trimesh();
        Some((v, t, false))
    } else if let Some(s) = shape.as_compound() {
        // Each part closed on its own (and already scaled to its volume); overlaps count
        // twice, as they do for mass.
        let (mut verts, mut tris) = (Vec::new(), Vec::new());
        for (pose, part) in s.shapes() {
            let hull = Hull::of(&**part)?;
            let base = verts.len() as u32;
            verts.extend(hull.verts.iter().map(|v| pose.transform_point(*v)));
            tris.extend(hull.tris.iter().map(|t| t.map(|i| i + base)));
        }
        Some((verts, tris, false))
    } else {
        None
    }
}

/// Hulls by shape, built on first use. Holds a clone of each shape so its address can't be
/// reused while cached; shapes nothing else uses any more are dropped now and then.
#[derive(Default)]
pub(crate) struct HullCache {
    hulls: HashMap<usize, (SharedShape, Option<Arc<Hull>>)>,
    prune_at: usize,
}

impl HullCache {
    pub fn get(&mut self, shape: &SharedShape) -> Option<Arc<Hull>> {
        let key = Arc::as_ptr(&shape.0) as *const () as usize;
        if self.hulls.len() >= self.prune_at.max(64) {
            self.hulls.retain(|_, (s, _)| Arc::strong_count(&s.0) > 1);
            self.prune_at = self.hulls.len() * 2;
        }
        self.hulls
            .entry(key)
            .or_insert_with(|| (shape.clone(), Hull::of(&*shape.0).map(Arc::new)))
            .1
            .clone()
    }
}

/// The water's answers, on world-aligned lattices: one node every 2 m, 4 m, 8 m...
///
/// `Water::sample` is 5-35 us (every lake's waves, its depth from the ground, ripples, flows),
/// and every floating collider asked for up to 26 of them every substep, at points that moved
/// with it. The water stands still through a step (its systems run between steps), so a node
/// asked once is shared by every substep, collider and body over it. A node is asked again
/// every second step near a focus, every fourth to [`MID_CELLS_M`] and every eighth past it
/// ([`node_steps`]), and between its asks it follows the surface at the rate it rose or fell
/// between its last two (first-order dead reckoning, as networked games carry a position
/// forward between updates). For a wave a meter high every 4 s that is within a
/// millimeter near a focus, 4 mm to [`MID_CELLS_M`] and 2 cm past it, where holding a node
/// still for the 3 steps it was kept there was up to 4 cm off; and the lake's boats ask for
/// half as many samples (53 a step near them, 0.34 ms on the VM, round 18).
/// Aged by [`WaterCache::next_step`].
#[derive(Default)]
pub(crate) struct WaterCache {
    nodes: HashMap<(u8, i64, i64), Kept>,
    step: u64,
    asked: usize,
}

/// A lattice node: the surface height (world) and current there, if there is water.
type Node = Option<(f64, Vec3)>;

/// A node's last answer: when it was asked, how fast its surface was rising then (meters a
/// step, from the answer before), and the step it is asked again at.
#[derive(Clone, Copy)]
struct Kept {
    node: Node,
    asked_at: u64,
    rate: f64,
    until: u64,
}

impl Kept {
    /// The node `step`s on: its surface moved on at its rate.
    fn at(&self, step: u64) -> Node {
        let ahead = step.saturating_sub(self.asked_at) as f64;
        self.node.map(|(h, v)| (h + self.rate * ahead, v))
    }
}

/// Steps a lattice node is kept, by the grid cells its collider asked for ([`cells_at`]): two
/// near a focus, four to [`MID_CELLS_M`], eight past it.
fn node_steps(cells: usize) -> u64 {
    if cells >= MAX_CELLS {
        2
    } else if cells > FAR_CELLS {
        4
    } else {
        8
    }
}

/// The fastest a node's surface is taken to rise or fall, meters a step: past it the two
/// answers aren't the same wave (a stream turned on, a lake streamed in) and the node is held.
const MAX_RATE_M: f64 = 0.25;

impl WaterCache {
    /// Starts a step. A node due to be asked again stays one more of its spans, so its next
    /// answer can give its rate; past that it is forgotten.
    pub fn next_step(&mut self) {
        self.step += 1;
        let now = self.step;
        self.nodes
            .retain(|_, k| k.until + (k.until - k.asked_at) > now);
        self.asked = 0;
    }

    /// Forgets everything (the world moved under the lattice: a floating-origin shift).
    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Water samples taken this step (each one a `Water::sample`).
    pub fn asked(&self) -> usize {
        self.asked
    }

    /// Surface height (world) and current at lattice node `ix`, `iz` of `level`, kept for
    /// `keep` steps once asked.
    fn node(
        &mut self,
        water: &Water,
        level: u8,
        ix: i64,
        iz: i64,
        keep: u64,
    ) -> Option<(f64, Vec3)> {
        let before = self.nodes.get(&(level, ix, iz)).copied();
        if let Some(k) = before
            && k.until > self.step
        {
            return k.at(self.step);
        }
        self.asked += 1;
        let s = spacing(level) as f64;
        let n = water
            .sample(DVec3::new(ix as f64 * s, 0.0, iz as f64 * s))
            .map(|w| (w.surface_height, w.velocity));
        let rate = match (before, n) {
            (
                Some(Kept {
                    node: Some((h0, _)),
                    asked_at,
                    ..
                }),
                Some((h, _)),
            ) if asked_at < self.step => {
                let r = (h - h0) / (self.step - asked_at) as f64;
                if r.abs() <= MAX_RATE_M { r } else { 0.0 }
            }
            _ => 0.0,
        };
        self.nodes.insert(
            (level, ix, iz),
            Kept {
                node: n,
                asked_at: self.step,
                rate,
                until: self.step + keep,
            },
        );
        n
    }

    /// Surface height (world) and current at `p`, bilinear between the finest lattice's
    /// nodes; asked directly where the water ends between them (a shore).
    pub fn at(&mut self, water: &Water, p: DVec3, cells: usize) -> Option<(f64, Vec3)> {
        let keep = node_steps(cells);
        let s = spacing(0) as f64;
        let (fx, fz) = (p.x / s, p.z / s);
        let (ix, iz) = (fx.floor() as i64, fz.floor() as i64);
        let (tx, tz) = (fx - ix as f64, fz - iz as f64);
        let mut corners = [(0.0, Vec3::ZERO); 4];
        for (k, c) in corners.iter_mut().enumerate() {
            match self.node(water, 0, ix + (k & 1) as i64, iz + (k >> 1) as i64, keep) {
                Some(n) => *c = n,
                None => {
                    self.asked += 1;
                    return water.sample(p).map(|w| (w.surface_height, w.velocity));
                }
            }
        }
        let lerp = |a: (f64, Vec3), b: (f64, Vec3), t: f64| {
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t as f32)
        };
        let near = lerp(corners[0], corners[1], tx);
        let far = lerp(corners[2], corners[3], tx);
        Some(lerp(near, far, tz))
    }
}

/// Spacing of lattice `level`, meters.
fn spacing(level: u8) -> f32 {
    SAMPLE_SPACING * (1u32 << level) as f32
}

/// The water surface over a patch: heights on a grid (region-local y) and the mean current.
pub(crate) struct Surface {
    x0: f32,
    z0: f32,
    step: f32,
    nx: usize,
    nz: usize,
    heights: Vec<f32>,
    /// Highest point of the patch, region-local.
    pub top: f32,
    pub current: Vec3,
}

/// Grid spacing over a collider: one sample every 2 m, at most about a 5 × 5 grid.
const SAMPLE_SPACING: f32 = 2.0;
pub(crate) const MAX_CELLS: usize = 4;
/// The grid's cells across past the full level of detail ([`crate::lod`]): its corners only,
/// a plane through the water there. Each sample asks the water module for its waves and
/// ripples, and a pixel there spans 15 cm or more.
pub(crate) const FAR_CELLS: usize = 1;

/// Within this of a focus a floating body's water is looked at on the full grid
/// ([`MAX_CELLS`]), meters; to [`MID_CELLS_M`] on 2 × 2 cells, past it on [`FAR_CELLS`].
const NEAR_CELLS_M: f64 = 60.0;
const MID_CELLS_M: f64 = 150.0;

/// The grid's cells across for a collider at `at` (world), by its own distance to the nearest
/// focus: a region's tier follows its nearest mover, and the lake's boats share one with the
/// race cars, so 300 m from the plaza they asked for the full grid, 26 water samples a
/// collider a substep (1.5 ms a step for two boats on 4 cores, most of it `Water::sample`).
/// No focus (tests, tools): the full grid.
pub(crate) fn cells_at(at: DVec3, foci: &[DVec3]) -> usize {
    let d = foci
        .iter()
        .map(|f| f.distance(at))
        .fold(f64::INFINITY, f64::min);
    if foci.is_empty() || d <= NEAR_CELLS_M {
        MAX_CELLS
    } else if d <= MID_CELLS_M {
        2
    } else {
        FAR_CELLS
    }
}

impl Surface {
    /// The water over the box `mins`..`maxs` (region-local, region at `anchor`), from the
    /// step's lattice ([`WaterCache`]) whose spacing spans the box in at most `max_cells`
    /// cells, or `None` where there is no water under it. Nodes past the water's edge take
    /// the mean height of those in it.
    pub fn sample(
        water: &Water,
        cache: &mut WaterCache,
        anchor: DVec3,
        mins: Vec3,
        maxs: Vec3,
        max_cells: usize,
    ) -> Option<Surface> {
        let span = (maxs.x - mins.x).max(maxs.z - mins.z);
        let mut level = 0u8;
        while level < 12 && span / spacing(level) > max_cells.max(1) as f32 {
            level += 1;
        }
        let step = spacing(level);
        let s = step as f64;
        let keep = node_steps(max_cells);
        let (lo, hi) = (anchor + mins.as_dvec3(), anchor + maxs.as_dvec3());
        let (ix0, iz0) = ((lo.x / s).floor() as i64, (lo.z / s).floor() as i64);
        let (ix1, iz1) = (
            ((hi.x / s).ceil() as i64).max(ix0 + 1),
            ((hi.z / s).ceil() as i64).max(iz0 + 1),
        );
        let (nx, nz) = ((ix1 - ix0 + 1) as usize, (iz1 - iz0 + 1) as usize);
        let mut heights = Vec::with_capacity(nx * nz);
        let mut current = Vec3::ZERO;
        let (mut found, mut sum) = (0usize, 0.0f32);
        for iz in iz0..=iz1 {
            for ix in ix0..=ix1 {
                let h = match cache.node(water, level, ix, iz, keep) {
                    Some((h, v)) => {
                        current += v;
                        found += 1;
                        let h = (h - anchor.y) as f32;
                        sum += h;
                        h
                    }
                    None => f32::NAN,
                };
                heights.push(h);
            }
        }
        if found == 0 {
            return None;
        }
        let fallback = sum / found as f32;
        for h in &mut heights {
            if h.is_nan() {
                *h = fallback;
            }
        }
        let top = heights.iter().copied().fold(f32::MIN, f32::max);
        Some(Surface {
            x0: (ix0 as f64 * s - anchor.x) as f32,
            z0: (iz0 as f64 * s - anchor.z) as f32,
            step,
            nx,
            nz,
            heights,
            top,
            current: current / found as f32,
        })
    }

    /// Surface height at `x`, `z` (region-local), bilinear between samples.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let fx = ((x - self.x0) / self.step).clamp(0.0, (self.nx - 1) as f32);
        let fz = ((z - self.z0) / self.step).clamp(0.0, (self.nz - 1) as f32);
        let (ix, iz) = (
            (fx as usize).min(self.nx - 2),
            (fz as usize).min(self.nz - 2),
        );
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let at = |i: usize, j: usize| self.heights[j * self.nx + i];
        let near = at(ix, iz) + (at(ix + 1, iz) - at(ix, iz)) * tx;
        let far = at(ix, iz + 1) + (at(ix + 1, iz + 1) - at(ix, iz + 1)) * tx;
        near + (far - near) * tz
    }

    /// A flat surface at height `h` (tests).
    #[cfg(test)]
    pub fn flat(h: f32) -> Surface {
        Surface {
            x0: -1e6,
            z0: -1e6,
            step: 2e6,
            nx: 2,
            nz: 2,
            heights: vec![h; 4],
            top: h,
            current: Vec3::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapier3d::parry::shape::{Ball, Capsule, Cuboid};

    const G: f32 = 9.81;
    const RHO_G: f32 = WATER_DENSITY * G;

    #[test]
    fn hull_volumes_match_the_shapes() {
        let shapes: Vec<Box<dyn Shape>> = vec![
            Box::new(Cuboid::new(Vec3::new(0.5, 0.2, 1.0))),
            Box::new(Ball::new(0.4)),
            Box::new(Capsule::new_y(0.6, 0.25)),
        ];
        for s in shapes {
            let hull = Hull::of(&*s).unwrap();
            let real = s.mass_properties(1.0).mass();
            assert!(
                (hull.volume - real).abs() < 1e-4 * real.max(1.0),
                "{} vs {real}",
                hull.volume
            );
            let (v, _) = hull.volume_and_centroid();
            assert!(
                (v - real).abs() < 0.002 * real,
                "outward mesh volume {v} vs {real}"
            );
        }
    }

    /// Fully under water, any shape gets exactly ρ g V, straight up, through its centroid.
    #[test]
    fn submerged_hulls_get_archimedes_lift() {
        let hull = Hull::of(&Cuboid::new(Vec3::new(0.5, 0.25, 1.0))).unwrap();
        let pose = Pose::from_parts(
            Vec3::new(1.0, -3.0, 2.0),
            engine_core::math::Quat::from_rotation_z(0.4)
                * engine_core::math::Quat::from_rotation_x(0.3),
        );
        let w = hull.immerse(
            &pose,
            &Surface::flat(0.0),
            G,
            pose.translation,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!((w.force - Vec3::Y * RHO_G).length() < 1.0, "{w:?}");
        assert!(w.torque.length() < 0.5, "{w:?}");
        assert!((w.volume - 1.0).abs() < 1e-3);
    }

    /// A far body's coarse hull ([`Hull::at`]) has a fraction of the triangles and floats
    /// about as deep: the same lift within a few percent, sunk to its middle or near its top.
    #[test]
    fn coarse_hulls_float_like_the_full_ones() {
        let hull = Hull::of(&Capsule::new_x(0.8, 0.3)).unwrap();
        let coarse = hull.at(FAR_CELLS);
        assert!(
            coarse.tris.len() * 2 < hull.tris.len(),
            "{} of {} triangles",
            coarse.tris.len(),
            hull.tris.len()
        );
        assert!(std::ptr::eq(hull.at(MAX_CELLS), &hull));
        for (depth, tip) in [(0.0, 0.0), (0.2, 0.0), (0.0, 0.25)] {
            let pose = Pose::from_parts(
                Vec3::new(0.0, -depth, 0.0),
                engine_core::math::Quat::from_rotation_z(tip),
            );
            let lift = |h: &Hull| {
                h.immerse(
                    &pose,
                    &Surface::flat(0.0),
                    G,
                    pose.translation,
                    Vec3::ZERO,
                    Vec3::ZERO,
                )
            };
            let (fine, rough) = (lift(&hull), lift(coarse));
            assert!(
                (rough.volume - fine.volume).abs() < 0.06 * fine.volume,
                "at {depth} m, {tip} rad: {rough:?} vs {fine:?}"
            );
        }
    }

    /// A detailed hull floats near a focus on a copy with a fraction of its triangles that
    /// floats as deep and rights itself as hard: a boat-like ellipsoid of about 1,200
    /// triangles, level and tipped, half and mostly under.
    #[test]
    fn near_hulls_float_like_the_full_ones() {
        use rapier3d::parry::shape::ConvexPolyhedron;
        let mut points = Vec::new();
        for i in 0..30 {
            let v = std::f32::consts::PI * (i as f32 + 0.5) / 30.0;
            for j in 0..20 {
                let u = std::f32::consts::TAU * j as f32 / 20.0;
                points.push(Vec3::new(
                    2.5 * v.sin() * u.cos(),
                    0.6 * v.cos(),
                    1.0 * v.sin() * u.sin(),
                ));
            }
        }
        let shape = ConvexPolyhedron::from_convex_hull(&points).unwrap();
        let hull = Hull::of(&shape).unwrap();
        let near = hull.at(MAX_CELLS);
        assert!(
            hull.tris.len() > NEAR_ABOVE,
            "{} triangles",
            hull.tris.len()
        );
        assert!(
            near.tris.len() * 2 < hull.tris.len(),
            "{} of {} triangles",
            near.tris.len(),
            hull.tris.len()
        );
        assert!(std::ptr::eq(hull.at(FAR_CELLS), hull.at(1)));
        for (depth, tip) in [(0.0, 0.0), (0.3, 0.0), (0.0, 0.3), (0.2, 0.5)] {
            let pose = Pose::from_parts(
                Vec3::new(0.0, -depth, 0.0),
                engine_core::math::Quat::from_rotation_x(tip),
            );
            let lift = |h: &Hull| {
                h.immerse(
                    &pose,
                    &Surface::flat(0.0),
                    G,
                    pose.translation,
                    Vec3::ZERO,
                    Vec3::ZERO,
                )
            };
            let (fine, rough) = (lift(&hull), lift(near));
            assert!(
                (rough.volume - fine.volume).abs() < 0.02 * fine.volume,
                "at {depth} m, {tip} rad: {rough:?} vs {fine:?}"
            );
            if tip > 0.0 {
                assert!(
                    (rough.torque - fine.torque).length() < 0.1 * fine.torque.length(),
                    "at {depth} m, {tip} rad: {rough:?} vs {fine:?}"
                );
            }
        }
    }

    /// Half under water, a box tipped about a long axis is pushed back level.
    #[test]
    fn tipped_boxes_are_righted() {
        let hull = Hull::of(&Cuboid::new(Vec3::new(1.0, 0.1, 0.3))).unwrap();
        let pose = Pose::from_parts(Vec3::ZERO, engine_core::math::Quat::from_rotation_x(0.3));
        let w = hull.immerse(
            &pose,
            &Surface::flat(0.0),
            G,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        // Rolled +x: the righting torque turns it back (−x).
        assert!(w.torque.x < -10.0, "{w:?}");
        // Its center on the waterline: half its 0.24 m³ under.
        assert!((w.volume - 0.12).abs() < 0.005, "{w:?}");
    }

    /// A ball sunk to its center displaces half its volume; a quarter of the way up from the
    /// bottom, the volume of a spherical cap.
    #[test]
    fn balls_displace_spherical_caps() {
        let r = 0.5;
        let hull = Hull::of(&Ball::new(r)).unwrap();
        let full = 4.0 / 3.0 * std::f32::consts::PI * r * r * r;
        for (h, want) in [
            (r, 0.5 * full),
            (
                0.5 * r,
                std::f32::consts::PI * (0.5 * r).powi(2) * (3.0 * r - 0.5 * r) / 3.0,
            ),
        ] {
            let pose = Pose::from_translation(Vec3::new(0.0, r - h, 0.0));
            let w = hull.immerse(
                &pose,
                &Surface::flat(0.0),
                G,
                pose.translation,
                Vec3::ZERO,
                Vec3::ZERO,
            );
            assert!(
                (w.volume - want).abs() < 0.02 * full,
                "draft {h}: {} vs {want}",
                w.volume
            );
        }
    }

    /// A surface rising steadily, one centimeter a step ([`Rising::step`] moves it on).
    struct Rising(std::sync::atomic::AtomicU64, std::sync::atomic::AtomicUsize);

    impl Rising {
        fn step(&self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl engine_world::WaterQuery for Rising {
        fn sample(&self, _: DVec3) -> Option<engine_world::WaterSample> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let k = self.0.load(std::sync::atomic::Ordering::Relaxed);
            Some(engine_world::WaterSample {
                surface_height: 0.01 * k as f64,
                depth: 5.0,
                velocity: Vec3::ZERO,
                normal: Vec3::Y,
            })
        }
    }

    /// Between its asks a node follows the surface at the rate it moved between the last two:
    /// a steadily rising surface is read exactly every step, near a focus asked every second
    /// step, far off every eighth.
    #[test]
    fn kept_water_nodes_follow_the_surface_between_asks() {
        let rising = Arc::new(Rising(Default::default(), Default::default()));
        let water = Water(rising.clone());
        for (cells, every) in [(MAX_CELLS, 2), (FAR_CELLS, 8)] {
            let mut cache = WaterCache::default();
            rising.1.store(0, std::sync::atomic::Ordering::Relaxed);
            let steps = 64;
            for _ in 0..steps {
                rising.step();
                cache.next_step();
                let now = rising.0.load(std::sync::atomic::Ordering::Relaxed);
                // Past the first two asks (the rate needs two answers).
                let (h, _) = cache.node(&water, 0, 3, -2, node_steps(cells)).unwrap();
                if cache.step > 2 * every {
                    assert!(
                        (h - 0.01 * now as f64).abs() < 1e-9,
                        "{cells} cells, step {}: {h} vs {}",
                        cache.step,
                        0.01 * now as f64
                    );
                }
            }
            let asked = rising.1.load(std::sync::atomic::Ordering::Relaxed);
            assert_eq!(asked as u64, steps / every, "{cells} cells");
        }
    }
}
