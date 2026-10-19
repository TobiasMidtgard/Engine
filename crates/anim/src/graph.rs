//! Animation graphs: blend trees, layers, additive poses and state machines.
//!
//! A graph is plain data ([`AnimGraph`], JSON-serializable) so designers and AI agents can
//! write one; [`GraphInstance`] is the compiled, per-character runtime.
//!
//! ```json
//! {"name": "humanoid", "root": {"type": "state_machine", "initial": "move",
//!   "states": [
//!     {"name": "move", "node": {"type": "blend1d", "param": "speed", "sync": true, "match_speed": true,
//!        "children": [{"at": 0.0, "node": {"type": "clip", "clip": "idle"}},
//!                     {"at": 1.4, "node": {"type": "clip", "clip": "walk"}},
//!                     {"at": 4.0, "node": {"type": "clip", "clip": "run"}}]}},
//!     {"name": "swing", "node": {"type": "clip", "clip": "heavy_swing"}}],
//!   "transitions": [
//!     {"from": "move", "to": "swing", "when": {"trigger": "attack_heavy"}, "duration": 0.15},
//!     {"from": "swing", "to": "move", "when": "finished", "duration": 0.3, "blend": "inertialize"}]}}
//! ```
//!
//! * **blend1d / blend2d** mix children by parameters. `sync` keeps their cycles in phase so
//!   walk and run footfalls line up; `match_speed` speeds playback up past the fastest child
//!   so feet don't slide when the character moves faster than the clip.
//! * **layer** plays an overlay on masked bones (upper-body attack over legs that walk);
//!   `additive` layers add a difference instead (breathing, recoil, leaning).
//! * **state_machine** switches between states on triggers, parameter conditions or when a
//!   one-shot finishes; transitions crossfade, or inertialize (switch at once and decay the
//!   difference, which looks snappier and costs one evaluation).

use std::sync::Arc;

use engine_core::math::{Quat, Vec2, Vec3};
use engine_world::animation::AnimationParams;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::clip::{AnimationClip, ClipBinding};
use crate::skeleton::{Skeleton, apply_additive, blend_masked};
use crate::xform::{Xf, nlerp};

fn one() -> f32 {
    1.0
}

/// A weight, optionally read from a parameter (`value × param`, clamped to 0..1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weight {
    pub value: f32,
    pub param: Option<String>,
}

impl Default for Weight {
    fn default() -> Self {
        Self {
            value: 1.0,
            param: None,
        }
    }
}

impl Weight {
    pub fn param(name: &str) -> Self {
        Self {
            value: 1.0,
            param: Some(name.into()),
        }
    }

    fn get(&self, params: &AnimationParams) -> f32 {
        let p = self.param.as_deref().map_or(1.0, |p| params.get(p));
        (self.value * p).clamp(0.0, 1.0)
    }
}

/// Bones a layer affects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoneMaskDef {
    pub bones: Vec<String>,
    /// Include every descendant of the listed bones.
    pub include_children: bool,
}

impl Default for BoneMaskDef {
    fn default() -> Self {
        Self {
            bones: Vec::new(),
            include_children: true,
        }
    }
}

/// One child of a 1D blend, placed at `at` on the parameter axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blend1dChild {
    pub at: f32,
    pub node: NodeDef,
}

/// One child of a 2D blend, placed at `at` in parameter space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blend2dChild {
    pub at: [f32; 2],
    pub node: NodeDef,
}

/// A node of an animation graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeDef {
    /// The skeleton's rest pose.
    Rest,
    /// Play a clip from the [`AnimationLibrary`](crate::AnimationLibrary).
    Clip {
        clip: String,
        #[serde(default = "one")]
        speed: f32,
        /// Override the clip's own looping flag.
        #[serde(default)]
        looping: Option<bool>,
    },
    Blend1d {
        param: String,
        children: Vec<Blend1dChild>,
        #[serde(default)]
        sync: bool,
        /// Children's `at` values are their movement speeds (m/s); play faster beyond the
        /// last one so strides keep up.
        #[serde(default)]
        match_speed: bool,
    },
    Blend2d {
        x: String,
        y: String,
        children: Vec<Blend2dChild>,
        #[serde(default)]
        sync: bool,
    },
    Additive {
        base: Box<NodeDef>,
        additive: Box<NodeDef>,
        #[serde(default)]
        weight: Weight,
    },
    Layer {
        base: Box<NodeDef>,
        overlay: Box<NodeDef>,
        #[serde(default)]
        mask: BoneMaskDef,
        #[serde(default)]
        weight: Weight,
        /// Add the overlay's difference from the rest pose instead of replacing.
        #[serde(default)]
        additive: bool,
    },
    StateMachine {
        states: Vec<StateDef>,
        #[serde(default)]
        transitions: Vec<TransitionDef>,
        /// First state (defaults to the first listed).
        #[serde(default)]
        initial: Option<String>,
    },
}

/// A state of a state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateDef {
    pub name: String,
    pub node: NodeDef,
    #[serde(default = "one")]
    pub speed: f32,
}

/// Comparison for parameter conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Gt,
    Lt,
    Ge,
    Le,
}

/// When a transition fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Always,
    /// The current state's one-shot clip reached its end.
    Finished,
    /// An `AnimationTrigger` with this name arrived (consumed by the transition).
    Trigger(String),
    Param {
        name: String,
        op: Cmp,
        value: f32,
    },
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

/// How a transition blends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    /// Fade from the last pose of the old state to the new state.
    #[default]
    Crossfade,
    /// Switch immediately and decay the pose difference.
    Inertialize,
}

/// A state machine transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionDef {
    /// Source state name, or `"*"` for any state.
    pub from: String,
    pub to: String,
    pub when: Condition,
    /// Blend time, seconds.
    #[serde(default)]
    pub duration: f32,
    #[serde(default)]
    pub blend: BlendMode,
    /// Only fire once the source state has played this fraction (0..1) of its length.
    #[serde(default)]
    pub exit_time: Option<f32>,
}

impl TransitionDef {
    pub fn new(from: &str, to: &str, when: Condition, duration: f32) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            when,
            duration,
            blend: BlendMode::Crossfade,
            exit_time: None,
        }
    }

    pub fn inertialize(mut self) -> Self {
        self.blend = BlendMode::Inertialize;
        self
    }
}

/// A named animation graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimGraph {
    pub name: String,
    pub root: NodeDef,
}

impl NodeDef {
    pub fn clip(name: &str) -> Self {
        NodeDef::Clip {
            clip: name.into(),
            speed: 1.0,
            looping: None,
        }
    }

    pub fn one_shot(name: &str) -> Self {
        NodeDef::Clip {
            clip: name.into(),
            speed: 1.0,
            looping: Some(false),
        }
    }
}

impl Condition {
    pub fn trigger(name: &str) -> Self {
        Condition::Trigger(name.into())
    }

    pub fn param(name: &str, op: Cmp, value: f32) -> Self {
        Condition::Param {
            name: name.into(),
            op,
            value,
        }
    }
}

// ------------------------------------------------------------------------------- runtime

/// An event fired by a clip during evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct FiredEvent {
    pub name: String,
    pub bone: Option<String>,
}

/// Inputs and outputs of one graph evaluation.
pub struct EvalCtx<'a> {
    pub params: &'a AnimationParams,
    /// Pending trigger names (with seconds left before they expire); transitions consume them.
    pub triggers: &'a mut Vec<(String, f32)>,
    /// Seconds to advance (already scaled by time dilation and the animator's speed).
    pub dt: f32,
    pub events: &'a mut Vec<FiredEvent>,
    /// Weighted curve values (name, value).
    pub curves: &'a mut Vec<(String, f32)>,
    /// Weighted root-motion displacement, model space.
    pub root_delta: Vec3,
}

impl EvalCtx<'_> {
    fn add_curve(&mut self, name: &str, value: f32) {
        if let Some(c) = self.curves.iter_mut().find(|c| c.0 == name) {
            c.1 += value;
        } else {
            self.curves.push((name.to_string(), value));
        }
    }

    fn has_trigger(&self, name: &str) -> bool {
        self.triggers.iter().any(|t| t.0 == name)
    }

    fn consume(&mut self, name: &str) {
        if let Some(i) = self.triggers.iter().position(|t| t.0 == name) {
            self.triggers.remove(i);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Args {
    /// Global weight of this node in the final pose (for curves and root motion).
    weight: f32,
    /// Fire clip events.
    emit: bool,
    /// Synchronized phase 0..1 imposed by a parent blend.
    phase: Option<f32>,
    /// Playback rate multiplier.
    rate: f32,
    /// Write a pose (false: only advance time).
    sample: bool,
}

struct ClipNode {
    clip: Arc<AnimationClip>,
    binding: ClipBinding,
    speed: f32,
    looping: bool,
    time: f32,
    cycles: f32,
    last_phase: f32,
    finished: bool,
    last_weight: f32,
}

struct Blend1dNode {
    param: String,
    points: Vec<(f32, usize)>,
    sync: bool,
    match_speed: bool,
    phase: f32,
}

struct Blend2dNode {
    x: String,
    y: String,
    points: Vec<(Vec2, usize)>,
    sync: bool,
    phase: f32,
}

struct State {
    name: String,
    node: usize,
    speed: f32,
}

struct Transition {
    from: Option<usize>,
    to: usize,
    when: Condition,
    duration: f32,
    mode: BlendMode,
    exit_time: Option<f32>,
}

struct Fade {
    mode: BlendMode,
    elapsed: f32,
    duration: f32,
    /// Crossfade: the frozen source pose. Inertialize: per-bone offsets (filled on the first
    /// frame).
    pose: Vec<Xf>,
    offsets_ready: bool,
}

struct StateMachineNode {
    states: Vec<State>,
    transitions: Vec<Transition>,
    initial: usize,
    current: usize,
    state_time: f32,
    fade: Option<Fade>,
    last: Vec<Xf>,
}

enum Node {
    Rest,
    Clip(ClipNode),
    Blend1d(Blend1dNode),
    Blend2d(Blend2dNode),
    Additive {
        base: usize,
        add: usize,
        weight: Weight,
    },
    Layer {
        base: usize,
        overlay: usize,
        mask: Vec<f32>,
        weight: Weight,
        additive: bool,
    },
    StateMachine(Box<StateMachineNode>),
}

/// A compiled graph bound to one skeleton. One per character.
pub struct GraphInstance {
    nodes: Vec<Node>,
    root: usize,
    rest: Vec<Xf>,
    pool: Vec<Vec<Xf>>,
}

impl GraphInstance {
    /// Compile `graph` for `skeleton`; `clips` resolves clip names. Errors name the missing
    /// clip or state.
    pub fn compile(
        graph: &AnimGraph,
        skeleton: &Skeleton,
        clips: &dyn Fn(&str) -> Option<Arc<AnimationClip>>,
    ) -> Result<Self, String> {
        let mut g = GraphInstance {
            nodes: Vec::new(),
            root: 0,
            rest: skeleton.rest.clone(),
            pool: Vec::new(),
        };
        g.root = g.add(&graph.root, skeleton, clips)?;
        Ok(g)
    }

    fn add(
        &mut self,
        def: &NodeDef,
        skeleton: &Skeleton,
        clips: &dyn Fn(&str) -> Option<Arc<AnimationClip>>,
    ) -> Result<usize, String> {
        let node = match def {
            NodeDef::Rest => Node::Rest,
            NodeDef::Clip {
                clip,
                speed,
                looping,
            } => {
                let c = clips(clip).ok_or_else(|| {
                    format!("animation clip {clip} is not in the library; add it first (anim.clips lists them)")
                })?;
                Node::Clip(ClipNode {
                    binding: ClipBinding::new(&c, skeleton),
                    looping: looping.unwrap_or(c.looping),
                    clip: c,
                    speed: *speed,
                    time: 0.0,
                    cycles: 0.0,
                    last_phase: 0.0,
                    finished: false,
                    last_weight: 0.0,
                })
            }
            NodeDef::Blend1d {
                param,
                children,
                sync,
                match_speed,
            } => {
                if children.is_empty() {
                    return Err(format!("blend1d on {param} has no children"));
                }
                let mut points = Vec::new();
                for c in children {
                    points.push((c.at, self.add(&c.node, skeleton, clips)?));
                }
                points.sort_by(|a, b| a.0.total_cmp(&b.0));
                Node::Blend1d(Blend1dNode {
                    param: param.clone(),
                    points,
                    sync: *sync,
                    match_speed: *match_speed,
                    phase: 0.0,
                })
            }
            NodeDef::Blend2d {
                x,
                y,
                children,
                sync,
            } => {
                if children.is_empty() {
                    return Err(format!("blend2d on {x}/{y} has no children"));
                }
                let mut points = Vec::new();
                for c in children {
                    points.push((Vec2::from(c.at), self.add(&c.node, skeleton, clips)?));
                }
                Node::Blend2d(Blend2dNode {
                    x: x.clone(),
                    y: y.clone(),
                    points,
                    sync: *sync,
                    phase: 0.0,
                })
            }
            NodeDef::Additive {
                base,
                additive,
                weight,
            } => Node::Additive {
                base: self.add(base, skeleton, clips)?,
                add: self.add(additive, skeleton, clips)?,
                weight: weight.clone(),
            },
            NodeDef::Layer {
                base,
                overlay,
                mask,
                weight,
                additive,
            } => Node::Layer {
                base: self.add(base, skeleton, clips)?,
                overlay: self.add(overlay, skeleton, clips)?,
                mask: if mask.bones.is_empty() {
                    vec![1.0; skeleton.len()]
                } else {
                    skeleton.mask(&mask.bones, mask.include_children)
                },
                weight: weight.clone(),
                additive: *additive,
            },
            NodeDef::StateMachine {
                states,
                transitions,
                initial,
            } => {
                if states.is_empty() {
                    return Err("state machine has no states".into());
                }
                let mut rt_states = Vec::new();
                for s in states {
                    rt_states.push(State {
                        name: s.name.clone(),
                        node: self.add(&s.node, skeleton, clips)?,
                        speed: s.speed,
                    });
                }
                let find = |name: &str| -> Result<usize, String> {
                    rt_states
                        .iter()
                        .position(|s| s.name == name)
                        .ok_or_else(|| {
                            let names: Vec<_> = rt_states.iter().map(|s| s.name.as_str()).collect();
                            format!(
                                "state {name} does not exist; states are {}",
                                names.join(", ")
                            )
                        })
                };
                let mut rt_transitions = Vec::new();
                for t in transitions {
                    rt_transitions.push(Transition {
                        from: if t.from == "*" {
                            None
                        } else {
                            Some(find(&t.from)?)
                        },
                        to: find(&t.to)?,
                        when: t.when.clone(),
                        duration: t.duration.max(0.0),
                        mode: t.blend,
                        exit_time: t.exit_time,
                    });
                }
                let initial = match initial {
                    Some(n) => find(n)?,
                    None => 0,
                };
                Node::StateMachine(Box::new(StateMachineNode {
                    states: rt_states,
                    transitions: rt_transitions,
                    initial,
                    current: initial,
                    state_time: 0.0,
                    fade: None,
                    last: skeleton.rest.clone(),
                }))
            }
        };
        self.nodes.push(node);
        Ok(self.nodes.len() - 1)
    }

    /// Evaluate one frame into `out` (local pose, resized to the skeleton).
    pub fn evaluate(&mut self, ctx: &mut EvalCtx, out: &mut Vec<Xf>) {
        out.clear();
        out.extend_from_slice(&self.rest);
        let args = Args {
            weight: 1.0,
            emit: true,
            phase: None,
            rate: 1.0,
            sample: true,
        };
        self.eval(self.root, ctx, out, args);
        // Expire unused triggers.
        let dt = ctx.dt;
        ctx.triggers.retain_mut(|t| {
            t.1 -= dt;
            t.1 > 0.0
        });
    }

    fn take_pose(&mut self) -> Vec<Xf> {
        let mut p = self.pool.pop().unwrap_or_default();
        p.clear();
        p.extend_from_slice(&self.rest);
        p
    }

    fn give_pose(&mut self, p: Vec<Xf>) {
        self.pool.push(p);
    }

    /// Cycle length of a node in seconds at the current parameters (for sync).
    fn duration(&self, i: usize, params: &AnimationParams) -> f32 {
        match &self.nodes[i] {
            Node::Clip(c) => c.clip.duration / c.speed.max(1e-3),
            Node::Blend1d(b) => {
                let x = params.get(&b.param);
                blend1d_weights(&b.points, x)
                    .into_iter()
                    .map(|(child, w)| w * self.duration(child, params))
                    .sum::<f32>()
                    .max(1e-3)
            }
            Node::Blend2d(b) => {
                let p = Vec2::new(params.get(&b.x), params.get(&b.y));
                let w = blend2d_weights(&b.points, p);
                b.points
                    .iter()
                    .zip(w)
                    .map(|((_, child), w)| w * self.duration(*child, params))
                    .sum::<f32>()
                    .max(1e-3)
            }
            Node::Additive { base, .. } | Node::Layer { base, .. } => self.duration(*base, params),
            Node::StateMachine(sm) => self.duration(sm.states[sm.current].node, params),
            Node::Rest => 1.0,
        }
    }

    fn finished(&self, i: usize) -> bool {
        match &self.nodes[i] {
            Node::Clip(c) => !c.looping && c.finished,
            Node::Blend1d(b) => b.points.iter().all(|(_, c)| self.finished(*c)),
            Node::Blend2d(b) => b.points.iter().all(|(_, c)| self.finished(*c)),
            Node::Additive { base, .. } | Node::Layer { base, .. } => self.finished(*base),
            Node::StateMachine(sm) => self.finished(sm.states[sm.current].node),
            Node::Rest => false,
        }
    }

    fn reset(&mut self, i: usize) {
        let mut children = Vec::new();
        match &mut self.nodes[i] {
            Node::Clip(c) => {
                c.time = 0.0;
                c.cycles = 0.0;
                c.last_phase = 0.0;
                c.finished = false;
            }
            Node::Blend1d(b) => {
                b.phase = 0.0;
                children.extend(b.points.iter().map(|p| p.1));
            }
            Node::Blend2d(b) => {
                b.phase = 0.0;
                children.extend(b.points.iter().map(|p| p.1));
            }
            Node::Additive { base, add, .. } => children.extend([*base, *add]),
            Node::Layer { base, overlay, .. } => children.extend([*base, *overlay]),
            Node::StateMachine(sm) => {
                sm.current = sm.initial;
                sm.state_time = 0.0;
                sm.fade = None;
                children.push(sm.states[sm.current].node);
            }
            Node::Rest => {}
        }
        for c in children {
            self.reset(c);
        }
    }

    fn eval(&mut self, i: usize, ctx: &mut EvalCtx, out: &mut [Xf], args: Args) {
        let mut node = std::mem::replace(&mut self.nodes[i], Node::Rest);
        match &mut node {
            Node::Rest => {}
            Node::Clip(c) => eval_clip(c, ctx, out, args),
            Node::Blend1d(b) => {
                let x = ctx.params.get(&b.param);
                let weights = blend1d_weights(&b.points, x);
                let mut rate = args.rate;
                if b.match_speed
                    && let Some(&(top, _)) = b.points.last()
                    && top > 1e-3
                    && x > top
                {
                    rate *= x / top;
                }
                // With `match_speed` the points are speeds, and a child standing still (at 0)
                // has no step cycle: the cycle is the moving children's, whose stride shrinks
                // with their weight, so planted feet keep pace with the body at slow speeds
                // instead of the idle's long loop slowing every step.
                let mut cycle = weights.clone();
                if b.match_speed {
                    cycle.retain(|(c, _)| b.points.iter().any(|p| p.1 == *c && p.0.abs() > 1e-3));
                    let total: f32 = cycle.iter().map(|w| w.1).sum();
                    if total > 1e-3 {
                        cycle.iter_mut().for_each(|w| w.1 /= total);
                    } else {
                        cycle = weights.clone();
                    }
                }
                let phase = if b.sync {
                    // Put the nodes back temporarily so `duration` can read them.
                    self.nodes[i] = node;
                    let d: f32 = cycle
                        .iter()
                        .map(|&(c, w)| w * self.duration(c, ctx.params))
                        .sum::<f32>()
                        .max(1e-3);
                    node = std::mem::replace(&mut self.nodes[i], Node::Rest);
                    let Node::Blend1d(b) = &mut node else {
                        unreachable!()
                    };
                    b.phase = match args.phase {
                        Some(p) => p,
                        None => (b.phase + ctx.dt * rate / d).rem_euclid(1.0),
                    };
                    Some(b.phase)
                } else {
                    args.phase
                };
                let Node::Blend1d(b) = &node else {
                    unreachable!()
                };
                let all: Vec<usize> = b.points.iter().map(|p| p.1).collect();
                self.blend_children(&all, &weights, ctx, out, args, phase, rate);
            }
            Node::Blend2d(b) => {
                let p = Vec2::new(ctx.params.get(&b.x), ctx.params.get(&b.y));
                let w = blend2d_weights(&b.points, p);
                let weights: Vec<(usize, f32)> = b
                    .points
                    .iter()
                    .zip(w)
                    .filter(|(_, w)| *w > 1e-4)
                    .map(|((_, c), w)| (*c, w))
                    .collect();
                let phase = if b.sync {
                    self.nodes[i] = node;
                    let d: f32 = weights
                        .iter()
                        .map(|&(c, w)| w * self.duration(c, ctx.params))
                        .sum::<f32>()
                        .max(1e-3);
                    node = std::mem::replace(&mut self.nodes[i], Node::Rest);
                    let Node::Blend2d(b) = &mut node else {
                        unreachable!()
                    };
                    b.phase = match args.phase {
                        Some(p) => p,
                        None => (b.phase + ctx.dt * args.rate / d).rem_euclid(1.0),
                    };
                    Some(b.phase)
                } else {
                    args.phase
                };
                let Node::Blend2d(b) = &node else {
                    unreachable!()
                };
                let all: Vec<usize> = b.points.iter().map(|p| p.1).collect();
                self.blend_children(&all, &weights, ctx, out, args, phase, args.rate);
            }
            Node::Additive { base, add, weight } => {
                let w = weight.get(ctx.params);
                self.eval(*base, ctx, out, args);
                let mut tmp = self.take_pose();
                self.eval(
                    *add,
                    ctx,
                    &mut tmp,
                    Args {
                        weight: args.weight * w,
                        sample: args.sample && w > 0.0,
                        ..args
                    },
                );
                if args.sample {
                    let rest = std::mem::take(&mut self.rest);
                    apply_additive(out, &tmp, &rest, w, None);
                    self.rest = rest;
                }
                self.give_pose(tmp);
            }
            Node::Layer {
                base,
                overlay,
                mask,
                weight,
                additive,
            } => {
                let w = weight.get(ctx.params);
                self.eval(*base, ctx, out, args);
                let mut tmp = self.take_pose();
                self.eval(
                    *overlay,
                    ctx,
                    &mut tmp,
                    Args {
                        weight: args.weight * w,
                        sample: args.sample && w > 0.0,
                        ..args
                    },
                );
                if args.sample {
                    if *additive {
                        let rest = std::mem::take(&mut self.rest);
                        apply_additive(out, &tmp, &rest, w, Some(mask));
                        self.rest = rest;
                    } else {
                        blend_masked(out, &tmp, w, Some(mask));
                    }
                }
                self.give_pose(tmp);
            }
            Node::StateMachine(sm) => {
                // Transitions.
                let state = &sm.states[sm.current];
                sm.state_time += ctx.dt * args.rate * state.speed;
                self.nodes[i] = node;
                let (current, state_time) = match &self.nodes[i] {
                    Node::StateMachine(sm) => (sm.current, sm.state_time),
                    _ => unreachable!(),
                };
                let current_node = match &self.nodes[i] {
                    Node::StateMachine(sm) => sm.states[current].node,
                    _ => unreachable!(),
                };
                let finished = self.finished(current_node);
                let length = self.duration(current_node, ctx.params);
                let mut fire: Option<(usize, f32, BlendMode, Option<String>)> = None;
                if let Node::StateMachine(sm) = &self.nodes[i] {
                    for t in &sm.transitions {
                        if t.from.is_some_and(|f| f != current)
                            || (t.from.is_none() && t.to == current)
                        {
                            continue;
                        }
                        if let Some(e) = t.exit_time
                            && state_time / length < e
                        {
                            continue;
                        }
                        let mut used = None;
                        if check(&t.when, ctx, finished, &mut used) {
                            fire = Some((t.to, t.duration, t.mode, used));
                            break;
                        }
                    }
                }
                if let Some((to, duration, mode, used)) = fire {
                    if let Some(name) = used {
                        ctx.consume(&name);
                    }
                    let target = match &mut self.nodes[i] {
                        Node::StateMachine(sm) => {
                            sm.current = to;
                            sm.state_time = 0.0;
                            sm.fade = (duration > 0.0).then(|| Fade {
                                mode,
                                elapsed: 0.0,
                                duration,
                                pose: sm.last.clone(),
                                offsets_ready: false,
                            });
                            sm.states[to].node
                        }
                        _ => unreachable!(),
                    };
                    self.reset(target);
                }
                node = std::mem::replace(&mut self.nodes[i], Node::Rest);
                let Node::StateMachine(sm) = &mut node else {
                    unreachable!()
                };
                let state_node = sm.states[sm.current].node;
                let speed = sm.states[sm.current].speed;
                self.eval(
                    state_node,
                    ctx,
                    out,
                    Args {
                        rate: args.rate * speed,
                        ..args
                    },
                );
                if let Some(fade) = &mut sm.fade {
                    if args.sample {
                        let x = (fade.elapsed / fade.duration).clamp(0.0, 1.0);
                        let w = x * x * (3.0 - 2.0 * x);
                        match fade.mode {
                            BlendMode::Crossfade => {
                                for (o, s) in out.iter_mut().zip(&fade.pose) {
                                    *o = s.lerp(o, w);
                                }
                            }
                            BlendMode::Inertialize => {
                                if !fade.offsets_ready {
                                    for (s, o) in fade.pose.iter_mut().zip(out.iter()) {
                                        *s = Xf {
                                            t: s.t - o.t,
                                            r: (s.r * o.r.conjugate()).normalize(),
                                            s: Vec3::ONE,
                                        };
                                    }
                                    fade.offsets_ready = true;
                                }
                                // Quintic ease-out of the offset (smooth start and end).
                                let k = 1.0 - x;
                                let decay = k * k * k * (6.0 * x * x + 3.0 * x + 1.0);
                                for (o, off) in out.iter_mut().zip(&fade.pose) {
                                    o.t += off.t * decay;
                                    o.r = (nlerp(Quat::IDENTITY, off.r, decay) * o.r).normalize();
                                }
                            }
                        }
                    }
                    fade.elapsed += ctx.dt;
                    if fade.elapsed >= fade.duration {
                        sm.fade = None;
                    }
                }
                if args.sample {
                    sm.last.clear();
                    sm.last.extend_from_slice(out);
                }
            }
        }
        self.nodes[i] = node;
    }

    #[allow(clippy::too_many_arguments)]
    fn blend_children(
        &mut self,
        all: &[usize],
        weights: &[(usize, f32)],
        ctx: &mut EvalCtx,
        out: &mut [Xf],
        args: Args,
        phase: Option<f32>,
        rate: f32,
    ) {
        // The dominant child fires events.
        let dominant = weights
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|w| w.0);
        let mut acc = 0.0f32;
        let mut tmp = self.take_pose();
        for &child in all {
            let w = weights
                .iter()
                .find(|(c, _)| *c == child)
                .map_or(0.0, |x| x.1);
            let child_args = Args {
                weight: args.weight * w,
                emit: args.emit && Some(child) == dominant,
                phase,
                rate,
                sample: args.sample && w > 1e-4,
            };
            if !child_args.sample {
                // Keep unsynchronized children's clocks running.
                let mut scratch: [Xf; 0] = [];
                self.eval(child, ctx, &mut scratch, child_args);
                continue;
            }
            if acc <= 0.0 {
                self.eval(child, ctx, out, child_args);
                acc = w;
            } else {
                tmp.clear();
                tmp.extend_from_slice(&self.rest);
                self.eval(child, ctx, &mut tmp, child_args);
                acc += w;
                blend_masked(out, &tmp, w / acc, None);
            }
        }
        self.give_pose(tmp);
    }

    /// The current state of every state machine, outermost first.
    pub fn active_states(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_states(self.root, &mut out);
        out
    }

    fn collect_states(&self, i: usize, out: &mut Vec<String>) {
        match &self.nodes[i] {
            Node::StateMachine(sm) => {
                let s = &sm.states[sm.current];
                out.push(s.name.clone());
                self.collect_states(s.node, out);
            }
            Node::Blend1d(b) => b.points.iter().for_each(|p| self.collect_states(p.1, out)),
            Node::Blend2d(b) => b.points.iter().for_each(|p| self.collect_states(p.1, out)),
            Node::Additive { base, add, .. } => {
                self.collect_states(*base, out);
                self.collect_states(*add, out);
            }
            Node::Layer { base, overlay, .. } => {
                self.collect_states(*base, out);
                self.collect_states(*overlay, out);
            }
            _ => {}
        }
    }

    /// Length of the clip named `name` if the graph plays it, seconds.
    pub fn clip_duration(&self, name: &str) -> Option<f32> {
        self.nodes.iter().find_map(|n| match n {
            Node::Clip(c) if c.clip.name == name => Some(c.clip.duration),
            _ => None,
        })
    }

    /// Clips that contributed to the last pose, with their weights and playback times.
    pub fn describe(&self) -> serde_json::Value {
        let clips: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Clip(c) if c.last_weight > 1e-3 => Some(json!({
                    "clip": c.clip.name,
                    "weight": c.last_weight,
                    "time": c.clip.local_time(c.time),
                })),
                _ => None,
            })
            .collect();
        json!({ "states": self.active_states(), "clips": clips })
    }

    /// Whether any state machine is in a state named `name`.
    pub fn in_state(&self, name: &str) -> bool {
        self.active_states().iter().any(|s| s == name)
    }
}

fn eval_clip(c: &mut ClipNode, ctx: &mut EvalCtx, out: &mut [Xf], args: Args) {
    let d = c.clip.duration.max(1e-6);
    let prev = c.time;
    let next = match args.phase {
        Some(p) => {
            if p + 0.5 < c.last_phase {
                c.cycles += 1.0;
            }
            c.last_phase = p;
            (c.cycles + p) * d
        }
        None => c.time + ctx.dt * c.speed * args.rate,
    };
    let next = if c.looping { next } else { next.min(d) };
    c.finished = !c.looping && next >= d;
    c.time = next;
    c.last_weight = if args.sample { args.weight } else { 0.0 };
    if next > prev {
        if args.emit {
            for e in c.clip.events_between(prev, next) {
                ctx.events.push(FiredEvent {
                    name: e.name.clone(),
                    bone: e.bone.clone(),
                });
            }
        }
        if args.weight > 0.0 && c.binding.root.is_some() {
            ctx.root_delta += c.clip.root_delta(&c.binding, prev, next) * args.weight;
        }
    }
    if args.sample {
        let mut t = c.clip.local_time(next);
        // A finished one-shot holds its last frame.
        if !c.looping && c.finished {
            t = d;
        }
        c.clip.sample(t, &c.binding, out);
        for curve in &c.clip.curves {
            if let Some(v) = curve.track.sample(t) {
                ctx.add_curve(&curve.name, v * args.weight);
            }
        }
    }
}

fn check(c: &Condition, ctx: &EvalCtx, finished: bool, used: &mut Option<String>) -> bool {
    match c {
        Condition::Always => true,
        Condition::Finished => finished,
        Condition::Trigger(name) => {
            let hit = ctx.has_trigger(name);
            if hit {
                *used = Some(name.clone());
            }
            hit
        }
        Condition::Param { name, op, value } => {
            let v = ctx.params.get(name);
            match op {
                Cmp::Gt => v > *value,
                Cmp::Lt => v < *value,
                Cmp::Ge => v >= *value,
                Cmp::Le => v <= *value,
            }
        }
        Condition::All(cs) => cs.iter().all(|c| check(c, ctx, finished, used)),
        Condition::Any(cs) => cs.iter().any(|c| check(c, ctx, finished, used)),
        Condition::Not(c) => {
            let mut ignore = None;
            !check(c, ctx, finished, &mut ignore)
        }
    }
}

/// Weights of a 1D blend at `x`: at most two neighboring children.
fn blend1d_weights(points: &[(f32, usize)], x: f32) -> Vec<(usize, f32)> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    if x <= points[0].0 || n == 1 {
        return vec![(points[0].1, 1.0)];
    }
    if x >= points[n - 1].0 {
        return vec![(points[n - 1].1, 1.0)];
    }
    let hi = points.partition_point(|p| p.0 <= x).min(n - 1);
    let lo = hi - 1;
    let span = (points[hi].0 - points[lo].0).max(1e-6);
    let w = ((x - points[lo].0) / span).clamp(0.0, 1.0);
    vec![(points[lo].1, 1.0 - w), (points[hi].1, w)]
}

/// Inverse-distance weights of a 2D blend at `p` (exact at sample points).
fn blend2d_weights(points: &[(Vec2, usize)], p: Vec2) -> Vec<f32> {
    if let Some(i) = points
        .iter()
        .position(|(q, _)| q.distance_squared(p) < 1e-8)
    {
        let mut w = vec![0.0; points.len()];
        w[i] = 1.0;
        return w;
    }
    let raw: Vec<f32> = points
        .iter()
        .map(|(q, _)| 1.0 / q.distance_squared(p).powi(2))
        .collect();
    let sum: f32 = raw.iter().sum();
    raw.into_iter().map(|w| w / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{BoneTrack, Track};
    use crate::skeleton::BoneDef;
    use std::collections::HashMap;

    fn skel() -> Skeleton {
        Skeleton::new(
            "s",
            vec![
                BoneDef::new("root", None, Vec3::ZERO),
                BoneDef::new("arm", Some("root"), Vec3::Y),
            ],
        )
        .unwrap()
    }

    /// A clip that holds `arm` at `angle` about Z, lasting `d` seconds.
    fn hold(name: &str, angle: f32, d: f32, looping: bool) -> Arc<AnimationClip> {
        let mut c = AnimationClip::new(name, d);
        c.looping = looping;
        c.tracks.push(BoneTrack {
            bone: "arm".into(),
            rotation: Track::new(
                vec![0.0, d],
                vec![Quat::from_rotation_z(angle), Quat::from_rotation_z(angle)],
            ),
            ..Default::default()
        });
        Arc::new(c.with_curve("effort", 2, move |_| angle))
    }

    fn library() -> HashMap<String, Arc<AnimationClip>> {
        let mut m = HashMap::new();
        for c in [
            hold("idle", 0.0, 1.0, true),
            hold("walk", 0.5, 1.0, true),
            hold("run", 1.0, 0.6, true),
            hold("swing", 1.5, 0.5, false),
        ] {
            m.insert(c.name.clone(), c);
        }
        m
    }

    fn run(
        g: &mut GraphInstance,
        params: &AnimationParams,
        triggers: &mut Vec<(String, f32)>,
        dt: f32,
    ) -> (Vec<Xf>, Vec<(String, f32)>) {
        let mut events = Vec::new();
        let mut curves = Vec::new();
        let mut ctx = EvalCtx {
            params,
            triggers,
            dt,
            events: &mut events,
            curves: &mut curves,
            root_delta: Vec3::ZERO,
        };
        let mut out = Vec::new();
        g.evaluate(&mut ctx, &mut out);
        (out, curves)
    }

    fn arm_angle(p: &[Xf]) -> f32 {
        crate::xform::twist_angle(p[1].r, Vec3::Z)
    }

    fn locomotion() -> AnimGraph {
        AnimGraph {
            name: "test".into(),
            root: NodeDef::StateMachine {
                initial: Some("move".into()),
                states: vec![
                    StateDef {
                        name: "move".into(),
                        speed: 1.0,
                        node: NodeDef::Blend1d {
                            param: "speed".into(),
                            sync: true,
                            match_speed: true,
                            children: vec![
                                Blend1dChild {
                                    at: 0.0,
                                    node: NodeDef::clip("idle"),
                                },
                                Blend1dChild {
                                    at: 1.0,
                                    node: NodeDef::clip("walk"),
                                },
                                Blend1dChild {
                                    at: 3.0,
                                    node: NodeDef::clip("run"),
                                },
                            ],
                        },
                    },
                    StateDef {
                        name: "swing".into(),
                        speed: 1.0,
                        node: NodeDef::one_shot("swing"),
                    },
                ],
                transitions: vec![
                    TransitionDef::new("move", "swing", Condition::trigger("attack"), 0.1),
                    TransitionDef::new("swing", "move", Condition::Finished, 0.1).inertialize(),
                ],
            },
        }
    }

    #[test]
    fn blend1d_mixes_by_parameter() {
        let lib = library();
        let s = skel();
        let mut g = GraphInstance::compile(&locomotion(), &s, &|n| lib.get(n).cloned()).unwrap();
        let mut params = AnimationParams::default();
        let mut trig = Vec::new();
        params.set("speed", 2.0); // halfway walk → run
        let (pose, curves) = run(&mut g, &params, &mut trig, 0.016);
        assert!(
            (arm_angle(&pose) - 0.75).abs() < 0.02,
            "{}",
            arm_angle(&pose)
        );
        let effort = curves.iter().find(|c| c.0 == "effort").unwrap().1;
        assert!((effort - 0.75).abs() < 1e-3, "{effort}");
    }

    #[test]
    fn trigger_plays_one_shot_and_returns() {
        let lib = library();
        let s = skel();
        let mut g = GraphInstance::compile(&locomotion(), &s, &|n| lib.get(n).cloned()).unwrap();
        let params = AnimationParams::default();
        let mut trig = vec![("attack".to_string(), 0.3)];
        run(&mut g, &params, &mut trig, 0.016);
        assert!(trig.is_empty(), "trigger consumed");
        assert!(g.in_state("swing"));
        // After the fade, the swing pose is fully applied.
        let mut pose = Vec::new();
        for _ in 0..12 {
            pose = run(&mut g, &params, &mut trig, 0.016).0;
        }
        assert!(
            (arm_angle(&pose) - 1.5).abs() < 0.02,
            "{}",
            arm_angle(&pose)
        );
        // The clip is 0.5 s: it finishes and returns to move (idle at speed 0).
        for _ in 0..60 {
            pose = run(&mut g, &params, &mut trig, 0.016).0;
        }
        assert!(g.in_state("move"));
        assert!(arm_angle(&pose).abs() < 0.02, "{}", arm_angle(&pose));
    }

    #[test]
    fn inertialization_starts_from_the_old_pose() {
        let lib = library();
        let s = skel();
        let mut g = GraphInstance::compile(&locomotion(), &s, &|n| lib.get(n).cloned()).unwrap();
        let params = AnimationParams::default();
        let mut trig = vec![("attack".to_string(), 0.3)];
        // Play the swing to its end; the frame it returns, the pose is still near 1.5.
        let mut last = 0.0;
        for _ in 0..200 {
            let (pose, _) = run(&mut g, &params, &mut trig, 0.005);
            if g.in_state("move") {
                last = arm_angle(&pose);
                break;
            }
        }
        assert!(last > 1.3, "inertialized start {last}");
    }

    #[test]
    fn sync_keeps_children_in_phase() {
        // Phase advances by dt / blended duration.
        let lib = library();
        let s = skel();
        let graph = AnimGraph {
            name: "b".into(),
            root: NodeDef::Blend1d {
                param: "speed".into(),
                sync: true,
                match_speed: false,
                children: vec![
                    Blend1dChild {
                        at: 1.0,
                        node: NodeDef::clip("walk"),
                    },
                    Blend1dChild {
                        at: 3.0,
                        node: NodeDef::clip("run"),
                    },
                ],
            },
        };
        let mut g = GraphInstance::compile(&graph, &s, &|n| lib.get(n).cloned()).unwrap();
        let mut params = AnimationParams::default();
        params.set("speed", 2.0);
        let mut trig = Vec::new();
        run(&mut g, &params, &mut trig, 0.4);
        // Blended duration 0.8 s → phase 0.5.
        match &g.nodes[g.root] {
            Node::Blend1d(b) => assert!((b.phase - 0.5).abs() < 1e-4, "{}", b.phase),
            _ => panic!(),
        }
    }

    #[test]
    fn missing_clip_is_a_readable_error() {
        let s = skel();
        let e = GraphInstance::compile(
            &AnimGraph {
                name: "x".into(),
                root: NodeDef::clip("nope"),
            },
            &s,
            &|_| None,
        )
        .err()
        .unwrap();
        assert!(e.contains("nope"), "{e}");
    }

    #[test]
    fn graph_round_trips_through_json() {
        let g = locomotion();
        let s = serde_json::to_string(&g).unwrap();
        let back: AnimGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(g, back);
        // The documented example parses.
        let doc = r#"{"name": "h", "root": {"type": "state_machine", "states": [
            {"name": "move", "node": {"type": "clip", "clip": "walk"}}],
            "transitions": [{"from": "*", "to": "move", "when": {"trigger": "x"}}]}}"#;
        let parsed: AnimGraph = serde_json::from_str(doc).unwrap();
        assert!(matches!(parsed.root, NodeDef::StateMachine { .. }));
    }
}
