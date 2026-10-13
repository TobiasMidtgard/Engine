//! The vehicle test cases in the showcase.
//!
//! * [`spawn_track`]: an oval race track with a jump, two sports cars racing round it and a
//!   rally car drifting on a dirt pad.
//! * [`spawn_airfield`]: a runway with one plane taking off and another flying circuits
//!   overhead.
//! * [`spawn_spaceport`]: a VTOL spaceplane hopping off a landing pad (hover engines, dust
//!   and rocket wash) next to a parked one.
//!
//! Every surface a vehicle touches here has a static collider, so the demos work with any
//! physics backend and need nothing else from the game. The colliders are invisible; the
//! scenery drawn over them (asphalt, markings, buildings, lights) comes from
//! [`crate::models::scenery`], built to the same dimensions.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::SurfaceMaterials;
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};

use crate::autopilot::{Autopilot, AutopilotMode};
use crate::models::scenery;
use crate::presets;

/// Half-axes of the oval's center line, meters (x, z).
pub const TRACK_RADII: (f64, f64) = (138.0, 80.0);
/// Asphalt width, meters.
pub const TRACK_WIDTH: f32 = 12.0;
/// Height of the asphalt top above the zone's ground, meters.
pub const TRACK_SURFACE: f64 = 0.06;
/// Where the jump sits on the oval (angle around it, radians).
pub const JUMP_ANGLE: f64 = std::f64::consts::FRAC_PI_2;

const SEGMENTS: usize = 64;

/// A point on the oval's center line at `angle` (radians, counter-clockwise seen from
/// above, 0 = +X), on the asphalt.
pub fn oval_point(center: DVec3, angle: f64) -> DVec3 {
    center
        + DVec3::new(
            angle.cos() * TRACK_RADII.0,
            TRACK_SURFACE,
            angle.sin() * TRACK_RADII.1,
        )
}

/// Driving direction (increasing angle) at `angle`.
pub fn oval_direction(angle: f64) -> Vec3 {
    DVec3::new(
        -angle.sin() * TRACK_RADII.0,
        0.0,
        angle.cos() * TRACK_RADII.1,
    )
    .normalize()
    .as_vec3()
}

/// The racing line: the oval's center line, `n` points.
pub fn oval_path(center: DVec3, n: usize) -> Vec<DVec3> {
    (0..n)
        .map(|i| oval_point(center, i as f64 / n as f64 * std::f64::consts::TAU))
        .collect()
}

/// Rotation that points a vehicle's nose (−Z) along `dir` (horizontal).
pub fn facing(dir: Vec3) -> Quat {
    Quat::from_rotation_y((-dir.x).atan2(-dir.z))
}

/// A static box with a matching collider: `size` full extents, `surface` by material name.
fn slab(
    commands: &mut Commands,
    size: Vec3,
    transform: Transform,
    visual: Option<StandardMaterial>,
    surface: &'static str,
) {
    let mut e = commands.spawn((transform, RigidBody::fixed()));
    if let Some(m) = visual {
        e.insert(MeshRenderer::new(
            Shape::Cuboid {
                size: size.to_array(),
            },
            m,
        ));
    }
    let id = e.id();
    commands.queue(move |world: &mut World| {
        let surface = world
            .get_resource::<SurfaceMaterials>()
            .map(|m| m.id_or_default(surface))
            .unwrap_or_default();
        if let Ok(mut e) = world.get_entity_mut(id) {
            e.insert(Collider::cuboid(size * 0.5).with_surface(surface));
        }
    });
}

fn disc(
    commands: &mut Commands,
    radius: f32,
    height: f32,
    transform: Transform,
    visual: Option<StandardMaterial>,
    surface: &'static str,
) {
    let mut e = commands.spawn((transform, RigidBody::fixed()));
    if let Some(m) = visual {
        e.insert(MeshRenderer::new(
            Shape::Cylinder {
                radius,
                height,
                segments: 64,
            },
            m,
        ));
    }
    let id = e.id();
    commands.queue(move |world: &mut World| {
        let surface = world
            .get_resource::<SurfaceMaterials>()
            .map(|m| m.id_or_default(surface))
            .unwrap_or_default();
        if let Ok(mut e) = world.get_entity_mut(id) {
            e.insert(Collider {
                shape: ColliderShape::Cylinder {
                    radius,
                    half_height: height * 0.5,
                },
                surface,
                ..Default::default()
            });
        }
    });
}

/// Spawn a preset already moving at `speed` along its nose (so the demo is in full swing
/// the moment a camera arrives).
fn moving(
    commands: &mut Commands,
    preset: &str,
    position: DVec3,
    dir: Vec3,
    speed: f32,
    name: &str,
    mode: AutopilotMode,
) -> Entity {
    let t = Transform::from_translation(position).with_rotation(facing(dir));
    let e = presets::spawn(commands, preset, t).expect("preset");
    commands.entity(e).insert((
        Name::new(name.to_string()),
        Velocity {
            linear: dir * speed,
            angular: Vec3::ZERO,
        },
        Autopilot::new(mode),
    ));
    e
}

/// The ground under the zone: a grass collider (the zone's grass is drawn by the game).
fn ground(commands: &mut Commands, center: DVec3, half: Vec3) {
    slab(
        commands,
        Vec3::new(half.x * 2.0, 1.0, half.z * 2.0),
        Transform::from_translation(center + DVec3::new(0.0, -0.5, 0.0)),
        None,
        "grass",
    );
}

/// All three vehicle demos side by side on flat ground: the track at `origin`, a 1.2 km
/// runway 250 m to its north and the spaceport 330 m to its east.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    spawn_track(commands, origin);
    let runway = origin + DVec3::new(0.0, 0.0, -250.0);
    spawn_airfield(
        commands,
        runway + DVec3::new(-600.0, 0.0, 0.0),
        runway + DVec3::new(600.0, 0.0, 0.0),
    );
    let pad = origin + DVec3::new(330.0, 0.4, 0.0);
    spawn_spaceport(commands, pad, 30.0);
}

/// The vehicle track, centered on `center` (a point on flat ground). Fits in a 300 × 175 m
/// rectangle (inside a 150 m circle).
pub fn spawn_track(commands: &mut Commands, center: DVec3) {
    let (rx, rz) = TRACK_RADII;

    ground(
        commands,
        center,
        Vec3::new((rx + 40.0) as f32, 0.0, (rz + 40.0) as f32),
    );

    // The oval: asphalt segments with curbs on both edges (drawn as one continuous road by
    // the scenery).
    for i in 0..SEGMENTS {
        let a0 = i as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
        let a1 = (i + 1) as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
        let p0 = oval_point(center, a0);
        let p1 = oval_point(center, a1);
        let mid = (p0 + p1) * 0.5;
        let dir = (p1 - p0).as_vec3();
        let len = dir.length();
        let rot = facing(dir / len);
        slab(
            commands,
            Vec3::new(TRACK_WIDTH, 0.12, len + 0.4),
            Transform::from_translation(mid - DVec3::Y * 0.06).with_rotation(rot),
            None,
            "concrete",
        );
        for side in [-1.0f32, 1.0] {
            let offset = rot * Vec3::X * side * (TRACK_WIDTH * 0.5 + 0.5);
            slab(
                commands,
                Vec3::new(1.0, 0.1, len + 0.05),
                Transform::from_translation(mid + offset.as_dvec3() - DVec3::Y * 0.04)
                    .with_rotation(rot),
                None,
                "concrete",
            );
        }
    }

    // The jump: a concrete kicker across the track, 7.5° up over 6 m.
    let jump = oval_point(center, JUMP_ANGLE);
    let dir = oval_direction(JUMP_ANGLE);
    let (len, angle, thick) = (6.0f32, 7.5f32.to_radians(), 0.6f32);
    let rot = facing(dir) * Quat::from_rotation_x(angle);
    // Put the low end of the top face exactly on the asphalt.
    let center_of_ramp = jump - (rot * Vec3::new(0.0, thick * 0.5, len * 0.5)).as_dvec3();
    slab(
        commands,
        Vec3::new(TRACK_WIDTH, thick, len),
        Transform::from_translation(center_of_ramp).with_rotation(rot),
        None,
        "concrete",
    );

    // Drift pad: packed dirt inside the oval.
    let pad = center + DVec3::new(-70.0, 0.0, -20.0);
    disc(
        commands,
        26.0,
        0.12,
        Transform::from_translation(pad),
        None,
        "dirt",
    );
    scenery::track::spawn(commands, center);

    // Two sports cars racing round the oval.
    let path = oval_path(center, SEGMENTS * 2);
    let race = |max_speed: f32| AutopilotMode::Drive {
        path: path.clone(),
        looped: true,
        max_speed,
        lateral_g: 0.95,
    };
    let lift = DVec3::Y * presets::ride_height("sports_car");
    for (angle, speed, name, paint) in [
        (0.25f64, 34.0f32, "Red car", 0xc4161cu32),
        (-0.35, 32.0, "Blue car", 0x1d4fb8),
    ] {
        let p = oval_point(center, angle) + lift;
        let dir = oval_direction(angle);
        let t = Transform::from_translation(p).with_rotation(facing(dir));
        let e = presets::sports_car(commands, t, paint);
        commands.entity(e).insert((
            Name::new(name),
            Velocity {
                linear: dir * speed,
                angular: Vec3::ZERO,
            },
            Autopilot::new(race(46.0)),
        ));
    }

    // Rally car sliding round the dirt pad.
    let radius = 15.0f32;
    let drift_start = pad
        + DVec3::new(
            radius as f64,
            TRACK_SURFACE + presets::ride_height("rally_car"),
            0.0,
        );
    moving(
        commands,
        "rally_car",
        drift_start,
        Vec3::NEG_Z,
        9.0,
        "Rally car",
        AutopilotMode::Drift {
            center: pad,
            radius,
            speed: 10.0,
            slip_deg: 30.0,
            clockwise: false,
        },
    );
}

/// A runway from `start` to `end` (centers of its two ends on flat ground), 45 m wide, with
/// one plane lined up on it taking off and another flying circuits over it at 140 m.
pub fn spawn_airfield(commands: &mut Commands, start: DVec3, end: DVec3) {
    let along = (end - start).with_y(0.0);
    let length = along.length();
    let dir = (along / length).as_vec3();
    let side = Vec3::Y.cross(dir);
    let yaw = facing(dir);
    let mid = (start + end) * 0.5;
    let width = 45.0f32;
    slab(
        commands,
        Vec3::new(width, 0.12, length as f32),
        Transform::from_translation(mid).with_rotation(yaw),
        None,
        "concrete",
    );
    scenery::airfield::spawn(commands, start, end);

    // Planes: one flying circuits over the runway, one taking off to join it.
    let alt = 140.0;
    let (a, b) = (length * 0.5 + 100.0, 380.0);
    let circuit: Vec<DVec3> = (0..8)
        .map(|i| {
            let t = i as f64 / 8.0 * std::f64::consts::TAU;
            mid + (dir.as_dvec3() * (t.cos() * a))
                + (side.as_dvec3() * (t.sin() * b))
                + DVec3::Y * alt
        })
        .collect();
    let fly = AutopilotMode::Fly {
        waypoints: circuit.clone(),
        looped: true,
        cruise_speed: 60.0,
        rotate_speed: 32.0,
    };
    let first = circuit[0];
    let toward = (circuit[1] - first).normalize().as_vec3();
    moving(
        commands,
        "sport_plane",
        first,
        toward,
        60.0,
        "Circuit plane",
        fly.clone(),
    );
    let threshold =
        start + (dir * 12.0).as_dvec3() + DVec3::Y * (0.12 + presets::ride_height("sport_plane"));
    let t = Transform::from_translation(threshold).with_rotation(yaw);
    let e = presets::sport_plane(commands, t);
    commands
        .entity(e)
        .insert((Name::new("Departing plane"), Autopilot::new(fly)));
}

/// A spaceplane hopping off the landing pad at `pad` (center of its top surface) and one
/// parked beside it. `pad_radius` is the pad's size; the pad, its apron and the spaceport
/// around it are drawn by [`scenery::spaceport`].
pub fn spawn_spaceport(commands: &mut Commands, pad: DVec3, pad_radius: f32) {
    let thick = 0.4f32;
    disc(
        commands,
        pad_radius,
        thick,
        Transform::from_translation(pad - DVec3::Y * (thick as f64 * 0.5)),
        None,
        "concrete",
    );
    ground(
        commands,
        pad - DVec3::Y * thick as f64,
        Vec3::new(130.0, 0.0, 130.0),
    );
    // An apron for the parked ship.
    let apron = pad + DVec3::new(58.0, 0.0, -45.0);
    slab(
        commands,
        Vec3::new(34.0, 0.12, 40.0),
        Transform::from_translation(DVec3::new(apron.x, pad.y - thick as f64 + 0.06, apron.z)),
        None,
        "concrete",
    );
    scenery::spaceport::spawn(commands, pad, pad_radius);
    let hover = 18.0;
    let rest = presets::ride_height("spaceplane");
    let ship_at = pad + DVec3::Y * (rest + hover as f64);
    let e = presets::spaceplane(
        commands,
        Transform::from_translation(ship_at).with_rotation(facing(Vec3::NEG_Z)),
    );
    commands.entity(e).insert((
        Name::new("Hopper"),
        Autopilot::new(AutopilotMode::Hop {
            pad,
            height_m: hover,
            hover_s: 14.0,
            rest_s: 5.0,
        }),
    ));
    let parked_at = DVec3::new(apron.x, pad.y - thick as f64 + 0.12, apron.z) + DVec3::Y * rest;
    let e = presets::spaceplane(
        commands,
        Transform::from_translation(parked_at)
            .with_rotation(facing(Vec3::new(0.3, 0.0, -1.0).normalize())),
    );
    commands.entity(e).insert(Name::new("Parked spaceplane"));
}

/// A suggested camera viewpoint: eye and target relative to the demo's center, the hour of
/// day, how long the demo runs before the shot, and registry calls (API name, JSON args) to
/// make on arriving, such as lining up a fresh plane so the take-off is in frame.
#[derive(Debug, Clone)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup_secs: f32,
    pub on_arrive: Vec<(&'static str, String)>,
}

impl DemoView {
    fn new(key: &'static str, name: &'static str, eye: [f64; 3], target: [f64; 3]) -> Self {
        Self {
            key,
            name,
            eye,
            target,
            hour: 15.5,
            warmup_secs: 0.5,
            on_arrive: Vec::new(),
        }
    }

    fn at(mut self, hour: f32, warmup_secs: f32) -> Self {
        self.hour = hour;
        self.warmup_secs = warmup_secs;
        self
    }

    fn call(mut self, api: &'static str, args: serde_json::Value) -> Self {
        self.on_arrive.push((api, args.to_string()));
        self
    }
}

/// `entity.set` arguments that move the vehicle `name` to `at`, nose along `nose`, moving
/// at `velocity` and yawing left at `yaw_rate` (rad/s). The physics backend treats the move
/// as a teleport.
fn on_mark(name: &str, at: DVec3, nose: Vec3, velocity: Vec3, yaw_rate: f32) -> serde_json::Value {
    let rot = facing(nose);
    serde_json::json!({
        "entity": {"name": name},
        "components": {
            "Transform": {
                "translation": [at.x, at.y, at.z],
                "rotation": [rot.x, rot.y, rot.z, rot.w],
            },
            "Velocity": {
                "linear": [velocity.x, velocity.y, velocity.z],
                "angular": [0.0, yaw_rate, 0.0],
            },
        },
    })
}

/// Viewpoints for the showcase tour by zone (`track`, `airfield`, `spaceport`), for a demo
/// built at `center` (the zone's center). The airfield views assume the showcase runway:
/// from 600 m south of the zone's center to 600 m north.
pub fn views(zone: &str, center: DVec3) -> Vec<DemoView> {
    let abs = |x: f64, y: f64, z: f64| {
        let p = center + DVec3::new(x, y, z);
        [p.x, p.y, p.z]
    };
    match zone {
        // The cars are wherever their laps have taken them when the tour arrives, so each
        // shot first puts its car on a mark the camera covers (paint and autopilot stay).
        "track" => {
            let on_oval = |name: &str, angle: f64, speed: f32| {
                let dir = oval_direction(angle);
                let at = oval_point(center, angle) + DVec3::Y * presets::ride_height("sports_car");
                on_mark(name, at, dir, dir * speed, 0.0)
            };
            // Rally car on its drift circle, sliding at 10 m/s with the nose 30° into the
            // turn (it circles counter-clockwise seen from above).
            let pad = center + DVec3::new(-70.0, 0.0, -20.0);
            let (r, phi) = (15.0f64, 136.0f64.to_radians());
            let out = DVec3::new(phi.cos(), 0.0, phi.sin());
            let tangent = Vec3::Y.cross(out.as_vec3());
            let nose = Quat::from_rotation_y(30f32.to_radians()) * tangent;
            let at = pad + out * r + DVec3::Y * (TRACK_SURFACE + presets::ride_height("rally_car"));
            let drift = on_mark("Rally car", at, nose, tangent * 10.0, 10.0 / r as f32);
            vec![
                DemoView::new(
                    "track_cars",
                    "Sports cars racing round the oval",
                    [110.0, 2.2, 16.0],
                    [138.0, 0.9, -6.0],
                )
                .at(15.5, 1.0)
                .call("entity.set", on_oval("Red car", -0.5, 30.0))
                .call("entity.set", on_oval("Blue car", -0.68, 30.0)),
                DemoView::new(
                    "track_drift",
                    "Rally car drifting on the dirt pad",
                    [-46.0, 2.0, -4.0],
                    [-66.0, 0.8, -8.0],
                )
                .at(16.0, 1.2)
                .call("entity.set", drift),
                DemoView::new(
                    "track_jump",
                    "The kicker jump",
                    [-44.0, 2.2, 98.0],
                    [-12.0, 1.6, 80.0],
                )
                .at(15.5, 1.5)
                .call("entity.set", on_oval("Red car", JUMP_ANGLE - 0.42, 40.0)),
            ]
        }
        "airfield" => {
            // A fresh plane lined up on the southern threshold for each shot, flying the
            // same circuit as the demo's planes once it is airborne.
            let start = center + DVec3::new(0.0, 0.0, -600.0);
            let end = center + DVec3::new(0.0, 0.0, 600.0);
            let mid = (start + end) * 0.5;
            let circuit: Vec<[f64; 3]> = (0..8)
                .map(|i| {
                    let t = i as f64 / 8.0 * std::f64::consts::TAU;
                    let p = mid + DVec3::new(t.sin() * 380.0, 140.0, t.cos() * 700.0);
                    [p.x, p.y, p.z]
                })
                .collect();
            let line_up = serde_json::json!({
                "preset": "sport_plane",
                "at": abs(0.0, 0.12, -588.0),
                "yaw_deg": 180.0,
                "name": "Tour plane",
                "autopilot": {"Fly": {
                    "waypoints": circuit,
                    "looped": true,
                    "cruise_speed": 60.0,
                    "rotate_speed": 32.0,
                }},
            });
            let gone = serde_json::json!({"name": "Tour plane"});
            vec![
                DemoView::new(
                    "airfield_lineup",
                    "Sport plane lined up on the runway",
                    [14.0, 3.5, -606.0],
                    [0.0, 1.8, -584.0],
                )
                .at(10.0, 0.5)
                .call(
                    "entity.despawn",
                    serde_json::json!({"name": "Departing plane"}),
                )
                .call("entity.despawn", gone.clone())
                .call("vehicles.spawn", line_up.clone()),
                // On the Rapier backend the plane rolls 130 m and rotates about 7.4 s after
                // it is spawned; the shot catches the nose coming up.
                DemoView::new(
                    "airfield_rotate",
                    "Sport plane rotating for take-off",
                    [26.0, 3.0, -419.0],
                    [0.0, 2.0, -438.0],
                )
                .at(10.0, 7.6)
                .call(
                    "entity.despawn",
                    serde_json::json!({"name": "Departing plane"}),
                )
                .call("entity.despawn", gone)
                .call("vehicles.spawn", line_up),
            ]
        }
        "spaceport" => vec![
            // Restart the hop so the shot lands in the hover: from the pad it lifts off
            // after 5 s and hovers from about 9 s to 23 s; caught in the air, it hovers at once.
            DemoView::new(
                "spaceport_hover",
                "Spaceplane hovering over the pad",
                [38.0, 8.0, 46.0],
                [0.0, 18.0, 0.0],
            )
            .at(17.0, 12.0)
            .call(
                "vehicles.autopilot",
                serde_json::json!({
                    "vehicle": "Hopper",
                    "mode": {"Hop": {
                        "pad": abs(0.0, 0.4, 0.0),
                        "height_m": 18.0,
                        "hover_s": 14.0,
                        "rest_s": 5.0,
                    }},
                }),
            ),
            DemoView::new(
                "spaceport_parked",
                "Spaceplane parked on the apron",
                [82.0, 5.0, -18.0],
                [58.0, 3.0, -45.0],
            )
            .at(17.0, 0.5),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tour_hooks_name_real_apis_and_parse() {
        let mut calls = 0;
        for zone in ["track", "airfield", "spaceport"] {
            for v in views(zone, DVec3::new(780.0, 0.0, -150.0)) {
                for (api, args) in &v.on_arrive {
                    let args: serde_json::Value = serde_json::from_str(args).unwrap();
                    let mode = match *api {
                        "vehicles.spawn" => args.get("autopilot"),
                        "vehicles.autopilot" => args.get("mode"),
                        "entity.despawn" => None,
                        "entity.set" => {
                            let c = &args["components"];
                            serde_json::from_value::<Transform>(c["Transform"].clone())
                                .unwrap_or_else(|e| panic!("{}: {e}", v.key));
                            serde_json::from_value::<Velocity>(c["Velocity"].clone())
                                .unwrap_or_else(|e| panic!("{}: {e}", v.key));
                            None
                        }
                        other => panic!("{} calls unknown API {other}", v.key),
                    };
                    if let Some(mode) = mode {
                        let parsed: AutopilotMode = serde_json::from_value(mode.clone())
                            .unwrap_or_else(|e| panic!("{}: {e}", v.key));
                        assert_ne!(parsed, AutopilotMode::Off, "{}", v.key);
                    }
                    calls += 1;
                }
            }
        }
        assert!(calls >= 5);
    }
}
