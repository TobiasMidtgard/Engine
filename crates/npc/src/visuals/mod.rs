//! Stand-in bodies: simple figures built from primitives and posed every step from what the
//! AI is doing, so behavior reads clearly before real character models exist.
//!
//! **Frozen, no new moves.** This posing is the no-animation fallback: engine_npc stays
//! swappable, so without an animation module these poses still animate every fighter. Where
//! one answers a fighter's `BodyRequest` (`intent_body.rs`) it poses that body and this module
//! writes none of it, decided per body from the answer (tested in
//! `tests/intent_body.rs`). New moves go to the animation module, not here.
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
//! Fighters are ready for a character module's faces and bodies (see `character.rs`): they ask
//! for a look, write their `SkeletonPose` and `FaceControls`, wear blood, bruises and dirt,
//! and while a character module draws appearances, their own body is hidden (what they hold
//! stays).
//!
//! With `NpcSettings::show_intent` on, a ring on the ground marks where each NPC decided to
//! go and a line leads to it, colored by why: blue cover, orange flank, purple high ground,
//! grey falling back, yellow investigating or searching.

mod animal;
pub(crate) mod character;
pub(crate) mod fur;
mod humanoid;

use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::animation::{BodyRequest, SkeletonPose};
use engine_world::character::{Appearance, AppearanceBodies, AppearanceRequest, BodyFit};
use engine_world::combat::AttackWindow;
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};
use engine_world::{AttackEvent, AttackKind, DamageEvent};

use crate::agent::{
    Npc, NpcAction, NpcBrain, NpcClock, NpcCombat, NpcMotor, NpcRole, NpcSettings, NpcVitals,
    NpcWeapon,
};
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
    Option<&'a Npc>,
);

/// The fighter kit's shared images are in this world (see [`build_stand_ins`]).
#[derive(Resource)]
pub(crate) struct FighterImagesIn;

/// Species whose meshes and textures a worker is making ([`build_stand_ins`]).
#[derive(Resource, Default)]
pub(crate) struct AnimalArtWarming(Vec<Species>);

/// Keys of fighter art (a team's paint, a face) a worker is making ([`build_stand_ins`]).
#[derive(Resource, Default)]
pub(crate) struct FighterArtWarming(Vec<String>);

/// Spawn the parts of new stand-ins. The fighters' shared kit takes a second or more to make
/// the first time, and so does each species' art (3.4 s in one frame on the session VM when
/// the paddock's animals arrived), so both are built on a worker ([`engine_core::off_frame`])
/// and stand-ins get their parts once theirs is in.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_stand_ins(
    mut commands: Commands,
    q: Query<BuildItem, Without<StandInState>>,
    mut art: ArtAssets,
    mut warming: Local<bool>,
    images_in: Option<Res<FighterImagesIn>>,
    mut animals_warming: Local<Vec<Species>>,
    animals_in: Option<Res<AnimalArtWarming>>,
    mut fighters_warming: Local<Vec<String>>,
    fighters_in: Option<Res<FighterArtWarming>>,
    time: Res<Time>,
) {
    // A species whose job has put its art in is no longer warming (and one whose art was
    // released since is made again).
    let still: &[Species] = animals_in.as_ref().map_or(&[], |w| &w.0);
    animals_warming.retain(|s| still.contains(s));
    let still: &[String] = fighters_in.as_ref().map_or(&[], |w| &w.0);
    fighters_warming.retain(|k| still.contains(k));
    use crate::art::fighter;
    use engine_core::time::TimeSource;
    // Until this world's kit job has put its images in, humanoids wait (the job may have
    // finished the kit on its worker a moment before).
    let kit_ready =
        !art.available() || (fighter::kit_ready() && (!*warming || images_in.is_some()));
    if art.available() && !fighter::kit_ready() && !*warming && q.iter().any(|i| i.2.is_none()) {
        *warming = true;
        // The kit, and copies of the images every fighter shares (the atlas and each wear's
        // paint maps), made on the worker so the frame only moves them into the store.
        engine_core::off_frame::OffFrameCommands::off_frame(
            &mut commands,
            "npc fighter kit",
            || {
                let kit = fighter::kit();
                let mut sets = vec![("fighter/atlas".to_string(), kit.atlas())];
                sets.extend(
                    (0..fighter::WEARS).map(|w| (format!("fighter/paint{w}"), kit.paint_maps(w))),
                );
                sets
            },
            |world, sets| {
                let mut state = bevy_ecs::system::SystemState::<ArtAssets>::new(world);
                if let Ok(mut art) = state.get_mut(world) {
                    for (key, set) in sets {
                        let _ = art.textures(&key, move || set);
                    }
                }
                world.insert_resource(FighterImagesIn);
            },
        );
    }
    // A new fighter's paint takes ~20 ms to make: with the renderer and a real-time clock, a
    // few per frame and the rest on the next (a manual clock, as in tests, builds them all).
    let start = std::time::Instant::now();
    let budgeted = art.available() && matches!(time.source(), TimeSource::Realtime);
    for (e, stand_in, animal, weapon, faction, skills, npc) in &q {
        if animal.is_none() && !kit_ready {
            continue;
        }
        // With a manual clock (tests) the art is made on the spot, as the fighters' paint is.
        if let Some(species) = animal.map(|a| a.species)
            && budgeted
            && !crate::art::animals::species_ready(species, &art)
        {
            if !animals_warming.contains(&species) {
                animals_warming.push(species);
                warm_species(&mut commands, species);
            }
            continue;
        }
        if budgeted && start.elapsed().as_secs_f64() > 0.004 {
            break;
        }
        let medic = npc.is_some_and(|n| n.role == NpcRole::Medic);
        // A fighter whose look (a team's paint, a face) isn't made yet waits for a worker to
        // make it: 10-26 ms a texture set and up to 15 ms a mesh, 10-74 ms frames at the
        // fortress (VM) when they were made here.
        if animal.is_none() && budgeted {
            let mut plan = crate::art::Plan::new(&art);
            humanoid::parts(stand_in, weapon, faction, skills, medic, &mut plan);
            if !plan.jobs.is_empty() {
                warm_fighter(&mut commands, plan, &mut fighters_warming);
                continue;
            }
        }
        let parts = match animal.map(|a| a.species) {
            None => humanoid::parts(stand_in, weapon, faction, skills, medic, &mut art),
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

/// Make what a fighter's look still needs on a worker, the pieces no other job is making
/// already, then put them in the stores ([`FighterArtWarming`] says which are on the way).
fn warm_fighter(commands: &mut Commands, plan: crate::art::Plan, warming: &mut Vec<String>) {
    use crate::art::animals;
    let (keys, jobs): (Vec<String>, Vec<animals::Job>) = plan
        .into_jobs()
        .into_iter()
        .filter(|(k, _)| !warming.contains(k))
        .unzip();
    if jobs.is_empty() {
        return;
    }
    warming.extend(keys.iter().cloned());
    let queued = keys.clone();
    commands.queue(move |world: &mut World| {
        world
            .get_resource_or_init::<FighterArtWarming>()
            .0
            .extend(queued);
    });
    engine_core::off_frame::OffFrameCommands::off_frame(
        commands,
        "npc fighter art",
        move || animals::run_jobs(jobs),
        move |world, made| {
            let mut state = bevy_ecs::system::SystemState::<ArtAssets>::new(world);
            if let Ok(mut art) = state.get_mut(world) {
                animals::add_species(made, &mut art);
            }
            if let Some(mut w) = world.get_resource_mut::<FighterArtWarming>() {
                w.0.retain(|k| !keys.contains(k));
            }
        },
    );
}

/// Make `species`' art on a worker, then put it in the stores ([`AnimalArtWarming`] says
/// which species are on the way).
fn warm_species(commands: &mut Commands, species: Species) {
    use crate::art::animals;
    commands.queue(move |world: &mut World| {
        world
            .get_resource_or_init::<AnimalArtWarming>()
            .0
            .push(species);
    });
    engine_core::off_frame::OffFrameCommands::off_frame(
        commands,
        "npc animal art",
        move || animals::make_species(species),
        move |world, made| {
            let mut state = bevy_ecs::system::SystemState::<ArtAssets>::new(world);
            if let Ok(mut art) = state.get_mut(world) {
                animals::add_species(made, &mut art);
            }
            if let Some(mut w) = world.get_resource_mut::<AnimalArtWarming>() {
                w.0.retain(|s| *s != species);
            }
        },
    );
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

/// Headgear a look leaves to the fighter's helmet: a mail coif for it to sit over, or an
/// open helm (bare under it).
const HELMED: [&str; 2] = ["mail_coif", "open_helm"];
/// How much bigger a helmet is drawn over a person, about the middle of the head: about 1 cm
/// more over the crown, so it sits over the look's coif rather than in it.
const HELMET_GROWTH: f32 = 1.09;

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
        // A look a character module may draw instead of the stand-in body.
        Option<&'a Appearance>,
        Has<AppearanceRequest>,
        // That person's own skeleton (the pose is sized to it).
        Option<&'a BodyFit>,
        // Posed by the animation module from intent (`intent_body.rs`): its pose, and
        // whether it holds the props of its own.
        Option<&'a SkeletonPose>,
        Option<&'a crate::intent_body::IntentBody>,
        Has<BodyRequest>,
        Has<AttackWindow>,
        // What its hands still hold (the combat module empties a hand whose arm is ruined).
        Option<&'a engine_world::combat::Loadout>,
    ),
);

/// What a fighter's pose reads of another (the attacker whose blow it meets).
type FoeItem<'a> = (
    &'a Transform,
    Option<&'a NpcCombat>,
    Option<&'a LocomotionState>,
    Option<&'a NpcWeapon>,
);

/// The attacks telegraphed and the blows landed this step, and where the downed fighters'
/// chests lie and a mate's knees go beside them, from either side (kept between steps to
/// reuse the space).
#[derive(Default)]
pub(crate) struct Sightings {
    attacks: Vec<(Entity, Option<Entity>, AttackKind)>,
    blows: Vec<DamageEvent>,
    lying: Vec<(Entity, DVec3, [DVec3; 2])>,
}

/// Pose every stand-in from its owner's state (a fighter's also from the attacks telegraphed
/// at it, the blows that land on it, the attacker whose blow its guard meets, and the downed
/// mate it revives). While a character module draws appearances, a fighter with a look shows
/// only what it holds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pose_stand_ins(
    mut owners: Query<PoseItem, (Without<StandInPart>, Without<crate::rest::Resting>)>,
    foes: Query<FoeItem, Without<StandInPart>>,
    mut parts: Query<(&StandInPart, &mut Transform, &mut MeshRenderer)>,
    mut attacks: MessageReader<AttackEvent>,
    mut blows: MessageReader<DamageEvent>,
    mut seen: Local<Sightings>,
    seeds: Query<&crate::seed::Seed>,
    drawn: Option<Res<AppearanceBodies>>,
) {
    let seen = &mut *seen;
    seen.attacks.clear();
    seen.attacks
        .extend(attacks.read().map(|a| (a.attacker, a.target, a.kind)));
    seen.blows.clear();
    // A bleeding wound is no blow to flinch from.
    seen.blows.extend(
        blows
            .read()
            .filter(|b| b.kind != engine_world::DamageKind::Bleed)
            .cloned(),
    );
    // How each downed fighter lay as last posed (a mate reviving it kneels there and puts its
    // hands on the chest).
    seen.lying.clear();
    seen.lying.extend(owners.iter().filter_map(|item| {
        let (me, _, st, _, owner, .., vitals, _, _, extra) = item;
        // A body the animation module lets fall lies where it fell.
        if let (_, _, _, _, _, _, _, _, Some(pose), Some(body), true, _, _) = extra
            && body.asked
            && vitals.is_some_and(|v| v.downed)
            && let Some(lying) = crate::intent_body::drawn_lying(owner, pose)
        {
            let (chest, knees) = crate::intent_body::kneel_by(owner.translation.y, lying);
            return Some((me, chest, knees));
        }
        let l = st.motion.lying()?;
        let world = |p: Vec3| owner.translation + (owner.rotation * p).as_dvec3();
        Some((me, world(l.chest), l.knees.map(world)))
    }));
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
        let (
            animal,
            mind,
            bird,
            motor,
            memory,
            looks,
            asks,
            fit,
            pose,
            body,
            requested,
            armed,
            holds,
        ) = animal;
        // An animal the animation module has a body for (`animal_body.rs`): that body is drawn
        // and posed there, so none of the stand-in shows.
        if animal.is_some() && requested && pose.is_some() {
            for &c in children {
                if let Ok((_, _, mut r)) = parts.get_mut(c)
                    && r.visible
                {
                    r.visible = false;
                }
            }
            continue;
        }
        let drawn_elsewhere = drawn.is_some() && animal.is_none() && (looks.is_some() || asks);
        // The animation module poses it and holds its weapons (`intent_body.rs`).
        let posed_elsewhere = drawn_elsewhere
            && body.is_some_and(|b| b.asked)
            && requested
            && armed
            && pose.is_some();
        let mut materials: Vec<(Part, StandardMaterial)> = Vec::new();
        let placed: Box<dyn Fn(Part) -> Placed> = match animal.map(|a| a.species) {
            None => {
                // The mate it revives: its chest, and where the knees go on the side it kneels
                // (the brain's spot is there).
                let patient = brain
                    .filter(|b| b.action == NpcAction::Revive)
                    .and_then(|b| {
                        let &(_, chest, knees) =
                            seen.lying.iter().find(|l| Some(l.0) == b.revive)?;
                        let spot = b.destination.unwrap_or(owner.translation);
                        let near = |p: DVec3| DVec2::new(p.x - spot.x, p.z - spot.z).length();
                        let side = usize::from(near(knees[1]) < near(knees[0]));
                        Some((chest, knees[side]))
                    });
                let view = humanoid::Seen {
                    me,
                    vary: seeds.get(me).map_or(0.0, |s| s.unit()),
                    at: owner.translation,
                    attacks: &seen.attacks,
                    blows: &seen.blows,
                    dash_secs: motor.map_or(0.22, |m| m.dash_secs),
                    foe: &foe,
                    memory,
                    patient,
                    size: if drawn_elsewhere {
                        fit.map_or(1.0, character::size_of)
                    } else {
                        1.0
                    },
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
                    vary: seeds.get(me).map_or(0.0, |s| s.unit()),
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
        // On a person drawn elsewhere, the pieces it wears move to that person's joints: the
        // helmet only where the look leaves the head to one (grown to sit over the look's own
        // coif), the tabards only where the look has none of its own.
        let worn = drawn_elsewhere
            .then(|| fit.zip(st.motion.body()))
            .flatten()
            .and_then(|(f, (s, _))| character::worn_frames(f, s, pose.filter(|_| posed_elsewhere)));
        let ranged = weapon.is_some_and(|w| w.style == crate::WeaponStyle::Ranged);
        let helmed = looks.is_some_and(|l| HELMED.contains(&l.headgear.as_str()));
        let tabarded = looks.is_some_and(|l| l.outfit.accent_on == "tabard");
        let helmet = placed(Part::Head);
        let tending = brain.is_some_and(|b| b.kneeling());
        for child in children.iter() {
            let Ok((part, mut t, mut r)) = parts.get_mut(child) else {
                continue;
            };
            let mut p = placed(part.part);
            let mut scale = part.scale;
            // In the hands the animation module poses: the weapon and shield, carried there as
            // the stand-in holds them.
            // (Not what the stand-in has set down or dropped: that stays where it lies.)
            // (Archers and riflemen shoot the animation module's bow and carbine: their own
            // are put away.)
            let hand = humanoid::in_hand(part.part).filter(|_| !ranged);
            // Knocked from its hand (`wounds`): no longer drawn in it.
            let lost = posed_elsewhere
                && hand.is_some_and(|k| {
                    let w = crate::wounds::Wounds::of(
                        None,
                        holds,
                        &weapon.copied().unwrap_or_default(),
                    );
                    if k == 1 { w.disarmed } else { w.shield_lost }
                });
            let hand = hand.filter(|_| !lost);
            let carried = match (posed_elsewhere, worn, hand) {
                (true, Some(frames), Some(k))
                    if !tending && p.pos.distance(frames[3 + k].own) < 0.3 =>
                {
                    (p.pos, p.rot) = frames[3 + k].carry(p.pos, p.rot);
                    true
                }
                _ => false,
            };
            // A rifleman's muzzle flash, at the muzzle of the animation module's carbine in
            // the drawn right hand: from the wrist bone, 8.5 cm to the fist's middle, then
            // along the barrel (the hand's −Y) to the muzzle, the bore 7.5 cm toward −Z.
            let flash = match (posed_elsewhere && ranged, worn, part.part) {
                (true, Some(frames), Part::Muzzle) => {
                    let hand = frames[4];
                    p.pos = hand.at + hand.rot * (Vec3::NEG_Y * 0.805 + Vec3::NEG_Z * 0.075);
                    p.rot = hand.rot * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                    true
                }
                _ => false,
            };
            let kept = match (worn, humanoid::worn(part.part)) {
                _ if carried => None,
                (Some(_), Some(humanoid::Worn::Head)) if !helmed => None,
                (Some(_), Some(humanoid::Worn::Tabard)) if tabarded => None,
                (Some([hips, chest, head, ..]), Some(w)) => {
                    let frame = match w {
                        humanoid::Worn::Hips | humanoid::Worn::Tabard => hips,
                        humanoid::Worn::Chest => chest,
                        humanoid::Worn::Head => head,
                    };
                    (p.pos, p.rot) = frame.carry(p.pos, p.rot);
                    if w == humanoid::Worn::Head {
                        // Grown about the middle of the head.
                        let (at, rot) = head.carry(helmet.pos, helmet.rot);
                        let middle = at + rot * crate::art::fighter::rig::HEAD_MIDDLE;
                        p.pos = middle + HELMET_GROWTH * (p.pos - middle);
                        scale *= HELMET_GROWTH;
                    }
                    Some(w)
                }
                _ => None,
            };
            let new = Transform::from_translation(p.pos.as_dvec3())
                .with_rotation(p.rot)
                .with_scale(scale);
            if *t != new {
                *t = new;
            }
            // What it holds stays: on a body the animation module poses, only what it carries
            // there (its own props are hidden, `hide_props`).
            // Kneeling by a mate, the drawn hands go to its chest: the weapon shows once it
            // lies on the ground beside the knee, not on its way down in the stand-in's hand.
            let set_down = posed_elsewhere
                && !carried
                && hand.is_some()
                && !(tending && hand == Some(1) && p.pos.y > LAID_HEIGHT);
            let held = humanoid::held(part.part)
                && !lost
                && (!posed_elsewhere || carried || set_down || flash);
            let visible = p.visible && !(drawn_elsewhere && !held && kept.is_none());
            if r.visible != visible {
                r.visible = visible;
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

/// Below this height (m, in the body's frame) a weapon set down to revive a mate lies on the
/// ground.
const LAID_HEIGHT: f32 = 0.15;

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
            "revive" => 0x3fe0c8,
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
