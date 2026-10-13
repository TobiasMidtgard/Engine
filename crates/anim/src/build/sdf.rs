//! Procedural skinned meshes from smooth signed-distance shapes.
//!
//! A [`Body`] is a set of primitives (tapered capsules, ellipsoids, rounded boxes), each bound
//! to a bone, blended with a smooth union so joints and muscles flow into each other. It is
//! meshed with surface nets, projected onto the exact surface, and skinned automatically:
//! every vertex weighs the bones of the parts it is close to, so elbows, knees and the
//! shoulders blend smoothly without hand-painted weights. Muscles become morph targets
//! (a bulge along the surface normal and a sideways sway for jiggle), with matching normal
//! deltas so they show in the lighting and not only the silhouette.
//!
//! Used for the demo characters; real games import skinned glTF characters instead.

use std::collections::HashMap;

use engine_core::prelude::*;

use crate::skeleton::Skeleton;
use crate::skin::{MorphDelta, MorphTarget, SkinVertex, SkinnedMesh};

/// Vertex paint: linear RGBA, the detail mask (cloth, skin, veins, sheen) and how much fur
/// grows (0..1 of the material's fur length).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub color: Vec4,
    pub mask: Vec4,
    pub fur: f32,
}

impl Paint {
    /// Skin with veins where muscles work.
    pub fn skin(rgb: [f32; 3]) -> Self {
        Self {
            color: Vec3::from(rgb).extend(1.0),
            mask: Vec4::new(0.0, 1.0, 0.0, 0.15),
            fur: 0.0,
        }
    }
    /// Bare skin over working muscle (veins show when it flexes).
    pub fn veined(rgb: [f32; 3]) -> Self {
        Self {
            color: Vec3::from(rgb).extend(1.0),
            mask: Vec4::new(0.0, 1.0, 1.0, 0.15),
            fur: 0.0,
        }
    }
    /// Fabric that wrinkles under compression and stretch.
    pub fn cloth(rgb: [f32; 3]) -> Self {
        Self {
            color: Vec3::from(rgb).extend(1.0),
            mask: Vec4::new(1.0, 0.0, 0.0, 0.8),
            fur: 0.0,
        }
    }
    /// Short fur or hide: skin-like light wrap with sheen.
    pub fn hide(rgb: [f32; 3]) -> Self {
        Self {
            color: Vec3::from(rgb).extend(1.0),
            mask: Vec4::new(0.0, 0.6, 0.5, 1.0),
            fur: 0.0,
        }
    }
    /// A furred coat: hide underneath, `amount` (0..1) of the material's fur length on top.
    pub fn fur(rgb: [f32; 3], amount: f32) -> Self {
        Self {
            fur: amount,
            ..Self::hide(rgb)
        }
    }
    /// Hard material (paint, plastic, chitin): no detail.
    pub fn hard(rgb: [f32; 3]) -> Self {
        Self {
            color: Vec3::from(rgb).extend(1.0),
            mask: Vec4::ZERO,
            fur: 0.0,
        }
    }
}

/// A primitive shape in the body's bind (rest) pose, meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A capsule whose radius tapers from `ra` at `a` to `rb` at `b`.
    Capsule { a: Vec3, b: Vec3, ra: f32, rb: f32 },
    Ellipsoid {
        center: Vec3,
        radii: Vec3,
        rotation: Quat,
    },
    RoundBox {
        center: Vec3,
        half: Vec3,
        rotation: Quat,
        radius: f32,
    },
    /// A cylinder along `axis` with rounded rims (wheels, tubes, barrels).
    Cylinder {
        center: Vec3,
        axis: Vec3,
        radius: f32,
        half_length: f32,
        round: f32,
    },
}

impl Shape {
    pub fn capsule(a: Vec3, b: Vec3, ra: f32, rb: f32) -> Self {
        Self::Capsule { a, b, ra, rb }
    }
    pub fn ellipsoid(center: Vec3, radii: Vec3) -> Self {
        Self::Ellipsoid {
            center,
            radii,
            rotation: Quat::IDENTITY,
        }
    }
    pub fn round_box(center: Vec3, half: Vec3, radius: f32) -> Self {
        Self::RoundBox {
            center,
            half,
            rotation: Quat::IDENTITY,
            radius,
        }
    }
    pub fn cylinder(center: Vec3, axis: Vec3, radius: f32, half_length: f32, round: f32) -> Self {
        Self::Cylinder {
            center,
            axis: axis.normalize_or(Vec3::Y),
            radius,
            half_length,
            round,
        }
    }
    pub fn rotated(self, r: Quat) -> Self {
        match self {
            Self::Ellipsoid { center, radii, .. } => Self::Ellipsoid {
                center,
                radii,
                rotation: r,
            },
            Self::RoundBox {
                center,
                half,
                radius,
                ..
            } => Self::RoundBox {
                center,
                half,
                rotation: r,
                radius,
            },
            other => other,
        }
    }

    /// Signed distance (approximate for ellipsoids and tapered capsules), meters.
    pub fn distance(&self, p: Vec3) -> f32 {
        match *self {
            Self::Capsule { a, b, ra, rb } => {
                let ba = b - a;
                let t = ((p - a).dot(ba) / ba.length_squared().max(1e-12)).clamp(0.0, 1.0);
                (p - (a + ba * t)).length() - (ra + (rb - ra) * t)
            }
            Self::Ellipsoid {
                center,
                radii,
                rotation,
            } => {
                let q = rotation.conjugate() * (p - center);
                let k0 = (q / radii).length();
                let k1 = (q / (radii * radii)).length();
                if k1 < 1e-9 {
                    -radii.min_element()
                } else {
                    k0 * (k0 - 1.0) / k1
                }
            }
            Self::RoundBox {
                center,
                half,
                rotation,
                radius,
            } => {
                let q = (rotation.conjugate() * (p - center)).abs() - half + Vec3::splat(radius);
                q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - radius
            }
            Self::Cylinder {
                center,
                axis,
                radius,
                half_length,
                round,
            } => {
                let d = p - center;
                let along = d.dot(axis);
                let radial = (d - axis * along).length();
                let q = Vec2::new(radial - radius + round, along.abs() - half_length + round);
                q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - round
            }
        }
    }

    /// Direction the part runs along (limb axis), for wrinkle and vein orientation.
    pub fn axis(&self) -> Vec3 {
        match *self {
            Self::Capsule { a, b, .. } => (b - a).normalize_or(Vec3::Y),
            Self::Cylinder { axis, .. } => axis,
            Self::Ellipsoid {
                radii, rotation, ..
            }
            | Self::RoundBox {
                half: radii,
                rotation,
                ..
            } => {
                let local = if radii.x >= radii.y && radii.x >= radii.z {
                    Vec3::X
                } else if radii.z >= radii.y {
                    Vec3::Z
                } else {
                    Vec3::Y
                };
                rotation * local
            }
        }
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        match *self {
            Self::Capsule { a, b, ra, rb } => {
                let r = Vec3::splat(ra.max(rb));
                (a.min(b) - r, a.max(b) + r)
            }
            Self::Ellipsoid { center, radii, .. } => {
                let r = Vec3::splat(radii.max_element());
                (center - r, center + r)
            }
            Self::RoundBox { center, half, .. } => {
                let r = Vec3::splat(half.length());
                (center - r, center + r)
            }
            Self::Cylinder {
                center,
                radius,
                half_length,
                ..
            } => {
                let r = Vec3::splat(radius.max(half_length) * 1.42);
                (center - r, center + r)
            }
        }
    }
}

/// One part of a [`Body`].
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub shape: Shape,
    pub bone: String,
    pub paint: Paint,
    /// Smooth-union radius with the rest of the body, meters.
    pub blend: f32,
    /// Overrides the shape's own axis for detail orientation.
    pub axis: Option<Vec3>,
    /// Carve this shape out of the parts before it (wheel arches, eye sockets).
    pub subtract: bool,
}

/// A muscle region: becomes a bulge morph target `<name>` and a sway target `<name>.sway`.
#[derive(Debug, Clone, PartialEq)]
pub struct MuscleShape {
    pub name: String,
    pub center: Vec3,
    pub radii: Vec3,
    pub rotation: Quat,
    /// Bulge height at the center at full activation, meters.
    pub bulge: f32,
    /// Sway offset at the center at weight 1 (direction and size), meters.
    pub sway: Vec3,
}

/// A character body. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Body {
    pub parts: Vec<Part>,
    pub muscles: Vec<MuscleShape>,
}

impl Body {
    pub fn part(&mut self, bone: &str, shape: Shape, paint: Paint, blend: f32) -> &mut Self {
        self.parts.push(Part {
            shape,
            bone: bone.into(),
            paint,
            blend,
            axis: None,
            subtract: false,
        });
        self
    }

    /// Carve `shape` out of everything added before it; the cut surface follows `bone`.
    pub fn cut(&mut self, bone: &str, shape: Shape, paint: Paint, blend: f32) -> &mut Self {
        self.parts.push(Part {
            shape,
            bone: bone.into(),
            paint,
            blend,
            axis: None,
            subtract: true,
        });
        self
    }

    pub fn muscle(
        &mut self,
        name: &str,
        center: Vec3,
        radii: Vec3,
        bulge: f32,
        sway: Vec3,
    ) -> &mut Self {
        self.muscles.push(MuscleShape {
            name: name.into(),
            center,
            radii,
            rotation: Quat::IDENTITY,
            bulge,
            sway,
        });
        self
    }

    /// Smooth-union distance of the whole body.
    pub fn distance(&self, p: Vec3) -> f32 {
        let mut d = f32::MAX;
        for part in &self.parts {
            d = combine(d, part, part.shape.distance(p));
        }
        d
    }

    fn field(&self) -> Field<'_> {
        Field {
            body: self,
            bounds: self.parts.iter().map(|p| p.shape.bounds()).collect(),
        }
    }

    /// Mesh, skin and add muscle targets. `cell` is the grid spacing in meters (smaller = more
    /// triangles); `falloff` how far (meters) skin weights blend across neighboring parts.
    pub fn build(&self, skeleton: &Skeleton, cell: f32, falloff: f32) -> SkinnedMesh {
        let f = self.field();
        let (positions, indices) = f.surface_nets(cell);
        let mut mesh = SkinnedMesh {
            joints: skeleton.names.to_vec(),
            inverse_bind: skeleton.inverse_bind.clone(),
            indices,
            ..Default::default()
        };
        let mut dists = vec![0.0f32; self.parts.len()];
        for p in positions {
            // How far each part's own surface is from this vertex. On a union surface every
            // part is outside (d ≥ 0); a cut or an overlap can leave parts around a vertex.
            // A cut also removes the parts before it, so their surfaces only count where
            // they survive the cut.
            for (d, part) in dists.iter_mut().zip(&self.parts) {
                *d = part.shape.distance(p);
            }
            for j in (0..self.parts.len()).rev() {
                if self.parts[j].subtract {
                    let carve = -dists[j];
                    for (i, d) in dists.iter_mut().enumerate().take(j) {
                        if !self.parts[i].subtract {
                            *d = d.max(carve);
                        }
                    }
                }
            }
            for d in dists.iter_mut() {
                *d = d.abs();
            }
            let nearest = dists
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .map_or(0, |(i, _)| i);
            let d_min = dists[nearest];
            // Soft weights per bone from every nearby part.
            let mut per_bone: HashMap<usize, f32> = HashMap::new();
            let mut paint_w = 0.0;
            let mut color = Vec4::ZERO;
            let mut mask = Vec4::ZERO;
            let mut fur = 0.0;
            let mut axis = Vec3::ZERO;
            for (i, part) in self.parts.iter().enumerate() {
                let w = (-(dists[i] - d_min).max(0.0) / falloff.max(1e-4)).exp();
                if w < 0.02 {
                    continue;
                }
                if let Some(b) = skeleton.index_of(&part.bone) {
                    *per_bone.entry(b).or_default() += w;
                }
                // Paint blends over a much shorter distance so seams stay crisp.
                let pw = (-(dists[i] - d_min).max(0.0) / (falloff * 0.12).max(1e-4)).exp();
                paint_w += pw;
                color += part.paint.color * pw;
                mask += part.paint.mask * pw;
                fur += part.paint.fur * pw;
                let a = part.axis.unwrap_or_else(|| part.shape.axis());
                // Axes are directions without sign: align before averaging.
                axis += if axis.dot(a) < 0.0 { -a } else { a } * w;
            }
            let mut bones: Vec<(usize, f32)> = per_bone.into_iter().collect();
            bones.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            bones.truncate(4);
            let total: f32 = bones.iter().map(|b| b.1).sum::<f32>().max(1e-6);
            let mut joints = [0u16; 4];
            let mut weights = [0.0f32; 4];
            for (k, (b, w)) in bones.iter().enumerate() {
                joints[k] = *b as u16;
                weights[k] = w / total;
            }
            // Subtle color variation so large areas don't look flat.
            let n1 = hash3(p * 37.0) * 0.5 + hash3(p * 9.0) * 0.5;
            let tint = 0.94 + 0.08 * n1;
            let c = color / paint_w.max(1e-6);
            mesh.vertices.push(SkinVertex {
                position: p,
                normal: f.gradient(p).normalize_or(Vec3::Y),
                uv: Vec2::new(p.x + p.z, p.y),
                color: Vec4::new(c.x * tint, c.y * tint, c.z * tint, c.w),
                joints,
                weights,
                pattern: p,
                axis: axis.normalize_or(Vec3::Y),
                mask: mask / paint_w.max(1e-6),
                fur: fur / paint_w.max(1e-6),
            });
        }
        self.add_muscles(&mut mesh);
        mesh
    }

    fn add_muscles(&self, mesh: &mut SkinnedMesh) {
        add_muscle_morphs(mesh, &self.muscles, |_, _| true);
    }
}

/// Add each muscle as a bulge morph target `<name>` (along the vertex normals, strongest at
/// the shape's center) and, if it sways, a `<name>.sway` target, with matching normal deltas.
/// `allowed(vertex, muscle)` can keep a muscle off vertices it overlaps but doesn't belong to
/// (an arm muscle near the chest). Works on any mesh, generated or imported.
pub fn add_muscle_morphs(
    mesh: &mut SkinnedMesh,
    muscles: &[MuscleShape],
    allowed: impl Fn(usize, &MuscleShape) -> bool,
) {
    for m in muscles {
        let mut bulge = Vec::new();
        let mut sway = Vec::new();
        for (i, v) in mesh.vertices.iter().enumerate() {
            let q = m.rotation.conjugate() * (v.position - m.center);
            let r2 = (q / m.radii).length_squared();
            if r2 >= 1.0 || !allowed(i, m) {
                continue;
            }
            let influence = (1.0 - r2) * (1.0 - r2);
            bulge.push(MorphDelta {
                vertex: i as u32,
                position: v.normal * m.bulge * influence,
                normal: Vec3::ZERO,
                influence,
            });
            if m.sway != Vec3::ZERO {
                sway.push(MorphDelta {
                    vertex: i as u32,
                    position: m.sway * influence,
                    normal: Vec3::ZERO,
                    influence: 0.0,
                });
            }
        }
        if bulge.is_empty() {
            continue;
        }
        set_normal_deltas(mesh, &mut bulge);
        mesh.morph_targets.push(MorphTarget {
            name: m.name.clone(),
            deltas: bulge,
        });
        if !sway.is_empty() {
            mesh.morph_targets.push(MorphTarget {
                name: format!("{}.sway", m.name),
                deltas: sway,
            });
        }
    }
}

/// A body prepared for many distance queries: parts whose bounds are far enough away to not
/// change the smooth union are skipped (exactly, not approximately).
struct Field<'a> {
    body: &'a Body,
    bounds: Vec<(Vec3, Vec3)>,
}

impl Field<'_> {
    fn distance(&self, p: Vec3) -> f32 {
        let mut d = f32::MAX;
        for (part, (lo, hi)) in self.body.parts.iter().zip(&self.bounds) {
            if d != f32::MAX {
                let outside = (*lo - p).max(p - *hi).max(Vec3::ZERO).length();
                // Far from a union part: it can't lower the minimum. Outside a cut: nothing
                // to carve.
                let skip = if part.subtract {
                    outside >= -d + part.blend
                } else {
                    outside >= d + part.blend
                };
                if skip {
                    continue;
                }
            }
            d = combine(d, part, part.shape.distance(p));
        }
        d
    }

    fn gradient(&self, p: Vec3) -> Vec3 {
        let e = 5e-4;
        Vec3::new(
            self.distance(p + Vec3::X * e) - self.distance(p - Vec3::X * e),
            self.distance(p + Vec3::Y * e) - self.distance(p - Vec3::Y * e),
            self.distance(p + Vec3::Z * e) - self.distance(p - Vec3::Z * e),
        ) / (2.0 * e)
    }

    /// Surface nets: one vertex per grid cell the surface crosses, one quad per crossed edge.
    fn surface_nets(&self, cell: f32) -> (Vec<Vec3>, Vec<u32>) {
        self.surface_nets_sampled(cell, true)
    }

    /// Surface nets; `skip_empty` samples only near the surface (same mesh, much faster).
    fn surface_nets_sampled(&self, cell: f32, skip_empty: bool) -> (Vec<Vec3>, Vec<u32>) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for part in &self.body.parts {
            let (a, b) = part.shape.bounds();
            lo = lo.min(a - Vec3::splat(part.blend));
            hi = hi.max(b + Vec3::splat(part.blend));
        }
        if self.body.parts.is_empty() {
            return (Vec::new(), Vec::new());
        }
        lo -= Vec3::splat(cell * 2.0);
        hi += Vec3::splat(cell * 2.0);
        let dims = ((hi - lo) / cell).ceil().as_uvec3() + UVec3::ONE;
        let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
        let at = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
        let corner =
            |x: usize, y: usize, z: usize| lo + Vec3::new(x as f32, y as f32, z as f32) * cell;
        // Sample the field block by block: a block whose center is farther from the surface
        // than its corners are from the center holds no surface, so its points only need the
        // right sign (the center's value); blocks near the surface are sampled exactly. The
        // margin covers shapes whose distance is only approximate (ellipsoids, blends).
        const BLOCK: usize = 4;
        let half_diagonal = (BLOCK as f32 * cell) * 3f32.sqrt() * 0.5;
        let mut field = vec![f32::NAN; nx * ny * nz];
        for bz in (0..nz).step_by(BLOCK) {
            for by in (0..ny).step_by(BLOCK) {
                for bx in (0..nx).step_by(BLOCK) {
                    let center = lo
                        + (Vec3::new(bx as f32, by as f32, bz as f32) + BLOCK as f32 * 0.5) * cell;
                    let dc = self.distance(center);
                    let empty = skip_empty && dc.abs() > half_diagonal * 1.5 + cell;
                    for z in bz..(bz + BLOCK + 1).min(nz) {
                        for y in by..(by + BLOCK + 1).min(ny) {
                            for x in bx..(bx + BLOCK + 1).min(nx) {
                                let i = at(x, y, z);
                                if !empty {
                                    field[i] = self.distance(corner(x, y, z));
                                } else if field[i].is_nan() {
                                    field[i] = dc;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Cell vertices.
        let mut cell_vertex = vec![u32::MAX; nx * ny * nz];
        let mut positions = Vec::new();
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
        for z in 0..nz - 1 {
            for y in 0..ny - 1 {
                for x in 0..nx - 1 {
                    let d: [f32; 8] = CORNERS.map(|(i, j, k)| field[at(x + i, y + j, z + k)]);
                    let inside = d.iter().filter(|v| **v < 0.0).count();
                    if inside == 0 || inside == 8 {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut n = 0.0;
                    for (a, b) in EDGES {
                        if (d[a] < 0.0) != (d[b] < 0.0) {
                            let t = d[a] / (d[a] - d[b]);
                            let (ia, ja, ka) = CORNERS[a];
                            let (ib, jb, kb) = CORNERS[b];
                            let pa = Vec3::new(ia as f32, ja as f32, ka as f32);
                            let pb = Vec3::new(ib as f32, jb as f32, kb as f32);
                            sum += pa.lerp(pb, t);
                            n += 1.0;
                        }
                    }
                    let mut p = corner(x, y, z) + sum / n * cell;
                    // Project onto the surface.
                    for _ in 0..3 {
                        let g = self.gradient(p);
                        let g2 = g.length_squared();
                        if g2 < 1e-8 {
                            break;
                        }
                        let step = g * (self.distance(p) / g2);
                        p -= step.clamp_length_max(cell);
                    }
                    cell_vertex[at(x, y, z)] = positions.len() as u32;
                    positions.push(p);
                }
            }
        }
        // Quads around crossed edges.
        let mut indices = Vec::new();
        let mut quad = |q: [u32; 4], flip: bool| {
            if q.contains(&u32::MAX) {
                return;
            }
            let [a, b, c, d] = if flip { [q[0], q[3], q[2], q[1]] } else { q };
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        };
        for z in 1..nz - 1 {
            for y in 1..ny - 1 {
                for x in 1..nx - 1 {
                    let d0 = field[at(x, y, z)] < 0.0;
                    // +X edge: cells sharing it vary in y and z.
                    if x + 1 < nx && d0 != (field[at(x + 1, y, z)] < 0.0) {
                        quad(
                            [
                                cell_vertex[at(x, y - 1, z - 1)],
                                cell_vertex[at(x, y, z - 1)],
                                cell_vertex[at(x, y, z)],
                                cell_vertex[at(x, y - 1, z)],
                            ],
                            !d0,
                        );
                    }
                    if y + 1 < ny && d0 != (field[at(x, y + 1, z)] < 0.0) {
                        quad(
                            [
                                cell_vertex[at(x - 1, y, z - 1)],
                                cell_vertex[at(x - 1, y, z)],
                                cell_vertex[at(x, y, z)],
                                cell_vertex[at(x, y, z - 1)],
                            ],
                            !d0,
                        );
                    }
                    if z + 1 < nz && d0 != (field[at(x, y, z + 1)] < 0.0) {
                        quad(
                            [
                                cell_vertex[at(x - 1, y - 1, z)],
                                cell_vertex[at(x, y - 1, z)],
                                cell_vertex[at(x, y, z)],
                                cell_vertex[at(x - 1, y, z)],
                            ],
                            !d0,
                        );
                    }
                }
            }
        }
        // Make every triangle face outward (along the field gradient).
        for t in indices.as_chunks_mut::<3>().0 {
            let [a, b, c] = [
                positions[t[0] as usize],
                positions[t[1] as usize],
                positions[t[2] as usize],
            ];
            let face = (b - a).cross(c - a);
            if face.dot(self.gradient((a + b + c) / 3.0)) < 0.0 {
                t.swap(1, 2);
            }
        }
        (positions, indices)
    }
}

/// Normal deltas for a morph target: the morphed mesh's normals minus the rest normals.
/// Vertices split along UV or material seams share one normal (by position).
fn welded(mesh: &SkinnedMesh) -> Vec<usize> {
    let mut first: HashMap<[i32; 3], usize> = HashMap::new();
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let k = (v.position * 1e5).round();
            *first
                .entry([k.x as i32, k.y as i32, k.z as i32])
                .or_insert(i)
        })
        .collect()
}

fn set_normal_deltas(mesh: &SkinnedMesh, deltas: &mut [MorphDelta]) {
    let weld = welded(mesh);
    let mut moved: Vec<Vec3> = mesh.vertices.iter().map(|v| v.position).collect();
    for d in deltas.iter() {
        moved[d.vertex as usize] += d.position;
    }
    let mut normals = vec![Vec3::ZERO; moved.len()];
    let mut rest_normals = vec![Vec3::ZERO; moved.len()];
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let face = (moved[b] - moved[a]).cross(moved[c] - moved[a]);
        let pa = mesh.vertices[a].position;
        let rest = (mesh.vertices[b].position - pa).cross(mesh.vertices[c].position - pa);
        for i in [a, b, c] {
            normals[weld[i]] += face;
            rest_normals[weld[i]] += rest;
        }
    }
    for d in deltas.iter_mut() {
        let i = weld[d.vertex as usize];
        d.normal = normals[i].normalize_or_zero() - rest_normals[i].normalize_or_zero();
    }
}

/// Add one part's distance to the running field value.
fn combine(d: f32, part: &Part, di: f32) -> f32 {
    if d == f32::MAX {
        if part.subtract { f32::MAX } else { di }
    } else if part.subtract {
        -smin(-d, di, part.blend)
    } else {
        smin(d, di, part.blend)
    }
}

/// Polynomial smooth minimum (Quilez).
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

fn hash3(p: Vec3) -> f32 {
    let p = p.floor();
    let h = (p.x * 127.1 + p.y * 311.7 + p.z * 74.7).sin() * 43758.547;
    h - h.floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::BoneDef;

    fn arm() -> (Skeleton, Body) {
        let s = Skeleton::new(
            "arm",
            vec![
                BoneDef::new("upper", None, Vec3::ZERO),
                BoneDef::new("lower", Some("upper"), Vec3::new(0.0, -0.3, 0.0)),
            ],
        )
        .unwrap();
        let mut body = Body::default();
        body.part(
            "upper",
            Shape::capsule(Vec3::ZERO, Vec3::new(0.0, -0.3, 0.0), 0.06, 0.05),
            Paint::cloth([0.8, 0.2, 0.2]),
            0.03,
        )
        .part(
            "lower",
            Shape::capsule(
                Vec3::new(0.0, -0.3, 0.0),
                Vec3::new(0.0, -0.58, 0.0),
                0.05,
                0.04,
            ),
            Paint::veined([0.8, 0.6, 0.5]),
            0.03,
        )
        .muscle(
            "biceps",
            Vec3::new(0.0, -0.15, -0.04),
            Vec3::new(0.06, 0.1, 0.06),
            0.02,
            Vec3::new(0.0, -0.01, 0.0),
        );
        (s, body)
    }

    #[test]
    fn sampling_only_near_the_surface_makes_the_same_mesh() {
        let (_, mut body) = arm();
        body.part(
            "lower",
            Shape::ellipsoid(Vec3::new(0.03, -0.45, 0.02), Vec3::new(0.05, 0.03, 0.04)),
            Paint::hard([0.1; 3]),
            0.01,
        )
        .cut(
            "upper",
            Shape::capsule(
                Vec3::new(0.0, -0.1, -0.08),
                Vec3::new(0.0, -0.2, -0.08),
                0.04,
                0.04,
            ),
            Paint::hard([0.1; 3]),
            0.01,
        );
        let f = body.field();
        for cell in [0.006, 0.011] {
            assert_eq!(
                f.surface_nets_sampled(cell, true),
                f.surface_nets_sampled(cell, false),
                "cell {cell}"
            );
        }
    }

    #[test]
    fn surface_nets_make_a_closed_outward_mesh() {
        let (s, body) = arm();
        let mesh = body.build(&s, 0.012, 0.03);
        assert!(mesh.vertices.len() > 300, "{}", mesh.vertices.len());
        assert!(mesh.triangle_count() > 600);
        // Every vertex lies on the surface.
        for v in &mesh.vertices {
            assert!(
                body.distance(v.position).abs() < 0.004,
                "{}",
                body.distance(v.position)
            );
        }
        // Closed: each edge is shared by exactly two triangles.
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for t in mesh.indices.as_chunks::<3>().0 {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open = edges.values().filter(|c| **c != 2).count();
        assert!(
            open * 100 < edges.len(),
            "{open} of {} edges open",
            edges.len()
        );
        // Outward: signed volume is positive.
        let volume: f32 = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize].position);
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        assert!(volume > 0.0, "{volume}");
    }

    #[test]
    fn weights_blend_across_the_joint_and_paint_stays_crisp() {
        let (s, body) = arm();
        let mesh = body.build(&s, 0.012, 0.03);
        let lower = s.index_of("lower").unwrap() as u16;
        let weight_of = |v: &SkinVertex, j: u16| {
            v.joints
                .iter()
                .zip(v.weights)
                .filter(|(jj, _)| **jj == j)
                .map(|(_, w)| w)
                .sum::<f32>()
        };
        for v in &mesh.vertices {
            let w = weight_of(v, lower);
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4);
            if v.position.y > -0.15 {
                assert!(w < 0.1, "shoulder vertex follows the forearm: {w}");
            }
            if v.position.y < -0.45 {
                assert!(w > 0.9, "wrist vertex doesn't follow the forearm: {w}");
            }
            if v.position.y > -0.1 {
                assert!(v.mask.x > 0.9, "sleeve is cloth");
            }
            if v.position.y < -0.4 {
                assert!(v.mask.y > 0.9 && v.mask.x < 0.1, "forearm is skin");
            }
        }
        let elbow: Vec<f32> = mesh
            .vertices
            .iter()
            .filter(|v| (v.position.y + 0.3).abs() < 0.01)
            .map(|v| weight_of(v, lower))
            .collect();
        assert!(!elbow.is_empty());
        let mean = elbow.iter().sum::<f32>() / elbow.len() as f32;
        assert!((0.25..0.75).contains(&mean), "elbow blend {mean}");
    }

    #[test]
    fn muscles_become_bulge_and_sway_targets() {
        let (s, body) = arm();
        let mesh = body.build(&s, 0.012, 0.03);
        let names: Vec<&str> = mesh.morph_targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["biceps", "biceps.sway"]);
        let bulge = &mesh.morph_targets[0];
        let max = bulge
            .deltas
            .iter()
            .map(|d| d.position.length())
            .fold(0.0, f32::max);
        assert!(max > 0.012 && max <= 0.0201, "{max}");
        // Bulges push outward and tilt the normals.
        for d in &bulge.deltas {
            assert!(d.position.dot(mesh.vertices[d.vertex as usize].normal) >= 0.0);
        }
        assert!(bulge.deltas.iter().any(|d| d.normal.length() > 0.02));
    }
}
