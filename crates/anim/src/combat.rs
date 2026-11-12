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
//! Weapons ([`AttackSet::sword_shield`], [`AttackSet::greatsword`], [`AttackSet::bow`]): a
//! hit window with a [`Striker`] lands when that segment (a blade from hilt to tip, a
//! shield's face) swept from last frame's place to this frame's comes within reach of the
//! target (a fast cut moves farther in a frame than its own width), pushes along the blade's
//! motion and does the striker's kind of damage. A target with a [`Shield`] that is guarding
//! (`guarding` or `shield_guard`) blocks at the shield: low strikes as well as high, from a
//! wider cone, letting through what the shield's [`Block`] says. A weapon that lands on a
//! guard bounces back up its arc and the move ends ([`Attacker::recoil`]); a guard raised
//! just before it lands deflects it instead, and a blow that `breaks_guard` flings the guard
//! open ([`Defense`]).
//!
//! [`HitReaction::knock`]: crate::hit::HitReaction::knock

use std::collections::{HashMap, HashSet};

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::combat::{BodyZone, ItemClass, Loadout, MeleeContact, MeleeJudge};
use engine_world::{DamageEvent, DamageKind};

use crate::Animator;
use crate::animator::{AnimationLibrary, AnimatorSettings};
use crate::build::humanoid::{Approach as Travel, Evasion, Punch};
use crate::build::weapons::{self, WeaponMove};
use crate::hit::HitReaction;
use crate::hurt::Hurt;
use crate::ik::to_model;
use crate::knockdown::Knockdown;
use crate::paired::Riposted;

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
        /// What strikes, when it isn't the fist: a blade or a shield's face on `bone`.
        #[serde(default)]
        striker: Option<Striker>,
        /// Heavy enough to break a guard it lands on (see [`Defense::broken`]).
        #[serde(default)]
        breaks_guard: bool,
    },
    /// A buffered input matching a link starts the linked move.
    Cancel,
    /// Close to `range` meters of the target and face it, moving at most `max_speed` m/s.
    /// With `ease` (an approach's travel), the body speeds up and slows down over the window
    /// instead of spreading the gap evenly, and has turned to face the target by 40% of it.
    Warp {
        range: f32,
        max_speed: f32,
        #[serde(default)]
        ease: bool,
    },
    /// Hyper armor: a hit doesn't interrupt (for the rules to read).
    Armor,
}

/// A striking segment in a bone's frame (a sword from hilt to tip, a shield's face across
/// its middle), and the kind of damage it does.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Striker {
    pub from: Vec3,
    pub to: Vec3,
    #[serde(default)]
    pub kind: DamageKind,
}

impl Striker {
    /// A sword's blade in the right hand.
    pub fn sword(kind: DamageKind) -> Self {
        Self::blade(weapons::SWORD_BLADE, kind)
    }

    /// A great sword's blade in the right hand.
    pub fn greatsword(kind: DamageKind) -> Self {
        Self::blade(weapons::GREATSWORD_BLADE, kind)
    }

    fn blade([from, to]: [f32; 2], kind: DamageKind) -> Self {
        Self {
            from: weapons::GRIP + weapons::BLADE_AXIS * from,
            to: weapons::GRIP + weapons::BLADE_AXIS * to,
            kind,
        }
    }

    /// A round shield's face in the left hand, across its middle along the handle.
    pub fn shield() -> Self {
        let across = weapons::HANDLE_AXIS * weapons::SHIELD_RADIUS * 0.8;
        Self {
            from: weapons::SHIELD_CENTER - across,
            to: weapons::SHIELD_CENTER + across,
            kind: DamageKind::Blunt,
        }
    }
}

/// A shield on a character's arm, for blocking: guarding, it blocks strikes at its face
/// instead of on the gloves, low ones as well as high, letting through what its `block`
/// says (the attacker's set decides for a block on the gloves).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shield {
    /// The bone it is held by, and the middle of its face in that bone's frame.
    pub bone: String,
    pub center: Vec3,
    pub radius: f32,
    pub block: Block,
}

impl Default for Shield {
    fn default() -> Self {
        Self {
            bone: "hand_l".into(),
            center: weapons::SHIELD_CENTER,
            radius: weapons::SHIELD_RADIUS,
            // A shield takes nearly all of it and bounces the attacker off harder than a
            // glove, from almost square on to the side.
            block: Block {
                damage: 0.05,
                impulse: 0.3,
                hitstop: 0.8,
                pushback: 0.2,
                cone_deg: 80.0,
            },
        }
    }
}

/// What a target's guard is doing, for the rules and for its own moves (published; put on
/// anything an [`Attacker`] goes for). A guard raised just before a weapon's blow lands
/// deflects it (Sekiro, Elden Ring's parry): no damage, and the attacker staggers open. A
/// guard that came down only a moment before gets a much shorter window, so mashing it
/// doesn't deflect everything. A blocked blow leaves the defender in shield-stun, unable to
/// start a move, and a blow that breaks guards (a great sword's overhead, a shield bash)
/// knocks the guard aside for [`Defense::broken`] seconds, whatever its parameters say.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defense {
    /// Seconds the guard has been up (0 while it is down).
    pub up_secs: f32,
    /// Seconds since it last came down.
    pub down_secs: f32,
    /// How soon after rising a blow had to land to be deflected, s (set as it rises).
    pub deflect_window: f32,
    /// Seconds of shield-stun left: no new move until it is over.
    pub stun: f32,
    /// Seconds left with the guard broken: it doesn't block.
    pub broken: f32,
    /// Blows taken on the guard, deflected, and that broke it.
    pub blocks: u32,
    pub deflects: u32,
    pub breaks: u32,
}

impl Default for Defense {
    fn default() -> Self {
        Self {
            up_secs: 0.0,
            // Never raised yet: as if it had been down a long while.
            down_secs: 10.0,
            deflect_window: DEFLECT_SECS,
            stun: 0.0,
            broken: 0.0,
            blocks: 0,
            deflects: 0,
            breaks: 0,
        }
    }
}

impl Defense {
    /// Whether it can start a move (not in shield-stun or staggering from a broken guard).
    pub fn ready(&self) -> bool {
        self.stun <= 0.0 && self.broken <= 0.0
    }

    /// Keep time with the guard (`up`: its parameters hold it up and nothing stops it).
    fn tick(&mut self, up: bool, dt: f32) {
        self.stun = (self.stun - dt).max(0.0);
        self.broken = (self.broken - dt).max(0.0);
        let up = up && self.broken <= 0.0;
        if up {
            if self.up_secs <= 0.0 {
                self.deflect_window = if self.down_secs < SPAM_SECS {
                    DEFLECT_SPAM_SECS
                } else {
                    DEFLECT_SECS
                };
            }
            self.up_secs += dt.max(1e-4);
        } else {
            if self.up_secs > 0.0 {
                self.down_secs = 0.0;
            }
            self.up_secs = 0.0;
            self.down_secs += dt;
        }
    }

    /// Whether a blow landing now is deflected.
    fn deflects_now(&self) -> bool {
        self.up_secs > 0.0 && self.up_secs <= self.deflect_window
    }
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
    /// A paired move (a riposte) played with its victim: started instead of the set's other
    /// moves on its input while the target is open.
    #[serde(default)]
    pub paired: Option<Pairing>,
    /// The line an aimed cut or stab travels: a set with lines picks one by the aimed zone
    /// on a light press (see [`Attacker::zone`]).
    #[serde(default)]
    pub line: Option<Line>,
}

/// The line of an aimed cut, or a stab: which way the blade travels across the foe (degrees
/// round it as the attacker sees it, [`weapons::LINE_ANGLES`]) and at what height.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub angle: f32,
    pub height: weapons::Height,
    pub stab: bool,
}

impl Line {
    /// Which way the blade moves across the foe (model space, the attacker facing −Z), none
    /// for a stab.
    pub fn travel(&self) -> Vec3 {
        if self.stab {
            Vec3::ZERO
        } else {
            weapons::line_travel(self.angle)
        }
    }
}

/// How a target was opened up for a paired move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opening {
    /// Its blade was deflected ([`Attacker::open`]).
    Deflected,
    /// Its guard was broken ([`Defense::broken`]).
    GuardBroken,
}

/// The victim's side of a paired move and where it stands: the victim is moved onto its spot
/// `distance` ahead of the attacker, facing it, over `align_secs`, then held there and plays
/// `victim_clip` on the attacker's clock (both start the same frame). The move's hit windows
/// land without a reach test and can't be blocked. It starts on its input (from neutral)
/// instead of the set's other moves when the target is open in one of the ways in `after`,
/// within `max_distance` meters and `max_angle` degrees of the attacker's front.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pairing {
    pub victim_clip: String,
    pub distance: f32,
    pub after: Vec<Opening>,
    #[serde(default = "default_pair_reach")]
    pub max_distance: f32,
    #[serde(default = "default_pair_angle")]
    pub max_angle: f32,
    #[serde(default = "default_pair_align")]
    pub align_secs: f32,
}

fn default_pair_reach() -> f32 {
    2.4
}

fn default_pair_angle() -> f32 {
    60.0
}

fn default_pair_align() -> f32 {
    0.25
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

/// A travel move that closes a long gap before a strike (a lunge, a leap): a strike pressed
/// at a target more than `beyond` meters past its range starts this move instead (of those
/// that fit, the one for the longest gap), and the strike follows it in its cancel window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approach {
    /// The move (in the same set).
    pub name: String,
    pub beyond: f32,
}

/// A fighter's moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttackSet {
    pub name: String,
    pub moves: Vec<AttackMove>,
    /// What a guarding target's block does to these moves.
    #[serde(default)]
    pub block: Block,
    /// Moves that close long gaps before a strike.
    #[serde(default)]
    pub approaches: Vec<Approach>,
}

impl AttackSet {
    pub fn get(&self, name: &str) -> Option<&AttackMove> {
        self.moves.iter().find(|m| m.name == name)
    }

    /// Whether `name` is one of the set's approaches.
    pub fn is_approach(&self, name: &str) -> bool {
        self.approaches.iter().any(|a| a.name == name)
    }

    /// The approach for a target `gap` meters past a strike's range, if it needs one.
    pub fn approach_for(&self, gap: f32) -> Option<&Approach> {
        self.approaches
            .iter()
            .filter(|a| gap > a.beyond)
            .max_by(|a, b| a.beyond.total_cmp(&b.beyond))
    }

    /// Boxing on the humanoid's punches: jab (light) or cross (heavy) from neutral; jab →
    /// cross → hook → uppercut on light presses; a heavy press after a jab digs a hook to the
    /// body (the liver, under a raised guard), which a light press follows up with the cross;
    /// a heavy press after the cross goes straight to the uppercut, and one after the hook
    /// throws `uppercut_heavy`, a finisher hard enough to knock a person down. Each steps in to its
    /// range (closer for hooks and uppercuts) and the uppercuts step back out to sparring
    /// distance. A target more than a meter past a punch's range is closed on first with a
    /// `lunge`, and one more than 2.6 m past it with a `leap`.
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
                            ease: false,
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
                            striker: None,
                            breaks_guard: false,
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
                        ease: false,
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
                paired: None,
                line: None,
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
                travel(Travel::Lunge, 12.0),
                travel(Travel::Leap, 18.0),
            ],
            block: Block::default(),
            approaches: vec![
                Approach {
                    name: Travel::Lunge.clip_name().into(),
                    beyond: 1.0,
                },
                Approach {
                    name: Travel::Leap.clip_name().into(),
                    beyond: 2.6,
                },
            ],
        }
    }
}

impl AttackSet {
    /// Sword and shield: a forehand slash (light) or a one-handed overhead (heavy) from
    /// neutral; slash → backhand → thrust on light presses; a heavy press after the slash
    /// rams the shield into the foe, after the backhand throws the overhead; after the bash
    /// a light press thrusts and a heavy one throws the overhead. Each closes to its range in
    /// its wind-up; the finishers step back out after.
    pub fn sword_shield() -> Self {
        use WeaponMove as W;
        let cut = |kind| Some(Striker::sword(kind));
        AttackSet {
            name: "sword_shield".into(),
            moves: vec![
                weapon(
                    W::SwordSlash,
                    Some("attack_light"),
                    &[
                        ("attack_light", W::SwordBackhand),
                        ("attack_heavy", W::ShieldBash),
                    ],
                    1.1,
                    Some(Blow::new(12.0, 28.0, 0.07)),
                    cut(DamageKind::Slash),
                ),
                weapon(
                    W::SwordBackhand,
                    None,
                    &[
                        ("attack_light", W::SwordThrust),
                        ("attack_heavy", W::SwordOverhead),
                    ],
                    1.1,
                    Some(Blow::new(12.0, 28.0, 0.07)),
                    cut(DamageKind::Slash),
                ),
                weapon(
                    W::SwordThrust,
                    None,
                    &[],
                    1.2,
                    Some(Blow::new(18.0, 40.0, 0.1)),
                    cut(DamageKind::Pierce),
                ),
                weapon(
                    W::SwordOverhead,
                    Some("attack_heavy"),
                    &[],
                    1.05,
                    Some(Blow {
                        high: true,
                        ..Blow::new(24.0, 55.0, 0.12)
                    }),
                    cut(DamageKind::Slash),
                ),
                weapon(
                    W::ShieldBash,
                    None,
                    &[
                        ("attack_light", W::SwordThrust),
                        ("attack_heavy", W::SwordOverhead),
                    ],
                    0.85,
                    // Little harm, a big shove: the foe staggers back open, and a raised
                    // guard is knocked aside.
                    Some(Blow {
                        breaks_guard: true,
                        ..Blow::new(5.0, 75.0, 0.08)
                    }),
                    Some(Striker::shield()),
                ),
                riposte(
                    weapons::SWORD_RIPOSTE,
                    weapons::SWORD_RIPOSTE_DISTANCE,
                    &[Opening::Deflected, Opening::GuardBroken],
                    // Three times the thrust (Elden Ring ripostes are 2.6 to 4.2 times a
                    // normal hit).
                    54.0,
                    Striker::sword(DamageKind::Pierce),
                ),
            ]
            .into_iter()
            .chain(line_moves(false))
            .collect(),
            block: Block::default(),
            approaches: Vec::new(),
        }
    }

    /// Great sword: a horizontal sweep (light) or an overhead from vom Tag (heavy) from
    /// neutral; sweep and rising cut alternate on light presses, and a heavy press after
    /// either throws the overhead, which can't be interrupted while it winds up (armor),
    /// lands hard enough to put a person down, breaks a guard it lands on and sweeps on into
    /// the opening on a light press.
    pub fn greatsword() -> Self {
        use WeaponMove as W;
        let cut = Some(Striker::greatsword(DamageKind::Slash));
        // It breaks a guard it lands on, and a light press after it sweeps into the opening.
        let mut overhead = weapon(
            W::GreatswordOverhead,
            Some("attack_heavy"),
            &[("attack_light", W::GreatswordSweep)],
            1.4,
            Some(Blow {
                high: true,
                breaks_guard: true,
                ..Blow::new(42.0, 120.0, 0.16)
            }),
            cut,
        );
        overhead.windows.push(Window {
            kind: WindowKind::Armor,
            start: 0.0,
            end: W::GreatswordOverhead.timing().1[0],
        });
        AttackSet {
            name: "greatsword".into(),
            moves: vec![
                weapon(
                    W::GreatswordSweep,
                    Some("attack_light"),
                    &[
                        ("attack_light", W::GreatswordRise),
                        ("attack_heavy", W::GreatswordOverhead),
                    ],
                    1.45,
                    Some(Blow::new(22.0, 60.0, 0.11)),
                    cut,
                ),
                weapon(
                    W::GreatswordRise,
                    None,
                    &[
                        ("attack_light", W::GreatswordSweep),
                        ("attack_heavy", W::GreatswordOverhead),
                    ],
                    1.4,
                    Some(Blow::new(22.0, 60.0, 0.11)),
                    cut,
                ),
                overhead,
                riposte(
                    weapons::GREATSWORD_RIPOSTE,
                    weapons::GREATSWORD_RIPOSTE_DISTANCE,
                    &[Opening::Deflected, Opening::GuardBroken],
                    // Under three times the sweep (great swords riposte for less, 2.75).
                    60.0,
                    Striker::greatsword(DamageKind::Pierce),
                ),
            ]
            .into_iter()
            .chain(line_moves(true))
            .collect(),
            block: Block::default(),
            approaches: Vec::new(),
        }
    }

    /// Bow: a quick shot (light) or a long aimed one (heavy). Each turns to face the target
    /// without stepping; the arrow is the game's to fly, on the clip's `release` event (the
    /// `draw` curve says how far the string is back, `nocked` whether an arrow is on it).
    pub fn bow() -> Self {
        let shot = |m: WeaponMove, input: &str| {
            let (secs, [release, follow]) = m.timing();
            AttackMove {
                name: m.clip_name().into(),
                clip: m.clip_name().into(),
                starts_on: Some(input.into()),
                fade_in: 0.2,
                fade_out: 0.3,
                windows: vec![
                    Window {
                        kind: WindowKind::Warp {
                            range: 0.0,
                            max_speed: 0.0,
                            ease: false,
                        },
                        start: 0.0,
                        end: release,
                    },
                    Window {
                        kind: WindowKind::Cancel,
                        start: follow,
                        end: secs * 0.9,
                    },
                ],
                links: vec![
                    Link {
                        input: "attack_light".into(),
                        to: WeaponMove::BowShot.clip_name().into(),
                    },
                    Link {
                        input: "attack_heavy".into(),
                        to: WeaponMove::BowShotFull.clip_name().into(),
                    },
                ],
                evaded_by: None,
                paired: None,
                line: None,
            }
        };
        AttackSet {
            name: "bow".into(),
            moves: vec![
                shot(WeaponMove::BowShot, "attack_light"),
                shot(WeaponMove::BowShotFull, "attack_heavy"),
            ],
            block: Block::default(),
            approaches: Vec::new(),
        }
    }
}

/// What a weapon blow does when it lands.
#[derive(Clone, Copy)]
struct Blow {
    damage: f32,
    impulse: f32,
    hitstop: f32,
    high: bool,
    breaks_guard: bool,
}

impl Blow {
    fn new(damage: f32, impulse: f32, hitstop: f32) -> Self {
        Self {
            damage,
            impulse,
            hitstop,
            high: false,
            breaks_guard: false,
        }
    }
}

/// A weapon move from its clip ([`WeaponMove::timing`]): it closes to `range` of the target
/// over its wind-up, lands `blow` with `striker` while the stroke passes in front (from a
/// third of the way through it), and takes a linked press after the stroke. A finisher (no
/// links) steps back out a little on the way back to the stance.
fn weapon(
    m: WeaponMove,
    starts_on: Option<&str>,
    links: &[(&str, WeaponMove)],
    range: f32,
    blow: Option<Blow>,
    striker: Option<Striker>,
) -> AttackMove {
    let (secs, [a, b]) = m.timing();
    let bone = if m.left_handed() { "hand_l" } else { "hand_r" };
    let mut windows = vec![Window {
        kind: WindowKind::Warp {
            range,
            max_speed: 2.5,
            ease: false,
        },
        start: 0.0,
        end: a + (b - a) * 0.3,
    }];
    if let Some(blow) = blow {
        windows.push(Window {
            kind: WindowKind::Hit {
                bone: bone.into(),
                // From the middle of the chest or face: a blade that comes this close has
                // crossed the body (a torso is about 0.35 m across).
                reach: 0.3,
                damage: blow.damage,
                impulse: blow.impulse,
                hitstop: blow.hitstop,
                high: blow.high,
                liver: false,
                striker,
                breaks_guard: blow.breaks_guard,
            },
            start: a + (b - a) * 0.3,
            end: b + 0.03,
        });
    }
    windows.push(Window {
        kind: WindowKind::Cancel,
        start: b + 0.04,
        end: secs * 0.9,
    });
    if links.is_empty() {
        windows.push(Window {
            kind: WindowKind::Warp {
                range: range + 0.35,
                max_speed: 1.4,
                ease: false,
            },
            start: b + 0.1,
            end: secs,
        });
    }
    AttackMove {
        name: m.clip_name().into(),
        clip: m.clip_name().into(),
        starts_on: starts_on.map(Into::into),
        fade_in: 0.1,
        fade_out: 0.25,
        windows,
        links: links
            .iter()
            .map(|(input, to)| Link {
                input: (*input).into(),
                to: to.clip_name().into(),
            })
            .collect(),
        evaded_by: None,
        paired: None,
        line: None,
    }
}

/// A riposte on a light press at an opened foe: `clip`, its victim `distance` ahead playing
/// [`RIPOSTED`](crate::build::humanoid::RIPOSTED), the blade going in for `damage` as the
/// clips meet at [`RIPOSTED_LANDS`](crate::build::humanoid::RIPOSTED_LANDS).
fn riposte(
    clip: &str,
    distance: f32,
    after: &[Opening],
    damage: f32,
    striker: Striker,
) -> AttackMove {
    use crate::build::humanoid::{RIPOSTED, RIPOSTED_LANDS};
    AttackMove {
        name: clip.into(),
        clip: clip.into(),
        starts_on: Some("attack_light".into()),
        fade_in: 0.12,
        fade_out: 0.3,
        windows: vec![Window {
            kind: WindowKind::Hit {
                bone: "hand_r".into(),
                reach: 0.3,
                damage,
                // It goes in rather than knocking the body away: the victim's clip carries it.
                impulse: 0.0,
                hitstop: 0.12,
                high: false,
                liver: false,
                striker: Some(striker),
                breaks_guard: false,
            },
            start: RIPOSTED_LANDS - 0.02,
            end: RIPOSTED_LANDS + 0.04,
        }],
        links: Vec::new(),
        evaded_by: None,
        line: None,
        paired: Some(Pairing {
            victim_clip: RIPOSTED.into(),
            distance,
            after: after.to_vec(),
            max_distance: default_pair_reach(),
            max_angle: default_pair_angle(),
            align_secs: default_pair_align(),
        }),
    }
}

/// An aimed cut along `angle` at `height`, or a stab, with one hand (sword and shield) or
/// both (great sword): it closes to the range its clip strikes at in its wind-up, its blow
/// lands where its blade crosses the aimed zone, and another aimed strike may follow after
/// its stroke. Only an aimed press reaches it (no input starts it).
pub fn line_move(two_handed: bool, line: Line) -> AttackMove {
    let (secs, [a, b]) = weapons::line_timing(two_handed, line.stab);
    let set = if two_handed { "greatsword" } else { "sword" };
    let clip = if line.stab {
        weapons::stab_clip(set, line.height)
    } else {
        weapons::line_clip(set, line.angle, line.height)
    };
    let kind = if line.stab {
        DamageKind::Pierce
    } else {
        DamageKind::Slash
    };
    let striker = if two_handed {
        Striker::greatsword(kind)
    } else {
        Striker::sword(kind)
    };
    // A stab hurts more for its point, a great sword for its weight; a low cut less.
    let blow = match (two_handed, line.stab) {
        (false, false) => Blow::new(12.0, 28.0, 0.07),
        (false, true) => Blow::new(18.0, 40.0, 0.1),
        (true, false) => Blow::new(22.0, 60.0, 0.11),
        (true, true) => Blow::new(30.0, 70.0, 0.12),
    };
    let range = weapons::line_reach(two_handed);
    // A cut sweeps through the middle of its stroke; a stab goes in at its end.
    let hit = if line.stab {
        [a + (b - a) * 0.5, b + 0.06]
    } else {
        [a + (b - a) * 0.3, b + 0.03]
    };
    AttackMove {
        name: clip.clone(),
        clip,
        starts_on: None,
        fade_in: 0.1,
        fade_out: 0.25,
        windows: vec![
            Window {
                kind: WindowKind::Warp {
                    range,
                    max_speed: 2.5,
                    ease: false,
                },
                start: 0.0,
                end: hit[0],
            },
            Window {
                kind: WindowKind::Hit {
                    bone: "hand_r".into(),
                    reach: 0.3,
                    damage: blow.damage,
                    impulse: blow.impulse,
                    hitstop: blow.hitstop,
                    high: line.height == weapons::Height::High,
                    liver: false,
                    striker: Some(striker),
                    breaks_guard: false,
                },
                start: hit[0],
                end: hit[1],
            },
            Window {
                kind: WindowKind::Cancel,
                start: hit[1] + 0.01,
                end: secs * 0.9,
            },
        ],
        links: Vec::new(),
        evaded_by: None,
        paired: None,
        line: Some(line),
    }
}

/// Every aimed cut and stab of a set, one-handed or with both hands.
fn line_moves(two_handed: bool) -> Vec<AttackMove> {
    weapons::Height::ALL
        .into_iter()
        .flat_map(|height| {
            weapons::LINE_ANGLES
                .into_iter()
                .map(move |angle| Line {
                    angle,
                    height,
                    stab: false,
                })
                .chain([Line {
                    angle: 0.0,
                    height,
                    stab: true,
                }])
        })
        .map(|line| line_move(two_handed, line))
        .collect()
}

/// An approach move on the humanoid's lunge or leap: the warp eases the body to just outside
/// punching range over the clip's travel (at most `max_speed` m/s), and the punch follows on
/// touch-down.
fn travel(kind: Travel, max_speed: f32) -> AttackMove {
    let secs = kind.secs();
    let (go, land) = kind.travel();
    AttackMove {
        name: kind.clip_name().into(),
        clip: kind.clip_name().into(),
        starts_on: None,
        fade_in: 0.06,
        fade_out: 0.12,
        windows: vec![
            Window {
                kind: WindowKind::Warp {
                    range: 0.8,
                    max_speed,
                    ease: true,
                },
                start: go * secs,
                end: land * secs,
            },
            Window {
                kind: WindowKind::Cancel,
                start: land * secs,
                end: secs,
            },
        ],
        links: Vec::new(),
        evaded_by: None,
        paired: None,
        line: None,
    }
}

/// Attack sets by name.
#[derive(Resource, Debug, Clone)]
pub struct AttackSets(pub HashMap<String, AttackSet>);

impl Default for AttackSets {
    fn default() -> Self {
        let sets = [
            AttackSet::boxing(),
            AttackSet::sword_shield(),
            AttackSet::greatsword(),
            AttackSet::bow(),
        ];
        Self(sets.into_iter().map(|s| (s.name.clone(), s)).collect())
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

/// How fast each phase of a strike plays, times as authored: a slow wind-up reads as a
/// telegraph (a rookie's), a fast one as a master's; a sharper active phase snaps the blow,
/// a slower recovery holds the follow-through. `speed` scales all three.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tempo {
    pub speed: f32,
    pub windup: f32,
    pub active: f32,
    pub recovery: f32,
}

impl Default for Tempo {
    fn default() -> Self {
        Self {
            speed: 1.0,
            windup: 1.0,
            active: 1.0,
            recovery: 1.0,
        }
    }
}

impl Tempo {
    /// The rate the move plays at in `phase`.
    pub fn rate(&self, phase: AttackPhase) -> f32 {
        let p = match phase {
            AttackPhase::Idle => 1.0,
            AttackPhase::Startup => self.windup,
            AttackPhase::Active => self.active,
            AttackPhase::Recovery => self.recovery,
        };
        (p * self.speed).clamp(0.1, 4.0)
    }
}

/// A character that attacks with an [`AttackSet`].
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attacker {
    pub set: String,
    /// Who its hits land on and warps close to (a [`Freeflow`](crate::freeflow::Freeflow)
    /// picks it again on each press).
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Who the current move is aimed at: its target when it started. A move goes on at the
    /// one it started on, so a target picked mid-move (the last one went down) gets the next.
    #[serde(skip)]
    pub struck: Option<Entity>,
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
    /// How fast each phase plays.
    pub tempo: Tempo,
    /// Strikes pulled back in their wind-up so far (feints).
    pub feinted: u32,
    /// World-space move the last warp step asked for, meters.
    pub warp_delta: Vec3,
    /// Hits landed by the current move (blocked ones not counted).
    pub hits: u32,
    /// Strikes of the current move that landed on a guard (and bounced, or broke through it:
    /// see `broke`).
    pub blocked: u32,
    /// Strikes of the current move that a guard deflected.
    pub deflected: u32,
    /// Strikes of the current move that broke the guard they landed on.
    pub broke: u32,
    /// Strikes of the current move whose hit window passed without landing (out of reach,
    /// or the target's head moved).
    pub missed: u32,
    /// Seconds left of a rebound: a weapon strike that landed on a guard bounces back up its
    /// arc and the move ends with it; no new move starts until then (the defender's opening
    /// for a guard counter). A deflected strike leaves it open longer.
    pub recoil: f32,
    /// Seconds it stays open to a riposte after its blade was deflected.
    pub open: f32,
    /// The newest press waiting, and how long it has waited.
    #[serde(skip)]
    pub buffered: Option<(String, f32)>,
    /// The strike an approach under way leads into.
    #[serde(skip)]
    pub follow: Option<String>,
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
    /// Where the striker of the hit window under way was last frame (world), to sweep from.
    #[serde(skip)]
    swept: Option<(DVec3, DVec3)>,
    /// The body zone its strikes aim at (from [`StrikeTargets`](engine_world::animation::StrikeTargets)):
    /// with one set, a light press in a set with aimed lines picks the cut or stab for it.
    #[serde(skip)]
    pub zone: Option<BodyZone>,
    /// Where in that zone (from `StrikeTargets.point`, the target's model space): the rules'
    /// skill-scattered point, so a rookie's blade wanders off the middle.
    #[serde(skip)]
    pub point: Option<Vec3>,
    /// How good it is with what it holds, 0..1: a rookie knows three lines, a master all
    /// eight (see [`known_lines`]).
    pub skill: f32,
    /// The aimed strikes it threw, newest last (at most [`RECENT_LINES`]): the next one
    /// takes a new angle.
    #[serde(skip)]
    pub recent: Vec<Line>,
    /// The zone the aimed strike playing was picked for.
    #[serde(skip)]
    pub aimed: Option<BodyZone>,
    /// The point in that zone it was picked for (target model space).
    #[serde(skip)]
    pub aimed_point: Option<Vec3>,
    /// A blade being steered through that point this frame (see [`aim_strikes`]).
    #[serde(skip)]
    pub(crate) blade_aim: Option<BladeAim>,
}

/// A blade steered through an aimed point: the hand holding it, the blade in that hand's
/// frame, the point (world) and how much of the way (0..1).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BladeAim {
    bone: String,
    striker: Striker,
    point: DVec3,
    weight: f32,
}

impl Default for Attacker {
    fn default() -> Self {
        Self {
            set: "boxing".into(),
            target: None,
            struck: None,
            buffer_secs: 0.2,
            warp: true,
            current: None,
            time: 0.0,
            phase: AttackPhase::Idle,
            cancel_open: false,
            armored: false,
            tempo: Tempo::default(),
            feinted: 0,
            warp_delta: Vec3::ZERO,
            hits: 0,
            blocked: 0,
            deflected: 0,
            broke: 0,
            missed: 0,
            recoil: 0.0,
            open: 0.0,
            buffered: None,
            follow: None,
            landed: Vec::new(),
            whiffed: Vec::new(),
            committed: None,
            aim: None,
            swept: None,
            zone: None,
            point: None,
            skill: 0.5,
            recent: Vec::new(),
            aimed: None,
            aimed_point: None,
            blade_aim: None,
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

/// How long a feinted strike takes to settle back into the stance, s.
const FEINT_SECS: f32 = 0.2;

/// How long before its hit window a strike's aim stops following the target, s.
const COMMIT_SECS: f32 = 0.2;

/// A blocked weapon's rebound: how fast the swing runs back (times its own speed), for how
/// long (s), and the fade back to the stance after (s).
const REBOUND_RATE: f32 = 1.4;
const REBOUND_SECS: f32 = 0.22;
const REBOUND_FADE: f32 = 0.3;

/// How far the struck body shakes side to side in hitstop, per second of hitstop (m/s), and
/// at most (m).
const SHAKE_PER_HITSTOP: f32 = 0.12;
const SHAKE_MAX: f32 = 0.025;

/// A deflect: how soon after the guard rises a blow must land (Elden Ring's parries are
/// active 3 to 6 frames at 30 fps), and how soon when the guard came down less than
/// `SPAM_SECS` before it rose (Sekiro shrinks its window to 7 frames at 60 fps then), s.
const DEFLECT_SECS: f32 = 0.2;
const DEFLECT_SPAM_SECS: f32 = 0.07;
const SPAM_SECS: f32 = 0.4;

/// A deflected weapon's rebound (rate, seconds, fade), longer and harder than a block's, and
/// how long the attacker stays open after it (its recoil in all), s.
const DEFLECT_REBOUND: (f32, f32, f32) = (1.8, 0.3, 0.35);
const DEFLECT_RECOIL: f32 = 0.9;

/// How far a deflected attacker is shoved back, m, and its shake.
const DEFLECT_SHOVE: f32 = 0.3;
const DEFLECT_SHAKE: f32 = 0.02;

/// Shield-stun after the hitstop of a blocked blow, s.
const SHIELD_STUN: f32 = 0.15;

/// How long a broken guard stays down (For Honor leaves 800 ms), s, and how far the blow
/// shoves its owner back, m.
const GUARD_BROKEN_SECS: f32 = 1.0;
const GUARD_BROKEN_SHOVE: f32 = 0.3;

/// How far to its own right of the spine a target's liver is, meters.
const LIVER_SIDE: f64 = 0.1;

/// How far short of the middle of the head or chest a strike aims, meters (about where the
/// face or the front of the ribs is, less the fist).
const AIM_SHORT: f64 = 0.1;

/// From the wrist (the hand bone) to the knuckles, along the forearm, meters.
const FIST_LEN: f32 = 0.08;

/// How many aimed strikes an attacker remembers: none of them is thrown again at once.
pub const RECENT_LINES: usize = 5;

/// The cut lines known at `skill` (0..1): a rookie's three (the two diagonals down and the
/// cut across from the right), then the backhand across and straight down, and from 2/3 on
/// the rising cuts too.
pub fn known_lines(skill: f32) -> &'static [f32] {
    if skill < 1.0 / 3.0 {
        &[45.0, 315.0, 90.0]
    } else if skill < 2.0 / 3.0 {
        &[45.0, 315.0, 90.0, 270.0, 0.0]
    } else {
        &weapons::LINE_ANGLES
    }
}

/// The height an aimed strike at `zone` goes at.
pub fn zone_height(zone: BodyZone) -> weapons::Height {
    use BodyZone::*;
    match zone {
        Head | Neck => weapons::Height::High,
        ThighL | ThighR | KneeL | KneeR | ShinL | ShinR => weapons::Height::Low,
        _ => weapons::Height::Mid,
    }
}

/// What an aimed press sees of its foe.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Foe {
    /// Open: recovering from a strike, staggered, its blade deflected or its guard broken.
    pub open: bool,
    /// Its guard up (a shield or gloves held high).
    pub guarding: bool,
    /// Its back turned to the attacker.
    pub turned_away: bool,
    /// Flat distance between them, m.
    pub distance: f32,
}

/// How close a stab pays (a bind, too close to swing: well inside the 1.15 m a cut strikes
/// from), m.
const STAB_CLOSE: f32 = 0.85;

/// Score `line` for the next aimed strike at `zone`, after `recent`: a new angle (none of
/// the last few, nothing within 60° of the last two), starting where the last one ended,
/// a stab when the foe is open or too close to swing, or at its middle when it guards high
/// or has turned away (the belly, the kidney).
pub fn line_score(line: &Line, zone: BodyZone, recent: &[Line], foe: &Foe) -> f32 {
    let mut score = 0.0;
    if line.stab {
        // Its guard high or its back turned, the point goes in at the belly or the kidney; an
        // opening is taken at the body, not the legs (a leg is cut), but too close to swing
        // the point is all there is.
        let height = zone_height(zone);
        let middle = height == weapons::Height::Mid;
        let open = foe.open && height != weapons::Height::Low;
        let pays =
            open || foe.distance < STAB_CLOSE || (middle && (foe.guarding || foe.turned_away));
        score += if pays { 3.0 } else { -3.0 };
        // A stab never follows a stab.
        if recent.last().is_some_and(|l| l.stab) {
            score -= 4.0;
        }
        return score;
    }
    // Nothing thrown again while it is remembered, newer repeats costing more.
    for (k, old) in recent.iter().rev().enumerate() {
        if !old.stab && (old.angle - line.angle).abs() < 1.0 {
            score -= 6.0 - k as f32;
        }
    }
    let travel = line.travel();
    for old in recent.iter().rev().take(2).filter(|l| !l.stab) {
        if travel.angle_between(old.travel()).to_degrees() < 60.0 {
            score -= 2.0;
        }
    }
    // Starting where the last one ended: its far side is this one's near side.
    if let Some(last) = recent.last().filter(|l| !l.stab) {
        score += 1.0 * (1.0 - travel.dot(last.travel())) * 0.5;
    }
    // A cut from the side the zone is on reaches it first (its left is the attacker's
    // right, the side a cut from the right comes from).
    if let Some(side) = zone.side() {
        let from_right = line.angle > 0.0 && line.angle < 180.0;
        let near = (side == 0) == from_right;
        if near {
            score += 0.3;
        }
    }
    score
}

/// The aimed strike for `zone` among `moves`, at `skill` after `recent`, or none when the
/// set has no lines. A rookie goes round its three lines; a master never throws one of
/// its last five again.
pub fn pick_line<'a>(
    moves: &'a AttackSet,
    zone: BodyZone,
    skill: f32,
    recent: &[Line],
    foe: &Foe,
) -> Option<&'a AttackMove> {
    let height = zone_height(zone);
    let known = known_lines(skill);
    // One who knows few lines goes round them: it remembers one fewer than it knows.
    let recent = &recent[recent.len().saturating_sub(known.len() - 1)..];
    moves
        .moves
        .iter()
        .filter(|m| {
            m.line.is_some_and(|l| {
                l.height == height && (l.stab || known.iter().any(|a| (a - l.angle).abs() < 1.0))
            })
        })
        .map(|m| {
            (
                line_score(m.line.as_ref().expect("a line"), zone, recent, foe),
                m,
            )
        })
        // The first best, so a tie goes the same way every time.
        .fold(
            None,
            |best: Option<(f32, &AttackMove)>, (s, m)| match best {
                Some((b, _)) if b >= s => best,
                _ => Some((s, m)),
            },
        )
        .map(|b| b.1)
}

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

/// How much farther than its data says move `m` reaches: warp ranges are for an arm of
/// REFERENCE_ARM, and longer or shorter arms strike from farther or closer.
fn arm_scale(m: &AttackMove, skeleton: &crate::skeleton::Skeleton) -> f32 {
    m.windows
        .iter()
        .find_map(|w| match &w.kind {
            WindowKind::Hit { bone, .. } => skeleton.index_of(bone),
            _ => None,
        })
        .map_or(1.0, |hand| arm_length(skeleton, hand) / REFERENCE_ARM)
}

/// The range move `m` strikes from (its first warp window's), for the arm it strikes with.
fn strike_range(m: &AttackMove, skeleton: &crate::skeleton::Skeleton) -> Option<f32> {
    let range = m.windows.iter().find_map(|w| match &w.kind {
        WindowKind::Warp { range, .. } => Some(*range),
        _ => None,
    })?;
    Some(range * arm_scale(m, skeleton))
}

/// How far along an eased travel the body is at `u` (0..1) of it: speeding up over the first
/// quarter, steady, slowing over the last (a trapezoid of speed, peaking at 4/3 of the
/// average).
fn ease_travel(u: f32) -> f32 {
    const A: f32 = 0.25;
    let u = u.clamp(0.0, 1.0);
    let k = 2.0 * A * (1.0 - A);
    if u < A {
        u * u / k
    } else if u > 1.0 - A {
        1.0 - (1.0 - u) * (1.0 - u) / k
    } else {
        (u - A * 0.5) / (1.0 - A)
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
        if let Some(blade) = &attacker.blade_aim {
            steer_blade(&mut animator, gt, blade);
            continue;
        }
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
        // The clip's own bend and roll (a hook's elbow out, a jab's down): no hinge to roll
        // to, and no soft reach (the aim already stops a fist short).
        crate::ik::two_bone_live(
            &mut animator.edit(),
            upper,
            lower,
            end,
            to_model(gt, *aim) - forearm * FIST_LEN,
            crate::ik::Limb {
                weight: weight.min(1.0),
                soft: false,
                ..crate::ik::Limb::animated(pole)
            },
        );
    }
}

/// How long before a blade's hit window it starts steering toward the aimed point, s.
const BLADE_AIM_LEAD: f32 = 0.2;
/// How long after the window the steer lets go, s.
const BLADE_AIM_FADE: f32 = 0.2;
/// Farthest the hand moves to put the blade through the point, m: past it the cut lands
/// where the swing takes it.
const BLADE_AIM_REACH: f32 = 0.3;

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Move the hand holding a blade, keeping its turn, so the blade passes through the aimed
/// point: the part of the blade nearest the point (from a third of the way out to the tip)
/// is brought onto it.
fn steer_blade(animator: &mut Animator, gt: &GlobalTransform, blade: &BladeAim) {
    if !animator.posed() {
        return;
    }
    let s = animator.skeleton.clone();
    let Some(end) = s.index_of(&blade.bone) else {
        return;
    };
    let parent = |b: usize| s.parents[b].map(usize::from);
    let (Some(lower), Some(upper)) = (parent(end), parent(end).and_then(parent)) else {
        return;
    };
    animator.refresh_model();
    let hand = animator.model[end];
    let point = to_model(gt, blade.point);
    let (from, to) = (
        hand.transform_point(blade.striker.from),
        hand.transform_point(blade.striker.to),
    );
    let ab = to - from;
    let along = ((point - from).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.33, 1.0);
    let on_blade = from + ab * along;
    let shift = (point - on_blade).clamp_length_max(BLADE_AIM_REACH);
    let (a, b, c) = (
        animator.model[upper].t,
        animator.model[lower].t,
        animator.model[end].t,
    );
    let pole = b - (a + c) * 0.5;
    crate::ik::two_bone_live(
        &mut animator.edit(),
        upper,
        lower,
        end,
        c + shift,
        crate::ik::Limb {
            weight: blade.weight.min(1.0),
            soft: true,
            end_rotation: Some((hand.r, 1.0)),
            ..crate::ik::Limb::animated(pole)
        },
    );
}

/// A landed blow, applied after the attackers have moved: freeze both (hitstop), and push
/// a blocked attacker back (world, m).
pub(crate) struct Landed {
    attacker: Entity,
    target: Entity,
    hitstop: f32,
    pushback: Vec3,
    outcome: Outcome,
    /// Which way the blow pushed (flat, world), and how far a block shoves the defender, m.
    push: Vec3,
    shove: f32,
    /// It met a shield (rather than gloves or the body).
    shield: bool,
}

/// What became of a blow that reached its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Hit,
    Blocked,
    Deflected,
    BrokeGuard,
}

/// Where a target can be struck (world): its chest and face, and its guard if it is up.
#[derive(Clone)]
struct Target {
    chest: DVec3,
    head: DVec3,
    /// Under the ribs on its own right.
    liver: DVec3,
    guard: Option<Guard>,
    /// Its guard rose just now: a weapon's blow landing on it is deflected.
    deflects: bool,
    /// Open to a paired move (a riposte), and how.
    opening: Option<Opening>,
    /// In a paired move with this one: nobody else's blows touch it.
    pair_with: Option<Entity>,
    /// The middle of each body zone (world), by [`BodyZone::index`].
    zones: [DVec3; 16],
    /// Its own frame, for points given in its model space.
    frame: GlobalTransform,
    /// Its parts as capsules (world), for where a blade goes in, and its knee joints.
    capsules: Vec<Capsule>,
    knees: [DVec3; 2],
    /// Which way it faces (flat).
    forward: DVec3,
    /// Recovering from a strike of its own, or staggered by a blow.
    recovering: bool,
}

/// A raised guard: which way the target faces (flat), its knuckles (world), and the middle
/// of its shield's face with the shield's radius and block, if it has one.
#[derive(Clone)]
struct Guard {
    forward: DVec3,
    gloves: [DVec3; 2],
    shield: Option<(DVec3, f32, Block)>,
}

impl Target {
    /// Where an aimed strike at `zone` goes: the given point (its model space) when there is
    /// one, held to [`POINT_STRAY`] of the zone's middle, else the middle.
    fn aim_at(&self, zone: BodyZone, point: Option<Vec3>) -> DVec3 {
        let middle = self.zones[zone.index()];
        point.map_or(middle, |p| {
            let at = crate::ik::to_world(&self.frame, p);
            middle + (at - middle).clamp_length_max(POINT_STRAY)
        })
    }

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

    /// Where a strike from `from` (flat on the ground, the attacker's position) with the
    /// fist now at `fist` is blocked, if the guard is up and faces it: on a shield (high or
    /// low, with the shield's block and radius), or for a high strike on the nearer glove.
    fn blocking(
        &self,
        from: DVec3,
        fist: DVec3,
        high: bool,
        cone_deg: f32,
    ) -> Option<(DVec3, Option<(f32, Block)>)> {
        let g = self.guard.as_ref()?;
        let to = (from - self.chest).with_y(0.0).normalize_or_zero();
        let faces = |cone: f32| g.forward.dot(to) >= (cone as f64).to_radians().cos();
        if let Some((center, radius, block)) = &g.shield {
            return faces(block.cone_deg).then(|| (*center, Some((*radius, block.clone()))));
        }
        if !high || !faces(cone_deg) {
            return None;
        }
        g.gloves
            .into_iter()
            .min_by(|a, b| a.distance(fist).total_cmp(&b.distance(fist)))
            .map(|glove| (glove, None))
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
        Option<&'static Defense>,
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
            Option<&Shield>,
            Option<&mut Defense>,
            Option<&LocalTimeScale>,
            Option<&Riposted>,
        )>,
        Query<(
            &mut Animator,
            Option<&mut HitReaction>,
            Option<&mut Defense>,
        )>,
    )>,
    mut damage: MessageWriter<DamageEvent>,
    (judge, mut contacts, loadouts): (
        Option<Res<MeleeJudge>>,
        MessageWriter<MeleeContact>,
        Query<&Loadout>,
    ),
    mut landed: Local<Vec<Landed>>,
    mut pairs: Local<Vec<(Entity, Entity, Pairing)>>,
    mut commands: Commands,
) {
    let Some(sets) = sets else {
        return;
    };
    // Where targets are: chest and head height and the guard, world space.
    let mut targets: HashMap<Entity, Target> = HashMap::new();
    let mut downed: HashSet<Entity> = HashSet::new();
    {
        let attackers: Vec<Entity> = set
            .p0()
            .iter()
            .flat_map(|(_, a, ..)| [a.target, a.struck])
            .flatten()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut q = set.p1();
        for t in attackers {
            let Ok((
                gt,
                animator,
                params,
                down,
                hurt,
                striking,
                evader,
                shield,
                defense,
                local,
                riposted,
            )) = q.get_mut(t)
            else {
                continue;
            };
            // Knocked down or on a knee, or getting up after a riposte: nothing to strike
            // until it is up again.
            if down.is_some()
                || hurt.is_some_and(Hurt::is_down)
                || riposted.is_some_and(|r| r.getting_up)
            {
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
            let guarding = params.is_some_and(|p| p.get("guarding") > 0.5 || p.get("shield_guard") > 0.5)
                && !hurt.is_some_and(Hurt::guard_down)
                // Run through.
                && riposted.is_none()
                && !striking.is_some_and(|a| a.current.is_some())
                // Moving its head out of the way instead.
                && !evader.is_some_and(|e| e.evading.is_some());
            // Keep time with the guard: how long it has been up, and whether it is broken.
            let mut deflects = false;
            let mut opening = striking
                .filter(|a| a.open > 0.0)
                .map(|_| Opening::Deflected);
            let guarding = match defense {
                Some(mut d) => {
                    let frozen = animator.is_some_and(|a| a.frozen() > 0.0);
                    d.tick(guarding, if frozen { 0.0 } else { time.delta_for(local) });
                    deflects = d.deflects_now();
                    if d.broken > 0.0 {
                        opening = Some(Opening::GuardBroken);
                    }
                    guarding && d.broken <= 0.0
                }
                None => {
                    commands.entity(t).try_insert(Defense::default());
                    guarding
                }
            };
            let guard = animator.filter(|_| guarding).and_then(|a| {
                let glove = |hand: &str| Some(fist_of(a, gt, a.skeleton.index_of(hand)?));
                Some(Guard {
                    forward: (gt.rotation() * Vec3::NEG_Z)
                        .with_y(0.0)
                        .normalize()
                        .as_dvec3(),
                    gloves: [glove("hand_l")?, glove("hand_r")?],
                    shield: shield.and_then(|sh| {
                        let b = a.skeleton.index_of(&sh.bone)?;
                        let m = a.model[b];
                        let center = crate::ik::to_world(gt, m.transform_point(sh.center));
                        Some((center, sh.radius, sh.block.clone()))
                    }),
                })
            });
            // The middle of each zone: a limb's between its joints, the knee on the joint.
            let mid = |a: &str, b: &str, up: f64| (at(a, up) + at(b, up)) * 0.5;
            let side = |stem: &str, k: usize| format!("{stem}_{}", ["l", "r"][k]);
            let mut zones = [DVec3::ZERO; 16];
            for zone in BodyZone::ALL {
                let k = zone.side().unwrap_or(0);
                use BodyZone::*;
                zones[zone.index()] = match zone {
                    Head => at("head", 1.55) + DVec3::Y * FACE_ABOVE_HEAD,
                    Neck => mid("neck", "head", 1.5),
                    Chest => at("chest", 1.3),
                    Belly => at("spine", 1.07),
                    UpperArmL | UpperArmR => mid(&side("upper_arm", k), &side("lower_arm", k), 1.3),
                    ForearmL | ForearmR => mid(&side("lower_arm", k), &side("hand", k), 1.1),
                    HandL | HandR => at(&side("hand", k), 1.0),
                    ThighL | ThighR => mid(&side("upper_leg", k), &side("lower_leg", k), 0.75),
                    KneeL | KneeR => at(&side("lower_leg", k), 0.5),
                    ShinL | ShinR => mid(&side("lower_leg", k), &side("foot", k), 0.3),
                };
            }
            let recovering = striking.is_some_and(|a| a.phase == AttackPhase::Recovery)
                || hurt.is_some_and(Hurt::guard_down);
            // The head bone sits at the top of the neck: the face is a little above it.
            targets.insert(
                t,
                Target {
                    zones,
                    frame: *gt,
                    capsules: capsules(&at, gt.translation()),
                    knees: [at("lower_leg_l", 0.5), at("lower_leg_r", 0.5)],
                    forward: (gt.rotation() * Vec3::NEG_Z)
                        .with_y(0.0)
                        .normalize_or_zero()
                        .as_dvec3(),
                    recovering,
                    chest: at("chest", 1.3),
                    head: at("head", 1.55) + DVec3::Y * FACE_ABOVE_HEAD,
                    liver: at("spine", 1.07) + (gt.rotation() * Vec3::X).as_dvec3() * LIVER_SIDE,
                    guard,
                    deflects,
                    opening: opening.filter(|_| riposted.is_none()),
                    // A victim, or an attacker in a paired move of its own.
                    pair_with: riposted.map(|r| r.by).or_else(|| {
                        let a = striking?;
                        let m = sets.0.get(&a.set)?.get(a.current.as_deref()?)?;
                        m.paired.as_ref().and(a.struck)
                    }),
                },
            );
        }
    }
    landed.clear();
    pairs.clear();
    for (entity, mut attacker, mut animator, mut transform, gt, settings, local, defense) in
        &mut set.p0()
    {
        let speed = settings.map_or(1.0, |s| if s.paused { 0.0 } else { s.speed });
        let dt = if animator.frozen() > 0.0 {
            0.0
        } else {
            time.delta_for(local) * speed
        };
        let attacker = &mut *attacker;
        attacker.warp_delta = Vec3::ZERO;
        attacker.aim = None;
        attacker.blade_aim = None;
        // Presses: the newest one waits in the buffer.
        let mut pressed = None;
        let mut feint = false;
        animator.triggers.retain(|(name, _)| {
            if name.starts_with("attack_") {
                pressed = Some(name.clone());
                false
            } else if name == "feint" {
                feint = true;
                false
            } else {
                true
            }
        });
        // A feint pulls the strike back while it still winds up: the move ends with a short
        // recovery (the window reads it as cancelled), and nothing lands.
        if feint && attacker.current.is_some() && attacker.phase == AttackPhase::Startup {
            animator.stop_overlay(FEINT_SECS);
            attacker.recoil = FEINT_SECS;
            attacker.feinted += 1;
        }
        // An older press ages first, so a new one always gets this frame to start in, however
        // long the frame (at 4 fps a frame outlasts the buffer). It doesn't age while the
        // fighter can't act (rebounding, in shield-stun or with its guard broken): a counter
        // pressed then comes the moment it can.
        let can_act = attacker.recoil <= 0.0 && defense.is_none_or(Defense::ready);
        if let Some(b) = attacker.buffered.as_mut().filter(|_| can_act) {
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
        attacker.open = (attacker.open - dt).max(0.0);
        // Advance the move playing; at its end it is over (the overlay fades itself). A
        // rebounding move is over when the rebound is. `step` is how far the clip went
        // this frame, at its tempo.
        let mut step = dt;
        if attacker.recoil > 0.0 {
            attacker.recoil -= dt;
            if attacker.recoil <= 0.0 {
                attacker.recoil = 0.0;
                attacker.current = None;
            }
        } else if let Some(name) = attacker.current.clone() {
            let rate = attacker.tempo.rate(attacker.phase);
            animator.set_overlay_rate(rate);
            step = dt * rate;
            attacker.time += step;
            let over = moves
                .get(&name)
                .and_then(|m| library.clip(&m.clip))
                .is_none_or(|c| attacker.time >= c.duration);
            if over {
                attacker.current = None;
            }
        }
        // An approach lands in its cancel window and throws the strike it was for.
        let playing = attacker.current.as_ref().and_then(|c| moves.get(c));
        let now = attacker.time;
        let cancel_open = |m: &AttackMove| {
            m.windows
                .iter()
                .any(|w| w.kind == WindowKind::Cancel && w.overlaps(now, step))
        };
        let follow = playing
            .filter(|m| moves.is_approach(&m.name) && cancel_open(m))
            .and_then(|_| attacker.follow.take());
        // A press starts a move from neutral, or a linked one in a cancel window. Once the
        // move's blows are over, a target picked since it started (the last one went down)
        // gets the next press at once: the rest of the recovery gives way.
        let neutral = |input: &str| {
            moves
                .moves
                .iter()
                .find(|m| m.starts_on.as_deref() == Some(input) && m.paired.is_none())
                .map(|m| m.name.clone())
        };
        let handed_over = attacker.target.is_some() && attacker.struck != attacker.target;
        // A press at a target that is open (its blade deflected, its guard broken), in reach
        // and in front, throws the set's riposte for it.
        let riposte = |input: &str| {
            let tg = targets.get(&attacker.target?)?;
            let opening = tg.opening?;
            let to = (tg.chest - gt.translation()).with_y(0.0);
            let forward = (gt.rotation() * Vec3::NEG_Z)
                .with_y(0.0)
                .normalize_or_zero();
            let off = (to.normalize_or_zero().as_vec3().dot(forward))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            moves
                .moves
                .iter()
                .find(|m| {
                    m.starts_on.as_deref() == Some(input)
                        && m.paired.as_ref().is_some_and(|p| {
                            p.after.contains(&opening)
                                && to.length() as f32 <= p.max_distance
                                && off <= p.max_angle
                        })
                })
                .map(|m| m.name.clone())
        };
        // A light press with a zone aimed at, in a set with lines: the cut or stab for it.
        let aimed = |input: &str| {
            let zone = attacker.zone.filter(|_| input == "attack_light")?;
            let foe = attacker
                .target
                .and_then(|e| targets.get(&e))
                .map(|tg| {
                    let to = (tg.chest - gt.translation()).with_y(0.0);
                    Foe {
                        open: tg.opening.is_some() || tg.recovering,
                        guarding: tg.guard.is_some(),
                        // Facing the way the attacker looks at it.
                        turned_away: tg.forward.dot(to.normalize_or_zero()) > 0.5,
                        distance: to.length() as f32,
                    }
                })
                .unwrap_or(Foe {
                    distance: f32::INFINITY,
                    ..Foe::default()
                });
            pick_line(moves, zone, attacker.skill, &attacker.recent, &foe).map(|m| m.name.clone())
        };
        let neutral = |input: &str| {
            riposte(input)
                .or_else(|| aimed(input))
                .or_else(|| neutral(input))
        };
        let pressed_next = attacker
            .buffered
            .as_ref()
            // Not while rebounding, in shield-stun or with its guard broken.
            .filter(|_| can_act)
            .and_then(|(input, _)| match playing {
                None => neutral(input),
                Some(m) if moves.is_approach(&m.name) => None,
                Some(m) => {
                    // In a cancel window a riposte at an opened foe comes before the link
                    // (the overhead that broke the guard runs straight into it).
                    let linked = cancel_open(m)
                        .then(|| {
                            riposte(input).or_else(|| aimed(input)).or_else(|| {
                                m.links
                                    .iter()
                                    .find(|l| &l.input == input)
                                    .map(|l| l.to.clone())
                            })
                        })
                        .flatten();
                    let blows_over = m
                        .windows
                        .iter()
                        .filter(|w| matches!(w.kind, WindowKind::Hit { .. }))
                        .all(|w| w.end < now);
                    // At a new target the combo starts over from the top.
                    if handed_over {
                        blows_over.then(|| neutral(input)).flatten()
                    } else {
                        linked
                    }
                }
            });
        let from_press = follow.is_none() && pressed_next.is_some();
        // A strike at a target well past its range is thrown after an approach that closes
        // the gap.
        let next = follow.or(pressed_next).map(|name| {
            let gap = (from_press && !moves.approaches.is_empty())
                .then(|| {
                    let m = moves.get(&name).filter(|m| m.paired.is_none())?;
                    let range = strike_range(m, &animator.skeleton)?;
                    let chest = targets.get(&attacker.target?)?.chest;
                    let to = (chest - gt.translation()).with_y(0.0);
                    Some(to.length() as f32 - range)
                })
                .flatten();
            match gap.and_then(|g| moves.approach_for(g)) {
                Some(a) => {
                    attacker.follow = Some(name);
                    a.name.clone()
                }
                None => name,
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
            attacker.deflected = 0;
            attacker.broke = 0;
            attacker.missed = 0;
            attacker.landed.clear();
            attacker.whiffed.clear();
            attacker.committed = None;
            attacker.swept = None;
            attacker.struck = attacker.target;
            attacker.aimed = m.line.and(attacker.zone);
            attacker.aimed_point = attacker.aimed.and(attacker.point);
            if let Some(line) = m.line {
                attacker.recent.push(line);
                let extra = attacker.recent.len().saturating_sub(RECENT_LINES);
                attacker.recent.drain(..extra);
            }
            if from_press {
                attacker.buffered = None;
            }
            if let (Some(p), Some(victim)) = (&m.paired, attacker.target) {
                pairs.push((entity, victim, p.clone()));
                // Taken: nobody else pairs with it or touches it from now.
                if let Some(tg) = targets.get_mut(&victim) {
                    tg.opening = None;
                    tg.pair_with = Some(entity);
                }
            }
        }
        // Windows.
        attacker.cancel_open = false;
        attacker.armored = false;
        attacker.phase = AttackPhase::Idle;
        let Some(m) = attacker.current.as_ref().and_then(|c| moves.get(c)) else {
            continue;
        };
        if attacker.recoil > 0.0 {
            attacker.phase = AttackPhase::Recovery;
            attacker.swept = None;
            continue;
        }
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
            .struck
            .and_then(|e| targets.get(&e).map(|p| (e, p)));
        // Aim the strike: the front of the face (or chest), a fist's width short of its
        // middle on the attacker's side, or the glove of a guard that blocks it.
        let mut blocked_at = None;
        if let Some((_, tg)) = target
            && let Some(WindowKind::Hit {
                bone,
                high,
                liver,
                striker,
                ..
            }) = m
                .windows
                .iter()
                .map(|w| &w.kind)
                .find(|k| matches!(k, WindowKind::Hit { .. }))
        {
            let mid = match attacker.aimed {
                Some(zone) => tg.aim_at(zone, attacker.aimed_point),
                None => tg.aim(*high, *liver),
            };
            let mut back = gt.translation() - mid;
            back.y = 0.0;
            let back = back.normalize_or_zero();
            let fist = animator
                .skeleton
                .index_of(bone)
                .map_or(mid, |b| fist_of(&animator, gt, b));
            // A paired blow can't be blocked: the victim is open and held in place.
            if m.paired.is_none() {
                blocked_at = tg.blocking(gt.translation(), fist, *high, moves.block.cone_deg);
            }
            let mut aim = match &blocked_at {
                // The front of the glove or shield.
                Some((guard, _)) => *guard + back * 0.04,
                None => mid + back * AIM_SHORT,
            };
            // Shortly before the hit the strike is committed: from then on the fist goes where
            // the target was, so a head that slips away is missed.
            if t >= first_hit - COMMIT_SECS {
                aim = *attacker.committed.get_or_insert(aim);
            }
            attacker.aim = Some((bone.clone(), aim));
            // An aimed blade is steered through the point the rules picked (or the aimed
            // part's middle) over the stroke: in over the last of the wind-up, held while it
            // can land, out after.
            if let (Some(st), Some(_), None) = (striker, attacker.aimed, &blocked_at) {
                let weight = if t < first_hit {
                    smooth(1.0 - (first_hit - t) / BLADE_AIM_LEAD)
                } else if t <= last_hit {
                    1.0
                } else {
                    smooth(1.0 - (t - last_hit) / BLADE_AIM_FADE)
                };
                attacker.blade_aim = (weight > 0.0).then(|| BladeAim {
                    bone: bone.clone(),
                    striker: *st,
                    point: mid,
                    weight,
                });
            }
        }
        let mut sweeping = false;
        let aimed = attacker.aimed;
        let aimed_point = attacker.aimed_point;
        for (k, w) in m.windows.iter().enumerate() {
            // A hit window a long frame stepped right over still lands (the fist is still
            // close, its aim fading out); the others act only while they are open.
            let open = (w.start..=w.end).contains(&t);
            let hit = matches!(w.kind, WindowKind::Hit { .. });
            if !open && !(hit && w.overlaps(t, step)) {
                continue;
            }
            match &w.kind {
                WindowKind::Cancel => attacker.cancel_open = true,
                WindowKind::Armor => attacker.armored = true,
                WindowKind::Warp {
                    range,
                    max_speed,
                    ease,
                } => {
                    let Some((_, tg)) = target else {
                        continue;
                    };
                    let chest = tg.chest;
                    let here = gt.translation();
                    let mut to = chest - here;
                    to.y = 0.0;
                    let dist = to.length();
                    if dist < 1e-3 {
                        continue;
                    }
                    let dir = to / dist;
                    let range = *range * arm_scale(m, &animator.skeleton);
                    let left = (w.end - t).max(dt).max(1e-3);
                    let (share, turn) = if *ease {
                        // The share of the gap still to go falls along the eased travel, and
                        // the turn to face it is over by 40% of the window.
                        let span = (w.end - w.start).max(1e-3);
                        let u = |t: f32| (t - w.start) / span;
                        let (before, now) = (1.0 - ease_travel(u(t - dt)), 1.0 - ease_travel(u(t)));
                        let share = if before > 1e-4 {
                            1.0 - now / before
                        } else {
                            1.0
                        };
                        let facing = (w.start + 0.4 * span - t).max(dt).max(1e-3);
                        (share as f64, (dt / facing).min(1.0))
                    } else {
                        // Spread what's left of the gap over what's left of the window.
                        let share = (dt / left).min(1.0);
                        (share as f64, share)
                    };
                    let step = ((dist - range as f64) * share)
                        .clamp(-(*max_speed * dt) as f64, (*max_speed * dt) as f64);
                    let delta = dir * step;
                    attacker.warp_delta = delta.as_vec3();
                    if attacker.warp {
                        transform.translation += delta;
                        // Face it (the rig faces −Z).
                        let want = Quat::from_rotation_y((-dir.x).atan2(-dir.z) as f32);
                        transform.rotation = transform.rotation.slerp(want, turn);
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
                    striker,
                    breaks_guard,
                } => {
                    let Some((victim, tg)) = target else {
                        continue;
                    };
                    // Nobody else's blows touch a pair while it plays.
                    if attacker.landed.contains(&k)
                        || tg.pair_with.is_some_and(|other| other != entity)
                    {
                        continue;
                    }
                    let Some(b) = animator.skeleton.index_of(bone) else {
                        continue;
                    };
                    // A shield is struck anywhere on its face.
                    let (aim, reach) = match &blocked_at {
                        Some((guard, Some((radius, _)))) => (*guard, reach.max(radius * 0.8)),
                        Some((guard, None)) => (*guard, *reach),
                        None => match aimed {
                            Some(zone) => (tg.aim_at(zone, aimed_point), *reach),
                            None => (tg.aim(*high, *liver), *reach),
                        },
                    };
                    let to_target = (aim - gt.translation()).with_y(0.0).normalize_or_zero();
                    // How close it came, where it lands on them and which way it pushes: a
                    // striker swept over the frame pushes along its motion (and on into
                    // them), a fist straight in.
                    let (close, point, push, edge) = match striker {
                        Some(st) => {
                            sweeping = true;
                            let m = animator.model[b];
                            let now = (
                                crate::ik::to_world(gt, m.transform_point(st.from)),
                                crate::ik::to_world(gt, m.transform_point(st.to)),
                            );
                            let was = attacker.swept.replace(now).unwrap_or(now);
                            let (close, mut point) = sweep(was, now, aim);
                            // On the body, the blade lands where it entered: on the surface
                            // of the part it passed nearest (for the rules to judge by), not
                            // on the blade beside what it was aimed at.
                            let mut edge = None;
                            if blocked_at.is_none() {
                                let (at, zone, dir) = entry(&tg.capsules, tg.knees, aim, was, now);
                                point = at;
                                // Landing on the window's first frame the blade has no motion
                                // yet: a stab goes in along the blade, a cut at them.
                                let along = (now.1 - now.0).normalize_or(to_target);
                                let dir = dir.unwrap_or(match st.kind {
                                    DamageKind::Pierce => along.as_vec3(),
                                    _ => (aim - at).normalize_or(to_target).as_vec3(),
                                });
                                edge = Some((at, dir, zone));
                            }
                            let motion = (now.1 - was.1).with_y(0.0).normalize_or_zero();
                            (
                                close,
                                point,
                                (motion + to_target * 0.6).normalize_or(to_target),
                                edge,
                            )
                        }
                        None => {
                            let fist = fist_of(&animator, gt, b);
                            let point = attacker.aim.as_ref().map_or(fist, |a| a.1);
                            (fist.distance(aim), point, to_target, None)
                        }
                    };
                    // A paired blow lands on its keyed frame: the two are lined up for it.
                    if close > reach as f64 && m.paired.is_none() {
                        continue;
                    }
                    attacker.landed.push(k);
                    let push = push.as_vec3();
                    // A guard takes a share of it, and the attacker bounces off; a guard
                    // raised just now turns a weapon aside, and a heavy blow breaks it.
                    let block = match &blocked_at {
                        Some((_, Some((_, shield)))) => shield,
                        _ => &moves.block,
                    };
                    let outcome = match &blocked_at {
                        None => Outcome::Hit,
                        Some(_) if striker.is_some() && tg.deflects => Outcome::Deflected,
                        Some(_) if *breaks_guard => Outcome::BrokeGuard,
                        Some(_) => Outcome::Blocked,
                    };
                    let (share, stop, pushback) = match outcome {
                        Outcome::Hit => {
                            attacker.hits += 1;
                            ((1.0, 1.0), 1.0, Vec3::ZERO)
                        }
                        Outcome::Blocked => {
                            attacker.blocked += 1;
                            // A weapon bounces off the guard: the swing runs back up its arc
                            // after the hitstop, and the move ends with it.
                            if striker.is_some() {
                                attacker.recoil = REBOUND_SECS + REBOUND_FADE;
                                animator.rebound(REBOUND_RATE, REBOUND_SECS, REBOUND_FADE);
                            }
                            (
                                (block.damage, block.impulse),
                                block.hitstop,
                                -push * block.pushback,
                            )
                        }
                        Outcome::Deflected => {
                            // Turned aside: nothing gets through, the blade flies back
                            // harder and the attacker is left open.
                            attacker.deflected += 1;
                            let (rate, secs, fade) = DEFLECT_REBOUND;
                            attacker.recoil = DEFLECT_RECOIL;
                            attacker.open = DEFLECT_RECOIL;
                            animator.rebound(rate, secs, fade);
                            ((0.0, 0.0), block.hitstop, -push * DEFLECT_SHOVE)
                        }
                        Outcome::BrokeGuard => {
                            // Through the guard: a block's share gets through and the blow
                            // carries on.
                            attacker.blocked += 1;
                            attacker.broke += 1;
                            ((block.damage, block.impulse), 1.0, Vec3::ZERO)
                        }
                    };
                    if outcome != Outcome::Deflected {
                        let blow = DamageEvent {
                            target: victim,
                            amount: *amount * share.0,
                            kind: striker.map_or(DamageKind::Blunt, |s| s.kind),
                            // Where it lands on them: the front of the face, chest, glove or
                            // shield, or where the blade crossed them.
                            point,
                            impulse: push * *impulse * share.1,
                            source: Some(entity),
                        };
                        // With a rules module judging melee, it writes the damage.
                        if judge.is_some() {
                            let (item, off_hand) =
                                striking_item(striker.as_ref(), bone, loadouts.get(entity).ok());
                            contacts.write(MeleeContact {
                                blow,
                                item,
                                off_hand,
                                entry: edge.map(|e| e.0),
                                edge_dir: edge.map(|e| e.1),
                                bone: edge.map(|e| zone_bone(e.2)),
                            });
                        } else {
                            damage.write(blow);
                        }
                    }
                    landed.push(Landed {
                        attacker: entity,
                        target: victim,
                        hitstop: *hitstop * stop,
                        pushback,
                        outcome,
                        push: push.with_y(0.0).normalize_or_zero(),
                        shove: block.pushback,
                        shield: matches!(blocked_at, Some((_, Some(_)))),
                    });
                }
            }
        }
        if !sweeping {
            attacker.swept = None;
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
    // back (stepping, where it has a hit reaction). The struck body shakes; a deflected
    // attacker shakes instead, and the defender's shield swats the blade aside. A blocking
    // defender slides back and is held in shield-stun; a broken guard is flung open.
    let mut animators = set.p2();
    // A paired move starts its victim's clip on the same frame (the victim is lined up and
    // held by `paired::hold_pairs`).
    for (by, victim, p) in pairs.drain(..) {
        if let Ok((mut a, reaction, _)) = animators.get_mut(victim)
            && let Some(clip) = library.clip(&p.victim_clip)
        {
            a.play_over(clip, 0.1, crate::paired::GET_UP_LEAD);
            if let Some(mut r) = reaction {
                r.stagger.0 = Vec3::ZERO;
            }
            commands.entity(victim).try_insert(Riposted {
                by,
                clip: p.victim_clip,
                distance: p.distance,
                align_secs: p.align_secs,
                ..Default::default()
            });
        }
    }
    for l in landed.drain(..) {
        if let Ok((mut a, reaction, defense)) = animators.get_mut(l.target) {
            a.freeze_for(l.hitstop);
            let mut shove = 0.0;
            let mut clip = None;
            match l.outcome {
                Outcome::Hit => a.shake_for((l.hitstop * SHAKE_PER_HITSTOP).min(SHAKE_MAX)),
                Outcome::Blocked => {
                    a.shake_for((l.hitstop * SHAKE_PER_HITSTOP).min(SHAKE_MAX));
                    // A blow on a shield drives the whole body back; one on the gloves only
                    // the struck arm (its hit reaction).
                    if l.shield {
                        shove = l.shove;
                    }
                }
                Outcome::Deflected => clip = Some(weapons::SHIELD_DEFLECT),
                Outcome::BrokeGuard => {
                    a.shake_for((l.hitstop * SHAKE_PER_HITSTOP).min(SHAKE_MAX));
                    shove = GUARD_BROKEN_SHOVE;
                    clip = Some(weapons::GUARD_BROKEN);
                }
            }
            if l.shield
                && let Some(c) = clip.and_then(|c| library.clip(c))
            {
                a.play_over(c, 0.04, 0.2);
            }
            if shove > 0.0
                && let Some(mut r) = reaction
            {
                r.knock(l.push * shove, 0.12);
            }
            if let Some(mut d) = defense {
                match l.outcome {
                    Outcome::Hit => {}
                    Outcome::Blocked => {
                        d.blocks += 1;
                        d.stun = d.stun.max(l.hitstop + SHIELD_STUN);
                    }
                    Outcome::Deflected => d.deflects += 1,
                    Outcome::BrokeGuard => {
                        d.breaks += 1;
                        d.broken = GUARD_BROKEN_SECS;
                    }
                }
            }
        }
        if let Ok((mut a, reaction, _)) = animators.get_mut(l.attacker) {
            a.freeze_for(l.hitstop);
            if l.outcome == Outcome::Deflected {
                a.shake_for(DEFLECT_SHAKE);
            }
            if l.pushback != Vec3::ZERO
                && let Some(mut r) = reaction
            {
                r.knock(l.pushback, 0.12);
            }
        }
    }
}

/// The closest a segment swept from `was` to `now` comes to `p`, and the point on it.
/// Where a blade swept from `was` to `now` entered the body: on the surface of the zone it
/// passed nearest (`zones`: their middles, world space, [`BodyZone::ALL`] order), toward the
/// A body part as a capsule (world): the zone, the ends of its axis and its radius.
#[derive(Debug, Clone, Copy)]
struct Capsule {
    zone: BodyZone,
    a: DVec3,
    b: DVec3,
    radius: f64,
    /// How far out from the axis the skin is, where a blade goes in (the radius, but for a
    /// knee: the region round the joint is wider than the leg).
    skin: f64,
}

/// Bones each zone's capsules run between, and their radius for a 1.75 m person (as the
/// combat module's zone capsules have them).
const CAPSULES: &[(BodyZone, &str, &str, f64)] = &[
    (BodyZone::Neck, "neck", "head", 0.06),
    (BodyZone::Chest, "chest", "neck", 0.15),
    (BodyZone::Chest, "upper_arm_l", "upper_arm_r", 0.1),
    (BodyZone::Belly, "hips", "chest", 0.15),
    (BodyZone::UpperArmL, "upper_arm_l", "lower_arm_l", 0.055),
    (BodyZone::UpperArmR, "upper_arm_r", "lower_arm_r", 0.055),
    (BodyZone::ForearmL, "lower_arm_l", "hand_l", 0.045),
    (BodyZone::ForearmR, "lower_arm_r", "hand_r", 0.045),
    (BodyZone::ThighL, "upper_leg_l", "lower_leg_l", 0.08),
    (BodyZone::ThighR, "upper_leg_r", "lower_leg_r", 0.08),
    (BodyZone::ShinL, "lower_leg_l", "foot_l", 0.055),
    (BodyZone::ShinR, "lower_leg_r", "foot_r", 0.055),
    (BodyZone::ShinL, "foot_l", "toe_l", 0.045),
    (BodyZone::ShinR, "foot_r", "toe_r", 0.045),
];

/// A body's capsules from where its bones are (world; `feet` is its origin): the skull on
/// past the head bone, the hands past the wrists.
fn capsules(at: &dyn Fn(&str, f64) -> DVec3, feet: DVec3) -> Vec<Capsule> {
    let head = at("head", 1.55);
    let scale = ((head.y - feet.y + 0.13) / 1.75).clamp(0.3, 2.0);
    let mut out: Vec<Capsule> = CAPSULES
        .iter()
        .map(|&(zone, from, to, r)| {
            let (a, mut b) = (at(from, 1.0), at(to, 1.0));
            // The chest stops below the neck's base rather than swallowing it.
            if zone == BodyZone::Chest && to == "neck" {
                b -= (b - a).normalize_or_zero() * r * scale * 0.8;
            }
            Capsule {
                zone,
                a,
                b,
                radius: r * scale,
                skin: r * scale,
            }
        })
        .collect();
    let up = (head - at("neck", 1.5)).normalize_or(DVec3::Y);
    out.push(Capsule {
        zone: BodyZone::Head,
        a: head + up * 0.04 * scale,
        b: head + up * 0.15 * scale,
        radius: 0.1 * scale,
        skin: 0.1 * scale,
    });
    for (zone, elbow, wrist) in [
        (BodyZone::HandL, "lower_arm_l", "hand_l"),
        (BodyZone::HandR, "lower_arm_r", "hand_r"),
    ] {
        let (e, w) = (at(elbow, 1.1), at(wrist, 1.0));
        let along = (w - e).normalize_or(DVec3::NEG_Y);
        out.push(Capsule {
            zone,
            a: w + along * 0.03 * scale,
            b: w + along * 0.1 * scale,
            radius: 0.045 * scale,
            skin: 0.045 * scale,
        });
    }
    // The knees: the region round each joint (the contract's), on a leg of the shin's girth.
    for (zone, joint) in [
        (BodyZone::KneeL, "lower_leg_l"),
        (BodyZone::KneeR, "lower_leg_r"),
    ] {
        let j = at(joint, 0.5);
        out.push(Capsule {
            zone,
            a: j,
            b: j,
            radius: BodyZone::KNEE_RADIUS_M as f64 * scale,
            skin: 0.06 * scale,
        });
    }
    out
}

/// How close a blade must come to the part it was aimed at to go in there, m.
const AIMED_NEAR_MISS: f64 = 0.06;

/// The closest points of segments `p0..p1` and `q0..q1`.
fn closest_between(p0: DVec3, p1: DVec3, q0: DVec3, q1: DVec3) -> (DVec3, DVec3) {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    let (s, t) = if a <= 1e-12 && e <= 1e-12 {
        (0.0, 0.0)
    } else if a <= 1e-12 {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= 1e-12 {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom > 1e-12 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    (p0 + d1 * s, q0 + d2 * t)
}

/// Where a blade swept from `was` to `now` (hilt, tip; world) went into the body, the part it
/// went into and which way that bit of blade was moving. Of the parts it passed through, the
/// one nearest the aim (the aimed part itself, when the blade crossed it); through none, the
/// one it came closest to. A thigh or shin struck within the knee's radius of the joint is the
/// knee.
fn entry(
    capsules: &[Capsule],
    knees: [DVec3; 2],
    aim: DVec3,
    was: (DVec3, DVec3),
    now: (DVec3, DVec3),
) -> (DVec3, BodyZone, Option<Vec3>) {
    const STEPS: usize = 6;
    // Per capsule: the gap to its surface, and where the blade went in (or came closest,
    // on its surface) with the bit of blade there.
    let gaps: Vec<(Capsule, f64, DVec3, DVec3)> = capsules
        .iter()
        .map(|c| {
            (0..=STEPS)
                .map(|k| {
                    let u = k as f64 / STEPS as f64;
                    let (h, t) = (was.0.lerp(now.0, u), was.1.lerp(now.1, u));
                    let (on_blade, on_axis) = closest_between(h, t, c.a, c.b);
                    let at = on_axis + (on_blade - on_axis).clamp_length_max(c.skin);
                    (*c, on_blade.distance(on_axis) - c.radius, at, on_blade)
                })
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .unwrap_or((*c, f64::INFINITY, c.a, c.a))
        })
        .collect();
    // How far the aim is outside a part (0 inside), and then how far from the aim the
    // blade went in: the part aimed at wins where the blade crossed it, and of a thigh and
    // shin both holding an aimed knee, the one it went into nearer the knee.
    let off_aim =
        |c: &Capsule| (closest_between(aim, aim, c.a, c.b).1.distance(aim) - c.radius).max(0.0);
    let rank = |g: &(Capsule, f64, DVec3, DVec3)| off_aim(&g.0) + 0.25 * g.2.distance(aim);
    // The part aimed at counts a near miss as through: the swing carries on into it.
    let through = gaps
        .iter()
        .filter(|g| g.1 <= 0.0 || (g.1 <= AIMED_NEAR_MISS && off_aim(&g.0) <= 0.0))
        .min_by(|x, y| rank(x).total_cmp(&rank(y)));
    let Some(&(capsule, _, at, on_blade)) =
        through.or_else(|| gaps.iter().min_by(|x, y| x.1.total_cmp(&y.1)))
    else {
        return (aim, BodyZone::Chest, None);
    };
    let zone = match (capsule.zone, capsule.zone.side()) {
        (BodyZone::ThighL | BodyZone::ShinL | BodyZone::ThighR | BodyZone::ShinR, Some(k))
            if at.distance(knees[k]) <= BodyZone::KNEE_RADIUS_M as f64 =>
        {
            [BodyZone::KneeL, BodyZone::KneeR][k]
        }
        (zone, _) => zone,
    };
    // How that point of the blade moved over the frame: hilt and tip motion, mixed by how
    // far along the blade it is.
    let blade = now.1 - now.0;
    let along = ((on_blade - now.0).dot(blade) / blade.length_squared().max(1e-12)).clamp(0.0, 1.0);
    let moved = (now.0 - was.0).lerp(now.1 - was.1, along);
    let dir = (moved.length_squared() > 1e-10).then(|| moved.normalize().as_vec3());
    (at, zone, dir)
}

/// The humanoid bone nearest a zone's middle.
fn zone_bone(zone: BodyZone) -> String {
    use BodyZone::*;
    let side = |stem: &str| format!("{stem}_{}", ["l", "r"][zone.side().unwrap_or(0)]);
    match zone {
        Head => "head".into(),
        Neck => "neck".into(),
        Chest => "chest".into(),
        Belly => "spine".into(),
        UpperArmL | UpperArmR => side("upper_arm"),
        ForearmL | ForearmR => side("lower_arm"),
        HandL | HandR => side("hand"),
        ThighL | ThighR => side("upper_leg"),
        KneeL | KneeR | ShinL | ShinR => side("lower_leg"),
    }
}

/// What struck and with which hand: a blade or shield on the left arm is the off hand's,
/// a fist, elbow, knee or kick is no item. Without a [`Loadout`], a guess from the striker.
fn striking_item(
    striker: Option<&Striker>,
    bone: &str,
    loadout: Option<&Loadout>,
) -> (ItemClass, bool) {
    let Some(striker) = striker else {
        return (ItemClass::None, false);
    };
    let off_hand = bone.ends_with("_l");
    let guess = if *striker == Striker::shield() {
        ItemClass::Shield
    } else if striker.to == Striker::greatsword(striker.kind).to {
        ItemClass::TwoHandedBlade
    } else {
        ItemClass::OneHandedBlade
    };
    let item = match loadout {
        Some(l) if off_hand => l.off,
        Some(l) => l.main,
        None => guess,
    };
    (if item == ItemClass::None { guess } else { item }, off_hand)
}

/// Farthest an aimed point may sit from its zone's middle, m: a scattered point can take
/// the blade off the part, onto its neighbour or past it, but not across the body.
const POINT_STRAY: f64 = 0.35;

fn sweep(was: (DVec3, DVec3), now: (DVec3, DVec3), p: DVec3) -> (f64, DVec3) {
    const STEPS: usize = 6;
    (0..=STEPS)
        .map(|k| {
            let u = k as f64 / STEPS as f64;
            let (a, b) = (was.0.lerp(now.0, u), was.1.lerp(now.1, u));
            let ab = b - a;
            let along = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            let q = a + ab * along;
            (q.distance(p), q)
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .unwrap_or((f64::INFINITY, p))
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
            let target = a.struck?;
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

#[cfg(test)]
mod aimed {
    use super::*;

    fn picks(set: &AttackSet, zone: BodyZone, skill: f32, foe: Foe, n: usize) -> Vec<Line> {
        let mut recent: Vec<Line> = Vec::new();
        let mut all = Vec::new();
        for _ in 0..n {
            let m = pick_line(set, zone, skill, &recent, &foe).expect("a line");
            let line = m.line.unwrap();
            all.push(line);
            recent.push(line);
            let extra = recent.len().saturating_sub(RECENT_LINES);
            recent.drain(..extra);
        }
        all
    }

    const FACING: Foe = Foe {
        open: false,
        guarding: false,
        turned_away: false,
        distance: 1.15,
    };

    /// Six presses at one zone give six different cuts at its height, and none comes within
    /// 60° of the one before it, for a master with either blade.
    #[test]
    fn a_master_cuts_a_new_line_every_press() {
        for set in [AttackSet::sword_shield(), AttackSet::greatsword()] {
            for zone in [BodyZone::KneeL, BodyZone::Neck, BodyZone::UpperArmR] {
                let lines = picks(&set, zone, 0.9, FACING, 6);
                let angles: Vec<f32> = lines.iter().map(|l| l.angle).collect();
                assert!(
                    lines
                        .iter()
                        .all(|l| !l.stab && l.height == zone_height(zone))
                );
                for (k, a) in angles.iter().enumerate() {
                    assert!(!angles[..k].contains(a), "{zone:?}: {angles:?}");
                }
                for w in lines.windows(2) {
                    let turn = w[0].travel().angle_between(w[1].travel()).to_degrees();
                    assert!(turn >= 60.0, "{zone:?}: {angles:?}");
                }
            }
        }
    }

    /// A rookie only has its three lines; an open foe, or one guarding high, gets the point
    /// at the belly; one that turned away gets it too.
    #[test]
    fn rookies_know_three_lines_and_openings_get_a_stab() {
        let set = AttackSet::sword_shield();
        let rookie = picks(&set, BodyZone::Chest, 0.1, FACING, 6);
        for l in &rookie {
            assert!(known_lines(0.1).contains(&l.angle) && !l.stab, "{rookie:?}");
        }
        let distinct: HashSet<i32> = rookie.iter().map(|l| l.angle as i32).collect();
        assert_eq!(distinct.len(), 3);
        for foe in [
            Foe {
                open: true,
                ..FACING
            },
            Foe {
                guarding: true,
                ..FACING
            },
            Foe {
                turned_away: true,
                ..FACING
            },
            Foe {
                distance: 0.6,
                ..FACING
            },
        ] {
            let first = picks(&set, BodyZone::Belly, 0.5, foe, 2);
            assert!(first[0].stab, "{foe:?}: {first:?}");
            // Never two stabs in a row.
            assert!(!first[1].stab, "{foe:?}: {first:?}");
        }
        // Guarding high doesn't make a stab pay at the head.
        let high = picks(
            &set,
            BodyZone::Head,
            0.5,
            Foe {
                guarding: true,
                ..FACING
            },
            1,
        );
        assert!(!high[0].stab);
        // Nor a turned back at a hamstring.
        let behind = Foe {
            turned_away: true,
            ..FACING
        };
        assert!(!picks(&set, BodyZone::ThighR, 0.5, behind, 1)[0].stab);
    }
}
