//! Draws planet maps (PLANET_FLAT) offscreen and checks them against the CPU terrain in
//! `planet_noise`: where the CPU says sea the map is blue, where it says land it is not, both
//! for the whole world and for a 16x zoom on a coast (where the shader adds detail octaves).
//!
//! Needs a GPU or a software Vulkan driver (lavapipe); passes with a note when neither is
//! available (`RenderDisabled`), unless `ENGINE_REQUIRE_GPU=1`.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_ui::draw::{DrawCtx, Painters, PlanetLook};
use engine_ui::map::PlanetKind;
use engine_ui::planet_noise::{self, direction};
use engine_ui::prelude::*;

const W: u32 = 960;
const H: u32 = 720;

/// What the painter draws: the look, and the center of the zoomed map (degrees).
#[derive(Resource, Clone, Copy)]
struct TestMap {
    look: PlanetLook,
    zoom_center: (f64, f64),
}

/// The two maps the painter drew: (rect, lon0, lat0, lon1, lat1) in px and degrees.
#[derive(Resource, Default, Clone)]
struct Drawn(Vec<(Rect, [f64; 4])>);

const ZOOM_SPAN: f64 = 360.0 / 16.0;

fn paint_maps(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let map = *world.resource::<TestMap>();
    let r = ctx.rect;
    let top = Rect::new(r.x, r.y, r.w, r.h * 2.0 / 3.0);
    let bottom = Rect::new(r.x, r.y + top.h, r.w, r.h - top.h);
    let (clon, clat) = map.zoom_center;
    let half_lat = ZOOM_SPAN * 0.5 * bottom.h as f64 / bottom.w as f64;
    let maps = [
        (top, [-180.0, 90.0, 180.0, -90.0]),
        (
            bottom,
            [
                clon - ZOOM_SPAN * 0.5,
                clat + half_lat,
                clon + ZOOM_SPAN * 0.5,
                clat - half_lat,
            ],
        ),
    ];
    for (rect, [lon0, lat0, lon1, lat1]) in maps {
        ctx.planet_flat(
            rect,
            map.look,
            [lon0.to_radians() as f32, lon1.to_radians() as f32],
            [lat0.to_radians() as f32, lat1.to_radians() as f32],
            0.0,
            0.0,
            0.0,
        );
    }
    world.insert_resource(Drawn(maps.to_vec()));
}

fn app(map: TestMap) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((RenderPlugin, engine_input::InputPlugin, UiPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.4, -0.5, -0.6), Vec3::Y),
    ));
    w.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 1.7, 6.0).looking_at(DVec3::new(0.0, 1.2, 0.0), Vec3::Y),
    ));
    w.insert_resource(map);
    w.init_resource::<Drawn>();
    w.resource_mut::<Painters>().add("terrain_test", paint_maps);
    app
}

/// Whether an sRGB pixel reads as water: clearly bluer than red, at least as blue as green.
fn looks_blue(p: [u8; 4]) -> bool {
    let [r, g, b, _] = p.map(i32::from);
    b > r + 20 && b >= g
}

#[test]
fn map_water_matches_the_cpu_terrain() {
    let look = PlanetKind::Terran.look(3.0);
    // Zoom in on a coast: walk from open sea to the nearest land.
    let zoom_center = planet_noise::find_land(40.0, 10.0, &look);
    let mut app = app(TestMap { look, zoom_center });
    engine_ui::el::spawn(
        &mut app.world,
        None,
        &El::new()
            .paint("terrain_test")
            .with_style(|s| s.fill_parent()),
    )
    .unwrap();
    app.update_n(10);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        assert!(
            std::env::var("ENGINE_REQUIRE_GPU").as_deref() != Ok("1"),
            "ENGINE_REQUIRE_GPU=1 but rendering is disabled: {}",
            d.0
        );
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("planet_terrain.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    eprintln!("wrote {}", path.display());
    let img = image::open(&path).expect("png").to_rgba8();
    let drawn = app.world.resource::<Drawn>().0.clone();
    assert_eq!(drawn.len(), 2, "the painter ran");

    let sea = look.shape[1];
    let mut checked = [0usize; 2];
    let mut failures = Vec::new();
    for (m, (rect, [lon0, lat0, lon1, lat1])) in drawn.iter().enumerate() {
        // Candidate pixels on a grid; keep those clearly away from the coast (the shader's
        // detail octaves and antialiasing may move it a little) and the polar ice.
        let mut water = Vec::new();
        let mut land = Vec::new();
        let (nx, ny) = (41, 23);
        for j in 0..ny {
            for i in 0..nx {
                let x = (rect.x + rect.w * (i as f32 + 0.5) / nx as f32).floor();
                let y = (rect.y + rect.h * (j as f32 + 0.5) / ny as f32).floor();
                let tx = ((x + 0.5 - rect.x) / rect.w) as f64;
                let ty = ((y + 0.5 - rect.y) / rect.h) as f64;
                let lon = lon0 + (lon1 - lon0) * tx;
                let lat = lat0 + (lat1 - lat0) * ty;
                if lat.abs() > 55.0 {
                    continue;
                }
                let h = planet_noise::height(direction(lon, lat), &look);
                if (h - sea).abs() < 0.02 {
                    continue;
                }
                let sample = (x as u32, y as u32, lon, lat, h);
                if h < sea {
                    water.push(sample);
                } else {
                    land.push(sample);
                }
            }
        }
        assert!(
            water.len() >= 10 && land.len() >= 10,
            "map {m}: {} water and {} land candidates",
            water.len(),
            land.len()
        );
        // About 25 of each, spread over the map.
        for (list, want_water) in [(&water, true), (&land, false)] {
            let step = (list.len() / 25).max(1);
            for &(x, y, lon, lat, h) in list.iter().step_by(step).take(25) {
                let px = img.get_pixel(x, y).0;
                checked[m] += 1;
                if looks_blue(px) != want_water {
                    failures.push(format!(
                        "map {m} ({x}, {y}) lon {lon:.2} lat {lat:.2}: height {h:.3} vs sea {sea}, cpu says {}, pixel {px:?}",
                        if want_water { "water" } else { "land" }
                    ));
                }
            }
        }
    }
    eprintln!("checked {checked:?} points, {} failed", failures.len());
    assert!(checked[0] >= 40 && checked[1] >= 30, "checked {checked:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
