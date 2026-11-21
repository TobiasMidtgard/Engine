//! What the hand shows while building, besides the ghost itself ([`crate::tools`]): where a
//! building fits (a grid of free cells round the cursor), the berths along a coast that quays
//! snap to, and how far a public building reaches (the streets it serves, the houses in its
//! reach and the ones still without it).

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use engine::prelude::*;
use engine::render::Vertex;

use crate::camera::StrategyCam;
use crate::defs::{CELL_M, Kind, Site};
use crate::draw::{MapRow, Tiled};
use crate::grid::{self, Grid};
use crate::island;
use crate::sim::{Building, House};
use crate::tools::Tool;

// ---------------------------------------------------------------------------------------
// Berths

/// A place on a coast where a quay of one kind fits: its origin and turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Berth {
    pub origin: IVec2,
    pub rot: u8,
}

type BerthMap = HashMap<(Kind, usize), Arc<Vec<Berth>>>;
static BERTHS: LazyLock<RwLock<BerthMap>> = LazyLock::new(Default::default);
static FINDING: LazyLock<Mutex<HashSet<(Kind, usize)>>> = LazyLock::new(Default::default);

/// The berths for `kind` round island `isle`, once found. They are looked for on another
/// thread the first time they are asked for (the coast never changes), so this is `None`
/// for a moment.
pub fn berths(kind: Kind, isle: usize) -> Option<Arc<Vec<Berth>>> {
    if let Some(b) = BERTHS
        .read()
        .ok()
        .and_then(|m| m.get(&(kind, isle)).cloned())
    {
        return Some(b);
    }
    if FINDING.lock().is_ok_and(|mut f| f.insert((kind, isle))) {
        std::thread::spawn(move || {
            let found = Arc::new(find_berths(kind, isle));
            if let Ok(mut m) = BERTHS.write() {
                m.insert((kind, isle), found);
            }
        });
    }
    None
}

/// Look for every island's harbour berths now (a thread each), so they are ready when the
/// player first takes up a harbour.
pub fn warm_berths() {
    for isle in 0..island::islands().len() {
        let _ = berths(Kind::Harbour, isle);
    }
}

/// How many berth sets are known (the overlay redraws when one arrives).
fn berths_known() -> usize {
    BERTHS.read().map_or(0, |m| m.len())
}

/// The turn whose front faces `out` most nearly.
fn facing(out: DVec2) -> u8 {
    (0..4u8)
        .max_by(|a, b| {
            let da = grid::front_dir(*a).as_dvec2().dot(out);
            let db = grid::front_dir(*b).as_dvec2().dot(out);
            da.total_cmp(&db)
        })
        .unwrap_or(0)
}

/// Where a ray from the island's middle last leaves its land for the sea (its outer shore).
fn shore(isle: usize, dir: DVec2) -> Option<DVec2> {
    let il = &island::islands()[isle];
    let end = il.radius * 2.2;
    let step = (il.radius / 160.0).max(8.0);
    let at = |t: f64| il.center + dir * t;
    let land = |t: f64| {
        let p = at(t);
        island::height(p.x, p.y) > 0.0
    };
    let mut t = il.radius * 0.2;
    let mut was_land = land(t);
    let mut cross = None;
    while t < end {
        t += step;
        let now = land(t);
        if was_land && !now {
            let p = at(t);
            if island::nearest_island(p.x, p.y) == isle {
                cross = Some(t);
            }
        }
        was_land = now;
    }
    let t1 = cross?;
    let (mut lo, mut hi) = (t1 - step, t1);
    for _ in 0..5 {
        let m = (lo + hi) * 0.5;
        if land(m) {
            lo = m;
        } else {
            hi = m;
        }
    }
    Some(at(lo))
}

/// Berths round `isle` for `kind`: walk the outer shore a cell at a time, try the quay at
/// each shore point facing the open water (as far inland as it still fits), then keep the
/// ones that leave a cell between each other, in order round the coast.
pub fn find_berths(kind: Kind, isle: usize) -> Vec<Berth> {
    if kind.def().site != Site::Coast || isle >= island::islands().len() {
        return Vec::new();
    }
    let il = &island::islands()[isle];
    let empty = Grid::default();
    let tau = std::f64::consts::TAU;
    let n = ((tau * il.radius * 1.3) / CELL_M)
        .ceil()
        .clamp(64.0, 2400.0) as usize;
    let mut found: Vec<Berth> = Vec::new();
    for k in 0..n {
        let a = k as f64 / n as f64 * tau;
        let dir = DVec2::new(a.cos(), a.sin());
        let Some(p) = shore(isle, dir) else {
            continue;
        };
        // The open water lies down the slope.
        let e = 6.0;
        let down = -DVec2::new(
            island::height(p.x + e, p.y) - island::height(p.x - e, p.y),
            island::height(p.x, p.y + e) - island::height(p.x, p.y - e),
        );
        let out = if down.length() > 1e-6 {
            down.normalize()
        } else {
            dir
        };
        let rot = facing(out);
        let f = grid::front_dir(rot).as_dvec2();
        let (w, d) = kind.footprint(rot);
        let depth = (if f.x != 0.0 { w } else { d }) as f64;
        for s in -1..=3 {
            // The footprint's middle: half its depth inland of the shore, then `s` cells out.
            let c = p - f * (depth * 0.5 - s as f64) * CELL_M;
            let origin = grid::cell_of(c.x, c.y) - IVec2::new(w / 2, d / 2);
            let mid = grid::footprint_center(kind, origin, rot);
            if island::nearest_island(mid.x, mid.y) == isle
                && grid::check(&empty, kind, origin, rot).is_ok()
            {
                found.push(Berth { origin, rot });
                break;
            }
        }
    }
    let mut taken: HashSet<IVec2> = HashSet::new();
    let mut out = Vec::new();
    for b in found {
        let (w, d) = kind.footprint(b.rot);
        let crowded =
            (-1..=d).any(|z| (-1..=w).any(|x| taken.contains(&(b.origin + IVec2::new(x, z)))));
        if crowded {
            continue;
        }
        taken.extend(grid::footprint(kind, b.origin, b.rot));
        out.push(b);
    }
    out
}

/// Whether nothing stands on a berth now.
fn berth_free(g: &Grid, kind: Kind, b: &Berth) -> bool {
    grid::footprint(kind, b.origin, b.rot)
        .iter()
        .all(|c| g.is_free(*c))
}

/// The free berth for `kind` nearest the map point `at`, if the cursor is near enough to
/// one for the quay to snap to it.
pub fn snap(world: &World, kind: Kind, at: DVec2) -> Option<Berth> {
    if kind.def().site != Site::Coast {
        return None;
    }
    let list = berths(kind, island::nearest_island(at.x, at.y))?;
    let g = world.resource::<Grid>();
    let (w, d) = kind.footprint(0);
    let reach = (w.max(d) as f64 * CELL_M).max(24.0);
    list.iter()
        .filter(|b| berth_free(g, kind, b))
        .map(|b| {
            (
                b,
                (grid::footprint_center(kind, b.origin, b.rot) - at).length(),
            )
        })
        .filter(|(_, dist)| *dist <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| *b)
}

// ---------------------------------------------------------------------------------------
// Reach of public buildings

/// How a public building reaches: how much further than its radius along the streets, and
/// the colour its reach is shown in.
fn reach_style(kind: Kind) -> Option<(f32, [f32; 3])> {
    let street = crate::build::STREET_REACH;
    let station = crate::incidents::STATION_STREET_REACH;
    Some(match kind {
        Kind::Market => (street, [1.0, 0.68, 0.14]),
        Kind::Chapel => (street, [0.72, 0.56, 1.0]),
        Kind::School => (street, [0.3, 0.74, 1.0]),
        Kind::Tavern => (street, [1.0, 0.46, 0.22]),
        Kind::FireWatch => (station, [1.0, 0.42, 0.26]),
        Kind::WatchHouse => (station, [0.48, 0.62, 1.0]),
        Kind::Infirmary => (station, [0.55, 1.0, 0.76]),
        _ => return None,
    })
}

/// Whether `kind` serves the buildings round it (and so shows its reach).
pub fn has_reach(kind: Kind) -> bool {
    reach_style(kind).is_some()
}

/// What a public building turned `rot` at `origin` reaches.
pub struct Reach {
    pub centre: DVec2,
    pub radius_m: f64,
    /// Road cells along its streets within its street reach.
    pub roads: Vec<IVec2>,
    /// The buildings it serves: houses for a market, chapel, school or tavern, every building
    /// for a fire watch, watch house or infirmary.
    pub served: Vec<Building>,
    /// Houses on its island near it that want it but no building of its kind serves yet
    /// (markets, chapels, schools and taverns).
    pub lacking: Vec<Building>,
    pub colour: [f32; 3],
}

/// The reach of `kind` turned `rot` at `origin` on island `isle` (`me`: the building itself,
/// when it already stands).
pub fn reach(
    world: &mut World,
    kind: Kind,
    origin: IVec2,
    rot: u8,
    isle: usize,
    me: Option<Entity>,
) -> Option<Reach> {
    let (factor, colour) = reach_style(kind)?;
    let centre = grid::footprint_center(kind, origin, rot);
    let radius_m = kind.def().reach as f64 * CELL_M;
    let budget = kind.def().reach as f32 * factor;
    let roads = {
        let g = world.resource::<Grid>();
        street_cells(g, grid::ring(kind, origin, rot), budget)
    };
    let on_road: HashSet<IVec2> = roads.iter().copied().collect();
    let service = crate::build::SERVICES.iter().position(|k| *k == kind);
    let mut served = Vec::new();
    let mut lacking = Vec::new();
    let mut q = world.query::<(Entity, &Building, Option<&House>)>();
    for (e, b, house) in q.iter(world) {
        if b.island != isle || Some(e) == me {
            continue;
        }
        if service.is_some() && house.is_none() {
            continue;
        }
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let reached = (c - centre).length() <= radius_m
            || grid::ring(b.kind, b.origin, b.rot)
                .iter()
                .any(|c| on_road.contains(c));
        if reached {
            served.push(b.clone());
        } else if let (Some(i), Some(h)) = (service, house)
            && !h.services[i]
            && h.tier.services().contains(&kind)
            && (c - centre).length() <= radius_m * 2.5
        {
            lacking.push(b.clone());
        }
    }
    Some(Reach {
        centre,
        radius_m,
        roads,
        served,
        lacking,
        colour,
    })
}

/// Road cells within `budget` steps of the `door` cells along the streets (a paved cell
/// counts [`crate::defs::PAVED_REACH_COST`] of a step), as the services measure it.
fn street_cells(g: &Grid, door: Vec<IVec2>, budget: f32) -> Vec<IVec2> {
    let mut best: HashMap<IVec2, f32> = HashMap::new();
    let mut queue = VecDeque::new();
    for c in door {
        if g.is_road(c) {
            best.insert(c, 0.0);
            queue.push_back(c);
        }
    }
    while let Some(c) = queue.pop_front() {
        let d = best[&c];
        for n in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let nc = c + n;
            if !g.is_road(nc) {
                continue;
            }
            let step = if g.is_paved(nc) {
                crate::defs::PAVED_REACH_COST
            } else {
                1.0
            };
            let nd = d + step;
            if nd <= budget && best.get(&nc).is_none_or(|old| nd < *old) {
                best.insert(nc, nd);
                queue.push_back(nc);
            }
        }
    }
    best.into_keys().collect()
}

// ---------------------------------------------------------------------------------------
// Smart roads

/// The road a drag from `a` to `b` lays: the cheapest way round what stands in between,
/// taking existing roads where they help, crossing fields only when it must, with as few
/// turns as it can. `None` when no way is open within reach.
pub fn road_path(g: &Grid, a: IVec2, b: IVec2) -> Option<Vec<IVec2>> {
    road_path_around(g, &HashSet::new(), a, b)
}

/// [`road_path`], keeping off the cells in `kept` (the ground blueprints keep).
pub fn road_path_around(g: &Grid, kept: &HashSet<IVec2>, a: IVec2, b: IVec2) -> Option<Vec<IVec2>> {
    let open =
        |c: IVec2| !kept.contains(&c) && (g.is_road(c) || (g.is_open(c) && grid::cell_roadable(c)));
    if !open(a) || !open(b) {
        return None;
    }
    if a == b {
        return Some(vec![a]);
    }
    let span = (b - a).abs();
    if span.x + span.y > 400 {
        return None;
    }
    let margin = 24;
    let lo = a.min(b) - IVec2::splat(margin);
    let hi = a.max(b) + IVec2::splat(margin);
    let cost = |c: IVec2| -> f32 {
        if g.is_road(c) {
            0.8
        } else if g.is_free(c) {
            1.0
        } else {
            // A field: the farm loses that cell.
            4.0
        }
    };
    const TURN: f32 = 0.45;
    let dirs = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y];
    let h = |c: IVec2| ((b - c).abs().x + (b - c).abs().y) as f32 * 0.8;
    // A* over (cell, heading): the heading makes turns cost.
    #[derive(PartialEq)]
    struct Open(f32, u32);
    impl Eq for Open {}
    impl PartialOrd for Open {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Open {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
        }
    }
    let mut nodes: Vec<(IVec2, u8)> = Vec::new();
    let mut index: HashMap<(IVec2, u8), u32> = HashMap::new();
    let mut gcost: Vec<f32> = Vec::new();
    let mut came: Vec<u32> = Vec::new();
    let mut heap = std::collections::BinaryHeap::new();
    for (k, _) in dirs.iter().enumerate() {
        let id = nodes.len() as u32;
        nodes.push((a, k as u8));
        index.insert((a, k as u8), id);
        gcost.push(0.0);
        came.push(u32::MAX);
        heap.push(Open(h(a), id));
    }
    let mut done: HashSet<u32> = HashSet::new();
    let mut end = None;
    while let Some(Open(_, id)) = heap.pop() {
        if !done.insert(id) {
            continue;
        }
        let (c, dir) = nodes[id as usize];
        if c == b {
            end = Some(id);
            break;
        }
        if done.len() > 120_000 {
            break;
        }
        for (k, d) in dirs.iter().enumerate() {
            let n = c + *d;
            if n.x < lo.x || n.y < lo.y || n.x > hi.x || n.y > hi.y || !open(n) {
                continue;
            }
            let turn = if k as u8 == dir || came[id as usize] == u32::MAX {
                0.0
            } else {
                TURN
            };
            let g2 = gcost[id as usize] + cost(n) + turn;
            let key = (n, k as u8);
            let nid = match index.get(&key) {
                Some(&x) => {
                    if g2 >= gcost[x as usize] {
                        continue;
                    }
                    gcost[x as usize] = g2;
                    came[x as usize] = id;
                    x
                }
                None => {
                    let x = nodes.len() as u32;
                    nodes.push(key);
                    index.insert(key, x);
                    gcost.push(g2);
                    came.push(id);
                    x
                }
            };
            heap.push(Open(g2 + h(n), nid));
        }
    }
    let mut id = end?;
    let mut path = vec![nodes[id as usize].0];
    while came[id as usize] != u32::MAX {
        id = came[id as usize];
        path.push(nodes[id as usize].0);
    }
    path.reverse();
    Some(path)
}

// ---------------------------------------------------------------------------------------
// Drawing

/// The overlay's draw sets: a shape and how strongly it shows.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Mark {
    /// A filled cell.
    Fill(u8),
    /// A cell's outline.
    Frame(u8),
    /// One edge of a cell (the side its yaw faces).
    Edge(u8),
}

/// Alpha of each strength, strongest first.
const ALPHA: [f32; 4] = [0.8, 0.5, 0.3, 0.16];

#[derive(Resource, Default)]
struct Overlay {
    sets: Tiled<Mark>,
    looks: HashMap<Mark, MeshRenderer>,
    made_for: Option<u64>,
}

/// What the hand holds this frame, for the overlay.
pub struct Held {
    pub tool: Tool,
    /// The ground under the cursor.
    pub hit: Option<DVec3>,
    /// The building the ghost shows, if one: kind, origin, turn.
    pub building: Option<(Kind, IVec2, u8)>,
    pub selected: Option<Entity>,
}

fn square(inner: f32) -> Mesh {
    let n: [f32; 3] = Vec3::Y.into();
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: n,
        uv: [x + 0.5, z + 0.5],
        color: [1.0; 4],
    };
    if inner <= 0.0 {
        return Mesh::new(
            vec![v(-0.5, -0.5), v(0.5, -0.5), v(0.5, 0.5), v(-0.5, 0.5)],
            vec![0, 2, 1, 0, 3, 2],
        );
    }
    // A square ring: outer corners 0..4, inner 4..8.
    let i = 0.5 - inner;
    let verts = vec![
        v(-0.5, -0.5),
        v(0.5, -0.5),
        v(0.5, 0.5),
        v(-0.5, 0.5),
        v(-i, -i),
        v(i, -i),
        v(i, i),
        v(-i, i),
    ];
    let mut idx = Vec::new();
    for k in 0..4u32 {
        let (o0, o1, i0, i1) = (k, (k + 1) % 4, 4 + k, 4 + (k + 1) % 4);
        idx.extend([o0, i1, o1, o0, i0, i1]);
    }
    Mesh::new(verts, idx)
}

/// A bar along a cell's +Z edge.
fn edge() -> Mesh {
    let n: [f32; 3] = Vec3::Y.into();
    let v = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: n,
        uv: [x + 0.5, z + 0.5],
        color: [1.0; 4],
    };
    let t = 0.5 - 0.11;
    Mesh::new(
        vec![v(-0.5, t), v(0.5, t), v(0.5, 0.5), v(-0.5, 0.5)],
        vec![0, 2, 1, 0, 3, 2],
    )
}

fn renderer(world: &mut World, m: Mark) -> MeshRenderer {
    let (mesh, band) = match m {
        Mark::Fill(b) => (square(0.0), b),
        Mark::Frame(b) => (square(0.085), b),
        Mark::Edge(b) => (edge(), b),
    };
    let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let mat = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: [1.0, 1.0, 1.0, ALPHA[band.min(3) as usize]],
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            ..Default::default()
        });
    let mut r = MeshRenderer::new(mesh, mat);
    r.cast_shadows = false;
    r
}

/// Rows for the map.
#[derive(Default)]
struct Rows(HashMap<Mark, Vec<MapRow>>, f64);

/// How far over the ground the marks float: a little near, more from afar, where the coarser
/// far ground and the depth's lost precision would bury a mark laid flat on it.
pub fn lift(world: &mut World) -> f64 {
    let d = world
        .query::<&StrategyCam>()
        .iter(world)
        .next()
        .map_or(200.0, |c| c.dist);
    ((d - 150.0).max(0.0) * 0.006) as f64
}

impl Rows {
    fn cell(&mut self, m: Mark, c: IVec2, tint: [f32; 3]) {
        let p = grid::cell_center(c);
        let y = grid::cell_range(c).1.max(0.0) + 0.13 + self.1;
        self.0.entry(m).or_default().push(MapRow {
            at: DVec3::new(p.x, y, p.y),
            yaw: 0.0,
            scale: CELL_M as f32,
            tint,
        });
    }

    /// An outline round a set of cells, `y` meters up (or on each cell's ground).
    fn outline(&mut self, band: u8, cells: &[IVec2], y: Option<f64>, tint: [f32; 3]) {
        let set: HashSet<IVec2> = cells.iter().copied().collect();
        for &c in cells {
            for rot in 0..4u8 {
                if set.contains(&(c + grid::front_dir(rot))) {
                    continue;
                }
                let p = grid::cell_center(c);
                let h = y.unwrap_or_else(|| grid::cell_range(c).1.max(0.0) + 0.14 + self.1);
                self.0.entry(Mark::Edge(band)).or_default().push(MapRow {
                    at: DVec3::new(p.x, h, p.y),
                    yaw: grid::yaw(rot),
                    scale: CELL_M as f32,
                    tint,
                });
            }
        }
    }

    /// A diamond floating over a building's roof.
    fn badge(&mut self, band: u8, b: &Building, tint: [f32; 3]) {
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let (w, d) = b.kind.footprint(b.rot);
        let up = if b.kind == Kind::House { 13.0 } else { 16.0 };
        self.0.entry(Mark::Fill(band)).or_default().push(MapRow {
            at: DVec3::new(c.x, b.base_y + up, c.y),
            yaw: std::f32::consts::FRAC_PI_4,
            scale: (w.min(d) as f32 * CELL_M as f32 * 0.32).clamp(3.0, 5.0),
            tint,
        });
    }

    /// A dashed ring on the ground.
    fn ring(&mut self, band: u8, centre: DVec2, r: f64, tint: [f32; 3]) {
        let n = (std::f64::consts::TAU * r / 4.4) as usize;
        for i in 0..n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let at = centre + DVec2::new(a.cos(), a.sin()) * r;
            let y = grid::cell_range(grid::cell_of(at.x, at.y)).1.max(0.0) + 0.16 + self.1;
            self.0.entry(Mark::Fill(band)).or_default().push(MapRow {
                at: DVec3::new(at.x, y, at.y),
                yaw: -(a as f32),
                scale: 2.3,
                tint,
            });
        }
    }
}

/// A cheap fingerprint of what stands on the map.
fn grid_print(world: &World) -> (usize, usize, usize, u32) {
    let g = world.resource::<Grid>();
    let id = world
        .get_resource::<crate::build::Changed>()
        .map_or(0, |c| c.next_id);
    (g.cells.len(), g.roads.len(), g.paved.len(), id)
}

/// Draw what the hand shows besides the ghost (see the module's notes).
pub fn draw(world: &mut World, held: Held) {
    world.init_resource::<Overlay>();
    let cam_dist = world
        .query::<&StrategyCam>()
        .iter(world)
        .next()
        .map_or(200.0, |c| c.dist);
    let radius = (cam_dist / 8.0).clamp(12.0, 36.0) as i32;
    let cursor = held.hit.map(|h| grid::cell_of(h.x, h.z));
    let key = {
        let mut s = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", held.tool).hash(&mut s);
        cursor.map(|c| (c.x, c.y)).hash(&mut s);
        held.building.map(|(k, o, r)| (k, o.x, o.y, r)).hash(&mut s);
        held.selected.map(|e| e.to_bits()).hash(&mut s);
        grid_print(world).hash(&mut s);
        berths_known().hash(&mut s);
        radius.hash(&mut s);
        s.finish()
    };
    if world.resource::<Overlay>().made_for == Some(key) {
        return;
    }
    let rows = rows(world, &held, cursor, radius);
    let mut ov = std::mem::take(&mut *world.resource_mut::<Overlay>());
    for m in rows.0.keys() {
        if !ov.looks.contains_key(m) {
            let r = renderer(world, *m);
            ov.looks.insert(*m, r);
        }
    }
    let looks = ov.looks.clone();
    ov.sets.set(world, rows.0, |m| looks.get(&m).cloned());
    ov.made_for = Some(key);
    *world.resource_mut::<Overlay>() = ov;
}

/// Whether the company giving orders may build on island `isle` now.
fn ours(world: &World, isle: usize) -> bool {
    crate::settle::claimed(world, isle)
}

fn rows(world: &mut World, held: &Held, cursor: Option<IVec2>, radius: i32) -> Rows {
    let mut rows = Rows(HashMap::new(), lift(world));
    // Where it fits: a grid of free cells round the cursor (for a road: where one may go).
    let road = match held.tool {
        Tool::Build { kind, .. } if kind.def().site == Site::Land => Some(false),
        Tool::Road | Tool::Paving => Some(true),
        _ => None,
    };
    let fits = |g: &Grid, c: IVec2, road: bool| {
        if road {
            g.is_open(c) && !g.is_road(c) && grid::cell_roadable(c)
        } else {
            g.is_free(c) && grid::cell_buildable(c)
        }
    };
    if let (Some(road), Some(c0), Some(hit)) = (road, cursor, held.hit)
        && island::island_at(hit.x, hit.z).is_some_and(|i| ours(world, i))
    {
        let g = world.resource::<Grid>();
        let r2 = (radius * radius) as f32;
        for z in -radius..=radius {
            for x in -radius..=radius {
                let d2 = (x * x + z * z) as f32;
                if d2 > r2 {
                    continue;
                }
                let c = c0 + IVec2::new(x, z);
                if !fits(g, c, road) {
                    continue;
                }
                let t = (d2 / r2).sqrt();
                let band = if t < 0.45 {
                    1
                } else if t < 0.75 {
                    2
                } else {
                    3
                };
                rows.cell(Mark::Frame(band), c, [0.82, 1.0, 0.92]);
            }
        }
    }
    // Berths along the coast for a quay.
    if let (Tool::Build { kind, .. }, Some(hit)) = (held.tool, held.hit)
        && kind.def().site == Site::Coast
    {
        let isle = island::nearest_island(hit.x, hit.z);
        if let Some(list) = berths(kind, isle) {
            // Brighter than white light: amber reads over pale sand and blue sea alike.
            let amber = [1.55, 0.72, 0.1];
            let rim = [1.8, 1.35, 0.55];
            let g = world.resource::<Grid>();
            let chosen = held.building.map(|(_, o, r)| (o, r));
            for b in list.iter().filter(|b| berth_free(g, kind, b)) {
                if chosen == Some((b.origin, b.rot)) {
                    continue;
                }
                let cells = grid::footprint(kind, b.origin, b.rot);
                rows.outline(0, &cells, None, rim);
                for c in &cells {
                    rows.cell(Mark::Fill(1), *c, amber);
                }
                // White dashes along the water side, where ships come alongside.
                for (i, c) in grid::front_row(kind, b.origin, b.rot).iter().enumerate() {
                    if i % 2 == 1 {
                        rows.cell(Mark::Fill(0), *c, [1.7, 1.7, 1.7]);
                    }
                }
            }
        }
    }
    // How far a public building reaches: the one in the hand, or the one selected.
    let subject = match (held.tool, held.building, held.selected) {
        (Tool::Build { kind, .. }, Some((_, origin, rot)), _) if has_reach(kind) => {
            let c = grid::footprint_center(kind, origin, rot);
            island::island_at(c.x, c.y).map(|isle| (kind, origin, rot, isle, None))
        }
        (Tool::Select, _, Some(e)) => world
            .get::<Building>(e)
            .filter(|b| has_reach(b.kind))
            .map(|b| (b.kind, b.origin, b.rot, b.island, Some(e))),
        _ => None,
    };
    if let Some((kind, origin, rot, isle, me)) = subject
        && let Some(r) = reach(world, kind, origin, rot, isle, me)
    {
        let col = r.colour;
        rows.ring(0, r.centre, r.radius_m, col);
        for c in &r.roads {
            rows.cell(Mark::Fill(1), *c, col);
        }
        // A badge over everything it reaches, and a red one over houses still without it.
        for b in &r.served {
            rows.badge(0, b, col);
        }
        for b in &r.lacking {
            rows.badge(0, b, [1.0, 0.3, 0.24]);
        }
        // The others of its kind on the island, faintly.
        let others: Vec<Building> = world
            .query::<(Entity, &Building)>()
            .iter(world)
            .filter(|(e, b)| b.kind == kind && b.island == isle && Some(*e) != me)
            .map(|(_, b)| b.clone())
            .collect();
        for b in others {
            let c = grid::footprint_center(b.kind, b.origin, b.rot);
            rows.ring(2, c, r.radius_m, col);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harbour_berths_ring_the_home_island_and_fit() {
        let list = find_berths(Kind::Harbour, 0);
        assert!(list.len() >= 8, "{} berths round Haverholm", list.len());
        let g = Grid::default();
        let mut seen: HashSet<IVec2> = HashSet::new();
        for b in &list {
            assert!(grid::check(&g, Kind::Harbour, b.origin, b.rot).is_ok());
            for c in grid::footprint(Kind::Harbour, b.origin, b.rot) {
                assert!(seen.insert(c), "berths overlap at {c}");
            }
        }
        // They lie all round the coast, not in one bay.
        let il = &island::islands()[0];
        let mut quarters = [false; 4];
        for b in &list {
            let c = grid::footprint_center(Kind::Harbour, b.origin, b.rot) - il.center;
            let a = c.y.atan2(c.x).rem_euclid(std::f64::consts::TAU);
            quarters[(a / std::f64::consts::FRAC_PI_2) as usize % 4] = true;
        }
        assert!(quarters.iter().all(|q| *q), "{quarters:?}");
    }

    #[test]
    fn smart_roads_go_round_what_stands_in_the_way() {
        // Open ground by the home town, with a wall of buildings across the way.
        let spot = island::home_town_spot();
        let mid = grid::cell_of(spot.x, spot.y);
        let mut g = Grid::default();
        let open =
            |c: IVec2| (0..14).all(|k| (-8..=8).all(|z| grid::cell_roadable(c + IVec2::new(k, z))));
        let a = (0..120)
            .flat_map(|r| {
                (-r..=r).flat_map(move |d| {
                    [
                        IVec2::new(d, r),
                        IVec2::new(d, -r),
                        IVec2::new(r, d),
                        IVec2::new(-r, d),
                    ]
                })
            })
            .map(|d| mid + d)
            .find(|c| open(*c));
        let a = a.expect("flat ground on Haverholm");
        let b = a + IVec2::new(13, 0);
        for z in -4..=4 {
            g.cells.insert(
                a + IVec2::new(6, z),
                grid::Occ::Building(Entity::PLACEHOLDER),
            );
        }
        let path = road_path(&g, a, b).expect("a way round");
        assert_eq!(path.first(), Some(&a));
        assert_eq!(path.last(), Some(&b));
        assert!(path.iter().all(|c| g.is_free(*c)), "through a building");
        for w in path.windows(2) {
            assert_eq!((w[1] - w[0]).abs().element_sum(), 1, "a gap in {path:?}");
        }
        // Round the wall's end, not far afield: 13 across plus 2 × 5 up and back.
        assert!(path.len() <= 13 + 10 + 1 + 2, "{} cells", path.len());
        // A straight drag over open ground stays straight.
        let straight = road_path(&g, a + IVec2::new(0, 6), b + IVec2::new(0, 6)).unwrap();
        assert_eq!(straight.len(), 14);
    }
}
