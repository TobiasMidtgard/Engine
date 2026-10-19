//! Trees and shrubs: a branching skeleton (trunk, limbs, branches, twigs) clothed in leaf and
//! needle cards.
//!
//! A tree is built in three parts so it can be felled: the **base** (roots and the trunk up to
//! the cut, which stays behind as the stump, with growth rings on top), the **wood** above the
//! cut (trunk and branches, which falls) and the **foliage** (leaf cards, which fall with the
//! wood). Standing, the three meet seamlessly. Each part also has a cheaper level of detail.
//!
//! Crowns are built the way the tree grows: limbs reach for the crown's outline, branches
//! fan out from them and twigs carry sprays of leaves (cards textured from the species'
//! leaf atlas). The leaves of each branch form a clump. Cards are shaded with normals bent
//! outward from their clump and the crown, so a crown shades as a rounded, lumpy volume rather
//! than a heap of flat cards, and they darken with depth inside the crown (ambient occlusion
//! baked into the vertex color). Near cards fold along their twig and droop toward the tip;
//! the far level of detail keeps a third of them, flat and larger.
//!
//! Conifers stack whorls of drooping limbs: spruces carry flat needle sprays with curtains of
//! hanging shoots in a tapering cone under a clad spire, pines dense pads of small sprays
//! (branch ends of needle brushes) at the ends of upturned limbs.

use engine_core::math::{Quat, Rng, Vec2, Vec3, Vec4, noise, smoothstep};

use super::bark::{self, BarkKind, CUT_VERTEX_WOOD, layout};
use super::fungi::{self, Host, host_fungus};
use super::textures::{CLUSTER_CELL, LeafKind, atlas_cell, kind_of};
use super::{MeshBuilder, rgb};
use crate::species::{BarkStyle, LeafStyle, PlantForm, PlantSpecies};

/// Meshes for one plant variant. Parts a form doesn't have are `None`.
#[derive(Default)]
pub struct PlantMeshes {
    pub base: Option<engine_render::Mesh>,
    /// The stump (`base`) further on in its weathering, one per [`STUMP_WEATHER`] step after
    /// the first.
    pub base_weathered: Vec<engine_render::Mesh>,
    /// The moss cushions on each of those (same model space).
    pub base_moss: Vec<Option<engine_render::Mesh>>,
    pub wood: Option<engine_render::Mesh>,
    pub foliage: Option<engine_render::Mesh>,
    pub wood_lod: Option<engine_render::Mesh>,
    pub foliage_lod: Option<engine_render::Mesh>,
    /// Thinned copies of `foliage` for leaf fall and leaf-out ([`LEAF_STEPS`]); deciduous
    /// species only.
    pub foliage_thin: Vec<engine_render::Mesh>,
    /// The same for `foliage_lod`.
    pub foliage_lod_thin: Vec<engine_render::Mesh>,
    /// The cheapest level, for trees hundreds of meters off (whole valleys of scenery trees):
    /// the trunk from the ground up on a few four-sided rings with sticks for the big limbs,
    /// and about a tenth of the crown's cards, flat and much bigger. No base goes with it.
    pub wood_distant: Option<engine_render::Mesh>,
    pub foliage_distant: Option<engine_render::Mesh>,
    /// Thinned copies of `foliage_distant` (deciduous species only, as `foliage_thin`).
    pub foliage_distant_thin: Vec<engine_render::Mesh>,
    /// A felled log once its twigs have rotted off: the trunk above the cut with stubs of the
    /// big limbs (same model space as `wood`).
    pub log: Option<engine_render::Mesh>,
    /// The same log further on in its decay, one per [`LOG_MOSS`] step after the first:
    /// mossier and greyer.
    pub log_mossy: Vec<engine_render::Mesh>,
    /// The moss cushions on the log at each [`LOG_MOSS`] step (same model space).
    pub log_moss: Vec<Option<engine_render::Mesh>>,
    /// The same broken trunk standing as a snag: moss at its foot rather than along one side.
    pub snag: Option<engine_render::Mesh>,
    /// Bracket fungi on the log, the snag and the stump (same model space as `log`, `snag`
    /// and `base`).
    pub log_fungi: Option<engine_render::Mesh>,
    pub snag_fungi: Option<engine_render::Mesh>,
    pub stump_fungi: Option<engine_render::Mesh>,
    /// Height of the mesh at full size, meters.
    pub height: f32,
    /// Height of the felling cut (top of the stump), meters.
    pub cut_height: f32,
}

/// A branch of the skeleton. Level 0 is the trunk, 1 the limbs, 2 branches, 3 twigs.
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
    /// Color multiplier (shift toward the accent color, brightness).
    tint: Vec3,
    /// Atlas cell of the leaf texture.
    cell: usize,
    /// Mirror the texture left to right.
    mirror: bool,
    /// How far the tip bends away from the face, as a fraction of the length.
    droop: f32,
    /// How far the side edges fall below the middle (a fold along the twig), as a fraction of
    /// the width.
    fold: f32,
    /// The clump of foliage this card belongs to.
    clump: usize,
    /// Kept, flat and larger, in the far level of detail.
    far: bool,
    /// Centered on `base` instead of growing from it (clusters seen from the shoot tip).
    centered: bool,
}

/// The leaves of one branch, shaded as a rounded volume.
#[derive(Clone, Copy)]
struct Clump {
    center: Vec3,
    radius: f32,
    /// Vertical squash (1 round, below 1 a flat layer whose top faces the sky: spruce tiers).
    flat: f32,
}

/// A point on a branch that twigs can grow from.
#[derive(Clone, Copy)]
struct Anchor {
    at: Vec3,
    radius: f32,
    clump: usize,
    tint: Vec3,
}

/// The crown's outline: an ellipsoid, or a cone (spruces) from `center.y - radii.y` to
/// `center.y + radii.y` with base radius `radii.x`.
#[derive(Clone, Copy)]
struct Crown {
    center: Vec3,
    radii: Vec3,
    cone: bool,
}

impl Crown {
    fn normal(&self, p: Vec3) -> Vec3 {
        if self.cone {
            let radial =
                Vec3::new(p.x - self.center.x, 0.0, p.z - self.center.z).normalize_or(Vec3::X);
            (radial + Vec3::Y * (self.radii.x / (2.0 * self.radii.y)).max(0.15)).normalize()
        } else {
            ((p - self.center) / (self.radii * self.radii)).normalize_or(Vec3::Y)
        }
    }

    /// 0 at the center (the axis of a cone) .. 1 on the surface.
    fn depth(&self, p: Vec3) -> f32 {
        if self.cone {
            let rel =
                ((p.y - (self.center.y - self.radii.y)) / (2.0 * self.radii.y)).clamp(0.0, 1.0);
            let r = (self.radii.x * (1.0 - rel).powf(0.9)).max(0.3);
            Vec2::new(p.x - self.center.x, p.z - self.center.z).length() / r
        } else {
            ((p - self.center) / self.radii).length()
        }
    }

    /// Distance from `p` along `d` to the surface of the ellipsoid (p inside).
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

    /// Horizontal direction from the crown's axis to `p`.
    fn outward(&self, p: Vec3, fallback: Vec3) -> Vec3 {
        Vec3::new(p.x - self.center.x, 0.0, p.z - self.center.z).normalize_or(fallback)
    }
}

/// How a crown's cards are lit.
struct Shading {
    crown: Crown,
    clumps: Vec<Clump>,
    /// Weight of a clump's own normal against the crown's (0 crown only .. 1 clump only).
    clump_weight: f32,
    /// Extra upward bend of the normals: crowns face the sky.
    up: f32,
    /// Vertex color multiplier deep inside the crown (ambient occlusion).
    deep: f32,
}

impl Shading {
    /// Ambient occlusion of wood at `p`: darker than the leaves deep in the crown, since a
    /// limb there is wrapped in foliage on every side.
    fn wood_ao(&self, p: Vec3) -> f32 {
        let deep = self.deep * 0.8;
        deep + (1.0 - deep) * smoothstep(0.1, 1.05, self.crown.depth(p))
    }

    /// Normal and ambient occlusion of a leaf at `p`.
    fn at(&self, p: Vec3, clump: usize) -> (Vec3, f32) {
        let cn = self.crown.normal(p);
        let k = self.clumps.get(clump).copied().unwrap_or(Clump {
            center: self.crown.center,
            radius: self.crown.radii.max_element(),
            flat: 1.0,
        });
        let rel = (p - k.center) / k.radius.max(0.1);
        let kn = (rel * Vec3::new(1.0, 1.0 / k.flat.max(0.2), 1.0)).normalize_or(cn);
        let n = (cn * (1.0 - self.clump_weight) + kn * self.clump_weight + Vec3::Y * self.up)
            .normalize_or(Vec3::Y);
        let crown_ao = self.deep + (1.0 - self.deep) * smoothstep(0.15, 1.0, self.crown.depth(p));
        let clump_ao = 0.8 + 0.2 * smoothstep(0.0, 1.0, rel.length());
        // Leaves facing the ground see less sky.
        let under = 1.0 + 0.2 * n.y.min(0.0);
        (n, crown_ao * clump_ao * under)
    }
}

/// A crown under construction: skeleton, cards and clumps.
struct Crowns {
    crown: Crown,
    branches: Vec<Branch>,
    cards: Vec<Card>,
    clumps: Vec<Clump>,
    /// Card size, meters.
    leaf: f32,
    /// Share of spray cards kept in the far LOD.
    far_share: f32,
    /// Vertices across and along a near card (3×3 folds and droops; 2 skips the fold, a
    /// 2-row card doesn't droop).
    grid: (u32, u32),
    /// Where twigs can grow from to fill the crown's shell.
    anchors: Vec<Anchor>,
}

impl Crowns {
    fn new(crown: Crown, leaf: f32) -> Self {
        Self {
            crown,
            branches: Vec::new(),
            cards: Vec::new(),
            clumps: Vec::new(),
            leaf,
            far_share: 0.3,
            grid: (3, 3),
            anchors: Vec::new(),
        }
    }

    fn clump(&mut self, center: Vec3, radius: f32) -> usize {
        self.clumps.push(Clump {
            center,
            radius,
            flat: 1.0,
        });
        self.clumps.len() - 1
    }

    /// Twig anchors along `path` (from `s0` to its tip) for the shell fill.
    fn anchor_along(&mut self, path: &[Vec3], radius: &[f32], s0: f32, clump: usize, tint: Vec3) {
        for i in 0..4 {
            let s = s0 + (1.0 - s0) * i as f32 / 3.0;
            let (at, _, r) = point_at(path, radius, s);
            self.anchors.push(Anchor {
                at,
                radius: r,
                clump,
                tint,
            });
        }
    }

    /// Clothe the crown's outer layer: points spread over the crown's shell (a few deeper
    /// inside) are pulled onto the nearest branch clump, and a twig grows from the nearest
    /// anchor out to each, carrying leaf sprays. The crown fills without holes between its
    /// limbs, yet stays lumpy, with gaps where no branch reaches. `per_m2` sets how many
    /// twigs per square meter of crown surface.
    fn fill_shell(&mut self, rng: &mut Rng, per_m2: f32, weeping: bool) {
        let c = self.crown;
        let (rh, rv) = (c.radii.x, c.radii.y);
        let area = 4.0 * std::f32::consts::PI * (2.0 * rh * rv + rh * rh) / 3.0;
        let count = (area * per_m2).round().max(4.0) as usize;
        let reach = (rh * 0.45).max(self.leaf * 1.8);
        let leaf = self.leaf;
        for i in 0..count {
            // Fibonacci sphere with jitter.
            let y = 1.0 - 2.0 * (i as f32 + rng.range(0.2, 0.8)) / count as f32;
            if y < -0.55 && rng.chance(0.6) {
                continue;
            }
            let r = (1.0 - y * y).max(0.0).sqrt();
            let a = i as f32 * 2.399_963 + rng.range(-0.2, 0.2);
            let v = Vec3::new(r * a.cos(), y, r * a.sin());
            let depth = if rng.chance(0.2) {
                rng.range(0.45, 0.7)
            } else {
                rng.range(0.74, 0.98)
            };
            let p0 = c.center + v * c.radii * depth;
            // The leaves sit on the nearest clump, so the crown is the lumpy union of its
            // branches' clumps (with gaps where no branch reaches) rather than a smooth ball.
            let Some((ki, k)) = self
                .clumps
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let fa = a.center.distance(p0) / a.radius;
                    let fb = b.center.distance(p0) / b.radius;
                    fa.total_cmp(&fb)
                })
                .map(|(i, k)| (i, *k))
            else {
                return;
            };
            let kd = k.center.distance(p0) / k.radius;
            if kd > 1.9 {
                continue;
            }
            let along = (p0 - k.center).normalize_or(c.normal(p0));
            let p = k.center + along * k.radius * depth.max(0.6) * rng.range(0.85, 1.05);
            let p = if c.depth(p) > 1.12 { p0 } else { p };
            let Some(anchor) = self
                .anchors
                .iter()
                .min_by(|a, b| {
                    a.at.distance_squared(p)
                        .total_cmp(&b.at.distance_squared(p))
                })
                .copied()
            else {
                return;
            };
            let to = p - anchor.at;
            let len = to.length();
            if len > reach || len < 0.05 {
                continue;
            }
            let d = to / len;
            let sag = if weeping { 0.25 } else { 0.06 };
            let kink = jitter(rng, len * 0.08);
            let twig = vec![
                anchor.at,
                anchor.at + to * 0.4 - Vec3::Y * len * sag * 0.6 + kink,
                anchor.at + to * 0.75 - Vec3::Y * len * sag + kink * 0.5,
                p,
            ];
            let out = c.normal(p);
            let tint = anchor.tint * rng.range(0.92, 1.08);
            let clump = if kd < 1.3 { ki } else { anchor.clump };
            let per = if rng.chance(0.55) { 2 } else { 1 };
            for k in 0..per {
                let size = leaf * rng.range(0.85, 1.15);
                if weeping {
                    let dir = (Vec3::NEG_Y * 1.4 + out * 0.45 + jitter(rng, 0.25)).normalize();
                    let face = (c.outward(p, out) + Vec3::Y * 0.3).normalize();
                    let face = rotate(face, dir, rng.range(-0.5, 0.5));
                    let base = p + Vec3::Y * size * 0.15 - d * size * 0.2 * k as f32;
                    self.spray(rng, base, dir, face, size, clump, tint, -0.08);
                } else {
                    let dir = (d * 0.5 + out * 0.7 + jitter(rng, 0.3)).normalize();
                    let face = (out * 0.6 + Vec3::Y * 0.8).normalize();
                    let face = rotate(face, dir, rng.range(-0.7, 0.7));
                    let out = (p - self.clumps[clump].center).normalize_or(out);
                    let dir = (dir + out * 0.3).normalize();
                    let base = p - dir * size * (0.35 + 0.3 * k as f32);
                    self.spray(rng, base, dir, face, size, clump, tint, 0.14);
                }
            }
            if !weeping && depth > 0.8 && rng.chance(0.3) {
                let size = leaf * rng.range(0.65, 0.85);
                self.cluster(rng, p + out * leaf * 0.1, out, size, clump, tint);
            }
            let r0 = (anchor.radius * 0.55)
                .clamp(0.007, 0.03)
                .min(0.006 + len * 0.012);
            self.branches.push(Branch {
                path: twig,
                radius: vec![r0, r0 * 0.75, r0 * 0.5, 0.004],
                level: 3,
            });
        }
    }

    /// A twig spray growing from `base` along `dir`, facing `face`.
    #[allow(clippy::too_many_arguments)]
    fn spray(
        &mut self,
        rng: &mut Rng,
        base: Vec3,
        dir: Vec3,
        face: Vec3,
        size: f32,
        clump: usize,
        tint: Vec3,
        droop: f32,
    ) {
        let cells = [0, 1, 3];
        self.cards.push(Card {
            base,
            dir,
            face,
            width: size * rng.range(0.85, 1.05),
            length: size,
            tint: tint * rng.range(0.9, 1.08),
            cell: cells[rng.range_i32(0, 3) as usize],
            mirror: rng.chance(0.5),
            droop: droop * rng.range(0.6, 1.3),
            fold: rng.range(0.08, 0.22),
            clump,
            far: rng.chance(self.far_share),
            centered: false,
        });
    }

    /// A cluster seen from its shoot tip, centered at `at` and facing `out`.
    fn cluster(&mut self, rng: &mut Rng, at: Vec3, out: Vec3, size: f32, clump: usize, tint: Vec3) {
        let up = (Vec3::Y - out * out.y).normalize_or(out.any_orthonormal_vector());
        self.cards.push(Card {
            base: at,
            dir: rotate(up, out, rng.range(0.0, std::f32::consts::TAU)),
            face: out,
            width: size,
            length: size,
            tint: tint * rng.range(0.92, 1.08),
            cell: CLUSTER_CELL,
            mirror: rng.chance(0.5),
            droop: -0.12,
            fold: 0.1,
            clump,
            far: rng.chance(self.far_share * 1.5),
            centered: true,
        });
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
    /// bounds. Also records the plant's height. The distant level shares bounds of its own
    /// (its big cards reach a little past the crown), so the near ones and the plant's height
    /// stay as they were.
    pub fn share_bounds(&mut self) {
        fn bounds<'a>(parts: impl IntoIterator<Item = &'a engine_render::Mesh>) -> (f32, f32) {
            parts.into_iter().fold((0.0f32, 0.0f32), |(lo, hi), m| {
                let b = m.aabb();
                (lo.min(b.min.y), hi.max(b.max.y))
            })
        }
        fn widen<'a>(
            parts: impl IntoIterator<Item = &'a mut engine_render::Mesh>,
            lo: f32,
            hi: f32,
        ) {
            for m in parts {
                for y in [lo, hi] {
                    m.vertices.push(engine_render::Vertex::new(
                        Vec3::new(0.0, y, 0.0),
                        Vec3::Y,
                        Vec2::ZERO,
                    ));
                }
            }
        }
        let (lo, hi) = bounds(
            [&self.wood, &self.foliage, &self.wood_lod, &self.foliage_lod]
                .into_iter()
                .flatten(),
        );
        // Thinned crowns lie within the full one, and must sway with it.
        widen(
            [
                &mut self.wood,
                &mut self.foliage,
                &mut self.wood_lod,
                &mut self.foliage_lod,
            ]
            .into_iter()
            .flatten()
            .chain(&mut self.foliage_thin)
            .chain(&mut self.foliage_lod_thin),
            lo,
            hi,
        );
        let (dlo, dhi) = bounds(
            [&self.wood_distant, &self.foliage_distant]
                .into_iter()
                .flatten(),
        );
        widen(
            [&mut self.wood_distant, &mut self.foliage_distant]
                .into_iter()
                .flatten()
                .chain(&mut self.foliage_distant_thin),
            lo.min(dlo),
            hi.max(dhi),
        );
        self.height = hi;
    }
}

/// Cut height for a species (stump top), meters.
pub fn cut_height(sp: &PlantSpecies) -> f32 {
    (0.35 + sp.trunk_radius_m * 0.5).clamp(0.3, 0.75)
}

/// A point on a path at fraction `s` of its points: position, direction and radius.
pub(super) fn point_at(path: &[Vec3], radius: &[f32], s: f32) -> (Vec3, Vec3, f32) {
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
    Quat::from_axis_angle(axis, angle) * v
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

/// How a log mosses over as it rots: from each share of its life ([`crate::FelledLog`]'s
/// `years / rot_years`) on, how far the moss has spread (`moss_log`).
pub const LOG_MOSS: [(f32, f32); 3] = [(0.0, 0.15), (0.25, 0.55), (0.5, 1.0)];

/// Moss over the model's +X side of a log (the side `felling::log_rest_transform` turns up
/// once the twigs have rotted off), patchy along it, and a darker, damp underside. The cut
/// faces stay bare. `amount` (0..1) is how far it has spread: a few cushions along the top of
/// a log down a few years, a thick coat over the top and down the flanks of an old one. The
/// bark between the cushions dulls and greys with it, green with algae in places. The
/// cushions themselves stand on the bark as a separate shell ([`moss_shell`]).
fn moss_log(mesh: &mut engine_render::Mesh, amount: f32, wood: CutWood, seed: u64) {
    // Olive, not lawn green: moss on deadwood is dark and dull, drier and yellower in places.
    let moss = Vec3::new(0.052, 0.07, 0.026);
    let dry = Vec3::new(0.075, 0.074, 0.036);
    // Soil and litter stain the underside.
    let soil = Vec3::new(0.07, 0.052, 0.038);
    for v in &mut mesh.vertices {
        let c = Vec3::new(v.color[0], v.color[1], v.color[2]);
        if let Some(at) = on_cut(v)
            && v.normal[1] < -0.5
        {
            // The butt's cut face (the model's −Y) silvers, then rots and mosses over, a step
            // ahead of the bark: the log was bare for a year or more before it got here.
            let (c, _) = weather_cut(c, at, (0.45 + amount).min(1.0), wood, seed);
            v.color = [c.x, c.y, c.z, v.color[3]];
            continue;
        }
        let m = MossAt::new(v, amount, seed);
        let moss = moss.lerp(dry, smoothstep(-0.2, 0.4, m.tint));
        // Dead bark loses its color within a year or two: it greys and dulls (birch's white
        // with it, pine's orange), greening with algae in places. Pale bark also dirties, in
        // blotches, so a birch log doesn't lie on the floor like a white pipe.
        let lum = c.dot(Vec3::new(0.3, 0.55, 0.15));
        let algae = smoothstep(-0.3, 0.5, m.patch) * m.side;
        let grey = Vec3::new(0.74, 0.69, 0.62).lerp(Vec3::new(0.62, 0.66, 0.5), 0.5 * algae) * lum;
        let pale = smoothstep(0.2, 0.55, lum);
        let grime = 1.0 - pale * (0.25 + 0.2 * amount + 0.25 * smoothstep(-0.1, 0.5, m.tint));
        let c = c.lerp(grey, 0.25 + 0.35 * amount) * grime;
        let c = c.lerp(soil, (0.3 + 0.3 * pale) * m.under * m.side);
        let cover = m.cover * (0.6 + 0.3 * amount);
        let c = c.lerp(moss, cover) * (1.0 - (0.2 + 0.2 * amount) * m.under * m.side);
        v.color = [c.x, c.y, c.z, v.color[3]];
    }
}

/// How a species' cut faces look and weather ([`weather_cut`]).
#[derive(Debug, Clone, Copy)]
struct CutWood {
    /// The mean color its cut faces are painted in ([`bark::cut_wood_mean`]).
    mean: Vec3,
    /// Whether a durable heartwood outlasts the sapwood ([`bark::has_heartwood`]).
    heartwood: bool,
}

impl CutWood {
    fn of(sp: &PlantSpecies) -> Self {
        let fresh = Vec3::from(CUT_VERTEX_WOOD) * 0.93;
        let kind = BarkKind::for_species(sp);
        Self {
            mean: kind.map_or(fresh, |k| Vec3::from(bark::cut_wood_mean(k))),
            heartwood: kind.is_some_and(bark::has_heartwood),
        }
    }

    /// The vertex color that shows a cut face in `color` (linear RGB): the end-grain texture
    /// is painted for the fresh wood's vertex color.
    fn tint(&self, color: Vec3) -> Vec3 {
        color * Vec3::from(CUT_VERTEX_WOOD) * 0.93 / self.mean.max(Vec3::splat(1e-3))
    }
}

/// Where a vertex lies on a cut face: its relative radius (0 center .. 1 rim) and angle,
/// from its texture coordinates in the bark atlas' end-grain disc. `None` on bark.
fn on_cut(v: &engine_render::Vertex) -> Option<(f32, f32)> {
    if v.uv[0] < 0.5 {
        return None;
    }
    let [cu, cv] = layout::CUT_CENTER;
    let [ru, rv] = layout::CUT_RADIUS;
    let d = Vec2::new((v.uv[0] - cu) / ru, (v.uv[1] - cv) / rv);
    Some((d.length().min(1.0), d.y.atan2(d.x)))
}

/// A cut face's vertex color `c` at `(r, angle)` ([`on_cut`]) after weathering `amount`
/// (0 fresh .. 1 old), and how mossy it is there (0..1). The pale fresh wood silvers within a
/// year or two and darkens; then it rots dark and soft, from the heart out or (round a
/// durable heartwood) in the sapwood, and moss creeps in from the bark. The bark ring round
/// it darkens.
fn weather_cut(c: Vec3, (r, a): (f32, f32), amount: f32, wood: CutWood, seed: u64) -> (Vec3, f32) {
    if amount <= 0.0 {
        return (c, 0.0);
    }
    let q = Vec2::new(a.cos(), a.sin()) * r;
    let k = noise::fbm3(
        q.x as f64 * 2.5,
        q.y as f64 * 2.5,
        7.3,
        seed ^ 0xc0f7,
        3,
        2.0,
        0.5,
    );
    let grey = smoothstep(0.0, 0.4, amount);
    let soft = smoothstep(0.4, 1.0, amount);
    let mossy = smoothstep(0.75, 1.0, r + 0.3 * k + 0.25 * soft) * soft;
    if r > 0.92 {
        return (c * (1.0 - 0.35 * amount), mossy);
    }
    let silver = Vec3::new(0.16, 0.15, 0.14).lerp(Vec3::new(0.085, 0.078, 0.07), soft);
    let rot = Vec3::new(0.045, 0.032, 0.02);
    let moss = Vec3::new(0.052, 0.07, 0.026);
    // The growth rings' shading of the fresh face, kept as it greys.
    let fresh = Vec3::from(CUT_VERTEX_WOOD) * 0.93;
    let ring = (c.dot(Vec3::ONE) / fresh.dot(Vec3::ONE)).clamp(0.6, 1.2);
    let rots_here = if wood.heartwood {
        smoothstep(0.45, 0.75, r)
    } else {
        1.0 - smoothstep(0.35, 0.8, r)
    };
    let rotten = smoothstep(0.3, 0.7, rots_here + 0.35 * k) * soft;
    let t = silver.lerp(rot, rotten).lerp(moss, 0.6 * mossy) * ring;
    (c.lerp(wood.tint(t), grey.max(soft)), mossy)
}

/// How a stump weathers: from each age on (years since the cut), how far it has gone
/// ([`weather_stump`]). A fresh cut, then silvered wood with moss at the foot, then a dark,
/// soft, mossy top sinking as it rots (stumps last about ten years).
pub const STUMP_WEATHER: [(f32, f32); 3] = [(0.0, 0.0), (1.5, 0.45), (5.0, 1.0)];

/// The stump (`base`: roots and trunk to the cut) after weathering `amount`
/// ([`STUMP_WEATHER`]), and its moss cushions. The cut face silvers, rots and mosses over
/// ([`weather_cut`]) and sinks into a hollow (rotting from the heart) or slumps round a
/// heartwood that stands proud; the bark dulls, and moss climbs it from the ground, over the
/// roots, and creeps over the top edge. The cushions are drawn like a log's ([`moss_shell`]).
fn weather_stump(
    base: &engine_render::Mesh,
    amount: f32,
    wood: CutWood,
    seed: u64,
) -> (engine_render::Mesh, Option<engine_render::Mesh>) {
    let mut m = base.clone();
    // The cut face's center and radius.
    let cap: Vec<Vec3> = m
        .vertices
        .iter()
        .filter(|v| on_cut(v).is_some())
        .map(|v| Vec3::from(v.position))
        .collect();
    let center = cap.iter().copied().sum::<Vec3>() / cap.len().max(1) as f32;
    let rim = cap
        .iter()
        .map(|p| (*p - center).with_y(0.0).length())
        .fold(0.0f32, f32::max);
    let cut = center.y;
    let sink = smoothstep(0.45, 1.0, amount) * rim;
    let depth = |r: f32| {
        if wood.heartwood {
            0.12 * sink * smoothstep(0.5, 0.8, r) * (1.0 - smoothstep(0.88, 1.0, r))
        } else {
            0.35 * sink * (1.0 - (r / 0.92).powi(2)).max(0.0)
        }
    };
    let moss = Vec3::new(0.052, 0.07, 0.026);
    // Moss cover and dryness at each vertex, for the cushions.
    let mut cover = vec![(0.0f32, 0.0f32); m.vertices.len()];
    for (v, cover) in m.vertices.iter_mut().zip(&mut cover) {
        let c = Vec3::new(v.color[0], v.color[1], v.color[2]);
        let p = Vec3::from(v.position);
        let tint = noise::fbm3(
            p.x as f64 * 2.3,
            p.y as f64 * 2.3,
            p.z as f64 * 2.3,
            seed ^ 0x0d27,
            2,
            2.0,
            0.5,
        );
        let c = if let Some((r, a)) = on_cut(v) {
            // Sink the face, and tilt its normal up the slope of the hollow.
            let e = 0.01;
            let slope = (depth(r - e) - depth(r + e)) / (2.0 * e * rim.max(1e-3));
            let radial = (p - center).with_y(0.0).normalize_or_zero();
            v.position[1] -= depth(r);
            v.normal = (Vec3::Y - radial * slope).normalize().to_array();
            let (c, mossy) = weather_cut(c, (r, a), amount, wood, seed);
            *cover = (mossy, tint);
            c
        } else {
            let patch = noise::fbm3(
                p.x as f64 * 2.1,
                p.y as f64 * 1.4,
                p.z as f64 * 2.1,
                seed ^ 0x5707,
                3,
                2.0,
                0.5,
            );
            let reach = (0.06 + 0.35 * amount) * (0.6 + 0.8 * (patch + 0.5).clamp(0.0, 1.0));
            let foot = 1.0 - smoothstep(0.3 * reach, reach, p.y);
            let edge =
                smoothstep(cut - 0.05 - 0.1 * amount, cut, p.y) * smoothstep(0.5, 1.0, amount);
            let moss_here =
                foot.max(0.8 * edge) * smoothstep(-0.2, 0.25, patch) * smoothstep(0.0, 0.4, amount);
            *cover = (moss_here, tint);
            let lum = c.dot(Vec3::new(0.3, 0.55, 0.15));
            let c = c.lerp(Vec3::new(0.84, 0.83, 0.78) * lum, 0.3 * amount) * (1.0 - 0.15 * amount);
            c.lerp(moss, 0.5 * moss_here)
        };
        v.color = [c.x, c.y, c.z, v.color[3]];
    }

    // The cushions: the mossy surface lifted a little, textured flat across the top and
    // round the sides.
    let around = |p: Vec3| (p.z - center.z).atan2(p.x - center.x);
    let mut b = MeshBuilder::new();
    let mut remap = vec![u32::MAX; m.vertices.len()];
    for tri in m.indices.as_chunks::<3>().0 {
        let ids = tri.map(|i| i as usize);
        if ids.iter().all(|&i| cover[i].0 < 0.03) {
            continue;
        }
        let top = on_cut(&m.vertices[ids[0]]).is_some();
        if !top {
            // Skip the seam where the angle wraps round.
            let angles = ids.map(|i| around(Vec3::from(m.vertices[i].position)));
            let spread = angles
                .iter()
                .fold(0.0f32, |s, a| s.max((a - angles[0]).abs()));
            if spread > std::f32::consts::PI {
                continue;
            }
        }
        let mut out = [0u32; 3];
        for (k, &i) in ids.iter().enumerate() {
            if remap[i] == u32::MAX {
                let v = &m.vertices[i];
                let (amount, tint) = cover[i];
                let p = Vec3::from(v.position);
                let n = Vec3::from(v.normal).normalize_or_zero();
                let uv = if top {
                    Vec2::new(p.x, p.z)
                } else {
                    Vec2::new(around(p) * rim.max(0.1), p.y)
                } / LOG_MOSS_TILE_M;
                let c = cushion_color(smoothstep(-0.2, 0.4, tint));
                let alpha = smoothstep(0.0, 0.8, amount);
                let lift = 0.004 + 0.016 * amount;
                remap[i] = b.vertex(p + n * lift, n, uv, c.extend(alpha));
            }
            out[k] = remap[i];
        }
        b.tri(out[0], out[1], out[2]);
    }
    (m, (!b.is_empty()).then(|| b.finish()))
}

/// The irregular cross-section of a trunk, standing or felled: shallow flutes round it, bulges where old
/// knots were and a slow wander of its girth along it, as a radial scale about the trunk's axis
/// (`path` with radii `rad`) that depends only on the angle round it and the height along it.
/// Everything built on the trunk's surface (stubs, cut faces, fungi) moves with it, so nothing
/// opens up or floats.
struct TrunkShape<'a> {
    path: &'a [Vec3],
    rad: &'a [f32],
    seed: u64,
    phase: [f32; 3],
}

impl<'a> TrunkShape<'a> {
    fn new(path: &'a [Vec3], rad: &'a [f32], seed: u64) -> Self {
        let h = |k: i64| engine_core::math::hash01(k, 3, 9, seed) * std::f32::consts::TAU;
        Self {
            path,
            rad,
            seed,
            phase: [h(1), h(2), h(3)],
        }
    }

    /// The trunk's axis point and radius at height `y` (model space, the trunk along +Y).
    /// Points past either end take the end's.
    fn axis_at(&self, y: f32) -> (Vec3, f32) {
        let (path, rad) = (self.path, self.rad);
        let i = path
            .windows(2)
            .position(|w| y < w[1].y)
            .unwrap_or(path.len() - 2);
        let t = ((y - path[i].y) / (path[i + 1].y - path[i].y).max(1e-4)).clamp(0.0, 1.0);
        (
            path[i].lerp(path[i + 1], t),
            rad[i] + (rad[i + 1] - rad[i]) * t,
        )
    }

    /// Radial scale at angle `a` round the axis and height `y`: 1 ± about a tenth.
    fn scale(&self, a: f32, y: f32) -> f32 {
        let [p1, p2, p3] = self.phase;
        // Flutes: a few broad lobes, twisting slowly along the trunk.
        let twist = 0.12 * y;
        let flutes = 0.05 * (3.0 * (a + twist) + p1).sin() + 0.03 * (5.0 * (a - twist) + p2).sin();
        // Knots and burls: a few rounded swellings.
        let (c, s) = (a.cos() as f64, a.sin() as f64);
        let knot = noise::fbm3(
            c * 1.4,
            y as f64 * 1.1,
            s * 1.4,
            self.seed ^ 0x6b_0075,
            2,
            2.0,
            0.5,
        );
        let burls = 0.11 * smoothstep(0.2, 0.7, knot);
        // Girth wandering along the trunk.
        let girth = 0.03 * (0.7 * y + p3).sin();
        1.0 + flutes + burls + girth
    }

    /// Reshape `mesh` (built on this trunk) by [`Self::scale`], bending its normals with the
    /// surface.
    fn roughen(&self, mesh: &mut engine_render::Mesh) {
        self.roughen_vertices(&mut mesh.vertices);
    }

    fn roughen_vertices(&self, vertices: &mut [engine_render::Vertex]) {
        let da = 0.02;
        for v in vertices {
            let p = Vec3::from(v.position);
            let (c, _) = self.axis_at(p.y);
            let rel = Vec3::new(p.x - c.x, 0.0, p.z - c.z);
            let a = rel.z.atan2(rel.x);
            let k = self.scale(a, p.y);
            // The slope of the surface round the trunk tips the normal sideways.
            let dk = (self.scale(a + da, p.y) - self.scale(a - da, p.y)) / (2.0 * da);
            let along = Vec3::new(-a.sin(), 0.0, a.cos());
            let n = Vec3::from(v.normal);
            let radial = n.dot(Vec3::new(a.cos(), 0.0, a.sin())).max(0.0);
            let n = (n - along * (dk / k) * radial).normalize_or(n);
            let q = c + rel * k + Vec3::Y * (p.y - c.y);
            v.position = q.to_array();
            v.normal = n.to_array();
        }
    }

    /// Slump `mesh` by `amount` (0..1): an old log's rotten heart gives way, so it settles
    /// lower and wider, flattening most on its top (the model's +X side, turned up).
    fn slump(&self, mesh: &mut engine_render::Mesh, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        // It sags along its length too, its middle settling into the litter.
        let (y0, y1) = (self.path[0].y, self.path[self.path.len() - 1].y);
        let r0 = self.rad[0];
        for v in &mut mesh.vertices {
            let p = Vec3::from(v.position);
            let (c, _) = self.axis_at(p.y);
            let (mut q, n) = Self::flatten(p, Vec3::from(v.normal), c, amount);
            let t = ((p.y - y0) / (y1 - y0).max(1e-3)).clamp(0.0, 1.0);
            q.x -= 0.3 * r0 * amount * (std::f32::consts::PI * t).sin();
            v.position = q.to_array();
            v.normal = n.to_array();
        }
    }

    fn flatten(p: Vec3, n: Vec3, c: Vec3, amount: f32) -> (Vec3, Vec3) {
        // Only the top sinks: the underside stays on the ground.
        let x = p.x - c.x;
        let sx = 1.0 - if x > 0.0 { 0.2 * amount } else { 0.0 };
        let sz = 1.0 + 0.07 * amount;
        let q = Vec3::new(c.x + x * sx, p.y, c.z + (p.z - c.z) * sz);
        let n = Vec3::new(n.x / sx, n.y, n.z / sz).normalize_or(n);
        (q, n)
    }
}

/// How mossy a log's surface is at a vertex (see [`moss_log`]).
struct MossAt {
    /// Moss cover, 0..1.
    cover: f32,
    /// Facing the ground (the model's −X), 0..1.
    under: f32,
    /// Facing sideways rather than along the trunk (cut faces are 0).
    side: f32,
    /// Patch and tint noise, about −1..1.
    patch: f32,
    tint: f32,
}

impl MossAt {
    fn new(v: &engine_render::Vertex, amount: f32, seed: u64) -> Self {
        let n = Vec3::from(v.normal).normalize_or_zero();
        let side = 1.0 - n.y.abs();
        let p = Vec3::from(v.position);
        let patch = noise::fbm3(
            p.x as f64 * 1.3,
            p.y as f64 * 0.8,
            p.z as f64 * 1.3,
            seed ^ 0x6d055,
            3,
            2.0,
            0.5,
        );
        let tint = noise::fbm3(
            p.x as f64 * 2.1,
            p.y as f64 * 1.7,
            p.z as f64 * 2.1,
            seed ^ 0x0d27,
            2,
            2.0,
            0.5,
        );
        // Cushions with bark between them, mostly along the top, spreading down the flanks
        // and closing up as the years go by.
        let reach = -0.15 - 0.3 * patch - 0.6 * (amount - 0.5);
        let top = smoothstep(reach, reach + 0.7, n.x)
            * smoothstep(0.15 - 0.5 * amount, 0.4 - 0.3 * amount, patch);
        Self {
            cover: top * side,
            under: smoothstep(0.0, 0.7, -n.x),
            side,
            patch,
            tint,
        }
    }
}

/// The vertex color of moss cushions on deadwood (over the forest floor's moss texture),
/// `dry` 0..1: darker and more olive than the moss on the floor (bright moss reads lime in
/// sunlight), drier and browner in places.
fn cushion_color(dry: f32) -> Vec3 {
    Vec3::new(0.27, 0.24, 0.15).lerp(Vec3::new(0.36, 0.29, 0.17), dry)
}

/// Size of the moss texture's tile on deadwood, meters.
pub const LOG_MOSS_TILE_M: f32 = 1.2;

/// The moss cushions on a log ([`moss_log`] with the same `amount` and `seed`): the log's own
/// surface lifted a centimeter or two wherever moss covers it, to be drawn with the forest
/// floor's moss texture, alpha-masked. Its vertex alpha is the cover, so thin cover shows only
/// the tallest cushions and patches end raggedly; texture coordinates wrap round the trunk
/// (`path` with radii `rad`) at [`LOG_MOSS_TILE_M`]. `None` if nothing is mossy.
fn moss_shell(
    log: &engine_render::Mesh,
    path: &[Vec3],
    rad: &[f32],
    amount: f32,
    seed: u64,
) -> Option<engine_render::Mesh> {
    let moss: Vec<MossAt> = log
        .vertices
        .iter()
        .map(|v| MossAt::new(v, amount, seed))
        .collect();
    // Where round the trunk each vertex is: the angle and the trunk's radius there.
    let around = |p: Vec3| {
        let i = path
            .windows(2)
            .position(|w| p.y < w[1].y)
            .unwrap_or(path.len() - 2);
        let t = ((p.y - path[i].y) / (path[i + 1].y - path[i].y).max(1e-4)).clamp(0.0, 1.0);
        let c = path[i].lerp(path[i + 1], t);
        let r = rad[i] + (rad[i + 1] - rad[i]) * t;
        ((p.z - c.z).atan2(p.x - c.x), r)
    };
    let mut b = MeshBuilder::new();
    let mut remap = vec![u32::MAX; log.vertices.len()];
    for tri in log.indices.as_chunks::<3>().0 {
        let ids = tri.map(|i| i as usize);
        if ids.iter().all(|&i| moss[i].cover < 0.03) {
            continue;
        }
        // Skip the seam on the underside where the angle wraps round.
        let angles = ids.map(|i| around(Vec3::from(log.vertices[i].position)).0);
        let spread = angles
            .iter()
            .fold(0.0f32, |m, a| m.max((a - angles[0]).abs()));
        if spread > std::f32::consts::PI {
            continue;
        }
        let mut out = [0u32; 3];
        for (k, &i) in ids.iter().enumerate() {
            if remap[i] == u32::MAX {
                let v = &log.vertices[i];
                let m = &moss[i];
                let p = Vec3::from(v.position);
                let n = Vec3::from(v.normal).normalize_or_zero();
                let (angle, r) = around(p);
                let uv = Vec2::new(angle * r, p.y) / LOG_MOSS_TILE_M;
                // Darker down the flanks.
                let c = cushion_color(smoothstep(-0.2, 0.4, m.tint)) * (1.0 - 0.3 * m.under);
                let alpha = smoothstep(0.0, 0.8, m.cover);
                let lift = 0.004 + 0.014 * m.cover;
                remap[i] = b.vertex(p + n * lift, n, uv, c.extend(alpha));
            }
            out[k] = remap[i];
        }
        b.tri(out[0], out[1], out[2]);
    }
    (!b.is_empty()).then(|| b.finish())
}

/// Break the end of a trunk at `top` (radius `r`, pointing along `axis`) off jaggedly: the
/// rotten heart sinks into a hollow and the sound wood around it stands in uneven teeth. Their
/// outsides keep the `bark`; their torn faces show weathered grey wood, its fibers running up
/// the teeth. Call it right after the tube whose end cap is the heart.
fn break_top(b: &mut MeshBuilder, top: Vec3, axis: Vec3, r: f32, bark: Vec4, seed: u64) {
    use std::f32::consts::TAU;
    // Sink the heart: the cap's inner rings drop below the rim.
    for v in &mut b.vertices {
        let p = Vec3::from(v.position) - top;
        let rel = (p - axis * p.dot(axis)).length() / r;
        if p.dot(axis).abs() < 0.1 * r && rel < 0.97 {
            let sink = 0.55 * r * (1.0 - rel).powf(0.7);
            v.position = (Vec3::from(v.position) - axis * sink).into();
        }
    }
    let (n, m) = axis.any_orthonormal_pair();
    let dir = |a: f32| n * a.cos() + m * a.sin();
    let tangent = |a: f32| m * a.cos() - n * a.sin();
    let rnd = |k: u32, salt: i64| engine_core::math::hash01(k as i64, salt, 0, seed);
    let wood = Vec4::new(0.3, 0.29, 0.27, 1.0);
    let rot = Vec4::new(0.025, 0.018, 0.013, 1.0);
    let [cu, cv] = layout::CUT_CENTER;
    let [ru, rv] = layout::CUT_RADIUS;
    // Torn wood samples the end-grain disc sideways: across a face runs across the growth
    // rings, up it runs along them, so the rings read as fibers running up the tooth.
    let grain = |across: f32, up: f32, k: u32| {
        let rho = 0.5 + 0.4 * across;
        let th = TAU * rnd(k, 9) + 0.35 * up / r;
        Vec2::new(cu + ru * rho * th.cos(), cv + rv * rho * th.sin())
    };
    let tile_u = layout::BARK_U1 - layout::BARK_U0;
    let tile_v = layout::TILE_WIDTH_M * layout::TILE_ASPECT;
    let count = 11;
    for k in 0..count {
        // Now and then a tooth has broken away entirely.
        if rnd(k, 1) < 0.15 {
            continue;
        }
        let step = TAU / count as f32;
        let a0 = (k as f32 + 0.2 * rnd(k, 2)) * step;
        let a1 = a0 + step * (0.6 + 0.4 * rnd(k, 3));
        let mid = 0.5 * (a0 + a1) + 0.3 * (a1 - a0) * (rnd(k, 4) - 0.5);
        let half = 0.12 * (a1 - a0);
        // Mostly short stubs, a few long slivers.
        let tall = r * (0.2 + 1.9 * rnd(k, 5).powi(2));
        let inner = r * (0.6 + 0.25 * rnd(k, 6));
        let tip_r = r * (0.82 + 0.12 * rnd(k, 7));
        let base = -0.01 * r;
        let at = |a: f32, radius: f32, s: f32| top + dir(a) * radius + axis * s;
        let o0 = at(a0, r, base);
        let o1 = at(a1, r, base);
        let i0 = at(a0, inner, base);
        let i1 = at(a1, inner, base);
        let t0 = at(mid - half, tip_r, tall * (0.75 + 0.2 * rnd(k, 8)));
        let t1 = at(mid + half, tip_r, tall);
        let up = |p: Vec3| (p - top).dot(axis);
        let fade = |p: Vec3| rot.lerp(wood, (up(p) / (0.3 * r)).clamp(0.0, 1.0));
        // The bark side: the tube's bark tile carried on up.
        let u = |a: f32| layout::BARK_U0 + tile_u * (0.1 + (a - a0) * r / layout::TILE_WIDTH_M);
        let outer = [o0, o1, t1, t0].map(|p| {
            let a = (p - top).dot(m).atan2((p - top).dot(n));
            let a = a0 + (a - a0).rem_euclid(TAU);
            (
                p,
                Vec2::new(u(a).min(layout::BARK_U1), 0.5 - up(p) / tile_v),
                bark,
            )
        });
        flat_face(b, &outer, dir(mid));
        let torn = |p: Vec3, across: f32| (p, grain(across, up(p), k), fade(p));
        flat_face(
            b,
            &[torn(i1, 1.0), torn(i0, 0.0), torn(t0, 0.3), torn(t1, 0.7)],
            -dir(mid),
        );
        flat_face(
            b,
            &[torn(i0, 0.0), torn(o0, 1.0), torn(t0, 0.6)],
            -tangent(a0),
        );
        flat_face(
            b,
            &[torn(o1, 1.0), torn(i1, 0.0), torn(t1, 0.6)],
            tangent(a1),
        );
    }
}

/// A flat polygon (a fan, convex) with its own vertices, wound to face `out`.
fn flat_face(b: &mut MeshBuilder, corners: &[(Vec3, Vec2, Vec4)], out: Vec3) {
    let normal = (corners[1].0 - corners[0].0).cross(corners[2].0 - corners[0].0);
    let flip = normal.dot(out) < 0.0;
    let normal = if flip { -normal } else { normal };
    let ids: Vec<u32> = corners
        .iter()
        .map(|&(p, uv, c)| b.vertex(p, normal, uv, c))
        .collect();
    for i in 1..ids.len() as u32 - 1 {
        let (x, y) = if flip { (i + 1, i) } else { (i, i + 1) };
        b.tri(ids[0], ids[x as usize], ids[y as usize]);
    }
}

/// Moss around the foot of a standing snag, in patches, thinning out a meter or so up.
fn moss_foot(mesh: &mut engine_render::Mesh, seed: u64) {
    let moss = Vec3::new(0.052, 0.07, 0.026);
    for v in &mut mesh.vertices {
        let n = Vec3::from(v.normal).normalize_or_zero();
        let side = 1.0 - n.y.abs();
        let p = Vec3::from(v.position);
        let patch = noise::fbm3(
            p.x as f64 * 1.3,
            p.y as f64 * 0.8,
            p.z as f64 * 1.3,
            seed ^ 0x6d055,
            3,
            2.0,
            0.5,
        );
        let reach = 0.6 + 0.8 * (patch + 0.5).clamp(0.0, 1.0);
        let foot = (1.0 - smoothstep(0.2, reach, p.y)) * smoothstep(-0.2, 0.2, patch);
        let c = Vec3::new(v.color[0], v.color[1], v.color[2]);
        let c = c.lerp(moss, foot * side * 0.75);
        v.color = [c.x, c.y, c.z, v.color[3]];
    }
}

/// Sides of the trunk near the camera. The stump and the wood above it use the same count so
/// their rings meet at the cut without cracks (which would show the cut faces through them).
const TRUNK_SIDES: u32 = 14;

/// Vertex color for bark at height `y`.
fn bark_color(sp: &PlantSpecies, y: f32, h: f32, ring: usize, seed: u64) -> Vec4 {
    let n = engine_core::math::hash01(ring as i64, 0, 0, seed);
    let base = Vec3::from(sp.bark_color);
    let c = match sp.shape.bark {
        BarkStyle::Birch => {
            // Old birches go black and rough at the foot, fading up into the white over the
            // first meter or two, unevenly from ring to ring; young ones barely darken.
            let dark = Vec3::new(0.035, 0.03, 0.028);
            let reach = (h / 14.0).clamp(0.2, 1.0) * (1.3 + 0.9 * n);
            let foot = 1.0 - smoothstep(0.2 * reach, reach, y);
            // The bark texture carries the dark lenticel bands; rings only vary gently.
            let band = 0.85 + 0.15 * n;
            base.lerp(dark, foot) * band.max(foot)
        }
        BarkStyle::Pine => {
            // The pine bark texture is painted for this foot color (× the mean ring
            // variation); keep the two equal.
            let foot = Vec3::new(0.12, 0.09, 0.07);
            foot.lerp(base, smoothstep(0.3 * h, 0.6 * h, y)) * (0.85 + 0.25 * n)
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
fn cut_color(rim: Vec4, r: f32) -> Vec4 {
    if r > 0.92 {
        rim
    } else {
        // Equal to the cut-wood color the end-grain texture is painted for.
        let wood = Vec3::new(0.62, 0.45, 0.26);
        let ring = 0.86 + 0.14 * (r * 23.0).cos();
        (wood * ring * (0.9 + 0.1 * r)).extend(1.0)
    }
}

/// Limbs and twigs; `shade` darkens wood deep inside the crown, like the leaves around it
/// (little sky reaches a spruce's inner limbs).
fn add_branches(
    b: &mut MeshBuilder,
    sp: &PlantSpecies,
    branches: &[Branch],
    h: f32,
    lod: bool,
    seed: u64,
    shade: &Shading,
) {
    let many = branches.iter().filter(|b| b.level == 1).count() > 30;
    let spruce_like = sp.form == PlantForm::Conifer && sp.shape.bark != BarkStyle::Pine;
    for (bi, br) in branches.iter().enumerate() {
        // The trunk is drawn by the caller (split at the cut). Far away, twigs vanish and (on
        // trees with many limbs) every other limb too.
        if br.level == 0 || (lod && (br.level >= 2 || (many && br.level == 1 && bi % 2 == 0))) {
            continue;
        }
        let r0 = br.radius[0];
        let sides = match (br.level, lod) {
            (1, false) if r0 > 0.08 => 8,
            (1, false) if r0 > 0.035 => 6,
            (1, false) if r0 > 0.022 => 4,
            (1, false) => 3,
            (1, true) => 3,
            (2, _) if r0 > 0.03 => 5,
            (2, _) => 4,
            _ => 3,
        };
        let path = &br.path;
        // A spruce's limbs live in the shade of their own needle shelves and grey with age;
        // its inner twigs are dead and silvery.
        let (dim, grey) = match (spruce_like, br.level) {
            (true, 1) => (0.62, 0.35),
            (true, _) => (0.7, 0.65),
            _ => (1.0, 0.0),
        };
        let color = |i: usize, _a: f32| {
            let c = bark_color(sp, path[i].y, h, i + bi * 7, seed) * shade.wood_ao(path[i]);
            let luma = c.truncate().dot(Vec3::new(0.3, 0.55, 0.15));
            (c.truncate().lerp(Vec3::splat(luma), grey) * dim).extend(1.0)
        };
        b.tube(path, &br.radius, sides, 0.5, &color, None, None);
    }
}

/// Which level of detail a crown mesh is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// Every card, folded and drooping.
    Near,
    /// The cards marked `far` (about a third), flat and 1.65× larger.
    Far,
    /// A share ([`DISTANT_KEEP`]) of the far cards, flat and [`DISTANT_CARD_SCALE`]× larger.
    Distant,
}

/// Share of the far cards the distant level keeps.
const DISTANT_KEEP: f32 = 0.45;
/// How much bigger the distant level's cards are, so its crown covers about what the full
/// one does with about a seventh of the cards.
const DISTANT_CARD_SCALE: f32 = 2.4;

/// Emit one card: near cards are a 3×3 grid, folded along the twig and drooping toward the
/// tip; far ones a flat quad. Both faces share the bent crown normals.
fn emit_card(
    b: &mut MeshBuilder,
    c: &Card,
    shading: &Shading,
    kind: LeafKind,
    detail: Detail,
    grid: (u32, u32),
) {
    let far = detail != Detail::Near;
    let scale = match detail {
        Detail::Near => 1.0,
        Detail::Far => 1.65,
        Detail::Distant => DISTANT_CARD_SCALE,
    };
    let g = c.dir.normalize_or(Vec3::Y);
    let n = (c.face - g * c.face.dot(g)).normalize_or(g.any_orthonormal_vector());
    let u = g.cross(n);
    let (w, l) = (c.width * scale, c.length * scale);
    let (uv0, uv1) = atlas_cell(kind, c.cell);
    let (cols, rows) = if far { (2u32, 2u32) } else { grid };
    let start = if c.centered {
        c.base - g * l * 0.5
    } else {
        c.base - g * l * 0.06
    };
    // Each face gets the crown's bent normal, pushed into that face's own hemisphere: the
    // side of a spray facing away from the light stays dark (it glows only by translucency),
    // and no face is lit with a normal pointing away from whoever sees it (which would turn
    // it white with grazing reflections).
    let mut grid_pts = Vec::with_capacity((cols * rows) as usize);
    for r in 0..rows {
        let t = r as f32 / (rows - 1) as f32;
        for k in 0..cols {
            let s = k as f32 / (cols - 1) as f32 - 0.5;
            let bend = if far {
                0.0
            } else {
                c.droop * l * t * t + c.fold * w * s.abs()
            };
            let p = start + g * (l * t) + u * (w * s) - n * bend;
            let su = if c.mirror { 0.5 - s } else { s + 0.5 };
            let uv = Vec2::new(
                uv0.x + (uv1.x - uv0.x) * su,
                uv0.y + (uv1.y - uv0.y) * (1.0 - t),
            );
            let (normal, ao) = shading.at(p, c.clump);
            // The twig end of a spray sits deeper in the foliage than its tip.
            let inner = if c.centered { 1.0 } else { 0.84 + 0.2 * t };
            grid_pts.push((p, uv, normal, (c.tint * (ao * inner)).extend(1.0)));
        }
    }
    // Needle brushes are tufts all round their shoot rather than flat leaves, so a card of
    // them shades with its clump as a whole and only the strongest turns away are pushed back.
    let min_dot = if kind.is_needles() { -0.2 } else { 0.25 };
    let hemisphere = |v: Vec3, side: Vec3| {
        let d = v.dot(side);
        if d < min_dot {
            (v + side * (min_dot - d)).normalize_or(side)
        } else {
            v
        }
    };
    for side in [n, -n] {
        let first = b.vertices.len() as u32;
        for &(p, uv, normal, color) in &grid_pts {
            b.vertex(p, hemisphere(normal, side), uv, color);
        }
        let front = side == n;
        for r in 0..rows - 1 {
            for k in 0..cols - 1 {
                let a = first + r * cols + k;
                let (bb, cc, d) = (a + 1, a + cols + 1, a + cols);
                if front {
                    b.tri(a, bb, cc);
                    b.tri(a, cc, d);
                } else {
                    b.tri(a, cc, bb);
                    b.tri(a, d, cc);
                }
            }
        }
    }
}

/// Leaf tint: brightness and a shift toward the accent color.
/// Share of a deciduous crown's leaf cards kept at each step of leaf fall (and, the other way,
/// of leaf-out in spring). The same cards go first at every step, so a crown thins out rather
/// than reshuffling.
pub const LEAF_STEPS: [f32; 3] = [0.6, 0.3, 0.1];

/// The thinning steps a species' crown needs: deciduous ones only.
fn leaf_steps(sp: &PlantSpecies) -> &'static [f32] {
    if sp.seasons.is_some() {
        &LEAF_STEPS
    } else {
        &[]
    }
}

/// A crown's leaf cards as one mesh (`lod`: only the far cards, drawn larger), and thinned
/// copies of it keeping each share in `steps`.
fn crown_meshes(
    cards: &[Card],
    shading: &Shading,
    kind: LeafKind,
    detail: Detail,
    grid: (u32, u32),
    steps: &[f32],
) -> (engine_render::Mesh, Vec<engine_render::Mesh>) {
    let mut full = MeshBuilder::new();
    let mut thin: Vec<MeshBuilder> = steps.iter().map(|_| MeshBuilder::new()).collect();
    let shown = |i: usize, c: &Card| match detail {
        Detail::Near => true,
        Detail::Far => c.far,
        Detail::Distant => {
            c.far && engine_core::math::hash01(i as i64, 0, 0, 0xd157_a4ce) < DISTANT_KEEP
        }
    };
    for (i, c) in cards.iter().enumerate().filter(|(i, c)| shown(*i, c)) {
        emit_card(&mut full, c, shading, kind, detail, grid);
        let keep = engine_core::math::hash01(i as i64, 0, 0, 0x1eaf_fa11);
        for (b, &share) in thin.iter_mut().zip(steps) {
            if keep < share {
                emit_card(b, c, shading, kind, detail, grid);
            }
        }
    }
    (
        full.finish(),
        thin.into_iter().map(MeshBuilder::finish).collect(),
    )
}

fn leaf_tint(sp: &PlantSpecies, rng: &mut Rng) -> Vec3 {
    let leaf = Vec3::from(sp.leaf_color).max(Vec3::splat(0.01));
    let accent = Vec3::from(sp.accent_color);
    let t = rng.range(-0.3, 0.6).max(0.0);
    let ratio = Vec3::ONE.lerp(accent / leaf, t);
    ratio * rng.range(0.88, 1.1)
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    sp: &PlantSpecies,
    seed: u64,
    trunk_path: &[Vec3],
    trunk_r: &[f32],
    crowns: Crowns,
    shading: (f32, f32, f32),
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
    let branches = &crowns.branches;
    // The bark rim round the cut faces, as dark as the bark there (a pale rim would show as a
    // hairline round a birch's black foot where the stump and the wood above it meet).
    let rim = (bark_color(sp, cut, h, cut_i, seed).truncate() * 0.6).extend(1.0);
    // No trunk is a true cylinder: fluted, bulging at old knots (the near level only).
    let trunk_shape = TrunkShape::new(trunk_path, trunk_r, seed);
    let bark_v_shift = engine_core::math::hash01(11, 0, 0, seed);
    // Base: roots + trunk up to the cut, growth rings on top.
    if !sapling {
        let mut b = MeshBuilder::new();
        b.bark_v_shift = bark_v_shift;
        let path = &trunk_path[..=cut_i];
        let rad = &trunk_r[..=cut_i];
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i, seed);
        let cap = |r: f32| cut_color(rim, r);
        b.tube(path, rad, TRUNK_SIDES, 0.5, &color, None, Some(&cap));
        if sp.trunk_radius_m > 0.12 {
            // Root flares: broad, low ridges that leave the flared foot and sink into the
            // ground. Their round sections are mostly buried and stay thick until they are
            // under the ground, so they read as rounded buttresses rather than pointed fins.
            let mut rng = Rng::new(seed ^ 0x2007);
            let n = rng.range_i32(5, 8);
            let r0 = sp.trunk_radius_m;
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU + rng.range(-0.3, 0.3);
                let d = Vec3::new(a.cos(), 0.0, a.sin());
                let side = Vec3::new(-d.z, 0.0, d.x) * r0 * rng.range(-0.35, 0.35);
                let len = rng.range(2.6, 3.6);
                let thick = rng.range(0.85, 1.15);
                let root = [
                    d * r0 * 0.6 + Vec3::Y * cut * 0.5,
                    d * r0 * 1.15 + Vec3::Y * 0.08,
                    d * r0 * 1.7 + side * 0.3 - Vec3::Y * (0.08 * r0),
                    d * r0 * (0.9 + 0.5 * len) + side * 0.7 - Vec3::Y * (0.2 * r0),
                    d * r0 * len + side - Vec3::Y * (0.5 * r0),
                ];
                let radius = [0.5, 0.42, 0.34, 0.26, 0.16].map(|f| f * r0 * thick);
                let color = |i: usize, _a: f32| bark_color(sp, root[i].y, h, i + 40, seed) * 0.85;
                b.tube(&root, &radius, 8, 0.5, &color, None, None);
            }
        }
        let mut base = b.finish();
        // The same flutes and burls as the trunk above (and the log it becomes), so the rings
        // still meet at the cut.
        trunk_shape.roughen(&mut base);
        let wood = CutWood::of(sp);
        (out.base_weathered, out.base_moss) = STUMP_WEATHER[1..]
            .iter()
            .map(|&(_, amount)| weather_stump(&base, amount, wood, seed))
            .unzip();
        out.base = Some(base);

        // A bare log: the top has broken off where the trunk thins and rotted away, leaving a
        // dark, rotten end (about a third of the tree's height remains).
        let mut b = MeshBuilder::new();
        b.bark_v_shift = bark_v_shift;
        let snap = 0.55 + 0.15 * engine_core::math::hash01(7, 0, 0, seed);
        let end = (cut_i..trunk_path.len())
            .find(|&i| trunk_r[i] < sp.trunk_radius_m * snap)
            .unwrap_or(trunk_path.len())
            .max(cut_i + 2);
        let top_y = trunk_path[end - 1].y;
        let path = &trunk_path[cut_i..end];
        let rad = &trunk_r[cut_i..end];
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i + cut_i, seed);
        let cap = |r: f32| cut_color(rim, r);
        let rotten = |r: f32| (Vec3::new(0.018, 0.013, 0.009) * (0.6 + 0.4 * r)).extend(1.0);
        let broken = |_: f32| rgb(sp.bark_color, 0.55);
        b.tube(path, rad, 10, 0.5, &color, Some(&cap), Some(&rotten));
        let last = path.len() - 1;
        let axis = (path[last] - path[last - 1]).normalize_or(Vec3::Y);
        break_top(&mut b, path[last], axis, rad[last], color(last, 0.0), seed);
        // Short broken stubs where the thickest limbs were (the thin ones snap off flush). Limbs
        // forking right at the break went with the top.
        let below = top_y - 2.0 * rad[last];
        let mut limbs: Vec<_> = branches
            .iter()
            .enumerate()
            .filter(|(_, br)| br.level == 1 && br.path[0].y < below)
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
        let mut log = b.finish();
        // Its flutes and burls carry on from the standing trunk's.
        let shape = TrunkShape::new(path, rad, seed);
        shape.roughen(&mut log);
        let mut snag = log.clone();
        moss_foot(&mut snag, seed);
        out.snag = Some(snag);
        // Each step of decay slumps the log a little further as its heart rots soft, and the
        // moss grows on the slumped shape.
        let stages: Vec<_> = LOG_MOSS
            .iter()
            .map(|&(_, amount)| {
                let mut m = log.clone();
                shape.slump(&mut m, amount);
                let shell = moss_shell(&m, path, rad, amount, seed);
                moss_log(&mut m, amount, wood, seed);
                (m, shell)
            })
            .collect();
        let (mut logs, shells): (Vec<_>, Vec<_>) = stages.into_iter().unzip();
        out.log_moss = shells;
        out.log = Some(logs.remove(0));
        out.log_mossy = logs;
        let grow = |host: Host, path: &[Vec3], rad: &[f32], along: (f32, f32)| {
            host_fungus(sp, host).and_then(|f| fungi::on_trunk(path, rad, host, f, along, seed))
        };
        // Snags fruit in reach of a walker's eye: from just above the cut to 4 m or so.
        let length = top_y - cut;
        let low = (0.2 / length).min(0.3);
        out.snag_fungi =
            grow(Host::Snag, path, rad, (low, (4.0 / length).clamp(low, 0.7))).map(|mut f| {
                shape.roughen(&mut f);
                f
            });
        out.log_fungi = grow(Host::Log, path, rad, (0.05, 0.9)).map(|mut f| {
            shape.roughen(&mut f);
            f
        });
        out.stump_fungi = grow(
            Host::Stump,
            &trunk_path[..=cut_i],
            &trunk_r[..=cut_i],
            (0.55, 0.85),
        )
        .map(|mut f| {
            trunk_shape.roughen(&mut f);
            f
        });
    }
    let kind = kind_of(sp.shape.leaf);
    let steps = leaf_steps(sp);
    let shading = Shading {
        crown: crowns.crown,
        clumps: crowns.clumps,
        clump_weight: shading.0,
        up: shading.1,
        deep: shading.2,
    };
    for lod in [false, true] {
        // Wood above the cut (with a cut face on its bottom, seen once it falls).
        let mut b = MeshBuilder::new();
        b.bark_v_shift = bark_v_shift;
        let start = if sapling { 0 } else { cut_i };
        let mut path = trunk_path[start..].to_vec();
        if !sapling {
            // Reach a few centimeters into the stump: the two tubes' rings meet the cut at
            // slightly different angles, and would leave hairline cracks there otherwise.
            let down = (path[0] - path[1]).normalize_or(-Vec3::Y);
            path[0] += down * 0.03;
        }
        let path = &path;
        let rad = &trunk_r[start..];
        let color = |i: usize, _a: f32| bark_color(sp, path[i].y, h, i + start, seed);
        let cap = |r: f32| cut_color(rim, r);
        let tip = |_: f32| rgb(sp.bark_color, 0.8);
        b.tube(
            path,
            rad,
            if lod { 6 } else { TRUNK_SIDES },
            0.5,
            &color,
            if sapling { None } else { Some(&cap) },
            Some(&tip),
        );
        if !lod && !sapling {
            trunk_shape.roughen_vertices(&mut b.vertices);
        }
        add_branches(&mut b, sp, branches, h, lod, seed, &shading);
        let detail = if lod { Detail::Far } else { Detail::Near };
        let (f, thin) = crown_meshes(&crowns.cards, &shading, kind, detail, crowns.grid, steps);
        if lod {
            out.wood_lod = Some(b.finish());
            out.foliage_lod = Some(f);
            out.foliage_lod_thin = thin;
        } else {
            out.wood = Some(b.finish());
            out.foliage = Some(f);
            out.foliage_thin = thin;
        }
    }
    out.wood_distant = Some(distant_wood(sp, trunk_path, trunk_r, branches, h, seed));
    let (f, thin) = crown_meshes(
        &crowns.cards,
        &shading,
        kind,
        Detail::Distant,
        crowns.grid,
        steps,
    );
    out.foliage_distant = Some(f);
    out.foliage_distant_thin = thin;
    out
}

/// The distant level's wood: the trunk from the ground up as a four-sided tube on a few
/// rings, and the thick limbs as three-sided sticks (an old pine's or an oak's bare limbs
/// still show from afar; a spruce's thin ones never do).
fn distant_wood(
    sp: &PlantSpecies,
    path: &[Vec3],
    radius: &[f32],
    branches: &[Branch],
    h: f32,
    seed: u64,
) -> engine_render::Mesh {
    let mut b = MeshBuilder::new();
    let n = path.len();
    let mut pick: Vec<usize> = (0..n).step_by(((n - 1) / 4).max(1)).collect();
    if pick.last() != Some(&(n - 1)) {
        pick.push(n - 1);
    }
    let p: Vec<Vec3> = pick.iter().map(|&i| path[i]).collect();
    let r: Vec<f32> = pick.iter().map(|&i| radius[i]).collect();
    let color = |i: usize, _a: f32| bark_color(sp, p[i].y, h, pick[i], seed);
    // No cap: the trunk's top is hidden in the crown.
    b.tube(&p, &r, 4, 0.5, &color, None, None);
    for (k, br) in branches
        .iter()
        .filter(|br| br.level == 1 && br.radius[0] > 0.045)
        .enumerate()
    {
        let m = br.path.len() / 2;
        let pts = [br.path[0], br.path[m], *br.path.last().unwrap()];
        let rr = [br.radius[0], br.radius[m], *br.radius.last().unwrap()];
        let color = |i: usize, _a: f32| bark_color(sp, pts[i].y, h, k + 50, seed);
        b.tube(&pts, &rr, 3, 0.5, &color, None, None);
    }
    b.finish()
}

/// A curving path of `segs` segments from `start` along `dir`; `steer(f, d)` returns the
/// direction change at fraction `f` along it.
fn grow(
    start: Vec3,
    dir: Vec3,
    length: f32,
    segs: usize,
    mut steer: impl FnMut(f32, Vec3) -> Vec3,
) -> Vec<Vec3> {
    let mut path = Vec::with_capacity(segs + 1);
    path.push(start);
    let mut d = dir;
    for i in 1..=segs {
        let f = i as f32 / segs as f32;
        d = (d + steer(f, d)).normalize_or(d);
        path.push(*path.last().unwrap() + d * length / segs as f32);
    }
    path
}

fn taper(segs: usize, r0: f32, r1: f32) -> Vec<f32> {
    (0..=segs)
        .map(|i| {
            let f = i as f32 / segs as f32;
            (r0 * (1.0 - f) + r1 * f).max(0.004)
        })
        .collect()
}

/// Oak-, maple- or birch-like tree.
fn broadleaf(sp: &PlantSpecies, seed: u64, sapling: bool) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let cut = cut_height(sp);
    // Birches: hanging twigs and sprays.
    let weeping = sh.tip_bend < -0.2;
    let top = h * if sapling { 0.96 } else { 0.84 };
    let (path, radius) = trunk(sp, &mut rng, top, cut, !sapling);
    let crown_base_y = h * if sapling { 0.35 } else { sh.crown_base };
    let apex = *path.last().unwrap();
    let crown = Crown {
        center: Vec3::new(apex.x * 0.6, (crown_base_y + h) * 0.5, apex.z * 0.6),
        radii: Vec3::new(
            sp.crown_radius_m,
            (h - crown_base_y) * 0.5 + 0.3,
            sp.crown_radius_m,
        ),
        cone: false,
    };
    let leaf = sh.leaf_size_m * if sapling { 0.7 } else { 1.0 };
    let density = sh.leaf_density.max(0.1);
    let mut cr = Crowns::new(crown, leaf);
    cr.branches.push(Branch {
        path: path.clone(),
        radius: radius.clone(),
        level: 0,
    });
    let nb = if sapling { 5 } else { sh.branches.max(3) };
    for k in 0..nb {
        let frac = if sapling {
            0.4 + 0.55 * k as f32 / nb as f32
        } else {
            sh.crown_base
                + (0.97 - sh.crown_base) * ((k as f32 + rng.range(0.15, 0.85)) / nb as f32)
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
            * rng.range(0.78, 0.95);
        let wobble = 0.1 * (0.3 + sh.gnarl);
        let limb = grow(attach - dir * r_t * 0.6, dir, length, 7, |f, _| {
            Vec3::Y * sh.tip_bend * 0.16 * f + jitter(&mut rng, wobble)
        });
        let r_start =
            (r_t * 0.62).min(sp.trunk_radius_m * 0.5 * (length / sp.crown_radius_m).sqrt());
        let limb_r = taper(7, r_start, 0.012);
        let tint = leaf_tint(sp, &mut rng);
        if !sapling {
            // Branches fan out from the limb toward the crown's outline; twigs carry the leaves.
            let n2 = ((length * 1.15).round() as i32).clamp(2, 8);
            for j in 0..n2 {
                let s = 0.2 + 0.76 * (j as f32 + rng.range(0.1, 0.8)) / n2 as f32;
                let (p, dp, rp) = point_at(&limb, &limb_r, s);
                let out = crown.outward(p, dp);
                let side = rotate(
                    dp.any_orthonormal_vector(),
                    dp,
                    j as f32 * 2.4 + rng.range(-0.5, 0.5),
                );
                let a2 = rng.range(0.55, 1.0);
                let d2 =
                    (dp * a2.cos() + side * a2.sin() + out * 0.35 + Vec3::Y * 0.12 * sh.tip_bend)
                        .normalize();
                let l2 = crown
                    .reach(p, d2)
                    .unwrap_or(1.0)
                    .min(length * (1.05 - s) + 1.2)
                    .clamp(0.8, sp.crown_radius_m * 0.85)
                    * rng.range(0.7, 0.95);
                let segs = 4;
                let branch = grow(p, d2, l2, segs, |f, _| {
                    let sag = if weeping { -0.16 * f } else { 0.0 };
                    Vec3::Y * (sh.tip_bend * 0.1 + sag) + jitter(&mut rng, wobble * 0.8)
                });
                let branch_r = taper(segs, (rp * 0.62).min(0.07), 0.008);
                let (kc, _, _) = point_at(&branch, &branch_r, 0.68);
                let clump = cr.clump(kc + out * 0.2, l2 * 0.55 + leaf * 0.5);
                let ctint = tint * rng.range(0.92, 1.08);
                cr.anchor_along(&branch, &branch_r, 0.3, clump, ctint);
                let n3 = ((l2 / 1.1 * density).round() as i32).clamp(1, 4);
                for t in 0..n3 {
                    let s3 = 0.18 + 0.8 * (t as f32 + rng.range(0.0, 0.8)) / n3 as f32;
                    let (q, dq, rq) = point_at(&branch, &branch_r, s3);
                    let out_q = crown.outward(q, dq);
                    let side3 = rotate(
                        dq.any_orthonormal_vector(),
                        dq,
                        t as f32 * 2.1 + rng.range(-0.6, 0.6),
                    );
                    let d3 = if weeping {
                        (dq * 0.25 + side3 * 0.35 + out_q * 0.3 - Vec3::Y * 1.1).normalize()
                    } else {
                        (dq * 0.55 + side3 * 0.8 + out_q * 0.35 + Vec3::Y * 0.25 * sh.tip_bend)
                            .normalize()
                    };
                    let l3 = leaf * rng.range(0.45, 0.8);
                    let twig = grow(q, d3, l3, 2, |_, _| jitter(&mut rng, 0.12));
                    let deep = crown.depth(q) < 0.45 && rng.chance(0.6);
                    if !deep {
                        let per = if rng.chance(0.5 * density) { 2 } else { 1 };
                        for c in 0..per {
                            let (cp, cd, _) = point_at(&twig, &[0.0; 3], 0.15 + 0.5 * c as f32);
                            let size = leaf * rng.range(0.85, 1.15);
                            let (dir, face, droop) = if weeping {
                                (
                                    (cd + Vec3::NEG_Y * 0.6).normalize(),
                                    (out_q + Vec3::Y * 0.3).normalize(),
                                    -0.08,
                                )
                            } else {
                                (
                                    (cd + out_q * 0.4).normalize(),
                                    (crown.normal(cp) * 0.6 + Vec3::Y * 0.8).normalize(),
                                    0.14,
                                )
                            };
                            let face = rotate(face, dir, rng.range(-0.6, 0.6));
                            cr.spray(&mut rng, cp, dir, face, size, clump, ctint, droop);
                        }
                    }
                    cr.branches.push(Branch {
                        path: twig,
                        radius: vec![rq * 0.55, rq * 0.35, 0.004],
                        level: 3,
                    });
                }
                // The branch tip: a spray reaching on, and a cluster facing out.
                let (tp, td, _) = point_at(&branch, &branch_r, 0.9);
                let out_t = crown.normal(tp);
                let tdir = if weeping {
                    (td - Vec3::Y * 0.8).normalize()
                } else {
                    (td + out_t * 0.3).normalize()
                };
                let face = (out_t + Vec3::Y * 0.6).normalize();
                let size = leaf * rng.range(0.9, 1.15);
                cr.spray(&mut rng, tp, tdir, face, size, clump, ctint, 0.12);
                if !weeping {
                    let end = *branch.last().unwrap() + out_t * leaf * 0.12;
                    let size = leaf * rng.range(0.7, 0.9);
                    cr.cluster(&mut rng, end, out_t, size, clump, ctint);
                }
                cr.branches.push(Branch {
                    path: branch,
                    radius: branch_r,
                    level: 2,
                });
            }
        }
        // Leaves at the limb's end.
        let lc = cr.clump(*limb.last().unwrap(), leaf * 1.2);
        cr.anchor_along(&limb, &limb_r, 0.55, lc, tint);
        for s in [0.75, 0.9, 1.0] {
            let (cp, cd, _) = point_at(&limb, &limb_r, s);
            let out = crown.normal(cp);
            let dir = if weeping {
                (cd - Vec3::Y * 0.7).normalize()
            } else {
                (cd + out * 0.4).normalize()
            };
            let face = (out + Vec3::Y * 0.6).normalize();
            let size = leaf * rng.range(0.9, 1.2);
            cr.spray(&mut rng, cp, dir, face, size, lc, tint, 0.12);
        }
        cr.branches.push(Branch {
            path: limb,
            radius: limb_r,
            level: 1,
        });
    }
    if !sapling {
        let airy = if weeping { 1.25 } else { 1.0 };
        let per_m2 = 1.5 * airy * density * sp.canopy_density.clamp(0.4, 1.5) / (leaf * leaf);
        cr.fill_shell(&mut rng, per_m2, weeping);
    }
    // Crown top.
    let tc = cr.clump(apex + Vec3::Y * leaf * 0.3, leaf * 1.5);
    let tint = leaf_tint(sp, &mut rng);
    for k in 0..if sapling { 3 } else { 6 } {
        let a = k as f32 * 2.4 + rng.range(0.0, 1.0);
        let d = Vec3::new(a.cos() * 0.7, 1.0, a.sin() * 0.7).normalize();
        let face = Vec3::new(a.cos(), 0.6, a.sin()).normalize();
        let size = leaf * rng.range(0.9, 1.2);
        cr.spray(
            &mut rng,
            apex - Vec3::Y * leaf * 0.4,
            d,
            face,
            size,
            tc,
            tint,
            0.1,
        );
    }
    let shading = if weeping {
        (0.4, 0.25, 0.5)
    } else {
        (0.55, 0.3, 0.42)
    };
    assemble(sp, seed, &path, &radius, cr, shading, h, cut, sapling)
}

/// Pine- or spruce-like tree: straight trunk, whorls of branches, needle sprays.
fn conifer(sp: &PlantSpecies, seed: u64, sapling: bool) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let cut = cut_height(sp);
    // Pines: a rounded crown high on a bare trunk, needles in brushes at the shoot tips. An
    // old pine's leader gives out below the top, which its upper limbs then make.
    let pine = sh.leaf == LeafStyle::LongNeedles || (!sapling && sh.crown_base > 0.3);
    let old_pine = pine && !sapling;
    let top = h * if old_pine { PINE_TRUNK_TOP } else { 0.99 };
    let (path, mut radius) = trunk(sp, &mut rng, top, cut, !sapling);
    if old_pine {
        // Its limbs fork off a trunk that is still stout at the top.
        for (p, r) in path.iter().zip(radius.iter_mut()) {
            *r = r.max(sp.trunk_radius_m * (0.42 - 0.12 * p.y / top));
        }
    }
    let crown_base = if sapling { 0.15 } else { sh.crown_base };
    let y0 = h * crown_base;
    let leaf = sh.leaf_size_m * if sapling { 0.8 } else { 1.0 };
    let spacing = if pine { 0.75 } else { 0.62 } * if sapling { 1.8 } else { 1.0 };
    let whorls = (((h - y0) / spacing).floor() as usize).max(2);
    let crown = if old_pine {
        // A broad dome, flattening as the tree ages.
        Crown {
            center: Vec3::new(0.0, (y0 + h) * 0.5 + 0.3, 0.0),
            radii: Vec3::new(sp.crown_radius_m, (h - y0) * 0.5 + 0.5, sp.crown_radius_m),
            cone: false,
        }
    } else if pine {
        Crown {
            center: Vec3::new(0.0, (y0 + h) * 0.5, 0.0),
            radii: Vec3::new(sp.crown_radius_m, (h - y0) * 0.5 + 0.4, sp.crown_radius_m),
            cone: false,
        }
    } else {
        Crown {
            center: Vec3::new(0.0, (y0 + h) * 0.5, 0.0),
            radii: Vec3::new(sp.crown_radius_m, (h - y0) * 0.5, sp.crown_radius_m),
            cone: true,
        }
    };
    let mut cr = Crowns::new(crown, leaf);
    cr.far_share = if pine { 0.32 } else { 0.38 };
    // Pine sprays fold around their shoot; spruce sprays droop.
    cr.grid = if pine { (3, 2) } else { (2, 3) };
    cr.branches.push(Branch {
        path: path.clone(),
        radius: radius.clone(),
        level: 0,
    });
    // Old pines shed their lower limbs: a few dead stubs on the bare trunk.
    if old_pine {
        for _ in 0..rng.range_i32(5, 9) {
            let y = rng.range(h * 0.18, y0);
            let (p, _, r_t) = point_at(&path, &radius, y / top);
            let az = rng.range(0.0, std::f32::consts::TAU);
            let d = Vec3::new(az.cos(), rng.range(-0.3, 0.2), az.sin()).normalize();
            let l = rng.range(0.15, 0.5);
            cr.branches.push(Branch {
                path: vec![p - d * r_t * 0.5, p + d * l],
                radius: vec![(r_t * 0.18).min(0.04), 0.01],
                level: 1,
            });
        }
    }
    if old_pine {
        pine_scaffold(&mut cr, &mut rng, sp, &path, &radius, y0, h);
    }
    for w in 0..if old_pine { 0 } else { whorls } {
        let y = y0 + (h - y0 - 0.3) * (w as f32 + rng.range(0.0, 0.35)) / whorls as f32;
        let rel = ((y - y0) / (h - y0)).clamp(0.0, 1.0);
        let envelope = if pine {
            (std::f32::consts::PI * (0.12 + 0.88 * rel))
                .sin()
                .max(0.12)
                .powf(0.55)
                * (1.0 - rel * 0.3)
        } else {
            // A narrow cone; the oldest, lowest branches a little shorter.
            ((1.0 - rel).powf(0.95) * 0.96 + 0.04) * (0.88 + 0.12 * smoothstep(0.0, 0.08, rel))
        };
        let (attach, _, r_t) = point_at(&path, &radius, y / (h * 0.99));
        let nb = (sh.branches as i32 + rng.range_i32(-1, 2)).max(3);
        let az0 = rng.range(0.0, std::f32::consts::TAU);
        // Spruces add smaller branches between the whorls.
        let extra = if pine || sapling { 0 } else { nb / 2 };
        for k in 0..nb + extra {
            let inter = k >= nb;
            let az =
                az0 + if inter {
                    (k - nb) as f32 * std::f32::consts::TAU / extra.max(1) as f32 + 0.5
                } else {
                    k as f32 * std::f32::consts::TAU / nb as f32
                } + rng.range(-0.3, 0.3);
            let by = if inter { y + spacing * 0.5 } else { y };
            let (attach, r_t) = if inter {
                let (p, _, r) = point_at(&path, &radius, (by / (h * 0.99)).min(0.99));
                (p, r)
            } else {
                (attach, r_t)
            };
            let angle = (sh.angle_low_deg
                + (sh.angle_high_deg - sh.angle_low_deg) * rel
                + rng.range(-6.0, 6.0))
            .to_radians();
            let dir = Vec3::new(angle.sin() * az.cos(), angle.cos(), angle.sin() * az.sin());
            // Pine crowns are irregular: some limbs stay short.
            let vary = if pine {
                rng.range(0.6, 1.1)
            } else {
                rng.range(0.9, 1.12)
            };
            let length =
                (sp.crown_radius_m * envelope * vary * if inter { 0.7 } else { 1.0 }).max(0.3);
            let segs = 5;
            let gnarl = 0.04 + 0.1 * sh.gnarl;
            let bp = grow(attach - dir * r_t * 0.5, dir, length, segs, |f, _| {
                // Spruce limbs sag in the middle and turn up at the tip; pine limbs curve up.
                let sag = if pine { 0.0 } else { -0.1 * (1.0 - f) };
                Vec3::Y * (sh.tip_bend * 0.3 * f + sag) + jitter(&mut rng, gnarl)
            });
            // Conifer limbs stay slender (a few centimeters), even low on an old trunk.
            let r_start =
                (r_t * 0.3).min(0.022 + sp.trunk_radius_m * 0.05) * (0.5 + 0.5 * envelope);
            let br = taper(segs, r_start, 0.008);
            let tint = leaf_tint(sp, &mut rng);
            if pine {
                pine_limb(&mut cr, &mut rng, &bp, &br, length, tint, sapling);
            } else {
                spruce_limb(&mut cr, &mut rng, &bp, &br, length, tint, rel);
                if !sapling && rel < 0.65 {
                    dead_twigs(&mut cr, &mut rng, &bp, &br, length);
                }
            }
            cr.branches.push(Branch {
                path: bp,
                radius: br,
                level: 1,
            });
        }
    }
    // The leader at the top (an old pine's ends among its upper limbs).
    let tip = *path.last().unwrap();
    let crest = if old_pine { tip.y + leaf * 0.5 } else { h };
    let tc = cr.clump(Vec3::new(tip.x, crest - leaf * 0.6, tip.z), leaf);
    let tint = leaf_tint(sp, &mut rng);
    for k in 0..4 {
        let a = k as f32 * 1.6 + rng.range(0.0, 0.5);
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let size = leaf * if pine { 0.8 } else { 0.7 };
        let base = Vec3::new(tip.x, crest - size * 1.05, tip.z);
        let dir = (Vec3::Y * 2.5 + out).normalize();
        cr.spray(&mut rng, base, dir, out, size, tc, tint, -0.05);
    }
    // A spruce's top is a narrow spire clad down the leader: short shoots all round it, so
    // the trunk never shows between its small top whorls.
    if !pine {
        let n = 12;
        for k in 0..n {
            let f = (k as f32 + rng.range(0.0, 0.8)) / n as f32;
            let y = h - 0.5 - 2.8 * f;
            let (p, _, _) = point_at(&path, &radius, (y / (h * 0.99)).clamp(0.0, 0.995));
            let a = k as f32 * 2.399_963 + rng.range(-0.3, 0.3);
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            let size = leaf * (0.45 + 0.35 * f);
            let dir = (Vec3::Y * 1.2 + out * (0.7 + 0.6 * f)).normalize();
            cr.spray(&mut rng, p, dir, out, size, tc, tint, 0.05);
        }
    }
    let shading = if pine {
        (0.6, 0.3, 0.3)
    } else {
        (0.5, 0.2, 0.3)
    };
    assemble(sp, seed, &path, &radius, cr, shading, h, cut, sapling)
}

/// Where an old pine's trunk ends, as a share of its height: its upper limbs make the top.
const PINE_TRUNK_TOP: f32 = 0.84;

/// An old pine's crown: a few heavy limbs climbing out of the top of a long bare bole, each
/// rising steeply and then leveling out under a pad of needles, with side branches spreading
/// the pad sideways. Together they make the broad, flat-topped, uneven dome of a Scots pine
/// rather than a narrow cone of whorls.
fn pine_scaffold(
    cr: &mut Crowns,
    rng: &mut Rng,
    sp: &PlantSpecies,
    path: &[Vec3],
    radius: &[f32],
    y0: f32,
    h: f32,
) {
    let sh = sp.shape;
    let top = h * PINE_TRUNK_TOP;
    let crown_r = sp.crown_radius_m;
    let n = (sh.branches as i32 + rng.range_i32(2, 6)).max(4);
    let az0 = rng.range(0.0, std::f32::consts::TAU);
    let gnarl = 0.05 + 0.1 * sh.gnarl;
    for k in 0..n {
        // Lowest first; the top ones fork off the end of the trunk.
        let rel = (k as f32 + rng.range(0.1, 0.9)) / n as f32;
        let y = y0 + (top - y0) * rel.powf(0.8);
        let (attach, _, r_t) = point_at(path, radius, (y / top).min(0.995));
        let az = az0 + k as f32 * 2.399_963 + rng.range(-0.4, 0.4);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        // How far out it reaches: widest a third of the way up, the top ones shorter.
        let reach = crown_r
            * (0.62 + 0.4 * (1.0 - ((rel - 0.35) / 0.65).abs().powi(2)))
            * rng.range(0.75, 1.05);
        // It climbs, then levels out a little under the crown's flat top.
        let end_y = (y + 1.2 + 2.6 * (1.0 - rel)).min(h - 0.6 - 0.8 * rng.range(0.0, 1.0));
        let end = attach + out * reach + Vec3::Y * (end_y - y);
        let rise = Vec3::Y * (end_y - y).max(0.5) * 0.85 + out * reach * 0.2;
        let ctrl = attach + rise;
        let segs = 8;
        let mut limb: Vec<Vec3> = (0..=segs)
            .map(|i| {
                let t = i as f32 / segs as f32;
                let a = attach.lerp(ctrl, t);
                let b = ctrl.lerp(end, t);
                a.lerp(b, t)
            })
            .collect();
        // Crooked, as pine limbs are.
        for p in limb.iter_mut().skip(1) {
            *p += jitter(rng, gnarl * 1.6);
        }
        limb[0] = attach - out * r_t * 0.4;
        let length: f32 = limb.windows(2).map(|w| w[0].distance(w[1])).sum();
        let r_start =
            (r_t * 0.8).min(0.035 + sp.trunk_radius_m * 0.22) * (0.65 + 0.35 * reach / crown_r);
        let limb_r = taper(segs, r_start, 0.02);
        let tint = leaf_tint(sp, rng);
        // Side branches spread the pad sideways from the limb's outer half.
        let sides = rng.range_i32(2, 5);
        for j in 0..sides {
            let s = 0.45 + 0.45 * (j as f32 + rng.range(0.0, 0.8)) / sides as f32;
            let (p, dp, rp) = point_at(&limb, &limb_r, s);
            let flat = Vec3::new(dp.x, 0.0, dp.z).normalize_or(out);
            let side = Vec3::Y.cross(flat) * if j % 2 == 0 { 1.0 } else { -1.0 };
            let d2 = (flat * 0.6 + side * rng.range(0.7, 1.1) + Vec3::Y * rng.range(0.05, 0.3))
                .normalize();
            let l2 = rng.range(0.9, 1.8) * (0.6 + 0.4 * reach / crown_r);
            let segs2 = 4;
            let bp = grow(p, d2, l2, segs2, |f, _| {
                Vec3::Y * sh.tip_bend * 0.25 * f + jitter(rng, gnarl)
            });
            let br = taper(segs2, (rp * 0.55).min(0.045), 0.01);
            let clump = pine_limb(cr, rng, &bp, &br, l2, tint, false);
            cr.anchor_along(&bp, &br, 0.3, clump, tint);
            cr.branches.push(Branch {
                path: bp,
                radius: br,
                level: 2,
            });
        }
        // The limb's own end carries the densest pad.
        let s_end = (1.0 - 1.8 / length.max(1.9)).max(0.5);
        let (p_end, _, _) = point_at(&limb, &limb_r, s_end);
        let tail: Vec<Vec3> = std::iter::once(p_end)
            .chain(
                limb.iter()
                    .copied()
                    .filter(|q| q.distance(attach) > p_end.distance(attach)),
            )
            .collect();
        if tail.len() >= 2 {
            let tail_r = taper(tail.len() - 1, limb_r[segs] * 1.8, 0.015);
            let clump = pine_limb(cr, rng, &tail, &tail_r, length * (1.0 - s_end), tint, false);
            cr.anchor_along(&tail, &tail_r, 0.0, clump, tint);
        }
        cr.branches.push(Branch {
            path: limb,
            radius: limb_r,
            level: 1,
        });
    }
    // Shoots fill out each limb's pad into the lumpy dome, leaving sky between the pads.
    let per_m2 = 1.5 * sh.leaf_density * sp.canopy_density.clamp(0.4, 1.5) / (cr.leaf * cr.leaf);
    cr.fill_shell(rng, per_m2, false);
}

/// Needles of a pine limb: side shoots ending in brushes, gathered into a cloud-like clump.
fn pine_limb(
    cr: &mut Crowns,
    rng: &mut Rng,
    bp: &[Vec3],
    br: &[f32],
    length: f32,
    tint: Vec3,
    sapling: bool,
) -> usize {
    let leaf = cr.leaf;
    let (kc, _, _) = point_at(bp, br, 0.72);
    let clump = cr.clump(kc + Vec3::Y * 0.15, length * 0.45 + leaf * 0.6);
    // A pad of needles round the shoot's end: smaller sprays over a flattened dome, growing
    // out from its middle and facing the sky, overlapping into one tufted mass with a few
    // tufts seen end-on on top. Its outline is ragged with brushes, not a few big cards.
    let pad = |cr: &mut Crowns, rng: &mut Rng, p: Vec3, d: Vec3, scale: f32| {
        let r = leaf * scale * 0.42;
        let n = if sapling { 4 } else { 11 };
        let a0 = rng.range(0.0, std::f32::consts::TAU);
        for i in 0..n {
            let a = a0 + i as f32 * 2.399_963 + rng.range(-0.3, 0.3);
            let up = rng.range(-0.3, 0.9);
            let ring = (1.0 - up * up).max(0.0).sqrt();
            let v = Vec3::new(ring * a.cos(), up * 0.55, ring * a.sin());
            let at = p + d * leaf * 0.1 + v * r;
            let out = (v + Vec3::Y * 0.35).normalize_or(Vec3::Y);
            let dir = (out + d * 0.6 + jitter(rng, 0.2)).normalize();
            let roll = rng.range(-0.6, 0.6);
            let face = rotate((Vec3::Y - dir * dir.y).normalize_or(out), dir, roll);
            let size = leaf * scale * rng.range(0.6, 0.78);
            cr.spray(
                rng,
                at - dir * size * 0.5,
                dir,
                face,
                size,
                clump,
                tint,
                0.06,
            );
        }
        for _ in 0..if sapling { 1 } else { 2 } {
            let at = p + d * leaf * 0.12 + Vec3::Y * r * 0.3 + jitter(rng, r * 0.4);
            let face = (d * 0.6 + Vec3::Y * 0.6 + jitter(rng, 0.3)).normalize();
            cr.cluster(rng, at, face, leaf * scale * 0.5, clump, tint);
        }
    };
    let n2 = if sapling {
        1
    } else {
        ((length / 0.7).round() as i32).clamp(1, 5)
    };
    for j in 0..n2 {
        let s = 0.35 + 0.55 * (j as f32 + rng.range(0.0, 0.7)) / n2 as f32;
        let (p, dp, rp) = point_at(bp, br, s);
        let side = Vec3::Y.cross(dp).normalize_or(Vec3::X) * if j % 2 == 0 { 1.0 } else { -1.0 };
        let d2 = (dp + side * rng.range(0.6, 1.1) + Vec3::Y * 0.25).normalize();
        let l2 = (length * (1.0 - s) * 0.6 + 0.25).min(1.4) * rng.range(0.8, 1.1);
        let shoot = grow(p, d2, l2, 3, |_, _| Vec3::Y * 0.12 + jitter(rng, 0.08));
        let (tp, td, _) = point_at(&shoot, &[0.0; 4], 1.0);
        pad(cr, rng, tp, td, 0.85);
        cr.branches.push(Branch {
            path: shoot,
            radius: vec![rp * 0.5, rp * 0.35, rp * 0.22, 0.006],
            level: 2,
        });
    }
    let (tp, td, _) = point_at(bp, br, 1.0);
    pad(cr, rng, tp, td, 1.0);
    clump
}

/// Dead twigs on the inner part of a spruce limb, where the shade has killed the needles:
/// short, thin and pointing out and down.
fn dead_twigs(cr: &mut Crowns, rng: &mut Rng, bp: &[Vec3], br: &[f32], length: f32) {
    for _ in 0..rng.range_i32(1, 4) {
        let s = rng.range(0.1, 0.45);
        let (p, d, rp) = point_at(bp, br, s);
        let side =
            Vec3::Y.cross(d).normalize_or(Vec3::X) * if rng.chance(0.5) { 1.0 } else { -1.0 };
        let dir =
            (d * 0.7 + side * rng.range(0.4, 1.0) - Vec3::Y * rng.range(0.1, 0.7)).normalize();
        let l = rng.range(0.12, 0.3) * (0.6 + 0.4 * (length / 3.0).min(1.0));
        let kink = jitter(rng, 0.03);
        cr.branches.push(Branch {
            path: vec![p, p + dir * l * 0.5 + kink, p + dir * l + kink * 1.5],
            radius: vec![(rp * 0.4).min(0.007), 0.0035, 0.0015],
            level: 2,
        });
    }
}

/// Needles of a spruce limb: flat sprays fanning sideways along it and curtains of shoots
/// hanging below.
fn spruce_limb(
    cr: &mut Crowns,
    rng: &mut Rng,
    bp: &[Vec3],
    br: &[f32],
    length: f32,
    tint: Vec3,
    rel: f32,
) {
    let leaf = cr.leaf;
    let crown = cr.crown;
    let (kc, _, _) = point_at(bp, br, 0.6);
    let clump = cr.clump(kc + Vec3::Y * 0.05, length * 0.5 + leaf * 0.5);
    cr.clumps[clump].flat = 0.45;
    let size = leaf * (0.8 + 0.4 * (length / (crown.radii.x * 0.8)).min(1.0));
    let step = (size * 0.42 / length).max(0.09);
    let mut s = rng.range(0.08, 0.2);
    let mut i = 0;
    while s < 0.97 {
        let (p, d, _) = point_at(bp, br, s);
        let flat = Vec3::Y.cross(d).normalize_or(Vec3::X);
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        // Shelf: a spray fanning out sideways, tilted down a little, facing the sky.
        let a = rng.range(0.45, 1.0) * side;
        let dir = (rotate(d, Vec3::Y, a) - Vec3::Y * rng.range(0.15, 0.45)).normalize();
        // Rolled toward the outside so the sprays show from the side too; higher up, where
        // the limbs are short, they face out more, so the trunk doesn't show between whorls.
        let out = crown.outward(p, d);
        let facing = Vec3::Y * (1.0 - 0.5 * rel) + out * (0.5 + 0.9 * rel);
        let face = rotate(facing.normalize(), dir, rng.range(-1.0, 1.0));
        let sz = size * rng.range(0.9, 1.15) * (1.0 - 0.2 * s);
        cr.spray(rng, p, dir, face, sz, clump, tint, 0.22);
        // Curtain: shoots hanging from the limb, seen from outside the crown. Few enough
        // that the limbs still read as tiers.
        if rng.chance(0.35 + 0.25 * (1.0 - rel)) && s > 0.25 {
            let dir = (Vec3::NEG_Y * 1.2 + d * 0.5 + flat * side * 0.3).normalize();
            let sz = size * rng.range(0.5, 0.7);
            cr.spray(rng, p, dir, out, sz, clump, tint * 0.9, -0.1);
        }
        s += step * rng.range(0.8, 1.2);
        i += 1;
    }
    // The tip reaches on, and ends in a tuft seen from outside.
    let (tp, td, _) = point_at(bp, br, 0.9);
    let face = rotate(Vec3::Y, td, rng.range(-0.3, 0.3));
    cr.spray(
        rng,
        tp,
        (td + Vec3::Y * 0.15).normalize(),
        face,
        size * 0.8,
        clump,
        tint,
        0.05,
    );
    let end = *bp.last().unwrap();
    let out = (crown.outward(end, td) + Vec3::Y * 0.2).normalize();
    cr.cluster(rng, end, out, size * 0.6, clump, tint);
}

/// Multi-stemmed bush.
fn shrub(sp: &PlantSpecies, seed: u64) -> PlantMeshes {
    let mut rng = Rng::new(seed);
    let sh = sp.shape;
    let h = sp.max_height_m;
    let crown = Crown {
        center: Vec3::new(0.0, h * 0.55, 0.0),
        radii: Vec3::new(sp.crown_radius_m, h * 0.5, sp.crown_radius_m),
        cone: false,
    };
    let leaf = sh.leaf_size_m;
    let density = sh.leaf_density.max(0.1);
    let mut cr = Crowns::new(crown, leaf);
    cr.far_share = 0.35;
    let n = sh.branches.max(3) as i32 + rng.range_i32(-1, 2);
    for k in 0..n {
        let az = k as f32 * 2.399_963 + rng.range(-0.3, 0.3);
        let angle = rng.range(sh.angle_high_deg, sh.angle_low_deg).to_radians();
        let dir = Vec3::new(angle.sin() * az.cos(), angle.cos(), angle.sin() * az.sin());
        let length = h * rng.range(0.78, 1.0) / angle.cos().max(0.5);
        let segs = 6;
        let start = Vec3::new(az.cos(), 0.0, az.sin()) * rng.range(0.03, 0.15) - Vec3::Y * 0.05;
        let outward = Vec3::new(az.cos(), 0.0, az.sin());
        // Stems arch outward under their leaves.
        let stem = grow(start, dir, length, segs, |f, _| {
            outward * 0.07 * f + Vec3::Y * (sh.tip_bend * 0.1 - 0.03 * f) + jitter(&mut rng, 0.1)
        });
        let stem_r = taper(segs, sp.trunk_radius_m, sp.trunk_radius_m * 0.2);
        let tint = leaf_tint(sp, &mut rng);
        let n2 = rng.range_i32(3, 5);
        for j in 0..n2 {
            let s = 0.35 + 0.55 * (j as f32 + rng.range(0.0, 0.7)) / n2 as f32;
            let (p, dp, rp) = point_at(&stem, &stem_r, s);
            let out = crown.outward(p, outward);
            let side = rotate(
                dp.any_orthonormal_vector(),
                dp,
                j as f32 * 2.2 + rng.range(0.0, 1.0),
            );
            let d2 = (dp * 0.5 + side * 0.8 + out * 0.5).normalize();
            let l2 = length * rng.range(0.3, 0.45) * (1.1 - 0.4 * s);
            let shoot = grow(p, d2, l2, 3, |_, _| Vec3::Y * 0.06 + jitter(&mut rng, 0.1));
            let (kc, _, _) = point_at(&shoot, &[0.0; 4], 0.6);
            let clump = cr.clump(kc + out * 0.1, l2 * 0.6 + leaf * 0.45);
            cr.anchor_along(
                &shoot,
                &[rp * 0.6, rp * 0.4, rp * 0.25, 0.004],
                0.3,
                clump,
                tint,
            );
            let per = ((l2 / (leaf * 0.4) * density).round() as i32).clamp(2, 6);
            for c in 0..per {
                let s2 = 0.2 + 0.8 * (c as f32 + rng.range(0.0, 0.6)) / per as f32;
                let (cp, cd, _) = point_at(&shoot, &[0.0; 4], s2);
                let o = crown.outward(cp, out);
                let side = Vec3::Y.cross(cd).normalize_or(o) * if c % 2 == 0 { 1.0 } else { -1.0 };
                let dir = (cd + side * 0.5 + o * 0.3).normalize();
                let face = rotate(
                    (crown.normal(cp) + Vec3::Y).normalize(),
                    dir,
                    rng.range(-0.5, 0.5),
                );
                let size = leaf * rng.range(0.85, 1.15);
                cr.spray(&mut rng, cp, dir, face, size, clump, tint, 0.15);
            }
            let end = *shoot.last().unwrap();
            let o = crown.normal(end);
            cr.cluster(&mut rng, end + o * leaf * 0.1, o, leaf * 0.75, clump, tint);
            cr.branches.push(Branch {
                path: shoot,
                radius: vec![rp * 0.6, rp * 0.4, rp * 0.25, 0.004],
                level: 2,
            });
        }
        // Leaves along the upper stem and at its tip.
        let (kc, _, _) = point_at(&stem, &stem_r, 0.8);
        let sc = cr.clump(kc, length * 0.3 + leaf * 0.5);
        let per = ((4.0 * density).round() as i32).max(2);
        for c in 0..per {
            let s = 0.45 + 0.55 * (c as f32 + 1.0) / per as f32;
            let (cp, cd, _) = point_at(&stem, &stem_r, s);
            let o = crown.normal(cp);
            let dir = (cd + o * 0.5).normalize();
            let face = rotate((o + Vec3::Y * 0.7).normalize(), dir, rng.range(-0.5, 0.5));
            let size = leaf * rng.range(0.85, 1.2);
            cr.spray(&mut rng, cp, dir, face, size, sc, tint, 0.12);
        }
        cr.anchor_along(&stem, &stem_r, 0.5, sc, tint);
        cr.branches.push(Branch {
            path: stem,
            radius: stem_r,
            level: 1,
        });
    }
    let per_m2 = 2.2 * density * sp.canopy_density.clamp(0.4, 1.5) / (leaf * leaf);
    cr.fill_shell(&mut rng, per_m2, false);
    let top = h * 1.05;
    let mut out = PlantMeshes {
        height: top,
        cut_height: 0.0,
        ..Default::default()
    };
    let kind = kind_of(sh.leaf);
    let shading = Shading {
        crown,
        clumps: std::mem::take(&mut cr.clumps),
        clump_weight: 0.5,
        up: 0.3,
        deep: 0.38,
    };
    let steps = leaf_steps(sp);
    for lod in [false, true] {
        let mut b = MeshBuilder::new();
        add_branches(&mut b, sp, &cr.branches, h, lod, seed, &shading);
        let detail = if lod { Detail::Far } else { Detail::Near };
        let (f, thin) = crown_meshes(&cr.cards, &shading, kind, detail, cr.grid, steps);
        if lod {
            out.wood_lod = Some(b.finish());
            out.foliage_lod = Some(f);
            out.foliage_lod_thin = thin;
        } else {
            out.wood = Some(b.finish());
            out.foliage = Some(f);
            out.foliage_thin = thin;
        }
    }
    // Far off a shrub is its leaves alone.
    let (f, thin) = crown_meshes(&cr.cards, &shading, kind, Detail::Distant, cr.grid, steps);
    out.foliage_distant = Some(f);
    out.foliage_distant_thin = thin;
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
                let near = wood.triangle_count() + foliage.triangle_count();
                assert!(lod * 2 < near, "{name} LOD");
                // The distant level: a few hundred triangles for whole wooded hillsides.
                let distant = m.wood_distant.as_ref().unwrap().triangle_count()
                    + m.foliage_distant.as_ref().unwrap().triangle_count();
                assert!(
                    distant * 2 < lod && distant < 1_500,
                    "{name}: {distant} far off"
                );
                // A near tree stays affordable for a forest of instanced trees.
                assert!(near < 40_000, "{name}: {near} triangles");
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

    #[test]
    fn old_pines_carry_a_broad_crown_on_a_bare_bole() {
        let lib = PlantLibrary::default();
        let sp = lib.get(lib.id("pine").unwrap());
        for seed in 0..3 {
            let m = woody(sp, seed, false);
            let wood = m.wood.as_ref().unwrap();
            let h = wood.aabb().max.y;
            // Only drawn needles (the shared bounds add two loose vertices on the axis).
            let foliage = m.foliage.as_ref().unwrap();
            let leaves: Vec<_> = foliage
                .indices
                .iter()
                .map(|&i| foliage.vertices[i as usize])
                .collect();
            let ys = || leaves.iter().map(|v| v.position[1]);
            let (low, high) = (ys().fold(f32::MAX, f32::min), ys().fold(f32::MIN, f32::max));
            // Needles start well up the bole, not down by the ground like a spruce's.
            assert!(low > 0.55 * h, "seed {seed}: needles from {low} of {h}");
            // The crown is a broad, flat-topped umbrella: wider than it is tall.
            let reach = |axis: usize| {
                let xs = || leaves.iter().map(|v| v.position[axis]);
                xs().fold(f32::MIN, f32::max) - xs().fold(f32::MAX, f32::min)
            };
            let width = reach(0).min(reach(2));
            assert!(
                width > high - low,
                "seed {seed}: {width} wide, {} tall",
                high - low
            );
            // Dark plated bark at the foot, the warm orange only up in the crown.
            let bark = |from: f32, to: f32| {
                let (sum, n) = wood
                    .vertices
                    .iter()
                    .filter(|v| on_cut(v).is_none() && (from..to).contains(&v.position[1]))
                    .fold((0.0, 0), |(s, n), v| (s + v.color[0] - v.color[2], n + 1));
                sum / n.max(1) as f32
            };
            let (foot, top) = (bark(0.5, 0.2 * h), bark(0.75 * h, h));
            assert!(top > foot + 0.05, "seed {seed}: foot {foot}, top {top}");
        }
    }

    #[test]
    fn stumps_silver_then_rot_and_sink() {
        let lib = PlantLibrary::default();
        for (name, heartwood) in [("oak", true), ("birch", false), ("spruce", false)] {
            let sp = lib.get(lib.id(name).unwrap());
            let m = woody(sp, 1, false);
            assert_eq!(m.base_weathered.len(), STUMP_WEATHER.len() - 1, "{name}");
            // Moss cushions grow on the old stump, over the top's rim and up from the ground.
            let cushions = m
                .base_moss
                .last()
                .unwrap()
                .as_ref()
                .expect("old stump moss");
            let tops = cushions.vertices.iter().filter(|v| v.normal[1] > 0.7);
            assert!(tops.count() > 10, "{name}");
            let stages: Vec<_> = std::iter::once(m.base.as_ref().unwrap())
                .chain(&m.base_weathered)
                .collect();
            let wood = CutWood::of(sp);
            let fresh = Vec3::from(CUT_VERTEX_WOOD) * 0.93;
            // The cut face as it shows (vertex color relative to the painted wood).
            let face = |mesh: &engine_render::Mesh| {
                let shown: Vec<(f32, Vec3, f32)> = mesh
                    .vertices
                    .iter()
                    .filter_map(|v| {
                        let (r, _) = on_cut(v)?;
                        let c = Vec3::new(v.color[0], v.color[1], v.color[2]);
                        (r < 0.9).then_some((r, c * wood.mean / fresh, v.position[1]))
                    })
                    .collect();
                let mean = shown.iter().map(|s| s.1).sum::<Vec3>() / shown.len() as f32;
                let y = |near: f32| {
                    let ys: Vec<f32> = shown
                        .iter()
                        .filter(|s| (s.0 - near).abs() < 0.1)
                        .map(|s| s.2)
                        .collect();
                    ys.iter().sum::<f32>() / ys.len() as f32
                };
                (mean, y(0.0), y(0.62))
            };
            let [(new, ..), (grey, ..), (old, heart_y, sap_y)] = [0, 1, 2].map(|i| face(stages[i]));
            let warmth = |c: Vec3| (c.x - c.z) / c.x.max(1e-3);
            // Fresh wood is warm; it silvers (no longer warm, a little darker), then rots dark.
            assert!(warmth(grey) < 0.6 * warmth(new), "{name}: {new} -> {grey}");
            assert!(
                old.length() < 0.7 * grey.length(),
                "{name}: {grey} -> {old}"
            );
            // Birch and spruce rot from the heart into a hollow; oak's heartwood stands proud
            // while the sapwood round it slumps.
            let cut = m.cut_height;
            if heartwood {
                assert!((heart_y - cut).abs() < 0.005, "{name}: heart at {heart_y}");
                assert!(sap_y < cut - 0.005, "{name}: sapwood at {sap_y}");
            } else {
                assert!(heart_y < cut - 0.04, "{name}: heart at {heart_y}");
            }
            // Moss climbs the bark from the ground, and the bark greens under it.
            let foot = cushions.vertices.iter().filter(|v| v.position[1] < 0.15);
            assert!(foot.count() > 10, "{name}");
            let foot_green = |mesh: &engine_render::Mesh| {
                let (sum, n) = mesh
                    .vertices
                    .iter()
                    .filter(|v| on_cut(v).is_none() && (0.0..0.2).contains(&v.position[1]))
                    .fold((0.0, 0), |(s, n), v| {
                        (s + v.color[1] / (v.color[0] + v.color[2]).max(1e-3), n + 1)
                    });
                sum / n.max(1) as f32
            };
            assert!(foot_green(stages[2]) > foot_green(stages[0]), "{name}");
        }
    }

    #[test]
    fn pale_birch_logs_dirty_as_they_lie() {
        let lib = PlantLibrary::default();
        let sp = lib.get(lib.id("birch").unwrap());
        let m = woody(sp, 1, false);
        let snag = m.snag.as_ref().unwrap();
        let log = m.log.as_ref().unwrap();
        // The bark well above the dark foot, standing and lying.
        let bark = |mesh: &engine_render::Mesh, under: bool| {
            let (sum, n) = mesh
                .vertices
                .iter()
                .filter(|v| on_cut(v).is_none() && v.position[1] > 2.0)
                .filter(|v| !under || v.normal[0] < -0.6)
                .fold((0.0, 0), |(s, n), v| (s + v.color[1], n + 1));
            sum / n.max(1) as f32
        };
        assert!(bark(log, false) < 0.8 * bark(snag, false));
        // Soil stains the side it lies on.
        assert!(bark(log, true) < 0.8 * bark(log, false));
    }

    #[test]
    fn logs_moss_over_as_they_rot() {
        let lib = PlantLibrary::default();
        let sp = lib.get(lib.id("oak").unwrap());
        let m = woody(sp, 3, false);
        assert_eq!(m.log_mossy.len(), LOG_MOSS.len() - 1);
        assert_eq!(m.log_moss.len(), LOG_MOSS.len());
        let shells: Vec<_> = m.log_moss.iter().map(|s| s.as_ref().unwrap()).collect();
        // More cushions, and more solid ones, at each step.
        let solid = |s: &engine_render::Mesh| {
            s.vertices.iter().map(|v| v.color[3]).sum::<f32>() / s.vertices.len() as f32
        };
        for w in shells.windows(2) {
            assert!(w[1].vertices.len() > w[0].vertices.len());
            assert!(solid(w[1]) > solid(w[0]));
        }
        // The cushions sit on the log's top (the model's +X), outside its bark.
        let mean_x = |s: &engine_render::Mesh| {
            s.vertices.iter().map(|v| v.position[0]).sum::<f32>() / s.vertices.len() as f32
        };
        let oldest = &m.log_mossy[LOG_MOSS.len() - 2];
        assert!(mean_x(shells[2]) > mean_x(oldest) + 0.05);
        // An old log has slumped: its top lies lower than a fresh one's.
        let top = |s: &engine_render::Mesh| {
            s.vertices
                .iter()
                .map(|v| v.position[0])
                .fold(f32::MIN, f32::max)
        };
        assert!(top(oldest) < top(m.log.as_ref().unwrap()) - 0.02);
    }

    #[test]
    fn deciduous_crowns_come_in_thinner_copies_for_leaf_fall() {
        let lib = PlantLibrary::default();
        for name in ["oak", "birch", "hazel"] {
            let m = woody(lib.get(lib.id(name).unwrap()), 2, false);
            let full = m.foliage.as_ref().unwrap().triangle_count() as f32;
            assert_eq!(m.foliage_thin.len(), LEAF_STEPS.len(), "{name}");
            for (thin, share) in m.foliage_thin.iter().zip(LEAF_STEPS) {
                let kept = thin.triangle_count() as f32 / full;
                assert!(
                    (kept - share).abs() < 0.12,
                    "{name}: kept {kept} of {share}"
                );
            }
            // They sway with the full crown, so they share its bounds.
            let b = m.foliage.as_ref().unwrap().aabb();
            for thin in &m.foliage_thin {
                assert_eq!((thin.aabb().min.y, thin.aabb().max.y), (b.min.y, b.max.y));
            }
        }
        let pine = woody(lib.get(lib.id("pine").unwrap()), 2, false);
        assert!(pine.foliage_thin.is_empty() && pine.foliage_lod_thin.is_empty());
    }

    #[test]
    fn crowns_shade_as_volumes() {
        let lib = PlantLibrary::default();
        let oak = lib.get(lib.id("oak").unwrap());
        let m = woody(oak, 2, false);
        let foliage = m.foliage.unwrap();
        let leaves: Vec<_> = foliage
            .vertices
            .iter()
            .filter(|v| v.uv != [0.0, 0.0] || v.color != [1.0; 4])
            .collect();
        let center =
            leaves.iter().map(|v| Vec3::from(v.position)).sum::<Vec3>() / leaves.len() as f32;
        // Normals point away from the crown's middle, and the inside is darker.
        let outward = leaves
            .iter()
            .map(|v| {
                let to = (Vec3::from(v.position) - center).normalize_or(Vec3::Y);
                to.dot(Vec3::from(v.normal))
            })
            .sum::<f32>()
            / leaves.len() as f32;
        assert!(outward > 0.25, "mean outwardness {outward}");
        let dist = |v: &engine_render::Vertex| Vec3::from(v.position).distance(center);
        let far = leaves.iter().map(|v| dist(v)).fold(0.0f32, f32::max);
        let (mut inner, mut outer) = ((0.0f32, 0usize), (0.0f32, 0usize));
        for v in &leaves {
            let acc = if dist(v) < far * 0.45 {
                &mut inner
            } else {
                &mut outer
            };
            acc.0 += v.color[1];
            acc.1 += 1;
        }
        let (inner, outer) = (
            inner.0 / inner.1.max(1) as f32,
            outer.0 / outer.1.max(1) as f32,
        );
        assert!(
            inner > 0.0 && inner < outer * 0.9,
            "inside {inner}, outside {outer}"
        );
    }
}
