//! Procedural art kit: baked PBR materials (brick, lime plaster, clay pantiles, slate, thatch,
//! timber, stone, canvas, painted steel) and a mesh builder that sorts geometry by material.
//!
//! Everything is generated in code. UVs are in meters, so one material covers every
//! building at the same scale. Vertex colors carry per-piece tint and a ground-occlusion
//! gradient (darker toward the foot of walls), which reads well from the strategy camera's
//! height.

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

use engine::prelude::*;
use engine::render::{Mesh, Vertex};
use engine_ui::demo::art::noise::{cells_xy, fbm_tile, fbm_tile_xy, noised, noised_xy};
use engine_ui::demo::art::textures::{
    canvas_texel, cobble_texel, metal_texel, rock_texel, wood_texel,
};
use engine_ui::demo::art::{
    TexImages, Texel, add_mat, add_set, bake_images, fix_winding, mix3, sstep, textured,
};

/// The materials buildings, ships and airships are made of.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Mat {
    Brick,
    Plaster,
    Timber,
    Planks,
    RoofTile,
    Slate,
    Thatch,
    Stone,
    Cobble,
    Window,
    Iron,
    Canvas,
    SailDark,
    Paint,
    HullDark,
    HullRed,
    Envelope,
    Crop,
    Leaf,
    Wool,
    Clay,
    Lamp,
    Copper,
}

impl Mat {
    pub fn casts_shadows(self) -> bool {
        !matches!(self, Mat::Lamp | Mat::Window)
    }
}

/// Geometry sorted by material, in one object's space.
#[derive(Default, Clone)]
pub struct Parts {
    pub meshes: BTreeMap<Mat, Mesh>,
}

pub fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

impl Parts {
    fn mesh(&mut self, m: Mat) -> &mut Mesh {
        self.meshes
            .entry(m)
            .or_insert_with(|| Mesh::new(Vec::new(), Vec::new()))
    }

    /// Merge another object's parts, moved by `t`.
    pub fn add_parts(&mut self, other: &Parts, t: Mat4) {
        for (m, mesh) in &other.meshes {
            let mut mesh = mesh.clone();
            mesh.transform(&t);
            self.mesh(*m).merge(&mesh);
        }
    }

    pub fn triangles(&self) -> usize {
        self.meshes.values().map(|m| m.triangle_count()).sum()
    }

    /// A flat quad `a b c d` (counter-clockwise seen from its front), UVs in meters along
    /// `a→b` and `a→d`.
    pub fn quad(&mut self, m: Mat, a: Vec3, b: Vec3, c: Vec3, d: Vec3, col: Vec4) {
        let n = (b - a).cross(d - a).normalize_or_zero();
        let u = (b - a).normalize_or_zero();
        let w = n.cross(u);
        let uv = |p: Vec3| Vec2::new((p - a).dot(u), -(p - a).dot(w));
        let mesh = self.mesh(m);
        let base = mesh.vertices.len() as u32;
        for p in [a, b, c, d] {
            mesh.vertices.push(vert(p, n, uv(p), col));
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A triangle (counter-clockwise seen from its front).
    pub fn tri(&mut self, m: Mat, a: Vec3, b: Vec3, c: Vec3, col: Vec4) {
        let n = (b - a).cross(c - a).normalize_or_zero();
        let u = (b - a).normalize_or_zero();
        let w = n.cross(u);
        let uv = |p: Vec3| Vec2::new((p - a).dot(u), -(p - a).dot(w));
        let mesh = self.mesh(m);
        let base = mesh.vertices.len() as u32;
        for p in [a, b, c] {
            mesh.vertices.push(vert(p, n, uv(p), col));
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// An axis-aligned box from `lo` to `hi`, every face with metric UVs (walls: u along the
    /// wall, v down). `bottom`: whether to close the underside.
    pub fn cuboid(&mut self, m: Mat, lo: Vec3, hi: Vec3, col: Vec4, bottom: bool) {
        let (x0, y0, z0) = (lo.x, lo.y, lo.z);
        let (x1, y1, z1) = (hi.x, hi.y, hi.z);
        // +Z, -Z, +X, -X, top.
        self.quad(
            m,
            v3(x0, y0, z1),
            v3(x1, y0, z1),
            v3(x1, y1, z1),
            v3(x0, y1, z1),
            col,
        );
        self.quad(
            m,
            v3(x1, y0, z0),
            v3(x0, y0, z0),
            v3(x0, y1, z0),
            v3(x1, y1, z0),
            col,
        );
        self.quad(
            m,
            v3(x1, y0, z1),
            v3(x1, y0, z0),
            v3(x1, y1, z0),
            v3(x1, y1, z1),
            col,
        );
        self.quad(
            m,
            v3(x0, y0, z0),
            v3(x0, y0, z1),
            v3(x0, y1, z1),
            v3(x0, y1, z0),
            col,
        );
        self.quad(
            m,
            v3(x0, y1, z1),
            v3(x1, y1, z1),
            v3(x1, y1, z0),
            v3(x0, y1, z0),
            col,
        );
        if bottom {
            self.quad(
                m,
                v3(x0, y0, z0),
                v3(x1, y0, z0),
                v3(x1, y0, z1),
                v3(x0, y0, z1),
                col,
            );
        }
    }

    /// A box turned `yaw` about Y around its own middle `c`, half size `h`.
    pub fn box_at(&mut self, m: Mat, c: Vec3, h: Vec3, yaw: f32, col: Vec4) {
        let mut p = Parts::default();
        p.cuboid(m, -h, h, col, true);
        self.add_parts(
            &p,
            Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), c),
        );
    }

    /// A beam of square section `w` from `a` to `b`.
    pub fn beam(&mut self, m: Mat, a: Vec3, b: Vec3, w: f32, col: Vec4) {
        let d = b - a;
        let len = d.length();
        if len < 1e-4 {
            return;
        }
        let rot = Quat::from_rotation_arc(Vec3::Y, d / len);
        let mut p = Parts::default();
        p.cuboid(
            m,
            v3(-w * 0.5, 0.0, -w * 0.5),
            v3(w * 0.5, len, w * 0.5),
            col,
            true,
        );
        self.add_parts(&p, Mat4::from_rotation_translation(rot, a));
    }

    /// A tapered cylinder (or cone when `r1` is 0) on `base`, `segs` sides, standing up `h`.
    #[allow(clippy::too_many_arguments)]
    pub fn cylinder(
        &mut self,
        m: Mat,
        base: Vec3,
        r0: f32,
        r1: f32,
        h: f32,
        segs: u32,
        col: Vec4,
        cap: bool,
    ) {
        let mesh = self.mesh(m);
        let start = mesh.vertices.len() as u32;
        let slope = (r0 - r1) / h.max(1e-4);
        for i in 0..=segs {
            let a = i as f32 / segs as f32 * TAU;
            let (s, c) = a.sin_cos();
            let n = v3(c, slope, s).normalize();
            let u = a * r0.max(r1);
            mesh.vertices.push(vert(
                base + v3(c * r0, 0.0, s * r0),
                n,
                Vec2::new(u, 0.0),
                col,
            ));
            mesh.vertices
                .push(vert(base + v3(c * r1, h, s * r1), n, Vec2::new(u, -h), col));
        }
        for i in 0..segs {
            let a = start + i * 2;
            mesh.indices
                .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
        if cap && r1 > 0.0 {
            let c0 = mesh.vertices.len() as u32;
            mesh.vertices
                .push(vert(base + v3(0.0, h, 0.0), Vec3::Y, Vec2::ZERO, col));
            for i in 0..=segs {
                let a = i as f32 / segs as f32 * TAU;
                let (s, c) = a.sin_cos();
                mesh.vertices.push(vert(
                    base + v3(c * r1, h, s * r1),
                    Vec3::Y,
                    Vec2::new(c * r1, s * r1),
                    col,
                ));
            }
            for i in 0..segs {
                mesh.indices
                    .extend_from_slice(&[c0, c0 + 2 + i, c0 + 1 + i]);
            }
        }
    }

    /// A cylinder between two points.
    pub fn rod(&mut self, m: Mat, a: Vec3, b: Vec3, r: f32, segs: u32, col: Vec4) {
        let d = b - a;
        let len = d.length();
        if len < 1e-4 {
            return;
        }
        let mut p = Parts::default();
        p.cylinder(m, Vec3::ZERO, r, r, len, segs, col, false);
        let rot = Quat::from_rotation_arc(Vec3::Y, d / len);
        self.add_parts(&p, Mat4::from_rotation_translation(rot, a));
    }

    /// A surface of revolution about +X: `profile` is (x, radius) from tail to nose; `segs`
    /// around. For hulls of airships, buoys, barrels lying down.
    pub fn lathe_x(
        &mut self,
        m: Mat,
        profile: &[(f32, f32)],
        segs: u32,
        col: impl Fn(f32, f32) -> Vec4,
    ) {
        let mesh = self.mesh(m);
        let start = mesh.vertices.len() as u32;
        let rows = profile.len();
        for (k, &(x, r)) in profile.iter().enumerate() {
            let (xp, rp) = profile[k.saturating_sub(1)];
            let (xn, rn) = profile[(k + 1).min(rows - 1)];
            let dr = (rn - rp) / (xn - xp).max(1e-4);
            for i in 0..=segs {
                let a = i as f32 / segs as f32 * TAU;
                let (s, c) = a.sin_cos();
                let n = v3(-dr, c, s).normalize();
                mesh.vertices.push(vert(
                    v3(x, c * r, s * r),
                    n,
                    Vec2::new(x, a * 2.0),
                    col(x, a),
                ));
            }
        }
        let w = segs + 1;
        for k in 0..rows as u32 - 1 {
            for i in 0..segs {
                let a = start + k * w + i;
                mesh.indices
                    .extend_from_slice(&[a, a + 1, a + w + 1, a, a + w + 1, a + w]);
            }
        }
    }

    /// A gabled roof over a `w` (across) × `l` (along X) rectangle centred on `c` (its eaves
    /// at `c.y`), `h` high at the ridge, with `over` meters of overhang. The gable triangles
    /// go to `wall`. Ridge along X unless `ridge_z`.
    #[allow(clippy::too_many_arguments)]
    pub fn gable_roof(
        &mut self,
        roof: Mat,
        wall: Mat,
        c: Vec3,
        l: f32,
        w: f32,
        h: f32,
        over: f32,
        ridge_z: bool,
        col: Vec4,
        wall_col: Vec4,
    ) {
        let mut p = Parts::default();
        let (hl, hw) = (l * 0.5 + over, w * 0.5 + over);
        let drop = h / (w * 0.5) * over;
        let t = 0.18;
        // Slopes (+Z and -Z sides), with thickness at the eaves.
        let ridge = h;
        p.quad(
            roof,
            v3(-hl, -drop, hw),
            v3(hl, -drop, hw),
            v3(hl, ridge, 0.0),
            v3(-hl, ridge, 0.0),
            col,
        );
        p.quad(
            roof,
            v3(hl, -drop, -hw),
            v3(-hl, -drop, -hw),
            v3(-hl, ridge, 0.0),
            v3(hl, ridge, 0.0),
            col,
        );
        // Undersides of the overhang.
        let dim = col * Vec4::new(0.55, 0.55, 0.55, 1.0);
        p.quad(
            roof,
            v3(hl, -drop - t, hw),
            v3(-hl, -drop - t, hw),
            v3(-hl, ridge - t, 0.0),
            v3(hl, ridge - t, 0.0),
            dim,
        );
        p.quad(
            roof,
            v3(-hl, -drop - t, -hw),
            v3(hl, -drop - t, -hw),
            v3(hl, ridge - t, 0.0),
            v3(-hl, ridge - t, 0.0),
            dim,
        );
        // Eave fascias.
        p.quad(
            roof,
            v3(-hl, -drop - t, hw),
            v3(hl, -drop - t, hw),
            v3(hl, -drop, hw),
            v3(-hl, -drop, hw),
            dim,
        );
        p.quad(
            roof,
            v3(hl, -drop - t, -hw),
            v3(-hl, -drop - t, -hw),
            v3(-hl, -drop, -hw),
            v3(hl, -drop, -hw),
            dim,
        );
        // Gables (walls), flush with the wall below.
        let (gl, gw) = (l * 0.5, w * 0.5);
        p.tri(
            wall,
            v3(gl, 0.0, gw),
            v3(gl, 0.0, -gw),
            v3(gl, h - h * (over / (w * 0.5 + over)) * 0.0, 0.0),
            wall_col,
        );
        p.tri(
            wall,
            v3(-gl, 0.0, -gw),
            v3(-gl, 0.0, gw),
            v3(-gl, h, 0.0),
            wall_col,
        );
        // Barge boards at the gable ends.
        for sx in [-hl, hl] {
            p.quad(
                roof,
                v3(sx, -drop, hw),
                v3(sx, ridge, 0.0),
                v3(sx, ridge - t, 0.0),
                v3(sx, -drop - t, hw),
                dim,
            );
            p.quad(
                roof,
                v3(sx, ridge, 0.0),
                v3(sx, -drop, -hw),
                v3(sx, -drop - t, -hw),
                v3(sx, ridge - t, 0.0),
                dim,
            );
        }
        // Ridge cap.
        p.beam(
            roof,
            v3(-hl, ridge - 0.05, 0.0),
            v3(hl, ridge - 0.05, 0.0),
            0.22,
            col * Vec4::new(0.8, 0.8, 0.8, 1.0),
        );
        let rot = if ridge_z {
            Quat::from_rotation_y(PI * 0.5)
        } else {
            Quat::IDENTITY
        };
        self.add_parts(&p, Mat4::from_rotation_translation(rot, c));
    }

    /// A hipped roof over `l` × `w` (ridge along X), eaves at `c.y`.
    #[allow(clippy::too_many_arguments)]
    pub fn hip_roof(&mut self, roof: Mat, c: Vec3, l: f32, w: f32, h: f32, over: f32, col: Vec4) {
        let (hl, hw) = (l * 0.5 + over, w * 0.5 + over);
        let drop = h / (w * 0.5) * over;
        let inset = (w * 0.5).min(l * 0.5);
        let rl = (hl - inset - over).max(0.0);
        let a = c + v3(-hl, -drop, hw);
        let b = c + v3(hl, -drop, hw);
        let cc = c + v3(hl, -drop, -hw);
        let d = c + v3(-hl, -drop, -hw);
        let r0 = c + v3(-rl, h, 0.0);
        let r1 = c + v3(rl, h, 0.0);
        self.quad(roof, a, b, r1, r0, col);
        self.quad(roof, cc, d, r0, r1, col);
        self.tri(roof, b, cc, r1, col);
        self.tri(roof, d, a, r0, col);
        if rl > 0.0 {
            self.beam(
                roof,
                r0 - Vec3::Y * 0.05,
                r1 - Vec3::Y * 0.05,
                0.22,
                col * 0.8,
            );
        }
    }

    /// Windows along a wall: `n` openings centred on the wall's middle `c` (facing `normal`),
    /// `w` × `h` each, `gap` apart; dark glass in a light frame with a sill.
    #[allow(clippy::too_many_arguments)]
    pub fn windows(
        &mut self,
        c: Vec3,
        normal: Vec3,
        n: u32,
        w: f32,
        h: f32,
        gap: f32,
        frame: Vec4,
    ) {
        let along = Vec3::Y.cross(normal).normalize();
        for i in 0..n {
            let off = (i as f32 - (n as f32 - 1.0) * 0.5) * (w + gap);
            let m = c + along * off + normal * 0.03;
            let (hw, hh) = (along * w * 0.5, Vec3::Y * h * 0.5);
            self.quad(
                Mat::Window,
                m - hw - hh,
                m + hw - hh,
                m + hw + hh,
                m - hw + hh,
                Vec4::ONE,
            );
            let f = 0.07;
            let out = normal * 0.06;
            // Frame: four bars and a mullion cross.
            self.beam(
                Mat::Paint,
                m - hw - hh + out * 0.5,
                m + hw - hh + out * 0.5,
                f,
                frame,
            );
            self.beam(
                Mat::Paint,
                m - hw + hh + out * 0.5,
                m + hw + hh + out * 0.5,
                f,
                frame,
            );
            self.beam(
                Mat::Paint,
                m - hw - hh + out * 0.5,
                m - hw + hh + out * 0.5,
                f,
                frame,
            );
            self.beam(
                Mat::Paint,
                m + hw - hh + out * 0.5,
                m + hw + hh + out * 0.5,
                f,
                frame,
            );
            self.beam(
                Mat::Paint,
                m - hh + out * 0.3,
                m + hh + out * 0.3,
                f * 0.6,
                frame,
            );
            self.beam(
                Mat::Paint,
                m - hw + out * 0.3,
                m + hw + out * 0.3,
                f * 0.6,
                frame,
            );
            // Sill.
            let s = m - hh - Vec3::Y * 0.05 + normal * 0.08;
            self.box_at(
                Mat::Stone,
                s,
                along * (w * 0.5 + 0.08) + Vec3::Y * 0.04 + normal * 0.1,
                0.0,
                Vec4::splat(0.9).with_w(1.0),
            );
        }
    }

    /// A door in a wall facing `normal`, bottom middle at `at`.
    pub fn door(&mut self, at: Vec3, normal: Vec3, w: f32, h: f32, col: Vec4) {
        let along = Vec3::Y.cross(normal).normalize();
        let m = at + normal * 0.04;
        let hw = along * w * 0.5;
        self.quad(
            Mat::Planks,
            m - hw,
            m + hw,
            m + hw + Vec3::Y * h,
            m - hw + Vec3::Y * h,
            col,
        );
        self.beam(
            Mat::Paint,
            m - hw + normal * 0.03,
            m - hw + Vec3::Y * h + normal * 0.03,
            0.09,
            Vec4::splat(0.85),
        );
        self.beam(
            Mat::Paint,
            m + hw + normal * 0.03,
            m + hw + Vec3::Y * h + normal * 0.03,
            0.09,
            Vec4::splat(0.85),
        );
        self.beam(
            Mat::Paint,
            m - hw + Vec3::Y * h + normal * 0.03,
            m + hw + Vec3::Y * h + normal * 0.03,
            0.09,
            Vec4::splat(0.85),
        );
        // Step.
        self.box_at(
            Mat::Stone,
            at + normal * 0.35 + Vec3::Y * 0.08,
            along * (w * 0.5 + 0.2) + Vec3::Y * 0.08 + normal * 0.3,
            0.0,
            Vec4::splat(0.85),
        );
    }

    /// Darken vertex colors near the ground (y = 0 → `foot`, fading out by `fade_m`).
    pub fn ground_occlusion(&mut self, foot: f32, fade_m: f32) {
        for mesh in self.meshes.values_mut() {
            for v in &mut mesh.vertices {
                let k = foot + (1.0 - foot) * sstep(-0.2, fade_m, v.position[1]);
                for c in &mut v.color[..3] {
                    *c *= k;
                }
            }
        }
    }

    /// Flip triangles to agree with their normals (the builders above are mostly right; this
    /// catches the rest) and return the parts.
    pub fn finish(mut self) -> Self {
        for m in self.meshes.values_mut() {
            fix_winding(m);
        }
        self
    }
}

fn vert(p: Vec3, n: Vec3, uv: Vec2, c: Vec4) -> Vertex {
    Vertex {
        position: p.to_array(),
        normal: n.to_array(),
        uv: uv.to_array(),
        color: c.to_array(),
    }
}

/// A linear color from sRGB hex, with alpha 1.
pub fn hexc(rgb: u32) -> Vec4 {
    engine_ui::demo::art::hex(rgb).extend(1.0)
}

// ----------------------------------------------------------------------------- textures

/// Red brick in running bond, 1.8 m tile: 24 courses of 75 mm, eight bricks a course.
fn brick_texel(u: f32, v: f32) -> Texel {
    let courses = 24.0;
    let per = 8.0;
    let row = (v * courses).floor();
    let fv = (v * courses).fract();
    let shift = if row as i32 % 2 == 0 { 0.0 } else { 0.5 };
    let fu = (u * per + shift).fract();
    let id = ((u * per + shift).floor() as i32).rem_euclid(per as i32) + row as i32 * 17;
    let jh = 0.13;
    let jv = 0.045;
    let joint = 1.0 - sstep(0.0, jh, fv.min(1.0 - fv)) * sstep(0.0, jv, fu.min(1.0 - fu));
    let h = (id as u32).wrapping_mul(2_654_435_761);
    let r = (h & 255) as f32 / 255.0;
    let g = ((h >> 8) & 255) as f32 / 255.0;
    let base = mix3(Vec3::new(0.36, 0.11, 0.06), Vec3::new(0.5, 0.2, 0.1), r);
    let base = mix3(
        base,
        Vec3::new(0.25, 0.12, 0.09),
        (g > 0.86) as i32 as f32 * 0.7,
    );
    let grain = noised(u * 160.0, v * 160.0, 160, 5)[0];
    let soot = fbm_tile(u * 3.0, v * 3.0, 3, 3, 9) * 0.5 + 0.5;
    let mortar = Vec3::new(0.52, 0.48, 0.42) * (0.9 + 0.1 * grain);
    let mut albedo = mix3(base * (0.9 + 0.12 * grain), mortar, joint);
    albedo *= 0.85 + 0.2 * soot;
    Texel {
        albedo,
        height: (1.0 - joint) * 0.006 + grain * 0.0006,
        rough: 0.85 + 0.1 * joint,
        ao: 1.0 - 0.35 * joint,
        ..Default::default()
    }
}

/// Lime plaster, 2 m tile: soft blotches, hairline cracks, a damp-darkened lower band is
/// left to the vertex colors.
fn plaster_texel(u: f32, v: f32) -> Texel {
    let blot = fbm_tile(u * 4.0, v * 4.0, 4, 4, 21);
    let fine = noised(u * 120.0, v * 120.0, 120, 22)[0];
    let (f1, f2, _) = cells_xy(u * 6.0, v * 6.0, 6, 6, 23);
    let crack =
        (1.0 - ((f2 - f1) * 40.0).min(1.0)) * sstep(0.2, 0.5, fbm_tile(u * 2.0, v * 2.0, 2, 2, 24));
    let albedo =
        Vec3::new(0.78, 0.74, 0.65) * (0.9 + 0.12 * blot + 0.03 * fine) * (1.0 - 0.3 * crack);
    Texel {
        albedo,
        height: blot * 0.002 + fine * 0.0004 - crack * 0.001,
        rough: 0.92,
        ..Default::default()
    }
}

/// Clay pantiles, 2 m tile: 8 rows of S-curved tiles, 10 across.
fn pantile_texel(u: f32, v: f32) -> Texel {
    let cols = 10.0;
    let rows = 8.0;
    let fu = (u * cols).fract();
    let fv = (v * rows).fract();
    let col = (u * cols).floor() as i32;
    let row = (v * rows).floor() as i32;
    let wave = (fu * TAU).sin() * 0.5 + 0.5;
    let lap = sstep(0.82, 1.0, fv);
    let h = ((col * 31 + row * 97) as u32).wrapping_mul(2_246_822_519);
    let r = (h & 255) as f32 / 255.0;
    let lichen = sstep(0.35, 0.7, fbm_tile(u * 5.0, v * 5.0, 3, 5, 31)) * 0.6;
    let base = mix3(Vec3::new(0.42, 0.14, 0.06), Vec3::new(0.55, 0.24, 0.1), r);
    let mut albedo = base * (0.75 + 0.35 * wave) * (1.0 - 0.35 * lap);
    albedo = mix3(albedo, Vec3::new(0.32, 0.3, 0.18), lichen * 0.5);
    Texel {
        albedo,
        height: wave * 0.025 - lap * 0.012,
        rough: 0.7 + 0.2 * lichen,
        ao: 0.75 + 0.25 * wave,
        ..Default::default()
    }
}

/// Welsh slate, 2 m tile: 10 courses of slates, 8 across, staggered.
fn slate_texel(u: f32, v: f32) -> Texel {
    let rows = 10.0;
    let row = (v * rows).floor();
    let fv = (v * rows).fract();
    let shift = if row as i32 % 2 == 0 { 0.0 } else { 0.5 };
    let fu = (u * 8.0 + shift).fract();
    let id = (u * 8.0 + shift).floor() as i32 + row as i32 * 13;
    let h = (id as u32).wrapping_mul(2_654_435_761);
    let r = (h & 255) as f32 / 255.0;
    let gap = 1.0 - sstep(0.0, 0.03, fu.min(1.0 - fu));
    let lap = sstep(0.85, 1.0, fv);
    let grain = noised_xy(u * 200.0, v * 40.0, 200, 40, 33)[0];
    let albedo = mix3(Vec3::new(0.08, 0.09, 0.1), Vec3::new(0.14, 0.15, 0.17), r)
        * (0.9 + 0.1 * grain)
        * (1.0 - 0.4 * gap - 0.3 * lap);
    Texel {
        albedo,
        height: fv * 0.006 - gap * 0.003,
        rough: 0.55 + 0.2 * r,
        ..Default::default()
    }
}

/// Reed thatch, 2 m tile: combed strands running down the slope.
fn thatch_texel(u: f32, v: f32) -> Texel {
    let strands = noised_xy(u * 260.0, v * 8.0, 260, 8, 41)[0];
    let clumps = fbm_tile_xy(u * 12.0, v * 3.0, 3, 12, 3, 42);
    let rows = ((v * 12.0).fract() * PI).sin();
    let moss = sstep(0.3, 0.7, fbm_tile(u * 4.0, v * 4.0, 3, 4, 43)) * 0.5;
    let base =
        Vec3::new(0.42, 0.33, 0.18) * (0.75 + 0.25 * strands + 0.15 * clumps) * (0.8 + 0.2 * rows);
    Texel {
        albedo: mix3(base, Vec3::new(0.2, 0.22, 0.1), moss),
        height: strands * 0.01 + clumps * 0.02 + rows * 0.015,
        rough: 0.95,
        ao: 0.7 + 0.3 * rows,
        ..Default::default()
    }
}

/// Crop material: vertex colored, a faint stalk streak.
fn crop_texel(u: f32, v: f32) -> Texel {
    let s = noised_xy(u * 64.0, v * 4.0, 64, 4, 51)[0];
    Texel {
        albedo: Vec3::splat(0.8 + 0.2 * s),
        height: s * 0.002,
        rough: 0.85,
        ..Default::default()
    }
}

/// Doped linen of an airship's envelope, 4 m tile: panels and lacing lines.
fn envelope_texel(u: f32, v: f32) -> Texel {
    let panel = 1.0 - sstep(0.0, 0.012, (u * 8.0).fract().min(1.0 - (u * 8.0).fract()));
    let seam = 1.0 - sstep(0.0, 0.01, (v * 2.0).fract().min(1.0 - (v * 2.0).fract()));
    let weave = noised(u * 300.0, v * 300.0, 300, 61)[0];
    let blot = fbm_tile(u * 2.0, v * 2.0, 3, 2, 62);
    let albedo = Vec3::new(0.62, 0.62, 0.58)
        * (0.93 + 0.05 * weave + 0.06 * blot)
        * (1.0 - 0.15 * panel - 0.1 * seam);
    Texel {
        albedo,
        height: -panel * 0.003 - seam * 0.002 + weave * 0.0003,
        rough: 0.55,
        ..Default::default()
    }
}

/// Stained hull planking, 1 × 2 m tile, horizontal strakes.
fn strake_texel(u: f32, v: f32) -> Texel {
    let t = wood_texel(v, u);
    Texel {
        albedo: t.albedo * 0.8,
        ..t
    }
}

/// The game's materials, made once.
#[derive(Resource, Clone)]
pub struct Mats {
    pub map: BTreeMap<Mat, Handle<StandardMaterial>>,
}

impl Mats {
    pub fn get(&self, m: Mat) -> Handle<StandardMaterial> {
        self.map[&m]
    }
}

/// Bake every texture (on all cores) and make the materials.
pub fn make_mats(world: &mut World) -> Mats {
    type Bake = (Mat, u32, f32, f32, fn(f32, f32) -> Texel);
    let bakes: [Bake; 13] = [
        (Mat::Brick, 512, 1.8, 2.0, brick_texel),
        (Mat::Plaster, 512, 2.0, 2.0, plaster_texel),
        (Mat::RoofTile, 512, 2.0, 1.5, pantile_texel),
        (Mat::Slate, 512, 2.0, 2.0, slate_texel),
        (Mat::Thatch, 512, 2.0, 1.5, thatch_texel),
        (Mat::Timber, 256, 0.5, 2.0, wood_texel),
        (Mat::Planks, 256, 0.5, 2.0, strake_texel),
        (Mat::Stone, 512, 1.6, 1.0, rock_texel),
        (Mat::Cobble, 512, 2.0, 1.0, cobble_texel),
        (Mat::Iron, 256, 1.0, 1.0, metal_texel),
        (Mat::Crop, 128, 1.0, 1.0, crop_texel),
        (Mat::Envelope, 512, 4.0, 1.0, envelope_texel),
        (Mat::Canvas, 256, 1.0, 1.0, |u, v| {
            canvas_texel(
                u,
                v,
                Vec3::new(0.78, 0.74, 0.64),
                Vec3::new(0.72, 0.68, 0.58),
                1.0,
                7,
            )
        }),
    ];
    let images: Vec<(Mat, f32, TexImages)> = engine_ui::demo::art::par_map(bakes.len(), |i| {
        let (m, res, tile, strength, f) = bakes[i];
        let w = res;
        let h = if m == Mat::Timber || m == Mat::Planks {
            res * 2
        } else {
            res
        };
        (m, tile, bake_images(w, h, tile, true, strength, f))
    });
    let mut map = BTreeMap::new();
    for (m, tile, img) in images {
        let set = add_set(world, img);
        let mut mat = textured(&set, 1.0 / tile);
        match m {
            Mat::Canvas => {
                mat.double_sided = true;
                mat.subsurface = 0.35;
            }
            Mat::Crop => {
                mat.double_sided = true;
                mat.subsurface = 0.4;
                mat.wind_sway = 0.6;
            }
            Mat::Envelope => {
                mat.reflectance = 0.45;
            }
            _ => {}
        }
        mat.receives_wetness = true;
        map.insert(m, add_mat(world, mat));
    }
    let plain = |r: f32, g: f32, b: f32, rough: f32, metal: f32| StandardMaterial {
        receives_wetness: true,
        ..StandardMaterial::color(r, g, b)
            .with_roughness(rough)
            .with_metallic(metal)
    };
    map.insert(
        Mat::Window,
        add_mat(
            world,
            StandardMaterial {
                base_color: [0.03, 0.04, 0.05, 1.0],
                perceptual_roughness: 0.08,
                reflectance: 0.9,
                ..Default::default()
            },
        ),
    );
    map.insert(Mat::Paint, add_mat(world, plain(1.0, 1.0, 1.0, 0.55, 0.0)));
    map.insert(
        Mat::HullDark,
        add_mat(world, plain(0.03, 0.03, 0.035, 0.45, 0.2)),
    );
    map.insert(
        Mat::HullRed,
        add_mat(world, plain(0.32, 0.05, 0.03, 0.5, 0.1)),
    );
    map.insert(
        Mat::SailDark,
        add_mat(
            world,
            StandardMaterial {
                base_color: [0.42, 0.2, 0.12, 1.0],
                double_sided: true,
                subsurface: 0.3,
                ..textured_like(&map, Mat::Canvas, world)
            },
        ),
    );
    map.insert(
        Mat::Leaf,
        add_mat(
            world,
            StandardMaterial {
                base_color: [1.0; 4],
                perceptual_roughness: 0.7,
                double_sided: true,
                subsurface: 0.5,
                wind_sway: 0.5,
                ..Default::default()
            },
        ),
    );
    map.insert(
        Mat::Wool,
        add_mat(
            world,
            StandardMaterial {
                base_color: [0.78, 0.75, 0.68, 1.0],
                perceptual_roughness: 1.0,
                subsurface: 0.4,
                ..Default::default()
            },
        ),
    );
    map.insert(
        Mat::Clay,
        add_mat(world, plain(0.42, 0.26, 0.17, 0.95, 0.0)),
    );
    map.insert(
        Mat::Copper,
        add_mat(world, plain(0.25, 0.48, 0.4, 0.5, 0.4)),
    );
    map.insert(
        Mat::Lamp,
        add_mat(world, engine_ui::demo::art::lamp([1.0, 0.7, 0.4], 600.0)),
    );
    Mats { map }
}

fn textured_like(
    map: &BTreeMap<Mat, Handle<StandardMaterial>>,
    m: Mat,
    world: &World,
) -> StandardMaterial {
    world
        .resource::<Assets<StandardMaterial>>()
        .get(map[&m])
        .cloned()
        .unwrap_or_default()
}

/// Small helpers for color math in looks.
pub fn shade(c: Vec4, k: f32) -> Vec4 {
    Vec4::new(c.x * k, c.y * k, c.z * k, c.w)
}
