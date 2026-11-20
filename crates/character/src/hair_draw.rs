//! People's scalp hair drawn by the one strand renderer (`engine_render::strands`), the
//! renderer hair and fur share (engine-design/render-perf/strand-renderer.md): every hair with
//! real coverage, lit once per point, shadowed by the hair around it.
//!
//! Each person's scalp [`StrandSet`](crate::strands::StrandSet) becomes a [`StrandGroom`] in
//! the skin's rest space, worn by a child entity that rides the head bone ([`DrawnHair`]):
//! the head carries the hair exactly, and hair that moves takes the rest from its guides,
//! posed by the hair simulation (`HairGuidesPosed` on the person) and brought into the head's
//! frame when the renderer draws it ([`guides_now`]). Brows, lashes and beards follow the face's
//! expressions and are still drawn by the character feature.
//!
//! Only near the camera ([`SharedHairNear`]): past [`lod::LOCKS_TO`] the character feature has
//! long drawn a head of hair as its locks alone (locks.rs), cheap enough for a crowd, and the
//! strand renderer draws the nearest few dozen grooms only.

use engine_assets::{Assets, Handle};
use engine_core::math::{Mat4, Vec3};
use engine_core::prelude::*;
use engine_render::{Camera, StrandFollow, StrandGroom, StrandGuidesWhenDrawn, StrandInstance};
use engine_world::animation::SkeletonPose;
use engine_world::{HairGuides, HairGuidesPosed};

use crate::BuiltCharacter;
use crate::build::CharacterMeshes;
use crate::lod::{self, CharacterLod};
use crate::strands::{PackedStrands, StrandKind, StrandSet};

/// Whether hair of this kind is drawn by the strand renderer (and not by the character
/// feature). `ENGINE_CHARACTER_HAIR=old` keeps all of it in the character feature (to compare).
pub fn shared(kind: StrandKind) -> bool {
    static OLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let old = *OLD.get_or_init(|| std::env::var("ENGINE_CHARACTER_HAIR").is_ok_and(|v| v == "old"));
    kind == StrandKind::Scalp && !old
}

/// The people whose scalp hair the strand renderer draws this frame (and the character feature
/// does not): those within [`NEAR_IN`] of the camera, until they are past [`NEAR_OUT`].
#[derive(Resource, Debug, Default)]
pub struct SharedHairNear(pub std::collections::HashSet<Entity>);

/// Where the strand renderer takes a person's hair over from their locks: where the feature
/// shows the locks alone. A little farther before it hands them back, so a person standing at
/// the line doesn't flicker between the two.
pub const NEAR_IN: f32 = lod::LOCKS_TO;
pub const NEAR_OUT: f32 = lod::LOCKS_TO + 0.3;

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
}

/// How rough the cuticle of scalp hair is (brows and beard: 0.42).
const SCALP_ROUGHNESS: f32 = 0.33;

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
        tips.push([tip.x, tip.y, tip.z, SCALP_ROUGHNESS]);
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

/// The head bone's skinning matrix (the skin's rest space to the person's model space) in
/// `pose`; identity without a rig or a pose (the skin is drawn at rest then).
fn head_matrix(
    meshes: &CharacterMeshes,
    pose: Option<&SkeletonPose>,
    palette: &mut Vec<Mat4>,
) -> Mat4 {
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
    )>,
    mut drawn: Query<(Entity, &DrawnHair, &mut Transform, &mut StrandInstance)>,
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
    for (person, _, _, at) in &people {
        let distance = match (eye, at) {
            (Some(eye), Some(at)) => (at.0.translation - eye).length() as f32 * scale,
            _ => 0.0,
        };
        let reach = if was.contains(&person) {
            NEAR_OUT
        } else {
            NEAR_IN
        };
        if distance < reach {
            near.0.insert(person);
        }
    }
    let mut have: Vec<(Entity, u64)> = Vec::new();
    for (e, hair, mut t, mut instance) in &mut drawn {
        match people.get(hair.person) {
            Ok((_, built, pose, _)) if built.version == hair.version => {
                let shown = near.0.contains(&hair.person);
                if instance.visible != shown {
                    instance.visible = shown;
                }
                if shown {
                    let next = to_transform(head_matrix(&built.meshes, pose, &mut palette));
                    if *t != next {
                        *t = next;
                    }
                }
                have.push((hair.person, hair.version));
            }
            _ => commands.entity(e).despawn(),
        }
    }
    for (person, built, pose, _) in &people {
        if have.contains(&(person, built.version)) {
            continue;
        }
        let m = &built.meshes;
        let head = head_matrix(m, pose, &mut palette);
        let pigment = [
            built.uniform.hair[0],
            built.uniform.hair[1],
            built.uniform.hair[2],
        ];
        for (i, set) in m.strands.iter().enumerate() {
            if !shared(set.kind) {
                continue;
            }
            let Some(packed) = m.packed_strands.get(i) else {
                continue;
            };
            let Some(g) = groom(set, packed, pigment) else {
                continue;
            };
            let moves = !g.guides.is_empty();
            let rest_guides = g.guides.iter().map(|p| Vec3::from_array(*p)).collect();
            let handle: Handle<StrandGroom> = grooms.add(g);
            let mut child = commands.spawn((
                Name::new("Hair"),
                to_transform(head),
                StrandInstance {
                    visible: near.0.contains(&person),
                    ..StrandInstance::new(handle)
                },
                DrawnHair {
                    person,
                    version: built.version,
                    set: i,
                    rest_guides,
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
    fn the_hand_over_to_locks_has_a_margin() {
        const { assert!(NEAR_OUT > NEAR_IN && NEAR_IN >= lod::LOCKS_TO) };
    }
}
