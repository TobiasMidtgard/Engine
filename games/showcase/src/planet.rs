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
use engine_space::look::{FarGround, GroundDrawn, GroundPoint};
use engine_space::nearby::WorldClass;
use engine_terrain::far_look::FAR_LOOK_WIDTH;
use engine_terrain::{
    FarLook, FarLookMap, FlatPad, OverlayPaint, PlanetTerrain, WorldKind, WorldRecipe,
};
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

/// The valley's and the weather's north is +Z: the planet turns half round to match (the
/// space frame's `home_heading_deg` too, see `main`).
pub const HEADING_DEG: f64 = 180.0;

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
        heading_deg: HEADING_DEG,
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
        .with_overlay_paint({
            let cache = crate::paint_cache::PaintCache::new(land, paint);
            move |x, z| Some(cache.paint(x, z))
        })
        .with_ground_surface(surfaces);
    commands
        .entity(bodies["Earth"])
        // The terrain draws Earth's ground at every distance: the far view keeps the cloud
        // tops over it.
        .insert((
            Name::new("Earth"),
            earth.home_transform(pad),
            earth,
            GroundDrawn,
        ));

    // Ground and colliders on the Moon and Mars too: each answers the world's ground while
    // the focus is near it, so a pilot can walk, a ship land and a rock roll there.
    let moon = PlanetTerrain::moon(LANDSCAPE_SEED).with_radius(1_737_400.0);
    commands
        .entity(bodies["Moon"])
        .insert((Name::new("Moon"), moon));
    let mars = PlanetTerrain::mars(LANDSCAPE_SEED).with_radius(3_389_500.0);
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

/// Planets of other stars get their ground when the camera comes within this many of their
/// radii, and lose it again past twice as far.
const GROUND_REACH_RADII: f64 = 40.0;
/// Their look from afar is baked for the ground from this much farther out, so it is ready
/// when the ground comes.
const FAR_LOOK_LEAD: f64 = 1.5;

/// What [`ground_near_planets`] reads of a planet.
type PlanetGround = (
    Entity,
    &'static CelestialBody,
    Has<PlanetTerrain>,
    Option<&'static FarGround>,
    Option<&'static mut BakingGround>,
);

/// A planet's far look being baked for its ground (on a thread of its own).
#[derive(Component)]
pub struct BakingGround(Option<std::thread::JoinHandle<FarLook>>);

/// Give the planets of the nearby stars ground as a ship closes in: each carries a recipe of
/// what its surface is (engine_space's `nearby` module), made into a terrain of that kind, so
/// a pilot can land and walk there. Far behind, the terrain goes again.
///
/// The far view and the ground show one planet: an Earth-like world's far view paints the
/// terrain's own continents ([`FarGround`]), the terrain wears the far view's colors and glow
/// from high up ([`FarLook`]), and the cloud tops stay over the ground ([`GroundDrawn`]), so
/// nothing changes where the ground takes over.
pub fn ground_near_planets(
    mut commands: Commands,
    focus: Query<&Transform, With<engine::world::SimulationFocus>>,
    space: Option<Res<engine_space::SpaceState>>,
    mut bodies: Query<PlanetGround>,
) {
    let Some(eye) = focus.iter().next().map(|t| t.translation) else {
        return;
    };
    // Where space put each body this frame. Until it has (the first frame), every body sits
    // at the origin, right on the camera, and each planet would get its ground at startup.
    let Some(space) = space.filter(|s| s.anchor.is_some()) else {
        return;
    };
    for (entity, body, has, ground, baking) in &mut bodies {
        let Some(surface) = body.surface else {
            continue;
        };
        let Some(position) = space
            .bodies
            .iter()
            .find(|b| b.entity == entity)
            .map(|b| b.world_position)
        else {
            continue;
        };
        let recipe = world_recipe(body, &surface);
        // The far view paints an Earth-like world from its terrain's ground from the start.
        if ground.is_none() && body.look == engine_space::BodyLook::Earthlike {
            commands
                .entity(entity)
                .insert(far_ground(&PlanetTerrain::world(&recipe)));
        }
        let radii = (position - eye).length() / body.radius_m;
        // Under a cloud deck the ground is hidden from above: it comes only below the deck.
        let reach = if body.look.solid() {
            GROUND_REACH_RADII
        } else {
            1.1
        };
        if !has && radii < reach * FAR_LOOK_LEAD {
            match baking {
                None => {
                    let (body, ground) = (body.clone(), ground.cloned());
                    let job = std::thread::Builder::new()
                        .name(format!("far look {}", body.name))
                        .spawn(move || far_look(&body, ground.as_ref()))
                        .ok();
                    commands.entity(entity).insert(BakingGround(job));
                }
                Some(mut job) if radii < reach => {
                    if job.0.as_ref().is_some_and(|j| !j.is_finished()) {
                        continue;
                    }
                    let mut e = commands.entity(entity);
                    e.remove::<BakingGround>()
                        .insert((PlanetTerrain::world(&recipe), GroundDrawn));
                    if let Some(look) = job.0.take().and_then(|j| j.join().ok()) {
                        e.insert(look);
                    }
                }
                Some(_) => {}
            }
        } else if (has && radii > reach * 2.0)
            || (!has && baking.is_some() && radii > reach * FAR_LOOK_LEAD * 1.2)
        {
            commands
                .entity(entity)
                .remove::<(PlanetTerrain, FarLook, GroundDrawn, BakingGround)>();
        }
    }
}

/// The far view's sampler of a terrain's ground: its heights, moisture and frost.
pub fn far_ground(terrain: &PlanetTerrain) -> FarGround {
    let shape = Arc::new(terrain.shape.clone());
    let frost = terrain.palette.snow_below_c;
    FarGround::new(move |d, texel_m| {
        let s = shape.sample(d.as_dvec3(), (2.0 * texel_m as f64).max(1_000.0));
        GroundPoint {
            height_m: s.height as f32,
            moisture: s.moisture,
            above_frost_c: s.temperature_c - frost,
            temperature_c: s.temperature_c,
        }
    })
}

/// A planet's look from afar, for its terrain to wear from high up.
pub fn far_look(body: &CelestialBody, ground: Option<&FarGround>) -> FarLook {
    let maps = engine_space::look::bake_surface_on(body, ground, FAR_LOOK_WIDTH);
    let rgba = maps
        .albedo
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(maps.aux.data.as_chunks::<4>().0)
        .flat_map(|(a, x)| [a[0], a[1], a[2], x[3]])
        .collect();
    FarLook {
        map: Arc::new(FarLookMap::new(maps.albedo.width, maps.albedo.height, rgba)),
        // Barren worlds' provinces, dunes and lineae are the far view's alone: they tint the
        // ground all the way down. A living world's ground draws its own biomes.
        near_tint: if body.look == engine_space::BodyLook::Earthlike {
            0.2
        } else {
            0.8
        },
    }
}

/// The terrain recipe of a planet of another star.
fn world_recipe(body: &CelestialBody, s: &engine_space::nearby::SurfaceRecipe) -> WorldRecipe {
    WorldRecipe {
        kind: match s.class {
            WorldClass::Airless => WorldKind::Airless,
            WorldClass::Lava => WorldKind::Lava,
            WorldClass::Desert => WorldKind::Desert,
            WorldClass::Terra => WorldKind::Terra,
            WorldClass::Ocean => WorldKind::Ocean,
            WorldClass::ColdDesert => WorldKind::ColdDesert,
            WorldClass::Ice => WorldKind::Ice,
            WorldClass::Hazy => WorldKind::Hazy,
        },
        seed: s.seed,
        radius_m: body.radius_m,
        sea_fraction: s.sea_fraction,
        crater_density: s.crater_density,
        pressure_bar: s.pressure_bar,
        temperature_k: s.temperature_k,
        star_kelvin: s.star_kelvin,
        color: body.color,
        lava_glow_nits: engine_space::look::melt_nits(body),
        locked: body.tidally_locked,
    }
}

/// Where the Moon is now. It moves along its orbit and, while the world turns with Earth,
/// round the sky once a day, so views of it are aimed when visited.
pub fn moon_now(world: &mut World) -> Option<DVec3> {
    let mut q = world.query::<(&CelestialBody, &Transform)>();
    let at = q
        .iter(world)
        .find(|(b, _)| b.name == "Moon")
        .map(|(_, t)| t.translation)?;
    // In the valley's home coordinates, wherever the world is anchored.
    Some(
        world
            .get_resource::<engine_space::SpaceState>()
            .map_or(at, |s| s.to_home(at)),
    )
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
pub fn paint(land: &Landscape, x: f64, z: f64) -> OverlayPaint {
    let h = land.height(x, z);
    // Forward differences over the metre `land.normal(x, z, 0.5)` spans: two more heights
    // instead of four, as this runs for every vertex of every terrain patch near the valley.
    let n = Vec3::new(
        (h - land.height(x + 1.0, z)) as f32,
        1.0,
        (h - land.height(x, z + 1.0)) as f32,
    )
    .normalize();
    let (cover, color) = land.cover_and_color(x, z, h, n);
    OverlayPaint::layer(layer(cover), 1.0).tinted(color.to_array(), TINT)
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

    /// `cargo test --release -p showcase paint_cost -- --ignored --nocapture`: what the
    /// terrain's patch builds pay per vertex for the overlay's height and paint.
    #[test]
    #[ignore]
    fn paint_cost() {
        let land = crate::landscape::shared();
        let mut rng = Rng::new(7);
        for (name, r0, r1) in [("valley", 0.0, 1500.0), ("hills", 1500.0, 15_000.0)] {
            let points: Vec<(f64, f64)> = (0..20_000)
                .map(|_| {
                    let a = rng.range(0.0, std::f32::consts::TAU) as f64;
                    let r = r0 + (r1 - r0) * (rng.f32() as f64).sqrt();
                    (r * a.cos(), r * a.sin())
                })
                .collect();
            let t = std::time::Instant::now();
            let heights: f64 = points.iter().map(|&(x, z)| land.height(x, z)).sum();
            let height_us = t.elapsed().as_secs_f64() * 1e6 / points.len() as f64;
            let time = |what: &str, f: &dyn Fn(f64, f64) -> f64| {
                let t = std::time::Instant::now();
                let sum: f64 = points.iter().map(|&(x, z)| f(x, z)).sum();
                let us = t.elapsed().as_secs_f64() * 1e6 / points.len() as f64;
                println!("  {what} {us:.2} us ({sum:.1})");
            };
            time("natural", &|x, z| land.natural(x, z));
            time("woodland", &|x, z| land.woodland(x, z));
            let up = Vec3::Y;
            time("cover", &|x, z| land.cover(x, z, 10.0, up) as u8 as f64);
            time("color", &|x, z| land.color(x, z, 10.0, up).x as f64);
            let t = std::time::Instant::now();
            let tints: f32 = points.iter().map(|&(x, z)| paint(land, x, z).tint[1]).sum();
            let paint_us = t.elapsed().as_secs_f64() * 1e6 / points.len() as f64;
            // As a far patch asks: a 2 km square, a tap every 16 m.
            let cache = crate::paint_cache::PaintCache::new(land.clone(), paint);
            let (cx, cz) = points[0];
            let patch: Vec<(f64, f64)> = (0..125 * 125)
                .map(|k| (cx + (k % 125) as f64 * 16.0, cz + (k / 125) as f64 * 16.0))
                .collect();
            for (what, footprint) in [("exact", 0.0), ("baked cold", 16.0), ("baked warm", 16.0)] {
                let t = std::time::Instant::now();
                let sum: f32 = patch
                    .iter()
                    .map(|&(x, z)| cache.paint_for(x, z, footprint).tint[1])
                    .sum();
                let us = t.elapsed().as_secs_f64() * 1e6 / patch.len() as f64;
                println!("  patch taps, {what}: {us:.2} us ({sum:.2})");
            }
            println!(
                "{name}: height {height_us:.2} us, paint {paint_us:.2} us (sums {heights:.3}, {tints:.4})"
            );
        }
    }
}
