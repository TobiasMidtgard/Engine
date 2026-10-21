//! The footstep trail: a loop with one stretch per kind of ground (grass, mud, snow, ice,
//! gravel, a wooden boardwalk, a metal grate, a concrete underpass that echoes, sand, fallen
//! leaves and puddles), walked round and round by a hiker, a runner who ignores slow motion, a
//! dog and a horse.
//!
//! Every footstep is a [`GroundContact`] like a real character's, so the trail tests the
//! whole path: contact → surface → ground state (wet, snowy, burnt) → synthesis → 3D mix.
//! Coloured rings on the ground show each sound as it plays.
//!
//! Every walker asks the animation module for a body
//! ([`BodyRequest`](engine_world::animation::BodyRequest)); once it answers with a
//! `SkeletonPose` that walker's own stand-in body hides and the animated feet write the
//! contacts.
//!
//! ```ignore
//! engine_audio::demo::spawn_demo(&mut commands, zone.center);
//! ```

use std::f64::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::physical::{Deformation, SurfaceMaterial};
use engine_world::{ContactKind, GroundContact, SurfaceMaterials};

mod backdrop;
mod bodies;
mod ground;
mod rings;

pub use backdrop::{lawn_style, meadow_reach, spawn_backdrop};
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

/// The trail's worn ground as a [`engine_world::GroundWear`] provider: the bare earth line it
/// draws down the middle of the grass (worn through, fraying out to the turf 0.45 m either
/// side) and the trench trodden through the snow, so footsteps there sound like the path they
/// are on. A terrain under the trail keeps its own wear map, fed by the walkers' steps; this is
/// for tools that walk the trail without one (`trail_stills --worn`):
/// `app.insert_resource(GroundWear(Arc::new(TrailWear { origin })))`.
#[derive(Debug, Clone, Copy)]
pub struct TrailWear {
    /// The trail's origin, world meters.
    pub origin: DVec3,
}

impl engine_world::WearQuery for TrailWear {
    fn wear(&self, p: DVec3) -> f32 {
        let offset = p - self.origin;
        let (along, _) = nearest_on_trail(offset);
        let (centre, d) = trail_point(along);
        let right = Vec3::new(-d.z, 0.0, d.x).as_dvec3();
        let x = (offset - centre).dot(right) as f32;
        let w = engine_core::math::Vec2::new(offset.x as f32, offset.z as f32);
        ground::drawn_wear(surface_at(along), along, x, w)
    }
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
    /// Where it walks across the trail; walkers step aside to pass each other.
    lane: Lane,
    /// Keeps to the course set with [`TrailWalker::keep_course`] instead of making way.
    kept: bool,
}

/// A walker's sideways place on the trail, meters from the middle (+ toward the inside of the
/// loop, the walker's right), as a move from one place to another over a stretch of the trail.
/// Being a function of distance along the trail, a foot planted before a move keeps its spot.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Lane {
    /// Where along the trail the move starts, meters.
    start: f64,
    /// Meters of travel the move takes.
    len: f64,
    from: f32,
    to: f32,
}

impl Lane {
    fn stay(along: f64) -> Self {
        Self {
            start: along,
            len: 1.0,
            from: 0.0,
            to: 0.0,
        }
    }

    /// How far through the move `along` is (0..1, unclamped), the nearest lap.
    fn progress(&self, along: f64) -> f64 {
        let d =
            (along - self.start + TRAIL_LENGTH * 0.5).rem_euclid(TRAIL_LENGTH) - TRAIL_LENGTH * 0.5;
        d / self.len
    }

    fn at(&self, along: f64) -> f32 {
        let t = self.progress(along).clamp(0.0, 1.0) as f32;
        self.from + (self.to - self.from) * t * t * (3.0 - 2.0 * t)
    }

    /// Sideways meters per meter along the trail.
    fn slope(&self, along: f64) -> f32 {
        let t = self.progress(along).clamp(0.0, 1.0) as f32;
        (self.to - self.from) * 6.0 * t * (1.0 - t) / self.len as f32
    }
}

/// The lane a walker takes to pass someone slower (toward the inside of the loop, away from
/// the follow camera) and the one it moves to while someone faster passes it.
const PASS_LANE: f32 = 0.8;
const GIVE_WAY_LANE: f32 = -0.35;
/// A lane change starts this far ahead of the walker, past every footprint its feet are
/// already headed for, so no swinging foot's target moves.
const LANE_LEAD: f64 = 3.0;

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
            lane: Lane::stay(along),
            kept: false,
        }
    }

    /// Walk `from` meters off the trail's middle (+ toward the inside of the loop) until
    /// `start` meters along the trail, then move over to `to` across the next `len` meters
    /// and stay there, whoever else is about (the walker no longer makes way for anyone).
    /// Films use it to walk someone off the path and back. Set it before the walker's feet
    /// head for `start`, or planted feet move.
    pub fn keep_course(&mut self, start: f64, len: f64, from: f32, to: f32) {
        self.lane = Lane {
            start,
            len: len.max(0.1),
            from,
            to,
        };
        self.kept = true;
    }

    /// Sideways offset from the trail's middle at `along`, meters (+ toward the inside).
    pub fn lane_at(&self, along: f64) -> f32 {
        self.lane.at(along)
    }

    /// How fast the lane changes per meter along the trail at `along`.
    pub fn lane_slope(&self, along: f64) -> f32 {
        self.lane.slope(along)
    }

    /// Half its length along the trail, meters, with some room.
    fn reach(&self) -> f64 {
        match self.kind {
            ContactKind::Hoof => 1.3,
            ContactKind::Paw => 0.5,
            _ => 0.3,
        }
    }

    /// Plan a move to `lane` once the last move is done and a stride behind it (so every
    /// planted foot is past it).
    fn steer_to(&mut self, lane: f32) {
        if self.lane.from != self.lane.to {
            let stride = bodies::gait::stride(self) as f64;
            if self.lane.progress(self.along) < 1.0 + stride / self.lane.len {
                return;
            }
            // Done: hold the new place (a finished move would read as not started once the
            // walker is half a lap past it).
            self.lane = Lane {
                from: self.lane.to,
                ..self.lane
            };
        }
        if (self.lane.to - lane).abs() < 1e-3 {
            return;
        }
        self.lane = Lane {
            start: self.along + LANE_LEAD,
            len: (self.speed as f64 * 0.8).max(2.5),
            from: self.lane.to,
            to: lane,
        };
    }
}

/// Which lane walker `me` wants, from everyone's (along, speed, reach): the passing lane while
/// it closes on and passes someone slower, the give-way lane while someone faster does that to
/// it, else the middle.
fn wanted_lane(me: usize, all: &[(f64, f32, f64)]) -> f32 {
    let (along, speed, reach) = all[me];
    let mut lane = 0.0;
    for (i, &(other, other_speed, other_reach)) in all.iter().enumerate() {
        if i == me {
            continue;
        }
        // How far ahead the other is, the nearest way round.
        let gap =
            (other - along + TRAIL_LENGTH * 0.5).rem_euclid(TRAIL_LENGTH) - TRAIL_LENGTH * 0.5;
        let clear = reach + other_reach + 0.8;
        if speed > other_speed + 0.05 {
            // Catching up: move over in time, as if the other stood still, and stay over
            // until well past.
            let len = (speed as f64 * 0.8).max(2.5);
            if gap > -clear && gap < clear + LANE_LEAD + len {
                return PASS_LANE;
            }
        } else if other_speed > speed + 0.05 {
            let len = (other_speed as f64 * 0.8).max(2.5);
            if gap < clear && gap > -(clear + LANE_LEAD + len) {
                lane = GIVE_WAY_LANE;
            }
        }
    }
    lane
}

impl TrailWalker {
    /// The hiker: two feet at walking pace (the runner is faster). Cameras and recordings
    /// follow this one.
    pub fn is_hiker(&self) -> bool {
        self.feet == 2 && self.speed < 2.0
    }

    /// Where a camera following this walker wants to be (behind and on the outside of the
    /// loop, where the underpass is open) and what it looks at, world meters. Smooth both.
    pub fn follow_view(&self) -> (DVec3, DVec3) {
        let (p, d) = trail_point(self.along);
        let (ahead, _) = trail_point(self.along + 2.5);
        let right = Vec3::new(-d.z, 0.0, d.x).as_dvec3();
        let outward = if p.dot(right) > 0.0 { right } else { -right };
        let eye = self.origin + p - d.as_dvec3() * 4.2 + outward * 3.0 + DVec3::Y * 2.1;
        let look = self.origin + ahead + DVec3::Y * 0.7;
        (eye, look)
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

    // The walkers, spread round the loop so they bunch up within the first minute: the runner
    // passes the dog and the hiker, the horse passes the hiker, and the dog draws level with
    // the hiker at about 60 s, so a film following any of them shows the others.
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
        TrailWalker::new(origin, 92.0, ContactKind::Paw, 22.0, 1.8),
    );
    bodies::horse(
        commands,
        TrailWalker::new(origin, 108.0, ContactKind::Hoof, 500.0, 1.7),
    );
}

/// Yaw that turns the -Z forward axis to `dir`.
fn yaw(dir: Vec3) -> Quat {
    Quat::from_rotation_y((-dir.x).atan2(-dir.z))
}

/// Move the walkers along the trail and put their feet down where their bodies plant them
/// (see `bodies::gait`).
#[allow(clippy::type_complexity)]
fn walk_trail(
    time: Res<Time>,
    materials: Res<SurfaceMaterials>,
    mut walkers: Query<(
        Entity,
        &mut TrailWalker,
        &mut Transform,
        Option<&LocalTimeScale>,
        Has<SkeletonPose>,
    )>,
    mut contacts: MessageWriter<GroundContact>,
) {
    // Make way: everyone picks a lane from where the others are.
    let all: Vec<(f64, f32, f64)> = walkers
        .iter()
        .map(|(_, w, _, _, _)| (w.along, w.speed, w.reach()))
        .collect();
    for (i, (_, mut w, _, _, _)) in walkers.iter_mut().enumerate() {
        if !w.kept {
            let lane = wanted_lane(i, &all);
            w.steer_to(lane);
        }
    }
    for (entity, mut w, mut t, scale, dressed) in &mut walkers {
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
            // A body from the animation module puts its own feet down.
            if dressed {
                continue;
            }
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
    fn the_trail_is_worn_where_it_is_drawn_bare() {
        use engine_world::WearQuery;
        let origin = DVec3::new(100.0, 5.0, -40.0);
        let wear = TrailWear { origin };
        let at = |stretch: &str, into: f64, x: f64| {
            let (p, d) = trail_point(stretch_start(stretch).unwrap() + into);
            let right = DVec3::new(-d.z as f64, 0.0, d.x as f64);
            wear.wear(origin + p + right * x)
        };
        // The earth line down the middle of the grass, the turf beside it.
        assert!(at("grass", 6.0, 0.0) > 0.99);
        assert!(at("grass", 6.0, -0.05) > 0.99);
        let edge = at("grass", 6.0, 0.3);
        assert!(edge > 0.05 && edge < 0.6, "{edge}");
        assert_eq!(at("grass", 6.0, 0.9), 0.0);
        assert!(at("grass", 0.2, 0.0) < 0.01);
        // The trench through the snow, none on the gravel.
        assert!(at("snow", 6.0, 0.0) > 0.5, "{}", at("snow", 6.0, 0.0));
        assert!(at("snow", 6.0, 0.0) > at("snow", 6.0, 1.7));
        assert_eq!(at("gravel", 2.0, 0.0), 0.0);
    }

    #[test]
    fn a_kept_course_walks_off_the_earth_line_and_onto_it() {
        use engine_world::WearQuery;
        let wear = TrailWear {
            origin: DVec3::ZERO,
        };
        let s0 = stretch_start("grass").unwrap();
        let mut w = TrailWalker::new(DVec3::ZERO, s0 + 1.0, ContactKind::Foot, 75.0, 1.4);
        w.keep_course(s0 + 3.0, 2.0, 0.65, 0.0);
        // Under each foot, a step's length apart: the meadow beside the line, then the line.
        let under = |w: &TrailWalker, along: f64| {
            let (p, _, _) = bodies::gait::footprint(w, 0, along);
            let (q, _, _) = bodies::gait::footprint(w, 1, along);
            wear.wear(p).max(wear.wear(q))
        };
        assert_eq!(under(&w, s0 + 1.5), 0.0);
        assert_eq!(under(&w, s0 + 2.0), 0.0);
        assert!(under(&w, s0 + 5.5) > 0.95, "{}", under(&w, s0 + 5.5));
        // Others coming by don't move it: it no longer makes way.
        assert!(w.kept);
        // And back off it.
        w.keep_course(s0 + 7.0, 2.0, 0.0, 0.65);
        assert!(under(&w, s0 + 6.0) > 0.95);
        assert_eq!(under(&w, s0 + 9.5), 0.0);
    }

    #[test]
    fn walkers_step_aside_to_pass() {
        // The hiker, with a horse, a runner or a dog coming up 8 m behind.
        for (kind, mass, speed) in [
            (ContactKind::Hoof, 500.0, 1.7),
            (ContactKind::Foot, 68.0, 4.0),
            (ContactKind::Paw, 22.0, 1.8),
        ] {
            let mut ws = [
                TrailWalker::new(DVec3::ZERO, 10.0, ContactKind::Foot, 75.0, 1.4),
                TrailWalker::new(DVec3::ZERO, 2.0, kind, mass, speed),
            ];
            let dt = 1.0 / 30.0;
            let (mut closest, mut widest, mut aside) = (f64::MAX, 0.0f32, 0.0f32);
            for _ in 0..(90.0 / dt) as usize {
                let all: Vec<_> = ws.iter().map(|w| (w.along, w.speed, w.reach())).collect();
                for (i, w) in ws.iter_mut().enumerate() {
                    w.steer_to(wanted_lane(i, &all));
                }
                for w in &mut ws {
                    w.along = (w.along + (w.speed * dt) as f64).rem_euclid(TRAIL_LENGTH);
                }
                // The passer's body runs from its tail to its nose along its heading.
                let (hiker, _) = bodies::gait::root(&ws[0]);
                let (p, d) = bodies::gait::root(&ws[1]);
                let half = (ws[1].reach() - 0.2).max(0.1);
                let (a, b) = (p - d.as_dvec3() * half, p + d.as_dvec3() * half);
                let t = ((hiker - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
                closest = closest.min(hiker.distance(a + (b - a) * t));
                widest = widest.max(ws[1].lane_at(ws[1].along));
                aside = aside.min(ws[0].lane_at(ws[0].along));
                // Far apart, both walk the middle.
                let gap = (ws[1].along - ws[0].along + TRAIL_LENGTH * 0.5).rem_euclid(TRAIL_LENGTH)
                    - TRAIL_LENGTH * 0.5;
                if gap.abs() > 20.0 {
                    for w in &ws {
                        assert_eq!(w.lane_at(w.along), 0.0, "{kind:?} at gap {gap}");
                    }
                }
            }
            assert!(
                closest > 0.8,
                "{kind:?} came within {closest:.2} m of the hiker"
            );
            assert!(
                widest > PASS_LANE * 0.99,
                "{kind:?} never moved over: {widest}"
            );
            assert!(
                aside < GIVE_WAY_LANE * 0.99,
                "the hiker never gave way: {aside}"
            );
        }
    }

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
