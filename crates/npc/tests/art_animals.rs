//! Close-up renders of the procedural wildlife frozen in chosen states (grazing, alert,
//! walking, trotting, galloping, lying, dead, stalking, biting, hopping, sitting up, pecking,
//! flying), plus budget checks on the generated meshes. Rendering needs a GPU or lavapipe and
//! passes with a note without one. Set `NPC_SCREENSHOT_DIR` to keep the PNGs.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::art::animals;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::locomotion::{LocomotionMode, LocomotionState};

const W: u32 = 960;
const H: u32 = 540;

/// One frozen animal.
#[derive(Clone, Copy)]
struct Pose {
    species: Species,
    male: bool,
    state: AnimalState,
    /// Seconds since the state began.
    t: f64,
    speed: f32,
    mode: LocomotionMode,
    phase: f32,
    /// Animal clock (gallops and flaps are timed by it).
    clock: f64,
    flap: f32,
    bite: bool,
}

impl Pose {
    fn new(species: Species, state: AnimalState) -> Self {
        Self {
            species,
            male: false,
            state,
            t: 5.0,
            speed: 0.0,
            mode: LocomotionMode::Idle,
            phase: 0.0,
            clock: 100.0,
            flap: 0.0,
            bite: false,
        }
    }

    fn moving(mut self, speed: f32, phase: f32, mode: LocomotionMode) -> Self {
        self.speed = speed;
        self.phase = phase;
        self.mode = mode;
        self
    }
}

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
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
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55703a).with_roughness(0.95),
        ),
    ));
    app
}

/// Spawn `poses` 40 m apart along X, facing +X; returns their entities and positions.
fn spawn(app: &mut App, poses: &[Pose]) -> Vec<(Entity, DVec3)> {
    let mut out = Vec::new();
    for (i, p) in poses.iter().enumerate() {
        let at = DVec3::new(i as f64 * 40.0, 0.0, 0.0);
        let mut animal = Animal::new(p.species);
        animal.male = p.male;
        let e = app
            .world
            .spawn((
                Transform::from_translation(at)
                    .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
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
            l.velocity = Vec3::X * p.speed;
            l.mode = p.mode;
            l.grounded = p.state != AnimalState::Flying;
        }
        if let Some(mut m) = ent.get_mut::<MoverState>() {
            m.phase = p.phase;
        }
        if let Some(mut b) = ent.get_mut::<BirdFlight>() {
            b.flap = p.flap;
            b.velocity = Vec3::X * p.speed;
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
    let mut n = 0;
    for y in h / 4..h * 3 / 4 {
        for x in w / 4..w * 3 / 4 {
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

fn render(shots: &[Shot]) {
    let mut app = app();
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    let list: Vec<Pose> = shots.iter().map(|p| p.1).collect();
    let animals = spawn(&mut app, &list);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("npc_art_animals");
    std::fs::create_dir_all(&dir).expect("dir");
    for (i, (name, pose, dist, azimuth)) in shots.iter().enumerate() {
        let at = animals[i].1;
        let (h, ahead) = match pose.species {
            Species::Deer if pose.male => (0.95, 0.4),
            Species::Deer => (0.85, 0.35),
            Species::Wolf => (0.45, 0.25),
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
                Species::Rabbit => (0.16, 0.1),
                Species::Bird => (0.1, 0.05),
            }
        } else if name.ends_with("_feet") {
            match pose.species {
                Species::Deer => (0.12, 0.42),
                Species::Wolf => (0.06, 0.36),
                _ => (0.02, 0.08),
            }
        } else {
            (h, ahead)
        };
        let lift = if pose.state == AnimalState::Flying {
            0.6
        } else {
            0.0
        };
        let az = azimuth.to_radians();
        // The animal faces +X: look at its middle, a little toward the head.
        let target = at + DVec3::new(ahead, h + lift, 0.0);
        let eye = target + DVec3::new(dist * az.sin(), dist * 0.22, dist * az.cos());
        let cam = camera(&mut app, eye, target, 28.0);
        hold(&mut app, &animals, &list);
        app.update();
        hold(&mut app, &animals, &list);
        let Some(img) = shoot(&mut app, cam, &dir, &format!("animal_{name}.png")) else {
            return;
        };
        assert!(
            busy(&img) > 400,
            "{name}: the animal should show up ({} px)",
            busy(&img)
        );
    }
}

#[test]
fn deer_closeups() {
    let p = |state| Pose::new(Species::Deer, state);
    let stag = |state| Pose {
        male: true,
        ..Pose::new(Species::Deer, state)
    };
    render(&[
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
                clock: 100.15,
                ..p(AnimalState::Flee).moving(10.0, 0.0, LocomotionMode::Sprint)
            },
            5.0,
            10.0,
        ),
        ("deer_stag_rest", stag(AnimalState::Rest), 4.5, 40.0),
        ("deer_hind_dead", p(AnimalState::Dead), 4.5, 60.0),
        ("deer_hind_rear", p(AnimalState::Alert), 3.5, -140.0),
    ]);
}

#[test]
fn wolf_closeups() {
    let w = |state| Pose::new(Species::Wolf, state);
    render(&[
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
            Pose {
                clock: 100.2,
                ..w(AnimalState::Chase).moving(11.0, 0.0, LocomotionMode::Sprint)
            },
            3.8,
            10.0,
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
                clock: 100.1,
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
    ]);
}

#[test]
fn meshes_fit_their_budgets() {
    let mut total = 0.0;
    for species in [Species::Deer, Species::Wolf] {
        let (meshes, secs) = animals::build_all(species);
        total += secs;
        for male in [false, true] {
            let tris = animals::triangles_per_animal(species, male, &meshes);
            let parts = animals::piece_count(species, male);
            eprintln!("{species:?} male={male}: {tris} triangles in {parts} parts ({secs:.2} s)");
            assert!(tris <= 42_000, "{species:?}: {tris} triangles");
            assert!(parts <= 30, "{species:?}: {parts} parts");
        }
    }
    eprintln!("all animals generated in {total:.2} s");
}
