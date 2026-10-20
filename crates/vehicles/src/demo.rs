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

/// The oval's center line, `n` points.
pub fn oval_path(center: DVec3, n: usize) -> Vec<DVec3> {
    (0..n)
        .map(|i| oval_point(center, i as f64 / n as f64 * std::f64::consts::TAU))
        .collect()
}

/// The race line, `n` points: the oval's center line, except over the jump, where it runs
/// straight from 10 m before the kicker to 30 m past it, easing off the center line and back
/// onto it over 100 m either side. A car that leaves the kicker while turning keeps turning
/// in the air and lands sliding sideways, so it straightens up for the jump.
pub fn race_line(center: DVec3, n: usize) -> Vec<DVec3> {
    const STRAIGHT: (f64, f64) = (-10.0, 30.0);
    const EASE: f64 = 100.0;
    let (rx, rz) = TRACK_RADII;
    let point = |a: f64| DVec3::new(a.cos() * rx, 0.0, a.sin() * rz);
    // Toward the inside of the oval.
    let inward = |a: f64| oval_direction(a).cross(Vec3::Y).as_dvec3();
    // Meters along the center line from the kicker to `angle` (negative before it).
    let arc = |angle: f64| {
        let steps = 64;
        let h = (angle - JUMP_ANGLE) / steps as f64;
        (0..steps)
            .map(|i| {
                let a = JUMP_ANGLE + (i as f64 + 0.5) * h;
                (rx * a.sin()).hypot(rz * a.cos())
            })
            .sum::<f64>()
            * h
    };
    let angle_at = |s: f64| {
        let (mut lo, mut hi) = (JUMP_ANGLE - 1.5, JUMP_ANGLE + 1.5);
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if arc(mid) < s {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    };
    // How far inward the straight lies from the center line, `s` meters along it.
    let (c1, c2) = (point(angle_at(STRAIGHT.0)), point(angle_at(STRAIGHT.1)));
    let cross = |a: DVec3, b: DVec3| a.x * b.z - a.z * b.x;
    let to_straight = |s: f64| {
        let a = angle_at(s);
        cross(c1 - point(a), c2 - c1) / cross(inward(a), c2 - c1)
    };
    // Ease in and out with quintics that meet the straight's offset, slope and bend, so the
    // line's curvature never jumps.
    let ease = |at: f64, outward: f64| {
        let h = 0.5;
        let (m, p) = (to_straight(at - h), to_straight(at + h));
        let d0 = to_straight(at);
        let d1 = (p - m) / (2.0 * h) * EASE * outward;
        let d2 = (p - 2.0 * d0 + m) / (h * h) * EASE * EASE;
        (
            10.0 * d0 - 4.0 * d1 + 0.5 * d2,
            -15.0 * d0 + 7.0 * d1 - d2,
            6.0 * d0 - 3.0 * d1 + 0.5 * d2,
        )
    };
    let (q_in, q_out) = (ease(STRAIGHT.0, 1.0), ease(STRAIGHT.1, -1.0));
    let quintic = |q: (f64, f64, f64), t: f64| t * t * t * (q.0 + t * (q.1 + t * q.2));
    oval_path(center, n)
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let mut a = i as f64 / n as f64 * std::f64::consts::TAU;
            if a - JUMP_ANGLE > std::f64::consts::PI {
                a -= std::f64::consts::TAU;
            }
            if (a - JUMP_ANGLE).abs() > 1.5 {
                return p;
            }
            let s = arc(a);
            let offset = if (STRAIGHT.0..=STRAIGHT.1).contains(&s) {
                to_straight(s)
            } else if (STRAIGHT.0 - EASE..STRAIGHT.0).contains(&s) {
                quintic(q_in, (s - (STRAIGHT.0 - EASE)) / EASE)
            } else if (STRAIGHT.1..STRAIGHT.1 + EASE).contains(&s) {
                quintic(q_out, (STRAIGHT.1 + EASE - s) / EASE)
            } else {
                0.0
            };
            p + inward(a) * offset
        })
        .collect()
}

/// The kicker across the track: a concrete ramp 7.5° up over 6 m, its full size and where
/// it sits (the low end of its top face exactly on the asphalt).
pub fn kicker(center: DVec3) -> (Vec3, Transform) {
    let jump = oval_point(center, JUMP_ANGLE);
    let dir = oval_direction(JUMP_ANGLE);
    let (len, angle, thick) = (6.0f32, 7.5f32.to_radians(), 0.6f32);
    let rot = facing(dir) * Quat::from_rotation_x(angle);
    let middle = jump - (rot * Vec3::new(0.0, thick * 0.5, len * 0.5)).as_dvec3();
    (
        Vec3::new(TRACK_WIDTH, thick, len),
        Transform::from_translation(middle).with_rotation(rot),
    )
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
    commands.queue(move |world: &mut World| crate::wheels::set_rolling(world, e, speed));
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

    // The jump: a concrete kicker across the track.
    let (size, at) = kicker(center);
    slab(commands, size, at, None, "concrete");

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
    scenery::solid::spawn(
        commands,
        center,
        Quat::IDENTITY,
        "track",
        scenery::track::solids(),
    );

    // Two sports cars racing round the oval, cornering at 85 % of what their tires hold.
    let path = race_line(center, SEGMENTS * 2);
    let race = |max_speed: f32| AutopilotMode::Drive {
        path: path.clone(),
        looped: true,
        max_speed,
        lateral_g: 0.85,
    };
    let lift = DVec3::Y * presets::ride_height("sports_car");
    for (angle, speed, name, paint) in [
        (0.25f64, 21.0f32, "Red car", 0xc4161cu32),
        (-0.35, 22.0, "Blue car", 0x1d4fb8),
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
        commands.queue(move |world: &mut World| crate::wheels::set_rolling(world, e, speed));
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
    if length >= 400.0 {
        scenery::solid::spawn(
            commands,
            mid,
            yaw,
            "airfield",
            scenery::airfield::solids(length as f32),
        );
    }

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
    scenery::solid::spawn(
        commands,
        pad - DVec3::Y * thick as f64,
        Quat::IDENTITY,
        "spaceport",
        scenery::spaceport::solids(),
    );
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

/// `vehicles.place` arguments that put the vehicle `name` on the ground at `at`, nose along
/// `nose`, moving at `velocity` and turning left at `yaw_rate_deg`.
fn on_mark(
    name: &str,
    at: DVec3,
    nose: Vec3,
    velocity: Vec3,
    yaw_rate_deg: f32,
) -> serde_json::Value {
    serde_json::json!({
        "vehicle": name,
        "at": [at.x, at.y, at.z],
        "yaw_deg": nose.x.atan2(-nose.z).to_degrees(),
        "velocity": [velocity.x, velocity.y, velocity.z],
        "yaw_rate_deg": yaw_rate_deg,
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
                on_mark(name, oval_point(center, angle), dir, dir * speed, 0.0)
            };
            // Rally car on its drift circle, sliding at 10 m/s with the nose 30° into the
            // turn (it circles counter-clockwise seen from above).
            let pad = center + DVec3::new(-70.0, TRACK_SURFACE, -20.0);
            let (r, phi) = (15.0f64, 136.0f64.to_radians());
            let out = DVec3::new(phi.cos(), 0.0, phi.sin());
            let tangent = Vec3::Y.cross(out.as_vec3());
            let nose = Quat::from_rotation_y(30f32.to_radians()) * tangent;
            let omega = (10.0 / r as f32).to_degrees();
            let drift = on_mark("Rally car", pad + out * r, nose, tangent * 10.0, omega);
            vec![
                // From the gravel trap at the east turn's exit, the two cars coming through
                // the apex with the sponsor boards behind them.
                DemoView::new(
                    "track_cars",
                    "Sports cars racing round the oval",
                    [142.4, 1.3, 29.3],
                    [135.8, 0.9, 10.5],
                )
                .at(15.5, 3.2)
                .call("vehicles.place", on_oval("Red car", -0.7, 30.0))
                .call("vehicles.place", on_oval("Blue car", -0.95, 30.0)),
                DemoView::new(
                    "track_drift",
                    "Rally car drifting on the dirt pad",
                    [-53.0, 1.5, -1.0],
                    [-64.0, 0.7, -6.5],
                )
                .at(16.0, 1.2)
                .call("vehicles.place", drift),
                // Behind the barrier just past the kicker, the car flying by in the morning sun
                // with the kicker behind it (placed at 40 m/s, it slows to about 33 for the
                // straight over the kicker and is mid-air 2.2 s later).
                DemoView::new(
                    "track_jump",
                    "The kicker jump",
                    [-20.0, 1.8, 93.0],
                    [-14.0, 2.0, 84.0],
                )
                .at(9.5, 1.75)
                .call(
                    "vehicles.place",
                    on_oval("Red car", JUMP_ANGLE - 0.42, 40.0),
                ),
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
                    [8.5, 2.0, -598.5],
                    [0.0, 1.5, -585.0],
                )
                .at(10.0, 0.5)
                .call(
                    "entity.despawn",
                    serde_json::json!({"name": "Departing plane"}),
                )
                .call("entity.despawn", gone.clone())
                .call("vehicles.spawn", line_up.clone()),
                // On the Rapier backend the plane rolls 130 m and rotates about 7.4 s after
                // it is spawned; the shot (8 s after) catches the nose coming up near z −434,
                // with room ahead of it in the frame.
                DemoView::new(
                    "airfield_rotate",
                    "Sport plane rotating for take-off",
                    [14.0, 1.6, -424.0],
                    [0.0, 2.4, -431.0],
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
        "spaceport" => {
            let launch = serde_json::json!({"Ascend": {
                "target_altitude_m": 200_000.0,
                "turn_start_m": 1_000.0,
                "turn_end_m": 45_000.0,
                "heading": [0.0, 0.0, -1.0],
            }});
            // The showcase's Earth: its center under the plaza, the zone on its surface.
            let (radius, mu) = (6_371_000.0f64, 3.986e14f64);
            let height = 200_000.0;
            let orbit = center + DVec3::new(0.0, height, 0.0);
            // Where the ship starts so it is at `orbit` when the shot is taken: a little higher
            // for the fall.
            let t = orbit_capture_secs(ORBIT_WARMUP as f64);
            let fall = 0.5 * mu / (radius + height).powi(2) * t * t;
            let orbit_from = orbit + DVec3::new(0.0, fall, 0.0);
            let entry_height = 70_000.0;
            let entry_fall = 0.5 * mu / (radius + entry_height).powi(2) * t * t;
            let entry_from = center + DVec3::new(0.0, entry_height + entry_fall, 0.0);
            vec![
                // Restart the hop so the shot lands in the hover: from the pad it lifts off
                // after 5 s and hovers from about 9 s to 23 s; caught in the air, it hovers at once.
                DemoView::new(
                    "spaceport_hover",
                    "Spaceplane hovering over the pad",
                    [24.0, 6.0, 29.0],
                    [0.0, 17.0, 0.0],
                )
                .at(17.0, 12.0)
                // Back over the pad first, in case it was sent off to orbit.
                .call(
                    "vehicles.place",
                    serde_json::json!({
                        "vehicle": "Hopper",
                        "at": abs(0.0, 18.4, 0.0),
                        "yaw_deg": 0.0,
                        "velocity": [0.0, 0.0, 0.0],
                    }),
                )
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
                    [75.0, 3.6, -26.0],
                    [58.0, 3.0, -45.0],
                )
                .at(17.0, 0.5),
                // The hopper launches for a 200 km orbit from the pad: it lifts off on its hover
                // engines, stands on its tail and climbs on its main engines past the tower, seen
                // from a drone south-west of the pad as it passes 150 m.
                DemoView::new(
                    "spaceport_launch",
                    "Spaceplane launching for orbit",
                    [-75.0, 55.0, -100.0],
                    [5.0, 95.0, 10.0],
                )
                .at(17.0, 5.5)
                .call(
                    "vehicles.place",
                    serde_json::json!({
                        "vehicle": "Hopper",
                        "at": abs(0.0, 0.4, 0.0),
                        "yaw_deg": 0.0,
                        "velocity": [0.0, 0.0, 0.0],
                    }),
                )
                .call(
                    "vehicles.autopilot",
                    serde_json::json!({"vehicle": "Hopper", "mode": launch}),
                ),
                // At orbit height, 200 km up (the climb there and the orbit run in the tests,
                // nine minutes of it). The ship is held still beside the camera: at 7.8 km/s it
                // would cross the frame in a single tour step.
                DemoView::new(
                    "spaceport_orbit",
                    "Spaceplane 200 km up, at orbit height",
                    [22.0, height + 9.0, 14.0],
                    [0.0, height - 2.0, -2.0],
                )
                .at(17.0, ORBIT_WARMUP)
                .call("entity.despawn", serde_json::json!({"name": "Orbiter"}))
                .call(
                    "vehicles.spawn",
                    serde_json::json!({
                        "preset": "spaceplane",
                        "name": "Orbiter",
                        "at": [orbit_from.x, orbit_from.y, orbit_from.z],
                        "on_ground": false,
                        "gear": "up",
                    }),
                )
                .call(
                    "vehicles.place",
                    serde_json::json!({
                        "vehicle": "Orbiter",
                        "at": [orbit_from.x, orbit_from.y, orbit_from.z],
                        "on_ground": false,
                        "yaw_deg": 0.0,
                        "velocity": [0.0, 0.0, 0.0],
                    }),
                )
                // Coasting, as in orbit: the assist would fire the hover engines against the fall.
                .call(
                    "vehicles.control",
                    serde_json::json!({"vehicle": "Orbiter", "assist": "Off"}),
                ),
                // Re-entry, 70 km up: nose 40° up and banked, the hull at the 1 390 K the
                // return from orbit peaks at (test `returns_from_orbit_and_lands_on_the_pad`).
                // Held still beside the camera, as in orbit: at 7 km/s it would be gone.
                DemoView::new(
                    "spaceport_reentry",
                    "Spaceplane re-entering, hull glowing at 1 390 K",
                    [-27.0, entry_height - 4.0, -20.0],
                    [0.0, entry_height - 0.5, 0.0],
                )
                .at(18.0, ORBIT_WARMUP)
                .call("entity.despawn", serde_json::json!({"name": "Orbiter"}))
                .call(
                    "vehicles.spawn",
                    serde_json::json!({
                        "preset": "spaceplane",
                        "name": "Orbiter",
                        "at": [entry_from.x, entry_from.y, entry_from.z],
                        "on_ground": false,
                        "gear": "up",
                    }),
                )
                .call(
                    "vehicles.place",
                    serde_json::json!({
                        "vehicle": "Orbiter",
                        "at": [entry_from.x, entry_from.y, entry_from.z],
                        "on_ground": false,
                        "yaw_deg": 0.0,
                        "pitch_deg": 40.0,
                        "roll_deg": 45.0,
                        "velocity": [0.0, 0.0, 0.0],
                        "hull_temperature_k": 1_390.0,
                    }),
                )
                .call(
                    "vehicles.control",
                    serde_json::json!({"vehicle": "Orbiter", "assist": "Off"}),
                ),
                // Home again: coming in from the west, belly first on its hover engines, the
                // Return autopilot's landing burn sets the hopper down on the pad.
                DemoView::new(
                    "spaceport_landing",
                    "Spaceplane braking on its hover engines to land on the pad",
                    [-15.0, 98.0, 52.0],
                    [-62.0, 74.0, 0.0],
                )
                .at(17.0, LANDING_WARMUP)
                .call(
                    "vehicles.place",
                    serde_json::json!({
                        "vehicle": "Hopper",
                        "at": abs(-160.0, 110.0, 0.0),
                        "on_ground": false,
                        "yaw_deg": 90.0,
                        "pitch_deg": 20.0,
                        "velocity": [30.0, -4.0, 0.0],
                    }),
                )
                .call(
                    "vehicles.autopilot",
                    serde_json::json!({
                        "vehicle": "Hopper",
                        "mode": {"Return": {"pad": abs(0.0, 0.4, 0.0)}},
                    }),
                ),
            ]
        }
        _ => Vec::new(),
    }
}

/// Warmup of the in-orbit and re-entry shots (s).
const ORBIT_WARMUP: f32 = 0.5;
/// Warmup of the landing shot (s): the landing burn is under way, braking over the apron.
const LANDING_WARMUP: f32 = 3.0;

/// Seconds from a tour stop's arrival calls to its shot: the showcase tour steps 0.05 s, runs
/// the warmup and then nine more steps.
fn orbit_capture_secs(warmup: f64) -> f64 {
    (9.0 + (warmup / 0.05).ceil()) * 0.05
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
                        "vehicles.control" => {
                            let assist = args["assist"].clone();
                            serde_json::from_value::<crate::ship::AssistMode>(assist)
                                .unwrap_or_else(|e| panic!("{} assist: {e}", v.key));
                            None
                        }
                        "vehicles.place" => {
                            for key in ["at", "velocity"] {
                                serde_json::from_value::<[f64; 3]>(args[key].clone())
                                    .unwrap_or_else(|e| panic!("{} {key}: {e}", v.key));
                            }
                            assert!(args["yaw_deg"].is_f64(), "{}", v.key);
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
