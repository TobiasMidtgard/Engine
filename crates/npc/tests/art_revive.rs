//! A captioned render strip of a downed fighter, before and after squad revives: whole NPCs
//! with their AI. Before (the old showcase timer, nobody coming): it lies there six seconds
//! and is sent home whole. After: a squad mate runs in, kneels beside it, sets its carbine
//! down, puts its hands on the chest and looks up at the enemy now and then, and the ally gets
//! up where it lay. Needs a GPU or a software Vulkan driver (lavapipe) for the pictures (the
//! behaviour is checked either way).
//!
//! A close-up strip follows one reviver from two sides: getting down, its hands on the chest
//! (the carbine set down clear of what the mate let go of), and looking up.
//!
//! The strips land in the test's temp dir as `fighters-revive-before-after.png` and
//! `fighters-revive-kneel-closeup.png`; set `NPC_SCREENSHOT_DIR` to also copy them there.
//! Without it nothing is drawn (drawing is costly without a GPU): the scenes play out the
//! same and every check runs.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::visuals::{Part, StandInPart};
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::locomotion::LocomotionState;
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

/// One picture of the strip, and the band over it for its caption.
const TILE_W: u32 = 400;
const TILE_H: u32 = 340;
const BAND: u32 = 30;
/// The simulation step between pictures (s).
const STEP: f64 = 1.0 / 30.0;

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
    // The showcase timer the demos use.
    app.world.resource_mut::<NpcSettings>().revive_secs = 6.0;
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.6, -0.6, 0.55), Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x8c7a5b).with_roughness(0.95),
        ),
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

/// A fighter with a carbine at `at`, facing −Z (in squad 4 when `squad`).
fn fighter(app: &mut App, at: DVec3, squad: bool, skills: NpcSkills, home: DVec3) -> Entity {
    let mut t = Transform::from_translation(at);
    t.look_to(Vec3::NEG_Z, Vec3::Y);
    let mut e = app.world.spawn((
        t,
        Npc {
            home: Some(home),
            ..Default::default()
        },
        Faction::ENEMY,
        skills,
        NpcWeapon::rifle(),
        StandIn {
            color: None,
            plain: false,
        },
    ));
    if squad {
        e.insert(Squad(4));
    }
    e.id()
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

fn hit(app: &mut App, e: Entity, amount: f32) {
    let at = app.world.get::<Transform>(e).unwrap().translation;
    app.world.write_message(DamageEvent {
        target: e,
        amount,
        kind: DamageKind::Ballistic,
        point: at + DVec3::Y * 1.2,
        impulse: Vec3::Z * 10.0,
        source: None,
    });
}

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn downed(app: &App, e: Entity) -> bool {
    app.world.get::<NpcVitals>(e).unwrap().downed
}

/// A picture and its caption, once taken (none without a GPU).
type Picture = Option<(String, Option<image::RgbaImage>)>;

/// The pictures by row.
type Pictures = [[Picture; 7]; 2];

/// Where a part of `e`'s stand-in is (world), and which way it faces.
fn part(app: &App, e: Entity, which: Part) -> Option<(DVec3, Vec3)> {
    let owner = *app.world.get::<Transform>(e)?;
    let children: Vec<Entity> = app.world.get::<Children>(e)?.iter().collect();
    children.into_iter().find_map(|c| {
        let p = app.world.get::<StandInPart>(c)?;
        if p.part != which {
            return None;
        }
        let t = app.world.get::<Transform>(c)?;
        let pos = owner.translation + (owner.rotation * t.translation.as_vec3()).as_dvec3();
        Some((pos, owner.rotation * t.rotation * Vec3::NEG_Z))
    })
}

#[test]
fn fighters_revive_before_and_after() {
    let mut app = app();
    // Before: no squad, nobody comes (the old showcase timer: six seconds, then home whole).
    let a0 = DVec3::new(-150.0, 0.0, 0.0);
    let home = a0 + DVec3::new(40.0, 0.0, 30.0);
    let lone = fighter(&mut app, a0, false, NpcSkills::soldier(), home);
    let by = a0 + DVec3::new(-3.0, 0.0, 2.5);
    fighter(&mut app, by, false, NpcSkills::rookie(), by);
    // After: a rookie squad mate (it runs straight in, though the enemy sees the spot).
    let b0 = DVec3::new(150.0, 0.0, 0.0);
    let ally = fighter(&mut app, b0, true, NpcSkills::soldier(), b0);
    let mate_at = b0 + DVec3::new(-4.0, 0.0, 2.5);
    let mate = fighter(&mut app, mate_at, true, NpcSkills::rookie(), mate_at);
    let mut enemy = Transform::from_translation(b0 + DVec3::new(4.0, 0.0, -16.0));
    enemy.look_to(Vec3::Z, Vec3::Y);
    app.world
        .spawn((enemy, Faction::PLAYER, LocomotionState::default()));
    let cam_home = camera(
        &mut app,
        home + DVec3::new(2.4, 1.6, -2.6),
        home + DVec3::new(0.0, 0.9, 0.0),
        40.0,
    );
    let away = camera(
        &mut app,
        DVec3::new(0.0, 2.0, 300.0),
        DVec3::new(0.0, 2.0, 600.0),
        30.0,
    );
    use_camera(&mut app, away);
    // They see the enemy first, then both go down (and land).
    app.update_n((1.5 / STEP) as usize);
    hit(&mut app, lone, 1000.0);
    hit(&mut app, ally, 1000.0);
    let mut t = 0.0f64;
    while t < 0.5 {
        app.update();
        t += STEP;
    }
    assert!(downed(&app, lone) && downed(&app, ally));
    let (lies_a, lies_b) = (at(&app, lone), at(&app, ally));
    // The cameras: on each fallen fighter's chest (it falls back along +Z) from the front and
    // its right, and on the home the lone one is sent to.
    let chest_of = |app: &App, e: Entity| {
        part(app, e, Part::Torso).map_or(at(app, e) + DVec3::new(0.0, 0.3, 0.7), |(p, _)| p)
    };
    let (chest_a, chest_b) = (chest_of(&app, lone), chest_of(&app, ally));
    let view = |app: &mut App, chest: DVec3| {
        camera(
            app,
            chest + DVec3::new(2.3, 1.3, -1.9),
            chest + DVec3::new(-0.35, 0.15, 0.05),
            40.0,
        )
    };
    let cam_a = view(&mut app, chest_a);
    let cam_b = view(&mut app, chest_b);

    let mut pictures: Pictures = Default::default();
    let shot = |app: &mut App, pictures: &mut Pictures, (row, n): (usize, usize), cam, caption| {
        let name = format!("revive_{row}_{n}.png");
        let img = shoot(app, cam, away, &name);
        pictures[row][n] = Some((caption, img));
    };
    let (mut up_at, mut glanced, mut set_down, mut sent_home) = (None, false, false, false);
    let (mut up_from, mut up_health, mut home_at) = (DVec3::ZERO, 0.0, None);
    // When the mate's current kneel began, and when its hands went on the chest in it (a
    // revive it let go of starts over).
    let (mut knelt_at, mut hands_at, mut first_knelt) = (None, None, None);
    let before = [0.6, 1.5, 3.0, 4.5, 5.9];
    while t < 14.0 {
        app.update();
        t += STEP;
        // Before: lying there until the timer sends it home.
        for (n, &when) in before.iter().enumerate() {
            if pictures[0][n].is_none() && t >= when {
                assert!(downed(&app, lone), "the lone fighter got up at {t:.1} s");
                let what = match n {
                    0 => "BEFORE: SHOT DOWN".to_string(),
                    1 => format!("{when:.1} S: A MATE STANDS BY"),
                    _ => format!("{when:.1} S: STILL DOWN"),
                };
                shot(&mut app, &mut pictures, (0, n), cam_a, what);
            }
        }
        if home_at.is_none() && !downed(&app, lone) {
            home_at = Some(t);
            sent_home = at(&app, lone).distance(home) < 0.5;
            assert!(t >= 5.95, "the lone fighter was up after {t:.1} s");
        }
        if let Some(gone) = home_at {
            if pictures[0][5].is_none() && t >= gone + 0.3 {
                let what = format!("{t:.1} S: GONE FROM THE SPOT");
                shot(&mut app, &mut pictures, (0, 5), cam_a, what);
            }
            if pictures[0][6].is_none() && t >= gone + 1.8 {
                let what = "SENT HOME, WHOLE".to_string();
                shot(&mut app, &mut pictures, (0, 6), cam_home, what);
            }
        }
        // After: the mate runs in, kneels, sets the carbine down, hands on, looks up; the ally
        // gets up where it lay.
        let (action, knelt, progress) = {
            let b = app.world.get::<NpcBrain>(mate).unwrap();
            (b.action, b.revive_kneel, b.revive_progress)
        };
        let near = at(&app, mate).distance(chest_b);
        let kneeling = action == NpcAction::Revive && knelt > 0.0;
        // (Up to the frame the ally gets up, the mate standing from it the same frame.)
        if downed(&app, ally) {
            if !kneeling {
                (knelt_at, hands_at) = (None, None);
            } else {
                if knelt_at.is_none() {
                    knelt_at = Some(t);
                    first_knelt.get_or_insert(t);
                }
                if progress > 0.0 && hands_at.is_none() {
                    hands_at = Some(t);
                }
            }
        }
        if pictures[1][0].is_none() && t >= 0.6 {
            shot(
                &mut app,
                &mut pictures,
                (1, 0),
                cam_b,
                "AFTER: SHOT DOWN".into(),
            );
        }
        if pictures[1][1].is_none() && action == NpcAction::Revive && !kneeling && near < 2.4 {
            shot(
                &mut app,
                &mut pictures,
                (1, 1),
                cam_b,
                format!("{t:.1} S: A MATE RUNS IN"),
            );
        }
        if pictures[1][2].is_none() && kneeling && knelt >= 0.75 {
            shot(
                &mut app,
                &mut pictures,
                (1, 2),
                cam_b,
                format!("{t:.1} S: KNEELS, SETS IT DOWN"),
            );
        }
        let head = part(&app, mate, Part::Head);
        let looks_up = head.is_some_and(|(_, f)| f.y > -0.2);
        if kneeling && progress >= 0.3 {
            // The carbine on the ground beside it.
            if let Some((w, _)) = part(&app, mate, Part::Weapon) {
                set_down |= w.y < 0.12;
            }
            if pictures[1][3].is_none() && !looks_up {
                shot(
                    &mut app,
                    &mut pictures,
                    (1, 3),
                    cam_b,
                    format!("{t:.1} S: HANDS ON THE CHEST"),
                );
            }
            if pictures[1][4].is_none() && looks_up {
                glanced = true;
                shot(
                    &mut app,
                    &mut pictures,
                    (1, 4),
                    cam_b,
                    format!("{t:.1} S: LOOKS UP AT THE ENEMY"),
                );
            }
        }
        if up_at.is_none() && !downed(&app, ally) {
            up_at = Some(t);
            up_from = at(&app, ally);
            up_health = app.world.get::<NpcVitals>(ally).unwrap().fraction();
        }
        if let Some(up) = up_at {
            if pictures[1][5].is_none() && t >= up + 0.6 {
                shot(
                    &mut app,
                    &mut pictures,
                    (1, 5),
                    cam_b,
                    format!("{t:.1} S: UP WHERE IT LAY"),
                );
            }
            if pictures[1][6].is_none() && t >= up + 1.7 {
                shot(
                    &mut app,
                    &mut pictures,
                    (1, 6),
                    cam_b,
                    format!("{t:.1} S: BOTH UP, ARMED"),
                );
            }
        }
        if pictures.iter().flatten().all(Option::is_some) {
            break;
        }
    }
    // The behaviour, before and after.
    assert!(sent_home, "the lone fighter was not sent home");
    assert!(lies_a.distance(home) > 10.0);
    let up = up_at.expect("the squad mate never revived its ally");
    // The mate has to be at it before the 6 s timer would send its ally home (the timer then
    // waits for it), and once it is, it gets down and puts its hands on, and the revive takes
    // as long as the settings say, no longer.
    let first = first_knelt.expect("the squad mate never knelt to revive");
    let knelt = knelt_at.unwrap_or(first);
    let hands = hands_at.expect("the squad mate never put its hands on the chest");
    eprintln!(
        "first knelt at {first:.2} s, last kneel from {knelt:.2} s, hands on from {hands:.2} s, \
         ally up at {up:.2} s"
    );
    assert!(
        first < 6.0,
        "began the revive at {first:.1} s (the timer's 6 s)"
    );
    let settings = app.world.resource::<NpcSettings>();
    let takes = settings.ally_revive_secs as f64 * 1.2;
    assert!(
        up - hands <= takes + 0.5,
        "the revive took {:.1} s from the hands on (the setting's {takes:.1} s)",
        up - hands
    );
    let down = NpcBrain::REVIVE_KNEEL_SECS as f64;
    assert!(
        up - knelt <= down + takes + 0.5,
        "the revive took {:.1} s from the kneel ({down:.1} s getting down, the setting's \
         {takes:.1} s)",
        up - knelt
    );
    assert_eq!(app.world.get::<NpcCombat>(mate).unwrap().stats.revives, 1);
    let moved = up_from.distance(lies_b);
    assert!(
        moved < 0.05,
        "the ally got up {moved:.2} m from where it lay"
    );
    let ally_health = app.world.resource::<NpcSettings>().ally_revive_health;
    assert!(
        (up_health - ally_health).abs() < 0.01,
        "up with {up_health:.2} of its health"
    );
    if part(&app, mate, Part::Head).is_some() {
        assert!(glanced, "the reviver never looked up from the chest");
        assert!(set_down, "the reviver kept its carbine in hand");
    }
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping the pictures: no GPU");
        return;
    }
    // The strip: before over after, each picture captioned.
    let mut strip = image::RgbaImage::from_pixel(
        TILE_W * 7,
        (TILE_H + BAND) * 2,
        image::Rgba([24, 24, 28, 255]),
    );
    for (row, frames) in pictures.iter().enumerate() {
        for (n, frame) in frames.iter().enumerate() {
            let (caption, img) = frame.as_ref().expect("a picture was never taken");
            let (x, y) = (TILE_W * n as u32, (TILE_H + BAND) * row as u32);
            if let Some(img) = img {
                image::imageops::replace(&mut strip, img, i64::from(x), i64::from(y + BAND));
            }
            write(&mut strip, x + 10, y + 8, caption);
        }
    }
    let name = "fighters-revive-before-after.png";
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    strip.save(&path).expect("save the strip");
    if let Ok(dir) = std::env::var("NPC_SCREENSHOT_DIR") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&path, std::path::Path::new(&dir).join(name)).expect("copy the strip");
    }
    eprintln!("wrote {}", path.display());
}

/// Where the middle of `e`'s hands is (world), if it has a stand-in.
fn hands(app: &App, e: Entity) -> Option<DVec3> {
    let l = part(app, e, Part::Fighter(7))?.0;
    let r = part(app, e, Part::Fighter(8))?.0;
    Some((l + r) * 0.5)
}

#[test]
fn fighters_revive_kneel_closeup() {
    let mut app = app();
    // A downed ally, a rookie squad mate (it goes in at once), the enemy up the field.
    let a0 = DVec3::new(0.0, 0.0, 0.0);
    let ally = fighter(&mut app, a0, true, NpcSkills::soldier(), a0);
    let mate_at = a0 + DVec3::new(-4.0, 0.0, 2.5);
    let mate = fighter(&mut app, mate_at, true, NpcSkills::rookie(), mate_at);
    let mut enemy = Transform::from_translation(a0 + DVec3::new(5.0, 0.0, -16.0));
    enemy.look_to(Vec3::Z, Vec3::Y);
    app.world
        .spawn((enemy, Faction::PLAYER, LocomotionState::default()));
    let away = camera(
        &mut app,
        DVec3::new(0.0, 2.0, 300.0),
        DVec3::new(0.0, 2.0, 600.0),
        30.0,
    );
    use_camera(&mut app, away);
    app.update_n((1.5 / STEP) as usize);
    hit(&mut app, ally, 1000.0);
    // Two sides of it, set once it starts to get down: from its right, and from across the
    // mate, looking at it over the chest.
    let mut cams: Option<[Entity; 2]> = None;
    let mut pictures: [[Picture; 3]; 2] = Default::default();
    let (mut t, mut clear, mut on_chest) = (0.0f64, f64::MAX, f64::MAX);
    while t < 14.0 {
        app.update();
        t += STEP;
        let (action, knelt, progress) = {
            let b = app.world.get::<NpcBrain>(mate).unwrap();
            (b.action, b.revive_kneel, b.revive_progress)
        };
        if action != NpcAction::Revive || knelt <= 0.0 {
            continue;
        }
        let chest = part(&app, ally, Part::Torso).map_or(at(&app, ally), |(p, _)| p);
        let [side, across] = *cams.get_or_insert_with(|| {
            let me = at(&app, mate);
            let d = DVec3::new(chest.x - me.x, 0.0, chest.z - me.z).normalize();
            let right = DVec3::new(-d.z, 0.0, d.x);
            let mid = (me + chest) * 0.5 + DVec3::Y * 0.3;
            [
                camera(
                    &mut app,
                    mid + right * 2.1 + DVec3::Y * 0.9 - d * 0.4,
                    mid,
                    36.0,
                ),
                camera(
                    &mut app,
                    chest + d * 1.9 + right * 0.7 + DVec3::Y * 1.2,
                    mid + DVec3::Y * 0.1,
                    36.0,
                ),
            ]
        });
        let looks_up = part(&app, mate, Part::Head).is_some_and(|(_, f)| f.y > -0.2);
        if progress > 0.2 {
            // The carbine on the ground clear of the one the mate let go of, the hands on the
            // chest.
            if let (Some((w, _)), Some((theirs, _))) = (
                part(&app, mate, Part::Weapon),
                part(&app, ally, Part::Weapon),
            ) {
                assert!(w.y < 0.12, "the carbine at {:.2} m", w.y);
                clear = clear.min(w.distance(theirs));
            }
            if let Some(h) = hands(&app, mate) {
                on_chest = on_chest.min(h.distance(chest));
            }
        }
        let mut take = |row: usize, n: usize, cam: Entity, caption: String| {
            if pictures[row][n].is_none() {
                let img = shoot(&mut app, cam, away, &format!("closeup_{row}_{n}.png"));
                pictures[row][n] = Some((caption, img));
            }
        };
        let (from_side, from_across) =
            ("FROM ITS RIGHT".to_string(), "ACROSS THE MATE".to_string());
        if knelt >= 0.7 && progress <= 0.0 {
            take(0, 0, side, format!("{from_side}: SETS IT DOWN"));
            take(1, 0, across, format!("{from_across}: SETS IT DOWN"));
        }
        if progress >= 0.4 && !looks_up {
            take(0, 1, side, "HANDS ON THE CHEST".into());
            take(1, 1, across, "HANDS ON THE CHEST".into());
        }
        if progress > 0.0 && looks_up {
            take(0, 2, side, "LOOKS UP AT THE ENEMY".into());
            take(1, 2, across, "LOOKS UP AT THE ENEMY".into());
        }
        if pictures.iter().flatten().all(Option::is_some) || !downed(&app, ally) {
            break;
        }
    }
    assert!(cams.is_some(), "the mate never knelt");
    if part(&app, mate, Part::Head).is_some() {
        eprintln!("the carbines {clear:.2} m apart, the hands {on_chest:.2} m off the chest");
        assert!(clear > 0.3, "the carbines {clear:.2} m apart");
        assert!(on_chest < 0.3, "the hands {on_chest:.2} m off the chest");
    }
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping the pictures: no GPU");
        return;
    }
    let mut strip = image::RgbaImage::from_pixel(
        TILE_W * 3,
        (TILE_H + BAND) * 2,
        image::Rgba([24, 24, 28, 255]),
    );
    for (row, frames) in pictures.iter().enumerate() {
        for (n, frame) in frames.iter().enumerate() {
            let (caption, img) = frame.as_ref().expect("a picture was never taken");
            let (x, y) = (TILE_W * n as u32, (TILE_H + BAND) * row as u32);
            if let Some(img) = img {
                image::imageops::replace(&mut strip, img, i64::from(x), i64::from(y + BAND));
            }
            write(&mut strip, x + 10, y + 8, caption);
        }
    }
    let name = "fighters-revive-kneel-closeup.png";
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    strip.save(&path).expect("save the strip");
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
