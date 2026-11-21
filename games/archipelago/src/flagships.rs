//! The flagships: three craft at the very top of the yard's list, each the biggest, the most
//! powerful and by far the dearest of its kind.
//!
//! - The **Dreadnought** is an armoured battleship, 150 m, after the all-big-gun ships of
//!   1906 to 1915: five twin turrets (two forward, one superfiring, one amidships, two aft),
//!   tripod masts, two great funnels and a belt of armour. Her main battery alone outguns a
//!   rival house's whole navy ([`battery`]), she sees 3 km, and her convoy ring makes every
//!   ship of her company within [`ESCORT_M`] sail a third faster ([`update`]).
//! - The **Leviathan** is a giant iron steamship, 180 m, after the Great Eastern of 1858:
//!   five funnels, six masts, side paddles as tall as a house, two tiers of portholes. Eight
//!   holds of 80 (the freighter has four of 50), and she loads and lands half again as fast.
//! - The **Sky carrier** is a 220 m rigid airship after the American carriers of the 1930s,
//!   which launched and caught little fighters on a trapeze through a hangar in the belly.
//!   Two escort biplanes fly round her wherever she goes ([`update`]); she sees 6 km over the
//!   fog, flies straight over land and carries four holds of 60.
//!
//! Prices, tiers and upkeep are in [`crate::shipyard::PLANS`]; the models are here so the
//! yard, the review line and the tours all draw the same ships.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::draw::{Art, Body, spawn_model};
use crate::fleet::Vessel;
use crate::vessels::{Craft, hull, mast, shrouds, square_sail, strake};

/// How far a dreadnought's convoy ring reaches, meters.
pub const ESCORT_M: f32 = 1_500.0;
/// How much faster her company's ships sail inside it.
pub const ESCORT_SPEED: f32 = 0.33;
/// Escort planes round a sky carrier, their circle's radius and height over her, meters.
const WINGS: u32 = 2;
const WING_R: f32 = 170.0;
const WING_UP: f32 = 28.0;
/// Their speed round the circle, m/s.
const WING_SPEED: f32 = 42.0;

/// The guns a craft carries built in, before fittings: damage a minute and reach, meters.
pub fn battery(c: Craft) -> (f32, f32) {
    match c {
        // Ten 13.5-inch guns and sixteen 4-inch: about 400 (a frigate fitted for war has
        // 46, House Ostrava's whole navy 260).
        Craft::Dreadnought => (420.0, 1_800.0),
        // The escort planes.
        Craft::SkyCarrier => (60.0, 900.0),
        _ => (0.0, 0.0),
    }
}

/// Hull points of a bare flagship (a frigate has 320).
pub fn hull_points(c: Craft) -> Option<f32> {
    Some(match c {
        Craft::Dreadnought => 2_400.0,
        Craft::Leviathan => 1_400.0,
        Craft::SkyCarrier => 600.0,
        _ => return None,
    })
}

/// What a flagship adds to its kit by herself: sight, loading.
pub fn innate(c: Craft, kit: &mut crate::gear::Kit) {
    match c {
        // 1.8 km × 1.7: about 3 km.
        Craft::Dreadnought => kit.sight += 0.7,
        // Her own cranes and a crew of hundreds.
        Craft::Leviathan => {
            kit.loads = (kit.loads + 0.5).min(0.8);
            kit.sight += 0.25;
        }
        // 2.6 km × 2.4: about 6 km over the fog.
        Craft::SkyCarrier => kit.sight += 1.4,
        _ => {}
    }
}

/// Names for new flagships, taken in turn.
pub fn names(c: Craft) -> &'static [&'static str] {
    match c {
        Craft::Dreadnought => &[
            "Stormcrown",
            "Iron Duchess",
            "Thunder of the Sound",
            "Bulwark",
        ],
        Craft::Leviathan => &[
            "Great Meridian",
            "Queen of the Sound",
            "Colossus",
            "Far Horizon",
        ],
        Craft::SkyCarrier => &["Cloud Citadel", "Skyhold", "Halcyon", "High Admiral"],
        _ => &[],
    }
}

// ------------------------------------------------------------------ powers at work

/// One of a sky carrier's escort planes (not a vessel: it costs nothing and counts nowhere).
#[derive(Component, Clone, Copy, Debug)]
pub struct Wing {
    pub carrier: Entity,
    /// Where on its circle it is, radians.
    pub angle: f32,
}

/// The convoy rings and the escort planes, every frame.
pub fn update(world: &mut World) {
    convoy(world);
    wings(world);
}

/// Every ship of a dreadnought's company within [`ESCORT_M`] of her sails faster.
fn convoy(world: &mut World) {
    let mut q = world.query::<(&Vessel, &Transform)>();
    let rings: Vec<(crate::sim::Company, DVec3)> = q
        .iter(world)
        .filter(|(v, _)| v.craft == Craft::Dreadnought && v.owned())
        .map(|(v, t)| (v.company, t.translation))
        .collect();
    let mut q = world.query::<(&mut Vessel, &Transform)>();
    for (mut v, t) in q.iter_mut(world) {
        let near = !v.craft.flies()
            && v.craft != Craft::Dreadnought
            && v.owned()
            && rings
                .iter()
                .any(|(c, p)| *c == v.company && p.distance(t.translation) < ESCORT_M as f64);
        let want = if near { ESCORT_SPEED } else { 0.0 };
        if v.escort != want {
            v.escort = want;
        }
    }
}

/// Keep [`WINGS`] planes flying round every sky carrier, and take them away with her.
fn wings(world: &mut World) {
    let mut q = world.query::<(Entity, &Vessel, &Transform)>();
    let carriers: Vec<(Entity, Transform)> = q
        .iter(world)
        .filter(|(_, v, _)| v.craft == Craft::SkyCarrier)
        .map(|(e, _, t)| (e, *t))
        .collect();
    let mut q = world.query::<(Entity, &Wing, &Body)>();
    let have: Vec<(Entity, Wing, Vec<Entity>)> = q
        .iter(world)
        .map(|(e, w, b)| (e, *w, b.parts.clone()))
        .collect();
    // Gone with their carrier.
    for (e, w, parts) in &have {
        if !carriers.iter().any(|(c, _)| *c == w.carrier) {
            for p in parts {
                if world.get_entity(*p).is_ok() {
                    world.despawn(*p);
                }
            }
            world.despawn(*e);
        }
    }
    if carriers.is_empty() {
        return;
    }
    // New carriers get theirs.
    if let Some(art) = world.get_resource::<Art>().cloned()
        && let Some(model) = art.crafts.get(&Craft::Biplane).cloned()
    {
        for (c, t) in &carriers {
            let n = have.iter().filter(|(_, w, _)| w.carrier == *c).count() as u32;
            for k in n..WINGS {
                let parts = spawn_model(world, &art, &model, *t, true);
                world.spawn((
                    Name::new("Escort fighter"),
                    *t,
                    Wing {
                        carrier: *c,
                        angle: k as f32 * TAU / WINGS as f32,
                    },
                    Body { parts },
                ));
            }
        }
    }
    let dt = world.resource::<Time>().delta_secs();
    let gdt = dt * world.resource::<crate::sim::Economy>().speed;
    let rate = WING_SPEED / WING_R;
    let mut q = world.query::<(&mut Wing, &mut Transform)>();
    for (mut w, mut t) in q.iter_mut(world) {
        let Some((_, ct)) = carriers.iter().find(|(c, _)| *c == w.carrier) else {
            continue;
        };
        w.angle = (w.angle + rate * gdt) % TAU;
        let a = w.angle;
        // In the carrier's own space: round her, rising and falling a little.
        let local = v3(
            a.cos() * WING_R,
            WING_UP + (a * 2.0).sin() * 12.0,
            a.sin() * WING_R,
        );
        t.translation = ct.translation + (ct.rotation * local).as_dvec3();
        t.rotation =
            ct.rotation * Quat::from_rotation_y(-(a + FRAC_PI_2)) * Quat::from_rotation_x(0.45);
    }
}

// ------------------------------------------------------------------ the models

/// Deck height at `x` of a hull from [`hull`] (its sheer rises toward the ends).
fn deck_at(x: f32, len: f32, free: f32) -> f32 {
    let u = 2.0 * x / len;
    free + 0.5 * free * u * u - 0.25
}

/// Half beam at `x` of a hull from [`hull`], at the deck edge.
fn half_at(x: f32, len: f32, beam: f32, fine: f32) -> f32 {
    let t = x / len + 0.5;
    let s = if t > 0.5 {
        1.0 - ((t - 0.5) * 2.0).powf(fine)
    } else {
        1.0 - ((0.5 - t) * 2.0).powi(3) * 0.75
    };
    beam * 0.5 * s.max(0.0).sqrt().max(0.02)
}

/// A flag on a staff at `at`, flying aft (-X), `w` × `h`.
fn flag(p: &mut Parts, at: Vec3, staff: f32, w: f32, h: f32, col: Vec4, stripe: Vec4) {
    p.rod(
        Mat::Timber,
        at,
        at + v3(0.0, staff, 0.0),
        0.09,
        6,
        hexc(0xd8d0c0),
    );
    let top = at.y + staff;
    p.cuboid(
        Mat::Canvas,
        v3(at.x - w, top - h, at.z - 0.04),
        v3(at.x, top, at.z + 0.04),
        col,
        true,
    );
    p.cuboid(
        Mat::Canvas,
        v3(at.x - w - 0.01, top - h * 0.62, at.z - 0.06),
        v3(at.x + 0.01, top - h * 0.38, at.z + 0.06),
        stripe,
        true,
    );
}

/// A long pennant streaming aft from `at`.
fn pennant(p: &mut Parts, at: Vec3, len: f32, col: Vec4) {
    for s in [1.0f32, -1.0] {
        let z = s * 0.03;
        let (a, b, c) = (
            at + v3(0.0, 0.0, z),
            at + v3(0.0, -0.9, z),
            at + v3(-len, -0.8, z),
        );
        if s > 0.0 {
            p.tri(Mat::Canvas, a, b, c, col);
        } else {
            p.tri(Mat::Canvas, b, a, c, col);
        }
    }
}

/// A twin gun turret on its barbette, standing at deck height 0, guns toward +X. `raise`
/// lifts it on a taller barbette (superfiring), `k` scales it (1: 13.5-inch, 0.45: 6-inch).
fn turret(raise: f32, k: f32, grey: Vec4) -> Parts {
    let mut t = Parts::default();
    let steel = shade(grey, 0.82);
    t.cylinder(
        Mat::Iron,
        v3(-0.6 * k, -0.6, 0.0),
        5.0 * k,
        5.0 * k,
        raise + 1.6 * k + 0.6,
        20,
        steel,
        true,
    );
    let y = raise + 1.6 * k;
    let (x0, x1, x2, w, h) = (-4.8 * k, 2.4 * k, 4.4 * k, 4.0 * k, 3.2 * k);
    // The gunhouse: a box with a sloped face plate.
    t.cuboid(Mat::Iron, v3(x0, y, -w), v3(x1, y + h, w), grey, false);
    t.quad(
        Mat::Iron,
        v3(x2, y, w),
        v3(x2, y, -w),
        v3(x1, y + h, -w),
        v3(x1, y + h, w),
        shade(grey, 1.08),
    );
    t.tri(
        Mat::Iron,
        v3(x1, y, w),
        v3(x2, y, w),
        v3(x1, y + h, w),
        grey,
    );
    t.tri(
        Mat::Iron,
        v3(x2, y, -w),
        v3(x1, y, -w),
        v3(x1, y + h, -w),
        grey,
    );
    // Sighting hoods on the roof, the rangefinder across the back.
    for z in [-0.55f32, 0.55] {
        t.cuboid(
            Mat::Iron,
            v3(x1 - 1.6 * k, y + h, z * w - 0.5 * k),
            v3(x1 - 0.4 * k, y + h + 0.6 * k, z * w + 0.5 * k),
            steel,
            false,
        );
    }
    t.cuboid(
        Mat::Iron,
        v3(x0 + 0.4 * k, y + h - 0.2 * k, -w - 1.6 * k),
        v3(x0 + 1.5 * k, y + h + 0.7 * k, w + 1.6 * k),
        steel,
        true,
    );
    // Two guns in canvas blast bags.
    for z in [-1.7f32 * k, 1.7 * k] {
        let gy = y + h * 0.45;
        t.rod(
            Mat::Canvas,
            v3(x1 + 0.4 * k, gy, z),
            v3(x2 + 0.8 * k, gy + 0.03, z),
            0.85 * k,
            10,
            hexc(0x9a9488),
        );
        t.rod(
            Mat::Iron,
            v3(x2, gy, z),
            v3(x2 + 17.0 * k, gy + 0.3 * k, z),
            0.42 * k,
            10,
            shade(grey, 0.7),
        );
        t.rod(
            Mat::Iron,
            v3(x2 + 16.0 * k, gy + 0.28 * k, z),
            v3(x2 + 17.2 * k, gy + 0.3 * k, z),
            0.52 * k,
            10,
            shade(grey, 0.6),
        );
    }
    t
}

/// An armoured battleship: grey hull with a dark belt over red, five twin turrets (A and B
/// forward, B superfiring, Q amidships, X superfiring and Y aft), a tripod foremast over the
/// bridge, two great funnels, a tripod mainmast, casemate guns along both sides, boats and
/// cranes on the shelter deck, searchlights and the company's colours at every masthead.
pub fn dreadnought() -> Parts {
    let mut p = Parts::default();
    let (len, beam, free, fine) = (150.0f32, 22.0f32, 6.0f32, 2.2f32);
    let grey = hexc(0x8a929a);
    let dark = hexc(0x4c535a);
    let light = hexc(0x969ea6);
    let h = hull(
        &mut p,
        Mat::Paint,
        len,
        beam,
        8.0,
        free,
        fine,
        grey,
        hexc(0x6a2418),
        0.0,
    );
    let deck = |x: f32| deck_at(x, len, free);
    let half = |x: f32| half_at(x, len, beam, fine);
    // Boot-topping, the armour belt and a row of scuttles above it.
    strake(&mut p, &h, -73.0, 73.0, -0.1, 0.7, hexc(0x141517));
    strake(&mut p, &h, -50.0, 56.0, 0.7, 3.4, dark);
    for (n, s) in [(Vec3::Z, 1.0f32), (-Vec3::Z, -1.0)] {
        for (x0, x1) in [(-62.0f32, -36.0f32), (34.0, 60.0)] {
            let x = (x0 + x1) * 0.5;
            p.windows(
                v3(x, 4.6, s * (half(x) + 0.02)),
                n,
                9,
                0.45,
                0.45,
                2.4,
                hexc(0x3a3e42),
            );
        }
    }
    // Anchors in their hawse pipes.
    for s in [1.0f32, -1.0] {
        let x = 64.0;
        let z = s * (half(x) + 0.1);
        p.cuboid(
            Mat::Iron,
            v3(x - 0.9, 4.2, z - 0.25),
            v3(x + 0.9, 6.4, z + 0.25),
            hexc(0x1a1a1c),
            true,
        );
    }
    // The five main turrets.
    for (x, raise, aft) in [
        (51.0f32, 0.0f32, false),
        (37.0, 4.2, false),
        (-4.0, 1.0, true),
        (-44.0, 4.2, true),
        (-58.0, 0.0, true),
    ] {
        let t = turret(raise, 1.0, grey);
        let yaw = if aft { PI } else { 0.0 };
        p.add_parts(
            &t,
            Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), v3(x, deck(x), 0.0)),
        );
    }
    // Casemate guns along both sides under the shelter deck.
    for s in [1.0f32, -1.0] {
        for i in 0..8 {
            let x = -30.0 + i as f32 * 8.2;
            let z = s * half(x);
            let y = deck(x) - 1.6;
            p.cuboid(
                Mat::Paint,
                v3(x - 1.0, y - 0.7, z - 0.05 * s - 0.03),
                v3(x + 1.0, y + 0.7, z - 0.05 * s + 0.03),
                hexc(0x2a2d30),
                false,
            );
            p.rod(
                Mat::Iron,
                v3(x, y, z * 0.95),
                v3(x + 1.2, y + 0.1, z + s * 4.6),
                0.18,
                6,
                hexc(0x3a3e42),
            );
        }
    }
    // The shelter deck: forward and aft casings round the funnels.
    let casing = |p: &mut Parts, x0: f32, x1: f32, w: f32| {
        let y = deck((x0 + x1) * 0.5);
        p.cuboid(
            Mat::Paint,
            v3(x0, y - 0.3, -w),
            v3(x1, y + 3.0, w),
            grey,
            false,
        );
        p.cuboid(
            Mat::Planks,
            v3(x0 - 0.2, y + 3.0, -w - 0.2),
            v3(x1 + 0.2, y + 3.15, w + 0.2),
            hexc(0xc8a878),
            false,
        );
        for (n, z) in [(Vec3::Z, w + 0.01), (-Vec3::Z, -w - 0.01)] {
            p.windows(
                v3((x0 + x1) * 0.5, y + 1.5, z),
                n,
                ((x1 - x0) / 3.0) as u32,
                0.6,
                0.6,
                2.4,
                hexc(0x4a4e52),
            );
        }
    };
    casing(&mut p, 2.0, 30.0, 8.0);
    casing(&mut p, -34.0, -11.0, 7.0);
    // Secondary turrets on the casings' corners.
    for (x, z, aft) in [
        (27.0f32, 5.0f32, false),
        (27.0, -5.0, false),
        (-31.0, 4.2, true),
        (-31.0, -4.2, true),
    ] {
        let t = turret(0.0, 0.42, light);
        // Trained out on the beam a little.
        let yaw = if aft {
            PI + z.signum() * 0.5
        } else {
            -z.signum() * 0.5
        };
        p.add_parts(
            &t,
            Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), v3(x, deck(x) + 3.15, z)),
        );
    }
    // The bridge: conning tower, two tiers of bridge house, and wings.
    let bd = deck(22.0) + 3.15;
    p.cylinder(Mat::Iron, v3(29.5, bd, 0.0), 2.6, 2.6, 4.6, 16, dark, true);
    p.windows(
        v3(32.12, bd + 3.8, 0.0),
        Vec3::X,
        3,
        0.9,
        0.35,
        0.3,
        hexc(0x2a2d30),
    );
    for (i, (x0, x1, w)) in [(15.0f32, 28.0f32, 5.5f32), (17.0, 27.0, 4.5)]
        .into_iter()
        .enumerate()
    {
        let y = bd + i as f32 * 3.0;
        p.cuboid(Mat::Paint, v3(x0, y, -w), v3(x1, y + 3.0, w), light, false);
        p.windows(
            v3(x1 + 0.01, y + 1.8, 0.0),
            Vec3::X,
            (w * 1.3) as u32,
            0.8,
            0.9,
            0.35,
            hexc(0x3a3e42),
        );
    }
    let top = bd + 6.0;
    p.cuboid(
        Mat::Planks,
        v3(21.0, top, -10.5),
        v3(27.5, top + 0.25, 10.5),
        hexc(0xc8a878),
        true,
    );
    p.cuboid(
        Mat::Paint,
        v3(23.5, top + 0.25, -3.2),
        v3(27.2, top + 2.6, 3.2),
        light,
        false,
    );
    p.windows(
        v3(27.21, top + 1.6, 0.0),
        Vec3::X,
        5,
        0.9,
        1.0,
        0.25,
        hexc(0x2a2d30),
    );
    // The tripod foremast and its spotting top.
    let fx = 21.0;
    let tall = top + 32.0;
    p.rod(
        Mat::Iron,
        v3(fx, top, 0.0),
        v3(fx, tall, 0.0),
        0.75,
        10,
        light,
    );
    for z in [-4.5f32, 4.5] {
        p.rod(
            Mat::Iron,
            v3(fx - 6.0, top, z),
            v3(fx, tall - 6.0, 0.0),
            0.5,
            8,
            light,
        );
    }
    p.cylinder(
        Mat::Paint,
        v3(fx, tall - 3.0, 0.0),
        3.6,
        3.4,
        3.2,
        16,
        light,
        true,
    );
    // Its slit all round.
    p.cylinder(
        Mat::Paint,
        v3(fx, tall - 1.9, 0.0),
        3.52,
        3.48,
        0.7,
        16,
        hexc(0x1e2124),
        false,
    );
    p.rod(
        Mat::Iron,
        v3(fx, tall, 0.0),
        v3(fx, tall + 14.0, 0.0),
        0.28,
        8,
        light,
    );
    p.rod(
        Mat::Iron,
        v3(fx, tall + 7.0, -7.0),
        v3(fx, tall + 7.0, 7.0),
        0.16,
        6,
        light,
    );
    // Searchlight platform below the top, lamps lit.
    p.cylinder(
        Mat::Planks,
        v3(fx, tall - 12.0, 0.0),
        4.2,
        4.2,
        0.25,
        16,
        hexc(0xc8a878),
        true,
    );
    for z in [-3.0f32, 3.0] {
        p.cylinder(
            Mat::Lamp,
            v3(fx + 1.5, tall - 11.75, z),
            0.7,
            0.7,
            1.0,
            10,
            hexc(0xfff2d0),
            true,
        );
    }
    // Two great funnels with black caps and steam pipes, ringed with the company's band.
    for x in [10.0f32, -22.0] {
        let y = deck(x) + 3.15;
        let mut f = Parts::default();
        f.cylinder(Mat::Paint, Vec3::ZERO, 3.6, 3.4, 20.0, 20, grey, false);
        f.cylinder(
            Mat::Paint,
            v3(0.0, 14.5, 0.0),
            3.47,
            3.46,
            1.3,
            20,
            hexc(0xe8e4d8),
            false,
        );
        f.cylinder(
            Mat::Paint,
            v3(0.0, 20.0, 0.0),
            3.42,
            3.4,
            2.0,
            20,
            hexc(0x141416),
            false,
        );
        f.cylinder(
            Mat::Iron,
            v3(0.0, 20.0, 0.0),
            3.0,
            3.0,
            1.9,
            20,
            hexc(0x0a0a0a),
            true,
        );
        p.add_parts(
            &f,
            Mat4::from_scale_rotation_translation(
                v3(1.35, 1.0, 0.9),
                Quat::IDENTITY,
                v3(x, y, 0.0),
            ),
        );
        for z in [-1.4f32, 1.4] {
            p.rod(
                Mat::Iron,
                v3(x - 5.0, y, z),
                v3(x - 5.0, y + 22.5, z),
                0.22,
                6,
                hexc(0x6a6e72),
            );
        }
    }
    // Boats on the shelter deck and their cranes.
    for (x, z) in [
        (4.0f32, 5.2f32),
        (4.0, -5.2),
        (-14.0, 4.6),
        (-14.0, -4.6),
        (-27.0, 4.6),
        (-27.0, -4.6),
    ] {
        let y = deck(x) + 3.15;
        let boat = crate::looks::small_boat();
        p.add_parts(
            &boat,
            Mat4::from_scale_rotation_translation(
                Vec3::splat(1.7),
                Quat::IDENTITY,
                v3(x, y + 0.4, z),
            ),
        );
    }
    for z in [-1.0f32, 1.0] {
        let x = -6.0 - 10.0;
        let y = deck(x) + 3.15;
        p.rod(
            Mat::Iron,
            v3(x, y, z * 2.0),
            v3(x, y + 9.0, z * 2.0),
            0.35,
            8,
            light,
        );
        p.rod(
            Mat::Iron,
            v3(x, y + 8.5, z * 2.0),
            v3(x + 7.0, y + 12.0, z * 7.5),
            0.2,
            6,
            light,
        );
        // The block at the jib's head.
        p.cuboid(
            Mat::Iron,
            v3(x + 6.6, y + 11.6, z * 7.5 - 0.4),
            v3(x + 7.4, y + 12.4, z * 7.5 + 0.4),
            hexc(0x2e3236),
            true,
        );
    }
    // The tripod mainmast.
    let mx = -36.5;
    let md = deck(mx);
    p.rod(
        Mat::Iron,
        v3(mx, md, 0.0),
        v3(mx, md + 30.0, 0.0),
        0.6,
        10,
        light,
    );
    for z in [-3.5f32, 3.5] {
        p.rod(
            Mat::Iron,
            v3(mx + 5.0, md, z),
            v3(mx, md + 24.0, 0.0),
            0.42,
            8,
            light,
        );
    }
    p.cylinder(
        Mat::Paint,
        v3(mx, md + 22.0, 0.0),
        2.6,
        2.4,
        2.4,
        14,
        light,
        true,
    );
    p.rod(
        Mat::Iron,
        v3(mx, md + 30.0, 0.0),
        v3(mx, md + 40.0, 0.0),
        0.22,
        6,
        light,
    );
    // Wireless aerials between the mastheads.
    p.rod(
        Mat::Iron,
        v3(fx, tall + 13.0, 0.0),
        v3(mx, md + 39.0, 0.0),
        0.05,
        3,
        hexc(0x2a2420),
    );
    // Colours: the company's ensign aft, a jack forward, pennants at both mastheads.
    let blue = hexc(0x2a3a8a);
    let gold = hexc(0xd8b048);
    flag(
        &mut p,
        v3(-73.5, deck(-73.5), 0.0),
        9.0,
        7.5,
        4.6,
        blue,
        gold,
    );
    flag(&mut p, v3(73.0, deck(73.0), 0.0), 5.0, 3.6, 2.4, blue, gold);
    pennant(&mut p, v3(fx, tall + 13.5, 0.0), 24.0, gold);
    pennant(&mut p, v3(mx, md + 39.5, 0.0), 12.0, blue);
    // The torpedo-net shelf along both sides, and the booms folded against it.
    for s in [1.0f32, -1.0] {
        let mut prev: Option<Vec3> = None;
        for i in 0..=14 {
            let x = -42.0 + i as f32 * 6.0;
            let q = v3(x, deck(x) - 0.9, s * (half(x) + 0.35));
            if let Some(a) = prev {
                p.rod(Mat::Iron, a, q, 0.28, 5, hexc(0x2e3236));
            }
            prev = Some(q);
            p.rod(
                Mat::Iron,
                q,
                q + v3(-4.2, 0.15, s * 0.1),
                0.08,
                4,
                hexc(0x3a3e42),
            );
        }
    }
    // Rails along the forecastle and quarterdeck.
    for s in [1.0f32, -1.0] {
        let mut prev: Option<Vec3> = None;
        for i in 0..=30 {
            let x = -72.0 + i as f32 * 4.8;
            let q = v3(x, deck(x) + 1.1, s * (half(x) - 0.15));
            if let Some(a) = prev {
                p.rod(Mat::Iron, a, q, 0.05, 3, light);
            }
            prev = Some(q);
        }
    }
    p.ground_occlusion(1.0, 1.0);
    p.finish()
}

/// A giant iron steamship: a long black hull with a gilt line and a white band, two tiers of
/// portholes, five red funnels with black tops (two forward of the paddles, three aft), six
/// masts (the forward two with sail set), paddle boxes as tall as a house with a gilt fan, a
/// bridge across between them, white deckhouses, lifeboats in davits, a clipper bow with a
/// figurehead and a long bowsprit.
pub fn leviathan() -> Parts {
    let mut p = Parts::default();
    let (len, beam, free, fine) = (180.0f32, 24.0f32, 8.0f32, 2.6f32);
    let black = hexc(0x15171a);
    let gold = hexc(0xc8a040);
    let white = hexc(0xf0ece2);
    let h = hull(
        &mut p,
        Mat::Paint,
        len,
        beam,
        7.0,
        free,
        fine,
        black,
        hexc(0x7a2a1a),
        0.5,
    );
    let deck = |x: f32| deck_at(x, len, free);
    let half = |x: f32| half_at(x, len, beam, fine);
    strake(&mut p, &h, -88.0, 88.0, 6.6, 7.4, white);
    strake(&mut p, &h, -86.0, 86.0, 3.9, 4.15, gold);
    // Two tiers of portholes.
    for (n, s) in [(Vec3::Z, 1.0f32), (-Vec3::Z, -1.0)] {
        for (x0, x1) in [(-70.0f32, -22.0f32), (-14.0, 10.0), (18.0, 66.0)] {
            let x = (x0 + x1) * 0.5;
            let k = ((x1 - x0) / 2.4) as u32;
            for y in [2.7f32, 5.4] {
                p.windows(
                    v3(x, y, s * (half(x) + 0.02)),
                    n,
                    k,
                    0.5,
                    0.55,
                    1.9,
                    hexc(0x8a6a4a),
                );
            }
        }
    }
    // Paddle boxes: a disc as tall as a house each side, a gilt fan on its face.
    let px = -6.0;
    let hb = half(px);
    let (pc, pr) = (4.0f32, 10.5f32);
    for s in [1.0f32, -1.0] {
        p.rod(
            Mat::Paint,
            v3(px, pc, s * (hb - 0.5)),
            v3(px, pc, s * (hb + 4.6)),
            pr,
            28,
            black,
        );
        // The face, closed.
        let face = s * (hb + 4.6);
        let mut disc = Parts::default();
        disc.cylinder(Mat::Paint, Vec3::ZERO, pr, 0.0, 0.01, 28, black, false);
        p.add_parts(
            &disc,
            Mat4::from_rotation_translation(Quat::from_rotation_x(s * FRAC_PI_2), v3(px, pc, face)),
        );
        for k in 0..9 {
            let a = PI * (0.08 + 0.84 * k as f32 / 8.0);
            let d = v3(a.cos(), a.sin(), 0.0);
            p.rod(
                Mat::Paint,
                v3(px, pc, face + s * 0.08) + d * 2.6,
                v3(px, pc, face + s * 0.08) + d * (pr - 0.5),
                0.16,
                4,
                gold,
            );
        }
        let mut ring = Parts::default();
        ring.cylinder(Mat::Paint, Vec3::ZERO, 2.6, 2.6, 0.2, 18, gold, true);
        p.add_parts(
            &ring,
            Mat4::from_rotation_translation(Quat::from_rotation_x(s * FRAC_PI_2), v3(px, pc, face)),
        );
        // Sponsons fore and aft of the box.
        p.cuboid(
            Mat::Planks,
            v3(
                px - 16.0,
                deck(px) - 0.2,
                s.min(0.0) * (hb + 4.6) + s.max(0.0) * hb,
            ),
            v3(
                px + 16.0,
                deck(px),
                s.max(0.0) * (hb + 4.6) + s.min(0.0) * hb,
            ),
            hexc(0xc8a878),
            true,
        );
    }
    // The bridge across between the paddle boxes, with its wheelhouse.
    let bd = deck(px) + pr - 0.5;
    p.cuboid(
        Mat::Planks,
        v3(px - 1.6, bd, -hb - 4.6),
        v3(px + 1.6, bd + 0.35, hb + 4.6),
        hexc(0xc8a878),
        true,
    );
    for s in [1.0f32, -1.0] {
        p.rod(
            Mat::Iron,
            v3(px + 1.5, bd + 1.2, 0.0),
            v3(px + 1.5, bd + 1.2, s * (hb + 4.6)),
            0.05,
            3,
            white,
        );
    }
    p.cuboid(
        Mat::Paint,
        v3(px - 1.4, bd + 0.35, -2.4),
        v3(px + 1.4, bd + 3.0, 2.4),
        white,
        false,
    );
    p.cuboid(
        Mat::Paint,
        v3(px - 1.7, bd + 3.0, -2.7),
        v3(px + 1.9, bd + 3.2, 2.7),
        white,
        true,
    );
    p.windows(
        v3(px + 1.41, bd + 2.0, 0.0),
        Vec3::X,
        3,
        0.8,
        0.9,
        0.4,
        hexc(0x8a6a4a),
    );
    // White deckhouses along the middle, with windows and skylights.
    for (x0, x1) in [(12.0f32, 48.0f32), (-70.0, -16.0)] {
        let y = deck((x0 + x1) * 0.5);
        let w = 5.2;
        p.cuboid(
            Mat::Paint,
            v3(x0, y - 0.3, -w),
            v3(x1, y + 2.8, w),
            white,
            false,
        );
        p.cuboid(
            Mat::Timber,
            v3(x0 - 0.3, y + 2.8, -w - 0.3),
            v3(x1 + 0.3, y + 3.0, w + 0.3),
            hexc(0xb8a888),
            true,
        );
        for (n, z) in [(Vec3::Z, w + 0.01), (-Vec3::Z, -w - 0.01)] {
            p.windows(
                v3((x0 + x1) * 0.5, y + 1.5, z),
                n,
                ((x1 - x0) / 2.2) as u32,
                0.9,
                1.0,
                1.3,
                hexc(0x8a6a4a),
            );
        }
        let mut x = x0 + 3.0;
        while x < x1 - 2.0 {
            p.cuboid(
                Mat::Window,
                v3(x - 1.0, y + 3.0, -1.2),
                v3(x + 1.0, y + 3.7, 1.2),
                hexc(0x9ab0b8),
                false,
            );
            x += 7.0;
        }
    }
    // Five funnels: red, a white band, black tops.
    for x in [38.0f32, 23.0, -26.0, -40.0, -54.0] {
        let y = deck(x) + 2.8;
        p.cylinder(
            Mat::Paint,
            v3(x, y, 0.0),
            2.3,
            2.15,
            21.0,
            16,
            hexc(0xa8321e),
            false,
        );
        p.cylinder(
            Mat::Paint,
            v3(x, y + 14.5, 0.0),
            2.22,
            2.21,
            1.0,
            16,
            white,
            false,
        );
        p.cylinder(
            Mat::Paint,
            v3(x, y + 18.0, 0.0),
            2.19,
            2.15,
            3.0,
            16,
            hexc(0x0e0e0e),
            false,
        );
        p.cylinder(
            Mat::Iron,
            v3(x, y + 20.9, 0.0),
            1.9,
            1.9,
            0.1,
            16,
            hexc(0x050505),
            true,
        );
        // Guys.
        for s in [1.0f32, -1.0] {
            p.rod(
                Mat::Iron,
                v3(x, y + 17.0, 0.0),
                v3(x, deck(x) + 0.2, s * (half(x) - 0.4)),
                0.03,
                3,
                hexc(0x2a2420),
            );
        }
    }
    // Six masts: the forward two square-rigged with sail set, the rest with sails furled.
    for (i, (x, h)) in [
        (74.0f32, 46.0f32),
        (54.0, 50.0),
        (6.0, 46.0),
        (-13.0, 46.0),
        (-68.0, 42.0),
        (-82.0, 36.0),
    ]
    .into_iter()
    .enumerate()
    {
        let d = deck(x);
        mast(&mut p, x, d, h, 0.75);
        shrouds(&mut p, x, d, h, 2.0 * half(x));
        if i < 2 {
            square_sail(&mut p, x, d + h * 0.42, 24.0, 10.0);
            square_sail(&mut p, x, d + h * 0.64, 20.0, 8.5);
            square_sail(&mut p, x, d + h * 0.83, 14.0, 6.0);
        } else {
            for (k, w) in [(0.45f32, 22.0f32), (0.68, 17.0), (0.86, 12.0)] {
                let y = d + h * k;
                p.rod(
                    Mat::Timber,
                    v3(x + 0.6, y, -w * 0.5),
                    v3(x + 0.6, y, w * 0.5),
                    0.16,
                    6,
                    Vec4::ONE,
                );
                p.rod(
                    Mat::Canvas,
                    v3(x + 0.9, y - 0.25, -w * 0.45),
                    v3(x + 0.9, y - 0.25, w * 0.45),
                    0.42,
                    8,
                    hexc(0xe8e0cc),
                );
            }
        }
        pennant(
            &mut p,
            v3(x, d + h, 0.0),
            9.0,
            if i % 2 == 0 { gold } else { hexc(0x2a3a8a) },
        );
    }
    // Stays from mast to mast.
    let tops: Vec<Vec3> = [
        (74.0f32, 46.0f32),
        (54.0, 50.0),
        (6.0, 46.0),
        (-13.0, 46.0),
        (-68.0, 42.0),
        (-82.0, 36.0),
    ]
    .iter()
    .map(|&(x, h)| v3(x, deck(x) + h * 0.9, 0.0))
    .collect();
    for w in tops.windows(2) {
        p.rod(Mat::Iron, w[0], w[1], 0.04, 3, hexc(0x2a2420));
    }
    // Clipper bow: bowsprit, jib boom, a gilt figurehead and the headsails.
    let bow = v3(len * 0.5 - 0.5, deck(len * 0.5) + 0.2, 0.0);
    p.rod(
        Mat::Timber,
        bow,
        bow + v3(22.0, 5.5, 0.0),
        0.55,
        8,
        Vec4::ONE,
    );
    p.box_at(
        Mat::Paint,
        bow + v3(0.8, -2.2, 0.0),
        v3(1.4, 1.4, 0.5),
        0.0,
        gold,
    );
    for (tack, head) in [
        (bow + v3(20.0, 5.0, 0.0), v3(74.2, deck(74.0) + 42.0, 0.0)),
        (bow + v3(12.0, 3.0, 0.0), v3(74.2, deck(74.0) + 32.0, 0.0)),
    ] {
        crate::vessels::jib(
            &mut p,
            Mat::Canvas,
            tack,
            head,
            v3(70.0, deck(70.0) + 3.0, 0.8),
        );
    }
    // Lifeboats in davits along both rails.
    for s in [1.0f32, -1.0] {
        for i in 0..9 {
            let x = -60.0 + i as f32 * 14.0;
            if (x - px).abs() < 18.0 {
                continue;
            }
            let z = s * (half(x) - 1.6);
            let y = deck(x) + 2.6;
            p.add_parts(
                &crate::looks::small_boat(),
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(1.6),
                    Quat::IDENTITY,
                    v3(x, y, z),
                ),
            );
            for dx in [-2.6f32, 2.6] {
                let head = v3(x + dx, y + 1.6, z);
                p.rod(
                    Mat::Iron,
                    v3(x + dx, deck(x), s * (half(x) - 0.3)),
                    head,
                    0.1,
                    4,
                    white,
                );
                p.cuboid(
                    Mat::Iron,
                    head - Vec3::splat(0.18),
                    head + Vec3::splat(0.18),
                    white,
                    true,
                );
            }
        }
    }
    // Gallery windows across the stern and the ensign.
    let sx = -len * 0.5 + 0.2;
    p.windows(
        v3(sx - 0.05, deck(sx) - 1.6, 0.0),
        -Vec3::X,
        7,
        0.9,
        1.2,
        0.8,
        gold,
    );
    flag(
        &mut p,
        v3(sx + 1.0, deck(sx), 0.0),
        10.0,
        8.0,
        5.0,
        hexc(0x2a3a8a),
        gold,
    );
    // Rails all along.
    for s in [1.0f32, -1.0] {
        let mut prev: Option<Vec3> = None;
        for i in 0..=36 {
            let x = -88.0 + i as f32 * 4.9;
            let q = v3(x, deck(x) + 1.1, s * (half(x) - 0.15));
            if let Some(a) = prev {
                p.rod(Mat::Iron, a, q, 0.05, 3, white);
            }
            prev = Some(q);
        }
    }
    p.ground_occlusion(1.0, 1.0);
    p.finish()
}

/// A rigid sky carrier: a 220 m silver envelope with girder bands and the company's blue
/// and gold round the tail, a cruciform tail, eight engines on outriggers along her sides,
/// a streamlined control car under the nose, a gun station on her back, the hangar in her
/// belly with an escort fighter hanging from its trapeze, and riding lights.
pub fn sky_carrier() -> Parts {
    let mut p = Parts::default();
    let len = 220.0f32;
    let r = 21.0f32;
    let n = 40;
    let shape = |t: f32| -> f32 {
        // t 0 tail .. 1 nose: a full, blunt nose and a long taper aft, fattest at 0.6.
        if t > 0.6 {
            let u = (t - 0.6) / 0.4;
            (1.0 - u.powf(2.2)).max(0.0).sqrt()
        } else {
            let u = (0.6 - t) / 0.6;
            (1.0 - u.powf(1.7)).max(0.0).powf(0.75)
        }
    };
    let prof: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            ((t - 0.5) * len, r * shape(t) + 0.05)
        })
        .collect();
    let blue = hexc(0x2a3a8a);
    let gold = hexc(0xd8b048);
    p.lathe_x(Mat::Envelope, &prof, 24, |x, a| {
        // Girder rings every 15 m, the company's colours round the tail.
        let ring = ((x + 200.0) % 15.0) < 0.9;
        if (-82.0..-76.0).contains(&x) {
            return blue;
        }
        if (-74.5..-72.5).contains(&x) {
            return gold;
        }
        let k = if ring { 0.8 } else { 1.0 } * (0.9 + 0.1 * a.sin().abs());
        Vec4::new(k, k, k * 0.985, 1.0)
    });
    let radius = |x: f32| r * shape(x / len + 0.5);
    // The company's livery: a blue cheatline with gold pinstripes along both flanks, and a
    // blue and gold ring round the tail, each standing a hand proud of the cover.
    let band = |p: &mut Parts, x0: f32, x1: f32, a0: f32, a1: f32, col: Vec4| {
        let steps = 4;
        let steps = if a1 - a0 > 1.0 { 24 } else { steps };
        for w in prof.windows(2) {
            let (xa, xb) = (w[0].0, w[1].0);
            if xb < x0 || xa > x1 {
                continue;
            }
            let (xa, xb) = (xa.max(x0), xb.min(x1));
            let rad = |x: f32| radius(x) + 0.12;
            let at = |x: f32, a: f32| v3(x, a.cos() * rad(x), a.sin() * rad(x));
            for k in 0..steps {
                let b0 = a0 + (a1 - a0) * k as f32 / steps as f32;
                let b1 = a0 + (a1 - a0) * (k + 1) as f32 / steps as f32;
                p.quad(
                    Mat::Paint,
                    at(xa, b1),
                    at(xb, b1),
                    at(xb, b0),
                    at(xa, b0),
                    col,
                );
            }
        }
    };
    let deg = |d: f32| d.to_radians();
    for side in [0.0f32, 180.0] {
        band(
            &mut p,
            -84.0,
            92.0,
            deg(side + 80.0),
            deg(side + 98.0),
            blue,
        );
        band(
            &mut p,
            -84.0,
            92.0,
            deg(side + 76.5),
            deg(side + 79.0),
            gold,
        );
        band(
            &mut p,
            -84.0,
            92.0,
            deg(side + 99.0),
            deg(side + 101.5),
            gold,
        );
    }
    band(&mut p, -80.0, -74.0, 0.0, TAU, blue);
    band(&mut p, -72.5, -71.0, 0.0, TAU, gold);
    // Cruciform tail: four fins with blue rudders and elevators.
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2;
        let d = v3(0.0, a.cos(), a.sin());
        let side = v3(0.0, -a.sin(), a.cos()) * 0.35;
        let (x0, x1) = (-106.0f32, -70.0f32);
        let root0 = v3(x0, 0.0, 0.0) + d * (radius(x0) - 0.5);
        let root1 = v3(x1, 0.0, 0.0) + d * (radius(x1) - 0.5);
        let tip0 = v3(-110.0, 0.0, 0.0) + d * 22.0;
        let tip1 = v3(-90.0, 0.0, 0.0) + d * 22.0;
        let fin = Vec4::splat(0.86).with_w(1.0);
        p.quad(
            Mat::Envelope,
            root0 + side,
            root1 + side,
            tip1 + side,
            tip0 + side,
            fin,
        );
        p.quad(
            Mat::Envelope,
            root1 - side,
            root0 - side,
            tip0 - side,
            tip1 - side,
            fin,
        );
        p.quad(
            Mat::Envelope,
            tip0 + side,
            tip1 + side,
            tip1 - side,
            tip0 - side,
            fin,
        );
        let aft = v3(-5.0, 0.0, 0.0);
        let tip_r = tip0 - d * 0.5;
        p.quad(
            Mat::Paint,
            root0 + side * 1.2 + aft,
            root0 + side * 1.2,
            tip_r + side * 1.2,
            tip_r + side * 1.2 + aft,
            blue,
        );
        p.quad(
            Mat::Paint,
            root0 - side * 1.2,
            root0 - side * 1.2 + aft,
            tip_r - side * 1.2 + aft,
            tip_r - side * 1.2,
            blue,
        );
    }
    // The control car under the nose.
    let mut car = Parts::default();
    car.lathe_x(
        Mat::Paint,
        &[
            (-9.0, 0.05),
            (-7.0, 1.8),
            (4.0, 2.6),
            (8.0, 2.2),
            (10.0, 0.05),
        ],
        14,
        |_, _| hexc(0x3a3e44),
    );
    let cx = 72.0;
    let cy = -radius(cx) - 1.6;
    p.add_parts(&car, Mat4::from_translation(v3(cx, cy, 0.0)));
    for (n, z) in [(Vec3::Z, 2.62f32), (-Vec3::Z, -2.62)] {
        p.windows(
            v3(cx + 2.5, cy + 0.6, z),
            n,
            6,
            0.9,
            0.8,
            0.5,
            hexc(0x9a9a9a),
        );
    }
    // Eight engines on outriggers, four a side, their three-bladed airscrews.
    for x in [52.0f32, 26.0, 0.0, -26.0] {
        for s in [1.0f32, -1.0] {
            let rr = radius(x);
            let root = v3(x, -rr * 0.45, s * rr * 0.86);
            let hub = v3(x, -rr * 0.5, s * (rr + 4.5));
            p.rod(Mat::Iron, root, hub, 0.35, 6, hexc(0x8a8a88));
            p.rod(
                Mat::Iron,
                root + v3(-3.0, 0.0, 0.0),
                hub,
                0.2,
                4,
                hexc(0x8a8a88),
            );
            let mut nac = Parts::default();
            nac.lathe_x(
                Mat::Paint,
                &[(-3.2, 0.05), (-2.4, 0.9), (1.6, 1.0), (2.6, 0.4)],
                10,
                |_, _| hexc(0x4a4a48),
            );
            p.add_parts(&nac, Mat4::from_translation(hub));
            for k in 0..3 {
                let a = k as f32 * TAU / 3.0 + 0.4;
                let d = v3(0.0, a.cos(), a.sin());
                p.box_at(
                    Mat::Timber,
                    hub + v3(-3.4, 0.0, 0.0) + d * 1.8,
                    v3(0.08, 0.18, 0.18) + d.abs() * 1.7,
                    0.0,
                    hexc(0x4a3424),
                );
            }
        }
    }
    // The hangar keel under her belly: a T-shaped door, and the trapeze with a fighter on it.
    let (h0, h1) = (4.0f32, 40.0f32);
    let hy = -radius(22.0) + 0.8;
    p.cuboid(
        Mat::Paint,
        v3(h0, hy - 3.0, -6.0),
        v3(h1, hy, 6.0),
        hexc(0xb8bcc0),
        true,
    );
    p.quad(
        Mat::Paint,
        v3(h0 + 8.0, hy - 3.02, -1.6),
        v3(h1 - 6.0, hy - 3.02, -1.6),
        v3(h1 - 6.0, hy - 3.02, 1.6),
        v3(h0 + 8.0, hy - 3.02, 1.6),
        hexc(0x101214),
    );
    p.quad(
        Mat::Paint,
        v3(h0 + 3.0, hy - 3.02, -5.0),
        v3(h0 + 8.0, hy - 3.02, -5.0),
        v3(h0 + 8.0, hy - 3.02, 5.0),
        v3(h0 + 3.0, hy - 3.02, 5.0),
        hexc(0x101214),
    );
    let tx = 20.0;
    for z in [-1.2f32, 1.2] {
        p.rod(
            Mat::Iron,
            v3(tx - 2.0, hy - 3.0, z),
            v3(tx, hy - 12.0, z * 0.3),
            0.16,
            6,
            hexc(0x5a5a5a),
        );
    }
    p.rod(
        Mat::Iron,
        v3(tx, hy - 12.0, -1.0),
        v3(tx, hy - 12.0, 1.0),
        0.12,
        6,
        hexc(0x5a5a5a),
    );
    p.add_parts(
        &crate::vessels::biplane(),
        Mat4::from_translation(v3(tx, hy - 14.0, 0.0)),
    );
    // A gun station on her back, behind the nose.
    let gx = 70.0;
    let gy = radius(gx) - 0.4;
    p.cylinder(
        Mat::Paint,
        v3(gx, gy, 0.0),
        1.6,
        1.3,
        1.2,
        12,
        hexc(0x3a3e44),
        true,
    );
    for z in [-0.4f32, 0.4] {
        p.rod(
            Mat::Iron,
            v3(gx, gy + 1.0, z),
            v3(gx + 3.0, gy + 1.4, z),
            0.08,
            4,
            hexc(0x1a1a1a),
        );
    }
    // Riding lights: red to port, green to starboard, white at the tail, lamps along the keel.
    for (z, col) in [(1.0f32, hexc(0xff3020)), (-1.0, hexc(0x30ff60))] {
        p.cylinder(
            Mat::Lamp,
            v3(60.0, 0.0, z * (radius(60.0) + 0.1)),
            0.5,
            0.5,
            0.6,
            8,
            col,
            true,
        );
    }
    p.cylinder(
        Mat::Lamp,
        v3(-109.0, 0.0, 0.0),
        0.5,
        0.5,
        0.6,
        8,
        hexc(0xfff2d0),
        true,
    );
    let mut x = -60.0;
    while x < 60.0 {
        if !(h0 - 2.0..h1 + 2.0).contains(&x) {
            p.cylinder(
                Mat::Lamp,
                v3(x, -radius(x) - 0.3, 0.0),
                0.3,
                0.3,
                0.3,
                6,
                hexc(0xffe8b0),
                true,
            );
        }
        x += 10.0;
    }
    // The company's colours from the tail.
    p.rod(
        Mat::Iron,
        v3(-108.0, 0.0, 0.0),
        v3(-113.0, -3.0, 0.0),
        0.1,
        4,
        hexc(0x8a8a88),
    );
    p.cuboid(
        Mat::Canvas,
        v3(-121.0, -7.0, -0.04),
        v3(-113.0, -3.0, 0.04),
        blue,
        true,
    );
    p.cuboid(
        Mat::Canvas,
        v3(-121.01, -5.6, -0.06),
        v3(-112.99, -4.4, 0.06),
        gold,
        true,
    );
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flagships_outclass_everything_else() {
        for c in Craft::ALL.into_iter().filter(|c| !c.flagship()) {
            for f in [Craft::Dreadnought, Craft::Leviathan, Craft::SkyCarrier] {
                assert!(
                    f.half_length() > c.half_length(),
                    "{f:?} is not longer than {c:?}"
                );
            }
            let (h, n) = c.hold();
            assert!(h * n < 8 * 80);
            let fire = crate::fittings::stats(c, &crate::fittings::usual(c)).firepower;
            let ours = crate::fittings::stats(
                Craft::Dreadnought,
                &crate::fittings::usual(Craft::Dreadnought),
            )
            .firepower;
            assert!(
                ours > fire * 5.0,
                "{c:?} has {fire}, the dreadnought {ours}"
            );
            assert!(
                crate::fittings::base_hull(c) * 4.0
                    < crate::fittings::base_hull(Craft::Dreadnought)
            );
        }
        // And cost far more than the dearest of the rest.
        let top = crate::shipyard::PLANS
            .iter()
            .filter(|p| !p.craft.flagship())
            .map(|p| p.coins)
            .max()
            .unwrap();
        for p in crate::shipyard::PLANS.iter().filter(|p| p.craft.flagship()) {
            assert!(p.coins >= top * 5, "{:?} costs {}", p.craft, p.coins);
        }
    }

    #[test]
    fn the_models_are_built() {
        for f in [dreadnought(), leviathan(), sky_carrier()] {
            assert!(f.triangles() > 3_000, "{} triangles", f.triangles());
        }
    }
}
