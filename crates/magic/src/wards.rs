//! Wards you can see (round-8-plan.md, research-wards.md). A ward is a pool of damage it takes
//! for someone. One effect makes three shapes:
//!
//! * a **shell** fitted round a body (a ward on yourself, a touch, a bolt on an ally): the pool
//!   lives in the body's [`Afflictions`] and every hit on the body goes through it first;
//! * a **dome** over an area (ward + area) and a **ward wall** of panels (ward + wall):
//!   standing [`Barrier`]s with pools of their own that stop enemy missiles and beams at their
//!   surface and let friends' spells and every body through, as Overwatch's barriers and Halo 3's
//!   bubble shield do. A dome is hollow: whoever walks in fights inside it.
//!
//! Magic draws them itself, as it draws walls, stone and skeletons: a hex lattice from a geodesic
//! sphere's dual (a Goldberg polyhedron) whose cell edges glow and crowd together toward the
//! outline, so the rim reads brighter than the middle, as a fresnel shader would draw it (Halo
//! Reach's shield, Chris Tchou, GDC 2011). Each hit lights the cells round the point it struck;
//! cells go dark and fall out at half and at a quarter of the pool (Reinhardt's barrier); under
//! a fifth it flickers; when it breaks the cells fly off as glowing shards with a flash, and a
//! body can't be warded again for [`LOCK_SECS`] (Overwatch's 5 s lockout, shortened).
//!
//! An element types the ward ([`ward_cost`]): it soaks hits of its own element for nothing,
//! takes double from its opposite, and pays back whoever strikes it from close by (burn, chill,
//! shock: [`payback_spell`]).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, MaterialRef, Mesh, MeshRef, MeshRenderer, NoMeshLod, NoVirtualGeometry, PointLight,
    StandardMaterial, Vertex,
};
use engine_world::Factions;
use engine_world::physics::{CharacterController, PhysicsQueries, QueryFilter};

use crate::compile::{CastContext, Spell, compile};
use crate::design::{CastType, SpellDesign};
use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, MagicQueue, ground_below, roll};
use crate::schools::{Element, Mastery, Power};
use crate::status::Afflictions;

/// Seconds a broken ward keeps a body from being warded again.
pub const LOCK_SECS: f32 = 3.0;
/// Seconds a shell lasts.
pub const SHELL_SECS: f32 = 12.0;
/// A dome's pool against the spell's ward on one body.
pub const DOME_POOL: f32 = 3.0;
/// Seconds a dome stands.
pub const DOME_SECS: f32 = 10.0;
/// A ward wall panel's pool against the spell's ward on one body.
pub const PANEL_POOL: f32 = 1.2;
/// A ward wall panel, m.
pub const PANEL_W: f32 = 1.7;
pub const PANEL_H: f32 = 2.7;
/// Strikes from closer than this are paid back by an elemental ward.
pub const PAYBACK_M: f32 = 3.5;
/// Seconds a hit flare lasts.
pub const FLARE_SECS: f32 = 0.45;
/// Seconds a shard flies.
pub const SHARD_SECS: f32 = 1.2;
/// Seconds a ward takes to swell in, and to fade when its time is up.
pub const SWELL_SECS: f32 = 0.3;
pub const FADE_SECS: f32 = 0.5;
/// Pool shares at which cells crack out.
pub const CRACKS: [f32; 2] = [0.5, 0.25];
/// Under this share of its pool a ward flickers.
pub const FLICKER_BELOW: f32 = 0.2;
/// Emission of a ward's lattice, a flare and a shard, in nits. Daylight needs thousands to
/// read as light (the particle module's glowing sparks use 18 000 to 45 000).
pub const LATTICE_GLOW: f32 = 11_000.0;
pub const FLARE_GLOW: f32 = 60_000.0;
pub const SHARD_GLOW: f32 = 20_000.0;

// ------------------------------------------------------------------------------- rules

/// A plain ward's colour, and an elemental one's.
pub fn ward_color(element: Option<Element>) -> [f32; 3] {
    match element {
        None => [1.0, 0.72, 0.3],
        Some(Element::Frost) => [0.35, 0.72, 1.0],
        Some(e) => e.color(),
    }
}

/// The element that hurts a ward of `e` twice as much: fire and frost undo each other, fire
/// burns the dark (blood and rot), and lightning's light tears shadow.
pub fn opposite(e: Element) -> Option<Element> {
    match e {
        Element::Fire => Some(Element::Frost),
        Element::Frost | Element::Blood | Element::Blight => Some(Element::Fire),
        Element::Shadow => Some(Element::Storm),
        Element::Storm => None,
    }
}

/// What a hit of `damage` of `hit`'s element costs a ward of `ward`'s: nothing from its own
/// element, double from its opposite, else the damage.
pub fn ward_cost(damage: f32, hit: Option<Element>, ward: Option<Element>) -> f32 {
    match (hit, ward) {
        (Some(h), Some(w)) if h == w => 0.0,
        (Some(h), Some(w)) if opposite(w) == Some(h) => damage * 2.0,
        _ => damage,
    }
}

/// The touch an elemental ward answers a close blow with (fire burns, frost chills toward a
/// freeze, storm shocks), compiled once per element.
pub fn payback_spell(e: Element) -> Arc<Spell> {
    static SPELLS: OnceLock<Vec<Arc<Spell>>> = OnceLock::new();
    let all = SPELLS.get_or_init(|| {
        [
            Element::Fire,
            Element::Frost,
            Element::Storm,
            Element::Blood,
            Element::Blight,
            Element::Shadow,
        ]
        .into_iter()
        .map(|e| {
            compile(
                &SpellDesign::new(&[e.key()], CastType::Touch).named("Ward's answer"),
                CastContext::new(Mastery::Expert, Power::at_level(40.0)),
            )
            .expect("an element's touch compiles")
        })
        .collect()
    });
    let i = match e {
        Element::Fire => 0,
        Element::Frost => 1,
        Element::Storm => 2,
        Element::Blood => 3,
        Element::Blight => 4,
        Element::Shadow => 5,
    };
    all[i].clone()
}

/// Do `a` and `b` (two casts: a barrier's and a missile's) stand on opposite sides? A caster is
/// never against itself; two factions follow the engine's `Factions`; else magic's sides.
pub fn against(a: &CastInfo, b: &CastInfo, factions: Option<&Factions>) -> bool {
    if a.caster.is_some() && a.caster == b.caster {
        return false;
    }
    if let (Some(x), Some(y)) = (a.faction, b.faction) {
        return factions.map_or(x != y, |f| f.hostile(y, x));
    }
    a.side != b.side
}

/// What shape a standing ward has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BarrierKind {
    /// A half sphere on the ground, centred at `at`.
    Dome { at: DVec3, radius: f32 },
    /// A flat panel standing on `at` (its bottom middle), facing `normal` (level).
    Panel {
        at: DVec3,
        normal: Vec3,
        width: f32,
        height: f32,
    },
}

impl BarrierKind {
    /// Where the visual stands, how it turns and its scale.
    pub fn frame(&self) -> (DVec3, Quat, Vec3) {
        match *self {
            BarrierKind::Dome { at, radius } => (at, Quat::IDENTITY, Vec3::splat(radius)),
            BarrierKind::Panel { at, normal, .. } => {
                let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or(Vec3::Z);
                (at, Quat::from_rotation_arc(Vec3::Z, n), Vec3::ONE)
            }
        }
    }

    /// The first point at which a segment from `from` along `dir` (unit) for `len` m crosses
    /// into the ward from outside: its distance and the surface's outward normal there. A
    /// dome stops only what comes in (a foe who walked inside fights inside); a panel stops
    /// both ways.
    pub fn crossing(&self, from: DVec3, dir: Vec3, len: f32) -> Option<(f32, Vec3)> {
        match *self {
            BarrierKind::Dome { at, radius } => {
                let o = (from - at).as_vec3();
                let r = radius;
                if o.length_squared() <= r * r {
                    return None;
                }
                let b = o.dot(dir);
                let c = o.length_squared() - r * r;
                let disc = b * b - c;
                if disc < 0.0 {
                    return None;
                }
                let t = -b - disc.sqrt();
                if !(0.0..=len).contains(&t) {
                    return None;
                }
                let p = o + dir * t;
                (p.y >= -0.05).then(|| (t, p.normalize_or(Vec3::Y)))
            }
            BarrierKind::Panel {
                at,
                normal,
                width,
                height,
            } => {
                let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or(Vec3::Z);
                let denom = dir.dot(n);
                if denom.abs() < 1e-4 {
                    return None;
                }
                let t = (at - from).as_vec3().dot(n) / denom;
                if !(0.0..=len).contains(&t) {
                    return None;
                }
                let p = (from - at).as_vec3() + dir * t;
                let along = Vec3::Y.cross(n).normalize_or(Vec3::X);
                (p.dot(along).abs() <= width * 0.5 && (-0.05..=height).contains(&p.y))
                    .then(|| (t, if denom > 0.0 { -n } else { n }))
            }
        }
    }
}

/// A ward standing on its own: a dome or one panel of a ward wall.
#[derive(Component, Debug, Clone)]
pub struct Barrier {
    /// Who raised it (its side and faction).
    pub cast: CastInfo,
    pub kind: BarrierKind,
    pub pool: f32,
    pub full: f32,
    pub element: Option<Element>,
    pub age: f32,
    pub life: f32,
}

impl Barrier {
    /// Still stops spells.
    pub fn stands(&self) -> bool {
        self.pool > 0.0 && self.age < self.life
    }

    /// Take a hit of `damage` of `element`: the pool it cost, its share of the full pool, and
    /// whether it broke the barrier.
    pub fn strike(&mut self, damage: f32, element: Option<Element>) -> (f32, f32, bool) {
        if !self.stands() {
            return (0.0, 0.0, false);
        }
        let cost = ward_cost(damage, element, self.element).min(self.pool);
        self.pool -= cost;
        (cost, cost / self.full.max(1e-3), self.pool <= 0.0)
    }
}

/// The first standing barrier against `cast` that a segment from `from` along `dir` (unit) for
/// `len` m crosses into: the barrier, the distance and the outward normal.
pub fn barrier_in_way<'a>(
    barriers: impl Iterator<Item = (Entity, &'a Barrier)>,
    cast: &CastInfo,
    from: DVec3,
    dir: Vec3,
    len: f32,
    factions: Option<&Factions>,
) -> Option<(Entity, f32, Vec3)> {
    barriers
        .filter(|(_, b)| b.stands() && against(&b.cast, cast, factions))
        .filter_map(|(e, b)| b.kind.crossing(from, dir, len).map(|(t, n)| (e, t, n)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
}

/// A barrier to raise this step (a release queues it, [`tend_wards`] puts it up).
#[derive(Debug, Clone)]
pub struct NewBarrier {
    pub cast: CastInfo,
    pub kind: BarrierKind,
    pub pool: f32,
    pub life: f32,
    pub element: Option<Element>,
}

/// The barriers a ward spell cast as an area (a dome) or a wall (panels) raises.
pub fn raise(
    info: &CastInfo,
    aim_point: DVec3,
    flat: Vec3,
    queries: &PhysicsQueries,
    exclude: &QueryFilter,
    queue: &mut MagicQueue,
) {
    let spell = &info.spell;
    let c = &spell.carrier;
    let ward = spell.payload.ward * info.scale;
    let element = spell.payload.ward_element;
    match spell.cast {
        CastType::Area => {
            let at = ground_below(queries, aim_point, exclude);
            queue.barriers.push(NewBarrier {
                cast: info.clone(),
                kind: BarrierKind::Dome {
                    at,
                    radius: (c.radius * 1.3).clamp(3.0, 9.0),
                },
                pool: ward * DOME_POOL,
                life: DOME_SECS * (c.duration / 10.0).max(1.0),
                element,
            });
        }
        CastType::Wall => {
            let along = Vec3::new(-flat.z, 0.0, flat.x).normalize_or(Vec3::X);
            let half = c.radius.max(PANEL_W * 0.5);
            let n = ((2.0 * half / PANEL_W).round() as i32).max(1);
            let width = 2.0 * half / n as f32;
            for i in 0..n {
                let off = (i as f32 + 0.5) * width - half;
                let at = ground_below(queries, aim_point + (along * off).as_dvec3(), exclude);
                queue.barriers.push(NewBarrier {
                    cast: CastInfo {
                        seed: info.seed.wrapping_add(i as u32 * 101),
                        ..info.clone()
                    },
                    kind: BarrierKind::Panel {
                        at,
                        normal: flat,
                        width: width - 0.06,
                        height: PANEL_H,
                    },
                    pool: ward * PANEL_POOL,
                    life: c.duration.max(6.0),
                    element,
                });
            }
        }
        _ => {}
    }
}

/// A ward took a hit: a shell on a body or a barrier. Written by the hits (shells) and the
/// carriers (barriers); [`tend_wards`] lights a flare there.
#[derive(Message, Debug, Clone, Copy)]
pub struct WardStruck {
    /// The warded body, or the barrier.
    pub ward: Entity,
    pub barrier: bool,
    pub point: DVec3,
    pub dir: Vec3,
    /// Pool it cost (0: soaked by the ward's own element).
    pub taken: f32,
    /// That cost's share of the full pool.
    pub share: f32,
    pub broke: bool,
}

// ------------------------------------------------------------------------------- lattice

/// One cell of a hex lattice, in the lattice's own space.
#[derive(Debug, Clone)]
pub struct Cell {
    pub center: Vec3,
    pub normal: Vec3,
    /// In order round the cell.
    pub corners: Vec<Vec3>,
}

/// A ward's cells: a whole sphere (shells), a half sphere (domes) or a flat panel.
#[derive(Debug, Clone, Default)]
pub struct Lattice {
    pub cells: Vec<Cell>,
    /// How far round a struck cell a flare reaches, in the lattice's units.
    pub flare_r: f32,
    /// A cell's size (centre to corner), in the lattice's units.
    pub cell_r: f32,
    /// Draw a glowing band where it meets the ground (a dome).
    pub rim: bool,
    /// A flat panel's width and height: drawn with a glowing frame round it and a firmer
    /// fill, so a wall reads as panels standing side by side.
    pub frame: Option<[f32; 2]>,
}

impl Lattice {
    /// The cell nearest a point in the lattice's space.
    pub fn nearest(&self, p: Vec3) -> usize {
        self.cells
            .iter()
            .enumerate()
            .min_by(|a, b| {
                a.1.center
                    .distance_squared(p)
                    .total_cmp(&b.1.center.distance_squared(p))
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

/// An icosphere: vertices on the unit sphere and triangles, `sub` times subdivided.
pub fn icosphere(sub: u32) -> (Vec<Vec3>, Vec<[usize; 3]>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from_array(*p).normalize())
    .collect();
    let mut f: Vec<[usize; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..sub {
        let mut mid: HashMap<(usize, usize), usize> = HashMap::new();
        let mut half = |a: usize, b: usize, v: &mut Vec<Vec3>| -> usize {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                v.push(((v[a] + v[b]) * 0.5).normalize());
                v.len() - 1
            })
        };
        let mut next = Vec::with_capacity(f.len() * 4);
        for [a, b, c] in f {
            let ab = half(a, b, &mut v);
            let bc = half(b, c, &mut v);
            let ca = half(c, a, &mut v);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = next;
    }
    (v, f)
}

/// A whole sphere of hex cells (and twelve pentagons): the dual of an icosphere.
pub fn sphere_lattice(sub: u32) -> Lattice {
    let (v, f) = icosphere(sub);
    let centroids: Vec<Vec3> = f
        .iter()
        .map(|t| (v[t[0]] + v[t[1]] + v[t[2]]).normalize())
        .collect();
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); v.len()];
    for (i, t) in f.iter().enumerate() {
        for &k in t {
            around[k].push(i);
        }
    }
    let cells: Vec<Cell> = v
        .iter()
        .enumerate()
        .map(|(k, &n)| {
            let (a, b) = n.any_orthonormal_pair();
            let mut fs = around[k].clone();
            let angle = |i: usize| centroids[i].dot(b).atan2(centroids[i].dot(a));
            fs.sort_by(|&x, &y| angle(x).total_cmp(&angle(y)));
            Cell {
                center: n,
                normal: n,
                corners: fs.iter().map(|&i| centroids[i]).collect(),
            }
        })
        .collect();
    let cell_r = cells
        .first()
        .map(|c| {
            c.corners.iter().map(|p| p.distance(c.center)).sum::<f32>() / c.corners.len() as f32
        })
        .unwrap_or(0.1);
    Lattice {
        cells,
        flare_r: cell_r * 4.5,
        cell_r,
        rim: false,
        frame: None,
    }
}

/// The upper half of a sphere of cells, its rim brought down onto the ground plane.
pub fn dome_lattice(sub: u32) -> Lattice {
    let mut l = sphere_lattice(sub);
    let flat = |p: Vec3| {
        if p.y < 0.0 {
            Vec3::new(p.x, 0.0, p.z).normalize_or(Vec3::X)
        } else {
            p
        }
    };
    l.rim = true;
    l.cells.retain(|c| c.center.y > -0.6 * l.cell_r);
    for c in &mut l.cells {
        c.center = flat(c.center);
        for p in &mut c.corners {
            *p = flat(*p);
        }
    }
    l
}

/// A flat panel `w` by `h` m (x across, y up, facing +z) of pointy-top hexes of size `s`.
pub fn panel_lattice(w: f32, h: f32, s: f32) -> Lattice {
    let dx = 3f32.sqrt() * s;
    let dy = 1.5 * s;
    let rows = (h / dy).ceil() as i32 + 1;
    let cols = (w / dx).ceil() as i32 + 2;
    let mut cells = Vec::new();
    for j in 0..rows {
        let y = j as f32 * dy + s * 0.5;
        let shift = if j % 2 == 1 { dx * 0.5 } else { 0.0 };
        for i in -cols..=cols {
            let x = i as f32 * dx + shift;
            if x.abs() > w * 0.5 || y > h {
                continue;
            }
            let center = Vec3::new(x, y, 0.0);
            let corners = (0..6)
                .map(|k| {
                    let a = (30.0 + 60.0 * k as f32).to_radians();
                    let p = center + Vec3::new(a.cos(), a.sin(), 0.0) * s;
                    Vec3::new(p.x.clamp(-w * 0.5, w * 0.5), p.y.clamp(0.0, h), 0.0)
                })
                .collect();
            cells.push(Cell {
                center,
                normal: Vec3::Z,
                corners,
            });
        }
    }
    Lattice {
        cells,
        flare_r: s * 3.4,
        cell_r: s,
        rim: false,
        frame: Some([w, h]),
    }
}

/// Share of the cell's way from corner to centre that its glowing edge takes.
const EDGE: f32 = 0.13;
/// A ward wall panel's cell size (centre to corner), m: big enough that its edges still show
/// from across the hall.
pub const PANEL_CELL: f32 = 0.3;
/// Width of the glowing frame round a panel, m.
const PANEL_FRAME: f32 = 0.06;
/// Opacity of a cell's edge, of its inner rim, and of its fill.
const EDGE_A: f32 = 0.95;
const INNER_A: f32 = 0.22;
const FILL_A: f32 = 0.07;

fn push_cell(mesh: &mut Mesh, c: &Cell, edge_a: f32, fill_a: f32) {
    let n = c.corners.len();
    if n < 3 {
        return;
    }
    let inner: Vec<Vec3> = c
        .corners
        .iter()
        .map(|&p| p + (c.center - p) * EDGE)
        .collect();
    let vert = |p: Vec3, a: f32| {
        Vertex::new(p, c.normal, Default::default()).with_color(Vec4::new(1.0, 1.0, 1.0, a))
    };
    for k in 0..n {
        let k2 = (k + 1) % n;
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend([
            vert(c.corners[k], edge_a),
            vert(c.corners[k2], edge_a),
            vert(inner[k2], edge_a * INNER_A),
            vert(inner[k], edge_a * INNER_A),
        ]);
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    if fill_a > 0.0 {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(vert(c.center, fill_a * 0.6));
        for p in &inner {
            mesh.vertices.push(vert(*p, fill_a));
        }
        for k in 0..n as u32 {
            mesh.indices
                .extend([base, base + 1 + k, base + 1 + (k + 1) % n as u32]);
        }
    }
}

/// Is cell `i` gone at crack level `cracks` (0 whole, 1 at half, 2 at a quarter)?
pub fn cracked(i: usize, cracks: u8) -> bool {
    let r = roll(i as u64 * 977 + 13, 7);
    match cracks {
        0 => false,
        1 => r < 0.14,
        _ => r < 0.32,
    }
}

/// The whole lattice at a crack level.
pub fn lattice_mesh(l: &Lattice, cracks: u8) -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let fill = if l.frame.is_some() {
        FILL_A * 2.0
    } else {
        FILL_A
    };
    for (i, c) in l.cells.iter().enumerate() {
        if !cracked(i, cracks) {
            push_cell(&mut m, c, EDGE_A, fill);
        }
    }
    if let Some([w, h]) = l.frame {
        // A bright frame round the panel, fading inward.
        let (x, f) = (w * 0.5, PANEL_FRAME);
        let corners = [
            Vec3::new(-x, 0.0, 0.0),
            Vec3::new(x, 0.0, 0.0),
            Vec3::new(x, h, 0.0),
            Vec3::new(-x, h, 0.0),
        ];
        let inset = [
            Vec3::new(-x + f, f, 0.0),
            Vec3::new(x - f, f, 0.0),
            Vec3::new(x - f, h - f, 0.0),
            Vec3::new(-x + f, h - f, 0.0),
        ];
        let v = |p: Vec3, a: f32| {
            Vertex::new(p, Vec3::Z, Default::default()).with_color(Vec4::new(1.0, 1.0, 1.0, a))
        };
        for k in 0..4 {
            let k2 = (k + 1) % 4;
            let base = m.vertices.len() as u32;
            m.vertices.extend([
                v(corners[k], 1.0),
                v(corners[k2], 1.0),
                v(inset[k2], 0.35),
                v(inset[k], 0.35),
            ]);
            m.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    if l.rim {
        // Where a dome meets the ground it glows hardest (the depth-fade line of a shield
        // shader): a band round its foot, brightest at the floor.
        let n = 128;
        for k in 0..n {
            let a0 = k as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (k + 1) as f32 / n as f32 * std::f32::consts::TAU;
            let (p0, p1) = (
                Vec3::new(a0.cos(), 0.0, a0.sin()),
                Vec3::new(a1.cos(), 0.0, a1.sin()),
            );
            let base = m.vertices.len() as u32;
            let v = |p: Vec3, y: f32, a: f32| {
                Vertex::new(p + Vec3::Y * y, p, Default::default())
                    .with_color(Vec4::new(1.0, 1.0, 1.0, a))
            };
            m.vertices.extend([
                v(p0, 0.004, 1.0),
                v(p1, 0.004, 1.0),
                v(p1, 0.05, 0.0),
                v(p0, 0.05, 0.0),
            ]);
            m.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    m
}

/// The cells round cell `at`, brightest at it: a hit's flare.
pub fn patch_mesh(l: &Lattice, at: usize) -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let Some(hit) = l.cells.get(at).map(|c| c.center) else {
        return m;
    };
    for c in &l.cells {
        let d = c.center.distance(hit) / l.flare_r.max(1e-4);
        if d < 1.0 {
            let w = (1.0 - d).powf(1.5);
            push_cell(&mut m, c, w, w * 0.45);
        }
    }
    m
}

/// One glowing hex plate of unit size facing +z: a shard.
pub fn shard_mesh() -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let corners = (0..6)
        .map(|k| {
            let a = (30.0 + 60.0 * k as f32).to_radians();
            Vec3::new(a.cos(), a.sin(), 0.0)
        })
        .collect();
    push_cell(
        &mut m,
        &Cell {
            center: Vec3::ZERO,
            normal: Vec3::Z,
            corners,
        },
        1.0,
        0.35,
    );
    m
}

/// Which lattice a ward is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatticeKey {
    Shell,
    Dome,
    /// A panel, its size in cm.
    Panel {
        w_cm: u32,
        h_cm: u32,
    },
}

impl LatticeKey {
    pub fn lattice(self) -> Lattice {
        match self {
            LatticeKey::Shell => sphere_lattice(2),
            LatticeKey::Dome => dome_lattice(3),
            LatticeKey::Panel { w_cm, h_cm } => {
                panel_lattice(w_cm as f32 / 100.0, h_cm as f32 / 100.0, PANEL_CELL)
            }
        }
    }

    fn panel(width: f32, height: f32) -> Self {
        LatticeKey::Panel {
            w_cm: (width * 100.0).round() as u32,
            h_cm: (height * 100.0).round() as u32,
        }
    }
}

#[derive(Debug, Default)]
struct LatticeSet {
    lattice: Lattice,
    whole: [Option<Handle<Mesh>>; 3],
    patches: HashMap<usize, Handle<Mesh>>,
}

/// Every ward mesh built so far, built once on first use.
#[derive(Resource, Debug, Default)]
pub struct WardMeshes {
    sets: HashMap<LatticeKey, LatticeSet>,
    shard: Option<Handle<Mesh>>,
}

impl WardMeshes {
    pub fn lattice(&mut self, key: LatticeKey) -> &Lattice {
        &self.set(key).lattice
    }

    fn set(&mut self, key: LatticeKey) -> &mut LatticeSet {
        self.sets.entry(key).or_insert_with(|| LatticeSet {
            lattice: key.lattice(),
            ..Default::default()
        })
    }

    fn whole(&mut self, key: LatticeKey, cracks: u8, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        let set = self.set(key);
        let i = cracks.min(2) as usize;
        if let Some(h) = set.whole[i] {
            return h;
        }
        let h = meshes.add(lattice_mesh(&set.lattice, cracks));
        set.whole[i] = Some(h);
        h
    }

    fn patch(&mut self, key: LatticeKey, cell: usize, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        let set = self.set(key);
        if let Some(h) = set.patches.get(&cell) {
            return *h;
        }
        let h = meshes.add(patch_mesh(&set.lattice, cell));
        set.patches.insert(cell, h);
        h
    }

    fn shard(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        *self.shard.get_or_insert_with(|| meshes.add(shard_mesh()))
    }
}

// ------------------------------------------------------------------------------- looks

/// The glowing, see-through, two-sided material of a ward's lattice: black, so all its light
/// is its emission, and its vertices' alpha draws the pattern.
pub fn ward_material(color: [f32; 3], strength: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        emissive: color,
        emissive_strength: strength,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        metallic: 0.0,
        perceptual_roughness: 1.0,
        ..Default::default()
    }
}

fn renderer(mesh: Handle<Mesh>, color: [f32; 3], strength: f32) -> MeshRenderer {
    MeshRenderer {
        mesh: MeshRef::Handle(mesh),
        material: MaterialRef::Inline(ward_material(color, glow_step(strength))),
        cast_shadows: false,
        visible: true,
    }
}

/// A glow in steps of 6% (and none under 1 nit): inline materials are shared by value, so
/// stepping keeps the fading wards, flares and shards on a few hundred materials instead of a
/// new one every frame.
pub fn glow_step(strength: f32) -> f32 {
    const STEP: f32 = 1.06;
    if strength < 1.0 {
        return 0.0;
    }
    STEP.powf((strength.ln() / STEP.ln()).round())
}

/// Ward meshes' own materials, by entity: a ward's glow changes every frame, and an inline
/// material that changes makes a new material each time, so each ward mesh gets its own
/// material on its first change and is edited through its handle after that (as
/// `engine_render::resolve_mesh_renderers` asks). Freed once the entity is gone.
pub struct Paint<'a> {
    materials: Option<&'a mut Assets<StandardMaterial>>,
    owned: &'a mut HashMap<Entity, Handle<StandardMaterial>>,
}

impl Paint<'_> {
    /// Set `e`'s glow (stepped, see [`glow_step`]), writing only when the step changes.
    fn glow(&mut self, e: Entity, r: &mut Mut<MeshRenderer>, color: [f32; 3], strength: f32) {
        let strength = glow_step(strength);
        let Some(materials) = self.materials.as_deref_mut() else {
            // No renderer (tests): the inline material carries it.
            if let MaterialRef::Inline(m) = &r.material
                && (m.emissive != color || m.emissive_strength != strength)
                && let MaterialRef::Inline(m) = &mut r.material
            {
                m.emissive = color;
                m.emissive_strength = strength;
            }
            return;
        };
        let handle = match (&r.material, self.owned.get(&e)) {
            (MaterialRef::Handle(h), Some(owned)) if h == owned => *h,
            (MaterialRef::Inline(m), _) => {
                if m.emissive == color && m.emissive_strength == strength {
                    return;
                }
                let h = materials.add(*m);
                self.owned.insert(e, h);
                r.material = MaterialRef::Handle(h);
                h
            }
            _ => return,
        };
        if let Some(m) = materials.get(handle)
            && m.emissive == color
            && m.emissive_strength == strength
        {
            return;
        }
        if let Some(m) = materials.get_mut(handle) {
            m.emissive = color;
            m.emissive_strength = strength;
        }
    }
}

/// How a ward looks now: on a shell's own entity (riding its body) or on a barrier.
#[derive(Component, Debug, Clone)]
pub struct WardLook {
    pub key: LatticeKey,
    pub color: [f32; 3],
    pub cracks: u8,
    /// Brightness kick from the last hits, fading.
    pub glow: f32,
    /// Seconds since it appeared (it swells in).
    pub age: f32,
    /// Seconds since its time ran out (it fades), or None.
    pub fading: Option<f32>,
    /// Seconds since its last flare (beams flare at most this often).
    pub since_flare: f32,
    pub seed: u64,
}

impl WardLook {
    fn new(key: LatticeKey, color: [f32; 3], seed: u64) -> Self {
        Self {
            key,
            color,
            cracks: 0,
            glow: 0.0,
            age: 0.0,
            fading: None,
            since_flare: 1.0,
            seed,
        }
    }

    /// Its emission now, with `share` of its pool left at time `t`.
    fn strength(&self, share: f32, t: f32) -> f32 {
        let swell = (self.age / SWELL_SECS).clamp(0.0, 1.0);
        // It comes in bright and settles.
        let birth = 1.0 + 1.5 * (1.0 - (self.age / 0.6).clamp(0.0, 1.0));
        let fade = self.fading.map_or(1.0, |f| (1.0 - f / FADE_SECS).max(0.0));
        let flicker = if share < FLICKER_BELOW {
            let k = roll(self.seed ^ (t * 24.0) as u64, 3);
            0.35 + 0.65 * k
        } else {
            1.0
        };
        LATTICE_GLOW * swell * birth * fade * flicker * (1.0 + self.glow)
    }
}

/// The shell round a warded body (its entity rides the body; the pool is the body's).
#[derive(Component, Debug, Clone, Copy)]
pub struct WardShell {
    pub owner: Entity,
    /// Half its size along x, y and z.
    pub half: Vec3,
    /// Its centre above the body's feet.
    pub lift: f32,
}

/// A hit's flare: the cells round the point struck, a child of the ward it lit.
#[derive(Component, Debug, Clone, Copy)]
pub struct WardFlare {
    pub age: f32,
    pub peak: f32,
    pub color: [f32; 3],
}

/// A shard of a broken ward, flying off.
#[derive(Component, Debug, Clone, Copy)]
pub struct WardShard {
    pub vel: Vec3,
    pub spin: Vec3,
    pub age: f32,
    pub life: f32,
    pub color: [f32; 3],
}

/// The flash of a ward breaking.
#[derive(Component, Debug, Clone, Copy)]
pub struct WardFlash {
    pub age: f32,
    pub peak: f32,
}

/// A shell's size round a body: a person by default, a character by its controller.
fn shell_size(cc: Option<&CharacterController>) -> (Vec3, f32) {
    let (r, h) = cc.map_or((0.35, 1.8), |c| (c.radius, c.height));
    let half = Vec3::new(r + 0.42, h * 0.5 + 0.28, r + 0.42);
    (half, h * 0.5)
}

/// Shards and a flash for a ward breaking: `n` of its cells fly off.
#[allow(clippy::too_many_arguments)]
fn shatter(
    commands: &mut Commands,
    wm: &mut WardMeshes,
    meshes: &mut Option<ResMut<Assets<Mesh>>>,
    key: LatticeKey,
    frame: (DVec3, Quat, Vec3),
    color: [f32; 3],
    every: usize,
    only: Option<&[usize]>,
    seed: u64,
) {
    let Some(meshes) = meshes.as_deref_mut() else {
        return;
    };
    let shard = wm.shard(meshes);
    let (at, rot, scale) = frame;
    let l = wm.lattice(key).clone();
    let size = l.cell_r * scale.min_element() * 0.75;
    let pick: Vec<usize> = match only {
        Some(list) => list.to_vec(),
        None => (0..l.cells.len()).step_by(every.max(1)).collect(),
    };
    for (k, i) in pick.into_iter().enumerate() {
        let c = &l.cells[i];
        let local = c.center * scale;
        let n = (rot * (c.normal * scale.recip()).normalize_or(c.normal)).normalize_or(Vec3::Y);
        let r = |s: u64| roll(seed.wrapping_add(k as u64 * 7 + s), 11 + s) * 2.0 - 1.0;
        let vel =
            n * (2.2 + 1.6 * (r(1) * 0.5 + 0.5)) + Vec3::new(r(2), 0.6 + 0.4 * r(3), r(4)) * 0.9;
        commands.spawn((
            Name::new("Ward shard"),
            Transform::from_translation(at + (rot * local).as_dvec3())
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, n))
                .with_scale(Vec3::splat(size)),
            renderer(shard, color, SHARD_GLOW),
            WardShard {
                vel,
                spin: Vec3::new(r(5), r(6), r(7)) * 9.0,
                age: 0.0,
                life: SHARD_SECS * (0.75 + 0.25 * r(8).abs()),
                color,
            },
            NoVirtualGeometry,
        ));
    }
}

fn flash(commands: &mut Commands, at: DVec3, color: [f32; 3], peak: f32) {
    commands.spawn((
        Name::new("Ward breaking"),
        Transform::from_translation(at),
        PointLight {
            color,
            intensity_lm: peak,
            range: 12.0,
            radius: 0.3,
        },
        WardFlash { age: 0.0, peak },
    ));
}

/// Keep every ward's look: raise the queued barriers, ride shells on their bodies, light a flare
/// where each hit landed, crack them as their pools drain, shatter the broken, fade the expired,
/// and fly the shards.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn tend_wards(
    time: Res<Time>,
    mut commands: Commands,
    mut queue: ResMut<MagicQueue>,
    mut struck: MessageReader<WardStruck>,
    mut wm: ResMut<WardMeshes>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut bodies: Query<(
        Entity,
        &mut Afflictions,
        &GlobalTransform,
        Option<&CharacterController>,
    )>,
    mut shells: Query<
        (
            Entity,
            &WardShell,
            &mut WardLook,
            &mut Transform,
            Option<&mut MeshRenderer>,
        ),
        Without<Barrier>,
    >,
    mut barriers: Query<
        (
            Entity,
            &mut Barrier,
            &mut WardLook,
            &mut Transform,
            Option<&mut MeshRenderer>,
        ),
        Without<WardShell>,
    >,
    mut flares: Query<
        (Entity, &mut WardFlare, Option<&mut MeshRenderer>),
        (Without<WardShell>, Without<Barrier>),
    >,
    mut shards: Query<
        (
            Entity,
            &mut WardShard,
            &mut Transform,
            Option<&mut MeshRenderer>,
        ),
        (Without<WardShell>, Without<Barrier>, Without<WardFlare>),
    >,
    mut flashes: Query<(Entity, &mut WardFlash, &mut PointLight)>,
    (mut materials, mut painted): (
        Option<ResMut<Assets<StandardMaterial>>>,
        Local<HashMap<Entity, Handle<StandardMaterial>>>,
    ),
    mut owned: Local<HashMap<Entity, Entity>>,
    mut seq: Local<u64>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs();
    // Free the materials of ward meshes that are gone.
    if let Some(mats) = materials.as_deref_mut() {
        painted.retain(|&e, h| {
            let live = shells.contains(e)
                || barriers.contains(e)
                || flares.contains(e)
                || shards.contains(e);
            if !live {
                mats.remove(*h);
            }
            live
        });
    }
    let mut next_seed = || {
        *seq += 1;
        *seq * 7919 + 17
    };

    // ---- raise the barriers queued this step
    for nb in std::mem::take(&mut queue.barriers) {
        let key = match nb.kind {
            BarrierKind::Dome { .. } => LatticeKey::Dome,
            BarrierKind::Panel { width, height, .. } => LatticeKey::panel(width, height),
        };
        let color = ward_color(nb.element);
        let (at, rot, scale) = nb.kind.frame();
        let mut e = commands.spawn((
            Name::new(match nb.kind {
                BarrierKind::Dome { .. } => "Ward dome",
                BarrierKind::Panel { .. } => "Ward wall panel",
            }),
            Transform::from_translation(at)
                .with_rotation(rot)
                .with_scale(scale * 0.05),
            WardLook::new(key, color, next_seed()),
            NoVirtualGeometry,
            NoMeshLod,
        ));
        if let Some(m) = meshes.as_deref_mut() {
            e.insert(renderer(wm.whole(key, 0, m), color, 0.0));
        }
        let me = e.id();
        commands.entity(me).insert(Barrier {
            cast: nb.cast,
            kind: nb.kind,
            pool: nb.pool,
            full: nb.pool,
            element: nb.element,
            age: 0.0,
            life: nb.life,
        });
    }

    // ---- shells: one for every warded body
    let mut fresh: Vec<Entity> = Vec::new();
    for (body, aff, gt, cc) in &bodies {
        if aff.ward > 0.0 && !owned.contains_key(&body) {
            let (half, lift) = shell_size(cc);
            let color = ward_color(aff.ward_element);
            let mut e = commands.spawn((
                Name::new("Ward shell"),
                Transform::from_translation(gt.translation() + DVec3::Y * lift as f64)
                    .with_scale(half * 0.05),
                WardShell {
                    owner: body,
                    half,
                    lift,
                },
                WardLook::new(LatticeKey::Shell, color, next_seed()),
                NoVirtualGeometry,
                NoMeshLod,
            ));
            if let Some(m) = meshes.as_deref_mut() {
                e.insert(renderer(wm.whole(LatticeKey::Shell, 0, m), color, 0.0));
            }
            owned.insert(body, e.id());
            fresh.push(e.id());
        }
    }

    // ---- flares where hits landed
    for s in struck.read() {
        let Some(m) = meshes.as_deref_mut() else {
            continue;
        };
        let found = if s.barrier {
            barriers.get_mut(s.ward).ok().map(|(e, b, look, _, _)| {
                let (at, rot, scale) = b.kind.frame();
                (e, at, rot, scale, look)
            })
        } else {
            owned.get(&s.ward).and_then(|&sh| {
                shells.get_mut(sh).ok().map(|(e, shell, look, tf, _)| {
                    (e, tf.translation, tf.rotation, shell.half, look)
                })
            })
        };
        let Some((e, at, rot, scale, mut look)) = found else {
            continue;
        };
        look.glow = (look.glow + 0.35 + 2.5 * s.share).min(1.6);
        if look.since_flare < 0.08 && s.share < 0.05 {
            continue;
        }
        look.since_flare = 0.0;
        let local = rot.inverse() * (s.point - at).as_vec3() / scale.max(Vec3::splat(1e-3));
        let local = match look.key {
            LatticeKey::Panel { .. } => local,
            _ => local.normalize_or(Vec3::Y),
        };
        let key = look.key;
        let cell = wm.lattice(key).nearest(local);
        let patch = wm.patch(key, cell, m);
        // Soaked hits (its own element) flare white-hot but small; costly ones big.
        let peak = FLARE_GLOW * (0.7 + 6.0 * s.share).min(2.6);
        let color = if s.taken <= 0.0 {
            [1.0, 1.0, 1.0]
        } else {
            look.color
        };
        let lift = match key {
            LatticeKey::Panel { .. } => Transform::from_xyz(0.0, 0.0, 0.012),
            _ => Transform::from_scale(Vec3::splat(1.012)),
        };
        commands.spawn((
            Name::new("Ward flare"),
            lift,
            renderer(patch, color, peak),
            WardFlare {
                age: 0.0,
                peak,
                color,
            },
            NoVirtualGeometry,
            NoMeshLod,
            ChildOf(e),
        ));
    }

    // ---- shells ride, crack, fade and shatter
    let mut paint = Paint {
        materials: materials.as_deref_mut(),
        owned: &mut painted,
    };
    let mut gone: Vec<Entity> = Vec::new();
    for (&body, &shell_e) in owned.iter() {
        let Ok((_, shell, mut look, mut tf, mut r)) = shells.get_mut(shell_e) else {
            // Spawned this step: it is there from the next.
            if !fresh.contains(&shell_e) {
                gone.push(body);
            }
            continue;
        };
        let Ok((_, mut aff, gt, _)) = bodies.get_mut(body) else {
            commands.entity(shell_e).despawn();
            gone.push(body);
            continue;
        };
        look.age += dt;
        look.since_flare += dt;
        look.glow = (look.glow - dt * 3.0).max(0.0);
        tf.translation = gt.translation() + DVec3::Y * shell.lift as f64;
        let swell = 0.6 + 0.4 * (look.age / SWELL_SECS).clamp(0.0, 1.0);
        tf.scale = shell.half * swell;
        if aff.ward_broke {
            aff.ward_broke = false;
            shatter(
                &mut commands,
                &mut wm,
                &mut meshes,
                LatticeKey::Shell,
                (tf.translation, tf.rotation, tf.scale),
                look.color,
                3,
                None,
                look.seed,
            );
            flash(&mut commands, tf.translation, look.color, 4000.0);
            commands.entity(shell_e).despawn();
            gone.push(body);
            continue;
        }
        if aff.ward <= 0.0 {
            let f = look.fading.unwrap_or(0.0) + dt;
            look.fading = Some(f);
            if f >= FADE_SECS {
                commands.entity(shell_e).despawn();
                gone.push(body);
                continue;
            }
        } else if look.fading.is_some() {
            // Warded again while fading.
            look.fading = None;
            look.age = 0.0;
        }
        let share = if aff.ward_full > 0.0 {
            aff.ward / aff.ward_full
        } else {
            1.0
        };
        let cracks = CRACKS.iter().filter(|&&c| share < c).count() as u8;
        let color = ward_color(aff.ward_element);
        if cracks != look.cracks || color != look.color {
            if cracks > look.cracks {
                let dropped: Vec<usize> = (0..wm.lattice(LatticeKey::Shell).cells.len())
                    .filter(|&i| cracked(i, cracks) && !cracked(i, look.cracks))
                    .step_by(2)
                    .collect();
                shatter(
                    &mut commands,
                    &mut wm,
                    &mut meshes,
                    LatticeKey::Shell,
                    (tf.translation, tf.rotation, tf.scale),
                    look.color,
                    1,
                    Some(&dropped),
                    look.seed ^ cracks as u64,
                );
            }
            look.cracks = cracks;
            look.color = color;
            if let (Some(m), Some(r)) = (meshes.as_deref_mut(), r.as_mut()) {
                r.mesh = MeshRef::Handle(wm.whole(LatticeKey::Shell, cracks, m));
            }
        }
        if let Some(r) = r.as_mut() {
            paint.glow(shell_e, r, look.color, look.strength(share, t));
        }
    }
    for b in gone {
        owned.remove(&b);
    }

    // ---- barriers stand, crack, fade and shatter
    for (e, mut b, mut look, mut tf, mut r) in &mut barriers {
        b.age += dt;
        look.age = b.age;
        look.since_flare += dt;
        look.glow = (look.glow - dt * 3.0).max(0.0);
        let (at, rot, scale) = b.kind.frame();
        let swell = (b.age / SWELL_SECS).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - swell).powi(3);
        tf.translation = at;
        tf.rotation = rot;
        tf.scale = match b.kind {
            BarrierKind::Dome { .. } => scale * (0.05 + 0.95 * ease),
            BarrierKind::Panel { .. } => Vec3::new(1.0, 0.05 + 0.95 * ease, 1.0),
        };
        if b.pool <= 0.0 {
            let every = match b.kind {
                BarrierKind::Dome { .. } => 4,
                BarrierKind::Panel { .. } => 2,
            };
            shatter(
                &mut commands,
                &mut wm,
                &mut meshes,
                look.key,
                (at, rot, scale),
                look.color,
                every,
                None,
                look.seed,
            );
            let mid = match b.kind {
                BarrierKind::Dome { at, radius } => at + DVec3::Y * (radius * 0.5) as f64,
                BarrierKind::Panel { at, height, .. } => at + DVec3::Y * (height * 0.5) as f64,
            };
            flash(&mut commands, mid, look.color, 12000.0);
            queue.looks.push(LookCmd::Burst {
                at: mid,
                up: Vec3::Y,
                look: "light".into(),
                role: Role::Impact,
                scale: 2.0,
                color: look.color,
            });
            commands.entity(e).despawn();
            continue;
        }
        if b.age >= b.life {
            let f = look.fading.unwrap_or(0.0) + dt;
            look.fading = Some(f);
            if f >= FADE_SECS {
                commands.entity(e).despawn();
                continue;
            }
        }
        let share = b.pool / b.full.max(1e-3);
        let cracks = CRACKS.iter().filter(|&&c| share < c).count() as u8;
        if cracks > look.cracks {
            let key = look.key;
            let dropped: Vec<usize> = (0..wm.lattice(key).cells.len())
                .filter(|&i| cracked(i, cracks) && !cracked(i, look.cracks))
                .step_by(2)
                .collect();
            shatter(
                &mut commands,
                &mut wm,
                &mut meshes,
                key,
                (at, rot, scale),
                look.color,
                1,
                Some(&dropped),
                look.seed ^ cracks as u64,
            );
            look.cracks = cracks;
            if let (Some(m), Some(r)) = (meshes.as_deref_mut(), r.as_mut()) {
                r.mesh = MeshRef::Handle(wm.whole(key, cracks, m));
            }
        }
        let strength = look.strength(share, t);
        if let Some(r) = r.as_mut() {
            paint.glow(e, r, look.color, strength);
        }
    }

    // ---- flares fade
    for (e, mut f, r) in &mut flares {
        f.age += dt;
        if f.age >= FLARE_SECS {
            commands.entity(e).despawn();
            continue;
        }
        let k = 1.0 - f.age / FLARE_SECS;
        if let Some(mut r) = r {
            paint.glow(e, &mut r, f.color, f.peak * k * k);
        }
    }

    // ---- shards fly
    for (e, mut s, mut tf, r) in &mut shards {
        s.age += dt;
        if s.age >= s.life {
            commands.entity(e).despawn();
            continue;
        }
        s.vel += Vec3::NEG_Y * 4.0 * dt;
        s.vel *= 1.0 - 0.6 * dt;
        tf.translation += (s.vel * dt).as_dvec3();
        tf.rotation = Quat::from_scaled_axis(s.spin * dt) * tf.rotation;
        let k = 1.0 - s.age / s.life;
        tf.scale *= 1.0 - 0.5 * dt;
        if let Some(mut r) = r {
            paint.glow(e, &mut r, s.color, SHARD_GLOW * k);
        }
    }

    // ---- flashes
    for (e, mut f, mut light) in &mut flashes {
        f.age += dt;
        if f.age >= 0.3 {
            commands.entity(e).despawn();
            continue;
        }
        light.intensity_lm = f.peak * (1.0 - f.age / 0.3);
    }
}

/// Clear every ward from the world (a hall program starting over).
pub fn clear(world: &mut World) {
    let mut q = world.query_filtered::<Entity, Or<(
        With<Barrier>,
        With<WardShell>,
        With<WardShard>,
        With<WardFlash>,
    )>>();
    let list: Vec<Entity> = q.iter(world).collect();
    for e in list {
        if let Ok(em) = world.get_entity_mut(e) {
            em.despawn();
        }
    }
}

/// `magic.wards`: what stands and who is warded, for scripts and agents.
pub fn register(app: &mut App) {
    app.register_api(
        "magic.wards",
        "Wards now: barriers (domes and ward wall panels: where, size, pool left of full, \
         element, seconds left, side) and warded bodies (pool, element, seconds left, and the \
         lockout after a break).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Barrier)>();
            let barriers: Vec<serde_json::Value> = q
                .iter(world)
                .filter(|(_, b)| b.stands())
                .map(|(e, b)| {
                    let (shape, at, size) = match b.kind {
                        BarrierKind::Dome { at, radius } => ("dome", at, vec![radius]),
                        BarrierKind::Panel {
                            at, width, height, ..
                        } => ("panel", at, vec![width, height]),
                    };
                    serde_json::json!({
                        "id": e.to_bits(),
                        "shape": shape,
                        "at": [at.x, at.y, at.z],
                        "size_m": size,
                        "pool": b.pool,
                        "full": b.full,
                        "element": b.element,
                        "secs_left": (b.life - b.age).max(0.0),
                        "side": b.cast.side,
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Afflictions)>();
            let bodies: Vec<serde_json::Value> = q
                .iter(world)
                .filter(|(_, a)| a.ward > 0.0 || a.ward_lock > 0.0)
                .map(|(e, a)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "name": world.get::<Name>(e).map(|n| n.as_str().to_string()),
                        "pool": a.ward,
                        "full": a.ward_full,
                        "element": a.ward_element,
                        "secs_left": a.ward_secs,
                        "locked_secs": a.ward_lock,
                    })
                })
                .collect();
            Ok(serde_json::json!({ "barriers": barriers, "bodies": bodies }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glow_steps_are_few_and_close() {
        assert_eq!(glow_step(0.4), 0.0);
        for g in [3.0, 140.0, LATTICE_GLOW, FLARE_GLOW * 2.6] {
            let q = glow_step(g);
            assert!((q / g - 1.0).abs() < 0.031, "{g} -> {q}");
            assert_eq!(glow_step(q), q, "a step is its own step");
        }
        // A fading flare passes through few distinct materials.
        let mut steps: Vec<u32> = (0..200)
            .map(|i| glow_step(FLARE_GLOW * (1.0 - i as f32 / 200.0)).to_bits())
            .collect();
        steps.dedup();
        assert!(steps.len() < 120, "{}", steps.len());
    }

    #[test]
    fn a_goldberg_sphere_is_hexes_and_twelve_pentagons() {
        let l = sphere_lattice(2);
        assert_eq!(l.cells.len(), 162);
        let pent = l.cells.iter().filter(|c| c.corners.len() == 5).count();
        let hex = l.cells.iter().filter(|c| c.corners.len() == 6).count();
        assert_eq!((pent, hex), (12, 150));
        // Corners go round each cell in order: neighbouring corners are close.
        for c in &l.cells {
            for k in 0..c.corners.len() {
                let d = c.corners[k].distance(c.corners[(k + 1) % c.corners.len()]);
                assert!(d < 3.0 * l.cell_r, "{d}");
            }
        }
        let d = dome_lattice(3);
        assert!(d.cells.iter().all(|c| c.center.y >= 0.0));
        assert!(
            d.cells.len() > 300 && d.cells.len() < 400,
            "{}",
            d.cells.len()
        );
        let p = panel_lattice(1.7, 2.7, PANEL_CELL);
        assert!(p.cells.len() > 20);
        assert!(
            p.cells
                .iter()
                .flat_map(|c| c.corners.iter())
                .all(|q| q.x.abs() <= 0.851 && (0.0..=2.7).contains(&q.y))
        );
    }

    #[test]
    fn cracks_take_more_cells_and_a_patch_is_brightest_where_it_struck() {
        let l = sphere_lattice(2);
        let n = |k| lattice_mesh(&l, k).indices.len();
        assert!(n(0) > n(1) && n(1) > n(2));
        let at = l.nearest(Vec3::X);
        assert!(l.cells[at].center.dot(Vec3::X) > 0.95);
        let p = patch_mesh(&l, at);
        assert!(!p.indices.is_empty() && p.indices.len() < n(0) / 4);
    }

    #[test]
    fn a_ward_soaks_its_own_element_and_takes_double_from_its_opposite() {
        let fire = Some(Element::Fire);
        let frost = Some(Element::Frost);
        assert_eq!(ward_cost(10.0, fire, fire), 0.0);
        assert_eq!(ward_cost(10.0, fire, frost), 20.0);
        assert_eq!(ward_cost(10.0, Some(Element::Storm), frost), 10.0);
        assert_eq!(ward_cost(10.0, fire, None), 10.0);
        let spell = payback_spell(Element::Frost);
        assert_eq!(spell.element(), Some(Element::Frost));
    }

    #[test]
    fn a_dome_stops_what_comes_in_and_a_panel_both_ways() {
        let dome = BarrierKind::Dome {
            at: DVec3::ZERO,
            radius: 4.0,
        };
        let (t, n) = dome
            .crossing(DVec3::new(10.0, 1.0, 0.0), Vec3::NEG_X, 10.0)
            .unwrap();
        assert!((t - (10.0 - 15f32.sqrt())).abs() < 1e-3, "{t}");
        assert!(n.x > 0.9);
        // From inside, nothing; under the floor, nothing.
        assert!(
            dome.crossing(DVec3::new(1.0, 1.0, 0.0), Vec3::X, 10.0)
                .is_none()
        );
        assert!(
            dome.crossing(DVec3::new(10.0, -2.0, 0.0), Vec3::NEG_X, 20.0)
                .is_none()
        );
        let panel = BarrierKind::Panel {
            at: DVec3::ZERO,
            normal: Vec3::X,
            width: 1.7,
            height: 2.7,
        };
        let (t, n) = panel
            .crossing(DVec3::new(5.0, 1.0, 0.3), Vec3::NEG_X, 10.0)
            .unwrap();
        assert!((t - 5.0).abs() < 1e-4 && n.x > 0.9);
        assert!(
            panel
                .crossing(DVec3::new(-5.0, 1.0, 0.3), Vec3::X, 10.0)
                .is_some()
        );
        // Past its edge, over its top.
        assert!(
            panel
                .crossing(DVec3::new(5.0, 1.0, 1.2), Vec3::NEG_X, 10.0)
                .is_none()
        );
        assert!(
            panel
                .crossing(DVec3::new(5.0, 3.0, 0.0), Vec3::NEG_X, 10.0)
                .is_none()
        );
    }
}
