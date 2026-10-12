//! Procedural wildlife: red deer (stags carry branching antlers and a dark neck mane), grey
//! wolves, European rabbits and starlings.
//!
//! Every animal is a set of rigid pieces (torso, neck, head, jaw, ears, tail, leg segments,
//! hooves, wings) that the stand-in pose code places each step. Each piece is sculpted in the
//! animal's rest pose as a signed distance field and meshed with surface nets on a grid aligned
//! with its bone (fine across, coarse along), or built directly as tubes (antlers, bird legs)
//! and ribbons (feathers). Vertices carry the coat pattern (countershading, rump patch, masks,
//! agouti saddle) times ambient occlusion baked against the whole body, and UVs that follow the
//! fur's flow, so the tiled fur, feather, horn and hoof textures run the right way.
//!
//! Rest coordinates: meters, Y up, −Z forward, origin on the ground under the body; the right
//! side is +X. A piece's mesh is stored relative to its pivot (the joint it turns about), with
//! the rest orientation, so the pose code only needs each joint's position and rotation.
//!
//! All meshes and textures of a species are generated once, in parallel, the first time an
//! animal of that species appears, and shared by every animal of that species.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, StandardMaterial, Vertex};

use super::ArtAssets;
use super::noise::{fbm3, perlin2, rand3};
use super::sdf::{ellipsoid, round_cone, smax, smin, sphere, ssub};
use super::tex::{Texel, TextureSet, hex, mix, smooth};
use crate::wildlife::Species;

// ======================================================================= rigs

/// One leg in the rest pose, right side (the left mirrors x): the body joint (shoulder or
/// hip), elbow or stifle, carpus or hock, fetlock (paw joint for wolves) and the toe tip on
/// the ground.
#[derive(Debug, Clone, Copy)]
pub struct LegRest {
    pub joints: [Vec3; 5],
}

impl LegRest {
    /// Joint positions of leg `k` (0 left front, 1 right front, 2 left hind, 3 right hind).
    pub fn side(&self, k: u8) -> [Vec3; 5] {
        if k.is_multiple_of(2) {
            self.joints.map(flip_x)
        } else {
            self.joints
        }
    }
}

/// Rest-pose joints of a four-legged animal (deer and wolves).
#[derive(Debug, Clone, Copy)]
pub struct QuadRig {
    /// Torso pivot.
    pub body: Vec3,
    /// Neck base (inside the chest) and mid-neck joint.
    pub neck: Vec3,
    pub neck2: Vec3,
    /// Poll: where the skull turns on the neck.
    pub head: Vec3,
    /// Jaw hinge (wolves open their mouth).
    pub jaw: Vec3,
    /// Right ear base (the left mirrors x).
    pub ear: Vec3,
    /// Tail base and, for two-piece tails, the middle joint.
    pub tail: Vec3,
    pub tail2: Vec3,
    pub front: LegRest,
    pub hind: LegRest,
    /// Height of the brisket and belly (lying down puts them on the ground), meters.
    pub belly: f32,
    /// Half the torso's width (an animal lying on its side rests on it), meters.
    pub half_width: f32,
    /// Distance from the poll to the nose tip, meters.
    pub head_len: f32,
}

impl QuadRig {
    pub fn leg(&self, k: u8) -> [Vec3; 5] {
        if k < 2 {
            self.front.side(k)
        } else {
            self.hind.side(k)
        }
    }
}

const fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

const fn v2(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

fn flip_x(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

/// Red deer hind (stags use the same rig scaled by [`STAG_SCALE`]): 1.15 m at the shoulder.
pub const DEER: QuadRig = QuadRig {
    body: v3(0.0, 0.90, -0.02),
    neck: v3(0.0, 0.93, -0.55),
    neck2: v3(0.0, 1.14, -0.75),
    head: v3(0.0, 1.36, -0.93),
    jaw: v3(0.0, 1.29, -0.97),
    ear: v3(0.044, 1.3973, -1.0132),
    tail: v3(0.0, 1.075, 0.565),
    tail2: v3(0.0, 0.935, 0.605),
    front: LegRest {
        joints: [
            v3(0.12, 0.88, -0.47),
            v3(0.125, 0.665, -0.375),
            v3(0.115, 0.37, -0.385),
            v3(0.115, 0.105, -0.385),
            v3(0.115, 0.0, -0.462),
        ],
    },
    hind: LegRest {
        joints: [
            v3(0.10, 1.0, 0.34),
            v3(0.14, 0.715, 0.20),
            v3(0.115, 0.455, 0.455),
            v3(0.11, 0.105, 0.405),
            v3(0.11, 0.0, 0.328),
        ],
    },
    belly: 0.64,
    half_width: 0.2,
    head_len: 0.4,
};

/// Stags are this much bigger than hinds.
pub const STAG_SCALE: f32 = 1.1;

/// Grey wolf: 0.8 m at the shoulder. The neck is one piece (`neck2` sits on the poll).
pub const WOLF: QuadRig = QuadRig {
    body: v3(0.0, 0.60, -0.02),
    neck: v3(0.0, 0.655, -0.34),
    neck2: v3(0.0, 0.80, -0.56),
    head: v3(0.0, 0.80, -0.56),
    jaw: v3(0.0, 0.765, -0.60),
    ear: v3(0.045, 0.87, -0.595),
    tail: v3(0.0, 0.675, 0.40),
    tail2: v3(0.0, 0.50, 0.535),
    front: LegRest {
        joints: [
            v3(0.09, 0.60, -0.36),
            v3(0.095, 0.425, -0.30),
            v3(0.085, 0.14, -0.315),
            v3(0.085, 0.035, -0.345),
            v3(0.085, 0.0, -0.405),
        ],
    },
    hind: LegRest {
        joints: [
            v3(0.085, 0.655, 0.28),
            v3(0.115, 0.455, 0.17),
            v3(0.095, 0.225, 0.36),
            v3(0.085, 0.035, 0.31),
            v3(0.085, 0.0, 0.255),
        ],
    },
    belly: 0.42,
    half_width: 0.15,
    head_len: 0.27,
};

/// European rabbit crouched on all fours, about 0.2 m tall.
#[derive(Debug, Clone, Copy)]
pub struct RabbitRig {
    pub body: Vec3,
    /// Where the head turns.
    pub head: Vec3,
    /// Right ear base.
    pub ear: Vec3,
    pub tail: Vec3,
    /// Right foreleg: shoulder, wrist, toe tip.
    pub front: [Vec3; 3],
    /// Right hind leg: hip, heel, toe tip (the long foot lies on the ground).
    pub hind: [Vec3; 3],
}

pub const RABBIT: RabbitRig = RabbitRig {
    body: v3(0.0, 0.10, 0.0),
    head: v3(0.0, 0.155, -0.085),
    ear: v3(0.017, 0.205, -0.105),
    tail: v3(0.0, 0.125, 0.13),
    front: [
        v3(0.028, 0.085, -0.07),
        v3(0.03, 0.025, -0.085),
        v3(0.03, 0.0, -0.105),
    ],
    hind: [
        v3(0.045, 0.105, 0.065),
        v3(0.05, 0.014, 0.105),
        v3(0.045, 0.0, 0.015),
    ],
};

/// Starling standing on the ground: 0.2 m long, 0.38 m wingspan.
#[derive(Debug, Clone, Copy)]
pub struct BirdRig {
    pub body: Vec3,
    pub head: Vec3,
    pub tail: Vec3,
    /// Right shoulder and wrist (the wing's rest pose is spread flat to the right).
    pub shoulder: Vec3,
    pub wrist: Vec3,
    /// Right hip and foot.
    pub hip: Vec3,
    pub foot: Vec3,
}

pub const BIRD: BirdRig = BirdRig {
    body: v3(0.0, 0.075, 0.0),
    head: v3(0.0, 0.105, -0.052),
    tail: v3(0.0, 0.078, 0.055),
    shoulder: v3(0.02, 0.092, -0.028),
    wrist: v3(0.095, 0.093, -0.02),
    hip: v3(0.012, 0.052, 0.004),
    foot: v3(0.014, 0.0, -0.004),
};

// ======================================================================= pieces

/// A rigid piece of an animal's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Piece {
    Body,
    Neck,
    Neck2,
    Head,
    /// Eyes and nose (glossy), riding on the head.
    Gloss,
    /// Lower jaw.
    Jaw,
    /// 0 left, 1 right.
    Ear(u8),
    Tail,
    Tail2,
    /// Both antlers (stags), riding on the head.
    Antlers,
    /// Leg `k` (0 left front, 1 right front, 2 left hind, 3 right hind), segment 0 (upper
    /// arm or thigh) .. 3 (pastern or paw).
    Leg(u8, u8),
    /// Hoof of leg `k`, riding on its last segment.
    Hoof(u8),
    /// Bird wing of side `s` (0 left, 1 right): 0 arm, 1 hand, 2 folded on the body.
    Wing(u8, u8),
    /// A bird's tail spread for flight (`Tail` is the folded one).
    TailFan,
}

impl Piece {
    fn key(self) -> String {
        match self {
            Piece::Body => "body".into(),
            Piece::Neck => "neck".into(),
            Piece::Neck2 => "neck2".into(),
            Piece::Head => "head".into(),
            Piece::Gloss => "gloss".into(),
            Piece::Jaw => "jaw".into(),
            Piece::Ear(s) => format!("ear{s}"),
            Piece::Tail => "tail".into(),
            Piece::Tail2 => "tail2".into(),
            Piece::Antlers => "antlers".into(),
            Piece::Leg(k, s) => format!("leg{k}_{s}"),
            Piece::Hoof(k) => format!("hoof{k}"),
            Piece::Wing(s, w) => format!("wing{s}_{w}"),
            Piece::TailFan => "tailfan".into(),
        }
    }
}

/// Materials of animal pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skin {
    /// The species' fur or plumage.
    Coat,
    /// Eyes, nose and beak: wet and glossy, colored by the vertices.
    Gloss,
    Hoof,
    Antler,
    /// Wing and tail feathers (double sided).
    Feather,
    /// Bare skin: bird legs.
    Bare,
}

/// Whether a species has its sculpted body yet (the others keep primitive stand-ins).
pub fn sculpted(species: Species) -> bool {
    matches!(species, Species::Deer | Species::Wolf)
}

/// The pieces of one animal with meshes and materials, or `None` without the renderer.
/// `tint` (linear RGB, 1 = as painted) multiplies the coat.
pub fn pieces(
    species: Species,
    male: bool,
    tint: Option<Vec3>,
    art: &mut ArtAssets,
) -> Option<Vec<(Piece, MeshRef, MaterialRef)>> {
    if !art.available() || !sculpted(species) {
        return None;
    }
    ensure_species(species, art);
    let list = piece_list(species, male);
    let mut out = Vec::with_capacity(list.len());
    for (piece, skin) in list {
        let key = mesh_key(species, piece, male);
        let mesh = art.mesh(&key, Mesh::default)?;
        let mut material = material(species, skin, art)?;
        if let (Some(t), Skin::Coat, MaterialRef::Inline(m)) = (tint, skin, &mut material) {
            m.base_color = [t.x, t.y, t.z, 1.0];
        }
        out.push((piece, mesh, material));
    }
    Some(out)
}

/// The pieces an animal is made of (also without the renderer).
pub fn piece_names(species: Species, male: bool) -> Vec<Piece> {
    piece_list(species, male)
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

fn species_key(s: Species) -> &'static str {
    match s {
        Species::Deer => "deer",
        Species::Wolf => "wolf",
        Species::Rabbit => "rabbit",
        Species::Bird => "bird",
    }
}

fn mesh_key(species: Species, piece: Piece, male: bool) -> String {
    let stag = species == Species::Deer && male && matches!(piece, Piece::Neck | Piece::Neck2);
    format!(
        "animals/{}/{}{}",
        species_key(species),
        piece.key(),
        if stag { "_stag" } else { "" }
    )
}

/// What each animal is made of.
fn piece_list(species: Species, male: bool) -> Vec<(Piece, Skin)> {
    let mut v = vec![
        (Piece::Body, Skin::Coat),
        (Piece::Head, Skin::Coat),
        (Piece::Gloss, Skin::Gloss),
    ];
    match species {
        Species::Deer => {
            v.extend([
                (Piece::Neck, Skin::Coat),
                (Piece::Neck2, Skin::Coat),
                (Piece::Tail, Skin::Coat),
            ]);
            v.extend([(Piece::Ear(0), Skin::Coat), (Piece::Ear(1), Skin::Coat)]);
            for k in 0..4 {
                for s in 0..4 {
                    v.push((Piece::Leg(k, s), Skin::Coat));
                }
                v.push((Piece::Hoof(k), Skin::Hoof));
            }
            if male {
                v.push((Piece::Antlers, Skin::Antler));
            }
        }
        Species::Wolf => {
            v.extend([(Piece::Neck, Skin::Coat), (Piece::Jaw, Skin::Coat)]);
            v.extend([(Piece::Tail, Skin::Coat), (Piece::Tail2, Skin::Coat)]);
            v.extend([(Piece::Ear(0), Skin::Coat), (Piece::Ear(1), Skin::Coat)]);
            for k in 0..4 {
                for s in 0..4 {
                    v.push((Piece::Leg(k, s), Skin::Coat));
                }
            }
        }
        Species::Rabbit => {
            v.extend([
                (Piece::Tail, Skin::Coat),
                (Piece::Ear(0), Skin::Coat),
                (Piece::Ear(1), Skin::Coat),
            ]);
            for k in 0..4 {
                for s in 0..2 {
                    v.push((Piece::Leg(k, s), Skin::Coat));
                }
            }
        }
        Species::Bird => {
            v.extend([
                (Piece::Tail, Skin::Feather),
                (Piece::TailFan, Skin::Feather),
            ]);
            for s in 0..2 {
                for w in 0..3 {
                    v.push((Piece::Wing(s, w), Skin::Feather));
                }
                v.push((Piece::Leg(s, 0), Skin::Bare));
            }
        }
    }
    v
}

fn material(species: Species, skin: Skin, art: &mut ArtAssets) -> Option<MaterialRef> {
    let k = species_key(species);
    let white = StandardMaterial::color(1.0, 1.0, 1.0);
    match skin {
        Skin::Coat => {
            let mut base = white;
            base.reflectance = 0.35;
            base.subsurface = if species == Species::Bird { 0.05 } else { 0.25 };
            let key = format!("animals/{k}/coat");
            art.material(&key, base, || coat_texture(species))
        }
        Skin::Gloss => {
            let mut m = white.with_roughness(0.22);
            m.reflectance = 0.55;
            Some(MaterialRef::Inline(m))
        }
        Skin::Hoof => art.material("animals/hoof", white, hoof_texture),
        Skin::Antler => art.material("animals/antler", white, antler_texture),
        Skin::Feather => {
            let mut base = white;
            base.double_sided = true;
            base.reflectance = 0.4;
            base.subsurface = 0.15;
            art.material("animals/bird/feather", base, feather_texture)
        }
        Skin::Bare => {
            let mut m = white.with_roughness(0.55);
            m.subsurface = 0.2;
            Some(MaterialRef::Inline(m))
        }
    }
}

/// Generate a species' meshes and textures (in parallel) unless they already exist.
fn ensure_species(species: Species, art: &mut ArtAssets) {
    let probe = format!("npc/{}", mesh_key(species, Piece::Body, false));
    if art
        .meshes
        .as_ref()
        .is_some_and(|m| m.loaded_by_key(&probe).is_some())
    {
        return;
    }
    let made = run_jobs(species_jobs(species));
    for m in made {
        match m {
            Made::Mesh(key, mesh) => {
                art.mesh(&key, move || mesh);
            }
            Made::Tex(key, set) => {
                art.textures(&key, move || set);
            }
        }
    }
}

// ======================================================================= jobs

/// Something generated off the main thread: a mesh or a texture set under its key.
pub enum Made {
    Mesh(String, Mesh),
    Tex(String, TextureSet),
}

type Job = Box<dyn FnOnce() -> Vec<Made> + Send>;

fn run_jobs(jobs: Vec<Job>) -> Vec<Made> {
    let n = jobs.len();
    let slots: Vec<Mutex<Option<Job>>> = jobs.into_iter().map(|j| Mutex::new(Some(j))).collect();
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism()
        .map_or(4, |t| t.get())
        .clamp(1, n.max(1));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let job = slots[i].lock().ok().and_then(|mut j| j.take());
                    if let Some(job) = job {
                        let made = job();
                        if let Ok(mut o) = out.lock() {
                            o.extend(made);
                        }
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_default()
}

/// Everything a species needs, as jobs (biggest first so the threads finish together).
pub fn species_jobs(species: Species) -> Vec<Job> {
    let k = species_key(species);
    let mut jobs: Vec<Job> = Vec::new();
    let coat_key = format!("animals/{k}/coat");
    jobs.push(Box::new(move || {
        vec![Made::Tex(coat_key, coat_texture(species))]
    }));
    match species {
        Species::Deer => {
            jobs.push(Box::new(|| {
                vec![Made::Tex("animals/hoof".into(), hoof_texture())]
            }));
            jobs.push(Box::new(|| {
                vec![Made::Tex("animals/antler".into(), antler_texture())]
            }));
            deer::jobs(&mut jobs);
        }
        Species::Wolf => wolf::jobs(&mut jobs),
        Species::Rabbit | Species::Bird => {}
    }
    jobs
}

/// Build every mesh of `species` now and return `(key, triangles)` per mesh (tests, budgets).
pub fn build_all(species: Species) -> (Vec<(String, usize)>, f32) {
    let start = std::time::Instant::now();
    let made = run_jobs(species_jobs(species));
    let secs = start.elapsed().as_secs_f32();
    let mut out: Vec<(String, usize)> = made
        .into_iter()
        .filter_map(|m| match m {
            Made::Mesh(k, mesh) => Some((k, mesh.triangle_count())),
            Made::Tex(..) => None,
        })
        .collect();
    out.sort();
    (out, secs)
}

/// Triangles of one animal: the pieces it is made of.
pub fn triangles_per_animal(species: Species, male: bool, meshes: &[(String, usize)]) -> usize {
    piece_list(species, male)
        .into_iter()
        .map(|(p, _)| {
            let key = mesh_key(species, p, male);
            meshes
                .iter()
                .find(|(k, _)| *k == key)
                .map_or(0, |(_, t)| *t)
        })
        .sum()
}

/// How many pieces one animal has.
pub fn piece_count(species: Species, male: bool) -> usize {
    piece_list(species, male).len()
}

// ======================================================================= sdf meshing

type Field<'a> = &'a (dyn Fn(Vec3) -> f32 + Sync);

/// A plain signed distance function.
type SdfFn = fn(Vec3) -> f32;

/// A box in a piece's own frame: `rot` turns local coordinates into rest coordinates.
#[derive(Debug, Clone, Copy)]
struct Region {
    center: Vec3,
    rot: Quat,
    half: Vec3,
    voxel: Vec3,
}

impl Region {
    fn aabb(min: Vec3, max: Vec3, voxel: Vec3) -> Self {
        Self {
            center: (min + max) * 0.5,
            rot: Quat::IDENTITY,
            half: (max - min) * 0.5,
            voxel,
        }
    }

    /// A box around the segment `a`→`b` (its local Y), reaching `radius` around it and past
    /// both ends; voxels `across` the bone and `along` it.
    fn bone(a: Vec3, b: Vec3, radius: f32, across: f32, along: f32) -> Self {
        let d = b - a;
        let len = d.length().max(1e-4);
        Self {
            center: (a + b) * 0.5,
            rot: Quat::from_rotation_arc(Vec3::Y, d / len),
            half: Vec3::new(radius, len * 0.5 + radius, radius),
            voxel: Vec3::new(across, along, across),
        }
    }
}

/// Surface nets over `r`: positions in rest coordinates and triangle indices. Only blocks
/// near the surface are sampled finely.
fn nets(f: Field, r: &Region) -> (Vec<Vec3>, Vec<u32>) {
    let v = r.voxel;
    let size = (r.half * 2.0 / v).ceil();
    let (nx, ny, nz) = (
        size.x as usize + 2,
        size.y as usize + 2,
        size.z as usize + 2,
    );
    let origin = -r.half - v * 0.5;
    let local = |x: usize, y: usize, z: usize| origin + Vec3::new(x as f32, y as f32, z as f32) * v;
    let rest = |q: Vec3| r.center + r.rot * q;
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let mut field = vec![0.0f32; nx * ny * nz];
    const B: usize = 4;
    let reach = (v * B as f32).length() * 0.5 * 1.3 + v.max_element();
    for bz in (0..nz).step_by(B) {
        for by in (0..ny).step_by(B) {
            for bx in (0..nx).step_by(B) {
                let (ex, ey, ez) = ((bx + B).min(nx), (by + B).min(ny), (bz + B).min(nz));
                let c = (local(bx, by, bz) + local(ex - 1, ey - 1, ez - 1)) * 0.5;
                let dc = f(rest(c));
                let far = dc.abs() > reach;
                for z in bz..ez {
                    for y in by..ey {
                        for x in bx..ex {
                            field[idx(x, y, z)] = if far { dc } else { f(rest(local(x, y, z))) };
                        }
                    }
                }
            }
        }
    }
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cidx = |x: usize, y: usize, z: usize| (z * cy + y) * cx + x;
    let mut cell_vertex = vec![u32::MAX; cx * cy * cz];
    let mut positions: Vec<Vec3> = Vec::new();
    const CORNERS: [(usize, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (1, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (0, 1, 1),
        (1, 1, 1),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let d: [f32; 8] = CORNERS.map(|(a, b, c)| field[idx(x + a, y + b, z + c)]);
                let inside = d.iter().filter(|&&s| s < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                let mut sum = Vec3::ZERO;
                let mut n = 0.0;
                for (a, b) in EDGES {
                    if (d[a] < 0.0) != (d[b] < 0.0) {
                        let t = d[a] / (d[a] - d[b]);
                        let ca = CORNERS[a];
                        let cb = CORNERS[b];
                        let pa = Vec3::new(ca.0 as f32, ca.1 as f32, ca.2 as f32);
                        let pb = Vec3::new(cb.0 as f32, cb.1 as f32, cb.2 as f32);
                        sum += pa + (pb - pa) * t;
                        n += 1.0;
                    }
                }
                cell_vertex[cidx(x, y, z)] = positions.len() as u32;
                positions.push(local(x, y, z) + sum / n * v);
            }
        }
    }
    let mut indices = Vec::new();
    let mut quad = |a: u32, b: u32, c: u32, d: u32, out: Vec3| {
        if [a, b, c, d].contains(&u32::MAX) {
            return;
        }
        let [pa, pb, pc] = [a, b, c].map(|i| positions[i as usize]);
        if (pb - pa).cross(pc - pa).dot(out) >= 0.0 {
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        } else {
            indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    };
    for z in 1..cz {
        for y in 1..cy {
            for x in 0..cx {
                let (s0, s1) = (field[idx(x, y, z)], field[idx(x + 1, y, z)]);
                if (s0 < 0.0) != (s1 < 0.0) {
                    quad(
                        cell_vertex[cidx(x, y - 1, z - 1)],
                        cell_vertex[cidx(x, y, z - 1)],
                        cell_vertex[cidx(x, y, z)],
                        cell_vertex[cidx(x, y - 1, z)],
                        if s0 < 0.0 { Vec3::X } else { Vec3::NEG_X },
                    );
                }
            }
        }
    }
    for z in 1..cz {
        for y in 0..cy {
            for x in 1..cx {
                let (s0, s1) = (field[idx(x, y, z)], field[idx(x, y + 1, z)]);
                if (s0 < 0.0) != (s1 < 0.0) {
                    quad(
                        cell_vertex[cidx(x - 1, y, z - 1)],
                        cell_vertex[cidx(x - 1, y, z)],
                        cell_vertex[cidx(x, y, z)],
                        cell_vertex[cidx(x, y, z - 1)],
                        if s0 < 0.0 { Vec3::Y } else { Vec3::NEG_Y },
                    );
                }
            }
        }
    }
    for z in 0..cz {
        for y in 1..cy {
            for x in 1..cx {
                let (s0, s1) = (field[idx(x, y, z)], field[idx(x, y, z + 1)]);
                if (s0 < 0.0) != (s1 < 0.0) {
                    quad(
                        cell_vertex[cidx(x - 1, y - 1, z)],
                        cell_vertex[cidx(x, y - 1, z)],
                        cell_vertex[cidx(x, y, z)],
                        cell_vertex[cidx(x - 1, y, z)],
                        if s0 < 0.0 { Vec3::Z } else { Vec3::NEG_Z },
                    );
                }
            }
        }
    }
    (positions.into_iter().map(rest).collect(), indices)
}

/// Unit gradient of `f` at `p` (tetrahedral differences).
fn gradient(f: Field, p: Vec3, e: f32) -> Vec3 {
    const K: [Vec3; 4] = [
        v3(1.0, -1.0, -1.0),
        v3(-1.0, -1.0, 1.0),
        v3(-1.0, 1.0, -1.0),
        v3(1.0, 1.0, 1.0),
    ];
    let mut g = Vec3::ZERO;
    for k in K {
        g += k * f(p + k * e);
    }
    g.normalize_or(Vec3::Y)
}

/// How a piece is painted: UV flow, colors and occlusion.
struct Paint<'a> {
    flow: Flow,
    /// Rest-pose color (linear RGB) at a surface point with its normal.
    color: &'a (dyn Fn(Vec3, Vec3) -> Vec3 + Sync),
    /// The whole body (for ambient occlusion).
    body: Field<'a>,
    /// Size of the animal: scales the occlusion probe distances.
    scale: f32,
    pivot: Vec3,
}

/// Baked ambient occlusion at `p` (normal `n`): small creases against the piece's own `near`
/// field (so seams where posed pieces meet stay clean), wider shadowing against `far`.
fn occlusion(near: Field, far: Field, p: Vec3, n: Vec3, scale: f32) -> f32 {
    let mut occ = 0.0;
    for (dist, w) in [(0.01, 0.3), (0.028, 0.3), (0.065, 0.25), (0.14, 0.2)] {
        let field = if dist < 0.05 { near } else { far };
        let dist = dist * scale;
        let d = field(p + n * dist);
        occ += w * ((dist - d) / dist).clamp(0.0, 1.0);
    }
    (1.0 - occ * 1.15).clamp(0.25, 1.0)
}

/// Shrink `r` to the part of it where `f` has its surface (plus a margin), sampling coarsely.
fn fit(f: Field, r: Region) -> Region {
    let step = r.voxel * 3.0;
    let n = (r.half * 2.0 / step).ceil();
    let (nx, ny, nz) = (n.x as i32, n.y as i32, n.z as i32);
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let reach = step.length();
    for z in 0..=nz {
        for y in 0..=ny {
            for x in 0..=nx {
                let q = -r.half + Vec3::new(x as f32, y as f32, z as f32) * step;
                if f(r.center + r.rot * q) < reach {
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
            }
        }
    }
    if lo.x > hi.x {
        return r;
    }
    let lo = (lo - step).max(-r.half);
    let hi = (hi + step).min(r.half);
    Region {
        center: r.center + r.rot * ((lo + hi) * 0.5),
        half: (hi - lo) * 0.5,
        ..r
    }
}

/// A symmetric 4×4 error quadric (Garland & Heckbert), upper triangle.
#[derive(Debug, Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: DVec3, d: f64, w: f64) -> Self {
        let (a, b, c) = (n.x, n.y, n.z);
        Self([
            a * a * w,
            a * b * w,
            a * c * w,
            a * d * w,
            b * b * w,
            b * c * w,
            b * d * w,
            c * c * w,
            c * d * w,
            d * d * w,
        ])
    }

    fn add(&mut self, o: &Quadric) {
        for i in 0..10 {
            self.0[i] += o.0[i];
        }
    }

    fn error(&self, p: DVec3) -> f64 {
        let q = &self.0;
        let (x, y, z) = (p.x, p.y, p.z);
        (q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9])
            .max(0.0)
    }
}

/// The vertices sharing a live triangle with `v` (`fv` lists `v`'s triangles), sorted.
fn neighbors(fv: &[u32], v: usize, out: &mut Vec<u32>, faces: &[[u32; 3]], face_alive: &[bool]) {
    out.clear();
    for &f in fv {
        if face_alive[f as usize] {
            for w in faces[f as usize] {
                if w as usize != v {
                    out.push(w);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
}

/// Simplify a closed triangle mesh to about `target` triangles by edge collapses ordered by
/// quadric error: flat areas lose vertices first, curved ones and edges between differently
/// painted vertices (`paint`) keep them. Collapses that would fold a triangle over or pinch
/// the surface are skipped.
fn decimate(pos: Vec<Vec3>, idx: Vec<u32>, target: usize, paint: &[Vec3]) -> (Vec<Vec3>, Vec<u32>) {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let nv = pos.len();
    let mut tris = idx.len() / 3;
    if tris <= target {
        return (pos, idx);
    }
    let mut p: Vec<DVec3> = pos.iter().map(|v| v.as_dvec3()).collect();
    let mut faces: Vec<[u32; 3]> = idx.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut face_alive = vec![true; faces.len()];
    let mut vf: Vec<Vec<u32>> = vec![Vec::new(); nv];
    let mut q = vec![Quadric::default(); nv];
    for (fi, f) in faces.iter().enumerate() {
        let [a, b, c] = f.map(|v| p[v as usize]);
        let n = (b - a).cross(c - a);
        let len = n.length();
        for &v in f {
            vf[v as usize].push(fi as u32);
        }
        if len < 1e-18 {
            continue;
        }
        let nn = n / len;
        let pq = Quadric::plane(nn, -nn.dot(a), len * 0.5);
        for &v in f {
            q[v as usize].add(&pq);
        }
    }
    let paint: Vec<Vec3> = paint.to_vec();
    let mut alive = vec![true; nv];
    let mut stamp = vec![0u32; nv];
    let mut heap = BinaryHeap::new();
    let cost = |q: &[Quadric], p: &[DVec3], a: usize, b: usize| -> (f64, DVec3) {
        let mut qq = q[a];
        qq.add(&q[b]);
        let mid = (p[a] + p[b]) * 0.5;
        let mut best = (qq.error(mid), mid);
        for c in [p[a], p[b]] {
            let e = qq.error(c);
            if e < best.0 {
                best = (e, c);
            }
        }
        let dc = (paint[a] - paint[b]).length_squared() as f64;
        let l2 = p[a].distance_squared(p[b]);
        (best.0 + 2e-5 * dc * l2 + 1e-9 * l2 * l2, best.1)
    };
    let mut seen = std::collections::HashSet::new();
    for f in &faces {
        for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[0])] {
            let (a, b) = (a.min(b) as usize, a.max(b) as usize);
            if seen.insert((a, b)) {
                let (c, _) = cost(&q, &p, a, b);
                heap.push(Reverse((c.to_bits(), a as u32, b as u32, 0u32, 0u32)));
            }
        }
    }
    drop(seen);
    let mut na: Vec<u32> = Vec::new();
    let mut nb: Vec<u32> = Vec::new();
    while tris > target {
        let Some(Reverse((_, a, b, sa, sb))) = heap.pop() else {
            break;
        };
        let (a, b) = (a as usize, b as usize);
        if !alive[a] || !alive[b] || stamp[a] != sa || stamp[b] != sb {
            continue;
        }
        let (_, np) = cost(&q, &p, a, b);
        // Link condition: the only shared neighbors are across the edge's two triangles.
        neighbors(&vf[a], a, &mut na, &faces, &face_alive);
        neighbors(&vf[b], b, &mut nb, &faces, &face_alive);
        let shared = na.iter().filter(|v| nb.binary_search(v).is_ok()).count();
        let across = vf[a]
            .iter()
            .filter(|&&f| face_alive[f as usize] && faces[f as usize].contains(&(b as u32)))
            .count();
        if shared != across || across != 2 {
            continue;
        }
        // No folded triangles.
        let mut ok = true;
        for (v, other) in [(a, b), (b, a)] {
            for &f in &vf[v] {
                let fi = f as usize;
                if !face_alive[fi] || faces[fi].contains(&(other as u32)) {
                    continue;
                }
                let t = faces[fi];
                let old = t.map(|w| p[w as usize]);
                let new = t.map(|w| if w as usize == v { np } else { p[w as usize] });
                let n0 = (old[1] - old[0]).cross(old[2] - old[0]);
                let n1 = (new[1] - new[0]).cross(new[2] - new[0]);
                let l1 = n1.length();
                if l1 < 1e-14 || n0.normalize_or_zero().dot(n1 / l1) < 0.3 {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
        }
        if !ok {
            continue;
        }
        // Collapse b into a.
        let fb = std::mem::take(&mut vf[b]);
        for f in fb {
            let fi = f as usize;
            if !face_alive[fi] {
                continue;
            }
            if faces[fi].contains(&(a as u32)) {
                face_alive[fi] = false;
                tris -= 1;
            } else {
                for w in faces[fi].iter_mut() {
                    if *w as usize == b {
                        *w = a as u32;
                    }
                }
                vf[a].push(f);
            }
        }
        vf[a].retain(|&f| face_alive[f as usize]);
        alive[b] = false;
        p[a] = np;
        let qb = q[b];
        q[a].add(&qb);
        stamp[a] += 1;
        neighbors(&vf[a], a, &mut na, &faces, &face_alive);
        for &c in &na {
            let c = c as usize;
            let (x, y) = (a.min(c), a.max(c));
            let (cst, _) = cost(&q, &p, x, y);
            heap.push(Reverse((
                cst.to_bits(),
                x as u32,
                y as u32,
                stamp[x],
                stamp[y],
            )));
        }
    }
    // Compact.
    let mut remap = vec![u32::MAX; nv];
    let mut out_pos = Vec::new();
    let mut out_idx = Vec::with_capacity(tris * 3);
    for (fi, f) in faces.iter().enumerate() {
        if !face_alive[fi] {
            continue;
        }
        for &v in f {
            let v = v as usize;
            if remap[v] == u32::MAX {
                remap[v] = out_pos.len() as u32;
                out_pos.push(p[v].as_vec3());
            }
            out_idx.push(remap[v]);
        }
    }
    (out_pos, out_idx)
}

/// Mesh the zero surface of `f` in `region`, simplified to about `target` triangles,
/// projected onto the surface, painted and stored relative to the pivot.
fn sculpt(f: Field, region: Region, target: usize, paint: &Paint) -> Mesh {
    let region = fit(f, region);
    let (positions, indices) = nets(f, &region);
    let e = region.voxel.min_element() * 0.35;
    let rough: Vec<Vec3> = positions
        .iter()
        .map(|&p| (paint.color)(p, gradient(f, p, e)))
        .collect();
    let (positions, indices) = decimate(positions, indices, target, &rough);
    let ao_field = |p: Vec3| f(p).min((paint.body)(p));
    let vertices = positions
        .iter()
        .map(|&p0| {
            let mut p = p0;
            for _ in 0..2 {
                let d = f(p);
                if d.abs() > region.voxel.max_element() * 2.0 {
                    break;
                }
                p -= gradient(f, p, e) * d;
            }
            let n = gradient(f, p, e);
            let ao = occlusion(&f, &ao_field, p, n, paint.scale);
            let c = (paint.color)(p, n) * ao;
            Vertex::new(p - paint.pivot, n, paint.flow.uv(p)).with_color(c.extend(1.0))
        })
        .collect();
    Mesh::new(vertices, indices)
}

/// The mirror image of a mesh across the YZ plane (left pieces from right ones).
fn mirrored(mesh: &Mesh) -> Mesh {
    let vertices = mesh
        .vertices
        .iter()
        .map(|v| {
            let mut v = *v;
            v.position[0] = -v.position[0];
            v.normal[0] = -v.normal[0];
            v
        })
        .collect();
    let mut indices = mesh.indices.clone();
    for t in indices.chunks_mut(3) {
        t.swap(1, 2);
    }
    Mesh::new(vertices, indices)
}

// ======================================================================= uv flow

/// Texture coordinates whose +V runs the way the hair lies: along `axis` around a capsule-
/// like piece, with polar caps beyond `start`/`end`, mirrored at the `up` side (the parting)
/// and sheared by `tilt` so strands lean toward the far side. In tiles of `tile` meters.
#[derive(Debug, Clone, Copy)]
struct Flow {
    origin: Vec3,
    axis: Vec3,
    up: Vec3,
    start: f32,
    end: f32,
    tilt: f32,
    tile: f32,
}

impl Flow {
    fn new(from: Vec3, to: Vec3, up: Vec3, tile: f32) -> Self {
        let d = to - from;
        let len = d.length().max(1e-4);
        let axis = d / len;
        let up = (up - axis * up.dot(axis)).normalize_or(axis.any_orthonormal_vector());
        Self {
            origin: from,
            axis,
            up,
            start: 0.0,
            end: len,
            tilt: 0.0,
            tile,
        }
    }

    fn tilt(mut self, t: f32) -> Self {
        self.tilt = t;
        self
    }

    fn uv(&self, p: Vec3) -> Vec2 {
        let d = p - self.origin;
        let z = d.dot(self.axis);
        let perp = d - self.axis * z;
        let r = perp.length();
        let side = self.axis.cross(self.up);
        let phi = perp.dot(side).atan2(perp.dot(self.up)).abs();
        let v = if z < self.start {
            let dz = self.start - z;
            self.start - dz.atan2(r) * (r * r + dz * dz).sqrt()
        } else if z > self.end {
            let dz = z - self.end;
            self.end + dz.atan2(r) * (r * r + dz * dz).sqrt()
        } else {
            z
        };
        let u = phi * r;
        Vec2::new(u - v * self.tilt, v) / self.tile
    }
}

// ======================================================================= explicit shapes

/// Raw geometry before painting.
#[derive(Default)]
struct Geo {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    indices: Vec<u32>,
}

impl Geo {
    fn append(&mut self, o: Geo) {
        let base = self.positions.len() as u32;
        self.positions.extend(o.positions);
        self.normals.extend(o.normals);
        self.uvs.extend(o.uvs);
        self.indices.extend(o.indices.into_iter().map(|i| i + base));
    }

    /// Paint and store relative to `pivot`. `color` gets the rest position, normal and UV.
    fn finish(
        self,
        pivot: Vec3,
        body: Field,
        scale: f32,
        color: &dyn Fn(Vec3, Vec3, Vec2) -> Vec3,
    ) -> Mesh {
        let vertices = (0..self.positions.len())
            .map(|i| {
                let (p, n, uv) = (self.positions[i], self.normals[i], self.uvs[i]);
                let ao = occlusion(body, body, p, n, scale).max(0.55);
                let c = color(p, n, uv) * ao;
                Vertex::new(p - pivot, n, uv).with_color(c.extend(1.0))
            })
            .collect();
        Mesh::new(vertices, self.indices)
    }
}

/// A UV sphere.
fn ball(center: Vec3, radius: Vec3, rot: Quat, rings: usize, sides: usize) -> Geo {
    let mut g = Geo::default();
    for i in 0..=rings {
        let th = PI * i as f32 / rings as f32;
        for j in 0..=sides {
            let ph = TAU * j as f32 / sides as f32;
            let unit = Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
            g.positions.push(center + rot * (unit * radius));
            g.normals.push((rot * (unit / radius)).normalize());
            g.uvs
                .push(Vec2::new(j as f32 / sides as f32, i as f32 / rings as f32));
        }
    }
    let w = sides as u32 + 1;
    for i in 0..rings as u32 {
        for j in 0..sides as u32 {
            let (a, b, c, d) = (
                i * w + j,
                i * w + j + 1,
                (i + 1) * w + j + 1,
                (i + 1) * w + j,
            );
            g.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    g
}

/// Catmull-Rom points through `p`, `per` samples per span, and the matching radii.
fn spline(p: &[Vec3], r: &[f32], per: usize) -> (Vec<Vec3>, Vec<f32>) {
    let n = p.len();
    let mut pts = Vec::new();
    let mut rad = Vec::new();
    for i in 0..n - 1 {
        let p0 = p[i.saturating_sub(1)];
        let (p1, p2) = (p[i], p[i + 1]);
        let p3 = p[(i + 2).min(n - 1)];
        for s in 0..per {
            let t = s as f32 / per as f32;
            let (t2, t3) = (t * t, t * t * t);
            pts.push(
                0.5 * ((2.0 * p1)
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3),
            );
            rad.push(r[i] + (r[i + 1] - r[i]) * t);
        }
    }
    pts.push(p[n - 1]);
    rad.push(r[n - 1]);
    (pts, rad)
}

/// A tube along `pts` with `rad` radii and a pointed tip; UV u around (meters), v along.
fn tube(pts: &[Vec3], rad: &[f32], sides: usize) -> Geo {
    let mut g = Geo::default();
    let n = pts.len();
    let tangent =
        |i: usize| (pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)]).normalize_or(Vec3::Y);
    let mut normal = tangent(0).any_orthonormal_vector();
    let mut along = 0.0;
    for i in 0..n {
        let t = tangent(i);
        normal = (normal - t * normal.dot(t)).normalize_or(t.any_orthonormal_vector());
        let bin = t.cross(normal);
        if i > 0 {
            along += pts[i].distance(pts[i - 1]);
        }
        let dr = if i + 1 < n {
            (rad[i + 1] - rad[i]) / pts[i + 1].distance(pts[i]).max(1e-5)
        } else {
            (rad[i] - rad[i - 1]) / pts[i].distance(pts[i - 1]).max(1e-5)
        };
        for j in 0..=sides {
            let a = TAU * j as f32 / sides as f32;
            let radial = normal * a.cos() + bin * a.sin();
            g.positions.push(pts[i] + radial * rad[i]);
            g.normals.push((radial - t * dr).normalize());
            g.uvs.push(Vec2::new(a * rad[0], along));
        }
    }
    let w = sides as u32 + 1;
    for i in 0..n as u32 - 1 {
        for j in 0..sides as u32 {
            let (a, b, c, d) = (
                i * w + j,
                i * w + j + 1,
                (i + 1) * w + j + 1,
                (i + 1) * w + j,
            );
            g.indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }
    // Close the tip.
    let tip = pts[n - 1] + tangent(n - 1) * rad[n - 1];
    let ti = g.positions.len() as u32;
    g.positions.push(tip);
    g.normals.push(tangent(n - 1));
    g.uvs.push(Vec2::new(0.0, along + rad[n - 1]));
    let last = (n as u32 - 1) * w;
    for j in 0..sides as u32 {
        g.indices.extend_from_slice(&[last + j, last + j + 1, ti]);
    }
    g
}

// ======================================================================= helpers

fn ell(p: Vec3, c: Vec3, r: Vec3) -> f32 {
    ellipsoid(p, c, r)
}

fn cone(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    round_cone(p, a, b, ra, rb)
}

/// A round cone with an elliptical cross-section: `squash` > 1 makes it narrower across X,
/// < 1 wider.
fn cone_x(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32, squash: f32) -> f32 {
    let s = Vec3::new(squash, 1.0, 1.0);
    round_cone(p * s, a * s, b * s, ra, rb) / squash.max(1.0)
}

/// Signed distance to a convex polygon in the plane (vertices counter-clockwise), exact
/// inside and close enough outside for small roundings.
fn convex2(p: Vec2, poly: &[Vec2]) -> f32 {
    let mut d = f32::MIN;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let e = (b - a).normalize();
        d = d.max((p - a).perp_dot(e));
    }
    d
}

/// Mirror to the right side (+X).
fn mx(p: Vec3) -> Vec3 {
    Vec3::new(p.x.abs(), p.y, p.z)
}

/// Point `p` in a local frame at `o` with axes `x`, `y`, `z` (orthonormal).
fn local(p: Vec3, o: Vec3, x: Vec3, y: Vec3, z: Vec3) -> Vec3 {
    let d = p - o;
    Vec3::new(d.dot(x), d.dot(y), d.dot(z))
}

/// Gentle large-scale variation, about ±1.
fn mottle(p: Vec3, freq: f32, seed: u32) -> f32 {
    fbm3(p * freq, 3, 0.5, seed)
}

/// A part of a body for whole-body occlusion: bounding sphere and field.
struct Bound {
    center: Vec3,
    radius: f32,
    f: fn(Vec3) -> f32,
}

/// The union of `parts` (fields far from `p` are skipped).
fn union_of(parts: &[Bound], p: Vec3) -> f32 {
    let mut best = 0.4f32;
    for b in parts {
        let lb = p.distance(b.center) - b.radius;
        if lb < best {
            best = best.min((b.f)(p));
        }
    }
    best
}

/// One sculpted piece: its field, where to mesh it, how finely, how its fur runs and the joint
/// it turns about.
struct Spec {
    piece: Piece,
    f: SdfFn,
    region: Region,
    target: usize,
    flow: Flow,
    pivot: Vec3,
    /// Also store the mirror image as this (left) piece.
    mirror: Option<Piece>,
    /// A variant of the piece (a stag's neck).
    variant: bool,
}

/// How a species paints and shades its pieces.
#[derive(Clone, Copy)]
struct Kind {
    key: &'static str,
    /// Coat color (linear RGB) at a rest-pose point with its normal; the flag is `variant`.
    coat: fn(Piece, Vec3, Vec3, bool) -> Vec3,
    /// The whole body, for occlusion.
    body: SdfFn,
    /// Size of the animal relative to a deer (scales occlusion probes).
    scale: f32,
}

impl Spec {
    fn run(self, kind: Kind) -> Vec<Made> {
        let (piece, variant) = (self.piece, self.variant);
        let color = move |p: Vec3, n: Vec3| (kind.coat)(piece, p, n, variant);
        let paint = Paint {
            flow: self.flow,
            color: &color,
            body: &kind.body,
            scale: kind.scale,
            pivot: self.pivot,
        };
        let mesh = sculpt(&self.f, self.region, self.target, &paint);
        let suffix = if variant { "_stag" } else { "" };
        let mut out = Vec::new();
        if let Some(m) = self.mirror {
            out.push(Made::Mesh(
                format!("animals/{}/{}{suffix}", kind.key, m.key()),
                mirrored(&mesh),
            ));
        }
        out.push(Made::Mesh(
            format!("animals/{}/{}{suffix}", kind.key, piece.key()),
            mesh,
        ));
        out
    }
}

// ======================================================================= textures

/// A tiling fur texture: overlapping tufts of strands lying along +V, with darker roots,
/// per-tuft and per-strand variation and optional agouti (dark-tipped) guard hairs.
#[derive(Debug, Clone, Copy)]
struct Fur {
    /// Tufts across and along the tile (rows must be even so the stagger tiles).
    cells: i32,
    /// Tuft length and width in cells.
    len: f32,
    width: f32,
    strands: f32,
    /// Height of the tufts, meters.
    height: f32,
    /// Brightness variation between tufts and strands.
    contrast: f32,
    /// Share of strands with dark tips and a pale band (agouti).
    agouti: f32,
    tint: Vec3,
    band: Vec3,
    roughness: f32,
    /// Brightness variation in long streaks along the hair.
    streaks: f32,
    seed: u32,
}

/// Smooth value noise tiling every `nx` × `ny` cells over the unit square, −1..1.
fn tiled_noise(uv: Vec2, nx: i32, ny: i32, seed: u32) -> f32 {
    let q = uv * Vec2::new(nx as f32, ny as f32);
    let (ix, iy) = (q.x.floor() as i32, q.y.floor() as i32);
    let f = q - Vec2::new(ix as f32, iy as f32);
    let f = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let v = |dx: i32, dy: i32| {
        rand3((ix + dx).rem_euclid(nx), (iy + dy).rem_euclid(ny), 5, seed) * 2.0 - 1.0
    };
    let a = v(0, 0) + (v(1, 0) - v(0, 0)) * f.x;
    let b = v(0, 1) + (v(1, 1) - v(0, 1)) * f.x;
    a + (b - a) * f.y
}

fn fur_texel(f: &Fur, uv: Vec2) -> Texel {
    let n = f.cells;
    let q = uv * n as f32;
    let (ix, iy) = (q.x.floor() as i32, q.y.floor() as i32);
    let reach = f.len.ceil() as i32;
    // The visible tuft is the one rooted farthest upstream (it lies on top).
    let mut best: Option<(f32, f32, f32, f32, i32, i32)> = None; // root y, along t, across s, id, col, row
    for dy in 0..=reach {
        let row = iy - dy;
        for dx in -1..=1 {
            let col = ix + dx;
            let (wc, wr) = (col.rem_euclid(n), row.rem_euclid(n));
            let stagger = if wr % 2 == 1 { 0.5 } else { 0.0 };
            let root = Vec2::new(
                col as f32 + 0.5 + stagger + (rand3(wc, wr, 0, f.seed) - 0.5) * 0.7,
                row as f32 + rand3(wc, wr, 1, f.seed) * 0.8,
            );
            let ang = (rand3(wc, wr, 2, f.seed) - 0.5) * 0.4;
            let dir = Vec2::new(ang.sin(), ang.cos());
            let d = q - root;
            let along = d.dot(dir);
            let len = f.len * (0.8 + 0.4 * rand3(wc, wr, 3, f.seed));
            if along < 0.0 || along > len {
                continue;
            }
            let t = along / len;
            let across = d.x * dir.y - d.y * dir.x;
            let w = f.width
                * 0.5
                * (1.0 - t.powf(1.8)).max(0.0).sqrt()
                * (0.55 + 0.45 * smooth(0.0, 0.25, t));
            if across.abs() > w {
                continue;
            }
            if best.is_none_or(|b| root.y < b.0) {
                best = Some((root.y, t, across / w, rand3(wc, wr, 4, f.seed), wc, wr));
            }
        }
    }
    let streak = 1.0
        + f.streaks
            * (0.65 * tiled_noise(uv, 9, 2, f.seed) + 0.35 * tiled_noise(uv, 23, 4, f.seed + 1));
    let Some((_, t, s, id, wc, wr)) = best else {
        return Texel {
            albedo: f.tint * 0.6 * streak,
            height: 0.0,
            roughness: f.roughness + 0.05,
            metallic: 0.0,
            occlusion: 0.35,
        };
    };
    // Strands converge toward the tip.
    let k = s * f.strands + id * 37.0;
    let strand = perlin2(Vec2::new(k, t * 1.3 + id * 11.0), 0, f.seed ^ 0x55);
    let ridge = 1.0 - (strand * 2.2).abs().min(1.0);
    let sid = (k * 0.5).floor() as i32;
    let sr = rand3(sid, wc, wr, f.seed ^ 0x77);
    let lift = smooth(0.0, 0.4, t) * (1.0 - 0.45 * smooth(0.75, 1.0, t));
    let height = f.height * (0.25 + 0.75 * lift) * (1.0 - 0.45 * s * s) + f.height * 0.18 * ridge;
    let mut albedo = f.tint
        * (1.0 + f.contrast * (id - 0.5))
        * (1.0 + f.contrast * 0.8 * (sr - 0.5))
        * (0.62 + 0.38 * smooth(0.0, 0.35, t))
        * (0.9 + 0.1 * ridge)
        * streak;
    if f.agouti > 0.0 {
        let guard = sr < f.agouti;
        if guard && t > 0.66 {
            albedo *= 0.4 + 0.3 * (1.0 - smooth(0.66, 0.78, t));
        } else if t > 0.35 && t < 0.62 {
            albedo = mix(
                albedo,
                f.band,
                0.35 * smooth(0.35, 0.45, t) * (1.0 - smooth(0.52, 0.62, t)),
            );
        }
    }
    Texel {
        albedo,
        height,
        roughness: f.roughness - 0.08 * smooth(0.5, 1.0, t) + 0.05 * (sr - 0.5),
        metallic: 0.0,
        occlusion: 0.45 + 0.55 * smooth(0.0, 0.45, t) * (1.0 - 0.25 * s * s),
    }
}

fn coat_texture(species: Species) -> TextureSet {
    match species {
        Species::Deer => {
            let f = Fur {
                cells: 14,
                len: 3.6,
                width: 1.15,
                strands: 7.0,
                height: 0.0009,
                contrast: 0.22,
                agouti: 0.0,
                tint: Vec3::new(1.0, 0.97, 0.94),
                band: Vec3::ONE,
                roughness: 0.8,
                streaks: 0.12,
                seed: 11,
            };
            TextureSet::bake(512, 0.16, |uv| fur_texel(&f, uv))
        }
        Species::Wolf => {
            let f = Fur {
                cells: 20,
                len: 3.2,
                width: 1.3,
                strands: 11.0,
                height: 0.0016,
                contrast: 0.1,
                agouti: 0.5,
                tint: Vec3::new(1.0, 0.98, 0.95),
                band: Vec3::new(1.25, 1.15, 0.95),
                roughness: 0.82,
                streaks: 0.12,
                seed: 23,
            };
            TextureSet::bake(512, 0.2, |uv| fur_texel(&f, uv))
        }
        Species::Rabbit => {
            let f = Fur {
                cells: 24,
                len: 2.4,
                width: 1.4,
                strands: 5.0,
                height: 0.0006,
                contrast: 0.18,
                agouti: 0.35,
                tint: Vec3::new(1.0, 0.98, 0.95),
                band: Vec3::new(1.2, 1.1, 0.9),
                roughness: 0.85,
                streaks: 0.1,
                seed: 31,
            };
            TextureSet::bake(512, 0.07, |uv| fur_texel(&f, uv))
        }
        Species::Bird => TextureSet::bake(4, 0.05, |_| Texel::default()),
    }
}

fn hoof_texture() -> TextureSet {
    TextureSet::bake(128, 0.05, |uv| {
        let lines = perlin2(Vec2::new(uv.x * 40.0, uv.y * 3.0), 40, 3);
        let rings = perlin2(Vec2::new(uv.x * 2.0, uv.y * 24.0), 2, 4);
        let n = lines * 0.7 + rings * 0.3;
        Texel {
            albedo: Vec3::splat(0.9 + 0.12 * n),
            height: 0.00015 * n,
            roughness: 0.42 + 0.08 * lines,
            metallic: 0.0,
            occlusion: 1.0,
        }
    })
}

fn antler_texture() -> TextureSet {
    TextureSet::bake(256, 0.08, |uv| {
        // Grooves along the beam and pearled bumps.
        let groove = super::noise::ridged2(Vec2::new(uv.x * 10.0, uv.y * 1.5), 10, 3, 7);
        let (d1, _, id) = super::noise::worley2(uv * 18.0, 18, 9);
        let pearl = (1.0 - d1 * 2.2).max(0.0) * (0.5 + id);
        let h = groove * 0.6 + pearl * 0.5;
        Texel {
            albedo: Vec3::splat(0.75 + 0.3 * h),
            height: 0.0012 * h,
            roughness: 0.82 - 0.12 * h,
            metallic: 0.0,
            occlusion: 0.7 + 0.3 * h,
        }
    })
}

fn feather_texture() -> TextureSet {
    TextureSet::bake(4, 0.05, |_| Texel::default())
}

// ======================================================================= deer

mod deer {
    use super::*;

    const R: QuadRig = DEER;

    const KIND: Kind = Kind {
        key: "deer",
        coat,
        body,
        scale: 1.0,
    };

    /// Head frame: the face points down this way in the rest pose.
    pub const FWD: Vec3 = v3(0.0, -0.64, -0.768);
    pub const UP: Vec3 = v3(0.0, 0.768, -0.64);

    /// Head-local coordinates: x right, y up the face, z along it toward the nose.
    /// The skull's frame sits a little above the joint the head turns on.
    const SKULL: Vec3 = v3(0.0, R.head.y + UP.y * 0.03, R.head.z + UP.z * 0.03);

    fn hl(p: Vec3) -> Vec3 {
        local(p, SKULL, Vec3::X, UP, FWD)
    }

    fn hp(q: Vec3) -> Vec3 {
        SKULL + Vec3::X * q.x + UP * q.y + FWD * q.z
    }

    const EYE: Vec3 = v3(0.05, 0.03, 0.152);
    const EYE_AXIS: Vec3 = v3(0.86, 0.22, 0.46);
    const GLAND: Vec3 = v3(0.043, 0.014, 0.192);
    const NOSE: Vec3 = v3(0.0, -0.008, 0.392);

    pub fn torso(p: Vec3) -> f32 {
        let m = mx(p);
        let barrel = ell(p, v3(0.0, 0.89, -0.13), v3(0.19, 0.245, 0.38));
        let chest = ell(p, v3(0.0, 0.84, -0.44), v3(0.145, 0.21, 0.215));
        let belly = ell(p, v3(0.0, 0.89, 0.12), v3(0.175, 0.2, 0.27));
        let loin = ell(p, v3(0.0, 0.99, 0.3), v3(0.15, 0.13, 0.22));
        let withers = cone(p, v3(0.0, 1.08, -0.47), v3(0.0, 1.06, -0.2), 0.075, 0.058);
        let back = cone(p, v3(0.0, 1.06, -0.2), v3(0.0, 1.07, 0.42), 0.052, 0.058);
        // Shoulder blades and the triceps behind the upper arm.
        let scap = ell(m, v3(0.112, 0.955, -0.37), v3(0.045, 0.14, 0.095));
        let triceps = ell(m, v3(0.112, 0.77, -0.355), v3(0.062, 0.12, 0.085));
        let point = ell(m, v3(0.108, 0.85, -0.5), v3(0.058, 0.075, 0.05));
        // Haunches: thigh, buttock and hamstring down the back of the leg.
        let haunch = ell(m, v3(0.1, 0.89, 0.35), v3(0.085, 0.2, 0.19));
        let butt = ell(m, v3(0.062, 0.94, 0.5), v3(0.075, 0.125, 0.09));
        let ham = cone(m, v3(0.075, 0.95, 0.52), v3(0.1, 0.71, 0.44), 0.065, 0.042);
        let hip = ell(m, v3(0.115, 1.06, 0.29), v3(0.045, 0.045, 0.055));
        // Base of the neck: a ball around the neck joint a little bigger than the neck's own,
        // so the neck turns in it without opening a seam.
        let socket = neck_ball(p, NECK_R + 0.004);
        // Balls the upper arm and thigh turn in.
        let shoulder = sphere(m, fj(0), 0.062);
        let hip_ball = sphere(m, hj(0), 0.072);
        let mut d = smin(barrel, chest, 0.1);
        d = smin(d, belly, 0.12);
        d = smin(d, loin, 0.1);
        d = smin(d, withers, 0.08);
        d = smin(d, back, 0.07);
        d = smin(d, scap, 0.06);
        d = smin(d, triceps, 0.06);
        d = smin(d, point, 0.05);
        d = smin(d, haunch, 0.08);
        d = smin(d, butt, 0.06);
        d = smin(d, ham, 0.05);
        d = smin(d, hip, 0.05);
        d = smin(d, shoulder, 0.05);
        d = smin(d, hip_ball, 0.05);
        smin(d, socket, 0.07)
    }

    /// Neck thickness at its base (depth; the sides are narrower by [`NECK_SQUASH`]) and at
    /// the mid-neck joint.
    const NECK_R: f32 = 0.16;
    const NECK_R2: f32 = 0.112;
    const NECK_SQUASH: f32 = 1.35;

    fn neck_ball(p: Vec3, r: f32) -> f32 {
        ell(p, R.neck, v3(r / NECK_SQUASH, r, r))
    }

    fn fj(i: usize) -> Vec3 {
        R.front.joints[i]
    }

    fn hj(i: usize) -> Vec3 {
        R.hind.joints[i]
    }

    /// Upper arm: mostly inside the chest; fills the elbow as the leg swings.
    pub fn arm(p: Vec3) -> f32 {
        cone(p, fj(0), fj(1), 0.06, 0.048)
    }

    pub fn forearm(p: Vec3) -> f32 {
        let fa = cone_x(p, fj(1), fj(2), 0.045, 0.025, 1.12);
        let olecranon = ell(p, v3(0.122, 0.672, -0.338), v3(0.027, 0.04, 0.03));
        let ext = ell(p, v3(0.117, 0.56, -0.398), v3(0.034, 0.1, 0.032));
        let carpus = ell(p, fj(2) + v3(0.0, 0.004, 0.0), v3(0.028, 0.034, 0.03));
        smin(smin(smin(fa, olecranon, 0.03), ext, 0.035), carpus, 0.022)
    }

    fn cannon(p: Vec3, top: Vec3, bottom: Vec3, ra: f32) -> f32 {
        let c = cone_x(p, top, bottom, ra, 0.02, 1.25);
        let tendon = cone_x(
            p,
            top + v3(0.0, -0.04, 0.012),
            bottom + v3(0.0, 0.035, 0.011),
            0.012,
            0.011,
            1.3,
        );
        let fetlock = ell(p, bottom + v3(0.0, 0.0, 0.004), v3(0.023, 0.028, 0.028));
        smin(smin(c, tendon, 0.015), fetlock, 0.02)
    }

    pub fn front_cannon(p: Vec3) -> f32 {
        cannon(p, fj(2), fj(3), 0.024)
    }

    pub fn hind_cannon(p: Vec3) -> f32 {
        let c = cannon(p, hj(2), hj(3), 0.026);
        // The metatarsal gland's tuft on the outside of the hind cannon.
        let tuft = ell(p, v3(0.128, 0.3, 0.43), v3(0.012, 0.03, 0.016));
        smin(c, tuft, 0.012)
    }

    fn coronet(j3: Vec3, toe: Vec3) -> Vec3 {
        j3 + (toe - j3) * 0.6 + v3(0.0, 0.006, 0.0)
    }

    fn pastern(p: Vec3, j3: Vec3, toe: Vec3) -> f32 {
        cone_x(p, j3, coronet(j3, toe), 0.02, 0.0205, 1.12)
    }

    pub fn front_pastern(p: Vec3) -> f32 {
        pastern(p, fj(3), fj(4))
    }

    pub fn hind_pastern(p: Vec3) -> f32 {
        pastern(p, hj(3), hj(4))
    }

    /// A cloven hoof: two pointed claws with a cleft between them, a sloped front wall, heel
    /// bulbs, flat soles and dewclaws behind the fetlock.
    fn hoof(p: Vec3, j3: Vec3, toe: Vec3) -> f32 {
        let f = (toe.z - j3.z).signum();
        let q = p - v3(toe.x, 0.0, 0.0);
        let x = q.x.abs();
        // Side profile in (back, up), measured from the toe tip.
        let b = (toe.z - p.z) * f;
        let s = Vec2::new(b, p.y);
        let profile = [
            Vec2::new(-0.004, 0.0),
            Vec2::new(0.058, 0.0),
            Vec2::new(0.062, 0.026),
            Vec2::new(0.031, 0.042),
            Vec2::new(-0.004, 0.005),
        ];
        let side = convex2(s, &profile);
        // Each claw narrows to its point; the outer wall is rounded.
        let t = smooth(-0.004, 0.045, b);
        let half = 0.0035 + 0.006 * t;
        let mid = 0.0035 + half;
        let across = (x - mid).abs() - half;
        let claw = smax(side, across, 0.006);
        let bulb = ell(
            Vec3::new(x, p.y, b),
            v3(0.011, 0.013, 0.051),
            v3(0.009, 0.013, 0.012),
        );
        let mut d = smin(claw, bulb, 0.008);
        // Flat sole on the ground.
        d = smax(d, -p.y, 0.0015);
        // Dewclaws behind the fetlock, pointing down and back.
        let bj = (toe.z - j3.z) * f;
        let dew = cone(
            Vec3::new(x, p.y, b),
            v3(0.012, j3.y - 0.022, bj + 0.016),
            v3(0.013, j3.y - 0.04, bj + 0.026),
            0.0065,
            0.0035,
        );
        d.min(dew)
    }

    pub fn front_hoof(p: Vec3) -> f32 {
        hoof(p, fj(3), fj(4))
    }

    pub fn hind_hoof(p: Vec3) -> f32 {
        hoof(p, hj(3), hj(4))
    }

    /// Thigh bone: inside the haunch; shows at the stifle as the leg swings.
    pub fn thigh(p: Vec3) -> f32 {
        let bone = cone(p, hj(0), hj(1), 0.07, 0.055);
        // The lower thigh muscles move with the femur (inside the haunch at rest).
        let muscle = ell(
            p,
            hj(0) + (hj(1) - hj(0)) * 0.6 + v3(-0.004, 0.0, 0.035),
            v3(0.056, 0.1, 0.085),
        );
        smin(bone, muscle, 0.04)
    }

    pub fn gaskin(p: Vec3) -> f32 {
        let g = cone_x(p, hj(1), hj(2), 0.052, 0.027, 1.15);
        let calf = ell(p, v3(0.122, 0.625, 0.36), v3(0.038, 0.095, 0.05));
        let achilles = cone_x(
            p,
            v3(0.118, 0.62, 0.405),
            v3(0.115, 0.47, 0.49),
            0.02,
            0.014,
            1.3,
        );
        let hock = ell(p, hj(2), v3(0.027, 0.034, 0.034));
        let point = ell(p, v3(0.115, 0.468, 0.487), v3(0.018, 0.022, 0.02));
        let mut d = smin(g, calf, 0.05);
        d = smin(d, achilles, 0.03);
        d = smin(d, hock, 0.02);
        smin(d, point, 0.015)
    }

    /// Both neck pieces are round (in the joint's plane) where they meet the chest, each other
    /// and the head, so nodding and grazing turn them without opening seams.
    fn neck1_with(p: Vec3, s: f32) -> f32 {
        let r0 = NECK_R.min(NECK_R * s);
        cone_x(p, R.neck, R.neck2, r0, NECK_R2 * s, NECK_SQUASH)
    }

    fn neck2_with(p: Vec3, s: f32) -> f32 {
        let n = cone_x(
            p,
            R.neck2,
            R.head,
            NECK_R2 * s + 0.003,
            HEAD_BALL - 0.004,
            1.3,
        );
        // Throat and the windpipe under it, ahead of the joint.
        let throat = ell(p, hp(v3(0.0, -0.08, 0.04)), v3(0.042, 0.055, 0.065));
        let pipe = cone_x(
            p,
            R.neck2 + v3(0.0, -0.07, -0.06),
            hp(v3(0.0, -0.08, 0.02)),
            0.035,
            0.035,
            1.2,
        );
        smin(smin(n, throat, 0.05), pipe, 0.06)
    }

    /// The ball at the top of the neck the head turns on (part of the head).
    const HEAD_BALL: f32 = 0.07;

    pub fn neck1(p: Vec3) -> f32 {
        neck1_with(p, 1.0)
    }

    pub fn neck2(p: Vec3) -> f32 {
        neck2_with(p, 1.0)
    }

    /// Shaggy hair hanging under a stag's neck.
    fn mane(p: Vec3, base: f32) -> f32 {
        if base > 0.09 {
            return base;
        }
        // The neck's underside, measured across its axis.
        let axis = (R.head - R.neck).normalize();
        let down = Vec3::X.cross(axis).normalize();
        let along = (p - R.neck).dot(axis) / R.neck.distance(R.head);
        let under = smooth(-0.02, 0.06, (p - R.neck).dot(-down) + 0.02)
            * smooth(0.05, 0.25, along)
            * smooth(1.05, 0.75, along);
        if under <= 0.0 {
            return base;
        }
        let q = Vec3::new(
            p.x * 30.0,
            (p - R.neck).dot(axis) * 6.0,
            (p - R.neck).dot(down) * 30.0,
        );
        let shag = fbm3(q, 3, 0.55, 5) * 0.5 + 0.5;
        base - under * (0.03 + 0.035 * shag)
    }

    pub fn neck1_stag(p: Vec3) -> f32 {
        mane(p, neck1_with(p, 1.12))
    }

    pub fn neck2_stag(p: Vec3) -> f32 {
        mane(p, neck2_with(p, 1.12))
    }

    pub fn head(p: Vec3) -> f32 {
        let q = hl(p);
        let m = Vec3::new(q.x.abs(), q.y, q.z);
        let poll = ell(p, R.head, v3(HEAD_BALL / 1.3, HEAD_BALL, HEAD_BALL));
        let cranium = smin(
            ell(q, v3(0.0, 0.0, 0.06), v3(0.067, 0.07, 0.09)),
            poll,
            0.04,
        );
        let face = cone_x(
            q,
            v3(0.0, 0.0, 0.11),
            v3(0.0, -0.022, 0.33),
            0.06,
            0.036,
            1.15,
        );
        let cheek = ell(m, v3(0.041, -0.036, 0.145), v3(0.031, 0.048, 0.07));
        let jaw = cone(
            m,
            v3(0.035, -0.075, 0.08),
            v3(0.018, -0.066, 0.31),
            0.031,
            0.018,
        );
        let ramus = ell(m, v3(0.035, -0.07, 0.065), v3(0.025, 0.055, 0.04));
        let muzzle = ell(q, v3(0.0, -0.032, 0.352), v3(0.035, 0.044, 0.048));
        let chin = ell(q, v3(0.0, -0.074, 0.32), v3(0.02, 0.02, 0.028));
        let brow = ell(m, v3(0.037, 0.038, 0.142), v3(0.02, 0.012, 0.034));
        let larynx = ell(q, v3(0.0, -0.095, 0.03), v3(0.035, 0.035, 0.05));
        let mut d = smin(cranium, face, 0.05);
        d = smin(d, cheek, 0.03);
        d = smin(d, jaw, 0.025);
        d = smin(d, ramus, 0.03);
        d = smin(d, muzzle, 0.03);
        d = smin(d, chin, 0.015);
        d = smin(d, brow, 0.02);
        d = smin(d, larynx, 0.04);
        // Eye socket with lids around the eyeball.
        let axis = EYE_AXIS.normalize();
        d = ssub(d, sphere(m, EYE + axis * 0.004, 0.0175), 0.004);
        let e = m - EYE;
        let along = e.dot(axis);
        let ring = Vec2::new((e - axis * along).length() - 0.0168, along - 0.002).length() - 0.0042;
        d = smin(d, ring, 0.004);
        // Preorbital gland, nostrils and the mouth line.
        d = ssub(d, ell(m, GLAND, v3(0.006, 0.008, 0.016)), 0.004);
        d = ssub(
            d,
            ell(m, v3(0.015, -0.012, 0.392), v3(0.0055, 0.0045, 0.012)),
            0.003,
        );
        let lip = cone(
            m,
            v3(0.028, -0.057, 0.29),
            v3(0.012, -0.06, 0.385),
            0.002,
            0.002,
        );
        ssub(d, lip, 0.003)
    }

    fn eye_center() -> [Vec3; 2] {
        let e = hp(EYE);
        [flip_x(e), e]
    }

    pub fn gloss_nose(p: Vec3) -> f32 {
        let q = hl(p);
        smax(
            head(p) - 0.0018,
            ell(q, NOSE, v3(0.03, 0.028, 0.024)),
            0.003,
        )
    }

    /// Ear frame (right ear): along the ear, across it, out of the cup.
    pub fn ear_frame() -> (Vec3, Vec3, Vec3) {
        let along = (Vec3::X * 0.72 + UP * 0.62 - FWD * 0.32).normalize();
        let cup0 = (Vec3::X * 0.35 + FWD * 0.9 + UP * 0.1).normalize();
        let cup = (cup0 - along * cup0.dot(along)).normalize();
        let across = along.cross(cup);
        (along, across, cup)
    }

    pub fn ear(p: Vec3) -> f32 {
        let (along, across, cup) = ear_frame();
        let q = local(p, R.ear, across, along, cup);
        let leaf = ell(q, v3(0.0, 0.09, 0.0), v3(0.044, 0.094, 0.015));
        let tip = ell(q, v3(0.0, 0.152, 0.0), v3(0.022, 0.042, 0.011));
        let hollow = ell(q, v3(0.0, 0.096, 0.012), v3(0.035, 0.08, 0.012));
        let roll = cone(q, v3(0.0, -0.012, -0.004), v3(0.0, 0.045, 0.0), 0.017, 0.02);
        smin(ssub(smin(leaf, tip, 0.02), hollow, 0.006), roll, 0.015)
    }

    pub fn tail(p: Vec3) -> f32 {
        cone_x(p, R.tail, R.tail2, 0.032, 0.022, 0.8)
    }

    /// Everything, for occlusion.
    pub fn body(p: Vec3) -> f32 {
        const PARTS: [Bound; 9] = [
            Bound {
                center: v3(0.0, 0.9, -0.02),
                radius: 0.7,
                f: torso,
            },
            Bound {
                center: v3(0.0, 1.03, -0.65),
                radius: 0.32,
                f: neck1,
            },
            Bound {
                center: v3(0.0, 1.25, -0.84),
                radius: 0.25,
                f: neck2,
            },
            Bound {
                center: v3(0.0, 1.22, -1.08),
                radius: 0.3,
                f: head,
            },
            Bound {
                center: v3(0.12, 0.52, -0.38),
                radius: 0.19,
                f: forearm,
            },
            Bound {
                center: v3(0.11, 0.24, -0.39),
                radius: 0.17,
                f: front_cannon,
            },
            Bound {
                center: v3(0.12, 0.58, 0.34),
                radius: 0.22,
                f: gaskin,
            },
            Bound {
                center: v3(0.11, 0.28, 0.43),
                radius: 0.2,
                f: hind_cannon,
            },
            Bound {
                center: v3(0.0, 1.0, 0.6),
                radius: 0.12,
                f: tail,
            },
        ];
        union_of(&PARTS, mx(p))
    }

    /// Red deer coat pattern (linear RGB) at a rest-pose surface point.
    fn coat(piece: Piece, p: Vec3, n: Vec3, stag: bool) -> Vec3 {
        let red = hex(0x7f573e);
        let dark = hex(0x573d2c);
        let grey = hex(0x6b5d50);
        let belly = hex(0xa89983);
        let cream = hex(0xd6c59f);
        let mut c = red;
        // Dorsal darkening and a pale, greyer underside (countershading).
        let up = n.y;
        c = mix(c, dark, smooth(0.3, 0.95, up) * 0.5);
        let under = smooth(0.84, 0.64, p.y) * smooth(0.15, -0.7, up);
        c = mix(c, belly, under * 0.8);
        match piece {
            Piece::Leg(k, _) => {
                // Greyer lower legs, darker toward the hooves; paler inner legs.
                let lower = smooth(0.66, 0.25, p.y);
                c = mix(c, grey, lower * 0.8);
                c = mix(c, hex(0x4a3b30), smooth(0.22, 0.06, p.y) * 0.65);
                let inner = smooth(0.2, 0.9, -p.x.signum() * n.x) * smooth(0.12, 0.3, p.y);
                c = mix(c, mix(belly, grey, 0.4), inner * 0.5);
                if k >= 2 {
                    // The rump patch reaches down the back of the thighs.
                    let patch =
                        smooth(0.1, 0.6, n.z) * smooth(0.66, 0.8, p.y) * smooth(0.42, 0.5, p.z);
                    c = mix(c, cream, patch * 0.7);
                }
            }
            Piece::Body => {
                // Buff rump patch around the tail, edged darker above.
                let r = ((p - v3(0.0, 0.96, 0.6)) * v3(1.35, 0.95, 1.0)).length();
                let patch = (1.0 - smooth(0.12, 0.18, r)) * smooth(-0.25, 0.25, n.z);
                let edge = (smooth(0.13, 0.18, r) - smooth(0.18, 0.26, r))
                    * smooth(0.0, 0.4, n.z)
                    * smooth(0.92, 1.04, p.y);
                c = mix(c, dark * 0.75, edge * 0.55);
                c = mix(c, cream, patch);
                // Faint dark line along the spine.
                c = mix(
                    c,
                    dark * 0.85,
                    smooth(0.04, 0.0, p.x.abs()) * smooth(0.8, 0.98, up) * 0.45,
                );
                // Greyer chest front.
                c = mix(c, mix(red, grey, 0.6), smooth(-0.45, -0.62, p.z) * 0.5);
            }
            Piece::Neck | Piece::Neck2 => {
                // Greyer, like the chest it grows from.
                c = mix(c, mix(red, grey, 0.6), 0.5);
                if stag {
                    let mane = smooth(0.1, -0.6, up);
                    c = mix(c, hex(0x48352a), 0.45 + 0.45 * mane);
                }
                // Pale throat patch under the jaw.
                let throat =
                    smooth(0.09, 0.0, p.distance(hp(v3(0.0, -0.09, 0.04)))) * smooth(0.2, -0.4, up);
                c = mix(c, belly, throat * 0.55);
            }
            Piece::Head | Piece::Jaw => {
                let q = hl(p);
                let face = hex(0x74563f);
                c = mix(face, dark, smooth(0.3, 0.9, up) * 0.3);
                // Darker muzzle, white chin and lips, pale eye ring, dark gland.
                c = mix(c, hex(0x4f3f33), smooth(0.27, 0.35, q.z) * 0.7);
                let chin = smooth(-0.05, -0.075, q.y) * smooth(0.25, 0.32, q.z);
                c = mix(c, hex(0xc4b8a6), chin * 0.85);
                let lips = smooth(0.008, 0.0, (q.y + 0.058).abs()) * smooth(0.3, 0.35, q.z);
                c = mix(c, hex(0xc4b8a6), lips * 0.6);
                let m = Vec3::new(q.x.abs(), q.y, q.z);
                let ring = smooth(0.034, 0.022, m.distance(EYE));
                c = mix(c, hex(0xa08a70), ring * 0.3);
                let gland = smooth(0.016, 0.006, m.distance(GLAND));
                c = mix(c, hex(0x2a1d16), gland);
                c = mix(
                    c,
                    belly,
                    smooth(-0.055, -0.1, q.y) * smooth(0.2, 0.05, q.z) * 0.7,
                );
            }
            Piece::Ear(_) => {
                let (along, across, cup) = ear_frame();
                let q = local(mx(p), R.ear, across, along, cup);
                let inside = smooth(0.2, 0.7, mx(n).dot(cup));
                c = mix(hex(0x7a5c45), hex(0xcfc4b5), inside * 0.85);
                // Dark rims and tips.
                let rim = smooth(0.72, 1.0, (q.x / 0.044).abs()) + smooth(0.15, 0.19, q.y);
                c = mix(c, hex(0x35281f), rim.min(1.0) * 0.8);
            }
            Piece::Tail => {
                // Buff above, paler where it lies against the rump.
                c = mix(hex(0xb3976f), cream, smooth(0.2, -0.5, n.z));
                c = mix(c, hex(0x8a6a4c), smooth(0.3, 0.9, up) * 0.45);
            }
            _ => {}
        }
        c * (1.0 + 0.08 * mottle(p, 4.0, 3))
    }

    pub fn jobs(jobs: &mut Vec<Job>) {
        let up = Vec3::Y;
        let back = Vec3::Z;
        let mut specs: Vec<Spec> = Vec::new();
        // Torso.
        let mut torso_flow = Flow::new(v3(0.0, 0.9, -0.6), v3(0.0, 0.9, 0.62), up, 0.16).tilt(0.45);
        torso_flow.start = 0.22;
        torso_flow.end = 1.0;
        specs.push(Spec {
            piece: Piece::Body,
            f: torso,
            region: Region::aabb(
                v3(-0.3, 0.55, -0.75),
                v3(0.3, 1.25, 0.72),
                v3(0.0095, 0.0095, 0.013),
            ),
            target: 7000,
            flow: torso_flow,
            pivot: R.body,
            mirror: None,
            variant: false,
        });
        // Necks (hind and stag).
        for stag in [false, true] {
            let (f1, f2): (SdfFn, SdfFn) = if stag {
                (neck1_stag, neck2_stag)
            } else {
                (neck1, neck2)
            };
            specs.push(Spec {
                piece: Piece::Neck,
                f: f1,
                region: Region::bone(R.neck, R.neck2, 0.3, 0.009, 0.013),
                target: if stag { 3000 } else { 2000 },
                flow: Flow::new(R.neck2, R.neck, -Vec3::Z + Vec3::Y, 0.16).tilt(0.25),
                pivot: R.neck,
                mirror: None,
                variant: stag,
            });
            specs.push(Spec {
                piece: Piece::Neck2,
                f: f2,
                region: Region::bone(R.neck2, R.head, 0.25, 0.008, 0.011),
                target: if stag { 2400 } else { 1600 },
                flow: Flow::new(R.head, R.neck2, -Vec3::Z + Vec3::Y, 0.16).tilt(0.25),
                pivot: R.neck2,
                mirror: None,
                variant: stag,
            });
        }
        // Head.
        let nose = hp(v3(0.0, 0.0, 0.4));
        let mut head_flow = Flow::new(nose, R.head, UP, 0.12).tilt(0.2);
        head_flow.start = 0.03;
        specs.push(Spec {
            piece: Piece::Head,
            f: head,
            region: Region::bone(
                R.head - FWD * 0.08 + UP * 0.02,
                nose + FWD * 0.03,
                0.15,
                0.004,
                0.005,
            ),
            target: 4200,
            flow: head_flow,
            pivot: R.head,
            mirror: None,
            variant: false,
        });
        // Ears (right sculpted, left mirrored) and tail.
        let (along, _, cup) = ear_frame();
        specs.push(Spec {
            piece: Piece::Ear(1),
            f: ear,
            region: Region::bone(
                R.ear - along * 0.03,
                R.ear + along * 0.2,
                0.07,
                0.0032,
                0.0045,
            ),
            target: 650,
            flow: Flow::new(R.ear, R.ear + along * 0.2, cup, 0.1),
            pivot: R.ear,
            mirror: Some(Piece::Ear(0)),
            variant: false,
        });
        specs.push(Spec {
            piece: Piece::Tail,
            f: tail,
            region: Region::bone(R.tail, R.tail2, 0.07, 0.005, 0.007),
            target: 450,
            flow: Flow::new(R.tail, R.tail2, back, 0.1),
            pivot: R.tail,
            mirror: None,
            variant: false,
        });
        // Legs: right side sculpted, left mirrored.
        type Legs = (
            u8,
            [fn(Vec3) -> f32; 4],
            fn(Vec3) -> f32,
            LegRest,
            [f32; 4],
            [usize; 4],
        );
        let legs: [Legs; 2] = [
            (
                1,
                [arm, forearm, front_cannon, front_pastern],
                front_hoof,
                R.front,
                [0.2, 0.13, 0.07, 0.05],
                [500, 900, 650, 320],
            ),
            (
                3,
                [thigh, gaskin, hind_cannon, hind_pastern],
                hind_hoof,
                R.hind,
                [0.3, 0.16, 0.08, 0.05],
                [550, 1100, 650, 320],
            ),
        ];
        for (k, fields, hoof_f, leg, reach, target) in legs {
            let j = leg.joints;
            let across = [0.008, 0.0055, 0.004, 0.003];
            let along = [0.012, 0.01, 0.01, 0.0045];
            for s in 0..4 {
                let (a, b) = if s == 3 {
                    (j[3], coronet(j[3], j[4]))
                } else {
                    (j[s], j[s + 1])
                };
                specs.push(Spec {
                    piece: Piece::Leg(k, s as u8),
                    f: fields[s],
                    region: Region::bone(a, b, reach[s], across[s], along[s]),
                    target: target[s],
                    flow: Flow::new(a, b, back, if s >= 2 { 0.09 } else { 0.12 }),
                    pivot: j[s],
                    mirror: Some(Piece::Leg(k - 1, s as u8)),
                    variant: false,
                });
            }
            // Hoof.
            jobs.push(Box::new(move || {
                let color = |p: Vec3, n: Vec3| {
                    let sole = smooth(0.006, 0.0, p.y);
                    let dew = smooth(0.06, 0.08, p.y);
                    mix(hex(0x2e2723), hex(0x5a4c40), sole * 0.6 + dew * 0.2) * (1.0 + 0.05 * n.y)
                };
                let paint = Paint {
                    flow: Flow::new(j[3], j[4], back, 0.05),
                    color: &color,
                    body: &body,
                    scale: 0.4,
                    pivot: j[3],
                };
                let c = v3(j[4].x, 0.04, (j[3].z + j[4].z) * 0.5);
                let region = Region::aabb(
                    c - v3(0.045, 0.05, 0.07),
                    c + v3(0.045, 0.07, 0.07),
                    Vec3::splat(0.0018),
                );
                let mesh = sculpt(&hoof_f, region, 520, &paint);
                vec![
                    Made::Mesh(
                        format!("animals/deer/{}", Piece::Hoof(k - 1).key()),
                        mirrored(&mesh),
                    ),
                    Made::Mesh(format!("animals/deer/{}", Piece::Hoof(k).key()), mesh),
                ]
            }));
        }
        specs.sort_by_key(|s| std::cmp::Reverse(s.target));
        for spec in specs {
            jobs.push(Box::new(move || spec.run(KIND)));
        }
        // Eyes and nose.
        jobs.push(Box::new(|| {
            let color = |_p: Vec3, _n: Vec3| Vec3::ONE;
            let paint = Paint {
                flow: Flow::new(R.head, R.head + FWD, UP, 0.1),
                color: &color,
                body: &body,
                scale: 0.3,
                pivot: R.head,
            };
            let nose = hp(NOSE);
            let mut mesh = sculpt(
                &gloss_nose,
                Region::aabb(
                    nose - Vec3::splat(0.05),
                    nose + Vec3::splat(0.05),
                    Vec3::splat(0.0018),
                ),
                500,
                &paint,
            );
            for v in &mut mesh.vertices {
                // The vertex color holds the occlusion; tint it near black.
                let ao = v.color[0].clamp(0.4, 1.0);
                v.color = (hex(0x121110) * ao).extend(1.0).to_array();
            }
            let axis = EYE_AXIS.normalize();
            for (i, c) in eye_center().into_iter().enumerate() {
                let ax =
                    Vec3::X * axis.x * if i == 0 { -1.0 } else { 1.0 } + UP * axis.y + FWD * axis.z;
                let rot = Quat::from_rotation_arc(Vec3::Y, ax);
                let g = ball(c, Vec3::splat(0.0168), rot, 10, 14);
                let eye = g.finish(R.head, &body, 0.2, &|p, _, _| {
                    let t = (p - c).normalize().dot(ax);
                    mix(hex(0x2a1a10), hex(0x080605), smooth(0.6, 0.9, t))
                });
                mesh.merge(&eye);
            }
            vec![Made::Mesh("animals/deer/gloss".into(), mesh)]
        }));
        // Antlers.
        jobs.push(Box::new(|| {
            vec![Made::Mesh("animals/deer/antlers".into(), antlers())]
        }));
    }

    /// Both antlers of a stag: a beam with brow, bez and trez tines and a crown.
    fn antlers() -> Mesh {
        let burr = hp(v3(0.034, 0.055, 0.045));
        let o = |x: f32, y: f32, z: f32| burr + v3(x, y, z);
        let beam = [
            o(0.0, -0.01, 0.0),
            o(0.03, 0.10, 0.025),
            o(0.09, 0.26, 0.06),
            o(0.15, 0.42, 0.07),
            o(0.19, 0.56, 0.04),
            o(0.19, 0.66, 0.0),
        ];
        let beam_r = [0.024, 0.022, 0.019, 0.017, 0.015, 0.013];
        let tines: [(&[Vec3], &[f32]); 6] = [
            (
                &[
                    o(0.012, 0.045, 0.0),
                    o(0.03, 0.08, -0.12),
                    o(0.04, 0.15, -0.22),
                ],
                &[0.015, 0.011, 0.005],
            ),
            (
                &[
                    o(0.03, 0.10, 0.02),
                    o(0.06, 0.135, -0.1),
                    o(0.07, 0.19, -0.16),
                ],
                &[0.013, 0.009, 0.0045],
            ),
            (
                &[
                    o(0.14, 0.38, 0.07),
                    o(0.17, 0.42, -0.05),
                    o(0.18, 0.49, -0.11),
                ],
                &[0.012, 0.009, 0.0045],
            ),
            (
                &[
                    o(0.19, 0.64, 0.0),
                    o(0.18, 0.72, -0.03),
                    o(0.17, 0.79, -0.04),
                ],
                &[0.012, 0.009, 0.0045],
            ),
            (
                &[
                    o(0.19, 0.62, 0.02),
                    o(0.25, 0.68, 0.06),
                    o(0.29, 0.74, 0.09),
                ],
                &[0.011, 0.008, 0.0042],
            ),
            (
                &[o(0.19, 0.6, 0.03), o(0.15, 0.68, 0.1), o(0.12, 0.74, 0.14)],
                &[0.011, 0.008, 0.0042],
            ),
        ];
        let mut g = Geo::default();
        let (p, r) = spline(&beam, &beam_r, 5);
        let bumpy: Vec<f32> = r
            .iter()
            .enumerate()
            .map(|(i, r)| r * (1.0 + 0.05 * ((i as f32) * 2.7).sin()))
            .collect();
        g.append(tube(&p, &bumpy, 12));
        for (pts, rad) in tines {
            let (p, r) = spline(pts, rad, 5);
            g.append(tube(&p, &r, 9));
        }
        // The burr: a knobbly ring at the base.
        let axis = (beam[1] - beam[0]).normalize();
        let side = axis.any_orthonormal_vector();
        let mut ring = Vec::new();
        for i in 0..=16 {
            let a = TAU * i as f32 / 16.0;
            ring.push(burr + axis * 0.004 + Quat::from_axis_angle(axis, a) * side * 0.024);
        }
        let rr: Vec<f32> = (0..ring.len())
            .map(|i| 0.008 + 0.002 * ((i * 7 % 5) as f32 / 5.0))
            .collect();
        g.append(tube(&ring, &rr, 7));
        let color = |p: Vec3, _n: Vec3, _uv: Vec2| {
            let h = (p.y - burr.y) / 0.7;
            let tip_d = tines
                .iter()
                .map(|(pts, _)| p.distance(*pts.last().unwrap_or(&p)))
                .fold(p.distance(beam[5]), f32::min);
            let base = mix(hex(0x6e5840), hex(0xa8916f), smooth(0.0, 0.6, h));
            mix(base, hex(0xe9dfc8), smooth(0.07, 0.015, tip_d))
        };
        let right = g.finish(R.head, &body, 0.5, &color);
        let mut mesh = mirrored(&right);
        mesh.merge(&right);
        mesh
    }
}

// ======================================================================= wolf

/// Fur clumps: a lumpy 0..1 pattern stretched along `along` (the way the hair lies), about
/// `size` meters across.
fn clumps(p: Vec3, along: Vec3, size: f32, seed: u32) -> f32 {
    let a = along.normalize();
    let b = a.any_orthonormal_vector();
    let c = a.cross(b);
    let q = Vec3::new(p.dot(b), p.dot(c), p.dot(a) * 0.3) / size;
    (fbm3(q, 3, 0.5, seed) * 0.5 + 0.5).clamp(0.0, 1.0)
}

mod wolf {
    use super::*;

    const R: QuadRig = WOLF;

    const KIND: Kind = Kind {
        key: "wolf",
        coat,
        body,
        scale: 0.75,
    };

    /// The skull's frame: x right, y up, z forward (the head looks along −Z at rest).
    const SKULL: Vec3 = v3(0.0, R.head.y + 0.03, R.head.z);

    fn hl(p: Vec3) -> Vec3 {
        let d = p - SKULL;
        Vec3::new(d.x, d.y, -d.z)
    }

    fn hp(q: Vec3) -> Vec3 {
        SKULL + Vec3::new(q.x, q.y, -q.z)
    }

    const EYE: Vec3 = v3(0.037, 0.024, 0.128);
    const EYE_AXIS: Vec3 = v3(0.62, 0.12, 0.78);
    const EYE_R: f32 = 0.0115;
    const NOSE: Vec3 = v3(0.0, -0.004, 0.262);
    /// The tip of the tail at rest.
    const TIP: Vec3 = v3(0.0, 0.30, 0.6);
    const NECK_R: f32 = 0.112;
    const HEAD_BALL: f32 = 0.066;

    fn fj(i: usize) -> Vec3 {
        R.front.joints[i]
    }

    fn hj(i: usize) -> Vec3 {
        R.hind.joints[i]
    }

    pub fn torso(p: Vec3) -> f32 {
        let m = mx(p);
        let chest = ell(p, v3(0.0, 0.6, -0.27), v3(0.115, 0.19, 0.2));
        let ribs = ell(p, v3(0.0, 0.62, -0.08), v3(0.125, 0.165, 0.28));
        let belly = ell(p, v3(0.0, 0.62, 0.12), v3(0.095, 0.115, 0.2));
        let loin = ell(p, v3(0.0, 0.67, 0.22), v3(0.092, 0.085, 0.17));
        let withers = cone(p, v3(0.0, 0.755, -0.33), v3(0.0, 0.735, -0.1), 0.058, 0.05);
        let back = cone(p, v3(0.0, 0.735, -0.1), v3(0.0, 0.715, 0.32), 0.046, 0.05);
        let scap = ell(m, v3(0.08, 0.67, -0.3), v3(0.04, 0.11, 0.08));
        let triceps = ell(m, v3(0.078, 0.5, -0.275), v3(0.045, 0.08, 0.07));
        let point = ell(m, v3(0.075, 0.57, -0.4), v3(0.045, 0.06, 0.045));
        let haunch = ell(m, v3(0.072, 0.63, 0.27), v3(0.058, 0.13, 0.12));
        let butt = ell(m, v3(0.045, 0.655, 0.36), v3(0.052, 0.085, 0.07));
        let socket = ell(
            p,
            R.neck,
            v3((NECK_R + 0.004) / 1.2, NECK_R + 0.004, NECK_R + 0.004),
        );
        // Balls the upper arm and thigh turn in.
        let shoulder = sphere(m, fj(0), 0.048);
        let hip = sphere(m, hj(0), 0.053);
        let mut d = smin(chest, ribs, 0.1);
        d = smin(d, belly, 0.1);
        d = smin(d, loin, 0.08);
        d = smin(d, withers, 0.07);
        d = smin(d, back, 0.06);
        d = smin(d, scap, 0.05);
        d = smin(d, triceps, 0.05);
        d = smin(d, point, 0.045);
        d = smin(d, haunch, 0.07);
        d = smin(d, butt, 0.05);
        d = smin(d, socket, 0.06);
        d = smin(d, shoulder, 0.04);
        d = smin(d, hip, 0.04);
        if d > 0.05 {
            return d;
        }
        // A cape of longer fur over the shoulders, and breeches behind the thighs.
        let cape = smooth(-0.05, -0.3, p.z) * smooth(0.6, 0.72, p.y);
        let breeches =
            smooth(0.25, 0.38, p.z) * smooth(0.72, 0.55, p.y) * smooth(0.02, 0.06, p.x.abs());
        let fringe = smooth(0.5, 0.44, p.y) * smooth(0.1, -0.2, p.z);
        let amount = cape * 0.022 + breeches * 0.016 + fringe * 0.01;
        if amount <= 0.0 {
            return d;
        }
        let along = v3(0.0, -0.35, 1.0);
        d - amount * (0.35 + 0.65 * clumps(p, along, 0.022, 3))
    }

    pub fn neck(p: Vec3) -> f32 {
        let n = cone_x(p, R.neck, R.head, NECK_R, HEAD_BALL + 0.006, 1.2);
        if n > 0.06 {
            return n;
        }
        // The ruff: long fur all round, longest over the nape.
        let axis = (R.head - R.neck).normalize();
        let nape = Vec3::X.cross(axis).normalize();
        let t = ((p - R.neck).dot(axis) / R.neck.distance(R.head)).clamp(0.0, 1.0);
        let top = smooth(0.0, 0.8, (p - R.neck).dot(nape) / NECK_R);
        let amount = (0.02 + 0.016 * top) * smooth(0.05, 0.3, t);
        n - amount * (0.55 + 0.45 * clumps(p, -axis + nape * 0.4, 0.026, 7))
    }

    pub fn head(p: Vec3) -> f32 {
        let q = hl(p);
        let m = Vec3::new(q.x.abs(), q.y, q.z);
        let poll = ell(p, R.head, v3(HEAD_BALL / 1.2, HEAD_BALL, HEAD_BALL));
        let cranium = ell(q, v3(0.0, 0.0, 0.07), v3(0.058, 0.054, 0.08));
        let cheek = ell(m, v3(0.048, -0.014, 0.1), v3(0.03, 0.03, 0.045));
        let masseter = ell(m, v3(0.043, -0.038, 0.06), v3(0.026, 0.032, 0.04));
        let muzzle = cone_x(
            q,
            v3(0.0, -0.012, 0.13),
            v3(0.0, -0.013, 0.25),
            0.04,
            0.023,
            1.04,
        );
        let bridge = cone(q, v3(0.0, 0.006, 0.14), v3(0.0, 0.001, 0.24), 0.022, 0.016);
        let nose = ell(q, NOSE, v3(0.017, 0.014, 0.012));
        let flews = ell(m, v3(0.021, -0.036, 0.185), v3(0.016, 0.018, 0.062));
        let brow = ell(m, v3(0.032, 0.03, 0.122), v3(0.018, 0.012, 0.024));
        let mut d = smin(poll, cranium, 0.04);
        d = smin(d, cheek, 0.03);
        d = smin(d, masseter, 0.03);
        d = smin(d, muzzle, 0.035);
        d = smin(d, bridge, 0.02);
        d = smin(d, nose, 0.012);
        d = smin(d, flews, 0.015);
        d = smin(d, brow, 0.015);
        // Eye sockets and lids.
        let axis = EYE_AXIS.normalize();
        d = ssub(d, sphere(m, EYE + axis * 0.003, EYE_R + 0.0008), 0.003);
        let e = m - EYE;
        let along = e.dot(axis);
        let radial = (e - axis * along) * v3(1.0, 1.45, 1.0);
        let ring = Vec2::new(radial.length() - EYE_R, along - 0.001).length() - 0.003;
        d = smin(d, ring, 0.003);
        // Nostrils.
        d = ssub(
            d,
            ell(m, v3(0.008, -0.006, 0.272), v3(0.0045, 0.004, 0.008)),
            0.002,
        );
        if d > 0.05 {
            return d;
        }
        // Cheek ruff flaring back under the ears.
        let ruff = smooth(0.12, 0.03, q.z) * smooth(0.02, 0.05, m.x) * smooth(0.03, -0.02, q.y);
        if ruff <= 0.0 {
            return d;
        }
        d - ruff * 0.02 * (0.3 + 0.7 * clumps(m, v3(0.4, -0.3, -1.0), 0.014, 11))
    }

    pub fn jaw(p: Vec3) -> f32 {
        let q = hl(p);
        let m = Vec3::new(q.x.abs(), q.y, q.z);
        let jaw = cone_x(
            q,
            v3(0.0, -0.062, 0.04),
            v3(0.0, -0.052, 0.222),
            0.024,
            0.013,
            0.85,
        );
        let lip = ell(m, v3(0.011, -0.049, 0.19), v3(0.012, 0.009, 0.038));
        let canine = cone(
            m,
            v3(0.015, -0.04, 0.218),
            v3(0.015, -0.02, 0.222),
            0.0042,
            0.0012,
        );
        let d = smin(jaw, lip, 0.015).min(canine);
        // Fur under the chin and throat.
        let fur = smooth(0.18, 0.08, q.z) * smooth(-0.06, -0.08, q.y);
        d - fur * 0.008 * clumps(m, v3(0.0, -0.4, -1.0), 0.01, 13)
    }

    pub fn ear_frame() -> (Vec3, Vec3, Vec3) {
        // In the rest pose: up, a little out to the side and back; the cup faces forward.
        let along = v3(0.32, 1.0, 0.08).normalize();
        let cup0 = v3(0.3, 0.0, -1.0).normalize();
        let cup = (cup0 - along * cup0.dot(along)).normalize();
        let across = along.cross(cup);
        (along, across, cup)
    }

    pub fn ear(p: Vec3) -> f32 {
        let (along, across, cup) = ear_frame();
        let q = local(p, R.ear, across, along, cup);
        let s = Vec2::new(q.x, q.y);
        let tri = convex2(s, &[v2(-0.037, -0.01), v2(0.037, -0.01), v2(0.003, 0.088)]);
        let th = 0.013 - 0.007 * (q.y / 0.088).clamp(0.0, 1.0);
        let plate = smax(tri, q.z.abs() - th, 0.008) - 0.002;
        let hollow = smax(
            convex2(s, &[v2(-0.026, 0.004), v2(0.026, 0.004), v2(0.002, 0.071)]),
            (q.z - th - 0.004).abs() - th * 0.8,
            0.005,
        );
        let base = ell(q, v3(0.0, 0.0, -0.004), v3(0.03, 0.022, 0.018));
        smin(ssub(plate, hollow, 0.004), base, 0.012)
    }

    pub fn tail(p: Vec3) -> f32 {
        let c = cone(p, R.tail, R.tail2, 0.032, 0.056);
        bushy(p, c, R.tail, R.tail2, 17)
    }

    pub fn tail2(p: Vec3) -> f32 {
        let c = cone(p, R.tail2, TIP, 0.059, 0.026);
        bushy(p, c, R.tail2, TIP, 19)
    }

    fn bushy(p: Vec3, base: f32, a: Vec3, b: Vec3, seed: u32) -> f32 {
        if base > 0.05 {
            return base;
        }
        base - 0.016 * (0.25 + 0.75 * clumps(p, b - a, 0.018, seed))
    }

    pub fn arm(p: Vec3) -> f32 {
        cone(p, fj(0), fj(1), 0.045, 0.039)
    }

    pub fn forearm(p: Vec3) -> f32 {
        let fa = cone_x(p, fj(1), fj(2), 0.037, 0.023, 1.12);
        let olecranon = ell(p, fj(1) + v3(0.0, 0.0, 0.03), v3(0.022, 0.032, 0.024));
        let flexor = ell(p, v3(0.09, 0.34, -0.29), v3(0.024, 0.07, 0.028));
        let extensor = ell(p, v3(0.09, 0.35, -0.325), v3(0.024, 0.07, 0.022));
        let carpus = ell(p, fj(2), v3(0.02, 0.024, 0.021));
        let pad = ell(p, fj(2) + v3(0.0, -0.012, 0.021), v3(0.008, 0.01, 0.008));
        let mut d = smin(fa, olecranon, 0.03);
        d = smin(d, flexor, 0.03);
        d = smin(d, extensor, 0.03);
        d = smin(d, carpus, 0.02);
        d = smin(d, pad, 0.008);
        // Feathering down the back of the forearm.
        let feather = smooth(0.4, 0.25, p.y) * smooth(0.18, 0.3, p.y) * smooth(-0.31, -0.29, p.z);
        d - feather * 0.008 * clumps(p, v3(0.0, -1.0, 0.3), 0.01, 23)
    }

    pub fn front_cannon(p: Vec3) -> f32 {
        cone_x(p, fj(2), fj(3), 0.021, 0.02, 1.08)
    }

    pub fn thigh(p: Vec3) -> f32 {
        cone(p, hj(0), hj(1), 0.05, 0.043)
    }

    pub fn gaskin(p: Vec3) -> f32 {
        let g = cone_x(p, hj(1), hj(2), 0.041, 0.021, 1.12);
        let calf = ell(p, v3(0.098, 0.37, 0.27), v3(0.03, 0.075, 0.042));
        let achilles = cone(
            p,
            v3(0.097, 0.34, 0.3),
            hj(2) + v3(0.0, 0.012, 0.02),
            0.012,
            0.009,
        );
        let hock = ell(p, hj(2) + v3(0.0, 0.004, 0.016), v3(0.016, 0.02, 0.016));
        let mut d = smin(g, calf, 0.04);
        d = smin(d, achilles, 0.025);
        smin(d, hock, 0.015)
    }

    pub fn hind_cannon(p: Vec3) -> f32 {
        cone_x(p, hj(2), hj(3), 0.021, 0.02, 1.08)
    }

    /// A paw: knuckles, four toes with dark claws and the pads under them.
    fn paw(p: Vec3, j3: Vec3, toe: Vec3, s: f32) -> f32 {
        let q = p - v3(toe.x, 0.0, toe.z);
        let m = Vec3::new(q.x.abs(), q.y, q.z) / s;
        let back = (j3.z - toe.z) / s;
        let knuckles = ell(m, v3(0.0, 0.026, back * 0.55), v3(0.028, 0.022, 0.034));
        let mut d = knuckles;
        for (x, z) in [(0.011, 0.012), (0.03, 0.026)] {
            let toe = ell(m, v3(x, 0.013, z), v3(0.0115, 0.013, 0.016));
            d = smin(d, toe, 0.008);
            let claw = cone(
                m,
                v3(x, 0.013, z - 0.012),
                v3(x, 0.002, z - 0.022),
                0.0038,
                0.0012,
            );
            d = d.min(claw);
        }
        let pad = ell(m, v3(0.0, 0.011, back * 0.62), v3(0.019, 0.012, 0.016));
        d = smin(d, pad, 0.01);
        let link = cone(
            m,
            v3(0.0, 0.024, back * 0.55),
            (j3 - v3(toe.x, 0.0, toe.z)) / s,
            0.02,
            0.019,
        );
        d = smin(d, link, 0.015);
        smax(d * s, -p.y, 0.002)
    }

    pub fn front_paw(p: Vec3) -> f32 {
        paw(p, fj(3), fj(4), 1.0)
    }

    pub fn hind_paw(p: Vec3) -> f32 {
        paw(p, hj(3), hj(4), 0.92)
    }

    pub fn gloss_nose(p: Vec3) -> f32 {
        let q = hl(p);
        smax(
            head(p) - 0.0015,
            ell(q, NOSE + v3(0.0, 0.001, 0.006), v3(0.019, 0.015, 0.012)),
            0.002,
        )
    }

    /// Everything, for occlusion.
    pub fn body(p: Vec3) -> f32 {
        const PARTS: [Bound; 10] = [
            Bound {
                center: v3(0.0, 0.62, -0.02),
                radius: 0.55,
                f: torso,
            },
            Bound {
                center: v3(0.0, 0.73, -0.45),
                radius: 0.22,
                f: neck,
            },
            Bound {
                center: v3(0.0, 0.8, -0.7),
                radius: 0.22,
                f: head,
            },
            Bound {
                center: v3(0.0, 0.76, -0.75),
                radius: 0.16,
                f: jaw,
            },
            Bound {
                center: v3(0.09, 0.3, -0.31),
                radius: 0.2,
                f: forearm,
            },
            Bound {
                center: v3(0.085, 0.09, -0.33),
                radius: 0.09,
                f: front_cannon,
            },
            Bound {
                center: v3(0.1, 0.34, 0.27),
                radius: 0.2,
                f: gaskin,
            },
            Bound {
                center: v3(0.09, 0.13, 0.33),
                radius: 0.12,
                f: hind_cannon,
            },
            Bound {
                center: v3(0.0, 0.59, 0.47),
                radius: 0.16,
                f: tail,
            },
            Bound {
                center: v3(0.0, 0.42, 0.57),
                radius: 0.2,
                f: tail2,
            },
        ];
        union_of(&PARTS, mx(p))
    }

    /// Grey wolf coat (linear RGB): a dark saddle over grey sides, tawny legs and a cream
    /// underside, throat and face mask.
    fn coat(piece: Piece, p: Vec3, n: Vec3, _: bool) -> Vec3 {
        let saddle = hex(0x3b352f);
        let grey = hex(0x8a8073);
        let buff = hex(0xa88e6c);
        let cream = hex(0xdcd3c3);
        let black = hex(0x1d1a17);
        let up = n.y;
        let mut c = grey;
        match piece {
            Piece::Body | Piece::Neck => {
                c = mix(
                    c,
                    saddle,
                    smooth(-0.2, 0.7, up) * smooth(0.56, 0.7, p.y) * 0.9,
                );
                c = mix(c, buff, smooth(0.6, 0.48, p.y) * 0.45);
                let under = smooth(0.0, -0.6, up) * smooth(0.6, 0.5, p.y);
                c = mix(c, cream, under);
                if piece == Piece::Neck {
                    // A pale bib under the throat, grey above.
                    let axis = (R.head - R.neck).normalize();
                    let down = axis.cross(Vec3::X).normalize();
                    let bib = smooth(0.1, 0.7, n.dot(down));
                    c = mix(c, cream, bib * 0.85);
                } else {
                    // Pale chest front.
                    c = mix(
                        c,
                        cream,
                        smooth(-0.35, -0.45, p.z) * smooth(0.65, 0.5, p.y) * 0.8,
                    );
                }
            }
            Piece::Leg(k, s) => {
                c = mix(buff, grey, smooth(0.3, 0.52, p.y));
                let inner = smooth(0.1, 0.8, -p.x.signum() * n.x);
                c = mix(c, cream, inner * 0.8);
                // Dark line down the front of the forelegs; pale feet.
                if k < 2 && s >= 1 {
                    let front =
                        smooth(-0.3, -0.85, n.z) * smooth(0.12, 0.2, p.y) * smooth(0.45, 0.35, p.y);
                    c = mix(c, saddle, front * 0.55);
                }
                c = mix(c, mix(buff, cream, 0.5), smooth(0.08, 0.03, p.y) * 0.6);
                if s == 3 {
                    // Dark claws and pads.
                    let claw = smooth(0.006, 0.0, p.y) + smooth(0.6, 0.9, -n.y);
                    c = mix(c, black, claw.min(1.0) * 0.85);
                }
            }
            Piece::Head => {
                let q = hl(p);
                let m = Vec3::new(q.x.abs(), q.y, q.z);
                c = mix(
                    hex(0x8c8070),
                    saddle,
                    smooth(0.3, 0.9, up) * smooth(0.08, -0.02, q.z) * 0.6,
                );
                // Tawny muzzle top, cream cheeks, lips and brows, a dark lip line.
                c = mix(c, buff, smooth(0.13, 0.18, q.z) * smooth(-0.02, 0.01, q.y));
                let cheeks =
                    smooth(0.0, -0.03, q.y) * smooth(0.02, 0.05, m.x).max(smooth(0.14, 0.18, q.z));
                c = mix(c, cream, cheeks * 0.85);
                let brow = smooth(0.02, 0.008, m.distance(v3(0.03, 0.038, 0.115)));
                c = mix(c, cream, brow * 0.8);
                let lip = smooth(0.004, 0.0, (q.y + 0.05).abs()) * smooth(0.12, 0.16, q.z);
                c = mix(c, black, lip * 0.7);
                let liner = smooth(0.006, 0.002, (m.distance(EYE) - EYE_R - 0.002).abs());
                c = mix(c, black, liner * 0.8);
                // Palate under the muzzle (seen when the mouth opens).
                let palate =
                    smooth(-0.045, -0.052, q.y) * smooth(-0.6, -0.9, up) * smooth(0.08, 0.12, q.z);
                c = mix(c, hex(0x5c3434), palate);
            }
            Piece::Jaw => {
                let q = hl(p);
                c = mix(cream, buff, smooth(0.0, 0.5, n.x.abs()) * 0.3);
                let lip = smooth(0.005, 0.0, (q.y + 0.04).abs()) * smooth(0.1, 0.14, q.z);
                c = mix(c, black, lip * 0.9);
                let mouth = smooth(0.3, 0.8, up) * smooth(-0.045, -0.035, q.y);
                c = mix(c, hex(0x8a4848), mouth);
                let tooth = smooth(-0.033, -0.026, q.y) * smooth(0.21, 0.214, q.z);
                c = mix(c, hex(0xeae4d6), tooth);
            }
            Piece::Ear(_) => {
                let (along, across, cup) = ear_frame();
                let q = local(mx(p), R.ear, across, along, cup);
                let inside = smooth(0.2, 0.7, mx(n).dot(cup));
                c = mix(hex(0x9a7b57), cream, inside * 0.9);
                let rim = smooth(
                    0.75,
                    1.0,
                    q.x.abs() / (0.035 * (1.0 - q.y / 0.1)).max(0.004),
                ) * (1.0 - inside);
                c = mix(c, black, (rim + smooth(0.07, 0.1, q.y)).min(1.0) * 0.8);
            }
            Piece::Tail | Piece::Tail2 => {
                c = mix(c, saddle, smooth(0.0, 0.8, n.z * 0.6 + up * 0.8) * 0.75);
                c = mix(c, buff, smooth(0.0, -0.7, n.z) * 0.35);
                if piece == Piece::Tail {
                    // The dark violet gland spot on top near the base.
                    let gland = smooth(0.04, 0.015, p.distance(v3(0.0, 0.64, 0.46)))
                        * smooth(0.0, 0.5, up + n.z);
                    c = mix(c, black, gland * 0.7);
                } else {
                    c = mix(c, black, smooth(0.42, 0.34, p.y) * 0.9);
                }
            }
            _ => {}
        }
        c * (1.0 + 0.1 * mottle(p, 6.0, 5))
    }

    pub fn jobs(jobs: &mut Vec<Job>) {
        let up = Vec3::Y;
        let back = Vec3::Z;
        let mut specs: Vec<Spec> = Vec::new();
        let mut torso_flow =
            Flow::new(v3(0.0, 0.62, -0.47), v3(0.0, 0.62, 0.45), up, 0.2).tilt(0.4);
        torso_flow.start = 0.12;
        specs.push(Spec {
            piece: Piece::Body,
            f: torso,
            region: Region::aabb(
                v3(-0.22, 0.36, -0.56),
                v3(0.22, 0.9, 0.5),
                v3(0.0072, 0.0072, 0.0095),
            ),
            target: 6500,
            flow: torso_flow,
            pivot: R.body,
            mirror: None,
            variant: false,
        });
        specs.push(Spec {
            piece: Piece::Neck,
            f: neck,
            region: Region::bone(R.neck, R.head, 0.19, 0.0062, 0.0075),
            target: 2600,
            flow: Flow::new(R.head, R.neck, up - back, 0.18).tilt(0.2),
            pivot: R.neck,
            mirror: None,
            variant: false,
        });
        let nose = hp(v3(0.0, 0.0, 0.28));
        let mut head_flow = Flow::new(nose, hp(v3(0.0, 0.0, -0.06)), up, 0.12).tilt(0.2);
        head_flow.start = 0.02;
        specs.push(Spec {
            piece: Piece::Head,
            f: head,
            region: Region::bone(hp(v3(0.0, -0.01, -0.08)), nose, 0.13, 0.0034, 0.0042),
            target: 4600,
            flow: head_flow,
            pivot: R.head,
            mirror: None,
            variant: false,
        });
        specs.push(Spec {
            piece: Piece::Jaw,
            f: jaw,
            region: Region::bone(
                hp(v3(0.0, -0.055, 0.03)),
                hp(v3(0.0, -0.045, 0.25)),
                0.05,
                0.003,
                0.004,
            ),
            target: 1000,
            flow: Flow::new(hp(v3(0.0, -0.05, 0.25)), hp(v3(0.0, -0.06, 0.0)), -up, 0.1),
            pivot: R.jaw,
            mirror: None,
            variant: false,
        });
        let (along, _, cup) = ear_frame();
        specs.push(Spec {
            piece: Piece::Ear(1),
            f: ear,
            region: Region::bone(
                R.ear - along * 0.02,
                R.ear + along * 0.11,
                0.05,
                0.0028,
                0.0036,
            ),
            target: 520,
            flow: Flow::new(R.ear, R.ear + along * 0.1, cup, 0.08),
            pivot: R.ear,
            mirror: Some(Piece::Ear(0)),
            variant: false,
        });
        specs.push(Spec {
            piece: Piece::Tail,
            f: tail,
            region: Region::bone(R.tail, R.tail2, 0.09, 0.0055, 0.007),
            target: 1300,
            flow: Flow::new(R.tail, R.tail2, back, 0.16),
            pivot: R.tail,
            mirror: None,
            variant: false,
        });
        specs.push(Spec {
            piece: Piece::Tail2,
            f: tail2,
            region: Region::bone(R.tail2, TIP, 0.09, 0.0055, 0.007),
            target: 1500,
            flow: Flow::new(R.tail2, TIP, back, 0.16),
            pivot: R.tail2,
            mirror: None,
            variant: false,
        });
        type Legs = (u8, [SdfFn; 4], LegRest, [f32; 4], [usize; 4]);
        let legs: [Legs; 2] = [
            (
                1,
                [arm, forearm, front_cannon, front_paw],
                R.front,
                [0.15, 0.1, 0.05, 0.0],
                [380, 950, 420, 1000],
            ),
            (
                3,
                [thigh, gaskin, hind_cannon, hind_paw],
                R.hind,
                [0.2, 0.12, 0.05, 0.0],
                [420, 1050, 420, 950],
            ),
        ];
        for (k, fields, leg, reach, target) in legs {
            let j = leg.joints;
            let across = [0.0065, 0.0045, 0.0032, 0.0];
            let along = [0.009, 0.0075, 0.0075, 0.0];
            for s in 0..4 {
                let region = if s == 3 {
                    let c = v3(j[4].x, 0.03, (j[3].z + j[4].z) * 0.5);
                    Region::aabb(
                        c - v3(0.05, 0.035, 0.06),
                        c + v3(0.05, 0.035, 0.06),
                        Vec3::splat(0.0022),
                    )
                } else {
                    Region::bone(j[s], j[s + 1], reach[s], across[s], along[s])
                };
                let (a, b) = if s == 3 {
                    (j[3], j[4])
                } else {
                    (j[s], j[s + 1])
                };
                specs.push(Spec {
                    piece: Piece::Leg(k, s as u8),
                    f: fields[s],
                    region,
                    target: target[s],
                    flow: Flow::new(a, b, back, if s >= 2 { 0.08 } else { 0.12 }),
                    pivot: j[s],
                    mirror: Some(Piece::Leg(k - 1, s as u8)),
                    variant: false,
                });
            }
        }
        specs.sort_by_key(|s| std::cmp::Reverse(s.target));
        for spec in specs {
            jobs.push(Box::new(move || spec.run(KIND)));
        }
        // Nose and amber eyes.
        jobs.push(Box::new(|| {
            let color = |_p: Vec3, _n: Vec3| Vec3::ONE;
            let paint = Paint {
                flow: Flow::new(R.head, hp(NOSE), Vec3::Y, 0.1),
                color: &color,
                body: &body,
                scale: 0.25,
                pivot: R.head,
            };
            let nose = hp(NOSE);
            let mut mesh = sculpt(
                &gloss_nose,
                Region::aabb(
                    nose - Vec3::splat(0.035),
                    nose + Vec3::splat(0.035),
                    Vec3::splat(0.0014),
                ),
                450,
                &paint,
            );
            for v in &mut mesh.vertices {
                let ao = v.color[0].clamp(0.4, 1.0);
                v.color = (hex(0x0e0d0c) * ao).extend(1.0).to_array();
            }
            let axis = EYE_AXIS.normalize();
            for side in [-1.0f32, 1.0] {
                let c = hp(v3(EYE.x * side, EYE.y, EYE.z));
                let ax = v3(axis.x * side, axis.y, -axis.z);
                let rot = Quat::from_rotation_arc(Vec3::Y, ax);
                let g = ball(c, Vec3::splat(EYE_R), rot, 10, 14);
                let eye = g.finish(R.head, &body, 0.2, &|p, _, _| {
                    let t = (p - c).normalize().dot(ax);
                    let iris = mix(hex(0x6a3f0e), hex(0xc8902c), smooth(0.55, 0.85, t));
                    mix(iris, hex(0x050403), smooth(0.9, 0.93, t))
                });
                mesh.merge(&eye);
            }
            vec![Made::Mesh("animals/wolf/gloss".into(), mesh)]
        }));
    }
}

pub use deer::{FWD as DEER_HEAD_FWD, UP as DEER_HEAD_UP};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deer_meshes_fit_the_budget() {
        let (meshes, secs) = build_all(Species::Deer);
        for (k, t) in &meshes {
            eprintln!("{k}: {t}");
        }
        let hind = triangles_per_animal(Species::Deer, false, &meshes);
        let stag = triangles_per_animal(Species::Deer, true, &meshes);
        eprintln!("deer: hind {hind} stag {stag} triangles, {secs:.2} s");
        assert!(stag <= 42_000, "{stag}");
        assert!(piece_count(Species::Deer, true) <= 30);
        assert!(meshes.iter().all(|(_, t)| *t > 0));
    }
}
