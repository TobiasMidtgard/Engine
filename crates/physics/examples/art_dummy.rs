//! Renders the crash test dummy headless, for checking it by eye: the ragdoll parts assembled
//! in rest pose, exploded, bent at every joint, three shirts side by side, and the standing
//! figure. Prints triangle counts and how well each part fits its collider.
//!
//! ```sh
//! cargo run -p engine_physics --example art_dummy -- [out_dir] [only] [width]x[height]
//! ```
//!
//! `only` renders just the shots whose name contains it. Needs a GPU or a software Vulkan
//! driver (lavapipe).

// The kit and the dummy are shared with the crate, which uses more of them.
#![allow(dead_code)]

#[path = "../src/demo/art/dummy.rs"]
mod dummy;
#[path = "../src/demo/art/kit.rs"]
mod kit;

use bevy_ecs::component::{Component, Mutable};
use dummy::{DummyMesh, RagdollLayout, RagdollMeshes};
use engine_core::math::{DVec3, Quat, Vec3, Vec4};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use kit::srgb;

/// `Assets<Mesh>` without naming `engine_assets` (not a dependency of this crate): the
/// type of `resolve_mesh_renderers`' third parameter.
trait ResInner {
    type Inner: Resource + Component<Mutability = Mutable>;
}
impl<T: Resource + Component<Mutability = Mutable>> ResInner for ResMut<'_, T> {
    type Inner = T;
}
fn mesh_assets<A, B, C: ResInner, D, E>(
    _: fn(A, B, C, D, E),
    world: &mut World,
) -> Mut<'_, C::Inner> {
    world.resource_mut::<C::Inner>()
}

const BLUE: u32 = 0x3a6ea5;
const RED: u32 = 0xb4413a;
const GREEN: u32 = 0x4e8a3e;

/// Scene origins (x), meters apart so shots don't see each other.
const REST: f64 = 0.0;
const EXPLODED: f64 = 10.0;
const BENT: f64 = 20.0;
const CURLED: f64 = 30.0;
const STANDING: f64 = 40.0;
const LINEUP: f64 = 50.0;
const LAYOUTS: f64 = 60.0;

/// (name, eye, target, fov) in world meters.
const SHOTS: &[(&str, [f64; 3], [f64; 3], f32)] = &[
    ("01_rest_front", [REST, 1.0, 3.6], [REST, 0.92, 0.0], 32.0),
    (
        "02_rest_side",
        [REST + 3.6, 1.0, 0.0],
        [REST, 0.92, 0.0],
        32.0,
    ),
    (
        "03_rest_34_back",
        [REST - 2.4, 1.6, -2.6],
        [REST, 0.95, 0.0],
        32.0,
    ),
    (
        "04_head_close",
        [REST + 0.3, 1.72, 0.62],
        [REST, 1.62, 0.0],
        30.0,
    ),
    (
        "05_head_side",
        [REST + 0.62, 1.66, 0.14],
        [REST, 1.63, 0.0],
        30.0,
    ),
    (
        "06_torso_close",
        [REST + 0.75, 1.4, 1.05],
        [REST, 1.2, 0.0],
        32.0,
    ),
    (
        "07_hand_shoe_close",
        [REST + 0.85, 0.55, 0.85],
        [REST + 0.2, 0.42, 0.05],
        34.0,
    ),
    (
        "07b_hand_close",
        [REST + 0.55, 0.86, 0.42],
        [REST + 0.2, 0.81, 0.0],
        30.0,
    ),
    (
        "07c_knee_close",
        [REST + 0.55, 0.56, 0.75],
        [REST + 0.1, 0.48, 0.0],
        30.0,
    ),
    (
        "07f_knee_side",
        [REST + 0.36, 0.52, 0.06],
        [REST + 0.1, 0.5, 0.0],
        30.0,
    ),
    (
        "07g_hem_close",
        [REST + 0.22, 0.8, 0.36],
        [REST + 0.06, 0.86, 0.06],
        30.0,
    ),
    (
        "07d_hand_back",
        [EXPLODED + 0.62, 1.21, 0.32],
        [EXPLODED + 0.29, 1.14, 0.0],
        30.0,
    ),
    (
        "07e_hand_palm",
        [EXPLODED - 0.05, 1.21, 0.35],
        [EXPLODED + 0.29, 1.14, 0.0],
        30.0,
    ),
    (
        "08_exploded",
        [EXPLODED + 0.5, 1.45, 4.6],
        [EXPLODED, 1.2, 0.0],
        34.0,
    ),
    ("09_bent", [BENT + 1.8, 1.5, 2.6], [BENT, 0.95, 0.0], 32.0),
    (
        "09b_bent_hips",
        [BENT + 0.55, 0.62, 0.95],
        [BENT, 0.86, 0.05],
        34.0,
    ),
    (
        "09c_bent_under",
        [BENT - 0.3, 0.35, 0.9],
        [BENT, 0.8, 0.0],
        40.0,
    ),
    (
        "10_bent_back",
        [BENT - 2.2, 1.3, -2.2],
        [BENT, 0.9, 0.0],
        32.0,
    ),
    (
        "11_curled",
        [CURLED + 2.3, 1.45, 2.3],
        [CURLED, 0.85, 0.0],
        34.0,
    ),
    (
        "12_standing",
        [STANDING + 0.6, 1.2, 4.2],
        [STANDING, 0.9, 0.0],
        32.0,
    ),
    (
        "13_lineup",
        [LINEUP + 0.5, 1.6, 5.4],
        [LINEUP, 0.9, 0.0],
        36.0,
    ),
    (
        "14_layouts",
        [LAYOUTS + 0.4, 1.1, 4.2],
        [LAYOUTS, 0.98, 0.0],
        34.0,
    ),
];

/// (rotation, translation) taking rest-pose layout space to posed space.
type Pose = (Quat, Vec3);

fn about(pivot: Vec3, r: Quat) -> Pose {
    (r, pivot - r * pivot)
}

fn then(parent: Pose, child: Pose) -> Pose {
    (parent.0 * child.0, parent.0 * child.1 + parent.1)
}

/// Joint rotations for a pose: waist, neck, shoulders, elbows (about X), hips, knees (about
/// X); paired joints are [right, left].
#[derive(Clone, Copy)]
struct Joints {
    root: Pose,
    waist: Quat,
    neck: Quat,
    shoulder: [Quat; 2],
    elbow: [f32; 2],
    hip: [Quat; 2],
    knee: [f32; 2],
}

impl Default for Joints {
    fn default() -> Self {
        Self {
            root: (Quat::IDENTITY, Vec3::ZERO),
            waist: Quat::IDENTITY,
            neck: Quat::IDENTITY,
            shoulder: [Quat::IDENTITY; 2],
            elbow: [0.0; 2],
            hip: [Quat::IDENTITY; 2],
            knee: [0.0; 2],
        }
    }
}

/// Every part's (mesh, body center, pose) for a layout and joint rotations.
fn posed<'a>(
    m: &'a RagdollMeshes,
    l: &RagdollLayout,
    j: &Joints,
) -> Vec<(&'a DummyMesh, Vec3, Pose)> {
    let side = |p: Vec3, s: usize| {
        if s == 0 {
            p * Vec3::new(-1.0, 1.0, 1.0)
        } else {
            p
        }
    };
    let pelvis = j.root;
    let chest = then(pelvis, about(l.waist, j.waist));
    let head = then(chest, about(l.neck, j.neck));
    let mut out = vec![
        (&m.pelvis, l.pelvis, pelvis),
        (&m.chest, l.chest.center, chest),
        (&m.head, l.head, head),
    ];
    for s in 0..2 {
        let upper = then(chest, about(side(l.shoulder, s), j.shoulder[s]));
        let fore = then(
            upper,
            about(side(l.elbow, s), Quat::from_rotation_x(j.elbow[s])),
        );
        let thigh = then(pelvis, about(side(l.hip, s), j.hip[s]));
        let shin = then(
            thigh,
            about(side(l.knee, s), Quat::from_rotation_x(j.knee[s])),
        );
        out.push((&m.upper_arm[s], side(l.upper_arm.center, s), upper));
        out.push((&m.forearm[s], side(l.forearm.center, s), fore));
        out.push((&m.thigh[s], side(l.thigh.center, s), thigh));
        out.push((&m.shin[s], side(l.shin.center, s), shin));
    }
    out
}

fn spawn_mesh(
    world: &mut World,
    mesh: &engine_render::Mesh,
    mat: StandardMaterial,
    at: DVec3,
    rot: Quat,
) {
    if mesh.indices.is_empty() {
        return;
    }
    let handle = mesh_assets(engine_render::resolve_mesh_renderers, world).add(mesh.clone());
    world.spawn((
        Transform::from_translation(at).with_rotation(rot),
        MeshRenderer::new(handle, mat),
    ));
}

fn spawn_dummy(world: &mut World, m: &DummyMesh, at: DVec3, rot: Quat) {
    spawn_mesh(world, &m.vinyl, dummy::vinyl_material(), at, rot);
    spawn_mesh(world, &m.steel, dummy::steel_material(), at, rot);
}

fn spawn_ragdoll(
    world: &mut World,
    m: &RagdollMeshes,
    l: &RagdollLayout,
    j: &Joints,
    origin: DVec3,
) {
    for (mesh, center, (q, t)) in posed(m, l, j) {
        let pos = q * center + t;
        spawn_dummy(world, mesh, origin + pos.as_dvec3(), q);
    }
}

/// Signed distance to a capsule along Y.
fn sd_capsule(p: Vec3, c: &dummy::CapsuleSpec) -> f32 {
    let q = p - c.center;
    let y = q.y.clamp(-c.half_height, c.half_height);
    (q - Vec3::Y * y).length() - c.radius
}

/// Signed distance to the union of every collider of the ragdoll in rest pose.
fn sd_colliders(l: &RagdollLayout, p: Vec3) -> f32 {
    let q = (p - l.pelvis).abs() - l.pelvis_half;
    let pelvis = q.max(Vec3::ZERO).length() + q.max_element().min(0.0);
    let mut d = pelvis
        .min(sd_capsule(p, &l.chest))
        .min(p.distance(l.head) - l.head_radius);
    let mirror = Vec3::new(-1.0, 1.0, 1.0);
    for c in [l.upper_arm, l.forearm, l.thigh, l.shin] {
        let m = dummy::CapsuleSpec {
            center: c.center * mirror,
            ..c
        };
        d = d.min(sd_capsule(p, &c)).min(sd_capsule(p, &m));
    }
    d
}

/// A part to report: name, mesh, body center, collider support distance along a direction.
type Row<'a> = (&'a str, &'a DummyMesh, Vec3, Box<dyn Fn(Vec3) -> f32>);

fn report(name: &str, l: &RagdollLayout, m: &RagdollMeshes) {
    println!("== {name}: {} triangles", m.triangle_count());
    let cap = |c: dummy::CapsuleSpec| move |d: Vec3| c.radius + c.half_height * d.y.abs();
    let rows: [Row; 7] = [
        ("pelvis", &m.pelvis, l.pelvis, {
            let h = l.pelvis_half;
            Box::new(move |d: Vec3| (d.abs() * h).element_sum())
        }),
        ("chest", &m.chest, l.chest.center, Box::new(cap(l.chest))),
        ("head", &m.head, l.head, {
            let r = l.head_radius;
            Box::new(move |_| r)
        }),
        (
            "upper arm",
            &m.upper_arm[1],
            l.upper_arm.center,
            Box::new(cap(l.upper_arm)),
        ),
        (
            "forearm",
            &m.forearm[1],
            l.forearm.center,
            Box::new(cap(l.forearm)),
        ),
        ("thigh", &m.thigh[1], l.thigh.center, Box::new(cap(l.thigh))),
        ("shin", &m.shin[1], l.shin.center, Box::new(cap(l.shin))),
    ];
    let axes = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z];
    println!(
        "  part        tris   beyond own collider along +x -x +y -y +z -z (cm)   worst outside all colliders"
    );
    for (part, mesh, center, support) in rows {
        let per_axis: Vec<String> = axes
            .iter()
            .map(|&d| {
                let s = mesh
                    .vinyl
                    .vertices
                    .iter()
                    .map(|v| Vec3::from(v.position).dot(d))
                    .fold(f32::MIN, f32::max);
                format!("{:5.1}", (s - support(d)) * 100.0)
            })
            .collect();
        let (worst, at) = mesh
            .vinyl
            .vertices
            .iter()
            .map(|v| {
                let p = Vec3::from(v.position) + center;
                (sd_colliders(l, p), p)
            })
            .fold((f32::MIN, Vec3::ZERO), |a, b| if b.0 > a.0 { b } else { a });
        println!(
            "  {part:<10} {:>6}   {}   {:4.1} cm at {:.3?}",
            mesh.triangle_count(),
            per_axis.join(" "),
            worst * 100.0,
            at.to_array()
        );
    }
}

fn build_scene(world: &mut World) {
    let hy = RagdollLayout::HYBRID_III_50TH;
    let pg = RagdollLayout::PROVING_GROUND;
    let t0 = std::time::Instant::now();
    let blue = dummy::ragdoll_parts(&hy, srgb(BLUE));
    println!(
        "ragdoll_parts: {:.0} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    let red = dummy::ragdoll_parts(&hy, srgb(RED));
    let green = dummy::ragdoll_parts(&hy, srgb(GREEN));
    let old = dummy::ragdoll_parts(&pg, srgb(RED));
    let t0 = std::time::Instant::now();
    let walker = dummy::standing_dummy(1.8, srgb(0x2e86e8));
    println!(
        "standing_dummy: {:.0} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    let rider = dummy::standing_dummy(1.8, srgb(0xe8772e));
    report("ragdoll, HYBRID_III_50TH", &hy, &blue);
    report("ragdoll, PROVING_GROUND", &pg, &old);
    let bb = walker.vinyl.aabb();
    println!(
        "== standing: {} triangles, aabb {:.3?}..{:.3?}",
        walker.triangle_count(),
        bb.min.to_array(),
        bb.max.to_array()
    );

    let x = |x0: f64| DVec3::new(x0, 0.0, 0.0);
    spawn_ragdoll(world, &blue, &hy, &Joints::default(), x(REST));

    // Exploded: each part pushed away from the pelvis.
    for (mesh, center, _) in posed(&blue, &hy, &Joints::default()) {
        let from = hy.pelvis;
        let at = center + (center - from) * Vec3::new(0.9, 0.55, 0.0) + Vec3::Y * 0.35;
        spawn_dummy(world, mesh, x(EXPLODED) + at.as_dvec3(), Quat::IDENTITY);
    }

    // Bent at every joint to its limits: holes would show here.
    let bent = Joints {
        root: (Quat::IDENTITY, Vec3::ZERO),
        waist: Quat::from_rotation_x(0.42) * Quat::from_rotation_z(0.22),
        neck: Quat::from_rotation_x(0.5) * Quat::from_rotation_z(-0.3),
        shoulder: [
            Quat::from_rotation_x(-1.3),
            Quat::from_rotation_z(1.35) * Quat::from_rotation_x(-0.3),
        ],
        elbow: [-2.0, -1.2],
        hip: [
            Quat::from_rotation_x(-1.15),
            Quat::from_rotation_x(0.45) * Quat::from_rotation_z(0.35),
        ],
        knee: [1.9, 0.8],
    };
    spawn_ragdoll(world, &blue, &hy, &bent, x(BENT));
    // Curled up, as after a fall: chest folded forward, knees to the chest, arms around.
    let curled = Joints {
        root: about(Vec3::new(0.0, 0.9, 0.0), Quat::from_rotation_x(0.3)),
        waist: Quat::from_rotation_x(0.5) * Quat::from_rotation_y(0.3),
        neck: Quat::from_rotation_x(-0.55),
        shoulder: [
            Quat::from_rotation_x(-0.9) * Quat::from_rotation_z(-0.4),
            Quat::from_rotation_x(-1.1) * Quat::from_rotation_z(0.6),
        ],
        elbow: [-2.15, -1.6],
        hip: [Quat::from_rotation_x(-1.2), Quat::from_rotation_x(-0.9)],
        knee: [2.1, 1.7],
    };
    spawn_ragdoll(world, &blue, &hy, &curled, x(CURLED));

    spawn_dummy(
        world,
        &walker,
        x(STANDING - 0.45) + DVec3::Y * 0.9,
        Quat::IDENTITY,
    );
    spawn_dummy(
        world,
        &rider,
        x(STANDING + 0.45) + DVec3::Y * 0.9,
        Quat::from_rotation_y(-0.6),
    );

    for (i, m) in [&blue, &red, &green].into_iter().enumerate() {
        let j = Joints {
            root: about(Vec3::ZERO, Quat::from_rotation_y(0.35 * (i as f32 - 1.0))),
            ..Default::default()
        };
        spawn_ragdoll(world, m, &hy, &j, x(LINEUP - 0.8 + 0.8 * i as f64));
    }

    spawn_ragdoll(world, &old, &pg, &Joints::default(), x(LAYOUTS - 0.45));
    spawn_ragdoll(world, &red, &hy, &Joints::default(), x(LAYOUTS + 0.45));
    // A 1.8 m capsule for scale next to the layouts.
    world.spawn((
        Transform::from_translation(x(LAYOUTS + 1.2) + DVec3::Y * 0.9),
        MeshRenderer::new(
            Shape::Capsule {
                radius: 0.2,
                height: 1.8,
                segments: 24,
            },
            StandardMaterial::hex(0x8a8d92).with_roughness(0.6),
        ),
    ));
    let _ = Vec4::ONE;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).cloned().unwrap_or("dummy_shots".into()));
    let only = args.get(2).cloned().unwrap_or_default();
    let (w, h) = args
        .get(3)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<AmbientLight>().brightness = 400.0;
    app.world.resource_mut::<RenderSettings>().shadow_distance = 30.0;
    let camera = app
        .world
        .spawn((
            Camera::perspective(35.0),
            Transform::from_xyz(0.0, 1.0, 4.0),
        ))
        .id();
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.5, -0.7, -0.6), Vec3::Y),
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x8c8b86).with_roughness(0.9),
        ),
    ));
    build_scene(&mut app.world);

    for &(name, eye, target, fov) in SHOTS.iter().filter(|s| s.0.contains(only.as_str())) {
        *app.world.get_mut::<Camera>(camera).unwrap() = Camera::perspective(fov);
        *app.world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(DVec3::from_array(eye))
                .looking_at(DVec3::from_array(target), Vec3::Y);
        for _ in 0..4 {
            app.update();
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
