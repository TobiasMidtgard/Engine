//! Films the movement in the climbing quarter: scripted controller input played through the
//! real play mode (the pilot, `PlayerControlled`, and its follow camera), rendered headless at
//! 20 frames a second and encoded to MP4 and GIF with ffmpeg.
//!
//! ```text
//! cargo run -p engine_movement --release --example parkour_film -- [out_dir] [clip ...]
//! ```
//!
//! Each clip starts a fresh, deterministic world (fixed 60 Hz steps on a manual clock), puts the
//! pilot at its start, and feeds the `move`, `jump` and `crouch` actions from its script each
//! step, the way a gamepad would. `before` clips run the same input with the movement's
//! pre-2026-11-05 settings (world gravity, the same height for a tap and a hold, no steering
//! in the air), for side-by-side comparisons. Needs a GPU or a software Vulkan driver
//! (lavapipe) and ffmpeg on the path. With `FILM_DRY=1` nothing is drawn: the pilot's mode and
//! position are printed every quarter second instead, to tune a script.

use std::path::{Path, PathBuf};
use std::process::Command;

use engine_core::math::{DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_input::{ActionState, InputPlugin};
use engine_movement::city;
use engine_movement::look::LookSettings;
use engine_movement::{MovementPlugin, MovementProfile, Mover, PilotCamera, demo, play};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;
const DT: f64 = 1.0 / 60.0;
/// Frames simulated before a clip starts (the look bakes, the pilot settles).
const SETTLE_FRAMES: u32 = 40;

/// The controls this step.
#[derive(Debug, Clone, Copy, Default)]
struct Pad {
    /// Where the stick points, in the world (horizontal; length up to 1).
    stick: Vec3,
    jump: bool,
    crouch: bool,
    /// Sprint held (a player runs a parkour line with it held).
    sprint: bool,
    attack: bool,
}

fn go(x: f32, z: f32) -> Pad {
    Pad {
        stick: Vec3::new(x, 0.0, z),
        sprint: true,
        ..Default::default()
    }
}

impl Pad {
    fn jump(mut self) -> Self {
        self.jump = true;
        self
    }
    fn crouch(mut self) -> Self {
        self.crouch = true;
        self
    }
    fn attack(mut self) -> Self {
        self.attack = true;
        self
    }
    /// Without sprint (on a rope: the stick climbs instead of swinging).
    fn calm(mut self) -> Self {
        self.sprint = false;
        self
    }
}

/// Looking along: yaw of the follow camera for a direction (0 = toward −Z).
fn yaw_toward(x: f32, z: f32) -> f32 {
    // The camera looks along rot_y(yaw) · −Z = (−sin yaw, 0, −cos yaw).
    (-x).atan2(-z)
}

struct Clip {
    name: &'static str,
    /// Start: the pilot's feet, from the quarter's origin, and the way it faces.
    start: [f64; 3],
    facing: [f32; 2],
    secs: f32,
    /// The controls at `t` seconds into the clip, from the pilot's mover and position
    /// (its center, from the quarter's origin).
    script: fn(f32, &Mover, DVec3) -> Pad,
    /// Follow camera: yaw (radians, 0 looks toward −Z), pitch (up), boom length, shoulder.
    camera: fn(f32) -> (f32, f32, f32, f32),
    /// With the old settings (for a before/after pair).
    before: bool,
}

fn behind(dir: [f32; 2], pitch: f32, boom: f32) -> (f32, f32, f32, f32) {
    (yaw_toward(dir[0], dir[1]), pitch, boom, 0.5)
}

fn air_control(t: f32, _: &Mover, _: DVec3) -> Pad {
    // Sprint east along the street; jump; mid-air swing the stick left (north) for a moment,
    // then straight on again. Then a second jump, braked with the stick held back.
    match t {
        t if t < 1.1 => go(1.0, 0.0),
        t if t < 1.3 => go(1.0, 0.0).jump(),
        t if t < 1.65 => go(0.0, -1.0).jump(),
        t if t < 3.3 => go(1.0, 0.0),
        t if t < 3.5 => go(1.0, 0.0).jump(),
        t if t < 4.5 => go(-1.0, 0.0).jump(),
        _ => Pad::default(),
    }
}

/// What the pilot holds or stands on, for the dry run's log.
fn held(m: &Mover) -> String {
    let mut s = Vec::new();
    if m.hang.is_some() {
        s.push("hang".to_string());
    }
    if m.rope.is_some() {
        s.push("rope".into());
    }
    if m.zip.is_some() {
        s.push("zip".into());
    }
    if let Some(l) = m.leap {
        s.push(format!("leap {:?}", l.aim));
    }
    if m.perch.is_some() {
        s.push("perch".into());
    }
    if let Some(h) = m.pole {
        s.push(format!("pole s={:.2} spin={:.1}", h.s, h.spin));
    }
    if let Some(h) = m.bar {
        s.push(format!("bar ang={:.2} rate={:.1}", h.angle, h.rate));
    }
    if let Some(w) = m.line {
        s.push(format!(
            "line s={:.2} v={:.1} lean={:.2} load={:.2}",
            w.s, w.speed, w.lean, w.load
        ));
    }
    if let Some(r) = m.roll {
        s.push(format!("roll t={:.2}", r.t()));
    }
    if m.recover.0 > 0.0 {
        s.push(format!("recover {:.2}", m.recover.0));
    }
    if let Some(c) = m.crawl {
        s.push(format!("crawl {c:.1}"));
    }
    if let Some(r) = m.runup {
        s.push(format!("runup {:.2}", r.secs));
    }
    if let Some(td) = m.takedown {
        s.push(format!("takedown t={:.2}", td.t()));
    }
    if let Some(f) = m.face {
        s.push(format!("face moving={:?}", f.moving.map(|l| l.limb)));
    }
    s.join(" ")
}

/// The next lamp along the street from `x` (its foot, from the quarter's origin).
fn next_lamp(x: f64) -> Option<(f64, f64)> {
    city::LAMPS.into_iter().find(|&(lx, _)| lx > x + 1.0)
}

/// The stick toward `to` (x, z) from `p`.
fn toward(p: DVec3, to: (f64, f64)) -> Pad {
    let d = Vec3::new((to.0 - p.x) as f32, 0.0, (to.1 - p.z) as f32).normalize_or(Vec3::X);
    go(d.x, d.z)
}

/// The x of the ledge held.
fn held_x(m: &Mover) -> Option<f64> {
    m.hang.map(|h| h.top.x - city::OFFSET[0])
}

const CLIPS: &[Clip] = &[
    Clip {
        name: "air_control_before",
        start: [-24.0, 0.0, 1.0],
        facing: [1.0, 0.0],
        secs: 5.5,
        script: air_control,
        // From high above and behind the runner, so the swerve reads as a curve.
        camera: |_| (yaw_toward(1.0, 0.0), -1.05, 9.0, 0.0),
        before: true,
    },
    Clip {
        name: "air_control_after",
        start: [-24.0, 0.0, 1.0],
        facing: [1.0, 0.0],
        secs: 5.5,
        script: air_control,
        // From high above and behind the runner, so the swerve reads as a curve.
        camera: |_| (yaw_toward(1.0, 0.0), -1.05, 9.0, 0.0),
        before: false,
    },
    Clip {
        // Up the bakery's street face under a window column: catch the band, then hold
        // toward the wall: sill, band, sill, band, cornice, over onto the roof.
        name: "climb_facade",
        start: [-10.33, 0.0, -1.0],
        facing: [0.0, 1.0],
        secs: 5.5,
        script: |t, m, p| match t {
            t if t < 0.45 => go(0.0, 1.0),
            t if t < 0.8 => go(0.0, 1.0).jump(),
            _ if p.y > 10.0 && m.hang.is_none() && m.mantle.is_none() => {
                if p.z < 4.6 {
                    go(0.0, 0.5)
                } else {
                    Pad::default()
                }
            }
            _ => go(0.0, 1.0),
        },
        camera: |t| behind([0.0, 1.0], 0.15 + 0.35 * (t / 4.0).min(1.0), 6.5),
        before: false,
    },
    Clip {
        // Off the bakery's roof edge with crouch held: hang from the cornice, then crouch
        // again and again: down the face ledge by ledge, letting go at the bottom.
        name: "drop_down",
        start: [-10.33, 10.5, 4.2],
        facing: [0.0, -1.0],
        secs: 7.0,
        script: |t, m, _| {
            let tap = |every: f32, from: f32| t > from && ((t - from) % every) < 0.1;
            match t {
                t if t < 2.5 && m.hang.is_none() => go(0.0, -0.45).crouch(),
                _ if tap(0.7, 2.6) => Pad::default().crouch(),
                _ => Pad::default(),
            }
        },
        camera: |t| {
            (
                yaw_toward(0.0, 1.0),
                0.35 - 0.3 * (t / 6.0).min(1.0),
                8.0,
                0.0,
            )
        },
        before: false,
    },
    Clip {
        // Hanging on the townhouse's first band: shimmy to its end, leap across the gap to
        // the warehouse's band, shimmy to a window column, climb it onto the roof.
        name: "leap_across",
        start: [8.0, 0.0, 0.6],
        facing: [0.0, 1.0],
        secs: 8.0,
        script: |t, m, p| {
            let transit = m.hang.is_some_and(|h| h.transit);
            match (t, held_x(m)) {
                (t, _) if t < 0.35 => go(0.0, 1.0),
                (t, _) if t < 0.7 => go(0.0, 1.0).jump(),
                _ if transit => Pad::default(),
                (_, Some(x)) if x < 10.3 => go(1.0, 0.0),
                (_, Some(x)) if x < 12.0 => go(1.0, 0.0).jump(),
                (_, Some(x)) if x < 15.1 => go(1.0, 0.0),
                (_, Some(_)) => go(0.0, 1.0),
                _ if p.y > 6.0 && m.mantle.is_none() => Pad::default(),
                _ => go(0.0, 1.0),
            }
        },
        camera: |t| (yaw_toward(0.35, 1.0), 0.2 + 0.25 * (t / 7.0), 8.0, 0.0),
        before: false,
    },
    Clip {
        // Sprinting at the townhouse under a window column: up the wall, a ledge caught, on
        // up its sills and bands to its roof.
        name: "wall_run_up",
        start: [8.75, 0.0, -4.4],
        facing: [0.0, 1.0],
        secs: 7.5,
        script: |t, m, p| {
            if t > 1.5 && p.y > 13.0 && m.hang.is_none() && m.mantle.is_none() {
                Pad::default()
            } else {
                go(0.0, 1.0)
            }
        },
        camera: |t| behind([0.0, 1.0], 0.1 + 0.5 * (t / 5.0).min(1.0), 7.5),
        before: false,
    },
    Clip {
        // Along the townhouse's front, a running jump into the hoist rope, climbed hand over
        // hand to the beam, stepping off onto the townhouse roof beside it.
        name: "rope_climb",
        start: [0.5, 0.0, 1.95],
        facing: [1.0, 0.0],
        secs: 9.0,
        script: |t, m, p| match t {
            t if t < 0.4 => go(1.0, 0.0),
            t if t < 0.8 => go(1.0, 0.0).jump(),
            // On the rope the climber faces the way the camera looks: forward climbs.
            _ if m.rope.is_some() => go(0.45, 1.0).calm(),
            _ if m.mantle.is_some() || t < 1.4 => go(1.0, 0.0),
            _ if p.y > 13.0 && p.z < 3.6 => go(0.0, 0.6),
            _ => Pad::default(),
        },
        camera: |t| {
            (
                yaw_toward(0.45, 1.0),
                0.1 + 0.5 * (t / 5.0).min(1.0),
                8.0,
                0.0,
            )
        },
        before: false,
    },
    Clip {
        // A running jump into the gallows rope on the square, pumped, and let go at the top
        // of the swing.
        name: "rope_swing",
        start: [-21.0, 0.0, 0.0],
        facing: [-1.0, 0.0],
        secs: 6.5,
        script: |t, m, _| match t {
            t if t < 0.6 => go(-1.0, 0.0),
            t if t < 0.95 => go(-1.0, 0.0).jump(),
            _ if m.rope.is_some() => {
                let r = m.rope.unwrap();
                let out = t > 4.0 && r.swing.x < -0.2 && r.dir.x < -0.35;
                if out {
                    go(-1.0, 0.0).jump()
                } else if r.swing.x > 0.0 {
                    go(0.0, 0.0)
                } else {
                    go(-1.0, 0.0)
                }
            }
            _ => go(-0.4, 0.0),
        },
        camera: |_| (yaw_toward(0.0, -1.0), 0.12, 10.0, 0.0),
        before: false,
    },
    Clip {
        // Along the top of the bell tower and a jump off its street edge into the zip line,
        // down over the chandlery's roof and along the street to its end.
        name: "zip_line",
        start: [6.0, 14.0, -6.26],
        facing: [0.938, 0.346],
        secs: 4.5,
        script: |t, m, p| match t {
            _ if m.zip.is_some() => go(0.6, 0.2),
            _ if p.y > 14.5 && p.x > 6.9 && t < 1.0 => go(0.938, 0.346).jump(),
            t if t < 1.0 => go(0.938, 0.346),
            t if t < 3.0 => go(0.6, 0.2),
            _ => Pad::default(),
        },
        camera: |_| behind([0.938, 0.346], 0.05, 7.0),
        before: false,
    },
    Clip {
        // Run at the first lamp post, leap onto its shaft, climb to the lantern and crouch on
        // it, then lamp post to lamp post down the street.
        name: "lamp_posts",
        start: [-16.5, 0.0, -1.7],
        facing: [1.0, 0.0],
        secs: 9.0,
        script: |t, m, p| match t {
            _ if m.leap.is_some() => Pad::default(),
            _ if m.pole.is_some() => go(1.0, 0.0).calm(),
            _ if m.perch.is_some() => match (m.perch.unwrap().secs, next_lamp(p.x)) {
                (s, Some(l)) if s > 0.35 => toward(p, l).jump(),
                _ => Pad::default(),
            },
            _ if p.x < -15.2 && t < 1.0 => go(1.0, 0.0),
            t if t < 1.0 => go(1.0, 0.0).jump(),
            _ => Pad::default(),
        },
        camera: |t| (yaw_toward(1.0, -0.25), 0.25 + 0.1 * (t / 9.0), 9.0, 0.0),
        before: false,
    },
    Clip {
        // From the townhouse's roof onto the tightrope and over the street to the bell
        // tower, balancing.
        name: "tightrope",
        start: [8.6, 14.0, 6.0],
        facing: [0.0, -1.0],
        secs: 9.0,
        script: |_, m, p| {
            if m.line.is_some() || p.z > 3.0 {
                go(0.0, -1.0).calm()
            } else {
                Pad::default()
            }
        },
        camera: |t| (yaw_toward(-1.0, -0.6), 0.12, 7.0 - 0.2 * t, 0.0),
        before: false,
    },
    Clip {
        // Up the crates to the bars, swinging from bar to bar across the street, and off
        // the last into the warehouse's band.
        name: "bars",
        start: [14.8, 2.2, -2.0],
        facing: [1.0, 0.0],
        secs: 9.5,
        script: |t, m, _| match (t, m.bar) {
            (t, None) if t < 0.5 => Pad::default(),
            (t, None) if t < 0.8 => go(1.0, 0.3).jump(),
            // Off the last bar at the top of a swing: out onto the street.
            (_, Some(h)) if h.at.x - city::OFFSET[0] > 20.0 => {
                if h.secs > 1.2 && h.rate > 0.8 && h.angle > 0.25 {
                    go(1.0, 0.0).calm().jump()
                } else {
                    go(1.0, 0.0).calm()
                }
            }
            (_, Some(h)) if h.secs > 0.6 && h.rate > 0.8 && h.angle > 0.15 => {
                go(1.0, 0.0).calm().jump()
            }
            (_, Some(_)) => go(1.0, 0.0).calm(),
            _ => Pad::default(),
        },
        camera: |_| (yaw_toward(0.0, -1.0), 0.12, 8.5, 0.0),
        before: false,
    },
    Clip {
        // Up the crag by its holds and over the top.
        name: "rock_climb",
        start: [-30.6, 0.0, -12.5],
        facing: [-1.0, 0.0],
        secs: 12.0,
        script: |_, m, p| {
            if p.y > 9.2 && m.face.is_none() {
                Pad::default()
            } else {
                go(-1.0, 0.0).calm()
            }
        },
        camera: |t| {
            (
                yaw_toward(-1.0, 0.4),
                0.15 + 0.25 * (t / 10.0).min(1.0),
                7.0,
                0.0,
            )
        },
        before: false,
    },
    Clip {
        // Up the frozen waterfall with the picks.
        name: "ice_climb",
        start: [-30.6, 0.0, 11.5],
        facing: [-1.0, 0.0],
        secs: 12.0,
        script: |_, m, p| {
            if p.y > 10.2 && m.face.is_none() {
                Pad::default()
            } else {
                go(-1.0, 0.0).calm()
            }
        },
        camera: |t| {
            (
                yaw_toward(-1.0, -0.4),
                0.15 + 0.25 * (t / 10.0).min(1.0),
                7.0,
                0.0,
            )
        },
        before: false,
    },
    Clip {
        // Up the oak's trunk into the fork, and out along its north branch over the lane,
        // crouched at its tip.
        name: "oak",
        start: [-24.0, 0.0, 11.0],
        facing: [-1.0, 0.0],
        secs: 9.0,
        script: |t, m, p| match t {
            _ if m.pole.is_some() => go(-1.0, 0.0).calm(),
            _ if m.perch.is_some() && t > 1.0 => go(0.0, 1.0).calm(),
            _ if m.line.is_some() && p.z < 14.6 => go(0.0, 1.0).calm(),
            _ if m.line.is_some() => Pad::default(),
            t if t < 0.4 => go(-1.0, 0.0),
            t if t < 0.75 => go(-1.0, 0.0).jump(),
            _ => Pad::default(),
        },
        camera: |t| (yaw_toward(-0.4, 1.0), 0.2 + 0.1 * (t / 9.0), 8.0, 0.0),
        before: false,
    },
    Clip {
        // On the guild hall's balcony: up onto its railing, along it, and across the street
        // down onto a lamp.
        name: "railing",
        start: [-8.0, 7.0, -2.3],
        facing: [0.0, 1.0],
        secs: 7.0,
        script: |t, m, p| match t {
            _ if m.perch.is_some() => Pad::default(),
            _ if m.line.is_some() && p.x < -4.4 => go(1.0, 0.0).calm(),
            // Across the street and down onto the lamp on the far side.
            _ if m.line.is_some() || m.leap.is_some() => toward(p, (-2.0, 1.7)).jump(),
            t if t < 0.3 => Pad::default(),
            t if t < 0.5 => go(0.0, 1.0).calm().jump(),
            _ => Pad::default(),
        },
        camera: |_| (yaw_toward(0.3, -1.0), 0.15, 7.0, 0.0),
        before: false,
    },
    Clip {
        // Out along the oak's north branch to its tip over the lane, and down onto the
        // lookout below.
        name: "oak_hunt",
        start: [-24.0, 0.0, 11.0],
        facing: [-1.0, 0.0],
        secs: 8.5,
        script: |t, m, p| match t {
            _ if m.takedown.is_some() => Pad::default(),
            _ if m.pole.is_some() => go(-1.0, 0.0).calm(),
            _ if m.perch.is_some() && t > 1.0 => go(0.0, 1.0).calm(),
            _ if m.line.is_some() && p.z < 14.4 => go(0.0, 1.0).calm(),
            _ if m.line.is_some() && m.line.unwrap().speed.abs() < 0.2 => Pad::default().attack(),
            _ if m.line.is_some() => Pad::default(),
            t if t < 0.4 => go(-1.0, 0.0),
            t if t < 0.75 => go(-1.0, 0.0).jump(),
            _ => Pad::default(),
        },
        camera: |t| (yaw_toward(-1.0, 0.5), 0.15 + 0.1 * (t / 8.0), 7.5, 0.0),
        before: false,
    },
    Clip {
        // Off the inn's roof (7 m) with crouch tapped just before the ground: a roll that
        // keeps the run going.
        name: "landing_roll",
        start: [-21.5, 7.0, -6.0],
        facing: [-0.3, 0.954],
        secs: 3.5,
        script: |_, m, p| {
            let on = go(-0.3, 0.954).calm();
            if !m.grounded && p.y < 2.8 && p.y > 1.2 {
                on.crouch()
            } else {
                on
            }
        },
        camera: |_| (yaw_toward(1.0, 0.1), 0.1, 8.0, 0.0),
        before: false,
    },
    Clip {
        // The same drop without the roll: a hard landing, down on a knee.
        name: "landing_hard",
        start: [-21.5, 7.0, -6.0],
        facing: [-0.3, 0.954],
        secs: 3.5,
        script: |_, _, _| go(-0.3, 0.954).calm(),
        camera: |_| (yaw_toward(1.0, 0.1), 0.1, 8.0, 0.0),
        before: false,
    },
    Clip {
        // A leap of faith off the inn's roof into the hay cart.
        name: "hay_leap",
        start: [-18.0, 7.0, -6.0],
        facing: [0.0, 1.0],
        secs: 3.5,
        script: |t, _, p| match t {
            _ if p.z > -3.4 || t > 1.6 => Pad::default(),
            _ if p.z > -3.9 => go(0.0, 0.5).calm().jump(),
            _ => go(0.0, 0.5).calm(),
        },
        camera: |_| (yaw_toward(1.0, 0.0), 0.15, 9.0, 0.0),
        before: false,
    },
    Clip {
        // A sprint off the inn's roof across the street onto the bakery's awning: bounced up
        // its front, the hands catch the band.
        name: "awning",
        start: [-18.0, 7.0, -8.0],
        facing: [0.0, 1.0],
        secs: 3.5,
        script: |t, _, p| match t {
            _ if p.z > -4.4 || t > 1.0 => Pad::default(),
            _ if p.z > -5.0 => go(0.0, 1.0).jump(),
            _ => go(0.0, 1.0),
        },
        camera: |_| (yaw_toward(1.0, 0.0), 0.15, 10.0, 0.0),
        before: false,
    },
    Clip {
        // Off the townhouse's roof (14 m), stepping back to the face on the way down with
        // grab (sprint) held: the hands catch a band going by.
        name: "fall_catch",
        start: [9.5, 14.0, 3.6],
        facing: [0.0, -1.0],
        secs: 4.5,
        script: |t, m, p| match t {
            _ if m.hang.is_some() => Pad::default(),
            _ if m.grounded && t < 2.5 => go(0.0, -0.15).calm(),
            _ if p.y > 12.2 => Pad::default(),
            // Off the edge: back toward the face, reaching for it.
            _ => Pad {
                sprint: true,
                ..go(0.0, 0.6)
            },
        },
        camera: |_| (yaw_toward(1.0, -0.3), 0.0, 9.0, 0.0),
        before: false,
    },
    Clip {
        // Across the alley: up the townhouse's wall, kicked off it to the bakery's, up that,
        // kicked back.
        name: "wall_kicks",
        start: [-1.55, 0.0, 6.5],
        facing: [1.0, 0.0],
        secs: 4.5,
        script: |_, m, _| match m.runup {
            Some(r) if r.secs > 0.12 => go(-r.normal.x, 0.0).jump(),
            Some(r) => go(-r.normal.x, 0.0),
            None if m.mode == engine_world::LocomotionMode::Air => Pad::default(),
            None => go(1.0, 0.0),
        },
        camera: |_| (yaw_toward(0.0, 1.0), 0.25, 8.0, 0.0),
        before: false,
    },
    Clip {
        // Through the culvert on hands and knees.
        name: "crawl",
        start: [-26.5, 0.0, -6.5],
        facing: [-1.0, 0.0],
        secs: 6.0,
        script: |_, _, p| {
            if p.x > -33.0 {
                go(-1.0, 0.0).calm()
            } else {
                Pad::default()
            }
        },
        camera: |_| (yaw_toward(-1.0, 0.6), 0.2, 5.5, 0.0),
        before: false,
    },
];

/// The movement before this pass: world gravity rising and falling, the same height for a
/// tap and a hold, and nothing but a little push along the stick in the air.
fn old_settings(p: &mut MovementProfile) {
    p.rise_gravity = 1.0;
    p.fall_gravity = 1.0;
    p.apex_gravity = 1.0;
    p.jump_cut_gravity = 1.0;
    p.air_accel = 0.0;
    p.air_brake = 0.0;
    p.air_drag = 0.0;
}

fn build_app() -> (App, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WIDTH;
        w.height = HEIGHT;
    }
    app.insert_resource(LookSettings {
        texture_size: 512,
        background: false,
    });
    app.add_plugins(RenderPlugin)
        .add_plugins(InputPlugin)
        .add_plugins(WorldPlugin)
        .add_plugins(MovementPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.insert_resource(RenderDisabled("settling".into()));
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.0012;
        fog.height_falloff = 0.05;
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.bloom_intensity = 0.05;
        s.vignette = 0.2;
    }
    // Late-afternoon sun from the south-west, raking across the street faces.
    let sun = Vec3::new(-0.55, 0.42, -0.72).normalize();
    app.world.spawn((
        DirectionalLight {
            color: [1.0, 0.88, 0.72],
            illuminance: 75_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun.as_dvec3(), Vec3::Y),
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, -0.02, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4a5a3a).with_roughness(0.95),
        ),
    ));
    let camera = app
        .world
        .spawn((
            Camera::perspective(60.0).with_exposure(Exposure::Auto {
                min_ev100: 9.0,
                max_ev100: 16.0,
                speed: 30.0,
                compensation: 0.2,
            }),
            Transform::from_xyz(0.0, 5.0, 60.0),
        ))
        .id();
    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    queue.apply(&mut app.world);
    (app, camera)
}

fn film(clip: &Clip, out: &Path) -> Result<PathBuf, String> {
    let (mut app, base_camera) = build_app();
    let origin = DVec3::from_array(city::OFFSET);
    let feet = origin + DVec3::from_array(clip.start);
    let facing = Vec3::new(clip.facing[0], 0.0, clip.facing[1]).normalize();
    *app.world.get_mut::<Transform>(base_camera).unwrap() =
        Transform::from_translation(feet - (facing * 4.0).as_dvec3() + DVec3::Y * 2.0)
            .looking_at(feet + (facing * 4.0).as_dvec3() + DVec3::Y * 1.5, Vec3::Y);
    // Settle the world (the look bakes on the first frame).
    for _ in 0..5 {
        app.update();
    }
    let pilot = play::start(&mut app.world, Some(feet))?;
    if clip.before {
        let mut p = app.world.get_mut::<MovementProfile>(pilot).unwrap();
        old_settings(&mut p);
    }
    let set_camera = |world: &mut World, t: f32| {
        let (yaw, pitch, boom, shoulder) = (clip.camera)(t);
        let mut q = world.query::<&mut PilotCamera>();
        for mut pc in q.iter_mut(world) {
            pc.yaw = yaw;
            pc.pitch = pitch;
            pc.distance = boom;
            pc.shoulder = shoulder;
        }
    };
    for _ in 0..SETTLE_FRAMES {
        set_camera(&mut app.world, 0.0);
        app.update();
    }
    if clip.before {
        // (The pilot's mover keeps its variable jump: the old one had none.)
        app.world.get_mut::<Mover>(pilot).unwrap().variable_jump = false;
    }

    let dry = std::env::var_os("FILM_DRY").is_some();
    let dir = out.join(clip.name);
    if !dry {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        app.world.remove_resource::<RenderDisabled>();
    }
    let steps = (clip.secs as f64 / DT).round() as u32;
    let mut frame = 0;
    for step in 0..steps {
        let t = step as f32 * DT as f32;
        set_camera(&mut app.world, t);
        let pad = {
            let m = app.world.get::<Mover>(pilot).unwrap().clone();
            let p = app.world.get::<Transform>(pilot).unwrap().translation - origin;
            (clip.script)(t, &m, p)
        };
        // The stick, as the camera sees it.
        let cam_fwd = {
            let mut q = app.world.query::<(&PilotCamera, &GlobalTransform)>();
            q.iter(&app.world)
                .next()
                .map(|(_, g)| g.forward())
                .unwrap_or(Vec3::NEG_Z)
        };
        let fwd = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z).normalize_or(Vec3::NEG_Z);
        let right = fwd.cross(Vec3::Y);
        let axis = Vec2::new(pad.stick.dot(right), pad.stick.dot(fwd));
        {
            let mut a = app.world.resource_mut::<ActionState>();
            a.set_axis("move", axis, None);
            for (name, on) in [
                ("jump", pad.jump),
                ("crouch", pad.crouch),
                ("sprint", pad.sprint),
                ("attack", pad.attack),
            ] {
                if on {
                    a.press(name);
                } else {
                    a.release(name);
                }
            }
        }
        if dry {
            app.update();
            if step % 15 == 0 || std::env::var_os("FILM_EVERY").is_some() {
                let m = app.world.get::<Mover>(pilot).unwrap();
                let p = app.world.get::<Transform>(pilot).unwrap().translation - origin;
                println!(
                    "  {t:5.2} {:?} {:.1} m/s v {:.1?} {} ({:.2}, {:.2}, {:.2})",
                    m.mode,
                    m.horizontal_speed(),
                    m.velocity,
                    held(m),
                    p.x,
                    p.y,
                    p.z
                );
            }
            continue;
        }
        // Draw every third step: 20 frames a second (or, for a still, only the steps at the
        // times asked for).
        let still: Option<Vec<f32>> = std::env::var("FILM_STILLS")
            .ok()
            .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect());
        let draw = match &still {
            Some(times) => times.iter().any(|&at| (t - at).abs() < DT as f32 * 0.5),
            None => step % 3 == 0,
        };
        if !draw {
            app.world
                .insert_resource(RenderDisabled("between frames".into()));
        }
        app.update();
        if !draw {
            app.world.remove_resource::<RenderDisabled>();
            continue;
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            return Err(format!("cannot render: {}", d.0));
        }
        let path = dir.join(format!("{frame:04}.png"));
        capture_now(&mut app.world, &path).map_err(|e| e.to_string())?;
        frame += 1;
    }
    let m = app.world.get::<Mover>(pilot).unwrap();
    let p = app.world.get::<Transform>(pilot).unwrap().translation - origin;
    println!(
        "{}: {frame} frames, ends {:?} at ({:.1}, {:.1}, {:.1})",
        clip.name, m.mode, p.x, p.y, p.z
    );
    Ok(dir)
}

fn ffmpeg(args: &[&str]) -> Result<(), String> {
    let s = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    s.success()
        .then_some(())
        .ok_or_else(|| format!("ffmpeg {args:?} failed"))
}

/// MP4 (H.264, 20 fps) and GIF (10 fps, 640 wide) of a clip's frames.
fn encode(frames: &Path, out: &Path, name: &str) -> Result<(), String> {
    let pattern = frames.join("%04d.png");
    let pattern = pattern.to_str().unwrap();
    let mp4 = out.join(format!("{name}.mp4"));
    ffmpeg(&[
        "-framerate",
        "20",
        "-i",
        pattern,
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        "20",
        "-movflags",
        "+faststart",
        mp4.to_str().unwrap(),
    ])?;
    let gif = out.join(format!("{name}.gif"));
    ffmpeg(&[
        "-framerate",
        "20",
        "-i",
        pattern,
        "-vf",
        "fps=10,scale=640:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=160[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
        gif.to_str().unwrap(),
    ])?;
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/parkour_film"));
    let only: Vec<String> = args.collect();
    std::fs::create_dir_all(&out).expect("output directory");
    for clip in CLIPS {
        if !only.is_empty() && !only.iter().any(|o| clip.name.starts_with(o.as_str())) {
            continue;
        }
        if std::env::var_os("FILM_DRY").is_some() {
            if let Err(e) = film(clip, &out) {
                eprintln!("{}: {e}", clip.name);
            }
            continue;
        }
        match film(clip, &out).and_then(|frames| encode(&frames, &out, clip.name)) {
            Ok(()) => println!("saved {}", out.join(format!("{}.mp4", clip.name)).display()),
            Err(e) => eprintln!("{}: {e}", clip.name),
        }
    }
}
