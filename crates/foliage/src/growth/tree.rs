//! Developing a [`Tree`] node: the plant grown year by year from its buds (see [`Tree`] for
//! the model and its sources), written out as the same [`Organ`]s every graph develops into.
//! Each shoot is an axis organ holding the direction, length and day of every metamer it
//! added, year after year; branches grow at the nodes of their parent; leaves come out on the
//! young nodes each spring and fall each autumn.

use engine_core::math::{Rng, Vec3};

use super::develop::{Organ, Skeleton, mix_seed};
use super::graph::{Node, PlantGraph, Tree};

const UP: Vec3 = Vec3::Y;
const YEAR: f32 = 365.0;
/// Most leaf organs one tree holds over its life.
pub const MAX_LEAVES: usize = 600_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bud {
    /// Waiting to break (since the year it formed).
    Dormant,
    /// Grew into this shoot.
    Branched(usize),
    Dead,
}

#[derive(Debug, Clone)]
struct Metamer {
    /// Where it ends (its node), at full length.
    end: Vec3,
    dir: Vec3,
    len: f32,
    day: f32,
    year: u32,
    bud: Bud,
    /// Turn of its lateral bud around the shoot.
    roll: f32,
}

#[derive(Debug, Clone)]
struct Shoot {
    parent: Option<usize>,
    /// The parent's node it grows from (its points index: the end of metamer `at - 1`).
    at: u32,
    base: Vec3,
    /// Direction its first metamer starts from.
    dir: Vec3,
    roll: f32,
    angle: f32,
    depth: u32,
    seed: u64,
    metamers: Vec<Metamer>,
    birth: f32,
    death: f32,
    tip_alive: bool,
}

impl Shoot {
    fn alive(&self) -> bool {
        self.death.is_infinite()
    }

    fn tip(&self) -> (Vec3, Vec3) {
        self.metamers
            .last()
            .map_or((self.base, self.dir), |m| (m.end, m.dir))
    }
}

/// The shadow the crown casts, one cell per `voxel_m`, over the box the crown fills.
struct ShadowGrid {
    min: [i32; 3],
    dims: [i32; 3],
    cell: f32,
    s: Vec<f32>,
}

impl ShadowGrid {
    fn new(points: impl Iterator<Item = Vec3> + Clone, cell: f32, depth: i32) -> Self {
        let mut lo = [i32::MAX; 3];
        let mut hi = [i32::MIN; 3];
        for p in points {
            let c = Self::cell_of(p, cell);
            for a in 0..3 {
                lo[a] = lo[a].min(c[a]);
                hi[a] = hi[a].max(c[a]);
            }
        }
        if lo[0] > hi[0] {
            lo = [0; 3];
            hi = [0; 3];
        }
        let pad = [depth + 1, depth + 1, depth + 1];
        let min = [lo[0] - pad[0], lo[1] - pad[1], lo[2] - pad[2]];
        let dims = [
            hi[0] - lo[0] + 1 + 2 * pad[0],
            hi[1] - lo[1] + 1 + 2 * pad[1],
            hi[2] - lo[2] + 1 + 2 * pad[2],
        ];
        Self {
            min,
            dims,
            cell,
            s: vec![0.0; (dims[0] * dims[1] * dims[2]) as usize],
        }
    }

    fn cell_of(p: Vec3, cell: f32) -> [i32; 3] {
        [
            (p.x / cell).floor() as i32,
            (p.y / cell).floor() as i32,
            (p.z / cell).floor() as i32,
        ]
    }

    fn index(&self, c: [i32; 3]) -> Option<usize> {
        let l = [c[0] - self.min[0], c[1] - self.min[1], c[2] - self.min[2]];
        (0..3)
            .all(|a| l[a] >= 0 && l[a] < self.dims[a])
            .then(|| ((l[1] * self.dims[2] + l[2]) * self.dims[0] + l[0]) as usize)
    }

    /// A bud at `p` shades a pyramid of cells below it (Palubicki 2009, "shadow propagation").
    /// The pyramid widens `spread` cells for each cell down (1: Palubicki's 45 degrees; less
    /// lets the sky light in from the side, as round an open-grown crown).
    fn cast(&mut self, p: Vec3, a: f32, b: f32, depth: i32, spread: f32) {
        let c = Self::cell_of(p, self.cell);
        let mut v = a;
        for q in 0..=depth {
            let w = (q as f32 * spread).round() as i32;
            for dx in -w..=w {
                for dz in -w..=w {
                    if let Some(i) = self.index([c[0] + dx, c[1] - q, c[2] + dz]) {
                        self.s[i] += v;
                    }
                }
            }
            v /= b;
        }
    }

    fn at(&self, c: [i32; 3]) -> f32 {
        self.index(c).map_or(0.0, |i| self.s[i])
    }

    /// Light at `p` (1 open sky) and the way toward more of it, leaving out the shadow the bud
    /// there casts itself (`own`).
    fn light(&self, p: Vec3, own: f32) -> (f32, Vec3) {
        let c = Self::cell_of(p, self.cell);
        let q = (1.0 - self.at(c) + own).clamp(0.0, 1.0);
        let mut toward = Vec3::ZERO;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let open = (1.0 - self.at([c[0] + dx, c[1] + dy, c[2] + dz])).max(0.0);
                    toward += Vec3::new(dx as f32, dy as f32, dz as f32).normalize() * open;
                }
            }
        }
        (q, toward.normalize_or(UP))
    }
}

/// A bud this spring: whose, where and how much light it has.
#[derive(Debug, Clone, Copy)]
struct BudSite {
    shoot: usize,
    /// Its metamer (a lateral bud), or none (the shoot's terminal bud).
    metamer: Option<usize>,
    pos: Vec3,
    light: f32,
    toward: Vec3,
}

fn turn(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    if axis.length_squared() < 1e-12 {
        return v;
    }
    engine_core::math::Quat::from_axis_angle(axis.normalize(), angle) * v
}

fn perpendicular(v: Vec3) -> Vec3 {
    let a = if v.x.abs() < 0.9 { Vec3::X } else { Vec3::Z };
    v.cross(a).normalize()
}

/// Grow `tree` (the graph's root, `node`) from `seed` into `sk`.
pub(super) fn develop(graph: &PlantGraph, tree: &Tree, node: usize, seed: u64, sk: &mut Skeleton) {
    let mut rng = Rng::new(mix_seed(seed, 0x7ee));
    let years = tree
        .years
        .min((graph.life_days / YEAR).ceil().max(1.0) as u32);
    let mut shoots = vec![Shoot {
        parent: None,
        at: 0,
        base: Vec3::ZERO,
        dir: UP,
        roll: 0.0,
        angle: 0.0,
        depth: 0,
        seed,
        metamers: Vec::new(),
        birth: tree.flush_day,
        death: f32::INFINITY,
        tip_alive: true,
    }];
    let depth = tree.shadow_depth as i32;
    let lambda = tree.apical_control.clamp(0.05, 0.95);
    let mut metamers = 0usize;
    for y in 0..years {
        let spring = y as f32 * YEAR + tree.flush_day;
        // The buds this spring: every living shoot's tip, and lateral buds formed in an earlier
        // year that haven't broken, died or waited too long.
        let mut buds = Vec::new();
        for (si, s) in shoots.iter_mut().enumerate() {
            if !s.alive() {
                continue;
            }
            if s.tip_alive {
                let (pos, _) = s.tip();
                buds.push(BudSite {
                    shoot: si,
                    metamer: None,
                    pos,
                    light: 0.0,
                    toward: UP,
                });
            }
            for (mi, m) in s.metamers.iter_mut().enumerate() {
                if m.bud != Bud::Dormant || m.year >= y {
                    continue;
                }
                if (y - m.year) as f32 > tree.bud_years {
                    m.bud = Bud::Dead;
                    continue;
                }
                buds.push(BudSite {
                    shoot: si,
                    metamer: Some(mi),
                    pos: m.end,
                    light: 0.0,
                    toward: UP,
                });
            }
        }
        if buds.is_empty() {
            break;
        }
        // The crown's shadow: every bud casts it (the leaves come out round the buds).
        let casters: Vec<Vec3> = buds.iter().map(|b| b.pos).collect();
        let mut grid = ShadowGrid::new(casters.iter().copied(), tree.voxel_m, depth);
        for &p in &casters {
            grid.cast(
                p,
                tree.shadow,
                tree.falloff.max(1.01),
                depth,
                tree.shadow_spread,
            );
        }
        for b in &mut buds {
            (b.light, b.toward) = grid.light(b.pos, tree.shadow);
        }

        let n = shoots.len();
        let mut bud_of = vec![Vec::new(); n];
        for (bi, b) in buds.iter().enumerate() {
            bud_of[b.shoot].push(bi);
        }
        let mut light = sum_light(&shoots, &buds, &bud_of);
        // Shed the branches whose buds no longer get the light to pay their way (and all they
        // carry): the shaded inside and foot of the crown.
        let mut shed = false;
        for si in 1..n {
            let s = &shoots[si];
            if !s.alive() || (spring - s.birth) < 2.0 * YEAR {
                continue;
            }
            let per = light.total[si] / light.count[si].max(1) as f32;
            if light.count[si] == 0 || per < tree.shed_light {
                let mut stack = vec![si];
                while let Some(x) = stack.pop() {
                    if !shoots[x].alive() {
                        continue;
                    }
                    shoots[x].death = spring - 5.0;
                    for m in &shoots[x].metamers {
                        if let Bud::Branched(c) = m.bud {
                            stack.push(c);
                        }
                    }
                }
                shed = true;
            }
        }
        if shed {
            // Their light no longer reaches the parent.
            light = sum_light(&shoots, &buds, &bud_of);
        }
        let Light {
            total,
            above,
            lateral,
            ..
        } = light;

        // Vigor handed up from the roots (Borchert-Honda): at every node the leader keeps
        // its share by apical control, the lateral side gets the rest.
        let mut vigor_in = vec![0.0f32; n];
        vigor_in[0] = tree.vigor * total[0];
        let mut bud_vigor = vec![0.0f32; buds.len()];
        for si in 0..n {
            if !shoots[si].alive() {
                continue;
            }
            let mut v = vigor_in[si];
            let k = shoots[si].metamers.len();
            for mi in 0..k {
                let qm = above[si][mi + 1];
                let ql = lateral[si][mi];
                let split = if qm + ql <= 0.0 {
                    1.0
                } else {
                    lambda * qm / (lambda * qm + (1.0 - lambda) * ql).max(1e-6)
                };
                let vl = v * (1.0 - split);
                v -= vl;
                match shoots[si].metamers[mi].bud {
                    Bud::Branched(c) => vigor_in[c] = vl,
                    Bud::Dormant => {
                        if let Some(&bi) =
                            bud_of[si].iter().find(|&&bi| buds[bi].metamer == Some(mi))
                        {
                            bud_vigor[bi] = vl;
                        }
                    }
                    Bud::Dead => {}
                }
            }
            if let Some(&bi) = bud_of[si].iter().find(|&&bi| buds[bi].metamer.is_none()) {
                bud_vigor[bi] = v;
            }
        }

        // Each bud grows a shoot of ⌊v⌋ metamers this spring, toward the light.
        for (bi, b) in buds.iter().enumerate() {
            if metamers >= tree.max_metamers as usize {
                break;
            }
            let v = bud_vigor[bi];
            // Under one metamer's vigor a bud can still put out a spur: one short metamer.
            let (count, short) = if v >= 1.0 {
                ((v.floor() as u32).min(tree.max_shoot) as usize, 1.0)
            } else if v >= tree.spur_vigor {
                (1, v.max(0.35))
            } else {
                (0, 1.0)
            };
            if count == 0 {
                continue;
            }
            let si = match b.metamer {
                None => b.shoot,
                Some(mi) => {
                    // A lateral bud breaks: a new branch from this node.
                    let parent = &shoots[b.shoot];
                    let m = &parent.metamers[mi];
                    let side = turn(perpendicular(m.dir), m.dir, m.roll);
                    let angle = tree.branch_angle_deg.to_radians() * rng.range(0.85, 1.15);
                    let dir = (m.dir * angle.cos() + side * angle.sin()).normalize();
                    let child = Shoot {
                        parent: Some(b.shoot),
                        at: mi as u32 + 1,
                        base: m.end,
                        dir,
                        roll: m.roll,
                        angle,
                        depth: parent.depth + 1,
                        seed: mix_seed(parent.seed, ((mi as u64) << 8) | 0xb4),
                        metamers: Vec::new(),
                        birth: spring,
                        death: f32::INFINITY,
                        tip_alive: true,
                    };
                    shoots.push(child);
                    let c = shoots.len() - 1;
                    shoots[b.shoot].metamers[mi].bud = Bud::Branched(c);
                    c
                }
            };
            let s = &mut shoots[si];
            let order = tree.order_length.powi(s.depth as i32);
            let (mut pos, mut dir) = s.tip();
            let wander = tree.wander_deg.to_radians();
            for i in 0..count {
                // The leader turns up; limbs turn out toward the level (plagiotropism), so
                // the crown spreads instead of every limb racing the leader for the top.
                let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::ZERO);
                let toward = b.toward * tree.light_seeking
                    + if s.depth == 0 {
                        UP * tree.gravitropism
                    } else {
                        flat * tree.plagiotropism + UP * (tree.gravitropism * 0.35)
                    };
                dir = (dir + toward).normalize_or(dir);
                dir = turn(dir, perpendicular(dir), rng.range(-wander, wander));
                dir = turn(dir, UP, rng.range(-wander, wander));
                let len = tree.internode_m * order * short * rng.range(0.85, 1.15);
                pos += dir * len;
                let k = s.metamers.len() as u32;
                let day = spring + (i as f32 + 1.0) / count as f32 * tree.flush_days
                    - tree.flush_days / count as f32 * rng.range(0.6, 1.0);
                s.metamers.push(Metamer {
                    end: pos,
                    dir,
                    len,
                    day,
                    year: y,
                    bud: Bud::Dormant,
                    roll: tree.phyllotaxis.roll(k, 0) + rng.range(-0.2, 0.2),
                });
                metamers += 1;
            }
            // Only the buds near this year's tip may break later (acrotony).
            if tree.distal_buds > 0 {
                let n = s.metamers.len();
                let keep = (tree.distal_buds as usize).min(count);
                for m in &mut s.metamers[n - count..n - keep] {
                    m.bud = Bud::Dead;
                }
            }
        }
    }
    write_organs(graph, tree, node, &shoots, years, sk);
}

/// Light summed down every living shoot (Borchert-Honda's basipetal pass).
struct Light {
    /// Reaching each shoot's base.
    total: Vec<f32>,
    /// Per shoot and node: what the leader carries above it, and what grows at its side.
    above: Vec<Vec<f32>>,
    lateral: Vec<Vec<f32>>,
    /// Buds each shoot carries, its branches' included.
    count: Vec<usize>,
}

fn sum_light(shoots: &[Shoot], buds: &[BudSite], bud_of: &[Vec<usize>]) -> Light {
    let n = shoots.len();
    let mut out = Light {
        total: vec![0.0; n],
        above: vec![Vec::new(); n],
        lateral: vec![Vec::new(); n],
        count: vec![0; n],
    };
    // Children come after their parents in the list, so walking it backwards sums them first.
    for si in (0..n).rev() {
        let s = &shoots[si];
        if !s.alive() {
            continue;
        }
        let k = s.metamers.len();
        let mut lat = vec![0.0f32; k];
        let mut tip = 0.0;
        for &bi in &bud_of[si] {
            match buds[bi].metamer {
                Some(mi) if s.metamers[mi].bud == Bud::Dormant => lat[mi] += buds[bi].light,
                Some(_) => {}
                None => tip += buds[bi].light,
            }
        }
        let mut count = bud_of[si]
            .iter()
            .filter(|&&bi| {
                buds[bi]
                    .metamer
                    .is_none_or(|mi| s.metamers[mi].bud == Bud::Dormant)
            })
            .count();
        for (mi, m) in s.metamers.iter().enumerate() {
            if let Bud::Branched(c) = m.bud
                && shoots[c].alive()
            {
                lat[mi] += out.total[c];
                count += out.count[c];
            }
        }
        let mut up = vec![0.0f32; k + 1];
        up[k] = tip;
        for mi in (0..k).rev() {
            up[mi] = up[mi + 1] + lat[mi];
        }
        out.total[si] = up[0];
        out.count[si] = count;
        out.above[si] = up;
        out.lateral[si] = lat;
    }
    out
}

/// The shoots as axis organs (parents first), then the leaves on their young nodes.
fn write_organs(
    graph: &PlantGraph,
    tree: &Tree,
    node: usize,
    shoots: &[Shoot],
    years: u32,
    sk: &mut Skeleton,
) {
    // Organ index of each shoot (shoots without a metamer are left out).
    let mut organ_of = vec![None; shoots.len()];
    for (si, s) in shoots.iter().enumerate() {
        if s.metamers.is_empty() {
            continue;
        }
        let parent = match s.parent {
            Some(p) => match organ_of[p] {
                Some(o) => Some(o),
                None => continue,
            },
            None => None,
        };
        let id = sk.organs.len() as u32;
        organ_of[si] = Some(id);
        let birth = s.metamers[0].day - tree.flush_days / 4.0;
        sk.organs.push(Organ {
            node,
            parent,
            at_node: s.at,
            roll: s.roll,
            angle: s.angle,
            upright: false,
            birth: birth.min(s.birth),
            death: s.death,
            scale: 1.0,
            seed: s.seed,
            depth: s.depth,
            internodes: s.metamers.iter().map(|m| m.len).collect(),
            bends: vec![(0.0, 0.0); s.metamers.len()],
            dirs: s.metamers.iter().map(|m| m.dir).collect(),
            node_days: s.metamers.iter().map(|m| m.day).collect(),
        });
        sk.children.push(Vec::new());
        if let Some(p) = parent {
            sk.children[p as usize].push(id);
        }
    }
    let Some(leaf) = tree
        .leaf
        .as_deref()
        .and_then(|n| sk.names.iter().position(|m| m == n))
    else {
        return;
    };
    let Some(Node::Leaf(l)) = graph.nodes.get(&sk.names[leaf]) else {
        return;
    };
    let lasts = l.senesce_days + l.fade_days.max(0.0);
    // The springs a node leafs (year and day), while its shoot lives.
    fn seasons<'a>(
        tree: &'a Tree,
        years: u32,
        s: &'a Shoot,
        m: &'a Metamer,
    ) -> impl Iterator<Item = (u32, f32)> + 'a {
        (m.year..(m.year + tree.leaf_years.max(1)).min(years.max(1))).filter_map(move |y| {
            let birth = (y as f32 * YEAR + tree.leaf_day).max(m.day + 4.0);
            (birth + 30.0 <= s.death).then_some((y, birth))
        })
    }
    // Over the cap, every node keeps the same share of its leaves (not the oldest shoots all).
    let wanted: usize = shoots
        .iter()
        .enumerate()
        .filter(|(si, _)| organ_of[*si].is_some())
        .flat_map(|(_, s)| {
            s.metamers
                .iter()
                .map(move |m| seasons(tree, years, s, m).count())
        })
        .sum::<usize>()
        * tree.leaves_per_node as usize;
    let keep = (MAX_LEAVES as f64 / wanted.max(1) as f64).min(1.0);
    for (si, s) in shoots.iter().enumerate() {
        let Some(parent) = organ_of[si] else { continue };
        for (k, m) in s.metamers.iter().enumerate() {
            for (y, birth) in seasons(tree, years, s, m) {
                for i in 0..tree.leaves_per_node {
                    let ls = mix_seed(s.seed, ((y as u64) << 40) | ((k as u64) << 8) | i as u64);
                    if keep < 1.0 && (ls & 0xffff) as f64 >= keep * 65536.0 {
                        continue;
                    }
                    let jitter = ((ls >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.5;
                    let id = sk.organs.len() as u32;
                    sk.organs.push(Organ {
                        node: leaf,
                        parent: Some(parent),
                        at_node: k as u32 + 1,
                        roll: m.roll + i as f32 * 2.4 + jitter,
                        angle: tree.leaf_angle_deg.to_radians() * (1.0 + 0.4 * jitter),
                        upright: false,
                        birth: birth + jitter * 6.0,
                        death: birth + jitter * 6.0 + lasts,
                        scale: 1.0 + 0.3 * jitter,
                        seed: ls,
                        depth: s.depth + 1,
                        internodes: Vec::new(),
                        bends: Vec::new(),
                        dirs: Vec::new(),
                        node_days: Vec::new(),
                    });
                    sk.children.push(Vec::new());
                    sk.children[parent as usize].push(id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Skeleton, grow_meshes, oak};
    use super::*;

    /// Height, trunk radius at the foot, shoots and leaves alive on day `t`.
    fn measure(sk: &Skeleton, t: f32) -> (f32, f32, usize, usize, usize) {
        let g = oak();
        let m = grow_meshes(&g, sk, t, 1.0);
        let top = m
            .stems
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(0.0f32, f32::max);
        let foot = m
            .stems
            .vertices
            .iter()
            .map(|v| v.position)
            .filter(|p| p[1] < 0.3)
            .map(|p| (p[0] * p[0] + p[2] * p[2]).sqrt())
            .fold(0.0f32, f32::max);
        let width = m
            .leaves
            .vertices
            .iter()
            .map(|v| (v.position[0] * v.position[0] + v.position[2] * v.position[2]).sqrt())
            .fold(0.0f32, f32::max);
        let foot = foot.min(width * 100.0);
        eprint!("crown radius {width:5.2} m; ");
        let shoots = sk
            .alive(t)
            .filter(|(_, o)| sk.names[o.node] == "tree")
            .count();
        let leaves = sk
            .alive(t)
            .filter(|(_, o)| sk.names[o.node] == "leaf")
            .count();
        (top, foot, shoots, leaves, m.triangles())
    }

    /// The oak, grown for `years` seasons only (debug builds are slow).
    fn young_oak(years: u32) -> PlantGraph {
        let mut g = oak();
        if let Some(Node::Tree(t)) = g.nodes.get_mut("tree") {
            t.years = years;
        }
        g
    }

    #[test]
    fn an_oak_is_the_same_tree_from_the_same_seed_and_another_from_another() {
        let g = young_oak(8);
        let a = Skeleton::develop(&g, 3);
        let b = Skeleton::develop(&g, 3);
        let c = Skeleton::develop(&g, 4);
        assert_eq!(a.organs, b.organs);
        assert_ne!(a.organs, c.organs);
    }

    #[test]
    fn an_oak_grows_taller_wider_and_thicker_year_by_year() {
        let g = young_oak(12);
        let sk = Skeleton::develop(&g, 3);
        let mut last = (0.0f32, 0.0f32);
        for y in [2, 5, 8, 11] {
            let (h, foot, shoots, leaves, tris) = measure(&sk, y as f32 * YEAR + 200.0);
            // (At two a low twig can still reach out past the foot of the stem.)
            assert!(
                h > last.0 && foot > last.1,
                "year {y}: {h} m, foot {foot} m after {last:?}"
            );
            assert!(shoots > 0 && leaves > 0 && tris < 2_000_000, "year {y}");
            last = (h, if y == 2 { 0.0 } else { foot });
        }
        // English oak on open ground is about 5-8 m tall at 12 (Colin et al. 2010).
        assert!((4.5..9.0).contains(&last.0), "{} m at 12", last.0);
        assert!((0.03..0.15).contains(&last.1), "foot radius {} m", last.1);
    }

    #[test]
    fn an_oak_leafs_out_in_spring_and_stands_bare_in_winter() {
        let g = young_oak(10);
        let sk = Skeleton::develop(&g, 3);
        let leaves = |t: f32| sk.census(t).get("leaf").copied().unwrap_or(0);
        let shoots = |t: f32| sk.census(t).get("tree").copied().unwrap_or(0);
        let year = 8.0 * YEAR;
        assert_eq!(leaves(year + 30.0), 0, "bare in midwinter");
        assert!(
            leaves(year + 200.0) > 500,
            "{} in summer",
            leaves(year + 200.0)
        );
        assert_eq!(leaves(year + 360.0), 0, "fallen by the next winter");
        assert!(shoots(year + 30.0) > 20, "{} shoots", shoots(year + 30.0));
        assert!(sk.organs.len() < MAX_LEAVES + 50_000);
    }

    #[test]
    fn an_oak_sheds_the_branches_its_crown_shades() {
        let sk = Skeleton::develop(&young_oak(14), 3);
        let shed = sk
            .organs
            .iter()
            .filter(|o| sk.names[o.node] == "tree" && o.death.is_finite())
            .count();
        assert!(shed > 0, "no branch was shed");
    }

    #[test]
    fn a_tree_can_only_be_the_root() {
        let mut g = oak();
        g.root = "leaf".into();
        assert!(g.validate().unwrap_err().contains("only be the root"));
    }

    #[test]
    #[ignore]
    fn print_an_oak_growing() {
        let t0 = std::time::Instant::now();
        let sk = Skeleton::develop(&oak(), 3);
        eprintln!(
            "developed in {:.2} s: {} organs",
            t0.elapsed().as_secs_f32(),
            sk.organs.len()
        );
        for y in [1, 2, 3, 5, 8, 12, 16, 20, 24] {
            let t = y as f32 * YEAR + 200.0;
            let (h, r, s, l, tris) = measure(&sk, t);
            eprintln!(
                "year {y:2}: {h:5.2} m tall, foot radius {r:.3} m, {s:5} shoots, {l:6} leaves, {tris:7} triangles"
            );
        }
    }
}
