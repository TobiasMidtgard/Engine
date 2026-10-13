//! Stand-in bodies: simple figures built from primitives and posed every step from what the
//! AI is doing, so behavior reads clearly before real character models exist.
//!
//! Fighters are armored combatants built from procedural meshes (see [`crate::art::fighter`])
//! on a skeleton whose knees and elbows bend. They walk and run with planted feet and swinging
//! arms, breathe and shift their weight when idle, kneel behind low cover, raise their weapon
//! during an attack's telegraph (it glows orange for a heavy guard breaker), raise the guard
//! or shield to block (on a parry the blade glints white and the shield lights up), lean into
//! dodges, reel when staggered, light up for an instant when hit (tinted, never blown out to
//! white: the plates light up in a pale tint of the team's color), and fall and drop their
//! weapon when downed. Shooters shoulder their carbine (with a muzzle flash and kick); archers
//! draw to the jaw. Faces show under open helmets worn over mail coifs. The helmet's horsehair
//! crest shows skill (grey rookie, bronze, silver, gold master) and a light in its brow band
//! glows with alertness (yellow suspicious, orange searching, red fighting). Their painted
//! armor shows skill too: a rookie's is nearly new, a veteran's or master's chipped to the
//! steel, scratched, dented and dirty. The team's color covers the cuirass, helmet, pauldrons,
//! knee-length tabards and arm bands, so squads tell apart across the arena; the gambesons and
//! hose under it are undyed or earthy, and differ between fighters.
//!
//! Animals are sculpted procedural red deer, grey wolves, rabbits and starlings (see
//! `art::animals`; cheap primitives without the renderer). They plant their feet as they walk,
//! trot and gallop, graze head-down, raise their heads when alert, lie down to rest, hop
//! (rabbits), creep low when stalking, peck, flap and bank in flight, and fall over when
//! killed.
//!
//! With `NpcSettings::show_intent` on, a ring on the ground marks where each NPC decided to
//! go and a line leads to it, colored by why: blue cover, orange flank, purple high ground,
//! grey falling back, yellow investigating or searching.

mod animal;
mod humanoid;

use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionState, MoveIntent};

use crate::agent::{NpcBrain, NpcClock, NpcCombat, NpcSettings, NpcVitals, NpcWeapon};
use crate::art::ArtAssets;
use crate::combat::Downed;
use crate::faction::Faction;
use crate::locomotion::MoverState;
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

/// Per-body bookkeeping for effects.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct StandInState {
    was_winding: bool,
    flash_until: f64,
    markers: Option<[Entity; 2]>,
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
    &'a StandIn,
    &'a mut StandInState,
    &'a Children,
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
    ),
);

/// Pose every stand-in from its owner's state.
pub(crate) fn pose_stand_ins(
    mut owners: Query<PoseItem>,
    mut parts: Query<(&StandInPart, &mut Transform, &mut MeshRenderer)>,
) {
    for (
        stand_in,
        mut st,
        children,
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
        let speed = Vec3::new(loco.velocity.x, 0.0, loco.velocity.z).length();
        let (animal, mind, bird) = animal;
        let mut materials: Vec<(Part, StandardMaterial)> = Vec::new();
        let placed: Box<dyn Fn(Part) -> Placed> = match animal.map(|a| a.species) {
            None => {
                let pose = humanoid::humanoid(
                    now,
                    phase,
                    speed,
                    loco,
                    intent,
                    combat,
                    brain,
                    vitals,
                    downed,
                    weapon.copied().unwrap_or_default(),
                    &mut st,
                    &mut materials,
                    stand_in,
                );
                Box::new(pose)
            }
            Some(species) => {
                let state = mind.map(|m| m.state).unwrap_or_default();
                let since = mind.map(|m| m.since).unwrap_or(0.0);
                let biting = mind.is_some_and(|m| now < m.bite_ready - 0.55);
                let flap = bird.map(|b| b.flap).unwrap_or(0.0);
                let male = animal.is_some_and(|a| a.male);
                Box::new(move |p| {
                    animal::animal_pose(
                        species, p, state, since, now, phase, speed, loco, biting, flap, male,
                    )
                })
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
