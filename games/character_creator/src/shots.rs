//! Headless portrait sheets: each preset from the front, three-quarter and profile, the whole
//! body in a pose, and the face's expressions.

use std::path::PathBuf;

use engine::prelude::*;
use engine::world::character::{FaceControls, HandControls, HandPose, HandShape};
use engine_character::{
    Appearance, AppearanceExt, BuiltCharacter, CharacterBuild, CharacterDebug,
    CharacterPipelinesReady,
};

use crate::Subject;
use crate::pose::{self, PoseName};
use crate::posing::PoseEdit;
use crate::studio::{CameraFocus, Orbit};

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
    /// Both hands in this shape (`--hand`).
    hand: Option<HandShape>,
    /// The bone the camera aims at.
    focus: Option<&'static str>,
    /// A saved design's pose (`--design`).
    edit: Option<PoseEdit>,
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
    /// `--grip g`: both hands this closed (0 open .. 1 a fist).
    grip: Option<f32>,
    /// `--handle r`: a grip's handle radius, cm.
    handle_cm: f32,
    /// `--sever forearm_l,neck` and `--wound chest=0.8,thigh_r=0.5`: the subject's injuries
    /// (as the combat module writes them), and whether they're on it yet.
    injuries: Option<engine::world::combat::BodyInjuries>,
    injured: bool,
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
        "mouth" => (
            Orbit {
                yaw: 0.25,
                distance: 0.3,
                drop: 0.075,
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
        // The hands at the sides, from a little in front and above.
        "hands" => Orbit {
            yaw: 0.35,
            pitch: 0.25,
            distance: 1.5,
            drop: 0.85,
            fov_deg: 30.0,
        },
        // Close-ups that aim at a hand (see `focus`): the left from its outside, the right
        // from its outside, the left from the front and a little above, and from its palm side.
        "lhand" => Orbit {
            yaw: -1.25,
            pitch: 0.15,
            distance: 0.5,
            drop: 0.06,
            fov_deg: 30.0,
        },
        "rhand" => Orbit {
            yaw: 1.25,
            pitch: 0.15,
            distance: 0.5,
            drop: 0.06,
            fov_deg: 30.0,
        },
        "lhandfront" => Orbit {
            yaw: -0.35,
            pitch: 0.35,
            distance: 0.5,
            drop: 0.06,
            fov_deg: 30.0,
        },
        "lhandpalm" => Orbit {
            yaw: -2.3,
            pitch: 0.1,
            distance: 0.5,
            drop: 0.06,
            fov_deg: 30.0,
        },
        // Close-ups at the left elbow and the right knee (where a cut limb's stump is),
        // from the front and a little to the side.
        "lelbow" => Orbit {
            yaw: -1.2,
            pitch: 0.2,
            distance: 0.7,
            drop: 0.06,
            fov_deg: 30.0,
        },
        "rknee" => Orbit {
            yaw: 0.7,
            pitch: 0.3,
            distance: 0.8,
            drop: 0.06,
            fov_deg: 30.0,
        },
        // A close look at the left hand, from that side.
        "hand" => Orbit {
            yaw: 1.35,
            pitch: 0.15,
            distance: 0.75,
            drop: 0.82,
            fov_deg: 30.0,
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
    /// [--outfit top,bottom,accent_on] [--colors top,bottom,accent] [--grip 0..1]` (colors as
    /// sRGB hex, `2a4d8f`), or None without
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
        // `--wear 0.6,0.5,0.4,0`: dirt, blood, bruises, wet.
        let wear: Option<Vec<f32>> =
            value("--wear").map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect());
        // `--hide torso,upper_arms`: body regions left out, as under armor.
        let hide: Option<Vec<String>> =
            value("--hide").map(|s| s.split(',').map(|r| r.trim().to_string()).collect());
        let outfit = value("--outfit");
        let colors = value("--colors");
        let grip = value("--grip").and_then(|g| g.parse::<f32>().ok());
        let injuries = injuries_from(value("--sever"), value("--wound"));
        let handle_cm = value("--handle")
            .and_then(|g| g.parse::<f32>().ok())
            .unwrap_or(1.7);
        // `--hand fist,point` (or `all`): each shot once per hand shape.
        let shape_named = |n: &str| {
            engine_character::hands::SHAPES
                .iter()
                .find(|s| engine_character::hands::shape_name(**s).eq_ignore_ascii_case(n.trim()))
                .copied()
        };
        let hand_list: Vec<Option<HandShape>> = match value("--hand").as_deref() {
            None => vec![None],
            Some("all") => engine_character::hands::SHAPES
                .iter()
                .copied()
                .map(Some)
                .collect(),
            Some(list) => list.split(',').filter_map(shape_named).map(Some).collect(),
        };
        // `--design file.json`: the look and pose of a saved design (instead of the presets).
        let design = value("--design").and_then(|d| match crate::posing::load(&d) {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!("--design: {e}");
                None
            }
        });
        let mut people: Vec<(String, Appearance)> = Appearance::presets()
            .into_iter()
            .map(|(n, a)| (n.to_lowercase(), a))
            .collect();
        for seed in 1..=6u64 {
            people.push((format!("random{seed}"), Appearance::random(seed)));
        }
        // A design's own look, unless `--only` names people to wear its pose.
        let only = match (&design, only) {
            (Some(d), None) => {
                people = vec![("design".into(), d.appearance.clone())];
                Some(vec!["design".to_string()])
            }
            (_, only) => only,
        };
        let mut shots = Vec::new();
        // `--shape limb-forearm=1,limb-hand=0.5`: sliders set on everyone.
        let shapes: Vec<(String, f32)> = value("--shape")
            .map(|s| {
                s.split(',')
                    .filter_map(|kv| {
                        let (k, v) = kv.split_once('=')?;
                        Some((k.trim().to_string(), v.trim().parse().ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (name, mut a) in people {
            for (k, v) in &shapes {
                a.set_slider(k, *v);
            }
            if let Some(h) = &headgear {
                a.headgear = h.clone();
            }
            if let Some(w) = &wear {
                let at = |i: usize| w.get(i).copied().unwrap_or(0.0);
                a.wear.dirt = at(0);
                a.wear.blood = at(1);
                a.wear.bruises = at(2);
                a.wear.wet = at(3);
            }
            if let Some(h) = &hide {
                a.hide = h.clone();
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
            for (v, hand) in views
                .split(',')
                .map(str::trim)
                .flat_map(|v| hand_list.iter().map(move |h| (v, *h)))
            {
                let file = match hand {
                    Some(h) => format!(
                        "{name}_{v}_{}.png",
                        engine_character::hands::shape_name(h).to_lowercase()
                    ),
                    None => format!("{name}_{v}.png"),
                };
                let focus = match v {
                    "lhand" | "lhandfront" | "lhandpalm" => Some("hand_l"),
                    "rhand" => Some("hand_r"),
                    "lelbow" => Some("lower_arm_l"),
                    "rknee" => Some("lower_leg_r"),
                    _ => None,
                };
                let edit = design.as_ref().map(|d| d.pose.clone());
                let shot = |orbit, debug, pose: Option<String>, face| Shot {
                    file: file.clone(),
                    appearance: a.clone(),
                    orbit,
                    debug,
                    pose,
                    face,
                    hand,
                    focus,
                    edit: edit.clone(),
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
            grip,
            handle_cm,
            injuries,
            injured: false,
        })
    }
}

/// A body zone by its name in the flags (`forearm_l`, `neck`, ...).
fn zone_named(name: &str) -> Option<engine::world::combat::BodyZone> {
    let want = name.trim().replace('_', "").to_lowercase();
    engine::world::combat::BodyZone::ALL
        .into_iter()
        .find(|z| format!("{z:?}").to_lowercase() == want)
}

/// `--sever` (zones cut off) and `--wound` (`zone=amount`: wound and bleeding that much).
fn injuries_from(
    sever: Option<String>,
    wound: Option<String>,
) -> Option<engine::world::combat::BodyInjuries> {
    if sever.is_none() && wound.is_none() {
        return None;
    }
    let mut inj = engine::world::combat::BodyInjuries::default();
    for z in sever
        .iter()
        .flat_map(|s| s.split(','))
        .filter_map(zone_named)
    {
        let z = inj.get_mut(z);
        (z.severed, z.wound, z.bleeding) = (true, 1.0, 1.0);
    }
    for kv in wound.iter().flat_map(|s| s.split(',')) {
        let Some((name, v)) = kv.split_once('=') else {
            continue;
        };
        if let (Some(z), Ok(v)) = (zone_named(name), v.trim().parse::<f32>()) {
            let z = inj.get_mut(z);
            (z.wound, z.bleeding) = (v, v);
        }
    }
    Some(inj)
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
    mut focus: ResMut<CameraFocus>,
    edits: Query<&PoseEdit, With<Subject>>,
    mut shots: ResMut<Screenshots>,
    mut debug: ResMut<CharacterDebug>,
    mut subject: ShotSubject,
    mut cams: Query<&mut Orbit>,
    ready: Option<Res<CharacterPipelinesReady>>,
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
    if let (Some(inj), false) = (plan.injuries, plan.injured) {
        commands.entity(entity).insert(inj);
        plan.injured = true;
        plan.settled = 0;
        return;
    }
    let want_build = if shot.pose.is_some() || shot.edit.is_some() {
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
    if let Some(g) = plan.grip {
        commands.entity(entity).insert(HandControls {
            grip: [g, g],
            ..Default::default()
        });
    }
    if let Some(shape) = shot.hand {
        let p = HandPose {
            shape,
            weight: 1.0,
            handle_radius_m: plan.handle_cm * 0.01,
            curl: None,
        };
        commands.entity(entity).insert(HandControls {
            grip: [0.0; 2],
            hands: [p, p],
        });
    }
    if edits.single().ok() != shot.edit.as_ref() {
        match &shot.edit {
            Some(e) => commands.entity(entity).insert(e.clone()),
            None => commands.entity(entity).remove::<PoseEdit>(),
        };
        changed = true;
    }
    let want_focus = shot.focus.map(str::to_string);
    if focus.0 != want_focus {
        focus.0 = want_focus;
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
    if ready.is_none()
        || !built.is_some_and(|b| {
            engine_character::same_build(&b.appearance, &shot.appearance)
                && b.options
                    == engine_character::build::BuildOptions {
                        severed: b.options.severed,
                        ..want_build.options()
                    }
        })
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
