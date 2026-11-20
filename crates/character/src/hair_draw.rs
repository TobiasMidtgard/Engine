//! People's scalp hair drawn by the one strand renderer (`engine_render::strands`), the
//! renderer hair and fur share (engine-design/render-perf/strand-renderer.md): every hair with
//! real coverage, lit once per point, shadowed by the hair around it.
//!
//! Each person's scalp [`StrandSet`](crate::strands::StrandSet) becomes a [`StrandGroom`] in
//! the skin's rest space, worn by a child entity that rides the head bone ([`DrawnHair`]):
//! the head carries the hair exactly, and hair that moves takes the rest from its guides,
//! posed by the hair simulation (`HairGuidesPosed` on the person) and brought into the head's
//! frame when the renderer draws it ([`guides_now`]). Brows and beards follow the face: their
//! hairs go with guides made from the skin vertices they grow from, which the face's expressions
//! and the jaw move ([`FaceFollow`]). Lashes (they blink) and stubble are still drawn by the
//! character feature.
//!
//! Only near the camera ([`SharedHairNear`]): past [`lod::LOCKS_TO`] the character feature has
//! long drawn a head of hair as its locks alone (locks.rs), cheap enough for a crowd, and the
//! strand renderer draws the nearest few dozen grooms only.

use engine_assets::{Assets, Handle};
use engine_core::math::{Mat4, Vec3};
use engine_core::prelude::*;
use engine_render::{Camera, StrandFollow, StrandGroom, StrandGuidesWhenDrawn, StrandInstance};
use engine_world::animation::SkeletonPose;
use engine_world::character::FaceControls;
use engine_world::{HairGuides, HairGuidesPosed};

use crate::BuiltCharacter;
use crate::build::CharacterMeshes;
use crate::face::{self, MAX_CHANNELS};
use crate::lod::{self, CharacterLod};
use crate::strands::{PackedStrands, StrandKind, StrandSet};

/// Whether hair of this kind, `points` points a hair, is drawn by the strand renderer near the
/// camera (and not by the character feature): scalp hair, brows and beards, but not stubble (three
/// points, a millimetre or two), whose hairs are far thinner than a pixel and vanish into the
/// skin there; the character feature's marks read as stubble. `ENGINE_CHARACTER_HAIR=old` keeps
/// all of it in the character feature (to compare).
pub fn shared(kind: StrandKind, points: u32) -> bool {
    static OLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let old = *OLD.get_or_init(|| std::env::var("ENGINE_CHARACTER_HAIR").is_ok_and(|v| v == "old"));
    let face = matches!(kind, StrandKind::Beard | StrandKind::Brows);
    (kind == StrandKind::Scalp || (face && points > 3)) && !old
}

/// The people whose hair the strand renderer draws this frame (and the character feature does
/// not): the nearest [`MAX_NEAR`] within [`NEAR_IN`] of the camera, until they are past
/// [`NEAR_OUT`].
#[derive(Resource, Debug, Default)]
pub struct SharedHairNear(pub std::collections::HashSet<Entity>);

/// Where the strand renderer takes a person's hair over from their locks: as far as the
/// feature kept a person's single hairs on the GPU (so the video memory for hair is what it
/// was), where a head is a few dozen pixels high and the locks' coarser look shows little (at
/// 3.5 m the change from hairs to locks showed). A little farther before it hands them back,
/// so a person standing at the line doesn't flicker between the two.
pub const NEAR_IN: f32 = lod::HAIRS_KEPT;
pub const NEAR_OUT: f32 = NEAR_IN + 0.5;
/// How far a person's grooms are kept once made (and made again only when they come back
/// within `NEAR_IN`): a long head of hair is some 50 MB of points, built on the frame.
pub const KEEP_GROOMS: f32 = NEAR_OUT + 2.0;
/// People drawn by the strand renderer at once, the nearest first: it draws the nearest 32
/// grooms (animals' coats among them), and a person beyond that would lose their hair.
pub const MAX_NEAR: usize = 24;

/// A person's hair drawn by the strand renderer: on a child of the person, at the head bone.
#[derive(Component, Debug, Clone)]
pub struct DrawnHair {
    pub person: Entity,
    /// The build it was made from (`BuiltCharacter::version`).
    pub version: u64,
    /// The strand set it draws (`CharacterMeshes::strands`).
    pub set: usize,
    /// The groom's guides at rest, the skin's rest space.
    rest_guides: Vec<Vec3>,
    /// Hair that follows the face (beards); `None` for hair the simulation moves.
    face: Option<FaceFollow>,
}

/// The guides of hair that follows the face: one per skin vertex its hairs grow from, which
/// moves as that vertex does (its expressions, then its bones), and its hairs with it.
#[derive(Debug, Clone, Default)]
pub struct FaceFollow {
    /// Per guide, the skin vertex.
    roots: Vec<u32>,
    /// The guides this frame, in the head's frame (`draw_hair`).
    now: Vec<Vec3>,
}

/// How rough the cuticle of scalp hair is, and of brows and beard.
const SCALP_ROUGHNESS: f32 = 0.33;
const FACE_ROUGHNESS: f32 = 0.42;

/// Absorption of a hair fibre per unit length (relative) from its pigments, as measured for
/// real hair (d'Eon et al. 2011): eumelanin browns and blacks, pheomelanin reds. As the
/// character shaders' `hair_absorption`.
fn absorption(eumelanin: f32, red: f32) -> Vec3 {
    let eu = 7.0 * eumelanin.max(0.0).powf(2.2) + 0.4 * eumelanin;
    let pheo = red * (0.3 + 1.2 * (1.0 - eumelanin));
    Vec3::new(0.419, 0.697, 1.37) * eu + Vec3::new(0.187, 0.4, 1.05) * pheo
}

/// The color of hair under white light (as the character shaders' `hair_color`): pigment, or
/// white for the share of grey hairs.
pub fn hair_color(eumelanin: f32, red: f32, grey: f32, random: f32) -> Vec3 {
    let sigma = absorption(eumelanin * (0.85 + 0.3 * random), red);
    let pigmented = Vec3::new(
        (-sigma.x * 1.6).exp(),
        (-sigma.y * 1.6).exp(),
        (-sigma.z * 1.6).exp(),
    ) * 0.92
        + Vec3::new(0.012, 0.0115, 0.011);
    let white = Vec3::new(0.78, 0.76, 0.72) * (0.85 + 0.25 * random);
    if (random * 7.31).fract() < grey * 0.85 {
        white
    } else {
        pigmented
    }
}

/// How much darker or fairer than the head's color one hair is, from its random value: most
/// within a fifth, one in six about a third darker.
fn strand_shade(random: f32) -> f32 {
    let a = (random * 12.9898 + 0.137).sin() * 43758.547;
    let h = a - a.floor();
    let b = (random * 78.233 + 0.531).sin() * 24634.635;
    let dark = if b - b.floor() < 0.17 { 0.68 } else { 1.0 };
    (0.82 + 0.36 * h) * dark
}

/// The groom of a strand set, from its GPU packing (the full-precision points are released
/// once packed). `pigment`: eumelanin, red, grey.
pub fn groom(set: &StrandSet, packed: &PackedStrands, pigment: [f32; 3]) -> Option<StrandGroom> {
    let per = set.points_per as usize;
    if per < 2 || packed.points.is_empty() || !packed.points.len().is_multiple_of(per) {
        return None;
    }
    let (lo, size) = (packed.box_min, packed.box_size);
    let unorm = |v: u32| (v & 0xffff) as f32 / 65535.0;
    let points: Vec<[f32; 4]> = packed
        .points
        .iter()
        .map(|q| {
            [
                lo[0] + unorm(q[0]) * size[0],
                lo[1] + unorm(q[0] >> 16) * size[1],
                lo[2] + unorm(q[1]) * size[2],
                unorm(q[1] >> 16) * lo[3],
            ]
        })
        .collect();
    let strands = points.len() / per;
    let [eu, red, grey] = pigment;
    let mut roots = Vec::with_capacity(strands);
    let mut tips = Vec::with_capacity(strands);
    for s in 0..strands {
        let random = packed.attrs.get(s * per).map_or(0.5, |a| unorm(a >> 16));
        // No two hairs on a head are quite the same shade: some a good deal darker, a few
        // fairer (what lets single hairs show inside a lock instead of one painted sheet).
        let shade = strand_shade(random);
        let root = hair_color(eu, red, grey, random) * shade;
        // Toward the tips hair is older, weathered by sun and combing: a little fairer.
        let tip = hair_color(eu * 0.9, red, grey, random) * shade;
        roots.push([root.x, root.y, root.z, random]);
        let rough = if set.kind == StrandKind::Scalp {
            SCALP_ROUGHNESS
        } else {
            FACE_ROUGHNESS
        };
        tips.push([tip.x, tip.y, tip.z, rough]);
    }
    let follow = if set.follow.len() == strands {
        set.follow
            .iter()
            .map(|f| StrandFollow {
                guides: f.guides,
                weights: f.weights,
            })
            .collect()
    } else {
        Vec::new()
    };
    let g = StrandGroom {
        points_per: set.points_per,
        points,
        roots,
        tips,
        guides: if follow.is_empty() {
            Vec::new()
        } else {
            set.guides.iter().map(|p| p.to_array()).collect()
        },
        follow,
        ..Default::default()
    };
    g.is_valid().then_some(g)
}

/// Guides for hair that follows the face (a groom without guides of its own): the first hair
/// grown from each skin vertex stands for all hairs from it. `roots`: per hair, its skin vertex.
/// Returns each guide's vertex (empty when `roots` doesn't fit the groom).
pub fn face_guides(g: &mut StrandGroom, roots: &[u32]) -> Vec<u32> {
    let per = g.points_per as usize;
    let strands = g.strand_count();
    if !g.guides.is_empty() || roots.len() != strands || per < 2 {
        return Vec::new();
    }
    let mut of_vertex = std::collections::HashMap::new();
    let mut guide_roots = Vec::new();
    g.follow = roots
        .iter()
        .enumerate()
        .map(|(h, &v)| {
            let guide = *of_vertex.entry(v).or_insert_with(|| {
                guide_roots.push(v);
                g.guides.extend(
                    g.points[h * per..(h + 1) * per]
                        .iter()
                        .map(|p| [p[0], p[1], p[2]]),
                );
                guide_roots.len() as u32 - 1
            });
            StrandFollow {
                guides: [guide, u32::MAX, u32::MAX],
                weights: [1.0, 0.0, 0.0],
            }
        })
        .collect();
    guide_roots
}

/// A half float's value from the low 16 bits (the expression shapes' packing).
fn half(bits: u32) -> f32 {
    face::f16_value(bits as u16)
}

/// Where the face's guides are this frame, in the head's frame: each guide's points carried as
/// its skin vertex is, by `weights` (the expression channels) and the bones in `palette`.
fn pose_face_guides(
    meshes: &CharacterMeshes,
    rest: &[Vec3],
    face: &mut FaceFollow,
    weights: &[f32; MAX_CHANNELS],
    palette: &[Mat4],
    to_head: Mat4,
) {
    let per = rest.len() / face.roots.len().max(1);
    face.now.clear();
    for (g, &v) in face.roots.iter().enumerate() {
        let at = v as usize;
        let mut dp = Vec3::ZERO;
        let shapes = &meshes.face;
        if let (Some(&a), Some(&b)) = (shapes.offsets.get(at), shapes.offsets.get(at + 1)) {
            for e in shapes.entries.get(a as usize..b as usize).unwrap_or(&[]) {
                let w = weights
                    .get((e[0] & 0xffff) as usize)
                    .copied()
                    .unwrap_or(0.0);
                if w != 0.0 {
                    dp += Vec3::new(half(e[0] >> 16), half(e[1]), half(e[1] >> 16)) * w;
                }
            }
        }
        let skin = if palette.is_empty() {
            Mat4::IDENTITY
        } else {
            let bones = meshes.skin.bones.get(at).copied().unwrap_or_default();
            let w = meshes
                .skin
                .weights
                .get(at)
                .copied()
                .unwrap_or([255, 0, 0, 0]);
            let mut m = Mat4::ZERO;
            for (b, w) in bones.iter().zip(w) {
                m += palette.get(*b as usize).copied().unwrap_or(Mat4::IDENTITY)
                    * (w as f32 / 255.0);
            }
            m
        };
        let m = to_head * skin;
        face.now.extend(
            rest[g * per..(g + 1) * per]
                .iter()
                .map(|p| m.transform_point3(*p + dp)),
        );
    }
}

/// The head bone's skinning matrix (the skin's rest space to the person's model space) in
/// `pose`; identity without a rig or a pose (the skin is drawn at rest then).
fn head_matrix(
    meshes: &CharacterMeshes,
    pose: Option<&SkeletonPose>,
    palette: &mut Vec<Mat4>,
) -> Mat4 {
    palette.clear();
    let (Some(rig), Some(pose)) = (meshes.rig.as_ref(), pose) else {
        return Mat4::IDENTITY;
    };
    let Some(head) = rig.bones.iter().position(|b| b == "head") else {
        return Mat4::IDENTITY;
    };
    rig.palette(pose, &rig.map(pose), palette);
    palette.get(head).copied().unwrap_or(Mat4::IDENTITY)
}

fn to_transform(m: Mat4) -> Transform {
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    Transform {
        translation: translation.as_dvec3(),
        rotation,
        scale,
    }
}

/// `PostUpdate`, before transforms propagate: give each built person's shared hair its
/// grooms and instances (again after a rebuild), and keep them on the head bone.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn draw_hair(
    mut commands: Commands,
    people: Query<(
        Entity,
        &BuiltCharacter,
        Option<&SkeletonPose>,
        Option<&GlobalTransform>,
        Option<&FaceControls>,
    )>,
    mut drawn: Query<(Entity, &mut DrawnHair, &mut Transform, &mut StrandInstance)>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    grooms: Option<ResMut<Assets<StrandGroom>>>,
    lod_scale: Option<Res<CharacterLod>>,
    mut near: ResMut<SharedHairNear>,
    mut palette: Local<Vec<Mat4>>,
) {
    let Some(mut grooms) = grooms else {
        near.0.clear();
        return;
    };
    // The camera on screen: the active one of the highest order (none: everyone is near).
    let eye = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, g)| g.0.translation);
    let scale = lod_scale.map_or(1.0, |l| l.distance_scale);
    let was = std::mem::take(&mut near.0);
    let mut by_distance: Vec<(f32, Entity)> = people
        .iter()
        .map(|(person, _, _, at, _)| {
            let distance = match (eye, at) {
                (Some(eye), Some(at)) => (at.0.translation - eye).length() as f32 * scale,
                _ => 0.0,
            };
            (distance, person)
        })
        .collect();
    by_distance.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.0.extend(
        by_distance
            .iter()
            .filter(|(d, p)| *d < if was.contains(p) { NEAR_OUT } else { NEAR_IN })
            .take(MAX_NEAR)
            .map(|(_, p)| *p),
    );
    let kept = |person: Entity| {
        near.0.contains(&person)
            || by_distance
                .iter()
                .any(|(d, p)| *p == person && *d < KEEP_GROOMS)
    };
    let mut have: Vec<(Entity, u64)> = Vec::new();
    let mut weights = [0.0f32; MAX_CHANNELS];
    for (e, mut hair, mut t, mut instance) in &mut drawn {
        match people.get(hair.person) {
            Ok((_, built, pose, _, face_controls))
                if built.version == hair.version && kept(hair.person) =>
            {
                let shown = near.0.contains(&hair.person);
                if instance.visible != shown {
                    instance.visible = shown;
                }
                if shown {
                    let head = head_matrix(&built.meshes, pose, &mut palette);
                    let next = to_transform(head);
                    if *t != next {
                        *t = next;
                    }
                    let hair = &mut *hair;
                    if let Some(follow) = hair.face.as_mut() {
                        match face_controls {
                            Some(f) => face::channel_weights(f, &mut weights),
                            None => weights.fill(0.0),
                        }
                        pose_face_guides(
                            &built.meshes,
                            &hair.rest_guides,
                            follow,
                            &weights,
                            &palette,
                            head.inverse(),
                        );
                    }
                }
                have.push((hair.person, hair.version));
            }
            _ => commands.entity(e).despawn(),
        }
    }
    for (person, built, pose, _, _) in &people {
        if have.contains(&(person, built.version)) || !near.0.contains(&person) {
            continue;
        }
        let m = &built.meshes;
        let head = head_matrix(m, pose, &mut palette);
        for (i, set) in m.strands.iter().enumerate() {
            if !shared(set.kind, set.points_per) {
                continue;
            }
            let Some(packed) = m.packed_strands.get(i) else {
                continue;
            };
            let p = match set.kind {
                StrandKind::Beard => built.uniform.beard,
                StrandKind::Brows => built.uniform.brows,
                _ => built.uniform.hair,
            };
            let Some(mut g) = groom(set, packed, [p[0], p[1], p[2]]) else {
                continue;
            };
            // A thin layer, with next to no hair behind it to scatter light back.
            g.thin = set.kind == StrandKind::Brows;
            // Hair the simulation doesn't move follows the face (brows, beards).
            let face = (set.kind != StrandKind::Scalp).then(|| {
                let roots = m.strand_roots.get(i).map_or(&[][..], |r| &r[..]);
                FaceFollow {
                    roots: face_guides(&mut g, roots),
                    now: Vec::new(),
                }
            });
            if !g.is_valid() {
                continue;
            }
            let moves = !g.guides.is_empty();
            let rest_guides = g.guides.iter().map(|p| Vec3::from_array(*p)).collect();
            let handle: Handle<StrandGroom> = grooms.add(g);
            let mut child = commands.spawn((
                Name::new("Hair"),
                to_transform(head),
                StrandInstance {
                    visible: near.0.contains(&person),
                    // Brow hairs are coarser than scalp hair, and so few that a thinner
                    // sprinkle of them lets the skin show through as a pale brow.
                    width_scale: if set.kind == StrandKind::Brows {
                        1.3
                    } else {
                        1.0
                    },
                    ..StrandInstance::new(handle)
                },
                DrawnHair {
                    person,
                    version: built.version,
                    set: i,
                    rest_guides,
                    face,
                },
                ChildOf(person),
            ));
            if moves {
                child.insert(StrandGuidesWhenDrawn(guides_now));
            }
        }
    }
}

/// The guides of a person's drawn hair as the hair simulation left them, in the head's frame
/// (the instance's model space): `false` while the simulation has none for this build.
fn guides_now(world: &World, entity: Entity, out: &mut Vec<Vec3>) -> bool {
    let Some(hair) = world.get::<DrawnHair>(entity) else {
        return false;
    };
    if let Some(face) = &hair.face {
        if face.now.len() != hair.rest_guides.len() || face.now.is_empty() {
            return false;
        }
        out.clear();
        out.extend_from_slice(&face.now);
        return true;
    }
    let (Some(guides), Some(posed), Some(t)) = (
        world.get::<HairGuides>(hair.person),
        world.get::<HairGuidesPosed>(hair.person),
        world.get::<Transform>(entity),
    ) else {
        return false;
    };
    if posed.version != guides.version
        || posed.version != hair.version as u32
        || posed.points.len() != hair.rest_guides.len()
    {
        return false;
    }
    // The person's model space to the head's frame (the skin's rest space riding the head).
    let to_head =
        Mat4::from_scale_rotation_translation(t.scale, t.rotation, t.translation.as_vec3())
            .inverse();
    out.clear();
    out.extend(posed.points.iter().map(|p| to_head.transform_point3(*p)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A set of `n` straight hairs of four points each, packed as the build packs them.
    fn packed_set(n: usize) -> (StrandSet, PackedStrands) {
        let mut set = StrandSet::new(StrandKind::Scalp, 4);
        let mut packed = PackedStrands {
            box_min: [0.0, 0.0, 0.0, 1e-4],
            box_size: [0.1, 0.1, 0.1, 0.0],
            ..Default::default()
        };
        for s in 0..n {
            let random = ((s as f32 + 0.5) / n as f32 * 65535.0) as u32;
            for k in 0..4u32 {
                let x = (s as u32 * 997) % 65535;
                packed
                    .points
                    .push([x | ((k * 16000) << 16), 30000 | (65535 << 16)]);
                packed.attrs.push(65535 | (random << 16));
            }
        }
        set.released = n;
        (set, packed)
    }

    #[test]
    fn groom_decodes_the_packed_points_and_varies_each_hair() {
        let (set, packed) = packed_set(64);
        let g = groom(&set, &packed, [0.3, 0.1, 0.0]).expect("a valid groom");
        assert_eq!(g.points.len(), 64 * 4);
        assert_eq!(g.roots.len(), 64);
        // Along each hair the points climb in y and keep the packed radius.
        assert!(g.points[1][1] > g.points[0][1]);
        assert!((g.points[0][3] - 1e-4).abs() < 1e-7);
        // Hairs differ in shade, by no more than the variation allows.
        let lum: Vec<f32> = g.roots.iter().map(|c| c[0] + c[1] + c[2]).collect();
        let (lo, hi) = lum
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &l| (a.min(l), b.max(l)));
        assert!(hi > lo * 1.2, "shades {lo}..{hi}");
        assert!(hi < lo * 3.0, "shades {lo}..{hi}");
        // Without guides nothing moves.
        assert!(g.guides.is_empty() && g.follow.is_empty());
    }

    #[test]
    fn strand_shade_stays_in_range() {
        let mut dark = 0;
        for i in 0..1000 {
            let s = strand_shade(i as f32 / 1000.0);
            assert!((0.68 * 0.82 - 1e-4..=1.18 + 1e-4).contains(&s), "{s}");
            dark += (s < 0.81) as u32;
        }
        assert!((80..=300).contains(&dark), "{dark} dark hairs in 1000");
    }

    #[test]
    fn face_guides_take_one_hair_per_skin_vertex() {
        let (set, packed) = packed_set(6);
        let mut g = groom(&set, &packed, [0.3, 0.1, 0.0]).unwrap();
        let roots = face_guides(&mut g, &[7, 7, 9, 7, 9, 11]);
        assert_eq!(roots, vec![7, 9, 11]);
        assert_eq!(g.guides.len(), 3 * 4);
        let guide_of: Vec<u32> = g.follow.iter().map(|f| f.guides[0]).collect();
        assert_eq!(guide_of, vec![0, 0, 1, 0, 1, 2]);
        // Each guide is the first hair grown from its vertex.
        assert_eq!(
            g.guides[4],
            [g.points[8][0], g.points[8][1], g.points[8][2]]
        );
        assert!(g.is_valid());
        // Roots that don't fit the groom give no guides.
        let mut h = groom(&set, &packed, [0.3, 0.1, 0.0]).unwrap();
        assert!(face_guides(&mut h, &[1, 2]).is_empty() && h.guides.is_empty());
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(0x3555), 0.333_251_95);
        assert_eq!(half(0), 0.0);
        assert!((half(crate::face::f16_bits(0.0123) as u32) - 0.0123).abs() < 1e-5);
    }

    #[test]
    fn the_hand_over_to_locks_has_a_margin() {
        const { assert!(KEEP_GROOMS > NEAR_OUT && NEAR_OUT > NEAR_IN && NEAR_IN >= lod::LOCKS_TO) };
    }
}
