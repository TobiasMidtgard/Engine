//! Draws a flag, a jelly cube, a rope and a head of hair offscreen and checks each shows up in
//! its color. Needs a GPU or software Vulkan (lavapipe); passes with a note when neither is
//! available (`RenderDisabled`), unless `ENGINE_REQUIRE_GPU=1`.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_softbody::{
    Cloth, ClothPins, HairGroom, HairStyle, Rope, RopeEnd, SoftBodyPlugin, SoftBodySim,
    SoftMaterial, SoftVolume,
};
use engine_world::animation::{BonePose, SkeletonPose};

const W: u32 = 480;
const H: u32 = 270;

fn scene() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((RenderPlugin, engine_world::WorldPlugin, SoftBodyPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.3, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [60.0, 60.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x808080).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.6, 6.0).looking_at(DVec3::new(0.0, 1.3, 0.0), Vec3::Y),
    ));
    // A red flag hanging from two corners (no wind needed to see it).
    w.spawn((
        Transform::from_xyz(-3.2, 2.6, 0.0),
        Cloth {
            size: [2.0, 1.4],
            resolution: [16, 12],
            pins: ClothPins::TopCorners,
            material: SoftMaterial::hex(0xd01010),
            ..Cloth::default()
        },
    ));
    // An opaque green jelly cube on the ground.
    let mut jelly = SoftVolume::jelly(0.8, SoftMaterial::hex(0x10c040).with_alpha(1.0));
    jelly.material.subsurface = 0.0;
    w.spawn((Transform::from_xyz(2.4, 0.45, 0.0), jelly));
    // A blue rope between two points.
    w.spawn((
        Transform::from_xyz(-0.8, 2.9, 1.0),
        Rope {
            end_a: RopeEnd::Here { offset: [0.0; 3] },
            end_b: RopeEnd::World {
                at: [0.8, 2.9, 1.0],
            },
            length: 2.2,
            radius: 0.04,
            material: SoftMaterial::hex(0x1040e0),
            ..Rope::default()
        },
    ));
    // Yellow hair on a one-bone skeleton.
    let head = w
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            SkeletonPose {
                names: Arc::from(vec!["head".to_string()]),
                parents: Arc::from(vec![None]),
                bones: vec![BonePose {
                    translation: Vec3::new(0.0, 1.5, 0.0),
                    ..BonePose::default()
                }],
            },
        ))
        .id();
    w.spawn((
        Transform::default(),
        HairGroom::long_hair(
            "head",
            HairStyle {
                tint: Some([0.75, 0.6, 0.02]),
                thickness: 0.002,
                ..HairStyle::long()
            },
        ),
        ChildOf(head),
    ));
    app
}

fn count(img: &image::RgbaImage, f: impl Fn(f32, f32, f32) -> bool) -> usize {
    img.pixels()
        .filter(|p| f(p.0[0] as f32, p.0[1] as f32, p.0[2] as f32))
        .count()
}

#[test]
fn cloth_jelly_rope_and_hair_are_drawn() {
    let mut app = scene();
    app.update_n(30);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    // Every body built and simulated.
    let mut q = app.world.query::<&SoftBodySim>();
    assert_eq!(q.iter(&app.world).count(), 4);
    assert!(
        q.iter(&app.world)
            .all(|b| b.steps > 0 && b.sim.pos.iter().all(|p| p.is_finite()))
    );

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("softbody_headless.png");
    let info = capture_now(&mut app.world, &path).expect("screenshot");
    assert_eq!((info.width, info.height), (W, H));
    eprintln!("wrote {}", path.display());
    let img = image::open(&path).expect("png").to_rgba8();

    let red = count(&img, |r, g, b| r > 60.0 && r > 2.0 * g && r > 2.0 * b);
    let green = count(&img, |r, g, b| g > 60.0 && g > 1.6 * r && g > 1.4 * b);
    let blue = count(&img, |r, g, b| b > 60.0 && b > 1.8 * r && b > 1.3 * g);
    let yellow = count(&img, |r, g, b| {
        r > 60.0 && g > 45.0 && r > 2.0 * b && g > 1.6 * b
    });
    eprintln!("red {red}, green {green}, blue {blue}, yellow {yellow}");
    assert!(red > 2000, "flag: {red} red pixels");
    assert!(green > 1000, "jelly: {green} green pixels");
    assert!(blue > 150, "rope: {blue} blue pixels");
    assert!(yellow > 50, "hair: {yellow} yellow pixels");
}
