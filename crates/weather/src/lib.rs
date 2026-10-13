#![doc = include_str!("../README.md")]

pub mod api;
pub mod climate;
pub mod cloudmap;
pub mod demo;
pub mod lightning;
pub mod presets;
pub mod render;
mod shift;
pub mod sim;
pub mod sky;
pub mod surface;

use engine_core::prelude::*;
use engine_render::AmbientLight;
use engine_world::{
    FocusPoints, Ground, HeatField, IgniteRequest, NoiseEvent, SurfaceLayers, SurfaceMaterials,
    WeatherState, WindField, WorldImpulse,
};

pub use climate::Climate;
pub use cloudmap::CloudMap;
pub use lightning::{ActiveBolts, Lightning};
pub use presets::{Regime, WeatherPreset};
pub use render::{CloudSettings, GroundEffectSettings, PrecipitationSettings};
pub use sim::{ForecastStep, Weather, WeatherSim};
pub use sky::{MoonLight, Sky, SkyLights, SunLight, spawn_sky_lights};
pub use surface::{GroundPatch, GroundWeather, WeatherShelter};

/// Dynamic weather, wind, temperature, volumetric clouds, rain and snow that change the
/// ground, lightning, and the day–night sky. Add it after `DefaultPlugins`:
///
/// ```ignore
/// app.add_plugins((DefaultPlugins, engine_weather::WeatherPlugin));
/// // Later, from game code, the console or an AI agent:
/// world.resource_mut::<engine_weather::Weather>().preset = WeatherPreset::Storm;
/// // or: call_api(world, "weather.set", json!({"preset": "storm", "transition_s": 300}))
/// ```
pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        // Shared world resources (normally from WorldPlugin; harmless if already there).
        app.init_resource::<WeatherState>()
            .init_resource::<WindField>()
            .init_resource::<SurfaceLayers>()
            .init_resource::<HeatField>()
            .init_resource::<Ground>()
            .init_resource::<SurfaceMaterials>()
            .init_resource::<FocusPoints>()
            .init_resource::<AmbientLight>()
            .add_message::<IgniteRequest>()
            .add_message::<NoiseEvent>()
            .add_message::<WorldImpulse>();

        let weather = app
            .world
            .get_resource::<Weather>()
            .cloned()
            .unwrap_or_default();
        app.insert_resource(sim::WeatherSim::new(&weather))
            .insert_resource(weather)
            .init_resource::<Climate>()
            .init_resource::<Sky>()
            .init_resource::<SkyLights>()
            .init_resource::<GroundWeather>()
            .init_resource::<surface::GroundCache>()
            .init_resource::<Lightning>()
            .init_resource::<ActiveBolts>()
            .init_resource::<lightning::StrikeRequests>()
            .init_resource::<CloudMap>()
            .register_resource::<Weather>(
                "What the weather should do: preset (clear, partly_cloudy, overcast, showers, rain, storm, \
                 snow, blizzard, fog), transition_s (game seconds for a front to arrive), auto \
                 (pick new weather by itself), forecast queue, time_scale, wind_from_deg (0 = \
                 from north/+Z, 90 = from east/−X). WeatherState is the simulated result.",
            )
            .register_resource::<Climate>(
                "Where the world is: latitude_deg, axial_tilt_deg, mean_temperature_c, \
                 seasonal_swing_c, daily_swing_c (°C), year_days.",
            )
            .register_resource::<Sky>(
                "Sky clock: hour (0–24), day_of_year, hours_per_second (0 = stopped), sun_lux, \
                 moon_lux, moon_phase (0 new, 0.5 full), night_fill (cd/m²).",
            )
            .register_resource::<GroundWeather>(
                "How rain and snow change the ground: wetting_mm, max_puddle_m, puddle_share, \
                 evaporation_mm_h, infiltration_mm_h, snow_ratio, melt_mm_per_c_day, \
                 shelter_rays, rate_hz, time_scale.",
            )
            .register_resource::<Lightning>(
                "Lightning: flashes_per_min at full storm, ground_share, radius_m, ignites \
                 (start fires), flash_lumens.",
            )
            .register_component::<SunLight>(
                "This DirectionalLight is the sun: the sky clock aims and dims it.",
            )
            .register_component::<MoonLight>(
                "This DirectionalLight is the moon: the sky clock aims it and sets its phase.",
            )
            .register_component::<WeatherShelter>(
                "Keeps rain and snow off the ground below it (half_extents X/Z in meters) for \
                 covers without physics colliders.",
            )
            .register_component::<GroundPatch>(
                "Ground the terrain doesn't describe (a paved yard, a sand pit): half_extents \
                 X/Z in meters, absorbency (0 asphalt .. 1 sand), puddles (share that pools).",
            )
            .add_systems(Update, (sky::ensure_lights, demo::instruments_system))
            .add_systems(
                PostUpdate,
                shift::shift_system.in_set(engine_world::WorldShiftSet::Apply),
            )
            .add_systems(
                FixedUpdate,
                (
                    sky::sky_system,
                    sim::weather_system,
                    cloudmap::cloud_map_system,
                    surface::ground_system,
                    lightning::lightning_system,
                )
                    .chain()
                    .in_set(SimSet::Environment),
            );
        api::register(app);
        render::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::registry::call_api;
    use serde_json::json;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, WeatherPlugin));
        app
    }

    #[test]
    fn plugin_registers_and_runs() {
        let mut app = app();
        app.update_n(3);
        let registry = app.world.resource::<TypeRegistry>();
        for name in ["Weather", "Climate", "Sky", "GroundWeather", "Lightning"] {
            assert!(registry.resource(name).is_some(), "{name} not registered");
        }
        // A world without lights gets a sun and a moon.
        let mut q = app.world.query::<&SunLight>();
        assert_eq!(q.iter(&app.world).count(), 1);
    }

    #[test]
    fn api_sets_and_fast_forwards_the_weather() {
        let mut app = app();
        app.update_n(2);
        let w = &mut app.world;
        call_api(
            w,
            "weather.set",
            json!({"preset": "rain", "transition_s": 120}),
        )
        .unwrap();
        let err = call_api(w, "weather.set", json!({"preset": "lava"})).unwrap_err();
        assert!(err.contains("Use one of"), "{err}");
        let out = call_api(w, "weather.advance", json!({"seconds": 900})).unwrap();
        assert_eq!(out["here"], "rain", "{out}");
        assert!(out["precipitation_mm_h"].as_f64().unwrap() > 1.0, "{out}");
        let surface = w.resource::<SurfaceLayers>();
        assert!(surface.sample(engine_core::math::DVec3::ZERO).wetness > 0.5);

        let t = call_api(w, "weather.time", json!({"hour": 18.5})).unwrap();
        assert_eq!(t["hour"], 18.5);
        call_api(
            w,
            "weather.forecast",
            json!({"steps": [{"preset": "storm", "after_s": 0, "transition_s": 0}]}),
        )
        .unwrap();
        let out = call_api(w, "weather.advance", json!({"seconds": 10})).unwrap();
        assert_eq!(out["here"], "storm", "{out}");
        call_api(w, "weather.lightning", json!({"at": [10, 0, 10]})).unwrap();
        assert!(
            call_api(w, "weather.presets", json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .len()
                == 9
        );
    }

    #[test]
    fn ground_api_fills_only_the_dips_the_ground_has() {
        let mut app = app();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        // A sand pit where nothing pools next to an asphalt yard full of dips.
        app.world.spawn((
            Transform::default(),
            GroundPatch {
                half_extents: [6.0, 6.0],
                absorbency: 0.9,
                puddles: 0.0,
            },
        ));
        app.world.spawn((
            Transform::from_translation(engine_core::math::DVec3::new(20.0, 0.0, 20.0)),
            GroundPatch {
                half_extents: [9.0, 9.0],
                absorbency: 0.05,
                puddles: 0.5,
            },
        ));
        app.update_n(40);
        call_api(&mut app.world, "weather.ground", json!({"puddle_m": 0.02})).unwrap();
        let surface = app.world.resource::<SurfaceLayers>();
        let puddled = |x: f64, z: f64, r: f64| {
            let mut n = 0;
            for i in 0..21 {
                for j in 0..21 {
                    let p = engine_core::math::DVec3::new(
                        x + (i as f64 / 10.0 - 1.0) * r,
                        0.0,
                        z + (j as f64 / 10.0 - 1.0) * r,
                    );
                    if surface.sample(p).puddle_m > 0.001 {
                        n += 1;
                    }
                }
            }
            n
        };
        assert_eq!(puddled(0.0, 0.0, 5.0), 0, "puddles in the sand pit");
        assert!(puddled(20.0, 20.0, 8.0) > 60, "no puddles on the asphalt");
    }

    #[test]
    fn volumetric_clouds_switch_the_renderers_flat_cloud_layer_off_and_back() {
        let mut app = app();
        app.world
            .insert_resource(engine_render::RenderSettings::default());
        app.update();
        let layer = |app: &App| {
            app.world
                .resource::<engine_render::RenderSettings>()
                .sky_cloud_layer
        };
        assert!(!layer(&app), "flat layer drawn under the volumetric clouds");
        app.world.resource_mut::<render::CloudSettings>().enabled = false;
        app.update();
        assert!(layer(&app), "flat layer not back with the clouds off");
        // Someone else's choice stays: off before the clouds came on, off after they go.
        app.world
            .resource_mut::<engine_render::RenderSettings>()
            .sky_cloud_layer = false;
        app.update();
        app.world.resource_mut::<render::CloudSettings>().enabled = true;
        app.update();
        app.world.resource_mut::<render::CloudSettings>().enabled = false;
        app.update();
        assert!(!layer(&app));
    }

    #[test]
    fn ground_api_finds_the_dips_right_after_the_player_moves() {
        // A teleport moves the ground grid, and a tour sets the ground before the ground
        // simulation has looked at the new place: water must still only go where it pools.
        let mut app = app();
        let far = engine_core::math::DVec3::new(300.0, 0.0, 300.0);
        app.world.spawn((
            Transform::from_translation(far),
            GroundPatch {
                half_extents: [9.0, 9.0],
                absorbency: 0.05,
                puddles: 0.5,
            },
        ));
        app.update_n(3);
        app.world.resource_mut::<FocusPoints>().points = vec![far];
        app.world
            .resource_mut::<SurfaceLayers>()
            .grid
            .recenter(engine_core::math::DVec2::new(far.x, far.z));
        call_api(
            &mut app.world,
            "weather.ground",
            json!({"wetness": 1.0, "puddle_m": 0.02}),
        )
        .unwrap();
        let surface = app.world.resource::<SurfaceLayers>();
        let puddled = |x: f64, z: f64, r: f64| {
            let mut n = 0;
            for i in 0..21 {
                for j in 0..21 {
                    let p = far
                        + engine_core::math::DVec3::new(
                            x + (i as f64 / 10.0 - 1.0) * r,
                            0.0,
                            z + (j as f64 / 10.0 - 1.0) * r,
                        );
                    if surface.sample(p).puddle_m > 0.001 {
                        n += 1;
                    }
                }
            }
            n
        };
        // The test world's ground is a grass meadow: it drains.
        let meadow = puddled(-40.0, 0.0, 20.0);
        assert!(meadow < 20, "{meadow} of 441 meadow samples hold water");
        assert!(puddled(0.0, 0.0, 8.0) > 60, "no puddles on the asphalt");
    }
}
