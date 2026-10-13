#![doc = include_str!("../README.md")]

pub mod demo;
pub mod domain;
pub mod fog;
pub mod noise;
pub mod obscurance;
pub mod render;
pub mod settings;
mod shift;
pub mod sim;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_world::{SmokeEmission, WorldPlugin, WorldShiftSet};
use serde_json::{Value, json};

pub use demo::SmokeDemo;
pub use domain::{Domain, FogRates, Voxel};
pub use fog::FogBank;
pub use obscurance::SmokeSnapshot;
pub use settings::SmokeSettings;
pub use sim::{
    SmokeCommand, SmokeCommands, SmokeDomains, SmokeFan, SmokeObstacle, SmokeSource, SmokeTunnels,
    SmokeVolume,
};

/// Smoke and gas: volumes of smoke that rise when hot, drift and swirl with the wind, get
/// punched through by bullets, blown apart by explosions and pushed aside by moving things.
/// See the crate docs.
pub struct SmokePlugin;

impl Plugin for SmokePlugin {
    fn build(&self, app: &mut App) {
        if !app.has_plugin::<WorldPlugin>() {
            app.add_plugins(WorldPlugin);
        }
        let settings = app
            .world
            .get_resource::<SmokeSettings>()
            .cloned()
            .unwrap_or_default();
        app.insert_resource(settings)
            .init_resource::<SmokeDomains>()
            .init_resource::<SmokeTunnels>()
            .init_resource::<SmokeCommands>()
            .init_resource::<sim::SmokeClock>()
            .register_resource::<SmokeSettings>(
                "Smoke tuning: sim_hz, buoyancy, weight, wind_coupling, vorticity (swirl), \
                 iterations, dissipation_secs, cool_secs, extinction_m2_kg (how thick it \
                 looks), auto volumes (auto_cell_m, auto_size_m, max_volumes), bullets \
                 (bullet_clear, tunnel_radius_m, tunnel_secs), detail, max_steps.",
            )
            .register_component::<SmokeVolume>(
                "A box where smoke is simulated: size [x, y, z] m and cell_m (voxel size). \
                 Transform at the center of its floor.",
            )
            .register_component::<SmokeSource>(
                "Keeps releasing smoke (grenade, chimney, vent): rate_kg_s, radius, \
                 temperature_c, velocity, color (linear RGB), duration_secs (0 = forever), \
                 active.",
            )
            .register_component::<SmokeObstacle>(
                "Pushes smoke aside as it moves (car, character): radius m.",
            )
            .register_component::<SmokeFan>(
                "A fan, vent or rotor downwash blowing along the entity's forward (-Z): speed \
                 m/s, radius m, reach m, on. Also stirs the shared WindField.",
            )
            .register_component::<FogBank>(
                "Ground fog over the floor of the SmokeVolume on the same entity: amount 0..1 \
                 (negative follows WeatherState.fog), visibility_m inside full fog, depth_m of \
                 its top above the lowest ground, top_fade_m, chill_c (cold fog pools in \
                 hollows), form_secs, burn_secs, color.",
            )
            .register_component::<SmokeDemo>(
                "The smoke-range demo: cycle_secs, warmup_secs, fire_rate, shooting [from, to], \
                 blast_at, fans [from, to] (seconds into each round).",
            )
            .add_systems(
                FixedUpdate,
                (
                    sim::sync_volumes,
                    sim::track_obstacles,
                    sim::release,
                    fog::settle_fog,
                    sim::disturb,
                    sim::step,
                    obscurance::publish,
                )
                    .chain()
                    .in_set(SimSet::Environment),
            )
            .add_systems(Update, (sim::age_tunnels, demo::run_demos))
            .add_systems(
                PostUpdate,
                (
                    shift::shift_smoke.in_set(WorldShiftSet::Apply),
                    shift::publish_shifted.in_set(WorldShiftSet::Rebuild),
                ),
            )
            .add_render_feature(render::SmokeFeature::default());
        register_apis(app);
    }
}

fn vec3_arg(args: &Value, key: &str) -> Result<DVec3, String> {
    let v = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("missing '{key}': a world position like [10, 2, -5]"))?;
    let f = |i: usize| v.get(i).and_then(|x| x.as_f64());
    match (f(0), f(1), f(2)) {
        (Some(x), Some(y), Some(z)) => Ok(DVec3::new(x, y, z)),
        _ => Err(format!("'{key}' must be three numbers, like [10, 2, -5]")),
    }
}

fn f32_arg(args: &Value, key: &str, default: f32) -> f32 {
    args.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn register_apis(app: &mut App) {
    app.register_api(
        "smoke.emit",
        "Release a puff of smoke: mass_kg (0.5 is a smoke grenade's cloud), radius m, \
         temperature_c (hot rises), color [r, g, b] linear (default white) or darkness 0..1, \
         velocity [x, y, z] m/s.",
        r#"{"at": [130, 1, 150], "mass_kg": 0.5, "radius": 1.5, "temperature_c": 40, "color": [0.9, 0.2, 0.1]}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let darkness = f32_arg(&args, "darkness", 0.0).clamp(0.0, 1.0);
            let color = match args.get("color").and_then(|v| v.as_array()) {
                Some(c) if c.len() == 3 => Vec3::new(
                    c[0].as_f64().unwrap_or(0.9) as f32,
                    c[1].as_f64().unwrap_or(0.9) as f32,
                    c[2].as_f64().unwrap_or(0.9) as f32,
                )
                .clamp(Vec3::ZERO, Vec3::ONE),
                Some(_) => return Err("'color' must be three numbers 0..1".into()),
                None => sim::albedo_of(darkness),
            };
            let velocity = match args.get("velocity") {
                Some(_) => vec3_arg(&args, "velocity")?.as_vec3(),
                None => Vec3::ZERO,
            };
            let emission = SmokeEmission {
                position: at,
                velocity,
                radius: f32_arg(&args, "radius", 1.0).max(0.05),
                mass_kg: f32_arg(&args, "mass_kg", 0.3).max(0.0),
                temperature_c: f32_arg(&args, "temperature_c", 40.0),
                darkness,
            };
            world
                .resource_mut::<SmokeCommands>()
                .0
                .push(SmokeCommand::Emit(emission, color));
            Ok(json!({"queued": true, "at": at.to_array()}))
        },
    )
    .register_api(
        "smoke.clear",
        "Remove all smoke.",
        "{}",
        |world, _| {
            world
                .resource_mut::<SmokeCommands>()
                .0
                .push(SmokeCommand::Clear);
            Ok(json!({"queued": true}))
        },
    )
    .register_api(
        "smoke.status",
        "Smoke volumes: where, how big, voxels, smoke in each (kg), and recent bullet tunnels.",
        "{}",
        |world, _| {
            let domains = world.resource::<SmokeDomains>();
            let list: Vec<Value> = domains
                .list
                .iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "explicit": d.owner.is_some(),
                        "min": d.domain.origin.to_array(),
                        "size_m": d.domain.size().to_array(),
                        "cell_m": d.domain.cell,
                        "voxels": d.domain.dims.to_array(),
                        "mass_kg": d.domain.mass_kg,
                        "max_density": d.domain.max_density,
                    })
                })
                .collect();
            Ok(json!({
                "mass_kg": domains.mass_kg(),
                "volumes": list,
                "tunnels": world.resource::<SmokeTunnels>().list.len(),
            }))
        },
    )
    .register_api(
        "smoke.sample",
        "Smoke at a point: density kg/m³, temperature above the air (°C), velocity m/s, color.",
        r#"{"at": [130, 2, 150]}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let domains = world.resource::<SmokeDomains>();
            Ok(match domains.find(at) {
                Some(i) => {
                    let v = domains.list[i].domain.sample(at);
                    json!({
                        "density": v.density,
                        "heat_c": v.heat,
                        "velocity": v.vel.to_array(),
                        "color": v.albedo().to_array(),
                    })
                }
                None => json!({"density": 0.0}),
            })
        },
    )
    .register_api(
        "smoke.demo",
        "The smoke-range demos: which round each is on and how far into it (s). \
         restart: true starts every demo's round over (its smoke clears, the grenades pop).",
        r#"{"restart": true}"#,
        |world, args| {
            let restart = args
                .get("restart")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut q = world.query::<&mut demo::SmokeDemoState>();
            let mut list = Vec::new();
            for mut state in q.iter_mut(world) {
                if restart {
                    state.restart();
                }
                list.push(json!({"round": state.rounds, "since": state.since}));
            }
            Ok(json!({"demos": list, "restarting": restart}))
        },
    )
    .register_api(
        "smoke.visibility",
        "How much light gets from one point to another through smoke (1 = clear, 0 = blind). \
         Use it for AI sight lines.",
        r#"{"from": [120, 1.7, 150], "to": [140, 1.7, 150]}"#,
        |world, args| {
            let from = vec3_arg(&args, "from")?;
            let to = vec3_arg(&args, "to")?;
            let extinction = world.resource::<SmokeSettings>().extinction_m2_kg;
            let t = world
                .resource::<SmokeDomains>()
                .transmittance(from, to, extinction);
            Ok(json!({"transmittance": t}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::{ProjectileTrace, WorldImpulse};

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(SmokePlugin);
        app
    }

    fn emit(app: &mut App, at: DVec3, kg: f32) {
        app.world.write_message(SmokeEmission {
            position: at,
            velocity: Vec3::ZERO,
            radius: 1.5,
            mass_kg: kg,
            temperature_c: 20.0,
            darkness: 0.2,
        });
    }

    #[test]
    fn released_smoke_makes_a_volume_that_drifts_with_the_wind() {
        let mut app = app();
        app.world.resource_mut::<engine_world::WeatherState>().wind = Vec3::new(3.0, 0.0, 0.0);
        let at = DVec3::new(10.0, 2.0, 10.0);
        emit(&mut app, at, 1.0);
        app.update_n(3);
        let domains = app.world.resource::<SmokeDomains>();
        assert_eq!(domains.list.len(), 1);
        assert!(domains.density(at) > 0.0);
        let mass = domains.mass_kg();
        assert!((mass - 1.0).abs() < 0.05, "{mass}");
        // The rest of the engine sees it through the obscurance contract.
        let sight = app
            .world
            .resource::<engine_world::Obscurance>()
            .transmittance(at - DVec3::X * 6.0, at + DVec3::X * 6.0);
        assert!(sight < 0.5, "{sight}");
        app.update_n(90);
        let domains = app.world.resource::<SmokeDomains>();
        let d = &domains.list[0].domain;
        // Center of mass moved downwind.
        let mut sum = DVec3::ZERO;
        let mut total = 0.0;
        for z in 0..d.dims.z {
            for y in 0..d.dims.y {
                for x in 0..d.dims.x {
                    let m = d.voxels[d.index(x, y, z)].density as f64;
                    sum += d.voxel_center(x, y, z) * m;
                    total += m;
                }
            }
        }
        let c = sum / total;
        assert!(c.x > at.x + 3.0, "{c:?}");
    }

    #[test]
    fn explicit_volume_holds_smoke_and_bullets_cut_through_it() {
        let mut app = app();
        app.world.spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            SmokeVolume {
                size: [16.0, 8.0, 16.0],
                cell_m: 0.5,
            },
        ));
        app.update();
        for _ in 0..10 {
            emit(&mut app, DVec3::new(0.0, 1.5, 0.0), 0.03);
            app.update();
        }
        app.update_n(10);
        {
            let domains = app.world.resource::<SmokeDomains>();
            assert_eq!(domains.list.len(), 1);
            assert!(domains.list[0].owner.is_some());
        }
        let a = DVec3::new(-7.0, 1.5, 0.0);
        let b = DVec3::new(7.0, 1.5, 0.0);
        let ext = app.world.resource::<SmokeSettings>().extinction_m2_kg;
        let mid = DVec3::new(0.0, 1.5, 0.0);
        let before = app
            .world
            .resource::<SmokeDomains>()
            .transmittance(a, b, ext);
        let thick = app.world.resource::<SmokeDomains>().density(mid);
        assert!(before < 0.05, "smoke too thin: {before}");
        app.world.write_message(ProjectileTrace {
            from: a,
            to: b,
            radius: 0.01,
            speed: 900.0,
            shooter: None,
        });
        app.update();
        let after = app
            .world
            .resource::<SmokeDomains>()
            .transmittance(a, b, ext);
        let thin = app.world.resource::<SmokeDomains>().density(mid);
        assert!(thin < thick * 0.2, "{thick} -> {thin}");
        assert!(
            after > before * 100.0 && after > 0.01,
            "{before} -> {after}"
        );
        assert_eq!(app.world.resource::<SmokeTunnels>().list.len(), 1);
    }

    #[test]
    fn grenade_source_runs_out_and_explosions_push_smoke_away() {
        let mut app = app();
        let grenade = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 0.3, 0.0),
                SmokeSource::grenade([0.85, 0.15, 0.1], 2.0),
            ))
            .id();
        app.update_n(90);
        assert!(!app.world.get::<SmokeSource>(grenade).unwrap().active);
        let domains = app.world.resource::<SmokeDomains>();
        let mass = domains.mass_kg();
        assert!(mass > 0.05, "{mass}");
        let at = DVec3::new(0.0, 1.0, 0.0);
        let before = domains.density(at);
        assert!(before > 0.0);
        let color = domains.list[0].domain.sample(at).albedo();
        assert!(color.x > color.y * 2.0, "{color:?}");
        app.world.write_message(WorldImpulse {
            center: DVec3::new(0.0, 0.5, 0.0),
            radius: 6.0,
            strength: 20.0,
            energy_j: 1.0e6,
            heat_c: 0.0,
            source: None,
        });
        app.update_n(10);
        let after = app.world.resource::<SmokeDomains>().density(at);
        assert!(after < before * 0.5, "{before} -> {after}");
    }

    #[test]
    fn apis_respond() {
        let mut app = app();
        let r = engine_core::registry::call_api(
            &mut app.world,
            "smoke.emit",
            json!({"at": [0, 2, 0], "mass_kg": 1.0, "radius": 2, "color": [0.2, 0.8, 0.2]}),
        )
        .unwrap();
        assert_eq!(r["queued"], true);
        app.update_n(3);
        let s = engine_core::registry::call_api(&mut app.world, "smoke.status", json!({})).unwrap();
        assert!(s["mass_kg"].as_f64().unwrap() > 0.9, "{s}");
        let c = engine_core::registry::call_api(
            &mut app.world,
            "smoke.sample",
            json!({"at": [0, 2, 0]}),
        )
        .unwrap();
        assert!(c["density"].as_f64().unwrap() > 0.0, "{c}");
        let v = engine_core::registry::call_api(
            &mut app.world,
            "smoke.visibility",
            json!({"from": [-10, 2, 0], "to": [10, 2, 0]}),
        )
        .unwrap();
        assert!(v["transmittance"].as_f64().unwrap() < 0.9, "{v}");
        engine_core::registry::call_api(&mut app.world, "smoke.clear", json!({})).unwrap();
        app.update_n(2);
        assert!(app.world.resource::<SmokeDomains>().mass_kg() < 1e-4);
        assert!(engine_core::registry::call_api(&mut app.world, "smoke.emit", json!({})).is_err());
    }
}
