//! The valley on a real planet: the terrain module's Earth, with the showcase landscape laid
//! into it as a hand-made overlay. Out to [`OVERLAY_RADIUS`] the ground is the landscape's own
//! (heights, and its colors as a tint over the terrain's ground textures); beyond the blend the
//! planet carries on with its own hills, ranges and coast, all the way round, so the tower,
//! the stratosphere and orbit look down on a whole world instead of a disc of valley.
//!
//! The terrain draws the ground, keeps physics colliders around the focus, takes footprints
//! and shows the weather (wet ground, puddles, snow). The landscape still answers the world's
//! `Ground` query ([`LandscapeGround`](crate::landscape::LandscapeGround)): it knows every
//! surface the showcase lays out (the footstep trail's boards and ice, the footprint lab's
//! strips), which the terrain's paint layers don't all have.

use crate::landscape::{Cover, LANDSCAPE_SEED, Landscape, PLANET_RADIUS};
use engine::prelude::*;
use engine_terrain::{FlatPad, OverlayPaint, PlanetTerrain};
use std::sync::Arc;

/// The landscape replaces the planet's ground out to here from the plaza (m): the valley and
/// the ring of mountains around it.
pub const OVERLAY_RADIUS: f64 = 9_000.0;
/// Width of the blend from the landscape back into the planet (m).
pub const OVERLAY_BLEND: f64 = 6_000.0;
/// How far the landscape's colors tint the terrain's ground textures (0..1). High enough that
/// fields, woods, beaches and the snow line read as the landscape paints them; the textures
/// keep their detail.
const TINT: f32 = 0.75;

/// Use the planet terrain for the ground (the default; `--no-planet` draws the landscape's own
/// mesh instead).
#[derive(Resource, Debug, Clone, Copy)]
pub struct UsePlanet;

/// Spawn the Earth with the valley laid into it, and the Moon's surface.
pub fn spawn(commands: &mut Commands, land: Arc<Landscape>) {
    // Far inland, with a range on the northern horizon beyond the valley's own mountains.
    let pad = FlatPad {
        mountains: 0.9,
        mountain_distance_m: 30_000.0,
        ..FlatPad::default()
    };
    let height = land.clone();
    let mut earth = PlanetTerrain::earthlike(LANDSCAPE_SEED)
        .with_radius(PLANET_RADIUS)
        .with_pad(pad)
        .with_overlay(OVERLAY_RADIUS, OVERLAY_BLEND, move |x, z| {
            height.flat_height(x, z)
        })
        .with_overlay_paint(move |x, z| Some(paint(&land, x, z)));
    earth.ground = false;
    commands.spawn((Name::new("Earth"), earth.home_transform(pad), earth));
}

/// The Moon's cratered surface at its real size, centered at `center`.
pub fn spawn_moon(commands: &mut Commands, center: DVec3) {
    let mut moon = PlanetTerrain::moon(LANDSCAPE_SEED).with_radius(1_737_400.0);
    moon.ground = false;
    moon.colliders = false;
    commands.spawn((Name::new("Moon"), Transform::from_translation(center), moon));
}

/// The terrain layer for a landscape cover (the terrain's paint names).
fn layer(cover: Cover) -> &'static str {
    match cover {
        Cover::Grass => "grass",
        Cover::Forest => "leaves",
        Cover::Dirt => "dirt",
        Cover::Cobble | Cover::Rock => "rock",
        Cover::Sand => "sand",
        Cover::Snow | Cover::Ice => "snow",
        Cover::LakeBed | Cover::Mud | Cover::Puddle => "mud",
        Cover::Gravel => "gravel",
        Cover::Wood | Cover::Metal | Cover::Concrete => "concrete",
    }
}

/// The landscape's ground at (x, z) as terrain paint: its cover's layer, tinted toward the
/// color the landscape gives it.
fn paint(land: &Landscape, x: f64, z: f64) -> OverlayPaint {
    let h = land.height(x, z);
    let n = land.normal(x, z, 0.5);
    let color = land.color(x, z, h, n);
    OverlayPaint::layer(layer(land.cover(x, z, h, n)), 1.0).tinted(color.to_array(), TINT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cover_has_a_terrain_layer() {
        for cover in crate::landscape::COVERS {
            let paint = OverlayPaint::layer(layer(cover), 1.0);
            assert!(paint.weight > 0.0, "{cover:?}");
        }
    }

    #[test]
    fn paint_tints_toward_the_landscape_color() {
        let land = crate::landscape::shared();
        let p = paint(land, 120.0, -340.0);
        assert!((p.tint_amount - TINT).abs() < 1e-6);
        assert!(p.tint.iter().all(|c| (0.0..=1.0).contains(c)));
    }
}
