//! What the subject's hands hold (`HandEdit::held`): a sword, dagger, spear or torch drawn in
//! the hand's grip, placed every frame from the posed finger joints so the fingers wrap the
//! handle whatever the hand shape and the wrist do.

use engine::prelude::*;
use engine_character::BuiltCharacter;
use engine_character::rig::BoneKind;

use crate::Subject;
use crate::posing::{Held, PoseEdit};

/// One piece of a held item, in the grip's frame: x along the edge (the way the knuckles
/// point), y along the handle toward the index finger (the blade's way), z across the flat.
struct Part {
    shape: Shape,
    material: StandardMaterial,
    at: Vec3,
}

fn steel() -> StandardMaterial {
    StandardMaterial::hex(0xc9cdd3)
        .with_metallic(1.0)
        .with_roughness(0.28)
}

fn iron() -> StandardMaterial {
    StandardMaterial::hex(0x6f737a)
        .with_metallic(1.0)
        .with_roughness(0.45)
}

fn leather() -> StandardMaterial {
    StandardMaterial::hex(0x3b2617).with_roughness(0.8)
}

fn wood() -> StandardMaterial {
    StandardMaterial::hex(0x6b4a2c).with_roughness(0.7)
}

fn cuboid(size: [f32; 3], material: StandardMaterial, at: Vec3) -> Part {
    Part {
        shape: Shape::Cuboid { size },
        material,
        at,
    }
}

fn rod(radius: f32, height: f32, material: StandardMaterial, at: Vec3) -> Part {
    Part {
        shape: Shape::Cylinder {
            radius,
            height,
            segments: 16,
        },
        material,
        at,
    }
}

fn ball(radius: f32, material: StandardMaterial, at: Vec3) -> Part {
    Part {
        shape: Shape::Sphere {
            radius,
            segments: 16,
        },
        material,
        at,
    }
}

/// A blade with its guard just past the index finger (`front`, m from the grip's center),
/// grip to `back` behind the little finger and a pommel.
fn blade(length: f32, width: f32, guard: f32, front: f32, back: f32, r: f32) -> Vec<Part> {
    let g = front + 0.008;
    let grip = front + back;
    vec![
        rod(r, grip, leather(), Vec3::Y * (front - back) * 0.5),
        cuboid([guard, 0.016, 0.022], iron(), Vec3::Y * g),
        cuboid(
            [width, length, 0.0065],
            steel(),
            Vec3::Y * (g + 0.008 + length * 0.5),
        ),
        // The point: a narrower last stretch.
        cuboid(
            [width * 0.55, 0.05, 0.005],
            steel(),
            Vec3::Y * (g + 0.008 + length + 0.025),
        ),
        ball(r * 1.6, iron(), -Vec3::Y * (back + r * 1.2)),
    ]
}

fn parts(held: Held, front: f32) -> Vec<Part> {
    match held {
        Held::Nothing => Vec::new(),
        // An arming sword: 78 cm blade, 20 cm guard, a one-hand grip.
        Held::Sword => blade(0.78, 0.046, 0.2, front, 0.055, 0.016),
        Held::Dagger => blade(0.24, 0.032, 0.085, front, 0.05, 0.0135),
        // A 2.2 m spear held a third of the way up.
        Held::Spear => vec![
            rod(0.016, 2.2, wood(), Vec3::Y * 0.4),
            cuboid([0.04, 0.26, 0.009], steel(), Vec3::Y * 1.62),
            rod(0.02, 0.07, iron(), Vec3::Y * 1.5),
        ],
        Held::Torch => vec![
            rod(0.02, 0.55, wood(), Vec3::Y * 0.17),
            rod(
                0.034,
                0.12,
                StandardMaterial::hex(0x2a2018)
                    .with_roughness(0.9)
                    .with_emissive([1.0, 0.45, 0.12], 2.0),
                Vec3::Y * 0.47,
            ),
        ],
    }
}

/// The spawned pieces of each hand's item.
#[derive(Resource, Default)]
pub struct HeldProps {
    hands: [(Held, Vec<(Entity, Vec3)>); 2],
}

/// The grip's frame of one hand from its posed joints: center, edge, handle and flat axes, and
/// how far the index finger reaches along the handle.
fn grip_frame(points: &[(String, DVec3, BoneKind)], side: &str) -> Option<(DVec3, Quat, f32)> {
    let at = |b: &str| {
        points
            .iter()
            .find(|(n, _, _)| n == &format!("{b}_{side}"))
            .map(|(_, p, _)| *p)
    };
    // The curled fingers' joints lie round the handle: their middle is its center.
    let mut sum = DVec3::ZERO;
    let mut n = 0.0;
    for f in ["index", "middle", "ring", "pinky"] {
        for j in ["01", "02", "03"] {
            sum += at(&format!("{f}_{j}"))?;
            n += 1.0;
        }
    }
    let center = sum / n;
    let handle = (at("index_02")? - at("pinky_02")?).normalize_or_zero();
    let along = at("middle_01")? - at("hand")?;
    let edge = (along - handle * along.dot(handle)).normalize_or_zero();
    if handle == DVec3::ZERO || edge == DVec3::ZERO {
        return None;
    }
    let flat = edge.cross(handle);
    let rot = Quat::from_mat3(&Mat3::from_cols(
        edge.as_vec3(),
        handle.as_vec3(),
        flat.as_vec3(),
    ));
    let front = (at("index_02")? - center).dot(handle) as f32 + 0.012;
    Some((center, rot, front))
}

/// The subject's parts a held item follows.
type HolderParts = (
    &'static Transform,
    Option<&'static PoseEdit>,
    Option<&'static BuiltCharacter>,
    Option<&'static engine::world::animation::SkeletonPose>,
);

/// Spawns, places and removes the subject's held items.
pub fn update(
    mut commands: Commands,
    mut props: ResMut<HeldProps>,
    subject: Query<HolderParts, With<Subject>>,
    mut placed: Query<&mut Transform, Without<Subject>>,
) {
    let Ok((t, edit, built, pose)) = subject.single() else {
        return;
    };
    let points = match (built, pose) {
        (Some(b), Some(p)) => crate::joints::points_of(t, b, p),
        _ => Vec::new(),
    };
    for (side, suffix) in ["l", "r"].into_iter().enumerate() {
        let want = edit.map_or(Held::Nothing, |e| e.hands[side].held);
        let frame = grip_frame(&points, suffix);
        let (have, spawned) = &mut props.hands[side];
        if *have != want || (spawned.is_empty() && want != Held::Nothing && frame.is_some()) {
            for (e, _) in spawned.drain(..) {
                commands.entity(e).despawn();
            }
            *have = want;
            if let Some((_, _, front)) = frame {
                for part in parts(want, front) {
                    let mut mesh = MeshRenderer::new(part.shape, part.material);
                    mesh.cast_shadows = true;
                    let e = commands
                        .spawn((
                            Name::new(format!("held {} {suffix}", want.name())),
                            Transform::default(),
                            mesh,
                        ))
                        .id();
                    spawned.push((e, part.at));
                }
            }
        }
        let Some((center, rot, _)) = frame else {
            continue;
        };
        for (e, at) in spawned.iter() {
            if let Ok(mut pt) = placed.get_mut(*e) {
                pt.translation = center + (rot * *at).as_dvec3();
                pt.rotation = rot;
            }
        }
    }
}
