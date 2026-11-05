//! Render the foliage showcase from several viewpoints into PNGs, headless:
//!
//! ```sh
//! cargo run -p engine_foliage --example foliage_shots --release -- screenshots/foliage [shot..]
//! ```
//!
//! Name shots after the directory (by number or name: `06`, `burn_scar`) to take only those.
//! `FOLIAGE_DEBUG_VIEW=sky|reflections|gi|normals|cascades` shows a renderer debug view instead
//! (on the ground and rocks, `cascades` shows where the sun reaches, tinted by shadow cascade).
//!
//! `FOLIAGE_FROZEN=1` keeps the live ecology still, so two runs show the same forest, and
//! `FOLIAGE_EV=<ev100>` fixes the exposure: for before/after comparisons.
//!
//! `FOLIAGE_QUALITY=ultra` renders at the Ultra preset with the showcase's shadow reach (the
//! fine shadow cascades reach about 190 m instead of 50 m).
//!
//! Shot `11_switch` photographs the clearing twice from one spot, every tree past its middle
//! level first drawn at its middle level and then at its far level: the shadows on the ground
//! should not change between them.
//!
//! `FOLIAGE_GRASS_STATS=1` prints what each grass and floor field holds once the forest has
//! grown (texels on, heights, kinds); name no shot that exists to print only that.
//!
//! `FOLIAGE_CENSUS=1` also prints, for each shot, the plant parts' triangles by level of detail:
//! in view (a cone a little wider than the camera's) and casting shadows (within 350 m).
//!
//! Grows the showcase forest and mushroom cave, then photographs them from the demo's
//! viewpoints: the old forest, the trail, the clear-cut, a tree mid-fall and after it lands, the
//! spring, the burn scar, the dry ridge, the clearing through the year (spring leaves, autumn
//! color, leaf fall, bare winter), the clearing ten years later, and the cave.

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::{
    FallingTree, FelledLog, FoliagePlugin, FoliageSeason, Plant, PlantLibrary, PlantStage,
    fell_tree,
};
use engine_foliage::{demo, season};
use engine_render::prelude::*;
use engine_render::{
    DebugView, Exposure, RenderDisabled, RenderSettings, Screenshots, ShadowTexelRange,
};
use engine_world::WeatherState;

const FOREST: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const CAVE: DVec3 = DVec3::new(400.0, 0.0, 0.0);
const STEP: f64 = 1.0 / 15.0;

fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
}

fn set_hour(world: &mut World, hour: f32) {
    let dir = sun_direction(hour);
    world.resource_mut::<WeatherState>().sun_direction = dir;
    world.resource_mut::<WeatherState>().time_of_day_h = hour;
    let mut q = world.query_filtered::<&mut Transform, With<DirectionalLight>>();
    for mut t in q.iter_mut(world) {
        let mut sun = Transform::default();
        sun.look_to(-dir, Vec3::Y);
        *t = sun;
    }
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3, hour: f32) {
    aim(&mut app.world, eye, target);
    set_hour(&mut app.world, hour);
    app.update_n(8);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    println!("saved {path}");
    if std::env::var("FOLIAGE_CENSUS").is_ok_and(|v| v == "1") {
        census(app, name);
    }
}

/// Plant parts' triangles by level of detail, in view and casting shadows.
fn census(app: &mut App, name: &str) {
    use engine_foliage::assets::FoliageVisuals;
    use engine_foliage::shadow_standin::ShadowStandIn;
    use engine_foliage::visuals::PlantPart;
    use std::collections::BTreeMap;
    let mut level = std::collections::HashMap::new();
    for vis in app
        .world
        .resource::<FoliageVisuals>()
        .species
        .iter()
        .flatten()
    {
        for v in &vis.variants {
            let mut put = |name: &'static str, m: &Option<Handle<Mesh>>| {
                if let Some(m) = m {
                    level.insert(m.index(), name);
                }
            };
            put("3 hero", &v.foliage_hero);
            put("4 near", &v.foliage);
            put("4 near", &v.wood);
            put("4 near", &v.base);
            put("5 mid", &v.foliage_mid);
            put("5 mid", &v.wood_mid);
            put("6 far", &v.foliage_lod);
            put("6 far", &v.wood_lod);
            put("7 distant", &v.foliage_distant);
            put("7 distant", &v.wood_distant);
            put("2 sapling", &v.sapling_foliage);
            put("2 sapling", &v.sapling_wood);
            for (name, list) in [
                ("4 near", &v.foliage_thin),
                ("5 mid", &v.foliage_mid_thin),
                ("6 far", &v.foliage_lod_thin),
                ("7 distant", &v.foliage_distant_thin),
                ("2 sapling", &v.sapling_foliage_thin),
            ] {
                for m in list {
                    level.insert(m.index(), name);
                }
            }
        }
    }
    let (eye, forward, half_fov) = {
        let mut q = app.world.query::<(&Camera, &GlobalTransform)>();
        let (cam, gt) = q.iter(&app.world).next().expect("a camera");
        let fov = match cam.projection {
            Projection::Perspective { fov_y_deg, .. } => fov_y_deg,
            _ => 60.0,
        };
        // 16:9, a little wider than the view.
        let half = (fov.to_radians() * 0.5).tan() * 16.0 / 9.0;
        (gt.translation(), gt.forward(), half.atan() * 1.15)
    };
    let mut view: BTreeMap<&str, (u32, u64)> = BTreeMap::new();
    let mut cast: BTreeMap<&str, (u32, u64)> = BTreeMap::new();
    let mut fine: BTreeMap<&str, (u32, u64)> = BTreeMap::new();
    let mut coarse: BTreeMap<&str, (u32, u64)> = BTreeMap::new();
    let mut q = app.world.query::<(
        &MeshRenderer,
        &GlobalTransform,
        Option<&PlantPart>,
        Option<&ShadowStandIn>,
        Option<&ShadowTexelRange>,
    )>();
    let meshes = app.world.resource::<Assets<Mesh>>();
    for (r, gt, part, standin, maps) in q.iter(&app.world) {
        if part.is_none() && standin.is_none() {
            continue;
        }
        let MeshRef::Handle(h) = &r.mesh else {
            continue;
        };
        let Some(mesh) = meshes.get(*h) else { continue };
        let tris = mesh.triangle_count() as u64;
        let lvl = level.get(&h.index()).copied().unwrap_or("8 other");
        let to = gt.translation() - eye;
        let d = to.length();
        if r.visible && standin.is_none() {
            let angle = (to.normalize_or_zero().dot(forward.as_dvec3()))
                .clamp(-1.0, 1.0)
                .acos();
            let slack = (12.0 / d.max(1.0)).atan();
            if angle as f32 <= half_fov + slack as f32 {
                let e = view.entry(lvl).or_default();
                e.0 += 1;
                e.1 += tris;
            }
        }
        if r.cast_shadows && d < 350.0 {
            // Into which maps: the fine ones only, the coarse ones only, or all.
            let maps = maps.copied().unwrap_or_default();
            let to = if maps.max.is_finite() {
                &mut fine
            } else if maps.min > 0.0 {
                &mut coarse
            } else {
                &mut cast
            };
            let e = to.entry(lvl).or_default();
            e.0 += 1;
            e.1 += tris;
        }
    }
    {
        let mut q = app.world.query_filtered::<&MeshRenderer, With<PlantPart>>();
        let (all, hidden) = q
            .iter(&app.world)
            .fold((0u32, 0u32), |(a, h), r| (a + 1, h + u32::from(!r.visible)));
        let plants = app.world.query::<&Plant>().iter(&app.world).count();
        println!("census {name}: {plants} plants, {all} plant parts, {hidden} of them hidden");
        // By form: plants, and the part entities under them.
        let library = app
            .world
            .resource::<engine_foliage::species::PlantLibrary>()
            .clone();
        let mut forms: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        let mut q = app.world.query::<(&Plant, Option<&Children>)>();
        for (p, children) in q.iter(&app.world) {
            let e = forms
                .entry(format!("{:?}", library.get(p.species).form))
                .or_default();
            e.0 += 1;
            e.1 += children.map_or(0, |c| {
                c.iter()
                    .filter(|&c| app.world.get::<PlantPart>(c).is_some())
                    .count() as u32
            });
        }
        let total = app.world.entities().len();
        println!("census {name}: {total} entities; plants (parts) by form {forms:?}");
    }
    let show = |m: &BTreeMap<&str, (u32, u64)>| {
        let total: u64 = m.values().map(|v| v.1).sum();
        let parts: Vec<String> = m
            .iter()
            .map(|(k, v)| format!("{} {}: {:.2} M", v.0, &k[2..], v.1 as f64 / 1e6))
            .collect();
        format!("{:.2} M ({})", total as f64 / 1e6, parts.join(", "))
    };
    println!("census {name}: in view {}", show(&view));
    println!("census {name}: casting into every map {}", show(&cast));
    println!("census {name}: casting into fine maps only {}", show(&fine));
    println!(
        "census {name}: casting into coarse maps only {}",
        show(&coarse)
    );
    let d = app.world.resource::<Diagnostics>();
    let ms = |k: &str| d.get(k).unwrap_or(f64::NAN);
    // The renderer's CPU side: what the plant parts and their shadow stand-ins cost to prepare.
    let cpu: Vec<String> = d
        .metrics
        .iter()
        .filter(|(k, v)| k.starts_with("render.cpu.") && **v >= 0.3)
        .map(|(k, v)| format!("{} {v:.1}", &k["render.cpu.".len()..]))
        .collect();
    println!(
        "census {name}: {} entities; mesh slots {}, cascade instances {}, cascade triangles {:.2} M; CPU ms {:.1}: {}",
        d.entities,
        ms("render.meshes.slots"),
        ms("render.meshes.cascade_instances"),
        ms("render.meshes.cascade_triangles") / 1e6,
        ms("render.cpu_ms"),
        cpu.join(", ")
    );
    println!(
        "census {name}: drawn {:.2} M triangles in {} draws; GPU {:.0} ms: shadows {:.0}, prepass {:.0}, scene: meshes {:.0}, scene: grass {:.0}",
        ms("render.triangles") / 1e6,
        ms("render.draw_calls"),
        ms("render.gpu_ms"),
        ms("render.gpu.shadows"),
        ms("render.gpu.prepass"),
        ms("render.gpu.scene: meshes"),
        ms("render.gpu.scene: grass"),
    );
}

/// How far the tree being felled leans from upright, radians (`None` once it has settled).
fn fall_angle(app: &mut App) -> Option<f32> {
    let mut q = app.world.query::<&FallingTree>();
    q.iter(&app.world).next().map(|f| f.angle)
}

fn set_step(app: &mut App, step_secs: f64) {
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs });
}

/// Run frames without drawing them (software rendering is the slow part here).
fn simulate(app: &mut App, frames: usize) {
    let renderer = app.world.remove_resource::<engine_render::Renderer>();
    app.world
        .insert_resource(RenderDisabled("simulating only".into()));
    app.update_n(frames);
    app.world.remove_resource::<RenderDisabled>();
    if let Some(r) = renderer {
        app.world.insert_resource(r);
    }
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "screenshots/foliage".into());
    std::fs::create_dir_all(&out).expect("output directory");
    let only: Vec<String> = std::env::args().skip(2).collect();
    let wanted = |file: &str| only.is_empty() || only.iter().any(|o| file.contains(o.as_str()));

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);

    if let Ok(view) = std::env::var("FOLIAGE_DEBUG_VIEW") {
        app.world.resource_mut::<RenderSettings>().debug_view = match view.as_str() {
            "sky" => DebugView::SkyVisibility,
            "reflections" => DebugView::Reflections,
            "gi" => DebugView::IndirectLight,
            "normals" => DebugView::Normals,
            "cascades" => DebugView::Cascades,
            other => panic!("unknown debug view {other}"),
        };
    }
    if std::env::var("FOLIAGE_QUALITY").is_ok_and(|q| q == "ultra") {
        app.world.insert_resource(engine_world::Scalability::preset(
            engine_world::QualityPreset::Ultra,
        ));
        let mut settings = app.world.resource_mut::<RenderSettings>();
        settings.shadow_distance = settings.shadow_distance.max(400.0);
    }
    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.18, 0.04];
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default(),
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 55.0,
                near: 0.1,
            },
            ..Default::default()
        },
        Transform::default(),
    ));
    // `FOLIAGE_EV=<ev100>`: a fixed exposure, so two builds' shots differ only where the
    // pictures do (auto exposure carries any change in brightness to every pixel).
    let fixed_ev: Option<f32> = std::env::var("FOLIAGE_EV")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut cams = app.world.query::<&mut Camera>();
    for mut cam in cams.iter_mut(&mut app.world) {
        if let Some(ev100) = fixed_ev {
            cam.exposure = Exposure::Manual { ev100 };
        } else if let Exposure::Auto { speed, .. } = &mut cam.exposure {
            *speed = 40.0;
        }
    }
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, FOREST);
        demo::spawn_cave_demo(&mut commands, CAVE);
        demo::spawn_cave_chamber(&mut commands, CAVE);
    }
    app.world.flush();
    // The demo fells a tree every 25 s; this tool fells its own.
    let mut felling = app.world.query::<&mut demo::DemoFelling>();
    for mut d in felling.iter_mut(&mut app.world) {
        d.max_trees = 0;
    }
    // `FOLIAGE_FROZEN=1`: no live ecology, so every run shows the same forest (how many frames
    // the loading below takes depends on the machine, and the forest steps meanwhile): for
    // before/after comparisons of how it is drawn.
    // `FOLIAGE_GPU_FERNS=0`: ferns as meshes, one entity each (as before F3).
    if std::env::var("FOLIAGE_GPU_FERNS").is_ok_and(|v| v == "0") {
        app.world
            .resource_mut::<engine_foliage::EcologySettings>()
            .gpu_ferns = false;
    }
    // `FOLIAGE_HERB_KINDS=0`: one kind of forest floor herb (as before round 32).
    if std::env::var("FOLIAGE_HERB_KINDS").is_ok_and(|v| v == "0") {
        app.world
            .resource_mut::<engine_foliage::EcologySettings>()
            .herb_kinds = false;
    }
    // `FOLIAGE_BONE_WIND=0`: trees sway as one (the legacy sway), not on their wind bones.
    if std::env::var("FOLIAGE_BONE_WIND").is_ok_and(|v| v == "0") {
        app.world
            .resource_mut::<engine_foliage::EcologySettings>()
            .bone_wind = false;
    }
    if std::env::var("FOLIAGE_FROZEN").is_ok_and(|v| v == "1") {
        app.world
            .resource_mut::<engine_foliage::EcologySettings>()
            .paused = true;
    }
    let started = std::time::Instant::now();
    app.update();
    // Regions grow their history and plant on workers over frames: wait until the plant count
    // holds still before picking a tree to fell.
    let (mut last, mut still) = (0usize, 0u32);
    for _ in 0..2400 {
        simulate(&mut app, 1);
        std::thread::sleep(std::time::Duration::from_millis(25));
        let n = app.world.query::<&Plant>().iter(&app.world).count();
        still = if n == last { still + 1 } else { 0 };
        last = n;
        if n > 0
            && still > 40
            && app
                .world
                .resource::<engine_foliage::FoliageLoading>()
                .is_ready()
        {
            break;
        }
    }
    println!(
        "grew the forest and cave in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    let stats =
        engine_core::registry::call_api(&mut app.world, "foliage.stats", serde_json::json!({}));
    println!("{}", serde_json::to_string_pretty(&stats.unwrap()).unwrap());

    // `FOLIAGE_GRASS_STATS=1`: what each grass and floor field holds.
    if std::env::var("FOLIAGE_GRASS_STATS").is_ok_and(|v| v == "1") {
        let mut q = app.world.query::<(
            &engine_render::GrassField,
            Option<&engine_foliage::grass::FloorLayer>,
            &Transform,
        )>();
        for (n, (f, layer, at)) in q.iter(&app.world).enumerate() {
            // Each field's density as a grey map (north up), named by its layer and center.
            let [w, h] = f.resolution;
            let mut pgm = format!("P5 {w} {h} 255\n").into_bytes();
            pgm.extend(f.density.iter().map(|d| (d.clamp(0.0, 1.0) * 255.0) as u8));
            let name = format!(
                "{out}/field_{n}_{layer:?}_{:.0}_{:.0}.pgm",
                at.translation.x, at.translation.z
            );
            let _ = std::fs::write(name, pgm);
            let on = f.density.iter().filter(|d| **d > 0.0).count();
            let mut kinds = [0usize; 16];
            for k in &f.kinds {
                kinds[(*k as usize).min(15)] += 1;
            }
            let nan = f.heights.iter().filter(|h| !h.is_finite()).count();
            let (lo, hi) = f
                .heights
                .iter()
                .filter(|h| h.is_finite())
                .fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
            let dn = f.density.iter().filter(|d| !d.is_finite()).count();
            println!(
                "  heights {lo}..{hi}, {nan} not finite; density {dn} not finite; style {:?}",
                f.style
            );
            println!(
                "grass field {layer:?}: {:?} m at {:?}, {} of {} texels on, heights {}, {} styles, kinds {kinds:?}",
                f.size,
                f.resolution,
                on,
                f.density.len(),
                f.heights.len(),
                f.styles.len()
            );
        }
    }
    // `FOLIAGE_WIND=<m/s>`: a steady wind at 10 m (gust fronts on above 10 m/s), settled
    // before the shots: trees on their wind bones lean, sway and show their leaves' undersides.
    if let Some(w) = std::env::var("FOLIAGE_WIND")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        let wind = Vec3::new(w, 0.0, 0.35 * w);
        app.world.resource_mut::<WeatherState>().wind = wind;
        if let Some(mut field) = app.world.get_resource_mut::<engine_world::WindField>() {
            let fronts = w > 10.0;
            field.waves.broad.strength = if fronts { 0.55 } else { 0.0 };
            field.waves.fine.strength = if fronts { 0.3 } else { 0.0 };
        }
        simulate(&mut app, 240);
    }
    // `FOLIAGE_SNOW=<m>`: the ground under that much fresh snow, everywhere, while it snows.
    if let Some(s) = std::env::var("FOLIAGE_SNOW")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        let cell = engine_world::SurfaceCell {
            wetness: 0.3,
            snow_m: s,
            ..Default::default()
        };
        app.world.insert_resource(engine_world::SurfaceLayers {
            grid: engine_world::Grid2::centered(DVec2::ZERO, 512.0, 1.0, cell),
        });
        {
            let mut w = app.world.resource_mut::<WeatherState>();
            w.precipitation_kind = engine_world::PrecipitationKind::Snow;
            w.precipitation_mm_h = 1.0;
            w.temperature_c = -3.0;
            w.cloud_cover = 0.5;
        }
        simulate(&mut app, 30);
    }
    // `FOLIAGE_DAY=<day of year>`: every shot in that season (winter: 20 or 345).
    if let Some(day) = std::env::var("FOLIAGE_DAY")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        app.world.resource_mut::<FoliageSeason>().day_of_year = day;
        simulate(&mut app, 30);
        let mut q = app.world.query::<(
            &engine_render::GrassField,
            Option<&engine_foliage::grass::FloorLayer>,
        )>();
        for (f, layer) in q.iter(&app.world) {
            println!(
                "day {day}: {layer:?} lying {} height {}",
                f.style.lying, f.style.height
            );
        }
    }
    let forest = demo::forest_viewpoints(FOREST);
    let cave = demo::cave_viewpoints(CAVE);
    let take = |app: &mut App, file: &str, v: &demo::Viewpoint| {
        if wanted(file) {
            shot(app, &out, file, v.eye, v.target, v.hour)
        }
    };
    let by_name = |name: &str| *forest.iter().find(|v| v.name == name).expect("viewpoint");
    take(&mut app, "01_forest", &by_name("forest_overview"));
    take(&mut app, "02_trail", &by_name("forest_trail"));
    if wanted("02_trail_inner") {
        // Along the trail where it runs inside the old forest (between its 4th and 6th points),
        // at walking height: the floor beside the tread and the leaves blown onto it.
        let ground = |x: f64, z: f64| {
            app.world
                .get_resource::<engine_world::Ground>()
                .and_then(|g| g.height(DVec3::new(x, 0.0, z)))
                .unwrap_or(0.0)
        };
        let eye = DVec3::new(30.4, ground(30.4, 24.0) + 1.7, 24.0);
        let at = DVec3::new(12.0, ground(12.0, 17.5) + 0.4, 17.5);
        shot(&mut app, &out, "02_trail_inner", eye, at, 10.5);
    }
    take(&mut app, "03_clearing", &by_name("forest_clearing"));
    take(&mut app, "03_old_oak", &by_name("forest_old_oak"));
    if wanted("03_ferns") {
        // Into the thickest ferns within 40 m of the trail's viewpoint, from 5 m off.
        let near = by_name("forest_trail").eye;
        let library = app
            .world
            .resource::<engine_foliage::species::PlantLibrary>()
            .clone();
        let mut cells: std::collections::HashMap<(i64, i64), (u32, DVec3)> = Default::default();
        let mut q = app.world.query::<(&Plant, &GlobalTransform)>();
        for (p, t) in q.iter(&app.world) {
            let at = t.translation();
            if library.get(p.species).form != engine_foliage::species::PlantForm::Fern
                || DVec2::new(at.x - near.x, at.z - near.z).length() > 40.0
            {
                continue;
            }
            let c = cells
                .entry(((at.x / 3.0).floor() as i64, (at.z / 3.0).floor() as i64))
                .or_insert((0, DVec3::ZERO));
            c.0 += 1;
            c.1 += at;
        }
        // Ties go to the cell with the lowest key, so every run frames the same ferns.
        if let Some((_, &(n, sum))) = cells
            .iter()
            .max_by_key(|(k, c)| (c.0, std::cmp::Reverse(**k)))
        {
            let at = sum / n as f64;
            let back = DVec3::new(near.x - at.x, 0.0, near.z - at.z).normalize_or(DVec3::X);
            let eye = at + back * 5.0 + DVec3::Y * 1.6;
            shot(&mut app, &out, "03_ferns", eye, at + DVec3::Y * 0.3, 10.5);
        }
    }
    if wanted("04_floor") {
        // The forest floor where its herbs grow thickest within 40 m of the trail's viewpoint
        // (5 m cells), from 4 m off, looking down across it as one walks (ref3_forest_floor).
        let near = by_name("forest_trail").eye;
        let mut best: Option<(f32, DVec3)> = None;
        let mut q = app.world.query::<(
            &engine_render::GrassField,
            &engine_foliage::grass::FloorLayer,
            &GlobalTransform,
        )>();
        for (f, layer, t) in q.iter(&app.world) {
            if *layer != engine_foliage::grass::FloorLayer::Herbs || f.density.is_empty() {
                continue;
            }
            let [rx, rz] = f.resolution;
            let step = [f.size[0] / (rx - 1) as f32, f.size[1] / (rz - 1) as f32];
            let corner =
                t.translation() - DVec3::new(f.size[0] as f64, 0.0, f.size[1] as f64) * 0.5;
            let mut cells: std::collections::BTreeMap<(i64, i64), (f32, u32)> = Default::default();
            for j in 0..rz {
                for i in 0..rx {
                    let p = corner
                        + DVec3::new(
                            (i as f32 * step[0]) as f64,
                            0.0,
                            (j as f32 * step[1]) as f64,
                        );
                    if DVec2::new(p.x - near.x, p.z - near.z).length() > 40.0 {
                        continue;
                    }
                    let c = cells
                        .entry(((p.x / 5.0).floor() as i64, (p.z / 5.0).floor() as i64))
                        .or_default();
                    c.0 += f.density[(j * rx + i) as usize];
                    c.1 += 1;
                }
            }
            for ((cx, cz), (sum, n)) in cells {
                let mean = sum / n.max(1) as f32;
                if best.is_none_or(|(b, _)| mean > b) {
                    let at = DVec3::new(
                        cx as f64 * 5.0 + 2.5,
                        t.translation().y,
                        cz as f64 * 5.0 + 2.5,
                    );
                    best = Some((mean, at));
                }
            }
        }
        if let Some((_, at)) = best {
            let ground = app
                .world
                .get_resource::<engine_world::Ground>()
                .and_then(|g| g.height(at))
                .unwrap_or(at.y);
            let at = DVec3::new(at.x, ground, at.z);
            let back = DVec3::new(near.x - at.x, 0.0, near.z - at.z).normalize_or(DVec3::X);
            let eye = at + back * 4.0 + DVec3::Y * 1.5;
            shot(&mut app, &out, "04_floor", eye, at + DVec3::Y * 0.1, 10.5);
        }
    }
    if wanted("11_switch") {
        // Same spot, same frame count: only the trees' level changes (switched at once).
        let v = by_name("forest_clearing");
        app.world
            .resource_mut::<engine_foliage::lod_fade::LodFades>()
            .enabled = false;
        let (lod, mid) = {
            let s = app.world.resource::<engine_foliage::EcologySettings>();
            (s.tree_lod_distance_m, s.tree_mid_distance_m)
        };
        for (file, far_from) in [("11_switch_mid", 1.0e4), ("11_switch_far", mid)] {
            app.world
                .resource_mut::<engine_foliage::EcologySettings>()
                .tree_lod_distance_m = far_from;
            shot(&mut app, &out, file, v.eye, v.target, v.hour);
        }
        app.world
            .resource_mut::<engine_foliage::EcologySettings>()
            .tree_lod_distance_m = lod;
        app.world
            .resource_mut::<engine_foliage::lod_fade::LodFades>()
            .enabled = true;
    }

    // Fell a big tree at the edge of the clear-cut into the open, and photograph it coming
    // down from beside its path.
    let library = app.world.resource::<PlantLibrary>().clone();
    let view = by_name("forest_clearing");
    let fwd = (view.target - view.eye).with_y(0.0).normalize();
    let clearing = FOREST + DVec3::new(demo::CLEARING[0] as f64, 0.0, demo::CLEARING[1] as f64);
    let mut q = app.world.query::<(Entity, &Plant, &Transform)>();
    // Trees between the camera and the falling one would hide it (bushes and saplings only
    // hide its foot); anything at the camera would fill the frame.
    let standing: Vec<(DVec3, f32, f64)> = q
        .iter(&app.world)
        .filter(|(_, p, _)| p.stage.is_alive())
        .map(|(_, p, t)| {
            let sp = library.get(p.species);
            (t.translation, p.height(sp), p.crown_radius(sp) as f64 + 0.3)
        })
        .collect();
    let in_view = |eye: DVec3, base: DVec3| {
        let to = (base - eye).with_y(0.0);
        let len = to.length();
        let dir = to / len;
        !standing.iter().any(|&(p, h, r)| {
            let rel = (p - eye).with_y(0.0);
            let along = rel.dot(dir);
            h > 6.0 && along > 1.0 && along < len - 4.0 && (rel - dir * along).length() < r
        })
    };
    let crowded = |eye: DVec3| {
        standing
            .iter()
            .any(|&(p, h, r)| h > 0.8 && (p - eye).with_y(0.0).length() < r + 2.5)
    };
    let candidates: Vec<(Entity, DVec3, f32)> = q
        .iter(&app.world)
        .filter(|(_, p, t)| {
            let edge = (t.translation - clearing).with_y(0.0).length();
            p.stage == PlantStage::Alive
                && library.get(p.species).is_tree()
                && p.size > 0.6
                && (demo::CLEARING[2] as f64 - 3.0..demo::CLEARING[2] as f64 + 3.0).contains(&edge)
        })
        .map(|(e, p, t)| (e, t.translation, p.size))
        .collect();
    // Fell it straight into the open and watch from beside its path, inside the clearing:
    // the trunk shows its whole length, and its crown rises above the far tree line.
    let camera_for = |base: DVec3, side: f64| {
        let outward = (base - clearing).with_y(0.0).normalize();
        let across = DVec3::Y.cross(outward) * side;
        let eye = (base - outward * 9.0 + across * 12.0).with_y(view.eye.y - 0.5);
        (outward, eye)
    };
    let felled = candidates
        .into_iter()
        .flat_map(|c| [(c, 1.0), (c, -1.0)])
        .max_by(|a, b| {
            // Clear line of sight, nothing at the camera, a big tree, the morning sun as in
            // the clearing view.
            let score = |&((_, base, size), side): &((Entity, DVec3, f32), f64)| {
                let (outward, eye) = camera_for(base, side);
                let path = base - outward * 7.0;
                let clear = if in_view(eye, path) { 10.0 } else { 0.0 };
                let room = if crowded(eye) { 0.0 } else { 5.0 };
                let looking = (path - eye).with_y(0.0).normalize();
                clear + room + size + 2.0 * looking.dot(fwd) as f32
            };
            score(a).total_cmp(&score(b))
        });
    if let Some(((tree, base, _), side)) =
        felled.filter(|_| wanted("04_falling") || wanted("05_fallen"))
    {
        let (outward, eye) = camera_for(base, side);
        println!(
            "felling a tree {:.0} m from the camera ({}, {})",
            (base - eye).with_y(0.0).length(),
            if in_view(eye, base - outward * 7.0) {
                "in plain view"
            } else {
                "partly hidden"
            },
            if crowded(eye) {
                "a plant at the camera"
            } else {
                "room at the camera"
            }
        );
        let dir = -outward;
        let look = base + dir * 8.0 + DVec3::Y * 5.5;
        aim(&mut app.world, eye, look);
        set_hour(&mut app.world, view.hour);
        app.update_n(6);
        fell_tree(&mut app.world, tree, dir.as_vec3());
        // It tips slowly, then comes down fast: photograph it while its crown still stands
        // clear of the trees around it, with time stopped so the frame is sharp.
        let mut frames = 0;
        while fall_angle(&mut app).is_some_and(|a| a < 0.7) && frames < 300 {
            simulate(&mut app, 1);
            frames += 1;
        }
        println!(
            "after {:.1} s it leans {:.0} degrees",
            frames as f64 * STEP,
            fall_angle(&mut app).unwrap_or(0.0).to_degrees()
        );
        if wanted("04_falling") {
            set_step(&mut app, 0.0);
            app.update_n(6);
            app.world
                .resource_mut::<Screenshots>()
                .request(format!("{out}/04_falling.png"));
            app.update();
            println!("saved {out}/04_falling.png");
            set_step(&mut app, STEP);
        }
        // Down and settled: walk up to it, looking across the log from the clearing side.
        let length = {
            let mut q = app.world.query::<&FallingTree>();
            q.iter(&app.world).next().map_or(15.0, |f| f.length as f64)
        };
        while fall_angle(&mut app).is_some() && frames < 600 {
            simulate(&mut app, 1);
            frames += 1;
        }
        simulate(&mut app, 4);
        let mut logs = app.world.query::<(&FelledLog, &Transform)>();
        let lying = logs
            .iter(&app.world)
            .map(|(_, t)| (t.translation, (t.rotation * Vec3::Y).as_dvec3()))
            .min_by(|a, b| a.0.distance(base).total_cmp(&b.0.distance(base)));
        let (eye, target) = match lying {
            Some((butt, axis)) => {
                // From the same side, a little higher: grass and saplings would hide it from
                // head height.
                let mid = butt + axis * length * 0.55;
                let across = DVec3::Y.cross(outward) * side;
                let eye = |s: f64| (mid + across * (s * 12.0)).with_y(view.eye.y + 2.5);
                let s = if crowded(eye(1.0)) && !crowded(eye(-1.0)) {
                    -1.0
                } else {
                    1.0
                };
                (eye(s), mid + DVec3::Y * 0.4)
            }
            None => (eye, look - DVec3::Y * 5.0),
        };
        shot(&mut app, &out, "05_fallen", eye, target, view.hour);
        if let Some((butt, axis)) = lying {
            // Down at the crown from knee height, to see it pressed on the ground.
            let crown = butt + axis * length * 0.8;
            let across = DVec3::Y.cross(outward) * side;
            let eye = (crown + across * 7.0 - axis * 2.0).with_y(crown.y + 0.2);
            let eye = eye.with_y(eye.y.max(0.6));
            shot(
                &mut app,
                &out,
                "05_fallen_low",
                eye,
                crown.with_y(0.4),
                view.hour,
            );
        }
    }
    take(&mut app, "06_spring", &by_name("forest_spring"));
    take(&mut app, "07_burn_scar", &by_name("forest_burn_scar"));
    take(&mut app, "08_dry_ridge", &by_name("forest_dry_ridge"));

    // The same clearing through the year: fresh leaves, autumn color, leaf fall, bare winter.
    for (file, day, view) in [
        ("12_spring", 126.0, "forest_clearing"),
        ("13_autumn", 282.0, "forest_clearing"),
        ("14_autumn_trail", 282.0, "forest_trail"),
        ("15_late_autumn", 300.0, "forest_clearing"),
        ("16_winter", 345.0, "forest_clearing"),
    ] {
        if wanted(file) {
            app.world.resource_mut::<FoliageSeason>().day_of_year = day;
            take(&mut app, file, &by_name(view));
        }
    }
    app.world.resource_mut::<FoliageSeason>().day_of_year = season::MIDSUMMER;

    // Ten more years in the clearing.
    if wanted("09_clearing_10_years_later") {
        ten_years_later(&mut app, &out, &by_name("forest_clearing"));
    }

    take(&mut app, "10_cave", &cave[0]);
    take(&mut app, "11_cave_pool", &cave[1]);
    let stats =
        engine_core::registry::call_api(&mut app.world, "foliage.stats", serde_json::json!({}));
    println!("{}", serde_json::to_string_pretty(&stats.unwrap()).unwrap());
}

fn ten_years_later(app: &mut App, out: &str, v: &demo::Viewpoint) {
    let years = 10.0;
    let frames = 24;
    let region_scale = 30.0;
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = years / (frames as f32 * STEP as f32) / region_scale;
    simulate(app, frames);
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = 0.0;
    shot(
        app,
        out,
        "09_clearing_10_years_later",
        v.eye,
        v.target,
        v.hour,
    );
}
