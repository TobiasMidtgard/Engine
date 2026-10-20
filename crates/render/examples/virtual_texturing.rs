//! Renders the virtual texturing gallery: a rock wall and a log, each twice side by side (the
//! old way, a 2k texture uploaded whole, next to the full-resolution scan streamed through the
//! tile cache), seen from 30 cm and from far away, in the tile debug view, and a forest floor
//! repeated across a clearing with and without anti-tiling. Prints the cache numbers and frame
//! times for every shot, and the bake time and size of each scan.
//!
//! ```text
//! cargo run -p fetch-assets -- render      # the scans (once; without them: procedural)
//! cargo run -p engine_render --example virtual_texturing -- <output dir>
//! ```
//!
//! The first run bakes a tiled copy next to each scan (`<file>.vtex`), which takes a minute or
//! so for a 16k texture; later runs stream from it straight away. Needs a GPU or lavapipe.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use engine_assets::{AssetPlugin, AssetServer, Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::virtual_texture::file_exists;
use engine_render::{
    DebugView, Exposure, RenderDisabled, VirtualTextureStats, VirtualTextureStatus, capture_now,
    paint_tile_with,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

/// The scans (Poly Haven, CC0; `assets/manifests/render.toml`).
const ROCK: &str = "fetched/render/rock_face/rock_face";
const BARK: &str = "fetched/render/pine_bark/pine_bark";
const GROUND: &str = "fetched/render/forrest_ground_01/forrest_ground_01";

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x
        .wrapping_mul(0x8da6_b343)
        .wrapping_add(y.wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

/// Stand-in when a scan wasn't fetched: grainy stripes painted at any resolution.
struct Stripes {
    size: u32,
    srgb: bool,
}

impl Stripes {
    fn texel(&self, x: u32, y: u32, w: u32) -> [u8; 4] {
        let scale = self.size as f32 / w as f32;
        let stripe = ((x as f32 * scale / 97.0).sin() * 0.5 + 0.5) * 0.6;
        let grain = if scale > 4.0 { 0.5 } else { hash(x, y, 3) };
        let v = 0.25 + 0.5 * stripe + 0.2 * (grain - 0.5);
        if self.srgb {
            let c = (v * 255.0) as u8;
            [c, (c as f32 * 0.85) as u8, (c as f32 * 0.7) as u8, 255]
        } else {
            [128, 128, 255, 255]
        }
    }
}

impl TileSource for Stripes {
    fn size(&self) -> (u32, u32) {
        (self.size, self.size)
    }

    fn srgb(&self) -> bool {
        self.srgb
    }

    fn paint_tile(&self, mip: u32, x: u32, y: u32, out: &mut [u8]) {
        paint_tile_with(self.size(), mip, x, y, out, |tx, ty, w, _| {
            self.texel(tx, ty, w)
        });
    }
}

/// The maps of one material: color, normal, and roughness/occlusion (`arm`), if any.
#[derive(Clone, Copy)]
struct Maps {
    diff: Handle<Image>,
    nor: Handle<Image>,
    arm: Option<Handle<Image>>,
}

impl Maps {
    fn material(self) -> StandardMaterial {
        let mut m = StandardMaterial::color(1.0, 1.0, 1.0)
            .with_roughness(0.9)
            .with_base_color_texture(self.diff)
            .with_normal_texture(self.nor);
        if let Some(arm) = self.arm {
            // ARM: occlusion, roughness, metallic in R, G, B; the factors let the map decide.
            m = m
                .with_roughness(1.0)
                .with_metallic(1.0)
                .with_metallic_roughness_texture(arm)
                .with_occlusion_texture(arm);
        }
        m
    }
}

/// The scan at `res` streamed as a virtual texture (`None` when it wasn't fetched).
fn virtual_maps(app: &mut App, base: &str, res: &str, arm: bool) -> Option<Maps> {
    let key = |map: &str| format!("{base}_{map}_{res}.jpg");
    let server = app.world.resource::<AssetServer>();
    let maps = ["diff", "nor_gl", "arm"];
    if !maps[..if arm { 3 } else { 2 }]
        .iter()
        .all(|m| file_exists(server, &key(m)))
    {
        return None;
    }
    let mut out = None;
    app.world
        .resource_scope(|world, mut vt: Mut<VirtualTextures>| {
            world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                let server = world.resource::<AssetServer>();
                out = Some(Maps {
                    diff: vt.load(server, &mut images, &key("diff"), true),
                    nor: vt.load(server, &mut images, &key("nor_gl"), false),
                    arm: arm.then(|| vt.load(server, &mut images, &key("arm"), false)),
                });
            })
        });
    out
}

/// The scan at `res` loaded whole, the ordinary way (`None` when it wasn't fetched).
fn whole_maps(app: &mut App, base: &str, res: &str, arm: bool) -> Option<Maps> {
    let key = |map: &str| format!("{base}_{map}_{res}.jpg");
    let mut out = None;
    app.world
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            let server = world.resource::<AssetServer>();
            let maps = ["diff", "nor_gl", "arm"];
            if maps[..if arm { 3 } else { 2 }]
                .iter()
                .all(|m| file_exists(server, &key(m)))
            {
                out = Some(Maps {
                    diff: server.load(&mut images, &key("diff")),
                    nor: server.load(&mut images, &key("nor_gl")),
                    arm: arm.then(|| server.load(&mut images, &key("arm"))),
                });
            }
        });
    out
}

/// Procedural stand-in maps at `size` (virtual when over the threshold).
fn stand_in(app: &mut App, size: u32) -> Maps {
    app.world
        .resource_scope(|world, mut vt: Mut<VirtualTextures>| {
            let mut images = world.resource_mut::<Assets<Image>>();
            let diff = vt.add_procedural(&mut images, Stripes { size, srgb: true });
            let nor = vt.add_procedural(&mut images, Stripes { size, srgb: false });
            Maps {
                diff: diff.expect("stand-in"),
                nor: nor.expect("stand-in"),
                arm: None,
            }
        })
}

fn stats(app: &App) -> VirtualTextureStats {
    app.world.resource::<VirtualTextureStats>().clone()
}

/// Run frames until the tiles stop streaming (or `max` frames). Returns (frames, the most
/// tiles loaded in one frame, all tiles loaded).
fn settle(app: &mut App, max: u32) -> (u32, u32, u64) {
    let before = stats(app).tiles_loaded_total;
    let (mut quiet, mut peak) = (0, 0);
    for frame in 1..=max {
        app.update();
        let s = stats(app);
        peak = peak.max(s.tiles_loaded);
        if s.pending == 0 && s.tiles_loaded == 0 && s.feedback_misses == 0 {
            quiet += 1;
            if quiet >= 24 {
                return (frame, peak, s.tiles_loaded_total - before);
            }
        } else {
            quiet = 0;
        }
    }
    (max, peak, stats(app).tiles_loaded_total - before)
}

/// Mean wall time of `n` frames, milliseconds.
fn frame_ms(app: &mut App, n: u32) -> f64 {
    let start = Instant::now();
    for _ in 0..n {
        app.update();
    }
    start.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn set_camera(app: &mut App, eye: DVec3, target: DVec3) {
    let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(&mut app.world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "virtual_texturing".into()),
    );
    std::fs::create_dir_all(&out).expect("output directory");
    engine_core::log::init("info");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WIDTH;
        w.height = HEIGHT;
    }
    app.add_plugins(AssetPlugin {
        hot_reload: false,
        root: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into()),
    });
    app.add_plugins(RenderPlugin);

    // The full-resolution scans stream; the 2k ones load whole, as before.
    let rock_full = virtual_maps(&mut app, ROCK, "16k", true);
    let bark_full = virtual_maps(&mut app, BARK, "8k", false);
    let rock_2k = whole_maps(&mut app, ROCK, "2k", true);
    let bark_2k = whole_maps(&mut app, BARK, "2k", false);
    let ground = whole_maps(&mut app, GROUND, "2k", false);
    if rock_full.is_none() || bark_full.is_none() {
        eprintln!(
            "scans missing (run `cargo run -p fetch-assets -- render`): using procedural stand-ins"
        );
    }
    let rock_full = rock_full.unwrap_or_else(|| stand_in(&mut app, 16384));
    let bark_full = bark_full.unwrap_or_else(|| stand_in(&mut app, 8192));
    let rock_2k = rock_2k.unwrap_or(rock_full);
    let bark_2k = bark_2k.unwrap_or(bark_full);

    // Ground: the forest floor 2k scan repeated every 2.5 m (anti-tiled or not).
    let ground_material = ground
        .map(Maps::material)
        .unwrap_or_else(|| StandardMaterial::color(0.2, 0.17, 0.12))
        .with_roughness(1.0)
        .with_triplanar_world()
        .with_texture_scale(2.5);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(55.0)
        },
        Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.55, -0.45, -0.7), Vec3::Y),
    ));
    let plane = Shape::Plane {
        size: [120.0, 120.0],
        subdivisions: 1,
    };
    let tiled = w
        .spawn((
            Transform::default(),
            MeshRenderer::new(plane, ground_material),
        ))
        .id();
    let anti_tiled = w
        .spawn((
            Transform::default(),
            MeshRenderer::new(plane, ground_material.with_anti_tiling(true)),
        ))
        .id();
    // Rock walls (3 m faces, the scan once across): 2k on the left, 16k on the right.
    let wall = Shape::Cuboid {
        size: [3.0, 3.0, 0.8],
    };
    for (x, maps) in [(-1.8, rock_2k), (1.8, rock_full)] {
        w.spawn((
            Transform::from_xyz(x, 1.5, -2.0),
            MeshRenderer::new(wall, maps.material()),
        ));
    }
    // Logs (35 cm thick, 4 m long, lying along x): 2k in front, 8k behind.
    let log = Shape::Cylinder {
        radius: 0.35,
        height: 4.0,
        segments: 96,
    };
    for (z, maps) in [(1.6, bark_2k), (0.5, bark_full)] {
        w.spawn((
            Transform::from_xyz(0.0, 0.35, z)
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
            MeshRenderer::new(log, maps.material().with_uv_scale(1.0, 1.8)),
        ));
    }

    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    // Bakes (first run only) and the 2k loads.
    let started = Instant::now();
    while !app.world.resource::<VirtualTextures>().all_opened() {
        assert!(
            started.elapsed() < Duration::from_secs(1800),
            "bakes didn't finish"
        );
        std::thread::sleep(Duration::from_millis(200));
        app.update();
    }
    {
        let vt = app.world.resource::<VirtualTextures>();
        for (name, h) in [
            ("rock diff", rock_full.diff),
            ("rock nor_gl", rock_full.nor),
            ("bark diff", bark_full.diff),
            ("bark nor_gl", bark_full.nor),
        ]
        .into_iter()
        .chain(rock_full.arm.map(|a| ("rock arm", a)))
        {
            match (vt.status(h), vt.bake_report(h)) {
                (VirtualTextureStatus::Ready, Some(r)) => eprintln!(
                    "{name}: baked {}x{}, {} levels, {} tiles ({:?}), {:.1} MB in {:.1} s",
                    r.width,
                    r.height,
                    r.levels,
                    r.tiles,
                    r.codec,
                    r.bytes as f64 / 1e6,
                    r.seconds
                ),
                (status, _) => eprintln!("{name}: {status:?} (tile file already baked)"),
            }
        }
    }
    eprintln!("opened in {:.1} s", started.elapsed().as_secs_f32());
    app.update_n(10);

    let rock_close = (DVec3::new(2.05, 1.62, -1.32), DVec3::new(1.85, 1.5, -1.6));
    let rock_close_2k = (
        rock_close.0 - DVec3::new(3.6, 0.0, 0.0),
        rock_close.1 - DVec3::new(3.6, 0.0, 0.0),
    );
    // 25 cm from the 8k log's top, looking down along it; the 2k log is 1.1 m nearer the
    // camera at the same height.
    let bark_close = (DVec3::new(0.3, 0.88, 0.62), DVec3::new(0.05, 0.62, 0.5));
    let bark_close_2k = (
        bark_close.0 + DVec3::new(0.0, 0.0, 1.1),
        bark_close.1 + DVec3::new(0.0, 0.0, 1.1),
    );
    let far = (DVec3::new(6.0, 3.0, 22.0), DVec3::new(0.0, 1.0, -1.0));
    // From 6 m up the 2.5 m repeat of the forest floor shows plainly.
    let clearing = (DVec3::new(0.0, 6.0, 16.0), DVec3::new(0.0, 0.0, -2.0));
    let shots = [
        ("01_rock_closeup_2k", rock_close_2k, DebugView::None, false),
        (
            "02_rock_closeup_virtual_16k",
            rock_close,
            DebugView::None,
            false,
        ),
        ("03_bark_closeup_2k", bark_close_2k, DebugView::None, false),
        (
            "04_bark_closeup_virtual_8k",
            bark_close,
            DebugView::None,
            false,
        ),
        ("05_far", far, DebugView::None, false),
        (
            "06_tiles_debug_rock_closeup",
            rock_close,
            DebugView::VirtualTextures,
            false,
        ),
        ("07_tiles_debug_far", far, DebugView::VirtualTextures, false),
        ("08_ground_tiled_before", clearing, DebugView::None, false),
        (
            "09_ground_anti_tiled_after",
            clearing,
            DebugView::None,
            true,
        ),
    ];
    for (name, (eye, target), debug, anti) in shots {
        set_camera(&mut app, eye, target);
        app.world.resource_mut::<RenderSettings>().debug_view = debug;
        for (e, show) in [(tiled, !anti), (anti_tiled, anti)] {
            if let Some(mut r) = app.world.get_mut::<MeshRenderer>(e) {
                r.visible = show;
            }
        }
        let (frames, peak, loaded) = settle(&mut app, 900);
        app.update_n(4);
        let ms = frame_ms(&mut app, 10);
        let path = out.join(format!("{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        let s = stats(&app);
        eprintln!(
            "{name}: settled in {frames} frames, {loaded} tiles loaded (at most {peak} a frame), \
             {} resident ({:.1} of {:.1} MB cache, {} slots, {}), by level {:?}, {ms:.0} ms/frame",
            s.resident_tiles,
            s.resident_bytes as f64 / 1e6,
            s.cache_bytes as f64 / 1e6,
            s.cache_tiles,
            if s.compressed { "BC7" } else { "RGBA8" },
            s.resident_by_level
        );
    }

    // Flying from far to the rock close-up: tiles per frame and memory on the way.
    app.world.resource_mut::<RenderSettings>().debug_view = DebugView::None;
    let (mut peak, mut total, mut max_resident) = (0, 0, 0u64);
    let steps = 120;
    let start = Instant::now();
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let s = t * t * (3.0 - 2.0 * t);
        set_camera(
            &mut app,
            far.0.lerp(rock_close.0, s),
            far.1.lerp(rock_close.1, s),
        );
        app.update();
        let st = stats(&app);
        peak = peak.max(st.tiles_loaded);
        total += st.tiles_loaded as u64;
        max_resident = max_resident.max(st.resident_bytes);
    }
    let s = stats(&app);
    let fly_in_evictions = s.evictions;
    eprintln!(
        "fly-in: {steps} frames, {:.0} ms/frame, {total} tiles loaded (at most {peak} a frame), \
         at most {:.1} MB resident of {:.1} MB, {} evictions",
        start.elapsed().as_secs_f64() * 1000.0 / steps as f64,
        max_resident as f64 / 1e6,
        s.cache_bytes as f64 / 1e6,
        fly_in_evictions
    );

    // The same close and far views with an uncompressed cache (software drivers decode BC7 on
    // every sample, which dominates their frame time).
    app.world
        .resource_mut::<RenderSettings>()
        .virtual_texture_compression = false;
    for (name, (eye, target)) in [("rock close-up", rock_close), ("far", far)] {
        set_camera(&mut app, eye, target);
        settle(&mut app, 900);
        let ms = frame_ms(&mut app, 6);
        let s = stats(&app);
        eprintln!(
            "RGBA8 cache, {name}: {} resident ({:.1} of {:.1} MB cache), {ms:.0} ms/frame",
            s.resident_tiles,
            s.resident_bytes as f64 / 1e6,
            s.cache_bytes as f64 / 1e6
        );
    }
}
