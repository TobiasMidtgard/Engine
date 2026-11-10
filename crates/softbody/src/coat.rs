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
use engine_world::animation::SkeletonPose;
use engine_world::{FurCoat, HairGuidesPosed};

use crate::strand_draw::{RootGrid, follow};

/// Drawn strands on one coat at most (a horse at the coat's 60 per cm² would be millions).
pub const MAX_STRANDS: usize = 200_000;
/// Strands per guide, on average.
pub const STRANDS_PER_GUIDE: usize = 40;
/// Guides on one coat at most (skinned every frame).
const MAX_GUIDES: usize = 3_000;
/// Hair longer than this (m) gets more points per strand (a mane, a tail, feathering).
const LONG: f32 = 0.08;

/// On an entity with a `FurCoat`: the child entity drawing it, and the coat it was grown for.
#[derive(Component, Debug, Clone)]
pub struct CoatDrawn {
    pub entity: Entity,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CoatKey {
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
    /// Per guide: the bones under its root and their weights.
    pub skin: Vec<([u16; 4], [f32; 4])>,
}

/// Grooms grown so far, by coat.
#[derive(Resource, Default)]
pub struct CoatGrooms(HashMap<CoatKey, Arc<CoatGroom>>);

/// A groom built off the frame, before its asset exists.
pub struct GrownCoat {
    pub groom: StrandGroom,
    pub per: usize,
    pub skin: Vec<([u16; 4], [f32; 4])>,
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

    /// The way the coat lies here, along the skin: the coat's comb, or the bones' under it.
    fn comb(&self, at: OnSkin, n: Vec3) -> Vec3 {
        let c = if self.coat.comb.is_empty() {
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
        } else {
            self.lerp3(at, |v| self.coat.comb.get(v).copied().unwrap_or(Vec3::Z))
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

/// Grow a groom on `coat` (model space at `grown_on`).
pub fn grow(coat: &FurCoat, bones: Option<&Bones>) -> GrownCoat {
    let skin = Skin {
        coat,
        bone_comb: bone_combs(coat, bones),
    };
    let style = coat.style;
    let tris = coat.indices.len() / 3;
    // Furred area per triangle (m², times its mean amount).
    let mut cdf = Vec::with_capacity(tris);
    let mut total = 0.0f64;
    for t in 0..tris {
        let [a, b, c] = skin.corners(t);
        let (pa, pb, pc) = (
            coat.positions.get(a).copied().unwrap_or_default(),
            coat.positions.get(b).copied().unwrap_or_default(),
            coat.positions.get(c).copied().unwrap_or_default(),
        );
        let area = 0.5 * (pb - pa).cross(pc - pa).length();
        let amount = (skin.amount(a) + skin.amount(b) + skin.amount(c)) / 3.0;
        total += (area * amount) as f64;
        cdf.push(total);
    }
    let wanted = (total * 1e4 * style.density.max(0.0) as f64) as usize;
    let strands = wanted.min(MAX_STRANDS);
    let mut out = GrownCoat {
        groom: StrandGroom {
            fiber: StrandFiber::Fur,
            ..Default::default()
        },
        per: 2,
        skin: Vec::new(),
    };
    if strands == 0 || total <= 0.0 {
        return out;
    }
    // Fewer hairs than the coat asks for: each as much wider, the same cover.
    let radius = style.radius * (wanted as f32 / strands as f32).sqrt();
    let mut rng = Rng::new(0xF0C0A7 ^ coat.positions.len() as u64);
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
    let long = (0..coat.positions.len())
        .map(|v| coat.length.get(v).copied().unwrap_or(1.0) * style.length)
        .fold(0.0f32, f32::max)
        > LONG;
    let per = if long { 6 } else { 4 };
    out.per = per;
    // Each guide's shape: leaving the skin along the comb, lying down toward the tip; long hair
    // sags with gravity.
    let mut rest: Vec<Vec3> = Vec::with_capacity(roots.len() * per);
    let mut normals = Vec::with_capacity(roots.len());
    let lift = 1.0 - 0.7 * style.stiffness.clamp(0.0, 1.0) * 0.9;
    for at in &roots {
        let p0 = skin.position(*at);
        let n = skin.normal(*at);
        let along = skin.comb(*at, n);
        let len = skin.length(*at).max(0.002);
        let sag = (1.0 - style.stiffness.clamp(0.0, 1.0)) * (len / 0.15).clamp(0.0, 1.0);
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
            if below < 0.002 * k as f32 {
                p += n * (0.002 * k as f32 - below);
            }
            rest.push(p);
        }
        normals.push(n);
        out.skin.push(skin.bones(*at));
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
    for s in 0..strands {
        let at = sample(&mut rng);
        let root = skin.position(at);
        let n = skin.normal(at);
        let near = grid.nearest3(root, guide_root, |g| normals[g as usize].dot(n) > 0.25);
        let f: StrandFollow = follow(near, root, guide_root);
        // Its shape: the guides' blended, scaled to its own length.
        let len = skin.length(at) * (1.0 - 0.3 * rng.f32());
        let mut shape = [Vec3::ZERO; 6];
        for (g, w) in f.guides.iter().zip(f.weights) {
            if *g == u32::MAX {
                continue;
            }
            let base = *g as usize * per;
            for (j, sp) in shape.iter_mut().enumerate().take(per) {
                *sp += (rest[base + j] - rest[base]) * w;
            }
        }
        let blended: f32 = shape[..per].windows(2).map(|w| w[0].distance(w[1])).sum();
        let scale = if blended > 1e-6 { len / blended } else { 0.0 };
        let phase = rng.f32() * std::f32::consts::TAU;
        let side = n
            .cross(shape[per - 1].normalize_or(n))
            .normalize_or(n.any_orthonormal_vector());
        for (j, sp) in shape.iter().enumerate().take(per) {
            let t = j as f32 / (per - 1) as f32;
            let curl = side * style.curl * (phase + t * 9.0).sin() * t;
            let p = root + *sp * scale + curl;
            groom
                .points
                .push([p.x, p.y, p.z, radius * (1.0 - 0.85 * t)]);
        }
        let c = skin.color(at) * (0.85 + 0.3 * rng.f32());
        let rand = rng.f32();
        groom.roots.push([c.x, c.y, c.z, rand]);
        let t = (c * tip).min(Vec3::splat(0.95));
        groom.tips.push([t.x, t.y, t.z, 0.45]);
        groom.follow.push(f);
        let _ = s;
    }
    groom.guides = rest.iter().map(|p| p.to_array()).collect();
    if coat.joints.is_empty() {
        // Rigid: no guides to move it.
        groom.guides.clear();
        groom.follow.clear();
    }
    out
}

/// Skin `rest` guide points with the pose: each guide by the bones under its root.
pub fn skin_guides(
    groom: &CoatGroom,
    grown_on: &[engine_world::animation::BonePose],
    pose: &SkeletonPose,
    out: &mut Vec<Vec3>,
) {
    let mats: Vec<Mat4> = grown_on
        .iter()
        .zip(&pose.bones)
        .map(|(rest, now)| {
            let a = Mat4::from_scale_rotation_translation(now.scale, now.rotation, now.translation);
            let b =
                Mat4::from_scale_rotation_translation(rest.scale, rest.rotation, rest.translation);
            a * b.inverse()
        })
        .collect();
    out.clear();
    for (g, (joints, weights)) in groom.skin.iter().enumerate() {
        let mut m = Mat4::ZERO;
        for (j, w) in joints.iter().zip(weights) {
            if *w > 0.0
                && let Some(b) = mats.get(*j as usize)
            {
                m += *b * *w;
            }
        }
        let pts = &groom.rest[g * groom.per..(g + 1) * groom.per];
        if m == Mat4::ZERO {
            out.extend_from_slice(pts);
        } else {
            out.extend(pts.iter().map(|p| m.transform_point3(*p)));
        }
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
    )>,
    mut drawn: Query<(Entity, &CoatStrands, &ChildOf, &mut HairGuidesPosed)>,
    cache: Res<CoatGrooms>,
) {
    for (entity, coat, shown, growing, pose) in &coats {
        let key = CoatKey::of(coat);
        if let Some(d) = shown {
            if d.key == key {
                continue;
            }
            commands.entity(d.entity).despawn();
            commands.entity(entity).remove::<CoatDrawn>();
        }
        if let Some(groom) = cache.0.get(&key) {
            let child = spawn_child(&mut commands, entity, groom.clone());
            commands
                .entity(entity)
                .remove::<CoatGrowing>()
                .insert(CoatDrawn { entity: child, key });
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
            move |world: &mut World, grown: GrownCoat| {
                let rest: Vec<Vec3> = grown.groom.guides.iter().map(|p| Vec3::from(*p)).collect();
                let handle = world
                    .get_resource_or_insert_with(Assets::<StrandGroom>::default)
                    .add(grown.groom);
                let groom = Arc::new(CoatGroom {
                    handle,
                    per: grown.per,
                    rest,
                    skin: grown.skin,
                });
                world
                    .get_resource_or_insert_with(CoatGrooms::default)
                    .0
                    .insert(key, groom);
                if let Ok(mut e) = world.get_entity_mut(entity) {
                    e.remove::<CoatGrowing>();
                }
            },
        );
    }
    for (e, strands, parent, mut posed) in &mut drawn {
        let Ok((_, coat, Some(d), _, pose)) = coats.get(parent.parent()) else {
            commands.entity(e).despawn();
            continue;
        };
        if d.entity != e {
            commands.entity(e).despawn();
            continue;
        }
        let (Some(pose), false) = (pose, strands.groom.rest.is_empty()) else {
            continue;
        };
        if pose.bones.len() != coat.grown_on.len() {
            continue;
        }
        skin_guides(&strands.groom, &coat.grown_on, pose, &mut posed.points);
    }
}

fn spawn_child(commands: &mut Commands, body: Entity, groom: Arc<CoatGroom>) -> Entity {
    let handle = groom.handle;
    commands
        .spawn((
            Name::new("Fur"),
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(body),
            StrandInstance::new(handle),
            HairGuidesPosed::default(),
            CoatStrands { groom },
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::animation::BonePose;

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
        let grown = grow(&coat, None);
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
        assert_eq!(grown.skin.len(), g.guide_count());
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

    #[test]
    fn bare_skin_grows_nothing() {
        let mut coat = tube();
        coat.amount = vec![0.0; coat.positions.len()].into();
        assert_eq!(grow(&coat, None).groom.strand_count(), 0);
    }

    #[test]
    fn guides_skin_with_the_bones_under_their_roots() {
        let coat = tube();
        let grown = grow(&coat, None);
        let groom = CoatGroom {
            handle: Handle::from_index(0),
            per: grown.per,
            rest: grown.groom.guides.iter().map(|p| Vec3::from(*p)).collect(),
            skin: grown.skin,
        };
        let mut pose = SkeletonPose {
            names: vec!["a".to_string(), "b".to_string()].into(),
            parents: vec![None, Some(0)].into(),
            bones: vec![BonePose::default(); 2],
        };
        let mut out = Vec::new();
        skin_guides(&groom, &coat.grown_on, &pose, &mut out);
        let off = out
            .iter()
            .zip(&groom.rest)
            .map(|(a, b)| a.distance(*b))
            .fold(0.0f32, f32::max);
        assert!(off < 1e-5, "at rest, as grown: {off}");
        pose.bones[1].translation = Vec3::Y * 0.1;
        skin_guides(&groom, &coat.grown_on, &pose, &mut out);
        // Guides on the far end ride bone 1 up; on the near end stay.
        for (g, (j, w)) in groom.skin.iter().enumerate() {
            let rise = out[g * groom.per].y - groom.rest[g * groom.per].y;
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
