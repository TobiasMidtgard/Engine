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
//! drawn hairs. Fur is the same thing with short, stiff guides spread over body capsules.

use engine_core::prelude::*;

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::cloth::SkeletonRef;
use crate::settings::{SimSettings, SoftMaterial};
use crate::solver::{Goal, Sim, Tether};

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
            ..SimSettings::default()
        }
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
    Ellipsoid { center: Vec3, radii: Vec3 },
    Capsule { a: Vec3, b: Vec3, radius: f32 },
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
        }
    }
}

/// A root in bone space: position, normal, comb direction, and the skin it grows from.
struct Root {
    bone: usize,
    pos: Vec3,
    normal: Vec3,
    comb: Vec3,
    skin: Skin,
}

const GOLDEN_ANGLE: f32 = 2.399_963;

fn fibonacci_sphere(n: usize) -> impl Iterator<Item = Vec3> {
    (0..n).map(move |i| {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let a = i as f32 * GOLDEN_ANGLE;
        Vec3::new(r * a.cos(), y, r * a.sin())
    })
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
                    pos: center + d * radii,
                    normal: (d / radii).normalize(),
                    comb,
                    skin: Skin::Ellipsoid { center, radii },
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
                            pos: dir * (t * len) + normal * *radius,
                            normal,
                            comb,
                            skin: Skin::Capsule {
                                a: Vec3::ZERO,
                                b: axis,
                                radius: *radius,
                            },
                        });
                    }
                }
                None => {
                    for d in fibonacci_sphere(n) {
                        roots.push(Root {
                            bone: b,
                            pos: d * *radius,
                            normal: d,
                            comb,
                            skin: Skin::Ellipsoid {
                                center: Vec3::ZERO,
                                radii: Vec3::splat(*radius),
                            },
                        });
                    }
                }
            }
        }
    }
    Ok(roots)
}

/// Build hair for `skeleton`. `entity` is the groom's own entity.
pub fn build(spec: &HairGroom, skeleton: Option<SkeletonRef>) -> Result<SoftBodySim, String> {
    let Some((owner, owner_t, pose)) = skeleton else {
        return Err("HairGroom needs a parent (or ancestor) with a SkeletonPose.".into());
    };
    let style = spec.style;
    let segments = style.segments.clamp(1, 32) as usize;
    let origin = owner_t.translation();
    let mut sim = Sim::new(origin);
    let mut frames = Vec::new();
    let mut frame_of_bone: Vec<(usize, u16)> = Vec::new();
    let mut guides = Vec::new();
    let mut rng = Rng::new(0x4A12);
    let thickness = spec.sim.thickness.max(style.spread * 0.5 + 0.003);
    let s = style.stiffness.clamp(0.01, 1.0);
    let (omega_root, omega_tip) = (15.0 + 300.0 * s, 0.5 + 25.0 * s * s);
    let total_mass = 0.0004; // kg per guide (a lock of a few hundred hairs)
    for emitter in &spec.emitters {
        for root in emit_roots(emitter, pose)? {
            let Some((bp, br)) = pose.bone_world(owner_t, root.bone) else {
                continue;
            };
            let frame = match frame_of_bone.iter().find(|(b, _)| *b == root.bone) {
                Some((_, f)) => *f,
                None => {
                    frames.push(FrameSource::Bone {
                        owner,
                        bone: root.bone,
                    });
                    let f = sim.add_frame((bp - origin).as_vec3(), br);
                    frame_of_bone.push((root.bone, f));
                    f
                }
            };
            let length = style.length * (1.0 + style.length_variation * (rng.f32() * 2.0 - 1.0));
            let seg_len = length.max(0.002) / segments as f32;
            // Rest shape in bone space.
            let mut rest = vec![root.pos];
            // Combed hair leaves the skin at a shallow angle, along the comb direction.
            let along = (root.comb - root.normal * root.comb.dot(root.normal)).normalize_or_zero();
            let lift = 1.0 - 0.75 * style.comb.clamp(0.0, 1.0);
            let combed = (root.normal * lift + along * style.comb).normalize_or(root.normal);
            let mut dir = combed;
            let out = Vec3::new(root.normal.x, 0.0, root.normal.z).normalize_or_zero();
            let fall = (Vec3::NEG_Y + out * 0.35 + Vec3::Z * 0.12).normalize();
            let mut p = root.pos;
            for k in 1..=segments {
                let arc = k as f32 * seg_len;
                let bend = smoothstep(0.0, 1.0, arc / (0.03 + 0.1 * (1.0 - style.droop)));
                let target = combed.lerp(fall, bend * style.droop).normalize_or(combed);
                dir = dir.lerp(target, 0.6).normalize_or(target);
                // Drape over the skin instead of falling through it.
                let next = root.skin.outside(p + dir * seg_len, thickness);
                dir = (next - p).normalize_or(dir);
                p += dir * seg_len;
                rest.push(p);
            }
            let mass = total_mass / (segments + 1) as f32;
            let first = sim.len() as u32;
            for q in &rest {
                sim.add_particle((bp - origin).as_vec3() + br * *q, mass);
            }
            for k in 1..=segments as u32 {
                sim.add_distance(first + k - 1, first + k, 0.0, u32::MAX);
            }
            // Two root particles pinned: the root keeps its direction.
            sim.pin(first, frame, rest[0]);
            sim.pin(first + 1, frame, rest[1]);
            let mut arc = seg_len;
            for (k, &local) in rest.iter().enumerate().take(segments + 1).skip(2) {
                arc += seg_len;
                let t = k as f32 / segments as f32;
                let omega =
                    (omega_root.ln() + (omega_tip.ln() - omega_root.ln()) * t.powf(0.6)).exp();
                sim.goals.push(Goal {
                    p: first + k as u32,
                    frame,
                    local,
                    compliance: mass / (omega * omega),
                });
                sim.tethers.push(Tether {
                    p: first + k as u32,
                    anchor: first,
                    max: arc * 1.03,
                });
            }
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
    let head = frame_of_bone[0].1;
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
        let mut body = build(&groom, Some((Entity::PLACEHOLDER, &t, &pose))).unwrap();
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
}
