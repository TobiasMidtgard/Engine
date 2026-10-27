//! Levels of detail of instanced meshes: a field of dense stones reaching into the distance is
//! drawn with far fewer triangles once their levels are built, in the view and in the shadow
//! maps, and looks the same; stones marked `NoMeshLod` keep every triangle. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::rock::rock;
use engine_render::{Mesh, NoMeshLod, RenderDisabled, RenderStats, capture_now};

const W: u32 = 320;
const H: u32 = 180;

#[derive(Clone, Copy, PartialEq)]
enum Lods {
    Off,
    On,
    /// On, with every stone marked `NoMeshLod` (and nothing culled or drawn as an impostor).
    Marked,
}

/// Render the stone field: the triangles drawn and the image's pixels.
fn render(lods: Lods, shot: &str) -> Option<(u64, Vec<u8>)> {
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
        // Stones under the virtual geometry threshold stay on the instanced path anyway.
        s.virtual_geometry = false;
        if lods == Lods::Off {
            s.mesh_lod_pixels = 0.0;
        }
        if lods != Lods::On {
            s.min_object_pixels = 0.0;
            s.impostor_pixels = 0.0;
        }
    }
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 1.7, 0.0).looking_at(DVec3::new(0.0, 0.0, -40.0), Vec3::Y),
    ));
    let e = 35f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.6 * e.cos(), -e.sin(), -0.8 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.25, 0.3, 0.2),
        ),
    ));
    // Four kinds of stone, 20k triangles each, from 3 m to 120 m away.
    let stones: Vec<_> = (0..4)
        .map(|seed| {
            w.resource_mut::<Assets<Mesh>>()
                .add(rock(Vec3::new(0.5, 0.3, 0.4), seed, 5))
        })
        .collect();
    let material = StandardMaterial::color(0.45, 0.42, 0.38);
    for i in 0..600u32 {
        let row = (i / 20) as f64;
        let col = (i % 20) as f64 - 9.5;
        let z = -3.0 - row * row * 0.13 - row;
        let x = col * (1.0 + row * 0.25);
        let stone = w
            .spawn((
                Transform::from_xyz(x, 0.1, z),
                MeshRenderer::new(stones[(i % 4) as usize], material),
            ))
            .id();
        if lods == Lods::Marked {
            w.entity_mut(stone).insert(NoMeshLod);
        }
    }
    let mut last = u64::MAX;
    let start = std::time::Instant::now();
    // Until the levels are built (the triangle count stops falling).
    let mut steady = 0;
    while steady < 3 && start.elapsed().as_secs() < 120 {
        app.update();
        if app.world.get_resource::<RenderDisabled>().is_some() {
            break;
        }
        let tris = app
            .world
            .get_resource::<RenderStats>()
            .map_or(0, |s| s.triangles);
        steady = if tris == last { steady + 1 } else { 0 };
        last = tris;
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-mesh-lod");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    let image = image::open(&path).expect("read the shot").to_rgb8();
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let tris = app.world.resource::<RenderStats>().triangles;
    Some((tris, image.into_raw()))
}

#[test]
fn far_stones_draw_coarser_levels_and_look_the_same() {
    let Some((full, before)) = render(Lods::Off, "stones_full.png") else {
        return;
    };
    let Some((lod, after)) = render(Lods::On, "stones_lod.png") else {
        return;
    };
    eprintln!("triangles in view: {full} full, {lod} with levels of detail");
    assert!(lod * 4 < full, "{lod} of {full} triangles");
    let diff = before
        .iter()
        .zip(&after)
        .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
        .sum::<u32>() as f32
        / before.len() as f32;
    eprintln!("mean difference {diff:.2} of 255");
    // Shading of the coarser stones moves by a level or two at their rims, nothing more.
    assert!(
        diff < 3.0,
        "the images differ by {diff:.2} of 255 on average"
    );
}

#[test]
fn stones_marked_no_mesh_lod_keep_every_triangle() {
    let Some((full, before)) = render(Lods::Off, "stones_full_again.png") else {
        return;
    };
    let Some((marked, after)) = render(Lods::Marked, "stones_marked.png") else {
        return;
    };
    assert_eq!(marked, full, "triangles drawn");
    assert_eq!(before, after, "the pictures differ");
}
