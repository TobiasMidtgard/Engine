//! GPU skinning (`GpuSkin`): a two-bone post bends at its joint when the upper bone turns, in
//! the view and its shadow, with dual quaternions and linear blending; an unchanged pose is not
//! skinned again. Needs a GPU or lavapipe; passes with a note when neither is available.

use std::sync::Arc;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    GpuSkin, Mesh, RenderDisabled, SkinBlend, SkinBone, SkinWeights, capture_now, mesh,
};

const W: u32 = 200;
const H: u32 = 200;

/// A 0.3 m square post 2 m tall standing on the origin: the lower meter on bone 0, the upper
/// on bone 1 (the joint at 1 m).
fn post() -> (Mesh, Arc<SkinWeights>) {
    let mut post = Mesh::new(Vec::new(), Vec::new());
    let mut weights = SkinWeights::default();
    for (bone, y) in [(0u16, 0.5f32), (1, 1.5)] {
        let mut part = mesh::cuboid(Vec3::new(0.3, 1.0, 0.3));
        part.transform(&Mat4::from_translation(Vec3::new(0.0, y, 0.0)));
        for _ in &part.vertices {
            weights.joints.push([bone, 0, 0, 0]);
            weights.weights.push([1.0, 0.0, 0.0, 0.0]);
        }
        post.merge(&part);
    }
    (post, Arc::new(weights))
}

/// The post with its upper bone turned `angle` about +Z at the joint, seen from the front;
/// the image's pixels and the meshes skinned in the last frame.
fn render(angle: f32, blend: SkinBlend, name: &str) -> Option<(image::RgbImage, f64)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.film_grain = 0.0;
        s.anti_aliasing = AntiAliasing::None;
    }
    let (post, weights) = post();
    let handle = app.world.resource_mut::<Assets<Mesh>>().add(post);
    let joint = (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0));
    let turned = (Quat::from_rotation_z(angle), joint.1);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(handle, StandardMaterial::color(0.8, 0.1, 0.1)),
        GpuSkin {
            weights,
            bones: vec![SkinBone::IDENTITY, SkinBone::from_poses(joint, turned)],
            blend,
        },
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20.0, 20.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.7, 0.7, 0.7),
        ),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.2, -1.0, -0.3), Vec3::Y),
    ));
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 1.2, 5.0).looking_at(DVec3::new(0.0, 1.2, 0.0), Vec3::Y),
    ));
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let skinned = app
        .world
        .resource::<Diagnostics>()
        .get("render.meshes.skinned")
        .unwrap_or(-1.0);
    let path = std::env::temp_dir().join(format!("engine-skinning-{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(
            &path,
            std::path::Path::new(&out).join(format!("{name}.png")),
        );
    }
    Some((image::open(&path).expect("read").to_rgb8(), skinned))
}

/// Share of the rectangle (fractions of the image) the red post covers.
fn red(image: &image::RgbImage, rect: [f32; 4]) -> f32 {
    let (x0, y0) = ((rect[0] * W as f32) as u32, (rect[1] * H as f32) as u32);
    let (x1, y1) = ((rect[2] * W as f32) as u32, (rect[3] * H as f32) as u32);
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = image.get_pixel(x, y);
            if p[0] as i32 > p[1] as i32 + 40 && p[0] as i32 > p[2] as i32 + 40 {
                n += 1;
            }
        }
    }
    n as f32 / ((x1 - x0) * (y1 - y0)) as f32
}

/// Above the joint, where the straight post stands.
const TOP: [f32; 4] = [0.48, 0.36, 0.52, 0.5];
/// Left of the joint, where the upper half lies once turned 90° about +Z.
const LEFT: [f32; 4] = [0.3, 0.525, 0.44, 0.555];

#[test]
fn a_two_bone_post_bends_at_its_joint() {
    let Some((straight, _)) = render(0.0, SkinBlend::DualQuaternion, "straight") else {
        return;
    };
    let (bent, _) = render(
        std::f32::consts::FRAC_PI_2,
        SkinBlend::DualQuaternion,
        "bent",
    )
    .expect("GPU");
    let (bent_linear, skinned) =
        render(std::f32::consts::FRAC_PI_2, SkinBlend::Linear, "linear").expect("GPU");
    eprintln!(
        "top: straight {:.2} bent {:.2}; left: straight {:.2} bent {:.2} linear {:.2}",
        red(&straight, TOP),
        red(&bent, TOP),
        red(&straight, LEFT),
        red(&bent, LEFT),
        red(&bent_linear, LEFT)
    );
    assert!(red(&straight, TOP) > 0.8 && red(&straight, LEFT) < 0.05);
    assert!(red(&bent, TOP) < 0.05, "bent: the upper half left the top");
    assert!(
        red(&bent, LEFT) > 0.5,
        "bent: the upper half lies to the left"
    );
    assert!(red(&bent_linear, LEFT) > 0.5, "linear: the same");
    // The pose stayed the same for the last frames: nothing skinned again.
    assert_eq!(skinned, 0.0, "meshes skinned in a frame with the same pose");
}
