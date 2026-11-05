//! A timber watchtower down range of the catapult, for its ball to bring down.
//!
//! Two lifts of four posts with rails between them, a plank deck on top with a railing and the
//! lookout's crates on it. Every timber is its own rigid body, pegged to the next with a fixed
//! joint that snaps when the force through it passes its `break_force` or the bending through
//! it passes its [`BreakTorque`] (as PhysX's and Unity's breakable joints do; Chaos breaks the
//! bonds of a connection graph on strain the same way). Standing, each joint holds a small part
//! of its limits. The ball strikes the corner post that faces the catapult: the pegs round the
//! blow snap, the post is knocked in, and the deck's weight, now on three posts, bends and
//! breaks the rest, so the tower comes down in its separate timbers.
//!
//! The tower is turned 45° so its corner post faces the catapult, where the ball flies.

use std::f32::consts::FRAC_PI_4;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_world::physics::{Collider, JointKind};

use super::Yard;
use super::art::{Mat, timber};
use crate::BreakTorque;

/// The posts' centers stand this far either side of the tower's middle, meters.
pub const HALF: f32 = 0.9;
/// The posts' half width, meters (12 cm square).
pub const POST: f32 = 0.06;
/// Each lift's height, meters; there are two, the deck on the second.
pub const LIFT: f32 = 2.5;
pub const LIFTS: usize = 2;
/// The rails' half width, meters, and their heights in each lift (the last flush with its
/// top: the girt the next lift or the deck stands on).
pub const RAIL: f32 = 0.05;
pub const RAIL_HEIGHTS: [f32; 3] = [0.85, 1.7, LIFT - RAIL];
/// The deck's half extents, meters: a little past the posts.
pub const DECK: Vec3 = Vec3::new(1.05, 0.04, 1.05);
/// The deck railing's posts (half extents) and its rail's height over the deck, meters.
pub const RAILING_POST: Vec3 = Vec3::new(0.05, 0.5, 0.05);
pub const RAILING_HEIGHT: f32 = 0.95;
/// The lookout's crates on the deck (full of stores), kg/m³.
pub const STORES_DENSITY: f32 = 600.0;
/// Pine, kg/m³.
pub const DENSITY: f32 = 550.0;

/// Breaking limits (N, N·m) of the pegs: rail ends, post to post, posts in their footings,
/// the deck on the posts and the railing on the deck.
pub const RAIL_PEG: (f32, f32) = (2_500.0, 450.0);
pub const POST_PEG: (f32, f32) = (12_000.0, 2_500.0);
pub const FOOTING: (f32, f32) = (15_000.0, 3_000.0);
pub const DECK_PEG: (f32, f32) = (9_000.0, 2_000.0);
pub const RAILING_PEG: (f32, f32) = (3_000.0, 500.0);

/// The tower's turn about the vertical: a corner toward -X, where the catapult stands.
pub fn turn() -> Quat {
    Quat::from_rotation_y(FRAC_PI_4)
}

/// The corners' (x, z) in the tower's own frame, going round: the first faces -X once turned.
pub fn corners() -> [Vec3; 4] {
    [
        Vec3::new(-HALF, 0.0, -HALF),
        Vec3::new(HALF, 0.0, -HALF),
        Vec3::new(HALF, 0.0, HALF),
        Vec3::new(-HALF, 0.0, HALF),
    ]
}

/// Every timber of the tower in its own frame (origin on the ground under its middle):
/// (name, center, rotation, half extents).
pub fn timbers() -> Vec<(String, Vec3, Quat, Vec3)> {
    let mut out = Vec::new();
    let c = corners();
    for lift in 0..LIFTS {
        let floor = lift as f32 * LIFT;
        for (k, at) in c.iter().enumerate() {
            out.push((
                format!("post {lift} {k}"),
                *at + Vec3::Y * (floor + LIFT * 0.5),
                Quat::IDENTITY,
                Vec3::new(POST, LIFT * 0.5, POST),
            ));
        }
        for k in 0..4 {
            let (a, b) = (c[k], c[(k + 1) % 4]);
            let along = (b - a).normalize();
            // Rails run along their local X.
            let rotation = Quat::from_rotation_arc(Vec3::X, along);
            for (r, h) in RAIL_HEIGHTS.iter().enumerate() {
                out.push((
                    format!("rail {lift} {k} {r}"),
                    (a + b) * 0.5 + Vec3::Y * (floor + h),
                    rotation,
                    Vec3::new(HALF - POST - 0.002, RAIL, RAIL),
                ));
            }
        }
    }
    let top = LIFTS as f32 * LIFT;
    out.push((
        "deck".into(),
        Vec3::Y * (top + DECK.y + 0.001),
        Quat::IDENTITY,
        DECK,
    ));
    let deck_top = top + 2.0 * DECK.y + 0.001;
    for (k, at) in c.iter().enumerate() {
        out.push((
            format!("railing post {k}"),
            *at + Vec3::Y * (deck_top + RAILING_POST.y + 0.001),
            Quat::IDENTITY,
            RAILING_POST,
        ));
    }
    for k in 0..4 {
        let (a, b) = (c[k], c[(k + 1) % 4]);
        out.push((
            format!("railing {k}"),
            (a + b) * 0.5 + Vec3::Y * (deck_top + RAILING_HEIGHT),
            Quat::from_rotation_arc(Vec3::X, (b - a).normalize()),
            Vec3::new(HALF - RAILING_POST.x - 0.002, 0.04, 0.04),
        ));
    }
    out
}

/// Build the tower with its middle on the ground at `middle` (yard-local).
pub(super) fn watchtower(y: &mut Yard, middle: DVec3) {
    let turn = turn();
    let place = |p: Vec3| middle + (turn * p).as_dvec3();
    let mut bodies = std::collections::HashMap::new();
    for (i, (name, at, rotation, half)) in timbers().into_iter().enumerate() {
        let seed = (i % 5) as u32;
        let key = format!(
            "watchtower/{:.3}x{:.3}x{:.3}/{seed}",
            half.x, half.y, half.z
        );
        let parts = y.parts(&key, [Mat::Timber], move || {
            [timber::timber_beam(half, seed)]
        });
        let c = Collider::cuboid(half)
            .with_surface(y.wood)
            .with_density(DENSITY);
        let e = y.body(place(at), turn * rotation, c, parts);
        y.name(e, &format!("Watchtower {name}"));
        bodies.insert(name, e);
    }
    let body = |name: &str| bodies[name];
    let c = corners();
    let peg = |y: &mut Yard, a, b: Option<_>, at: Vec3, (force, torque): (f32, f32)| {
        let j = y.joint(JointKind::Fixed, a, b, place(at));
        y.world
            .get_mut::<engine_world::physics::Joint>(j)
            .unwrap()
            .break_force = Some(force);
        y.world.entity_mut(j).insert(BreakTorque(torque));
    };
    for lift in 0..LIFTS {
        let floor = lift as f32 * LIFT;
        for (k, at) in c.iter().enumerate() {
            let post = body(&format!("post {lift} {k}"));
            if lift == 0 {
                peg(y, post, None, *at, FOOTING);
            } else {
                let below = body(&format!("post {} {k}", lift - 1));
                peg(y, post, Some(below), *at + Vec3::Y * floor, POST_PEG);
            }
        }
        for k in 0..4 {
            let (a, b) = (c[k], c[(k + 1) % 4]);
            let along = (b - a).normalize();
            for (r, h) in RAIL_HEIGHTS.iter().enumerate() {
                let rail = body(&format!("rail {lift} {k} {r}"));
                for (end, corner) in [(a, k), (b, (k + 1) % 4)] {
                    let toward = if corner == k { along } else { -along };
                    let at = end + toward * (POST + 0.002) + Vec3::Y * (floor + h);
                    let post = body(&format!("post {lift} {corner}"));
                    peg(y, rail, Some(post), at, RAIL_PEG);
                }
            }
        }
    }
    let top = LIFTS as f32 * LIFT;
    let deck = body("deck");
    for (k, at) in c.iter().enumerate() {
        let post = body(&format!("post {} {k}", LIFTS - 1));
        peg(y, deck, Some(post), *at + Vec3::Y * top, DECK_PEG);
        let railing_post = body(&format!("railing post {k}"));
        let deck_top = top + 2.0 * DECK.y + 0.001;
        peg(
            y,
            railing_post,
            Some(deck),
            *at + Vec3::Y * deck_top,
            RAILING_PEG,
        );
    }
    let deck_top = top + 2.0 * DECK.y + 0.001;
    for k in 0..4 {
        let (a, b) = (c[k], c[(k + 1) % 4]);
        let along = (b - a).normalize();
        let rail = body(&format!("railing {k}"));
        for (end, corner, toward) in [(a, k, along), (b, (k + 1) % 4, -along)] {
            let at =
                end + toward * (RAILING_POST.x + 0.002) + Vec3::Y * (deck_top + RAILING_HEIGHT);
            let post = body(&format!("railing post {corner}"));
            peg(y, rail, Some(post), at, RAILING_PEG);
        }
    }
    // The lookout's stores: crates on the deck, loose.
    let crate_half = Vec3::splat(0.25);
    let stores = [(0.4f32, 0.4f32), (-0.4, -0.4), (0.4, -0.4), (-0.4, 0.4)];
    for (k, (x, z)) in stores.into_iter().enumerate() {
        let at = place(Vec3::new(x, deck_top + crate_half.y + 0.003, z));
        let e = y.crate_of(
            at,
            crate_half,
            turn * Quat::from_rotation_y(k as f32 * 0.2),
            (k * 3 + 1) as u32,
            STORES_DENSITY,
        );
        y.name(e, "Watchtower crate");
    }
}
