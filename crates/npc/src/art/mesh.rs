//! Mesh builders for procedural props and architecture.
//!
//! * [`rounded_box`]: a box with rounded edges (cut stone, planks, iron straps), with hidden
//!   faces left out; [`box_lid`] closes the flat middle of one that is only partly hidden.
//! * [`surface`]: any parametric surface `f(u, v)` (cloth, sand, curved panels), with normals
//!   from its derivatives; [`lathe`] turns a profile around Y (columns, drums, bowls, finials)
//!   with an optional angular modulation (flutes, ribs); [`tube`] sweeps a circle along a path
//!   (hoops, rails, scrolls, poles).
//! * [`sdf_mesh`]: signed-distance sculpting meshed only near the surface (chipped stone),
//!   with per-face projected UVs and baked occlusion.
//! * Editing: [`append`] (merge with a transform), [`deform`] (bend or shear, normals follow),
//!   [`displace`] (push along normals), [`smooth_normals`], [`paint`] (vertex colors).
//! * [`bake_par`]: texture baking spread over all cores ([`bake_par_alpha`] with an alpha
//!   channel), [`image_par`] for single images, and [`par_map`].
//!
//! Texture coordinates are in meters unless a builder says otherwise, so tiling materials keep
//! their scale everywhere: set the material's `uv_scale` to one over the meters a texture
//! repeat covers. All meshes wind counter-clockwise (front faces out).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_4, TAU};

use engine_core::prelude::*;
use engine_render::{Image, Mesh, Vertex};

use super::tex::{Texel, TextureSet};

// ------------------------------------------------------------------------------ editing

/// Append `src` transformed by `m` to `dst` (mirroring transforms keep the winding right).
pub fn append(dst: &mut Mesh, src: &Mesh, m: Mat4) {
    let base = dst.vertices.len() as u32;
    let lin = Mat3::from_mat4(m);
    let nm = lin.inverse().transpose();
    dst.vertices.extend(src.vertices.iter().map(|v| {
        let mut v = *v;
        v.position = m.transform_point3(Vec3::from(v.position)).to_array();
        v.normal = (nm * Vec3::from(v.normal)).normalize_or(Vec3::Y).to_array();
        v
    }));
    if lin.determinant() < 0.0 {
        for t in src.indices.as_chunks::<3>().0 {
            dst.indices
                .extend_from_slice(&[t[0] + base, t[2] + base, t[1] + base]);
        }
    } else {
        dst.indices.extend(src.indices.iter().map(|i| i + base));
    }
}

/// `src` transformed by `m`.
pub fn transformed(src: &Mesh, m: Mat4) -> Mesh {
    let mut out = Mesh::default();
    append(&mut out, src, m);
    out
}

/// Move every vertex through `f`. Normals follow the local stretch of the mapping (inverse
/// transpose of its Jacobian), so bends and shears shade correctly; a mapping that mirrors
/// flips the winding back to counter-clockwise.
pub fn deform(m: &mut Mesh, f: impl Fn(Vec3) -> Vec3) {
    let e = 1e-3;
    let mut mirrored = false;
    for (i, v) in m.vertices.iter_mut().enumerate() {
        let p = Vec3::from(v.position);
        let j = Mat3::from_cols(
            (f(p + Vec3::X * e) - f(p - Vec3::X * e)) / (2.0 * e),
            (f(p + Vec3::Y * e) - f(p - Vec3::Y * e)) / (2.0 * e),
            (f(p + Vec3::Z * e) - f(p - Vec3::Z * e)) / (2.0 * e),
        );
        if i == 0 {
            mirrored = j.determinant() < 0.0;
        }
        let n = (j.inverse().transpose() * Vec3::from(v.normal)).normalize_or(Vec3::Y);
        v.position = f(p).to_array();
        v.normal = n.to_array();
    }
    if mirrored {
        for t in m.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
}

/// Push every vertex along its normal by `f(position, normal)` meters, then recompute smooth
/// normals. Vertices that share a position and normal (seams) move together.
pub fn displace(m: &mut Mesh, f: impl Fn(Vec3, Vec3) -> f32) {
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        v.position = (p + n * f(p, n)).to_array();
    }
    smooth_normals(m);
}

/// Area-weighted smooth normals, shared by all vertices at the same position (to 0.1 mm), so
/// UV seams don't show as shading seams.
pub fn smooth_normals(m: &mut Mesh) {
    let key = |p: [f32; 3]| {
        (
            (p[0] * 1e4).round() as i64,
            (p[1] * 1e4).round() as i64,
            (p[2] * 1e4).round() as i64,
        )
    };
    let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::with_capacity(m.vertices.len());
    let group: Vec<u32> = m
        .vertices
        .iter()
        .map(|v| {
            let n = ids.len() as u32;
            *ids.entry(key(v.position)).or_insert(n)
        })
        .collect();
    let mut acc = vec![Vec3::ZERO; ids.len()];
    for t in m.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let pa = Vec3::from(m.vertices[a].position);
        let pb = Vec3::from(m.vertices[b].position);
        let pc = Vec3::from(m.vertices[c].position);
        let face = (pb - pa).cross(pc - pa);
        for i in [a, b, c] {
            acc[group[i] as usize] += face;
        }
    }
    for (v, g) in m.vertices.iter_mut().zip(group) {
        let old = Vec3::from(v.normal);
        v.normal = acc[g as usize].normalize_or(old).to_array();
    }
}

/// Multiply vertex colors by `f(position, normal)` (linear RGBA).
pub fn paint(m: &mut Mesh, f: impl Fn(Vec3, Vec3) -> Vec4) {
    for v in &mut m.vertices {
        let c = Vec4::from(v.color) * f(Vec3::from(v.position), Vec3::from(v.normal));
        v.color = c.to_array();
    }
}

/// Multiply all vertex colors by `c`.
pub fn tint(m: &mut Mesh, c: Vec4) {
    paint(m, |_, _| c);
}

/// Shift texture coordinates (meters) so repeated pieces don't show the same texture patch.
pub fn offset_uv(m: &mut Mesh, d: Vec2) {
    for v in &mut m.vertices {
        v.uv = (Vec2::from(v.uv) + d).to_array();
    }
}

/// Merge several meshes into one.
pub fn merged(parts: impl IntoIterator<Item = Mesh>) -> Mesh {
    let mut out = Mesh::default();
    for p in parts {
        out.merge(&p);
    }
    out
}

// ------------------------------------------------------------------------------ boxes

/// Box faces for [`BoxOpts::skip`].
pub const POS_X: u8 = 1;
pub const NEG_X: u8 = 2;
pub const POS_Y: u8 = 4;
pub const NEG_Y: u8 = 8;
pub const POS_Z: u8 = 16;
pub const NEG_Z: u8 = 32;

/// Options for [`rounded_box`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxOpts {
    /// Edge rounding radius, meters (kept below the smallest half extent).
    pub radius: f32,
    /// Facets across each half of a rounded edge: 1 is a soft chamfer, 2–3 round.
    pub bevel: u32,
    /// Largest grid spacing on the flat parts, meters (0 = no subdivision). Subdivide when the
    /// box will be bent or displaced.
    pub step: f32,
    /// Faces to leave out because they are hidden (`NEG_Y | POS_Z`). The rounded edges along
    /// a left-out face wrap all the way round onto its plane, so where the box sits on (or
    /// against) something no slot opens into its hollow inside.
    pub skip: u8,
}

impl Default for BoxOpts {
    fn default() -> Self {
        Self {
            radius: 0.02,
            bevel: 1,
            step: 0.0,
            skip: 0,
        }
    }
}

impl BoxOpts {
    /// Rounded edges of `radius` meters.
    pub fn round(radius: f32) -> Self {
        Self {
            radius,
            ..Default::default()
        }
    }

    pub fn bevel(mut self, facets: u32) -> Self {
        self.bevel = facets;
        self
    }

    pub fn step(mut self, meters: f32) -> Self {
        self.step = meters;
        self
    }

    pub fn skip(mut self, faces: u8) -> Self {
        self.skip = faces;
        self
    }
}

/// The [`BoxOpts::skip`] bit of the face whose outward normal is the axis direction `d`.
fn face_bit(d: Vec3) -> u8 {
    match (d.x, d.y, d.z) {
        (x, _, _) if x > 0.5 => POS_X,
        (x, _, _) if x < -0.5 => NEG_X,
        (_, y, _) if y > 0.5 => POS_Y,
        (_, y, _) if y < -0.5 => NEG_Y,
        (_, _, z) if z > 0.5 => POS_Z,
        _ => NEG_Z,
    }
}

/// A box with half extents `half`, centered on the origin, with rounded edges and corners.
/// Texture coordinates are per face in meters (looking at a face from outside, u goes right
/// and v down), like the box projection of [`sdf_mesh`].
pub fn rounded_box(half: Vec3, o: BoxOpts) -> Mesh {
    box_faces(half, o, !o.skip, false)
}

/// The flat middles of the left-out `faces` of a [`rounded_box`] with the same `half` and `o`
/// (whose rims wrap round onto them): two triangles (or the step grid) that close a face
/// which is only partly hidden, such as the underside of a coping overhanging its wall.
pub fn box_lid(half: Vec3, o: BoxOpts, faces: u8) -> Mesh {
    box_faces(half, o, faces, true)
}

fn box_faces(half: Vec3, o: BoxOpts, which: u8, flat_only: bool) -> Mesh {
    let half = half.max(Vec3::splat(1e-4));
    let r = o.radius.clamp(0.0, half.min_element() * 0.95);
    let inner = half - Vec3::splat(r);
    let seg = o.bevel.max(1);
    let ticks = |h: f32| -> Vec<f32> {
        let hi = h - r;
        let mut t = Vec::new();
        let bevel = |k: u32| (FRAC_PI_4 * k as f32 / seg as f32).tan() * r;
        if r > 0.0 {
            t.push(-h);
            for k in (1..seg).rev() {
                t.push(-(hi + bevel(k)));
            }
        }
        let n = if o.step > 0.0 {
            ((2.0 * hi) / o.step).ceil().max(1.0) as u32
        } else {
            1
        };
        for k in 0..=n {
            t.push(-hi + 2.0 * hi * k as f32 / n as f32);
        }
        if r > 0.0 {
            for k in 1..seg {
                t.push(hi + bevel(k));
            }
            t.push(h);
        }
        t
    };
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y, POS_X),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y, NEG_X),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z, POS_Y),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z, NEG_Y),
        (Vec3::Z, Vec3::X, Vec3::Y, POS_Z),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y, NEG_Z),
    ];
    let mut m = Mesh::default();
    for (n, u, v, bit) in faces {
        if which & bit == 0 {
            continue;
        }
        let hn = n.abs().dot(half);
        let mut tu = ticks(u.abs().dot(half));
        let mut tv = ticks(v.abs().dot(half));
        if flat_only {
            let (iu, iv) = (u.abs().dot(inner), v.abs().dot(inner));
            tu.retain(|t| t.abs() <= iu + 1e-6);
            tv.retain(|t| t.abs() <= iv + 1e-6);
        }
        let base = m.vertices.len() as u32;
        for &b in &tv {
            for &a in &tu {
                let p = n * hn + u * a + v * b;
                let (pos, normal) = if r > 0.0 {
                    let q = p.clamp(-inner, inner);
                    let d = p - q;
                    // A face covers half of each rounded edge, its neighbor the other half;
                    // next to a left-out neighbor it covers the whole quarter round.
                    let (dn, mut scale_n, mut wrapped) = (d.dot(n), 1.0, d);
                    for (axis, c) in [(u, a), (v, b)] {
                        let toward = axis * c.signum();
                        if c.abs() > axis.abs().dot(inner) + 1e-6 && o.skip & face_bit(toward) != 0
                        {
                            let dt = d.dot(toward);
                            let len = (dn * dn + dt * dt).sqrt();
                            let ang = 2.0 * dt.atan2(dn);
                            scale_n *= ang.cos().max(0.0) * len / dn;
                            wrapped += toward * (len * ang.sin() - dt);
                        }
                    }
                    wrapped += n * (dn * scale_n - dn);
                    let dir = wrapped.normalize_or(n);
                    (q + dir * r, dir)
                } else {
                    (p, n)
                };
                let uv = Vec2::new(pos.dot(u), -pos.dot(v));
                m.vertices.push(Vertex::new(pos, normal, uv));
            }
        }
        let w = tu.len() as u32;
        for j in 0..tv.len() as u32 - 1 {
            for i in 0..w - 1 {
                let a = base + j * w + i;
                let (b, c, d) = (a + 1, a + w + 1, a + w);
                m.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    m
}

// ------------------------------------------------------------------------------ surfaces

fn derivative(f: &dyn Fn(f32) -> Vec3, x: f32, lo: f32, hi: f32, h: f32) -> Vec3 {
    let a = (x - h).max(lo);
    let b = (x + h).min(hi);
    if b - a <= 0.0 {
        return Vec3::ZERO;
    }
    (f(b) - f(a)) / (b - a)
}

/// A parametric surface sampled at `us` × `vs` (both increasing), two triangles per cell.
/// Normals are `∂f/∂u × ∂f/∂v` (so order the parameters such that this points out) taken from
/// `f` itself, which keeps them exact on curved shapes. `uv(u, v, position)` gives texture
/// coordinates.
pub fn surface(
    us: &[f32],
    vs: &[f32],
    f: &dyn Fn(f32, f32) -> Vec3,
    uv: &dyn Fn(f32, f32, Vec3) -> Vec2,
) -> Mesh {
    let mut m = Mesh::default();
    if us.len() < 2 || vs.len() < 2 {
        return m;
    }
    let (u0, u1) = (us[0], us[us.len() - 1]);
    let (v0, v1) = (vs[0], vs[vs.len() - 1]);
    let hu = (u1 - u0).abs() * 1e-4;
    let hv = (v1 - v0).abs() * 1e-4;
    for &v in vs {
        for &u in us {
            let p = f(u, v);
            let normal_at = |u: f32, v: f32| {
                let du = derivative(&|x| f(x, v), u, u0, u1, hu);
                let dv = derivative(&|y| f(u, y), v, v0, v1, hv);
                du.cross(dv)
            };
            let mut n = normal_at(u, v);
            if n.length_squared() < 1e-14 {
                // A pole or a pinched edge: take the normal just inside.
                let inward = |x: f32, lo: f32, hi: f32| {
                    let d = (hi - lo) * 2e-3;
                    if x + d <= hi { x + d } else { x - d }
                };
                n = normal_at(u, inward(v, v0, v1));
                if n.length_squared() < 1e-14 {
                    n = normal_at(inward(u, u0, u1), v);
                }
            }
            m.vertices
                .push(Vertex::new(p, n.normalize_or(Vec3::Y), uv(u, v, p)));
        }
    }
    let w = us.len() as u32;
    for j in 0..vs.len() as u32 - 1 {
        for i in 0..w - 1 {
            let a = j * w + i;
            let (b, c, d) = (a + 1, a + w + 1, a + w);
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    m
}

/// `n + 1` evenly spaced values from `a` to `b`.
pub fn steps(a: f32, b: f32, n: u32) -> Vec<f32> {
    let n = n.max(1);
    (0..=n).map(|k| a + (b - a) * k as f32 / n as f32).collect()
}

/// Turn a profile around the Y axis. `profile` lists `(radius, height)` points; walk up the
/// outside and inward across the top, so the solid is on the left, and the normals point out.
/// Each entry of `spans` is a smooth run of points; neighboring spans meet in a crease.
/// `radial(angle, height, radius)` can reshape the radius (flutes, ribs, dents); angle 0 is
/// +X, increasing toward −Z. Texture u is the angle times `uv_radius` (meters around), v the
/// distance along the profile (meters, going up the texture).
pub fn lathe(
    spans: &[&[Vec2]],
    segments: u32,
    uv_radius: f32,
    radial: &dyn Fn(f32, f32, f32) -> f32,
) -> Mesh {
    let mut m = Mesh::default();
    let us = steps(0.0, TAU, segments.max(3));
    let mut arc0 = 0.0;
    for span in spans {
        if span.len() < 2 {
            continue;
        }
        let mut cum = vec![0.0f32];
        for w in span.windows(2) {
            cum.push(cum[cum.len() - 1] + w[0].distance(w[1]));
        }
        let at = |v: f32| -> (Vec2, f32) {
            let i = (v.floor() as usize).min(span.len() - 2);
            let t = (v - i as f32).clamp(0.0, 1.0);
            (
                span[i].lerp(span[i + 1], t),
                cum[i] + (cum[i + 1] - cum[i]) * t,
            )
        };
        let vs: Vec<f32> = (0..span.len()).map(|i| i as f32).collect();
        let f = |u: f32, v: f32| {
            let (p, _) = at(v);
            let r = radial(u, p.y, p.x);
            Vec3::new(r * u.cos(), p.y, -r * u.sin())
        };
        let a0 = arc0;
        let part = surface(&us, &vs, &f, &|u, v, _| {
            Vec2::new(u * uv_radius, -(a0 + at(v).1))
        });
        arc0 += cum[cum.len() - 1];
        m.merge(&part);
    }
    m
}

/// Points of a quarter-round (or any arc) from angle `a0` to `a1` (radians, 0 = +r) around
/// `center` in the profile plane, for building lathe and sweep profiles.
pub fn arc_points(center: Vec2, radius: f32, a0: f32, a1: f32, n: u32) -> Vec<Vec2> {
    (0..=n.max(1))
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / n.max(1) as f32;
            center + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect()
}

/// A tube of `radius(t)` (t = 0..1 along the path) around a polyline, with `sides` facets.
/// Frames are carried along the path without twisting. `caps` closes the ends with flat discs.
/// Texture u goes around (meters at the first radius), v along the path (meters).
pub fn tube(path: &[Vec3], radius: &dyn Fn(f32) -> f32, sides: u32, caps: bool) -> Mesh {
    let mut m = Mesh::default();
    let n = path.len();
    if n < 2 {
        return m;
    }
    let sides = sides.max(3);
    let tangent = |i: usize| {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).normalize_or(Vec3::Y)
    };
    let t0 = tangent(0);
    let up = if t0.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let mut normal = t0.cross(up).normalize();
    let mut frames = Vec::with_capacity(n);
    let mut prev_t = t0;
    let mut len = 0.0;
    for i in 0..n {
        let t = tangent(i);
        // Parallel transport: rotate the normal by the turn between tangents.
        let q = Quat::from_rotation_arc(prev_t, t);
        normal = (q * normal).normalize();
        prev_t = t;
        if i > 0 {
            len += path[i].distance(path[i - 1]);
        }
        frames.push((t, normal, t.cross(normal), len));
    }
    let r0 = radius(0.0).max(1e-4);
    for (i, &(_, nrm, bin, l)) in frames.iter().enumerate() {
        let s = i as f32 / (n - 1) as f32;
        let r = radius(s);
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * TAU;
            let dir = nrm * a.cos() + bin * a.sin();
            m.vertices
                .push(Vertex::new(path[i] + dir * r, dir, Vec2::new(a * r0, -l)));
        }
    }
    let w = sides + 1;
    for i in 0..n as u32 - 1 {
        for k in 0..sides {
            let a = i * w + k;
            let (b, c, d) = (a + 1, a + w + 1, a + w);
            // Around (k) × along (i) points out.
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    if caps {
        for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
            let (t, nrm, bin, _) = frames[i];
            let r = radius(i as f32 / (n - 1) as f32);
            let center = m.vertices.len() as u32;
            let face = t * sign;
            m.vertices.push(Vertex::new(path[i], face, Vec2::ZERO));
            for k in 0..=sides {
                let a = k as f32 / sides as f32 * TAU;
                let dir = nrm * a.cos() + bin * a.sin();
                m.vertices.push(Vertex::new(
                    path[i] + dir * r,
                    face,
                    Vec2::new(a.cos(), a.sin()) * r,
                ));
            }
            for k in 0..sides {
                let (a, b) = (center + 1 + k, center + 2 + k);
                if sign > 0.0 {
                    m.indices.extend_from_slice(&[center, a, b]);
                } else {
                    m.indices.extend_from_slice(&[center, b, a]);
                }
            }
        }
    }
    m
}

// ------------------------------------------------------------------------------ sdf

/// Options for [`sdf_mesh`].
#[derive(Debug, Clone, Copy)]
pub struct SdfOpts {
    /// Voxel size, meters. Detail finer than about twice this is lost.
    pub voxel: f32,
    /// How strongly crevices darken the vertex color (0 = no baked occlusion).
    pub occlusion: f32,
}

/// Vertex paint for [`sdf_mesh`]: color (linear RGBA, multiplied with the material) and an
/// offset added to the texture coordinates (meters).
pub type SdfPaint<'a> = &'a dyn Fn(Vec3, Vec3) -> (Vec4, Vec2);

const UNSET: f32 = f32::NAN;

/// Mesh the zero surface of the signed distance `f` inside `min..max` with surface nets,
/// evaluating `f` only near the surface (coarse blocks far from it are skipped, so cost grows
/// with area, not volume; `f` should be close to a true distance). Vertices sit on the surface
/// with normals from the field's gradient; each triangle gets texture coordinates projected
/// along its main axis (meters, as [`rounded_box`] faces), plus `paint`'s offset and color,
/// and crevices darken the vertex color.
pub fn sdf_mesh(
    f: &dyn Fn(Vec3) -> f32,
    min: Vec3,
    max: Vec3,
    o: SdfOpts,
    paint: SdfPaint,
) -> Mesh {
    let v = o.voxel.max(1e-4);
    let size = ((max - min) / v).ceil();
    let (nx, ny, nz) = (
        size.x as usize + 3,
        size.y as usize + 3,
        size.z as usize + 3,
    );
    let origin = min - Vec3::splat(v);
    let at = |x: usize, y: usize, z: usize| origin + Vec3::new(x as f32, y as f32, z as f32) * v;
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let mut field = vec![UNSET; nx * ny * nz];
    let get = |field: &mut Vec<f32>, x: usize, y: usize, z: usize| {
        let i = idx(x, y, z);
        let d = field[i];
        if d.is_nan() {
            let d = f(at(x, y, z));
            field[i] = d;
            d
        } else {
            d
        }
    };
    // Coarse blocks of B cells: skip those whose center is farther from the surface than
    // their half diagonal (with a margin for fields that aren't exact distances).
    const B: usize = 4;
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let reach = (B as f32 * v) * 0.5 * 3f32.sqrt() * 1.3 + v * 2.0;
    let mut active: Vec<(usize, usize, usize)> = Vec::new();
    for bz in (0..cz).step_by(B) {
        for by in (0..cy).step_by(B) {
            for bx in (0..cx).step_by(B) {
                let c = at(bx, by, bz) + Vec3::splat(B as f32 * v * 0.5);
                if f(c).abs() <= reach {
                    active.push((bx, by, bz));
                }
            }
        }
    }
    const CORNERS: [(usize, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (1, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (0, 1, 1),
        (1, 1, 1),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let cidx = |x: usize, y: usize, z: usize| (z * cy + y) * cx + x;
    let mut cell_vertex: HashMap<usize, u32> = HashMap::new();
    let mut cells: Vec<(usize, usize, usize)> = Vec::new();
    let mut positions: Vec<Vec3> = Vec::new();
    for &(bx, by, bz) in &active {
        for z in bz..(bz + B).min(cz) {
            for y in by..(by + B).min(cy) {
                for x in bx..(bx + B).min(cx) {
                    let d: [f32; 8] = CORNERS.map(|(a, b, c)| get(&mut field, x + a, y + b, z + c));
                    let inside = d.iter().filter(|&&s| s < 0.0).count();
                    if inside == 0 || inside == 8 {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut n = 0.0;
                    for (a, b) in EDGES {
                        if (d[a] < 0.0) != (d[b] < 0.0) {
                            let t = d[a] / (d[a] - d[b]);
                            let pa = Vec3::new(
                                CORNERS[a].0 as f32,
                                CORNERS[a].1 as f32,
                                CORNERS[a].2 as f32,
                            );
                            let pb = Vec3::new(
                                CORNERS[b].0 as f32,
                                CORNERS[b].1 as f32,
                                CORNERS[b].2 as f32,
                            );
                            sum += pa + (pb - pa) * t;
                            n += 1.0;
                        }
                    }
                    cell_vertex.insert(cidx(x, y, z), positions.len() as u32);
                    cells.push((x, y, z));
                    positions.push(at(x, y, z) + sum / n * v);
                }
            }
        }
    }
    // One quad per sign-changing grid edge; each edge is visited from the lowest of its four
    // cells.
    let mut indices: Vec<u32> = Vec::new();
    let vert = |x: usize, y: usize, z: usize| -> Option<u32> {
        if x >= cx || y >= cy || z >= cz {
            return None;
        }
        cell_vertex.get(&cidx(x, y, z)).copied()
    };
    for &(x, y, z) in &cells {
        let edges = [
            // Along +X from (x, y+1, z+1).
            (
                (x, y + 1, z + 1),
                (x + 1, y + 1, z + 1),
                [(x, y, z), (x, y + 1, z), (x, y + 1, z + 1), (x, y, z + 1)],
                Vec3::X,
            ),
            // Along +Y from (x+1, y, z+1).
            (
                (x + 1, y, z + 1),
                (x + 1, y + 1, z + 1),
                [(x, y, z), (x, y, z + 1), (x + 1, y, z + 1), (x + 1, y, z)],
                Vec3::Y,
            ),
            // Along +Z from (x+1, y+1, z).
            (
                (x + 1, y + 1, z),
                (x + 1, y + 1, z + 1),
                [(x, y, z), (x + 1, y, z), (x + 1, y + 1, z), (x, y + 1, z)],
                Vec3::Z,
            ),
        ];
        for (p0, p1, quad, axis) in edges {
            let s0 = field[idx(p0.0, p0.1, p0.2)];
            let s1 = field[idx(p1.0, p1.1, p1.2)];
            if s0.is_nan() || s1.is_nan() || (s0 < 0.0) == (s1 < 0.0) {
                continue;
            }
            let ids = quad.map(|(a, b, c)| vert(a, b, c));
            let [Some(a), Some(b), Some(c), Some(d)] = ids else {
                continue;
            };
            let out = if s0 < 0.0 { axis } else { -axis };
            let [pa, pb, pc] = [a, b, c].map(|i| positions[i as usize]);
            if (pb - pa).cross(pc - pa).dot(out) >= 0.0 {
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            } else {
                indices.extend_from_slice(&[a, c, b, a, d, c]);
            }
        }
    }
    drop(field);
    // Project onto the surface, normals from the gradient, occlusion.
    let e = v * 0.25;
    let gradient = |p: Vec3| {
        Vec3::new(
            f(p + Vec3::X * e) - f(p - Vec3::X * e),
            f(p + Vec3::Y * e) - f(p - Vec3::Y * e),
            f(p + Vec3::Z * e) - f(p - Vec3::Z * e),
        )
        .normalize_or(Vec3::Y)
    };
    let surf: Vec<(Vec3, Vec3, Vec4, Vec2)> = positions
        .iter()
        .map(|&p0| {
            let mut p = p0;
            for _ in 0..2 {
                let d = f(p);
                if d.abs() > v * 2.0 {
                    break;
                }
                p -= gradient(p) * d;
            }
            let n = gradient(p);
            let mut ao = 1.0;
            if o.occlusion > 0.0 {
                let mut occ = 0.0;
                let mut w = 0.5;
                for k in 1..=4 {
                    let dist = v * 1.5 * k as f32;
                    occ += w * (dist - f(p + n * dist)).max(0.0) / dist;
                    w *= 0.5;
                }
                ao = (1.0 - occ * o.occlusion).clamp(0.3, 1.0);
            }
            let (color, uv_off) = paint(p, n);
            (p, n, color * Vec4::new(ao, ao, ao, 1.0), uv_off)
        })
        .collect();
    // Split vertices by projection axis so no triangle mixes two projections.
    let mut copies = vec![[u32::MAX; 6]; surf.len()];
    let mut m = Mesh::default();
    for t in indices.as_chunks::<3>().0 {
        let [pa, pb, pc] = [t[0], t[1], t[2]].map(|i| surf[i as usize].0);
        let face = (pb - pa).cross(pc - pa);
        let a = face.abs();
        let axis = if a.x >= a.y && a.x >= a.z {
            if face.x >= 0.0 { 0 } else { 1 }
        } else if a.y >= a.z {
            if face.y >= 0.0 { 2 } else { 3 }
        } else if face.z >= 0.0 {
            4
        } else {
            5
        };
        for &i in t {
            let slot = &mut copies[i as usize][axis];
            if *slot == u32::MAX {
                let (p, n, color, off) = surf[i as usize];
                *slot = m.vertices.len() as u32;
                m.vertices
                    .push(Vertex::new(p, n, project_uv(p, axis) + off).with_color(color));
            }
            m.indices.push(*slot);
        }
    }
    m
}

/// Box-projected texture coordinates (meters) along axis `axis` (0 +X, 1 −X, 2 +Y, 3 −Y,
/// 4 +Z, 5 −Z), matching [`rounded_box`] faces.
pub fn project_uv(p: Vec3, axis: usize) -> Vec2 {
    match axis {
        0 => Vec2::new(-p.z, -p.y),
        1 => Vec2::new(p.z, -p.y),
        2 => Vec2::new(p.x, p.z),
        3 => Vec2::new(p.x, -p.z),
        4 => Vec2::new(p.x, -p.y),
        _ => Vec2::new(-p.x, -p.y),
    }
}

// ------------------------------------------------------------------------------ parallel

/// `f(0..n)` computed on all cores, in order.
pub fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .min(n.max(1));
    if threads <= 1 {
        return (0..n).map(f).collect();
    }
    let f = &f;
    // Workers take the next index as they finish, so uneven jobs still balance.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let next = &next;
    let parts: Vec<Vec<(usize, T)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break out;
                        }
                        out.push((i, f(i)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("art worker panicked"))
            .collect()
    });
    let mut out: Vec<Option<T>> = (0..n).map(|_| None).collect();
    for part in parts {
        for (i, x) in part {
            out[i] = Some(x);
        }
    }
    out.into_iter().flatten().collect()
}

fn srgb_exact(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// Linear 0..1 to an sRGB byte through a table (within one step of the exact curve).
fn srgb_u8(linear: f32) -> u8 {
    const N: usize = 8192;
    static TABLE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..N)
            .map(|i| srgb_exact(i as f32 / (N - 1) as f32))
            .collect()
    });
    table[(linear.clamp(0.0, 1.0) * (N - 1) as f32 + 0.5) as usize]
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// An sRGB image with straight alpha, rows computed on all cores: `f(u, v)` (texel centers,
/// 0..1, v down the rows) gives linear RGB and alpha. Its mip chain is built too.
pub fn image_par(w: u32, h: u32, f: impl Fn(Vec2) -> Vec4 + Sync) -> Image {
    let (wn, hn) = (w.max(1) as usize, h.max(1) as usize);
    let rows = par_map(hn, |y| {
        let mut row = Vec::with_capacity(wn * 4);
        for x in 0..wn {
            let c = f(Vec2::new(
                (x as f32 + 0.5) / wn as f32,
                (y as f32 + 0.5) / hn as f32,
            ));
            row.extend_from_slice(&[srgb_u8(c.x), srgb_u8(c.y), srgb_u8(c.z), unorm(c.w)]);
        }
        row
    });
    let mut image = Image::new(w, h, rows.concat(), true);
    image.generate_mips();
    image
}

/// [`TextureSet::bake`] spread over all cores (texels, the three images and their mip
/// chains), with the same encoding: sRGB albedo, a tangent-space normal map from Sobel slopes
/// of the height (green up), and linear ORM.
pub fn bake_par(size: u32, meters: f32, f: impl Fn(Vec2) -> Texel + Sync) -> TextureSet {
    bake_par_alpha(size, meters, |uv| (f(uv), 1.0))
}

/// [`bake_par`] with an alpha channel in the albedo (`f` returns the texel and its alpha), for
/// masked and blended materials.
pub fn bake_par_alpha(
    size: u32,
    meters: f32,
    f: impl Fn(Vec2) -> (Texel, f32) + Sync,
) -> TextureSet {
    // Each row's texels with their alpha, and its heights (for the normal map).
    type Row = (Vec<(Texel, f32)>, Vec<f32>);
    let n = size.max(1) as usize;
    let rows: Vec<Row> = par_map(n, |y| {
        let row: Vec<(Texel, f32)> = (0..n)
            .map(|x| {
                f(Vec2::new(
                    (x as f32 + 0.5) / n as f32,
                    (y as f32 + 0.5) / n as f32,
                ))
            })
            .collect();
        let heights = row.iter().map(|t| t.0.height).collect();
        (row, heights)
    });
    let inv = 1.0 / (8.0 * meters / n as f32);
    let packed = par_map(n, |y| {
        let (mut a, mut nm, mut o) = (
            Vec::with_capacity(n * 4),
            Vec::with_capacity(n * 4),
            Vec::with_capacity(n * 4),
        );
        // Rows above and below (wrapping), for the Sobel slopes.
        let (r0, r1, r2) = (&rows[(y + n - 1) % n].1, &rows[y].1, &rows[(y + 1) % n].1);
        for (x, (t, alpha)) in rows[y].0.iter().enumerate() {
            a.extend_from_slice(&[
                srgb_u8(t.albedo.x),
                srgb_u8(t.albedo.y),
                srgb_u8(t.albedo.z),
                unorm(*alpha),
            ]);
            let (xm, xp) = (
                if x == 0 { n - 1 } else { x - 1 },
                if x + 1 == n { 0 } else { x + 1 },
            );
            let dx = (r0[xp] + 2.0 * r1[xp] + r2[xp] - r0[xm] - 2.0 * r1[xm] - r2[xm]) * inv;
            let dy = (r2[xm] + 2.0 * r2[x] + r2[xp] - r0[xm] - 2.0 * r0[x] - r0[xp]) * inv;
            let nrm = Vec3::new(-dx, dy, 1.0).normalize();
            nm.extend_from_slice(&[
                unorm(nrm.x * 0.5 + 0.5),
                unorm(nrm.y * 0.5 + 0.5),
                unorm(nrm.z * 0.5 + 0.5),
                255,
            ]);
            o.extend_from_slice(&[
                unorm(t.occlusion),
                unorm(t.roughness),
                unorm(t.metallic),
                255,
            ]);
        }
        (a, nm, o)
    });
    drop(rows);
    let mut data = [
        Vec::with_capacity(n * n * 4),
        Vec::with_capacity(n * n * 4),
        Vec::with_capacity(n * n * 4),
    ];
    for (a, nm, o) in packed {
        data[0].extend(a);
        data[1].extend(nm);
        data[2].extend(o);
    }
    let [a, nm, o] = data;
    let mut images = [
        Image::new(size, size, a, true),
        Image::new(size, size, nm, false),
        Image::new(size, size, o, false),
    ];
    std::thread::scope(|s| {
        for image in images.iter_mut() {
            s.spawn(move || image.generate_mips());
        }
    });
    let [albedo, normal, orm] = images;
    TextureSet {
        albedo,
        normal,
        orm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wound_out(m: &Mesh) -> usize {
        // Triangles whose winding disagrees with their vertex normals.
        m.indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| {
                let p = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
                let n: Vec3 = t
                    .iter()
                    .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                    .sum();
                let g = (p[1] - p[0]).cross(p[2] - p[0]);
                g.length_squared() > 1e-14 && g.dot(n) < 0.0
            })
            .count()
    }

    #[test]
    fn rounded_box_is_closed_round_and_wound_out() {
        let half = Vec3::new(0.6, 0.25, 0.4);
        let m = rounded_box(half, BoxOpts::round(0.05).bevel(2).step(0.1));
        assert_eq!(wound_out(&m), 0);
        let b = m.aabb();
        assert!(b.max.abs_diff_eq(half, 1e-4), "{b:?}");
        assert!(b.min.abs_diff_eq(-half, 1e-4), "{b:?}");
        // Corners are rounded off: nothing reaches the sharp corner.
        let far = m
            .vertices
            .iter()
            .map(|v| Vec3::from(v.position).length())
            .fold(0.0, f32::max);
        assert!(far < half.length() - 0.01);
        let skipped = rounded_box(half, BoxOpts::round(0.05).skip(NEG_Y | POS_Z));
        assert!(
            skipped.triangle_count() < rounded_box(half, BoxOpts::round(0.05)).triangle_count()
        );
        // Along the left-out faces the edges wrap onto their planes, so the open rims lie on
        // them (no slot into the hollow box where it sits on something).
        assert_eq!(wound_out(&skipped), 0);
        let b = skipped.aabb();
        assert!(
            b.min.abs_diff_eq(-half, 1e-4) && b.max.abs_diff_eq(half, 1e-4),
            "{b:?}"
        );
        let rim = |p: Vec3| p.y < -half.y + 1e-4 || p.z > half.z - 1e-4;
        let on_rim = skipped
            .vertices
            .iter()
            .filter(|v| rim(Vec3::from(v.position)))
            .count();
        assert!(on_rim >= 12, "{on_rim}");
        let o = BoxOpts::round(0.05).skip(NEG_Y);
        let lid = box_lid(half, o, NEG_Y);
        assert_eq!(lid.triangle_count(), 2);
        assert_eq!(wound_out(&lid), 0);
        assert!(
            lid.vertices
                .iter()
                .all(|v| (v.position[1] + half.y).abs() < 1e-5 && v.normal[1] < -0.999)
        );
    }

    #[test]
    fn bake_par_matches_the_serial_bake() {
        let f = |uv: Vec2| Texel {
            albedo: Vec3::new(uv.x, uv.y, 0.5 * (uv.x + uv.y)),
            height: 0.01 * (uv.x * 12.0).sin() * (uv.y * 7.0).cos() + 0.004 * uv.y * uv.y,
            roughness: uv.x,
            metallic: uv.y,
            occlusion: 1.0 - 0.5 * uv.x * uv.y,
        };
        let (a, b) = (TextureSet::bake(32, 2.0, f), bake_par(32, 2.0, f));
        for (x, y) in [
            (&a.albedo, &b.albedo),
            (&a.normal, &b.normal),
            (&a.orm, &b.orm),
        ] {
            assert_eq!((x.width, x.height, x.srgb), (y.width, y.height, y.srgb));
            // The parallel bake's sRGB table is within one step of the exact curve.
            let worst = x
                .data
                .iter()
                .zip(&y.data)
                .map(|(p, q)| p.abs_diff(*q))
                .max();
            assert!(worst <= Some(1), "{worst:?}");
            assert_eq!(x.mips.len(), y.mips.len());
        }
    }

    #[test]
    fn lathe_and_tube_face_out() {
        let profile = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.5, 0.0),
            Vec2::new(0.5, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let m = lathe(
            &[&profile[..2], &profile[1..3], &profile[2..]],
            24,
            0.5,
            &|_, _, r| r,
        );
        assert_eq!(wound_out(&m), 0);
        for v in &m.vertices {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            if p.y > 0.01 && p.y < 0.99 {
                assert!(
                    n.dot(Vec3::new(p.x, 0.0, p.z).normalize()) > 0.99,
                    "{p} {n}"
                );
            }
        }
        let path: Vec<Vec3> = (0..20)
            .map(|i| {
                let a = i as f32 * 0.3;
                Vec3::new(a.cos(), i as f32 * 0.05, a.sin())
            })
            .collect();
        let t = tube(&path, &|_| 0.05, 8, true);
        assert_eq!(wound_out(&t), 0);
    }

    #[test]
    fn sdf_mesh_matches_the_shape_and_deform_keeps_normals_right() {
        let f = |p: Vec3| p.length() - 0.4;
        let m = sdf_mesh(
            &f,
            Vec3::splat(-0.5),
            Vec3::splat(0.5),
            SdfOpts {
                voxel: 0.03,
                occlusion: 1.0,
            },
            &|_, _| (Vec4::ONE, Vec2::ZERO),
        );
        assert!(m.triangle_count() > 500);
        assert_eq!(wound_out(&m), 0);
        for v in &m.vertices {
            assert!((Vec3::from(v.position).length() - 0.4).abs() < 0.003);
        }
        let mut b = rounded_box(Vec3::splat(0.5), BoxOpts::round(0.1).step(0.1));
        deform(&mut b, |p| Vec3::new(p.x * (1.0 + 0.3 * p.z), p.y, p.z));
        assert_eq!(wound_out(&b), 0);
        displace(&mut b, |p, _| 0.01 * p.x.sin());
        assert_eq!(wound_out(&b), 0);
    }

    #[test]
    fn par_map_keeps_order() {
        let v = par_map(1000, |i| i * 2);
        assert!(v.iter().enumerate().all(|(i, &x)| x == i * 2));
    }
}
