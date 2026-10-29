//! Combat moves: attack sets with timed windows, an input buffer, combo links taken in cancel
//! windows, closing in on the target (motion warping) and hitstop.
//!
//! An [`AttackSet`] is serde data like a graph: each [`AttackMove`] plays a clip over the
//! character's graph ([`Animator::play_over`]) and carries [`Window`]s in seconds: `hit`
//! (the striking bone lands a [`DamageEvent`] on the target if it comes within reach, once
//! per window, and both fighters freeze for the hitstop), `cancel` (a buffered input that
//! matches one of the move's [`Link`]s starts the next move), `warp` (the attacker closes to
//! striking range and turns to face the target over the window, clamped so it never
//! teleports) and `armor` (published for the rules that decide whether a hit interrupts).
//!
//! An [`Attacker`] takes the `attack_light` / `attack_heavy` triggers (the
//! [`AnimationTrigger`](engine_world::animation::AnimationTrigger) contract) before the graph
//! sees them. Only the newest press counts, and it is kept for [`Attacker::buffer_secs`] (0.2
//! s), so a press a little early still links and mashing doesn't queue a whole combo. It
//! publishes what the move is doing ([`AttackPhase`], cancel and armor flags, the warp it
//! applied) for the module that owns the rules; damage numbers and who may hit whom stay
//! with the combat module. While a strike plays, two-bone IK pulls the striking hand onto
//! the target, weighted by the clip's `strike` curve (0 on the guard, 1 where it lands), so
//! blows land on the face in front of it rather than where the clip's author stood.
//! This is Unreal's notify-state model (montage windows, Motion
//! Warping) with For Honor's and Ghost of Tsushima's buffering.
//!
//! A target holding up its guard (the `guarding` parameter above 0.5) blocks high strikes
//! thrown from in front of it: the strike lands on the nearer glove instead of the face,
//! with [`Block`]'s share of the damage and push (the struck arm flinches back), a shorter
//! hitstop (blockstun is shorter than hitstun in fighting games), and the attacker is
//! pushed back a little ([`HitReaction::knock`]) as fighting games push a blocked attacker
//! out of reach. [`Attacker::blocked`] counts them for the rules.
//!
//! [`HitReaction::knock`]: crate::hit::HitReaction::knock

use std::collections::{HashMap, HashSet};

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::{DamageEvent, DamageKind};

use crate::Animator;
use crate::animator::{AnimationLibrary, AnimatorSettings};
use crate::build::humanoid::{Evasion, Punch};
use crate::hit::HitReaction;
use crate::hurt::Hurt;
use crate::ik::{to_model, two_bone};
use crate::knockdown::Knockdown;

/// What a window does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WindowKind {
    /// The `bone` lands on the target if it comes within `reach` meters of the target's
    /// chest (or head, for `high`, or liver, for `liver`).
    Hit {
        bone: String,
        reach: f32,
        damage: f32,
        /// Push at the hit point, N·s.
        impulse: f32,
        /// Seconds both fighters freeze when it lands.
        hitstop: f32,
        #[serde(default)]
        high: bool,
        /// Aimed at the target's liver (its own right side, under the ribs) instead.
        #[serde(default)]
        liver: bool,
    },
    /// A buffered input matching a link starts the linked move.
    Cancel,
    /// Close to `range` meters of the target and face it, moving at most `max_speed` m/s.
    Warp { range: f32, max_speed: f32 },
    /// Hyper armor: a hit doesn't interrupt (for the rules to read).
    Armor,
}

/// A stretch of a move, seconds into its clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    #[serde(flatten)]
    pub kind: WindowKind,
    pub start: f32,
    pub end: f32,
}

impl Window {
    /// Whether it was open at any time in the frame of `dt` seconds that ended at `t`: a long
    /// frame (a slow machine, a film at a few frames a second) can step over a short window.
    pub fn overlaps(&self, t: f32, dt: f32) -> bool {
        self.start <= t && self.end >= t - dt.max(0.0)
    }
}

/// Pressing `input` in a cancel window plays `to` next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub input: String,
    pub to: String,
}

/// One attack: a clip and its windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttackMove {
    pub name: String,
    pub clip: String,
    /// The input that starts it from neutral (`None`: only reached through a link).
    #[serde(default)]
    pub starts_on: Option<String>,
    #[serde(default = "default_fade_in")]
    pub fade_in: f32,
    #[serde(default = "default_fade_out")]
    pub fade_out: f32,
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub links: Vec<Link>,
    /// The evasion clip that makes it miss, for an [`Evader`] to play.
    #[serde(default)]
    pub evaded_by: Option<String>,
}

fn default_fade_in() -> f32 {
    0.08
}

fn default_fade_out() -> f32 {
    0.2
}

/// What a block lets through, and what it does to the attacker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Block {
    /// Share of the damage that gets through.
    pub damage: f32,
    /// Share of the push the guard takes.
    pub impulse: f32,
    /// Share of the hitstop.
    pub hitstop: f32,
    /// How far the attacker is pushed back, m.
    pub pushback: f32,
    /// Widest angle off the target's front a strike is still blocked from, degrees.
    pub cone_deg: f32,
}

impl Default for Block {
    fn default() -> Self {
        Self {
            damage: 0.2,
            impulse: 0.35,
            hitstop: 0.6,
            pushback: 0.12,
            cone_deg: 70.0,
        }
    }
}

/// A fighter's moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttackSet {
    pub name: String,
    pub moves: Vec<AttackMove>,
    /// What a guarding target's block does to these moves.
    #[serde(default)]
    pub block: Block,
}

impl AttackSet {
    pub fn get(&self, name: &str) -> Option<&AttackMove> {
        self.moves.iter().find(|m| m.name == name)
    }

    /// Boxing on the humanoid's punches: jab (light) or cross (heavy) from neutral; jab →
    /// cross → hook → uppercut on light presses; a heavy press after a jab digs a hook to the
    /// body (the liver, under a raised guard), which a light press follows up with the cross;
    /// a heavy press after the cross goes straight to the uppercut, and one after the hook
    /// throws `uppercut_heavy`, a finisher hard enough to knock a person down. Each steps in to its
    /// range (closer for hooks and uppercuts) and the uppercuts step back out to sparring
    /// distance.
    pub fn boxing() -> Self {
        let mv = |p: Punch, starts_on: Option<&str>, links: &[(&str, Punch)]| {
            let (secs, land) = p.timing();
            let at = secs * land;
            AttackMove {
                name: p.clip_name().into(),
                clip: p.clip_name().into(),
                starts_on: starts_on.map(Into::into),
                fade_in: 0.08,
                fade_out: 0.18,
                windows: vec![
                    Window {
                        kind: WindowKind::Warp {
                            // Hooks and uppercuts are thrown from closer in.
                            range: match p {
                                Punch::Jab => 0.78,
                                Punch::Cross => 0.74,
                                Punch::Hook => 0.62,
                                Punch::Uppercut => 0.58,
                                Punch::BodyHook => 0.56,
                            },
                            max_speed: 2.5,
                        },
                        start: 0.0,
                        end: at,
                    },
                    Window {
                        kind: WindowKind::Hit {
                            bone: if p.side() == 0 { "hand_l" } else { "hand_r" }.into(),
                            reach: 0.2,
                            damage: match p {
                                Punch::Jab => 6.0,
                                Punch::Cross => 10.0,
                                Punch::Hook => 14.0,
                                Punch::Uppercut => 16.0,
                                Punch::BodyHook => 14.0,
                            },
                            impulse: match p {
                                Punch::Jab => 18.0,
                                Punch::Cross => 30.0,
                                Punch::Hook => 38.0,
                                Punch::Uppercut => 42.0,
                                Punch::BodyHook => 40.0,
                            },
                            hitstop: match p {
                                Punch::Jab => 0.04,
                                Punch::Cross => 0.06,
                                Punch::Hook | Punch::Uppercut | Punch::BodyHook => 0.09,
                            },
                            high: p != Punch::BodyHook,
                            liver: p == Punch::BodyHook,
                        },
                        start: at - 0.06,
                        end: at + 0.05,
                    },
                    Window {
                        kind: WindowKind::Cancel,
                        start: at + 0.04,
                        end: secs * 0.85,
                    },
                ]
                .into_iter()
                // A finisher steps back out to sparring distance on its way back to the guard.
                .chain(links.is_empty().then_some(Window {
                    kind: WindowKind::Warp {
                        range: 1.0,
                        max_speed: 1.6,
                    },
                    start: at + 0.12,
                    end: secs,
                }))
                .collect(),
                links: links
                    .iter()
                    .map(|(input, to)| Link {
                        input: (*input).into(),
                        to: to.clip_name().into(),
                    })
                    .collect(),
                // Slip a straight punch to the outside, roll under a hook, pull back from an
                // uppercut; a body shot can't be slipped.
                evaded_by: match p {
                    Punch::Jab => Some(Evasion::SlipRight),
                    Punch::Cross => Some(Evasion::SlipLeft),
                    Punch::Hook => Some(Evasion::Roll),
                    Punch::Uppercut => Some(Evasion::PullBack),
                    Punch::BodyHook => None,
                }
                .map(|e| e.clip_name().to_string()),
            }
        };
        AttackSet {
            name: "boxing".into(),
            moves: vec![
                mv(
                    Punch::Jab,
                    Some("attack_light"),
                    &[
                        ("attack_light", Punch::Cross),
                        ("attack_heavy", Punch::BodyHook),
                    ],
                ),
                mv(
                    Punch::Cross,
                    Some("attack_heavy"),
                    &[
                        ("attack_light", Punch::Hook),
                        ("attack_heavy", Punch::Uppercut),
                    ],
                ),
                {
                    let mut hook = mv(Punch::Hook, None, &[("attack_light", Punch::Uppercut)]);
                    hook.links.push(Link {
                        input: "attack_heavy".into(),
                        to: "uppercut_heavy".into(),
                    });
                    hook
                },
                mv(Punch::Uppercut, None, &[]),
                // Under the guard to the liver, then back up to the head.
                mv(Punch::BodyHook, None, &[("attack_light", Punch::Cross)]),
                // Everything behind it: the same punch, hard enough to put a person down
                // (HitReaction::fall_beyond), freezing both for longer.
                {
                    let mut heavy = mv(Punch::Uppercut, None, &[]);
                    heavy.name = "uppercut_heavy".into();
                    for w in &mut heavy.windows {
                        if let WindowKind::Hit {
                            damage,
                            impulse,
                            hitstop,
                            ..
                        } = &mut w.kind
                        {
                            (*damage, *impulse, *hitstop) = (30.0, 95.0, 0.13);
                        }
                    }
                    heavy
                },
            ],
            block: Block::default(),
        }
    }
}

/// Attack sets by name.
#[derive(Resource, Debug, Clone)]
pub struct AttackSets(pub HashMap<String, AttackSet>);

impl Default for AttackSets {
    fn default() -> Self {
        let boxing = AttackSet::boxing();
        Self(HashMap::from([(boxing.name.clone(), boxing)]))
    }
}

/// Where a move is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttackPhase {
    #[default]
    Idle,
    /// Before its first hit window.
    Startup,
    /// In a hit window.
    Active,
    /// After its last hit window.
    Recovery,
}

/// A character that attacks with an [`AttackSet`].
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attacker {
    pub set: String,
    /// Who its hits land on and warps close to.
    #[serde(skip)]
    pub target: Option<Entity>,
    /// How long a press waits for a move to take it, seconds.
    pub buffer_secs: f32,
    /// Move its own `Transform` in warp windows. Off for characters whose movement module
    /// owns their transform: then [`Attacker::warp_delta`] says what the warp asked for.
    pub warp: bool,
    // What it is doing (published).
    pub current: Option<String>,
    /// Seconds into the current move.
    pub time: f32,
    pub phase: AttackPhase,
    pub cancel_open: bool,
    pub armored: bool,
    /// World-space move the last warp step asked for, meters.
    pub warp_delta: Vec3,
    /// Hits landed by the current move (blocked ones not counted).
    pub hits: u32,
    /// Strikes of the current move that landed on a guard.
    pub blocked: u32,
    /// Strikes of the current move whose hit window passed without landing (out of reach,
    /// or the target's head moved).
    pub missed: u32,
    /// The newest press waiting, and how long it has waited.
    #[serde(skip)]
    pub buffered: Option<(String, f32)>,
    /// Hit windows of the current move that have landed.
    #[serde(skip)]
    landed: Vec<usize>,
    /// Hit windows of the current move that passed without landing.
    #[serde(skip)]
    whiffed: Vec<usize>,
    /// Where the strike was committed to (world), from shortly before its hit window: a
    /// thrown fist doesn't follow the head.
    #[serde(skip)]
    committed: Option<DVec3>,
    /// The striking bone and the world point it reaches for this frame (on the target's
    /// face or chest), while a move with a hit window plays at a target.
    #[serde(skip)]
    pub aim: Option<(String, DVec3)>,
}

impl Default for Attacker {
    fn default() -> Self {
        Self {
            set: "boxing".into(),
            target: None,
            buffer_secs: 0.2,
            warp: true,
            current: None,
            time: 0.0,
            phase: AttackPhase::Idle,
            cancel_open: false,
            armored: false,
            warp_delta: Vec3::ZERO,
            hits: 0,
            blocked: 0,
            missed: 0,
            buffered: None,
            landed: Vec::new(),
            whiffed: Vec::new(),
            committed: None,
            aim: None,
        }
    }
}

impl Attacker {
    pub fn new(set: &str, target: Option<Entity>) -> Self {
        Self {
            set: set.into(),
            target,
            ..Default::default()
        }
    }
}

/// How far the middle of the face is above the head bone (the top of the neck), meters.
const FACE_ABOVE_HEAD: f64 = 0.07;

/// How long before its hit window a strike's aim stops following the target, s.
const COMMIT_SECS: f32 = 0.2;

/// How far to its own right of the spine a target's liver is, meters.
const LIVER_SIDE: f64 = 0.1;

/// How far short of the middle of the head or chest a strike aims, meters (about where the
/// face or the front of the ribs is, less the fist).
const AIM_SHORT: f64 = 0.1;

/// From the wrist (the hand bone) to the knuckles, along the forearm, meters.
const FIST_LEN: f32 = 0.08;

/// Shoulder to knuckles of the arm the warp ranges are tuned for (the generated humanoid's),
/// meters.
const REFERENCE_ARM: f32 = 0.62;

/// Shoulder to knuckles of the arm ending at `hand`, in the rest pose, meters.
fn arm_length(skeleton: &crate::skeleton::Skeleton, hand: usize) -> f32 {
    let parent = |b: usize| skeleton.parents[b].map(usize::from);
    let at = |b: usize| skeleton.rest_model[b].t;
    match (parent(hand), parent(hand).and_then(parent)) {
        (Some(lower), Some(upper)) => {
            (at(lower) - at(upper)).length() + (at(hand) - at(lower)).length() + FIST_LEN
        }
        _ => REFERENCE_ARM,
    }
}

/// Model-space knuckles of the hand `hand`: past the wrist along the forearm.
fn knuckles(animator: &Animator, hand: usize) -> Vec3 {
    let c = animator.model[hand].t;
    match animator.skeleton.parents[hand] {
        Some(p) => c + (c - animator.model[p as usize].t).normalize_or_zero() * FIST_LEN,
        None => c,
    }
}

/// World-space knuckles of the hand `hand`.
fn fist_of(animator: &Animator, gt: &GlobalTransform, hand: usize) -> DVec3 {
    crate::ik::to_world(gt, knuckles(animator, hand))
}

/// Pull striking hands onto their aim (`AnimSet::Ik`), weighted by the `strike` curve.
pub(crate) fn aim_strikes(mut q: Query<(&mut Animator, &Attacker, &GlobalTransform)>) {
    for (mut animator, attacker, gt) in &mut q {
        let Some((bone, aim)) = &attacker.aim else {
            continue;
        };
        let weight = animator.curve("strike");
        if !animator.posed() || weight <= 1e-3 {
            continue;
        }
        let s = animator.skeleton.clone();
        let Some(end) = s.index_of(bone) else {
            continue;
        };
        let parent = |b: usize| s.parents[b].map(usize::from);
        let (Some(lower), Some(upper)) = (parent(end), parent(end).and_then(parent)) else {
            continue;
        };
        animator.refresh_model();
        // Keep the elbow where the clip bends it (down for a jab, out for a hook).
        let (a, b, c) = (
            animator.model[upper].t,
            animator.model[lower].t,
            animator.model[end].t,
        );
        let pole = b - (a + c) * 0.5;
        // The wrist goes a fist's length short, so the knuckles land on the aim.
        let forearm = (c - b).normalize_or_zero();
        two_bone(
            &mut animator.edit(),
            upper,
            lower,
            end,
            to_model(gt, *aim) - forearm * FIST_LEN,
            pole,
            weight.min(1.0),
        );
    }
}

/// A landed blow, applied after the attackers have moved: freeze both (hitstop), and push
/// a blocked attacker back (world, m).
pub(crate) struct Landed {
    attacker: Entity,
    target: Entity,
    hitstop: f32,
    pushback: Vec3,
}

/// Where a target can be struck (world): its chest and face, and its guard if it is up.
#[derive(Clone, Copy)]
struct Target {
    chest: DVec3,
    head: DVec3,
    /// Under the ribs on its own right.
    liver: DVec3,
    guard: Option<Guard>,
}

/// A raised guard: which way the target faces (flat) and its knuckles (world).
#[derive(Clone, Copy)]
struct Guard {
    forward: DVec3,
    gloves: [DVec3; 2],
}

impl Target {
    /// The middle of what a strike aims at.
    fn aim(&self, high: bool, liver: bool) -> DVec3 {
        if liver {
            self.liver
        } else if high {
            self.head
        } else {
            self.chest
        }
    }

    /// The glove a high strike from `from` (flat on the ground, the attacker's position)
    /// with the fist now at `fist` lands on, if the guard is up and faces it.
    fn blocking_glove(&self, from: DVec3, fist: DVec3, cone_deg: f32) -> Option<DVec3> {
        let g = self.guard?;
        let to = (from - self.chest).with_y(0.0).normalize_or_zero();
        if g.forward.dot(to) < (cone_deg as f64).to_radians().cos() {
            return None;
        }
        g.gloves
            .into_iter()
            .min_by(|a, b| a.distance(fist).total_cmp(&b.distance(fist)))
    }
}

type AttackerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Attacker,
        &'static mut Animator,
        &'static mut Transform,
        &'static GlobalTransform,
        Option<&'static AnimatorSettings>,
        Option<&'static LocalTimeScale>,
    ),
>;

/// Take presses, run moves and their windows, warp, land hits (in `AnimSet::Setup`, after
/// triggers are queued and before the graph runs).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn run_attacks(
    time: Res<Time>,
    sets: Option<Res<AttackSets>>,
    library: Res<AnimationLibrary>,
    mut set: ParamSet<(
        AttackerQuery,
        Query<(
            &GlobalTransform,
            Option<&Animator>,
            Option<&AnimationParams>,
            Option<&Knockdown>,
            Option<&Hurt>,
            Option<&Attacker>,
            Option<&Evader>,
        )>,
        Query<(&mut Animator, Option<&mut HitReaction>)>,
    )>,
    mut damage: MessageWriter<DamageEvent>,
    mut landed: Local<Vec<Landed>>,
) {
    let Some(sets) = sets else {
        return;
    };
    // Where targets are: chest and head height and the guard, world space.
    let mut targets: HashMap<Entity, Target> = HashMap::new();
    let mut downed: HashSet<Entity> = HashSet::new();
    {
        let attackers: Vec<Entity> = set.p0().iter().filter_map(|(_, a, ..)| a.target).collect();
        let q = set.p1();
        for t in attackers {
            let Ok((gt, animator, params, down, hurt, striking, evader)) = q.get(t) else {
                continue;
            };
            // Knocked down or on a knee: nothing to strike until it is up again.
            if down.is_some() || hurt.is_some_and(Hurt::is_down) {
                downed.insert(t);
                continue;
            }
            let at = |bone: &str, up: f64| -> DVec3 {
                animator
                    .and_then(|a| a.skeleton.index_of(bone).map(|b| a.bone_world(gt, b).0))
                    .unwrap_or_else(|| gt.translation() + DVec3::Y * up)
            };
            // Too hurt to keep the hands up, or throwing a punch of its own (the opening a
            // counter-puncher waits for), it doesn't block.
            let guarding = params.is_some_and(|p| p.get("guarding") > 0.5)
                && !hurt.is_some_and(Hurt::guard_down)
                && !striking.is_some_and(|a| a.current.is_some())
                // Moving its head out of the way instead.
                && !evader.is_some_and(|e| e.evading.is_some());
            let guard = animator.filter(|_| guarding).and_then(|a| {
                let glove = |hand: &str| Some(fist_of(a, gt, a.skeleton.index_of(hand)?));
                Some(Guard {
                    forward: (gt.rotation() * Vec3::NEG_Z)
                        .with_y(0.0)
                        .normalize()
                        .as_dvec3(),
                    gloves: [glove("hand_l")?, glove("hand_r")?],
                })
            });
            // The head bone sits at the top of the neck: the face is a little above it.
            targets.insert(
                t,
                Target {
                    chest: at("chest", 1.3),
                    head: at("head", 1.55) + DVec3::Y * FACE_ABOVE_HEAD,
                    liver: at("spine", 1.07) + (gt.rotation() * Vec3::X).as_dvec3() * LIVER_SIDE,
                    guard,
                },
            );
        }
    }
    landed.clear();
    for (entity, mut attacker, mut animator, mut transform, gt, settings, local) in &mut set.p0() {
        let speed = settings.map_or(1.0, |s| if s.paused { 0.0 } else { s.speed });
        let dt = if animator.frozen() > 0.0 {
            0.0
        } else {
            time.delta_for(local) * speed
        };
        let attacker = &mut *attacker;
        attacker.warp_delta = Vec3::ZERO;
        attacker.aim = None;
        // Presses: the newest one waits in the buffer.
        let mut pressed = None;
        animator.triggers.retain(|(name, _)| {
            if name.starts_with("attack_") {
                pressed = Some(name.clone());
                false
            } else {
                true
            }
        });
        // An older press ages first, so a new one always gets this frame to start in, however
        // long the frame (at 4 fps a frame outlasts the buffer).
        if let Some(b) = &mut attacker.buffered {
            b.1 += dt;
            if b.1 > attacker.buffer_secs {
                attacker.buffered = None;
            }
        }
        if let Some(p) = pressed {
            attacker.buffered = Some((p, 0.0));
        }
        // No pressing on a target that is down.
        if attacker.target.is_some_and(|t| downed.contains(&t)) {
            attacker.buffered = None;
        }
        let Some(moves) = sets.0.get(&attacker.set) else {
            continue;
        };
        // Advance the move playing; at its end it is over (the overlay fades itself).
        if let Some(name) = attacker.current.clone() {
            attacker.time += dt;
            let over = moves
                .get(&name)
                .and_then(|m| library.clip(&m.clip))
                .is_none_or(|c| attacker.time >= c.duration);
            if over {
                attacker.current = None;
            }
        }
        // A press starts a move from neutral, or a linked one in a cancel window.
        let next = attacker.buffered.as_ref().and_then(|(input, _)| {
            match attacker.current.as_ref().and_then(|c| moves.get(c)) {
                None => moves
                    .moves
                    .iter()
                    .find(|m| m.starts_on.as_deref() == Some(input.as_str()))
                    .map(|m| m.name.clone()),
                Some(m) => {
                    let open = m
                        .windows
                        .iter()
                        .any(|w| w.kind == WindowKind::Cancel && w.overlaps(attacker.time, dt));
                    open.then(|| m.links.iter().find(|l| &l.input == input))
                        .flatten()
                        .map(|l| l.to.clone())
                }
            }
        });
        if let Some(name) = next
            && let Some(m) = moves.get(&name)
            && let Some(clip) = library.clip(&m.clip)
        {
            animator.play_over(clip, m.fade_in, m.fade_out);
            attacker.current = Some(name);
            attacker.time = 0.0;
            attacker.hits = 0;
            attacker.blocked = 0;
            attacker.missed = 0;
            attacker.landed.clear();
            attacker.whiffed.clear();
            attacker.committed = None;
            attacker.buffered = None;
        }
        // Windows.
        attacker.cancel_open = false;
        attacker.armored = false;
        attacker.phase = AttackPhase::Idle;
        let Some(m) = attacker.current.as_ref().and_then(|c| moves.get(c)) else {
            continue;
        };
        let t = attacker.time;
        let first_hit = m
            .windows
            .iter()
            .filter(|w| matches!(w.kind, WindowKind::Hit { .. }))
            .map(|w| w.start)
            .fold(f32::INFINITY, f32::min);
        let last_hit = m
            .windows
            .iter()
            .filter(|w| matches!(w.kind, WindowKind::Hit { .. }))
            .map(|w| w.end)
            .fold(f32::NEG_INFINITY, f32::max);
        attacker.phase = if t < first_hit {
            AttackPhase::Startup
        } else if t <= last_hit {
            AttackPhase::Active
        } else {
            AttackPhase::Recovery
        };
        let target = attacker
            .target
            .and_then(|e| targets.get(&e).map(|p| (e, *p)));
        // Aim the strike: the front of the face (or chest), a fist's width short of its
        // middle on the attacker's side, or the glove of a guard that blocks it.
        let mut blocked_at = None;
        if let Some((_, tg)) = target
            && let Some(WindowKind::Hit {
                bone, high, liver, ..
            }) = m
                .windows
                .iter()
                .map(|w| &w.kind)
                .find(|k| matches!(k, WindowKind::Hit { .. }))
        {
            let mid = tg.aim(*high, *liver);
            let mut back = gt.translation() - mid;
            back.y = 0.0;
            let back = back.normalize_or_zero();
            let fist = animator
                .skeleton
                .index_of(bone)
                .map_or(mid, |b| fist_of(&animator, gt, b));
            blocked_at = high
                .then(|| tg.blocking_glove(gt.translation(), fist, moves.block.cone_deg))
                .flatten();
            let mut aim = match blocked_at {
                // The front of the glove.
                Some(glove) => glove + back * 0.04,
                None => mid + back * AIM_SHORT,
            };
            // Shortly before the hit the strike is committed: from then on the fist goes where
            // the target was, so a head that slips away is missed.
            if t >= first_hit - COMMIT_SECS {
                aim = *attacker.committed.get_or_insert(aim);
            }
            attacker.aim = Some((bone.clone(), aim));
        }
        for (k, w) in m.windows.iter().enumerate() {
            // A hit window a long frame stepped right over still lands (the fist is still
            // close, its aim fading out); the others act only while they are open.
            let open = (w.start..=w.end).contains(&t);
            let hit = matches!(w.kind, WindowKind::Hit { .. });
            if !open && !(hit && w.overlaps(t, dt)) {
                continue;
            }
            match &w.kind {
                WindowKind::Cancel => attacker.cancel_open = true,
                WindowKind::Armor => attacker.armored = true,
                WindowKind::Warp { range, max_speed } => {
                    let Some((_, Target { chest, .. })) = target else {
                        continue;
                    };
                    let here = gt.translation();
                    let mut to = chest - here;
                    to.y = 0.0;
                    let dist = to.length();
                    if dist < 1e-3 {
                        continue;
                    }
                    let dir = to / dist;
                    // Ranges are for an arm of REFERENCE_ARM: longer or shorter arms strike
                    // from farther or closer.
                    let range = m
                        .windows
                        .iter()
                        .find_map(|w| match &w.kind {
                            WindowKind::Hit { bone, .. } => animator.skeleton.index_of(bone),
                            _ => None,
                        })
                        .map_or(*range, |hand| {
                            *range * (arm_length(&animator.skeleton, hand) / REFERENCE_ARM)
                        });
                    let range = &range;
                    // Spread what's left of the gap over what's left of the window.
                    let left = (w.end - t).max(dt).max(1e-3);
                    let share = (dt / left).min(1.0) as f64;
                    let step = ((dist - *range as f64) * share)
                        .clamp(-(*max_speed * dt) as f64, (*max_speed * dt) as f64);
                    let delta = dir * step;
                    attacker.warp_delta = delta.as_vec3();
                    if attacker.warp {
                        transform.translation += delta;
                        // Face it (the rig faces −Z).
                        let want = Quat::from_rotation_y((-dir.x).atan2(-dir.z) as f32);
                        transform.rotation = transform.rotation.slerp(want, share as f32);
                    }
                }
                WindowKind::Hit {
                    bone,
                    reach,
                    damage: amount,
                    impulse,
                    hitstop,
                    high,
                    liver,
                } => {
                    let Some((victim, tg)) = target else {
                        continue;
                    };
                    if attacker.landed.contains(&k) {
                        continue;
                    }
                    let Some(b) = animator.skeleton.index_of(bone) else {
                        continue;
                    };
                    let fist = fist_of(&animator, gt, b);
                    let aim = blocked_at.unwrap_or(tg.aim(*high, *liver));
                    if fist.distance(aim) > *reach as f64 {
                        continue;
                    }
                    attacker.landed.push(k);
                    let mut push = (aim - gt.translation()).as_vec3();
                    push.y = 0.0;
                    let push = push.normalize_or_zero();
                    // A guard takes a share of it, and the attacker bounces off.
                    let block = &moves.block;
                    let (share, stop, pushback) = if blocked_at.is_some() {
                        attacker.blocked += 1;
                        (
                            (block.damage, block.impulse),
                            block.hitstop,
                            -push * block.pushback,
                        )
                    } else {
                        attacker.hits += 1;
                        ((1.0, 1.0), 1.0, Vec3::ZERO)
                    };
                    damage.write(DamageEvent {
                        target: victim,
                        amount: *amount * share.0,
                        kind: DamageKind::Blunt,
                        // Where it lands on them: the front of the face, chest or glove.
                        point: attacker.aim.as_ref().map_or(fist, |a| a.1),
                        impulse: push * *impulse * share.1,
                        source: Some(entity),
                    });
                    landed.push(Landed {
                        attacker: entity,
                        target: victim,
                        hitstop: *hitstop * stop,
                        pushback,
                    });
                }
            }
        }
        // Strikes at a target whose window has passed without landing missed.
        if target.is_some() {
            for (k, w) in m.windows.iter().enumerate() {
                if matches!(w.kind, WindowKind::Hit { .. })
                    && w.end < t
                    && !attacker.landed.contains(&k)
                    && !attacker.whiffed.contains(&k)
                {
                    attacker.whiffed.push(k);
                    attacker.missed += 1;
                }
            }
        }
    }
    // Hitstop: both fighters hold still for a few frames; a blocked attacker is pushed
    // back (stepping, where it has a hit reaction).
    let mut animators = set.p2();
    for l in landed.drain(..) {
        for e in [l.attacker, l.target] {
            if let Ok((mut a, reaction)) = animators.get_mut(e) {
                a.freeze_for(l.hitstop);
                if e == l.attacker
                    && l.pushback != Vec3::ZERO
                    && let Some(mut r) = reaction
                {
                    r.knock(l.pushback, 0.12);
                }
            }
        }
    }
}

/// Moves its head out of the way of strikes aimed at it (slips, rolls, pull-backs): when a
/// move at it is within [`Evader::read_secs`] of its first hit window, it evades with chance
/// [`Evader::skill`] by playing the move's [`AttackMove::evaded_by`] clip over its graph.
/// The strike's aim has committed by then, so the fist goes where the head was. While its
/// `AnimationParams` have `evasive`, it evades only when that is above 0.5. Out of the
/// evasions in [`Evader::counter_after`], one with its own [`Attacker`] throws
/// `attack_heavy` back at the attacker halfway through.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Evader {
    /// Chance of evading each strike it reads (0..1).
    pub skill: f32,
    /// How long before a strike's hit window it moves, s: later and the fist lands first.
    pub read_secs: f32,
    /// Evasions it counters out of.
    pub counter_after: Vec<String>,
    /// The evasion playing (clip, seconds in, its length, who it is evading, countered yet).
    #[serde(skip)]
    pub evading: Option<(String, f32, f32, Entity, bool)>,
    /// The strike last seen from each attacker (its move, and how far into it), so each is
    /// read once.
    #[serde(skip)]
    read: Vec<(Entity, String, f32)>,
    /// Strikes read in total (varies the dice).
    #[serde(skip)]
    count: u32,
}

impl Default for Evader {
    fn default() -> Self {
        Self {
            skill: 0.8,
            read_secs: 0.18,
            counter_after: vec![
                Evasion::Roll.clip_name().into(),
                Evasion::PullBack.clip_name().into(),
            ],
            evading: None,
            read: Vec::new(),
            count: 0,
        }
    }
}

impl Evader {
    pub fn new(skill: f32) -> Self {
        Self {
            skill,
            ..Default::default()
        }
    }
}

/// Evaders read strikes coming at them and move their heads (in `AnimSet::Setup`, after the
/// attacks have run).
#[allow(clippy::type_complexity)]
pub(crate) fn evade_strikes(
    time: Res<Time>,
    sets: Option<Res<AttackSets>>,
    library: Res<AnimationLibrary>,
    mut set: ParamSet<(
        Query<(Entity, &Attacker)>,
        Query<(
            Entity,
            &mut Evader,
            &mut Animator,
            Option<&mut Attacker>,
            Option<&AnimationParams>,
            Option<&LocalTimeScale>,
        )>,
    )>,
) {
    let Some(sets) = sets else {
        return;
    };
    // Strikes on their way: (attacker, target, move, seconds into it, seconds to its hit
    // window).
    let coming: Vec<(Entity, Entity, String, f32, f32)> = set
        .p0()
        .iter()
        .filter_map(|(e, a)| {
            let target = a.target?;
            let name = a.current.clone()?;
            let m = sets.0.get(&a.set)?.get(&name)?;
            let hit = m
                .windows
                .iter()
                .filter(|w| matches!(w.kind, WindowKind::Hit { .. }))
                .map(|w| w.start)
                .fold(f32::INFINITY, f32::min);
            (hit.is_finite() && a.time < hit && a.missed == 0 && a.hits == 0).then_some((
                e,
                target,
                name,
                a.time,
                hit - a.time,
            ))
        })
        .collect();
    for (me, mut evader, mut animator, mut attacker, params, local) in &mut set.p1() {
        let evader = &mut *evader;
        let dt = if animator.frozen() > 0.0 {
            0.0
        } else {
            time.delta_for(local)
        };
        // The evasion under way, and a counter out of it.
        if let Some((clip, t, secs, from, countered)) = &mut evader.evading {
            *t += dt;
            if !*countered && *t >= *secs * 0.5 && evader.counter_after.contains(clip) {
                *countered = true;
                if let Some(a) = attacker.as_mut()
                    && a.current.is_none()
                {
                    a.target.get_or_insert(*from);
                    a.buffered = Some(("attack_heavy".into(), 0.0));
                }
            }
            if *t >= *secs {
                evader.evading = None;
            }
        }
        // A new strike can be read mid-evasion: slip, then slip back the other way.
        let gate = params
            .is_none_or(|p| !p.values.contains_key("evasive") || p.get("evasive") > 0.5)
            && attacker.as_ref().is_none_or(|a| a.current.is_none());
        for (from, target, name, at, left) in &coming {
            if *target != me || *left > evader.read_secs {
                continue;
            }
            // Read already, unless the attacker has started a move since (the same one again
            // starts over from 0).
            let seen = evader
                .read
                .iter()
                .any(|r| r.0 == *from && r.1 == *name && r.2 <= *at);
            evader.read.retain(|r| r.0 != *from);
            evader.read.push((*from, name.clone(), *at));
            if seen {
                continue;
            }
            evader.count += 1;
            // The same dice every run: spread by who it is and how many it has read.
            let roll = ((me.to_bits() as u32)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(evader.count.wrapping_mul(40_503))
                >> 8)
                % 1000;
            if !gate || roll as f32 >= evader.skill * 1000.0 {
                continue;
            }
            let Some(m) = sets
                .0
                .values()
                .find_map(|s| s.get(name))
                .and_then(|m| m.evaded_by.clone())
            else {
                continue;
            };
            let Some(clip) = library.clip(&m) else {
                continue;
            };
            let secs = clip.duration;
            animator.play_over(clip, 0.05, 0.15);
            evader.evading = Some((m, 0.0, secs, *from, false));
            break;
        }
    }
}
