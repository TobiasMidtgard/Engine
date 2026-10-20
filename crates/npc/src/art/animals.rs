//! Procedural wildlife: red deer (stags carry branching antlers and a dark neck mane), grey
//! wolves, wild boar (bristly, with a mane along the spine; boars carry tusks, piglets are
//! striped), European rabbits and starlings.
//!
//! Every animal is a set of rigid pieces (torso, neck, head, jaw, ears, tail, leg segments,
//! hooves, tusks, wings) that the pose code in `visuals::animal` places each step. Deer, wolf
//! and boar torsos come in two halves, the fore half and the croup, hinged at the loin so the back
//! bends: the fore half carries a ball behind the cut (the cut's cross-section swept round the
//! hinge) that the croup's rim slides over, so the seam stays closed as the croup tucks and
//! lifts, and the ball is painted as the cut it was drawn out of, so the coat runs on across
//! it. Each piece is sculpted in the animal's rest pose as a signed distance field and meshed
//! with surface nets on a grid aligned with its bone (fine across, coarse along), or built
//! directly as tubes (antlers, bird legs) and feather ribbons (a starling's wings and tail, the
//! folded wings laid onto its body). Vertices carry the coat pattern (countershading, rump
//! patch, masks, agouti saddle, a starling's iridescent sheen) times ambient occlusion baked
//! against the whole body, and UVs that follow the fur's flow, so the tiled fur, plumage,
//! feather, horn and hoof textures run the right way.
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

/// Rest-pose joints of a four-legged animal (deer, wolves and boars).
#[derive(Debug, Clone, Copy)]
pub struct QuadRig {
    /// Torso pivot.
    pub body: Vec3,
    /// The loin: where the croup bends on the fore half of the torso (on the spine, at the
    /// cut between the two halves).
    pub loin: Vec3,
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
    loin: v3(0.0, 1.04, 0.0),
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
    loin: v3(0.0, 0.70, -0.02),
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

/// Wild boar sow: 0.84 m at the withers and front-heavy: a deep chest and massive shoulders
/// under the hump of the withers, the back sloping down to a narrow, lower rump, short stout
/// legs, a short thick neck that hardly shows and a big wedge of a head carried low. The neck
/// is one piece (`neck2` sits on the poll); boars use the same rig scaled by [`BOAR_SCALE`],
/// piglets by [`PIGLET_SCALE`].
pub const BOAR: QuadRig = QuadRig {
    body: v3(0.0, 0.55, -0.04),
    loin: v3(0.0, 0.48, -0.06),
    neck: v3(0.0, 0.58, -0.33),
    neck2: v3(0.0, 0.66, -0.5),
    head: v3(0.0, 0.66, -0.5),
    jaw: v3(0.0, 0.552, -0.492),
    ear: v3(0.05, 0.714, -0.5803),
    tail: v3(0.0, 0.555, 0.42),
    tail2: v3(0.0, 0.355, 0.48),
    front: LegRest {
        joints: [
            v3(0.115, 0.5, -0.38),
            v3(0.125, 0.32, -0.32),
            v3(0.11, 0.155, -0.345),
            v3(0.105, 0.055, -0.35),
            v3(0.105, 0.0, -0.41),
        ],
    },
    hind: LegRest {
        joints: [
            v3(0.09, 0.5, 0.28),
            v3(0.12, 0.35, 0.19),
            v3(0.1, 0.2, 0.345),
            v3(0.092, 0.055, 0.33),
            v3(0.092, 0.0, 0.272),
        ],
    },
    belly: 0.32,
    half_width: 0.17,
    head_len: 0.45,
};

/// Boars are this much bigger than sows.
pub const BOAR_SCALE: f32 = 1.12;
/// Striped piglets, a few months old.
pub const PIGLET_SCALE: f32 = 0.45;

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
    ear: v3(0.02, 0.212, -0.108),
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

/// Starling standing on the ground: 0.21 m from bill to tail, 0.39 m wingspan.
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
    body: v3(0.0, 0.068, 0.0),
    head: v3(0.0, 0.096, -0.048),
    tail: v3(0.0, 0.074, 0.05),
    shoulder: v3(0.019, 0.086, -0.024),
    wrist: v3(0.085, 0.087, -0.026),
    hip: v3(0.0125, 0.054, 0.006),
    foot: v3(0.0125, 0.0, -0.001),
};

// ======================================================================= pieces

/// A rigid piece of an animal's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Piece {
    /// The torso; for deer and wolves its fore half (chest and barrel), the croup riding on it.
    Body,
    /// The hind half of a deer's or wolf's torso (loin, croup and haunches), bending on the
    /// fore half at the loin.
    Croup,
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
    /// A boar's lower tusks (both sides), riding on the jaw.
    Tusks,
    /// A boar's upper tusks (both sides), riding on the head in its upper lip.
    UpperTusks,
    /// A boar's crest of long bristles: 0 down the nape (on the neck), 1 over the withers
    /// (on the torso). Raised, they slide up out of the coat.
    Mane(u8),
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
            Piece::Croup => "croup".into(),
            Piece::Neck => "neck".into(),
            Piece::Neck2 => "neck2".into(),
            Piece::Head => "head".into(),
            Piece::Gloss => "gloss".into(),
            Piece::Jaw => "jaw".into(),
            Piece::Ear(s) => format!("ear{s}"),
            Piece::Tail => "tail".into(),
            Piece::Tail2 => "tail2".into(),
            Piece::Antlers => "antlers".into(),
            Piece::Tusks => "tusks".into(),
            Piece::UpperTusks => "tusks_upper".into(),
            Piece::Mane(s) => format!("mane{s}"),
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
    /// Tusks: smooth ivory, colored by the vertices.
    Ivory,
    /// Wing and tail feathers (double sided).
    Feather,
    /// Bare skin: bird legs.
    Bare,
}

/// The pieces of one animal (a male, or young) with meshes and materials, or `None` without
/// the renderer. `tint` (linear RGB, 1 = as painted) multiplies the coat.
pub fn pieces(
    species: Species,
    male: bool,
    young: bool,
    tint: Option<Vec3>,
    art: &mut ArtAssets,
) -> Option<Vec<(Piece, MeshRef, MaterialRef)>> {
    if !art.available() {
        return None;
    }
    ensure_species(species, art);
    let list = piece_list(species, male, young);
    let mut out = Vec::with_capacity(list.len());
    for (piece, skin) in list {
        let key = mesh_key(species, piece, male, young);
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
pub fn piece_names(species: Species, male: bool, young: bool) -> Vec<Piece> {
    piece_list(species, male, young)
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
        Species::Boar => "boar",
    }
}

fn mesh_key(species: Species, piece: Piece, male: bool, young: bool) -> String {
    let stag = species == Species::Deer && male && matches!(piece, Piece::Neck | Piece::Neck2);
    // Piglets are striped: their own torso, neck, head and jaw.
    let piglet = species == Species::Boar
        && young
        && matches!(
            piece,
            Piece::Body | Piece::Croup | Piece::Neck | Piece::Head | Piece::Jaw
        );
    format!(
        "animals/{}/{}{}",
        species_key(species),
        piece.key(),
        if stag {
            "_stag"
        } else if piglet {
            "_young"
        } else {
            ""
        }
    )
}

/// What each animal is made of.
fn piece_list(species: Species, male: bool, young: bool) -> Vec<(Piece, Skin)> {
    let mut v = vec![
        (Piece::Body, Skin::Coat),
        (Piece::Head, Skin::Coat),
        (Piece::Gloss, Skin::Gloss),
    ];
    match species {
        Species::Deer => {
            v.extend([
                (Piece::Croup, Skin::Coat),
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
            v.extend([
                (Piece::Croup, Skin::Coat),
                (Piece::Neck, Skin::Coat),
                (Piece::Jaw, Skin::Coat),
            ]);
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
        Species::Boar => {
            v.extend([
                (Piece::Croup, Skin::Coat),
                (Piece::Neck, Skin::Coat),
                (Piece::Jaw, Skin::Coat),
                (Piece::Tail, Skin::Coat),
            ]);
            v.extend([(Piece::Ear(0), Skin::Coat), (Piece::Ear(1), Skin::Coat)]);
            // The trotters are part of the pasterns.
            for k in 0..4 {
                for s in 0..4 {
                    v.push((Piece::Leg(k, s), Skin::Coat));
                }
            }
            if !young {
                v.extend([(Piece::Mane(0), Skin::Coat), (Piece::Mane(1), Skin::Coat)]);
            }
            if male && !young {
                v.extend([
                    (Piece::Tusks, Skin::Ivory),
                    (Piece::UpperTusks, Skin::Ivory),
                ]);
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
        Skin::Ivory => {
            let mut m = white.with_roughness(0.38);
            m.reflectance = 0.5;
            m.subsurface = 0.15;
            Some(MaterialRef::Inline(m))
        }
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
    let probe = format!("npc/{}", mesh_key(species, Piece::Body, false, false));
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
        Species::Rabbit => rabbit::jobs(&mut jobs),
        Species::Bird => {
            jobs.push(Box::new(|| {
                vec![Made::Tex("animals/bird/feather".into(), feather_texture())]
            }));
            bird::jobs(&mut jobs);
        }
        Species::Boar => {
            jobs.push(Box::new(|| {
                vec![Made::Tex("animals/hoof".into(), hoof_texture())]
            }));
            boar::jobs(&mut jobs);
        }
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

/// Triangles of one grown animal: the pieces it is made of.
pub fn triangles_per_animal(species: Species, male: bool, meshes: &[(String, usize)]) -> usize {
    triangles_of(species, male, false, meshes)
}

/// Triangles of one animal, grown or young.
pub fn triangles_of(
    species: Species,
    male: bool,
    young: bool,
    meshes: &[(String, usize)],
) -> usize {
    piece_list(species, male, young)
        .into_iter()
        .map(|(p, _)| {
            let key = mesh_key(species, p, male, young);
            meshes
                .iter()
                .find(|(k, _)| *k == key)
                .map_or(0, |(_, t)| *t)
        })
        .sum()
}

/// How many pieces one grown animal has.
pub fn piece_count(species: Species, male: bool) -> usize {
    piece_list(species, male, false).len()
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
    // Blocks of `size`³ samples whose center is farther from the surface than `reach` take
    // the center's value (the sign is all the cells away from the surface need); nearer
    // blocks of 4³ are split into blocks of 2³ the same way, and only those are sampled.
    let reach = |size: usize| (v * size as f32).length() * 0.5 * 1.3 + v.max_element();
    let (reach4, reach2) = (reach(4), reach(2));
    let block = |field: &mut Vec<f32>, lo: (usize, usize, usize), size: usize, r: f32| {
        let hi = (
            (lo.0 + size).min(nx),
            (lo.1 + size).min(ny),
            (lo.2 + size).min(nz),
        );
        let c = (local(lo.0, lo.1, lo.2) + local(hi.0 - 1, hi.1 - 1, hi.2 - 1)) * 0.5;
        let dc = f(rest(c));
        let far = dc.abs() > r;
        if far {
            for z in lo.2..hi.2 {
                for y in lo.1..hi.1 {
                    for x in lo.0..hi.0 {
                        field[idx(x, y, z)] = dc;
                    }
                }
            }
        }
        (far, hi)
    };
    for bz in (0..nz).step_by(4) {
        for by in (0..ny).step_by(4) {
            for bx in (0..nx).step_by(4) {
                let (far, hi) = block(&mut field, (bx, by, bz), 4, reach4);
                if far {
                    continue;
                }
                for sz in (bz..hi.2).step_by(2) {
                    for sy in (by..hi.1).step_by(2) {
                        for sx in (bx..hi.0).step_by(2) {
                            let (far, h) = block(&mut field, (sx, sy, sz), 2, reach2);
                            if far {
                                continue;
                            }
                            for z in sz..h.2 {
                                for y in sy..h.1 {
                                    for x in sx..h.0 {
                                        field[idx(x, y, z)] = f(rest(local(x, y, z)));
                                    }
                                }
                            }
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
    /// Paint the ball behind this cut as the cut itself (see [`Seam`]).
    seam: Option<Seam>,
}

impl Paint<'_> {
    /// The rest point and normal a surface point is painted as.
    fn source(&self, p: Vec3, n: Vec3) -> (Vec3, Vec3) {
        self.seam.map_or((p, n), |s| s.unbend(p, n))
    }
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
    // Heap order: non-negative costs sort like their bits (as f32: smaller entries).
    let key = |c: f64| (c as f32).to_bits();
    // Each edge once: a closed mesh has it as a→b in one triangle and b→a in the other.
    // (Edges listed twice or not at all, at the region's border or where the surface is
    // pinched, are not collapsible anyway.)
    let mut edges = Vec::with_capacity(faces.len() * 3 / 2 + 1);
    for f in &faces {
        for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[0])] {
            if a < b {
                let (c, _) = cost(&q, &p, a as usize, b as usize);
                edges.push(Reverse((key(c), a, b, 0u32)));
            }
        }
    }
    let mut heap = BinaryHeap::from(edges);
    let mut na: Vec<u32> = Vec::new();
    let mut nb: Vec<u32> = Vec::new();
    while tris > target {
        let Some(Reverse((_, a, b, s))) = heap.pop() else {
            break;
        };
        let (a, b) = (a as usize, b as usize);
        // Stamps only grow, so their sum tells whether either end changed since.
        if !alive[a] || !alive[b] || stamp[a] + stamp[b] != s {
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
            heap.push(Reverse((key(cst), x as u32, y as u32, stamp[x] + stamp[y])));
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

/// Area-weighted vertex normals of a triangle mesh.
fn mesh_normals(pos: &[Vec3], idx: &[u32]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; pos.len()];
    for t in idx.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| i as usize);
        let f = (pos[b] - pos[a]).cross(pos[c] - pos[a]);
        n[a] += f;
        n[b] += f;
        n[c] += f;
    }
    n.into_iter().map(|v| v.normalize_or(Vec3::Y)).collect()
}

/// Mesh the zero surface of `f` in `region`, simplified to about `target` triangles,
/// projected onto the surface, painted and stored relative to the pivot.
fn sculpt(f: Field, region: Region, target: usize, paint: &Paint) -> Mesh {
    let region = fit(f, region);
    let (positions, indices) = nets(f, &region);
    let e = region.voxel.min_element() * 0.35;
    let rough: Vec<Vec3> = positions
        .iter()
        .zip(mesh_normals(&positions, &indices))
        .map(|(&p, n)| {
            let (p, n) = paint.source(p, n);
            (paint.color)(p, n)
        })
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
            let mut n = gradient(f, p, e);
            if let Some(s) = paint.seam {
                (p, n) = s.onto_skin(p, n, region.voxel.max_element(), e);
            }
            let (q, m) = paint.source(p, n);
            let ao = occlusion(&f, &ao_field, q, m, paint.scale);
            let c = (paint.color)(q, m) * ao;
            Vertex::new(p - paint.pivot, n, paint.flow.uv(q)).with_color(c.extend(1.0))
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

// ======================================================================= the bending torso

/// Where a deer's or wolf's torso is cut in two, so its spine can flex and extend: the fore
/// half (chest and barrel, [`Piece::Body`]) ahead of the plane square to the body through
/// `pivot` (the loin), the croup ([`Piece::Croup`]) behind it, turning about the pivot's X
/// axis.
///
/// Behind the cut the fore half carries a ball: the torso's cross-section at the cut swept
/// round the pivot, up to `reach` radians either way. However the croup turns (within
/// `reach`), the edge of its cut face stays on that ball, so the wedge that opens over the
/// back or under the belly is filled with skin meeting both halves edge to edge, and the
/// side closing tucks the croup inside the fore half; at rest the ball lies inside the croup.
/// The croup reaches a little way into the fore half, its edge hidden there. The ball is painted as the cross-section it was swept from (coat, fur direction and
/// occlusion), so the coat runs on across the seam however far it opens, the hair drawn out
/// along the stretch.
#[derive(Debug, Clone, Copy)]
struct Seam {
    pivot: Vec3,
    reach: f32,
    /// The whole torso: by the cut, the fore half's rim is laid on its skin and shaded with
    /// its normals, so no crease shows where the halves meet.
    skin: SdfFn,
}

/// How far the fore half of a torso reaches past the cut, so no face lies on it (m).
const SEAM_OVERLAP: f32 = 0.003;
/// How far the croup reaches into the fore half, a hair inside its skin, so the croup's edge
/// (which meshing rounds off) stays hidden there (m).
const SEAM_TUCK: f32 = 0.02;
/// How far the croup stands proud of the fore half's ball at rest, away from the cut, so the
/// two never share a surface there (m).
const SEAM_PROUD: f32 = 0.0015;
/// How far at most the croup swells over the ball behind the cut (m).
const SEAM_SWELL: f32 = 0.006;

impl Seam {
    /// The fore half of `torso` with its ball.
    fn fore(&self, torso: SdfFn, p: Vec3) -> f32 {
        let r = p - self.pivot;
        let ahead = torso(p).max(r.z - SEAM_OVERLAP);
        let behind = -r.z - SEAM_OVERLAP;
        if behind >= ahead {
            return ahead;
        }
        let rho = r.y.hypot(r.z);
        let swept = torso(v3(p.x, self.pivot.y + rho.copysign(r.y), self.pivot.z));
        let (s, c) = self.reach.sin_cos();
        let sector = r.z.abs() * c - r.y.abs() * s;
        // Where the torso falls away behind the cut much faster than the ball's arc (the top
        // of a back sloping steeply to the rump), the ball keeps within `SEAM_SWELL` of it.
        let held = torso(p) - SEAM_SWELL;
        ahead.min(swept.max(sector).max(behind).max(held))
    }

    /// The croup: `torso` behind the cut, reaching a little way into the fore half. Where the
    /// torso falls away behind the cut faster than the ball's arc (a back sloping down to the
    /// rump, tufts of coat), the croup swells over the ball (by `SEAM_SWELL` at most), a hair
    /// proud of it, and eases back onto the torso further back: at rest the ball stays hidden,
    /// with no step or crease where it would have broken through, and as the back bends the
    /// croup's edge still meets the ball flush.
    fn croup(&self, torso: SdfFn, p: Vec3) -> f32 {
        let cut = self.pivot.z;
        let r = p - self.pivot;
        let d = torso(p);
        if r.z > 0.0 {
            let a = r.z.atan2(r.y.abs());
            let w = smooth(2.5 * self.reach, self.reach, a);
            if w <= 0.0 {
                return d;
            }
            let rho = r.y.hypot(r.z);
            let ball = torso(v3(p.x, self.pivot.y + rho.copysign(r.y), cut));
            let proud = SEAM_PROUD * smooth(0.0, self.reach, a);
            return d + w * (ball - proud - d).clamp(-SEAM_SWELL - proud, 0.0);
        }
        let inset = 0.0025 * smooth(cut, cut - SEAM_TUCK, p.z);
        (d + inset).max(cut - SEAM_TUCK - p.z)
    }

    /// A point of the fore half by the cut (where meshing rounds its rim off just under the
    /// skin) laid on the whole torso's skin, with the skin's normal: within `voxel` of the
    /// skin, fading out by twice that.
    fn onto_skin(&self, p: Vec3, n: Vec3, voxel: f32, e: f32) -> (Vec3, Vec3) {
        let z = p.z - self.pivot.z;
        let d = (self.skin)(p);
        let w =
            smooth(2.0 * voxel, voxel, d.abs()) * smooth(-0.04, -0.02, z) * smooth(0.015, 0.008, z);
        if w <= 0.0 {
            return (p, n);
        }
        let g = gradient(&self.skin, p, e);
        let p = p - g * d * w;
        let g = gradient(&self.skin, p, e);
        (p, n.lerp(g, w).normalize_or(g))
    }

    /// The rest point and normal a point of the fore half is painted as: on the ball, turned
    /// back about the pivot onto the cross-section it was swept from.
    fn unbend(&self, p: Vec3, n: Vec3) -> (Vec3, Vec3) {
        let r = p - self.pivot;
        if r.z <= 0.0 {
            return (p, n);
        }
        let a = if r.y >= 0.0 {
            r.z.atan2(r.y)
        } else {
            -r.z.atan2(-r.y)
        };
        let q = Quat::from_rotation_x(-a);
        (self.pivot + q * r, q * n)
    }
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
    /// The fore half of a torso cut at this seam: its ball is painted as the cut.
    seam: Option<Seam>,
}

/// How a species paints and shades its pieces.
#[derive(Clone, Copy)]
struct Kind {
    key: &'static str,
    /// Coat color (linear RGB) at a rest-pose point with its normal; the flag is `variant`.
    coat: fn(Piece, Vec3, Vec3, bool) -> Vec3,
    /// What a variant's mesh keys end in (a stag's neck, a piglet's striped torso).
    variant: &'static str,
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
            seam: self.seam,
        };
        let mesh = sculpt(&self.f, self.region, self.target, &paint);
        let suffix = if variant { kind.variant } else { "" };
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

/// One tuft of a fur tile: where in its cell it is rooted, which way it lies, how long it
/// is and its brightness id.
#[derive(Debug, Clone, Copy)]
struct Tuft {
    root: Vec2,
    dir: Vec2,
    len: f32,
    id: f32,
}

/// The tufts of every cell of a fur tile (row major).
fn fur_tufts(f: &Fur) -> Vec<Tuft> {
    tuft_table(f.cells, f.len, f.seed)
}

/// One tuft (or feather) per cell of an `n`×`n` tile, rows staggered, about `len` cells long.
fn tuft_table(n: i32, len: f32, seed: u32) -> Vec<Tuft> {
    let mut out = Vec::with_capacity((n * n) as usize);
    for wr in 0..n {
        for wc in 0..n {
            let stagger = if wr % 2 == 1 { 0.5 } else { 0.0 };
            let ang = (rand3(wc, wr, 2, seed) - 0.5) * 0.4;
            out.push(Tuft {
                root: Vec2::new(
                    0.5 + stagger + (rand3(wc, wr, 0, seed) - 0.5) * 0.7,
                    rand3(wc, wr, 1, seed) * 0.8,
                ),
                dir: Vec2::new(ang.sin(), ang.cos()),
                len: len * (0.8 + 0.4 * rand3(wc, wr, 3, seed)),
                id: rand3(wc, wr, 4, seed),
            });
        }
    }
    out
}

fn fur_texel(f: &Fur, tufts: &[Tuft], uv: Vec2) -> Texel {
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
            let tuft = tufts[(wr * n + wc) as usize];
            let root = Vec2::new(col as f32, row as f32) + tuft.root;
            let dir = tuft.dir;
            let d = q - root;
            let along = d.dot(dir);
            let len = tuft.len;
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
                best = Some((root.y, t, across / w, tuft.id, wc, wr));
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
            let tufts = fur_tufts(&f);
            TextureSet::bake(512, 0.16, |uv| fur_texel(&f, &tufts, uv))
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
            let tufts = fur_tufts(&f);
            TextureSet::bake(512, 0.2, |uv| fur_texel(&f, &tufts, uv))
        }
        Species::Rabbit => {
            let f = Fur {
                cells: 24,
                len: 2.6,
                width: 1.3,
                strands: 10.0,
                height: 0.0005,
                contrast: 0.1,
                agouti: 0.45,
                tint: Vec3::new(1.0, 0.98, 0.95),
                band: Vec3::new(1.2, 1.1, 0.9),
                roughness: 0.85,
                streaks: 0.1,
                seed: 31,
            };
            let tufts = fur_tufts(&f);
            TextureSet::bake(256, 0.07, |uv| fur_texel(&f, &tufts, uv))
        }
        Species::Bird => plumage_texture(),
        Species::Boar => {
            // Long coarse bristles, split at the tips, a pale band on many: grizzled.
            let f = Fur {
                cells: 14,
                len: 4.4,
                width: 0.9,
                strands: 4.5,
                height: 0.0024,
                contrast: 0.3,
                agouti: 0.45,
                tint: Vec3::new(1.0, 0.98, 0.97),
                band: Vec3::new(1.4, 1.36, 1.3),
                roughness: 0.86,
                streaks: 0.18,
                seed: 47,
            };
            let tufts = fur_tufts(&f);
            TextureSet::bake(512, 0.2, |uv| fur_texel(&f, &tufts, uv))
        }
    }
}

/// A starling's contour feathers: rows of overlapping rounded feathers lying along +V with
/// glossy vanes (metallic, so the iridescent vertex tint colors their sheen) and, on some, a
/// pale buff spot at the tip.
fn plumage_texture() -> TextureSet {
    const CELLS: i32 = 8;
    let tufts = tuft_table(CELLS, 1.9, 43);
    TextureSet::bake(256, 0.045, |uv| plumage_texel(&tufts, CELLS, uv))
}

fn plumage_texel(tufts: &[Tuft], n: i32, uv: Vec2) -> Texel {
    let q = uv * n as f32;
    let (ix, iy) = (q.x.floor() as i32, q.y.floor() as i32);
    // The visible feather is the one rooted farthest upstream (its tip lies on top).
    let mut best: Option<(f32, f32, f32, f32)> = None; // root y, along t, across s, id
    for dy in 0..=3 {
        let row = iy - dy;
        for dx in -1..=1 {
            let col = ix + dx;
            let (wc, wr) = (col.rem_euclid(n), row.rem_euclid(n));
            let tuft = tufts[(wr * n + wc) as usize];
            let root = Vec2::new(col as f32, row as f32) + tuft.root;
            let d = q - root;
            let along = d.dot(tuft.dir);
            if along < 0.0 || along > tuft.len {
                continue;
            }
            let t = along / tuft.len;
            let across = d.x * tuft.dir.y - d.y * tuft.dir.x;
            // Narrow at the root, a rounded tip.
            let tip = ((t - 0.45) / 0.55).max(0.0);
            let w = 0.82 * (0.7 + 0.3 * smooth(0.0, 0.4, t)) * (1.0 - tip * tip).max(0.0).sqrt();
            if across.abs() > w {
                continue;
            }
            if best.is_none_or(|b| root.y < b.0) {
                best = Some((root.y, t, across / w, tuft.id));
            }
        }
    }
    let Some((_, t, s, id)) = best else {
        return Texel {
            albedo: Vec3::splat(0.06),
            height: 0.0,
            roughness: 0.6,
            metallic: 0.3,
            occlusion: 0.3,
        };
    };
    let a = s.abs();
    let shaft = 1.0 - smooth(0.04, 0.1, a);
    // Barbs sweep out from the shaft toward the tip.
    let barb = ((t * 7.0 - a * 2.2 + id * 3.0) * TAU).sin();
    // Pale arrowhead spots on about half the feathers.
    let spot = if id < 0.42 {
        let e = Vec2::new(s / 0.42, (t - 0.87) / 0.15).length();
        1.0 - smooth(0.75, 1.0, e)
    } else {
        0.0
    };
    // Each feather catches the light a little differently.
    let glint = 1.0 + 0.16 * (id * 7.3).fract() - 0.08;
    let vane = Vec3::new(0.22, 0.21, 0.23) * glint * (0.94 + 0.06 * barb);
    let mut albedo = mix(vane, Vec3::new(0.95, 0.82, 0.6), spot);
    albedo = mix(albedo, Vec3::splat(0.2), shaft * 0.5 * (1.0 - spot));
    Texel {
        albedo,
        height: 0.00025 * (0.3 + 0.7 * smooth(0.0, 0.7, t)) * (1.0 - 0.3 * s * s)
            + 0.00003 * barb
            + 0.00004 * shaft,
        roughness: 0.42 + 0.28 * spot + 0.04 * barb,
        metallic: 0.85 * (1.0 - spot) * (1.0 - 0.3 * shaft),
        occlusion: 0.5 + 0.5 * smooth(0.0, 0.4, t),
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

/// One flight or tail feather filling the texture: U across (0.5 on the shaft), V from the
/// downy base to the tip. A glossy dark vane of fine barbs with buff fringes, broadest on
/// the outer vane (U > 0.5) and around the tip.
fn feather_texture() -> TextureSet {
    TextureSet::bake(256, 0.06, |uv| {
        let s = uv.x * 2.0 - 1.0;
        let t = uv.y;
        let a = s.abs();
        let shaft_w = 0.05 * (1.0 - 0.6 * t);
        let shaft = 1.0 - smooth(shaft_w * 0.5, shaft_w, a);
        let barb = ((t * 46.0 - a * 9.0) * TAU).sin() * 0.5 + 0.5;
        let edge = if s > 0.0 { 0.88 } else { 0.94 };
        let fringe = smooth(edge, 0.995, a).max(smooth(0.95, 0.995, t)) * smooth(0.25, 0.45, t);
        let down = 1.0 - smooth(0.04, 0.2, t);
        let vane = Vec3::new(0.065, 0.058, 0.052) * (0.85 + 0.25 * barb);
        let mut albedo = mix(vane, Vec3::new(0.36, 0.24, 0.12), fringe);
        albedo = mix(albedo, Vec3::splat(0.25), down * 0.7);
        albedo = mix(albedo, Vec3::splat(0.08), shaft * 0.8);
        Texel {
            albedo,
            height: 0.00012 * shaft + 0.000025 * barb - 0.00006 * a * a,
            roughness: 0.52 + 0.22 * fringe + 0.2 * down + 0.05 * barb,
            metallic: 0.3 * (1.0 - fringe) * (1.0 - down),
            occlusion: 1.0 - 0.2 * a * a,
        }
    })
}

// ======================================================================= deer

mod deer {
    use super::*;

    const R: QuadRig = DEER;

    const KIND: Kind = Kind {
        key: "deer",
        coat,
        variant: "_stag",
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
        // The flank fold: skin from the belly to the stifle, over the thigh bone.
        let fold = ell(m, v3(0.122, 0.76, 0.225), v3(0.06, 0.082, 0.085));
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
        d = smin(d, fold, 0.07);
        d = smin(d, shoulder, 0.05);
        d = smin(d, hip_ball, 0.05);
        smin(d, socket, 0.07)
    }

    /// The cut between the fore half of the torso and the croup.
    pub(super) const SEAM: Seam = Seam {
        pivot: R.loin,
        reach: 0.25,
        skin: torso,
    };

    fn fore(p: Vec3) -> f32 {
        SEAM.fore(torso, p)
    }

    fn croup(p: Vec3) -> f32 {
        SEAM.croup(torso, p)
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

    /// Upper arm: slim enough to stay inside the chest and triceps as the leg swings.
    pub fn arm(p: Vec3) -> f32 {
        cone(p, fj(0), fj(1) + v3(-0.008, 0.01, 0.0), 0.046, 0.036)
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

    /// Thigh: the bone and the muscle round it under the haunch, filling the flank as the leg
    /// swings.
    pub fn thigh(p: Vec3) -> f32 {
        let bone = cone(p, hj(0), hj(1), 0.058, 0.046);
        let quarter = ell(p, v3(0.112, 0.8, 0.28), v3(0.062, 0.1, 0.09));
        smin(bone, quarter, 0.06)
    }

    pub fn gaskin(p: Vec3) -> f32 {
        let g = cone_x(p, hj(1), hj(2), 0.052, 0.027, 1.15);
        // Gaskin muscles: thick under the stifle, tapering into the Achilles tendon.
        let calf = cone_x(
            p,
            v3(0.128, 0.68, 0.27),
            v3(0.12, 0.55, 0.4),
            0.052,
            0.026,
            1.2,
        );
        let achilles = cone_x(
            p,
            v3(0.12, 0.58, 0.385),
            v3(0.115, 0.47, 0.49),
            0.021,
            0.014,
            1.3,
        );
        let hock = ell(p, hj(2), v3(0.027, 0.034, 0.034));
        let point = ell(p, v3(0.115, 0.468, 0.487), v3(0.018, 0.022, 0.02));
        // The stifle: a round top the leg turns in under the flank fold.
        let stifle = sphere(p, hj(1) + v3(-0.006, 0.0, 0.01), 0.05);
        let mut d = smin(g, calf, 0.05);
        d = smin(d, stifle, 0.04);
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

    /// Short, broad and flat, lying on the rump patch, with a tuft of longer hair at the tip.
    pub fn tail(p: Vec3) -> f32 {
        let tip = R.tail2 + (R.tail2 - R.tail) * 0.12;
        let d = cone_x(p, R.tail, tip, 0.036, 0.014, 0.62);
        let tuft = ell(p, R.tail2 + v3(0.0, 0.0, 0.004), v3(0.03, 0.045, 0.02));
        let d = smin(d, tuft, 0.03);
        if d > 0.02 {
            return d;
        }
        d - 0.007 * clumps(p, R.tail2 - R.tail, 0.012, 61)
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
            Piece::Body | Piece::Croup => {
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
                // Sandy brown on the outside, cream where it lies against the rump.
                c = mix(hex(0x9c7a58), cream, smooth(0.1, -0.6, n.z));
                c = mix(c, hex(0x7a5a40), smooth(0.2, 0.9, up) * 0.5);
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
        // In two halves, the croup bending on the fore half at the loin; both painted with
        // the same flow so the coat runs on across the cut.
        let voxel = v3(0.0095, 0.0095, 0.013);
        let cut = R.loin.z;
        specs.push(Spec {
            piece: Piece::Body,
            f: fore,
            region: Region::aabb(v3(-0.3, 0.55, -0.75), v3(0.3, 1.25, cut + 0.16), voxel),
            target: 4300,
            flow: torso_flow,
            pivot: R.body,
            mirror: None,
            variant: false,
            seam: Some(SEAM),
        });
        specs.push(Spec {
            piece: Piece::Croup,
            f: croup,
            region: Region::aabb(v3(-0.3, 0.55, cut - 0.05), v3(0.3, 1.25, 0.72), voxel),
            target: 3600,
            flow: torso_flow,
            pivot: R.loin,
            mirror: None,
            variant: false,
            seam: None,
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
                seam: None,
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
                seam: None,
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
            seam: None,
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
            seam: None,
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
            seam: None,
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
                [1000, 1100, 650, 320],
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
                    seam: None,
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
                    seam: None,
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
                seam: None,
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
        variant: "",
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

    /// The cut between the fore half of the torso and the croup.
    pub(super) const SEAM: Seam = Seam {
        pivot: R.loin,
        reach: 0.36,
        skin: torso,
    };

    fn fore(p: Vec3) -> f32 {
        SEAM.fore(torso, p)
    }

    fn croup(p: Vec3) -> f32 {
        SEAM.croup(torso, p)
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
            Piece::Body | Piece::Croup | Piece::Neck => {
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
        // In two halves, the croup bending on the fore half at the loin; both painted with
        // the same flow so the coat runs on across the cut.
        let voxel = v3(0.0082, 0.0082, 0.0105);
        let cut = R.loin.z;
        specs.push(Spec {
            piece: Piece::Body,
            f: fore,
            region: Region::aabb(v3(-0.22, 0.36, -0.56), v3(0.22, 0.9, cut + 0.12), voxel),
            target: 4000,
            flow: torso_flow,
            pivot: R.body,
            mirror: None,
            variant: false,
            seam: Some(SEAM),
        });
        specs.push(Spec {
            piece: Piece::Croup,
            f: croup,
            region: Region::aabb(v3(-0.22, 0.36, cut - 0.05), v3(0.22, 0.9, 0.5), voxel),
            target: 3200,
            flow: torso_flow,
            pivot: R.loin,
            mirror: None,
            variant: false,
            seam: None,
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
            seam: None,
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
            seam: None,
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
            seam: None,
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
            seam: None,
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
            seam: None,
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
            seam: None,
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
                    seam: None,
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
                seam: None,
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

// ======================================================================= wild boar

mod boar {
    use super::*;

    const R: QuadRig = BOAR;

    const KIND: Kind = Kind {
        key: "boar",
        coat,
        variant: "_young",
        body,
        scale: 0.7,
    };

    /// Head frame: the face slopes down this way in the rest pose (38° below level).
    pub const FWD: Vec3 = v3(0.0, -0.616, -0.788);
    pub const UP: Vec3 = v3(0.0, 0.788, -0.616);

    /// Head-local coordinates from the poll: x right, y up the face, z along it to the snout.
    fn hl(p: Vec3) -> Vec3 {
        local(p, R.head, Vec3::X, UP, FWD)
    }

    fn hp(q: Vec3) -> Vec3 {
        R.head + Vec3::X * q.x + UP * q.y + FWD * q.z
    }

    const EYE: Vec3 = v3(0.058, 0.04, 0.15);
    const EYE_AXIS: Vec3 = v3(0.8, 0.32, 0.5);
    const EYE_R: f32 = 0.0105;
    /// The snout's disc (head-local center) and its half width and height.
    const DISC: Vec3 = v3(0.0, -0.014, 0.43);
    const DISC_R: Vec2 = v2(0.043, 0.036);
    const NECK_R: f32 = 0.18;
    const HEAD_BALL: f32 = 0.11;
    /// The tip of the tail at rest.
    const TIP: Vec3 = R.tail2;

    /// The top line of the back (z, y at x = 0): up from the nape onto the hump of the withers,
    /// then sloping down to the rump.
    const BACK: [Vec2; 9] = [
        v2(-0.47, 0.78),
        v2(-0.38, 0.852),
        v2(-0.28, 0.872),
        v2(-0.15, 0.846),
        v2(-0.03, 0.802),
        v2(0.1, 0.778),
        v2(0.22, 0.735),
        v2(0.34, 0.665),
        v2(0.44, 0.575),
    ];
    /// The top line of the neck (z, y at x = 0), from behind the ears to the withers.
    const NAPE: [Vec2; 3] = [v2(-0.58, 0.752), v2(-0.5, 0.78), v2(-0.42, 0.806)];
    /// How deep the crests' roots go into the coat (m): raised, they stand this much taller.
    const SINK: f32 = 0.05;

    fn fj(i: usize) -> Vec3 {
        R.front.joints[i]
    }

    fn hj(i: usize) -> Vec3 {
        R.hind.joints[i]
    }

    /// The cut between the fore half of the torso and the croup.
    pub(super) const SEAM: Seam = Seam {
        pivot: R.loin,
        reach: 0.24,
        skin: torso,
    };

    fn fore(p: Vec3) -> f32 {
        SEAM.fore(torso, p)
    }

    fn croup(p: Vec3) -> f32 {
        SEAM.croup(torso, p)
    }

    /// A low ridge of bristles along the spine and a coarse, tufted coat everywhere, on a
    /// surface `d` near `p`.
    fn bristles(p: Vec3, d: f32, crest: f32, seed: u32) -> f32 {
        if d > 0.06 {
            return d;
        }
        let ridge = smooth(0.075, 0.0, p.x.abs()) * crest;
        let tufts = clumps(p, v3(0.0, 0.25, 1.0), 0.016, seed);
        let mane = if ridge > 0.0 {
            let c = clumps(p, v3(0.0, 0.8, 1.0), 0.013, seed + 1);
            ridge * (0.45 + 0.55 * c * c)
        } else {
            0.0
        };
        d - 0.006 * tufts - mane
    }

    /// Where `p` is beside a line over the spine ((z, y) points at x = 0): how far along it
    /// (0..1), how high above it, how far past its ends, and its length.
    fn along_line(p: Vec3, line: &[Vec2]) -> (f32, f32, f32, f32) {
        let q = v2(p.z, p.y);
        let total: f32 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
        let (mut best, mut h, mut u, mut out, mut run) = (f32::MAX, 0.0, 0.0, 0.0, 0.0);
        let last = line.len() - 2;
        for (i, w) in line.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            let e = b - a;
            let len = e.length();
            let dir = e / len;
            let s = (q - a).dot(dir);
            let t = (s / len).clamp(0.0, 1.0);
            let c = a + e * t;
            let d = q.distance(c);
            if d < best {
                best = d;
                let n = v2(-dir.y, dir.x);
                let n = if n.y < 0.0 { -n } else { n };
                h = (q - c).dot(n);
                u = (run + len * t) / total;
                out = if i == 0 && s < 0.0 {
                    -s
                } else if i == last && s > len {
                    s - len
                } else {
                    0.0
                };
            }
            run += len;
        }
        (u, h, out, total)
    }

    /// A crest of long, coarse bristles standing in tufts along `line` over the spine, up to
    /// `tall` above it (tapering off at its ends), each tuft leaning back, its roots sunk
    /// [`SINK`] into the coat.
    fn crest(p: Vec3, line: &[Vec2], tall: f32, seed: u32) -> f32 {
        let (u, h, out, total) = along_line(p, line);
        if h > tall + 0.03 || h < -SINK - 0.03 || p.x.abs() > 0.06 {
            return (h - tall).max(-SINK - h).max(p.x.abs() - 0.04).max(0.02);
        }
        let taper = smooth(0.0, 0.2, u) * smooth(1.0, 0.75, u);
        // Tufts every 1.5 cm or so, leaning back as they rise, of uneven length.
        let k = (u * total + 0.45 * h.max(0.0)) / 0.019;
        let i = k.floor();
        let f = k - i;
        let r = rand3(i as i32, 0, 0, seed);
        let spike = 1.0 - (2.0 * f - 1.0).abs();
        let top = tall * taper * (0.55 + 0.45 * r) * (0.55 + 0.45 * spike);
        let rise = ((h + SINK) / (top + SINK)).clamp(0.0, 1.0);
        let w = lerp(0.03, 0.007, rise) * (0.5 + 0.5 * taper);
        let side = p.x.abs() - w + 0.003 * clumps(p, v3(0.0, 1.0, 0.4), 0.005, seed + 1);
        let d = side.max(h - top).max(-SINK - h);
        if out > 0.0 { d.max(out) } else { d }
    }

    /// The crest over the withers and the front of the back (on the fore half of the torso).
    pub fn back_crest(p: Vec3) -> f32 {
        crest(p, &BACK[..5], 0.085, 51)
    }

    /// The crest down the nape (on the neck).
    pub fn nape_crest(p: Vec3) -> f32 {
        crest(p, &NAPE, 0.07, 53)
    }

    /// The spine's top line as a chain of round cones `r` under it.
    fn spine(p: Vec3, r: f32) -> f32 {
        let mut d = f32::MAX;
        for w in BACK.windows(2) {
            let a = v3(0.0, w[0].y - r, w[0].x);
            let b = v3(0.0, w[1].y - r, w[1].x);
            d = d.min(cone(p, a, b, r, r));
        }
        d
    }

    pub fn torso(p: Vec3) -> f32 {
        let m = mx(p);
        // Front-heavy: a deep chest under the hump of the withers, a barrel and belly slung
        // low, and the back sloping down to a narrower, lower rump.
        let chest = ell(p, v3(0.0, 0.54, -0.3), v3(0.18, 0.255, 0.22));
        let hump = ell(p, v3(0.0, 0.73, -0.28), v3(0.15, 0.15, 0.2));
        let barrel = ell(p, v3(0.0, 0.555, -0.04), v3(0.17, 0.215, 0.34));
        let belly = ell(p, v3(0.0, 0.47, -0.1), v3(0.145, 0.15, 0.22));
        let loin = ell(p, v3(0.0, 0.585, 0.17), v3(0.118, 0.105, 0.17));
        let rump = ell(p, v3(0.0, 0.51, 0.31), v3(0.09, 0.115, 0.12));
        let back = spine(p, 0.06);
        // Massive shoulders: the blades, the triceps behind the upper arm, the point.
        let scap = ell(m, v3(0.11, 0.63, -0.3), v3(0.065, 0.16, 0.1));
        let triceps = ell(m, v3(0.11, 0.42, -0.3), v3(0.058, 0.1, 0.08));
        let point = ell(m, v3(0.1, 0.5, -0.42), v3(0.055, 0.07, 0.06));
        // Haunches: thigh, buttock and hamstring.
        let haunch = ell(m, v3(0.065, 0.47, 0.28), v3(0.058, 0.12, 0.11));
        let butt = ell(m, v3(0.044, 0.48, 0.37), v3(0.046, 0.08, 0.058));
        let ham = cone(m, v3(0.055, 0.48, 0.38), v3(0.075, 0.34, 0.3), 0.045, 0.038);
        let fold = ell(m, v3(0.12, 0.4, 0.17), v3(0.045, 0.065, 0.07));
        // The neck turns in a ball a little bigger than its own base.
        let socket = ell(
            p,
            R.neck,
            v3((NECK_R + 0.004) / 1.1, NECK_R + 0.004, NECK_R + 0.004),
        );
        // Balls the upper arm and thigh turn in.
        let shoulder = sphere(m, fj(0), 0.06);
        let hip = sphere(m, hj(0), 0.064);
        let mut d = smin(chest, hump, 0.1);
        d = smin(d, barrel, 0.1);
        d = smin(d, belly, 0.1);
        d = smin(d, loin, 0.1);
        d = smin(d, rump, 0.08);
        d = smin(d, back, 0.08);
        d = smin(d, scap, 0.05);
        d = smin(d, triceps, 0.05);
        d = smin(d, point, 0.045);
        d = smin(d, haunch, 0.07);
        d = smin(d, butt, 0.05);
        d = smin(d, ham, 0.05);
        d = smin(d, fold, 0.06);
        d = smin(d, socket, 0.07);
        d = smin(d, shoulder, 0.04);
        d = smin(d, hip, 0.04);
        // A low ridge of bristles along the spine behind the crest.
        let crest = 0.012 * smooth(0.66, 0.74, p.y);
        bristles(p, d, crest, 3)
    }

    pub fn neck(p: Vec3) -> f32 {
        let n = cone_x(p, R.neck, R.head, NECK_R, HEAD_BALL + 0.012, 1.1);
        // The heavy throat, filling in between the jowls and the chest.
        let throat = ell(p, v3(0.0, 0.48, -0.43), v3(0.09, 0.08, 0.09));
        let d = smin(n, throat, 0.07);
        let axis = (R.head - R.neck).normalize();
        let nape = Vec3::X.cross(axis).normalize();
        let top = smooth(0.4, 0.9, (p - R.neck).dot(nape) / NECK_R);
        bristles(p, d, 0.015 * top, 7)
    }

    pub fn head(p: Vec3) -> f32 {
        let q = hl(p);
        let m = Vec3::new(q.x.abs(), q.y, q.z);
        let poll = ell(p, R.head, v3(HEAD_BALL / 1.1, HEAD_BALL, HEAD_BALL));
        // The skull, broad and high between the ears.
        let cranium = ell(q, v3(0.0, 0.035, 0.035), v3(0.085, 0.08, 0.1));
        // Heavy jowls down the back of the cheeks.
        let jowl = ell(m, v3(0.06, -0.06, 0.07), v3(0.05, 0.08, 0.095));
        // The wedge: deep and broad behind, tapering straight to the snout's disc.
        let face = cone_x(
            q,
            v3(0.0, -0.01, 0.03),
            v3(0.0, -0.014, 0.41),
            0.115,
            0.04,
            1.3,
        );
        // The snout's flat disc, a little wider than the snout.
        let e = Vec2::new(q.x / DISC_R.x, (q.y - DISC.y) / DISC_R.y);
        let rim = (e.length() - 1.0) * DISC_R.y;
        let thick = (q.z - DISC.z).abs() - 0.018;
        let disc =
            Vec2::new(rim.max(0.0), thick.max(0.0)).length() + rim.max(thick).min(0.0) - 0.006;
        // The upper lip flaring over the tusks, and the brows.
        let lip = ell(m, v3(0.034, -0.045, 0.34), v3(0.022, 0.024, 0.065));
        let brow = ell(m, v3(0.05, 0.056, 0.14), v3(0.026, 0.016, 0.036));
        let mut d = smin(poll, cranium, 0.05);
        d = smin(d, jowl, 0.05);
        d = smin(d, face, 0.05);
        d = smin(d, disc, 0.02);
        d = smin(d, lip, 0.02);
        d = smin(d, brow, 0.02);
        // Small eyes deep in their sockets.
        let axis = EYE_AXIS.normalize();
        d = ssub(d, sphere(m, EYE + axis * 0.003, EYE_R + 0.001), 0.003);
        let ee = m - EYE;
        let along = ee.dot(axis);
        let ring = Vec2::new((ee - axis * along).length() - EYE_R, along - 0.001).length() - 0.0035;
        d = smin(d, ring, 0.003);
        // Nostrils in the disc.
        d = ssub(
            d,
            ell(m, v3(0.014, -0.012, 0.449), v3(0.0075, 0.0095, 0.01)),
            0.002,
        );
        if d > 0.05 {
            return d;
        }
        // Bristly: tufts over the crown, a ruff of longer hair down the cheeks.
        let crown = smooth(0.22, 0.05, q.z) * smooth(-0.04, 0.03, q.y);
        let cheek = smooth(0.04, 0.08, m.x) * smooth(0.0, -0.08, q.y) * smooth(0.2, 0.06, q.z);
        let amount = 0.008 * crown + 0.016 * cheek;
        if amount <= 0.0 {
            return d;
        }
        d - amount * (0.3 + 0.7 * clumps(m, v3(0.2, -0.2, -1.0), 0.012, 29))
    }

    pub fn jaw(p: Vec3) -> f32 {
        let q = hl(p);
        let m = Vec3::new(q.x.abs(), q.y, q.z);
        let jaw = cone_x(
            q,
            v3(0.0, -0.095, 0.06),
            v3(0.0, -0.058, 0.39),
            0.05,
            0.022,
            0.95,
        );
        let chin = ell(q, v3(0.0, -0.07, 0.38), v3(0.024, 0.02, 0.03));
        let lip = ell(m, v3(0.02, -0.055, 0.3), v3(0.016, 0.012, 0.08));
        let d = smin(smin(jaw, chin, 0.02), lip, 0.015);
        // Bristles under the chin.
        let fur = smooth(0.3, 0.1, q.z) * smooth(-0.08, -0.11, q.y);
        d - fur * 0.01 * clumps(m, v3(0.0, -0.4, -1.0), 0.012, 31)
    }

    /// The tusks of one side (head-local): the lower one curving up out of the jaw and back,
    /// the upper one curling up against it.
    fn tusk_curves() -> [(Vec<Vec3>, Vec<f32>); 2] {
        [
            (
                vec![
                    v3(0.022, -0.068, 0.345),
                    v3(0.042, -0.048, 0.362),
                    v3(0.064, -0.014, 0.366),
                    v3(0.078, 0.018, 0.35),
                    v3(0.082, 0.038, 0.322),
                ],
                vec![0.0115, 0.011, 0.0092, 0.006, 0.0018],
            ),
            (
                vec![
                    v3(0.024, -0.03, 0.338),
                    v3(0.046, -0.02, 0.352),
                    v3(0.064, -0.002, 0.348),
                    v3(0.07, 0.014, 0.332),
                ],
                vec![0.0085, 0.008, 0.0052, 0.0018],
            ),
        ]
    }

    /// Ear frame (right ear): along the ear, across it, out of the cup.
    pub fn ear_frame() -> (Vec3, Vec3, Vec3) {
        let along = (Vec3::X * 0.32 + UP * 0.92 - FWD * 0.22).normalize();
        let cup0 = (Vec3::X * 0.45 + FWD * 0.85 + UP * 0.1).normalize();
        let cup = (cup0 - along * cup0.dot(along)).normalize();
        let across = along.cross(cup);
        (along, across, cup)
    }

    /// Erect and pointed, thick with hair, a fringe of bristles round the rim.
    pub fn ear(p: Vec3) -> f32 {
        let (along, across, cup) = ear_frame();
        let q = local(p, R.ear, across, along, cup);
        let s = Vec2::new(q.x, q.y);
        let tri = convex2(s, &[v2(-0.042, -0.01), v2(0.042, -0.01), v2(0.004, 0.1)]) - 0.004;
        let th = 0.013 - 0.007 * (q.y / 0.1).clamp(0.0, 1.0);
        let plate = smax(tri, q.z.abs() - th, 0.008) - 0.002;
        let hollow = smax(
            convex2(s, &[v2(-0.03, 0.006), v2(0.03, 0.006), v2(0.004, 0.08)]),
            (q.z - th - 0.004).abs() - th * 0.8,
            0.005,
        );
        let base = ell(q, v3(0.0, 0.0, -0.004), v3(0.04, 0.026, 0.021));
        let d = smin(ssub(plate, hollow, 0.004), base, 0.012);
        if d > 0.02 {
            return d;
        }
        let rim = smooth(-0.012, 0.0, tri);
        d - 0.006 * rim * clumps(q, v3(0.0, 1.0, 0.0), 0.006, 41)
    }

    /// Thin, hanging straight down, a flat tassel of bristles at the end.
    pub fn tail(p: Vec3) -> f32 {
        let d = cone(p, R.tail, TIP, 0.02, 0.011);
        let tuft = ell(p, TIP + v3(0.0, -0.025, 0.004), v3(0.022, 0.05, 0.011));
        let d = smin(d, tuft, 0.02);
        if d > 0.03 {
            return d;
        }
        d - 0.008 * clumps(p, TIP - R.tail, 0.01, 37)
    }

    pub fn arm(p: Vec3) -> f32 {
        cone(p, fj(0), fj(1), 0.062, 0.05)
    }

    /// Short and stout: a heavy forearm, the knobbed carpus.
    pub fn forearm(p: Vec3) -> f32 {
        let fa = cone_x(p, fj(1), fj(2), 0.052, 0.032, 1.1);
        let olecranon = ell(p, fj(1) + v3(0.0, 0.0, 0.032), v3(0.03, 0.04, 0.03));
        let muscle = ell(
            p,
            fj(1).lerp(fj(2), 0.35) + v3(0.0, 0.0, -0.008),
            v3(0.04, 0.07, 0.038),
        );
        let carpus = ell(p, fj(2), v3(0.029, 0.032, 0.03));
        let mut d = smin(fa, olecranon, 0.03);
        d = smin(d, muscle, 0.035);
        smin(d, carpus, 0.02)
    }

    fn cannon(p: Vec3, top: Vec3, bottom: Vec3, ra: f32) -> f32 {
        let c = cone_x(p, top, bottom, ra, 0.024, 1.15);
        let fetlock = ell(p, bottom + v3(0.0, 0.0, 0.004), v3(0.028, 0.03, 0.031));
        smin(c, fetlock, 0.02)
    }

    pub fn front_cannon(p: Vec3) -> f32 {
        cannon(p, fj(2), fj(3), 0.028)
    }

    pub fn hind_cannon(p: Vec3) -> f32 {
        cannon(p, hj(2), hj(3), 0.029)
    }

    fn coronet(j3: Vec3, toe: Vec3) -> Vec3 {
        j3 + (toe - j3) * 0.55 + v3(0.0, 0.008, 0.0)
    }

    /// The pastern and its trotter in one.
    fn foot(p: Vec3, j3: Vec3, toe: Vec3) -> f32 {
        let pastern = cone_x(p, j3, coronet(j3, toe), 0.024, 0.026, 1.05);
        smin(pastern, hoof(p, j3, toe), 0.006)
    }

    pub fn front_foot(p: Vec3) -> f32 {
        foot(p, fj(3), fj(4))
    }

    pub fn hind_foot(p: Vec3) -> f32 {
        foot(p, hj(3), hj(4))
    }

    /// A pig's trotter: two blunt, rounded claws with a cleft between them, heel bulbs, flat
    /// soles, and big dewclaws set low behind, nearly touching the ground.
    fn hoof(p: Vec3, j3: Vec3, toe: Vec3) -> f32 {
        let f = (toe.z - j3.z).signum();
        let q = p - v3(toe.x, 0.0, 0.0);
        let x = q.x.abs();
        // Side profile in (back, up), measured from the toe tip.
        let b = (toe.z - p.z) * f;
        let s = Vec2::new(b, p.y);
        let profile = [
            Vec2::new(-0.002, 0.0),
            Vec2::new(0.054, 0.0),
            Vec2::new(0.058, 0.026),
            Vec2::new(0.03, 0.043),
            Vec2::new(0.004, 0.024),
            Vec2::new(-0.004, 0.006),
        ];
        let side = convex2(s, &profile);
        // Each claw rounds off toward its tip.
        let t = smooth(-0.004, 0.04, b);
        let half = 0.0055 + 0.006 * t;
        let mid = 0.0045 + half;
        let across = (x - mid).abs() - half;
        let claw = smax(side, across, 0.007);
        let bulb = ell(
            Vec3::new(x, p.y, b),
            v3(0.013, 0.014, 0.049),
            v3(0.011, 0.014, 0.013),
        );
        let mut d = smin(claw, bulb, 0.008);
        // Flat soles on the ground.
        d = smax(d, -p.y, 0.0015);
        // Dewclaws behind the fetlock, pointing down and back.
        let bj = (toe.z - j3.z) * f;
        let dew = cone(
            Vec3::new(x, p.y, b),
            v3(0.016, j3.y - 0.012, bj + 0.013),
            v3(0.018, 0.016, bj + 0.031),
            0.0085,
            0.0048,
        );
        d.min(dew)
    }

    /// Thigh: the bone and the heavy muscle round it under the haunch.
    pub fn thigh(p: Vec3) -> f32 {
        let bone = cone(p, hj(0), hj(1), 0.07, 0.056);
        let quarter = ell(
            p,
            hj(0).lerp(hj(1), 0.5) + v3(-0.01, 0.0, 0.03),
            v3(0.05, 0.09, 0.08),
        );
        smin(bone, quarter, 0.06)
    }

    pub fn gaskin(p: Vec3) -> f32 {
        let g = cone_x(p, hj(1), hj(2), 0.056, 0.031, 1.12);
        let calf = cone_x(
            p,
            hj(1).lerp(hj(2), 0.2) + v3(-0.004, 0.0, 0.045),
            hj(1).lerp(hj(2), 0.7) + v3(-0.006, 0.0, 0.03),
            0.05,
            0.029,
            1.15,
        );
        let achilles = cone(
            p,
            hj(1).lerp(hj(2), 0.65) + v3(-0.01, 0.0, 0.035),
            hj(2) + v3(0.0, 0.012, 0.024),
            0.017,
            0.013,
        );
        let hock = ell(p, hj(2) + v3(0.0, 0.004, 0.013), v3(0.026, 0.03, 0.026));
        let stifle = sphere(p, hj(1) + v3(-0.006, 0.0, 0.008), 0.054);
        let mut d = smin(g, calf, 0.04);
        d = smin(d, stifle, 0.04);
        d = smin(d, achilles, 0.025);
        smin(d, hock, 0.015)
    }

    pub fn gloss_nose(p: Vec3) -> f32 {
        let q = hl(p);
        smax(
            head(p) - 0.0018,
            ell(q, DISC + v3(0.0, 0.0, 0.018), v3(0.05, 0.043, 0.016)),
            0.003,
        )
    }

    /// Everything, for occlusion.
    pub fn body(p: Vec3) -> f32 {
        const PARTS: [Bound; 10] = [
            Bound {
                center: v3(0.0, 0.57, -0.02),
                radius: 0.6,
                f: torso,
            },
            Bound {
                center: v3(0.0, 0.62, -0.42),
                radius: 0.28,
                f: neck,
            },
            Bound {
                center: v3(0.0, 0.52, -0.68),
                radius: 0.32,
                f: head,
            },
            Bound {
                center: v3(0.0, 0.45, -0.68),
                radius: 0.28,
                f: jaw,
            },
            Bound {
                center: v3(0.12, 0.25, -0.33),
                radius: 0.15,
                f: forearm,
            },
            Bound {
                center: v3(0.105, 0.1, -0.35),
                radius: 0.09,
                f: front_cannon,
            },
            Bound {
                center: v3(0.12, 0.29, 0.27),
                radius: 0.17,
                f: gaskin,
            },
            Bound {
                center: v3(0.1, 0.12, 0.34),
                radius: 0.1,
                f: hind_cannon,
            },
            Bound {
                center: v3(0.115, 0.47, 0.24),
                radius: 0.17,
                f: thigh,
            },
            Bound {
                center: v3(0.0, 0.53, 0.5),
                radius: 0.16,
                f: tail,
            },
        ];
        union_of(&PARTS, mx(p))
    }

    /// Wild boar coat (linear RGB): dark grey-brown, grizzled on the flanks and pale on the
    /// cheeks, near black along the spine, down the legs and on the snout; the crests' long
    /// bristles are black with pale grizzled tips. Piglets (`young`) are rufous with pale
    /// stripes running the length of the body.
    fn coat(piece: Piece, p: Vec3, n: Vec3, young: bool) -> Vec3 {
        if young {
            return piglet(piece, p, n);
        }
        let base = hex(0x312a25);
        let dark = hex(0x141110);
        let grizzle = hex(0x7a7369);
        let up = n.y;
        let mut c = mix(base, dark, smooth(0.2, 0.9, up) * 0.4);
        // Grizzled patches where the pale-banded bristles lie thick.
        let patch = smooth(0.0, 0.6, mottle(p, 5.0, 17));
        match piece {
            Piece::Mane(s) => {
                let line: &[Vec2] = if s == 0 { &NAPE } else { &BACK[..5] };
                let (_, h, _, _) = along_line(p, line);
                // Black at the roots, the long bristles' tips pale and grizzled, catching
                // the light.
                let tip = smooth(0.0, 0.05, h);
                c = mix(
                    dark * 0.8,
                    hex(0xa49a8a),
                    tip * (0.55 + 0.45 * patch.max(0.0)),
                );
            }
            Piece::Body | Piece::Croup | Piece::Neck => {
                c = mix(c, grizzle, patch * 0.3 * smooth(0.9, 0.3, up));
                let mane = smooth(0.09, 0.0, p.x.abs()) * smooth(0.3, 0.85, up);
                c = mix(c, dark * 0.7, mane * 0.9);
                // Darker under the belly and between the legs.
                let under = smooth(0.0, -0.6, up) * smooth(0.48, 0.38, p.y);
                c = mix(c, dark, under * 0.5);
                if piece == Piece::Neck {
                    // Grizzled down the sides of the throat.
                    let side = smooth(0.3, 0.8, n.x.abs()) * smooth(0.6, 0.48, p.y);
                    c = mix(c, grizzle, side * 0.3);
                }
            }
            Piece::Leg(_, s) => {
                // Near black, darkest toward the trotters; the trotters dark horn, the soles
                // worn paler.
                c = mix(base, dark, 0.55 + 0.35 * smooth(0.4, 0.1, p.y));
                if s == 0 {
                    c = mix(c, base, smooth(0.42, 0.55, p.y) * 0.5);
                }
                if s == 3 {
                    let horn = smooth(0.038, 0.03, p.y);
                    let sole = smooth(0.006, 0.0, p.y);
                    c = mix(c, mix(hex(0x262120), hex(0x4a403a), sole * 0.6), horn);
                }
            }
            Piece::Head | Piece::Jaw => {
                let q = hl(p);
                let m = Vec3::new(q.x.abs(), q.y, q.z);
                c = mix(base, dark, smooth(0.3, 0.9, up) * 0.35);
                // Pale grizzled whiskers along the jaw line, from under the eye back onto
                // the throat.
                let cheek = smooth(0.03, 0.06, m.x)
                    * smooth(0.0, -0.04, q.y)
                    * smooth(-0.15, -0.09, q.y)
                    * smooth(0.3, 0.2, q.z)
                    * smooth(0.06, 0.13, q.z);
                c = mix(c, grizzle * 1.1, cheek * 0.6);
                // The snout darkens to near black; the disc is wet and dark.
                c = mix(c, dark, smooth(0.2, 0.34, q.z) * 0.85);
                let disc = smooth(0.415, 0.43, q.z);
                c = mix(c, hex(0x2e2624), disc);
                // Dark round the small eyes.
                let ring = smooth(0.03, 0.016, m.distance(EYE));
                c = mix(c, dark, ring * 0.6);
                if piece == Piece::Jaw {
                    let lip = smooth(0.006, 0.0, (q.y + 0.05).abs()) * smooth(0.2, 0.26, q.z);
                    c = mix(c, hex(0x2a2220), lip * 0.8);
                    let mouth = smooth(0.3, 0.8, up)
                        * smooth(-0.058, -0.048, q.y)
                        * smooth(0.02, 0.012, m.x);
                    c = mix(c, hex(0x5a3a38), mouth);
                }
            }
            Piece::Ear(_) => {
                let (along, across, cup) = ear_frame();
                let q = local(mx(p), R.ear, across, along, cup);
                let inside = smooth(0.2, 0.7, mx(n).dot(cup));
                c = mix(dark, grizzle * 0.8, inside * 0.6);
                c = mix(c, dark * 0.7, smooth(0.08, 0.11, q.y) * 0.8);
            }
            Piece::Tail => {
                c = mix(dark, base, smooth(0.6, 0.5, p.y) * 0.3);
            }
            _ => {}
        }
        c * (1.0 + 0.12 * mottle(p, 7.0, 9))
    }

    /// A piglet's coat: buff and chestnut stripes running the length of the body from the dark
    /// line down the spine, breaking into spots on the haunches, and on down the face.
    fn piglet(piece: Piece, p: Vec3, n: Vec3) -> Vec3 {
        let brown = hex(0x4b3829);
        let pale = hex(0xb0a084);
        let dark = hex(0x221913);
        let up = n.y;
        let mut c = mix(brown, dark, smooth(0.3, 0.95, up) * 0.25);
        // Angle round the long axis from the spine (0) to the belly (π), and where along.
        let (a, along) = match piece {
            Piece::Head | Piece::Jaw => {
                let q = hl(p);
                (q.x.atan2(q.y).abs(), q.z * 0.5 - 0.6)
            }
            Piece::Neck => (p.x.atan2(p.y - 0.6).abs(), p.z),
            _ => (p.x.atan2(p.y - 0.56).abs(), p.z),
        };
        let wave = 0.08 * (along * 11.0).sin();
        let phase = a / PI * 6.0 + wave;
        let stripe = smooth(0.55, 0.85, (phase * TAU).cos());
        // Broken into dashes on the haunches.
        let dash = 1.0 - smooth(0.25, 0.4, along) * smooth(-0.2, 0.4, (along * 55.0).sin());
        let flank = smooth(0.25, 0.45, a) * smooth(2.4, 2.0, a);
        // The face is plain.
        let face = match piece {
            Piece::Head | Piece::Jaw => smooth(0.16, 0.06, hl(p).z),
            _ => 1.0,
        };
        c = mix(c, pale, stripe * flank * dash * face * 0.85);
        // The dark line down the spine, and a paler belly.
        c = mix(c, dark, smooth(0.22, 0.08, a) * 0.8);
        c = mix(c, mix(brown, pale, 0.5), smooth(2.3, 2.7, a) * 0.6);
        if matches!(piece, Piece::Head | Piece::Jaw) {
            let q = hl(p);
            c = mix(c, dark, smooth(0.3, 0.4, q.z) * 0.8);
            c = mix(c, hex(0x5a4440), smooth(0.415, 0.43, q.z));
        }
        if let Piece::Leg(_, 3) = piece {
            c = mix(c, hex(0x2a2420), smooth(0.038, 0.03, p.y));
        }
        c * (1.0 + 0.08 * mottle(p, 9.0, 13))
    }

    pub fn jobs(jobs: &mut Vec<Job>) {
        let up = Vec3::Y;
        let back = Vec3::Z;
        let mut specs: Vec<Spec> = Vec::new();
        let mut torso_flow =
            Flow::new(v3(0.0, 0.58, -0.55), v3(0.0, 0.58, 0.5), up, 0.2).tilt(0.35);
        torso_flow.start = 0.14;
        // In two halves, the croup bending on the fore half at the loin; both painted with
        // the same flow so the coat runs on across the cut. Piglets get their own, striped,
        // and a little coarser (they are small).
        let voxel = v3(0.0085, 0.0085, 0.011);
        let cut = R.loin.z;
        let neck_flow = Flow::new(R.head, R.neck, up - back, 0.2).tilt(0.2);
        let nose = hp(v3(0.0, 0.0, 0.46));
        let mut head_flow = Flow::new(nose, hp(v3(0.0, 0.0, -0.08)), UP, 0.14).tilt(0.15);
        head_flow.start = 0.02;
        for young in [false, true] {
            let k = if young { 0.6 } else { 1.0 };
            let n = |t: f32| (t * k) as usize;
            specs.push(Spec {
                piece: Piece::Body,
                f: fore,
                region: Region::aabb(v3(-0.27, 0.26, -0.62), v3(0.27, 0.92, cut + 0.14), voxel),
                target: n(4800.0),
                flow: torso_flow,
                pivot: R.body,
                mirror: None,
                variant: young,
                seam: Some(SEAM),
            });
            specs.push(Spec {
                piece: Piece::Croup,
                f: croup,
                region: Region::aabb(v3(-0.27, 0.26, cut - 0.05), v3(0.27, 0.9, 0.6), voxel),
                target: n(3200.0),
                flow: torso_flow,
                pivot: R.loin,
                mirror: None,
                variant: young,
                seam: None,
            });
            specs.push(Spec {
                piece: Piece::Neck,
                f: neck,
                region: Region::bone(R.neck, R.head, 0.26, 0.0068, 0.0085),
                target: n(2200.0),
                flow: neck_flow,
                pivot: R.neck,
                mirror: None,
                variant: young,
                seam: None,
            });
            specs.push(Spec {
                piece: Piece::Head,
                f: head,
                region: Region::bone(hp(v3(0.0, 0.0, -0.1)), nose, 0.19, 0.0036, 0.0045),
                target: n(5200.0),
                flow: head_flow,
                pivot: R.head,
                mirror: None,
                variant: young,
                seam: None,
            });
            // The lower jaw: a piglet's is striped and rufous like its head.
            specs.push(Spec {
                piece: Piece::Jaw,
                f: jaw,
                region: Region::bone(
                    hp(v3(0.0, -0.09, 0.0)),
                    hp(v3(0.0, -0.06, 0.42)),
                    0.08,
                    0.0032,
                    0.0042,
                ),
                target: n(1200.0),
                flow: Flow::new(hp(v3(0.0, -0.06, 0.42)), hp(v3(0.0, -0.09, 0.0)), -UP, 0.12),
                pivot: R.jaw,
                mirror: None,
                variant: young,
                seam: None,
            });
        }
        // The crests of long bristles: down the nape on the neck, over the withers on the
        // fore half of the torso (raised, they slide up out of the coat).
        for (s, line, f, pivot) in [
            (0u8, &NAPE[..], nape_crest as SdfFn, R.neck),
            (1, &BACK[..5], back_crest as SdfFn, R.body),
        ] {
            let (a, b) = (line[0], line[line.len() - 1]);
            let lo = a.y.min(b.y) - SINK - 0.02;
            let hi = a.y.max(b.y) + 0.1;
            specs.push(Spec {
                piece: Piece::Mane(s),
                f,
                region: Region::aabb(
                    v3(-0.045, lo, a.x - 0.02),
                    v3(0.045, hi, b.x + 0.02),
                    Vec3::splat(0.0026),
                ),
                target: if s == 0 { 700 } else { 1100 },
                flow: Flow::new(v3(0.0, a.y, a.x), v3(0.0, b.y, b.x), up, 0.08),
                pivot,
                mirror: None,
                variant: false,
                seam: None,
            });
        }
        let (along, _, cup) = ear_frame();
        specs.push(Spec {
            piece: Piece::Ear(1),
            f: ear,
            region: Region::bone(
                R.ear - along * 0.02,
                R.ear + along * 0.12,
                0.065,
                0.0028,
                0.0036,
            ),
            target: 650,
            flow: Flow::new(R.ear, R.ear + along * 0.12, cup, 0.08),
            pivot: R.ear,
            mirror: Some(Piece::Ear(0)),
            variant: false,
            seam: None,
        });
        specs.push(Spec {
            piece: Piece::Tail,
            f: tail,
            region: Region::bone(R.tail, TIP, 0.08, 0.004, 0.005),
            target: 520,
            flow: Flow::new(R.tail, TIP, back, 0.1),
            pivot: R.tail,
            mirror: None,
            variant: false,
            seam: None,
        });
        // Legs: right side sculpted, left mirrored; the last piece is the pastern with its
        // trotter.
        type Legs = (u8, [SdfFn; 4], LegRest, [f32; 3], [usize; 4]);
        let legs: [Legs; 2] = [
            (
                1,
                [arm, forearm, front_cannon, front_foot],
                R.front,
                [0.16, 0.11, 0.065],
                [520, 900, 520, 820],
            ),
            (
                3,
                [thigh, gaskin, hind_cannon, hind_foot],
                R.hind,
                [0.2, 0.13, 0.065],
                [950, 1100, 520, 820],
            ),
        ];
        for (k, fields, leg, reach, target) in legs {
            let j = leg.joints;
            let across = [0.0068, 0.005, 0.0036];
            let along = [0.01, 0.0085, 0.008];
            for s in 0..4 {
                let region = if s == 3 {
                    let (z0, z1) = (j[3].z.min(j[4].z), j[3].z.max(j[4].z));
                    Region::aabb(
                        v3(j[4].x - 0.05, -0.004, z0 - 0.05),
                        v3(j[4].x + 0.05, j[3].y + 0.035, z1 + 0.05),
                        Vec3::splat(0.0018),
                    )
                } else {
                    Region::bone(j[s], j[s + 1], reach[s], across[s], along[s])
                };
                specs.push(Spec {
                    piece: Piece::Leg(k, s as u8),
                    f: fields[s],
                    region,
                    target: target[s],
                    flow: Flow::new(j[s], j[s + 1], back, if s >= 2 { 0.08 } else { 0.12 }),
                    pivot: j[s],
                    mirror: Some(Piece::Leg(k - 1, s as u8)),
                    variant: false,
                    seam: None,
                });
            }
        }
        specs.sort_by_key(|s| std::cmp::Reverse(s.target));
        for spec in specs {
            jobs.push(Box::new(move || spec.run(KIND)));
        }
        // The snout's disc and the small dark eyes.
        jobs.push(Box::new(|| {
            let color = |_p: Vec3, _n: Vec3| Vec3::ONE;
            let paint = Paint {
                flow: Flow::new(R.head, hp(DISC), UP, 0.1),
                color: &color,
                body: &body,
                scale: 0.25,
                pivot: R.head,
                seam: None,
            };
            let disc = hp(DISC);
            let mut mesh = sculpt(
                &gloss_nose,
                Region::aabb(
                    disc - Vec3::splat(0.065),
                    disc + Vec3::splat(0.065),
                    Vec3::splat(0.0016),
                ),
                500,
                &paint,
            );
            for v in &mut mesh.vertices {
                let ao = v.color[0].clamp(0.4, 1.0);
                v.color = (hex(0x3a2e2b) * ao).extend(1.0).to_array();
            }
            let axis = EYE_AXIS.normalize();
            for side in [-1.0f32, 1.0] {
                let c = hp(v3(EYE.x * side, EYE.y, EYE.z));
                let ax = Vec3::X * axis.x * side + UP * axis.y + FWD * axis.z;
                let rot = Quat::from_rotation_arc(Vec3::Y, ax);
                let g = ball(c, Vec3::splat(EYE_R), rot, 10, 14);
                let eye = g.finish(R.head, &body, 0.2, &|p, _, _| {
                    let t = (p - c).normalize().dot(ax);
                    mix(hex(0x2a1a10), hex(0x070504), smooth(0.6, 0.9, t))
                });
                mesh.merge(&eye);
            }
            vec![Made::Mesh("animals/boar/gloss".into(), mesh)]
        }));
        // Tusks.
        jobs.push(Box::new(|| {
            vec![
                Made::Mesh("animals/boar/tusks".into(), tusks(0, R.jaw)),
                Made::Mesh("animals/boar/tusks_upper".into(), tusks(1, R.head)),
            ]
        }));
    }

    /// A boar's lower (0) or upper (1) tusks, both sides, stored relative to `pivot`: the
    /// lower pair to the jaw's hinge, the upper pair, which stay in the upper lip whatever
    /// the jaw does, to the head's.
    fn tusks(which: usize, pivot: Vec3) -> Mesh {
        let mut g = Geo::default();
        let [lower, upper] = tusk_curves();
        let (pts, rad) = if which == 0 { lower } else { upper };
        let pts: Vec<Vec3> = pts.into_iter().map(hp).collect();
        let (root, tip) = (pts[0], pts[pts.len() - 1]);
        let (p, r) = spline(&pts, &rad, 5);
        g.append(tube(&p, &r, 10));
        let color = move |p: Vec3, _n: Vec3, _uv: Vec2| {
            // Yellowed at the root, worn white toward the tip.
            let t = ((p - root).length() / (tip - root).length().max(1e-3)).clamp(0.0, 1.0);
            mix(hex(0xa8946a), hex(0xeee7d6), smooth(0.15, 0.85, t))
        };
        let right = g.finish(pivot, &body, 0.3, &color);
        let mut mesh = mirrored(&right);
        mesh.merge(&right);
        mesh
    }
}

// ======================================================================= rabbit

mod rabbit {
    use super::*;

    const R: RabbitRig = RABBIT;

    const KIND: Kind = Kind {
        key: "rabbit",
        variant: "",
        coat,
        body,
        scale: 0.22,
    };

    const EYE: Vec3 = v3(0.0265, 0.164, -0.122);
    const EYE_AXIS: Vec3 = v3(0.92, 0.18, -0.34);
    const EYE_R: f32 = 0.0098;
    /// The head is modelled small and scaled up about its joint by this much.
    const HS: f32 = 1.15;

    /// A point of the unscaled head model where it ends up.
    fn hs(v: Vec3) -> Vec3 {
        R.head + (v - R.head) * HS
    }

    /// Back from the scaled head to the model.
    fn unhs(p: Vec3) -> Vec3 {
        R.head + (p - R.head) / HS
    }
    const HEAD_BALL: f32 = 0.036;
    /// The scut at rest.
    const SCUT: Vec3 = v3(0.0, 0.132, 0.148);

    /// Crouched on all fours: a rounded back rising to the rump, big haunches.
    pub fn torso(p: Vec3) -> f32 {
        let m = mx(p);
        let back = ell(p, v3(0.0, 0.106, 0.03), v3(0.063, 0.08, 0.125));
        let rump = ell(p, v3(0.0, 0.11, 0.088), v3(0.067, 0.082, 0.068));
        let chest = ell(p, v3(0.0, 0.088, -0.058), v3(0.05, 0.06, 0.062));
        let haunch = ell(m, v3(0.045, 0.066, 0.074), v3(0.031, 0.058, 0.07));
        let shoulder = ell(m, v3(0.034, 0.088, -0.062), v3(0.024, 0.04, 0.035));
        let neck = ell(
            p,
            R.head + v3(0.0, -0.004, 0.012),
            v3(HEAD_BALL / 1.15, HEAD_BALL, HEAD_BALL) + 0.003,
        );
        let mut d = smin(back, rump, 0.04);
        d = smin(d, chest, 0.04);
        d = smin(d, haunch, 0.03);
        d = smin(d, shoulder, 0.025);
        d = smin(d, neck, 0.03);
        // Flattened belly resting near the ground.
        d = smax(d, 0.026 - p.y, 0.012);
        if d > 0.02 {
            return d;
        }
        // Soft, dense fur.
        d - 0.003 * clumps(p, v3(0.0, -0.3, 1.0), 0.008, 31)
    }

    pub fn head(p: Vec3) -> f32 {
        head_model(unhs(p)) * HS
    }

    fn head_model(p: Vec3) -> f32 {
        let m = mx(p);
        let poll = ell(p, R.head, v3(HEAD_BALL / 1.15, HEAD_BALL, HEAD_BALL));
        let cranium = ell(p, v3(0.0, 0.166, -0.112), v3(0.031, 0.033, 0.04));
        let cheek = ell(m, v3(0.024, 0.146, -0.12), v3(0.021, 0.025, 0.03));
        let snout = ell(p, v3(0.0, 0.142, -0.15), v3(0.021, 0.023, 0.026));
        let nose = ell(p, v3(0.0, 0.144, -0.168), v3(0.012, 0.011, 0.009));
        let lip = ell(m, v3(0.007, 0.132, -0.163), v3(0.009, 0.009, 0.01));
        let mut d = smin(poll, cranium, 0.025);
        d = smin(d, cheek, 0.015);
        d = smin(d, snout, 0.018);
        d = smin(d, nose, 0.008);
        d = smin(d, lip, 0.006);
        // Split upper lip and nostrils.
        d = ssub(
            d,
            ell(p, v3(0.0, 0.134, -0.171), v3(0.0014, 0.007, 0.006)),
            0.0015,
        );
        d = ssub(
            d,
            ell(m, v3(0.0055, 0.146, -0.176), v3(0.0028, 0.0018, 0.004)),
            0.0012,
        );
        // Eye sockets with lids.
        let axis = EYE_AXIS.normalize();
        d = ssub(d, sphere(m, EYE + axis * 0.002, EYE_R + 0.0005), 0.002);
        let e = m - EYE;
        let along = e.dot(axis);
        let ring = Vec2::new((e - axis * along).length() - EYE_R, along).length() - 0.0018;
        smin(d, ring, 0.002)
    }

    pub fn ear_frame() -> (Vec3, Vec3, Vec3) {
        // Up, back and a little out; the cup faces out and forward.
        let along = v3(0.22, 0.9, 0.38).normalize();
        let cup0 = v3(0.55, 0.0, -0.8).normalize();
        let cup = (cup0 - along * cup0.dot(along)).normalize();
        (along, along.cross(cup), cup)
    }

    pub fn ear(p: Vec3) -> f32 {
        let (along, across, cup) = ear_frame();
        let q = local(p, R.ear, across, along, cup);
        let leaf = ell(q, v3(0.0, 0.045, -0.001), v3(0.0155, 0.046, 0.0058));
        let base = cone(
            q,
            v3(0.0, -0.004, -0.002),
            v3(0.0, 0.022, -0.001),
            0.0085,
            0.011,
        );
        let hollow = ell(q, v3(0.0, 0.048, 0.0045), v3(0.0115, 0.038, 0.0042));
        smin(ssub(leaf, hollow, 0.002), base, 0.008)
    }

    pub fn tail(p: Vec3) -> f32 {
        let d = ell(p, SCUT, v3(0.021, 0.024, 0.017));
        let d = smin(d, sphere(p, R.tail, 0.012), 0.012);
        d - 0.003 * clumps(p, v3(0.0, 1.0, 0.4), 0.005, 37)
    }

    pub fn foreleg(p: Vec3) -> f32 {
        let f = R.front;
        let arm = ell(p, f[0] + v3(0.0, -0.008, 0.004), v3(0.017, 0.028, 0.021));
        let leg = cone(p, f[0], f[1], 0.0135, 0.009);
        smin(arm, leg, 0.012)
    }

    pub fn forefoot(p: Vec3) -> f32 {
        let f = R.front;
        let foot = cone(p, f[1], f[2] + v3(0.0, 0.007, 0.004), 0.0092, 0.0085);
        smax(foot, -p.y, 0.002)
    }

    pub fn hindleg(p: Vec3) -> f32 {
        let h = R.hind;
        // Thigh and shank folded together, as thick as the haunch they sit in.
        let shank = cone(p, h[0] + v3(0.0, -0.022, 0.0), h[1], 0.027, 0.011);
        smin(shank, sphere(p, h[0], 0.026), 0.016)
    }

    pub fn hindfoot(p: Vec3) -> f32 {
        let h = R.hind;
        let foot = cone(
            p,
            h[1] + v3(0.0, 0.002, 0.0),
            h[2] + v3(0.0, 0.008, 0.006),
            0.012,
            0.0095,
        );
        let d = smax(foot, -p.y, 0.002);
        d - 0.0015 * clumps(p, v3(0.0, 0.0, -1.0), 0.004, 41)
    }

    /// Everything, for occlusion.
    pub fn body(p: Vec3) -> f32 {
        const PARTS: [Bound; 6] = [
            Bound {
                center: v3(0.0, 0.1, 0.02),
                radius: 0.18,
                f: torso,
            },
            Bound {
                center: v3(0.0, 0.15, -0.125),
                radius: 0.08,
                f: head,
            },
            Bound {
                center: v3(0.03, 0.05, -0.08),
                radius: 0.06,
                f: foreleg,
            },
            Bound {
                center: v3(0.03, 0.01, -0.095),
                radius: 0.03,
                f: forefoot,
            },
            Bound {
                center: v3(0.047, 0.06, 0.085),
                radius: 0.07,
                f: hindleg,
            },
            Bound {
                center: v3(0.047, 0.01, 0.06),
                radius: 0.07,
                f: hindfoot,
            },
        ];
        union_of(&PARTS, mx(p))
    }

    /// European rabbit (linear RGB): agouti grey-brown, a rufous nape, white belly and scut
    /// underside, black-edged ears.
    fn coat(piece: Piece, p: Vec3, n: Vec3, _: bool) -> Vec3 {
        let brown = hex(0x7d6a52);
        let dark = hex(0x5a4a3a);
        let buff = hex(0xa08a6c);
        let white = hex(0xdcd7cc);
        let rufous = hex(0xa0683e);
        let black = hex(0x221d19);
        let up = n.y;
        let mut c = mix(buff, brown, smooth(-0.2, 0.5, up));
        c = mix(c, dark, smooth(0.5, 1.0, up) * 0.5);
        let under = smooth(0.0, -0.6, up) * smooth(0.07, 0.04, p.y);
        c = mix(c, white, under * 0.85);
        match piece {
            Piece::Body => {
                // Rufous nape behind the ears.
                let nape = smooth(0.04, 0.015, p.distance(v3(0.0, 0.175, -0.065)));
                c = mix(c, rufous, nape * 0.8);
                // Pale chest.
                c = mix(
                    c,
                    mix(white, buff, 0.5),
                    smooth(-0.07, -0.11, p.z) * smooth(0.12, 0.08, p.y),
                );
            }
            Piece::Head => {
                let p = unhs(p);
                let m = mx(p);
                c = mix(brown, buff, smooth(0.0, -0.6, up) * 0.6);
                // Pale eye ring, white chin and lips, buff muzzle.
                let ring = smooth(0.016, 0.011, m.distance(EYE));
                c = mix(c, hex(0xd2c19f), ring * 0.8);
                let chin = smooth(0.14, 0.13, p.y) * smooth(-0.13, -0.15, p.z);
                c = mix(c, white, chin * 0.85);
                c = mix(c, buff, smooth(-0.15, -0.165, p.z) * 0.5);
                c = mix(
                    c,
                    rufous,
                    smooth(0.0, 0.6, up) * smooth(-0.11, -0.09, p.z) * 0.35,
                );
                let nose = smooth(0.006, 0.002, p.distance(v3(0.0, 0.147, -0.177)));
                c = mix(c, hex(0x7a5a52), nose * 0.7);
            }
            Piece::Ear(_) => {
                let (along, across, cup) = ear_frame();
                let q = local(mx(p), R.ear, across, along, cup);
                let inside = smooth(0.2, 0.7, mx(n).dot(cup));
                c = mix(mix(brown, buff, 0.4), hex(0xb8a48e), inside * 0.8);
                let rim = smooth(0.8, 1.0, q.x.abs() / 0.0155) * smooth(0.04, 0.07, q.y);
                let tip = smooth(0.072, 0.088, q.y);
                c = mix(c, black, (rim * 0.7 + tip).min(1.0) * 0.85);
            }
            Piece::Tail => {
                // White scut, dark on top.
                c = mix(white, hex(0xf2efe8), 0.5);
                c = mix(c, hex(0x3a3029), smooth(0.2, 0.7, up - n.z * 0.4) * 0.9);
            }
            Piece::Leg(k, _) => {
                c = mix(brown, buff, smooth(0.04, 0.0, p.y) * 0.5);
                c = mix(c, white, smooth(0.0, 0.7, -p.x.signum() * n.x) * 0.5);
                if k >= 2 {
                    c = mix(c, mix(buff, white, 0.4), smooth(0.03, 0.0, p.y) * 0.6);
                }
            }
            _ => {}
        }
        c * (1.0 + 0.08 * mottle(p, 30.0, 7))
    }

    pub fn jobs(jobs: &mut Vec<Job>) {
        let up = Vec3::Y;
        let back = Vec3::Z;
        let mut specs: Vec<Spec> = Vec::new();
        let mut torso_flow = Flow::new(v3(0.0, 0.1, -0.12), v3(0.0, 0.1, 0.17), up, 0.07).tilt(0.3);
        torso_flow.start = 0.04;
        torso_flow.end = 0.25;
        specs.push(Spec {
            piece: Piece::Body,
            f: torso,
            region: Region::aabb(
                v3(-0.11, 0.0, -0.15),
                v3(0.11, 0.21, 0.19),
                Vec3::splat(0.0042),
            ),
            target: 3200,
            flow: torso_flow,
            pivot: R.body,
            mirror: None,
            variant: false,
            seam: None,
        });
        let mut head_flow = Flow::new(
            hs(v3(0.0, 0.145, -0.18)),
            hs(v3(0.0, 0.16, -0.07)),
            up,
            0.05,
        )
        .tilt(0.2);
        head_flow.start = 0.01;
        specs.push(Spec {
            piece: Piece::Head,
            f: head,
            region: Region::aabb(
                v3(-0.055, 0.095, -0.2),
                v3(0.055, 0.215, -0.035),
                Vec3::splat(0.0024),
            ),
            target: 2200,
            flow: head_flow,
            pivot: R.head,
            mirror: None,
            variant: false,
            seam: None,
        });
        let (along, _, cup) = ear_frame();
        specs.push(Spec {
            piece: Piece::Ear(1),
            f: ear,
            region: Region::bone(
                R.ear - along * 0.01,
                R.ear + along * 0.095,
                0.022,
                0.0011,
                0.0016,
            ),
            target: 420,
            flow: Flow::new(R.ear, R.ear + along * 0.09, cup, 0.05),
            pivot: R.ear,
            mirror: Some(Piece::Ear(0)),
            variant: false,
            seam: None,
        });
        specs.push(Spec {
            piece: Piece::Tail,
            f: tail,
            region: Region::aabb(
                SCUT - v3(0.03, 0.035, 0.035),
                SCUT + v3(0.03, 0.03, 0.026),
                Vec3::splat(0.002),
            ),
            target: 320,
            flow: Flow::new(R.tail, SCUT + v3(0.0, 0.02, 0.0), back, 0.04),
            pivot: R.tail,
            mirror: None,
            variant: false,
            seam: None,
        });
        let f = R.front;
        let h = R.hind;
        // Leg, segment, field, from, to, reach, voxel, triangles.
        type LegSpec = (u8, u8, SdfFn, Vec3, Vec3, f32, f32, usize);
        let legs: [LegSpec; 4] = [
            (1, 0, foreleg, f[0], f[1], 0.03, 0.002, 260),
            (1, 1, forefoot, f[1], f[2], 0.016, 0.0014, 140),
            (3, 0, hindleg, h[0], h[1], 0.035, 0.0024, 320),
            (3, 1, hindfoot, h[1], h[2], 0.022, 0.0018, 260),
        ];
        for (k, s, f, a, b, reach, voxel, target) in legs {
            specs.push(Spec {
                piece: Piece::Leg(k, s),
                f,
                region: Region::bone(a, b, reach, voxel, voxel * 1.4),
                target,
                flow: Flow::new(a, b, back, 0.04),
                pivot: a,
                mirror: Some(Piece::Leg(k - 1, s)),
                variant: false,
                seam: None,
            });
        }
        specs.sort_by_key(|s| std::cmp::Reverse(s.target));
        for spec in specs {
            jobs.push(Box::new(move || spec.run(KIND)));
        }
        // Big dark eyes.
        jobs.push(Box::new(|| {
            let axis = EYE_AXIS.normalize();
            let mut mesh = Mesh::default();
            for side in [-1.0f32, 1.0] {
                let c = hs(v3(EYE.x * side, EYE.y, EYE.z));
                let ax = v3(axis.x * side, axis.y, axis.z);
                let rot = Quat::from_rotation_arc(Vec3::Y, ax);
                let g = ball(c, Vec3::splat(EYE_R * HS), rot, 10, 14);
                let eye = g.finish(R.head, &body, 0.1, &|p, _, _| {
                    let t = (p - c).normalize().dot(ax);
                    mix(hex(0x2a1608), hex(0x050302), smooth(0.5, 0.8, t))
                });
                mesh.merge(&eye);
            }
            vec![Made::Mesh("animals/rabbit/gloss".into(), mesh)]
        }));
    }
}

// ======================================================================= starling

mod bird {
    use super::*;

    const R: BirdRig = BIRD;

    const KIND: Kind = Kind {
        key: "bird",
        variant: "",
        coat,
        body,
        scale: 0.09,
    };

    /// The head's neck ball (the body holds a slightly bigger socket around it).
    const HEAD_BALL: f32 = 0.0125;
    const EYE: Vec3 = v3(0.0091, 0.1035, -0.0705);
    const EYE_AXIS: Vec3 = v3(0.93, 0.18, -0.32);
    const EYE_R: f32 = 0.0031;
    /// Bill base (inside the feathered forehead) and tip.
    const BILL: Vec3 = v3(0.0, 0.0985, -0.079);
    const BILL_TIP: Vec3 = v3(0.0, 0.0955, -0.111);
    /// End of the right hand, where the outer primaries sit (wing spread).
    const HAND: Vec3 = v3(0.118, 0.087, -0.02);

    // Iridescent sheens: the glossy vanes take these as their color.
    const PURPLE: Vec3 = v3(0.72, 0.36, 1.0);
    const GREEN: Vec3 = v3(0.36, 1.0, 0.5);
    const BRONZE: Vec3 = v3(1.0, 0.8, 0.52);
    /// Flight and tail feathers: dull black-brown with buff fringes.
    const DULL: Vec3 = v3(0.78, 0.76, 0.74);

    /// The body with the breast and flank feathers that cover the bend of the folded wing.
    pub fn torso(p: Vec3) -> f32 {
        let d = trunk(p);
        if d > 0.012 {
            return d;
        }
        let shoulder = ell(
            mx(p),
            v3(0.0205, 0.0745, -0.033),
            v3(0.0062, 0.0125, 0.0105),
        );
        smin(d, shoulder, 0.006)
    }

    /// Plump, short-tailed body standing a little nose-up, with the socket the head turns in
    /// and feathered thighs.
    fn trunk(p: Vec3) -> f32 {
        let m = mx(p);
        let main = ell(p, v3(0.0, 0.066, 0.004), v3(0.0235, 0.025, 0.046));
        let breast = ell(p, v3(0.0, 0.068, -0.024), v3(0.0228, 0.027, 0.027));
        let nape = ell(p, v3(0.0, 0.085, -0.034), v3(0.015, 0.0155, 0.019));
        let socket = sphere(p, R.head, HEAD_BALL + 0.0005);
        let belly = ell(p, v3(0.0, 0.054, 0.006), v3(0.0185, 0.017, 0.032));
        let vent = ell(p, v3(0.0, 0.064, 0.05), v3(0.0105, 0.009, 0.026));
        let rump = ell(p, v3(0.0, 0.079, 0.044), v3(0.0125, 0.007, 0.024));
        let thigh = ell(m, v3(0.0125, 0.047, 0.008), v3(0.0085, 0.0125, 0.011));
        let mut d = smin(main, breast, 0.012);
        d = smin(d, nape, 0.012);
        d = smin(d, socket, 0.008);
        d = smin(d, belly, 0.01);
        d = smin(d, vent, 0.01);
        d = smin(d, rump, 0.008);
        // Contour feathers lie smooth: the texture carries them.
        smin(d, thigh, 0.006)
    }

    /// Flat crown and a long sloping forehead running into the bill, big dark eyes.
    pub fn head(p: Vec3) -> f32 {
        let m = mx(p);
        let poll = sphere(p, R.head, HEAD_BALL);
        let skull = ell(p, v3(0.0, 0.1015, -0.063), v3(0.0118, 0.0115, 0.0175));
        let brow = ell(p, v3(0.0, 0.1, -0.078), v3(0.0068, 0.0076, 0.011));
        let throat = ell(p, v3(0.0, 0.0915, -0.064), v3(0.0088, 0.0085, 0.013));
        let cheek = ell(m, v3(0.006, 0.0985, -0.067), v3(0.0062, 0.0085, 0.012));
        let mut d = smin(poll, skull, 0.008);
        d = smin(d, brow, 0.006);
        d = smin(d, throat, 0.006);
        d = smin(d, cheek, 0.004);
        d = smax(d, p.y - 0.1118, 0.004);
        // Eye sockets with lids.
        let axis = EYE_AXIS.normalize();
        d = ssub(d, sphere(m, EYE + axis * 0.0006, EYE_R + 0.0002), 0.0008);
        let e = m - EYE;
        let along = e.dot(axis);
        let ring = Vec2::new((e - axis * along).length() - EYE_R, along).length() - 0.0007;
        smin(d, ring, 0.0008)
    }

    /// Long, straight and sharp, a little flattened, with the gape along each side.
    pub fn bill(p: Vec3) -> f32 {
        let s = v3(0.88, 1.15, 1.0);
        let d = round_cone(p * s, BILL * s, BILL_TIP * s, 0.0047, 0.0003) / 1.15;
        let gape = cone(
            mx(p),
            v3(0.0049, 0.0973, -0.0815),
            v3(0.0015, 0.0954, -0.103),
            0.00045,
            0.0002,
        );
        ssub(d, gape, 0.0003)
    }

    /// Everything, for occlusion.
    pub fn body(p: Vec3) -> f32 {
        torso(p).min(head(p))
    }

    /// The surface the folded wings lie on: the body and, behind it, the base of the tail.
    fn bed(p: Vec3) -> f32 {
        smin(
            trunk(p),
            ell(p, v3(0.0, 0.073, 0.075), v3(0.011, 0.0045, 0.04)),
            0.012,
        )
    }

    /// Plumage tint (linear). The glossy vanes reflect it, so it is the sheen: purple on the
    /// head and breast, green on the back and flanks, bronze toward the tail, in patches like
    /// oil on water.
    fn coat(piece: Piece, p: Vec3, n: Vec3, _: bool) -> Vec3 {
        let oil = (mottle(p, 70.0, 5) * 0.8 + 0.5).clamp(0.0, 1.0);
        let hue = mix(GREEN, PURPLE, oil * 0.5);
        if piece == Piece::Head {
            let crown = smooth(0.5, 1.0, n.y) * 0.4 + oil * 0.25;
            return mix(PURPLE, GREEN, crown) * 0.62;
        }
        let breast = smooth(-0.015, -0.04, p.z) * smooth(0.4, -0.3, n.y);
        let mut hue = mix(hue, PURPLE, breast * 0.8);
        hue = mix(hue, BRONZE, smooth(0.02, 0.065, p.z) * 0.45);
        hue * (0.72 + 0.2 * smooth(0.2, -0.6, n.y))
    }

    /// One feather: a vane from `root` along `dir`, its face toward `up`, `outer` the share
    /// of the width on the `up × dir` side.
    #[derive(Clone, Copy)]
    struct Quill {
        root: Vec3,
        dir: Vec3,
        up: Vec3,
        len: f32,
        width: f32,
        outer: f32,
        /// Arch across the vane and droop toward the tip, meters.
        camber: f32,
        droop: f32,
        /// 1 a rounded tip, 2 a pointed one, below 1 blunt.
        point: f32,
        tint: Vec3,
    }

    impl Quill {
        fn new(root: Vec3, dir: Vec3, len: f32, width: f32, tint: Vec3) -> Self {
            Self {
                root,
                dir: dir.normalize(),
                up: Vec3::Y,
                len,
                width,
                outer: 0.5,
                camber: 0.0007,
                droop: 0.0,
                point: 1.0,
                tint,
            }
        }

        /// The vane at `s` across (−1..1) and `t` along (0 root, 1 tip).
        fn at(&self, s: f32, t: f32) -> Vec3 {
            let side = self.up.cross(self.dir).normalize();
            let up = self.dir.cross(side);
            let tip = ((t - 0.45) / 0.55).max(0.0);
            let w = self.width
                * (0.55 + 0.45 * smooth(0.0, 0.2, t))
                * (1.0 - tip * tip).max(0.0).powf(0.5 * self.point);
            let half = if s > 0.0 {
                self.outer
            } else {
                1.0 - self.outer
            };
            let arch = self.camber * (1.0 - s * s) * (w / self.width);
            self.root
                + self.dir * (t * self.len)
                + side * (s * half * w)
                + up * (arch - self.droop * t * t)
        }

        /// The vane as a grid, its points from `pos(s, t)`.
        fn grid(&self, across: usize, rows: usize, pos: &dyn Fn(f32, f32) -> Vec3) -> Geo {
            let mut g = Geo::default();
            let e = 0.02;
            for i in 0..=rows {
                let t = 0.97 * i as f32 / rows as f32;
                for j in 0..=across {
                    let s = -1.0 + 2.0 * j as f32 / across as f32;
                    let ds = pos((s + e).min(1.0), t) - pos((s - e).max(-1.0), t);
                    let dt = pos(s, t + e) - pos(s, (t - e).max(0.0));
                    g.positions.push(pos(s, t));
                    g.normals.push(dt.cross(ds).normalize_or(self.up));
                    g.uvs.push(Vec2::new(0.5 + 0.5 * s, t));
                }
            }
            let w = across as u32 + 1;
            for i in 0..rows as u32 {
                for j in 0..across as u32 {
                    let (a, b, c, d) = (
                        i * w + j,
                        i * w + j + 1,
                        (i + 1) * w + j + 1,
                        (i + 1) * w + j,
                    );
                    g.indices.extend_from_slice(&[a, b, c, a, c, d]);
                }
            }
            orient(&mut g);
            g
        }

        /// Free in the air (spread wings, tail).
        fn free(&self, across: usize, rows: usize) -> Geo {
            self.grid(across, rows, &|s, t| self.at(s, t))
        }

        /// Laid `off` above the bed (folded wings), its root tucked in and its outer edge
        /// lifted a little like a shingle.
        fn laid(&self, off: f32, across: usize, rows: usize) -> Geo {
            self.grid(across, rows, &|s, t| {
                let mut p = self.at(s, t);
                let o = (off + 0.0004 * (1.0 + s)) * (0.3 + 0.7 * smooth(0.0, 0.3, t));
                for _ in 0..3 {
                    let d = bed(p) - o;
                    p -= gradient(&bed, p, 0.0005) * d;
                }
                p
            })
        }

        /// Painted (darker at the root, under the feathers above) about `pivot`.
        fn mesh(&self, g: Geo, pivot: Vec3) -> Mesh {
            let tint = self.tint;
            g.finish(pivot, &body, KIND.scale, &|_, _, uv| {
                tint * (0.55 + 0.45 * smooth(0.0, 0.45, uv.y))
            })
        }
    }

    /// Turn every triangle so it winds counter-clockwise seen from its vertices' normals.
    fn orient(g: &mut Geo) {
        for t in g.indices.chunks_mut(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
            let n = g.normals[a] + g.normals[b] + g.normals[c];
            let face = (g.positions[b] - g.positions[a]).cross(g.positions[c] - g.positions[a]);
            if face.dot(n) < 0.0 {
                t.swap(1, 2);
            }
        }
    }

    /// A wing's leading edge from `a` out to `b`: an elliptical tube `ry` thick and `rz` deep
    /// behind the edge line, rounded off at `b`. Textured with a plain stretch of vane.
    fn edge(a: Vec3, b: Vec3, ry: (f32, f32), rz: (f32, f32)) -> Geo {
        let mut g = Geo::default();
        let (rings, sides) = (8usize, 10usize);
        let along = (b - a).normalize();
        for i in 0..=rings + 2 {
            let (t, k) = if i <= rings {
                (i as f32 / rings as f32, 1.0)
            } else {
                (1.0, if i == rings + 1 { 0.6 } else { 0.0 })
            };
            let c = a.lerp(b, t) + along * (1.0 - k) * rz.1;
            let (hy, hz) = (ry.0 + (ry.1 - ry.0) * t, rz.0 + (rz.1 - rz.0) * t);
            for j in 0..=sides {
                let th = TAU * j as f32 / sides as f32;
                let (cy, cz) = (th.sin(), -th.cos());
                g.positions.push(c + v3(0.0, cy * hy * k, hz + cz * hz * k));
                let n = v3(0.0, cy / hy, cz / hz).normalize() * k + along * (1.0 - k);
                g.normals.push(n.normalize());
                g.uvs.push(Vec2::new(
                    0.2 + 0.06 * j as f32 / sides as f32,
                    0.3 + 0.6 * t,
                ));
            }
        }
        let w = sides as u32 + 1;
        for i in 0..rings as u32 + 2 {
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
        orient(&mut g);
        g
    }

    /// The right arm wing spread flat about the shoulder: secondaries and tertials, the
    /// coverts above and below them and the leading edge.
    fn spread_arm() -> Mesh {
        let sheen = mix(GREEN, PURPLE, 0.35) * 0.85;
        let mut mesh = edge(
            R.shoulder + v3(-0.006, 0.0, 0.0),
            R.wrist,
            (0.0032, 0.0024),
            (0.0085, 0.0065),
        )
        .finish(R.shoulder, &body, KIND.scale, &|_, _, _| sheen);
        for i in 0..12 {
            let f = i as f32 / 11.0;
            let x = 0.083 - f * 0.06;
            let a = 0.12 - f * 0.16;
            let dir = v3(a.sin(), 0.0, a.cos());
            let (len, tint) = if i < 9 {
                (0.057 + 0.005 * f, DULL)
            } else {
                ([0.06, 0.055, 0.048][i - 9], sheen)
            };
            let q = Quill {
                outer: 0.42,
                camber: 0.0008,
                droop: 0.002,
                point: 0.35,
                ..Quill::new(v3(x, 0.0862 + f * 0.0012, -0.019), dir, len, 0.0135, tint)
            };
            mesh.merge(&q.mesh(q.free(3, 6), R.shoulder));
            // Greater coverts over the bases, under-wing coverts below.
            if i < 10 {
                let cover = |y: f32, z: f32, len: f32, width: f32, tint: Vec3| {
                    let q = Quill {
                        outer: 0.45,
                        camber: 0.0005,
                        ..Quill::new(v3(x + 0.001, y, z), dir, len, width, tint)
                    };
                    q.mesh(q.free(2, 4), R.shoulder)
                };
                mesh.merge(&cover(0.0881 + f * 0.0006, -0.022, 0.03, 0.0118, sheen));
                mesh.merge(&cover(0.0846, -0.022, 0.03, 0.012, DULL * 0.9));
                if i % 3 != 2 {
                    mesh.merge(&cover(0.0892 + f * 0.0004, -0.025, 0.017, 0.0095, sheen));
                }
            }
        }
        mesh
    }

    /// The right hand spread flat about the wrist: primaries fanning to the pointed tip, their
    /// coverts, the alula and the leading edge.
    fn spread_hand() -> Mesh {
        const LEN: [f32; 9] = [0.06, 0.061, 0.063, 0.066, 0.07, 0.076, 0.083, 0.089, 0.088];
        let w = R.wrist;
        let sheen = mix(GREEN, PURPLE, 0.6) * 0.8;
        let mut mesh = edge(
            w + v3(-0.003, 0.0, 0.0),
            HAND + v3(0.006, 0.0, 0.0),
            (0.0025, 0.0016),
            (0.0065, 0.004),
        )
        .finish(w, &body, KIND.scale, &|_, _, _| sheen);
        for (i, &len) in LEN.iter().enumerate() {
            let f = i as f32 / 8.0;
            let a = 0.34 + 0.82 * f.powf(1.2);
            let dir = v3(a.sin(), 0.0, a.cos());
            let root = w.lerp(HAND, f) + v3(0.0, (1.0 - f) * 0.0012 - 0.0004, 0.004);
            let q = Quill {
                outer: 0.3,
                camber: 0.0009,
                droop: 0.004,
                point: 0.55,
                ..Quill::new(root, dir, len, 0.0165 - 0.003 * f, DULL * 0.95)
            };
            mesh.merge(&q.mesh(q.free(3, 8), w));
            let c = Quill {
                outer: 0.4,
                camber: 0.0005,
                point: 1.2,
                ..Quill::new(
                    root + v3(0.0, 0.0016, -0.002),
                    dir,
                    0.026,
                    0.011,
                    mix(sheen, DULL, f),
                )
            };
            mesh.merge(&c.mesh(c.free(2, 4), w));
            if i % 2 == 0 {
                let u = Quill::new(
                    root + v3(0.0, -0.0015, -0.002),
                    dir,
                    0.024,
                    0.012,
                    DULL * 0.9,
                );
                mesh.merge(&u.mesh(u.free(2, 4), w));
            }
        }
        // Alula: two small feathers at the wrist, pointing out along the edge.
        for (k, len) in [0.017f32, 0.013].into_iter().enumerate() {
            let q = Quill {
                camber: 0.0004,
                ..Quill::new(
                    w + v3(0.002, 0.0026 - k as f32 * 0.0004, 0.001),
                    v3(1.0, 0.0, 0.25 + 0.2 * k as f32),
                    len,
                    0.006,
                    sheen,
                )
            };
            mesh.merge(&q.mesh(q.free(2, 4), w));
        }
        mesh
    }

    /// The right wing folded along the side about the body pivot: primaries lowest (their
    /// tips crossing over the tail), secondaries over them along the flank, tertials along
    /// the back and the coverts at the shoulder.
    fn folded() -> Mesh {
        let side = v3(1.0, 0.25, 0.0).normalize();
        let top = v3(0.7, 1.0, 0.0).normalize();
        let sheen = mix(GREEN, PURPLE, 0.35) * 0.85;
        let mut mesh = Mesh::default();
        let mut add =
            |root: Vec3, tip: Vec3, width: f32, off: f32, tint: Vec3, up: Vec3, rows: usize| {
                let q = Quill {
                    up,
                    outer: 0.4,
                    camber: 0.0,
                    ..Quill::new(root, tip - root, root.distance(tip), width, tint)
                };
                mesh.merge(&q.mesh(q.laid(off, 2, rows), R.body));
            };
        for i in 0..8 {
            let f = i as f32;
            add(
                v3(0.021, 0.07 + f * 0.0006, -0.016 + f * 0.0012),
                v3(0.0065 + f * 0.0009, 0.077 - f * 0.0004, 0.097 - f * 0.0045),
                0.011,
                0.0005 + f * 0.00008,
                DULL * 0.95,
                side,
                7,
            );
        }
        for j in 0..8 {
            let f = j as f32;
            add(
                v3(0.0225, 0.0705 + f * 0.0018, -0.01 + f * 0.001),
                v3(0.0145 - f * 0.0003, 0.066 + f * 0.0022, 0.05 - f * 0.001),
                0.012,
                0.0013 + f * 0.0001,
                DULL,
                side,
                6,
            );
        }
        for k in 0..3 {
            let f = k as f32;
            add(
                v3(0.016 - f * 0.0015, 0.0875 + f * 0.0005, -0.008 + f * 0.002),
                v3(0.0105 - f * 0.0012, 0.0845 - f * 0.0005, 0.058 - f * 0.004),
                0.013,
                0.0022 + f * 0.0001,
                sheen,
                top,
                6,
            );
        }
        for j in 0..8 {
            let f = j as f32;
            add(
                v3(0.0232, 0.0715 + f * 0.0018, -0.026 + f * 0.0008),
                v3(0.02 - f * 0.0003, 0.07 + f * 0.002, 0.002 + f * 0.0028),
                0.011,
                0.0026 + f * 0.0001,
                sheen,
                side,
                4,
            );
        }
        for j in 0..7 {
            let f = j as f32;
            add(
                v3(0.0222, 0.0735 + f * 0.0018, -0.035),
                v3(0.0225, 0.073 + f * 0.0018, -0.016 + f * 0.0022),
                0.009,
                0.0034 + f * 0.0001,
                sheen,
                side,
                3,
            );
        }
        mesh
    }

    /// Twelve tail feathers about the tail joint, `fan` radians apart (folded or spread for
    /// flight), with the coverts over and under their base.
    fn tail(fan: f32) -> Mesh {
        let mut mesh = Mesh::default();
        for side in [-1.0f32, 1.0] {
            for i in 0..6 {
                let f = i as f32;
                let a = side * (0.012 + f * fan);
                // The middle pair lies on top; the outer ones a little lower, so the folded
                // tail is gently keeled.
                let dir = v3(a.sin(), -0.11 - 0.006 * f, a.cos());
                let root = R.tail
                    + v3(
                        side * (0.0012 + f * (0.0012 + fan * 0.006)),
                        (5.0 - f) * 0.0003,
                        0.0,
                    );
                // The narrow outer vane faces away from the middle.
                let outer = if i == 0 {
                    0.5
                } else if side > 0.0 {
                    0.38
                } else {
                    0.62
                };
                let q = Quill {
                    outer,
                    camber: 0.0009,
                    droop: 0.001,
                    point: 0.45,
                    ..Quill::new(root, dir, 0.064 - 0.0004 * f, 0.0125, DULL)
                };
                mesh.merge(&q.mesh(q.free(3, 6), R.tail));
            }
            // Long coverts over and under the base taper the tail into the body: glossy
            // above, buff-fringed below.
            for k in 0..3 {
                let x = side * (0.0018 + 0.0036 * k as f32);
                let over = Quill {
                    camber: 0.0008,
                    point: 0.7,
                    ..Quill::new(
                        v3(x, 0.0792 - 0.0004 * k as f32, 0.036),
                        v3(x * 0.3, -0.09, 1.0),
                        0.036 - 0.003 * k as f32,
                        0.013,
                        mix(GREEN, BRONZE, 0.5) * 0.85,
                    )
                };
                mesh.merge(&over.mesh(over.free(2, 4), R.tail));
                let under = Quill {
                    up: Vec3::NEG_Y,
                    camber: 0.0008,
                    point: 0.7,
                    ..Quill::new(
                        v3(x, 0.0662 + 0.0004 * k as f32, 0.037),
                        v3(x * 0.3, 0.035, 1.0),
                        0.042 - 0.004 * k as f32,
                        0.0135,
                        DULL * 1.1,
                    )
                };
                mesh.merge(&under.mesh(under.free(2, 4), R.tail));
            }
        }
        mesh
    }

    /// The right leg about the hip: a reddish-pink scaly tarsus from the feathered thigh,
    /// three toes forward and one back, dark claws.
    fn leg() -> Mesh {
        let skin = v3(0.5, 0.2, 0.16);
        let claw = v3(0.07, 0.055, 0.045);
        let x = R.foot.x;
        let ankle = v3(x, 0.031, 0.0135);
        let foot = v3(x, 0.0026, -0.0005);
        let paint = |g: Geo, c: Vec3| g.finish(R.hip, &body, KIND.scale, &|_, _, _| c);
        let (pts, rad) = spline(
            &[v3(x, 0.043, 0.01), ankle, v3(x, 0.017, 0.0065), foot],
            &[0.0018, 0.0015, 0.0013, 0.0012],
            4,
        );
        let mut mesh = paint(tube(&pts, &rad, 8), skin);
        for (yaw, len, hook) in [
            (0.0f32, 0.017f32, 0.0045f32),
            (-0.42, 0.013, 0.004),
            (0.42, 0.014, 0.004),
            (PI, 0.011, 0.0065),
        ] {
            let d = v3(yaw.sin(), 0.0, -yaw.cos());
            let end = foot + d * len + v3(0.0, -0.0016, 0.0);
            let (pts, rad) = spline(
                &[
                    foot + d * 0.001,
                    foot + d * (len * 0.5) + v3(0.0, -0.0012, 0.0),
                    end,
                ],
                &[0.0011, 0.0009, 0.0008],
                3,
            );
            mesh.merge(&paint(tube(&pts, &rad, 6), skin * 0.85));
            let (pts, rad) = spline(
                &[
                    end,
                    end + d * (hook * 0.55) + v3(0.0, 0.0001, 0.0),
                    end + d * hook + v3(0.0, -0.0011, 0.0),
                ],
                &[0.0007, 0.0005, 0.0002],
                3,
            );
            mesh.merge(&paint(tube(&pts, &rad, 6), claw));
        }
        mesh
    }

    /// Bill color: lemon yellow with a blue-grey base, like a starling in spring.
    fn bill_color(p: Vec3) -> Vec3 {
        let t = ((p.z - BILL.z) / (BILL_TIP.z - BILL.z)).clamp(0.0, 1.0);
        mix(hex(0x6a7486), hex(0xe8c228), smooth(0.12, 0.35, t))
    }

    pub fn jobs(jobs: &mut Vec<Job>) {
        let up = Vec3::Y;
        let mut torso_flow =
            Flow::new(v3(0.0, 0.096, -0.068), v3(0.0, 0.066, 0.076), up, 0.045).tilt(0.15);
        // The front pole sits inside the head's neck ball, out of sight.
        torso_flow.start = 0.008;
        torso_flow.end = 0.14;
        let mut head_flow =
            Flow::new(v3(0.0, 0.1, -0.09), v3(0.0, 0.097, -0.042), up, 0.022).tilt(0.1);
        head_flow.start = 0.006;
        let specs = [
            Spec {
                piece: Piece::Body,
                f: torso,
                region: Region::aabb(
                    v3(-0.033, 0.028, -0.068),
                    v3(0.033, 0.106, 0.082),
                    Vec3::splat(0.0019),
                ),
                target: 2000,
                flow: torso_flow,
                pivot: R.body,
                mirror: None,
                variant: false,
                seam: None,
            },
            Spec {
                piece: Piece::Head,
                f: head,
                region: Region::aabb(
                    v3(-0.017, 0.078, -0.094),
                    v3(0.017, 0.117, -0.033),
                    Vec3::splat(0.0009),
                ),
                target: 1100,
                flow: head_flow,
                pivot: R.head,
                mirror: None,
                variant: false,
                seam: None,
            },
        ];
        for spec in specs {
            jobs.push(Box::new(move || spec.run(KIND)));
        }
        let pair = |w: u8, mesh: Mesh| {
            vec![
                Made::Mesh(format!("animals/bird/wing0_{w}"), mirrored(&mesh)),
                Made::Mesh(format!("animals/bird/wing1_{w}"), mesh),
            ]
        };
        jobs.push(Box::new(move || pair(2, folded())));
        jobs.push(Box::new(move || pair(0, spread_arm())));
        jobs.push(Box::new(move || pair(1, spread_hand())));
        jobs.push(Box::new(|| {
            vec![
                Made::Mesh("animals/bird/tail".into(), tail(0.016)),
                Made::Mesh("animals/bird/tailfan".into(), tail(0.06)),
            ]
        }));
        jobs.push(Box::new(|| {
            let leg = leg();
            vec![
                Made::Mesh("animals/bird/leg0_0".into(), mirrored(&leg)),
                Made::Mesh("animals/bird/leg1_0".into(), leg),
            ]
        }));
        // Eyes and bill.
        jobs.push(Box::new(|| {
            let axis = EYE_AXIS.normalize();
            let mut mesh = Mesh::default();
            for side in [-1.0f32, 1.0] {
                let c = v3(EYE.x * side, EYE.y, EYE.z);
                let ax = v3(axis.x * side, axis.y, axis.z);
                let rot = Quat::from_rotation_arc(Vec3::Y, ax);
                let g = ball(c, Vec3::splat(EYE_R), rot, 8, 12);
                let eye = g.finish(R.head, &body, KIND.scale, &|p, _, _| {
                    let t = (p - c).normalize().dot(ax);
                    mix(hex(0x2a1a10), hex(0x050302), smooth(0.55, 0.8, t))
                });
                mesh.merge(&eye);
            }
            let color = |p: Vec3, _: Vec3| bill_color(p);
            let paint = Paint {
                flow: Flow::new(BILL, BILL_TIP, Vec3::Y, 0.03),
                color: &color,
                body: &body,
                scale: KIND.scale,
                pivot: R.head,
                seam: None,
            };
            let region = Region::bone(BILL, BILL_TIP, 0.0075, 0.00035, 0.0006);
            mesh.merge(&sculpt(&bill, region, 360, &paint));
            vec![Made::Mesh("animals/bird/gloss".into(), mesh)]
        }));
    }
}

pub use boar::{FWD as BOAR_HEAD_FWD, UP as BOAR_HEAD_UP};
pub use deer::{FWD as DEER_HEAD_FWD, UP as DEER_HEAD_UP};

#[cfg(test)]
mod tests {
    use super::*;

    /// Deer, wolf and boar torsos bend at the loin: at rest the fore half's ball stays inside
    /// the croup over the whole height of the torso (a sloping back or a tuft of coat would
    /// otherwise break through it as a step and a crease), the croup swelling over it only a
    /// little; and the croup, bent either way a little further than the gallop bends it, stays
    /// inside the fore half (no lip where the halves meet).
    #[test]
    fn torso_halves_stay_joined_when_the_back_bends() {
        let cases: [(&str, Seam, SdfFn, f32); 3] = [
            ("deer", deer::SEAM, deer::torso, 0.2),
            ("wolf", wolf::SEAM, wolf::torso, 0.32),
            ("boar", boar::SEAM, boar::torso, 0.26),
        ];
        for (name, seam, torso, bend) in cases {
            let (mut ball, mut lip, mut swell) = (0.0f32, 0.0f32, 0.0f32);
            let mut top = 0;
            for ix in 0..40 {
                for iy in 0..200 {
                    for iz in 0..70 {
                        let p = seam.pivot
                            + v3(
                                ix as f32 * 0.005,
                                iy as f32 * 0.005 - 0.5,
                                iz as f32 * 0.005 - 0.12,
                            );
                        let f = seam.fore(torso, p);
                        let croup = seam.croup(torso, p);
                        if f < 0.0 {
                            top = top.max(iy);
                            if p.z > seam.pivot.z + SEAM_OVERLAP {
                                ball = ball.max(croup);
                            } else {
                                ball = ball.max(torso(p));
                            }
                        }
                        if p.z > seam.pivot.z && croup.abs() < 0.003 {
                            swell = swell.max(torso(p) - croup);
                        }
                        if p.z < seam.pivot.z {
                            for th in [-bend, bend] {
                                let r = seam.pivot + Quat::from_rotation_x(-th) * (p - seam.pivot);
                                if seam.croup(torso, r) < 0.0 {
                                    lip = lip.max(f);
                                }
                            }
                        }
                    }
                }
            }
            eprintln!("{name}: ball out {ball:.4} m, croup out {lip:.4} m, swell {swell:.4} m");
            assert!(
                top < 199,
                "{name}: the torso reaches past the probed height"
            );
            assert!(
                ball < 0.0005,
                "{name}: the ball stands {ball:.4} m out of the croup"
            );
            assert!(
                lip < 0.0025,
                "{name}: the croup stands {lip:.4} m out at the seam"
            );
            assert!(
                swell < SEAM_SWELL + SEAM_PROUD + 0.0005,
                "{name}: the croup swells {swell:.4} m"
            );
        }
    }

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

    #[test]
    fn boar_meshes_fit_the_budget() {
        let (meshes, secs) = build_all(Species::Boar);
        for (k, t) in &meshes {
            eprintln!("{k}: {t}");
        }
        let sow = triangles_of(Species::Boar, false, false, &meshes);
        let boar = triangles_of(Species::Boar, true, false, &meshes);
        let piglet = triangles_of(Species::Boar, false, true, &meshes);
        eprintln!("boar: sow {sow} boar {boar} piglet {piglet} triangles, {secs:.2} s");
        assert!(boar <= 42_000 && piglet <= 42_000, "{boar} {piglet}");
        assert!(piece_count(Species::Boar, true) <= 30);
        // Every piece of every build has its mesh.
        for (male, young) in [(false, false), (true, false), (false, true)] {
            for piece in piece_names(Species::Boar, male, young) {
                let key = mesh_key(Species::Boar, piece, male, young);
                assert!(meshes.iter().any(|(k, t)| *k == key && *t > 0), "{key}");
                // A piglet's coat is its own all over its head, jaw and body.
                if young
                    && matches!(
                        piece,
                        Piece::Body | Piece::Croup | Piece::Neck | Piece::Head | Piece::Jaw
                    )
                {
                    assert!(key.ends_with("_young"), "a piglet wears the adult {key}");
                }
            }
        }
        // A tusker's upper tusks are their own piece, apart from the jaw's lower ones.
        let tusker = piece_names(Species::Boar, true, false);
        assert!(tusker.contains(&Piece::Tusks) && tusker.contains(&Piece::UpperTusks));
        assert!(!piece_names(Species::Boar, false, false).contains(&Piece::UpperTusks));
    }
}
