//! Hair and fur for humans and animals.
//!
//! A hair strand is a tiny rope, so it runs on the same solver as everything else: each
//! *guide* strand is a chain of particles whose root is pinned to a bone of the character's
//! `SkeletonPose`, with
//!
//! * rigid segments (hair doesn't stretch) and a tether to the root (no rubber hair when the
//!   head whips around),
//! * a pull toward the hairstyle's rest shape in the bone's frame that is stiff at the root
//!   and nearly free at the tip, so it keeps its style but swings,
//! * collisions against the character's [`BoneColliders`](crate::BoneColliders) (head,
//!   neck, shoulders), wind with turbulence, and partial rigid following of the head so a
//!   dash doesn't fling it.
//!
//! The renderer draws `strands_per_guide` strands around every guide, spread, clumped and
//! curled as the [`HairStyle`] says, so a few hundred simulated guides become thousands of
//! drawn hairs. Fur is the same thing with short, stiff guides spread over body capsules, or
//! over a whole skinned body mesh ([`HairEmitter::Surface`]): roots evenly spread by area,
//! riding the bones that carry the skin under them, combed along the skeleton and longer or
//! shorter by region.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::Mesh;
use engine_world::animation::SkeletonPose;

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::cloth::SkeletonRef;
use crate::settings::{SimSettings, SoftMaterial};
use crate::solver::{Goal, Sim, SkinPlane, Tether};

/// How hair or fur looks and moves.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HairStyle {
    /// Strand length, meters.
    pub length: f32,
    /// Random length variation, 0..1.
    pub length_variation: f32,
    /// Simulated segments per guide (3 for fur, 10–16 for long hair).
    pub segments: u32,
    /// Strands drawn around each guide.
    pub strands_per_guide: u32,
    /// How far drawn strands spread around their guide at the root, meters.
    pub spread: f32,
    /// 0..1: how much drawn strands gather toward their guide at the tips.
    pub clumping: f32,
    /// Curl radius, meters (0 = straight).
    pub curl: f32,
    /// Curl turns per meter.
    pub curl_frequency: f32,
    /// 0..1: how firmly it holds its style (long hair 0.2, short hair 0.6, fur 0.9).
    pub stiffness: f32,
    /// 0 = strands stick out from the scalp, 1 = they fall straight down.
    pub droop: f32,
    /// Strand width at the root, meters (drawn strands taper to the tip).
    pub thickness: f32,
    /// Eumelanin, 0 (platinum) .. 1 (black).
    pub melanin: f32,
    /// Pheomelanin, 0..1 (red and auburn tones).
    pub redness: f32,
    /// Dyed color (linear RGB), replacing the melanin color.
    pub tint: Option<[f32; 3]>,
    /// How much fur lies down along its comb direction (0 = sticks straight out).
    pub comb: f32,
}

impl Default for HairStyle {
    fn default() -> Self {
        Self::long()
    }
}

impl HairStyle {
    /// Long, straight, shoulder-length hair.
    pub fn long() -> Self {
        Self {
            length: 0.42,
            length_variation: 0.15,
            segments: 12,
            strands_per_guide: 28,
            spread: 0.012,
            clumping: 0.45,
            curl: 0.0,
            curl_frequency: 6.0,
            stiffness: 0.22,
            droop: 0.9,
            thickness: 0.0007,
            melanin: 0.35,
            redness: 0.25,
            tint: None,
            comb: 0.8,
        }
    }

    /// Short, dense animal fur.
    pub fn fur() -> Self {
        Self {
            length: 0.045,
            length_variation: 0.35,
            segments: 3,
            strands_per_guide: 56,
            spread: 0.02,
            clumping: 0.05,
            curl: 0.0015,
            curl_frequency: 30.0,
            stiffness: 0.85,
            droop: 0.0,
            thickness: 0.0005,
            melanin: 0.55,
            redness: 0.55,
            tint: None,
            comb: 0.9,
        }
    }

    /// Linear RGB albedo from melanin (or the tint).
    pub fn color(&self) -> [f32; 3] {
        if let Some(t) = self.tint {
            return t;
        }
        // Eumelanin takes hair from platinum through blond and brown to black (linear
        // albedos of real hair as seen, multiple scattering included); pheomelanin
        // (`redness`) warms it toward strawberry, auburn and ginger.
        const STOPS: [(f32, [f32; 3]); 6] = [
            (0.0, [0.80, 0.70, 0.52]),
            (0.15, [0.55, 0.40, 0.22]),
            (0.35, [0.27, 0.17, 0.09]),
            (0.6, [0.11, 0.066, 0.036]),
            (0.8, [0.045, 0.029, 0.018]),
            (1.0, [0.016, 0.013, 0.011]),
        ];
        let m = self.melanin.clamp(0.0, 1.0);
        let k = STOPS
            .iter()
            .rposition(|s| s.0 <= m)
            .unwrap_or(0)
            .min(STOPS.len() - 2);
        let (a, b) = (STOPS[k], STOPS[k + 1]);
        let t = (m - a.0) / (b.0 - a.0);
        let base = Vec3::from(a.1).lerp(Vec3::from(b.1), t);
        // Redness matters most in light and medium hair; black hair stays black.
        let r = self.redness.clamp(0.0, 1.0) * (1.0 - m * m);
        let warm = Vec3::new(1.0 + 0.7 * r, 1.0 - 0.25 * r, 1.0 - 0.65 * r);
        (base * warm).min(Vec3::splat(0.9)).to_array()
    }
}

/// Where hair grows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HairEmitter {
    /// Scalp hair on an ellipsoid around `bone` (`center` and `radii` in the bone's space,
    /// meters). Roots cover the top, sides and back of the head; `hairline` (0..1) pushes the
    /// front hairline back.
    Scalp {
        bone: String,
        center: [f32; 3],
        radii: [f32; 3],
        guides: u32,
        hairline: f32,
    },
    /// Fur around the capsule from `bone` to `to` (or a ball around `bone` when `to` is
    /// `None`), lying toward `comb` (a direction in the bone's space).
    Fur {
        bone: String,
        to: Option<String>,
        radius: f32,
        guides: u32,
        comb: [f32; 3],
    },
    /// Fur over a body mesh: `mesh` is the key of a mesh asset in the skeleton's model space,
    /// posed like the skeleton when the groom is built (a CPU-skinned body's live mesh; the
    /// groom waits until it exists). About `guides` roots are spread evenly over its area, each
    /// riding the bone that carries the skin under it (a [`BoneBlend`] of two where two do)
    /// when the mesh's skin weights are in [`MeshSkins`], else its nearest bone, and combed
    /// along the skeleton away from the bone `flow_from` (from a head: back along the body,
    /// down the legs, out to the tail tip), leaning down by `gravity` (0..1). `zones` make the
    /// fur longer or shorter by region; `bones` keeps only roots on those bones (empty: every
    /// bone) and `skip_bones` leaves some out (to give a tail its own longer groom, say).
    Surface {
        mesh: String,
        guides: u32,
        flow_from: String,
        gravity: f32,
        zones: Vec<FurZone>,
        bones: Vec<String>,
        skip_bones: Vec<String>,
    },
}

/// A region of a [`HairEmitter::Surface`] groom with longer or shorter fur: a ball of
/// `radius` meters around `offset` in `bone`'s frame, scaling the length by `length` at its
/// center and fading out at its edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FurZone {
    pub bone: String,
    pub offset: [f32; 3],
    pub radius: f32,
    pub length: f32,
}

impl Default for FurZone {
    fn default() -> Self {
        Self {
            bone: String::new(),
            offset: [0.0; 3],
            radius: 0.1,
            length: 1.0,
        }
    }
}

impl FurZone {
    pub fn new(bone: &str, offset: [f32; 3], radius: f32, length: f32) -> Self {
        Self {
            bone: bone.into(),
            offset,
            radius,
            length,
        }
    }
}

/// Skin weights of skinned meshes in `Assets<Mesh>`, by the mesh's key. Fur grown on such a
/// mesh ([`HairEmitter::Surface`]) rides the bones that carry the skin under each root,
/// blended like the skinning, so the coat moves with the skin.
#[derive(Resource, Debug, Clone, Default)]
pub struct MeshSkins(pub std::collections::HashMap<String, std::sync::Arc<MeshSkin>>);

/// Up to four bones and their weights per vertex, in the mesh's vertex order.
#[derive(Debug, Clone, Default)]
pub struct MeshSkin {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

impl MeshSkin {
    /// The two bones with the largest weights at the point with barycentric coordinates
    /// `bary` in the triangle `tri`, and the second one's share of their sum (0..0.5).
    fn binding(&self, tri: [u32; 3], bary: [f32; 3]) -> Option<(usize, usize, f32)> {
        let mut acc = [(0u16, 0.0f32); 12];
        let mut n = 0;
        for (v, b) in tri.iter().zip(bary) {
            let (j, w) = (
                self.joints.get(*v as usize)?,
                self.weights.get(*v as usize)?,
            );
            for k in 0..4 {
                let w = w[k] * b;
                if w <= 0.0 {
                    continue;
                }
                match acc[..n].iter_mut().find(|a| a.0 == j[k]) {
                    Some(a) => a.1 += w,
                    None => {
                        acc[n] = (j[k], w);
                        n += 1;
                    }
                }
            }
        }
        let acc = &mut acc[..n];
        acc.sort_by(|a, b| b.1.total_cmp(&a.1));
        match *acc {
            [] => None,
            [(a, _)] => Some((a as usize, a as usize, 0.0)),
            [(a, wa), (b, wb), ..] => Some((a as usize, b as usize, wb / (wa + wb))),
        }
    }
}

/// Two bones of a character's `SkeletonPose` blended like dual-quaternion skinning: a frame
/// that moves the way skin weighted `1 − t` to bone `a` and `t` to bone `b` does, from the
/// pose the bones had when it was made (`rest_a`, `rest_b`: model-space translation and
/// rotation). Fur at the joints of a skinned body rides these, so it stays on the skin as the
/// joint bends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoneBlend {
    pub owner: Entity,
    pub a: usize,
    pub b: usize,
    pub t: f32,
    pub rest_a: (Vec3, Quat),
    pub rest_b: (Vec3, Quat),
}

impl BoneBlend {
    /// The blended skinning transform for `pose` (model space, from the rest pose):
    /// translation and rotation.
    pub fn model(&self, pose: &SkeletonPose) -> Option<(Vec3, Quat)> {
        let delta = |i: usize, (t, r): (Vec3, Quat)| {
            let p = pose.bones.get(i)?;
            let rot = (p.rotation * r.conjugate()).normalize();
            Some((rot, p.translation - rot * t))
        };
        let (ra, ta) = delta(self.a, self.rest_a)?;
        let (mut rb, tb) = delta(self.b, self.rest_b)?;
        if ra.dot(rb) < 0.0 {
            rb = -rb;
        }
        let dual = |r: Quat, t: Vec3| Quat::from_xyzw(t.x, t.y, t.z, 0.0) * r * 0.5;
        let real = ra * (1.0 - self.t) + rb * self.t;
        let d = dual(ra, ta) * (1.0 - self.t) + dual(rb, tb) * self.t;
        let inv = 1.0 / real.length().max(1e-8);
        let real = real * inv;
        let t = d * inv * real.conjugate() * 2.0;
        Some((Vec3::new(t.x, t.y, t.z), real))
    }

    /// The frame in the world for the owner's transform `owner`.
    pub fn world(&self, owner: &GlobalTransform, pose: &SkeletonPose) -> Option<(DVec3, Quat)> {
        let (t, r) = self.model(pose)?;
        let rot = owner.rotation();
        Some((
            owner.translation() + (rot * (t * owner.scale())).as_dvec3(),
            rot * r,
        ))
    }
}

/// Simulated hair or fur. Add it to a child (or descendant) of an entity with a
/// `SkeletonPose`; it grows from that skeleton's bones.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HairGroom {
    pub emitters: Vec<HairEmitter>,
    pub style: HairStyle,
    pub sim: SimSettings,
}

impl Default for HairGroom {
    fn default() -> Self {
        Self {
            emitters: Vec::new(),
            style: HairStyle::long(),
            sim: Self::hair_settings(),
        }
    }
}

impl HairGroom {
    /// Solver settings that suit hair.
    pub fn hair_settings() -> SimSettings {
        SimSettings {
            substeps: 12,
            damping: 0.6,
            friction: 0.2,
            thickness: 0.008,
            follow: 0.35,
            prewarm_secs: 1.0,
            max_speed: 40.0,
            // Keratin is a little denser than water; in it, hair drifts and fans out.
            buoyancy: 0.85,
            water_drag: 9.0,
            soak: 0.5,
            ..SimSettings::default()
        }
    }

    /// True while a [`HairEmitter::Surface`] mesh this groom grows on isn't in `meshes` yet.
    pub fn waiting_for_mesh(&self, meshes: Option<&Assets<Mesh>>) -> bool {
        self.emitters.iter().any(|e| match e {
            HairEmitter::Surface { mesh, .. } => {
                meshes.and_then(|m| m.loaded_by_key(mesh)).is_none()
            }
            _ => false,
        })
    }

    /// Long hair on a head bone of radius ~0.1 m.
    pub fn long_hair(bone: &str, style: HairStyle) -> Self {
        Self {
            emitters: vec![HairEmitter::Scalp {
                bone: bone.into(),
                center: [0.0, 0.03, 0.01],
                radii: [0.095, 0.1, 0.1],
                guides: 260,
                hairline: 0.5,
            }],
            style,
            sim: Self::hair_settings(),
        }
    }
}

/// One simulated guide strand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    /// First particle and particle count (root first).
    pub first: u32,
    pub count: u32,
    /// Solver frame (bone) the root follows.
    pub frame: u16,
    /// Root normal and a tangent in the frame's space (orient the drawn strands around it).
    pub normal: Vec3,
    pub tangent: Vec3,
}

/// What the renderer needs to draw hair.
#[derive(Debug, Clone)]
pub struct Strands {
    pub guides: Vec<Guide>,
    pub style: HairStyle,
    /// Linear RGB.
    pub color: [f32; 3],
}

/// The skin hair grows from, in bone space. Rest shapes are kept outside it.
#[derive(Clone, Copy)]
enum Skin {
    Ellipsoid {
        center: Vec3,
        radii: Vec3,
    },
    Capsule {
        a: Vec3,
        b: Vec3,
        radius: f32,
    },
    /// The tangent plane at a root on a mesh.
    Plane {
        point: Vec3,
        normal: Vec3,
    },
}

impl Skin {
    /// `p` moved out to at least `margin` above the skin.
    fn outside(&self, p: Vec3, margin: f32) -> Vec3 {
        match *self {
            Skin::Ellipsoid { center, radii } => {
                let r = radii + Vec3::splat(margin);
                let q = (p - center) / r;
                if q.length_squared() >= 1.0 {
                    p
                } else {
                    center + q.normalize_or(Vec3::Y) * r
                }
            }
            Skin::Capsule { a, b, radius } => {
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                let c = a + ab * t;
                let d = p - c;
                let r = radius + margin;
                if d.length_squared() >= r * r {
                    p
                } else {
                    c + d.normalize_or(Vec3::Y) * r
                }
            }
            Skin::Plane { point, normal } => {
                let h = (p - point).dot(normal);
                if h >= margin {
                    p
                } else {
                    p + normal * (margin - h)
                }
            }
        }
    }
}

/// A root in its frame's space (its bone's, or the model's when it rides a blend of two
/// bones): position, normal, comb direction, and the skin it grows from.
struct Root {
    bone: usize,
    /// The second bone and its share (quantized), for a root on skin two bones carry.
    blend: Option<(usize, f32)>,
    pos: Vec3,
    normal: Vec3,
    comb: Vec3,
    skin: Skin,
    /// Length scale of this root's strand.
    length: f32,
}

const GOLDEN_ANGLE: f32 = 2.399_963;

/// How close fur on a body mesh lies to the skin, meters.
const FUR_ON_SKIN: f32 = 0.002;

fn fibonacci_sphere(n: usize) -> impl Iterator<Item = Vec3> {
    (0..n).map(move |i| {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let a = i as f32 * GOLDEN_ANGLE;
        Vec3::new(r * a.cos(), y, r * a.sin())
    })
}

/// How fully hair covers the scalp in direction `d` from the scalp ellipsoid's center (in its
/// radii), 0..1: [`scalp_keep`] with a soft edge just behind the hairline, so paint under the
/// hair fades out as a hairline rather than a hard line.
pub(crate) fn scalp_cover(d: Vec3, hairline: f32) -> f32 {
    let front = smoothstep(0.0, 0.9, -d.z);
    let min_y = lerp(-0.4, 0.3 + 0.3 * hairline, front);
    let ears = smoothstep(0.74, 0.86, d.x.abs())
        * (1.0 - smoothstep(0.08, 0.22, d.y))
        * (1.0 - smoothstep(0.22, 0.38, d.z));
    smoothstep(min_y, min_y + 0.16, d.y) * (1.0 - ears)
}

fn scalp_keep(d: Vec3, hairline: f32) -> bool {
    // −Z is forward. The face (front, low) and the ears stay bare.
    let front = smoothstep(0.0, 0.9, -d.z);
    let min_y = lerp(-0.4, 0.3 + 0.3 * hairline, front);
    let ears = d.x.abs() > 0.8 && d.y < 0.15 && d.z < 0.3;
    d.y > min_y && !ears
}

fn emit_roots(
    emitter: &HairEmitter,
    pose: &engine_world::animation::SkeletonPose,
    meshes: Option<&Assets<Mesh>>,
    skins: Option<&MeshSkins>,
) -> Result<Vec<Root>, String> {
    let bone_index = |name: &str| {
        pose.index_of(name).ok_or_else(|| {
            format!(
                "Bone \"{name}\" is not in the skeleton. Bones: {}.",
                pose.names.join(", ")
            )
        })
    };
    let mut roots = Vec::new();
    match emitter {
        HairEmitter::Scalp {
            bone,
            center,
            radii,
            guides,
            hairline,
        } => {
            let b = bone_index(bone)?;
            let center = Vec3::from(*center);
            let radii = Vec3::from(*radii).max(Vec3::splat(0.01));
            let want = (*guides).clamp(1, 4000) as usize;
            let mut m = want * 2;
            let kept = loop {
                let kept: Vec<Vec3> = fibonacci_sphere(m)
                    .filter(|d| scalp_keep(*d, *hairline))
                    .collect();
                if kept.len() >= want || m > want * 16 {
                    break kept;
                }
                m = m * 3 / 2;
            };
            for k in 0..want.min(kept.len()) {
                let d = kept[k * kept.len() / want];
                // Swept back from the hairline (−Z is the face), falling down at the sides
                // and the back.
                let comb = Vec3::new(0.0, -1.0 + 1.6 * (-d.z).max(0.0), 0.8).normalize();
                roots.push(Root {
                    bone: b,
                    blend: None,
                    pos: center + d * radii,
                    normal: (d / radii).normalize(),
                    comb,
                    skin: Skin::Ellipsoid { center, radii },
                    length: 1.0,
                });
            }
        }
        HairEmitter::Fur {
            bone,
            to,
            radius,
            guides,
            comb,
        } => {
            let b = bone_index(bone)?;
            let n = (*guides).clamp(1, 8000) as usize;
            let comb = Vec3::from(*comb).normalize_or(Vec3::Z);
            match to {
                Some(to) => {
                    let t = bone_index(to)?;
                    let (pa, pb) = (pose.bones[b], pose.bones[t]);
                    // The far bone's position in this bone's space.
                    let axis = pa.rotation.conjugate() * (pb.translation - pa.translation);
                    let len = axis.length().max(1e-3);
                    let dir = axis / len;
                    let side = dir.any_orthonormal_vector();
                    let up = dir.cross(side);
                    for k in 0..n {
                        let t = (k as f32 + 0.5) / n as f32;
                        let a = k as f32 * GOLDEN_ANGLE;
                        let normal = side * a.cos() + up * a.sin();
                        roots.push(Root {
                            bone: b,
                            blend: None,
                            pos: dir * (t * len) + normal * *radius,
                            normal,
                            comb,
                            skin: Skin::Capsule {
                                a: Vec3::ZERO,
                                b: axis,
                                radius: *radius,
                            },
                            length: 1.0,
                        });
                    }
                }
                None => {
                    for d in fibonacci_sphere(n) {
                        roots.push(Root {
                            bone: b,
                            blend: None,
                            pos: d * *radius,
                            normal: d,
                            comb,
                            skin: Skin::Ellipsoid {
                                center: Vec3::ZERO,
                                radii: Vec3::splat(*radius),
                            },
                            length: 1.0,
                        });
                    }
                }
            }
        }
        HairEmitter::Surface {
            mesh,
            guides,
            flow_from,
            gravity,
            zones,
            bones,
            skip_bones,
        } => {
            let Some(m) = meshes.and_then(|a| a.loaded_by_key(mesh).and_then(|h| a.get(h))) else {
                return Err(format!(
                    "Mesh \"{mesh}\" isn't in Assets<Mesh>. Register the body mesh under that \
                     key first."
                ));
            };
            let source = bone_index(flow_from)?;
            let keep: Vec<usize> = bones
                .iter()
                .map(|b| bone_index(b))
                .collect::<Result<_, _>>()?;
            let skip: Vec<usize> = skip_bones
                .iter()
                .map(|b| bone_index(b))
                .collect::<Result<_, _>>()?;
            let zones: Vec<(usize, &FurZone)> = zones
                .iter()
                .map(|z| bone_index(&z.bone).map(|b| (b, z)))
                .collect::<Result<_, _>>()?;
            let skin = skins.and_then(|s| s.0.get(mesh)).filter(|s| {
                s.joints.len() == m.vertices.len() && s.weights.len() == m.vertices.len()
            });
            let field = FurField::new(pose, source);
            // Where the flow runs straight out of the skin (a forehead, the rump under a
            // raised tail), the fur lies down and back instead (characters face −Z).
            let down_back = Vec3::new(0.0, -1.0, 1.0).normalize();
            for sp in sample_surface(m, (*guides).clamp(1, 20_000) as usize) {
                let (p, n) = (sp.position, sp.normal);
                let (nearest, comb) = field.at(p, *gravity);
                // Skin two bones carry gets a frame blended between them, in sixteenths.
                let (bone, blend) = match skin
                    .and_then(|s| s.binding(sp.triangle, sp.bary))
                    .filter(|(a, b, _)| *a < pose.bones.len() && *b < pose.bones.len())
                {
                    Some((a, b, t)) => {
                        let t = (t * 16.0).round() / 16.0;
                        (a, (t > 0.0).then_some((b, t)))
                    }
                    None => (nearest, None),
                };
                let along = comb - n * comb.dot(n);
                let fallback = (down_back - n * down_back.dot(n)).normalize_or_zero();
                let comb =
                    (along + fallback * (1.0 - along.length() / 0.6).max(0.0)).normalize_or(comb);
                if (!keep.is_empty() && !keep.contains(&bone)) || skip.contains(&bone) {
                    continue;
                }
                let mut length = 1.0;
                for (b, z) in &zones {
                    let bp = pose.bones[*b];
                    let c = bp.translation + bp.rotation * Vec3::from(z.offset);
                    let w = 1.0 - smoothstep(0.0, z.radius.max(1e-3), p.distance(c));
                    length *= lerp(1.0, z.length, w);
                }
                // A blended frame starts out as the model's; a bone's is the bone's.
                let (origin, inv) = match blend {
                    Some(_) => (Vec3::ZERO, Quat::IDENTITY),
                    None => (
                        pose.bones[bone].translation,
                        pose.bones[bone].rotation.conjugate(),
                    ),
                };
                let pos = inv * (p - origin);
                let normal = (inv * n).normalize_or(Vec3::Y);
                roots.push(Root {
                    bone,
                    blend,
                    pos,
                    normal,
                    comb: inv * comb,
                    skin: Skin::Plane { point: pos, normal },
                    length,
                });
            }
        }
    }
    Ok(roots)
}

/// Bone binding and comb direction over a skeleton: every bone is a segment to its first
/// child (a point for end bones), and fur flows along each segment away from the source bone
/// (counted in bones along the skeleton).
struct FurField {
    segments: Vec<(Vec3, Vec3)>,
    flow: Vec<Vec3>,
}

impl FurField {
    fn new(pose: &engine_world::animation::SkeletonPose, source: usize) -> Self {
        let n = pose.bones.len();
        let parent = |i: usize| pose.parents.get(i).copied().flatten().map(|p| p as usize);
        let first_child = |i: usize| (0..n).find(|&c| parent(c) == Some(i));
        // Hops from the source along the skeleton.
        let mut hops = vec![usize::MAX; n];
        hops[source.min(n - 1)] = 0;
        for _ in 0..n {
            for i in 0..n {
                if let Some(p) = parent(i) {
                    let (a, b) = (hops[i], hops[p]);
                    if a != usize::MAX && b == usize::MAX {
                        hops[p] = a + 1;
                    }
                    if b != usize::MAX && a == usize::MAX {
                        hops[i] = b + 1;
                    }
                }
            }
        }
        let pos = |i: usize| pose.bones[i].translation;
        let mut segments = Vec::with_capacity(n);
        let mut flow = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = match first_child(i) {
                Some(c) => (pos(i), pos(c)),
                None => (pos(i), pos(i)),
            };
            segments.push((a, b));
            // Along the segment toward the bone farther from the source; an end bone flows
            // like the segment leading into it.
            let (from, to, far_end) = match first_child(i) {
                Some(c) => (pos(i), pos(c), hops[c] >= hops[i]),
                None => match parent(i) {
                    Some(p) => (pos(p), pos(i), hops[i] >= hops[p]),
                    None => (pos(i), pos(i), true),
                },
            };
            let d = (to - from).normalize_or(Vec3::NEG_Y);
            flow.push(if far_end { d } else { -d });
        }
        Self { segments, flow }
    }

    /// The bone a point binds to (the nearest segment; ties go to the child, so a muzzle
    /// past the end of the neck belongs to the head) and the comb direction there.
    fn at(&self, p: Vec3, gravity: f32) -> (usize, Vec3) {
        let dist = |(a, b): (Vec3, Vec3)| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            p.distance(a + ab * t)
        };
        let d: Vec<f32> = self.segments.iter().map(|s| dist(*s)).collect();
        let mut best = 0;
        for i in 1..d.len() {
            if d[i] <= d[best] + 1e-5 {
                best = i;
            }
        }
        // Blend the flow of nearby segments so the comb turns smoothly at joints.
        let mut comb = Vec3::ZERO;
        for (i, di) in d.iter().enumerate() {
            let x = (di - d[best]) / 0.04;
            comb += self.flow[i] * (-x * x).exp();
        }
        let comb = comb.normalize_or(self.flow[best]) + Vec3::NEG_Y * gravity;
        (best, comb.normalize_or(Vec3::NEG_Y))
    }
}

/// A point on a mesh: where it is, the interpolated normal, and the triangle it's in.
struct SurfacePoint {
    position: Vec3,
    normal: Vec3,
    triangle: [u32; 3],
    bary: [f32; 3],
}

/// About `n` evenly spread points on a mesh: area-weighted random candidates thinned so no two
/// are closer than the spacing `n` points need.
fn sample_surface(mesh: &Mesh, n: usize) -> Vec<SurfacePoint> {
    let tris = mesh.indices.as_chunks::<3>().0;
    let vert = |i: u32| mesh.vertices.get(i as usize);
    let mut cdf = Vec::with_capacity(tris.len());
    let mut total = 0.0f32;
    for t in tris {
        let area = match (vert(t[0]), vert(t[1]), vert(t[2])) {
            (Some(a), Some(b), Some(c)) => {
                let (a, b, c) = (
                    Vec3::from(a.position),
                    Vec3::from(b.position),
                    Vec3::from(c.position),
                );
                (b - a).cross(c - a).length() * 0.5
            }
            _ => 0.0,
        };
        total += area;
        cdf.push(total);
    }
    if total <= 0.0 || n == 0 {
        return Vec::new();
    }
    let spacing = (total / n as f32).sqrt() * 0.7;
    let mut rng = Rng::new(0x5EED_F0A1);
    let mut grid: std::collections::HashMap<IVec3, Vec<Vec3>> = Default::default();
    let cell = |p: Vec3| (p / spacing).floor().as_ivec3();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n * 10 {
        if out.len() >= n {
            break;
        }
        let r = rng.f32() * total;
        let k = cdf.partition_point(|c| *c < r).min(tris.len() - 1);
        let t = tris[k];
        let (Some(a), Some(b), Some(c)) = (vert(t[0]), vert(t[1]), vert(t[2])) else {
            continue;
        };
        let (mut u, mut v) = (rng.f32(), rng.f32());
        if u + v > 1.0 {
            (u, v) = (1.0 - u, 1.0 - v);
        }
        let w = 1.0 - u - v;
        let p =
            Vec3::from(a.position) * w + Vec3::from(b.position) * u + Vec3::from(c.position) * v;
        let c0 = cell(p);
        let crowded = (-1..=1).any(|x| {
            (-1..=1).any(|y| {
                (-1..=1).any(|z| {
                    grid.get(&(c0 + IVec3::new(x, y, z))).is_some_and(|l| {
                        l.iter().any(|q| q.distance_squared(p) < spacing * spacing)
                    })
                })
            })
        });
        if crowded {
            continue;
        }
        grid.entry(c0).or_default().push(p);
        let nrm = (Vec3::from(a.normal) * w + Vec3::from(b.normal) * u + Vec3::from(c.normal) * v)
            .normalize_or(Vec3::Y);
        out.push(SurfacePoint {
            position: p,
            normal: nrm,
            triangle: t,
            bary: [w, u, v],
        });
    }
    out
}

/// Mass of one guide strand, kg (a lock of a few hundred hairs).
pub(crate) const GUIDE_MASS: f32 = 0.0004;

/// Goal spring frequencies at a strand's root and tip (rad/s) for a style's `stiffness`.
pub(crate) fn strand_omegas(stiffness: f32) -> (f32, f32) {
    let s = stiffness.clamp(0.01, 1.0);
    (15.0 + 300.0 * s, 0.5 + 25.0 * s * s)
}

/// Add one guide strand to `sim`: particles at `rest` (root first, in the frame's space; the
/// frame is placed at `placed`, body-local), rigid segments, the two root particles pinned,
/// goals toward `rest` from stiff at the root to loose at the tip (at least `held(arc)` times
/// 35% of the root's frequency), tethers to the root, and optionally a skin plane each
/// particle stays above. Returns the first particle.
/// How firmly (0..1) hair rooted with outward `normal` (the person faces −Z) is held along
/// its grown shape `arc` meters from the root. Hair from the front and the crown is swept back
/// over the head and held along the sweep, as the roots and the hair around them hold it, else
/// it slides off the crown over the face.
pub(crate) fn swept_hold(normal: Vec3, arc: f32) -> f32 {
    let sweep = (-normal.z).max(normal.y).clamp(0.0, 1.0);
    sweep * (1.0 - smoothstep(0.14, 0.26, arc))
}

pub(crate) fn push_strand(
    sim: &mut Sim,
    frame: u16,
    placed: (Vec3, Quat),
    rest: &[Vec3],
    (omega_root, omega_tip): (f32, f32),
    held: impl Fn(f32) -> f32,
    skin_plane: Option<(Vec3, Vec3)>,
) -> u32 {
    let segments = rest.len().saturating_sub(1).max(1);
    let mass = GUIDE_MASS / rest.len().max(1) as f32;
    let first = sim.len() as u32;
    for q in rest {
        sim.add_particle(placed.0 + placed.1 * *q, mass);
    }
    for k in 1..rest.len() as u32 {
        sim.add_distance(first + k - 1, first + k, 0.0, u32::MAX);
    }
    // Two root particles pinned: the root keeps its direction.
    sim.pin(first, frame, rest[0]);
    if rest.len() > 1 {
        sim.pin(first + 1, frame, rest[1]);
    }
    let mut arc = rest.get(1).map_or(0.0, |r1| r1.distance(rest[0]));
    for k in 2..rest.len() {
        let local = rest[k];
        arc += local.distance(rest[k - 1]);
        let t = k as f32 / segments as f32;
        let omega = (omega_root.ln() + (omega_tip.ln() - omega_root.ln()) * t.powf(0.6))
            .exp()
            .max(omega_root * 0.35 * held(arc));
        // A spring of stiffness k = m·ω² on each particle; compliance is 1/k.
        sim.goals.push(Goal {
            p: first + k as u32,
            frame,
            local,
            compliance: 1.0 / (mass * omega * omega),
        });
        sim.tethers.push(Tether {
            p: first + k as u32,
            anchor: first,
            max: arc * 1.03,
        });
        // Fur on a body mesh doesn't sink into the skin it grows from.
        if let Some((point, normal)) = skin_plane {
            sim.skin_planes.push(SkinPlane {
                p: first + k as u32,
                frame,
                point,
                normal,
                margin: FUR_ON_SKIN,
            });
        }
    }
    first
}

/// Build hair for `skeleton`; [`HairEmitter::Surface`] emitters read their mesh from `meshes`
/// and its skin weights from `skins`.
pub fn build(
    spec: &HairGroom,
    skeleton: Option<SkeletonRef>,
    meshes: Option<&Assets<Mesh>>,
    skins: Option<&MeshSkins>,
) -> Result<SoftBodySim, String> {
    let Some((owner, owner_t, pose)) = skeleton else {
        return Err("HairGroom needs a parent (or ancestor) with a SkeletonPose.".into());
    };
    let style = spec.style;
    let segments = style.segments.clamp(1, 32) as usize;
    let origin = owner_t.translation();
    let mut sim = Sim::new(origin);
    let mut frames = Vec::new();
    // Frame per bone, or per pair of bones and blend (in sixteenths).
    let mut frame_of: std::collections::HashMap<(usize, Option<(usize, u8)>), u16> =
        Default::default();
    let mut guides = Vec::new();
    let mut rng = Rng::new(0x4A12);
    let thickness = spec.sim.thickness.max(style.spread * 0.5 + 0.003);
    let (omega_root, omega_tip) = strand_omegas(style.stiffness);
    for emitter in &spec.emitters {
        for root in emit_roots(emitter, pose, meshes, skins)? {
            let source = match root.blend {
                None => FrameSource::Bone {
                    owner,
                    bone: root.bone,
                },
                Some((b, t)) => {
                    let rest = |i: usize| (pose.bones[i].translation, pose.bones[i].rotation);
                    FrameSource::Blend(BoneBlend {
                        owner,
                        a: root.bone,
                        b,
                        t,
                        rest_a: rest(root.bone),
                        rest_b: rest(b),
                    })
                }
            };
            let placed = match source {
                FrameSource::Blend(blend) => blend.world(owner_t, pose),
                _ => pose.bone_world(owner_t, root.bone),
            };
            let Some((bp, br)) = placed else {
                continue;
            };
            let key = (
                root.bone,
                root.blend.map(|(b, t)| (b, (t * 16.0).round() as u8)),
            );
            let frame = *frame_of.entry(key).or_insert_with(|| {
                frames.push(source);
                sim.add_frame((bp - origin).as_vec3(), br)
            });
            let length = style.length
                * root.length
                * (1.0 + style.length_variation * (rng.f32() * 2.0 - 1.0));
            let seg_len = length.max(0.002) / segments as f32;
            // Rest shape in the frame's space.
            let mut rest = vec![root.pos];
            // Combed hair leaves the skin at a shallow angle, along the comb direction.
            let along = (root.comb - root.normal * root.comb.dot(root.normal)).normalize_or_zero();
            let lift = 1.0 - 0.75 * style.comb.clamp(0.0, 1.0);
            let combed = (root.normal * lift + along * style.comb).normalize_or(root.normal);
            let mut dir = combed;
            // Hangs down, a little out to the sides and the back. Hair from the front and the
            // crown is first swept back over the head, so it falls behind it instead of over
            // the face (−Z) or straight into the scalp.
            let out = Vec3::new(root.normal.x, 0.0, root.normal.z.max(0.0)).normalize_or_zero();
            let hang = (Vec3::NEG_Y + out * 0.35 + Vec3::Z * 0.12).normalize();
            let sweep = (-root.normal.z).max(root.normal.y).clamp(0.0, 1.0);
            let back = Vec3::new(0.0, -0.25, 1.0).normalize();
            // Hair drapes over a scalp at its thickness; fur on a body mesh lies close to it.
            let drape = match root.skin {
                Skin::Plane { .. } => FUR_ON_SKIN,
                _ => thickness,
            };
            let mut p = root.pos;
            for k in 1..=segments {
                let arc = k as f32 * seg_len;
                let bend = smoothstep(0.0, 1.0, arc / (0.03 + 0.1 * (1.0 - style.droop)));
                let swept = sweep * (1.0 - smoothstep(0.08, 0.2, arc));
                let fall = hang.lerp(back, swept).normalize_or(hang);
                let target = combed.lerp(fall, bend * style.droop).normalize_or(combed);
                dir = dir.lerp(target, 0.6).normalize_or(target);
                // Drape over the skin instead of falling through it.
                let next = root.skin.outside(p + dir * seg_len, drape);
                dir = (next - p).normalize_or(dir);
                p += dir * seg_len;
                rest.push(p);
            }
            let held = |arc: f32| swept_hold(root.normal, arc);
            let skin_plane = match root.skin {
                Skin::Plane { point, normal } => Some((point, normal)),
                _ => None,
            };
            let first = push_strand(
                &mut sim,
                frame,
                ((bp - origin).as_vec3(), br),
                &rest,
                (omega_root, omega_tip),
                held,
                skin_plane,
            );
            guides.push(super::hair::Guide {
                first,
                count: (segments + 1) as u32,
                frame,
                normal: root.normal,
                tangent: root.normal.any_orthonormal_vector(),
            });
        }
    }
    if guides.is_empty() {
        return Err("HairGroom has no emitters (or they produced no roots).".into());
    }
    sim.params.radius = thickness;
    sim.params.drag = 2.5;
    sim.params.turbulence = 0.6;
    // The groom partly follows the frame nearest the skeleton's root (the head for scalp
    // hair, the hips for a coat), not a swinging limb's.
    let depth = |bone: usize| {
        let mut i = bone;
        let mut d = 0;
        while let Some(p) = pose.parents.get(i).copied().flatten() {
            i = p as usize;
            d += 1;
            if d > pose.bones.len() {
                break;
            }
        }
        d
    };
    let head = frame_of
        .iter()
        .min_by_key(|((bone, blend), f)| (depth(*bone), blend.is_some(), **f))
        .map_or(guides[0].frame, |(_, f)| *f);
    if spec.sim.follow > 0.0 {
        sim.params.follow = Some((head, spec.sim.follow));
    }
    let mut body = SoftBodySim::new(
        SoftKind::Hair,
        sim,
        SimSettings {
            thickness,
            ..spec.sim
        },
        SoftMaterial {
            color: style.color(),
            ..SoftMaterial::default()
        },
    );
    body.frames = frames;
    body.time_owner = Some(owner);
    body.strands = Some(Strands {
        guides,
        style,
        color: style.color(),
    });
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::animation::{BonePose, SkeletonPose};
    use std::sync::Arc;

    fn head_pose() -> SkeletonPose {
        SkeletonPose {
            names: Arc::from(vec!["root".to_string(), "head".to_string()]),
            parents: Arc::from(vec![None, Some(0)]),
            bones: vec![
                BonePose::default(),
                BonePose {
                    translation: Vec3::new(0.0, 1.65, 0.0),
                    ..BonePose::default()
                },
            ],
        }
    }

    #[test]
    fn long_hair_grows_outside_the_head_and_holds_while_the_head_dashes() {
        let pose = head_pose();
        let mut t = GlobalTransform::default();
        let groom = HairGroom::long_hair("head", HairStyle::long());
        let mut body = build(&groom, Some((Entity::PLACEHOLDER, &t, &pose)), None, None).unwrap();
        let strands = body.strands.clone().unwrap();
        assert!(strands.guides.len() > 200, "{}", strands.guides.len());
        // A head sphere collider that moves with the head.
        let head_radius = 0.095;
        let mut head = Vec3::new(0.0, 1.68, 0.01);
        let dt = 1.0 / 60.0;
        for step in 0..240 {
            // Dash: 20 m/s along −Z for a quarter second, then stop dead.
            let v = if (60..75).contains(&step) { 20.0 } else { 0.0 };
            let prev = head;
            head.z -= v * dt;
            t = GlobalTransform::from(Transform::from_xyz(0.0, 0.0, (head.z - 0.01) as f64));
            if let Some((p, r)) = pose.bone_world(&t, 1) {
                body.sim.set_frame(0, (p - body.sim.origin).as_vec3(), r);
            }
            let c = crate::collide::LocalCollider {
                shape: crate::collide::LocalShape::Sphere {
                    radius: head_radius,
                },
                from_pos: prev,
                from_rot: Quat::IDENTITY,
                to_pos: head,
                to_rot: Quat::IDENTITY,
                friction: 0.2,
                id: 0,
                dynamic: false,
            };
            body.sim.step(dt, &[c]);
            for (i, p) in body.sim.pos.iter().enumerate() {
                assert!(p.is_finite(), "particle {i} exploded");
            }
            // No guide particle deep inside the head.
            let deepest = body
                .sim
                .pos
                .iter()
                .map(|p| p.distance(head) - head_radius)
                .fold(f32::INFINITY, f32::min);
            assert!(
                deepest > -0.01,
                "step {step}: a strand is {deepest} m inside the head"
            );
        }
        // After the dash it settles: tips hang below the roots.
        let g = strands.guides[0];
        let root = body.sim.pos[g.first as usize];
        let tip = body.sim.pos[(g.first + g.count - 1) as usize];
        assert!(tip.y < root.y, "tip {tip} root {root}");
        assert!(body.sim.kinetic_energy() < 1e-3);
        // The face stays clear: hair from the front of the head is swept back over it.
        for p in &body.sim.pos {
            let d = *p - head;
            let over_face = d.z < -0.05 && d.y < 0.02 && d.x.abs() < 0.06;
            assert!(!over_face, "a strand hangs over the face at {d}");
        }
    }

    #[test]
    fn hair_sways_in_the_wind_and_falls_back() {
        let pose = head_pose();
        let t = GlobalTransform::default();
        let groom = HairGroom::long_hair("head", HairStyle::long());
        let mut body = build(&groom, Some((Entity::PLACEHOLDER, &t, &pose)), None, None).unwrap();
        let strands = body.strands.clone().unwrap();
        let tips: Vec<usize> = strands
            .guides
            .iter()
            .map(|g| (g.first + g.count - 1) as usize)
            .collect();
        let mean_x =
            |sim: &Sim| tips.iter().map(|&i| sim.pos[i].x).sum::<f32>() / tips.len() as f32;
        let dt = 1.0 / 60.0;
        for _ in 0..120 {
            body.sim.step(dt, &[]);
        }
        let rest = mean_x(&body.sim);
        // A fresh breeze from the left blows the ends out to the right.
        body.sim.params.wind = Vec3::new(6.0, 0.0, 0.0);
        for _ in 0..90 {
            body.sim.step(dt, &[]);
        }
        let blown = mean_x(&body.sim);
        assert!(
            blown - rest > 0.04,
            "tips moved {} m in the wind",
            blown - rest
        );
        // When it drops, the hair falls back into its style.
        body.sim.params.wind = Vec3::ZERO;
        for _ in 0..240 {
            body.sim.step(dt, &[]);
        }
        let back = mean_x(&body.sim);
        assert!(
            (back - rest).abs() < 0.015,
            "tips {} m from where they were",
            back - rest
        );
    }

    #[test]
    fn melanin_darkens_hair() {
        let blond = HairStyle {
            melanin: 0.1,
            redness: 0.1,
            ..HairStyle::long()
        }
        .color();
        let black = HairStyle {
            melanin: 1.0,
            redness: 0.0,
            ..HairStyle::long()
        }
        .color();
        assert!(blond[0] > black[0] * 4.0, "{blond:?} vs {black:?}");
        assert!(blond[0] > blond[2], "blond is warm");
    }

    /// Points hashed into 2.5 cm cells, for nearest-point queries within that distance.
    struct PointGrid<'a> {
        points: &'a [Vec3],
        cells: std::collections::HashMap<IVec3, Vec<usize>>,
    }

    impl<'a> PointGrid<'a> {
        const CELL: f32 = 0.025;

        fn new(points: &'a [Vec3]) -> Self {
            let mut cells: std::collections::HashMap<IVec3, Vec<usize>> = Default::default();
            for (i, p) in points.iter().enumerate() {
                cells
                    .entry((*p / Self::CELL).floor().as_ivec3())
                    .or_default()
                    .push(i);
            }
            Self { points, cells }
        }

        /// The nearest point within a cell of `p` (index, distance), or infinitely far.
        fn nearest(&self, p: Vec3) -> (usize, f32) {
            let c = (p / Self::CELL).floor().as_ivec3();
            let mut best = (usize::MAX, f32::INFINITY);
            for x in -1..=1 {
                for y in -1..=1 {
                    for z in -1..=1 {
                        let near = self.cells.get(&(c + IVec3::new(x, y, z)));
                        for &i in near.into_iter().flatten() {
                            let d = self.points[i].distance(p);
                            if d < best.1 {
                                best = (i, d);
                            }
                        }
                    }
                }
            }
            best
        }
    }

    /// Where a root's frame is (model space) in `pose`, for roots made in `rest`.
    fn root_frame(r: &Root, rest: &SkeletonPose, pose: &SkeletonPose) -> (Vec3, Quat) {
        match r.blend {
            None => (pose.bones[r.bone].translation, pose.bones[r.bone].rotation),
            Some((b, t)) => {
                let at = |i: usize| (rest.bones[i].translation, rest.bones[i].rotation);
                let blend = BoneBlend {
                    owner: Entity::PLACEHOLDER,
                    a: r.bone,
                    b,
                    t,
                    rest_a: at(r.bone),
                    rest_b: at(b),
                };
                blend.model(pose).unwrap()
            }
        }
    }

    /// The dog's body, registered with its skin weights, and its bind pose.
    fn dog_body() -> (
        crate::demo::body::SkinTemplate,
        Assets<Mesh>,
        MeshSkins,
        SkeletonPose,
    ) {
        use crate::demo::body::{self, BodyPart, BodyQuality};
        let template = body::build(BodyPart::Dog, BodyQuality(2.0));
        let mut meshes = Assets::<Mesh>::default();
        meshes.insert_keyed("Dog body", template.mesh());
        let mut skins = MeshSkins::default();
        let skin = MeshSkin {
            joints: template.joints.clone(),
            weights: template.weights.clone(),
        };
        skins.0.insert("Dog body".into(), Arc::new(skin));
        let mut pose = crate::demo::skeleton(&crate::demo::DOG, crate::demo::Rig::Dog);
        pose.bones = template.bind.clone();
        (template, meshes, skins, pose)
    }

    fn dog_coat(guides: u32, zones: Vec<FurZone>, bones: &[&str], skip: &[&str]) -> HairEmitter {
        HairEmitter::Surface {
            mesh: "Dog body".into(),
            guides,
            flow_from: "head".into(),
            gravity: 0.3,
            zones,
            bones: bones.iter().map(|b| b.to_string()).collect(),
            skip_bones: skip.iter().map(|b| b.to_string()).collect(),
        }
    }

    #[test]
    fn surface_fur_covers_the_dog_and_follows_its_bones() {
        use crate::demo::body::Dog;
        let (template, meshes, skins, pose) = dog_body();
        let tail = ["tail_1", "tail_2", "tail_3"];
        let emitter = dog_coat(5200, crate::demo::dog_fur_zones(), &[], &tail);
        let roots = emit_roots(&emitter, &pose, Some(&meshes), Some(&skins)).unwrap();
        assert!(roots.len() > 4600, "{} roots", roots.len());
        // The tail has its own groom.
        let tail_fur = dog_coat(2600, Vec::new(), &tail, &[]);
        let tail_roots = emit_roots(&tail_fur, &pose, Some(&meshes), Some(&skins)).unwrap();
        let on_tail = |b: usize| (Dog::TAIL_I..Dog::TAIL_I + 3).contains(&b);
        assert!(tail_roots.iter().all(|r| on_tail(r.bone)));
        let model = |r: &Root| {
            let (t, q) = root_frame(r, &pose, &pose);
            (t + q * r.pos, q * r.normal, q * r.comb)
        };
        // No bare skin: every vertex has a root within 2.5 cm.
        let all = || roots.iter().chain(&tail_roots);
        let roots_at: Vec<Vec3> = all().map(|r| model(r).0).collect();
        let root_grid = PointGrid::new(&roots_at);
        for p in &template.positions {
            let d = root_grid.nearest(*p).1;
            assert!(d < 0.025, "bare skin at {p}: nearest root {d} m away");
        }
        assert!(template.positions.len() > 1000);
        // Every root moves with the skin under it: it's bound to the bone that carries most
        // of that skin, never to one that barely moves it.
        let vertex_grid = PointGrid::new(&template.positions);
        let mut carried = Vec::new();
        for (r, p) in all().zip(&roots_at) {
            let (i, d) = vertex_grid.nearest(*p);
            assert!(d < 0.02, "root {p} is {d} m off the skin");
            let (j, w) = (template.joints[i], template.weights[i]);
            let weight: f32 = (0..4)
                .filter(|&k| j[k] as usize == r.bone)
                .map(|k| w[k])
                .sum();
            assert!(
                weight > 0.15,
                "root {p} bound to a bone with skin weight {weight}"
            );
            carried.push(weight);
        }
        let mean_weight = carried.iter().sum::<f32>() / carried.len() as f32;
        assert!(
            mean_weight > 0.85,
            "mean skin weight of the bound bones {mean_weight}"
        );
        // Roots sit on the skin facing out, combed along it; the muzzle's fur is short and the
        // chest's long; each root is bound to a nearby bone.
        let (mut muzzle, mut chest) = (Vec::new(), Vec::new());
        for r in &roots {
            let (p, n, comb) = model(r);
            assert!(n.is_normalized() && comb.is_normalized());
            assert!(
                comb.dot(n).abs() < 1e-3,
                "fur at {p} combed out of the skin"
            );
            let b = pose.bones[r.bone].translation;
            assert!(p.distance(b) < 0.3, "root {p} bound to bone at {b}");
            if p.z < -0.64 {
                muzzle.push(r.length);
            }
            if (0.3..0.38).contains(&p.y) && p.x.abs() < 0.05 && (p.z + 0.22).abs() < 0.08 {
                chest.push(r.length);
            }
        }
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        assert!(!muzzle.is_empty() && !chest.is_empty());
        let (muzzle, chest) = (mean(&muzzle), mean(&chest));
        assert!(
            muzzle < 0.6 && chest > 1.15,
            "muzzle {muzzle}, chest {chest}"
        );
    }

    #[test]
    fn surface_fur_stays_on_the_skin_as_the_dog_trots() {
        let (template, meshes, skins, rest) = dog_body();
        let coat = dog_coat(5200, Vec::new(), &[], &[]);
        let roots = emit_roots(&coat, &rest, Some(&meshes), Some(&skins)).unwrap();
        let blended = roots.iter().filter(|r| r.blend.is_some()).count();
        assert!(
            blended > roots.len() / 10,
            "{blended} of {} roots blended",
            roots.len()
        );
        // Built, the coat rides a modest number of frames and partly follows the hips.
        let groom = HairGroom {
            emitters: vec![coat],
            style: HairStyle::fur(),
            sim: HairGroom::hair_settings(),
        };
        let owner = GlobalTransform::default();
        let skeleton = Some((Entity::PLACEHOLDER, &owner, &rest));
        let body = build(&groom, skeleton, Some(&meshes), Some(&skins)).unwrap();
        assert!(body.frames.len() < 400, "{} frames", body.frames.len());
        let (follow, _) = body.sim.params.follow.unwrap();
        let hips = crate::demo::body::Dog::HIPS_I;
        assert!(
            matches!(body.frames[follow as usize], FrameSource::Bone { bone, .. } if bone == hips),
            "follows {:?}",
            body.frames[follow as usize]
        );
        let mut pose = rest.clone();
        let mut skinned = template.mesh().vertices;
        let mut scratch = Vec::new();
        for phase in [0.1, 0.35, 0.6, 0.85] {
            crate::demo::pose_rig(crate::demo::Rig::Dog, phase * TAU, &mut pose);
            crate::demo::skin::skin(&template, &pose.bones, &mut skinned, &mut scratch);
            let skin: Vec<Vec3> = skinned.iter().map(|v| Vec3::from(v.position)).collect();
            let at: Vec<Vec3> = roots
                .iter()
                .map(|r| {
                    let (t, q) = root_frame(r, &rest, &pose);
                    t + q * r.pos
                })
                .collect();
            // Roots stay on the skin, neither sunk into it at the bent joints nor lifted off...
            let vertex_grid = PointGrid::new(&skin);
            let mut off = Vec::new();
            for p in &at {
                let (i, d) = vertex_grid.nearest(*p);
                assert!(
                    d < 0.025,
                    "phase {phase}: root {p} drifted {d} m off the skin"
                );
                off.push((*p - skin[i]).dot(Vec3::from(skinned[i].normal)).abs());
            }
            off.sort_by(f32::total_cmp);
            let (p99, max) = (off[off.len() * 99 / 100], off[off.len() - 1]);
            assert!(
                p99 < 0.003 && max < 0.008,
                "phase {phase}: off the skin by {p99} (99%), {max}"
            );
            // ...and still cover all of it.
            let root_grid = PointGrid::new(&at);
            for p in &skin {
                let d = root_grid.nearest(*p).1;
                assert!(
                    d < 0.025,
                    "phase {phase}: bare skin at {p}, nearest root {d} m away"
                );
            }
        }
    }
}
