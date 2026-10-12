//! Renders UI offscreen and checks the pixels. Needs a GPU or a software Vulkan driver
//! (lavapipe); passes with a note when neither is available (`RenderDisabled`).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_ui::prelude::*;

const W: u32 = 960;
const H: u32 = 540;

fn app() -> App {
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
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f7a3a).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 1.7, 6.0).looking_at(DVec3::new(0.0, 1.2, 0.0), Vec3::Y),
    ));
    app
}

fn render(app: &mut App, name: &str) -> Option<image::RgbaImage> {
    app.update_n(30);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

#[test]
fn draws_a_panel_with_text_and_icons() {
    let mut app = app();
    let el = El::new()
        .class("panel")
        .id("card")
        .with_style(|s| s.at(40.0, 40.0).width(420.0))
        .child(El::text("Merchant of Hollow Reach").class("title"))
        .child(El::text("Fine [color=#e8b65a]gold[/color] and [b]rare[/b] goods.").class("body"))
        .child(
            El::new()
                .class("row")
                .child(El::icon("sword").with_style(|s| s.size(56.0, 56.0)))
                .child(El::icon("potion_health").with_style(|s| s.size(56.0, 56.0)))
                .child(El::icon("ui_close").with_style(|s| s.size(28.0, 28.0))),
        )
        .child(
            El::button("Trade", "shop.open")
                .class("primary")
                .id("trade"),
        );
    engine_ui::el::spawn(&mut app.world, None, &el).unwrap();
    let Some(img) = render(&mut app, "ui_panel.png") else {
        return;
    };
    let tree =
        engine_core::registry::call_api(&mut app.world, "ui.tree", serde_json::json!({})).unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&tree).unwrap());
    let diag = app.world.resource::<Diagnostics>();
    assert!(diag.get("ui.glyphs").unwrap_or(0.0) > 20.0, "glyphs drawn");
    let mut rect = |id: &str| {
        let e = engine_ui::input::find_id(&mut app.world, id).expect(id);
        app.world.get::<ComputedNode>(e).unwrap().rect
    };
    let (card, trade) = (rect("card"), rect("trade"));
    let lum = |x: f32, y: f32| {
        let p = img.get_pixel(x as u32, y as u32).0;
        p[0] as u32 + p[1] as u32 + p[2] as u32
    };
    // The panel (an empty spot right of the icons) is darker than the sky beside it, and
    // the primary button is brighter than the panel.
    let inside = lum(card.x + card.w - 12.0, card.y + card.h * 0.5);
    let outside = lum(card.x + card.w + 200.0, card.y + card.h * 0.5);
    let button = lum(trade.x + 8.0, trade.y + trade.h * 0.5);
    eprintln!("inside {inside} outside {outside} button {button}");
    assert!(inside < outside, "panel darker than the sky");
    assert!(button > inside + 150, "button brighter than the panel");
}
