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

    fn horizon_culled(&self, key: NodeKey, b: &NodeBounds, cam: DVec3, occluder: f64) -> bool {
        if !self.map.is_sphere() || key.level < 3 {
            return false;
        }
        let (lo, size) = key.face_rect();
        let top = b.max_height;
        for (i, j) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
            let q = self
                .map
                .position(key.face, lo.x + size * i, lo.y + size * j, top);
            if !Self::behind_horizon(cam, q, occluder) {
                return false;
            }
        }
        true
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
        let occluder = self.map.radius + self.height_range.0.min(0.0);
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
            let factor = match input.merge_factor {
                Some(m) if b.split => m.max(input.split_factor),
                _ => input.split_factor,
            };
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
