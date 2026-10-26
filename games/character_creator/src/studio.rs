//! The portrait studio: a soft key light from the front left, a grey backdrop, a warm rim
//! light behind, and a camera that orbits the subject's face.

use engine::prelude::*;
use engine::world::animation::SkeletonPose;
use engine_character::{Appearance, AppearanceExt, BuiltCharacter, CharacterBuild};

use crate::Subject;

/// The backdrop, kept opposite the camera.
#[derive(Component)]
pub struct Backdrop;

/// Where the camera sits around the subject's eyes.
#[derive(Component, Debug, Clone, Copy)]
pub struct Orbit {
    /// Around the subject, radians (0 = straight in front).
    pub yaw: f32,
    /// Above (+) or below the eyes, radians.
    pub pitch: f32,
    /// From the eyes, meters.
    pub distance: f32,
    /// Aim this far below the eyes, meters (the face's center is the nose).
    pub drop: f32,
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
}

impl Orbit {
    pub const CLOSE_UP: Orbit = Orbit {
        yaw: 0.0,
        pitch: 0.02,
        distance: 0.78,
        drop: 0.035,
        fov_deg: 24.0,
    };
}

/// Exposure for the studio light (EV100).
pub const EV100: f32 = 14.0;

/// `--daylight`: the subject outdoors at midday under the open sky (no backdrop, no studio
/// lamps), to check how people read in the game's own light.
fn daylight() -> bool {
    std::env::args().any(|a| a == "--daylight")
}

pub fn setup(mut commands: Commands, mut settings: ResMut<RenderSettings>) {
    let daylight = daylight();
    // Close-up quality: tight, sharp shadow cascades and short-range occlusion.
    settings.shadow_distance = 6.0;
    settings.far_shadow_distance = 0.0;
    settings.shadow_softness = 1.0;
    settings.sun_penumbra_scale = 3.0;
    settings.contact_shadow_length = 0.03;
    settings.ao_radius = 0.06;
    settings.volumetric_fog = false;
    settings.vignette = 0.25;
    settings.sharpness = 0.25;

    let mut camera = Camera::perspective(Orbit::CLOSE_UP.fov_deg);
    // Outdoors the camera adapts like the game's.
    camera.exposure = if daylight {
        Exposure::default()
    } else {
        Exposure::Manual { ev100: EV100 }
    };
    if let Projection::Perspective { near, .. } = &mut camera.projection {
        *near = 0.02;
    }
    commands.spawn((
        Name::new("camera"),
        camera,
        Orbit::CLOSE_UP,
        Transform::from_xyz(0.0, 1.7, 0.8),
    ));
    if daylight {
        // Late-morning sun from the front left, the open sky does the rest.
        commands.spawn((
            Name::new("sun"),
            DirectionalLight {
                color: [1.0, 0.97, 0.92],
                illuminance: 100_000.0,
                cast_shadows: true,
                angular_size_deg: 0.53,
            },
            Transform::default().looking_at(DVec3::new(0.5, -0.6, -0.62), Vec3::Y),
        ));
    } else {
        // Key light: late-afternoon sun from the front left, softened like a big window.
        commands.spawn((
            Name::new("key light"),
            DirectionalLight {
                color: [0.97, 0.97, 1.0],
                illuminance: 90_000.0,
                cast_shadows: true,
                angular_size_deg: 2.0,
            },
            Transform::default().looking_at(DVec3::new(0.55, -0.62, -0.56), Vec3::Y),
        ));
        // Rim light behind the right shoulder: separates hair and jaw from the backdrop.
        commands.spawn((
            Name::new("rim light"),
            PointLight {
                color: [1.0, 0.86, 0.7],
                intensity_lm: 260_000.0,
                range: 6.0,
                radius: 0.25,
            },
            Transform::from_xyz(0.9, 2.1, -0.9),
        ));
        // Fill on the other side of the camera from the key (the subject's left, +X), soft and
        // a little cool, lifting the shadow side to about a third of the key, as a portrait
        // photographer would.
        commands.spawn((
            Name::new("fill light"),
            PointLight {
                color: [0.94, 0.97, 1.0],
                intensity_lm: 260_000.0,
                range: 8.0,
                radius: 0.6,
            },
            Transform::from_xyz(1.3, 1.65, 1.4).looking_at(DVec3::new(0.0, 1.7, 0.0), Vec3::Y),
        ));
    }
    // A backdrop that stays behind the subject whichever way the camera turns (it casts no
    // shadow, so it never blocks the key light), and a floor.
    let mut backdrop = MeshRenderer::new(
        Shape::Cuboid {
            size: [6.0, 4.0, 0.05],
        },
        StandardMaterial::hex(0x55585e).with_roughness(0.95),
    );
    backdrop.cast_shadows = false;
    if !daylight {
        commands.spawn((
            Name::new("backdrop"),
            Backdrop,
            Transform::from_xyz(0.0, 1.6, -1.6),
            backdrop,
        ));
    }
    commands.spawn((
        Name::new("floor"),
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20.0, 20.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(if daylight { 0xb08850 } else { 0x2e2d2b }).with_roughness(0.9),
        ),
    ));
    commands.spawn((
        Name::new("subject"),
        Subject,
        Transform::from_xyz(0.0, 1.0, 0.0),
        Appearance::preset("Kofi").unwrap_or_default(),
        CharacterBuild::portrait(),
    ));
}

type SubjectQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Appearance,
        Option<&'static BuiltCharacter>,
        Option<&'static SkeletonPose>,
    ),
    With<Subject>,
>;

/// A posed subject lives in the animation module's model space (facing −Z, standing on its
/// origin): turn it to face the camera and stand it on the floor. At rest it's a bust at
/// chest height.
pub fn face_camera(mut q: Query<(&mut Transform, Has<SkeletonPose>), With<Subject>>) {
    for (mut t, posed) in &mut q {
        let want = if posed {
            Transform::from_xyz(0.0, 0.0, 0.0)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI))
        } else {
            Transform::from_xyz(0.0, 1.0, 0.0)
        };
        if *t != want {
            *t = want;
        }
    }
}
type CameraQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Orbit,
        &'static mut Camera,
    ),
    (Without<Subject>, Without<Backdrop>),
>;
type BackdropQuery<'w, 's> =
    Query<'w, 's, &'static mut Transform, (With<Backdrop>, Without<Subject>, Without<Orbit>)>;

/// Places the camera from its [`Orbit`]; in the window, left-drag turns and scroll zooms.
#[allow(clippy::too_many_arguments)]
pub fn orbit_camera(
    config: Res<EngineConfig>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<Mouse>,
    ui: Option<Res<engine_ui::input::UiInputState>>,
    subject: SubjectQuery,
    mut cams: CameraQuery,
    mut backdrops: BackdropQuery,
    mut dragging: Local<bool>,
) {
    let eyes = crate::subject_eyes(subject.iter())
        .map(|(p, _)| p)
        .unwrap_or(DVec3::new(0.0, 1.73, 0.12));
    let over_ui = ui.is_some_and(|u| u.hit.is_some());
    if mouse.just_pressed(MouseButton::Left) {
        *dragging = !over_ui;
    }
    if !mouse.pressed(MouseButton::Left) {
        *dragging = false;
    }
    for (mut t, mut orbit, mut camera) in &mut cams {
        if !config.headless {
            if *dragging {
                orbit.yaw -= motion.delta.x * 0.006;
                orbit.pitch = (orbit.pitch + motion.delta.y * 0.004).clamp(-0.8, 0.8);
            }
            if !over_ui && motion.scroll.y != 0.0 {
                orbit.distance = (orbit.distance * 0.9f32.powf(motion.scroll.y)).clamp(0.35, 3.5);
            }
        }
        // Farther out, look at the shoulders too (whole-body views set their own drop).
        let drop = if orbit.distance > 2.5 {
            orbit.drop
        } else {
            orbit.drop + (orbit.distance - 0.78).max(0.0) * 0.12
        };
        let aim = eyes - DVec3::Y * drop as f64;
        let dir = Vec3::new(
            orbit.yaw.sin() * orbit.pitch.cos(),
            orbit.pitch.sin(),
            orbit.yaw.cos() * orbit.pitch.cos(),
        );
        *t = Transform::from_translation(aim + (dir * orbit.distance).as_dvec3())
            .looking_at(aim, Vec3::Y);
        for mut b in &mut backdrops {
            let behind = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
            *b = Transform::from_translation(
                DVec3::new(eyes.x, 1.6, eyes.z) + (behind * 1.6).as_dvec3(),
            )
            .with_rotation(Quat::from_rotation_y(orbit.yaw));
        }
        if let Projection::Perspective { fov_y_deg, .. } = &mut camera.projection
            && (*fov_y_deg - orbit.fov_deg).abs() > 1e-4
        {
            *fov_y_deg = orbit.fov_deg;
        }
    }
}
