//! Captioned render strips of a squad suppressing, flanking and flushing out a player in
//! cover: whole NPCs with their AI, in the yard of `tactics.rs` dressed with the arena's art
//! (its sand, and its cut sandstone for every wall and block). Needs a GPU or a software Vulkan
//! driver (lavapipe) for the pictures (the behaviour is checked either way).
//!
//! * Eye level, from behind the player's wall: the suppressor's tracers coming in over it
//!   while the squad's flankers slip away, then a flanker off the player's side, firing.
//! * Top down at the same moments: the walls, the player (blue), the suppressor (red) and the
//!   flankers' paths so far (yellow dots, a big dot at each cover they held at).
//! * Over the player's shoulder: a grenade lobbed over its wall (its arc marked), the player
//!   running from it, the blast, and the player in the open under fire.
//!
//! Tracers, marks and the blast's flash are drawn by the test (no fire or smoke module is
//! installed), unlit and without shadows. The strips land in the test's temp dir as
//! `fighters-flank-eye-level.png`, `fighters-flank-top-down.png` and
//! `fighters-flush-grenade.png`; set `NPC_SCREENSHOT_DIR` to also copy them there. Without it
//! nothing is drawn (drawing is costly without a GPU): the scenes play out the same and every
//! check runs.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::locomotion::LocomotionState;
use engine_world::physics::Collider;
use engine_world::{ProjectileTrace, SimulationFocus, WorldImpulse, WorldPlugin};

/// One picture of the strip, and the band over it for its caption.
const TILE_W: u32 = 400;
const TILE_H: u32 = 340;
const BAND: u32 = 30;
/// The simulation step (s), as in `tactics.rs`.
const STEP: f64 = 1.0 / 60.0;
/// The seed of the squad's randomness.
const SEED: u64 = 0;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = TILE_W;
        win.height = TILE_H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    if std::env::var_os("NPC_SCREENSHOT_DIR").is_none() {
        app.add_systems(Extract, hold_off_drawing);
    }
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.6, -0.6, 0.55), Vec3::Y),
    ));
    app
}

/// No drawing (without `NPC_SCREENSHOT_DIR`). Held off in the first frame, where the renderer
/// would be made, so the scene's entities are numbered as they are with pictures.
fn hold_off_drawing(world: &mut World) {
    if !world.contains_resource::<RenderDisabled>() {
        world.insert_resource(RenderDisabled(
            "pictures only with NPC_SCREENSHOT_DIR".into(),
        ));
    }
}

/// The yard of `tactics.rs`: the player's wall facing the squad 20 m off along +Z, and cover
/// on both sides to bound between (center and half size of each wall and block).
fn yard(app: &mut App) -> Vec<(DVec3, Vec3)> {
    let mut walls = vec![(DVec3::new(0.0, 1.0, 1.4), Vec3::new(2.4, 1.0, 0.25))];
    for sx in [-1.0, 1.0] {
        walls.extend([
            (DVec3::new(4.5 * sx, 1.0, 14.0), Vec3::new(1.2, 1.0, 0.3)),
            (DVec3::new(8.0 * sx, 0.6, 11.0), Vec3::new(0.6, 0.6, 0.6)),
            (DVec3::new(9.5 * sx, 1.0, 6.5), Vec3::new(0.3, 1.0, 1.4)),
            (DVec3::new(11.5 * sx, 0.6, 2.5), Vec3::new(0.6, 0.6, 0.6)),
        ]);
    }
    for &(c, h) in &walls {
        app.world
            .spawn((Transform::from_translation(c), Collider::cuboid(h)));
    }
    walls
}

/// The yard's look, once the scene is set (so the AI's entities are numbered the same with
/// pictures or without): the arena's art when drawing, its sand underfoot and its cut
/// sandstone for the walls and blocks (each block chipped to its wall's size), else plain
/// shapes.
fn dress(app: &mut App, walls: &[(DVec3, Vec3)]) {
    use bevy_ecs::system::SystemState;
    use engine_npc::art::ArtAssets;
    use engine_npc::art::arena::{Chip, chipped_block, sand_set, span, stone_set};
    let drawing = std::env::var_os("NPC_SCREENSHOT_DIR").is_some();
    let tiled = |meters: f32| {
        StandardMaterial {
            base_color: [1.0; 4],
            ..Default::default()
        }
        .with_uv_scale(1.0 / meters, 1.0 / meters)
    };
    let mut state = SystemState::<ArtAssets>::new(&mut app.world);
    let kit = match state.get_mut(&mut app.world) {
        Ok(mut art) if drawing && art.available() => Some({
            let sand = art.material("arena/sand", tiled(span::SAND), sand_set);
            let stone = art.material("arena/stone", tiled(span::STONE), stone_set);
            let blocks: Vec<_> = walls
                .iter()
                .enumerate()
                .map(|(i, &(_, h))| {
                    art.mesh(&format!("tactics/block{i}"), || {
                        chipped_block(
                            h,
                            Chip {
                                voxel: 0.04,
                                depth: 0.06,
                                breaks: 2,
                                ..Chip::new(91 + i as u64)
                            },
                        )
                    })
                })
                .collect();
            (sand, stone, blocks)
        }),
        _ => None,
    };
    let plane = Shape::Plane {
        size: [400.0, 400.0],
        subdivisions: 1,
    };
    match kit {
        Some((Some(sand), Some(stone), blocks)) => {
            let floor = MeshRenderer::new(plane, StandardMaterial::default());
            app.world.spawn((
                Transform::default(),
                MeshRenderer {
                    material: sand,
                    ..floor
                },
            ));
            for (&(c, h), mesh) in walls.iter().zip(blocks) {
                let plain = MeshRenderer::new(
                    Shape::Cuboid {
                        size: (h * 2.0).to_array(),
                    },
                    StandardMaterial::default(),
                );
                app.world.spawn((
                    Transform::from_translation(c),
                    MeshRenderer {
                        mesh: mesh.unwrap_or(plain.mesh.clone()),
                        material: stone.clone(),
                        ..plain
                    },
                ));
            }
        }
        _ => {
            app.world.spawn((
                Transform::default(),
                MeshRenderer::new(plane, StandardMaterial::hex(0x8c7a5b).with_roughness(0.95)),
            ));
            for &(c, h) in walls {
                app.world.spawn((
                    Transform::from_translation(c),
                    MeshRenderer::new(
                        Shape::Cuboid {
                            size: (h * 2.0).to_array(),
                        },
                        StandardMaterial::hex(0x4f5560).with_roughness(0.9),
                    ),
                ));
            }
        }
    }
}

/// A veteran, an elite and a veteran with carbines, 20 m off and facing the player.
fn squad(app: &mut App) -> Vec<Entity> {
    let skills = [
        NpcSkills::veteran(),
        NpcSkills::elite(),
        NpcSkills::veteran(),
    ];
    let n = skills.len();
    skills
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let x = (k as f64 - (n as f64 - 1.0) * 0.5) * 3.0;
            let mut t = Transform::from_translation(DVec3::new(x, 0.0, 20.0));
            t.look_to(Vec3::NEG_Z, Vec3::Y);
            let e = app
                .world
                .spawn((
                    t,
                    Npc::default(),
                    Faction::ENEMY,
                    Squad(5),
                    s,
                    NpcWeapon::rifle(),
                    Seed(hash_u64(SEED * 31 + k as u64 + 1)),
                    StandIn {
                        color: None,
                        plain: false,
                    },
                ))
                .id();
            let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
            v.max_health = 1e6;
            v.health = 1e6;
            e
        })
        .collect()
}

const PLAYER: DVec3 = DVec3::ZERO;
const EYE: DVec3 = DVec3::new(0.0, 1.6, 0.0);
/// Where a player flushed out of its cover runs to: out past the end of its wall.
const ESCAPE: DVec3 = DVec3::new(-6.5, 0.0, -2.5);

/// The player, as in `tactics.rs`: seen beside its wall for a moment, then down behind it, up
/// over it a moment to fire back now and then (at whoever pins it); if it `flees`, it runs out
/// of cover from a grenade coming in.
struct Player {
    e: Entity,
    next_shot: f64,
    up_until: f64,
    flees: bool,
    fled_at: Option<f64>,
}

impl Player {
    fn spawn(app: &mut App, flees: bool) -> Self {
        let mut t = Transform::from_translation(DVec3::new(3.4, 0.0, 0.0));
        t.look_to(Vec3::Z, Vec3::Y);
        let e = app
            .world
            .spawn((
                t,
                Faction::PLAYER,
                LocomotionState::default(),
                SimulationFocus::default(),
            ))
            .id();
        Player {
            e,
            next_shot: 3.0,
            up_until: 0.0,
            flees,
            fled_at: None,
        }
    }

    fn step(&mut self, app: &mut App, t: f64, squad: &[Entity]) {
        if self.flees
            && self.fled_at.is_none()
            && app.world.resource::<Grenades>().incoming(PLAYER, 4.0)
        {
            self.fled_at = Some(t);
        }
        if self.fled_at.is_some() {
            let p = at(app, self.e);
            let to = ESCAPE - p;
            let step = to.normalize_or_zero() * (5.0 * STEP).min(to.length());
            app.world.get_mut::<Transform>(self.e).unwrap().translation = p + step;
            app.world
                .get_mut::<LocomotionState>(self.e)
                .unwrap()
                .velocity = (step / STEP).as_vec3();
            return;
        }
        if t >= 1.5 {
            let up = if t < self.up_until { 0.6 } else { 0.0 };
            app.world.get_mut::<Transform>(self.e).unwrap().translation = PLAYER + DVec3::Y * up;
        }
        if t >= self.next_shot {
            self.next_shot += 2.5;
            self.up_until = t + 0.4;
            let pinned_by = squad
                .iter()
                .copied()
                .find(|&e| brain(app, e).role == "suppress")
                .unwrap_or(squad[0]);
            let to = at(app, pinned_by) + DVec3::new(0.8, 1.3, 0.0);
            app.world.write_message(ProjectileTrace {
                from: DVec3::new(0.0, 2.2, 0.6),
                to,
                radius: 0.05,
                speed: 600.0,
                shooter: Some(self.e),
            });
        }
    }
}

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn brain(app: &App, e: Entity) -> &NpcBrain {
    app.world.get::<NpcBrain>(e).unwrap()
}

/// Can the player (its eye over the wall) see `p`'s body?
fn seen_by_player(app: &App, p: DVec3) -> bool {
    let world = app.world.resource::<TacticalWorld>();
    world
        .raycast(EYE + DVec3::Y * 0.4, p + DVec3::Y * 1.2)
        .is_none()
}

/// Degrees round from the way the player faces (+Z, at the squad), signed (+ toward +X).
fn bearing(p: DVec3) -> f64 {
    let d = DVec2::new(p.x - PLAYER.x, p.z - PLAYER.z);
    d.x.atan2(d.y).to_degrees()
}

fn camera(app: &mut App, eye: DVec3, target: DVec3, fov: f32, up: Vec3) -> Entity {
    app.world
        .spawn((
            Camera {
                active: false,
                ..Camera::perspective(fov)
            },
            Transform::from_translation(eye).looking_at(target, up),
        ))
        .id()
}

fn use_camera(app: &mut App, cam: Entity) {
    let mut q = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in q.iter_mut(&mut app.world) {
        c.active = e == cam;
    }
}

/// A picture through `cam` with the clocks stopped (then back to looking at nothing). Without
/// a renderer the clocks stop all the same, so the scene plays out as it does with pictures.
fn shoot(app: &mut App, cam: Entity, away: Entity, name: &str) -> Option<image::RgbaImage> {
    use_camera(app, cam);
    let time = |app: &mut App, step_secs: f64| {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs })
    };
    time(app, 0.0);
    app.update_n(3);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let drawn = app.world.get_resource::<RenderDisabled>().is_none();
    if drawn {
        capture_now(&mut app.world, &path).expect("screenshot");
    }
    time(app, STEP);
    use_camera(app, away);
    drawn.then(|| image::open(&path).expect("png").to_rgba8())
}

/// A glowing mark: a tracer, a dot on a path or a body seen from above.
fn glow(hex: u32, nits: f32) -> StandardMaterial {
    let c = [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    ];
    StandardMaterial::hex(hex)
        .with_emissive(c, nits)
        .with_roughness(1.0)
}

/// A tracer along a shot's path (thicker than a real one, so a picture shows it).
fn tracer(app: &mut App, from: DVec3, to: DVec3) -> Entity {
    let len = from.distance(to) as f32;
    let t = Transform::from_translation((from + to) * 0.5).looking_at(to, Vec3::Y);
    app.world
        .spawn((
            t,
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.1, 0.1, len],
                },
                StandardMaterial {
                    unlit: true,
                    ..glow(0xffb050, 80_000.0)
                },
            )
            .without_shadows(),
        ))
        .id()
}

/// A flat dot on the ground, for the top-down map.
fn dot(app: &mut App, p: DVec3, radius: f32, hex: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(DVec3::new(p.x, 0.03, p.z)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius,
                    height: 0.04,
                    segments: 16,
                },
                glow(hex, 4_000.0),
            )
            .without_shadows(),
        ))
        .id()
}

/// A small glowing ball at `p` (a mark along a grenade's arc).
fn dot_at(app: &mut App, p: DVec3, radius: f32, hex: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(p),
            MeshRenderer::new(
                Shape::Sphere {
                    radius,
                    segments: 8,
                },
                StandardMaterial {
                    unlit: true,
                    ..glow(hex, 20_000.0)
                },
            )
            .without_shadows(),
        ))
        .id()
}

/// A picture and its caption, once taken (none without a GPU).
type Picture = Option<(String, Option<image::RgbaImage>)>;

/// How many moments the strips show.
const MOMENTS: usize = 5;

#[test]
fn fighters_flank_a_player_in_cover() {
    let mut app = app();
    let walls = yard(&mut app);
    let squad = squad(&mut app);
    let mut player = Player::spawn(&mut app, false);
    let away = camera(
        &mut app,
        DVec3::new(0.0, 2.0, -300.0),
        DVec3::new(0.0, 2.0, -600.0),
        30.0,
        Vec3::Y,
    );
    // Over the player's wall (its top 2 m up), turned as the moment needs.
    let eye_at = DVec3::new(0.9, 2.55, -0.8);
    let eye = camera(&mut app, eye_at, DVec3::new(0.0, 1.2, 16.0), 56.0, Vec3::Y);
    // Wider, once a flanker is round: it and the end of the wall it came past.
    let eye_wide = camera(&mut app, eye_at, DVec3::new(0.0, 1.2, 16.0), 70.0, Vec3::Y);
    // Straight down over the yard, the squad's side up.
    let map = camera(
        &mut app,
        DVec3::new(0.0, 34.0, 4.5),
        DVec3::new(0.0, 0.0, 4.5),
        54.0,
        Vec3::Z,
    );
    dress(&mut app, &walls);
    use_camera(&mut app, away);

    let mut cursor = app
        .world
        .resource::<Messages<ProjectileTrace>>()
        .get_cursor();
    let mut tracers: Vec<(Entity, f64)> = Vec::new();
    let mut trails: Vec<(DVec3, f32, u32)> = Vec::new();
    let mut was_holding = vec![false; squad.len()];
    let mut eye_shots: [Picture; MOMENTS] = Default::default();
    let mut map_shots: [Picture; MOMENTS] = Default::default();
    let (mut flanked_at, mut flanker) = (None::<f64>, None::<Entity>);
    // The first flanker to get to its spot off the player's side.
    let mut placed = None::<Entity>;
    let mut suppressor_shots = 0u32;
    let mut t = 0.0f64;
    while t < 30.0 && eye_shots.iter().any(Option::is_none) {
        player.step(&mut app, t, &squad);
        app.update();
        t += STEP;
        // The suppressor's shots as tracers, a fifth of a second each.
        tracers.retain(|&(e, until)| {
            let keep = t < until;
            if !keep {
                app.world.despawn(e);
            }
            keep
        });
        let msgs = app.world.resource::<Messages<ProjectileTrace>>();
        let traces: Vec<ProjectileTrace> = cursor.read(msgs).copied().collect();
        let (mut fresh, mut from_flank) = (false, false);
        for tr in traces {
            let Some(s) = tr.shooter else { continue };
            if squad.contains(&s) {
                let e = tracer(&mut app, tr.from, tr.to);
                tracers.push((e, t + 0.2));
                if brain(&app, s).role == "suppress" {
                    suppressor_shots += 1;
                    fresh = true;
                }
                from_flank |= flanked_at.is_some() && flanker == Some(s);
            }
        }
        // The paths, every quarter second.
        for (k, &e) in squad.iter().enumerate() {
            let (p, b) = (at(&app, e), brain(&app, e));
            let color = match b.role {
                "suppress" => 0xe0403a,
                "flank" => 0xf2c94a,
                _ => 0xd8d8d8,
            };
            if b.holding && !was_holding[k] {
                trails.push((p, 0.45, color));
            }
            was_holding[k] = b.holding;
            if ((t / STEP).round() as u64).is_multiple_of(15) {
                trails.push((p, 0.14, color));
            }
            if placed.is_none() && b.role == "flank" && b.in_position {
                placed = Some(e);
            }
            if flanked_at.is_none()
                && b.role == "flank"
                && b.in_position
                && bearing(p).abs() >= 65.0
                && p.distance(PLAYER) < 16.0
                && seen_by_player(&app, p)
            {
                flanked_at = Some(t);
                flanker = Some(e);
            }
        }
        // The moments: pinned (at a suppressor's shot while the player is down, so its tracer
        // runs into the wall), once more on the way, the first flanker at its spot off the
        // player's side, a flanker with a clear shot from there, and its shots.
        let suppressing = squad.iter().any(|&e| brain(&app, e).role == "suppress");
        let n = eye_shots.iter().position(Option::is_none).unwrap();
        let due = match n {
            0 | 1 => suppressing && fresh && t >= [2.5, 5.0][n] && t > player.up_until + 0.2,
            2 => placed.is_some(),
            3 => flanked_at.is_some(),
            _ => from_flank || flanked_at.is_some_and(|f| t >= f + 3.0),
        };
        if !due {
            continue;
        }
        let side = |e: Entity| bearing(at(&app, e)).abs().round();
        let caption = match n {
            0 => format!("{t:.1} S: PINNED BY FIRE"),
            1 => format!("{t:.1} S: STILL PINNED"),
            2 => format!("{t:.1} S: A FLANKER IN POSITION"),
            3 => format!("{t:.1} S: FLANKED, {} DEG ROUND", side(flanker.unwrap())),
            _ => format!("{t:.1} S: SHOT FROM THE FLANK"),
        };
        // Eye level: turned to take in the suppressor and the flanker farthest round (or the
        // squad, before anyone is round).
        let suppressor = squad
            .iter()
            .copied()
            .find(|&e| brain(&app, e).role == "suppress");
        let round = flanker.or(placed).or_else(|| {
            squad
                .iter()
                .copied()
                .filter(|&e| brain(&app, e).role == "flank")
                .max_by(|&a, &b| {
                    let (a, b) = (bearing(at(&app, a)).abs(), bearing(at(&app, b)).abs());
                    a.total_cmp(&b)
                })
        });
        let look = match (suppressor, round) {
            (Some(s), Some(f)) => {
                let (bs, bf) = (bearing(at(&app, s)), bearing(at(&app, f)));
                if n >= 2 {
                    // The flanker off the side and the end of the wall it looks past.
                    bf - 15.0 * bf.signum()
                } else {
                    // At the suppressor (its tracer runs from there to the wall).
                    bs
                }
            }
            _ => 0.0,
        };
        let a = look.to_radians();
        let target = eye_at + DVec3::new(a.sin() * 16.0, -1.35, a.cos() * 16.0);
        let eye = if n >= 2 { eye_wide } else { eye };
        *app.world.get_mut::<Transform>(eye).unwrap() =
            Transform::from_translation(eye_at).looking_at(target, Vec3::Y);
        let img = shoot(&mut app, eye, away, &format!("flank_eye_{n}.png"));
        eye_shots[n] = Some((caption.clone(), img));
        // Top down: the paths so far (and the player), only while the picture is taken.
        let mut marks: Vec<Entity> = trails
            .iter()
            .map(|&(p, r, c)| dot(&mut app, p, r, c))
            .collect();
        marks.push(dot(&mut app, PLAYER, 0.5, 0x3a7ce0));
        for &e in &squad {
            let color = match brain(&app, e).role {
                "suppress" => 0xe0403a,
                "flank" => 0xf2c94a,
                _ => 0xd8d8d8,
            };
            let p = at(&app, e);
            marks.push(dot(&mut app, p, 0.6, color));
        }
        let img = shoot(&mut app, map, away, &format!("flank_map_{n}.png"));
        for e in marks {
            app.world.despawn(e);
        }
        let caption = match n {
            0 => format!("{t:.1} S: RED PINS, YELLOW GO ROUND"),
            _ => caption,
        };
        map_shots[n] = Some((caption, img));
    }
    let flanked_at = flanked_at.expect("no flanker got round");
    eprintln!("flanked at {flanked_at:.1} s; the suppressor fired {suppressor_shots} shots");
    assert!(
        suppressor_shots >= 4,
        "{suppressor_shots} suppressing shots"
    );
    assert!(eye_shots.iter().all(Option::is_some), "a moment never came");

    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping the pictures: no GPU");
        return;
    }
    strip(&eye_shots, "fighters-flank-eye-level.png");
    strip(&map_shots, "fighters-flank-top-down.png");
}

/// A stand-in for the player (it has no body of its own here): a blue capsule, crouched
/// behind its wall and upright when it runs or pops up to fire.
fn player_body(app: &mut App) -> Entity {
    app.world
        .spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Capsule {
                    radius: 0.28,
                    height: 1.75,
                    segments: 16,
                },
                StandardMaterial::hex(0x2f5fb8).with_roughness(0.6),
            ),
        ))
        .id()
}

fn pose_player_body(app: &mut App, body: Entity, player: &Player) {
    let p = at(app, player.e);
    let upright = player.fled_at.is_some() || p.y > 0.1;
    let h = if upright { 1.75 } else { 1.15 };
    let mut t = app.world.get_mut::<Transform>(body).unwrap();
    t.translation = DVec3::new(p.x, 0.0, p.z) + DVec3::Y * (h as f64 * 0.5);
    t.scale = Vec3::new(1.0, h / 1.75, 1.0);
}

/// A blast as the picture shows it (no fire or smoke module is installed here): a flash for a
/// tenth of a second and a dust cloud that swells and settles over two seconds.
struct Burst {
    flash: Entity,
    dust: Entity,
    at: DVec3,
    since: f64,
}

fn burst(app: &mut App, at: DVec3, now: f64) -> Burst {
    let flash = app
        .world
        .spawn((
            Transform::from_translation(at + DVec3::Y * 0.6),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.3,
                    segments: 20,
                },
                StandardMaterial {
                    unlit: true,
                    ..glow(0xffc46a, 120_000.0)
                },
            )
            .without_shadows(),
        ))
        .id();
    let dust = app
        .world
        .spawn((
            Transform::from_translation(at + DVec3::Y * 0.8),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.0,
                    segments: 20,
                },
                StandardMaterial::hex(0x6b6152)
                    .with_roughness(1.0)
                    .with_alpha(0.75),
            ),
        ))
        .id();
    Burst {
        flash,
        dust,
        at,
        since: now,
    }
}

/// Swell and settle the dust, put the flash out; false once it is gone.
fn age_burst(app: &mut App, b: &Burst, now: f64) -> bool {
    let age = now - b.since;
    if age > 0.12
        && let Some(mut r) = app.world.get_mut::<MeshRenderer>(b.flash)
    {
        r.visible = false;
    }
    if age > 2.0 {
        app.world.despawn(b.flash);
        app.world.despawn(b.dust);
        return false;
    }
    let k = (age / 2.0) as f32;
    let mut t = app.world.get_mut::<Transform>(b.dust).unwrap();
    t.scale = Vec3::splat(1.0 + 1.6 * k.sqrt());
    t.translation = b.at + DVec3::Y * (0.8 + 0.6 * k as f64);
    true
}

#[test]
fn fighters_flush_a_player_out_of_cover_with_a_grenade() {
    let mut app = app();
    let walls = yard(&mut app);
    let squad = squad(&mut app);
    let mut player = Player::spawn(&mut app, true);
    let away = camera(
        &mut app,
        DVec3::new(0.0, 2.0, -300.0),
        DVec3::new(0.0, 2.0, -600.0),
        30.0,
        Vec3::Y,
    );
    // Behind the player and off its right shoulder, over the yard toward the squad.
    let cam_at = DVec3::new(3.2, 3.0, -6.5);
    let cam = camera(&mut app, cam_at, DVec3::new(-1.0, 0.8, 5.0), 62.0, Vec3::Y);
    dress(&mut app, &walls);
    let body = player_body(&mut app);
    use_camera(&mut app, away);

    let mut traces = app
        .world
        .resource::<Messages<ProjectileTrace>>()
        .get_cursor();
    let mut blasts = app.world.resource::<Messages<WorldImpulse>>().get_cursor();
    let mut tracers: Vec<(Entity, f64)> = Vec::new();
    let mut bursts: Vec<Burst> = Vec::new();
    let mut shots: [Picture; MOMENTS] = Default::default();
    let (mut thrown, mut blast_at, mut landed) = (None::<f64>, None::<f64>, None::<DVec3>);
    let mut t = 0.0f64;
    while t < 30.0 && shots.iter().any(Option::is_none) {
        player.step(&mut app, t, &squad);
        app.update();
        t += STEP;
        pose_player_body(&mut app, body, &player);
        let now = app.world.resource::<Time>().elapsed_secs_f64();
        tracers.retain(|&(e, until)| {
            let keep = t < until;
            if !keep {
                app.world.despawn(e);
            }
            keep
        });
        bursts.retain(|b| age_burst(&mut app, b, now));
        let msgs = app.world.resource::<Messages<ProjectileTrace>>();
        let fired: Vec<ProjectileTrace> = traces.read(msgs).copied().collect();
        let (mut pinning, mut at_runner) = (false, false);
        for tr in fired {
            let Some(s) = tr.shooter else { continue };
            if !squad.contains(&s) {
                continue;
            }
            let e = tracer(&mut app, tr.from, tr.to);
            tracers.push((e, t + 0.2));
            pinning |= brain(&app, s).role == "suppress";
            at_runner |= player.fled_at.is_some()
                && engine_npc::perception::segment_distance(
                    at(&app, player.e) + DVec3::Y,
                    tr.from,
                    tr.to,
                ) < 2.0;
        }
        let msgs = app.world.resource::<Messages<WorldImpulse>>();
        let went_off: Vec<WorldImpulse> = blasts.read(msgs).copied().collect();
        for b in went_off {
            blast_at.get_or_insert(t);
            bursts.push(burst(&mut app, b.center - DVec3::Y * 0.3, now));
        }
        let flight = app.world.resource::<Grenades>().flights.first().copied();
        if let Some(f) = flight {
            thrown.get_or_insert(t);
            landed = Some(f.at);
            // Its arc marked as it flies (the grenade itself is a few centimeters across).
            if now - f.thrown <= f.flight_secs {
                dot_at(&mut app, f.position(now), 0.05, 0xfff0c0);
            }
        }
        // The moments: pinned (at a suppressor's shot), the grenade over the wall, the player
        // running from where it lands, the blast, and the player in the open under fire.
        let n = shots.iter().position(Option::is_none).unwrap();
        let due = match n {
            0 => pinning && t >= 3.0,
            1 => flight.is_some_and(|f| now - f.thrown >= f.flight_secs * 0.55),
            2 => player.fled_at.is_some_and(|f| t >= f + 0.6),
            3 => blast_at.is_some_and(|b| t - b < 0.05),
            _ => blast_at.is_some_and(|b| (at_runner && t > b + 0.3) || t >= b + 2.5),
        };
        if !due {
            continue;
        }
        let caption = match n {
            0 => format!("{t:.1} S: PINNED BEHIND THE WALL"),
            1 => format!("{t:.1} S: A GRENADE COMES OVER"),
            2 => format!("{t:.1} S: IT LANDS, HE RUNS"),
            3 => format!("{t:.1} S: THE BLAST"),
            _ => format!("{t:.1} S: IN THE OPEN, UNDER FIRE"),
        };
        // On the player and the grenade (or where it went off).
        let p = at(&app, player.e) + DVec3::Y * 0.9;
        let look = match (n, flight, landed) {
            (1, Some(f), _) => (p + f.position(now)) * 0.5,
            (2..=4, _, Some(l)) => (p + l) * 0.5 + DVec3::new(0.0, 0.0, 2.0),
            _ => DVec3::new(-1.0, 0.8, 5.0),
        };
        *app.world.get_mut::<Transform>(cam).unwrap() =
            Transform::from_translation(cam_at).looking_at(look, Vec3::Y);
        let img = shoot(&mut app, cam, away, &format!("flush_{n}.png"));
        shots[n] = Some((caption, img));
    }
    let thrown = thrown.expect("no grenade was thrown");
    let fled = player.fled_at.expect("the player never ran");
    eprintln!("thrown at {thrown:.1} s, the player ran at {fled:.1} s, went off at {blast_at:?}");
    assert!(thrown >= 7.5, "thrown at {thrown:.1} s");
    assert!(shots.iter().all(Option::is_some), "a moment never came");
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping the pictures: no GPU");
        return;
    }
    strip(&shots, "fighters-flush-grenade.png");
}

/// Put the pictures side by side under their captions and save the strip as `name`.
fn strip(shots: &[Picture], name: &str) {
    let mut img = image::RgbaImage::from_pixel(
        TILE_W * shots.len() as u32,
        TILE_H + BAND,
        image::Rgba([24, 24, 28, 255]),
    );
    for (n, shot) in shots.iter().enumerate() {
        let (caption, pic) = shot.as_ref().expect("a picture was never taken");
        let x = TILE_W * n as u32;
        if let Some(pic) = pic {
            image::imageops::replace(&mut img, pic, i64::from(x), i64::from(BAND));
        }
        write(&mut img, x + 10, 8, caption);
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    img.save(&path).expect("save the strip");
    if let Ok(dir) = std::env::var("NPC_SCREENSHOT_DIR") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&path, std::path::Path::new(&dir).join(name)).expect("copy the strip");
    }
    eprintln!("wrote {}", path.display());
}

/// Write `text` (capitals, digits and a little punctuation) in a 5×7 pixel font at twice its
/// size, from its top left at (`x`, `y`).
fn write(img: &mut image::RgbaImage, x: u32, y: u32, text: &str) {
    let ink = image::Rgba([240, 236, 220, 255]);
    for (i, c) in text.chars().enumerate() {
        for (row, bits) in glyph(c).into_iter().enumerate() {
            for col in 0..5 {
                if bits & (0b10000 >> col) == 0 {
                    continue;
                }
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let px = x + i as u32 * 12 + col * 2 + dx;
                    let py = y + row as u32 * 2 + dy;
                    if px < img.width() && py < img.height() {
                        img.put_pixel(px, py, ink);
                    }
                }
            }
        }
    }
}

/// A character's rows, top first, its five columns as the low five bits (leftmost highest).
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0c, 0x04, 0x08],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        _ => [0; 7],
    }
}
