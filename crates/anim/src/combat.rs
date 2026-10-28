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

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::{DamageEvent, DamageKind};

use crate::Animator;
use crate::animator::{AnimationLibrary, AnimatorSettings};
use crate::build::humanoid::Punch;
use crate::ik::{to_model, two_bone};

/// What a window does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WindowKind {
    /// The `bone` lands on the target if it comes within `reach` meters of the target's
    /// chest (or head, for `high`).
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
}

fn default_fade_in() -> f32 {
    0.08
}

fn default_fade_out() -> f32 {
    0.2
}

/// A fighter's moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttackSet {
    pub name: String,
    pub moves: Vec<AttackMove>,
}

impl AttackSet {
    pub fn get(&self, name: &str) -> Option<&AttackMove> {
        self.moves.iter().find(|m| m.name == name)
    }

    /// Boxing on the humanoid's punches: jab (light) or cross (heavy) from neutral; jab →
    /// cross → hook → uppercut on light presses, a heavy press after a jab or cross goes
    /// straight to the hook or the uppercut. Each steps in to its range (closer for hooks
    /// and uppercuts) and the uppercut steps back out to sparring distance.
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
                            },
                            impulse: match p {
                                Punch::Jab => 18.0,
                                Punch::Cross => 30.0,
                                Punch::Hook => 38.0,
                                Punch::Uppercut => 42.0,
                            },
                            hitstop: match p {
                                Punch::Jab => 0.04,
                                Punch::Cross => 0.06,
                                Punch::Hook | Punch::Uppercut => 0.09,
                            },
                            high: true,
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
                        ("attack_heavy", Punch::Hook),
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
                mv(Punch::Hook, None, &[("attack_light", Punch::Uppercut)]),
                mv(Punch::Uppercut, None, &[]),
            ],
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
    /// Hits landed by the current move.
    pub hits: u32,
    /// The newest press waiting, and how long it has waited.
    #[serde(skip)]
    pub buffered: Option<(String, f32)>,
    /// Hit windows of the current move that have landed.
    #[serde(skip)]
    landed: Vec<usize>,
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
            buffered: None,
            landed: Vec::new(),
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

/// A landed blow, applied after the attackers have moved: freeze both (hitstop).
pub(crate) struct Landed {
    attacker: Entity,
    target: Entity,
    hitstop: f32,
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
        Query<(&GlobalTransform, Option<&Animator>)>,
        Query<&mut Animator>,
    )>,
    mut damage: MessageWriter<DamageEvent>,
    mut landed: Local<Vec<Landed>>,
) {
    let Some(sets) = sets else {
        return;
    };
    // Where targets are: chest and head height, world space.
    let mut targets: HashMap<Entity, (DVec3, DVec3)> = HashMap::new();
    {
        let attackers: Vec<Entity> = set.p0().iter().filter_map(|(_, a, ..)| a.target).collect();
        let q = set.p1();
        for t in attackers {
            let Ok((gt, animator)) = q.get(t) else {
                continue;
            };
            let at = |bone: &str, up: f64| -> DVec3 {
                animator
                    .and_then(|a| a.skeleton.index_of(bone).map(|b| a.bone_world(gt, b).0))
                    .unwrap_or_else(|| gt.translation() + DVec3::Y * up)
            };
            // The head bone sits at the top of the neck: the face is a little above it.
            targets.insert(
                t,
                (
                    at("chest", 1.3),
                    at("head", 1.55) + DVec3::Y * FACE_ABOVE_HEAD,
                ),
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
        if let Some(p) = pressed {
            attacker.buffered = Some((p, 0.0));
        }
        if let Some(b) = &mut attacker.buffered {
            b.1 += dt;
            if b.1 > attacker.buffer_secs {
                attacker.buffered = None;
            }
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
                    let open = m.windows.iter().any(|w| {
                        w.kind == WindowKind::Cancel && (w.start..=w.end).contains(&attacker.time)
                    });
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
            attacker.landed.clear();
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
        // middle on the attacker's side.
        if let Some((_, (chest, head))) = target
            && let Some(WindowKind::Hit { bone, high, .. }) = m
                .windows
                .iter()
                .map(|w| &w.kind)
                .find(|k| matches!(k, WindowKind::Hit { .. }))
        {
            let mid = if *high { head } else { chest };
            let mut back = gt.translation() - mid;
            back.y = 0.0;
            attacker.aim = Some((bone.clone(), mid + back.normalize_or_zero() * AIM_SHORT));
        }
        for (k, w) in m.windows.iter().enumerate() {
            if !(w.start..=w.end).contains(&t) {
                continue;
            }
            match &w.kind {
                WindowKind::Cancel => attacker.cancel_open = true,
                WindowKind::Armor => attacker.armored = true,
                WindowKind::Warp { range, max_speed } => {
                    let Some((_, (chest, _))) = target else {
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
                } => {
                    let Some((victim, (chest, head))) = target else {
                        continue;
                    };
                    if attacker.landed.contains(&k) {
                        continue;
                    }
                    let Some(b) = animator.skeleton.index_of(bone) else {
                        continue;
                    };
                    let fist = fist_of(&animator, gt, b);
                    let aim = if *high { head } else { chest };
                    if fist.distance(aim) > *reach as f64 {
                        continue;
                    }
                    attacker.landed.push(k);
                    attacker.hits += 1;
                    let mut push = (aim - gt.translation()).as_vec3();
                    push.y = 0.0;
                    damage.write(DamageEvent {
                        target: victim,
                        amount: *amount,
                        kind: DamageKind::Blunt,
                        // Where it lands on them: the front of the face or chest.
                        point: attacker.aim.as_ref().map_or(fist, |a| a.1),
                        impulse: push.normalize_or_zero() * *impulse,
                        source: Some(entity),
                    });
                    landed.push(Landed {
                        attacker: entity,
                        target: victim,
                        hitstop: *hitstop,
                    });
                }
            }
        }
    }
    // Hitstop: both fighters hold still for a few frames.
    let mut animators = set.p2();
    for l in landed.drain(..) {
        for e in [l.attacker, l.target] {
            if let Ok(mut a) = animators.get_mut(e) {
                a.freeze_for(l.hitstop);
            }
        }
    }
}
