//! Renders the combat and movement effects of the movement demo to PNGs, headless: a blade
//! trail at full speed and in slow motion, a parry, a counter-kill flash, slide dust and a
//! dash with full-body afterimages.
//!
//! ```text
//! cargo run -p engine_movement --release --example effects_preview -- [out_dir]
//! ```
//!
//! Each shot replays the deterministic demo twice: once without drawing to find the frame of
//! its moment, then again drawing only the frames just before it. The demo's stand-in swords
//! get a [`Blade`] (and a tip) here, and a small swing driver sweeps them through a slash on
//! the world clock, the way an animated rig would. Needs a GPU or lavapipe.

use std::path::{Path, PathBuf};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::{TimeDilation, TimeSource};
use engine_movement::combat::{
    AttackPhase, CombatLog, CombatRecord, CombatState, Fighter, Outcome,
};
use engine_movement::demo::{self, DuelDirector, Round};
use engine_movement::look::Blade;
use engine_movement::{Afterimages, MovementPlugin, MovementSet, Mover, TimePerception};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::{AttackKind, LocomotionMode, MoveIntent, WorldPlugin};

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
const DT: f64 = 1.0 / 60.0;
/// Longest run searched for a moment, real seconds.
const MAX_SECS: f64 = 40.0;
/// Frames drawn before each moment so exposure settles and trails fill.
const WARMUP_FRAMES: u64 = 14;
/// The stand-in blade: a long thin box under the sword entity.
const BLADE_SIZE: [f32; 3] = [0.04, 0.09, 1.1];

/// The demo's characters.
#[derive(Clone)]
struct Cast {
    runner: Entity,
    player: Entity,
    elite: Entity,
}

struct Shot {
    file: &'static str,
    round: Round,
    /// Reflex slow motion and cinematic beats on.
    perception: bool,
    /// What the duel player does.
    stance: Stance,
    /// Runs before every frame (cues such as slow motion).
    cue: fn(&mut World, &Cast),
    /// The moment: true on the frame to save.
    when: fn(&World, &Cast) -> bool,
    /// Camera (eye, target, vertical FOV in degrees).
    camera: fn(&World, &Cast) -> (DVec3, DVec3, f32),
}

fn pos(w: &World, e: Entity) -> DVec3 {
    w.get::<GlobalTransform>(e).unwrap().translation()
}

fn combat(w: &World, e: Entity) -> &CombatState {
    w.get::<CombatState>(e).unwrap()
}

fn mover(w: &World, e: Entity) -> &Mover {
    w.get::<Mover>(e).unwrap()
}

/// Fraction of the current phase done (0..1) if `e` is in `phase`.
fn phase_done(w: &World, e: Entity, phase: AttackPhase) -> Option<f32> {
    let a = combat(w, e).attack.filter(|a| a.phase == phase)?;
    let f = w.get::<Fighter>(e).unwrap();
    let total = match phase {
        AttackPhase::DashIn => return Some(0.0),
        AttackPhase::Windup if a.kind == AttackKind::Dash => f.dash_windup_secs,
        AttackPhase::Windup => f.windup_secs,
        AttackPhase::Strike => f.strike_secs,
        AttackPhase::Recovery => f.recovery_secs,
    };
    Some((1.0 - a.secs_left / total.max(1e-4)).clamp(0.0, 1.0))
}

/// World seconds since the last combat record that `pick` accepts.
fn since_record(w: &World, pick: impl Fn(&CombatRecord) -> bool) -> Option<f64> {
    let r = w
        .resource::<CombatLog>()
        .records
        .iter()
        .rev()
        .find(|r| pick(r))?;
    Some(w.resource::<Time>().elapsed_secs_f64() - r.world_secs)
}

fn no_cue(_: &mut World, _: &Cast) {}

/// Frames the runner has been sliding (its first slide).
#[derive(Resource, Default)]
struct SlideFrames(u32);

fn count_slide_frames(w: &mut World, c: &Cast) {
    let sliding = mover(w, c.runner).mode == LocomotionMode::Slide;
    let mut frames = w.get_resource_or_init::<SlideFrames>();
    if sliding {
        frames.0 += 1;
    } else if frames.0 > 0 && frames.0 < 20 {
        // Too short to show: wait for the next one.
        frames.0 = 0;
    }
}

/// Side-on view of the duel between the player and the elite.
fn duel_camera(w: &World, c: &Cast, side: f64, back: f64, fov: f32) -> (DVec3, DVec3, f32) {
    let (p, e) = (pos(w, c.player), pos(w, c.elite));
    let mid = (p + e) * 0.5;
    let along = (p - e).with_y(0.0).normalize_or(DVec3::Z);
    let across = along.cross(DVec3::Y);
    (
        mid + across * side + along * back + DVec3::new(0.0, 0.25, 0.0),
        mid + DVec3::new(0.0, 0.1, 0.0),
        fov,
    )
}

const SHOTS: &[Shot] = &[
    Shot {
        file: "01_strike_full_speed.png",
        round: Round::Blink,
        perception: false,
        stance: Stance::Brain,
        cue: no_cue,
        when: |w, c| phase_done(w, c.elite, AttackPhase::Recovery).is_some(),
        camera: |w, c| duel_camera(w, c, -3.8, 0.6, 40.0),
    },
    Shot {
        file: "02_strike_slow_motion.png",
        round: Round::Blink,
        perception: false,
        stance: Stance::Stand,
        cue: |w, c| {
            let attacking = combat(w, c.elite).attack.is_some();
            let mut d = w.resource_mut::<TimeDilation>();
            if attacking && !d.is_active("preview") {
                d.hold("preview", 0.12, 0.05);
            }
        },
        when: |w, c| phase_done(w, c.elite, AttackPhase::Strike).is_some_and(|p| p >= 0.7),
        camera: |w, c| duel_camera(w, c, -3.6, 0.3, 40.0),
    },
    Shot {
        file: "03_parry_sparks.png",
        round: Round::Blink,
        perception: false,
        stance: Stance::Block,
        cue: no_cue,
        when: |w, _| since_record(w, |r| r.outcome == Outcome::Blocked).is_some_and(|s| s >= 0.08),
        camera: |w, c| duel_camera(w, c, -3.4, 1.0, 40.0),
    },
    Shot {
        file: "04_counter_kill_flash.png",
        round: Round::Focus,
        perception: true,
        stance: Stance::Brain,
        cue: no_cue,
        // The cut has drawn across and glows (world time: the world is slowed).
        when: |w, c| {
            combat(w, c.elite).defeated
                && since_record(w, |r| {
                    r.target == Some(c.elite)
                        && matches!(r.outcome, Outcome::Hit | Outcome::Counter)
                })
                .is_some_and(|s| s >= 0.05)
        },
        camera: |w, c| duel_camera(w, c, -3.6, -0.4, 42.0),
    },
    Shot {
        file: "05_slide_dust.png",
        round: Round::Blink,
        perception: false,
        stance: Stance::Brain,
        cue: count_slide_frames,
        // A third of a second into the runner's first slide.
        when: |w, _| w.get_resource::<SlideFrames>().is_some_and(|f| f.0 >= 20),
        camera: |w, c| {
            let p = pos(w, c.runner);
            let ahead = mover(w, c.runner)
                .velocity
                .as_dvec3()
                .with_y(0.0)
                .normalize_or(DVec3::X);
            let side = ahead.cross(DVec3::Y);
            (
                p - ahead * 0.6 + side * 3.0 + DVec3::Y * 0.15,
                p - ahead * 1.0 - side * 0.2 - DVec3::Y * 0.4,
                55.0,
            )
        },
    },
    Shot {
        file: "06_dash_afterimages.png",
        // Seen by the trained player, in slow motion: a dense line of ghosts.
        round: Round::Focus,
        perception: true,
        stance: Stance::Brain,
        cue: no_cue,
        when: |w, c| phase_done(w, c.elite, AttackPhase::Windup).is_some(),
        // From the side, looking back along the dash.
        camera: |w, c| {
            let e = pos(w, c.elite);
            let along = (pos(w, c.player) - e).with_y(0.0).normalize_or(DVec3::Z);
            let side = along.cross(DVec3::Y);
            (
                e - side * 5.0 - along * 0.4 + DVec3::Y * 0.6,
                e - along * 2.2 - DVec3::Y * 0.1,
                46.0,
            )
        },
    },
];

/// Marks a demo sword for the swing driver.
#[derive(Component)]
struct PreviewSword {
    owner: Entity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stance {
    /// The demo brain decides (dodges, counters).
    Brain,
    /// Holds a block toward the nearest fighter.
    Block,
    /// Stands and faces the nearest fighter.
    Stand,
}

/// The duel player whose stance is forced, and the stance.
#[derive(Resource, Default)]
struct ForceStance(Option<(Entity, Stance)>);

fn force_stance(
    force: Res<ForceStance>,
    others: Query<(Entity, &Transform), With<Fighter>>,
    mut q: Query<(&Transform, &mut MoveIntent)>,
) {
    let Some((me, stance)) = force.0 else {
        return;
    };
    let Ok((t, mut intent)) = q.get_mut(me) else {
        return;
    };
    let nearest = others
        .iter()
        .filter(|(e, _)| *e != me)
        .map(|(_, o)| o.translation)
        .min_by(|a, b| {
            a.distance(t.translation)
                .total_cmp(&b.distance(t.translation))
        });
    *intent = MoveIntent {
        look: nearest.and_then(|p| (p - t.translation).with_y(0.0).as_vec3().try_normalize()),
        block: stance == Stance::Block,
        ..Default::default()
    };
}

/// Sword poses (base position in rig space, blade direction) of a slash: cocked over the
/// right shoulder, through the front, down across the left hip.
const SLASH: [(Vec3, Vec3); 3] = [
    (Vec3::new(0.42, 0.32, 0.08), Vec3::new(0.35, 0.85, 0.4)),
    (Vec3::new(0.22, 0.12, -0.38), Vec3::new(0.12, 0.1, -1.0)),
    (Vec3::new(-0.18, -0.12, -0.3), Vec3::new(-0.85, -0.38, -0.3)),
];

fn slash_key(i: usize) -> (Vec3, Quat) {
    let (at, dir) = SLASH[i];
    (at, Quat::from_rotation_arc(Vec3::NEG_Z, dir.normalize()))
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn blend(a: (Vec3, Quat), b: (Vec3, Quat), t: f32) -> (Vec3, Quat) {
    (a.0.lerp(b.0, t), a.1.slerp(b.1, t))
}

/// Sweep the demo swords through a slash on the attack's own clock (PostUpdate, before
/// transforms propagate; the demo poses them in Update).
fn drive_swords(
    owners: Query<(&CombatState, &Fighter)>,
    mut swords: Query<(&PreviewSword, &mut Transform)>,
) {
    for (s, mut t) in &mut swords {
        let Ok((cs, f)) = owners.get(s.owner) else {
            continue;
        };
        let Some(a) = cs.attack else {
            continue;
        };
        let puppet = (t.translation.as_vec3(), t.rotation);
        let left = |total: f32| 1.0 - (a.secs_left / total.max(1e-4)).clamp(0.0, 1.0);
        let (at, rotation) = match a.phase {
            AttackPhase::DashIn => continue,
            AttackPhase::Windup => {
                let total = if a.kind == AttackKind::Dash {
                    f.dash_windup_secs
                } else {
                    f.windup_secs
                };
                blend(puppet, slash_key(0), smooth(left(total) * 1.4))
            }
            AttackPhase::Strike => {
                // Fast out of the cock, easing into the follow-through.
                let p = left(f.strike_secs);
                let e = 1.0 - (1.0 - p).powf(1.7);
                if e < 0.5 {
                    blend(slash_key(0), slash_key(1), e * 2.0)
                } else {
                    blend(slash_key(1), slash_key(2), e * 2.0 - 1.0)
                }
            }
            AttackPhase::Recovery => blend(slash_key(2), puppet, smooth(left(f.recovery_secs))),
        };
        t.translation = at.as_dvec3();
        t.rotation = rotation;
    }
}

/// Give every demo sword a [`Blade`] (owner, tip, trail color) unless the characters already
/// carry blades.
fn attach_blades(world: &mut World) {
    if world.query::<&Blade>().iter(world).next().is_some() {
        return;
    }
    let mut blades = world.query::<(&MeshRenderer, &ChildOf)>();
    let swords: Vec<Entity> = blades
        .iter(world)
        .filter(|(r, _)| matches!(&r.mesh, MeshRef::Shape(Shape::Cuboid { size }) if *size == BLADE_SIZE))
        .map(|(_, c)| c.parent())
        .collect();
    for sword in swords {
        let mut owner = sword;
        while let Some(p) = world.get::<ChildOf>(owner) {
            owner = p.parent();
        }
        let color = world
            .get::<Afterimages>(owner)
            .map_or([0.35, 0.8, 1.0], |a| a.color);
        let tip = world
            .spawn((
                Name::new("Blade tip"),
                Transform::from_xyz(0.0, 0.0, -1.1),
                ChildOf(sword),
            ))
            .id();
        world
            .entity_mut(sword)
            .insert((Blade { owner, tip, color }, PreviewSword { owner }));
    }
}

fn build_app(shot: &Shot) -> (App, Cast, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WIDTH;
        w.height = HEIGHT;
    }
    app.add_plugins(RenderPlugin)
        .add_plugins(WorldPlugin)
        .add_plugins(MovementPlugin)
        .init_resource::<ForceStance>()
        .add_systems(
            FixedUpdate,
            force_stance
                .after(MovementSet::Intent)
                .before(MovementSet::Act),
        )
        .add_systems(PostUpdate, drive_swords.before(UpdateSet::Propagate));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world
        .insert_resource(RenderDisabled("parked until the moment".into()));
    {
        let mut tp = app.world.resource_mut::<TimePerception>();
        tp.enabled = shot.perception;
        tp.cinematic.enabled = shot.perception;
    }
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.0015;
        fog.height_falloff = 0.05;
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.bloom_intensity = 0.06;
        s.vignette = 0.25;
    }

    // Late-afternoon sun from +X, a little toward -Z.
    let sun = Vec3::new(0.84, 0.32, -0.43).normalize();
    app.world.spawn((
        DirectionalLight {
            color: [1.0, 0.86, 0.7],
            illuminance: 70_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun.as_dvec3(), Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x33373d).with_roughness(0.9),
        ),
    ));
    for (i, (x, z, h)) in [
        (-40.0, 34.0, 26.0),
        (-18.0, 40.0, 18.0),
        (8.0, 44.0, 34.0),
        (34.0, 36.0, 22.0),
        (52.0, 10.0, 30.0),
        (-52.0, -8.0, 24.0),
        (-36.0, -42.0, 38.0),
        (6.0, -48.0, 28.0),
        (40.0, -36.0, 20.0),
    ]
    .into_iter()
    .enumerate()
    {
        let shade = [0x5d6470, 0x6f6a64, 0x545b66][i % 3];
        app.world.spawn((
            Transform::from_xyz(x, h * 0.5, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [10.0, h as f32, 10.0],
                },
                StandardMaterial::hex(shade).with_roughness(0.8),
            ),
        ));
    }
    let camera = app
        .world
        .spawn((
            Camera::perspective(50.0).with_exposure(Exposure::Auto {
                min_ev100: 8.0,
                max_ev100: 16.0,
                speed: 40.0,
                compensation: 0.3,
            }),
            Transform::from_xyz(0.0, 5.0, 20.0),
        ))
        .id();

    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    queue.apply(&mut app.world);
    attach_blades(&mut app.world);

    let named = |app: &mut App, name: &str| {
        let mut q = app.world.query::<(Entity, &Name)>();
        q.iter(&app.world)
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let runner = named(&mut app, "Runner");
    let director = named(&mut app, "Duel director");
    let (player, elite) = {
        let mut d = app.world.get_mut::<DuelDirector>(director).unwrap();
        d.round = shot.round;
        d.only = Some(shot.round);
        // Keep the duel running wherever the camera is, so both passes match.
        d.watch_radius = f32::INFINITY;
        (d.player.unwrap(), d.elite.unwrap())
    };
    if shot.stance != Stance::Brain {
        app.world.resource_mut::<ForceStance>().0 = Some((player, shot.stance));
    }
    (
        app,
        Cast {
            runner,
            player,
            elite,
        },
        camera,
    )
}

fn point_camera(world: &mut World, camera: Entity, (eye, target, fov): (DVec3, DVec3, f32)) {
    *world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(eye).looking_at(target, Vec3::Y);
    world.get_mut::<Camera>(camera).unwrap().projection = Projection::Perspective {
        fov_y_deg: fov,
        near: 0.05,
    };
}

/// Find the frame of the shot's moment (no drawing).
fn find_moment(shot: &Shot) -> Option<u64> {
    let (mut app, cast, _) = build_app(shot);
    for frame in 1..=(MAX_SECS / DT) as u64 {
        (shot.cue)(&mut app.world, &cast);
        app.update();
        if (shot.when)(&app.world, &cast) {
            return Some(frame);
        }
    }
    None
}

/// Replay up to the moment, drawing its last frames, and save it.
fn render_shot(shot: &Shot, at: u64, out: &Path) -> Result<PathBuf, String> {
    let (mut app, cast, camera) = build_app(shot);
    let mut frame = 0;
    while frame + WARMUP_FRAMES < at {
        (shot.cue)(&mut app.world, &cast);
        app.update();
        frame += 1;
    }
    app.world.remove_resource::<RenderDisabled>();
    while frame < at + 30 {
        (shot.cue)(&mut app.world, &cast);
        let view = (shot.camera)(&app.world, &cast);
        point_camera(&mut app.world, camera, view);
        app.update();
        frame += 1;
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            return Err(format!("cannot render: {}", d.0));
        }
        if frame >= at && (shot.when)(&app.world, &cast) {
            let view = (shot.camera)(&app.world, &cast);
            point_camera(&mut app.world, camera, view);
            let path = out.join(shot.file);
            capture_now(&mut app.world, &path)?;
            let time = app.world.resource::<Time>();
            let fx = app.world.resource::<engine_movement::Effects>();
            println!(
                "saved {} (t {:.2} s, scale {:.2}, {} particles, {} trails)",
                path.display(),
                time.elapsed_secs_f64(),
                time.scale(),
                fx.particles.len(),
                fx.trails.len(),
            );
            if std::env::var_os("EFFECTS_PREVIEW_DEBUG").is_some() {
                for t in &fx.trails {
                    let glows: Vec<String> = t
                        .samples
                        .iter()
                        .map(|s| format!("{:.1}@{:.3}", s.glow, fx.now - s.world_secs))
                        .collect();
                    println!("  trail {:?}: {}", t.blade, glows.join(" "));
                }
            }
            app.world.remove_resource::<Renderer>();
            return Ok(path);
        }
    }
    Err("the moment did not repeat".into())
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/effects_preview"));
    let only: Option<String> = std::env::args().nth(2);
    std::fs::create_dir_all(&out).expect("output directory");
    let mut saved = Vec::new();
    for shot in SHOTS {
        // Optional second argument: render only shots whose file names contain one of these
        // comma-separated parts.
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|part| shot.file.contains(part)))
        {
            continue;
        }
        let Some(at) = find_moment(shot) else {
            eprintln!("{}: never happened", shot.file);
            continue;
        };
        match render_shot(shot, at, &out) {
            Ok(p) => saved.push(p),
            Err(e) => eprintln!("{}: {e}", shot.file),
        }
    }
    if only.is_none() {
        contact_sheet(&saved, &out.join("00_contact_sheet.png"));
    }
}

/// All shots on one image, three across.
fn contact_sheet(paths: &[PathBuf], out: &Path) {
    if paths.is_empty() {
        return;
    }
    let (tw, th) = (WIDTH / 3, HEIGHT / 3);
    let cols = 3u32;
    let rows = (paths.len() as u32).div_ceil(cols);
    let gap = 6u32;
    let mut sheet = image::RgbaImage::from_pixel(
        cols * tw + (cols + 1) * gap,
        rows * th + (rows + 1) * gap,
        image::Rgba([18, 18, 22, 255]),
    );
    for (i, p) in paths.iter().enumerate() {
        let Ok(img) = image::open(p) else {
            continue;
        };
        let small = image::imageops::resize(
            &img.to_rgba8(),
            tw,
            th,
            image::imageops::FilterType::Triangle,
        );
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        image::imageops::overlay(
            &mut sheet,
            &small,
            (gap + c * (tw + gap)) as i64,
            (gap + r * (th + gap)) as i64,
        );
    }
    sheet.save(out).expect("contact sheet");
    println!("saved {}", out.display());
}
