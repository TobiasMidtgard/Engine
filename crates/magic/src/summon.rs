//! Summons (round-5-plan.md sections 1-3): creatures a Summon spell puts into the world.
//!
//! The weave picks the creature ([`kind_of`]): stone makes a golem that walks and punches,
//! an element without stone an elemental that floats and shoots bolts of its weave, heal a wisp
//! that heals the weakest ally. Skyrim's conjuration sets the rules: a summon lasts a minute
//! (Duration scales it), stands where the caster points within range, and a caster keeps one of
//! each kind (two from Master up); a new one replaces the oldest, which falls.
//!
//! A minion is a body in the world through the engine contracts only: `Vitality` (spells and
//! blows hurt it), a kinematic collider (bolts burst on it, it shoves what it walks into), and
//! a `MagicCaster` on its summoner's side, so the usual friend and foe rules spare its allies.
//! It fights through the hit queue like any spell, so element reactions apply.
//!
//! The golem is twenty rocks posed each step, with no skeleton and no clips:
//! * feet stay planted and step one at a time when the spot under the hip (led by the body's
//!   velocity) is too far, along an arc onto the ground found by a ray (Vulkan's foot
//!   placement, Roblox IKPF);
//! * legs and arms are two-bone IK ([`two_bone`]); the pelvis bobs and sways over the planted
//!   foot; arms swing against the legs;
//! * punches and the slam are key poses blended with smoothstep ([`stance`]);
//! * it climbs out of the floor through gravel, and falls apart into tumbling rocks.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    MaterialPreset, MaterialRef, MeshRef, MeshRenderer, PointLight, Shape, StandardMaterial,
};
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody, Velocity,
};
use engine_world::{Factions, Gravity, Statuses, WorldShift};

use crate::caster::{MagicCaster, Vitality};
use crate::compile::{CastContext, Spell, compile};
use crate::design::{CastType, SpellDesign, TargetFilter};
use crate::effects::ForceDir;
use crate::looks::{LookCmd, Role};
use crate::runtime::{
    Allegiance, CastInfo, FieldKind, HitQueue, MagicQueue, PendingHit, Spawn, field, ground_below,
    release, roll,
};
use crate::schools::{Element, Mastery};

/// What a summon makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MinionKind {
    /// Walking rock that punches and slams (Shaping's stone golem; with an element, the lava
    /// or frost golem).
    Golem,
    /// A floating core in a shell of shards that shoots bolts of its weave (Evocation).
    Elemental,
    /// A healing light (Vitalism).
    Wisp,
}

/// The creature a summon spell makes, or None (it casts as an area, as before).
pub fn kind_of(spell: &Spell) -> Option<MinionKind> {
    if spell.payload.stone_m > 0.0 {
        Some(MinionKind::Golem)
    } else if spell.element().is_some() {
        Some(MinionKind::Elemental)
    } else if spell.payload.heal > 0.0 {
        Some(MinionKind::Wisp)
    } else {
        None
    }
}

/// How many of each kind a caster keeps: Skyrim's Twin Souls from Master up.
pub fn limit(mastery: Mastery) -> usize {
    if mastery >= Mastery::Master { 2 } else { 1 }
}

// ------------------------------------------------------------------------------- numbers

/// Seconds a golem takes to climb out of the floor.
pub const RISE_SECS: f32 = 1.8;
/// A golem's walk, m/s (times its size).
pub const WALK: f32 = 1.3;
/// How fast it turns, rad/s.
pub const TURN: f32 = 2.4;
/// How far a golem looks for a foe, and how far from its summoner it will chase one, m.
pub const AGGRO_M: f32 = 18.0;
pub const LEASH_M: f32 = 25.0;
/// An elemental's and a wisp's reach, m.
pub const SHOOT_M: f32 = 20.0;
pub const HOVER_M: f32 = 2.2;
/// An elemental fights from this far off its foe, and strays at most this far from its
/// summoner, m.
pub const ENGAGE_M: f32 = 5.5;
pub const GUARD_M: f32 = 12.0;
/// A punch: wind-up, the moment it lands, and the whole swing, s.
pub const PUNCH_SECS: f32 = 1.1;
pub const PUNCH_HIT: f32 = 0.52;
/// A slam: both fists overhead, then down.
pub const SLAM_SECS: f32 = 1.5;
pub const SLAM_HIT: f32 = 0.76;
pub const SLAM_M: f32 = 2.8;
/// Seconds between an elemental's bolts and a wisp's heals; an elemental gathers for the
/// first part of it.
pub const SHOOT_EVERY: f32 = 1.8;
pub const GATHER_SECS: f32 = 0.5;
pub const HEAL_EVERY: f32 = 1.2;
/// Seconds rubble lies before it sinks, and how long sinking takes.
pub const RUBBLE_SECS: f32 = 4.0;
pub const SINK_SECS: f32 = 1.5;

// Golem proportions at size 1 (a little over 2.6 m tall), m.
const HIP_Y: f32 = 1.12;
const HIP_X: f32 = 0.36;
const THIGH: f32 = 0.62;
const SHIN: f32 = 0.6;
const ANKLE_Y: f32 = 0.14;
const UPPER: f32 = 0.66;
const FORE: f32 = 0.6;
const HUNCH: f32 = 0.18;
/// A step: how long it takes, how high the foot lifts, and how far the foot may stray first.
const STEP_SECS: f32 = 0.42;
const STEP_H: f32 = 0.28;
const STEP_FAR: f32 = 0.35;
const STEP_SETTLE: f32 = 0.14;
const LEAD_SECS: f32 = 0.32;

/// The golem's pieces, in order.
pub const GOLEM_PARTS: usize = 20;
const PELVIS: usize = 0;
const TORSO: usize = 1;
const HEAD: usize = 2;
/// Thigh, shin, foot; left then right.
const LEG: [usize; 2] = [3, 6];
/// Upper arm, forearm, fist; left then right.
const ARM: [usize; 2] = [9, 12];
const PLATE: [usize; 2] = [15, 16];
const CORE: usize = 17;
const EYE: [usize; 2] = [18, 19];

/// Rock sizes of the golem's pieces at size 1 (width, length, depth): the length runs along
/// the bone.
const GOLEM_SIZES: [[f32; 3]; 17] = [
    [0.95, 0.42, 0.62],
    [1.45, 1.05, 0.95],
    [0.5, 0.46, 0.52],
    [0.42, THIGH, 0.42],
    [0.46, SHIN, 0.5],
    [0.5, 0.26, 0.72],
    [0.42, THIGH, 0.42],
    [0.46, SHIN, 0.5],
    [0.5, 0.26, 0.72],
    [0.42, UPPER, 0.42],
    [0.5, FORE, 0.5],
    [0.56, 0.5, 0.56],
    [0.42, UPPER, 0.42],
    [0.5, FORE, 0.5],
    [0.56, 0.5, 0.56],
    [0.62, 0.42, 0.66],
    [0.62, 0.42, 0.66],
];

/// Shards around an elemental's core.
pub const SHARDS: usize = 11;
/// An elemental's shell, m across its shard orbit (times its size): about an atronach's
/// presence, so it reads as a creature beside a 2.5 m golem rather than a spark.
pub const SHELL_M: f32 = 1.8;

// ------------------------------------------------------------------------------- state

/// What a minion is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    /// Climbing out of the floor (golems).
    Rise {
        t: f32,
    },
    /// Free to move and pick its next attack.
    Ready,
    Punch {
        t: f32,
        right: bool,
    },
    Slam {
        t: f32,
    },
    /// Gathering a bolt (elementals).
    Shoot {
        t: f32,
    },
}

/// A golem's foot: planted where it landed, or stepping from one spot to the next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Foot {
    pub at: DVec3,
    pub from: DVec3,
    pub to: DVec3,
    /// Progress through a step (None: planted).
    pub t: Option<f32>,
}

impl Foot {
    pub fn planted(at: DVec3) -> Self {
        Self {
            at,
            from: at,
            to: at,
            t: None,
        }
    }

    /// Where the foot is now: on the ground, or along its arc.
    pub fn now(&self, lift: f32) -> DVec3 {
        match self.t {
            None => self.at,
            Some(t) => {
                let e = smooth(t);
                self.from.lerp(self.to, e as f64) + DVec3::Y * (lift * (PI * t).sin()) as f64
            }
        }
    }
}

/// A summoned creature.
#[derive(Component, Debug, Clone)]
pub struct Minion {
    pub kind: MinionKind,
    /// The summon cast: the spell, its summoner as caster, side and faction.
    pub cast: CastInfo,
    /// Its punch, bolt or heal.
    pub attack: Arc<Spell>,
    /// A golem's slam.
    pub slam: Option<Arc<Spell>>,
    pub age: f32,
    pub life: f32,
    /// Body size (1: a 2.6 m golem).
    pub size: f32,
    /// Golem: the ground under its pelvis. Others: where it floats.
    pub pos: DVec3,
    pub yaw: f32,
    pub vel: Vec3,
    pub target: Option<Entity>,
    /// Seconds until it looks around for a foe (or a hurt ally) again.
    pub look_in: f32,
    pub cooldown: f32,
    pub act: Act,
    pub attacks: u32,
    pub feet: [Foot; 2],
    /// The jolt of the last footfall, fading.
    pub dip: f32,
    /// Shards' and motes' turn.
    pub spin: f32,
    /// Its pieces (children), in [`GOLEM_PARTS`] order for a golem.
    pub parts: Vec<Entity>,
    /// Which side of its summoner it keeps (+1 right, -1 left).
    pub flank: f32,
}

/// A piece of a minion's body.
#[derive(Component, Debug, Clone, Copy)]
pub struct MinionPart {
    pub index: usize,
    /// Rock size, m (zero for glowing pieces, which don't become rubble).
    pub size: Vec3,
}

/// A piece of a fallen minion, tumbling, then sinking away.
#[derive(Component, Debug, Clone, Copy)]
pub struct Rubble {
    pub age: f32,
    pub life: f32,
}

/// A summon to put into the world (from the flush).
#[derive(Debug, Clone)]
pub struct NewMinion {
    pub kind: MinionKind,
    pub cast: CastInfo,
    /// The ground where it stands (or under where it floats).
    pub at: DVec3,
    pub facing: Vec3,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smoothstep from `a` to `b`.
fn ramp(a: f32, b: f32, t: f32) -> f32 {
    smooth((t - a) / (b - a).max(1e-4))
}

fn yaw_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// The middle joint and the end of a two-bone limb from `a` toward `target`, bones `l1` and
/// `l2` long, bending toward `pole`. A target out of reach is reached for in a straight line.
pub fn two_bone(a: Vec3, target: Vec3, l1: f32, l2: f32, pole: Vec3) -> (Vec3, Vec3) {
    let to = target - a;
    let dir = to.normalize_or(Vec3::NEG_Y);
    let d = to.length().clamp((l1 - l2).abs() + 1e-3, (l1 + l2) * 0.999);
    let x = (l1 * l1 - l2 * l2 + d * d) / (2.0 * d);
    let h = (l1 * l1 - x * x).max(0.0).sqrt();
    let bend = (pole - dir * pole.dot(dir)).normalize_or(dir.any_orthonormal_vector());
    (a + dir * x + bend * h, a + dir * d)
}

// ------------------------------------------------------------------------------- golem pose

/// How the golem holds itself this step, at size 1: torso twist (rad), lunge forward (m),
/// extra bend (rad), pelvis bob and sway (m), each fist's offset from its shoulder (root
/// axes), and how lit its eyes are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stance {
    pub twist: f32,
    pub lunge: f32,
    pub pitch: f32,
    pub bob: f32,
    pub sway: f32,
    pub fists: [Vec3; 2],
    pub eyes: f32,
}

/// The golem's stance for an act, with its feet (root-local, size 1) driving the walk's arm
/// swing, bob and sway.
pub fn stance(act: Act, feet: [Vec3; 2], stepping: Option<(usize, f32)>, dip: f32) -> Stance {
    let reach = UPPER + FORE;
    // Walking: arms hang and swing against the opposite leg.
    let swing = |other: Vec3| (0.7 * other.z).clamp(-0.45, 0.45);
    let hang = [
        Vec3::new(-0.12, -reach * 0.93, swing(feet[1])),
        Vec3::new(0.12, -reach * 0.93, swing(feet[0])),
    ];
    let (lift, sway) = match stepping {
        Some((i, t)) => {
            let s = (PI * t).sin();
            // Over the planted foot: away from the stepping one.
            (s, if i == 0 { 0.06 } else { -0.06 } * s)
        }
        None => (0.0, 0.0),
    };
    let mut st = Stance {
        twist: 0.0,
        lunge: 0.0,
        pitch: 0.0,
        bob: 0.06 * lift - 0.03 - dip,
        sway,
        fists: hang,
        eyes: 1.0,
    };
    match act {
        Act::Rise { t } => {
            // Hauling itself out: arms up over the rim of the hole, eyes lighting at the top.
            let up = 1.0 - ramp(RISE_SECS * 0.6, RISE_SECS, t);
            for (i, f) in st.fists.iter_mut().enumerate() {
                let side = if i == 0 { -1.0 } else { 1.0 };
                *f = f.lerp(Vec3::new(0.15 * side, 0.55, -0.45), up);
            }
            st.pitch = 0.3 * up;
            st.eyes = ramp(RISE_SECS * 0.8, RISE_SECS, t);
        }
        Act::Punch { t, right } => {
            let (i, side) = if right { (1, 1.0) } else { (0, -1.0) };
            let cock = ramp(0.0, 0.45, t) * (1.0 - ramp(0.45, 0.58, t));
            let strike = ramp(0.42, 0.58, t) * (1.0 - ramp(0.75, PUNCH_SECS, t));
            st.twist = side * (-0.35 * cock + 0.4 * strike);
            st.lunge = -0.25 * strike;
            st.pitch = 0.12 * strike;
            let cocked = Vec3::new(0.05 * side, -0.45, 0.5);
            let thrown = Vec3::new(-0.3 * side, -0.2, -reach * 0.97);
            st.fists[i] = st.fists[i].lerp(cocked, cock).lerp(thrown, strike);
            let guard = Vec3::new(-0.15 * side, -0.75, -0.35);
            st.fists[1 - i] = st.fists[1 - i].lerp(guard, cock.max(strike));
        }
        Act::Slam { t } => {
            let raise = ramp(0.0, 0.6, t) * (1.0 - ramp(0.6, 0.8, t));
            let smash = ramp(0.6, 0.78, t) * (1.0 - ramp(1.05, SLAM_SECS, t));
            st.pitch = -0.25 * raise + 0.5 * smash;
            st.lunge = -0.15 * smash;
            st.bob -= 0.2 * smash;
            for (i, f) in st.fists.iter_mut().enumerate() {
                let side = if i == 0 { -1.0 } else { 1.0 };
                let over = Vec3::new(-0.25 * side, 0.85, 0.15);
                let down = Vec3::new(-0.45 * side, -1.75, -1.0);
                *f = f.lerp(over, raise).lerp(down, smash);
            }
        }
        Act::Ready | Act::Shoot { .. } => {}
    }
    st
}

/// Each piece's place and turn under the golem's root (meters at size `k`), from its stance
/// and its ankles (root-local, already at size `k`).
pub fn pose_golem(st: &Stance, ankles: [Vec3; 2], k: f32) -> [(Vec3, Quat); GOLEM_PARTS] {
    let mut out = [(Vec3::ZERO, Quat::IDENTITY); GOLEM_PARTS];
    let ry = Quat::from_rotation_y;
    let pelvis = Vec3::new(st.sway, HIP_Y + st.bob, st.lunge) * k;
    let r_pelvis = ry(st.twist * 0.4);
    let r_torso = ry(st.twist) * Quat::from_rotation_x(-(HUNCH + st.pitch));
    let r_head = r_torso * Quat::from_rotation_x(HUNCH + 0.5 * st.pitch);
    out[PELVIS] = (pelvis, r_pelvis);
    out[TORSO] = (pelvis + r_torso * Vec3::new(0.0, 0.68, 0.0) * k, r_torso);
    let head = pelvis + r_torso * Vec3::new(0.0, 1.36, -0.22) * k;
    out[HEAD] = (head, r_head);
    out[CORE] = (pelvis + r_torso * Vec3::new(0.0, 0.74, -0.44) * k, r_torso);
    let bone = |a: Vec3, b: Vec3| {
        (
            (a + b) * 0.5,
            Quat::from_rotation_arc(Vec3::Y, (b - a).normalize_or(Vec3::NEG_Y)),
        )
    };
    for i in 0..2 {
        let side = if i == 0 { -1.0 } else { 1.0 };
        // Leg: hip to ankle, knee forward and a little out.
        let hip = pelvis + r_pelvis * Vec3::new(side * HIP_X, -0.06, 0.0) * k;
        let (knee, ankle) = two_bone(
            hip,
            ankles[i],
            THIGH * k,
            SHIN * k,
            Vec3::new(0.15 * side, 0.0, -1.0),
        );
        out[LEG[i]] = bone(hip, knee);
        out[LEG[i] + 1] = bone(knee, ankle);
        out[LEG[i] + 2] = (ankle + Vec3::new(0.0, -0.04, -0.14) * k, Quat::IDENTITY);
        // Arm: shoulder to fist, elbow back and out.
        let shoulder = pelvis + r_torso * Vec3::new(side * 0.86, 1.0, 0.05) * k;
        let (elbow, wrist) = two_bone(
            shoulder,
            shoulder + st.fists[i] * k,
            UPPER * k,
            FORE * k,
            Vec3::new(0.5 * side, -0.2, 1.0),
        );
        out[ARM[i]] = bone(shoulder, elbow);
        out[ARM[i] + 1] = bone(elbow, wrist);
        let fore = (wrist - elbow).normalize_or(Vec3::NEG_Y);
        out[ARM[i] + 2] = (
            wrist + fore * 0.16 * k,
            Quat::from_rotation_arc(Vec3::Y, fore),
        );
        out[PLATE[i]] = (
            shoulder + r_torso * Vec3::new(side * 0.08, 0.16, 0.0) * k,
            r_torso * Quat::from_rotation_z(-0.3 * side),
        );
        out[EYE[i]] = (
            head + r_head * Vec3::new(side * 0.12, 0.04, -0.25) * k,
            r_head,
        );
    }
    out
}

/// Move the feet: finish steps under way (returning where each landed), and start one when a
/// foot has strayed too far from under its hip, only while the other foot is down.
#[allow(clippy::too_many_arguments)]
pub fn step_feet(
    feet: &mut [Foot; 2],
    pos: DVec3,
    yaw: f32,
    vel: Vec3,
    k: f32,
    dt: f32,
    held: bool,
    ground: impl Fn(DVec3) -> DVec3,
) -> Vec<DVec3> {
    let mut landed = Vec::new();
    let secs = STEP_SECS * k.sqrt();
    for f in feet.iter_mut() {
        if let Some(t) = f.t {
            let t = t + dt / secs;
            if t >= 1.0 {
                f.at = f.to;
                f.t = None;
                landed.push(f.at);
            } else {
                f.t = Some(t);
            }
        }
    }
    if held || feet.iter().any(|f| f.t.is_some()) {
        return landed;
    }
    let rot = Quat::from_rotation_y(yaw);
    let lead = vel * LEAD_SECS;
    let moving = vel.length() > 0.15;
    let ideal = |i: usize| {
        let side = if i == 0 { -1.0 } else { 1.0 };
        pos + (rot * Vec3::new(side * HIP_X * k, 0.0, 0.0) + lead).as_dvec3()
    };
    let far = if moving { STEP_FAR } else { STEP_SETTLE } * k;
    let err = |i: usize| {
        let d = ideal(i) - feet[i].at;
        DVec3::new(d.x, 0.0, d.z).length() as f32
    };
    let i = if err(0) >= err(1) { 0 } else { 1 };
    if err(i) > far {
        let to = ground(ideal(i));
        feet[i].from = feet[i].at;
        feet[i].to = to;
        feet[i].t = Some(0.0);
    }
    landed
}

// ------------------------------------------------------------------------------- spawning

/// The attack a minion makes: a golem's punch (and slam), an elemental's bolt of its weave, a
/// wisp's heal. Never on its own side.
pub fn attacks(kind: MinionKind, spell: &Spell, k: f32) -> (Arc<Spell>, Option<Arc<Spell>>) {
    let blow = |dir: ForceDir, impulse: f32, damage: f32| {
        let mut s = spell.clone();
        s.cast = CastType::Touch;
        s.stages.clear();
        s.payload.stone_m = 0.0;
        s.payload.stone_secs = 0.0;
        s.payload.heal = 0.0;
        s.payload.ward = 0.0;
        s.payload.damage = spell.power.damage() * damage * k;
        s.payload.force = Some((dir, impulse * k));
        s.carrier.radius = 0.4;
        s.filter = TargetFilter::Enemies;
        Arc::new(s)
    };
    match kind {
        // A shove that sends a dummy a few metres (4.5 m/s on 50 kg), and a slam that throws
        // what stands near up off the floor.
        MinionKind::Golem => (
            blow(ForceDir::Away, 225.0, 0.8),
            Some(blow(ForceDir::Up, 170.0, 1.0)),
        ),
        MinionKind::Elemental => {
            let keys: Vec<&str> = spell.weave.elements.iter().map(|e| e.key()).collect();
            let bolt = compile(
                &SpellDesign::new(&keys, CastType::Bolt),
                CastContext::new(spell.mastery, spell.power),
            )
            .map(|s| (*s).clone())
            .unwrap_or_else(|_| {
                let mut s = spell.clone();
                s.cast = CastType::Bolt;
                s.carrier.speed = 26.0;
                s.carrier.range = 45.0;
                s.carrier.radius = 0.16;
                s.carrier.count = 1;
                s
            });
            let mut bolt = bolt;
            bolt.filter = TargetFilter::Enemies;
            (Arc::new(bolt), None)
        }
        MinionKind::Wisp => {
            let mut s = spell.clone();
            s.cast = CastType::OnSelf;
            s.stages.clear();
            s.payload.heal = spell.power.damage() * 0.35;
            s.filter = TargetFilter::Allies;
            (Arc::new(s), None)
        }
    }
}

fn rock(size: Vec3, seed: u64, material: StandardMaterial) -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Shape(Shape::Rock {
            size: size.to_array(),
            seed: (seed % 100_000) as u32,
            detail: 2,
        }),
        MaterialRef::Inline(material),
    )
}

fn glow(radius: f32, color: [f32; 3], nits: f32) -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Shape(Shape::Sphere {
            radius,
            segments: 16,
        }),
        MaterialRef::Inline(
            StandardMaterial::default()
                .with_tint(color[0], color[1], color[2])
                .with_emissive(color, nits),
        ),
    )
    .without_shadows()
}

/// The stone a minion is made of, by its weave's first element: plain rock; dark basalt
/// glowing from inside (fire); frosted pale stone (frost); stone tinted by the element.
pub fn body_material(element: Option<Element>) -> StandardMaterial {
    let rock = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
    match element {
        None => rock,
        Some(Element::Fire) => rock
            .with_tint(0.16, 0.13, 0.12)
            .with_emissive([1.0, 0.3, 0.05], 1.6),
        Some(Element::Frost) => rock
            .with_tint(0.78, 0.88, 0.98)
            .with_roughness(0.35)
            .with_emissive([0.4, 0.65, 1.0], 0.12),
        Some(e) => {
            let c = e.color();
            rock.with_tint(0.3 + 0.4 * c[0], 0.3 + 0.4 * c[1], 0.3 + 0.4 * c[2])
                .with_emissive(c, 0.15)
        }
    }
}

/// The glow of a minion's heart and eyes: the weave's colour, or amber for plain stone.
pub fn glow_color(spell: &Spell) -> [f32; 3] {
    if spell.weave.elements.is_empty() && spell.payload.heal <= 0.0 {
        [1.0, 0.58, 0.18]
    } else {
        spell.weave.color
    }
}

/// Body size from the summon's power: an Adept's golem is about 2.3 m, an Ascended's 2.7 m.
pub fn size_of(spell: &Spell, scale: f32) -> f32 {
    ((0.75 + 0.2 * spell.power.size()) * scale.sqrt()).clamp(0.85, 1.5)
}

/// Put a minion into the world (from the flush): its root, its pieces, and its looks.
pub fn spawn_minion(commands: &mut Commands, m: NewMinion, looks: &mut Vec<LookCmd>) -> Entity {
    let spell = m.cast.spell.clone();
    let k = size_of(&spell, m.cast.scale);
    let (attack, slam) = attacks(m.kind, &spell, k);
    let yaw = yaw_of(m.facing);
    let seed = m.cast.seed as u64;
    let color = glow_color(&spell);
    let element = spell.element();
    let life = spell.carrier.duration.max(1.0);
    let (pos, health) = match m.kind {
        MinionKind::Golem => (m.at, 400.0 * k),
        MinionKind::Elemental => (m.at + DVec3::Y * HOVER_M as f64, 150.0 * k),
        MinionKind::Wisp => (m.at + DVec3::Y * 2.0, 0.0),
    };
    let start = match m.kind {
        // It climbs out of the floor: it starts below it.
        MinionKind::Golem => pos - DVec3::Y * (2.9 * k) as f64,
        _ => pos,
    };
    let rot = Quat::from_rotation_y(yaw);
    let name = match m.kind {
        MinionKind::Golem => "Summoned golem",
        MinionKind::Elemental => "Summoned elemental",
        MinionKind::Wisp => "Summoned wisp",
    };
    let root = commands
        .spawn((
            Name::new(name),
            Transform::from_translation(start).with_rotation(rot),
            MagicCaster {
                learns: false,
                side: m.cast.side,
                seed,
                hand: Vec3::ZERO,
                mana: 0.0,
                max_mana: 0.0,
                mana_regen: 0.0,
                ..MagicCaster::default()
            },
        ))
        .id();
    if let Some(f) = m.cast.faction {
        commands.entity(root).insert(f);
    }
    if health > 0.0 {
        commands.entity(root).insert(Vitality::new(health));
    }
    let mut parts = Vec::new();
    let mut part = |commands: &mut Commands, index: usize, size: Vec3, r: MeshRenderer| {
        let id = commands
            .spawn((
                Name::new("Minion piece"),
                Transform::default(),
                r,
                MinionPart { index, size },
                ChildOf(root),
            ))
            .id();
        parts.push(id);
    };
    match m.kind {
        MinionKind::Golem => {
            let mat = body_material(element);
            for (i, s) in GOLEM_SIZES.iter().enumerate() {
                let size = Vec3::from_array(*s) * k;
                part(
                    commands,
                    i,
                    size,
                    rock(size, seed.wrapping_add(i as u64 * 7919), mat),
                );
            }
            part(commands, CORE, Vec3::ZERO, glow(0.17 * k, color, 40.0));
            for e in EYE {
                part(commands, e, Vec3::ZERO, glow(0.055 * k, color, 70.0));
            }
            // A low glow from its heart on the floor around it.
            commands.entity(root).insert(PointLight {
                color,
                intensity_lm: 900.0,
                range: 6.0,
                radius: 0.2,
            });
            // A golem woven with an element wears it: its element's aura rides its chest, so
            // flames lick a lava golem and frost smokes off a frost one.
            if element.is_some() {
                looks.push(LookCmd::Attach {
                    entity: parts[TORSO],
                    look: spell.weave.key.clone(),
                    role: Role::Aura,
                    scale: 1.3 * k,
                    color: spell.weave.color,
                    power_j: spell.power.energy_j as f32 * 0.2,
                });
            }
        }
        MinionKind::Elemental => {
            part(commands, 0, Vec3::ZERO, glow(0.3 * k, color, 30.0));
            let mat = body_material(element);
            for i in 0..SHARDS {
                let s = 0.19 + 0.17 * roll(seed, 40 + i as u64);
                let size = Vec3::new(s, s * 1.6, s) * k;
                part(
                    commands,
                    1 + i,
                    size,
                    rock(size, seed.wrapping_add(i as u64 * 104_729), mat),
                );
            }
            commands.entity(root).insert((
                PointLight {
                    color,
                    intensity_lm: 4000.0,
                    range: 10.0,
                    radius: 0.4,
                },
                Collider {
                    shape: ColliderShape::Sphere { radius: 0.8 * k },
                    ..Default::default()
                },
                RigidBody {
                    kind: BodyKind::Kinematic,
                    ..Default::default()
                },
            ));
            looks.push(LookCmd::Attach {
                entity: root,
                look: spell.weave.key.clone(),
                role: Role::Aura,
                // Particles' auras are sized for a 1.3 m body at scale 1.
                scale: SHELL_M / 1.3 * k,
                color: spell.weave.color,
                power_j: spell.power.energy_j as f32 * 0.2,
            });
        }
        MinionKind::Wisp => {
            part(commands, 0, Vec3::ZERO, glow(0.22, color, 120.0));
            for i in 0..3 {
                part(commands, 1 + i, Vec3::ZERO, glow(0.05, color, 70.0));
            }
            commands.entity(root).insert(PointLight {
                color,
                intensity_lm: 1400.0,
                range: 5.0,
                radius: 0.1,
            });
        }
    }
    // Gravel bursts out where it rises; an elemental or wisp flares into being.
    looks.push(LookCmd::Burst {
        at: m.at,
        up: Vec3::Y,
        look: if m.kind == MinionKind::Golem {
            "stone".into()
        } else {
            spell.weave.key.clone()
        },
        role: Role::Impact,
        scale: 1.2 * k,
        color: spell.weave.color,
    });
    let side = |s: f32| pos + (rot * Vec3::new(s * HIP_X * k, 0.0, 0.0)).as_dvec3();
    commands.entity(root).insert(Minion {
        kind: m.kind,
        cast: m.cast,
        attack,
        slam,
        age: 0.0,
        life,
        size: k,
        pos,
        yaw,
        vel: Vec3::ZERO,
        target: None,
        look_in: 0.0,
        cooldown: 0.0,
        act: if m.kind == MinionKind::Golem {
            Act::Rise { t: 0.0 }
        } else {
            Act::Ready
        },
        attacks: 0,
        feet: [Foot::planted(side(-1.0)), Foot::planted(side(1.0))],
        dip: 0.0,
        spin: roll(seed, 3) * TAU,
        parts,
        flank: if roll(seed, 5) < 0.5 { -1.0 } else { 1.0 },
    });
    root
}

/// A golem's body for physics once it stands: a capsule from knee to shoulder.
pub fn golem_collider(k: f32) -> (Collider, RigidBody) {
    (
        Collider {
            shape: ColliderShape::Compound(vec![(
                Vec3::new(0.0, 1.35 * k, 0.0),
                Quat::IDENTITY,
                ColliderShape::Capsule {
                    radius: 0.55 * k,
                    half_height: 0.75 * k,
                },
            )]),
            density: 2600.0,
            ..Default::default()
        },
        RigidBody {
            kind: BodyKind::Kinematic,
            ..Default::default()
        },
    )
}

// ------------------------------------------------------------------------------- living

/// A body a minion might act on: who, where, how hurt, whose side.
struct Seen {
    e: Entity,
    at: DVec3,
    hurt: f32,
    friend: bool,
}

/// Minions live: the oldest of a kind past its summoner's limit falls; each picks a foe (or a
/// hurt ally), moves, attacks, and is posed; at the end of its time or its health it falls.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_minions(
    time: Res<Time>,
    gravity: Res<Gravity>,
    queries: Res<PhysicsQueries>,
    factions: Option<Res<Factions>>,
    mut minions: Query<(
        Entity,
        &mut Minion,
        &mut Transform,
        Option<&Vitality>,
        Option<&LocalTimeScale>,
        Option<&Statuses>,
    )>,
    bodies: Query<(Entity, &GlobalTransform, &Vitality, Allegiance)>,
    anchors: Query<&GlobalTransform>,
    mut parts: Query<(&MinionPart, &mut Transform), Without<Minion>>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    if minions.is_empty() {
        return;
    }
    // Past the limit: the oldest of a summoner's kind goes.
    let mut kept: Vec<(Option<Entity>, MinionKind, f32, Entity, Mastery)> = minions
        .iter()
        .filter(|(_, m, ..)| m.age < m.life)
        .map(|(e, m, ..)| (m.cast.caster, m.kind, m.age, e, m.cast.spell.mastery))
        .collect();
    kept.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut retire = Vec::new();
    for (i, a) in kept.iter().enumerate() {
        let younger = kept[..i]
            .iter()
            .filter(|b| b.0 == a.0 && b.1 == a.1)
            .count();
        if a.0.is_some() && younger >= limit(a.4) {
            retire.push(a.3);
        }
    }
    let g = gravity.at(DVec3::ZERO).length();
    for (e, mut m, mut tf, vit, local, statuses) in &mut minions {
        let dt = time.delta_for(local);
        m.age += dt;
        if retire.contains(&e) {
            m.life = m.age;
        }
        let down = vit.is_some_and(|v| v.down());
        if down || m.age >= m.life {
            fall(e, &m, &tf, &parts, &mut commands, &mut queue);
            continue;
        }
        let k = m.size;
        let held = statuses.is_some_and(|s| s.rooted || s.frozen);
        let owner = m
            .cast
            .caster
            .and_then(|o| anchors.get(o).ok())
            .map(|g| (g.translation(), g.rotation()));
        // Look around now and then.
        m.look_in -= dt;
        if m.look_in <= 0.0 {
            m.look_in = 0.5;
            let near = |p: DVec3, r: f32| p.distance(m.pos) <= r as f64;
            let leash = |p: DVec3| owner.is_none_or(|(o, _)| o.distance(p) <= LEASH_M as f64);
            let seen = bodies
                .iter()
                .filter(|b| b.0 != e)
                .map(|(b, g, v, al)| Seen {
                    e: b,
                    at: g.translation(),
                    hurt: if v.down() {
                        -1.0
                    } else {
                        1.0 - v.health / v.max.max(1e-3)
                    },
                    friend: m.cast.friend(b, al.caster, al.faction, factions.as_deref()),
                });
            m.target = match m.kind {
                MinionKind::Wisp => seen
                    .filter(|s| s.friend && s.hurt > 0.001 && leash(s.at))
                    .max_by(|a, b| a.hurt.total_cmp(&b.hurt))
                    .map(|s| s.e),
                _ => {
                    let reach = if m.kind == MinionKind::Golem {
                        AGGRO_M
                    } else {
                        SHOOT_M
                    };
                    seen.filter(|s| !s.friend && s.hurt >= 0.0 && near(s.at, reach) && leash(s.at))
                        .min_by(|a, b| a.at.distance(m.pos).total_cmp(&b.at.distance(m.pos)))
                        .map(|s| s.e)
                }
            };
        }
        let target = m
            .target
            .and_then(|t| bodies.get(t).ok())
            .filter(|b| !b.2.down() || m.kind == MinionKind::Wisp)
            .map(|b| (b.0, b.1.translation()));
        if m.target.is_some() && target.is_none() {
            m.target = None;
        }
        m.cooldown -= dt;
        let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;

        let exclude = QueryFilter::excluding(e);
        match m.kind {
            MinionKind::Golem => {
                let mut act = m.act;
                let mut rising = None;
                match &mut act {
                    Act::Rise { t } => {
                        *t += dt;
                        rising = Some(*t);
                        if *t >= RISE_SECS {
                            act = Act::Ready;
                            commands.entity(e).insert(golem_collider(k));
                            queue.looks.push(LookCmd::Burst {
                                at: m.pos,
                                up: Vec3::Y,
                                look: "stone".into(),
                                role: Role::Impact,
                                scale: 0.9 * k,
                                color: m.cast.spell.weave.color,
                            });
                        }
                    }
                    Act::Punch { t, .. } => {
                        let before = *t;
                        *t += dt;
                        if before < PUNCH_HIT
                            && *t >= PUNCH_HIT
                            && let Some((te, tp)) = target
                            && flat_distance(tp, m.pos) <= (reach_of(k) + 0.7) as f64
                        {
                            let at = tp + DVec3::Y * 1.1;
                            hits.0.push(PendingHit {
                                cast: minion_cast(&m, e, &m.attack, 1.0),
                                target: Some(te),
                                point: at,
                                normal: -fwd,
                                dir: fwd,
                                multi: false,
                                impact: true,
                                reacts: true,
                                expire: false,
                            });
                            queue.looks.push(LookCmd::Burst {
                                at,
                                up: -fwd,
                                look: m.cast.spell.weave.key.clone(),
                                role: Role::Impact,
                                scale: 0.6 * k,
                                color: m.cast.spell.weave.color,
                            });
                        }
                        if *t >= PUNCH_SECS {
                            act = Act::Ready;
                            m.cooldown = 0.5;
                        }
                    }
                    Act::Slam { t } => {
                        let before = *t;
                        *t += dt;
                        if before < SLAM_HIT && *t >= SLAM_HIT {
                            let at = ground_below(
                                &queries,
                                m.pos + (fwd * 1.3 * k).as_dvec3(),
                                &exclude,
                            );
                            queue.looks.push(LookCmd::Burst {
                                at,
                                up: Vec3::Y,
                                look: "stone".into(),
                                role: Role::Impact,
                                scale: 1.7 * k,
                                color: m.cast.spell.weave.color,
                            });
                            if !m.cast.spell.weave.elements.is_empty() {
                                queue.looks.push(LookCmd::Burst {
                                    at,
                                    up: Vec3::Y,
                                    look: m.cast.spell.weave.key.clone(),
                                    role: Role::Impact,
                                    scale: 1.2 * k,
                                    color: m.cast.spell.weave.color,
                                });
                            }
                            if let Some(slam) = m.slam.clone() {
                                for (b, bg, bv, al) in &bodies {
                                    let p = bg.translation();
                                    if b == e
                                        || bv.down()
                                        || flat_distance(p, at) > (SLAM_M * k) as f64
                                        || m.cast.friend(
                                            b,
                                            al.caster,
                                            al.faction,
                                            factions.as_deref(),
                                        )
                                    {
                                        continue;
                                    }
                                    let dir = (p - at).as_vec3();
                                    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or(fwd);
                                    hits.0.push(PendingHit {
                                        cast: minion_cast(&m, e, &slam, 1.0),
                                        target: Some(b),
                                        point: p + DVec3::Y * 0.5,
                                        normal: Vec3::Y,
                                        dir,
                                        multi: false,
                                        impact: false,
                                        reacts: true,
                                        expire: false,
                                    });
                                }
                            }
                        }
                        if *t >= SLAM_SECS {
                            act = Act::Ready;
                            m.cooldown = 0.7;
                        }
                    }
                    Act::Ready | Act::Shoot { .. } => {}
                }
                // Where to go: to the foe, else beside its summoner.
                let mut speed = 0.0;
                if act == Act::Ready && rising.is_none() {
                    let goal = match (target, owner) {
                        (Some((_, tp)), _) => Some((tp, reach_of(k))),
                        (None, Some((op, orot))) => Some((
                            op + (orot * Vec3::new(1.6 * m.flank, 0.0, 1.6)).as_dvec3(),
                            1.0,
                        )),
                        _ => None,
                    };
                    if let Some((goal, stop)) = goal {
                        let to = (goal - m.pos).as_vec3();
                        let flat = Vec3::new(to.x, 0.0, to.z);
                        let dist = flat.length();
                        if dist > 0.05 {
                            let want = yaw_of(flat / dist);
                            let diff = wrap(want - m.yaw);
                            if !held {
                                m.yaw += diff.clamp(-TURN * dt, TURN * dt);
                            }
                            if dist > stop {
                                speed =
                                    WALK * k * diff.cos().max(0.0) * ((dist - stop) / 0.6).min(1.0);
                            } else if target.is_some()
                                && diff.abs() < 0.4
                                && m.cooldown <= 0.0
                                && !held
                            {
                                m.attacks += 1;
                                act = if m.attacks % 3 == 0 {
                                    Act::Slam { t: 0.0 }
                                } else {
                                    Act::Punch {
                                        t: 0.0,
                                        right: m.attacks % 2 == 1,
                                    }
                                };
                            }
                        }
                    }
                }
                if held {
                    speed = 0.0;
                }
                let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;
                let want = fwd * speed;
                let blend = (5.0 * dt).min(1.0);
                m.vel = m.vel.lerp(want, blend);
                let step = (m.vel * dt).as_dvec3();
                if step.length_squared() > 1e-10 {
                    let next = m.pos + step;
                    m.pos = ground_below(&queries, next + DVec3::Y * 0.5, &exclude);
                }
                m.act = act;
                // Feet, then the body over them.
                let (pos, yaw, vel) = (m.pos, m.yaw, m.vel);
                let mut feet = m.feet;
                let landed = if rising.is_some() {
                    let rot = Quat::from_rotation_y(yaw);
                    for (i, f) in feet.iter_mut().enumerate() {
                        let side = if i == 0 { -1.0 } else { 1.0 };
                        *f = Foot::planted(
                            pos + (rot * Vec3::new(side * HIP_X * k, 0.0, 0.0)).as_dvec3(),
                        );
                    }
                    Vec::new()
                } else {
                    step_feet(&mut feet, pos, yaw, vel, k, dt, held, |p| {
                        ground_below(&queries, p + DVec3::Y * 0.5, &exclude)
                    })
                };
                m.feet = feet;
                m.dip = (m.dip - dt * 0.4).max(0.0);
                for at in landed {
                    m.dip = 0.05;
                    queue.looks.push(LookCmd::Burst {
                        at,
                        up: Vec3::Y,
                        look: "stone".into(),
                        role: Role::Impact,
                        scale: 0.25 * k,
                        color: m.cast.spell.weave.color,
                    });
                }
                let rise = rising.map_or(0.0, |t| {
                    (2.9 * k) * (1.0 - crate::stone::climb(t, RISE_SECS))
                });
                tf.translation = m.pos - DVec3::Y * rise as f64;
                tf.rotation = Quat::from_rotation_y(m.yaw);
                let inv = tf.rotation.inverse();
                let lift = STEP_H * k;
                let ankles = m.feet.map(|f| {
                    if rising.is_some() {
                        Vec3::ZERO
                    } else {
                        inv * (f.now(lift) - m.pos).as_vec3() + Vec3::Y * ANKLE_Y * k
                    }
                });
                let ankles = if rising.is_some() {
                    [
                        Vec3::new(-HIP_X * k, ANKLE_Y * k, 0.0),
                        Vec3::new(HIP_X * k, ANKLE_Y * k, 0.0),
                    ]
                } else {
                    ankles
                };
                let stepping = m
                    .feet
                    .iter()
                    .enumerate()
                    .find_map(|(i, f)| f.t.map(|t| (i, t)));
                let st = stance(m.act, ankles.map(|a| a / k), stepping, m.dip);
                let pose = pose_golem(&st, ankles, k);
                for &p in &m.parts {
                    if let Ok((part, mut ptf)) = parts.get_mut(p) {
                        let (at, rot) = pose[part.index];
                        ptf.translation = at.as_dvec3();
                        ptf.rotation = rot;
                        ptf.scale = if EYE.contains(&part.index) {
                            Vec3::splat(st.eyes.max(0.001))
                        } else {
                            Vec3::ONE
                        };
                    }
                }
            }
            MinionKind::Elemental => {
                // Keep to its summoner's flank; with a foe in sight, drift out to fight it
                // from a bolt's comfortable range, never far from its summoner.
                let spot = match (target, owner) {
                    (Some((_, tp)), Some((op, _))) => {
                        let back = flat(op - tp).normalize_or(DVec3::Z);
                        let side = DVec3::new(-back.z, 0.0, back.x);
                        let stand = tp + back * ENGAGE_M as f64;
                        let out = flat(stand - op);
                        let stand = if out.length() > GUARD_M as f64 {
                            op + out.normalize() * GUARD_M as f64
                        } else {
                            stand
                        };
                        Some(stand + side * (1.5 * m.flank) as f64)
                    }
                    (None, Some((op, orot))) => {
                        Some(op + (orot * Vec3::new(2.4 * m.flank, 0.0, 0.6)).as_dvec3())
                    }
                    _ => None,
                };
                let goal = spot
                    .map(|p| {
                        ground_below(&queries, p + DVec3::Y, &exclude) + DVec3::Y * HOVER_M as f64
                    })
                    .unwrap_or(m.pos);
                let pull = 1.0 - (-2.0 * dt).exp();
                let mv = ((goal - m.pos) * pull as f64)
                    .as_vec3()
                    .clamp_length_max(4.0 * dt);
                if !held {
                    m.pos += mv.as_dvec3();
                }
                let face = target
                    .map(|(_, tp)| (tp - m.pos).as_vec3())
                    .unwrap_or(owner.map_or(Vec3::NEG_Z, |(_, r)| r * Vec3::NEG_Z));
                let face = Vec3::new(face.x, 0.0, face.z).normalize_or(Vec3::NEG_Z);
                let diff = wrap(yaw_of(face) - m.yaw);
                m.yaw += diff.clamp(-TURN * dt, TURN * dt);
                let mut act = m.act;
                let mut gather = 0.0;
                match &mut act {
                    Act::Shoot { t } => {
                        *t += dt;
                        gather = (*t / GATHER_SECS).min(1.0);
                        if *t >= GATHER_SECS {
                            if let Some((te, tp)) = target {
                                let aim = tp + DVec3::Y * 1.0;
                                let info = minion_cast(&m, e, &m.attack, 0.6);
                                release(
                                    &info,
                                    m.pos,
                                    m.pos,
                                    face,
                                    aim,
                                    Some(te),
                                    g,
                                    &queries,
                                    &mut queue,
                                    &mut hits,
                                );
                            }
                            queue.looks.push(LookCmd::EndCharge { caster: e });
                            act = Act::Ready;
                            m.cooldown = SHOOT_EVERY - GATHER_SECS;
                        }
                    }
                    _ => {
                        if let Some((_, tp)) = target
                            && m.cooldown <= 0.0
                            && !held
                            && tp.distance(m.pos) <= SHOOT_M as f64
                        {
                            m.attacks += 1;
                            act = Act::Shoot { t: 0.0 };
                            queue.looks.push(LookCmd::Charge {
                                caster: e,
                                look: m.cast.spell.weave.key.clone(),
                                secs: GATHER_SECS,
                                scale: 0.6 * k,
                            });
                        }
                    }
                }
                m.act = act;
                m.spin += dt * (1.4 + 4.0 * gather);
                let bob = 0.12 * (m.age * 4.4).sin();
                tf.translation = m.pos + DVec3::Y * bob as f64;
                tf.rotation = Quat::from_rotation_y(m.yaw);
                pose_shell(&m, gather, &mut parts);
            }
            MinionKind::Wisp => {
                // Circle above the hurt ally's shoulder (or the summoner) and heal.
                let host = target.map(|t| t.1).or(owner.map(|o| o.0));
                let a = m.age * 1.4 + m.flank;
                let goal = host
                    .map(|h| {
                        h + DVec3::new(
                            (a.cos() * 0.7) as f64,
                            (2.0 + 0.15 * (a * 2.3).sin()) as f64,
                            (a.sin() * 0.7) as f64,
                        )
                    })
                    .unwrap_or(m.pos);
                let pull = 1.0 - (-3.0 * dt).exp();
                let mv = ((goal - m.pos) * pull as f64)
                    .as_vec3()
                    .clamp_length_max(6.0 * dt);
                m.pos += mv.as_dvec3();
                if let Some((te, tp)) = target
                    && m.cooldown <= 0.0
                    && m.pos.distance(tp + DVec3::Y * 1.2) < 2.8
                {
                    m.cooldown = HEAL_EVERY;
                    m.attacks += 1;
                    let at = tp + DVec3::Y * 1.1;
                    hits.0.push(PendingHit {
                        cast: minion_cast(&m, e, &m.attack, 1.0),
                        target: Some(te),
                        point: at,
                        normal: Vec3::Y,
                        dir: Vec3::NEG_Y,
                        multi: false,
                        impact: false,
                        reacts: false,
                        expire: false,
                    });
                    queue.looks.push(LookCmd::Beam {
                        from: m.pos,
                        to: at,
                        look: m.cast.spell.weave.key.clone(),
                        color: m.cast.spell.weave.color,
                        width: 0.05,
                        secs: 0.35,
                    });
                    queue.looks.push(LookCmd::Burst {
                        at,
                        up: Vec3::Y,
                        look: m.cast.spell.weave.key.clone(),
                        role: Role::Impact,
                        scale: 0.6,
                        color: m.cast.spell.weave.color,
                    });
                }
                m.spin += dt * 3.0;
                tf.translation = m.pos;
                pose_shell(&m, 0.0, &mut parts);
            }
        }
    }
}

/// The cast a minion's own blow, bolt or heal carries: its summoner's side, itself as caster.
fn minion_cast(m: &Minion, e: Entity, spell: &Arc<Spell>, scale: f32) -> CastInfo {
    CastInfo {
        spell: spell.clone(),
        caster: Some(e),
        scale,
        seed: m.cast.seed.wrapping_add(m.attacks * 7919),
        ..m.cast.clone()
    }
}

fn flat(d: DVec3) -> DVec3 {
    DVec3::new(d.x, 0.0, d.z)
}

fn flat_distance(a: DVec3, b: DVec3) -> f64 {
    let d = a - b;
    DVec3::new(d.x, 0.0, d.z).length()
}

/// How near a golem walks to what it fights, m (its arm's reach and the foe's girth).
pub fn reach_of(k: f32) -> f32 {
    1.45 * k + 0.35
}

/// An elemental's shards ring its core on tilted orbits, faster while it gathers a bolt; a
/// wisp's motes circle it.
fn pose_shell(
    m: &Minion,
    gather: f32,
    parts: &mut Query<(&MinionPart, &mut Transform), Without<Minion>>,
) {
    let k = m.size;
    for &p in &m.parts {
        let Ok((part, mut tf)) = parts.get_mut(p) else {
            continue;
        };
        if part.index == 0 {
            tf.scale = Vec3::splat(1.0 + 0.35 * gather);
            continue;
        }
        let i = (part.index - 1) as f32;
        let (n, r) = if m.kind == MinionKind::Wisp {
            (3.0, 0.36)
        } else {
            (SHARDS as f32, 0.5 * SHELL_M * k * (1.0 - 0.25 * gather))
        };
        let a = m.spin
            * if (i as usize).is_multiple_of(2) {
                1.0
            } else {
                -0.8
            }
            + TAU * i / n;
        let tilt = 0.5 * (i * 1.7).sin();
        let at = Vec3::new(
            a.cos() * r,
            (a * 1.3 + i).sin() * 0.3 * k + tilt * a.sin() * r,
            a.sin() * r,
        );
        tf.translation = at.as_dvec3();
        tf.rotation = Quat::from_rotation_y(-a) * Quat::from_rotation_z(0.6 + tilt);
    }
}

/// A minion's end: a golem's and an elemental's rocks come apart and tumble, its glow goes
/// out; a golem woven with an element bursts in it (the lava golem's explosion).
fn fall(
    e: Entity,
    m: &Minion,
    tf: &Transform,
    parts: &Query<(&MinionPart, &mut Transform), Without<Minion>>,
    commands: &mut Commands,
    queue: &mut MagicQueue,
) {
    let spell = &m.cast.spell;
    let center = tf.translation
        + DVec3::Y
            * if m.kind == MinionKind::Golem {
                1.5 * m.size
            } else {
                0.0
            } as f64;
    for &p in &m.parts {
        let Ok((part, ptf)) = parts.get(p) else {
            continue;
        };
        if part.size == Vec3::ZERO {
            continue;
        }
        let at = tf.translation + (tf.rotation * ptf.translation.as_vec3()).as_dvec3();
        let rot = tf.rotation * ptf.rotation;
        let out = (at - center).as_vec3();
        let out = Vec3::new(out.x, 0.0, out.z).normalize_or(Vec3::X);
        let r = |s: u64| roll(m.cast.seed as u64 ^ p.to_bits(), s) - 0.5;
        let volume = part.size.x * part.size.y * part.size.z;
        commands.entity(p).remove::<ChildOf>().insert((
            Transform::from_translation(at).with_rotation(rot),
            Collider {
                shape: ColliderShape::Cuboid {
                    half_extents: part.size * 0.4,
                },
                density: 2600.0,
                ..Default::default()
            },
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some((volume * 1200.0).clamp(15.0, 600.0)),
                linear_damping: 0.05,
                angular_damping: 0.2,
                ..Default::default()
            },
            Velocity {
                linear: out * (1.2 + r(1)) + Vec3::Y * (1.0 + r(2)),
                angular: Vec3::new(r(3), r(4), r(5)) * 4.0,
            },
            Rubble {
                age: 0.0,
                life: RUBBLE_SECS + r(6),
            },
        ));
    }
    queue.looks.push(LookCmd::Burst {
        at: center,
        up: Vec3::Y,
        look: if m.kind == MinionKind::Golem {
            "stone".into()
        } else {
            spell.weave.key.clone()
        },
        role: Role::Impact,
        scale: 1.4 * m.size,
        color: spell.weave.color,
    });
    if m.kind == MinionKind::Golem && !spell.weave.elements.is_empty() {
        // It bursts in its element as it falls.
        queue.spawns.push(Spawn::Field(
            field(
                &CastInfo {
                    caster: Some(e),
                    ..m.cast.clone()
                },
                FieldKind::Burst,
                3.0 * m.size,
                0.0,
            ),
            center,
        ));
    }
    commands.entity(e).despawn();
}

/// Rubble lies a while, then sinks into the floor and goes.
pub fn settle_rubble(
    time: Res<Time>,
    mut q: Query<(Entity, &mut Rubble, &mut Transform)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut r, mut tf) in &mut q {
        let before = r.age;
        r.age += dt;
        if before < r.life && r.age >= r.life {
            commands
                .entity(e)
                .remove::<(Collider, RigidBody, Velocity)>();
        }
        if r.age > r.life {
            tf.translation.y -= (0.9 * dt) as f64;
        }
        if r.age > r.life + SINK_SECS {
            commands.entity(e).despawn();
        }
    }
}

/// Minions keep world positions of their own; move them when the world re-anchors.
pub fn follow_world_shift(mut shifts: MessageReader<WorldShift>, mut q: Query<&mut Minion>) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut m in &mut q {
        m.pos = s.point(m.pos);
        m.vel = s.direction(m.vel);
        for f in m.feet.iter_mut() {
            f.at = s.point(f.at);
            f.from = s.point(f.from);
            f.to = s.point(f.to);
        }
    }
}

/// Where a summon stands: where the caster points, within the spell's range of them.
pub fn place(body: DVec3, aim_point: DVec3, range: f32) -> DVec3 {
    let d = aim_point - body;
    let flat = DVec3::new(d.x, 0.0, d.z);
    let far = flat.length();
    if far > range as f64 {
        body + flat * (range as f64 / far) + DVec3::Y * d.y
    } else {
        aim_point
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caster::MagicCaster;

    fn spell(fx: &[&str]) -> Arc<Spell> {
        MagicCaster::new(40.0, Mastery::Adept)
            .compile(&SpellDesign::new(fx, CastType::Summon), 0.0)
            .unwrap()
    }

    #[test]
    fn the_weave_picks_the_creature() {
        assert_eq!(kind_of(&spell(&["stone"])), Some(MinionKind::Golem));
        assert_eq!(kind_of(&spell(&["fire", "stone"])), Some(MinionKind::Golem));
        assert_eq!(kind_of(&spell(&["frost"])), Some(MinionKind::Elemental));
        assert_eq!(kind_of(&spell(&["heal"])), Some(MinionKind::Wisp));
        assert_eq!(kind_of(&spell(&["push"])), None);
        assert_eq!(limit(Mastery::Adept), 1);
        assert_eq!(limit(Mastery::Master), 2);
    }

    #[test]
    fn a_summon_lasts_a_minute_within_reach() {
        let s = spell(&["stone"]);
        assert_eq!(s.cast, CastType::Summon);
        assert!((s.carrier.duration - 60.0).abs() < 1e-3);
        let at = place(DVec3::ZERO, DVec3::new(30.0, 0.0, 40.0), s.carrier.range);
        assert!((DVec3::new(at.x, 0.0, at.z).length() - s.carrier.range as f64).abs() < 1e-6);
    }

    #[test]
    fn two_bone_ik_keeps_its_bones_and_reaches() {
        let a = Vec3::new(0.0, 1.2, 0.0);
        let t = Vec3::new(0.1, 0.2, -0.3);
        let (j, end) = two_bone(a, t, 0.62, 0.6, Vec3::NEG_Z);
        assert!((j.distance(a) - 0.62).abs() < 1e-4);
        assert!((end.distance(j) - 0.6).abs() < 1e-4);
        assert!(end.distance(t) < 1e-4);
        // The knee bends toward the pole.
        assert!(j.z < (a.z + t.z) * 0.5);
        // Out of reach: straight toward it.
        let (_, end) = two_bone(a, Vec3::new(0.0, -5.0, 0.0), 0.62, 0.6, Vec3::NEG_Z);
        assert!((end.distance(a) - 1.22).abs() < 0.01);
    }

    #[test]
    fn feet_stay_planted_and_step_one_at_a_time() {
        let mut feet = [
            Foot::planted(DVec3::new(-0.36, 0.0, 0.0)),
            Foot::planted(DVec3::new(0.36, 0.0, 0.0)),
        ];
        let dt = 1.0 / 60.0;
        let vel = Vec3::new(0.0, 0.0, -1.3);
        let mut pos = DVec3::ZERO;
        let mut landed = 0;
        let mut last = None;
        let mut alternated = true;
        for _ in 0..240 {
            pos += (vel * dt).as_dvec3();
            let before = feet;
            let l = step_feet(&mut feet, pos, 0.0, vel, 1.0, dt, false, |p| {
                DVec3::new(p.x, 0.0, p.z)
            });
            assert!(
                feet.iter().filter(|f| f.t.is_some()).count() <= 1,
                "both feet up"
            );
            for i in 0..2 {
                if before[i].t.is_none() && feet[i].t.is_none() {
                    assert_eq!(before[i].at, feet[i].at, "a planted foot slid");
                }
                if before[i].t.is_none() && feet[i].t.is_some() {
                    if last == Some(i) {
                        alternated = false;
                    }
                    last = Some(i);
                }
            }
            landed += l.len();
        }
        // Four seconds at 1.3 m/s: a stride each ~0.8 m, alternating, keeping up.
        assert!(landed >= 5, "{landed}");
        assert!(alternated);
        let mid = (feet[0].now(0.0) + feet[1].now(0.0)) * 0.5;
        assert!(mid.distance(pos) < 1.0, "{mid} vs {pos}");
    }

    #[test]
    fn a_punch_reaches_forward_and_a_slam_reaches_the_floor() {
        let rest = [
            Vec3::new(-HIP_X, ANKLE_Y, 0.0),
            Vec3::new(HIP_X, ANKLE_Y, 0.0),
        ];
        let fist = |act| {
            let st = stance(act, rest, None, 0.0);
            pose_golem(&st, rest, 1.0)[ARM[1] + 2].0
        };
        let idle = fist(Act::Ready);
        let thrown = fist(Act::Punch {
            t: PUNCH_HIT,
            right: true,
        });
        assert!(thrown.z < idle.z - 0.8, "{thrown} vs {idle}");
        assert!(thrown.y > 1.2, "{thrown}");
        let slam = fist(Act::Slam { t: SLAM_HIT });
        assert!(slam.y < 0.7, "{slam}");
        let raised = fist(Act::Slam { t: 0.6 });
        assert!(raised.y > 2.6, "{raised}");
        // A golem stands a little over 2.6 m (head top).
        let st = stance(Act::Ready, rest, None, 0.0);
        let head = pose_golem(&st, rest, 1.0)[HEAD].0;
        assert!(head.y > 2.2 && head.y < 2.6, "{head}");
    }
}
