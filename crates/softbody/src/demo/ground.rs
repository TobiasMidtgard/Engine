//! A cheap textured ground to review the yard on: packed earth and gravel where the props
//! stand, fraying into a grass field through an alpha-masked edge, with large-scale color
//! variation baked into vertex colors. Two meshes, two draws, no shadows cast.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial, Vertex,
};

use super::kit::{Set, add_set, register_mesh, textured};
use super::texgen::{self, noise3};

const EARTH: &str = "softbody/yard/ground/earth";
const GRASS: &str = "softbody/yard/ground/grass";

/// Make the ground's textures and materials (once).
fn ensure_ground_materials(world: &mut World) {
    if !world.contains_resource::<Assets<Image>>()
        || !world.contains_resource::<Assets<StandardMaterial>>()
    {
        return;
    }
    if world
        .resource::<Assets<StandardMaterial>>()
        .loaded_by_key(EARTH)
        .is_some()
    {
        return;
    }
    let earth = texgen::earth(1024);
    let grass = texgen::grass(1024);
    let mut images = world.resource_mut::<Assets<Image>>();
    let earth: Set = add_set(&mut images, earth);
    let grass: Set = add_set(&mut images, grass);
    let mut mats = world.resource_mut::<Assets<StandardMaterial>>();
    let mut e = textured(earth, [1.0; 3], 1.0, 0.0, 1.0 / 3.0);
    e.alpha_mode = AlphaMode::Mask(0.5);
    mats.insert_keyed(EARTH, e);
    mats.insert_keyed(GRASS, textured(grass, [1.0; 3], 1.0, 0.0, 0.5));
}

/// Large-scale noise (−1..1) at a world position, `scale` meters per feature.
fn macro_noise(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    let p = Vec3::new(x / scale, 0.0, z / scale);
    0.6 * noise3(p, seed) + 0.3 * noise3(p * 2.03, seed + 1) + 0.15 * noise3(p * 4.1, seed + 2)
}

/// A grid on XZ with per-vertex color and alpha from `shade(x, z)`; `xs`/`zs` are the grid
/// coordinates (meters).
fn grid(xs: &[f32], zs: &[f32], y: f32, shade: impl Fn(f32, f32) -> Vec4) -> Mesh {
    let mut m = Mesh::default();
    for &z in zs {
        for &x in xs {
            m.vertices.push(
                Vertex::new(Vec3::new(x, y, z), Vec3::Y, Vec2::new(x, z)).with_color(shade(x, z)),
            );
        }
    }
    let row = xs.len() as u32;
    for j in 0..zs.len() as u32 - 1 {
        for i in 0..row - 1 {
            let a = j * row + i;
            m.indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    m
}

/// Grass field: 1.2 km across, dense near the yard and coarse far away.
fn grass_mesh() -> Mesh {
    let n = 80;
    let (half, k) = (600.0f32, 5.0f32);
    let axis: Vec<f32> = (-n..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            t.signum() * half * ((k * t.abs()).exp() - 1.0) / (k.exp() - 1.0)
        })
        .collect();
    grid(&axis, &axis, 0.0, |x, z| {
        let lush = macro_noise(x, z, 38.0, 31);
        let patch = macro_noise(x, z, 9.0, 37);
        let b = 0.92 + 0.12 * patch;
        let dry = (0.5 - 0.5 * lush).clamp(0.0, 1.0);
        Vec4::new(
            b * (0.95 + 0.2 * dry),
            b * (1.0 + 0.04 * dry),
            b * (0.95 - 0.25 * dry),
            1.0,
        )
    })
}

/// Signed distance (meters, negative inside) to the yard's worn-earth patch.
fn patch_distance(x: f32, z: f32) -> f32 {
    let (cx, cz, hx, hz, r) = (6.5f32, -2.5f32, 27.5f32, 21.5f32, 9.0f32);
    let qx = (x - cx).abs() - (hx - r);
    let qz = (z - cz).abs() - (hz - r);
    let outside = Vec2::new(qx.max(0.0), qz.max(0.0)).length();
    outside + qx.max(qz).min(0.0) - r + 2.6 * macro_noise(x, z, 7.0, 41)
}

/// Packed earth and gravel over the yard, fading out (vertex alpha) into the grass.
fn earth_mesh() -> Mesh {
    let xs: Vec<f32> = (0..=136).map(|i| -27.0 + i as f32 * 0.5).collect();
    let zs: Vec<f32> = (0..=108).map(|i| -29.5 + i as f32 * 0.5).collect();
    let mut m = grid(&xs, &zs, 0.004, |x, z| {
        let d = patch_distance(x, z);
        let alpha = engine_core::math::smoothstep(2.2, -2.8, d);
        let damp = macro_noise(x, z, 11.0, 43);
        let worn = macro_noise(x, z, 4.0, 47);
        let edge = engine_core::math::smoothstep(-6.0, 0.0, d);
        let b = (0.95 + 0.1 * worn - 0.1 * damp.max(0.0)) * (1.0 - 0.12 * edge);
        Vec4::new(b * (1.0 + 0.03 * worn), b, b * (0.97 - 0.03 * damp), alpha)
    });
    // Drop the triangles that are fully transparent.
    let keep: Vec<u32> = m
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t.iter().any(|&i| m.vertices[i as usize].color[3] > 0.45))
        .flatten()
        .copied()
        .collect();
    m.indices = keep;
    m
}

/// Spawn the review ground around `center`: an earth-and-gravel yard fraying into grass.
pub fn spawn_review_ground(commands: &mut Commands, center: DVec3) {
    commands.queue(ensure_ground_materials);
    register_mesh(commands, GRASS.to_string(), grass_mesh());
    register_mesh(commands, EARTH.to_string(), earth_mesh());
    for key in [GRASS, EARTH] {
        commands.spawn((
            Transform::from_translation(center),
            MeshRenderer::new(MeshRef::Asset(key.into()), MaterialRef::Asset(key.into()))
                .without_shadows(),
        ));
    }
}
