//! Close-up renders of the procedural wildlife frozen in chosen states (grazing, rooting,
//! alert, walking, trotting, galloping, the back gathered and extended, lying, dead,
//! stalking, biting, charging, hopping, sitting up, pecking, flying), flipbook strips of them
//! on the move (gaits, an alarm, a wolf's hunt from stalk to bite, a boar's charge and slash,
//! a deer killed at a gallop, animals flinching from a blow), plus budget checks on the
//! generated meshes.
//! Rendering needs a GPU or lavapipe and passes with a note without one.
//!
//! By default (and in CI) only the checks run: the mesh budgets, and a few close-ups and
//! motion strips rendered small, each of which must show its animal. The full-size pictures
//! (every close-up and strip, as an animator looks at them) are ignored tests:
//! `cargo test -p engine_npc --test art_animals -- --ignored`, with `NPC_SCREENSHOT_DIR` set to
//! keep the PNGs and `NPC_STRIPS` (names, comma separated) to shoot only some strips. They
//! hold the same checks.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::art::animals;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};

const W: u32 = 960;
const H: u32 = 540;
/// The checks render this size (a quarter of the pixels).
const SMALL: (u32, u32) = (W / 2, H / 2);

/// One animal for a close-up: in `state` for `t` s at `speed` (m/s, straight ahead) when it
/// is shot. Walkers run there at the real rate; birds are held in the pose.
#[derive(Clone, Copy)]
struct Pose {
    species: Species,
    male: bool,
    young: bool,
    state: AnimalState,
    /// Seconds since the state began.
    t: f64,
    speed: f32,
    mode: LocomotionMode,
    /// Birds: mover phase and animal clock (pecks and flaps are timed by it).
    phase: f32,
    clock: f64,
    flap: f32,
    bite: bool,
    /// Climb rate (birds glide when sinking) and bank angle in flight.
    climb: f32,
    bank: f32,
}

impl Pose {
    fn new(species: Species, state: AnimalState) -> Self {
        Self {
            species,
            male: false,
            young: false,
            state,
            t: 5.0,
            speed: 0.0,
            mode: LocomotionMode::Idle,
            phase: 0.0,
            clock: 100.0,
            flap: 0.0,
            bite: false,
            climb: 0.0,
            bank: 0.0,
        }
    }

    fn moving(mut self, speed: f32, phase: f32, mode: LocomotionMode) -> Self {
        self.speed = speed;
        self.phase = phase;
        self.mode = mode;
        self
    }
}

fn app((width, height): (u32, u32)) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.05 });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    app.world.resource_mut::<WildlifeSettings>().enabled = false;
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.45, -0.55, -0.7), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55703a).with_roughness(0.95),
        ),
    ));
    app
}

/// Spawn `poses` 40 m apart along X, facing +X (birds in flight 0.6 m up); returns their
/// entities and positions.
fn spawn(app: &mut App, poses: &[Pose]) -> Vec<(Entity, DVec3)> {
    let mut out = Vec::new();
    for (i, p) in poses.iter().enumerate() {
        let up = if p.state == AnimalState::Flying {
            0.6
        } else {
            0.0
        };
        let at = DVec3::new(i as f64 * 40.0, up, 0.0);
        let mut animal = Animal::new(p.species);
        animal.male = p.male;
        animal.young = p.young;
        let e = app
            .world
            .spawn((
                Transform::from_translation(at).with_rotation(
                    Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)
                        * Quat::from_rotation_z(p.bank),
                ),
                animal,
                StandIn::default(),
                SelfPropelled,
            ))
            .id();
        out.push((e, at));
    }
    app.world.flush();
    out
}

/// Hold every animal in its pose.
fn hold(app: &mut App, animals: &[(Entity, DVec3)], poses: &[Pose]) {
    for ((e, _), p) in animals.iter().zip(poses) {
        let mut ent = app.world.entity_mut(*e);
        if let Some(mut c) = ent.get_mut::<NpcClock>() {
            c.now = p.clock;
        }
        if let Some(mut m) = ent.get_mut::<AnimalMind>() {
            m.state = p.state;
            m.since = p.clock - p.t;
            m.bite_ready = if p.bite { p.clock + 1.0 } else { 0.0 };
        }
        if let Some(mut l) = ent.get_mut::<LocomotionState>() {
            l.velocity = Vec3::X * p.speed + Vec3::Y * p.climb;
            l.mode = p.mode;
            l.grounded = p.state != AnimalState::Flying;
        }
        if let Some(mut m) = ent.get_mut::<MoverState>() {
            m.phase = p.phase;
        }
        if let Some(mut b) = ent.get_mut::<BirdFlight>() {
            b.flap = p.flap;
            b.velocity = Vec3::X * p.speed + Vec3::Y * p.climb;
        }
    }
}

fn shoot(
    app: &mut App,
    cam: Entity,
    dir: &std::path::Path,
    name: &str,
) -> Option<image::RgbaImage> {
    let mut q = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in q.iter_mut(&mut app.world) {
        c.active = e == cam;
    }
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(keep) = std::env::var("NPC_SCREENSHOT_DIR") {
        let _ = std::fs::create_dir_all(&keep);
        std::fs::copy(&path, std::path::Path::new(&keep).join(name)).expect("copy screenshot");
    }
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

fn camera(app: &mut App, eye: DVec3, target: DVec3, fov: f32) -> Entity {
    app.world
        .spawn((
            Camera {
                active: false,
                ..Camera::perspective(fov)
            },
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        ))
        .id()
}

/// Pixels that differ from the empty ground and sky around them: a crude "something is
/// there" count in the middle of the frame.
fn busy(img: &image::RgbaImage) -> usize {
    let (w, h) = img.dimensions();
    busy_in(img, (w / 4, h / 4), (w * 3 / 4, h * 3 / 4))
}

/// [`busy`] over the pixels from `from` up to (not including) `to`.
fn busy_in(img: &image::RgbaImage, from: (u32, u32), to: (u32, u32)) -> usize {
    let mut n = 0;
    for y in from.1..to.1 {
        for x in from.0..to.0 {
            let p = img.get_pixel(x, y);
            let q = img.get_pixel(x.saturating_sub(3), y);
            let d: i32 = (0..3).map(|i| (p[i] as i32 - q[i] as i32).abs()).sum();
            if d > 30 {
                n += 1;
            }
        }
    }
    n
}

/// One close-up: file name, pose, camera distance (m) and camera azimuth (degrees from the
/// animal's side toward its front).
type Shot<'a> = (&'a str, Pose, f64, f64);

/// Where the camera stands and looks for close-up `shot` of an animal standing at `at`,
/// facing +X.
fn framing(shot: &Shot, at: DVec3) -> (DVec3, DVec3) {
    let (name, pose, dist, azimuth) = *shot;
    // A boar's size: piglets are a lot smaller, old boars a size up.
    let boar = if pose.young {
        f64::from(animals::PIGLET_SCALE)
    } else if pose.male {
        f64::from(animals::BOAR_SCALE)
    } else {
        1.0
    };
    let (h, ahead) = match pose.species {
        Species::Deer if pose.male => (0.95, 0.4),
        Species::Deer => (0.85, 0.35),
        Species::Wolf => (0.45, 0.25),
        Species::Boar => (0.5 * boar, 0.2 * boar),
        Species::Rabbit => (0.1, 0.03),
        Species::Bird => (0.06, 0.0),
    };
    let h = match pose.state {
        AnimalState::Rest | AnimalState::Dead => h * 0.5,
        AnimalState::Graze | AnimalState::Feed => h * 0.75,
        _ => h,
    };
    // Portraits (names ending in "_head") look at the head, "_feet" at the forefeet.
    let (h, ahead) = if name.ends_with("_head") {
        match pose.species {
            Species::Deer if pose.state == AnimalState::Alert => (1.5, 1.0),
            Species::Deer => (1.22, 1.08),
            Species::Wolf => (0.72, 0.62),
            Species::Boar => (0.48 * boar, 0.7 * boar),
            Species::Rabbit => (0.16, 0.1),
            Species::Bird => (0.1, 0.06),
        }
    } else if name.ends_with("_feet") {
        match pose.species {
            Species::Deer => (0.12, 0.42),
            Species::Wolf => (0.06, 0.36),
            Species::Boar => (0.06 * boar, 0.36 * boar),
            _ => (0.02, 0.08),
        }
    } else {
        (h, ahead)
    };
    let az = azimuth.to_radians();
    // The animal faces +X: look at its middle, a little toward the head.
    let target = at + DVec3::new(ahead, h, 0.0);
    // Names ending in "_top" look down from high above.
    let rise = if name.ends_with("_top") { 2.5 } else { 0.22 };
    let eye = target + DVec3::new(dist * az.sin(), dist * rise, dist * az.cos());
    (eye, target)
}

/// The animal shows up: at least 400 busy pixels, however small the picture (a small one
/// only makes it harder).
fn check(name: &str, img: &image::RgbaImage) {
    eprintln!("{name}: {} busy px", busy(img));
    assert!(
        busy(img) > 400,
        "{name}: the animal should show up ({} px)",
        busy(img)
    );
}

/// What the file names of pictures rendered at `size` begin with: small ones are told apart
/// from the full-size pictures.
fn tag(size: (u32, u32)) -> &'static str {
    if size == (W, H) || size == (SW, SH) {
        ""
    } else {
        "small_"
    }
}

/// Close-ups of birds, held in their poses (flight is posed, not flown).
fn render_held(shots: &[Shot], size: (u32, u32)) {
    let mut app = app(size);
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    let list: Vec<Pose> = shots.iter().map(|p| p.1).collect();
    let animals = spawn(&mut app, &list);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("npc_art_animals");
    std::fs::create_dir_all(&dir).expect("dir");
    for (i, shot) in shots.iter().enumerate() {
        let (eye, target) = framing(shot, animals[i].1);
        let cam = camera(&mut app, eye, target, 28.0);
        hold(&mut app, &animals, &list);
        app.update();
        hold(&mut app, &animals, &list);
        let file = format!("{}animal_{}.png", tag(size), shot.0);
        let Some(img) = shoot(&mut app, cam, &dir, &file) else {
            return;
        };
        check(shot.0, &img);
    }
}

/// Moves a close-up animal as the wildlife code would, with the fallback mover at the real
/// rate: it stands until its state begins `pose.t` s before the shot, then keeps to that state
/// and goes straight on (+X) at the pose's speed, so the shot is a still from the run.
#[derive(Component, Clone, Copy)]
struct Steady {
    pose: Pose,
    /// When the shot is taken (animal clock, s).
    shot: f64,
}

/// A biting animal bites this long (s) before it is shot.
const BITE_LEAD: f64 = 0.12;

type SteadyItem<'a> = (
    &'a Steady,
    &'a NpcClock,
    &'a NpcMotor,
    &'a mut MoveIntent,
    &'a mut AnimalMind,
);

fn steady(mut q: Query<SteadyItem>) {
    for (s, clock, motor, mut intent, mut mind) in &mut q {
        let p = s.pose;
        let start = s.shot - p.t;
        let on = clock.now >= start;
        let state = if on { p.state } else { AnimalState::Wander };
        if mind.state != state {
            mind.state = state;
            mind.since = if on { start } else { clock.now };
        }
        let sprint = p.mode == LocomotionMode::Sprint;
        let crouch = p.mode == LocomotionMode::Crouch;
        let full = if sprint {
            motor.sprint_speed
        } else {
            motor.run_speed
        } * if crouch { 0.45 } else { 1.0 };
        let share = if on { p.speed / full.max(0.1) } else { 0.0 };
        intent.direction = Vec3::X * share;
        intent.sprint = sprint;
        intent.crouch = crouch;
        // As the wildlife code bites: ready again 1.1 s on.
        if p.bite && clock.now >= s.shot - BITE_LEAD && clock.now >= mind.bite_ready {
            mind.bite_ready = clock.now + 1.1;
        }
    }
}

/// Close-ups of walking animals: every one runs at the real rate (60 Hz) into its pose, all
/// reaching their moments together, and is shot where it then is, time standing still.
fn render(shots: &[Shot], size: (u32, u32)) {
    let mut app = app(size);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.0 });
    app.add_systems(FixedUpdate, steady.in_set(NpcSet::Decide));
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    // Far apart along X (a gallop covers some 50 m), facing +X.
    let n = shots.len() as f64;
    let animals: Vec<Entity> = shots
        .iter()
        .enumerate()
        .map(|(i, (_, p, _, _))| {
            let at = DVec3::new((i as f64 - 0.5 * n) * 120.0, 0.0, 0.0);
            let mut animal = Animal::new(p.species);
            animal.male = p.male;
            animal.young = p.young;
            app.world
                .spawn((
                    Transform::from_translation(at)
                        .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
                    animal,
                    StandIn::default(),
                ))
                .id()
        })
        .collect();
    app.world.flush();
    let step = |app: &mut App, secs: f64| {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs: secs });
        app.update();
    };
    step(&mut app, 1.0 / 60.0 + 1e-6);
    let now = app.world.get::<NpcClock>(animals[0]).map_or(0.0, |c| c.now);
    // Everyone stands a moment before the longest state begins.
    let shot = now + 0.5 + shots.iter().map(|s| s.1.t).fold(0.0, f64::max);
    for (e, (_, pose, _, _)) in animals.iter().zip(shots) {
        app.world
            .entity_mut(*e)
            .insert(Steady { pose: *pose, shot });
    }
    live_until(&mut app, animals[0], shot, size == (W, H));
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("npc_art_animals");
    std::fs::create_dir_all(&dir).expect("dir");
    for (e, s) in animals.iter().zip(shots) {
        let at = app.world.get::<Transform>(*e).expect("animal").translation;
        let (eye, target) = framing(s, DVec3::new(at.x, 0.0, at.z));
        let cam = camera(&mut app, eye, target, 28.0);
        let file = format!("{}animal_{}.png", tag(size), s.0);
        let Some(img) = shoot(&mut app, cam, &dir, &file) else {
            return;
        };
        check(s.0, &img);
    }
}

/// The deer close-ups.
fn deer_shots() -> Vec<Shot<'static>> {
    let p = |state| Pose::new(Species::Deer, state);
    let stag = |state| Pose {
        male: true,
        ..Pose::new(Species::Deer, state)
    };
    vec![
        ("deer_stag_alert", stag(AnimalState::Alert), 5.0, 35.0),
        ("deer_stag_profile", stag(AnimalState::Wander), 5.0, 0.0),
        ("deer_hind_profile", p(AnimalState::Wander), 4.5, 0.0),
        ("deer_hind_head", p(AnimalState::Alert), 1.6, 50.0),
        ("deer_side_head", p(AnimalState::Wander), 1.3, 0.0),
        ("deer_stag_head", stag(AnimalState::Wander), 2.4, 35.0),
        ("deer_hind_feet", p(AnimalState::Wander), 1.0, 20.0),
        ("deer_hind_graze", p(AnimalState::Graze), 4.5, 30.0),
        (
            "deer_hind_walk",
            p(AnimalState::Wander).moving(1.3, 1.3, LocomotionMode::Walk),
            4.5,
            15.0,
        ),
        (
            "deer_stag_trot",
            stag(AnimalState::Wander).moving(3.2, 0.4, LocomotionMode::Run),
            5.0,
            15.0,
        ),
        (
            "deer_hind_gallop",
            Pose {
                t: 5.18,
                ..p(AnimalState::Flee).moving(10.0, 0.0, LocomotionMode::Sprint)
            },
            5.0,
            10.0,
        ),
        // The spine through the gallop: gathered (croup tucked, back arched, hind feet
        // reaching under) and extended (croup raised, back hollowed, legs stretched out), at
        // those moments of a run at the real rate (the gallop's moment is when its state began).
        (
            "deer_hind_gathered",
            Pose {
                t: 5.09,
                ..p(AnimalState::Flee).moving(10.0, 0.0, LocomotionMode::Sprint)
            },
            4.2,
            0.0,
        ),
        (
            "deer_hind_extended",
            Pose {
                t: 5.27,
                ..p(AnimalState::Flee).moving(10.0, 0.0, LocomotionMode::Sprint)
            },
            4.2,
            0.0,
        ),
        ("deer_stag_rest", stag(AnimalState::Rest), 4.5, 40.0),
        ("deer_hind_dead", p(AnimalState::Dead), 4.5, 60.0),
        ("deer_hind_rear", p(AnimalState::Alert), 3.5, -140.0),
    ]
}

#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn deer_closeups() {
    render(&deer_shots(), (W, H));
}

/// The wolf close-ups.
fn wolf_shots() -> Vec<Shot<'static>> {
    let w = |state| Pose::new(Species::Wolf, state);
    vec![
        ("wolf_profile", w(AnimalState::Wander), 3.4, 0.0),
        ("wolf_alert", w(AnimalState::Alert), 3.4, 35.0),
        ("wolf_head", w(AnimalState::Wander), 1.1, 45.0),
        (
            "wolf_trot",
            w(AnimalState::Wander).moving(3.0, 0.6, LocomotionMode::Run),
            3.6,
            15.0,
        ),
        (
            "wolf_gallop",
            w(AnimalState::Chase).moving(11.0, 0.0, LocomotionMode::Sprint),
            3.8,
            10.0,
        ),
        (
            "wolf_gathered",
            Pose {
                t: 5.16,
                ..w(AnimalState::Chase).moving(11.0, 0.0, LocomotionMode::Sprint)
            },
            3.2,
            0.0,
        ),
        (
            "wolf_extended",
            Pose {
                t: 5.04,
                ..w(AnimalState::Chase).moving(11.0, 0.0, LocomotionMode::Sprint)
            },
            3.2,
            0.0,
        ),
        (
            "wolf_stalk",
            w(AnimalState::Stalk).moving(0.8, 2.0, LocomotionMode::Crouch),
            3.4,
            30.0,
        ),
        (
            "wolf_bite",
            Pose {
                bite: true,
                ..w(AnimalState::Chase)
            },
            1.8,
            50.0,
        ),
        ("wolf_feed", w(AnimalState::Feed), 3.2, 40.0),
        ("wolf_rest", w(AnimalState::Rest), 3.2, 40.0),
        ("wolf_dead", w(AnimalState::Dead), 3.2, 60.0),
        ("wolf_rear", w(AnimalState::Wander), 3.0, -140.0),
        ("wolf_feet", w(AnimalState::Wander), 0.8, 25.0),
    ]
}

#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn wolf_closeups() {
    render(&wolf_shots(), (W, H));
}

/// The boar close-ups.
fn boar_shots() -> Vec<Shot<'static>> {
    let sow = |state| Pose::new(Species::Boar, state);
    let tusker = |state| Pose {
        male: true,
        ..Pose::new(Species::Boar, state)
    };
    let piglet = |state| Pose {
        young: true,
        ..Pose::new(Species::Boar, state)
    };
    vec![
        ("boar_tusker_profile", tusker(AnimalState::Wander), 3.4, 0.0),
        ("boar_sow_profile", sow(AnimalState::Wander), 3.2, 0.0),
        ("boar_tusker_head", tusker(AnimalState::Wander), 1.4, 40.0),
        ("boar_sow_head", sow(AnimalState::Alert), 1.2, 55.0),
        ("boar_piglet", piglet(AnimalState::Wander), 1.5, 25.0),
        ("boar_piglet_head", piglet(AnimalState::Wander), 0.6, 45.0),
        (
            "boar_piglet_gallop",
            piglet(AnimalState::Flee).moving(4.2, 0.0, LocomotionMode::Sprint),
            1.6,
            10.0,
        ),
        (
            "boar_sow_rooting",
            Pose {
                t: 3.0,
                ..sow(AnimalState::Graze)
            },
            3.0,
            35.0,
        ),
        ("boar_tusker_alert", tusker(AnimalState::Alert), 3.4, 35.0),
        (
            "boar_tusker_trot",
            tusker(AnimalState::Wander).moving(3.0, 0.6, LocomotionMode::Run),
            3.6,
            15.0,
        ),
        (
            "boar_sow_gallop",
            sow(AnimalState::Flee).moving(9.0, 0.0, LocomotionMode::Sprint),
            3.6,
            10.0,
        ),
        (
            "boar_tusker_charge",
            tusker(AnimalState::Charge).moving(9.5, 1.0, LocomotionMode::Sprint),
            3.4,
            30.0,
        ),
        ("boar_sow_rest", sow(AnimalState::Rest), 3.2, 40.0),
        ("boar_sow_dead", sow(AnimalState::Dead), 3.2, 60.0),
        ("boar_tusker_rear", tusker(AnimalState::Wander), 3.0, -140.0),
        ("boar_sow_feet", sow(AnimalState::Wander), 0.8, 25.0),
    ]
}

#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn boar_closeups() {
    render(&boar_shots(), (W, H));
}

/// The rabbit close-ups.
fn rabbit_shots() -> Vec<Shot<'static>> {
    let r = |state| Pose::new(Species::Rabbit, state);
    vec![
        ("rabbit_profile", r(AnimalState::Wander), 1.1, 0.0),
        ("rabbit_graze", r(AnimalState::Graze), 1.0, 35.0),
        ("rabbit_head", r(AnimalState::Wander), 0.45, 40.0),
        ("rabbit_sit_up", r(AnimalState::Alert), 1.0, 30.0),
        (
            "rabbit_hop",
            r(AnimalState::Wander).moving(1.2, 2.2, LocomotionMode::Walk),
            1.1,
            10.0,
        ),
        (
            "rabbit_run",
            r(AnimalState::Flee).moving(6.0, 0.0, LocomotionMode::Sprint),
            1.2,
            -30.0,
        ),
        ("rabbit_rest", r(AnimalState::Rest), 1.0, 40.0),
        ("rabbit_rear", r(AnimalState::Wander), 0.9, -140.0),
        ("rabbit_dead", r(AnimalState::Dead), 1.0, 60.0),
    ]
}

#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn rabbit_closeups() {
    render(&rabbit_shots(), (W, H));
}

/// The starling close-ups.
fn starling_shots() -> Vec<Shot<'static>> {
    let b = |state| Pose::new(Species::Bird, state);
    let fly = |flap: f32| Pose {
        flap,
        t: 3.0,
        ..b(AnimalState::Flying).moving(7.0, 0.0, LocomotionMode::Air)
    };
    vec![
        ("bird_profile", b(AnimalState::Perched), 0.6, 0.0),
        ("bird_head", b(AnimalState::Perched), 0.3, 40.0),
        (
            "bird_peck",
            Pose {
                clock: 100.085,
                ..b(AnimalState::Perched)
            },
            0.6,
            30.0,
        ),
        (
            "bird_hop",
            Pose {
                climb: 1.0,
                ..b(AnimalState::Perched)
            },
            0.6,
            20.0,
        ),
        ("bird_rear", b(AnimalState::Perched), 0.55, -140.0),
        (
            "bird_takeoff",
            Pose {
                t: 0.1,
                climb: 4.0,
                ..fly(0.6)
            },
            0.9,
            25.0,
        ),
        ("bird_fly_up", fly(0.0), 0.9, 15.0),
        ("bird_fly_raised", fly(1.57), 0.9, 15.0),
        ("bird_fly_top", fly(3.6), 0.5, 0.0),
        ("bird_perched_top", b(AnimalState::Perched), 0.25, 0.0),
        ("bird_fly_down", fly(3.6), 0.9, 15.0),
        (
            "bird_glide",
            Pose {
                climb: -1.0,
                ..fly(4.7)
            },
            0.9,
            40.0,
        ),
        (
            "bird_bank",
            Pose {
                bank: 0.6,
                ..fly(3.6)
            },
            0.9,
            -20.0,
        ),
        ("bird_dead", b(AnimalState::Dead), 0.6, 60.0),
    ]
}

#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn starling_closeups() {
    render_held(&starling_shots(), (W, H));
}

/// A few close-ups rendered small, of every walking species standing and on the run (the
/// starling shows up in its flight strip): each shows its animal.
#[test]
fn a_few_closeups_show_their_animals() {
    let pick = |shots: Vec<Shot<'static>>, names: &[&str]| -> Vec<Shot<'static>> {
        let out: Vec<_> = shots.into_iter().filter(|s| names.contains(&s.0)).collect();
        assert_eq!(out.len(), names.len(), "no close-up named in {names:?}");
        out
    };
    let mut walkers = pick(deer_shots(), &["deer_stag_alert", "deer_hind_gallop"]);
    walkers.extend(pick(wolf_shots(), &["wolf_bite"]));
    walkers.extend(pick(
        boar_shots(),
        &["boar_tusker_charge", "boar_piglet_gallop"],
    ));
    walkers.extend(pick(rabbit_shots(), &["rabbit_run"]));
    render(&walkers, SMALL);
}

#[test]
fn meshes_fit_their_budgets() {
    let mut total = 0.0;
    for species in Species::ALL {
        let (meshes, secs) = animals::build_all(species);
        total += secs;
        for (male, young) in [(false, false), (true, false), (false, true)] {
            let tris = animals::triangles_of(species, male, young, &meshes);
            let parts = animals::piece_names(species, male, young).len();
            eprintln!(
                "{species:?} male={male} young={young}: {tris} triangles in {parts} parts ({secs:.2} s)"
            );
            assert!(tris <= 42_000, "{species:?}: {tris} triangles");
            assert!(parts <= 30, "{species:?}: {parts} parts");
        }
    }
    eprintln!("all animals generated in {total:.2} s");
}

// ======================================================================= motion strips

/// Motion strips (pixels): one row of eight frames per strip.
const SW: u32 = 2400;
const SH: u32 = 230;
const FRAMES: usize = 8;

/// What a scripted animal is asked to do at one moment of its course.
#[derive(Clone, Copy)]
struct Cue {
    /// Wished velocity (world x, z) as a share of the run speed (1 = run).
    go: Vec2,
    sprint: bool,
    crouch: bool,
    /// Face this way (world x, z) instead of the way it goes.
    look: Option<Vec2>,
    state: AnimalState,
    /// A threat or noise (world x, z from where the course starts) and how dangerous it is.
    threat: Option<(Vec2, f32)>,
    /// The noise is heard at this moment.
    heard: bool,
    /// Wolves: the animal it hunts is this far (m) ahead of it.
    prey: Option<f32>,
    /// Wolves: it bites (once; the bite is ready again a second later).
    bite: bool,
    /// Hurt (killed) by something here (world x, z from where the course starts).
    shot: Option<Vec2>,
    /// Hurt (not killed) by something here, the first moment it is set.
    hit: Option<Vec2>,
}

impl Cue {
    fn new(state: AnimalState, x: f32, z: f32) -> Self {
        Self {
            go: Vec2::new(x, z),
            sprint: false,
            crouch: false,
            look: None,
            state,
            threat: None,
            heard: false,
            prey: None,
            bite: false,
            shot: None,
            hit: None,
        }
    }
}

/// A course: what the animal is asked to do `t` s after it starts.
type Course = fn(f32) -> Cue;

/// Drives an animal through a course with the fallback mover (no AI). The course starts
/// `delay` s into the animal's life; before that it stands as the course begins.
#[derive(Component, Clone, Copy)]
struct Drive {
    course: Course,
    origin: DVec3,
    delay: f64,
    /// Stands in for the animal a wolf hunts.
    prey: Entity,
}

/// What a scripted wolf hunts: a bare point the course keeps ahead of it.
#[derive(Component)]
struct Quarry;

type DriveItem<'a> = (
    &'a Drive,
    &'a NpcClock,
    &'a Transform,
    &'a mut MoveIntent,
    &'a mut AnimalMind,
);

/// Where a probe animal was at each step of its course (course time, position).
#[derive(Component, Default)]
struct Track(Vec<(f64, DVec3)>);

fn drive(
    mut q: Query<DriveItem>,
    mut quarry: Query<&mut Transform, (With<Quarry>, Without<Drive>)>,
) {
    for (d, clock, tf, mut intent, mut mind) in &mut q {
        let t = clock.now - d.delay;
        let mut cue = (d.course)(t.max(0.0) as f32);
        if t < 0.0 {
            cue.go = Vec2::ZERO;
            cue.look = None;
        }
        intent.direction = Vec3::new(cue.go.x, 0.0, cue.go.y);
        intent.sprint = cue.sprint;
        intent.crouch = cue.crouch;
        intent.look = cue.look.map(|l| Vec3::new(l.x, 0.0, l.y).normalize());
        if mind.state != cue.state {
            mind.state = cue.state;
            mind.since = clock.now;
        }
        mind.heard = None;
        if let Some((p, danger)) = cue.threat {
            let at = d.origin + DVec3::new(f64::from(p.x), 0.0, f64::from(p.y));
            mind.threat_pos = Some(at);
            mind.danger = danger;
            if danger >= 0.5 {
                mind.last_danger = clock.now;
            }
            if cue.heard {
                mind.heard = Some((at, 1.0));
            }
        }
        mind.hurt_from = cue
            .shot
            .map(|p| d.origin + DVec3::new(f64::from(p.x), 0.0, f64::from(p.y)));
        if let Some(p) = cue.hit
            && mind.wounded.is_none()
        {
            let at = d.origin + DVec3::new(f64::from(p.x), 0.0, f64::from(p.y));
            mind.wounded = Some((clock.now, at, None));
        }
        mind.prey = cue.prey.map(|ahead| {
            if let Ok(mut q) = quarry.get_mut(d.prey) {
                q.translation = tf.translation + (tf.rotation * Vec3::NEG_Z * ahead).as_dvec3();
            }
            d.prey
        });
        // As the wildlife code bites: ready again 0.9..1.3 s on.
        if cue.bite && clock.now >= mind.bite_ready {
            mind.bite_ready = clock.now + 1.1;
        }
    }
}

/// A starling's scripted flight: it pecks, takes off, climbs, cruises, banks round to the
/// left, glides down and lands. The transform, velocity and wing beat move as the wildlife
/// code moves them in flight.
#[derive(Component, Clone, Copy)]
struct Flight {
    origin: DVec3,
    delay: f64,
    /// Heading (rad, 0 along +X, turning toward −Z).
    heading: f32,
    landed: bool,
}

type FlightItem<'a> = (
    &'a mut Flight,
    &'a NpcClock,
    &'a mut Transform,
    &'a mut BirdFlight,
    &'a mut AnimalMind,
    &'a mut LocomotionState,
);

fn fly_course(mut q: Query<FlightItem>) {
    let s = |a: f32, b: f32, x: f32| {
        let u = ((x - a) / (b - a)).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    };
    for (mut f, clock, mut tf, mut b, mut mind, mut loco) in &mut q {
        let (t, dt) = ((clock.now - f.delay) as f32, clock.dt);
        if dt <= 0.0 {
            continue;
        }
        let set = |mind: &mut AnimalMind, state: AnimalState| {
            if mind.state != state {
                mind.state = state;
                mind.since = clock.now;
            }
        };
        let pos = tf.translation;
        let h = (pos.y - f.origin.y) as f32;
        if t < 0.6 || f.landed {
            set(&mut mind, AnimalState::Perched);
            b.velocity = Vec3::ZERO;
            b.flap = 0.0;
            b.landing = None;
            loco.velocity = Vec3::ZERO;
            loco.mode = LocomotionMode::Idle;
            loco.grounded = true;
            tf.rotation = Quat::from_rotation_y(f.heading - std::f32::consts::FRAC_PI_2);
            continue;
        }
        if mind.state != AnimalState::Flying {
            set(&mut mind, AnimalState::Flying);
            b.airborne_since = clock.now;
            b.landing = None;
        }
        let u = t - 0.6;
        if (2.2..3.4).contains(&u) {
            f.heading += 1.4 * dt;
        }
        let fwd = Vec3::new(f.heading.cos(), 0.0, -f.heading.sin());
        let mut v = if u < 1.2 {
            fwd * (2.0 + 5.0 * s(0.0, 1.0, u)) + Vec3::Y * (6.0 - 4.5 * s(0.0, 1.2, u))
        } else if u < 3.4 {
            fwd * 7.0 + Vec3::Y * (0.4 * (u * 2.0).sin())
        } else {
            let fall = (h * 1.3).clamp(0.5, 2.2);
            fwd * (1.2 + 5.8 * s(0.0, 1.2, h)) - Vec3::Y * fall
        };
        if u >= 3.4 {
            let reach = Vec3::new(v.x, 0.0, v.z) * (h / (-v.y).max(0.1));
            b.landing = Some(DVec3::new(
                pos.x + f64::from(reach.x),
                f.origin.y,
                pos.z + f64::from(reach.z),
            ));
            if h <= 0.005 {
                f.landed = true;
                tf.translation.y = f.origin.y;
                set(&mut mind, AnimalState::Perched);
                b.velocity = Vec3::ZERO;
                loco.velocity = Vec3::ZERO;
                loco.grounded = true;
                continue;
            }
        }
        if h + v.y * dt < 0.0 {
            v.y = -h / dt;
        }
        let acc = (v - b.velocity) / dt;
        b.velocity = v;
        tf.translation = pos + (v * dt).as_dvec3();
        if v.length() > 0.5 {
            // As the wildlife code turns a flying bird.
            let yaw = (-v.x).atan2(-v.z);
            let pitch = (v.y / v.length()).asin() * 0.6;
            let left = Vec3::new(v.z, 0.0, -v.x).normalize_or_zero();
            let bank = (-acc.dot(left) * 0.06).clamp(-0.8, 0.8);
            tf.rotation = Quat::from_rotation_y(yaw)
                * Quat::from_rotation_x(pitch)
                * Quat::from_rotation_z(bank);
        }
        let rate = if v.y > 0.5 || u < 1.5 { 18.0 } else { 9.0 };
        b.flap = (b.flap + rate * dt) % std::f32::consts::TAU;
        loco.velocity = v;
        loco.mode = LocomotionMode::Air;
        loco.grounded = false;
    }
}

type RecordItem<'a> = (
    &'a mut Track,
    &'a NpcClock,
    &'a Transform,
    Option<&'a Drive>,
    Option<&'a Flight>,
);

fn record(mut q: Query<RecordItem>) {
    for (mut track, clock, t, drive, flight) in &mut q {
        let delay = drive.map(|d| d.delay).or(flight.map(|f| f.delay));
        track
            .0
            .push((clock.now - delay.unwrap_or(0.0), t.translation));
    }
}

/// One strip: an animal on a course, shot from its right side at eight moments (s after it
/// starts). Each frame is its own animal living through the same course, so the frames are
/// side by side in one picture; `track` centers each frame on its animal, otherwise the frames
/// hold still on the ground and the animal moves through them.
struct Strip {
    name: &'static str,
    species: Species,
    male: bool,
    young: bool,
    /// The course, or `None` for the starling's scripted flight.
    course: Option<Course>,
    times: [f32; FRAMES],
    track: bool,
}

fn every(from: f32, step: f32) -> [f32; FRAMES] {
    std::array::from_fn(|i| from + step * i as f32)
}

// Courses: the animals start facing +X.
fn deer_walk(_: f32) -> Cue {
    Cue::new(AnimalState::Wander, 0.2, 0.0)
}
fn deer_trot(_: f32) -> Cue {
    Cue::new(AnimalState::Wander, 0.5, 0.0)
}
fn deer_gallop(_: f32) -> Cue {
    Cue {
        sprint: true,
        ..Cue::new(AnimalState::Flee, 1.0, 0.0)
    }
}
/// Grazing; a noise behind it to the left: head up and round to it, frozen, then it bolts.
fn deer_alarm(t: f32) -> Cue {
    let at = Vec2::new(-3.0, -9.0);
    if t < 0.8 {
        Cue::new(AnimalState::Graze, 0.0, 0.0)
    } else if t < 2.4 {
        Cue {
            threat: Some((at, 0.7)),
            heard: t < 0.84,
            ..Cue::new(AnimalState::Alert, 0.0, 0.0)
        }
    } else {
        Cue {
            sprint: true,
            threat: Some((at, 1.2)),
            ..Cue::new(AnimalState::Flee, 0.95, 0.3)
        }
    }
}
/// Trotting, then stopping short.
fn deer_stop(t: f32) -> Cue {
    if t < 1.2 {
        Cue::new(AnimalState::Wander, 0.5, 0.0)
    } else {
        Cue::new(AnimalState::Graze, 0.0, 0.0)
    }
}
/// Shot dead at a gallop from off its right side: it drops, skids to a stop, falls onto its
/// left side and settles.
fn deer_death(t: f32) -> Cue {
    if t < 1.0 {
        deer_gallop(t)
    } else {
        Cue {
            shot: Some(Vec2::new(14.0, 20.0)),
            ..Cue::new(AnimalState::Dead, 0.0, 0.0)
        }
    }
}
/// Walking round a tight bend to the left.
fn deer_bend(t: f32) -> Cue {
    let a = 0.9 * t;
    Cue::new(AnimalState::Wander, 0.2 * a.cos(), -0.2 * a.sin())
}
fn wolf_trot(_: f32) -> Cue {
    Cue::new(AnimalState::Wander, 0.43, 0.0)
}
/// From a trot up through the lope into the gallop.
fn wolf_chase(t: f32) -> Cue {
    if t < 0.6 {
        Cue::new(AnimalState::Wander, 0.43, 0.0)
    } else if t < 1.6 {
        Cue::new(AnimalState::Chase, 0.86, 0.0)
    } else {
        Cue {
            sprint: true,
            ..Cue::new(AnimalState::Chase, 1.0, 0.0)
        }
    }
}
fn wolf_stalk(_: f32) -> Cue {
    Cue {
        crouch: true,
        ..Cue::new(AnimalState::Stalk, 0.4, 0.0)
    }
}
/// Stalking its prey 10 m ahead, then the rush: it sprints in and bites as the wildlife code
/// bites, within 1.6 m.
fn wolf_hunt(t: f32) -> Cue {
    if t < 1.2 {
        Cue {
            crouch: true,
            prey: Some(10.0),
            ..Cue::new(AnimalState::Stalk, 0.3, 0.0)
        }
    } else {
        let d = (10.0 - 6.0 * (t - 1.2)).max(1.4);
        Cue {
            sprint: true,
            prey: Some(d),
            bite: d < 1.6,
            ..Cue::new(AnimalState::Chase, 1.0, 0.0)
        }
    }
}
/// Walking at `walk` (a share of its run), struck from its right at 1 s and not killed: it
/// flinches away from the blow and staggers, ears pinned, and bolts off away from it.
fn hurt(t: f32, walk: f32) -> Cue {
    hurt_from(t, walk, 1.0)
}

/// Hurt from the side the strips are shot from (`side` 1) or from the far side (-1).
fn hurt_from(t: f32, walk: f32, side: f32) -> Cue {
    let at = Vec2::new(1.2, 6.0 * side);
    if t < 1.0 {
        Cue::new(AnimalState::Wander, walk, 0.0)
    } else if t < 1.22 {
        Cue {
            hit: Some(at),
            ..Cue::new(AnimalState::Wander, 0.0, 0.0)
        }
    } else {
        Cue {
            sprint: true,
            hit: Some(at),
            threat: Some((at, 1.2)),
            ..Cue::new(AnimalState::Flee, 0.8, -0.6 * side)
        }
    }
}
fn deer_hurt(t: f32) -> Cue {
    hurt(t, 0.2)
}
fn deer_hurt_far(t: f32) -> Cue {
    hurt_from(t, 0.2, -1.0)
}
/// A boar hurt by something it can't find: it flinches, then stands and scents for it.
fn boar_search(t: f32) -> Cue {
    let at = Vec2::new(1.2, 6.0);
    if t < 1.0 {
        Cue::new(AnimalState::Wander, 0.2, 0.0)
    } else {
        Cue {
            hit: (t < 1.22).then_some(at),
            ..Cue::new(AnimalState::Alert, 0.0, 0.0)
        }
    }
}
fn wolf_hurt(t: f32) -> Cue {
    hurt(t, 0.2)
}
fn boar_hurt(t: f32) -> Cue {
    hurt(t, 0.2)
}
fn boar_walk(_: f32) -> Cue {
    Cue::new(AnimalState::Wander, 0.2, 0.0)
}
fn boar_trot(_: f32) -> Cue {
    Cue::new(AnimalState::Wander, 0.55, 0.0)
}
fn boar_gallop(_: f32) -> Cue {
    Cue {
        sprint: true,
        ..Cue::new(AnimalState::Flee, 1.0, 0.0)
    }
}
/// Rooting along slowly.
fn boar_rooting(_: f32) -> Cue {
    Cue::new(AnimalState::Graze, 0.05, 0.0)
}
/// When the charging boar reaches the threat (s).
const SLASH: f32 = 2.2;
/// Standing its ground champing at a threat 10 m ahead, then the charge: head down, it runs
/// at it, slashes it with its tusks as it reaches it and peels away.
fn boar_charge(t: f32) -> Cue {
    let at = Vec2::new(10.0, 0.0);
    if t < 1.0 {
        Cue {
            threat: Some((at, 0.9)),
            ..Cue::new(AnimalState::Alert, 0.0, 0.0)
        }
    } else if t < SLASH + 0.4 {
        Cue {
            sprint: true,
            threat: Some((at, 1.2)),
            bite: t >= SLASH,
            ..Cue::new(AnimalState::Charge, 1.0, 0.0)
        }
    } else {
        Cue {
            sprint: true,
            threat: Some((at, 1.2)),
            ..Cue::new(AnimalState::Flee, 0.7, -0.7)
        }
    }
}
fn rabbit_hop(_: f32) -> Cue {
    Cue::new(AnimalState::Wander, 0.3, 0.0)
}
fn rabbit_run(_: f32) -> Cue {
    Cue {
        sprint: true,
        ..Cue::new(AnimalState::Flee, 1.0, 0.0)
    }
}

fn strip(name: &'static str, species: Species, course: Course, times: [f32; FRAMES]) -> Strip {
    Strip {
        name,
        species,
        male: false,
        young: false,
        course: Some(course),
        times,
        track: true,
    }
}

/// The starling's flight.
fn flight(name: &'static str, times: [f32; FRAMES]) -> Strip {
    Strip {
        course: None,
        ..strip(name, Species::Bird, deer_walk, times)
    }
}

fn held(mut s: Strip) -> Strip {
    s.track = false;
    s
}

/// Spawn an animal of `s` on its course from `origin`, the course starting `delay` s into
/// its life.
fn course_animal(app: &mut App, s: &Strip, origin: DVec3, delay: f64) -> Entity {
    let mut animal = Animal::new(s.species);
    animal.male = s.male;
    animal.young = s.young;
    let prey = app
        .world
        .spawn((Transform::from_translation(origin), Quarry))
        .id();
    let mut e = app.world.spawn((
        Transform::from_translation(origin)
            .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
        animal,
        StandIn::default(),
    ));
    match s.course {
        Some(course) => {
            e.insert(Drive {
                course,
                origin,
                delay,
                prey,
            });
        }
        None => {
            e.insert(Flight {
                origin,
                delay,
                heading: 0.0,
                landed: false,
            });
        }
    }
    e.id()
}

/// Step the fixed clock on to `until` (s of the animals' lives), several steps a frame,
/// looking away (cheap frames), or, unless `draw`, without drawing at all: the animals live,
/// move and are posed as in a drawn frame (the checks' small pictures; each shot's camera
/// starts with a cut, so they miss nothing but the light's exposure settling, which the
/// full-size pictures keep as before).
fn live_until(app: &mut App, clock_of: Entity, until: f64, draw: bool) {
    let render = (!draw).then(|| {
        let render = app
            .world
            .resource_mut::<Schedules>()
            .remove(Render)
            .expect("render schedule");
        app.world
            .resource_mut::<Schedules>()
            .insert(Schedule::new(Render));
        render
    });
    let step = 1.0 / 60.0;
    loop {
        let now = app.world.get::<NpcClock>(clock_of).map_or(0.0, |c| c.now);
        let left = ((until - now) / step).round() as i64;
        if left <= 0 {
            break;
        }
        let n = left.min(6) as f64;
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: n * step + 1e-6,
            });
        app.update();
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.0 });
    if let Some(render) = render {
        app.world.resource_mut::<Schedules>().insert(render);
    }
}

/// Flipbook strips of the wildlife on the move: each animal runs a course with the fallback
/// mover (no AI; the course sets what the wildlife code would: state, threat, prey, bite,
/// killing blow) and is shot at eight moments side by side, so gaits, footfalls, starts and
/// stops, alarms, the hunt, the fall and flight read frame by frame as an animator reads
/// them. The PNGs are `animal_motion_<strip>.png`.
#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn wildlife_motion_strips() {
    // `NPC_STRIPS=deer-walk,rabbit-hop` shoots only those.
    let only = std::env::var("NPC_STRIPS").ok();
    let names: Option<Vec<&str>> = only.as_deref().map(|o| o.split(',').collect());
    motion_strips(names.as_deref(), (SW, SH));
}

/// A few motion strips rendered small (gaits, the hunt, a charge, a flinch, a hop and
/// flight): every frame shows its animal.
#[test]
fn a_few_motion_strips_show_their_animals() {
    motion_strips(
        Some(&[
            "deer-gallop",
            "wolf-hunt",
            "boar-charge",
            "deer-hurt",
            "rabbit-run",
            "starling-flight",
        ]),
        (SW / 2, SH / 2),
    );
}

/// Shoot the strips named in `only` (all of them if `None`), `size` pixels each.
fn motion_strips(only: Option<&[&str]>, size: (u32, u32)) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        (w.width, w.height) = size;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.0 });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    app.world.resource_mut::<WildlifeSettings>().enabled = false;
    app.add_systems(FixedUpdate, (drive, fly_course).in_set(NpcSet::Decide));
    app.add_systems(FixedUpdate, record.in_set(NpcSet::Show));
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.45, -0.55, -0.7), Vec3::Y),
    ));
    let away = camera(
        &mut app,
        DVec3::new(0.0, 2.0, -600.0),
        DVec3::new(0.0, 2.0, -900.0),
        30.0,
    );
    let mut cams = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in cams.iter_mut(&mut app.world) {
        c.active = e == away;
    }
    let stag = |mut s: Strip| {
        s.male = true;
        s
    };
    let strips = [
        held(strip(
            "deer-walk",
            Species::Deer,
            deer_walk,
            every(2.0, 0.09),
        )),
        held(stag(strip(
            "deer-trot",
            Species::Deer,
            deer_trot,
            every(2.0, 0.067),
        ))),
        strip("deer-gallop", Species::Deer, deer_gallop, every(2.0, 0.05)),
        strip(
            "deer-alarm",
            Species::Deer,
            deer_alarm,
            [0.6, 0.95, 1.4, 2.2, 2.5, 2.62, 2.75, 3.0],
        ),
        strip(
            "deer-death",
            Species::Deer,
            deer_death,
            [0.9, 1.08, 1.2, 1.32, 1.45, 1.58, 1.75, 2.3],
        ),
        held(strip(
            "deer-stop",
            Species::Deer,
            deer_stop,
            every(1.0, 0.15),
        )),
        strip("deer-bend", Species::Deer, deer_bend, every(1.5, 0.15)),
        held(strip(
            "wolf-trot",
            Species::Wolf,
            wolf_trot,
            every(2.0, 0.05),
        )),
        strip(
            "wolf-chase",
            Species::Wolf,
            wolf_chase,
            [0.5, 1.2, 1.27, 1.34, 2.4, 2.45, 2.5, 2.55],
        ),
        // One stride of the gallop.
        strip("wolf-gallop", Species::Wolf, wolf_chase, every(2.4, 0.033)),
        strip(
            "wolf-hunt",
            Species::Wolf,
            wolf_hunt,
            [0.7, 1.15, 1.27, 1.4, 1.6, 2.5, 2.62, 2.75],
        ),
        held(strip(
            "wolf-stalk",
            Species::Wolf,
            wolf_stalk,
            every(2.0, 0.1),
        )),
        held(strip(
            "rabbit-hop",
            Species::Rabbit,
            rabbit_hop,
            every(2.0, 0.05),
        )),
        strip("rabbit-run", Species::Rabbit, rabbit_run, every(2.0, 0.033)),
        flight(
            "starling-flight",
            [0.5, 0.65, 0.73, 1.4, 2.6, 3.2, 4.4, 4.8],
        ),
        held(strip(
            "boar-walk",
            Species::Boar,
            boar_walk,
            every(2.0, 0.066),
        )),
        held(stag(strip(
            "boar-trot",
            Species::Boar,
            boar_trot,
            every(2.0, 0.05),
        ))),
        strip("boar-gallop", Species::Boar, boar_gallop, every(2.0, 0.033)),
        held(strip(
            "boar-rooting",
            Species::Boar,
            boar_rooting,
            every(2.0, 0.2),
        )),
        stag(strip(
            "boar-charge",
            Species::Boar,
            boar_charge,
            [0.6, 1.15, 1.5, 2.1, 2.24, 2.33, 2.45, 2.8],
        )),
        strip(
            "piglet-gallop",
            Species::Boar,
            boar_gallop,
            every(2.0, 0.025),
        ),
        strip(
            "deer-hurt",
            Species::Deer,
            deer_hurt,
            [0.9, 1.04, 1.08, 1.14, 1.22, 1.34, 1.5, 1.8],
        ),
        strip(
            "deer-hurt-far",
            Species::Deer,
            deer_hurt_far,
            [0.9, 1.04, 1.08, 1.14, 1.22, 1.34, 1.5, 1.8],
        ),
        stag(strip(
            "boar-search",
            Species::Boar,
            boar_search,
            [0.9, 1.1, 1.5, 1.9, 2.3, 2.7, 3.1, 3.5],
        )),
        strip(
            "wolf-hurt",
            Species::Wolf,
            wolf_hurt,
            [0.9, 1.04, 1.08, 1.14, 1.22, 1.34, 1.5, 1.8],
        ),
        strip(
            "boar-hurt",
            Species::Boar,
            boar_hurt,
            [0.9, 1.04, 1.08, 1.14, 1.22, 1.34, 1.5, 1.8],
        ),
    ];
    let strips = strips.map(|mut s| {
        s.young = s.name.starts_with("piglet");
        s
    });
    let strips: Vec<Strip> = strips
        .into_iter()
        .filter(|s| only.is_none_or(|o| o.contains(&s.name)))
        .collect();
    assert_eq!(
        strips.len(),
        only.map_or(strips.len(), <[&str]>::len),
        "no strip named in {only:?}"
    );
    // The ground, out past the last row of strips (each row 100 m on from the last).
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [
                    4000.0,
                    2.0 * (100.0 * strips.len() as f32 + 200.0).max(2000.0),
                ],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55703a).with_roughness(0.95),
        ),
    ));
    // Moments on the 60 Hz grid.
    let grid = |t: f32| (f64::from(t) * 60.0).round() / 60.0;
    // A first run of each course finds where the animal is at each moment.
    let probes: Vec<Entity> = strips
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let origin = DVec3::new(-300.0 + 50.0 * k as f64, 0.0, -150.0);
            let e = course_animal(&mut app, s, origin, 0.0);
            app.world.entity_mut(e).insert(Track::default());
            e
        })
        .collect();
    app.world.flush();
    let end = strips
        .iter()
        .map(|s| grid(s.times[FRAMES - 1]))
        .fold(0.0, f64::max);
    live_until(&mut app, probes[0], end + 0.05, size == (SW, SH));
    let went: Vec<[DVec3; FRAMES]> = strips
        .iter()
        .zip(&probes)
        .map(|(s, &e)| {
            let track = &app.world.get::<Track>(e).expect("track").0;
            let start = app.world.get::<Drive>(e).map_or_else(
                || app.world.get::<Flight>(e).expect("flight").origin,
                |d| d.origin,
            );
            std::array::from_fn(|i| {
                let t = grid(s.times[i]);
                let (_, p) = track
                    .iter()
                    .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
                    .expect("steps");
                *p - start
            })
        })
        .collect();
    for e in probes {
        app.world.despawn(e);
    }
    // Then every frame of every strip: its own animal, set off so that it reaches its moment
    // when the strip is shot, from where it then stands in its frame.
    let mut frames: Vec<Vec<Entity>> = Vec::new();
    let mut views = Vec::new();
    let mut shoot_at = Vec::new();
    for (k, s) in strips.iter().enumerate() {
        let (gap, h) = match s.species {
            Species::Deer => (3.2, 0.9),
            Species::Wolf => (2.6, 0.5),
            Species::Boar if s.young => (1.3, 0.24),
            Species::Boar => (2.6, 0.5),
            Species::Rabbit => (0.8, 0.12),
            Species::Bird => (0.55, 0.12),
        };
        // Lower for a strip that ends with the animal on the ground.
        let down = s
            .course
            .is_some_and(|c| c(s.times[FRAMES - 1]).state == AnimalState::Dead);
        let h = if down { h * 0.5 } else { h };
        let width = gap * FRAMES as f64;
        let row = DVec3::new(0.0, 0.0, 100.0 * k as f64);
        let at = grid(s.times[FRAMES - 1]);
        let mid = went[k][FRAMES / 2 - 1].lerp(went[k][FRAMES / 2], 0.5);
        let mut list = Vec::new();
        for (i, (&moment, &here)) in s.times.iter().zip(&went[k]).enumerate() {
            let center = row + DVec3::X * ((i as f64 + 0.5) * gap - width * 0.5);
            let off = if s.track { here } else { mid };
            let origin = center - DVec3::new(off.x, if s.track { off.y } else { 0.0 }, off.z);
            list.push(course_animal(&mut app, s, origin, at - grid(moment)));
            if !s.track {
                // Lines across the ground every 0.5 m (the same under every frame), to see
                // the feet stay where they are set down.
                let unit = if s.species == Species::Rabbit {
                    0.1
                } else {
                    0.5
                };
                let n = (gap * 0.5 / unit).floor() as i32;
                for j in -n..=n {
                    app.world.spawn((
                        Transform::from_translation(
                            center + DVec3::new(j as f64 * unit, 0.0005, 0.0),
                        ),
                        MeshRenderer::new(
                            Shape::Cuboid {
                                size: [0.012 * unit as f32 / 0.5, 0.001, gap as f32 * 0.5],
                            },
                            StandardMaterial::hex(0x6a8648).with_roughness(0.95),
                        ),
                    ));
                }
            }
        }
        frames.push(list);
        // Far off with a long lens, from the animals' right (+Z), a little above.
        let dist = width * 1.9;
        let target = row + DVec3::Y * h;
        let half_h = width * 0.5 * f64::from(SH) / f64::from(SW) + 0.02;
        let fov = (2.0 * (half_h / dist).atan()).to_degrees() as f32;
        let eye = target + DVec3::new(0.0, dist * 0.1, dist);
        views.push((eye, target, fov));
        shoot_at.push(at);
    }
    app.world.flush();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("npc_art_animals");
    std::fs::create_dir_all(&dir).expect("dir");
    let clock = frames[0][0];
    let mut order: Vec<usize> = (0..strips.len()).collect();
    order.sort_by(|&a, &b| shoot_at[a].total_cmp(&shoot_at[b]));
    for k in order {
        live_until(&mut app, clock, shoot_at[k], size == (SW, SH));
        let (eye, target, fov) = views[k];
        let cam = camera(&mut app, eye, target, fov);
        let name = format!("{}animal_motion_{}.png", tag(size), strips[k].name);
        let img = shoot(&mut app, cam, &dir, &name).expect("rendering");
        app.world.despawn(cam);
        let mut cams = app.world.query::<(Entity, &mut Camera)>();
        for (e, mut c) in cams.iter_mut(&mut app.world) {
            c.active = e == away;
        }
        // Every frame shows its animal.
        let (w, h) = img.dimensions();
        for i in 0..FRAMES as u32 {
            let x0 = w * i / FRAMES as u32;
            let mut n = 0;
            // Above the lines on the ground.
            for y in h / 8..h * 5 / 8 {
                for x in x0 + 3..x0 + w / FRAMES as u32 {
                    let p = img.get_pixel(x, y);
                    let q = img.get_pixel(x - 3, y);
                    let d: i32 = (0..3).map(|c| (p[c] as i32 - q[c] as i32).abs()).sum();
                    n += usize::from(d > 30);
                }
            }
            // As many in a small strip as in a full-size one (only harder to pass).
            assert!(
                n > 60,
                "{name} frame {i}: the animal should show up ({n} px)"
            );
        }
    }
}

// ------------------------------------------------------------------------------------------
// The wildlife AI at work: one live run each (the animals think for themselves), shot at
// eight moments and laid side by side. Full-size pictures only (ignored tests).
// ------------------------------------------------------------------------------------------

/// What a live run's markers mean.
const TESTER: u32 = 0xff7a1a;
const FLANK_LEFT: u32 = 0x2f6bff;
const FLANK_RIGHT: u32 = 0xb04dff;
const AMBUSHER: u32 = 0xffd21a;
const QUARRY: u32 = 0xff2a2a;
const ALERT: u32 = 0xffd21a;
const FLEEING: u32 = 0xff2a2a;
const SNORT: u32 = 0xffffff;

/// An app whose animals think for themselves, `size` pixels a picture; `None` without a GPU.
fn live_app(size: (u32, u32)) -> Option<App> {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        (w.width, w.height) = size;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.0 });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    {
        let mut s = app.world.resource_mut::<WildlifeSettings>();
        s.respawn_secs = 0.0;
        s.howl = false;
    }
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.45, -0.55, -0.7), Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55703a).with_roughness(0.95),
        ),
    ));
    Some(app)
}

fn live_animal(app: &mut App, species: Species, at: DVec3, herd: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Animal::new(species),
            Herd(herd),
            StandIn::default(),
        ))
        .id()
}

fn live_herd(app: &mut App, species: Species, center: DVec3, n: usize, id: u32) -> Vec<Entity> {
    (0..n)
        .map(|k| {
            let a = k as f64 / n as f64 * std::f64::consts::TAU;
            let at = center + DVec3::new(a.cos() * 2.5, 0.0, a.sin() * 2.5);
            live_animal(app, species, at, id)
        })
        .collect()
}

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn mind_of(app: &App, e: Entity) -> &AnimalMind {
    app.world.get::<AnimalMind>(e).unwrap()
}

/// One 60 Hz step of the animals' lives, without drawing.
fn tick(app: &mut App, clock: Entity) {
    let now = app.world.get::<NpcClock>(clock).map_or(0.0, |c| c.now);
    live_until(app, clock, now + 1.0 / 60.0, false);
}

fn now_of(app: &App, clock: Entity) -> f64 {
    app.world.get::<NpcClock>(clock).map_or(0.0, |c| c.now)
}

/// A ball of colour `rgb` at `at` (markers, crumbs along a trail).
fn ball(app: &mut App, at: DVec3, radius: f32, rgb: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            MeshRenderer::new(
                Shape::Sphere {
                    radius,
                    segments: 12,
                },
                StandardMaterial::hex(rgb).with_roughness(0.6),
            ),
        ))
        .id()
}

/// A picture from `eye` looking at `target` (`up` the picture's up), checked to show
/// something, and the camera removed again.
fn live_shot(
    app: &mut App,
    (eye, target, up): (DVec3, DVec3, Vec3),
    fov: f32,
    name: &str,
) -> image::RgbaImage {
    let cam = app
        .world
        .spawn((
            Camera::perspective(fov),
            Transform::from_translation(eye).looking_at(target, up),
        ))
        .id();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("npc_art_animals");
    std::fs::create_dir_all(&dir).expect("dir");
    let img = shoot(app, cam, &dir, name).expect("rendering");
    app.world.despawn(cam);
    // The animals are small and anywhere in these, so look over all of the picture.
    let busy = busy_in(&img, (0, 0), img.dimensions());
    eprintln!("{name}: {busy} busy px");
    assert!(busy > 400, "{name}: the animals should show up ({busy} px)");
    img
}

/// The pictures side by side, saved as `name` (and kept with `NPC_SCREENSHOT_DIR`).
fn side_by_side(frames: &[image::RgbaImage], name: &str) {
    let (w, h) = frames[0].dimensions();
    let mut out = image::RgbaImage::new(w * frames.len() as u32, h);
    for (i, f) in frames.iter().enumerate() {
        image::imageops::replace(&mut out, f, i64::from(w * i as u32), 0);
    }
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("npc_art_animals");
    out.save(dir.join(name)).expect("save");
    if let Ok(keep) = std::env::var("NPC_SCREENSHOT_DIR") {
        out.save(std::path::Path::new(&keep).join(name))
            .expect("save");
    }
    eprintln!("wrote {name}");
}

fn role_colour(r: PackRole) -> u32 {
    match r {
        PackRole::Tester | PackRole::Chaser => TESTER,
        PackRole::FlankLeft => FLANK_LEFT,
        PackRole::FlankRight => FLANK_RIGHT,
        PackRole::Ambusher => AMBUSHER,
        PackRole::None => 0x808080,
    }
}

/// A pack of four hunting a herd of four from above, the wounded deer furthest from it:
/// balls over the wolves in the colours of their parts (orange the tester, blue and purple
/// the left and right flankers, yellow the ambusher), red over the quarry, and crumbs where
/// each has been. The hunt is run once to time it, then again for three moments of the
/// stalk, four of the chase and one just after the kill. The PNG is
/// `animal_live_pack_hunt.png`.
#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn a_pack_hunt_from_above() {
    let Some((rush, kill)) = pack_hunt(None) else {
        return;
    };
    eprintln!("the pack rushes at {rush:.2} s and kills at {kill:.2} s");
    let mut plan = vec![2.0, rush * 0.5, rush - 0.4];
    plan.extend((0..4).map(|k| rush + (kill - rush) * k as f64 / 4.0));
    plan.push(kill + 0.3);
    pack_hunt(Some(&plan));
}

/// The hunt of [`a_pack_hunt_from_above`], shot at the moments of `plan` (seconds from the
/// start) when there is one: when the first wolf set off running, and when the quarry fell.
fn pack_hunt(plan: Option<&[f64]>) -> Option<(f64, f64)> {
    let mut app = live_app((640, 640))?;
    let deer = live_herd(&mut app, Species::Deer, DVec3::ZERO, 4, 1);
    let pack = live_herd(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -45.0), 4, 2);
    // A first step of their lives (which sets up their minds), then the pack is hungry.
    tick(&mut app, deer[0]);
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    {
        let mut v = app.world.get_mut::<NpcVitals>(deer[1]).unwrap();
        v.health = v.max_health * 0.4;
    }
    let clock = deer[0];
    let start = now_of(&app, clock);
    let mut frames = Vec::new();
    let mut rush: Option<f64> = None;
    let mut killed: Option<f64> = None;
    let mut last_crumb = f64::MIN;
    loop {
        tick(&mut app, clock);
        let t = now_of(&app, clock) - start;
        assert!(t < 90.0, "the hunt took too long");
        if rush.is_none()
            && pack
                .iter()
                .any(|&w| mind_of(&app, w).state == AnimalState::Chase)
        {
            rush = Some(t);
        }
        if killed.is_none()
            && deer
                .iter()
                .any(|&d| mind_of(&app, d).state == AnimalState::Dead)
        {
            killed = Some(t);
        }
        // Crumbs every 0.25 s: red after the quarry, the wolves' in their colours.
        if t - last_crumb >= 0.25 {
            last_crumb = t;
            for &w in &pack {
                let m = mind_of(&app, w);
                let rgb = role_colour(m.role);
                let p = at(&app, w);
                ball(&mut app, p + DVec3::Y * 0.05, 0.12, rgb);
            }
            if let Some(q) = mind_of(&app, pack[0]).prey {
                let p = at(&app, q);
                ball(&mut app, p + DVec3::Y * 0.05, 0.14, QUARRY);
            }
        }
        let Some(plan) = plan else {
            match (rush, killed) {
                (Some(r), Some(k)) => return Some((r, k)),
                _ => continue,
            }
        };
        if frames.len() == plan.len() {
            break;
        }
        if t < plan[frames.len()] {
            continue;
        }
        let mut marks = Vec::new();
        let mut seen = Vec::new();
        for &w in &pack {
            let m = mind_of(&app, w);
            let (rgb, p) = (role_colour(m.role), at(&app, w));
            seen.push(p);
            marks.push(ball(&mut app, p + DVec3::Y * 1.6, 0.8, rgb));
        }
        let wolves = seen.iter().sum::<DVec3>() / seen.len() as f64;
        // Midway between the pack and its quarry (the herd, before it has one), high enough
        // to take in every wolf with some ground around.
        let quarry = match mind_of(&app, pack[0]).prey {
            Some(q) => {
                let p = at(&app, q);
                marks.push(ball(&mut app, p + DVec3::Y * 2.0, 0.9, QUARRY));
                p
            }
            None => deer.iter().map(|&d| at(&app, d)).sum::<DVec3>() / deer.len() as f64,
        };
        seen.push(quarry);
        let mid = (wolves + quarry) / 2.0;
        let reach = seen
            .iter()
            .map(|p| (p.x - mid.x).abs().max((p.z - mid.z).abs()))
            .fold(0.0, f64::max);
        let high = ((reach + 6.0) / 22f64.to_radians().tan()).clamp(30.0, 75.0);
        let view = (mid + DVec3::new(0.0, high, 0.01), mid, Vec3::NEG_Z);
        let name = format!("animal_live_pack_hunt_{}.png", frames.len());
        eprintln!(
            "frame {} at {t:.2} s: {:.0} m up, rush {rush:?}, kill {killed:?}, roles {:?}, \
             states {:?}",
            frames.len(),
            high,
            pack.iter()
                .map(|&w| mind_of(&app, w).role.name())
                .collect::<Vec<_>>(),
            pack.iter()
                .map(|&w| mind_of(&app, w).state)
                .collect::<Vec<_>>()
        );
        frames.push(live_shot(&mut app, view, 44.0, &name));
        for e in marks {
            app.world.despawn(e);
        }
    }
    side_by_side(&frames, "animal_live_pack_hunt.png");
    Some((rush.unwrap_or(0.0), killed.unwrap_or(0.0)))
}

/// A wary deer that can't outrun the wolf after it, from close behind and low: it runs straight
/// while the wolf is well back and cuts from side to side once it is close (white crumbs
/// where the deer ran, orange where the wolf did). The PNG is `animal_live_zigzag.png`.
#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn a_chased_deer_zigzags() {
    let Some(mut app) = live_app((560, 420)) else {
        return;
    };
    let deer = app
        .world
        .spawn((
            Transform::from_translation(DVec3::ZERO),
            Animal {
                skittishness: 6.0,
                range: 200.0,
                ..Animal::new(Species::Deer)
            },
            Herd(1),
            StandIn::default(),
            NpcMotor {
                run_speed: 6.0,
                sprint_speed: 7.0,
                ..Species::Deer.motor()
            },
        ))
        .id();
    let wolf = live_animal(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -30.0), 2);
    tick(&mut app, deer);
    app.world.get_mut::<AnimalMind>(wolf).unwrap().hunger = 1.0;
    let clock = deer;
    let start = now_of(&app, clock);
    let mut frames = Vec::new();
    let mut close: Option<f64> = None;
    let mut line: Option<Vec3> = None;
    let mut last_crumb = f64::MIN;
    while frames.len() < FRAMES {
        tick(&mut app, clock);
        let t = now_of(&app, clock) - start;
        assert!(t < 60.0, "the wolf never closed in");
        let (d, w) = (at(&app, deer), at(&app, wolf));
        let chasing = mind_of(&app, wolf).state == AnimalState::Chase;
        if chasing && t - last_crumb >= 0.1 {
            last_crumb = t;
            ball(&mut app, d + DVec3::Y * 0.03, 0.07, 0xffffff);
            ball(&mut app, w + DVec3::Y * 0.03, 0.07, TESTER);
        }
        let gap = Vec3::new((d.x - w.x) as f32, 0.0, (d.z - w.z) as f32);
        // From when the wolf is 8 m back: a frame as it closes, then the cuts.
        if chasing && close.is_none() && gap.length() < 8.0 {
            close = Some(t);
            line = Some(gap.normalize());
        }
        let Some(c) = close else { continue };
        if t < c + 0.3 * frames.len() as f64 {
            continue;
        }
        // From up behind and off to the deer's left, looking between the two, so the wolf
        // doesn't hide the deer and the crumbs show the cuts on the ground.
        let ahead = line.unwrap().as_dvec3();
        let left = DVec3::new(ahead.z, 0.0, -ahead.x);
        let mid = (d + w) / 2.0 + DVec3::Y * 0.6;
        let view = (
            mid - ahead * 7.0 + left * 8.0 + DVec3::Y * 6.0,
            mid,
            Vec3::Y,
        );
        let name = format!("animal_live_zigzag_{}.png", frames.len());
        eprintln!(
            "frame {} at {t:.2} s: wolf {:.1} m behind, pursuer {:?}",
            frames.len(),
            gap.length(),
            mind_of(&app, deer).pursuer.map(|p| p.1)
        );
        frames.push(live_shot(&mut app, view, 40.0, &name));
        if mind_of(&app, deer).state == AnimalState::Dead {
            break;
        }
    }
    side_by_side(&frames, "animal_live_zigzag.png");
}

/// A herd of six, one of them shot from out of the picture to the left: it bolts snorting
/// (a white ball), each of the others looks up toward the shot (yellow) and bolts a moment
/// later (red), and they run off bunched. The PNG is `animal_live_herd_alarm.png`.
#[test]
#[ignore = "full-size pictures: run with --ignored"]
fn the_herds_alarm_spreads() {
    let Some(mut app) = live_app((560, 420)) else {
        return;
    };
    let deer = live_herd(&mut app, Species::Deer, DVec3::ZERO, 6, 3);
    app.update();
    let clock = deer[0];
    let settle = now_of(&app, clock) + 1.0;
    live_until(&mut app, clock, settle, false);
    let shooter = app
        .world
        .spawn(Transform::from_translation(DVec3::new(-40.0, 1.5, 0.0)))
        .id();
    let moments = [-0.1, 0.05, 0.15, 0.3, 0.45, 0.6, 0.8, 1.6];
    let shot_at = now_of(&app, clock) + 0.1;
    let mut frames = Vec::new();
    let mut fired = false;
    for (i, m) in moments.iter().enumerate() {
        while now_of(&app, clock) < shot_at + m - 1e-6 {
            if !fired && now_of(&app, clock) >= shot_at - 1e-6 {
                fired = true;
                let p = at(&app, deer[0]);
                app.world.write_message(engine_world::DamageEvent {
                    target: deer[0],
                    amount: 5.0,
                    kind: engine_world::DamageKind::Ballistic,
                    point: p,
                    impulse: Vec3::ZERO,
                    source: Some(shooter),
                });
            }
            tick(&mut app, clock);
        }
        let mut marks = Vec::new();
        let mut sum = DVec3::ZERO;
        for &d in &deer {
            let (mind, p) = (mind_of(&app, d), at(&app, d));
            sum += p;
            let rgb = match mind.state {
                _ if mind.alarm.is_some_and(|a| a.1) => Some(SNORT),
                AnimalState::Flee => Some(FLEEING),
                AnimalState::Alert => Some(ALERT),
                _ => None,
            };
            if let Some(rgb) = rgb {
                marks.push(ball(&mut app, p + DVec3::Y * 2.0, 0.22, rgb));
            }
        }
        let mid = sum / deer.len() as f64;
        let view = (
            mid + DVec3::new(-4.0, 5.0, 17.0),
            mid + DVec3::Y * 0.7,
            Vec3::Y,
        );
        let name = format!("animal_live_herd_alarm_{i}.png");
        eprintln!(
            "frame {i} at {m:+.2} s: {:?}",
            deer.iter()
                .map(|&d| mind_of(&app, d).state.name())
                .collect::<Vec<_>>()
        );
        frames.push(live_shot(&mut app, view, 34.0, &name));
        for e in marks {
            app.world.despawn(e);
        }
    }
    side_by_side(&frames, "animal_live_herd_alarm.png");
}
