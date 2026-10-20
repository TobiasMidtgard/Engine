//! Renders particles offscreen and checks the pixels: glowing particles appear where their
//! emitter is (also 30 000 km from the world origin), a spell's light brightens the ground
//! around it, smoke lit by the sun covers what's behind it, nearer puffs cover farther ones,
//! an engine plume shows shock diamonds only in air, a splash's crown shows where it stands
//! and nowhere else, and marks change the ground. Needs a GPU or a software
//! Vulkan driver (lavapipe); passes with a note when neither is available (`RenderDisabled`),
//! unless `ENGINE_REQUIRE_GPU=1`.

use engine_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::prelude::*;
use engine_vfx::{EffectDesc, Gradient, Mark, MarkKind, Marks, Sprite};

const W: u32 = 320;
const H: u32 = 180;
const FOV_DEG: f32 = 60.0;
const EYE: DVec3 = DVec3::new(0.0, 1.5, 6.0);
const TARGET: DVec3 = DVec3::new(0.0, 1.0, 0.0);

/// A dusk scene around `offset`: low sun, dark ground, fixed exposure.
fn app(offset: DVec3, sun_lux: f32) -> App {
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
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, VfxPlugin));
    app.world.resource_mut::<Atmosphere>().planet_center =
        offset + DVec3::new(0.0, -6_360_000.0, 0.0);
    // The flat ground the particles collide with follows the scene.
    app.insert_resource(engine_world::Ground(std::sync::Arc::new(FlatAt(offset.y))));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: sun_lux,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.5, -0.5, -0.7), Vec3::Y),
    ));
    w.spawn((
        Transform::from_translation(offset),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5a5550).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Camera::perspective(FOV_DEG).with_exposure(Exposure::Manual { ev100: 8.0 }),
        Transform::from_translation(offset + EYE).looking_at(offset + TARGET, Vec3::Y),
    ));
    app
}

struct FlatAt(f64);

impl engine_world::GroundQuery for FlatAt {
    fn sample(&self, _p: DVec3) -> Option<engine_world::GroundSample> {
        Some(engine_world::GroundSample {
            height: self.0,
            normal: Vec3::Y,
            surface: engine_world::SurfaceId(2),
        })
    }
}

/// Render and save a screenshot. `None` when there is no GPU.
fn shot(app: &mut App, name: &str) -> Option<image::RgbaImage> {
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let info = capture_now(&mut app.world, &path).expect("screenshot");
    assert_eq!((info.width, info.height), (W, H));
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Pixel position of a point relative to the scene offset (same math as the renderer).
fn project(p: DVec3) -> (u32, u32) {
    let eye = Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y);
    let proj = engine_render::frame::perspective_infinite_reverse(
        FOV_DEG.to_radians(),
        W as f32 / H as f32,
        0.05,
    );
    let view = Mat4::from_quat(Quat::inverse(eye.rotation));
    let rel = (p - eye.translation).as_vec3();
    let clip = proj * view * Vec4::new(rel.x, rel.y, rel.z, 1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as u32,
        ((0.5 - ndc.y * 0.5) * H as f32) as u32,
    )
}

/// Average luma (0..255) in a square of `r` pixels around `(x, y)`.
fn luma(img: &image::RgbaImage, (x, y): (u32, u32), r: i32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    for dy in -r..=r {
        for dx in -r..=r {
            let (px, py) = (x as i32 + dx, y as i32 + dy);
            if px < 0 || py < 0 || px >= W as i32 || py >= H as i32 {
                continue;
            }
            let c = img.get_pixel(px as u32, py as u32).0;
            sum += 0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32;
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

/// A still cloud of glowing motes that lives long enough to be photographed.
fn glow_cloud() -> EffectDesc {
    EffectDesc {
        name: "test.glow".into(),
        emitters: vec![EmitterDesc {
            bursts: vec![engine_vfx::Burst {
                count: 400,
                ..Default::default()
            }],
            shape: engine_vfx::EmitShape::Sphere {
                radius: 0.35,
                surface: false,
            },
            speed: [0.0, 0.0],
            lifetime: [20.0, 20.0],
            size: [0.12, 0.2],
            sprite: Sprite::Glow,
            color: Gradient::constant([1.0, 0.5, 0.15, 1.0]),
            emissive: 4_000.0,
            additive: 1.0,
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn glowing_particles_appear_where_the_emitter_is_even_far_from_the_origin() {
    for (i, offset) in [DVec3::ZERO, DVec3::new(3.0e7, 0.0, -2.0e7)]
        .into_iter()
        .enumerate()
    {
        let mut app = app(offset, 400.0);
        let Some(before) = shot(&mut app, &format!("vfx_glow_{i}_before.png")) else {
            return;
        };
        app.world
            .resource_mut::<EffectLibrary>()
            .insert(glow_cloud())
            .unwrap();
        app.world.spawn((
            Transform::from_translation(offset + TARGET),
            ParticleEffect::new("test.glow"),
        ));
        let after = shot(&mut app, &format!("vfx_glow_{i}_after.png")).unwrap();
        let center = project(TARGET);
        let (b, a) = (luma(&before, center, 3), luma(&after, center, 3));
        assert!(a > b + 60.0, "offset {offset}: center luma {b} -> {a}");
        // Far from the cloud nothing changed.
        let corner = (W / 10, H / 10);
        let (b, a) = (luma(&before, corner, 2), luma(&after, corner, 2));
        assert!(
            (a - b).abs() < 6.0,
            "offset {offset}: corner luma {b} -> {a}"
        );
    }
}

#[test]
fn a_spell_lights_the_ground_around_it() {
    let mut app = app(DVec3::ZERO, 200.0);
    let ground = project(DVec3::new(1.2, 0.0, 1.0));
    let Some(before) = shot(&mut app, "vfx_spell_before.png") else {
        return;
    };
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 0.8, 0.0)),
        ParticleEffect::new("spell.fireball"),
    ));
    app.update_n(10);
    let after = shot(&mut app, "vfx_spell_after.png").unwrap();
    let (b, a) = (luma(&before, ground, 3), luma(&after, ground, 3));
    assert!(a > b + 10.0, "ground luma {b} -> {a}");
    // The core glows.
    let core = project(DVec3::new(0.0, 0.8, 0.0));
    assert!(luma(&after, core, 2) > 150.0);
}

#[test]
fn sunlit_smoke_covers_what_is_behind_it() {
    let mut app = app(DVec3::ZERO, 30_000.0);
    // A bright emissive panel behind where the smoke will be.
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 1.0, -3.0)),
        MeshRenderer::new(
            Shape::Quad { size: [6.0, 4.0] },
            StandardMaterial::hex(0x000000).with_emissive([1.0, 1.0, 1.0], 20_000.0),
        ),
    ));
    let at = project(DVec3::new(0.0, 1.0, 0.0));
    let Some(before) = shot(&mut app, "vfx_smoke_before.png") else {
        return;
    };
    let smoke = EffectDesc {
        name: "test.smoke".into(),
        emitters: vec![EmitterDesc {
            bursts: vec![engine_vfx::Burst {
                count: 120,
                ..Default::default()
            }],
            shape: engine_vfx::EmitShape::Sphere {
                radius: 0.4,
                surface: false,
            },
            speed: [0.0, 0.0],
            lifetime: [20.0, 20.0],
            size: [1.2, 1.5],
            sprite: Sprite::Smoke,
            lit: 1.0,
            color: Gradient::constant([0.12, 0.12, 0.12, 1.0]),
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(smoke)
        .unwrap();
    app.world.spawn((
        Transform::from_translation(TARGET),
        ParticleEffect::new("test.smoke"),
    ));
    let after = shot(&mut app, "vfx_smoke_after.png").unwrap();
    let (b, a) = (luma(&before, at, 3), luma(&after, at, 3));
    assert!(
        a < b - 30.0,
        "dark smoke should hide the bright panel: {b} -> {a}"
    );
}

#[test]
fn nearer_puffs_cover_farther_ones() {
    // Opaque puffs stream from the target straight at the camera, black when born and white
    // as they age, so all of them stack on the same pixels. Drawn in spawn order the newest,
    // farthest (black) one would cover the rest; sorted far to near, the oldest, nearest
    // (whitest) one is on top.
    let mut app = app(DVec3::ZERO, 200.0);
    let center = project(TARGET);
    let Some(before) = shot(&mut app, "vfx_sort_before.png") else {
        return;
    };
    let toward_eye = (EYE - TARGET).normalize().as_vec3() * 2.0;
    let stream = EffectDesc {
        name: "test.stream".into(),
        emitters: vec![EmitterDesc {
            rate: 30.0,
            speed: [0.0, 0.0],
            velocity: toward_eye.to_array(),
            lifetime: [1.5, 1.5],
            size: [0.5, 0.5],
            sprite: Sprite::Glow,
            color: Gradient::new(&[(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])]),
            emissive: 4_000.0,
            additive: 0.0,
            soft: 0.0,
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(stream)
        .unwrap();
    app.world.spawn((
        Transform::from_translation(TARGET),
        ParticleEffect::new("test.stream"),
    ));
    app.update_n(26);
    let after = shot(&mut app, "vfx_sort_after.png").unwrap();
    let (b, a) = (luma(&before, center, 2), luma(&after, center, 2));
    assert!(
        a > b + 80.0,
        "the nearest, whitest puff should be on top: {b} -> {a}"
    );
}

#[test]
fn a_plume_shows_shock_diamonds_in_air_but_not_in_a_vacuum() {
    // A plume lying along +X across the view, its nozzle at x = -2. In air its axis is banded:
    // bright knots at the shock diamonds and darker gas between them. Without air there are no
    // diamonds, so the same spots are as bright as the gaps.
    const LENGTH: f64 = 4.0;
    let nozzle = DVec3::new(-2.0, 1.0, 0.0);
    // Knots and gaps alternate down the axis (see `plume` in particles_draw.wgsl), and their
    // averages sit at the same distance, so the flame dimming along its length cancels.
    let knots = [0.85, 1.85, 2.85].map(|k: f64| k / 6.5);
    let gaps = [1.35, 2.35].map(|k: f64| k / 6.5);
    let mut contrast = Vec::new();
    for vacuum in [false, true] {
        let mut app = app(DVec3::ZERO, 200.0);
        app.add_plugins(engine_world::WorldPlugin);
        if vacuum {
            app.world.resource_mut::<engine_world::Atmospheres>().top_m = 0.0;
        }
        let plume = EffectDesc {
            name: "test.plume".into(),
            emitters: vec![EmitterDesc {
                rate: 12.0,
                speed: [0.0, 0.0],
                lifetime: [0.2, 0.2],
                size: [LENGTH as f32, LENGTH as f32],
                rotation_deg: [0.0, 0.0],
                local_space: true,
                sprite: Sprite::Plume,
                facing: engine_vfx::Facing::Axis,
                stretch: 0.3,
                color: Gradient::constant([1.0, 0.6, 0.3, 1.0]),
                emissive: 500.0,
                additive: 1.0,
                soft: 0.0,
                wind: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        app.world
            .resource_mut::<EffectLibrary>()
            .insert(plume)
            .unwrap();
        app.world.spawn((
            Transform::from_translation(nozzle)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::X)),
            ParticleEffect::new("test.plume"),
        ));
        app.update_n(12);
        let name = if vacuum { "vacuum" } else { "air" };
        let Some(img) = shot(&mut app, &format!("vfx_plume_{name}.png")) else {
            return;
        };
        let along = |t: f64| luma(&img, project(nozzle + DVec3::X * (t * LENGTH)), 1);
        let mean = |ts: &[f64]| ts.iter().map(|t| along(*t)).sum::<f32>() / ts.len() as f32;
        let (k, g) = (mean(&knots), mean(&gaps));
        eprintln!("{name}: knots {k}, gaps {g}");
        assert!(k > 30.0, "{name}: the plume should glow, knots {k}");
        contrast.push(k - g);
    }
    assert!(
        contrast[0] > 20.0 && contrast[0] > contrast[1] * 2.0 + 10.0,
        "diamonds should stand out in air but not in a vacuum: knots - gaps {contrast:?}"
    );
}

#[test]
fn a_blaze_burns_hottest_low_in_its_heart_and_leaves_the_air_beside_it_clear() {
    // A fire 2 m tall and 2 m wide standing on the origin. Its body is brightest low in the
    // middle and cools toward its tongues; its ragged edge never strays out to the quad's sides.
    let mut app = app(DVec3::ZERO, 200.0);
    let Some(before) = shot(&mut app, "vfx_blaze_before.png") else {
        return;
    };
    let blaze = EffectDesc {
        name: "test.blaze".into(),
        emitters: vec![EmitterDesc {
            rate: 12.0,
            speed: [0.0, 0.0],
            lifetime: [0.2, 0.2],
            size: [2.0, 2.0],
            rotation_deg: [0.0, 0.0],
            local_space: true,
            sprite: Sprite::Blaze,
            facing: engine_vfx::Facing::Axis,
            stretch: 1.0,
            color: Gradient::constant([1.0, 0.6, 0.25, 1.0]),
            emissive: 500.0,
            additive: 1.0,
            soft: 0.0,
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(blaze)
        .unwrap();
    app.world.spawn((
        Transform::from_translation(DVec3::ZERO),
        ParticleEffect::new("test.blaze"),
    ));
    app.update_n(12);
    let after = shot(&mut app, "vfx_blaze_after.png").unwrap();
    let lift = |x: f64, y: f64, r: i32| {
        let at = project(DVec3::new(x, y, 0.0));
        luma(&after, at, r) - luma(&before, at, r)
    };
    let heart = lift(0.0, 0.4, 2);
    let top = lift(0.0, 1.1, 2);
    eprintln!("blaze: heart {heart}, top {top}");
    assert!(heart > 40.0, "the fire's heart should glow: {heart}");
    assert!(
        top < heart * 0.6,
        "the fire should cool toward its tongues: {heart} -> {top}"
    );
    for x in [-0.95, 0.95] {
        let side = lift(x, 0.3, 1).abs();
        assert!(
            side < 3.0,
            "no flame should stand off at the side ({x}): {side}"
        );
    }
}

#[test]
fn a_fireball_glows_while_young_and_chars_to_soot() {
    // One ball of burning gas 2 m across standing over the ground in daylight: young, it glows
    // brighter than the ground behind it; near the end of its life it is dark soot covering it.
    let mut app = app(DVec3::ZERO, 30_000.0);
    let Some(before) = shot(&mut app, "vfx_fireball_before.png") else {
        return;
    };
    let ball = EffectDesc {
        name: "test.fireball".into(),
        emitters: vec![EmitterDesc {
            bursts: vec![engine_vfx::Burst {
                count: 1,
                ..Default::default()
            }],
            offset: [0.0, 0.8, 0.0],
            speed: [0.0, 0.0],
            gravity: 0.0,
            lifetime: [2.0, 2.0],
            size: [2.0, 2.0],
            sprite: Sprite::Fireball,
            lit: 1.0,
            color: Gradient::constant([1.0, 0.45, 0.12, 1.0]),
            emissive: 18_000.0,
            soft: 0.0,
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(ball)
        .unwrap();
    app.world.spawn((
        Transform::from_translation(DVec3::ZERO),
        ParticleEffect::new("test.fireball"),
    ));
    let heart = project(DVec3::new(0.0, 0.8, 0.0));
    app.update_n(4);
    let young = shot(&mut app, "vfx_fireball_young.png").unwrap();
    app.update_n(40);
    let old = shot(&mut app, "vfx_fireball_old.png").unwrap();
    let (b, y, o) = (
        luma(&before, heart, 2),
        luma(&young, heart, 2),
        luma(&old, heart, 2),
    );
    eprintln!("fireball: ground {b}, young {y}, old {o}");
    assert!(
        y > b + 30.0,
        "a young fireball should glow: ground {b}, young {y}"
    );
    assert!(
        o < b - 10.0,
        "an old fireball should be dark soot: ground {b}, old {o}"
    );
}

#[test]
fn a_splash_crown_stands_out_of_the_ground_and_nothing_else_changes() {
    let mut app = app(DVec3::ZERO, 30_000.0);
    let Some(before) = shot(&mut app, "vfx_crown_before.png") else {
        return;
    };
    let crown = EffectDesc {
        name: "test.crown".into(),
        emitters: vec![EmitterDesc {
            bursts: vec![engine_vfx::Burst {
                count: 1,
                ..Default::default()
            }],
            speed: [0.0, 0.0],
            gravity: 0.0,
            lifetime: [1.0, 1.0],
            size: [3.0, 3.0],
            sprite: Sprite::Crown,
            lit: 1.0,
            color: Gradient::constant([1.0, 1.0, 1.0, 1.0]),
            wind: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(crown)
        .unwrap();
    app.world.spawn((
        Transform::from_translation(DVec3::ZERO),
        ParticleEffect::new("test.crown"),
    ));
    // A third of the way through, the crown stands at its full height.
    app.update_n(8);
    let after = shot(&mut app, "vfx_crown_after.png").unwrap();
    // Its wall leans out to about 1 m and stands about 1 m tall. Glassy water shows faintly
    // against the bright ground, most where it mirrors the sky at a glance.
    let mut most = 0.0f32;
    for i in 0..=20 {
        for j in 1..=4 {
            let at = project(DVec3::new(-1.2 + i as f64 * 0.12, j as f64 * 0.2, 0.0));
            most = most.max((luma(&after, at, 1) - luma(&before, at, 1)).abs());
        }
    }
    assert!(most > 10.0, "the crown's wall should show: {most}");
    let away = project(DVec3::new(3.5, 0.5, 0.0));
    let off = (luma(&after, away, 2) - luma(&before, away, 2)).abs();
    assert!(off < 3.0, "nothing beyond the crown should change: {off}");
}

#[test]
fn sparks_stick_to_a_wall_the_camera_sees() {
    // Glowing motes fly at a wall; with scene collision they stop on its face, without it they
    // pass behind it and vanish.
    let hit = project(DVec3::new(0.0, 1.0, -0.75));
    let mut lumas = Vec::new();
    for collide in [true, false] {
        let mut app = app(DVec3::ZERO, 200.0);
        app.world.resource_mut::<VfxSettings>().scene_collision = collide;
        app.world.spawn((
            Transform::from_xyz(0.0, 1.0, -1.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [4.0, 3.0, 0.5],
                },
                StandardMaterial::hex(0x303030).with_roughness(0.9),
            ),
        ));
        if shot(&mut app, "vfx_wall_before.png").is_none() {
            return;
        }
        let darts = EffectDesc {
            name: "test.darts".into(),
            emitters: vec![EmitterDesc {
                bursts: vec![engine_vfx::Burst {
                    count: 300,
                    ..Default::default()
                }],
                shape: engine_vfx::EmitShape::Sphere {
                    radius: 0.15,
                    surface: false,
                },
                speed: [0.0, 0.0],
                velocity: [0.0, 0.0, -6.0],
                lifetime: [20.0, 20.0],
                size: [0.08, 0.12],
                sprite: Sprite::Glow,
                color: Gradient::constant([1.0, 0.6, 0.2, 1.0]),
                emissive: 4_000.0,
                additive: 1.0,
                wind: 0.0,
                collide: engine_vfx::Collide::Stick,
                ..Default::default()
            }],
            ..Default::default()
        };
        app.world
            .resource_mut::<EffectLibrary>()
            .insert(darts)
            .unwrap();
        app.world.spawn((
            Transform::from_xyz(0.0, 1.0, 2.0),
            ParticleEffect::new("test.darts"),
        ));
        app.update_n(40);
        let name = format!("vfx_wall_collide_{collide}.png");
        let img = shot(&mut app, &name).unwrap();
        lumas.push(luma(&img, hit, 3));
    }
    assert!(
        lumas[0] > lumas[1] + 40.0,
        "stuck on the wall {} vs passed behind it {}",
        lumas[0],
        lumas[1]
    );
}

#[test]
fn hits_and_deaths_feed_other_layers_on_the_gpu() {
    // Invisible drops fall onto the ground on the right and invisible seeds expire in the air
    // on the left. Only their sub-emitters make anything glow: glints where the drops land,
    // stars where the seeds die. With the chance at 0 nothing appears.
    use engine_vfx::{Burst, Collide, EmitShape, SubEmit};
    let landed = project(DVec3::new(1.2, 0.05, 0.0));
    let burst = project(DVec3::new(-1.2, 1.6, 0.0));
    let unseen = Gradient::constant([1.0, 1.0, 1.0, 0.0]);
    let glow = |name: &str, rgb: [f32; 3], shape: EmitShape| EmitterDesc {
        name: name.into(),
        shape,
        speed: [0.0, 0.0],
        lifetime: [20.0, 20.0],
        size: [0.12, 0.18],
        sprite: Sprite::Glow,
        color: Gradient::constant([rgb[0], rgb[1], rgb[2], 1.0]),
        emissive: 4_000.0,
        additive: 1.0,
        wind: 0.0,
        ..Default::default()
    };
    let effect = |chance: f32| EffectDesc {
        name: "test.subs".into(),
        duration: Some(0.1),
        emitters: vec![
            EmitterDesc {
                name: "drops".into(),
                bursts: vec![Burst {
                    count: 60,
                    ..Default::default()
                }],
                shape: EmitShape::Disc { radius: 0.15 },
                offset: [1.2, 1.0, 0.0],
                speed: [0.0, 0.0],
                gravity: 1.0,
                lifetime: [3.0, 3.0],
                collide: Collide::Bounce {
                    restitution: 0.2,
                    friction: 0.5,
                },
                color: unseen.clone(),
                wind: 0.0,
                on_hit: Some(SubEmit {
                    chance,
                    ..SubEmit::new("glints", 3)
                }),
                ..Default::default()
            },
            EmitterDesc {
                name: "seeds".into(),
                bursts: vec![Burst {
                    count: 60,
                    ..Default::default()
                }],
                shape: EmitShape::Point,
                offset: [-1.2, 1.6, 0.0],
                speed: [0.0, 0.0],
                lifetime: [0.3, 0.3],
                color: unseen.clone(),
                wind: 0.0,
                on_death: Some(SubEmit {
                    chance,
                    ..SubEmit::new("stars", 3)
                }),
                ..Default::default()
            },
            glow("glints", [1.0, 0.6, 0.2], EmitShape::Point),
            glow(
                "stars",
                [0.4, 0.6, 1.0],
                EmitShape::Sphere {
                    radius: 0.3,
                    surface: false,
                },
            ),
        ],
        ..Default::default()
    };
    let mut lumas = Vec::new();
    for chance in [1.0, 0.0] {
        let mut app = app(DVec3::ZERO, 200.0);
        let Some(before) = shot(&mut app, "vfx_subs_before.png") else {
            return;
        };
        let desc = effect(chance);
        desc.validate().unwrap();
        app.world
            .resource_mut::<EffectLibrary>()
            .insert(desc)
            .unwrap();
        app.world
            .spawn((Transform::default(), ParticleEffect::new("test.subs")));
        app.update_n(36);
        let after = shot(&mut app, &format!("vfx_subs_{chance}.png")).unwrap();
        lumas.push([
            luma(&after, landed, 3) - luma(&before, landed, 3),
            luma(&after, burst, 3) - luma(&before, burst, 3),
        ]);
    }
    let [[land_on, burst_on], [land_off, burst_off]] = [lumas[0], lumas[1]];
    assert!(land_on > 40.0, "glints where the drops landed: +{land_on}");
    assert!(burst_on > 40.0, "stars where the seeds died: +{burst_on}");
    assert!(
        land_off.abs() < 6.0 && burst_off.abs() < 6.0,
        "nothing without sub-emitters: {land_off} {burst_off}"
    );
}

#[test]
fn particles_stay_put_when_the_world_re_anchors() {
    // A still cloud in front of the camera; then the world moves to a new origin (a plain
    // floating-origin shift), then to a turned frame (as when entering another planet's
    // sphere of influence). The camera moves with the world, so the cloud must stay on screen
    // where it was.
    let mut app = app(DVec3::ZERO, 400.0);
    app.add_plugins(engine_world::WorldPlugin);
    app.world
        .resource_mut::<EffectLibrary>()
        .insert(glow_cloud())
        .unwrap();
    app.world.spawn((
        Transform::from_translation(TARGET),
        ParticleEffect::new("test.glow"),
    ));
    let Some(first) = shot(&mut app, "vfx_shift_0.png") else {
        return;
    };
    let center = project(TARGET);
    let aside = project(TARGET + DVec3::new(2.0, 0.0, 0.0));
    let lit = luma(&first, center, 3);
    assert!(lit > luma(&first, aside, 2) + 60.0, "cloud not visible");
    let shifts = [
        engine_world::WorldShift::translate(DVec3::new(250.0, 0.0, -400.0)),
        engine_world::WorldShift {
            rotation: engine_core::math::DQuat::from_rotation_y(1.2),
            translation: DVec3::new(-30.0, 0.0, 75.0),
            ..engine_world::WorldShift::IDENTITY
        },
    ];
    for (i, s) in shifts.into_iter().enumerate() {
        app.world.write_message(s);
        let img = shot(&mut app, &format!("vfx_shift_{}.png", i + 1)).unwrap();
        let (c, a) = (luma(&img, center, 3), luma(&img, aside, 2));
        assert!(
            c > lit * 0.7 && c > a + 60.0,
            "after shift {}: center {c} (was {lit}), aside {a}",
            i + 1
        );
    }
}

#[test]
fn marks_char_frost_and_wet_the_ground() {
    let mut app = app(DVec3::ZERO, 3000.0);
    let spots = [
        DVec3::new(-1.6, 0.0, 1.5),
        DVec3::new(0.0, 0.0, 1.5),
        DVec3::new(1.6, 0.0, 1.5),
    ];
    let Some(before) = shot(&mut app, "vfx_marks_before.png") else {
        return;
    };
    {
        let mut marks = app.world.resource_mut::<Marks>();
        let mark = |kind, at, r| Mark {
            life_secs: 600.0,
            ..Mark::round(kind, at, Vec3::Y, r)
        };
        marks.add(mark(MarkKind::Scorch, spots[0], 0.6));
        marks.add(mark(MarkKind::Frost, spots[1], 0.6));
        marks.add(mark(MarkKind::Wet, spots[2], 0.6));
    }
    let after = shot(&mut app, "vfx_marks_after.png").unwrap();
    let at = |img: &image::RgbaImage, p: DVec3| luma(img, project(p), 2);
    let (b, a) = (at(&before, spots[0]), at(&after, spots[0]));
    assert!(a < b * 0.6, "scorch: {b} -> {a}");
    let (b, a) = (at(&before, spots[1]), at(&after, spots[1]));
    assert!(a > b + 15.0, "frost: {b} -> {a}");
    let (b, a) = (at(&before, spots[2]), at(&after, spots[2]));
    assert!(a < b * 0.85, "wet: {b} -> {a}");
    // Away from the marks nothing changed.
    let aside = DVec3::new(0.0, 0.0, -2.5);
    assert!((at(&after, aside) - at(&before, aside)).abs() < 2.0);
}
