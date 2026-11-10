//! The mesh pool's vertex and index buffers start small and grow as meshes and their levels of
//! detail arrive (`mesh_pool`): a field of stones uploaded a few kinds at a time is drawn the
//! same as with whole pages made at once and index buffers as large as the vertex buffers,
//! which never need to grow, and takes less video memory. Two skinned posts stand among the
//! stones: their posed vertices live in the vertex buffer that is replaced, one bent before
//! the stones arrive and left alone, one bent again after. Needs a GPU or lavapipe; passes
//! with a note when neither is available.

use std::sync::Arc;

use engine_assets::Assets;
use engine_core::diagnostics::Diagnostics;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::rock::rock;
use engine_render::{
    GpuSkin, Mesh, RenderDisabled, RenderStats, SkinBlend, SkinBone, SkinWeights, capture_now,
    mesh, set_mesh_index_share, set_mesh_vertex_share,
};

const W: u32 = 480;
const H: u32 = 270;
/// Kinds of stone (20k triangles each), spawned [`KINDS_A_STEP`] at a time.
const KINDS: u32 = 24;
const KINDS_A_STEP: u32 = 6;

struct Run {
    pixels: Vec<u8>,
    index_mb: f64,
    index_used_mb: f64,
    growths: f64,
    vertex_mb: f64,
    vertex_used_mb: f64,
    vertex_growths: f64,
    refused: f64,
}

/// A 0.3 m square post 2 m tall: the lower meter on bone 0, the upper on bone 1.
fn post() -> (Mesh, Arc<SkinWeights>) {
    let mut post = Mesh::new(Vec::new(), Vec::new());
    let mut weights = SkinWeights::default();
    for (bone, y) in [(0u16, 0.5f32), (1, 1.5)] {
        let mut part = mesh::cuboid(Vec3::new(0.3, 1.0, 0.3));
        part.transform(&Mat4::from_translation(Vec3::new(0.0, y, 0.0)));
        for _ in &part.vertices {
            weights.joints.push([bone, 0, 0, 0]);
            weights.weights.push([1.0, 0.0, 0.0, 0.0]);
        }
        post.merge(&part);
    }
    (post, Arc::new(weights))
}

/// The post's bones with the upper one turned `angle` about +Z at the joint.
fn bent(angle: f32) -> Vec<SkinBone> {
    let joint = (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0));
    vec![
        SkinBone::IDENTITY,
        SkinBone::from_poses(joint, (Quat::from_rotation_z(angle), joint.1)),
    ]
}

/// The stone field drawn with new pool pages getting `share` index bytes a vertex byte and
/// vertex buffers that start at `vertex_share` of their page.
fn render(share: f64, vertex_share: f64, shot: &str) -> Option<Run> {
    engine_core::log::init("error");
    set_mesh_index_share(share);
    set_mesh_vertex_share(vertex_share);
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        // Whole-second steps keep the post dither the same however many frames a run needs.
        let mut time = app.world.resource_mut::<Time>();
        time.set_max_delta(1.0);
        time.set_source(engine_core::time::TimeSource::Manual { step_secs: 1.0 });
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        // Every stone through the pool, none as clusters.
        s.virtual_geometry = false;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
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
    // The posts, among the first meshes in the pool: the left one bent now and never again,
    // the right one bent once the stones are in.
    let red = StandardMaterial::color(0.8, 0.1, 0.1);
    let mut posts = Vec::new();
    for x in [-1.6, 1.6] {
        let (post, weights) = post();
        let handle = w.resource_mut::<Assets<Mesh>>().add(post);
        posts.push(
            w.spawn((
                Transform::from_xyz(x, 0.0, -5.0),
                MeshRenderer::new(handle, red),
                GpuSkin {
                    weights,
                    bones: bent(if x < 0.0 { 1.2 } else { 0.0 }),
                    blend: SkinBlend::DualQuaternion,
                },
            ))
            .id(),
        );
    }
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    app.update_n(3);
    // A few kinds at a time, with frames between: later kinds arrive when the index buffer
    // already holds earlier ones and their levels of detail.
    let material = StandardMaterial::color(0.45, 0.42, 0.38);
    for step in 0..KINDS / KINDS_A_STEP {
        let w = &mut app.world;
        for kind in step * KINDS_A_STEP..(step + 1) * KINDS_A_STEP {
            let stone =
                w.resource_mut::<Assets<Mesh>>()
                    .add(rock(Vec3::new(0.5, 0.3, 0.4), kind, 5));
            // Each kind near, in the middle distance and far.
            for (row, z) in [-4.0, -14.0, -45.0].into_iter().enumerate() {
                let spread = 0.45 + row as f64 * 1.3;
                let x = (kind as f64 - (KINDS - 1) as f64 * 0.5) * spread;
                w.spawn((
                    Transform::from_xyz(x, 0.1, z - (kind % 3) as f64 * 1.5),
                    MeshRenderer::new(stone, material),
                ));
            }
        }
        app.update_n(3);
    }
    if let Some(mut skin) = app.world.get_mut::<GpuSkin>(posts[1]) {
        skin.bones = bent(-1.2);
    }
    // Until the levels are built (the triangle count stops falling) and none is fading in.
    let (mut last, mut steady) = (u64::MAX, 0);
    let start = std::time::Instant::now();
    while steady < 4 && start.elapsed().as_secs() < 180 {
        app.update();
        let tris = app
            .world
            .get_resource::<RenderStats>()
            .map_or(0, |s| s.triangles);
        let fading = describe(&app)["lods_fading"].as_u64().unwrap_or(0);
        steady = if tris == last && fading == 0 {
            steady + 1
        } else {
            0
        };
        last = tris;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let d = app.world.resource::<Diagnostics>();
    let get = |name: &str| d.get(name).unwrap_or(-1.0);
    Some(Run {
        pixels: image::open(&path).expect("png").to_rgb8().into_raw(),
        index_mb: get("render.meshes.pool_index_mb"),
        index_used_mb: get("render.meshes.pool_index_used_mb"),
        growths: get("render.meshes.pool_index_growths"),
        vertex_mb: get("render.meshes.pool_vertex_mb"),
        vertex_used_mb: get("render.meshes.pool_vertex_used_mb"),
        vertex_growths: get("render.meshes.pool_vertex_growths"),
        refused: get("render.meshes.pool_lods_refused"),
    })
}

#[test]
fn buffers_that_grow_draw_the_same_stones_and_posts_in_less_memory() {
    // Whole pages at once with index buffers as large as the vertex buffers, as they used to
    // be: nothing grows.
    let Some(full) = render(1.0, 1.0, "pool_full.png") else {
        return;
    };
    // An eighth, the default: they grow as the stones and their levels arrive.
    let Some(grown) = render(0.125, 0.125, "pool_grown.png") else {
        return;
    };
    eprintln!(
        "index buffers: {:.1} MB ({:.1} used) without growing, {:.1} MB ({:.1} used) after {} \
         growths",
        full.index_mb, full.index_used_mb, grown.index_mb, grown.index_used_mb, grown.growths
    );
    eprintln!(
        "vertex buffers: {:.1} MB ({:.1} used) without growing, {:.1} MB ({:.1} used) after {} \
         growths",
        full.vertex_mb,
        full.vertex_used_mb,
        grown.vertex_mb,
        grown.vertex_used_mb,
        grown.vertex_growths
    );
    let (mut sum, mut worst, mut off) = (0u64, 0u8, 0u32);
    for (a, b) in full.pixels.iter().zip(&grown.pixels) {
        let d = a.abs_diff(*b);
        sum += d as u64;
        worst = worst.max(d);
        off += (d > 12) as u32;
    }
    let mean = sum as f32 / full.pixels.len() as f32;
    eprintln!("pictures: mean difference {mean:.4} of 255, worst {worst}, {off} values over 12");
    assert_eq!(full.growths, 0.0);
    assert!(grown.growths >= 2.0, "{} growths", grown.growths);
    assert_eq!((full.refused, grown.refused), (0.0, 0.0));
    // The same meshes and levels in both.
    assert!((full.index_used_mb - grown.index_used_mb).abs() < 0.01);
    // Grown a quarter at a time: little more than is used, and well under the full buffers.
    assert!(
        grown.index_mb < grown.index_used_mb * 1.3 + 1.0,
        "{} MB for {} MB of indices",
        grown.index_mb,
        grown.index_used_mb
    );
    assert!(grown.index_mb < full.index_mb * 0.6);
    // The vertex buffers likewise.
    assert_eq!(full.vertex_growths, 0.0);
    assert!(
        grown.vertex_growths >= 2.0,
        "{} growths",
        grown.vertex_growths
    );
    assert!((full.vertex_used_mb - grown.vertex_used_mb).abs() < 0.01);
    assert!(
        grown.vertex_mb < grown.vertex_used_mb * 1.3 + 1.0,
        "{} MB for {} MB of vertices",
        grown.vertex_mb,
        grown.vertex_used_mb
    );
    assert!(grown.vertex_mb < full.vertex_mb * 0.8);

    // Two runs of one build differ by a mean of 0.002 with a few edge pixels of up to 37 (which
    // frame each stone's levels arrive in); a stone drawn from the wrong indices or a post
    // left in an old pose would change hundreds of pixels.
    assert!(mean < 0.05, "the pictures differ: mean {mean}");
    assert!(off < 150, "{off} values differ by more than 12 of 255");
}

/// The mesh feature's state.
fn describe(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<engine_render::Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .map(|f| f["state"].clone())
        .unwrap_or_default()
}
