//! Small plants: grass clumps, fern clumps, meadow flowers, bluebells and mushroom clusters.
//!
//! Each is built from real parts rather than a few flat cards:
//!
//! - **Grass**: dozens of curved, tapered, twisting blades arching out of a tussock, short
//!   filler blades spread over the whole clump, a few dead ones, seed heads on taller culms and
//!   a couple of tuft cards that thicken the base. Blades darken toward the root and some dry
//!   at the tips, so neighboring clumps read as one meadow.
//! - **Ferns**: arching fronds with a rachis and paired, drooping pinnae (each a small card
//!   with a lobed pinna texture) that follow a lanceolate outline, a dead frond lying low, and
//!   unrolling fiddleheads.
//! - **Wildflowers**: daisies, poppies, buttercups and cranesbills with cupped petals around
//!   florets or seed capsules, buds, stem leaves and a rosette of toothed leaves.
//! - **Bluebells**: nodding six-tepalled bells hanging to one side of arching scapes over a
//!   fan of strap leaves.
//! - **Mushrooms**: clusters of lathed caps (bell, dome, flat, button) with gills underneath and
//!   slender stems; lantern caps are tall translucent bells with a ring and a volva. Buttons at
//!   the base go in the wood mesh (no glow).
//!
//! Textures come from [`super::small_textures`]. Vertex colors of grass and ferns are relative
//! to the species' leaf color (the material carries the tint); flowers and mushrooms carry
//! absolute colors over a white material.

use engine_core::math::{Quat, Rng, TAU, Vec2, Vec3, Vec4, hash01, smoothstep};

use super::MeshBuilder;
use super::small_textures::{Cell, cap_uv, fern_stem_uv, gill_uv, pinna_uv, stem_uv};
use super::tree::PlantMeshes;
use crate::species::{PlantForm, PlantSpecies, srgb};

const UP: Vec3 = Vec3::Y;

/// Build one variant of a small plant.
pub fn small(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut m = match sp.form {
        PlantForm::Fern => fern(sp, seed),
        PlantForm::Flower => flowers(sp, seed),
        PlantForm::Mushroom => mushrooms(sp, seed),
        _ => grass(sp, seed),
    };
    m.share_bounds();
    m
}

// ---------------------------------------------------------------- building blocks

/// A vertex of a grid.
#[derive(Debug, Clone, Copy)]
struct Gv {
    p: Vec3,
    n: Vec3,
    uv: Vec2,
    c: Vec4,
}

fn gv(p: Vec3, n: Vec3, uv: Vec2, c: Vec3) -> Gv {
    Gv {
        p,
        n,
        uv,
        c: c.extend(1.0),
    }
}

/// Add a grid of vertices (row-major, `cols` per row). Two-sided grids get both windings with
/// the same normals (soft shading from either side); one-sided grids get the winding that
/// agrees with their normals. Degenerate triangles (pointed tips) are skipped.
fn grid(b: &mut MeshBuilder, v: &[Gv], cols: usize, two_sided: bool) {
    if cols < 2 || v.len() < cols * 2 {
        return;
    }
    let base = b.vertices.len() as u32;
    for g in v {
        b.vertex(g.p, g.n, g.uv, g.c);
    }
    let rows = v.len() / cols;
    let pos = |i: u32| v[(i - base) as usize].p;
    let solid = |i: u32, j: u32, k: u32| (pos(j) - pos(i)).cross(pos(k) - pos(i)).length() > 1e-9;
    let nrm = |i: u32| v[(i - base) as usize].n;
    for r in 0..rows - 1 {
        for k in 0..cols - 1 {
            let a = base + (r * cols + k) as u32;
            let c = cols as u32;
            let q = [a, a + 1, a + 1 + c, a + c];
            for (i, j, l) in [(q[0], q[1], q[2]), (q[0], q[2], q[3])] {
                if !solid(i, j, l) {
                    continue;
                }
                if two_sided {
                    b.tri(i, j, l);
                    b.tri(i, l, j);
                } else {
                    let geo = (pos(j) - pos(i)).cross(pos(l) - pos(i));
                    if geo.dot(nrm(i) + nrm(j) + nrm(l)) >= 0.0 {
                        b.tri(i, j, l);
                    } else {
                        b.tri(i, l, j);
                    }
                }
            }
        }
    }
}

/// Smooth normals of a grid from its own shape. `wrap` grids repeat their first column as the
/// last (lathes). Normals are flipped as a whole so they agree with `prefer(vertex)` on
/// average, then blended toward it by `soft` (0 = geometric).
fn grid_normals(
    v: &mut [Gv],
    cols: usize,
    wrap: bool,
    prefer: &dyn Fn(usize, &Gv) -> Vec3,
    soft: f32,
) {
    let rows = v.len() / cols;
    if rows < 2 || cols < 2 {
        return;
    }
    let mut ns = vec![Vec3::ZERO; v.len()];
    for r in 0..rows {
        for k in 0..cols {
            let (kp, km) = if wrap {
                (
                    if k + 1 < cols { k + 1 } else { 1 },
                    if k > 0 { k - 1 } else { cols - 2 },
                )
            } else {
                ((k + 1).min(cols - 1), k.saturating_sub(1))
            };
            let (rp, rm) = ((r + 1).min(rows - 1), r.saturating_sub(1));
            let across = v[r * cols + kp].p - v[r * cols + km].p;
            let along = v[rp * cols + k].p - v[rm * cols + k].p;
            ns[r * cols + k] = across.cross(along);
        }
    }
    // Rows that collapse to a point (apexes) take their neighbors' normals.
    for r in 0..rows {
        let row = r * cols..(r + 1) * cols;
        if ns[row.clone()].iter().all(|n| n.length_squared() < 1e-14) {
            let src = if r + 1 < rows {
                r + 1
            } else {
                r.saturating_sub(1)
            };
            let avg: Vec3 = (0..cols).map(|k| ns[src * cols + k]).sum::<Vec3>();
            for n in &mut ns[row] {
                *n = avg;
            }
        }
    }
    let agree: f32 = v
        .iter()
        .zip(&ns)
        .enumerate()
        .map(|(i, (g, n))| n.normalize_or_zero().dot(prefer(i, g)))
        .sum();
    let sign = if agree < 0.0 { -1.0 } else { 1.0 };
    for (i, (g, n)) in v.iter_mut().zip(ns).enumerate() {
        let want = prefer(i, g);
        let geo = (n * sign).normalize_or(want);
        g.n = (geo * (1.0 - soft) + want * soft).normalize_or(geo);
    }
}

/// A surface of revolution around the local Y axis, placed at `at` and turned by `rot`.
/// `profile` is (radius, height) per row; `wobble(around, row)` scales the radius per vertex.
#[allow(clippy::too_many_arguments)]
fn lathe(
    b: &mut MeshBuilder,
    at: Vec3,
    rot: Quat,
    profile: &[(f32, f32)],
    sides: u32,
    wobble: &dyn Fn(f32, usize) -> f32,
    uv: &dyn Fn(f32, usize) -> Vec2,
    color: &dyn Fn(usize, f32) -> Vec3,
    outward: bool,
    two_sided: bool,
) {
    let cols = sides as usize + 1;
    let mut v = Vec::with_capacity(cols * profile.len());
    for (i, &(r, y)) in profile.iter().enumerate() {
        for k in 0..cols {
            let a = k as f32 / sides as f32;
            let ang = a * TAU;
            let rr = r * wobble(a % 1.0, i);
            let p = at + rot * Vec3::new(ang.cos() * rr, y, ang.sin() * rr);
            v.push(gv(p, Vec3::ZERO, uv(a, i), color(i, a)));
        }
    }
    let axis = rot * UP;
    let sign = if outward { 1.0 } else { -1.0 };
    let radial = move |_: usize, g: &Gv| {
        let d = g.p - at;
        (d - axis * d.dot(axis)).normalize_or(axis) * sign
    };
    grid_normals(&mut v, cols, true, &radial, 0.0);
    grid(b, &v, cols, two_sided);
}

/// A tube along `path` whose texture coordinates come from `uv(around, along 0..1)`.
fn tube(
    b: &mut MeshBuilder,
    path: &[Vec3],
    radius: &[f32],
    sides: u32,
    uv: &dyn Fn(f32, f32) -> Vec2,
    color: &dyn Fn(usize, f32) -> Vec4,
) {
    let start = b.vertices.len();
    let total: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    b.tube(
        path,
        radius,
        sides,
        1.0 / total.max(1e-5),
        color,
        None,
        None,
    );
    for vtx in &mut b.vertices[start..] {
        let [a, along] = vtx.uv;
        vtx.uv = uv(a, along.min(1.0)).to_array();
    }
}

/// Point and unit tangent at `s` (0..1) along a polyline with evenly spaced samples.
fn along(path: &[Vec3], s: f32) -> (Vec3, Vec3) {
    let n = path.len() - 1;
    let f = (s.clamp(0.0, 1.0) * n as f32).min(n as f32 - 1e-4);
    let i = f as usize;
    let t = (path[i + 1] - path[i]).normalize_or(UP);
    (path[i].lerp(path[i + 1], f - i as f32), t)
}

/// `n` points spread evenly (by index) along a path: the same curve with fewer segments, for
/// far meshes.
fn coarse(path: &[Vec3], n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|i| along(path, i as f32 / (n - 1).max(1) as f32).0)
        .collect()
}

/// A path that starts at `base` heading `a0` radians from vertical toward `out` and bends
/// further by `bend · s^power` along its length.
fn arc(base: Vec3, out: Vec3, len: f32, a0: f32, bend: f32, power: f32, n: usize) -> Vec<Vec3> {
    let sub = 4;
    let ds = len / (n * sub) as f32;
    let mut p = base;
    let mut path = Vec::with_capacity(n + 1);
    path.push(p);
    for j in 0..n {
        for q in 0..sub {
            let s = ((j * sub + q) as f32 + 0.5) / (n * sub) as f32;
            let th = (a0 + bend * s.powf(power)).min(2.0);
            p += (UP * th.cos() + out * th.sin()) * ds;
        }
        p.y = p.y.max(0.004);
        path.push(p);
    }
    path
}

/// A ribbon (grass blade, strap leaf, flower leaf) along a path: `width(s)` full width,
/// `fold` raises the edges (3 columns) for a channeled or keeled leaf, `twist` turns it about
/// its midrib. With `soft = Some(reach)` normals lean toward a rounded clump of that radius.
struct Ribbon<'a> {
    path: &'a [Vec3],
    out: Vec3,
    width: &'a dyn Fn(f32) -> f32,
    twist: f32,
    fold: f32,
    cols: usize,
    cell: Cell,
    span: (f32, f32),
    color: &'a dyn Fn(f32, Vec3) -> Vec3,
    soft: Option<f32>,
}

fn ribbon(b: &mut MeshBuilder, r: &Ribbon) {
    let side0 = UP.cross(r.out).normalize_or(Vec3::X);
    let rows = r.path.len();
    let mut v = Vec::with_capacity(rows * r.cols);
    for (j, &p) in r.path.iter().enumerate() {
        let s = j as f32 / (rows - 1) as f32;
        let t = (r.path[(j + 1).min(rows - 1)] - r.path[j.saturating_sub(1)]).normalize_or(UP);
        let side = Quat::from_axis_angle(t, r.twist * s) * (side0 - t * side0.dot(t));
        let side = side.normalize_or(side0);
        let mut face = side.cross(t).normalize_or(UP);
        if face.y < 0.0 {
            face = -face;
        }
        let half = (r.width)(s) * 0.5;
        let n = match r.soft {
            Some(reach) => {
                let clump = (UP + Vec3::new(p.x, 0.0, p.z) / reach.max(0.05) * 0.8).normalize();
                (face * 0.35 + clump * 0.65).normalize()
            }
            None => face,
        };
        let c = (r.color)(s, p);
        for k in 0..r.cols {
            let x = k as f32 / (r.cols - 1) as f32 * 2.0 - 1.0;
            let lift = face * r.fold * half * x * x;
            let u = r.span.0 + (r.span.1 - r.span.0) * (x * 0.5 + 0.5);
            v.push(gv(p + side * half * x + lift, n, r.cell.uv(u, s), c));
        }
    }
    if r.soft.is_none() && r.cols > 2 {
        let nominal: Vec<Vec3> = v.iter().map(|g| g.n).collect();
        grid_normals(&mut v, r.cols, false, &|i, _| nominal[i], 0.45);
    }
    grid(b, &v, r.cols, true);
}

fn rel(c: [f32; 3], leaf: Vec3) -> Vec3 {
    Vec3::from(c) / leaf
}

// ---------------------------------------------------------------- grass

fn grass(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let reach = sp.crown_radius_m;
    let leaf = Vec3::from(sp.leaf_color).max(Vec3::splat(0.01));
    let dry = rel(sp.accent_color, leaf);
    // Dry tips and dead blades: straw, whatever the species' green.
    let straw = Vec3::from(srgb(0xb7a26c)) / leaf;
    let dead_c = Vec3::from(srgb(0x8f7a55)) / leaf;
    let culm = rel(sp.bark_color, leaf).lerp(straw, 0.25);
    let lean_max = sh.angle_low_deg.to_radians().max(0.05);
    let droop = 1.0 - sh.tip_bend.clamp(-1.0, 1.0) * 0.6;
    let ao = |p: Vec3| 0.26 + 0.74 * smoothstep(0.0, h * 0.55, p.y);
    let mut b = MeshBuilder::new();
    // The far mesh (beyond about 22 m) keeps the mat, the tufts and one blade in six.
    let mut lod = MeshBuilder::new();

    for i in 0..sh.branches.max(8) {
        // A third arch out of a central tussock; the rest rise all over the clump, so
        // neighboring clumps merge into one sward.
        let tussock = i % 3 == 0;
        let spot = if tussock {
            rng.in_unit_disk() * reach * 0.16
        } else {
            rng.in_unit_disk() * reach
        };
        let base = Vec3::new(spot.x, 0.0, spot.y);
        let az = if tussock && spot.length() > reach * 0.03 {
            spot.y.atan2(spot.x) + rng.range(-0.7, 0.7)
        } else {
            rng.range(0.0, TAU)
        };
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let dead = rng.chance(0.06);
        let len = h * if tussock {
            rng.range(0.7, 1.15)
        } else {
            rng.range(0.4, 0.9) * (1.0 - 0.3 * spot.length() / reach)
        };
        let lean = if tussock { 1.25 } else { 1.4 };
        let a0 = rng.f32().powf(1.4) * lean_max * lean * if dead { 1.8 } else { 1.0 } + 0.04;
        let bend = rng.range(0.25, 1.1) * (len / h) * droop * if dead { 1.6 } else { 1.0 };
        let width = sp.trunk_radius_m * rng.range(1.2, 2.6) * if tussock { 1.0 } else { 0.9 };
        let path = arc(base, out, len, a0, bend, 1.6, 5);
        let mut tint = Vec3::ONE.lerp(dry, rng.range(0.0, 0.25)) * rng.range(0.78, 1.12);
        if rng.chance(0.3) {
            tint *= Vec3::new(0.9, 1.0, 1.1);
        }
        let tip_dry = if rng.chance(0.25) {
            rng.range(0.25, 0.8)
        } else {
            0.0
        };
        let color = |s: f32, p: Vec3| {
            let c = if dead {
                dead_c * (0.75 + 0.35 * s)
            } else {
                tint.lerp(straw, tip_dry * smoothstep(0.55, 1.0, s)) * (0.88 + 0.22 * s)
            };
            c * ao(p)
        };
        let w = |s: f32| {
            width
                * (1.0 - s.powf(2.4)).max(0.0).powf(0.65)
                * (0.78 + 0.22 * smoothstep(0.0, 0.2, s))
        };
        ribbon(
            &mut b,
            &Ribbon {
                path: &path,
                out,
                width: &w,
                twist: rng.range(-1.0, 1.0),
                fold: 0.0,
                cols: 2,
                cell: Cell::Blade,
                span: (0.12, 0.88),
                color: &color,
                soft: Some(reach),
            },
        );
        // Far away, one blade in six (tussock and spread alike), coarser and wider: blades
        // are thinner than a pixel there, so their width stands in for the missing ones.
        if i % 12 == 0 || i % 12 == 7 {
            ribbon(
                &mut lod,
                &Ribbon {
                    path: &coarse(&path, 3),
                    out,
                    width: &|s| w(s) * 2.5,
                    twist: 0.0,
                    fold: 0.0,
                    cols: 2,
                    cell: Cell::Blade,
                    span: (0.12, 0.88),
                    color: &color,
                    soft: Some(reach),
                },
            );
        }
    }

    // Flowering culms with airy seed heads above the leaves.
    for _ in 0..rng.range_i32(1, 5) {
        let spot = rng.in_unit_disk() * reach * 0.14;
        let base = Vec3::new(spot.x, 0.0, spot.y);
        let az = rng.range(0.0, TAU);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let hc = h * rng.range(0.95, 1.2);
        let path = arc(
            base,
            out,
            hc,
            rng.range(0.05, 0.3),
            rng.range(0.1, 0.4),
            2.0,
            6,
        );
        let purple = if rng.chance(0.4) {
            Vec3::new(1.12, 0.82, 1.18)
        } else {
            Vec3::ONE
        };
        let color = |s: f32, p: Vec3| culm.lerp(dry, s) * ao(p);
        ribbon(
            &mut b,
            &Ribbon {
                path: &path,
                out,
                width: &|s| 0.0032 * (1.0 - 0.45 * s),
                twist: 0.0,
                fold: 0.0,
                cols: 2,
                cell: Cell::Blade,
                span: (0.4, 0.6),
                color: &color,
                soft: Some(reach),
            },
        );
        ribbon(
            &mut lod,
            &Ribbon {
                path: &coarse(&path, 3),
                out,
                width: &|s| 0.006 * (1.0 - 0.45 * s),
                twist: 0.0,
                fold: 0.0,
                cols: 2,
                cell: Cell::Blade,
                span: (0.4, 0.6),
                color: &color,
                soft: Some(reach),
            },
        );
        let (low, _) = along(&path, 0.7);
        let tip = *path.last().unwrap();
        let wide = hc * rng.range(0.12, 0.17);
        let head = straw * purple * 0.8;
        let clump = (UP + tip.with_y(0.0) / reach).normalize();
        for k in 0..2 {
            let a = Quat::from_rotation_y(k as f32 * 1.57 + az) * Vec3::X * wide * 0.5;
            let corners = [low - a, low + a, tip + a, tip - a];
            let uvs = [
                Cell::SeedHead.uv(0.0, 0.0),
                Cell::SeedHead.uv(1.0, 0.0),
                Cell::SeedHead.uv(1.0, 1.0),
                Cell::SeedHead.uv(0.0, 1.0),
            ];
            let colors = [
                (head * 0.75).extend(1.0),
                (head * 0.75).extend(1.0),
                head.extend(1.0),
                head.extend(1.0),
            ];
            b.card(corners, uvs, clump, colors);
            if k == 0 {
                lod.card(corners, uvs, clump, colors);
            }
        }
    }

    // Tufts of crossed cards scattered over the clump fill the sward between the blades
    // (each card holds dozens of painted blades for four triangles).
    for _ in 0..4 {
        let spot = rng.in_unit_disk() * reach * 0.75;
        let c = Vec3::new(spot.x, 0.0, spot.y);
        let tall = h * rng.range(0.45, 0.7);
        let lean = Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)) * tall * 0.15;
        let tint = Vec3::ONE.lerp(dry, rng.range(0.0, 0.2)) * rng.range(0.85, 1.05);
        let soft = (UP + c / reach * 0.6).normalize();
        let az = rng.range(0.0, TAU);
        let half = reach * rng.range(0.2, 0.3);
        for k in 0..2 {
            let a = Quat::from_rotation_y(az + k as f32 * 1.7) * Vec3::X * half;
            let top = UP * tall + lean;
            let corners = [c - a, c + a, c + a * 1.2 + top, c - a * 1.2 + top];
            let uvs = [
                Cell::Tuft.uv(0.0, 0.0),
                Cell::Tuft.uv(1.0, 0.0),
                Cell::Tuft.uv(1.0, 1.0),
                Cell::Tuft.uv(0.0, 1.0),
            ];
            let colors = [
                (tint * 0.5).extend(1.0),
                (tint * 0.5).extend(1.0),
                tint.extend(1.0),
                tint.extend(1.0),
            ];
            b.card(corners, uvs, soft, colors);
            lod.card(corners, uvs, soft, colors);
        }
    }

    // A low skirt of blades lying over toward the edge hides the soil between clumps.
    let skirt = 6;
    for k in 0..skirt {
        let az = (k as f32 + rng.range(-0.3, 0.3)) / skirt as f32 * TAU;
        let dir = Vec3::new(az.cos(), 0.0, az.sin());
        let side = UP.cross(dir);
        let inner = dir * reach * 0.06 + UP * 0.01;
        let outer = dir * reach * rng.range(0.8, 1.05) + UP * h * rng.range(0.18, 0.32);
        let (w0, w1) = (reach * 0.1, reach * rng.range(0.3, 0.4));
        let tint = Vec3::ONE.lerp(dry, rng.range(0.0, 0.25)) * rng.range(0.85, 1.05);
        let soft = (UP * 1.5 + dir * 0.5).normalize();
        let corners = [
            inner - side * w0,
            inner + side * w0,
            outer + side * w1,
            outer - side * w1,
        ];
        let uvs = [
            Cell::Tuft.uv(0.0, 0.0),
            Cell::Tuft.uv(1.0, 0.0),
            Cell::Tuft.uv(1.0, 1.0),
            Cell::Tuft.uv(0.0, 1.0),
        ];
        let colors = [
            (tint * 0.45).extend(1.0),
            (tint * 0.45).extend(1.0),
            tint.extend(1.0),
            tint.extend(1.0),
        ];
        b.card(corners, uvs, soft, colors);
        lod.card(corners, uvs, soft, colors);
    }

    // A low domed mat of short sward and thatch under the clump. Neighboring clumps' mats
    // overlap into one turf, so a meadow reads as continuous grass instead of tufts on soil.
    let mat = if rng.chance(0.3) {
        Cell::Thatch
    } else {
        Cell::Sward
    };
    let mat_r = reach * rng.range(0.78, 0.92);
    let spin = rng.range(0.0, TAU);
    let mat_tint = Vec3::ONE.lerp(dry, rng.range(0.0, 0.3)) * rng.range(0.82, 1.0);
    let mat_mesh = |b: &mut MeshBuilder, rings: usize, seg: usize| {
        let mut v = Vec::with_capacity(rings * (seg + 1));
        for ring in 0..rings {
            let f = ring as f32 / (rings - 1) as f32;
            for k in 0..=seg {
                let a = k as f32 / seg as f32 * TAU;
                let dir = Vec3::new((a + spin).cos(), 0.0, (a + spin).sin());
                let p = dir * mat_r * f + UP * (0.004 + 0.026 * (1.0 - f * f));
                let uv = mat.uv(0.5 + 0.5 * f * a.cos(), 0.5 + 0.5 * f * a.sin());
                let n = (UP + dir * 0.2 * f).normalize();
                v.push(gv(p, n, uv, mat_tint * (0.78 + 0.22 * f)));
            }
        }
        grid(b, &v, seg + 1, false);
    };
    mat_mesh(&mut b, 3, 12);
    mat_mesh(&mut lod, 2, 8);

    PlantMeshes {
        foliage: Some(b.finish()),
        foliage_lod: Some(lod.finish()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------- ferns

struct Frond {
    az: f32,
    tilt: f32,
    length: f32,
    droop: f32,
    roll: f32,
    tint: Vec3,
    stipe: Vec3,
    dead: bool,
}

fn fern(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let leaf = Vec3::from(sp.leaf_color).max(Vec3::splat(0.01));
    let young = rel(sp.accent_color, leaf);
    let stipe = rel(sp.bark_color, leaf);
    let brown = Vec3::new(0.30, 0.17, 0.065) / leaf;
    let (steep, flat) = (
        sh.angle_high_deg.to_radians(),
        sh.angle_low_deg.to_radians(),
    );
    let mut b = MeshBuilder::new();
    let mut lod = MeshBuilder::new();
    let n = (sh.branches as i32 + rng.range_i32(-2, 3)).max(5);
    for k in 0..n {
        let inner = rng.f32();
        let f = Frond {
            az: k as f32 / n as f32 * TAU + rng.range(-0.3, 0.3),
            tilt: flat + (steep - flat) * inner + rng.range(-0.08, 0.08),
            length: sp.crown_radius_m * rng.range(1.05, 1.35) * (1.0 - 0.15 * inner),
            droop: rng.range(0.5, 0.95) + (-sh.tip_bend).max(0.0) * 0.8,
            roll: rng.range(-0.25, 0.25),
            tint: Vec3::ONE.lerp(young, inner * rng.range(0.2, 0.7)) * rng.range(0.85, 1.1),
            stipe,
            dead: false,
        };
        frond(&mut b, &mut lod, &f, h, &mut rng);
    }
    // Last year's frond, brown and lying low.
    if rng.chance(0.65) {
        let f = Frond {
            az: rng.range(0.0, TAU),
            tilt: rng.range(1.25, 1.45),
            length: sp.crown_radius_m * rng.range(0.9, 1.2),
            droop: 0.35,
            roll: rng.range(-0.5, 0.5),
            tint: brown * rng.range(0.8, 1.2),
            stipe: brown * 0.8,
            dead: true,
        };
        frond(&mut b, &mut lod, &f, h, &mut rng);
    }
    // Fiddleheads unrolling from the crown.
    for _ in 0..rng.range_i32(0, 4) {
        let az = rng.range(0.0, TAU);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let spot = rng.in_unit_disk() * 0.05;
        let height = h * rng.range(0.18, 0.45);
        fiddlehead(
            &mut b,
            Vec3::new(spot.x, 0.0, spot.y),
            out,
            height,
            height * rng.range(0.13, 0.2),
            young * 1.15,
            brown,
            &mut rng,
        );
    }
    PlantMeshes {
        foliage: Some(b.finish()),
        foliage_lod: Some(lod.finish()),
        ..Default::default()
    }
}

fn frond(b: &mut MeshBuilder, lod: &mut MeshBuilder, f: &Frond, max_h: f32, rng: &mut Rng) {
    let out = Vec3::new(f.az.cos(), 0.0, f.az.sin());
    let side = Vec3::new(-f.az.sin(), 0.0, f.az.cos());
    let samples = 16;
    let mut path = arc(
        out * 0.025 + UP * 0.01,
        out,
        f.length,
        f.tilt,
        f.droop,
        1.7,
        samples,
    );
    let top = path.iter().map(|p| p.y).fold(0.0f32, f32::max);
    if top > max_h {
        for p in &mut path {
            p.y *= max_h / top;
        }
    }
    for p in &mut path {
        p.y = p.y.max(0.012);
    }
    let rachis_color = |i: usize, _: f32| {
        let s = i as f32 / samples as f32;
        (f.stipe.lerp(f.tint * 0.9, smoothstep(0.0, 0.5, s)) * (0.55 + 0.45 * s)).extend(1.0)
    };
    let radius: Vec<f32> = (0..=samples)
        .map(|i| 0.0055 * (1.0 - 0.78 * i as f32 / samples as f32))
        .collect();
    tube(b, &path, &radius, 4, &fern_stem_uv, &rachis_color);

    let lamina0 = 0.2;
    let pairs = 22;
    for j in 0..pairs {
        let u = j as f32 / (pairs - 1) as f32;
        let outline = (1.0 - u.powf(1.8)) * (0.55 + 0.45 * smoothstep(0.0, 0.35, u));
        if outline < 0.04 {
            continue;
        }
        for sgn in [-1.0f32, 1.0] {
            if f.dead && rng.chance(0.3) {
                continue;
            }
            let pl = f.length * 0.27 * outline * rng.range(0.9, 1.08);
            let s = lamina0 + (0.985 - lamina0) * (u + sgn * 0.25 / pairs as f32).clamp(0.0, 1.0);
            let (pos, t) = along(&path, s);
            let mut nrm = t.cross(side).normalize_or(UP);
            if nrm.y < 0.0 {
                nrm = -nrm;
            }
            let nrm = Quat::from_axis_angle(t, f.roll) * nrm;
            let toward = nrm.cross(t).normalize_or(side) * sgn;
            let ang = rng.range(0.95, 1.2) - 0.3 * u;
            let d = (t * ang.cos() + toward * ang.sin() + nrm * 0.18).normalize();
            let sag = -(nrm * 0.3 + UP * 0.25) * if f.dead { 2.0 } else { 1.0 };
            let wdir = d.cross(nrm).normalize_or(side);
            let half = pl * 0.27;
            let soft = (nrm * 0.6 + UP * 0.3 + out * 0.15).normalize();
            let shade = 0.58 + 0.42 * s;
            // Pinnae that would sag into the soil lie flat on it instead.
            let rest = |p: Vec3| p.with_y(p.y.max(0.004));
            let mut v = Vec::with_capacity(6);
            for (r, vv) in [0.0f32, 0.5, 1.0].iter().enumerate() {
                let c = pos + d * pl * vv + sag * pl * vv * vv;
                let col = f.tint * shade * (0.86 + 0.22 * vv) * if r == 0 { 0.92 } else { 1.0 };
                v.push(gv(rest(c - wdir * half), soft, pinna_uv(0.0, *vv), col));
                v.push(gv(rest(c + wdir * half), soft, pinna_uv(1.0, *vv), col));
            }
            grid(b, &v, 2, true);
        }
    }

    // The far mesh: the rachis as a flat strip and six pairs of broad pinnae, each widened
    // to close the gaps left by the pinnae it stands in for. No random draws, so the near
    // mesh is the same with or without it.
    let rest = |p: Vec3| p.with_y(p.y.max(0.004));
    let far_path = coarse(&path, 4);
    let mut v = Vec::with_capacity(8);
    for (i, &p) in far_path.iter().enumerate() {
        let s = i as f32 / (far_path.len() - 1) as f32;
        let w = 0.009 * (1.0 - 0.7 * s);
        let col = rachis_color((s * samples as f32) as usize, 0.0).truncate();
        let n = (UP + out * 0.3).normalize();
        v.push(gv(p - side * w, n, fern_stem_uv(0.0, s), col));
        v.push(gv(p + side * w, n, fern_stem_uv(1.0, s), col));
    }
    grid(lod, &v, 2, true);
    let rachis_len: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    let far_pairs = 6;
    let gap = rachis_len * (0.985 - lamina0) / far_pairs as f32;
    for j in 0..far_pairs {
        let u = (j as f32 + 0.5) / far_pairs as f32;
        let outline = (1.0 - u.powf(1.8)) * (0.55 + 0.45 * smoothstep(0.0, 0.35, u));
        let pl = f.length * 0.27 * outline;
        let s = lamina0 + (0.985 - lamina0) * u;
        let (pos, t) = along(&path, s);
        let mut nrm = t.cross(side).normalize_or(UP);
        if nrm.y < 0.0 {
            nrm = -nrm;
        }
        let nrm = Quat::from_axis_angle(t, f.roll) * nrm;
        let ang = 1.075 - 0.3 * u;
        let sag = -(nrm * 0.3 + UP * 0.25) * if f.dead { 2.0 } else { 1.0 };
        let soft = (nrm * 0.6 + UP * 0.3 + out * 0.15).normalize();
        let shade = 0.58 + 0.42 * s;
        let half = (pl * 0.27).max(gap * ang.sin() * 0.55);
        for sgn in [-1.0f32, 1.0] {
            let toward = nrm.cross(t).normalize_or(side) * sgn;
            let d = (t * ang.cos() + toward * ang.sin() + nrm * 0.18).normalize();
            let wdir = d.cross(nrm).normalize_or(side);
            let mut v = Vec::with_capacity(4);
            for vv in [0.0f32, 1.0] {
                let c = pos + d * pl * vv + sag * pl * vv * vv;
                let col = f.tint * shade * (0.84 + 0.22 * vv);
                v.push(gv(rest(c - wdir * half), soft, pinna_uv(0.0, vv), col));
                v.push(gv(rest(c + wdir * half), soft, pinna_uv(1.0, vv), col));
            }
            grid(lod, &v, 2, true);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fiddlehead(
    b: &mut MeshBuilder,
    base: Vec3,
    out: Vec3,
    height: f32,
    coil: f32,
    fresh: Vec3,
    scale: Vec3,
    rng: &mut Rng,
) {
    let mut path: Vec<Vec3> = (0..=6)
        .map(|i| {
            let s = i as f32 / 6.0;
            base + UP * height * s + out * height * 0.1 * s * s
        })
        .collect();
    let top = *path.last().unwrap();
    let center = top + out * coil;
    let turns = rng.range(1.6, 2.0) * std::f32::consts::PI;
    let n = 16;
    for i in 1..=n {
        let phi = i as f32 / n as f32 * turns;
        let r = coil * (1.0 - phi / (turns * 1.25));
        path.push(center + (-out * phi.cos() + UP * phi.sin()) * r);
    }
    let rings = path.len();
    let radius: Vec<f32> = (0..rings)
        .map(|i| {
            if i <= 6 {
                0.0055
            } else {
                let f = (i - 6) as f32 / (rings - 7) as f32;
                0.0075 * (1.0 - 0.6 * f)
            }
        })
        .collect();
    let seed = rng.next_u32() as i64;
    let color = |i: usize, a: f32| {
        let spot = hash01(i as i64, (a * 5.0) as i64, seed, 9) < 0.35;
        let c = if spot { fresh.lerp(scale, 0.65) } else { fresh };
        let shade = if i > 6 {
            1.0
        } else {
            0.6 + 0.4 * i as f32 / 6.0
        };
        (c * shade).extend(1.0)
    };
    tube(b, &path, &radius, 6, &fern_stem_uv, &color);
}

// ---------------------------------------------------------------- flowers

fn flowers(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut b = MeshBuilder::new();
    let mut lod = MeshBuilder::new();
    if sp.shape.tip_bend < -0.3 {
        bluebells(sp, seed, &mut b, &mut lod);
    } else {
        meadow_flowers(sp, seed, &mut b, &mut lod);
    }
    PlantMeshes {
        foliage: Some(b.finish()),
        foliage_lod: Some(lod.finish()),
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Head {
    Daisy,
    Poppy,
    Buttercup,
    Cranesbill,
}

/// A thin three-sided stem along `path`, darker at its foot.
fn stem(b: &mut MeshBuilder, path: &[Vec3], r0: f32, r1: f32, color: Vec3) {
    let n = path.len();
    let radius: Vec<f32> = (0..n)
        .map(|i| r0 + (r1 - r0) * i as f32 / (n - 1) as f32)
        .collect();
    let col = |i: usize, _: f32| (color * (0.55 + 0.45 * i as f32 / (n - 1) as f32)).extend(1.0);
    tube(
        b,
        path,
        &radius,
        3,
        &|a, t| Cell::Blade.uv(0.3 + 0.4 * a, t),
        &col,
    );
}

fn meadow_flowers(sp: &PlantSpecies, seed: u64, b: &mut MeshBuilder, lod: &mut MeshBuilder) {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let reach = sp.crown_radius_m;
    let stem_c = Vec3::from(sp.bark_color);
    let leaf_c = stem_c * Vec3::new(0.8, 0.92, 0.75);

    // A rosette of toothed leaves.
    for _ in 0..rng.range_i32(5, 9) {
        let az = rng.range(0.0, TAU);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let len = h * rng.range(0.24, 0.42);
        let path = arc(
            out * 0.01,
            out,
            len,
            rng.range(0.7, 1.15),
            rng.range(0.5, 0.9),
            1.5,
            4,
        );
        let tint = leaf_c * rng.range(0.8, 1.1);
        let w = len * rng.range(0.3, 0.38);
        let color = |s: f32, p: Vec3| {
            tint * (0.55 + 0.45 * smoothstep(0.0, h * 0.3, p.y)) * (0.9 + 0.15 * s)
        };
        ribbon(
            b,
            &Ribbon {
                path: &path,
                out,
                width: &|_| w,
                twist: rng.range(-0.3, 0.3),
                fold: 0.35,
                cols: 3,
                cell: Cell::Leaf,
                span: (0.0, 1.0),
                color: &color,
                soft: None,
            },
        );
        ribbon(
            lod,
            &Ribbon {
                path: &coarse(&path, 3),
                out,
                width: &|_| w,
                twist: 0.0,
                fold: 0.0,
                cols: 2,
                cell: Cell::Leaf,
                span: (0.0, 1.0),
                color: &color,
                soft: None,
            },
        );
    }

    let n = (sh.branches as i32 + rng.range_i32(-2, 3)).max(4);
    let white = Vec3::from(sp.leaf_color);
    let red = Vec3::from(sp.accent_color);
    for _ in 0..n {
        let spot = rng.in_unit_disk() * reach * 0.45;
        let base = Vec3::new(spot.x, 0.0, spot.y);
        let az = if spot.length() > 0.01 {
            spot.y.atan2(spot.x) + rng.range(-0.5, 0.5)
        } else {
            rng.range(0.0, TAU)
        };
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let height = h * rng.range(0.5, 1.0);
        let lean = rng.range(0.04, 0.3);
        let path = arc(base, out, height, lean, rng.range(-0.25, 0.2), 1.5, 6);
        let (top, tangent) = (*path.last().unwrap(), along(&path, 0.95).1);
        stem(b, &path, 0.0024, 0.0015, stem_c);
        stem(lod, &[path[0], top], 0.004, 0.003, stem_c);
        // A leaf or two on the stem.
        for _ in 0..rng.range_i32(0, 3) {
            let (at, _) = along(&path, rng.range(0.15, 0.5));
            let laz = rng.range(0.0, TAU);
            let lout = Vec3::new(laz.cos(), 0.0, laz.sin());
            let len = rng.range(0.035, 0.065);
            let lp = arc(at, lout, len, rng.range(0.5, 0.9), 0.5, 1.5, 3);
            let tint = leaf_c * rng.range(0.9, 1.15);
            ribbon(
                b,
                &Ribbon {
                    path: &lp,
                    out: lout,
                    width: &|_| len * 0.32,
                    twist: 0.0,
                    fold: 0.3,
                    cols: 3,
                    cell: Cell::Leaf,
                    span: (0.0, 1.0),
                    color: &|_, _| tint,
                    soft: None,
                },
            );
        }
        let pick = rng.f32();
        let kind = if pick < 0.38 {
            Head::Daisy
        } else if pick < 0.66 {
            Head::Poppy
        } else if pick < 0.83 {
            Head::Buttercup
        } else {
            Head::Cranesbill
        };
        // Heads turn sideways toward the light rather than straight up.
        let face = (tangent * 0.5 + UP * 0.35 + out * 0.45).normalize();
        let size = sh.leaf_size_m * rng.range(0.85, 1.25);
        if rng.chance(0.15) {
            bud(b, top, tangent, size * 0.4, stem_c, red.lerp(white, 0.5));
        } else {
            let petal = match kind {
                Head::Daisy => white,
                Head::Poppy => red,
                Head::Buttercup => Vec3::from(srgb(0xf2c014)),
                Head::Cranesbill => Vec3::from(srgb(0x8a62c4)),
            } * rng.range(0.88, 1.08);
            flower_head(b, kind, top, face, size, petal, &mut rng);
            far_head(lod, kind, top, face, size, petal);
        }
        // Some stems carry a bud on a side shoot.
        if rng.chance(0.3) {
            let (at, t) = along(&path, rng.range(0.55, 0.8));
            let dir = (t + Vec3::new(rng.range(-1.0, 1.0), 0.3, rng.range(-1.0, 1.0))).normalize();
            let tip = at + dir * height * rng.range(0.12, 0.2);
            stem(
                b,
                &[at, at.lerp(tip, 0.5) + UP * 0.01, tip],
                0.0015,
                0.0012,
                stem_c,
            );
            bud(b, tip, dir, size * 0.32, stem_c, white.lerp(red, rng.f32()));
        }
    }
}

/// A closed bud: green sepals with the petal color showing at the tip.
fn bud(b: &mut MeshBuilder, at: Vec3, dir: Vec3, size: f32, green: Vec3, petal: Vec3) {
    let rot = Quat::from_rotation_arc(UP, dir.normalize_or(UP));
    let profile = [
        (0.15, -0.1),
        (0.42, 0.15),
        (0.55, 0.5),
        (0.45, 0.85),
        (0.2, 1.08),
        (0.0, 1.15),
    ];
    let p: Vec<(f32, f32)> = profile.iter().map(|(r, y)| (r * size, y * size)).collect();
    lathe(
        b,
        at,
        rot,
        &p,
        6,
        &|_, _| 1.0,
        &|a, i| Cell::Blade.uv(0.2 + 0.6 * a, i as f32 / 5.0),
        &|i, _| green.lerp(petal, smoothstep(2.0, 5.0, i as f32)) * 0.9,
        true,
        false,
    );
}

/// Petals around a center: `size` is the head radius.
fn flower_head(
    b: &mut MeshBuilder,
    kind: Head,
    at: Vec3,
    face: Vec3,
    size: f32,
    petal: Vec3,
    rng: &mut Rng,
) {
    let u = face.any_orthonormal_vector();
    let w = face.cross(u);
    let rot = Quat::from_rotation_arc(UP, face);
    let phase = rng.range(0.0, TAU);
    // (petals, length, width / length, rise, curl, cup, base color, cell, columns)
    let dark = Vec3::new(0.02, 0.015, 0.02);
    let (count, len, wide, rise, curl, cup, base_c, cell, cols) = match kind {
        Head::Daisy => (
            rng.range_i32(17, 24),
            size,
            0.24,
            0.18,
            0.45,
            0.12,
            petal * Vec3::new(0.95, 0.95, 0.75),
            Cell::PetalNarrow,
            2,
        ),
        Head::Poppy => (
            4,
            size * 1.25,
            1.05,
            0.75,
            -0.35,
            0.35,
            dark,
            Cell::PetalBroad,
            3,
        ),
        Head::Buttercup => (
            5,
            size * 0.62,
            0.95,
            0.85,
            -0.1,
            0.3,
            petal * Vec3::new(0.7, 0.75, 0.3),
            Cell::PetalBroad,
            3,
        ),
        Head::Cranesbill => (
            5,
            size * 0.75,
            0.85,
            0.25,
            0.1,
            0.15,
            petal.lerp(Vec3::splat(0.85), 0.6),
            Cell::PetalBroad,
            3,
        ),
    };
    for k in 0..count {
        let ang = phase + k as f32 / count as f32 * TAU + rng.range(-0.08, 0.08);
        let dir = u * ang.cos() + w * ang.sin();
        let l = len * rng.range(0.88, 1.06);
        // Poppies: the inner pair stands taller.
        let rise = rise
            + if kind == Head::Poppy && k % 2 == 1 {
                0.2
            } else {
                0.0
            };
        petal_mesh(
            b,
            at + dir * size * 0.12 + face * size * 0.02,
            dir,
            face,
            l,
            l * wide,
            rise + rng.range(-0.08, 0.08),
            curl + rng.range(-0.1, 0.1),
            cup,
            cell,
            cols,
            (base_c, petal * rng.range(0.92, 1.05)),
        );
    }
    let s = size;
    match kind {
        Head::Daisy => {
            let disc = [
                (0.0, 0.17),
                (0.12, 0.16),
                (0.21, 0.12),
                (0.27, 0.06),
                (0.28, 0.0),
                (0.2, -0.06),
                (0.06, -0.12),
            ];
            let yellow = Vec3::from(srgb(0xf0ad16));
            let green = Vec3::from(srgb(0x4f6b2a));
            lathe(
                b,
                at,
                rot,
                &disc.map(|(r, y)| (r * s, y * s)),
                10,
                &|_, _| 1.0,
                &|a, i| Cell::Florets.uv(a, i as f32 / 6.0),
                &|i, _| match i {
                    0 | 1 => yellow * 0.7,
                    2..=4 => yellow,
                    _ => green,
                },
                true,
                false,
            );
        }
        Head::Poppy => {
            let capsule = [
                (0.0, 0.42),
                (0.1, 0.41),
                (0.15, 0.36),
                (0.15, 0.25),
                (0.12, 0.1),
                (0.05, 0.02),
            ];
            let green = Vec3::from(srgb(0x6f8455));
            lathe(
                b,
                at,
                rot,
                &capsule.map(|(r, y)| (r * s, y * s)),
                8,
                &|a, i| {
                    if i < 2 {
                        1.0 + 0.25 * (a * 8.0 * TAU).cos()
                    } else {
                        1.0
                    }
                },
                &|a, i| Cell::Florets.uv(a, i as f32 / 5.0),
                &|i, _| if i < 2 { green * 0.55 } else { green },
                true,
                false,
            );
            let ring = [(0.13, 0.22), (0.26, 0.2), (0.3, 0.12), (0.18, 0.06)];
            lathe(
                b,
                at,
                rot,
                &ring.map(|(r, y)| (r * s, y * s)),
                10,
                &|_, _| 1.0,
                &|a, i| Cell::Florets.uv(a, 0.3 + i as f32 / 6.0),
                &|_, _| Vec3::new(0.035, 0.03, 0.035),
                true,
                false,
            );
        }
        Head::Buttercup | Head::Cranesbill => {
            let dome = [(0.0, 0.14), (0.1, 0.12), (0.17, 0.06), (0.18, 0.0)];
            let c = if kind == Head::Buttercup {
                Vec3::from(srgb(0xc9b02a))
            } else {
                Vec3::from(srgb(0xe6dcef))
            };
            lathe(
                b,
                at,
                rot,
                &dome.map(|(r, y)| (r * s, y * s)),
                8,
                &|_, _| 1.0,
                &|a, i| Cell::Florets.uv(a, i as f32 / 4.0),
                &|_, _| c,
                true,
                false,
            );
        }
    }
}

/// A flower head for the far mesh: a hexagon of petal color, shading to the head's center
/// color, seen from both sides (12 triangles).
fn far_head(b: &mut MeshBuilder, kind: Head, at: Vec3, face: Vec3, size: f32, petal: Vec3) {
    let (radius, center) = match kind {
        Head::Daisy => (size * 1.05, Vec3::from(srgb(0xf0ad16))),
        Head::Poppy => (size * 1.15, petal * 0.35),
        Head::Buttercup => (size * 0.65, Vec3::from(srgb(0xc9b02a))),
        Head::Cranesbill => (size * 0.78, petal.lerp(Vec3::splat(0.85), 0.6)),
    };
    let u = face.any_orthonormal_vector();
    let w = face.cross(u);
    let sides = 6;
    let mut v = Vec::with_capacity(2 * (sides + 1));
    for ring in 0..2 {
        let r = ring as f32;
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * TAU;
            let p = at + face * size * 0.08 + (u * a.cos() + w * a.sin()) * radius * r;
            let uv = Cell::Florets.uv(0.5 + 0.4 * r * a.cos(), 0.5 + 0.4 * r * a.sin());
            v.push(gv(p, face, uv, center.lerp(petal, r)));
        }
    }
    grid(b, &v, sides + 1, true);
}

/// One petal from `at` outward along `dir`, rising by `rise` (radians) above the head plane
/// and curling back by `curl` toward the tip; `cup` hollows it across.
#[allow(clippy::too_many_arguments)]
fn petal_mesh(
    b: &mut MeshBuilder,
    at: Vec3,
    dir: Vec3,
    face: Vec3,
    len: f32,
    width: f32,
    rise: f32,
    curl: f32,
    cup: f32,
    cell: Cell,
    cols: usize,
    (base_c, tip_c): (Vec3, Vec3),
) {
    let across = dir.cross(face).normalize_or(Vec3::X);
    let rows = 3;
    let mut v = Vec::with_capacity(rows * cols);
    let mut c = at;
    for r in 0..rows {
        let t = r as f32 / (rows - 1) as f32;
        if r > 0 {
            let ang = rise - curl * t;
            c += (dir * ang.cos() + face * ang.sin()) * len / (rows - 1) as f32;
        }
        let col = base_c.lerp(tip_c, smoothstep(0.0, 0.55, t));
        for k in 0..cols {
            let s = k as f32 / (cols - 1) as f32;
            let x = s * 2.0 - 1.0;
            let p = c + across * x * width * 0.5 + face * cup * x * x * width * 0.5 * t.min(0.6);
            v.push(gv(p, face, cell.uv(s, t), col));
        }
    }
    grid_normals(&mut v, cols, false, &|_, _| face, 0.5);
    grid(b, &v, cols, true);
}

fn bluebells(sp: &PlantSpecies, seed: u64, b: &mut MeshBuilder, lod: &mut MeshBuilder) {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let stem_c = Vec3::from(sp.bark_color);
    let leaf_c = stem_c * Vec3::new(0.68, 0.85, 0.72);
    let petal = Vec3::from(sp.leaf_color);
    let accent = Vec3::from(sp.accent_color);

    // Glossy strap leaves arching out to the ground.
    for _ in 0..rng.range_i32(6, 10) {
        let az = rng.range(0.0, TAU);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let spot = rng.in_unit_disk() * 0.03;
        let len = h * rng.range(0.75, 1.15);
        let path = arc(
            Vec3::new(spot.x, 0.0, spot.y),
            out,
            len,
            rng.range(0.25, 0.7),
            rng.range(0.9, 1.5),
            1.4,
            5,
        );
        let wid = rng.range(0.011, 0.016);
        let tint = leaf_c * rng.range(0.85, 1.1);
        let width = |s: f32| wid * (0.7 + 0.3 * smoothstep(0.0, 0.2, s)) * (1.0 - s.powi(4)).sqrt();
        let color = |s: f32, p: Vec3| {
            tint * (0.45 + 0.55 * smoothstep(0.0, h * 0.4, p.y)) * (0.9 + 0.2 * s)
        };
        ribbon(
            b,
            &Ribbon {
                path: &path,
                out,
                width: &width,
                twist: rng.range(-0.4, 0.4),
                fold: 0.45,
                cols: 3,
                cell: Cell::Blade,
                span: (0.1, 0.9),
                color: &color,
                soft: None,
            },
        );
        ribbon(
            lod,
            &Ribbon {
                path: &coarse(&path, 3),
                out,
                width: &|s| width(s) * 1.3,
                twist: 0.0,
                fold: 0.0,
                cols: 2,
                cell: Cell::Blade,
                span: (0.1, 0.9),
                color: &color,
                soft: None,
            },
        );
    }

    let n = (sh.branches as i32 - 3 + rng.range_i32(-1, 2)).max(3);
    for _ in 0..n {
        let spot = rng.in_unit_disk() * sp.crown_radius_m * 0.35;
        let base = Vec3::new(spot.x, 0.0, spot.y);
        let az = rng.range(0.0, TAU);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let side = UP.cross(out);
        let hs = h * rng.range(0.78, 1.05);
        // The scape rises, then the top third arches over (bells hang on the outer side).
        let crook = rng.range(1.0, 1.6);
        let lean = rng.range(0.03, 0.2);
        let samples = 9;
        let mut path = Vec::with_capacity(samples + 1);
        let mut p = base;
        path.push(p);
        let ds = hs * 1.05 / samples as f32;
        for i in 0..samples {
            let s = (i as f32 + 0.5) / samples as f32;
            let th = lean + crook * smoothstep(0.45, 1.0, s);
            p += (UP * th.cos() + out * th.sin()) * ds;
            path.push(p);
        }
        stem(b, &path, 0.0028, 0.0015, stem_c);
        stem(lod, &coarse(&path, 4), 0.004, 0.0025, stem_c);
        let tint = rng.range(0.88, 1.1);
        let hue = petal.lerp(accent, rng.range(0.0, 0.4)) * tint;
        let bells = rng.range_i32(5, 9);
        for j in 0..bells {
            let f = j as f32 / (bells - 1) as f32;
            let s = 0.48 + 0.49 * f;
            let (at, _) = along(&path, s);
            let open = f < 0.78;
            let size = 1.0 - 0.45 * f.powf(1.4);
            let jitter = side * rng.range(-0.45, 0.45);
            let ped = (out * 0.7 - UP * 0.45 + jitter).normalize();
            let hang = at + ped * 0.008 * size;
            stem(b, &[at, hang], 0.0011, 0.0009, stem_c);
            let axis = if open {
                (-UP + out * 0.35 + jitter * 0.5).normalize()
            } else {
                (-UP * 0.6 + out * 0.8 + jitter * 0.3).normalize()
            };
            let rot = Quat::from_rotation_arc(UP, -axis);
            let (l, r) = (0.021 * size, 0.0055 * size);
            let color = if open { hue } else { stem_c.lerp(hue, 0.6) };
            if open {
                let outer = [
                    (0.05, 0.0),
                    (0.7, -0.12),
                    (0.92, -0.45),
                    (1.05, -0.75),
                    (1.5, -0.95),
                    (1.85, -0.9),
                ];
                let prof = outer.map(|(x, y)| (x * r, y * l));
                lathe(
                    b,
                    hang,
                    rot,
                    &prof,
                    6,
                    &|_, _| 1.0,
                    &|a, i| Cell::Bell.uv(a, i as f32 / 5.0),
                    &|i, _| color * if i == 0 { 0.55 } else { 1.0 },
                    true,
                    false,
                );
                let inner: Vec<(f32, f32)> =
                    prof[3..].iter().map(|(x, y)| (x * 0.88, *y)).collect();
                lathe(
                    b,
                    hang,
                    rot,
                    &inner,
                    6,
                    &|_, _| 1.0,
                    &|a, i| Cell::Bell.uv(a, (i + 3) as f32 / 5.0),
                    &|_, _| color * 0.42,
                    false,
                    false,
                );
            } else {
                let closed = [
                    (0.05, 0.0),
                    (0.6, -0.15),
                    (0.78, -0.5),
                    (0.6, -0.8),
                    (0.25, -0.97),
                    (0.0, -1.0),
                ];
                lathe(
                    b,
                    hang,
                    rot,
                    &closed.map(|(x, y)| (x * r, y * l * 0.7)),
                    6,
                    &|_, _| 1.0,
                    &|a, i| Cell::Bell.uv(a, i as f32 / 5.0 * 0.7),
                    &|_, _| color,
                    true,
                    false,
                );
            }
            // Far away each bell is one card showing three tepals and the scalloped mouth,
            // turned alternately across and along the arch so no scape goes edge-on.
            let across = if j % 2 == 0 { side } else { out };
            let across = (across - axis * across.dot(axis)).normalize_or(side);
            let (len, half, t1) = if open {
                (l * 0.95, r * 1.35, 1.0)
            } else {
                (l * 0.7, r * 0.8, 0.7)
            };
            let mouth = hang + axis * len;
            let facing = axis.cross(across).normalize_or(out);
            let facing = (facing * facing.y.signum() + UP * 0.4).normalize_or(UP);
            lod.card(
                [
                    hang - across * half * 0.55,
                    hang + across * half * 0.55,
                    mouth + across * half,
                    mouth - across * half,
                ],
                [
                    Cell::Bell.uv(0.25, 0.0),
                    Cell::Bell.uv(0.75, 0.0),
                    Cell::Bell.uv(0.75, t1),
                    Cell::Bell.uv(0.25, t1),
                ],
                facing,
                [
                    (color * 0.55).extend(1.0),
                    (color * 0.55).extend(1.0),
                    color.extend(1.0),
                    color.extend(1.0),
                ],
            );
        }
    }
}

// ---------------------------------------------------------------- mushrooms

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cap {
    /// Young and still closed around the stem.
    Button,
    /// Conical bell (Mycena-like).
    Bell,
    /// Convex dome.
    Dome,
    /// Old and flattened, the margin turned up around a low umbo.
    Flat,
    /// The lantern cap's tall translucent bell.
    Lantern,
}

impl Cap {
    /// Top surface from the apex to the margin, then the underside back to the stem, as
    /// (radius, height) in cap radii; returns the profile and how many rows are on top.
    fn profile(self) -> (&'static [(f32, f32)], usize) {
        match self {
            Cap::Button => (
                &[
                    (0.0, 1.1),
                    (0.35, 1.04),
                    (0.65, 0.84),
                    (0.88, 0.5),
                    (0.96, 0.18),
                    (0.9, -0.04),
                    (0.7, -0.08),
                    (0.3, -0.02),
                ],
                5,
            ),
            Cap::Bell => (
                &[
                    (0.0, 0.98),
                    (0.22, 0.94),
                    (0.45, 0.8),
                    (0.66, 0.6),
                    (0.84, 0.34),
                    (0.96, 0.1),
                    (1.0, -0.02),
                    (0.84, 0.1),
                    (0.55, 0.3),
                    (0.3, 0.42),
                ],
                7,
            ),
            Cap::Dome => (
                &[
                    (0.0, 0.62),
                    (0.28, 0.59),
                    (0.52, 0.5),
                    (0.72, 0.37),
                    (0.88, 0.21),
                    (0.98, 0.05),
                    (0.97, -0.04),
                    (0.78, 0.02),
                    (0.5, 0.08),
                    (0.3, 0.12),
                ],
                6,
            ),
            Cap::Flat => (
                &[
                    (0.0, 0.34),
                    (0.12, 0.33),
                    (0.3, 0.24),
                    (0.55, 0.18),
                    (0.8, 0.15),
                    (0.95, 0.17),
                    (1.0, 0.13),
                    (0.84, 0.06),
                    (0.55, 0.04),
                    (0.3, 0.07),
                ],
                7,
            ),
            Cap::Lantern => (
                &[
                    (0.0, 1.0),
                    (0.18, 0.985),
                    (0.36, 0.94),
                    (0.54, 0.85),
                    (0.7, 0.71),
                    (0.83, 0.53),
                    (0.92, 0.34),
                    (0.98, 0.16),
                    (1.03, 0.03),
                    (1.06, -0.05),
                    (0.96, -0.02),
                    (0.78, 0.13),
                    (0.56, 0.3),
                    (0.36, 0.46),
                    (0.2, 0.56),
                ],
                10,
            ),
        }
    }
}

/// One mushroom: stem from `base`, cap of `cap_r` on top.
struct Shroom {
    base: Vec3,
    height: f32,
    lean: Vec3,
    stem_r: f32,
    bulb: f32,
    cap: Cap,
    cap_r: f32,
    wave: f32,
    cap_sides: u32,
    stem_sides: u32,
    cap_c: Vec3,
    gill_c: Vec3,
    stem_c: Vec3,
}

/// Build a mushroom; returns the stem's path.
fn shroom(b: &mut MeshBuilder, m: &Shroom, rng: &mut Rng) -> Vec<Vec3> {
    let (profile, top_rows) = m.cap.profile();
    let attach_y = profile.last().unwrap().1;
    let cap_h = profile[0].1 * m.cap_r;
    // The stem leans, then straightens up under the cap.
    let stem_len = (m.height - cap_h + attach_y * m.cap_r).max(m.height * 0.2);
    let n = 8;
    let mut path = Vec::with_capacity(n + 1);
    let mut p = m.base - UP * 0.01;
    path.push(p);
    for i in 0..n {
        let s = (i as f32 + 0.5) / n as f32;
        p += (UP + m.lean * (1.0 - s).powf(1.5)).normalize() * stem_len / n as f32;
        path.push(p);
    }
    let radius: Vec<f32> = (0..=n)
        .map(|i| {
            let s = i as f32 / n as f32;
            m.stem_r * (1.0 + m.bulb * (1.0 - smoothstep(0.0, 0.22, s))) * (1.0 - 0.28 * s)
        })
        .collect();
    let stem_c = m.stem_c;
    let col = |i: usize, _: f32| {
        (stem_c * (0.5 + 0.5 * smoothstep(0.0, 0.5, i as f32 / n as f32))).extend(1.0)
    };
    tube(
        b,
        &path,
        &radius,
        m.stem_sides,
        &|a, t| stem_uv(a, 1.0 - t),
        &col,
    );

    let top = *path.last().unwrap();
    let t = (path[n] - path[n - 1]).normalize_or(UP);
    let tilt = Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)) * 0.12;
    let axis = (t * 0.6 + UP * 0.4 + tilt).normalize();
    let rot = Quat::from_rotation_arc(UP, axis);
    let origin = top - axis * (attach_y * m.cap_r) + axis * 0.002;
    let rows = profile.len();
    let scaled: Vec<(f32, f32)> = profile
        .iter()
        .enumerate()
        .map(|(i, &(r, y))| {
            // The underside meets the stem.
            let r = if i == rows - 1 {
                radius[n] * 1.15 / m.cap_r
            } else {
                r
            };
            (r * m.cap_r, y * m.cap_r)
        })
        .collect();
    let phase = rng.range(0.0, TAU);
    let lobes = rng.range_i32(5, 9) as f32;
    let wave = m.wave;
    let wobble = move |a: f32, i: usize| {
        let edge = smoothstep(top_rows as f32 * 0.5, top_rows as f32, i as f32);
        1.0 + wave * edge * (a * lobes * TAU + phase).sin()
    };
    let under = rows - top_rows;
    let uv = move |a: f32, i: usize| {
        if i < top_rows {
            cap_uv(a, i as f32 / (top_rows - 1) as f32)
        } else {
            gill_uv(a, (i - top_rows) as f32 / (under - 1).max(1) as f32)
        }
    };
    let (cap_c, gill_c) = (m.cap_c, m.gill_c);
    let color = move |i: usize, _: f32| {
        if i < top_rows {
            let r = i as f32 / (top_rows - 1) as f32;
            (cap_c * (0.72 + 0.28 * r)).lerp(gill_c, 0.3 * smoothstep(0.6, 1.0, r))
        } else {
            gill_c
        }
    };
    lathe(
        b,
        origin,
        rot,
        &scaled,
        m.cap_sides,
        &wobble,
        &uv,
        &color,
        true,
        false,
    );
    path
}

fn mushrooms(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let mut caps = MeshBuilder::new();
    let mut pins = MeshBuilder::new();
    if sp.max_height_m > 1.0 {
        lantern_caps(sp, &mut rng, &mut caps, &mut pins);
    } else {
        glowcaps(sp, &mut rng, &mut caps, &mut pins);
    }
    PlantMeshes {
        wood: Some(pins.finish()),
        foliage: Some(caps.finish()),
        ..Default::default()
    }
}

/// Gills: pale, a little toward the glow color.
fn gill_color(sp: &PlantSpecies) -> Vec3 {
    let glow = sp.glow.map_or(Vec3::ONE, |g| Vec3::from(g.color));
    Vec3::from(sp.bark_color).lerp(glow * 0.8, 0.25)
}

/// Small closed buttons around the base (in the wood mesh: they don't glow yet).
fn buttons(
    b: &mut MeshBuilder,
    sp: &PlantSpecies,
    rng: &mut Rng,
    around: Vec3,
    spread: f32,
    n: i32,
    size: f32,
) {
    let stem_c = Vec3::from(sp.bark_color);
    let cap_c = Vec3::from(sp.leaf_color).lerp(stem_c, 0.35);
    for _ in 0..n {
        let spot = rng.in_unit_disk() * spread;
        let at = around + Vec3::new(spot.x, 0.0, spot.y);
        let s = size * rng.range(0.5, 1.0);
        let rot = Quat::from_rotation_arc(
            UP,
            (UP + Vec3::new(rng.range(-0.3, 0.3), 0.0, rng.range(-0.3, 0.3))).normalize(),
        );
        let profile = [
            (0.35, -0.2),
            (0.45, 0.4),
            (0.75, 0.55),
            (0.95, 0.9),
            (0.8, 1.35),
            (0.45, 1.6),
            (0.0, 1.68),
        ];
        lathe(
            b,
            at,
            rot,
            &profile.map(|(r, y)| (r * s, y * s)),
            8,
            &|_, _| 1.0,
            &|a, i| {
                if i < 3 {
                    stem_uv(a, 1.0 - i as f32 / 3.0)
                } else {
                    cap_uv(a, (6 - i) as f32 / 3.0)
                }
            },
            &|i, _| if i < 3 { stem_c * 0.8 } else { cap_c },
            true,
            false,
        );
    }
}

fn glowcaps(sp: &PlantSpecies, rng: &mut Rng, b: &mut MeshBuilder, pins: &mut MeshBuilder) {
    let h = sp.max_height_m;
    let reach = sp.crown_radius_m;
    let cap_c = Vec3::from(sp.leaf_color);
    let accent = Vec3::from(sp.accent_color);
    let stem_c = Vec3::from(sp.bark_color);
    let gill_c = gill_color(sp);
    let style = rng.range_i32(0, 4);
    // (count, base spread as a fraction of the reach, lean, height range)
    let (count, spread, lean, tall) = match style {
        // A tight bouquet from one base, leaning out.
        0 => (rng.range_i32(7, 12), 0.12, (0.3, 0.7), (0.4, 1.0)),
        // A loose troop of small ones.
        1 => (rng.range_i32(4, 8), 0.8, (0.0, 0.25), (0.3, 0.75)),
        // A few tall slender ones.
        2 => (rng.range_i32(2, 5), 0.3, (0.05, 0.25), (0.7, 1.0)),
        // An arc, tallest at one end.
        _ => (rng.range_i32(5, 9), 0.6, (0.05, 0.3), (0.35, 0.95)),
    };
    let count = count + sp.shape.branches as i32 - 7;
    let arc_phase = rng.range(0.0, TAU);
    for k in 0..count.max(1) {
        let f = k as f32 / count.max(2) as f32;
        let (base, height) = if style == 3 {
            let a = arc_phase + f * 2.2;
            let p = Vec3::new(a.cos(), 0.0, a.sin()) * reach * spread;
            (
                p,
                h * (tall.1 - (tall.1 - tall.0) * f) * rng.range(0.85, 1.05),
            )
        } else {
            let d = rng.in_unit_disk() * reach * spread;
            let first = if k == 0 {
                tall.1
            } else {
                rng.range(tall.0, tall.1)
            };
            (Vec3::new(d.x, 0.0, d.y), h * first)
        };
        let out = base.with_y(0.0).normalize_or(
            Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)).normalize_or(Vec3::X),
        );
        let pick = rng.f32();
        let cap = if height < h * 0.45 && pick < 0.35 {
            Cap::Button
        } else if pick < 0.45 {
            Cap::Bell
        } else if pick < 0.85 {
            Cap::Dome
        } else {
            Cap::Flat
        };
        let stem_r = sp.trunk_radius_m * rng.range(0.38, 0.6) * (height / h).powf(0.5);
        let cap_r = stem_r
            * rng.range(3.4, 5.2)
            * match cap {
                Cap::Button => 0.65,
                Cap::Flat => 1.15,
                _ => 1.0,
            };
        let tint = rng.range(0.8, 1.2);
        let m = Shroom {
            base,
            height,
            lean: out * rng.range(lean.0, lean.1),
            stem_r,
            bulb: rng.range(0.15, 0.5),
            cap,
            cap_r,
            wave: if cap == Cap::Flat { 0.06 } else { 0.025 },
            cap_sides: if cap_r > 0.06 { 16 } else { 12 },
            stem_sides: 6,
            cap_c: cap_c.lerp(accent, rng.f32() * 0.6) * tint,
            gill_c,
            stem_c: stem_c * rng.range(0.85, 1.05),
        };
        shroom(b, &m, rng);
    }
    let n = rng.range_i32(2, 6);
    buttons(
        pins,
        sp,
        rng,
        Vec3::ZERO,
        reach * 0.4,
        n,
        sp.trunk_radius_m * 0.45,
    );
}

fn lantern_caps(sp: &PlantSpecies, rng: &mut Rng, b: &mut MeshBuilder, pins: &mut MeshBuilder) {
    let h = sp.max_height_m;
    let reach = sp.crown_radius_m;
    let cap_c = Vec3::from(sp.leaf_color);
    let accent = Vec3::from(sp.accent_color);
    let stem_c = Vec3::from(sp.bark_color);
    let gill_c = gill_color(sp);
    let count = (sp.shape.branches as i32 + rng.range_i32(-1, 2)).max(1);
    for k in 0..count {
        let first = k == 0;
        let young = !first && rng.chance(0.35);
        let height = if first {
            h * rng.range(0.82, 1.0)
        } else if young {
            h * rng.range(0.25, 0.4)
        } else {
            h * rng.range(0.45, 0.78)
        };
        let base = if first {
            Vec3::ZERO
        } else {
            let a = rng.range(0.0, TAU);
            Vec3::new(a.cos(), 0.0, a.sin()) * reach * rng.range(0.4, 0.75)
        };
        let out = base.normalize_or(Vec3::X);
        let stem_r = sp.trunk_radius_m * rng.range(0.85, 1.1) * (height / h).powf(0.5);
        let cap = if young { Cap::Button } else { Cap::Lantern };
        let cap_r = reach
            * 0.5
            * (height / h).powf(0.6)
            * rng.range(0.85, 1.1)
            * if young { 0.55 } else { 1.0 };
        let m = Shroom {
            base,
            height,
            lean: out * rng.range(0.05, 0.25)
                + Vec3::new(rng.range(-0.1, 0.1), 0.0, rng.range(-0.1, 0.1)),
            stem_r,
            bulb: rng.range(0.5, 0.8),
            cap,
            cap_r,
            wave: 0.045,
            cap_sides: 32,
            stem_sides: 12,
            cap_c: cap_c.lerp(accent, rng.f32() * 0.6) * rng.range(0.85, 1.15),
            gill_c,
            stem_c: stem_c * rng.range(0.9, 1.05),
        };
        let path = shroom(b, &m, rng);
        // A volva cupping the base and, on grown ones, a ring hanging from the stem.
        let volva = [
            (1.0, 0.0),
            (1.75, 0.04),
            (2.0, 0.12),
            (1.85, 0.22),
            (1.55, 0.26),
        ];
        let vr = stem_r;
        lathe(
            b,
            base,
            Quat::IDENTITY,
            &volva.map(|(r, y)| (r * vr, y * vr * 2.2)),
            12,
            &|a, i| {
                1.0 + if i >= 3 {
                    0.12 * (a * 5.0 * TAU).sin()
                } else {
                    0.0
                }
            },
            &|a, i| stem_uv(a, 1.0 - i as f32 * 0.03),
            &|i, _| stem_c * (0.45 + 0.1 * i as f32),
            true,
            true,
        );
        if !young {
            let (at, axis) = along(&path, 0.62);
            let rr = stem_r * 0.82;
            let ring = [(1.0, 0.0), (1.45, -0.25), (2.0, -0.9), (2.25, -1.7)];
            let waves = rng.range_i32(7, 11) as f32;
            lathe(
                b,
                at,
                Quat::from_rotation_arc(UP, axis),
                &ring.map(|(r, y)| (r * rr, y * rr)),
                20,
                &|a, i| {
                    1.0 + if i == 3 {
                        0.1 * (a * waves * TAU).sin()
                    } else {
                        0.0
                    }
                },
                &|a, i| gill_uv(a, 0.9 - i as f32 * 0.25),
                &|_, _| gill_c * 0.9,
                true,
                true,
            );
        }
    }
    let n = rng.range_i32(2, 5);
    buttons(
        pins,
        sp,
        rng,
        Vec3::ZERO,
        reach * 0.6,
        n,
        sp.trunk_radius_m * 0.5,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::PlantLibrary;

    #[test]
    fn small_plants_build() {
        let lib = PlantLibrary::default();
        for name in [
            "fern",
            "grass",
            "wildflowers",
            "bluebell",
            "glowcap",
            "lantern cap",
        ] {
            let sp = lib.get(lib.id(name).unwrap());
            for seed in 0..4 {
                let m = small(sp, seed * 0x9e37_79b9 + 4);
                if sp.form == PlantForm::Mushroom {
                    assert!(m.wood.is_some(), "{name} buttons");
                }
                for f in [m.foliage.as_ref().expect(name)]
                    .into_iter()
                    .chain(m.foliage_lod.as_ref())
                {
                    assert!(f.triangle_count() > 20, "{name}: {}", f.triangle_count());
                    assert!(f.aabb().max.y <= sp.max_height_m * 1.3, "{name}");
                    assert!(f.aabb().min.y >= -0.05, "{name} below ground");
                    for v in &f.vertices {
                        assert!(v.position.iter().all(|c| c.is_finite()), "{name}");
                        assert!(v.normal.iter().all(|c| c.is_finite()), "{name}");
                        assert!(
                            (Vec3::from(v.normal).length() - 1.0).abs() < 1e-3,
                            "{name} normal"
                        );
                        assert!(v.uv.iter().all(|c| (0.0..=1.0).contains(c)), "{name} uv");
                    }
                }
            }
        }
    }

    /// Small plants are instanced by the thousand: keep every variant within a budget.
    #[test]
    fn triangle_budgets() {
        let lib = PlantLibrary::default();
        for (name, budget) in [
            ("grass", 2_200),
            ("fern", 6_000),
            ("wildflowers", 4_500),
            ("bluebell", 5_000),
            ("glowcap", 6_000),
            ("lantern cap", 8_000),
        ] {
            let sp = lib.get(lib.id(name).unwrap());
            for seed in 0..8u64 {
                let m = small(sp, seed * 0x9e37_79b9 + 1);
                let tris = m.foliage.as_ref().map_or(0, |f| f.triangle_count())
                    + m.wood.as_ref().map_or(0, |w| w.triangle_count());
                assert!(tris <= budget, "{name} variant {seed}: {tris} triangles");
            }
        }
    }

    /// Far meshes (shown beyond `small_plant_lod_distance_m`) cost under a fifth of the near
    /// mesh and cover the same ground; mushrooms keep one mesh.
    #[test]
    fn far_meshes_are_cheap_and_cover_the_plant() {
        let lib = PlantLibrary::default();
        for name in ["grass", "fern", "wildflowers", "bluebell"] {
            let sp = lib.get(lib.id(name).unwrap());
            for seed in 0..8u64 {
                let m = small(sp, seed * 0x9e37_79b9 + 11);
                let near = m.foliage.as_ref().expect(name);
                let far = m.foliage_lod.as_ref().expect(name);
                let (n, f) = (near.triangle_count(), far.triangle_count());
                assert!(f > 20, "{name} variant {seed}: far mesh has {f} triangles");
                assert!(
                    f * 5 < n,
                    "{name} variant {seed}: far {f} vs near {n} triangles"
                );
                let (a, b) = (near.aabb(), far.aabb());
                for axis in [0, 2] {
                    let (wn, wf) = (a.max[axis] - a.min[axis], b.max[axis] - b.min[axis]);
                    assert!(
                        wf > wn * 0.6,
                        "{name} variant {seed}: far width {wf} vs {wn}"
                    );
                }
            }
        }
        for name in ["glowcap", "lantern cap"] {
            let sp = lib.get(lib.id(name).unwrap());
            assert!(small(sp, 3).foliage_lod.is_none(), "{name}");
        }
    }

    #[test]
    fn grass_clumps_are_dense() {
        let lib = PlantLibrary::default();
        let sp = lib.get(lib.id("grass").unwrap());
        let m = small(sp, 7).foliage.unwrap();
        assert!(m.triangle_count() > 800, "{}", m.triangle_count());
        // Blades spread over most of the clump.
        let bb = m.aabb();
        assert!(bb.max.x - bb.min.x > sp.crown_radius_m, "{bb:?}");
    }

    #[test]
    fn mushroom_variants_differ() {
        let lib = PlantLibrary::default();
        let sp = lib.get(lib.id("glowcap").unwrap());
        let counts: std::collections::HashSet<usize> = (0..4u64)
            .map(|v| {
                small(sp, 99 ^ (v * 0x9e37_79b9))
                    .foliage
                    .unwrap()
                    .vertices
                    .len()
            })
            .collect();
        assert!(counts.len() >= 3, "{counts:?}");
    }
}
