//! Choosing which quadtree patches to draw (level of detail), on the CPU.
//!
//! Each frame walks the quadtree from the roots: a patch splits into its four children when
//! the camera is closer than `split_factor` patch sizes, and only when all children are built
//! (otherwise it is drawn itself and the children are requested). Patches outside the view
//! frustum or behind the planet's horizon are skipped. The walk is cheap (a few hundred
//! nodes); building patches happens elsewhere, in parallel.

use std::collections::HashMap;

use engine_core::math::DVec3;

use crate::cube::{NodeKey, SurfaceMap};

/// What the walk knows about a patch.
#[derive(Debug, Clone, Copy)]
pub struct NodeBounds {
    pub min_height: f64,
    pub max_height: f64,
    /// Bounding sphere, terrain-local.
    pub center: DVec3,
    pub radius: f64,
    /// Whether a mesh for this patch exists.
    pub built: bool,
    /// Frame this patch was last visited.
    pub last_seen: u64,
    /// Whether the view's last walk split it into its children (see
    /// [`SelectInput::merge_factor`]).
    pub split: bool,
}

/// Inputs to one selection walk.
pub struct SelectInput<'a> {
    /// The camera in terrain-local coordinates.
    pub camera: DVec3,
    /// Split when closer than this many patch sizes (higher = more detail).
    pub split_factor: f64,
    /// A patch split on the last walk merges back only beyond this many patch sizes (at
    /// least `split_factor`), so the detail doesn't flicker at the boundary. `None` walks
    /// without that memory (another view of the same tree, such as a shadow map).
    pub merge_factor: Option<f64>,
    /// Frustum test for a terrain-local bounding sphere.
    pub visible: &'a dyn Fn(DVec3, f64) -> bool,
    /// Patches never split into children smaller than this (m across; 0 = down to the finest
    /// level). The far shadow map has texels of several meters, so its walk stops early.
    pub min_size: f64,
    /// How much of the split distance holds for a patch at a distance with its terrain-local
    /// bounding center (1 = all; less splits later). The view passes the air's transmittance
    /// to it, so land deep in haze, whose detail the haze hides, stays coarser (Cesium's fog
    /// raises tiles' allowed screen-space error the same way). `None` = 1 everywhere.
    pub detail: Option<&'a dyn Fn(f64, DVec3) -> f64>,
}

/// Result of a walk.
#[derive(Debug, Default)]
pub struct Selection {
    /// Patches to draw (all built), nearest first.
    pub draw: Vec<NodeKey>,
    /// Patches the walk wanted but which aren't built, with their distance (nearest first
    /// after sorting).
    pub missing: Vec<(f64, NodeKey)>,
    /// Nodes visited.
    pub visited: u32,
    /// About how many patches `draw` would hold if `SelectInput::detail` were 1 everywhere:
    /// each drawn patch counts 1 / detail², as the patch count grows with the square of the
    /// split distance. The detail boost fills the budget against this, so what the haze saves
    /// stays saved.
    pub clear_count: f64,
}

/// Quadtree state of one terrain.
#[derive(Debug, Clone)]
pub struct Lod {
    pub map: SurfaceMap,
    pub max_level: u8,
    pub nodes: HashMap<NodeKey, NodeBounds>,
    /// Height range used for patches nothing is known about yet.
    pub height_range: (f64, f64),
    /// Lowest point of the whole planet, for horizon culling.
    pub frame: u64,
}

impl Lod {
    /// `finest_spacing_m`: vertex spacing of the most detailed patches.
    pub fn new(map: SurfaceMap, height_range: (f64, f64), finest_spacing_m: f64) -> Self {
        let root_spacing = map.root_size() / crate::chunk::QUADS as f64;
        let max_level = (root_spacing / finest_spacing_m.max(0.01))
            .log2()
            .ceil()
            .clamp(0.0, 24.0) as u8;
        Self {
            map,
            max_level,
            nodes: HashMap::new(),
            height_range,
            frame: 0,
        }
    }

    /// Bounds for a patch from a height range (before or without its mesh).
    pub fn estimate(&self, key: NodeKey, min_h: f64, max_h: f64) -> NodeBounds {
        let (lo, size) = key.face_rect();
        let mut pts = [DVec3::ZERO; 18];
        let mut k = 0;
        for j in 0..3 {
            for i in 0..3 {
                let a = lo.x + size * i as f64 * 0.5;
                let b = lo.y + size * j as f64 * 0.5;
                pts[k] = self.map.position(key.face, a, b, min_h);
                pts[k + 1] = self.map.position(key.face, a, b, max_h);
                k += 2;
            }
        }
        let center = pts.iter().copied().sum::<DVec3>() / pts.len() as f64;
        let mut r = pts
            .iter()
            .map(|p| (*p - center).length())
            .fold(0.0, f64::max);
        // Curved patches bulge between the sample points.
        if self.map.is_sphere() {
            let half_angle = size * std::f64::consts::FRAC_PI_4;
            r += (self.map.radius + max_h.max(0.0)) * (1.0 - half_angle.cos()) + 1.0;
        }
        NodeBounds {
            min_height: min_h,
            max_height: max_h,
            center,
            radius: r,
            built: false,
            last_seen: 0,
            split: false,
        }
    }

    /// Record a built patch's real bounds.
    pub fn set_built(&mut self, key: NodeKey, min_h: f64, max_h: f64, center: DVec3, radius: f64) {
        let frame = self.frame;
        self.nodes.insert(
            key,
            NodeBounds {
                min_height: min_h,
                max_height: max_h,
                center,
                radius,
                built: true,
                last_seen: frame,
                split: false,
            },
        );
    }

    /// Forget a patch's mesh (it will be rebuilt when needed).
    pub fn unbuild(&mut self, key: NodeKey) {
        if let Some(n) = self.nodes.get_mut(&key) {
            n.built = false;
        }
    }

    fn bounds(&mut self, key: NodeKey) -> NodeBounds {
        if let Some(b) = self.nodes.get(&key) {
            return *b;
        }
        let (lo, hi) = key
            .parent()
            .and_then(|p| self.nodes.get(&p))
            .map(|p| (p.min_height, p.max_height))
            .unwrap_or(self.height_range);
        let b = self.estimate(key, lo, hi);
        self.nodes.insert(key, b);
        b
    }

    /// Whether `q` (terrain-local) is hidden behind a sphere of radius `occluder` around the
    /// terrain origin, seen from `cam`.
    fn behind_horizon(cam: DVec3, q: DVec3, occluder: f64) -> bool {
        let d = q - cam;
        let len = d.length();
        if len < 1e-6 {
            return false;
        }
        let dir = d / len;
        let b = cam.dot(dir);
        let c = cam.length_squared() - occluder * occluder;
        if c <= 0.0 {
            return false;
        }
        let disc = b * b - c;
        if disc <= 0.0 {
            return false;
        }
        let t0 = -b - disc.sqrt();
        t0 > 0.0 && t0 < len
    }

    /// Whether all of patch `key`, up to its top (`b.max_height`), is hidden from `cam` behind
    /// a sphere of radius `occluder`. On a sphere, whether a point at a given height is hidden
    /// depends only on its angle from the eye, so the patch is hidden exactly when its point
    /// nearest the eye's direction is: the eye's own when the eye is over the patch, else a
    /// point on an edge. (Its corners and centre alone missed the near edge of a patch hundreds
    /// of km across, even the one the eye stands on.) Never on flat terrain, nor for levels 0
    /// to 2.
    fn horizon_culled(&self, key: NodeKey, b: &NodeBounds, cam: DVec3, occluder: f64) -> bool {
        if !self.map.is_sphere() || key.level < 3 {
            return false;
        }
        let Some(eye) = cam.try_normalize() else {
            return false;
        };
        let dir = nearest_on_patch(eye, &patch_corners(&self.map, key));
        Self::behind_horizon(cam, dir * (self.map.radius + b.max_height), occluder)
    }

    /// Whether a walk from `cam` (terrain-local) passes over patch `key`, bounded by `b`, as
    /// behind the planet's horizon: no walk from there builds it. Never on flat terrain, nor
    /// for levels 0 to 2.
    pub fn behind_horizon_from(&self, key: NodeKey, b: &NodeBounds, cam: DVec3) -> bool {
        self.horizon_culled(key, b, cam, self.occluder())
    }

    /// Radius of the sphere the horizon is reckoned against: the planet's lowest point.
    fn occluder(&self) -> f64 {
        self.map.radius + self.height_range.0.min(0.0)
    }

    /// Walk the quadtree for a new frame.
    pub fn select(&mut self, input: &SelectInput) -> Selection {
        self.frame += 1;
        self.walk(input)
    }

    /// Walk the quadtree again in the same frame (another view of it, such as a shadow map).
    /// Patches it visits count as seen this frame.
    pub fn walk(&mut self, input: &SelectInput) -> Selection {
        let mut out = Selection::default();
        let occluder = self.occluder();
        let mut stack: Vec<NodeKey> = (0..self.map.roots()).map(NodeKey::root).collect();
        while let Some(key) = stack.pop() {
            out.visited += 1;
            let b = self.bounds(key);
            if let Some(n) = self.nodes.get_mut(&key) {
                n.last_seen = self.frame;
            }
            if !(input.visible)(b.center, b.radius)
                || self.horizon_culled(key, &b, input.camera, occluder)
            {
                continue;
            }
            let dist = ((input.camera - b.center).length() - b.radius).max(0.0);
            if !b.built {
                // Only roots get here unbuilt (children are checked before splitting).
                out.missing.push((dist, key));
                continue;
            }
            let size = self.map.node_size(key.level);
            let detail = input.detail.map_or(1.0, |d| d(dist, b.center));
            let factor = match input.merge_factor {
                Some(m) if b.split => m.max(input.split_factor),
                _ => input.split_factor,
            } * detail;
            let split =
                key.level < self.max_level && dist < factor * size && size * 0.5 >= input.min_size;
            if split {
                let children = key.children();
                let mut ready = true;
                for c in children {
                    let cb = self.bounds(c);
                    if let Some(n) = self.nodes.get_mut(&c) {
                        n.last_seen = self.frame;
                    }
                    if cb.built {
                        continue;
                    }
                    // Children behind the horizon don't hold the split back. Children outside
                    // the frustum do, so turning the camera never reveals a hole.
                    if self.horizon_culled(c, &cb, input.camera, occluder) {
                        continue;
                    }
                    ready = false;
                    let cd = ((input.camera - cb.center).length() - cb.radius).max(0.0);
                    out.missing.push((cd, c));
                }
                if ready {
                    stack.extend(children);
                    if input.merge_factor.is_some()
                        && let Some(n) = self.nodes.get_mut(&key)
                    {
                        n.split = true;
                    }
                    continue;
                }
            }
            if input.merge_factor.is_some()
                && let Some(n) = self.nodes.get_mut(&key)
            {
                n.split = false;
            }
            out.draw.push(key);
            out.clear_count += if key.level < self.max_level {
                1.0 / detail.max(0.03)
            } else {
                1.0
            };
        }
        out.missing
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.level.cmp(&b.1.level)));
        out.missing.dedup_by_key(|m| m.1);
        out
    }

    /// Drop bounds of patches not visited for `frames` frames that have no mesh. Returns built
    /// patches unused that long (the caller frees their meshes and calls [`Lod::unbuild`]).
    pub fn stale(&mut self, frames: u64) -> Vec<NodeKey> {
        let now = self.frame;
        let mut out = Vec::new();
        self.nodes.retain(|k, n| {
            let old = now.saturating_sub(n.last_seen) > frames;
            if old && n.built && k.level > 0 {
                out.push(*k);
            }
            !(old && !n.built)
        });
        out
    }

    /// Patches whose bounds touch a terrain-local sphere (for rebuilding after edits).
    pub fn touching(&self, center: DVec3, radius: f64) -> Vec<NodeKey> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.built && (n.center - center).length() <= n.radius + radius)
            .map(|(k, _)| *k)
            .collect()
    }
}

/// The unit directions of a sphere's patch's corners, in order around it. Lines of constant
/// face coordinate are great circles, so its edges are arcs of them.
fn patch_corners(map: &SurfaceMap, key: NodeKey) -> [DVec3; 4] {
    let (lo, size) = key.face_rect();
    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .map(|(i, j)| map.base(key.face, lo.x + size * i, lo.y + size * j).1)
}

/// The unit direction in the spherical quadrilateral with unit corners `c` (in order around
/// it, each edge an arc of a great circle shorter than half a turn) nearest the unit direction
/// `d`: `d` itself when it lies inside, else the nearest point of an edge.
fn nearest_on_patch(d: DVec3, c: &[DVec3; 4]) -> DVec3 {
    let edges = [(c[0], c[1]), (c[1], c[2]), (c[2], c[3]), (c[3], c[0])];
    let mid = c[0] + c[1] + c[2] + c[3];
    // Inside: on the patch's side of every edge's great circle.
    if edges.iter().all(|&(a, b)| {
        let n = a.cross(b);
        n.dot(d) * n.dot(mid) >= 0.0
    }) {
        return d;
    }
    edges
        .iter()
        .map(|&(a, b)| nearest_on_arc(d, a, b))
        .max_by(|p, q| p.dot(d).total_cmp(&q.dot(d)))
        .unwrap_or(c[0])
}

/// The unit direction on the shorter great circle arc from unit `a` to unit `b` nearest the
/// unit direction `d`: the foot of `d` on the arc's great circle when it lies between them,
/// else the nearer end.
fn nearest_on_arc(d: DVec3, a: DVec3, b: DVec3) -> DVec3 {
    let n = a.cross(b);
    let nn = n.length_squared();
    if nn > 0.0
        && let Some(p) = (d - n * (d.dot(n) / nn)).try_normalize()
        && a.cross(p).dot(n) >= 0.0
        && p.cross(b).dot(n) >= 0.0
    {
        return p;
    }
    if d.dot(a) >= d.dot(b) { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cube::TerrainDomain;

    /// Pretend every requested patch gets built at sea level.
    fn settle(lod: &mut Lod, cam: DVec3) -> Selection {
        let visible = |_: DVec3, _: f64| true;
        for _ in 0..64 {
            let sel = lod.select(&SelectInput {
                camera: cam,
                split_factor: 3.0,
                merge_factor: Some(3.0),
                visible: &visible,
                min_size: 0.0,
                detail: None,
            });
            if sel.missing.is_empty() {
                return sel;
            }
            for (_, k) in sel.missing {
                let b = lod.estimate(k, 0.0, 10.0);
                lod.set_built(k, 0.0, 10.0, b.center, b.radius);
            }
        }
        panic!("selection did not settle");
    }

    #[test]
    fn detail_concentrates_under_the_camera() {
        let r = 1_000_000.0;
        let map = SurfaceMap::new(TerrainDomain::Sphere, r);
        let mut lod = Lod::new(map, (-100.0, 100.0), 1.0);
        let cam = DVec3::new(0.0, r + 2.0, 0.0);
        let sel = settle(&mut lod, cam);
        let finest = sel.draw.iter().map(|k| k.level).max().unwrap();
        assert_eq!(finest, lod.max_level);
        // The patch under the camera is the finest one.
        let under = map.node_at(cam, finest).unwrap();
        assert!(sel.draw.contains(&under));
        // Far side of the planet: hidden by the horizon, nothing fine there.
        let far = map.node_at(-cam, 4).unwrap();
        assert!(!sel.draw.iter().any(|k| k.face == far.face && k.level > 4));
        // About 3πK² patches per level (K = split factor) inside the horizon; frustum culling
        // removes most of them in practice.
        assert!(sel.draw.len() < 1500, "{} patches", sel.draw.len());
    }

    #[test]
    fn from_orbit_few_patches() {
        let r = 1_000_000.0;
        let map = SurfaceMap::new(TerrainDomain::Sphere, r);
        let mut lod = Lod::new(map, (-100.0, 100.0), 1.0);
        let sel = settle(&mut lod, DVec3::new(0.0, r * 4.0, 0.0));
        assert!(sel.draw.len() < 120, "{}", sel.draw.len());
        assert!(sel.draw.iter().all(|k| k.level < 6));
    }

    #[test]
    fn a_second_walk_keeps_the_frame_and_stops_at_its_minimum_size() {
        let r = 1_000_000.0;
        let map = SurfaceMap::new(TerrainDomain::Sphere, r);
        let mut lod = Lod::new(map, (-100.0, 100.0), 1.0);
        let cam = DVec3::new(0.0, r + 2.0, 0.0);
        settle(&mut lod, cam);
        let frame = lod.frame;
        let visible = |_: DVec3, _: f64| true;
        let sel = lod.walk(&SelectInput {
            camera: cam,
            split_factor: 3.0,
            merge_factor: None,
            visible: &visible,
            min_size: 200.0,
            detail: None,
        });
        assert_eq!(lod.frame, frame);
        // Coarser than the view's walk, so everything it wants is already built.
        assert!(sel.missing.is_empty());
        let smallest = sel
            .draw
            .iter()
            .map(|k| map.node_size(k.level))
            .fold(f64::INFINITY, f64::min);
        assert!((200.0..400.0).contains(&smallest), "{smallest}");
        // What it visits counts as seen this frame.
        let finest = sel.draw.iter().map(|k| k.level).max().unwrap();
        let under = map.node_at(cam, finest).unwrap();
        assert!(sel.draw.contains(&under));
        assert_eq!(lod.nodes[&under].last_seen, frame);
    }

    /// A planet the size of the Earth, its lowest point 5,075 m down (as the home planet's).
    fn earth() -> (SurfaceMap, Lod) {
        let map = SurfaceMap::new(TerrainDomain::Sphere, 6.371e6);
        (map, Lod::new(map, (-5075.0, 3000.0), 0.5))
    }

    /// The patch under the eye is never behind the horizon, at any level, wherever on the face
    /// the eye stands 2 m above ground 0 to 10 m high. A level-3 patch is 1,250 km across: from
    /// near the middle of one of its edges its corners and centre are all past the horizon
    /// (about 510 km off), and those five points alone used to decide.
    #[test]
    fn the_patch_under_the_eye_is_never_behind_the_horizon() {
        let (map, lod) = earth();
        let n = 128;
        let t = |k: usize| -1.0 + (k as f64 + 0.5) * 2.0 / n as f64;
        for i in 0..n {
            for j in 0..n {
                let eye = map.position(2, t(i), t(j), 2.0);
                for level in 3..=lod.max_level {
                    let k = map.node_at(eye, level).unwrap();
                    let b = lod.estimate(k, 0.0, 10.0);
                    assert!(!lod.behind_horizon_from(k, &b, eye), "{k:?} under {i}, {j}");
                }
            }
        }
    }

    /// The point a patch is reckoned by is in the patch and as near the eye as any point of it
    /// (a grid of 129 by 129), with the eye over it, beside an edge, past a corner, over
    /// another face or on the far side.
    #[test]
    fn a_patchs_point_nearest_the_eye_is_its_nearest() {
        use crate::cube::FACES;
        let map = SurfaceMap::new(TerrainDomain::Sphere, 1.0);
        let keys = [
            (2, 3, 5, 5),
            (2, 3, 0, 7),
            (4, 5, 0, 17),
            (0, 9, 300, 511),
            (5, 14, 9000, 4321),
        ]
        .map(|(face, level, x, y)| NodeKey { face, level, x, y });
        let offsets = [
            -40.0, -1.5, -0.6, -0.5, -0.45, 0.0, 0.3, 0.5, 0.55, 1.0, 7.0,
        ];
        for key in keys {
            let (lo, size) = key.face_rect();
            let corners = patch_corners(&map, key);
            let mut eyes = vec![
                DVec3::X,
                DVec3::NEG_Y,
                DVec3::new(-1.0, 2.0, 3.0).normalize(),
            ];
            for da in offsets {
                for db in offsets {
                    let a = (lo.x + size * (0.5 + da)).clamp(-1.9, 1.9);
                    let b = (lo.y + size * (0.5 + db)).clamp(-1.9, 1.9);
                    eyes.push(map.base(key.face, a, b).1);
                }
            }
            let (nf, uf, vf) = FACES[key.face as usize];
            for eye in eyes {
                let got = nearest_on_patch(eye, &corners);
                assert!((got.length() - 1.0).abs() < 1e-12);
                let k = got.dot(nf);
                let a = (got.dot(uf) / k).atan() / std::f64::consts::FRAC_PI_4;
                let b = (got.dot(vf) / k).atan() / std::f64::consts::FRAC_PI_4;
                let tol = 1e-9;
                assert!(k > 0.0, "{key:?} {eye}");
                assert!(
                    (lo.x - tol..=lo.x + size + tol).contains(&a),
                    "{key:?} {eye}: a {a}"
                );
                assert!(
                    (lo.y - tol..=lo.y + size + tol).contains(&b),
                    "{key:?} {eye}: b {b}"
                );
                let m = 128;
                let mut best = f64::MIN;
                for i in 0..=m {
                    for j in 0..=m {
                        let (fa, fb) = (i as f64 / m as f64, j as f64 / m as f64);
                        let d = map.base(key.face, lo.x + size * fa, lo.y + size * fb).1;
                        best = best.max(d.dot(eye));
                    }
                }
                assert!(got.dot(eye) >= best - 1e-12, "{key:?} {eye}");
            }
        }
    }

    /// On an Earth-sized planet, an eye 2 m up and about 25 km from the middle of a level-3
    /// patch's edge gets the finest patch under it, and the ground across that edge is drawn
    /// as finely as its distance asks. (Both level-3 patches there used to be culled: no
    /// ground at all.)
    #[test]
    fn the_ground_under_an_eye_near_a_large_patchs_edge_is_drawn() {
        let (map, mut lod) = earth();
        // Level 3 on face 2: x 5 spans a from 0.25 to 0.5, and y 5 spans b from 0.25 to 0.5.
        let eye = map.position(2, 0.255, 0.375, 2.0);
        let sel = settle(&mut lod, eye);
        let under = map.node_at(eye, lod.max_level).unwrap();
        assert!(sel.draw.contains(&under));
        let across = map.position(2, 0.2499, 0.375, 0.0);
        let level = sel
            .draw
            .iter()
            .filter(|k| map.node_at(across, k.level) == Some(**k))
            .map(|k| k.level)
            .max();
        // About 23 km off (its bounding sphere), the view's walk draws patches of about 5 km
        // here: level 11 (a level-10 patch splits within 3 x 9.8 = 29 km).
        assert_eq!(level, Some(11));
    }

    /// No patch is culled while any of it is in view: from eyes 2 m up all over a face, every
    /// patch of levels 3 to 5 near the horizon (all faces) against a 17 by 17 grid of its
    /// points just under its top, each tested alone. Three cases a test of fewer points
    /// missed: a patch 192 km off reckoned by its diagonals (corners out of order), and two
    /// reckoned by the eye's face coordinates clamped into them (485 and 491 km off, inside
    /// the horizon of about 509 km).
    #[test]
    fn no_patch_in_view_is_behind_the_horizon() {
        let (map, lod) = earth();
        let occluder = lod.occluder();
        let culled = |face: u8, level: u8, x: u32, y: u32, eye: DVec3| {
            let k = NodeKey { face, level, x, y };
            lod.behind_horizon_from(k, &lod.estimate(k, 0.0, 10.0), eye)
        };
        assert!(!culled(2, 3, 4, 5, map.position(2, 0.29, 0.375, 2.0)));
        assert!(!culled(2, 5, 3, 5, map.position(2, -0.8, -0.8, 2.0)));
        assert!(!culled(2, 4, 15, 2, map.position(2, 0.9, -0.5, 2.0)));
        let horizon = 2.0 * (2.0 * map.radius * 5085.0).sqrt();
        let mut in_view = 0;
        for i in 0..16 {
            for j in 0..16 {
                let t = |k: usize| -1.0 + (k as f64 + 0.5) / 8.0;
                let eye = map.position(2, t(i), t(j), 2.0);
                for face in 0..6u8 {
                    for level in 3..=5u8 {
                        let n = 1u32 << level;
                        for (x, y) in (0..n * n).map(|c| (c % n, c / n)) {
                            let k = NodeKey { face, level, x, y };
                            let (lo, size) = k.face_rect();
                            let mid = map.position(face, lo.x + size * 0.5, lo.y + size * 0.5, 0.0);
                            if (mid - eye).length() > horizon + 1.5 * map.node_size(level) {
                                continue;
                            }
                            let m = 16;
                            let seen = (0..=m * m + m).any(|c| {
                                let (fa, fb) = ((c % (m + 1)) as f64, (c / (m + 1)) as f64);
                                let (a, b) =
                                    (lo.x + size * fa / m as f64, lo.y + size * fb / m as f64);
                                let q = map.position(face, a, b, 10.0 - 0.01);
                                !Lod::behind_horizon(eye, q, occluder)
                            });
                            if seen {
                                in_view += 1;
                                assert!(!culled(face, level, x, y, eye), "{k:?} from {i}, {j}");
                            }
                        }
                    }
                }
            }
        }
        assert!(in_view > 5000, "{in_view}");
    }

    #[test]
    fn horizon_test() {
        let cam = DVec3::new(0.0, 110.0, 0.0);
        assert!(Lod::behind_horizon(
            cam,
            DVec3::new(0.0, -100.0, 0.0),
            100.0
        ));
        assert!(!Lod::behind_horizon(
            cam,
            DVec3::new(30.0, 100.0, 0.0),
            100.0
        ));
    }

    #[test]
    fn split_patches_merge_back_only_past_the_hysteresis() {
        let r = 1_000_000.0;
        let map = SurfaceMap::new(TerrainDomain::Sphere, r);
        let mut lod = Lod::new(map, (-100.0, 100.0), 1.0);
        let cam = DVec3::new(0.0, r + 2.0, 0.0);
        let near = settle(&mut lod, cam);
        let visible = |_: DVec3, _: f64| true;
        let walk = |lod: &mut Lod, split: f64| {
            lod.select(&SelectInput {
                camera: cam,
                split_factor: split,
                merge_factor: Some(split * 1.15),
                visible: &visible,
                min_size: 0.0,
                detail: None,
            })
        };
        // A little less detail asked for: what is split stays split.
        let same = walk(&mut lod, 3.0 / 1.1);
        assert_eq!(same.draw.len(), near.draw.len());
        // Much less: it merges.
        let coarser = walk(&mut lod, 3.0 / 1.5);
        assert!(coarser.draw.len() < near.draw.len());
    }
}
