//! A captioned render strip of spells on minds in a sand yard: a red and a blue squad of
//! swordsmen clash, then fear sends the reds running from the blue caster, a command turns a
//! red against its own squad, a frenzy sets a blue on its mates, and calm makes everyone lower
//! their blades. A glowing disc under each fighter shows the spell on it (fear violet, command
//! gold, frenzy red, calm green); dots mark where the frightened ran. Needs a GPU or lavapipe
//! for the picture (the behaviour is checked either way).
//!
//! The strip lands in the test's temp dir as `fighters-mind-spells.png`; set
//! `NPC_SCREENSHOT_DIR` to also copy it there. Without it nothing is drawn: the scene plays
//! out the same and every check runs.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::physics::Collider;
use engine_world::{MindEffect, MindKind, SimulationFocus, WorldPlugin};

const TILE_W: u32 = 440;
const TILE_H: u32 = 320;
const BAND: u32 = 30;
const STEP: f64 = 1.0 / 60.0;

const FEAR: u32 = 0x9b5cff;
const COMMAND: u32 = 0xf2c94a;
const FRENZY: u32 = 0xff3a30;
const CALM: u32 = 0x4fe0a0;

fn app() -> App {
    engine_npc::art::fighter::kit();
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

fn hold_off_drawing(world: &mut World) {
    if !world.contains_resource::<RenderDisabled>() {
        world.insert_resource(RenderDisabled(
            "pictures only with NPC_SCREENSHOT_DIR".into(),
        ));
    }
}
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

/// Blocks to fight round (center and half size).
fn yard(app: &mut App) -> Vec<(DVec3, Vec3)> {
    let walls = vec![
        (DVec3::new(-7.0, 0.55, 3.0), Vec3::new(1.6, 0.55, 0.25)),
        (DVec3::new(6.5, 0.55, -2.0), Vec3::new(1.6, 0.55, 0.25)),
        (DVec3::new(0.5, 1.1, 9.0), Vec3::new(0.8, 1.1, 0.8)),
    ];
    for &(c, h) in &walls {
        app.world
            .spawn((Transform::from_translation(c), Collider::cuboid(h)));
    }
    walls
}

/// Four swordsmen of a side, in a line `z` m from the middle, facing it.
fn side(app: &mut App, faction: Faction, squad: u32, z: f64, seed: u64) -> Vec<Entity> {
    let skills = [
        NpcSkills::veteran(),
        NpcSkills::elite(),
        NpcSkills::soldier(),
        NpcSkills::veteran(),
    ];
    skills
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let x = (k as f64 - 1.5) * 2.6;
            let mut t = Transform::from_translation(DVec3::new(x, 0.0, z));
            t.look_to(Vec3::new(0.0, 0.0, -z.signum() as f32), Vec3::Y);
            let weapon = if k % 2 == 0 {
                NpcWeapon::sword_and_shield()
            } else {
                NpcWeapon::sword()
            };
            let e = app
                .world
                .spawn((
                    t,
                    Npc::default(),
                    faction,
                    Squad(squad),
                    s,
                    weapon,
                    Seed(hash_u64(seed * 31 + k as u64 + 1)),
                    StandIn {
                        color: None,
                        plain: true,
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

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn cast(app: &mut App, target: Entity, kind: MindKind, secs: f32, source: Option<Entity>) {
    app.world.write_message(MindEffect {
        target,
        kind,
        secs,
        source,
    });
}

/// The spell on `e` now, as its disc's color.
fn aura(app: &App, e: Entity) -> Option<u32> {
    let now = app.world.resource::<Time>().elapsed_secs_f64();
    let m = app.world.get::<Spellbound>(e)?;
    if m.afraid(now) {
        Some(FEAR)
    } else if m.frenzied(now) {
        Some(FRENZY)
    } else if m.commanded(now).is_some() {
        Some(COMMAND)
    } else if m.calm(now) {
        Some(CALM)
    } else {
        None
    }
}

fn hits_on(app: &App, e: Entity) -> u32 {
    app.world.get::<NpcCombat>(e).unwrap().stats.hits_taken
}

fn attacks(app: &App, e: Entity) -> u32 {
    app.world.get::<NpcCombat>(e).unwrap().stats.attacks
}

type Picture = Option<(String, Option<image::RgbaImage>)>;

#[test]
fn spells_turn_a_fight() {
    let mut app = app();
    let walls = yard(&mut app);
    let reds = side(&mut app, Faction::ENEMY, 11, 7.0, 1);
    let blues = side(&mut app, Faction::RIVAL, 12, -7.0, 2);
    // Where the picture looks from (the simulation runs near it).
    app.world.spawn((
        SimulationFocus::default(),
        Transform::from_translation(DVec3::new(0.0, 2.0, -16.0)),
    ));
    let away = camera(
        &mut app,
        DVec3::new(0.0, 2.0, -300.0),
        DVec3::new(0.0, 2.0, -600.0),
        30.0,
        Vec3::Y,
    );
    let view = camera(
        &mut app,
        DVec3::new(-3.0, 9.5, -17.0),
        DVec3::new(0.0, 0.0, 2.0),
        50.0,
        Vec3::Y,
    );
    dress(&mut app, &walls);
    use_camera(&mut app, away);

    let caster = blues[1];
    let turned = reds[2];
    let mad = blues[3];
    let mut trail: Vec<DVec3> = Vec::new();
    let mut shots: Vec<Picture> = Vec::new();
    let mut fled = (0.0f64, 0.0f64);
    let mut before = (0u32, 0u32, 0u32);
    let mut calm_attacks = None::<u32>;
    let mut t = 0.0f64;
    let moments = [4.5, 7.5, 13.0, 16.5, 21.0];
    let mut next = 0;
    while next < moments.len() {
        app.update();
        t += STEP;
        let step = (t / STEP).round() as u64;
        // Spells, as a caster would throw them.
        let from_caster = |app: &App| {
            let c = at(app, caster);
            reds.iter().map(|&r| at(app, r).distance(c)).sum::<f64>() / reds.len() as f64
        };
        if step == (5.0 / STEP) as u64 {
            fled.0 = from_caster(&app);
            for &r in &reds {
                cast(&mut app, r, MindKind::Fear, 3.5, Some(caster));
            }
        }
        if step == (10.0 / STEP) as u64 {
            before.0 = reds
                .iter()
                .filter(|&&r| r != turned)
                .map(|&r| hits_on(&app, r))
                .sum();
            cast(
                &mut app,
                turned,
                MindKind::Command {
                    side: Faction::RIVAL.0,
                },
                9.0,
                Some(caster),
            );
        }
        if step == (13.5 / STEP) as u64 {
            before.1 = blues
                .iter()
                .filter(|&&b| b != mad)
                .map(|&b| hits_on(&app, b))
                .sum();
            cast(&mut app, mad, MindKind::Frenzy, 6.0, None);
        }
        if step == (18.0 / STEP) as u64 {
            for &e in reds.iter().chain(&blues) {
                cast(&mut app, e, MindKind::Calm, 10.0, None);
            }
        }
        if step == (19.0 / STEP) as u64 {
            calm_attacks = Some(reds.iter().chain(&blues).map(|&e| attacks(&app, e)).sum());
        }
        // Where the frightened run, every quarter second.
        if (5.0..9.0).contains(&t) && step.is_multiple_of(15) {
            trail.extend(reds.iter().map(|&r| at(&app, r)));
        }
        if step == (8.4 / STEP) as u64 {
            fled.1 = from_caster(&app);
        }
        if t < moments[next] {
            continue;
        }
        let caption = match next {
            0 => "BEFORE: THE SQUADS CLASH".to_string(),
            1 => "FEAR: THE REDS RUN".to_string(),
            2 => "COMMAND: A RED FIGHTS FOR BLUE".to_string(),
            3 => "FRENZY: A BLUE HITS ITS OWN".to_string(),
            _ => "CALM: BLADES DOWN".to_string(),
        };
        let mut marks: Vec<Entity> = Vec::new();
        if next == 1 {
            marks.extend(trail.iter().map(|&p| dot(&mut app, p, 0.12, FEAR)));
        }
        for &e in reds.iter().chain(&blues) {
            if let Some(c) = aura(&app, e) {
                let p = at(&app, e);
                marks.push(dot(&mut app, p, 0.65, c));
            }
        }
        // Framed on who the moment is about, from the south-west and above.
        let who: Vec<Entity> = match next {
            1 => reds.iter().copied().chain([caster]).collect(),
            2 => reds.clone(),
            3 => blues.clone(),
            _ => reds.iter().chain(&blues).copied().collect(),
        };
        let center = who.iter().map(|&e| at(&app, e)).sum::<DVec3>() / who.len() as f64;
        let spread = who
            .iter()
            .map(|&e| at(&app, e).distance(center))
            .fold(0.0, f64::max);
        let dist = (spread * 2.0 + 4.5).max(7.5);
        let eye = center + DVec3::new(-0.35, 0.62, -1.0).normalize() * dist;
        *app.world.get_mut::<Transform>(view).unwrap() =
            Transform::from_translation(eye).looking_at(center + DVec3::Y * 0.6, Vec3::Y);
        let img = shoot(&mut app, view, away, &format!("mind_spells_{next}.png"));
        for e in marks {
            app.world.despawn(e);
        }
        shots.push(Some((format!("{t:.0} S {caption}"), img)));
        next += 1;
    }

    // Fear: the reds ran from the caster.
    eprintln!("the reds from the caster: {:.1} -> {:.1} m", fled.0, fled.1);
    assert!(fled.1 > fled.0 + 8.0, "the reds didn't run: {fled:?}");
    // Command: the turned red struck its own squad.
    let red_hits: u32 = reds
        .iter()
        .filter(|&&r| r != turned)
        .map(|&r| hits_on(&app, r))
        .sum();
    assert!(attacks(&app, turned) > 0);
    assert!(
        red_hits > before.0,
        "no red was struck while one was turned"
    );
    // Frenzy: a blue struck by someone after the frenzy (its mates, if it reached them first).
    let blue_hits: u32 = blues
        .iter()
        .filter(|&&b| b != mad)
        .map(|&b| hits_on(&app, b))
        .sum();
    assert!(blue_hits > before.1, "no blue was struck in the frenzy");
    // Calm: from a second after it, nobody swings.
    let now: u32 = reds.iter().chain(&blues).map(|&e| attacks(&app, e)).sum();
    assert_eq!(Some(now), calm_attacks, "someone swung while calmed");

    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping the pictures: no GPU");
        return;
    }
    strip(&shots, "fighters-mind-spells.png");
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
