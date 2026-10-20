//! Headless portrait sheets: each preset from the front, three-quarter and profile, the whole
//! body in a pose, and the face's expressions.

use std::path::PathBuf;

use engine::prelude::*;
use engine::world::character::FaceControls;
use engine_character::{Appearance, AppearanceExt, BuiltCharacter, CharacterBuild, CharacterDebug};

use crate::Subject;
use crate::pose::{self, PoseName};
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
    /// The whole body in this pose (`None`: the bust at rest).
    pose: Option<String>,
    face: FaceControls,
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

/// Whole-body views: the camera on the subject's chest (`feet`: on the shoes).
fn body_view(name: &str) -> Option<Orbit> {
    let body = Orbit {
        yaw: 0.0,
        pitch: 0.06,
        distance: 4.0,
        drop: 0.74,
        fov_deg: 30.0,
    };
    Some(match name {
        "body" => body,
        "body34" => Orbit { yaw: 0.62, ..body },
        "bodyside" => Orbit {
            yaw: std::f32::consts::FRAC_PI_2,
            ..body
        },
        "bodyback" => Orbit {
            yaw: std::f32::consts::PI,
            ..body
        },
        // The shoes, from the front and a little above.
        "feet" => Orbit {
            yaw: 0.5,
            pitch: 0.4,
            distance: 1.3,
            drop: 1.45,
            fov_deg: 30.0,
        },
        _ => return None,
    })
}

/// Linear RGB from an sRGB hex color (`2a4d8f`).
fn linear_from_hex(hex: &str) -> Option<[f32; 3]> {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).ok()?;
    let c = |shift: u32| {
        let s = ((v >> shift) & 0xff) as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    (hex.trim_start_matches('#').len() == 6).then(|| [c(16), c(8), c(0)])
}

impl ShotPlan {
    /// `--shots <dir> [--only name,name] [--views front,34,...] [--pose walk] [--headgear id]
    /// [--outfit top,bottom,accent_on] [--colors top,bottom,accent]` (colors as sRGB hex,
    /// `2a4d8f`), or None without
    /// `--shots`. Views are close-ups (`front`, `34`, `profile`, `eyes`, ...), whole-body views
    /// (`body`, `body34`, `bodyside`, `bodyback`, `feet`, in `--pose`), or expressions in close-up
    /// (`smile`, `sad`, `surprise`, ...; `<expression>34` from three-quarters).
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
        let body_pose = value("--pose").unwrap_or_else(|| "stand".into());
        let headgear = value("--headgear");
        let outfit = value("--outfit");
        let colors = value("--colors");
        let mut people: Vec<(String, Appearance)> = Appearance::presets()
            .into_iter()
            .map(|(n, a)| (n.to_lowercase(), a))
            .collect();
        for seed in 1..=6u64 {
            people.push((format!("random{seed}"), Appearance::random(seed)));
        }
        let mut shots = Vec::new();
        for (name, mut a) in people {
            if let Some(h) = &headgear {
                a.headgear = h.clone();
            }
            if let Some(o) = &outfit {
                let ids: Vec<&str> = o.split(',').map(str::trim).collect();
                let o = &mut a.outfit;
                for (field, id) in [&mut o.top, &mut o.bottom, &mut o.accent_on]
                    .into_iter()
                    .zip(ids)
                {
                    *field = id.to_string();
                }
            }
            if let Some(c) = &colors {
                let o = &mut a.outfit;
                for (field, hex) in [&mut o.top_color, &mut o.bottom_color, &mut o.accent]
                    .into_iter()
                    .zip(c.split(','))
                {
                    if let Some(rgb) = linear_from_hex(hex.trim()) {
                        *field = rgb;
                    }
                }
            }
            let wanted = match &only {
                Some(list) => list.contains(&name),
                None => !name.starts_with("random"),
            };
            if !wanted {
                continue;
            }
            for v in views.split(',').map(str::trim) {
                let file = format!("{name}_{v}.png");
                let shot = |orbit, debug, pose: Option<String>, face| Shot {
                    file: file.clone(),
                    appearance: a.clone(),
                    orbit,
                    debug,
                    pose,
                    face,
                };
                if let Some((orbit, debug)) = view(v) {
                    shots.push(shot(orbit, debug, None, FaceControls::default()));
                } else if let Some(orbit) = body_view(v) {
                    shots.push(shot(
                        orbit,
                        0,
                        Some(body_pose.clone()),
                        FaceControls::default(),
                    ));
                } else {
                    let (expr, orbit) = match v.strip_suffix("34") {
                        Some(e) => (e, view("34").map(|o| o.0)),
                        None => (v, view("front").map(|o| o.0)),
                    };
                    if let (true, Some(orbit)) = (pose::EXPRESSIONS.contains(&expr), orbit) {
                        shots.push(shot(orbit, 0, None, pose::expression(expr)));
                    }
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

type ShotSubject<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Appearance,
        &'static CharacterBuild,
        Option<&'static PoseName>,
        Option<&'static FaceControls>,
        Option<&'static BuiltCharacter>,
    ),
    With<Subject>,
>;

#[allow(clippy::too_many_arguments)]
pub fn run_shots(
    mut commands: Commands,
    mut plan: ResMut<ShotPlan>,
    mut shots: ResMut<Screenshots>,
    mut debug: ResMut<CharacterDebug>,
    mut subject: ShotSubject,
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
    let Ok((entity, mut a, build, pose_name, face, built)) = subject.single_mut() else {
        return;
    };
    let want_build = if shot.pose.is_some() {
        CharacterBuild::default()
    } else {
        CharacterBuild::portrait()
    };
    let mut changed = false;
    if *build != want_build {
        commands.entity(entity).insert(want_build);
        changed = true;
    }
    if pose_name.map(|p| &p.0) != shot.pose.as_ref() {
        match &shot.pose {
            Some(p) => commands.entity(entity).insert(PoseName(p.clone())),
            None => commands.entity(entity).remove::<PoseName>(),
        };
        changed = true;
    }
    if face != Some(&shot.face) {
        commands.entity(entity).insert(shot.face.clone());
        changed = true;
    }
    if *a != shot.appearance {
        *a = shot.appearance.clone();
        changed = true;
    }
    if changed {
        plan.settled = 0;
        return;
    }
    if !built.is_some_and(|b| b.appearance == shot.appearance && b.options == want_build.options())
    {
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
