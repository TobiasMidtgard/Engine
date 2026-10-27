//! Procedural meshes for the fire demo's props: grown trees (trunk with root flare, branching
//! limbs, leaf cards), a timber shed (frame, plank walls with gaps, a door, a corrugated roof),
//! round hay bales with net wrap, and a campfire of shaped stones and split logs.
//!
//! Everything is smooth-shaded with enough segments and bevels to hold up close, carries
//! texture coordinates in meters for the procedural materials in [`crate::textures`], and
//! bakes ambient occlusion and color variation into vertex colors.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use engine_assets::{Assets, Handle};
use engine_core::math::{Mat4, Quat, Rng, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, Mesh, MeshRenderer, StandardMaterial, Vertex};

use crate::objects::Burning;
use crate::textures::{self, TexSet};

/// Builds a [`Mesh`] piece by piece.
#[derive(Default)]
pub struct MeshBuilder {
    pub mesh: Mesh,
}

impl MeshBuilder {
    pub fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2, color: Vec4) -> u32 {
        self.mesh
            .vertices
            .push(Vertex::new(p, n.normalize_or(Vec3::Y), uv).with_color(color));
        self.mesh.vertices.len() as u32 - 1
    }

    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.mesh.indices.extend_from_slice(&[a, b, c]);
    }

    /// Quad a-b-c-d, counter-clockwise seen from its front.
    pub fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// Append `other` transformed by `m`.
    pub fn add(&mut self, mut other: Mesh, m: Mat4) {
        other.transform(&m);
        self.mesh.merge(&other);
    }

    pub fn finish(self) -> Mesh {
        self.mesh
    }
}

/// A frame perpendicular to `t` (unit), continuing `prev` as smoothly as possible (parallel
/// transport), so tubes don't twist.
fn transport(prev: Vec3, t: Vec3) -> Vec3 {
    let n = prev - t * prev.dot(t);
    if n.length_squared() > 1e-8 {
        n.normalize()
    } else {
        t.any_orthonormal_vector()
    }
}

/// A ring of a [`tube`].
#[derive(Debug, Clone, Copy)]
pub struct Ring {
    pub center: Vec3,
    pub radius: f32,
    /// Vertex color (AO in RGB).
    pub color: Vec4,
}

/// A smooth generalized cylinder through `rings`, `sides` around, open at both ends.
/// Texture u runs around (`u_repeat` times), v along the length in units of `v_tile` meters.
/// `shape(angle, ring)` scales the radius per vertex (root buttresses, lumps).
pub fn tube(
    b: &mut MeshBuilder,
    rings: &[Ring],
    sides: u32,
    u_repeat: f32,
    v_tile: f32,
    v0: f32,
    shape: &dyn Fn(f32, usize) -> f32,
) {
    if rings.len() < 2 {
        return;
    }
    let base = b.mesh.vertices.len() as u32;
    let mut normal = Vec3::ZERO;
    let mut v = v0;
    for (i, r) in rings.iter().enumerate() {
        let next = rings[(i + 1).min(rings.len() - 1)].center;
        let prev = rings[i.saturating_sub(1)].center;
        let t = (next - prev).normalize_or(Vec3::Y);
        normal = if i == 0 {
            transport(Vec3::X, t)
        } else {
            transport(normal, t)
        };
        let binormal = t.cross(normal);
        if i > 0 {
            v += rings[i].center.distance(rings[i - 1].center) / v_tile;
        }
        // Taper tilts the surface normal along the tube.
        let dr = (rings[(i + 1).min(rings.len() - 1)].radius - rings[i.saturating_sub(1)].radius)
            / (next - prev).length().max(1e-4);
        for s in 0..=sides {
            let a = s as f32 / sides as f32 * TAU;
            let radial = normal * a.cos() + binormal * a.sin();
            let k = shape(a, i);
            let p = r.center + radial * r.radius * k;
            let n = (radial - t * dr).normalize_or(radial);
            b.vertex(
                p,
                n,
                Vec2::new(s as f32 / sides as f32 * u_repeat, v),
                r.color,
            );
        }
    }
    let row = sides + 1;
    for i in 0..rings.len() as u32 - 1 {
        for s in 0..sides {
            let a = base + i * row + s;
            b.quad(a, a + 1, a + row + 1, a + row);
        }
    }
}

/// A box with rounded edges (radius `bevel`), centered on the origin. `uv(normal, p)` gives
/// texture coordinates for a point on a face.
pub fn rounded_box(size: Vec3, bevel: f32, color: Vec4, uv: &dyn Fn(Vec3, Vec3) -> Vec2) -> Mesh {
    let h = size * 0.5;
    let bevel = bevel.min(h.min_element() * 0.95).max(0.0);
    let inner = h - Vec3::splat(bevel);
    let mut b = MeshBuilder::default();
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    for (n, u, v) in faces {
        let base = b.mesh.vertices.len() as u32;
        let eu = u.abs().dot(h);
        let ev = v.abs().dot(h);
        let iu = u.abs().dot(inner);
        let iv = v.abs().dot(inner);
        let us = [-eu, -iu, -iu * 0.5, iu * 0.5, iu, eu];
        let vs = [-ev, -iv, -iv * 0.5, iv * 0.5, iv, ev];
        for &cv in &vs {
            for &cu in &us {
                let p = n * n.abs().dot(h) + u * cu + v * cv;
                let c = p.clamp(-inner, inner);
                let d = (p - c).normalize_or(n);
                let q = c + d * bevel;
                b.vertex(q, d, uv(n, q), color);
            }
        }
        let row = us.len() as u32;
        for j in 0..vs.len() as u32 - 1 {
            for i in 0..row - 1 {
                let a = base + j * row + i;
                b.quad(a, a + 1, a + row + 1, a + row);
            }
        }
    }
    b.finish()
}

// ---------------------------------------------------------------------------------- trees

/// Size and seed of a grown tree.
#[derive(Debug, Clone, Copy)]
pub struct TreeParams {
    /// Total height, meters.
    pub height: f32,
    /// Height of the lowest main limbs, meters.
    pub crown_base: f32,
    /// Crown radius, meters.
    pub crown_radius: f32,
    /// Trunk radius at breast height, meters.
    pub trunk_radius: f32,
    pub seed: u64,
}

/// A grown tree: bark-covered wood and leaf cards (each in its own local space: the wood's
/// origin is the trunk's foot, the leaves' origin is `crown_center` relative to the foot).
pub struct TreeMesh {
    pub wood: Mesh,
    pub leaves: Mesh,
    /// Center of the leaves relative to the trunk's foot.
    pub crown_center: Vec3,
}

struct Limb {
    rings: Vec<Ring>,
    level: u32,
    sides: u32,
}

struct LeafSpot {
    pos: Vec3,
    dir: Vec3,
}

/// Bark texture tile, meters.
const BARK_TILE: f32 = 1.0;

/// Grow a broadleaf tree (oak-like): a buttressed trunk that forks into a few main limbs,
/// each branching twice more into twigs carrying leaf cards that fill an ellipsoid crown.
pub fn grow_tree(p: &TreeParams) -> TreeMesh {
    let mut rng = Rng::new(p.seed);
    let center = Vec3::new(0.0, p.crown_base + (p.height - p.crown_base) * 0.52, 0.0);
    let radii = Vec3::new(
        p.crown_radius,
        (p.height - p.crown_base) * 0.55,
        p.crown_radius,
    );
    let mut limbs: Vec<Limb> = Vec::new();
    let mut spots: Vec<LeafSpot> = Vec::new();

    // Trunk: from below the ground to the crown, leaning a little, the leader fading out
    // into the crown.
    let lean = Vec3::new(rng.range(-0.08, 0.08), 1.0, rng.range(-0.08, 0.08)).normalize();
    let trunk_len = p.crown_base + (p.height - p.crown_base) * 0.55;
    let mut trunk = Vec::new();
    let mut pos = Vec3::new(0.0, -0.25, 0.0);
    let mut dir = lean;
    let steps = (trunk_len / 0.22).ceil() as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let y = pos.y.max(0.0);
        let flare = 1.0 + 0.85 * (-y / 0.35).exp();
        let r = p.trunk_radius * (1.0 - 0.55 * t.powf(1.3)) * flare;
        let ao = 0.55 + 0.45 * (y / 1.2).min(1.0);
        trunk.push(Ring {
            center: pos,
            radius: r,
            color: Vec4::new(ao, ao, ao, 1.0),
        });
        let wiggle = Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)) * 0.05;
        dir = (dir + wiggle).normalize();
        pos += dir * (trunk_len / steps as f32);
    }
    let trunk_top = trunk.last().map(|r| r.radius).unwrap_or(0.1);
    limbs.push(Limb {
        rings: trunk.clone(),
        level: 0,
        sides: 20,
    });

    // Main limbs: spiral out of the upper trunk.
    let main = rng.range_i32(5, 8);
    let mut phase = rng.range(0.0, TAU);
    for k in 0..main {
        let t = 0.45 + 0.5 * (k as f32 + rng.range(0.0, 0.5)) / main as f32;
        let i = ((trunk.len() - 1) as f32 * t) as usize;
        let start = trunk[i].center;
        phase += 2.4 + rng.range(-0.3, 0.3);
        let out = Vec3::new(phase.cos(), 0.0, phase.sin());
        let up = rng.range(0.3, 0.85);
        let d = (out + Vec3::Y * up).normalize();
        let len = (p.crown_radius * rng.range(0.95, 1.3)) * (1.15 - 0.4 * t);
        let r0 = trunk[i].radius * rng.range(0.5, 0.65);
        grow_branch(
            &mut rng, start, d, len, r0, 1, center, radii, &mut limbs, &mut spots,
        );
    }
    // The leader continues up as one more limb.
    let top = trunk.last().unwrap().center;
    let leader = (dir + Vec3::new(rng.range(-0.2, 0.2), 0.0, rng.range(-0.2, 0.2))).normalize();
    grow_branch(
        &mut rng,
        top,
        leader,
        (p.height - top.y) * 0.85,
        trunk_top,
        1,
        center,
        radii,
        &mut limbs,
        &mut spots,
    );

    // Wood mesh.
    let mut wood = MeshBuilder::default();
    let lobes = rng.range_i32(4, 7) as f32;
    let lobe_phase = rng.range(0.0, TAU);
    for limb in &limbs {
        let r0 = limb.rings[0].radius;
        let u_repeat = (TAU * r0 / BARK_TILE).round().max(1.0);
        if limb.level == 0 {
            // Root buttresses: lobes that fade out a meter up the trunk.
            let rings = &limb.rings;
            let shape = |a: f32, i: usize| {
                let y = rings[i].center.y.max(0.0);
                let w = (-y / 0.5).exp();
                let l = ((a * lobes + lobe_phase).cos() * 0.5 + 0.5).powf(3.0);
                1.0 + w * (0.55 * l - 0.12)
                    + 0.03 * ((a * 3.0 + y * 2.0).sin() * (a * 7.0 - y).cos())
            };
            tube(
                &mut wood,
                rings,
                limb.sides,
                u_repeat * 2.0,
                BARK_TILE,
                0.0,
                &shape,
            );
        } else {
            let v0 = rng.f32();
            tube(
                &mut wood,
                &limb.rings,
                limb.sides,
                u_repeat,
                BARK_TILE,
                v0,
                &|_, _| 1.0,
            );
            // Cap the twig end.
            let last = limb.rings.last().unwrap();
            let prev = limb.rings[limb.rings.len() - 2].center;
            let tip = last.center + (last.center - prev).normalize_or(Vec3::Y) * last.radius;
            let c = wood.vertex(tip, last.center - prev, Vec2::new(0.5, v0), last.color);
            let n = wood.mesh.vertices.len() as u32;
            let row = limb.sides + 1;
            let first = n - 1 - row;
            for s in 0..limb.sides {
                wood.tri(first + s, first + s + 1, c);
            }
        }
    }

    // Leaf cards: a few around every spot, facing out of the crown.
    let mut leaves = MeshBuilder::default();
    for spot in &spots {
        let cards = rng.range_i32(2, 4);
        for _ in 0..cards {
            let size = rng.range(0.6, 0.95);
            let outward = (spot.pos - center).normalize_or(Vec3::Y);
            // The card's twig leaves the branch outward and a little up.
            let along =
                (spot.dir * 0.6 + outward * 0.8 + Vec3::Y * 0.25 + rng.unit_vector() * 0.45)
                    .normalize();
            let across0 = along
                .cross(outward + rng.unit_vector() * 0.6)
                .normalize_or(along.any_orthonormal_vector());
            // Twist the card around its twig.
            let twist = Quat::from_axis_angle(along, rng.range(-0.7, 0.7));
            let across = twist * across0;
            let face = across.cross(along).normalize();
            let base = spot.pos - center;
            let depth = ((spot.pos - center) / radii).length().min(1.2);
            let ao = 0.4 + 0.6 * depth.min(1.0).powf(1.5);
            let tint = rng.range(0.88, 1.08);
            let alpha = rng.range(0.78, 1.0);
            let color = Vec4::new(
                ao * tint,
                ao * rng.range(0.96, 1.04),
                ao * rng.range(0.78, 0.9),
                alpha,
            );
            let variant = rng.range_i32(0, 4);
            let (u0, v0) = ((variant % 2) as f32 * 0.5, (variant / 2) as f32 * 0.5);
            // Normals: mostly the crown's (soft, volumetric shading), a little the card's.
            let sphere = (spot.pos - center).normalize_or(Vec3::Y);
            let bend = rng.range(0.1, 0.25);
            let corners = [
                (-0.5f32, 0.0f32, Vec2::new(u0, v0 + 0.5)),
                (0.5, 0.0, Vec2::new(u0 + 0.5, v0 + 0.5)),
                (0.5, 1.0, Vec2::new(u0 + 0.5, v0)),
                (-0.5, 1.0, Vec2::new(u0, v0)),
            ];
            let mut idx = [0u32; 4];
            for (k, (x, y, uv)) in corners.iter().enumerate() {
                // Cards droop a little toward their tips.
                let p = base + across * (x * size) + along * (y * size)
                    - Vec3::Y * (y * y * bend * size);
                let card_n = if face.dot(sphere) >= 0.0 { face } else { -face };
                let n = (sphere * 0.7 + card_n * 0.3 + Vec3::Y * 0.15).normalize();
                idx[k] = leaves.vertex(p, n, *uv, color);
            }
            leaves.quad(idx[0], idx[1], idx[2], idx[3]);
        }
    }
    TreeMesh {
        wood: wood.finish(),
        leaves: leaves.finish(),
        crown_center: center,
    }
}

#[allow(clippy::too_many_arguments)]
fn grow_branch(
    rng: &mut Rng,
    start: Vec3,
    dir0: Vec3,
    length: f32,
    radius: f32,
    level: u32,
    center: Vec3,
    radii: Vec3,
    limbs: &mut Vec<Limb>,
    spots: &mut Vec<LeafSpot>,
) {
    let seg = match level {
        1 => 0.2,
        2 => 0.15,
        _ => 0.1,
    };
    let steps = ((length / seg).ceil() as usize).max(2);
    let mut rings = Vec::with_capacity(steps + 1);
    let mut pos = start;
    let mut dir = dir0;
    let tip_radius: f32 = match level {
        1 => 0.03,
        2 => 0.012,
        _ => 0.006,
    };
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let r = radius + (tip_radius.min(radius) - radius) * t.powf(0.8);
        let depth = ((pos - center) / radii).length();
        let ao = 0.5 + 0.5 * depth.min(1.0);
        rings.push(Ring {
            center: pos,
            radius: r,
            color: Vec4::new(ao, ao, ao, 1.0),
        });
        // Wander, reach up toward the light, and stay inside the crown.
        let mut steer = rng.unit_vector() * 0.18 + Vec3::Y * 0.06 * (level as f32);
        if depth > 0.95 {
            steer += (center - pos).normalize_or(Vec3::ZERO) * 0.35 * (depth - 0.95).min(0.5) * 4.0;
        }
        // Heavy limbs sag a little.
        if level == 1 {
            steer -= Vec3::Y * 0.04 * t;
        }
        dir = (dir + steer).normalize();
        pos += dir * (length / steps as f32);
    }
    let sides = match level {
        1 => 10,
        2 => 6,
        _ => 4,
    };
    // Children.
    if level < 3 {
        let count = match level {
            1 => rng.range_i32(5, 9),
            _ => rng.range_i32(5, 9),
        };
        let mut phase = rng.range(0.0, TAU);
        for k in 0..count {
            let t = 0.25 + 0.72 * (k as f32 + rng.range(0.0, 0.7)) / count as f32;
            let i = ((rings.len() - 1) as f32 * t) as usize;
            let here = rings[i].center;
            let along = (rings[(i + 1).min(rings.len() - 1)].center
                - rings[i.saturating_sub(1)].center)
                .normalize_or(dir0);
            phase += 2.39996;
            let side = transport(Vec3::X, along);
            let side = Quat::from_axis_angle(along, phase) * side;
            let angle = rng.range(0.55, 1.0);
            let d = (along * angle.cos() + side * angle.sin() + Vec3::Y * 0.15).normalize();
            let remaining = length * (1.0 - t);
            let len = (remaining * rng.range(0.55, 0.85) + length * 0.15)
                * if level == 1 { 0.75 } else { 0.7 };
            let r = rings[i].radius * rng.range(0.45, 0.65);
            grow_branch(rng, here, d, len, r, level + 1, center, radii, limbs, spots);
        }
    }
    // Leaves along the outer part of the finest branches.
    if level >= 2 {
        let n = rings.len();
        let from = if level == 2 { n / 2 } else { n / 5 };
        for (i, r) in rings.iter().enumerate().skip(from) {
            if level == 3 || i % 2 == 0 || i == n - 1 {
                let d = (rings[(i + 1).min(n - 1)].center - rings[i.saturating_sub(1)].center)
                    .normalize_or(dir);
                spots.push(LeafSpot {
                    pos: r.center,
                    dir: d,
                });
            }
        }
    }
    limbs.push(Limb {
        rings,
        level,
        sides,
    });
}

// ---------------------------------------------------------------------------------- noise

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 16_777_216.0
}

/// Smooth value noise in [-1, 1].
pub fn noise3(p: Vec3, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let h = |dx: i32, dy: i32, dz: i32| hash3(x + dx, y + dy, z + dz, seed);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(h(0, 0, 0), h(1, 0, 0), u.x);
    let x10 = lerp(h(0, 1, 0), h(1, 1, 0), u.x);
    let x01 = lerp(h(0, 0, 1), h(1, 0, 1), u.x);
    let x11 = lerp(h(0, 1, 1), h(1, 1, 1), u.x);
    lerp(lerp(x00, x10, u.y), lerp(x01, x11, u.y), u.z) * 2.0 - 1.0
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Multiply every vertex color by `f(position, normal)`.
fn shade_vertices(mesh: &mut Mesh, f: impl Fn(Vec3, Vec3) -> Vec4) {
    for v in &mut mesh.vertices {
        let k = f(Vec3::from(v.position), Vec3::from(v.normal));
        v.color = (Vec4::from(v.color) * k).to_array();
    }
}

// ---------------------------------------------------------------------------------- materials

/// Image handles of a registered [`TexSet`].
#[derive(Debug, Clone, Copy)]
pub struct TexHandles {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub orm: Handle<Image>,
    pub emissive: Option<Handle<Image>>,
}

/// Register a texture set under `fire/<name>/…` once (generating it with `make` the first
/// time) and return its handles.
pub fn register_set(
    images: &mut Assets<Image>,
    name: &str,
    make: impl FnOnce() -> TexSet,
) -> TexHandles {
    let key = |part: &str| format!("fire/{name}/{part}");
    if let (Some(albedo), Some(normal), Some(orm)) = (
        images.loaded_by_key(&key("albedo")),
        images.loaded_by_key(&key("normal")),
        images.loaded_by_key(&key("orm")),
    ) {
        return TexHandles {
            albedo,
            normal,
            orm,
            emissive: images.loaded_by_key(&key("emissive")),
        };
    }
    let set = take_prefetched(name).unwrap_or_else(make);
    TexHandles {
        albedo: images.insert_keyed(&key("albedo"), set.albedo),
        normal: images.insert_keyed(&key("normal"), set.normal),
        orm: images.insert_keyed(&key("orm"), set.orm),
        emissive: set
            .emissive
            .map(|e| images.insert_keyed(&key("emissive"), e)),
    }
}

/// A fully textured material: albedo × `base_color`, normal map, roughness/metal and
/// occlusion from the ORM image, and the emission mask (glowing cracks once it chars).
pub fn textured(t: &TexHandles) -> StandardMaterial {
    let mut m = StandardMaterial {
        base_color: [1.0; 4],
        metallic: 1.0,
        perceptual_roughness: 1.0,
        ..Default::default()
    }
    .with_base_color_texture(t.albedo)
    .with_normal_texture(t.normal)
    .with_metallic_roughness_texture(t.orm)
    .with_occlusion_texture(t.orm);
    if let Some(e) = t.emissive {
        m = m.with_emissive_texture(e);
    }
    m
}

/// The demo props' materials (textures generated once per process, shared by key).
#[derive(Debug, Clone, Copy)]
pub struct PropMaterials {
    pub bark: StandardMaterial,
    pub leaves: StandardMaterial,
    pub planks: StandardMaterial,
    pub metal: StandardMaterial,
    pub straw: StandardMaterial,
    pub granite: StandardMaterial,
    pub firewood: StandardMaterial,
}

/// Texture size of the props (texels per side; leaves and straw are atlases).
const PROP_TEX: u32 = 1024;

/// The props' texture sets by name.
const PROP_SETS: [&str; 7] = [
    "bark", "leaves", "planks", "metal", "straw", "granite", "firewood",
];

/// Make one of [`PROP_SETS`] (a second or so each, on as many threads as there are cores).
fn make_set(name: &str) -> TexSet {
    match name {
        "bark" => textures::bark(PROP_TEX, 0xB4C),
        "leaves" => textures::leaves(PROP_TEX, 0x1EAF),
        "planks" => textures::planks(256, 2048, 0x9A4),
        "metal" => textures::rusty_metal(PROP_TEX, 0x3E7),
        "straw" => textures::straw(PROP_TEX, 0x57A),
        "granite" => textures::granite(PROP_TEX, 0x6A2),
        "firewood" => textures::firewood(PROP_TEX, 0xF12),
        _ => unreachable!("not a prop texture set: {name}"),
    }
}

/// Prop textures made ahead of time ([`prefetch_textures`]), waiting to be registered.
#[derive(Default)]
struct Prefetch {
    sets: Mutex<Vec<(&'static str, TexSet)>>,
    done: AtomicBool,
}

static PREFETCH: OnceLock<Prefetch> = OnceLock::new();

/// Start making the fire demo's prop textures and its painted ground's layers on threads of
/// their own, so the demo's props (`prop_materials`) and the ground's first frame find them
/// made instead of making them on the frame. Call it once, early in loading;
/// [`textures_ready`] says when they are done. Later calls do nothing.
pub fn prefetch_textures() {
    crate::ground::prefetch_layers();
    if PREFETCH.set(Prefetch::default()).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("fire prop textures".into())
        .spawn(|| {
            let p = PREFETCH.get().expect("set before the thread starts");
            for name in PROP_SETS {
                let set = make_set(name);
                p.sets
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((name, set));
            }
            p.done.store(true, Ordering::Release);
        })
        .expect("spawn the prop texture thread");
}

/// Whether the textures [`prefetch_textures`] started are all made (false if it wasn't called).
pub fn textures_ready() -> bool {
    PREFETCH
        .get()
        .is_some_and(|p| p.done.load(Ordering::Acquire))
        && crate::ground::layers_ready()
}

/// Whether textures [`prefetch_textures`] started are still being made.
pub(crate) fn textures_pending() -> bool {
    PREFETCH
        .get()
        .is_some_and(|p| !p.done.load(Ordering::Acquire))
        || crate::ground::layers_pending()
}

/// A set [`prefetch_textures`] has made, taken out of its store.
fn take_prefetched(name: &str) -> Option<TexSet> {
    let mut sets = PREFETCH
        .get()?
        .sets
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let i = sets.iter().position(|(n, _)| *n == name)?;
    Some(sets.swap_remove(i).1)
}

pub fn prop_materials(images: &mut Assets<Image>) -> PropMaterials {
    let started = std::time::Instant::now();
    let set =
        |images: &mut Assets<Image>, name: &str| register_set(images, name, || make_set(name));
    let bark = set(images, "bark");
    let leaves = set(images, "leaves");
    let planks = set(images, "planks");
    let metal = set(images, "metal");
    let straw = set(images, "straw");
    let granite = set(images, "granite");
    let firewood = set(images, "firewood");
    // Sheet metal hardly glows when the shed burns: a dim, even emission mask.
    let dim = images.get_or_insert_keyed("fire/metal/emissive", || {
        Image::new(4, 4, [70u8, 70, 70, 255].repeat(16), true)
    });
    tracing::debug!(
        "fire prop textures ready in {:.2} s",
        started.elapsed().as_secs_f32()
    );
    PropMaterials {
        bark: textured(&bark),
        leaves: StandardMaterial {
            double_sided: true,
            subsurface: 0.6,
            wind_sway: 0.12,
            metallic: 0.0,
            ..textured(&leaves)
        }
        .with_alpha_mask(0.5),
        planks: textured(&planks),
        metal: textured(&metal).with_emissive_texture(dim),
        straw: StandardMaterial {
            subsurface: 0.2,
            ..textured(&straw)
        },
        granite: textured(&granite),
        firewood: textured(&firewood).with_emissive([1.0, 0.33, 0.08], 160.0),
    }
}

/// Leaves that burn away: as the crown chars (`Burning::char`), its leaves first scorch brown,
/// then blacken, and its leaf cards drop out one by one (each leaf in the atlas has its own
/// alpha level), on top of [`crate::Charring`]'s shrinking. Leaves go much faster than wood.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Foliage;

/// Quantized char level for foliage, so a burning forest shares a few materials.
fn foliage_level(char_level: f32) -> f32 {
    (char_level.clamp(0.0, 1.0) * 16.0).round() / 16.0
}

/// How much of the material's alpha is left at a char level: 1 while green, down to just
/// under the 0.5 cut-off once the crown has properly caught.
pub fn foliage_alpha(char_level: f32) -> f32 {
    let lost = smoothstep(0.02, 0.4, foliage_level(char_level));
    1.0 - 0.52 * (lost * 12.0).round() / 12.0
}

/// Base color of burning foliage: green `base` scorches to brown at the first char, then goes
/// to `char_color`.
pub fn foliage_color(base: [f32; 3], char_color: [f32; 3], char_level: f32) -> [f32; 3] {
    let c = foliage_level(char_level);
    let scorch = smoothstep(0.0, 0.16, c) * 0.9;
    let black = smoothstep(0.14, 0.55, c);
    // Multiplies the green leaf texture toward a dry, rusty brown.
    let tint = [2.4, 0.78, 0.85];
    std::array::from_fn(|i| {
        let scorched = base[i] * (1.0 + (tint[i] - 1.0) * scorch);
        scorched + (char_color[i] - scorched) * black
    })
}

/// Burn [`Foliage`] (runs after `apply_charring`, whose wood-paced darkening it replaces).
#[allow(clippy::type_complexity)]
pub fn burn_foliage(
    mut q: Query<
        (
            &Burning,
            &crate::Charring,
            &crate::sim::CharBase,
            &mut MeshRenderer,
        ),
        (With<Foliage>, Changed<Burning>),
    >,
) {
    for (b, charring, base, mut r) in &mut q {
        if let MaterialRef::Inline(m) = &r.material {
            let mut next = *m;
            let rgb = foliage_color(
                [
                    base.material.base_color[0],
                    base.material.base_color[1],
                    base.material.base_color[2],
                ],
                charring.char_color,
                b.char,
            );
            next.base_color = [rgb[0], rgb[1], rgb[2], foliage_alpha(b.char)];
            if next != *m {
                r.material = MaterialRef::Inline(next);
            }
        }
    }
}

// ---------------------------------------------------------------------------------- shed

/// Height of the shed roof's center (the roof entity's origin), m.
pub const SHED_ROOF_Y: f32 = 2.46;
/// The roof's pitch about x (front, −z, higher), radians.
pub const SHED_ROOF_TILT: f32 = 0.08;
/// Board thickness, m.
const BOARD_T: f32 = 0.022;
/// The door opening in the front wall: from x, to x, height.
const DOOR: [f32; 3] = [-0.15, 0.75, 1.92];

/// Underside of the roof sheet above a point of the shed (shed coordinates).
fn roof_under(z: f32) -> f32 {
    SHED_ROOF_Y - 0.016 - z * SHED_ROOF_TILT.tan()
}

/// A plank or beam: a beveled box `size` (width, length along the grain, thickness) on the
/// basis (`width_dir`, `len_dir`), whose length end at +len rises by `shear` per meter of
/// width (sloped tops), textured with the 0.2 × 2 m board texture.
#[allow(clippy::too_many_arguments)]
fn plank(
    b: &mut MeshBuilder,
    center: Vec3,
    width_dir: Vec3,
    len_dir: Vec3,
    size: Vec3,
    color: Vec4,
    uv_off: Vec2,
    shear: f32,
) {
    let bevel = (size.min_element() * 0.3).min(0.006);
    let mut m = rounded_box(size, bevel, color, &|n, q| {
        let a = n.abs();
        if a.z >= a.x && a.z >= a.y {
            Vec2::new(q.x / 0.2, -q.y / 2.0) + uv_off
        } else if a.x >= a.y {
            Vec2::new(q.z / 0.2, -q.y / 2.0) + uv_off
        } else {
            Vec2::new(q.x / 0.2, q.z / 2.0) + uv_off
        }
    });
    if shear != 0.0 {
        for v in &mut m.vertices {
            let t = (v.position[1] / size.y + 0.5).clamp(0.0, 1.0);
            v.position[1] += shear * v.position[0] * t;
        }
    }
    let thick_dir = width_dir.cross(len_dir);
    b.add(
        m,
        Mat4::from_cols(
            width_dir.extend(0.0),
            len_dir.extend(0.0),
            thick_dir.extend(0.0),
            center.extend(1.0),
        ),
    );
}

/// Where each wall of the shed is: its frame plane's center on the ground, the direction
/// along it (left to right seen from outside) and its outward normal (shed coordinates).
pub fn shed_wall_frame(index: usize) -> (Vec3, Vec3, Vec3) {
    match index {
        0 => (Vec3::new(0.0, 0.0, -1.5), Vec3::X, Vec3::NEG_Z),
        1 => (Vec3::new(0.0, 0.0, 1.5), Vec3::NEG_X, Vec3::Z),
        2 => (Vec3::new(-1.5, 0.0, 0.0), Vec3::NEG_Z, Vec3::NEG_X),
        _ => (Vec3::new(1.5, 0.0, 0.0), Vec3::Z, Vec3::X),
    }
}

/// One wall of the timber shed, in shed coordinates (origin on the ground at the shed's
/// center): weathered vertical boards with gaps on girts and corner posts, corner and door
/// trim, purlins and fascia under the roof; the front wall (0) has a ledged-and-braced door
/// on strap hinges.
pub fn shed_wall(index: usize) -> Mesh {
    let mut rng = Rng::new(0x5ED0 + index as u64 * 7919);
    let (center, along, out) = shed_wall_frame(index);
    let up = Vec3::Y;
    let mut b = MeshBuilder::default();
    let tan = SHED_ROOF_TILT.tan();
    // Height of the wall top changes along the side walls.
    let slope = -along.z * tan;
    let at = |s: f32, y: f32, o: f32| center + along * s + up * y + out * o;
    let top = |s: f32| roof_under(at(s, 0.0, 0.0).z);
    let front_back = index < 2;
    let half = if front_back {
        1.5 + BOARD_T + 0.004
    } else {
        1.5
    };
    let wood_tint = |rng: &mut Rng| {
        let k = rng.range(0.82, 1.1);
        let warm = rng.range(-0.06, 0.06);
        Vec4::new(k * (1.0 + warm), k, k * (1.0 - warm), 1.0)
    };

    // Boards, in runs (the door interrupts the front wall).
    let mut runs: Vec<(f32, f32, f32)> = Vec::new(); // from, to, bottom
    if index == 0 {
        runs.push((-half, DOOR[0], 0.0));
        runs.push((DOOR[0], DOOR[1], DOOR[2] + 0.07));
        runs.push((DOOR[1], half, 0.0));
    } else {
        runs.push((-half, half, 0.0));
    }
    for (from, to, bottom) in runs {
        let mut s = from;
        while to - s > 0.04 {
            let mut w = rng.range(0.15, 0.22);
            let gap = rng.range(0.004, 0.016);
            if to - s - w < 0.1 {
                w = to - s - gap * 0.5;
            }
            let c = s + w * 0.5;
            let y0 = bottom + rng.range(0.015, 0.045);
            let y1 = top(c) - 0.006 - rng.range(0.0, 0.006);
            let len = y1 - y0;
            plank(
                &mut b,
                at(c, (y0 + y1) * 0.5, BOARD_T * 0.5),
                along,
                up,
                Vec3::new(w, len, BOARD_T),
                wood_tint(&mut rng),
                Vec2::new(rng.range(0.0, 1.0), rng.range(0.0, 1.0)),
                slope,
            );
            s += w + gap;
        }
    }

    let frame_tint = Vec4::new(0.8, 0.76, 0.72, 1.0);
    let beam = |b: &mut MeshBuilder, rng: &mut Rng, from: f32, to: f32, y: f32, h: f32| {
        // A horizontal member along the wall, inside the boards.
        plank(
            b,
            at((from + to) * 0.5, y, -0.0225),
            up,
            along,
            Vec3::new(h, to - from, 0.045),
            frame_tint,
            Vec2::new(rng.range(0.0, 1.0), rng.range(0.0, 1.0)),
            0.0,
        );
    };
    // Corner post at the start of the wall, and girts.
    let post = |b: &mut MeshBuilder, rng: &mut Rng, s: f32, size: f32| {
        let y1 = top(s) - 0.07;
        plank(
            b,
            at(s, y1 * 0.5, -size * 0.5),
            along,
            up,
            Vec3::new(size, y1, size),
            frame_tint,
            Vec2::new(rng.range(0.0, 1.0), rng.range(0.0, 1.0)),
            0.0,
        );
    };
    post(&mut b, &mut rng, -1.45, 0.1);
    for y in [0.42, 1.32] {
        if index == 0 {
            beam(&mut b, &mut rng, -1.4, DOOR[0] - 0.08, y, 0.09);
            beam(&mut b, &mut rng, DOOR[1] + 0.08, 1.4, y, 0.09);
        } else {
            beam(&mut b, &mut rng, -1.4, 1.4, y, 0.09);
        }
    }
    if index == 0 {
        // Door jambs and header inside.
        post(&mut b, &mut rng, DOOR[0] - 0.04, 0.08);
        post(&mut b, &mut rng, DOOR[1] + 0.04, 0.08);
        beam(
            &mut b,
            &mut rng,
            DOOR[0] - 0.08,
            DOOR[1] + 0.08,
            DOOR[2] + 0.05,
            0.1,
        );
    }
    if !front_back {
        // Sloped top plate the purlins rest on.
        let c = 0.0;
        let y = top(c) - 0.06 - 0.045;
        plank(
            &mut b,
            at(c, y, -0.045),
            along,
            up,
            Vec3::new(2.9, 0.09, 0.09),
            frame_tint,
            Vec2::new(rng.range(0.0, 1.0), 0.3),
            0.0,
        );
        // Rotate it to the slope: shear its vertices.
        let n = b.mesh.vertices.len();
        let base = n - 6 * 36;
        for v in &mut b.mesh.vertices[base..] {
            let p = Vec3::from(v.position);
            let s = (p - center).dot(along);
            v.position[1] += slope * s;
        }
    }

    // Purlins under the roof sheet (all the way out to the barge boards).
    let purlins: &[f32] = match index {
        0 => &[-1.47, -0.75],
        1 => &[0.75, 1.47],
        2 => &[0.0],
        _ => &[],
    };
    for &z in purlins {
        let y = roof_under(z) - 0.03;
        plank(
            &mut b,
            Vec3::new(0.0, y, z),
            Vec3::Z,
            Vec3::X,
            Vec3::new(0.07, 3.5, 0.06),
            frame_tint,
            Vec2::new(rng.range(0.0, 1.0), rng.range(0.0, 1.0)),
            0.0,
        );
    }

    // Trim outside: corner boards, fascia and barge boards, door casing.
    let trim = BOARD_T + 0.011;
    let trim_tint = Vec4::new(0.92, 0.9, 0.86, 1.0);
    for s in [-1.0f32, 1.0] {
        let sc = s * (half - 0.05);
        let y1 = top(sc) - 0.01;
        plank(
            &mut b,
            at(sc, y1 * 0.5, trim),
            along,
            up,
            Vec3::new(0.1, y1, BOARD_T),
            trim_tint,
            Vec2::new(rng.range(0.0, 1.0), 0.0),
            0.0,
        );
    }
    if front_back {
        let z = center.z + out.z * 0.3;
        let y = roof_under(z) - 0.085;
        plank(
            &mut b,
            Vec3::new(0.0, y, z),
            Vec3::Y,
            Vec3::X,
            Vec3::new(0.16, 3.54, BOARD_T),
            trim_tint,
            Vec2::new(rng.range(0.0, 1.0), 0.0),
            0.0,
        );
    } else {
        let x = center.x + out.x * 0.265;
        let n0 = b.mesh.vertices.len();
        plank(
            &mut b,
            Vec3::new(x, roof_under(0.0) - 0.075, 0.0),
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.14, 3.62, BOARD_T),
            trim_tint,
            Vec2::new(rng.range(0.0, 1.0), 0.0),
            0.0,
        );
        for v in &mut b.mesh.vertices[n0..] {
            v.position[1] -= v.position[2] * tan;
        }
    }
    if index == 0 {
        // Casing around the door opening.
        for s in [DOOR[0] - 0.045, DOOR[1] + 0.045] {
            plank(
                &mut b,
                at(s, (DOOR[2] + 0.09) * 0.5, trim),
                along,
                up,
                Vec3::new(0.09, DOOR[2] + 0.09, BOARD_T),
                trim_tint,
                Vec2::new(rng.range(0.0, 1.0), 0.0),
                0.0,
            );
        }
        plank(
            &mut b,
            at((DOOR[0] + DOOR[1]) * 0.5, DOOR[2] + 0.045, trim),
            up,
            along,
            Vec3::new(0.09, DOOR[1] - DOOR[0] + 0.18, BOARD_T),
            trim_tint,
            Vec2::new(rng.range(0.0, 1.0), 0.0),
            0.0,
        );
        // The door: tight boards, two ledges and a brace, strap hinges and a latch.
        let boards = 5;
        let dw = (DOOR[1] - DOOR[0] - 0.02) / boards as f32;
        let door_tint = Vec4::new(0.95, 0.88, 0.8, 1.0);
        for k in 0..boards {
            let c = DOOR[0] + 0.01 + dw * (k as f32 + 0.5);
            plank(
                &mut b,
                at(c, (DOOR[2] + 0.03) * 0.5, BOARD_T * 0.5 + 0.004),
                along,
                up,
                Vec3::new(dw - 0.003, DOOR[2] - 0.05, BOARD_T),
                door_tint * rng.range(0.9, 1.05),
                Vec2::new(rng.range(0.0, 1.0), rng.range(0.0, 1.0)),
                0.0,
            );
        }
        let face = BOARD_T + 0.004 + BOARD_T * 0.5;
        let dl = DOOR[1] - DOOR[0] - 0.06;
        let dc = (DOOR[0] + DOOR[1]) * 0.5;
        for y in [0.32, 1.6] {
            plank(
                &mut b,
                at(dc, y, face),
                up,
                along,
                Vec3::new(0.13, dl, BOARD_T),
                door_tint,
                Vec2::new(rng.range(0.0, 1.0), 0.0),
                0.0,
            );
        }
        let p0 = Vec2::new(DOOR[0] + 0.12, 0.42);
        let p1 = Vec2::new(DOOR[1] - 0.12, 1.5);
        let d = (p1 - p0).normalize();
        let len_dir = (along * d.x + up * d.y).normalize();
        let width_dir = len_dir.cross(out).normalize();
        plank(
            &mut b,
            at((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5, face),
            width_dir,
            len_dir,
            Vec3::new(0.11, p0.distance(p1) + 0.06, BOARD_T),
            door_tint,
            Vec2::new(rng.range(0.0, 1.0), 0.0),
            0.0,
        );
        let iron = Vec4::new(0.16, 0.1, 0.07, 1.0);
        let iron_face = face + BOARD_T * 0.5 + 0.003;
        for y in [0.32, 1.6] {
            plank(
                &mut b,
                at(DOOR[0] + 0.17, y, iron_face),
                up,
                along,
                Vec3::new(0.045, 0.46, 0.006),
                iron,
                Vec2::ZERO,
                0.0,
            );
            // Pintle on the casing.
            plank(
                &mut b,
                at(DOOR[0] - 0.04, y, trim + BOARD_T * 0.5 + 0.008),
                along,
                up,
                Vec3::new(0.05, 0.06, 0.016),
                iron,
                Vec2::ZERO,
                0.0,
            );
        }
        plank(
            &mut b,
            at(DOOR[1] - 0.1, 1.05, iron_face + 0.01),
            along,
            up,
            Vec3::new(0.03, 0.17, 0.025),
            iron,
            Vec2::ZERO,
            0.0,
        );
    }

    let mut mesh = b.finish();
    // Dirt and damp at the foot of the wall, darker inside.
    shade_vertices(&mut mesh, |p, n| {
        let foot = 0.55 + 0.45 * smoothstep(0.0, 0.45, p.y);
        let inside = if n.dot(out) < -0.3 { 0.7 } else { 1.0 };
        Vec4::new(foot * inside, foot * inside, foot * inside, 1.0)
    });
    mesh
}

/// The shed roof in the roof entity's space (origin at its center, before its tilt): three
/// overlapping corrugated sheets with a few dents, top and underside.
pub fn shed_roof() -> Mesh {
    let mut b = MeshBuilder::default();
    let wave = 0.125;
    let amp = 0.014;
    let sheet_w = 1.25;
    let (z0, z1) = (-1.8, 1.8);
    for k in 0..3 {
        let x0 = -1.75 + k as f32 * (sheet_w - wave);
        let lift = k as f32 * 0.0035;
        let seed = 0x7007 + k;
        let cols = 10 * 12;
        let rows = 14;
        let height = |x: f32, z: f32| {
            let corr = amp * (TAU * (x - x0) / wave).sin();
            let dent = 0.005 * noise3(Vec3::new(x * 1.7, 0.0, z * 1.3), seed)
                + 0.0015 * noise3(Vec3::new(x * 6.0, 1.0, z * 5.0), seed);
            corr + dent + lift
        };
        for side in [1.0f32, -1.0] {
            let base = b.mesh.vertices.len() as u32;
            for j in 0..=rows {
                let z = z0 + (z1 - z0) * j as f32 / rows as f32;
                for i in 0..=cols {
                    let x = x0 + sheet_w * i as f32 / cols as f32;
                    let y = height(x, z);
                    let e = 0.002;
                    let dx = (height(x + e, z) - height(x - e, z)) / (2.0 * e);
                    let dz = (height(x, z + e) - height(x, z - e)) / (2.0 * e);
                    let n = Vec3::new(-dx, 1.0, -dz).normalize() * side;
                    let p = Vec3::new(x, y - if side < 0.0 { 0.0015 } else { 0.0 }, z);
                    let shade = if side < 0.0 { 0.55 } else { 1.0 };
                    // Dirt collects toward the low (back) edge.
                    let grime = 1.0 - 0.2 * smoothstep(0.8, 1.8, z);
                    let c = shade * grime;
                    b.vertex(
                        p,
                        n,
                        Vec2::new(x + k as f32 * 0.37, z + k as f32 * 0.21),
                        Vec4::new(c, c, c, 1.0),
                    );
                }
            }
            let row = cols + 1;
            for j in 0..rows {
                for i in 0..cols {
                    let a = base + j * row + i;
                    if side > 0.0 {
                        b.quad(a, a + row, a + row + 1, a + 1);
                    } else {
                        b.quad(a, a + 1, a + row + 1, a + row);
                    }
                }
            }
        }
    }
    b.finish()
}

// ---------------------------------------------------------------------------------- hay bales

/// Round bale radius and width, m.
pub const BALE_RADIUS: f32 = 0.6;
pub const BALE_WIDTH: f32 = 1.3;

/// A round hay bale lying on its side, its axis along x and its flattened foot on y = 0:
/// rolled ends with rounded edges, a slightly bulging side, twine wrapped around it. Uses the
/// [`textures::straw`] atlas.
pub fn hay_bale(seed: u32) -> Mesh {
    let r = BALE_RADIUS;
    let w = BALE_WIDTH;
    let fillet = 0.11;
    let sides = 72usize;
    // Profile from the -x end's center around to the +x end's center: (x, radius, region)
    // with region 0 = end face, 1 = side (incl. the rounded edges).
    let mut profile: Vec<(f32, f32, u8)> = Vec::new();
    let end_steps = 6;
    for i in 0..=end_steps {
        profile.push((-w * 0.5, (r - fillet) * i as f32 / end_steps as f32, 0));
    }
    let arc = 5;
    let mut side_part = Vec::new();
    for i in 0..=arc {
        let a = FRAC_PI_2 * i as f32 / arc as f32;
        side_part.push((
            -w * 0.5 + fillet - fillet * a.cos(),
            r - fillet + fillet * a.sin(),
        ));
    }
    let along = 12;
    for i in 1..along {
        let x = -w * 0.5 + fillet + (w - 2.0 * fillet) * i as f32 / along as f32;
        side_part.push((x, r));
    }
    for i in 0..=arc {
        let a = FRAC_PI_2 * (1.0 - i as f32 / arc as f32);
        side_part.push((
            w * 0.5 - fillet + fillet * a.cos(),
            r - fillet + fillet * a.sin(),
        ));
    }
    for (x, rr) in &side_part {
        profile.push((*x, *rr, 1));
    }
    for i in (0..=end_steps).rev() {
        profile.push((w * 0.5, (r - fillet) * i as f32 / end_steps as f32, 2));
    }

    // Surface point for a profile entry and an angle (0 = up, around +x).
    let surface = |x: f32, rr: f32, a: f32| {
        let v = (x / w + 0.5).clamp(0.0, 1.0);
        let bulge = 1.0 + 0.03 * (1.0 - (2.0 * v - 1.0).powi(2));
        let lumpy = 1.0
            + 0.018 * noise3(Vec3::new(a.cos() * 2.0, a.sin() * 2.0, x * 2.0), seed)
            + 0.006 * noise3(Vec3::new(a.cos() * 7.0, a.sin() * 7.0, x * 6.0), seed ^ 5);
        let k = rr * bulge * lumpy;
        let y = r + k * a.cos();
        let z = k * a.sin();
        // Settled on the ground: a flat, slightly spread foot.
        let h0 = 0.05;
        let soft = 0.09;
        let smax = |a: f32, b: f32| 0.5 * (a + b + ((a - b) * (a - b) + soft * soft).sqrt());
        let floor = smax(0.0, h0);
        let y2 = smax(y, h0) - floor;
        let spread = 1.0 + 0.05 * (1.0 - (y2 / (2.0 * r)).min(1.0)).powi(4);
        Vec3::new(x, y2, z * spread)
    };
    let grid: Vec<Vec<Vec3>> = profile
        .iter()
        .map(|(x, rr, _)| {
            (0..sides)
                .map(|i| surface(*x, *rr, i as f32 / sides as f32 * TAU))
                .collect()
        })
        .collect();
    let normal_at = |j: usize, i: usize| -> Vec3 {
        let jp = (j + 1).min(profile.len() - 1);
        let jm = j.saturating_sub(1);
        let ip = (i + 1) % sides;
        let im = (i + sides - 1) % sides;
        let dj = grid[jp][i] - grid[jm][i];
        let di = grid[j][ip] - grid[j][im];
        let n = di.cross(dj);
        if n.length_squared() < 1e-12 || profile[j].1 < 1e-4 {
            // End-face centers.
            return if profile[j].0 < 0.0 {
                Vec3::NEG_X
            } else {
                Vec3::X
            };
        }
        n.normalize()
    };
    let mut b = MeshBuilder::default();
    let white = Vec4::ONE;
    let ao = |p: Vec3| {
        let k = 0.6 + 0.4 * smoothstep(0.0, 0.35, p.y);
        Vec4::new(k, k, k, 1.0)
    };
    // End faces: the rolled spiral.
    for (region, flip) in [(0u8, 1.0f32), (2, -1.0)] {
        let rows: Vec<usize> = (0..profile.len())
            .filter(|&j| profile[j].2 == region)
            .collect();
        let base = b.mesh.vertices.len() as u32;
        for &j in &rows {
            let rho = 0.96 * profile[j].1 / (r - fillet);
            for i in 0..=sides {
                let a = i as f32 / sides as f32 * TAU;
                let ii = i % sides;
                let p = grid[j][ii];
                let uv = Vec2::new(
                    0.75 + 0.25 * rho * a.cos() * flip,
                    0.5 + 0.5 * rho * a.sin(),
                );
                b.vertex(p, normal_at(j, ii), uv, white * ao(p));
            }
        }
        let row = sides as u32 + 1;
        for k in 0..rows.len() as u32 - 1 {
            for i in 0..sides as u32 {
                let a = base + k * row + i;
                b.quad(a, a + 1, a + row + 1, a + row);
            }
        }
    }
    // Side: stalks wrapped around, the atlas' left half twice around the bale.
    let side_rows: Vec<usize> = (0..profile.len()).filter(|&j| profile[j].2 == 1).collect();
    let half = sides / 2;
    for h in 0..2 {
        let base = b.mesh.vertices.len() as u32;
        for &j in &side_rows {
            let v = (profile[j].0 / w + 0.5).clamp(0.0, 1.0);
            for i in 0..=half {
                let ii = (h * half + i) % sides;
                let p = grid[j][ii];
                let uv = Vec2::new(0.5 * i as f32 / half as f32, v);
                b.vertex(p, normal_at(j, ii), uv, white * ao(p));
            }
        }
        let row = half as u32 + 1;
        for k in 0..side_rows.len() as u32 - 1 {
            for i in 0..half as u32 {
                let a = base + k * row + i;
                b.quad(a, a + 1, a + row + 1, a + row);
            }
        }
    }
    // Twine: a few loops spiralling around the bale.
    let loops = 6;
    let twine = Vec4::new(0.42, 0.36, 0.26, 1.0);
    let twine_uv = Vec2::new(0.5 + 0.985 * 0.5, 0.015);
    let mut rng = Rng::new(seed as u64 ^ 0x7A1E);
    for l in 0..loops {
        let v0 = 0.1 + 0.8 * (l as f32 + rng.range(-0.15, 0.15)) / (loops - 1) as f32;
        let steps = 96;
        let rings: Vec<Ring> = (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                let a = t * TAU + l as f32 * 0.7;
                let x = (v0 - 0.5 + 0.035 * (t - 0.5)) * (w - 2.0 * fillet);
                let p = surface(x, r, a);
                let q = surface(x, r * 0.98, a);
                let n = (p - q).normalize_or(Vec3::Y);
                Ring {
                    center: p + n * 0.0005,
                    radius: 0.003,
                    color: twine * ao(p),
                }
            })
            .collect();
        let n0 = b.mesh.vertices.len();
        tube(&mut b, &rings, 5, 1.0, 1.0, 0.0, &|_, _| 1.0);
        for v in &mut b.mesh.vertices[n0..] {
            v.uv = twine_uv.to_array();
        }
    }
    let mut mesh = b.finish();
    // Rain and sun bleach the top grey.
    shade_vertices(&mut mesh, |_, n| {
        let k = smoothstep(0.1, 0.9, n.y) * 0.5;
        Vec4::new(1.0 - 0.22 * k, 1.0 - 0.16 * k, 1.0 + 0.1 * k, 1.0)
    });
    mesh
}

// ---------------------------------------------------------------------------------- campfire

/// Fieldstones for a campfire ring of `radius`, around the origin on the ground: each one
/// its own lumpy, flattened shape, half sunk, sooted on the side facing the fire. Uses the
/// [`textures::granite`] texture (box-projected).
pub fn campfire_stones(radius: f32, count: u32, seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut out = MeshBuilder::default();
    let (seg_a, seg_b) = (22usize, 12usize);
    for k in 0..count {
        let a = k as f32 / count as f32 * TAU + rng.range(-0.12, 0.12);
        let ring = radius + rng.range(-0.05, 0.05);
        let size = Vec3::new(
            rng.range(0.15, 0.21),
            rng.range(0.11, 0.16),
            rng.range(0.12, 0.17),
        );
        let s = rng.next_u32();
        let off = Vec2::new(rng.f32(), rng.f32());
        // Split faces: a few planes cut each stone, so it reads as broken rock, not a pebble.
        let cuts: Vec<(Vec3, f32)> = (0..rng.range_i32(3, 6))
            .map(|_| {
                let mut n = rng.unit_vector();
                n.y *= 0.6;
                (n.normalize_or(Vec3::X), rng.range(0.45, 0.72))
            })
            .collect();
        let tone = rng.range(0.5, 0.85);
        let warm = rng.range(-0.06, 0.08);
        // A lumpy blob.
        let mut grid = Vec::with_capacity((seg_a + 1) * (seg_b + 1));
        for j in 0..=seg_b {
            let th = PI * j as f32 / seg_b as f32;
            for i in 0..=seg_a {
                let ph = TAU * (i % seg_a) as f32 / seg_a as f32;
                let d = Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
                let lump = 1.0
                    + 0.22 * noise3(d * 1.3, s)
                    + 0.08 * noise3(d * 3.1, s ^ 1)
                    + 0.025 * noise3(d * 7.0, s ^ 2);
                let mut q = d * lump;
                for (n, o) in &cuts {
                    let over = q.dot(*n) - o;
                    if over > 0.0 {
                        q -= *n * over * 0.95;
                    }
                }
                let mut p = q * size;
                // Sunk into the ground, a flat-ish underside.
                p.y = p.y.max(-size.y * 0.45);
                grid.push(p);
            }
        }
        let mut blob = Mesh::default();
        for p in &grid {
            blob.vertices.push(Vertex::new(*p, Vec3::Y, Vec2::ZERO));
        }
        let row = seg_a as u32 + 1;
        for j in 0..seg_b as u32 {
            for i in 0..seg_a as u32 {
                let q = j * row + i;
                blob.indices
                    .extend_from_slice(&[q, q + 1, q + row + 1, q, q + row + 1, q + row]);
            }
        }
        // Weld the seam so normals are smooth across it.
        for j in 0..=seg_b {
            let first = j * (seg_a + 1);
            let last = first + seg_a;
            blob.vertices[last].position = blob.vertices[first].position;
        }
        blob.compute_normals();
        // Average the seam normals.
        for j in 0..=seg_b {
            let first = j * (seg_a + 1);
            let last = first + seg_a;
            let n = (Vec3::from(blob.vertices[first].normal)
                + Vec3::from(blob.vertices[last].normal))
            .normalize_or(Vec3::Y);
            blob.vertices[first].normal = n.to_array();
            blob.vertices[last].normal = n.to_array();
        }
        let yaw = a + FRAC_PI_2 + rng.range(-0.3, 0.3);
        let tilt = Quat::from_rotation_x(rng.range(-0.15, 0.15))
            * Quat::from_rotation_z(rng.range(-0.15, 0.15));
        let pos = Vec3::new(a.cos() * ring, size.y * 0.12, a.sin() * ring);
        let m = Mat4::from_rotation_translation(Quat::from_rotation_y(-yaw) * tilt, pos);
        blob.transform(&m);
        // Per-triangle box projection (granite tile 0.5 m) with smooth normals.
        let inward = Vec3::new(-a.cos(), 0.0, -a.sin());
        for tri in blob.indices.as_chunks::<3>().0 {
            let p: [Vec3; 3] =
                std::array::from_fn(|i| Vec3::from(blob.vertices[tri[i] as usize].position));
            let fnrm = (p[1] - p[0]).cross(p[2] - p[0]);
            if fnrm.length_squared() < 1e-14 {
                continue;
            }
            let an = fnrm.abs();
            let mut idx = [0u32; 3];
            for (c, &t) in tri.iter().enumerate() {
                let v = blob.vertices[t as usize];
                let q = Vec3::from(v.position);
                let n = Vec3::from(v.normal);
                let uv = if an.y >= an.x && an.y >= an.z {
                    Vec2::new(q.x, q.z)
                } else if an.x >= an.z {
                    Vec2::new(q.z, -q.y)
                } else {
                    Vec2::new(q.x, -q.y)
                } / 0.5
                    + off;
                // Soot on the fire side and top, earth at the foot.
                let soot = (n.dot(inward) * 0.7 + n.y * 0.3).clamp(0.0, 1.0)
                    * (1.0 - 0.35 * smoothstep(0.0, 0.5, q.distance(pos) / size.x));
                let foot = 0.55 + 0.45 * smoothstep(0.0, 0.08, q.y);
                let k = (1.0 - 0.8 * soot) * foot * tone;
                let tint = Vec4::new(k * (1.0 + warm), k, k * (1.0 - warm), 1.0);
                idx[c] = out.vertex(q, n, uv, tint);
            }
            out.tri(idx[0], idx[1], idx[2]);
        }
    }
    out.finish()
}

/// One split log along +x from its fire end (x = 0) to `length`: a wedge of a round log
/// (`wedge` radians of bark, the rest split faces; a full turn is an unsplit log) with
/// end grain on both ends. Uses the [`textures::firewood`] atlas (charred toward x = 0).
fn split_log(b: &mut MeshBuilder, length: f32, radius: f32, wedge: f32, seed: u32, m: Mat4) {
    let mut log = MeshBuilder::default();
    let rings = 14usize;
    let bark_n = 18usize;
    let face_n = 5usize;
    let split = wedge < TAU - 0.01;
    let wobble = |x: f32, a: f32| {
        1.0 + 0.06 * noise3(Vec3::new(a.cos() * 1.5, a.sin() * 1.5, x * 4.0), seed)
            + 0.025 * noise3(Vec3::new(a.cos() * 5.0, a.sin() * 5.0, x * 12.0), seed ^ 3)
    };
    let bend = |x: f32| {
        Vec3::new(
            0.0,
            0.012 * noise3(Vec3::new(x * 2.0, 0.0, 0.0), seed ^ 7),
            0.012 * noise3(Vec3::new(x * 2.0, 5.0, 0.0), seed ^ 9),
        )
    };
    // The fire end is burnt thinner and rounder.
    let taper = |t: f32| 1.0 - 0.3 * (1.0 - smoothstep(0.0, 0.25, t));
    let point = |x: f32, a: f32, rr: f32| -> Vec3 {
        let t = x / length;
        let k = rr * taper(t) * wobble(x, a);
        Vec3::new(x, a.cos() * k, a.sin() * k) + bend(x)
    };
    let a0 = -wedge * 0.5;
    // Bark.
    let base = log.mesh.vertices.len() as u32;
    for j in 0..=rings {
        let x = length * j as f32 / rings as f32;
        let v = x / length;
        for i in 0..=bark_n {
            let a = a0 + wedge * i as f32 / bark_n as f32;
            let p = point(x, a, radius);
            let e = 0.01;
            let pa = point(x, a + e, radius) - point(x, a - e, radius);
            let px = point(x + e, a, radius) - point(x - e, a, radius);
            let n = px.cross(pa).normalize_or(Vec3::new(0.0, a.cos(), a.sin()));
            let u = 0.5 * i as f32 / bark_n as f32;
            log.vertex(p, n, Vec2::new(u, v), Vec4::ONE);
        }
    }
    let row = bark_n as u32 + 1;
    for j in 0..rings as u32 {
        for i in 0..bark_n as u32 {
            let q = base + j * row + i;
            log.quad(q, q + row, q + row + 1, q + 1);
        }
    }
    // Split faces: from each end of the bark arc in to the heart.
    if split {
        for (side, a) in [(0, a0 + wedge), (1, a0)] {
            let base = log.mesh.vertices.len() as u32;
            for j in 0..=rings {
                let x = length * j as f32 / rings as f32;
                let rim = point(x, a, radius);
                let heart = Vec3::new(x, 0.0, 0.0) + bend(x);
                for i in 0..=face_n {
                    let t = i as f32 / face_n as f32;
                    let torn = 0.004 * noise3(Vec3::new(x * 20.0, t * 4.0, side as f32), seed ^ 11);
                    let dir = Vec3::new(0.0, a.cos(), a.sin());
                    let side_n = if side == 0 {
                        Vec3::new(0.0, -a.sin(), a.cos())
                    } else {
                        Vec3::new(0.0, a.sin(), -a.cos())
                    };
                    let p = rim.lerp(heart, t) + side_n * torn - dir * 0.0;
                    let u = 0.5 + 0.375 * (if side == 0 { t * 0.5 } else { 0.5 + t * 0.5 });
                    log.vertex(p, side_n, Vec2::new(u, x / length), Vec4::ONE);
                }
            }
            let row = face_n as u32 + 1;
            for j in 0..rings as u32 {
                for i in 0..face_n as u32 {
                    let q = base + j * row + i;
                    if side == 1 {
                        log.quad(q, q + row, q + row + 1, q + 1);
                    } else {
                        log.quad(q, q + 1, q + row + 1, q + row);
                    }
                }
            }
        }
    }
    // End grain: fans over the cross-section at both ends.
    for (end, x) in [(0, 0.0f32), (1, length)] {
        let center = Vec3::new(x, 0.0, 0.0) + bend(x);
        let disc = if end == 0 {
            Vec2::new(0.9375, 0.1875)
        } else {
            Vec2::new(0.9375, 0.0625)
        };
        let n = if end == 0 { Vec3::NEG_X } else { Vec3::X };
        let mut outline: Vec<Vec3> = (0..=bark_n)
            .map(|i| point(x, a0 + wedge * i as f32 / bark_n as f32, radius))
            .collect();
        if split {
            outline.push(center);
        }
        let c = log.vertex(center, n, disc, Vec4::ONE);
        let first = log.mesh.vertices.len() as u32;
        for p in &outline {
            let d = (*p - center) / (radius * 1.05);
            log.vertex(*p, n, disc + Vec2::new(d.y, d.z) * 0.0625, Vec4::ONE);
        }
        let count = outline.len() as u32;
        let segs = if split { count } else { count - 1 };
        for i in 0..segs {
            let a = first + i;
            let bb = first + (i + 1) % count;
            if end == 0 {
                log.tri(c, bb, a);
            } else {
                log.tri(c, a, bb);
            }
        }
    }
    b.add(log.finish(), m);
}

/// The campfire's wood: split logs leaning into a teepee and lying in a star, their burnt
/// ends in the fire, and chunks of glowing charcoal in the middle. Uses the
/// [`textures::firewood`] atlas; the emission mask lights only the charred cracks.
pub fn campfire_wood(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = MeshBuilder::default();
    let logs = 6;
    let mut yaw = rng.range(0.0, TAU);
    for k in 0..logs {
        yaw += TAU / logs as f32 + rng.range(-0.25, 0.25);
        let leaning = k % 2 == 0;
        let radius = rng.range(0.06, 0.09);
        let wedge = match rng.range_i32(0, 3) {
            0 => TAU,
            1 => PI,
            _ => rng.range(1.4, 2.2),
        };
        // Leaning logs rest their fire ends on each other, star logs lie in the embers;
        // either way the outer end rests on the ground inside the stones.
        let (length, inner) = if leaning {
            (
                rng.range(0.62, 0.74),
                Vec3::new(0.04, rng.range(0.3, 0.4), 0.0),
            )
        } else {
            (
                rng.range(0.52, 0.64),
                Vec3::new(0.08, radius * 0.9 + 0.03, 0.0),
            )
        };
        let dy = radius * 0.85 - inner.y;
        let reach = (length * length - dy * dy).max(0.01).sqrt();
        let outer = inner + Vec3::new(reach, dy, 0.0);
        let rot = Quat::from_rotation_y(-yaw);
        let p0 = rot * inner;
        let dir = (rot * outer - p0).normalize();
        let side = dir.cross(Vec3::Y).normalize_or(Vec3::Z);
        let upish = side.cross(dir);
        let roll = rng.range(0.0, TAU);
        let (s, c) = roll.sin_cos();
        let y_axis = upish * c + side * s;
        let z_axis = dir.cross(y_axis);
        let m = Mat4::from_cols(
            dir.extend(0.0),
            y_axis.extend(0.0),
            z_axis.extend(0.0),
            p0.extend(1.0),
        );
        split_log(&mut b, length, radius, wedge, rng.next_u32(), m);
    }
    // Charcoal: a bed of chunky lumps from the live-coal region of the atlas, glowing between
    // the logs.
    for _ in 0..30 {
        let a = rng.range(0.0, TAU);
        let d = rng.range(0.0, 0.34).sqrt() * 0.58;
        let size = Vec3::new(
            rng.range(0.035, 0.08),
            rng.range(0.025, 0.045),
            rng.range(0.03, 0.06),
        );
        let s = rng.next_u32();
        let (du, dv) = (rng.range(-0.02, 0.02), rng.range(-0.06, 0.06));
        let mut lump = rounded_box(size, size.min_element() * 0.35, Vec4::ONE, &|_, q| {
            Vec2::new(
                0.9375 + du + (q.x + q.z) * 0.5,
                0.375 + dv + q.y + (q.x - q.z) * 0.5,
            )
        });
        for v in &mut lump.vertices {
            let p = Vec3::from(v.position);
            let k = 1.0 + 0.25 * noise3(p * 40.0, s);
            v.position = (p * k).to_array();
        }
        let m = Mat4::from_rotation_translation(
            Quat::from_rotation_y(rng.range(0.0, TAU))
                * Quat::from_rotation_x(rng.range(-0.4, 0.4)),
            Vec3::new(a.cos() * d, size.y * 0.4, a.sin() * d),
        );
        b.add(lump, m);
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_mesh(m: &Mesh) {
        assert!(!m.indices.is_empty());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        assert!(
            m.vertices
                .iter()
                .all(|v| v.position.iter().chain(&v.normal).all(|x| x.is_finite()))
        );
    }

    #[test]
    fn rounded_box_is_closed_and_bounded() {
        let m = rounded_box(Vec3::new(2.0, 1.0, 0.5), 0.05, Vec4::ONE, &|_, p| {
            Vec2::new(p.x, p.y)
        });
        check_mesh(&m);
        let b = m.aabb();
        assert!(b.max.abs_diff_eq(Vec3::new(1.0, 0.5, 0.25), 1e-5), "{b:?}");
    }

    #[test]
    fn trees_grow_wood_and_leaves_inside_their_crown() {
        let p = TreeParams {
            height: 8.0,
            crown_base: 2.6,
            crown_radius: 2.8,
            trunk_radius: 0.28,
            seed: 7,
        };
        let t = grow_tree(&p);
        check_mesh(&t.wood);
        check_mesh(&t.leaves);
        let wood = t.wood.aabb();
        assert!(wood.max.y > 6.0 && wood.max.y < 9.5, "{wood:?}");
        assert!(wood.min.y < 0.0, "trunk reaches into the ground");
        let cards = t.leaves.vertices.len() / 4;
        assert!(cards > 250, "{cards} leaf cards");
        let leaves = t.leaves.aabb();
        assert!(
            leaves.max.x - leaves.min.x < 2.0 * p.crown_radius + 3.0,
            "{leaves:?}"
        );
        assert!(t.crown_center.y > p.crown_base);
    }

    #[test]
    fn shed_parts_fit_the_shed() {
        for i in 0..4 {
            let m = shed_wall(i);
            check_mesh(&m);
            let b = m.aabb();
            assert!(b.min.y >= -0.01 && b.max.y < 2.7, "wall {i}: {b:?}");
            assert!(b.min.x > -1.9 && b.max.x < 1.9 && b.min.z > -1.9 && b.max.z < 1.9);
        }
        let roof = shed_roof();
        check_mesh(&roof);
        let b = roof.aabb();
        assert!(b.max.y < 0.06 && b.min.y > -0.06, "{b:?}");
        assert!(b.max.x <= 1.76 && b.max.z <= 1.81, "{b:?}");
    }

    /// Share of a mesh's triangles whose winding faces the way their normals do (front faces
    /// outward; the rest are culled and show what's behind them).
    fn wound_out(m: &Mesh) -> f32 {
        let (mut out, mut total) = (0, 0);
        for t in m.indices.chunks(3) {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            let face = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            let n: Vec3 = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                .sum();
            if face.length_squared() > 1e-14 {
                total += 1;
                out += (face.dot(n) > 0.0) as u32;
            }
        }
        out as f32 / total.max(1) as f32
    }

    #[test]
    fn prop_textures_made_ahead_are_the_ones_used() {
        prefetch_textures();
        let started = std::time::Instant::now();
        while !textures_ready() {
            assert!(started.elapsed().as_secs() < 900, "never finished");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut images = Assets::<Image>::default();
        prop_materials(&mut images);
        assert!(images.loaded_by_key("fire/straw/albedo").is_some());
        // Taken from the store, not made again.
        assert!(take_prefetched("straw").is_none());
    }

    #[test]
    fn props_face_outward() {
        // The bale's sides and one end, and one split face of each log, were wound inside
        // out: from outside they were culled, and a bale showed the fire behind it through
        // its sides like a glass lantern.
        let tree = grow_tree(&TreeParams {
            height: 8.0,
            crown_base: 2.6,
            crown_radius: 2.8,
            trunk_radius: 0.28,
            seed: 7,
        });
        for (name, m) in [
            ("bale", hay_bale(3)),
            ("stones", campfire_stones(0.8, 13, 1)),
            ("firewood", campfire_wood(2)),
            ("shed wall", shed_wall(0)),
            ("shed roof", shed_roof()),
            ("trunk", tree.wood),
        ] {
            let share = wound_out(&m);
            assert!(share > 0.99, "{name}: {share}");
        }
    }

    #[test]
    fn bales_sit_on_the_ground() {
        let m = hay_bale(3);
        check_mesh(&m);
        let b = m.aabb();
        assert!(b.min.y.abs() < 0.012, "{b:?}");
        assert!(b.max.y > 1.0 && b.max.y < 1.3, "{b:?}");
        assert!((b.max.x - BALE_WIDTH * 0.5).abs() < 0.02, "{b:?}");
    }

    #[test]
    fn campfire_stays_inside_its_ring() {
        let stones = campfire_stones(0.78, 12, 1);
        let wood = campfire_wood(2);
        check_mesh(&stones);
        check_mesh(&wood);
        for m in [&stones, &wood] {
            let b = m.aabb();
            assert!(
                b.max.x < 1.1 && b.min.x > -1.1 && b.max.y < 0.6 && b.min.y > -0.1,
                "{b:?}"
            );
        }
    }

    #[test]
    fn foliage_burns_away_with_the_char() {
        assert_eq!(foliage_alpha(0.0), 1.0);
        assert!(foliage_alpha(1.0) < 0.5);
        assert!(
            foliage_alpha(0.45) < 0.5,
            "crowns are bare well before the wood is"
        );
        let base = [1.0, 1.0, 1.0];
        let black = [0.03, 0.025, 0.02];
        assert_eq!(foliage_color(base, black, 0.0), base);
        let scorched = foliage_color(base, black, 0.12);
        assert!(
            scorched[0] > 1.5 && scorched[1] < 1.0,
            "leaves brown first: {scorched:?}"
        );
        let burnt = foliage_color(base, black, 0.8);
        for i in 0..3 {
            assert!((burnt[i] - black[i]).abs() < 1e-4);
        }
        let mut last = 1.0;
        for i in 0..=20 {
            let a = foliage_alpha(i as f32 / 20.0);
            assert!(a <= last);
            last = a;
        }
    }
}
