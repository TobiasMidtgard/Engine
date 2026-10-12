//! The footstep trail: a loop with one stretch per kind of ground (grass, mud, snow, ice,
//! gravel, a wooden boardwalk, a metal grate, a concrete underpass that echoes, sand, fallen
//! leaves and puddles), walked round and round by a hiker, a runner who ignores slow motion, a
//! dog and a horse.
//!
//! Every footstep is a [`GroundContact`] like a real character's, so the trail tests the
//! whole path: contact → surface → ground state (wet, snowy, burnt) → synthesis → 3D mix.
//! Coloured rings on the ground show each sound as it plays.
//!
//! ```ignore
//! engine_audio::demo::spawn_demo(&mut commands, zone.center);
//! ```

use std::f64::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physical::{Deformation, SurfaceMaterial};
use engine_world::{ContactKind, GroundContact, SurfaceMaterials};

mod bodies;
mod ground;
mod rings;

pub use rings::SoundRings;

/// Half the length of the two straights, meters.
pub const HALF_STRAIGHT: f64 = 18.0;
/// Radius of the two bends, meters.
pub const BEND_RADIUS: f64 = 7.0;
const BEND: f64 = PI * BEND_RADIUS;
/// Once round the loop, meters.
pub const TRAIL_LENGTH: f64 = 4.0 * HALF_STRAIGHT + 2.0 * BEND;
/// Width of the path, meters.
pub const TRAIL_WIDTH: f32 = 2.6;

/// The stretches in walking order: surface material name and length in meters. The first
/// straight, the far bend, the second straight (boardwalk, grate, underpass, sand), the near
/// bend.
pub const STRETCHES: &[(&str, f64)] = &[
    ("grass", 12.0),
    ("mud", 12.0),
    ("snow", 12.0),
    ("ice", BEND * 0.5),
    ("gravel", BEND * 0.5),
    ("wood", 9.0),
    ("metal", 9.0),
    ("concrete", 9.0),
    ("sand", 9.0),
    ("leaves", BEND * 0.5),
    ("water", BEND * 0.5),
];

/// Where the trail is `along` meters from the start (relative to the trail's origin), and
/// which way it heads there.
pub fn trail_point(along: f64) -> (DVec3, Vec3) {
    let s = along.rem_euclid(TRAIL_LENGTH);
    let (h, r) = (HALF_STRAIGHT, BEND_RADIUS);
    if s < 2.0 * h {
        (DVec3::new(-h + s, 0.0, -r), Vec3::X)
    } else if s < 2.0 * h + BEND {
        let a = (s - 2.0 * h) / r;
        (
            DVec3::new(h + r * a.sin(), 0.0, -r * a.cos()),
            Vec3::new(a.cos() as f32, 0.0, a.sin() as f32),
        )
    } else if s < 4.0 * h + BEND {
        let t = s - 2.0 * h - BEND;
        (DVec3::new(h - t, 0.0, r), Vec3::NEG_X)
    } else {
        let a = (s - 4.0 * h - BEND) / r;
        (
            DVec3::new(-h - r * a.sin(), 0.0, r * a.cos()),
            Vec3::new(-a.cos() as f32, 0.0, -a.sin() as f32),
        )
    }
}

/// The surface material name `along` meters into the trail.
pub fn surface_at(along: f64) -> &'static str {
    let mut s = along.rem_euclid(TRAIL_LENGTH);
    for &(name, len) in STRETCHES {
        if s < len {
            return name;
        }
        s -= len;
    }
    STRETCHES[STRETCHES.len() - 1].0
}

/// The point on the trail's center line nearest to `offset` (relative to the trail's
/// origin, height ignored): how far along the trail it is and how far `offset` is from it
/// sideways, meters.
pub fn nearest_on_trail(offset: DVec3) -> (f64, f64) {
    let (h, r) = (HALF_STRAIGHT, BEND_RADIUS);
    let (x, z) = (offset.x, offset.z);
    if x > h {
        let a = (x - h).atan2(-z);
        (2.0 * h + a * r, ((x - h).hypot(z) - r).abs())
    } else if x < -h {
        let a = (-h - x).atan2(z);
        (4.0 * h + BEND + a * r, ((x + h).hypot(z) - r).abs())
    } else if z < 0.0 {
        (x + h, (z + r).abs())
    } else {
        (2.0 * h + BEND + (h - x), (z - r).abs())
    }
}

/// The surface material of the trail at `offset` from its origin, or `None` off the path.
/// Lets a landscape paint its ground to match the trail, so anything else walking it (the
/// player, NPCs, rain) finds the same surfaces as the trail's walkers.
pub fn surface_under(offset: DVec3) -> Option<&'static str> {
    let (along, side) = nearest_on_trail(offset);
    (side <= TRAIL_WIDTH as f64 * 0.5).then(|| surface_at(along))
}

/// Where each stretch starts, meters along the trail.
pub fn stretch_start(name: &str) -> Option<f64> {
    let mut s = 0.0;
    for &(n, len) in STRETCHES {
        if n == name {
            return Some(s);
        }
        s += len;
    }
    None
}

/// Someone (or something) walking the trail.
#[derive(Component, Debug, Clone)]
#[require(Transform)]
pub struct TrailWalker {
    /// The trail's origin.
    pub origin: DVec3,
    /// Distance along the loop, meters.
    pub along: f64,
    /// m/s.
    pub speed: f32,
    /// Distance between footfalls (any foot), meters.
    pub step_len: f32,
    /// 2 or 4.
    pub feet: u8,
    pub kind: ContactKind,
    pub mass_kg: f32,
    /// Half the distance between left and right feet, meters.
    pub stance: f32,
    /// Distance between front and hind feet (quadrupeds), meters.
    pub length: f32,
    /// Height of the root above the ground, meters.
    pub height: f32,
    next_foot: u8,
    to_next_step: f32,
}

impl TrailWalker {
    fn new(origin: DVec3, along: f64, kind: ContactKind, mass_kg: f32, speed: f32) -> Self {
        let (step_len, feet, stance, length, height) = match kind {
            ContactKind::Paw => (0.26, 4, 0.07, 0.5, 0.0),
            ContactKind::Hoof => (0.45, 4, 0.15, 1.05, 0.0),
            _ if speed > 2.5 => (1.3, 2, 0.075, 0.0, 0.0),
            _ => (0.72, 2, 0.095, 0.0, 0.0),
        };
        Self {
            origin,
            along,
            speed,
            step_len,
            feet,
            kind,
            mass_kg,
            stance,
            length,
            height,
            next_foot: 0,
            to_next_step: step_len * 0.5,
        }
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<SoundRings>()
        .register_resource::<SoundRings>(
            "Show a coloured ring on the ground for each sound as it plays (enabled): near \
             footstep trails, or everywhere.",
        )
        .add_systems(Update, (walk_trail, bodies::animate).chain())
        .add_systems(
            PostUpdate,
            shift_trails.in_set(engine_world::WorldShiftSet::Apply),
        )
        .add_systems(
            PostUpdate,
            (rings::spawn_rings, rings::grow_rings)
                .chain()
                .in_set(UpdateSet::AfterPropagate),
        );
}

/// The world moved to a new frame: the trails' props move with it (they are plain
/// transforms), so move the walkers' trail origins and the ring areas too. The trail keeps its
/// own axes, so a rotating shift (into a planet's spinning frame) leaves its walkers off the
/// path; floating-origin moves are exact.
fn shift_trails(
    mut shifts: MessageReader<engine_world::WorldShift>,
    mut walkers: Query<&mut TrailWalker>,
    mut rings: ResMut<SoundRings>,
) {
    for shift in shifts.read() {
        if !shift.is_translation_only() {
            engine_core::log::warn!(
                "footstep trail: a rotating world shift moves its walkers off the path"
            );
        }
        for mut w in &mut walkers {
            w.origin = shift.point(w.origin);
        }
        for p in &mut rings.near {
            *p = shift.point(*p);
        }
    }
}

/// Build the footstep trail around `origin` (about 54 × 22 m) with its four walkers, and turn
/// on sound rings.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(|world: &mut World| {
        let mut materials = world.resource_mut::<SurfaceMaterials>();
        if materials.id("gravel").is_none() {
            materials.add(SurfaceMaterial {
                name: "gravel".into(),
                friction: 0.7,
                density: 1_800.0,
                absorbency: 0.15,
                deformation: Deformation::Granular { depth_m: 0.01 },
                footstep_sound: "footstep.gravel".into(),
                impact_effect: "impact.gravel".into(),
                ..Default::default()
            });
        }
    });
    commands.queue(move |world: &mut World| {
        let mut rings = world.resource_mut::<SoundRings>();
        rings.enabled = true;
        rings.near.push(origin);
    });

    ground::spawn(commands, origin);

    // The walkers, spread round the loop.
    bodies::person(
        commands,
        TrailWalker::new(origin, 0.0, ContactKind::Foot, 75.0, 1.4),
        0xb3362c,
        false,
        None,
    );
    bodies::person(
        commands,
        TrailWalker::new(origin, 30.0, ContactKind::Foot, 68.0, 4.0),
        0x2c5fb3,
        true,
        Some(LocalTimeScale::unaffected_by_dilation(1.0)),
    );
    bodies::dog(
        commands,
        TrailWalker::new(origin, 60.0, ContactKind::Paw, 22.0, 1.8),
    );
    bodies::horse(
        commands,
        TrailWalker::new(origin, 86.0, ContactKind::Hoof, 500.0, 1.7),
    );
}

/// Yaw that turns the -Z forward axis to `dir`.
fn yaw(dir: Vec3) -> Quat {
    Quat::from_rotation_y((-dir.x).atan2(-dir.z))
}

/// Move the walkers along the trail and put their feet down where their bodies plant them
/// (see `bodies::gait`).
fn walk_trail(
    time: Res<Time>,
    materials: Res<SurfaceMaterials>,
    mut walkers: Query<(
        Entity,
        &mut TrailWalker,
        &mut Transform,
        Option<&LocalTimeScale>,
    )>,
    mut contacts: MessageWriter<GroundContact>,
) {
    for (entity, mut w, mut t, scale) in &mut walkers {
        let dt = time.delta_for(scale);
        if dt <= 0.0 {
            continue;
        }
        let moved = w.speed * dt;
        w.along = (w.along + moved as f64).rem_euclid(TRAIL_LENGTH);
        let (p, d) = bodies::gait::root(&w);
        t.translation = p;
        t.rotation = yaw(d);

        w.to_next_step -= moved;
        while w.to_next_step <= 0.0 {
            w.to_next_step += w.step_len;
            let foot = w.next_foot;
            w.next_foot = (foot + 1) % w.feet;
            // Feet: left, right for two legs; left fore, right hind, right fore, left hind
            // (a walking quadruped's footfall order) for four. Each lands where the gait puts
            // it relative to where the walker was when it came down: a heel strikes ahead of
            // the hips, a hoof ahead of its shoulder or hip.
            let root_along = w.along + (w.to_next_step - w.step_len) as f64;
            let (position, fd, foot_s) = bodies::gait::footprint(&w, foot, root_along);
            let surface = surface_at(foot_s);
            let load = if w.feet == 2 {
                w.mass_kg
            } else {
                w.mass_kg * 0.5
            };
            contacts.write(GroundContact {
                position,
                normal: Vec3::Y,
                size: match w.kind {
                    ContactKind::Hoof => [0.13, 0.15],
                    ContactKind::Paw => [0.05, 0.06],
                    _ => [0.1, 0.27],
                },
                yaw: (-fd.x).atan2(-fd.z),
                load_kg: load,
                speed: w.speed,
                kind: w.kind,
                surface: materials.id(surface),
                entity: Some(entity),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_is_a_closed_loop() {
        let (a, da) = trail_point(0.0);
        let (b, db) = trail_point(TRAIL_LENGTH - 1e-6);
        assert!(a.distance(b) < 1e-3, "{a} vs {b}");
        assert!(da.distance(db) < 1e-3);
        // Continuous: no jumps anywhere.
        let mut last = trail_point(0.0).0;
        let mut s = 0.05;
        while s < TRAIL_LENGTH {
            let p = trail_point(s).0;
            assert!(p.distance(last) < 0.051, "jump at {s}");
            last = p;
            s += 0.05;
        }
        let total: f64 = STRETCHES.iter().map(|s| s.1).sum();
        assert!((total - TRAIL_LENGTH).abs() < 1e-9);
    }

    #[test]
    fn stretches_are_where_they_say() {
        assert_eq!(surface_at(1.0), "grass");
        assert_eq!(surface_at(13.0), "mud");
        assert_eq!(surface_at(25.0), "snow");
        assert_eq!(
            surface_at(stretch_start("concrete").unwrap() + 1.0),
            "concrete"
        );
        assert_eq!(surface_at(TRAIL_LENGTH - 0.5), "water");
        assert_eq!(surface_at(TRAIL_LENGTH + 1.0), "grass");
    }

    #[test]
    fn nearest_point_matches_the_path() {
        let mut s = 0.0;
        while s < TRAIL_LENGTH {
            let (p, d) = trail_point(s);
            let side = Vec3::new(-d.z, 0.0, d.x).as_dvec3();
            for off in [-1.0, 0.0, 0.9] {
                let (along, dist) = nearest_on_trail(p + side * off);
                let gap = (along - s).abs().min(TRAIL_LENGTH - (along - s).abs());
                assert!(
                    gap < 1e-6 && (dist - off.abs()).abs() < 1e-6,
                    "s {s} off {off}: {along} {dist}"
                );
            }
            assert_eq!(surface_under(p), Some(surface_at(s + 1e-9)));
            s += 0.37;
        }
        assert_eq!(surface_under(DVec3::ZERO), None);
        assert_eq!(surface_under(DVec3::new(0.0, 0.0, -7.0 - 2.0)), None);
    }
}
