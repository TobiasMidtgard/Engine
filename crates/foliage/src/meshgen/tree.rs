//! Trees and shrubs: a branching skeleton (trunk, limbs, twigs) with leaf cards at the twigs.
//!
//! A tree is built in three parts so it can be felled: the **base** (roots and the trunk up to
//! the cut, which stays behind as the stump, with growth rings on top), the **wood** above the
//! cut (trunk and branches, which falls) and the **foliage** (leaf cards, which fall with the
//! wood). Standing, the three meet seamlessly. Each part also has a cheaper level of detail.

use engine_core::math::{Rng, Vec2, Vec3, Vec4, smoothstep};

use super::{MeshBuilder, rgb};
use crate::species::{BarkStyle, PlantForm, PlantSpecies};

/// Meshes for one plant variant. Parts a form doesn't have are `None`.
#[derive(Default)]
pub struct PlantMeshes {
    pub base: Option<engine_render::Mesh>,
    pub wood: Option<engine_render::Mesh>,
    pub foliage: Option<engine_render::Mesh>,
    pub wood_lod: Option<engine_render::Mesh>,
    pub foliage_lod: Option<engine_render::Mesh>,
    /// A felled log once its twigs have rotted off: the trunk above the cut with stubs of the
    /// big limbs (same model space as `wood`).
    pub log: Option<engine_render::Mesh>,
    /// Height of the mesh at full size, meters.
    pub height: f32,
    /// Height of the felling cut (top of the stump), meters.
    pub cut_height: f32,
}

/// A branch of the skeleton.
struct Branch {
    path: Vec<Vec3>,
    radius: Vec<f32>,
    level: u8,
}

/// A leaf card: attached at `base`, growing along `dir`.
struct Card {
    base: Vec3,
    dir: Vec3,
    /// Preferred facing (the card turns toward it around `dir`).
    face: Vec3,
    width: f32,
    length: f32,
    tint: Vec4,
}

/// The crown as an ellipsoid (for leaf shading and branch lengths).
#[derive(Clone, Copy)]
struct Crown {
    center: Vec3,
    radii: Vec3,
}

impl Crown {
    fn normal(&self, p: Vec3) -> Vec3 {
        ((p - self.center) / (self.radii * self.radii)).normalize_or(Vec3::Y)
    }

    /// 0 at the center .. 1 on the surface.
    fn depth(&self, p: Vec3) -> f32 {
        ((p - self.center) / self.radii).length()
    }

    /// Distance from `p` along `d` to the surface (p inside).
    fn reach(&self, p: Vec3, d: Vec3) -> Option<f32> {
        let q = (p - self.center) / self.radii;
        let e = d / self.radii;
        let a = e.dot(e);
        let b = q.dot(e);
        let c = q.dot(q) - 1.0;
        let disc = b * b - a * c;
        if disc < 0.0 || c > 0.0 {
            return None;
        }
        Some((-b + disc.sqrt()) / a)
    }
}

/// Build the meshes of one variant of a woody species. `sapling` builds the slender young form
/// used while a tree is small.
pub fn woody(sp: &PlantSpecies, seed: u64, sapling: bool) -> PlantMeshes {
    let mut m = match sp.form {
        PlantForm::Conifer => conifer(sp, seed, sapling),
        PlantForm::Shrub => shrub(sp, seed),
        _ => broadleaf(sp, seed, sapling),
    };
    m.share_bounds();
    m
}

impl PlantMeshes {
    /// Give the swaying parts (wood, foliage and their LODs) the same vertical bounds, so they
    /// sway in the wind exactly together: the renderer's sway uses the height within the mesh
    /// bounds. Also records the plant's height.
    pub fn share_bounds(&mut self) {
        let parts = [&self.wood, &self.foliage, &self.wood_lod, &self.foliage_lod];
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for m in parts.into_iter().flatten() {
            let b = m.aabb();
            lo = lo.min(b.min.y);
            hi = hi.max(b.max.y);
        }
        for m in [
            &mut self.wood,
            &mut self.foliage,
            &mut self.wood_lod,
            &mut self.foliage_lod,
        ]
        .into_iter()
        .flatten()
        {
            for y in [lo, hi] {
                m.vertices.push(engine_render::Vertex::new(
                    Vec3::new(0.0, y, 0.0),
                    Vec3::Y,
                    Vec2::ZERO,
                ));
            }
        }
        self.height = hi;
    }
}

/// Cut height for a species (stump top), meters.
pub fn cut_height(sp: &PlantSpecies) -> f32 {
    (0.35 + sp.trunk_radius_m * 0.5).clamp(0.3, 0.75)
}

fn point_at(path: &[Vec3], radius: &[f32], s: f32) -> (Vec3, Vec3, f32) {
    let n = path.len() - 1;
    let f = (s.clamp(0.0, 1.0) * n as f32).min(n as f32 - 1e-4);
    let i = f as usize;
    let t = f - i as f32;
    let p = path[i].lerp(path[i + 1], t);
    let d = (path[i + 1] - path[i]).normalize_or(Vec3::Y);
    (p, d, radius[i] + (radius[i + 1] - radius[i]) * t)
}

fn jitter(rng: &mut Rng, amount: f32) -> Vec3 {
    Vec3::new(
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
    ) * amount
}

/// Rotate `v` around unit `axis` by `angle`.
fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    engine_core::math::Quat::from_axis_angle(axis, angle) * v
}

/// The trunk from the ground to `top`, with a point exactly at the cut.
fn trunk(
    sp: &PlantSpecies,
    rng: &mut Rng,
    top: f32,
    cut: f32,
    flare: bool,
) -> (Vec<Vec3>, Vec<f32>) {
    let r0 = sp.trunk_radius_m;
    let wander = sp.shape.gnarl * 0.03 * top;
    let (p1, p2) = (rng.range(0.0, 6.3), rng.range(0.0, 6.3));
    let (f1, f2) = (rng.range(0.8, 2.0), rng.range(0.7, 1.7));
    let mut ys = vec![0.0, cut * 0.5, cut];
    let n = ((top - cut) / 0.55).ceil().clamp(4.0, 40.0) as usize;
    for i in 1..=n {
        ys.push(cut + (top - cut) * i as f32 / n as f32);
    }
    let mut path = Vec::with_capacity(ys.len());
    let mut radius = Vec::with_capacity(ys.len());
    for y in ys {
        let t = y / top;
        let off = |f: f32, ph: f32| ((f * t * std::f32::consts::PI + ph).sin() - ph.sin()) * wander;
        path.push(
            Vec3::new(off(f1, p1), y, off(f2, p2)) * Vec3::new(t.powf(0.6), 1.0, t.powf(0.6)),
        );
        let mut r = r0 * (0.1 + 0.9 * (1.0 - t).powf(1.15));
        if flare {
            r += r0 * 0.5 * (-y / 0.45).exp();
        }
        radius.push(r.max(0.01));
    }
    (path, radius)
}

/// Vertex color for bark at height `y`.
fn bark_color(sp: &PlantSpecies, y: f32, h: f32, ring: usize, seed: u64) -> Vec4 {
    let n = engine_core::math::hash01(ring as i64, 0, 0, seed);
    let base = Vec3::from(sp.bark_color);
    let c = match sp.shape.bark {
        BarkStyle::Birch => {
            let dark = Vec3::new(0.035, 0.03, 0.028);
            let foot = (1.0 - (y / 1.4).min(1.0)).powi(2);
            let band = if n < 0.2 { 0.3 } else { 0.9 + 0.12 * n };
            base.lerp(dark, foot) * band.max(foot)
        }
        BarkStyle::Pine => {
            let foot = Vec3::new(0.12, 0.09, 0.07);
            foot.lerp(base, smoothstep(0.2 * h, 0.5 * h, y)) * (0.85 + 0.25 * n)
        }
        BarkStyle::Plain => {
            let moss = Vec3::new(0.06, 0.09, 0.025);
            base.lerp(moss, (1.0 - (y / 0.9).min(1.0)).powi(2) * 0.6) * (0.8 + 0.3 * n)
        }
    };
    let ao = 0.55 + 0.45 * smoothstep(0.0, 0.7, y);
    (c * ao).extend(1.0)
}

/// Cut wood with growth rings (r = 0 center .. 1 bark).
fn cut_color(sp: &PlantSpecies, r: f32) -> Vec4 {
    if r > 0.92 {
        rgb(sp.bark_color, 0.6)
    } else {
        let wood = Vec3::new(0.62, 0.45, 0.26);
        let ring = 0.86 + 0.14 * (r * 23.0).cos();
        (wood * ring * (0.9 + 0.1 * r)).extend(1.0)
    }
}

fn add_branches(
    b: &mut MeshBuilder,
    sp: &PlantSpecies,
    branches: &[Branch],
    h: f32,
    lod: bool,
    seed: u64,
) {
    let many = branches.iter().filter(|b| b.level == 1).count() > 30;
    for (bi, br) in branches.iter().enumerate() {
        // Far away, twigs vanish and (on trees with many limbs) every other limb too.
        if lod && (br.level >= 2 || (many && br.level == 1 && bi % 2 == 0)) {
            continue;
        }
        let sides = match (br.level, lod) {
            (0, false) => 10,
            (0, true) => 6,
            (1, false) => 6,
            (1, true) => 3,
            _ => 4,
        };
        let path = &br.path;
        let cap_tip = |_: f32| rgb(sp.bark_color, 0.8);
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i + bi * 7, seed);
        b.tube(
            path,
            &br.radius,
            sides,
            0.5,
            &color,
            None,
            if br.level == 0 { Some(&cap_tip) } else { None },
        );
    }
}

fn add_cards(
    b: &mut MeshBuilder,
    cards: &[Card],
    crown: &Crown,
    lod: bool,
    rng_seed: u64,
    sp: &PlantSpecies,
) {
    let mut rng = Rng::new(rng_seed);
    for (i, c) in cards.iter().enumerate() {
        let (scale, keep) = if lod { (1.75, i % 3 == 0) } else { (1.0, true) };
        if !keep {
            continue;
        }
        let g = c.dir.normalize_or(Vec3::Y);
        // Face the preferred direction as far as the growth direction allows, then twist a bit.
        let mut n = (c.face - g * c.face.dot(g)).normalize_or(g.any_orthonormal_vector());
        n = rotate(n, g, rng.range(-0.7, 0.7));
        let u = g.cross(n);
        let (w, l) = (c.width * scale, c.length * scale);
        let base = c.base - g * l * 0.08;
        let corners = [
            base - u * w * 0.5,
            base + u * w * 0.5,
            base + u * w * 0.5 + g * l,
            base - u * w * 0.5 + g * l,
        ];
        let mid = base + g * l * 0.5;
        let normal = (crown.normal(mid) * 0.8 + Vec3::Y * 0.25).normalize();
        let depth = crown.depth(mid);
        let ao = 0.35 + 0.65 * smoothstep(0.2, 1.05, depth);
        let height = smoothstep(
            crown.center.y - crown.radii.y,
            crown.center.y + crown.radii.y,
            mid.y,
        );
        let shade = ao * (0.78 + 0.3 * height);
        let inner = c.tint * shade * 0.82;
        let outer = c.tint * shade * 1.05;
        let colors = [inner, inner, outer, outer].map(|v| v.with_w(1.0));
        b.card(
            corners,
            [
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 0.0),
            ],
            normal,
            colors,
        );
    }
    let _ = sp;
}

/// Leaf tint: brightness and a shift toward the accent color.
fn leaf_tint(sp: &PlantSpecies, rng: &mut Rng) -> Vec4 {
    let leaf = Vec3::from(sp.leaf_color).max(Vec3::splat(0.01));
    let accent = Vec3::from(sp.accent_color);
    let t = rng.range(-0.3, 0.6).max(0.0);
    let ratio = Vec3::ONE.lerp(accent / leaf, t);
    (ratio * rng.range(0.85, 1.12)).extend(1.0)
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    sp: &PlantSpecies,
    seed: u64,
    trunk_path: &[Vec3],
    trunk_r: &[f32],
    branches: &[Branch],
    cards: &[Card],
    crown: Crown,
    h: f32,
    cut: f32,
    sapling: bool,
) -> PlantMeshes {
    let top = h * 1.02;
    let cut_i = trunk_path
        .iter()
        .position(|p| (p.y - cut).abs() < 1e-4)
        .unwrap_or(2);
    let mut out = PlantMeshes {
        height: top,
        cut_height: cut,
        ..Default::default()
    };
    // Base: roots + trunk up to the cut, growth rings on top.
    if !sapling {
        let mut b = MeshBuilder::new();
        let path = &trunk_path[..=cut_i];
        let rad = &trunk_r[..=cut_i];
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i, seed);
        let cap = |r: f32| cut_color(sp, r);
        b.tube(path, rad, 12, 0.5, &color, None, Some(&cap));
        if sp.trunk_radius_m > 0.12 {
            let mut rng = Rng::new(seed ^ 0x2007);
            let n = rng.range_i32(4, 7);
            let r0 = sp.trunk_radius_m;
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU + rng.range(-0.3, 0.3);
                let d = Vec3::new(a.cos(), 0.0, a.sin());
                let len = rng.range(1.6, 2.8);
                let root = [
                    d * r0 * 0.3 + Vec3::Y * cut * 0.75,
                    d * r0 * 1.2 + Vec3::Y * 0.12,
                    d * r0 * len + Vec3::Y * -0.06,
                ];
                let radius = [r0 * 0.42, r0 * 0.22, r0 * 0.05];
                let color = |i: usize, _a: f32| bark_color(sp, root[i].y, h, i + 40, seed) * 0.85;
                b.tube(&root, &radius, 6, 0.5, &color, None, None);
            }
        }
        out.base = Some(b.finish());

        let mut b = MeshBuilder::new();
        let path = &trunk_path[cut_i..];
        let rad = &trunk_r[cut_i..];
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i + cut_i, seed);
        let cap = |r: f32| cut_color(sp, r);
        let broken = |_: f32| rgb(sp.bark_color, 0.55);
        b.tube(path, rad, 10, 0.5, &color, Some(&cap), Some(&broken));
        // Short broken stubs where the thickest limbs were (the thin ones snap off flush).
        let mut limbs: Vec<_> = branches
            .iter()
            .enumerate()
            .filter(|(_, br)| br.level == 1)
            .collect();
        limbs.sort_by(|a, b| b.1.radius[0].total_cmp(&a.1.radius[0]));
        limbs.truncate((limbs.len() / 3).clamp(2, 9));
        for (bi, br) in limbs {
            let len = 0.05 + br.radius[0] * 1.6;
            let d = (br.path[1] - br.path[0]).normalize_or(Vec3::Y);
            let stub = [br.path[0], br.path[0] + d * len];
            let r = [br.radius[0] * 0.9, br.radius[0] * 0.55];
            let color = |i: usize, _a: f32| bark_color(sp, stub[i].y, h, i + bi * 7, seed);
            b.tube(&stub, &r, 5, 0.5, &color, None, Some(&broken));
        }
        out.log = Some(b.finish());
    }
    for lod in [false, true] {
        // Wood above the cut (with a cut face on its bottom, seen once it falls).
        let mut b = MeshBuilder::new();
        let start = if sapling { 0 } else { cut_i };
        let path = &trunk_path[start..];
        let rad = &trunk_r[start..];
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i + start, seed);
        let cap = |r: f32| cut_color(sp, r);
        let tip = |_: f32| rgb(sp.bark_color, 0.8);
        b.tube(
            path,
            rad,
            if lod { 6 } else { 12 },
            0.5,
            &color,
            if sapling { None } else { Some(&cap) },
            Some(&tip),
        );
        add_branches(&mut b, sp, branches, h, lod, seed);
        let mut f = MeshBuilder::new();
        add_cards(&mut f, cards, &crown, lod, seed ^ 0xca2d, sp);
        if lod {
            out.wood_lod = Some(b.finish());
            out.foliage_lod = Some(f.finish());
        } else {
            out.wood = Some(b.finish());
            out.foliage = Some(f.finish());
        }
    }
    out
}

/// Oak-, maple- or birch-like tree.
fn broadleaf(sp: &PlantSpecies, seed: u64, sapling: bool) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let cut = cut_height(sp);
    let top = h * if sapling { 0.96 } else { 0.84 };
    let (path, radius) = trunk(sp, &mut rng, top, cut, !sapling);
    let crown_base_y = h * if sapling { 0.35 } else { sh.crown_base };
    let crown = Crown {
        center: Vec3::new(
            path.last().unwrap().x * 0.6,
            (crown_base_y + h) * 0.5,
            path.last().unwrap().z * 0.6,
        ),
        radii: Vec3::new(
            sp.crown_radius_m,
            (h - crown_base_y) * 0.5 + 0.3,
            sp.crown_radius_m,
        ),
    };
    let mut branches = vec![Branch {
        path: path.clone(),
        radius: radius.clone(),
        level: 0,
    }];
    let mut cards = Vec::new();
    let card = |rng: &mut Rng, base: Vec3, dir: Vec3, size: f32| Card {
        base,
        dir,
        face: (crown.normal(base) + Vec3::Y * 0.6).normalize(),
        width: size,
        length: size,
        tint: leaf_tint(sp, rng),
    };
    let leaf = sh.leaf_size_m * if sapling { 0.7 } else { 1.0 };
    let nb = if sapling { 5 } else { sh.branches.max(3) };
    for k in 0..nb {
        let frac = sh.crown_base
            + (0.97 - sh.crown_base) * ((k as f32 + rng.range(0.15, 0.85)) / nb as f32);
        let frac = if sapling {
            0.4 + 0.55 * k as f32 / nb as f32
        } else {
            frac
        };
        let s_trunk = (frac * h / top).min(0.98);
        let (attach, _, r_t) = point_at(&path, &radius, s_trunk);
        let rel = ((frac - sh.crown_base) / (1.0 - sh.crown_base)).clamp(0.0, 1.0);
        let angle = (sh.angle_low_deg
            + (sh.angle_high_deg - sh.angle_low_deg) * rel
            + rng.range(-8.0, 8.0))
        .to_radians();
        let az = k as f32 * 2.399_963 + rng.range(-0.35, 0.35);
        let dir = Vec3::new(angle.sin() * az.cos(), angle.cos(), angle.sin() * az.sin());
        let length = crown
            .reach(attach, dir)
            .unwrap_or(sp.crown_radius_m * 0.5)
            .clamp(sp.crown_radius_m * 0.3, sp.crown_radius_m * 1.3)
            * rng.range(0.82, 1.02);
        let segs = 6;
        let mut bp = vec![attach - dir * r_t * 0.6];
        let mut d = dir;
        for i in 1..=segs {
            let f = i as f32 / segs as f32;
            d = (d + Vec3::Y * sh.tip_bend * 0.16 * f + jitter(&mut rng, 0.1 * (0.3 + sh.gnarl)))
                .normalize();
            bp.push(*bp.last().unwrap() + d * length / segs as f32);
        }
        let r_start =
            (r_t * 0.62).min(sp.trunk_radius_m * 0.5 * (length / sp.crown_radius_m).sqrt());
        let br: Vec<f32> = (0..=segs)
            .map(|i| {
                let f = i as f32 / segs as f32;
                (r_start * (1.0 - f) + 0.012 * f).max(0.008)
            })
            .collect();
        // Twigs and leaves along the branch.
        if !sapling {
            let n2 = ((length / 1.1).round() as i32).clamp(2, 6);
            for j in 0..n2 {
                let s = 0.28 + 0.66 * (j as f32 + rng.range(0.0, 0.6)) / n2 as f32;
                let (p, dp, rp) = point_at(&bp, &br, s);
                let side = rotate(
                    dp.any_orthonormal_vector(),
                    dp,
                    j as f32 * 2.3 + rng.range(-0.5, 0.5),
                );
                let a2 = rng.range(0.6, 1.0);
                let mut d2 =
                    (dp * a2.cos() + side * a2.sin() + Vec3::Y * sh.tip_bend * 0.3).normalize();
                let l2 = length * rng.range(0.35, 0.55) * (1.0 - 0.4 * s);
                let mut tp = vec![p];
                for _ in 0..3 {
                    d2 = (d2 + Vec3::Y * sh.tip_bend * 0.12 + jitter(&mut rng, 0.08)).normalize();
                    tp.push(*tp.last().unwrap() + d2 * l2 / 3.0);
                }
                let tr = vec![rp * 0.6, rp * 0.4, rp * 0.25, 0.006];
                let per = (3.0 * sh.leaf_density).round().max(1.0) as usize;
                for c in 0..per {
                    let s2 = 0.3 + 0.7 * (c as f32 + 1.0) / per as f32;
                    let (cp, cd, _) = point_at(&tp, &tr, s2);
                    let out = (cp - crown.center).normalize_or(Vec3::Y);
                    let size = leaf * rng.range(0.8, 1.15);
                    cards.push(card(&mut rng, cp, (cd + out * 0.6).normalize(), size));
                }
                branches.push(Branch {
                    path: tp,
                    radius: tr,
                    level: 2,
                });
            }
        }
        for s in [0.6, 0.82, 1.0] {
            let (cp, cd, _) = point_at(&bp, &br, s);
            let out = (cp - crown.center).normalize_or(Vec3::Y);
            let size = leaf * rng.range(0.85, 1.2);
            cards.push(card(&mut rng, cp, (cd + out * 0.4).normalize(), size));
        }
        branches.push(Branch {
            path: bp,
            radius: br,
            level: 1,
        });
    }
    // Crown top.
    let apex = *path.last().unwrap();
    for k in 0..if sapling { 2 } else { 4 } {
        let a = k as f32 * 1.7 + rng.range(0.0, 1.0);
        let d = Vec3::new(a.cos() * 0.6, 1.0, a.sin() * 0.6).normalize();
        let size = leaf * rng.range(0.9, 1.2);
        cards.push(card(&mut rng, apex - Vec3::Y * leaf * 0.3, d, size));
    }
    assemble(
        sp, seed, &path, &radius, &branches, &cards, crown, h, cut, sapling,
    )
}

/// Pine- or spruce-like tree: straight trunk, whorls of branches, needle sprays.
fn conifer(sp: &PlantSpecies, seed: u64, sapling: bool) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let cut = cut_height(sp);
    let (path, radius) = trunk(sp, &mut rng, h * 0.99, cut, !sapling);
    let crown_base = if sapling { 0.15 } else { sh.crown_base };
    let y0 = h * crown_base;
    let spacing = (sh.leaf_size_m * 0.55).max(0.3) * if sapling { 2.0 } else { 1.0 };
    let whorls = (((h - y0) / spacing).floor() as usize).max(2);
    let rounded = crown_base > 0.3;
    let crown = Crown {
        center: Vec3::new(0.0, (y0 + h) * 0.5, 0.0),
        radii: Vec3::new(sp.crown_radius_m, (h - y0) * 0.5 + 0.4, sp.crown_radius_m),
    };
    let mut branches = vec![Branch {
        path: path.clone(),
        radius: radius.clone(),
        level: 0,
    }];
    let mut cards = Vec::new();
    for w in 0..whorls {
        let y = y0 + (h - y0 - 0.4) * (w as f32 + rng.range(0.0, 0.4)) / whorls as f32;
        let rel = ((y - y0) / (h - y0)).clamp(0.0, 1.0);
        let envelope = if rounded {
            (std::f32::consts::PI * (0.2 + 0.8 * rel))
                .sin()
                .max(0.15)
                .powf(0.6)
                * (1.0 - rel * 0.35)
        } else {
            (1.0 - rel).powf(0.9) * 0.95 + 0.05
        };
        let (attach, _, r_t) = point_at(&path, &radius, y / (h * 0.99));
        let nb = (sh.branches as i32 + rng.range_i32(-1, 2)).max(3);
        let az0 = rng.range(0.0, std::f32::consts::TAU);
        for k in 0..nb {
            let az = az0 + k as f32 * std::f32::consts::TAU / nb as f32 + rng.range(-0.25, 0.25);
            let angle = (sh.angle_low_deg
                + (sh.angle_high_deg - sh.angle_low_deg) * rel
                + rng.range(-6.0, 6.0))
            .to_radians();
            let dir = Vec3::new(angle.sin() * az.cos(), angle.cos(), angle.sin() * az.sin());
            let length = (sp.crown_radius_m * envelope * rng.range(0.82, 1.08)).max(0.25);
            let segs = 4;
            let mut bp = vec![attach - dir * r_t * 0.5];
            let mut d = dir;
            for i in 1..=segs {
                let f = i as f32 / segs as f32;
                d = (d
                    + Vec3::Y * (sh.tip_bend * 0.3 * f - 0.08)
                    + jitter(&mut rng, 0.05 + 0.1 * sh.gnarl))
                .normalize();
                bp.push(*bp.last().unwrap() + d * length / segs as f32);
            }
            let r_start = (r_t * 0.45).min(0.07 + sp.trunk_radius_m * 0.12);
            let br: Vec<f32> = (0..=segs)
                .map(|i| (r_start * (1.0 - i as f32 / segs as f32)).max(0.008))
                .collect();
            let n_cards = ((length / (sh.leaf_size_m * 0.55)).round() as usize).clamp(1, 7);
            for c in 0..n_cards {
                let s = 0.18 + 0.82 * (c as f32 + 0.5) / n_cards as f32;
                let (cp, cd, _) = point_at(&bp, &br, s);
                let size = sh.leaf_size_m * rng.range(0.8, 1.1) * (0.75 + 0.35 * (1.0 - s));
                let tint = leaf_tint(sp, &mut rng);
                for (roll, keep) in [(0.0f32, true), (1.1, !sapling)] {
                    if !keep {
                        continue;
                    }
                    let face = rotate(
                        (Vec3::Y - cd * cd.y).normalize_or(Vec3::Y),
                        cd,
                        roll + rng.range(-0.3, 0.3),
                    );
                    cards.push(Card {
                        base: cp - cd * size * 0.35,
                        dir: cd,
                        face,
                        width: size * 0.85,
                        length: size,
                        tint,
                    });
                }
            }
            branches.push(Branch {
                path: bp,
                radius: br,
                level: 1,
            });
        }
    }
    // Leader at the top.
    for k in 0..3 {
        let a = k as f32 * 2.1 + rng.range(0.0, 0.5);
        let face = Vec3::new(a.cos(), 0.0, a.sin());
        cards.push(Card {
            base: Vec3::new(0.0, h - sh.leaf_size_m * 0.9, 0.0),
            dir: Vec3::Y,
            face,
            width: sh.leaf_size_m * 0.6,
            length: sh.leaf_size_m * 1.1,
            tint: leaf_tint(sp, &mut rng),
        });
    }
    assemble(
        sp, seed, &path, &radius, &branches, &cards, crown, h, cut, sapling,
    )
}

/// Multi-stemmed bush.
fn shrub(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let crown = Crown {
        center: Vec3::new(0.0, h * 0.55, 0.0),
        radii: Vec3::new(sp.crown_radius_m, h * 0.5, sp.crown_radius_m),
    };
    let mut branches = Vec::new();
    let mut cards = Vec::new();
    let n = sh.branches.max(3) as i32 + rng.range_i32(-1, 2);
    for k in 0..n {
        let az = k as f32 * 2.399_963 + rng.range(-0.3, 0.3);
        let angle = rng.range(sh.angle_high_deg, sh.angle_low_deg).to_radians();
        let dir = Vec3::new(angle.sin() * az.cos(), angle.cos(), angle.sin() * az.sin());
        let length = h * rng.range(0.75, 1.0) / angle.cos().max(0.5);
        let segs = 5;
        let start = Vec3::new(az.cos(), 0.0, az.sin()) * rng.range(0.03, 0.15) - Vec3::Y * 0.05;
        let mut bp = vec![start];
        let mut d = dir;
        for _ in 0..segs {
            d = (d + Vec3::Y * sh.tip_bend * 0.1 + jitter(&mut rng, 0.12)).normalize();
            bp.push(*bp.last().unwrap() + d * length / segs as f32);
        }
        let br: Vec<f32> = (0..=segs)
            .map(|i| (sp.trunk_radius_m * (1.0 - 0.8 * i as f32 / segs as f32)).max(0.006))
            .collect();
        for j in 0..3 {
            let s = 0.4 + 0.2 * j as f32 + rng.range(0.0, 0.1);
            let (p, dp, rp) = point_at(&bp, &br, s);
            let side = rotate(
                dp.any_orthonormal_vector(),
                dp,
                j as f32 * 2.2 + rng.range(0.0, 1.0),
            );
            let d2 = (dp + side * 0.9).normalize();
            let l2 = length * rng.range(0.25, 0.4);
            let tp = vec![
                p,
                p + d2 * l2 * 0.5,
                p + (d2 + Vec3::Y * 0.2).normalize() * l2,
            ];
            let tr = vec![rp * 0.6, rp * 0.35, 0.005];
            let (cp, cd, _) = point_at(&tp, &tr, 0.9);
            let out = (cp - crown.center).normalize_or(Vec3::Y);
            let size = sh.leaf_size_m * rng.range(0.8, 1.15);
            cards.push(Card {
                base: cp,
                dir: (cd + out * 0.5).normalize(),
                face: (crown.normal(cp) + Vec3::Y * 0.5).normalize(),
                width: size,
                length: size,
                tint: leaf_tint(sp, &mut rng),
            });
            branches.push(Branch {
                path: tp,
                radius: tr,
                level: 2,
            });
        }
        let per = (4.0 * sh.leaf_density).round() as usize;
        for c in 0..per {
            let s = 0.45 + 0.55 * (c as f32 + 1.0) / per as f32;
            let (cp, cd, _) = point_at(&bp, &br, s);
            let out = (cp - crown.center).normalize_or(Vec3::Y);
            let size = sh.leaf_size_m * rng.range(0.85, 1.2);
            cards.push(Card {
                base: cp,
                dir: (cd + out * 0.5).normalize(),
                face: (crown.normal(cp) + Vec3::Y * 0.5).normalize(),
                width: size,
                length: size,
                tint: leaf_tint(sp, &mut rng),
            });
        }
        branches.push(Branch {
            path: bp,
            radius: br,
            level: 1,
        });
    }
    let top = h * 1.05;
    let mut out = PlantMeshes {
        height: top,
        cut_height: 0.0,
        ..Default::default()
    };
    for lod in [false, true] {
        let mut b = MeshBuilder::new();
        add_branches(&mut b, sp, &branches, h, lod, seed);
        let mut f = MeshBuilder::new();
        add_cards(&mut f, &cards, &crown, lod, seed ^ 0x5b, sp);
        if lod {
            out.wood_lod = Some(b.finish());
            out.foliage_lod = Some(f.finish());
        } else {
            out.wood = Some(b.finish());
            out.foliage = Some(f.finish());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::PlantLibrary;

    #[test]
    fn trees_have_all_parts_within_their_bounds() {
        let lib = PlantLibrary::default();
        for name in ["oak", "birch", "pine", "spruce"] {
            let sp = lib.get(lib.id(name).unwrap());
            for seed in 0..3 {
                let m = woody(sp, seed, false);
                let base = m.base.as_ref().expect("stump");
                let wood = m.wood.as_ref().unwrap();
                let foliage = m.foliage.as_ref().unwrap();
                assert!(base.triangle_count() > 50, "{name}");
                assert!(wood.triangle_count() > 200, "{name}");
                assert!(foliage.triangle_count() > 80, "{name}");
                let lod = m.wood_lod.as_ref().unwrap().triangle_count()
                    + m.foliage_lod.as_ref().unwrap().triangle_count();
                assert!(
                    lod * 2 < wood.triangle_count() + foliage.triangle_count(),
                    "{name} LOD"
                );
                // Parts share vertical bounds, so they sway together.
                let (a, b) = (wood.aabb(), foliage.aabb());
                assert!((a.min.y - b.min.y).abs() < 1e-4 && (a.max.y - b.max.y).abs() < 1e-4);
                // The crown stays roughly within the species' size.
                assert!(
                    b.max.y <= sp.max_height_m * 1.15,
                    "{name} too tall: {}",
                    b.max.y
                );
                let wide = b.max.x.max(-b.min.x).max(b.max.z).max(-b.min.z);
                assert!(wide < sp.crown_radius_m * 1.8, "{name} too wide: {wide}");
                assert!(m.cut_height > 0.2 && m.cut_height < 1.0);
            }
            let sapling = woody(sp, 1, true);
            assert!(sapling.base.is_none() && sapling.foliage.is_some());
        }
        let hazel = lib.get(lib.id("hazel").unwrap());
        let m = woody(hazel, 3, false);
        assert!(m.base.is_none() && m.foliage.unwrap().triangle_count() > 50);
    }
}
