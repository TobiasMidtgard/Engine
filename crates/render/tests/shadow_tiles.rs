//! A shadow map is put back to its static image only where moving casters were drawn the frame
//! before (`shadows::ShadowTiles`, `RenderFeature::shadow_moving`), not copied whole every
//! frame: crates sliding over a floor cast the same shadows as with every map copied, for a
//! hundredth of the texels, nothing is put back while nothing moves, and crates that stop join
//! the images by tile. And every feature
//! in the workspace that draws moving casters names their tiles. The pictures need a GPU or
//! lavapipe; that part passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::{shade_skip, trial_cut_more};
use engine_render::{RenderDisabled, RenderStats, capture_now};

const W: u32 = 320;
const H: u32 = 180;

/// What a run of [`slide`] did.
struct Slid {
    /// The last sliding frame's picture.
    pixels: Vec<u8>,
    /// Whole layers copied and tiles put back in five frames before the crates set off (they
    /// had stood long enough to be in the static images).
    standing: (u32, u32),
    /// The same over the 24 frames they slid.
    sliding: (u32, u32),
    /// Over the 110 frames after they stopped, in which they join the static images.
    stopping: (u32, u32),
    /// And over five frames after those.
    rested: (u32, u32),
    /// The picture then.
    rested_pixels: Vec<u8>,
}

/// Crates slide over a floor between pillars, then stop. `tiles`: maps are put back by tile
/// (else copied whole every frame); `never`: they are never put back (the measuring switch).
fn slide(tiles: bool, never: bool, shot: &str) -> Option<Slid> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        if !tiles {
            s.trial_cuts_more &= !trial_cut_more::SHADOW_TILES;
        }
        if never {
            s.debug_skip |= shade_skip::SHADOW_COPIES;
        }
    }
    let w = &mut app.world;
    // From above, so the picture is floor and shadows.
    w.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 14.0, 6.0).looking_at(DVec3::new(0.0, 0.0, -8.0), Vec3::Y),
    ));
    let e = 35f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.7 * e.cos(), -e.sin(), -0.7 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.6, 0.6, 0.55),
        ),
    ));
    for i in 0..6 {
        w.spawn((
            Transform::from_xyz(-10.0 + i as f64 * 4.0, 1.5, -16.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.8, 3.0, 0.8],
                },
                StandardMaterial::color(0.5, 0.5, 0.5),
            ),
        ));
    }
    let crates: Vec<Entity> = (0..3)
        .map(|i| {
            w.spawn((
                Transform::from_xyz(-9.0, 0.6, -4.0 - i as f64 * 4.0),
                MeshRenderer::new(
                    Shape::Cuboid {
                        size: [1.2, 1.2, 1.2],
                    },
                    StandardMaterial::color(0.5, 0.3, 0.2),
                ),
            ))
            .id()
        })
        .collect();
    // The floor and the pillars join the static images.
    app.update_n(140);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let put_back = |app: &App| {
        let s = app.world.resource::<RenderStats>();
        (s.shadow_layers_copied, s.shadow_tiles_restored)
    };
    let mut standing = (0, 0);
    for _ in 0..5 {
        app.update();
        let (layers, tiles) = put_back(&app);
        standing = (standing.0 + layers, standing.1 + tiles);
    }
    let mut sliding = (0, 0);
    for _ in 0..24 {
        for (i, c) in crates.iter().enumerate() {
            app.world
                .get_mut::<Transform>(*c)
                .expect("crate")
                .translation
                .x += 0.5 + i as f64 * 0.15;
        }
        app.update();
        let (layers, tiles) = put_back(&app);
        sliding = (sliding.0 + layers, sliding.1 + tiles);
    }
    let dir = std::env::temp_dir().join("engine-shadow-tiles");
    let _ = std::fs::create_dir_all(&dir);
    let picture = |app: &mut App, shot: &str| {
        let path = dir.join(shot);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
        }
        image::open(&path)
            .expect("read the shot")
            .to_rgb8()
            .into_raw()
    };
    let pixels = picture(&mut app, shot);
    let mut stopping = (0, 0);
    for _ in 0..110 {
        app.update();
        let (layers, tiles) = put_back(&app);
        stopping = (stopping.0 + layers, stopping.1 + tiles);
    }
    let mut rested = (0, 0);
    for _ in 0..5 {
        app.update();
        let (layers, tiles) = put_back(&app);
        rested = (rested.0 + layers, rested.1 + tiles);
    }
    let rested_pixels = picture(&mut app, &shot.replace(".png", "_rested.png"));
    Some(Slid {
        pixels,
        standing,
        sliding,
        stopping,
        rested,
        rested_pixels,
    })
}

/// Pixels that differ by more than a rounding between two pictures.
fn differing(a: &[u8], b: &[u8]) -> usize {
    a.chunks(3)
        .zip(b.chunks(3))
        .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 6))
        .count()
}

#[test]
fn moving_casters_leave_no_shadow_behind() {
    let Some(whole) = slide(false, false, "tiles_copied_whole.png") else {
        return;
    };
    let Some(tiles) = slide(true, false, "tiles_put_back.png") else {
        return;
    };
    let Some(never) = slide(true, true, "tiles_never_put_back.png") else {
        return;
    };
    let px = (W * H) as usize;
    let same = differing(&whole.pixels, &tiles.pixels);
    let trails = differing(&whole.pixels, &never.pixels);
    let rested = differing(&whole.rested_pixels, &tiles.rested_pixels);
    eprintln!(
        "layers copied whole and tiles put back, standing then sliding: {:?} then {:?} copying \
         whole, {:?} then {:?} by tile (then {:?} stopping, {:?} rested); {same} of {px} pixels \
         differ by tile ({rested} once rested), {trails} with nothing put back",
        whole.standing, whole.sliding, tiles.standing, tiles.sliding, tiles.stopping, tiles.rested
    );
    // The same picture as with every map copied whole every frame.
    assert!(same * 2000 < px, "{same} of {px} pixels differ");
    // And the comparison would show a shadow left behind.
    assert!(
        trails * 200 > px,
        "only {trails} pixels differ with nothing put back"
    );
    // Copied whole: every map, every frame, moving or not.
    assert!(whole.standing.0 >= 5 * 4, "{:?}", whole.standing);
    assert!(whole.sliding.0 >= 24 * 4, "{:?}", whole.sliding);
    // By tile, nothing moving: nothing is put back.
    assert_eq!(tiles.standing, (0, 0), "put back with nothing moving");
    // Sliding: each map once whole at most (its static image is drawn again without the
    // crates when they set off), then the tiles under the crates: a hundredth of the texels
    // (a layer is 256 tiles).
    assert!(tiles.sliding.0 <= 6, "{:?}", tiles.sliding);
    assert!(tiles.sliding.1 > 0, "no tile was put back");
    // Stopped, the crates join the static images where they stand: tiles again, no layer.
    assert_eq!(tiles.stopping.0, 0, "{:?}", tiles.stopping);
    assert!(tiles.stopping.1 > 0, "{:?}", tiles.stopping);
    assert_eq!(
        tiles.rested,
        (0, 0),
        "put back with the crates in the images"
    );
    assert!(rested * 2000 < px, "{rested} of {px} pixels differ at rest");
    let texels = |(layers, tiles): (u32, u32)| layers * 256 + tiles;
    assert!(
        texels(tiles.sliding) * 10 < texels(whole.sliding),
        "{:?} by tile, {:?} whole",
        tiles.sliding,
        whole.sliding
    );
}

/// Rust sources under `dir`, its subfolders included.
fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A map is put back only in the tiles its features named the frame before, and is not drawn
/// into at all when none names any: a feature that draws moving casters without naming their
/// tiles would cast no shadow, or leave it behind.
#[test]
fn every_feature_that_draws_shadows_names_its_tiles() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["crates", "games", "tools"] {
        sources(&root.join(dir), &mut files);
    }
    assert!(files.len() > 100, "found {} sources", files.len());
    let mut features = 0;
    let mut missing = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Each `impl RenderFeature for X { ... }`, to its closing brace at the line's start.
        let mut rest = text.as_str();
        while let Some(at) = rest.find("impl RenderFeature for ") {
            let block = &rest[at..];
            let end = block.find("\n}\n").unwrap_or(block.len());
            let (block, after) = block.split_at(end);
            rest = after;
            if block.contains("fn draw_shadow(") || block.contains("fn draw_far_shadow(") {
                features += 1;
                if !block.contains("fn shadow_moving(") {
                    let name = block.lines().next().unwrap_or_default();
                    missing.push(format!("{}: {name}", path.display()));
                }
            }
        }
    }
    assert!(
        features >= 10,
        "found {features} features that draw shadows"
    );
    assert!(
        missing.is_empty(),
        "these draw moving shadow casters without `shadow_moving`:\n{}",
        missing.join("\n")
    );
}
