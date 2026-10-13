//! Procedural art for the UI demos: the showcase's market street and the gallery's backdrop.
//!
//! Everything is generated in code (no asset files): noise, PBR texture sets baked from
//! per-texel functions (albedo, tangent-space normal from a height field, and occlusion /
//! roughness / metal), and meshes with metric UVs and rounded edges.

pub mod mesh;
pub mod noise;
pub mod props;
pub mod stall;
pub mod textures;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{Mesh, Vertex};

/// Run `f(i)` for `i` in `0..n` on all cores; results in order.
pub fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    let chunk = n.div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let jobs: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| {
                s.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        jobs.into_iter()
            .flat_map(|j| j.join().expect("art worker"))
            .collect()
    })
}

pub fn sat(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

pub fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = sat((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

pub fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// sRGB hex to linear RGB.
pub fn hex(rgb: u32) -> Vec3 {
    Vec3::from(engine_render::material::srgb_hex_to_linear(rgb))
}

pub fn rgba(c: Vec3) -> Vec4 {
    c.extend(1.0)
}

pub fn vtx(p: Vec3, n: Vec3, uv: Vec2, c: Vec4) -> Vertex {
    Vertex {
        position: p.to_array(),
        normal: n.to_array(),
        uv: uv.to_array(),
        color: c.to_array(),
    }
}

/// Flip triangles whose winding disagrees with their vertex normals.
pub fn fix_winding(m: &mut Mesh) {
    let verts = &m.vertices;
    for tri in m.indices.as_chunks_mut::<3>().0 {
        let p = |i: u32| Vec3::from(verts[i as usize].position);
        let n = |i: u32| Vec3::from(verts[i as usize].normal);
        let g = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
        if g.dot(n(tri[0]) + n(tri[1]) + n(tri[2])) < 0.0 {
            tri.swap(1, 2);
        }
    }
}

/// Multiply every vertex color by `c`.
pub fn tint(mut m: Mesh, c: Vec4) -> Mesh {
    for v in &mut m.vertices {
        v.color = (Vec4::from(v.color) * c).to_array();
    }
    m
}

pub fn moved(mut m: Mesh, t: Mat4) -> Mesh {
    m.transform(&t);
    m
}

pub fn add_mesh(world: &mut World, m: Mesh) -> Handle<Mesh> {
    world.resource_mut::<Assets<Mesh>>().add(m)
}

pub fn add_mat(world: &mut World, m: StandardMaterial) -> Handle<StandardMaterial> {
    world.resource_mut::<Assets<StandardMaterial>>().add(m)
}

pub fn add_image(world: &mut World, i: Image) -> Handle<Image> {
    world.resource_mut::<Assets<Image>>().add(i)
}

/// A texture set: albedo (sRGB), tangent-space normal and ORM (R occlusion, G roughness,
/// B metal), all the same size.
#[derive(Clone, Copy, Debug)]
pub struct TexSet {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub orm: Handle<Image>,
}

/// One texel of a material: linear albedo, height (m), roughness, metal, occlusion, alpha.
#[derive(Clone, Copy, Debug)]
pub struct Texel {
    pub albedo: Vec3,
    pub height: f32,
    pub rough: f32,
    pub metal: f32,
    pub ao: f32,
    pub alpha: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            albedo: Vec3::splat(0.5),
            height: 0.0,
            rough: 0.8,
            metal: 0.0,
            ao: 1.0,
            alpha: 1.0,
        }
    }
}

fn linear_to_srgb8(v: f32) -> u8 {
    let v = sat(v);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm8(v: f32) -> u8 {
    (sat(v) * 255.0 + 0.5) as u8
}

/// Build a texture set `w`×`h` texels covering `size_m` meters horizontally; `f(u, v)` gets
/// texel coordinates in `[0, 1)`. Normals come from the height (meters), wrapping at the
/// edges when `tile` is set.
pub fn bake_set(
    world: &mut World,
    w: u32,
    h: u32,
    size_m: f32,
    tile: bool,
    normal_strength: f32,
    f: impl Fn(f32, f32) -> Texel + Sync,
) -> TexSet {
    let texels: Vec<Vec<Texel>> = par_map(h as usize, |y| {
        (0..w)
            .map(|x| f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32))
            .collect()
    });
    let at = |x: i32, y: i32| -> f32 {
        let (x, y) = if tile {
            (x.rem_euclid(w as i32), y.rem_euclid(h as i32))
        } else {
            (x.clamp(0, w as i32 - 1), y.clamp(0, h as i32 - 1))
        };
        texels[y as usize][x as usize].height
    };
    let texel_m = size_m / w as f32;
    let mut albedo = Vec::with_capacity((w * h * 4) as usize);
    let mut normal = Vec::with_capacity((w * h * 4) as usize);
    let mut orm = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let t = texels[y as usize][x as usize];
            albedo.extend_from_slice(&[
                linear_to_srgb8(t.albedo.x),
                linear_to_srgb8(t.albedo.y),
                linear_to_srgb8(t.albedo.z),
                unorm8(t.alpha),
            ]);
            let du = (at(x + 1, y) - at(x - 1, y)) / (2.0 * texel_m) * normal_strength;
            let dv = (at(x, y + 1) - at(x, y - 1)) / (2.0 * texel_m) * normal_strength;
            let n = Vec3::new(-du, dv, 1.0).normalize();
            normal.extend_from_slice(&[
                unorm8(n.x * 0.5 + 0.5),
                unorm8(n.y * 0.5 + 0.5),
                unorm8(n.z * 0.5 + 0.5),
                255,
            ]);
            orm.extend_from_slice(&[unorm8(t.ao), unorm8(t.rough), unorm8(t.metal), 255]);
        }
    }
    TexSet {
        albedo: add_image(world, Image::new(w, h, albedo, true)),
        normal: add_image(world, Image::new(w, h, normal, false)),
        orm: add_image(world, Image::new(w, h, orm, false)),
    }
}

/// A material whose texture set drives color, normals, roughness, metal and occlusion, with
/// `uv_scale` repeats per meter.
pub fn textured(set: &TexSet, uv_scale: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 1.0,
        metallic: 1.0,
        base_color_texture: Some(set.albedo),
        normal_texture: Some(set.normal),
        metallic_roughness_texture: Some(set.orm),
        occlusion_texture: Some(set.orm),
        uv_scale: [uv_scale, uv_scale],
        ..Default::default()
    }
}

/// An emissive lamp material (glass, windows, beacons).
pub fn lamp(c: [f32; 3], strength: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [c[0], c[1], c[2], 1.0],
        emissive: c,
        emissive_strength: strength,
        perceptual_roughness: 0.2,
        ..Default::default()
    }
}
