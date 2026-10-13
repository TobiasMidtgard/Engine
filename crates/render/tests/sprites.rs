//! Offscreen renders of sprites, sprite sheets, tile maps, parallax layers and pixel-perfect
//! cameras (2D), and of lit, cut-out and blended sprites among meshes (2.5D). Needs a GPU or a
//! software Vulkan driver (lavapipe); passes with a note when neither is available.
//!
//! Unlit sprites are rendered with `RenderSettings::with_flat_colors`, so a texel of 255 shows
//! as 255 give or take the tonemapper's ±1–2 levels of dither.

use std::f32::consts::FRAC_PI_2;
use std::path::Path;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, TILE_FLIP_X, capture_now};
use image::RgbaImage;

/// Dither and rounding tolerance per channel.
const TOL: i32 = 4;
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];
/// The backdrop behind 2D scenes (linear 0.2 gray: about 124 on screen).
const BACKDROP: f32 = 0.2;

/// A headless app of `w × h` pixels showing colors as authored, with an orthographic camera
/// `height` meters tall at z = 10 looking down −z, a dark gray backdrop at z = −50 and world
/// time advancing 1/8 s per update.
fn app_2d(w: u32, h: u32, height: f32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut window = app.world.resource_mut::<Window>();
        window.width = w;
        window.height = h;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut settings = app.world.resource_mut::<RenderSettings>();
        *settings = settings.clone().with_flat_colors();
        settings.anti_aliasing = AntiAliasing::None;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.125 });
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 0.0 },
            ..Camera::orthographic(height)
        },
        Transform::from_xyz(0.0, 0.0, 10.0),
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -50.0),
        Sprite::solid([BACKDROP, BACKDROP, BACKDROP, 1.0], [1000.0, 1000.0])
            .with_alpha(SpriteAlpha::Cutout(0.5))
            .with_layer(-100),
    ));
    app
}

/// Render a few frames and read the picture back; `None` (with a note) without a GPU. Panics
/// if the GPU reported a validation error.
fn render(app: &mut App, name: &str, frames: usize) -> Option<RgbaImage> {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {name}: no GPU ({})", d.0);
        return None;
    }
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sprites_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let gpu_errors: Vec<String> = engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect();
    assert!(
        gpu_errors.is_empty(),
        "GPU errors while rendering {name}: {gpu_errors:#?}"
    );
    Some(image::open(&path).expect("png").to_rgba8())
}

fn add_image(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

/// An sRGB image from rows of texels (top row first).
fn texels(rows: &[&[[u8; 4]]]) -> Image {
    let (w, h) = (rows[0].len() as u32, rows.len() as u32);
    let data = rows
        .iter()
        .flat_map(|r| r.iter().flatten().copied())
        .collect();
    Image::new(w, h, data, true)
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x, y).0
}

fn close(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= tol)
}

#[track_caller]
fn assert_color(img: &RgbaImage, (x, y): (u32, u32), want: [u8; 4], what: &str) {
    let got = px(img, x, y);
    assert!(
        close(got, want, TOL),
        "{what}: pixel ({x}, {y}) is {got:?}, expected {want:?}"
    );
}

/// The backdrop's color on screen.
fn backdrop(img: &RgbaImage) -> [u8; 4] {
    px(img, 0, 0)
}

/// Bounding box `[x0, y0, x1, y1)` of pixels that differ from the backdrop by more than the
/// dither.
fn bounds(img: &RgbaImage) -> [u32; 4] {
    let bg = backdrop(img);
    let mut b = [u32::MAX, u32::MAX, 0, 0];
    for (x, y, p) in img.enumerate_pixels() {
        if !close(p.0, bg, 6) {
            b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
        }
    }
    b
}

/// The sprite feature's stats in `render.stats`.
fn sprite_stats(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "sprites"))
        .cloned()
        .expect("the sprites feature")
}

/// Four colored quadrants: red top-left, green top-right, blue bottom-left, white bottom-right.
fn quadrants() -> Image {
    texels(&[&[RED, GREEN], &[BLUE, WHITE]])
}

#[test]
fn sprite_lands_at_its_position_with_its_size_and_texels() {
    // 200 px for 4 m: 50 px per meter, the origin at pixel (100, 100).
    let mut app = app_2d(200, 200, 4.0);
    let image = add_image(&mut app, quadrants());
    app.world.spawn((
        Transform::from_xyz(0.5, -0.3, 0.0),
        Sprite::new(image, [2.0, 1.0]).with_filter(SpriteFilter::Nearest),
    ));
    let Some(img) = render(&mut app, "position", 2) else {
        return;
    };
    // Centered at (125, 115), 100 × 50 px.
    assert_eq!(
        bounds(&img),
        [75, 90, 175, 140],
        "the sprite's screen rectangle"
    );
    assert_color(&img, (90, 100), RED, "top-left texel");
    assert_color(&img, (160, 100), GREEN, "top-right texel");
    assert_color(&img, (90, 125), BLUE, "bottom-left texel");
    assert_color(&img, (160, 125), WHITE, "bottom-right texel");
    // The anchor moves the quad: feet at the entity's position.
    let mut q = app.world.query::<&mut Sprite>();
    for mut s in q.iter_mut(&mut app.world) {
        if s.image.is_some() {
            s.anchor = [0.5, 0.0];
        }
    }
    let Some(img) = render(&mut app, "anchor", 1) else {
        return;
    };
    assert_eq!(bounds(&img), [75, 65, 175, 115], "anchored at the bottom");
}

#[test]
fn flips_mirror_the_image() {
    let mut app = app_2d(200, 200, 4.0);
    let image = add_image(&mut app, quadrants());
    let sprite = Sprite::new(image, [2.0, 2.0]).with_filter(SpriteFilter::Nearest);
    app.world.spawn((
        Transform::from_xyz(-1.0, 0.0, 0.0),
        sprite.clone().flipped_x(true),
    ));
    app.world.spawn((
        Transform::from_xyz(1.0, 0.0, 0.0),
        Sprite {
            flip_y: true,
            ..sprite
        },
    ));
    let Some(img) = render(&mut app, "flips", 2) else {
        return;
    };
    // Left sprite (pixels 0..100): mirrored left to right.
    assert_color(&img, (25, 75), GREEN, "flip_x top-left");
    assert_color(&img, (75, 75), RED, "flip_x top-right");
    assert_color(&img, (25, 125), WHITE, "flip_x bottom-left");
    // Right sprite (pixels 100..200): mirrored top to bottom.
    assert_color(&img, (125, 75), BLUE, "flip_y top-left");
    assert_color(&img, (175, 75), WHITE, "flip_y top-right");
    assert_color(&img, (125, 125), RED, "flip_y bottom-left");
}

/// A sheet of four 2 × 2 cells: red, green, blue, yellow.
fn sheet() -> Image {
    let row = [RED, RED, GREEN, GREEN, BLUE, BLUE, YELLOW, YELLOW];
    texels(&[&row, &row])
}

const SHEET_COLORS: [[u8; 4]; 4] = [RED, GREEN, BLUE, YELLOW];

#[test]
fn atlas_frames_show_their_cell_and_animation_follows_game_time() {
    let mut app = app_2d(64, 64, 2.0);
    let image = add_image(&mut app, sheet());
    let atlas = SpriteAtlas::grid(4, 1);
    let still = app
        .world
        .spawn((
            Transform::from_xyz(-0.5, 0.0, 0.0),
            Sprite::new(image, [1.0, 1.0])
                .with_atlas(atlas.clone())
                .with_frame(2)
                .with_filter(SpriteFilter::Nearest),
        ))
        .id();
    // Four frames a second; world time advances 1/8 s per update: one frame every 2 updates.
    let animated = app
        .world
        .spawn((
            Transform::from_xyz(0.5, 0.0, 0.0),
            Sprite::new(image, [1.0, 1.0])
                .with_atlas(atlas)
                .with_filter(SpriteFilter::Nearest),
            SpriteAnimation::new(0..4, 4.0),
        ))
        .id();
    let still_at = (16, 32);
    let animated_at = (48, 32);
    // After 3 updates: 0.375 s → frame 1.
    let Some(img) = render(&mut app, "atlas_a", 3) else {
        return;
    };
    assert_color(&img, still_at, BLUE, "frame 2 of the sheet");
    assert_eq!(app.world.get::<Sprite>(animated).unwrap().frame, 1);
    assert_color(&img, animated_at, GREEN, "animation frame 1");
    assert_eq!(app.world.get::<Sprite>(still).unwrap().frame, 2);
    // 3 more: 0.75 s → frame 3.
    let img = render(&mut app, "atlas_b", 3).unwrap();
    assert_color(&img, animated_at, YELLOW, "animation frame 3");
    // Slow motion at a quarter: 6 updates are 0.1875 s of world time, under a frame.
    app.world
        .resource_mut::<TimeDilation>()
        .hold("bullet time", 0.25, 0.0);
    let img = render(&mut app, "atlas_slow", 6).unwrap();
    let time = app.world.get::<SpriteAnimation>(animated).unwrap().time;
    assert!((time - 0.9375).abs() < 1e-4, "animation clock {time}");
    assert_color(&img, animated_at, YELLOW, "still frame 3 in slow motion");
    // Back to full speed: 2 updates move a whole frame on (wrapping to 0).
    app.world.resource_mut::<TimeDilation>().clear();
    let img = render(&mut app, "atlas_fast", 2).unwrap();
    let frame = app.world.get::<Sprite>(animated).unwrap().frame;
    assert_eq!(frame, 0, "after 1.1875 s");
    assert_color(
        &img,
        animated_at,
        SHEET_COLORS[frame as usize],
        "frame 0 again",
    );
}

#[test]
fn tile_map_draws_its_tiles_in_their_cells_and_skips_empty_ones() {
    // 50 px per meter; the map's 4 × 3 one-meter tiles span x -2..2, y -1.5..1.5.
    let mut app = app_2d(200, 200, 4.0);
    // Tileset of 2 × 2 cells, 2 texels each: red, green / blue, a cell that is red on the left
    // and white on the right (to see flips).
    let tileset = add_image(
        &mut app,
        texels(&[
            &[RED, RED, GREEN, GREEN],
            &[RED, RED, GREEN, GREEN],
            &[BLUE, BLUE, RED, WHITE],
            &[BLUE, BLUE, RED, WHITE],
        ]),
    );
    let mut map = TileMap::new(tileset, SpriteAtlas::grid(2, 2), [4, 3], [1.0, 1.0])
        .with_filter(SpriteFilter::Nearest);
    let rows: [[u32; 4]; 3] = [[1, 0, 2, 3], [4, 4 | TILE_FLIP_X, 0, 1], [0, 2, 3, 0]];
    for (y, row) in rows.iter().enumerate() {
        for (x, v) in row.iter().enumerate() {
            map.set(x as u32, y as u32, *v);
        }
    }
    app.world.spawn((Transform::from_xyz(-2.0, -1.5, 0.0), map));
    let Some(img) = render(&mut app, "tile_map", 2) else {
        return;
    };
    let bg = backdrop(&img);
    let cell = |x: u32, y: u32| (25 + x * 50, 25 + 25 + y * 50);
    let expect = [
        [Some(RED), None, Some(GREEN), Some(BLUE)],
        [None, None, None, Some(RED)],
        [None, Some(GREEN), Some(BLUE), None],
    ];
    for (y, row) in expect.iter().enumerate() {
        for (x, want) in row.iter().enumerate() {
            let at = cell(x as u32, y as u32);
            match want {
                Some(c) => assert_color(&img, at, *c, &format!("tile ({x}, {y})")),
                None if rows[y][x] == 0 => {
                    assert_color(&img, at, bg, &format!("empty tile ({x}, {y})"))
                }
                None => {}
            }
        }
    }
    // Tile 4 is red on its left, white on its right; mirrored, white on the left.
    let (x0, y0) = cell(0, 1);
    assert_color(&img, (x0 - 12, y0), RED, "tile 4, left half");
    assert_color(&img, (x0 + 12, y0), WHITE, "tile 4, right half");
    let (x1, y1) = cell(1, 1);
    assert_color(&img, (x1 - 12, y1), WHITE, "mirrored tile 4, left half");
    assert_color(&img, (x1 + 12, y1), RED, "mirrored tile 4, right half");
    // Eight tiles and the backdrop sprite, one draw call each in the prepass and the opaque pass
    // (no shadows: no sun).
    let stats = sprite_stats(&app);
    assert_eq!(
        stats["instances"], 9,
        "eight tiles and the backdrop: {stats}"
    );
    assert!(stats["draw_calls"].as_u64().unwrap() <= 4, "{stats}");
}

/// A 16 × 16 checkerboard of single black and white texels.
fn checker_texels() -> Image {
    Image::from_fn(16, 16, true, |x, y| {
        if (x + y) % 2 == 0 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    })
}

/// Every pixel inside `b` belongs to a `k × k` block aligned with `b`'s corner whose pixels are
/// all black or all white (within the dither): no texel straddles a pixel.
#[track_caller]
fn assert_whole_texels(img: &RgbaImage, b: [u32; 4], k: u32, what: &str) {
    for y in b[1]..b[3] {
        for x in b[0]..b[2] {
            let p = px(img, x, y);
            let corner = px(img, b[0] + (x - b[0]) / k * k, b[1] + (y - b[1]) / k * k);
            let pure = close(p, [0, 0, 0, 255], 3) || close(p, WHITE, 3);
            assert!(
                pure && close(p, corner, 3),
                "{what}: pixel ({x}, {y}) is {p:?}, its block starts with {corner:?}"
            );
        }
    }
}

#[test]
fn pixel_perfect_camera_shows_each_texel_as_a_whole_block() {
    // 180 rows for 2.8 m at 16 texels per meter: 4 pixels per texel (2.8125 m shown).
    let mut app = app_2d(320, 180, 2.8);
    // Temporal anti-aliasing stays on in the settings: the camera turns it off.
    app.world.resource_mut::<RenderSettings>().anti_aliasing = AntiAliasing::Taa;
    let camera = {
        let mut q = app.world.query_filtered::<Entity, With<Camera>>();
        q.single(&app.world).unwrap()
    };
    app.world.entity_mut(camera).insert(PixelPerfect::new(16.0));
    // Off the texel grid: the sprite at 0.3 and the camera at 0.3 + 0.45 texels.
    let image = add_image(&mut app, checker_texels());
    app.world.spawn((
        Transform::from_xyz(0.3 / 16.0, -0.2 / 16.0, 0.0),
        Sprite::pixel_art(image, [16, 16], 16.0),
    ));
    let mut x = 0.0;
    let mut previous: Option<[u32; 4]> = None;
    for (step, dx) in [0.75, 0.45, 0.3].into_iter().enumerate() {
        x += dx / 16.0;
        app.world.get_mut::<Transform>(camera).unwrap().translation =
            DVec3::new(x, 0.13 / 16.0, 10.0);
        let Some(img) = render(&mut app, &format!("pixel_perfect_{step}"), 3) else {
            return;
        };
        let b = bounds(&img);
        assert_eq!(
            [b[2] - b[0], b[3] - b[1]],
            [64, 64],
            "16 texels at 4 px each (step {step}): {b:?}"
        );
        assert_whole_texels(&img, b, 4, &format!("step {step}"));
        // The picture only ever moves by whole texels.
        if let Some(p) = previous {
            let moved = b[0] as i32 - p[0] as i32;
            assert_eq!(moved % 4, 0, "moved {moved} px at step {step}");
        }
        previous = Some(b);
    }
}

/// Lit 2.5D scene: a perspective camera at `eye` looking at `target`, a sun shining along
/// `sun_dir` and a large gray ground.
fn app_3d(eye: DVec3, target: DVec3, sun_dir: DVec3) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut window = app.world.resource_mut::<Window>();
        window.width = 256;
        window.height = 256;
    }
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(sun_dir, Vec3::Y),
    ));
    app
}

fn luma(p: [u8; 4]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// Pixel of a world point.
fn project(app: &mut App, p: DVec3) -> (u32, u32) {
    let (gt, cam) = {
        let mut q = app.world.query::<(&GlobalTransform, &Camera)>();
        let (gt, cam) = q.single(&app.world).unwrap();
        (*gt, *cam)
    };
    let Projection::Perspective { fov_y_deg, .. } = cam.projection else {
        unreachable!()
    };
    let local = gt.rotation().inverse() * (p - gt.translation()).as_vec3();
    let f = 1.0 / (fov_y_deg.to_radians() * 0.5).tan();
    let ndc = Vec2::new(local.x, local.y) * f / -local.z;
    (
        ((ndc.x * 0.5 + 0.5) * 256.0) as u32,
        ((0.5 - ndc.y * 0.5) * 256.0) as u32,
    )
}

fn unlit(color: u32) -> StandardMaterial {
    StandardMaterial {
        unlit: true,
        ..StandardMaterial::hex(color)
    }
}

#[test]
fn cutout_sprite_hides_meshes_behind_it_and_is_hidden_by_meshes_in_front() {
    let mut app = app_3d(
        DVec3::new(0.0, 0.0, 5.0),
        DVec3::ZERO,
        DVec3::new(0.0, -1.0, -0.3),
    );
    // A red card with a hole in its middle.
    let image = add_image(
        &mut app,
        texels(&[
            &[RED, RED, RED, RED],
            &[RED, CLEAR, CLEAR, RED],
            &[RED, CLEAR, CLEAR, RED],
            &[RED, RED, RED, RED],
        ]),
    );
    app.world.spawn((
        Transform::default(),
        Sprite::new(image, [2.0, 2.0])
            .with_filter(SpriteFilter::Nearest)
            .with_alpha(SpriteAlpha::Cutout(0.5)),
    ));
    // A green plate behind the card, a blue bar in front of its lower part.
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -1.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 3.0, 0.1],
            },
            unlit(0x00ff00),
        ),
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, -0.75, 1.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 0.3, 0.1],
            },
            unlit(0x0000ff),
        ),
    ));
    // A translucent white strip between the plate and the card's top, drawn after everything
    // opaque: only the card's depth keeps it off the card.
    app.world.spawn((
        Transform::from_xyz(0.0, 0.75, -0.5),
        Sprite::solid([1.0, 1.0, 1.0, 0.9], [3.0, 0.3]),
    ));
    let Some(img) = render(&mut app, "cutout_depth", 3) else {
        return;
    };
    let card = project(&mut app, DVec3::new(0.75, 0.75, 0.0));
    let hole = project(&mut app, DVec3::new(0.0, 0.1, 0.0));
    let bar = project(&mut app, DVec3::new(-0.75, -0.75, 1.0));
    let strip = project(&mut app, DVec3::new(1.4, 0.75, -0.5));
    let red = px(&img, card.0, card.1);
    assert!(
        red[0] > 150 && red[1] < 60 && red[2] < 60,
        "the card in front of the plate and the strip: {red:?}"
    );
    let white = px(&img, strip.0, strip.1);
    assert!(
        white.iter().take(3).all(|&c| c > 150),
        "the strip beside the card: {white:?}"
    );
    let green = px(&img, hole.0, hole.1);
    assert!(
        green[1] > 150 && green[0] < 60,
        "the plate through the card's hole: {green:?}"
    );
    let blue = px(&img, bar.0, bar.1);
    assert!(
        blue[2] > 150 && blue[0] < 60,
        "the bar in front of the card: {blue:?}"
    );
}

#[test]
fn lit_sprite_is_darker_in_shadow_than_in_sun_and_cutouts_cast_shadows() {
    // Looking steeply down at the ground; the sun shines from the left at 45°.
    let sun = DVec3::new(1.0, -1.0, 0.0);
    let mut app = app_3d(DVec3::new(2.0, 12.0, 4.0), DVec3::new(2.0, 0.0, 0.0), sun);
    let gray = StandardMaterial::color(0.5, 0.5, 0.5);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            gray.clone(),
        ),
    ));
    // A white paper sprite lying on the ground (facing up), lit; a 2 m wall to its left shades
    // its left half (the wall's shadow reaches x = 0.1).
    app.world.spawn((
        Transform::from_xyz(0.0, 0.01, 0.0).with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
        Sprite::solid([1.0, 1.0, 1.0, 1.0], [3.0, 3.0])
            .with_alpha(SpriteAlpha::Cutout(0.5))
            .lit(),
    ));
    app.world.spawn((
        Transform::from_xyz(-2.0, 1.0, 0.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.2, 2.0, 6.0],
            },
            gray,
        ),
    ));
    // A cut-out card standing at x = 4 facing the sun (its width along z), a window in its
    // middle: its shadow on the ground is the card sheared to x = 4..6, window x = 4.5..5.5,
    // z = -0.5..0.5.
    let card = add_image(
        &mut app,
        texels(&[
            &[WHITE, WHITE, WHITE, WHITE],
            &[WHITE, CLEAR, CLEAR, WHITE],
            &[WHITE, CLEAR, CLEAR, WHITE],
            &[WHITE, WHITE, WHITE, WHITE],
        ]),
    );
    app.world.spawn((
        Transform::from_xyz(4.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(-FRAC_PI_2)),
        Sprite::new(card, [2.0, 2.0])
            .with_anchor([0.5, 0.0])
            .with_filter(SpriteFilter::Nearest)
            .with_alpha(SpriteAlpha::Cutout(0.5))
            .lit(),
    ));
    let Some(img) = render(&mut app, "lit_shadow", 4) else {
        return;
    };
    let mut at = |x: f64, y: f64, z: f64| {
        let p = project(&mut app, DVec3::new(x, y, z));
        luma(px(&img, p.0, p.1))
    };
    let shade = at(-0.75, 0.01, 0.0);
    let sun_lit = at(0.75, 0.01, 0.0);
    assert!(
        sun_lit > 60.0 && shade < sun_lit * 0.6,
        "lit sprite: {shade} in the wall's shadow, {sun_lit} in the sun"
    );
    // The card's shadow: dark under its frame, light through its window.
    let open = at(7.0, 0.0, 0.0);
    let top_frame = at(5.75, 0.0, 0.0);
    let side_frame = at(5.0, 0.0, -0.75);
    let window = at(5.0, 0.0, 0.0);
    for (what, v) in [("top", top_frame), ("side", side_frame)] {
        assert!(
            v < open * 0.6,
            "the card's {what} frame casts a shadow: {v} vs open ground {open}"
        );
    }
    assert!(
        window > open * 0.9,
        "light comes through the card's window: {window} vs open ground {open}"
    );
}

#[test]
fn translucent_sprites_blend_back_to_front_within_their_layer() {
    let mut app = app_2d(100, 100, 2.0);
    // Pure black behind (an opaque backdrop drawn in front of the gray one).
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -5.0),
        Sprite::solid([0.0, 0.0, 0.0, 1.0], [10.0, 10.0]).with_alpha(SpriteAlpha::Cutout(0.5)),
    ));
    // The near blue sprite is spawned first; the far red one second.
    app.world.spawn((
        Transform::from_xyz(0.25, 0.0, 1.0),
        Sprite::solid([0.0, 0.0, 1.0, 0.5], [1.0, 1.0]),
    ));
    app.world.spawn((
        Transform::from_xyz(-0.25, 0.0, -1.0),
        Sprite::solid([1.0, 0.0, 0.0, 0.5], [1.0, 1.0]),
    ));
    let Some(img) = render(&mut app, "blend_order", 2) else {
        return;
    };
    // Overlap: blue × 0.5 over (red × 0.5 over black): linear (0.25, 0, 0.5).
    let encode = |v: f32| (v.powf(1.0 / 2.2) * 255.0).round() as u8;
    let overlap = px(&img, 50, 50);
    assert!(
        close(overlap, [encode(0.25), 0, encode(0.5), 255], 6),
        "near over far: {overlap:?}"
    );
    // A layer outranks depth: the far sprite on a higher layer draws over the near one.
    let mut q = app.world.query::<&mut Sprite>();
    for mut s in q.iter_mut(&mut app.world) {
        if s.color == [1.0, 0.0, 0.0, 0.5] {
            s.layer = 1;
        }
    }
    let img = render(&mut app, "blend_layers", 1).unwrap();
    let overlap = px(&img, 50, 50);
    assert!(
        close(overlap, [encode(0.5), 0, encode(0.25), 255], 6),
        "the higher layer on top: {overlap:?}"
    );
}

#[test]
fn billboards_face_the_camera() {
    // Seen from 45° to the side: a fixed sprite is foreshortened, billboards are not.
    let eye = DVec3::new(5.0, 0.0, 5.0);
    let mut app = app_3d(eye, DVec3::ZERO, DVec3::new(0.0, -1.0, -0.3));
    for (x, billboard, color) in [
        (-1.4, Billboard::Fixed, [1.0, 0.0, 0.0, 1.0]),
        (0.0, Billboard::Upright, [0.0, 1.0, 0.0, 1.0]),
        (1.4, Billboard::FaceCamera, [0.0, 0.0, 1.0, 1.0]),
    ] {
        // Along the line through the origin perpendicular to the view.
        let p = DVec3::new(x, 0.0, -x) * std::f64::consts::FRAC_1_SQRT_2;
        app.world.spawn((
            Transform::from_translation(p),
            Sprite::solid(color, [1.0, 1.0])
                .with_billboard(billboard)
                .with_alpha(SpriteAlpha::Cutout(0.5)),
        ));
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        *s = s.clone().with_flat_colors();
    }
    let Some(img) = render(&mut app, "billboards", 3) else {
        return;
    };
    // Width of each color's run along the middle row.
    let row = 128;
    let width = |channel: usize| {
        (0..256)
            .filter(|&x| {
                let p = px(&img, x, row);
                p[channel] > 100 && (0..3).all(|c| c == channel || p[c] < 80)
            })
            .count() as f32
    };
    let (fixed, upright, facing) = (width(0), width(1), width(2));
    assert!(upright > 20.0, "the upright billboard shows: {upright}");
    assert!(
        (upright - facing).abs() <= 2.0,
        "both billboards face the camera: {upright} vs {facing}"
    );
    assert!(
        fixed < upright * 0.8,
        "the fixed sprite is seen at an angle: {fixed} vs {upright}"
    );
}

#[test]
fn parallax_layers_follow_the_camera_at_their_factor_and_repeat() {
    let mut app = app_2d(200, 100, 2.0);
    // A 1 m red block on a layer at half speed, repeating every 4 m.
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        Sprite::solid([1.0, 0.0, 0.0, 1.0], [1.0, 1.0]),
        Parallax::new(0.5).repeating(4.0),
    ));
    let red_columns = |img: &RgbaImage| {
        (0..200)
            .filter(|&x| px(img, x, 50)[0] > 200 && px(img, x, 50)[1] < 50)
            .collect::<Vec<u32>>()
    };
    let Some(img) = render(&mut app, "parallax_0", 2) else {
        return;
    };
    // 50 px per meter: the block at the center, 150..250 out of view on the right.
    let at_rest = red_columns(&img);
    assert_eq!(
        (at_rest.first(), at_rest.last(), at_rest.len()),
        (Some(&75), Some(&124), 50),
        "the block in the middle"
    );
    // The camera moves 3 m right: the layer moves 1.5 m with it, so it shows 1.5 m left of
    // where it was, and its next copy (4 m on) comes in at 2.5 m right of it.
    let camera = {
        let mut q = app.world.query_filtered::<Entity, With<Camera>>();
        q.single(&app.world).unwrap()
    };
    app.world.get_mut::<Transform>(camera).unwrap().translation = DVec3::new(3.0, 0.0, 10.0);
    let img = render(&mut app, "parallax_3", 1).unwrap();
    let moved = red_columns(&img);
    // The view spans x = 1..5: the block (moved 1.5 m) spans 1..2, pixels 0..50; its next
    // copy, 4 m on, spans 5..6, just out of view.
    assert_eq!(
        (moved.first(), moved.last()),
        (Some(&0), Some(&49)),
        "the layer at half speed: {moved:?}"
    );
    // Far along, copies keep coming.
    app.world.get_mut::<Transform>(camera).unwrap().translation = DVec3::new(1001.0, 0.0, 10.0);
    let img = render(&mut app, "parallax_far", 1).unwrap();
    assert!(
        !red_columns(&img).is_empty(),
        "a repeated copy is in view 1 km on"
    );
}

#[test]
fn scenes_without_sprites_build_nothing() {
    let mut app = app_2d(64, 64, 2.0);
    // Remove the backdrop: no sprites at all.
    let sprites: Vec<Entity> = app
        .world
        .query_filtered::<Entity, With<Sprite>>()
        .iter(&app.world)
        .collect();
    for e in sprites {
        app.world.despawn(e);
    }
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(Shape::Cuboid { size: [1.0; 3] }, unlit(0xff0000)),
    ));
    if render(&mut app, "no_sprites", 2).is_none() {
        return;
    }
    let stats = sprite_stats(&app);
    assert_eq!(stats["draw_calls"], 0, "{stats}");
    assert_eq!(stats["state"]["pipelines"], false, "{stats}");
}
