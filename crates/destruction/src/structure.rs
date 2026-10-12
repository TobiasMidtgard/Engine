//! Building pieces with Valheim-style structural integrity, and props that fracture.
//!
//! **Structural pieces** (posts, beams, floors, roofs, stone blocks) touch each other. Pieces
//! on the ground (or marked `foundation`) have their material's full support; every other
//! piece takes the best support of its neighbors minus a loss per meter climbed and a bigger
//! loss per meter reached sideways. A piece whose support drops below the minimum gives way a
//! moment later and falls as debris (stone shatters when it lands). Knock out a post and the
//! floor it carried comes down with everything on it. Burnt wooden pieces collapse too.
//!
//! **Fracturable** props (pillars, crates, statues, panes) take damage and, when it is enough,
//! split into Voronoi pieces around the impact point.

use engine_core::math::{DVec3, Quat, Rng, Vec3};
use engine_core::prelude::*;

use crate::fracture::{ConvexPoly, impact_seeds, voronoi};
use crate::material::{DestructibleMaterial, DestructionMaterials, MaterialProps};

/// A building piece that holds up, and is held up by, the pieces it touches.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform, PieceState)]
#[serde(default)]
pub struct StructuralPiece {
    /// Half size of the piece's box in its local axes, meters.
    pub half_extents: Vec3,
    pub material: DestructibleMaterial,
    /// Energy that breaks the piece, J (0 = from its volume and material).
    pub break_energy_j: f32,
    /// Always fully supported (foundations, posts set in concrete).
    pub foundation: bool,
    /// Linear RGB of the piece (for its fragments); `None` = the material's color.
    pub color: Option<[f32; 3]>,
}

impl Default for StructuralPiece {
    fn default() -> Self {
        Self {
            half_extents: Vec3::new(0.1, 1.0, 0.1),
            material: DestructibleMaterial::Wood,
            break_energy_j: 0.0,
            foundation: false,
            color: None,
        }
    }
}

impl StructuralPiece {
    pub fn new(material: DestructibleMaterial, size: Vec3) -> Self {
        Self {
            half_extents: size * 0.5,
            material,
            ..Self::default()
        }
    }

    /// Energy that breaks it, J.
    pub fn break_energy(&self, props: &MaterialProps) -> f32 {
        if self.break_energy_j > 0.0 {
            return self.break_energy_j;
        }
        let v = 8.0 * self.half_extents.x * self.half_extents.y * self.half_extents.z;
        (v * props.toughness_j_m3 * props.break_fraction).max(50.0)
    }
}

/// Live structural state of a piece, written by the module (read it to show support like
/// Valheim's build mode: blue on the ground, green, yellow, red about to fail).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PieceState {
    /// Current support, support units.
    pub support: f32,
    /// Support it would have resting on the ground.
    pub max_support: f32,
    /// Damage taken, J.
    pub damage_j: f32,
    /// Seconds until it gives way (set when it loses support).
    pub collapse_in: Option<f32>,
    /// Where it was last hit, local space.
    pub last_hit: Option<Vec3>,
}

impl PieceState {
    /// Support as a fraction of the ground value, 0..1.
    pub fn support_fraction(&self) -> f32 {
        if self.max_support <= 0.0 {
            0.0
        } else {
            (self.support / self.max_support).clamp(0.0, 1.0)
        }
    }
}

/// Paint this piece in Valheim's build-mode support colors (blue on the ground, green, yellow,
/// orange, red about to fail) instead of its own material while `on`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowSupport {
    pub on: bool,
    /// Paint its children's renderers too (pieces built from several meshes).
    pub children: bool,
}

impl Default for ShowSupport {
    fn default() -> Self {
        Self {
            on: true,
            children: false,
        }
    }
}

/// The materials a tinted piece had before [`ShowSupport`] painted it.
#[derive(Component, Debug, Clone)]
pub struct SupportOriginal(pub Vec<(Entity, engine_render::MaterialRef)>);

/// Recolor [`ShowSupport`] pieces from their live support, and give them their own look back
/// when the view is switched off or the piece breaks loose.
pub fn support_tint_system(
    mut commands: Commands,
    assets: Option<Res<crate::assets::DestructionAssets>>,
    graph: Res<StructureGraph>,
    pieces: Query<
        (
            Entity,
            Option<&PieceState>,
            Option<&ShowSupport>,
            Option<&Children>,
            Option<&SupportOriginal>,
        ),
        Or<(With<ShowSupport>, With<SupportOriginal>)>,
    >,
    mut renderers: Query<&mut engine_render::MeshRenderer>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (e, state, show, children, original) in &pieces {
        let active = show.is_some_and(|s| s.on) && state.is_some();
        if !active {
            if let Some(original) = original {
                for (t, m) in &original.0 {
                    if let Ok(mut r) = renderers.get_mut(*t) {
                        r.material = m.clone();
                    }
                }
                commands.entity(e).remove::<SupportOriginal>();
            }
            continue;
        }
        let mut targets = vec![e];
        if show.is_some_and(|s| s.children)
            && let Some(children) = children
        {
            targets.extend(children.iter());
        }
        // Remember each target's own material the first time it is tinted (children such as
        // a prop's meshes can appear after the piece itself).
        let mut saved = original.map(|o| o.0.clone()).unwrap_or_default();
        let before = saved.len();
        for t in &targets {
            if !saved.iter().any(|(s, _)| s == t)
                && let Ok(r) = renderers.get(*t)
            {
                saved.push((*t, r.material.clone()));
            }
        }
        if original.is_none() || saved.len() != before {
            commands.entity(e).insert(SupportOriginal(saved));
        }
        let grounded = graph
            .pieces
            .iter()
            .position(|p| *p == e)
            .is_some_and(|i| graph.grounded[i]);
        let fraction = state.map(|s| s.support_fraction()).unwrap_or(0.0);
        let m = engine_render::MaterialRef::Handle(assets.support_color(grounded, fraction));
        for t in targets {
            if let Ok(mut r) = renderers.get_mut(t)
                && r.material != m
            {
                r.material = m.clone();
            }
        }
    }
}

/// Shape a [`Fracturable`] breaks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FractureShape {
    /// Box with these half extents, meters.
    Box { half_extents: Vec3 },
    /// Upright column.
    Column {
        radius: f32,
        half_height: f32,
        sides: u32,
    },
}

impl Default for FractureShape {
    fn default() -> Self {
        FractureShape::Box {
            half_extents: Vec3::splat(0.5),
        }
    }
}

impl FractureShape {
    pub fn poly(&self) -> ConvexPoly {
        match *self {
            FractureShape::Box { half_extents } => ConvexPoly::cuboid(half_extents),
            FractureShape::Column {
                radius,
                half_height,
                sides,
            } => ConvexPoly::prism(radius, half_height, sides),
        }
    }

    pub fn half_extents(&self) -> Vec3 {
        match *self {
            FractureShape::Box { half_extents } => half_extents,
            FractureShape::Column {
                radius,
                half_height,
                ..
            } => Vec3::new(radius, half_height, radius),
        }
    }

    pub fn volume(&self) -> f32 {
        match *self {
            FractureShape::Box { half_extents: h } => 8.0 * h.x * h.y * h.z,
            FractureShape::Column {
                radius,
                half_height,
                ..
            } => std::f32::consts::PI * radius * radius * half_height * 2.0,
        }
    }
}

/// A prop that shatters into Voronoi pieces when damaged enough. Draw it however you like
/// (its own `MeshRenderer`); the pieces use `color` or the material's colors.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Fracturable {
    pub shape: FractureShape,
    pub material: DestructibleMaterial,
    /// Energy that breaks it, J (0 = from volume and material).
    pub break_energy_j: f32,
    /// Number of pieces.
    pub pieces: u32,
    /// Damage taken so far, J.
    pub damage_j: f32,
    /// Linear RGB of the outside (for the pieces); `None` = the material's color.
    pub color: Option<[f32; 3]>,
}

impl Default for Fracturable {
    fn default() -> Self {
        Self {
            shape: FractureShape::default(),
            material: DestructibleMaterial::Stone,
            break_energy_j: 0.0,
            pieces: 14,
            damage_j: 0.0,
            color: None,
        }
    }
}

impl Fracturable {
    pub fn break_energy(&self, props: &MaterialProps) -> f32 {
        if self.break_energy_j > 0.0 {
            self.break_energy_j
        } else {
            (self.shape.volume() * props.toughness_j_m3 * props.break_fraction).max(20.0)
        }
    }
}

/// Neighbor graph of structural pieces and their support values.
#[derive(Resource, Debug, Default)]
pub struct StructureGraph {
    pub pieces: Vec<Entity>,
    /// For each piece: (neighbor index, vertical meters, horizontal meters).
    pub edges: Vec<Vec<(usize, f32, f32)>>,
    pub grounded: Vec<bool>,
    pub dirty: bool,
}

/// An oriented box in world space.
#[derive(Debug, Clone, Copy)]
pub struct Obb {
    pub center: DVec3,
    pub rotation: Quat,
    pub half: Vec3,
}

impl Obb {
    pub fn axes(&self) -> [Vec3; 3] {
        [
            self.rotation * Vec3::X,
            self.rotation * Vec3::Y,
            self.rotation * Vec3::Z,
        ]
    }

    /// Lowest world Y of any corner.
    pub fn min_y(&self) -> f64 {
        let a = self.axes();
        let ext = (a[0] * self.half.x).y.abs()
            + (a[1] * self.half.y).y.abs()
            + (a[2] * self.half.z).y.abs();
        self.center.y - ext as f64
    }

    /// Whether two boxes touch (overlap or come within `margin` meters), by separating axes.
    pub fn touches(&self, other: &Obb, margin: f32) -> bool {
        let d = (other.center - self.center).as_vec3();
        let a = self.axes();
        let b = other.axes();
        let ha = self.half + Vec3::splat(margin * 0.5);
        let hb = other.half + Vec3::splat(margin * 0.5);
        let radius = |axes: &[Vec3; 3], h: Vec3, l: Vec3| {
            (axes[0].dot(l) * h.x).abs()
                + (axes[1].dot(l) * h.y).abs()
                + (axes[2].dot(l) * h.z).abs()
        };
        let test = |l: Vec3| {
            let len = l.length_squared();
            if len < 1e-10 {
                return true;
            }
            let l = l / len.sqrt();
            d.dot(l).abs() <= radius(&a, ha, l) + radius(&b, hb, l)
        };
        for i in 0..3 {
            if !test(a[i]) || !test(b[i]) {
                return false;
            }
        }
        for ai in a {
            for bj in b {
                if !test(ai.cross(bj)) {
                    return false;
                }
            }
        }
        true
    }

    /// Distance from `p` to the box surface (0 inside).
    pub fn distance(&self, p: DVec3) -> f32 {
        let local = self.rotation.inverse() * (p - self.center).as_vec3();
        (local.abs() - self.half).max(Vec3::ZERO).length()
    }

    /// Ray entry distance along `dir` (normalized) from `origin`, if it hits within `max`.
    pub fn ray(&self, origin: DVec3, dir: Vec3, max: f32) -> Option<f32> {
        let inv_rot = self.rotation.inverse();
        let o = inv_rot * (origin - self.center).as_vec3();
        let d = inv_rot * dir;
        let inv = d.recip();
        let t0 = (-self.half - o) * inv;
        let t1 = (self.half - o) * inv;
        let tmin = t0.min(t1).max_element().max(0.0);
        let tmax = t0.max(t1).min_element().min(max);
        (tmin <= tmax).then_some(tmin)
    }
}

/// Build the neighbor graph: pieces whose boxes touch support each other.
pub fn build_graph(
    items: &[(Entity, Obb, bool)],
    ground_height: impl Fn(DVec3) -> f64,
) -> StructureGraph {
    let n = items.len();
    let mut edges = vec![Vec::new(); n];
    // Broad phase: sort by min x.
    let mut order: Vec<usize> = (0..n).collect();
    let bound = |o: &Obb| {
        let a = o.axes();
        Vec3::new(
            (a[0] * o.half.x).x.abs() + (a[1] * o.half.y).x.abs() + (a[2] * o.half.z).x.abs(),
            (a[0] * o.half.x).y.abs() + (a[1] * o.half.y).y.abs() + (a[2] * o.half.z).y.abs(),
            (a[0] * o.half.x).z.abs() + (a[1] * o.half.y).z.abs() + (a[2] * o.half.z).z.abs(),
        )
    };
    let bounds: Vec<Vec3> = items.iter().map(|(_, o, _)| bound(o)).collect();
    order.sort_by(|&a, &b| {
        (items[a].1.center.x - bounds[a].x as f64)
            .total_cmp(&(items[b].1.center.x - bounds[b].x as f64))
    });
    let margin = 0.05;
    for (oi, &a) in order.iter().enumerate() {
        let max_x = items[a].1.center.x + (bounds[a].x + margin) as f64;
        for &b in &order[oi + 1..] {
            if items[b].1.center.x - bounds[b].x as f64 > max_x {
                break;
            }
            let (oa, ob) = (&items[a].1, &items[b].1);
            let d = (ob.center - oa.center).as_vec3();
            let reach = bounds[a] + bounds[b] + Vec3::splat(margin);
            if d.y.abs() > reach.y || d.z.abs() > reach.z {
                continue;
            }
            if oa.touches(ob, margin) {
                let v = d.y.abs();
                let h = Vec3::new(d.x, 0.0, d.z).length();
                edges[a].push((b, v, h));
                edges[b].push((a, v, h));
            }
        }
    }
    let grounded = items
        .iter()
        .map(|(_, o, foundation)| *foundation || o.min_y() <= ground_height(o.center) + 0.08)
        .collect();
    StructureGraph {
        pieces: items.iter().map(|(e, ..)| *e).collect(),
        edges,
        grounded,
        dirty: false,
    }
}

/// Support of every piece (Valheim rules, per meter): grounded pieces get their material's
/// maximum; others the best neighbor's support minus losses for the distance between them.
pub fn solve_support(graph: &StructureGraph, props: &[MaterialProps]) -> Vec<f32> {
    let n = graph.pieces.len();
    let mut best = vec![0.0f32; n];
    let mut heap = std::collections::BinaryHeap::new();
    #[derive(PartialEq)]
    struct S(f32, usize);
    impl Eq for S {}
    impl PartialOrd for S {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for S {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&o.0).then(self.1.cmp(&o.1))
        }
    }
    for i in 0..n {
        if graph.grounded[i] {
            best[i] = props[i].max_support;
            heap.push(S(best[i], i));
        }
    }
    while let Some(S(s, i)) = heap.pop() {
        if s < best[i] {
            continue;
        }
        for &(j, v, h) in &graph.edges[i] {
            let p = &props[j];
            let cand =
                (s - p.vertical_loss_per_m * v - p.horizontal_loss_per_m * h).min(p.max_support);
            if cand > best[j] {
                best[j] = cand;
                heap.push(S(cand, j));
            }
        }
    }
    best
}

/// Voronoi pieces of `shape` broken at local `impact`, as (piece, center, volume).
pub fn shatter(
    shape: &ConvexPoly,
    impact: Vec3,
    pieces: u32,
    rng: &mut Rng,
) -> Vec<(ConvexPoly, Vec3, f32)> {
    let (lo, hi) = shape.bounds();
    let size = (hi - lo).max_element();
    let seeds = impact_seeds(
        shape,
        impact,
        pieces.clamp(2, 48) as usize,
        size * 0.35,
        rng,
    );
    voronoi(shape, &seeds)
        .into_iter()
        .map(|p| {
            let (v, c) = p.volume_centroid();
            (p, c, v)
        })
        .collect()
}

/// Color for a piece's fragments: explicit, else the material's outside color.
pub fn piece_colors(
    color: Option<[f32; 3]>,
    material: DestructibleMaterial,
    table: &DestructionMaterials,
    textured: bool,
) -> ([f32; 3], [f32; 3]) {
    let p = table.get(material);
    let (ext, int) = (color.unwrap_or(p.exterior), p.interior);
    if !textured {
        return (ext, int);
    }
    // The texture carries the albedo: shade relative to the material's own color.
    let rel = |c: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|k| (c[k] / p.exterior[k].max(1e-3)).clamp(0.3, 1.35))
    };
    (rel(ext), rel(int))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obb(c: [f64; 3], h: [f32; 3]) -> Obb {
        Obb {
            center: DVec3::from_array(c),
            rotation: Quat::IDENTITY,
            half: Vec3::from_array(h),
        }
    }

    #[test]
    fn touching_boxes_connect() {
        let a = obb([0.0, 1.0, 0.0], [0.1, 1.0, 0.1]);
        let b = obb([0.0, 2.1, 0.0], [1.0, 0.1, 0.1]);
        assert!(a.touches(&b, 0.05));
        let c = obb([3.0, 2.1, 0.0], [1.0, 0.1, 0.1]);
        assert!(!a.touches(&c, 0.05));
        let rotated = Obb {
            rotation: Quat::from_rotation_z(0.6),
            ..obb([0.9, 2.5, 0.0], [0.6, 0.1, 0.1])
        };
        assert!(rotated.touches(&b, 0.05));
    }

    /// A post holding a long beam: remove the post and the beam's far end loses support.
    #[test]
    fn support_runs_out_along_a_long_beam() {
        let e = |i: u32| Entity::from_raw_u32(i).unwrap();
        let post = (e(1), obb([0.0, 1.5, 0.0], [0.1, 1.5, 0.1]), false);
        let beams: Vec<_> = (0..4)
            .map(|k| {
                (
                    e(10 + k),
                    obb([1.1 + 2.0 * k as f64, 3.1, 0.0], [1.0, 0.1, 0.1]),
                    false,
                )
            })
            .collect();
        let mut items = vec![post];
        items.extend(beams.iter().cloned());
        let g = build_graph(&items, |_| 0.0);
        assert!(g.grounded[0] && !g.grounded[1]);
        let props: Vec<_> = items
            .iter()
            .map(|_| crate::material::props(DestructibleMaterial::Wood))
            .collect();
        let s = solve_support(&g, &props);
        assert_eq!(s[0], 100.0);
        assert!(s[1] > s[2] && s[2] > s[3], "{s:?}");
        let min = props[0].min_support;
        assert!(s[1] >= min, "the first beam is held");
        assert!(s[4] < min, "8 m out on wood beams is too far: {s:?}");

        // Without the post nothing is held.
        let g2 = build_graph(&items[1..], |_| 0.0);
        let s2 = solve_support(&g2, &props[1..]);
        assert!(s2.iter().all(|v| *v < min));
    }

    #[test]
    fn ray_hits_box() {
        let b = obb([0.0, 1.0, 5.0], [0.5, 1.0, 0.5]);
        let t = b.ray(DVec3::new(0.0, 1.0, 0.0), Vec3::Z, 20.0).unwrap();
        assert!((t - 4.5).abs() < 1e-4);
        assert!(b.ray(DVec3::new(3.0, 1.0, 0.0), Vec3::Z, 20.0).is_none());
        assert!((b.distance(DVec3::new(0.0, 1.0, 7.0)) - 1.5).abs() < 1e-4);
    }

    #[test]
    fn shatter_conserves_volume() {
        let shape = FractureShape::Column {
            radius: 0.4,
            half_height: 1.5,
            sides: 10,
        };
        let mut rng = Rng::new(3);
        let pieces = shatter(&shape.poly(), Vec3::new(0.3, 0.5, 0.0), 12, &mut rng);
        let total: f32 = pieces.iter().map(|p| p.2).sum();
        let (v, _) = shape.poly().volume_centroid();
        assert!((total - v).abs() / v < 0.01, "{total} vs {v}");
    }
}
