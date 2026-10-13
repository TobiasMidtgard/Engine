//! The firework mortar in the north-west: a bronze tube on a carved octagonal footing ringed
//! with a glowing rune band, where the starburst spell is fired from.

use engine_core::math::{Quat, Vec3};
use engine_render::Mesh;

use super::super::meshes::metal::band;
use super::super::meshes::{Outline, Profile, colored, moved, revolve, sweep};
use super::super::{Prop, Surface};
use super::dress;

/// Height of the mortar's mouth (where the shell leaves).
pub const MUZZLE: f32 = 1.3;
/// Top of the stone footing.
const FOOT: f32 = 0.36;

/// A squat bronze mortar on a two-step octagonal footing, with iron bands, a flared mouth, a
/// dark bore and a violet rune band around the footing's top.
pub fn mortar() -> Prop {
    let mut stone = Mesh::default();
    let mut dark = Mesh::default();
    let mut bronze = Mesh::default();
    let mut iron = Mesh::default();
    let mut runes = Mesh::default();

    // Footing: a wide step, then a narrower one the tube sits in, with rounded top edges.
    let step = |y0: f32, y1: f32| {
        Profile::new()
            .corner(0.0, y0)
            .to(0.0, y1 - 0.03)
            .arc(-0.03, y1 - 0.03, 0.03, 0.0, 90.0, 2)
            .corner(-0.03, y1)
            .scale(0.0)
            .corner(0.0, y1)
    };
    let octagon = |r: f32| Outline::regular(8, r, std::f32::consts::PI / 8.0);
    let mut m = sweep(&octagon(0.8), &step(-0.02, 0.2));
    dress(&mut m, 31, 1.3);
    stone.merge(&m);
    let mut m = sweep(&octagon(0.52), &step(0.19, FOOT));
    dress(&mut m, 32, 1.1);
    stone.merge(&m);
    // A basalt collar where the tube meets the stone.
    let collar = Profile::new()
        .corner(0.3, FOOT - 0.01)
        .to(0.3, FOOT + 0.05)
        .arc(0.27, FOOT + 0.05, 0.03, 0.0, 90.0, 2)
        .corner(0.0, FOOT + 0.08);
    dark.merge(&revolve(&collar, 32));
    // The rune band around the upper step, just proud of its faces.
    let line = Profile::new()
        .corner(0.0, FOOT - 0.1)
        .corner(0.004, FOOT - 0.1)
        .corner(0.004, FOOT - 0.065)
        .corner(0.0, FOOT - 0.065);
    runes.merge(&sweep(&octagon(0.52), &line));

    // The bronze tube: up the outside to a flared lip, then down the bore.
    let y = FOOT + 0.06;
    let top = MUZZLE;
    let tube = Profile::new()
        // A heavy breech ring,
        .corner(0.26, y)
        .to(0.26, y + 0.06)
        .arc(0.23, y + 0.06, 0.03, 0.0, 90.0, 2)
        .corner(0.215, y + 0.09)
        // a barrel tapering up to a cast ring below the muzzle,
        .to(0.205, y + 0.3)
        .to(0.19, top - 0.2)
        .corner(0.19, top - 0.15)
        .arc(0.19, top - 0.13, 0.02, -90.0, 90.0, 3)
        .corner(0.19, top - 0.11)
        // and a flared lip.
        .to(0.2, top - 0.07)
        .corner(0.245, top - 0.03)
        .arc(0.23, top - 0.015, 0.015, 0.0, 90.0, 2)
        .corner(0.15, top)
        .corner(0.15, top - 0.04);
    bronze.merge(&revolve(&tube, 40));
    // The bore, dark with soot.
    let bore = Profile::new()
        .corner(0.15, top - 0.04)
        .to(0.15, y + 0.15)
        .corner(0.0, y + 0.15);
    dark.merge(&colored(revolve(&bore, 32), Vec3::splat(0.12)));
    for h in [y + 0.3, y + 0.56] {
        let r = 0.205 - 0.015 * ((h - y - 0.3) / (top - 0.2 - y - 0.3));
        iron.merge(&moved(
            band(r, 0.06, 0.014, 32),
            Vec3::Y * h,
            Quat::IDENTITY,
        ));
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, stone);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::Bronze, bronze);
    prop.push(Surface::Iron, iron);
    prop.push(Surface::RuneArcane, runes);
    prop
}
