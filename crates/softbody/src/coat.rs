//! Fur grown on animals other modules build: every entity with an `engine_world::FurCoat`
//! (anim's paddock dog and horse, npc's wildlife) gets a groom grown on its skin and drawn by
//! the shared strand renderer, on a child entity of Softbody's own.
//!
//! - **Roots** are spread over the skin by area and the coat's `amount` (bare noses, pads and
//!   hooves get none), at the coat's density up to [`MAX_STRANDS`] (fewer, wider hairs past
//!   that, the same cover). Colour and length are read at each root from the per-vertex maps;
//!   tips take `style.tip` times the root colour.
//! - **Guides**, about one per [`STRANDS_PER_GUIDE`] strands and evenly spread, grow along the
//!   coat's comb (lifted off the skin, the long hair of a mane or tail sagging with gravity).
//!   Each strand is the blend of the three guides around its root (the weights that rebuild
//!   the root from theirs), scaled to its own length, so it moves exactly as they do.
//! - **Motion**: on a skinned body the guides are skinned with the bones under their roots
//!   every frame (linear blend, from `grown_on` to the current `SkeletonPose`), written as the
//!   child's `HairGuidesPosed`; a body of rigid pieces carries its fur rigidly, without guides.
//!
//! Grooms are cached by the coat's shared arrays, so a pack of wolves built from one mesh grows
//! one groom. Building runs on a worker (`off_frame`).

use std::collections::HashMap;
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::off_frame::OffFrameCommands;
use engine_core::prelude::*;
use engine_render::{StrandFiber, StrandFollow, StrandGroom, StrandInstance};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{FurCoat, FurStyle, HairGuidesPosed};

use crate::strand_draw::{RootGrid, follow};

/// Drawn strands on one coat at most (a horse at the coat's 60 per cm² would be millions).
pub const MAX_STRANDS: usize = 200_000;
/// Strands per guide, on average.
pub const STRANDS_PER_GUIDE: usize = 40;
/// Guides on one coat at most (skinned every frame).
const MAX_GUIDES: usize = 3_000;
/// Hair longer than this (m) gets more points per strand (a mane, a tail, feathering).
const LONG: f32 = 0.08;

/// On an entity with a `FurCoat`: the child entities drawing it (the short coat, then the
/// long hair when it has any), and the coat they were grown for.
#[derive(Component, Debug, Clone)]
pub struct CoatDrawn {
    pub entities: Vec<Entity>,
    key: CoatKey,
}

/// On the child entity drawing a coat.
#[derive(Component, Debug, Clone)]
pub struct CoatStrands {
    groom: Arc<CoatGroom>,
}

/// On an entity whose coat's groom is being built.
#[derive(Component, Debug, Clone)]
pub struct CoatGrowing(CoatKey);

/// Which groom a coat wears: its shared arrays and version (the same for a pack built from one
/// mesh).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoatKey {
    positions: usize,
    colors: usize,
    lengths: usize,
    version: u32,
}

impl CoatKey {
    fn of(coat: &FurCoat) -> Self {
        Self {
            positions: Arc::as_ptr(&coat.positions) as *const u8 as usize,
            colors: Arc::as_ptr(&coat.color) as *const u8 as usize,
            lengths: Arc::as_ptr(&coat.length) as *const u8 as usize,
            version: coat.version,
        }
    }
}

/// A groom grown on a coat, and how to skin its guides.
#[derive(Debug)]
pub struct CoatGroom {
    pub handle: Handle<StrandGroom>,
    /// Points per guide.
    pub per: usize,
    /// Guide points at rest (model space at `grown_on`).
    pub rest: Vec<Vec3>,
    /// Per guide point: the bones skinning it and their weights.
    pub skin: Vec<([u16; 4], [f32; 4])>,
    pub long: bool,
    /// How long hair hangs (none for a short coat).
    pub drape: Option<Drape>,
}

/// Grooms grown so far, by coat.
#[derive(Resource, Default)]
pub struct CoatGrooms(HashMap<CoatKey, Arc<[Arc<CoatGroom>]>>);

/// The bones skinning a guide point and their weights.
pub type BoneWeights = ([u16; 4], [f32; 4]);

/// A groom built off the frame, before its asset exists.
pub struct GrownCoat {
    pub groom: StrandGroom,
    pub per: usize,
    /// Per guide point: the bones skinning it and their weights.
    pub skin: Vec<([u16; 4], [f32; 4])>,
    /// The long hair (mane, tail), not the short coat.
    pub long: bool,
    pub drape: Option<Drape>,
}

/// A point on the skin: its triangle and barycentric weights.
#[derive(Debug, Clone, Copy)]
struct OnSkin {
    tri: usize,
    bary: [f32; 3],
}

struct Skin<'a> {
    coat: &'a FurCoat,
    /// Per bone, the way the coat lies over it when the coat gives none (see [`bone_combs`]).
    bone_comb: Vec<Vec3>,
}

/// The skeleton a coat grows on: parents and names, for its default comb.
#[derive(Debug, Clone)]
pub struct Bones {
    pub parents: Arc<[Option<u16>]>,
    pub names: Arc<[String]>,
}

/// Per bone, the way a coat lies over it, from the skeleton as it runs: away from the head
/// (down the back toward the tail, down the neck), out along the legs and the tail, and a
/// little downward. `None`: from the front (−Z) to the back.
fn bone_combs(coat: &FurCoat, bones: Option<&Bones>) -> Vec<Vec3> {
    let fallback = Vec3::new(0.0, -0.6, 1.0).normalize();
    let Some(b) = bones else {
        return vec![fallback; coat.grown_on.len()];
    };
    let pos = |i: usize| coat.grown_on.get(i).map_or(Vec3::ZERO, |p| p.translation);
    let root = (0..b.parents.len())
        .find(|i| b.parents[*i].is_none())
        .unwrap_or(0);
    let head = b
        .names
        .iter()
        .position(|n| n.to_lowercase().contains("head"));
    let forward = head
        .map(|h| pos(h) - pos(root))
        .filter(|f| f.length_squared() > 1e-6)
        .map_or(Vec3::NEG_Z, Vec3::normalize);
    (0..coat.grown_on.len())
        .map(|i| {
            let along = match b.parents.get(i).copied().flatten() {
                Some(p) => pos(i) - pos(p as usize),
                None => -forward,
            };
            let mut d = along.normalize_or(-forward);
            // The coat lies back from the head: a bone running toward it (the spine, the neck)
            // is combed the other way.
            if d.dot(forward) > 0.0 {
                d = -d;
            }
            (d + Vec3::NEG_Y * 0.3).normalize_or(fallback)
        })
        .collect()
}

impl Skin<'_> {
    fn corners(&self, tri: usize) -> [usize; 3] {
        let i = &self.coat.indices[tri * 3..tri * 3 + 3];
        [i[0] as usize, i[1] as usize, i[2] as usize]
    }

    fn lerp3(&self, at: OnSkin, f: impl Fn(usize) -> Vec3) -> Vec3 {
        let c = self.corners(at.tri);
        f(c[0]) * at.bary[0] + f(c[1]) * at.bary[1] + f(c[2]) * at.bary[2]
    }

    fn lerp1(&self, at: OnSkin, f: impl Fn(usize) -> f32) -> f32 {
        let c = self.corners(at.tri);
        f(c[0]) * at.bary[0] + f(c[1]) * at.bary[1] + f(c[2]) * at.bary[2]
    }

    fn position(&self, at: OnSkin) -> Vec3 {
        self.lerp3(at, |v| self.coat.positions[v])
    }

    fn normal(&self, at: OnSkin) -> Vec3 {
        self.lerp3(at, |v| self.coat.normals.get(v).copied().unwrap_or(Vec3::Y))
            .normalize_or(Vec3::Y)
    }

    fn amount(&self, v: usize) -> f32 {
        self.coat
            .amount
            .get(v)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }

    /// Hair length at vertex `v`, meters.
    fn length_at(&self, v: usize) -> f32 {
        self.coat.length.get(v).copied().unwrap_or(1.0).max(0.0) * self.coat.style.length
    }

    fn length(&self, at: OnSkin) -> f32 {
        self.lerp1(at, |v| self.coat.length.get(v).copied().unwrap_or(1.0))
            .max(0.0)
            * self.coat.style.length
    }

    fn color(&self, at: OnSkin) -> Vec3 {
        self.lerp3(at, |v| {
            Vec3::from(self.coat.color.get(v).copied().unwrap_or([0.3, 0.2, 0.12]))
        })
    }

    /// The way the coat lies here, along the skin: the coat's comb, or the bones' under it
    /// where it gives none (no comb, or a zero direction at these corners).
    fn comb(&self, at: OnSkin, n: Vec3) -> Vec3 {
        let given = if self.coat.comb.is_empty() {
            Vec3::ZERO
        } else {
            self.lerp3(at, |v| self.coat.comb.get(v).copied().unwrap_or(Vec3::ZERO))
        };
        let c = if given.length_squared() > 1e-4 {
            given
        } else {
            let fallback = Vec3::new(0.0, -0.6, 1.0);
            if self.coat.joints.is_empty() {
                fallback
            } else {
                let (j, w) = self.bones(at);
                j.iter()
                    .zip(w)
                    .map(|(b, w)| self.bone_comb.get(*b as usize).copied().unwrap_or(fallback) * w)
                    .sum::<Vec3>()
            }
        };
        let along = c - n * c.dot(n);
        along.normalize_or(n.any_orthonormal_vector())
    }

    /// The bones under a point: the corners' weights summed, the four heaviest kept.
    fn bones(&self, at: OnSkin) -> ([u16; 4], [f32; 4]) {
        if self.coat.joints.is_empty() {
            return ([0; 4], [0.0; 4]);
        }
        let mut acc: Vec<(u16, f32)> = Vec::with_capacity(12);
        for (k, v) in self.corners(at.tri).into_iter().enumerate() {
            let (Some(j), Some(w)) = (self.coat.joints.get(v), self.coat.weights.get(v)) else {
                continue;
            };
            for (b, bw) in j.iter().zip(w) {
                let bw = bw * at.bary[k];
                if bw <= 0.0 {
                    continue;
                }
                match acc.iter_mut().find(|(x, _)| x == b) {
                    Some(e) => e.1 += bw,
                    None => acc.push((*b, bw)),
                }
            }
        }
        acc.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut joints = [0u16; 4];
        let mut weights = [0.0f32; 4];
        let sum: f32 = acc.iter().take(4).map(|a| a.1).sum();
        for (k, (b, w)) in acc.into_iter().take(4).enumerate() {
            joints[k] = b;
            weights[k] = w / sum.max(1e-6);
        }
        (joints, weights)
    }
}

/// Grow the grooms on `coat` (model space at `grown_on`): the short coat, then the long hair
/// (a mane, a tail, feathering) when it has any, each its own groom (long hair takes more
/// points per strand and its own share of strands). Parts without hair are left out.
pub fn grow(coat: &FurCoat, bones: Option<&Bones>) -> Vec<GrownCoat> {
    let skin = Skin {
        coat,
        bone_comb: bone_combs(coat, bones),
    };
    let chains = bones.map(|b| Chains::new(coat, b));
    [false, true]
        .into_iter()
        .map(|long| grow_part(&skin, chains.as_ref(), long))
        .filter(|g| g.groom.strand_count() > 0)
        .collect()
}

/// Bone chains a long hair hangs along: each bone's single child, its child's, and so on (a
/// tail, a neck, a leg), for skinning the points of a guide by the bones they lie by.
struct Chains {
    /// Per bone: the chain from it down (itself first) and each chain bone's segment.
    chains: Vec<Vec<(u16, Vec3, Vec3)>>,
}

impl Chains {
    fn new(coat: &FurCoat, b: &Bones) -> Self {
        let n = coat.grown_on.len();
        let pos = |i: usize| coat.grown_on.get(i).map_or(Vec3::ZERO, |p| p.translation);
        let mut children: Vec<Vec<u16>> = vec![Vec::new(); n];
        for (i, p) in b.parents.iter().enumerate().take(n) {
            if let Some(p) = p
                && (*p as usize) < n
            {
                children[*p as usize].push(i as u16);
            }
        }
        let chains = (0..n)
            .map(|start| {
                let mut chain = Vec::new();
                let mut at = start;
                let mut last_dir = Vec3::ZERO;
                for _ in 0..12 {
                    let a = pos(at);
                    let next = (children[at].len() == 1).then(|| children[at][0] as usize);
                    let end = match next {
                        Some(c) => pos(c),
                        // The last bone runs on as far as the one before it.
                        None => a + last_dir,
                    };
                    chain.push((at as u16, a, end));
                    last_dir = end - a;
                    match next {
                        Some(c) => at = c,
                        None => break,
                    }
                }
                chain
            })
            .collect();
        Self { chains }
    }

    /// The bone of `root`'s chain that `p` lies by (`None`: `root` itself).
    fn bone_by(&self, root: u16, p: Vec3) -> Option<u16> {
        let chain = self.chains.get(root as usize)?;
        let (best, _) = chain
            .iter()
            .map(|(b, a, e)| {
                let d = *e - *a;
                let t = if d.length_squared() > 1e-10 {
                    ((p - *a).dot(d) / d.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (*b, p.distance(*a + d * t))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))?;
        (best != root).then_some(best)
    }
}

/// Strands of long hair on one coat at most.
pub const MAX_LONG_STRANDS: usize = 60_000;

fn grow_part(skin: &Skin, chains: Option<&Chains>, long: bool) -> GrownCoat {
    let coat = skin.coat;
    let style = coat.style;
    let tris = coat.indices.len() / 3;
    // Furred area per triangle of this part (m², times its mean amount).
    let mut cdf = Vec::with_capacity(tris);
    let mut total = 0.0f64;
    for t in 0..tris {
        let [a, b, c] = skin.corners(t);
        let mean_len = (skin.length_at(a) + skin.length_at(b) + skin.length_at(c)) / 3.0;
        let area = if (mean_len > LONG) == long {
            let (pa, pb, pc) = (
                coat.positions.get(a).copied().unwrap_or_default(),
                coat.positions.get(b).copied().unwrap_or_default(),
                coat.positions.get(c).copied().unwrap_or_default(),
            );
            let amount = (skin.amount(a) + skin.amount(b) + skin.amount(c)) / 3.0;
            0.5 * (pb - pa).cross(pc - pa).length() * amount
        } else {
            0.0
        };
        total += area as f64;
        cdf.push(total);
    }
    let per = if long { 8 } else { 4 };
    let wanted = (total * 1e4 * style.density.max(0.0) as f64) as usize;
    let strands = wanted.min(if long { MAX_LONG_STRANDS } else { MAX_STRANDS });
    let mut out = GrownCoat {
        groom: StrandGroom {
            fiber: StrandFiber::Fur,
            ..Default::default()
        },
        per,
        skin: Vec::new(),
        long,
        drape: None,
    };
    if strands == 0 || total <= 0.0 {
        return out;
    }
    // Fewer hairs than the coat asks for: each as much wider, the same cover.
    let radius = style.radius * (wanted as f32 / strands as f32).sqrt();
    let mut rng = Rng::new(0xF0C0A7 ^ coat.positions.len() as u64 ^ u64::from(long));
    let sample = |rng: &mut Rng| -> OnSkin {
        let x = rng.f32() as f64 * total;
        let tri = cdf.partition_point(|c| *c < x).min(tris - 1);
        let (mut u, mut v) = (rng.f32(), rng.f32());
        if u + v > 1.0 {
            (u, v) = (1.0 - u, 1.0 - v);
        }
        OnSkin {
            tri,
            bary: [1.0 - u - v, u, v],
        }
    };
    // Guides: evenly spread roots (candidates thinned to a minimum spacing).
    let guides_wanted = (strands / STRANDS_PER_GUIDE).clamp(16, MAX_GUIDES);
    let spacing = ((total as f32) / guides_wanted as f32).sqrt() * 0.85;
    let mut taken: HashMap<IVec3, Vec<Vec3>> = HashMap::new();
    let mut roots: Vec<OnSkin> = Vec::with_capacity(guides_wanted);
    for _ in 0..guides_wanted * 6 {
        if roots.len() >= guides_wanted {
            break;
        }
        let at = sample(&mut rng);
        let p = skin.position(at);
        let cell = (p / spacing).floor().as_ivec3();
        let near = (-1..=1).any(|dz| {
            (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    taken
                        .get(&(cell + IVec3::new(dx, dy, dz)))
                        .is_some_and(|l| l.iter().any(|q| q.distance(p) < spacing))
                })
            })
        });
        if !near {
            taken.entry(cell).or_default().push(p);
            roots.push(at);
        }
    }
    // Each guide's shape: leaving the skin along the comb, lying down toward the tip. A hair
    // holds its shape less the longer it is (bending stiffness falls with the cube of the
    // length), so long hair lifts less and sags with gravity.
    let mut rest: Vec<Vec3> = Vec::with_capacity(roots.len() * per);
    let mut normals = Vec::with_capacity(roots.len());
    let mut hangs = Vec::with_capacity(roots.len());
    for at in &roots {
        let p0 = skin.position(*at);
        let n = skin.normal(*at);
        let along = skin.comb(*at, n);
        let len = skin.length(*at).max(0.002);
        let hold = style.stiffness.clamp(0.0, 1.0) * (LONG / len).min(1.0).powi(2);
        let lift = 1.0 - 0.63 * hold;
        let sag = (1.0 - hold) * (len / 0.15).clamp(0.0, 1.0);
        hangs.push(sag * 0.7);
        let combed = (n * lift + along).normalize_or(n);
        let mut p = p0;
        rest.push(p);
        for k in 1..per {
            let f = k as f32 / (per - 1) as f32;
            // Lies down toward the comb, and down (−Y) where long and soft.
            let lie = (combed.lerp(along, 0.6 * f)).normalize_or(combed);
            let dir = lie.lerp(Vec3::NEG_Y, sag * f).normalize_or(lie);
            p += dir * len / (per - 1) as f32;
            // Never into the skin under the root.
            let below = (p - p0).dot(n);
            let clear = 0.002 * k as f32;
            if below < clear {
                p += n * (clear - below);
            }
            rest.push(p);
        }
        normals.push(n);
        // Skinned by the bones under the root; a long hair's points further out by the bones
        // of the chain they hang along (a tail's hair swings with the tail).
        let under = skin.bones(*at);
        let start = rest.len() - per;
        for (k, p) in rest[start..].iter().enumerate() {
            let by = (long && k > 0)
                .then(|| chains.and_then(|c| c.bone_by(under.0[0], *p)))
                .flatten();
            out.skin.push(match by {
                Some(b) => ([b, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
                None => under,
            });
        }
    }
    let guide_root = |g: u32| rest[g as usize * per];
    let grid = RootGrid::new(
        (0..roots.len() as u32).map(guide_root),
        (spacing * 2.0).max(0.004),
    );
    let groom = &mut out.groom;
    groom.points_per = per as u32;
    groom.points.reserve(strands * per);
    groom.roots.reserve(strands);
    groom.tips.reserve(strands);
    groom.follow.reserve(strands);
    let tip = Vec3::from(style.tip);
    let mut shape = vec![Vec3::ZERO; per];
    for _ in 0..strands {
        let at = sample(&mut rng);
        let root = skin.position(at);
        let n = skin.normal(at);
        let near = grid.nearest3(root, guide_root, |g| normals[g as usize].dot(n) > 0.25);
        let f: StrandFollow = follow(near, root, guide_root);
        // Its shape: the guides' blended, scaled to its own length.
        let len = skin.length(at) * (1.0 - 0.3 * rng.f32());
        shape.fill(Vec3::ZERO);
        for (g, w) in f.guides.iter().zip(f.weights) {
            if *g == u32::MAX {
                continue;
            }
            let base = *g as usize * per;
            for (j, sp) in shape.iter_mut().enumerate() {
                *sp += (rest[base + j] - rest[base]) * w;
            }
        }
        let blended: f32 = shape.windows(2).map(|w| w[0].distance(w[1])).sum();
        let scale = if blended > 1e-6 { len / blended } else { 0.0 };
        let phase = rng.f32() * std::f32::consts::TAU;
        let side = n
            .cross(shape[per - 1].normalize_or(n))
            .normalize_or(n.any_orthonormal_vector());
        for (j, sp) in shape.iter().enumerate() {
            let t = j as f32 / (per - 1) as f32;
            let curl = side * style.curl * (phase + t * 9.0).sin() * t;
            let p = root + *sp * scale + curl;
            // Long hair tapers only near the tip.
            let taper = if long {
                1.0 - 0.85 * ((t - 0.6) / 0.4).clamp(0.0, 1.0)
            } else {
                1.0 - 0.85 * t
            };
            groom.points.push([p.x, p.y, p.z, radius * taper]);
        }
        let c = skin.color(at) * (0.85 + 0.3 * rng.f32());
        let rand = rng.f32();
        groom.roots.push([c.x, c.y, c.z, rand]);
        let t = (c * tip).min(Vec3::splat(0.95));
        groom.tips.push([t.x, t.y, t.z, 0.45]);
        groom.follow.push(f);
    }
    groom.guides = rest.iter().map(|p| p.to_array()).collect();
    if long && !coat.joints.is_empty() {
        out.drape = Some(Drape::new(coat, &rest, per, hangs));
    }
    if coat.joints.is_empty() {
        // Rigid: no guides to move it.
        groom.guides.clear();
        groom.follow.clear();
        out.skin.clear();
    }
    out
}

/// A part of the body long hair can't pass through: a capsule riding one bone (model space at
/// `grown_on`), fitted to the skin that bone carries.
#[derive(Debug, Clone, Copy)]
pub struct Capsule {
    pub bone: u16,
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}

/// How long hair hangs: each guide falls toward the ground as far as it is soft, kept out of the
/// body's capsules, whatever pose the body is in (a grazing horse's mane hangs down, not along
/// its neck toward its ears).
#[derive(Debug, Clone, Default)]
pub struct Drape {
    pub capsules: Vec<Capsule>,
    /// Per guide: how far it gives to gravity (0..1), and the capsules within its reach.
    pub hang: Vec<f32>,
    pub near: Vec<Vec<u16>>,
}

/// Capsules fitted to the skin each bone carries most: along the skin's longest extent, as
/// thick as the thinner part of it (so hair roots stay outside).
pub fn body_capsules(coat: &FurCoat) -> Vec<Capsule> {
    let bones = coat.grown_on.len();
    let mut by_bone: Vec<Vec<Vec3>> = vec![Vec::new(); bones];
    for (v, (j, w)) in coat.joints.iter().zip(coat.weights.iter()).enumerate() {
        let k = (0..4).max_by(|a, b| w[*a].total_cmp(&w[*b])).unwrap_or(0);
        if let (Some(list), Some(p)) = (by_bone.get_mut(j[k] as usize), coat.positions.get(v)) {
            list.push(*p);
        }
    }
    let mut out = Vec::new();
    for (bone, pts) in by_bone.iter().enumerate() {
        if pts.len() < 24 {
            continue;
        }
        let c = pts.iter().sum::<Vec3>() / pts.len() as f32;
        // Principal axis by power iteration on the covariance.
        let mut cov = Mat3::ZERO;
        for p in pts {
            let d = *p - c;
            cov += Mat3::from_cols(d * d.x, d * d.y, d * d.z);
        }
        let mut axis = Vec3::new(0.3, 0.5, 0.8);
        for _ in 0..16 {
            axis = (cov * axis).normalize_or(Vec3::Y);
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut across: Vec<f32> = Vec::with_capacity(pts.len());
        for p in pts {
            let t = (*p - c).dot(axis);
            lo = lo.min(t);
            hi = hi.max(t);
            across.push((*p - c - axis * t).length());
        }
        across.sort_by(f32::total_cmp);
        let radius = across[across.len() * 3 / 10] * 0.85;
        if radius < 0.005 {
            continue;
        }
        // The ends rounded off inside the skin.
        let (lo, hi) = (lo + radius * 0.8, hi - radius * 0.8);
        let (lo, hi) = if lo < hi { (lo, hi) } else { (0.0, 0.0) };
        out.push(Capsule {
            bone: bone as u16,
            a: c + axis * lo,
            b: c + axis * hi,
            radius,
        });
    }
    out
}

fn closest_on(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let d = b - a;
    let t = if d.length_squared() > 1e-12 {
        ((p - a).dot(d) / d.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    a + d * t
}

impl Drape {
    /// The drape of guides at `rest` (`per` points each) over `coat`'s body; `hang` per guide.
    pub fn new(coat: &FurCoat, rest: &[Vec3], per: usize, hang: Vec<f32>) -> Self {
        let capsules = body_capsules(coat);
        let near = rest
            .chunks(per)
            .map(|g| {
                let len: f32 = g.windows(2).map(|w| w[0].distance(w[1])).sum();
                capsules
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        g[0].distance(closest_on(c.a, c.b, g[0])) < len * 1.5 + c.radius + 0.02
                    })
                    .map(|(i, _)| i as u16)
                    .collect()
            })
            .collect();
        Self {
            capsules,
            hang,
            near,
        }
    }

    /// Hang skinned guides `out` (`per` points each; `rest` gives the segment lengths) toward
    /// `down` (model space), out of the capsules posed by `mats`.
    fn apply(&self, rest: &[Vec3], per: usize, mats: &[Mat4], down: Vec3, out: &mut [Vec3]) {
        let posed: Vec<(Vec3, Vec3, f32)> = self
            .capsules
            .iter()
            .map(|c| match mats.get(c.bone as usize) {
                Some(m) => (m.transform_point3(c.a), m.transform_point3(c.b), c.radius),
                None => (c.a, c.b, c.radius),
            })
            .collect();
        for (g, pts) in out.chunks_mut(per).enumerate() {
            let hang = self.hang.get(g).copied().unwrap_or(0.0);
            let near = self.near.get(g).map_or(&[][..], Vec::as_slice);
            let mut prev = pts[0];
            for k in 1..per {
                let seg = rest[g * per + k].distance(rest[g * per + k - 1]);
                let f = k as f32 / (per - 1) as f32;
                let skinned = (pts[k] - pts[k - 1]).normalize_or(down);
                let dir = skinned.lerp(down, hang * f).normalize_or(skinned);
                let mut p = prev + dir * seg;
                // The first point stays with the root's skin; beyond it, out of the body.
                if k >= 2 {
                    // Out of every capsule near it, keeping the segment's length where that
                    // allows (the last pass leaves it outside, a little stretched if need be).
                    for pass in 0..5 {
                        for c in near {
                            let (a, b, r) = posed[*c as usize];
                            let q = closest_on(a, b, p);
                            let d = p - q;
                            if d.length_squared() < r * r {
                                p = q + d.normalize_or(Vec3::Y) * r;
                            }
                        }
                        if pass < 2 {
                            p = prev + (p - prev).normalize_or(dir) * seg;
                        }
                    }
                }
                pts[k] = p;
                prev = p;
            }
        }
    }
}

/// Skin `rest` guide points with the pose: each point by its bones (the bones under the
/// guide's root, or for long hair the bones it hangs along); long hair then hangs toward `down`
/// (model space) out of the body.
pub fn skin_guides(
    groom: &CoatGroom,
    grown_on: &[BonePose],
    now: &[BonePose],
    down: Vec3,
    out: &mut Vec<Vec3>,
) {
    let mats: Vec<Mat4> = grown_on
        .iter()
        .zip(now)
        .map(|(rest, now)| {
            let a = Mat4::from_scale_rotation_translation(now.scale, now.rotation, now.translation);
            let b =
                Mat4::from_scale_rotation_translation(rest.scale, rest.rotation, rest.translation);
            a * b.inverse()
        })
        .collect();
    out.clear();
    let mut last: Option<(BoneWeights, Mat4)> = None;
    for (p, bones) in groom.rest.iter().zip(&groom.skin) {
        let m = match last {
            Some((b, m)) if b == *bones => m,
            _ => {
                let mut m = Mat4::ZERO;
                for (j, w) in bones.0.iter().zip(bones.1) {
                    if w > 0.0
                        && let Some(b) = mats.get(*j as usize)
                    {
                        m += *b * w;
                    }
                }
                last = Some((*bones, m));
                m
            }
        };
        out.push(if m == Mat4::ZERO {
            *p
        } else {
            m.transform_point3(*p)
        });
    }
    if let Some(drape) = &groom.drape
        && groom.per >= 2
    {
        drape.apply(&groom.rest, groom.per, &mats, down, out);
    }
}

/// Grow grooms for new coats (once per shared coat), give each coat a drawing child, skin the
/// guides of skinned coats every frame, and drop drawings whose coat is gone or changed.
#[allow(clippy::type_complexity)]
pub fn draw_coats(
    mut commands: Commands,
    coats: Query<(
        Entity,
        &FurCoat,
        Option<&CoatDrawn>,
        Option<&CoatGrowing>,
        Option<&SkeletonPose>,
        Option<&ChildOf>,
        Option<&GlobalTransform>,
    )>,
    mut drawn: Query<(Entity, &CoatStrands, &ChildOf, &mut HairGuidesPosed)>,
    cache: Res<CoatGrooms>,
) {
    for (entity, coat, shown, growing, pose, parent, gt) in &coats {
        let down = down_in(gt);
        // A rigid piece of a body: drawn with its body's other pieces (`draw_piece_coats`).
        if coat.joints.is_empty() && parent.is_some() {
            continue;
        }
        // A skinned coat without the bones it was built on can't move: not drawn.
        if !coat.joints.is_empty() && coat.grown_on.is_empty() {
            continue;
        }
        let key = CoatKey::of(coat);
        if let Some(d) = shown {
            if d.key == key {
                continue;
            }
            for e in &d.entities {
                commands.entity(*e).despawn();
            }
            commands.entity(entity).remove::<CoatDrawn>();
        }
        if let Some(parts) = cache.0.get(&key) {
            // Posed from the first frame it draws (the renderer takes rest guides otherwise).
            let entities = parts
                .iter()
                .map(|g| {
                    let mut posed = Vec::new();
                    if let Some(p) = pose.filter(|p| p.bones.len() == coat.grown_on.len()) {
                        skin_guides(g, &coat.grown_on, &p.bones, down, &mut posed);
                    }
                    spawn_child(&mut commands, entity, g.clone(), posed)
                })
                .collect();
            commands
                .entity(entity)
                .remove::<CoatGrowing>()
                .insert(CoatDrawn { entities, key });
            continue;
        }
        if growing.is_some_and(|g| g.0 == key) {
            continue;
        }
        commands.entity(entity).insert(CoatGrowing(key));
        // Another body wearing the same coat may be growing it already: it lands in the cache
        // and this one picks it up there.
        if cache.0.contains_key(&key) {
            continue;
        }
        let coat = coat.clone();
        let bones = pose.map(|p| Bones {
            parents: p.parents.clone(),
            names: p.names.clone(),
        });
        commands.off_frame(
            "softbody coat",
            move || grow(&coat, bones.as_ref()),
            move |world: &mut World, grown: Vec<GrownCoat>| {
                let parts: Arc<[Arc<CoatGroom>]> = {
                    let mut assets =
                        world.get_resource_or_insert_with(Assets::<StrandGroom>::default);
                    grown
                        .into_iter()
                        .map(|g| {
                            let rest = g.groom.guides.iter().map(|p| Vec3::from(*p)).collect();
                            Arc::new(CoatGroom {
                                handle: assets.add(g.groom),
                                per: g.per,
                                rest,
                                skin: g.skin,
                                long: g.long,
                                drape: g.drape,
                            })
                        })
                        .collect()
                };
                world
                    .get_resource_or_insert_with(CoatGrooms::default)
                    .0
                    .insert(key, parts);
                if let Ok(mut e) = world.get_entity_mut(entity) {
                    e.remove::<CoatGrowing>();
                }
            },
        );
    }
    for (e, strands, parent, mut posed) in &mut drawn {
        let Ok((_, coat, Some(d), _, pose, _, gt)) = coats.get(parent.parent()) else {
            commands.entity(e).despawn();
            continue;
        };
        if !d.entities.contains(&e) {
            commands.entity(e).despawn();
            continue;
        }
        let (Some(pose), false) = (pose, strands.groom.rest.is_empty()) else {
            continue;
        };
        if pose.bones.len() != coat.grown_on.len() {
            continue;
        }
        skin_guides(
            &strands.groom,
            &coat.grown_on,
            &pose.bones,
            down_in(gt),
            &mut posed.points,
        );
    }
}

/// World down in the model space of a body placed at `gt`.
fn down_in(gt: Option<&GlobalTransform>) -> Vec3 {
    gt.map_or(Vec3::NEG_Y, |g| {
        (g.rotation().inverse() * Vec3::NEG_Y).normalize_or(Vec3::NEG_Y)
    })
}

fn spawn_child(
    commands: &mut Commands,
    body: Entity,
    groom: Arc<CoatGroom>,
    posed: Vec<Vec3>,
) -> Entity {
    let handle = groom.handle;
    commands
        .spawn((
            Name::new(if groom.long { "Long hair" } else { "Fur" }),
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(body),
            StrandInstance::new(handle),
            HairGuidesPosed {
                version: 0,
                points: posed,
            },
            CoatStrands { groom },
        ))
        .id()
}

// ======================================================================= bodies of rigid pieces

/// On a body made of rigid pieces wearing coats (its children, each with a `FurCoat` and no
/// bones): the children drawing all of them as one (the short coat, then the long hair), the
/// pieces in the order grown, and the coats they were grown for.
#[derive(Component, Debug, Clone)]
pub struct PiecesDrawn {
    pub entities: Vec<Entity>,
    pub pieces: Vec<Entity>,
    key: Arc<[CoatKey]>,
}

/// On a body of pieces whose groom is being built.
#[derive(Component, Debug, Clone)]
pub struct PiecesGrowing(Arc<[CoatKey]>);

/// On the child entity drawing a body of pieces.
#[derive(Component, Debug, Clone)]
pub struct PieceCoatStrands {
    groom: Arc<CoatGroom>,
    /// Each piece's transform in the body when the groom was grown.
    grown_on: Arc<[BonePose]>,
}

/// Grooms grown on bodies of pieces so far, by their pieces' coats (a herd built from the
/// same meshes shares one).
#[derive(Resource, Default)]
pub struct PieceGrooms(HashMap<Arc<[CoatKey]>, PiecesGrown>);

/// A body of pieces' grooms (short coat, long hair), and the pieces' places when grown.
type PiecesGrown = (Arc<[Arc<CoatGroom>]>, Arc<[BonePose]>);

fn bone_pose(t: &Transform) -> BonePose {
    BonePose {
        translation: t.translation.as_vec3(),
        rotation: t.rotation,
        scale: t.scale,
    }
}

/// One coat for a body of rigid pieces: each piece's skin placed in the body as it stands
/// (`pieces`: coat and the piece's transform in the body), every piece a bone. Lengths and
/// densities are carried over relative to the first piece's style.
pub fn combine_pieces(pieces: &[(FurCoat, BonePose)]) -> FurCoat {
    let base = pieces.first().map(|p| p.0.style).unwrap_or_default();
    let density = pieces
        .iter()
        .map(|p| p.0.style.density)
        .fold(base.density, f32::max)
        .max(1e-3);
    let mut out = FurCoat {
        version: 1,
        style: FurStyle { density, ..base },
        ..Default::default()
    };
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    let (mut joints, mut weights, mut color, mut amount, mut length, mut comb) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let mut grown_on = Vec::with_capacity(pieces.len());
    for (k, (coat, at)) in pieces.iter().enumerate() {
        let first = positions.len() as u32;
        let m = Mat4::from_scale_rotation_translation(at.scale, at.rotation, at.translation);
        let n = coat.positions.len();
        positions.extend(coat.positions.iter().map(|p| m.transform_point3(*p)));
        normals.extend((0..n).map(|v| {
            let nv = coat.normals.get(v).copied().unwrap_or(Vec3::Y);
            (at.rotation * (nv / at.scale)).normalize_or(Vec3::Y)
        }));
        indices.extend(coat.indices.iter().map(|i| i + first));
        joints.extend(std::iter::repeat_n([k as u16, 0, 0, 0], n));
        weights.extend(std::iter::repeat_n([1.0f32, 0.0, 0.0, 0.0], n));
        let lengths = coat.style.length / base.length.max(1e-6);
        let thin = coat.style.density / density;
        for v in 0..n {
            color.push(coat.color.get(v).copied().unwrap_or([0.3, 0.2, 0.12]));
            amount.push(coat.amount.get(v).copied().unwrap_or(1.0) * thin);
            length.push(coat.length.get(v).copied().unwrap_or(1.0) * lengths);
            comb.push(
                coat.comb
                    .get(v)
                    .map_or(Vec3::ZERO, |c| at.rotation * (*c * at.scale)),
            );
        }
        grown_on.push(*at);
    }
    out.positions = positions.into();
    out.normals = normals.into();
    out.indices = indices.into();
    out.joints = joints.into();
    out.weights = weights.into();
    out.color = color.into();
    out.amount = amount.into();
    out.length = length.into();
    out.comb = comb.into();
    out.grown_on = grown_on.into();
    out
}

/// Bodies of rigid pieces: grow one groom on all of a body's coated pieces (once per set of
/// shared coats), drawn on one child of the body, its guides moved with the pieces every
/// frame; drop drawings whose pieces changed or went.
#[allow(clippy::type_complexity)]
pub fn draw_piece_coats(
    mut commands: Commands,
    pieces: Query<(Entity, &FurCoat, &ChildOf, &Transform)>,
    bodies: Query<(
        Option<&PiecesDrawn>,
        Option<&PiecesGrowing>,
        Option<&GlobalTransform>,
    )>,
    mut drawn: Query<(Entity, &PieceCoatStrands, &ChildOf, &mut HairGuidesPosed)>,
    cache: Res<PieceGrooms>,
) {
    // Each body's coated pieces, in a fixed order (by coat, then where they sit), so every
    // body built from the same meshes lines its pieces up the same way.
    let mut by_body: HashMap<Entity, Vec<(CoatKey, Entity, Transform)>> = HashMap::new();
    for (e, coat, parent, t) in &pieces {
        if coat.joints.is_empty() {
            by_body
                .entry(parent.parent())
                .or_default()
                .push((CoatKey::of(coat), e, *t));
        }
    }
    let mut bones: HashMap<Entity, Vec<BonePose>> = HashMap::new();
    for (body, mut list) in by_body {
        list.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.2.translation.x.total_cmp(&b.2.translation.x))
                .then(a.2.translation.z.total_cmp(&b.2.translation.z))
                .then(a.2.translation.y.total_cmp(&b.2.translation.y))
        });
        let Ok((shown, growing, gt)) = bodies.get(body) else {
            continue;
        };
        let order: Vec<Entity> = list.iter().map(|p| p.1).collect();
        let key: Arc<[CoatKey]> = list.iter().map(|p| p.0).collect();
        if let Some(d) = shown {
            if d.key == key && d.pieces == order {
                bones.insert(body, list.iter().map(|p| bone_pose(&p.2)).collect());
                continue;
            }
            for e in &d.entities {
                commands.entity(*e).despawn();
            }
            commands.entity(body).remove::<PiecesDrawn>();
        }
        if let Some((parts, grown_on)) = cache.0.get(&key) {
            let now: Vec<BonePose> = list.iter().map(|p| bone_pose(&p.2)).collect();
            let entities = parts
                .iter()
                .map(|g| {
                    let mut posed = Vec::new();
                    if now.len() == grown_on.len() {
                        skin_guides(g, grown_on, &now, down_in(gt), &mut posed);
                    }
                    commands
                        .spawn((
                            Name::new(if g.long { "Long hair" } else { "Fur" }),
                            Transform::default(),
                            GlobalTransform::default(),
                            ChildOf(body),
                            StrandInstance::new(g.handle),
                            HairGuidesPosed {
                                version: 0,
                                points: posed,
                            },
                            PieceCoatStrands {
                                groom: g.clone(),
                                grown_on: grown_on.clone(),
                            },
                        ))
                        .id()
                })
                .collect();
            commands
                .entity(body)
                .remove::<PiecesGrowing>()
                .insert(PiecesDrawn {
                    entities,
                    pieces: order,
                    key,
                });
            continue;
        }
        if growing.is_some_and(|g| g.0 == key) {
            continue;
        }
        commands.entity(body).insert(PiecesGrowing(key.clone()));
        let input: Vec<(FurCoat, BonePose)> = list
            .iter()
            .filter_map(|(_, e, t)| pieces.get(*e).ok().map(|p| (p.1.clone(), bone_pose(t))))
            .collect();
        commands.off_frame(
            "softbody piece coats",
            move || {
                let coat = combine_pieces(&input);
                let grown = grow(&coat, None);
                (grown, coat.grown_on)
            },
            move |world: &mut World, (grown, grown_on): (Vec<GrownCoat>, Arc<[BonePose]>)| {
                let parts: Arc<[Arc<CoatGroom>]> = {
                    let mut assets =
                        world.get_resource_or_insert_with(Assets::<StrandGroom>::default);
                    grown
                        .into_iter()
                        .map(|g| {
                            let rest = g.groom.guides.iter().map(|p| Vec3::from(*p)).collect();
                            Arc::new(CoatGroom {
                                handle: assets.add(g.groom),
                                per: g.per,
                                rest,
                                skin: g.skin,
                                long: g.long,
                                drape: g.drape,
                            })
                        })
                        .collect()
                };
                world
                    .get_resource_or_insert_with(PieceGrooms::default)
                    .0
                    .insert(key, (parts, grown_on));
                if let Ok(mut e) = world.get_entity_mut(body) {
                    e.remove::<PiecesGrowing>();
                }
            },
        );
    }
    for (e, strands, parent, mut posed) in &mut drawn {
        let body = parent.parent();
        let Some(now) = bones.get(&body) else {
            // Its body's pieces changed (being redrawn) or went.
            if !bodies
                .get(body)
                .ok()
                .and_then(|b| b.0)
                .is_some_and(|d| d.entities.contains(&e))
            {
                commands.entity(e).despawn();
            }
            continue;
        };
        if now.len() == strands.grown_on.len() {
            let down = down_in(bodies.get(body).ok().and_then(|b| b.2));
            skin_guides(
                &strands.groom,
                &strands.grown_on,
                now,
                down,
                &mut posed.points,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A furred tube along x (a leg or a tail), two bones: the near half on bone 0, the far on 1.
    fn tube() -> FurCoat {
        let (rings, sides) = (12, 16);
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut joints = Vec::new();
        let mut weights = Vec::new();
        for r in 0..rings {
            let x = r as f32 * 0.04;
            for s in 0..sides {
                let a = s as f32 / sides as f32 * std::f32::consts::TAU;
                let n = Vec3::new(0.0, a.cos(), a.sin());
                positions.push(Vec3::new(x, 0.0, 0.0) + n * 0.05);
                normals.push(n);
                joints.push([0u16, 1, 0, 0]);
                let w1 = (x / 0.44).clamp(0.0, 1.0);
                weights.push([1.0 - w1, w1, 0.0, 0.0]);
            }
        }
        let mut indices = Vec::new();
        for r in 0..rings - 1 {
            for s in 0..sides {
                let a = (r * sides + s) as u32;
                let b = (r * sides + (s + 1) % sides) as u32;
                let (c, d) = (a + sides as u32, b + sides as u32);
                indices.extend([a, c, b, b, c, d]);
            }
        }
        let n = positions.len();
        FurCoat {
            positions: positions.into(),
            normals: normals.into(),
            indices: indices.into(),
            joints: joints.into(),
            weights: weights.into(),
            grown_on: vec![BonePose::default(), BonePose::default()].into(),
            color: vec![[0.4, 0.25, 0.1]; n].into(),
            amount: vec![1.0; n].into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_coat_grows_a_valid_groom_at_its_density() {
        let coat = tube();
        let parts = grow(&coat, None);
        assert_eq!(parts.len(), 1, "a short coat only");
        let grown = &parts[0];
        let g = &grown.groom;
        assert!(g.is_valid());
        assert_eq!(g.fiber, StrandFiber::Fur);
        // 0.05 m radius, 0.44 m long: 1382 cm² at 60 per cm².
        assert!(
            (70_000..=MAX_STRANDS).contains(&g.strand_count()),
            "{}",
            g.strand_count()
        );
        assert!(g.guide_count() * STRANDS_PER_GUIDE * 2 > g.strand_count());
        assert_eq!(grown.skin.len(), g.guides.len());
        // Every root on the skin, every tip outside it.
        let per = g.points_per as usize;
        for s in (0..g.strand_count()).step_by(97) {
            let root = Vec3::from_slice(&g.points[s * per][..3]);
            let tip = Vec3::from_slice(&g.points[s * per + per - 1][..3]);
            let r = Vec2::new(root.y, root.z).length();
            assert!((r - 0.05).abs() < 0.003, "root at radius {r}");
            assert!(
                Vec2::new(tip.y, tip.z).length() > 0.051,
                "tip inside the skin"
            );
        }
    }

    /// A long-haired tail: the tube's far end grows 0.4 m hair that hangs along the chain.
    #[test]
    fn long_hair_is_its_own_groom_and_swings_with_its_chain() {
        let mut coat = tube();
        let lengths: Vec<f32> = coat
            .positions
            .iter()
            .map(|p| if p.x > 0.2 { 13.0 } else { 1.0 })
            .collect();
        coat.length = lengths.into();
        // Three bones in a chain along x; the far half of the skin rides bone 1.
        coat.grown_on = vec![
            BonePose::default(),
            BonePose {
                translation: Vec3::X * 0.22,
                ..Default::default()
            },
            BonePose {
                translation: Vec3::X * 0.6,
                ..Default::default()
            },
        ]
        .into();
        let bones = Bones {
            parents: vec![None, Some(0), Some(1)].into(),
            names: vec!["body".into(), "tail_1".into(), "tail_2".into()].into(),
        };
        let parts = grow(&coat, Some(&bones));
        assert_eq!(parts.len(), 2, "short coat and long hair");
        let (short, long) = (&parts[0], &parts[1]);
        assert!(!short.long && long.long);
        assert!(long.groom.is_valid() && long.per > short.per);
        assert!(long.groom.strand_count() <= MAX_LONG_STRANDS);
        // Long hair sags: its tips hang well below its roots.
        let per = long.per;
        let g = &long.groom;
        let mut dropped = 0.0;
        for s in 0..g.strand_count() {
            dropped += g.points[s * per][1] - g.points[s * per + per - 1][1];
        }
        assert!(dropped / g.strand_count() as f32 > 0.1, "hangs {dropped}");
        // Points out past bone 2's joint are skinned by bone 2.
        let far = long
            .groom
            .guides
            .iter()
            .zip(&long.skin)
            .filter(|(p, _)| p[0] > 0.7)
            .collect::<Vec<_>>();
        if !far.is_empty() {
            assert!(
                far.iter().all(|(_, s)| s.0[0] == 2),
                "by the chain's last bone"
            );
        }
    }

    /// Long hair hangs with gravity in any pose, outside the body: the tube's far half
    /// turned straight up, its hair still falls, and no guide point ends inside the tube.
    #[test]
    fn long_hair_hangs_down_and_stays_out_of_the_body() {
        let mut coat = tube();
        let lengths: Vec<f32> = coat
            .positions
            .iter()
            .map(|p| if p.x > 0.2 { 13.0 } else { 1.0 })
            .collect();
        coat.length = lengths.into();
        coat.grown_on = vec![
            BonePose::default(),
            BonePose {
                translation: Vec3::X * 0.22,
                ..Default::default()
            },
        ]
        .into();
        let bones = Bones {
            parents: vec![None, Some(0)].into(),
            names: vec!["body".into(), "tail".into()].into(),
        };
        let g = grow(&coat, Some(&bones)).remove(1);
        let drape = g.drape.clone().expect("long hair drapes");
        assert!(!drape.capsules.is_empty());
        let groom = CoatGroom {
            handle: Handle::from_index(0),
            per: g.per,
            rest: g.groom.guides.iter().map(|p| Vec3::from(*p)).collect(),
            skin: g.skin,
            long: true,
            drape: Some(drape.clone()),
        };
        let mut now = coat.grown_on.to_vec();
        now[1].rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mut out = Vec::new();
        skin_guides(&groom, &coat.grown_on, &now, Vec3::NEG_Y, &mut out);
        let per = groom.per;
        let mut falling = 0;
        for pts in out.chunks(per) {
            if pts[per - 1].y < pts[1].y {
                falling += 1;
            }
        }
        assert!(
            falling * 10 > out.len() / per * 7,
            "{falling} of {} guides hang",
            out.len() / per
        );
        // Out of the tube (radius 0.05 about its axis, now partly vertical): the drape's
        // capsules, posed, hold every point past the bent skin round the roots.
        let m = Mat4::from_rotation_translation(now[1].rotation, Vec3::X * 0.22)
            * Mat4::from_translation(-Vec3::X * 0.22);
        for (gi, pts) in out.chunks(per).enumerate() {
            for (k, p) in pts.iter().enumerate().skip(3) {
                for (ci, c) in drape.capsules.iter().enumerate() {
                    let (a, b) = if c.bone == 1 {
                        (m.transform_point3(c.a), m.transform_point3(c.b))
                    } else {
                        (c.a, c.b)
                    };
                    let d = p.distance(closest_on(a, b, *p));
                    assert!(
                        d > c.radius * 0.9,
                        "guide {gi} point {k}: {d} m from capsule {ci} {c:?} (near {:?})",
                        drape.near[gi]
                    );
                }
            }
        }
    }

    /// A body of two rigid pieces grows one groom whose guides ride each piece.
    #[test]
    fn rigid_pieces_grow_one_groom_that_moves_with_each_piece() {
        let mut piece = tube();
        piece.joints = Vec::new().into();
        piece.weights = Vec::new().into();
        piece.grown_on = Vec::new().into();
        let at = |y: f32| BonePose {
            translation: Vec3::Y * y,
            ..Default::default()
        };
        let coat = combine_pieces(&[(piece.clone(), at(0.0)), (piece, at(0.5))]);
        assert_eq!(coat.grown_on.len(), 2);
        let grown = grow(&coat, None).remove(0);
        assert!(grown.groom.is_valid() && !grown.groom.guides.is_empty());
        let groom = CoatGroom {
            handle: Handle::from_index(0),
            per: grown.per,
            rest: grown.groom.guides.iter().map(|p| Vec3::from(*p)).collect(),
            skin: grown.skin,
            long: false,
            drape: None,
        };
        // Roots on both pieces.
        let roots: Vec<f32> = groom.rest.iter().step_by(groom.per).map(|p| p.y).collect();
        assert!(roots.iter().any(|y| *y > 0.3) && roots.iter().any(|y| *y < 0.2));
        let mut now = coat.grown_on.to_vec();
        now[1].translation.y += 0.2;
        now[1].rotation = Quat::from_rotation_x(0.5);
        let mut out = Vec::new();
        skin_guides(&groom, &coat.grown_on, &now, Vec3::NEG_Y, &mut out);
        for (k, (p, r)) in out.iter().zip(&groom.rest).enumerate() {
            if groom.skin[k].0[0] == 0 {
                assert!(p.distance(*r) < 1e-5, "piece 0 stays");
            } else {
                let want = Quat::from_rotation_x(0.5) * (*r - Vec3::Y * 0.5) + Vec3::Y * 0.7;
                assert!(p.distance(want) < 1e-4, "piece 1 carries its guides");
            }
        }
    }

    #[test]
    fn bare_skin_grows_nothing() {
        let mut coat = tube();
        coat.amount = vec![0.0; coat.positions.len()].into();
        assert!(grow(&coat, None).is_empty());
    }

    #[test]
    fn guides_skin_with_the_bones_under_their_roots() {
        let coat = tube();
        let grown = grow(&coat, None).remove(0);
        let groom = CoatGroom {
            handle: Handle::from_index(0),
            per: grown.per,
            rest: grown.groom.guides.iter().map(|p| Vec3::from(*p)).collect(),
            skin: grown.skin,
            long: false,
            drape: None,
        };
        let mut pose = SkeletonPose {
            names: vec!["a".to_string(), "b".to_string()].into(),
            parents: vec![None, Some(0)].into(),
            bones: vec![BonePose::default(); 2],
        };
        let mut out = Vec::new();
        skin_guides(&groom, &coat.grown_on, &pose.bones, Vec3::NEG_Y, &mut out);
        let off = out
            .iter()
            .zip(&groom.rest)
            .map(|(a, b)| a.distance(*b))
            .fold(0.0f32, f32::max);
        assert!(off < 1e-5, "at rest, as grown: {off}");
        pose.bones[1].translation = Vec3::Y * 0.1;
        skin_guides(&groom, &coat.grown_on, &pose.bones, Vec3::NEG_Y, &mut out);
        // Guides on the far end ride bone 1 up; on the near end stay.
        for (g, (j, w)) in groom.skin.iter().enumerate().step_by(groom.per) {
            let rise = out[g].y - groom.rest[g].y;
            let w1: f32 = j
                .iter()
                .zip(w)
                .filter(|(b, _)| **b == 1)
                .map(|(_, w)| *w)
                .sum();
            assert!(
                (rise - 0.1 * w1).abs() < 1e-4,
                "guide {g}: {rise} for weight {w1}"
            );
        }
    }
}
