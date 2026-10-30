//! Developing a graph into a skeleton: every organ the plant will grow over its life, with the
//! day it starts and the day it is dropped. Worked out once; the plant is then drawn at any age
//! from it ([`super::shape`]). Each organ's random numbers come from its place in the plant
//! (which generator, node and position on its parent), so growing a plant on never changes the
//! organs it already has.

use engine_core::math::Rng;

use super::graph::{Node, PlantGraph};

/// Most organs one skeleton holds (a runaway graph stops here).
pub const MAX_ORGANS: usize = 20_000;

/// One organ: a stem, leaf, flower or fruit.
#[derive(Debug, Clone, PartialEq)]
pub struct Organ {
    /// Index of its generator in [`Skeleton::names`].
    pub node: usize,
    /// The axis it grows on (none for the root).
    pub parent: Option<u32>,
    /// The node of the parent axis it grows at (its tip for an apex).
    pub at_node: u32,
    /// Turn around the parent axis and angle out of it, radians.
    pub roll: f32,
    pub angle: f32,
    /// Stands up straight rather than leaning out of its parent.
    pub upright: bool,
    /// Day it starts growing and day it is gone (`f32::INFINITY`: never).
    pub birth: f32,
    pub death: f32,
    /// Size relative to its generator's own.
    pub scale: f32,
    /// Seed for its own random numbers.
    pub seed: u64,
    pub depth: u32,
    /// Axes: full length of each internode, and the random bend at each node (axis-local
    /// angles, radians).
    pub internodes: Vec<f32>,
    pub bends: Vec<(f32, f32)>,
    /// A tree's shoots ([`super::graph::Tree`]): each internode's direction (set by the light
    /// it grew toward, not by the generator's bends) and the day its node was laid (shoots
    /// grow in spring flushes over many years). Empty for other axes.
    pub dirs: Vec<engine_core::math::Vec3>,
    pub node_days: Vec<f32>,
    /// Comes back every `period` days for `lasts` days of it, from `birth` to `death` (a tree's
    /// leaves, out each spring); `None`: once.
    pub recur: Option<(f32, f32)>,
}

impl Organ {
    /// Its age on day `t` (in this round, if it recurs), or `None` while it is not there.
    pub fn age_at(&self, t: f32) -> Option<f32> {
        if t < self.birth || t >= self.death {
            return None;
        }
        let age = t - self.birth;
        match self.recur {
            None => Some(age),
            Some((period, lasts)) => {
                let a = age.rem_euclid(period.max(1.0));
                (a < lasts).then_some(a)
            }
        }
    }
}

/// Every organ a plant grows, parents before their children.
#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    /// Generator names, in the graph's order (`Organ::node` indexes this).
    pub names: Vec<String>,
    pub organs: Vec<Organ>,
    /// The organs' children, by organ index.
    pub children: Vec<Vec<u32>>,
    pub life_days: f32,
}

pub(super) fn mix_seed(seed: u64, a: u64) -> u64 {
    let mut x = seed ^ a.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

struct Spawn {
    node: usize,
    parent: Option<u32>,
    at_node: u32,
    roll: f32,
    angle: f32,
    upright: bool,
    birth: f32,
    scale: f32,
    seed: u64,
    depth: u32,
}

impl Skeleton {
    /// Develop `graph` from a seed `seed`. The graph must be valid ([`PlantGraph::validate`]).
    pub fn develop(graph: &PlantGraph, seed: u64) -> Self {
        let names: Vec<String> = graph.nodes.keys().cloned().collect();
        let index = |n: &str| names.iter().position(|m| m == n);
        let mut sk = Skeleton {
            names: names.clone(),
            organs: Vec::new(),
            children: Vec::new(),
            life_days: graph.life_days,
        };
        let Some(root) = index(&graph.root) else {
            return sk;
        };
        if let Some(Node::Tree(tree)) = graph.nodes.get(&graph.root) {
            super::tree::develop(graph, tree, root, seed, &mut sk);
            return sk;
        }
        // Breadth-first by spawn order, so parents always come first.
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(Spawn {
            node: root,
            parent: None,
            at_node: 0,
            roll: 0.0,
            angle: 0.0,
            upright: true,
            birth: 0.0,
            scale: 1.0,
            seed,
            depth: 0,
        });
        while let Some(s) = queue.pop_front() {
            if s.birth > graph.life_days || sk.organs.len() >= MAX_ORGANS {
                continue;
            }
            let id = sk.organs.len() as u32;
            let mut rng = Rng::new(s.seed);
            let generator = &graph.nodes[&names[s.node]];
            let mut organ = Organ {
                node: s.node,
                parent: s.parent,
                at_node: s.at_node,
                roll: s.roll,
                angle: s.angle,
                upright: s.upright,
                birth: s.birth,
                death: f32::INFINITY,
                scale: s.scale,
                seed: s.seed,
                depth: s.depth,
                internodes: Vec::new(),
                bends: Vec::new(),
                dirs: Vec::new(),
                node_days: Vec::new(),
                recur: None,
            };
            match generator {
                Node::Axis(a) => {
                    if a.life_days > 0.0 {
                        organ.death = s.birth + a.life_days;
                    }
                    let n = a.max_nodes.max(1);
                    for k in 0..n {
                        let along = k as f32 / n.max(2).saturating_sub(1) as f32;
                        let len = rng.range(a.internode_m.0, a.internode_m.1.max(a.internode_m.0))
                            * (1.0 - a.taper_length * along)
                            * s.scale;
                        organ.internodes.push(len.max(0.0));
                        let w = a.wander_deg.to_radians();
                        organ.bends.push((rng.range(-w, w), rng.range(-w, w)));
                    }
                    let mut counts = vec![0u32; a.children.len()];
                    for k in 0..n {
                        let laid = s.birth + k as f32 * a.plastochron_days;
                        if laid > graph.life_days {
                            break;
                        }
                        for (ci, c) in a.children.iter().enumerate() {
                            let Some(child) = index(&c.spawn) else {
                                continue;
                            };
                            if k < c.from_node
                                || (k - c.from_node) % c.every.max(1) != 0
                                || c.dormant.contains(&k)
                                || (c.count > 0 && counts[ci] >= c.count)
                                || s.depth + 1 > c.max_depth
                            {
                                continue;
                            }
                            counts[ci] += 1;
                            let end = organ.death.min(graph.life_days);
                            for i in 0..c.phyllotaxis.per_node() {
                                for round in 0u64.. {
                                    let cs = mix_seed(
                                        s.seed,
                                        (round << 52)
                                            | ((ci as u64) << 40)
                                            | ((k as u64) << 8)
                                            | i as u64,
                                    );
                                    let jitter =
                                        ((cs >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.25;
                                    let birth = laid
                                        + c.delay_days.max(0.0) * (1.0 + jitter)
                                        + round as f32 * c.repeat_days;
                                    if round > 0 && (c.repeat_days <= 0.0 || birth >= end) {
                                        break;
                                    }
                                    queue.push_back(Spawn {
                                        node: child,
                                        parent: Some(id),
                                        at_node: k,
                                        roll: c.phyllotaxis.roll(k, i) + jitter,
                                        angle: c.angle_deg.to_radians() * (1.0 + 0.6 * jitter),
                                        upright: c.upright,
                                        birth,
                                        scale: s.scale * c.scale,
                                        seed: cs,
                                        depth: s.depth + 1,
                                    });
                                }
                            }
                        }
                    }
                    if let Some(apex) = a.apex.as_deref().and_then(index) {
                        let last = s.birth + (n - 1) as f32 * a.plastochron_days;
                        queue.push_back(Spawn {
                            node: apex,
                            parent: Some(id),
                            at_node: n,
                            roll: 0.0,
                            angle: 0.0,
                            upright: false,
                            birth: last + 0.5 * a.internode_days,
                            scale: s.scale,
                            seed: mix_seed(s.seed, 0xa9e7),
                            depth: s.depth + 1,
                        });
                    }
                }
                Node::Leaf(l) => {
                    if l.senesce_days > 0.0 {
                        organ.death = s.birth + l.senesce_days + l.fade_days.max(0.0);
                    }
                }
                Node::Flower(f) => {
                    let fall = s.birth + f.bud_days + f.open_days + f.bloom_days;
                    organ.death = fall;
                    if let Some(fruit) = f.then.as_deref().and_then(index) {
                        queue.push_back(Spawn {
                            node: fruit,
                            parent: s.parent,
                            at_node: s.at_node,
                            roll: s.roll,
                            angle: s.angle,
                            upright: s.upright,
                            // The fruit starts while the petals are still falling.
                            birth: fall - 0.5 * f.bloom_days,
                            scale: s.scale,
                            seed: mix_seed(s.seed, 0xf2),
                            depth: s.depth + 1,
                        });
                    }
                }
                Node::Fruit(f) => {
                    if f.drop_days > 0.0 {
                        organ.death = s.birth + f.ripe_days + f.drop_days;
                    }
                }
                // Only ever the root (validated), developed above.
                Node::Tree(_) => {}
            }
            sk.organs.push(organ);
            sk.children.push(Vec::new());
            if let Some(p) = s.parent {
                sk.children[p as usize].push(id);
            }
        }
        sk
    }

    /// Organs alive on day `t`.
    pub fn alive(&self, t: f32) -> impl Iterator<Item = (usize, &Organ)> {
        self.organs
            .iter()
            .enumerate()
            .filter(move |(_, o)| o.age_at(t).is_some())
    }

    /// How many organs are alive on day `t`, by generator name.
    pub fn census(&self, t: f32) -> std::collections::BTreeMap<&str, usize> {
        let mut out = std::collections::BTreeMap::new();
        for (_, o) in self.alive(t) {
            *out.entry(self.names[o.node].as_str()).or_insert(0) += 1;
        }
        out
    }
}
