//! Headless portrait sheets: each preset from the front, three-quarter and profile.

use std::path::PathBuf;

use engine::prelude::*;
use engine_character::{Appearance, BuiltCharacter, CharacterDebug};

use crate::Subject;
use crate::studio::Orbit;

/// Frames rendered after a change before a shot (temporal anti-aliasing and screen-space
/// lighting settle).
const SETTLE_FRAMES: u32 = 20;

#[derive(Debug, Clone)]
struct Shot {
    file: String,
    appearance: Appearance,
    orbit: Orbit,
    debug: u8,
}

/// The shots still to take.
#[derive(Resource, Debug)]
pub struct ShotPlan {
    dir: PathBuf,
    shots: Vec<Shot>,
    next: usize,
    settled: u32,
    requested: bool,
    completed_before: u64,
}

fn view(name: &str) -> Option<(Orbit, u8)> {
    // `<view>-masks` / `<view>-normals`: that view in a debug view.
    for (suffix, debug) in [("-masks", 1), ("-normals", 2), ("-curvature", 3)] {
        if let Some(base) = name.strip_suffix(suffix) {
            return view(base).map(|(o, _)| (o, debug));
        }
    }
    let c = Orbit::CLOSE_UP;
    Some(match name {
        "front" => (c, 0),
        "34" => (
            Orbit {
                yaw: 0.62,
                pitch: 0.04,
                ..c
            },
            0,
        ),
        "profile" => (
            Orbit {
                yaw: std::f32::consts::FRAC_PI_2,
                pitch: 0.0,
                ..c
            },
            0,
        ),
        "eyes" => (
            Orbit {
                yaw: 0.15,
                distance: 0.32,
                drop: 0.0,
                fov_deg: 20.0,
                ..c
            },
            0,
        ),
        "bust" => (
            Orbit {
                yaw: 0.35,
                distance: 1.5,
                ..c
            },
            0,
        ),
        "top" => (
            Orbit {
                pitch: 1.1,
                distance: 0.9,
                ..c
            },
            0,
        ),
        "back" => (
            Orbit {
                yaw: std::f32::consts::PI,
                ..c
            },
            0,
        ),
        "masks" => (c, 1),
        "normals" => (c, 2),
        _ => return None,
    })
}

impl ShotPlan {
    /// `--shots <dir> [--only name,name] [--views front,34,...]`, or None without `--shots`.
    pub fn from_args() -> Option<Self> {
        let args: Vec<String> = std::env::args().collect();
        let value = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let dir = PathBuf::from(value("--shots")?);
        let only: Option<Vec<String>> =
            value("--only").map(|s| s.split(',').map(|n| n.trim().to_lowercase()).collect());
        let views = value("--views").unwrap_or_else(|| "front,34,profile".into());
        let mut people: Vec<(String, Appearance)> = Appearance::presets()
            .into_iter()
            .map(|(n, a)| (n.to_lowercase(), a))
            .collect();
        for seed in 1..=6u64 {
            people.push((format!("random{seed}"), Appearance::random(seed)));
        }
        let mut shots = Vec::new();
        for (name, a) in people {
            let wanted = match &only {
                Some(list) => list.contains(&name),
                None => !name.starts_with("random"),
            };
            if !wanted {
                continue;
            }
            for v in views.split(',') {
                if let Some((orbit, debug)) = view(v.trim()) {
                    shots.push(Shot {
                        file: format!("{name}_{}.png", v.trim()),
                        appearance: a.clone(),
                        orbit,
                        debug,
                    });
                }
            }
        }
        Some(Self {
            dir,
            shots,
            next: 0,
            settled: 0,
            requested: false,
            completed_before: 0,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_shots(
    mut plan: ResMut<ShotPlan>,
    mut shots: ResMut<Screenshots>,
    mut debug: ResMut<CharacterDebug>,
    mut subject: Query<(&mut Appearance, Option<&BuiltCharacter>), With<Subject>>,
    mut cams: Query<&mut Orbit>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(shot) = plan.shots.get(plan.next).cloned() else {
        if !shots.has_pending() {
            exit.write(AppExit::Success);
        }
        return;
    };
    if plan.requested {
        // Wait for the capture to finish before moving on.
        if shots.completed > plan.completed_before {
            tracing::info!("shot {}", shot.file);
            plan.next += 1;
            plan.requested = false;
            plan.settled = 0;
        }
        return;
    }
    let Ok((mut a, built)) = subject.single_mut() else {
        return;
    };
    if *a != shot.appearance {
        *a = shot.appearance.clone();
        plan.settled = 0;
        return;
    }
    if !built.is_some_and(|b| b.appearance == shot.appearance) {
        plan.settled = 0;
        return;
    }
    for mut o in &mut cams {
        let (yaw, pitch, distance, drop, fov) = (
            shot.orbit.yaw,
            shot.orbit.pitch,
            shot.orbit.distance,
            shot.orbit.drop,
            shot.orbit.fov_deg,
        );
        if o.yaw != yaw || o.pitch != pitch || o.distance != distance || o.drop != drop {
            plan.settled = 0;
        }
        *o = Orbit {
            yaw,
            pitch,
            distance,
            drop,
            fov_deg: fov,
        };
    }
    if debug.view != shot.debug {
        debug.view = shot.debug;
        plan.settled = 0;
    }
    plan.settled += 1;
    if plan.settled >= SETTLE_FRAMES {
        let _ = std::fs::create_dir_all(&plan.dir);
        let path = plan.dir.join(&shot.file);
        plan.completed_before = shots.completed;
        shots.request(path);
        plan.requested = true;
    }
}
