//! The growth graph: a plant described as generators (stems, leaves, flowers, fruit), each
//! growing organs on its parent over time. Plain data (serde), so it is saved, edited and sent
//! through the API as JSON or RON.

use std::collections::BTreeMap;

use engine_core::prelude::*;

/// A plant as a graph of generators: `root` grows first, and every generator names the
/// generators that grow on what it makes. See the [module docs](super).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantGraph {
    pub name: String,
    /// The generator that grows from the seed.
    pub root: String,
    /// Days the plant is grown for (a skeleton covers this much of its life).
    pub life_days: f32,
    /// Material looks: a species from the plant library whose bark the stems wear (trees), or
    /// none for green stems in the vertex colors.
    pub bark_species: Option<String>,
    pub nodes: BTreeMap<String, Node>,
}

impl Default for PlantGraph {
    fn default() -> Self {
        Self {
            name: "plant".into(),
            root: "stem".into(),
            life_days: 120.0,
            bark_species: None,
            nodes: BTreeMap::new(),
        }
    }
}

/// One generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Axis(Axis),
    Leaf(Leaf),
    Flower(Flower),
    Fruit(Fruit),
}

/// How an axis runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AxisKind {
    /// Upright or leaning shoot: trunks, branches, flower stalks.
    #[default]
    Stem,
    /// A crown with no length to speak of: leaves and stalks fan out of one point.
    Rosette,
    /// A runner: arches out and lies along the ground.
    Stolon,
}

/// A stem made of nodes (where children grow) and the internodes between them. Node `k` is
/// laid down `k x plastochron_days` after the axis is born, and its internode then lengthens
/// over `internode_days`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Axis {
    pub kind: AxisKind,
    pub plastochron_days: f32,
    pub max_nodes: u32,
    /// Full length of an internode, meters (low, high: each picks its own).
    pub internode_m: (f32, f32),
    /// Internodes shorten toward the tip by this fraction at the last node.
    pub taper_length: f32,
    pub internode_days: f32,
    /// Radius at the tip, and how much each node grown above a point thickens it (meters per
    /// node to the power 0.7, a stand-in for the pipe model: wood carries what grows above).
    pub tip_radius_m: f32,
    pub thickening_m: f32,
    /// Bending toward the sky per meter (negative: toward the ground).
    pub gravitropism: f32,
    /// Turning toward horizontal per meter (runners, spreading branches).
    pub plagiotropism: f32,
    /// Random bending per node, degrees.
    pub wander_deg: f32,
    /// Bends down under the weight of what grows on it (leaves, flowers and fruit at full
    /// size), radians at full load.
    pub sag: f32,
    /// Stem color (linear RGB) when green, and when woody (old internodes darken to it).
    pub color: [f32; 3],
    pub wood_color: [f32; 3],
    /// Days an internode takes to turn woody (0: never).
    pub lignify_days: f32,
    /// The axis dies (and is dropped) this many days after it is born (0: lives on).
    pub life_days: f32,
    pub children: Vec<Child>,
    /// A generator grown at the tip once the last node is laid down (a flower on a stalk).
    pub apex: Option<String>,
}

impl Default for Axis {
    fn default() -> Self {
        Self {
            kind: AxisKind::Stem,
            plastochron_days: 5.0,
            max_nodes: 8,
            internode_m: (0.05, 0.08),
            taper_length: 0.3,
            internode_days: 8.0,
            tip_radius_m: 0.002,
            thickening_m: 0.0005,
            gravitropism: 0.4,
            plagiotropism: 0.0,
            wander_deg: 6.0,
            sag: 0.0,
            color: [0.16, 0.30, 0.07],
            wood_color: [0.20, 0.15, 0.10],
            lignify_days: 0.0,
            life_days: 0.0,
            children: Vec::new(),
            apex: None,
        }
    }
}

/// How children are turned around their parent axis from node to node.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Phyllotaxis {
    /// Each node turned on by this many degrees (137.5, the golden angle, packs leaves best).
    Spiral(f32),
    /// Two at a node, facing each other; pairs turned 90 degrees from node to node.
    Opposite,
    /// `n` at a node, evenly around, each whorl turned half a step from the last.
    Whorl(u32),
    /// One at a node, alternating sides in one plane.
    Alternate,
}

impl Default for Phyllotaxis {
    fn default() -> Self {
        Self::Spiral(137.5)
    }
}

impl Phyllotaxis {
    /// How many children grow at one node.
    pub fn per_node(self) -> u32 {
        match self {
            Self::Spiral(_) | Self::Alternate => 1,
            Self::Opposite => 2,
            Self::Whorl(n) => n.max(1),
        }
    }

    /// Roll around the axis of child `i` at node `k`, radians.
    pub fn roll(self, k: u32, i: u32) -> f32 {
        use std::f32::consts::{PI, TAU};
        match self {
            Self::Spiral(deg) => (k as f32 * deg).to_radians(),
            Self::Alternate => k as f32 * PI,
            Self::Opposite => k as f32 * 0.5 * PI + i as f32 * PI,
            Self::Whorl(n) => {
                let n = n.max(1) as f32;
                (i as f32 + 0.5 * (k % 2) as f32) * TAU / n
            }
        }
    }
}

/// A rule growing generator `spawn` at the nodes of an axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Child {
    pub spawn: String,
    /// First node it grows at (0 = the base), then every `every` nodes, at most `count` times
    /// (0: no limit).
    pub from_node: u32,
    pub every: u32,
    pub count: u32,
    /// Nodes skipped (a runner's first node stays dormant).
    pub dormant: Vec<u32>,
    pub phyllotaxis: Phyllotaxis,
    /// Angle from the parent axis, degrees (0: carries straight on, 90: straight out).
    pub angle_deg: f32,
    /// Days after its node is laid down that it starts growing.
    pub delay_days: f32,
    /// Size of what it grows (lengths, radii) relative to the generator's own.
    pub scale: f32,
    /// Stand up straight (a daughter plant on a runner) instead of leaning out of the parent.
    pub upright: bool,
    /// Stop recursing this deep (a generator that grows itself, or a runner's daughter that
    /// grows runners).
    pub max_depth: u32,
    /// Grow it again every this many days while the parent lives (365: a deciduous tree's
    /// leaves coming back each spring from the buds along last year's shoots); 0 once.
    pub repeat_days: f32,
}

impl Default for Child {
    fn default() -> Self {
        Self {
            spawn: String::new(),
            from_node: 0,
            every: 1,
            count: 0,
            dormant: Vec::new(),
            phyllotaxis: Phyllotaxis::default(),
            angle_deg: 45.0,
            delay_days: 0.0,
            scale: 1.0,
            upright: false,
            max_depth: 6,
            repeat_days: 0.0,
        }
    }
}

/// Leaf blade outline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LeafKind {
    /// One blade.
    Simple,
    /// Three leaflets at the petiole's tip (strawberry, clover).
    Trifoliate,
    /// `n` leaflets fanned from the tip (horse chestnut, lupin).
    Palmate(u32),
    /// `n` pairs of leaflets along a rachis, and one at the end (ash, rose).
    Pinnate(u32),
}

/// A leaf: a petiole and its blade or leaflets, unfolding as it grows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Leaf {
    pub kind: LeafKind,
    pub petiole_m: (f32, f32),
    pub petiole_radius_m: f32,
    /// Blade (or leaflet) length, meters, and its width as a fraction of the length.
    pub blade_m: (f32, f32),
    pub width: f32,
    /// Where the blade is widest, 0 (base) .. 1 (tip).
    pub widest: f32,
    /// Teeth along the edge: how many per side, and how deep (fraction of the width).
    pub teeth: u32,
    pub tooth_depth: f32,
    /// How far the petiole arches over, and the blade droops at its tip.
    pub droop: f32,
    /// Days to full size; the blade unfolds about its midrib over the first `unfold` of it.
    pub grow_days: f32,
    pub unfold: f32,
    /// Days after birth it starts to yellow, days it takes, then it drops (0: keeps).
    pub senesce_days: f32,
    pub fade_days: f32,
    pub color: [f32; 3],
    pub old_color: [f32; 3],
}

impl Default for Leaf {
    fn default() -> Self {
        Self {
            kind: LeafKind::Simple,
            petiole_m: (0.02, 0.04),
            petiole_radius_m: 0.0012,
            blade_m: (0.05, 0.07),
            width: 0.5,
            widest: 0.4,
            teeth: 0,
            tooth_depth: 0.0,
            droop: 0.3,
            grow_days: 12.0,
            unfold: 0.35,
            senesce_days: 0.0,
            fade_days: 8.0,
            color: [0.07, 0.19, 0.03],
            old_color: [0.42, 0.30, 0.05],
        }
    }
}

/// A flower: a bud that swells, opens its petals, then drops them and sets fruit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Flower {
    pub petals: u32,
    /// Petal length and width, meters.
    pub petal_m: (f32, f32),
    pub petal_color: [f32; 3],
    pub center_color: [f32; 3],
    pub sepals: u32,
    pub sepal_color: [f32; 3],
    /// Stamens ringing the center.
    pub stamens: u32,
    /// Days from bud to opening, days it takes to open, days it stays open.
    pub bud_days: f32,
    pub open_days: f32,
    pub bloom_days: f32,
    /// How far open petals cup (0 flat .. 1 a bowl).
    pub cup: f32,
    /// Grows when the petals drop (a fruit generator).
    pub then: Option<String>,
}

impl Default for Flower {
    fn default() -> Self {
        Self {
            petals: 5,
            petal_m: (0.012, 0.011),
            petal_color: [0.86, 0.85, 0.80],
            center_color: [0.75, 0.62, 0.08],
            sepals: 5,
            sepal_color: [0.08, 0.20, 0.04],
            stamens: 16,
            bud_days: 14.0,
            open_days: 1.5,
            bloom_days: 4.0,
            cup: 0.25,
            then: None,
        }
    }
}

/// A fruit swelling from a flower's center and ripening through its colors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fruit {
    /// Length (along its stalk) and width at full size, meters.
    pub length_m: f32,
    pub width_m: f32,
    /// How pointed it is (0 round .. 1 a cone).
    pub point: f32,
    pub swell_days: f32,
    /// Colors over its life: (fraction of `ripe_days`, linear RGB). The tip runs ahead of the
    /// shoulders.
    pub colors: Vec<(f32, [f32; 3])>,
    pub ripe_days: f32,
    /// Seeds on the skin (a strawberry's achenes), and their color.
    pub seeds: u32,
    pub seed_color: [f32; 3],
    /// Days after ripe it drops (0: stays).
    pub drop_days: f32,
}

impl Default for Fruit {
    fn default() -> Self {
        Self {
            length_m: 0.03,
            width_m: 0.026,
            point: 0.5,
            swell_days: 22.0,
            colors: vec![(0.0, [0.2, 0.35, 0.08]), (1.0, [0.5, 0.02, 0.02])],
            ripe_days: 30.0,
            seeds: 0,
            seed_color: [0.6, 0.45, 0.1],
            drop_days: 0.0,
        }
    }
}

impl PlantGraph {
    /// Checks every generator a child, apex or flower names exists, and the root does.
    pub fn validate(&self) -> Result<(), String> {
        let known = |n: &str, from: &str| {
            if self.nodes.contains_key(n) {
                Ok(())
            } else {
                Err(format!(
                    "\"{from}\" grows \"{n}\", which the graph doesn't have"
                ))
            }
        };
        known(&self.root, "root")?;
        for (name, node) in &self.nodes {
            match node {
                Node::Axis(a) => {
                    for c in &a.children {
                        known(&c.spawn, name)?;
                    }
                    if let Some(apex) = &a.apex {
                        known(apex, name)?;
                    }
                }
                Node::Flower(f) => {
                    if let Some(then) = &f.then {
                        known(then, name)?;
                    }
                }
                Node::Leaf(_) | Node::Fruit(_) => {}
            }
        }
        if self.life_days <= 0.0 {
            return Err("life_days must be above 0".into());
        }
        Ok(())
    }
}
