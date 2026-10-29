//! Look development for small plants and cave fungi, rendered headless into PNGs:
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_small --release -- /tmp/lookdev_small after
//! ```
//!
//! A sunlit forest-edge patch (a meadow of grass clumps with wildflowers, a fern bed and a
//! bluebell drift) seen at eye level and close up, and a dark cave floor of glowcaps and lantern
//! caps lit only by their own glow, plus near and far meshes side by side from the 22 m
//! switch distance. The second argument prefixes the file names, so renders before and after
//! a change sit side by side. Also prints triangle counts per species.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::assets::FoliageVisuals;
use engine_foliage::meshgen::small::small;
use engine_foliage::{FoliagePlugin, PlantLibrary, spawn_plant};
use engine_render::prelude::*;
use engine_render::{Exposure, Screenshots, SkyLightVolume};
use engine_world::WeatherState;

/// The cave sits far enough away that its walls never show in the meadow shots.
const CAVE: DVec3 = DVec3::new(300.0, 0.0, 0.0);
const SUN_LUX: f32 = 90_000.0;
/// Where near and far meshes are compared side by side.
const LOD_PATCH: DVec3 = DVec3::new(0.0, 0.0, 120.0);

fn set_sun(world: &mut World, dir: Vec3, lux: f32) {
    world.resource_mut::<WeatherState>().sun_direction = dir;
    let mut q = world.query::<(&mut Transform, &mut DirectionalLight)>();
    for (mut t, mut light) in q.iter_mut(world) {
        let mut sun = Transform::default();
        sun.look_to(-dir, Vec3::Y);
        *t = sun;
        light.illuminance = lux;
    }
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn set_fov(world: &mut World, fov_y_deg: f32) {
    let mut q = world.query::<&mut Camera>();
    for mut camera in q.iter_mut(world) {
        camera.projection = Projection::Perspective {
            fov_y_deg,
            near: 0.05,
        };
    }
}

fn shot(app: &mut App, path: &str, eye: DVec3, target: DVec3) {
    aim(&mut app.world, eye, target);
    app.update_n(7);
    app.world
        .resource_mut::<Screenshots>()
        .request(path.to_string());
    app.update();
    println!("saved {path}");
}

/// A deterministic scatter of `n` points in a disc of `radius` around `c` (x, z).
fn scatter(seed: u64, c: [f64; 2], radius: f32, spacing: f32) -> Vec<DVec3> {
    let mut rng = Rng::new(seed);
    let mut out: Vec<DVec3> = Vec::new();
    let tries = ((radius / spacing).powi(2) * 6.0) as usize;
    for _ in 0..tries {
        let p = rng.in_unit_disk() * radius;
        let at = DVec3::new(c[0] + p.x as f64, 0.0, c[1] + p.y as f64);
        if out.iter().all(|q| q.distance(at) > spacing as f64) {
            out.push(at);
        }
    }
    out
}

fn plant(world: &mut World, species: &str, at: DVec3, size: f32) {
    spawn_plant(world, species, at, size, None).expect("species");
}

fn cave_walls(commands: &mut Commands, c: DVec3) {
    let rock = StandardMaterial::hex(0x3a3634).with_roughness(0.9);
    let (w, h, d) = (16.0f32, 6.0f32, 16.0f32);
    let mut wall = |size: [f32; 3], at: DVec3| {
        commands.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(Shape::Cuboid { size }, rock),
        ));
    };
    let (hw, hh, hd) = (w as f64 * 0.5, h as f64 * 0.5, d as f64 * 0.5);
    wall([w, h, 1.0], c + DVec3::new(0.0, hh, hd));
    wall([w, h, 1.0], c + DVec3::new(0.0, hh, -hd));
    wall([1.0, h, d], c + DVec3::new(-hw, hh, 0.0));
    wall([1.0, h, d], c + DVec3::new(hw, hh, 0.0));
    wall([w + 2.0, 1.0, d + 2.0], c + DVec3::new(0.0, h as f64, 0.0));
    commands.spawn((
        Transform::from_translation(c + DVec3::new(0.0, hh, 0.0)),
        SkyLightVolume::new([w * 0.5, h * 0.5, d * 0.5]),
    ));
    // Damp, dark cave floor.
    commands.spawn((
        Transform::from_translation(c + DVec3::Y * 0.002),
        MeshRenderer::new(
            Shape::Plane {
                size: [w, d],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x2a2725).with_roughness(0.7),
        ),
    ));
}

fn print_triangle_counts(library: &PlantLibrary) {
    for name in [
        "grass",
        "fern",
        "wildflowers",
        "bluebell",
        "glowcap",
        "lantern cap",
    ] {
        let sp = library.get(library.id(name).expect(name));
        let counts: Vec<String> = (0..4u64)
            .map(|v| {
                let m = small(sp, 0x51 ^ (v * 0x9e37_79b9));
                let tri = |m: &Option<engine_render::Mesh>| {
                    m.as_ref().map_or(0, engine_render::Mesh::triangle_count)
                };
                format!(
                    "{}+{} far {}",
                    tri(&m.foliage),
                    tri(&m.wood),
                    tri(&m.foliage_lod)
                )
            })
            .collect();
        println!("triangles {name:12} (foliage+stems, far mesh per variant): {counts:?}");
    }
}

/// The same plants twice, side by side: near meshes on the left, far meshes on the right. Seen
/// from the switch distance, the two patches should match in color and silhouette.
fn lod_compare(world: &mut World, at: DVec3) {
    let mut parts = Vec::new();
    {
        let library = world.resource::<PlantLibrary>();
        let visuals = world.resource::<FoliageVisuals>();
        for (k, (name, spacing)) in [
            ("grass", 0.8),
            ("wildflowers", 1.5),
            ("fern", 1.4),
            ("bluebell", 0.7),
        ]
        .into_iter()
        .enumerate()
        {
            let Some(v) = library.id(name).and_then(|id| visuals.get(id)) else {
                continue;
            };
            for (i, p) in scatter(10 + k as u64, [0.0, 0.0], 2.4, spacing)
                .into_iter()
                .enumerate()
            {
                let var = &v.variants[i % v.variants.len()];
                let (Some(near), Some(far)) = (&var.foliage, &var.foliage_lod) else {
                    continue;
                };
                let leaves = v.leaves[0];
                parts.push((at + p - DVec3::X * 2.7, *near, leaves));
                parts.push((at + p + DVec3::X * 2.7, *far, leaves));
            }
        }
    }
    for (pos, mesh, material) in parts {
        let mut r = MeshRenderer::new(
            Shape::Cuboid { size: [0.01; 3] },
            StandardMaterial::default(),
        );
        r.mesh = engine_render::MeshRef::Handle(mesh);
        r.material = engine_render::MaterialRef::Handle(material);
        r.cast_shadows = false;
        world.spawn((Transform::from_translation(pos), r));
    }
}

/// Save the small-plant textures (albedo, normal, emission) as PNGs for inspection.
fn dump_textures(out: &str) {
    use engine_foliage::meshgen::small_textures::{Fungus, fern_pinna, fungus, meadow_atlas};
    let save = |name: &str, img: &engine_render::Image| {
        let path = format!("{out}/tex_{name}.png");
        image::save_buffer(
            &path,
            &img.data,
            img.width,
            img.height,
            image::ExtendedColorType::Rgba8,
        )
        .expect("png");
        println!("saved {path}");
    };
    for (name, set) in [
        ("meadow", meadow_atlas(512)),
        ("fern", fern_pinna(256)),
        ("glowcap", fungus(Fungus::Glowcap, 256)),
        ("lantern", fungus(Fungus::Lantern, 256)),
    ] {
        save(&format!("{name}_albedo"), &set.albedo);
        save(&format!("{name}_normal"), &set.normal);
        if let Some(e) = &set.emissive {
            save(&format!("{name}_emissive"), e);
        }
    }
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_small".into());
    let tag = args.next().unwrap_or_else(|| "after".into());
    std::fs::create_dir_all(&out).expect("output directory");
    if std::env::var_os("LOOKDEV_TEXTURES").is_some() {
        dump_textures(&out);
    }

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 640;
        w.height = 360;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);
    print_triangle_counts(app.world.resource::<PlantLibrary>());

    app.world.resource_mut::<Atmosphere>().ground_color = [0.08, 0.12, 0.05];
    // Bare soil: the plants have to cover it.
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4a3b2a).with_roughness(0.95),
        ),
    ));
    app.world.spawn((
        DirectionalLight {
            illuminance: SUN_LUX,
            ..Default::default()
        },
        Transform::default(),
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 50.0,
                near: 0.05,
            },
            exposure: Exposure::Auto {
                min_ev100: -3.0,
                max_ev100: 18.0,
                speed: 40.0,
                compensation: 0.0,
            },
            ..Default::default()
        },
        Transform::default(),
    ));

    // ---- the daylight patch around the origin (camera looks toward −Z)
    let w = &mut app.world;
    // Meadow: grass clumps at the ecology's natural spacing, wildflowers mixed in.
    for (i, at) in scatter(1, [0.0, -6.0], 9.0, 0.8).into_iter().enumerate() {
        plant(w, "grass", at, 0.75 + 0.25 * ((i * 7 % 10) as f32 / 10.0));
    }
    for at in scatter(2, [1.5, -4.0], 5.0, 1.6) {
        plant(w, "wildflowers", at + DVec3::new(0.3, 0.0, 0.2), 0.9);
    }
    // A fern bed on the left, bluebells under it toward the camera.
    for at in scatter(3, [-3.2, -3.0], 2.4, 1.2) {
        plant(w, "fern", at, 0.95);
    }
    for at in scatter(4, [-1.4, -0.6], 1.6, 0.55) {
        plant(w, "bluebell", at, 0.95);
    }
    // Close-up subjects right in front of the camera.
    plant(w, "fern", DVec3::new(-0.9, 0.0, 0.2), 1.0);
    plant(w, "wildflowers", DVec3::new(0.35, 0.0, 0.9), 1.0);
    plant(w, "bluebell", DVec3::new(-0.2, 0.0, 1.2), 1.0);
    plant(w, "grass", DVec3::new(0.9, 0.0, 0.6), 1.0);

    // ---- the cave
    {
        let mut commands = app.world.commands();
        cave_walls(&mut commands, CAVE);
    }
    app.world.flush();
    let w = &mut app.world;
    for (i, at) in scatter(5, [CAVE.x - 0.5, CAVE.z - 1.0], 3.2, 0.6)
        .into_iter()
        .enumerate()
    {
        plant(w, "glowcap", at, 0.8 + 0.2 * ((i * 3 % 5) as f32 / 5.0));
    }
    for at in [
        DVec3::new(-2.6, 0.0, -3.2),
        DVec3::new(1.8, 0.0, -3.8),
        DVec3::new(3.6, 0.0, -1.0),
    ] {
        plant(w, "lantern cap", CAVE + at, 1.0);
    }
    plant(w, "glowcap", CAVE + DVec3::new(0.2, 0.0, 2.6), 1.0);

    // Late morning, from behind the camera's left shoulder.
    let sun = Vec3::new(-0.55, 0.62, 0.5).normalize();
    set_sun(&mut app.world, sun, SUN_LUX);
    app.update();

    let file = |name: &str| format!("{out}/{tag}_{name}.png");
    shot(
        &mut app,
        &file("1_meadow"),
        DVec3::new(0.4, 1.7, 4.5),
        DVec3::new(-0.6, 0.4, -4.0),
    );
    shot(
        &mut app,
        &file("2_closeup"),
        DVec3::new(0.1, 0.75, 2.6),
        DVec3::new(-0.3, 0.2, 0.6),
    );
    shot(
        &mut app,
        &file("5_flowers_macro"),
        DVec3::new(0.75, 0.42, 1.75),
        DVec3::new(0.15, 0.22, 0.95),
    );
    // Near (left) and far (right) meshes from the 22 m switch distance, far enough from the
    // meadow that its plants are culled.
    lod_compare(&mut app.world, LOD_PATCH);
    shot(
        &mut app,
        &file("6_lod_22m"),
        LOD_PATCH + DVec3::new(0.0, 1.7, 22.0),
        LOD_PATCH + DVec3::new(0.0, 0.2, 0.0),
    );
    // The same through a telephoto lens: a stricter look than any player gets.
    set_fov(&mut app.world, 12.0);
    shot(
        &mut app,
        &file("7_lod_22m_zoom"),
        LOD_PATCH + DVec3::new(0.0, 1.7, 22.0),
        LOD_PATCH + DVec3::new(0.0, 0.3, 0.0),
    );
    set_fov(&mut app.world, 50.0);
    // Every fern variant in a row, seen from above: last year's dead fronds lie brown among
    // the living ones.
    let row = DVec3::new(-60.0, 0.0, 0.0);
    for v in 0..engine_foliage::assets::VARIANTS as u8 {
        let mut genes = engine_foliage::Genes::random(&mut Rng::new(v as u64 + 7));
        genes.variant = v;
        spawn_plant(
            &mut app.world,
            "fern",
            row + DVec3::new(v as f64 * 1.6 - 2.4, 0.0, 0.0),
            1.0,
            Some(genes),
        )
        .expect("fern");
    }
    app.update();
    shot(
        &mut app,
        &file("8_dead_fronds"),
        row + DVec3::new(0.0, 3.2, 2.6),
        row + DVec3::new(0.0, 0.0, -0.2),
    );
    set_sun(&mut app.world, sun, 0.0);
    shot(
        &mut app,
        &file("3_cave"),
        CAVE + DVec3::new(0.0, 1.6, 5.0),
        CAVE + DVec3::new(0.0, 0.6, -1.5),
    );
    shot(
        &mut app,
        &file("4_cave_close"),
        CAVE + DVec3::new(0.35, 0.24, 3.55),
        CAVE + DVec3::new(0.15, 0.24, 2.4),
    );
}
