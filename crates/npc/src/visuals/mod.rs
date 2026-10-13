//! Stand-in bodies: simple figures built from primitives and posed every step from what the
//! AI is doing, so behavior reads clearly before real character models exist.
//!
//! Fighters are armored combatants built from procedural meshes (see [`crate::art::fighter`])
//! on a skeleton whose knees and elbows bend. Their feet stay where they are set on the ground
//! while the body walks, runs or turns over them, the stride lengthening into a run with the
//! heels kicking up and the arms pumping; on guard they shuffle without breaking their stance.
//! Turning on the spot, the hips come round after the body and the feet pivot on their balls
//! and step round under them.
//! Standing, they breathe and shift their weight from leg to leg (on guard, bob and rock on
//! bent knees), and they kneel behind low cover. A swing coils back onto the rear foot during
//! the attack's telegraph (the blade glows orange for a heavy guard breaker), then the hips
//! lead the shoulders round into the cut, the weight goes onto the front foot and the blade
//! follows through; a feint called off mid-telegraph lurches into the release, checks and
//! rocks back onto its guard instead of snapping there. A dash pushes off the rear foot: the
//! body is drawn low behind the owner and eases after it as the drive leg extends, the lead
//! foot reaching ahead. They raise the guard or shield to the line the blow comes on (a roof
//! over the helmet against a heavy cut from above, the elbows high and out so the face shows,
//! on the side a cut comes from, low across a thrust or a dash), bracing and rocking back as
//! it lands (on a parry the blade beats the blow aside and
//! glints white, and the shield lights up), hop aside to dodge and step back under themselves,
//! and are thrown back and stumble when staggered. A blow that lands throws them by where it
//! struck (the head is wrenched round away from it, the shoulders following, as they stumble
//! back half a step; the trunk doubles over and turns away; the legs buckle on the struck
//! side), and they light up for an instant (tinted, never blown out to white: the
//! plates light up in a pale tint of the team's color). Downed, the knees give, the body
//! topples back along the blow and lands hips, back, then head, the legs kick and settle (one
//! straight, one folded out) and the arms flop out on the ground, the weapon dropped beside
//! them. Shooters carry their carbine at port arms on the move and shoulder it to aim, the
//! muzzle drifting a little as they breathe; a shot flashes and kicks the muzzle up, and it
//! settles; every few shots they change the magazine as it cools down (stripped, stowed in the
//! hip pouch, a fresh one seated, the charging handle racked). Archers draw to the corner of the
//! mouth and hold, loose (the string hand flies back along the jaw, the bow rolls forward in
//! the hand), then draw the next arrow from the quiver over the shoulder and nock it. The
//! crest, the tabards and the belt pouches swing after the body (a stop throws them on, a blow
//! snaps them round) and settle. Faces show under open helmets worn over mail coifs. The helmet's horsehair
//! crest shows skill (grey rookie, bronze, silver, gold master) and a light in its brow band
//! glows with alertness (yellow suspicious, orange searching, red fighting). Their painted
//! armor shows skill too: a rookie's is nearly new, a veteran's or master's chipped to the
//! steel, scratched, dented and dirty. The team's color covers the cuirass, helmet, pauldrons,
//! knee-length tabards and arm bands, so squads tell apart across the arena; the gambesons and
//! hose under it are undyed or earthy, and differ between fighters.
//!
//! Fighters look at what they are after: the eyes flick to a new foe first and the head
//! follows a beat behind (the shoulders taking what the neck can't); they glance at a flanker
//! or toward a noise for a moment and sweep the ground ahead when searching. They blink (more
//! often when alarmed), set the jaw or grimace on a heavy blow and when hit, and breathe hard
//! through an open mouth after a sprint or when badly hurt. Badly hurt, they limp on the leg
//! last struck, or hunch over a wound to the body, with shorter steps, and at ease press a
//! free hand to it. At ease the weapon is carried (the longsword on the shoulder, the arming
//! sword point down, the carbine hanging by its grip, the bow low); a foe brings it up to guard
//! (an archer drawing an arrow from the quiver and nocking it on the way), and it goes down
//! again after the fight. Revived, they get up from where they lay: roll onto the side, come
//! up onto a knee taking up the weapon, push up and settle onto guard.
//!
//! Animals are sculpted procedural red deer, grey wolves, rabbits and starlings (see
//! `art::animals`; cheap primitives without the renderer), posed by `animal.rs` from their
//! state and what their body did the step before (kept on `StandInState`): planted feet in
//! each species' footfalls, a body that rides on them, flexes its spine in the gallop and
//! throws its weight on starts, stops and turns, ears and tails that answer noises and alarm,
//! grazing, alert freezes, rest, stalking, hopping, a starling's wing beat, bounds, glides,
//! banks, takeoffs and landings, and falling over when killed.
//!
//! With `NpcSettings::show_intent` on, a ring on the ground marks where each NPC decided to
//! go and a line leads to it, colored by why: blue cover, orange flank, purple high ground,
//! grey falling back, yellow investigating or searching.

mod animal;
mod humanoid;

use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};
use engine_world::{AttackEvent, AttackKind, DamageEvent};

use crate::agent::{NpcBrain, NpcClock, NpcCombat, NpcMotor, NpcSettings, NpcVitals, NpcWeapon};
use crate::art::ArtAssets;
use crate::combat::Downed;
use crate::faction::Faction;
use crate::locomotion::MoverState;
use crate::perception::NpcMemory;
use crate::skills::NpcSkills;
use crate::wildlife::{Animal, AnimalMind, BirdFlight, Species};

/// Give an NPC or animal a stand-in body. Colors are sRGB hex; `None` picks from the faction
/// (fighters) or species (animals).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StandIn {
    pub color: Option<u32>,
    /// Hide the alert orb and skill band.
    pub plain: bool,
}

/// One piece of a stand-in body (a child of the NPC or animal).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct StandInPart {
    pub part: Part,
    scale: Vec3,
    /// The part's own material; poses override it briefly (hit flashes, glowing telegraphs).
    base: MaterialRef,
}

/// Body parts of stand-ins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Torso,
    Head,
    Band,
    LegL,
    LegR,
    ArmL,
    ArmR,
    Weapon,
    Shield,
    Orb,
    Muzzle,
    // Four-legged animals and birds.
    Body,
    Neck,
    Snout,
    Leg(u8),
    Tail,
    Ear(u8),
    Antler(u8),
    Wing(u8),
    /// Extra fighter pieces (limb segments, hands, armor plates); `humanoid.rs` names them.
    Fighter(u8),
    /// Extra animal pieces (leg segments, hooves, feathers); `animal.rs` names them.
    Beast(u8),
}

/// Ground marker showing where an NPC is going (see the module docs).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct IntentMarker {
    pub owner: Entity,
    line: bool,
}

/// Per-body bookkeeping for effects, and a fighter's or animal's motion from one pose to the
/// next.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct StandInState {
    was_winding: bool,
    flash_until: f64,
    markers: Option<[Entity; 2]>,
    motion: humanoid::Motion,
    beast: animal::Motion,
}

fn mat(hex: u32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(0.75)
}

fn glow(hex: u32, strength: f32) -> StandardMaterial {
    let m = StandardMaterial::hex(hex);
    let c = [m.base_color[0], m.base_color[1], m.base_color[2]];
    m.with_emissive(c, strength).with_roughness(0.4)
}

const SKIN: u32 = 0xd2a37c;
const STEEL: u32 = 0xb9bec7;
const GUNMETAL: u32 = 0x2c2d30;
const WOOD: u32 = 0x7a5230;

fn faction_color(f: Option<&Faction>) -> u32 {
    match f.map(|f| f.0) {
        Some(0) => 0x3d8a4e,
        Some(1) => 0xb02f35,
        Some(2) => 0x2f7cf2,
        _ => 0x8a8a8a,
    }
}

fn skill_color(level: f32) -> u32 {
    if level < 0.35 {
        0x7d7d7d
    } else if level < 0.6 {
        0xb0773a
    } else if level < 0.8 {
        0xd0d4da
    } else {
        0xf0c030
    }
}

fn species_color(s: Species) -> u32 {
    match s {
        Species::Deer => 0x8d5b34,
        Species::Rabbit => 0x9a8166,
        Species::Wolf => 0x77787a,
        Species::Bird => 0x3f4558,
        Species::Boar => 0x4a4037,
    }
}

type BuildItem<'a> = (
    Entity,
    &'a StandIn,
    Option<&'a Animal>,
    Option<&'a NpcWeapon>,
    Option<&'a Faction>,
    Option<&'a NpcSkills>,
);

/// Spawn the parts of new stand-ins.
pub(crate) fn build_stand_ins(
    mut commands: Commands,
    q: Query<BuildItem, Added<StandIn>>,
    mut art: ArtAssets,
) {
    for (e, stand_in, animal, weapon, faction, skills) in &q {
        let parts = match animal.map(|a| a.species) {
            None => humanoid::parts(stand_in, weapon, faction, skills, &mut art),
            Some(species) => animal::parts(stand_in, animal, species, &mut art),
        };
        for (part, renderer, scale) in parts {
            commands.spawn((
                StandInPart {
                    part,
                    scale,
                    base: renderer.material.clone(),
                },
                Transform::default().with_scale(scale),
                renderer,
                ChildOf(e),
            ));
        }
        commands.entity(e).insert(StandInState::default());
    }
}

/// A part's pose relative to its owner.
#[derive(Debug, Clone, Copy)]
struct Placed {
    pos: Vec3,
    rot: Quat,
    visible: bool,
}

fn at(pos: Vec3, rot: Quat) -> Placed {
    Placed {
        pos,
        rot,
        visible: true,
    }
}

const HIDDEN: Placed = Placed {
    pos: Vec3::ZERO,
    rot: Quat::IDENTITY,
    visible: false,
};

type PoseItem<'a> = (
    Entity,
    &'a StandIn,
    &'a mut StandInState,
    &'a Children,
    &'a Transform,
    &'a LocomotionState,
    Option<&'a MoverState>,
    Option<&'a MoveIntent>,
    Option<&'a NpcClock>,
    Option<&'a NpcCombat>,
    Option<&'a NpcBrain>,
    Option<&'a NpcVitals>,
    Option<&'a Downed>,
    Option<&'a NpcWeapon>,
    (
        Option<&'a Animal>,
        Option<&'a AnimalMind>,
        Option<&'a BirdFlight>,
        Option<&'a NpcMotor>,
        Option<&'a NpcMemory>,
    ),
);

/// What a fighter's pose reads of another (the attacker whose blow it meets).
type FoeItem<'a> = (
    &'a Transform,
    Option<&'a NpcCombat>,
    Option<&'a LocomotionState>,
    Option<&'a NpcWeapon>,
);

/// The attacks telegraphed and the blows landed this step (kept between steps to reuse the
/// space).
#[derive(Default)]
pub(crate) struct Sightings {
    attacks: Vec<(Entity, Option<Entity>, AttackKind)>,
    blows: Vec<DamageEvent>,
}

/// Pose every stand-in from its owner's state (a fighter's also from the attacks telegraphed
/// at it, the blows that land on it, and the attacker whose blow its guard meets).
pub(crate) fn pose_stand_ins(
    mut owners: Query<PoseItem, Without<StandInPart>>,
    foes: Query<FoeItem, Without<StandInPart>>,
    mut parts: Query<(&StandInPart, &mut Transform, &mut MeshRenderer)>,
    mut attacks: MessageReader<AttackEvent>,
    mut blows: MessageReader<DamageEvent>,
    mut seen: Local<Sightings>,
) {
    let seen = &mut *seen;
    seen.attacks.clear();
    seen.attacks
        .extend(attacks.read().map(|a| (a.attacker, a.target, a.kind)));
    seen.blows.clear();
    seen.blows.extend(blows.read().cloned());
    let foe = |e: Entity| {
        foes.get(e)
            .ok()
            .map(|(t, c, l, w)| humanoid::Foe::of(t, c, l, w))
    };
    for (
        me,
        stand_in,
        mut st,
        children,
        owner,
        loco,
        mover,
        intent,
        clock,
        combat,
        brain,
        vitals,
        downed,
        weapon,
        animal,
    ) in &mut owners
    {
        let now = clock.map(|c| c.now).unwrap_or(0.0);
        let phase = mover.map(|m| m.phase).unwrap_or(0.0);
        let (animal, mind, bird, motor, memory) = animal;
        let mut materials: Vec<(Part, StandardMaterial)> = Vec::new();
        let placed: Box<dyn Fn(Part) -> Placed> = match animal.map(|a| a.species) {
            None => {
                let view = humanoid::Seen {
                    me,
                    at: owner.translation,
                    attacks: &seen.attacks,
                    blows: &seen.blows,
                    dash_secs: motor.map_or(0.22, |m| m.dash_secs),
                    foe: &foe,
                    memory,
                };
                let pose = humanoid::humanoid(
                    now,
                    phase,
                    owner.rotation,
                    loco,
                    intent,
                    combat,
                    brain,
                    vitals,
                    downed,
                    weapon.copied().unwrap_or_default(),
                    &view,
                    &mut st,
                    &mut materials,
                    stand_in,
                );
                Box::new(pose)
            }
            Some(species) => {
                let input = animal::Input {
                    species,
                    male: animal.is_some_and(|a| a.male),
                    young: animal.is_some_and(|a| a.young),
                    state: mind.map(|m| m.state).unwrap_or_default(),
                    since: mind.map(|m| m.since).unwrap_or(0.0),
                    now,
                    phase,
                    at: owner.translation,
                    facing: owner.rotation,
                    vel: loco.velocity,
                    crouch: loco.mode == LocomotionMode::Crouch,
                    biting: mind.is_some_and(|m| now < m.bite_ready - 0.55),
                    vary: ((me.to_bits() as u32) as f32 * 0.618_034).fract(),
                    senses: mind
                        .map(|m| animal::Senses {
                            prey: m
                                .prey
                                .and_then(|e| foes.get(e).ok())
                                .map(|f| f.0.translation),
                            ..animal::Senses::of(m, now)
                        })
                        .unwrap_or_default(),
                    bird: bird.copied(),
                };
                let pose = animal::animal(&input, &mut st.beast);
                Box::new(move |p| pose.place(p))
            }
        };
        for child in children.iter() {
            let Ok((part, mut t, mut r)) = parts.get_mut(child) else {
                continue;
            };
            let p = placed(part.part);
            let new = Transform::from_translation(p.pos.as_dvec3())
                .with_rotation(p.rot)
                .with_scale(part.scale);
            if *t != new {
                *t = new;
            }
            if r.visible != p.visible {
                r.visible = p.visible;
            }
            // Overridden this step, or back to the part's own material.
            let m = materials
                .iter()
                .find(|(q, _)| *q == part.part)
                .map(|(_, m)| MaterialRef::Inline(*m));
            let m = m.as_ref().unwrap_or(&part.base);
            if r.material != *m {
                r.material = m.clone();
            }
        }
    }
}

/// Rings and lines on the ground showing where NPCs are going and why.
#[allow(clippy::type_complexity)]
pub(crate) fn intent_markers(
    mut commands: Commands,
    settings: Res<NpcSettings>,
    mut owners: Query<
        (Entity, &Transform, &NpcBrain, &NpcVitals, &mut StandInState),
        Without<IntentMarker>,
    >,
    mut markers: Query<(Entity, &IntentMarker, &mut Transform, &mut MeshRenderer)>,
) {
    if !settings.show_intent {
        for (e, ..) in &markers {
            commands.entity(e).despawn();
        }
        for (.., mut st) in &mut owners {
            st.markers = None;
        }
        return;
    }
    for (e, _, _, _, mut st) in &mut owners {
        if st.markers.is_none() {
            let ring = commands
                .spawn((
                    IntentMarker {
                        owner: e,
                        line: false,
                    },
                    Transform::default(),
                    MeshRenderer::new(
                        Shape::Torus {
                            radius: 0.55,
                            tube: 0.05,
                            segments: 24,
                        },
                        glow(0xffffff, 4.0),
                    )
                    .without_shadows(),
                ))
                .id();
            let line = commands
                .spawn((
                    IntentMarker {
                        owner: e,
                        line: true,
                    },
                    Transform::default(),
                    MeshRenderer::new(
                        Shape::Cuboid {
                            size: [0.05, 0.02, 1.0],
                        },
                        glow(0xffffff, 4.0),
                    )
                    .without_shadows(),
                ))
                .id();
            st.markers = Some([ring, line]);
        }
    }
    for (e, marker, mut t, mut r) in &mut markers {
        let Ok((_, owner_t, brain, vitals, _)) = owners.get(marker.owner) else {
            commands.entity(e).despawn();
            continue;
        };
        let Some(dest) = brain.destination.filter(|_| !vitals.downed) else {
            r.visible = false;
            continue;
        };
        let color = match brain.destination_reason {
            "cover" => 0x3b8cff,
            "flank" => 0xff8a1a,
            "high ground" => 0xb05cff,
            "fall back" => 0x9a9a9a,
            "investigate" | "search" => 0xf2d24c,
            "patrol" => 0x7fd37f,
            _ => 0xffffff,
        };
        let m = engine_render::MaterialRef::Inline(glow(color, 4.0));
        if r.material != m {
            r.material = m;
        }
        let from = owner_t.translation;
        let new = if marker.line {
            let d = dest - from;
            let len = DVec2::new(d.x, d.z).length();
            if len < 1.0 {
                r.visible = false;
                continue;
            }
            let yaw = (-d.x).atan2(-d.z) as f32;
            Transform::from_translation((from + dest) * 0.5 + DVec3::Y * 0.06)
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::new(1.0, 1.0, len as f32))
        } else {
            Transform::from_translation(dest + DVec3::Y * 0.06)
        };
        r.visible = true;
        *t = new;
    }
}
