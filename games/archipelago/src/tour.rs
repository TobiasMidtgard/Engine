//! `--tour`: run the start for a while without drawing, then fly to each view, wait for the
//! ground and the trees, and save `screenshots/archipelago/<key>.png`.

use engine::prelude::*;
use engine::render::{RenderDisabled, RenderPaused, capture_now};

use crate::camera::StrategyCam;
use crate::fleet::Vessel;
use crate::island::islands;
use crate::sim::Building;
use crate::vessels::Craft;

/// Where a view looks from.
enum Aim {
    /// The home town's middle.
    Town,
    Island(usize),
    /// The first building of this kind on an island.
    Kind(crate::defs::Kind, usize),
    /// The first vessel of this craft.
    Craft(Craft),
    At(f64, f64),
}

struct View {
    key: &'static str,
    aim: Aim,
    /// Offset from the aim, meters (x, z).
    shift: (f64, f64),
    yaw: f32,
    dist: f32,
    pitch_deg: Option<f32>,
    /// Hide the HUD for this one.
    clean: bool,
}

const fn v(key: &'static str, aim: Aim, yaw: f32, dist: f32) -> View {
    View {
        key,
        aim,
        shift: (0.0, 0.0),
        yaw,
        dist,
        pitch_deg: None,
        clean: false,
    }
}

fn views() -> Vec<View> {
    use crate::defs::Kind as K;
    vec![
        View {
            pitch_deg: Some(52.0),
            ..v("archipelago", Aim::At(-150.0, 250.0), 0.35, 5200.0)
        },
        v("town", Aim::Town, 0.6, 330.0),
        View {
            clean: true,
            pitch_deg: Some(24.0),
            ..v("town_street", Aim::Town, 2.1, 110.0)
        },
        v("harbour", Aim::Kind(K::Harbour, 0), 3.4, 190.0),
        v("market", Aim::Kind(K::Market, 0), 1.2, 90.0),
        View {
            pitch_deg: Some(30.0),
            ..v("farms", Aim::Kind(K::SheepFarm, 0), 4.0, 260.0)
        },
        v("brannsey", Aim::Island(1), 0.9, 1400.0),
        View {
            pitch_deg: Some(26.0),
            ..v("mill", Aim::Kind(K::Mill, 1), 5.2, 120.0)
        },
        View {
            pitch_deg: Some(18.0),
            clean: true,
            ..v("airship", Aim::Craft(Craft::Airship), 2.6, 260.0)
        },
        View {
            pitch_deg: Some(22.0),
            clean: true,
            ..v("steamer", Aim::Craft(Craft::Steamer), 1.9, 150.0)
        },
        v("northmoor", Aim::Island(3), 4.4, 1300.0),
    ]
}

fn aim_point(world: &mut World, aim: &Aim) -> Option<DVec3> {
    Some(match aim {
        Aim::Town => world.get_resource::<crate::town::TownCentre>()?.0,
        Aim::Island(i) => {
            let c = islands().get(*i)?.center;
            DVec3::new(c.x, 0.0, c.y)
        }
        Aim::Kind(k, isle) => {
            let mut q = world.query::<&Building>();
            let b = q
                .iter(world)
                .filter(|b| b.kind == *k && b.island == *isle)
                .min_by_key(|b| b.id)?
                .clone();
            let c = crate::grid::footprint_center(b.kind, b.origin, b.rot);
            DVec3::new(c.x, b.base_y, c.y)
        }
        Aim::Craft(c) => {
            let mut q = world.query::<(&Vessel, &Transform)>();
            q.iter(world)
                .find(|(v, _)| v.craft == *c)
                .map(|(_, t)| t.translation)?
        }
        Aim::At(x, z) => DVec3::new(*x, 0.0, *z),
    })
}

/// Frames until the terrain's patches and the foliage are in, at most `cap_s` wall seconds.
fn settle(app: &mut App, cap_s: f32) {
    let started = std::time::Instant::now();
    loop {
        app.update();
        let w = &app.world;
        let (building, missing) = w
            .get_resource::<engine::core::diagnostics::Diagnostics>()
            .map_or((0.0, 0.0), |d| {
                (
                    d.get("terrain.patches_building").unwrap_or(0.0),
                    d.get("terrain.patches_missing").unwrap_or(0.0),
                )
            });
        let foliage = w
            .get_resource::<engine_foliage::FoliageLoading>()
            .is_some_and(|f| !f.is_ready());
        let elapsed = started.elapsed().as_secs_f32();
        if building + missing == 0.0 && !foliage && elapsed > 0.5 {
            break;
        }
        if elapsed > cap_s {
            engine::core::log::warn!(
                "tour: gave up waiting ({building} patches building, {missing} missing, foliage {foliage})"
            );
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
}

/// Run the tour instead of the game loop.
pub fn run(mut app: App, only: Option<&str>) {
    let out = "screenshots/archipelago";
    let _ = std::fs::create_dir_all(out);
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    let started = std::time::Instant::now();
    // Run the start for a minute of game time without drawing: carts out, ships under way.
    app.world.insert_resource(RenderPaused);
    app.update_n(2);
    app.world.resource_mut::<crate::sim::Economy>().speed = 4.0;
    app.update_n(450);
    app.world.resource_mut::<crate::sim::Economy>().speed = 1.0;
    app.world.remove_resource::<RenderPaused>();
    engine::core::log::info!(
        "tour: warmed up in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    if let Ok(state) =
        engine::core::registry::call_api(&mut app.world, "isle.state", serde_json::json!({}))
    {
        let _ = std::fs::write(
            format!("{out}/state.json"),
            serde_json::to_string_pretty(&state).unwrap_or_default(),
        );
    }
    if let Ok(list) =
        engine::core::registry::call_api(&mut app.world, "isle.list", serde_json::json!({}))
    {
        let _ = std::fs::write(
            format!("{out}/buildings.json"),
            serde_json::to_string_pretty(&list).unwrap_or_default(),
        );
    }
    let keys: Option<Vec<&str>> = only.map(|s| s.split(',').collect());
    for view in views() {
        if keys.as_ref().is_some_and(|k| !k.contains(&view.key)) {
            continue;
        }
        let Some(p) = aim_point(&mut app.world, &view.aim) else {
            engine::core::log::warn!("tour: nothing to aim {} at", view.key);
            continue;
        };
        {
            let mut q = app.world.query::<&mut StrategyCam>();
            if let Some(mut cam) = q.iter_mut(&mut app.world).next() {
                cam.goal_focus = DVec3::new(p.x + view.shift.0, p.y, p.z + view.shift.1);
                cam.goal_yaw = view.yaw;
                cam.goal_dist = view.dist;
                cam.pitch_override = view.pitch_deg.map(f32::to_radians);
                cam.free_height = matches!(view.aim, Aim::Craft(_));
                cam.snap();
            }
        }
        set_hud(&mut app.world, !view.clean);
        let t0 = std::time::Instant::now();
        settle(&mut app, 240.0);
        // Exposure and the environment, following a moving craft.
        for _ in 0..8 {
            if matches!(view.aim, Aim::Craft(_))
                && let Some(p) = aim_point(&mut app.world, &view.aim)
            {
                let mut q = app.world.query::<&mut StrategyCam>();
                if let Some(mut cam) = q.iter_mut(&mut app.world).next() {
                    cam.goal_focus = p;
                    cam.free_height = true;
                    cam.snap();
                }
            }
            app.update();
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{}.png", view.key);
        match capture_now(&mut app.world, &path) {
            Ok(_) => engine::core::log::info!("tour: {path} ({:.1} s)", t0.elapsed().as_secs_f32()),
            Err(e) => engine::core::log::warn!("tour: {path}: {e}"),
        }
    }
}

/// Show or hide the game's bars.
fn set_hud(world: &mut World, on: bool) {
    let mut q = world.query::<(&engine_ui::node::Screen, &mut engine_ui::node::Node)>();
    for (s, mut node) in q.iter_mut(world) {
        if s.key.starts_with("isle_") {
            node.style.opacity = Some(if on { 1.0 } else { 0.0 });
        }
    }
}
