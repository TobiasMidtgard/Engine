//! The valley on a real planet: the terrain module's Earth, with the showcase landscape laid
//! into it as a hand-made overlay. Out to [`OVERLAY_RADIUS`] the ground is the landscape's own
//! (heights, and its colors as a tint over the terrain's ground textures); beyond the blend the
//! planet carries on with its own hills, ranges and coast, all the way round, so the tower,
//! the stratosphere and orbit look down on a whole world instead of a disc of valley.
//!
//! Earth is one body of the space module's solar system (the Sun, the planets and the Moon
//! on their real orbits), so the Moon and Mars are real terrain at their real places and
//! `space.travel` can go there.
//!
//! The terrain draws the ground, keeps physics colliders around the focus, takes footprints
//! and shows the weather (wet ground, puddles, snow). It also answers the world's `Ground`
//! query, so terrain edits (craters, trenches) reach footsteps, NPCs and foliage, while the
//! landscape ([`LandscapeGround`](crate::landscape::LandscapeGround)) names the surface: it
//! knows every surface the showcase lays out (the footstep trail's boards and ice, the
//! footprint lab's strips), which the terrain's paint layers don't all have.

use crate::landscape::{Cover, LANDSCAPE_SEED, Landscape, LandscapeGround, PLANET_RADIUS};
use crate::places::Place;
use engine::prelude::*;
use engine_space::CelestialBody;
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

/// The planet's ground color seen from above, for the sky's light from below.
const GROUND_COLOR: [f32; 3] = [0.07, 0.13, 0.04];

/// Spawn the solar system with the valley laid into Earth, and the Moon's and Mars's surfaces.
/// `surfaces` names the valley's ground for footsteps (see the module docs).
pub fn spawn(commands: &mut Commands, land: Arc<Landscape>, surfaces: Arc<LandscapeGround>) {
    // The space module keeps Earth (the anchor) where `home_transform(pad)` puts it: the
    // pad's 40° N 0° E, 60 m above sea level, is `SpaceFrame::default()`.
    let bodies = engine_space::demo::spawn_solar_system(commands);
    // Far inland, with a range on the northern horizon beyond the valley's own mountains.
    let pad = FlatPad {
        mountains: 0.9,
        mountain_distance_m: 30_000.0,
        ..FlatPad::default()
    };
    let height = land.clone();
    let earth = PlanetTerrain::earthlike(LANDSCAPE_SEED)
        .with_radius(PLANET_RADIUS)
        .with_pad(pad)
        // World heights as the scenery sees them (with the curvature drop): the terrain draws
        // the ground exactly there.
        .with_overlay(OVERLAY_RADIUS, OVERLAY_BLEND, move |x, z| {
            height.height(x, z)
        })
        .with_overlay_paint(move |x, z| Some(paint(&land, x, z)))
        .with_ground_surface(surfaces);
    commands
        .entity(bodies["Earth"])
        .insert((Name::new("Earth"), earth.home_transform(pad), earth));

    let mut moon = PlanetTerrain::moon(LANDSCAPE_SEED).with_radius(1_737_400.0);
    moon.ground = false;
    moon.colliders = false;
    commands
        .entity(bodies["Moon"])
        .insert((Name::new("Moon"), moon));
    let mut mars = PlanetTerrain::mars(LANDSCAPE_SEED).with_radius(3_389_500.0);
    mars.ground = false;
    mars.colliders = false;
    commands
        .entity(bodies["Mars"])
        .insert((Name::new("Mars"), mars));

    // The space module sets the sky from Earth's air; keep the valley's green ground under it.
    let earth = bodies["Earth"];
    commands.queue(move |world: &mut World| {
        if let Some(mut body) = world.get_mut::<CelestialBody>(earth)
            && let Some(air) = body.atmosphere.as_mut()
        {
            air.ground_color = GROUND_COLOR;
        }
    });
}

/// Where the Moon is now. It moves along its orbit and, while the world turns with Earth,
/// round the sky once a day, so views of it are aimed when visited.
pub fn moon_now(world: &mut World) -> Option<DVec3> {
    let mut q = world.query::<(&CelestialBody, &Transform)>();
    q.iter(world)
        .find(|(b, _)| b.name == "Moon")
        .map(|(_, t)| t.translation)
}

/// `place` as it is to be seen now: views of the Moon aimed at where it is.
pub fn aim(world: &mut World, place: &Place) -> Place {
    if place.key == "moon"
        && let Some(moon) = moon_now(world)
    {
        let day = world.resource::<engine_weather::Sky>().day_of_year;
        return at_moon(place, moon, day);
    }
    place.clone()
}

/// `place` aimed at the Moon at `moon` (see [`moon_now`]), from the same distance on its
/// sunlit side at the place's hour.
pub fn at_moon(place: &Place, moon: DVec3, day_of_year: f32) -> Place {
    let distance = (place.position - place.target).length();
    let sun = engine_weather::Climate::default()
        .sun_direction(place.hour, day_of_year)
        .as_dvec3();
    let side = (sun * 0.7 - moon.normalize() * 0.7).normalize();
    Place {
        position: moon + side * distance,
        target: moon,
        ..place.clone()
    }
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
