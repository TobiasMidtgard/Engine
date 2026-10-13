//! The unveiling: a marble youth on a sandstone plinth under a crimson silk sheet, which a
//! timber jib crane lifts off and lowers back on, again and again.
//!
//! The figure is the mannequin's body carved as a statue ([`super::body::statue_sculpt`]),
//! skinned once into [`statue_pose`] (after Michelangelo's David), with carved curls on its
//! head, in veined marble ([`super::kit::Mat::Marble`]); its posed surface becomes a
//! triangle-mesh collider ([`cast_statues`]), which the cloth solver bakes into a distance
//! grid, so the silk drapes over the curls, the raised hand and the shoulders rather than over
//! stand-in capsules.

use std::f32::consts::PI;

use engine_assets::Assets;
use engine_render::{MaterialRef, Mesh};

use super::body::{BodyPart, Human};
use super::characters::statue_pose;
use super::kit::{Mat, Model, ModelBuilder, cuboid, part, register_mesh, renderer};
use super::meshkit::*;
use super::props::{at, nuts};
use super::skin::SkinnedPart;
use super::*;

/// Seconds per unveiling: held up, lowered over the statue, left on, lifted off.
pub const UNVEIL_PERIOD: f32 = 22.0;
/// Height of the plinth the statue stands on, m.
const PLINTH_TOP: f32 = 0.95;
/// Side of the square silk sheet, m, and its particles along each side (odd, so one sits in
/// the middle, where the crane's hook holds it).
const SHEET: f32 = 4.0;
const SHEET_RES: u32 = 45;
/// Height of the crane's post, m.
const CRANE_HEIGHT: f32 = 7.0;
/// The cranium of the mannequin's sculpt in its head bone's bind frame: center and radii.
const CRANIUM: Vec3 = Vec3::new(0.0, 1.737, 0.012);
const CRANIUM_RADII: Vec3 = Vec3::new(0.079, 0.099, 0.103);

/// A posed body to turn into fixed geometry: once it's skinned, its mesh plus `extra` (in
/// the same space, carved details) becomes its collider and it stops being skinned.
#[derive(Component, Debug, Clone)]
pub struct DemoStatue {
    pub extra: Mesh,
}

/// A cord hanging from `top` down to `hook` (drawn as a stretched tube), `above` meters over
/// the hook's origin.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoCord {
    pub hook: Entity,
    pub top: DVec3,
    pub above: f32,
}

/// Short carved curls over the scalp, posed with the head (`head`, `head_rot`: the head
/// bone in model space), behind a hairline that runs over the brow, above the ears and down to
/// the nape: a mass of low, rounded locks with snail-shell curls cut into it, each winding in
/// from a thick lock to a fine tip, flowing down from the crown and forward over the brow,
/// darker in the drilled hollows between them.
fn curls(head: Vec3, head_rot: Quat) -> Mesh {
    let golden = PI * (3.0 - 5f32.sqrt());
    // Directions from the cranium's center over the hair: a Fibonacci spiral of `n` points
    // with the bare face, ears and neck left out.
    let sites = |n: u32| {
        (0..n).filter_map(move |i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - y * y).sqrt();
            let a = golden * i as f32;
            let d = Vec3::new(r * a.cos(), y, r * a.sin());
            let forward = -d.z;
            let hairline = if forward > 0.0 {
                0.05 + 0.32 * smoothstep(0.0, 0.7, forward)
            } else {
                0.05 - 0.55 * smoothstep(0.0, 0.8, -forward)
            };
            let ear = d.x.abs() > 0.8 && d.y < 0.3 && d.z < 0.35;
            (d.y >= hairline && !ear).then_some((i, d))
        })
    };
    let shade = |mesh: &mut Mesh, up: Vec3, low: f32| {
        for v in &mut mesh.vertices {
            let ao = low + (1.0 - low) * Vec3::from(v.normal).dot(up).max(0.0);
            v.color = [ao, ao, ao, 1.0];
        }
    };
    let mut out = Mesh::default();
    // The mass: flattened lumps packed over the scalp.
    for (i, d) in sites(300) {
        let k = super::hash01(i * 7 + 3);
        let normal = (d / CRANIUM_RADII).normalize();
        let size = 0.019 + 0.007 * k;
        let spin = Quat::from_rotation_y(TAU * super::hash01(i * 7 + 5));
        let mut lump = xf(
            &sphere(size, 8),
            Mat4::from_scale_rotation_translation(
                Vec3::new(1.0, 0.42 + 0.12 * k, 1.0),
                Quat::from_rotation_arc(Vec3::Y, normal) * spin,
                CRANIUM + d * CRANIUM_RADII * 0.975,
            ),
        );
        shade(&mut lump, normal, 0.5);
        out.merge(&lump);
    }
    // The curls, flowing from a whorl at the back of the crown.
    let whorl = Vec3::new(0.05, 0.75, 0.66).normalize();
    for (i, d) in sites(190) {
        let h = |j: u32| super::hash01(i * 13 + j);
        let n = (d / CRANIUM_RADII).normalize();
        let at = CRANIUM + d * CRANIUM_RADII * 1.02;
        let away = d - whorl;
        let flow = (away - n * away.dot(n))
            .try_normalize()
            .unwrap_or(n.any_orthonormal_vector());
        let t = Quat::from_axis_angle(n, (h(1) - 0.5) * 1.4) * flow;
        let b = n.cross(t);
        let hand = if h(2) < 0.5 { -1.0 } else { 1.0 };
        let r0 = 0.011 + 0.006 * h(3);
        let turns = (1.35 + 0.4 * h(4)) * TAU;
        let steps = 16;
        let path: Vec<Vec3> = (0..=steps)
            .map(|j| {
                let f = j as f32 / steps as f32;
                let a = f * turns;
                let r = r0 * (1.0 - 0.72 * f);
                let lift = 0.004 * (f * PI).sin();
                // The lock comes in from behind the curl's center and winds inward.
                at - t * r0 * 0.3 + (t * a.cos() + b * a.sin() * hand) * r + n * lift
            })
            .collect();
        let thick = 0.0042 + 0.0016 * h(5);
        let mut curl = tube(&path, |f| thick * (1.0 - 0.7 * f), 7, false, true, 50.0);
        shade(&mut curl, n, 0.55);
        out.merge(&curl);
    }
    out.transform(&Mat4::from_rotation_translation(
        head_rot,
        head - head_rot * Human::HEAD,
    ));
    out
}

/// Build the unveiling with the plinth at `c` (on the ground) and the statue facing
/// `facing`.
pub fn spawn_unveiling(commands: &mut Commands, c: DVec3, facing: Vec3) {
    commands.queue(super::kit::ensure_materials);
    let face = Quat::from_rotation_arc(Vec3::NEG_Z, facing.with_y(0.0).normalize());
    let (plinth, plinth_shape) = plinth(commands);
    plinth.spawn_solid(commands, at(c).with_rotation(face), plinth_shape);

    let mut pose = skeleton(&HUMANOID, Rig::Humanoid);
    let (head, head_rot) = statue_pose(&mut pose.bones);
    let base = c + DVec3::Y * f64::from(PLINTH_TOP);
    let statue = commands
        .spawn((
            Name::new("Marble statue"),
            at(base).with_rotation(face),
            pose,
        ))
        .id();
    let hair = curls(head, head_rot);
    let key = "softbody/yard/statue/curls".to_string();
    register_mesh(commands, key.clone(), hair.clone());
    commands.spawn((
        Transform::default(),
        renderer(&key, Mat::Marble),
        ChildOf(statue),
    ));
    commands.spawn((
        Name::new("Marble statue body"),
        Transform::default(),
        SkinnedPart::new(
            BodyPart::Statue,
            statue,
            "softbody/yard/statue/body",
            MaterialRef::Asset(Mat::Marble.key()),
        ),
        DemoStatue { extra: hair },
        ChildOf(statue),
    ));
    // The top of the curls, where the hook sets the sheet down.
    let crown = head + head_rot * (CRANIUM + Vec3::Y * (CRANIUM_RADII.y + 0.012) - Human::HEAD);
    let crown = base + (face * crown).as_dvec3();

    // The crane stands behind the statue's left shoulder, its jib over the head.
    let post = c + (face * Vec3::new(-0.75, 0.0, 0.65).normalize() * 2.3).as_dvec3();
    let to_statue = (crown - post).with_y(0.0);
    let reach = to_statue.length() as f32;
    let (crane, crane_shape, sheave) = crane(commands, reach);
    let crane_rot = Quat::from_rotation_arc(Vec3::X, to_statue.normalize().as_vec3());
    crane.spawn_solid(commands, at(post).with_rotation(crane_rot), crane_shape);
    let cord_top = post + (crane_rot * sheave).as_dvec3();

    // The sheet hangs from its middle on the hook: held up clear of the statue (its corners
    // hang 2.8 m below the hook), lowered until it rests on the crown.
    let high = cord_top - DVec3::Y * 0.55;
    let low = crown + DVec3::Y * 0.012;
    let silk = SoftMaterial::hex(0x8c1c2c)
        .with_pattern(Pattern::Border, 0xc9a227, 0xe8c547)
        .with_roughness(0.5)
        .with_sheen(0.85)
        .with_wrinkles(0.6);
    let mid = SHEET_RES / 2;
    let sheet = commands
        .spawn((
            Name::new("Unveiling sheet"),
            at(high).with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
            Cloth {
                size: [SHEET, SHEET],
                resolution: [SHEET_RES, SHEET_RES],
                anchor: [0.5, 0.5],
                pins: ClothPins::Points(vec![[mid, mid]]),
                mass_per_m2: 0.16,
                shear_compliance: 1e-2,
                bend_compliance: 2e-2,
                aero_drag: 0.8,
                pierceable: false,
                flammable: false,
                material: silk,
                sim: SimSettings {
                    self_collide: true,
                    thickness: 0.012,
                    friction: 0.55,
                    damping: 0.12,
                    wind_scale: 0.3,
                    prewarm_secs: 3.0,
                    ..SimSettings::default()
                },
                ..Cloth::default()
            },
            DemoMotion::Hoist {
                low,
                high,
                period: UNVEIL_PERIOD,
            },
            DemoClock::default(),
        ))
        .id();
    // The brass ring on the hook, upright (the sheet's frame lies flat), just above the silk.
    let ring = hook_ring(commands);
    ring.attach(
        commands,
        sheet,
        Transform::from_xyz(0.0, 0.0, 0.05).with_rotation(Quat::from_rotation_x(FRAC_PI_2)),
    );
    let cord = "softbody/yard/statue/cord".to_string();
    register_mesh(
        commands,
        cord.clone(),
        tube(&[Vec3::ZERO, Vec3::NEG_Y], |_| 0.008, 8, false, false, 4.0),
    );
    commands.spawn((
        Name::new("Crane cord"),
        at(cord_top),
        renderer(&cord, Mat::Rope),
        DemoCord {
            hook: sheet,
            top: cord_top,
            above: 0.09,
        },
    ));
}

/// A molded sandstone plinth, PLINTH_TOP high, with a bronze plaque on the front (−Z).
fn plinth(commands: &mut Commands) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("statue_plinth").grounded();
    // Footing, base moldings, die, cornice and cap: half width, bottom, top, edge radius.
    let tiers = [
        (0.60, 0.0, 0.14, 0.02),
        (0.545, 0.14, 0.2, 0.028),
        (0.5, 0.2, 0.24, 0.012),
        (0.45, 0.24, 0.78, 0.01),
        (0.49, 0.78, 0.82, 0.012),
        (0.53, 0.82, 0.88, 0.028),
        (0.56, 0.88, PLINTH_TOP, 0.016),
    ];
    for (k, (hw, y0, y1, r)) in tiers.into_iter().enumerate() {
        let block = rounded_box(
            Vec3::new(2.0 * hw, y1 - y0, 2.0 * hw),
            r,
            3,
            0.1,
            BoxUv::Planar {
                offset: Vec2::new(0.37 * k as f32, 0.61 * k as f32),
            },
        );
        let block = jitter(block, 0.0015, 9.0, 31 + k as u32);
        mb.add(
            Mat::Stone,
            place(block, Vec3::Y * (0.5 * (y0 + y1)), Quat::IDENTITY),
        );
    }
    mb.add(
        Mat::Bronze,
        place(
            rounded_box(Vec3::new(0.44, 0.2, 0.014), 0.005, 2, 0.1, BoxUv::PLANAR),
            Vec3::new(0.0, 0.53, -0.455),
            Quat::IDENTITY,
        ),
    );
    let corners: Vec<Vec3> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|&(a, b)| Vec3::new(a * 0.19, 0.53 + b * 0.075, -0.462))
        .collect();
    mb.add_small(Mat::Bronze, nuts(&corners, Vec3::NEG_Z, 0.012));
    let shape = ColliderShape::Compound(vec![
        part(Vec3::Y * 0.12, cuboid(Vec3::new(1.2, 0.24, 1.2))),
        part(Vec3::Y * 0.51, cuboid(Vec3::new(0.9, 0.54, 0.9))),
        part(
            Vec3::Y * (0.5 * (0.78 + PLINTH_TOP)),
            cuboid(Vec3::new(1.12, PLINTH_TOP - 0.78, 1.12)),
        ),
    ]);
    (mb.build(commands), shape)
}

/// A timber jib crane: a post on a concrete footing with a jib along +X reaching `reach`
/// meters, braced, a sheave block at the tip and a hand winch on the post. Returns the model,
/// its collider and where the cord leaves the sheave (local).
fn crane(commands: &mut Commands, reach: f32) -> (Model, ColliderShape, Vec3) {
    let mut mb = ModelBuilder::new("statue_crane").grounded();
    let h = CRANE_HEIGHT;
    let post = 0.22f32;
    mb.add(
        Mat::Concrete,
        place(
            rounded_box(Vec3::new(0.8, 0.3, 0.8), 0.03, 2, 0.5, BoxUv::PLANAR),
            Vec3::Y * 0.1,
            Quat::IDENTITY,
        ),
    );
    // Steel shoe gripping the foot of the post.
    for k in 0..4 {
        let rot = Quat::from_rotation_y(k as f32 * FRAC_PI_2);
        mb.add(
            Mat::BlackSteel,
            place(
                rounded_box(Vec3::new(0.3, 0.42, 0.012), 0.003, 1, 0.2, BoxUv::PLANAR),
                rot * Vec3::new(0.0, 0.46, post * 0.5 + 0.006),
                rot,
            ),
        );
        let bolts: Vec<Vec3> = [0.36, 0.56]
            .iter()
            .map(|&y| rot * Vec3::new(0.0, y, post * 0.5 + 0.012))
            .collect();
        mb.add_small(Mat::Galvanized, nuts(&bolts, rot * Vec3::Z, 0.02));
    }
    mb.add(
        Mat::Timber,
        place(
            timber(
                Vec3::new(post, h - 0.25, post),
                0.012,
                Vec2::new(0.05, 0.3),
                false,
            ),
            Vec3::Y * (0.25 + 0.5 * (h - 0.25)),
            Quat::IDENTITY,
        ),
    );
    // The jib, flush with the top of the post, and a brace under it.
    let jib_y = h - 0.1;
    let (x0, x1) = (-0.2f32, reach + 0.22);
    mb.add(
        Mat::Timber,
        place(
            timber(
                Vec3::new(x1 - x0, 0.2, 0.15),
                0.01,
                Vec2::new(0.3, 1.1),
                true,
            ),
            Vec3::new(0.5 * (x0 + x1), jib_y, 0.0),
            Quat::IDENTITY,
        ),
    );
    let (a, b) = (
        Vec3::new(post * 0.5, jib_y - 1.4, 0.0),
        Vec3::new(1.35, jib_y - 0.1, 0.0),
    );
    let d = b - a;
    mb.add(
        Mat::Timber,
        place(
            timber(
                Vec3::new(0.11, d.length() + 0.1, 0.11),
                0.01,
                Vec2::new(0.6, 0.2),
                false,
            ),
            (a + b) * 0.5,
            Quat::from_rotation_arc(Vec3::Y, d.normalize()),
        ),
    );
    // Knee plates where the jib meets the post and the brace meets both.
    for (at, size) in [
        (
            Vec3::new(0.12, jib_y - 0.16, 0.0),
            Vec3::new(0.42, 0.42, 0.16),
        ),
        (a + Vec3::new(0.0, 0.05, 0.0), Vec3::new(0.05, 0.3, 0.16)),
        (b - Vec3::new(0.0, 0.05, 0.0), Vec3::new(0.3, 0.06, 0.16)),
    ] {
        for z in [-1.0f32, 1.0] {
            mb.add(
                Mat::BlackSteel,
                place(
                    rounded_box(
                        Vec3::new(size.x, size.y, 0.01),
                        0.003,
                        1,
                        0.2,
                        BoxUv::PLANAR,
                    ),
                    at + Vec3::Z * z * (size.z * 0.5 + 0.005),
                    Quat::IDENTITY,
                ),
            );
        }
    }
    // Sheave block hanging under the tip: cheeks, a grooved wheel and a strap up to the jib.
    let sheave_r = 0.075f32;
    let hub = Vec3::new(reach - sheave_r, jib_y - 0.1 - 0.17, 0.0);
    let wheel = lathe(
        &[
            Vec2::new(0.0, -0.022),
            Vec2::new(sheave_r, -0.022),
            Vec2::new(sheave_r - 0.018, 0.0),
            Vec2::new(sheave_r, 0.022),
            Vec2::new(0.0, 0.022),
        ],
        28,
        LatheUv::meters(sheave_r),
    );
    mb.add(
        Mat::Galvanized,
        place(wheel, hub, Quat::from_rotation_x(FRAC_PI_2)),
    );
    for z in [-1.0f32, 1.0] {
        mb.add(
            Mat::BlackSteel,
            place(
                rounded_box(Vec3::new(0.2, 0.36, 0.012), 0.03, 3, 0.1, BoxUv::PLANAR),
                hub + Vec3::new(0.0, 0.07, z * 0.032),
                Quat::IDENTITY,
            ),
        );
    }
    mb.add_small(Mat::Galvanized, nuts(&[hub], Vec3::Z, 0.03));
    // The hauling rope runs back over the sheave and down to the winch drum on the post.
    let drum = Vec3::new(post * 0.5 + 0.16, 1.15, 0.0);
    mb.add(
        Mat::Rope,
        tube(
            &[
                hub + Vec3::new(-sheave_r, 0.0, 0.0),
                hub + Vec3::new(-0.55 * sheave_r, 0.55 * sheave_r, 0.0),
                hub + Vec3::new(0.0, sheave_r, 0.0),
                hub + Vec3::new(0.55 * sheave_r, 0.55 * sheave_r, 0.0),
                hub + Vec3::new(sheave_r, 0.0, 0.0),
            ],
            |_| 0.008,
            8,
            false,
            false,
            4.0,
        ),
    );
    mb.add(
        Mat::Rope,
        tube(
            &[
                hub + Vec3::new(-sheave_r, 0.0, 0.0),
                drum + Vec3::new(0.06, 0.0, 0.0),
            ],
            |_| 0.008,
            8,
            false,
            false,
            4.0,
        ),
    );
    // Winch: a drum between two cheeks bolted to the post, and a crank.
    let drum_mesh = lathe(
        &[
            Vec2::new(0.0, -0.11),
            Vec2::new(0.07, -0.11),
            Vec2::new(0.07, -0.1),
            Vec2::new(0.05, -0.1),
            Vec2::new(0.05, 0.1),
            Vec2::new(0.07, 0.1),
            Vec2::new(0.07, 0.11),
            Vec2::new(0.0, 0.11),
        ],
        24,
        LatheUv::meters(0.07),
    );
    mb.add(
        Mat::DarkSteel,
        place(drum_mesh, drum, Quat::from_rotation_x(FRAC_PI_2)),
    );
    // A few turns of rope on the drum.
    mb.add(
        Mat::Rope,
        place(
            lathe(
                &[
                    Vec2::new(0.0, -0.085),
                    Vec2::new(0.062, -0.085),
                    Vec2::new(0.065, 0.0),
                    Vec2::new(0.062, 0.085),
                    Vec2::new(0.0, 0.085),
                ],
                24,
                LatheUv::meters(0.065),
            ),
            drum,
            Quat::from_rotation_x(FRAC_PI_2),
        ),
    );
    for z in [-1.0f32, 1.0] {
        mb.add(
            Mat::PaintGreen,
            place(
                rounded_box(Vec3::new(0.3, 0.24, 0.014), 0.02, 2, 0.1, BoxUv::PLANAR),
                Vec3::new(post * 0.5 + 0.12, 1.15, z * 0.125),
                Quat::IDENTITY,
            ),
        );
    }
    let crank_at = drum + Vec3::Z * 0.15;
    mb.add(
        Mat::BlackSteel,
        tube(
            &[
                drum + Vec3::Z * 0.13,
                crank_at,
                crank_at + Vec3::new(0.0, -0.17, 0.0),
                crank_at + Vec3::new(0.0, -0.17, 0.1),
            ],
            |_| 0.011,
            8,
            false,
            true,
            1.0,
        ),
    );
    let shape = ColliderShape::Compound(vec![
        part(Vec3::Y * 0.1, cuboid(Vec3::new(0.8, 0.3, 0.8))),
        part(Vec3::Y * (0.5 * h), cuboid(Vec3::new(post, h, post))),
    ]);
    // The cord drops from the far side of the sheave.
    (mb.build(commands), shape, hub + Vec3::X * sheave_r)
}

/// The crane hook's brass ring and swivel eye.
fn hook_ring(commands: &mut Commands) -> Model {
    let mut mb = ModelBuilder::new("statue_hook");
    mb.add(
        Mat::Brass,
        place(
            torus(0.032, 0.007, 28, 10),
            Vec3::Y * 0.032,
            Quat::from_rotation_x(FRAC_PI_2),
        ),
    );
    mb.add(
        Mat::Brass,
        lathe(
            &[
                Vec2::new(0.0, 0.06),
                Vec2::new(0.012, 0.06),
                Vec2::new(0.012, 0.085),
                Vec2::new(0.006, 0.095),
                Vec2::new(0.0, 0.095),
            ],
            16,
            LatheUv::meters(0.012),
        ),
    );
    mb.build(commands)
}

/// Once a statue's body is skinned into its pose, make the posed surface its (fixed)
/// collider and stop skinning it.
pub fn cast_statues(
    mut commands: Commands,
    meshes: Option<Res<Assets<Mesh>>>,
    statues: Query<(Entity, &SkinnedPart, &DemoStatue)>,
) {
    let Some(meshes) = meshes else {
        return;
    };
    for (e, part, statue) in &statues {
        let Some(mesh) = part.mesh.and_then(|h| meshes.get(h)) else {
            continue;
        };
        let mut all = mesh.clone();
        all.merge(&statue.extra);
        let vertices = all
            .vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .collect();
        let indices = all.indices.as_chunks::<3>().0.to_vec();
        commands
            .entity(e)
            .insert((
                RigidBody::fixed(),
                Collider {
                    shape: ColliderShape::TriMesh { vertices, indices },
                    ..Collider::default()
                },
            ))
            .remove::<(SkinnedPart, DemoStatue)>();
    }
}

/// Stretch each cord from its top down to its hook.
pub fn hang_cords(
    mut cords: Query<(&mut Transform, &DemoCord)>,
    hooks: Query<&Transform, Without<DemoCord>>,
) {
    for (mut t, cord) in &mut cords {
        let Ok(hook) = hooks.get(cord.hook) else {
            continue;
        };
        let end = hook.translation + DVec3::Y * f64::from(cord.above);
        let d = (end - cord.top).as_vec3();
        let len = d.length().max(0.01);
        t.translation = cord.top;
        t.rotation = Quat::from_rotation_arc(Vec3::NEG_Y, d / len);
        t.scale = Vec3::new(1.0, len, 1.0);
    }
}
