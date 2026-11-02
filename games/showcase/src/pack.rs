//! The player's bag and map in play (playtest 3 #5): Tab opens the inventory and M the map,
//! as PLAY.txt says, and either key (or Escape) closes it again. The UI module draws both
//! screens; the showcase gives them something true to show: the pilot's own gear in the bag,
//! and on the map Earth with the valley's zones as places and the player where they stand.

use engine::prelude::*;
use engine_ui::items::{Inventory, ItemStack};
use engine_ui::map::{MapData, MapLocation, Planet, PlanetKind, Poi, PoiKind, StarSystem};
use engine_ui::screens;
use serde_json::json;

use crate::landscape::PLANET_RADIUS;
use crate::zones::ZONES;

/// Where the valley's middle (the plaza) lies on Earth, degrees (`planet::spawn`'s pad).
const HOME_LAT: f64 = 40.0;
const HOME_LON: f64 = 0.0;

/// Install the keys, the pilot's gear and the map's data.
pub fn install(app: &mut App) {
    app.add_systems(Startup, fill_map)
        .add_systems(Update, (gear_the_pilot, toggle_screens, track_player));
}

/// Latitude and longitude (degrees) of a world point near the valley: north is +Z and east
/// is -X (the valley's and the weather's compass, `planet::HEADING_DEG`).
pub fn lat_lon(p: DVec3) -> (f64, f64) {
    let lat = HOME_LAT + (p.z / PLANET_RADIUS).to_degrees();
    let lon = HOME_LON + (-p.x / (PLANET_RADIUS * HOME_LAT.to_radians().cos())).to_degrees();
    (lat, lon)
}

/// What kind of place each zone is on the map.
fn kind(key: &str) -> PoiKind {
    match key {
        "plaza" => PoiKind::City,
        "market" => PoiKind::Vendor,
        "spaceport" | "airfield" => PoiKind::Spaceport,
        "cave" => PoiKind::Cave,
        "arena" | "fortress" | "range" => PoiKind::Danger,
        "field" | "paddock" => PoiKind::Npc,
        _ => PoiKind::Outpost,
    }
}

/// The real sky and the valley's zones, ahead of the UI demo's made-up galaxy.
fn fill_map(mut map: ResMut<MapData>) {
    let pois = ZONES
        .iter()
        .map(|z| {
            let (lat, lon) = lat_lon(z.center);
            Poi {
                id: z.key.into(),
                name: z.name.into(),
                kind: kind(z.key),
                lat,
                lon,
                discovered: true,
                description: format!("A showcase zone: {}.", z.name),
            }
        })
        .collect();
    let earth = Planet {
        id: "earth".into(),
        name: "Earth".into(),
        kind: PlanetKind::Terran,
        seed: 3.0,
        radius_km: PLANET_RADIUS / 1000.0,
        orbit_au: 1.0,
        phase_deg: 40.0,
        period_days: 365.25,
        moons: 1,
        gravity_g: 1.0,
        atmosphere: "Nitrogen and oxygen, breathable".into(),
        population: "The valley".into(),
        description: "Home: the valley lies at 40° N, under the northern range.".into(),
        pois,
        ..Default::default()
    };
    let moon_and_mars = [
        Planet {
            id: "mercury".into(),
            name: "Mercury".into(),
            kind: PlanetKind::Barren,
            radius_km: 2_439.7,
            orbit_au: 0.39,
            phase_deg: 210.0,
            period_days: 88.0,
            gravity_g: 0.38,
            atmosphere: "None".into(),
            ..Default::default()
        },
        Planet {
            id: "venus".into(),
            name: "Venus".into(),
            kind: PlanetKind::Desert,
            radius_km: 6_051.8,
            orbit_au: 0.72,
            phase_deg: 120.0,
            period_days: 224.7,
            gravity_g: 0.9,
            atmosphere: "Carbon dioxide, crushing".into(),
            ..Default::default()
        },
        earth,
        Planet {
            id: "mars".into(),
            name: "Mars".into(),
            kind: PlanetKind::Desert,
            seed: 7.0,
            radius_km: 3_389.5,
            orbit_au: 1.52,
            phase_deg: 300.0,
            period_days: 687.0,
            moons: 2,
            gravity_g: 0.38,
            atmosphere: "Thin carbon dioxide".into(),
            description: "Walkable ground: fly there from the spaceport.".into(),
            ..Default::default()
        },
        Planet {
            id: "jupiter".into(),
            name: "Jupiter".into(),
            kind: PlanetKind::Gas,
            radius_km: 69_911.0,
            orbit_au: 5.2,
            phase_deg: 75.0,
            period_days: 4_333.0,
            moons: 95,
            gravity_g: 2.53,
            atmosphere: "Hydrogen and helium".into(),
            ..Default::default()
        },
        Planet {
            id: "saturn".into(),
            name: "Saturn".into(),
            kind: PlanetKind::Gas,
            seed: 2.0,
            radius_km: 58_232.0,
            orbit_au: 9.58,
            phase_deg: 160.0,
            period_days: 10_759.0,
            rings: true,
            moons: 146,
            gravity_g: 1.07,
            atmosphere: "Hydrogen and helium".into(),
            ..Default::default()
        },
    ];
    let sol = StarSystem {
        id: "sol".into(),
        name: "Sol".into(),
        x: 0.32,
        y: -0.18,
        star_class: "G2".into(),
        star_size: 1.0,
        description: "The Sun and its planets.".into(),
        planets: moon_and_mars.into(),
        ..Default::default()
    };
    // The nearest stars, where they lie seen from above the galaxy's disk (a sketch: the
    // disk is 1 across, so they all sit right by the Sun).
    let near = |id: &str, name: &str, class: &str, dx: f32, dy: f32| StarSystem {
        id: id.into(),
        name: name.into(),
        x: 0.32 + dx,
        y: -0.18 + dy,
        star_class: class.into(),
        star_size: 0.6,
        planets: Vec::new(),
        ..Default::default()
    };
    map.galaxy.name = "Milky Way".into();
    map.systems = vec![
        sol,
        near("alpha_centauri", "Alpha Centauri", "G2", 0.03, 0.02),
        near("barnards_star", "Barnard's Star", "M4", -0.025, 0.035),
        near("sirius", "Sirius", "A1", 0.05, -0.03),
        near("tau_ceti", "Tau Ceti", "G8", -0.06, -0.04),
    ];
    map.lanes = vec![
        ["sol".into(), "alpha_centauri".into()],
        ["sol".into(), "barnards_star".into()],
        ["sol".into(), "sirius".into()],
        ["alpha_centauri".into(), "tau_ceti".into()],
    ];
    // The UI demo's made-up worlds stay too, farther out: the market street's map stops
    // select them by id.
    let demo = engine_ui::map::demo_map();
    map.systems.extend(demo.systems);
    map.lanes.extend(demo.lanes);
    map.player = MapLocation {
        system: "sol".into(),
        planet: "earth".into(),
        lon: HOME_LON,
        lat: HOME_LAT,
    };
}

/// The pilot carries what it really has in play: the sword it fights with, the grapple's rope,
/// a torch, the throwables and some food and medicine, with the sword in hand.
fn gear_the_pilot(
    mut commands: Commands,
    play: Option<Res<engine_movement::Play>>,
    bags: Query<(), With<Inventory>>,
) {
    let Some(pilot) = play.and_then(|p| p.pilot) else {
        return;
    };
    if bags.get(pilot).is_ok() {
        return;
    }
    let mut bag = Inventory::with_capacity(30);
    let carried = [
        ("rope", 1),
        ("torch", 1),
        ("grenade", 4),
        ("medkit", 2),
        ("potion_health", 3),
        ("potion_stamina", 2),
        ("bread", 3),
        ("compass", 1),
        ("map_scroll", 1),
    ];
    for (slot, (id, count)) in bag.slots.iter_mut().zip(carried) {
        *slot = Some(ItemStack::new(id, count));
    }
    bag.equipment.insert(
        engine_ui::items::EquipSlot::MainHand,
        ItemStack::new("sword", 1),
    );
    bag.gold = 120;
    if let Ok(mut e) = commands.get_entity(pilot) {
        e.insert(bag);
    }
}

/// Tab and M (Select and D-pad up on a gamepad) open and close the bag and the map.
///
/// A plain system that queues the change only on the frame a key goes down, so the frame
/// keeps no exclusive system for it (the lead's schedule audit).
fn toggle_screens(mut commands: Commands, actions: Option<Res<engine::input::ActionState>>) {
    let Some(actions) = actions else {
        return;
    };
    let (bag, map) = (
        actions.just_pressed("inventory"),
        actions.just_pressed("map"),
    );
    if bag || map {
        commands.queue(move |world: &mut World| toggle(world, bag));
    }
}

/// Open the bag (`bag`) or the map, or close it when it is open.
fn toggle(world: &mut World, bag: bool) {
    let key = if bag { "inventory" } else { "map" };
    if screens::find_screen(world, key).is_some() {
        screens::hide(world, key, false);
        return;
    }
    // One at a time: the other one closes.
    screens::hide(world, if bag { "map" } else { "inventory" }, true);
    let pilot = world
        .get_resource::<engine_movement::Play>()
        .and_then(|p| p.pilot);
    let args = if bag {
        match pilot {
            Some(_) => json!({ "owner": engine_movement::play::PILOT_NAME }),
            // Flying freely there is no body to carry anything: show the pilot's kit anyway
            // when the bag's owner is missing (the screen says so otherwise).
            None => json!({}),
        }
    } else {
        let here = world.resource::<MapData>().player.clone();
        json!({
            "zoom": 0.5,
            "planet": here.planet,
            "system": here.system,
            "focus": true,
        })
    };
    if let Err(e) = screens::show(world, key, &args) {
        engine::core::log::warn!("{key}: {e}");
        engine_ui::toast::notify(world, engine_ui::toast::ToastKind::Bad, "", key, &e, 4.0);
    }
}

/// Keep the map's "you are here" on the pilot (or the camera when flying freely), twice a
/// second.
fn track_player(
    time: Res<Time>,
    mut timer: Local<f32>,
    mut map: ResMut<MapData>,
    play: Option<Res<engine_movement::Play>>,
    bodies: Query<&GlobalTransform>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    *timer -= time.real_delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.5;
    let at = play
        .and_then(|p| p.pilot)
        .and_then(|p| bodies.get(p).ok())
        .map(|t| t.translation())
        .or_else(|| {
            cameras
                .iter()
                .filter(|(c, _)| c.active)
                .max_by_key(|(c, _)| c.order)
                .map(|(_, t)| t.translation())
        });
    let Some(at) = at else {
        return;
    };
    // Near home only (within a few hundred kilometers the flat sketch holds); farther out the
    // marker stays where the player left the valley.
    if DVec2::new(at.x, at.z).length() > 300_000.0 || at.y > 100_000.0 {
        return;
    }
    let (lat, lon) = lat_lon(at);
    if map.player.planet == "earth"
        && (map.player.lat - lat).abs() + (map.player.lon - lon).abs() < 1e-6
    {
        return;
    }
    map.player = MapLocation {
        system: "sol".into(),
        planet: "earth".into(),
        lat,
        lon,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compass_puts_north_at_plus_z_and_east_at_minus_x() {
        let (lat0, lon0) = lat_lon(DVec3::ZERO);
        assert_eq!((lat0, lon0), (HOME_LAT, HOME_LON));
        let (lat, lon) = lat_lon(DVec3::new(0.0, 0.0, 1000.0));
        assert!(lat > lat0 && (lon - lon0).abs() < 1e-12);
        let (lat, lon) = lat_lon(DVec3::new(-1000.0, 0.0, 0.0));
        assert!(lon > lon0 && (lat - lat0).abs() < 1e-12);
        // A kilometer is about 0.009° of latitude.
        let (lat, _) = lat_lon(DVec3::new(0.0, 0.0, 1000.0));
        assert!((lat - lat0 - 0.008993).abs() < 1e-5);
    }

    #[test]
    fn every_zone_is_a_place_on_earth() {
        let mut app = App::new();
        app.init_resource::<MapData>();
        app.add_systems(Update, fill_map);
        app.update();
        let map = app.world.resource::<MapData>();
        let (_, earth) = map.planet("earth").expect("earth");
        assert_eq!(earth.pois.len(), ZONES.len());
        assert_eq!(map.player.planet, "earth");
        assert!(map.poi("spaceport").is_some());
    }
}
