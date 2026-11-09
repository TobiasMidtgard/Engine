//! Setting the stage: the islands' ground, the sea, the sky and the woods.

use engine::prelude::*;
use engine_foliage::{PlantLibrary, SceneryTree};
use engine_terrain::{FlatPad, PlanetTerrain};
use engine_water::{WaterBed, WaterBody};

use crate::build::Forest;
use crate::grid;
use crate::island;

/// How thickly to plant the woods (1 = a tree about every 8 m in a wood).
#[derive(Resource, Clone, Copy)]
pub struct WoodsDensity(pub f32);

/// Spawn the ground, the sea and the sky's settings.
pub fn spawn_world(world: &mut World) {
    let pad = FlatPad {
        height_m: 0.0,
        mountains: 0.0,
        radius_m: 50.0,
        blend_m: 50.0,
        sea_distance_m: 0.0,
        ..Default::default()
    };
    let mut terrain = PlanetTerrain::flat(48_000.0, 7).with_pad(pad);
    terrain.shape.ocean_fraction = 1.0;
    terrain.shape.rivers = false;
    terrain.shape.mountain_coverage = 0.0;
    terrain.ocean_surface = false;
    terrain.colliders = false;
    terrain.landmarks = Default::default();
    let terrain = terrain
        .with_overlay(island::MAP_RADIUS_M, 3000.0, island::height)
        .with_overlay_paint(island::paint);
    let home = terrain.home_transform(pad);
    world.spawn((Name::new("Archipelago ground"), home, terrain));
    world.spawn((
        Name::new("Sea"),
        Transform::default(),
        WaterBody::ocean().with_bed(WaterBed::Ground),
    ));
    if let Some(mut sky) = world.get_resource_mut::<engine_weather::Sky>() {
        sky.hour = 15.5;
        sky.day_of_year = 160.0;
        sky.hours_per_second = 0.0;
    }
    if let Some(mut w) = world.get_resource_mut::<engine_weather::Weather>() {
        w.preset = engine_weather::WeatherPreset::Clear;
        w.transition_s = 0.0;
        w.auto = false;
        w.wind_from_deg = 240.0;
    }
}

fn hash(x: i64, z: i64, k: u64) -> f64 {
    engine::core::math::hash01(x, z, 0, k) as f64
}

/// Plant the woods as scenery trees, recorded by build cell so building can clear them.
pub fn plant_woods(world: &mut World) {
    let density = world
        .get_resource::<WoodsDensity>()
        .map_or(1.0, |d| d.0)
        .max(0.05);
    let spacing = 8.0 / (density as f64).sqrt();
    let Some(library) = world.get_resource::<PlantLibrary>().cloned() else {
        return;
    };
    let mut trees = Vec::new();
    for isle in island::islands() {
        let r = isle.radius * 1.35;
        let (x0, z0) = (isle.center.x - r, isle.center.y - r);
        let n = (2.0 * r / spacing) as i64;
        for j in 0..n {
            for i in 0..n {
                let jx = hash(i, j, 1) - 0.5;
                let jz = hash(i, j, 2) - 0.5;
                let x = x0 + (i as f64 + 0.5 + jx * 0.9) * spacing;
                let z = z0 + (j as f64 + 0.5 + jz * 0.9) * spacing;
                let w = island::woods(x, z);
                if w <= 0.02 || hash(i, j, 3) > w {
                    continue;
                }
                let h = island::height(x, z);
                if island::slope(x, z, 3.0) > 0.9 {
                    continue;
                }
                // Conifers on the hills and the north island, broadleaf on low ground.
                let conifer = (h / 40.0
                    + hash(i, j, 4) * 0.6
                    + if isle.name == "Northmoor" { 0.35 } else { 0.0 })
                .clamp(0.0, 1.0);
                let species = if conifer > 0.7 {
                    if hash(i, j, 5) > 0.5 {
                        "spruce"
                    } else {
                        "pine"
                    }
                } else if hash(i, j, 6) > 0.6 {
                    "birch"
                } else {
                    "oak"
                };
                trees.push((
                    x,
                    h,
                    z,
                    species,
                    (i as u64) << 20 ^ j as u64 ^ isle.seed << 40,
                    0.65 + 0.5 * hash(i, j, 7) as f32,
                    hash(i, j, 8) as f32,
                ));
            }
        }
    }
    let mut forest = Forest::default();
    for (x, y, z, species, seed, size, yaw) in trees {
        let Some(mut tree) = SceneryTree::new(&library, species, seed) else {
            continue;
        };
        tree.size = size;
        let e = world
            .spawn((
                Transform::from_xyz(x, y - 0.1, z)
                    .with_rotation(Quat::from_rotation_y(yaw * std::f32::consts::TAU)),
                tree,
            ))
            .id();
        forest
            .by_cell
            .entry(grid::cell_of(x, z))
            .or_default()
            .push(e);
    }
    engine::core::log::info!(
        "archipelago woods: {} trees",
        forest.by_cell.values().map(Vec::len).sum::<usize>()
    );
    world.insert_resource(forest);
}
