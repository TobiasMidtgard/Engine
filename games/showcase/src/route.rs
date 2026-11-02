//! `--route`: the free-flight route for streaming checks (playtest 2026-11-01 items 28, 30, 1).
//! The free camera flies it on its own controls, at its own speeds: from the village up to
//! 2 km, a cruise out, straight up to orbit and back down, home again, then down to 100 m for a
//! low pass toward the lake. Every frame is logged to `screenshots/route/route.csv` (time, leg,
//! altitude, speed, frame and render times, the terrain's patches, uploads, RAM, exposure);
//! at the end the log lists each leg's frames, the hitches (frames over 33 ms, with their
//! slowest render steps) and RAM. A picture is saved at the end of each leg and at a few
//! heights on the way up, to catch a black screen high up.
//!
//! Headless runs step time by 1/60 s, so the camera flies as it would at 60 fps and the
//! per-frame costs are comparable; in a window it flies in real time.

use crate::camera::{self, ShowcaseCamera};
use crate::places;
use engine::core::diagnostics::{Diagnostics, PROCESS_RAM_MB};
use engine::prelude::*;
use std::fmt::Write as _;

/// A frame longer than this is a hitch (two 60 Hz frames).
const HITCH_MS: f64 = 33.0;
/// Heights on the way up where a picture is saved (m).
const SHOTS_UP_M: [f64; 4] = [20_000.0, 60_000.0, 150_000.0, 300_000.0];

#[derive(Clone, Copy)]
enum Do {
    /// Hold still for this long (s).
    Wait(f32),
    /// Turn: pitch (rad), and yaw added (rad).
    Face { pitch: f32, turn: f32 },
    /// Climb (`ascend` 1) or sink (−1) until this height (m), moving `forward` (0..1) too.
    Rise {
        to_m: f64,
        ascend: f32,
        forward: f32,
        sprint: bool,
    },
    /// Fly forward for this long (s).
    Fly { secs: f32, sprint: bool },
}

const LEGS: [(&str, Do); 13] = [
    ("village", Do::Wait(8.0)),
    (
        "face up",
        Do::Face {
            pitch: 0.15,
            turn: 0.0,
        },
    ),
    (
        "climb to 2 km",
        Do::Rise {
            to_m: 2_000.0,
            ascend: 1.0,
            forward: 1.0,
            sprint: false,
        },
    ),
    (
        "cruise at 2 km",
        Do::Fly {
            secs: 12.0,
            sprint: false,
        },
    ),
    (
        "face down",
        Do::Face {
            pitch: -0.6,
            turn: 0.0,
        },
    ),
    (
        "climb to orbit",
        Do::Rise {
            to_m: 400_000.0,
            ascend: 1.0,
            forward: 0.0,
            sprint: false,
        },
    ),
    ("orbit", Do::Wait(6.0)),
    (
        "descend to 2 km",
        Do::Rise {
            to_m: 2_000.0,
            ascend: -1.0,
            forward: 0.0,
            sprint: false,
        },
    ),
    (
        "turn home",
        Do::Face {
            pitch: 0.0,
            turn: std::f32::consts::PI,
        },
    ),
    (
        "cruise home",
        Do::Fly {
            secs: 12.0,
            sprint: false,
        },
    ),
    (
        "down to 100 m",
        Do::Rise {
            to_m: 100.0,
            ascend: -1.0,
            forward: 0.3,
            sprint: false,
        },
    ),
    (
        "low pass",
        Do::Fly {
            secs: 10.0,
            sprint: true,
        },
    ),
    ("end", Do::Wait(2.0)),
];

struct Row {
    t: f64,
    leg: usize,
    altitude_m: f64,
    speed: f64,
    frame_ms: f64,
    render_cpu_ms: f64,
    terrain_ms: f64,
    missing: f64,
    building: f64,
    built: f64,
    resident: f64,
    upload_mb: f64,
    ram_mb: f64,
    ev100: f64,
    /// The frame's slowest render steps, for the hitch list.
    slowest: String,
}

#[derive(Resource)]
struct Route {
    out_dir: String,
    leg: usize,
    /// Time in the current leg (s).
    in_leg: f32,
    t: f64,
    last_pos: Option<DVec3>,
    shots_up: usize,
    rows: Vec<Row>,
    started: bool,
    done_frames: u32,
}

pub fn install(app: &mut App) {
    if app.world.resource::<EngineConfig>().headless {
        app.world
            .resource_mut::<Time>()
            .set_source(engine::core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
    }
    let out_dir = "screenshots/route".to_string();
    let _ = std::fs::create_dir_all(&out_dir);
    app.insert_resource(Route {
        out_dir,
        leg: 0,
        in_leg: 0.0,
        t: 0.0,
        last_pos: None,
        shots_up: 0,
        rows: Vec::new(),
        started: false,
        done_frames: 0,
    });
    app.add_systems(Update, run.before(camera::fly));
}

fn run(world: &mut World) {
    if world.get_resource::<crate::build::WorldBuild>().is_some() {
        return;
    }
    if !world.resource::<Route>().started {
        let plaza = places::find("plaza").expect("plaza");
        camera::go_to(world, &plaza);
        world.resource_mut::<Route>().started = true;
        return;
    }
    let dt = world.resource::<Time>().delta_secs();
    let real_ms = world.resource::<Time>().real_delta_secs() as f64 * 1000.0;
    let (pos, altitude) = {
        let mut q = world.query::<(&Transform, &ShowcaseCamera)>();
        let Some((t, cam)) = q.iter(world).next() else {
            return;
        };
        let gravity = world.resource::<Gravity>();
        let ground = world.resource::<engine::world::Ground>();
        (
            t.translation,
            camera::altitude(gravity, ground, cam.level, t.translation),
        )
    };
    record(world, pos, altitude, dt, real_ms);

    let (leg, in_leg) = {
        let r = world.resource::<Route>();
        (r.leg, r.in_leg)
    };
    let Some(&(name, step)) = LEGS.get(leg) else {
        finish(world);
        return;
    };
    let mut done = false;
    {
        let mut actions = world.resource_mut::<ActionState>();
        actions.release_all_overrides();
        match step {
            Do::Wait(secs) => done = in_leg >= secs,
            Do::Face { .. } => {}
            Do::Rise {
                to_m,
                ascend,
                forward,
                sprint,
            } => {
                done = if ascend > 0.0 {
                    altitude >= to_m
                } else {
                    altitude <= to_m
                };
                if !done {
                    actions.set_axis("ascend", Vec2::new(ascend, 0.0), None);
                    if forward > 0.0 {
                        actions.set_axis("move", Vec2::new(0.0, forward), None);
                    }
                    if sprint {
                        actions.press("sprint");
                    }
                }
            }
            Do::Fly { secs, sprint } => {
                done = in_leg >= secs;
                if !done {
                    actions.set_axis("move", Vec2::new(0.0, 1.0), None);
                    if sprint {
                        actions.press("sprint");
                    }
                }
            }
        }
    }
    if let Do::Face { pitch, turn } = step {
        let mut q = world.query::<&mut ShowcaseCamera>();
        for mut cam in q.iter_mut(world) {
            cam.pitch = pitch;
            cam.yaw += turn;
        }
        done = true;
    }
    // Pictures on the way up, and at each leg's end.
    let shot = {
        let mut r = world.resource_mut::<Route>();
        let mut shot = None;
        if matches!(step, Do::Rise { ascend, .. } if ascend > 0.0)
            && r.shots_up < SHOTS_UP_M.len()
            && altitude >= SHOTS_UP_M[r.shots_up]
        {
            shot = Some(format!(
                "{}/up_{:.0}km.png",
                r.out_dir,
                SHOTS_UP_M[r.shots_up] / 1000.0
            ));
            r.shots_up += 1;
        }
        if done {
            if !matches!(step, Do::Face { .. }) {
                shot = Some(format!(
                    "{}/{:02}_{}.png",
                    r.out_dir,
                    leg + 1,
                    name.replace(' ', "_")
                ));
            }
            engine::core::log::info!("route: {name} done at {:.1} s, {:.0} m up", r.t, altitude);
            r.leg += 1;
            r.in_leg = 0.0;
        } else {
            r.in_leg += dt;
        }
        shot
    };
    if let Some(path) = shot {
        world.resource_mut::<Screenshots>().request(path);
    }
}

fn record(world: &mut World, pos: DVec3, altitude: f64, dt: f32, real_ms: f64) {
    let d = world.resource::<Diagnostics>();
    let get = |k: &str| d.get(k).unwrap_or(f64::NAN);
    let mut steps: Vec<(&str, f64)> = d
        .metrics
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("render.cpu.").map(|s| (s, *v)))
        .collect();
    let terrain_ms: f64 = steps
        .iter()
        .filter(|(s, _)| s.contains("terrain"))
        .map(|(_, v)| v)
        .sum();
    steps.sort_by(|a, b| b.1.total_cmp(&a.1));
    let slowest = steps
        .iter()
        .take(4)
        .map(|(s, v)| format!("{s} {v:.1}"))
        .collect::<Vec<_>>()
        .join(", ");
    let row = Row {
        t: 0.0,
        leg: 0,
        altitude_m: altitude,
        speed: 0.0,
        frame_ms: real_ms,
        render_cpu_ms: get("render.cpu_ms"),
        terrain_ms,
        missing: get("terrain.patches_missing"),
        building: get("terrain.patches_building"),
        built: get("terrain.patches_built"),
        resident: get("terrain.patches_resident"),
        upload_mb: get("render.upload_mb"),
        ram_mb: get(PROCESS_RAM_MB),
        ev100: get("render.ev100"),
        slowest,
    };
    let mut r = world.resource_mut::<Route>();
    // Speed over the ground frame (a world shift's jump reads as one fast frame: skipped).
    let speed = r
        .last_pos
        .map_or(0.0, |p| (pos - p).length() / dt.max(1e-6) as f64);
    r.last_pos = Some(pos);
    r.t += dt as f64;
    let (t, leg) = (r.t, r.leg.min(LEGS.len() - 1));
    r.rows.push(Row {
        t,
        leg,
        speed,
        ..row
    });
}

fn finish(world: &mut World) {
    let frames = {
        let mut r = world.resource_mut::<Route>();
        r.done_frames += 1;
        r.done_frames
    };
    // Give the last picture a few frames to be written.
    if frames < 4 {
        return;
    }
    let r = world.resource::<Route>();
    let mut csv = String::from(
        "t_s,leg,altitude_m,speed_mps,frame_ms,render_cpu_ms,terrain_ms,patches_missing,\
         patches_building,patches_built,patches_resident,upload_mb,ram_mb,ev100\n",
    );
    for x in &r.rows {
        let _ = writeln!(
            csv,
            "{:.3},{},{:.0},{:.0},{:.2},{:.2},{:.2},{},{},{},{},{:.2},{:.0},{:.2}",
            x.t,
            LEGS[x.leg].0,
            x.altitude_m,
            x.speed,
            x.frame_ms,
            x.render_cpu_ms,
            x.terrain_ms,
            x.missing,
            x.building,
            x.built,
            x.resident,
            x.upload_mb,
            x.ram_mb,
            x.ev100
        );
    }
    let path = format!("{}/route.csv", r.out_dir);
    if let Err(e) = std::fs::write(&path, csv) {
        engine::core::log::warn!("route: can't write {path}: {e}");
    }
    // Each leg: frames, frame times, hitches, the terrain's worst, RAM.
    let mut report = String::from("route summary\n");
    for (i, (name, _)) in LEGS.iter().enumerate() {
        let rows: Vec<&Row> = r.rows.iter().filter(|x| x.leg == i).collect();
        if rows.is_empty() {
            continue;
        }
        let ms: Vec<f64> = rows.iter().map(|x| x.frame_ms).collect();
        let mean = ms.iter().sum::<f64>() / ms.len() as f64;
        let max = ms.iter().cloned().fold(0.0, f64::max);
        let hitches = ms.iter().filter(|&&m| m > HITCH_MS).count();
        let terrain = rows.iter().map(|x| x.terrain_ms).fold(0.0, f64::max);
        let missing = rows.iter().map(|x| x.missing).fold(0.0, f64::max);
        let ram = rows.iter().map(|x| x.ram_mb).fold(0.0, f64::max);
        let _ = writeln!(
            report,
            "  {name}: {} frames, {mean:.1} ms mean, {max:.1} max, {hitches} over {HITCH_MS} ms; \
             terrain prepare max {terrain:.1} ms, patches missing max {missing}, RAM max {ram:.0} MB",
            rows.len()
        );
    }
    let hitches: Vec<&Row> = r.rows.iter().filter(|x| x.frame_ms > HITCH_MS).collect();
    let _ = writeln!(report, "hitches ({}):", hitches.len());
    for x in hitches.iter().take(60) {
        let _ = writeln!(
            report,
            "  {:.2} s {} at {:.0} m, {:.0} m/s: {:.1} ms (render CPU {:.1}; {})",
            x.t, LEGS[x.leg].0, x.altitude_m, x.speed, x.frame_ms, x.render_cpu_ms, x.slowest
        );
    }
    let ram: Vec<f64> = r
        .rows
        .iter()
        .map(|x| x.ram_mb)
        .filter(|v| v.is_finite())
        .collect();
    if let (Some(first), Some(last)) = (ram.first(), ram.last()) {
        let max = ram.iter().cloned().fold(0.0, f64::max);
        let _ = writeln!(
            report,
            "RAM: {first:.0} MB at the start, {max:.0} max, {last:.0} at the end"
        );
    }
    engine::core::log::info!("{report}");
    let _ = std::fs::write(format!("{}/route.txt", r.out_dir), report);
    world.write_message(AppExit::Success);
}
