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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::math::{Mat4, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    Camera, StrandBodyWhenDrawn, StrandBox, StrandCapsule, StrandFollow, StrandGroom,
    StrandGuideState, StrandGuidesWhenDrawn, StrandInstance, StrandKeptOutWhenDrawn,
    StrandStateWhenDrawn,
};
use engine_world::animation::SkeletonPose;
use engine_world::character::{Appearance, FaceControls};
use engine_world::{HairBody, HairGuides, HairGuidesPosed};

use crate::BuiltCharacter;
use crate::build::CharacterMeshes;
use crate::face::{self, MAX_CHANNELS};
use crate::feature::GpuCharacter;
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
    /// The wetness it was made for ([`wet_step`]).
    pub wet: u8,
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

/// Scalp hair drawn this much wider than its fibres: a real head's hundred thousand hairs
/// cover it, and where long hair spreads over the shoulders and thins at its ends ours, at
/// true width and a little under a pixel each at 1440p, let the shirt show through: thin.
const SCALP_WIDTH: f32 = 1.2;

/// How dark the most buried hair is drawn, against hair in the open (its color's share).
const BURIED_DARK: f32 = 0.55;

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
        // How buried the hair is (the build's occlusion, from a 3 mm grid of all the hair):
        // hairs under the outer layer and in the gaps between locks darker. The renderer's own
        // light through the hair comes from a coarser volume (centimetre cells over a head of
        // long hair), which shades the layers but can't part one lock from the next: without
        // this the hair read as one smooth even sheet, thin beside the deep gaps between
        // clumps real hair shows.
        // (Over a stretch of the hair, each point counted by how open it is: a hair shows where
        // it comes to the surface, and there it is in the light. By the plain mean a hair that
        // ran mostly under its lock was dark all along, and wherever it surfaced it drew a dark
        // dash across the lit hair: sunlit fair hair looked hatched.)
        let occ = |from: usize, to: usize| {
            let (mut sum, mut seen) = (0.0, 0.0);
            for k in from..to {
                let o = packed.attrs.get(s * per + k).map_or(1.0, |a| unorm(*a));
                sum += o;
                seen += o * o;
            }
            if sum > 0.0 { seen / sum } else { 0.0 }
        };
        let buried = |o: f32| BURIED_DARK + (1.0 - BURIED_DARK) * o.clamp(0.0, 1.0);
        let root = hair_color(eu, red, grey, random) * shade * buried(occ(0, per.div_ceil(3)));
        // Toward the tips hair is older, weathered by sun and combing: a little fairer.
        let tip = hair_color(eu * 0.9, red, grey, random) * shade * buried(occ(per / 3, per));
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

/// Wet hair, from `Appearance::wear.wet` (0 dry .. 1 soaked): water in and on the cuticle
/// darkens and deepens the color (as wet sand), smooths the cuticle (a sharper, brighter
/// highlight), and draws neighbouring hairs together into pointed clumps. `cell`: about how
/// wide a clump is at the root (m); 0 leaves the hairs where they are.
pub fn wet_groom(g: &mut StrandGroom, wet: f32, cell: f32) {
    let wet = wet.clamp(0.0, 1.0);
    if wet <= 0.0 {
        return;
    }
    let deepen = |c: &mut [f32; 4]| {
        for v in &mut c[..3] {
            *v = v.max(0.0).powf(1.0 + WET_DEEPEN * wet) * (1.0 - WET_DARKEN * wet);
        }
    };
    g.roots.iter_mut().for_each(deepen);
    for t in &mut g.tips {
        deepen(t);
        t[3] *= 1.0 - WET_SMOOTH * wet;
    }
    let per = g.points_per as usize;
    let strands = g.strand_count();
    if cell <= 0.0 || per < 2 || strands == 0 {
        return;
    }
    // Clumps: the hairs rooted in one cell whose tips end near each other (hairs combed apart
    // at a parting stay apart). Each is drawn toward the clump's mean hair, more toward the
    // tip, where water pulls wet hair into points.
    let cell_of = |v: f32, size: f32| (v / size).floor() as i32;
    let key = |s: usize| {
        let (r, t) = (g.points[s * per], g.points[s * per + per - 1]);
        [
            cell_of(r[0], cell),
            cell_of(r[1], cell),
            cell_of(r[2], cell),
            cell_of(t[0], WET_TIP_CELL),
            cell_of(t[1], WET_TIP_CELL),
            cell_of(t[2], WET_TIP_CELL),
        ]
    };
    let mut clumps: HashMap<[i32; 6], (f32, Vec<Vec3>)> = HashMap::new();
    let mut of_hair = Vec::with_capacity(strands);
    for s in 0..strands {
        let k = key(s);
        let (n, mean) = clumps
            .entry(k)
            .or_insert_with(|| (0.0, vec![Vec3::ZERO; per]));
        *n += 1.0;
        for (m, p) in mean.iter_mut().zip(&g.points[s * per..(s + 1) * per]) {
            *m += Vec3::new(p[0], p[1], p[2]);
        }
        of_hair.push(k);
    }
    for (n, mean) in clumps.values_mut() {
        mean.iter_mut().for_each(|m| *m /= *n);
    }
    for (s, k) in of_hair.iter().enumerate() {
        let mean = &clumps[k].1;
        let hair = &mut g.points[s * per..(s + 1) * per];
        let from_mean = Vec3::new(hair[0][0], hair[0][1], hair[0][2]) - mean[0];
        for (i, (p, m)) in hair.iter_mut().zip(mean).enumerate() {
            let along = i as f32 / (per - 1) as f32;
            let to = *m + from_mean * (1.0 - along);
            let pull = WET_CLUMP * wet * along.sqrt();
            let at = Vec3::new(p[0], p[1], p[2]).lerp(to, pull);
            (p[0], p[1], p[2]) = (at.x, at.y, at.z);
        }
    }
}

/// How far apart the tips of one wet clump's hairs may end at most (m), and how strongly a
/// soaked clump's tips meet (1: at one point).
const WET_TIP_CELL: f32 = 0.04;
const WET_CLUMP: f32 = 0.9;
/// Wet hair's color: deeper (a power of the dry color), darker, and its cuticle smoother
/// (wet fair hair reads as dark honey; as the character shaders' `strand_base_color`).
const WET_DEEPEN: f32 = 1.0;
const WET_DARKEN: f32 = 0.25;
const WET_SMOOTH: f32 = 0.45;
/// About how wide a clump of wet scalp hair is at the root, and of beard hair (m).
const WET_SCALP_CELL: f32 = 0.012;
const WET_BEARD_CELL: f32 = 0.007;
/// The wetness a person's grooms are made for: a few steps, so a person getting wetter
/// remakes their grooms a few times, not every frame.
const WET_STEPS: f32 = 3.0;

fn wet_step(appearance: Option<&Appearance>) -> u8 {
    appearance.map_or(0, |a| {
        (a.wear.wet.clamp(0.0, 1.0) * WET_STEPS).round() as u8
    })
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
pub(crate) fn draw_hair(
    mut commands: Commands,
    people: Query<(
        Entity,
        &BuiltCharacter,
        Option<&SkeletonPose>,
        Option<&GlobalTransform>,
        Option<&FaceControls>,
        Option<&Appearance>,
    )>,
    mut drawn: Query<(Entity, &mut DrawnHair, &mut Transform, &mut StrandInstance)>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    grooms: Option<ResMut<Assets<StrandGroom>>>,
    lod_scale: Option<Res<CharacterLod>>,
    mut near: ResMut<SharedHairNear>,
    config: Option<Res<engine_core::config::EngineConfig>>,
    mut pending: Local<HashMap<(Entity, u64, u8), Slot>>,
    mut nothing: Local<std::collections::HashSet<(Entity, u64, u8)>>,
    mut palette: Local<Vec<Mat4>>,
) {
    let off_frame = config.is_some_and(|c| c.off_frame());
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
        .map(|(person, _, _, at, ..)| {
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
    let mut have: Vec<(Entity, u64, u8)> = Vec::new();
    // Hair made for another wetness: drawn until the person's new grooms are made.
    let mut stale: Vec<(Entity, Entity)> = Vec::new();
    let mut weights = [0.0f32; MAX_CHANNELS];
    for (e, mut hair, mut t, mut instance) in &mut drawn {
        match people.get(hair.person) {
            Ok((_, built, pose, _, face_controls, look))
                if built.version == hair.version && kept(hair.person) =>
            {
                if hair.wet == wet_step(look) {
                    have.push((hair.person, hair.version, hair.wet));
                } else {
                    stale.push((e, hair.person));
                }
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
            }
            _ => commands.entity(e).despawn(),
        }
    }
    let mut spawned = Vec::new();
    for (person, built, pose, _, _, look) in &people {
        let wet = wet_step(look);
        let key = (person, built.version, wet);
        if have.contains(&key) || !near.0.contains(&person) || nothing.contains(&key) {
            continue;
        }
        // Built once per build of the person: on a worker while the game runs (a long head
        // of hair is millions of points), in the frame for headless runs (a picture shows
        // everyone it is asked for).
        let made = if off_frame {
            let slot = pending.entry(key).or_insert_with(|| {
                let slot: Slot = Arc::new(Mutex::new(None));
                let (done, meshes, uniform) = (slot.clone(), built.meshes.clone(), built.uniform);
                let spawned = std::thread::Builder::new()
                    .name("character hair grooms".into())
                    .spawn(move || {
                        let _span = tracing::info_span!("character hair grooms").entered();
                        let made = make_grooms(&meshes, &uniform, wet);
                        *done.lock().unwrap_or_else(|e| e.into_inner()) = Some(made);
                    });
                if spawned.is_err() {
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
                }
                slot
            });
            let Some(made) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                continue;
            };
            pending.remove(&key);
            made
        } else {
            make_grooms(&built.meshes, &built.uniform, wet)
        };
        if made.is_empty() {
            // (Bald and clean-shaven, or nothing that makes a groom: not looked at again.)
            nothing.insert(key);
            continue;
        }
        let head = head_matrix(&built.meshes, pose, &mut palette);
        spawned.push(person);
        for (old, _) in stale.iter().filter(|(_, p)| *p == person) {
            commands.entity(*old).despawn();
        }
        for Made {
            set,
            kind,
            groom: g,
            face,
        } in made
        {
            let moves = !g.guides.is_empty();
            let rest_guides = g.guides.iter().map(|p| Vec3::from_array(*p)).collect();
            let handle: Handle<StrandGroom> = grooms.add(g);
            let mut child = commands.spawn((
                Name::new("Hair"),
                to_transform(head),
                StrandInstance {
                    visible: true,
                    // Brow hairs are coarser than scalp hair, and so few that a thinner
                    // sprinkle of them lets the skin show through as a pale brow.
                    width_scale: match kind {
                        StrandKind::Brows => 1.3,
                        StrandKind::Scalp => SCALP_WIDTH,
                        _ => 1.0,
                    },
                    ..StrandInstance::new(handle)
                },
                DrawnHair {
                    person,
                    version: built.version,
                    set,
                    wet,
                    rest_guides,
                    face,
                },
                ChildOf(person),
            ));
            if moves {
                child.insert((
                    StrandGuidesWhenDrawn(guides_now),
                    StrandKeptOutWhenDrawn(kept_out_now),
                    StrandBodyWhenDrawn(body_now),
                    StrandStateWhenDrawn(state_now),
                ));
            }
        }
    }
    // (A build nobody waits for any more: its worker finishes and its grooms are dropped.)
    pending.retain(|(person, version, wet), _| {
        people.get(*person).is_ok_and(|(_, b, .., look)| {
            b.version == *version && wet_step(look) == *wet && near.0.contains(person)
        })
    });
    nothing.retain(|(person, version, wet)| {
        people
            .get(*person)
            .is_ok_and(|(_, b, .., look)| b.version == *version && wet_step(look) == *wet)
    });
    // The character feature leaves a person's hair to the strand renderer only once it has
    // their grooms (it draws their locks while a worker makes them).
    near.0.retain(|p| {
        have.iter().any(|(q, ..)| q == p)
            || stale.iter().any(|(_, q)| q == p)
            || spawned.contains(p)
    });
}

/// A groom made for one of a person's strand sets ([`make_grooms`]).
pub(crate) struct Made {
    set: usize,
    kind: StrandKind,
    groom: StrandGroom,
    face: Option<FaceFollow>,
}

/// Grooms being made on a worker, per person and build.
pub(crate) type Slot = Arc<Mutex<Option<Vec<Made>>>>;

/// The grooms of a person's hair the strand renderer draws ([`shared`]), with the face's
/// guides for hair that follows it.
fn make_grooms(m: &CharacterMeshes, uniform: &GpuCharacter, wet: u8) -> Vec<Made> {
    let wet = wet as f32 / WET_STEPS;
    let mut out = Vec::new();
    for (i, set) in m.strands.iter().enumerate() {
        if !shared(set.kind, set.points_per) {
            continue;
        }
        let Some(packed) = m.packed_strands.get(i) else {
            continue;
        };
        let p = match set.kind {
            StrandKind::Beard => uniform.beard,
            StrandKind::Brows => uniform.brows,
            _ => uniform.hair,
        };
        let Some(mut g) = groom(set, packed, [p[0], p[1], p[2]]) else {
            continue;
        };
        let cell = match set.kind {
            StrandKind::Scalp => WET_SCALP_CELL,
            StrandKind::Beard => WET_BEARD_CELL,
            _ => 0.0,
        };
        // (Hair the simulation moves is wetted lock by lock as it is drawn, by how wet the
        // simulation has each: it takes the person's wetness for the least of it.)
        if set.guide_count() == 0 {
            wet_groom(&mut g, wet, cell);
        }
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
        if g.is_valid() {
            out.push(Made {
                set: i,
                kind: set.kind,
                groom: g,
                face,
            });
        }
    }
    out
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
    let Some((posed, to_head)) = simulated(world, hair, entity) else {
        return false;
    };
    out.clear();
    out.extend(posed.points.iter().map(|p| to_head.transform_point3(*p)));
    true
}

/// The shapes the hair simulation held a person's hair out of (head, neck, shoulders), in the
/// head's frame as [`guides_now`] gives the guides: the hairs drawn between the guides are
/// kept out of them too.
fn kept_out_now(world: &World, entity: Entity, out: &mut Vec<StrandCapsule>) {
    out.clear();
    let Some(hair) = world.get::<DrawnHair>(entity) else {
        return;
    };
    if hair.face.is_some() {
        return;
    }
    let Some((posed, to_head)) = simulated(world, hair, entity) else {
        return;
    };
    let size = to_head.x_axis.truncate().length();
    out.extend(posed.capsules.iter().map(|c| StrandCapsule {
        a: to_head.transform_point3(c.a),
        ra: c.ra * size,
        b: to_head.transform_point3(c.b),
        rb: c.rb * size,
    }));
}

/// The person's measured body ([`HairBody`]: skin and clothes as drawn) where the hair
/// simulation posed its bones, in the head's frame as [`guides_now`] gives the guides: every
/// drawn hair is kept out of it.
fn body_now(world: &World, entity: Entity, out: &mut Vec<StrandBox>) {
    out.clear();
    let Some(hair) = world.get::<DrawnHair>(entity) else {
        return;
    };
    if hair.face.is_some() {
        return;
    }
    let Some((posed, to_head)) = simulated(world, hair, entity) else {
        return;
    };
    let (Some(body), Some(guides)) = (
        world.get::<HairBody>(hair.person),
        world.get::<HairGuides>(hair.person),
    ) else {
        return;
    };
    if body.version != posed.version || posed.body.len() != body.parts.len() {
        return;
    }
    let from_head = to_head.inverse();
    for (part, now) in body.parts.iter().zip(&posed.body) {
        let Some(grown) = guides.grown_on.get(part.bone as usize) else {
            continue;
        };
        // The box rides its bone from where the hair was grown to where the step left it.
        let back = grown.rotation * now.rotation.conjugate();
        let to_grown =
            Mat4::from_rotation_translation(back, grown.translation - back * now.translation);
        out.push(StrandBox {
            to_box: to_grown * from_head,
            min: part.min,
            cell: part.cell,
            size: part.size,
            distance: part.distance.clone(),
            weight: part.weight.clone(),
        });
    }
}

/// What the hair simulation left for a person's drawn hair, when it is of this build, and the
/// way from the person's model space to the head's frame (the instance's model space).
/// A drawn head of hair's guides as the simulation has them now: how wet each lock is, how
/// much of it a blade left, and how far gone the piece it cut off is.
fn state_now(world: &World, entity: Entity, out: &mut Vec<StrandGuideState>) {
    out.clear();
    let Some(hair) = world.get::<DrawnHair>(entity) else {
        return;
    };
    let Some((posed, _)) = simulated(world, hair, entity) else {
        return;
    };
    let guides = posed.wet.len().max(posed.cut.len());
    out.extend((0..guides).map(|g| StrandGuideState {
        wet: posed.wet.get(g).copied().unwrap_or(0.0),
        cut: posed.cut.get(g).copied().unwrap_or(1.0),
        shed: posed.shed.get(g).copied().unwrap_or(1.0),
    }));
}

fn simulated<'w>(
    world: &'w World,
    hair: &DrawnHair,
    entity: Entity,
) -> Option<(&'w HairGuidesPosed, Mat4)> {
    let (Some(guides), Some(posed), Some(t)) = (
        world.get::<HairGuides>(hair.person),
        world.get::<HairGuidesPosed>(hair.person),
        world.get::<Transform>(entity),
    ) else {
        return None;
    };
    if posed.version != guides.version
        || posed.version != hair.version as u32
        || posed.points.len() != hair.rest_guides.len()
    {
        return None;
    }
    // The person's model space to the head's frame (the skin's rest space riding the head), as
    // the simulation saw the head: fitted to where it left the guides' roots, which ride the
    // scalp. The simulation steps at its own rate, a frame or two behind the pose drawn (and
    // beside the frame when it is busy); taking the head from this frame's pose instead left
    // the hair that far behind every turn and step of the head, sliding on the scalp. Fitted,
    // the hair rides the head as drawn this frame and only its swing comes from the steps.
    let now = Mat4::from_scale_rotation_translation(t.scale, t.rotation, t.translation.as_vec3());
    let per = hair.rest_guides.len() / guides.guides.len().max(1);
    let to_head = if per >= 2 {
        let roots = hair
            .rest_guides
            .iter()
            .step_by(per)
            .zip(posed.points.iter().step_by(per));
        similarity_fit(roots, t.rotation).unwrap_or(now)
    } else {
        now
    }
    .inverse();
    Some((posed, to_head))
}

/// The similarity transform (rotation, uniform scale, translation) carrying the `from` points
/// onto their `to` partners best in the least-squares sense (Horn 1987: the rotation is the
/// quaternion of the largest eigenvalue of a 4x4 matrix of the cross-covariance). `guess`
/// starts the power iteration (the rotation last seen). `None` for fewer than three pairs or
/// points all in one place.
pub(crate) fn similarity_fit<'a>(
    pairs: impl Iterator<Item = (&'a Vec3, &'a Vec3)> + Clone,
    guess: Quat,
) -> Option<Mat4> {
    let (mut n, mut cp, mut cq) = (0.0f32, Vec3::ZERO, Vec3::ZERO);
    for (p, q) in pairs.clone() {
        n += 1.0;
        cp += *p;
        cq += *q;
    }
    if n < 3.0 {
        return None;
    }
    cp /= n;
    cq /= n;
    let (mut s, mut pp, mut qq) = ([[0.0f32; 3]; 3], 0.0f32, 0.0f32);
    for (p, q) in pairs {
        let (a, b) = ((*p - cp).to_array(), (*q - cq).to_array());
        for (r, row) in s.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v += a[r] * b[c];
            }
        }
        pp += (*p - cp).length_squared();
        qq += (*q - cq).length_squared();
    }
    if pp < 1e-12 || qq < 1e-12 {
        return None;
    }
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = s;
    let m = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, syy - sxx - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, szz - sxx - syy],
    ];
    // Shifted so every eigenvalue is positive: the largest then dominates the iteration.
    let shift: f32 = s.iter().flatten().map(|v| v.abs()).sum();
    let mut v = [guess.w, guess.x, guess.y, guess.z];
    for _ in 0..64 {
        let mut next = [0.0f32; 4];
        for (r, out) in next.iter_mut().enumerate() {
            *out = shift * v[r] + (0..4).map(|c| m[r][c] * v[c]).sum::<f32>();
        }
        let len = next.iter().map(|x| x * x).sum::<f32>().sqrt();
        if len < 1e-20 {
            return None;
        }
        v = next.map(|x| x / len);
    }
    let rotation = Quat::from_xyzw(v[1], v[2], v[3], v[0]).normalize();
    let scale = (qq / pp).sqrt();
    let translation = cq - rotation * (cp * scale);
    Some(Mat4::from_scale_rotation_translation(
        Vec3::splat(scale),
        rotation,
        translation,
    ))
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
    fn wet_hair_is_darker_smoother_and_clumps_at_the_tips() {
        // Pairs of hairs rooted 2 mm apart, 2 cm apart at the tip.
        let mut g = StrandGroom {
            points_per: 4,
            points: (0..2)
                .flat_map(|h| {
                    (0..4).map(move |k| {
                        let x = 0.0011 + h as f32 * (0.002 + 0.006 * k as f32);
                        [x, 0.001 + 0.05 * k as f32, 0.001, 1e-4]
                    })
                })
                .collect(),
            roots: vec![[0.6, 0.5, 0.3, 0.2], [0.6, 0.5, 0.3, 0.7]],
            tips: vec![[0.6, 0.5, 0.3, SCALP_ROUGHNESS]; 2],
            ..Default::default()
        };
        let dry = g.clone();
        wet_groom(&mut g, 0.0, WET_SCALP_CELL);
        assert_eq!(g.points, dry.points);
        wet_groom(&mut g, 1.0, WET_SCALP_CELL);
        assert!(g.is_valid());
        // Darker (fair hair by about a half), the random value kept, a smoother cuticle.
        assert!(
            g.roots[0][0] < 0.6 * 0.55 && g.roots[0][0] > 0.6 * 0.3,
            "{:?}",
            g.roots[0]
        );
        assert_eq!(g.roots[1][3], 0.7);
        assert!((g.tips[0][3] - SCALP_ROUGHNESS * (1.0 - WET_SMOOTH)).abs() < 1e-6);
        // The roots stay; the tips come together.
        assert_eq!(g.points[0], dry.points[0]);
        assert_eq!(g.points[4], dry.points[4]);
        let gap = |p: &[[f32; 4]]| (p[3][0] - p[7][0]).abs();
        assert!(
            gap(&g.points) < gap(&dry.points) * 0.3,
            "{} {}",
            gap(&g.points),
            gap(&dry.points)
        );
    }

    #[test]
    fn the_head_is_fitted_to_where_the_roots_are() {
        let rest: Vec<Vec3> = (0..40)
            .map(|i| {
                let a = i as f32 * 0.7;
                let b = i as f32 * 0.31;
                Vec3::new(a.cos() * b.sin(), b.cos(), a.sin() * b.sin()) * 0.09
            })
            .collect();
        let truth = Mat4::from_scale_rotation_translation(
            Vec3::splat(1.04),
            Quat::from_euler(engine_core::math::EulerRot::YXZ, 1.9, -0.4, 0.25),
            Vec3::new(0.02, 1.62, -0.05),
        );
        let moved: Vec<Vec3> = rest.iter().map(|p| truth.transform_point3(*p)).collect();
        // From a guess far off (the head turned most of the way round since).
        let fit = similarity_fit(rest.iter().zip(&moved), Quat::IDENTITY).expect("a fit");
        for (p, q) in rest.iter().zip(&moved) {
            assert!(fit.transform_point3(*p).distance(*q) < 1e-4, "{p} {q}");
        }
        assert!(similarity_fit(rest[..2].iter().zip(&moved[..2]), Quat::IDENTITY).is_none());
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
