//! The scripting showcase: the Script courtyard, a cobbled circle at dusk run entirely by Luau.
//!
//! What to look for:
//!
//! - The ring of twelve lanterns and the brass orrery in the middle are built by
//!   `assets/scripts/showcase/courtyard.luau` when it starts, not by Rust: the stone, the
//!   braziers, the lever and the code board are the only things spawned here.
//! - A wave of light chases round the lanterns and the orrery's rings turn, every frame from
//!   the script's `update`.
//! - The code board on the north side shows the director script's own source, syntax-colored.
//!   Edit the file while the game runs and the courtyard changes within a quarter second: the
//!   board shows the new text with the changed lines lit, the lanterns keep their place in the
//!   wave (the script keeps its state), and a typo shows an error on the board while the old
//!   code keeps running.
//! - Pull the lever (E) and the ritual plays: the lanterns go out one by one, flare back in
//!   sequence, the braziers light, the orrery spins up and a beam goes up from its core, with a
//!   moment of slow motion. It is one coroutine in the script, written with `task.wait`.
//! - The profile board beside the code board shows where the scripts spend their time, as a
//!   flame graph and the hottest lines, recorded live ten seconds at a time; the code board
//!   shows each line's share of it beside the line.
//! - An error in a script marks its line on the board in red with the message, and the rest of
//!   the courtyard keeps running.
//!
//! `script.send {"entity": {"name": "Courtyard director"}, "event": "ritual"}` plays the ritual
//! from the console or a tour stop; `script.edit` changes a script file like an editor would.

use engine_core::math::{DVec3, Quat};
use engine_core::prelude::*;
use engine_render::{MaterialPreset, MeshRenderer, Shape, StandardMaterial};
use engine_world::Interactable;
use serde_json::json;

use crate::Script;
use crate::board::{CodeBoard, ProfileBoard};

/// The director: its script builds the lanterns and the orrery and plays the ritual.
pub const DIRECTOR: &str = "Courtyard director";
/// The director's script.
pub const COURTYARD_SCRIPT: &str = "showcase/courtyard.luau";
/// The lever's script.
pub const LEVER_SCRIPT: &str = "showcase/lever.luau";

/// Radius of the cobbled floor, meters.
pub const FLOOR_RADIUS: f64 = 14.0;
/// How far the courtyard stands above its pad: a low plinth, since the terrain's coarser
/// patches rise a few centimeters above a flat pad between their vertices and showed through
/// a floor laid flush with it. Everything here is built this much higher.
pub const LIFT: f64 = 0.3;
/// Where the code board stands (meters from the middle, facing the middle).
pub const BOARD: [f64; 3] = [0.0, 2.05, -9.0];
/// Where the profile board stands, and its turn about Y (it faces the middle).
pub const PROFILE_BOARD: [f64; 3] = [4.3, 2.05, -8.2];
pub const PROFILE_BOARD_YAW: f32 = -0.483;
/// Where the lever stands.
pub const LEVER: [f64; 3] = [4.2, 0.0, 6.6];
/// Where the braziers stand: on the diagonals between the lanterns and the middle.
pub const BRAZIER_RADIUS: f64 = 7.6;

/// A tour stop: eye and target relative to the courtyard's middle, and API calls made on
/// arriving and leaving (name, JSON args).
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup_secs: f32,
    pub on_arrive: &'static [(&'static str, &'static str)],
    pub on_leave: &'static [(&'static str, &'static str)],
}

/// The lanterns' color as the director script ships, and the color the hot-reload stop edits
/// it to (`script.edit` finds this exact text).
const WARM: &str = r#"{"path": "showcase/courtyard.luau", "find": "color = vec3(1.0, 0.56, 0.22)", "replace": "color = vec3(0.25, 0.62, 1.0)"}"#;
const COOL: &str = r#"{"path": "showcase/courtyard.luau", "find": "color = vec3(0.25, 0.62, 1.0)", "replace": "color = vec3(1.0, 0.56, 0.22)"}"#;

/// The debugger stop: the board scrolls to `update` and logpoints show what its lines compute
/// each frame (`script.logpoint`), without stopping the game. Line numbers are
/// courtyard.luau's (the courtyard test checks they still hold code).
pub const LIVE_VALUES_ON: &[(&str, &str)] = &[
    (
        "script.eval",
        r#"{"code": "world.find('Code board'):set('CodeBoard', { first_line = 17 })"}"#,
    ),
    (
        "script.logpoint",
        r#"{"path": "showcase/courtyard.luau", "line": 26, "message": "t = {self.t}"}"#,
    ),
    (
        "script.logpoint",
        r#"{"path": "showcase/courtyard.luau", "line": 30, "message": "lamp {i}: phase = {phase}"}"#,
    ),
    (
        "script.logpoint",
        r#"{"path": "showcase/courtyard.luau", "line": 31, "message": "crest = {crest}"}"#,
    ),
    (
        "script.logpoint",
        r#"{"path": "showcase/courtyard.luau", "line": 32, "message": "glow = {glow}"}"#,
    ),
    (
        "script.logpoint",
        r#"{"path": "showcase/courtyard.luau", "line": 36, "message": "spin = {self.spin}"}"#,
    ),
];
const LIVE_VALUES_OFF: &[(&str, &str)] = &[
    ("script.logpoint", r#"{"clear": true}"#),
    (
        "script.eval",
        r#"{"code": "world.find('Code board'):set('CodeBoard', { first_line = 1 })"}"#,
    ),
];

/// Scrolls the code board to `update` (the lines the profile and the error stops show).
const BOARD_AT_UPDATE: (&str, &str) = (
    "script.eval",
    r#"{"code": "world.find('Code board'):set('CodeBoard', { first_line = 17 })"}"#,
);
const BOARD_AT_TOP: (&str, &str) = (
    "script.eval",
    r#"{"code": "world.find('Code board'):set('CodeBoard', { first_line = 1 })"}"#,
);

/// The tour records one profile from its first courtyard stop to the profiler's stop, so the
/// flame graph covers the ritual too.
const PROFILE_START: (&str, &str) = ("script.profile", r#"{"seconds": 0, "hz": 4000}"#);
const PROFILE_STOP: (&str, &str) = ("script.profile", r#"{"stop": true}"#);
/// Back to the live profile, a window at a time.
const PROFILE_RESET: (&str, &str) = ("script.profile", r#"{"reset": true}"#);

/// The error stop's typo (a field that doesn't exist, so `update` fails on that line every
/// frame), and its fix.
const TYPO: &str = r#"{"path": "showcase/courtyard.luau", "find": "local glow = lamp.dim * (", "replace": "local glow = lamp.dimm * ("}"#;
const FIX: &str = r#"{"path": "showcase/courtyard.luau", "find": "local glow = lamp.dimm * (", "replace": "local glow = lamp.dim * ("}"#;

/// Undoes the tour's edits if a tour was stopped before it could (a killed screenshot run
/// left the typo in the file once). Stops that show the courtyard as written start with it.
const TIDY: &[(&str, &str)] = &[
    (
        "script.edit",
        r#"{"path": "showcase/courtyard.luau", "find": "local glow = lamp.dimm * (", "replace": "local glow = lamp.dim * (", "missing_ok": true}"#,
    ),
    (
        "script.edit",
        r#"{"path": "showcase/courtyard.luau", "find": "color = vec3(0.25, 0.62, 1.0)", "replace": "color = vec3(1.0, 0.56, 0.22)", "missing_ok": true}"#,
    ),
];

/// The courtyard's overview, from above the lanterns in the south-east, looking down enough
/// that the floor fills the frame (the moonlit grass outside it outshone the lanterns).
const OVERVIEW: ([f64; 3], [f64; 3]) = ([7.0, 11.0 + LIFT, 11.0], [0.0, LIFT, -1.0]);

/// Suggested viewpoints (camera bookmarks and screenshot-tour stops). The first two are a
/// before and after pair: the same view, then again once the tour has edited the lanterns'
/// color in the file.
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "script_courtyard",
        name: "Script courtyard at dusk: lanterns and orrery built and run by Luau",
        eye: OVERVIEW.0,
        target: OVERVIEW.1,
        hour: 18.9,
        warmup_secs: 2.0,
        on_arrive: &[TIDY[0], TIDY[1], PROFILE_START],
        on_leave: &[],
    },
    DemoView {
        key: "script_reload",
        name: "Hot reload: the lantern color edited in the file while the game runs",
        eye: OVERVIEW.0,
        target: OVERVIEW.1,
        hour: 18.9,
        warmup_secs: 2.0,
        on_arrive: &[("script.edit", WARM)],
        on_leave: &[("script.edit", COOL)],
    },
    DemoView {
        key: "script_board",
        name: "The code board: the director script's live source, the edited line lit",
        eye: [0.5, 2.2 + LIFT, -5.9],
        target: [0.0, 2.05 + LIFT, -9.0],
        hour: 18.9,
        warmup_secs: 0.5,
        on_arrive: &[],
        on_leave: &[],
    },
    DemoView {
        key: "script_live_values",
        name: "Live values: logpoints print what update() computes beside each line, without \
               stopping the game",
        eye: [0.5, 2.2 + LIFT, -5.9],
        target: [0.0, 2.05 + LIFT, -9.0],
        hour: 18.9,
        warmup_secs: 1.0,
        on_arrive: LIVE_VALUES_ON,
        on_leave: LIVE_VALUES_OFF,
    },
    DemoView {
        key: "script_swarm",
        name: "Parallel scripts: 320 wisps, each its own Luau script, run side by side on every \
               core round the orrery",
        // From the south, between the braziers, the boards behind the swarm.
        eye: [-1.2, 1.4 + LIFT, 10.4],
        target: [0.3, 3.0 + LIFT, 0.0],
        hour: 18.9,
        warmup_secs: 2.0,
        on_arrive: TIDY,
        on_leave: &[],
    },
    DemoView {
        key: "script_ritual",
        name: "The lever ritual: braziers, flaring lanterns and the orrery's beam",
        eye: [9.5, 3.2 + LIFT, 12.5],
        target: [0.0, 2.6 + LIFT, 0.0],
        hour: 18.9,
        // The ritual peaks 5.6 s after it starts (courtyard.luau, Courtyard:ritual).
        warmup_secs: 5.5,
        on_arrive: &[(
            "script.send",
            r#"{"entity": {"name": "Courtyard director"}, "event": "ritual"}"#,
        )],
        on_leave: &[],
    },
    DemoView {
        key: "script_profiler",
        name: "The script profiler: a flame graph of where the courtyard's scripts spend their \
               time, engine calls included, and the hottest lines",
        eye: [2.86, 2.2 + LIFT, -5.46],
        target: [4.3, 2.05 + LIFT, -8.2],
        hour: 18.9,
        warmup_secs: 1.0,
        on_arrive: &[PROFILE_STOP],
        on_leave: &[],
    },
    DemoView {
        key: "script_heat",
        name: "Profile heat on the code board: each line of update() with its share of the \
               script time",
        eye: [0.5, 2.2 + LIFT, -5.9],
        target: [0.0, 2.05 + LIFT, -9.0],
        hour: 18.9,
        warmup_secs: 0.5,
        on_arrive: &[BOARD_AT_UPDATE],
        on_leave: &[],
    },
    DemoView {
        key: "script_error",
        name: "A typo while the game runs: the board marks the failing line with the error and \
               how often it repeated; the rest of the courtyard keeps running",
        eye: [0.5, 2.2 + LIFT, -5.9],
        target: [0.0, 2.05 + LIFT, -9.0],
        hour: 18.9,
        warmup_secs: 1.0,
        on_arrive: &[("script.edit", TYPO), BOARD_AT_UPDATE],
        on_leave: &[("script.edit", FIX), BOARD_AT_TOP, PROFILE_RESET],
    },
];

fn mesh(shape: Shape, material: StandardMaterial) -> MeshRenderer {
    MeshRenderer::new(shape, material)
}

/// Spawns the courtyard's stone, braziers, lever, code board and director around `origin`
/// (on the ground there).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let at =
        |x: f64, y: f64, z: f64| Transform::from_translation(origin + DVec3::new(x, y + LIFT, z));
    let cobbles = StandardMaterial::preset(MaterialPreset::Cobblestone).with_triplanar_world();
    let marble = StandardMaterial::preset(MaterialPreset::Marble);
    let granite = StandardMaterial::preset(MaterialPreset::Rock).with_tint(0.62, 0.6, 0.58);
    let iron = StandardMaterial::color(0.045, 0.043, 0.042)
        .with_metallic(0.85)
        .with_roughness(0.55);
    let brass = StandardMaterial::color(0.78, 0.56, 0.26)
        .with_metallic(1.0)
        .with_roughness(0.28);
    let walnut = StandardMaterial::preset(MaterialPreset::WoodPlanks).with_tint(0.38, 0.27, 0.2);

    // The floor: cobbles in a ring of marble curb stones, a dark polished disc in the middle
    // with a brass inlay where the orrery stands. Floor and curbs reach down to the pad.
    commands.spawn((
        Name::new("Courtyard floor"),
        at(0.0, -0.13 - LIFT / 2.0, 0.0),
        mesh(
            Shape::Cylinder {
                radius: FLOOR_RADIUS as f32,
                height: (0.3 + LIFT) as f32,
                segments: 96,
            },
            cobbles,
        ),
    ));
    let curb_stones = 36;
    let curb_r = FLOOR_RADIUS + 0.25;
    let curb_len = (std::f64::consts::TAU * curb_r / curb_stones as f64 - 0.04) as f32;
    for i in 0..curb_stones {
        let a = i as f64 / curb_stones as f64 * std::f64::consts::TAU;
        commands.spawn((
            Name::new(format!("Curb stone {}", i + 1)),
            at(a.cos() * curb_r, 0.06 - LIFT / 2.0, a.sin() * curb_r).with_rotation(
                Quat::from_rotation_y(-(a as f32) + std::f32::consts::FRAC_PI_2),
            ),
            mesh(
                Shape::RoundedCuboid {
                    size: [curb_len, (0.24 + LIFT) as f32, 0.5],
                    radius: 0.04,
                    segments: 0,
                },
                marble,
            ),
        ));
    }
    commands.spawn((
        Name::new("Orrery dais"),
        at(0.0, 0.035, 0.0),
        mesh(
            Shape::Cylinder {
                radius: 3.4,
                height: 0.07,
                segments: 64,
            },
            StandardMaterial::preset(MaterialPreset::Marble)
                .with_tint(0.16, 0.17, 0.2)
                .with_roughness(0.12)
                .with_triplanar_world(),
        ),
    ));
    commands.spawn((
        Name::new("Dais inlay"),
        at(0.0, 0.07, 0.0),
        mesh(
            Shape::Torus {
                radius: 3.4,
                tube: 0.035,
                segments: 96,
            },
            brass,
        ),
    ));

    // The code board: a walnut easel holding the director script's source.
    let [bx, by, bz] = BOARD;
    commands.spawn((
        Name::new("Code board frame"),
        at(bx, by, bz - 0.08),
        mesh(
            Shape::RoundedCuboid {
                size: [3.85, 2.5, 0.14],
                radius: 0.04,
                segments: 0,
            },
            walnut,
        ),
    ));
    for side in [-1.0, 1.0] {
        commands.spawn((
            Name::new("Code board post"),
            at(bx + side * 1.78, 1.68, bz - 0.17),
            mesh(
                Shape::RoundedCuboid {
                    size: [0.13, 3.36, 0.13],
                    radius: 0.03,
                    segments: 0,
                },
                walnut,
            ),
        ));
    }
    commands.spawn((
        Name::new("Code board"),
        at(bx, by, bz),
        mesh(
            Shape::Quad { size: [3.6, 2.25] },
            StandardMaterial::color(0.02, 0.022, 0.026),
        )
        .without_shadows(),
        CodeBoard {
            // Big enough to read from the board's tour stop; the props and `update` fit.
            lines: 28,
            ..CodeBoard::new(COURTYARD_SCRIPT)
        },
    ));

    // The profile board, on its own easel to the code board's right, turned to the middle.
    let [px, py, pz] = PROFILE_BOARD;
    let turn = Quat::from_rotation_y(PROFILE_BOARD_YAW);
    let local = |x: f64, y: f64, z: f64| {
        let o = turn.as_dquat() * DVec3::new(x, y, z);
        at(px + o.x, py + o.y, pz + o.z).with_rotation(turn)
    };
    commands.spawn((
        Name::new("Profile board frame"),
        local(0.0, 0.0, -0.08),
        mesh(
            Shape::RoundedCuboid {
                size: [3.85, 2.5, 0.14],
                radius: 0.04,
                segments: 0,
            },
            walnut,
        ),
    ));
    for side in [-1.0, 1.0] {
        commands.spawn((
            Name::new("Profile board post"),
            local(side * 1.78, 1.68 - py, -0.17),
            mesh(
                Shape::RoundedCuboid {
                    size: [0.13, 3.36, 0.13],
                    radius: 0.03,
                    segments: 0,
                },
                walnut,
            ),
        ));
    }
    commands.spawn((
        Name::new("Profile board"),
        local(0.0, 0.0, 0.0),
        mesh(
            Shape::Quad { size: [3.6, 2.25] },
            StandardMaterial::color(0.02, 0.022, 0.026),
        )
        .without_shadows(),
        ProfileBoard::default(),
    ));

    // Four braziers on the diagonals, cold until the ritual lights them.
    for i in 0..4 {
        let a = (i as f64 + 0.5) * std::f64::consts::FRAC_PI_2;
        let (x, z) = (a.cos() * BRAZIER_RADIUS, a.sin() * BRAZIER_RADIUS);
        commands
            .spawn((Name::new(format!("Brazier {}", i + 1)), at(x, 0.0, z)))
            .with_children(|b| {
                b.spawn((
                    Transform::from_xyz(0.0, 0.42, 0.0),
                    mesh(
                        Shape::Cylinder {
                            radius: 0.24,
                            height: 0.84,
                            segments: 24,
                        },
                        granite,
                    ),
                ));
                b.spawn((
                    Transform::from_xyz(0.0, 1.06, 0.0)
                        .with_rotation(Quat::from_rotation_x(std::f32::consts::PI)),
                    mesh(
                        Shape::Cone {
                            radius: 0.52,
                            height: 0.44,
                            segments: 32,
                        },
                        iron,
                    ),
                ));
            });
    }

    // The lever on its stone pedestal: pulling it sends "ritual" to the director.
    let [lx, _, lz] = LEVER;
    // Facing the middle (its -Z toward the orrery).
    let facing = Quat::from_rotation_y(lx.atan2(lz) as f32);
    commands.spawn((
        Name::new("Lever pedestal"),
        at(lx, 0.5, lz).with_rotation(facing),
        mesh(
            Shape::RoundedCuboid {
                size: [0.62, 1.0, 0.46],
                radius: 0.05,
                segments: 0,
            },
            granite,
        ),
    ));
    commands
        .spawn((
            Name::new("Ritual lever"),
            at(lx, 1.025, lz).with_rotation(facing),
            mesh(
                Shape::RoundedCuboid {
                    size: [0.16, 0.05, 0.42],
                    radius: 0.015,
                    segments: 0,
                },
                brass,
            ),
            Interactable {
                verb: "Pull".into(),
                reach_m: 2.5,
                ..Default::default()
            },
            Script::new(LEVER_SCRIPT).with_props(json!({ "target": DIRECTOR })),
        ))
        .with_children(|l| {
            l.spawn((
                Name::new("Lever arm"),
                Transform::from_xyz(0.0, 0.02, 0.0).with_rotation(Quat::from_rotation_x(-0.55)),
            ))
            .with_children(|arm| {
                arm.spawn((
                    Transform::from_xyz(0.0, 0.3, 0.0),
                    mesh(
                        Shape::Cylinder {
                            radius: 0.022,
                            height: 0.6,
                            segments: 12,
                        },
                        iron,
                    ),
                ));
                arm.spawn((
                    Transform::from_xyz(0.0, 0.62, 0.0),
                    mesh(
                        Shape::Sphere {
                            radius: 0.055,
                            segments: 16,
                        },
                        StandardMaterial::color(0.42, 0.05, 0.04).with_roughness(0.3),
                    ),
                ));
            });
        });

    // The director: everything that moves or glows is its script's doing.
    commands.spawn((
        Name::new(DIRECTOR),
        at(0.0, 0.0, 0.0),
        Script::new(COURTYARD_SCRIPT),
    ));
}
