//! The ground's weather per pixel (`surface_weather`): `SurfaceLayers` cells show where they
//! are, so ground under a roof stays dry while the roof itself gets the snow, and fire scorch
//! chars the ground it burned. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::math::{DVec2, IVec2};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::{Grid2, SurfaceCell, SurfaceLayers};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A 10 m carport roof 3 m up over the origin.
const ROOF_CENTER: DVec3 = DVec3::new(0.0, 3.2, 0.0);
const ROOF_SIZE: [f32; 3] = [10.0, 0.4, 10.0];

const UNDER_ROOF: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const OPEN_GROUND: DVec3 = DVec3::new(0.0, 0.0, 9.0);
const ROOF_TOP: DVec3 = DVec3::new(0.0, 3.4, 0.0);

/// Snowy ground: 20 cm of snow, wet underneath.
const SNOWY: SurfaceCell = SurfaceCell {
    wetness: 0.8,
    puddle_m: 0.0,
    snow_m: 0.2,
    scorch: 0.0,
};

/// A 64 m grid of 1 m cells around the origin, each cell `cell(center)` (world X/Z).
fn layers(cell: impl Fn(DVec2) -> SurfaceCell) -> SurfaceLayers {
    let mut grid = Grid2::centered(DVec2::ZERO, 64.0, 1.0, SurfaceCell::default());
    for y in 0..grid.height as i32 {
        for x in 0..grid.width as i32 {
            let c = IVec2::new(x, y);
            grid.set(c, cell(grid.cell_center(c)));
        }
    }
    SurfaceLayers { grid }
}

/// Snow everywhere but under the roof, where the weather keeps the ground dry.
fn snow_but_under_the_roof() -> SurfaceLayers {
    layers(|p| {
        if p.x.abs() < 5.0 && p.y.abs() < 5.0 {
            SurfaceCell::default()
        } else {
            SNOWY
        }
    })
}

/// Gray ground and the carport under a high sun, seen from `eye`; the default material
/// responds to the weather.
fn scene_app(eye: DVec3, target: DVec3, surface: SurfaceLayers) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.world.insert_resource(surface);
    {
        // Pixel values follow radiance; no light bounced off the snow onto the ground nearby.
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.screen_space_gi = false;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 15.5 },
            ..Camera::perspective(60.0)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.2, -1.0, -0.1), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Transform::from_translation(ROOF_CENTER),
        MeshRenderer::new(
            Shape::Cuboid { size: ROOF_SIZE },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(0.9),
        ),
    ));
    app
}

/// The scene from the side, 14 m away: the ground under the roof and the open ground.
fn side_view(surface: SurfaceLayers) -> App {
    scene_app(
        DVec3::new(0.0, 2.0, 14.0),
        DVec3::new(0.0, 0.5, 0.0),
        surface,
    )
}

/// The scene from above the roof.
fn top_view(surface: SurfaceLayers) -> App {
    scene_app(DVec3::new(0.0, 12.0, 12.0), ROOF_TOP, surface)
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean luma in a 5x5 block around where world point `p` lands on screen.
fn luma_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let (x, y) = (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    );
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
        }
    }
    sum / 25.0
}

#[test]
fn snow_lies_per_pixel_and_not_under_a_roof() {
    let mut snowy = side_view(snow_but_under_the_roof());
    let Some(snow_img) = render(&mut snowy, "surface_snow_side") else {
        return;
    };
    let mut bare = side_view(layers(|_| SurfaceCell::default()));
    let bare_img = render(&mut bare, "surface_bare_side").unwrap();
    let (under_snowy, under_bare) = (
        luma_at(&snowy, &snow_img, UNDER_ROOF),
        luma_at(&bare, &bare_img, UNDER_ROOF),
    );
    let (open_snowy, open_bare) = (
        luma_at(&snowy, &snow_img, OPEN_GROUND),
        luma_at(&bare, &bare_img, OPEN_GROUND),
    );
    eprintln!(
        "under the roof {under_snowy} (bare {under_bare}), open ground {open_snowy} (bare {open_bare})"
    );
    assert!(
        open_snowy > open_bare + 15.0,
        "the open ground should be snowy ({open_snowy} vs {open_bare} bare)"
    );
    // The camera stands on snowy ground; the ground under the roof must still be bare.
    assert!(
        (under_snowy - under_bare).abs() < 3.0,
        "the ground under the roof should stay bare ({under_snowy} vs {under_bare} bare)"
    );
}

#[test]
fn a_roof_over_dry_ground_gets_the_snow_around_it() {
    let mut snowy = top_view(snow_but_under_the_roof());
    let Some(snow_img) = render(&mut snowy, "surface_snow_roof") else {
        return;
    };
    let mut bare = top_view(layers(|_| SurfaceCell::default()));
    let bare_img = render(&mut bare, "surface_bare_roof").unwrap();
    let (roof_snowy, roof_bare) = (
        luma_at(&snowy, &snow_img, ROOF_TOP),
        luma_at(&bare, &bare_img, ROOF_TOP),
    );
    eprintln!("roof top {roof_snowy} (bare {roof_bare})");
    assert!(
        roof_snowy > roof_bare + 15.0,
        "the roof is out in the snow even though the ground under it is dry \
         ({roof_snowy} vs {roof_bare} bare)"
    );
}

#[test]
fn scorch_chars_the_ground_where_fire_burned() {
    // A burnt 4 m patch to the right of the roof; the same spot on the left is untouched.
    let burnt = layers(|p| SurfaceCell {
        scorch: if (p.x - 7.0).abs() < 2.0 && (p.y - 6.0).abs() < 2.0 {
            1.0
        } else {
            0.0
        },
        ..Default::default()
    });
    let mut app = side_view(burnt);
    let Some(img) = render(&mut app, "surface_scorch") else {
        return;
    };
    let scorched = luma_at(&app, &img, DVec3::new(7.0, 0.0, 6.0));
    let untouched = luma_at(&app, &img, DVec3::new(-7.0, 0.0, 6.0));
    eprintln!("scorched ground {scorched}, untouched {untouched}");
    assert!(
        untouched > 60.0,
        "test scene too dark to judge ({untouched})"
    );
    assert!(
        scorched < untouched * 0.4,
        "fire scorch should char the ground ({scorched} vs {untouched})"
    );
}

fn uploads(app: &App) -> f64 {
    app.world.resource::<Diagnostics>().metrics["render.surface_weather_uploads"]
}

#[test]
fn the_layers_are_uploaded_only_when_they_change() {
    let mut app = side_view(layers(|_| SurfaceCell::default()));
    if render(&mut app, "surface_upload_first").is_none() {
        return;
    }
    let first = uploads(&app);
    app.update_n(3);
    assert_eq!(
        uploads(&app),
        first,
        "unchanged layers are not uploaded again"
    );
    // A fire scorches the ground: the next frames show it.
    std::thread::sleep(std::time::Duration::from_millis(150));
    {
        let mut surface = app.world.resource_mut::<SurfaceLayers>();
        for dx in -2..=2 {
            for dz in -2..=2 {
                let p = DVec3::new(-7.0 + dx as f64, 0.0, 6.0 + dz as f64);
                surface.modify(p, |c| c.scorch = 1.0);
            }
        }
    }
    let img = render(&mut app, "surface_upload_changed").unwrap();
    assert_eq!(
        uploads(&app),
        first + 1.0,
        "changed layers are uploaded once"
    );
    let scorched = luma_at(&app, &img, DVec3::new(-7.0, 0.0, 6.0));
    let untouched = luma_at(&app, &img, DVec3::new(7.0, 0.0, 6.0));
    eprintln!("after a change: scorched cell {scorched}, untouched {untouched}");
    assert!(
        scorched < untouched * 0.8,
        "the change should show ({scorched} vs {untouched})"
    );
}
